//! Additional Setup window (E29-A3, UI-SHL-094, UI-PRF-052). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::command::{
    Command, RaplStatus, SetupPhase, SetupRun, SetupState, SetupStatus, SetupStep,
};
use tl_model::health::SourceId;
use tl_model::settings::{Settings, UiState};
use tl_ui::actions::OpenAdditionalSetup;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::badge::TlBadgeKind;
use tl_ui::components::dialog::{DialogButton, TlDialogView};
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::i18n::tr;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::setup::{
    CardView, SETUP_SCRIPT, ScriptWindow, SetupRow, SetupWindow, confirm_words, open_script_window,
    row_status,
};

use super::common::{toasts, window_count};

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
            pair("Fan sensors", Some("Not available")),
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
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|c| c.status.is_none()), "{all:?}");
    for row in [
        SetupRow::CpuPower,
        SetupRow::JournalAccess,
        SetupRow::AdminHelper,
        SetupRow::FanSensors,
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

fn publish_run(engine: &FakeEngine, st: SetupStatus, run: Option<SetupRun>) {
    engine.publish_with(|s| {
        let mut health = (*s.health).clone();
        health.version += 1;
        health.setup = Arc::new(SetupState {
            status: Some(st),
            run,
        });
        s.health = Arc::new(health);
    });
}

fn run(seq: u64, step: SetupStep, phase: SetupPhase) -> SetupRun {
    SetupRun {
        seq,
        step,
        phase,
        output: Arc::from(""),
    }
}

fn not_readable() -> SetupStatus {
    status(RaplStatus::NotReadable, true, true)
}

fn tasklens_rule() -> SetupStatus {
    status(
        RaplStatus::Readable {
            tasklens_rule: true,
        },
        true,
        true,
    )
}

fn fan(hwmon_fan: bool, lm_sensors: bool, sensors_conf: bool) -> SetupStatus {
    SetupStatus {
        hwmon_fan,
        lm_sensors,
        sensors_conf,
        ..status(RaplStatus::NoZone, true, true)
    }
}

fn run_setup_count(engine: &FakeEngine, step: SetupStep) -> usize {
    engine
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::RunSetup { step: s } if *s == step))
        .count()
}

fn sensors_reprobe_count(engine: &FakeEngine) -> usize {
    engine
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::ReprobeSource(SourceId::Sensors)))
        .count()
}

fn reprobe_count(engine: &FakeEngine) -> usize {
    engine
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::ReprobeSource(SourceId::Cpu)))
        .count()
}

/// The setup window's own context, active.
fn setup_vcx(cx: &mut TestAppContext) -> VisualTestContext {
    let mut vcx = VisualTestContext::from_window(handle(cx).into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx
}

fn content(cx: &mut TestAppContext) -> Entity<SetupWindow> {
    let frame = frame(cx);
    cx.update(|cx| frame.read(cx).content().clone())
}

/// Opens the setup window with `st` published, and returns its context.
fn open_with(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
    st: SetupStatus,
    run: Option<SetupRun>,
) -> VisualTestContext {
    publish_run(engine, st, run);
    cx.run_until_parked();
    let mut pcx = parent(cx);
    dispatch(&mut pcx, None);
    setup_vcx(cx)
}

/// The dialog window that is not the setup window or the parent.
fn dialog_view(cx: &mut TestAppContext) -> (Entity<TlDialogView>, VisualTestContext) {
    let setup_id = handle(cx).window_id();
    let h = cx
        .update(|cx| {
            cx.windows()
                .into_iter()
                .filter(|w| w.window_id() != setup_id)
                .find_map(|w| {
                    let root = w.downcast::<Root>()?;
                    TlSecondaryWindow::<TlDialogView>::find(root, cx).map(|_| root)
                })
        })
        .expect("dialog window");
    let view = cx
        .update(|cx| TlSecondaryWindow::<TlDialogView>::find(h, cx))
        .map(|f| cx.read(|cx| f.read(cx).content().clone()))
        .expect("dialog view");
    let mut vcx = VisualTestContext::from_window(h.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    (view, vcx)
}

fn press(cx: &mut TestAppContext, vcx: &mut VisualTestContext, enable: bool) {
    let w = content(cx);
    vcx.update(|window, cx| {
        w.update(cx, |w, cx| {
            if enable {
                w.press_enable(window, cx)
            } else {
                w.press_revert(window, cx)
            }
        })
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn setup_enable_confirm_text(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, not_readable(), None);
    assert_eq!(cards(cx)[0].buttons[0], ("Enable".to_owned(), false));
    press(cx, &mut vcx, true);
    let words = cx.update(|cx| confirm_words(SetupStep::RaplEnable, cx));
    assert_eq!(words.heading.as_ref(), "Enable CPU power draw?");
    assert_eq!(
        words.text.as_ref(),
        "Lets every local user read CPU energy counters. This can leak information about other programs (CVE-2020-8694)."
    );
    assert_eq!(words.action.as_ref(), "Enable");
    let (view, mut dvcx) = dialog_view(cx);
    assert_eq!(
        cx.update(|cx| view.read(cx).dialog().text().to_string()),
        words.text.to_string()
    );
    assert_eq!(
        cx.update(|cx| view.read(cx).dialog().cancel_text().to_string()),
        "Cancel"
    );
    // R-104: a non-destructive dialog starts on its action button.
    assert_eq!(
        dvcx.update(|window, cx| view.read(cx).focused_button(window)),
        Some(DialogButton::Action)
    );
    assert_eq!(run_setup_count(&engine, SetupStep::RaplEnable), 0);
}

#[gpui_kit::test]
fn setup_enable_sends_run_setup(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, not_readable(), None);
    press(cx, &mut vcx, true);
    let (_, mut dvcx) = dialog_view(cx);
    dvcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(run_setup_count(&engine, SetupStep::RaplEnable), 1);
    // A busy engine leaves the card idle.
    engine.set_busy(true);
    press(cx, &mut vcx, true);
    let (_, mut dvcx) = dialog_view(cx);
    dvcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(run_setup_count(&engine, SetupStep::RaplEnable), 1);
    assert_eq!(cards(cx)[0].failure, None);
    assert_eq!(cards(cx)[0].buttons[0], ("Enable".to_owned(), false));
}

#[gpui_kit::test]
fn setup_running_disables_buttons(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    open_with(
        cx,
        &engine,
        not_readable(),
        Some(run(1, SetupStep::RaplEnable, SetupPhase::Running)),
    );
    let card = cards(cx).remove(0);
    assert_eq!(card.status.as_deref(), Some("Running…"));
    assert_eq!(card.buttons[0], ("Enable".to_owned(), true));
}

#[gpui_kit::test]
fn setup_success_reprobes_once(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    open_with(
        cx,
        &engine,
        not_readable(),
        Some(run(1, SetupStep::RaplEnable, SetupPhase::Running)),
    );
    for _ in 0..2 {
        publish_run(
            &engine,
            not_readable(),
            Some(run(1, SetupStep::RaplEnable, SetupPhase::Succeeded)),
        );
        cx.run_until_parked();
    }
    assert_eq!(
        cards(cx)[0].done.as_deref(),
        Some("Done. No restart needed.")
    );
    assert_eq!(reprobe_count(&engine), 1);
}

#[gpui_kit::test]
fn setup_failure_block_shows_output(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, not_readable(), None);
    publish_run(
        &engine,
        not_readable(),
        Some(run(
            1,
            SetupStep::RaplEnable,
            SetupPhase::Failed(Arc::from("udevadm: not found\n")),
        )),
    );
    cx.run_until_parked();
    assert_eq!(
        cards(cx)[0].failure.as_deref(),
        Some("udevadm: not found\n")
    );
    let w = content(cx);
    vcx.update(|_, cx| w.update(cx, |w, cx| w.retry(cx)));
    assert_eq!(run_setup_count(&engine, SetupStep::RaplEnable), 1);
    vcx.update(|window, cx| w.update(cx, |w, cx| w.copy_output(window, cx)));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .as_deref(),
        Some("udevadm: not found\n")
    );
    let toasts: Vec<String> = vcx.update(|window, cx| {
        ToastQueue::of_window(window, cx)
            .expect("toast queue")
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    });
    assert!(
        toasts.iter().any(|t| t == "Copied to clipboard"),
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn setup_cancelled_pkexec_keeps_status(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    open_with(
        cx,
        &engine,
        not_readable(),
        Some(run(1, SetupStep::RaplEnable, SetupPhase::Cancelled)),
    );
    let card = cards(cx).remove(0);
    assert_eq!(card.failure, None);
    assert_eq!(card.done, None);
    assert_eq!(card.status.as_deref(), Some("Not enabled"));
}

#[gpui_kit::test]
fn setup_existing_rule_script_buttons_only(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    open_with(
        cx,
        &engine,
        status(
            RaplStatus::Readable {
                tasklens_rule: false,
            },
            true,
            true,
        ),
        None,
    );
    assert_eq!(
        cards(cx)[0].buttons,
        vec![
            ("View script".to_owned(), false),
            ("Copy command".to_owned(), false)
        ]
    );
}

#[gpui_kit::test]
fn setup_revert_confirm_and_send(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, tasklens_rule(), None);
    assert_eq!(cards(cx)[0].buttons[0], ("Revert".to_owned(), false));
    press(cx, &mut vcx, false);
    let words = cx.update(|cx| confirm_words(SetupStep::RaplRevert, cx));
    assert_eq!(words.heading.as_ref(), "Revert CPU power draw?");
    assert_eq!(
        words.text.as_ref(),
        "Removes the tasklens udev rule. A reboot may be needed before the files return to mode 0400."
    );
    let (view, mut dvcx) = dialog_view(cx);
    assert_eq!(
        dvcx.update(|window, cx| view.read(cx).focused_button(window)),
        Some(DialogButton::Cancel)
    );
    dvcx.simulate_keystrokes("tab");
    dvcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(run_setup_count(&engine, SetupStep::RaplRevert), 1);
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()))
}

fn copy_command(cx: &mut TestAppContext, vcx: &mut VisualTestContext, step: SetupStep) {
    let w = content(cx);
    vcx.update(|window, cx| w.update(cx, |w, cx| w.copy_command(step, window, cx)));
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn setup_copy_command_enable(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, not_readable(), None);
    assert_eq!(
        cards(cx)[0].buttons,
        vec![
            ("Enable".to_owned(), false),
            ("View script".to_owned(), false),
            ("Copy command".to_owned(), false)
        ]
    );
    copy_command(cx, &mut vcx, SetupStep::RaplEnable);
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("pkexec /usr/lib/tasklens/tasklens-setup rapl-enable")
    );
    assert!(toasts(&mut vcx).iter().any(|t| t == "Command copied"));
}

#[gpui_kit::test]
fn setup_copy_command_revert(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, tasklens_rule(), None);
    assert_eq!(
        cards(cx)[0].buttons,
        vec![
            ("Revert".to_owned(), false),
            ("View script".to_owned(), false),
            ("Copy command".to_owned(), false)
        ]
    );
    copy_command(cx, &mut vcx, SetupStep::RaplRevert);
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("pkexec /usr/lib/tasklens/tasklens-setup rapl-revert")
    );
}

#[gpui_kit::test]
fn setup_view_script_window(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let _vcx = open_with(cx, &engine, not_readable(), None);
    cx.update(|cx| open_script_window(SetupStep::RaplEnable, cx));
    cx.run_until_parked();
    let key = WindowKey::SetupScript("rapl-enable");
    let h = cx
        .update(|cx| cx.global::<GlobalRegistry>().0.read(cx).get(&key))
        .and_then(|h| h.downcast::<Root>())
        .expect("script window registered");
    let frame = cx
        .update(|cx| TlSecondaryWindow::<ScriptWindow>::find(h, cx))
        .expect("script frame");
    let (title, text) = cx.update(|cx| {
        let f = frame.read(cx);
        (f.title().to_string(), f.content().read(cx).text())
    });
    assert_eq!(title, "tasklens-setup rapl-enable");
    assert_eq!(text, SETUP_SCRIPT);
    // A second click focuses the open window.
    let before = cx.update(|cx| cx.windows().len());
    cx.update(|cx| open_script_window(SetupStep::RaplEnable, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), before);
    // The Copy button copies the text and shows the toast.
    let mut scx = VisualTestContext::from_window(h.into(), cx);
    scx.update(|window, _| window.activate_window());
    let content = cx.update(|cx| frame.read(cx).content().clone());
    scx.update(|window, cx| content.update(cx, |w, cx| w.copy(window, cx)));
    scx.run_until_parked();
    assert_eq!(clipboard(cx).as_deref(), Some(SETUP_SCRIPT));
    assert!(toasts(&mut scx).iter().any(|t| t == "Copied to clipboard"));
}

#[gpui_kit::test]
fn setup_journal_copy_only(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, not_readable(), None);
    assert_eq!(cards(cx)[1].buttons, vec![("Copy".to_owned(), false)]);
    let before = engine.commands().len();
    let w = content(cx);
    vcx.update(|window, cx| w.update(cx, |w, cx| w.copy_journal(window, cx)));
    vcx.run_until_parked();
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("sudo usermod -aG systemd-journal $USER")
    );
    assert!(toasts(&mut vcx).iter().any(|t| t == "Command copied"));
    assert_eq!(engine.commands().len(), before, "no command sent");
}

#[gpui_kit::test]
fn setup_not_needed_no_buttons(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let _vcx = open_with(cx, &engine, status(RaplStatus::NoZone, true, true), None);
    let card = cards(cx).remove(0);
    assert_eq!(card.status.as_deref(), Some("Not needed"));
    assert!(card.buttons.is_empty());
}

fn press_run(cx: &mut TestAppContext, vcx: &mut VisualTestContext, step: SetupStep) {
    let w = content(cx);
    vcx.update(|_, cx| w.update(cx, |w, cx| w.press_run(step, cx)));
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn fan_not_needed_with_hwmon_fan(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let _vcx = open_with(cx, &engine, fan(true, true, false), None);
    let card = cards(cx).remove(3);
    assert_eq!(card.status.as_deref(), Some("Not needed"));
    assert!(card.buttons.is_empty());
}

#[gpui_kit::test]
fn fan_lm_sensors_missing(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let _vcx = open_with(cx, &engine, fan(false, false, false), None);
    let card = cards(cx).remove(3);
    assert_eq!(card.status.as_deref(), Some("Not available"));
    assert_eq!(card.description, "Install lm_sensors to detect fan sensors");
    assert!(card.buttons.is_empty());
}

#[gpui_kit::test]
fn fan_run_detection_sends_step(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, fan(false, true, false), None);
    let card = cards(cx).remove(3);
    assert_eq!(card.status.as_deref(), Some("Not enabled"));
    assert_eq!(
        card.description,
        "Finds motherboard sensor chips and loads their drivers."
    );
    assert_eq!(
        card.buttons,
        vec![
            ("Run sensor detection".to_owned(), false),
            ("View script".to_owned(), false),
            ("Copy command".to_owned(), false)
        ]
    );
    let windows = window_count(cx);
    press_run(cx, &mut vcx, SetupStep::SensorsDetect);
    assert_eq!(run_setup_count(&engine, SetupStep::SensorsDetect), 1);
    assert_eq!(window_count(cx), windows);
    publish_run(
        &engine,
        fan(false, true, false),
        Some(run(1, SetupStep::SensorsDetect, SetupPhase::Succeeded)),
    );
    cx.run_until_parked();
    assert_eq!(
        cards(cx)[3].done.as_deref(),
        Some("Done. No restart needed.")
    );
    assert_eq!(cards(cx)[0].done, None);
    assert_eq!(sensors_reprobe_count(&engine), 1);
    assert_eq!(reprobe_count(&engine), 0);
}

#[gpui_kit::test]
fn fan_revert_sends_step(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let mut vcx = open_with(cx, &engine, fan(false, true, true), None);
    let card = cards(cx).remove(3);
    assert_eq!(card.status.as_deref(), Some("Enabled"));
    assert_eq!(card.buttons[0], ("Revert".to_owned(), false));
    press_run(cx, &mut vcx, SetupStep::SensorsRevert);
    assert_eq!(run_setup_count(&engine, SetupStep::SensorsRevert), 1);
    copy_command(cx, &mut vcx, SetupStep::SensorsRevert);
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("pkexec /usr/lib/tasklens/tasklens-setup sensors-revert")
    );
}

#[gpui_kit::test]
fn rows_order_p4(cx: &mut TestAppContext) {
    let engine = setup(cx, true);
    let _vcx = open_with(cx, &engine, fan(false, true, false), None);
    let titles: Vec<String> = cards(cx).into_iter().map(|c| c.title).collect();
    assert_eq!(
        titles,
        [
            "CPU power draw",
            "Journal access",
            "Administrator helper",
            "Fan sensors",
            "Per-process network"
        ]
    );
}
