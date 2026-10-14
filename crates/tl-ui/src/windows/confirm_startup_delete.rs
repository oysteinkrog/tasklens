//! "tasklens Delete Startup Entry" confirmation (UI-SYS-047).
//!
//! A `TlDialog` window, modal to the main window. It lists only the entries the user may delete.
//! "Delete" closes it at once and calls `on_delete`; the result toast in the main window reports
//! the outcome, and `SysActions::submit` refuses a second delete on a pending target (leader
//! decision on tasklens-9o0.10). Focus starts on Cancel, so a stray Enter cancels.

use fluent_bundle::FluentValue;
use gpui_kit::base::SelectableText;
use gpui_kit::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div,
};
use smallvec::SmallVec;
use tl_model::snapshot::StartupKind;

use crate::components::dialog::{DialogResult, TlDialog};
use crate::i18n::{tr, tr_args};
use crate::text::style::{label_style, t_base, t_sm};
use crate::theme::{current, metrics};

/// Names listed in the several-entries form before "and <m> more".
const MAX_NAMES: usize = 5;

/// One deletable entry, captured when the menu opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupEntryView {
    pub name: SharedString,
    pub source_path: SharedString,
    pub kind: StartupKind,
}

/// The texts of the dialog body, built from the entries once.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DeleteTexts {
    pub heading: SharedString,
    pub text: SharedString,
    /// One-entry form only: what deleting does for this kind of entry.
    pub type_line: Option<SharedString>,
    /// The entry name (one entry) or up to five names (several).
    pub names: Vec<SharedString>,
    /// One-entry form only.
    pub path: Option<SharedString>,
    /// "and <m> more".
    pub more: Option<SharedString>,
    /// "<n> entries cannot be deleted and are skipped."
    pub skipped: Option<SharedString>,
}

fn count(n: usize) -> FluentValue<'static> {
    FluentValue::from(n as i64)
}

fn type_key(kind: StartupKind) -> &'static str {
    match kind {
        StartupKind::UserUnit => "startup-delete-type-unit",
        StartupKind::PlasmaPreStartup | StartupKind::PlasmaLogout => "startup-delete-type-script",
        _ => "startup-delete-type-xdg",
    }
}

/// The dialog texts for `entries` (all deletable) and `skipped` entries left out.
pub fn delete_texts(entries: &[StartupEntryView], skipped: usize, cx: &App) -> DeleteTexts {
    let mut out = DeleteTexts::default();
    if let [one] = entries {
        out.heading = tr(cx, "startup-delete-heading-one");
        out.text = tr(cx, "startup-delete-text-one");
        out.type_line = Some(tr(cx, type_key(one.kind)));
        out.names = vec![one.name.clone()];
        out.path = Some(one.source_path.clone());
    } else {
        out.heading = tr(cx, "startup-delete-heading-many");
        out.text = tr_args(
            cx,
            "startup-delete-text-many",
            &[("n", count(entries.len()))],
        );
        out.names = entries
            .iter()
            .take(MAX_NAMES)
            .map(|e| e.name.clone())
            .collect();
        if entries.len() > MAX_NAMES {
            out.more = Some(tr_args(
                cx,
                "startup-delete-more",
                &[("m", count(entries.len() - MAX_NAMES))],
            ));
        }
    }
    if skipped > 0 {
        out.skipped = Some(tr_args(
            cx,
            "startup-delete-skipped",
            &[("n", count(skipped))],
        ));
    }
    out
}

/// The dialog, not yet opened.
pub fn delete_dialog(texts: &DeleteTexts, cx: &App) -> TlDialog {
    let body = texts.clone();
    TlDialog::confirm(texts.text.clone(), tr(cx, "startup-delete-confirm"))
        .destructive()
        .title(tr(cx, "startup-delete-title"))
        .heading(texts.heading.clone())
        .cancel_label(tr(cx, "startup-delete-cancel"))
        .extra(move |_, cx| lines(&body, cx))
}

fn lines(texts: &DeleteTexts, cx: &mut App) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    // (text, secondary colour, medium weight)
    let mut rows: Vec<(SharedString, bool, bool)> = Vec::new();
    rows.extend(texts.type_line.iter().map(|t| (t.clone(), true, false)));
    rows.extend(texts.names.iter().map(|t| (t.clone(), false, true)));
    rows.extend(texts.more.iter().map(|t| (t.clone(), false, false)));
    rows.extend(texts.skipped.iter().map(|t| (t.clone(), true, false)));
    let mut out = div().flex().flex_col().gap(m.sp[4]);
    for (text, secondary, medium) in rows {
        let color = if secondary {
            theme.text_secondary
        } else {
            theme.text_primary
        };
        let mut line = div();
        *line.text_style() = if medium {
            label_style(&m, m.text.base, FontWeight::MEDIUM)
        } else {
            t_base(&m)
        };
        out = out.child(line.text_color(color).child(text));
    }
    if let Some(path) = &texts.path {
        let mut line = div();
        *line.text_style() = t_sm(&m);
        out = out.child(
            line.text_color(theme.text_secondary)
                .child(SelectableText::new("startup-delete-path", path.clone())),
        );
    }
    out.into_any_element()
}

/// Opens the confirmation modal to `parent`. `entries` holds only deletable entries and `skipped`
/// counts the rest. `on_delete` runs in the parent window when "Delete" is chosen; Cancel, Esc
/// and the close button run nothing. If the window cannot open, nothing is deleted.
pub fn open_startup_delete(
    parent: &mut Window,
    entries: SmallVec<[StartupEntryView; 4]>,
    skipped: usize,
    on_delete: impl FnOnce(&mut Window, &mut App) + 'static,
    cx: &mut App,
) -> gpui_kit::Result<()> {
    let texts = delete_texts(&entries, skipped, cx);
    delete_dialog(&texts, cx).open(parent, cx, move |result, window, cx| {
        if matches!(result, DialogResult::Confirmed { .. }) {
            on_delete(window, cx);
        }
    })
}
