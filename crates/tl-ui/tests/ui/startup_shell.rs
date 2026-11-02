//! Startup page tests that need `MainWindow`: the filter box, the tab strip, the status chip and
//! the keymap at app init. The other Startup tests moved to tl-ui-sys (tasklens-zljr5.43).

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::snapshot::{Impact, StartupEntry, StartupKind, StartupScope, StartupStatus};
use tl_model::table::{StartupRef, StartupSource};

fn entry(name: &str, kind: StartupKind) -> StartupEntry {
    StartupEntry {
        id: StartupRef {
            source: StartupSource::AutostartUser,
            id: Arc::from(format!("{name}.desktop")),
        },
        name: Arc::from(name),
        icon_path: None,
        publisher: Some(Arc::from("app 1.0")),
        status: StartupStatus::Enabled,
        impact: Impact::Low,
        impact_detail: None,
        state_code: None,
        kind,
        location: Arc::from("/home/u/.config/autostart"),
        command: Arc::from("/usr/bin/app --tray"),
        source_path: Arc::from("/home/u/.config/autostart/app.desktop"),
        can_toggle: true,
        can_delete: true,
        can_run: true,
        overridden_by_user: false,
        resolved_path: None,
        file_exists: true,
        source_word: "XDG autostart",
        scope: StartupScope::User,
        only_show_in: Default::default(),
        not_show_in: Default::default(),
        try_exec: None,
        autostart_phase: None,
        generated_unit: None,
        overrides: None,
    }
}

mod filter {
    use super::*;
    use gpui_kit::{AppContext as _, Context, VisualTestContext, Window};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::host::HostContext;
    use tl_ui::pages::startup::StartupPage;
    use tl_ui::pages::startup::actions::bind_keys_startup;
    use tl_ui::window::filter::FILTER_DEBOUNCE;
    use tl_ui::window::main::{
        MainWindow, PageSlot, main_window_view, open_main_window_on, set_test_page,
    };
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    fn startup_page(window: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let host_ctx = HostContext::local(cx);
        let page = cx.new(|cx| StartupPage::new(&host_ctx, window, cx));
        PageSlot::new(page, cx)
    }

    #[gpui_kit::test]
    fn startup_filter_sends_query(cx: &mut TestAppContext) {
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
        let handle = cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            bind_keys_startup(cx);
            set_test_page(cx, Tab::Startup, startup_page);
            open_main_window_on(Tab::Startup, cx).expect("main window opens")
        });
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let (main, page) = vcx.update(|_, cx| {
            let main = main_window_view(cx).expect("main window view");
            let page = main
                .read(cx)
                .page_entity::<StartupPage>(Tab::Startup)
                .expect("startup page");
            (main, page)
        });
        let focus = vcx.update(|_, cx| page.read(cx).table().read(cx).focus_handle().clone());
        vcx.update(|window, cx| window.focus(&focus, cx));
        vcx.simulate_keystrokes("d");
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let filter = vcx.read_entity(&main, |main, _| main.filter_box().clone());
        let (text, focused) = vcx.update(|window, cx| {
            let filter = filter.read(cx);
            (
                filter.input().read(cx).text().to_string(),
                filter.is_focused(window, cx),
            )
        });
        assert_eq!(text, "d");
        assert!(focused, "printable key moves focus to the filter box");

        vcx.executor().advance_clock(FILTER_DEBOUNCE);
        vcx.run_until_parked();
        let query = engine
            .commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetSysTableQuery(query) => Some(query),
                _ => None,
            })
            .expect("filter query sent");
        assert_eq!(query.filter.as_ref(), "d");
    }
}

/// `TasklensUi::init` binds the Startup keys and the shell builds `StartupPage` for its tab
/// (tasklens-9o0.13).
#[gpui_kit::test]
fn startup_keymap_registered(cx: &mut TestAppContext) {
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::startup::StartupPage;
    use tl_ui::pages::startup::actions::StartupDelete;
    use tl_ui::window::main::{main_window_view, open_main_window};
    use tl_ui_settings::settings::{Settings, StartTab, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let mut settings = Settings::default();
    settings.general.start_tab = StartTab::Startup;
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();

    cx.update(|cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        assert!(
            keymap.bindings_for_action(&StartupDelete).any(|b| {
                b.keystrokes().len() == 1
                    && b.keystrokes()[0].unparse() == "delete"
                    && b.predicate()
                        .is_some_and(|p| p.to_string() == "StartupTable")
            }),
            "delete in StartupTable maps to StartupDelete"
        );
    });

    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    let page = vcx.read_entity(&main, |m, _| m.page_entity::<StartupPage>(Tab::Startup));
    assert!(page.is_some(), "the Startup tab builds a StartupPage");
}

mod main_window {
    use super::*;
    use gpui_kit::AppContext as _;
    use gpui_kit::{Entity, VisualTestContext, px};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::snapshot::{ComponentHeader, StartupCounts, StartupSnapshot};
    use tl_model::table::{SysNodeId, SysRow, SysTable};
    use tl_model::validity::Validity;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::pages::startup::StartupPage;
    use tl_ui::pages::startup::delegate::StartupDelegate;
    use tl_ui::table::tree_table::TreeTableDelegate as _;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};
    use tl_ui_settings::settings::{Settings, StartTab, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        main: Entity<MainWindow>,
        version: u64,
    }

    fn boot(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.general.start_tab = StartTab::Processes;
        let init = UiInit {
            settings,
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
        cx.run_until_parked();
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        Rig {
            engine,
            vcx,
            main,
            version: 0,
        }
    }
    fn fixture() -> Vec<StartupEntry> {
        let discord = super::entry("Discord", StartupKind::AutostartUser);
        let mut kde = super::entry("KDE Connect", StartupKind::AutostartSystem);
        kde.id.source = StartupSource::AutostartSystem;
        kde.scope = StartupScope::System;
        kde.can_delete = false;
        let mut syncthing = super::entry("syncthing", StartupKind::UserUnit);
        syncthing.id.source = StartupSource::UserUnit;
        let mut ssh = super::entry("ssh-agent.sh", StartupKind::PlasmaPreStartup);
        ssh.id.source = StartupSource::PlasmaEnv;
        ssh.status = StartupStatus::Disabled;
        vec![discord, kde, syncthing, ssh]
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn select(&mut self, tab: Tab) {
            self.main
                .update_in(&mut self.vcx, |m, window, cx| m.select_tab(tab, window, cx));
            self.draw();
        }

        fn page(&mut self) -> Option<Entity<StartupPage>> {
            self.vcx.read_entity(&self.main, |m, _| {
                m.page_entity::<StartupPage>(Tab::Startup)
            })
        }

        fn table(&mut self) -> Entity<tl_ui::table::tree_table::TlTreeTable<StartupDelegate>> {
            let page = self.page().expect("the Startup page exists");
            self.vcx.read_entity(&page, |page, _| page.table().clone())
        }

        /// Publishes `entries` for the last Startup query the page sent.
        fn publish(&mut self, entries: Vec<StartupEntry>) {
            self.version += 1;
            let version = self.version;
            let generation = self
                .engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) if query.table == SysTable::Startup => {
                        Some(query.generation)
                    }
                    _ => None,
                })
                .expect("the page sent a Startup query");
            let rows: Vec<SysRow> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| SysRow {
                    node: SysNodeId::Startup(entry.id.clone()),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(index as u32),
                })
                .collect();
            let enabled = entries
                .iter()
                .filter(|e| e.status == StartupStatus::Enabled)
                .count() as u32;
            let counts = StartupCounts {
                total: entries.len() as u32,
                enabled,
                disabled: entries.len() as u32 - enabled,
            };
            self.engine.publish_with(|snapshot| {
                snapshot.startup = Arc::new(StartupSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: rows.into(),
                    query_generation: generation,
                    entries: entries.into(),
                    counts,
                });
            });
            self.draw();
        }

        /// Visits the Startup tab and publishes the fixture.
        fn visit_with_fixture(&mut self) {
            self.select(Tab::Startup);
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.publish(fixture());
        }

        fn chip(&mut self) -> String {
            let chip = self
                .vcx
                .read_entity(&self.main, |m, _| m.status_chip().clone());
            self.vcx.read_entity(&chip, |c, _| c.drawn_text())
        }

        fn page_status(&mut self) -> String {
            let page = self.page().expect("the Startup page exists");
            self.vcx.read_entity(&page, |p, _| {
                let mut out = String::new();
                p.status_text(&mut out);
                out
            })
        }
    }

    #[gpui_kit::test]
    fn startup_tab_mounted_and_kept(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        assert!(rig.page().is_none(), "no page before the first visit");
        rig.vcx.simulate_keystrokes("ctrl-4");
        rig.draw();
        assert_eq!(
            rig.vcx.read_entity(&rig.main, |m, _| m.active_tab()),
            Tab::Startup
        );
        let first = rig.page().expect("Ctrl+4 builds the Startup page");
        let expected = rig.page_status();
        assert!(!expected.is_empty(), "the page has status text");
        assert_eq!(rig.chip(), expected, "the chip shows the page text");
        rig.select(Tab::Processes);
        rig.select(Tab::Startup);
        let again = rig.page().expect("the page is kept");
        assert_eq!(first.entity_id(), again.entity_id());
    }
    #[gpui_kit::test]
    fn startup_page_layout_golden(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        let header = rig
            .vcx
            .debug_bounds("TlTreeTableHeader")
            .expect("header drawn");
        let mut last_x = px(-1.);
        for key in [
            "name",
            "publisher",
            "status",
            "impact",
            "type",
            "location",
            "command",
        ] {
            let b = rig
                .vcx
                .debug_bounds(format!("TlTreeTableHeader-label-{key}").leak())
                .unwrap_or_else(|| panic!("{key} label drawn"));
            assert!(b.origin.x > last_x, "{key} is right of the one before");
            assert!(
                b.origin.y >= header.origin.y && b.bottom() <= header.bottom(),
                "{key} sits in the header"
            );
            last_x = b.origin.x;
        }
        let table = rig.table();
        assert_eq!(
            rig.vcx.read_entity(&table, |t, _| t.delegate.row_count()),
            4
        );
        let body = rig
            .vcx
            .debug_bounds("TlTreeTable-body")
            .expect("body drawn");
        assert!(
            body.origin.y >= header.bottom(),
            "rows are below the header"
        );
        assert_eq!(rig.chip(), "4 Total | 3 Enabled | 1 Disabled");
    }
    #[gpui_kit::test]
    fn startup_demand_cleared_on_tab_switch(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        rig.vcx.run_until_parked();
        assert!(
            rig.engine.demands().last().is_some_and(|d| d.startup),
            "the Startup tab asks for startup data"
        );
        rig.select(Tab::Processes);
        rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        rig.vcx.run_until_parked();
        assert!(
            rig.engine.demands().last().is_some_and(|d| !d.startup),
            "leaving the tab clears the demand"
        );
    }
}
