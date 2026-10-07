//! System tables: status cell looks and painter (E27, UI-SYS-003, SYS-003, SYS-020, SYS-029).

use std::sync::Arc;

use gpui_kit::{
    App, Background, Context, Hsla, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::i18n::init_strings;
use tl_ui::pages::sys_status::{
    ConnStateClass, DotStyle, LabelTone, STALE_DOT_ALPHA, StatusLook, SysStatus, cell_colors,
    conn_state_tone, status_cell, status_label, status_look, tone_color,
};
use tl_ui::theme::{TlTheme, UiMetrics, current, metrics};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    let theme = ThemeSettings {
        active: "Default Dark".to_owned(),
        ..ThemeSettings::default()
    };
    tl_ui::theme::store::init(cx, &theme, &AppearanceSettings::default(), Vec::new());
    init_strings(cx, "en-US");
}

fn theme(cx: &mut TestAppContext) -> Arc<TlTheme> {
    cx.update(|cx| {
        setup(cx);
        assert_eq!(&*current(cx).name, "Default Dark");
        current(cx).clone()
    })
}

/// The bead's table: (status, dot, label tone, label text).
const TABLE: [(SysStatus, DotStyle, LabelTone, &str); 13] = {
    use DotStyle::{Filled, Ring};
    use LabelTone::{Danger, Info, Primary, Secondary, Success, Warning};
    [
        (SysStatus::Running, Filled(Success), Primary, "Running"),
        (SysStatus::Active, Ring(Success), Primary, "Active"),
        (SysStatus::Starting, Filled(Info), Primary, "Starting"),
        (SysStatus::Stopping, Filled(Info), Primary, "Stopping"),
        (SysStatus::Reloading, Filled(Info), Primary, "Reloading"),
        (SysStatus::Stopped, Ring(Secondary), Secondary, "Stopped"),
        (SysStatus::Failed, Filled(Danger), Danger, "Failed"),
        (
            SysStatus::Maintenance,
            Filled(Warning),
            Primary,
            "Maintenance",
        ),
        (SysStatus::Enabled, Filled(Success), Primary, "Enabled"),
        (SysStatus::Disabled, Ring(Secondary), Secondary, "Disabled"),
        (SysStatus::Masked, Filled(Warning), Primary, "Masked"),
        (
            SysStatus::NotInThisDesktop,
            Ring(Secondary),
            Secondary,
            "Not in this desktop",
        ),
        (
            SysStatus::MissingProgram,
            Filled(Warning),
            Primary,
            "Missing program",
        ),
    ]
};

#[gpui_kit::test]
fn status_cell_colors_per_state(cx: &mut TestAppContext) {
    let theme = theme(cx);
    assert_eq!(SysStatus::ALL.len(), TABLE.len());
    for (s, dot, label, text) in TABLE {
        let look = status_look(s);
        assert_eq!(look, StatusLook { dot, label }, "{s:?}");
        assert_ne!(
            look.label,
            LabelTone::Success,
            "{s:?}: Success is a dot tone only"
        );
        assert_eq!(cx.update(|cx| status_label(cx, s)), text, "{s:?}");
    }
    let t = &*theme;
    let named = [
        (LabelTone::Primary, t.text_primary, t.text_selected_primary),
        (
            LabelTone::Secondary,
            t.text_secondary,
            t.text_selected_secondary,
        ),
        (LabelTone::Danger, t.status.danger, t.status.danger),
        (LabelTone::Warning, t.status.warning, t.status.warning),
        (LabelTone::Info, t.status.info, t.status.info),
        (LabelTone::Success, t.status.success, t.status.success),
    ];
    for (tone, plain, selected) in named {
        assert_eq!(tone_color(tone, t, false), plain, "{tone:?}");
        assert_eq!(tone_color(tone, t, true), selected, "{tone:?} selected");
    }
}

#[gpui_kit::test]
fn status_cell_selected_keeps_danger(cx: &mut TestAppContext) {
    let theme = theme(cx);
    let failed = status_look(SysStatus::Failed);
    assert_eq!(tone_color(failed.label, &theme, true), theme.status.danger);
    assert_ne!(theme.status.danger, theme.text_selected_primary);

    let running = cell_colors(status_look(SysStatus::Running), true, false, &theme);
    assert_eq!(running.label, theme.text_selected_primary);
    assert_eq!(
        running.dot,
        Some(theme.status.success),
        "the dot keeps its colour"
    );
}

struct Host {
    look: StatusLook,
    stale: bool,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m: UiMetrics = *metrics(cx);
        let label = status_label(cx, SysStatus::Running);
        div().size(px(400.)).child(
            div()
                .w(px(160.))
                .child(status_cell(self.look, label, false, self.stale, &theme, &m)),
        )
    }
}

fn open(cx: &mut TestAppContext, host: Host) -> &mut VisualTestContext {
    cx.update(setup);
    let (_, vcx) = cx.add_window_view(|_, _| host);
    vcx.run_until_parked();
    vcx
}

/// Painted quads filled with `color`.
fn fills(cx: &mut VisualTestContext, color: Hsla) -> usize {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|q| q.background == Background::from(color))
            .count()
    })
}

#[gpui_kit::test]
fn status_cell_stale_dims(cx: &mut TestAppContext) {
    let theme = theme(cx);
    let look = status_look(SysStatus::Running);
    let colors = cell_colors(look, false, true, &theme);
    assert_eq!(colors.label, theme.text_secondary);
    let dot = colors.dot.expect("Running has a dot");
    assert_eq!(dot.a, theme.status.success.a * STALE_DOT_ALPHA);
    assert_eq!(
        cell_colors(look, true, true, &theme).label,
        theme.text_secondary,
        "stale wins over selected"
    );

    let vcx = open(cx, Host { look, stale: true });
    assert!(vcx.debug_bounds("sys-status-dot").is_some());
    assert!(vcx.debug_bounds("sys-status-label").is_some());
    assert_eq!(fills(vcx, dot), 1, "the dot is painted at half alpha");
    assert_eq!(fills(vcx, theme.status.success), 0, "no full-alpha dot");
}

#[gpui_kit::test]
fn status_cell_fresh_dot_full_alpha(cx: &mut TestAppContext) {
    let theme = theme(cx);
    let look = status_look(SysStatus::Running);
    let vcx = open(cx, Host { look, stale: false });
    assert_eq!(fills(vcx, theme.status.success), 1);
}

#[gpui_kit::test]
fn status_cell_no_dot(cx: &mut TestAppContext) {
    let look = StatusLook {
        dot: DotStyle::None,
        label: conn_state_tone(ConnStateClass::Listening),
    };
    let vcx = open(cx, Host { look, stale: false });
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(vcx.debug_bounds("sys-status-dot").is_none(), "no dot child");
    let cell = vcx.debug_bounds("sys-status-cell").expect("cell drawn");
    let label = vcx.debug_bounds("sys-status-label").expect("label drawn");
    assert_eq!(
        label.origin.x, cell.origin.x,
        "the label starts at the left"
    );
}

#[gpui_kit::test]
fn status_cell_dot_then_gap(cx: &mut TestAppContext) {
    let look = status_look(SysStatus::Active);
    let vcx = open(cx, Host { look, stale: false });
    let m = vcx.update(|_, cx| *metrics(cx));
    let dot = vcx.debug_bounds("sys-status-dot").expect("dot drawn");
    let label = vcx.debug_bounds("sys-status-label").expect("label drawn");
    assert_eq!(dot.size.width, px(8.0 * m.ui_scale));
    assert_eq!(label.origin.x, dot.origin.x + dot.size.width + m.sp[3]);
}

#[test]
fn conn_state_tones() {
    assert_eq!(conn_state_tone(ConnStateClass::Listening), LabelTone::Info);
    assert_eq!(
        conn_state_tone(ConnStateClass::Established),
        LabelTone::Primary
    );
    assert_eq!(
        conn_state_tone(ConnStateClass::Transitional),
        LabelTone::Primary
    );
    assert_eq!(
        conn_state_tone(ConnStateClass::TimeWaitOrClosed),
        LabelTone::Secondary
    );
    assert_eq!(conn_state_tone(ConnStateClass::Unknown), LabelTone::Warning);
    assert_eq!(conn_state_tone(ConnStateClass::Udp), LabelTone::Secondary);
}

#[cfg(feature = "test-hooks")]
mod words {
    use gpui_kit::TestAppContext;
    use tl_ui::i18n::{set_test_catalogue, tr_calls};
    use tl_ui::pages::sys_status::{SysStatus, status_label};

    const MARKERS: &str = "sys-sys003-running = [Running]\nsys-sys003-failed = [Failed]\n";

    #[gpui_kit::test]
    fn status_labels_once_per_strings_version(cx: &mut TestAppContext) {
        cx.update(|cx| {
            super::setup(cx);
            assert_eq!(status_label(cx, SysStatus::Running), "Running");
            let before = tr_calls();
            for _ in 0..60 {
                for s in SysStatus::ALL {
                    let _ = status_label(cx, s);
                }
            }
            assert_eq!(tr_calls(), before, "60 frames make no tr call");

            set_test_catalogue(cx, MARKERS);
            assert_eq!(status_label(cx, SysStatus::Running), "[Running]");
            assert_eq!(status_label(cx, SysStatus::Failed), "[Failed]");
            assert_eq!(
                status_label(cx, SysStatus::Masked),
                "Masked",
                "English fallback"
            );
            assert!(tr_calls() > before, "a new strings version translates");
        });
    }
}

/// `SysQuery` and `SysPageState` (tasklens-hk6.6).
#[cfg(feature = "test-hooks")]
mod query_and_demand {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, TestAppContext};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::demand::Demand;
    use tl_model::health::SourceId;
    use tl_model::table::{StartupColumn, SysColumnId, SysSortSpec, SysTable, SysTableQuery};
    use tl_model::validity::Validity;
    use tl_ui::app::EngineGlobal;
    use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::sys_table::{SysPageState, SysQuery};
    use tl_ui::testing::fake_engine::FakeEngine;

    fn query() -> SysQuery {
        let sort = SysSortSpec {
            column: SysColumnId::Startup(StartupColumn::Name),
            ascending: true,
        };
        SysQuery::new(SysTable::Startup, sort, None)
    }

    fn sent(engine: &FakeEngine) -> Vec<SysTableQuery> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::SetSysTableQuery(q) => Some(q),
                _ => None,
            })
            .collect()
    }

    fn refreshes(engine: &FakeEngine) -> Vec<SourceId> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::RefreshNow(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    /// `EngineGlobal` and an aggregator over a new `FakeEngine`.
    fn rig(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Entity<DemandAggregator>) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let agg = cx.update(|cx| {
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            cx.new(|cx| DemandAggregator::new(client, &hub, cx))
        });
        (engine, agg)
    }

    /// Lets the aggregator's coalesced flush run and returns the last demand sent.
    fn flushed(cx: &mut TestAppContext, engine: &FakeEngine) -> Demand {
        cx.executor().advance_clock(DEMAND_COALESCE);
        cx.run_until_parked();
        engine.demands().last().cloned().unwrap_or_default()
    }

    #[test]
    fn sys_query_sent_on_filter_changed() {
        let engine = FakeEngine::new();
        let mut q = query();
        for _ in 0..3 {
            q.resend(&*engine);
        }
        assert_eq!(q.last_sent(), 3);

        q.set_filter("SSH", &*engine);

        let all = sent(&engine);
        assert_eq!(all.len(), 4);
        let last = all.last().expect("one more query");
        assert_eq!(&*last.filter, "ssh");
        assert_eq!(last.generation, 4);
        assert_eq!(q.last_sent(), 4);
        assert_eq!(q.query(), last);
    }

    #[test]
    fn sys_query_ignores_stale_generation() {
        let engine = FakeEngine::new();
        let mut q = query();
        assert!(q.accepts(0), "nothing sent yet");
        for _ in 0..4 {
            q.resend(&*engine);
        }
        assert!(!q.accepts(3));
        assert!(!q.accepts(5));
        assert!(q.accepts(4));
    }

    #[test]
    fn sys_query_resends_after_busy() {
        let engine = FakeEngine::new();
        let mut q = query();
        q.resend(&*engine);
        assert_eq!(sent(&engine).len(), 1);

        engine.set_busy(true);
        q.set_filter("x", &*engine);
        engine.set_busy(false);
        assert_eq!(sent(&engine).len(), 1, "busy records nothing");
        assert_eq!(q.last_sent(), 2, "a busy generation still counts");
        assert!(!q.accepts(1), "the older view is refused");

        q.on_snapshot(&*engine);
        q.on_snapshot(&*engine);
        let all = sent(&engine);
        assert_eq!(all.len(), 2, "exactly one resend");
        assert_eq!(all[1].generation, 3);
        assert_eq!(&*all[1].filter, "x");
        assert!(q.accepts(3));
    }

    #[gpui_kit::test]
    fn sys_page_sets_demand_only_while_visible(cx: &mut TestAppContext) {
        let (engine, agg) = rig(cx);
        let mut startup = SysPageState::new(Tab::Startup, &[SourceId::Startup]);
        let mut conns = SysPageState::new(Tab::Connections, &[SourceId::Connections]);
        let (mut q1, mut q2) = (query(), query());

        cx.update(|cx| startup.on_activated(&mut q1, &agg, cx));
        let d = flushed(cx, &engine);
        assert!(d.any_surface_visible && d.startup);
        assert!(!d.services && !d.connections);
        assert!(startup.visible);

        cx.update(|cx| startup.on_deactivated(&agg, cx));
        let d = flushed(cx, &engine);
        assert!(!d.startup && !d.any_surface_visible);
        assert!(!startup.visible);

        cx.update(|cx| conns.on_activated(&mut q2, &agg, cx));
        let d = flushed(cx, &engine);
        assert!(d.any_surface_visible && d.connections);
        assert!(!d.startup && !d.services);
    }

    #[gpui_kit::test]
    fn sys_page_refresh_now_first_visit_only(cx: &mut TestAppContext) {
        let (engine, agg) = rig(cx);
        let mut page = SysPageState::new(
            Tab::Services,
            &[SourceId::SystemdSystem, SourceId::SystemdUser],
        );
        let mut q = query();

        cx.update(|cx| page.on_activated(&mut q, &agg, cx));
        assert_eq!(
            refreshes(&engine),
            [SourceId::SystemdSystem, SourceId::SystemdUser]
        );
        assert_eq!(sent(&engine).len(), 1, "one query on the first visit");
        assert!(page.first_visit_done);
        assert!(flushed(cx, &engine).services);

        cx.update(|cx| page.on_deactivated(&agg, cx));
        cx.update(|cx| page.on_activated(&mut q, &agg, cx));
        assert_eq!(
            refreshes(&engine).len(),
            2,
            "no RefreshNow on the second visit"
        );
        assert_eq!(sent(&engine).len(), 1);
        assert!(flushed(cx, &engine).services, "demand set again");
    }

    #[test]
    fn sys_page_first_data_seen_sticks() {
        let mut page = SysPageState::new(Tab::Startup, &[SourceId::Startup]);
        assert!(!page.note_data(Validity::WarmingUp, 0));
        assert!(page.note_data(Validity::Valid, 0));
        assert!(!page.note_data(Validity::WarmingUp, 0));
        assert!(page.first_data_seen);

        let mut rows = SysPageState::new(Tab::Startup, &[SourceId::Startup]);
        assert!(rows.note_data(Validity::WarmingUp, 3), "rows count as data");
    }
}

/// Body states: skeleton, rows, empty and error states (tasklens-hk6.7).
#[cfg(feature = "test-hooks")]
mod body_states {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div,
        px,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::health::{SourceHealth, SourceId, SourceState};
    use tl_model::table::{SysNodeId, SysRow};
    use tl_model::validity::Validity;
    use tl_ui::actions::{ClearFilter, RefreshCurrentTab};
    use tl_ui::pages::sys_table::{
        SysBody, SysBodyInput, SysEmptyButton, SysPageState, render_empty, skeleton_rows, sys_body,
        sys_empty_content,
    };
    use tl_ui::testing::fake_engine::FakeEngine;

    fn input(tab: Tab, first: bool, rows: usize) -> SysBodyInput<'static> {
        SysBodyInput {
            tab,
            first_data_seen: first,
            rows_len: rows,
            filter_active: false,
            source: None,
            managers_unavailable: false,
        }
    }

    fn failed(error: &str) -> SourceHealth {
        SourceHealth {
            state: SourceState::Failed,
            last_error: Some(Arc::from(error)),
            ..SourceHealth::default()
        }
    }

    #[test]
    fn sys_body_rules_order() {
        let ok = SourceHealth {
            state: SourceState::Running,
            ..SourceHealth::default()
        };
        let bad = failed("bus timeout\nsecond line");

        // 1. Failed before the first data, one line of the error.
        let i = SysBodyInput {
            source: Some(&bad),
            ..input(Tab::Startup, false, 0)
        };
        assert_eq!(
            sys_body(&i),
            SysBody::FailedFirst {
                error: "bus timeout".into()
            }
        );
        // 2. No data yet with a healthy source.
        let i = SysBodyInput {
            source: Some(&ok),
            ..input(Tab::Startup, false, 0)
        };
        assert_eq!(sys_body(&i), SysBody::Skeleton);
        // 3. Services with both managers gone, ahead of the filter rules.
        let i = SysBodyInput {
            managers_unavailable: true,
            filter_active: true,
            ..input(Tab::Services, true, 0)
        };
        assert_eq!(sys_body(&i), SysBody::ManagerMissing);
        let i = SysBodyInput {
            managers_unavailable: true,
            ..input(Tab::Startup, true, 0)
        };
        assert_eq!(sys_body(&i), SysBody::NoEntries, "Startup has no managers");
        // 4. No rows under a filter.
        let i = SysBodyInput {
            filter_active: true,
            ..input(Tab::Connections, true, 0)
        };
        assert_eq!(sys_body(&i), SysBody::NoMatches);
        // 5. No rows, no filter; Services keeps its rows.
        assert_eq!(sys_body(&input(Tab::Startup, true, 0)), SysBody::NoEntries);
        assert_eq!(sys_body(&input(Tab::Services, true, 0)), SysBody::Rows);
        // 6. Rows, and a source that fails after the first data keeps them.
        assert_eq!(sys_body(&input(Tab::Connections, true, 5)), SysBody::Rows);
        let i = SysBodyInput {
            source: Some(&bad),
            ..input(Tab::Connections, true, 5)
        };
        assert_eq!(sys_body(&i), SysBody::Rows);
        let i = SysBodyInput {
            source: Some(&bad),
            ..input(Tab::Startup, true, 0)
        };
        assert_eq!(
            sys_body(&i),
            SysBody::NoEntries,
            "no error state after data"
        );
    }

    fn row(n: u32) -> SysRow {
        SysRow {
            node: SysNodeId::Conn(u64::from(n)),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(n),
        }
    }

    #[test]
    fn skeleton_shows_once_per_session() {
        let engine = FakeEngine::new();
        let mut page = SysPageState::new(Tab::Startup, &[SourceId::Startup]);
        let body = |engine: &FakeEngine, page: &mut SysPageState| {
            let snap = engine.latest();
            let (validity, rows) = (snap.startup.header.validity, snap.startup.rows.len());
            page.note_data(validity, rows);
            sys_body(&input(Tab::Startup, page.first_data_seen, rows))
        };

        // An empty WarmingUp component.
        assert_eq!(body(&engine, &mut page), SysBody::Skeleton);
        engine.publish_with(|s| {
            let st = Arc::make_mut(&mut s.startup);
            st.header.validity = Validity::Valid;
            st.rows = vec![row(0), row(1)].into();
        });
        assert_eq!(body(&engine, &mut page), SysBody::Rows);
        // Empty and WarmingUp again (an engine restart). `first_data_seen` never resets, so
        // leaving and entering the tab cannot bring the skeleton back either.
        engine.publish_with(|s| {
            let st = Arc::make_mut(&mut s.startup);
            st.header.validity = Validity::WarmingUp;
            st.rows = Vec::new().into();
        });
        assert_eq!(body(&engine, &mut page), SysBody::NoEntries);
    }

    #[test]
    fn skeleton_row_count_capped_at_12() {
        assert_eq!(skeleton_rows(px(1000.), px(28.)), 12);
        assert_eq!(skeleton_rows(px(100.), px(28.)), 3);
        assert_eq!(skeleton_rows(px(27.), px(28.)), 0);
        assert_eq!(skeleton_rows(px(100.), px(0.)), 0);
    }

    type Row = (
        SysBody,
        Tab,
        &'static str,
        Option<&'static str>,
        Option<(&'static str, SysEmptyButton)>,
    );

    #[gpui_kit::test]
    fn empty_state_strings_per_tab(cx: &mut TestAppContext) {
        use SysEmptyButton::{Clear, Retry};
        let error = || SysBody::FailedFirst {
            error: "bus timeout".into(),
        };
        let table: [Row; 9] = [
            (
                SysBody::ManagerMissing,
                Tab::Services,
                "systemd not found",
                Some(
                    "tasklens lists services from systemd. Neither the system manager nor the \
                     user manager answered.",
                ),
                Some(("Retry", Retry)),
            ),
            (
                SysBody::NoMatches,
                Tab::Services,
                "No matches",
                Some("No services match the current filters."),
                Some(("Clear filters", Clear)),
            ),
            (
                SysBody::NoMatches,
                Tab::Startup,
                "No matches",
                Some("No startup entries match the filter."),
                Some(("Clear filter", Clear)),
            ),
            (
                SysBody::NoMatches,
                Tab::Connections,
                "No matches",
                Some("No connections match the filter."),
                Some(("Clear filter", Clear)),
            ),
            (
                SysBody::NoEntries,
                Tab::Startup,
                "No startup entries",
                Some("Nothing starts automatically when you log in."),
                None,
            ),
            (
                SysBody::NoEntries,
                Tab::Connections,
                "No connections",
                None,
                None,
            ),
            (
                error(),
                Tab::Services,
                "Could not load services",
                Some("bus timeout"),
                Some(("Retry", Retry)),
            ),
            (
                error(),
                Tab::Startup,
                "Could not load startup entries",
                Some("bus timeout"),
                Some(("Retry", Retry)),
            ),
            (
                error(),
                Tab::Connections,
                "Could not load connections",
                Some("bus timeout"),
                Some(("Retry", Retry)),
            ),
        ];
        cx.update(|cx| {
            super::setup(cx);
            for (body, tab, heading, text, button) in table {
                let c = sys_empty_content(&body, tab, cx).expect("an empty state");
                assert_eq!(&*c.heading, heading, "{body:?} {tab:?}");
                assert_eq!(c.text.as_deref(), text, "{body:?} {tab:?}");
                let got = c.button.as_ref().map(|(l, b)| (&**l, *b));
                assert_eq!(got, button, "{body:?} {tab:?}");
                assert_eq!(c.warning, matches!(body, SysBody::FailedFirst { .. }));
            }
            assert!(sys_empty_content(&SysBody::Rows, Tab::Startup, cx).is_none());
            assert!(sys_empty_content(&SysBody::Skeleton, Tab::Startup, cx).is_none());
            assert!(sys_empty_content(&SysBody::NoEntries, Tab::Services, cx).is_none());
        });
    }

    /// One `render_empty` under a focused root that counts the two actions.
    struct Host {
        body: SysBody,
        tab: Tab,
        clears: Rc<Cell<usize>>,
        refreshes: Rc<Cell<usize>>,
        focus: FocusHandle,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (clears, refreshes) = (self.clears.clone(), self.refreshes.clone());
            div()
                .id("root")
                .track_focus(&self.focus)
                .size(px(600.))
                .on_action(move |_: &ClearFilter, _, _| clears.set(clears.get() + 1))
                .on_action(move |_: &RefreshCurrentTab, _, _| refreshes.set(refreshes.get() + 1))
                .child(render_empty(
                    &self.body,
                    self.tab,
                    Box::new(ClearFilter),
                    cx,
                ))
        }
    }

    /// Clicks the empty-state button of `body` on `tab`; returns (ClearFilter, RefreshCurrentTab)
    /// counts.
    fn click(cx: &mut TestAppContext, body: SysBody, tab: Tab) -> (usize, usize) {
        cx.update(super::setup);
        let clears = Rc::new(Cell::new(0));
        let refreshes = Rc::new(Cell::new(0));
        let (_, vcx): (_, &mut VisualTestContext) = cx.add_window_view({
            let (clears, refreshes) = (clears.clone(), refreshes.clone());
            move |window, cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Host {
                    body,
                    tab,
                    clears,
                    refreshes,
                    focus,
                }
            }
        });
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            vcx.debug_bounds("SysEmpty-heading").is_some(),
            "heading drawn"
        );
        let button = vcx
            .debug_bounds("TlButton-SysEmpty-button")
            .expect("button drawn");
        vcx.simulate_click(button.center(), Modifiers::none());
        vcx.run_until_parked();
        (clears.get(), refreshes.get())
    }

    #[gpui_kit::test]
    fn startup_no_matches_button_clears_filter(cx: &mut TestAppContext) {
        let first = input(Tab::Startup, true, 0);
        let body = sys_body(&SysBodyInput {
            filter_active: true,
            ..first
        });
        assert_eq!(body, SysBody::NoMatches);
        assert_eq!(click(cx, body, Tab::Startup), (1, 0));
    }

    #[gpui_kit::test]
    fn services_systemd_missing_state(cx: &mut TestAppContext) {
        let body = sys_body(&SysBodyInput {
            managers_unavailable: true,
            ..input(Tab::Services, true, 0)
        });
        assert_eq!(body, SysBody::ManagerMissing);
        let heading = cx.update(|cx| {
            super::setup(cx);
            sys_empty_content(&body, Tab::Services, cx)
                .expect("an empty state")
                .heading
        });
        assert_eq!(&*heading, "systemd not found");
        assert_eq!(click(cx, body, Tab::Services), (0, 1), "Retry refreshes");
    }
}
