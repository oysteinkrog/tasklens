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
    use tl_ui::host::HostContext;
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

        cx.update(|cx| startup.on_activated(&HostContext::local(cx), &mut q1, &agg, cx));
        let d = flushed(cx, &engine);
        assert!(d.any_surface_visible && d.startup);
        assert!(!d.services && !d.connections);
        assert!(startup.visible);

        cx.update(|cx| startup.on_deactivated(&agg, cx));
        let d = flushed(cx, &engine);
        assert!(!d.startup && !d.any_surface_visible);
        assert!(!startup.visible);

        cx.update(|cx| conns.on_activated(&HostContext::local(cx), &mut q2, &agg, cx));
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

        cx.update(|cx| page.on_activated(&HostContext::local(cx), &mut q, &agg, cx));
        assert_eq!(
            refreshes(&engine),
            [SourceId::SystemdSystem, SourceId::SystemdUser]
        );
        assert_eq!(sent(&engine).len(), 1, "one query on the first visit");
        assert!(page.first_visit_done);
        assert!(flushed(cx, &engine).services);

        cx.update(|cx| page.on_deactivated(&agg, cx));
        cx.update(|cx| page.on_activated(&HostContext::local(cx), &mut q, &agg, cx));
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
        Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
        Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px,
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
            vcx.debug_bounds("TlEmptyState-heading").is_some(),
            "heading drawn"
        );
        let button = vcx
            .debug_bounds("TlButton-TlEmptyState-clear-filter")
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

/// System-table action texts (tasklens-hk6.9).
mod action_texts {
    use std::sync::Arc;

    use tl_model::actions::ActionError;
    use tl_ui::action_tracker::message;
    use tl_ui::pages::sys_table::failure_text;

    const PREFIX: &str = "Failed to start service";

    fn job(result: &str, error_name: Option<&str>) -> ActionError {
        ActionError::UnitJob {
            result: Arc::from(result),
            error_name: error_name.map(Arc::from),
        }
    }

    #[test]
    fn failure_text_maps_dbus_errors() {
        assert_eq!(
            failure_text(PREFIX, &ActionError::PermissionDenied),
            "Failed to start service: not authorized"
        );
        assert_eq!(
            failure_text(
                PREFIX,
                &job("failed", Some("org.freedesktop.DBus.Error.AccessDenied"))
            ),
            "Failed to start service: not authorized"
        );
        assert_eq!(
            failure_text(
                PREFIX,
                &job(
                    "failed",
                    Some("org.freedesktop.DBus.Error.InteractiveAuthorizationRequired")
                )
            ),
            "Failed to start service: no authentication agent is running"
        );
        assert_eq!(
            failure_text(PREFIX, &job("timeout", None)),
            "Failed to start service: timeout"
        );
        assert_eq!(
            failure_text(
                PREFIX,
                &job("failed", Some("org.freedesktop.systemd1.NoSuchUnit"))
            ),
            "Failed to start service: org.freedesktop.systemd1.NoSuchUnit",
            "an unmapped D-Bus error name is shown as is"
        );
        let internal = ActionError::Internal(Arc::from("bus closed"));
        assert_eq!(
            failure_text(PREFIX, &internal),
            format!("{PREFIX}: {}", message(&internal))
        );
    }
}

/// `SysActions`: system-table action toasts and the pending map (tasklens-hk6.9).
#[cfg(feature = "test-hooks")]
mod sys_actions {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use smallvec::SmallVec;
    use tl_model::actions::{
        ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::table::{Manager, UnitRef};
    use tl_ui::action_tracker::{ActionTracker, PushedToast, ToastTexts};
    use tl_ui::components::toast::{self, ToastKey, ToastKind, ToastQueue};
    use tl_ui::pages::sys_table::{ActionKindTag, ActionTexts, SysActions, SysActionsEvent};
    use tl_ui::testing::fake_engine::FakeEngine;

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("content")
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        queue: Entity<ToastQueue>,
        tracker: Entity<ActionTracker>,
        actions: Entity<SysActions>,
        events: Rc<RefCell<Vec<SysActionsEvent>>>,
    }

    impl Rig {
        fn submit(
            &mut self,
            tag: ActionKindTag,
            kind: ActionKind,
            targets: impl IntoIterator<Item = ActionTarget>,
            texts: ActionTexts,
        ) -> Option<ActionId> {
            let targets: SmallVec<[ActionTarget; 1]> = targets.into_iter().collect();
            let actions = self.actions.clone();
            let id = self.vcx.update(|window, cx| {
                actions.update(cx, |a, cx| a.submit(kind, tag, targets, texts, window, cx))
            });
            self.vcx.run_until_parked();
            id
        }

        fn emit(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
            self.engine.emit(ActionEvent { id, target, state });
            self.vcx.run_until_parked();
        }

        /// `Started`, one state per target, then the summary of those states.
        fn finish(&mut self, id: ActionId, states: &[ActionState]) {
            self.emit(id, None, ActionState::Started);
            for (i, state) in states.iter().enumerate() {
                self.emit(id, Some(i as u16), state.clone());
            }
            self.emit(id, None, ActionState::summarize(states));
        }

        fn pushed(&mut self) -> Vec<PushedToast> {
            self.actions
                .read_with(&self.vcx, |a, _| a.pushed().to_vec())
        }

        fn visible(&mut self) -> Vec<String> {
            self.queue.read_with(&self.vcx, |q, _| {
                q.visible_texts().iter().map(|t| t.to_string()).collect()
            })
        }

        fn pending(&mut self, target: &ActionTarget, tag: ActionKindTag) -> bool {
            self.actions
                .read_with(&self.vcx, |a, _| a.is_pending(target, tag))
        }

        fn actions_sent(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .collect()
        }
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        cx.update(|cx| {
            super::setup(cx);
            toast::init(cx);
            cx.set_reduce_motion(true);
        });
        let engine = FakeEngine::new();
        let window = cx.add_window(|window, cx| {
            let host = cx.new(|_| Host);
            Root::new(host, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        let client: Arc<dyn EngineClient> = engine.clone();
        let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, None, cx)));
        let actions = vcx.update(|_, cx| cx.new(|cx| SysActions::new(&tracker, cx)));
        let events: Rc<RefCell<Vec<SysActionsEvent>>> = Rc::default();
        let sink = events.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&actions, move |_, ev: &SysActionsEvent, _| {
                sink.borrow_mut().push(ev.clone())
            })
            .detach();
        });
        vcx.run_until_parked();
        Rig {
            engine,
            vcx,
            queue,
            tracker,
            actions,
            events,
        }
    }

    fn unit(name: &str) -> ActionTarget {
        ActionTarget::Unit(UnitRef {
            manager: Manager::System,
            name: name.into(),
        })
    }

    fn units(n: usize) -> Vec<ActionTarget> {
        (0..n).map(|i| unit(&format!("u{i}.service"))).collect()
    }

    fn start_texts() -> ActionTexts {
        ActionTexts {
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
        }
    }

    fn start(r: &mut Rig, targets: Vec<ActionTarget>) -> ActionId {
        r.submit(
            ActionKindTag::Start,
            ActionKind::UnitStart,
            targets,
            start_texts(),
        )
        .expect("sent")
    }

    const FAIL: ActionState = ActionState::Failed(ActionError::PermissionDenied);

    #[gpui_kit::test]
    fn action_toast_replaced_in_place(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let id = start(&mut r, vec![unit("sshd.service")]);
        assert_eq!(r.visible(), ["Starting service..."]);
        r.emit(id, None, ActionState::Started);
        assert_eq!(r.visible(), ["Starting service..."]);
        r.emit(id, Some(0), ActionState::Succeeded);
        r.emit(id, None, ActionState::Succeeded);

        assert_eq!(r.visible(), ["Service started"], "one toast throughout");
        let pushed = r.pushed();
        assert!(pushed.iter().all(|t| t.key == Some(ToastKey(id.0))));
        assert_eq!(pushed.last().map(|t| t.kind), Some(ToastKind::Success));
    }

    #[gpui_kit::test]
    fn failed_toast_reads_the_dbus_reason(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let id = start(&mut r, vec![unit("sshd.service")]);
        let err = ActionError::UnitJob {
            result: "failed".into(),
            error_name: Some("org.freedesktop.DBus.Error.InteractiveAuthorizationRequired".into()),
        };
        r.finish(id, &[ActionState::Failed(err)]);
        assert_eq!(
            r.visible(),
            ["Failed to start service: no authentication agent is running"]
        );
        assert_eq!(r.pushed().last().map(|t| t.kind), Some(ToastKind::Error));
    }

    #[gpui_kit::test]
    fn multi_target_summary_toast_at_4(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let id = start(&mut r, units(6));
        let pushed = r.pushed();
        assert_eq!(pushed.len(), 1, "no per-target toast");
        assert_eq!(pushed[0].kind, ToastKind::Progress);
        assert_eq!(&*pushed[0].text, "Starting 6 services...");
        assert_eq!(pushed[0].key, Some(ToastKey(id.0)));

        let ok = ActionState::Succeeded;
        r.finish(
            id,
            &[ok.clone(), FAIL, ok.clone(), FAIL, ok.clone(), ok.clone()],
        );
        let last = r.pushed().pop().expect("summary");
        assert_eq!(last.kind, ToastKind::Warning);
        assert_eq!(&*last.text, "4 of 6 services started. 2 failed.");
        assert_eq!(last.button.as_deref(), Some("Details"));
        assert_eq!(
            r.pushed().len(),
            2,
            "only the summary replaced the progress toast"
        );
        let lines = r
            .actions
            .read_with(&r.vcx, |a, _| a.details_lines(id))
            .expect("details kept");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("u1.service: "), "{lines:?}");
        assert!(lines[1].starts_with("u3.service: "), "{lines:?}");
    }

    #[gpui_kit::test]
    fn already_running_rejection_is_an_info_toast(cx: &mut TestAppContext) {
        use tl_model::actions::RejectReason;
        let mut r = rig(cx);
        let id = start(&mut r, vec![unit("sshd.service")]);
        r.finish(id, &[ActionState::Rejected(RejectReason::AlreadyRunning)]);
        assert_eq!(r.visible(), ["Already running"]);
        assert_eq!(r.pushed().last().map(|t| t.kind), Some(ToastKind::Info));
    }

    #[gpui_kit::test]
    fn three_targets_three_toasts(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let id = start(&mut r, units(3));
        let pushed = r.pushed();
        assert_eq!(pushed.len(), 3);
        let keys: std::collections::HashSet<_> = pushed.iter().map(|t| t.key).collect();
        assert_eq!(keys.len(), 3, "distinct keys");
        assert!(!keys.contains(&Some(ToastKey(id.0))));
        r.finish(id, &[ActionState::Succeeded, FAIL, ActionState::Succeeded]);
        let mut visible = r.visible();
        visible.sort();
        assert_eq!(
            visible,
            [
                "Failed to start service: not authorized",
                "Service started",
                "Service started"
            ]
        );
    }

    #[gpui_kit::test]
    fn pending_target_disables_same_action(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let sshd = unit("sshd.service");
        start(&mut r, vec![sshd.clone()]);
        assert!(r.pending(&sshd, ActionKindTag::Start));
        assert!(!r.pending(&sshd, ActionKindTag::Stop));

        let again = r.submit(
            ActionKindTag::Start,
            ActionKind::UnitStart,
            [sshd.clone()],
            start_texts(),
        );
        assert_eq!(again, None);
        assert_eq!(r.actions_sent().len(), 1, "no second request");
        let last = r.pushed().pop().expect("busy toast");
        assert_eq!(last.kind, ToastKind::Warning);
        assert_eq!(&*last.text, "A service action is already in progress.");
    }

    #[gpui_kit::test]
    fn empty_pending_text_pushes_no_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let texts = ActionTexts {
            pending_one: "".into(),
            failed_prefix: "Could not open the link".into(),
            failed_suffix: false,
            ..start_texts()
        };
        let id = r
            .submit(
                ActionKindTag::Other,
                ActionKind::OpenUri("https://example.org".into()),
                [],
                texts,
            )
            .expect("sent");
        assert!(r.pushed().is_empty());
        r.emit(id, None, ActionState::Started);
        assert!(r.visible().is_empty(), "nothing until the final event");
        r.emit(id, None, ActionState::Failed(ActionError::Unsupported));
        assert_eq!(r.visible(), ["Could not open the link"], "prefix alone");
    }

    #[gpui_kit::test]
    fn empty_done_text_no_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let texts = ActionTexts {
            done_one: "".into(),
            ..start_texts()
        };
        let id = r
            .submit(
                ActionKindTag::StartupRunNow,
                ActionKind::StartupRunNow,
                [unit("a.service")],
                texts,
            )
            .expect("sent");
        assert_eq!(r.visible(), ["Starting service..."]);
        r.finish(id, &[ActionState::Succeeded]);
        assert!(r.visible().is_empty(), "the toast is removed");
    }

    #[gpui_kit::test]
    fn service_job_progress_has_no_cap(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let sshd = unit("sshd.service");
        let id = start(&mut r, vec![sshd.clone()]);
        r.emit(id, None, ActionState::Started);
        r.vcx.executor().advance_clock(Duration::from_secs(130));
        r.vcx.run_until_parked();
        assert_eq!(r.visible(), ["Starting service..."]);
        assert_eq!(r.pushed().len(), 1);
        assert_eq!(r.pushed()[0].kind, ToastKind::Progress);
        assert!(r.pending(&sshd, ActionKindTag::Start));
        assert_eq!(r.tracker.read_with(&r.vcx, |t, _| t.pending_len()), 1);
    }

    #[gpui_kit::test]
    fn sys_actions_follow_tracker_events_by_id(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let sshd = unit("sshd.service");
        let mine = start(&mut r, vec![sshd.clone()]);
        r.events.borrow_mut().clear();

        // Another owner's request on the same unit, through the tracker directly.
        let queue = r.queue.downgrade();
        let tracker = r.tracker.clone();
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::UnitStop,
            targets: [sshd.clone()].into_iter().collect(),
            privileged: false,
        };
        let other = r
            .vcx
            .update(|_, cx| {
                tracker.update(cx, |t, cx| {
                    t.track(request, queue, ToastTexts::new("Stopped", "No."), cx)
                })
            })
            .expect("tracked");
        r.vcx.run_until_parked();
        r.finish(other, &[ActionState::Succeeded]);
        assert!(
            r.pending(&sshd, ActionKindTag::Start),
            "other id: no change"
        );
        assert!(r.events.borrow().is_empty());

        r.finish(mine, &[ActionState::Succeeded]);
        assert!(!r.pending(&sshd, ActionKindTag::Start));
        assert_eq!(
            *r.events.borrow(),
            [SysActionsEvent::PendingChanged { target: sshd }]
        );
    }
}

// `ToastQueue::visible_texts` is a test hook.
#[cfg(feature = "test-hooks")]
mod copy {
    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
        Styled as _, TestAppContext, VisualTestContext, Window, div, px,
    };
    use smallvec::SmallVec;
    use tl_model::ids::ColumnId;
    use tl_ui::components::toast::{self, ToastQueue};
    use tl_ui::pages::sys_copy::{
        CopyTexts, SysCopySource, copied_toast_text, copy_cell, copy_row, copy_rows_as_table,
    };

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("content")
        }
    }

    /// One row: id, "Copy row" block, Name cell, PID cell.
    type Row = (u32, &'static str, &'static str, &'static str);

    /// A page stand-in. `groups` are group row ids, which `copy_rows` drops.
    struct Source {
        rows: Vec<Row>,
        selected: Vec<u32>,
        groups: Vec<u32>,
        texts: CopyTexts,
    }

    impl Source {
        fn new(rows: Vec<Row>, selected: Vec<u32>) -> Self {
            Self {
                rows,
                selected,
                groups: Vec::new(),
                texts: CopyTexts {
                    row_ok: "Row copied".into(),
                    row_failed: "Copy failed".into(),
                    nothing_selected: "Select a row first.".into(),
                },
            }
        }

        fn row(&self, id: u32) -> &Row {
            self.rows.iter().find(|r| r.0 == id).expect("row")
        }
    }

    impl SysCopySource for Source {
        type RowId = u32;

        fn copy_rows(&self) -> SmallVec<[u32; 8]> {
            self.selected
                .iter()
                .copied()
                .filter(|id| !self.groups.contains(id))
                .collect()
        }

        fn write_block(&self, row: u32, out: &mut String) {
            out.push_str(self.row(row).1);
        }

        fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
            SmallVec::from_iter([
                ("Name".into(), ColumnId::Name),
                ("PID".into(), ColumnId::Pid),
            ])
        }

        fn write_cell(&self, row: u32, col: ColumnId, out: &mut String) {
            let row = self.row(row);
            let text = if col == ColumnId::Name { row.2 } else { row.3 };
            // The validity glyph cell writes nothing.
            if text != "\u{2026}" {
                out.push_str(text);
            }
        }

        fn copy_texts(&self) -> &CopyTexts {
            &self.texts
        }
    }

    struct Rig {
        vcx: VisualTestContext,
        queue: Entity<ToastQueue>,
    }

    impl Rig {
        fn run(&mut self, f: impl FnOnce(&mut Window, &mut gpui_kit::App)) {
            self.vcx.update(f);
            self.vcx.run_until_parked();
        }

        fn clipboard(&mut self) -> Option<String> {
            self.vcx
                .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        }

        fn toasts(&mut self) -> Vec<String> {
            self.queue.read_with(&self.vcx, |q, _| {
                q.visible_texts().iter().map(|t| t.to_string()).collect()
            })
        }
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        cx.update(|cx| {
            super::setup(cx);
            toast::init(cx);
            cx.set_reduce_motion(true);
        });
        let window = cx.add_window(|window, cx| {
            let host = cx.new(|_| Host);
            Root::new(host, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()))
        });
        vcx.run_until_parked();
        Rig { vcx, queue }
    }

    #[gpui_kit::test]
    fn copy_row_blocks_joined_by_blank_line(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let src = Source::new(vec![(1, "A", "a", "1"), (2, "B", "b", "2")], vec![1, 2]);
        r.run(|window, cx| copy_row(&src, window, cx));
        assert_eq!(r.clipboard().as_deref(), Some("A\n\nB"));
        assert_eq!(r.toasts(), ["Row copied"]);
    }

    #[gpui_kit::test]
    fn copy_nothing_selected_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let src = Source::new(vec![(1, "A", "a", "1")], Vec::new());
        r.run(|window, cx| copy_row(&src, window, cx));
        assert_eq!(
            r.clipboard().as_deref(),
            Some("before"),
            "clipboard unchanged"
        );
        assert_eq!(r.toasts(), ["Select a row first."]);
        r.run(|window, cx| copy_rows_as_table(&src, window, cx));
        assert_eq!(r.clipboard().as_deref(), Some("before"));
        assert_eq!(r.toasts(), ["Select a row first. (×2)"]);
    }

    #[gpui_kit::test]
    fn copy_rows_as_table_tsv_header(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let src = Source::new(vec![(1, "A", "sshd.service", "812")], vec![1]);
        r.run(|window, cx| copy_rows_as_table(&src, window, cx));
        assert_eq!(
            r.clipboard().as_deref(),
            Some("Name\tPID\nsshd.service\t812")
        );
        assert_eq!(r.toasts(), ["Row copied"]);

        // Rows in view order, a tab inside a value becomes a space, a glyph cell is empty.
        let src = Source::new(
            vec![(1, "A", "b\tc", "\u{2026}"), (2, "B", "a", "7")],
            vec![2, 1],
        );
        r.run(|window, cx| copy_rows_as_table(&src, window, cx));
        assert_eq!(r.clipboard().as_deref(), Some("Name\tPID\na\t7\nb c\t"));
    }

    #[gpui_kit::test]
    fn copy_cell_truncates_toast_at_60(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let value: &'static str = "v".repeat(75).leak();
        let src = Source::new(vec![(1, "A", value, "1")], Vec::new());
        r.run(|window, cx| copy_cell(&src, 1, ColumnId::Name, window, cx));
        assert_eq!(r.clipboard().as_deref(), Some(value), "all 75 chars");
        let expected = format!("Copied: {}\u{2026}", "v".repeat(60));
        assert_eq!(r.toasts(), [expected.as_str()]);
        assert_eq!(copied_toast_text(value).as_ref(), expected);
    }

    #[gpui_kit::test]
    fn copy_skips_group_rows(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let mut src = Source::new(
            vec![
                (1, "A", "a", "1"),
                (9, "GROUP", "g", ""),
                (2, "B", "b", "2"),
            ],
            vec![1, 9, 2],
        );
        src.groups = vec![9];
        r.run(|window, cx| copy_row(&src, window, cx));
        assert_eq!(r.clipboard().as_deref(), Some("A\n\nB"));
    }
}

#[cfg(feature = "test-hooks")]
mod frame {
    use std::sync::Arc;

    use gpui_kit::{
        AnyView, App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _,
        Render, SharedString, StyleRefinement, Styled as _, Subscription, TestAppContext,
        VisualTestContext, Window, WindowBounds, WindowOptions, div, point, px, size,
    };
    use smallvec::SmallVec;
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::health::SourceId;
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{ComponentHeader, ComponentMask, Snapshot, StartupSnapshot};
    use tl_model::table::{StartupColumn, SysColumnId, SysNodeId, SysRow, SysSortSpec};
    use tl_model::validity::Validity;
    use tl_ui::actions::ClearFilter;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::TlMenu;
    use tl_ui::host::HostContext;
    use tl_ui::hub::{GlobalHub, SnapshotChanged};
    use tl_ui::pages::sys_table::{SysBody, SysDelegate, SysTableFrame, loading_status_text};
    use tl_ui::table::actions::TABLE_CONTEXT;
    use tl_ui::table::cells::CellOut;
    use tl_ui::table::columns::{
        Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
    };
    use tl_ui::table::tree_table::{
        HEADER_H_ONE, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable,
        TreeTableDelegate,
    };
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{GlobalShellEvents, PauseChanged};
    use tl_ui::window::status::CHIP_H;

    const NAME: SysColumnId = SysColumnId::Startup(StartupColumn::Name);
    const CONTEXT: &str = "StartupTable";

    /// A Startup table whose rows are the `Conn` ids of `Snapshot.startup.rows`.
    struct TestDelegate {
        specs: Vec<ColumnSpec<SysColumnId>>,
        layout: ColumnLayout<SysColumnId>,
        rows: Vec<u32>,
        version: u64,
    }

    impl TestDelegate {
        fn new() -> Self {
            let specs = vec![ColumnSpec {
                id: NAME,
                label: "Name".into(),
                full_name: "Name".into(),
                group: "Name".into(),
                align: ColumnAlign::Left,
                kind: ColumnKind::Text,
                sortable: true,
                template: None,
                default_width: px(300.),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }];
            Self {
                layout: ColumnLayout::defaults(TableId::Startup, &specs, &[NAME], &[NAME]),
                specs,
                rows: Vec::new(),
                version: 0,
            }
        }
    }

    impl TreeTableDelegate for TestDelegate {
        type RowId = u32;
        type Column = SysColumnId;
        type Sort = SysSortSpec;

        fn row_count(&self) -> usize {
            self.rows.len()
        }
        fn row_id(&self, ix: usize) -> u32 {
            self.rows[ix]
        }
        fn row_index(&self, id: u32) -> Option<usize> {
            self.rows.iter().position(|r| *r == id)
        }
        fn row_kind(&self, _: usize) -> RowKind {
            RowKind::Plain
        }
        fn row_depth(&self, _: usize) -> u8 {
            0
        }
        fn row_expandable(&self, _: usize) -> Option<bool> {
            None
        }
        fn row_flags(&self, _: usize) -> RowFlags {
            RowFlags::default()
        }
        fn table_version(&self) -> u64 {
            self.version
        }
        fn columns(&self) -> &ColumnLayout<SysColumnId> {
            &self.layout
        }
        fn column_spec(&self, _: SysColumnId) -> &ColumnSpec<SysColumnId> {
            &self.specs[0]
        }
        fn cell(&self, ix: usize, _: SysColumnId, out: &mut CellOut) {
            out.bits = self.rows[ix] as u64;
        }
        fn name_cell(&self, _: usize) -> NameCell {
            NameCell {
                icon: NameIcon::None,
                icon_size: IconSlot::Small,
                primary: SharedString::new_static("entry"),
                count_run: None,
                secondary: None,
                glyphs: SmallVec::new(),
                highlight_fffd: true,
                trailing: None,
            }
        }
        fn header_total(&self, _: SysColumnId, _: &mut CellOut) -> bool {
            false
        }
        fn sort(&self) -> SysSortSpec {
            SysSortSpec {
                column: NAME,
                ascending: true,
            }
        }
        fn selectable(&self, _: usize) -> bool {
            true
        }
        fn context_menu(
            &mut self,
            _: &[u32],
            _: Option<SysColumnId>,
            menu: TlMenu,
            _: &mut Window,
            _: &mut Context<TlTreeTable<Self>>,
        ) -> TlMenu {
            menu
        }
        fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
        fn on_sort(&mut self, _: SysColumnId, _: &mut Context<TlTreeTable<Self>>) {}
        fn on_columns_changed(
            &mut self,
            _: &ColumnLayout<SysColumnId>,
            _: &mut Context<TlTreeTable<Self>>,
        ) {
        }
        fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
        fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
        fn row_tooltip(&self, _: usize) -> Option<AnyView> {
            None
        }
        fn empty_text(&self) -> Option<SharedString> {
            None
        }
        fn header_lines(&self) -> HeaderLines {
            HeaderLines::One
        }
    }

    impl SysDelegate for TestDelegate {
        fn component_header<'a>(&self, snap: &'a Snapshot) -> &'a ComponentHeader {
            &snap.startup.header
        }
        fn rows_len(&self, snap: &Snapshot) -> usize {
            snap.startup.rows.len()
        }
        fn query_generation(&self, snap: &Snapshot) -> u64 {
            snap.startup.query_generation
        }
        fn install(&mut self, snap: &Arc<Snapshot>) {
            self.rows = snap
                .startup
                .rows
                .iter()
                .map(|r| match r.node {
                    SysNodeId::Conn(n) => n as u32,
                    _ => 0,
                })
                .collect();
            self.version += 1;
        }
        fn mask(&self) -> ComponentMask {
            ComponentMask::STARTUP
        }
    }

    /// A page built on the frame, as Startup Apps is.
    struct TestPage {
        frame: SysTableFrame<TestDelegate>,
        status: String,
        _subs: Vec<Subscription>,
    }

    impl TestPage {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let sort = SysSortSpec {
                column: NAME,
                ascending: true,
            };
            let host = HostContext::local(cx);
            let mut frame = SysTableFrame::new(
                &host,
                Tab::Startup,
                CONTEXT,
                &[SourceId::Startup],
                TestDelegate::new(),
                sort,
                None,
                window,
                cx,
            );
            frame.on_activated(cx);
            let hub = cx.global::<GlobalHub>().0.clone();
            let events = cx.global::<GlobalShellEvents>().0.clone();
            let subs = vec![
                cx.subscribe(&hub, |this: &mut Self, hub, ev: &SnapshotChanged, cx| {
                    let snap = Arc::clone(hub.read(cx).current());
                    if this.frame.on_snapshot(&snap, ev.mask, cx) {
                        cx.notify();
                    }
                }),
                cx.subscribe(&events, |this: &mut Self, _, ev: &PauseChanged, cx| {
                    if this.frame.set_paused(ev.0, cx) {
                        cx.notify();
                    }
                }),
            ];
            Self {
                frame,
                status: String::new(),
                _subs: subs,
            }
        }

        fn status_text(&self, out: &mut String, cx: &App) {
            if self.frame.body() == SysBody::Skeleton {
                out.push_str(&loading_status_text(cx));
            } else {
                out.push_str(&self.status);
            }
        }
    }

    impl Render for TestPage {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.frame.render(None, Box::new(ClearFilter), window, cx)
        }
    }

    /// The window root. It embeds the page cached, as `MainWindow` does, so a window draw
    /// renders the page only when the page notified.
    struct Host {
        page: Entity<TestPage>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                AnyView::from(self.page.clone()).cached(StyleRefinement::default().size_full()),
            )
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<TestPage>,
    }

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
            cx.set_reduce_motion(true);
        });
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1280.), px(800.)),
            })),
            ..WindowOptions::default()
        };
        let handle = cx
            .update(|cx| {
                cx.open_window(options, |window, cx| {
                    let page = cx.new(|cx| TestPage::new(window, cx));
                    cx.new(|_| Host { page })
                })
            })
            .expect("the window opens");
        let host = handle.root(cx).expect("the host");
        let page = cx.read(|cx| host.read(cx).page.clone());
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let mut r = Rig { engine, vcx, page };
        r.draw();
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
            self.vcx.run_until_parked();
        }

        /// The generation of the last query the frame sent.
        fn generation(&mut self) -> u64 {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).frame.query.last_sent())
        }

        /// Publishes a Startup snapshot of `rows` built for query `generation`.
        fn publish(&mut self, rows: &[u64], generation: u64) {
            let rows: Vec<SysRow> = rows
                .iter()
                .map(|n| SysRow {
                    node: SysNodeId::Conn(*n),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: None,
                })
                .collect();
            self.engine.publish_with(|s| {
                let version = s.startup.header.version + 1;
                s.startup = Arc::new(StartupSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: rows.into(),
                    query_generation: generation,
                    ..StartupSnapshot::clone(&s.startup)
                });
            });
            self.vcx.run_until_parked();
            self.draw();
        }

        fn rows(&mut self) -> Vec<u32> {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).frame.table.read(cx).delegate.rows.clone())
        }

        fn renders(&mut self) -> u64 {
            let page = self.page.clone();
            self.vcx.update(|_, cx| page.read(cx).frame.render_count())
        }

        fn pause(&mut self, on: bool) {
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| cx.emit(PauseChanged(on)));
            });
            self.vcx.run_until_parked();
            self.draw();
        }
    }

    #[gpui_kit::test]
    fn sys_frame_layout_header_one_line(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let g = r.generation();
        r.publish(&[1, 2, 3], g);
        assert_eq!(r.rows(), [1, 2, 3]);
        let header = r
            .vcx
            .debug_bounds("TlTreeTableHeader")
            .expect("header drawn");
        assert_eq!(header.size.height, px(HEADER_H_ONE));
        let table = r.vcx.debug_bounds("TlTreeTable").expect("table drawn");
        let slot = r
            .vcx
            .debug_bounds("SysTableFrame-table")
            .expect("frame drawn");
        assert_eq!(slot.size.height, px(800.), "the frame fills the window");
        assert_eq!(
            slot.bottom() - table.bottom(),
            px(CHIP_H),
            "status chip padding"
        );
        let page = r.page.clone();
        let built = r.vcx.update(|_, cx| {
            page.read(cx)
                .frame
                .table
                .read(cx)
                .rows_built_last_frame
                .get()
        });
        assert_eq!(built, 3);
        assert!(r.vcx.debug_bounds("SysTableFrame-cover").is_none());
    }

    #[gpui_kit::test]
    fn sys_page_not_rerendered_for_unrelated_mask(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let g = r.generation();
        r.publish(&[1, 2, 3], g);
        let before = r.renders();
        let page = r.page.clone();
        let notify = r.vcx.update(|_, cx| {
            let snap = Arc::clone(cx.global::<GlobalHub>().0.read(cx).current());
            page.update(cx, |p, cx| {
                p.frame.on_snapshot(&snap, ComponentMask::SYSTEM, cx)
            })
        });
        assert!(!notify);
        r.engine.publish_with(|s| s.system = Arc::default());
        r.vcx.run_until_parked();
        r.draw();
        assert_eq!(
            r.renders(),
            before,
            "a SYSTEM-only change does not render the page"
        );
    }

    #[gpui_kit::test]
    fn sys_frame_keeps_rows_for_stale_generation(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let g = r.generation();
        assert!(g >= 1, "the first visit sends the query");
        r.publish(&[1, 2, 3], g);
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.update(cx, |p, cx| p.frame.on_filter_changed("x", cx)));
        assert_eq!(r.generation(), g + 1);
        r.publish(&[7], g);
        assert_eq!(r.rows(), [1, 2, 3], "a view for the old query is not shown");
        r.publish(&[7], g + 1);
        assert_eq!(r.rows(), [7]);
    }

    #[gpui_kit::test]
    fn sys_frame_paused_keeps_rows(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let g = r.generation();
        r.publish(&[1, 2, 3], g);
        r.pause(true);
        r.publish(&[4, 5], g);
        assert_eq!(r.rows(), [1, 2, 3], "no install while Ctrl is held");
        let page = r.page.clone();
        assert!(
            r.vcx
                .update(|_, cx| page.read(cx).frame.table.read(cx).paused)
        );
        r.pause(false);
        assert_eq!(r.rows(), [4, 5], "the latest snapshot on release");
        assert!(
            !r.vcx
                .update(|_, cx| page.read(cx).frame.table.read(cx).paused)
        );
    }

    #[gpui_kit::test]
    fn sys_frame_loading_status_text(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let page = r.page.clone();
        let (body, text) = r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let mut out = String::new();
            p.status_text(&mut out, cx);
            (p.frame.body(), out)
        });
        assert_eq!(body, SysBody::Skeleton);
        assert_eq!(text, "Loading…");
        assert!(
            r.vcx.debug_bounds("TlTreeTableHeader").is_some(),
            "the header shows over the skeleton"
        );
        let cover = r
            .vcx
            .debug_bounds("SysTableFrame-cover")
            .expect("skeleton cover");
        assert_eq!(cover.top(), px(HEADER_H_ONE));
        let g = r.generation();
        r.publish(&[1], g);
        assert_eq!(
            r.vcx.update(|_, cx| page.read(cx).frame.body()),
            SysBody::Rows
        );
        assert!(r.vcx.debug_bounds("SysTableFrame-cover").is_none());
    }

    #[gpui_kit::test]
    fn sys_frame_key_context_nesting(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).frame.table.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        });
        r.draw();
        let stack = r.vcx.update(|window, _| window.context_stack());
        let at = |name: &str| stack.iter().position(|c| c.contains(name));
        let page_at = at(CONTEXT).expect("the page context");
        let table_at = at(TABLE_CONTEXT).expect("the Table context");
        assert!(page_at < table_at, "the frame wraps the table");
    }
}
