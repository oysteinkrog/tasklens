//! Helpers that several test modules used to copy word for word.

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle};
use tl_model::actions::ActionRequest;
use tl_model::command::{Command, SetupPhase, SetupRun, SetupState, SetupStep};
use tl_model::details::SearchId;
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::components::toast::ToastQueue;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

/// The toast after a cancelled helper start.
pub const CANCELLED: &str = "Authentication cancelled";
/// The toast after a failed helper start.
pub const FAILED: &str = "Could not start the privileged helper";

/// Publishes `health.helper` and `health.helper_start`, then runs until parked.
pub fn publish_helper(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
    helper: HelperState,
    start: Option<(u64, HelperStartOutcome)>,
) {
    engine.publish_with(|s| {
        let health = Arc::make_mut(&mut s.health);
        health.helper = helper;
        health.helper_start = start;
        health.version += 1;
    });
    cx.run_until_parked();
}

/// Publishes `health.setup.run` for the polkit rule step.
pub fn publish_polkit(engine: &FakeEngine, seq: u64, on: bool, phase: SetupPhase) {
    engine.publish_with(|s| {
        let mut health = (*s.health).clone();
        health.version += 1;
        health.setup = Arc::new(SetupState {
            status: None,
            run: Some(SetupRun {
                seq,
                step: SetupStep::PolkitRule { on },
                phase,
                output: Arc::from(""),
            }),
        });
        s.health = Arc::new(health);
    });
}

pub fn passwordless(cx: &mut TestAppContext) -> bool {
    cx.update(|cx| TlSettings::settings(cx).privilege.passwordless)
}

pub fn sent(engine: &FakeEngine, f: fn(&Command) -> bool) -> usize {
    engine.commands().iter().filter(|c| f(c)).count()
}

pub fn starts_sent(engine: &FakeEngine) -> usize {
    sent(engine, |c| matches!(c, Command::StartHelperSession))
}

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

pub fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 100,
        boot_id: BootId(0),
        pid,
    }
}

pub fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

pub fn window_count(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| cx.windows().len())
}

/// The visible toast texts in the window of `vcx`.
pub fn toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        ToastQueue::of_window(window, cx)
            .expect("toast queue")
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}
