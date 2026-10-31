//! Host tabs: one tab per host, per-host pages and page state, Ctrl+digit, the failed tooltip,
//! and windows that stay on the host they were opened from (UI-SHL-008, ADR-018 "Hosts",
//! tasklens-q4x76.27). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext,
    WindowHandle, point, px,
};
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionTarget, Signal};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand, Tab};
use tl_model::ids::NodeId;
use tl_model::settings::{HostEntry, Settings, StartTab, UiState};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
use tl_model::table::{ProcessTableView, TableRow};
use tl_model::validity::Validity;
use tl_ui::action_tracker::ToastTexts;
use tl_ui::actions::{CancelWindowPick, StartWindowPick};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::demand::DEMAND_COALESCE;
use tl_ui::host::HostContext;
use tl_ui::hosts_api::{HostLinkStatus, HostsApi};
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::pages::processes::delegate::ProcRowId;
use tl_ui::pages::services::ServicesPage;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{MainWindow, main_window_title, main_window_view, open_main_window};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::windows::props::{PropsWindow, open_process_properties};

use super::common::{key, record, toasts};

type Statuses = Arc<Mutex<HashMap<String, HostLinkStatus>>>;

struct Rig {
    local: Arc<FakeEngine>,
    remotes: HashMap<&'static str, Arc<FakeEngine>>,
    statuses: Statuses,
    vcx: VisualTestContext,
    main: Entity<MainWindow>,
}

/// A host manager over fake engines: `connect` gives each name its own engine, `status` reads
/// `statuses` (connected when not set).
fn hosts_api(remotes: &HashMap<&'static str, Arc<FakeEngine>>, statuses: &Statuses) -> HostsApi {
    let engines: HashMap<String, Arc<dyn EngineClient>> = remotes
        .iter()
        .map(|(name, e)| ((*name).to_owned(), Arc::clone(e) as Arc<dyn EngineClient>))
        .collect();
    let statuses = Arc::clone(statuses);
    HostsApi {
        connect: Arc::new(move |entry: &HostEntry| {
            engines
                .get(&entry.name)
                .cloned()
                .ok_or_else(|| format!("no fake engine for {}", entry.name))
        }),
        sync: Arc::new(|_| Vec::new()),
        remove: Arc::new(|_| true),
        reconnect: Arc::new(|name: &str| Err(format!("no reconnect for {name}"))),
        test: Arc::new(|_| {
            let (tx, rx) = async_channel::bounded(1);
            let _ = tx.try_send(Ok(()));
            rx
        }),
        status: Arc::new(move |name: &str| {
            Some(
                statuses
                    .lock()
                    .expect("statuses")
                    .get(name)
                    .cloned()
                    .unwrap_or(HostLinkStatus::Connected),
            )
        }),
    }
}

fn boot(cx: &mut TestAppContext, names: &[&'static str]) -> Rig {
    let local = FakeEngine::new();
    let client: Arc<dyn EngineClient> = local.clone();
    let remotes: HashMap<&'static str, Arc<FakeEngine>> =
        names.iter().map(|n| (*n, FakeEngine::new())).collect();
    let statuses: Statuses = Arc::default();
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
    env.hosts = Some(hosts_api(&remotes, &statuses));
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
    let mut rig = Rig {
        local,
        remotes,
        statuses,
        vcx,
        main,
    };
    rig.draw();
    rig
}

impl Rig {
    fn draw(&mut self) {
        self.vcx.run_until_parked();
        self.vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Lets the demand aggregators send what they hold.
    fn settle(&mut self) {
        self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        self.draw();
    }

    fn select_host(&mut self, index: usize) {
        self.main.update_in(&mut self.vcx, |m, window, cx| {
            m.select_host(index, window, cx)
        });
        self.draw();
    }

    fn select_tab(&mut self, tab: Tab) {
        self.main
            .update_in(&mut self.vcx, |m, window, cx| m.select_tab(tab, window, cx));
        self.draw();
    }

    fn shown(&mut self) -> usize {
        self.vcx
            .read_entity(&self.main, |m, _| m.shown_host_index())
    }

    fn active_tab(&mut self) -> Tab {
        self.vcx.read_entity(&self.main, |m, _| m.active_tab())
    }

    fn shown_name(&mut self) -> Option<Arc<str>> {
        self.vcx
            .read_entity(&self.main, |m, _| m.host().remote_name().cloned())
    }

    fn filter_text(&mut self, tab: Tab) -> String {
        self.vcx.update(|_, cx| {
            let filter = self.main.read(cx).filter_box().clone();
            filter.read(cx).text(tab).to_string()
        })
    }

    fn type_filter(&mut self, text: &str) {
        let filter = self
            .vcx
            .read_entity(&self.main, |m, _| m.filter_box().clone());
        for ch in text.chars() {
            filter.update_in(&mut self.vcx, |f, window, cx| {
                f.begin_typing(ch, window, cx)
            });
        }
        self.draw();
    }

    fn page<T: 'static>(&mut self, tab: Tab) -> Option<Entity<T>> {
        self.vcx
            .read_entity(&self.main, |m, _| m.page_entity::<T>(tab))
    }

    /// Sends an End task on `pid` through `host`'s tracker, as a page does.
    fn end_task(&mut self, host: &HostContext, pid: u32) -> Option<ActionId> {
        let tracker = host.tracker().expect("tracker").clone();
        let queue = self
            .vcx
            .read_entity(&self.main, |m, _| m.toasts().downgrade());
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::Signal(Signal::Term),
            targets: [ActionTarget::Process(key(pid))].into_iter().collect(),
            privileged: false,
        };
        let id = self.vcx.update(|_, cx| {
            tracker.update(cx, |t, cx| {
                t.track_or_toast(request, queue, ToastTexts::new("Ended", "Not ended."), cx)
            })
        });
        self.draw();
        id
    }

    /// The actions `engine` received.
    fn actions(engine: &FakeEngine) -> usize {
        engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::Action(_)))
            .count()
    }

    fn banner(&mut self) -> Option<String> {
        self.vcx.read_entity(&self.main, |m, _| {
            m.reconnect_banner().map(ToString::to_string)
        })
    }

    /// Runs the hubs' 500 ms link check.
    fn tick(&mut self) {
        self.vcx
            .executor()
            .advance_clock(Duration::from_millis(600));
        self.draw();
    }

    /// The host tabs as last drawn: (label, selected, tooltip).
    fn drawn_tabs(&mut self) -> Vec<(String, bool, Option<String>)> {
        self.vcx.update(|_, cx| {
            let tabs = self.main.read(cx).host_tabs_view().clone();
            tabs.read(cx)
                .drawn()
                .iter()
                .map(|t| {
                    (
                        t.label.to_string(),
                        t.selected,
                        t.tooltip.as_ref().map(ToString::to_string),
                    )
                })
                .collect()
        })
    }
}

/// UI-SHL-008: with one host nothing changes. No host tabs, and Ctrl+2 still selects the
/// second page tab.
#[gpui_kit::test]
fn host_tabs_hidden_with_one_host(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &[]);
    assert_eq!(rig.vcx.read_entity(&rig.main, |m, _| m.host_count()), 1);
    assert!(rig.drawn_tabs().is_empty());
    assert!(rig.vcx.read_entity(&rig.main, |m, cx| {
        m.host_tabs_view().read(cx).tabs().is_empty()
    }));
    assert_eq!(rig.active_tab(), Tab::Processes);
    rig.vcx.simulate_keystrokes("ctrl-2");
    rig.draw();
    assert_eq!(rig.active_tab(), Tab::Performance);
    assert_eq!(rig.shown(), 0);
}

/// Two remote hosts: tabs local first, then in `[[hosts]]` order. Each host keeps its own main
/// tab, filter and page entities; pages are built the first time a host is shown.
#[gpui_kit::test]
fn host_tabs_switch_keeps_page_state(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    let tabs = rig.drawn_tabs();
    assert_eq!(tabs.len(), 3);
    assert!(tabs[0].1, "the local host is shown first: {tabs:?}");
    assert!(tabs[1].0.starts_with("home"), "{tabs:?}");
    assert!(tabs[2].0.starts_with("office"), "{tabs:?}");
    assert_eq!(rig.shown_name(), None);
    let local_processes = rig.page::<ProcessesPage>(Tab::Processes).expect("built");

    // Local: filter "fire" on Processes.
    rig.type_filter("fire");
    assert_eq!(rig.filter_text(Tab::Processes), "fire");

    // home: its own Processes page, an empty filter. Move to Services and filter "ssh".
    rig.select_host(1);
    assert_eq!(rig.shown(), 1);
    assert_eq!(rig.shown_name().as_deref(), Some("home"));
    assert_eq!(rig.active_tab(), Tab::Processes);
    assert_eq!(rig.filter_text(Tab::Processes), "");
    let home_processes = rig
        .page::<ProcessesPage>(Tab::Processes)
        .expect("built on show");
    assert_ne!(home_processes, local_processes);
    assert!(rig.drawn_tabs()[1].1, "home is selected");
    let title = rig
        .vcx
        .read_entity(&rig.main, |m, _| main_window_title(m.host()));
    assert_eq!(title, "tasklens - home");
    rig.vcx.update(|_, cx| {
        let shown = HostContext::shown(cx);
        assert_eq!(shown.remote_name().map(|n| &**n), Some("home"));
    });
    rig.select_tab(Tab::Services);
    rig.type_filter("ssh");
    let home_services = rig.page::<ServicesPage>(Tab::Services).expect("built");
    // The home pages ask the home engine, not the local one.
    rig.settle();
    assert!(
        rig.remotes["home"]
            .demands()
            .iter()
            .any(|d| d.services && d.any_surface_visible),
        "home got the Services demand"
    );

    // office was never shown: nothing is built for it.
    // Back to local: Processes with "fire", the same page entity.
    rig.select_host(0);
    assert_eq!(rig.shown_name(), None);
    assert_eq!(rig.active_tab(), Tab::Processes);
    assert_eq!(rig.filter_text(Tab::Processes), "fire");
    assert_eq!(
        rig.page::<ProcessesPage>(Tab::Processes),
        Some(local_processes.clone())
    );
    assert_eq!(rig.page::<ServicesPage>(Tab::Services), None);

    // Back to home: Services with "ssh", the same page entities.
    rig.select_host(1);
    assert_eq!(rig.active_tab(), Tab::Services);
    assert_eq!(rig.filter_text(Tab::Services), "ssh");
    assert_eq!(rig.filter_text(Tab::Processes), "");
    assert_eq!(rig.page::<ServicesPage>(Tab::Services), Some(home_services));
    assert_eq!(
        rig.page::<ProcessesPage>(Tab::Processes),
        Some(home_processes)
    );

    // office: built on its first show, on the tab shown at that moment.
    rig.select_host(2);
    assert_eq!(rig.shown_name().as_deref(), Some("office"));
    assert_eq!(rig.active_tab(), Tab::Services);
    assert_eq!(rig.filter_text(Tab::Services), "");
    assert!(rig.page::<ProcessesPage>(Tab::Processes).is_none());
}

/// Ctrl+1 to Ctrl+9 pick hosts in tab order while there is more than one host; Alt+digit
/// still picks page tabs.
#[gpui_kit::test]
fn host_tabs_ctrl_digit(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    rig.vcx.simulate_keystrokes("ctrl-3");
    rig.draw();
    assert_eq!(rig.shown(), 2);
    assert_eq!(rig.active_tab(), Tab::Processes);
    rig.vcx.simulate_keystrokes("ctrl-2");
    rig.draw();
    assert_eq!(rig.shown(), 1);
    // No fourth host: nothing changes.
    rig.vcx.simulate_keystrokes("ctrl-9");
    rig.draw();
    assert_eq!(rig.shown(), 1);
    rig.vcx.simulate_keystrokes("alt-2");
    rig.draw();
    assert_eq!(rig.active_tab(), Tab::Performance);
    assert_eq!(rig.shown(), 1);
    rig.vcx.simulate_keystrokes("ctrl-1");
    rig.draw();
    assert_eq!(rig.shown(), 0);
    assert_eq!(rig.active_tab(), Tab::Processes);
}

/// A failed host shows the reason as its tab tooltip; a connected one has none.
#[gpui_kit::test]
fn host_tab_failed_tooltip(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    rig.statuses.lock().expect("statuses").insert(
        "office".into(),
        HostLinkStatus::Failed(Arc::from("ssh needs a key for office")),
    );
    rig.vcx.executor().advance_clock(Duration::from_millis(600));
    rig.draw();
    let tabs = rig.drawn_tabs();
    assert_eq!(tabs[1].2, None, "{tabs:?}");
    assert_eq!(
        tabs[2].2.as_deref(),
        Some("Can't connect: ssh needs a key for office"),
        "{tabs:?}"
    );
}

/// The same ProcessKey on the local host and on home opens two Properties windows, one per
/// host: the second open does not focus the first host's window.
#[gpui_kit::test]
fn host_tabs_same_process_key_on_two_hosts_opens_two_windows(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home"]);
    let pid = 4321;
    let publish = |engine: &Arc<FakeEngine>| {
        engine.publish_with(|s| {
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                records: vec![record(pid)].into(),
                ..ProcessSnapshot::default()
            });
        });
    };
    publish(&rig.local);
    publish(&rig.remotes["home"]);
    rig.draw();
    let local = rig.vcx.read_entity(&rig.main, |m, _| m.host().clone());
    rig.vcx
        .update(|_, cx| open_process_properties(&local, key(pid), cx));
    rig.draw();
    rig.select_host(1);
    let home = rig.vcx.read_entity(&rig.main, |m, _| m.host().clone());
    assert!(!home.same_host(&local));
    rig.vcx
        .update(|_, cx| open_process_properties(&home, key(pid), cx));
    rig.draw();

    let (local_window, home_window) = rig.vcx.update(|_, cx| {
        let registry = cx.global::<GlobalRegistry>().0.clone();
        let registry = registry.read(cx);
        let local_key = WindowKey::Properties(key(pid)).on_host(local.remote_name());
        let home_key = WindowKey::Properties(key(pid)).on_host(home.remote_name());
        assert_eq!(local_key, WindowKey::Properties(key(pid)));
        assert_ne!(local_key, home_key);
        assert_eq!(
            registry.count(tl_ui::window::registry::CapClass::ProcessProps),
            2
        );
        (
            registry.get(&local_key).expect("the local window"),
            registry.get(&home_key).expect("the home window"),
        )
    });
    assert_ne!(local_window.window_id(), home_window.window_id());
    rig.vcx.update(|_, cx| {
        let local_props = PropsWindow::find(local_window, cx).expect("local props");
        let home_props = PropsWindow::find(home_window, cx).expect("home props");
        assert_eq!(local_props.read(cx).host().remote_name(), None);
        assert_eq!(
            home_props.read(cx).host().remote_name().map(|n| &**n),
            Some("home")
        );
    });
}

/// A Properties window opened on home stays on home after the main window shows another host:
/// its host, its title and its demand do not follow the tab.
#[gpui_kit::test]
fn host_tabs_properties_window_stays_on_its_host(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    rig.select_host(1);
    let pid = 4321;
    rig.remotes["home"].publish_with(|s| {
        s.processes = Arc::new(ProcessSnapshot {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            records: vec![record(pid)].into(),
            ..ProcessSnapshot::default()
        });
    });
    rig.draw();
    let host = rig.vcx.read_entity(&rig.main, |m, _| m.host().clone());
    rig.vcx
        .update(|_, cx| open_process_properties(&host, key(pid), cx));
    rig.draw();
    let handle: AnyWindowHandle = rig
        .vcx
        .update(|_, cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let window_key = WindowKey::Properties(key(pid)).on_host(host.remote_name());
            registry.read(cx).get(&window_key)
        })
        .expect("the properties window opens on home");
    let props = rig
        .vcx
        .update(|_, cx| PropsWindow::find(handle, cx))
        .expect("props window");

    rig.select_host(0);
    assert_eq!(rig.shown_name(), None);
    rig.vcx.update(|_, cx| {
        let p = props.read(cx);
        assert_eq!(p.host().remote_name().map(|n| &**n), Some("home"));
        assert!(p.host().same_host(&host));
        assert!(!p.host().same_host(&HostContext::shown(cx)));
    });
    let title = rig.vcx.update(|_, cx| {
        let frame = TlSecondaryWindow::<PropsWindow>::find(
            WindowHandle::<Root>::new(handle.window_id()),
            cx,
        )
        .expect("props frame");
        frame.read(cx).title().to_string()
    });
    assert!(title.ends_with(" - home"), "{title}");
    // Its detail demand goes to home's engine, never to the local one.
    rig.settle();
    assert!(
        rig.remotes["home"]
            .demands()
            .iter()
            .any(|d| d.details.iter().any(|t| format!("{t:?}").contains("4321"))),
        "home got the Properties detail demand"
    );
    assert!(
        rig.local
            .demands()
            .iter()
            .all(|d| d.details.iter().all(|t| !format!("{t:?}").contains("4321"))),
        "the local engine never got it"
    );
}

/// One process row for `pid`: its record and the table row that shows it.
fn with_process_row(s: &mut Snapshot, pid: u32) {
    let header = ComponentHeader {
        version: s.processes.header.version + 1,
        validity: Validity::Valid,
        ..ComponentHeader::default()
    };
    s.processes = Arc::new(ProcessSnapshot {
        header: header.clone(),
        records: vec![record(pid)].into(),
        ..ProcessSnapshot::default()
    });
    s.table = Arc::new(ProcessTableView {
        header,
        rows: vec![TableRow {
            node: NodeId::Process(key(pid)),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(0),
            group: None,
            descendants: 0,
            subtree: None,
        }]
        .into(),
        ..ProcessTableView::default()
    });
}

/// UI-SHL-008, ADR-018 Reconnect: when home's connection drops, its pages keep the last data at
/// 60% opacity under "Reconnecting to home" and every action is off until the first snapshot of
/// the next session. A mouse click on the page does not reach it, Delete on the focused table
/// is refused with a toast, and a direct command (window picking) is not sent. An action sent
/// in the lost session ends as outcome unknown. The local host and office stay live.
#[gpui_kit::test]
fn host_tab_reconnect_disables_actions(cx: &mut TestAppContext) {
    let mut rig = boot(cx, &["home", "office"]);
    let pid = 4321;
    rig.remotes["home"].publish_with(|s| with_process_row(s, pid));
    rig.select_host(1);
    let home = rig.vcx.read_entity(&rig.main, |m, _| m.host().clone());
    let local = rig.vcx.update(|_, cx| HostContext::local(cx));
    let office = rig.vcx.update(|_, cx| {
        tl_ui::host::HostRegistry::tabs(cx)
            .into_iter()
            .find(|t| &*t.name == "office")
            .expect("office")
            .host
    });
    assert_eq!(rig.banner(), None);
    assert_eq!(
        rig.vcx.read_entity(&rig.main, |m, _| m.reconnect_cover()),
        None
    );
    assert!(rig.vcx.debug_bounds("MainWindowReconnect").is_none());

    let page = rig
        .page::<ProcessesPage>(Tab::Processes)
        .expect("home's Processes page");
    let table = rig.vcx.read_entity(&page, |p, _| p.table().clone());
    let row = ProcRowId::Process(key(pid));
    let body = rig
        .vcx
        .debug_bounds("TlTreeTable-body")
        .expect("table body");
    // Below the only row: a press there clears the selection and focuses the table.
    let below = point(body.origin.x + px(40.), body.bottom() - px(8.));
    let select = |rig: &mut Rig| {
        rig.vcx.update(|window, cx| {
            table.update(cx, |t, cx| {
                t.selection.click(row);
                cx.notify();
            });
            page.update(cx, |p, cx| p.focus_table(window, cx));
        });
        rig.draw();
    };
    let selected = |rig: &mut Rig| rig.vcx.read_entity(&table, |t, _| t.selection.len());
    let picks = |engine: &FakeEngine| {
        engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::KWin(KWinCommand::Pick { .. })))
            .count()
    };

    // Live: a click below the row reaches the page and clears the selection.
    select(&mut rig);
    assert_eq!(selected(&mut rig), 1);
    rig.vcx.simulate_click(below, Modifiers::none());
    rig.draw();
    assert_eq!(selected(&mut rig), 0, "live, the click reaches the table");

    // Live: Delete on the focused table sends End task to home, which waits for its result.
    select(&mut rig);
    rig.vcx.simulate_keystrokes("delete");
    rig.draw();
    assert_eq!(Rig::actions(&rig.remotes["home"]), 1);
    let tracker = home.tracker().expect("tracker").clone();
    assert_eq!(rig.vcx.read_entity(&tracker, |t, _| t.pending_len()), 1);

    // The connection drops: the client's generation goes up at once, no snapshot follows.
    rig.statuses
        .lock()
        .expect("statuses")
        .insert("home".into(), HostLinkStatus::Connecting);
    rig.remotes["home"].bump_generation();
    rig.tick();
    assert_eq!(rig.banner().as_deref(), Some("Reconnecting to home"));
    let page_bounds = rig.vcx.debug_bounds("MainWindowPage").expect("page");
    let cover = rig.vcx.debug_bounds("MainWindowReconnect").expect("cover");
    assert_eq!(cover, page_bounds, "the cover lies over the whole page");
    // The page is dimmed: the cover is the window background at 40% alpha, so the page shows
    // through at 60%.
    let (dim, window_bg) = rig
        .vcx
        .read_entity(&rig.main, |m, _| m.reconnect_cover())
        .expect("the cover is drawn");
    assert_eq!(
        (dim.h, dim.s, dim.l),
        (window_bg.h, window_bg.s, window_bg.l)
    );
    assert!((dim.a - window_bg.a * 0.4).abs() < 1e-4, "{dim:?}");
    // The last data is still there.
    let rows = rig.vcx.update(|_, cx| {
        let hub = home.hub().expect("hub").read(cx);
        hub.current().processes.records.len()
    });
    assert_eq!(rows, 1);
    // The action from the lost session ended once, as outcome unknown.
    assert_eq!(rig.vcx.read_entity(&tracker, |t, _| t.pending_len()), 0);
    let pushed = |rig: &mut Rig| {
        rig.vcx.read_entity(&tracker, |t, _| {
            t.pushed()
                .iter()
                .map(|p| p.text.to_string())
                .collect::<Vec<_>>()
        })
    };
    let lost = pushed(&mut rig);
    assert!(
        lost.iter().any(|t| t == "Outcome unknown"),
        "the lost action: {lost:?}"
    );

    // A mouse click on the page stops at the cover: the selection stays.
    select(&mut rig);
    assert!(cover.contains(&below));
    rig.vcx.simulate_click(below, Modifiers::none());
    rig.draw();
    assert_eq!(selected(&mut rig), 1, "the click did not reach the table");

    // Delete on the focused table is refused on home, and says why.
    rig.vcx.simulate_keystrokes("delete");
    rig.draw();
    assert_eq!(Rig::actions(&rig.remotes["home"]), 1);
    assert_eq!(
        pushed(&mut rig).last().map(String::as_str),
        Some("Not sent while reconnecting to home.")
    );
    // So is an action from code outside the page.
    assert_eq!(rig.end_task(&home, pid), None);
    assert_eq!(Rig::actions(&rig.remotes["home"]), 1);

    // A command sent without the tracker is not sent either: window picking.
    rig.vcx.dispatch_action(StartWindowPick);
    rig.draw();
    assert_eq!(picks(&rig.remotes["home"]), 0);
    assert!(
        toasts(&mut rig.vcx)
            .iter()
            .any(|t| t == "Not sent while reconnecting to home."),
        "{:?}",
        toasts(&mut rig.vcx)
    );

    // The other hosts are live: their actions go out.
    assert!(rig.end_task(&local, pid).is_some());
    assert_eq!(Rig::actions(&rig.local), 1);
    assert!(rig.end_task(&office, pid).is_some());
    assert_eq!(Rig::actions(&rig.remotes["office"]), 1);
    rig.select_host(0);
    assert_eq!(rig.banner(), None);
    rig.select_host(2);
    assert_eq!(rig.banner(), None);
    assert!(rig.vcx.debug_bounds("MainWindowReconnect").is_none());
    rig.vcx.dispatch_action(StartWindowPick);
    rig.draw();
    assert_eq!(picks(&rig.remotes["office"]), 1, "office still picks");
    rig.vcx.dispatch_action(CancelWindowPick);
    rig.draw();

    // Back on home: still reconnecting. The first snapshot of the next session ends it.
    rig.select_host(1);
    assert_eq!(rig.banner().as_deref(), Some("Reconnecting to home"));
    let mut fresh = tl_ui::testing::fake_engine::empty_snapshot();
    fresh.sequence = 1;
    with_process_row(&mut fresh, pid);
    rig.statuses
        .lock()
        .expect("statuses")
        .insert("home".into(), HostLinkStatus::Connected);
    rig.remotes["home"].restart(fresh);
    rig.draw();
    assert_eq!(rig.banner(), None);
    assert_eq!(
        rig.vcx.read_entity(&rig.main, |m, _| m.reconnect_cover()),
        None
    );
    assert!(rig.vcx.debug_bounds("MainWindowReconnect").is_none());
    // Clicks and keys reach the page again.
    select(&mut rig);
    rig.vcx.simulate_click(below, Modifiers::none());
    rig.draw();
    assert_eq!(selected(&mut rig), 0, "the click reaches the table again");
    select(&mut rig);
    rig.vcx.simulate_keystrokes("delete");
    rig.draw();
    assert_eq!(Rig::actions(&rig.remotes["home"]), 2);
}
