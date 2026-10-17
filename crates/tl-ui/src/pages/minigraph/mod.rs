//! Minigraph cards over the Processes tab (E26, PERF-080, UI-PRF-040). `sources` holds the pure
//! model; the panel, card, menu and actions modules build on it.

pub mod actions;
pub mod bridge;
pub mod card;
pub mod menu;
pub mod panel;
pub mod sources;

use gpui_kit::private::schemars::JsonSchema;
use gpui_kit::{Hsla, SharedString};
use serde::{Deserialize, Serialize};

use crate::theme::TlTheme;
use crate::theme::palette::accent_color;

/// A card's graph colour (R-263, R-083): the accent colour or one of the 17 APP-039 names. One
/// type for E26 and the E21-032 Graph Color submenu. Its JSON form is [`GraphColor::as_str`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "gpui_kit::private::schemars")]
pub enum GraphColor {
    Accent,
    Coral,
    Orange,
    Amber,
    Yellow,
    Lime,
    Green,
    Mint,
    Teal,
    Cyan,
    Blue,
    Indigo,
    Violet,
    Lavender,
    Fuchsia,
    Pink,
    Rose,
    Slate,
}

impl GraphColor {
    pub const ALL: [GraphColor; 18] = [
        Self::Accent,
        Self::Coral,
        Self::Orange,
        Self::Amber,
        Self::Yellow,
        Self::Lime,
        Self::Green,
        Self::Mint,
        Self::Teal,
        Self::Cyan,
        Self::Blue,
        Self::Indigo,
        Self::Violet,
        Self::Lavender,
        Self::Fuchsia,
        Self::Pink,
        Self::Rose,
        Self::Slate,
    ];

    /// The stored `state.toml` value: `"accent"` or the lower-case name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accent => "accent",
            Self::Coral => "coral",
            Self::Orange => "orange",
            Self::Amber => "amber",
            Self::Yellow => "yellow",
            Self::Lime => "lime",
            Self::Green => "green",
            Self::Mint => "mint",
            Self::Teal => "teal",
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Indigo => "indigo",
            Self::Violet => "violet",
            Self::Lavender => "lavender",
            Self::Fuchsia => "fuchsia",
            Self::Pink => "pink",
            Self::Rose => "rose",
            Self::Slate => "slate",
        }
    }

    /// The inverse of [`GraphColor::as_str`]. `None` for any other string.
    // The bead contract names it `from_str` with an `Option` result; bj6.35 imports it so.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// The display name, for example "Accent" or "Lavender".
    pub fn label(&self) -> SharedString {
        let s = self.as_str();
        let mut label = String::with_capacity(s.len());
        let mut chars = s.chars();
        if let Some(first) = chars.next() {
            label.push(first.to_ascii_uppercase());
        }
        label.extend(chars);
        SharedString::from(label)
    }

    /// The colour to draw with: the theme accent for `Accent`, else the APP-039 colour from
    /// the one table in `theme::palette::ACCENT_COLORS`.
    pub fn hsla(&self, theme: &TlTheme) -> Hsla {
        match self {
            Self::Accent => theme.accent,
            named => accent_color(named.as_str()).unwrap_or(theme.accent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_color_str_roundtrip() {
        for color in GraphColor::ALL {
            assert_eq!(GraphColor::from_str(color.as_str()), Some(color));
        }
        assert_eq!(GraphColor::from_str("magenta"), None);
        assert_eq!(GraphColor::from_str("Accent"), None);
        assert_eq!(GraphColor::from_str(""), None);
        assert_eq!(GraphColor::Accent.label().as_ref(), "Accent");
        assert_eq!(GraphColor::Lavender.label().as_ref(), "Lavender");
    }
}
