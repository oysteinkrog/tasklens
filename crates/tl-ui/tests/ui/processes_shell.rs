//! The Processes page tests that need `MainWindow`: the shell, toolbar, jump, filter, pause, status
//! and pick tests, and the keys test that moves the focus to the shell's filter box. The other 41
//! tests of the old `processes` module run on the page host in tl-ui-procs.

use tl_model::command::Command;
use tl_model::table::TableQuery;
use tl_ui_testkit::fake_engine::FakeEngine;

fn sent(engine: &FakeEngine) -> Vec<TableQuery> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::SetTableQuery(q) => Some(q),
            _ => None,
        })
        .collect()
}

// ---- ProcessDelegate (tasklens-pdu.12) ----

/// The Processes arm of the main window's page match (tasklens-pdu.13).
mod shell {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Render, Styled as _, TestAppContext, VisualTestContext, Window, div,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_ui::actions::SelectTab;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::components::toast;
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::table::tree_table::TreeTableDelegate as _;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageFactory, PageSlot, ShellPage, main_window_view,
        open_main_window, set_test_page,
    };
    use tl_ui::window::status::StatusChanged;
    use tl_ui_settings::settings::{
        AppearanceSettings, Settings, StartTab, ThemeSettings, UiState,
    };
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{every_kind, snapshot};

    /// The Performance tab's stand-in, so there is a second tab to switch to.
    struct OtherPage {
        focus: FocusHandle,
    }

    impl gpui_kit::EventEmitter<StatusChanged> for OtherPage {}

    impl Render for OtherPage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).size_full()
        }
    }

    impl ShellPage for OtherPage {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    pub(super) fn other(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| OtherPage {
            focus: cx.focus_handle().tab_stop(true),
        });
        PageSlot::new(page, cx)
    }

    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        open_with(cx, UiState::default())
    }

    /// Opens the real main window with `state` as the saved UI state.
    pub(super) fn open_with(
        cx: &mut TestAppContext,
        state: UiState,
    ) -> (Arc<FakeEngine>, VisualTestContext) {
        open_with_perf(cx, state, other)
    }

    /// [`open_with`] with `perf` building the Performance tab.
    pub(super) fn open_with_perf(
        cx: &mut TestAppContext,
        state: UiState,
        perf: PageFactory,
    ) -> (Arc<FakeEngine>, VisualTestContext) {
        open_start(cx, state, perf, None)
    }

    /// `perf` as the Performance tab, started on it (`--tab performance`), so the Processes tab
    /// is never built.
    pub(super) fn open_on_perf(
        cx: &mut TestAppContext,
        perf: PageFactory,
    ) -> (Arc<FakeEngine>, VisualTestContext) {
        open_start(cx, UiState::default(), perf, Some(StartTab::Performance))
    }

    fn open_start(
        cx: &mut TestAppContext,
        state: UiState,
        perf: PageFactory,
        tab: Option<StartTab>,
    ) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::i18n::init_strings(cx, "en-US");
            tl_ui::table::actions::bind_keys_table(cx);
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state,
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            cx.set_global(EngineGlobal(client.clone()));
            let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
            tl_ui::action_tracker::install(client.clone(), Some(hub.clone()), cx);
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            cx.set_global(StartOptions {
                tab,
                ..StartOptions::default()
            });
            set_test_page(cx, Tab::Performance, perf);
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        (engine, vcx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn processes(vcx: &mut VisualTestContext) -> Option<Entity<ProcessesPage>> {
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        vcx.read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
    }

    pub(super) fn select(vcx: &mut VisualTestContext, tab: Tab) {
        vcx.dispatch_action(SelectTab(tab as usize));
        draw(vcx);
    }

    fn table_renders(vcx: &mut VisualTestContext, page: &Entity<ProcessesPage>) -> u32 {
        vcx.update(|_, cx| page.read(cx).table().read(cx).render_count.get())
    }

    #[gpui_kit::test]
    fn processes_page_created_on_first_visit_and_kept(cx: &mut TestAppContext) {
        let (_, mut vcx) = open(cx);
        let page = processes(&mut vcx).expect("Processes built on the first visit");
        assert!(table_renders(&mut vcx, &page) > 0, "the table is drawn");
        assert!(vcx.debug_bounds("ProcessesPage").is_some());
        assert!(vcx.debug_bounds("TlTreeTable").is_some());

        select(&mut vcx, Tab::Performance);
        assert!(vcx.debug_bounds("ProcessesPage").is_none());
        select(&mut vcx, Tab::Processes);
        let again = processes(&mut vcx).expect("still built");
        assert_eq!(again.entity_id(), page.entity_id());
        assert!(vcx.debug_bounds("ProcessesPage").is_some());
    }

    #[gpui_kit::test]
    fn inactive_page_not_rendered(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        let page = processes(&mut vcx).expect("Processes built");
        select(&mut vcx, Tab::Performance);
        let before = table_renders(&mut vcx, &page);

        // A view for the current query: the active page would redraw the table.
        let (rows, groups, records) = every_kind();
        let mut s = tl_model::snapshot::Snapshot::clone(&snapshot(2, rows, groups, records));
        s.sequence = engine.latest().sequence + 1;
        Arc::make_mut(&mut s.table).query_generation = 1;
        engine.publish(s);
        draw(&mut vcx);
        assert_eq!(table_renders(&mut vcx, &page), before);
        assert!(vcx.debug_bounds("ProcessesPage").is_none());
        let rows = vcx.update(|_, cx| page.read(cx).table().read(cx).delegate.row_count());
        assert_eq!(rows, 8, "the view was installed while hidden");
    }
}

/// The Processes toolbar buttons (tasklens-pdu.25, UI-PRC-019, UI-SHL-012, UI-SHL-013) and their
/// registration in the main window (tasklens-pdu.26).
mod toolbar {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        App, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        div, px,
    };
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::table::TableMode;
    use tl_ui::actions::{CycleViewMode, ToggleHideSystem};
    use tl_ui::components::icons::TlIcon;
    use tl_ui::i18n::tr_calls;
    use tl_ui::pages::processes::toolbar::{
        HIDE_SYSTEM_ID, ProcessesButtons, ProcessesToolbar, VIEW_MODE_ID, toolbar_words,
    };
    use tl_ui::window::toolbar::{Toolbar, ToolbarContent, ToolbarModel};
    use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings, UiState};

    use super::shell::{open_with, select};

    fn model(tab: Tab) -> ToolbarModel {
        ToolbarModel {
            tab,
            history_mode: false,
            hide_system: false,
            mode: TableMode::Grouped,
            helper: HelperState::Missing,
            helper_installed: false,
            picker_available: false,
            picking: false,
            window_active: true,
        }
    }

    /// A bare root that draws only the Toolbar with the Processes buttons, so their layout is
    /// measured without the rest of the title bar.
    struct Host {
        focus: FocusHandle,
        model: ToolbarModel,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let page = ProcessesToolbar.buttons(&self.model, window, cx);
            div()
                .track_focus(&self.focus)
                .size(px(600.))
                .flex()
                .items_start()
                .child(Toolbar::new(self.model.clone(), page))
        }
    }

    fn rig(cx: &mut TestAppContext, model: ToolbarModel) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let (view, cx) = cx.add_window_view(move |_, cx| Host {
            focus: cx.focus_handle(),
            model,
        });
        cx.update(|window, cx| {
            window.activate_window();
            view.read(cx).focus.clone().focus(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx)
    }

    /// The real main window with `state`, counting the two Actions as they reach the app. The
    /// window has no handler of its own for them, so each click arrives here.
    struct Shell {
        vcx: VisualTestContext,
        hide: Rc<Cell<usize>>,
        cycle: Rc<Cell<usize>>,
    }

    fn shell(cx: &mut TestAppContext, state: UiState) -> Shell {
        let (_, mut vcx) = open_with(cx, state);
        let hide = Rc::new(Cell::new(0));
        let cycle = Rc::new(Cell::new(0));
        vcx.update(|_, cx| {
            let (h, c) = (hide.clone(), cycle.clone());
            cx.on_action(move |_: &ToggleHideSystem, _| h.set(h.get() + 1));
            cx.on_action(move |_: &CycleViewMode, _| c.set(c.get() + 1));
        });
        Shell { vcx, hide, cycle }
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = button(vcx, selector).center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }

    /// The debug selectors of the two buttons, `TlButton-<id>`.
    const HIDE: &str = "TlButton-processes-hide-system";
    const CYCLER: &str = "TlButton-processes-view-mode";

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Option<Bounds<Pixels>> {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds(selector)
    }

    fn button(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        bounds(cx, selector).unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn buttons(cx: &mut App, model: &ToolbarModel) -> Option<ProcessesButtons> {
        let words = toolbar_words(cx);
        ProcessesButtons::for_model(model, &words)
    }

    #[gpui_kit::test]
    fn toolbar_buttons_only_on_processes_tab(cx: &mut TestAppContext) {
        assert_eq!(HIDE, format!("TlButton-{HIDE_SYSTEM_ID}"));
        assert_eq!(CYCLER, format!("TlButton-{VIEW_MODE_ID}"));
        let (view, cx) = rig(cx, model(Tab::Processes));
        let hide = button(cx, HIDE);
        let cycler = button(cx, CYCLER);
        for b in [hide, cycler] {
            assert_eq!((b.size.width, b.size.height), (px(24.), px(24.)));
        }
        assert!(
            hide.origin.x < cycler.origin.x,
            "toggle first, cycler second"
        );

        let b = cx
            .update(|_, cx| buttons(cx, &model(Tab::Processes)))
            .unwrap();
        assert!(!b.hide_on, "hide_system off draws the toggle off");
        assert_eq!(b.hide_tooltip.as_ref(), "Hide system processes");
        let on = ToolbarModel {
            hide_system: true,
            ..model(Tab::Processes)
        };
        let b = cx.update(|_, cx| buttons(cx, &on)).unwrap();
        assert!(b.hide_on);
        assert_eq!(b.hide_tooltip.as_ref(), "Show system processes");

        for tab in Tab::ALL.into_iter().filter(|t| *t != Tab::Processes) {
            let m = model(tab);
            let empty = cx.update(|window, cx| {
                buttons(cx, &m).is_none() && ProcessesToolbar.buttons(&m, window, cx).is_empty()
            });
            assert!(empty, "{tab:?}: no buttons");
        }

        cx.update(|_, cx| {
            view.update(cx, |host, cx| {
                host.model = model(Tab::Performance);
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(bounds(cx, HIDE).is_none());
        assert!(bounds(cx, CYCLER).is_none());
        let toolbar = bounds(cx, "Toolbar").expect("Toolbar drawn");
        assert_eq!(toolbar.size.width, px(0.), "no space reserved");
    }

    #[gpui_kit::test]
    fn toolbar_registered_for_processes_tab(cx: &mut TestAppContext) {
        let mut s = shell(cx, UiState::default());
        let vcx = &mut s.vcx;
        // Processes is the start tab.
        let hide = button(vcx, HIDE);
        let cycler = button(vcx, CYCLER);
        assert!(hide.origin.x < cycler.origin.x, "slot 2, then slot 3");

        for tab in Tab::ALL.into_iter().filter(|t| *t != Tab::Processes) {
            select(vcx, tab);
            assert!(bounds(vcx, HIDE).is_none(), "{tab:?}: no toggle");
            assert!(bounds(vcx, CYCLER).is_none(), "{tab:?}: no cycler");
        }

        select(vcx, Tab::Processes);
        assert_eq!(button(vcx, HIDE), hide);
        assert_eq!(button(vcx, CYCLER), cycler);
    }

    #[gpui_kit::test]
    fn cycler_icon_and_tooltip_per_mode(cx: &mut TestAppContext) {
        let (_, cx) = rig(cx, model(Tab::Processes));
        let cases = [
            (
                TableMode::Grouped,
                TlIcon::Rows2,
                "Toggle view mode (current: Task Manager Grouping)",
            ),
            (
                TableMode::Tree,
                TlIcon::ListTree,
                "Toggle view mode (current: Tree View)",
            ),
            (
                TableMode::Flat,
                TlIcon::List,
                "Toggle view mode (current: List View)",
            ),
        ];
        cx.update(|_, cx| {
            // The words are cached for this strings version: later lookups translate nothing.
            toolbar_words(cx);
            let before = tr_calls();
            for (mode, icon, tooltip) in cases {
                let m = ToolbarModel {
                    mode,
                    ..model(Tab::Processes)
                };
                let b = buttons(cx, &m).unwrap();
                assert_eq!(b.cycler_icon, icon, "{mode:?}");
                assert_eq!(b.cycler_tooltip.as_ref(), tooltip, "{mode:?}");
                assert!(!b.cycler_disabled);
            }
            assert_eq!(tr_calls(), before, "no tr call per frame");
        });
    }

    #[gpui_kit::test]
    fn cycler_disabled_in_history(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.processes.history_mode = true;
        let mut s = shell(cx, state);
        let history = ToolbarModel {
            history_mode: true,
            ..model(Tab::Processes)
        };
        let b = s.vcx.update(|_, cx| buttons(cx, &history)).unwrap();
        assert!(b.cycler_disabled);
        assert_eq!(b.cycler_tooltip.as_ref(), "History shows a flat list");
        assert_eq!(b.hide_tooltip.as_ref(), "Hide system processes");

        click(&mut s.vcx, CYCLER);
        assert_eq!(s.cycle.get(), 0, "the disabled cycler dispatches nothing");
        click(&mut s.vcx, HIDE);
        assert_eq!(s.hide.get(), 1, "the toggle stays enabled");
    }

    #[gpui_kit::test]
    fn toggle_dispatches_toggle_hide_system(cx: &mut TestAppContext) {
        let mut s = shell(cx, UiState::default());
        click(&mut s.vcx, HIDE);
        assert_eq!((s.hide.get(), s.cycle.get()), (1, 0));
        click(&mut s.vcx, CYCLER);
        assert_eq!((s.hide.get(), s.cycle.get()), (1, 1));
    }
}

/// The jump target: `ProcessesPage::select_by_key` and `select_by_pid` (tasklens-pdu.3,
/// PROC-071, UI-PRC-015).
mod jump {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::ids::{NodeId, ProcessKey};
    use tl_model::process::{GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcessRecord};
    use tl_model::snapshot::Snapshot;
    use tl_model::table::{TableMode, TableQuery, TableRow};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::actions::ProcessesShowHidden;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::jump::JumpTexts;
    use tl_ui::pages::processes::table::GroupHeader;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::window::main::{ShellPage, main_window_view};
    use tl_ui_settings::settings::UiState;
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{group, key, record, row, snapshot};
    use super::shell::{open_with, select};

    /// Fillers before the target, so centring has to scroll.
    const FILLERS: u32 = 300;
    const TARGET: u32 = 5000;
    const PARENT: u32 = 4000;
    const GRANDPARENT: u32 = 3000;

    fn firefox() -> tl_model::ids::GroupKey {
        GroupKind::App {
            app_id: Arc::from("firefox"),
        }
        .key()
    }

    /// Fillers pid 100.., then the Firefox chain grandparent > parent > target.
    fn records() -> Vec<ProcessRecord> {
        let mut out: Vec<ProcessRecord> = (0..FILLERS)
            .map(|i| record(100 + i, "filler", ProcFlags::empty()))
            .collect();
        for (pid, ppid) in [(GRANDPARENT, 1), (PARENT, GRANDPARENT), (TARGET, PARENT)] {
            let mut r = record(pid, "firefox", ProcFlags::empty());
            r.group = firefox();
            r.ppid = ppid;
            out.push(r);
        }
        out.sort_by_key(|r| r.key);
        out
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext, state: UiState) -> Rig {
        let opened = open_with(cx, state);
        rig_on(opened)
    }

    /// The rig on a main window [`open_with`] or `open_with_perf` opened.
    fn rig_on((engine, mut vcx): (Arc<FakeEngine>, VisualTestContext)) -> Rig {
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let mut r = Rig {
            engine,
            vcx,
            page,
            version: 1,
        };
        // The records the jump looks up, in a view of an old generation.
        r.publish(records(), &[], 0);
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A snapshot holding `records` whose view has `shown` (pids) as rows, for `generation`.
        fn publish(&mut self, records: Vec<ProcessRecord>, shown: &[u32], generation: u64) {
            let rows: Vec<TableRow> = shown
                .iter()
                .map(|&pid| {
                    let ix = records
                        .iter()
                        .position(|r| r.key.pid == pid)
                        .expect("shown pid is a record");
                    row(NodeId::Process(key(pid)), 0, Some(ix as u32), None)
                })
                .collect();
            self.publish_rows(records, rows, Vec::new(), generation);
        }

        /// A snapshot holding `records` whose view is `rows` and `groups`, for `generation`.
        fn publish_rows(
            &mut self,
            records: Vec<ProcessRecord>,
            rows: Vec<TableRow>,
            groups: Vec<GroupRow>,
            generation: u64,
        ) {
            self.version += 1;
            let mut s = Snapshot::clone(&snapshot(self.version, rows, groups, records));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            Arc::make_mut(&mut s.processes).header.version = self.version;
            self.engine.publish(s);
            self.draw();
        }

        /// Every filler, then the target, as the view of the last query sent.
        fn answer_with_target(&mut self) {
            let mut shown: Vec<u32> = (0..FILLERS).map(|i| 100 + i).collect();
            shown.push(TARGET);
            let generation = self.last_query().generation;
            self.publish(records(), &shown, generation);
        }

        fn last_query(&self) -> TableQuery {
            super::sent(&self.engine).pop().expect("a query was sent")
        }

        fn jump(&mut self, key: ProcessKey, texts: JumpTexts) {
            let page = self.page.clone();
            self.vcx.update(|window, cx| {
                page.update(cx, |p, cx| p.select_by_key(key, texts, window, cx))
            });
            self.draw();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        fn active_tab(&mut self) -> Tab {
            self.vcx.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window open")
                    .read(cx)
                    .active_tab()
            })
        }

        fn selected(&mut self) -> Vec<ProcRowId> {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).selection.ids().collect())
        }

        fn target_row_visible_after_scroll(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let t = page.read(cx).table().read(cx);
                let range = t.visible_range();
                range.start > 0 && range.contains(&(FILLERS as usize))
            })
        }

        fn table_focused(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx
                .update(|window, cx| page.read(cx).primary_focus().is_focused(window))
        }

        fn open_mem_group(&mut self, key: &str, name: &str) {
            let page = self.page.clone();
            let (key, name) = (Arc::<str>::from(key), name.to_string());
            self.vcx.update(|window, cx| {
                page.update(cx, |p, cx| p.open_mem_group(key, name.into(), window, cx))
            });
            self.draw();
        }

        /// The shell filter box's group chip text.
        fn chip(&mut self) -> Option<String> {
            self.vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let filter = main.read(cx).filter_box().read(cx);
                filter.group_chip().map(|t| t.to_string())
            })
        }
    }

    /// D-036, UI-PRF-060: the legend's entry point sends the memory group with the query and
    /// shows the chip; its x and Esc in the empty filter box clear both.
    #[gpui_kit::test]
    fn mem_group_chip_shows_and_clears(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        select(&mut r.vcx, Tab::Performance);
        r.open_mem_group("exe:claude", "Claude Code");
        assert_eq!(r.active_tab(), Tab::Processes);
        assert_eq!(r.last_query().mem_group.as_deref(), Some("exe:claude"));
        assert_eq!(r.chip().as_deref(), Some("Group: Claude Code"));
        assert!(r.vcx.debug_bounds("FilterBox-group-chip").is_some());
        let chip = r.vcx.debug_bounds("FilterBox-group-chip").expect("chip");
        let outline = r.vcx.debug_bounds("FilterBox-outline").expect("outline");
        assert!(chip.right() <= outline.left(), "left of the filter box");

        // The x clears the chip and the query's group.
        let x = r.vcx.debug_bounds("FilterBox-group-clear").expect("x");
        r.vcx.simulate_click(x.center(), Modifiers::none());
        r.draw();
        assert_eq!(r.last_query().mem_group, None);
        assert_eq!(r.chip(), None);
        assert!(r.vcx.debug_bounds("FilterBox-group-chip").is_none());

        // Esc in the focused empty filter box clears it too.
        r.open_mem_group("exe:claude", "Claude Code");
        assert_eq!(r.chip().as_deref(), Some("Group: Claude Code"));
        r.vcx.update(|window, cx| {
            let main = main_window_view(cx).expect("main window open");
            let input = main.read(cx).filter_box().read(cx).input().clone();
            input.update(cx, |s, cx| s.focus(window, cx));
        });
        r.draw();
        let before = super::sent(&r.engine).len();
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        assert_eq!(r.chip(), None);
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "one query");
        assert_eq!(sent.last().and_then(|q| q.mem_group.clone()), None);
    }

    /// An `app:` group in grouped mode expands the Apps header and the app row, then selects
    /// the app row once its view arrives.
    #[gpui_kit::test]
    fn mem_group_app_selects_app_row(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let mut recs = records();
        for rec in recs.iter_mut().filter(|x| x.group == firefox()) {
            Arc::make_mut(&mut rec.meta).app_id = Some(Arc::from("firefox"));
        }
        r.publish(recs.clone(), &[], 0);
        r.open_mem_group("app:firefox", "Firefox");
        let q = r.last_query();
        assert_eq!(q.mem_group.as_deref(), Some("app:firefox"));
        assert_eq!(q.mode, TableMode::Grouped);
        assert!(q.expanded.contains(&GroupHeader::Apps.node()));
        assert!(q.expanded.contains(&NodeId::Group(firefox())));
        assert!(r.selected().is_empty(), "nothing before the view");

        let target = recs.iter().position(|x| x.key.pid == TARGET).unwrap() as u32;
        let rows = vec![
            row(GroupHeader::Apps.node(), 0, None, None),
            row(NodeId::Group(firefox()), 1, None, Some(0)),
            row(NodeId::Process(key(TARGET)), 2, Some(target), None),
            row(GroupHeader::Background.node(), 0, None, None),
            row(GroupHeader::System.node(), 0, None, None),
        ];
        let groups = vec![group(
            NodeId::Group(firefox()),
            GroupRowKind::App,
            "Firefox",
            3,
        )];
        r.publish_rows(recs, rows, groups, q.generation);
        assert_eq!(r.selected(), [ProcRowId::Group(firefox())]);
        assert!(r.table_focused());
        assert!(r.toasts().is_empty(), "no toast");
    }

    /// D-036, UI-PRF-060, leader note 07:15: a click on a group's legend row, in the Overview
    /// Memory box and on the Memory page, switches to the Processes tab, sends the query with
    /// that memory group and shows its chip. "Other apps" and "Shared and kernel" open nothing.
    mod legend {
        use std::sync::Arc;

        use gpui_kit::{
            AnyView, AppContext as _, Context, EventEmitter, FocusHandle, InteractiveElement as _,
            IntoElement, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
            VisualTestContext, Window, div, px, size,
        };
        use tl_model::command::Tab;
        use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
        use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
        use tl_model::series::{Sample, SeriesView};
        use tl_model::snapshot::{ComponentHeader, SeriesSet};
        use tl_model::system::MemGroupBand;
        use tl_model::time::now_boot_ns;
        use tl_model::validity::Validity;
        use tl_ui::demand::GlobalDemand;
        use tl_ui::hub::GlobalHub;
        use tl_ui::pages::performance::memory::MemoryPage;
        use tl_ui::pages::performance::memory_apps::LEGEND_ID;
        use tl_ui::pages::performance::overview::mem_box::{APPS_MODE, MemBox};
        use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
        use tl_ui::pages::processes::ProcessesPage;
        use tl_ui::settings_global::TlSettings;
        use tl_ui::window::main::{MainWindow, PageFactory, PageSlot, ShellPage, main_window_view};
        use tl_ui::window::status::StatusChanged;
        use tl_ui_settings::settings::{MemoryView, UiState};
        use tl_ui_testkit::fake_engine::FakeEngine;

        use super::super::shell::{open_on_perf, open_with_perf, select};
        use super::rig_on;

        const MEMORY: DeviceIdx = DeviceIdx(1);
        const GROUPS: u16 = 3;
        const MIB: f32 = 1_048_576.;
        const GIB: u64 = 1 << 30;

        /// The Performance tab's stand-in: one view that draws the By app legend.
        struct Perf {
            view: AnyView,
            focus: FocusHandle,
        }

        impl EventEmitter<StatusChanged> for Perf {}

        impl Render for Perf {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .track_focus(&self.focus)
                    .size_full()
                    .child(self.view.clone())
            }
        }

        impl ShellPage for Perf {
            fn primary_focus(&self) -> FocusHandle {
                self.focus.clone()
            }
        }

        pub(super) fn slot(view: AnyView, cx: &mut Context<MainWindow>) -> PageSlot {
            let page = cx.new(|cx| Perf {
                view,
                focus: cx.focus_handle().tab_stop(true),
            });
            PageSlot::new(page, cx)
        }

        /// The Overview Memory box in "By app" mode.
        fn mem_box(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
            let hub = cx.global::<GlobalHub>().0.clone();
            let mem = cx.new(|cx| MemBox::new(&hub, MEMORY, cx));
            mem.update(cx, |b, cx| b.set_mode(APPS_MODE, cx));
            slot(mem.into(), cx)
        }

        /// The Memory page, in its By app view from the settings.
        fn memory_page(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
            let hub = cx.global::<GlobalHub>().0.clone();
            let demand = cx.global::<GlobalDemand>().0.clone();
            let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
            let row = DeviceRow {
                key: key.clone(),
                kind: DeviceKind::Memory,
                idx: MEMORY,
                generation: 0,
                net_class: None,
                fan_connected: None,
            };
            let page = cx.new(|cx| MemoryPage::new(key, row, hub, demand, cx));
            slot(page.into(), cx)
        }

        fn entry(idx: u16, identity: DeviceIdentity, name: &str) -> DeviceEntry {
            DeviceEntry {
                idx: DeviceIdx(idx),
                identity,
                generation: 0,
                display_name: Arc::from(name),
                present: true,
                parent: None,
                persistent_key: Arc::from(""),
                net_kind: None,
                hot_c: None,
            }
        }

        /// 60 one-second samples of `value` up to now.
        fn flat(device: DeviceIdx, value: f32) -> Arc<SeriesView> {
            const PERIOD: u64 = 1_000_000_000;
            let now = now_boot_ns();
            Arc::new(SeriesView {
                id: SeriesId {
                    device,
                    metric: MetricId::MEM_GROUP,
                },
                version: 1,
                period_ns: PERIOD,
                samples: (0..60u64)
                    .map(|k| Sample::valid(now - (59 - k) * PERIOD, value))
                    .collect::<Vec<_>>()
                    .into(),
            })
        }

        /// Groups p0 to p2 of 3 to 1 MiB with 2 processes each, "Other apps" 1 MiB and
        /// "Shared and kernel" 2 MiB.
        fn publish_groups(engine: &FakeEngine) {
            engine.publish_with(|s| {
                let mut entries = vec![
                    entry(0, DeviceIdentity::CpuPackage, "CPU"),
                    entry(1, DeviceIdentity::Memory, "Memory"),
                ];
                let mut views = Vec::new();
                let mut bands = Vec::new();
                let fixed = [
                    ("other", "Other apps", 1.),
                    ("shared", "Shared and kernel", 2.),
                ];
                let groups = (0..GROUPS).map(|i| {
                    let mib = f32::from(GROUPS - i);
                    (format!("exe:p{i}"), format!("p{i}"), mib)
                });
                let all = groups.chain(
                    fixed
                        .into_iter()
                        .map(|(k, n, mib)| (k.to_owned(), n.to_owned(), mib)),
                );
                for (key, name, mib) in all {
                    let idx = entries.len() as u16;
                    let identity = DeviceIdentity::MemGroup {
                        key: Arc::from(key.as_str()),
                    };
                    entries.push(entry(idx, identity, &name));
                    views.push(flat(DeviceIdx(idx), mib * MIB));
                    if key.starts_with("exe:") {
                        bands.push(MemGroupBand {
                            key: Arc::from(key),
                            name: Arc::from(name),
                            process_count: 2,
                            device: DeviceIdx(idx),
                            wine: None,
                        });
                    }
                }
                s.devices = Arc::new(DeviceCatalog {
                    version: s.devices.version + 1,
                    entries: entries.into(),
                });
                s.series = Arc::new(SeriesSet {
                    header: ComponentHeader {
                        version: s.series.header.version + 1,
                        ..ComponentHeader::default()
                    },
                    series: views.into(),
                    slow: Arc::from([]),
                });
                let sys = Arc::make_mut(&mut s.system);
                sys.header.version += 1;
                sys.header.validity = Validity::Valid;
                sys.memory.validity = Validity::Valid;
                sys.memory.total = 64 * GIB;
                sys.memory.used = 23 * GIB;
                sys.memory.available = 41 * GIB;
                sys.memory.groups = Arc::new(bands);
            });
        }

        fn click_row(vcx: &mut VisualTestContext, row: usize) {
            let id: &'static str = Box::leak(format!("{LEGEND_ID}-{row}").into_boxed_str());
            let at = vcx.debug_bounds(id).expect(id).center();
            vcx.simulate_click(at, Modifiers::none());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// The main window with `perf` as the Performance tab, on that tab with the groups shown.
        fn legend_click(cx: &mut TestAppContext, perf: PageFactory) {
            let opened = open_with_perf(cx, UiState::default(), perf);
            let mut r = rig_on(opened);
            r.vcx.update(|_, cx| {
                tl_ui::components::kv_grid::init(cx);
                TlSettings::update(cx, |d| {
                    d.settings.performance.memory_view = MemoryView::Apps
                });
            });
            r.vcx.simulate_resize(size(px(1600.), px(1200.)));
            publish_groups(&r.engine);
            select(&mut r.vcx, Tab::Performance);
            for _ in 0..3 {
                r.draw();
            }
            assert_eq!(r.active_tab(), Tab::Performance);

            // Rows: Other apps, p2, p1, p0, then Shared and kernel under the divider.
            for row in [0, 4] {
                click_row(&mut r.vcx, row);
                assert_eq!(r.active_tab(), Tab::Performance, "row {row} opens nothing");
                assert_eq!(r.chip(), None, "row {row}");
            }
            assert_eq!(r.last_query().mem_group, None);

            click_row(&mut r.vcx, 1);
            assert_eq!(r.active_tab(), Tab::Processes);
            assert_eq!(r.last_query().mem_group.as_deref(), Some("exe:p2"));
            assert_eq!(r.chip().as_deref(), Some("Group: p2"));
        }

        /// Like [`legend_click`], but started on the Performance tab: the Processes page is not
        /// built until the click selects the tab.
        fn legend_click_cold(cx: &mut TestAppContext, perf: PageFactory) {
            let (engine, mut vcx) = open_on_perf(cx, perf);
            vcx.update(|_, cx| {
                tl_ui::components::kv_grid::init(cx);
                TlSettings::update(cx, |d| {
                    d.settings.performance.memory_view = MemoryView::Apps
                });
            });
            vcx.simulate_resize(size(px(1600.), px(1200.)));
            publish_groups(&engine);
            for _ in 0..3 {
                vcx.run_until_parked();
                vcx.update(|window, cx| window.draw(cx).clear(cx));
            }
            let built = |vcx: &mut VisualTestContext| {
                vcx.update(|_, cx| {
                    let main = main_window_view(cx).expect("main window open");
                    let main = main.read(cx);
                    (
                        main.active_tab(),
                        main.page_entity::<ProcessesPage>(Tab::Processes).is_some(),
                    )
                })
            };
            assert_eq!(built(&mut vcx), (Tab::Performance, false));

            // Rows: Other apps, p2, p1, p0, then Shared and kernel under the divider.
            click_row(&mut vcx, 1);
            assert_eq!(built(&mut vcx), (Tab::Processes, true));
            let chip = vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let filter = main.read(cx).filter_box().read(cx);
                filter.group_chip().map(|t| t.to_string())
            });
            assert_eq!(chip.as_deref(), Some("Group: p2"));
            let query = super::super::sent(&engine).pop().expect("a query was sent");
            assert_eq!(query.mem_group.as_deref(), Some("exe:p2"));
        }

        #[gpui_kit::test]
        fn mem_group_legend_click_cold_on_memory_page(cx: &mut TestAppContext) {
            legend_click_cold(cx, memory_page);
        }

        #[gpui_kit::test]
        fn mem_group_legend_click_cold_in_overview_box(cx: &mut TestAppContext) {
            legend_click_cold(cx, mem_box);
        }

        #[gpui_kit::test]
        fn mem_group_legend_click_in_overview_box(cx: &mut TestAppContext) {
            legend_click(cx, mem_box);
        }

        #[gpui_kit::test]
        fn mem_group_legend_click_on_memory_page(cx: &mut TestAppContext) {
            legend_click(cx, memory_page);
        }
    }

    /// A click on a row of the Overview Top processes box (crw.103).
    mod top_procs {
        use gpui_kit::{Context, Modifiers, TestAppContext, Window, px, size};
        use tl_model::snapshot::Snapshot;
        use tl_ui::hub::GlobalHub;
        use tl_ui::pages::performance::overview::procs_box::{PROC_ROW_ID_PREFIX, ProcsBox};
        use tl_ui::window::main::{MainWindow, PageSlot};

        use super::*;

        /// The Performance tab's stand-in: the Top processes box alone.
        fn procs(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
            let hub = cx.global::<GlobalHub>().0.clone();
            let b = cx.new(|cx| ProcsBox::new(&hub, cx));
            super::legend::slot(b.into(), cx)
        }

        /// Starts on the Performance tab, publishes the records with `TARGET` busiest, then clicks
        /// its row. `cold` leaves the Processes page unbuilt until the click.
        fn click_target(cx: &mut TestAppContext, cold: bool) -> Rig {
            let (engine, mut vcx) = if cold {
                super::super::shell::open_on_perf(cx, procs)
            } else {
                let opened = super::super::shell::open_with_perf(cx, UiState::default(), procs);
                let (engine, mut vcx) = opened;
                select(&mut vcx, Tab::Performance);
                (engine, vcx)
            };
            vcx.simulate_resize(size(px(1600.), px(1200.)));
            let mut recs = records();
            for r in &mut recs {
                if r.key == key(TARGET) {
                    r.cpu_pct = 50.0;
                }
            }
            let mut s = Snapshot::clone(&snapshot(2, Vec::new(), Vec::new(), recs));
            s.sequence = engine.latest().sequence + 1;
            Arc::make_mut(&mut s.processes).header.version = 2;
            engine.publish(s);
            for _ in 0..3 {
                vcx.run_until_parked();
                vcx.update(|window, cx| window.draw(cx).clear(cx));
            }
            let state = |vcx: &mut VisualTestContext| {
                vcx.update(|_, cx| {
                    let main = main_window_view(cx).expect("main window open");
                    let main = main.read(cx);
                    (
                        main.active_tab(),
                        main.page_entity::<ProcessesPage>(Tab::Processes).is_some(),
                    )
                })
            };
            let built = !cold;
            assert_eq!(state(&mut vcx), (Tab::Performance, built));

            let id: &'static str =
                Box::leak(format!("{PROC_ROW_ID_PREFIX}{TARGET}").into_boxed_str());
            let at = vcx.debug_bounds(id).expect(id).center();
            vcx.simulate_click(at, Modifiers::none());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(state(&mut vcx), (Tab::Processes, true));

            let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
            let page = vcx
                .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
                .expect("Processes built");
            Rig {
                engine,
                vcx,
                page,
                version: 2,
            }
        }

        fn check(cx: &mut TestAppContext, cold: bool) {
            let mut r = click_target(cx, cold);
            r.answer_with_target();
            assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
            assert!(r.table_focused());
            assert_eq!(r.toasts(), ["Switched to Processes tab"]);
        }

        #[gpui_kit::test]
        fn top_procs_click_cold_selects_process(cx: &mut TestAppContext) {
            check(cx, true);
        }

        #[gpui_kit::test]
        fn top_procs_click_warm_selects_process(cx: &mut TestAppContext) {
            check(cx, false);
        }
    }

    #[gpui_kit::test]
    fn jump_expands_group_and_ancestors_then_centres(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        select(&mut r.vcx, Tab::Performance);
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        assert_eq!(r.active_tab(), Tab::Processes);
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "one query");
        let expanded = &sent.last().unwrap().expanded;
        for node in [
            GroupHeader::Apps.node(),
            NodeId::Group(firefox()),
            NodeId::Process(key(PARENT)),
            NodeId::Process(key(GRANDPARENT)),
        ] {
            assert!(expanded.contains(&node), "{node:?} expanded");
        }
        assert!(r.toasts().is_empty(), "no toast before the view");

        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
        assert!(r.target_row_visible_after_scroll(), "centred");
        assert!(r.table_focused());
        assert_eq!(r.toasts(), ["Switched to Processes tab"]);
    }

    #[gpui_kit::test]
    fn jump_tree_mode_expands_ancestors(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.processes.mode = TableMode::Tree);
        });
        r.draw();
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "one query");
        let q = sent.last().unwrap();
        assert_eq!(q.mode, TableMode::Tree);
        assert!(q.expanded.contains(&NodeId::Process(key(PARENT))));
        assert!(q.expanded.contains(&NodeId::Process(key(GRANDPARENT))));
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
        assert!(r.toasts().is_empty(), "same tab: no toast");
    }

    #[gpui_kit::test]
    fn jump_caller_text_replaces_switched_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        select(&mut r.vcx, Tab::Performance);
        r.jump(
            key(TARGET),
            JumpTexts {
                success: Some("Switched to owning process.".into()),
                ..JumpTexts::default()
            },
        );
        r.answer_with_target();
        assert_eq!(r.toasts(), ["Switched to owning process."]);
    }

    #[gpui_kit::test]
    fn jump_missing_toasts_not_found(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let mut recs = records();
        let ix = recs.iter().position(|x| x.key.pid == TARGET).unwrap();
        recs[ix].flags = ProcFlags::EXITED;
        r.publish(recs, &[], 0);
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        assert_eq!(r.toasts(), ["Process not found in current view"]);
        assert_eq!(
            super::sent(&r.engine).len(),
            before,
            "no query for an exited record"
        );

        // A key the snapshot never held.
        r.jump(key(99_999), JumpTexts::default());
        assert_eq!(super::sent(&r.engine).len(), before);
        let toasts = r.toasts();
        assert!(
            toasts
                .last()
                .is_some_and(|t| t.starts_with("Process not found in current view")),
            "{toasts:?}"
        );
    }

    #[gpui_kit::test]
    fn jump_hidden_offers_show_button(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.update(cx, |p, cx| {
                p.table()
                    .update(cx, |t, _| t.delegate.query_mut().set_filter("foo".into()));
            });
        });
        r.jump(
            key(TARGET),
            JumpTexts {
                hidden: Some("Owning process not visible in the current view.".into()),
                ..JumpTexts::default()
            },
        );
        // The filtered view lacks the target.
        let generation = r.last_query().generation;
        r.publish(records(), &[100, 101], generation);
        assert_eq!(
            r.toasts(),
            ["Owning process not visible in the current view."]
        );
        assert!(r.selected().is_empty());

        // The toast's Show button dispatches this; no element handles it, so it reaches the
        // page's app-level listener as the button's dispatch does.
        r.vcx.dispatch_action(ProcessesShowHidden);
        r.draw();
        let q = r.last_query();
        assert_eq!(q.filter.as_ref(), "", "the filter is cleared");
        assert!(q.generation > generation, "Show expands again");
        let hide = r
            .vcx
            .update(|_, cx| TlSettings::settings(cx).processes.hide_system);
        assert!(!hide);
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
    }

    #[gpui_kit::test]
    fn jump_leaves_history_mode(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.processes.history_mode = true;
        let mut r = rig(cx, state);
        r.jump(key(TARGET), JumpTexts::default());
        let history = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).processes.history_mode);
        assert!(!history);
    }

    #[gpui_kit::test]
    fn select_by_pid_resolves_identity(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        // An exited record with the same pid and an older start is never the target.
        let mut recs = records();
        let mut old = record(TARGET, "firefox", ProcFlags::EXITED);
        old.key.start_ticks = 1;
        recs.push(old);
        recs.sort_by_key(|x| x.key);
        r.publish(recs, &[], 0);
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            page.update(cx, |p, cx| {
                p.select_by_pid(TARGET, JumpTexts::default(), window, cx)
            })
        });
        r.draw();
        assert!(
            r.last_query()
                .expanded
                .contains(&NodeId::Process(key(PARENT)))
        );
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);

        r.vcx.update(|window, cx| {
            page.update(cx, |p, cx| {
                p.select_by_pid(4321, JumpTexts::default(), window, cx)
            })
        });
        r.draw();
        assert_eq!(r.toasts(), ["Process not found in current view"]);
    }
}

/// The filter box's Processes side: one query per `FilterChanged`, the include-children rule
/// and toasts, and the no-match state (tasklens-pdu.24, UI-PRC-016, UI-PRC-024).
mod filter {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::snapshot::Snapshot;
    use tl_model::table::TableMode;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::filter::include_children_visible;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::window::main::{FilterChanged, GlobalShellEvents, main_window_view};
    use tl_ui_settings::settings::UiState;
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::snapshot;
    use super::shell::open_with;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        Rig { engine, vcx, page }
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// What the shell emits after its 100 ms debounce.
        fn filter(&mut self, tab: Tab, text: &str) {
            let text = text.to_owned();
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| {
                    cx.emit(FilterChanged {
                        tab,
                        text: text.into(),
                    })
                });
            });
            self.draw();
        }

        fn generation(&mut self) -> u64 {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation())
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        /// An empty view for the current query, as a new table version.
        fn publish_empty(&mut self) {
            let generation = self.generation();
            let version = 10 + generation;
            let mut s = Snapshot::clone(&snapshot(version, Vec::new(), Vec::new(), Vec::new()));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            self.engine.publish(s);
            self.draw();
        }
    }

    #[gpui_kit::test]
    fn filter_changed_sends_new_generation(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let before = super::sent(&r.engine).len();
        let generation = r.generation();
        r.filter(Tab::Processes, "FireFox");
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "exactly one query, at once");
        let q = sent.last().expect("a query");
        assert_eq!(q.filter.as_ref(), "firefox");
        assert!(q.generation > generation, "a new generation");

        // Another tab's filter sends nothing here.
        r.filter(Tab::Services, "foo");
        assert_eq!(super::sent(&r.engine).len(), before + 1);
    }

    #[gpui_kit::test]
    fn filter_lowercased(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.filter(Tab::Processes, "FiReFox");
        let q = super::sent(&r.engine).pop().expect("a query");
        assert_eq!(q.filter.as_ref(), "firefox");
    }

    #[test]
    fn include_children_checkbox_visibility_rule() {
        assert!(include_children_visible(true, TableMode::Grouped));
        assert!(!include_children_visible(false, TableMode::Flat));
        assert!(include_children_visible(false, TableMode::Tree));
        assert!(!include_children_visible(false, TableMode::Grouped));
        // The shell's checkbox applies the same rule.
        for (text, mode) in [
            ("a", TableMode::Grouped),
            ("", TableMode::Flat),
            ("", TableMode::Tree),
        ] {
            assert_eq!(
                tl_ui::window::filter::include_children_visible(Tab::Processes, text, mode),
                include_children_visible(!text.is_empty(), mode),
            );
        }
    }

    #[gpui_kit::test]
    fn include_children_toasts(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let set = |r: &mut Rig, on: bool| {
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.processes.filter_include_children = on)
            });
            r.draw();
        };
        set(&mut r, true);
        assert_eq!(r.toasts(), ["Including filtered process children."]);
        set(&mut r, false);
        assert_eq!(
            r.toasts().last().map(String::as_str),
            Some("Hiding unmatched filtered process children.")
        );
        // Another setting changing shows no new toast.
        let count = r.toasts().len();
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.processes.mode = TableMode::Tree)
        });
        r.draw();
        assert_eq!(r.toasts().len(), count);
    }

    /// A `units` change reaches the engine as a new table query (tasklens-q4x76.22).
    #[gpui_kit::test]
    fn units_change_sends_table_query(cx: &mut TestAppContext) {
        use tl_ui_settings::settings::NetworkUnit;
        let mut r = rig(cx);
        let last = |r: &Rig| super::sent(&r.engine).pop().expect("a query was sent");
        let before = last(&r).generation;
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.units.network = NetworkUnit::Bytes)
        });
        r.draw();
        let q = last(&r);
        assert!(q.generation > before);
        assert_eq!(q.settings.network, NetworkUnit::Bytes);
    }

    #[gpui_kit::test]
    fn empty_filter_state_shows_text_and_button(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // An empty view without a filter: no no-match text.
        r.publish_empty();
        assert!(r.vcx.debug_bounds("TlTreeTable-empty").is_none());

        r.filter(Tab::Processes, "nothing-matches");
        r.publish_empty();
        let page = r.page.clone();
        let text = r.vcx.update(|_, cx| {
            use tl_ui::table::tree_table::TreeTableDelegate as _;
            page.read(cx).table().read(cx).delegate.empty_text()
        });
        assert_eq!(text.as_deref(), Some("No processes match the filter"));
        let state = r
            .vcx
            .debug_bounds("TlTreeTable-empty-state")
            .expect("the empty state is drawn");
        let body = r.vcx.debug_bounds("TlTreeTable-body").expect("the body");
        assert_eq!(
            state.origin.y - body.origin.y,
            gpui_kit::px(96.),
            "96 below the body top"
        );
        assert!(
            r.vcx.debug_bounds("TlTreeTable-empty").is_none(),
            "not the plain label"
        );
        let button = r
            .vcx
            .debug_bounds("TlButton-TlEmptyState-clear-filter")
            .expect("the Clear filter button");
        // The button dispatches ClearFilter, which the shell turns into an empty filter.
        r.vcx
            .simulate_click(button.center(), gpui_kit::Modifiers::none());
        // Past the shell's 100 ms debounce.
        r.vcx
            .executor()
            .advance_clock(std::time::Duration::from_millis(200));
        r.draw();
        let q = super::sent(&r.engine).pop().expect("a query");
        assert_eq!(q.filter.as_ref(), "", "Clear filter cleared the filter");
        assert!(
            r.vcx.debug_bounds("TlTreeTable-group-row-0").is_none(),
            "no group header row"
        );
    }
}

/// Hold Ctrl to pause (UI-PRC-017): the shell owns the rule, the page holds its snapshot.
mod pause {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::ids::NodeId;
    use tl_model::process::ProcFlags;
    use tl_model::snapshot::Snapshot;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::window::main::{ShellState, main_window_view};
    use tl_ui_settings::settings::UiState;

    use super::delegate::{key, record, row, snapshot};
    use super::shell::open_with;

    struct Rig {
        engine: Arc<tl_ui_testkit::fake_engine::FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let mut r = Rig {
            engine,
            vcx,
            page,
            version: 1,
        };
        r.publish(&[100, 101, 102]);
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A new table version with one flat process row per PID, for the current query.
        fn publish(&mut self, pids: &[u32]) {
            self.version += 1;
            let records = pids
                .iter()
                .map(|&p| record(p, &format!("proc-{p}"), ProcFlags::empty()))
                .collect();
            let rows = pids
                .iter()
                .enumerate()
                .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
                .collect();
            let page = self.page.clone();
            let generation = self
                .vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
            let mut s = Snapshot::clone(&snapshot(self.version, rows, Vec::new(), records));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            self.engine.publish(s);
            self.draw();
        }

        fn modifiers(&mut self, control: bool, shift: bool) {
            self.vcx.simulate_modifiers_change(Modifiers {
                control,
                shift,
                ..Modifiers::default()
            });
            self.draw();
        }

        fn is_paused(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx.update(|_, cx| page.read(cx).is_paused())
        }

        fn shell_paused(&mut self) -> bool {
            self.vcx.update(|_, cx| cx.global::<ShellState>().paused)
        }

        /// The PIDs of the rows the table draws.
        fn shown(&mut self) -> Vec<u32> {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let d = &page.read(cx).table().read(cx).delegate;
                let snap = d.snapshot();
                d.query()
                    .shown()
                    .map(|v| {
                        v.rows
                            .iter()
                            .filter_map(|r| r.record)
                            .map(|i| snap.processes.records[i as usize].key.pid)
                            .collect()
                    })
                    .unwrap_or_default()
            })
        }

        fn renders(&mut self) -> u32 {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).render_count.get())
        }
    }

    #[gpui_kit::test]
    fn ctrl_down_freezes_rows_and_sets_status(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        assert_eq!(r.shown(), [100, 101, 102]);
        r.modifiers(true, false);
        assert!(r.is_paused());
        assert!(r.shell_paused(), "the status chip shows the paused tag");
        r.publish(&[200, 201]);
        assert_eq!(r.shown(), [100, 101, 102], "rows frozen");

        // Ctrl up: the latest view, rendered once.
        let before = r.renders();
        r.modifiers(false, false);
        assert!(!r.is_paused());
        assert!(!r.shell_paused());
        assert_eq!(r.shown(), [200, 201]);
        assert_eq!(r.renders(), before + 1, "one render on resume");
    }

    #[gpui_kit::test]
    fn ctrl_with_other_modifier_does_not_pause(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(false, true);
        r.modifiers(true, true);
        assert!(!r.is_paused(), "Ctrl+Shift");
        r.publish(&[200]);
        assert_eq!(r.shown(), [200]);
    }

    #[gpui_kit::test]
    fn window_deactivate_resumes(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(true, false);
        r.publish(&[200]);
        assert!(r.is_paused());
        r.vcx.deactivate_window();
        r.draw();
        assert!(!r.is_paused());
        assert_eq!(r.shown(), [200]);
    }

    #[gpui_kit::test]
    fn no_render_while_paused(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(true, false);
        let before = r.renders();
        for n in 0..5 {
            r.publish(&[200 + n]);
        }
        assert_eq!(r.renders(), before, "five snapshots, no table render");
        assert!(r.is_paused());
    }
}

/// The Processes tab's status chip text (UI-PRC-018).
mod status {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::Modifiers;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::ids::{NodeId, PseudoKind};
    use tl_model::process::{GroupRowKind, ProcFlags};
    use tl_model::snapshot::Snapshot;
    use tl_model::table::TableRow;
    use tl_ui::actions::{EndHelperSession, StartHelperSession};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::window::main::{FilterChanged, GlobalShellEvents, main_window_view};
    use tl_ui::window::status::{StatusActionKind, StatusChanged};
    use tl_ui_settings::settings::UiState;

    use super::delegate::{group, key, record, row, snapshot};
    use super::shell::open_with;

    struct Rig {
        engine: Arc<tl_ui_testkit::fake_engine::FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
        emitted: Rc<Cell<u32>>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let emitted = Rc::new(Cell::new(0));
        let sink = emitted.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&page, move |_, _: &StatusChanged, _| {
                sink.set(sink.get() + 1)
            })
            .detach();
        });
        Rig {
            engine,
            vcx,
            page,
            version: 1,
            emitted,
        }
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A new table version with one process row per PID, `pseudo` pseudo rows, and the
        /// view's `total`, `shown` and `restricted`.
        fn publish(
            &mut self,
            pids: &[u32],
            pseudo: usize,
            total: u32,
            shown: u32,
            restricted: u32,
        ) {
            self.version += 1;
            let records = pids
                .iter()
                .map(|&p| record(p, &format!("proc-{p}"), ProcFlags::empty()))
                .collect();
            let mut rows: Vec<TableRow> = pids
                .iter()
                .enumerate()
                .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
                .collect();
            let kinds = [PseudoKind::Interrupts, PseudoKind::MemoryCompression];
            let groups: Vec<_> = kinds[..pseudo]
                .iter()
                .map(|&k| group(NodeId::Pseudo(k), GroupRowKind::Pseudo, "pseudo", 0))
                .collect();
            rows.extend(
                groups
                    .iter()
                    .enumerate()
                    .map(|(g, gr)| row(gr.node.clone(), 0, None, Some(g as u32))),
            );
            let page = self.page.clone();
            let generation = self
                .vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
            let mut s = Snapshot::clone(&snapshot(self.version, rows, groups, records));
            s.sequence = self.engine.latest().sequence + 1;
            s.health = Arc::clone(&self.engine.latest().health);
            let t = Arc::make_mut(&mut s.table);
            t.query_generation = generation;
            t.total = total;
            t.shown = shown;
            t.restricted = restricted;
            self.engine.publish(s);
            self.draw();
        }

        fn filter(&mut self, text: &str) {
            let text = text.to_owned();
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| {
                    cx.emit(FilterChanged {
                        tab: Tab::Processes,
                        text: text.into(),
                    })
                });
            });
            self.draw();
        }

        fn select(&mut self, pids: &[u32]) {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let table = page.read(cx).table().clone();
                table.update(cx, |t, cx| {
                    t.selection.clear();
                    for &p in pids {
                        t.selection.ctrl_click(ProcRowId::Process(key(p)));
                    }
                    cx.notify();
                });
            });
            self.draw();
        }

        fn text(&mut self) -> String {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let mut out = String::new();
                page.read(cx).status_text(&mut out);
                out
            })
        }

        /// Publishes a new helper state.
        fn helper(&mut self, state: HelperState) {
            self.engine.publish_with(|s| {
                let health = Arc::make_mut(&mut s.health);
                health.helper = state;
                health.version += 1;
            });
            self.draw();
        }

        /// The (kind, label, tooltip) of each chip part.
        fn actions(&mut self) -> Vec<(StatusActionKind, String, String)> {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                page.read(cx)
                    .status_actions()
                    .iter()
                    .map(|a| (a.kind, a.label.to_string(), a.tooltip.to_string()))
                    .collect()
            })
        }

        fn click(&mut self, selector: &'static str) {
            self.draw();
            let at = self
                .vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} drawn"))
                .center();
            self.vcx.simulate_click(at, Modifiers::none());
        }

        fn drawn(&mut self, selector: &'static str) -> bool {
            self.draw();
            self.vcx.debug_bounds(selector).is_some()
        }
    }

    /// Counts dispatches of the two helper session actions: (start, end).
    fn count_sessions(r: &mut Rig) -> (Rc<Cell<u32>>, Rc<Cell<u32>>) {
        let start = Rc::new(Cell::new(0));
        let end = Rc::new(Cell::new(0));
        let (s, e) = (start.clone(), end.clone());
        r.vcx.update(|_, cx| {
            cx.on_action(move |_: &StartHelperSession, _| s.set(s.get() + 1));
            cx.on_action(move |_: &EndHelperSession, _| e.set(e.get() + 1));
        });
        (start, end)
    }

    #[gpui_kit::test]
    fn restricted_segment_starts_helper(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (start, _) = count_sessions(&mut r);
        r.helper(HelperState::Locked);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        assert_eq!(r.text(), "296 Processes");
        assert_eq!(
            r.actions(),
            vec![(
                StatusActionKind::Link,
                "42 restricted".to_owned(),
                "Some values for other users' processes need administrator rights. Click to enable."
                    .to_owned()
            )]
        );
        r.click("StatusChip-action-0");
        assert_eq!(start.get(), 1);

        r.publish(&[1, 2, 3, 4], 0, 296, 296, 0);
        assert!(r.actions().is_empty());
        assert!(!r.drawn("StatusChip-action-0"));
    }

    #[gpui_kit::test]
    fn admin_badge_ends_helper(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (start, end) = count_sessions(&mut r);
        r.helper(HelperState::Unlocked);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        let kinds: Vec<_> = r.actions().into_iter().map(|a| (a.0, a.1)).collect();
        assert_eq!(
            kinds,
            vec![
                (StatusActionKind::Link, "42 restricted".to_owned()),
                (StatusActionKind::Badge, "Administrator mode".to_owned()),
            ]
        );
        r.click("StatusChip-action-1");
        assert_eq!(end.get(), 1);
        assert_eq!(start.get(), 0);

        r.helper(HelperState::Locked);
        assert_eq!(r.actions().len(), 1);
        assert!(!r.drawn("StatusChip-action-1"));
    }

    #[gpui_kit::test]
    fn status_text_variants(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 0);
        assert_eq!(r.text(), "296 Processes");

        r.select(&[1]);
        assert_eq!(r.text(), "296 Processes", "one selected row: no segment");
        r.select(&[1, 2, 3]);
        assert_eq!(r.text(), "296 Processes | 3 processes selected");

        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        assert_eq!(
            r.text(),
            "296 Processes | 3 processes selected | 42 restricted"
        );

        // A filter: the old counts stay until the view for it arrives.
        r.filter("proc");
        assert_eq!(
            r.text(),
            "296 Processes | 3 processes selected | 42 restricted"
        );
        // The view keeps PIDs 1 and 4; the hidden selected rows are not counted.
        r.publish(&[1, 4], 0, 296, 41, 0);
        assert_eq!(r.text(), "41 / 296 Processes");
        r.select(&[1, 2, 3, 4]);
        assert_eq!(r.text(), "41 / 296 Processes | 2 processes selected");
    }

    #[gpui_kit::test]
    fn status_count_excludes_pseudo_and_hidden_kthreads(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Three process rows and two pseudo rows; the engine left the hidden kernel threads
        // out of `total` (Hide system processes, R-280).
        r.publish(&[1, 2, 3], 2, 3, 3, 0);
        assert_eq!(r.text(), "3 Processes");
    }

    #[gpui_kit::test]
    fn status_changed_only_on_change(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.publish(&[1, 2], 0, 2, 2, 0);
        let after_first = r.emitted.get();
        assert!(after_first >= 1, "the first view emits");
        // A new version with the same counts.
        r.publish(&[2, 1], 0, 2, 2, 0);
        r.publish(&[1, 2], 0, 2, 2, 0);
        assert_eq!(r.emitted.get(), after_first, "nothing changed");
        // One selected row changes no text either.
        r.select(&[1]);
        assert_eq!(r.emitted.get(), after_first);
        r.publish(&[1, 2, 3], 0, 3, 3, 0);
        assert_eq!(r.emitted.get(), after_first + 1);
    }
}

/// The window-pick result (tasklens-pdu.48, PROC-055, UI-SHL-014, R-239). The window opens on
/// the Performance stand-in, so the shell builds the Processes page when a pick needs it.
mod pick {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{AnyWindowHandle, Entity, Modifiers, TestAppContext, VisualTestContext};
    use tl_model::actions::{ActionKind, ActionTarget, Signal};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, KWinCommand, Tab};
    use tl_model::ids::NodeId;
    use tl_model::process::{ProcFlags, ProcessRecord};
    use tl_model::snapshot::{ComponentHeader, PickOutcome, PickResult, Snapshot, WindowsSnapshot};
    use tl_model::validity::Validity;
    use tl_ui::actions::StartWindowPick;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::confirm::{ConfirmKind, texts as confirm_texts};
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window_on, set_test_page};
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{key, record, row, snapshot};

    const PID: u32 = 4242;
    const FILLER: u32 = 100;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        main: Entity<MainWindow>,
        version: u64,
        windows_version: u64,
    }

    fn records() -> Vec<ProcessRecord> {
        vec![
            record(FILLER, "filler", ProcFlags::empty()),
            record(PID, "firefox", ProcFlags::empty()),
        ]
    }

    /// The real app with its action tracker, on the Performance stand-in, with a KWin bridge,
    /// the records of [`records`] and a pick started.
    fn rig(cx: &mut TestAppContext) -> Rig {
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
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
            set_test_page(cx, Tab::Performance, super::shell::other);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Performance, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let mut r = Rig {
            engine,
            vcx,
            main,
            version: 1,
            windows_version: 0,
        };
        r.publish(&[], 0, None);
        r.vcx.dispatch_action(StartWindowPick);
        r.draw();
        assert!(r.picking(), "the pick started");
        assert!(r.page().is_none(), "Processes not built yet");
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A snapshot with [`records`], a view with `shown` (pids) as rows for `generation`,
        /// and a valid windows component holding `pick`.
        fn publish(&mut self, shown: &[u32], generation: u64, pick: Option<PickResult>) {
            let recs = records();
            let rows = shown
                .iter()
                .map(|&pid| {
                    let ix = recs
                        .iter()
                        .position(|r| r.key.pid == pid)
                        .expect("shown pid is a record");
                    row(NodeId::Process(key(pid)), 0, Some(ix as u32), None)
                })
                .collect();
            self.version += 1;
            self.windows_version += 1;
            let mut s = Snapshot::clone(&snapshot(self.version, rows, Vec::new(), recs));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            Arc::make_mut(&mut s.processes).header.version = self.version;
            s.windows = Arc::new(WindowsSnapshot {
                header: ComponentHeader {
                    version: self.windows_version,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                windows: Arc::from([]),
                pick,
            });
            self.engine.publish(s);
            self.draw();
        }

        fn result(&mut self, seq: u64, outcome: PickOutcome) {
            self.publish(&[], 0, Some(PickResult { seq, outcome }));
        }

        fn picked(&mut self, pid: u32) {
            self.result(1, PickOutcome::Picked { pid });
        }

        fn picking(&mut self) -> bool {
            let main = self.main.clone();
            self.vcx.update(|_, cx| main.read(cx).is_picking())
        }

        fn page(&mut self) -> Option<Entity<ProcessesPage>> {
            let main = self.main.clone();
            self.vcx
                .update(|_, cx| main.read(cx).page_entity::<ProcessesPage>(Tab::Processes))
        }

        fn active_tab(&mut self) -> Tab {
            let main = self.main.clone();
            self.vcx.update(|_, cx| main.read(cx).active_tab())
        }

        fn toasts(&mut self) -> Vec<String> {
            let main = self.main.clone();
            self.vcx.update(|_, cx| {
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
        }

        /// The open confirmation windows.
        fn dialogs(&mut self) -> Vec<(AnyWindowHandle, Entity<TlDialogView>)> {
            self.vcx.update(|_, cx| {
                cx.windows()
                    .into_iter()
                    .filter_map(|w| {
                        let handle = w.downcast::<Root>()?;
                        let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                        Some((w, frame.read(cx).content().clone()))
                    })
                    .collect()
            })
        }

        /// The one open confirmation, drawn.
        fn dialog(&mut self) -> (VisualTestContext, Entity<TlDialogView>) {
            let mut open = self.dialogs();
            assert_eq!(open.len(), 1, "one confirmation");
            let (any, view) = open.pop().expect("a confirmation");
            let mut vcx = VisualTestContext::from_window(any, &self.vcx.cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            (vcx, view)
        }

        /// Clicks the open confirmation's button `selector`.
        fn choose(&mut self, selector: &'static str) {
            let (mut vcx, _) = self.dialog();
            let at = vcx.debug_bounds(selector).expect("dialog button").center();
            vcx.simulate_click(at, Modifiers::none());
            vcx.run_until_parked();
            self.draw();
        }

        /// The action requests sent: kind and target PIDs.
        fn actions(&self) -> Vec<(ActionKind, Vec<u32>)> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .map(|r| {
                    let pids = r
                        .targets
                        .iter()
                        .filter_map(|t| match t {
                            ActionTarget::Process(k) => Some(k.pid),
                            _ => None,
                        })
                        .collect();
                    (r.kind, pids)
                })
                .collect()
        }

        /// Answers the page's last query with a view that shows the filler and the target.
        fn answer(&mut self) {
            let generation = super::sent(&self.engine)
                .pop()
                .expect("a query was sent")
                .generation;
            self.publish(&[FILLER, PID], generation, None);
        }

        fn selected(&mut self) -> Vec<ProcRowId> {
            let page = self.page().expect("Processes built");
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).selection.ids().collect())
        }

        fn target_visible(&mut self) -> bool {
            let page = self.page().expect("Processes built");
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).visible_range().contains(&1))
        }
    }

    #[gpui_kit::test]
    fn pick_result_opens_picked_dialog_when_inactive(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        assert!(!r.picking(), "end_window_pick ran");
        assert!(r.toasts().is_empty(), "the start toast went");
        let (mut vcx, view) = r.dialog();
        let heading = vcx.update(|_, cx| {
            let kind = ConfirmKind::Picked {
                name: "firefox".into(),
                pid: PID,
            };
            confirm_texts(&kind, cx).title.to_string()
        });
        assert_eq!(heading, "Picked firefox (PID 4242)");
        assert!(vcx.debug_bounds("TlDialogHeading").is_some());
        let (text, focused) = vcx.update(|window, cx| {
            let v = view.read(cx);
            (v.dialog().text().to_string(), v.focused_button(window))
        });
        assert_eq!(text, "What do you want to do with this process?");
        assert_eq!(focused, Some(DialogButton::Action), "Select is the default");
        assert!(vcx.debug_bounds("TlDialogSecond").is_some(), "Terminate");
        assert!(r.actions().is_empty());
    }

    #[gpui_kit::test]
    fn pick_select_jumps_to_row(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        r.choose("TlDialogAction");
        assert!(r.dialogs().is_empty());
        assert_eq!(r.active_tab(), Tab::Processes);
        r.answer();
        assert_eq!(r.selected(), [ProcRowId::Process(key(PID))]);
        assert!(r.target_visible());
        assert!(r.actions().is_empty(), "Select sends no action");
    }

    #[gpui_kit::test]
    fn pick_terminate_sends_end_task(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        r.choose("TlDialogSecond");
        assert!(
            r.dialogs().is_empty(),
            "an own process needs no second confirmation"
        );
        assert_eq!(r.actions(), [(ActionKind::Signal(Signal::Term), vec![PID])]);
    }

    #[gpui_kit::test]
    fn pick_ctrl_active_ends_task(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.simulate_modifiers_change(Modifiers::control());
        r.draw();
        r.picked(PID);
        assert!(!r.picking());
        // Leader decision Q2: Ctrl+click asks with the Picked dialog too (PROC-055).
        assert!(r.actions().is_empty(), "nothing before the confirmation");
        r.choose("TlDialogSecond");
        assert_eq!(r.actions(), [(ActionKind::Signal(Signal::Term), vec![PID])]);
    }

    #[gpui_kit::test]
    fn pick_plain_active_selects(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.picked(PID);
        assert!(!r.picking());
        assert!(r.dialogs().is_empty(), "no dialog");
        assert_eq!(r.active_tab(), Tab::Processes);
        r.answer();
        assert_eq!(r.selected(), [ProcRowId::Process(key(PID))]);
        assert!(r.target_visible());
        assert!(r.actions().is_empty());
    }

    #[gpui_kit::test]
    fn pick_no_window_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.result(1, PickOutcome::NoWindow);
        assert!(!r.picking());
        assert_eq!(r.toasts(), ["No window found"]);
        assert_eq!(r.active_tab(), Tab::Performance, "no tab switch");
        assert!(r.dialogs().is_empty());
    }

    #[gpui_kit::test]
    fn pick_cancelled_toast_once(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Esc cancels first and shows the toast; the engine's Cancelled reply replaces it.
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        let cancels = r
            .engine
            .commands()
            .into_iter()
            .filter(|c| matches!(c, Command::KWin(KWinCommand::CancelPick { seq: 1 })))
            .count();
        assert_eq!(cancels, 1);
        assert_eq!(r.toasts(), ["No window selected"]);
        r.result(1, PickOutcome::Cancelled);
        assert_eq!(r.toasts(), ["No window selected"]);
        assert!(!r.picking());
    }

    #[gpui_kit::test]
    fn pick_late_cancel_keeps_new_pick(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Esc cancels pick 1, then pick 2 starts before the engine answers pick 1.
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        assert!(!r.picking());
        r.vcx.dispatch_action(StartWindowPick);
        r.draw();
        assert!(r.picking(), "pick 2 started");
        let start = "Click a window to select its process. Hold CTRL while clicking to terminate \
                     process. Esc cancels.";
        assert_eq!(r.toasts(), [start]);

        // The late Cancelled for pick 1 leaves pick 2, its toast and Esc alone.
        r.result(1, PickOutcome::Cancelled);
        assert!(r.picking(), "pick 2 still runs");
        assert_eq!(r.toasts(), [start]);
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        assert!(!r.picking(), "Esc still cancels pick 2");
        let cancels: Vec<u64> = r
            .engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::KWin(KWinCommand::CancelPick { seq }) => Some(seq),
                _ => None,
            })
            .collect();
        assert_eq!(cancels, [1, 2]);
        assert_eq!(r.toasts(), ["No window selected"]);
    }

    #[gpui_kit::test]
    fn pick_unknown_process_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.picked(99_999);
        assert!(!r.picking());
        assert_eq!(r.toasts(), ["Unknown process"]);
        assert!(r.dialogs().is_empty());
        assert_eq!(r.active_tab(), Tab::Performance);
    }

    #[gpui_kit::test]
    fn pick_same_seq_ignored(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        let (mut vcx, _) = r.dialog();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        r.draw();
        assert!(r.dialogs().is_empty(), "Esc cancels the dialog");

        // A later snapshot still carries seq 1.
        r.picked(PID);
        assert!(r.dialogs().is_empty(), "handled once");
        r.result(1, PickOutcome::NoWindow);
        assert!(
            r.toasts().is_empty(),
            "same seq, new outcome: still ignored"
        );
        assert!(r.actions().is_empty());
    }
}

mod delegate {

    use std::sync::Arc;


    use tl_model::ids::{GroupKey, NodeId, ProcessKey, PseudoKind};
    use tl_model::process::{
        GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord,
    };
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
    use tl_model::table::{Manager, ProcessTableView, TableRow, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    use tl_ui::pages::processes::table::GroupHeader;

    pub(super) fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(0),
            pid,
        }
    }

    pub(super) fn record(pid: u32, name: &str, flags: ProcFlags) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a(name),
                name: a(name),
                exe: None,
                cmdline: None,
                user: a("oystein"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a(name),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags,
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

    pub(super) fn group(node: NodeId, kind: GroupRowKind, name: &str, members: u32) -> GroupRow {
        GroupRow {
            node,
            key: GroupKey(7),
            kind,
            class: GroupKind::System,
            name: Arc::from(name),
            icon: None,
            members,
            pids: members,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
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
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    pub(super) fn row(
        node: NodeId,
        depth: u8,
        record: Option<u32>,
        group: Option<u32>,
    ) -> TableRow {
        TableRow {
            node,
            depth,
            expanded: false,
            has_children: false,
            record,
            group,
            descendants: 0,
            subtree: None,
        }
    }

    pub(super) fn firefox() -> NodeId {
        NodeId::Group(
            GroupKind::App {
                app_id: Arc::from("firefox"),
            }
            .key(),
        )
    }

    fn unit() -> NodeId {
        NodeId::Unit(UnitRef {
            manager: Manager::System,
            name: Arc::from("foo.service"),
        })
    }

    /// One row of every kind, in the order the acceptance test names them, then the Kernel
    /// threads (N) row and an expanded parent.
    pub(super) fn every_kind() -> (Vec<TableRow>, Vec<GroupRow>, Vec<ProcessRecord>) {
        let records = vec![
            record(10, "bash", ProcFlags::empty()),
            record(20, "kworker/0:1", ProcFlags::KERNEL_THREAD),
            record(30, "sshd", ProcFlags::empty()),
        ];
        let groups = vec![
            group(firefox(), GroupRowKind::App, "Firefox", 14),
            group(unit(), GroupRowKind::Unit, "Foo daemon", 2),
            group(
                NodeId::Pseudo(PseudoKind::Interrupts),
                GroupRowKind::Pseudo,
                "Interrupts",
                0,
            ),
            group(
                NodeId::KernelThreads,
                GroupRowKind::KernelThreads,
                "Kernel threads",
                5,
            ),
        ];
        let rows = vec![
            TableRow {
                expanded: true,
                has_children: true,
                descendants: 20,
                ..row(GroupHeader::Apps.node(), 0, None, None)
            },
            TableRow {
                has_children: true,
                ..row(firefox(), 1, None, Some(0))
            },
            row(unit(), 1, None, Some(1)),
            TableRow {
                has_children: true,
                descendants: 3,
                ..row(NodeId::Process(key(10)), 1, Some(0), None)
            },
            row(NodeId::Process(key(20)), 1, Some(1), None),
            row(NodeId::Pseudo(PseudoKind::Interrupts), 1, None, Some(2)),
            row(NodeId::KernelThreads, 1, None, Some(3)),
            TableRow {
                has_children: true,
                expanded: true,
                descendants: 3,
                ..row(NodeId::Process(key(30)), 1, Some(2), None)
            },
        ];
        (rows, groups, records)
    }

    pub(super) fn snapshot(
        version: u64,
        rows: Vec<TableRow>,
        groups: Vec<GroupRow>,
        records: Vec<ProcessRecord>,
    ) -> Arc<Snapshot> {
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            groups: groups.into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: records.into(),
            ..ProcessSnapshot::default()
        });
        Arc::new(s)
    }
}

/// The ProcessTable keys for End Task and End Process (PROC-049, pdu.29).
mod keys {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use tl_model::actions::{ActionKind, ActionTarget};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::process::ProcFlags;
    use tl_model::snapshot::Snapshot;
    use tl_ui::actions::FocusFilter;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::i18n::init_strings;

    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::{ProcessesPage, bind_keys_processes};
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{key, record, row, snapshot};

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
    }

    /// The real app on the Processes tab, with its action tracker (the End handlers send
    /// through it).
    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
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
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        (engine, vcx)
    }

    /// The main window with PIDs 300 and 301 (own, unprotected) and PID 300 selected.
    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open(cx);
        vcx.update(|_, cx| bind_keys_processes(cx));
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let mut r = Rig { engine, vcx, page };
        let pids = [300, 301];
        let records = pids
            .iter()
            .map(|&p| record(p, "proc", ProcFlags::empty()))
            .collect();
        let rows = pids
            .iter()
            .enumerate()
            .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
            .collect();
        let page = r.page.clone();
        let generation = r
            .vcx
            .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
        let mut s = Snapshot::clone(&snapshot(2, rows, Vec::new(), records));
        s.sequence = r.engine.latest().sequence + 1;
        Arc::make_mut(&mut s.table).query_generation = generation;
        r.engine.publish(s);
        r.draw();
        r.vcx.update(|window, cx| {
            let table = page.read(cx).table().clone();
            table.update(cx, |t, cx| {
                t.selection.click(ProcRowId::Process(key(300)));
                cx.notify();
            });
            page.update(cx, |p, cx| p.focus_table(window, cx));
        });
        r.draw();
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn press(&mut self, keys: &str) {
            self.vcx.simulate_keystrokes(keys);
            self.draw();
        }

        /// The action requests sent: kind and target PIDs.
        fn sent(&self) -> Vec<(ActionKind, Vec<u32>)> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .map(|r| {
                    let pids = r
                        .targets
                        .iter()
                        .filter_map(|t| match t {
                            ActionTarget::Process(k) => Some(k.pid),
                            _ => None,
                        })
                        .collect();
                    (r.kind, pids)
                })
                .collect()
        }
    }

    #[gpui_kit::test]
    fn keys_ignored_when_input_focused(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.dispatch_action(FocusFilter);
        r.draw();
        r.press("delete");
        r.press("shift-delete");
        assert!(r.sent().is_empty(), "no action reached the table");
    }
}
