//! Run dialog history through an in-memory `RunHistoryStore` (tasklens-gma.31).

use std::cell::RefCell;

use tl_ui::app::{RunHistoryEntry, RunHistoryStore};
use tl_ui::windows::run_history;

#[derive(Default)]
struct MemStore(RefCell<Vec<RunHistoryEntry>>);

impl RunHistoryStore for MemStore {
    fn load(&self) -> Vec<RunHistoryEntry> {
        self.0.borrow().clone()
    }

    fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String> {
        *self.0.borrow_mut() = entries.to_vec();
        Ok(())
    }
}

#[test]
fn history_roundtrip_through_store() {
    let store = MemStore::default();
    *store.0.borrow_mut() = vec![
        RunHistoryEntry {
            command: "top".into(),
            executed_at: "2026-10-06T10:00:00+02:00".into(),
            elevated: false,
        },
        RunHistoryEntry {
            command: "ls".into(),
            executed_at: "2026-10-05T09:00:00+02:00".into(),
            elevated: false,
        },
    ];

    let loaded = run_history::load(Some(&store));
    let commands: Vec<_> = loaded.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["top", "ls"]);

    let updated = run_history::record(&loaded, "firefox", true, "2026-10-06T11:30:00-03:30");
    assert_eq!(run_history::save(Some(&store), &updated), Ok(()));

    let again = run_history::load(Some(&store));
    assert_eq!(&*again, updated.as_slice());
    assert_eq!(again[0].executed_at, "2026-10-06T11:30:00-03:30");
    assert!(again[0].elevated);
    assert!(!again[1].elevated);

    assert_eq!(run_history::clear(Some(&store)), Ok(()));
    assert!(run_history::load(Some(&store)).is_empty());
}
