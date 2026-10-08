//! The Details grid of a device page: a model of label and value rows that re-formats a value
//! only when its bits change, the PERF-013 fallback words with their tooltips, and the
//! `TlKeyValueGrid` that draws it (docs/ui/performance.md UI-PRF-021; PERF-011 to PERF-013,
//! R-050).
//!
//! A row's `value` always holds the text the grid shows: the formatted value when Valid or
//! Stale, otherwise the fallback word. So a copy reads exactly what is on screen.
//!
//! The words come from `en-US.ftl` through [`DetailWords`], translated once per strings version
//! by [`KeyValueModel::refresh_strings`]. Before the first refresh the words are in en-US.

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::{Action, App, Global, SharedString, px};
use smallvec::SmallVec;
use tl_model::units::short_age;
use tl_model::validity::Validity;

use crate::actions::OpenAdditionalSetup;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, value_copy_form,
};
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::theme::UiMetrics;

/// Row pitch of the stats grid (label, value and gap), × ui_scale. `kv_row_h`.
pub const KV_ROW_H: f32 = 46.;
/// Element id of the grid.
pub const GRID_ID: &str = "perf-details";

const KEY_COLLECTING: &str = "perf-fallback-collecting";
const KEY_RESTRICTED: &str = "perf-fallback-restricted";
const KEY_RESTRICTED_TIP: &str = "perf-fallback-restricted-tooltip";
const KEY_NEEDS_SETUP: &str = "perf-fallback-needs-setup";
const KEY_NEEDS_SETUP_TIP: &str = "perf-fallback-needs-setup-tooltip";
const KEY_NOT_SUPPORTED: &str = "perf-fallback-not-supported";
const KEY_NOT_SUPPORTED_TIP: &str = "perf-fallback-not-supported-tooltip";
const KEY_NOT_REPORTED: &str = "perf-fallback-not-reported";
const KEY_UNAVAILABLE: &str = "perf-fallback-unavailable";
const KEY_UNKNOWN: &str = "perf-fallback-unknown";
const KEY_COULD_NOT_READ: &str = "perf-fallback-could-not-read";
const KEY_LAST_UPDATED: &str = "perf-last-updated";
/// Indent of each copied line (PERF-011).
const COPY_INDENT: &str = "    ";

/// Stand-ins for the arguments of a [`Template`]: private-use characters no translation has.
const HOLES: [&str; 4] = ["\u{E000}", "\u{E001}", "\u{E002}", "\u{E003}"];

/// A translated message whose arguments are left as holes, so a new value is spliced in
/// without going through Fluent. Numbers never pass through Fluent anyway (R-081).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Template(SharedString);

impl Template {
    /// Translates `key` with the arguments `args` (at most four) as holes.
    pub fn new(cx: &App, key: &str, args: &[&str]) -> Self {
        Self(tr_args(cx, key, &holes(args)))
    }

    /// [`Template::new`] in en-US, for a table built before the strings can be read.
    pub fn en(key: &str, args: &[&str]) -> Self {
        Self(tr_en(key, &holes(args)))
    }

    /// Writes the text with `values[i]` in the place of argument `i`.
    pub fn write(&self, out: &mut dyn fmt::Write, values: &[&dyn fmt::Display]) -> fmt::Result {
        let mut rest = self.0.as_ref();
        while let Some((at, i)) = next_hole(rest) {
            out.write_str(&rest[..at])?;
            if let Some(value) = values.get(i) {
                write!(out, "{value}")?;
            }
            rest = &rest[at + HOLES[i].len()..];
        }
        out.write_str(rest)
    }

    /// Writes the text, with `arg(i, out)` writing argument `i` in its place.
    pub fn write_each(&self, out: &mut String, mut arg: impl FnMut(usize, &mut String)) {
        let mut rest = self.0.as_ref();
        while let Some((at, i)) = next_hole(rest) {
            out.push_str(&rest[..at]);
            arg(i, out);
            rest = &rest[at + HOLES[i].len()..];
        }
        out.push_str(rest);
    }

    /// The text with no arguments.
    pub fn text(&self) -> &SharedString {
        &self.0
    }
}

/// The Fluent arguments that put a hole in the place of each of `names`.
fn holes<'a>(names: &[&'a str]) -> SmallVec<[(&'a str, FluentValue<'static>); 4]> {
    names
        .iter()
        .zip(HOLES)
        .map(|(name, hole)| (*name, hole.into()))
        .collect()
}

/// The byte offset and index of the first hole in `s`.
fn next_hole(s: &str) -> Option<(usize, usize)> {
    s.char_indices().find_map(|(at, c)| {
        let i = (c as u32).checked_sub(0xE000)? as usize;
        (i < HOLES.len()).then_some((at, i))
    })
}

/// The Details words, translated once per strings version. A new table is in en-US.
#[derive(Clone, Debug)]
pub struct DetailWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub collecting: SharedString,
    pub restricted: SharedString,
    pub restricted_tip: SharedString,
    pub needs_setup: SharedString,
    pub needs_setup_tip: SharedString,
    pub not_supported: SharedString,
    pub not_supported_tip: SharedString,
    pub not_reported: SharedString,
    pub unavailable: SharedString,
    pub unknown: SharedString,
    /// `{$reason}`: the source's last error.
    pub could_not_read: Template,
    /// `{$age}`: the short age form.
    pub last_updated: Template,
}

impl Default for DetailWords {
    fn default() -> Self {
        let en = |key| tr_en(key, &[]);
        Self {
            version: None,
            collecting: en(KEY_COLLECTING),
            restricted: en(KEY_RESTRICTED),
            restricted_tip: en(KEY_RESTRICTED_TIP),
            needs_setup: en(KEY_NEEDS_SETUP),
            needs_setup_tip: en(KEY_NEEDS_SETUP_TIP),
            not_supported: en(KEY_NOT_SUPPORTED),
            not_supported_tip: en(KEY_NOT_SUPPORTED_TIP),
            not_reported: en(KEY_NOT_REPORTED),
            unavailable: en(KEY_UNAVAILABLE),
            unknown: en(KEY_UNKNOWN),
            could_not_read: Template::en(KEY_COULD_NOT_READ, &["reason"]),
            last_updated: Template::en(KEY_LAST_UPDATED, &["age"]),
        }
    }
}

impl DetailWords {
    /// Translates every word when the strings changed since the last call. True when it did.
    /// Without strings (tests) the words stay as they are.
    pub fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        if version == Some(None) {
            return false;
        }
        let t = |key| tr(cx, key);
        self.collecting = t(KEY_COLLECTING);
        self.restricted = t(KEY_RESTRICTED);
        self.restricted_tip = t(KEY_RESTRICTED_TIP);
        self.needs_setup = t(KEY_NEEDS_SETUP);
        self.needs_setup_tip = t(KEY_NEEDS_SETUP_TIP);
        self.not_supported = t(KEY_NOT_SUPPORTED);
        self.not_supported_tip = t(KEY_NOT_SUPPORTED_TIP);
        self.not_reported = t(KEY_NOT_REPORTED);
        self.unavailable = t(KEY_UNAVAILABLE);
        self.unknown = t(KEY_UNKNOWN);
        self.could_not_read = Template::new(cx, KEY_COULD_NOT_READ, &["reason"]);
        self.last_updated = Template::new(cx, KEY_LAST_UPDATED, &["age"]);
        true
    }
}

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
/// is `words.could_not_read` with the last error, which the caller fills in.
pub fn fallback_word(
    words: &DetailWords,
    v: Validity,
    f: Fallback,
) -> (&SharedString, Option<&SharedString>) {
    const NONE: &SharedString = &SharedString::new_static("");
    let w = words;
    if f == Fallback::Unknown {
        return (&w.unknown, None);
    }
    match v {
        Validity::Valid | Validity::Stale | Validity::Missing => (NONE, None),
        Validity::WarmingUp => (&w.collecting, None),
        Validity::Restricted => (&w.restricted, Some(&w.restricted_tip)),
        Validity::Unsupported => match f {
            Fallback::NeedsSetup => (&w.needs_setup, Some(&w.needs_setup_tip)),
            Fallback::NotSupported => (&w.not_supported, Some(&w.not_supported_tip)),
            Fallback::NotReported => (&w.not_reported, None),
            Fallback::Unavailable | Fallback::Unknown => (&w.unavailable, None),
        },
        Validity::Failed => (&w.unavailable, None),
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
    /// A row that is collecting its first value; `collecting` is the word it shows.
    fn new(label: &'static str, collecting: SharedString) -> Self {
        Self {
            label,
            value: collecting,
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
        // The grid has its own English word for each non-valid state, so a fallback word goes
        // in as a Stale value: the same `text_secondary`, with the translated text. Needs setup
        // goes in as a Valid link.
        let validity = match self.validity {
            Validity::Valid | Validity::Stale | Validity::Missing => self.validity,
            _ if self.link => Validity::Valid,
            _ => Validity::Stale,
        };
        TlValue {
            text: self.value.clone(),
            validity,
            word: None,
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

/// A text the owner puts in tooltips: its key, its argument names and the translation.
struct OwnerText {
    key: &'static str,
    args: &'static [&'static str],
    text: Template,
}

/// The rows of one Details grid, in grid order. The page keeps it and calls the setters on each
/// snapshot; they return true when the grid needs a redraw. The page calls
/// [`KeyValueModel::refresh_strings`] in its render.
#[derive(Default)]
pub struct KeyValueModel {
    pub rows: Vec<KvRow>,
    scratch: String,
    labels: RefCell<Labels>,
    words: DetailWords,
    texts: Vec<OwnerText>,
}

impl KeyValueModel {
    /// One collecting row per label, in this order.
    pub fn new(labels: &[&'static str]) -> Self {
        let words = DetailWords::default();
        Self {
            rows: labels
                .iter()
                .map(|l| KvRow::new(l, words.collecting.clone()))
                .collect(),
            words,
            ..Self::default()
        }
    }

    /// Adds texts the owner reads with [`KeyValueModel::text`], translated with the words.
    /// Each is a key and the names of its arguments.
    pub fn with_texts(mut self, texts: &[(&'static str, &'static [&'static str])]) -> Self {
        self.texts
            .extend(texts.iter().map(|&(key, args)| OwnerText {
                key,
                args,
                text: Template::en(key, args),
            }));
        self
    }

    /// The translation of a text from [`KeyValueModel::with_texts`]; an unknown key reads as
    /// empty.
    pub fn text(&self, key: &'static str) -> Template {
        self.texts
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    pub fn words(&self) -> &DetailWords {
        &self.words
    }

    /// Translates the words and texts when the strings changed since the last call, and puts
    /// the new words in the rows that show one. A row's Stale tooltip and the owner's
    /// tooltips change on the next update. True when the strings changed.
    pub fn refresh_strings(&mut self, cx: &App) -> bool {
        let old = self.words.clone();
        if !self.words.refresh(cx) {
            return false;
        }
        for t in &mut self.texts {
            t.text = Template::new(cx, t.key, t.args);
        }
        for row in &mut self.rows {
            let Some(fallback) = row.fallback else {
                if row.validity == Validity::WarmingUp && !row.formatted {
                    row.value = self.words.collecting.clone();
                }
                continue;
            };
            let (word, tip) = fallback_word(&self.words, row.validity, fallback);
            row.value = word.clone();
            row.tooltip = match (row.validity, &row.tooltip) {
                (Validity::Failed, Some(text)) => {
                    let reason = strip_template(&old.could_not_read, text);
                    self.scratch.clear();
                    let _ = self
                        .words
                        .could_not_read
                        .write(&mut self.scratch, &[&reason]);
                    Some(SharedString::from(self.scratch.as_str()))
                }
                (Validity::Failed, None) => None,
                _ => tip.cloned(),
            };
        }
        self.refresh_labels(cx);
        true
    }

    /// The row with `label`. A new label adds a row at the end.
    fn index(&mut self, label: &'static str) -> usize {
        if let Some(i) = self.rows.iter().position(|r| r.label == label) {
            return i;
        }
        self.rows
            .push(KvRow::new(label, self.words.collecting.clone()));
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
        let link = validity == Validity::Unsupported && fallback == Fallback::NeedsSetup;
        let i = self.index(label);
        let row = &mut self.rows[i];
        let (word, tip) = fallback_word(&self.words, validity, fallback);

        // The Failed tooltip is built in the scratch buffer, so it allocates only on change.
        self.scratch.clear();
        let tooltip: Option<&str> = match validity {
            Validity::Failed => reason.map(|reason| {
                let _ = self
                    .words
                    .could_not_read
                    .write(&mut self.scratch, &[&reason]);
                self.scratch.as_str()
            }),
            _ => tip.map(|t| t.as_ref()),
        };
        let same = row.validity == validity
            && row.fallback == Some(fallback)
            && row.value == *word
            && row.link == link
            && row.tooltip.as_deref() == tooltip;
        if same {
            return false;
        }
        row.validity = validity;
        row.fallback = Some(fallback);
        row.formatted = false;
        row.value = word.clone();
        row.tooltip = match tooltip {
            None => None,
            Some(_) if validity != Validity::Failed => tip.cloned(),
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
        self.words
            .last_updated
            .write_each(&mut self.scratch, |_, out| {
                short_age(out, age_ns / 1_000_000_000)
            });
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

/// The argument of a one-argument `template` in `text`, the text it wrote. `text` itself when it
/// does not match.
fn strip_template<'a>(template: &Template, text: &'a str) -> &'a str {
    let t = template.text().as_ref();
    let Some((at, _)) = next_hole(t) else {
        return text;
    };
    let (before, after) = (&t[..at], &t[at + HOLES[0].len()..]);
    text.strip_prefix(before)
        .and_then(|rest| rest.strip_suffix(after))
        .unwrap_or(text)
}

/// The copy form of the Details value under the last right-click, for the page menu's "Copy
/// value" (UI-PRF-020). The Performance tab clears it when a right press starts and reads it
/// once the press is handled; `None` when the press was not on a value.
#[derive(Default)]
pub struct ClickedValue(pub Option<SharedString>);

impl Global for ClickedValue {}

/// The Details grid: stacked label over value, three columns `sp6` apart, pairs `kv_row_h`
/// apart, filled left to right and top to bottom in model order. Translates the labels first if
/// needed. Values are selectable text (U-46). A right-click on a value names it in
/// [`ClickedValue`] instead of opening the grid's own menu, so only the page menu opens.
pub fn render_details(model: &KeyValueModel, m: &UiMetrics, cx: &App) -> TlKeyValueGrid {
    model.refresh_labels(cx);
    let rows: Vec<TlKvRow> = model.to_kv_rows().collect();
    let values: Rc<[SharedString]> = rows.iter().map(|r| r.value.text.clone()).collect();
    let section = TlKvSection {
        title: None,
        blocks: vec![TlKvBlock {
            indent: None,
            heading: None,
            rows,
        }],
    };
    // The grid's row height is half a stacked pair.
    TlKeyValueGrid::new(GRID_ID, KvLayout::Stacked)
        .sections(&[section])
        .columns(3)
        .column_gap(m.sp[6])
        .row_height(px(KV_ROW_H / 2. * m.ui_scale))
        .on_context_menu(move |n, _, _, cx| {
            let value = values
                .get(n)
                .map(|v| SharedString::from(value_copy_form(v).into_owned()));
            cx.set_global(ClickedValue(value));
        })
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
