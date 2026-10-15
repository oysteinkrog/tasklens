//! The `ProcessDetail` worker, part 1 (ADR-008, ADR-016): PSS, USS and swap for `detail_keys`.
//!
//! `/proc/<pid>/smaps_rollup` is read at 1 Hz, only while a PSS or USS column is shown and
//! `detail_keys` (visible rows plus one viewport of overscan, at most 512) is not empty. The
//! `tl-collect` `PssReader` keeps up to 128 fds open between samples (ADR-016), checks
//! `start_ticks` when it opens a file and closes the fds of keys that left the demand.
//! Per-key errors become `Validity`, never `WorkerError`.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use tl_collect::proc::smaps::{PssReader, RollupRead};
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::EngineConfig;
use crate::source::Source;
use crate::update::{DetailsUpdate, PssEntry, SourceUpdate};

/// PSS and USS are read once a second (ADR-008).
pub const PSS_PERIOD: Duration = Duration::from_secs(1);

/// True while a PSS or USS column is shown.
pub fn wants_pss(d: &Demand) -> bool {
    d.process_columns.contains(ColumnId::Pss) || d.process_columns.contains(ColumnId::Uss)
}

pub struct ProcessDetailSource {
    cfg: Arc<EngineConfig>,
    pss: PssReader,
    /// Set by `period`: the keys of the latest demand.
    keys: RefCell<Arc<[ProcessKey]>>,
    /// Reused every sample.
    buf: Vec<u8>,
    reads: Vec<(ProcessKey, RollupRead)>,
}

impl ProcessDetailSource {
    pub fn new(cfg: Arc<EngineConfig>) -> Self {
        Self {
            cfg,
            pss: PssReader::new(),
            keys: RefCell::new(Arc::from([])),
            buf: Vec::new(),
            reads: Vec::new(),
        }
    }

    /// Number of cached `smaps_rollup` fds (at most 128).
    pub fn cached_fds(&self) -> usize {
        self.pss.cached()
    }
}

impl Source for ProcessDetailSource {
    fn id(&self) -> SourceId {
        SourceId::ProcessDetail
    }

    fn period(&self, d: &Demand, _s: &EngineSettings) -> Option<Duration> {
        if wants_pss(d) {
            *self.keys.borrow_mut() = d.detail_keys.clone();
        }
        (wants_pss(d) && !d.detail_keys.is_empty()).then_some(PSS_PERIOD)
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        let keys = self.keys.borrow().clone();
        self.pss
            .read(&self.cfg.sys_root, &keys, &mut self.buf, &mut self.reads);
        let entries = self
            .reads
            .iter()
            .filter_map(|&(key, read)| {
                let (pss, uss, swap, validity) = match read {
                    RollupRead::Ok(r) => (r.pss, r.uss, r.swap, Validity::Valid),
                    RollupRead::Restricted => (0, 0, 0, Validity::Restricted),
                    RollupRead::Failed => (0, 0, 0, Validity::Failed),
                    RollupRead::Gone => return None,
                };
                Some(PssEntry {
                    key,
                    pss_bytes: pss,
                    uss_bytes: uss,
                    swap_bytes: swap,
                    validity,
                })
            })
            .collect();
        let update = SourceUpdate::Details(DetailsUpdate::Pss {
            t_ns: now_boot_ns(),
            entries,
        });
        (self, update)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tl_collect::SysRoot;
    use tl_model::demand::ColumnSet;
    use tl_model::time::BootId;

    use super::*;
    use crate::config::EngineMode;

    const STAT: &[u8] =
        include_bytes!("../../../tl-collect/tests/fixtures/process-edge/proc/4201/stat");
    const ROLLUP: &[u8] =
        include_bytes!("../../../tl-collect/tests/fixtures/process-edge/proc/4201/smaps_rollup");
    /// start_ticks of process-edge 4201.
    const START_TICKS: u64 = 351_220;

    fn key(pid: u32, start_ticks: u64) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    /// A proc root with `n` processes (pids 10000..) that all have the stat and smaps_rollup of
    /// process-edge 4201.
    fn fake_procs(n: u32) -> (tempfile::TempDir, Arc<EngineConfig>, Vec<ProcessKey>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut keys = Vec::new();
        for pid in 10_000..10_000 + n {
            let dir = tmp.path().join(format!("proc/{pid}"));
            fs::create_dir_all(&dir).expect("create proc dir");
            fs::write(dir.join("stat"), STAT).expect("write stat");
            fs::write(dir.join("smaps_rollup"), ROLLUP).expect("write smaps_rollup");
            keys.push(key(pid, START_TICKS));
        }
        let cfg = EngineConfig::new(EngineMode::Headless, PathBuf::from("/nonexistent"))
            .with_sys_root(SysRoot::at(tmp.path()));
        (tmp, Arc::new(cfg), keys)
    }

    fn demand(columns: &[ColumnId], keys: &[ProcessKey]) -> Demand {
        let mut set = ColumnSet::default();
        for &c in columns {
            set.insert(c);
        }
        Demand {
            process_columns: set,
            detail_keys: keys.into(),
            ..Demand::default()
        }
    }

    fn entries(update: SourceUpdate) -> Vec<PssEntry> {
        match update {
            SourceUpdate::Details(DetailsUpdate::Pss { entries, .. }) => entries,
            other => panic!("expected a Pss update, got {other:?}"),
        }
    }

    /// `period`, then `sample`, as the source loop does.
    fn run(src: ProcessDetailSource, d: &Demand) -> (ProcessDetailSource, Vec<PssEntry>) {
        src.period(d, &EngineSettings::default());
        let (src, update) = src.sample();
        (src, entries(update))
    }

    #[test]
    fn pss_only_when_column_shown() {
        let (_tmp, cfg, keys) = fake_procs(3);
        let src = ProcessDetailSource::new(cfg);
        let s = EngineSettings::default();
        assert_eq!(src.period(&demand(&[], &keys), &s), None);
        assert_eq!(
            src.period(&demand(&[ColumnId::Pss], &keys), &s),
            Some(PSS_PERIOD)
        );
        assert_eq!(
            src.period(&demand(&[ColumnId::Uss], &keys), &s),
            Some(Duration::from_secs(1))
        );
        assert_eq!(src.period(&demand(&[ColumnId::Pss], &[]), &s), None);
    }

    #[test]
    fn pss_values_from_fixture() {
        let (_tmp, cfg, keys) = fake_procs(3);
        let (_src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 3);
        for e in &out {
            assert_eq!(e.validity, Validity::Valid);
            assert_eq!(e.pss_bytes, 2_097_152);
            assert_eq!(e.uss_bytes, 1_572_864);
            assert_eq!(e.swap_bytes, 0);
        }
    }

    #[test]
    fn pss_restricted_other_user() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root");
            return;
        }
        let (tmp, cfg, keys) = fake_procs(2);
        let path = tmp.path().join("proc/10001/smaps_rollup");
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod");
        let (_src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].validity, Validity::Valid);
        assert_eq!(out[1].key, keys[1]);
        assert_eq!(out[1].validity, Validity::Restricted);
    }

    #[test]
    fn fd_cache_bounded_128() {
        let (_tmp, cfg, keys) = fake_procs(200);
        let d = demand(&[ColumnId::Pss], &keys);
        let (src, out) = run(ProcessDetailSource::new(cfg), &d);
        assert_eq!(out.len(), 200);
        assert_eq!(src.cached_fds(), 128);
        let (src, _) = run(src, &d);
        assert_eq!(src.cached_fds(), 128);
        let (src, _) = run(src, &demand(&[ColumnId::Pss], &keys[150..]));
        assert_eq!(src.cached_fds(), 50);
    }

    #[test]
    fn pss_identity_mismatch_skipped() {
        let (_tmp, cfg, keys) = fake_procs(1);
        let (src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(src.cached_fds(), 1);
        let stale = key(keys[0].pid, START_TICKS - 1);
        let (src, out) = run(src, &demand(&[ColumnId::Pss], &[stale]));
        assert!(out.is_empty());
        assert_eq!(src.cached_fds(), 0);
    }
}
