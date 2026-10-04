//! Block frame codec (ADR-012): `u32 LE length | u32 LE crc32 | zstd frame` of a postcard `Block`.
//!
//! Decoding never panics on bad input: a crashed segment is read up to the first bad frame.

use std::collections::HashMap;
use std::sync::Arc;

use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::ids::{ProcessKey, SeriesId};
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::series::{Sample, SampleFlags};
use tl_model::time::{BootId, TimeNs};

use crate::error::HistoryError;
use crate::format::{
    BLOCK_SECS, Block, FieldPatch, HistDevice, HistMeta, HistProcess, MetaId, SweepDelta,
    ZSTD_LEVEL, hist_process,
};

/// Bytes before the zstd frame: compressed length and crc32.
pub const FRAME_PREFIX_LEN: usize = 8;
/// Largest decompressed block accepted, 64 MiB. A frame claiming more is `Zstd`.
pub const MAX_BLOCK_RAW: usize = 64 << 20;

/// One encoded block. `bytes` is the full frame including the 8-byte prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedBlock {
    pub start_t_ns: u64,
    pub bytes: Vec<u8>,
}

/// Encodes `b` as postcard, compresses it with zstd level 3 and prefixes length and crc32.
pub fn encode_block(b: &Block) -> Result<EncodedBlock, HistoryError> {
    let raw = postcard::to_stdvec(b)?;
    let compressed = zstd::bulk::compress(&raw, ZSTD_LEVEL).map_err(HistoryError::Zstd)?;
    let len = u32::try_from(compressed.len())
        .map_err(|_| HistoryError::Zstd(std::io::Error::other("compressed block exceeds 4 GiB")))?;
    let mut bytes = Vec::with_capacity(FRAME_PREFIX_LEN + compressed.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&compressed).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    Ok(EncodedBlock {
        start_t_ns: b.start_t_ns,
        bytes,
    })
}

/// Decodes the frame at the start of `frame`. `offset` is the frame's position in its file and
/// only goes into errors. Returns the block and the number of bytes consumed; bytes after the
/// frame are left alone.
pub fn decode_block(frame: &[u8], offset: u64) -> Result<(Block, usize), HistoryError> {
    let Some((prefix, rest)) = frame.split_first_chunk::<FRAME_PREFIX_LEN>() else {
        return Err(HistoryError::Truncated(offset));
    };
    let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    let crc = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
    let Some(compressed) = rest.get(..len) else {
        return Err(HistoryError::Truncated(offset));
    };
    if crc32fast::hash(compressed) != crc {
        return Err(HistoryError::Crc(offset));
    }
    // Size the output from the frame header when it has one, so a small block does not
    // reserve 64 MiB.
    let capacity = match zstd::zstd_safe::get_frame_content_size(compressed) {
        Ok(Some(n)) if n > MAX_BLOCK_RAW as u64 => {
            return Err(HistoryError::Zstd(std::io::Error::other(
                "block exceeds 64 MiB",
            )));
        }
        Ok(Some(n)) => n as usize,
        _ => MAX_BLOCK_RAW,
    };
    let raw = zstd::bulk::decompress(compressed, capacity).map_err(HistoryError::Zstd)?;
    let block = postcard::from_bytes(&raw)?;
    Ok((block, FRAME_PREFIX_LEN + len))
}

const NS_PER_SEC: u64 = 1_000_000_000;
const NS_PER_MS: u64 = 1_000_000;
const BLOCK_NS: u64 = BLOCK_SECS * NS_PER_SEC;

/// Last stored state of one live process in the open block.
struct Live {
    /// Index in the block's process list (keyframe, then `started` in order).
    index: u32,
    stored: HistProcess,
    /// Sweep number that last saw the process; older means it exited.
    seen: u32,
}

/// The block being built.
struct OpenBlock {
    block: Block,
    /// False until the first sweep of the block has filled the keyframe.
    has_keyframe: bool,
    next_index: u32,
    /// Per series: (boottime second, value), in the order the seconds arrived.
    system: HashMap<SeriesId, Vec<(u64, f32)>>,
}

/// Turns process sweeps and system samples into 60 s blocks (ADR-012).
///
/// A block covers `[start, start + 60 s)`, where `start` is the time of its first event. The
/// first sweep of a block is its keyframe; later sweeps store only what crossed a `FieldPatch`
/// threshold against the last stored value. System series keep one point per boottime second.
pub struct BlockBuilder {
    boot: BootId,
    open: Option<OpenBlock>,
    /// Last stored value of each live process in the open block.
    live: HashMap<ProcessKey, Live>,
    /// Meta id and text of every process seen in this scope that is still referenced.
    metas: HashMap<ProcessKey, (MetaId, HistMeta)>,
    next_meta: MetaId,
    sweep: u32,
    mem_total_kib: u64,
    catalog: Option<Vec<HistDevice>>,
    catalog_version: Option<u64>,
    /// The next closed block carries the catalog.
    catalog_due: bool,
    /// The next opened block gets `gap_before = true`.
    gap_pending: bool,
}

impl BlockBuilder {
    pub fn new(boot: BootId) -> Self {
        BlockBuilder {
            boot,
            open: None,
            live: HashMap::new(),
            metas: HashMap::new(),
            next_meta: 0,
            sweep: 0,
            mem_total_kib: 0,
            catalog: None,
            catalog_version: None,
            catalog_due: true,
            gap_pending: false,
        }
    }

    /// The boot this builder records.
    pub fn boot(&self) -> BootId {
        self.boot
    }

    /// MetaId counter restarts at 0 (store: new segment).
    ///
    /// If a block is open, its processes are renumbered from 0 and its `new_meta` lists all of
    /// them, so that block can be the first of the new segment. The next closed block carries
    /// the catalog.
    pub fn reset_meta_scope(&mut self) {
        let old = std::mem::take(&mut self.metas);
        self.next_meta = 0;
        self.catalog_due = true;
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.block.new_meta.clear();
        let started = open
            .block
            .deltas
            .iter_mut()
            .flat_map(|d| d.started.iter_mut());
        for p in open.block.keyframe.iter_mut().chain(started) {
            if let Some((id, _)) = self.metas.get(&p.key) {
                p.meta = *id;
                continue;
            }
            let Some((_, meta)) = old.get(&p.key) else {
                continue;
            };
            let id = self.next_meta;
            self.next_meta += 1;
            open.block.new_meta.push((id, meta.clone()));
            self.metas.insert(p.key, (id, meta.clone()));
            p.meta = id;
        }
        for (key, live) in self.live.iter_mut() {
            if let Some((id, _)) = self.metas.get(key) {
                live.stored.meta = *id;
            }
        }
    }

    pub fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        if self.catalog_version == Some(catalog.version) {
            return;
        }
        self.catalog_version = Some(catalog.version);
        self.catalog = Some(
            catalog
                .entries
                .iter()
                .map(|e| HistDevice {
                    idx: e.idx,
                    kind: device_kind(&e.identity),
                    parent: e.parent,
                    name: e.display_name.clone(),
                })
                .collect(),
        );
        self.catalog_due = true;
    }

    /// MemTotal for blocks opened from now on.
    pub fn set_mem_total_kib(&mut self, kib: u64) {
        self.mem_total_kib = kib;
    }

    /// Returns a closed block when this call crossed the block boundary.
    pub fn on_process_sweep(&mut self, t_ns: TimeNs, records: &[ProcessRecord]) -> Option<Block> {
        let closed = self.advance(t_ns);
        self.sweep = self.sweep.wrapping_add(1);
        let sweep = self.sweep;
        let Self {
            open,
            live,
            metas,
            next_meta,
            ..
        } = self;
        let open = open.as_mut().expect("advance opens a block");
        let mut meta_of = |rec: &ProcessRecord, new_meta: &mut Vec<(MetaId, HistMeta)>| {
            if let Some((id, _)) = metas.get(&rec.key) {
                return *id;
            }
            let id = *next_meta;
            *next_meta += 1;
            let meta = hist_meta(rec);
            new_meta.push((id, meta.clone()));
            metas.insert(rec.key, (id, meta));
            id
        };

        if !open.has_keyframe {
            open.has_keyframe = true;
            live.clear();
            for rec in records.iter().filter(|r| is_live(r)) {
                if live.contains_key(&rec.key) {
                    continue;
                }
                let p = hist_process(rec, meta_of(rec, &mut open.block.new_meta));
                live.insert(
                    rec.key,
                    Live {
                        index: open.next_index,
                        stored: p.clone(),
                        seen: sweep,
                    },
                );
                open.next_index += 1;
                open.block.keyframe.push(p);
            }
            // Keys missing from the keyframe are gone for good: ProcessKey is never reused.
            metas.retain(|k, _| live.contains_key(k));
            return closed;
        }

        let mut delta = SweepDelta {
            dt_ms: ms_since(open.block.start_t_ns, t_ns),
            changed: Vec::new(),
            started: Vec::new(),
            exited: Vec::new(),
        };
        for rec in records.iter().filter(|r| is_live(r)) {
            match live.get_mut(&rec.key) {
                Some(l) if l.seen == sweep => {}
                Some(l) => {
                    l.seen = sweep;
                    let cur = hist_process(rec, l.stored.meta);
                    if let Some(patch) = FieldPatch::diff(&l.stored, &cur) {
                        patch.apply(&mut l.stored);
                        delta.changed.push((l.index, patch));
                    }
                }
                None => {
                    let p = hist_process(rec, meta_of(rec, &mut open.block.new_meta));
                    live.insert(
                        rec.key,
                        Live {
                            index: open.next_index,
                            stored: p.clone(),
                            seen: sweep,
                        },
                    );
                    open.next_index += 1;
                    delta.started.push(p);
                }
            }
        }
        live.retain(|_, l| {
            let alive = l.seen == sweep;
            if !alive {
                delta.exited.push(l.index);
            }
            alive
        });
        delta.exited.sort_unstable();
        open.block.deltas.push(delta);
        closed
    }

    /// Returns a closed block when this call crossed the block boundary.
    pub fn on_system_sample(
        &mut self,
        t_ns: TimeNs,
        series: &[(SeriesId, Sample)],
    ) -> Option<Block> {
        let closed = self.advance(t_ns);
        let open = self.open.as_mut().expect("advance opens a block");
        for (id, s) in series {
            let sec = s.t_ns / NS_PER_SEC;
            let gap =
                !s.flags.contains(SampleFlags::VALID) || s.flags.contains(SampleFlags::GAP_BEFORE);
            let value = if gap { f32::NAN } else { s.value };
            let points = open.system.entry(*id).or_default();
            match points.iter_mut().rev().find(|(s, _)| *s == sec) {
                // NaN wins: the second holds a gap.
                Some((_, v)) if !v.is_nan() => *v = if gap { f32::NAN } else { v.max(value) },
                Some(_) => {}
                None => points.push((sec, value)),
            }
        }
        closed
    }

    /// Gap: suspend, resume or engine epoch change. Closes the open block.
    pub fn mark_gap(&mut self, _t_ns: TimeNs) -> Option<Block> {
        self.gap_pending = true;
        self.close()
    }

    /// Closes the open block now (shutdown, mode switch).
    pub fn flush(&mut self) -> Option<Block> {
        self.close()
    }

    /// Rough heap size of the open block in bytes, before compression.
    pub fn open_bytes_estimate(&self) -> usize {
        use std::mem::size_of;
        let Some(open) = &self.open else {
            return 0;
        };
        let b = &open.block;
        let metas: usize = b
            .new_meta
            .iter()
            .map(|(_, m)| {
                size_of::<(MetaId, HistMeta)>()
                    + m.comm.len()
                    + m.name.len()
                    + m.user.len()
                    + m.exe_basename.as_ref().map_or(0, |s| s.len())
                    + m.app_id.as_ref().map_or(0, |s| s.len())
                    + m.exe_path.as_ref().map_or(0, |s| s.len())
            })
            .sum();
        let deltas: usize = b
            .deltas
            .iter()
            .map(|d| {
                size_of::<SweepDelta>()
                    + d.changed.len() * size_of::<(u32, FieldPatch)>()
                    + d.started.len() * size_of::<HistProcess>()
                    + d.exited.len() * size_of::<u32>()
            })
            .sum();
        let system: usize = open
            .system
            .values()
            .map(|v| size_of::<(SeriesId, Vec<(u32, f32)>)>() + v.len() * size_of::<(u32, f32)>())
            .sum();
        size_of::<Block>() + metas + b.keyframe.len() * size_of::<HistProcess>() + deltas + system
    }

    /// Closes the open block when `t_ns` is past its end, then opens one if none is open.
    fn advance(&mut self, t_ns: TimeNs) -> Option<Block> {
        let closed = match &self.open {
            Some(o) if t_ns >= o.block.start_t_ns.saturating_add(BLOCK_NS) => self.close(),
            _ => None,
        };
        if self.open.is_none() {
            self.open = Some(OpenBlock {
                block: Block {
                    start_t_ns: t_ns,
                    gap_before: std::mem::take(&mut self.gap_pending),
                    mem_total_kib: self.mem_total_kib,
                    catalog: None,
                    new_meta: Vec::new(),
                    keyframe: Vec::new(),
                    deltas: Vec::new(),
                    system: Vec::new(),
                },
                has_keyframe: false,
                next_index: 0,
                system: HashMap::new(),
            });
        }
        closed
    }

    fn close(&mut self) -> Option<Block> {
        let open = self.open.take()?;
        let mut block = open.block;
        let start = block.start_t_ns;
        let mut system: Vec<_> = open
            .system
            .into_iter()
            .map(|(id, mut points)| {
                points.sort_unstable_by_key(|(sec, _)| *sec);
                let points = points
                    .into_iter()
                    .map(|(sec, v)| (ms_since(start, sec * NS_PER_SEC), v))
                    .collect();
                (id, points)
            })
            .collect();
        system.sort_unstable_by_key(|(id, _)| *id);
        block.system = system;
        if self.catalog_due && self.catalog.is_some() {
            self.catalog_due = false;
            block.catalog = self.catalog.clone();
        }
        Some(block)
    }
}

/// Milliseconds from `start` to `t_ns`, 0 when `t_ns` is earlier.
fn ms_since(start: TimeNs, t_ns: TimeNs) -> u32 {
    u32::try_from(t_ns.saturating_sub(start) / NS_PER_MS).unwrap_or(u32::MAX)
}

/// A record with `EXITED` is gone and not stored.
fn is_live(rec: &ProcessRecord) -> bool {
    !rec.flags.contains(ProcFlags::EXITED)
}

/// The text of a process. Never reads `cmdline` (ADR-015).
fn hist_meta(rec: &ProcessRecord) -> HistMeta {
    let m = &rec.meta;
    let exe_basename = m
        .exe
        .as_deref()
        .and_then(|e| e.rsplit('/').next())
        .filter(|b| !b.is_empty())
        .map(Arc::from);
    HistMeta {
        comm: m.comm.clone(),
        name: m.name.clone(),
        exe_basename,
        app_id: m.app_id.clone(),
        exe_path: m.exe.clone(),
        user: m.user.clone(),
        uid: rec.uid,
        // The group kind is not reachable from the record (R-169 OI-2).
        system: rec
            .flags
            .intersects(ProcFlags::KERNEL_THREAD | ProcFlags::OTHER_USER),
    }
}

/// `DeviceIdentity` variant order: 0 CpuPackage .. 8 Battery, 9 Aggregate.
fn device_kind(identity: &DeviceIdentity) -> u8 {
    match identity {
        DeviceIdentity::CpuPackage => 0,
        DeviceIdentity::CpuLogical { .. } => 1,
        DeviceIdentity::Memory => 2,
        DeviceIdentity::Gpu { .. } => 3,
        DeviceIdentity::Disk { .. } => 4,
        DeviceIdentity::Net { .. } => 5,
        DeviceIdentity::Sensor { .. } => 6,
        DeviceIdentity::Fan { .. } => 7,
        DeviceIdentity::Battery { .. } => 8,
        DeviceIdentity::Aggregate(_) => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::DeviceEntry;
    use tl_model::ids::{DeviceIdx, GroupKey, MetricId};
    use tl_model::process::{ProcState, ProcessMeta};
    use tl_model::validity::{FieldValidity, Validity};

    const SEC: u64 = NS_PER_SEC;

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
                exe: Some(Arc::from(format!("/usr/bin/proc{i}"))),
                cmdline: Some(Arc::from("proc --password hunter2")),
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
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn sweep(n: u32) -> Vec<ProcessRecord> {
        (0..n).map(|i| record(i, 5.0)).collect()
    }

    fn cpu() -> SeriesId {
        SeriesId {
            device: DeviceIdx(0),
            metric: MetricId::CPU_TOTAL,
        }
    }

    fn catalog(version: u64) -> DeviceCatalog {
        DeviceCatalog {
            version,
            entries: Arc::from(vec![DeviceEntry {
                idx: DeviceIdx(0),
                identity: DeviceIdentity::Battery {
                    native_path: Arc::from("BAT0"),
                },
                generation: 0,
                display_name: Arc::from("Battery"),
                present: true,
                parent: None,
                persistent_key: Arc::from("battery:BAT0"),
                net_kind: None,
            }]),
        }
    }

    #[test]
    fn steady_state_has_empty_deltas() {
        let mut b = BlockBuilder::new(BootId(7));
        let records = sweep(10);
        let mut closed = Vec::new();
        for i in 0..70u64 {
            if let Some(block) = b.on_process_sweep(i * SEC, &records) {
                closed.push((i, block));
            }
        }
        assert_eq!(closed.len(), 1);
        let (at, block) = &closed[0];
        // Sweep 61 (t = 60 s) closes the first block.
        assert_eq!(*at, 60);
        assert_eq!(block.start_t_ns, 0);
        assert_eq!(block.keyframe.len(), 10);
        assert_eq!(block.deltas.len(), 59);
        for (n, d) in block.deltas.iter().enumerate() {
            assert_eq!(d.dt_ms, (n as u32 + 1) * 1000);
            assert!(d.changed.is_empty() && d.started.is_empty() && d.exited.is_empty());
        }
        // The next block starts with sweep 61 as its keyframe.
        let next = b.flush().unwrap();
        assert_eq!(next.start_t_ns, 60 * SEC);
        assert_eq!(next.keyframe.len(), 10);
        assert_eq!(next.deltas.len(), 9);
        assert!(next.new_meta.is_empty());
    }

    #[test]
    fn sub_threshold_change_not_stored() {
        let mut b = BlockBuilder::new(BootId(7));
        b.on_process_sweep(0, &[record(0, 10.0)]);
        // 0.4 permille rounds to the same stored value.
        b.on_process_sweep(SEC, &[record(0, 10.04)]);
        // 0.8 permille against the stored 100 crosses 1 permille after rounding.
        b.on_process_sweep(2 * SEC, &[record(0, 10.08)]);
        let block = b.flush().unwrap();
        assert!(block.deltas[0].changed.is_empty());
        assert_eq!(block.deltas[1].changed.len(), 1);
        assert_eq!(block.deltas[1].changed[0].0, 0);
        assert_eq!(block.deltas[1].changed[0].1.cpu_permille, Some(101));
        assert_eq!(block.deltas[1].changed[0].1.mem_anon_kib, None);
    }

    #[test]
    fn started_and_exited_indexes() {
        let mut b = BlockBuilder::new(BootId(7));
        let base = sweep(10);
        let mut with_new = base.clone();
        with_new.push(record(10, 5.0));
        let mut vanished = with_new.clone();
        vanished.remove(4);
        // An EXITED record counts as gone.
        let mut exited_flag = vanished.clone();
        exited_flag[0].flags = ProcFlags::EXITED;

        b.on_process_sweep(0, &base);
        b.on_process_sweep(SEC, &base);
        b.on_process_sweep(2 * SEC, &with_new);
        b.on_process_sweep(3 * SEC, &vanished);
        b.on_process_sweep(4 * SEC, &exited_flag);
        let block = b.flush().unwrap();

        assert_eq!(block.deltas.len(), 4);
        assert_eq!(block.deltas[1].started.len(), 1);
        assert_eq!(block.deltas[1].started[0].key, key(10));
        assert!(block.deltas[1].exited.is_empty());
        assert_eq!(block.deltas[2].exited, vec![4]);
        assert!(block.deltas[2].started.is_empty());
        assert_eq!(block.deltas[3].exited, vec![0]);
        assert_eq!(block.new_meta.len(), 11);
        assert_eq!(block.new_meta[10].0, block.deltas[1].started[0].meta);

        // Delta 2's started entry has index 10 (keyframe-then-started index space), so a later
        // change to it is written under index 10.
        let mut changed = exited_flag.clone();
        changed.last_mut().unwrap().cpu_pct = 50.0;
        let mut b2 = BlockBuilder::new(BootId(7));
        b2.on_process_sweep(0, &base);
        b2.on_process_sweep(SEC, &with_new);
        b2.on_process_sweep(2 * SEC, &changed);
        let block = b2.flush().unwrap();
        assert_eq!(block.deltas[1].changed.len(), 1);
        assert_eq!(block.deltas[1].changed[0].0, 10);
    }

    #[test]
    fn system_series_per_second_max() {
        let mut b = BlockBuilder::new(BootId(7));
        let start = 5 * SEC;
        for v in 1..=10u64 {
            let t = start + (v - 1) * 100_000_000;
            b.on_system_sample(t, &[(cpu(), Sample::valid(t, v as f32))]);
        }
        let t = start + SEC;
        b.on_system_sample(t, &[(cpu(), Sample::valid(t, 3.0))]);
        let block = b.flush().unwrap();
        assert_eq!(block.system.len(), 1);
        assert_eq!(block.system[0].0, cpu());
        assert_eq!(block.system[0].1, vec![(0, 10.0), (1000, 3.0)]);
    }

    #[test]
    fn gap_sample_writes_nan() {
        let mut b = BlockBuilder::new(BootId(7));
        let t0 = 3 * SEC;
        b.on_system_sample(t0, &[(cpu(), Sample::valid(t0, 4.0))]);
        let t1 = t0 + 100_000_000;
        let gap = Sample {
            t_ns: t1,
            value: 2.0,
            flags: SampleFlags::VALID | SampleFlags::GAP_BEFORE,
        };
        b.on_system_sample(t1, &[(cpu(), gap)]);
        let t2 = t0 + 200_000_000;
        b.on_system_sample(t2, &[(cpu(), Sample::valid(t2, 9.0))]);
        // A sample without VALID also marks its second.
        let t3 = t0 + SEC;
        b.on_system_sample(t3, &[(cpu(), Sample::gap(t3))]);
        let t4 = t0 + 2 * SEC;
        b.on_system_sample(t4, &[(cpu(), Sample::valid(t4, 1.0))]);
        let block = b.flush().unwrap();
        let points = &block.system[0].1;
        assert_eq!(points.len(), 3);
        assert!(points[0].1.is_nan());
        assert!(points[1].1.is_nan());
        assert_eq!(points[2], (2000, 1.0));
    }

    #[test]
    fn mark_gap_closes_and_flags_next() {
        let mut b = BlockBuilder::new(BootId(7));
        assert!(b.mark_gap(0).is_none());
        // The gap before an empty builder flags the first block too.
        b.on_process_sweep(0, &sweep(2));
        let first = b.mark_gap(SEC).unwrap();
        assert!(first.gap_before);
        assert!(b.flush().is_none());

        // The second gap flags the block after it; a flush does not.
        b.on_process_sweep(10 * SEC, &sweep(2));
        assert!(b.flush().unwrap().gap_before);
        b.on_process_sweep(15 * SEC, &sweep(2));
        assert!(!b.flush().unwrap().gap_before);

        b.on_process_sweep(20 * SEC, &sweep(2));
        let closed = b.mark_gap(21 * SEC).unwrap();
        assert!(!closed.gap_before);
        assert_eq!(closed.keyframe.len(), 2);
        b.on_process_sweep(500 * SEC, &sweep(2));
        let next = b.flush().unwrap();
        assert!(next.gap_before);
        assert_eq!(next.start_t_ns, 500 * SEC);
        // A gap restarts the keyframe; ids stay in scope.
        assert_eq!(next.keyframe.len(), 2);
        assert!(next.new_meta.is_empty());
    }

    #[test]
    fn catalog_only_when_changed() {
        let mut b = BlockBuilder::new(BootId(7));
        b.on_device_catalog(&catalog(1));
        b.reset_meta_scope();
        b.on_process_sweep(0, &sweep(1));
        let first = b.on_process_sweep(60 * SEC, &sweep(1)).unwrap();
        let devices = first.catalog.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].kind, 8);
        assert_eq!(&*devices[0].name, "Battery");

        // Same version: no catalog.
        b.on_device_catalog(&catalog(1));
        let second = b.on_process_sweep(120 * SEC, &sweep(1)).unwrap();
        assert!(second.catalog.is_none());

        b.on_device_catalog(&catalog(2));
        let third = b.on_process_sweep(180 * SEC, &sweep(1)).unwrap();
        assert!(third.catalog.is_some());
        let fourth = b.flush().unwrap();
        assert!(fourth.catalog.is_none());

        // A new scope repeats the catalog.
        b.reset_meta_scope();
        b.on_process_sweep(300 * SEC, &sweep(1));
        assert!(b.flush().unwrap().catalog.is_some());
    }

    #[test]
    fn meta_assigned_once_per_scope() {
        let mut b = BlockBuilder::new(BootId(7));
        b.set_mem_total_kib(64 << 20);
        b.on_process_sweep(0, &sweep(3));
        let first = b.on_process_sweep(60 * SEC, &sweep(3)).unwrap();
        assert_eq!(first.mem_total_kib, 64 << 20);
        let ids: Vec<MetaId> = first.new_meta.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(first.keyframe[2].meta, 2);
        assert_eq!(&*first.new_meta[1].1.comm, "proc1");
        assert_eq!(first.new_meta[1].1.exe_basename.as_deref(), Some("proc1"));
        assert_eq!(
            first.new_meta[1].1.exe_path.as_deref(),
            Some("/usr/bin/proc1")
        );
        assert!(!first.new_meta[1].1.system);

        // The open block (opened by the sweep at 60 s) reuses the ids.
        let mut more = sweep(3);
        let mut kthread = record(3, 0.0);
        kthread.flags = ProcFlags::KERNEL_THREAD;
        more.push(kthread);
        let second = b.on_process_sweep(120 * SEC, &more).unwrap();
        assert!(second.new_meta.is_empty());
        let third_open = b.on_process_sweep(121 * SEC, &more);
        assert!(third_open.is_none());

        // Reset with an open block: its processes are renumbered from 0 and listed again.
        b.reset_meta_scope();
        let third = b.flush().unwrap();
        let ids: Vec<MetaId> = third.new_meta.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
        assert!(third.new_meta[3].1.system);
        let metas: Vec<MetaId> = third.keyframe.iter().map(|p| p.meta).collect();
        assert_eq!(metas, vec![0, 1, 2, 3]);

        // After the reset the next block assigns nothing new.
        b.on_process_sweep(200 * SEC, &more);
        assert!(b.flush().unwrap().new_meta.is_empty());
        b.reset_meta_scope();
        b.on_process_sweep(300 * SEC, &more);
        let fresh = b.flush().unwrap();
        assert_eq!(fresh.new_meta.len(), 4);
        assert_eq!(fresh.new_meta[0].0, 0);
    }

    #[test]
    fn no_cmdline_in_meta() {
        let mut b = BlockBuilder::new(BootId(7));
        b.on_process_sweep(0, &sweep(2));
        let mut later = sweep(2);
        later.push(record(5, 1.0));
        b.on_process_sweep(SEC, &later);
        let block = b.flush().unwrap();
        let bytes = postcard::to_stdvec(&block).unwrap();
        let needle = b"hunter2";
        assert!(!bytes.windows(needle.len()).any(|w| w == needle));
        assert!(b.open_bytes_estimate() == 0);
    }

    #[test]
    fn open_bytes_estimate_grows() {
        let mut b = BlockBuilder::new(BootId(7));
        b.on_process_sweep(0, &sweep(10));
        let after_keyframe = b.open_bytes_estimate();
        assert!(after_keyframe > 10 * std::mem::size_of::<HistProcess>());
        let mut changed = sweep(10);
        changed[0].cpu_pct = 40.0;
        b.on_process_sweep(SEC, &changed);
        assert!(b.open_bytes_estimate() > after_keyframe);
        assert_eq!(b.boot(), BootId(7));
    }
}
