//! Settings window, Services page, P2 rows (E21-025, UI-SHL-068, APP-023, SYS-004): the unit
//! types the Services table lists and the states it shows. The Services filter strip (E27)
//! reads the same keys.

use gpui_kit::{App, SharedString};

use super::{PageBuilder, PageId};
use crate::host::HostContext;
use crate::i18n::tr;
use crate::settings_global::SettingKey;

/// The unit type checkboxes: label key and stored value. Column 1 holds the first five.
const UNIT_TYPES: [(&str, &str); 10] = [
    ("shell-settings-services-type-service", "service"),
    ("shell-settings-services-type-socket", "socket"),
    ("shell-settings-services-type-timer", "timer"),
    ("shell-settings-services-type-mount", "mount"),
    ("shell-settings-services-type-automount", "automount"),
    ("shell-settings-services-type-path", "path"),
    ("shell-settings-services-type-target", "target"),
    ("shell-settings-services-type-swap", "swap"),
    ("shell-settings-services-type-slice", "slice"),
    ("shell-settings-services-type-scope", "scope"),
];

/// The state checkboxes. None checked means all states.
const STATES: [(&str, &str); 4] = [
    ("shell-settings-services-state-running", "running"),
    ("shell-settings-services-state-failed", "failed"),
    ("shell-settings-services-state-stopped", "stopped"),
    ("shell-settings-services-state-disabled", "disabled"),
];

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::Services);
    // One untitled group: the page title already says "Services".
    let group = "";
    let options = |cx: &App, pairs: &[(&str, &'static str)]| -> Vec<(SharedString, SharedString)> {
        pairs
            .iter()
            .map(|(label, value)| (tr(cx, label), SharedString::new_static(value)))
            .collect()
    };
    let (first, second) = UNIT_TYPES.split_at(5);
    page.check_columns(
        group,
        SettingKey::ServicesUnitTypes,
        tr(cx, "shell-settings-services-unit-types"),
        vec![options(cx, first), options(cx, second)],
        Some(tr(cx, "shell-toast-unit-type-required")),
    )
    .check_columns(
        group,
        SettingKey::ServicesFilterStates,
        tr(cx, "shell-settings-services-states"),
        vec![options(cx, &STATES)],
        None,
    );
    page
}
