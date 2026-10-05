//! The reducer side of the `HistoryRecorder` worker (ADR-012, ADR-004).
//!
//! Disk and compression work must not run on the reducer. `HistorySink` is the reducer's
//! `RecordSink`: each callback builds one `HistoryMsg` and pushes it into the worker's channel
//! with `try_send`, so it never blocks. With no sender, a full queue or a closed channel the
//! message is dropped and counted; the worker reports the count in its health message (E30-013).
//!
//! Each worker generation creates its own channel and stores the sender in the `HistorySlot`, so
//! no `mpsc::Receiver` moves into a restartable worker (ADR-004 factory contract).

// The worker loop (E30-013) and the session wiring (E30-A2) use these items.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use asupersync::channel::mpsc;
use tl_model::command::HistoryWhat;
use tl_model::devices::DeviceCatalog;
use tl_model::history::{HistoryRange, RecordSink};
use tl_model::ids::{ProcessKey, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::time::TimeNs;

/// Capacity of the channel each worker generation creates.
pub(crate) const HISTORY_CHANNEL_CAPACITY: usize = 512;

/// One event for the `HistoryRecorder` worker.
pub(crate) enum HistoryMsg {
    SessionStart {
        epoch: u32,
        t_ns: TimeNs,
    },
    System {
        t_ns: TimeNs,
        series: Box<[(SeriesId, Sample)]>,
    },
    Sweep {
        t_ns: TimeNs,
        records: Arc<[ProcessRecord]>,
    },
    /// `DeviceCatalog` holds its entries in an `Arc`, so the clone is cheap.
    Catalog(DeviceCatalog),
    Query {
        id: u64,
        range: HistoryRange,
        what: HistoryWhat,
    },
    Delete {
        keys: Arc<[ProcessKey]>,
    },
}

/// Each worker generation creates a fresh `mpsc::channel(512)` and stores its `Sender` here; the
/// sink reads it.
pub(crate) type HistorySlot = Arc<ArcSwapOption<mpsc::Sender<HistoryMsg>>>;

/// The reducer's `RecordSink` while history is on. A thin forwarder to the worker's channel.
pub(crate) struct HistorySink {
    slot: HistorySlot,
    dropped: u64,
}

impl HistorySink {
    pub(crate) fn new(slot: HistorySlot) -> Self {
        HistorySink { slot, dropped: 0 }
    }

    /// For `Command::HistoryQuery` and `Command::HistoryDelete` routed by the reducer (E30-A2).
    pub(crate) fn command(&mut self, msg: HistoryMsg) {
        self.send(msg);
    }

    /// Messages dropped so far: no sender, queue full or channel closed.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    fn send(&mut self, msg: HistoryMsg) {
        let sent = match self.slot.load().as_deref() {
            Some(tx) => tx.try_send(msg).is_ok(),
            None => false,
        };
        if !sent {
            self.dropped += 1;
        }
    }
}

impl RecordSink for HistorySink {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        self.send(HistoryMsg::System {
            t_ns,
            series: series.into(),
        });
    }

    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
        self.send(HistoryMsg::Sweep {
            t_ns,
            records: Arc::clone(records),
        });
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.send(HistoryMsg::Catalog(catalog.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::history::HistoryMetric;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(pid: u32, meta: &Arc<ProcessMeta>) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: meta.clone(),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
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

    fn sweep(n: u32) -> Arc<[ProcessRecord]> {
        let meta = Arc::new(ProcessMeta {
            comm: "x".into(),
            name: "x".into(),
            exe: None,
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            search_lower: "x".into(),
        });
        (1..=n).map(|pid| record(pid, &meta)).collect()
    }

    fn slot_with_channel() -> (HistorySlot, mpsc::Receiver<HistoryMsg>) {
        let (tx, rx) = mpsc::channel(HISTORY_CHANNEL_CAPACITY);
        let slot: HistorySlot = Arc::new(ArcSwapOption::from_pointee(tx));
        (slot, rx)
    }

    #[test]
    fn sink_without_sender_drops() {
        let mut sink = HistorySink::new(Arc::new(ArcSwapOption::empty()));
        sink.on_process_sweep(1, &sweep(3));
        assert_eq!(sink.dropped(), 1);
    }

    #[test]
    fn sink_never_blocks_when_full() {
        let (slot, rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(1);
        for t in 0..600 {
            sink.on_process_sweep(t, &records);
        }
        assert_eq!(rx.len(), 512);
        assert_eq!(sink.dropped(), 88);
    }

    #[test]
    fn sweep_arc_not_copied() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(2_000);
        sink.on_process_sweep(7, &records);
        match rx.try_recv() {
            Ok(HistoryMsg::Sweep { t_ns, records: got }) => {
                assert_eq!(t_ns, 7);
                assert!(Arc::ptr_eq(&got, &records));
            }
            _ => panic!("expected one Sweep message"),
        }
        assert_eq!(sink.dropped(), 0);
    }

    #[test]
    fn command_forwards_query() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let what = HistoryWhat::TopAt {
            t: 5,
            metric: HistoryMetric::Cpu,
            n: 3,
        };
        sink.command(HistoryMsg::Query {
            id: 42,
            range: HistoryRange::At(5),
            what: what.clone(),
        });
        match rx.try_recv() {
            Ok(HistoryMsg::Query {
                id,
                range,
                what: got,
            }) => {
                assert_eq!(id, 42);
                assert_eq!(range, HistoryRange::At(5));
                assert_eq!(got, what);
            }
            _ => panic!("expected one Query message"),
        }
        assert_eq!(sink.dropped(), 0);
    }
}
