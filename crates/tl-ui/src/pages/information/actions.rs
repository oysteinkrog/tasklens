//! Information tab actions (UI-SYS-080, UI-SYS-083, R-128).
//!
//! All in the `tasklens` namespace. The key bindings come with the page (E28-033).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, gpui};
use serde::Deserialize;

use super::section::InfoSectionId;

gpui_kit::actions!(
    tasklens,
    [
        InformationScrollLineUp,
        InformationScrollLineDown,
        InformationScrollPageUp,
        InformationScrollPageDown,
        InformationScrollTop,
        InformationScrollBottom,
    ]
);

/// Copies one section as text (the section's copy button and its row menu).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct CopyInformationSection {
    pub section: InfoSectionId,
}
