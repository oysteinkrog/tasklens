//! `BenchEngine`: the scripted `EngineClient` the bench scenes run against (ADR-013 fake engine,
//! UI-PRF-053).
//!
//! One publisher thread builds full snapshots from the generators in [`super::synth`] and
//! publishes them like the real bridge (ADR-005): replace the latest snapshot, then send one wake
//! token. In the `bridge` scene the CPU total and kernel series come from a real client. In the
//! `table-2000` scene only the process table changes, once per second. In the `soak` scene the
//! same synthetic samples as `perf-cpu` are published once per second, ten at a time. The
//! `perf-overview` scene adds memory, disk, network and GPU totals, five more 1 Hz series and the
//! disk and network aggregates to the `perf-cpu` data.

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
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::history::HistoryView;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::series::{Sample, SeriesRing, SeriesView};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ConnectionsSnapshot, InventorySnapshot, ProcessSnapshot, SeriesSet,
    ServicesSnapshot, Snapshot, StartupSnapshot, SystemSnapshot, WindowsSnapshot,
};
use tl_model::system::{
    CoreInfo, CoreKind, CpuSet, CpuTopology, CpuTotals, DiskTotals, GpuTotals, LinkState,
    MemoryTotals, Metric, NetTotals,
};
use tl_model::table::ProcessTableView;
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use super::synth::{
    BenchRng, CORES, CpuFrame, CpuSynth, DeviceFrame, DeviceSynth, PrefilledSeries, StatsFrame,
    StatsSynth, TableSynth, prefill, seconds,
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
/// XORed into the seed for perf-overview-procs' process PRNG.
const PROCS_SEED: u64 = 0x7072_6f63;

/// Placeholder memory size for the MEM_USED series, in bytes (64 GiB).
pub(crate) const MEM_TOTAL_BYTES: u64 = 64 << 30;
/// Network series are bytes per second; the synthetic values are Mbps.
const BYTES_PER_S_PER_MBPS: f32 = 125_000.0;

/// Placeholder topology (R-160 OI-2): 16 cores x 2 threads.
const PHYS_CORES: usize = CORES / 2;

/// Device indexes of the placeholder catalog.
pub(crate) const DEV_PACKAGE: u16 = 0;
const DEV_FIRST_CORE: u16 = 1;
pub(crate) const DEV_MEMORY: u16 = DEV_FIRST_CORE + CORES as u16;
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

/// perf-overview: the disk and network aggregates, after the 41 catalog entries.
const DEV_AGG_DISK: u16 = DEV_FIRST_NET + NET_NAMES.len() as u16;
const DEV_AGG_NET: u16 = DEV_AGG_DISK + 1;
/// perf-overview memory totals: 8 GiB swap at 5% used, page cache at 20% of the total.
const SWAP_TOTAL_BYTES: u64 = 8 << 30;
const SWAP_USED_BYTES: u64 = SWAP_TOTAL_BYTES / 20;
const CACHED_BYTES: u64 = MEM_TOTAL_BYTES / 5;
/// perf-overview GPU memory size (16 GiB).
const VRAM_TOTAL_BYTES: u64 = 16 << 30;
/// perf-overview disks: each second a read burst and a write burst each start with this chance,
/// at up to 500 MB/s.
const DISK_BURST_P: f32 = 0.1;
const DISK_BURST_MAX_BPS: f32 = 500_000_000.0;
/// perf-overview disk size (2 TB).
const DISK_CAPACITY_BYTES: u64 = 2_000_000_000_000;
/// perf-overview link speed of the wired interface.
const WIRED_LINK_MBPS: u32 = 1_000;

/// perf-overview track indexes, sorted by `SeriesId` like the tracks above.
const OX_SWAP: usize = 0;
const OX_CACHED: usize = 1;
const OX_GPU_MEM: usize = 2;
/// Disk `d` has its read rate at `OX_FIRST_DISK + 2d` and its write rate right after.
const OX_FIRST_DISK: usize = 3;
const OX_AGG_DISK: usize = OX_FIRST_DISK + 8;
const OX_AGG_NET: usize = OX_AGG_DISK + 2;
const OX_TRACKS: usize = OX_AGG_NET + 2;

/// Where the CPU total comes from.
pub enum BenchFeed {
    /// Every series synthetic (perf-cpu, static).
    Synthetic,
    /// perf-overview: the `Synthetic` series plus memory, disk, network and GPU totals, the
    /// DISK_READ, DISK_WRITE, MEM_CACHED, SWAP_USED and GPU_MEM series, and the disk and
    /// network aggregates with their summed rates.
    Overview,
    /// perf-overview-procs: the `Overview` feed, and once per second the records of a
    /// [`TableSynth`] with their own PRNG as the PROCESSES component (crw.45).
    OverviewProcs,
    /// table-2000: the prefilled series never change; once per second a new [`TableSynth`]
    /// table replaces the PROCESSES and TABLE components, and nothing else is published.
    Table,
    /// soak: the `Synthetic` series, published once per second (ADR-010 "1 Hz updates"). Each
    /// publish carries the ten 100 ms samples of the second, so the 10 Hz series stay whole.
    Soak,
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

        let ticks_per_publish = if matches!(feed, BenchFeed::Soak) {
            TICKS_PER_1HZ
        } else {
            1
        };
        let (mut publisher, real) = match feed {
            BenchFeed::Synthetic | BenchFeed::Soak => (Publisher::new(seed, t0, None, false), None),
            BenchFeed::Overview => (Publisher::build(seed, t0, None, false, true), None),
            BenchFeed::OverviewProcs => {
                let mut publisher = Publisher::build(seed, t0, None, false, true);
                publisher.add_overview_procs(seed, t0);
                (publisher, None)
            }
            BenchFeed::Table => (Publisher::new(seed, t0, None, true), None),
            BenchFeed::Bridge(client) => {
                let wake = client.wake();
                (Publisher::new(seed, t0, Some(client), false), Some(wake))
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
            .spawn(move || run(publisher, &thread_shared, &signal_rx, t0, ticks_per_publish))
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

    /// Publishes made on a 10 Hz deadline. Bridge republishes between deadlines are not counted;
    /// in table-2000 only every 10th deadline publishes.
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

/// The publisher loop: absolute deadlines every `ticks_per_publish * 100 ms` from `t0`, no
/// drift. Each deadline runs that many 100 ms ticks, then publishes once.
fn run(
    mut publisher: Publisher,
    shared: &Shared,
    signals: &mpsc::Receiver<Signal>,
    t0: TimeNs,
    ticks_per_publish: u64,
) {
    let mut tick: u64 = 0;
    loop {
        let deadline = t0 + (tick + ticks_per_publish) * PERIOD_10HZ_NS;
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
        let mut changed = false;
        for _ in 0..ticks_per_publish {
            tick += 1;
            changed |= publisher.tick(tick, shared);
        }
        if changed {
            shared.publish(publisher.snapshot());
            shared.published_10hz.fetch_add(1, Ordering::AcqRel);
        }
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
    /// Some in table-2000 only.
    table_synth: Option<TableSynth>,
    /// perf-overview-procs: the process records and their own PRNG, so the series stay those
    /// of perf-overview.
    overview_procs: Option<(TableSynth, BenchRng)>,
    processes: Arc<ProcessSnapshot>,
    table: Arc<ProcessTableView>,
    /// Some in perf-overview only.
    overview: Option<OverviewSynth>,
}

impl Publisher {
    fn new(seed: u64, t0: TimeNs, bridge: Option<Arc<dyn EngineClient>>, table: bool) -> Self {
        Self::build(seed, t0, bridge, table, false)
    }

    /// `overview` adds the perf-overview extras. They draw from their own PRNG with the same
    /// seed, so the CPU and card series are those of perf-cpu.
    fn build(
        seed: u64,
        t0: TimeNs,
        bridge: Option<Arc<dyn EngineClient>>,
        table: bool,
        overview: bool,
    ) -> Self {
        let mut rng = BenchRng::new(seed);
        let mut cpu = CpuSynth::new(&mut rng);
        let mut devices = DeviceSynth::new();
        let pre = prefill(&mut cpu, &mut devices, &mut rng, t0);
        let overview = overview.then(|| OverviewSynth::new(seed, &pre));
        let stats = StatsSynth::new(seconds(t0));
        let system = Arc::new(SystemSnapshot {
            header: header(0, t0, Validity::WarmingUp),
            cpu_topology: Arc::new(topology()),
            ..SystemSnapshot::default()
        });
        let mut template = template();
        if overview.is_some() {
            template.devices = Arc::new(overview_catalog());
        }
        let mut this = Self {
            processes: Arc::clone(&template.processes),
            table: Arc::clone(&template.table),
            rng,
            cpu,
            devices,
            stats,
            t0,
            tracks: tracks(&pre),
            template,
            system,
            series_version: 1,
            sequence: 1,
            bridge,
            cursor: BridgeCursor::default(),
            table_synth: None,
            overview_procs: None,
            overview,
        };
        if table {
            this.table_synth = Some(TableSynth::new(&mut this.rng));
            this.tick_table(t0, None);
        }
        this
    }

    /// perf-overview-procs: process records from now on, first drawn at `t0`.
    fn add_overview_procs(&mut self, seed: u64, t0: TimeNs) {
        let mut rng = BenchRng::new(seed ^ PROCS_SEED);
        let mut synth = TableSynth::new(&mut rng);
        synth.tick_1hz(&mut rng, t0);
        self.processes = synth.processes();
        self.overview_procs = Some((synth, rng));
    }

    /// The next table at `t_ns`, built for the last `SetTableQuery` generation sent.
    fn tick_table(&mut self, t_ns: TimeNs, shared: Option<&Shared>) {
        let Some(synth) = &mut self.table_synth else {
            return;
        };
        let view = synth.tick_1hz(&mut self.rng, t_ns);
        let generation = shared.and_then(|sh| {
            lock(&sh.commands).iter().rev().find_map(|c| match c {
                Command::SetTableQuery(q) => Some(q.generation),
                _ => None,
            })
        });
        self.table = match generation {
            Some(generation) => Arc::new(ProcessTableView {
                query_generation: generation,
                ..ProcessTableView::clone(&view)
            }),
            None => view,
        };
        self.processes = synth.processes();
    }

    /// Appends tick `k`'s samples at `t0 + k * 100 ms`. Returns true when there is something to
    /// publish: always, except between the 1 Hz table ticks of table-2000.
    fn tick(&mut self, k: u64, shared: &Shared) -> bool {
        let t_ns = self.t0 + k * PERIOD_10HZ_NS;
        if self.table_synth.is_some() {
            if k.is_multiple_of(TICKS_PER_1HZ) {
                self.tick_table(t_ns, Some(shared));
                return true;
            }
            return false;
        }
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
            if let Some((synth, rng)) = &mut self.overview_procs {
                synth.tick_1hz(rng, t_ns);
                self.processes = synth.processes();
            }
        }
        self.series_version += 1;
        true
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
        let mut system = SystemSnapshot {
            header: header(version, t_ns, Validity::Valid),
            cpu: cpu_totals(&s),
            memory: memory_totals(mem_used),
            cpu_topology: Arc::clone(&self.system.cpu_topology),
            boot_time_s: boot_time_s(s.up_time_s),
            ..SystemSnapshot::default()
        };
        if let Some(o) = &mut self.overview {
            o.tick_1hz(t_ns, &d, &mut system);
        }
        self.system = Arc::new(system);
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
        let extra: &mut [Track] = match &mut self.overview {
            Some(o) => &mut o.tracks,
            None => &mut [],
        };
        for t in self.tracks.iter_mut().chain(extra.iter_mut()) {
            t.refresh();
        }
        let mut views: Vec<Arc<SeriesView>> = self
            .tracks
            .iter()
            .chain(extra.iter())
            .map(|t| {
                if let Some(s) = t.view.samples.last() {
                    newest = newest.max(s.t_ns);
                }
                Arc::clone(&t.view)
            })
            .collect();
        if !extra.is_empty() {
            // `SeriesSet::get` searches by `SeriesId`.
            views.sort_unstable_by_key(|v| v.id);
        }
        let views: Arc<[Arc<SeriesView>]> = views.into();
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
            processes: Arc::clone(&self.processes),
            table: Arc::clone(&self.table),
            ..self.template.clone()
        }
    }
}

/// The perf-overview extras: the totals the Overview boxes read and their 1 Hz series.
struct OverviewSynth {
    rng: BenchRng,
    /// [`OX_TRACKS`] tracks, sorted by `SeriesId`.
    tracks: Vec<Track>,
    /// Bytes read and written since boot, per disk.
    disk_totals: [(u64, u64); 4],
    /// Bytes received (and sent) since boot, per interface.
    net_totals: [u64; 2],
}

/// One second of perf-overview extras.
struct OverviewFrame {
    /// Read and write bytes per second of Disk 0 to Disk 3.
    disk_bps: [(f32, f32); 4],
    vram_used: u64,
    gpu_temp_c: f32,
    gpu_power_w: f32,
}

impl OverviewSynth {
    /// Prefills the extra series at the 1 Hz times of `pre`.
    fn new(seed: u64, pre: &PrefilledSeries) -> Self {
        let mut this = Self {
            rng: BenchRng::new(seed),
            tracks: overview_tracks(),
            disk_totals: [(0, 0); 4],
            net_totals: [0; 2],
        };
        // pre.devices is Memory, GPU 0, Disk 0 to Disk 3, enp38s0, tailscale0.
        for (i, s) in pre.devices[0].iter().enumerate() {
            let f = this.sample();
            let mbps = [pre.devices[6][i].value, pre.devices[7][i].value];
            this.push(s.t_ns, &f, mbps);
        }
        this
    }

    fn sample(&mut self) -> OverviewFrame {
        let rng = &mut self.rng;
        let mut disk_bps = [(0.0, 0.0); 4];
        for d in &mut disk_bps {
            for v in [&mut d.0, &mut d.1] {
                if rng.next_f32_unit() < DISK_BURST_P {
                    *v = rng.uniform(0.0, DISK_BURST_MAX_BPS);
                }
            }
        }
        // VRAM 30% +- 5, temperature 52 +- 2, power 120 +- 20: a fresh draw each second.
        let vram_pct = rng.uniform(25.0, 35.0);
        OverviewFrame {
            disk_bps,
            vram_used: (f64::from(vram_pct) / 100.0 * VRAM_TOTAL_BYTES as f64) as u64,
            gpu_temp_c: rng.uniform(50.0, 54.0),
            gpu_power_w: rng.uniform(100.0, 140.0),
        }
    }

    /// Appends one second at `t_ns`; `net_mbps` are the two interfaces' rates.
    fn push(&mut self, t_ns: TimeNs, f: &OverviewFrame, net_mbps: [f32; 2]) {
        let mut push = |i: usize, v: f32| self.tracks[i].push(Sample::valid(t_ns, v));
        push(OX_SWAP, SWAP_USED_BYTES as f32);
        push(OX_CACHED, CACHED_BYTES as f32);
        push(OX_GPU_MEM, f.vram_used as f32);
        let (mut read, mut write) = (0.0, 0.0);
        for (d, &(r, w)) in f.disk_bps.iter().enumerate() {
            push(OX_FIRST_DISK + 2 * d, r);
            push(OX_FIRST_DISK + 2 * d + 1, w);
            read += r;
            write += w;
        }
        push(OX_AGG_DISK, read);
        push(OX_AGG_DISK + 1, write);
        // Each interface sends what it receives, as in the card series.
        let bps = net_mbps.iter().sum::<f32>() * BYTES_PER_S_PER_MBPS;
        push(OX_AGG_NET, bps);
        push(OX_AGG_NET + 1, bps);
    }

    /// One second: the extra series and the memory, disk, network and GPU totals of `system`.
    fn tick_1hz(&mut self, t_ns: TimeNs, d: &DeviceFrame, system: &mut SystemSnapshot) {
        let f = self.sample();
        self.push(t_ns, &f, d.net_mbps);

        let mem = &mut system.memory;
        mem.cached = CACHED_BYTES;
        mem.swap_total = SWAP_TOTAL_BYTES;
        mem.swap_free = SWAP_TOTAL_BYTES - SWAP_USED_BYTES;

        let none = || Metric::with(Validity::Unsupported);
        let mut disks = Vec::with_capacity(4);
        for (i, (&(r, w), totals)) in f.disk_bps.iter().zip(&mut self.disk_totals).enumerate() {
            totals.0 += r as u64;
            totals.1 += w as u64;
            let device = DeviceIdx(DEV_FIRST_DISK + i as u16);
            let row = DiskTotals {
                device,
                active_pct: Metric::valid(d.disk_pct[i]),
                read_bps: Metric::valid(f64::from(r)),
                write_bps: Metric::valid(f64::from(w)),
                response_ms: none(),
                read_total: totals.0,
                written_total: totals.1,
                capacity: DISK_CAPACITY_BYTES,
                temp_c: none(),
            };
            disks.push((device, row));
        }
        system.disks = disks.into();

        let mut nets = Vec::with_capacity(NET_NAMES.len());
        for (i, (&mbps, total)) in d.net_mbps.iter().zip(&mut self.net_totals).enumerate() {
            let bps = f64::from(mbps * BYTES_PER_S_PER_MBPS);
            *total += bps as u64;
            let device = DeviceIdx(DEV_FIRST_NET + i as u16);
            let row = NetTotals {
                device,
                rx_bps: Metric::valid(bps),
                tx_bps: Metric::valid(bps),
                rx_total: *total,
                tx_total: *total,
                // Only the wired interface reports a speed.
                link_speed_mbps: if i == 0 {
                    Metric::valid(WIRED_LINK_MBPS)
                } else {
                    Metric::with(Validity::Unsupported)
                },
                state: LinkState::Up,
            };
            nets.push((device, row));
        }
        system.nets = nets.into();

        let gpu = DeviceIdx(DEV_GPU);
        let row = GpuTotals {
            device: gpu,
            util_pct: Metric::valid(d.gpu_pct),
            mem_util_pct: none(),
            mem_used: Metric::valid(f.vram_used),
            mem_total: Metric::valid(VRAM_TOTAL_BYTES),
            shared_used: Metric::with(Validity::Unsupported),
            shared_total: Metric::with(Validity::Unsupported),
            enc_pct: none(),
            dec_pct: none(),
            mem_reserved: Metric::with(Validity::Unsupported),
            enc_sessions: Metric::with(Validity::Unsupported),
            enc_dec_shared: false,
            temp_c: Metric::valid(f.gpu_temp_c),
            temp_edge_c: Metric::with(Validity::Unsupported),
            temp_junction_c: Metric::with(Validity::Unsupported),
            temp_mem_c: Metric::with(Validity::Unsupported),
            power_w: Metric::valid(f.gpu_power_w),
            power_limit_w: none(),
            clock_gfx_mhz: Metric::with(Validity::Unsupported),
            clock_mem_mhz: Metric::with(Validity::Unsupported),
            fan_pct: none(),
            pcie_rx_bps: none(),
            pcie_tx_bps: none(),
        };
        system.gpus = Arc::from([(gpu, row)]);
    }
}

/// The perf-overview extra series, sorted by `SeriesId`, with no samples yet.
fn overview_tracks() -> Vec<Track> {
    let slow = |device: u16, metric: MetricId| {
        let id = SeriesId {
            device: DeviceIdx(device),
            metric,
        };
        Track::new(id, PERIOD_1HZ_NS, CAP_1HZ, &[])
    };
    let mut out = Vec::with_capacity(OX_TRACKS);
    out.push(slow(DEV_MEMORY, MetricId::SWAP_USED));
    out.push(slow(DEV_MEMORY, MetricId::MEM_CACHED));
    out.push(slow(DEV_GPU, MetricId::GPU_MEM));
    for d in 0..4 {
        out.push(slow(DEV_FIRST_DISK + d, MetricId::DISK_READ));
        out.push(slow(DEV_FIRST_DISK + d, MetricId::DISK_WRITE));
    }
    out.push(slow(DEV_AGG_DISK, MetricId::DISK_READ));
    out.push(slow(DEV_AGG_DISK, MetricId::DISK_WRITE));
    out.push(slow(DEV_AGG_NET, MetricId::NET_RX));
    out.push(slow(DEV_AGG_NET, MetricId::NET_TX));
    debug_assert_eq!(out.len(), OX_TRACKS);
    debug_assert!(out.windows(2).all(|w| w[0].view.id < w[1].view.id));
    out
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

/// perf-overview catalog: the 41 entries of [`catalog`], then the disk and network aggregates.
fn overview_catalog() -> DeviceCatalog {
    let base = catalog();
    let mut entries = base.entries.to_vec();
    for (idx, kind, name, key) in [
        (
            DEV_AGG_DISK,
            AggregateKind::Disks,
            "Disks",
            "aggregate:disk",
        ),
        (DEV_AGG_NET, AggregateKind::Nets, "Network", "aggregate:net"),
    ] {
        debug_assert_eq!(usize::from(idx), entries.len());
        entries.push(DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Aggregate(kind),
            generation: 0,
            display_name: Arc::from(name),
            present: true,
            parent: None,
            persistent_key: Arc::from(key),
            net_kind: None,
        });
    }
    DeviceCatalog {
        version: base.version,
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
    fn soak_publishes_once_per_second_with_ten_samples() {
        let engine = BenchEngine::start(BenchFeed::Soak, 42, now_boot_ns());
        let before = engine.latest();
        std::thread::sleep(Duration::from_millis(2_300));
        let n = engine.published_10hz();
        let after = engine.latest();
        engine.stop();
        assert!((2..=3).contains(&n), "published {n}");
        let id = SeriesId {
            device: DeviceIdx(DEV_PACKAGE),
            metric: MetricId::CPU_TOTAL,
        };
        let newest = |snap: &Snapshot| {
            snap.series
                .get(id)
                .and_then(|v| v.samples.last().map(|s| s.t_ns))
        };
        let moved = newest(&after).unwrap() - newest(&before).unwrap();
        assert_eq!(
            moved,
            n * 10 * PERIOD_10HZ_NS,
            "ten 100 ms samples per publish"
        );
    }

    #[test]
    fn unchanged_series_reuse_arc() {
        let shared = quiet_shared();
        let t0 = 3_600 * PERIOD_1HZ_NS;
        let mut p = Publisher::new(42, t0, None, false);
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
    fn overview_feed_fills_totals_and_aggregates() {
        let shared = quiet_shared();
        let t0 = 3_600 * PERIOD_1HZ_NS;
        let mut p = Publisher::build(42, t0, None, false, true);
        let first = p.snapshot();
        assert_eq!(first.devices.entries.len(), 43);
        assert_eq!(catalog().entries.len(), 41, "perf-cpu keeps 41 entries");
        assert_eq!(first.series.series.len(), TRACKS + OX_TRACKS);
        assert!(first.series.series.windows(2).all(|w| w[0].id < w[1].id));
        for k in 1..=10 {
            p.tick(k, &shared);
        }
        let snap = p.snapshot();
        let sys = &snap.system;
        assert_eq!((sys.disks.len(), sys.nets.len(), sys.gpus.len()), (4, 2, 1));
        assert_eq!(sys.memory.swap_total, SWAP_TOTAL_BYTES);
        assert_eq!(sys.memory.cached, CACHED_BYTES);
        let last = |device: u16, metric| {
            snap.series
                .get(SeriesId {
                    device: DeviceIdx(device),
                    metric,
                })
                .and_then(|v| v.samples.last().map(|s| s.value))
                .expect("series")
        };
        let read: f32 = (0..4)
            .map(|d| last(DEV_FIRST_DISK + d, MetricId::DISK_READ))
            .sum();
        assert_eq!(last(DEV_AGG_DISK, MetricId::DISK_READ), read);
        let rx: f64 = sys.nets.iter().map(|(_, n)| n.rx_bps.value).sum();
        assert!((f64::from(last(DEV_AGG_NET, MetricId::NET_RX)) - rx).abs() < 1.0);
        let vram = sys.gpus[0].1.mem_used.value as f64 / VRAM_TOTAL_BYTES as f64;
        assert!((0.25..0.35).contains(&vram), "{vram}");

        // The CPU series are those of perf-cpu.
        let mut plain = Publisher::new(42, t0, None, false);
        for k in 1..=10 {
            plain.tick(k, &shared);
        }
        let plain = plain.snapshot();
        assert_eq!(
            plain.series.get(total_id()).map(|v| v.samples.clone()),
            snap.series.get(total_id()).map(|v| v.samples.clone())
        );
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
