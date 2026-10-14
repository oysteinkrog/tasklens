//! Startup Apps delegate tests (tasklens-9o0.6, UI-SYS-041, UI-SYS-042).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{App, TestAppContext, px};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::{
    ComponentHeader, Impact, ImpactDetail, Snapshot, StartupEntry, StartupKind, StartupSnapshot,
    StartupStatus,
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
        (StartupColumn::Status, 90., 80.),
        (StartupColumn::Impact, 100., 80.),
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
    use gpui_kit::base::Root;
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
    use tl_ui::components::toast::{ToastKey, ToastQueue};
    use tl_ui::demand::{DEMAND_COALESCE, GlobalDemand};
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
                    let hub = cx.global::<GlobalHub>().0.clone();
                    let demand = cx.global::<GlobalDemand>().0.clone();
                    let page = cx.new(|cx| StartupPage::new(&hub, &demand, client, window, cx));
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
        let hub = cx.global::<GlobalHub>().0.clone();
        let demand = cx.global::<GlobalDemand>().0.clone();
        let client = Arc::clone(&cx.global::<tl_ui::app::EngineGlobal>().0);
        let page = cx.new(|cx| StartupPage::new(&hub, &demand, client, window, cx));
        PageSlot::new(page, cx)
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
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
            self.version += 1;
            let generation = self.generation();
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
        assert_eq!(rig.engine.demands().last(), Some(&Demand::default()));
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

        rig.double_click(header);
        assert_eq!(rig.opened.get(), 0, "header double-click");

        rig.double_click(second_row);
        assert_eq!(rig.opened.get(), 1, "row double-click opens Properties");
        assert_eq!(rig.selected_names(), ["Steam"], "for the clicked row");

        rig.double_click(header);
        assert_eq!(rig.opened.get(), 1, "header double-click with a selection");

        rig.double_click(below_rows);
        assert_eq!(rig.opened.get(), 1, "empty body double-click");
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
