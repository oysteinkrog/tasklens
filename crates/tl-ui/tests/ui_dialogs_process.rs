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

fn publish_topology(cx: &mut TestAppContext, engine: &FakeEngine, topo: CpuTopology) {
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
    publish_topology(cx, &engine, topo);
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
    for selector in ["AffinityGroupIcon-P", "AffinityGroupIcon-E"] {
        assert!(rig.vcx.debug_bounds(selector).is_some(), "{selector}");
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

#[gpui_kit::test]
fn affinity_core_tooltip_names_core_type(cx: &mut TestAppContext) {
    let mut rig = open_on(cx, hybrid_topology(), Some((set(0..6), set(0..6))));
    let dialog = rig.dialog.clone();
    let tips = rig.vcx.update(|_, cx| {
        let words = affinity_words(cx);
        let d = dialog.read(cx);
        d.groups()
            .iter()
            .flat_map(|g| g.tiles.iter().map(|t| words.core_tooltip(t).to_string()))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        tips,
        [
            "P-core 0: CPU 0 and CPU 1",
            "P-core 1: CPU 2 and CPU 3",
            "E-core 2: CPU 4",
            "E-core 3: CPU 5",
        ]
    );
    // A CPU without hybrid cores keeps "Core".
    let words = rig.vcx.update(|_, cx| affinity_words(cx));
    let tile = affinity::CpuTile {
        core: 0,
        top: 0,
        bottom: Some(16),
        kind: CoreKind::Standard,
    };
    assert_eq!(
        words.core_tooltip(&tile).as_ref(),
        "Core 0: CPU 0 and CPU 16"
    );
}

// ---- Several targets, mixed marks and thread targets (tasklens-gma.8, UI-PWN-022) ----

fn key_of(pid: u32) -> ProcessKey {
    ProcessKey { pid, ..key() }
}

/// The dialog for `targets` with each process key's mask from `masks`, and the parent window
/// with its toast queue.
fn open_many(
    cx: &mut TestAppContext,
    targets: Vec<ActionTarget>,
    subtitle: &str,
    given: Option<Vec<CpuSet>>,
    masks: &[(ProcessKey, CpuSet)],
) -> (Rig, Entity<ToastQueue>) {
    let engine = setup(cx);
    publish_topology(cx, &engine, topology(&[]));
    let parent = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host);
        Root::new(host, window, cx)
    });
    let mut pcx = VisualTestContext::from_window(parent.into(), cx);
    let queue = pcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
    pcx.run_until_parked();
    let subtitle = subtitle.to_owned();
    let handle = cx
        .update(|cx| {
            affinity::open_with_masks(
                targets.into_iter().collect(),
                subtitle.into(),
                given,
                parent.into(),
                cx,
            )
        })
        .expect("dialog opens");
    let dialog = cx
        .update(|cx| AffinityDialog::find(handle, cx))
        .expect("dialog content");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    let masks = masks.to_vec();
    engine.publish_with(|s| {
        let details = Arc::make_mut(&mut s.details);
        for (key, mask) in masks {
            details.affinity.insert(
                key,
                AffinityInfo {
                    mask,
                    cpuset_limit: all(),
                },
            );
        }
        details.header.version += 1;
    });
    cx.run_until_parked();
    let mut rig = Rig {
        cx: cx.clone(),
        engine,
        handle,
        vcx,
        dialog,
    };
    rig.draw();
    (rig, queue)
}

/// Three processes with masks {0,1}, {0,2} and {0}.
fn three(cx: &mut TestAppContext) -> (Rig, Entity<ToastQueue>) {
    let masks = [
        (key_of(1), set([0, 1])),
        (key_of(2), set([0, 2])),
        (key_of(3), set([0])),
    ];
    let targets = masks
        .iter()
        .map(|(k, _)| ActionTarget::Process(*k))
        .collect();
    open_many(cx, targets, "3 processes", None, &masks)
}

fn mark(rig: &mut Rig, cpu: u16) -> u8 {
    rig.vcx.read(|cx| rig.dialog.read(cx).state().get(cpu))
}

fn click_cpu(rig: &mut Rig, cpu: u16) {
    rig.draw();
    let at = rig
        .vcx
        .debug_bounds(Box::leak(
            format!("TlCheckbox-AffinityCpu-{cpu}").into_boxed_str(),
        ))
        .map(|b| b.center());
    match at {
        Some(at) => {
            rig.vcx.simulate_click(at, Default::default());
            rig.vcx.run_until_parked();
            rig.draw();
        }
        None => rig.update(|d, window, cx| d.cycle_cpu(cpu, window, cx)),
    }
}

#[gpui_kit::test]
fn affinity_mixed_initial_marks(cx: &mut TestAppContext) {
    let (mut rig, _parent) = three(cx);
    assert_eq!(mark(&mut rig, 0), affinity::ON);
    assert_eq!(mark(&mut rig, 1), affinity::MIXED);
    assert_eq!(mark(&mut rig, 2), affinity::MIXED);
    assert!((3..32).all(|c| mark(&mut rig, c) == affinity::OFF));
    // The field lists only ON CPUs; the mixed helper shows; the MIXED box has its tooltip.
    assert_eq!(rig.list_text(), "0");
    assert!(rig.vcx.debug_bounds("AffinityMixedHelper").is_some());
    let view = rig.vcx.read(|cx| rig.dialog.read(cx).cpu_view(1, cx));
    assert!(view.mixed);
    assert_eq!(
        view.tooltip.as_deref(),
        Some("Set for some of the selected processes")
    );
    assert!(
        rig.vcx.debug_bounds("TlButton-AffinityReset").is_some(),
        "Reset shows"
    );
    assert_eq!(
        rig.vcx
            .read(|cx| rig.dialog.read(cx).subtitle(cx).to_string()),
        "3 processes"
    );
}

#[gpui_kit::test]
fn affinity_mixed_click_sets_all(cx: &mut TestAppContext) {
    let (mut rig, _parent) = three(cx);
    click_cpu(&mut rig, 1);
    assert_eq!(mark(&mut rig, 1), affinity::ON);
    click_cpu(&mut rig, 1);
    assert_eq!(mark(&mut rig, 1), affinity::OFF);
    click_cpu(&mut rig, 1);
    assert_eq!(mark(&mut rig, 1), affinity::ON);
    assert_eq!(rig.list_text(), "0-1");
}

#[gpui_kit::test]
fn affinity_reset_restores_mixed(cx: &mut TestAppContext) {
    let (mut rig, _parent) = three(cx);
    let before = rig.vcx.read(|cx| rig.dialog.read(cx).state().clone());
    click_cpu(&mut rig, 1);
    click_cpu(&mut rig, 2);
    click_cpu(&mut rig, 2);
    click_cpu(&mut rig, 5);
    assert_ne!(
        rig.vcx.read(|cx| rig.dialog.read(cx).state().clone()),
        before
    );
    let reset = rig
        .vcx
        .debug_bounds("TlButton-AffinityReset")
        .expect("Reset drawn");
    rig.vcx.simulate_click(reset.center(), Default::default());
    rig.vcx.run_until_parked();
    rig.draw();
    assert_eq!(
        rig.vcx.read(|cx| rig.dialog.read(cx).state().clone()),
        before
    );
    assert_eq!(mark(&mut rig, 1), affinity::MIXED);
    assert_eq!(rig.list_text(), "0");
}

#[gpui_kit::test]
fn affinity_one_target_has_no_reset(cx: &mut TestAppContext) {
    let mut rig = open(cx, &[], Some((set(0..4), all())));
    assert!(rig.vcx.debug_bounds("TlButton-AffinityReset").is_none());
    assert!(rig.vcx.debug_bounds("AffinityMixedHelper").is_none());
}

#[gpui_kit::test]
fn affinity_ok_groups_targets_by_mask(cx: &mut TestAppContext) {
    let (mut rig, parent) = three(cx);
    // CPU 2 ON: results {0,1,2}, {0,2}, {0,2}; CPU 1 stays MIXED.
    click_cpu(&mut rig, 2);
    let groups = rig.vcx.read(|cx| rig.dialog.read(cx).resulting_masks());
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, set([0, 1, 2]));
    assert_eq!(&groups[0].1[..], &[ActionTarget::Process(key_of(1))]);
    assert_eq!(groups[1].0, set([0, 2]));
    assert_eq!(
        &groups[1].1[..],
        &[
            ActionTarget::Process(key_of(2)),
            ActionTarget::Process(key_of(3))
        ]
    );

    let dialog = rig.dialog.clone();
    rig.vcx
        .update(|window, cx| dialog.update(cx, |d, cx| d.apply(window, cx)));
    rig.cx.run_until_parked();
    let sent = rig.actions_sent();
    assert_eq!(sent.len(), 2, "one request per group");
    assert_eq!(sent[0].kind, ActionKind::SetAffinity(set([0, 1, 2])));
    assert_eq!(sent[1].kind, ActionKind::SetAffinity(set([0, 2])));
    for req in &sent {
        rig.engine.emit(tl_model::actions::ActionEvent {
            id: req.id,
            target: None,
            state: tl_model::actions::ActionState::Succeeded,
        });
        rig.cx.run_until_parked();
    }
    let toasts = rig.cx.read(|cx| {
        parent
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        toasts,
        ["Selected process affinity updated."],
        "one toast id"
    );
}

#[gpui_kit::test]
fn affinity_thread_targets_subtitle_and_toast(cx: &mut TestAppContext) {
    let targets = vec![
        ActionTarget::Thread {
            process: key(),
            tid: 4325,
        },
        ActionTarget::Thread {
            process: key(),
            tid: 4326,
        },
    ];
    let subject = affinity::AffinitySubject::Threads {
        n: 2,
        name: "firefox".into(),
        pid: 4321,
    };
    let subtitle = cx.update(|cx| affinity::subtitle(&subject, cx).to_string());
    assert_eq!(subtitle, "2 threads of firefox (PID 4321)");
    // The thread masks come from the Threads payload; the process mask is not used.
    let given = vec![set([0, 1]), set([0, 1])];
    let (mut rig, parent) = open_many(
        cx,
        targets.clone(),
        &subtitle,
        Some(given),
        &[(key(), set(0..32))],
    );
    assert_eq!(rig.list_text(), "0-1");
    assert_eq!(
        rig.vcx
            .read(|cx| rig.dialog.read(cx).subtitle(cx).to_string()),
        "2 threads of firefox (PID 4321)"
    );
    assert_eq!(rig.demanded(), [key()], "the process once");
    let dialog = rig.dialog.clone();
    rig.vcx
        .update(|window, cx| dialog.update(cx, |d, cx| d.apply(window, cx)));
    rig.cx.run_until_parked();
    let sent = rig.actions_sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(&sent[0].targets[..], &targets[..]);
    rig.engine.emit(tl_model::actions::ActionEvent {
        id: sent[0].id,
        target: None,
        state: tl_model::actions::ActionState::Succeeded,
    });
    rig.cx.run_until_parked();
    let toasts = rig.cx.read(|cx| {
        parent
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(toasts, ["Thread affinity updated"]);
}

#[gpui_kit::test]
fn affinity_subtitle_forms(cx: &mut TestAppContext) {
    setup(cx);
    use affinity::AffinitySubject as S;
    let cases = [
        (
            S::Process {
                name: "firefox".into(),
                pid: 4321,
            },
            "firefox (PID 4321)",
        ),
        (S::Processes(4), "4 processes"),
        (
            S::Thread {
                tid: 4325,
                name: "firefox".into(),
                pid: 4321,
            },
            "Thread 4325 of firefox (PID 4321)",
        ),
        (
            S::App {
                name: "Firefox".into(),
                n: 34,
            },
            "Firefox (34 processes)",
        ),
    ];
    for (subject, want) in cases {
        assert_eq!(
            cx.update(|cx| affinity::subtitle(&subject, cx).to_string()),
            want
        );
    }
}

// ---- VirusTotal notice and hashing toast (tasklens-gma.3, UI-PWN-036) ----

mod virustotal {
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Modifiers,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowHandle, div, px, size,
    };
    use tl_model::actions::{ActionError, ActionEvent, ActionKind, ActionRequest, ActionState};
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::details::{
        DetailTab, DetailTarget, ElfArch, ExeLocation, GeneralDetail, HashError, HashState,
        IdTriple, PackageCheck, Relro, Seccomp, TabPayload, TabSlot, TargetDetail,
    };
    use tl_model::ids::ProcessKey;
    use tl_model::settings::{Settings, UiState};
    use tl_model::system::CpuSet;
    use tl_model::table::Total;
    use tl_model::time::BootId;
    use tl_model::validity::Validity;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::windows::notices::{TOAST_DELAY, open_virustotal, vt_url};

    struct Body;

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("parent")
        }
    }

    type ParentFrame = TlSecondaryWindow<Body>;

    struct Rig {
        engine: Arc<FakeEngine>,
        parent: Entity<ParentFrame>,
        pcx: VisualTestContext,
        parent_id: gpui_kit::WindowId,
    }

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 4_242,
            boot_id: BootId(7),
            pid: 4321,
        }
    }

    fn rig(cx: &mut TestAppContext, accepted: bool) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut state = UiState::default();
        state.processes.virustotal_notice_accepted = accepted;
        let init = UiInit {
            settings: Settings::default(),
            state,
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        let spec = TlWindowSpec::new(
            TlWindowKind::Tool { resizable: true },
            "parent",
            size(px(600.), px(400.)),
        );
        let handle = cx
            .update(|cx| ParentFrame::open(cx, spec, |_, cx| cx.new(|_| Body)))
            .expect("parent opens");
        let parent = cx
            .update(|cx| ParentFrame::find(handle, cx))
            .expect("parent frame");
        let mut pcx = VisualTestContext::from_window(handle.into(), cx);
        pcx.update(|window, _| window.activate_window());
        pcx.run_until_parked();
        Rig {
            engine,
            parent,
            pcx,
            parent_id: handle.window_id(),
        }
    }

    fn open(r: &mut Rig) {
        r.pcx
            .update(|window, cx| open_virustotal(key(), window, cx));
        r.pcx.run_until_parked();
    }

    fn hash_requests(r: &Rig) -> usize {
        r.engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::ComputeHashes(k) if *k == key()))
            .count()
    }

    /// Every action request sent, in order.
    fn actions(r: &Rig) -> Vec<ActionRequest> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(req) => Some(req),
                _ => None,
            })
            .collect()
    }

    fn accepted(cx: &mut TestAppContext) -> bool {
        cx.update(|cx| TlSettings::state(cx).processes.virustotal_notice_accepted)
    }

    fn toasts(r: &mut Rig) -> Vec<String> {
        let parent = r.parent.clone();
        r.pcx.update(|window, cx| window.draw(cx).clear(cx));
        r.pcx.read(|cx| {
            parent
                .read(cx)
                .toasts()
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        })
    }

    /// The notice window, drawn.
    fn notice(r: &Rig, cx: &mut TestAppContext) -> Option<(WindowHandle<Root>, VisualTestContext)> {
        let parent_id = r.parent_id;
        let handle = cx
            .update(|cx| {
                cx.windows()
                    .into_iter()
                    .find(|w| w.window_id() != parent_id)
            })
            .and_then(|w: AnyWindowHandle| w.downcast::<Root>())?;
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        Some((handle, vcx))
    }

    fn valid<T>(value: T) -> Total<T> {
        Total {
            value,
            validity: Validity::Valid,
        }
    }

    fn text(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn general(hashes: HashState) -> GeneralDetail {
        GeneralDetail {
            started_s: valid(1_760_000_000),
            uids: valid(IdTriple {
                real: 1000,
                effective: 1000,
                saved: 1000,
                fs: 1000,
            }),
            uid_names: valid((text("u"), text("u"), text("u"))),
            policy: valid(tl_model::actions::SchedPolicy::Other),
            rt_prio: 0,
            kernel_prio: valid(120),
            autogroup: valid((text("autogroup-1"), 0)),
            affinity: valid(CpuSet::empty()),
            affinity_threads_differ: false,
            pie: valid(true),
            nx_stack: valid(true),
            relro: valid(Relro::Full),
            shadow_stack: valid(false),
            aslr_system: valid(2),
            aslr_disabled_for_process: valid(false),
            no_new_privs: valid(false),
            seccomp: valid(Seccomp::Disabled),
            architecture: valid(ElfArch::X86_64),
            exe: valid(text("/usr/bin/secret-tool")),
            exe_deleted: false,
            location: valid(ExeLocation::System),
            package: valid(None),
            description: valid(text("")),
            cwd: valid(text("/home/u")),
            root: valid(text("/")),
            cgroup: None,
            unit: None,
            app_id: None,
            cmdline: valid(text("secret-tool --token abc")),
            hashes,
            package_check: PackageCheck::NotVerified,
            handles: valid(1),
            rss_shared_bytes: valid(0),
            commit_bytes: valid(0),
            cpu_time_ms: valid(0),
            io_rchar: valid(0),
            io_wchar: valid(0),
            io_syscr: valid(0),
            io_syscw: valid(0),
            disk_read_bytes: valid(0),
            disk_write_bytes: valid(0),
            cancelled_write_bytes: valid(0),
            gpu_mem_shared_bytes: valid(0),
            gpu_engine: valid(None),
        }
    }

    /// Publishes the General payload of `key()` with `hashes`.
    fn publish_hash(r: &mut Rig, hashes: HashState) {
        let target = DetailTarget::Process(key());
        r.engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.details);
            let mut detail = TargetDetail::default();
            detail.tabs[DetailTab::General.index()] = TabSlot {
                payload: Some(Arc::new(TabPayload::General(Arc::new(general(hashes))))),
                version: d.header.version + 1,
                validity: Validity::Valid,
            };
            d.targets.insert(target, Arc::new(detail));
            d.header.version += 1;
        });
        r.pcx.run_until_parked();
    }

    fn advance(r: &mut Rig, by: Duration) {
        r.pcx.executor().advance_clock(by);
        r.pcx.run_until_parked();
    }

    const SHA: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0xa0, 0xb1, 0xc2, 0xd3, 0xe4, 0xf5, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d,
        0x9e, 0xff,
    ];

    #[gpui_kit::test]
    fn vt_notice_shown_once_then_flag_saved(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        open(&mut r);
        assert_eq!(
            hash_requests(&r),
            0,
            "no ComputeHashes before the notice is answered"
        );
        let (_, mut vcx) = notice(&r, cx).expect("the notice opens");
        let at = vcx.debug_bounds("TlDialogAction").expect("Open drawn");
        vcx.simulate_click(at.center(), Modifiers::none());
        r.pcx.run_until_parked();
        assert!(accepted(cx), "Open sets the flag");
        assert_eq!(hash_requests(&r), 1, "then ComputeHashes(key)");

        // Later uses skip the notice.
        let before = cx.update(|cx| cx.windows().len());
        open(&mut r);
        assert_eq!(cx.update(|cx| cx.windows().len()), before, "no notice");
        assert_eq!(hash_requests(&r), 2);
    }

    #[gpui_kit::test]
    fn vt_cancel_keeps_flag_false(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        open(&mut r);
        let (handle, mut vcx) = notice(&r, cx).expect("the notice opens");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        let id = handle.window_id();
        assert!(!cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id)));
        assert_eq!(hash_requests(&r), 0, "no ComputeHashes");
        assert!(actions(&r).is_empty(), "nothing is sent");
        assert!(!accepted(cx), "the flag stays false");
    }

    #[gpui_kit::test]
    fn vt_toast_only_after_500ms(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        // The hash arrives after 200 ms: no toast.
        open(&mut r);
        assert!(notice(&r, cx).is_none(), "the flag is set: no notice");
        advance(&mut r, Duration::from_millis(200));
        assert!(toasts(&mut r).is_empty());
        publish_hash(
            &mut r,
            HashState::Done {
                sha256: SHA,
                md5: [0; 16],
            },
        );
        advance(&mut r, TOAST_DELAY);
        assert!(toasts(&mut r).is_empty(), "no Computing toast was shown");

        // The hash arrives after 800 ms: the toast shows at 500 ms and goes when it arrives.
        publish_hash(&mut r, HashState::Pending);
        open(&mut r);
        advance(&mut r, Duration::from_millis(400));
        assert!(toasts(&mut r).is_empty(), "nothing at 400 ms");
        advance(&mut r, Duration::from_millis(100));
        assert_eq!(toasts(&mut r), ["Computing SHA-256…"]);
        advance(&mut r, Duration::from_millis(300));
        publish_hash(
            &mut r,
            HashState::Done {
                sha256: SHA,
                md5: [0; 16],
            },
        );
        assert!(toasts(&mut r).is_empty(), "removed when the hash arrives");
    }

    #[gpui_kit::test]
    fn vt_error_toasts(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        open(&mut r);
        advance(&mut r, TOAST_DELAY);
        publish_hash(&mut r, HashState::Failed(HashError::ExePathUnavailable));
        assert_eq!(
            toasts(&mut r),
            ["Process image unavailable"],
            "replaces the toast"
        );

        publish_hash(&mut r, HashState::Pending);
        open(&mut r);
        publish_hash(
            &mut r,
            HashState::Failed(HashError::ReadFailed { errno: 13 }),
        );
        assert!(toasts(&mut r).contains(&"SHA-256 unavailable".to_owned()));

        // The browser open fails.
        publish_hash(&mut r, HashState::Pending);
        open(&mut r);
        publish_hash(
            &mut r,
            HashState::Done {
                sha256: SHA,
                md5: [0; 16],
            },
        );
        let id = actions(&r).last().expect("OpenUri sent").id;
        r.engine.emit(ActionEvent {
            id,
            target: None,
            state: ActionState::Failed(ActionError::PermissionDenied),
        });
        r.pcx.run_until_parked();
        let shown = toasts(&mut r);
        assert!(
            shown
                .iter()
                .any(|t| t.starts_with("Could not open the browser.")),
            "{shown:?}"
        );
    }

    #[gpui_kit::test]
    fn vt_opens_hash_url(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        open(&mut r);
        assert!(actions(&r).is_empty(), "nothing opens before the hash");
        publish_hash(
            &mut r,
            HashState::Done {
                sha256: SHA,
                md5: [0; 16],
            },
        );
        let sent = actions(&r);
        assert_eq!(sent.len(), 1, "{sent:?}");
        let expected = "https://www.virustotal.com/gui/file/\
            000102030405060708090a0b0c0d0e0fa0b1c2d3e4f5162738495a6b7c8d9eff";
        assert_eq!(vt_url(&SHA), expected);
        match &sent[0].kind {
            ActionKind::OpenUri(uri) => assert_eq!(uri.as_ref(), expected, "only the hash leaves"),
            other => panic!("not OpenUri: {other:?}"),
        }
        assert!(sent[0].targets.is_empty());
        assert!(!sent[0].privileged);
        // Nothing from the process (path, command line) is in any command.
        let all = format!("{:?}", r.engine.commands());
        assert!(!all.contains("secret-tool"), "{all}");
    }
}

// ---- Custom Set Priority and the Realtime confirmation (tasklens-gma.9, UI-PWN-023) ----

mod priority {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div, px,
    };
    use smallvec::smallvec;
    use tl_model::actions::{
        ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, SchedPolicy,
    };
    use tl_model::command::Command;
    use tl_ui::action_tracker::GlobalActionTracker;
    use tl_ui::components::dialog::TlDialogView;
    use tl_ui::components::toast::{ToastKey, ToastQueue};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::windows::actions::PriorityApply;
    use tl_ui::windows::priority::{self, PriorityDialog};

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("content")
        }
    }

    struct Rig {
        cx: TestAppContext,
        engine: Arc<FakeEngine>,
        handle: WindowHandle<Root>,
        vcx: VisualTestContext,
        dialog: Entity<PriorityDialog>,
        queue: Entity<ToastQueue>,
        parent_id: gpui_kit::WindowId,
    }

    /// The dialog for `firefox (PID 4321)` with `initial`, from a parent window with toasts.
    fn open(cx: &mut TestAppContext, initial: (SchedPolicy, i8, u8)) -> Rig {
        let engine = super::setup(cx);
        let parent = cx.add_window(|window, cx| {
            let host = cx.new(|_| Host);
            Root::new(host, window, cx)
        });
        let mut pcx = VisualTestContext::from_window(parent.into(), cx);
        let queue = pcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        pcx.run_until_parked();
        let targets = smallvec![ActionTarget::Process(super::key())];
        let handle = cx
            .update(|cx| {
                priority::open(
                    targets,
                    "firefox (PID 4321)".into(),
                    initial,
                    parent.into(),
                    cx,
                )
            })
            .expect("dialog opens");
        let dialog = cx
            .update(|cx| PriorityDialog::find(handle, cx))
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
            queue,
            parent_id: parent.window_id(),
        };
        rig.draw();
        rig
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn update(
            &mut self,
            f: impl FnOnce(&mut PriorityDialog, &mut Window, &mut Context<PriorityDialog>),
        ) {
            let dialog = self.dialog.clone();
            self.vcx
                .update(|window, cx| dialog.update(cx, |d, cx| f(d, window, cx)));
            self.vcx.run_until_parked();
            self.draw();
        }

        fn read<T>(&mut self, f: impl FnOnce(&PriorityDialog, &gpui_kit::App) -> T) -> T {
            let dialog = self.dialog.clone();
            self.vcx.read(|cx| f(dialog.read(cx), cx))
        }

        /// Presses "Set priority". The window may close, so nothing draws after.
        fn apply(&mut self) {
            let dialog = self.dialog.clone();
            self.vcx
                .update(|window, cx| dialog.update(cx, |d, cx| d.apply(window, cx)));
            self.cx.run_until_parked();
            if self.is_open() {
                self.draw();
            }
        }

        fn sent(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(request) => Some(request),
                    _ => None,
                })
                .collect()
        }

        fn succeed(&mut self, request: &ActionRequest) {
            self.engine.emit(ActionEvent {
                id: request.id,
                target: None,
                state: ActionState::Succeeded,
            });
            self.cx.run_until_parked();
        }

        fn is_open(&mut self) -> bool {
            let id = self.handle.window_id();
            self.cx
                .update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
        }

        /// The Realtime confirmation window, drawn, if it is open.
        fn confirmation(&mut self) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
            let skip = [self.parent_id, self.handle.window_id()];
            let any: AnyWindowHandle = self
                .cx
                .update(|cx| cx.windows())
                .into_iter()
                .find(|w| !skip.contains(&w.window_id()))?;
            let handle = any.downcast::<Root>()?;
            let frame = self
                .cx
                .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))?;
            let view = self.cx.update(|cx| frame.read(cx).content().clone());
            let mut vcx = VisualTestContext::from_window(any, &self.cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            Some((vcx, view))
        }

        fn toasts(&mut self) -> Vec<String> {
            let queue = self.queue.clone();
            self.cx.read(|cx| {
                queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(|t| t.to_string())
                    .collect()
            })
        }
    }

    const NORMAL: (SchedPolicy, i8, u8) = (SchedPolicy::Other, 0, 0);

    #[gpui_kit::test]
    fn priority_dialog_enables_fields_by_policy(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        assert!(rig.read(|d, cx| d.nice_enabled(cx)));
        assert!(!rig.read(|d, cx| d.rt_enabled(cx)));
        assert!(rig.vcx.debug_bounds("PriorityWarning").is_none());
        assert_eq!(
            rig.read(|d, cx| d.nice_tooltip(cx)).as_ref(),
            "With autogroup on, nice only orders processes within the same session."
        );

        rig.update(|d, _, cx| d.set_policy(SchedPolicy::Idle, cx));
        assert!(!rig.read(|d, cx| d.nice_enabled(cx)));
        assert!(!rig.read(|d, cx| d.rt_enabled(cx)));
        assert_eq!(
            rig.read(|d, cx| d.nice_tooltip(cx)).as_ref(),
            "Not used by this policy"
        );
        assert!(rig.vcx.debug_bounds("PriorityWarning").is_none());

        rig.update(|d, _, cx| d.set_policy(SchedPolicy::Fifo, cx));
        assert!(!rig.read(|d, cx| d.nice_enabled(cx)));
        assert!(rig.read(|d, cx| d.rt_enabled(cx)));
        assert!(rig.read(|d, cx| d.warning_shown(cx)));
        assert!(rig.vcx.debug_bounds("PriorityWarning").is_some());

        rig.update(|d, _, cx| d.set_policy(SchedPolicy::Batch, cx));
        assert!(rig.read(|d, cx| d.nice_enabled(cx)));
        assert!(!rig.read(|d, cx| d.rt_enabled(cx)));
    }

    #[gpui_kit::test]
    fn priority_dialog_slider_page_up_steps_five(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        // Tab from the policy select to the slider.
        rig.vcx.simulate_keystrokes("tab");
        rig.draw();
        rig.vcx.simulate_keystrokes("pageup");
        rig.draw();
        assert_eq!(rig.read(|d, cx| d.nice(cx)), 5);
        rig.vcx.simulate_keystrokes("right");
        rig.draw();
        assert_eq!(rig.read(|d, cx| d.nice(cx)), 6);
        rig.vcx.simulate_keystrokes("pagedown pagedown");
        rig.draw();
        assert_eq!(rig.read(|d, cx| d.nice(cx)), -4);
        rig.vcx.simulate_keystrokes("end");
        rig.draw();
        assert_eq!(rig.read(|d, cx| d.nice(cx)), 19);
    }

    #[gpui_kit::test]
    fn priority_dialog_dispatch_same_policy_nice_only(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        rig.update(|d, window, cx| d.set_nice(5, window, cx));
        rig.apply();
        let sent = rig.sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].kind, ActionKind::SetNice(5));
        assert_eq!(&sent[0].targets[..], &[ActionTarget::Process(super::key())]);
        assert!(!sent[0].privileged);
        assert!(!rig.is_open());
        rig.succeed(&sent[0]);
        assert_eq!(rig.toasts(), ["Priority updated"]);
    }

    #[gpui_kit::test]
    fn priority_dialog_policy_change_then_nice(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        rig.update(|d, window, cx| {
            d.set_policy(SchedPolicy::Batch, cx);
            d.set_nice(3, window, cx);
        });
        rig.apply();
        let sent = rig.sent();
        assert_eq!(sent.len(), 1, "nice waits for the policy: {sent:?}");
        assert_eq!(
            sent[0].kind,
            ActionKind::SetSched {
                policy: SchedPolicy::Batch,
                rt_prio: 0
            }
        );
        assert!(!rig.is_open());

        rig.succeed(&sent[0]);
        let sent = rig.sent();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[1].kind, ActionKind::SetNice(3));
        rig.succeed(&sent[1]);

        let pushed = rig.cx.read(|cx| {
            cx.global::<GlobalActionTracker>()
                .0
                .read(cx)
                .pushed()
                .iter()
                .map(|p| (p.text.to_string(), p.key))
                .collect::<Vec<_>>()
        });
        let key = Some(ToastKey(sent[0].id.0));
        assert_eq!(
            pushed,
            [("Priority updated".to_string(), key)],
            "one toast id"
        );
        assert_eq!(rig.toasts(), ["Priority updated"]);
    }

    #[gpui_kit::test]
    fn priority_dialog_policy_change_fails_no_nice(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        rig.update(|d, _, cx| d.set_policy(SchedPolicy::Batch, cx));
        rig.apply();
        let first = rig.sent().remove(0);
        rig.engine.emit(ActionEvent {
            id: first.id,
            target: None,
            state: ActionState::Failed(tl_model::actions::ActionError::PermissionDenied),
        });
        rig.cx.run_until_parked();
        assert_eq!(rig.sent().len(), 1, "no nice after a failed policy change");
    }

    #[gpui_kit::test]
    fn priority_dialog_realtime_confirms_first(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        rig.update(|d, _, cx| {
            d.set_policy(SchedPolicy::Rr, cx);
            d.set_rt_prio(10, cx);
        });
        rig.apply();
        assert!(rig.sent().is_empty(), "nothing before the confirmation");
        assert!(rig.is_open());
        let (mut vcx, view) = rig.confirmation().expect("confirmation open");
        let (text, destructive) = vcx.update(|_, cx| {
            let dialog = view.read(cx).dialog();
            (dialog.text().to_string(), dialog.is_destructive())
        });
        assert_eq!(
            text,
            "Realtime priority can freeze the desktop if the process spins. Set it anyway?"
        );
        assert!(destructive, "Set realtime is a danger button");
        // Focus starts on Cancel: Tab to "Set realtime", then Enter.
        vcx.simulate_keystrokes("tab");
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        rig.cx.run_until_parked();

        let sent = rig.sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(
            sent[0].kind,
            ActionKind::SetSched {
                policy: SchedPolicy::Rr,
                rt_prio: 10
            }
        );
        assert!(!rig.is_open(), "the dialog closes once sent");
    }

    #[gpui_kit::test]
    fn realtime_from_custom_dialog_confirms_first(cx: &mut TestAppContext) {
        // FIFO already, Enter in the dialog: realtime always asks first.
        let mut rig = open(cx, (SchedPolicy::Fifo, 0, 50));
        assert_eq!(rig.read(|d, _| d.rt_prio()), 50);
        rig.vcx.dispatch_action(PriorityApply);
        rig.vcx.run_until_parked();
        rig.cx.run_until_parked();
        assert!(rig.sent().is_empty());
        assert!(rig.read(|d, _| d.is_confirming()));
        let (mut vcx, _) = rig.confirmation().expect("confirmation open");
        vcx.simulate_keystrokes("tab");
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        rig.cx.run_until_parked();
        let sent = rig.sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(
            sent[0].kind,
            ActionKind::SetSched {
                policy: SchedPolicy::Fifo,
                rt_prio: 50
            }
        );
    }

    #[gpui_kit::test]
    fn realtime_cancel_sends_nothing(cx: &mut TestAppContext) {
        let mut rig = open(cx, NORMAL);
        rig.update(|d, _, cx| d.set_policy(SchedPolicy::Fifo, cx));
        rig.apply();
        let (mut vcx, _) = rig.confirmation().expect("confirmation open");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        rig.cx.run_until_parked();
        assert!(rig.sent().is_empty());
        assert!(rig.is_open(), "the dialog stays open");
        assert!(rig.confirmation().is_none());
        assert!(!rig.read(|d, _| d.is_confirming()));
    }
}
