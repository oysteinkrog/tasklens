//! Store-side `RecordSink` (ADR-012, ADR-004, PROC-086).
//!
//! The recorder builds blocks and queues each closed one for the engine, which writes it on the
//! blocking pool. It never touches the disk. The queue holds at most `MAX_QUEUED` blocks; when
//! the writer falls behind, the oldest block is dropped and counted.

use std::collections::VecDeque;
use std::sync::Arc;

use tl_model::devices::DeviceCatalog;
use tl_model::history::RecordSink;
use tl_model::ids::SeriesId;
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::time::{BootId, TimeNs};

use crate::block::BlockBuilder;
use crate::format::Block;

/// Closed blocks queued for the writer, at most about 8 MB.
pub const MAX_QUEUED: usize = 4;

/// A closed block and the clock offset to write into its segment header.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedBlock {
    pub block: Block,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME when the block closed.
    pub realtime_offset_ns: i64,
}

/// Builds blocks while recording is on and hands closed ones to the engine.
pub struct Recorder {
    builder: BlockBuilder,
    out: VecDeque<ClosedBlock>,
    max_queued: usize,
    dropped: u64,
    realtime_offset_ns: i64,
}

impl Recorder {
    pub fn new(boot: BootId) -> Self {
        Recorder {
            builder: BlockBuilder::new(boot),
            out: VecDeque::with_capacity(MAX_QUEUED),
            max_queued: MAX_QUEUED,
            dropped: 0,
            realtime_offset_ns: 0,
        }
    }

    /// Suspend, resume or engine epoch change: closes the open block, the next one starts with
    /// `gap_before`.
    pub fn mark_gap(&mut self, t_ns: TimeNs) {
        let closed = self.builder.mark_gap(t_ns);
        self.push(closed);
    }

    /// MemTotal for blocks opened from now on.
    pub fn set_mem_total_kib(&mut self, kib: u64) {
        self.builder.set_mem_total_kib(kib);
    }

    /// CLOCK_REALTIME minus CLOCK_BOOTTIME, computed by the engine. Blocks closed from now on
    /// carry it.
    pub fn set_realtime_offset_ns(&mut self, v: i64) {
        self.realtime_offset_ns = v;
    }

    /// Called by the engine when the store says the next block starts a new segment.
    ///
    /// Takes effect when the next block opens; the block open now is unchanged.
    pub fn reset_meta_scope(&mut self) {
        self.builder.reset_meta_scope();
    }

    /// The oldest queued closed block.
    pub fn take_closed(&mut self) -> Option<ClosedBlock> {
        self.out.pop_front()
    }

    /// Closes the open block now (shutdown, mode switch) and returns it without queueing.
    /// Drain `take_closed` first to keep blocks in time order.
    pub fn flush(&mut self) -> Option<ClosedBlock> {
        let block = self.builder.flush()?;
        Some(ClosedBlock {
            block,
            realtime_offset_ns: self.realtime_offset_ns,
        })
    }

    /// A copy of the open block, for queries that reach into the last 60 s.
    pub fn open_snapshot(&self) -> Option<Block> {
        self.builder.open_snapshot()
    }

    /// Closed blocks dropped because the queue was full (the writer is stuck).
    pub fn dropped_blocks(&self) -> u64 {
        self.dropped
    }

    fn push(&mut self, closed: Option<Block>) {
        let Some(block) = closed else {
            return;
        };
        if self.out.len() >= self.max_queued {
            self.out.pop_front();
            self.dropped += 1;
        }
        self.out.push_back(ClosedBlock {
            block,
            realtime_offset_ns: self.realtime_offset_ns,
        });
    }
}

impl RecordSink for Recorder {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        let closed = self.builder.on_system_sample(t_ns, series);
        self.push(closed);
    }

    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
        let closed = self.builder.on_process_sweep(t_ns, records);
        self.push(closed);
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.builder.on_device_catalog(catalog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::{DeviceEntry, DeviceIdentity};
    use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::validity::{FieldValidity, Validity};

    use crate::format::MetaId;

    const SEC: u64 = 1_000_000_000;

    fn record(i: u32) -> ProcessRecord {
        let text: Arc<str> = Arc::from(format!("proc{i}"));
        ProcessRecord {
            key: ProcessKey {
                start_ticks: 1000 + u64::from(i),
                boot_id: BootId(7),
                pid: 100 + i,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text.clone(),
                name: text.clone(),
                exe: Some(Arc::from(format!("/usr/bin/proc{i}"))),
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
            cpu_pct: 5.0,
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

    fn sweep(ids: impl Iterator<Item = u32>) -> Arc<[ProcessRecord]> {
        ids.map(record).collect::<Vec<_>>().into()
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
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

    fn drain(r: &mut Recorder) -> Vec<ClosedBlock> {
        std::iter::from_fn(|| r.take_closed()).collect()
    }

    #[test]
    fn closed_blocks_in_order() {
        let mut r = Recorder::new(BootId(7));
        r.set_realtime_offset_ns(1_000);
        let records = sweep(0..3);
        for s in 0..=130u64 {
            r.on_process_sweep(s * SEC, &records);
        }
        let closed = drain(&mut r);
        let starts: Vec<u64> = closed.iter().map(|c| c.block.start_t_ns).collect();
        assert_eq!(starts, vec![0, 60 * SEC]);
        assert!(closed.iter().all(|c| c.realtime_offset_ns == 1_000));
        assert_eq!(r.dropped_blocks(), 0);
        assert_eq!(r.open_snapshot().unwrap().start_t_ns, 120 * SEC);
    }

    #[test]
    fn queue_bounded_drops_oldest() {
        let mut r = Recorder::new(BootId(7));
        let records = sweep(0..2);
        // Sweeps at 0, 60, ..., 360 s close six blocks.
        for m in 0..=6u64 {
            r.on_process_sweep(m * 60 * SEC, &records);
        }
        assert_eq!(r.dropped_blocks(), 2);
        let starts: Vec<u64> = drain(&mut r).iter().map(|c| c.block.start_t_ns).collect();
        assert_eq!(starts, vec![120 * SEC, 180 * SEC, 240 * SEC, 300 * SEC]);
    }

    #[test]
    fn meta_scope_reset_at_next_block() {
        let mut r = Recorder::new(BootId(7));
        r.on_device_catalog(&catalog());
        r.on_process_sweep(0, &sweep(0..3));
        r.on_process_sweep(60 * SEC, &sweep(0..3));
        let first = r.take_closed().unwrap().block;
        assert!(first.catalog.is_some());

        // Block at 60 s: process 3 starts mid-block, then the reset.
        r.on_process_sweep(61 * SEC, &sweep(0..4));
        let before = r.open_snapshot().unwrap();
        r.reset_meta_scope();
        assert_eq!(r.open_snapshot().unwrap(), before);
        r.on_process_sweep(62 * SEC, &sweep(1..4));

        // Process 0 exited, so the block at 120 s starts with 1, 2 and 3.
        r.on_process_sweep(120 * SEC, &sweep(1..4));
        let second = r.take_closed().unwrap().block;
        assert_eq!(second.start_t_ns, 60 * SEC);
        assert!(second.catalog.is_none());
        let ids: Vec<MetaId> = second.new_meta.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![3]);
        let metas: Vec<MetaId> = second.keyframe.iter().map(|p| p.meta).collect();
        assert_eq!(metas, vec![0, 1, 2]);

        r.on_process_sweep(180 * SEC, &sweep(1..4));
        let third = r.take_closed().unwrap().block;
        assert_eq!(third.start_t_ns, 120 * SEC);
        assert!(third.catalog.is_some());
        let names: Vec<(MetaId, &str)> = third
            .new_meta
            .iter()
            .map(|(id, m)| (*id, &*m.comm))
            .collect();
        assert_eq!(names, vec![(0, "proc1"), (1, "proc2"), (2, "proc3")]);
        let metas: Vec<MetaId> = third.keyframe.iter().map(|p| p.meta).collect();
        assert_eq!(metas, vec![0, 1, 2]);

        // The scope holds: the block after that lists nothing new.
        r.on_process_sweep(240 * SEC, &sweep(1..4));
        let fourth = r.take_closed().unwrap().block;
        assert!(fourth.new_meta.is_empty());
        assert!(fourth.catalog.is_none());
    }

    #[test]
    fn flush_returns_open_block_once() {
        let mut r = Recorder::new(BootId(7));
        let records = sweep(0..2);
        for s in 0..=10u64 {
            r.on_process_sweep(s * SEC, &records);
        }
        let flushed = r.flush().unwrap();
        assert_eq!(flushed.block.start_t_ns, 0);
        assert_eq!(flushed.block.deltas.len(), 10);
        assert!(r.flush().is_none());
        assert!(r.take_closed().is_none());
    }
}
