//! Process action dialogs: Set Affinity for one target (E23-002, UI-PWN-021, PROC-067, ADR-008).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowHandle, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{ActionKind, ActionRequest, ActionTarget};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::details::AffinityInfo;
use tl_model::ids::ProcessKey;
use tl_model::settings::{Settings, UiState};
use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
use tl_model::time::BootId;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::toast::{self, ToastQueue};
use tl_ui::demand::GlobalDemand;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::windows::affinity::{self, AffinityDialog, affinity_words};

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("content")
    }
}

fn setup(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        toast::init(cx);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    engine
}

fn set(cpus: impl IntoIterator<Item = u16>) -> CpuSet {
    let mut s = CpuSet::empty();
    for c in cpus {
        s.insert(c);
    }
    s
}

/// Two L3 groups, CPUs 0-7,16-23 and 8-15,24-31, SMT pairs n and n+16. `offline` CPUs are not
/// in `online`.
fn topology(offline: &[u16]) -> CpuTopology {
    let cores: Vec<CoreInfo> = (0..16u16)
        .map(|i| CoreInfo {
            id: i,
            logical: SmallVec::from_slice(&[i, i + 16]),
            l3_group: i / 8,
            kind: CoreKind::Standard,
        })
        .collect();
    CpuTopology {
        online: set((0..32).filter(|c| !offline.contains(c))),
        cores: Arc::from(cores),
    }
}

fn key() -> ProcessKey {
    ProcessKey {
        start_ticks: 4_242,
        boot_id: BootId(7),
        pid: 4321,
    }
}

/// Two P-cores with SMT (CPUs 0-1 and 2-3) and two E-cores (CPUs 4 and 5), one L3.
fn hybrid_topology() -> CpuTopology {
    let core = |id: u16, logical: &[u16], kind| CoreInfo {
        id,
        logical: SmallVec::from_slice(logical),
        l3_group: 0,
        kind,
    };
    CpuTopology {
        online: set(0..6),
        cores: Arc::from(vec![
            core(0, &[0, 1], CoreKind::Performance),
            core(1, &[2, 3], CoreKind::Performance),
            core(2, &[4], CoreKind::Efficiency),
            core(3, &[5], CoreKind::Efficiency),
        ]),
    }
}

fn publish_topology(cx: &mut TestAppContext, engine: &FakeEngine, offline: &[u16]) {
    publish_topology_as(cx, engine, topology(offline));
}

fn publish_topology_as(cx: &mut TestAppContext, engine: &FakeEngine, topo: CpuTopology) {
    let topo = Arc::new(topo);
    engine.publish_with(|s| {
        let system = Arc::make_mut(&mut s.system);
        system.cpu_topology = topo;
        system.header.version += 1;
    });
    cx.run_until_parked();
}

fn publish_affinity(cx: &mut TestAppContext, engine: &FakeEngine, mask: CpuSet, limit: CpuSet) {
    engine.publish_with(|s| {
        let details = Arc::make_mut(&mut s.details);
        details.affinity.insert(
            key(),
            AffinityInfo {
                mask,
                cpuset_limit: limit,
            },
        );
        details.header.version += 1;
    });
    cx.run_until_parked();
}

struct Rig {
    cx: TestAppContext,
    engine: Arc<FakeEngine>,
    handle: WindowHandle<Root>,
    vcx: VisualTestContext,
    dialog: Entity<AffinityDialog>,
}

impl Rig {
    fn draw(&mut self) {
        self.vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn update(
        &mut self,
        f: impl FnOnce(&mut AffinityDialog, &mut Window, &mut Context<AffinityDialog>),
    ) {
        let dialog = self.dialog.clone();
        self.vcx
            .update(|window, cx| dialog.update(cx, |d, cx| f(d, window, cx)));
        self.vcx.run_until_parked();
        self.draw();
    }

    fn list_text(&mut self) -> String {
        let dialog = self.dialog.clone();
        self.vcx.read(|cx| dialog.read(cx).list_text(cx))
    }

    fn checked(&mut self) -> String {
        let dialog = self.dialog.clone();
        let set = self.vcx.read(|cx| dialog.read(cx).checked());
        let mut out = String::new();
        affinity::format_cpu_list(&set, &mut out);
        out
    }

    /// Replaces the field text by typing, as a user would.
    fn type_list(&mut self, text: &str) {
        let input = self
            .vcx
            .read(|cx| self.dialog.read(cx).list_input().clone());
        self.vcx
            .update(|window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
        self.vcx.simulate_keystrokes("ctrl-a backspace");
        self.vcx.simulate_input(text);
        self.vcx.run_until_parked();
        self.draw();
    }

    /// Replaces the field text in one edit, as a paste would.
    fn paste_list(&mut self, text: &str) {
        let input = self
            .vcx
            .read(|cx| self.dialog.read(cx).list_input().clone());
        let text = text.to_string();
        self.vcx
            .update(|window, cx| input.update(cx, |s, cx| s.replace_all(text, window, cx)));
        self.vcx.run_until_parked();
        self.draw();
    }

    fn is_open(&mut self) -> bool {
        let id = self.handle.window_id();
        self.cx
            .update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
    }

    fn actions_sent(&self) -> Vec<ActionRequest> {
        self.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    fn demanded(&mut self) -> Vec<ProcessKey> {
        self.cx.update(|cx| {
            let demand = cx.global::<GlobalDemand>().0.clone();
            demand.read(cx).merged().affinity_targets.to_vec()
        })
    }
}

/// Topology published, then the dialog opened for one target from a parent window with a
/// toast queue. With `mask`, the affinity payload arrives too.
fn open(cx: &mut TestAppContext, offline: &[u16], mask: Option<(CpuSet, CpuSet)>) -> Rig {
    open_on(cx, topology(offline), mask)
}

fn open_on(cx: &mut TestAppContext, topo: CpuTopology, mask: Option<(CpuSet, CpuSet)>) -> Rig {
    let engine = setup(cx);
    publish_topology_as(cx, &engine, topo);
    let parent = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host);
        Root::new(host, window, cx)
    });
    let mut pcx = VisualTestContext::from_window(parent.into(), cx);
    pcx.update(|window, cx| {
        let _ = cx.new(|cx| ToastQueue::new(window, cx, px(64.)));
    });
    pcx.run_until_parked();

    let targets: SmallVec<[ActionTarget; 1]> = smallvec![ActionTarget::Process(key())];
    let handle = cx
        .update(|cx| affinity::open(targets, "firefox (PID 4321)".into(), parent.into(), cx))
        .expect("dialog opens");
    let dialog = cx
        .update(|cx| AffinityDialog::find(handle, cx))
        .expect("dialog content");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    let mut rig = Rig {
        cx: cx.clone(),
        engine,
        handle,
        vcx,
        dialog,
    };
    rig.draw();
    if let Some((mask, limit)) = mask {
        publish_affinity(cx, &rig.engine, mask, limit);
        rig.draw();
    }
    rig
}

fn all() -> CpuSet {
    set(0..32)
}

#[gpui_kit::test]
fn affinity_grid_groups_by_l3(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], None);
    let dialog = rig.dialog.clone();
    let (labels, tiles, first) = rig.vcx.update(|_, cx| {
        let words = affinity_words(cx);
        let d = dialog.read(cx);
        let labels: Vec<String> = d
            .groups()
            .iter()
            .map(|g| words.group(g).to_string())
            .collect();
        let tiles: Vec<usize> = d.groups().iter().map(|g| g.tiles.len()).collect();
        let t = d.groups()[0].tiles[0];
        (labels, tiles, (t.top, t.bottom))
    });
    assert_eq!(labels, ["CCD 0: CPU 0-7,16-23", "CCD 1: CPU 8-15,24-31"]);
    assert_eq!(tiles, [8, 8]);
    assert_eq!(first, (0, Some(16)));

    // Loading until the payload arrives: every CPU disabled, the subtitle says so.
    let (loading, subtitle, cpu0) = rig.vcx.update(|_, cx| {
        let d = dialog.read(cx);
        let loading = d.is_loading();
        let subtitle = d.subtitle(cx).to_string();
        let cpu0 = d.cpu_view(0, cx);
        (loading, subtitle, cpu0)
    });
    assert!(loading);
    assert_eq!(subtitle, "firefox (PID 4321) - reading current affinity…");
    assert!(cpu0.disabled);
}

#[gpui_kit::test]
fn affinity_list_field_sync_both_ways(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let dialog = rig.dialog.clone();
    assert_eq!(rig.checked(), "0-3");
    assert_eq!(rig.list_text(), "0-3");
    let (all_state, subtitle) = rig
        .vcx
        .update(|_, cx| (dialog.read(cx).all_state(), dialog.read(cx).subtitle(cx)));
    assert_eq!(all_state, None, "All processors is indeterminate");
    assert_eq!(subtitle.as_ref(), "firefox (PID 4321)");

    rig.update(|d, window, cx| d.set_cpu(5, true, window, cx));
    assert_eq!(rig.list_text(), "0-3,5");

    rig.type_list("0-1,4");
    assert_eq!(rig.checked(), "0-1,4");
    assert!(!rig.vcx.read(|cx| dialog.read(cx).list_invalid()));
}

#[gpui_kit::test]
fn affinity_ok_disabled_when_empty_and_message(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let dialog = rig.dialog.clone();
    assert!(rig.vcx.read(|cx| dialog.read(cx).ok_enabled()));
    assert!(rig.vcx.debug_bounds("AffinityValidation").is_none());

    // Indeterminate: the first click checks all, the second unchecks all.
    rig.update(|d, window, cx| d.toggle_all(window, cx));
    assert_eq!(rig.checked(), "0-31");
    rig.update(|d, window, cx| d.toggle_all(window, cx));
    assert_eq!(rig.checked(), "");

    let (ok, tooltip) = rig
        .vcx
        .update(|_, cx| (dialog.read(cx).ok_enabled(), dialog.read(cx).ok_tooltip(cx)));
    assert!(!ok);
    assert_eq!(
        tooltip.as_deref(),
        Some("At least one CPU must stay enabled")
    );
    assert!(rig.vcx.debug_bounds("AffinityValidation").is_some());

    // Enter with nothing checked sends nothing and keeps the dialog open.
    rig.vcx.simulate_keystrokes("enter");
    assert!(rig.is_open());
    assert!(rig.actions_sent().is_empty());
}

#[gpui_kit::test]
fn affinity_cpuset_limited_cpus_disabled(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[31], Some((set(0..8), set(0..8))));
    let dialog = rig.dialog.clone();
    let (cpu3, cpu8, cpu31, all_state) = rig.vcx.update(|_, cx| {
        let d = dialog.read(cx);
        let views = (d.cpu_view(3, cx), d.cpu_view(8, cx), d.cpu_view(31, cx));
        (views.0, views.1, views.2, d.all_state())
    });
    assert!(cpu3.checked && !cpu3.disabled && cpu3.tooltip.is_none());
    assert!(cpu8.disabled && !cpu8.checked);
    assert_eq!(
        cpu8.tooltip.as_deref(),
        Some("Not allowed by the process's cpuset")
    );
    assert!(cpu31.disabled && !cpu31.checked);
    assert_eq!(cpu31.tooltip.as_deref(), Some("CPU is offline"));
    // Every allowed CPU is checked.
    assert_eq!(all_state, Some(true));

    // A list that names a CPU outside the cpuset is invalid.
    rig.paste_list("0-8");
    assert!(rig.vcx.read(|cx| dialog.read(cx).list_invalid()));
    assert_eq!(rig.checked(), "0-7");
}

#[gpui_kit::test]
fn affinity_invalid_list_text(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let dialog = rig.dialog.clone();
    rig.paste_list("0-x");
    let (invalid, ok) = rig
        .vcx
        .read(|cx| (dialog.read(cx).list_invalid(), dialog.read(cx).ok_enabled()));
    assert!(invalid);
    assert!(!ok);
    assert_eq!(rig.checked(), "0-3", "the grid keeps its last valid state");
    assert!(rig.vcx.debug_bounds("AffinityListHelper").is_some());

    // A valid edit clears the error.
    rig.paste_list("2");
    assert!(!rig.vcx.read(|cx| dialog.read(cx).list_invalid()));
    assert_eq!(rig.checked(), "2");
    assert!(rig.vcx.debug_bounds("AffinityListHelper").is_none());
}

#[gpui_kit::test]
fn affinity_enter_sends_one_request_and_closes(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let grid = rig.vcx.read(|cx| rig.dialog.read(cx).grid_focus().clone());
    rig.vcx.update(|window, cx| grid.focus(window, cx));
    // Space unchecks the CPU under the cursor, CPU 0.
    rig.vcx.simulate_keystrokes("space");
    assert_eq!(rig.checked(), "1-3");
    rig.vcx.simulate_keystrokes("enter");
    rig.cx.run_until_parked();

    let sent = rig.actions_sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::SetAffinity(set(1..4)));
    assert_eq!(&sent[0].targets[..], &[ActionTarget::Process(key())]);
    assert!(!sent[0].privileged);
    assert!(!rig.is_open());
}

#[gpui_kit::test]
fn affinity_shift_space_toggles_group(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let grid = rig.vcx.read(|cx| rig.dialog.read(cx).grid_focus().clone());
    rig.vcx.update(|window, cx| grid.focus(window, cx));
    rig.vcx.simulate_keystrokes("shift-space");
    assert_eq!(rig.checked(), "0-7,16-23");
    rig.vcx.simulate_keystrokes("shift-space");
    assert_eq!(rig.checked(), "");
    rig.vcx.simulate_keystrokes("down down space");
    assert_eq!(rig.checked(), "8");
}

#[gpui_kit::test]
fn affinity_demand_registered_and_removed(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    assert_eq!(rig.demanded(), [key()]);

    // Esc closes without an action.
    rig.vcx.simulate_keystrokes("escape");
    rig.cx.run_until_parked();
    assert!(!rig.is_open());
    assert!(rig.actions_sent().is_empty());
    // The test's own handle is the last one keeping the dialog alive.
    let Rig { cx, dialog, .. } = rig;
    drop(dialog);
    // The release runs when the next update flushes its effects.
    cx.update(|_| {});
    let keys = cx.update(|cx| {
        let demand = cx.global::<GlobalDemand>().0.clone();
        demand.read(cx).merged().affinity_targets.to_vec()
    });
    assert!(keys.is_empty(), "{keys:?}");
}

#[gpui_kit::test]
fn affinity_hybrid_core_icons(cx: &mut TestAppContext) {
    let mut rig = open_on(cx, hybrid_topology(), Some((set(0..6), set(0..6))));
    rig.draw();
    for mark in ['P', 'E'] {
        assert!(
            rig.vcx
                .debug_bounds(&format!("AffinityGroupIcon-{mark}"))
                .is_some(),
            "{mark} group icon"
        );
    }
}

#[gpui_kit::test]
fn affinity_cpu_labels_small_tabular(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    rig.draw();
    let cpu = rig
        .vcx
        .debug_bounds("TlCheckbox-label-sm-tnum")
        .expect("a CPU label in t_sm with tabular figures");
    // "All processors" keeps the default label at text.base.
    let base = rig
        .vcx
        .debug_bounds("TlCheckbox-label")
        .expect("All processors label");
    assert!(cpu.size.height < base.size.height, "{cpu:?} {base:?}");
    // L3 groups have no core icon.
    assert!(rig.vcx.debug_bounds("AffinityGroupIcon-P").is_none());
}

#[gpui_kit::test]
fn affinity_first_focus_all_processors(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    let dialog = rig.dialog.clone();
    let focused = rig
        .vcx
        .update(|window, cx| dialog.read(cx).all_focus().is_focused(window));
    assert!(focused, "All processors has focus");
}
