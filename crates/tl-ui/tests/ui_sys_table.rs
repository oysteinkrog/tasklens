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
