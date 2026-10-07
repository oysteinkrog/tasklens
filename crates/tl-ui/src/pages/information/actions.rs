//! Information tab actions (UI-SYS-080, UI-SYS-083, R-128).
//!
//! All in the `tasklens` namespace. The scroll keys are bound in the `Information` key context,
//! which the page's scroll region sets (E28-033, canonical table b).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, gpui};
use serde::Deserialize;

use super::section::InfoSectionId;

/// The key context of the page's scroll region.
pub const INFORMATION_CONTEXT: &str = "Information";

gpui_kit::actions!(
    tasklens,
    [
        InformationScrollLineUp,
        InformationScrollLineDown,
        InformationScrollPageUp,
        InformationScrollPageDown,
        InformationScrollTop,
        InformationScrollBottom,
        /// Copies every section (the tab copy icon and Ctrl+C without a text selection).
        CopyInformationAll,
    ]
);

/// Copies one section as text (the section's copy button and its row menu).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct CopyInformationSection {
    pub section: InfoSectionId,
}

/// Binds the Information rows of canonical table (b) in the `Information` context.
pub fn bind_keys_information(cx: &mut App) {
    let ctx = Some(INFORMATION_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", InformationScrollLineUp, ctx),
        KeyBinding::new("down", InformationScrollLineDown, ctx),
        KeyBinding::new("pageup", InformationScrollPageUp, ctx),
        KeyBinding::new("pagedown", InformationScrollPageDown, ctx),
        KeyBinding::new("home", InformationScrollTop, ctx),
        KeyBinding::new("end", InformationScrollBottom, ctx),
        KeyBinding::new("ctrl-c", CopyInformationAll, ctx),
    ]);
}
