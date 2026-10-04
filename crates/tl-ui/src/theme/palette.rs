//! Graph palette and the accent colour list (APP-035, APP-039, R-119).

use gpui_kit::{Hsla, rgb};

/// Colours for every chart kind. `chart_bg` is the plot area fill (R-119).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphPalette {
    pub cpu_fill: Hsla,
    pub cpu_line: Hsla,
    pub kernel_line: Hsla,
    pub mem_fill: Hsla,
    pub mem_line: Hsla,
    pub gpu_fill: Hsla,
    pub gpu_line: Hsla,
    pub disk_fill: Hsla,
    pub disk_line: Hsla,
    pub net_fill: Hsla,
    pub net_line: Hsla,
    pub grid: Hsla,
    pub chart_bg: Hsla,
}

/// The graph colour setting value that means "use the theme accent".
pub const GRAPH_COLOR_ACCENT: &str = "accent";

/// The 17 accent colours in menu order (APP-039). state.toml stores the name in lower case.
/// These hex values are fallbacks until U-18 replaces them.
pub const ACCENT_COLORS: [(&str, u32); 17] = [
    ("Coral", 0xFF7F50),
    ("Orange", 0xF28C28),
    ("Amber", 0xF5B700),
    ("Yellow", 0xE8D44D),
    ("Lime", 0x9BCB3C),
    ("Green", 0x4CAF50),
    ("Mint", 0x3EB489),
    ("Teal", 0x14A098),
    ("Cyan", 0x22B8CF),
    ("Blue", 0x3B82F6),
    ("Indigo", 0x6366F1),
    ("Violet", 0x8B5CF6),
    ("Lavender", 0xB4A7F5),
    ("Fuchsia", 0xD946EF),
    ("Pink", 0xEC4899),
    ("Rose", 0xF43F5E),
    ("Slate", 0x64748B),
];

/// Looks up an accent colour by name, ignoring case. Returns `None` for an unknown name.
pub fn accent_color(name_lower: &str) -> Option<Hsla> {
    ACCENT_COLORS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(name_lower))
        .map(|&(_, hex)| rgb(hex).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seventeen_accent_colours() {
        assert_eq!(ACCENT_COLORS.len(), 17);
        let names: Vec<&str> = ACCENT_COLORS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "Coral", "Orange", "Amber", "Yellow", "Lime", "Green", "Mint", "Teal", "Cyan",
                "Blue", "Indigo", "Violet", "Lavender", "Fuchsia", "Pink", "Rose", "Slate",
            ]
        );
        let coral: Hsla = rgb(0xFF7F50).into();
        assert_eq!(accent_color("coral"), Some(coral));
        assert_eq!(accent_color("Coral"), Some(coral));
        assert_eq!(accent_color("magenta"), None);
        for (name, hex) in ACCENT_COLORS {
            let lower = name.to_ascii_lowercase();
            assert_eq!(accent_color(&lower), Some(rgb(hex).into()));
        }
    }
}
