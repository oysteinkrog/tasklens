//! The memory composition bar under the Memory charts: In use, In use (compressed), Modified,
//! Standby and Free as proportional segments with tooltips (docs/ui/performance.md UI-PRF-027;
//! PERF-042; UI-FND-019).
//!
//! The bar is its own cached view, so a memory sample repaints the bar and nothing else, and a
//! sample that leaves the composition as it was repaints nothing. A paint is five quads, the
//! outline, the labels that fit (cached shaped lines) and one hatch path, which is built again
//! only when the compressed segment's physical edges move. Hitboxes exist only while the pointer
//! is inside the bar, one per segment.

use std::fmt::Write as _;

use gpui_kit::component::v_flex;
use gpui_kit::{
    App, AppContext as _, BorderStyle, Bounds, ContentMask, Context, Corners, Element, ElementId,
    Entity, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseMoveEvent, ParentElement as _, Path, PathBuilder, Pixels, Point,
    Render, SharedString, Style, Styled as _, Subscription, Window, div, fill, outline, point, px,
    relative, size,
};
use smallvec::SmallVec;
use tl_model::system::MemoryTotals;
use tl_model::units;
use tl_model::validity::Validity;

use super::details::Template;
use crate::chart::labels::{self, CachedLine, LineStyle};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Bar height, × ui_scale.
pub const BAR_H: f32 = 24.;
/// A non-zero segment is at least this wide, logical px.
pub const MIN_SEGMENT_W: f32 = 2.;
/// A segment shows its name only from this width, × ui_scale.
pub const LABEL_MIN_W: f32 = 72.;
/// Hatch line spacing, × ui_scale.
const HATCH_STEP: f32 = 4.;
/// Alpha of the hatch lines.
const HATCH_ALPHA: f32 = 0.5;
/// Added to a segment's alpha while it is hovered.
const HOVER_ALPHA: f32 = 0.15;

/// Debug selector of the bar box.
pub const BAR_ID: &str = "perf-mem-composition";
/// Debug selector of the hovered segment's tooltip trigger.
pub const BAR_TRIGGER_ID: &str = "perf-mem-composition-trigger";

const KEY_CAPTION: &str = "perf-mem-composition-caption";
const KEY_COLLECTING: &str = "perf-fallback-collecting";

/// The five memory parts in bytes, and their total (PERF-042).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    pub in_use: u64,
    pub compressed: u64,
    pub modified: u64,
    pub standby: u64,
    pub free: u64,
    pub total: u64,
}

impl Composition {
    /// The composition from the memory totals; `None` unless the meminfo read is Valid or
    /// Stale with a total. `compressed` is zram `mem_used_total` plus Zswap, clamped to the used
    /// memory; `standby` is the rest, at least 0.
    pub fn of(m: &MemoryTotals) -> Option<Self> {
        if !matches!(m.validity, Validity::Valid | Validity::Stale) || m.total == 0 {
            return None;
        }
        let used = m.total.saturating_sub(m.available);
        let zram = match m.zram.validity {
            Validity::Valid | Validity::Stale => m.zram.value.mem_used,
            _ => 0,
        };
        let compressed = zram.saturating_add(m.zswap).min(used);
        let in_use = used - compressed;
        let modified = m.dirty.saturating_add(m.writeback);
        let free = m.free;
        let standby = m
            .total
            .saturating_sub(in_use)
            .saturating_sub(compressed)
            .saturating_sub(modified)
            .saturating_sub(free);
        Some(Self {
            in_use,
            compressed,
            modified,
            standby,
            free,
            total: m.total,
        })
    }

    pub fn bytes(&self, kind: SegmentKind) -> u64 {
        match kind {
            SegmentKind::InUse => self.in_use,
            SegmentKind::Compressed => self.compressed,
            SegmentKind::Modified => self.modified,
            SegmentKind::Standby => self.standby,
            SegmentKind::Free => self.free,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    InUse,
    Compressed,
    Modified,
    Standby,
    Free,
}

impl SegmentKind {
    /// Left to right.
    pub const ALL: [SegmentKind; 5] = [
        SegmentKind::InUse,
        SegmentKind::Compressed,
        SegmentKind::Modified,
        SegmentKind::Standby,
        SegmentKind::Free,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// The `en-US.ftl` key of the segment name.
    pub const fn name_key(self) -> &'static str {
        match self {
            SegmentKind::InUse => "perf-mem-seg-in-use",
            SegmentKind::Compressed => "perf-mem-seg-compressed",
            SegmentKind::Modified => "perf-mem-seg-modified",
            SegmentKind::Standby => "perf-mem-seg-standby",
            SegmentKind::Free => "perf-mem-seg-free",
        }
    }

    /// The `en-US.ftl` key of the tooltip, with one argument `size`.
    pub const fn tip_key(self) -> &'static str {
        match self {
            SegmentKind::InUse => "perf-mem-tip-in-use",
            SegmentKind::Compressed => "perf-mem-tip-compressed",
            SegmentKind::Modified => "perf-mem-tip-modified",
            SegmentKind::Standby => "perf-mem-tip-standby",
            SegmentKind::Free => "perf-mem-tip-free",
        }
    }
}

/// One segment, logical px from the bar's left edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub x0: f32,
    pub x1: f32,
}

impl Segment {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
}

/// The non-zero segments left to right, proportional to their bytes over the sum of the five,
/// so the edges run from 0 to `width` with no gap. A non-zero segment is at least 2 px wide;
/// the largest segment gives up what the small ones took.
pub fn layout(c: &Composition, width: f32) -> SmallVec<[Segment; 5]> {
    let mut out = SmallVec::new();
    let sum: u64 = SegmentKind::ALL.iter().map(|&k| c.bytes(k)).sum();
    if sum == 0 || width <= 0.0 {
        return out;
    }
    let mut widths = [0f32; 5];
    for k in SegmentKind::ALL {
        widths[k.index()] = (c.bytes(k) as f64 / sum as f64 * f64::from(width)) as f32;
    }
    let mut taken = 0.0;
    for k in SegmentKind::ALL {
        let w = &mut widths[k.index()];
        if c.bytes(k) > 0 && *w < MIN_SEGMENT_W {
            taken += MIN_SEGMENT_W - *w;
            *w = MIN_SEGMENT_W;
        }
    }
    if taken > 0.0 {
        // The first largest segment pays for the minimums.
        let largest = SegmentKind::ALL
            .iter()
            .copied()
            .fold(SegmentKind::InUse, |best, k| {
                if widths[k.index()] > widths[best.index()] {
                    k
                } else {
                    best
                }
            });
        let w = &mut widths[largest.index()];
        *w = (*w - taken).max(MIN_SEGMENT_W);
    }
    let mut x = 0.0;
    let last = SegmentKind::ALL
        .iter()
        .rposition(|&k| c.bytes(k) > 0)
        .unwrap_or(0);
    for (i, k) in SegmentKind::ALL.into_iter().enumerate() {
        if c.bytes(k) == 0 {
            continue;
        }
        // The last segment ends at `width` exactly, so rounding never leaves a gap.
        let x1 = if i == last {
            width
        } else {
            x + widths[k.index()]
        };
        out.push(Segment { kind: k, x0: x, x1 });
        x = x1;
    }
    out
}

/// The tooltip of a segment in en-US, for example `In use: 20.11 GiB. Memory used by running
/// programs and the kernel.`
pub fn tooltip_text(kind: SegmentKind, bytes: u64) -> SharedString {
    tooltip_with(&Template::en(kind.tip_key(), &["size"]), bytes)
}

fn tooltip_with(template: &Template, bytes: u64) -> SharedString {
    let mut size = String::new();
    units::bytes(&mut size, bytes);
    let mut out = String::new();
    let _ = template.write(&mut out, &[&size]);
    out.into()
}

/// The bar's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct BarWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub caption: SharedString,
    pub collecting: SharedString,
    /// In [`SegmentKind::ALL`] order.
    pub names: [SharedString; 5],
    pub tips: [Template; 5],
}

impl BarWords {
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        if version == Some(None) {
            *self = Self::en();
            self.version = version;
            return true;
        }
        self.caption = tr(cx, KEY_CAPTION);
        self.collecting = tr(cx, KEY_COLLECTING);
        self.names = SegmentKind::ALL.map(|k| tr(cx, k.name_key()));
        self.tips = SegmentKind::ALL.map(|k| Template::new(cx, k.tip_key(), &["size"]));
        true
    }

    fn en() -> Self {
        Self {
            version: None,
            caption: tr_en(KEY_CAPTION, &[]),
            collecting: tr_en(KEY_COLLECTING, &[]),
            names: SegmentKind::ALL.map(|k| tr_en(k.name_key(), &[])),
            tips: SegmentKind::ALL.map(|k| Template::en(k.tip_key(), &["size"])),
        }
    }
}

/// What the bar draws, kept by the Memory page.
#[derive(Default)]
pub struct CompositionBarState {
    /// `None` while collecting (WarmingUp).
    pub composition: Option<Composition>,
    /// The segment under the pointer.
    pub hover: Option<SegmentKind>,
    /// Hatch lines over the compressed segment, bar-local logical px.
    pub hatch: Option<Path<Pixels>>,
    /// The compressed segment's physical x0 and x1 the hatch was built for.
    pub hatch_key: (u32, u32),
    /// Bar height in physical px the hatch was built for.
    hatch_h: u32,
    /// The segments of the last paint.
    pub segments: SmallVec<[Segment; 5]>,
    /// One shaped label per segment kind.
    labels: [CachedLine; 5],
    /// Hitboxes of the last prepaint, none while the pointer is outside.
    hitboxes: SmallVec<[(SegmentKind, Hitbox); 5]>,
}

/// The bar view: the caption, the bar element and the hovered segment's tooltip.
pub struct CompositionBarView {
    pub state: CompositionBarState,
    words: BarWords,
    _subs: SmallVec<[Subscription; 1]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// Hitboxes the last prepaint inserted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub hitbox_count: std::cell::Cell<u32>,
    /// Hatch path builds.
    #[cfg(any(test, feature = "test-hooks"))]
    pub hatch_builds: std::cell::Cell<u32>,
    /// Segment labels the last paint drew.
    #[cfg(any(test, feature = "test-hooks"))]
    pub painted_labels: std::cell::RefCell<SmallVec<[SegmentKind; 5]>>,
    /// Segment quads the last paint drew.
    #[cfg(any(test, feature = "test-hooks"))]
    pub painted_segments: std::cell::Cell<u32>,
}

impl CompositionBarView {
    pub fn new(composition: Option<Composition>, cx: &mut Context<Self>) -> Self {
        let mut words = BarWords::default();
        words.refresh(cx);
        let mut subs = SmallVec::new();
        // A new UI scale moves the physical edges; the hatch is built again on the next paint.
        subs.push(cx.observe_global::<UiMetrics>(|this: &mut Self, cx| {
            this.state.hatch = None;
            cx.notify();
        }));
        Self {
            state: CompositionBarState {
                composition,
                ..CompositionBarState::default()
            },
            words,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            hitbox_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            hatch_builds: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            painted_labels: std::cell::RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            painted_segments: std::cell::Cell::new(0),
        }
    }

    /// A new composition. Notifies only when it differs.
    pub fn set_composition(&mut self, c: Option<Composition>, cx: &mut Context<Self>) {
        if self.state.composition != c {
            self.state.composition = c;
            cx.notify();
        }
    }

    pub fn words(&self) -> &BarWords {
        &self.words
    }

    /// The caption the bar shows now.
    pub fn caption(&self) -> &SharedString {
        match self.state.composition {
            Some(_) => &self.words.caption,
            None => &self.words.collecting,
        }
    }

    /// The hovered segment's tooltip text.
    pub fn hover_tooltip(&self) -> Option<SharedString> {
        let (kind, c) = (self.state.hover?, self.state.composition?);
        Some(tooltip_with(&self.words.tips[kind.index()], c.bytes(kind)))
    }
}

impl Render for CompositionBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        self.words.refresh(cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let caption = div()
            .text_color(theme.text_secondary)
            .map_text(label_style(&m, m.text.sm, FontWeight::NORMAL))
            .child(self.caption().clone());

        let mut bar = div()
            .debug_selector(|| BAR_ID.into())
            .relative()
            .w_full()
            .h(px(BAR_H * m.ui_scale))
            .child(CompositionBar { view: cx.entity() });
        let hovered = self
            .state
            .hover
            .and_then(|k| self.state.segments.iter().find(|s| s.kind == k).copied());
        if let (Some(seg), Some(text)) = (hovered, self.hover_tooltip()) {
            let trigger = div()
                .debug_selector(|| BAR_TRIGGER_ID.into())
                .absolute()
                .top_0()
                .left(px(seg.x0))
                .w(px(seg.width()))
                .h_full();
            // One state per segment, so moving to another segment starts the delay again.
            let state = HoverTooltip::state((BAR_TRIGGER_ID, seg.kind.index()), window, cx);
            let trigger = HoverTooltip::attach(
                &state,
                trigger,
                DELAY_CONTROL,
                TlTooltip::text(text).build(),
                window,
                cx,
            );
            bar = bar.child(trigger);
        }
        v_flex().w_full().gap(m.sp[2]).child(caption).child(bar)
    }
}

/// Sets the text style of an element in one call.
trait MapText: Sized {
    fn map_text(self, style: gpui_kit::TextStyleRefinement) -> Self;
}

impl<E: gpui_kit::Styled> MapText for E {
    fn map_text(mut self, style: gpui_kit::TextStyleRefinement) -> Self {
        *self.text_style() = style;
        self
    }
}

/// The bar element. It reads and updates the view's state.
pub struct CompositionBar {
    view: Entity<CompositionBarView>,
}

impl IntoElement for CompositionBar {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// The colour of `kind` before hover.
fn segment_color(kind: SegmentKind, theme: &TlTheme) -> Hsla {
    let with = |c: Hsla, a: f32| Hsla { a: c.a * a, ..c };
    let line = theme.graph.mem_line;
    match kind {
        SegmentKind::InUse => line,
        SegmentKind::Compressed => with(line, 0.8),
        SegmentKind::Modified => with(theme.status.warning, 0.8),
        SegmentKind::Standby => with(line, 0.35),
        SegmentKind::Free => theme.window_bg,
    }
}

/// Diagonal 1 px lines every `step` px across `0..w × 0..h`, bar-local logical px from `x0`.
fn hatch_path(x0: f32, w: f32, h: f32, step: f32, sf: f32) -> Option<Path<Pixels>> {
    let mut b = PathBuilder::stroke(px(1.0 / sf.max(1.0)));
    let mut x = x0 - h;
    while x < x0 + w {
        b.move_to(point(px(x), px(h)));
        b.line_to(point(px(x + h), px(0.)));
        x += step;
    }
    b.build().ok()
}

/// `path` moved by `offset`.
fn moved(path: &Path<Pixels>, offset: Point<Pixels>) -> Path<Pixels> {
    let mut path = path.clone();
    path.bounds.origin = path.bounds.origin + offset;
    for v in &mut path.vertices {
        v.xy_position = v.xy_position + offset;
    }
    path
}

impl Element for CompositionBar {
    type RequestLayoutState = ();
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
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    /// Lays out the segments for the width, and inserts one hitbox per segment only while the
    /// pointer is inside the bar.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let inside = bounds.contains(&window.mouse_position());
        self.view.update(cx, |view, _| {
            let st = &mut view.state;
            st.segments.clear();
            if let Some(c) = st.composition {
                st.segments = layout(&c, f32::from(bounds.size.width));
            }
            st.hitboxes.clear();
            if inside {
                for seg in &st.segments {
                    let b = Bounds::new(
                        point(bounds.left() + px(seg.x0), bounds.top()),
                        size(px(seg.width()), bounds.size.height),
                    );
                    st.hitboxes
                        .push((seg.kind, window.insert_hitbox(b, HitboxBehavior::Normal)));
                }
            }
            #[cfg(any(test, feature = "test-hooks"))]
            view.hitbox_count.set(view.state.hitboxes.len() as u32);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let m = *cx.global::<UiMetrics>();
        let theme = current(cx).clone();
        let sf = window.scale_factor();
        let radius = m.radius_small;
        let view_id = self.view.entity_id();
        let had_hitboxes = !self.view.read(cx).state.hitboxes.is_empty();

        let hover_changed = self.view.update(cx, |view, cx| {
            let hovered = view
                .state
                .hitboxes
                .iter()
                .find(|(_, h)| h.is_hovered(window))
                .map(|(k, _)| *k);
            let changed = view.state.hover != hovered;
            view.state.hover = hovered;
            paint_bar(view, bounds, &m, &theme, sf, radius, window, cx);
            changed
        });
        // A notify during paint reaches no observer, so the view hears of it after the frame.
        if hover_changed {
            cx.defer(move |cx| cx.notify(view_id));
        }
        // Pointer moves repaint the bar only when the pointer enters or leaves it, or moves to
        // another segment.
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            let inside = bounds.contains(&e.position);
            view.update(cx, |view, cx| {
                if inside != had_hitboxes {
                    cx.notify();
                    return;
                }
                let hovered = view
                    .state
                    .hitboxes
                    .iter()
                    .find(|(_, h)| h.is_hovered(window))
                    .map(|(k, _)| *k);
                if hovered != view.state.hover {
                    view.state.hover = hovered;
                    cx.notify();
                }
            });
        });
    }
}

/// Segments, hatch, labels and outline.
#[allow(clippy::too_many_arguments)]
fn paint_bar(
    view: &mut CompositionBarView,
    bounds: Bounds<Pixels>,
    m: &UiMetrics,
    theme: &TlTheme,
    sf: f32,
    radius: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        view.painted_labels.borrow_mut().clear();
        view.painted_segments.set(0);
    }
    let st = &mut view.state;
    let n = st.segments.len();
    for (i, seg) in st.segments.iter().enumerate() {
        let b = Bounds::new(
            point(bounds.left() + px(seg.x0), bounds.top()),
            size(px(seg.width()), bounds.size.height),
        );
        let mut color = segment_color(seg.kind, theme);
        if st.hover == Some(seg.kind) {
            color.a = (color.a + HOVER_ALPHA).min(1.0);
        }
        // The outer corners follow the outline's radius.
        let (l, r) = (
            if i == 0 { radius } else { px(0.) },
            if i + 1 == n { radius } else { px(0.) },
        );
        let corners = Corners {
            top_left: l,
            bottom_left: l,
            top_right: r,
            bottom_right: r,
        };
        window.paint_quad(fill(b, color).corner_radii(corners));
        #[cfg(any(test, feature = "test-hooks"))]
        view.painted_segments.set(view.painted_segments.get() + 1);

        if seg.kind == SegmentKind::Compressed {
            let key = ((seg.x0 * sf).round() as u32, (seg.x1 * sf).round() as u32);
            let h = (f32::from(bounds.size.height) * sf).round() as u32;
            if st.hatch.is_none() || st.hatch_key != key || st.hatch_h != h {
                st.hatch = hatch_path(
                    seg.x0,
                    seg.width(),
                    f32::from(bounds.size.height),
                    HATCH_STEP * m.ui_scale,
                    sf,
                );
                st.hatch_key = key;
                st.hatch_h = h;
                #[cfg(any(test, feature = "test-hooks"))]
                view.hatch_builds.set(view.hatch_builds.get() + 1);
            }
            if let Some(path) = &st.hatch {
                let color = Hsla {
                    a: theme.window_bg.a * HATCH_ALPHA,
                    ..theme.window_bg
                };
                let path = moved(path, bounds.origin);
                window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                    window.paint_path(path, color);
                });
            }
        }
    }

    // Labels: a segment at least 72 px wide shows its name, left-aligned and centred.
    let label_min = LABEL_MIN_W * m.ui_scale;
    let row = px(labels::LABEL_ROW * m.ui_scale);
    let pad = m.sp[3];
    for seg in &st.segments {
        if seg.width() < label_min {
            continue;
        }
        let color = match seg.kind {
            SegmentKind::Free => theme.text_secondary,
            _ => theme.text_selected_primary,
        };
        let style = LineStyle {
            size: m.text.xs,
            weight: FontWeight::NORMAL,
            color,
            tnum: false,
        };
        let line = &mut st.labels[seg.kind.index()];
        line.sync(window, &view.words.names[seg.kind.index()], style);
        let origin = point(
            bounds.left() + px(seg.x0) + pad,
            bounds.top() + (bounds.size.height - row) / 2.0,
        );
        labels::paint_line(window, cx, line, origin, row);
        #[cfg(any(test, feature = "test-hooks"))]
        view.painted_labels.borrow_mut().push(seg.kind);
    }

    window.paint_quad(outline(bounds, theme.divider, BorderStyle::Solid).corner_radii(radius));
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;

    fn comp(in_use: u64, compressed: u64, modified: u64, standby: u64, free: u64) -> Composition {
        Composition {
            in_use,
            compressed,
            modified,
            standby,
            free,
            total: in_use + compressed + modified + standby + free,
        }
    }

    #[test]
    fn composition_segments_sum_to_total() {
        let c = comp(20 * GIB, GIB, 120 * MIB, 12 * GIB, 28 * GIB);
        let segs = layout(&c, 600.0);
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[0].x0, 0.0);
        assert_eq!(segs[4].x1, 600.0);
        for pair in segs.windows(2) {
            assert_eq!(pair[0].x1, pair[1].x0, "contiguous");
        }
        for s in &segs {
            let want = c.bytes(s.kind) as f64 / c.total as f64 * 600.0;
            if want >= f64::from(MIN_SEGMENT_W) {
                assert!(
                    (f64::from(s.width()) - want).abs() <= 0.5,
                    "{:?}: {} vs {want}",
                    s.kind,
                    s.width()
                );
            }
        }
    }

    #[test]
    fn composition_min_width_2px() {
        let c = Composition {
            in_use: 40 * GIB,
            compressed: 0,
            modified: 1024,
            standby: 8 * GIB,
            free: 16 * GIB - 1024,
            total: 64 * GIB,
        };
        let segs = layout(&c, 600.0);
        let w = |k| segs.iter().find(|s| s.kind == k).map(Segment::width);
        assert_eq!(w(SegmentKind::Modified), Some(2.0));
        assert!(
            w(SegmentKind::Compressed).is_none(),
            "zero segments are left out"
        );
        let exact = 40.0 / 64.0 * 600.0;
        let in_use = w(SegmentKind::InUse).unwrap_or_default();
        let taken = 2.0 - (1024.0 / (64.0 * GIB as f64) * 600.0) as f32;
        assert!((in_use - (exact - taken)).abs() < 1e-3, "{in_use}");
        assert_eq!(segs.last().map(|s| s.x1), Some(600.0));
    }

    #[test]
    fn composition_tooltip_texts() {
        let gib = |x: f64| (x * GIB as f64).round() as u64;
        let cases = [
            (
                SegmentKind::InUse,
                gib(20.11),
                "In use: 20.11 GiB. Memory used by running programs and the kernel.",
            ),
            (
                SegmentKind::Compressed,
                gib(1.02),
                "In use (compressed): 1.02 GiB. Memory held compressed in zram or zswap.",
            ),
            (
                SegmentKind::Modified,
                120 * MIB,
                "Modified: 120.00 MiB. Dirty pages waiting to be written to disk.",
            ),
            (
                SegmentKind::Standby,
                gib(12.40),
                "Standby: 12.40 GiB. Cache the kernel can drop.",
            ),
            (
                SegmentKind::Free,
                gib(28.60),
                "Free: 28.60 GiB. Unused memory.",
            ),
        ];
        for (kind, bytes, text) in cases {
            assert_eq!(tooltip_text(kind, bytes).as_ref(), text);
        }
    }

    #[test]
    fn composition_from_meminfo() {
        use tl_model::system::{Metric, ZramTotals};
        let m = MemoryTotals {
            validity: Validity::Valid,
            total: 64 * GIB,
            available: 40 * GIB,
            free: 20 * GIB,
            dirty: 100 * MIB,
            writeback: 20 * MIB,
            zswap: GIB,
            zram: Metric::valid(ZramTotals {
                orig_data: 0,
                compr_data: 0,
                mem_used: GIB,
            }),
            ..MemoryTotals::default()
        };
        let c = Composition::of(&m).expect("valid");
        assert_eq!(c.compressed, 2 * GIB);
        assert_eq!(c.in_use, 22 * GIB);
        assert_eq!(c.modified, 120 * MIB);
        assert_eq!(c.free, 20 * GIB);
        assert_eq!(
            c.standby,
            64 * GIB - 22 * GIB - 2 * GIB - 120 * MIB - 20 * GIB
        );
        let warming = MemoryTotals {
            validity: Validity::WarmingUp,
            ..m
        };
        assert_eq!(Composition::of(&warming), None);
    }
}
