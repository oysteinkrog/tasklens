//! In-RAM history while recording is off (ADR-012 rolling window, D-010, R-103).
//!
//! The window holds the open block plus the newest `ROLLING_CLOSED_BLOCKS` closed blocks, each
//! zstd-compressed when it closes. Nothing is written to disk, deleted keys included.
//!
//! `MetaId`s are scoped to the window's lifetime and never reset. `metas` holds every meta that a
//! kept closed block or the open block references, so the one `MetaTable` that `blocks` returns
//! resolves every block. A meta stays after the block that introduced it is dropped, and is
//! pruned when no kept block and not the open block references it.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tl_model::devices::DeviceCatalog;
use tl_model::history::RecordSink;
use tl_model::ids::{ProcessKey, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::time::{BootId, TimeNs};

use crate::block::{BlockBuilder, EncodedBlock, decode_block, encode_block};
use crate::error::HistoryError;
use crate::format::{BLOCK_SECS, Block, HistMeta, MetaId};
use crate::read::{DeletedKeys, MetaTable};

/// Closed blocks kept: 10 minutes of 60 s blocks.
pub const ROLLING_CLOSED_BLOCKS: usize = 10;

const BLOCK_NS: u64 = BLOCK_SECS * 1_000_000_000;

/// One closed block and the meta ids it references.
struct Closed {
    encoded: EncodedBlock,
    refs: Vec<MetaId>,
}

/// The last 10 minutes of process history, in RAM only.
pub struct RollingWindow {
    builder: BlockBuilder,
    closed: VecDeque<Closed>,
    metas: MetaTable,
    /// Every id below this is in `metas` or was pruned.
    metas_next: MetaId,
    /// Bytes of meta text in `metas`, for `ram_bytes`.
    metas_bytes: usize,
    deleted: DeletedKeys,
    /// Start of the open block, `None` when no block is open.
    open_start: Option<TimeNs>,
    /// Time of the newest event.
    last_t: Option<TimeNs>,
    /// Closed blocks that failed to encode and were dropped.
    encode_errors: u64,
}

impl RollingWindow {
    pub fn new(boot: BootId) -> Self {
        RollingWindow {
            builder: BlockBuilder::new(boot),
            closed: VecDeque::with_capacity(ROLLING_CLOSED_BLOCKS + 1),
            metas: MetaTable::default(),
            metas_next: 0,
            metas_bytes: 0,
            deleted: DeletedKeys::default(),
            open_start: None,
            last_t: None,
            encode_errors: 0,
        }
    }

    /// Suspend, resume or engine epoch change: closes the open block, the next one starts with
    /// `gap_before`.
    pub fn mark_gap(&mut self, t_ns: TimeNs) {
        let closed = self.builder.mark_gap(t_ns);
        if let Some(block) = closed {
            self.on_closed(block);
        }
        self.open_start = None;
    }

    /// MemTotal for blocks opened from now on.
    pub fn set_mem_total_kib(&mut self, kib: u64) {
        self.builder.set_mem_total_kib(kib);
    }

    /// Start of the oldest kept block to the newest event. `None` before the first event.
    pub fn span(&self) -> Option<(TimeNs, TimeNs)> {
        let start = self
            .closed
            .front()
            .map(|c| c.encoded.start_t_ns)
            .or(self.open_start)?;
        let end = self.last_t?;
        Some((start, end.max(start)))
    }

    /// Decoded closed blocks overlapping [from, to] plus a snapshot Block of the open block.
    pub fn blocks(
        &self,
        from: TimeNs,
        to: TimeNs,
    ) -> Result<(Vec<Arc<Block>>, &MetaTable), HistoryError> {
        let hit = |start: TimeNs| start <= to && start.saturating_add(BLOCK_NS) > from;
        let mut out = Vec::new();
        for c in self.closed.iter().filter(|c| hit(c.encoded.start_t_ns)) {
            let (block, _) = decode_block(&c.encoded.bytes, 0)?;
            out.push(Arc::new(block));
        }
        if let Some(open) = self.builder.open_snapshot()
            && hit(open.start_t_ns)
        {
            out.push(Arc::new(open));
        }
        Ok((out, &self.metas))
    }

    /// "Delete from history" (R-103). The keys live in RAM and die with the window.
    pub fn delete_keys(&mut self, keys: &[ProcessKey]) {
        self.deleted.insert_all(keys);
    }

    pub fn deleted(&self) -> &DeletedKeys {
        &self.deleted
    }

    /// Closed blocks that failed to encode and were dropped.
    pub fn encode_errors(&self) -> u64 {
        self.encode_errors
    }

    /// Number of kept closed blocks, at most `ROLLING_CLOSED_BLOCKS`.
    pub fn closed_len(&self) -> usize {
        self.closed.len()
    }

    /// Rough RAM held: compressed closed blocks, the open block before compression and metas.
    pub fn ram_bytes(&self) -> usize {
        use std::mem::size_of;
        let closed: usize = self
            .closed
            .iter()
            .map(|c| {
                size_of::<Closed>()
                    + c.encoded.bytes.capacity()
                    + c.refs.capacity() * size_of::<MetaId>()
            })
            .sum();
        size_of::<Self>() + closed + self.builder.open_bytes_estimate() + self.metas_bytes
    }

    /// Notes the event time and, when `closed` is set, the block that this event closed.
    fn after_event(&mut self, t_ns: TimeNs, closed: Option<Block>) {
        self.last_t = Some(self.last_t.map_or(t_ns, |t| t.max(t_ns)));
        if let Some(block) = closed {
            self.on_closed(block);
            self.open_start = None;
        }
        if self.open_start.is_none() {
            self.open_start = Some(t_ns);
        }
        self.absorb_open_metas();
    }

    /// Adds the metas the open block assigned since the last call.
    fn absorb_open_metas(&mut self) {
        let fresh = self.builder.open_new_meta();
        let first = fresh.partition_point(|(id, _)| *id < self.metas_next);
        if first == fresh.len() {
            return;
        }
        let fresh = fresh[first..].to_vec();
        self.add_metas(fresh);
    }

    fn add_metas(&mut self, metas: Vec<(MetaId, HistMeta)>) {
        let Some(max) = metas.iter().map(|(id, _)| *id).max() else {
            return;
        };
        self.metas_next = self.metas_next.max(max.saturating_add(1));
        self.metas_bytes += metas.iter().map(|(_, m)| meta_bytes(m)).sum::<usize>();
        self.metas.absorb(&meta_block(metas));
    }

    /// Encodes and keeps a closed block, then drops the oldest blocks over the limit.
    fn on_closed(&mut self, block: Block) {
        // Normally every id was absorbed while the block was open; this keeps the table whole
        // if one was not.
        let unseen: Vec<_> = block
            .new_meta
            .iter()
            .filter(|(id, _)| *id >= self.metas_next)
            .cloned()
            .collect();
        self.add_metas(unseen);
        let refs = meta_refs(&block);
        match encode_block(&block) {
            Ok(encoded) => self.closed.push_back(Closed { encoded, refs }),
            Err(_) => self.encode_errors += 1,
        }
        let mut dropped = false;
        while self.closed.len() > ROLLING_CLOSED_BLOCKS {
            self.closed.pop_front();
            dropped = true;
        }
        if dropped {
            self.prune_metas();
        }
    }

    /// Keeps only the metas a kept block or the open block references.
    fn prune_metas(&mut self) {
        let mut keep: HashSet<MetaId> = self
            .closed
            .iter()
            .flat_map(|c| c.refs.iter().copied())
            .collect();
        if let Some(open) = self.builder.open_snapshot() {
            keep.extend(meta_refs(&open));
        }
        // Processes the next keyframe will list are in the newest kept block, so its refs
        // cover the builder's ids too.
        let mut kept: Vec<(MetaId, HistMeta)> = keep
            .into_iter()
            .filter_map(|id| self.metas.get(id).map(|m| (id, m.clone())))
            .collect();
        kept.sort_unstable_by_key(|(id, _)| *id);
        self.metas_bytes = kept.iter().map(|(_, m)| meta_bytes(m)).sum();
        self.metas = MetaTable::default();
        self.metas.absorb(&meta_block(kept));
    }
}

impl RecordSink for RollingWindow {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        let closed = self.builder.on_system_sample(t_ns, series);
        self.after_event(t_ns, closed);
    }

    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
        let closed = self.builder.on_process_sweep(t_ns, records);
        self.after_event(t_ns, closed);
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.builder.on_device_catalog(catalog);
    }
}

/// Sorted meta ids that a block's keyframe and started processes reference.
fn meta_refs(b: &Block) -> Vec<MetaId> {
    let started = b.deltas.iter().flat_map(|d| d.started.iter());
    let mut refs: Vec<MetaId> = b.keyframe.iter().chain(started).map(|p| p.meta).collect();
    refs.sort_unstable();
    refs.dedup();
    refs.shrink_to_fit();
    refs
}

/// A block that carries only `new_meta`, to fill a `MetaTable`.
fn meta_block(new_meta: Vec<(MetaId, HistMeta)>) -> Block {
    Block {
        start_t_ns: 0,
        gap_before: false,
        mem_total_kib: 0,
        catalog: None,
        new_meta,
        keyframe: Vec::new(),
        deltas: Vec::new(),
        system: Vec::new(),
    }
}

/// Rough heap size of one meta entry. Arc text shared with the engine counts in full.
fn meta_bytes(m: &HistMeta) -> usize {
    std::mem::size_of::<(MetaId, HistMeta)>()
        + m.comm.len()
        + m.name.len()
        + m.user.len()
        + m.exe_basename.as_ref().map_or(0, |s| s.len())
        + m.app_id.as_ref().map_or(0, |s| s.len())
        + m.exe_path.as_ref().map_or(0, |s| s.len())
}
