//! `ThemeStore`: the built-in and user theme definitions, the active `TlTheme` and the optional
//! preview, plus the theme switch (ADR-009 "Theme switch", UI-FND-040, UI-SHL-080).
//!
//! With animations on (R-017) the switch is a 200 ms crossfade: each frame of the main window's
//! `AnimationDriver` replaces `active` with an interpolated theme. The store requests those frames
//! with `request_callback` under the id of a zero-sized entity it creates at init (R-209), so the
//! window still arms one `on_next_frame` per frame.
//!
//! The store is a GPUI Global, so it cannot emit events. `activate` returns a `ThemeEvent` and
//! the caller turns it into settings writes and toasts.
//!
//! Kit note (R-274): `Theme::update` reloads the kit's registered theme config when `mode`
//! changes inside the closure, which overwrites the mapped colours. A mode switch therefore sets
//! `mode` in one `Theme::update` and runs `map::apply` in a second one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::theme::{Theme, ThemeMode as KitMode};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, Entity, Global, Hsla, SharedString, Window, gpui,
};
use serde::Deserialize;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tracing::warn;

use super::builtin::{self, TS_THEME_ORDER};
use super::metrics;
use super::user_store::{self, LoadOutcome, PersistTheme};
use super::{
    Derived, GraphPalette, StatusColors, ThemeDef, ThemeMode, ThemeSlot, TlTheme, map,
    rebuild_derived,
};
use crate::anim::AnimationDriver;
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::settings_global::animations_on;

/// The theme used when no other name resolves.
pub(crate) const DEFAULT_DARK: &str = "Default Dark";
pub(crate) const DEFAULT_LIGHT: &str = "Default Light";

/// Every theme definition and the theme that renders. Installed by `init`.
pub struct ThemeStore {
    /// From `builtin::load()`: 2 in P1, 12 in P2, in TaskSlinger order.
    pub builtin: Arc<[ThemeDef]>,
    /// From themes.json (E17-008), in file order.
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
    /// The running crossfade. `None` when idle.
    crossfade: Option<Crossfade>,
    /// Its id is the theme's requester id on the `AnimationDriver` (R-209).
    anim: Entity<ThemeAnim>,
    /// The window whose `AnimationDriver` runs the crossfade. Set by `set_anim_window`. While
    /// `None` every switch is instant.
    anim_window: Option<AnyWindowHandle>,
}

impl Global for ThemeStore {}

/// Zero-sized entity that reserves the theme's requester id on the `AnimationDriver` (R-209).
struct ThemeAnim;

/// A theme crossfade from `from` to `to` (UI-FND-040 step 3).
struct Crossfade {
    from: Arc<TlTheme>,
    to: Arc<TlTheme>,
    /// From the background executor's clock, which tests advance.
    started: Instant,
    /// `m_base`, 200 ms.
    duration: Duration,
}

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
    /// "Built-in themes cannot be deleted" (UI-FND-042).
    BuiltinCannotBeDeleted,
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
    let outcome = LoadOutcome {
        themes: user,
        ..LoadOutcome::default()
    };
    init_store(cx, theme, appearance, builtin::load(), outcome);
}

/// `init` with the user themes parsed from the themes.json text (`None` when the file is
/// missing). A file that does not parse is reset: this sends `PersistTheme::RenameCorrupt`
/// through `ThemePersist` and returns the Warning toast text for the shell to queue.
pub fn init_with_file(
    cx: &mut App,
    theme: &ThemeSettings,
    appearance: &AppearanceSettings,
    themes_json: Option<&str>,
    now_unix: u64,
) -> Option<SharedString> {
    let builtin = builtin::load();
    let outcome = user_store::load_user_themes(themes_json, &builtin, now_unix);
    let corrupt = outcome.corrupt;
    if let Some(t) = corrupt {
        user_store::persist(PersistTheme::RenameCorrupt { unix_seconds: t }, cx);
    }
    init_store(cx, theme, appearance, builtin, outcome);
    corrupt.map(user_store::corrupt_toast_text)
}

fn init_store(
    cx: &mut App,
    theme: &ThemeSettings,
    appearance: &AppearanceSettings,
    builtin: Arc<[ThemeDef]>,
    outcome: LoadOutcome,
) {
    let fallback_dark = default_theme(&builtin, DEFAULT_DARK, appearance);
    let fallback_light = default_theme(&builtin, DEFAULT_LIGHT, appearance);
    let mut store = ThemeStore {
        builtin,
        user: Vec::new(),
        active: Arc::new(fallback_dark.clone()),
        preview: None,
        system_scheme: 0,
        version_counter: 0,
        fallback_dark,
        fallback_light,
        appearance: appearance.clone(),
        crossfade: None,
        anim: cx.new(|_| ThemeAnim),
        anim_window: None,
    };
    store.install_user(outcome);

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

/// Makes `window`'s `AnimationDriver` run the theme crossfade. Called once by `MainWindow::new`.
pub fn set_anim_window(window: AnyWindowHandle, cx: &mut App) {
    cx.global_mut::<ThemeStore>().anim_window = Some(window);
}

/// Switches to `name`. Writes no settings and pushes no toast: with `User` the caller writes
/// `theme.active` and turns off sync (UI-SHL-080). `CellTextCache` is not cleared, since text
/// colour is not part of its key.
///
/// With animations on and an animation window set, this starts a 200 ms crossfade from the
/// colours that render now, so a switch during a crossfade restarts from the interpolated
/// colours. It bumps `style_version` once here and once more when the crossfade ends, and maps
/// the theme onto the kit only at the end. Otherwise the switch is instant: one bump, one
/// `map::apply`.
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
    let animate = animations_on(cx);
    let now = cx.background_executor().now();

    let store = cx.global_mut::<ThemeStore>();
    let to = Arc::new(store.build(&def, is_builtin));
    let window = store.anim_window.filter(|_| animate);
    if let Some(window) = window {
        store.crossfade = Some(Crossfade {
            from: store.active.clone(),
            to,
            started: now,
            duration: M_BASE,
        });
        metrics::bump_style_version(cx);
        cx.refresh_windows();
        // Deferred: `activate` may run inside an update of that window, where it cannot be
        // updated again.
        cx.defer(move |cx| {
            let requested = window
                .update(cx, |_, window, cx| request_frame(window, cx))
                .unwrap_or(false);
            if !requested {
                finish_crossfade(cx);
            }
        });
    } else {
        store.crossfade = None;
        store.active = to.clone();
        metrics::bump_style_version(cx);
        apply_kit(&to, cx);
        cx.refresh_windows();
    }
    Ok(ThemeEvent::Activated { name: def.name })
}

/// Asks the window's driver to run `on_frame` on the next frame. `false` when the window has no
/// driver.
fn request_frame(window: &mut Window, cx: &mut App) -> bool {
    let Some(driver) = AnimationDriver::try_global(window, cx) else {
        return false;
    };
    let id = cx.global::<ThemeStore>().anim.entity_id();
    driver.update(cx, |d, cx| d.request_callback(id, on_frame, window, cx));
    true
}

/// The driver callback for the theme id: one crossfade step, and the next frame while it runs.
fn on_frame(window: &mut Window, cx: &mut App) {
    if tick_crossfade(cx) && !request_frame(window, cx) {
        finish_crossfade(cx);
    }
}

/// One crossfade frame: replaces `active` with the theme at `ease_out_cubic(elapsed / 200 ms)` and
/// refreshes the windows. Does not bump `style_version` and does not touch the kit. Once the
/// time is up it ends the crossfade (`finish_crossfade`). Returns `true` while another frame is
/// needed.
pub fn tick_crossfade(cx: &mut App) -> bool {
    let now = cx.background_executor().now();
    let store = cx.global_mut::<ThemeStore>();
    let Some(fade) = &store.crossfade else {
        return false;
    };
    let linear =
        now.saturating_duration_since(fade.started).as_secs_f32() / fade.duration.as_secs_f32();
    if linear >= 1.0 {
        finish_crossfade(cx);
        return false;
    }
    let t = ease_out_cubic(linear);
    store.active = Arc::new(lerp_theme(&fade.from, &fade.to, t));
    cx.refresh_windows();
    true
}

/// Ends a running crossfade: `active = to`, the second `style_version` bump and the one
/// `map::apply`. Does nothing when no crossfade runs.
fn finish_crossfade(cx: &mut App) {
    let store = cx.global_mut::<ThemeStore>();
    let Some(fade) = store.crossfade.take() else {
        return;
    };
    store.active = fade.to.clone();
    metrics::bump_style_version(cx);
    apply_kit(&fade.to, cx);
    cx.refresh_windows();
}

/// `a` moved toward `b` by `t` in Hsla space. Hue takes the shorter way round the circle.
/// `t <= 0` gives `a` and `t >= 1` gives `b` exactly.
fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let mix = |x: f32, y: f32| x + (y - x) * t;
    let mut dh = b.h - a.h;
    if dh > 0.5 {
        dh -= 1.0;
    } else if dh < -0.5 {
        dh += 1.0;
    }
    Hsla {
        h: (a.h + dh * t).rem_euclid(1.0),
        s: mix(a.s, b.s),
        l: mix(a.l, b.l),
        a: mix(a.a, b.a),
    }
}

/// The theme between `a` (`t = 0`) and `b` (`t = 1`): the 22 slots, the accent, the 4 status
/// colours and the 13 graph colours each lerp in Hsla space. Name, mode, version and `builtin`
/// come from `b`.
///
/// The derived tints lerp too, instead of a `rebuild_derived`, which needs the `[appearance]`
/// values. The colour-based tints come out the same as a rebuild, and shadow and overlay blend
/// between the two modes.
pub fn lerp_theme(a: &TlTheme, b: &TlTheme, t: f32) -> TlTheme {
    let l = |x: Hsla, y: Hsla| lerp_hsla(x, y, t);
    let mut out = b.clone();
    for slot in ThemeSlot::ALL {
        slot.set(&mut out, l(slot.get(a), slot.get(b)));
    }
    let (sa, sb) = (&a.status, &b.status);
    out.status = StatusColors {
        success: l(sa.success, sb.success),
        warning: l(sa.warning, sb.warning),
        danger: l(sa.danger, sb.danger),
        info: l(sa.info, sb.info),
    };
    let (ga, gb) = (&a.graph, &b.graph);
    out.graph = GraphPalette {
        cpu_fill: l(ga.cpu_fill, gb.cpu_fill),
        cpu_line: l(ga.cpu_line, gb.cpu_line),
        kernel_line: l(ga.kernel_line, gb.kernel_line),
        mem_fill: l(ga.mem_fill, gb.mem_fill),
        mem_line: l(ga.mem_line, gb.mem_line),
        gpu_fill: l(ga.gpu_fill, gb.gpu_fill),
        gpu_line: l(ga.gpu_line, gb.gpu_line),
        disk_fill: l(ga.disk_fill, gb.disk_fill),
        disk_line: l(ga.disk_line, gb.disk_line),
        net_fill: l(ga.net_fill, gb.net_fill),
        net_line: l(ga.net_line, gb.net_line),
        grid: l(ga.grid, gb.grid),
        chart_bg: l(ga.chart_bg, gb.chart_bg),
    };
    let (da, db) = (&a.derived, &b.derived);
    out.derived = Derived {
        new_row_tint: l(da.new_row_tint, db.new_row_tint),
        exited_row_tint: l(da.exited_row_tint, db.exited_row_tint),
        focus_ring: l(da.focus_ring, db.focus_ring),
        shadow: l(da.shadow, db.shadow),
        overlay: l(da.overlay, db.overlay),
        stale_text: l(da.stale_text, db.stale_text),
    };
    out
}

/// The preferred theme for a portal color-scheme value: `theme.light` for 2 (prefer light),
/// `theme.dark` for 0 (no preference), 1 (prefer dark) and any unknown value (UI-FND-040).
pub fn scheme_target(theme: &ThemeSettings, scheme: u8) -> &str {
    match scheme {
        2 => &theme.light,
        _ => &theme.dark,
    }
}

/// Records the portal color-scheme from the snapshot. With `theme.follow_system` on and a
/// changed scheme, activates the matching preferred theme with `FollowSystem`, which writes no
/// settings (UI-SHL-080). Returns `None` when nothing was activated.
pub fn on_system_scheme(
    scheme: u8,
    theme: &ThemeSettings,
    cx: &mut App,
) -> Option<Result<ThemeEvent, ThemeError>> {
    let store = cx.global_mut::<ThemeStore>();
    let changed = store.system_scheme != scheme;
    store.system_scheme = scheme;
    if !theme.follow_system || !changed {
        return None;
    }
    Some(activate(
        scheme_target(theme, scheme),
        ActivateSource::FollowSystem,
        cx,
    ))
}

/// Turning sync on applies the matching preferred theme at once (UI-SHL-080). Returns `None`
/// when that theme is already active. Turning sync off needs no call: the shell writes
/// `current()` to `theme.active` (UI-FND-040).
pub fn on_follow_system_enabled(
    theme: &ThemeSettings,
    cx: &mut App,
) -> Option<Result<ThemeEvent, ThemeError>> {
    let store = cx.global::<ThemeStore>();
    let target = scheme_target(theme, store.system_scheme);
    if &*store.active.name == target {
        return None;
    }
    Some(activate(target, ActivateSource::FollowSystem, cx))
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
        // A running crossfade ends on `to`, so it needs the new tints too.
        if let Some(fade) = &mut store.crossfade {
            let mut to = (*fade.to).clone();
            rebuild_derived(&mut to, appearance);
            fade.to = Arc::new(to);
        }
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
    use gpui_kit::{Context, IntoElement, Render, TestAppContext, VisualTestContext, div, px, rgb};
    use tl_model::settings::{Settings, UiState};

    use super::*;
    use crate::app::BootSettings;
    use crate::settings_global::TlSettings;

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

    /// Root view that owns the window's driver, as `MainWindow` does.
    struct Root {
        _driver: Entity<AnimationDriver>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// The store, `TlSettings` with `motion.animations = animations`, and a window with a driver
    /// set as the animation window.
    fn rig(cx: &mut TestAppContext, animations: bool) -> &mut VisualTestContext {
        cx.update(|cx| {
            setup(cx, Vec::new());
            let mut settings = Settings::default();
            settings.motion.animations = animations;
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            Root { _driver: driver }
        });
        vcx.update(|window, cx| set_anim_window(window.window_handle(), cx));
        vcx
    }

    /// Advances the test clock by `ms` and draws one frame. Returns the `on_next_frame`
    /// callbacks that ran.
    fn frame_after(vcx: &mut VisualTestContext, ms: u64) -> usize {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(ms));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.simulate_next_frame(cx))
    }

    fn switch(vcx: &mut VisualTestContext, name: &str) {
        vcx.update(|_, cx| activate(name, ActivateSource::User, cx).unwrap());
    }

    fn fading(vcx: &mut VisualTestContext) -> bool {
        vcx.update(|_, cx| cx.global::<ThemeStore>().crossfade.is_some())
    }

    #[gpui_kit::test]
    fn animations_off_is_instant_and_single_rebuild(cx: &mut TestAppContext) {
        let vcx = rig(cx, false);
        vcx.update(|_, cx| {
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
            assert!(cx.global::<ThemeStore>().crossfade.is_none());
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
        // No frame was requested.
        assert_eq!(frame_after(vcx, 16), 0);
    }

    #[gpui_kit::test]
    fn reduced_motion_is_instant(cx: &mut TestAppContext) {
        let vcx = rig(cx, true);
        vcx.update(|_, cx| {
            cx.set_reduce_motion(true);
            let n = style_version(cx);
            activate(DEFAULT_LIGHT, ActivateSource::User, cx).unwrap();
            assert!(cx.global::<ThemeStore>().crossfade.is_none());
            assert_eq!(current(cx).window_bg, hex(0xF3F4F6));
            assert_eq!(style_version(cx), n + 1);
            assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
        });
        assert_eq!(frame_after(vcx, 16), 0);
    }

    #[gpui_kit::test]
    fn crossfade_bumps_style_version_twice(cx: &mut TestAppContext) {
        let vcx = rig(cx, true);
        let n = vcx.update(|_, cx| style_version(cx));
        switch(vcx, DEFAULT_LIGHT);
        assert!(fading(vcx));
        assert_eq!(vcx.update(|_, cx| style_version(cx)), n + 1);

        // 16 frames of 16 ms: 256 ms of test time.
        for i in 0..16 {
            let running = fading(vcx);
            let callbacks = frame_after(vcx, 16);
            // Only the driver's own `on_next_frame` runs, one per frame, while the fade runs.
            assert_eq!(callbacks, usize::from(running), "frame {i}");
            if fading(vcx) {
                assert_eq!(vcx.update(|_, cx| style_version(cx)), n + 1, "frame {i}");
            }
        }
        vcx.update(|_, cx| {
            assert!(cx.global::<ThemeStore>().crossfade.is_none());
            assert_eq!(style_version(cx), n + 2);
            let t = current(cx);
            assert_eq!(&*t.name, DEFAULT_LIGHT);
            assert_eq!(t.window_bg, hex(0xF3F4F6));
            assert_eq!(t.mode, ThemeMode::Light);
            let kit = Theme::global(cx);
            assert_eq!(kit.mode, KitMode::Light);
            assert_eq!(kit.colors.background, hex(0xF3F4F6));
        });
        // Nothing is armed once the fade ended.
        assert_eq!(frame_after(vcx, 16), 0);
    }

    #[gpui_kit::test]
    fn crossfade_midpoint_is_between(cx: &mut TestAppContext) {
        let vcx = rig(cx, true);
        switch(vcx, DEFAULT_LIGHT);
        assert_eq!(frame_after(vcx, 100), 1);
        vcx.update(|_, cx| {
            let l = current(cx).window_bg.l;
            let (dark, light) = (hex(0x191B1D).l, hex(0xF3F4F6).l);
            assert!(dark < l && l < light, "{dark} < {l} < {light}");
            // The kit keeps the old theme until the end.
            assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
        });
    }

    #[gpui_kit::test]
    fn second_switch_restarts_from_current(cx: &mut TestAppContext) {
        let vcx = rig(cx, true);
        switch(vcx, DEFAULT_LIGHT);
        assert_eq!(frame_after(vcx, 100), 1);
        let mid = vcx.update(|_, cx| current(cx).window_bg);
        assert_ne!(mid, hex(0xF3F4F6));
        let n = vcx.update(|_, cx| style_version(cx));

        switch(vcx, DEFAULT_DARK);
        vcx.update(|_, cx| {
            let fade = cx.global::<ThemeStore>().crossfade.as_ref().unwrap();
            assert_eq!(fade.from.window_bg, mid);
            assert_eq!(fade.to.window_bg, hex(0x191B1D));
            assert_eq!(style_version(cx), n + 1);
        });
        for _ in 0..16 {
            frame_after(vcx, 16);
        }
        vcx.update(|_, cx| {
            assert!(cx.global::<ThemeStore>().crossfade.is_none());
            assert_eq!(style_version(cx), n + 2);
            assert_eq!(current(cx).window_bg, hex(0x191B1D));
            assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
        });
    }

    #[gpui_kit::test]
    fn no_anim_window_is_instant(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            let n = style_version(cx);
            activate(DEFAULT_LIGHT, ActivateSource::User, cx).unwrap();
            assert!(cx.global::<ThemeStore>().crossfade.is_none());
            assert_eq!(current(cx).window_bg, hex(0xF3F4F6));
            assert_eq!(style_version(cx), n + 1);
        });
    }

    /// The 23 slot colours, the 4 status colours and the 13 graph colours.
    fn colours(t: &TlTheme) -> Vec<Hsla> {
        let s = t.status;
        let g = t.graph;
        let mut v: Vec<Hsla> = ThemeSlot::ALL.iter().map(|slot| slot.get(t)).collect();
        v.extend([s.success, s.warning, s.danger, s.info]);
        v.extend([
            g.cpu_fill,
            g.cpu_line,
            g.kernel_line,
            g.mem_fill,
            g.mem_line,
            g.gpu_fill,
            g.gpu_line,
            g.disk_fill,
            g.disk_line,
            g.net_fill,
            g.net_line,
            g.grid,
            g.chart_bg,
        ]);
        v
    }

    #[gpui_kit::test]
    fn lerp_theme_endpoints(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            let store = cx.global::<ThemeStore>();
            let (a, b) = (&store.fallback_dark, &store.fallback_light);
            assert_eq!(colours(a).len(), 40);

            let start = lerp_theme(a, b, 0.0);
            assert_eq!(colours(&start), colours(a));
            assert_eq!(start.derived, a.derived);
            assert_eq!(&*start.name, DEFAULT_LIGHT);
            assert_eq!(start.mode, ThemeMode::Light);

            let end = lerp_theme(a, b, 1.0);
            assert_eq!(colours(&end), colours(b));
            assert_eq!(end.derived, b.derived);
            assert_eq!(end.version, b.version);
            assert_eq!(end.builtin, b.builtin);
        });
    }

    #[test]
    fn lerp_hue_takes_the_shorter_arc() {
        let c = |h: f32| Hsla {
            h,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let mid = lerp_hsla(c(0.9), c(0.1), 0.5);
        assert!(mid.h < 1e-5 || mid.h > 1.0 - 1e-5, "{}", mid.h);
        let mid = lerp_hsla(c(0.2), c(0.4), 0.5);
        assert!((mid.h - 0.3).abs() < 1e-6);
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

    fn follow(on: bool) -> ThemeSettings {
        ThemeSettings {
            follow_system: on,
            ..ThemeSettings::default()
        }
    }

    fn activated(name: &str) -> Option<Result<ThemeEvent, ThemeError>> {
        Some(Ok(ThemeEvent::Activated { name: name.into() }))
    }

    #[gpui_kit::test]
    fn follow_system_picks_light_for_2_dark_otherwise(cx: &mut TestAppContext) {
        let theme = follow(true);
        assert_eq!(scheme_target(&theme, 0), DEFAULT_DARK);
        assert_eq!(scheme_target(&theme, 1), DEFAULT_DARK);
        assert_eq!(scheme_target(&theme, 2), DEFAULT_LIGHT);
        assert_eq!(scheme_target(&theme, 7), DEFAULT_DARK);
        cx.update(|cx| {
            setup(cx, Vec::new());
            assert_eq!(&*current(cx).name, DEFAULT_DARK);

            assert_eq!(on_system_scheme(2, &theme, cx), activated(DEFAULT_LIGHT));
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
            assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
            let n = style_version(cx);
            // The same scheme again does not activate.
            assert_eq!(on_system_scheme(2, &theme, cx), None);
            assert_eq!(style_version(cx), n);

            assert_eq!(on_system_scheme(1, &theme, cx), activated(DEFAULT_DARK));
            assert_eq!(&*current(cx).name, DEFAULT_DARK);
            // The mapped colours survive the light to dark switch (R-274).
            assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));

            on_system_scheme(2, &theme, cx);
            assert_eq!(on_system_scheme(0, &theme, cx), activated(DEFAULT_DARK));
            on_system_scheme(2, &theme, cx);
            assert_eq!(on_system_scheme(7, &theme, cx), activated(DEFAULT_DARK));
            assert_eq!(cx.global::<ThemeStore>().system_scheme, 7);

            // A preferred theme that no longer exists keeps the current one.
            let missing = ThemeSettings {
                light: "Gone".to_owned(),
                ..theme.clone()
            };
            assert_eq!(
                on_system_scheme(2, &missing, cx),
                Some(Err(ThemeError::UnknownTheme("Gone".into())))
            );
            assert_eq!(&*current(cx).name, DEFAULT_DARK);
        });
    }

    #[gpui_kit::test]
    fn follow_off_ignores_scheme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            let before = current(cx).clone();
            let n = style_version(cx);
            assert_eq!(on_system_scheme(2, &follow(false), cx), None);
            assert_eq!(cx.global::<ThemeStore>().system_scheme, 2);
            assert!(Arc::ptr_eq(current(cx), &before));
            assert_eq!(style_version(cx), n);
        });
    }

    #[gpui_kit::test]
    fn enabling_sync_applies_at_once(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx, Vec::new());
            on_system_scheme(2, &follow(false), cx);
            assert_eq!(&*current(cx).name, DEFAULT_DARK);

            let theme = follow(true);
            assert_eq!(
                on_follow_system_enabled(&theme, cx),
                activated(DEFAULT_LIGHT)
            );
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
            let n = style_version(cx);
            assert_eq!(on_follow_system_enabled(&theme, cx), None);
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
