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

    pub(super) struct Rig {
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

    pub(super) fn rig(cx: &mut TestAppContext, accepted: bool) -> Rig {
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

    pub(super) fn open(r: &mut Rig) {
        r.pcx
            .update(|window, cx| open_virustotal(key(), window, cx));
        r.pcx.run_until_parked();
    }

    pub(super) fn hash_requests(r: &Rig) -> usize {
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

    pub(super) fn accepted(cx: &mut TestAppContext) -> bool {
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
    pub(super) fn notice(
        r: &Rig,
        cx: &mut TestAppContext,
    ) -> Option<(WindowHandle<Root>, VisualTestContext)> {
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
            cgroup: Total {
                value: None,
                validity: Validity::Valid,
            },
            unit: Total {
                value: None,
                validity: Validity::Valid,
            },
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

    pub(super) struct Rig {
        cx: TestAppContext,
        engine: Arc<FakeEngine>,
        handle: WindowHandle<Root>,
        vcx: VisualTestContext,
        dialog: Entity<PriorityDialog>,
        queue: Entity<ToastQueue>,
        parent_id: gpui_kit::WindowId,
    }

    /// The dialog for `firefox (PID 4321)` with `initial`, from a parent window with toasts.
    pub(super) fn open(cx: &mut TestAppContext, initial: (SchedPolicy, i8, u8)) -> Rig {
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

        pub(super) fn update(
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
        pub(super) fn apply(&mut self) {
            let dialog = self.dialog.clone();
            self.vcx
                .update(|window, cx| dialog.update(cx, |d, cx| d.apply(window, cx)));
            self.cx.run_until_parked();
            if self.is_open() {
                self.draw();
            }
        }

        pub(super) fn sent(&self) -> Vec<ActionRequest> {
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
        pub(super) fn confirmation(&mut self) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
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

    pub(super) const NORMAL: (SchedPolicy, i8, u8) = (SchedPolicy::Other, 0, 0);

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

// ---- Send Signal and real-time signal dialog (tasklens-gma.2, UI-PWN-026, UI-PWN-027) ----

mod signal {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div, px,
    };
    use smallvec::{SmallVec, smallvec};
    use tl_model::actions::{
        ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, Signal,
    };
    use tl_model::command::Command;
    use tl_model::ids::NodeId;
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::ProcessSnapshot;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::menu::{MenuEntryKind, TlMenu, TlMenuPopup};
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::pages::processes::actions::{CapturedTarget, CapturedTargets};
    use tl_ui::pages::processes::confirm::{ConfirmKind, texts as confirm_texts};
    use tl_ui::pages::processes::menu::{MenuCaps, row_menu};
    use tl_ui::pages::processes::signal_targets_for_test;
    use tl_ui::table::tree_table::RowKind;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::windows::signal::{self, RtSignalDialog, SIGNALS, SignalRefusal, TargetNames};

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("content")
        }
    }

    pub(super) struct Rig {
        cx: TestAppContext,
        engine: Arc<FakeEngine>,
        parent: WindowHandle<Root>,
        pcx: VisualTestContext,
        queue: Entity<ToastQueue>,
    }

    pub(super) fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = super::setup(cx);
        let parent = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
        let mut pcx = VisualTestContext::from_window(parent.into(), cx);
        let queue = pcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        pcx.run_until_parked();
        Rig {
            cx: cx.clone(),
            engine,
            parent,
            pcx,
            queue,
        }
    }

    impl Rig {
        pub(super) fn targets(n: u32) -> SmallVec<[ActionTarget; 1]> {
            (0..n)
                .map(|i| ActionTarget::Process(super::key_of(10_000 + i)))
                .collect()
        }

        pub(super) fn send(
            &mut self,
            signo: i32,
            targets: SmallVec<[ActionTarget; 1]>,
            names: TargetNames,
        ) {
            self.pcx
                .update(|window, cx| signal::send_signal(signo, targets, names, window, cx));
            self.pcx.run_until_parked();
            self.cx.run_until_parked();
        }

        pub(super) fn sent(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|command| match command {
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

        fn toasts(&self) -> Vec<String> {
            self.cx.read(|cx| {
                self.queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
        }

        pub(super) fn confirmation(&mut self) -> (VisualTestContext, Entity<TlDialogView>) {
            let parent = self.parent.window_id();
            let (any, frame): (AnyWindowHandle, _) = self
                .cx
                .update(|cx| {
                    cx.windows().into_iter().find_map(|window| {
                        if window.window_id() == parent {
                            return None;
                        }
                        let handle = window.downcast::<Root>()?;
                        let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                        Some((window, frame))
                    })
                })
                .expect("confirmation frame");
            let view = self.cx.read(|cx| frame.read(cx).content().clone());
            let mut vcx = VisualTestContext::from_window(any, &self.cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            (vcx, view)
        }

        fn confirm(&mut self) {
            let (mut vcx, view) = self.confirmation();
            let destructive = vcx.update(|_, cx| view.read(cx).dialog().is_destructive());
            if destructive {
                assert_eq!(
                    vcx.update(|window, cx| view.read(cx).focused_button(window)),
                    Some(DialogButton::Cancel)
                );
                vcx.simulate_keystrokes("tab");
            }
            vcx.simulate_keystrokes("enter");
            vcx.run_until_parked();
            self.cx.run_until_parked();
        }
    }

    fn captured(pid: u32, kind: RowKind, flags: ProcFlags) -> CapturedTarget {
        CapturedTarget {
            node: NodeId::Process(super::key_of(pid)),
            kind,
            name: format!("process-{pid}").into(),
            pid: Some(pid),
            uid: 1000,
            user: "me".into(),
            flags,
            member_count: 1,
            stopped: false,
            ppid: 1,
            nice: Some(0),
            exe_deleted: false,
            unit: None,
        }
    }

    fn process_record(pid: u32, ppid: u32, group: tl_model::ids::GroupKey) -> ProcessRecord {
        let text = |value: &str| Arc::<str>::from(value);
        let name = format!("process-{pid}");
        ProcessRecord {
            key: super::key_of(pid),
            ppid,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text(&name),
                name: text(&name),
                exe: Some(text(&format!("/usr/bin/{name}"))),
                cmdline: None,
                user: text("me"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: text(&name),
            }),
            group,
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

    #[gpui_kit::test]
    fn row_menu_send_signal_item_and_kernel_thread_rule(cx: &mut TestAppContext) {
        super::setup(cx);
        let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        let build = |vcx: &mut VisualTestContext, target: CapturedTarget| {
            vcx.update(|window, cx| {
                let focus = cx.focus_handle();
                let targets = CapturedTargets::new([target], None);
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    row_menu(menu, &targets, &MenuCaps::default(), window, cx)
                })
            })
        };

        let process = build(
            &mut vcx,
            captured(4321, RowKind::Process, ProcFlags::empty()),
        );
        let entries = vcx.update(|window, cx| process.read(cx).entries_for_test(window));
        let signal = entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Send Signal"))
            .expect("Send Signal item");
        assert_eq!(signal.kind, MenuEntryKind::Submenu);
        let submenu = signal.submenu.clone().expect("signal submenu");
        let labels: Vec<_> = vcx
            .update(|window, cx| submenu.read(cx).entries_for_test(window))
            .into_iter()
            .filter_map(|entry| entry.label.map(|label| label.to_string()))
            .collect();
        assert!(labels.contains(&"Hangup (HUP) (ends process)".to_owned()));
        assert!(labels.contains(&"Real-time signal…".to_owned()));

        let kernel = build(
            &mut vcx,
            captured(2, RowKind::KernelThread, ProcFlags::KERNEL_THREAD),
        );
        let entries = vcx.update(|window, cx| kernel.read(cx).entries_for_test(window));
        let signal = entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Send Signal"))
            .expect("disabled Send Signal item");
        assert_eq!(signal.kind, MenuEntryKind::Disabled);
        assert_eq!(
            signal.tooltip.as_deref(),
            Some("Kernel threads cannot be signalled.")
        );
    }

    #[gpui_kit::test]
    fn row_menu_virustotal_item_rules(cx: &mut TestAppContext) {
        super::setup(cx);
        let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        let mut labels = |targets: Vec<CapturedTarget>| {
            let menu = vcx.update(|window, cx| {
                let focus = cx.focus_handle();
                let targets = CapturedTargets::new(targets, None);
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    row_menu(menu, &targets, &MenuCaps::default(), window, cx)
                })
            });
            vcx.update(|window, cx| menu.read(cx).entries_for_test(window))
        };
        let find = |entries: &[_], label: &str| -> Option<usize> {
            entries
                .iter()
                .position(|e: &tl_ui::components::menu::MenuEntryInfo| {
                    e.label.as_deref().is_some_and(|l| l.contains(label))
                })
        };

        let entries = labels(vec![captured(4321, RowKind::Process, ProcFlags::empty())]);
        let file = find(&entries, "Open file location").expect("Open file location");
        let vt = find(&entries, "Open in VirusTotal").expect("VirusTotal item");
        assert_eq!(vt, file + 1, "item 22 follows item 21");
        assert_eq!(entries[vt].kind, MenuEntryKind::Item);

        let entries = labels(vec![captured(4321, RowKind::Process, ProcFlags::EXITED)]);
        let vt = find(&entries, "Open in VirusTotal").expect("disabled item");
        assert_eq!(entries[vt].kind, MenuEntryKind::Disabled);
        assert_eq!(entries[vt].tooltip.as_deref(), Some("Process exited"));

        let mut app = captured(4321, RowKind::App, ProcFlags::empty());
        app.pid = None;
        let pseudo = captured(5, RowKind::Pseudo, ProcFlags::empty());
        let two = vec![
            captured(1, RowKind::Process, ProcFlags::empty()),
            captured(2, RowKind::Process, ProcFlags::empty()),
        ];
        for targets in [vec![app], vec![pseudo], two] {
            let entries = labels(targets);
            assert!(find(&entries, "Open in VirusTotal").is_none());
        }
    }

    #[gpui_kit::test]
    fn dump_disabled_without_gcore_tooltip(cx: &mut TestAppContext) {
        super::setup(cx);
        let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        let mut labels = |targets: Vec<CapturedTarget>, gdb: bool| {
            let menu = vcx.update(|window, cx| {
                let focus = cx.focus_handle();
                let targets = CapturedTargets::new(targets, None);
                let caps = MenuCaps {
                    gdb,
                    ..MenuCaps::default()
                };
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    row_menu(menu, &targets, &caps, window, cx)
                })
            });
            vcx.update(|window, cx| menu.read(cx).entries_for_test(window))
        };
        let find = |entries: &[_], label: &str| -> Option<usize> {
            entries
                .iter()
                .position(|e: &tl_ui::components::menu::MenuEntryInfo| {
                    e.label.as_deref().is_some_and(|l| l.contains(label))
                })
        };
        let proc = || captured(4321, RowKind::Process, ProcFlags::empty());

        let entries = labels(vec![proc()], true);
        let vt = find(&entries, "Open in VirusTotal").expect("VirusTotal item");
        let dump = find(&entries, "Create dump file").expect("dump item");
        assert_eq!(dump, vt + 1, "item 23 follows item 22");
        assert_eq!(entries[dump].kind, MenuEntryKind::Item);

        let entries = labels(vec![proc()], false);
        let dump = find(&entries, "Create dump file").expect("disabled item");
        assert_eq!(entries[dump].kind, MenuEntryKind::Disabled);
        assert_eq!(
            entries[dump].tooltip.as_deref(),
            Some("Install gdb to create dumps")
        );

        for gdb in [true, false] {
            let entries = labels(
                vec![captured(4321, RowKind::Process, ProcFlags::EXITED)],
                gdb,
            );
            let dump = find(&entries, "Create dump file").expect("disabled item");
            assert_eq!(entries[dump].kind, MenuEntryKind::Disabled);
            assert_eq!(entries[dump].tooltip.as_deref(), Some("Process exited"));
        }

        let mut app = captured(4321, RowKind::App, ProcFlags::empty());
        app.pid = None;
        let pseudo = captured(5, RowKind::Pseudo, ProcFlags::empty());
        let two = vec![
            captured(1, RowKind::Process, ProcFlags::empty()),
            captured(2, RowKind::Process, ProcFlags::empty()),
        ];
        for targets in [vec![app], vec![pseudo], two] {
            let entries = labels(targets, true);
            assert!(find(&entries, "Create dump file").is_none());
        }
    }

    #[gpui_kit::test]
    fn row_menu_signal_app_row_targets_primaries(cx: &mut TestAppContext) {
        super::setup(cx);
        let group = GroupKind::App {
            app_id: Arc::from("firefox"),
        }
        .key();
        let snapshot = ProcessSnapshot {
            records: vec![
                process_record(100, 1, group),
                process_record(101, 100, group),
                process_record(200, 1, group),
            ]
            .into(),
            ..ProcessSnapshot::default()
        };
        let app = CapturedTarget {
            node: NodeId::Group(group),
            kind: RowKind::App,
            name: "Firefox".into(),
            pid: None,
            uid: 1000,
            user: "me".into(),
            flags: ProcFlags::empty(),
            member_count: 3,
            stopped: false,
            ppid: 0,
            nice: None,
            exe_deleted: false,
            unit: None,
        };
        let (targets, names) = signal_targets_for_test(&[app], &snapshot).expect("signal targets");
        assert_eq!(
            &targets[..],
            &[
                ActionTarget::Process(super::key_of(100)),
                ActionTarget::Process(super::key_of(200)),
            ]
        );
        assert_eq!(names.subtitle().as_ref(), "Firefox (2 processes)");
    }

    #[gpui_kit::test]
    fn row_menu_realtime_signal_opens_dialog(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let before = rig.cx.update(|cx| cx.windows().len());
        let (handle, dialog, _) = open_rt(&mut rig);
        assert_eq!(rig.cx.update(|cx| cx.windows().len()), before + 1);
        assert!((34..=64).contains(&rig.cx.read(|cx| dialog.read(cx).value())));
        let id = handle.window_id();
        assert!(
            rig.cx
                .update(|cx| cx.windows().iter().any(|window| window.window_id() == id))
        );
    }

    #[gpui_kit::test]
    fn signal_menu_labels_and_suffixes(cx: &mut TestAppContext) {
        super::setup(cx);
        let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        let menu: Entity<TlMenuPopup> = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            TlMenu::build(window, cx, focus, |menu, window, cx| {
                signal::signal_submenu(menu, false, window, cx)
            })
        });
        let entries = vcx.update(|window, cx| menu.read(cx).entries_for_test(window));
        let labels: Vec<_> = entries.iter().filter_map(|e| e.label.as_deref()).collect();
        assert!(labels.contains(&"Terminate (TERM) (ends process)"));
        assert!(labels.contains(&"Stop (STOP)"));
        let other = entries
            .iter()
            .find(|entry| entry.kind == MenuEntryKind::Submenu)
            .and_then(|entry| entry.submenu.clone())
            .expect("Other signals submenu");
        let other = vcx.update(|window, cx| other.read(cx).entries_for_test(window));
        let chld = other
            .iter()
            .find(|entry| entry.label.as_deref() == Some("CHLD (17)"))
            .expect("CHLD");
        assert_eq!(chld.kind, MenuEntryKind::Item);
        assert!(!chld.label.as_deref().unwrap().contains("ends process"));
    }

    #[gpui_kit::test]
    fn signal_toast_single_multi_app(cx: &mut TestAppContext) {
        let mut single = rig(cx);
        single.send(1, Rig::targets(1), TargetNames::process("firefox", 1234));
        let request = single.sent().pop().expect("single request");
        assert_eq!(request.kind, ActionKind::Signal(Signal::Hup));
        single.succeed(&request);
        assert_eq!(single.toasts(), ["Sent SIGHUP to firefox (1234)"]);

        let mut multiple = rig(cx);
        multiple.send(1, Rig::targets(4), TargetNames::processes(4));
        assert!(multiple.sent().is_empty());
        multiple.confirm();
        let request = multiple.sent().pop().expect("multi request");
        multiple.succeed(&request);
        assert_eq!(multiple.toasts(), ["Sent SIGHUP to 4 processes"]);

        let mut app = rig(cx);
        app.send(1, Rig::targets(3), TargetNames::app("Firefox", 3));
        app.confirm();
        let request = app.sent().pop().expect("app request");
        app.succeed(&request);
        assert_eq!(app.toasts(), ["Sent SIGHUP to Firefox (3 processes)"]);
    }

    #[gpui_kit::test]
    fn signal_multi_target_confirmation_text_and_focus(cx: &mut TestAppContext) {
        let mut hup = rig(cx);
        hup.send(1, Rig::targets(4), TargetNames::processes(4));
        let (mut vcx, view) = hup.confirmation();
        let expected_heading = vcx.update(|_, cx| {
            confirm_texts(
                &ConfirmKind::SignalMany {
                    count: 4,
                    signal_name: "HUP".into(),
                    ends_process: true,
                },
                cx,
            )
            .title
            .to_string()
        });
        let (text, danger, focused) = vcx.update(|window, cx| {
            let dialog = view.read(cx).dialog();
            (
                dialog.text().to_string(),
                dialog.is_destructive(),
                view.read(cx).focused_button(window),
            )
        });
        assert_eq!(expected_heading, "Send signal to 4 processes?");
        assert!(vcx.debug_bounds("TlDialogHeading").is_some());
        assert_eq!(text, "Send SIGHUP to 4 selected processes?");
        assert!(danger);
        assert_eq!(focused, Some(DialogButton::Cancel));
        vcx.simulate_keystrokes("tab enter");
        vcx.run_until_parked();
        hup.cx.run_until_parked();
        assert_eq!(hup.sent().len(), 1);

        let mut cont = rig(cx);
        cont.send(18, Rig::targets(4), TargetNames::processes(4));
        let (mut vcx, view) = cont.confirmation();
        assert!(!vcx.update(|_, cx| view.read(cx).dialog().is_destructive()));
        assert_eq!(
            vcx.update(|window, cx| view.read(cx).focused_button(window)),
            Some(DialogButton::Action)
        );
    }

    #[gpui_kit::test]
    fn signal_refusals_send_nothing(cx: &mut TestAppContext) {
        for (pid, refusal, text) in [
            (std::process::id(), None, "Refusing to end tasklens itself."),
            (1, None, "PID 1 (systemd) cannot be ended."),
            (
                10_000,
                Some(SignalRefusal::KernelThread),
                "Kernel threads cannot be signalled.",
            ),
        ] {
            let mut rig = rig(cx);
            let mut names = TargetNames::process("target", pid);
            if let Some(refusal) = refusal {
                names = names.with_refusal(refusal);
            }
            rig.send(
                1,
                smallvec![ActionTarget::Process(super::key_of(pid))],
                names,
            );
            assert!(rig.sent().is_empty(), "PID {pid}");
            assert_eq!(rig.toasts(), [text], "PID {pid}");
        }
    }

    fn open_rt(
        rig: &mut Rig,
    ) -> (
        WindowHandle<Root>,
        Entity<RtSignalDialog>,
        VisualTestContext,
    ) {
        let handle = rig
            .cx
            .update(|cx| {
                signal::open_rt_signal(
                    smallvec![ActionTarget::Process(super::key())],
                    TargetNames::process("firefox", 4321),
                    rig.parent.into(),
                    cx,
                )
            })
            .expect("real-time dialog");
        let dialog = rig
            .cx
            .read(|cx| RtSignalDialog::find(handle, cx))
            .expect("dialog content");
        let mut vcx = VisualTestContext::from_window(handle.into(), &rig.cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        (handle, dialog, vcx)
    }

    #[gpui_kit::test]
    fn rt_signal_label_for_34_and_64(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let (_, dialog, mut vcx) = open_rt(&mut rig);
        vcx.update(|_, cx| dialog.update(cx, |dialog, cx| dialog.set_value(34, cx)));
        assert_eq!(
            vcx.read(|cx| dialog.read(cx).label().to_string()),
            "SIGRTMIN+0"
        );
        vcx.update(|_, cx| dialog.update(cx, |dialog, cx| dialog.set_value(64, cx)));
        assert_eq!(
            vcx.read(|cx| dialog.read(cx).label().to_string()),
            "SIGRTMIN+30 (SIGRTMAX)"
        );
    }

    #[gpui_kit::test]
    fn rt_signal_remembers_last_value(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let (_, dialog, mut vcx) = open_rt(&mut rig);
        vcx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.set_value(36, cx);
                dialog.apply(window, cx);
            })
        });
        rig.cx.run_until_parked();
        let (_, again, _) = open_rt(&mut rig);
        assert_eq!(rig.cx.read(|cx| again.read(cx).value()), 36);
    }

    #[test]
    fn signals_table_has_expected_effects() {
        assert_eq!(SIGNALS.len(), 31);
        assert!(
            SIGNALS
                .iter()
                .find(|s| s.name == "TERM")
                .unwrap()
                .ends_process
        );
        assert!(
            !SIGNALS
                .iter()
                .find(|s| s.name == "STOP")
                .unwrap()
                .ends_process
        );
        assert!(
            !SIGNALS
                .iter()
                .find(|s| s.name == "CHLD")
                .unwrap()
                .ends_process
        );
    }
}

// ---- Dump confirmation, progress and result toasts (tasklens-gma.36, UI-PWN-034, UI-PWN-035) ----

mod dump {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Modifiers,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div,
        px, size,
    };
    use tl_model::actions::{
        ActionError, ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, DumpMode,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::health::HelperState;
    use tl_model::ids::{GroupKey, ProcessKey};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::settings::{Settings, UiState};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::TlDialogView;
    use tl_ui::components::toast::ToastKey;
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
    use tl_ui::hub::GlobalHub;
    use tl_ui::pages::processes::confirm::{ConfirmKind, texts as confirm_texts};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::windows::notices::{DUMP_POLL, DumpToast, set_test_home, start_dump};

    pub(super) const GIB: u64 = 1024 * 1024 * 1024;

    struct Body;

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("parent")
        }
    }

    type ParentFrame = TlSecondaryWindow<Body>;

    pub(super) struct Rig {
        engine: Arc<FakeEngine>,
        parent: Entity<ParentFrame>,
        pcx: VisualTestContext,
        parent_id: gpui_kit::WindowId,
    }

    pub(super) fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 4_242,
            boot_id: BootId(7),
            pid,
        }
    }

    pub(super) fn record(pid: u32, name: &str, rss: u64) -> ProcessRecord {
        let name: Arc<str> = Arc::from(name);
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: name.clone(),
                name: name.clone(),
                exe: None,
                cmdline: None,
                user: Arc::from("u"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: name,
            }),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.,
            rss_anon_bytes: rss,
            rss_bytes: rss,
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

    /// A parent window with toasts, `records` in the snapshot, dumps under `state_dir`.
    pub(super) fn rig(
        cx: &mut TestAppContext,
        state_dir: &Path,
        mode: DumpMode,
        mut records: Vec<ProcessRecord>,
    ) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.processes.dump_type = mode;
        let init = UiInit {
            settings,
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv {
                state_dir: state_dir.to_path_buf(),
                ..AppEnv::test()
            },
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        records.sort_by_key(|r| r.key);
        engine.publish_with(|s| {
            s.published_ns += 1_000_000_000;
            let p = Arc::make_mut(&mut s.processes);
            p.header.version += 1;
            p.records = Arc::from(records);
        });
        cx.run_until_parked();
        // No main window renders here, so mark the snapshot drawn: else the hub goes hidden
        // after 1 s and the toast queue drops every push.
        cx.update(|cx| {
            let hub = cx.global::<GlobalHub>().0.clone();
            hub.update(cx, |hub, cx| hub.note_render(cx));
        });
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

    pub(super) fn start(r: &mut Rig, pid: u32) {
        r.pcx.update(|window, cx| start_dump(key(pid), window, cx));
        r.pcx.run_until_parked();
    }

    /// Every action request sent, in order.
    pub(super) fn actions(r: &Rig) -> Vec<ActionRequest> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(req) => Some(req),
                _ => None,
            })
            .collect()
    }

    /// The one dump request: its mode and path.
    fn dump(r: &Rig) -> (ActionRequest, DumpMode, String) {
        let sent = actions(r);
        assert_eq!(sent.len(), 1, "{sent:?}");
        match &sent[0].kind {
            ActionKind::Dump { mode, path } => (sent[0].clone(), *mode, path.to_string()),
            other => panic!("not Dump: {other:?}"),
        }
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

    fn advance(r: &mut Rig, by: Duration) {
        r.pcx.executor().advance_clock(by);
        r.pcx.run_until_parked();
    }

    /// Clicks the element drawn with `selector` in the parent window.
    fn click(r: &mut Rig, selector: String) {
        r.pcx.update(|window, cx| window.draw(cx).clear(cx));
        // debug_bounds takes a static selector.
        let selector: &'static str = selector.leak();
        let at = r
            .pcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"));
        r.pcx.simulate_click(at.center(), Modifiers::none());
        r.pcx.run_until_parked();
    }

    fn toast_key(request: &ActionRequest) -> u64 {
        ToastKey::from(DumpToast(request.id.0)).0
    }

    /// The target result and the summary, both `state`.
    fn finish(r: &mut Rig, request: &ActionRequest, state: ActionState) {
        r.engine.emit(ActionEvent {
            id: request.id,
            target: Some(0),
            state: state.clone(),
        });
        r.engine.emit(ActionEvent {
            id: request.id,
            target: None,
            state,
        });
        r.pcx.run_until_parked();
    }

    /// The confirmation window, drawn.
    pub(super) fn confirmation(
        r: &Rig,
        cx: &mut TestAppContext,
    ) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
        let parent_id = r.parent_id;
        let (any, frame): (AnyWindowHandle, _) = cx.update(|cx| {
            cx.windows().into_iter().find_map(|window| {
                if window.window_id() == parent_id {
                    return None;
                }
                let handle = window.downcast::<Root>()?;
                let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                Some((window, frame))
            })
        })?;
        let view = cx.read(|cx| frame.read(cx).content().clone());
        let mut vcx = VisualTestContext::from_window(any, cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        Some((vcx, view))
    }

    /// `{name}_{pid}_{yyyymmdd_hhmmss}.core`.
    fn assert_file_name(file: &str, stem: &str, pid: u32) {
        let prefix = format!("{stem}_{pid}_");
        let stamp = file
            .strip_prefix(&prefix)
            .and_then(|f| f.strip_suffix(".core"))
            .unwrap_or_else(|| panic!("{file}"));
        let (date, time) = stamp.split_once('_').expect("date_time");
        assert_eq!(date.len(), 8, "{file}");
        assert_eq!(time.len(), 6, "{file}");
        assert!(
            date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()),
            "{file}"
        );
    }

    #[gpui_kit::test]
    fn dump_confirm_only_over_2gib(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let mut r = rig(
            cx,
            &state,
            DumpMode::Mini,
            vec![
                record(100, "Isolated Web Co", GIB),
                record(200, "big", 34 * GIB / 10),
            ],
        );

        // 1 GiB: no confirmation, the Dump goes out with the computed path.
        start(&mut r, 100);
        assert!(confirmation(&r, cx).is_none(), "no confirmation at 1 GiB");
        let (request, mode, path) = dump(&r);
        assert_eq!(mode, DumpMode::Mini);
        assert_eq!(
            request.targets.as_slice(),
            &[ActionTarget::Process(key(100))]
        );
        assert!(!request.privileged);
        let (dir, file) = path.rsplit_once('/').expect("a file name");
        assert_eq!(dir, "/s/tasklens/dumps");
        assert_file_name(file, "Isolated_Web_Co", 100);

        // 3.4 GiB: the confirmation comes first; Cancel sends nothing.
        start(&mut r, 200);
        let (mut vcx, view) = confirmation(&r, cx).expect("the confirmation opens");
        let expected = vcx.update(|_, cx| {
            confirm_texts(
                &ConfirmKind::LargeDump {
                    name: "big".into(),
                    size: "3.40 GiB".into(),
                },
                cx,
            )
        });
        assert_eq!(expected.title.as_ref(), "Create a large dump?");
        assert_eq!(expected.confirm_label.as_ref(), "Create dump");
        assert!(vcx.debug_bounds("TlDialogHeading").is_some());
        let text = vcx.update(|_, cx| view.read(cx).dialog().text().to_string());
        assert_eq!(text, expected.description.as_ref());
        assert_eq!(
            text,
            "big uses 3.40 GiB. The process pauses while the core is written and the file \
             will be about that size. Continue?"
        );
        assert_eq!(actions(&r).len(), 1, "nothing before the answer");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        r.pcx.run_until_parked();
        assert!(confirmation(&r, cx).is_none(), "closed");
        assert_eq!(actions(&r).len(), 1, "Cancel sends nothing");

        // "Create dump" sends it.
        start(&mut r, 200);
        let (mut vcx, _) = confirmation(&r, cx).expect("the confirmation opens again");
        let at = vcx
            .debug_bounds("TlDialogAction")
            .expect("Create dump drawn");
        vcx.simulate_click(at.center(), Modifiers::none());
        r.pcx.run_until_parked();
        let sent = actions(&r);
        assert_eq!(sent.len(), 2, "{sent:?}");
        match &sent[1].kind {
            ActionKind::Dump { path, .. } => {
                let file = path.rsplit_once('/').expect("a file name").1;
                assert_file_name(file, "big", 200);
            }
            other => panic!("not Dump: {other:?}"),
        }
    }

    #[gpui_kit::test]
    fn dump_mode_follows_setting(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let mut r = rig(
            cx,
            &state,
            DumpMode::Full,
            vec![record(7, "kworker/0:1H", 0)],
        );
        start(&mut r, 7);
        let (_, mode, path) = dump(&r);
        assert_eq!(mode, DumpMode::Full);
        let file = path.rsplit_once('/').expect("a file name").1;
        assert_file_name(file, "kworker_0_1H", 7);
    }

    #[gpui_kit::test]
    fn dump_progress_toast_updates_in_place(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut r = rig(cx, tmp.path(), DumpMode::Mini, vec![record(5, "x", GIB)]);
        start(&mut r, 5);
        let (request, _, path) = dump(&r);
        assert_eq!(toasts(&mut r), ["Writing core…"], "before the first size");
        advance(&mut r, DUMP_POLL);
        assert_eq!(toasts(&mut r), ["Writing core…"], "no file yet");

        // The file grows to 1.2 GiB. A sparse file has that size and holds no data.
        std::fs::create_dir_all(tmp.path().join("dumps")).expect("dumps dir");
        let file = std::fs::File::create(&path).expect("core file");
        file.set_len(12 * GIB / 10).expect("sparse size");
        advance(&mut r, DUMP_POLL);
        assert_eq!(toasts(&mut r), ["Writing core… 1.20 GiB"], "same toast");

        file.set_len(2 * GIB).expect("sparse size");
        advance(&mut r, DUMP_POLL);
        assert_eq!(toasts(&mut r), ["Writing core… 2.00 GiB"]);

        // The Info toast stays while the dump runs, past its 5 s.
        for _ in 0..16 {
            advance(&mut r, DUMP_POLL);
        }
        assert_eq!(toasts(&mut r), ["Writing core… 2.00 GiB"], "after 8 s");

        // The toast key is DumpToast(action id).
        let key: ToastKey = DumpToast(request.id.0).into();
        // debug_bounds takes a static selector.
        let selector: &'static str = format!("TlToast-k{}", key.0).leak();
        r.pcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(r.pcx.debug_bounds(selector).is_some(), "{selector}");

        // After the result the size is no longer read.
        finish(&mut r, &request, ActionState::Succeeded);
        file.set_len(3 * GIB).expect("sparse size");
        advance(&mut r, DUMP_POLL);
        let shown = toasts(&mut r);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].starts_with("Dump created: "), "{shown:?}");
    }

    #[gpui_kit::test]
    fn dump_progress_toast_stays_closed(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut r = rig(cx, tmp.path(), DumpMode::Mini, vec![record(5, "x", GIB)]);
        start(&mut r, 5);
        let (request, _, path) = dump(&r);
        std::fs::create_dir_all(tmp.path().join("dumps")).expect("dumps dir");
        let file = std::fs::File::create(&path).expect("core file");
        file.set_len(GIB).expect("sparse size");
        advance(&mut r, DUMP_POLL);
        assert_eq!(toasts(&mut r), ["Writing core… 1.00 GiB"]);

        // Closed while the dump runs: later size reads do not bring it back.
        let key = toast_key(&request);
        click(&mut r, format!("TlButton-TlToast-k{key}-close"));
        assert!(toasts(&mut r).is_empty());
        file.set_len(2 * GIB).expect("sparse size");
        for _ in 0..4 {
            advance(&mut r, DUMP_POLL);
        }
        assert!(toasts(&mut r).is_empty(), "still closed");

        // The result still shows.
        finish(&mut r, &request, ActionState::Succeeded);
        let shown = toasts(&mut r);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].starts_with("Dump created: "), "{shown:?}");
    }

    #[gpui_kit::test]
    fn dump_success_toast_text_and_button(cx: &mut TestAppContext) {
        let state = PathBuf::from("/home/u/.local/state/tasklens");
        let mut r = rig(
            cx,
            &state,
            DumpMode::Mini,
            vec![record(1, "x", GIB), record(2, "y", GIB)],
        );
        r.pcx.update(|_, cx| set_test_home(cx, "/home/u"));
        start(&mut r, 1);
        let (request, _, path) = dump(&r);
        let file = path.rsplit_once('/').expect("a file name").1.to_owned();
        assert_file_name(&file, "x", 1);
        finish(&mut r, &request, ActionState::Succeeded);
        assert_eq!(
            toasts(&mut r),
            [format!(
                "Dump created: ~/.local/state/tasklens/dumps/{file}"
            )]
        );

        let created = format!("Dump created: ~/.local/state/tasklens/dumps/{file}");

        // A click on the path copies the full path.
        let key = toast_key(&request);
        click(&mut r, format!("TlToast-k{key}-text"));
        let copied = r
            .pcx
            .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some(path.as_str()));
        assert_eq!(toasts(&mut r), [created.as_str(), "Copied to clipboard"]);

        // A second dump succeeds; "Open folder" on the first toast shows the first file.
        start(&mut r, 2);
        let second = actions(&r).pop().expect("second dump");
        finish(&mut r, &second, ActionState::Succeeded);
        assert_eq!(toasts(&mut r).len(), 3);
        click(&mut r, format!("TlButton-TlToast-k{key}-action"));
        let sent = actions(&r);
        assert_eq!(sent.len(), 3, "{sent:?}");
        assert_eq!(
            sent[2].kind,
            ActionKind::ShowInFileManager {
                path: path.as_str().into(),
                properties: false,
            }
        );
        let shown = toasts(&mut r);
        assert!(!shown.contains(&created), "{shown:?}");
        assert_eq!(
            shown.last().map(String::as_str),
            Some("Opening file location")
        );
    }

    #[gpui_kit::test]
    fn dump_failure_toast_text(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let mut r = rig(cx, &state, DumpMode::Mini, vec![record(3, "x", GIB)]);
        start(&mut r, 3);
        let (request, _, _) = dump(&r);
        finish(
            &mut r,
            &request,
            ActionState::Failed(ActionError::Internal("gcore exited 1".into())),
        );
        let shown = toasts(&mut r);
        assert_eq!(shown.len(), 1, "{shown:?}");
        let (first, second) = shown[0].split_once('\n').expect("two lines");
        assert_eq!(first, "Could not create dump file.");
        assert!(second.contains("gcore exited 1"), "{second}");

        // An error toast closes after 8 s.
        advance(&mut r, Duration::from_secs(9));
        assert!(toasts(&mut r).is_empty());

        // Refused for privileges.
        start(&mut r, 3);
        let request = actions(&r).pop().expect("second dump");
        finish(
            &mut r,
            &request,
            ActionState::Failed(ActionError::PermissionDenied),
        );
        assert_eq!(
            toasts(&mut r),
            ["Creating a dump needs administrator rights"]
        );
    }

    /// tl-helper is installed, so a privilege failure may be retried.
    fn publish_helper(r: &mut Rig, helper: HelperState) {
        r.engine.publish_with(|s| {
            let health = Arc::make_mut(&mut s.health);
            health.helper = helper;
            health.version += 1;
        });
        r.pcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn dump_privilege_failure_offers_retry(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let mut r = rig(cx, &state, DumpMode::Mini, vec![record(3, "x", GIB)]);
        publish_helper(&mut r, HelperState::Locked);
        start(&mut r, 3);
        let (request, _, path) = dump(&r);
        let tkey = toast_key(&request);
        finish(
            &mut r,
            &request,
            ActionState::Failed(ActionError::PermissionDenied),
        );
        assert_eq!(
            toasts(&mut r),
            ["Creating a dump needs administrator rights"]
        );

        // The button is the tracker's RetryActionAsAdmin: the dump goes again, privileged.
        click(&mut r, format!("TlButton-TlToast-k{tkey}-action"));
        let sent = actions(&r);
        assert_eq!(sent.len(), 2, "{sent:?}");
        let retry = &sent[1];
        assert!(retry.privileged);
        assert_ne!(retry.id, request.id);
        assert_eq!(retry.targets.as_slice(), &[ActionTarget::Process(key(3))]);
        assert_eq!(
            retry.kind,
            ActionKind::Dump {
                mode: DumpMode::Mini,
                path: path.as_str().into(),
            }
        );

        // The retry shows its progress and result in the same toast.
        assert_eq!(toasts(&mut r), ["Writing core…"]);
        r.pcx.update(|window, cx| window.draw(cx).clear(cx));
        let selector: &'static str = format!("TlToast-k{tkey}").leak();
        assert!(r.pcx.debug_bounds(selector).is_some(), "{selector}");
        finish(&mut r, retry, ActionState::Succeeded);
        let shown = toasts(&mut r);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].starts_with("Dump created: "), "{shown:?}");

        // A privileged dump that is refused again offers no second retry.
        start(&mut r, 3);
        let third = actions(&r).pop().expect("third dump");
        finish(
            &mut r,
            &third,
            ActionState::Failed(ActionError::PermissionDenied),
        );
        let third_key = toast_key(&third);
        click(&mut r, format!("TlButton-TlToast-k{third_key}-action"));
        let fourth = actions(&r).pop().expect("fourth dump");
        assert!(fourth.privileged);
        finish(
            &mut r,
            &fourth,
            ActionState::Failed(ActionError::PermissionDenied),
        );
        r.pcx.update(|window, cx| window.draw(cx).clear(cx));
        let button: &'static str = format!("TlButton-TlToast-k{third_key}-action").leak();
        assert!(
            r.pcx.debug_bounds(button).is_none(),
            "no retry after a privileged try"
        );
        assert!(toasts(&mut r).contains(&"Creating a dump needs administrator rights".to_owned()));
    }

    #[gpui_kit::test]
    fn dump_privilege_failure_without_helper_has_no_retry(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let mut r = rig(cx, &state, DumpMode::Mini, vec![record(3, "x", GIB)]);
        start(&mut r, 3);
        let (request, _, _) = dump(&r);
        finish(
            &mut r,
            &request,
            ActionState::Failed(ActionError::PermissionDenied),
        );
        assert_eq!(
            toasts(&mut r),
            ["Creating a dump needs administrator rights"]
        );
        let button: &'static str =
            format!("TlButton-TlToast-k{}-action", toast_key(&request)).leak();
        assert!(
            r.pcx.debug_bounds(button).is_none(),
            "no retry without tl-helper"
        );
    }

    #[gpui_kit::test]
    fn dump_paths_live_while_toasts_show(cx: &mut TestAppContext) {
        let state = PathBuf::from("/s/tasklens");
        let records = (1..=17)
            .map(|pid| record(pid, &format!("p{pid}"), GIB))
            .collect();
        let mut r = rig(cx, &state, DumpMode::Mini, records);
        let mut first = None;
        for pid in 1..=17 {
            start(&mut r, pid);
            let request = actions(&r).pop().expect("a dump");
            finish(&mut r, &request, ActionState::Succeeded);
            first.get_or_insert(request);
        }
        let first = first.expect("17 dumps");
        let ActionKind::Dump { path, .. } = &first.kind else {
            panic!("not Dump: {:?}", first.kind);
        };
        let path = path.to_string();
        assert!(path.contains("/p1_1_"), "{path}");
        let key = toast_key(&first);

        // Every toast is still shown or waiting; the first one's path and button still work.
        click(&mut r, format!("TlToast-k{key}-text"));
        let copied = r
            .pcx
            .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some(path.as_str()));
        click(&mut r, format!("TlButton-TlToast-k{key}-action"));
        let sent = actions(&r).pop().expect("Open folder");
        assert_eq!(
            sent.kind,
            ActionKind::ShowInFileManager {
                path: path.as_str().into(),
                properties: false,
            }
        );
    }
}

// ---- UI-PRC-014 texts for every process window trigger (tasklens-gma.38, UI-PWN-025) ----

mod prc014 {
    use std::path::Path;
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, WindowHandle, WindowId};
    use tl_model::actions::{ActionKind, ActionTarget, SchedPolicy};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::details::{HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId};
    use tl_model::ids::ProcessKey;
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::ProcessSnapshot;
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::pages::processes::confirm::{ConfirmKind, texts as confirm_texts};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::open_main_window_on;
    use tl_ui::windows::handle_search::{HandleSearchWindow, open_handle_search};
    use tl_ui::windows::signal::{OtherUserTarget, TargetNames};

    use super::{dump, priority, signal, virustotal};

    /// What one confirmation must show.
    struct Expect {
        heading: &'static str,
        text: &'static str,
        button: &'static str,
        danger: bool,
        focus: DialogButton,
    }

    type Dialog = (VisualTestContext, Entity<TlDialogView>);

    /// Window title "tasklens", the heading, text, button label, danger flag and initial focus.
    fn check(dialog: &mut Dialog, kind: &ConfirmKind, e: &Expect) {
        let (vcx, view) = dialog;
        let expected = vcx.update(|_, cx| confirm_texts(kind, cx));
        assert_eq!(expected.title.as_ref(), e.heading, "heading text");
        assert_eq!(expected.description.as_ref(), e.text, "description text");
        assert_eq!(expected.confirm_label.as_ref(), e.button, "button label");
        assert_eq!(expected.confirm_danger, e.danger, "texts danger flag");
        let (title, text, danger, focused) = vcx.update(|window, cx| {
            let dialog = view.read(cx).dialog();
            (
                dialog.window_title().to_string(),
                dialog.text().to_string(),
                dialog.is_destructive(),
                view.read(cx).focused_button(window),
            )
        });
        assert_eq!(title, "tasklens", "{}: window title", e.heading);
        assert!(
            vcx.debug_bounds("TlDialogHeading").is_some(),
            "{}: heading drawn",
            e.heading
        );
        assert_eq!(text, e.text, "{}: text", e.heading);
        assert_eq!(danger, e.danger, "{}: danger", e.heading);
        assert_eq!(focused, Some(e.focus), "{}: initial focus", e.heading);
    }

    /// Presses `keys` in the dialog and lets everything settle.
    fn press_in(dialog: &mut Dialog, cx: &mut TestAppContext, keys: &str) {
        dialog.0.simulate_keystrokes(keys);
        dialog.0.run_until_parked();
        cx.run_until_parked();
    }

    /// The `Command::Action`s in `commands`.
    fn action_count(engine: &FakeEngine) -> usize {
        engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::Action(_)))
            .count()
    }

    // ---- Handle Search fixtures (copied from ui_handle_search.rs, trimmed) ----

    struct HsRig {
        engine: Arc<FakeEngine>,
        win: VisualTestContext,
        view: Entity<HandleSearchWindow>,
    }

    fn hs_setup(cx: &mut TestAppContext) -> HsRig {
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
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let _main = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        cx.run_until_parked();
        cx.update(open_handle_search);
        cx.run_until_parked();
        let (handle, view) = cx
            .update(|cx| {
                cx.windows().into_iter().find_map(|w| {
                    let h = w.downcast::<Root>()?;
                    HandleSearchWindow::find(h, cx).map(|v| (h, v))
                })
            })
            .expect("the Handle Search window opens");
        let mut win = VisualTestContext::from_window(handle.into(), cx);
        win.update(|window, _| window.activate_window());
        win.run_until_parked();
        HsRig { engine, win, view }
    }

    fn hs_draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();
    }

    fn hs_key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1_000 + u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn hs_hit(pid: u32, fd: i32, path: &str) -> HandleHit {
        HandleHit {
            key: hs_key(pid),
            name: format!("proc{pid}").into(),
            user: "me".into(),
            kind: HandleKind::Fd,
            fd: Some(fd),
            path: path.into(),
        }
    }

    /// An own, unprotected live process `pid`.
    fn hs_record(pid: u32) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        let name = format!("proc{pid}");
        ProcessRecord {
            key: hs_key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a(&name),
                name: a(&name),
                exe: Some(a(&format!("/usr/bin/{name}"))),
                cmdline: None,
                user: a("me"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a(&name),
            }),
            group: GroupKind::Background.key(),
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

    /// Publishes own unprotected processes `pids`, then a finished search with `hits`, and
    /// draws the table.
    fn hs_show(cx: &mut TestAppContext, rig: &mut HsRig, pids: &[u32], hits: Vec<HandleHit>) {
        let mut records: Vec<ProcessRecord> = pids.iter().map(|&p| hs_record(p)).collect();
        records.sort_by_key(|r| r.key);
        rig.engine.publish_with(|s| {
            s.processes = Arc::new(ProcessSnapshot {
                records: records.into(),
                ..ProcessSnapshot::default()
            });
        });
        cx.run_until_parked();

        let input = rig.win.read(|cx| rig.view.read(cx).pattern().clone());
        rig.win
            .update(|window, cx| input.update(cx, |s, cx| s.set_value("log", window, cx)));
        rig.win.run_until_parked();
        rig.win.simulate_keystrokes("enter");
        cx.run_until_parked();
        let id: SearchId = rig
            .engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::HandleSearch { search, .. } => Some(search),
                _ => None,
            })
            .next_back()
            .expect("a search was sent");
        let chunk: Arc<[HandleHit]> = hits.into();
        let state = Arc::new(HandleSearchState {
            search: id,
            phase: HandlePhase::Done,
            progress_pct: 100,
            total_handles: 100,
            chunks: Arc::from([chunk]),
            searched: 2,
            total: 2,
            skipped: 0,
        });
        rig.engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.details);
            d.handle_search = Some(state);
            d.header.version += 1;
        });
        cx.run_until_parked();
        hs_draw(&mut rig.win);
    }

    /// Makes hit ids `ids` the selection.
    fn hs_select(rig: &mut HsRig, ids: &[u32]) {
        let table = rig.win.read(|cx| rig.view.read(cx).table().clone());
        rig.win.update(|_, cx| {
            table.update(cx, |t, cx| {
                for (n, id) in ids.iter().enumerate() {
                    if n == 0 {
                        t.selection.click(*id);
                    } else {
                        t.selection.ctrl_click(*id);
                    }
                }
                cx.notify();
            })
        });
        rig.win.run_until_parked();
    }

    fn hs_window_ids(rig: &mut HsRig) -> Vec<WindowId> {
        rig.win
            .update(|_, cx| cx.windows().iter().map(|w| w.window_id()).collect())
    }

    /// Focuses the table and presses `keys`. Returns the confirmation it opened.
    fn hs_press(rig: &mut HsRig, keys: &str) -> Option<Dialog> {
        let before = hs_window_ids(rig);
        let table = rig.win.read(|cx| rig.view.read(cx).table().clone());
        rig.win.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        hs_draw(&mut rig.win);
        rig.win.simulate_keystrokes(keys);
        rig.win.run_until_parked();
        let id = hs_window_ids(rig)
            .into_iter()
            .find(|w| !before.contains(w))?;
        let handle = WindowHandle::<Root>::new(id);
        let view = rig.win.update(|_, cx| {
            TlSecondaryWindow::<TlDialogView>::find(handle, cx)
                .map(|f| f.read(cx).content().clone())
        })?;
        let any = rig
            .win
            .update(|_, cx| cx.windows())
            .into_iter()
            .find(|w| w.window_id() == id)?;
        let mut vcx = VisualTestContext::from_window(any, &rig.win.cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        Some((vcx, view))
    }

    // ---- the test ----

    /// Security regression (UI-PWN-025, UI-PRC-014, R-104): every confirmation a process window
    /// triggers shows its fixed texts and focus, Esc sends nothing, and Enter runs the focused
    /// button. `cx_*` are one app context per trigger family.
    #[gpui_kit::test]
    fn pwn_triggers_use_prc014_texts(
        cx_rt: &mut TestAppContext,
        cx_sig: &mut TestAppContext,
        cx_hs: &mut TestAppContext,
        cx_dump: &mut TestAppContext,
        cx_vt: &mut TestAppContext,
    ) {
        // Realtime from the Custom priority dialog (RR).
        {
            let kind = ConfirmKind::Realtime;
            let e = Expect {
                heading: "Set realtime priority?",
                text: "Realtime priority can freeze the desktop if the process spins. Set it anyway?",
                button: "Set realtime",
                danger: true,
                focus: DialogButton::Cancel,
            };
            let mut rig = priority::open(cx_rt, priority::NORMAL);
            rig.update(|d, _, cx| {
                d.set_policy(SchedPolicy::Rr, cx);
                d.set_rt_prio(10, cx);
            });
            rig.apply();
            let mut dialog = rig.confirmation().expect("realtime confirmation");
            check(&mut dialog, &kind, &e);
            press_in(&mut dialog, cx_rt, "escape");
            assert!(rig.sent().is_empty(), "realtime: Esc sends nothing");
            assert!(rig.confirmation().is_none(), "realtime: Esc closes it");
            rig.apply();
            let mut dialog = rig.confirmation().expect("realtime confirmation again");
            press_in(&mut dialog, cx_rt, "enter");
            assert!(
                rig.sent().is_empty(),
                "realtime: Enter on Cancel sends nothing"
            );
        }

        // Signal cases: (signal, targets, names, kind, expected, Enter sends).
        let mut sig = signal::rig(cx_sig);
        let other = OtherUserTarget {
            name: "sshd".into(),
            pid: 10_000,
            user: "root".into(),
        };
        let cases: Vec<(i32, u32, TargetNames, ConfirmKind, Expect)> = vec![
            (
                15,
                1,
                TargetNames::process("plasmashell", 10_000).with_protected("plasmashell"),
                ConfirmKind::Protected {
                    action: "Send signal".into(),
                    name: "plasmashell".into(),
                    tree_count: None,
                },
                Expect {
                    heading: "End plasmashell?",
                    text: "Ending plasmashell will end your desktop session. End it anyway?",
                    button: "End anyway",
                    danger: true,
                    focus: DialogButton::Cancel,
                },
            ),
            (
                15,
                1,
                TargetNames::process("sshd", 10_000).with_other_user(other),
                ConfirmKind::OtherUser {
                    action: "Send signal".into(),
                    name: "sshd".into(),
                    pid: 10_000,
                    user: "root".into(),
                    destructive: true,
                },
                Expect {
                    heading: "Act on another user's process?",
                    text: "Send signal on sshd (PID 10000), owned by root. Continue?",
                    button: "Send signal",
                    danger: true,
                    focus: DialogButton::Cancel,
                },
            ),
            (
                1,
                4,
                TargetNames::processes(4),
                ConfirmKind::SignalMany {
                    count: 4,
                    signal_name: "HUP".into(),
                    ends_process: true,
                },
                Expect {
                    heading: "Send signal to 4 processes?",
                    text: "Send SIGHUP to 4 selected processes?",
                    button: "Send signal",
                    danger: true,
                    focus: DialogButton::Cancel,
                },
            ),
            (
                18,
                4,
                TargetNames::processes(4),
                ConfirmKind::SignalMany {
                    count: 4,
                    signal_name: "CONT".into(),
                    ends_process: false,
                },
                Expect {
                    heading: "Send signal to 4 processes?",
                    text: "Send SIGCONT to 4 selected processes?",
                    button: "Send signal",
                    danger: false,
                    focus: DialogButton::Action,
                },
            ),
        ];
        for (signo, count, names, kind, e) in cases {
            let before = sig.sent().len();
            sig.send(signo, signal::Rig::targets(count), names.clone());
            let mut dialog = sig.confirmation();
            check(&mut dialog, &kind, &e);
            press_in(&mut dialog, cx_sig, "escape");
            assert_eq!(sig.sent().len(), before, "{}: Esc sends nothing", e.heading);

            sig.send(signo, signal::Rig::targets(count), names);
            let mut dialog = sig.confirmation();
            press_in(&mut dialog, cx_sig, "enter");
            let expected = before + usize::from(e.focus == DialogButton::Action);
            assert_eq!(
                sig.sent().len(),
                expected,
                "{}: Enter runs the focused button",
                e.heading
            );
        }

        // Handle Search End Task with 2 targets.
        {
            let kind = ConfirmKind::EndMany {
                count: 2,
                kill: false,
            };
            let e = Expect {
                heading: "End 2 processes?",
                text: "End 2 selected processes?",
                button: "End tasks",
                danger: true,
                focus: DialogButton::Cancel,
            };
            let mut rig = hs_setup(cx_hs);
            hs_show(
                cx_hs,
                &mut rig,
                &[300, 301],
                vec![
                    hs_hit(300, 3, "/var/log/a.log"),
                    hs_hit(301, 4, "/var/log/b.log"),
                ],
            );
            hs_select(&mut rig, &[0, 1]);
            let mut dialog = hs_press(&mut rig, "delete").expect("the End many confirmation");
            check(&mut dialog, &kind, &e);
            press_in(&mut dialog, cx_hs, "escape");
            assert_eq!(
                action_count(&rig.engine),
                0,
                "Handle Search: Esc sends nothing"
            );

            let mut dialog = hs_press(&mut rig, "delete").expect("the confirmation again");
            press_in(&mut dialog, cx_hs, "enter");
            assert_eq!(
                action_count(&rig.engine),
                0,
                "Handle Search: Enter on Cancel sends nothing"
            );
        }

        // Large dump (VmRSS 3.4 GiB).
        {
            let kind = ConfirmKind::LargeDump {
                name: "big".into(),
                size: "3.40 GiB".into(),
            };
            let e = Expect {
                heading: "Create a large dump?",
                text: "big uses 3.40 GiB. The process pauses while the core is written and the \
                       file will be about that size. Continue?",
                button: "Create dump",
                danger: false,
                focus: DialogButton::Action,
            };
            let mut r = dump::rig(
                cx_dump,
                Path::new("/s/tasklens"),
                tl_model::actions::DumpMode::Mini,
                vec![dump::record(200, "big", 34 * dump::GIB / 10)],
            );
            dump::start(&mut r, 200);
            let mut dialog = dump::confirmation(&r, cx_dump).expect("the dump confirmation");
            check(&mut dialog, &kind, &e);
            press_in(&mut dialog, cx_dump, "escape");
            assert!(dump::actions(&r).is_empty(), "dump: Esc sends nothing");

            dump::start(&mut r, 200);
            let mut dialog = dump::confirmation(&r, cx_dump).expect("the confirmation again");
            press_in(&mut dialog, cx_dump, "enter");
            let sent = dump::actions(&r);
            assert_eq!(sent.len(), 1, "dump: Enter runs Create dump: {sent:?}");
            assert!(matches!(sent[0].kind, ActionKind::Dump { .. }));
            assert_eq!(
                sent[0].targets.as_slice(),
                &[ActionTarget::Process(dump::key(200))]
            );
        }

        // VirusTotal first use.
        {
            let kind = ConfirmKind::VirusTotalFirstUse;
            let e = Expect {
                heading: "Open VirusTotal?",
                text: "This opens virustotal.com with the file's hash. The file is not uploaded.",
                button: "Open",
                danger: false,
                focus: DialogButton::Action,
            };
            let mut r = virustotal::rig(cx_vt, false);
            virustotal::open(&mut r);
            let mut dialog = vt_dialog(&r, cx_vt);
            check(&mut dialog, &kind, &e);
            press_in(&mut dialog, cx_vt, "escape");
            assert_eq!(
                virustotal::hash_requests(&r),
                0,
                "VirusTotal: Esc sends nothing"
            );
            assert!(
                !virustotal::accepted(cx_vt),
                "VirusTotal: Esc keeps the notice"
            );

            virustotal::open(&mut r);
            let mut dialog = vt_dialog(&r, cx_vt);
            press_in(&mut dialog, cx_vt, "enter");
            assert!(virustotal::accepted(cx_vt), "VirusTotal: Enter accepts");
        }
    }

    /// The VirusTotal notice's dialog view.
    fn vt_dialog(r: &virustotal::Rig, cx: &mut TestAppContext) -> Dialog {
        let (handle, vcx) = virustotal::notice(r, cx).expect("the notice window");
        let frame = cx
            .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
            .expect("a dialog frame");
        let view = cx.read(|cx| frame.read(cx).content().clone());
        (vcx, view)
    }
}
