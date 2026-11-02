//! Small engine records that tests in more than one tl-ui crate build the same way.

use std::sync::Arc;

use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

/// The key of the fixture process `pid`: start ticks `pid`, boot 1.
pub fn proc_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BootId(1),
        pid,
    }
}

/// A running process `proc{pid}` of user 1000 in group 0 at `cpu_pct` % CPU, with every other
/// rate and size 0 and every field valid.
pub fn proc_record(pid: u32, cpu_pct: f32) -> ProcessRecord {
    let name: Arc<str> = Arc::from(format!("proc{pid}"));
    ProcessRecord {
        key: proc_key(pid),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.clone(),
            name: name.clone(),
            exe: None,
            cmdline: None,
            user: Arc::from("user"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: name,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct,
        rss_anon_bytes: 0,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 0.,
        net_tx_bps: 0.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.,
        validity: FieldValidity::all(Validity::Valid),
    }
}
