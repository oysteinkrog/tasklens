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
    ChevronsRight,
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
    RotateCcw,
    Minus,
    Plus,
    Eye,
    EyeOff,
    ArrowUp,
    ArrowDown,
    Square,
    Binary,
    LayoutGrid,
    Rows2,
    ListTree,
    List,
}

impl TlIcon {
    pub const ALL: [Self; 32] = [
        Self::Check,
        Self::ChevronRight,
        Self::ChevronsRight,
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
        Self::RotateCcw,
        Self::Minus,
        Self::Plus,
        Self::Eye,
        Self::EyeOff,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::Square,
        Self::Binary,
        Self::LayoutGrid,
        Self::Rows2,
        Self::ListTree,
        Self::List,
    ];

    /// The kit name for this icon.
    pub fn kit_name(self) -> IconName {
        match self {
            Self::Check => IconName::Check,
            Self::ChevronRight => IconName::ChevronRight,
            Self::ChevronsRight => IconName::ChevronsRight,
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
            Self::RotateCcw => IconName::RotateCcw,
            Self::Minus => IconName::Minus,
            Self::Plus => IconName::Plus,
            Self::Eye => IconName::Eye,
            Self::EyeOff => IconName::EyeOff,
            Self::ArrowUp => IconName::ArrowUp,
            Self::ArrowDown => IconName::ArrowDown,
            Self::Square => IconName::Square,
            Self::Binary => IconName::Binary,
            Self::LayoutGrid => IconName::LayoutGrid,
            Self::Rows2 => IconName::Rows2,
            Self::ListTree => IconName::ListTree,
            Self::List => IconName::List,
        }
    }
}

impl IconNamed for TlIcon {
    fn path(self) -> SharedString {
        self.kit_name().path()
    }
}

/// tasklens's own SVGs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppIcon {
    /// The unlocked shield (APP-003, UI-FND-028). The locked state uses Lucide `shield`.
    ShieldUnlocked,
    /// The app logo (UI-FND-003). A placeholder until E15-026 (D-014).
    Logo,
    /// A chip with "P": a performance core on a hybrid CPU (UI-PWN-021).
    CpuPCore,
    /// A chip with "E": an efficiency core on a hybrid CPU (UI-PWN-021).
    CpuECore,
}

impl AppIcon {
    pub const ALL: [Self; 4] = [
        Self::ShieldUnlocked,
        Self::Logo,
        Self::CpuPCore,
        Self::CpuECore,
    ];

    pub fn path_str(self) -> &'static str {
        match self {
            Self::ShieldUnlocked => "icons/tl/shield-unlocked.svg",
            Self::Logo => "icons/tl/tasklens.svg",
            Self::CpuPCore => "icons/tl/cpu-pcore.svg",
            Self::CpuECore => "icons/tl/cpu-ecore.svg",
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Self::ShieldUnlocked => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/shield-unlocked.svg"
            )),
            Self::Logo => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/tasklens.svg"
            )),
            Self::CpuPCore => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/cpu-pcore.svg"
            )),
            Self::CpuECore => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/cpu-ecore.svg"
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
// arrows (R-185). `Binary` is the Name cell's file icon fallback. `LayoutGrid` to `List` are
// the Processes toolbar icons (UI-PRC-019). `ChevronsRight` is the tab overflow button
// (E20-015). `CircleOff` and `ShieldAlert` are the Properties exited banner and access block
// (E23-A1). `Activity` to `SlidersHorizontal` are the Settings window page icons (E21-018).
// `RotateCcw` is the Set Affinity Reset button (E23-005). `Archive` is the Memory compression
// pseudo row (UI-PRC-002). `Cog` and `FileCode` are Startup Apps Name icon fallbacks
// (UI-SYS-041).
gpui_kit::assets::icon_assets!(
    pub TlExtraIcons,
    [
        X, Pin, PinOff, Lock, Shield, ShieldCheck, RotateCcwClock, RotateCcw, ArrowUp, ArrowDown,
        Square,
        Binary, LayoutGrid, Rows2, ListTree, List, ChevronsRight, CircleOff, ShieldAlert, Activity,
        Scaling, Gauge, ChartArea, Server, AppWindow, SlidersHorizontal, Archive, Cog, FileCode
    ]
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
        let own: [(&str, &[u8]); 2] = [
            (
                "icons/tl/shield-unlocked.svg",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../assets/icons/shield-unlocked.svg"
                )),
            ),
            (
                "icons/tl/tasklens.svg",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../assets/icons/tasklens.svg"
                )),
            ),
        ];
        for (path, bytes) in own {
            let loaded = TlAssets.load(path).unwrap().unwrap();
            assert_eq!(&*loaded, bytes, "{path}");
            assert!(matches!(loaded, Cow::Borrowed(_)));
        }

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
