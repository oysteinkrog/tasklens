//! Frozen id numbers and strings (ADR-003 "Verification", ADR-012 constraint 2).
//!
//! Each test builds the current list from the code and compares it with a checked-in file. The
//! files are edited by hand only when an id is added at the end; the tests never write them.

use tl_model::ids::{ColumnId, MetricId};
use tl_model::validity::{ProcessField, Validity};

const FROZEN_MSG: &str = "Frozen ids changed. History files and settings depend on these numbers. \
                          Add new ids at the end; never renumber.";

/// Compares `actual` with `frozen` line by line and panics at the first differing line.
fn assert_frozen(file: &str, actual: &[String], frozen: &str) {
    let frozen: Vec<&str> = frozen.lines().collect();
    let len = actual.len().max(frozen.len());
    for i in 0..len {
        let got = actual.get(i).map(String::as_str);
        let want = frozen.get(i).copied();
        assert_eq!(
            got,
            want,
            "{file} line {}: code has {got:?}, file has {want:?}. {FROZEN_MSG}",
            i + 1
        );
    }
}

#[test]
fn metric_ids_frozen() {
    let mut lines = Vec::new();
    for (name, id) in MetricId::ALL {
        let bytes = postcard::to_allocvec(&id).expect("serialize MetricId");
        let back: u16 = postcard::from_bytes(&bytes).expect("decode MetricId bytes as u16");
        assert_eq!(
            back, id.0,
            "{name} does not serialize as its number. {FROZEN_MSG}"
        );
        lines.push(format!("{name}={}", id.0));
    }
    assert_frozen(
        "metric_ids.txt",
        &lines,
        include_str!("frozen/metric_ids.txt"),
    );
}

#[test]
fn column_ids_frozen() {
    let lines: Vec<String> = ColumnId::ALL
        .iter()
        .map(|id| format!("{} {}", id.number(), id.as_str()))
        .collect();
    assert_frozen(
        "column_ids.txt",
        &lines,
        include_str!("frozen/column_ids.txt"),
    );
}

#[test]
fn validity_numbers_frozen() {
    let lines: Vec<String> = (0..=u8::MAX)
        .filter_map(Validity::from_u8)
        .map(|v| format!("{v:?}={}", v.as_u8()))
        .collect();
    assert_frozen("validity.txt", &lines, include_str!("frozen/validity.txt"));
}

#[test]
fn process_fields_frozen() {
    let lines: Vec<String> = ProcessField::ALL
        .iter()
        .map(|f| format!("{f:?}={}", *f as u8))
        .collect();
    assert_frozen(
        "process_fields.txt",
        &lines,
        include_str!("frozen/process_fields.txt"),
    );
}
