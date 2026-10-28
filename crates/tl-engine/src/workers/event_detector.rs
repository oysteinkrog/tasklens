//! The event detector of the `MetricHistory` worker (ADR-017, UI-PRF-057, D-030).
//!
//! [`EventDetector::observe`] runs once per second on a [`Reading`] of the live values. Each rule
//! is tracked per device and counts only fresh seconds: a second whose input is missing, or whose
//! source is stale (ADR-008), is neither over nor under. An event starts once its rule has been
//! over for its window in fresh seconds, and ends after [`QUIET_S`] fresh seconds under. More
//! than [`GAP_S`] seconds without a fresh one reset both counts and end a running event at its
//! last fresh second over. The stored start is the first second of the window, the duration runs
//! to the first of the seconds under, the peak is the highest value while over, and the process
//! is the one most often on top of the rule's resource while a process sweep ran. Ended events
//! come back from `observe`; the worker queues them with `MetricHistory::push_event`.
//!
//! Suspend spans come from the Logind `PrepareForSleep` signal ([`EventDetector::sleep`]) and,
//! when the signal was missed, from the ADR-008 clock check ([`EventDetector::check_clock`]).
//! Boots need nothing here: `MetricHistory::open` queues a `Boot` event when the newest segment's
//! `boot_id` differs. OOM kills and app crashes come from the journal ([`JournalEvents`]), which
//! the worker fetches at start and then every 60 s while a timeline box is visible, on the
//! blocking pool. Under `JournalAccess::OwnOnly` both kinds are left out.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use smallvec::SmallVec;
use tl_collect::counters::{Clock, SUSPEND_GAP_NS};
use tl_collect::parse::hwmon::CPU_TEMP_CHIPS;
use tl_history::metrics::{EventKind, HistEvent};
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::health::SourceId;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::journal::JournalAccess;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::{ProcessSnapshot, Snapshot};
use tl_model::system::Metric;
use tl_model::time::TimeNs;

use super::journal::{CollectError, access_for, collect, message_text, process_ids};

/// Memory PSI `some` avg10 at or above this percent is memory pressure.
pub const PRESSURE_PSI_PCT: f32 = 10.;
/// How long memory pressure must hold, s.
pub const PRESSURE_FOR_S: u64 = 30;
/// Swap-in at or above this many pages per second is a swap storm.
pub const SWAP_STORM_PPS: f32 = 2_000.;
/// How long a swap storm must hold, s.
pub const SWAP_STORM_FOR_S: u64 = 30;
/// How long a temperature at or above `hot_c`, or throttling, must hold, s.
pub const HEAT_FOR_S: u64 = 30;
/// CPU total or GPU utilization at or above this percent is full load.
pub const FULL_LOAD_PCT: f32 = 95.;
/// How long full load must hold, s.
pub const FULL_LOAD_FOR_S: u64 = 300;
/// An event ends after this many fresh seconds under the threshold.
pub const QUIET_S: u64 = 60;
/// More than this many seconds in a row without a fresh value of a rule's input is a gap
/// (suspend, a sampling stop, a stale source). The gap ends the rule's running event at its last
/// fresh second over, and drops a window that had not passed yet.
pub const GAP_S: u64 = 3;
/// A source's values count while its last update is at most max(3 × period, 1.5 s) old, the
/// ADR-008 stale threshold.
pub const STALE_PERIODS: u64 = 3;
/// The floor of the stale threshold, ns.
pub const STALE_MIN_NS: u64 = 1_500_000_000;
/// The device key of the memory rules (ADR-017).
pub const MEMORY_KEY: &str = "memory";
/// The device key of the CPU package (ADR-017).
pub const CPU_KEY: &str = "cpu";

/// The journal is fetched again this long after the last fetch while a timeline is visible.
pub const JOURNAL_PERIOD_NS: u64 = 60 * NS;
/// Each journal fetch is killed after this long (ADR-016 Service logs row).
pub const JOURNAL_TIMEOUT: Duration = Duration::from_secs(5);
/// The kernel message of an OOM kill.
pub const OOM_TEXT: &str = "Out of memory: Killed process";
/// systemd-coredump's `MESSAGE_ID`.
pub const COREDUMP_MESSAGE_ID: &str = "fc2e22bc6ee647b6b90729ab34a250b1";

const NS: u64 = 1_000_000_000;
/// A process sweep older than this is not running: the slowest refresh is 10 s.
const SWEEP_FRESH_NS: u64 = 12 * NS;
/// The labels the engine prefers on the CPU temperature chip (tl-collect `cpu_temp_input`).
const CPU_TEMP_LABELS: [&str; 2] = ["Tctl", "Package id 0"];

/// The process resource a rule ranks by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Gpu,
    /// Anonymous resident memory.
    Memory,
    Swap,
}

/// The top process per resource from a fresh process sweep. All `None` without one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopProcesses {
    pub cpu: Option<Arc<str>>,
    pub gpu: Option<Arc<str>>,
    pub memory: Option<Arc<str>>,
    pub swap: Option<Arc<str>>,
}

impl TopProcesses {
    /// The tops of `sweep` when it is valid, not empty and at most 12 s older than `now`.
    pub fn from_sweep(sweep: &ProcessSnapshot, now: TimeNs) -> Self {
        let fresh = sweep.header.validity.is_valid()
            && !sweep.records.is_empty()
            && now.saturating_sub(sweep.header.observed_end) <= SWEEP_FRESH_NS;
        if !fresh {
            return Self::default();
        }
        let name = |r: Resource| top_process(&sweep.records, r).map(|p| Arc::clone(&p.meta.name));
        Self {
            cpu: name(Resource::Cpu),
            gpu: name(Resource::Gpu),
            memory: name(Resource::Memory),
            swap: name(Resource::Swap),
        }
    }

    fn get(&self, r: Resource) -> Option<&Arc<str>> {
        match r {
            Resource::Cpu => self.cpu.as_ref(),
            Resource::Gpu => self.gpu.as_ref(),
            Resource::Memory => self.memory.as_ref(),
            Resource::Swap => self.swap.as_ref(),
        }
    }
}

/// The live process that uses most of `resource`, `None` when none uses any. The same ranking as
/// the status bar's reason (UI-PRF-054).
pub fn top_process(records: &[ProcessRecord], resource: Resource) -> Option<&ProcessRecord> {
    let by: fn(&ProcessRecord) -> f64 = match resource {
        Resource::Cpu => |r| f64::from(r.cpu_pct),
        Resource::Gpu => |r| f64::from(r.gpu_util_pct),
        Resource::Memory => |r| r.rss_anon_bytes as f64,
        Resource::Swap => |r| r.swap_bytes as f64,
    };
    records
        .iter()
        .filter(|r| !r.flags.contains(ProcFlags::EXITED))
        .map(|r| (by(r), r))
        .filter(|(v, _)| v.is_finite() && *v > 0.)
        .fold(None::<(f64, &ProcessRecord)>, |best, cur| match best {
            Some(b) if b.0 >= cur.0 => Some(b),
            _ => Some(cur),
        })
        .map(|(_, r)| r)
}

/// One GPU's values.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuReading {
    /// The GPU's `persistent_key`.
    pub key: Arc<str>,
    pub util_pct: Option<f32>,
    pub temp_c: Option<f32>,
    /// The device catalog's `hot_c` (ADR-016).
    pub hot_c: Option<f32>,
    pub throttling: Option<bool>,
}

/// What the rules read once per second. `None` is a value that is not valid now, or whose
/// source is stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// CLOCK_BOOTTIME, ns.
    pub t_ns: TimeNs,
    pub cpu_pct: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub cpu_hot_c: Option<f32>,
    pub cpu_throttling: Option<bool>,
    pub gpus: SmallVec<[GpuReading; 2]>,
    pub psi_some_pct: Option<f32>,
    pub swap_in_pps: Option<f32>,
    pub top: TopProcesses,
}

fn valid(m: Metric<f32>) -> Option<f32> {
    (m.validity.is_valid() && m.value.is_finite()).then_some(m.value)
}

fn flag(m: Metric<bool>) -> Option<bool> {
    m.validity.is_valid().then_some(m.value)
}

/// True when `id` sent data at most the ADR-008 stale threshold before `snap` was published.
fn fresh(snap: &Snapshot, id: SourceId, period_ns: u64) -> bool {
    let threshold = period_ns.saturating_mul(STALE_PERIODS).max(STALE_MIN_NS);
    snap.health.sources[id.index()]
        .last_ok_ns
        .is_some_and(|ok| snap.published_ns.saturating_sub(ok) <= threshold)
}

/// The sampling period of the first live series with `metric`, on `device` when given. 1 s
/// without one.
fn period_ns(snap: &Snapshot, metric: MetricId, device: Option<DeviceIdx>) -> u64 {
    let view = match device {
        Some(device) => snap.series.get(SeriesId { device, metric }),
        None => snap.series.series.iter().find(|v| v.id.metric == metric),
    };
    view.map_or(NS, |v| v.period_ns).max(1)
}

/// The GPU worker that sends a GPU's values: NVML devices carry their UUID, AMD and Intel cards
/// use `[0; 16]` (R-166).
fn gpu_source(identity: &DeviceIdentity) -> SourceId {
    match identity {
        DeviceIdentity::Gpu { uuid, .. } if *uuid != [0; 16] => SourceId::GpuNvidia,
        _ => SourceId::GpuDrm,
    }
}

/// The `hot_c` of the sensor the engine reads the CPU temperature from: the first chip in
/// `CPU_TEMP_CHIPS` order, its `Tctl` or `Package id 0` input, else its first input.
pub fn cpu_hot_c(devices: &DeviceCatalog) -> Option<f32> {
    let sensors = || {
        devices.entries.iter().filter_map(|e| match &e.identity {
            DeviceIdentity::Sensor { chip, label } if e.present => Some((chip, label, e.hot_c)),
            _ => None,
        })
    };
    let chip = CPU_TEMP_CHIPS
        .iter()
        .find(|wanted| sensors().any(|(chip, _, _)| &**chip == **wanted))?;
    let on_chip = || sensors().filter(|(c, _, _)| &***c == *chip);
    on_chip()
        .find(|(_, label, _)| CPU_TEMP_LABELS.contains(&&***label))
        .or_else(|| on_chip().next())
        .and_then(|(_, _, hot)| hot)
}

impl Reading {
    /// The values of `snap` at its publish time. A value whose source is stale then (its
    /// `last_ok_ns` in `snap.health`) is left out, so a stalled sampler's last value never
    /// starts or extends an event. The CPU worker sends the CPU temperature and throttling.
    pub fn from_snapshot(snap: &Snapshot) -> Self {
        let sys = &snap.system;
        let gpus = sys
            .gpus
            .iter()
            .filter_map(|(idx, g)| {
                let entry = snap.devices.get(*idx)?;
                let period = period_ns(snap, MetricId::GPU_UTIL, Some(*idx));
                let ok = fresh(snap, gpu_source(&entry.identity), period);
                Some(GpuReading {
                    key: Arc::clone(&entry.persistent_key),
                    util_pct: valid(g.util_pct).filter(|_| ok),
                    temp_c: valid(g.temp_c).filter(|_| ok),
                    hot_c: entry.hot_c,
                    throttling: flag(g.throttling).filter(|_| ok),
                })
            })
            .collect();
        let cpu = fresh(
            snap,
            SourceId::Cpu,
            period_ns(snap, MetricId::CPU_TOTAL, None),
        );
        let memory = fresh(
            snap,
            SourceId::Memory,
            period_ns(snap, MetricId::PSI_MEM_SOME, None),
        );
        Self {
            t_ns: snap.published_ns,
            cpu_pct: valid(sys.cpu.util_pct).filter(|_| cpu),
            cpu_temp_c: valid(sys.cpu.temp_c).filter(|_| cpu),
            cpu_hot_c: cpu_hot_c(&snap.devices),
            cpu_throttling: flag(sys.cpu.throttling).filter(|_| cpu),
            gpus,
            psi_some_pct: valid(sys.memory.psi_some_pct).filter(|_| memory),
            swap_in_pps: valid(sys.memory.swap_in_pps).filter(|_| memory),
            top: TopProcesses::from_sweep(&snap.processes, snap.published_ns),
        }
    }

    /// Every rule that is over its threshold now, with its value (NaN for a throttling flag on
    /// a device without a temperature).
    fn over(&self, out: &mut SmallVec<[Over; 8]>) {
        let mut push = |kind, key: &Arc<str>, value, resource| {
            out.push(Over {
                kind,
                key: Arc::clone(key),
                value,
                resource,
            });
        };
        let memory: Arc<str> = Arc::from(MEMORY_KEY);
        let cpu: Arc<str> = Arc::from(CPU_KEY);
        if let Some(v) = self.psi_some_pct.filter(|v| *v >= PRESSURE_PSI_PCT) {
            push(EventKind::MemoryPressure, &memory, v, Resource::Memory);
        }
        if let Some(v) = self.swap_in_pps.filter(|v| *v >= SWAP_STORM_PPS) {
            push(EventKind::SwapStorm, &memory, v, Resource::Swap);
        }
        if hot(self.cpu_temp_c, self.cpu_hot_c) || self.cpu_throttling == Some(true) {
            let v = self.cpu_temp_c.unwrap_or(f32::NAN);
            push(EventKind::Heat, &cpu, v, Resource::Cpu);
        }
        if let Some(v) = self.cpu_pct.filter(|v| *v >= FULL_LOAD_PCT) {
            push(EventKind::FullLoad, &cpu, v, Resource::Cpu);
        }
        for g in &self.gpus {
            if hot(g.temp_c, g.hot_c) || g.throttling == Some(true) {
                let v = g.temp_c.unwrap_or(f32::NAN);
                push(EventKind::Heat, &g.key, v, Resource::Gpu);
            }
            if let Some(v) = g.util_pct.filter(|v| *v >= FULL_LOAD_PCT) {
                push(EventKind::FullLoad, &g.key, v, Resource::Gpu);
            }
        }
    }

    /// True when the input of `kind` on `key` is in this reading: valid, and its source not
    /// stale.
    fn has_input(&self, kind: EventKind, key: &str) -> bool {
        let gpu = || self.gpus.iter().find(|g| &*g.key == key);
        match (kind, key) {
            (EventKind::MemoryPressure, _) => self.psi_some_pct.is_some(),
            (EventKind::SwapStorm, _) => self.swap_in_pps.is_some(),
            (EventKind::Heat, CPU_KEY) => {
                heat_input(self.cpu_temp_c, self.cpu_hot_c, self.cpu_throttling)
            }
            (EventKind::FullLoad, CPU_KEY) => self.cpu_pct.is_some(),
            (EventKind::Heat, _) => {
                gpu().is_some_and(|g| heat_input(g.temp_c, g.hot_c, g.throttling))
            }
            (EventKind::FullLoad, _) => gpu().is_some_and(|g| g.util_pct.is_some()),
            _ => false,
        }
    }
}

/// The input of the heat rule: the temperature on a device with `hot_c`, the throttling flag
/// on a device without one. A temperature without a threshold never says heat is under.
fn heat_input(temp_c: Option<f32>, hot_c: Option<f32>, throttling: Option<bool>) -> bool {
    match hot_c {
        Some(_) => temp_c.is_some(),
        None => throttling.is_some(),
    }
}

/// A temperature at or above its threshold. A device without a threshold is never hot from
/// its temperature.
fn hot(temp_c: Option<f32>, hot_c: Option<f32>) -> bool {
    matches!((temp_c, hot_c), (Some(t), Some(h)) if t >= h)
}

/// One rule over its threshold in one reading.
struct Over {
    kind: EventKind,
    key: Arc<str>,
    value: f32,
    resource: Resource,
}

/// How many fresh seconds over a rule needs before its event starts.
fn window_s(kind: EventKind) -> u64 {
    match kind {
        EventKind::MemoryPressure => PRESSURE_FOR_S,
        EventKind::SwapStorm => SWAP_STORM_FOR_S,
        EventKind::Heat => HEAT_FOR_S,
        EventKind::FullLoad => FULL_LOAD_FOR_S,
        _ => 0,
    }
}

/// One rule on one device, from the first fresh second over of its window.
struct Track {
    kind: EventKind,
    key: Arc<str>,
    resource: Resource,
    since: TimeNs,
    /// The last fresh second over.
    last_over: TimeNs,
    /// The CLOCK_BOOTTIME second of the last fresh reading, over or under.
    last_fresh_s: u64,
    /// Fresh seconds over since `since`. The window has passed once it reaches [`window_s`].
    over_s: u64,
    /// The window has passed.
    started: bool,
    /// The first fresh second under after `last_over`.
    under_since: Option<TimeNs>,
    /// Fresh seconds under since `under_since`.
    under_s: u64,
    /// The highest value while over, NaN while none was valid.
    peak: f32,
    /// Each top process seen while over and how many readings it was on top, first seen first.
    tops: SmallVec<[(Arc<str>, u32); 4]>,
}

impl Track {
    fn new(o: &Over, now: TimeNs) -> Self {
        Track {
            kind: o.kind,
            key: Arc::clone(&o.key),
            resource: o.resource,
            since: now,
            last_over: now,
            last_fresh_s: now / NS,
            over_s: 0,
            started: false,
            under_since: None,
            under_s: 0,
            peak: f32::NAN,
            tops: SmallVec::new(),
        }
    }

    /// A fresh second over: clears the quiet count and moves the window on.
    fn add_over(&mut self, o: &Over, now: TimeNs, top: &TopProcesses) {
        self.last_over = now;
        self.last_fresh_s = now / NS;
        self.under_since = None;
        self.under_s = 0;
        self.over_s += 1;
        self.started |= self.over_s >= window_s(self.kind);
        self.peak = self.peak.max(o.value);
        if let Some(name) = top.get(self.resource) {
            match self.tops.iter_mut().find(|(n, _)| n == name) {
                Some((_, count)) => *count += 1,
                None => self.tops.push((Arc::clone(name), 1)),
            }
        }
    }

    fn event(&self, end: TimeNs) -> HistEvent {
        let mut best: Option<&(Arc<str>, u32)> = None;
        for t in &self.tops {
            if best.is_none_or(|b| t.1 > b.1) {
                best = Some(t);
            }
        }
        HistEvent {
            kind: self.kind,
            start_t_ns: self.since,
            duration_ms: ms(end.saturating_sub(self.since)),
            peak: self.peak,
            device_key: Some(Arc::clone(&self.key)),
            process: best.map(|(name, _)| Arc::clone(name)),
        }
    }
}

fn ms(ns: u64) -> u32 {
    u32::try_from(ns / 1_000_000).unwrap_or(u32::MAX)
}

/// A suspend span. `peak` is 0, as for the store's `Boot` event.
fn suspend_event(start: TimeNs, end: TimeNs) -> HistEvent {
    HistEvent {
        kind: EventKind::Suspend,
        start_t_ns: start,
        duration_ms: ms(end.saturating_sub(start)),
        peak: 0.,
        device_key: None,
        process: None,
    }
}

/// The rule and suspend part of the detector. One per engine session.
#[derive(Default)]
pub struct EventDetector {
    tracks: Vec<Track>,
    last_ns: Option<TimeNs>,
    /// When Logind said the machine is going to sleep.
    sleep_since: Option<TimeNs>,
    /// When Logind said the machine woke, for the clock check.
    woke_at: Option<TimeNs>,
    clock: Option<Clock>,
}

impl EventDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// One evaluation. Returns the events that ended, oldest start first.
    pub fn observe(&mut self, r: &Reading) -> Vec<HistEvent> {
        let now = r.t_ns;
        let sec = now / NS;
        let mut ended = Vec::new();
        if self.last_ns.is_some_and(|last| now < last) {
            self.end_all(&mut ended, None);
        }
        self.last_ns = Some(now);

        let mut over: SmallVec<[Over; 8]> = SmallVec::new();
        r.over(&mut over);
        self.tracks.retain_mut(|t| {
            let o = over.iter().find(|o| o.kind == t.kind && o.key == t.key);
            let fresh = o.is_some() || r.has_input(t.kind, &t.key);
            // The seconds since the last fresh one without a fresh value, this one included when
            // it has none. A missing second is neither over nor under.
            let missing = sec
                .saturating_sub(t.last_fresh_s)
                .saturating_sub(u64::from(fresh));
            if missing > GAP_S {
                if t.started {
                    ended.push(t.event(t.last_over));
                }
                return false;
            }
            if !fresh {
                return true;
            }
            if let Some(o) = o {
                t.add_over(o, now, &r.top);
                return true;
            }
            t.last_fresh_s = sec;
            if !t.started {
                return false;
            }
            let under = *t.under_since.get_or_insert(now);
            t.under_s += 1;
            if t.under_s >= QUIET_S {
                ended.push(t.event(under));
                return false;
            }
            true
        });
        for o in &over {
            if !self
                .tracks
                .iter()
                .any(|t| t.kind == o.kind && t.key == o.key)
            {
                let mut t = Track::new(o, now);
                t.add_over(o, now, &r.top);
                self.tracks.push(t);
            }
        }
        ended.sort_by_key(|e| e.start_t_ns);
        ended
    }

    /// Ends every started event and drops the rest. With `now` (shutdown) a started event ends
    /// at its first second under, else at `now`; without it (the clock went back) at its last
    /// second over.
    fn end_all(&mut self, ended: &mut Vec<HistEvent>, now: Option<TimeNs>) {
        for t in self.tracks.drain(..) {
            if t.started {
                let end = match now {
                    Some(now) => t.under_since.unwrap_or(now),
                    None => t.last_over,
                };
                ended.push(t.event(end));
            }
        }
        ended.sort_by_key(|e| e.start_t_ns);
    }

    /// At shutdown: every started event with its duration so far.
    pub fn finish(&mut self, now: TimeNs) -> Vec<HistEvent> {
        let mut ended = Vec::new();
        self.end_all(&mut ended, Some(now));
        ended
    }

    /// The Logind `PrepareForSleep` signal. Waking returns the sleep span; a wake without a
    /// sleep before it returns nothing and leaves the span to the clock check.
    pub fn sleep(&mut self, entering: bool, t_ns: TimeNs) -> Option<HistEvent> {
        if entering {
            self.sleep_since = Some(t_ns);
            return None;
        }
        let start = self.sleep_since.take()?;
        self.woke_at = Some(t_ns);
        Some(suspend_event(start, t_ns))
    }

    /// The ADR-008 clock check, once per tick with both clocks read just before. When the
    /// machine slept since the last call and Logind did not report the wake, returns the sleep
    /// span: from the sleep signal when one came, else the slept time before `cur`.
    pub fn check_clock(&mut self, cur: Clock) -> Option<HistEvent> {
        let prev = self.clock.replace(cur)?;
        let d_boot = cur.boot_ns.saturating_sub(prev.boot_ns);
        let slept = d_boot.saturating_sub(cur.mono_ns.saturating_sub(prev.mono_ns));
        if slept <= SUSPEND_GAP_NS {
            return None;
        }
        if self.woke_at.is_some_and(|w| w > prev.boot_ns) {
            return None;
        }
        let start = self
            .sleep_since
            .take()
            .unwrap_or(cur.boot_ns.saturating_sub(slept));
        Some(suspend_event(start, cur.boot_ns))
    }
}

/// What the journal fetch reads from its surroundings. Tests replace each part.
#[derive(Clone, Debug)]
pub struct JournalEnv {
    /// `PATH` for the child. `None` inherits the engine's.
    pub path: Option<OsString>,
    pub group_file: PathBuf,
    /// Effective uid and the process's gids. `None` reads the process's own.
    pub ids: Option<(u32, Vec<u32>)>,
}

impl JournalEnv {
    pub fn system() -> Self {
        Self {
            path: None,
            group_file: PathBuf::from("/etc/group"),
            ids: None,
        }
    }
}

/// The answer to one [`JournalEvents::fetch`].
#[derive(Clone, Debug, PartialEq)]
pub struct JournalFetch {
    /// `OwnOnly` leaves both kinds out; the timeline's tooltip says why.
    pub access: JournalAccess,
    /// OOM kills and crashes, oldest first.
    pub events: Vec<HistEvent>,
}

/// One journal source: its arguments, the cursor to continue from, and the newest entry
/// already stored.
#[derive(Debug, Default)]
struct JournalSource {
    cursor: Option<Arc<str>>,
    /// CLOCK_REALTIME µs. Entries at or before it are dropped: the worker seeds it from the store
    /// so a restart in the same boot does not store them twice.
    skip_until_us: Option<u64>,
}

/// The OOM kill and app crash fetches.
pub struct JournalEvents {
    env: JournalEnv,
    access: Option<JournalAccess>,
    kernel: JournalSource,
    crashes: JournalSource,
    last_fetch: Option<TimeNs>,
}

impl JournalEvents {
    pub fn new(env: JournalEnv) -> Self {
        Self {
            env,
            access: None,
            kernel: JournalSource::default(),
            crashes: JournalSource::default(),
            last_fetch: None,
        }
    }

    /// Drops entries of `kind` (`OomKill` or `Crash`) at or before `realtime_ns`: the newest
    /// such event of this boot already in the store.
    pub fn skip_until(&mut self, kind: EventKind, realtime_ns: i64) {
        let us = u64::try_from(realtime_ns.max(0) / 1_000).unwrap_or(0);
        match kind {
            EventKind::OomKill => self.kernel.skip_until_us = Some(us),
            EventKind::Crash => self.crashes.skip_until_us = Some(us),
            _ => {}
        }
    }

    /// Once at start, then every 60 s while a timeline box is visible.
    pub fn due(&self, now: TimeNs, timeline_visible: bool) -> bool {
        match self.last_fetch {
            None => true,
            Some(last) => timeline_visible && now.saturating_sub(last) >= JOURNAL_PERIOD_NS,
        }
    }

    /// Blocking. Runs both fetches, each from its last cursor with a 5 s deadline, and returns
    /// the events placed on this boot's CLOCK_BOOTTIME. A fetch that fails keeps its cursor and
    /// is tried again at the next fetch. `stop` ends a fetch early.
    pub fn fetch(
        &mut self,
        now: TimeNs,
        realtime_offset_ns: i64,
        stop: impl Fn() -> bool,
    ) -> JournalFetch {
        self.last_fetch = Some(now);
        let access = *self.access.get_or_insert_with(|| {
            let (euid, gids) = self.env.ids.clone().unwrap_or_else(process_ids);
            access_for(euid, &gids, &self.env.group_file)
        });
        let mut events = Vec::new();
        if access == JournalAccess::OwnOnly {
            return JournalFetch { access, events };
        }
        let kernel = run(&self.env, &mut self.kernel, &["-k"], &[], &stop);
        events.extend(
            kernel
                .iter()
                .filter_map(|e| oom_event(e, realtime_offset_ns)),
        );
        let id = format!("MESSAGE_ID={COREDUMP_MESSAGE_ID}");
        let crashes = run(&self.env, &mut self.crashes, &[], &[id], &stop);
        events.extend(
            crashes
                .iter()
                .filter_map(|e| crash_event(e, realtime_offset_ns)),
        );
        events.sort_by_key(|e| e.start_t_ns);
        JournalFetch { access, events }
    }
}

/// The fields of one journal entry the detector reads.
#[derive(Debug, PartialEq)]
struct Entry {
    cursor: Arc<str>,
    realtime_us: u64,
    message: Arc<str>,
    comm: Option<Arc<str>>,
}

/// Runs `journalctl <pre> -b -o json --no-pager [--after-cursor=C] <post>` and returns the new
/// entries after `src.skip_until_us`. Moves the cursor to the newest entry read.
fn run(
    env: &JournalEnv,
    src: &mut JournalSource,
    pre: &[&str],
    post: &[String],
    stop: &impl Fn() -> bool,
) -> Vec<Entry> {
    let mut cmd = Command::new("journalctl");
    cmd.args(pre).args(["-b", "-o", "json", "--no-pager"]);
    if let Some(cursor) = &src.cursor {
        cmd.arg(format!("--after-cursor={cursor}"));
    }
    cmd.args(post)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = &env.path {
        cmd.env("PATH", path);
    }
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::debug!(target: "tasklens::events", error = %e, "journalctl did not start");
            return Vec::new();
        }
    };
    let out = match collect(child, Instant::now() + JOURNAL_TIMEOUT, stop) {
        Ok(out) if out.status.success() => out,
        Ok(_) | Err(CollectError::Io(_)) => {
            tracing::debug!(target: "tasklens::events", "journal event fetch failed");
            return Vec::new();
        }
        Err(CollectError::Timeout) => {
            tracing::debug!(target: "tasklens::events", "journal event fetch timed out");
            return Vec::new();
        }
        Err(CollectError::Stopped) => return Vec::new(),
    };
    let entries: Vec<Entry> = out
        .stdout
        .split(|&b| b == b'\n')
        .filter_map(parse_entry)
        .collect();
    if let Some(last) = entries.last() {
        src.cursor = Some(Arc::clone(&last.cursor));
    }
    let skip = src.skip_until_us;
    entries
        .into_iter()
        .filter(|e| skip.is_none_or(|s| e.realtime_us > s))
        .collect()
}

fn parse_entry(line: &[u8]) -> Option<Entry> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(line) else {
        return None;
    };
    let cursor = map.get("__CURSOR")?.as_str()?;
    let realtime_us = map
        .get("__REALTIME_TIMESTAMP")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<u64>().ok())
        .unwrap_or(0);
    Some(Entry {
        cursor: Arc::from(cursor),
        realtime_us,
        message: message_text(map.get("MESSAGE")),
        comm: map
            .get("COREDUMP_COMM")
            .and_then(Value::as_str)
            .map(Arc::from),
    })
}

/// CLOCK_BOOTTIME of a realtime µs stamp, 0 when it is before this boot.
fn boot_ns(realtime_us: u64, realtime_offset_ns: i64) -> u64 {
    let rt = i64::try_from(realtime_us.saturating_mul(1_000)).unwrap_or(i64::MAX);
    u64::try_from(rt.saturating_sub(realtime_offset_ns)).unwrap_or(0)
}

/// The process name of an OOM kill message: `... Killed process 1234 (name) total-vm:...`.
fn oom_name(message: &str) -> Option<&str> {
    let rest = &message[message.find(OOM_TEXT)? + OOM_TEXT.len()..];
    let open = rest.find('(')? + 1;
    let tail = &rest[open..];
    let close = tail.rfind(") total-vm").or_else(|| tail.rfind(')'))?;
    Some(&tail[..close])
}

fn oom_event(e: &Entry, realtime_offset_ns: i64) -> Option<HistEvent> {
    if !e.message.contains(OOM_TEXT) {
        return None;
    }
    Some(HistEvent {
        kind: EventKind::OomKill,
        start_t_ns: boot_ns(e.realtime_us, realtime_offset_ns),
        duration_ms: 0,
        peak: 0.,
        device_key: None,
        process: oom_name(&e.message).map(Arc::from),
    })
}

fn crash_event(e: &Entry, realtime_offset_ns: i64) -> Option<HistEvent> {
    Some(HistEvent {
        kind: EventKind::Crash,
        start_t_ns: boot_ns(e.realtime_us, realtime_offset_ns),
        duration_ms: 0,
        peak: 0.,
        device_key: None,
        process: e.comm.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::PermissionsExt;

    use smallvec::smallvec;
    use tl_model::series::SeriesView;
    use tl_model::snapshot::SeriesSet;
    use tl_model::time::BootId;

    const S: u64 = NS;
    const MS: u64 = 1_000_000;

    fn name(n: &str) -> Option<Arc<str>> {
        Some(Arc::from(n))
    }

    /// Readings once per second from `from` to `to` (exclusive), built by `f(second)`.
    fn run_secs(
        d: &mut EventDetector,
        from: u64,
        to: u64,
        f: impl Fn(u64) -> Reading,
    ) -> Vec<HistEvent> {
        let mut out = Vec::new();
        for s in from..to {
            let mut r = f(s);
            r.t_ns = s * S;
            out.extend(d.observe(&r));
        }
        out
    }

    fn psi(v: f32, top: Option<&str>) -> Reading {
        Reading {
            psi_some_pct: Some(v),
            top: TopProcesses {
                memory: top.map(Arc::from),
                ..TopProcesses::default()
            },
            ..Reading::default()
        }
    }

    #[test]
    fn event_detector_rules() {
        // Memory pressure: 29 s over is not an event.
        let mut d = EventDetector::new();
        let ev = run_secs(&mut d, 0, 29, |_| psi(12., None));
        assert!(ev.is_empty());
        assert!(run_secs(&mut d, 29, 200, |_| psi(1., None)).is_empty());

        // 40 s over, peak 30 at second 15, then under. Ends 60 s after the first second under.
        let mut d = EventDetector::new();
        let over = |s: u64| {
            let v = if s == 115 { 30. } else { 11. };
            let top = if s < 125 { "java" } else { "chrome" };
            psi(v, Some(top))
        };
        assert!(run_secs(&mut d, 100, 140, over).is_empty());
        assert!(run_secs(&mut d, 140, 199, |_| psi(2., None)).is_empty());
        let ev = run_secs(&mut d, 199, 201, |_| psi(2., None));
        assert_eq!(
            ev,
            vec![HistEvent {
                kind: EventKind::MemoryPressure,
                start_t_ns: 100 * S,
                duration_ms: 40_000,
                peak: 30.,
                device_key: name(MEMORY_KEY),
                process: name("java"),
            }]
        );

        // A dip under the threshold shorter than 60 s does not end it.
        let mut d = EventDetector::new();
        let dip = |s: u64| psi(if (40..90).contains(&s) { 3. } else { 15. }, None);
        assert!(run_secs(&mut d, 0, 120, dip).is_empty());
        let ev = run_secs(&mut d, 120, 181, |_| psi(0., None));
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].start_t_ns, ev[0].duration_ms), (0, 120_000));

        // Swap storm at 2,000 pages/s for 30 s.
        let mut d = EventDetector::new();
        let swap = |v: f32| Reading {
            swap_in_pps: Some(v),
            top: TopProcesses {
                swap: name("qemu"),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        assert!(run_secs(&mut d, 0, 31, |_| swap(2_000.)).is_empty());
        let ev = run_secs(&mut d, 31, 92, |_| swap(1_999.));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EventKind::SwapStorm);
        assert_eq!((ev[0].duration_ms, ev[0].peak), (31_000, 2_000.));
        assert_eq!(ev[0].process, name("qemu"));

        // Heat: CPU at its threshold, and a GPU without a threshold from throttling only.
        let mut d = EventDetector::new();
        let heat = |s: u64| Reading {
            cpu_temp_c: Some(if s < 29 { 100. } else { 60. }),
            cpu_hot_c: Some(100.),
            gpus: smallvec![GpuReading {
                key: Arc::from("gpu:a"),
                util_pct: Some(10.),
                temp_c: Some(99.),
                hot_c: None,
                throttling: Some(s < 40),
            }],
            top: TopProcesses {
                cpu: name("make"),
                gpu: name("blender"),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        let ev = run_secs(&mut d, 0, 120, heat);
        assert_eq!(ev.len(), 1, "29 s of CPU heat is too short: {ev:?}");
        assert_eq!(ev[0].kind, EventKind::Heat);
        assert_eq!(ev[0].device_key, name("gpu:a"));
        assert_eq!((ev[0].duration_ms, ev[0].peak), (40_000, 99.));
        assert_eq!(ev[0].process, name("blender"));

        // A 97 °C CPU without hot_c never gives heat.
        let mut d = EventDetector::new();
        let no_hot = |_| Reading {
            cpu_temp_c: Some(97.),
            ..Reading::default()
        };
        assert!(run_secs(&mut d, 0, 400, no_hot).is_empty());
        assert!(d.finish(400 * S).is_empty());

        // Heat from the temperature alone: the CPU at or above hot_c for 35 s, peak 104 °C.
        let mut d = EventDetector::new();
        let cpu_hot = |s: u64| Reading {
            cpu_temp_c: Some(match s {
                0..10 => 101.,
                10 => 104.,
                11..35 => 100.,
                _ => 70.,
            }),
            cpu_hot_c: Some(100.),
            top: TopProcesses {
                cpu: name("ffmpeg"),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        assert!(run_secs(&mut d, 0, 94, cpu_hot).is_empty());
        let ev = run_secs(&mut d, 94, 96, cpu_hot);
        assert_eq!(
            ev,
            vec![HistEvent {
                kind: EventKind::Heat,
                start_t_ns: 0,
                duration_ms: 35_000,
                peak: 104.,
                device_key: name(CPU_KEY),
                process: name("ffmpeg"),
            }]
        );

        // Full load: 5 min at 95%, process only while a sweep ran.
        let mut d = EventDetector::new();
        let load = |s: u64| Reading {
            cpu_pct: Some(if s < 299 { 96. } else { 50. }),
            top: TopProcesses {
                cpu: (s >= 200).then(|| Arc::from("rustc")),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        let ev = run_secs(&mut d, 0, 360, load);
        assert!(ev.is_empty(), "299 s is too short");
        let mut d = EventDetector::new();
        let long = |s: u64| Reading {
            cpu_pct: Some(if s <= 300 { 99. } else { 50. }),
            top: TopProcesses {
                cpu: (s >= 200).then(|| Arc::from("rustc")),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        let ev = run_secs(&mut d, 0, 362, long);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EventKind::FullLoad);
        assert_eq!(ev[0].device_key, name(CPU_KEY));
        assert_eq!((ev[0].duration_ms, ev[0].peak), (301_000, 99.));
        assert_eq!(ev[0].process, name("rustc"));
        // Without any sweep the event has no process.
        let mut d = EventDetector::new();
        let bare = |s: u64| Reading {
            cpu_pct: Some(if s <= 300 { 99. } else { 0. }),
            ..Reading::default()
        };
        let ev = run_secs(&mut d, 0, 362, bare);
        assert_eq!(ev[0].process, None);

        // Full load on each GPU on its own: only the busy one gives an event, under its key.
        let mut d = EventDetector::new();
        let gpu = |key: &str, util: f32| GpuReading {
            key: Arc::from(key),
            util_pct: Some(util),
            temp_c: Some(60.),
            hot_c: Some(90.),
            throttling: Some(false),
        };
        let gpus = |s: u64| Reading {
            gpus: smallvec![
                gpu("gpu:a", 40.),
                gpu("gpu:b", if s < 310 { 97. } else { 5. })
            ],
            top: TopProcesses {
                gpu: name("blender"),
                ..TopProcesses::default()
            },
            ..Reading::default()
        };
        assert!(run_secs(&mut d, 0, 369, gpus).is_empty());
        let ev = run_secs(&mut d, 369, 371, gpus);
        assert_eq!(
            ev,
            vec![HistEvent {
                kind: EventKind::FullLoad,
                start_t_ns: 0,
                duration_ms: 310_000,
                peak: 97.,
                device_key: name("gpu:b"),
                process: name("blender"),
            }]
        );
    }

    #[test]
    fn event_detector_gap_and_finish() {
        // A gap ends a started event at its last second over and drops an unstarted window.
        let mut d = EventDetector::new();
        let both = |_| Reading {
            psi_some_pct: Some(20.),
            cpu_pct: Some(99.),
            ..Reading::default()
        };
        assert!(run_secs(&mut d, 0, 40, both).is_empty());
        let ev = run_secs(&mut d, 100, 101, |_| Reading::default());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EventKind::MemoryPressure);
        assert_eq!(ev[0].duration_ms, 39_000);
        assert!(run_secs(&mut d, 101, 200, |_| Reading::default()).is_empty());

        // Shutdown stores a running event with its duration so far.
        let mut d = EventDetector::new();
        assert!(run_secs(&mut d, 0, 50, |_| psi(50., None)).is_empty());
        let ev = d.finish(50 * S);
        assert_eq!((ev.len(), ev[0].duration_ms, ev[0].peak), (1, 50_000, 50.));
        assert!(d.finish(60 * S).is_empty());
    }

    /// A snapshot published at `t_ns` with the CPU at 99% and memory PSI at 20%. The CPU
    /// samples at 10 Hz. Each source last sent data at the given time.
    fn stalled(t_ns: TimeNs, cpu_ok_ns: TimeNs, memory_ok_ns: TimeNs) -> Snapshot {
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.published_ns = t_ns;
        let sys = Arc::make_mut(&mut snap.system);
        sys.cpu.util_pct = Metric::valid(99.);
        sys.memory.psi_some_pct = Metric::valid(20.);
        let health = Arc::make_mut(&mut snap.health);
        health.sources[SourceId::Cpu.index()].last_ok_ns = Some(cpu_ok_ns);
        health.sources[SourceId::Memory.index()].last_ok_ns = Some(memory_ok_ns);
        snap.series = Arc::new(SeriesSet {
            series: Arc::from([Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric: MetricId::CPU_TOTAL,
                },
                version: 1,
                period_ns: 100 * MS,
                samples: Arc::default(),
            })]),
            ..SeriesSet::default()
        });
        snap
    }

    #[test]
    fn event_detector_stale_source() {
        // Each value counts up to the stale threshold of its own source: 1.5 s for the 10 Hz
        // CPU, 3 s for the 1 s memory sampler. The publish time never makes a value fresh.
        let r = Reading::from_snapshot(&stalled(10 * S, 8_500 * MS, 7 * S));
        assert_eq!(
            (r.t_ns, r.cpu_pct, r.psi_some_pct),
            (10 * S, Some(99.), Some(20.))
        );
        let r = Reading::from_snapshot(&stalled(10 * S, 8_400 * MS, 7 * S - 1));
        assert_eq!((r.cpu_pct, r.psi_some_pct), (None, None));
        // A source that never sent data has no values.
        let mut snap = stalled(10 * S, 10 * S, 10 * S);
        Arc::make_mut(&mut snap.health).sources[SourceId::Memory.index()].last_ok_ns = None;
        assert_eq!(Reading::from_snapshot(&snap).psi_some_pct, None);

        // Both samplers stall at 10 s while their last values stay over the thresholds: no
        // event starts.
        let mut d = EventDetector::new();
        for s in 10..400 {
            let r = Reading::from_snapshot(&stalled(s * S, 10 * S, 10 * S));
            assert!(d.observe(&r).is_empty(), "at {s} s");
        }
        assert!(d.finish(400 * S).is_empty());

        // The samplers stop at 48 s during a memory pressure event. Memory values count up to
        // 51 s. At 55 s, 4 s without a fresh value, the event ends at 51 s, its last fresh second
        // over: the stale seconds are not seconds under the threshold.
        let mut d = EventDetector::new();
        let at = |s: u64| {
            let ok = s.min(48) * S;
            Reading::from_snapshot(&stalled(s * S, ok, ok))
        };
        let mut ended = Vec::new();
        for s in 0..55 {
            assert!(d.observe(&at(s)).is_empty(), "at {s} s");
        }
        ended.extend(d.observe(&at(55)));
        assert_eq!(
            ended,
            vec![HistEvent {
                kind: EventKind::MemoryPressure,
                start_t_ns: 0,
                duration_ms: 51_000,
                peak: 20.,
                device_key: name(MEMORY_KEY),
                process: None,
            }]
        );
        assert!(run_secs(&mut d, 56, 200, at).is_empty());
        assert!(
            d.finish(200 * S).is_empty(),
            "the CPU window had not passed"
        );

        // Each GPU is judged by the worker that sends its values.
        let gpu_id = |uuid| DeviceIdentity::Gpu {
            uuid,
            pci_bus_id: Arc::from("0000:01:00.0"),
        };
        assert_eq!(gpu_source(&gpu_id([0; 16])), SourceId::GpuDrm);
        assert_eq!(gpu_source(&gpu_id([7; 16])), SourceId::GpuNvidia);
    }

    /// The rule an `event_detector_fresh_seconds` row drives, one at a time.
    #[derive(Clone, Copy, Debug)]
    enum Rule {
        Pressure,
        Swap,
        /// A CPU with `hot_c` 90 °C: the temperature is the input.
        CpuHeat,
        /// A GPU without `hot_c`: the throttling flag is the input, the temperature is not.
        GpuThrottle,
        CpuLoad,
        GpuLoad,
    }

    impl Rule {
        const ALL: [Rule; 6] = [
            Rule::Pressure,
            Rule::Swap,
            Rule::CpuHeat,
            Rule::GpuThrottle,
            Rule::CpuLoad,
            Rule::GpuLoad,
        ];

        fn kind_key(self) -> (EventKind, &'static str) {
            match self {
                Rule::Pressure => (EventKind::MemoryPressure, MEMORY_KEY),
                Rule::Swap => (EventKind::SwapStorm, MEMORY_KEY),
                Rule::CpuHeat => (EventKind::Heat, CPU_KEY),
                Rule::GpuThrottle => (EventKind::Heat, "gpu:a"),
                Rule::CpuLoad => (EventKind::FullLoad, CPU_KEY),
                Rule::GpuLoad => (EventKind::FullLoad, "gpu:a"),
            }
        }

        /// The reading of one second: `o` over, `u` under, `-` the input missing or stale.
        fn reading(self, c: u8) -> Reading {
            let pick = |over: f32, under: f32| match c {
                b'o' => Some(over),
                b'u' => Some(under),
                _ => None,
            };
            let gpu = |util_pct, throttling| GpuReading {
                key: Arc::from("gpu:a"),
                util_pct,
                temp_c: Some(99.),
                hot_c: None,
                throttling,
            };
            let mut r = Reading::default();
            match self {
                Rule::Pressure => r.psi_some_pct = pick(20., 1.),
                Rule::Swap => r.swap_in_pps = pick(3_000., 10.),
                Rule::CpuHeat => {
                    r.cpu_temp_c = pick(95., 60.);
                    r.cpu_hot_c = Some(90.);
                }
                Rule::GpuThrottle => r.gpus = smallvec![gpu(None, pick(1., 0.).map(|v| v > 0.))],
                Rule::CpuLoad => r.cpu_pct = pick(99., 10.),
                Rule::GpuLoad => r.gpus = smallvec![gpu(pick(99., 10.), Some(false))],
            }
            r
        }
    }

    /// A per-second pattern from runs of `(seconds, char)`. `_` is a second without a reading.
    fn pattern(runs: &[(usize, u8)]) -> Vec<u8> {
        runs.iter()
            .flat_map(|&(n, c)| std::iter::repeat_n(c, n))
            .collect()
    }

    /// The detector's ended events for `p` as (start, end) seconds.
    fn detect(rule: Rule, p: &[u8]) -> Vec<(u64, u64)> {
        let (kind, key) = rule.kind_key();
        let mut d = EventDetector::new();
        let mut out = Vec::new();
        for (s, &c) in p.iter().enumerate() {
            if c == b'_' {
                continue;
            }
            let mut r = rule.reading(c);
            r.t_ns = s as u64 * S;
            for e in d.observe(&r) {
                assert_eq!((e.kind, e.device_key.as_deref()), (kind, Some(key)));
                let end = e.start_t_ns + u64::from(e.duration_ms) * MS;
                out.push((e.start_t_ns / S, end / S));
            }
        }
        out
    }

    /// The fresh-seconds rule written plainly over a whole pattern: split it where more than 3 s
    /// pass without a fresh second, drop the missing seconds, then look for `window` fresh
    /// seconds over in a row and, after them, 60 fresh seconds under in a row. A part that a gap
    /// ends closes its running event at its last second over. Seconds after the last reading are
    /// not seen yet.
    fn reference(p: &[u8], window: usize) -> Vec<(u64, u64)> {
        let seen = p.len() - p.iter().rev().take_while(|&&c| c == b'_').count();
        // Each part: its fresh seconds (second, over) and whether a gap ends it.
        let mut parts: Vec<(Vec<(u64, bool)>, bool)> = vec![(Vec::new(), false)];
        let mut missing = 0;
        for (s, &c) in p[..seen].iter().enumerate() {
            if c == b'o' || c == b'u' {
                if missing > 3 {
                    parts.last_mut().unwrap().1 = true;
                    parts.push((Vec::new(), false));
                }
                missing = 0;
                parts.last_mut().unwrap().0.push((s as u64, c == b'o'));
            } else {
                missing += 1;
            }
        }
        if missing > 3 {
            parts.last_mut().unwrap().1 = true;
        }
        let run = |fresh: &[(u64, bool)], over: bool| {
            fresh.iter().take_while(|(_, o)| *o == over).count()
        };
        let mut out = Vec::new();
        for (fresh, gap_after) in parts {
            let mut i = 0;
            while i < fresh.len() {
                let n = run(&fresh[i..], true);
                if n < window {
                    i += n.max(1);
                    continue;
                }
                let start = fresh[i].0;
                let mut j = i + window;
                let mut end = None;
                while j < fresh.len() {
                    let n = run(&fresh[j..], false);
                    if n >= 60 {
                        end = Some(fresh[j].0);
                        j += 60;
                        break;
                    }
                    j += n.max(1);
                }
                match end {
                    Some(end) => {
                        out.push((start, end));
                        i = j;
                    }
                    None => {
                        if gap_after {
                            let last_over = fresh.iter().rev().find(|(_, o)| *o).unwrap().0;
                            out.push((start, last_over));
                        }
                        break;
                    }
                }
            }
        }
        out
    }

    /// A table row: what it checks, the rule, the pattern and the ended events as (start, end)
    /// seconds.
    type Row = (&'static str, Rule, Vec<u8>, Vec<(u64, u64)>);

    #[test]
    fn event_detector_fresh_seconds() {
        // Over only at 0, 3, ..., 30 with the seconds between missing: 11 fresh seconds over.
        let sparse: Vec<u8> = (0..=30)
            .map(|s| if s % 3 == 0 { b'o' } else { b'-' })
            .chain(std::iter::repeat_n(b'u', 70))
            .collect();
        let rows: Vec<Row> = vec![
            ("review example", Rule::Pressure, sparse, vec![]),
            (
                "30 s over then 60 s under",
                Rule::Pressure,
                pattern(&[(30, b'o'), (60, b'u')]),
                vec![(0, 30)],
            ),
            (
                "29 s over is too short",
                Rule::Pressure,
                pattern(&[(29, b'o'), (60, b'u')]),
                vec![],
            ),
            (
                "1 s missing in the start window",
                Rule::Pressure,
                pattern(&[(10, b'o'), (1, b'-'), (20, b'o'), (60, b'u')]),
                vec![(0, 31)],
            ),
            (
                "3 s missing in the start window",
                Rule::Swap,
                pattern(&[(10, b'o'), (3, b'-'), (20, b'o'), (60, b'u')]),
                vec![(0, 33)],
            ),
            (
                "4 s missing in the start window resets it",
                Rule::Pressure,
                pattern(&[(10, b'o'), (4, b'-'), (29, b'o'), (60, b'u')]),
                vec![],
            ),
            (
                "the window starts again after a 4 s gap",
                Rule::CpuHeat,
                pattern(&[(10, b'o'), (4, b'-'), (30, b'o'), (60, b'u')]),
                vec![(14, 44)],
            ),
            (
                "4 s without readings in the start window resets it",
                Rule::Pressure,
                pattern(&[(10, b'o'), (4, b'_'), (29, b'o'), (60, b'u')]),
                vec![],
            ),
            (
                "1 s missing in the quiet window",
                Rule::Pressure,
                pattern(&[(30, b'o'), (20, b'u'), (1, b'-'), (40, b'u')]),
                vec![(0, 30)],
            ),
            (
                "3 s missing in the quiet window",
                Rule::GpuThrottle,
                pattern(&[(30, b'o'), (20, b'u'), (3, b'-'), (40, b'u')]),
                vec![(0, 30)],
            ),
            (
                "4 s missing in the quiet window ends at the last second over",
                Rule::Pressure,
                pattern(&[(30, b'o'), (20, b'u'), (4, b'-'), (60, b'u')]),
                vec![(0, 29)],
            ),
            (
                "3 s without readings, then 60 s under",
                Rule::Swap,
                pattern(&[(30, b'o'), (3, b'_'), (60, b'u')]),
                vec![(0, 33)],
            ),
            (
                "4 s without readings while over",
                Rule::Pressure,
                pattern(&[(30, b'o'), (4, b'_'), (60, b'u')]),
                vec![(0, 29)],
            ),
            (
                "a running event ends at its last fresh second over, then a new one",
                Rule::GpuThrottle,
                pattern(&[(40, b'o'), (4, b'-'), (60, b'o'), (60, b'u')]),
                vec![(0, 39), (44, 104)],
            ),
            (
                "a dip of 59 s under does not end it",
                Rule::CpuHeat,
                pattern(&[(30, b'o'), (59, b'u'), (1, b'o'), (60, b'u')]),
                vec![(0, 90)],
            ),
            (
                "full load needs 300 fresh seconds",
                Rule::CpuLoad,
                pattern(&[(299, b'o'), (60, b'u'), (300, b'o'), (60, b'u')]),
                vec![(359, 659)],
            ),
            (
                "3 s missing inside a full load window",
                Rule::GpuLoad,
                pattern(&[(150, b'o'), (3, b'-'), (150, b'o'), (60, b'u')]),
                vec![(0, 303)],
            ),
        ];
        for (what, rule, p, want) in &rows {
            let window = window_s(rule.kind_key().0) as usize;
            assert_eq!(&reference(p, window), want, "reference: {what}");
            assert_eq!(&detect(*rule, p), want, "{what} ({rule:?})");
        }

        // Generated patterns of runs, checked against the reference. xorshift64 with a fixed
        // seed, so a failure repeats.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let mut events = 0;
        for i in 0..240 {
            let rule = Rule::ALL[i % Rule::ALL.len()];
            let scale = if matches!(rule, Rule::CpuLoad | Rule::GpuLoad) {
                10
            } else {
                1
            };
            let mut runs = Vec::new();
            for _ in 0..(4 + next(16)) {
                runs.push(match next(10) {
                    0..4 => ((1 + next(45)) as usize * scale, b'o'),
                    4..7 => ((1 + next(70)) as usize * scale, b'u'),
                    7..9 => ((1 + next(6)) as usize, b'-'),
                    _ => ((1 + next(6)) as usize, b'_'),
                });
            }
            let p = pattern(&runs);
            let window = window_s(rule.kind_key().0) as usize;
            let want = reference(&p, window);
            events += want.len();
            assert_eq!(detect(rule, &p), want, "pattern {i} ({rule:?}): {runs:?}");
        }
        assert!(
            events >= 20,
            "the generated patterns gave only {events} events"
        );
    }

    #[test]
    fn event_detector_suspend() {
        let clock = |boot_s: u64, mono_s: u64| Clock {
            boot_ns: boot_s * S,
            mono_ns: mono_s * S,
        };
        // Logind reports both edges; the clock check then stays quiet.
        let mut d = EventDetector::new();
        assert!(d.check_clock(clock(10, 10)).is_none());
        assert!(d.sleep(true, 11 * S).is_none());
        let ev = d.sleep(false, 311 * S).expect("wake gives the span");
        assert_eq!(ev, suspend_event(11 * S, 311 * S));
        assert!(d.check_clock(clock(312, 12)).is_none());

        // The wake was missed: the clock check closes the span from the sleep signal.
        let mut d = EventDetector::new();
        d.check_clock(clock(10, 10));
        d.sleep(true, 11 * S);
        let ev = d.check_clock(clock(500, 12)).expect("clock sees the sleep");
        assert_eq!((ev.start_t_ns, ev.duration_ms), (11 * S, 489_000));
        assert!(d.sleep(false, 501 * S).is_none(), "no second span");

        // Both signals missed: the slept time before now.
        let mut d = EventDetector::new();
        d.check_clock(clock(10, 10));
        let ev = d.check_clock(clock(111, 11)).expect("clock sees the sleep");
        assert_eq!((ev.start_t_ns, ev.duration_ms), (11 * S, 100_000));
        // Normal ticks give nothing.
        assert!(d.check_clock(clock(112, 12)).is_none());

        // The sleep signal was missed but the wake came: the clock check still gives the span.
        let mut d = EventDetector::new();
        d.check_clock(clock(10, 10));
        assert!(d.sleep(false, 110 * S).is_none());
        let ev = d.check_clock(clock(111, 11)).expect("clock sees the sleep");
        assert_eq!((ev.start_t_ns, ev.duration_ms), (11 * S, 100_000));
    }

    #[test]
    fn oom_name_from_kernel_message() {
        let m = "Out of memory: Killed process 4242 (stress-ng-vm) total-vm:1234kB, anon-rss:1kB, \
                 UID:1000 pgtables:12kB oom_score_adj:0";
        assert_eq!(oom_name(m), Some("stress-ng-vm"));
        assert_eq!(
            oom_name("Out of memory: Killed process 7 (a (b))"),
            Some("a (b)")
        );
        assert_eq!(oom_name("oom-kill:constraint=CONSTRAINT_NONE"), None);
    }

    /// A fresh directory under the target dir for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("event-detector-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fake `journalctl` that logs its argv and prints the kernel or coredump fixture.
    fn fake_env(name: &str, groups: &str, ids: (u32, Vec<u32>)) -> (JournalEnv, PathBuf) {
        let dir = scratch(name);
        let kernel = concat!(
            r#"{"__CURSOR":"k1","__REALTIME_TIMESTAMP":"1700000010000000","MESSAGE":"usb 1-1: new device"}"#,
            "\n",
            r#"{"__CURSOR":"k2","__REALTIME_TIMESTAMP":"1700000020000000","MESSAGE":"Out of memory: Killed process 99 (hog) total-vm:1kB, UID:1000 pgtables:1kB oom_score_adj:0"}"#,
            "\n",
        );
        let core = concat!(
            r#"{"__CURSOR":"c1","__REALTIME_TIMESTAMP":"1700000030000000","MESSAGE":"Process 5 (vlc) dumped core.","COREDUMP_COMM":"vlc"}"#,
            "\n",
        );
        std::fs::write(dir.join("kernel.json"), kernel).unwrap();
        std::fs::write(dir.join("core.json"), core).unwrap();
        let script = dir.join("journalctl");
        std::fs::write(
            &script,
            "#!/bin/sh\nd=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$d/calls\"\n\
             case \"$*\" in *after-cursor*) exit 0;; esac\n\
             case \"$1\" in -k) cat \"$d/kernel.json\";; *) cat \"$d/core.json\";; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let group_file = dir.join("group");
        std::fs::write(&group_file, groups).unwrap();
        let env = JournalEnv {
            path: Some(format!("{}:/usr/bin:/bin", dir.display()).into()),
            group_file,
            ids: Some(ids),
        };
        (env, dir)
    }

    #[test]
    fn event_detector_journal_kinds() {
        let (env, dir) = fake_env("all", "wheel:x:998:me\n", (1000, vec![1000, 998]));
        let mut j = JournalEvents::new(env);
        assert!(j.due(0, false), "fetched once at start");
        // Realtime 1_700_000_000 s is boottime 0.
        let offset = 1_700_000_000 * S as i64;
        let got = j.fetch(0, offset, || false);
        assert_eq!(got.access, JournalAccess::All);
        assert_eq!(got.events.len(), 2);
        assert_eq!(got.events[0].kind, EventKind::OomKill);
        assert_eq!(got.events[0].start_t_ns, 20 * S);
        assert_eq!(got.events[0].process, name("hog"));
        assert_eq!(got.events[1].kind, EventKind::Crash);
        assert_eq!(got.events[1].start_t_ns, 30 * S);
        assert_eq!(got.events[1].process, name("vlc"));

        assert!(!j.due(30 * S, true));
        assert!(!j.due(60 * S, false), "only while a timeline is visible");
        assert!(j.due(60 * S, true));
        let again = j.fetch(60 * S, offset, || false);
        assert!(again.events.is_empty());
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines[0], "-k -b -o json --no-pager");
        assert_eq!(
            lines[1],
            format!("-b -o json --no-pager MESSAGE_ID={COREDUMP_MESSAGE_ID}")
        );
        assert_eq!(lines[2], "-k -b -o json --no-pager --after-cursor=k2");
        assert_eq!(
            lines[3],
            format!("-b -o json --no-pager --after-cursor=c1 MESSAGE_ID={COREDUMP_MESSAGE_ID}")
        );

        // A restart in the same boot skips what the store already has.
        let (env, _) = fake_env("skip", "wheel:x:998:me\n", (1000, vec![998]));
        let mut j = JournalEvents::new(env);
        j.skip_until(EventKind::OomKill, 1_700_000_020 * S as i64);
        let got = j.fetch(0, offset, || false);
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, EventKind::Crash);
    }

    #[test]
    fn event_detector_journal_own_only_omits_kinds() {
        let (env, dir) = fake_env("own", "wheel:x:998:\n", (1000, vec![1000]));
        let mut j = JournalEvents::new(env);
        let got = j.fetch(0, 0, || false);
        assert_eq!(got.access, JournalAccess::OwnOnly);
        assert!(got.events.is_empty());
        assert!(
            !dir.join("calls").exists(),
            "journalctl must not run under OwnOnly"
        );
    }
}
