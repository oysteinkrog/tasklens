//! `TlKeyValueGrid`: label and value pairs for Performance stats, Information cards and
//! Properties General (UI-FND-025, PERF-011 to PERF-013, PROC-078, R-050).
//!
//! Two layouts:
//! - Stacked (Performance stats): label `t_sm` `text_secondary` over value `t_base`
//!   `text_primary` with `tnum`, three equal columns of at least 160 × ui_scale, column gap `sp7`,
//!   row gap 0, pair height 2 × row height.
//! - Inline (Information, Properties General): one row per pair, the label column at a fixed
//!   width, the value after it. The owner writes the ":" into the label.
//!
//! A section with a title gets a band: `header` fill, row height high, a 3 px `accent` bar at the
//! left edge, the title in `t_md` 500, and a copy button ("Copy section") at the right end. A
//! block with a heading ("Module 1") indents its rows by `block_indent`; blocks are `block_gap`
//! apart.
//!
//! Value words per validity (PERF-013), every non-valid word in `text_secondary`:
//!
//! | Validity | Shows | Tooltip |
//! |---|---|---|
//! | Valid | the text, `text_primary` | the owner's, if any |
//! | WarmingUp | "Collecting…" | the owner's, if any |
//! | Stale | the last value | the owner's "Last updated {age} ago" |
//! | Restricted | "Restricted" | the owner's, if any |
//! | Unsupported | "Not supported", "Not reported" or "Needs setup" (a link) | the owner's, if any |
//! | Missing | the row is not drawn | |
//! | Failed | "Unavailable" | the owner's "Could not read: {last_error}" |
//!
//! The owner builds the Stale and Failed tooltips with `validity::tooltip_text`, because only it
//! knows the age and the last error. A value wider than its space ends with "…" (paths: middle
//! ellipsis) and its tooltip holds the full text.
//!
//! Values are selectable text. A click on a value with `copy_on_click` copies it and flashes the
//! row. The context menu on a value has "Copy value", "Copy row" and "Copy section". Ctrl+C with a
//! text selection copies the selection; without one the key goes on to the page's Copy action.
//! For that the app calls [`init`] once, which binds Ctrl+C inside the grid.
//!
//! The grid caches nothing. Owners keep the formatted strings and re-format only changed values.
//! Values are never logged.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::Copy;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Action, Animation, AnimationExt as _, AnyElement, App, BorrowAppContext as _, ClipboardItem,
    Context, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled, Task, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use tl_model::validity::Validity;

use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::icons::TlIcon;
use super::link::TlLink;
use super::motion::{M_FAST, ease_out_cubic};
use super::skeleton::TlSkeletonBlock;
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::ellipsis::middle_ellipsis;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::{label_style, numeric_style, t_base, t_md, t_sm};
use crate::text::untrusted::debug_assert_clean;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Key context of the grid, where [`init`] binds Ctrl+C.
pub const KEY_CONTEXT: &str = "TlKeyValueGrid";
/// Shown when a filter leaves no rows.
pub const EMPTY_TEXT: &str = "No properties match the current filter.";
/// Tooltip and accessible name of the section copy button.
pub const COPY_SECTION_TOOLTIP: &str = "Copy section";

const WARMING_WORD: &str = "Collecting…";
const RESTRICTED_WORD: &str = "Restricted";
const FAILED_WORD: &str = "Unavailable";

/// Default row height, × ui_scale.
const ROW_HEIGHT: f32 = 28.;
/// Default inline label column width, × ui_scale.
const LABEL_WIDTH: f32 = 180.;
/// Minimum stacked column width, × ui_scale.
const MIN_COLUMN_WIDTH: f32 = 160.;
/// Stacked columns.
const COLUMNS: usize = 3;
/// Width of the accent bar at the left edge of a section band, × ui_scale.
const BAR_WIDTH: f32 = 3.;
/// Alpha of the `accent` row flash after a click copy.
const FLASH_ALPHA: f32 = 0.25;
/// Context menu widths, × ui_scale (TlMenu defaults).
const MENU_MIN_WIDTH: f32 = 180.;
const MENU_MAX_WIDTH: f32 = 360.;
/// Indent of each copied `Label: value` line (PERF-011).
const COPY_INDENT: &str = "    ";

type CopiedHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;
type SectionHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Binds Ctrl+C inside the grid, so a text selection is copied before the page's Copy binding
/// runs. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("ctrl-c", Copy, Some(KEY_CONTEXT))]);
}

/// Why an Unsupported value has no value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnsupportedWord {
    /// The hardware or driver does not provide it.
    #[default]
    NotSupported,
    /// The source answers with no value.
    NotReported,
    /// It needs setup first. Drawn as a link to the owner's Additional Setup action.
    NeedsSetup,
}

impl UnsupportedWord {
    pub fn text(self) -> &'static str {
        match self {
            Self::NotSupported => "Not supported",
            Self::NotReported => "Not reported",
            Self::NeedsSetup => "Needs setup",
        }
    }
}

/// One value. `text` is the owner's formatted value, shown when Valid or Stale.
pub struct TlValue {
    pub text: SharedString,
    pub validity: Validity,
    /// The Unsupported word. `None` reads as "Not supported".
    pub word: Option<UnsupportedWord>,
    pub tooltip: Option<SharedString>,
    /// Draws a Valid value, or "Needs setup", as a link that dispatches this action.
    pub link: Option<Box<dyn Action>>,
    /// A file path: cut in the middle instead of at the end.
    pub path: bool,
}

impl TlValue {
    /// A Valid value with no tooltip, link or path.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            validity: Validity::Valid,
            word: None,
            tooltip: None,
            link: None,
            path: false,
        }
    }
}

impl Clone for TlValue {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            validity: self.validity,
            word: self.word,
            tooltip: self.tooltip.clone(),
            link: self.link.as_ref().map(|action| action.boxed_clone()),
            path: self.path,
        }
    }
}

#[derive(Clone)]
pub struct TlKvRow {
    pub label: SharedString,
    pub value: TlValue,
}

/// Rows under an optional sub-heading, such as "Module 1".
#[derive(Clone)]
pub struct TlKvBlock {
    pub heading: Option<SharedString>,
    pub rows: Vec<TlKvRow>,
}

/// One card. A title draws the section band.
#[derive(Clone)]
pub struct TlKvSection {
    pub title: Option<SharedString>,
    pub blocks: Vec<TlKvBlock>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KvLayout {
    /// Label over value, three columns.
    #[default]
    Stacked,
    /// Label then value on one row.
    Inline,
}

/// What a value draws, or `None` for a Missing value, whose row is not drawn.
pub fn shown_text(value: &TlValue) -> Option<SharedString> {
    let text = match value.validity {
        Validity::Valid | Validity::Stale => value.text.clone(),
        Validity::WarmingUp => WARMING_WORD.into(),
        Validity::Restricted => RESTRICTED_WORD.into(),
        Validity::Unsupported => value.word.unwrap_or_default().text().into(),
        Validity::Missing => return None,
        Validity::Failed => FAILED_WORD.into(),
    };
    Some(text)
}

/// Text, colour and link flag of a value.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueLook {
    pub text: SharedString,
    pub color: Hsla,
    /// Drawn as a `TlLink`.
    pub link: bool,
}

/// How `value` draws in `theme`, or `None` when its row is not drawn.
pub fn value_look(value: &TlValue, theme: &TlTheme) -> Option<ValueLook> {
    let text = shown_text(value)?;
    let valid = value.validity.is_valid();
    let link = value.link.is_some()
        && (valid
            || (value.validity == Validity::Unsupported
                && value.word == Some(UnsupportedWord::NeedsSetup)));
    let color = if valid {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    Some(ValueLook { text, color, link })
}

/// "Label: value" for a row. A trailing ":" in the label is not doubled.
pub fn row_copy_text(row: &TlKvRow) -> Option<String> {
    let value = shown_text(&row.value)?;
    let label = row.label.trim_end();
    let label = label.strip_suffix(':').unwrap_or(label);
    Some(format!("{label}: {value}"))
}

/// The section as plain text: the title, then each heading and `    Label: value` line
/// (PERF-011). Rows under a heading get one more indent. Missing rows are left out.
pub fn section_copy_text(section: &TlKvSection) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(title) = &section.title {
        lines.push(title.to_string());
    }
    for block in &section.blocks {
        let indent = if let Some(heading) = &block.heading {
            lines.push(format!("{COPY_INDENT}{heading}"));
            COPY_INDENT.repeat(2)
        } else {
            COPY_INDENT.to_owned()
        };
        lines.extend(
            block
                .rows
                .iter()
                .filter_map(row_copy_text)
                .map(|row| format!("{indent}{row}")),
        );
    }
    lines.join("\n")
}

/// Stacked column width: `floor((w - 2 × gap) / 3)`, at least `min`.
fn column_width(width: Pixels, gap: Pixels, min: Pixels) -> Pixels {
    let gaps = gap * (COLUMNS - 1) as f32;
    px((f32::from(width - gaps) / COLUMNS as f32).floor()).max(min)
}

fn copy_and_report(
    text: SharedString,
    on_copied: Option<&CopiedHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    if let Some(on_copied) = on_copied {
        on_copied(text, window, cx);
    }
}

/// State kept across frames for one grid.
struct GridState {
    focus: FocusHandle,
    /// The grid width from the last paint, zero before the first.
    width: Rc<Cell<Pixels>>,
    /// The flat row index that is flashing.
    flash: Option<usize>,
    /// Bumps on each flash, so a new flash restarts its animation.
    flash_count: u64,
    flash_timer: Option<Task<()>>,
}

impl GridState {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            width: Rc::default(),
            flash: None,
            flash_count: 0,
            flash_timer: None,
        }
    }

    fn start_flash(&mut self, row: usize, cx: &mut Context<Self>) {
        self.flash = Some(row);
        self.flash_count += 1;
        self.flash_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(M_FAST).await;
            let _ = this.update(cx, |s, cx| {
                s.flash = None;
                s.flash_timer = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// Values every row needs while rendering.
struct Frame {
    m: UiMetrics,
    theme: Arc<TlTheme>,
    state: gpui_kit::Entity<GridState>,
    flash: Option<usize>,
    flash_count: u64,
    reduced_motion: bool,
    row_height: Pixels,
}

/// The key/value grid. Build it with [`TlKeyValueGrid::new`] and [`TlKeyValueGrid::sections`].
#[derive(IntoElement)]
pub struct TlKeyValueGrid {
    id: ElementId,
    layout: KvLayout,
    sections: Rc<[TlKvSection]>,
    row_height: Option<Pixels>,
    label_width: Option<Pixels>,
    block_indent: Option<Pixels>,
    block_gap: Option<Pixels>,
    copy_on_click: bool,
    on_copied: Option<CopiedHandler>,
    on_copy_section: Option<SectionHandler>,
    filtered_empty: bool,
    loading: bool,
}

impl TlKeyValueGrid {
    pub fn new(id: impl Into<ElementId>, layout: KvLayout) -> Self {
        Self {
            id: id.into(),
            layout,
            sections: Rc::from(Vec::new()),
            row_height: None,
            label_width: None,
            block_indent: None,
            block_gap: None,
            copy_on_click: false,
            on_copied: None,
            on_copy_section: None,
            filtered_empty: false,
            loading: false,
        }
    }

    pub fn sections(mut self, sections: &[TlKvSection]) -> Self {
        self.sections = sections.into();
        self
    }

    /// Row height, already scaled by the owner. Default 28 × ui_scale. A stacked pair is two rows.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    /// Inline label column width, already scaled by the owner. Default 180 × ui_scale.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// Indent of rows under a block heading, already scaled. Default `sp6`.
    pub fn block_indent(mut self, indent: Pixels) -> Self {
        self.block_indent = Some(indent);
        self
    }

    /// Space between blocks, already scaled. Default `sp4`.
    pub fn block_gap(mut self, gap: Pixels) -> Self {
        self.block_gap = Some(gap);
        self
    }

    /// A click on a value copies it, calls `on_copied` and flashes the row.
    pub fn copy_on_click(mut self, copy: bool) -> Self {
        self.copy_on_click = copy;
        self
    }

    /// Called with the copied text after every copy the grid makes. The owner shows the toast.
    pub fn on_copied(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copied = Some(Rc::new(handler));
        self
    }

    /// Called with the section index when its band's copy button is clicked. Without it the
    /// button copies [`section_copy_text`] and calls `on_copied`.
    pub fn on_copy_section(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy_section = Some(Rc::new(handler));
        self
    }

    /// Draws [`EMPTY_TEXT`] instead of the rows.
    pub fn filtered_empty(mut self, empty: bool) -> Self {
        self.filtered_empty = empty;
        self
    }

    /// Draws a `TlSkeletonBlock` instead of the rows.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    fn child_id(&self, name: &str, n: usize) -> ElementId {
        ElementId::NamedChild(Arc::new(self.id.clone()), format!("{name}{n}").into())
    }

    fn band(&self, si: usize, title: &SharedString, f: &Frame) -> AnyElement {
        let m = &f.m;
        let mut text = div().flex_1().min_w_0();
        *text.text_style() = t_md(m);
        let text = text
            .truncate()
            .text_color(f.theme.text_primary)
            .child(title.clone());

        let on_copy_section = self.on_copy_section.clone();
        let on_copied = self.on_copied.clone();
        let sections = self.sections.clone();
        let button = TlButton::new(self.child_id("copy-section", si), TlButtonKind::Icon)
            .icon(TlIcon::Copy)
            .size(TlButtonSize::Sm)
            .tooltip(COPY_SECTION_TOOLTIP)
            .on_click(move |_, window, cx| match &on_copy_section {
                Some(handler) => handler(si, window, cx),
                None => copy_and_report(
                    section_copy_text(&sections[si]).into(),
                    on_copied.as_ref(),
                    window,
                    cx,
                ),
            });

        h_flex()
            .debug_selector(move || format!("TlKv-band-{si}"))
            .relative()
            .h(f.row_height)
            .px(m.sp[4])
            .gap(m.sp[4])
            .bg(f.theme.header)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(BAR_WIDTH * m.ui_scale))
                    .bg(f.theme.accent),
            )
            .child(text)
            .child(button)
            .into_any_element()
    }

    /// The value element of row `n` in section `si`, or `None` when the row is Missing.
    #[allow(clippy::too_many_arguments)]
    fn value(
        &self,
        n: usize,
        si: usize,
        row: &TlKvRow,
        available: Option<Pixels>,
        f: &Frame,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let look = value_look(&row.value, &f.theme)?;
        let m = &f.m;
        let id = self.child_id("value", n);
        let mut cell = div()
            .id(id.clone())
            .debug_selector(move || format!("TlKv-value-{n}"))
            .flex()
            .items_center()
            .min_w_0()
            .max_w_full()
            .text_color(look.color);

        let mut full_text = None;
        if look.link {
            let action = row.value.link.as_ref()?.boxed_clone();
            cell = cell.child(
                TlLink::new((id.clone(), "link"), look.text.clone(), action).text_size(m.text.base),
            );
        } else {
            debug_assert_clean(&look.text);
            let mut shown = look.text.clone();
            if let Some(available) = available {
                let (width, cut) = measure(&look.text, row.value.path, available, m, window, cx);
                if width > available {
                    full_text = Some(look.text.clone());
                    if let Some(cut) = cut {
                        shown = cut;
                    }
                }
            }
            let mut text = div().min_w_0();
            *text.text_style() = numeric_style(m, m.text.base);
            cell = cell.child(
                text.truncate().child(
                    SelectableText::new((id.clone(), "text"), shown)
                        .text_style(numeric_style(m, m.text.base)),
                ),
            );
            if self.copy_on_click {
                let on_copied = self.on_copied.clone();
                let state = f.state.clone();
                let text = look.text.clone();
                cell = cell.on_click(move |_, window, cx| {
                    // A drag that selected text is not a copy click.
                    if TextSelection::has_selection(window, cx) {
                        return;
                    }
                    copy_and_report(text.clone(), on_copied.as_ref(), window, cx);
                    state.update(cx, |s, cx| s.start_flash(n, cx));
                });
            }
        }

        let tooltip = match (full_text, row.value.tooltip.clone()) {
            (Some(full), Some(tip)) => Some(TlTooltip::lines(vec![full, tip])),
            (Some(text), None) | (None, Some(text)) => Some(TlTooltip::text(text)),
            (None, None) => None,
        };
        let cell = match tooltip {
            Some(tip) => {
                let state = HoverTooltip::state((id, "tooltip"), window, cx);
                HoverTooltip::attach(&state, cell, DELAY_CONTROL, tip.build(), window, cx)
            }
            None => cell,
        };

        let on_copied = self.on_copied.clone();
        let sections = self.sections.clone();
        let value_text = look.text;
        let row_text: SharedString = row_copy_text(row).unwrap_or_default().into();
        let menu_min = px(MENU_MIN_WIDTH * m.ui_scale);
        let menu_max = px(MENU_MAX_WIDTH * m.ui_scale);
        Some(
            cell.context_menu(move |menu: PopupMenu, _, _| {
                let item = |label: &'static str, text: Rc<dyn Fn() -> SharedString>| {
                    let on_copied = on_copied.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        copy_and_report(text(), on_copied.as_ref(), window, cx)
                    })
                };
                let value_text = value_text.clone();
                let row_text = row_text.clone();
                let sections = sections.clone();
                menu.min_w(menu_min)
                    .max_w(menu_max)
                    .item(item("Copy value", Rc::new(move || value_text.clone())))
                    .item(item("Copy row", Rc::new(move || row_text.clone())))
                    .item(item(
                        "Copy section",
                        Rc::new(move || section_copy_text(&sections[si]).into()),
                    ))
            })
            .into_any_element(),
        )
    }

    /// The `accent` flash over row `n` while it is flashing.
    fn flash(&self, n: usize, f: &Frame) -> Option<AnyElement> {
        if f.flash != Some(n) {
            return None;
        }
        let color = f.theme.accent;
        let overlay = div()
            .debug_selector(move || format!("TlKv-flash-{n}"))
            .absolute()
            .inset_0()
            .bg(color.alpha(FLASH_ALPHA));
        if f.reduced_motion {
            return Some(overlay.into_any_element());
        }
        Some(
            overlay
                .with_animation(
                    self.child_id("flash", f.flash_count as usize),
                    Animation::new(M_FAST).with_easing(ease_out_cubic),
                    move |el, t| el.bg(color.alpha(FLASH_ALPHA * (1. - t))),
                )
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn inline_row(
        &self,
        n: usize,
        si: usize,
        row: &TlKvRow,
        label_width: Pixels,
        available: Option<Pixels>,
        f: &Frame,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let value = self.value(n, si, row, available, f, window, cx)?;
        let mut label = div().flex_none().w(label_width).min_w_0();
        *label.text_style() = t_base(&f.m);
        let label = label
            .truncate()
            .text_color(f.theme.text_secondary)
            .child(row.label.clone());
        Some(
            div()
                .debug_selector(move || format!("TlKv-row-{n}"))
                .relative()
                .flex()
                .items_center()
                .h(f.row_height)
                .child(label)
                .child(div().flex().flex_1().min_w_0().child(value))
                .children(self.flash(n, f))
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn stacked_pair(
        &self,
        n: usize,
        si: usize,
        row: &TlKvRow,
        min_width: Pixels,
        available: Option<Pixels>,
        f: &Frame,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let value = self.value(n, si, row, available, f, window, cx)?;
        let mut label = div().min_w_0();
        *label.text_style() = t_sm(&f.m);
        let label = label
            .truncate()
            .text_color(f.theme.text_secondary)
            .child(row.label.clone());
        Some(
            div()
                .debug_selector(move || format!("TlKv-row-{n}"))
                .relative()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .flex_basis(px(0.))
                .min_w(min_width)
                .h(f.row_height * 2.)
                .child(label)
                .child(value)
                .children(self.flash(n, f))
                .into_any_element(),
        )
    }

    fn rows(
        &self,
        si: usize,
        block: &TlKvBlock,
        first: usize,
        width: Option<Pixels>,
        f: &Frame,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let m = &f.m;
        match self.layout {
            KvLayout::Inline => {
                let label_width = self.label_width.unwrap_or(px(LABEL_WIDTH * m.ui_scale));
                let available = width.map(|w| (w - label_width).max(px(0.)));
                v_flex()
                    .children(block.rows.iter().enumerate().filter_map(|(i, row)| {
                        self.inline_row(first + i, si, row, label_width, available, f, window, cx)
                    }))
                    .into_any_element()
            }
            KvLayout::Stacked => {
                let gap = m.sp[7];
                let min_width = px(MIN_COLUMN_WIDTH * m.ui_scale);
                let available = width.map(|w| column_width(w, gap, min_width));
                let pairs: Vec<AnyElement> = block
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        self.stacked_pair(first + i, si, row, min_width, available, f, window, cx)
                    })
                    .collect();
                let mut lines = Vec::with_capacity(pairs.len().div_ceil(COLUMNS));
                let mut pairs = pairs.into_iter().peekable();
                while pairs.peek().is_some() {
                    let mut line = h_flex().w_full().gap(gap);
                    for _ in 0..COLUMNS {
                        line = match pairs.next() {
                            Some(pair) => line.child(pair),
                            // Empty cells keep the last line's columns equal.
                            None => line.child(div().flex_1().flex_basis(px(0.)).min_w(min_width)),
                        };
                    }
                    lines.push(line);
                }
                v_flex().children(lines).into_any_element()
            }
        }
    }
}

/// Width of `text` in the value font and, for a path that does not fit, the middle-cut text.
fn measure(
    text: &SharedString,
    path: bool,
    available: Pixels,
    m: &UiMetrics,
    window: &Window,
    cx: &mut App,
) -> (Pixels, Option<SharedString>) {
    if !cx.has_global::<TextMetricsCache>() {
        cx.set_global(TextMetricsCache::default());
    }
    let size = m.text.base;
    let version = m.style_version;
    cx.update_global::<TextMetricsCache, _>(|cache, cx| {
        cache.clear_if_stale(version);
        let mut width_of = |s: &str| {
            s.chars().fold(px(0.), |sum, ch| {
                sum + cache.char_advance(ch, size, window, cx)
            })
        };
        let width = width_of(text);
        let cut =
            (path && width > available).then(|| middle_ellipsis(text, available, &mut width_of));
        (width, cut)
    })
}

impl RenderOnce for TlKeyValueGrid {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let root = div()
            .id(self.id.clone())
            .debug_selector(|| "TlKeyValueGrid".into())
            .w_full();
        if self.loading {
            return root.child(TlSkeletonBlock::new()).into_any_element();
        }
        if self.filtered_empty {
            let mut text = div().debug_selector(|| "TlKv-empty".into());
            *text.text_style() = t_base(&m);
            return root
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(text.text_color(theme.text_secondary).child(EMPTY_TEXT))
                .into_any_element();
        }

        let state = window.use_keyed_state((self.id.clone(), "kv-state"), cx, |_, cx| {
            GridState::new(cx)
        });
        let (focus, width_cell, flash, flash_count) = {
            let s = state.read(cx);
            (s.focus.clone(), s.width.clone(), s.flash, s.flash_count)
        };
        let width = Some(width_cell.get()).filter(|w| *w > px(0.));
        let f = Frame {
            m,
            theme,
            state,
            flash,
            flash_count,
            reduced_motion: cx.reduce_motion(),
            row_height: self.row_height.unwrap_or(px(ROW_HEIGHT * m.ui_scale)),
        };
        let indent = self.block_indent.unwrap_or(m.sp[6]);
        let block_gap = self.block_gap.unwrap_or(m.sp[4]);

        let mut n = 0;
        let mut sections = Vec::with_capacity(self.sections.len());
        for (si, section) in self.sections.iter().enumerate() {
            let mut blocks = v_flex();
            for (bi, block) in section.blocks.iter().enumerate() {
                let indented = block.heading.is_some();
                let rows_width = width.map(|w| if indented { w - indent } else { w });
                let rows = self.rows(si, block, n, rows_width, &f, window, cx);
                n += block.rows.len();
                let heading = block.heading.clone().map(|heading| {
                    let mut text = div().flex().items_center().h(f.row_height);
                    *text.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
                    text.truncate()
                        .text_color(f.theme.text_primary)
                        .child(heading)
                });
                blocks = blocks.child(
                    v_flex()
                        .when(bi > 0, |b| b.mt(block_gap))
                        .children(heading)
                        .child(div().when(indented, |d| d.pl(indent)).child(rows)),
                );
            }
            let band = section.title.as_ref().map(|t| self.band(si, t, &f));
            let has_band = band.is_some();
            sections.push(
                v_flex()
                    .children(band)
                    .child(div().when(has_band, |d| d.mt(m.sp[4])).child(blocks)),
            );
        }

        root.key_context(KEY_CONTEXT)
            .track_focus(&focus)
            .relative()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
            })
            .on_action(|_: &Copy, window, cx| {
                let text = TextSelection::selected_text(window, cx);
                let text = text.trim();
                if text.is_empty() {
                    cx.propagate();
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            })
            .child(
                canvas(
                    move |bounds, window, _| {
                        if width_cell.get() != bounds.size.width {
                            width_cell.set(bounds.size.width);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .children(sections)
            .into_any_element()
    }
}
