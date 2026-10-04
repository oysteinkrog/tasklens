//! Process records, metadata, flags and groups (ADR-003).
//!
//! Numeric fields are fixed-size and text lives in a shared `ProcessMeta` behind an `Arc`, so a
//! 10 Hz CPU publish copies no strings (ADR-012 constraint 3).
//!
//! Privacy: `ProcessMeta` may hold user data (cmdline, exe, cgroup). Never implement `Display`
//! for these types and never log `ProcessMeta` at any level (ADR-015). `Debug` is for tests only.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{GroupKey, NodeId, ProcessKey};
use crate::validity::FieldValidity;

/// Text of one process. Shared by every record of that process until a field changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessMeta {
    /// /proc/<pid>/comm, sanitized.
    pub comm: Arc<str>,
    /// Display name: desktop entry Name, else exe basename, else comm.
    pub name: Arc<str>,
    /// readlink /proc/<pid>/exe; `None` if restricted.
    pub exe: Option<Arc<str>>,
    /// Bounded to 32 KiB, NULs to spaces, sanitized (ADR-015).
    pub cmdline: Option<Arc<str>>,
    /// User name for the uid.
    pub user: Arc<str>,
    /// cgroup v2 path relative to /sys/fs/cgroup.
    pub cgroup: Option<Arc<str>>,
    /// From the app scope or desktop entry.
    pub app_id: Option<Arc<str>>,
    /// Resolved icon file path.
    pub icon: Option<Arc<str>>,
    /// Desktop entry Comment or GenericName.
    pub description: Option<Arc<str>>,
    /// Lowercase filter text: display name, image name, cmdline, user, description, app name,
    /// unit name (R-043).
    pub search_lower: Arc<str>,
}

/// Scheduler state from /proc/<pid>/stat field 3.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ProcState {
    Running,
    Sleeping,
    DiskSleep,
    Stopped,
    Zombie,
    Idle,
    Other,
}

impl ProcState {
    /// From /proc/<pid>/stat field 3: b'R' Running, b'S' Sleeping, b'D' DiskSleep,
    /// b'T' and b't' Stopped, b'Z' Zombie, b'I' Idle, any other byte Other.
    pub const fn from_stat_char(c: u8) -> ProcState {
        match c {
            b'R' => ProcState::Running,
            b'S' => ProcState::Sleeping,
            b'D' => ProcState::DiskSleep,
            b'T' | b't' => ProcState::Stopped,
            b'Z' => ProcState::Zombie,
            b'I' => ProcState::Idle,
            _ => ProcState::Other,
        }
    }
}

bitflags::bitflags! {
    /// Per-process marks. Bit values are frozen.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
    pub struct ProcFlags: u16 {
        /// Appeared within the linger time (`linger_ms`).
        const NEW = 1;
        /// Exited within the linger time; kept for highlight, then removed.
        const EXITED = 2;
        const KERNEL_THREAD = 4;
        const OTHER_USER = 8;
        /// tasklens or its probe.
        const IS_SELF = 16;
        /// See ADR-006.
        const PROTECTED = 32;
    }
}

/// Linger time for NEW and EXITED: 2 × interval, clamped to 1_000..=3_000 ms.
pub const fn linger_ms(processes_interval_ms: u32) -> u32 {
    let twice = processes_interval_ms.saturating_mul(2);
    if twice < 1_000 {
        1_000
    } else if twice > 3_000 {
        3_000
    } else {
        twice
    }
}

/// One process in one sweep. Fixed-size apart from `meta`, which clones share.
#[derive(Clone, Debug)]
pub struct ProcessRecord {
    pub key: ProcessKey,
    pub ppid: u32,
    pub uid: u32,
    pub meta: Arc<ProcessMeta>,
    pub group: GroupKey,
    pub state: ProcState,
    pub flags: ProcFlags,
    pub nice: i8,
    pub threads: u32,
    /// Percent of whole machine capacity, 0..=100.
    pub cpu_pct: f32,
    /// RssAnon from /proc/<pid>/status; the default Memory column (D-004).
    pub rss_anon_bytes: u64,
    /// VmRSS; the "Working set total" column.
    pub rss_bytes: u64,
    /// Only for demanded keys (ADR-008).
    pub pss_bytes: u64,
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    /// Attributed TCP only (ADR-016).
    pub net_rx_bps: f32,
    pub net_tx_bps: f32,
    pub gpu_util_pct: f32,
    pub gpu_mem_bytes: u64,
    pub validity: FieldValidity,
}

/// Group classification of a process (PROC-004). Exactly these four classes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// cgroup app-*.scope or app-*.service under user@<uid>.service/app.slice.
    App { app_id: Arc<str> },
    /// Processes of the current uid not in an app scope.
    Background,
    /// Other uids, system.slice, init.scope, kernel threads.
    System,
    /// cgroup unreadable.
    Ungrouped,
}

impl GroupKind {
    /// FxHash (rustc_hash::FxHasher) of the value, as ADR-003 defines GroupKey (R-142 item 3).
    pub fn key(&self) -> GroupKey {
        let mut hasher = rustc_hash::FxHasher::default();
        self.hash(&mut hasher);
        GroupKey(hasher.finish())
    }
}

/// What a group row of the table is (R-141 item 3). GroupKind stays the four PROC-004 classes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupRowKind {
    App,
    Unit,
    KernelThreads,
    Pseudo,
}

/// Aggregated values of one group row. Each member process is counted once (no double
/// counting of descendants, ADR-009). Field meanings match ProcessRecord.
#[derive(Clone, Debug)]
pub struct GroupRow {
    /// Table identity of the row (R-141 item 3).
    pub node: NodeId,
    pub key: GroupKey,
    /// App, Unit, KernelThreads or Pseudo (R-141 item 3).
    pub kind: GroupRowKind,
    /// Classification the row sits under (App, Background, System, Ungrouped).
    pub class: GroupKind,
    /// Display name of the row.
    pub name: Arc<str>,
    /// Resolved icon file path.
    pub icon: Option<Arc<str>>,
    /// Member processes after filtering.
    pub members: u32,
    /// Processes in the cgroup (pids.current for App and Unit rows).
    pub pids: u32,
    /// Short row note, for example the pseudo-row explanation.
    pub note: Option<Arc<str>>,
    /// The value of the active Memory column for the row.
    pub memory_bytes: u64,
    /// io_read_bps + io_write_bps.
    pub disk_bps: f32,
    /// net_rx_bps + net_tx_bps.
    pub net_bps: f32,
    /// Same as gpu_util_pct.
    pub gpu_pct: f32,
    pub cpu_pct: f32,
    /// App rows: cgroup memory.stat `anon` (R-005).
    pub rss_anon_bytes: u64,
    pub rss_bytes: u64,
    pub pss_bytes: u64,
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    pub net_rx_bps: f32,
    pub net_tx_bps: f32,
    pub gpu_util_pct: f32,
    pub gpu_mem_bytes: u64,
    pub threads: u32,
    pub validity: FieldValidity,
}

/// Which expensive fields cover which keys (ADR-008). Keys not listed show WarmingUp for PSS and USS.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    /// Sorted ascending, so the UI can binary_search.
    pub pss_uss: Arc<[ProcessKey]>,
}

impl Coverage {
    pub fn has_pss_uss(&self, key: &ProcessKey) -> bool {
        self.pss_uss.binary_search(key).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::BootId;

    fn pkey(start_ticks: u64, pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    #[test]
    fn proc_state_from_stat_char() {
        let cases = [
            (b'R', ProcState::Running),
            (b'S', ProcState::Sleeping),
            (b'D', ProcState::DiskSleep),
            (b'T', ProcState::Stopped),
            (b't', ProcState::Stopped),
            (b'Z', ProcState::Zombie),
            (b'I', ProcState::Idle),
            (b'X', ProcState::Other),
        ];
        for (c, want) in cases {
            assert_eq!(ProcState::from_stat_char(c), want, "{}", c as char);
        }
    }

    #[test]
    fn linger_clamp() {
        assert_eq!(linger_ms(250), 1_000);
        assert_eq!(linger_ms(1_000), 2_000);
        assert_eq!(linger_ms(2_000), 3_000);
        assert_eq!(linger_ms(10_000), 3_000);
        assert_eq!(linger_ms(u32::MAX), 3_000);
    }

    #[test]
    fn proc_flags_bits_frozen() {
        assert_eq!(ProcFlags::NEW.bits(), 1);
        assert_eq!(ProcFlags::EXITED.bits(), 2);
        assert_eq!(ProcFlags::KERNEL_THREAD.bits(), 4);
        assert_eq!(ProcFlags::OTHER_USER.bits(), 8);
        assert_eq!(ProcFlags::IS_SELF.bits(), 16);
        assert_eq!(ProcFlags::PROTECTED.bits(), 32);
    }

    #[test]
    fn coverage_lookup() {
        let keys: Arc<[ProcessKey]> = vec![pkey(10, 1), pkey(20, 2), pkey(30, 3)].into();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        let coverage = Coverage { pss_uss: keys };
        assert!(coverage.has_pss_uss(&pkey(20, 2)));
        assert!(!coverage.has_pss_uss(&pkey(25, 2)));
        assert!(!Coverage::default().has_pss_uss(&pkey(10, 1)));
    }

    #[test]
    fn record_clone_shares_meta() {
        let meta = Arc::new(ProcessMeta {
            comm: "bash".into(),
            name: "bash".into(),
            exe: Some("/usr/bin/bash".into()),
            cmdline: Some("bash -l".into()),
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            search_lower: "bash bash -l user".into(),
        });
        let a = ProcessRecord {
            key: pkey(10, 1),
            ppid: 0,
            uid: 1000,
            meta,
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::NEW,
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
            validity: FieldValidity::default(),
        };
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.meta, &b.meta));
    }

    #[test]
    fn group_kind_key_stable() {
        let a = GroupKind::App {
            app_id: "firefox".into(),
        };
        let b = GroupKind::App {
            app_id: "firefox".into(),
        };
        assert_eq!(a.key(), b.key());
        assert_ne!(GroupKind::Background.key(), a.key());
        assert_ne!(GroupKind::Background.key(), b.key());
    }
}
