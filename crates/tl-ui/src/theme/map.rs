//! Copies the active `TlTheme` and `UiMetrics` onto the gpui-kit `Theme`, so kit widgets
//! (menus, dialogs, inputs, tooltips, toasts, buttons, scrollbars) match tasklens's own views
//! (ADR-009 theme model, UI-FND-002).
//!
//! Token names checked against gpui-kit 0.7.0 (gpui-component 0.7.0), file
//! `crates/component/src/theme/theme_color.rs` (`ThemeColor`), on 2026-10-04 (U-8). Every name
//! in the mapping exists there, so there are no substitutions. `list_head` exists in that source
//! but is not in the mapping; it stays at the kit default like the other unmapped fields.
//!
//! `notification` is not set here; the toast queue (E16-006) owns it.

use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::theme::{Theme, ThemeColor, ThemeMode as KitMode};
use gpui_kit::{Hsla, rgb};

use super::{ThemeMode, TlTheme, UiMetrics};

/// `c` with lightness moved by `step`, clamped to 0..=1.
fn lightness(c: Hsla, step: f32) -> Hsla {
    Hsla {
        l: (c.l + step).clamp(0.0, 1.0),
        ..c
    }
}

fn kit_mode(mode: ThemeMode) -> KitMode {
    match mode {
        ThemeMode::Dark => KitMode::Dark,
        ThemeMode::Light => KitMode::Light,
    }
}

/// The kit's own colours for `mode`.
fn kit_defaults(mode: ThemeMode) -> ThemeColor {
    match mode {
        ThemeMode::Dark => *ThemeColor::dark(),
        ThemeMode::Light => *ThemeColor::light(),
    }
}

/// The skeleton step: lighter on a dark theme, darker on a light one.
fn skeleton_step(mode: ThemeMode) -> f32 {
    match mode {
        ThemeMode::Dark => 0.06,
        ThemeMode::Light => -0.06,
    }
}

/// Writes `tl` and `m` onto `kit`. Every colour field is first reset to the kit default for
/// `tl.mode`, so fields not mapped here follow a mode switch.
pub fn apply(tl: &TlTheme, m: &UiMetrics, kit: &mut Theme) {
    kit.colors = kit_defaults(tl.mode);

    let white: Hsla = rgb(0xFFFFFF).into();
    let ink: Hsla = rgb(0x1B1F22).into();
    let s = &tl.status;
    let c = &mut kit.colors;

    c.background = tl.window_bg;
    c.foreground = tl.text_primary;
    c.muted = tl.row_alt;
    c.muted_foreground = tl.text_secondary;
    c.border = tl.divider;
    c.input = tl.input;
    c.ring = tl.accent;
    c.primary = tl.accent;
    c.primary_hover = lightness(tl.accent, 0.08);
    c.primary_active = lightness(tl.accent, -0.08);
    c.primary_foreground = white;
    c.secondary = tl.toolbar;
    c.secondary_hover = tl.tab_hover;
    c.secondary_active = tl.header;
    c.secondary_foreground = tl.text_primary;
    c.accent = tl.menu_hover;
    c.accent_foreground = tl.text_primary;
    c.popover = tl.menu_bg;
    c.popover_foreground = tl.text_primary;
    c.title_bar = tl.titlebar;
    c.title_bar_border = tl.divider;
    c.status_bar = tl.toolbar;
    c.status_bar_border = tl.divider;
    c.tab_bar = tl.tab_bg;
    c.tab = tl.tab_bg;
    c.tab_active = tl.window_bg;
    c.tab_foreground = tl.text_secondary;
    c.tab_active_foreground = tl.text_primary;
    c.table = tl.list_bg;
    c.table_even = tl.row_alt;
    c.table_hover = tl.row_hover;
    c.table_active = tl.row_selected;
    c.table_active_border = tl.accent;
    c.table_head = tl.header;
    c.table_head_foreground = tl.text_primary;
    c.table_row_border = tl.divider;
    c.list = tl.list_bg;
    c.list_even = tl.row_alt;
    c.list_hover = tl.row_hover;
    c.list_active = tl.row_selected;
    c.list_active_border = tl.accent;
    c.scrollbar = tl.scroll_track;
    c.scrollbar_thumb = tl.scroll_thumb;
    c.scrollbar_thumb_hover = lightness(tl.scroll_thumb, 0.10);
    c.selection = tl.accent.alpha(0.35);
    c.caret = tl.text_primary;
    c.skeleton = lightness(tl.row_alt, skeleton_step(tl.mode));
    c.success = s.success;
    c.success_foreground = white;
    c.success_hover = lightness(s.success, 0.08);
    c.success_active = lightness(s.success, -0.08);
    c.warning = s.warning;
    c.warning_foreground = ink;
    c.warning_hover = lightness(s.warning, 0.08);
    c.warning_active = lightness(s.warning, -0.08);
    c.danger = s.danger;
    c.danger_foreground = white;
    c.danger_hover = lightness(s.danger, 0.08);
    c.danger_active = lightness(s.danger, -0.08);
    c.info = s.info;
    c.info_foreground = white;
    c.info_hover = lightness(s.info, 0.08);
    c.info_active = lightness(s.info, -0.08);
    c.link = s.info;
    c.link_hover = lightness(s.info, 0.10);
    c.link_active = lightness(s.info, -0.10);
    c.switch = tl.scroll_thumb;
    c.switch_thumb = white;
    c.slider_bar = tl.accent;
    c.slider_thumb = white;
    c.progress_bar = tl.accent;
    c.description_list_label = tl.row_alt;
    c.description_list_label_foreground = tl.text_secondary;
    c.group_box = tl.row_alt;
    c.group_box_foreground = tl.text_primary;
    c.chart_1 = tl.graph.cpu_line;
    c.chart_2 = tl.graph.mem_line;
    c.chart_3 = tl.graph.gpu_line;
    c.chart_4 = tl.graph.disk_line;
    c.chart_5 = tl.graph.net_line;
    c.chart_grid = tl.graph.grid;
    c.overlay = tl.derived.overlay;
    c.window_border = tl.divider;
    c.drag_border = tl.accent;
    c.drop_target = tl.accent.alpha(0.15);
    c.button = tl.toolbar;
    c.button_hover = tl.tab_hover;
    c.button_active = tl.header;
    c.button_foreground = tl.text_primary;
    c.button_primary = c.primary;
    c.button_primary_hover = c.primary_hover;
    c.button_primary_active = c.primary_active;
    c.button_primary_foreground = c.primary_foreground;
    c.button_danger = c.danger;
    c.button_danger_hover = c.danger_hover;
    c.button_danger_active = c.danger_active;
    c.button_danger_foreground = c.danger_foreground;
    c.button_secondary = c.secondary;
    c.button_secondary_hover = c.secondary_hover;
    c.button_secondary_active = c.secondary_active;
    c.button_secondary_foreground = c.secondary_foreground;
    c.button_success = c.success;
    c.button_success_hover = c.success_hover;
    c.button_success_active = c.success_active;
    c.button_success_foreground = c.success_foreground;
    c.button_warning = c.warning;
    c.button_warning_hover = c.warning_hover;
    c.button_warning_active = c.warning_active;
    c.button_warning_foreground = c.warning_foreground;
    c.button_info = c.info;
    c.button_info_hover = c.info_hover;
    c.button_info_active = c.info_active;
    c.button_info_foreground = c.info_foreground;
    c.sidebar = tl.window_bg;
    c.sidebar_border = tl.divider;
    c.sidebar_foreground = tl.text_primary;
    c.sidebar_accent = tl.row_hover;
    c.sidebar_accent_foreground = tl.text_primary;
    c.sidebar_primary = tl.accent;
    c.sidebar_primary_foreground = white;

    kit.font_family = "Noto Sans".into();
    kit.font_size = m.base_text;
    kit.mono_font_family = "Noto Sans Mono".into();
    kit.mono_font_size = m.base_text;
    kit.radius = m.radius_small;
    kit.radius_lg = m.radius_card;
    kit.shadow = tl.derived.shadow.a > 0.0;
    kit.focus_ring = true;
    kit.scrollbar_mode = ScrollbarMode::Scrolling;
    kit.mode = kit_mode(tl.mode);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::px;
    use serde_json::Value;
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::theme::metrics::compute;
    use crate::theme::tests::test_theme;
    use crate::theme::{ThemeSlot, builtin, rebuild_derived};

    /// Kit fields `apply` leaves at the kit default.
    const UNMAPPED: [&str; 19] = [
        "red",
        "red_light",
        "green",
        "green_light",
        "blue",
        "blue_light",
        "yellow",
        "yellow_light",
        "magenta",
        "magenta_light",
        "cyan",
        "cyan_light",
        "chart_bullish",
        "chart_bearish",
        "accordion",
        "table_foot",
        "table_foot_foreground",
        "tab_bar_segmented",
        "list_head",
    ];

    /// Kit fields `apply` sets to a fixed colour, not one from the theme.
    const FIXED: [&str; 13] = [
        "primary_foreground",
        "success_foreground",
        "warning_foreground",
        "danger_foreground",
        "info_foreground",
        "switch_thumb",
        "slider_thumb",
        "sidebar_primary_foreground",
        "button_primary_foreground",
        "button_success_foreground",
        "button_warning_foreground",
        "button_danger_foreground",
        "button_info_foreground",
    ];

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    fn builtin_theme(name: &str, appearance: &AppearanceSettings) -> TlTheme {
        let def = builtin::load()
            .iter()
            .find(|d| &*d.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no built-in theme {name}"));
        let fallback = test_theme(def.effective_mode());
        TlTheme::from_def(&def, &fallback, appearance, true, 1)
    }

    fn metrics_for(appearance: &AppearanceSettings) -> UiMetrics {
        compute(appearance, 0)
    }

    fn fields(colors: &ThemeColor) -> serde_json::Map<String, Value> {
        match serde_json::to_value(colors) {
            Ok(Value::Object(map)) => map,
            other => panic!("ThemeColor did not serialize to an object: {other:?}"),
        }
    }

    fn sentinel_kit() -> Theme {
        let sentinel = hex(0x010203);
        let mut colors = fields(&ThemeColor::default());
        let encoded = serde_json::to_value(sentinel).unwrap();
        colors.values_mut().for_each(|v| *v = encoded.clone());
        let colors: ThemeColor = serde_json::from_value(Value::Object(colors)).unwrap();
        Theme::from(&colors)
    }

    #[test]
    fn every_mapped_field_is_set() {
        let appearance = AppearanceSettings::default();
        let m = metrics_for(&appearance);
        let sentinel = serde_json::to_value(hex(0x010203)).unwrap();
        let unmapped: BTreeSet<&str> = UNMAPPED.into_iter().collect();
        let fixed: BTreeSet<&str> = FIXED.into_iter().collect();

        let mut kit = sentinel_kit();
        apply(&builtin_theme("Default Dark", &appearance), &m, &mut kit);
        for (name, value) in fields(&kit.colors) {
            assert_ne!(value, sentinel, "{name} still holds the sentinel");
        }

        // Every field outside UNMAPPED and FIXED must follow the theme: two themes with no colour
        // in common give a different value in each such field.
        let a = test_theme(ThemeMode::Dark);
        let mut b = test_theme(ThemeMode::Dark);
        for (i, slot) in ThemeSlot::ALL.iter().enumerate() {
            slot.set(&mut b, hex(0x400000 + 0x020202 * i as u32));
        }
        b.status.success = hex(0x60A000);
        b.status.warning = hex(0x60A010);
        b.status.danger = hex(0x60A020);
        b.status.info = hex(0x60A030);
        b.graph.cpu_line = hex(0x70B000);
        b.graph.mem_line = hex(0x70B010);
        b.graph.gpu_line = hex(0x70B020);
        b.graph.disk_line = hex(0x70B030);
        b.graph.net_line = hex(0x70B040);
        b.graph.grid = hex(0x70B050);
        rebuild_derived(&mut b, &appearance);
        b.derived.overlay = hex(0x80C000).alpha(0.5);

        let mut kit_a = sentinel_kit();
        let mut kit_b = sentinel_kit();
        apply(&a, &m, &mut kit_a);
        apply(&b, &m, &mut kit_b);
        let fa = fields(&kit_a.colors);
        let fb = fields(&kit_b.colors);
        let defaults = fields(&ThemeColor::dark());
        for (name, va) in &fa {
            let n = name.as_str();
            let vb = &fb[name];
            if unmapped.contains(n) {
                assert_eq!(va, &defaults[name], "{n} should stay at the kit default");
                assert_eq!(va, vb, "{n} should not follow the theme");
            } else if fixed.contains(n) {
                assert_eq!(va, vb, "{n} should be a fixed colour");
            } else {
                assert_ne!(va, vb, "{n} does not follow the theme");
            }
        }
        for name in unmapped.iter().chain(fixed.iter()) {
            assert!(fa.contains_key(*name), "{name} is not a ThemeColor field");
        }
    }

    #[test]
    fn light_dark_reset() {
        let appearance = AppearanceSettings::default();
        let m = metrics_for(&appearance);
        let mut kit = sentinel_kit();

        apply(&builtin_theme("Default Dark", &appearance), &m, &mut kit);
        assert_eq!(kit.colors.red, ThemeColor::dark().red);
        assert_eq!(
            kit.colors.tab_bar_segmented,
            ThemeColor::dark().tab_bar_segmented
        );
        assert_eq!(kit.mode, KitMode::Dark);

        apply(&builtin_theme("Default Light", &appearance), &m, &mut kit);
        assert_eq!(kit.colors.tab_active, hex(0xF3F4F6));
        assert_eq!(kit.colors.red, ThemeColor::light().red);
        assert_eq!(
            kit.colors.tab_bar_segmented,
            ThemeColor::light().tab_bar_segmented
        );
        assert_eq!(kit.mode, KitMode::Light);
    }

    #[test]
    fn hover_active_steps() {
        let appearance = AppearanceSettings::default();
        let m = metrics_for(&appearance);
        let mut tl = builtin_theme("Default Dark", &appearance);
        tl.accent = hex(0x0277A6);
        let mut kit = sentinel_kit();
        apply(&tl, &m, &mut kit);
        let c = &kit.colors;

        assert_eq!(c.primary_hover.l, tl.accent.l + 0.08);
        assert_eq!(c.primary_active.l, tl.accent.l - 0.08);
        assert_eq!(
            (c.primary_hover.h, c.primary_hover.s),
            (tl.accent.h, tl.accent.s)
        );
        assert_eq!(c.button_primary_hover, c.primary_hover);
        assert_eq!(c.success_hover.l, tl.status.success.l + 0.08);
        assert_eq!(c.danger_active.l, tl.status.danger.l - 0.08);
        assert_eq!(c.link_hover.l, tl.status.info.l + 0.10);
        assert_eq!(c.link_active.l, tl.status.info.l - 0.10);
        assert_eq!(c.scrollbar_thumb_hover.l, tl.scroll_thumb.l + 0.10);
        assert_eq!(c.skeleton.l, tl.row_alt.l + 0.06);
        assert_eq!(c.selection, tl.accent.alpha(0.35));
        assert_eq!(c.drop_target, tl.accent.alpha(0.15));

        let light = builtin_theme("Default Light", &appearance);
        apply(&light, &m, &mut kit);
        assert_eq!(kit.colors.skeleton.l, light.row_alt.l - 0.06);

        // The step clamps to 0..=1.
        assert_eq!(lightness(hex(0xFFFFFF), 0.10).l, 1.0);
        assert_eq!(lightness(hex(0x000000), -0.10).l, 0.0);
    }

    #[test]
    fn metrics_fields() {
        let scaled = AppearanceSettings {
            ui_scale_pct: 150,
            rounded_corners: true,
            ..AppearanceSettings::default()
        };
        let tl = builtin_theme("Default Dark", &scaled);
        let mut kit = sentinel_kit();
        apply(&tl, &metrics_for(&scaled), &mut kit);
        assert_eq!(kit.font_size, px(19.5));
        assert_eq!(kit.mono_font_size, px(19.5));
        assert_eq!(kit.radius, px(6.0));
        assert_eq!(kit.radius_lg, px(9.0));
        assert!(kit.shadow);
        assert!(kit.focus_ring);
        assert_eq!(kit.scrollbar_mode, ScrollbarMode::Scrolling);
        assert_eq!(kit.font_family.as_ref(), "Noto Sans");
        assert_eq!(kit.mono_font_family.as_ref(), "Noto Sans Mono");

        let square = AppearanceSettings {
            rounded_corners: false,
            ..AppearanceSettings::default()
        };
        apply(&tl, &metrics_for(&square), &mut kit);
        assert_eq!(kit.radius, px(0.0));
        assert_eq!(kit.radius_lg, px(0.0));

        let flat = AppearanceSettings {
            shadow_intensity_pct: 0,
            ..AppearanceSettings::default()
        };
        let tl = builtin_theme("Default Dark", &flat);
        apply(&tl, &metrics_for(&flat), &mut kit);
        assert!(!kit.shadow);
    }
}
