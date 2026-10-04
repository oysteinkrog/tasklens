//! History read side (ADR-012 "Reading"): process state at one second and range averages.
//!
//! The state at time T is the block keyframe plus every delta with `start + dt <= T`.

use std::collections::HashMap;

use tl_model::ids::ProcessKey;
use tl_model::time::TimeNs;

use crate::format::{BLOCK_SECS, Block, HistMeta, HistProcess, MetaId};

const NS_PER_MS: u64 = 1_000_000;
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
) -> Vec<RangeStats> {
    let mut index: HashMap<ProcessKey, usize> = HashMap::new();
    let mut accs: Vec<Acc> = Vec::new();
    let mut add_state = |r: &Replay| {
        for p in r.live() {
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
    };
    for b in blocks {
        if b.start_t_ns > to || b.start_t_ns.saturating_add(BLOCK_NS) <= from {
            continue;
        }
        let mut r = Replay::new(b);
        if (from..=to).contains(&b.start_t_ns) {
            add_state(&r);
        }
        while let Some(t) = r.step() {
            if t > to {
                break;
            }
            if t >= from {
                add_state(&r);
            }
        }
    }
    accs.into_iter().map(Acc::finish).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockBuilder;
    use crate::format::hist_process;
    use std::sync::Arc;
    use tl_model::ids::GroupKey;
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

        let stats = range_stats(blocks.iter(), 0, 59 * SEC);
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
        let stats = range_stats(blocks.iter(), 25 * SEC, 34 * SEC);
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
        let stats = range_stats(std::iter::once(&block), 0, 59 * SEC);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].samples, 60);
        let p = &stats[1];
        assert_eq!(p.key, key(1));
        assert_eq!(p.samples, 5);
        // CPU 1..=5 %: mean 3.
        assert_eq!(p.avg.cpu_pct, 3.0);
        assert_eq!(p.peak.cpu_pct, 5.0);
    }
}
