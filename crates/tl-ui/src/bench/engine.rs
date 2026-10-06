//! `BenchEngine`: the scripted `EngineClient` the bench scenes run against (ADR-013 fake engine,
//! UI-PRF-053).
//!
//! One publisher thread builds full snapshots from the generators in [`super::synth`] and
//! publishes them like the real bridge (ADR-005): replace the latest snapshot, then send one wake
//! token. In the `bridge` scene the CPU total and kernel series come from a real client.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smallvec::smallvec;
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::details::DetailsSnapshot;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::history::HistoryView;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::series::{Sample, SeriesRing, SeriesView};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ConnectionsSnapshot, InventorySnapshot, SeriesSet, ServicesSnapshot, Snapshot,
    StartupSnapshot, SystemSnapshot, WindowsSnapshot,
};
use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology, CpuTotals, MemoryTotals, Metric};
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use super::synth::{
    BenchRng, CORES, CpuFrame, CpuSynth, DeviceSynth, PrefilledSeries, StatsFrame, StatsSynth,
    prefill, seconds,
};

const PERIOD_10HZ_NS: u64 = 100_000_000;
const PERIOD_1HZ_NS: u64 = 1_000_000_000;
/// Every 10th tick also appends the 1 Hz samples.
const TICKS_PER_1HZ: u64 = 10;
/// ADR-003 fast tier capacity at 100 ms.
const CAP_10HZ: u32 = SeriesRing::fast_capacity(PERIOD_10HZ_NS);
/// ADR-003 fast tier capacity at 1 s.
const CAP_1HZ: u32 = SeriesRing::fast_capacity(PERIOD_1HZ_NS);
/// Entries kept for [`BenchEngine::bridge_origin`].
const BRIDGE_ORIGINS: usize = 1_024;

/// Placeholder memory size for the MEM_USED series, in bytes (64 GiB).
const MEM_TOTAL_BYTES: u64 = 64 << 30;
/// Network series are bytes per second; the synthetic values are Mbps.
const BYTES_PER_S_PER_MBPS: f32 = 125_000.0;

/// Placeholder topology (R-160 OI-2): 16 cores x 2 threads.
const PHYS_CORES: usize = CORES / 2;

/// Device indexes of the placeholder catalog.
const DEV_PACKAGE: u16 = 0;
const DEV_FIRST_CORE: u16 = 1;
const DEV_MEMORY: u16 = DEV_FIRST_CORE + CORES as u16;
const DEV_GPU: u16 = DEV_MEMORY + 1;
const DEV_FIRST_DISK: u16 = DEV_GPU + 1;
const DEV_FIRST_NET: u16 = DEV_FIRST_DISK + 4;
const NET_NAMES: [&str; 2] = ["enp38s0", "tailscale0"];
const GPU_PCI: &str = "0000:01:00.0";

/// Track indexes. Tracks are sorted by `SeriesId`, so `SeriesSet.series` keeps their order.
const TR_TOTAL: usize = 0;
const TR_TOTAL_KERNEL: usize = 1;
/// Core `c` has its total at `TR_FIRST_CORE + 2c` and its kernel share right after.
const TR_FIRST_CORE: usize = 2;
const TR_MEMORY: usize = TR_FIRST_CORE + 2 * CORES;
const TR_GPU: usize = TR_MEMORY + 1;
const TR_FIRST_DISK: usize = TR_GPU + 1;
/// Interface `n` has RX at `TR_FIRST_NET + 2n` and TX right after.
const TR_FIRST_NET: usize = TR_FIRST_DISK + 4;
const TRACKS: usize = TR_FIRST_NET + 4;

/// Where the CPU total comes from.
pub enum BenchFeed {
    /// Every series synthetic (perf-cpu, static, table-2000).
    Synthetic,
    /// CPU_TOTAL and CPU_KERNEL of the all-CPUs device come from this client; the 32 per-core
    /// series and every other value stay synthetic (bridge scene).
    Bridge(Arc<dyn EngineClient>),
}

/// State the publisher thread and the client side share.
struct Shared {
    latest: Mutex<Arc<Snapshot>>,
    wake_tx: async_channel::Sender<()>,
    commands: Mutex<Vec<Command>>,
    demand: Mutex<Option<Demand>>,
    settings: Mutex<Option<EngineSettings>>,
    published_10hz: AtomicU64,
    bridge_samples: AtomicU64,
    /// `(sample t_ns, real published_ns)`, oldest first.
    bridge_origins: Mutex<VecDeque<(TimeNs, TimeNs)>>,
    stopping: AtomicBool,
}

/// Locks `m`, ignoring poison: a publisher panic aborts the bench run anyway.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    /// ADR-005 publish order: store the snapshot, then one wake token. A full channel means a
    /// wake is already pending.
    fn publish(&self, snap: Snapshot) {
        *lock(&self.latest) = Arc::new(snap);
        let _ = self.wake_tx.try_send(());
    }
}

/// What wakes the publisher thread before its next deadline.
enum Signal {
    Stop,
    RealWake,
}

/// A fake engine with a scripted 10 Hz publisher.
pub struct BenchEngine {
    shared: Arc<Shared>,
    wake_rx: async_channel::Receiver<()>,
    /// Kept so the action event channel stays open and empty.
    _ev_tx: async_channel::Sender<ActionEvent>,
    ev_rx: async_channel::Receiver<ActionEvent>,
    signal_tx: Mutex<Option<mpsc::Sender<Signal>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl BenchEngine {
    /// Prefills every series, publishes the first snapshot (sequence 1) and starts the
    /// `tl-bench-pub` thread. Tick `k` runs at `t0 + k * 100 ms` on CLOCK_BOOTTIME, so `t0` is
    /// normally `now_boot_ns()`. `t0` must be at least 60 s.
    pub fn start(feed: BenchFeed, seed: u64, t0: TimeNs) -> Arc<Self> {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (ev_tx, ev_rx) = async_channel::bounded(256);
        let (signal_tx, signal_rx) = mpsc::channel();

        let (mut publisher, real) = match feed {
            BenchFeed::Synthetic => (Publisher::new(seed, t0, None), None),
            BenchFeed::Bridge(client) => {
                let wake = client.wake();
                (Publisher::new(seed, t0, Some(client)), Some(wake))
            }
        };
        let shared = Arc::new(Shared {
            latest: Mutex::new(Arc::new(publisher.snapshot())),
            wake_tx,
            commands: Mutex::new(Vec::new()),
            demand: Mutex::new(None),
            settings: Mutex::new(None),
            published_10hz: AtomicU64::new(0),
            bridge_samples: AtomicU64::new(0),
            bridge_origins: Mutex::new(VecDeque::with_capacity(BRIDGE_ORIGINS)),
            stopping: AtomicBool::new(false),
        });

        if let Some(real_wake) = real {
            // Forwards the real wake tokens. It ends when the real client closes its channel or
            // on the first token after the publisher has exited.
            let tx = signal_tx.clone();
            std::thread::Builder::new()
                .name("tl-bench-bridge".into())
                .spawn(move || {
                    while real_wake.recv_blocking().is_ok() {
                        if tx.send(Signal::RealWake).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn tl-bench-bridge");
        }

        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tl-bench-pub".into())
            .spawn(move || run(publisher, &thread_shared, &signal_rx, t0))
            .expect("spawn tl-bench-pub");

        Arc::new(Self {
            shared,
            wake_rx,
            _ev_tx: ev_tx,
            ev_rx,
            signal_tx: Mutex::new(Some(signal_tx)),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Stops the publisher and joins its thread. No wake token is sent after this returns.
    /// A publisher panic is re-raised here.
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        if let Some(tx) = lock(&self.signal_tx).take() {
            let _ = tx.send(Signal::Stop);
        }
        if let Some(thread) = lock(&self.thread).take()
            && let Err(panic) = thread.join()
        {
            std::panic::resume_unwind(panic);
        }
    }

    /// Every command sent, in call order.
    pub fn commands(&self) -> Vec<Command> {
        lock(&self.shared.commands).clone()
    }

    /// The last `set_demand` value.
    pub fn demand(&self) -> Option<Demand> {
        lock(&self.shared.demand).clone()
    }

    /// The last `set_settings` value.
    pub fn settings(&self) -> Option<EngineSettings> {
        lock(&self.shared.settings).clone()
    }

    /// Publishes made on a 10 Hz deadline. Bridge republishes between deadlines are not counted.
    pub fn published_10hz(&self) -> u64 {
        self.shared.published_10hz.load(Ordering::Acquire)
    }

    /// CPU_TOTAL samples copied from the real client.
    pub fn bridge_samples(&self) -> u64 {
        self.shared.bridge_samples.load(Ordering::Acquire)
    }

    /// The `published_ns` of the real snapshot that carried the copied sample at `t_ns`, among
    /// the last 1,024 copied samples.
    pub fn bridge_origin(&self, t_ns: TimeNs) -> Option<TimeNs> {
        lock(&self.shared.bridge_origins)
            .iter()
            .rev()
            .find(|&&(t, _)| t == t_ns)
            .map(|&(_, published)| published)
    }
}

impl Drop for BenchEngine {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.stop();
        }
    }
}

impl EngineClient for BenchEngine {
    fn latest(&self) -> Arc<Snapshot> {
        Arc::clone(&lock(&self.shared.latest))
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.ev_rx.clone()
    }

    fn set_demand(&self, demand: Demand) {
        *lock(&self.shared.demand) = Some(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        *lock(&self.shared.settings) = Some(settings);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        lock(&self.shared.commands).push(cmd);
        Ok(())
    }

    /// The handle's channels are closed: no entries ever arrive.
    fn journal(&self, _query: JournalQuery) -> JournalHandle {
        let (_events_tx, events) = async_channel::bounded(1);
        let (control, _control_rx) = async_channel::bounded(1);
        JournalHandle::new(events, control)
    }
}

/// The publisher loop: absolute deadlines every 100 ms from `t0`, no drift.
fn run(mut publisher: Publisher, shared: &Shared, signals: &mpsc::Receiver<Signal>, t0: TimeNs) {
    let mut tick: u64 = 0;
    loop {
        let deadline = t0 + (tick + 1) * PERIOD_10HZ_NS;
        loop {
            if shared.stopping.load(Ordering::Acquire) {
                return;
            }
            let now = now_boot_ns();
            if now >= deadline {
                break;
            }
            match signals.recv_timeout(Duration::from_nanos(deadline - now)) {
                Ok(Signal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(Signal::RealWake) => {
                    if publisher.copy_bridge(shared) {
                        shared.publish(publisher.snapshot());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        tick += 1;
        publisher.tick(tick, shared);
        shared.publish(publisher.snapshot());
        shared.published_10hz.fetch_add(1, Ordering::AcqRel);
    }
}

/// One published series: its ring and the view of the last publish.
struct Track {
    ring: SeriesRing,
    view: Arc<SeriesView>,
    dirty: bool,
}

impl Track {
    fn new(id: SeriesId, period_ns: u64, capacity: u32, prefill: &[Sample]) -> Self {
        let mut ring = SeriesRing::with_capacity(capacity);
        for &s in prefill {
            ring.push(s);
        }
        let view = Arc::new(view_of(&ring, id, period_ns));
        Self {
            ring,
            view,
            dirty: false,
        }
    }

    fn push(&mut self, s: Sample) {
        self.ring.push(s);
        self.dirty = true;
    }

    /// A new view only when the ring changed; else the old `Arc` and its version stay.
    fn refresh(&mut self) {
        if self.dirty {
            self.view = Arc::new(view_of(&self.ring, self.view.id, self.view.period_ns));
            self.dirty = false;
        }
    }
}

/// Chronological copy of `ring` in one `Arc<[Sample]>` allocation.
fn view_of(ring: &SeriesRing, id: SeriesId, period_ns: u64) -> SeriesView {
    SeriesView {
        id,
        version: ring.version(),
        period_ns,
        samples: ring.iter().collect(),
    }
}

/// The newest copied real sample, per series.
#[derive(Default)]
struct BridgeCursor {
    total: Option<TimeNs>,
    kernel: Option<TimeNs>,
}

/// Owns the generators and the rings. Runs on the publisher thread.
struct Publisher {
    rng: BenchRng,
    cpu: CpuSynth,
    devices: DeviceSynth,
    stats: StatsSynth,
    t0: TimeNs,
    tracks: Vec<Track>,
    /// Components that never change, cloned into every snapshot.
    template: Snapshot,
    system: Arc<SystemSnapshot>,
    series_version: u64,
    sequence: u64,
    bridge: Option<Arc<dyn EngineClient>>,
    cursor: BridgeCursor,
}

impl Publisher {
    fn new(seed: u64, t0: TimeNs, bridge: Option<Arc<dyn EngineClient>>) -> Self {
        let mut rng = BenchRng::new(seed);
        let mut cpu = CpuSynth::new(&mut rng);
        let mut devices = DeviceSynth::new();
        let pre = prefill(&mut cpu, &mut devices, &mut rng, t0);
        let stats = StatsSynth::new(seconds(t0));
        let system = Arc::new(SystemSnapshot {
            header: header(0, t0, Validity::WarmingUp),
            cpu_topology: Arc::new(topology()),
            ..SystemSnapshot::default()
        });
        Self {
            rng,
            cpu,
            devices,
            stats,
            t0,
            tracks: tracks(&pre),
            template: template(),
            system,
            series_version: 1,
            sequence: 1,
            bridge,
            cursor: BridgeCursor::default(),
        }
    }

    /// Appends tick `k`'s samples at `t0 + k * 100 ms`.
    fn tick(&mut self, k: u64, shared: &Shared) {
        let t_ns = self.t0 + k * PERIOD_10HZ_NS;
        let t_s = seconds(t_ns);
        let f = self.cpu.sample(t_s);
        for (c, (&v, &kv)) in f.per_core.iter().zip(&f.per_core_kernel).enumerate() {
            self.tracks[TR_FIRST_CORE + 2 * c].push(Sample::valid(t_ns, v));
            self.tracks[TR_FIRST_CORE + 2 * c + 1].push(Sample::valid(t_ns, kv));
        }
        if self.bridge.is_some() {
            self.copy_bridge(shared);
        } else {
            self.tracks[TR_TOTAL].push(Sample::valid(t_ns, f.total));
            self.tracks[TR_TOTAL_KERNEL].push(Sample::valid(t_ns, f.total_kernel));
        }
        if k.is_multiple_of(TICKS_PER_1HZ) {
            self.tick_1hz(t_ns, &f);
        }
        self.series_version += 1;
    }

    fn tick_1hz(&mut self, t_ns: TimeNs, f: &CpuFrame) {
        let d = self.devices.sample_1hz(&mut self.rng);
        let mem_used = (d.memory_pct as f64 / 100.0 * MEM_TOTAL_BYTES as f64) as u64;
        self.tracks[TR_MEMORY].push(Sample::valid(t_ns, mem_used as f32));
        self.tracks[TR_GPU].push(Sample::valid(t_ns, d.gpu_pct));
        for (i, &v) in d.disk_pct.iter().enumerate() {
            self.tracks[TR_FIRST_DISK + i].push(Sample::valid(t_ns, v));
        }
        for (i, &mbps) in d.net_mbps.iter().enumerate() {
            let bps = mbps * BYTES_PER_S_PER_MBPS;
            self.tracks[TR_FIRST_NET + 2 * i].push(Sample::valid(t_ns, bps));
            self.tracks[TR_FIRST_NET + 2 * i + 1].push(Sample::valid(t_ns, bps));
        }

        let s = self.stats.sample_1hz(&mut self.rng, seconds(t_ns), f);
        let version = self.system.header.version + 1;
        self.system = Arc::new(SystemSnapshot {
            header: header(version, t_ns, Validity::Valid),
            cpu: cpu_totals(&s),
            memory: memory_totals(mem_used),
            cpu_topology: Arc::clone(&self.system.cpu_topology),
            boot_time_s: boot_time_s(s.up_time_s),
            ..SystemSnapshot::default()
        });
    }

    /// Copies the real client's new CPU_TOTAL and CPU_KERNEL samples of its `CpuPackage` device.
    /// The first read copies only the newest sample. Returns true when anything was copied.
    fn copy_bridge(&mut self, shared: &Shared) -> bool {
        let Some(client) = &self.bridge else {
            return false;
        };
        let real = client.latest();
        let Some(pkg) = real.devices.find(&DeviceIdentity::CpuPackage) else {
            return false;
        };
        let idx = pkg.idx;
        let mut copied = 0u64;
        if let Some(view) = real.series.get(SeriesId {
            device: idx,
            metric: MetricId::CPU_TOTAL,
        }) {
            let mut origins = lock(&shared.bridge_origins);
            for s in new_samples(&view.samples, &mut self.cursor.total) {
                self.tracks[TR_TOTAL].push(s);
                if origins.len() == BRIDGE_ORIGINS {
                    origins.pop_front();
                }
                origins.push_back((s.t_ns, real.published_ns));
                copied += 1;
            }
        }
        if let Some(view) = real.series.get(SeriesId {
            device: idx,
            metric: MetricId::CPU_KERNEL,
        }) {
            for s in new_samples(&view.samples, &mut self.cursor.kernel) {
                self.tracks[TR_TOTAL_KERNEL].push(s);
            }
        }
        shared.bridge_samples.fetch_add(copied, Ordering::AcqRel);
        if copied > 0 {
            self.series_version += 1;
        }
        copied > 0
    }

    /// The next snapshot: sequence + 1, `published_ns = now_boot_ns()`. Unchanged series keep
    /// their `Arc<SeriesView>`.
    fn snapshot(&mut self) -> Snapshot {
        let mut newest = 0;
        for t in &mut self.tracks {
            t.refresh();
        }
        let views: Arc<[Arc<SeriesView>]> = self
            .tracks
            .iter()
            .map(|t| {
                if let Some(s) = t.view.samples.last() {
                    newest = newest.max(s.t_ns);
                }
                Arc::clone(&t.view)
            })
            .collect();
        let series = Arc::new(SeriesSet {
            header: header(self.series_version, newest, Validity::Valid),
            series: views,
            slow: Arc::from([]),
        });
        let sequence = self.sequence;
        self.sequence += 1;
        Snapshot {
            sequence,
            published_ns: now_boot_ns(),
            system: Arc::clone(&self.system),
            series,
            ..self.template.clone()
        }
    }
}

/// The samples after `*cursor`, oldest first, and moves the cursor to the newest. With no cursor
/// yet, only the newest sample.
fn new_samples<'a>(
    samples: &'a [Sample],
    cursor: &mut Option<TimeNs>,
) -> impl Iterator<Item = Sample> + 'a {
    let start = match *cursor {
        Some(t) => samples.partition_point(|s| s.t_ns <= t),
        None => samples.len().saturating_sub(1),
    };
    let fresh = &samples[start..];
    if let Some(last) = fresh.last() {
        *cursor = Some(last.t_ns);
    }
    fresh.iter().copied()
}

fn header(version: u64, t_ns: TimeNs, validity: Validity) -> ComponentHeader {
    ComponentHeader {
        version,
        observed_start: t_ns,
        observed_end: t_ns,
        validity,
    }
}

fn unsupported() -> ComponentHeader {
    ComponentHeader {
        validity: Validity::Unsupported,
        ..ComponentHeader::default()
    }
}

/// The components the scenes do not draw: defaults with `Unsupported` headers.
fn template() -> Snapshot {
    let mut snap = Snapshot::empty(BootId(0), 0);
    snap.devices = Arc::new(catalog());
    snap.connections = Arc::new(ConnectionsSnapshot {
        header: unsupported(),
        ..ConnectionsSnapshot::default()
    });
    snap.services = Arc::new(ServicesSnapshot {
        header: unsupported(),
        ..ServicesSnapshot::default()
    });
    snap.startup = Arc::new(StartupSnapshot {
        header: unsupported(),
        ..StartupSnapshot::default()
    });
    snap.inventory = Arc::new(InventorySnapshot {
        header: unsupported(),
        ..InventorySnapshot::default()
    });
    snap.windows = Arc::new(WindowsSnapshot {
        header: unsupported(),
        ..WindowsSnapshot::default()
    });
    snap.details = Arc::new(DetailsSnapshot {
        header: unsupported(),
        ..DetailsSnapshot::default()
    });
    snap.history = Arc::new(HistoryView {
        header: unsupported(),
        ..HistoryView::default()
    });
    snap
}

/// Placeholder catalog (R-160 OI-2): package, 32 logical CPUs, then the 8 non-CPU cards.
fn catalog() -> DeviceCatalog {
    let s = |v: &str| Arc::<str>::from(v);
    let mut entries = Vec::with_capacity(usize::from(DEV_FIRST_NET) + NET_NAMES.len());
    let mut add = |identity: DeviceIdentity, name: &str, key: &str, net_kind: Option<NetKind>| {
        entries.push(DeviceEntry {
            idx: DeviceIdx(entries.len() as u16),
            identity,
            generation: 0,
            display_name: s(name),
            present: true,
            parent: None,
            persistent_key: s(key),
            net_kind,
        });
    };
    add(DeviceIdentity::CpuPackage, "CPU", "cpu", None);
    for id in 0..CORES as u16 {
        let name = format!("CPU {id}");
        let key = format!("cpu:{id}");
        add(DeviceIdentity::CpuLogical { id }, &name, &key, None);
    }
    add(DeviceIdentity::Memory, "Memory", "memory", None);
    add(
        DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: s(GPU_PCI),
        },
        "GPU 0",
        &format!("gpu:pci:{GPU_PCI}"),
        None,
    );
    for minor in 0..4u32 {
        let kernel = format!("nvme{minor}n1");
        add(
            DeviceIdentity::Disk {
                major: 259,
                minor,
                name: s(&kernel),
                wwn_or_serial: None,
            },
            &format!("Disk {minor}"),
            &format!("disk:{kernel}"),
            None,
        );
    }
    let kinds = [NetKind::Wired, NetKind::VpnTunnel];
    for ((ifindex, name), kind) in (2u32..).zip(NET_NAMES).zip(kinds) {
        add(
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex,
                name: s(name),
            },
            name,
            &format!("net:{name}"),
            Some(kind),
        );
    }
    DeviceCatalog {
        version: 1,
        entries: entries.into(),
    }
}

/// 32 logical CPUs, 16 cores of 2 threads, all `Standard`.
fn topology() -> CpuTopology {
    let mut online = CpuSet::empty();
    for cpu in 0..CORES as u16 {
        online.insert(cpu);
    }
    let cores = (0..PHYS_CORES as u16)
        .map(|id| CoreInfo {
            id,
            logical: smallvec![2 * id, 2 * id + 1],
            l3_group: 0,
            kind: CoreKind::Standard,
        })
        .collect();
    CpuTopology { online, cores }
}

/// Every series with its prefill, sorted by `SeriesId`.
fn tracks(pre: &PrefilledSeries) -> Vec<Track> {
    let fast = |device: u16, metric: MetricId, samples: &[Sample]| {
        let id = SeriesId {
            device: DeviceIdx(device),
            metric,
        };
        Track::new(id, PERIOD_10HZ_NS, CAP_10HZ, samples)
    };
    let slow = |device: u16, metric: MetricId, samples: &[Sample]| {
        let id = SeriesId {
            device: DeviceIdx(device),
            metric,
        };
        Track::new(id, PERIOD_1HZ_NS, CAP_1HZ, samples)
    };
    let mut out = Vec::with_capacity(TRACKS);
    out.push(fast(DEV_PACKAGE, MetricId::CPU_TOTAL, &pre.total));
    out.push(fast(DEV_PACKAGE, MetricId::CPU_KERNEL, &pre.total_kernel));
    for (c, (total, kernel)) in pre.per_core.iter().zip(&pre.per_core_kernel).enumerate() {
        let dev = DEV_FIRST_CORE + c as u16;
        out.push(fast(dev, MetricId::CPU_TOTAL, total));
        out.push(fast(dev, MetricId::CPU_KERNEL, kernel));
    }
    // pre.devices is Memory, GPU 0, Disk 0 to Disk 3, enp38s0, tailscale0.
    let memory: Vec<Sample> = pre.devices[0]
        .iter()
        .map(|s| {
            Sample::valid(
                s.t_ns,
                (s.value as f64 / 100.0 * MEM_TOTAL_BYTES as f64) as f32,
            )
        })
        .collect();
    out.push(slow(DEV_MEMORY, MetricId::MEM_USED, &memory));
    out.push(slow(DEV_GPU, MetricId::GPU_UTIL, &pre.devices[1]));
    for d in 0..4 {
        let dev = DEV_FIRST_DISK + d as u16;
        out.push(slow(dev, MetricId::DISK_ACTIVE, &pre.devices[2 + d]));
    }
    for n in 0..NET_NAMES.len() {
        let bps: Vec<Sample> = pre.devices[6 + n]
            .iter()
            .map(|s| Sample::valid(s.t_ns, s.value * BYTES_PER_S_PER_MBPS))
            .collect();
        let dev = DEV_FIRST_NET + n as u16;
        out.push(slow(dev, MetricId::NET_RX, &bps));
        out.push(slow(dev, MetricId::NET_TX, &bps));
    }
    debug_assert_eq!(out.len(), TRACKS);
    debug_assert!(out.windows(2).all(|w| w[0].view.id < w[1].view.id));
    out
}

/// The Details rows (UI-PRF-024) as CPU totals. Row 7 (Up time) goes to `boot_time_s`.
fn cpu_totals(s: &StatsFrame) -> CpuTotals {
    let power = match s.power_draw_w {
        Some(w) => Metric::valid(w),
        None => Metric::with(Validity::Unsupported),
    };
    CpuTotals {
        util_pct: Metric::valid(s.utilization_pct),
        kernel_pct: Metric::valid(s.kernel_pct),
        freq_mhz: Metric::valid(s.speed_mhz),
        processes: Metric::valid(s.processes),
        threads: Metric::valid(s.threads),
        handles: Metric::valid(u64::from(s.handles)),
        base_mhz: Metric::valid(s.base_speed_mhz),
        sockets: Metric::valid(s.sockets as u16),
        package_power_w: power,
        temp_c: Metric::valid(s.temperature_c),
        load_avg: Metric::valid(s.load_avg),
        ..CpuTotals::default()
    }
}

fn memory_totals(used: u64) -> MemoryTotals {
    MemoryTotals {
        validity: Validity::Valid,
        total: MEM_TOTAL_BYTES,
        used,
        available: MEM_TOTAL_BYTES - used,
        ..MemoryTotals::default()
    }
}

/// Wall-clock boot time that makes the Up time read `up_time_s` now.
fn boot_time_s(up_time_s: u64) -> i64 {
    let now_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    now_s as i64 - up_time_s as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fake_engine::{FakeEngine, empty_snapshot};
    use std::time::Instant;

    fn memory_id() -> SeriesId {
        SeriesId {
            device: DeviceIdx(DEV_MEMORY),
            metric: MetricId::MEM_USED,
        }
    }

    fn total_id() -> SeriesId {
        SeriesId {
            device: DeviceIdx(DEV_PACKAGE),
            metric: MetricId::CPU_TOTAL,
        }
    }

    fn quiet_shared() -> Shared {
        let (wake_tx, _) = async_channel::bounded(1);
        Shared {
            latest: Mutex::new(Arc::new(empty_snapshot())),
            wake_tx,
            commands: Mutex::new(Vec::new()),
            demand: Mutex::new(None),
            settings: Mutex::new(None),
            published_10hz: AtomicU64::new(0),
            bridge_samples: AtomicU64::new(0),
            bridge_origins: Mutex::new(VecDeque::new()),
            stopping: AtomicBool::new(false),
        }
    }

    #[test]
    fn publishes_at_10hz() {
        let engine = BenchEngine::start(BenchFeed::Synthetic, 42, now_boot_ns());
        let wake = engine.wake();
        std::thread::sleep(Duration::from_millis(1_000));
        let n = engine.published_10hz();
        engine.stop();
        assert!((9..=11).contains(&n), "published {n}");
        assert!(wake.try_recv().is_ok());
        assert_eq!(engine.latest().sequence, n + 1);
    }

    #[test]
    fn unchanged_series_reuse_arc() {
        let shared = quiet_shared();
        let t0 = 3_600 * PERIOD_1HZ_NS;
        let mut p = Publisher::new(42, t0, None);
        let mut prev = p.snapshot();
        for k in 1..=25 {
            p.tick(k, &shared);
            let next = p.snapshot();
            assert_eq!(next.sequence, prev.sequence + 1);
            let a = prev.series.get(memory_id()).expect("memory series");
            let b = next.series.get(memory_id()).expect("memory series");
            let one_hz = k % TICKS_PER_1HZ == 0;
            assert_eq!(Arc::ptr_eq(a, b), !one_hz, "tick {k}");
            assert_eq!(a.version == b.version, !one_hz, "tick {k}");
            assert_eq!(Arc::ptr_eq(&prev.system, &next.system), !one_hz);
            let ta = prev.series.get(total_id()).expect("total");
            let tb = next.series.get(total_id()).expect("total");
            assert!(!Arc::ptr_eq(ta, tb));
            assert!(Arc::ptr_eq(&prev.devices, &next.devices));
            prev = next;
        }
    }

    #[test]
    fn prefilled_before_first_tick() {
        // t0 one hour ahead, so no tick runs during the test.
        let engine = BenchEngine::start(
            BenchFeed::Synthetic,
            42,
            now_boot_ns() + 3_600 * PERIOD_1HZ_NS,
        );
        let snap = engine.latest();
        engine.stop();
        assert_eq!(snap.sequence, 1);
        assert_eq!(snap.series.series.len(), TRACKS);
        for view in snap.series.series.iter() {
            let want = if view.period_ns == PERIOD_10HZ_NS {
                600
            } else {
                60
            };
            assert_eq!(view.samples.len(), want, "{:?}", view.id);
        }
        let fast = snap
            .series
            .series
            .iter()
            .filter(|v| v.period_ns == PERIOD_10HZ_NS)
            .count();
        assert_eq!(fast, 2 + 2 * CORES);
        assert_eq!(engine.published_10hz(), 0);
    }

    #[test]
    fn catalog_has_41_entries_in_order() {
        let c = catalog();
        assert_eq!(c.entries.len(), 41);
        for (i, e) in c.entries.iter().enumerate() {
            assert_eq!(e.idx, DeviceIdx(i as u16));
            assert!(e.present);
            assert_eq!(e.generation, 0);
        }
        assert_eq!(c.entries[0].identity, DeviceIdentity::CpuPackage);
        assert_eq!(&*c.entries[0].display_name, "CPU");
        for id in 0..CORES as u16 {
            assert_eq!(
                c.entries[1 + usize::from(id)].identity,
                DeviceIdentity::CpuLogical { id }
            );
        }
        let names: Vec<&str> = c.entries[33..].iter().map(|e| &*e.display_name).collect();
        assert_eq!(names, &super::super::synth::DEVICE_CARDS[1..]);
        assert_eq!(c.entries[33].identity, DeviceIdentity::Memory);
        assert!(matches!(c.entries[34].identity, DeviceIdentity::Gpu { .. }));
        for (i, e) in c.entries[35..39].iter().enumerate() {
            assert!(matches!(
                e.identity,
                DeviceIdentity::Disk { major: 259, minor, .. } if minor == i as u32
            ));
        }
        assert!(matches!(
            c.entries[39].identity,
            DeviceIdentity::Net {
                ifindex: 2,
                netns_ino: 1,
                ..
            }
        ));
        assert!(matches!(
            c.entries[40].identity,
            DeviceIdentity::Net {
                ifindex: 3,
                netns_ino: 1,
                ..
            }
        ));

        let topo = topology();
        assert_eq!(topo.online.count(), 32);
        assert_eq!(topo.cores.len(), 16);
        assert!(topo.cores.iter().all(|c| c.logical.len() == 2));
    }

    /// A fake real engine: a catalog with the package at `DeviceIdx(0)` and a CPU_TOTAL series
    /// that grows by one sample per publish.
    fn fake_publish(fake: &FakeEngine, samples: &[Sample], published_ns: TimeNs, seq: u64) {
        let pkg = DeviceEntry {
            idx: DeviceIdx(0),
            identity: DeviceIdentity::CpuPackage,
            generation: 0,
            display_name: Arc::from("CPU"),
            present: true,
            parent: None,
            persistent_key: Arc::from("cpu"),
            net_kind: None,
        };
        let view = |metric: MetricId| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric,
                },
                version: seq,
                period_ns: PERIOD_10HZ_NS,
                samples: samples.into(),
            })
        };
        let mut snap = empty_snapshot();
        snap.sequence = seq;
        snap.published_ns = published_ns;
        snap.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from([pkg]),
        });
        snap.series = Arc::new(SeriesSet {
            header: header(seq, 0, Validity::Valid),
            series: Arc::from([view(MetricId::CPU_TOTAL), view(MetricId::CPU_KERNEL)]),
            slow: Arc::from([]),
        });
        fake.publish(snap);
    }

    #[test]
    fn bridge_feed_copies_real_total() {
        let fake = FakeEngine::new();
        let t0 = now_boot_ns();
        let base_t = t0 + 50_000_000;
        let engine = BenchEngine::start(BenchFeed::Bridge(fake.clone()), 42, t0);
        let mut real: Vec<Sample> = Vec::new();
        let mut sent = Vec::new();
        let start = Instant::now();
        for i in 0..10u64 {
            let t_ns = base_t + i * PERIOD_10HZ_NS;
            let s = Sample::valid(t_ns, 1_000.0 + i as f32);
            real.push(s);
            let published_ns = 7_000_000_000 + i;
            fake_publish(&fake, &real, published_ns, i + 1);
            sent.push((s, published_ns));
            let next = start + Duration::from_millis(100 * (i + 1));
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        std::thread::sleep(Duration::from_millis(150));
        let snap = engine.latest();
        engine.stop();

        assert_eq!(engine.bridge_samples(), 10);
        let total = snap.series.get(total_id()).expect("total");
        for (s, published_ns) in &sent {
            assert!(
                total.samples.iter().any(|x| x == s),
                "sample at {} missing",
                s.t_ns
            );
            assert_eq!(engine.bridge_origin(s.t_ns), Some(*published_ns));
        }
        assert_eq!(total.samples.last(), sent.last().map(|(s, _)| s));
        // Per-core series stay synthetic.
        let core0 = snap
            .series
            .get(SeriesId {
                device: DeviceIdx(DEV_FIRST_CORE),
                metric: MetricId::CPU_TOTAL,
            })
            .expect("core 0");
        assert!(core0.samples.iter().all(|s| s.value <= 100.0));
        assert_eq!(engine.bridge_origin(1), None);
    }

    #[test]
    fn stop_joins_thread() {
        let engine = BenchEngine::start(BenchFeed::Synthetic, 42, now_boot_ns());
        std::thread::sleep(Duration::from_millis(250));
        engine.stop();
        assert!(lock(&engine.thread).is_none());
        let wake = engine.wake();
        while wake.try_recv().is_ok() {}
        let seq = engine.latest().sequence;
        let n = engine.published_10hz();
        std::thread::sleep(Duration::from_millis(300));
        assert!(wake.try_recv().is_err());
        assert_eq!(engine.latest().sequence, seq);
        assert_eq!(engine.published_10hz(), n);
        // A second stop is a no-op.
        engine.stop();
    }

    #[test]
    fn client_calls_are_recorded() {
        let engine = BenchEngine::start(BenchFeed::Synthetic, 42, now_boot_ns());
        assert_eq!(engine.send(Command::ResetBaselines), Ok(()));
        assert_eq!(engine.commands().len(), 1);
        engine.set_demand(Demand::default());
        assert_eq!(engine.demand(), Some(Demand::default()));
        assert!(engine.action_events().try_recv().is_err());
        engine.stop();
    }
}
