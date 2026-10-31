//! Edits of the `[[hosts]]` list (UI-SHL-008, tasklens-q4x76.28). Settings > Hosts and the host
//! tab context menu both write through these functions. Each one checks the result with
//! `check_hosts` first and writes nothing when the list would not be valid, so a half typed
//! entry never reaches `settings.toml`. A write goes through `TlSettings::update`; the main
//! window watches the settings and adds, removes or renames the tabs without a restart.

use gpui_kit::App;
use tl_model::settings::{HostEntry, HostError, check_hosts};

use crate::host::HostRegistry;
use crate::settings_global::TlSettings;

/// The current `[[hosts]]` list.
pub fn hosts(cx: &App) -> Vec<HostEntry> {
    TlSettings::settings(cx).hosts.clone()
}

/// The reason entry `at` of `list` is not valid, if it is not. The local host's name counts as
/// taken.
fn problem(list: &[HostEntry], at: usize, cx: &mut App) -> Option<HostError> {
    let local = HostRegistry::local_name(cx);
    check_hosts(list, Some(&local))
        .into_iter()
        .find_map(|(i, why)| (i == at).then_some(why))
}

fn write(list: Vec<HostEntry>, cx: &mut App) {
    TlSettings::update(cx, |d| d.settings.hosts = list);
}

/// Appends a new, enabled host. The name and target are stored without spaces at the ends.
pub fn add(name: &str, target: &str, cx: &mut App) -> Result<(), HostError> {
    let mut list = hosts(cx);
    list.push(HostEntry {
        name: name.trim().to_owned(),
        target: target.trim().to_owned(),
        enabled: true,
    });
    if let Some(why) = problem(&list, list.len() - 1, cx) {
        return Err(why);
    }
    write(list, cx);
    Ok(())
}

/// Changes the name and target of entry `index`. Nothing for an index past the list.
pub fn edit(index: usize, name: &str, target: &str, cx: &mut App) -> Result<(), HostError> {
    let mut list = hosts(cx);
    let Some(entry) = list.get_mut(index) else {
        return Ok(());
    };
    entry.name = name.trim().to_owned();
    entry.target = target.trim().to_owned();
    if let Some(why) = problem(&list, index, cx) {
        return Err(why);
    }
    write(list, cx);
    Ok(())
}

/// Removes entry `index`. Its tab goes and its connection stops.
pub fn remove(index: usize, cx: &mut App) {
    let mut list = hosts(cx);
    if index < list.len() {
        list.remove(index);
        write(list, cx);
    }
}

/// Moves entry `index` one place up (`up`) or down. It changes the tab order.
pub fn move_entry(index: usize, up: bool, cx: &mut App) {
    let mut list = hosts(cx);
    let other = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1)
    };
    if let Some(other) = other
        && index < list.len()
        && other < list.len()
    {
        list.swap(index, other);
        write(list, cx);
    }
}

/// Turns the entry on or off. A disabled entry keeps its place but has no tab and no connection.
pub fn set_enabled(index: usize, enabled: bool, cx: &mut App) {
    let mut list = hosts(cx);
    if let Some(entry) = list.get_mut(index)
        && entry.enabled != enabled
    {
        entry.enabled = enabled;
        write(list, cx);
    }
}

/// The index of the entry named `name`.
pub fn position(name: &str, cx: &App) -> Option<usize> {
    TlSettings::settings(cx)
        .hosts
        .iter()
        .position(|e| e.name == name)
}
