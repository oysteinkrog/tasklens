//! Run dialog command history: the 50-entry list and the combo matcher (PROC-095, UI-PWN-029).
//!
//! tl-app owns `run-history.json` and implements [`RunHistoryStore`] on `write_atomic`
//! (R-130, R-171). This module does no file I/O (ADR-011).

use std::sync::Arc;

use smallvec::SmallVec;

use crate::app::{RunHistoryEntry, RunHistoryStore};

/// The most entries the history keeps (PROC-095).
pub const RUN_HISTORY_MAX: usize = 50;

/// Loads the history, newest first. Empty when there is no store (tests).
pub fn load(store: Option<&dyn RunHistoryStore>) -> Arc<[RunHistoryEntry]> {
    match store {
        Some(store) => store.load().into(),
        None => Arc::from([]),
    }
}

/// Local time as RFC 3339, for example `2026-10-06T14:03:22+02:00`.
pub fn now_rfc3339() -> String {
    jiff::Zoned::now()
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// Returns the history with `command` at the front. An entry with the same command text is
/// removed first, and the list is cut to [`RUN_HISTORY_MAX`].
pub fn record(
    history: &[RunHistoryEntry],
    command: &str,
    elevated: bool,
    now: &str,
) -> Vec<RunHistoryEntry> {
    let mut out = Vec::with_capacity(RUN_HISTORY_MAX);
    out.push(RunHistoryEntry {
        command: command.to_owned(),
        executed_at: now.to_owned(),
        elevated,
    });
    out.extend(
        history
            .iter()
            .filter(|e| e.command != command)
            .take(RUN_HISTORY_MAX - 1)
            .cloned(),
    );
    out
}

/// Saves the history. `Ok(())` when there is no store.
pub fn save(
    store: Option<&dyn RunHistoryStore>,
    history: &[RunHistoryEntry],
) -> Result<(), String> {
    match store {
        Some(store) => store.save(history),
        None => Ok(()),
    }
}

/// Saves an empty history.
pub fn clear(store: Option<&dyn RunHistoryStore>) -> Result<(), String> {
    save(store, &[])
}

/// Indexes into `history` for the combo list. Empty text returns every entry. Otherwise entries
/// that start with `text` come first, then entries that contain it, each group newest first.
/// Matching ignores case.
pub fn matches(history: &[RunHistoryEntry], text: &str) -> SmallVec<[u32; 16]> {
    if text.is_empty() {
        return (0..history.len() as u32).collect();
    }
    let needle = text.to_lowercase();
    let lowered: SmallVec<[String; 16]> =
        history.iter().map(|e| e.command.to_lowercase()).collect();
    let mut out: SmallVec<[u32; 16]> = lowered
        .iter()
        .enumerate()
        .filter(|(_, c)| c.starts_with(&needle))
        .map(|(i, _)| i as u32)
        .collect();
    out.extend(
        lowered
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.starts_with(&needle) && c.contains(&needle))
            .map(|(i, _)| i as u32),
    );
    out
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn entry(command: &str) -> RunHistoryEntry {
        RunHistoryEntry {
            command: command.to_owned(),
            executed_at: "2026-10-06T12:00:00+02:00".to_owned(),
            elevated: false,
        }
    }

    #[test]
    fn record_caps_at_50_and_dedups() {
        let full: Vec<_> = (0..50).map(|i| entry(&format!("cmd{i}"))).collect();
        let out = record(&full, "new", true, "2026-10-06T13:00:00+02:00");
        assert_eq!(out.len(), RUN_HISTORY_MAX);
        assert_eq!(out[0].command, "new");
        assert!(out[0].elevated);
        assert_eq!(out[49].command, "cmd48");

        let history = vec![entry("ls"), entry("firefox"), entry("top")];
        let out = record(&history, "firefox", false, "2026-10-06T13:00:00+02:00");
        let commands: Vec<_> = out.iter().map(|e| e.command.as_str()).collect();
        assert_eq!(commands, ["firefox", "ls", "top"]);
        assert_eq!(out[0].executed_at, "2026-10-06T13:00:00+02:00");
    }

    #[test]
    fn run_combo_prefix_first_then_contains() {
        let history = vec![entry("xfirefox"), entry("firefox --new"), entry("fire")];
        assert_eq!(matches(&history, "fire").as_slice(), [1, 2, 0]);
        assert_eq!(matches(&history, "FIRE").as_slice(), [1, 2, 0]);
        assert_eq!(matches(&history, "").as_slice(), [0, 1, 2]);
        assert!(matches(&history, "zzz").is_empty());
    }

    #[test]
    fn load_without_store_is_empty() {
        assert!(load(None).is_empty());
        assert_eq!(save(None, &[entry("ls")]), Ok(()));
        assert_eq!(clear(None), Ok(()));
    }

    #[test]
    fn now_rfc3339_parses() {
        let now = now_rfc3339();
        assert!(jiff::Timestamp::from_str(&now).is_ok(), "{now}");
        let offset = &now[now.len() - 6..];
        let b = offset.as_bytes();
        assert!(b[0] == b'+' || b[0] == b'-', "{now}");
        assert!(
            b[1].is_ascii_digit() && b[2].is_ascii_digit() && b[3] == b':',
            "{now}"
        );
        assert!(b[4].is_ascii_digit() && b[5].is_ascii_digit(), "{now}");
    }
}
