//! Theme store integration tests (E17-A1, UI-FND-040).

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Hsla, TestAppContext, rgb};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::theme::store::init;
use tl_ui::theme::{ThemeStore, UiMetrics, current};

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

fn init_with(active: &str, cx: &mut App) {
    gpui_kit::init(cx);
    let theme = ThemeSettings {
        active: active.to_owned(),
        ..ThemeSettings::default()
    };
    init(cx, &theme, &AppearanceSettings::default(), Vec::new());
}

#[gpui_kit::test]
fn theme_init_installs_globals(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("Default Dark", cx);
        assert!(cx.has_global::<ThemeStore>());
        assert!(cx.has_global::<UiMetrics>());
        assert!(cx.has_global::<Theme>());
        assert_eq!(&*current(cx).name, "Default Dark");
        assert!(current(cx).builtin);
        assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
        assert_eq!(Theme::global(cx).colors.primary, hex(0x0277A6));
    });
}

#[gpui_kit::test]
fn unknown_active_falls_back_to_dark(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("No Such Theme", cx);
        assert_eq!(&*current(cx).name, "Default Dark");
        assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
    });
}

#[gpui_kit::test]
fn light_active_at_start(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("Default Light", cx);
        assert_eq!(&*current(cx).name, "Default Light");
        assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
    });
}
