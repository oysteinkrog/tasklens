//! The theme editor live preview (UI-FND-043, APP-037): how the Theme Editor window (E21) shows
//! unsaved colours across the whole app, and how Done and Cancel end the preview.
//!
//! While a preview is live, `ThemeStore.preview` is what `current()` returns. Slot and name edits
//! are recorded at once and applied at most once per frame through the main window's
//! `AnimationDriver`, under the theme's requester id (R-209). No crossfade runs during a preview.
//!
//! The session state (token id, the edited def, the pending edits) lives in a private Global so
//! the store keeps only the rendered `preview` theme.

use std::mem;
use std::sync::Arc;

use gpui_kit::{App, Global, Hsla, Rgba, Window};
use tracing::debug;

use super::store::{ThemeError, ThemeStore, apply_kit, finish_crossfade};
use super::user_store::{PersistTheme, persist};
use super::{ThemeDef, ThemeSlot, bump_style_version, rebuild_derived};
use crate::anim::AnimationDriver;

/// The contrast floor of `text_primary` on `list_bg` for a random theme (WCAG AA body text).
const RANDOM_TEXT_CONTRAST: f32 = 4.5;

/// How many times `random_preview` re-rolls the `text_primary` lightness before it falls back to
/// black or white.
const RANDOM_REROLLS: usize = 64;

/// The handle of one live preview. Not `Clone`: one live preview at a time, and Done and Cancel
/// take it by value.
#[derive(Debug)]
pub struct PreviewToken {
    id: u64,
}

/// The live preview.
struct PreviewSession {
    id: u64,
    /// The base theme's name. `commit_preview` replaces the user theme with this name.
    original: Arc<str>,
    /// `true` when the base resolved to a built-in theme. Such a preview cannot be committed.
    base_builtin: bool,
    /// The base def with every edit applied, including edits not yet applied to `preview`.
    def: ThemeDef,
    /// The latest value per slot since the last applied frame, in `ThemeSlot::ALL` order.
    pending_slots: [Option<Hsla>; 23],
    pending_name: Option<Arc<str>>,
    /// `true` while an apply is scheduled for the next frame.
    scheduled: bool,
}

#[derive(Default)]
struct PreviewState {
    /// The last token id handed out. Ids start at 1.
    last_id: u64,
    live: Option<PreviewSession>,
}

impl Global for PreviewState {}

/// The live session when `tok` is its token. A stale token logs at debug level and gives `None`.
fn session<'a>(tok: &PreviewToken, cx: &'a mut App) -> Option<&'a mut PreviewSession> {
    let live = cx.default_global::<PreviewState>().live.as_mut();
    match live {
        Some(s) if s.id == tok.id => Some(s),
        _ => {
            debug!(target: "tasklens::ui", token = tok.id, "stale theme preview token ignored");
            None
        }
    }
}

/// Writes `c` into `slot` of `def`.
fn set_def_slot(def: &mut ThemeDef, slot: ThemeSlot, c: Hsla) {
    match slot {
        ThemeSlot::Accent => def.accent = c,
        s => def.slots[s as usize] = c,
    }
}

/// Records `c` for `slot` in the session's def and pending edits.
fn record_slot(s: &mut PreviewSession, slot: ThemeSlot, c: Hsla) {
    set_def_slot(&mut s.def, slot, c);
    s.pending_slots[slot as usize] = Some(c);
}

/// Schedules one apply of the pending edits unless one is scheduled. With an animation window it
/// runs in that window's next frame; without one (or when the window has no driver) it runs once
/// the current update ends. Either way many edits in one frame apply once.
fn schedule(cx: &mut App) {
    let Some(s) = cx.default_global::<PreviewState>().live.as_mut() else {
        return;
    };
    if s.scheduled {
        return;
    }
    s.scheduled = true;
    let window = cx.global::<ThemeStore>().anim_window;
    // Deferred: the editor may call this inside an update of that window.
    cx.defer(move |cx| {
        let requested = window.is_some_and(|w| {
            w.update(cx, |_, window, cx| request_frame(window, cx))
                .unwrap_or(false)
        });
        if !requested {
            apply_pending(cx);
        }
    });
}

/// Asks the window's driver to run the apply on the next frame. `false` when the window has no
/// driver.
fn request_frame(window: &mut Window, cx: &mut App) -> bool {
    let Some(driver) = AnimationDriver::try_global(window, cx) else {
        return false;
    };
    let id = cx.global::<ThemeStore>().anim.entity_id();
    driver.update(cx, |d, cx| {
        d.request_callback(id, |_, cx| apply_pending(cx), window, cx)
    });
    true
}

/// One applied preview frame: clones `preview`, writes the pending slots and name, rebuilds the
/// derived tints, replaces `preview`, bumps `style_version` once and refreshes the windows. An
/// accent edit also maps the theme onto the kit, so kit `primary` and `ring` preview too.
fn apply_pending(cx: &mut App) {
    let Some(s) = cx.default_global::<PreviewState>().live.as_mut() else {
        return;
    };
    s.scheduled = false;
    let slots = mem::take(&mut s.pending_slots);
    let name = s.pending_name.take();
    if name.is_none() && slots.iter().all(Option::is_none) {
        return;
    }
    let accent_changed = slots[ThemeSlot::Accent as usize].is_some();

    let store = cx.global_mut::<ThemeStore>();
    let Some(prev) = &store.preview else {
        return;
    };
    let mut t = (**prev).clone();
    for (slot, c) in ThemeSlot::ALL.into_iter().zip(slots) {
        if let Some(c) = c {
            slot.set(&mut t, c);
        }
    }
    if let Some(name) = name {
        t.name = name;
    }
    rebuild_derived(&mut t, &store.appearance);
    let t = Arc::new(t);
    store.preview = Some(t.clone());
    bump_style_version(cx);
    if accent_changed {
        apply_kit(&t, cx);
    }
    cx.refresh_windows();
}

/// Clears `preview` and returns the app to `active` in one frame: one `style_version` bump and
/// one `map::apply`.
fn end_preview(cx: &mut App) {
    let store = cx.global_mut::<ThemeStore>();
    store.preview = None;
    let active = store.active.clone();
    bump_style_version(cx);
    apply_kit(&active, cx);
    cx.refresh_windows();
}

/// WCAG relative luminance.
fn luminance(c: Hsla) -> f32 {
    let c: Rgba = c.into();
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio of `a` and `b`, 1 to 21.
fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// xorshift64 seeded through one splitmix64 step, so seed 0 works too.
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        XorShift(if z == 0 { 1 } else { z })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value in `0.0..1.0` with 24 bits of precision.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The 23 editable colours of a random theme for `seed`. `text_primary` on `list_bg` is at least
/// 4.5:1: its lightness is re-rolled up to 64 times, then set to black or white, whichever has
/// more contrast.
fn random_colours(seed: u64) -> [Hsla; 23] {
    let mut rng = XorShift::new(seed);
    let mut colours = [Hsla::default(); 23];
    for c in &mut colours {
        *c = Hsla {
            h: rng.unit(),
            s: rng.unit(),
            l: rng.unit(),
            a: 1.0,
        };
    }
    let list_bg = colours[ThemeSlot::ListBg as usize];
    let text = &mut colours[ThemeSlot::TextPrimary as usize];
    for _ in 0..RANDOM_REROLLS {
        if contrast(*text, list_bg) >= RANDOM_TEXT_CONTRAST {
            return colours;
        }
        text.l = rng.unit();
    }
    if contrast(*text, list_bg) < RANDOM_TEXT_CONTRAST {
        let black = Hsla { l: 0.0, ..*text };
        let white = Hsla { l: 1.0, ..*text };
        *text = if contrast(black, list_bg) >= contrast(white, list_bg) {
            black
        } else {
            white
        };
    }
    colours
}

impl ThemeStore {
    /// Starts a preview of `base`: `current()` returns it from now on, `active` stays. A live
    /// preview is cancelled first and its token stops working (the shell closes its editor
    /// window). A running crossfade ends first. Maps the preview onto the kit once.
    pub fn begin_preview(base: &ThemeDef, cx: &mut App) -> PreviewToken {
        finish_crossfade(cx);
        let state = cx.default_global::<PreviewState>();
        if let Some(old) = state.live.take() {
            debug!(target: "tasklens::ui", token = old.id, "theme preview replaced");
        }
        state.last_id += 1;
        let id = state.last_id;

        let store = cx.global_mut::<ThemeStore>();
        let base_builtin = store.resolve(&base.name).is_some_and(|(_, b)| b);
        let theme = Arc::new(store.build(base, base_builtin));
        store.preview = Some(theme.clone());
        cx.default_global::<PreviewState>().live = Some(PreviewSession {
            id,
            original: base.name.clone(),
            base_builtin,
            def: base.clone(),
            pending_slots: [None; 23],
            pending_name: None,
            scheduled: false,
        });
        bump_style_version(cx);
        apply_kit(&theme, cx);
        cx.refresh_windows();
        PreviewToken { id }
    }

    /// Sets one slot of the preview. The latest value per slot applies on the next frame, at
    /// most once per frame.
    pub fn set_preview_slot(tok: &PreviewToken, slot: ThemeSlot, c: Hsla, cx: &mut App) {
        let Some(s) = session(tok, cx) else {
            return;
        };
        record_slot(s, slot, c);
        schedule(cx);
    }

    /// Renames the previewed theme. Applies on the next frame like a slot edit.
    pub fn set_preview_name(tok: &PreviewToken, name: Arc<str>, cx: &mut App) {
        let Some(s) = session(tok, cx) else {
            return;
        };
        s.def.name = name.clone();
        s.pending_name = Some(name);
        schedule(cx);
    }

    /// The edited def with every edit so far, also the ones not yet applied to a frame. Share
    /// encodes this (E17-009). A stale token gives the active theme's def.
    pub fn preview_def(tok: &PreviewToken, cx: &App) -> ThemeDef {
        let live = cx
            .try_global::<PreviewState>()
            .and_then(|st| st.live.as_ref())
            .filter(|s| s.id == tok.id);
        if let Some(s) = live {
            return s.def.clone();
        }
        debug!(target: "tasklens::ui", token = tok.id, "stale theme preview token ignored");
        let store = cx.global::<ThemeStore>();
        store
            .resolve(&store.active.name)
            .map(|(d, _)| d.clone())
            .unwrap_or_else(|| store.builtin[0].clone())
    }

    /// Done: writes the edited def into `user` in place of the theme named like the base (or
    /// appends it when there is none), sends one `PersistTheme::Save`, activates it without
    /// crossfade and clears `preview`. The caller writes `theme.active`, as for
    /// `ActivateSource::User`.
    ///
    /// A built-in base gives `BuiltinCannotBeEdited`, saves nothing and ends the preview, since
    /// the token is used up. A stale token gives `PreviewEnded` and changes nothing.
    pub fn commit_preview(tok: PreviewToken, cx: &mut App) -> Result<ThemeDef, ThemeError> {
        if session(&tok, cx).is_none() {
            return Err(ThemeError::PreviewEnded);
        }
        let Some(s) = cx.default_global::<PreviewState>().live.take() else {
            return Err(ThemeError::PreviewEnded);
        };
        if s.base_builtin {
            end_preview(cx);
            return Err(ThemeError::BuiltinCannotBeEdited);
        }
        let def = s.def;

        let store = cx.global_mut::<ThemeStore>();
        match store.user.iter().position(|d| d.name == s.original) {
            Some(i) => store.user[i] = def.clone(),
            None => store.user.push(def.clone()),
        }
        let saved = store.user.clone();
        let active = Arc::new(store.build(&def, false));
        store.active = active.clone();
        store.preview = None;
        persist(PersistTheme::Save(saved), cx);
        bump_style_version(cx);
        apply_kit(&active, cx);
        cx.refresh_windows();
        Ok(def)
    }

    /// Cancel, or the editor window closed by its X: clears `preview`. `active` never changed,
    /// so the app returns to it in one frame. Saves nothing.
    pub fn cancel_preview(tok: PreviewToken, cx: &mut App) {
        if session(&tok, cx).is_none() {
            return;
        }
        cx.default_global::<PreviewState>().live = None;
        end_preview(cx);
    }

    /// "Random": sets the 22 slots and the accent to random colours from `seed`, with
    /// `text_primary` on `list_bg` at least 4.5:1. Graph and status colours keep the base
    /// theme's values. The shell shows the toast "Random theme generated." (APP-037).
    pub fn random_preview(tok: &PreviewToken, seed: u64, cx: &mut App) {
        let Some(s) = session(tok, cx) else {
            return;
        };
        for (slot, c) in ThemeSlot::ALL.into_iter().zip(random_colours(seed)) {
            record_slot(s, slot, c);
        }
        schedule(cx);
    }

    /// The name of the theme being edited, as it appears in the Theme submenu (the base name,
    /// before any rename), so the submenu can show "<name> (editing)" with the check on that
    /// row. `None` when no preview is live.
    pub fn preview_name(cx: &App) -> Option<Arc<str>> {
        cx.try_global::<PreviewState>()?
            .live
            .as_ref()
            .map(|s| s.original.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use gpui_kit::component::theme::Theme;
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
        div, rgb,
    };
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};

    use super::*;
    use crate::app::BootSettings;
    use crate::settings_global::TlSettings;
    use crate::theme::store::{self, ActivateSource, DEFAULT_DARK, DEFAULT_LIGHT};
    use crate::theme::user_store::ThemePersist;
    use crate::theme::{builtin, current, metrics};

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    fn style_version(cx: &App) -> u64 {
        metrics::metrics(cx).style_version
    }

    /// A user theme "Ocean": Default Light with a blue window background, no status or graph.
    fn ocean() -> ThemeDef {
        let mut def = builtin::load()
            .iter()
            .find(|d| &*d.name == DEFAULT_LIGHT)
            .cloned()
            .unwrap();
        def.name = "Ocean".into();
        def.slots[ThemeSlot::WindowBg as usize] = hex(0x103050);
        def.status = None;
        def.graph = None;
        def
    }

    fn builtin_def(name: &str) -> ThemeDef {
        builtin::load()
            .iter()
            .find(|d| &*d.name == name)
            .cloned()
            .unwrap()
    }

    type Saves = Rc<RefCell<Vec<Vec<ThemeDef>>>>;

    /// The store with Default Dark active and "Ocean" as the only user theme, `TlSettings` with
    /// animations on, a recording `ThemePersist`, and a window with a driver set as the animation
    /// window. Returns the recorded `Save` requests.
    fn rig(cx: &mut TestAppContext) -> (&mut VisualTestContext, Saves) {
        let saves: Saves = Rc::default();
        let r = saves.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(ThemePersist(Box::new(move |p| {
                if let PersistTheme::Save(v) = p {
                    r.borrow_mut().push(v);
                }
            })));
            store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                vec![ocean()],
            );
            let mut settings = Settings::default();
            settings.motion.animations = true;
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
        vcx.update(|window, cx| store::set_anim_window(window.window_handle(), cx));
        (vcx, saves)
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

    /// Lets deferred work run and draws one frame. Returns the `on_next_frame` callbacks that
    /// ran.
    fn frame(vcx: &mut VisualTestContext) -> usize {
        vcx.executor().advance_clock(Duration::from_millis(16));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.simulate_next_frame(cx))
    }

    fn begin(vcx: &mut VisualTestContext, def: ThemeDef) -> PreviewToken {
        vcx.update(|_, cx| ThemeStore::begin_preview(&def, cx))
    }

    fn active_name(cx: &App) -> Arc<str> {
        cx.global::<ThemeStore>().active.name.clone()
    }

    #[gpui_kit::test]
    fn views_read_preview_when_live(cx: &mut TestAppContext) {
        let (vcx, _) = rig(cx);
        let tok = begin(vcx, ocean());
        vcx.update(|_, cx| {
            assert_eq!(&*current(cx).name, "Ocean");
            assert_eq!(current(cx).window_bg, hex(0x103050));
            assert_eq!(&*active_name(cx), DEFAULT_DARK);
            assert_eq!(ThemeStore::preview_name(cx).as_deref(), Some("Ocean"));
            // Activate is refused while previewing (rule 9).
            assert_eq!(
                store::activate(DEFAULT_LIGHT, ActivateSource::User, cx),
                Err(ThemeError::PreviewActive)
            );
            ThemeStore::set_preview_slot(&tok, ThemeSlot::Header, hex(0x123456), cx);
            ThemeStore::set_preview_name(&tok, "Deep Ocean".into(), cx);
            // Share reads the edits before they are applied to a frame.
            let def = ThemeStore::preview_def(&tok, cx);
            assert_eq!(def.slots[ThemeSlot::Header as usize], hex(0x123456));
            assert_eq!(&*def.name, "Deep Ocean");
        });
        frame(vcx);
        vcx.update(|_, cx| {
            assert_eq!(current(cx).header, hex(0x123456));
            assert_eq!(&*current(cx).name, "Deep Ocean");
            // The submenu marks the row of the theme being edited.
            assert_eq!(ThemeStore::preview_name(cx).as_deref(), Some("Ocean"));
            assert_eq!(&*active_name(cx), DEFAULT_DARK);
        });
    }

    #[gpui_kit::test]
    fn slot_updates_coalesce_per_frame(cx: &mut TestAppContext) {
        let (vcx, _) = rig(cx);
        let tok = begin(vcx, ocean());
        let n = vcx.update(|_, cx| style_version(cx));
        vcx.update(|_, cx| {
            for i in 0..50u32 {
                ThemeStore::set_preview_slot(&tok, ThemeSlot::WindowBg, hex(0x010101 * i), cx);
            }
            ThemeStore::set_preview_slot(&tok, ThemeSlot::WindowBg, hex(0x000000), cx);
            // Nothing applies before the frame.
            assert_eq!(current(cx).window_bg, hex(0x103050));
            assert_eq!(style_version(cx), n);
        });
        // One driver frame applies the edits.
        assert_eq!(frame(vcx), 1);
        vcx.update(|_, cx| {
            assert_eq!(current(cx).window_bg, hex(0x000000));
            assert_eq!(style_version(cx), n + 1);
        });
        // Nothing is armed once applied.
        assert_eq!(frame(vcx), 0);
        assert_eq!(vcx.update(|_, cx| style_version(cx)), n + 1);
    }

    #[gpui_kit::test]
    fn accent_edit_updates_kit_tokens(cx: &mut TestAppContext) {
        let (vcx, _) = rig(cx);
        let tok = begin(vcx, ocean());
        let c = hex(0x22AA66);
        vcx.update(|_, cx| ThemeStore::set_preview_slot(&tok, ThemeSlot::Accent, c, cx));
        frame(vcx);
        vcx.update(|_, cx| {
            assert_eq!(current(cx).accent, c);
            assert_eq!(current(cx).derived.focus_ring, c);
            let kit = Theme::global(cx);
            assert_eq!(kit.colors.primary, c);
            assert_eq!(kit.colors.ring, c);
        });
    }

    #[gpui_kit::test]
    fn cancel_restores_active(cx: &mut TestAppContext) {
        let (vcx, saves) = rig(cx);
        let before = vcx.update(|_, cx| cx.global::<ThemeStore>().active.clone());
        let tok = begin(vcx, ocean());
        vcx.update(|_, cx| {
            ThemeStore::set_preview_slot(&tok, ThemeSlot::Accent, hex(0x22AA66), cx);
        });
        frame(vcx);
        let n = vcx.update(|_, cx| style_version(cx));
        vcx.update(|_, cx| {
            ThemeStore::cancel_preview(tok, cx);
            assert!(Arc::ptr_eq(current(cx), &before));
            assert!(cx.global::<ThemeStore>().preview.is_none());
            assert_eq!(ThemeStore::preview_name(cx), None);
            assert_eq!(style_version(cx), n + 1);
            assert_eq!(Theme::global(cx).colors.primary, before.accent);
            assert_eq!(Theme::global(cx).colors.background, before.window_bg);
            // Activate works again.
            store::activate(DEFAULT_LIGHT, ActivateSource::User, cx).unwrap();
        });
        assert!(saves.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn commit_saves_and_activates(cx: &mut TestAppContext) {
        let (vcx, saves) = rig(cx);
        let tok = begin(vcx, ocean());
        vcx.update(|_, cx| {
            ThemeStore::set_preview_slot(&tok, ThemeSlot::WindowBg, hex(0x000000), cx);
            let def = ThemeStore::commit_preview(tok, cx).unwrap();
            assert_eq!(&*def.name, "Ocean");
            assert_eq!(def.slots[ThemeSlot::WindowBg as usize], hex(0x000000));

            let store = cx.global::<ThemeStore>();
            assert_eq!(store.user.len(), 1);
            assert_eq!(&*store.user[0].name, "Ocean");
            assert_eq!(
                store.user[0].slots[ThemeSlot::WindowBg as usize],
                hex(0x000000)
            );
            assert!(store.preview.is_none());
            assert_eq!(&*store.active.name, "Ocean");
            assert!(!store.active.builtin);
            assert_eq!(current(cx).window_bg, hex(0x000000));
            assert_eq!(Theme::global(cx).colors.background, hex(0x000000));
            assert_eq!(ThemeStore::preview_name(cx), None);
        });
        let saves = saves.borrow();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].len(), 1);
        assert_eq!(
            saves[0][0].slots[ThemeSlot::WindowBg as usize],
            hex(0x000000)
        );
        drop(saves);
        // Activated without a crossfade: the next frame changes nothing. The frame the edit
        // scheduled before Done finds no preview and does nothing.
        let n = vcx.update(|_, cx| style_version(cx));
        frame(vcx);
        vcx.update(|_, cx| {
            assert_eq!(style_version(cx), n);
            assert_eq!(current(cx).window_bg, hex(0x000000));
        });
        assert_eq!(frame(vcx), 0);
    }

    #[gpui_kit::test]
    fn commit_rename_replaces_under_new_name(cx: &mut TestAppContext) {
        let (vcx, saves) = rig(cx);
        let tok = begin(vcx, ocean());
        vcx.update(|_, cx| {
            ThemeStore::set_preview_name(&tok, "Deep Ocean".into(), cx);
            ThemeStore::commit_preview(tok, cx).unwrap();
            let names: Vec<&str> = cx
                .global::<ThemeStore>()
                .user
                .iter()
                .map(|d| &*d.name)
                .collect();
            assert_eq!(names, ["Deep Ocean"]);
            assert_eq!(&*current(cx).name, "Deep Ocean");
        });
        assert_eq!(saves.borrow().len(), 1);
    }

    #[gpui_kit::test]
    fn commit_builtin_refused(cx: &mut TestAppContext) {
        let (vcx, saves) = rig(cx);
        let tok = begin(vcx, builtin_def(DEFAULT_LIGHT));
        vcx.update(|_, cx| {
            ThemeStore::set_preview_slot(&tok, ThemeSlot::WindowBg, hex(0x000000), cx);
            assert_eq!(
                ThemeStore::commit_preview(tok, cx).map(|d| d.name),
                Err(ThemeError::BuiltinCannotBeEdited)
            );
            let store = cx.global::<ThemeStore>();
            assert_eq!(store.user.len(), 1);
            assert!(store.preview.is_none());
            assert_eq!(&*store.active.name, DEFAULT_DARK);
        });
        assert!(saves.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn second_begin_cancels_first(cx: &mut TestAppContext) {
        let (vcx, saves) = rig(cx);
        let a = begin(vcx, ocean());
        let b = begin(vcx, builtin_def(DEFAULT_LIGHT));
        vcx.update(|_, cx| {
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
            assert_eq!(ThemeStore::preview_name(cx).as_deref(), Some(DEFAULT_LIGHT));
            let before = current(cx).clone();
            let n = style_version(cx);
            ThemeStore::set_preview_slot(&a, ThemeSlot::WindowBg, hex(0x000000), cx);
            ThemeStore::set_preview_name(&a, "Gone".into(), cx);
            ThemeStore::random_preview(&a, 7, cx);
            assert_eq!(&*ThemeStore::preview_def(&b, cx).name, DEFAULT_LIGHT);
            assert_eq!(style_version(cx), n);
            assert!(Arc::ptr_eq(current(cx), &before));
        });
        assert_eq!(frame(vcx), 0);
        vcx.update(|_, cx| {
            let n = style_version(cx);
            assert_eq!(
                ThemeStore::commit_preview(a, cx).map(|d| d.name),
                Err(ThemeError::PreviewEnded)
            );
            assert_eq!(&*current(cx).name, DEFAULT_LIGHT);
            assert_eq!(style_version(cx), n);
            assert_eq!(current(cx).window_bg, hex(0xF3F4F6));
            ThemeStore::cancel_preview(b, cx);
            assert_eq!(&*current(cx).name, DEFAULT_DARK);
        });
        assert!(saves.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn random_keeps_contrast(cx: &mut TestAppContext) {
        let (vcx, _) = rig(cx);
        let tok = begin(vcx, ocean());
        let (graph, status) = vcx.update(|_, cx| (current(cx).graph, current(cx).status));
        for seed in 0..100u64 {
            vcx.update(|_, cx| ThemeStore::random_preview(&tok, seed, cx));
            frame(vcx);
            vcx.update(|_, cx| {
                let t = current(cx);
                let ratio = contrast(t.text_primary, t.list_bg);
                assert!(ratio >= 4.5, "seed {seed}: {ratio}");
                assert_eq!(t.graph, graph, "seed {seed}");
                assert_eq!(t.status, status, "seed {seed}");
                let def = ThemeStore::preview_def(&tok, cx);
                assert_eq!(def.accent, t.accent);
                assert_eq!(def.graph, None);
                assert_eq!(def.status, None);
            });
        }
        // The same seed gives the same theme.
        assert_eq!(random_colours(42), random_colours(42));
        assert_ne!(random_colours(42), random_colours(43));
    }

    #[gpui_kit::test]
    fn edits_apply_without_anim_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                vec![ocean()],
            );
        });
        let tok = cx.update(|cx| ThemeStore::begin_preview(&ocean(), cx));
        let n = cx.update(|cx| style_version(cx));
        cx.update(|cx| {
            for _ in 0..10 {
                ThemeStore::set_preview_slot(&tok, ThemeSlot::Divider, hex(0xABCDEF), cx);
            }
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(current(cx).divider, hex(0xABCDEF));
            assert_eq!(style_version(cx), n + 1);
        });
    }
}
