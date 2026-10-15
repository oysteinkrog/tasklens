//! Service Properties window frame tests (E27-A3, UI-SYS-030). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, WindowHandle, div,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionState, ActionTarget,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{
    RecoverySettings, ServiceBackend, ServiceStatus, StartupType, UnitDetails, UnitRecord,
};
use tl_model::table::{Manager, Total, UnitRef, UnitType};
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::demand::GlobalDemand;
use tl_ui::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{main_window_view, open_main_window_on};
use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
use tl_ui::windows::service_properties::{
    ServicePropertiesWindow, ServicePropsTab, ServicePropsTabView,
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
    assert_eq!((width, height), (720. * scale, 560. * scale));
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

    // A new version with the same details Arc.
    publish(
        cx,
        &engine,
        &["sshd.service"],
        &map,
        ServiceBackend::Systemd,
    );
    assert_eq!(notified.get(), 0, "same details Arc, no notify");

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
