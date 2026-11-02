//! Small engine records that tests in more than one tl-ui crate build the same way.

use std::sync::Arc;

use tl_model::actions::ActionRequest;
use tl_model::command::Command;
use tl_model::details::SearchId;
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

use crate::fake_engine::FakeEngine;

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

/// The toast after a cancelled helper start.
pub const CANCELLED: &str = "Authentication cancelled";
/// The toast after a failed helper start.
pub const FAILED: &str = "Could not start the privileged helper";

/// How many commands `engine` got that `f` accepts.
pub fn sent(engine: &FakeEngine, f: fn(&Command) -> bool) -> usize {
    engine.commands().iter().filter(|c| f(c)).count()
}

/// How many `StartHelperSession` commands `engine` got.
pub fn starts_sent(engine: &FakeEngine) -> usize {
    sent(engine, |c| matches!(c, Command::StartHelperSession))
}

/// Every action request `engine` got, in order.
pub fn actions_sent(engine: &FakeEngine) -> Vec<ActionRequest> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(request) => Some(request),
            _ => None,
        })
        .collect()
}

/// Every handle search `engine` got, as the search id and the privileged flag.
pub fn searches(engine: &FakeEngine) -> Vec<(SearchId, bool)> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::HandleSearch {
                search, privileged, ..
            } => Some((search, privileged)),
            _ => None,
        })
        .collect()
}

/// The key of the sleeping system process `demo` with pid `pid`: start ticks 100, boot 0.
pub fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 100,
        boot_id: BootId(0),
        pid,
    }
}

/// A sleeping system process `demo` with pid `pid` and key [`key`].
pub fn record(pid: u32) -> ProcessRecord {
    let a = |s: &str| Arc::<str>::from(s);
    ProcessRecord {
        key: key(pid),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: a("demo"),
            name: a("demo"),
            exe: None,
            cmdline: None,
            user: a("u"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: a("demo"),
        }),
        group: GroupKind::System.key(),
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
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}
