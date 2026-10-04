//! History read side (ADR-012 "Reading"): process state at one second, range averages,
//! min/max decimation, system totals, the stacked top-3 graph and the top-K hover.
//!
//! The state at time T is the block keyframe plus every delta with `start + dt <= T`. Every
//! function that returns process rows skips the keys in `DeletedKeys` (R-103).

use std::collections::{HashMap, HashSet};

use tl_model::history::HistoryMetric;
use tl_model::ids::{DeviceIdx, MetricId, ProcessKey};
use tl_model::time::TimeNs;

use crate::format::{BLOCK_SECS, Block, HistDevice, HistMeta, HistProcess, MetaId};

const NS_PER_MS: u64 = 1_000_000;
const NS_PER_SEC: u64 = 1_000_000_000;
const BLOCK_NS: u64 = BLOCK_SECS * 1_000_000_000;

/// Meta ids of one scope (segment or window lifetime) and their text.
///
/// A reader of block k absorbs `new_meta` of blocks `0..=k` of the same segment.
#[derive(Clone, Debug, Default)]
pub struct MetaTable {
    metas: HashMap<MetaId, HistMeta>,
}

impl MetaTable {
    pub fn absorb(&mut self, b: &Block) {
        for (id, meta) in &b.new_meta {
            self.metas.insert(*id, meta.clone());
        }
    }

    pub fn get(&self, id: MetaId) -> Option<&HistMeta> {
        self.metas.get(&id)
    }
}

/// Process state at one second. `procs` is in block index order (keyframe, then started).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub t_ns: TimeNs,
    pub procs: Vec<HistProcess>,
}

/// The block's process list while it is replayed, indexed keyframe-then-started.
struct Replay<'b> {
    block: &'b Block,
    slots: Vec<HistProcess>,
    alive: Vec<bool>,
    /// Deltas applied so far.
    next: usize,
}

impl<'b> Replay<'b> {
    /// The keyframe state.
    fn new(block: &'b Block) -> Self {
        Replay {
            block,
            slots: block.keyframe.clone(),
            alive: vec![true; block.keyframe.len()],
            next: 0,
        }
    }

    /// Time of the next delta, if any is left.
    fn next_t(&self) -> Option<TimeNs> {
        let d = self.block.deltas.get(self.next)?;
        Some(delta_t(self.block, d.dt_ms))
    }

    /// Applies the next delta and returns its time.
    fn step(&mut self) -> Option<TimeNs> {
        let t = self.next_t()?;
        let d = &self.block.deltas[self.next];
        self.next += 1;
        for (i, patch) in &d.changed {
            if let Some(p) = self.slots.get_mut(*i as usize) {
                patch.apply(p);
            }
        }
        for p in &d.started {
            self.slots.push(p.clone());
            self.alive.push(true);
        }
        for i in &d.exited {
            if let Some(a) = self.alive.get_mut(*i as usize) {
                *a = false;
            }
        }
        Some(t)
    }

    fn live(&self) -> impl Iterator<Item = &HistProcess> {
        self.slots
            .iter()
            .zip(&self.alive)
            .filter_map(|(p, a)| a.then_some(p))
    }
}

/// `start + dt_ms`, saturating.
fn delta_t(b: &Block, dt_ms: u32) -> TimeNs {
    b.start_t_ns
        .saturating_add(u64::from(dt_ms).saturating_mul(NS_PER_MS))
}

/// State at `t`: keyframe plus every delta with start + dt <= t. None if t is outside the block,
/// which covers `[start, start + 60 s)`.
pub fn state_at(b: &Block, t_ns: TimeNs) -> Option<Frame> {
    if t_ns < b.start_t_ns || t_ns >= b.start_t_ns.saturating_add(BLOCK_NS) {
        return None;
    }
    let mut r = Replay::new(b);
    while r.next_t().is_some_and(|dt| dt <= t_ns) {
        r.step();
    }
    let Replay {
        mut slots, alive, ..
    } = r;
    let mut alive = alive.into_iter();
    slots.retain(|_| alive.next().unwrap_or(false));
    Some(Frame { t_ns, procs: slots })
}

/// Iterates every sweep state in the block in order (keyframe first). `Frame.t_ns` is the sweep
/// time: `start_t_ns` for the keyframe, `start + dt` for each delta. The frame buffer is reused.
pub fn for_each_state(b: &Block, mut f: impl FnMut(&Frame)) {
    let mut r = Replay::new(b);
    let mut frame = Frame {
        t_ns: b.start_t_ns,
        procs: Vec::with_capacity(r.slots.len()),
    };
    frame.procs.extend(r.live().cloned());
    f(&frame);
    while let Some(t) = r.step() {
        frame.t_ns = t;
        frame.procs.clear();
        frame.procs.extend(r.live().cloned());
        f(&frame);
    }
}

/// Averages and peaks of one process over a range.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeStats {
    pub key: ProcessKey,
    /// Meta id of the last sweep that saw the process. Resolve it with the `MetaTable` of that
    /// sweep's segment.
    pub meta: MetaId,
    /// Sweeps in the range in which the process was alive.
    pub samples: u32,
    pub avg: HistProcessF,
    pub peak: HistProcessF,
}

/// One process in display units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HistProcessF {
    pub cpu_pct: f32,
    pub mem_anon_bytes: u64,
    pub disk_bps: f32,
    pub net_bps: f32,
    pub gpu_pct: f32,
}

impl HistProcessF {
    /// Stored units back to display units.
    pub fn from_stored(p: &HistProcess) -> Self {
        HistProcessF {
            cpu_pct: f32::from(p.cpu_permille) / 10.0,
            mem_anon_bytes: u64::from(p.mem_anon_kib) * 1024,
            disk_bps: (u64::from(p.io_r_kibps) + u64::from(p.io_w_kibps)) as f32 * 1024.0,
            net_bps: p.net_kibps as f32 * 1024.0,
            gpu_pct: f32::from(p.gpu_pct),
        }
    }
}

/// Running sums and peaks of one process.
struct Acc {
    key: ProcessKey,
    meta: MetaId,
    samples: u32,
    cpu: f64,
    mem: u128,
    disk: f64,
    net: f64,
    gpu: f64,
    peak: HistProcessF,
}

impl Acc {
    fn add(&mut self, p: &HistProcess) {
        let v = HistProcessF::from_stored(p);
        self.meta = p.meta;
        self.samples += 1;
        self.cpu += f64::from(v.cpu_pct);
        self.mem += u128::from(v.mem_anon_bytes);
        self.disk += f64::from(v.disk_bps);
        self.net += f64::from(v.net_bps);
        self.gpu += f64::from(v.gpu_pct);
        let pk = &mut self.peak;
        pk.cpu_pct = pk.cpu_pct.max(v.cpu_pct);
        pk.mem_anon_bytes = pk.mem_anon_bytes.max(v.mem_anon_bytes);
        pk.disk_bps = pk.disk_bps.max(v.disk_bps);
        pk.net_bps = pk.net_bps.max(v.net_bps);
        pk.gpu_pct = pk.gpu_pct.max(v.gpu_pct);
    }

    fn finish(self) -> RangeStats {
        let n = f64::from(self.samples.max(1));
        let avg = HistProcessF {
            cpu_pct: (self.cpu / n) as f32,
            mem_anon_bytes: u64::try_from(self.mem / u128::from(self.samples.max(1)))
                .unwrap_or(u64::MAX),
            disk_bps: (self.disk / n) as f32,
            net_bps: (self.net / n) as f32,
            gpu_pct: (self.gpu / n) as f32,
        };
        RangeStats {
            key: self.key,
            meta: self.meta,
            samples: self.samples,
            avg,
            peak: self.peak,
        }
    }
}

/// Averages and peaks over [from, to] for every process seen in that range, across several blocks.
///
/// Only sweeps whose time lies in the range count. Average and peak use the sweeps in which the
/// process was alive. Results are in the order the processes were first seen.
pub fn range_stats<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    from: TimeNs,
    to: TimeNs,
    deleted: &DeletedKeys,
) -> Vec<RangeStats> {
    let mut index: HashMap<ProcessKey, usize> = HashMap::new();
    let mut accs: Vec<Acc> = Vec::new();
    for_each_sweep(blocks, from, to, |_, r| {
        for p in r.live().filter(|p| !deleted.contains(&p.key)) {
            let i = *index.entry(p.key).or_insert_with(|| {
                accs.push(Acc {
                    key: p.key,
                    meta: p.meta,
                    samples: 0,
                    cpu: 0.0,
                    mem: 0,
                    disk: 0.0,
                    net: 0.0,
                    gpu: 0.0,
                    peak: HistProcessF::default(),
                });
                accs.len() - 1
            });
            accs[i].add(p);
        }
    });
    accs.into_iter().map(Acc::finish).collect()
}

/// Calls `f` with the time and state of every sweep in [from, to], across several blocks.
fn for_each_sweep<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    from: TimeNs,
    to: TimeNs,
    mut f: impl FnMut(TimeNs, &Replay),
) {
    for b in blocks {
        if b.start_t_ns > to || b.start_t_ns.saturating_add(BLOCK_NS) <= from {
            continue;
        }
        let mut r = Replay::new(b);
        if (from..=to).contains(&b.start_t_ns) {
            f(b.start_t_ns, &r);
        }
        while let Some(t) = r.step() {
            if t > to {
                break;
            }
            if t >= from {
                f(t, &r);
            }
        }
    }
}

/// Most points in one decimated series (ADR-012 `HistoryView`).
pub const MAX_POINTS: usize = 1024;

/// The metric of a history graph. Mirrors `tl_model::history::HistoryMetric`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Metric {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
}

impl From<HistoryMetric> for Metric {
    fn from(m: HistoryMetric) -> Self {
        match m {
            HistoryMetric::Cpu => Metric::Cpu,
            HistoryMetric::Memory => Metric::Memory,
            HistoryMetric::Disk => Metric::Disk,
            HistoryMetric::Network => Metric::Network,
            HistoryMetric::Gpu => Metric::Gpu,
        }
    }
}

impl Metric {
    /// Value of one process: Cpu and Gpu in percent, Memory in bytes, Disk and Network in bytes
    /// per second.
    pub fn of(self, p: &HistProcess) -> f32 {
        let v = HistProcessF::from_stored(p);
        match self {
            Metric::Cpu => v.cpu_pct,
            Metric::Memory => v.mem_anon_bytes as f32,
            Metric::Disk => v.disk_bps,
            Metric::Network => v.net_bps,
            Metric::Gpu => v.gpu_pct,
        }
    }
}

/// One decimated bucket. `t_ns` is the bucket start. `min = max = NaN` is a gap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub t_ns: TimeNs,
    pub min: f32,
    pub max: f32,
}

/// Bucket width `w = max(1 s, ceil((to - from) / (MAX_POINTS - 1)))` rounded up to whole seconds.
/// Bucket i covers `[floor(t / w) * w, +w)` in absolute boottime, so two calls 100 ms apart give
/// the same boundaries.
///
/// The divisor is `MAX_POINTS - 1`, not `MAX_POINTS`: aligned buckets can cut the range so it
/// touches one more bucket than `(to - from) / w`, and the result must stay at `MAX_POINTS`.
pub fn bucket_width_ns(from: TimeNs, to: TimeNs) -> u64 {
    let per_point = to.saturating_sub(from).div_ceil(MAX_POINTS as u64 - 1);
    per_point.div_ceil(NS_PER_SEC).max(1) * NS_PER_SEC
}

/// Folds time-ordered points into min/max buckets in one pass.
struct Decimator {
    from: TimeNs,
    to: TimeNs,
    width: u64,
    out: Vec<Point>,
    cur: Option<Point>,
}

impl Decimator {
    fn new(from: TimeNs, to: TimeNs) -> Self {
        Decimator {
            from,
            to,
            width: bucket_width_ns(from, to),
            out: Vec::with_capacity(MAX_POINTS),
            cur: None,
        }
    }

    /// Points outside [from, to] are dropped. `f32::min` and `f32::max` ignore NaN, so a bucket
    /// stays NaN only when every point in it is NaN.
    fn push(&mut self, t_ns: TimeNs, v: f32) {
        if t_ns < self.from || t_ns > self.to {
            return;
        }
        let start = t_ns / self.width * self.width;
        match &mut self.cur {
            Some(p) if p.t_ns == start => {
                p.min = p.min.min(v);
                p.max = p.max.max(v);
            }
            cur => {
                if let Some(p) = cur.take() {
                    self.out.push(p);
                }
                *cur = Some(Point {
                    t_ns: start,
                    min: v,
                    max: v,
                });
            }
        }
    }

    fn finish(mut self) -> Vec<Point> {
        if let Some(p) = self.cur.take() {
            self.out.push(p);
        }
        self.out
    }
}

/// Min and max per bucket of `bucket_width_ns(from, to)` over the points in [from, to]. Points
/// must come in ascending time order. NaN points are gaps: they never count toward min or max,
/// and a bucket holding only NaN comes back as `min = max = NaN`. Buckets without points are
/// left out.
pub fn decimate(
    points: impl Iterator<Item = (TimeNs, f32)>,
    from: TimeNs,
    to: TimeNs,
) -> Vec<Point> {
    let mut d = Decimator::new(from, to);
    for (t, v) in points {
        d.push(t, v);
    }
    d.finish()
}

/// Value of one process for one metric at every sweep in [from, to] in which it was alive,
/// decimated. Empty when the key is deleted.
pub fn process_series<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    key: ProcessKey,
    m: Metric,
    from: TimeNs,
    to: TimeNs,
    deleted: &DeletedKeys,
) -> Vec<Point> {
    let mut d = Decimator::new(from, to);
    if deleted.contains(&key) {
        return d.finish();
    }
    for_each_sweep(blocks, from, to, |t, r| {
        if let Some(p) = r.live().find(|p| p.key == key) {
            d.push(t, m.of(p));
        }
    });
    d.finish()
}

/// Device kinds of `HistDevice.kind`.
const KIND_CPU_PACKAGE: u8 = 0;
const KIND_MEMORY: u8 = 2;
const KIND_GPU: u8 = 3;
const KIND_DISK: u8 = 4;
const KIND_NET: u8 = 5;

/// How the series of several devices make one total.
#[derive(Clone, Copy)]
enum Combine {
    Sum,
    Max,
}

impl Combine {
    /// NaN is a gap of one device and does not hide the others.
    fn apply(self, a: f32, b: f32) -> f32 {
        match self {
            Combine::Sum if a.is_nan() => b,
            Combine::Sum if b.is_nan() => a,
            Combine::Sum => a + b,
            Combine::Max => a.max(b),
        }
    }
}

/// Which devices and metrics make the system total of `m`.
fn system_parts(m: Metric) -> (fn(&HistDevice) -> bool, &'static [MetricId], Combine) {
    match m {
        Metric::Cpu => (
            |d| d.kind == KIND_CPU_PACKAGE,
            &[MetricId::CPU_TOTAL],
            Combine::Sum,
        ),
        Metric::Memory => (
            |d| d.kind == KIND_MEMORY,
            &[MetricId::MEM_USED],
            Combine::Sum,
        ),
        Metric::Disk => (
            |d| d.kind == KIND_DISK && d.parent.is_none(),
            &[MetricId::DISK_READ, MetricId::DISK_WRITE],
            Combine::Sum,
        ),
        Metric::Network => (
            |d| d.kind == KIND_NET,
            &[MetricId::NET_RX, MetricId::NET_TX],
            Combine::Sum,
        ),
        Metric::Gpu => (|d| d.kind == KIND_GPU, &[MetricId::GPU_UTIL], Combine::Max),
    }
}

/// System total of a metric per second from the stored 1 Hz series and the block catalog,
/// decimated. Cpu is `CPU_TOTAL` of the CpuPackage device; Memory is `MEM_USED`; Disk is the sum
/// of `DISK_READ + DISK_WRITE` over Disk devices with no parent; Network is the sum of
/// `NET_RX + NET_TX` over Net devices; Gpu is the largest `GPU_UTIL` over Gpu devices.
///
/// The catalog of a block holds until the next block that carries one; blocks before the first
/// catalog give no points. A device's NaN is skipped; a second is NaN only when every device is.
pub fn system_series<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    m: Metric,
    from: TimeNs,
    to: TimeNs,
) -> Vec<Point> {
    let (wanted, metrics, combine) = system_parts(m);
    let mut d = Decimator::new(from, to);
    let mut devices: Vec<DeviceIdx> = Vec::new();
    let mut seconds: Vec<(u32, f32)> = Vec::new();
    for b in blocks {
        if let Some(catalog) = &b.catalog {
            devices.clear();
            devices.extend(catalog.iter().filter(|d| wanted(d)).map(|d| d.idx));
        }
        if b.start_t_ns > to || b.start_t_ns.saturating_add(BLOCK_NS) <= from {
            continue;
        }
        seconds.clear();
        let series = b
            .system
            .iter()
            .filter(|(id, _)| metrics.contains(&id.metric) && devices.contains(&id.device));
        for (_, points) in series {
            for &(ms, v) in points {
                match seconds.binary_search_by_key(&ms, |(s, _)| *s) {
                    Ok(i) => seconds[i].1 = combine.apply(seconds[i].1, v),
                    Err(i) => seconds.insert(i, (ms, v)),
                }
            }
        }
        for &(ms, v) in &seconds {
            d.push(delta_t(b, ms), v);
        }
    }
    d.finish()
}

/// Running per-process sums of one stack bucket.
struct StackBucket {
    t_ns: TimeNs,
    sweeps: u32,
    sums: HashMap<ProcessKey, f64>,
}

impl StackBucket {
    /// The three largest per-process means, largest first, and the sum of the rest.
    fn finish(&mut self) -> (TimeNs, [f32; 3], f32) {
        let n = f64::from(self.sweeps.max(1));
        let mut top = [0.0f64; 3];
        let mut other = 0.0f64;
        for v in self.sums.drain().map(|(_, s)| s / n) {
            if v > top[2] {
                other += top[2];
                top[2] = v;
                top.sort_unstable_by(|a, b| b.total_cmp(a));
            } else {
                other += v;
            }
        }
        (self.t_ns, top.map(|v| v as f32), other as f32)
    }
}

/// Per bucket of `bucket_width_ns(from, to)`: the three largest per-process values of the metric
/// and "other" as the sum of the rest, drawn stacked in the metric colour (R-169 OI-5, D-020).
/// A process's value in a bucket is its mean over the bucket's sweeps, counting sweeps in which
/// it was not alive as 0, so the stack adds up to the mean of the per-sweep process total.
pub fn stack<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    m: Metric,
    from: TimeNs,
    to: TimeNs,
    deleted: &DeletedKeys,
) -> Vec<(TimeNs, [f32; 3], f32)> {
    let width = bucket_width_ns(from, to);
    let mut out = Vec::with_capacity(MAX_POINTS);
    let mut cur = StackBucket {
        t_ns: 0,
        sweeps: 0,
        sums: HashMap::new(),
    };
    for_each_sweep(blocks, from, to, |t, r| {
        let start = t / width * width;
        if cur.sweeps > 0 && cur.t_ns != start {
            out.push(cur.finish());
            cur.sweeps = 0;
        }
        cur.t_ns = start;
        cur.sweeps += 1;
        for p in r.live().filter(|p| !deleted.contains(&p.key)) {
            *cur.sums.entry(p.key).or_insert(0.0) += f64::from(m.of(p));
        }
    });
    if cur.sweeps > 0 {
        out.push(cur.finish());
    }
    out
}

/// Top `n` processes of a frame, ranked by the metric descending, ties by `ProcessKey`
/// ascending. Deleted keys are skipped.
pub fn top_at(
    frame: &Frame,
    m: Metric,
    n: usize,
    deleted: &DeletedKeys,
) -> Vec<(ProcessKey, MetaId, f32)> {
    let mut rows: Vec<_> = frame
        .procs
        .iter()
        .filter(|p| !deleted.contains(&p.key))
        .map(|p| (p.key, p.meta, m.of(p)))
        .collect();
    rows.sort_unstable_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
    rows.truncate(n);
    rows
}

/// Keys removed with "Delete from history". Every query filters them out (PROC-088, R-103).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletedKeys(HashSet<ProcessKey>);

impl DeletedKeys {
    pub fn contains(&self, k: &ProcessKey) -> bool {
        self.0.contains(k)
    }

    pub fn insert_all(&mut self, ks: &[ProcessKey]) {
        self.0.extend(ks.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockBuilder;
    use crate::format::hist_process;
    use std::sync::Arc;
    use tl_model::ids::GroupKey;
    use tl_model::ids::SeriesId;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    const SEC: u64 = 1_000_000_000;

    fn key(i: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + u64::from(i),
            boot_id: BootId(7),
            pid: 100 + i,
        }
    }

    fn record(i: u32, cpu_pct: f32) -> ProcessRecord {
        let text: Arc<str> = Arc::from(format!("proc{i}"));
        ProcessRecord {
            key: key(i),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text.clone(),
                name: text.clone(),
                exe: None,
                cmdline: None,
                user: Arc::from("alice"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                search_lower: text,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::default(),
            nice: 0,
            threads: 1,
            cpu_pct,
            rss_anon_bytes: 4 << 20,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 2048.0,
            io_write_bps: 1024.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn find(frame: &Frame, k: ProcessKey) -> Option<&HistProcess> {
        frame.procs.iter().find(|p| p.key == k)
    }

    #[test]
    fn state_at_matches_recorded_sweep() {
        let mut b = BlockBuilder::new(BootId(7));
        let mut inputs = Vec::new();
        for i in 0..60u32 {
            // A rises 1 permille per sweep; B stays put.
            let sweep = vec![record(0, i as f32 / 10.0), record(1, 5.0)];
            inputs.push(sweep.clone());
            assert!(b.on_process_sweep(u64::from(i) * SEC, &sweep).is_none());
        }
        let block = b.flush().unwrap();
        let mut metas = MetaTable::default();
        metas.absorb(&block);

        let frame = state_at(&block, 30 * SEC).unwrap();
        assert_eq!(frame.t_ns, 30 * SEC);
        let a = find(&frame, key(0)).unwrap();
        assert_eq!(a.cpu_permille, 30);
        assert_eq!(*a, hist_process(&inputs[30][0], a.meta));
        assert_eq!(&*metas.get(a.meta).unwrap().comm, "proc0");
        // Between sweeps the earlier sweep holds.
        let frame = state_at(&block, 30 * SEC + 999_000_000).unwrap();
        assert_eq!(find(&frame, key(0)).unwrap().cpu_permille, 30);

        let mut states = Vec::new();
        for_each_state(&block, |f| states.push(f.clone()));
        assert_eq!(states.len(), 60);
        for (i, f) in states.iter().enumerate() {
            assert_eq!(f.t_ns, i as u64 * SEC);
            assert_eq!(f.procs.len(), 2);
            assert_eq!(find(f, key(0)).unwrap().cpu_permille, i as u16);
        }
    }

    #[test]
    fn state_at_outside_block_none() {
        let mut b = BlockBuilder::new(BootId(7));
        for i in 10..70u64 {
            b.on_process_sweep(i * SEC, &[record(0, 1.0)]);
        }
        let block = b.flush().unwrap();
        assert_eq!(block.start_t_ns, 10 * SEC);
        assert!(state_at(&block, 9 * SEC).is_none());
        assert!(state_at(&block, 69 * SEC + 60 * SEC + 1).is_none());
        // The block covers [start, start + 60 s).
        assert!(state_at(&block, 70 * SEC).is_none());
        assert!(state_at(&block, 10 * SEC).is_some());
        assert!(state_at(&block, 70 * SEC - 1).is_some());
    }

    #[test]
    fn exited_absent_after_exit() {
        let mut b = BlockBuilder::new(BootId(7));
        for i in 0..20u64 {
            let mut sweep = vec![record(0, 1.0)];
            // Process 1 lives in sweeps 0..=9 and exits at sweep 10 (`deltas[9]`).
            if i < 10 {
                sweep.push(record(1, 2.0));
            }
            b.on_process_sweep(i * SEC, &sweep);
        }
        let block = b.flush().unwrap();
        assert_eq!(block.deltas[9].exited, vec![1]);
        let before = state_at(&block, 9 * SEC).unwrap();
        assert!(find(&before, key(1)).is_some());
        let after = state_at(&block, 15 * SEC).unwrap();
        assert!(find(&after, key(1)).is_none());
        assert!(find(&after, key(0)).is_some());
    }

    #[test]
    fn range_average_and_peak() {
        let mut b = BlockBuilder::new(BootId(7));
        for i in 0..30u64 {
            b.on_process_sweep(i * SEC, &[record(0, 10.0)]);
        }
        let first = b.flush().unwrap();
        for i in 30..60u64 {
            b.on_process_sweep(i * SEC, &[record(0, 20.0)]);
        }
        let second = b.flush().unwrap();
        let blocks = [first, second];

        let stats = range_stats(blocks.iter(), 0, 59 * SEC, &DeletedKeys::default());
        assert_eq!(stats.len(), 1);
        let a = &stats[0];
        assert_eq!(a.key, key(0));
        assert_eq!(a.samples, 60);
        assert_eq!(a.avg.cpu_pct, 15.0);
        assert_eq!(a.peak.cpu_pct, 20.0);
        assert_eq!(a.avg.mem_anon_bytes, 4 << 20);
        assert_eq!(a.avg.disk_bps, 3072.0);
        assert_eq!(a.peak.net_bps, 0.0);

        // Only sweeps inside the range count.
        let stats = range_stats(blocks.iter(), 25 * SEC, 34 * SEC, &DeletedKeys::default());
        assert_eq!(stats[0].samples, 10);
        assert_eq!(stats[0].avg.cpu_pct, 15.0);
    }

    #[test]
    fn range_partial_lifetime() {
        let mut b = BlockBuilder::new(BootId(7));
        for i in 0..60u64 {
            let mut sweep = vec![record(0, 1.0)];
            if (20..25).contains(&i) {
                sweep.push(record(1, (i - 19) as f32));
            }
            b.on_process_sweep(i * SEC, &sweep);
        }
        let block = b.flush().unwrap();
        let stats = range_stats(
            std::iter::once(&block),
            0,
            59 * SEC,
            &DeletedKeys::default(),
        );
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].samples, 60);
        let p = &stats[1];
        assert_eq!(p.key, key(1));
        assert_eq!(p.samples, 5);
        // CPU 1..=5 %: mean 3.
        assert_eq!(p.avg.cpu_pct, 3.0);
        assert_eq!(p.peak.cpu_pct, 5.0);
    }

    #[test]
    fn decimate_caps_at_1024() {
        // 6 h of 1 Hz points, starting off a bucket boundary.
        let from = 7 * SEC + 300_000_000;
        let to = from + 6 * 3600 * SEC;
        let pts = (0..=6 * 3600u64).map(|i| (from + i * SEC, (i % 97) as f32));
        let out = decimate(pts.clone(), from, to);
        assert!(out.len() <= MAX_POINTS);
        let w = bucket_width_ns(from, to);
        for p in &out {
            let vals: Vec<f32> = pts
                .clone()
                .filter(|(t, _)| t / w * w == p.t_ns)
                .map(|(_, v)| v)
                .collect();
            assert!(!vals.is_empty());
            assert_eq!(p.min, vals.iter().copied().fold(f32::INFINITY, f32::min));
            assert_eq!(
                p.max,
                vals.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            );
        }
        // A span of exactly 1,024 aligned widths still fits.
        let out = decimate((0..=1024u64).map(|i| (i * SEC, 1.0)), 0, 1024 * SEC);
        assert!(out.len() <= MAX_POINTS);
    }

    #[test]
    fn decimate_keeps_spike() {
        let to = 3600 * SEC;
        let pts = (0..=3600u64).map(|i| (i * SEC, if i == 1234 { 100.0 } else { 0.0 }));
        let out = decimate(pts, 0, to);
        let w = bucket_width_ns(0, to);
        assert!(w > SEC);
        let spike = out.iter().find(|p| p.t_ns == 1234 * SEC / w * w).unwrap();
        assert_eq!(spike.max, 100.0);
        assert_eq!(spike.min, 0.0);
        assert_eq!(out.iter().filter(|p| p.max > 0.0).count(), 1);
    }

    #[test]
    fn bucket_alignment_stable() {
        let from = 1000 * SEC;
        let to = from + 3 * 3600 * SEC;
        let w = bucket_width_ns(from, to);
        assert_eq!(w, bucket_width_ns(from + 100_000_000, to + 100_000_000));
        assert_eq!(w % SEC, 0);
        let pts = |from: u64| (0..3 * 3600u64).map(move |i| (from + i * SEC, i as f32));
        let a = decimate(pts(from), from, to);
        let b = decimate(
            pts(from + 100_000_000),
            from + 100_000_000,
            to + 100_000_000,
        );
        for p in a.iter().chain(&b) {
            assert_eq!(p.t_ns % w, 0);
        }
        let starts = |v: &[Point]| v.iter().map(|p| p.t_ns).collect::<Vec<_>>();
        // The same bucket starts, except where the shifted range ends a bucket later.
        let (sa, sb) = (starts(&a), starts(&b));
        let n = sa.len().min(sb.len());
        assert_eq!(sa[..n], sb[..n]);
    }

    #[test]
    fn nan_bucket_is_gap() {
        let to = 3600 * SEC;
        let w = bucket_width_ns(0, to);
        let pts = (0..=3600u64).map(|i| {
            let v = if i * SEC / w == 3 || i == 5 * w / SEC {
                f32::NAN
            } else {
                1.0
            };
            (i * SEC, v)
        });
        let out = decimate(pts, 0, to);
        let gap = out.iter().find(|p| p.t_ns == 3 * w).unwrap();
        assert!(gap.min.is_nan() && gap.max.is_nan());
        // A NaN next to real values is never folded in.
        let mixed = out.iter().find(|p| p.t_ns == 5 * w).unwrap();
        assert_eq!((mixed.min, mixed.max), (1.0, 1.0));
        assert_eq!(out.iter().filter(|p| p.max.is_nan()).count(), 1);
    }

    fn frame_with_cpu(cpus: &[u16]) -> Frame {
        Frame {
            t_ns: 0,
            procs: cpus
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    let mut p = hist_process(&record(i as u32, 0.0), i as MetaId);
                    p.cpu_permille = c;
                    p
                })
                .collect(),
        }
    }

    #[test]
    fn top_at_order_and_ties() {
        // k1..k4 are key(0)..key(3); key(1) < key(3).
        let frame = frame_with_cpu(&[50, 300, 120, 300]);
        let top = top_at(&frame, Metric::Cpu, 3, &DeletedKeys::default());
        let keys: Vec<_> = top.iter().map(|r| r.0).collect();
        assert_eq!(keys, vec![key(1), key(3), key(2)]);
        assert_eq!(top[0].1, 1);
        assert_eq!(top[0].2, 30.0);
        assert_eq!(top[2].2, 12.0);
    }

    #[test]
    fn deleted_keys_filtered() {
        let mut deleted = DeletedKeys::default();
        deleted.insert_all(&[key(1)]);
        assert!(deleted.contains(&key(1)));
        assert!(!deleted.contains(&key(0)));

        let frame = frame_with_cpu(&[50, 300, 120, 300]);
        let top = top_at(&frame, Metric::Cpu, 3, &deleted);
        assert_eq!(
            top.iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![key(3), key(2), key(0)]
        );

        let mut b = BlockBuilder::new(BootId(7));
        for i in 0..60u64 {
            b.on_process_sweep(i * SEC, &[record(0, 1.0), record(1, 50.0)]);
        }
        let block = b.flush().unwrap();
        let blocks = || std::iter::once(&block);
        let stats = range_stats(blocks(), 0, 59 * SEC, &deleted);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].key, key(0));
        assert!(process_series(blocks(), key(1), Metric::Cpu, 0, 59 * SEC, &deleted).is_empty());
        let series = process_series(blocks(), key(0), Metric::Cpu, 0, 59 * SEC, &deleted);
        assert_eq!(series.len(), 60);
        assert!(series.iter().all(|p| p.min == 1.0 && p.max == 1.0));
        for (_, top, other) in stack(blocks(), Metric::Cpu, 0, 59 * SEC, &deleted) {
            assert_eq!(top, [1.0, 0.0, 0.0]);
            assert_eq!(other, 0.0);
        }
    }

    fn device(idx: u16, kind: u8, parent: Option<u16>) -> HistDevice {
        HistDevice {
            idx: DeviceIdx(idx),
            kind,
            parent: parent.map(DeviceIdx),
            name: Arc::from(format!("dev{idx}")),
        }
    }

    /// A block of 60 seconds with only a catalog and system series.
    fn system_block(
        start: u64,
        catalog: Option<Vec<HistDevice>>,
        series: Vec<(u16, MetricId, f32)>,
    ) -> Block {
        Block {
            start_t_ns: start,
            gap_before: false,
            mem_total_kib: 0,
            catalog,
            new_meta: Vec::new(),
            keyframe: Vec::new(),
            deltas: Vec::new(),
            system: series
                .into_iter()
                .map(|(dev, metric, v)| {
                    let id = SeriesId {
                        device: DeviceIdx(dev),
                        metric,
                    };
                    (id, (0..60u32).map(|s| (s * 1000, v)).collect())
                })
                .collect(),
        }
    }

    #[test]
    fn system_cpu_uses_package_only() {
        let mut catalog = vec![device(0, 0, None)];
        catalog.extend((1..=4).map(|i| device(i, 1, Some(0))));
        let mut series = vec![(0, MetricId::CPU_TOTAL, 25.0)];
        series.extend((1..=4).map(|i| (i, MetricId::CPU_TOTAL, 90.0)));
        let first = system_block(0, Some(catalog), series.clone());
        // The catalog of the first block holds for the second.
        let second = system_block(60 * SEC, None, series);
        let blocks = [first, second];
        let out = system_series(blocks.iter(), Metric::Cpu, 0, 119 * SEC);
        assert_eq!(out.len(), 120);
        assert!(out.iter().all(|p| p.min == 25.0 && p.max == 25.0));
        assert_eq!(out[61].t_ns, 61 * SEC);
    }

    #[test]
    fn system_disk_skips_partitions() {
        let catalog = vec![
            device(0, 4, None),
            device(1, 4, Some(0)),
            device(2, 4, None),
            device(3, 5, None),
        ];
        let series = vec![
            (0, MetricId::DISK_READ, 100.0),
            (0, MetricId::DISK_WRITE, 10.0),
            (1, MetricId::DISK_READ, 1000.0),
            (2, MetricId::DISK_READ, f32::NAN),
            (2, MetricId::DISK_WRITE, 5.0),
            (3, MetricId::NET_RX, 7.0),
        ];
        let block = system_block(0, Some(catalog), series);
        let disk = system_series(std::iter::once(&block), Metric::Disk, 0, 59 * SEC);
        assert_eq!(disk.len(), 60);
        assert!(disk.iter().all(|p| p.max == 115.0));
        let net = system_series(std::iter::once(&block), Metric::Network, 0, 59 * SEC);
        assert!(net.iter().all(|p| p.max == 7.0));
        // A block before any catalog gives no points.
        let bare = system_block(0, None, vec![(0, MetricId::DISK_READ, 1.0)]);
        assert!(system_series(std::iter::once(&bare), Metric::Disk, 0, 59 * SEC).is_empty());
    }

    #[test]
    fn stack_top3_plus_other_sums_to_total() {
        let mut b = BlockBuilder::new(BootId(7));
        let mut blocks = Vec::new();
        for i in 0..1800u64 {
            // Six processes with changing loads; one lives only part of the time.
            let mut sweep: Vec<_> = (0..5u32)
                .map(|k| record(k, ((i + u64::from(k) * 7) % 40) as f32))
                .collect();
            if i % 300 < 120 {
                sweep.push(record(5, 33.0));
            }
            if let Some(block) = b.on_process_sweep(i * SEC, &sweep) {
                blocks.push(block);
            }
        }
        blocks.extend(b.flush());
        let to = 1799 * SEC;
        let none = DeletedKeys::default();
        let rows = stack(blocks.iter(), Metric::Cpu, 0, to, &none);
        let w = bucket_width_ns(0, to);
        assert_eq!(rows.len(), 1800usize.div_ceil((w / SEC) as usize));
        // Expected: the mean per-sweep total of all processes in each bucket.
        let mut totals: HashMap<u64, (f64, u32)> = HashMap::new();
        for_each_sweep(blocks.iter(), 0, to, |t, r| {
            let e = totals.entry(t / w * w).or_default();
            e.0 += r.live().map(|p| f64::from(Metric::Cpu.of(p))).sum::<f64>();
            e.1 += 1;
        });
        for (t, top, other) in &rows {
            assert!(top[0] >= top[1] && top[1] >= top[2]);
            let (sum, n) = totals[t];
            let got = f64::from(top.iter().sum::<f32>() + other);
            assert!(
                (got - sum / f64::from(n)).abs() < 1e-3,
                "{t}: {got} vs {sum}/{n}"
            );
        }
    }
}
