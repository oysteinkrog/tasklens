//! The process row tooltip on the Name cell (PROC-039, R-009, PROC-012, UI-PRC-012).
//!
//! Lines in this order, each only when it applies: the command line (first 512 characters, or
//! `[comm]` for a kernel thread), `path:`, `user:`, `Unit:`, `cgroup:`, `Desktop file:` (app
//! rows) and the restricted note in `status.warning`. Pseudo rows show one sentence. Group
//! headers have no tooltip. The Package line is P2 and waits for the package data.
//!
//! The table asks for the tooltip only on the hovered row, once its tooltips are ready, and its
//! `HoverTooltip` shows it after 600 ms (`DELAY_ROW_NAME`). The lines are built then and kept
//! until the snapshot or the row changes. The one [`RowTooltip`] view per table reads them.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::Tooltip as BaseTooltip;
use gpui_kit::{
    App, BoxShadow, Context, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, StyledText, Window, div, point, px,
};
use tl_model::ids::{NodeId, PseudoKind};
use tl_model::process::{GroupRowKind, ProcessRecord};
use tl_model::snapshot::Snapshot;
use tl_model::validity::{FieldValidity, Validity};

use super::cells::RowRef;
use crate::i18n::tr;
use crate::table::tree_table::RowKind;
use crate::text::style::t_sm;
use crate::theme::{current, metrics};

/// The command line line keeps this many characters (PROC-039).
pub const TIP_CMDLINE_CHARS: usize = 512;
/// Max width of the tooltip, × ui_scale (R-111, the `TlTooltip::lines` width).
const MAX_WIDTH: f32 = 560.;

/// One line: an optional label and the value.
pub type TipLine = (Option<SharedString>, SharedString);

/// The tooltip words, translated once per `strings_version`.
#[derive(Clone, Debug)]
pub struct RowTipWords {
    pub path: SharedString,
    pub restricted: SharedString,
    pub user: SharedString,
    pub uid: SharedString,
    pub unit: SharedString,
    pub cgroup: SharedString,
    pub desktop_file: SharedString,
    pub restricted_note: SharedString,
    pub interrupts: SharedString,
    pub memory_compression: SharedString,
}

impl RowTipWords {
    /// Every word through `tr`.
    pub fn new(cx: &App) -> Self {
        Self::from_fn(|key| tr(cx, key))
    }

    /// Every word from `text(fluent_key)`.
    pub fn from_fn(mut text: impl FnMut(&'static str) -> SharedString) -> Self {
        Self {
            path: text("processes-tooltip-path"),
            restricted: text("processes-tooltip-restricted"),
            user: text("processes-tooltip-user"),
            uid: text("processes-tooltip-uid"),
            unit: text("processes-tooltip-unit"),
            cgroup: text("processes-tooltip-cgroup"),
            desktop_file: text("processes-tooltip-desktop-file"),
            restricted_note: text("processes-tooltip-some-values-need-administrator-access"),
            interrupts: text("processes-tooltip-cpu-time-spent-in-hardware"),
            memory_compression: text("processes-tooltip-memory-held-by-zram-compressed"),
        }
    }
}

fn restricted(v: &FieldValidity) -> bool {
    v.0.contains(&Validity::Restricted)
}

/// Fills `out` with the tooltip lines of `row` from `snapshot`. True when the last line is the
/// restricted note, which shows in `status.warning`. Leaves `out` empty for a row with nothing
/// to show (the Kernel threads row). Called only when the tooltip is about to show.
pub fn row_tooltip_lines(
    row: RowRef,
    snapshot: &Snapshot,
    words: &RowTipWords,
    out: &mut Vec<TipLine>,
) -> bool {
    out.clear();
    let p = &snapshot.processes;
    let (member, unit, desktop, warn) = match row {
        RowRef::Group(g, _) => {
            if let NodeId::Pseudo(kind) = g.node {
                let sentence = match kind {
                    PseudoKind::Interrupts => &words.interrupts,
                    PseudoKind::MemoryCompression => &words.memory_compression,
                };
                out.push((None, sentence.clone()));
                return false;
            }
            // The earliest started member stands for the app or unit, as in its text cells.
            // `records` is sorted by start time.
            let member = match (&g.kind, &g.node) {
                (GroupRowKind::App, _) => p.records.iter().position(|r| r.group == g.key),
                (GroupRowKind::Unit, NodeId::Unit(u)) => {
                    p.extras.iter().position(|x| x.unit.as_ref() == Some(u))
                }
                _ => None,
            };
            let unit = match &g.node {
                NodeId::Unit(u) => Some((u, g.description.as_ref())),
                _ => member
                    .and_then(|i| p.extras.get(i))
                    .and_then(|x| x.unit.as_ref())
                    .map(|u| (u, None)),
            };
            let desktop = (g.kind == GroupRowKind::App)
                .then_some(g.desktop_path.as_ref())
                .flatten();
            let member = member.and_then(|i| p.records.get(i)).map(|r| (r, false));
            (member, unit, desktop, restricted(&g.validity))
        }
        RowRef::Record(r, row, kind) => {
            let unit = row
                .record
                .and_then(|i| p.extras.get(i as usize))
                .and_then(|x| x.unit.as_ref())
                .map(|u| (u, None));
            let kthread = kind == RowKind::KernelThread;
            (Some((r, kthread)), unit, None, restricted(&r.validity))
        }
    };

    if let Some((r, kthread)) = member {
        process_lines(r, kthread, words, out);
    }
    if let Some((u, own)) = unit {
        let description = snapshot
            .services
            .units
            .iter()
            .find(|rec| rec.unit == *u)
            .map(|rec| &rec.description)
            .filter(|d| !d.is_empty())
            .or(own);
        let value = match description {
            Some(d) => format!("{} ({d})", u.name).into(),
            None => SharedString::from(&u.name),
        };
        out.push((Some(words.unit.clone()), value));
    }
    if let Some((r, _)) = member
        && let Some(cgroup) = &r.meta.cgroup
    {
        out.push((Some(words.cgroup.clone()), cgroup.into()));
    }
    if let Some(path) = desktop {
        out.push((Some(words.desktop_file.clone()), path.into()));
    }
    if out.is_empty() {
        return false;
    }
    if warn {
        out.push((None, words.restricted_note.clone()));
    }
    warn
}

/// The command line, path and user lines of one process.
fn process_lines(r: &ProcessRecord, kthread: bool, words: &RowTipWords, out: &mut Vec<TipLine>) {
    let meta = &r.meta;
    if kthread {
        out.push((None, format!("[{}]", meta.comm).into()));
    } else if let Some(cmdline) = meta.cmdline.as_ref().filter(|c| !c.is_empty()) {
        let end = cmdline
            .char_indices()
            .nth(TIP_CMDLINE_CHARS)
            .map_or(cmdline.len(), |(i, _)| i);
        out.push((None, SharedString::new(&cmdline[..end])));
    }
    // A kernel thread has no executable, so its path is not restricted, just absent.
    if !kthread {
        let path = meta
            .exe
            .as_ref()
            .map_or_else(|| words.restricted.clone(), SharedString::from);
        out.push((Some(words.path.clone()), path));
    }
    out.push((
        Some(words.user.clone()),
        format!("{} ({} {})", meta.user, words.uid, r.uid).into(),
    ));
}

/// The built lines of the shown row, shared by the delegate and the [`RowTooltip`] view.
#[derive(Default)]
pub struct RowTipState {
    /// `(snapshot sequence, table version, row index)` the lines were built for.
    built_for: Option<(u64, u64, usize)>,
    /// Each line as one text with the label's byte range.
    lines: Vec<(SharedString, Option<Range<usize>>)>,
    warning: bool,
    scratch: Vec<TipLine>,
}

impl RowTipState {
    /// Builds the lines of `row` unless they were built for the same `key`. True when there is
    /// something to show.
    pub fn update(
        &mut self,
        key: (u64, u64, usize),
        row: RowRef,
        snapshot: &Snapshot,
        words: &RowTipWords,
    ) -> bool {
        if self.built_for != Some(key) {
            self.built_for = Some(key);
            let mut scratch = std::mem::take(&mut self.scratch);
            self.warning = row_tooltip_lines(row, snapshot, words, &mut scratch);
            self.lines.clear();
            self.lines
                .extend(scratch.drain(..).map(|(label, value)| match label {
                    Some(label) => {
                        let text = format!("{label} {value}");
                        (text.into(), Some(0..label.len()))
                    }
                    None => (value, None),
                }));
            self.scratch = scratch;
        }
        !self.lines.is_empty()
    }

    /// Forgets the built lines, after a locale change.
    pub fn clear(&mut self) {
        self.built_for = None;
        self.lines.clear();
    }

    /// The shown texts, label and value joined by a space.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|(t, _)| t.as_ref())
    }

    /// True when the last line is the restricted note.
    pub fn warning(&self) -> bool {
        self.warning
    }
}

/// The row tooltip view. One per table; it draws the lines of the hovered row.
pub struct RowTooltip {
    state: Rc<RefCell<RowTipState>>,
}

impl RowTooltip {
    pub fn new(state: Rc<RefCell<RowTipState>>) -> Self {
        Self { state }
    }
}

impl Render for RowTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let state = self.state.borrow();
        let last = state.lines.len().saturating_sub(1);
        let label = HighlightStyle {
            color: Some(theme.text_secondary),
            ..HighlightStyle::default()
        };
        let lines = state.lines.iter().enumerate().map(|(i, (text, range))| {
            let warning = state.warning && i == last;
            let color: Hsla = if warning {
                theme.status.warning
            } else {
                theme.text_primary
            };
            let mut line = div().debug_selector(move || {
                if warning {
                    "TlRowTip-warning".into()
                } else {
                    "TlRowTip-line".into()
                }
            });
            *line.text_style() = t_sm(&m);
            let styled = match range {
                Some(r) => StyledText::new(text.clone()).with_highlights([(r.clone(), label)]),
                None => StyledText::new(text.clone()),
            };
            line.text_color(color).child(styled)
        });

        let mut panel = BaseTooltip::new("tl-row-tooltip")
            .flex()
            .flex_col()
            .max_w(px(MAX_WIDTH * m.ui_scale))
            .py(m.sp[2])
            .px(m.sp[4])
            .bg(theme.menu_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(m.radius_small)
            .children(lines);
        panel.style().box_shadow = Some(vec![BoxShadow {
            color: theme.derived.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }]);
        div().debug_selector(|| "TlRowTip".into()).child(panel)
    }
}
