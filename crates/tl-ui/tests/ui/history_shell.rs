//! History mode page: header text and render on the HISTORY bit only (tasklens-8fx.19), and the
//! query controller with one Frame query in flight (tasklens-8fx.20), and the History table
//! (tasklens-8fx.22), and queries on strip window, metric and selection changes
//! (tasklens-8fx.38), and the timeline strip's cached geometry and gap bands (tasklens-8fx.24), and
//! the filter text typed before the page existed (tasklens-8fx.39), and the top-3 hover card
//! (tasklens-8fx.26), and the minimap (tasklens-8fx.27), and the legend chips (tasklens-8fx.28), and
//! the Live button, recording-off banner and toast and load-problem toasts (tasklens-8fx.29), and
//! the Settings window History rows (tasklens-8fx.30).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, HistoryWhat};
use tl_model::history::{
    HistoryMetric, HistoryProcessRow, HistoryRange, HistoryStoreState, HistoryTotals, HistoryView,
};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::snapshot::ComponentHeader;
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::window::settings::{PageBuilder, PageId, all_pages};
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::settings_global::{SettingKey, TlSettings};
use tl_ui_procs::pages::processes::history::HistoryPage;
use tl_ui_procs::pages::processes::history::table::HistoryTableDelegate;
use tl_ui_settings::settings::{
    AppearanceSettings, HistorySettings, Settings, ThemeSettings, UiState,
};
use tl_ui_table::table::tree_table::{TlTreeTable, TreeTableDelegate};
use tl_ui_testkit::fake_engine::FakeEngine;

const S: TimeNs = 1_000_000_000;

/// A view with a frame, 213 processes and a span ending at `newest`.
fn view(newest: TimeNs) -> HistoryView {
    HistoryView {
        span: Some((0, newest)),
        frame_t: Some(newest),
        process_count: 213,
        ..HistoryView::default()
    }
}

fn init(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        tl_ui_base::i18n::init_strings(cx, "en-US");
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        cx.set_global(GlobalHub(hub));
    });
    engine
}

/// The `(id, range, what)` of every history query sent.
fn queries(engine: &FakeEngine) -> Vec<(u64, HistoryRange, HistoryWhat)> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::HistoryQuery { id, range, what } => Some((id, range, what)),
            _ => None,
        })
        .collect()
}

fn overview(what: &HistoryWhat) -> bool {
    matches!(what, HistoryWhat::Frame { overview: true, .. })
}

/// Publishes a history view answering query `id`.
fn answer(engine: &FakeEngine, id: u64, vcx: &mut VisualTestContext) {
    engine.publish_with(|s| {
        let version = s.history.header.version + 1;
        s.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            query_id: id,
            ..view(700 * S)
        });
    });
    vcx.run_until_parked();
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid) * 10,
        boot_id: BootId(1),
        pid,
    }
}

/// A present row with `cpu` percent and `pid` MiB of memory.
fn prow(pid: u32, name: &str, cpu: f32) -> HistoryProcessRow {
    HistoryProcessRow {
        key: key(pid),
        name: Arc::from(name),
        comm: Arc::from(name),
        user: Arc::from("alice"),
        exe: Some(Arc::from(format!("/usr/bin/{name}").as_str())),
        app_id: None,
        system: false,
        present: true,
        previous_boot: false,
        cpu_pct: cpu,
        mem_anon_bytes: u64::from(pid) << 20,
        disk_bps: 0.0,
        net_bps: 0.0,
        gpu_pct: 0.0,
        peak: None,
    }
}

/// Publishes a view at a scrub second with `rows`.
fn publish_rows(
    engine: &FakeEngine,
    vcx: &mut VisualTestContext,
    range: HistoryRange,
    rows: Vec<HistoryProcessRow>,
) {
    engine.publish_with(|s| {
        let version = s.history.header.version + 1;
        s.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            store_state: HistoryStoreState::Ready,
            range,
            totals: Some(HistoryTotals {
                cpu_pct: 12.5,
                mem_used_bytes: 4 << 30,
                mem_total_bytes: 16 << 30,
            }),
            rows: rows.into(),
            ..view(700 * S)
        });
    });
    vcx.run_until_parked();
}

/// Runs `f` on the page's table.
fn with_table<R>(
    page: &Entity<HistoryPage>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&TlTreeTable<HistoryTableDelegate>) -> R,
) -> R {
    vcx.read_entity(page, |p, cx| f(p.table().read(cx)))
}

/// Installs `TlSettings` from `settings`, with an empty state.
fn init_settings(cx: &mut TestAppContext, engine: &Arc<FakeEngine>, settings: Settings) {
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
}

/// Paints the window once.
fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

/// A filter typed on Processes before History mode starts applies to the new page's table, and
/// clearing it shows every row (tasklens-8fx.39). Nothing builds `HistoryPage` in the shell yet,
/// so making the page stands for turning History mode on.
#[gpui_kit::test]
fn filter_typed_before_history_mode_applies(cx: &mut TestAppContext) {
    use tl_model::command::Tab;
    use tl_ui::app::StartOptions;
    use tl_ui::window::filter::FILTER_DEBOUNCE;
    use tl_ui::window::main::{self as shell, main_window_view, open_main_window};
    use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
    use tl_ui_kit::components::toast;

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        let appearance = AppearanceSettings::default();
        tl_ui_base::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
        tl_ui_base::theme::metrics::init_metrics(&appearance, cx);
        tl_ui_base::i18n::init_strings(cx, "en-US");
        tl_ui_table::table::actions::bind_keys_table(cx);
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        cx.set_global(EngineGlobal(client.clone()));
        let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand));
        toast::init(cx);
        tl_ui_kit::actions::bind_keys_global(cx);
        cx.set_global(StartOptions::default());
        shell::init(cx);
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let vcx = &mut vcx;
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();

    let filter = vcx.update(|_, cx| {
        let main = main_window_view(cx).expect("main window open");
        main.read(cx).filter_box().clone()
    });
    assert_eq!(vcx.read_entity(&filter, |f, _| f.tab()), Tab::Processes);
    vcx.update(|window, cx| {
        let input = filter.read(cx).input().clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
    vcx.simulate_input("fire");
    vcx.executor().advance_clock(FILTER_DEBOUNCE * 2);
    vcx.run_until_parked();
    assert_eq!(
        vcx.read_entity(&filter, |f, _| f.text(Tab::Processes).to_string()),
        "fire"
    );

    let page =
        vcx.update(|window, cx| cx.new(|cx| HistoryPage::new(&HostContext::local(cx), window, cx)));
    vcx.run_until_parked();
    let rows = vec![prow(2, "firefox", 1.0), prow(3, "bash", 2.0)];
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);
    let shown = |vcx: &mut VisualTestContext| {
        with_table(&page, vcx, |t| {
            let d = &t.delegate;
            (0..d.row_count())
                .map(|ix| d.row_id(ix).pid)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(shown(vcx), [2], "the earlier filter applies");

    vcx.update(|window, cx| filter.update(cx, |f, cx| f.clear(window, cx)));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [3, 2], "clearing shows every row");
}

/// Row menu, jump and Delete from history (tasklens-8fx.23). The real main window gives the
/// toast queue and the Processes page; the History page sits in a second window, because
/// nothing mounts it in the shell yet (8fx.3).
mod row_actions {}

/// History mode inside the Processes tab (tasklens-8fx.3): the real main window with the fake
/// engine, no second History window.
mod mode {
    use std::cell::Cell;
    use std::rc::Rc;

    use tl_model::command::Tab;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::ProcessSnapshot;
    use tl_model::validity::FieldValidity;
    use tl_ui::app::StartOptions;
    use tl_ui::window::main::{
        self as shell, GlobalShellEvents, PauseChanged, main_window_view, open_main_window,
    };
    use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
    use tl_ui_kit::actions::{SelectTab, ToggleHistoryMode};
    use tl_ui_kit::components::menu::TlMenu;
    use tl_ui_procs::pages::processes::ProcessesPage;
    use tl_ui_procs::pages::processes::delegate::ProcRowId;
    use tl_ui_procs::pages::processes::history::actions::DeleteFromHistory;

    use super::*;

    struct Mode {
        engine: Arc<FakeEngine>,
        persisted: Rc<Cell<u32>>,
        main: VisualTestContext,
    }

    fn record(pid: u32) -> ProcessRecord {
        let name: Arc<str> = Arc::from(format!("proc{pid}"));
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: name.clone(),
                name: name.clone(),
                exe: None,
                cmdline: None,
                user: Arc::from("user"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: name,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 1.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
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

    fn open(cx: &mut TestAppContext, history_mode: bool, recording: bool) -> Mode {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let persisted = Rc::new(Cell::new(0));
        let count = Rc::clone(&persisted);
        let mut state = UiState::default();
        state.processes.history_mode = history_mode;
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui_base::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui_base::theme::metrics::init_metrics(&appearance, cx);
            tl_ui_base::i18n::init_strings(cx, "en-US");
            tl_ui_table::table::actions::bind_keys_table(cx);
            // `history.enabled` is a host setting: the engine's snapshot carries it.
            engine.publish_host_settings(|v| v.history.enabled = recording);
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state,
                persist: Some(Box::new(move |_, _| count.set(count.get() + 1))),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            cx.set_global(EngineGlobal(client.clone()));
            let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            tl_ui_kit::components::toast::init(cx);
            tl_ui_kit::actions::bind_keys_global(cx);
            cx.set_global(StartOptions::default());
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.dispatch_action(SelectTab(Tab::Processes as usize));
        main.run_until_parked();
        engine.publish_with(|s| {
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: 1,
                    ..ComponentHeader::default()
                },
                records: Arc::from(vec![record(2)]),
                ..ProcessSnapshot::default()
            });
        });
        main.run_until_parked();
        Mode {
            engine,
            persisted,
            main,
        }
    }

    impl Mode {
        fn processes(&mut self) -> Entity<ProcessesPage> {
            self.main.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .page_entity::<ProcessesPage>(Tab::Processes)
                    .expect("the Processes page is built")
            })
        }

        fn history(&mut self) -> Option<Entity<HistoryPage>> {
            let page = self.processes();
            self.main
                .read_entity(&page, |p, _| p.history_page().cloned())
        }

        fn history_mode(&mut self) -> bool {
            self.main
                .update(|_, cx| TlSettings::state(cx).processes.history_mode)
        }

        fn toggle(&mut self) {
            self.main.dispatch_action(ToggleHistoryMode);
            self.main.run_until_parked();
            draw(&mut self.main);
        }

        fn first_label(&mut self) -> String {
            self.main.update(|_, cx| {
                let main = main_window_view(cx).expect("main window");
                let strip = main.read(cx).tabstrip_view().read(cx);
                strip.drawn_labels()[0].to_string()
            })
        }

        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let main = main_window_view(cx).expect("main window");
                let queue = main.read(cx).toasts().clone();
                queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
        }

        fn set_paused(&mut self, paused: bool) {
            self.main.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| cx.emit(PauseChanged(paused)));
            });
            self.main.run_until_parked();
        }

        fn has_row(&mut self, page: &Entity<HistoryPage>, pid: u32) -> bool {
            with_table(page, &mut self.main, |t| {
                t.delegate.row_index(key(pid)).is_some()
            })
        }

        /// Answers the Frame query in flight with the view of `answer`.
        fn answer_last(&mut self) {
            let (id, _, _) = queries(&self.engine).pop().expect("a query");
            answer(&self.engine, id, &mut self.main);
        }
    }

    #[gpui_kit::test]
    fn history_toggle_persists_and_relabels_tab(cx: &mut TestAppContext) {
        let mut m = open(cx, false, true);
        assert!(m.history().is_none());
        assert!(queries(&m.engine).is_empty());
        let before = m.persisted.get();
        m.toggle();
        assert!(m.history_mode());
        assert!(m.history().is_some(), "the history page replaces the table");
        assert!(m.persisted.get() > before, "the state is saved");
        assert_eq!(m.first_label(), "History");
        let sent = queries(&m.engine);
        let (_, range, what) = sent.first().expect("the entry query");
        assert_eq!(*range, HistoryRange::Live);
        assert!(overview(what));
    }

    #[gpui_kit::test]
    fn toggle_off_restores_live_table(cx: &mut TestAppContext) {
        let mut m = open(cx, false, true);
        let page = m.processes();
        let table = m.main.read_entity(&page, |p, _| p.table().clone());
        table.update(&mut m.main, |t, cx| {
            t.selection.click(ProcRowId::Process(key(2)));
            cx.notify();
        });
        let selected = |m: &mut Mode| -> Vec<_> {
            m.main
                .read_entity(&table, |t, _| t.selection.ids().collect())
        };
        let before = selected(&mut m);
        assert_eq!(before.len(), 1);
        let offset = m.main.update(|_, cx| {
            page.read(cx)
                .scroll_handle(cx)
                .0
                .borrow()
                .base_handle
                .offset()
        });
        m.toggle();
        let weak = m.history().expect("history page").downgrade();
        m.toggle();
        assert!(!m.history_mode());
        assert!(m.history().is_none());
        assert!(weak.upgrade().is_none(), "the history page is dropped");
        assert_eq!(
            m.main.read_entity(&page, |p, _| p.table().entity_id()),
            table.entity_id()
        );
        assert_eq!(selected(&mut m), before);
        let after = m.main.update(|_, cx| {
            page.read(cx)
                .scroll_handle(cx)
                .0
                .borrow()
                .base_handle
                .offset()
        });
        assert_eq!(after, offset);
        assert_eq!(m.first_label(), "Processes");
    }

    #[gpui_kit::test]
    fn recording_off_keeps_process_table_demand(cx: &mut TestAppContext) {
        let mut m = open(cx, false, false);
        m.toggle();
        let off = |m: &mut Mode| {
            m.toasts()
                .iter()
                .filter(|t| t.as_str() == "History recording is off")
                .count()
        };
        assert_eq!(off(&mut m), 1);
        m.toggle();
        m.toggle();
        assert_eq!(off(&mut m), 1, "once per session");
        m.main.executor().advance_clock(Duration::from_millis(200));
        m.main.run_until_parked();
        let last = m.engine.demands().pop().expect("a demand");
        assert!(last.process_table);
    }

    #[gpui_kit::test]
    fn history_mode_restored_at_start(cx: &mut TestAppContext) {
        let mut m = open(cx, true, true);
        assert!(m.history().is_some());
        assert!(m.history_mode());
        let sent = queries(&m.engine);
        assert_eq!(sent[0].1, HistoryRange::Live);
    }

    #[gpui_kit::test]
    fn ctrl_pause_freezes_history_table(cx: &mut TestAppContext) {
        let mut m = open(cx, true, true);
        let page = m.history().expect("history page");
        publish_rows(
            &m.engine,
            &mut m.main,
            HistoryRange::At(600 * S),
            vec![prow(2, "live", 3.0), prow(3, "gone", 2.0)],
        );
        assert!(m.has_row(&page, 3));
        m.set_paused(true);
        publish_rows(
            &m.engine,
            &mut m.main,
            HistoryRange::At(600 * S),
            vec![prow(2, "live", 3.0), prow(4, "other", 1.0)],
        );
        assert!(m.has_row(&page, 3), "the table keeps the paused rows");
        assert!(!m.has_row(&page, 4));
        m.set_paused(false);
        assert!(!m.has_row(&page, 3), "the table follows the new view");
        assert!(m.has_row(&page, 4));
    }

    #[gpui_kit::test]
    fn scripted_session_command_order(cx: &mut TestAppContext) {
        let mut m = open(cx, false, true);
        m.toggle();
        let page = m.history().expect("history page");
        let (k1, k2) = (key(3), key(4));
        publish_rows(
            &m.engine,
            &mut m.main,
            HistoryRange::At(600 * S),
            vec![prow(3, "a", 3.0), prow(4, "b", 2.0)],
        );
        m.answer_last();
        page.update(&mut m.main, |p, cx| p.set_scrub(Some(500 * S), cx));
        m.answer_last();
        page.update(&mut m.main, |p, cx| {
            p.set_range(Some((100 * S, 400 * S)), cx)
        });
        m.answer_last();
        page.update(&mut m.main, |p, cx| p.set_metric(HistoryMetric::Memory, cx));
        m.answer_last();
        page.update(&mut m.main, |p, cx| p.set_selected(vec![k1, k2], cx));
        m.answer_last();
        // The answers carry no rows; the menu only captures rows the table shows.
        publish_rows(
            &m.engine,
            &mut m.main,
            HistoryRange::Span {
                from: 100 * S,
                to: 400 * S,
            },
            vec![prow(3, "a", 3.0), prow(4, "b", 2.0)],
        );
        let table = m.main.read_entity(&page, |p, _| p.table().clone());
        let menu = m.main.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            TlMenu::build(window, cx, focus, |menu, window, cx| {
                table.update(cx, |t, cx| {
                    t.delegate
                        .context_menu(&[k2], Some(ColumnId::Cpu), menu, window, cx)
                })
            })
        });
        let _menu = menu;
        m.main.dispatch_action(DeleteFromHistory);
        m.main.run_until_parked();
        let log: Vec<Command> = m
            .engine
            .commands()
            .into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::HistoryQuery { .. } | Command::HistoryDelete { .. }
                )
            })
            .collect();
        assert!(log.len() >= 6, "{log:?}");
        let frame = |c: &Command| match c {
            Command::HistoryQuery {
                range,
                what: HistoryWhat::Frame { metric, keys, .. },
                ..
            } => Some((*range, *metric, keys.to_vec())),
            _ => None,
        };
        let (r0, ..) = frame(&log[0]).expect("Live Frame");
        assert_eq!(r0, HistoryRange::Live);
        let (r1, ..) = frame(&log[1]).expect("At Frame");
        assert_eq!(r1, HistoryRange::At(500 * S));
        let (r2, ..) = frame(&log[2]).expect("Span Frame");
        assert!(matches!(r2, HistoryRange::Span { .. }));
        let (r3, metric3, _) = frame(&log[3]).expect("Span Frame with Memory");
        assert!(matches!(r3, HistoryRange::Span { .. }));
        assert_eq!(metric3, HistoryMetric::Memory);
        let (r4, _, keys4) = frame(&log[4]).expect("Span Frame with keys");
        assert!(matches!(r4, HistoryRange::Span { .. }));
        assert_eq!(keys4, [k1, k2]);
        match &log[5] {
            Command::HistoryDelete { keys } => assert_eq!(keys.to_vec(), [k2]),
            other => panic!("expected HistoryDelete, got {other:?}"),
        }
    }
}

/// The real Settings Processes page, rebuilt from the live settings.
fn processes_page(cx: &mut TestAppContext) -> PageBuilder {
    cx.update(|cx| all_pages(&HostContext::local(cx), cx))
        .into_iter()
        .find(|p| p.id() == PageId::Processes)
        .expect("Processes page listed")
}

/// The `[history]` host settings the window shows (tasklens-q4x76.23).
fn history_settings(cx: &mut TestAppContext) -> HistorySettings {
    cx.run_until_parked();
    cx.update(|cx| tl_ui_base::host_settings::values(&HostContext::local(cx), cx).history)
}

#[gpui_kit::test]
fn settings_history_defaults(cx: &mut TestAppContext) {
    let engine = init(cx);
    init_settings(cx, &engine, Settings::default());
    let page = processes_page(cx);
    assert_eq!(history_settings(cx), HistorySettings::default());
    let labels: Vec<String> = page
        .dropdown_labels(SettingKey::HistoryRetentionH)
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(
        labels,
        [
            "1 hour", "2 hours", "3 hours", "4 hours", "6 hours", "8 hours", "12 hours"
        ]
    );
    let h = history_settings(cx);
    assert!(!h.enabled);
    assert_eq!(h.retention_h, 6);
    assert!(h.persist);
    assert!(page.is_disabled(SettingKey::HistoryPersist));
    assert_eq!(
        page.tooltip_of(SettingKey::HistoryPersist).as_deref(),
        Some("Turn on Record history first")
    );
    assert_eq!(
        page.description_of(SettingKey::HistoryMaxDiskMb).as_deref(),
        Some("MiB")
    );
    assert_eq!(h.max_disk_mb, 256);
}

#[gpui_kit::test]
fn persist_disabled_until_recording(cx: &mut TestAppContext) {
    let engine = init(cx);
    init_settings(cx, &engine, Settings::default());
    let page = processes_page(cx);
    cx.update(|cx| page.set_bool(SettingKey::HistoryEnabled, true, cx));
    cx.run_until_parked();
    let page = processes_page(cx);
    assert!(!page.is_disabled(SettingKey::HistoryPersist));
    assert_eq!(page.tooltip_of(SettingKey::HistoryPersist), None);
}

#[gpui_kit::test]
fn retention_choice_sent_to_engine(cx: &mut TestAppContext) {
    let engine = init(cx);
    init_settings(cx, &engine, Settings::default());
    let page = processes_page(cx);
    cx.update(|cx| page.pick(SettingKey::HistoryRetentionH, "12 hours", cx));
    cx.run_until_parked();
    assert_eq!(history_settings(cx).retention_h, 12);
    let writes = engine.host_writes();
    let (base_rev, changes) = writes.last().expect("a host settings write");
    assert_eq!(*base_rev, 1);
    assert_eq!(changes.history.as_ref().map(|h| h.retention_h), Some(12));
    assert!(engine.settings().is_empty());
}

#[gpui_kit::test]
fn disk_limit_clamp_and_step(cx: &mut TestAppContext) {
    let engine = init(cx);
    init_settings(cx, &engine, Settings::default());
    let page = processes_page(cx);
    for (typed, stored) in [(5000.0, 4096), (50.0, 64), (300.0, 320)] {
        cx.update(|cx| page.set_number(SettingKey::HistoryMaxDiskMb, typed, cx));
        cx.run_until_parked();
        assert_eq!(history_settings(cx).max_disk_mb, stored, "typed {typed}");
    }
}

/// The `history-scrub` bench synth (tasklens-8fx.31): two synths from seed 42 give the same
/// answers, and the answers have the scene's shape.
#[cfg(feature = "bench")]
#[test]
fn history_scrub_synth_deterministic() {
    use tl_model::history::HISTORY_MAX_POINTS;
    use tl_ui::bench::history_scrub::{
        NEAR_VIEW_NS, PROCESSES, SPAN_END_NS, SPAN_START_NS, ScrubSynth,
    };
    use tl_ui::bench::synth::SEED;
    use tl_ui_procs::pages::processes::history::hover::top_only;

    let t = SPAN_START_NS + 3_600 * S;
    let near = (t - NEAR_VIEW_NS, t);
    let whole = (SPAN_START_NS, SPAN_END_NS);
    let frame = |view, keys: &[ProcessKey]| HistoryWhat::Frame {
        metric: HistoryMetric::Cpu,
        view,
        keys: Arc::from(keys),
        overview: true,
    };
    let answers = |synth: &mut ScrubSynth| -> Vec<HistoryView> {
        let live = synth.answer(1, HistoryRange::Live, frame((0, 0), &[]));
        let key = live.rows[5].key;
        let at = synth.answer(2, HistoryRange::At(t), frame(near, &[key]));
        let top = synth.answer(
            1 << 48,
            HistoryRange::At(t),
            HistoryWhat::TopAt {
                t: t - 30 * S,
                metric: HistoryMetric::Cpu,
                n: 3,
            },
        );
        let wide = synth.answer(3, HistoryRange::At(t + S), frame(whole, &[]));
        vec![live, at, top, wide]
    };
    let a = answers(&mut ScrubSynth::new(SEED));
    let b = answers(&mut ScrubSynth::new(SEED));
    assert_eq!(format!("{a:?}"), format!("{b:?}"));

    let [live, at, top, wide] = &a[..] else {
        panic!("four answers");
    };
    assert_eq!(live.span, Some((SPAN_START_NS, SPAN_END_NS)));
    assert_eq!(live.frame_t, Some(SPAN_END_NS));
    assert_eq!(at.frame_t, Some(t));
    assert_eq!(at.rows.len(), PROCESSES);
    assert_eq!(at.process_count, PROCESSES as u32);
    // The system total and the one selected key; 601 one-second buckets for [t - 10 min, t].
    assert_eq!(at.series.len(), 2);
    assert_eq!(at.series[0].points.len(), 601);
    assert_eq!(at.series[1].points.len(), 601);
    assert_eq!(at.stack.len(), 601);
    assert!(!at.overview.is_empty() && at.overview.len() <= HISTORY_MAX_POINTS);
    // 6 h: 22 s buckets.
    let n = wide.series[0].points.len();
    assert!((900..=HISTORY_MAX_POINTS).contains(&n), "{n}");
    assert_eq!(wide.stack.len(), n);

    // The TopAt answer is the last view with the new list.
    assert!(top_only(at, top));
    let list = top.top.as_ref().expect("top list");
    assert_eq!(list.t_ns, t - 30 * S);
    assert_eq!(list.rows.len(), 3);
    assert!(list.rows.windows(2).all(|w| w[0].value >= w[1].value));
    assert!(Arc::ptr_eq(list, wide.top.as_ref().expect("kept list")));
    let versions: Vec<u64> = a.iter().map(|v| v.header.version).collect();
    assert_eq!(versions, [1, 2, 3, 4]);

    // 200 of the 2,000 processes change from one second to the next.
    let changed = at
        .rows
        .iter()
        .zip(wide.rows.iter())
        .filter(|(x, y)| x.cpu_pct != y.cpu_pct)
        .count();
    assert!(changed <= 200, "{changed}");
    assert!(changed >= 190, "{changed}");
}

/// The `history-scrub` scene runs headless for 2 s of simulated time, with one frame every
/// 100 ms, and writes its JSON (tasklens-8fx.31).
#[cfg(feature = "bench")]
#[gpui_kit::test]
fn history_scrub_scene_runs_headless_5_frames(cx: &mut TestAppContext) {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tl_ui::bench::stats::{BenchReport, FrameBudget, no_side_start, no_side_stop};
    use tl_ui::bench::{BenchClocks, BenchError, BenchOptions, BenchScene, run};

    static FAKE_CPU: AtomicU64 = AtomicU64::new(0);
    fn fake_cpu() -> u64 {
        FAKE_CPU.fetch_add(1_000, Ordering::Relaxed)
    }

    std::fs::create_dir_all("/var/tmp/claude").expect("scratch dir");
    let dir = tempfile::tempdir_in("/var/tmp/claude").expect("temp dir");
    let json = dir.path().join("history-scrub.json");
    cx.executor().allow_parking();
    let outcome: Rc<RefCell<Option<Result<BenchReport, BenchError>>>> = Rc::default();
    let slot = Rc::clone(&outcome);
    let duration_s = 2;
    cx.update(|cx| {
        let opts = BenchOptions {
            duration_s,
            warmup_s: 0,
            json: Some(json.clone()),
            budget: FrameBudget::default(),
        };
        let clocks = BenchClocks {
            thread_cpu_ns: fake_cpu,
            process_cpu_ns: fake_cpu,
            side_start: no_side_start,
            side_stop: no_side_stop,
        };
        let task = run(cx, BenchScene::HistoryScrub, opts, clocks, None);
        cx.spawn(async move |_| *slot.borrow_mut() = Some(task.await))
            .detach();
    });
    let window = cx
        .update(|cx| cx.windows().last().copied())
        .expect("scene window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    for _ in 0..=duration_s * 10 {
        if outcome.borrow().is_some() {
            break;
        }
        // The history views come from a real thread: give it time to answer.
        std::thread::sleep(Duration::from_millis(20));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        vcx.executor().advance_clock(Duration::from_millis(100));
    }
    vcx.run_until_parked();
    let report = outcome
        .borrow_mut()
        .take()
        .expect("the scene finished")
        .expect("the scene ran");
    assert_eq!(report.scene, "history-scrub");
    assert!(report.frames >= 5, "{} frames", report.frames);
    assert_eq!(report.pacing, "off");

    let text = std::fs::read_to_string(&json).expect("the JSON was written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(v["scene"], "history-scrub");
    for key in ["p50", "p95", "p99"] {
        assert!(v["scrub_frame_ms"][key].is_number(), "{key}");
    }
    assert!(v["scrub_frames"].as_u64().is_some_and(|n| n > 0));
    assert!(v["frame_cpu_ms"]["p99"].is_number());
}
