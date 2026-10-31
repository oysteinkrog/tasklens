//! Host tabs: one tab per host, per-host pages and page state, Ctrl+digit, the failed tooltip,
//! and windows that stay on the host they were opened from (UI-SHL-008, ADR-018 "Hosts",
//! tasklens-q4x76.27). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext, WindowHandle, px,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::settings::{HostEntry, Settings, StartTab, UiState};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::demand::DEMAND_COALESCE;
use tl_ui::host::HostContext;
use tl_ui::hosts_api::{HostLinkStatus, HostsApi};
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::pages::services::ServicesPage;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{MainWindow, main_window_title, main_window_view, open_main_window};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::windows::props::{PropsWindow, open_process_properties};

use super::common::{key, record};

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
            registry.read(cx).get(&WindowKey::Properties(key(pid)))
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
