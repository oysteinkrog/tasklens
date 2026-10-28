//! The one fit rule of every Overview box body (UI-PRF-017, UI-PRF-059, tasklens-crw.100).
//!
//! [`whole_rows`] is a column that draws a row only when the row fits whole, top first. A row
//! that would end below the column, or below the clip of a box part, is left out with every row
//! after it, so nothing draws in the time-axis band. [`fit_text`] is a value that shortens itself
//! by dropping decimals until it fits its width, and draws nothing when even the form without
//! decimals does not fit, so a drawn value never loses its unit.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ParentElement, Pixels, Refineable as _, ShapedLine, SharedString, Style,
    StyleRefinement, Styled, TextAlign, Window, point, px, size,
};
use smallvec::SmallVec;

/// The slack a fit check allows for sub-pixel layout, in px.
const EPS: f32 = 0.01;

/// A flex column of rows that draws only the rows that fit whole, top first. Style it like a
/// `div` (it starts as a flex column); the rows are its children.
pub struct WholeRows {
    style: StyleRefinement,
    children: SmallVec<[AnyElement; 8]>,
}

/// An empty [`WholeRows`] column.
pub fn whole_rows() -> WholeRows {
    WholeRows {
        style: StyleRefinement::default(),
        children: SmallVec::new(),
    }
    .flex()
    .flex_col()
}

impl Styled for WholeRows {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for WholeRows {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl IntoElement for WholeRows {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for WholeRows {
    type RequestLayoutState = SmallVec<[LayoutId; 8]>;
    /// How many rows, from the top, fit whole.
    type PrepaintState = usize;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let children = &mut self.children;
        window.with_text_style(style.text_style().cloned(), |window| {
            let ids: SmallVec<[LayoutId; 8]> = children
                .iter_mut()
                .map(|child| child.request_layout(window, cx))
                .collect();
            (window.request_layout(style, ids.iter().copied(), cx), ids)
        })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        ids: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> usize {
        // A row ends inside the column and inside the clip of the box part around it.
        let limit = bounds.bottom().min(window.content_mask().bounds.bottom()) + px(EPS);
        let shown = ids
            .iter()
            .take_while(|id| window.layout_bounds(**id).bottom() <= limit)
            .count();
        let mut style = Style::default();
        style.refine(&self.style);
        window.with_text_style(style.text_style().cloned(), |window| {
            for child in &mut self.children[..shown] {
                child.prepaint(window, cx);
            }
        });
        shown
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        shown: &mut usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut style = Style::default();
        style.refine(&self.style);
        window.with_text_style(style.text_style().cloned(), |window| {
            for child in &mut self.children[..*shown] {
                child.paint(window, cx);
            }
        });
    }
}

/// `text` with every number that has more than `decimals` digits after its point rounded to
/// `decimals` digits ("15.92 GiB" at 1 is "15.9 GiB", at 0 "16 GiB"). Only a `.` between two
/// digits is a decimal point; a `,` between digits groups thousands.
pub fn round_decimals(text: &str, decimals: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let is_point = c == '.'
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if !is_point {
            out.push(c);
            i += 1;
            continue;
        }
        let end = (i + 1..chars.len())
            .find(|&j| !chars[j].is_ascii_digit())
            .unwrap_or(chars.len());
        let frac = &chars[i + 1..end];
        if frac.len() <= decimals {
            out.extend_from_slice(&chars[i..end]);
            i = end;
            continue;
        }
        let round_up = frac[decimals] >= '5';
        // The integer part already in `out`: digits and grouping commas back to its start.
        let int_start = (0..out.len())
            .rev()
            .take_while(|&j| out[j].is_ascii_digit() || (out[j] == ',' && j > 0))
            .last()
            .unwrap_or(out.len());
        let mut kept: Vec<char> = frac[..decimals].to_vec();
        let mut carry = round_up;
        for d in kept.iter_mut().rev() {
            if !carry {
                break;
            }
            if *d == '9' {
                *d = '0';
            } else {
                *d = char::from(*d as u8 + 1);
                carry = false;
            }
        }
        if carry {
            for j in (int_start..out.len()).rev() {
                match out[j] {
                    ',' => {}
                    '9' => out[j] = '0',
                    d => {
                        out[j] = char::from(d as u8 + 1);
                        carry = false;
                        break;
                    }
                }
            }
            if carry {
                out.insert(int_start, '1');
            }
        }
        if decimals > 0 {
            out.push('.');
            out.extend(kept);
        }
        i = end;
    }
    out.into_iter().collect()
}

/// The most digits after a decimal point in any number of `text`.
fn max_decimals(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut best = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '.' && i > 0 && chars[i - 1].is_ascii_digit() {
            let n = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            best = best.max(n);
        }
    }
    best
}

/// The shorter forms of `text`, one decimal fewer each, down to no decimals.
pub fn shorter_forms(text: &str) -> Vec<String> {
    (0..max_decimals(text))
        .rev()
        .map(|d| round_decimals(text, d))
        .collect()
}

/// A one-line value that takes the width of its text and shrinks in a flex row. When it gets
/// less than its text, it draws the first [`shorter_forms`] form that fits, at the end of its
/// width; when none fits it draws nothing.
pub struct FitText {
    text: SharedString,
    align: TextAlign,
}

/// A right-aligned [`FitText`] of `text`.
pub fn fit_text(text: impl Into<SharedString>) -> FitText {
    FitText {
        text: text.into(),
        align: TextAlign::Right,
    }
}

impl FitText {
    /// Draws a shortened form at the start of the width instead of its end.
    pub fn start(mut self) -> Self {
        self.align = TextAlign::Left;
        self
    }
}

/// The current text style's shaped line of `text`, and its line height.
fn shape(text: SharedString, window: &Window) -> (ShapedLine, Pixels) {
    let style = window.text_style();
    let rem = window.rem_size();
    let run = style.to_run(text.len());
    let line = window
        .text_system()
        .shape_line(text, style.font_size.to_pixels(rem), &[run], None);
    (line, style.line_height_in_pixels(rem))
}

impl IntoElement for FitText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FitText {
    /// The whole text's shaped line and the line height.
    type RequestLayoutState = (ShapedLine, Pixels);
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (line, line_h) = shape(self.text.clone(), window);
        let mut style = Style::default();
        style.size = size(line.width().into(), line_h.into());
        style.min_size.width = px(0.).into();
        style.flex_shrink = 1.;
        (window.request_layout(style, [], cx), (line, line_h))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (whole, line_h): &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let room = bounds.size.width + px(EPS);
        let line = if whole.width() <= room {
            Some(whole.clone())
        } else {
            shorter_forms(&self.text)
                .into_iter()
                .map(|form| shape(form.into(), window).0)
                .find(|line| line.width() <= room)
        };
        let Some(line) = line else {
            return;
        };
        let x = match self.align {
            TextAlign::Left => bounds.left(),
            _ => bounds.right() - line.width(),
        };
        let origin = point(x, bounds.top());
        #[cfg(any(test, feature = "test-hooks"))]
        FIT_TEXTS.with_borrow_mut(|log| {
            log.push(FitTextPaint {
                bounds: Bounds::new(origin, size(line.width(), *line_h)),
                mask: window.content_mask().bounds,
                text: line.text.clone(),
                whole: self.text.clone(),
            });
        });
        let _ = line.paint(origin, *line_h, TextAlign::Left, None, window, cx);
    }
}

/// Where [`FitText`] painted a value: the text's bounds, the clip it painted under, the text
/// drawn and the whole value.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug)]
pub struct FitTextPaint {
    pub bounds: Bounds<Pixels>,
    pub mask: Bounds<Pixels>,
    pub text: SharedString,
    pub whole: SharedString,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FIT_TEXTS: std::cell::RefCell<Vec<FitTextPaint>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The values every [`FitText`] painted on this thread since the last call, in paint order.
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_fit_texts() -> Vec<FitTextPaint> {
    FIT_TEXTS.with(|t| std::mem::take(&mut *t.borrow_mut()))
}
