//! `ThemeStore`: the built-in and user theme definitions, the active `TlTheme` and the optional
//! preview, plus the instant theme switch (ADR-009 "Theme switch", UI-FND-040, UI-SHL-080).
//!
//! The store is a GPUI Global, so it cannot emit events. `activate` returns a `ThemeEvent` and
//! the caller turns it into settings writes and toasts.
//!
//! Kit note (R-274): `Theme::update` reloads the kit's registered theme config when `mode`
//! changes inside the closure, which overwrites the mapped colours. A mode switch therefore sets
//! `mode` in one `Theme::update` and runs `map::apply` in a second one.

use std::sync::Arc;

use gpui_kit::component::theme::{Theme, ThemeMode as KitMode};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, Global, SharedString, gpui};
use serde::Deserialize;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tracing::warn;

use super::builtin::{self, TS_THEME_ORDER};
use super::metrics;
use super::{
    Derived, GraphPalette, StatusColors, ThemeDef, ThemeMode, TlTheme, map, rebuild_derived,
};

/// The theme used when no other name resolves.
const DEFAULT_DARK: &str = "Default Dark";
const DEFAULT_LIGHT: &str = "Default Light";

/// Every theme definition and the theme that renders. Installed by `init`.
pub struct ThemeStore {
    /// From `builtin::load()`: 2 in P1, 12 in P2, in TaskSlinger order.
    pub builtin: Arc<[ThemeDef]>,
    /// From themes.json (E17-008). Empty until then.
    pub user: Vec<ThemeDef>,
    pub active: Arc<TlTheme>,
    /// Set only by the theme editor (E17-A2). When `Some`, it is what renders.
    pub preview: Option<Arc<TlTheme>>,
    /// Portal color-scheme 0, 1 or 2 (E17-007). 0 until then.
    pub system_scheme: u8,
    /// Source of `TlTheme.version`.
    version_counter: u64,
    /// Default Dark and Default Light. `TlTheme::from_def` takes the status and graph colours
    /// from the one of the same mode when a def has none.
    fallback_dark: TlTheme,
    fallback_light: TlTheme,
    /// The `[appearance]` values the derived tints were built with.
    appearance: AppearanceSettings,
}

impl Global for ThemeStore {}

/// Who asked for a theme switch. The caller writes `theme.active` only for `User`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateSource {
    User,
    FollowSystem,
}

/// What a theme store call did. Returned, not emitted, because the store is a Global.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeEvent {
    Activated { name: Arc<str> },
    Deleted { name: Arc<str> },
    Copied { name: Arc<str> },
    ImportFailed,
}

/// Why a theme store call was refused. The shell turns these into toasts (UI-SHL-038).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeError {
    UnknownTheme(Arc<str>),
    /// The theme editor is open: "Finish or cancel the theme editor first" (UI-FND-043).
    PreviewActive,
}

/// Switches to the named theme. Handled by the shell (E21-012).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ActivateTheme {
    pub name: SharedString,
}

impl ThemeStore {
    /// Finds `name`: user themes first, then built-ins. A user theme may shadow a built-in name
    /// (UI-FND-040 step 1). The flag is `true` for a built-in.
    pub fn resolve(&self, name: &str) -> Option<(&ThemeDef, bool)> {
        self.user
            .iter()
            .find(|d| &*d.name == name)
            .map(|d| (d, false))
            .or_else(|| {
                self.builtin
                    .iter()
                    .find(|d| &*d.name == name)
                    .map(|d| (d, true))
            })
    }

    fn fallback(&self, mode: ThemeMode) -> &TlTheme {
        match mode {
            ThemeMode::Dark => &self.fallback_dark,
            ThemeMode::Light => &self.fallback_light,
        }
    }

    /// Builds a `TlTheme` from `def` with the next version number.
    fn build(&mut self, def: &ThemeDef, builtin: bool) -> TlTheme {
        self.version_counter += 1;
        let fallback = self.fallback(def.effective_mode());
        TlTheme::from_def(
            def,
            fallback,
            &self.appearance,
            builtin,
            self.version_counter,
        )
    }
}

/// A theme with every colour transparent and the given status and graph colours. Only used as
/// the `fallback` for the first build of a default theme, which overwrites every other field.
fn seed(status: StatusColors, graph: GraphPalette) -> TlTheme {
    let c = gpui_kit::Hsla::default();
    TlTheme {
        name: "".into(),
        mode: ThemeMode::Dark,
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
        graph,
        status,
        derived: Derived::empty(),
    }
}

/// Builds Default Dark or Default Light from the built-in defs. The embedded file must carry
/// both with their status and graph colours; the builtin tests check that.
fn default_theme(builtin: &[ThemeDef], name: &str, appearance: &AppearanceSettings) -> TlTheme {
    let def = builtin
        .iter()
        .find(|d| &*d.name == name)
        .unwrap_or_else(|| panic!("built-in theme {name} is missing"));
    let (Some(status), Some(graph)) = (def.status, def.graph) else {
        panic!("built-in theme {name} has no status or graph colours");
    };
    TlTheme::from_def(def, &seed(status, graph), appearance, true, 0)
}

/// Copies `tl` and the installed metrics onto the kit `Theme`. A mode change gets its own
/// `Theme::update` first (R-274), so `map::apply` runs exactly once with the final theme.
fn apply_kit(tl: &TlTheme, cx: &mut App) {
    let m = *metrics::metrics(cx);
    let mode = match tl.mode {
        ThemeMode::Dark => KitMode::Dark,
        ThemeMode::Light => KitMode::Light,
    };
    if Theme::global(cx).mode != mode {
        Theme::update(cx, |kit| kit.mode = mode);
    }
    Theme::update(cx, |kit| map::apply(tl, &m, kit));
}

/// Installs `ThemeStore` and `UiMetrics` and maps the active theme onto the kit `Theme`, with
/// no crossfade (UI-SHL-110 step 8). Runs after `gpui_kit::init`. An unknown `theme.active`
/// falls back to `theme.dark` and then to Default Dark, with a warning.
pub fn init(
    cx: &mut App,
    theme: &ThemeSettings,
    appearance: &AppearanceSettings,
    user: Vec<ThemeDef>,
) {
    let builtin = builtin::load();
    let fallback_dark = default_theme(&builtin, DEFAULT_DARK, appearance);
    let fallback_light = default_theme(&builtin, DEFAULT_LIGHT, appearance);
    let mut store = ThemeStore {
        builtin,
        user,
        active: Arc::new(fallback_dark.clone()),
        preview: None,
        system_scheme: 0,
        version_counter: 0,
        fallback_dark,
        fallback_light,
        appearance: appearance.clone(),
    };

    let mut name = theme.active.as_str();
    if store.resolve(name).is_none() {
        warn!(theme = name, fallback = %theme.dark, "unknown active theme");
        name = theme.dark.as_str();
        if store.resolve(name).is_none() {
            warn!(theme = name, fallback = DEFAULT_DARK, "unknown dark theme");
            name = DEFAULT_DARK;
        }
    }
    let Some((def, is_builtin)) = store.resolve(name) else {
        unreachable!("Default Dark is always built in");
    };
    let def = def.clone();
    let active = Arc::new(store.build(&def, is_builtin));
    store.active = active.clone();

    metrics::init_metrics(appearance, cx);
    cx.set_global(store);
    apply_kit(&active, cx);
}

/// The theme that renders: `preview` when set, else `active`. Views never read `active`
/// directly (UI-FND-043 rule 2).
pub fn current(cx: &App) -> &Arc<TlTheme> {
    let store = cx.global::<ThemeStore>();
    store.preview.as_ref().unwrap_or(&store.active)
}

/// Switches to `name` at once, without a crossfade. Bumps `style_version` once and maps the new
/// theme onto the kit once. Writes no settings and pushes no toast: with `User` the caller
/// writes `theme.active` and turns off sync (UI-SHL-080). `CellTextCache` is not cleared, since
/// text colour is not part of its key.
pub fn activate(
    name: &str,
    _source: ActivateSource,
    cx: &mut App,
) -> Result<ThemeEvent, ThemeError> {
    let store = cx.global::<ThemeStore>();
    if store.preview.is_some() {
        return Err(ThemeError::PreviewActive);
    }
    let Some((def, is_builtin)) = store.resolve(name) else {
        return Err(ThemeError::UnknownTheme(name.into()));
    };
    let def = def.clone();

    let store = cx.global_mut::<ThemeStore>();
    let active = Arc::new(store.build(&def, is_builtin));
    store.active = active.clone();

    metrics::bump_style_version(cx);
    apply_kit(&active, cx);
    cx.refresh_windows();
    Ok(ThemeEvent::Activated { name: def.name })
}

/// Applies `[appearance]` changes. A metrics change (scale, row height, rounded corners)
/// installs new `UiMetrics`; a `shadow_intensity_pct` change rebuilds the derived tints of the
/// active theme. Either one bumps `style_version` once, maps the current theme onto the kit and
/// refreshes the windows. Unchanged values do nothing (UI-SHL-130 steps 1 to 3).
pub fn apply_metrics(appearance: &AppearanceSettings, cx: &mut App) {
    let metrics_changed = metrics::recompute(appearance, cx);

    let store = cx.global_mut::<ThemeStore>();
    let shadow_changed = store.appearance.shadow_intensity_pct != appearance.shadow_intensity_pct;
    store.appearance = appearance.clone();
    if shadow_changed {
        store.version_counter += 1;
        let mut active = (*store.active).clone();
        rebuild_derived(&mut active, appearance);
        active.version = store.version_counter;
        store.active = Arc::new(active);
        if !metrics_changed {
            metrics::bump_style_version(cx);
        }
    }

    if metrics_changed || shadow_changed {
        let tl = current(cx).clone();
        apply_kit(&tl, cx);
        cx.refresh_windows();
    }
}

/// Theme names in menu order: built-ins in `TS_THEME_ORDER`, then user themes in file order.
/// The flag is `true` for a built-in.
pub fn names(cx: &App) -> Vec<(Arc<str>, bool)> {
    let store = cx.global::<ThemeStore>();
    let rank = |name: &str| {
        TS_THEME_ORDER
            .iter()
            .position(|n| *n == name)
            .unwrap_or(TS_THEME_ORDER.len())
    };
    let mut builtin: Vec<&ThemeDef> = store.builtin.iter().collect();
    builtin.sort_by_key(|d| rank(&d.name));
    builtin
        .into_iter()
        .map(|d| (d.name.clone(), true))
        .chain(store.user.iter().map(|d| (d.name.clone(), false)))
        .collect()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Hsla, TestAppContext, px, rgb};

    use super::*;
    use crate::theme::ThemeSlot;

    const WINDOW_BG: usize = ThemeSlot::WindowBg as usize;

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    fn setup(cx: &mut App, user: Vec<ThemeDef>) {
        gpui_kit::init(cx);
        init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            user,
        );
    }

    /// A user theme named "Default Dark" with a black window background.
    fn shadowing_user_theme() -> ThemeDef {
        let mut def = builtin::load()
            .iter()
            .find(|d| &*d.name == DEFAULT_DARK)
            .cloned()
            .unwrap();
        def.slots[WINDOW_BG] = hex(0x000000);
        def.status = None;
        def.graph = None;
        def
    }

    fn style_version(cx: &App) -> u64 {
        metrics::metrics(cx).style_version
    }

    #[gpui_kit::test]
    fn activate_user_shadows_builtin(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, vec![shadowing_user_theme()]);
            let r = activate(DEFAULT_DARK, ActivateSource::User, cx);
            assert_eq!(
                r,
                Ok(ThemeEvent::Activated {
                    name: DEFAULT_DARK.into()
                })
            );
            let t = current(cx);
            assert!(!t.builtin);
            assert_eq!(t.window_bg, hex(0x000000));
            // Status and graph come from the built-in Default Dark.
            let fallback = &cx.global::<ThemeStore>().fallback_dark;
            assert_eq!(t.status, fallback.status);
            assert_eq!(t.graph, fallback.graph);
            assert_eq!(Theme::global(cx).colors.background, hex(0x000000));

            let names = names(cx);
            assert_eq!(names.first(), Some(&(DEFAULT_DARK.into(), true)));
            assert_eq!(names.last(), Some(&(DEFAULT_DARK.into(), false)));
        });
    }

    #[gpui_kit::test]
    fn animations_off_is_instant_and_single_rebuild(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            assert_eq!(&*current(cx).name, DEFAULT_DARK);
            let n = style_version(cx);
            let version = current(cx).version;

            let r = activate(DEFAULT_LIGHT, ActivateSource::User, cx);
            assert_eq!(
                r,
                Ok(ThemeEvent::Activated {
                    name: DEFAULT_LIGHT.into()
                })
            );
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
            assert_eq!(current(cx).mode, ThemeMode::Light);
            assert!(current(cx).builtin);
            assert_eq!(current(cx).version, version + 1);
            assert_eq!(style_version(cx), n + 1);
            let kit = Theme::global(cx);
            assert_eq!(kit.mode, KitMode::Light);
            assert_eq!(kit.colors.tab_active, hex(0xF3F4F6));
            assert_eq!(kit.colors.background, hex(0xF3F4F6));

            // Back to dark: the mode switch must not leave the kit's own dark colours behind.
            activate(DEFAULT_DARK, ActivateSource::FollowSystem, cx).unwrap();
            assert_eq!(style_version(cx), n + 2);
            let kit = Theme::global(cx);
            assert_eq!(kit.mode, KitMode::Dark);
            assert_eq!(kit.colors.background, hex(0x191B1D));
        });
    }

    #[gpui_kit::test]
    fn unknown_name_and_preview_refused(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            let before = current(cx).clone();
            let n = style_version(cx);

            assert_eq!(
                activate("Missing", ActivateSource::User, cx),
                Err(ThemeError::UnknownTheme("Missing".into()))
            );
            assert!(Arc::ptr_eq(current(cx), &before));
            assert_eq!(style_version(cx), n);

            let mut p = (*before).clone();
            p.name = "Preview".into();
            let p = Arc::new(p);
            cx.global_mut::<ThemeStore>().preview = Some(p.clone());
            assert!(Arc::ptr_eq(current(cx), &p));

            assert_eq!(
                activate(DEFAULT_LIGHT, ActivateSource::User, cx),
                Err(ThemeError::PreviewActive)
            );
            assert!(Arc::ptr_eq(&cx.global::<ThemeStore>().active, &before));
            assert_eq!(style_version(cx), n);
        });
    }

    #[gpui_kit::test]
    fn apply_metrics_updates_kit_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            assert_eq!(Theme::global(cx).font_size, px(13.0));
            let n = style_version(cx);

            apply_metrics(&AppearanceSettings::default(), cx);
            assert_eq!(style_version(cx), n);
            assert_eq!(Theme::global(cx).font_size, px(13.0));

            let scaled = AppearanceSettings {
                ui_scale_pct: 150,
                ..AppearanceSettings::default()
            };
            apply_metrics(&scaled, cx);
            assert_eq!(style_version(cx), n + 1);
            assert_eq!(Theme::global(cx).font_size, px(19.5));
            assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));

            apply_metrics(&scaled, cx);
            assert_eq!(style_version(cx), n + 1);

            let flat = AppearanceSettings {
                shadow_intensity_pct: 0,
                ..scaled
            };
            apply_metrics(&flat, cx);
            assert_eq!(style_version(cx), n + 2);
            assert_eq!(current(cx).derived.shadow.a, 0.0);
            assert!(!Theme::global(cx).shadow);
        });
    }
}
