//! Theme data: `TlTheme` with TaskSlinger's 22 colour slots, the accent, the graph palette, the
//! status colours and the derived tints (ADR-009, UI-FND-001, UI-FND-040).

pub mod builtin;
pub mod derived;
pub mod map;
pub mod metrics;
pub mod palette;
pub mod store;
pub mod user_store;

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::Hsla;
use serde::{Deserialize, Serialize};
use tl_model::settings::AppearanceSettings;

pub use derived::{Derived, rebuild_derived};
pub use metrics::{UiMetrics, bump_style_version, metrics};
pub use palette::GraphPalette;
pub use store::{ActivateSource, ActivateTheme, ThemeError, ThemeEvent, ThemeStore, current};

/// Dark or light. A theme file stores it as `"dark"` or `"light"`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
}

/// Colours for status text, badges and row tints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusColors {
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub info: Hsla,
}

/// The theme every view reads. Built from a `ThemeDef` by `TlTheme::from_def`.
#[derive(Clone, Debug)]
pub struct TlTheme {
    pub name: Arc<str>,
    pub mode: ThemeMode,
    pub version: u64,
    pub builtin: bool,
    pub window_bg: Hsla,
    pub titlebar: Hsla,
    pub toolbar: Hsla,
    pub input: Hsla,
    pub input_focus: Hsla,
    pub header: Hsla,
    pub list_bg: Hsla,
    pub row_alt: Hsla,
    pub row_hover: Hsla,
    pub row_selected: Hsla,
    pub divider: Hsla,
    pub menu_bg: Hsla,
    pub menu_hover: Hsla,
    pub tab_bg: Hsla,
    pub tab_hover: Hsla,
    pub scroll_track: Hsla,
    pub scroll_thumb: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_selected_primary: Hsla,
    pub text_selected_secondary: Hsla,
    pub value_heat: Hsla,
    pub accent: Hsla,
    pub graph: GraphPalette,
    pub status: StatusColors,
    pub derived: Derived,
}

/// One editable colour of a theme: the 22 TaskSlinger slots, then the accent (APP-035 order).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ThemeSlot {
    WindowBg,
    Titlebar,
    Toolbar,
    Input,
    InputFocus,
    Header,
    ListBg,
    RowAlt,
    RowHover,
    RowSelected,
    Divider,
    MenuBg,
    MenuHover,
    TabBg,
    TabHover,
    ScrollTrack,
    ScrollThumb,
    TextPrimary,
    TextSecondary,
    TextSelectedPrimary,
    TextSelectedSecondary,
    ValueHeat,
    Accent,
}

impl ThemeSlot {
    pub const ALL: [ThemeSlot; 23] = [
        ThemeSlot::WindowBg,
        ThemeSlot::Titlebar,
        ThemeSlot::Toolbar,
        ThemeSlot::Input,
        ThemeSlot::InputFocus,
        ThemeSlot::Header,
        ThemeSlot::ListBg,
        ThemeSlot::RowAlt,
        ThemeSlot::RowHover,
        ThemeSlot::RowSelected,
        ThemeSlot::Divider,
        ThemeSlot::MenuBg,
        ThemeSlot::MenuHover,
        ThemeSlot::TabBg,
        ThemeSlot::TabHover,
        ThemeSlot::ScrollTrack,
        ThemeSlot::ScrollThumb,
        ThemeSlot::TextPrimary,
        ThemeSlot::TextSecondary,
        ThemeSlot::TextSelectedPrimary,
        ThemeSlot::TextSelectedSecondary,
        ThemeSlot::ValueHeat,
        ThemeSlot::Accent,
    ];

    /// The key in a theme file: TaskSlinger's upper-case slot names, and `accent`.
    pub fn json_key(self) -> &'static str {
        match self {
            ThemeSlot::WindowBg => "WINDOW_BG",
            ThemeSlot::Titlebar => "TITLEBAR",
            ThemeSlot::Toolbar => "TOOLBAR",
            ThemeSlot::Input => "INPUT",
            ThemeSlot::InputFocus => "INPUT_FOCUS",
            ThemeSlot::Header => "HEADER",
            ThemeSlot::ListBg => "LIST_BG",
            ThemeSlot::RowAlt => "ROW_ALT",
            ThemeSlot::RowHover => "ROW_HOVER",
            ThemeSlot::RowSelected => "ROW_SELECTED",
            ThemeSlot::Divider => "DIVIDER",
            ThemeSlot::MenuBg => "MENU_BG",
            ThemeSlot::MenuHover => "MENU_HOVER",
            ThemeSlot::TabBg => "TAB_BG",
            ThemeSlot::TabHover => "TAB_HOVER",
            ThemeSlot::ScrollTrack => "SCROLL_TRACK",
            ThemeSlot::ScrollThumb => "SCROLL_THUMB",
            ThemeSlot::TextPrimary => "TEXT_PRIMARY",
            ThemeSlot::TextSecondary => "TEXT_SECONDARY",
            ThemeSlot::TextSelectedPrimary => "TEXT_SELECTED_PRIMARY",
            ThemeSlot::TextSelectedSecondary => "TEXT_SELECTED_SECONDARY",
            ThemeSlot::ValueHeat => "VALUE_HEAT",
            ThemeSlot::Accent => "accent",
        }
    }

    /// The label in the theme editor (APP-035).
    pub fn label(self) -> &'static str {
        match self {
            ThemeSlot::WindowBg => "Window background",
            ThemeSlot::Titlebar => "Title bar",
            ThemeSlot::Toolbar => "Toolbar",
            ThemeSlot::Input => "Input",
            ThemeSlot::InputFocus => "Input focus",
            ThemeSlot::Header => "Header",
            ThemeSlot::ListBg => "List background",
            ThemeSlot::RowAlt => "Alternate row",
            ThemeSlot::RowHover => "Row hover",
            ThemeSlot::RowSelected => "Selected row",
            ThemeSlot::Divider => "Divider",
            ThemeSlot::MenuBg => "Menu background",
            ThemeSlot::MenuHover => "Menu hover",
            ThemeSlot::TabBg => "Tab background",
            ThemeSlot::TabHover => "Tab hover",
            ThemeSlot::ScrollTrack => "Scroll track",
            ThemeSlot::ScrollThumb => "Scroll thumb",
            ThemeSlot::TextPrimary => "Primary text",
            ThemeSlot::TextSecondary => "Secondary text",
            ThemeSlot::TextSelectedPrimary => "Selected primary text",
            ThemeSlot::TextSelectedSecondary => "Selected secondary text",
            ThemeSlot::ValueHeat => "Value heat",
            ThemeSlot::Accent => "Accent",
        }
    }

    fn field_mut(self, t: &mut TlTheme) -> &mut Hsla {
        match self {
            ThemeSlot::WindowBg => &mut t.window_bg,
            ThemeSlot::Titlebar => &mut t.titlebar,
            ThemeSlot::Toolbar => &mut t.toolbar,
            ThemeSlot::Input => &mut t.input,
            ThemeSlot::InputFocus => &mut t.input_focus,
            ThemeSlot::Header => &mut t.header,
            ThemeSlot::ListBg => &mut t.list_bg,
            ThemeSlot::RowAlt => &mut t.row_alt,
            ThemeSlot::RowHover => &mut t.row_hover,
            ThemeSlot::RowSelected => &mut t.row_selected,
            ThemeSlot::Divider => &mut t.divider,
            ThemeSlot::MenuBg => &mut t.menu_bg,
            ThemeSlot::MenuHover => &mut t.menu_hover,
            ThemeSlot::TabBg => &mut t.tab_bg,
            ThemeSlot::TabHover => &mut t.tab_hover,
            ThemeSlot::ScrollTrack => &mut t.scroll_track,
            ThemeSlot::ScrollThumb => &mut t.scroll_thumb,
            ThemeSlot::TextPrimary => &mut t.text_primary,
            ThemeSlot::TextSecondary => &mut t.text_secondary,
            ThemeSlot::TextSelectedPrimary => &mut t.text_selected_primary,
            ThemeSlot::TextSelectedSecondary => &mut t.text_selected_secondary,
            ThemeSlot::ValueHeat => &mut t.value_heat,
            ThemeSlot::Accent => &mut t.accent,
        }
    }

    /// Reads this slot's colour from `t`.
    pub fn get(self, t: &TlTheme) -> Hsla {
        match self {
            ThemeSlot::WindowBg => t.window_bg,
            ThemeSlot::Titlebar => t.titlebar,
            ThemeSlot::Toolbar => t.toolbar,
            ThemeSlot::Input => t.input,
            ThemeSlot::InputFocus => t.input_focus,
            ThemeSlot::Header => t.header,
            ThemeSlot::ListBg => t.list_bg,
            ThemeSlot::RowAlt => t.row_alt,
            ThemeSlot::RowHover => t.row_hover,
            ThemeSlot::RowSelected => t.row_selected,
            ThemeSlot::Divider => t.divider,
            ThemeSlot::MenuBg => t.menu_bg,
            ThemeSlot::MenuHover => t.menu_hover,
            ThemeSlot::TabBg => t.tab_bg,
            ThemeSlot::TabHover => t.tab_hover,
            ThemeSlot::ScrollTrack => t.scroll_track,
            ThemeSlot::ScrollThumb => t.scroll_thumb,
            ThemeSlot::TextPrimary => t.text_primary,
            ThemeSlot::TextSecondary => t.text_secondary,
            ThemeSlot::TextSelectedPrimary => t.text_selected_primary,
            ThemeSlot::TextSelectedSecondary => t.text_selected_secondary,
            ThemeSlot::ValueHeat => t.value_heat,
            ThemeSlot::Accent => t.accent,
        }
    }

    /// Writes `c` into this slot of `t`. Does not rebuild the derived tints.
    pub fn set(self, t: &mut TlTheme, c: Hsla) {
        *self.field_mut(t) = c;
    }
}

/// A parsed theme file record. `extra` keeps unknown keys so a user file round-trips
/// (UI-FND-042).
#[derive(Clone, Debug)]
pub struct ThemeDef {
    pub name: Arc<str>,
    pub mode: Option<ThemeMode>,
    /// The 22 slots in `ThemeSlot::ALL` order (without `Accent`).
    pub slots: [Hsla; 22],
    pub accent: Hsla,
    pub status: Option<StatusColors>,
    pub graph: Option<GraphPalette>,
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ThemeDef {
    /// `mode` when set, else `Dark` when the window background lightness is below 0.5, else
    /// `Light` (UI-FND-040).
    pub fn effective_mode(&self) -> ThemeMode {
        match self.mode {
            Some(mode) => mode,
            None if self.slots[ThemeSlot::WindowBg as usize].l < 0.5 => ThemeMode::Dark,
            None => ThemeMode::Light,
        }
    }
}

impl TlTheme {
    /// Builds a theme from `def`. `status` and `graph` come from `fallback` (the built-in theme of
    /// the same mode) when the def has none. Rebuilds the derived tints.
    pub fn from_def(
        def: &ThemeDef,
        fallback: &TlTheme,
        appearance: &AppearanceSettings,
        builtin: bool,
        version: u64,
    ) -> TlTheme {
        let mut t = TlTheme {
            name: def.name.clone(),
            mode: def.effective_mode(),
            version,
            builtin,
            accent: def.accent,
            graph: def.graph.unwrap_or(fallback.graph),
            status: def.status.unwrap_or(fallback.status),
            derived: Derived::empty(),
            ..fallback.clone()
        };
        for (slot, &c) in ThemeSlot::ALL.iter().zip(def.slots.iter()) {
            slot.set(&mut t, c);
        }
        rebuild_derived(&mut t, appearance);
        t
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gpui_kit::rgb;

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    /// A theme with a distinct colour in every slot, for tests.
    pub(crate) fn test_theme(mode: ThemeMode) -> TlTheme {
        let c = hex(0x808080);
        let mut t = TlTheme {
            name: "Test".into(),
            mode,
            version: 0,
            builtin: true,
            window_bg: c,
            titlebar: c,
            toolbar: c,
            input: c,
            input_focus: c,
            header: c,
            list_bg: c,
            row_alt: c,
            row_hover: c,
            row_selected: c,
            divider: c,
            menu_bg: c,
            menu_hover: c,
            tab_bg: c,
            tab_hover: c,
            scroll_track: c,
            scroll_thumb: c,
            text_primary: c,
            text_secondary: c,
            text_selected_primary: c,
            text_selected_secondary: c,
            value_heat: c,
            accent: c,
            graph: GraphPalette {
                cpu_fill: hex(0x102030),
                cpu_line: hex(0x112131),
                kernel_line: hex(0x122232),
                mem_fill: hex(0x132333),
                mem_line: hex(0x142434),
                gpu_fill: hex(0x152535),
                gpu_line: hex(0x162636),
                disk_fill: hex(0x172737),
                disk_line: hex(0x182838),
                net_fill: hex(0x192939),
                net_line: hex(0x1A2A3A),
                grid: hex(0x1B2B3B),
                chart_bg: hex(0x1C2C3C),
            },
            status: StatusColors {
                success: hex(0x4CAF50),
                warning: hex(0xF5B700),
                danger: hex(0xF43F5E),
                info: hex(0x3B82F6),
            },
            derived: Derived::empty(),
        };
        for (i, slot) in ThemeSlot::ALL.iter().enumerate() {
            slot.set(&mut t, hex(0x010101 * (i as u32 + 1)));
        }
        rebuild_derived(&mut t, &AppearanceSettings::default());
        t
    }

    fn def_with_window_bg(window_bg: u32) -> ThemeDef {
        let mut slots = [hex(0x808080); 22];
        slots[ThemeSlot::WindowBg as usize] = hex(window_bg);
        ThemeDef {
            name: "Def".into(),
            mode: None,
            slots,
            accent: hex(0xFF7F50),
            status: None,
            graph: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn mode_inferred_from_window_bg() {
        assert_eq!(
            def_with_window_bg(0xF3F4F6).effective_mode(),
            ThemeMode::Light
        );
        assert_eq!(
            def_with_window_bg(0x191B1D).effective_mode(),
            ThemeMode::Dark
        );
        let mut set = def_with_window_bg(0x191B1D);
        set.mode = Some(ThemeMode::Light);
        assert_eq!(set.effective_mode(), ThemeMode::Light);
    }

    #[test]
    fn slot_keys_and_labels() {
        assert_eq!(ThemeSlot::ALL.len(), 23);
        let keys: Vec<&str> = ThemeSlot::ALL.iter().map(|s| s.json_key()).collect();
        assert_eq!(
            keys,
            [
                "WINDOW_BG",
                "TITLEBAR",
                "TOOLBAR",
                "INPUT",
                "INPUT_FOCUS",
                "HEADER",
                "LIST_BG",
                "ROW_ALT",
                "ROW_HOVER",
                "ROW_SELECTED",
                "DIVIDER",
                "MENU_BG",
                "MENU_HOVER",
                "TAB_BG",
                "TAB_HOVER",
                "SCROLL_TRACK",
                "SCROLL_THUMB",
                "TEXT_PRIMARY",
                "TEXT_SECONDARY",
                "TEXT_SELECTED_PRIMARY",
                "TEXT_SELECTED_SECONDARY",
                "VALUE_HEAT",
                "accent",
            ]
        );
        let unique: std::collections::BTreeSet<&str> = keys[..22].iter().copied().collect();
        assert_eq!(unique.len(), 22);
        assert!(keys[..22].iter().all(|k| *k == k.to_ascii_uppercase()));

        let labels: Vec<&str> = ThemeSlot::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            [
                "Window background",
                "Title bar",
                "Toolbar",
                "Input",
                "Input focus",
                "Header",
                "List background",
                "Alternate row",
                "Row hover",
                "Selected row",
                "Divider",
                "Menu background",
                "Menu hover",
                "Tab background",
                "Tab hover",
                "Scroll track",
                "Scroll thumb",
                "Primary text",
                "Secondary text",
                "Selected primary text",
                "Selected secondary text",
                "Value heat",
                "Accent",
            ]
        );

        let mut t = test_theme(ThemeMode::Dark);
        for (i, slot) in ThemeSlot::ALL.iter().enumerate() {
            let c = hex(0x200000 + i as u32);
            slot.set(&mut t, c);
            assert_eq!(slot.get(&t), c);
        }
        for (i, slot) in ThemeSlot::ALL.iter().enumerate() {
            assert_eq!(slot.get(&t), hex(0x200000 + i as u32));
        }
    }

    #[test]
    fn from_def_uses_fallback_status_and_graph() {
        let fallback = test_theme(ThemeMode::Dark);
        let def = def_with_window_bg(0x191B1D);
        let t = TlTheme::from_def(&def, &fallback, &AppearanceSettings::default(), false, 7);
        assert_eq!(t.status, fallback.status);
        assert_eq!(t.graph, fallback.graph);
        assert_eq!(t.mode, ThemeMode::Dark);
        assert_eq!(t.version, 7);
        assert!(!t.builtin);
        assert_eq!(t.accent, def.accent);
        for (i, slot) in ThemeSlot::ALL[..22].iter().enumerate() {
            assert_eq!(slot.get(&t), def.slots[i]);
        }
        assert_eq!(t.derived.focus_ring, def.accent);
    }
}
