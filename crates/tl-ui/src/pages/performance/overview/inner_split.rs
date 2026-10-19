//! The one inner splitter every Overview box reuses (UI-PRF-019 "Inner splitter", D-025).
//!
//! A box mode with two parts has one 8 px gap between them. It works like the gaps between
//! boxes (UI-PRF-018): a resize cursor, a 2 px `accent` line after 150 ms of hover, a drag that
//! keeps each part at its minimum, a double-click that resets the mode's default split, and an
//! [`InnerSplitChanged`] event when a drag or a double-click ends, which the page saves. Parts
//! side by side get a vertical gap ([`SplitAxis::Row`]), stacked parts a horizontal one
//! ([`SplitAxis::Column`]).
//!
//! [`InnerSplit`] is plain state inside the box view. The box draws its two parts with the
//! first taking [`InnerSplit::share`] of the body, and adds [`InnerSplit::probe`] and
//! [`InnerSplit::gap`] over the body. Both take a `fn` that finds the splitter in the box, so
//! their handlers reach it without a separate view.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, Bounds, Context, EventEmitter, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point,
    StatefulInteractiveElement as _, Styled as _, Task, canvas, div, px, relative,
};

use super::layout::SPLIT_RANGE;
use crate::theme::{current, metrics};

/// The smallest part when a mode sets no other minimum, logical px.
pub const PART_MIN: f32 = 120.;

/// How long the pointer rests on the gap before its accent line shows.
const HOVER_DELAY: Duration = Duration::from_millis(150);

/// The accent line in the gap, logical px.
const LINE_W: f32 = 2.;

/// How the two parts sit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitAxis {
    /// Side by side, with a vertical gap.
    Row,
    /// Stacked, with a horizontal gap.
    Column,
}

impl SplitAxis {
    fn along(self, p: Point<Pixels>) -> f32 {
        f32::from(match self {
            Self::Row => p.x,
            Self::Column => p.y,
        })
    }

    fn length(self, b: Bounds<Pixels>) -> f32 {
        f32::from(match self {
            Self::Row => b.size.width,
            Self::Column => b.size.height,
        })
    }
}

/// The share of the first part a drag or a double-click ended on, rounded to 3 decimals. The
/// page saves it as the box's `split`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InnerSplitChanged(pub f32);

/// `share` of a body `len` px long, clamped to [`SPLIT_RANGE`] and so that the first part
/// keeps `min_first` and the second `min_second` beside the `gap` (minimums in logical px,
/// `scale` the UI scale). The first part's floor wins when the body is too short for both.
pub fn clamp_split(
    share: f32,
    len: f32,
    gap: f32,
    (min_first, min_second): (f32, f32),
    scale: f32,
) -> f32 {
    let (lo, hi) = SPLIT_RANGE;
    if len <= 0. {
        return share.clamp(lo, hi);
    }
    let lo = lo.max(min_first * scale / len);
    let hi = hi.min((len - gap - min_second * scale) / len);
    share.clamp(lo, hi.max(lo))
}

/// A share rounded to 3 decimals, as the profile stores it.
pub fn round_split(share: f32) -> f32 {
    (share * 1000.).round() / 1000.
}

/// A drag in progress: the pointer and share when the button went down.
#[derive(Clone, Copy, PartialEq, Debug)]
struct SplitDrag {
    axis: SplitAxis,
    gap: f32,
    start: f32,
    start_share: f32,
    /// True once a move changed the share.
    changed: bool,
}

/// One inner splitter's state.
pub struct InnerSplit {
    id: &'static str,
    line_id: &'static str,
    share: f32,
    default: f32,
    mins: (f32, f32),
    /// When set, the untouched default share leaves the second part this many logical px.
    default_second_px: Option<f32>,
    /// The body's bounds at the last paint.
    body: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<SplitDrag>,
    hover: bool,
    hover_shown: bool,
    hover_task: Option<Task<()>>,
}

impl InnerSplit {
    /// A splitter at `default`, with debug selectors `id` for the gap and `line_id` for its
    /// accent line, and [`PART_MIN`] for both parts.
    pub fn new(id: &'static str, line_id: &'static str, default: f32) -> Self {
        Self {
            id,
            line_id,
            share: default,
            default,
            mins: (PART_MIN, PART_MIN),
            default_second_px: None,
            body: Rc::default(),
            drag: None,
            hover: false,
            hover_shown: false,
            hover_task: None,
        }
    }

    /// The parts' minimums in logical px, when the mode sets its own.
    pub fn with_mins(mut self, first: f32, second: f32) -> Self {
        self.mins = (first, second);
        self
    }

    /// Makes the default share a fixed second part: while the share is the default, the second
    /// part is `px` logical px wide (or tall) whatever the body size, within the clamps.
    pub fn with_default_second_px(mut self, px: f32) -> Self {
        self.default_second_px = Some(px);
        self
    }

    /// The share as saved or dragged, before the clamps.
    pub fn saved(&self) -> f32 {
        self.share
    }

    pub fn default_share(&self) -> f32 {
        self.default
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The first part's share of the body as laid out along `axis`, after the clamps. `gap` is
    /// the gap in px.
    pub fn share(&self, axis: SplitAxis, gap: Pixels, scale: f32) -> f32 {
        self.sharer(gap, scale)(axis.length(self.body.get()))
    }

    /// [`Self::share`] as a function of the body length in px, for a probe that must classify
    /// the layout from the state of this render.
    pub fn sharer(&self, gap: Pixels, scale: f32) -> impl Fn(f32) -> f32 + 'static + use<> {
        let (share, default, mins) = (self.share, self.default, self.mins);
        let second_px = self.default_second_px;
        let gap = f32::from(gap);
        move |len| {
            match second_px {
                // The fixed default is exempt from `SPLIT_RANGE`: a wide box keeps its column.
                Some(second) if len > 0. && share == default => {
                    let lo = mins.0 * scale / len;
                    let hi = (len - gap - mins.1 * scale) / len;
                    ((len - gap - second * scale) / len).clamp(lo, hi.max(lo))
                }
                _ => clamp_split(share, len, gap, mins, scale),
            }
        }
    }

    /// Takes the saved share. Nothing changes while the gap is held. Returns whether it changed.
    pub fn set_share(&mut self, share: f32) -> bool {
        if self.drag.is_some() || share == self.share {
            return false;
        }
        self.share = share;
        true
    }

    /// Button down on the gap: a double-click resets the default share, a single press starts
    /// a drag.
    fn down<V: EventEmitter<InnerSplitChanged>>(
        &mut self,
        axis: SplitAxis,
        gap: Pixels,
        ev: &MouseDownEvent,
        cx: &mut Context<V>,
    ) {
        cx.stop_propagation();
        if ev.click_count >= 2 {
            self.drag = None;
            if self.share != self.default {
                self.share = self.default;
                cx.emit(InnerSplitChanged(self.share));
            }
        } else {
            self.drag = Some(SplitDrag {
                axis,
                gap: f32::from(gap),
                start: axis.along(ev.position),
                start_share: self.share(axis, gap, metrics(cx).ui_scale),
                changed: false,
            });
        }
        cx.notify();
    }

    /// The pointer moved while the gap is held.
    fn moved<V: 'static>(&mut self, position: Point<Pixels>, cx: &mut Context<V>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let len = drag.axis.length(self.body.get());
        if len <= 0. {
            return;
        }
        let share = drag.start_share + (drag.axis.along(position) - drag.start) / len;
        let share = clamp_split(share, len, drag.gap, self.mins, metrics(cx).ui_scale);
        if share != self.share {
            drag.changed = true;
            self.share = share;
            cx.notify();
        }
    }

    /// The button went up: the drag ends and a changed share goes to the page to be saved.
    fn up<V: EventEmitter<InnerSplitChanged>>(&mut self, cx: &mut Context<V>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.changed {
            self.share = round_split(self.share);
            cx.emit(InnerSplitChanged(self.share));
        }
        cx.notify();
    }

    /// The pointer entered or left the gap. The accent line shows after [`HOVER_DELAY`] and
    /// goes at once on leaving.
    fn hovered<V: 'static>(
        &mut self,
        hovered: bool,
        this: fn(&mut V) -> &mut Self,
        cx: &mut Context<V>,
    ) {
        if hovered == self.hover {
            return;
        }
        self.hover = hovered;
        self.hover_shown = false;
        self.hover_task = hovered.then(|| {
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(HOVER_DELAY).await;
                let _ = view.update(cx, |v, cx| {
                    let split = this(v);
                    split.hover_task = None;
                    if split.hover {
                        split.hover_shown = true;
                        cx.notify();
                    }
                });
            })
        });
        cx.notify();
    }

    /// The gap's hit area with its cursor, handlers and accent line, absolutely placed in the
    /// body at the first part's `share` along `axis`. `gap` is the gap in px.
    pub fn gap<V: EventEmitter<InnerSplitChanged>>(
        &self,
        axis: SplitAxis,
        share: f32,
        gap: Pixels,
        this: fn(&mut V) -> &mut Self,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let (id, line_id) = (self.id, self.line_id);
        let el = div()
            .id(id)
            .debug_selector(move || id.into())
            .absolute()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, ev: &MouseDownEvent, _, cx| {
                    this(v).down(axis, gap, ev, cx);
                }),
            )
            .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                this(v).hovered(*hovered, this, cx);
            }));
        let el = match axis {
            SplitAxis::Row => el
                .top_0()
                .bottom_0()
                .left(relative(share))
                .w(gap)
                .cursor_ew_resize(),
            SplitAxis::Column => el
                .left_0()
                .right_0()
                .top(relative(share))
                .h(gap)
                .cursor_ns_resize(),
        };
        if !(self.drag.is_some() || (self.hover && self.hover_shown)) {
            return el.into_any_element();
        }
        let offset = px((f32::from(gap) - LINE_W) / 2.);
        let line = div()
            .debug_selector(move || line_id.into())
            .absolute()
            .bg(current(cx).accent);
        let line = match axis {
            SplitAxis::Row => line.top_0().bottom_0().left(offset).w(px(LINE_W)),
            SplitAxis::Column => line.left_0().right_0().top(offset).h(px(LINE_W)),
        };
        el.child(line).into_any_element()
    }

    /// A canvas over the body that keeps its bounds and, while the gap is held, follows the
    /// pointer wherever it goes.
    pub fn probe<V: EventEmitter<InnerSplitChanged>>(
        &self,
        this: fn(&mut V) -> &mut Self,
        cx: &mut Context<V>,
    ) -> impl IntoElement + use<V> {
        let body = Rc::clone(&self.body);
        let view = cx.entity().downgrade();
        let dragging = self.drag.is_some();
        // A fixed second part depends on the body length, so a new length needs one more render.
        let fixed = self.default_second_px.is_some() && self.share == self.default;
        let id = cx.entity_id();
        canvas(
            move |bounds, _, cx| {
                let old = body.replace(bounds);
                if fixed && old.size != bounds.size {
                    cx.defer(move |cx| cx.notify(id));
                }
            },
            move |_, _, window, _| {
                if !dragging {
                    return;
                }
                let moved = view.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let _ = moved.update(cx, |v, cx| {
                        if e.pressed_button == Some(MouseButton::Left) {
                            this(v).moved(e.position, cx);
                        } else {
                            this(v).up(cx);
                        }
                    });
                });
                let released = view.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() {
                        let _ = released.update(cx, |v, cx| this(v).up(cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINS: (f32, f32) = (PART_MIN, 188.);

    #[test]
    fn clamps_to_range_and_part_minimums() {
        // A 1000 px body: 20%..=80%, and 1000 - 8 - 188 = 804 leaves 80.4%.
        assert_eq!(clamp_split(0.1, 1000., 8., MINS, 1.), 0.2);
        assert_eq!(clamp_split(0.9, 1000., 8., MINS, 1.), 0.8);
        assert_eq!(clamp_split(0.5, 1000., 8., MINS, 1.), 0.5);
        // A 600 px body keeps 188 px for the second part: (600 - 8 - 188) / 600, and 120 px
        // for the first: 120 / 600 = 20%.
        let hi = clamp_split(0.8, 600., 8., MINS, 1.);
        assert!((hi - 404. / 600.).abs() < 1e-6, "{hi}");
        assert_eq!(clamp_split(0.1, 400., 8., MINS, 1.), 0.3);
        // A body too short for both keeps the first part's floor.
        assert_eq!(clamp_split(0.5, 200., 8., MINS, 1.), 0.6);
        // Scale widens the minimums.
        assert!(clamp_split(0.8, 600., 8., MINS, 2.) < hi);
        // No body yet: the range only.
        assert_eq!(clamp_split(0.95, 0., 8., MINS, 1.), 0.8);
    }

    #[test]
    fn rounds_to_three_decimals() {
        assert_eq!(round_split(0.123_456), 0.123);
        assert_eq!(round_split(0.4), 0.4);
    }
}
