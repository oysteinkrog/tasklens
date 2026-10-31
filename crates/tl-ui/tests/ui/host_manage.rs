//! Settings > Hosts and the host tab context menu (UI-SHL-008, ADR-018 "Hosts",
//! tasklens-q4x76.28). Adding, editing, reordering and removing hosts through the page updates
//! `[[hosts]]` and the tabs without a restart; the tab menu reconnects, disconnects, opens the
//! page and removes. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::{Arc, Mutex};

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    WindowHandle, px,
};
use tl_model::client::EngineClient;
use tl_model::settings::{HostEntry, Settings, StartTab, UiState};
use tl_ui::actions::{DisconnectHost, EditHost, ReconnectHost, RemoveHost};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::menu::open_context_menu;
use tl_ui::host::HostContext;
use tl_ui::hosts_api::{HostLinkStatus, HostsApi};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::settings::hosts::{HostsEditor, TestLine, open_editor};
use tl_ui::window::settings::{PageId, open_settings_on};

/// What the fake host manager was asked, and the engines it handed out.
#[derive(Default)]
struct Calls {
    engines: Vec<(String, Arc<FakeEngine>)>,
    removed: Vec<String>,
    reconnected: Vec<String>,
    tested: Vec<String>,
    test_result: Option<Result<(), String>>,
}

type Shared = Arc<Mutex<Calls>>;

fn hosts_api(calls: &Shared) -> HostsApi {
    let (c1, c2, c3, c4, c5) = (
        calls.clone(),
        calls.clone(),
        calls.clone(),
        calls.clone(),
        calls.clone(),
    );
    HostsApi {
        // One engine per name, kept while the target stays the same.
        connect: Arc::new(move |entry: &HostEntry| {
            let mut c = c1.lock().expect("calls");
            if let Some((_, e)) = c.engines.iter().rev().find(|(n, _)| *n == entry.name) {
                return Ok(Arc::clone(e) as Arc<dyn EngineClient>);
            }
            let e = FakeEngine::new();
            c.engines.push((entry.name.clone(), Arc::clone(&e)));
            Ok(e as Arc<dyn EngineClient>)
        }),
        sync: Arc::new(|_| Vec::new()),
        remove: Arc::new(move |name: &str| {
            c2.lock().expect("calls").removed.push(name.to_owned());
            true
        }),
        reconnect: Arc::new(move |name: &str| {
            let mut c = c3.lock().expect("calls");
            c.reconnected.push(name.to_owned());
            let e = FakeEngine::new();
            c.engines.push((name.to_owned(), Arc::clone(&e)));
            Ok(e as Arc<dyn EngineClient>)
        }),
        test: Arc::new(move |target: &str| {
            let mut c = c4.lock().expect("calls");
            c.tested.push(target.to_owned());
            let (tx, rx) = async_channel::bounded(1);
            let _ = tx.try_send(c.test_result.clone().unwrap_or(Ok(())));
            rx
        }),
        status: Arc::new(move |name: &str| {
            c5.lock()
                .expect("calls")
                .engines
                .iter()
                .any(|(n, _)| n == name)
                .then_some(HostLinkStatus::Connected)
        }),
    }
}

struct Rig {
    calls: Shared,
    vcx: VisualTestContext,
    main: Entity<MainWindow>,
}

fn boot(cx: &mut TestAppContext, names: &[&'static str]) -> Rig {
    let calls: Shared = Arc::default();
    let client: Arc<dyn EngineClient> = FakeEngine::new();
    let (_tx, rx) = async_channel::bounded(4);
    let mut settings = Settings::default();
    settings.general.start_tab = StartTab::Processes;
    settings.hosts = names
        .iter()
        .map(|n| HostEntry {
            name: (*n).to_owned(),
            target: format!("me@{n}"),
            enabled: true,
        })
        .collect();
    let mut env = AppEnv::test();
    env.hosts = Some(hosts_api(&calls));
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env,
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
    vcx.run_until_parked();
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    let mut rig = Rig { calls, vcx, main };
    rig.draw();
    rig
}

impl Rig {
    fn draw(&mut self) {
        self.vcx.run_until_parked();
        self.vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn tab_names(&mut self) -> Vec<String> {
        self.vcx.update(|_, cx| {
            let tabs = self.main.read(cx).host_tabs_view().clone();
            tabs.read(cx)
                .tabs()
                .iter()
                .map(|t| t.name.to_string())
                .collect()
        })
    }

    fn host_count(&mut self) -> usize {
        self.vcx.read_entity(&self.main, |m, _| m.host_count())
    }

    fn entries(&mut self) -> Vec<(String, String, bool)> {
        self.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .hosts
                .iter()
                .map(|e| (e.name.clone(), e.target.clone(), e.enabled))
                .collect()
        })
    }

    /// Opens Settings on the Hosts page and draws it.
    fn open_settings(
        &mut self,
        cx: &mut TestAppContext,
    ) -> (Entity<HostsEditor>, VisualTestContext) {
        cx.update(|cx| {
            let host = HostContext::shown(cx);
            open_settings_on(&host, PageId::Hosts, cx);
        });
        cx.run_until_parked();
        let handle = cx
            .update(|cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .get(&WindowKey::Settings)
            })
            .and_then(|h| h.downcast::<Root>())
            .expect("settings window registered");
        let mut svcx = VisualTestContext::from_window(handle.into(), cx);
        svcx.update(|window, _| window.activate_window());
        svcx.run_until_parked();
        svcx.update(|window, cx| window.draw(cx).clear(cx));
        let editor = svcx
            .update(|_, cx| open_editor(cx))
            .expect("the Hosts page built its editor");
        (editor, svcx)
    }
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Add, edit, move and remove through the page change `[[hosts]]` and the tabs, with no restart.
#[gpui_kit::test]
fn settings_hosts_add_edit_remove_update_settings_and_tabs(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home"]);
    assert_eq!(rig.tab_names().len(), 2, "the local host and home");
    let (editor, mut svcx) = rig.open_settings(cx);
    assert_eq!(editor.read_with(&svcx, |e, _| e.row_count()), 1);

    // Add: a bad entry shows its reason and writes nothing.
    editor.update_in(&mut svcx, |e, window, cx| {
        e.type_add("", "me@lab", window, cx);
        e.add(cx);
    });
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.add_error()).as_deref(),
        Some("The host needs a name.")
    );
    assert_eq!(rig.entries().len(), 1);
    editor.update_in(&mut svcx, |e, window, cx| {
        e.type_add("HOME", "me@lab", window, cx);
        e.add(cx);
    });
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.add_error()).as_deref(),
        Some("Another host already has this name.")
    );
    editor.update_in(&mut svcx, |e, window, cx| {
        e.type_add("lab", " me@lab ", window, cx);
        e.add(cx);
    });
    draw(&mut svcx);
    rig.draw();
    assert_eq!(
        rig.entries(),
        [
            ("home".into(), "me@home".into(), true),
            ("lab".into(), "me@lab".into(), true)
        ]
    );
    assert_eq!(rig.tab_names()[1..], ["home", "lab"]);
    assert_eq!(rig.host_count(), 3, "the local host and two remote hosts");
    assert_eq!(editor.read_with(&svcx, |e, _| e.row_count()), 2);
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.add_error()),
        None,
        "a valid add clears the error"
    );
    assert_eq!(
        editor.read_with(&svcx, |e, cx| e.add_text(cx)),
        (String::new(), String::new())
    );

    // Edit: a bad rename keeps the entry, a good one renames the tab.
    editor.update_in(&mut svcx, |e, window, cx| {
        e.type_name(1, "  ", window, cx);
        e.commit(1, cx);
    });
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.row_error(1)).as_deref(),
        Some("The host needs a name.")
    );
    assert_eq!(rig.entries()[1].0, "lab");
    editor.update_in(&mut svcx, |e, window, cx| {
        e.type_name(1, "office", window, cx);
        e.type_target(1, "me@office.lan", window, cx);
        e.commit(1, cx);
    });
    draw(&mut svcx);
    rig.draw();
    assert_eq!(
        rig.entries()[1],
        ("office".into(), "me@office.lan".into(), true)
    );
    assert_eq!(editor.read_with(&svcx, |e, _| e.row_error(1)), None);
    assert!(
        rig.calls
            .lock()
            .expect("calls")
            .removed
            .contains(&"lab".to_owned())
    );
    assert_eq!(rig.tab_names()[2], "office");

    // Move up: the tab order follows.
    editor.update(&mut svcx, |e, cx| e.press_move(1, true, cx));
    draw(&mut svcx);
    rig.draw();
    assert_eq!(rig.tab_names()[1..], ["office", "home"]);
    assert_eq!(
        editor.read_with(&svcx, |e, cx| e.row_text(0, cx)),
        ("office".to_owned(), "me@office.lan".to_owned())
    );

    // Off: no tab, the entry stays.
    editor.update(&mut svcx, |e, cx| e.press_enabled(0, false, cx));
    draw(&mut svcx);
    rig.draw();
    assert_eq!(rig.host_count(), 2);
    assert!(!rig.entries()[0].2);

    // Remove: the entry and the tab go and the connection stops.
    editor.update(&mut svcx, |e, cx| e.press_remove(0, cx));
    draw(&mut svcx);
    rig.draw();
    assert_eq!(rig.entries(), [("home".into(), "me@home".into(), true)]);
    assert_eq!(rig.host_count(), 2);
    assert_eq!(editor.read_with(&svcx, |e, _| e.row_count()), 1);
}

/// "Test connection" asks the host manager about the typed destination and shows the answer.
#[gpui_kit::test]
fn settings_hosts_test_connection_shows_the_result(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home"]);
    let (editor, mut svcx) = rig.open_settings(cx);
    editor.update(&mut svcx, |e, cx| e.test(0, cx));
    svcx.run_until_parked();
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.test_line(0)),
        TestLine::Done(Ok(()))
    );
    assert_eq!(rig.calls.lock().expect("calls").tested, ["me@home"]);
    rig.calls.lock().expect("calls").test_result = Some(Err("ssh needs a key for home".into()));
    editor.update(&mut svcx, |e, cx| e.test(0, cx));
    svcx.run_until_parked();
    assert_eq!(
        editor.read_with(&svcx, |e, _| e.test_line(0)),
        TestLine::Done(Err("ssh needs a key for home".into()))
    );
}

/// Right-click on a remote tab opens Reconnect, Disconnect, Edit and Remove; the local tab has
/// no menu.
#[gpui_kit::test]
fn host_tab_context_menu_items(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home"]);
    let local = rig.vcx.debug_bounds("HostTab0").expect("local tab drawn");
    rig.vcx
        .simulate_mouse_down(local.center(), MouseButton::Right, Modifiers::none());
    rig.draw();
    assert!(
        rig.vcx
            .update(|window, cx| open_context_menu(window, cx).is_none())
    );
    let home = rig.vcx.debug_bounds("HostTab1").expect("home tab drawn");
    rig.vcx
        .simulate_mouse_down(home.center(), MouseButton::Right, Modifiers::none());
    rig.draw();
    let labels: Vec<String> = rig.vcx.update(|window, cx| {
        let menu = open_context_menu(window, cx).expect("the tab menu is open");
        menu.read(cx)
            .entries_for_test(window)
            .into_iter()
            .filter_map(|e| e.label.map(|l| l.to_string()))
            .collect()
    });
    assert_eq!(labels, ["Reconnect", "Disconnect", "Edit", "Remove"]);
}

/// Reconnect gives the host a new client and keeps it shown.
#[gpui_kit::test]
fn host_tab_reconnect_replaces_the_client(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home"]);
    rig.vcx
        .update(|window, cx| window.dispatch_action(Box::new(tl_ui::actions::SelectHost(1)), cx));
    rig.draw();
    let old = rig
        .vcx
        .read_entity(&rig.main, |m, _| m.host().client().cloned());
    rig.vcx
        .update(|window, cx| window.dispatch_action(Box::new(ReconnectHost("home".into())), cx));
    rig.draw();
    assert_eq!(rig.calls.lock().expect("calls").reconnected, ["home"]);
    let new = rig
        .vcx
        .read_entity(&rig.main, |m, _| m.host().client().cloned());
    let (old, new) = (old.expect("old client"), new.expect("new client"));
    assert!(!std::ptr::addr_eq(Arc::as_ptr(&old), Arc::as_ptr(&new)));
    assert_eq!(
        rig.vcx.read_entity(&rig.main, |m, _| m.shown_host_index()),
        1
    );
    let fresh = rig
        .calls
        .lock()
        .expect("calls")
        .engines
        .last()
        .map(|(_, e)| Arc::clone(e));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&new),
        Arc::as_ptr(&fresh.expect("engine"))
    ));
}

/// Disconnect switches the entry off, Edit opens Settings on the Hosts page, Remove deletes.
#[gpui_kit::test]
fn host_tab_disconnect_edit_and_remove(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    rig.vcx
        .update(|window, cx| window.dispatch_action(Box::new(DisconnectHost("home".into())), cx));
    rig.draw();
    assert_eq!(
        rig.entries(),
        [
            ("home".into(), "me@home".into(), false),
            ("office".into(), "me@office".into(), true)
        ]
    );
    assert_eq!(rig.host_count(), 2);
    assert!(
        rig.calls
            .lock()
            .expect("calls")
            .removed
            .contains(&"home".to_owned())
    );

    rig.vcx
        .update(|window, cx| window.dispatch_action(Box::new(EditHost("office".into())), cx));
    rig.vcx.run_until_parked();
    let handle: Option<WindowHandle<Root>> = cx
        .update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Settings)
        })
        .and_then(|h| h.downcast::<Root>());
    let handle = handle.expect("Edit opened Settings");
    let mut svcx = VisualTestContext::from_window(handle.into(), cx);
    svcx.update(|window, _| window.activate_window());
    draw(&mut svcx);
    let editor = svcx
        .update(|_, cx| open_editor(cx))
        .expect("Settings starts on the Hosts page");
    assert_eq!(editor.read_with(&svcx, |e, _| e.row_count()), 2);

    rig.vcx
        .update(|window, cx| window.dispatch_action(Box::new(RemoveHost("office".into())), cx));
    rig.draw();
    assert_eq!(rig.entries(), [("home".into(), "me@home".into(), false)]);
}
