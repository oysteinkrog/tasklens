//! Service Properties window frame tests (E27-A3, UI-SYS-030), General tab tests (UI-SYS-031) and
//! the other tabs' tests up to Logs (UI-SYS-036).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, Window, WindowHandle, div,
    point, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    LaunchWhat, RecoveryEdit,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::health::HelperState;
use tl_model::journal::{JournalAccess, JournalControl, JournalEntry, JournalError, JournalEvent};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{
    DepEntry, PidIds, RecoveryKeySet, RecoverySettings, Relation, ServiceBackend, ServiceStatus,
    StartupType, UnitDetails, UnitRecord,
};
use tl_model::table::{Manager, Total, UnitRef, UnitType};
use tl_model::validity::Validity;
use tl_ui::actions::OpenAdditionalSetup;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::{DialogButton, TlDialogView};
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::demand::GlobalDemand;
use tl_ui::pages::sys_status::{DotStyle, LabelTone};
use tl_ui::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{main_window_view, open_main_window_on};
use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
use tl_ui::window::setup::SetupRow;
use tl_ui::windows::service_properties::{
    DependenciesTab, FollowStateForTest, GeneralTab, LimitAction, LogOnTab, LogRowForTest,
    LogsStatusForTest, LogsView, RecoveryForm, RecoveryTab, RestartPolicy, ServicePropertiesWindow,
    ServicePropsTab, ServicePropsTabView, changed_keys, validate_on_failure,
};

use tl_ui::pages::services::actions::ServicePropsOk;

fn a(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn unit(name: &str) -> UnitRef {
    UnitRef {
        manager: Manager::System,
        name: a(name),
    }
}

fn missing<T: Default>() -> Total<T> {
    Total {
        value: T::default(),
        validity: Validity::Missing,
    }
}

fn record(name: &str) -> UnitRecord {
    UnitRecord {
        unit: unit(name),
        kind: UnitType::Service,
        description: a("Unit"),
        status: ServiceStatus::Running,
        sub_state: a("running"),
        load_state: a("loaded"),
        startup: StartupType::Enabled,
        main_pid: 812,
        user: a("root"),
        path: a("/usr/bin/daemon"),
        unit_file: None,
        active_since_us: None,
        triggered_by: Arc::from([]),
        control_group: None,
        can_reload: false,
        has_cgroup_procs: false,
        cgroup_pids: Arc::from([]),
        cpu_pct: missing(),
        memory_bytes: missing(),
        disk_bps: missing(),
        tasks: missing(),
    }
}

fn details(name: &str) -> Arc<UnitDetails> {
    Arc::new(UnitDetails {
        id: a(name),
        description: a("Unit"),
        documentation: Arc::from([]),
        exec_start_argv: Arc::from([]),
        fragment_path: None,
        drop_in_paths: Arc::from([]),
        startup: StartupType::Enabled,
        active_state: a("active"),
        sub_state: a("running"),
        active_enter_timestamp_us: None,
        main_pid: 812,
        memory_current: None,
        tasks_current: None,
        cpu_usage_nsec: None,
        manager: Manager::System,
        user: None,
        group: None,
        dynamic_user: false,
        supplementary_groups: Arc::from([]),
        pam_name: None,
        main_pid_ids: None,
        graphical_session: false,
        recovery: RecoverySettings::default(),
        recovery_dropin_exists: false,
        recovery_dropin_keys: Default::default(),
        deps: Arc::from([]),
    })
}

fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
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
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Services, cx))
        .expect("the main window opens");
    let mut main = VisualTestContext::from_window(handle.into(), cx);
    main.update(|window, _| window.activate_window());
    main.run_until_parked();
    (engine, main)
}

/// Publishes the units and the details of those in `with_details`. Always bumps the version.
fn publish(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
    units: &[&str],
    with_details: &HashMap<UnitRef, Arc<UnitDetails>>,
    backend: ServiceBackend,
) {
    let records: Vec<UnitRecord> = units.iter().map(|name| record(name)).collect();
    let map = with_details.clone();
    engine.publish_with(move |s| {
        let services = Arc::make_mut(&mut s.services);
        services.units = records.into();
        services.details = Arc::new(map);
        services.backend = backend;
        services.header.version += 1;
    });
    cx.run_until_parked();
}

fn open(cx: &mut TestAppContext, name: &str, tab: ServicePropsTab) -> Option<WindowHandle<Root>> {
    cx.update(|cx| ServicePropertiesWindow::open(unit(name), tab, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let registry = cx.global::<GlobalRegistry>().0.clone();
        let handle = registry.read(cx).get(&WindowKey::Service(unit(name)));
        handle.and_then(|h| h.downcast::<Root>())
    })
}

fn view(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Entity<ServicePropertiesWindow> {
    cx.update(|cx| ServicePropertiesWindow::find(handle, cx))
        .expect("service properties")
}

fn title(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
    cx.update(|cx| {
        TlSecondaryWindow::<ServicePropertiesWindow>::find(handle, cx)
            .expect("frame")
            .read(cx)
            .title()
            .to_string()
    })
}

fn count(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| {
        let registry = cx.global::<GlobalRegistry>().0.clone();
        registry.read(cx).count(CapClass::ServiceProps)
    })
}

fn main_toasts(main: &mut VisualTestContext) -> Vec<String> {
    main.update(|_, cx| {
        let view = main_window_view(cx).expect("main window");
        let texts = view.read(cx).toasts().read(cx).visible_texts();
        texts.iter().map(|t| t.to_string()).collect()
    })
}

fn demanded(cx: &mut TestAppContext) -> Vec<UnitRef> {
    cx.update(|cx| {
        let demand = cx.global::<GlobalDemand>().0.clone();
        demand.read(cx).merged().unit_details.to_vec()
    })
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

fn units_named(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("svc{i}.service")).collect()
}

/// A tab view that is dirty and submits one action on apply.
struct TestTab {
    dirty: bool,
    actions: Entity<SysActions>,
    sent: Rc<Cell<Option<ActionId>>>,
    resets: Rc<Cell<u32>>,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for TestTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ServicePropsTabView for TestTab {
    fn is_dirty(&self, _: &gpui_kit::App) -> bool {
        self.dirty
    }

    fn apply(&mut self, window: &mut Window, cx: &mut gpui_kit::App) -> SmallVec<[ActionId; 2]> {
        self.log.borrow_mut().push("apply");
        let texts = ActionTexts {
            pending_one: "Starting service...".into(),
            pending_many: "sys-sys007-starting-n-services",
            done_one: "Service started".into(),
            done_many: "sys-sys007-n-services-started",
            partial_many: "sys-sys007-ok-of",
            failed_prefix: "Failed to start service".into(),
            failed_suffix: true,
            unknown: "Service action finished.".into(),
            busy: "A service action is already in progress.".into(),
            failed_button: None,
        };
        let targets = smallvec![ActionTarget::Unit(unit("sshd.service"))];
        let id = self.actions.update(cx, |actions, cx| {
            actions.submit(
                ActionKind::UnitStart,
                ActionKindTag::Start,
                targets,
                texts,
                window,
                cx,
            )
        });
        self.sent.set(id);
        id.into_iter().collect()
    }

    fn reset(&mut self, _: &mut gpui_kit::App) {
        self.dirty = false;
        self.resets.set(self.resets.get() + 1);
    }
}

struct Dirty {
    sent: Rc<Cell<Option<ActionId>>>,
    resets: Rc<Cell<u32>>,
}

struct Rig {
    engine: Arc<FakeEngine>,
    main: VisualTestContext,
    handle: WindowHandle<Root>,
    win: Entity<ServicePropertiesWindow>,
    dirty: Dirty,
}

/// Opens sshd.service with a dirty test tab on Recovery.
fn open_dirty(cx: &mut TestAppContext) -> Rig {
    let (engine, main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    let dirty = Dirty {
        sent: Rc::default(),
        resets: Rc::default(),
    };
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let (sent, resets) = (dirty.sent.clone(), dirty.resets.clone());
    vcx.update(|window, cx| {
        win.update(cx, |w, cx| {
            let actions = w.actions().clone();
            let tab = cx.new(|_| TestTab {
                dirty: true,
                actions,
                sent,
                resets,
                log: Rc::default(),
            });
            w.install_tab_view(ServicePropsTab::Recovery, tab, cx);
            w.select_tab(ServicePropsTab::Recovery, window, cx);
        })
    });
    draw(&mut vcx);
    drop(vcx);
    Rig {
        engine,
        main,
        handle,
        win,
        dirty,
    }
}

fn press_ok(rig: &mut Rig, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(rig.handle.into(), cx);
    vcx.dispatch_action(ServicePropsOk);
    vcx.run_until_parked();
}

fn finish(rig: &Rig, cx: &mut TestAppContext, state: ActionState) {
    let id = rig.dirty.sent.get().expect("an action was submitted");
    rig.engine.emit(ActionEvent {
        id,
        target: None,
        state,
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn service_props_demand_unit_details(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    assert_eq!(title(cx, handle), "sshd.service Properties");
    let (width, height) = cx.update(|cx| {
        handle
            .update(cx, |_, window, _| {
                let b = window.bounds().size;
                (f32::from(b.width), f32::from(b.height))
            })
            .expect("window")
    });
    let scale = cx.update(|cx| tl_ui::theme::metrics(cx).ui_scale);
    let extra = f32::from(tl_ui::components::window_frame::client_frame_extra(
        tl_model::settings::Decorations::Client,
    ));
    assert_eq!(
        (width, height),
        (720. * scale + extra, 560. * scale + extra)
    );
    {
        let win = view(cx, handle);
        cx.read(|cx| {
            let w = win.read(cx);
            assert_eq!(w.visible_tabs().len(), 5);
            assert_eq!(w.active(), ServicePropsTab::General);
        });
    }
    cx.run_until_parked();
    assert_eq!(demanded(cx), vec![unit("sshd.service")]);

    handle
        .update(cx, |_, window, _| window.remove_window())
        .expect("window");
    cx.run_until_parked();
    assert!(demanded(cx).is_empty(), "demand removed on close");
}

#[gpui_kit::test]
fn service_props_focus_existing(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let again = open(cx, "sshd.service", ServicePropsTab::Logs).expect("still open");
    assert_eq!(again.window_id(), handle.window_id());
    assert_eq!(count(cx), 1);
    let win = view(cx, handle);
    assert_eq!(cx.read(|cx| win.read(cx).active()), ServicePropsTab::Logs);
}

#[gpui_kit::test]
fn service_props_cap_10(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    let names = units_named(11);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    publish(cx, &engine, &refs, &HashMap::new(), ServiceBackend::Systemd);
    for name in &names[..10] {
        assert!(open(cx, name, ServicePropsTab::General).is_some());
    }
    main.update(|window, _| window.activate_window());
    main.run_until_parked();
    assert!(open(cx, &names[10], ServicePropsTab::General).is_none());
    assert_eq!(count(cx), 10);
    assert!(
        main_toasts(&mut main)
            .iter()
            .any(|t| t == "Too many service properties windows are already open"),
        "{:?}",
        main_toasts(&mut main)
    );
}

#[gpui_kit::test]
fn service_props_rerender_only_on_own_details(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    let own = details("sshd.service");
    let mut map = HashMap::new();
    map.insert(unit("sshd.service"), own.clone());
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    let notified = Rc::new(Cell::new(0u32));
    let seen = notified.clone();
    cx.update(|cx| {
        cx.observe(&win, move |_, _| seen.set(seen.get() + 1))
            .detach();
    });

    let renders = |cx: &mut TestAppContext| cx.read(|cx| win.read(cx).render_count());
    let before = renders(cx);

    // A new version with the same details Arc.
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    assert_eq!(notified.get(), 0, "same details Arc, no notify");
    assert_eq!(renders(cx), before, "same details Arc, no render");

    let mut changed = HashMap::new();
    changed.insert(unit("sshd.service"), details("sshd.service"));
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &changed,
        ServiceBackend::Systemd,
    );
    assert_eq!(notified.get(), 1, "new details Arc, one notify");
    assert!(renders(cx) > before, "new details Arc, renders again");
}

#[gpui_kit::test]
fn service_props_not_loaded_title(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    publish(cx, &engine, &[], &HashMap::new(), ServiceBackend::Systemd);
    assert_eq!(title(cx, handle), "sshd.service Properties (not loaded)");
    cx.read(|cx| {
        assert!(win.read(cx).is_not_loaded());
        assert!(!win.read(cx).apply_enabled(cx));
    });
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    assert_eq!(title(cx, handle), "sshd.service Properties");
}

#[gpui_kit::test]
fn service_props_failed_apply_stays_on_tab(cx: &mut TestAppContext) {
    let mut rig = open_dirty(cx);
    rig.win.read_with(cx, |w, cx| assert!(w.apply_enabled(cx)));
    press_ok(&mut rig, cx);
    assert!(rig.dirty.sent.get().is_some(), "apply submitted an action");
    finish(&rig, cx, ActionState::Failed(ActionError::PermissionDenied));
    assert_eq!(count(cx), 1, "window stays open");
    rig.win.read_with(cx, |w, _| {
        assert_eq!(w.active(), ServicePropsTab::Recovery);
    });
    assert_eq!(rig.dirty.resets.get(), 0, "a failed apply keeps the edits");
    drop(rig.main);
}

#[gpui_kit::test]
fn service_props_ok_applies_then_closes(cx: &mut TestAppContext) {
    let mut rig = open_dirty(cx);
    press_ok(&mut rig, cx);
    assert_eq!(count(cx), 1, "still open until the action finishes");
    finish(&rig, cx, ActionState::Succeeded);
    assert_eq!(rig.dirty.resets.get(), 1, "the tab was reset");
    assert_eq!(count(cx), 0, "closed after success");
    drop(rig.main);
}

#[gpui_kit::test]
fn service_props_openrc_general_only(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::OpenRc,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    cx.read(|cx| {
        assert_eq!(
            win.read(cx).visible_tabs().as_slice(),
            &[ServicePropsTab::General]
        );
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_keystrokes("ctrl-3");
    assert_eq!(
        vcx.read(|cx| win.read(cx).active()),
        ServicePropsTab::General
    );
}

#[gpui_kit::test]
fn service_props_tab_keys(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    draw(&mut vcx);
    let active = |vcx: &mut VisualTestContext| vcx.read(|cx| win.read(cx).active());

    vcx.simulate_keystrokes("ctrl-3");
    assert_eq!(active(&mut vcx), ServicePropsTab::Recovery);
    vcx.simulate_keystrokes("ctrl-tab");
    assert_eq!(active(&mut vcx), ServicePropsTab::Dependencies);
    vcx.simulate_keystrokes("ctrl-pageup");
    assert_eq!(active(&mut vcx), ServicePropsTab::Recovery);
    vcx.simulate_keystrokes("ctrl-shift-tab");
    assert_eq!(active(&mut vcx), ServicePropsTab::LogOn);
    vcx.simulate_keystrokes("ctrl-5 ctrl-pagedown");
    assert_eq!(active(&mut vcx), ServicePropsTab::General, "wraps around");
}

// General tab (UI-SYS-031).

const GENERAL_LABELS: [&str; 13] = [
    "Name",
    "Display",
    "Desc",
    "Path",
    "Unit file",
    "Drop-ins",
    "Startup",
    "Status",
    "Main PID",
    "Memory",
    "Tasks",
    "CPU time",
    "Manager",
];

/// `details(name)` changed by `f`.
fn details_with(name: &str, f: impl FnOnce(&mut UnitDetails)) -> Arc<UnitDetails> {
    let mut d = Arc::into_inner(details(name)).expect("one owner");
    f(&mut d);
    Arc::new(d)
}

/// sshd.service as systemd reports it on a running system.
fn sshd() -> Arc<UnitDetails> {
    details_with("sshd.service", |d| {
        d.description = a("OpenSSH Daemon");
        d.documentation = Arc::from([a("man:sshd(8)"), a("man:sshd_config(5)")]);
        d.exec_start_argv = Arc::from([a("/usr/bin/sshd"), a("-D")]);
        d.fragment_path = Some(a("/usr/lib/systemd/system/sshd.service"));
        d.drop_in_paths = Arc::from([a("/etc/systemd/system/sshd.service.d/override.conf")]);
        d.active_enter_timestamp_us = Some(1_700_000_000_000_000);
        d.memory_current = Some(5 * 1024 * 1024);
        d.tasks_current = Some(3);
        d.cpu_usage_nsec = Some(3_723_000_000_000);
    })
}

/// A unit with nothing reported: no docs, no drop-ins, no unit file, not running.
fn bare_inactive() -> Arc<UnitDetails> {
    details_with("sshd.service", |d| {
        d.active_state = a("inactive");
        d.sub_state = a("dead");
        d.main_pid = 0;
        d.active_enter_timestamp_us = Some(1_700_000_000_000_000);
    })
}

struct General {
    engine: Arc<FakeEngine>,
    _main: VisualTestContext,
    handle: WindowHandle<Root>,
    win: Entity<ServicePropertiesWindow>,
    tab: Entity<GeneralTab>,
}

/// Opens sshd.service on General with `d` published, or with no details.
fn open_general(cx: &mut TestAppContext, d: Option<Arc<UnitDetails>>) -> General {
    let (engine, main) = setup(cx);
    let mut map = HashMap::new();
    if let Some(d) = d {
        map.insert(unit("sshd.service"), d);
    }
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::General).expect("opened");
    let win = view(cx, handle);
    let tab = cx
        .read(|cx| win.read(cx).general_tab())
        .expect("the General tab is built");
    General {
        engine,
        _main: main,
        handle,
        win,
        tab,
    }
}

impl General {
    fn rows(&self, cx: &mut TestAppContext) -> Vec<(String, String)> {
        cx.read(|cx| {
            self.tab
                .read(cx)
                .rows_for_test(cx)
                .into_iter()
                .map(|(l, v)| (l.to_string(), v.to_string()))
                .collect()
        })
    }

    fn value(&self, cx: &mut TestAppContext, label: &str) -> String {
        let rows = self.rows(cx);
        let row = rows
            .iter()
            .find(|(l, _)| l.trim_end_matches(':') == label)
            .unwrap_or_else(|| panic!("no {label} row"));
        row.1.clone()
    }

    fn vcx(&self, cx: &mut TestAppContext) -> VisualTestContext {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        vcx
    }

    fn click(&self, cx: &mut TestAppContext, selector: &'static str) {
        let mut vcx = self.vcx(cx);
        let bounds = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"));
        vcx.simulate_click(bounds.center(), Modifiers::none());
        vcx.run_until_parked();
    }

    fn sent(&self) -> Vec<(ActionId, ActionKind)> {
        self.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(r) => Some((r.id, r.kind)),
                _ => None,
            })
            .collect()
    }

    /// `Started`, `state` for the one target, then the summary.
    fn finish(&self, cx: &mut TestAppContext, id: ActionId, state: ActionState) {
        for (target, state) in [
            (None, ActionState::Started),
            (Some(0), state.clone()),
            (None, state),
        ] {
            self.engine.emit(ActionEvent { id, target, state });
            cx.run_until_parked();
        }
    }

    /// Every toast this window's action layer pushed, oldest first.
    fn toasts(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.read(|cx| {
            let actions = self.win.read(cx).actions().clone();
            let pushed = actions.read(cx).pushed();
            pushed.iter().map(|t| t.text.to_string()).collect()
        })
    }
}

#[gpui_kit::test]
fn general_tab_labels_golden(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    let rows = g.rows(cx);
    let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
    let expected: Vec<String> = GENERAL_LABELS.iter().map(|l| format!("{l}:")).collect();
    assert_eq!(labels, expected);

    assert_eq!(g.value(cx, "Name"), "sshd.service");
    assert_eq!(g.value(cx, "Display"), "OpenSSH Daemon");
    assert_eq!(g.value(cx, "Desc"), "man:sshd(8)  man:sshd_config(5)");
    assert_eq!(g.value(cx, "Path"), "/usr/bin/sshd -D");
    assert_eq!(
        g.value(cx, "Unit file"),
        "/usr/lib/systemd/system/sshd.service"
    );
    assert_eq!(
        g.value(cx, "Drop-ins"),
        "/etc/systemd/system/sshd.service.d/override.conf"
    );
    assert_eq!(g.value(cx, "Startup"), "Enabled");
    let status = g.value(cx, "Status");
    let since = status
        .strip_prefix("Running (running) since ")
        .unwrap_or_else(|| panic!("status {status:?}"));
    assert_eq!(since.len(), "2023-11-14 22:13:20".len(), "{since:?}");
    assert_eq!(g.value(cx, "Main PID"), "812");
    assert_eq!(g.value(cx, "Memory"), "5.00 MiB");
    assert_eq!(g.value(cx, "Tasks"), "3");
    assert_eq!(g.value(cx, "CPU time"), "1:02:03");
    assert_eq!(g.value(cx, "Manager"), "System");
    let _ = g.vcx(cx);
    assert!(
        cx.read(|cx| g.tab.read(cx).render_count()) > 0,
        "the tab drew"
    );
}

#[gpui_kit::test]
fn general_tab_missing_values(cx: &mut TestAppContext) {
    let g = open_general(cx, None);
    for (label, value) in g.rows(cx) {
        assert_eq!(value, "Collecting…", "{label} before the details arrive");
    }

    publish(
        cx,
        &g.engine,
        &["sshd.service"],
        &HashMap::from([(unit("sshd.service"), bare_inactive())]),
        ServiceBackend::Systemd,
    );
    assert_eq!(g.value(cx, "Desc"), "Not reported");
    assert_eq!(g.value(cx, "Drop-ins"), "None");
    assert_eq!(g.value(cx, "Unit file"), "None");
    assert_eq!(g.value(cx, "Main PID"), "None");
    assert_eq!(g.value(cx, "Status"), "Stopped (dead)");
    assert_eq!(g.value(cx, "Memory"), "Not reported");
    assert_eq!(g.value(cx, "Tasks"), "Not reported");
    assert_eq!(g.value(cx, "CPU time"), "Not reported");
}

#[gpui_kit::test]
fn startup_combo_disabled_for_static(cx: &mut TestAppContext) {
    let g = open_general(
        cx,
        Some(details_with("sshd.service", |d| {
            d.startup = StartupType::Static;
        })),
    );
    let (enabled, tooltip) = cx.read(|cx| g.tab.read(cx).combo_for_test(cx));
    assert!(!enabled, "a Static unit's combo is disabled");
    assert_eq!(tooltip.as_deref(), Some("This unit has no install section"));
    assert_eq!(g.value(cx, "Startup"), "Static");

    // "Enabled (runtime)" shows "Enabled" and can change.
    publish(
        cx,
        &g.engine,
        &["sshd.service"],
        &HashMap::from([(
            unit("sshd.service"),
            details_with("sshd.service", |d| d.startup = StartupType::EnabledRuntime),
        )]),
        ServiceBackend::Systemd,
    );
    let (enabled, tooltip) = cx.read(|cx| g.tab.read(cx).combo_for_test(cx));
    assert!(enabled);
    assert_eq!(tooltip, None);
    assert_eq!(g.value(cx, "Startup"), "Enabled");
}

#[gpui_kit::test]
fn apply_startup_toast_changes_applied(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    cx.read(|cx| assert!(!g.win.read(cx).apply_enabled(cx), "clean at first"));
    g.tab.update(cx, |t, cx| t.choose_for_test("disabled", cx));
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(g.tab.read(cx).is_dirty(cx));
        assert!(
            g.win.read(cx).apply_enabled(cx),
            "a new choice enables Apply"
        );
    });

    g.click(cx, "TlButton-ServicePropsApply");
    let sent = g.sent();
    let (id, kind) = sent.last().cloned().expect("a request was sent");
    assert_eq!(kind, ActionKind::UnitDisable);
    g.finish(cx, id, ActionState::Succeeded);

    let toasts = g.toasts(cx);
    assert!(toasts.iter().any(|t| t == "Changes applied"), "{toasts:?}");
    assert!(
        !toasts.iter().any(|t| t == "Startup type changed"),
        "{toasts:?}"
    );
    cx.read(|cx| {
        assert!(!g.tab.read(cx).is_dirty(cx), "the tab is clean");
        assert!(!g.win.read(cx).apply_enabled(cx), "Apply is off again");
    });
    assert_eq!(
        g.value(cx, "Startup"),
        "Disabled",
        "shows the applied value"
    );
    assert_eq!(count(cx), 1, "Apply keeps the window open");
}

#[gpui_kit::test]
fn general_buttons_pending_disabled(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(bare_inactive()));
    let buttons = |cx: &mut TestAppContext| cx.read(|cx| g.tab.read(cx).buttons_for_test(cx));
    assert_eq!(
        buttons(cx),
        [true, false, false],
        "Start only for an inactive unit"
    );

    g.click(cx, "TlButton-ServiceGeneralStart");
    let (id, kind) = g.sent().last().cloned().expect("Start was sent");
    assert_eq!(kind, ActionKind::UnitStart);
    assert_eq!(buttons(cx), [false, false, false], "all off while pending");
    assert_eq!(g.value(cx, "Status"), "Starting…");

    g.finish(cx, id, ActionState::Succeeded);
    assert_eq!(buttons(cx), [true, false, false]);
    assert_eq!(g.value(cx, "Status"), "Stopped (dead)");

    // A running unit: Stop and Restart. Not loaded: none.
    publish(
        cx,
        &g.engine,
        &["sshd.service"],
        &HashMap::from([(unit("sshd.service"), sshd())]),
        ServiceBackend::Systemd,
    );
    assert_eq!(buttons(cx), [false, true, true]);
    publish(cx, &g.engine, &[], &HashMap::new(), ServiceBackend::Systemd);
    assert_eq!(buttons(cx), [false, false, false], "not loaded");
}

#[gpui_kit::test]
fn general_links_send_open_uri(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    g.click(cx, "ServiceGeneral-link-2-0");
    let kinds: Vec<ActionKind> = g.sent().into_iter().map(|(_, k)| k).collect();
    assert_eq!(kinds, vec![ActionKind::OpenUri(a("man:sshd(8)"))]);

    g.click(cx, "ServiceGeneral-link-4-0");
    let kinds: Vec<ActionKind> = g.sent().into_iter().map(|(_, k)| k).collect();
    assert_eq!(
        kinds.last(),
        Some(&ActionKind::OpenUri(a(
            "file:///usr/lib/systemd/system/sshd.service"
        )))
    );
    assert!(
        g.toasts(cx).is_empty(),
        "no success toast: {:?}",
        g.toasts(cx)
    );
}

#[gpui_kit::test]
fn apply_startup_rejected_keeps_choice_and_window(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    g.tab.update(cx, |t, cx| t.choose_for_test("disabled", cx));
    cx.run_until_parked();

    // A startup change already runs on this unit in this window, so the submit is refused.
    let mut vcx = g.vcx(cx);
    let actions = vcx.read_entity(&g.win, |w, _| w.actions().clone());
    let busy = vcx.update(|window, cx| {
        let texts = tl_ui::pages::services::flows::startup_texts(cx);
        let busy = texts.busy.to_string();
        let id = actions.update(cx, |a, cx| {
            a.submit(
                ActionKind::UnitMask,
                ActionKindTag::SetStartup,
                smallvec![ActionTarget::Unit(unit("sshd.service"))],
                texts,
                window,
                cx,
            )
        });
        assert!(id.is_some(), "the first change was sent");
        busy
    });
    vcx.dispatch_action(ServicePropsOk);
    vcx.run_until_parked();
    drop(vcx);

    assert!(g.toasts(cx).contains(&busy), "{:?}", g.toasts(cx));
    assert_eq!(count(cx), 1, "OK does not close when nothing was applied");
    cx.read(|cx| {
        assert!(g.tab.read(cx).is_dirty(cx), "the choice is kept");
        assert_eq!(g.win.read(cx).active(), ServicePropsTab::General);
    });
    assert_eq!(g.value(cx, "Startup"), "Disabled");
}

#[gpui_kit::test]
fn apply_startup_locks_combo_until_done(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    let combo = |cx: &mut TestAppContext| cx.read(|cx| g.tab.read(cx).combo_for_test(cx).0);
    g.tab.update(cx, |t, cx| t.choose_for_test("disabled", cx));
    cx.run_until_parked();
    g.click(cx, "TlButton-ServicePropsApply");
    let (id, _) = g.sent().last().cloned().expect("a request was sent");
    assert!(!combo(cx), "the combo is locked while the apply runs");

    // A pick while locked does not count.
    g.tab.update(cx, |t, cx| t.choose_for_test("masked", cx));
    cx.run_until_parked();
    assert_eq!(g.value(cx, "Startup"), "Disabled");

    g.finish(cx, id, ActionState::Failed(ActionError::PermissionDenied));
    assert!(combo(cx), "a failed apply unlocks the combo");
    cx.read(|cx| {
        assert!(
            g.tab.read(cx).is_dirty(cx),
            "a failed apply keeps the choice"
        );
        assert!(g.win.read(cx).apply_enabled(cx));
    });
    assert_eq!(g.value(cx, "Startup"), "Disabled");

    g.click(cx, "TlButton-ServicePropsApply");
    let (id, kind) = g.sent().last().cloned().expect("a second request was sent");
    assert_eq!(kind, ActionKind::UnitDisable);
    g.finish(cx, id, ActionState::Succeeded);
    assert!(combo(cx));
    cx.read(|cx| assert!(!g.tab.read(cx).is_dirty(cx)));
    assert_eq!(g.value(cx, "Startup"), "Disabled");
}

#[gpui_kit::test]
fn general_values_selectable(cx: &mut TestAppContext) {
    let g = open_general(cx, Some(sshd()));
    let mut vcx = g.vcx(cx);
    let path = vcx
        .debug_bounds("ServiceGeneral-path")
        .expect("Path value drawn");
    let (from, to) = (
        gpui_kit::point(path.left() + gpui_kit::px(1.), path.center().y),
        gpui_kit::point(path.right() - gpui_kit::px(1.), path.center().y),
    );
    vcx.simulate_mouse_down(from, gpui_kit::MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(to, Some(gpui_kit::MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(to, gpui_kit::MouseButton::Left, Modifiers::none());
    draw(&mut vcx);
    vcx.simulate_keystrokes("ctrl-c");
    vcx.run_until_parked();
    let copied = vcx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("/usr/bin/sshd -D"));
}

#[gpui_kit::test]
fn general_long_path_clamps_to_three_lines(cx: &mut TestAppContext) {
    let height = |cx: &mut TestAppContext, d: Arc<UnitDetails>| {
        let g = open_general(cx, Some(d));
        let mut vcx = g.vcx(cx);
        vcx.debug_bounds("ServiceGeneral-path")
            .expect("Path value drawn")
            .size
            .height
    };
    let short = height(cx, sshd());
    let long = height(
        cx,
        details_with("sshd.service", |d| {
            d.exec_start_argv = Arc::from(
                (0..400)
                    .map(|i| a(&format!("/usr/lib/very/long/argument/{i}")))
                    .collect::<Vec<_>>(),
            );
        }),
    );
    assert!(long > short, "a long path wraps");
    assert!(
        long <= short * 3.0 + gpui_kit::px(1.),
        "a long path is clamped to three lines: {long:?} vs {short:?}"
    );
}

struct LogOn {
    engine: Arc<FakeEngine>,
    _main: VisualTestContext,
    tab: Entity<LogOnTab>,
}

/// Opens sshd.service on Log On with `d` published, or with no details.
fn open_logon(cx: &mut TestAppContext, d: Option<Arc<UnitDetails>>) -> LogOn {
    let (engine, main) = setup(cx);
    let mut map = HashMap::new();
    if let Some(d) = d {
        map.insert(unit("sshd.service"), d);
    }
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::LogOn).expect("opened");
    let win = view(cx, handle);
    let tab = cx
        .read(|cx| win.read(cx).logon_tab())
        .expect("the Log On tab is built");
    LogOn {
        engine,
        _main: main,
        tab,
    }
}

impl LogOn {
    fn publish(&self, cx: &mut TestAppContext, d: Arc<UnitDetails>) {
        publish(
            cx,
            &self.engine,
            &["sshd.service"],
            &HashMap::from([(unit("sshd.service"), d)]),
            ServiceBackend::Systemd,
        );
    }

    fn rows(&self, cx: &mut TestAppContext) -> Vec<(String, String)> {
        cx.read(|cx| {
            self.tab
                .read(cx)
                .rows_for_test(cx)
                .into_iter()
                .map(|(l, v)| (l.to_string(), v.to_string()))
                .collect()
        })
    }

    fn value(&self, cx: &mut TestAppContext, label: &str) -> String {
        self.rows(cx)
            .into_iter()
            .find(|(l, _)| l.trim_end_matches(':') == label)
            .unwrap_or_else(|| panic!("no {label} row"))
            .1
    }
}

#[gpui_kit::test]
fn logon_missing_shows_not_reported(cx: &mut TestAppContext) {
    let t = open_logon(cx, None);
    for (label, value) in t.rows(cx) {
        assert_eq!(value, "Collecting…", "{label} before the details arrive");
    }

    t.publish(cx, details("sshd.service"));
    assert_eq!(t.value(cx, "Log on"), "root");
    assert_eq!(t.value(cx, "Group"), "Not reported");
    assert_eq!(t.value(cx, "Supplementary groups"), "Not reported");
    assert_eq!(t.value(cx, "Runs as"), "Not reported");
    assert_eq!(t.value(cx, "PAM service"), "Not reported");
    assert_eq!(t.value(cx, "Manager"), "System manager");
    assert_eq!(t.value(cx, "Session"), "none");
    for (label, value) in t.rows(cx) {
        assert!(!value.contains("N/A"), "{label} reads {value}");
    }
}

#[gpui_kit::test]
fn logon_runs_as_format(cx: &mut TestAppContext) {
    let t = open_logon(
        cx,
        Some(details_with("sshd.service", |d| {
            d.main_pid_ids = Some(PidIds {
                uid: 1000,
                gid: 1000,
            });
        })),
    );
    assert_eq!(t.value(cx, "Runs as"), "uid 1000, gid 1000");

    t.publish(
        cx,
        details_with("sshd.service", |d| {
            d.main_pid = 0;
            d.main_pid_ids = None;
        }),
    );
    assert_eq!(t.value(cx, "Runs as"), "Not running");
}

#[gpui_kit::test]
fn logon_dynamic_user(cx: &mut TestAppContext) {
    let t = open_logon(
        cx,
        Some(details_with("systemd-timesyncd.service", |d| {
            d.dynamic_user = true;
        })),
    );
    assert_eq!(t.value(cx, "Log on"), "dynamic user (systemd-timesyncd)");

    t.publish(
        cx,
        details_with("systemd-timesyncd.service", |d| {
            d.dynamic_user = true;
            d.user = Some(a("timesync"));
        }),
    );
    assert_eq!(t.value(cx, "Log on"), "dynamic user (timesync)");
}

#[gpui_kit::test]
fn logon_user_manager(cx: &mut TestAppContext) {
    let t = open_logon(
        cx,
        Some(details_with("sshd.service", |d| {
            d.manager = Manager::User;
            d.graphical_session = true;
            d.group = Some(a("users"));
            d.supplementary_groups = Arc::from([a("wheel"), a("video")]);
            d.pam_name = Some(a("systemd-user"));
        })),
    );
    t.tab.update(cx, |tab, cx| {
        tab.set_user_name_for_test(Some("oystein".into()), cx)
    });
    assert_eq!(t.value(cx, "Manager"), "User manager (oystein)");
    assert_eq!(t.value(cx, "Log on"), "oystein");
    assert_eq!(t.value(cx, "Session"), "graphical session");
    assert_eq!(t.value(cx, "Group"), "users");
    assert_eq!(t.value(cx, "Supplementary groups"), "wheel, video");
    assert_eq!(t.value(cx, "PAM service"), "systemd-user");
}

struct Recovery {
    engine: Arc<FakeEngine>,
    _main: VisualTestContext,
    handle: WindowHandle<Root>,
    win: Entity<ServicePropertiesWindow>,
    tab: Entity<RecoveryTab>,
}

fn publish_helper(cx: &mut TestAppContext, engine: &FakeEngine, helper: HelperState) {
    engine.publish_with(|s| {
        let health = Arc::make_mut(&mut s.health);
        health.helper = helper;
        health.version += 1;
    });
    cx.run_until_parked();
}

/// Opens sshd.service on Recovery with `d` published, or with no details, and the helper in
/// `helper`.
fn open_recovery_helper(
    cx: &mut TestAppContext,
    d: Option<Arc<UnitDetails>>,
    helper: HelperState,
) -> Recovery {
    let (engine, main) = setup(cx);
    publish_helper(cx, &engine, helper);
    let mut map = HashMap::new();
    if let Some(d) = d {
        map.insert(unit("sshd.service"), d);
    }
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::Recovery).expect("opened");
    let win = view(cx, handle);
    let tab = cx
        .read(|cx| win.read(cx).recovery_tab())
        .expect("the Recovery tab is built");
    Recovery {
        engine,
        _main: main,
        handle,
        win,
        tab,
    }
}

/// Opens sshd.service on Recovery with the helper installed but locked.
fn open_recovery(cx: &mut TestAppContext, d: Option<Arc<UnitDetails>>) -> Recovery {
    open_recovery_helper(cx, d, HelperState::Locked)
}

/// sshd.service with the Recovery values systemd reports for a typical service.
fn recovery_details(f: impl FnOnce(&mut UnitDetails)) -> Arc<UnitDetails> {
    details_with("sshd.service", |d| {
        d.recovery.restart = Some(a("on-failure"));
        d.recovery.restart_usec = Some(100_000);
        d.recovery.start_limit_burst = Some(5);
        d.recovery.start_limit_interval_usec = Some(10_000_000);
        d.recovery.start_limit_action = Some(a("none"));
        d.recovery.failure_action = Some(a("none"));
        d.recovery.n_restarts = Some(3);
        f(d);
    })
}

impl Recovery {
    fn rows(&self, cx: &mut TestAppContext) -> Vec<(String, String)> {
        cx.read(|cx| {
            self.tab
                .read(cx)
                .rows_for_test(cx)
                .into_iter()
                .map(|(l, v)| (l.to_string(), v.to_string()))
                .collect()
        })
    }

    fn value(&self, cx: &mut TestAppContext, label: &str) -> String {
        self.rows(cx)
            .into_iter()
            .find(|(l, _)| l.trim_end_matches(':') == label)
            .unwrap_or_else(|| panic!("no {label} row"))
            .1
    }

    fn publish(&self, cx: &mut TestAppContext, d: Arc<UnitDetails>) {
        publish(
            cx,
            &self.engine,
            &["sshd.service"],
            &HashMap::from([(unit("sshd.service"), d)]),
            ServiceBackend::Systemd,
        );
    }

    fn form(&self, cx: &mut TestAppContext) -> RecoveryForm {
        cx.read(|cx| self.tab.read(cx).form_for_test())
            .expect("the form is built")
    }

    fn edit(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut RecoveryForm)) {
        self.tab.update(cx, |t, cx| t.edit_for_test(f, cx));
        cx.run_until_parked();
    }

    fn vcx(&self, cx: &mut TestAppContext) -> VisualTestContext {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        vcx
    }

    fn drawn(&self, cx: &mut TestAppContext, selector: &'static str) -> bool {
        self.vcx(cx).debug_bounds(selector).is_some()
    }

    fn click(&self, cx: &mut TestAppContext, selector: &'static str) {
        let mut vcx = self.vcx(cx);
        let bounds = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"));
        vcx.simulate_click(bounds.center(), Modifiers::none());
        vcx.run_until_parked();
    }

    fn apply_enabled(&self, cx: &mut TestAppContext) -> bool {
        cx.read(|cx| self.win.read(cx).apply_enabled(cx))
    }

    fn sent(&self) -> Vec<ActionRequest> {
        self.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// `Started`, `state` for the one target, then the summary.
    fn finish(&self, cx: &mut TestAppContext, id: ActionId, state: ActionState) {
        for (target, state) in [
            (None, ActionState::Started),
            (Some(0), state.clone()),
            (None, state),
        ] {
            self.engine.emit(ActionEvent { id, target, state });
            cx.run_until_parked();
        }
    }

    /// Every toast this window's action layer pushed, oldest first.
    fn toasts(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.read(|cx| {
            let actions = self.win.read(cx).actions().clone();
            let pushed = actions.read(cx).pushed();
            pushed.iter().map(|t| t.text.to_string()).collect()
        })
    }

    /// Clicks Apply and returns the one request it sent.
    fn apply(&self, cx: &mut TestAppContext) -> ActionRequest {
        let before = self.sent().len();
        self.click(cx, "TlButton-ServicePropsApply");
        let sent = self.sent();
        assert_eq!(sent.len(), before + 1, "Apply sends one request");
        sent.last().cloned().expect("a request")
    }
}

#[gpui_kit::test]
fn recovery_view_formats(cx: &mut TestAppContext) {
    let t = open_recovery(
        cx,
        Some(recovery_details(|d| {
            d.recovery.restart_steps = Some(5);
            d.recovery.restart_max_delay_usec = Some(5_000_000);
            d.recovery.n_restarts = Some(12345);
        })),
    );
    assert_eq!(
        t.form(cx),
        RecoveryForm {
            restart: RestartPolicy::OnFailure,
            restart_ms: 100,
            burst: 5,
            interval_s: 10,
            start_limit_action: LimitAction::None,
            failure_action: LimitAction::None,
            on_failure: "".into(),
        }
    );
    assert_eq!(t.value(cx, "Restarts so far"), "12\u{2009}345");
    assert_eq!(
        cx.read(|cx| t.tab.read(cx).saved_as_for_test().to_string()),
        "/etc/systemd/system/sshd.service.d/50-tasklens-recovery.conf"
    );
    assert!(
        !t.drawn(cx, "ServiceRecoverySavedAs"),
        "no drop-in yet, so no Saved as line"
    );
    assert!(!t.drawn(cx, "ServiceRecoveryNote"), "the P2 note is gone");
    assert!(
        !t.drawn(cx, "TlButton-ServiceRecoveryRemove"),
        "no drop-in yet"
    );
    assert!(!t.apply_enabled(cx), "clean at first");

    // A clean form follows the unit.
    t.publish(
        cx,
        recovery_details(|d| d.recovery.restart_usec = Some(200_000)),
    );
    assert_eq!(t.form(cx).restart_ms, 200);
    assert!(!t.apply_enabled(cx));
}

#[gpui_kit::test]
fn recovery_run_on_failure_text_kept_while_dirty(cx: &mut TestAppContext) {
    let t = open_recovery(
        cx,
        Some(recovery_details(|d| {
            d.recovery.on_failure = Arc::from([a("failure-notify@sshd.service"), a("a.service")]);
        })),
    );
    assert_eq!(
        t.value(cx, "Run on failure"),
        "failure-notify@sshd.service a.service"
    );
    t.edit(cx, |f| f.burst = 7);
    assert!(t.apply_enabled(cx));
    // New details do not undo an edit in progress.
    t.publish(cx, recovery_details(|d| d.recovery.n_restarts = Some(4)));
    assert_eq!(t.form(cx).burst, 7);
    assert_eq!(t.value(cx, "Restarts so far"), "4");
    assert!(t.apply_enabled(cx));
}

#[gpui_kit::test]
fn recovery_view_missing_not_reported(cx: &mut TestAppContext) {
    let t = open_recovery(cx, None);
    for (label, value) in t.rows(cx) {
        assert_eq!(value, "Collecting\u{2026}", "{label}");
    }
    t.publish(cx, details("sshd.service"));
    assert_eq!(t.value(cx, "Restarts so far"), "Not reported");
    assert_eq!(t.form(cx), RecoveryForm::default());
    assert!(!t.apply_enabled(cx));
}

#[test]
fn recovery_validate_on_failure_pattern() {
    let name_255 = format!("{}.service", "a".repeat(255));
    let name_256 = format!("{}.service", "a".repeat(256));
    for ok in [
        "",
        "notify@%n.service",
        "failure-notify@%n.service",
        "a.service b.target",
        "a.service  c.timer",
        "x\\x2dy.mount",
        "user-1000.slice",
        "foo:bar_baz.socket",
        "sys-fs.path",
        "session-2.scope",
        name_255.as_str(),
    ] {
        assert!(validate_on_failure(ok), "{ok:?} is valid");
    }
    for bad in [
        "rm -rf /",
        "a.service b;c.service",
        "a.service\nb.service",
        "a.service\rb.service",
        "a.service\tb.service",
        "a/b.service",
        "/etc/x.service",
        "$(reboot).service",
        "a`id`.service",
        "a|b.service",
        "a&b.service",
        "a>b.service",
        "'a'.service",
        "\"a\".service",
        "a\0.service",
        "a*.service",
        ".service",
        "a",
        "a.exe",
        "a.service.",
        "a.Service",
        "\u{e4}.service",
        name_256.as_str(),
    ] {
        assert!(!validate_on_failure(bad), "{bad:?} is invalid");
    }
}

#[gpui_kit::test]
fn recovery_invalid_unit_blocks_apply(cx: &mut TestAppContext) {
    let t = open_recovery(cx, Some(recovery_details(|_| {})));
    let long = format!("{}.service", "a".repeat(256));
    for bad in [
        "rm -rf /",
        "a.service b;c.service",
        "a.service\nb.service",
        long.as_str(),
    ] {
        let text = bad.to_string();
        t.edit(cx, move |f| f.on_failure = text.into());
        cx.read(|cx| {
            let tab = t.tab.read(cx);
            assert!(tab.is_dirty(cx), "{bad:?}");
            assert!(!tab.is_valid(cx), "{bad:?}");
        });
        assert!(!t.apply_enabled(cx), "{bad:?} disables Apply");
        assert!(
            t.drawn(cx, "ServiceRecoveryOnFailureError"),
            "{bad:?} shows the danger line"
        );
    }
    // OK does nothing while the form is invalid.
    let mut vcx = t.vcx(cx);
    vcx.dispatch_action(ServicePropsOk);
    vcx.run_until_parked();
    drop(vcx);
    assert!(t.sent().is_empty(), "nothing was sent");
    assert_eq!(count(cx), 1, "the window stays open");

    t.edit(cx, |f| f.on_failure = "notify@%n.service".into());
    cx.read(|cx| assert!(t.tab.read(cx).is_valid(cx)));
    assert!(t.apply_enabled(cx));
    assert!(!t.drawn(cx, "ServiceRecoveryOnFailureError"));
}

#[gpui_kit::test]
fn recovery_helper_missing_disables(cx: &mut TestAppContext) {
    let t = open_recovery_helper(
        cx,
        Some(recovery_details(|d| d.recovery_dropin_exists = true)),
        HelperState::Missing,
    );
    let (enabled, tip) = cx.read(|cx| t.tab.read(cx).controls_for_test());
    assert!(!enabled, "every control is disabled");
    assert_eq!(tip.as_deref(), Some("Needs the tasklens helper"));
    assert!(t.drawn(cx, "ServiceRecoverySetupLink"));
    assert!(
        t.drawn(cx, "ServiceRecoverySavedAs"),
        "the drop-in exists, so the Saved as line shows"
    );
    assert!(t.drawn(cx, "TlButton-ServiceRecoveryRemove"));
    t.click(cx, "TlButton-ServiceRecoveryRemove");
    let dialog = cx.update(|cx| {
        cx.windows().into_iter().any(|w| {
            w.downcast::<Root>()
                .and_then(|h| TlSecondaryWindow::<TlDialogView>::find(h, cx))
                .is_some()
        })
    });
    assert!(!dialog, "a disabled Remove opens nothing");
    // The controls refuse edits.
    let before = t.form(cx);
    t.edit(cx, |f| f.burst = 9);
    assert_eq!(t.form(cx), before);
    assert!(!t.apply_enabled(cx));

    // Once the helper is installed the tab is editable and the link goes away.
    publish_helper(cx, &t.engine, HelperState::Locked);
    let (enabled, tip) = cx.read(|cx| t.tab.read(cx).controls_for_test());
    assert!(enabled);
    assert_eq!(tip, None);
    assert!(!t.drawn(cx, "ServiceRecoverySetupLink"));
}

#[gpui_kit::test]
fn recovery_remove_confirm(cx: &mut TestAppContext) {
    let t = open_recovery(
        cx,
        Some(recovery_details(|d| {
            d.recovery_dropin_exists = true;
            d.recovery_dropin_keys.restart = true;
        })),
    );
    t.click(cx, "TlButton-ServiceRecoveryRemove");
    assert!(t.sent().is_empty(), "nothing before the confirmation");
    let (any, frame) = cx
        .update(|cx| {
            cx.windows().into_iter().find_map(|w| {
                let handle = w.downcast::<Root>()?;
                let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                Some((w, frame))
            })
        })
        .expect("a confirmation");
    let dialog = cx.read(|cx| frame.read(cx).content().clone());
    let mut dvcx = VisualTestContext::from_window(any, cx);
    dvcx.update(|window, _| window.activate_window());
    draw(&mut dvcx);
    dvcx.update(|window, cx| {
        let view = dialog.read(cx);
        let d = view.dialog();
        assert_eq!(d.window_title().as_ref(), "tasklens");
        assert_eq!(
            d.text().as_ref(),
            "The unit goes back to the settings in its unit file."
        );
        assert_eq!(d.cancel_text().as_ref(), "Cancel");
        assert!(d.is_destructive(), "Remove is a Danger button");
        assert_eq!(view.focused_button(window), Some(DialogButton::Cancel));
    });
    assert!(dvcx.debug_bounds("TlDialogHeading").is_some());
    dvcx.simulate_keystrokes("tab");
    dvcx.simulate_keystrokes("enter");
    dvcx.run_until_parked();
    drop(dvcx);
    cx.run_until_parked();
    let sent = t.sent();
    assert_eq!(sent.len(), 1, "one request");
    assert_eq!(sent[0].kind, ActionKind::UnitRemoveRecovery);
    assert_eq!(
        sent[0].targets.to_vec(),
        vec![ActionTarget::Unit(unit("sshd.service"))]
    );
}

#[test]
fn recovery_changed_keys_pure() {
    let current = RecoverySettings {
        restart: Some(a("on-failure")),
        restart_usec: Some(100_000),
        start_limit_burst: Some(5),
        start_limit_interval_usec: Some(10_000_000),
        start_limit_action: Some(a("none")),
        failure_action: Some(a("none")),
        ..RecoverySettings::default()
    };
    let mut form = RecoveryForm::from_settings(&current);
    assert_eq!(
        changed_keys(&form, &current, &RecoveryKeySet::default()),
        RecoveryEdit::default(),
        "an unchanged form sends nothing"
    );
    form.restart_ms = 500;
    assert_eq!(
        changed_keys(&form, &current, &RecoveryKeySet::default()),
        RecoveryEdit {
            restart_usec: Some(500_000),
            ..RecoveryEdit::default()
        }
    );
    let in_dropin = RecoveryKeySet {
        restart: true,
        on_failure: true,
        ..RecoveryKeySet::default()
    };
    assert_eq!(
        changed_keys(&form, &current, &in_dropin),
        RecoveryEdit {
            restart: Some(a("on-failure")),
            restart_usec: Some(500_000),
            on_failure: Some(Arc::from([])),
            ..RecoveryEdit::default()
        }
    );
    // An unchanged key in the drop-in keeps the exact reported value and word.
    let odd = RecoverySettings {
        restart_usec: Some(100_500),
        start_limit_action: Some(a("soft-reboot")),
        ..current.clone()
    };
    let form = RecoveryForm::from_settings(&odd);
    let in_dropin = RecoveryKeySet {
        restart_sec: true,
        start_limit_action: true,
        ..RecoveryKeySet::default()
    };
    assert_eq!(
        changed_keys(&form, &odd, &in_dropin),
        RecoveryEdit {
            restart_usec: Some(100_500),
            start_limit_action: Some(a("soft-reboot")),
            ..RecoveryEdit::default()
        }
    );
}

#[gpui_kit::test]
fn recovery_changed_keys_only(cx: &mut TestAppContext) {
    let t = open_recovery(cx, Some(recovery_details(|_| {})));
    t.edit(cx, |f| f.restart_ms = 500);
    assert!(t.apply_enabled(cx));
    let req = t.apply(cx);
    assert_eq!(
        req.kind,
        ActionKind::UnitWriteRecovery(RecoveryEdit {
            restart_usec: Some(500_000),
            ..RecoveryEdit::default()
        })
    );
    assert_eq!(
        req.targets.to_vec(),
        vec![ActionTarget::Unit(unit("sshd.service"))]
    );
    t.finish(cx, req.id, ActionState::Succeeded);
    let toasts = t.toasts(cx);
    assert!(
        toasts.iter().any(|s| s == "Saving recovery settings..."),
        "{toasts:?}"
    );
    assert!(
        toasts.iter().any(|s| s == "Recovery settings saved"),
        "{toasts:?}"
    );
    assert!(!t.apply_enabled(cx), "clean after the save");
    assert_eq!(t.form(cx).restart_ms, 500, "shows the saved value");

    // A second edit before new details keeps the key the first save wrote.
    t.edit(cx, |f| f.burst = 6);
    let req = t.apply(cx);
    assert_eq!(
        req.kind,
        ActionKind::UnitWriteRecovery(RecoveryEdit {
            restart_usec: Some(500_000),
            start_limit_burst: Some(6),
            ..RecoveryEdit::default()
        })
    );
    t.finish(cx, req.id, ActionState::Succeeded);

    // Keys the drop-in reports are sent along with the changed one.
    t.publish(
        cx,
        recovery_details(|d| {
            d.recovery.restart_usec = Some(500_000);
            d.recovery.start_limit_burst = Some(6);
            d.recovery_dropin_exists = true;
            d.recovery_dropin_keys.restart_sec = true;
            d.recovery_dropin_keys.start_limit_burst = true;
        }),
    );
    t.edit(cx, |f| f.failure_action = LimitAction::Exit);
    let req = t.apply(cx);
    assert_eq!(
        req.kind,
        ActionKind::UnitWriteRecovery(RecoveryEdit {
            restart_usec: Some(500_000),
            start_limit_burst: Some(6),
            failure_action: Some(a("exit")),
            ..RecoveryEdit::default()
        })
    );
    t.finish(cx, req.id, ActionState::Failed(ActionError::AuthCancelled));
    let toasts = t.toasts(cx);
    assert_eq!(
        toasts.last().map(String::as_str),
        Some("Authentication cancelled")
    );
    assert!(t.apply_enabled(cx), "a cancelled save keeps the edit");
}

#[gpui_kit::test]
fn recovery_reboot_warning(cx: &mut TestAppContext) {
    let power = [
        LimitAction::Reboot,
        LimitAction::RebootForce,
        LimitAction::RebootImmediate,
        LimitAction::Poweroff,
        LimitAction::PoweroffForce,
        LimitAction::PoweroffImmediate,
    ];
    for action in LimitAction::ALL {
        assert_eq!(
            action.restarts_computer(),
            power.contains(&action),
            "{action:?}"
        );
    }
    let t = open_recovery(cx, Some(recovery_details(|_| {})));
    assert!(!t.drawn(cx, "ServiceRecoveryWarning-3"));
    assert!(!t.drawn(cx, "ServiceRecoveryWarning-4"));
    t.edit(cx, |f| f.start_limit_action = LimitAction::Reboot);
    assert!(t.drawn(cx, "ServiceRecoveryWarning-3"));
    assert!(!t.drawn(cx, "ServiceRecoveryWarning-4"));
    t.edit(cx, |f| f.failure_action = LimitAction::PoweroffImmediate);
    assert!(t.drawn(cx, "ServiceRecoveryWarning-4"));
    t.edit(cx, |f| f.start_limit_action = LimitAction::Exit);
    assert!(!t.drawn(cx, "ServiceRecoveryWarning-3"));
    assert!(t.drawn(cx, "ServiceRecoveryWarning-4"));
}

/// A value the selects do not offer, such as `soft-reboot`, shows as itself, so choosing
/// "none" is a change and Apply writes it.
#[gpui_kit::test]
fn recovery_other_action_can_be_disabled(cx: &mut TestAppContext) {
    for action in LimitAction::OTHER {
        assert_eq!(LimitAction::from_word(action.word()), Some(action));
        assert!(action.restarts_computer(), "{action:?}");
    }
    let t = open_recovery(
        cx,
        Some(recovery_details(|d| {
            d.recovery.failure_action = Some(a("soft-reboot"));
        })),
    );
    assert_eq!(t.form(cx).failure_action, LimitAction::SoftReboot);
    let shown = cx.read(|cx| t.tab.read(cx).action_selects_for_test(cx));
    assert_eq!(
        shown[1].as_deref(),
        Some("soft-reboot"),
        "the select shows it"
    );
    assert!(!t.apply_enabled(cx), "the unit's own value is not a change");
    assert!(t.drawn(cx, "ServiceRecoveryWarning-4"));

    t.edit(cx, |f| f.failure_action = LimitAction::None);
    assert!(t.apply_enabled(cx), "none differs from soft-reboot");
    assert!(!t.drawn(cx, "ServiceRecoveryWarning-4"));
    let req = t.apply(cx);
    assert_eq!(
        req.kind,
        ActionKind::UnitWriteRecovery(RecoveryEdit {
            failure_action: Some(a("none")),
            ..RecoveryEdit::default()
        })
    );
}

fn recovery_control_names(t: &Recovery, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| t.tab.read(cx).control_names_for_test())
        .into_iter()
        .map(|n| n.to_string())
        .collect()
}

/// Each focusable control carries its row label as its accessible name, and an invalid "Run on
/// failure" adds the validation line to the field's own name.
#[gpui_kit::test]
fn recovery_controls_named(cx: &mut TestAppContext) {
    let t = open_recovery(cx, Some(recovery_details(|_| {})));
    assert_eq!(
        recovery_control_names(&t, cx),
        [
            "On failure",
            "When giving up",
            "Failure action",
            "Restart service after",
            "Give up after, starts",
            "Give up after, seconds",
            "Run on failure",
        ]
    );
    t.edit(cx, |f| f.on_failure = "a.service b;c.service".into());
    assert_eq!(
        recovery_control_names(&t, cx).last().map(String::as_str),
        Some(
            "Run on failure, Enter unit names separated by spaces, for example \
             failure-notify@%n.service"
        )
    );
    t.edit(cx, |f| f.on_failure = "notify@%n.service".into());
    assert_eq!(
        recovery_control_names(&t, cx).last().map(String::as_str),
        Some("Run on failure")
    );
}

// Dependencies tab (UI-SYS-035).

struct Deps {
    engine: Arc<FakeEngine>,
    _main: VisualTestContext,
    handle: WindowHandle<Root>,
    tab: Entity<DependenciesTab>,
}

fn dep(name: &str, relation: Relation) -> DepEntry {
    DepEntry {
        unit: unit(name),
        relation,
    }
}

fn deps_details(deps: Vec<DepEntry>) -> Arc<UnitDetails> {
    details_with("sshd.service", move |d| d.deps = deps.into())
}

/// Opens sshd.service on Dependencies with `d` published, or with no details. The services
/// list holds sshd.service and system.slice.
fn open_deps(cx: &mut TestAppContext, d: Option<Arc<UnitDetails>>) -> Deps {
    let (engine, main) = setup(cx);
    let mut map = HashMap::new();
    if let Some(d) = d {
        map.insert(unit("sshd.service"), d);
    }
    publish(
        cx,
        &engine,
        &["sshd.service", "system.slice"],
        &map,
        ServiceBackend::Systemd,
    );
    let handle = open(cx, "sshd.service", ServicePropsTab::Dependencies).expect("opened");
    let win = view(cx, handle);
    let tab = cx
        .read(|cx| win.read(cx).dependencies_tab())
        .expect("the Dependencies tab is built");
    Deps {
        engine,
        _main: main,
        handle,
        tab,
    }
}

impl Deps {
    fn lines(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.read(|cx| self.tab.read(cx).lines_for_test())
    }

    fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, cx| {
            window.activate_window();
            window.focus(&self.tab.read(cx).focus_handle_for_test(), cx);
        });
        draw(&mut vcx);
        vcx.simulate_keystrokes(keys);
        vcx.run_until_parked();
    }
}

#[gpui_kit::test]
fn deps_empty_shows_none(cx: &mut TestAppContext) {
    let t = open_deps(cx, None);
    assert_eq!(t.lines(cx), ["Collecting\u{2026}"]);
    let _ = &t.engine;
    publish(
        cx,
        &t.engine,
        &["sshd.service", "system.slice"],
        &HashMap::from([(unit("sshd.service"), details("sshd.service"))]),
        ServiceBackend::Systemd,
    );
    assert_eq!(
        t.lines(cx),
        [
            "Depends on:",
            "(None)",
            "Dependents:",
            "(None)",
            "Ordering (0)"
        ]
    );
}

#[gpui_kit::test]
fn deps_one_row_per_relation_sorted(cx: &mut TestAppContext) {
    let t = open_deps(
        cx,
        Some(deps_details(vec![
            dep("system.slice", Relation::Requires),
            dep("network-online.target", Relation::Wants),
            dep("network-online.target", Relation::After),
        ])),
    );
    assert_eq!(
        t.lines(cx),
        [
            "Depends on:",
            "network-online.target Wants",
            "system.slice Requires",
            "Dependents:",
            "(None)",
            "Ordering (1)",
        ]
    );
    let rows = cx.read(|cx| t.tab.read(cx).rows_for_test());
    assert_eq!(rows[0].0, "network-online.target, Wants, Not loaded");
    assert_eq!(rows[0].1.dot, DotStyle::Ring(LabelTone::Secondary));
    assert_eq!(rows[1].0, "system.slice, Requires, Running");
    assert_eq!(rows[1].1.dot, DotStyle::Filled(LabelTone::Success));
}

#[gpui_kit::test]
fn deps_ordering_collapsed_by_default(cx: &mut TestAppContext) {
    let entries: Vec<DepEntry> = (0..12)
        .map(|i| {
            dep(
                &format!("o{:02}.service", 11 - i),
                if i % 2 == 0 {
                    Relation::After
                } else {
                    Relation::Before
                },
            )
        })
        .collect();
    let t = open_deps(cx, Some(deps_details(entries)));
    let tail = |lines: Vec<String>| lines[4..].to_vec();
    assert_eq!(tail(t.lines(cx)), ["Ordering (12)"]);
    t.keys(cx, "down enter");
    let lines = t.lines(cx);
    assert_eq!(lines.len(), 5 + 12);
    assert_eq!(lines[5], "o00.service Before");
    assert_eq!(lines[16], "o11.service After");
    let mut sorted = lines[5..].to_vec();
    sorted.sort();
    assert_eq!(&lines[5..], &sorted[..]);
    t.keys(cx, "space");
    assert_eq!(tail(t.lines(cx)), ["Ordering (12)"]);
}

#[gpui_kit::test]
fn deps_double_click_opens_props(cx: &mut TestAppContext) {
    let t = open_deps(
        cx,
        Some(deps_details(vec![dep(
            "multi-user.target",
            Relation::WantedBy,
        )])),
    );
    assert_eq!(count(cx), 1);
    let mut vcx = VisualTestContext::from_window(t.handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    draw(&mut vcx);
    let at = vcx
        .debug_bounds("ServiceDeps-row-3")
        .expect("the row is drawn")
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.simulate_event(gpui_kit::MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: gpui_kit::MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    vcx.simulate_event(gpui_kit::MouseUpEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: gpui_kit::MouseButton::Left,
        click_count: 2,
    });
    vcx.run_until_parked();
    drop(vcx);
    let opened = cx.update(|cx| {
        let registry = cx.global::<GlobalRegistry>().0.clone();
        registry
            .read(cx)
            .get(&WindowKey::Service(unit("multi-user.target")))
            .is_some()
    });
    assert!(opened, "the unit's window is open");
    assert_eq!(count(cx), 2);
}

// Logs tab (UI-SYS-036).

struct Logs {
    engine: Arc<FakeEngine>,
    _main: VisualTestContext,
    handle: WindowHandle<Root>,
    tab: Entity<LogsView>,
}

/// 2023-11-14 22:13:20 UTC plus `s` seconds, in microseconds.
fn at(s: u64) -> u64 {
    (1_700_000_000 + s) * 1_000_000
}

fn entry(s: u64, priority: u8, message: &str) -> JournalEntry {
    JournalEntry {
        realtime_us: at(s),
        priority,
        pid: Some(812),
        message: a(message),
        cursor: Arc::from(format!("s=1;i={s}")),
    }
}

/// Opens `unit` on the Logs tab. The services list holds sshd.service.
fn open_logs_of(cx: &mut TestAppContext, unit: UnitRef) -> Logs {
    let (engine, main) = setup(cx);
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &HashMap::new(),
        ServiceBackend::Systemd,
    );
    let key = WindowKey::Service(unit.clone());
    cx.update(|cx| ServicePropertiesWindow::open(unit, ServicePropsTab::Logs, cx));
    cx.run_until_parked();
    let handle = cx
        .update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let handle = registry.read(cx).get(&key);
            handle.and_then(|h| h.downcast::<Root>())
        })
        .expect("opened");
    let win = view(cx, handle);
    let tab = cx
        .read(|cx| win.read(cx).logs_tab())
        .expect("the Logs tab is built");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    draw(&mut vcx);
    Logs {
        engine,
        _main: main,
        handle,
        tab,
    }
}

fn open_logs(cx: &mut TestAppContext) -> Logs {
    open_logs_of(cx, unit("sshd.service"))
}

impl Logs {
    /// Sends one batch through the first journal handle. The channel holds one event.
    fn feed(&self, cx: &mut TestAppContext, entries: Vec<JournalEntry>) {
        let (events, _) = self.engine.journal_end(0).expect("a journal was opened");
        events
            .try_send(JournalEvent::Batch {
                entries: entries.into(),
                older: false,
                exhausted: false,
            })
            .expect("the event channel has room");
        cx.run_until_parked();
    }

    fn rows(&self, cx: &mut TestAppContext) -> Vec<LogRowForTest> {
        cx.read(|cx| self.tab.read(cx).rows_for_test(cx))
    }

    fn messages(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.rows(cx).into_iter().map(|r| r.message).collect()
    }

    fn vcx(&self, cx: &mut TestAppContext) -> VisualTestContext {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        vcx
    }

    fn click(&self, vcx: &mut VisualTestContext, selector: &'static str, mods: Modifiers) {
        let at = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"))
            .center();
        vcx.simulate_click(at, mods);
        vcx.run_until_parked();
    }

    fn toasts(&self, cx: &mut TestAppContext) -> Vec<String> {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
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
}

fn launches(engine: &FakeEngine) -> Vec<ActionRequest> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(req) if matches!(req.kind, ActionKind::Launch(_)) => Some(req),
            _ => None,
        })
        .collect()
}

fn launch_command(req: &ActionRequest) -> String {
    match &req.kind {
        ActionKind::Launch(spec) => {
            assert!(!spec.as_admin);
            match &spec.what {
                LaunchWhat::Command(c) => c.to_string(),
                other => panic!("expected a command, got {other:?}"),
            }
        }
        other => panic!("expected a launch, got {other:?}"),
    }
}

#[gpui_kit::test]
fn logs_priority_colors(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    let queries = t.engine.journals();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].unit, unit("sshd.service"));
    assert!(queries[0].follow);
    t.feed(
        cx,
        vec![
            entry(0, 3, "failed"),
            entry(1, 4, "slow"),
            entry(2, 6, "started"),
            entry(3, 7, "detail"),
        ],
    );
    let mut vcx = t.vcx(cx);
    assert!(vcx.debug_bounds("ServiceLogs-row-3").is_some());
    drop(vcx);
    let theme = cx.read(|cx| tl_ui::theme::current(cx).clone());
    let rows = t.rows(cx);
    let words: Vec<_> = rows.iter().map(|r| r.priority.as_str()).collect();
    assert_eq!(words, ["err", "warning", "info", "debug"]);
    let colors: Vec<_> = rows.iter().map(|r| r.text_color).collect();
    assert_eq!(
        colors,
        [
            theme.status.danger,
            theme.status.warning,
            theme.text_primary,
            theme.text_secondary
        ]
    );
    assert!(rows.iter().all(|r| r.time_color == theme.text_secondary));
    assert_eq!(rows[0].time, LogsView::time_for_test(at(0)));
}

#[gpui_kit::test]
fn logs_multiline_suffix(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, vec![entry(0, 6, "a\nb\nc"), entry(1, 6, "single")]);
    let rows = t.rows(cx);
    assert_eq!(rows[0].message, "a");
    assert_eq!(rows[0].suffix.as_deref(), Some(" (+2 lines)"));
    assert_eq!(rows[1].message, "single");
    assert_eq!(rows[1].suffix, None);
    let theme = cx.read(|cx| tl_ui::theme::current(cx).clone());
    assert_eq!(rows[0].time_color, theme.text_secondary);
}

#[gpui_kit::test]
fn logs_filter_debounced(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(
        cx,
        vec![
            entry(0, 6, "Connection TIMEOUT"),
            entry(1, 6, "started"),
            entry(2, 4, "first line\nthen a timeout"),
            entry(3, 3, "other"),
        ],
    );
    let all = ["Connection TIMEOUT", "started", "first line", "other"];
    assert_eq!(t.messages(cx), all);
    let input = cx.read(|cx| t.tab.read(cx).filter_input_for_test());
    let mut vcx = t.vcx(cx);
    vcx.update(|window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
    vcx.simulate_input("time");
    vcx.executor().advance_clock(Duration::from_millis(50));
    vcx.run_until_parked();
    vcx.simulate_input("out");
    vcx.executor().advance_clock(Duration::from_millis(50));
    vcx.run_until_parked();
    drop(vcx);
    assert_eq!(t.messages(cx), all, "a key within 100 ms restarts the wait");
    cx.executor().advance_clock(Duration::from_millis(50));
    cx.run_until_parked();
    assert_eq!(t.messages(cx), ["Connection TIMEOUT", "first line"]);

    // The priority word matches too.
    let mut vcx = t.vcx(cx);
    vcx.simulate_keystrokes("ctrl-a backspace");
    vcx.simulate_input("WARN");
    vcx.executor().advance_clock(Duration::from_millis(100));
    vcx.run_until_parked();
    drop(vcx);
    assert_eq!(t.messages(cx), ["first line"]);
}

#[gpui_kit::test]
fn logs_copy_format(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(
        cx,
        vec![
            entry(0, 3, "first"),
            entry(1, 6, "a\nb"),
            entry(2, 6, "not copied"),
        ],
    );
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "ServiceLogs-row-0", Modifiers::none());
    t.click(&mut vcx, "ServiceLogs-row-1", Modifiers::shift());
    vcx.simulate_keystrokes("ctrl-c");
    vcx.run_until_parked();
    let copied = vcx.read_from_clipboard().and_then(|item| item.text());
    let expected = format!(
        "{} err first\n{} info a\nb",
        LogsView::time_for_test(at(0)),
        LogsView::time_for_test(at(1))
    );
    assert_eq!(copied.as_deref(), Some(expected.as_str()));
    drop(vcx);
    assert_eq!(t.toasts(cx), ["Copied to clipboard"]);
}

#[gpui_kit::test]
fn logs_open_terminal_command_quoting(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsTerminal", Modifiers::none());
    drop(vcx);
    let sent = launches(&t.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        launch_command(&sent[0]),
        "xdg-terminal-exec journalctl -u \"sshd.service\" -b"
    );
}

#[gpui_kit::test]
fn logs_open_terminal_quoting_user_unit(cx: &mut TestAppContext) {
    let user = UnitRef {
        manager: Manager::User,
        name: a("a$b\"c`d\\e.service"),
    };
    let t = open_logs_of(cx, user);
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsTerminal", Modifiers::none());
    drop(vcx);
    let sent = launches(&t.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        launch_command(&sent[0]),
        "xdg-terminal-exec journalctl --user-unit \"a\\$b\\\"c\\`d\\\\e.service\" -b"
    );
}

#[gpui_kit::test]
fn logs_selected_line_keeps_secondary_time(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, vec![entry(0, 6, "a\nb\nc"), entry(1, 3, "bad")]);
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "ServiceLogs-row-0", Modifiers::none());
    t.click(&mut vcx, "ServiceLogs-row-1", Modifiers::control());
    drop(vcx);
    let theme = cx.read(|cx| tl_ui::theme::current(cx).clone());
    let rows = t.rows(cx);
    assert_eq!(rows[0].time_color, theme.text_secondary);
    assert_eq!(rows[0].text_color, theme.text_selected_primary);
    assert_eq!(rows[1].time_color, theme.text_secondary);
    assert_eq!(rows[1].text_color, theme.status.danger);
}

#[gpui_kit::test]
fn logs_open_terminal_two_clicks_each_fall_back(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsTerminal", Modifiers::none());
    t.click(&mut vcx, "TlButton-ServiceLogsTerminal", Modifiers::none());
    drop(vcx);
    let first = launches(&t.engine);
    assert_eq!(first.len(), 2);
    assert_ne!(first[0].id, first[1].id);
    for req in &first {
        t.engine.emit(ActionEvent {
            id: req.id,
            target: None,
            state: ActionState::Failed(ActionError::Os(2)),
        });
    }
    cx.run_until_parked();
    let sent = launches(&t.engine);
    assert_eq!(sent.len(), 4, "each failed click falls back to konsole");
    for req in &sent[2..] {
        assert_eq!(
            launch_command(req),
            "konsole -e journalctl -u \"sshd.service\" -b"
        );
    }
    for req in &sent[2..] {
        t.engine.emit(ActionEvent {
            id: req.id,
            target: None,
            state: ActionState::Failed(ActionError::Os(2)),
        });
    }
    cx.run_until_parked();
    assert_eq!(launches(&t.engine).len(), 4);
    // The stack merges the two identical error toasts into one with a count suffix.
    let toasts = t.toasts(cx);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(
        toasts[0].starts_with("Could not open a terminal"),
        "{toasts:?}"
    );
    assert_ne!(toasts[0], "Could not open a terminal", "both failures show");
}

#[gpui_kit::test]
fn logs_open_terminal_fallback_to_konsole(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsTerminal", Modifiers::none());
    drop(vcx);
    let sent = launches(&t.engine);
    assert_eq!(sent.len(), 1);
    t.engine.emit(ActionEvent {
        id: sent[0].id,
        target: None,
        state: ActionState::Failed(ActionError::Os(2)),
    });
    cx.run_until_parked();
    let sent = launches(&t.engine);
    assert_eq!(sent.len(), 2);
    assert_eq!(
        launch_command(&sent[1]),
        "konsole -e journalctl -u \"sshd.service\" -b"
    );
    assert!(t.toasts(cx).is_empty(), "the first failure shows nothing");
    t.engine.emit(ActionEvent {
        id: sent[1].id,
        target: None,
        state: ActionState::Failed(ActionError::Os(2)),
    });
    cx.run_until_parked();
    assert_eq!(launches(&t.engine).len(), 2);
    assert_eq!(t.toasts(cx), ["Could not open a terminal"]);
}

// Logs tab: follow, pill, Load older and the cap (UI-SYS-037).

impl Logs {
    /// Sends one older page through the first journal handle.
    fn feed_older(&self, cx: &mut TestAppContext, entries: Vec<JournalEntry>, exhausted: bool) {
        let (events, _) = self.engine.journal_end(0).expect("a journal was opened");
        events
            .try_send(JournalEvent::Batch {
                entries: entries.into(),
                older: true,
                exhausted,
            })
            .expect("the event channel has room");
        cx.run_until_parked();
    }

    fn state(&self, cx: &mut TestAppContext) -> FollowStateForTest {
        cx.read(|cx| self.tab.read(cx).follow_state_for_test(cx))
    }

    /// The next control the engine received, if any.
    fn control(&self) -> Option<JournalControl> {
        let (_, control) = self.engine.journal_end(0).expect("a journal was opened");
        control.try_recv().ok()
    }

    /// Scrolls the list up by `lines` lines of 18 px with the wheel.
    fn scroll_up(&self, cx: &mut TestAppContext, lines: f32) {
        let mut vcx = self.vcx(cx);
        let at = vcx
            .debug_bounds("ServiceLogsList")
            .expect("the list is drawn")
            .center();
        vcx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(18. * lines))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        vcx.run_until_parked();
        draw(&mut vcx);
    }

    fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, cx| {
            window.activate_window();
            window.focus(&self.tab.read(cx).focus_handle_for_test(), cx);
        });
        draw(&mut vcx);
        vcx.simulate_keystrokes(keys);
        vcx.run_until_parked();
    }

    fn select(&self, cx: &mut TestAppContext, tab: ServicePropsTab) {
        let win = view(cx, self.handle);
        let mut vcx = VisualTestContext::from_window(self.handle.into(), cx);
        vcx.update(|window, cx| win.update(cx, |w, cx| w.select_tab(tab, window, cx)));
        draw(&mut vcx);
    }

    fn pill_shown(&self, cx: &mut TestAppContext) -> bool {
        self.vcx(cx)
            .debug_bounds("TlButton-ServiceLogsNewLines")
            .is_some()
    }
}

fn numbered(range: std::ops::Range<u64>) -> Vec<JournalEntry> {
    range.map(|s| entry(s, 6, &format!("m{s}"))).collect()
}

#[gpui_kit::test]
fn follow_sticks_to_bottom(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(0..100));
    t.vcx(cx);
    assert!(t.state(cx).at_bottom);
    t.feed(cx, numbered(100..105));
    t.vcx(cx);
    let st = t.state(cx);
    assert!(st.at_bottom && st.stick_to_bottom);
    assert_eq!(st.new_since_unstick, 0);
    assert!(!t.pill_shown(cx));
}

#[gpui_kit::test]
fn follow_unstick_counts_new_lines(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(0..100));
    t.vcx(cx);
    let bottom = t.state(cx).first_visible;
    t.scroll_up(cx, 10.);
    let st = t.state(cx);
    assert!(!st.stick_to_bottom);
    assert_eq!(st.first_visible, bottom - 10);
    t.feed(cx, numbered(100..112));
    t.vcx(cx);
    let st = t.state(cx);
    assert_eq!(st.first_visible, bottom - 10, "the position is unchanged");
    assert_eq!(st.new_since_unstick, 12);
    assert_eq!(st.pill_text, "↓ 12 new lines");
    assert!(t.pill_shown(cx));
    t.keys(cx, "end");
    t.vcx(cx);
    let st = t.state(cx);
    assert!(st.at_bottom && st.stick_to_bottom);
    assert_eq!(st.new_since_unstick, 0);
    assert!(!t.pill_shown(cx));
}

#[gpui_kit::test]
fn follow_off_kills_child(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(0..10));
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsFollow", Modifiers::none());
    assert_eq!(t.control(), Some(JournalControl::SetFollow(false)));
    assert!(!t.state(cx).follow);
    t.click(&mut vcx, "TlButton-ServiceLogsFollow", Modifiers::none());
    assert_eq!(t.control(), Some(JournalControl::SetFollow(true)));
    assert!(t.state(cx).follow);
}

#[gpui_kit::test]
fn tab_leave_stops_follow_and_return_resumes(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(0..10));
    t.select(cx, ServicePropsTab::General);
    assert_eq!(t.control(), Some(JournalControl::SetFollow(false)));
    t.select(cx, ServicePropsTab::Logs);
    assert_eq!(t.control(), Some(JournalControl::SetFollow(true)));
    assert_eq!(t.engine.journals().len(), 1, "one handle for the window");
    assert_eq!(t.messages(cx).len(), 10, "the loaded lines stay");
    // With Follow off, leaving and returning sends nothing.
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsFollow", Modifiers::none());
    assert_eq!(t.control(), Some(JournalControl::SetFollow(false)));
    drop(vcx);
    t.select(cx, ServicePropsTab::General);
    t.select(cx, ServicePropsTab::Logs);
    assert_eq!(t.control(), None);
}

#[gpui_kit::test]
fn load_older_keeps_anchor(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(1000..1300));
    t.vcx(cx);
    t.scroll_up(cx, 100.);
    let before = t.state(cx);
    let first_message = t.messages(cx)[before.first_visible].clone();
    let mut vcx = t.vcx(cx);
    t.click(&mut vcx, "TlButton-ServiceLogsLoadOlder", Modifiers::none());
    assert_eq!(
        t.control(),
        Some(JournalControl::LoadOlder(Some(Arc::from("s=1;i=1000"))))
    );
    let st = t.state(cx);
    assert_eq!(st.older_label, "Loading…");
    assert!(!st.older_enabled);
    drop(vcx);
    t.feed_older(cx, numbered(800..1000), false);
    t.vcx(cx);
    let after = t.state(cx);
    assert!(after.older_enabled);
    assert_eq!(after.older_label, "Load older");
    assert_eq!(t.messages(cx)[after.first_visible], first_message);
    assert_eq!(after.first_visible, before.first_visible + 200);
}

#[gpui_kit::test]
fn load_older_exhausted(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(1000..1300));
    t.feed_older(cx, numbered(963..1000), true);
    let st = t.state(cx);
    assert_eq!(st.older_label, "No older entries");
    assert!(!st.older_enabled);
    assert_eq!(t.messages(cx).len(), 337);
}

#[gpui_kit::test]
fn cap_10000_drops_oldest(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.feed(cx, numbered(1000..10_900));
    t.feed_older(cx, numbered(900..1000), true);
    assert_eq!(t.messages(cx).len(), 10_000);
    assert!(!t.state(cx).older_enabled);
    t.feed(cx, numbered(20_000..20_050));
    let messages = t.messages(cx);
    assert_eq!(messages.len(), 10_000);
    assert_eq!(messages[0], "m950");
    assert_eq!(messages[9_999], "m20049");
    assert!(t.state(cx).older_enabled);
}

impl Logs {
    /// Sends one event through the journal handle `i`.
    fn send(&self, cx: &mut TestAppContext, i: usize, ev: JournalEvent) {
        let (events, _) = self.engine.journal_end(i).expect("a journal was opened");
        events.try_send(ev).expect("the event channel has room");
        cx.run_until_parked();
    }

    fn status(&self, cx: &mut TestAppContext) -> LogsStatusForTest {
        cx.read(|cx| self.tab.read(cx).status_for_test(cx))
    }
}

#[gpui_kit::test]
fn logs_loading_and_empty_states(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    assert_eq!(t.status(cx).text.as_deref(), Some("Loading logs…"));
    t.feed(cx, vec![]);
    assert_eq!(
        t.status(cx).text.as_deref(),
        Some("No log entries for this unit in the current boot.")
    );

    let t = open_logs(cx);
    t.feed(cx, vec![entry(0, 6, "started"), entry(1, 6, "stopped")]);
    assert_eq!(t.status(cx).text, None);
    let input = cx.read(|cx| t.tab.read(cx).filter_input_for_test());
    let mut vcx = t.vcx(cx);
    vcx.update(|window, cx| input.update(cx, |s, cx| s.focus(window, cx)));
    vcx.simulate_input("zzz");
    vcx.executor().advance_clock(Duration::from_millis(100));
    vcx.run_until_parked();
    drop(vcx);
    assert_eq!(
        t.status(cx).text.as_deref(),
        Some("No log entries match the filter.")
    );
}

#[gpui_kit::test]
fn logs_timeout_retry(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.send(cx, 0, JournalEvent::Error(JournalError::Timeout));
    let st = t.status(cx);
    assert_eq!(
        st.heading.as_deref(),
        Some("Reading the journal took too long.")
    );
    assert_eq!(st.button.as_deref(), Some("Retry"));
    let mut vcx = t.vcx(cx);
    t.click(
        &mut vcx,
        "TlButton-TlEmptyState-clear-filter",
        Modifiers::none(),
    );
    drop(vcx);
    let journals = t.engine.journals();
    assert_eq!(journals.len(), 2);
    assert_eq!(journals[1].unit, journals[0].unit);
    assert!(t.engine.journal_end(0).expect("first").1.is_closed());
    assert_eq!(t.status(cx).text.as_deref(), Some("Loading logs…"));
    t.send(
        cx,
        1,
        JournalEvent::Batch {
            entries: vec![entry(0, 6, "again")].into(),
            older: false,
            exhausted: false,
        },
    );
    assert_eq!(t.messages(cx), ["again"]);
}

#[gpui_kit::test]
fn logs_exit_error_text(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.send(
        cx,
        0,
        JournalEvent::Error(JournalError::Exit {
            code: 1,
            stderr_first_line: a("Failed to open journal"),
        }),
    );
    let st = t.status(cx);
    assert_eq!(st.heading.as_deref(), Some("Could not read the journal"));
    assert_eq!(st.text.as_deref(), Some("Failed to open journal"));
    assert_eq!(st.button.as_deref(), Some("Retry"));
}

#[gpui_kit::test]
fn logs_access_banner_shown(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.send(cx, 0, JournalEvent::Access(JournalAccess::OwnOnly));
    t.feed(cx, vec![entry(0, 6, "started")]);
    assert_eq!(
        t.status(cx).banner.as_deref(),
        Some(
            "Only your own log messages are visible. Add your user to the systemd-journal group to see all."
        )
    );
    assert_eq!(t.messages(cx), ["started"]);
    let mut vcx = t.vcx(cx);
    t.click(
        &mut vcx,
        "TlButton-TlBanner-Copy command",
        Modifiers::none(),
    );
    let copied = vcx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some("sudo usermod -aG systemd-journal $USER")
    );
    drop(vcx);
    assert_eq!(t.toasts(cx), ["Copied to clipboard"]);
    t.send(cx, 0, JournalEvent::Access(JournalAccess::All));
    assert_eq!(t.status(cx).banner, None);
}

#[gpui_kit::test]
fn logs_banner_additional_setup_link(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    let opened = Rc::new(Cell::new(0));
    let count = opened.clone();
    cx.update(|cx| {
        cx.on_action(move |a: &OpenAdditionalSetup, _| {
            if a.focus_row == Some(SetupRow::JournalAccess) {
                count.set(count.get() + 1)
            }
        });
    });
    t.send(cx, 0, JournalEvent::Access(JournalAccess::OwnOnly));
    t.feed(cx, vec![entry(0, 6, "started")]);
    let mut vcx = t.vcx(cx);
    let copy = vcx
        .debug_bounds("TlButton-TlBanner-Copy command")
        .expect("Copy command drawn");
    let link = vcx.debug_bounds("TlBanner-link").expect("link drawn");
    assert!(copy.right() <= link.left(), "the link follows Copy command");
    // Enter on the link is covered by `banner::tests::link_after_buttons`: the tab calls the same
    // `TlBanner::link`.
    t.click(&mut vcx, "TlBanner-link", Modifiers::none());
    assert_eq!(opened.get(), 1, "click dispatches OpenAdditionalSetup once");
}

#[gpui_kit::test]
fn logs_not_found_no_button(cx: &mut TestAppContext) {
    let t = open_logs(cx);
    t.send(cx, 0, JournalEvent::Error(JournalError::NotFound));
    let st = t.status(cx);
    assert_eq!(st.heading.as_deref(), Some("journalctl not found"));
    assert_eq!(st.button, None);
    let mut vcx = t.vcx(cx);
    assert!(
        vcx.debug_bounds("TlButton-TlEmptyState-clear-filter")
            .is_none()
    );
}

#[gpui_kit::test]
fn deps_opened_window_demands_and_fills_details(cx: &mut TestAppContext) {
    let t = open_deps(
        cx,
        Some(deps_details(vec![dep("system.slice", Relation::Requires)])),
    );
    t.keys(cx, "down enter");
    let handle = cx
        .update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let handle = registry
                .read(cx)
                .get(&WindowKey::Service(unit("system.slice")));
            handle.and_then(|h| h.downcast::<Root>())
        })
        .expect("the dependency's window is open");
    let win = view(cx, handle);
    assert!(cx.read(|cx| win.read(cx).details().is_none()));
    let demanded = demanded(cx);
    assert!(
        demanded.contains(&unit("system.slice")),
        "the dependency's unit is demanded: {demanded:?}"
    );
    let mut map = HashMap::new();
    map.insert(unit("system.slice"), details("system.slice"));
    publish(
        cx,
        &t.engine,
        &["sshd.service", "system.slice"],
        &map,
        ServiceBackend::Systemd,
    );
    assert!(
        cx.read(|cx| win.read(cx).details().is_some()),
        "the details fill once the engine publishes them"
    );
}
