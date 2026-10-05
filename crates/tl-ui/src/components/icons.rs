//! Icons: the Lucide names tasklens uses, the app SVGs and the asset source (UI-FND-003, R-185).
//!
//! Lucide icons come from gpui-kit-assets 0.7.0, which bundles Lucide 1.43.0 (its `lucide.json`).
//! Every `TlIcon` name was checked against `gpui_kit::assets::IconName`; none needed a rename.
//! App SVGs live in `assets/icons/` and are served under `icons/tl/`.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, IconName, IconNamed};
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, Hsla, Pixels, Result, SharedString, Styled};

/// Every Lucide icon tasklens draws (UI-FND-003, UI-PRC-005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TlIcon {
    Check,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    X,
    Search,
    Pin,
    PinOff,
    Lock,
    Shield,
    ShieldCheck,
    Info,
    CircleCheck,
    TriangleAlert,
    CircleX,
    LoaderCircle,
    Copy,
    RotateCcwClock,
    Minus,
    Plus,
    Eye,
    EyeOff,
    ArrowUp,
    ArrowDown,
}

impl TlIcon {
    pub const ALL: [Self; 24] = [
        Self::Check,
        Self::ChevronRight,
        Self::ChevronDown,
        Self::ChevronUp,
        Self::X,
        Self::Search,
        Self::Pin,
        Self::PinOff,
        Self::Lock,
        Self::Shield,
        Self::ShieldCheck,
        Self::Info,
        Self::CircleCheck,
        Self::TriangleAlert,
        Self::CircleX,
        Self::LoaderCircle,
        Self::Copy,
        Self::RotateCcwClock,
        Self::Minus,
        Self::Plus,
        Self::Eye,
        Self::EyeOff,
        Self::ArrowUp,
        Self::ArrowDown,
    ];

    /// The kit name for this icon.
    pub fn kit_name(self) -> IconName {
        match self {
            Self::Check => IconName::Check,
            Self::ChevronRight => IconName::ChevronRight,
            Self::ChevronDown => IconName::ChevronDown,
            Self::ChevronUp => IconName::ChevronUp,
            Self::X => IconName::X,
            Self::Search => IconName::Search,
            Self::Pin => IconName::Pin,
            Self::PinOff => IconName::PinOff,
            Self::Lock => IconName::Lock,
            Self::Shield => IconName::Shield,
            Self::ShieldCheck => IconName::ShieldCheck,
            Self::Info => IconName::Info,
            Self::CircleCheck => IconName::CircleCheck,
            Self::TriangleAlert => IconName::TriangleAlert,
            Self::CircleX => IconName::CircleX,
            Self::LoaderCircle => IconName::LoaderCircle,
            Self::Copy => IconName::Copy,
            Self::RotateCcwClock => IconName::RotateCcwClock,
            Self::Minus => IconName::Minus,
            Self::Plus => IconName::Plus,
            Self::Eye => IconName::Eye,
            Self::EyeOff => IconName::EyeOff,
            Self::ArrowUp => IconName::ArrowUp,
            Self::ArrowDown => IconName::ArrowDown,
        }
    }
}

impl IconNamed for TlIcon {
    fn path(self) -> SharedString {
        self.kit_name().path()
    }
}

/// tasklens's own SVGs. `Tasklens` (the logo) is added by E15-022.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppIcon {
    /// The unlocked shield (APP-003, UI-FND-028). The locked state uses Lucide `shield`.
    ShieldUnlocked,
}

impl AppIcon {
    pub const ALL: [Self; 1] = [Self::ShieldUnlocked];

    pub fn path_str(self) -> &'static str {
        match self {
            Self::ShieldUnlocked => "icons/tl/shield-unlocked.svg",
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Self::ShieldUnlocked => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/shield-unlocked.svg"
            )),
        }
    }
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        self.path_str().into()
    }
}

// The icons not in the kit's default component bundle (`default-icons.txt`), plus the sort
// arrows (R-185).
gpui_kit::assets::icon_assets!(
    pub TlExtraIcons,
    [X, Pin, PinOff, Lock, Shield, ShieldCheck, RotateCcwClock, ArrowUp, ArrowDown]
);

/// The app's asset source: tasklens SVGs first, then `TlExtraIcons`, then the kit bundle.
/// A path none of them has gives `Ok(None)`, like the kit's `icon_assets!` sources. All bytes
/// are borrowed from static storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct TlAssets;

impl AssetSource for TlAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(app) = AppIcon::ALL.iter().find(|a| a.path_str() == path) {
            return Ok(Some(Cow::Borrowed(app.bytes())));
        }
        if let Some(bytes) = TlExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        // The kit bundle reports a missing path as an error.
        Ok(Assets.load(path).ok().flatten())
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> = AppIcon::ALL
            .iter()
            .map(|a| a.path_str())
            .filter(|p| p.starts_with(path))
            .map(Into::into)
            .collect();
        paths.extend(TlExtraIcons.list(path)?);
        paths.extend(Assets.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// An icon at a size from `UiMetrics.icon` (`xs` 12, `sm` 14, `md` 16, `lg` 20), tinted `color`.
pub fn icon(name: TlIcon, size: Pixels, color: Hsla) -> Icon {
    Icon::new(name).size(size).text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads() {
        for name in TlIcon::ALL {
            let path = name.path();
            let bytes = TlAssets.load(&path).unwrap();
            assert!(bytes.is_some_and(|b| !b.is_empty()), "{path} did not load");
        }
    }

    #[test]
    fn asset_source_prefers_tasklens() {
        let own = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/shield-unlocked.svg"
        ));
        let loaded = TlAssets
            .load("icons/tl/shield-unlocked.svg")
            .unwrap()
            .unwrap();
        assert_eq!(&*loaded, own);
        assert!(matches!(loaded, Cow::Borrowed(_)));

        let kit = Assets.load("icons/chevron-down.svg").unwrap().unwrap();
        let loaded = TlAssets.load("icons/chevron-down.svg").unwrap().unwrap();
        assert_eq!(&*loaded, &*kit);

        assert!(TlAssets.load("icons/tl/no-such.svg").unwrap().is_none());
    }

    #[test]
    fn list_merges_sorted_and_unique() {
        let all = TlAssets.list("icons/").unwrap();
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert!(
            all.iter()
                .any(|p| p.as_ref() == "icons/tl/shield-unlocked.svg")
        );
        assert!(all.iter().any(|p| p.as_ref() == "icons/pin-off.svg"));
        assert!(all.iter().any(|p| p.as_ref() == "icons/chevron-down.svg"));
    }
}
