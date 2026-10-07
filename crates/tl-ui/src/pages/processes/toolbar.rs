//! The Processes tab's toolbar buttons: Hide system processes and the view-mode cycler
//! (UI-PRC-019, UI-SHL-012, UI-SHL-013).
//!
//! [`ProcessesToolbar`] fills slots 2 and 3 of the title bar toolbar while the Processes tab is
//! open; on any other tab it gives no buttons, so the toolbar reserves no space. The Actions and
//! their handlers are the shell's (`ToggleHideSystem`, `CycleViewMode`). The tooltips are
//! translated once per strings version, so a frame makes no `tr` call (R-132).

use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::{AnyElement, App, Global, IntoElement as _, SharedString, Window};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::table::TableMode;

use crate::actions::{CycleViewMode, ToggleHideSystem};
use crate::components::icons::TlIcon;
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::window::toolbar::{ToolbarContent, ToolbarModel, toolbar_icon, toolbar_toggle};

/// Element id of the Hide system processes toggle. Its debug selector is `TlButton-<id>`.
pub const HIDE_SYSTEM_ID: &str = "processes-hide-system";
/// Element id of the view-mode cycler.
pub const VIEW_MODE_ID: &str = "processes-view-mode";

const KEY_HIDE: &str = "processes-toolbar-hide-system";
const KEY_SHOW: &str = "processes-toolbar-show-system";
const KEY_CYCLE: &str = "processes-toolbar-cycle-mode";
const KEY_HISTORY: &str = "processes-toolbar-history-flat";
/// Mode names, in [`mode_index`] order: grouped, tree, flat.
const KEY_MODES: [&str; 3] = [
    "processes-toolbar-mode-grouped",
    "processes-toolbar-mode-tree",
    "processes-toolbar-mode-flat",
];

/// The toolbar tooltips for one strings version.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolbarWords {
    /// "Hide system processes", the toggle while off.
    pub hide_system: SharedString,
    /// "Show system processes", the toggle while on.
    pub show_system: SharedString,
    /// "Toggle view mode (current: <name>)", in [`mode_index`] order.
    cycle: [SharedString; 3],
    /// "History shows a flat list", the cycler in History mode.
    pub history_flat: SharedString,
}

impl Default for ToolbarWords {
    /// The en-US words.
    fn default() -> Self {
        Self::with(
            |key| tr_en(key, &[]),
            |name| tr_en(KEY_CYCLE, &[("mode", FluentValue::from(name.as_ref()))]),
        )
    }
}

impl ToolbarWords {
    /// Translates every word with the current strings.
    pub fn new(cx: &App) -> Self {
        Self::with(
            |key| tr(cx, key),
            |name| tr_args(cx, KEY_CYCLE, &[("mode", FluentValue::from(name.as_ref()))]),
        )
    }

    fn with(
        t: impl Fn(&str) -> SharedString,
        cycle: impl Fn(SharedString) -> SharedString,
    ) -> Self {
        Self {
            hide_system: t(KEY_HIDE),
            show_system: t(KEY_SHOW),
            cycle: KEY_MODES.map(|key| cycle(t(key))),
            history_flat: t(KEY_HISTORY),
        }
    }

    /// The cycler tooltip for `mode`.
    pub fn cycle(&self, mode: TableMode) -> &SharedString {
        &self.cycle[mode_index(mode)]
    }
}

fn mode_index(mode: TableMode) -> usize {
    match mode {
        TableMode::Grouped => 0,
        TableMode::Tree => 1,
        TableMode::Flat => 2,
    }
}

/// The words for one strings version, shared by every window.
struct ToolbarWordsCache {
    version: u64,
    words: Rc<ToolbarWords>,
}

impl Global for ToolbarWordsCache {}

/// The words for the current strings. They are translated only when the strings version changed
/// since the last call. Without strings they are in en-US.
pub fn toolbar_words(cx: &mut App) -> Rc<ToolbarWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        thread_local! {
            static EN: Rc<ToolbarWords> = Rc::new(ToolbarWords::default());
        }
        return EN.with(Rc::clone);
    };
    match cx.try_global::<ToolbarWordsCache>() {
        Some(cache) if cache.version == version => cache.words.clone(),
        _ => {
            let words = Rc::new(ToolbarWords::new(cx));
            cx.set_global(ToolbarWordsCache {
                version,
                words: words.clone(),
            });
            words
        }
    }
}

/// The cycler icon for `mode`: grouped `rows-2`, tree `list-tree`, flat `list` (UI-SHL-013).
pub fn mode_icon(mode: TableMode) -> TlIcon {
    match mode {
        TableMode::Grouped => TlIcon::Rows2,
        TableMode::Tree => TlIcon::ListTree,
        TableMode::Flat => TlIcon::List,
    }
}

/// What the two buttons show for one model. [`ProcessesToolbar`] draws exactly this.
#[derive(Clone, PartialEq, Debug)]
pub struct ProcessesButtons {
    /// The toggle is on: system processes are hidden.
    pub hide_on: bool,
    pub hide_tooltip: SharedString,
    pub cycler_icon: TlIcon,
    pub cycler_tooltip: SharedString,
    /// History mode shows a flat list, so the cycler is disabled (PROC-016).
    pub cycler_disabled: bool,
}

impl ProcessesButtons {
    /// The buttons for `model`, or `None` on any tab but Processes.
    pub fn for_model(model: &ToolbarModel, words: &ToolbarWords) -> Option<Self> {
        if model.tab != Tab::Processes {
            return None;
        }
        let hide_tooltip = if model.hide_system {
            &words.show_system
        } else {
            &words.hide_system
        };
        let cycler_tooltip = if model.history_mode {
            &words.history_flat
        } else {
            words.cycle(model.mode)
        };
        Some(Self {
            hide_on: model.hide_system,
            hide_tooltip: hide_tooltip.clone(),
            cycler_icon: mode_icon(model.mode),
            cycler_tooltip: cycler_tooltip.clone(),
            cycler_disabled: model.history_mode,
        })
    }
}

/// The Processes tab's page buttons: the Hide system processes toggle, then the view-mode
/// cycler. Registered for `Tab::Processes` in `window/main.rs`.
pub struct ProcessesToolbar;

impl ToolbarContent for ProcessesToolbar {
    fn buttons(
        &self,
        model: &ToolbarModel,
        _: &mut Window,
        cx: &mut App,
    ) -> SmallVec<[AnyElement; 2]> {
        let words = toolbar_words(cx);
        let Some(b) = ProcessesButtons::for_model(model, &words) else {
            return SmallVec::new();
        };
        SmallVec::from_iter([
            toolbar_toggle(
                HIDE_SYSTEM_ID,
                TlIcon::LayoutGrid,
                b.hide_on,
                b.hide_tooltip,
                Box::new(ToggleHideSystem),
                model,
            )
            .into_any_element(),
            toolbar_icon(
                VIEW_MODE_ID,
                b.cycler_icon,
                b.cycler_tooltip,
                Box::new(CycleViewMode),
                b.cycler_disabled,
                model,
            )
            .into_any_element(),
        ])
    }
}
