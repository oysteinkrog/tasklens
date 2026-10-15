//! Additional Setup window (E29-A3, UI-SHL-094, UI-PRF-052). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, RaplStatus, SetupState, SetupStatus};
use tl_model::settings::{Settings, UiState};
use tl_ui::actions::OpenAdditionalSetup;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::badge::TlBadgeKind;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::i18n::tr;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::setup::{CardView, SetupRow, SetupWindow, row_status};

struct Parent;

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("parent")
    }
}

type Frame = TlSecondaryWindow<SetupWindow>;

fn status(rapl: RaplStatus, journal_group: bool, helper_installed: bool) -> SetupStatus {
    SetupStatus {
        rapl,
        journal_group,
        helper_installed,
        lm_sensors: false,
        hwmon_fan: false,
        sensors_conf: false,
    }
}

fn publish(engine: &FakeEngine, st: Option<SetupStatus>) {
    engine.publish_with(|s| {
        let mut health = (*s.health).clone();
        health.version += 1;
        health.setup = Arc::new(SetupState {
            status: st,
            run: None,
        });
        s.health = Arc::new(health);
    });
}

fn setup(cx: &mut TestAppContext, reduce_motion: bool) -> Arc<FakeEngine> {
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
        cx.set_reduce_motion(reduce_motion);
    });
    cx.run_until_parked();
    engine
}

fn parent(cx: &mut TestAppContext) -> VisualTestContext {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    VisualTestContext::from_window(parent, cx)
}

fn dispatch(pcx: &mut VisualTestContext, focus_row: Option<SetupRow>) {
    pcx.update(|window, _| window.activate_window());
    pcx.dispatch_action(OpenAdditionalSetup { focus_row });
    pcx.run_until_parked();
}

fn handle(cx: &mut TestAppContext) -> WindowHandle<Root> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::AdditionalSetup)
    })
    .and_then(|h| h.downcast::<Root>())
    .expect("setup window registered")
}

fn frame(cx: &mut TestAppContext) -> Entity<Frame> {
    let h = handle(cx);
    cx.update(|cx| Frame::find(h, cx)).expect("setup frame")
}

fn cards(cx: &mut TestAppContext) -> Vec<CardView> {
    let frame = frame(cx);
    cx.update(|cx| frame.read(cx).content().read(cx).cards(cx))
}

fn flashing(cx: &mut TestAppContext) -> Option<SetupRow> {
    let frame = frame(cx);
    cx.update(|cx| frame.read(cx).content().read(cx).flashing())
}

fn status_commands(engine: &FakeEngine) -> usize {
    engine
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::SetupStatus))
        .count()
}

fn pairs(cards: &[CardView]) -> Vec<(String, Option<String>)> {
    cards
        .iter()
        .map(|c| (c.title.clone(), c.status.clone()))
        .collect()
}

fn pair(title: &str, status: Option<&str>) -> (String, Option<String>) {
    (title.to_owned(), status.map(str::to_owned))
}

#[gpui_kit::test]
fn setup_rows_order_and_statuses(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    publish(
        &engine,
        Some(status(
            RaplStatus::Readable {
                tasklens_rule: false,
            },
            false,
            true,
        )),
    );
    cx.run_until_parked();
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    assert_eq!(
        pairs(&cards(cx)),
        vec![
            pair("CPU power draw", Some("Enabled (existing rule)")),
            pair("Journal access", Some("Not enabled")),
            pair("Administrator helper", Some("Installed")),
            pair("Per-process network", Some("Not needed")),
        ]
    );
    let first = cards(cx).remove(0);
    assert_eq!(
        first.tooltip.as_deref(),
        Some(
            "Readable through /etc/udev/rules.d/99-powercap.rules. tasklens leaves that file alone."
        )
    );
}

#[gpui_kit::test]
fn setup_open_sends_status_once(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    assert_eq!(status_commands(&engine), 1);
}

#[gpui_kit::test]
fn setup_reopen_focuses_and_scrolls(cx: &mut TestAppContext) {
    let engine = setup(cx, false);
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    let before = cx.update(|cx| cx.windows().len());
    dispatch(&mut pcx, Some(SetupRow::AdminHelper));
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before,
        "no second window"
    );
    let id = handle(cx).window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the setup window has focus"
    );
    assert_eq!(status_commands(&engine), 2);
    // The flash marks the card that was scrolled to.
    assert_eq!(flashing(cx), Some(SetupRow::AdminHelper));
}

#[gpui_kit::test]
fn setup_rapl_no_zone_not_needed(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    publish(&engine, Some(status(RaplStatus::NoZone, true, true)));
    cx.run_until_parked();
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    assert_eq!(cards(cx)[0].status.as_deref(), Some("Not needed"));
}

#[gpui_kit::test]
fn setup_helper_missing_tooltip(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    publish(&engine, Some(status(RaplStatus::NotReadable, false, false)));
    cx.run_until_parked();
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    let helper = cards(cx).remove(2);
    assert_eq!(helper.status.as_deref(), Some("Not installed"));
    assert_eq!(
        helper.tooltip.as_deref(),
        Some("/usr/lib/tasklens/tl-helper was not found")
    );
    let st = status(RaplStatus::NotReadable, false, false);
    let badge = row_status(SetupRow::AdminHelper, Some(&st)).expect("status");
    assert_eq!(badge.kind, TlBadgeKind::Warning);
    let key = badge.tooltip_key.expect("tooltip key");
    assert_eq!(
        cx.update(|cx| tr(cx, key).to_string()),
        "/usr/lib/tasklens/tl-helper was not found"
    );
}

#[gpui_kit::test]
fn setup_status_spinner_before_answer(cx: &mut TestAppContext) {
    setup(cx, true);
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    let all = cards(cx);
    assert_eq!(all.len(), 4);
    assert!(all.iter().all(|c| c.status.is_none()), "{all:?}");
    for row in [
        SetupRow::CpuPower,
        SetupRow::JournalAccess,
        SetupRow::AdminHelper,
        SetupRow::ProcessNetwork,
    ] {
        assert!(row_status(row, None).is_none());
    }
}

#[gpui_kit::test]
fn setup_focus_flash_respects_reduced_motion(cx: &mut TestAppContext) {
    setup(cx, true);
    let mut pcx = parent(cx);
    dispatch(&mut pcx, Some(SetupRow::JournalAccess));
    assert_eq!(flashing(cx), None);
}
