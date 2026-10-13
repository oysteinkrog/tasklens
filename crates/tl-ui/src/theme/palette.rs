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

/// The five device colour families used by Performance surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceAccent {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Net,
}

impl GraphPalette {
    /// The line colour is the device accent. Overview frames, charts, meters and sidebar cards
    /// all resolve through this table, so a theme cannot give one device two competing hues.
    pub const fn device_accent(&self, device: DeviceAccent) -> Hsla {
        match device {
            DeviceAccent::Cpu => self.cpu_line,
            DeviceAccent::Memory => self.mem_line,
            DeviceAccent::Gpu => self.gpu_line,
            DeviceAccent::Disk => self.disk_line,
            DeviceAccent::Net => self.net_line,
        }
    }
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

    #[test]
    fn device_accents_are_graph_line_tokens() {
        let color = |n| rgb(n).into();
        let p = GraphPalette {
            cpu_fill: color(1),
            cpu_line: color(2),
            kernel_line: color(3),
            mem_fill: color(4),
            mem_line: color(5),
            gpu_fill: color(6),
            gpu_line: color(7),
            disk_fill: color(8),
            disk_line: color(9),
            net_fill: color(10),
            net_line: color(11),
            grid: color(12),
            chart_bg: color(13),
        };
        assert_eq!(p.device_accent(DeviceAccent::Cpu), p.cpu_line);
        assert_eq!(p.device_accent(DeviceAccent::Memory), p.mem_line);
        assert_eq!(p.device_accent(DeviceAccent::Gpu), p.gpu_line);
        assert_eq!(p.device_accent(DeviceAccent::Disk), p.disk_line);
        assert_eq!(p.device_accent(DeviceAccent::Net), p.net_line);
    }
}
