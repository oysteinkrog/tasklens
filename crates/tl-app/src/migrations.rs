//! Schema migration chain for `settings.toml` and `state.toml` (ADR-011 load step 3).
//!
//! A file with a `schema_version` lower than the current one runs each migration in order
//! before it is deserialized. A later schema change only adds a function to `MIGRATIONS`.

use tl_model::settings::SCHEMA_VERSION;
use tracing::{info, warn};

/// One migration step. It edits the raw table in place.
pub type Migration = fn(&mut toml::Table);

/// `MIGRATIONS[i]` migrates version i+1 to i+2. Empty while `SCHEMA_VERSION` is 1.
pub const MIGRATIONS: &[Migration] = &[];

const VERSION_KEY: &str = "schema_version";

/// The table's `schema_version`. A missing value counts as 1. A value that is not an integer
/// of at least 1 is logged and also counts as 1.
pub fn version_of(t: &toml::Table) -> u32 {
    let Some(value) = t.get(VERSION_KEY) else {
        return 1;
    };
    match value.as_integer().map(u32::try_from) {
        Some(Ok(v)) if v >= 1 => v,
        _ => {
            // The value itself is not logged (privacy rule: never log table contents).
            warn!(target: "tasklens::settings", "schema_version is not an integer >= 1; using 1");
            1
        }
    }
}

/// Runs `MIGRATIONS` from version `from` up to `SCHEMA_VERSION` and returns the new version.
/// With `from >= SCHEMA_VERSION` the table is left as is and `from` is returned.
pub fn migrate(t: &mut toml::Table, from: u32) -> u32 {
    migrate_with(t, from, MIGRATIONS, SCHEMA_VERSION)
}

/// The core of `migrate`: runs `chain[from-1 ..]` in order, sets `schema_version` to `to` and
/// returns `to`. `chain[i]` migrates version i+1 to i+2, so `chain.len()` must be `to - 1`.
fn migrate_with(t: &mut toml::Table, from: u32, chain: &[Migration], to: u32) -> u32 {
    if from >= to {
        return from;
    }
    debug_assert_eq!(chain.len() + 1, to as usize, "chain length must be to - 1");
    let start = from.max(1);
    for (version, step) in (start..to).zip(&chain[start as usize - 1..]) {
        step(t);
        info!(
            target: "tasklens::settings",
            "settings migrated from v{version} to v{}",
            version + 1
        );
    }
    t.insert(VERSION_KEY.to_owned(), toml::Value::Integer(i64::from(to)));
    to
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Runs `f` with a subscriber that records every event, and returns the log text.
    fn capture_logs(f: impl FnOnce()) -> String {
        let out = Captured::default();
        let writer = out.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(out.0.lock().unwrap().clone()).unwrap()
    }

    fn table(src: &str) -> toml::Table {
        src.parse().unwrap()
    }

    fn push(t: &mut toml::Table, name: &str) {
        let list = t
            .entry("ran")
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        list.as_array_mut()
            .unwrap()
            .push(toml::Value::String(name.to_owned()));
    }

    fn v1_to_v2(t: &mut toml::Table) {
        push(t, "v1_to_v2");
    }

    fn v2_to_v3(t: &mut toml::Table) {
        push(t, "v2_to_v3");
    }

    const TEST_CHAIN: &[Migration] = &[v1_to_v2, v2_to_v3];

    #[test]
    fn missing_version_is_1() {
        assert_eq!(version_of(&table("other = 5")), 1);
    }

    #[test]
    fn bad_version_is_1() {
        for src in [
            "schema_version = \"x\"",
            "schema_version = 0",
            "schema_version = -3",
            "schema_version = 1.5",
        ] {
            let t = table(src);
            let mut version = 0;
            let logs = capture_logs(|| version = version_of(&t));
            assert_eq!(version, 1, "{src}");
            assert!(logs.contains("WARN"), "{src}: {logs}");
            assert!(logs.contains("schema_version is not an integer"), "{src}");
        }
        assert_eq!(version_of(&table("schema_version = 2")), 2);
    }

    #[test]
    fn current_version_untouched() {
        let mut t = table("schema_version = 1\nkeep = true");
        let before = t.clone();
        assert!(MIGRATIONS.is_empty());
        assert_eq!(migrate(&mut t, 1), 1);
        assert_eq!(t, before);
    }

    #[test]
    fn chain_runs_in_order() {
        let mut t = table("schema_version = 1");
        let mut version = 0;
        let logs = capture_logs(|| version = migrate_with(&mut t, 1, TEST_CHAIN, 3));
        assert_eq!(version, 3);
        assert_eq!(version_of(&t), 3);
        let ran: Vec<&str> = t["ran"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ran, ["v1_to_v2", "v2_to_v3"]);
        assert!(logs.contains("settings migrated from v1 to v2"), "{logs}");
        assert!(logs.contains("settings migrated from v2 to v3"), "{logs}");
    }

    #[test]
    fn chain_starts_at_from() {
        let mut t = table("schema_version = 2");
        assert_eq!(migrate_with(&mut t, 2, TEST_CHAIN, 3), 3);
        assert_eq!(t["ran"].as_array().unwrap().len(), 1);
        assert_eq!(t["ran"][0].as_str(), Some("v2_to_v3"));
    }
}
