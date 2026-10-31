//! Startup Apps delegate tests (tasklens-9o0.6, UI-SYS-041, UI-SYS-042).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{App, TestAppContext, px};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::{
    ComponentHeader, Impact, ImpactDetail, Snapshot, StartupEntry, StartupKind, StartupScope,
    StartupSnapshot, StartupStatus,
};
use tl_model::table::{StartupColumn, StartupRef, StartupSource, SysColumnId, SysNodeId, SysRow};
use tl_model::time::BootId;
use tl_model::validity::Validity;
use tl_ui::pages::startup::delegate::{
    STARTUP_COLUMNS, StartupDelegate, StartupWords, default_width, id,
    impact_tooltip_lines as tooltip_lines, sys_status,
};
use tl_ui::pages::sys_status::{DotStyle, LabelTone, cell_colors, status_look};
use tl_ui::pages::sys_table::SysDelegate as _;
use tl_ui::table::cells::CellOut;
use tl_ui::table::columns::TableColumn as _;
use tl_ui::table::tree_table::{NameIcon, TreeTableDelegate as _};
use tl_ui::theme::{TlTheme, current};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
}

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

/// A delegate with `entries` installed as rows 0.., `HOME=/home/u`, and the theme.
fn delegate(
    cx: &mut TestAppContext,
    entries: Vec<StartupEntry>,
) -> (StartupDelegate, Arc<TlTheme>) {
    cx.update(|cx| {
        setup(cx);
        let mut d = StartupDelegate::new(Arc::from("/home/u"), cx);
        let rows: Vec<SysRow> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| SysRow {
                node: SysNodeId::Startup(e.id.clone()),
                depth: 0,
                expanded: false,
                has_children: false,
                record: Some(i as u32),
            })
            .collect();
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.startup = Arc::new(StartupSnapshot {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            query_generation: 1,
            entries: entries.into(),
            ..StartupSnapshot::default()
        });
        d.install(&Arc::new(snap));
        (d, current(cx).clone())
    })
}

fn cell(d: &StartupDelegate, ix: usize, c: StartupColumn) -> CellOut {
    let mut out = CellOut {
        want_tooltip: true,
        ..CellOut::default()
    };
    d.cell(ix, id(c), &mut out);
    out
}

fn text(out: &CellOut) -> &str {
    out.arc.as_deref().unwrap_or("")
}

#[gpui_kit::test]
fn startup_columns_widths(cx: &mut TestAppContext) {
    let (d, _) = delegate(cx, Vec::new());
    let ids: Vec<SysColumnId> = STARTUP_COLUMNS.map(id).to_vec();
    assert_eq!(d.columns().order, ids);
    let expect = [
        (StartupColumn::Name, 220., 120.),
        (StartupColumn::Publisher, 180., 60.),
        (StartupColumn::Status, 190., 80.),
        (StartupColumn::Impact, 120., 80.),
        (StartupColumn::Type, 190., 60.),
        (StartupColumn::Location, 260., 60.),
        (StartupColumn::Command, 170., 60.),
    ];
    for (c, width, min) in expect {
        assert_eq!(default_width(c), width, "{c:?}");
        assert_eq!(d.column_spec(id(c)).default_width, px(width), "{c:?}");
        assert_eq!(id(c).min_width(), Some(min), "{c:?}");
        assert_eq!(d.column_spec(id(c)).hideable, c != StartupColumn::Name);
    }
    let labels: Vec<&str> = STARTUP_COLUMNS
        .iter()
        .map(|&c| d.column_spec(id(c)).label.as_ref())
        .collect();
    assert_eq!(
        labels,
        [
            "Name",
            "Publisher",
            "Status",
            "Startup impact",
            "Type",
            "Location",
            "Command"
        ]
    );
    assert_eq!(d.sort().column, id(StartupColumn::Status));
    assert!(d.sort().ascending);
}

#[gpui_kit::test]
fn startup_row_fallback_icons(cx: &mut TestAppContext) {
    let mut with_icon = entry("d", StartupKind::AutostartUser);
    with_icon.icon_path = Some(Arc::from("/usr/share/icons/d.png"));
    let (d, _) = delegate(
        cx,
        vec![
            entry("a", StartupKind::AutostartSystem),
            entry("b", StartupKind::UserUnit),
            entry("c", StartupKind::PlasmaPreStartup),
            with_icon,
        ],
    );
    let icon = |ix| d.name_cell(ix).icon;
    assert!(matches!(icon(0), NameIcon::Lucide(IconName::AppWindow)));
    assert!(matches!(icon(1), NameIcon::Lucide(IconName::Cog)));
    assert!(matches!(icon(2), NameIcon::Lucide(IconName::FileCode)));
    assert!(matches!(icon(3), NameIcon::File(p) if &*p == "/usr/share/icons/d.png"));
    assert_eq!(d.name_cell(0).primary.as_ref(), "a");
}

#[gpui_kit::test]
fn startup_overridden_row_secondary(cx: &mut TestAppContext) {
    let mut e = entry("a", StartupKind::AutostartSystem);
    e.overridden_by_user = true;
    e.impact = Impact::High;
    let (d, theme) = delegate(cx, vec![e, entry("b", StartupKind::AutostartUser)]);
    for c in STARTUP_COLUMNS {
        assert_eq!(
            cell(&d, 0, c).text_color,
            Some(theme.text_secondary),
            "{c:?}"
        );
    }
    assert_eq!(cell(&d, 1, StartupColumn::Command).text_color, None);
    assert_eq!(cell(&d, 1, StartupColumn::Name).text_color, None);
}

#[gpui_kit::test]
fn startup_status_cell_looks(cx: &mut TestAppContext) {
    use LabelTone::{Primary, Secondary, Success, Warning};
    let cases = [
        (
            StartupStatus::Enabled,
            DotStyle::Filled(Success),
            Primary,
            "Enabled",
        ),
        (
            StartupStatus::Disabled,
            DotStyle::Ring(Secondary),
            Secondary,
            "Disabled",
        ),
        (
            StartupStatus::Masked,
            DotStyle::Filled(Warning),
            Primary,
            "Masked",
        ),
        (
            StartupStatus::NotInThisDesktop,
            DotStyle::Ring(Secondary),
            Secondary,
            "Not in this desktop",
        ),
        (
            StartupStatus::MissingProgram,
            DotStyle::Filled(Warning),
            Primary,
            "Missing program",
        ),
    ];
    let entries = cases
        .iter()
        .map(|(s, ..)| {
            let mut e = entry(&format!("{s:?}"), StartupKind::AutostartUser);
            e.status = *s;
            e
        })
        .collect();
    let (d, theme) = delegate(cx, entries);
    for (ix, (s, dot, label, word)) in cases.into_iter().enumerate() {
        let look = status_look(sys_status(s));
        assert_eq!(look.dot, dot, "{s:?}");
        assert_eq!(look.label, label, "{s:?}");
        let out = cell(&d, ix, StartupColumn::Status);
        assert_eq!(text(&out), word);
        let secondary = (label == Secondary).then_some(theme.text_secondary);
        assert_eq!(out.text_color, secondary, "{s:?}");
        assert_eq!(
            out.dot,
            Some(dot),
            "{s:?}: the cell draws the UI-SYS-003 dot"
        );
    }
    let enabled = cell_colors(
        status_look(sys_status(StartupStatus::Enabled)),
        false,
        false,
        &theme,
    );
    assert_eq!(enabled.dot, Some(theme.status.success));
}

#[gpui_kit::test]
fn impact_colors(cx: &mut TestAppContext) {
    let impacts = [
        Impact::High,
        Impact::Medium,
        Impact::Low,
        Impact::None,
        Impact::NotMeasured,
    ];
    let entries = impacts
        .iter()
        .map(|i| {
            let mut e = entry(&format!("{i:?}"), StartupKind::UserUnit);
            e.impact = *i;
            e
        })
        .collect();
    let (d, theme) = delegate(cx, entries);
    let expect = [
        (Some(theme.status.warning), "High"),
        (None, "Medium"),
        (None, "Low"),
        (Some(theme.text_secondary), "None"),
        (Some(theme.text_secondary), "Not measured"),
    ];
    for (ix, (color, word)) in expect.into_iter().enumerate() {
        let out = cell(&d, ix, StartupColumn::Impact);
        assert_eq!(out.text_color, color, "{word}");
        assert_eq!(text(&out), word);
    }
}

#[test]
fn impact_tooltip_lines() {
    let w = StartupWords::en();
    let detail = ImpactDetail {
        activation_ms: 412,
        started_after_login_ms: 3200,
        cpu_nsec: Some(1_000_000_000),
        fork_exec_only: false,
    };
    let three = [
        "Activation time: 412 ms",
        "Started 3.2 s after login",
        "CPU time: 0:00:01",
    ];
    let lines = tooltip_lines(Impact::High, Some(&detail), false, &w);
    assert_eq!(lines.as_slice(), three);
    let lines = tooltip_lines(Impact::High, Some(&detail), true, &w);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[..3], three);
    assert_eq!(
        lines[3],
        "For this unit type only the time to start the program is measured."
    );
    assert_eq!(
        tooltip_lines(Impact::None, None, false, &w).as_slice(),
        ["Disabled entries do not run at login."]
    );
    assert_eq!(
        tooltip_lines(Impact::NotMeasured, None, false, &w).as_slice(),
        ["This entry did not start through systemd in this session."]
    );
    assert!(tooltip_lines(Impact::Low, None, false, &w).is_empty());
}

#[gpui_kit::test]
fn impact_tooltip_on_hovered_cell(cx: &mut TestAppContext) {
    let mut e = entry("a", StartupKind::UserUnit);
    e.impact = Impact::Medium;
    e.impact_detail = Some(ImpactDetail {
        activation_ms: 412,
        started_after_login_ms: 3200,
        cpu_nsec: Some(1_000_000_000),
        fork_exec_only: false,
    });
    let (d, _) = delegate(cx, vec![e]);
    let out = cell(&d, 0, StartupColumn::Impact);
    assert_eq!(
        out.tooltip.as_deref(),
        Some("Activation time: 412 ms\nStarted 3.2 s after login\nCPU time: 0:00:01")
    );
    let mut quiet = CellOut::default();
    d.cell(0, id(StartupColumn::Impact), &mut quiet);
    assert_eq!(quiet.tooltip, None, "no tooltip off the hovered row");
}

#[gpui_kit::test]
fn impact_tooltip_fork_exec_line(cx: &mut TestAppContext) {
    let mut e = entry("a", StartupKind::UserUnit);
    e.impact = Impact::Medium;
    e.impact_detail = Some(ImpactDetail {
        activation_ms: 412,
        started_after_login_ms: 3200,
        cpu_nsec: Some(1_000_000_000),
        fork_exec_only: true,
    });
    let (d, _) = delegate(cx, vec![e]);
    let out = cell(&d, 0, StartupColumn::Impact);
    assert_eq!(
        out.tooltip.as_deref(),
        Some(
            "Activation time: 412 ms\nStarted 3.2 s after login\nCPU time: 0:00:01\nFor this unit type only the time to start the program is measured."
        )
    );
}

#[gpui_kit::test]
fn startup_location_home_tilde(cx: &mut TestAppContext) {
    let mut system = entry("b", StartupKind::AutostartSystem);
    system.location = Arc::from("/etc/xdg/autostart");
    let (d, theme) = delegate(cx, vec![entry("a", StartupKind::AutostartUser), system]);
    let out = cell(&d, 0, StartupColumn::Location);
    assert_eq!(text(&out), "~/.config/autostart");
    assert_eq!(out.text_color, Some(theme.text_secondary));
    assert_eq!(
        text(&cell(&d, 1, StartupColumn::Location)),
        "/etc/xdg/autostart"
    );
}

#[gpui_kit::test]
fn startup_name_tooltip_two_lines(cx: &mut TestAppContext) {
    let (d, _) = delegate(cx, vec![entry("a", StartupKind::AutostartUser)]);
    let lines = d.name_tooltip_lines(0).expect("row 0");
    assert_eq!(lines[0].as_ref(), "/usr/bin/app --tray");
    assert_eq!(lines[1].as_ref(), "/home/u/.config/autostart/app.desktop");
    assert_eq!(
        d.accessible_name(0).as_deref(),
        Some("a, Enabled, Autostart (user)")
    );
    assert!(d.name_tooltip_lines(1).is_none());
}

#[gpui_kit::test]
fn startup_publisher_user_secondary(cx: &mut TestAppContext) {
    let mut user = entry("a", StartupKind::UserUnit);
    user.publisher = Some(Arc::from("User"));
    let mut blank = entry("b", StartupKind::UserUnit);
    blank.publisher = None;
    let (d, theme) = delegate(
        cx,
        vec![user, blank, entry("c", StartupKind::AutostartUser)],
    );
    let out = cell(&d, 0, StartupColumn::Publisher);
    assert_eq!(text(&out), "User");
    assert_eq!(out.text_color, Some(theme.text_secondary));
    assert_eq!(text(&cell(&d, 1, StartupColumn::Publisher)), "");
    let out = cell(&d, 2, StartupColumn::Publisher);
    assert_eq!(text(&out), "app 1.0");
    assert_eq!(out.text_color, None);
}

/// The StartupPage entity (tasklens-9o0.7, UI-SYS-040, UI-SYS-049).
#[cfg(feature = "test-hooks")]
mod page {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use gpui_kit::Keystroke;
    use gpui_kit::base::Root;
    use gpui_kit::component::kbd::Kbd;
    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
        MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
        StyleRefinement, Styled as _, VisualTestContext, Window, WindowBounds, WindowOptions, div,
        point, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, CommandError, Tab};
    use tl_model::demand::Demand;
    use tl_model::health::SourceId;
    use tl_model::journal::{JournalHandle, JournalQuery};
    use tl_model::settings::EngineSettings;
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{Snapshot, StartupCounts};
    use tl_model::{actions::ActionEvent, table::SysTable};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::{
        MenuEntryInfo, MenuEntryKind, TlMenu, TlMenuPopup, open_context_menu,
    };
    use tl_ui::components::toast::{ToastKey, ToastQueue};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::host::HostContext;
    use tl_ui::hub::GlobalHub;
    use tl_ui::pages::startup::actions::bind_keys_startup;
    use tl_ui::pages::startup::{REFRESH_TOAST_AFTER, StartupPage};
    use tl_ui::pages::sys_table::SysBody;
    use tl_ui::table::actions::OpenProperties;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::filter::FILTER_DEBOUNCE;
    use tl_ui::window::main::{
        MainWindow, PageSlot, main_window_view, open_main_window_on, set_test_page,
    };

    struct Host {
        page: Entity<StartupPage>,
        opened: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = Rc::clone(&self.opened);
            div()
                .size_full()
                .on_action(move |_: &OpenProperties, _, _| opened.set(opened.get() + 1))
                .child(
                    self.page
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        page: Entity<StartupPage>,
        _queue: Entity<ToastQueue>,
        opened: Rc<Cell<u32>>,
        vcx: VisualTestContext,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        rig_with_client(cx, engine.clone(), engine)
    }

    fn rig_with_client(
        cx: &mut TestAppContext,
        engine: Arc<FakeEngine>,
        client: Arc<dyn EngineClient>,
    ) -> Rig {
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
            TasklensUi::init(cx, client.clone(), init);
            bind_keys_startup(cx);
            cx.set_reduce_motion(true);
        });
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1280.), px(800.)),
            })),
            ..WindowOptions::default()
        };
        let page_slot = Rc::new(RefCell::new(None));
        let opened = Rc::new(Cell::new(0));
        let handle = cx
            .update(|cx| {
                let page_slot = Rc::clone(&page_slot);
                let opened = Rc::clone(&opened);
                cx.open_window(options, move |window, cx| {
                    let host_ctx = HostContext::local(cx);
                    let page = cx.new(|cx| StartupPage::new(&host_ctx, window, cx));
                    *page_slot.borrow_mut() = Some(page.clone());
                    let host = cx.new(|_| Host { page, opened });
                    cx.new(|cx| Root::new(host, window, cx))
                })
            })
            .expect("the window opens");
        let page = page_slot.borrow_mut().take().expect("the page opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
        let mut rig = Rig {
            engine,
            page,
            _queue: queue,
            opened,
            vcx,
            version: 0,
        };
        rig.draw();
        rig
    }

    /// A demand-responsive client. It publishes startup data only when the page sends a complete
    /// visible-surface demand, matching the real worker's start condition from leader comment 159.
    /// The page against the real engine is `startup_page_fills_from_real_engine` in
    /// `tl-app/tests/live_engine.rs`.
    struct DemandDrivenEngine {
        inner: Arc<FakeEngine>,
    }

    impl EngineClient for DemandDrivenEngine {
        fn latest(&self) -> Arc<Snapshot> {
            self.inner.latest()
        }

        fn wake(&self) -> async_channel::Receiver<()> {
            self.inner.wake()
        }

        fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
            self.inner.action_events()
        }

        fn set_demand(&self, demand: Demand) {
            self.inner.set_demand(demand.clone());
            if !demand.any_surface_visible || !demand.startup {
                return;
            }
            let generation = self
                .inner
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) if query.table == SysTable::Startup => {
                        Some(query.generation)
                    }
                    _ => None,
                })
                .expect("startup query precedes demand flush");
            let entry = super::entry("Live entry", StartupKind::AutostartUser);
            let node = SysNodeId::Startup(entry.id.clone());
            self.inner.publish_with(|snapshot| {
                snapshot.startup = Arc::new(StartupSnapshot {
                    header: ComponentHeader {
                        version: 1,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: vec![SysRow {
                        node,
                        depth: 0,
                        expanded: false,
                        has_children: false,
                        record: Some(0),
                    }]
                    .into(),
                    query_generation: generation,
                    entries: vec![entry].into(),
                    counts: StartupCounts {
                        total: 1,
                        enabled: 1,
                        disabled: 0,
                    },
                });
            });
        }

        fn set_settings(&self, settings: EngineSettings) {
            self.inner.set_settings(settings);
        }

        fn send(&self, command: Command) -> Result<(), CommandError> {
            self.inner.send(command)
        }

        fn journal(&self, query: JournalQuery) -> JournalHandle {
            self.inner.journal(query)
        }
    }

    fn startup_page(window: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let host_ctx = HostContext::local(cx);
        let page = cx.new(|cx| StartupPage::new(&host_ctx, window, cx));
        PageSlot::new(page, cx)
    }

    impl Rig {
        /// Draws a frame and tells the hub the main window rendered, as `MainWindow::render`
        /// does. Without it the hub goes hidden after 1 s and drops Info toasts.
        fn draw(&mut self) {
            self.vcx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let hub = cx.global::<GlobalHub>().0.clone();
                hub.update(cx, |hub, cx| hub.note_render(cx));
            });
            self.vcx.run_until_parked();
        }

        fn set_active(&mut self, active: bool) {
            self.page
                .update(&mut self.vcx, |page, cx| page.set_active(active, cx));
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.vcx.run_until_parked();
        }

        fn generation(&self) -> u64 {
            self.engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) => Some(query.generation),
                    _ => None,
                })
                .unwrap_or(0)
        }

        fn publish(&mut self, names: &[&str], enabled: usize, counts: StartupCounts) {
            let entries: Vec<StartupEntry> = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let mut entry = super::entry(name, StartupKind::AutostartUser);
                    entry.status = if index < enabled {
                        StartupStatus::Enabled
                    } else {
                        StartupStatus::Disabled
                    };
                    entry
                })
                .collect();
            self.publish_entries(entries, counts);
        }

        fn publish_entries(&mut self, entries: Vec<StartupEntry>, counts: StartupCounts) {
            self.version += 1;
            let generation = self.generation();
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
            let version = self.version;
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
            self.vcx.run_until_parked();
            self.draw();
        }

        fn table(&self) -> Entity<tl_ui::table::tree_table::TlTreeTable<StartupDelegate>> {
            self.vcx
                .read_entity(&self.page, |page, _| page.table().clone())
        }

        fn build_menu(&mut self, targets: &[u64], col: Option<SysColumnId>) -> Entity<TlMenuPopup> {
            let table = self.table();
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    table.update(cx, |table, cx| {
                        table.delegate.context_menu(targets, col, menu, window, cx)
                    })
                })
            })
        }

        fn menu_entries(&mut self, menu: &Entity<TlMenuPopup>) -> Vec<MenuEntryInfo> {
            self.vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window))
        }

        fn row_ids(&mut self, count: usize) -> Vec<u64> {
            let table = self.table();
            self.vcx.read_entity(&table, |table, _| {
                (0..count).map(|ix| table.delegate.row_id(ix)).collect()
            })
        }

        fn toast_texts(&mut self) -> Vec<String> {
            self.vcx.update(|window, cx| {
                ToastQueue::of_window(window, cx)
                    .map(|queue| {
                        queue
                            .read(cx)
                            .visible_texts()
                            .iter()
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            })
        }

        fn status(&mut self) -> String {
            self.vcx.update(|_, cx| {
                let mut out = String::new();
                self.page.read(cx).status_text(&mut out);
                out
            })
        }

        fn select_first_two(&mut self) {
            let table = self
                .vcx
                .read_entity(&self.page, |page, _| page.table().clone());
            table.update(&mut self.vcx, |table, cx| {
                let first = table.delegate.row_id(0);
                let second = table.delegate.row_id(1);
                table.selection.click(first);
                table.selection.ctrl_click(second);
                cx.notify();
            });
            self.vcx.run_until_parked();
        }

        fn refresh_toasts(&mut self) -> usize {
            self.vcx.update(|window, cx| {
                ToastQueue::of_window(window, cx)
                    .expect("the window has a toast queue")
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .filter(|text| text.as_ref() == "Startup entries refreshed.")
                    .count()
            })
        }

        /// Closes the refresh toast, so a later push shows it again and a duplicate is counted.
        fn dismiss_refresh_toast(&mut self) {
            let key = ToastKey(self.page.entity_id().as_u64());
            self.vcx.update(|window, cx| {
                ToastQueue::of_window(window, cx)
                    .expect("the window has a toast queue")
                    .update(cx, |queue, cx| queue.remove(key, cx));
            });
            self.vcx.run_until_parked();
            assert_eq!(self.refresh_toasts(), 0, "the refresh toast closed");
        }

        fn advance(&mut self, by: Duration) {
            self.vcx.executor().advance_clock(by);
            self.vcx.run_until_parked();
        }

        /// A real double-click: a single click, then a press and release with click count 2.
        fn double_click(&mut self, at: Point<Pixels>) {
            self.vcx.simulate_click(at, Modifiers::none());
            self.vcx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count: 2,
                first_mouse: false,
            });
            self.vcx.simulate_event(MouseUpEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count: 2,
            });
            self.vcx.run_until_parked();
            self.draw();
        }

        fn selected_names(&mut self) -> Vec<String> {
            let table = self
                .vcx
                .read_entity(&self.page, |page, _| page.table().clone());
            self.vcx.read_entity(&table, |table, _| {
                table
                    .selection
                    .ids()
                    .filter_map(|id| table.delegate.row_index(id))
                    .filter_map(|index| table.delegate.entry(index))
                    .map(|entry| entry.name.to_string())
                    .collect()
            })
        }
    }

    fn menu_shape(entries: &[MenuEntryInfo]) -> Vec<(MenuEntryKind, String)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.kind,
                    entry.label.as_deref().unwrap_or("separator").to_owned(),
                )
            })
            .collect()
    }

    fn counts_for(entries: &[StartupEntry]) -> StartupCounts {
        let enabled = entries
            .iter()
            .filter(|e| e.status == StartupStatus::Enabled)
            .count() as u32;
        StartupCounts {
            total: entries.len() as u32,
            enabled,
            disabled: entries.len() as u32 - enabled,
        }
    }

    fn menu_rig(cx: &mut TestAppContext, entries: Vec<StartupEntry>) -> Rig {
        let mut rig = rig(cx);
        rig.set_active(true);
        let counts = counts_for(&entries);
        rig.publish_entries(entries, counts);
        rig
    }

    #[gpui_kit::test]
    fn startup_menu_order(cx: &mut TestAppContext) {
        let mut rig = menu_rig(cx, vec![super::entry("a", StartupKind::AutostartUser)]);
        let ids = rig.row_ids(1);
        let menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
        let all = rig.menu_entries(&menu);
        let item = MenuEntryKind::Item;
        let sep = MenuEntryKind::Separator;
        let expected: Vec<(MenuEntryKind, String)> = [
            (item, "Disable"),
            (item, "Run now"),
            (sep, "separator"),
            (item, "Copy"),
            (item, "Copy row"),
            (item, "Copy rows as table"),
            (item, "Copy command"),
            (item, "Open file location"),
            (item, "Properties"),
            (sep, "separator"),
            (item, "Delete entry\u{2026}"),
        ]
        .into_iter()
        .map(|(kind, label)| (kind, label.to_owned()))
        .collect();
        assert_eq!(menu_shape(&all), expected);
        let hint = |label: &str| {
            all.iter()
                .find(|e| e.label.as_deref() == Some(label))
                .and_then(|e| e.hint.clone())
                .map(|h| h.to_string())
        };
        let format = |key: &str| Kbd::format(&Keystroke::parse(key).expect("a keystroke"));
        assert_eq!(hint("Copy row"), Some(format("ctrl-c")));
        assert_eq!(hint("Copy rows as table"), Some(format("ctrl-shift-c")));
        assert_eq!(hint("Properties"), Some(format("enter")));
        assert_eq!(hint("Delete entry\u{2026}"), Some(format("delete")));
    }

    #[gpui_kit::test]
    fn startup_menu_mixed_shows_both(cx: &mut TestAppContext) {
        let on = super::entry("a", StartupKind::AutostartUser);
        let mut off = super::entry("b", StartupKind::AutostartUser);
        off.status = StartupStatus::Disabled;
        let mut rig = menu_rig(cx, vec![on, off]);
        let ids = rig.row_ids(2);
        let menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
        let all = rig.menu_entries(&menu);
        let kind = |label: &str| {
            all.iter()
                .find(|e| e.label.as_deref() == Some(label))
                .map(|e| e.kind)
        };
        assert_eq!(all[0].label.as_deref(), Some("Disable"));
        assert_eq!(all[1].label.as_deref(), Some("Enable"));
        assert_eq!(kind("Disable"), Some(MenuEntryKind::Item));
        assert_eq!(kind("Enable"), Some(MenuEntryKind::Item));
        assert_eq!(kind("Copy command"), Some(MenuEntryKind::Disabled));
        assert_eq!(kind("Open file location"), Some(MenuEntryKind::Disabled));
    }

    #[gpui_kit::test]
    fn startup_menu_reason_tooltips(cx: &mut TestAppContext) {
        let mut not_here = super::entry("a", StartupKind::AutostartUser);
        not_here.status = StartupStatus::NotInThisDesktop;
        not_here.can_toggle = false;
        let mut missing = super::entry("b", StartupKind::AutostartUser);
        missing.status = StartupStatus::MissingProgram;
        missing.can_toggle = false;
        let mut masked = super::entry("c", StartupKind::UserUnit);
        masked.status = StartupStatus::Masked;
        masked.can_toggle = false;
        let mut system = super::entry("d", StartupKind::AutostartSystem);
        system.can_delete = false;
        let mut package = super::entry("e", StartupKind::UserUnit);
        package.can_delete = false;
        let cases: [(StartupEntry, &str, &str); 5] = [
            (
                not_here,
                "Enable",
                "This entry is not used in this desktop (OnlyShowIn or NotShowIn).",
            ),
            (
                missing,
                "Enable",
                "The program for this entry is not installed (TryExec).",
            ),
            (
                masked,
                "Enable",
                "This unit is masked. Change its startup type in the Services tab.",
            ),
            (
                system,
                "Delete entry\u{2026}",
                "System entries cannot be deleted. Use Disable instead.",
            ),
            (
                package,
                "Delete entry\u{2026}",
                "This unit belongs to a package. Use Disable instead.",
            ),
        ];
        for (entry, label, tip) in cases {
            let mut rig = menu_rig(cx, vec![entry]);
            let ids = rig.row_ids(1);
            let menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
            let all = rig.menu_entries(&menu);
            let found = all
                .iter()
                .find(|e| e.label.as_deref() == Some(label))
                .expect("the item exists");
            assert_eq!(found.kind, MenuEntryKind::Disabled, "{label}");
            assert_eq!(found.tooltip.as_deref(), Some(tip));
        }
    }

    #[gpui_kit::test]
    fn delete_key_on_system_entry_toast(cx: &mut TestAppContext) {
        let mut system = super::entry("sys", StartupKind::AutostartSystem);
        system.can_delete = false;
        let mut rig = menu_rig(cx, vec![system]);
        let ids = rig.row_ids(1);
        let table = rig.table();
        table.update(&mut rig.vcx, |table, cx| {
            table.selection.click(ids[0]);
            cx.notify();
        });
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        });
        rig.draw();
        let windows = rig.vcx.update(|_, cx| cx.windows().len());
        rig.vcx.simulate_keystrokes("delete");
        rig.vcx.run_until_parked();
        assert!(
            rig.toast_texts()
                .iter()
                .any(|text| text == "This startup entry cannot be deleted")
        );
        assert_eq!(rig.vcx.update(|_, cx| cx.windows().len()), windows);
    }

    #[gpui_kit::test]
    fn startup_menu_targets_captured_at_open(cx: &mut TestAppContext) {
        let a = super::entry("a", StartupKind::AutostartUser);
        let b = super::entry("b", StartupKind::AutostartUser);
        let a_ref = a.id.clone();
        let mut rig = menu_rig(cx, vec![a.clone()]);
        let ids = rig.row_ids(1);
        let _menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
        let both = vec![b, a];
        let counts = counts_for(&both);
        rig.publish_entries(both, counts);
        let refs = rig
            .vcx
            .update(|_, cx| rig.page.read(cx).menu_target_refs_for_test().to_vec());
        assert_eq!(refs, [a_ref]);
    }

    // ---- Enable, Disable and Run now (tasklens-9o0.9) ----

    use tl_model::actions::{ActionError, ActionEvent as FlowEvent, ActionId, ActionRequest};
    use tl_model::actions::{ActionKind, ActionState, ActionTarget, RejectReason};
    use tl_ui::pages::processes::actions::CopyDetails;
    use tl_ui::pages::startup::actions::{
        StartupCopyCommand, StartupDisable, StartupEnable, StartupOpenFileLocation, StartupRunNow,
    };

    fn with_status(name: &str, status: StartupStatus) -> StartupEntry {
        let mut e = super::entry(name, StartupKind::AutostartUser);
        e.status = status;
        e
    }

    /// A rig with `entries` published and the rows at `selected` selected and focused.
    fn flow_rig(cx: &mut TestAppContext, entries: Vec<StartupEntry>, selected: &[usize]) -> Rig {
        let mut rig = menu_rig(cx, entries);
        let ids = rig.row_ids(selected.iter().copied().max().map_or(0, |m| m + 1));
        let table = rig.table();
        table.update(&mut rig.vcx, |table, cx| {
            for (n, ix) in selected.iter().enumerate() {
                if n == 0 {
                    table.selection.click(ids[*ix]);
                } else {
                    table.selection.ctrl_click(ids[*ix]);
                }
            }
            cx.notify();
        });
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        });
        rig.draw();
        rig
    }

    impl Rig {
        fn dispatch(&mut self, action: Box<dyn gpui_kit::Action>) {
            self.vcx
                .update(|window, cx| window.dispatch_action(action, cx));
            self.vcx.run_until_parked();
        }

        fn requests(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .collect()
        }

        fn emit_action(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
            self.engine.emit(FlowEvent { id, target, state });
            self.vcx.run_until_parked();
        }

        /// `Started`, one state per target, then the summary.
        fn finish_action(&mut self, id: ActionId, states: &[ActionState]) {
            self.emit_action(id, None, ActionState::Started);
            for (i, state) in states.iter().enumerate() {
                self.emit_action(id, Some(i as u16), state.clone());
            }
            self.emit_action(id, None, ActionState::summarize(states));
            self.draw();
        }

        fn status_cell(&mut self, ix: usize) -> String {
            let table = self.table();
            self.vcx.read_entity(&table, |table, _| {
                let mut out = CellOut::default();
                table.delegate.cell(ix, id(StartupColumn::Status), &mut out);
                text(&out).to_owned()
            })
        }

        fn status_dot(&mut self, ix: usize) -> Option<DotStyle> {
            let table = self.table();
            self.vcx.read_entity(&table, |table, _| {
                let mut out = CellOut::default();
                table.delegate.cell(ix, id(StartupColumn::Status), &mut out);
                out.dot
            })
        }
    }

    fn startup_target(name: &str) -> ActionTarget {
        ActionTarget::Startup(super::entry(name, StartupKind::AutostartUser).id)
    }

    #[gpui_kit::test]
    fn startup_enable_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![with_status("a", StartupStatus::Disabled)], &[0]);
        rig.dispatch(Box::new(StartupEnable));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::StartupEnable);
        assert_eq!(sent[0].targets.as_slice(), [startup_target("a")]);
        assert!(
            !rig.toast_texts().iter().any(|t| t.contains("Enabling")),
            "no pending toast"
        );
        rig.finish_action(sent[0].id, &[ActionState::Succeeded]);
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Startup entry enabled.")
        );
    }

    #[gpui_kit::test]
    fn startup_enable_failure_text(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![with_status("a", StartupStatus::Disabled)], &[0]);
        rig.dispatch(Box::new(StartupEnable));
        let id = rig.requests()[0].id;
        rig.finish_action(id, &[ActionState::Failed(ActionError::PermissionDenied)]);
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Failed to update startup entry: not authorized"),
            "{:?}",
            rig.toast_texts()
        );
    }

    #[gpui_kit::test]
    fn startup_disable_skips_already_disabled(cx: &mut TestAppContext) {
        let entries = vec![
            with_status("a", StartupStatus::Disabled),
            with_status("b", StartupStatus::Disabled),
            with_status("c", StartupStatus::Enabled),
        ];
        let mut rig = flow_rig(cx, entries, &[0, 1, 2]);
        rig.dispatch(Box::new(StartupDisable));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::StartupDisable);
        assert_eq!(sent[0].targets.as_slice(), [startup_target("c")]);
    }

    #[gpui_kit::test]
    fn startup_toggle_empty_is_noop(cx: &mut TestAppContext) {
        let entries = vec![
            with_status("a", StartupStatus::Disabled),
            with_status("b", StartupStatus::Disabled),
        ];
        let mut rig = flow_rig(cx, entries, &[0, 1]);
        rig.dispatch(Box::new(StartupDisable));
        assert!(rig.requests().is_empty());
        assert!(rig.toast_texts().is_empty(), "{:?}", rig.toast_texts());
    }

    #[gpui_kit::test]
    fn startup_enable_summary_at_4(cx: &mut TestAppContext) {
        let entries: Vec<StartupEntry> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|n| with_status(n, StartupStatus::Disabled))
            .collect();
        let mut rig = flow_rig(cx, entries, &[0, 1, 2, 3]);
        rig.dispatch(Box::new(StartupEnable));
        let sent = rig.requests();
        assert_eq!(sent[0].targets.len(), 4);
        let ok = ActionState::Succeeded;
        let fail = ActionState::Failed(ActionError::PermissionDenied);
        rig.finish_action(sent[0].id, &[ok.clone(), ok.clone(), ok, fail]);
        assert_eq!(
            rig.toast_texts(),
            ["3 of 4 startup entries enabled. 1 failed."]
        );
    }

    #[gpui_kit::test]
    fn startup_updating_status_while_pending(cx: &mut TestAppContext) {
        use tl_ui::pages::startup::delegate::UPDATING_LOOK;
        let mut rig = flow_rig(cx, vec![with_status("a", StartupStatus::Disabled)], &[0]);
        assert_eq!(rig.status_cell(0), "Disabled");
        rig.dispatch(Box::new(StartupEnable));
        assert_eq!(rig.status_cell(0), "Updating\u{2026}");
        assert_eq!(UPDATING_LOOK.dot, DotStyle::Filled(LabelTone::Info));
        assert_eq!(rig.status_dot(0), Some(UPDATING_LOOK.dot));
        let refreshes = |rig: &Rig| {
            rig.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::RefreshNow(SourceId::Startup)))
                .count()
        };
        let before = refreshes(&rig);
        let id = rig.requests()[0].id;
        rig.finish_action(id, &[ActionState::Succeeded]);
        assert_eq!(rig.status_cell(0), "Disabled");
        assert_eq!(
            refreshes(&rig),
            before + 1,
            "a finished Enable or Disable rescans at once instead of waiting for the watcher"
        );
    }

    #[gpui_kit::test]
    fn run_now_any_status(cx: &mut TestAppContext) {
        let mut masked = with_status("a", StartupStatus::Masked);
        masked.can_toggle = false;
        let mut rig = flow_rig(cx, vec![masked], &[0]);
        rig.dispatch(Box::new(StartupRunNow));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::StartupRunNow);
        assert_eq!(sent[0].targets.as_slice(), [startup_target("a")]);
        assert_eq!(rig.status_cell(0), "Masked", "Run now changes no row");
    }

    #[gpui_kit::test]
    fn run_now_already_running_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![with_status("a", StartupStatus::Enabled)], &[0]);
        rig.dispatch(Box::new(StartupRunNow));
        let id = rig.requests()[0].id;
        rig.finish_action(id, &[ActionState::Rejected(RejectReason::AlreadyRunning)]);
        assert_eq!(rig.toast_texts(), ["Already running"]);
    }

    #[gpui_kit::test]
    fn startup_busy_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![with_status("a", StartupStatus::Disabled)], &[0]);
        rig.dispatch(Box::new(StartupEnable));
        rig.dispatch(Box::new(StartupEnable));
        assert_eq!(rig.requests().len(), 1);
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "A startup action is already in progress.")
        );
    }

    #[gpui_kit::test]
    fn startup_enable_targets_only_captured_refs(cx: &mut TestAppContext) {
        let a = with_status("a", StartupStatus::Disabled);
        let b = with_status("b", StartupStatus::Disabled);
        let mut rig = menu_rig(cx, vec![a.clone()]);
        let ids = rig.row_ids(1);
        let _menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
        let both = vec![b, a];
        let counts = counts_for(&both);
        rig.publish_entries(both, counts);
        let table = rig.table();
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        });
        rig.dispatch(Box::new(StartupEnable));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].targets.as_slice(),
            [startup_target("a")],
            "only the entry captured at menu open"
        );
    }

    #[gpui_kit::test]
    fn startup_menu_close_drops_captured_targets(cx: &mut TestAppContext) {
        let a = super::entry("a", StartupKind::AutostartUser);
        let mut rig = menu_rig(cx, vec![a]);
        let ids = rig.row_ids(1);
        let _menu = rig.build_menu(&ids, Some(SysColumnId::Startup(StartupColumn::Name)));
        let table = rig.table();
        rig.vcx
            .update(|_, cx| table.update(cx, |table, cx| table.delegate.on_menu_closed(cx)));
        let refs = rig
            .vcx
            .update(|_, cx| rig.page.read(cx).menu_target_refs_for_test().to_vec());
        assert!(refs.is_empty());
    }

    #[gpui_kit::test]
    fn startup_menu_keyboard_hides_copy_cell(cx: &mut TestAppContext) {
        let mut rig = menu_rig(cx, vec![super::entry("a", StartupKind::AutostartUser)]);
        let ids = rig.row_ids(1);
        let table = rig.table();
        table.update(&mut rig.vcx, |table, cx| {
            table.selection.click(ids[0]);
            cx.notify();
        });
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        });
        rig.draw();
        rig.vcx.simulate_keystrokes("shift-f10");
        rig.vcx.run_until_parked();
        let menu = rig
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("the keyboard opened the menu");
        let all = rig.menu_entries(&menu);
        assert!(all.iter().any(|e| e.label.as_deref() == Some("Copy row")));
        assert!(all.iter().all(|e| e.label.as_deref() != Some("Copy")));
    }

    #[gpui_kit::test]
    fn startup_status_text(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish(
            &["Discord", "Steam"],
            2,
            StartupCounts {
                total: 12,
                enabled: 9,
                disabled: 3,
            },
        );
        assert_eq!(rig.status(), "12 Total | 9 Enabled | 3 Disabled");
        rig.publish(
            &["Discord", "Steam"],
            2,
            StartupCounts {
                total: 12_345,
                enabled: 9,
                disabled: 12_336,
            },
        );
        assert_eq!(
            rig.status(),
            "12\u{2009}345 Total | 9 Enabled | 12\u{2009}336 Disabled"
        );
        rig.select_first_two();
        assert_eq!(
            rig.status(),
            "12\u{2009}345 Total | 9 Enabled | 12\u{2009}336 Disabled | 2 selected"
        );
    }

    #[gpui_kit::test]
    fn startup_demand_and_first_visit_refresh(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        assert!(
            rig.engine
                .demands()
                .last()
                .is_some_and(|d| { d.any_surface_visible && d.startup })
        );
        rig.set_active(false);
        assert_eq!(
            rig.engine.demands().last(),
            Some(&Demand {
                device_history: true,
                ..Demand::default()
            })
        );
        rig.set_active(true);
        let refreshes = rig
            .engine
            .commands()
            .iter()
            .filter(|command| matches!(command, Command::RefreshNow(SourceId::Startup)))
            .count();
        assert_eq!(refreshes, 1);
    }

    #[gpui_kit::test]
    fn startup_data_arrives_from_demand_driven_engine(cx: &mut TestAppContext) {
        let inner = FakeEngine::new();
        let client: Arc<dyn EngineClient> = Arc::new(DemandDrivenEngine {
            inner: Arc::clone(&inner),
        });
        let mut rig = rig_with_client(cx, inner, client);
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Skeleton
        );
        rig.set_active(true);
        rig.draw();
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Rows
        );
        assert_eq!(rig.status(), "1 Total | 1 Enabled | 0 Disabled");
    }

    #[gpui_kit::test]
    fn startup_skeleton_then_rows(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        assert_eq!(rig.status(), "Loading…");
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Skeleton
        );
        rig.set_active(true);
        rig.publish(
            &["Discord"],
            1,
            StartupCounts {
                total: 1,
                enabled: 1,
                disabled: 0,
            },
        );
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Rows
        );
        assert_eq!(rig.status(), "1 Total | 1 Enabled | 0 Disabled");
    }

    #[gpui_kit::test]
    fn startup_refresh_toast(cx: &mut TestAppContext) {
        let counts = StartupCounts {
            total: 1,
            enabled: 1,
            disabled: 0,
        };
        let mut rig = rig(cx);
        assert!(
            rig.vcx
                .update(|window, cx| ToastQueue::of_window(window, cx).is_some())
        );
        rig.set_active(true);
        rig.publish(&["Discord"], 1, counts);

        // No new version: the toast shows at 2 s, once.
        rig.page.update(&mut rig.vcx, |page, cx| page.refresh(cx));
        rig.vcx.run_until_parked();
        rig.advance(REFRESH_TOAST_AFTER - Duration::from_millis(1));
        assert_eq!(rig.refresh_toasts(), 0, "no toast before two seconds");
        rig.advance(Duration::from_millis(1));
        assert_eq!(rig.refresh_toasts(), 1, "fallback toast at two seconds");
        rig.dismiss_refresh_toast();
        rig.advance(REFRESH_TOAST_AFTER + Duration::from_secs(1));
        assert_eq!(rig.refresh_toasts(), 0, "the fallback toast is pushed once");

        // A new version 500 ms after F5: the toast shows then, and the 2 s timer adds none.
        rig.page.update(&mut rig.vcx, |page, cx| page.refresh(cx));
        rig.vcx.run_until_parked();
        rig.advance(Duration::from_millis(500));
        assert_eq!(rig.refresh_toasts(), 0, "no toast before the new version");
        rig.publish(&["Discord"], 1, counts);
        assert_eq!(
            rig.refresh_toasts(),
            1,
            "toast when the new version arrives"
        );
        rig.dismiss_refresh_toast();
        rig.publish(&["Discord"], 1, counts);
        rig.advance(REFRESH_TOAST_AFTER);
        assert_eq!(
            rig.refresh_toasts(),
            0,
            "no second toast from a later version or the timer"
        );
    }

    #[gpui_kit::test]
    fn startup_rescan_keeps_selection(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish(
            &["Discord", "Steam"],
            2,
            StartupCounts {
                total: 2,
                enabled: 2,
                disabled: 0,
            },
        );
        let table = rig
            .vcx
            .read_entity(&rig.page, |page, _| page.table().clone());
        table.update(&mut rig.vcx, |table, cx| {
            table.selection.click(table.delegate.row_id(0));
            cx.notify();
        });
        rig.publish(
            &["Steam", "Discord"],
            2,
            StartupCounts {
                total: 2,
                enabled: 2,
                disabled: 0,
            },
        );
        assert_eq!(rig.selected_names(), ["Discord"]);
    }

    #[gpui_kit::test]
    fn startup_double_click_opens_properties(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish(
            &["Discord", "Steam"],
            2,
            StartupCounts {
                total: 2,
                enabled: 2,
                disabled: 0,
            },
        );
        let table = rig.vcx.debug_bounds("TlTreeTable").expect("table drawn");
        let body = rig
            .vcx
            .debug_bounds("TlTreeTable-body")
            .expect("table body drawn");
        assert!(body.origin.y > table.origin.y, "the table has a header");
        // Inside the Name column, clear of the column borders.
        let x = body.origin.x + px(60.);
        let header = point(x, table.origin.y + (body.origin.y - table.origin.y) * 0.5);
        let row_h = 28.;
        let second_row = point(x, body.origin.y + px(row_h * 1.5));
        let below_rows = point(x, body.origin.y + px(row_h * 2. + 40.));

        let starts = |rig: &mut Rig| {
            rig.vcx.update(|_, cx| {
                cx.global::<tl_ui::window::registry::GlobalRegistry>()
                    .0
                    .read(cx)
                    .count(tl_ui::window::registry::CapClass::StartupProps)
            })
        };
        rig.double_click(below_rows);
        assert_eq!(starts(&mut rig), 0, "empty body double-click");

        rig.double_click(second_row);
        assert_eq!(
            rig.selected_names(),
            ["Steam"],
            "the click hit the second row"
        );
        assert_eq!(starts(&mut rig), 1, "row double-click opens Properties");
        assert_eq!(rig.opened.get(), 0, "the page handles OpenProperties");

        // Last, because a header click sorts and sends a new query.
        rig.double_click(header);
        assert_eq!(starts(&mut rig), 1, "header double-click with a selection");
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

    // Delete confirmation and flow (tasklens-9o0.10, UI-SYS-047).

    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::pages::startup::actions::StartupDelete;
    use tl_ui::windows::confirm_startup_delete::{StartupEntryView, delete_texts};

    /// A deletable user entry of `kind` at its own path.
    fn deletable(name: &str, kind: StartupKind) -> StartupEntry {
        let mut e = super::entry(name, kind);
        e.source_path = Arc::from(format!("/home/u/.config/autostart/{name}.desktop"));
        e
    }

    fn system_entry(name: &str) -> StartupEntry {
        let mut e = super::entry(name, StartupKind::AutostartSystem);
        e.id.source = StartupSource::AutostartSystem;
        e.can_delete = false;
        e
    }

    struct Dialog {
        window: gpui_kit::WindowHandle<Root>,
        view: Entity<TlDialogView>,
    }

    impl Rig {
        fn page_window_id(&mut self) -> gpui_kit::WindowId {
            gpui_kit::VisualContext::window_handle(&self.vcx).window_id()
        }

        /// Dispatches Delete and returns the confirmation window, if one opened.
        fn open_delete(&mut self) -> Option<Dialog> {
            let parent = self.page_window_id();
            self.dispatch(Box::new(StartupDelete));
            self.dialog(parent)
        }

        fn dialog(&mut self, parent: gpui_kit::WindowId) -> Option<Dialog> {
            let window = self
                .vcx
                .update(|_, cx| cx.windows().into_iter().find(|w| w.window_id() != parent))?
                .downcast::<Root>()?;
            let frame = self
                .vcx
                .update(|_, cx| TlSecondaryWindow::<TlDialogView>::find(window, cx))?;
            let view = self.vcx.read(|cx| frame.read(cx).content().clone());
            Some(Dialog { window, view })
        }

        fn is_open(&mut self, d: &Dialog) -> bool {
            let id = d.window.window_id();
            self.vcx
                .update(|_, cx| cx.windows().iter().any(|w| w.window_id() == id))
        }

        fn focused_button(&mut self, d: &Dialog, cx: &mut TestAppContext) -> Option<DialogButton> {
            let id = d.window.window_id();
            let view = d.view.clone();
            let window = cx
                .update(|cx| cx.windows().into_iter().find(|w| w.window_id() == id))
                .expect("dialog window");
            let mut vcx = VisualTestContext::from_window(window, cx);
            vcx.update(|window, cx| view.read(cx).focused_button(window))
        }
    }

    fn views(entries: &[StartupEntry]) -> Vec<StartupEntryView> {
        entries
            .iter()
            .map(|e| StartupEntryView {
                name: e.name.to_string().into(),
                source_path: e.source_path.to_string().into(),
                kind: e.kind,
            })
            .collect()
    }

    #[gpui_kit::test]
    fn delete_dialog_texts_per_type(cx: &mut TestAppContext) {
        let mut rig = flow_rig(
            cx,
            vec![deletable("Discord", StartupKind::AutostartUser)],
            &[0],
        );
        let dialog = rig.open_delete().expect("the dialog opens");
        let (title, text) = rig.vcx.read(|cx| {
            let view = dialog.view.read(cx);
            (
                view.dialog().window_title().to_string(),
                view.dialog().text().to_string(),
            )
        });
        assert_eq!(title, "tasklens Delete Startup Entry");
        assert_eq!(text, "Permanently remove this startup entry?");
        assert!(
            rig.vcx
                .read(|cx| dialog.view.read(cx).dialog().is_destructive())
        );

        let one = |kind| {
            let entry = deletable("Discord", kind);
            rig.vcx.read(|cx| delete_texts(&views(&[entry]), 0, cx))
        };
        let xdg = one(StartupKind::AutostartUser);
        assert_eq!(xdg.heading, "Delete startup entry");
        assert_eq!(
            xdg.type_line.as_deref(),
            Some("The autostart file will be deleted. The app stays installed.")
        );
        assert_eq!(xdg.names, ["Discord"]);
        assert_eq!(
            xdg.path.as_deref(),
            Some("/home/u/.config/autostart/Discord.desktop")
        );
        assert_eq!(
            one(StartupKind::UserUnit).type_line.as_deref(),
            Some("The unit will be disabled and its unit file deleted.")
        );
        for kind in [StartupKind::PlasmaPreStartup, StartupKind::PlasmaLogout] {
            assert_eq!(
                one(kind).type_line.as_deref(),
                Some("The script file will be deleted.")
            );
        }
    }

    #[gpui_kit::test]
    fn delete_dialog_focus_on_cancel(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![deletable("a", StartupKind::AutostartUser)], &[0]);
        let dialog = rig.open_delete().expect("the dialog opens");
        assert_eq!(rig.focused_button(&dialog, cx), Some(DialogButton::Cancel));
        // A stray Enter cancels and sends nothing.
        let window = dialog.window;
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(!rig.is_open(&dialog));
        assert!(rig.requests().is_empty());
    }

    #[gpui_kit::test]
    fn delete_dialog_sends_and_closes_on_result(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![deletable("a", StartupKind::AutostartUser)], &[0]);
        let dialog = rig.open_delete().expect("the dialog opens");
        let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_keystrokes("tab enter");
        vcx.run_until_parked();
        // Leader decision: the window closes at once and exactly one request is sent.
        assert!(!rig.is_open(&dialog));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::StartupDelete);
        assert_eq!(sent[0].targets.as_slice(), [startup_target("a")]);
        rig.finish_action(sent[0].id, &[ActionState::Succeeded]);
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Startup entry deleted.")
        );
    }

    #[gpui_kit::test]
    fn delete_second_on_pending_shows_busy(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![deletable("a", StartupKind::AutostartUser)], &[0]);
        for _ in 0..2 {
            let dialog = rig.open_delete().expect("the dialog opens");
            let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
            vcx.update(|window, _| window.activate_window());
            vcx.simulate_keystrokes("tab enter");
            vcx.run_until_parked();
            rig.draw();
            let table = rig.table();
            rig.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
            });
        }
        assert_eq!(rig.requests().len(), 1);
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "A startup action is already in progress.")
        );
    }

    #[gpui_kit::test]
    fn delete_multi_skips_undeletable(cx: &mut TestAppContext) {
        let mut entries: Vec<StartupEntry> = (0..5)
            .map(|i| deletable(&format!("u{i}"), StartupKind::AutostartUser))
            .collect();
        entries.push(system_entry("s0"));
        entries.push(system_entry("s1"));
        let texts = rig_texts(cx, &entries[..5], 2);
        assert_eq!(texts.heading, "Delete startup entries");
        assert_eq!(texts.text, "Permanently remove these 5 startup entries?");
        assert_eq!(texts.names.len(), 5);
        assert!(texts.more.is_none());
        assert!(texts.type_line.is_none() && texts.path.is_none());
        assert_eq!(
            texts.skipped.as_deref(),
            Some("2 entries cannot be deleted and are skipped.")
        );
        // Through the page: 7 selected, only the 5 deletable ones are sent.
        let mut rig = flow_rig(cx, entries, &[0, 1, 2, 3, 4, 5, 6]);
        let dialog = rig.open_delete().expect("the dialog opens");
        let text = rig
            .vcx
            .read(|cx| dialog.view.read(cx).dialog().text().to_string());
        assert_eq!(text, "Permanently remove these 5 startup entries?");
    }

    fn rig_texts(
        cx: &mut TestAppContext,
        entries: &[StartupEntry],
        skipped: usize,
    ) -> tl_ui::windows::confirm_startup_delete::DeleteTexts {
        cx.update(|cx| tl_ui::i18n::init_strings(cx, "en-US"));
        cx.update(|cx| delete_texts(&views(entries), skipped, cx))
    }

    #[gpui_kit::test]
    fn delete_multi_and_more(cx: &mut TestAppContext) {
        let entries: Vec<StartupEntry> = (0..8)
            .map(|i| deletable(&format!("u{i}"), StartupKind::AutostartUser))
            .collect();
        let texts = rig_texts(cx, &entries, 0);
        assert_eq!(texts.names, ["u0", "u1", "u2", "u3", "u4"]);
        assert_eq!(texts.more.as_deref(), Some("and 3 more"));
        assert!(texts.skipped.is_none());
    }

    #[gpui_kit::test]
    fn delete_escape_cancels(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![deletable("a", StartupKind::AutostartUser)], &[0]);
        let dialog = rig.open_delete().expect("the dialog opens");
        let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(!rig.is_open(&dialog));
        assert!(rig.requests().is_empty());
    }

    /// Security regression: a system entry in the selection never reaches the request.
    #[gpui_kit::test]
    fn delete_never_targets_system_entry(cx: &mut TestAppContext) {
        let entries = vec![
            deletable("mine", StartupKind::AutostartUser),
            system_entry("sys"),
        ];
        let mut rig = flow_rig(cx, entries, &[0, 1]);
        let dialog = rig.open_delete().expect("the dialog opens");
        let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_keystrokes("tab enter");
        vcx.run_until_parked();
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].targets.as_slice(), [startup_target("mine")]);
    }
    fn discord() -> StartupEntry {
        let mut e = super::entry("Discord", StartupKind::AutostartUser);
        e.publisher = Some(Arc::from("Discord Inc."));
        e.impact = Impact::Medium;
        e.command = Arc::from("/usr/bin/discord --start-minimized");
        e.resolved_path = Some(Arc::from("/usr/bin/discord"));
        e
    }

    impl Rig {
        fn clipboard(&mut self) -> Option<String> {
            self.vcx.read_from_clipboard().and_then(|item| item.text())
        }
    }

    #[gpui_kit::test]
    fn startup_copy_block_golden(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![discord()], &[0]);
        rig.dispatch(Box::new(CopyDetails));
        let expected = include_str!("../golden/startup_copy_block.txt").trim_end_matches('\n');
        assert_eq!(rig.clipboard().as_deref(), Some(expected));
        assert!(
            rig.toast_texts().iter().any(|t| t == "Copied to clipboard"),
            "{:?}",
            rig.toast_texts()
        );
    }

    #[gpui_kit::test]
    fn startup_copy_multi_blank_line(cx: &mut TestAppContext) {
        let mut second = discord();
        second.id.id = Arc::from("Other.desktop");
        second.name = Arc::from("Other");
        second.publisher = None;
        second.resolved_path = None;
        let mut rig = flow_rig(cx, vec![discord(), second], &[0, 1]);
        rig.dispatch(Box::new(CopyDetails));
        let text = rig.clipboard().expect("copied");
        let blocks: Vec<&str> = text.split("\n\n").collect();
        assert_eq!(blocks.len(), 2, "{text}");
        assert!(blocks[0].starts_with("Name: Discord\n"));
        assert!(blocks[1].starts_with("Name: Other\nPublisher: \n"));
        assert!(blocks[1].ends_with("Resolved path: Not found"));
        assert!(!text.ends_with('\n'));
    }

    #[gpui_kit::test]
    fn startup_copy_nothing_selected_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![discord()], &[]);
        rig.vcx
            .write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()));
        rig.dispatch(Box::new(CopyDetails));
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Select a startup row first."),
            "{:?}",
            rig.toast_texts()
        );
        assert_eq!(rig.clipboard().as_deref(), Some("before"));
    }

    #[gpui_kit::test]
    fn startup_copy_location_full_vs_tilde(cx: &mut TestAppContext) {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }
        let mut entry = discord();
        entry.location = Arc::from(format!("{home}/.config/autostart"));
        let mut rig = flow_rig(cx, vec![entry], &[0]);
        rig.dispatch(Box::new(CopyDetails));
        let block = rig.clipboard().expect("copied");
        assert!(
            block.contains(&format!("\nLocation: {home}/.config/autostart\n")),
            "{block}"
        );
        let page = rig.page.clone();
        rig.vcx.update(|window, cx| {
            page.update(cx, |page, cx| page.copy_rows_as_table_for_test(window, cx));
        });
        let table = rig.clipboard().expect("copied");
        let mut lines = table.lines();
        let header: Vec<&str> = lines.next().expect("header").split('\t').collect();
        let row: Vec<&str> = lines.next().expect("row").split('\t').collect();
        let at = header
            .iter()
            .position(|h| *h == "Location")
            .expect("column");
        assert_eq!(row[at], "~/.config/autostart");
    }

    #[gpui_kit::test]
    fn startup_copy_command_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![discord()], &[0]);
        rig.dispatch(Box::new(StartupCopyCommand));
        assert_eq!(
            rig.clipboard().as_deref(),
            Some("/usr/bin/discord --start-minimized")
        );
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Startup command copied.")
        );
    }

    #[gpui_kit::test]
    fn open_location_missing_file_toast(cx: &mut TestAppContext) {
        let mut entry = discord();
        entry.file_exists = false;
        let mut rig = flow_rig(cx, vec![entry], &[0]);
        rig.dispatch(Box::new(StartupOpenFileLocation));
        assert!(rig.requests().is_empty());
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Startup file location unavailable.")
        );
    }

    #[gpui_kit::test]
    fn open_location_failure_replaces_toast(cx: &mut TestAppContext) {
        let mut rig = flow_rig(cx, vec![discord()], &[0]);
        rig.dispatch(Box::new(StartupOpenFileLocation));
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::ShowInFileManager {
                path: Arc::from("/home/u/.config/autostart/app.desktop"),
                properties: false,
            }
        );
        assert!(sent[0].targets.is_empty());
        assert!(
            rig.toast_texts()
                .iter()
                .any(|t| t == "Opening file location")
        );
        rig.finish_action(
            sent[0].id,
            &[ActionState::Failed(ActionError::PermissionDenied)],
        );
        let toasts = rig.toast_texts();
        assert!(
            toasts.iter().any(|t| t == "Failed to open file location"),
            "{toasts:?}"
        );
        assert!(!toasts.iter().any(|t| t == "Opening file location"));
    }
}

#[gpui_kit::test]
fn startup_keymap(cx: &mut TestAppContext) {
    use tl_ui::pages::startup::actions::{StartupDelete, bind_keys_startup};

    cx.update(|cx| {
        setup(cx);
        bind_keys_startup(cx);
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        assert!(keymap.bindings_for_action(&StartupDelete).any(|binding| {
            binding.keystrokes().len() == 1
                && binding.keystrokes()[0].unparse() == "delete"
                && binding
                    .predicate()
                    .is_some_and(|predicate| predicate.to_string() == "StartupTable")
        }));
    });
}

/// `TasklensUi::init` binds the Startup keys and the shell builds `StartupPage` for its tab
/// (tasklens-9o0.13).
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn startup_keymap_registered(cx: &mut TestAppContext) {
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::startup::StartupPage;
    use tl_ui::pages::startup::actions::StartupDelete;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window};

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

/// The Startup Properties window (tasklens-9o0.12).
#[cfg(feature = "test-hooks")]
mod props {
    use super::*;
    use gpui_kit::base::Root;
    use gpui_kit::{Modifiers, VisualTestContext, WindowHandle};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::hub::GlobalHub;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
    use tl_ui::windows::startup_properties::{StartupPropertiesView, open_startup_properties};

    struct Rig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext, entries: Vec<StartupEntry>) -> Rig {
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
            .update(|cx| open_main_window_on(Tab::Startup, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        let mut rig = Rig {
            engine,
            main,
            version: 0,
        };
        rig.publish(entries);
        rig
    }

    impl Rig {
        fn publish(&mut self, entries: Vec<StartupEntry>) {
            self.version += 1;
            let version = self.version;
            self.engine.publish_with(|snapshot| {
                snapshot.startup = Arc::new(StartupSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    entries: entries.into(),
                    ..StartupSnapshot::default()
                });
            });
            self.main.run_until_parked();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
        }
    }

    fn named(name: &str) -> StartupEntry {
        super::entry(name, StartupKind::AutostartUser)
    }

    /// Width of `text` in the table font, as the cell fit rule measures it.
    fn advance_width(vcx: &mut VisualTestContext, text: &str) -> gpui_kit::Pixels {
        use tl_ui::text::metrics::TextMetricsCache;
        use tl_ui::theme::metrics;
        vcx.update(|window, cx| {
            let size = metrics(cx).text.base;
            let mut cache = TextMetricsCache::default();
            text.chars()
                .fold(px(0.), |w, ch| w + cache.char_advance(ch, size, window, cx))
        })
    }

    /// The longest Status and Impact words, with the dot and padding, fit their default widths.
    #[gpui_kit::test]
    fn startup_columns_fit_longest_status_and_impact(cx: &mut TestAppContext) {
        let mut rig = rig(cx, vec![named("a")]);
        // Cell padding 8 px on both sides; the Status dot is 8 px plus a 6 px gap.
        let cases = [
            ("Not in this desktop", 16. + 14., StartupColumn::Status),
            ("Missing program", 16. + 14., StartupColumn::Status),
            ("Not measured", 16., StartupColumn::Impact),
        ];
        for (word, extra, c) in cases {
            let need = advance_width(&mut rig.main, word) + px(extra);
            assert!(
                need <= px(tl_ui::pages::startup::delegate::default_width(c)),
                "{word:?} needs {need:?} in {c:?}"
            );
        }
    }

    /// The longest Properties label fits its column, so it is not cut with an ellipsis.
    #[gpui_kit::test]
    fn startup_props_flags_label_fits(cx: &mut TestAppContext) {
        use tl_ui::windows::startup_properties::LABEL_W;
        let mut rig = rig(cx, vec![named("a")]);
        let label = "Can run / Can toggle / Can delete / File exists";
        let need = advance_width(&mut rig.main, label);
        assert!(
            need <= px(LABEL_W),
            "{label:?} needs {need:?}, column {LABEL_W}"
        );
    }

    fn discord() -> StartupEntry {
        let mut e = named("Discord");
        e.publisher = Some(Arc::from("Discord Inc."));
        e.impact = Impact::Medium;
        e.impact_detail = Some(ImpactDetail {
            activation_ms: 420,
            started_after_login_ms: 3000,
            cpu_nsec: None,
            fork_exec_only: false,
        });
        e.state_code = Some(Arc::from("active (running)"));
        e.command = Arc::from("/usr/bin/discord --start-minimized");
        e.resolved_path = Some(Arc::from("/usr/bin/discord"));
        e.generated_unit = Some(Arc::from("app-Discord@autostart.service"));
        e
    }

    fn open(cx: &mut TestAppContext, entry: &StartupEntry) -> Option<WindowHandle<Root>> {
        let r = entry.id.clone();
        cx.update(|cx| {
            let hub = cx.global::<GlobalHub>().0.clone();
            open_startup_properties(r.clone(), &hub, cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let handle = registry.read(cx).get(&WindowKey::StartupProperties(r));
            handle.and_then(|h| h.downcast::<Root>())
        })
    }

    fn frame(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
    ) -> gpui_kit::Entity<TlSecondaryWindow<StartupPropertiesView>> {
        cx.update(|cx| TlSecondaryWindow::<StartupPropertiesView>::find(handle, cx))
            .expect("startup properties frame")
    }

    fn title(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
        let frame = frame(cx, handle);
        cx.update(|cx| frame.read(cx).title().to_string())
    }

    fn lines(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Vec<String> {
        let frame = frame(cx, handle);
        cx.update(|cx| {
            let view = frame.read(cx).content().clone();
            let words = tl_ui::components::kv_grid::kv_words(cx);
            view.read(cx).line_texts(&words)
        })
    }

    fn count(cx: &mut TestAppContext) -> usize {
        cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .count(CapClass::StartupProps)
        })
    }

    fn golden(name: &str, actual: &str) {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("TL_UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, actual).expect("write the golden");
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(actual, expected, "{name}");
    }

    #[gpui_kit::test]
    fn startup_props_lines_golden(cx: &mut TestAppContext) {
        let entry = discord();
        let _rig = rig(cx, vec![entry.clone()]);
        let handle = open(cx, &entry).expect("opened");
        assert_eq!(title(cx, handle), "Startup properties: Discord");
        let text = lines(cx, handle).join("\n") + "\n";
        golden("startup_props_lines.txt", &text);
    }

    #[gpui_kit::test]
    fn startup_props_optional_lines_only_when_set(cx: &mut TestAppContext) {
        let base = named("Plain");
        let _rig = rig(cx, vec![base.clone()]);
        let handle = open(cx, &base).expect("opened");
        let optional = [
            "OnlyShowIn",
            "NotShowIn",
            "TryExec",
            "Autostart phase",
            "Generated unit",
            "Overrides",
        ];
        let all = lines(cx, handle);
        for label in optional {
            assert!(
                !all.iter().any(|l| l.starts_with(&format!("{label}\t"))),
                "{label} in {all:?}"
            );
        }

        let mut full = named("Full");
        full.only_show_in = ["KDE".into(), "GNOME".into()].into_iter().collect();
        full.not_show_in = ["LXQt".into()].into_iter().collect();
        full.try_exec = Some(Arc::from("full"));
        full.autostart_phase = Some(Arc::from("Applications"));
        full.generated_unit = Some(Arc::from("app-Full@autostart.service"));
        full.overrides = Some(Arc::from("/etc/xdg/autostart/Full.desktop"));
        let mut rig = _rig;
        rig.publish(vec![base, full.clone()]);
        let handle = open(cx, &full).expect("opened");
        let all = lines(cx, handle);
        for label in optional {
            assert!(
                all.iter().any(|l| l.starts_with(&format!("{label}\t"))),
                "{label} missing in {all:?}"
            );
        }
        assert!(all.iter().any(|l| l == "OnlyShowIn\tKDE, GNOME"), "{all:?}");
    }

    #[gpui_kit::test]
    fn startup_props_removed_title(cx: &mut TestAppContext) {
        let entry = discord();
        let mut rig = rig(cx, vec![entry.clone()]);
        let handle = open(cx, &entry).expect("opened");
        let before = lines(cx, handle);
        rig.publish(Vec::new());
        cx.run_until_parked();
        assert_eq!(title(cx, handle), "Startup properties: Discord (removed)");
        assert_eq!(lines(cx, handle), before, "the last values stay");
        let frame = frame(cx, handle);
        let secondary = cx.update(|cx| {
            let view = frame.read(cx).content().clone();
            let words = tl_ui::components::kv_grid::kv_words(cx);
            let theme = current(cx).clone();
            let view = view.read(cx);
            view.is_removed()
                && view
                    .value_looks(&words, &theme)
                    .iter()
                    .all(|look| look.color == theme.text_secondary && !look.link)
        });
        assert!(secondary, "every value is text_secondary");
    }

    #[gpui_kit::test]
    fn startup_props_cap(cx: &mut TestAppContext) {
        let entries: Vec<StartupEntry> = (0..11).map(|i| named(&format!("e{i}"))).collect();
        let mut rig = rig(cx, entries.clone());
        for entry in &entries[..10] {
            assert!(open(cx, entry).is_some());
        }
        rig.main.update(|window, _| window.activate_window());
        rig.main.run_until_parked();
        assert!(open(cx, &entries[10]).is_none());
        assert_eq!(count(cx), 10);
        assert!(
            rig.toasts()
                .iter()
                .any(|t| t == "Too many startup properties windows are already open"),
            "{:?}",
            rig.toasts()
        );
    }

    #[gpui_kit::test]
    fn startup_props_same_entry_focuses(cx: &mut TestAppContext) {
        let entry = named("A");
        let _rig = rig(cx, vec![entry.clone()]);
        let first = open(cx, &entry).expect("opened");
        let again = open(cx, &entry).expect("still open");
        assert_eq!(first.window_id(), again.window_id());
        assert_eq!(count(cx), 1);
    }

    #[gpui_kit::test]
    fn startup_props_copy_line(cx: &mut TestAppContext) {
        let entry = discord();
        let _rig = rig(cx, vec![entry.clone()]);
        let handle = open(cx, &entry).expect("opened");
        let mut win = VisualTestContext::from_window(handle.into(), cx);
        win.update(|window, _| window.activate_window());
        win.run_until_parked();
        win.update(|window, cx| window.draw(cx).clear(cx));
        // Line 8 is Command.
        let bounds = win
            .debug_bounds("StartupPropsCopy-8")
            .expect("the Command copy button is drawn");
        win.simulate_click(bounds.center(), Modifiers::default());
        win.run_until_parked();
        let clipboard = win.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(
            clipboard.as_deref(),
            Some("/usr/bin/discord --start-minimized")
        );
        let frame = frame(cx, handle);
        let toasts = win.update(|window, cx| {
            let queue = tl_ui::components::toast::ToastQueue::of_window(window, cx)
                .expect("the window's toast stack");
            let _ = &frame;
            queue
                .read(cx)
                .visible_texts()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        });
        assert!(
            toasts.iter().any(|t| t == "Copied to clipboard"),
            "{toasts:?}"
        );
    }

    #[gpui_kit::test]
    fn startup_props_rerender_only_on_own_entry(cx: &mut TestAppContext) {
        let a = named("A");
        let b = named("B");
        let mut rig = rig(cx, vec![a.clone(), b.clone()]);
        let handle = open(cx, &a).expect("opened");
        let mut win = VisualTestContext::from_window(handle.into(), cx);
        win.update(|window, _| window.activate_window());
        win.run_until_parked();
        win.update(|window, cx| window.draw(cx).clear(cx));
        let frame = frame(cx, handle);
        let renders = |win: &mut VisualTestContext| {
            win.update(|_, cx| frame.read(cx).content().read(cx).redraw_count())
        };
        let before = renders(&mut win);
        let mut other = b.clone();
        other.command = Arc::from("/usr/bin/changed");
        rig.publish(vec![a.clone(), other]);
        win.run_until_parked();
        assert_eq!(renders(&mut win), before, "another entry changed");
        let mut own = a;
        own.command = Arc::from("/usr/bin/own");
        rig.publish(vec![own, b]);
        win.run_until_parked();
        assert!(renders(&mut win) > before, "its own entry changed");
    }
}

/// The Startup Apps tab inside `MainWindow`, end to end through the fake engine
/// (tasklens-9o0.1, UI-SYS-040 to UI-SYS-050, table (b)).
#[cfg(feature = "test-hooks")]
mod main_window {
    use super::*;
    use gpui_kit::AppContext as _;
    use gpui_kit::{Entity, Modifiers, MouseButton, MouseDownEvent, VisualTestContext, point, px};
    use tl_model::actions::{
        ActionEvent as FlowEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_model::snapshot::StartupCounts;
    use tl_model::table::SysTable;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::{MenuEntryInfo, TlMenuPopup, open_context_menu};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::pages::startup::StartupPage;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};
    use tl_ui::window::registry::{CapClass, GlobalRegistry};

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

        fn select_row(&mut self, ix: usize) {
            let table = self.table();
            table.update(&mut self.vcx, |table, cx| {
                let id = table.delegate.row_id(ix);
                table.selection.click(id);
                cx.notify();
            });
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
            });
            self.draw();
        }

        fn selected_names(&mut self) -> Vec<String> {
            let table = self.table();
            self.vcx.read_entity(&table, |table, _| {
                table
                    .selection
                    .ids()
                    .filter_map(|id| table.delegate.row_index(id))
                    .filter_map(|index| table.delegate.entry(index))
                    .map(|entry| entry.name.to_string())
                    .collect()
            })
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
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

        fn properties_count(&mut self) -> usize {
            self.vcx.update(|_, cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .count(CapClass::StartupProps)
            })
        }

        fn window_count(&mut self) -> usize {
            self.vcx.update(|_, cx| cx.windows().len())
        }

        fn clipboard(&mut self) -> String {
            self.vcx
                .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
                .unwrap_or_default()
        }

        fn requests(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .collect()
        }

        fn emit(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
            self.engine.emit(FlowEvent { id, target, state });
            self.vcx.run_until_parked();
        }

        fn status_cell(&mut self, ix: usize) -> String {
            let table = self.table();
            self.vcx.read_entity(&table, |table, _| {
                let mut out = CellOut::default();
                table.delegate.cell(ix, id(StartupColumn::Status), &mut out);
                text(&out).to_owned()
            })
        }

        /// Right-clicks the Name cell of row `ix`; returns the menu that opened, if any.
        fn right_click(&mut self, ix: usize) -> Option<Entity<TlMenuPopup>> {
            self.draw();
            let body = self
                .vcx
                .debug_bounds("TlTreeTable-body")
                .expect("the table body");
            let row_h = self
                .vcx
                .update(|_, cx| tl_ui::theme::metrics(cx).row_height);
            let at = point(
                body.origin.x + px(40.),
                body.origin.y + row_h * (ix as f32 + 0.5),
            );
            self.vcx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Right,
                click_count: 1,
                first_mouse: false,
            });
            self.vcx.run_until_parked();
            self.draw();
            self.vcx.update(|window, cx| open_context_menu(window, cx))
        }

        /// Runs the item labelled `label` as a click does: in the table's context.
        fn run_item(&mut self, menu: &Entity<TlMenuPopup>, label: &str) {
            let entries: Vec<MenuEntryInfo> = self
                .vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window));
            let action = entries
                .into_iter()
                .find(|e| e.label.as_deref() == Some(label))
                .and_then(|e| e.action)
                .unwrap_or_else(|| panic!("a {label} item"));
            let table = self.table();
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                window.dispatch_action(action, cx);
            });
            self.vcx.run_until_parked();
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
    fn startup_keymap(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        rig.select_row(0);

        rig.vcx.simulate_keystrokes("down");
        rig.vcx.run_until_parked();
        assert_eq!(rig.selected_names(), ["KDE Connect"], "Down moves");

        rig.vcx.simulate_keystrokes("enter");
        rig.vcx.run_until_parked();
        assert_eq!(rig.properties_count(), 1, "Enter opens Properties");
        let handle = rig.vcx.update(|_, cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let key =
                tl_ui::window::registry::WindowKey::StartupProperties(fixture()[1].id.clone());
            registry
                .read(cx)
                .get(&key)
                .expect("the Properties window is registered")
        });
        let mut props = VisualTestContext::from_window(handle, cx);
        props.update(|window, _| window.activate_window());
        props.simulate_keystrokes("escape");
        props.run_until_parked();
        assert_eq!(rig.properties_count(), 0, "Esc closes Properties");

        rig.vcx.update(|window, _| window.activate_window());
        rig.select_row(1);
        let windows = rig.window_count();
        rig.vcx.simulate_keystrokes("delete");
        rig.vcx.run_until_parked();
        assert!(
            rig.toasts()
                .iter()
                .any(|t| t == "This startup entry cannot be deleted"),
            "{:?}",
            rig.toasts()
        );
        assert_eq!(rig.window_count(), windows, "no dialog for a system entry");

        rig.vcx.simulate_keystrokes("ctrl-c");
        rig.vcx.run_until_parked();
        let row = rig.clipboard();
        assert!(!row.is_empty(), "Ctrl+C copies the row");
        rig.vcx.simulate_keystrokes("ctrl-shift-c");
        rig.vcx.run_until_parked();
        let table_text = rig.clipboard();
        assert!(!table_text.is_empty(), "Ctrl+Shift+C copies a table");
        assert_ne!(row, table_text);

        rig.vcx.simulate_keystrokes("space");
        rig.vcx.run_until_parked();
        assert_eq!(rig.selected_names(), ["KDE Connect"], "Space does nothing");
        assert_eq!(rig.window_count(), windows);

        rig.vcx.simulate_keystrokes("shift-f10");
        rig.vcx.run_until_parked();
        assert!(
            rig.vcx
                .update(|window, cx| open_context_menu(window, cx))
                .is_some(),
            "Shift+F10 opens the menu"
        );
        rig.vcx.simulate_keystrokes("escape");
        rig.vcx.run_until_parked();
        let table = rig.table();
        assert!(
            rig.vcx
                .update(|window, cx| table.read(cx).focus_handle().is_focused(window)),
            "Esc gives the keys back to the table (jqk.37)"
        );

        rig.select_row(0);
        rig.vcx.simulate_keystrokes("delete");
        rig.vcx.run_until_parked();
        assert_eq!(
            rig.window_count(),
            windows + 1,
            "Delete on a user entry opens the confirm dialog"
        );
    }

    #[gpui_kit::test]
    fn startup_end_to_end_enable(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        let ssh = fixture()[3].clone();
        let menu = rig.right_click(3).expect("the context menu opens");
        rig.run_item(&menu, "Enable");
        let sent = rig.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::StartupEnable);
        assert_eq!(
            sent[0].targets.as_slice(),
            [ActionTarget::Startup(ssh.id.clone())]
        );
        assert_eq!(rig.status_cell(3), "Updating\u{2026}");
        let action = sent[0].id;
        rig.emit(action, None, ActionState::Started);
        rig.emit(action, Some(0), ActionState::Succeeded);
        rig.emit(
            action,
            None,
            ActionState::summarize(&[ActionState::Succeeded]),
        );
        rig.draw();
        assert!(
            rig.toasts().iter().any(|t| t == "Startup entry enabled."),
            "{:?}",
            rig.toasts()
        );
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

/// Text cells draw their values, never the warming-up "…" glyph (tasklens-rsf.30).
mod drawn_text {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::base::Root;
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_ui::table::cells::{record_painted_texts, take_painted_texts};
    use tl_ui::table::tree_table::TlTreeTable;

    #[gpui_kit::test]
    fn startup_table_draws_cell_values(cx: &mut TestAppContext) {
        let (d, _) = delegate(cx, vec![entry("app", StartupKind::AutostartUser)]);
        let slot = Rc::new(RefCell::new(None));
        let keep = Rc::clone(&slot);
        let handle = cx.add_window(move |window, cx| {
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *keep.borrow_mut() = Some(table.clone());
            Root::new(table, window, cx)
        });
        let table = slot.borrow().clone().expect("table");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        record_painted_texts();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let painted = take_painted_texts();
        assert!(
            vcx.debug_bounds("TlCell-glyph").is_none(),
            "no cell of a current row draws a validity glyph"
        );

        // Each painted cell, found by its x, holds the delegate's value for its column. Name is
        // the Name cell and paints no value cell.
        let (drawn, want) = vcx.update(|_, cx| {
            let t = table.read(cx);
            let layout = t.columns();
            let mut drawn: Vec<(SysColumnId, String)> = painted
                .iter()
                .filter_map(|(bounds, text)| {
                    let col = layout.column_at_x(bounds.center().x)?;
                    Some((col, text.to_string()))
                })
                .collect();
            drawn.sort_by_key(|(col, _)| layout.visible_columns().iter().position(|c| c == col));
            let want: Vec<(SysColumnId, String)> = layout
                .visible_columns()
                .iter()
                .filter(|&&col| col != id(StartupColumn::Name))
                .map(|&col| {
                    let mut out = CellOut::default();
                    t.delegate.cell(0, col, &mut out);
                    (col, text(&out).to_owned())
                })
                .collect();
            (drawn, want)
        });
        assert_eq!(
            want.len(),
            STARTUP_COLUMNS.len() - 1,
            "every column is shown"
        );
        assert!(
            want.iter().all(|(_, w)| !w.is_empty() && w != "…"),
            "{want:?}"
        );
        assert_eq!(drawn, want);
    }
}
