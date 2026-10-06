//! Status cells for the system tables (UI-SYS-003, SYS-003, SYS-020, SYS-029, APP-058).
//!
//! The Services, Startup Apps and Connections tables share these look tables and the one cell
//! painter, so a state has the same colour in every table. The label always shows; colour is a
//! second cue only.
//!
//! The labels are translated once per strings version into [`StatusWords`] (R-132). A render
//! reads them with [`status_words`] or [`status_label`] and clones a `SharedString`, so painting
//! a cell neither formats nor allocates.

use gpui_kit::component::h_flex;
use gpui_kit::{
    App, Global, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::FluentBuilder as _, px,
};

use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::t_base;
use crate::theme::{TlTheme, UiMetrics};

/// One label colour role; resolved against TlTheme at paint time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LabelTone {
    Primary,
    Secondary,
    Danger,
    Warning,
    Info,
    Success,
}

/// Dot shape and colour role. `None` draws no dot (Connections).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DotStyle {
    Filled(LabelTone),
    Ring(LabelTone),
    None,
}

/// The UI-side status words of the three tables. Pages map their model enums onto these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SysStatus {
    // Services (SYS-003 labels)
    Running,
    Active,
    Starting,
    Stopping,
    Reloading,
    Stopped,
    Failed,
    Maintenance,
    // Startup Apps (SYS-020)
    Enabled,
    Disabled,
    Masked,
    NotInThisDesktop,
    MissingProgram,
}

impl SysStatus {
    /// Every value, in declaration order.
    pub const ALL: [SysStatus; 13] = [
        SysStatus::Running,
        SysStatus::Active,
        SysStatus::Starting,
        SysStatus::Stopping,
        SysStatus::Reloading,
        SysStatus::Stopped,
        SysStatus::Failed,
        SysStatus::Maintenance,
        SysStatus::Enabled,
        SysStatus::Disabled,
        SysStatus::Masked,
        SysStatus::NotInThisDesktop,
        SysStatus::MissingProgram,
    ];

    /// The message key of the label.
    pub fn key(self) -> &'static str {
        match self {
            SysStatus::Running => "sys-sys003-running",
            SysStatus::Active => "sys-sys003-active",
            SysStatus::Starting => "sys-sys003-starting",
            SysStatus::Stopping => "sys-sys003-stopping",
            SysStatus::Reloading => "sys-sys003-reloading",
            SysStatus::Stopped => "sys-sys003-stopped",
            SysStatus::Failed => "sys-sys003-failed",
            SysStatus::Maintenance => "sys-sys003-maintenance",
            SysStatus::Enabled => "sys-sys003-enabled",
            SysStatus::Disabled => "sys-sys003-disabled",
            SysStatus::Masked => "sys-sys003-masked",
            SysStatus::NotInThisDesktop => "sys-sys003-not-in-this-desktop",
            SysStatus::MissingProgram => "sys-sys003-missing-program",
        }
    }
}

/// Connections State column classes (SYS-029). Labels are the page's (E28); only colour lives here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnStateClass {
    Listening,
    Established,
    Transitional,
    TimeWaitOrClosed,
    Unknown,
    Udp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatusLook {
    pub dot: DotStyle,
    pub label: LabelTone,
}

/// The dot and label tones of `s`. `Success` is a dot tone only.
pub fn status_look(s: SysStatus) -> StatusLook {
    use LabelTone::{Danger, Info, Primary, Secondary, Success, Warning};
    let (dot, label) = match s {
        SysStatus::Running | SysStatus::Enabled => (DotStyle::Filled(Success), Primary),
        SysStatus::Active => (DotStyle::Ring(Success), Primary),
        SysStatus::Starting | SysStatus::Stopping | SysStatus::Reloading => {
            (DotStyle::Filled(Info), Primary)
        }
        SysStatus::Stopped | SysStatus::Disabled | SysStatus::NotInThisDesktop => {
            (DotStyle::Ring(Secondary), Secondary)
        }
        SysStatus::Failed => (DotStyle::Filled(Danger), Danger),
        SysStatus::Maintenance | SysStatus::Masked | SysStatus::MissingProgram => {
            (DotStyle::Filled(Warning), Primary)
        }
    };
    StatusLook { dot, label }
}

/// The status labels, translated once per strings version. Installed as a GPUI global; read it
/// with [`status_words`].
#[derive(Clone, Debug)]
pub struct StatusWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    /// Indexed by `SysStatus as usize`.
    labels: [SharedString; 13],
}

impl Global for StatusWords {}

impl Default for StatusWords {
    /// The en-US labels, for tables built before the strings are installed.
    fn default() -> Self {
        Self {
            version: None,
            labels: SysStatus::ALL.map(|s| tr_en(s.key(), &[])),
        }
    }
}

impl StatusWords {
    /// The label of `s`.
    pub fn label(&self, s: SysStatus) -> &SharedString {
        &self.labels[s as usize]
    }
}

/// The status labels, translated again first when the strings changed.
pub fn status_words(cx: &mut App) -> &StatusWords {
    let version = cx.try_global::<Strings>().map(|s| s.strings_version);
    if cx
        .try_global::<StatusWords>()
        .is_none_or(|w| w.version != version)
    {
        let words = match version {
            Some(_) => StatusWords {
                version,
                labels: SysStatus::ALL.map(|s| tr(cx, s.key())),
            },
            None => StatusWords::default(),
        };
        cx.set_global(words);
    }
    cx.global::<StatusWords>()
}

/// The label of `s` in the current language. Interned once per strings version; a call clones a
/// `SharedString` and formats nothing.
pub fn status_label(cx: &mut App, s: SysStatus) -> SharedString {
    status_words(cx).label(s).clone()
}

/// The tone of a Connections State class (SYS-029).
pub fn conn_state_tone(c: ConnStateClass) -> LabelTone {
    match c {
        ConnStateClass::Listening => LabelTone::Info,
        ConnStateClass::Established | ConnStateClass::Transitional => LabelTone::Primary,
        ConnStateClass::TimeWaitOrClosed | ConnStateClass::Udp => LabelTone::Secondary,
        ConnStateClass::Unknown => LabelTone::Warning,
    }
}

/// The colour of `t`. Only Primary and Secondary change on a selected row; status colours stay,
/// so a failure is still visible when selected.
pub fn tone_color(t: LabelTone, theme: &TlTheme, selected: bool) -> Hsla {
    match (t, selected) {
        (LabelTone::Primary, false) => theme.text_primary,
        (LabelTone::Primary, true) => theme.text_selected_primary,
        (LabelTone::Secondary, false) => theme.text_secondary,
        (LabelTone::Secondary, true) => theme.text_selected_secondary,
        (LabelTone::Danger, _) => theme.status.danger,
        (LabelTone::Warning, _) => theme.status.warning,
        (LabelTone::Info, _) => theme.status.info,
        (LabelTone::Success, _) => theme.status.success,
    }
}

/// Alpha factor of a stale dot.
pub const STALE_DOT_ALPHA: f32 = 0.5;

/// The resolved colours of one status cell.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CellColors {
    /// `None` for `DotStyle::None`.
    pub dot: Option<Hsla>,
    pub label: Hsla,
}

/// The colours [`status_cell`] paints. Dots keep their colour on a selected row. A stale cell
/// has a `text_secondary` label and a dot at half alpha.
pub fn cell_colors(look: StatusLook, selected: bool, stale: bool, theme: &TlTheme) -> CellColors {
    let dot = match look.dot {
        DotStyle::Filled(t) | DotStyle::Ring(t) => Some(tone_color(t, theme, false)),
        DotStyle::None => None,
    };
    if stale {
        CellColors {
            dot: dot.map(|c| c.opacity(STALE_DOT_ALPHA)),
            label: theme.text_secondary,
        }
    } else {
        CellColors {
            dot,
            label: tone_color(look.label, theme, selected),
        }
    }
}

/// The status cell: dot 8 x 8, gap sp3 (6 px), label t_base with ellipsis. `DotStyle::None`
/// draws no dot and no gap.
pub fn status_cell(
    look: StatusLook,
    label: SharedString,
    selected: bool,
    stale: bool,
    theme: &TlTheme,
    metrics: &UiMetrics,
) -> impl IntoElement {
    let colors = cell_colors(look, selected, stale, theme);
    let dot = colors.dot.map(|color| {
        let d = div()
            .debug_selector(|| "sys-status-dot".into())
            .flex_none()
            .size(px(8.0 * metrics.ui_scale))
            .rounded_full();
        match look.dot {
            DotStyle::Ring(_) => d.border(px(1.5 * metrics.ui_scale)).border_color(color),
            _ => d.bg(color),
        }
    });
    let mut text = div()
        .debug_selector(|| "sys-status-label".into())
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(colors.label)
        .child(label);
    *text.text_style() = t_base(metrics);
    h_flex()
        .debug_selector(|| "sys-status-cell".into())
        .min_w_0()
        .items_center()
        .when_some(dot, |row, dot| row.gap(metrics.sp[3]).child(dot))
        .child(text)
}
