//! The Details grid of a device page: a model of label and value rows that re-formats a value
//! only when its bits change, the PERF-013 fallback words with their tooltips, and the
//! `TlKeyValueGrid` that draws it (docs/ui/performance.md UI-PRF-021; PERF-011 to PERF-013,
//! R-050).
//!
//! A row's `value` always holds the text the grid shows: the formatted value when Valid or
//! Stale, otherwise the fallback word. So a copy reads exactly what is on screen.
//!
//! The words below match `perf-fallback-*` in `en-US.ftl`. They are English constants until the
//! model gets a strings context, like the kv_grid words.

use std::cell::RefCell;
use std::fmt::Write as _;

use gpui_kit::{Action, App, SharedString, px};
use tl_model::units::short_age;
use tl_model::validity::Validity;

use crate::actions::OpenAdditionalSetup;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord,
};
use crate::i18n::{Strings, tr};
use crate::theme::UiMetrics;

/// Row pitch of the stats grid (label, value and gap), × ui_scale. `kv_row_h`.
pub const KV_ROW_H: f32 = 46.;
/// Element id of the grid.
pub const GRID_ID: &str = "perf-details";

const WORD_COLLECTING: &str = "Collecting…";
const WORD_RESTRICTED: &str = "Restricted";
const WORD_NEEDS_SETUP: &str = "Needs setup";
const WORD_NOT_SUPPORTED: &str = "Not supported";
const WORD_NOT_REPORTED: &str = "Not reported";
const WORD_UNAVAILABLE: &str = "Unavailable";
const WORD_UNKNOWN: &str = "Unknown";
const TIP_RESTRICTED: &str = "Needs administrator access.";
const TIP_NEEDS_SETUP: &str = "Needs setup. Open Additional Setup…";
const TIP_NOT_SUPPORTED: &str = "Not available on this device";
/// Followed by the source's last error.
const TIP_COULD_NOT_READ: &str = "Could not read: ";
/// Indent of each copied line (PERF-011).
const COPY_INDENT: &str = "    ";

/// Why a row has no value. Picks the word within a validity (PERF-013).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Unsupported until set up: "Needs setup", a link to Additional Setup.
    NeedsSetup,
    /// The hardware or driver does not provide it.
    NotSupported,
    /// The source answers with no value.
    NotReported,
    /// Shows "Unavailable" with no reason.
    Unavailable,
    /// An enum or status the source reported as unknown. Overrides the validity's word.
    Unknown,
}

/// The word and tooltip for a row with `v` and `f` (PERF-013). Valid, Stale and Missing have
/// no word: use `set_valid`, `set_stale`, or let the grid skip the row. For Failed the tooltip
/// is the prefix the last error follows.
pub fn fallback_word(v: Validity, f: Fallback) -> (&'static str, Option<&'static str>) {
    if f == Fallback::Unknown {
        return (WORD_UNKNOWN, None);
    }
    match v {
        Validity::Valid | Validity::Stale | Validity::Missing => ("", None),
        Validity::WarmingUp => (WORD_COLLECTING, None),
        Validity::Restricted => (WORD_RESTRICTED, Some(TIP_RESTRICTED)),
        Validity::Unsupported => match f {
            Fallback::NeedsSetup => (WORD_NEEDS_SETUP, Some(TIP_NEEDS_SETUP)),
            Fallback::NotSupported => (WORD_NOT_SUPPORTED, Some(TIP_NOT_SUPPORTED)),
            Fallback::NotReported => (WORD_NOT_REPORTED, None),
            Fallback::Unavailable | Fallback::Unknown => (WORD_UNAVAILABLE, None),
        },
        Validity::Failed => (WORD_UNAVAILABLE, Some(TIP_COULD_NOT_READ)),
    }
}

/// One label and value pair.
pub struct KvRow {
    /// Fluent key of the label.
    pub label: &'static str,
    /// The shown text: the formatted value, or the fallback word.
    pub value: SharedString,
    pub validity: Validity,
    pub tooltip: Option<SharedString>,
    /// Dispatched by a link value.
    pub action: Option<Box<dyn Action>>,
    /// Draws the value as a link to `action`.
    pub link: bool,
    /// The raw value `value` was formatted from.
    pub bits: u64,
    /// Skipped by the grid and the copy (PERF-013 hidden-when-missing rows).
    pub hidden: bool,
    /// The fallback behind a non-valid word.
    fallback: Option<Fallback>,
    /// `value` was formatted from `bits` by `set_valid`.
    formatted: bool,
}

impl KvRow {
    /// A row that is collecting its first value.
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            value: SharedString::new_static(WORD_COLLECTING),
            validity: Validity::WarmingUp,
            tooltip: None,
            action: None,
            link: false,
            bits: 0,
            hidden: false,
            fallback: None,
            formatted: false,
        }
    }

    /// Back to Valid. Drops what the fallback or stale state added. False when already Valid.
    fn make_valid(&mut self) -> bool {
        if self.validity == Validity::Valid {
            return false;
        }
        self.validity = Validity::Valid;
        self.fallback = None;
        self.tooltip = None;
        self.action = None;
        self.link = false;
        true
    }

    /// The value as the grid draws it.
    fn tl_value(&self) -> TlValue {
        // Unknown shows its word in `text_secondary`; Stale is the grid state that does that.
        let (validity, word) = match (self.validity, self.fallback) {
            (_, Some(Fallback::Unknown)) => (Validity::Stale, None),
            (Validity::Unsupported, Some(Fallback::NeedsSetup)) => {
                (Validity::Unsupported, Some(UnsupportedWord::NeedsSetup))
            }
            (Validity::Unsupported, Some(Fallback::NotReported)) => {
                (Validity::Unsupported, Some(UnsupportedWord::NotReported))
            }
            (Validity::Unsupported, Some(Fallback::Unavailable)) => (Validity::Failed, None),
            (v, _) => (v, None),
        };
        TlValue {
            text: self.value.clone(),
            validity,
            word,
            tooltip: self.tooltip.clone(),
            link: if self.link {
                self.action.as_ref().map(|a| a.boxed_clone())
            } else {
                None
            },
            path: false,
            tone: None,
        }
    }
}

/// Translated labels, rebuilt when the strings change.
#[derive(Default)]
struct Labels {
    version: Option<u64>,
    text: Vec<SharedString>,
}

/// The rows of one Details grid, in grid order. The page keeps it and calls the setters on each
/// snapshot; they return true when the grid needs a redraw.
#[derive(Default)]
pub struct KeyValueModel {
    pub rows: Vec<KvRow>,
    scratch: String,
    labels: RefCell<Labels>,
}

impl KeyValueModel {
    /// One collecting row per label, in this order.
    pub fn new(labels: &[&'static str]) -> Self {
        Self {
            rows: labels.iter().map(|l| KvRow::new(l)).collect(),
            ..Self::default()
        }
    }

    /// The row with `label`. A new label adds a row at the end.
    fn index(&mut self, label: &'static str) -> usize {
        if let Some(i) = self.rows.iter().position(|r| r.label == label) {
            return i;
        }
        self.rows.push(KvRow::new(label));
        self.rows.len() - 1
    }

    /// Makes the row Valid. `write` formats the value into a cleared buffer and runs only when
    /// `bits` differ from the row's, or the row did not hold a formatted value. True when the
    /// shown text or state changed. A row coming back to Valid loses its fallback tooltip and
    /// link; the owner sets its own tooltip again.
    pub fn set_valid(
        &mut self,
        label: &'static str,
        bits: u64,
        write: impl FnOnce(&mut String),
    ) -> bool {
        let i = self.index(label);
        let row = &mut self.rows[i];
        let same = row.formatted && row.bits == bits;
        let state_changed = row.make_valid();
        if same {
            return state_changed;
        }
        self.scratch.clear();
        write(&mut self.scratch);
        row.bits = bits;
        row.formatted = true;
        if row.value == self.scratch.as_str() {
            return state_changed;
        }
        row.value = SharedString::from(self.scratch.as_str());
        true
    }

    /// Makes the row Valid with `text`. For text that does not come from bits. True when the
    /// shown text or state changed.
    pub fn set_text(&mut self, label: &'static str, text: &str) -> bool {
        let i = self.index(label);
        let row = &mut self.rows[i];
        let state_changed = row.make_valid();
        row.formatted = false;
        if row.value == text {
            return state_changed;
        }
        row.value = SharedString::from(text);
        true
    }

    /// Shows the fallback word for `validity` and `fallback` with its tooltip ([`fallback_word`]).
    /// `reason` is the source's last error for Failed. Needs setup is a link that dispatches
    /// `OpenAdditionalSetup`. True when anything shown changed.
    pub fn set_fallback(
        &mut self,
        label: &'static str,
        validity: Validity,
        fallback: Fallback,
        reason: Option<&str>,
    ) -> bool {
        let (word, tip) = fallback_word(validity, fallback);
        let link = validity == Validity::Unsupported && fallback == Fallback::NeedsSetup;
        let i = self.index(label);
        let row = &mut self.rows[i];

        // The Failed tooltip is built in the scratch buffer, so it allocates only on change.
        self.scratch.clear();
        let tooltip: Option<&str> = match (validity, tip) {
            (Validity::Failed, Some(prefix)) => reason.map(|reason| {
                self.scratch.push_str(prefix);
                self.scratch.push_str(reason);
                self.scratch.as_str()
            }),
            (_, tip) => tip,
        };
        let same = row.validity == validity
            && row.fallback == Some(fallback)
            && row.value == word
            && row.link == link
            && row.tooltip.as_deref() == tooltip;
        if same {
            return false;
        }
        row.validity = validity;
        row.fallback = Some(fallback);
        row.formatted = false;
        row.value = SharedString::new_static(word);
        row.tooltip = match tooltip {
            None => None,
            Some(_) if validity != Validity::Failed => tip.map(SharedString::new_static),
            Some(text) => Some(SharedString::from(text)),
        };
        row.link = link;
        row.action = link.then(|| Box::new(OpenAdditionalSetup) as Box<dyn Action>);
        true
    }

    /// Keeps the last value and marks it Stale, with the tooltip "Last updated {age} ago" in the
    /// short age form (table (d)). True when the state or the tooltip changed.
    pub fn set_stale(&mut self, label: &'static str, age_ns: u64) -> bool {
        let i = self.index(label);
        let row = &mut self.rows[i];
        self.scratch.clear();
        self.scratch.push_str("Last updated ");
        short_age(&mut self.scratch, age_ns / 1_000_000_000);
        self.scratch.push_str(" ago");
        let state_changed = row.validity != Validity::Stale;
        row.validity = Validity::Stale;
        if row.tooltip.as_deref() == Some(self.scratch.as_str()) {
            return state_changed;
        }
        row.tooltip = Some(SharedString::from(self.scratch.as_str()));
        true
    }

    /// Replaces the row's tooltip.
    pub fn set_tooltip(&mut self, label: &'static str, text: Option<SharedString>) {
        let i = self.index(label);
        self.rows[i].tooltip = text;
    }

    /// Leaves the row out of the grid and the copy.
    pub fn hide(&mut self, label: &'static str) {
        let i = self.index(label);
        self.rows[i].hidden = true;
    }

    /// Draws the row again after [`KeyValueModel::hide`].
    pub fn show(&mut self, label: &'static str) {
        let i = self.index(label);
        self.rows[i].hidden = false;
    }

    /// Translates the labels when the strings changed since the last call. Without strings
    /// (tests) the labels read as their keys.
    pub fn refresh_labels(&self, cx: &App) {
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        let mut labels = self.labels.borrow_mut();
        if labels.version == version && labels.text.len() == self.rows.len() {
            return;
        }
        labels.version = version;
        labels.text.clear();
        labels
            .text
            .extend(self.rows.iter().map(|row| match version {
                Some(_) => tr(cx, row.label),
                None => SharedString::new_static(row.label),
            }));
    }

    /// The label as last translated by [`KeyValueModel::refresh_labels`], else its key.
    fn label_text(&self, i: usize) -> SharedString {
        let labels = self.labels.borrow();
        match labels.text.get(i) {
            Some(text) if labels.text.len() == self.rows.len() => text.clone(),
            _ => SharedString::new_static(self.rows[i].label),
        }
    }

    /// The visible rows for `TlKeyValueGrid`, in model order.
    pub fn to_kv_rows(&self) -> impl Iterator<Item = TlKvRow> + '_ {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.hidden)
            .map(|(i, row)| TlKvRow {
                label: self.label_text(i),
                value: row.tl_value(),
            })
    }

    /// Appends `    Label: value` for each visible row, one per line, as drawn (PERF-011).
    /// Missing rows are left out.
    pub fn copy_into(&self, out: &mut String) {
        let mut first = true;
        for (i, row) in self.rows.iter().enumerate() {
            if row.hidden || row.validity == Validity::Missing {
                continue;
            }
            if !first {
                out.push('\n');
            }
            first = false;
            let _ = write!(out, "{COPY_INDENT}{}: {}", self.label_text(i), row.value);
        }
    }
}

/// The Details grid: stacked label over value, three columns, pairs `kv_row_h` apart, filled
/// left to right and top to bottom in model order. Translates the labels first if needed.
pub fn render_details(model: &KeyValueModel, m: &UiMetrics, cx: &App) -> TlKeyValueGrid {
    model.refresh_labels(cx);
    let section = TlKvSection {
        title: None,
        blocks: vec![TlKvBlock {
            heading: None,
            rows: model.to_kv_rows().collect(),
        }],
    };
    // The grid's row height is half a stacked pair.
    TlKeyValueGrid::new(GRID_ID, KvLayout::Stacked)
        .sections(&[section])
        .row_height(px(KV_ROW_H / 2. * m.ui_scale))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn set_valid_formats_only_on_bits_change() {
        let mut model = KeyValueModel::new(&["Speed"]);
        let calls = Cell::new(0);
        let format = |out: &mut String, text: &str| {
            calls.set(calls.get() + 1);
            out.push_str(text);
        };

        assert!(model.set_valid("Speed", 0x1, |o| format(o, "1 MHz")));
        assert_eq!(calls.get(), 1);
        assert!(!model.set_valid("Speed", 0x1, |o| format(o, "1 MHz")));
        assert_eq!(calls.get(), 1, "same bits: no format");
        assert!(model.set_valid("Speed", 0x2, |o| format(o, "2 MHz")));
        assert_eq!(calls.get(), 2);
        assert_eq!(model.rows[0].value, "2 MHz");

        // New bits with the same text: formats, but nothing changed on screen.
        assert!(!model.set_valid("Speed", 0x3, |o| format(o, "2 MHz")));
        assert_eq!(calls.get(), 3);

        // Back from a fallback with the old bits: the word is replaced, so it formats again.
        assert!(model.set_fallback(
            "Speed",
            Validity::Failed,
            Fallback::Unavailable,
            Some("EIO")
        ));
        assert!(model.set_valid("Speed", 0x3, |o| format(o, "2 MHz")));
        assert_eq!(calls.get(), 4);
        assert_eq!(model.rows[0].value, "2 MHz");
        assert_eq!(model.rows[0].tooltip, None);

        // Back from Stale with the same bits: no format, but the state changed.
        assert!(model.set_stale("Speed", 5_000_000_000));
        assert!(model.set_valid("Speed", 0x3, |o| format(o, "2 MHz")));
        assert_eq!(calls.get(), 4);
        assert_eq!(model.rows[0].validity, Validity::Valid);
    }
}
