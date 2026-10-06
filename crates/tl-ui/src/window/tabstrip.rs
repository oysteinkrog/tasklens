//! The main tab strip under the title bar (E20-005, UI-SHL-020, UI-SHL-007, APP-027, R-023,
//! ADR-009, widgets §1).
//!
//! A row of plain `div` tabs in R-023 order, the filter box slot at the right end and an
//! `AccentRule` overlay that paints the active tab's outline and the accent line in one
//! `paint_path`. Not the kit `TabBar`: it has no outline joined to a line.
//!
//! Only the tabs whose page is built are shown. Tabs are not Tab stops; Ctrl+1..6, Alt+1..6,
//! Ctrl+Tab and Ctrl+Shift+Tab select them (E20-002). Each tab's accessible name is its label.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::accesskit::Role;
use gpui_kit::{
    AnyView, App, Bounds, Context, Element, ElementId, Entity, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    ParentElement, Path, PathBuilder, Pixels, Point, Position, Render, SharedString,
    StatefulInteractiveElement, Style, Styled, Subscription, TextRun, Window, div, point,
    prelude::FluentBuilder as _, px, relative,
};
use smallvec::SmallVec;
use tl_model::command::Tab;

use crate::actions::SelectTab;
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::theme::{UiMetrics, current, metrics};

/// Strip height, × ui_scale. Includes the 2 px accent line.
pub const TAB_STRIP_H: f32 = 30.0;
/// Tab padding left and right, × ui_scale. Measured.
pub const TAB_PAD_X: f32 = 22.0;
/// Outline and line thickness, × ui_scale.
pub const ACCENT_W: f32 = 2.0;

/// Control point factor of a cubic Bézier that follows a quarter circle.
const QUARTER_ARC_K: f32 = 0.552_284_8;

/// The active tab's and the filter slot's bounds, recorded when the tab row's children are
/// prepainted. `AccentRule` reads them in its own prepaint, which runs after the row's.
#[derive(Clone, Copy, Default)]
struct RuleLayout {
    active: Option<Bounds<Pixels>>,
    filter: Option<Bounds<Pixels>>,
}

/// What the cached rule path was built from. A change in any field rebuilds the path.
#[derive(Clone, PartialEq, Debug)]
struct AccentRuleKey {
    /// The active tab's bounds; zero size when the active tab is not shown.
    active: Bounds<Pixels>,
    /// Where the line ends: the filter slot's left edge, else the strip's right edge.
    end_x: Pixels,
    strip: Bounds<Pixels>,
    color: Hsla,
    scale_bits: u32,
}

/// The cached rule as tests see it.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq)]
pub struct AccentRuleProbe {
    pub active: Bounds<Pixels>,
    pub end_x: Pixels,
    pub strip: Bounds<Pixels>,
    pub color: Hsla,
    /// Outline and line thickness the path was built with.
    pub width: Pixels,
    /// Bounds of the built path.
    pub path_bounds: Bounds<Pixels>,
    /// Every vertex of the tessellated path.
    pub vertices: Vec<Point<Pixels>>,
}

/// The main tab strip.
pub struct TabStrip {
    /// Built tabs in R-023 order.
    tabs: SmallVec<[Tab; 6]>,
    active: Tab,
    /// The Processes label reads "History".
    history_mode: bool,
    window_active: bool,
    /// Set by E20-009. `None`: the line runs to the strip's right edge.
    filter_slot: Option<AnyView>,
    /// Label widths at `t_md` medium weight. Shaped once per language, scale and history mode
    /// change; read by the overflow menu (E20-015).
    label_widths: SmallVec<[(Tab, Pixels); 6]>,
    rule: Option<(AccentRuleKey, Path<Pixels>)>,
    layout: Rc<Cell<RuleLayout>>,
    style_version: u64,
    strings_version: u64,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn: SmallVec<[SharedString; 6]>,
    #[cfg(any(test, feature = "test-hooks"))]
    rule_builds: u32,
}

fn history_mode(cx: &App) -> bool {
    cx.try_global::<TlSettings>()
        .is_some_and(|_| TlSettings::state(cx).processes.history_mode)
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

impl TabStrip {
    pub fn new(
        tabs: SmallVec<[Tab; 6]>,
        active: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = SmallVec::new();
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if active != this.window_active {
                this.window_active = active;
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<TlSettings>(|this, cx| {
            let history = history_mode(cx);
            if history != this.history_mode {
                this.history_mode = history;
                this.label_widths.clear();
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<UiMetrics>(|this, cx| {
            let version = metrics(cx).style_version;
            if version != this.style_version {
                this.style_version = version;
                this.label_widths.clear();
                this.rule = None;
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<Strings>(|this, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.label_widths.clear();
                this.rule = None;
                cx.notify();
            }
        }));

        Self {
            tabs,
            active,
            history_mode: history_mode(cx),
            window_active: window.is_window_active(),
            filter_slot: None,
            label_widths: SmallVec::new(),
            rule: None,
            layout: Rc::new(Cell::new(RuleLayout::default())),
            style_version: metrics(cx).style_version,
            strings_version: strings_version(cx),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            drawn: SmallVec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            rule_builds: 0,
        }
    }

    /// Marks `tab` active. Notifies only when it changed.
    pub fn set_active(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab != self.active {
            self.active = tab;
            cx.notify();
        }
    }

    /// Replaces the built tabs (R-023 order).
    pub fn set_tabs(&mut self, tabs: SmallVec<[Tab; 6]>, cx: &mut Context<Self>) {
        if tabs != self.tabs {
            self.tabs = tabs;
            self.label_widths.clear();
            cx.notify();
        }
    }

    /// Sets the filter box view shown at the right end, or removes it.
    pub fn set_filter_slot(&mut self, slot: Option<AnyView>, cx: &mut Context<Self>) {
        self.filter_slot = slot;
        cx.notify();
    }

    /// The tab's label. Processes reads "History" in history mode.
    pub fn label(tab: Tab, history_mode: bool, cx: &App) -> SharedString {
        let key = match tab {
            Tab::Processes if history_mode => "shell-tab-history",
            Tab::Processes => "shell-tab-processes",
            Tab::Performance => "shell-tab-performance",
            Tab::Services => "shell-tab-services",
            Tab::Startup => "shell-tab-startup",
            Tab::Connections => "shell-tab-connections",
            Tab::Information => "shell-tab-information",
        };
        tr(cx, key)
    }

    pub fn active(&self) -> Tab {
        self.active
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Label widths at `t_md` medium weight, in tab order. Empty until the first render.
    pub fn label_widths(&self) -> &[(Tab, Pixels)] {
        &self.label_widths
    }

    /// The labels as last drawn, left to right.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_labels(&self) -> &[SharedString] {
        &self.drawn
    }

    /// The cached accent rule, `None` before the first paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn accent_rule(&self) -> Option<AccentRuleProbe> {
        self.rule.as_ref().map(|(key, path)| AccentRuleProbe {
            active: key.active,
            end_x: key.end_x,
            strip: key.strip,
            color: key.color,
            width: px(ACCENT_W * f32::from_bits(key.scale_bits)),
            path_bounds: path.bounds,
            vertices: path.vertices.iter().map(|v| v.xy_position).collect(),
        })
    }

    /// How many times the rule path was built.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rule_builds(&self) -> u32 {
        self.rule_builds
    }

    fn shape_labels(&mut self, labels: &[SharedString], m: &UiMetrics, window: &Window) {
        let mut font = window.text_style().font();
        font.weight = FontWeight::MEDIUM;
        self.label_widths = self
            .tabs
            .iter()
            .zip(labels)
            .map(|(tab, label)| {
                let run = TextRun {
                    len: label.len(),
                    font: font.clone(),
                    color: gpui_kit::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let width = window
                    .text_system()
                    .shape_line(label.clone(), m.text.md, &[run], None)
                    .width;
                (*tab, width)
            })
            .collect();
    }
}

fn stop(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

impl Render for TabStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let labels: SmallVec<[SharedString; 6]> = self
            .tabs
            .iter()
            .map(|tab| Self::label(*tab, self.history_mode, cx))
            .collect();
        if self.label_widths.is_empty() {
            self.shape_labels(&labels, &m, window);
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.drawn = labels.clone();
        }

        let height = px(TAB_STRIP_H * s);
        let tabs = self.tabs.iter().zip(labels).map(|(tab, label)| {
            let tab = *tab;
            let index = Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0);
            let active = tab == self.active;
            div()
                .id(ElementId::Name(tab.as_str().into()))
                .debug_selector(move || format!("TabStrip-{}", tab.as_str()))
                .role(Role::Tab)
                .aria_label(label.clone())
                .aria_selected(active)
                .flex()
                .flex_none()
                .items_center()
                .h(height)
                .px(px(TAB_PAD_X * s))
                .text_size(m.text.md)
                .text_color(theme.text_primary)
                .map(|el| {
                    if active {
                        el.font_weight(FontWeight::MEDIUM)
                            .bg(theme.header)
                            .rounded_t(m.radius_card)
                    } else {
                        let hover = theme.tab_hover;
                        el.font_weight(FontWeight::NORMAL)
                            .hover(move |st| st.bg(hover))
                            .active(move |st| st.bg(hover))
                    }
                })
                .on_mouse_down(MouseButton::Left, stop)
                .on_mouse_down(MouseButton::Right, stop)
                .on_mouse_down(MouseButton::Middle, stop)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(SelectTab(index)), cx);
                })
                .child(label)
        });

        let active_ix = self.tabs.iter().position(|t| *t == self.active);
        let filter_ix = self.filter_slot.is_some().then_some(self.tabs.len() + 1);
        let layout = self.layout.clone();
        let row = div()
            .flex()
            .flex_row()
            .size_full()
            .children(tabs)
            .child(div().flex_1())
            .when_some(self.filter_slot.clone(), |row, slot| {
                row.child(
                    div()
                        .debug_selector(|| "TabStripFilter".into())
                        .flex_none()
                        .h_full()
                        .child(slot),
                )
            })
            .on_children_prepainted(move |bounds, _, _| {
                layout.set(RuleLayout {
                    active: active_ix.and_then(|i| bounds.get(i).copied()),
                    filter: filter_ix.and_then(|i| bounds.get(i).copied()),
                });
            });

        let color = if self.window_active {
            theme.accent
        } else {
            theme.divider
        };
        div()
            .id("TabStrip")
            .debug_selector(|| "TabStrip".into())
            .role(Role::TabList)
            .relative()
            .flex_none()
            .w_full()
            .h(height)
            .bg(theme.tab_bg)
            .child(row)
            .child(AccentRule {
                strip: cx.entity(),
                layout: self.layout.clone(),
                color,
                scale: s,
                width: px(ACCENT_W * s),
                radius: m.radius_card,
            })
    }
}

/// Paints the active tab's outline and the accent line as one filled path, over the tabs.
struct AccentRule {
    strip: Entity<TabStrip>,
    layout: Rc<Cell<RuleLayout>>,
    color: Hsla,
    scale: f32,
    width: Pixels,
    radius: Pixels,
}

impl IntoElement for AccentRule {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for AccentRule {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// An absolute overlay that covers the whole strip.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    /// Rebuilds the cached path only when its key changed.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.layout.get();
        let key = AccentRuleKey {
            active: layout.active.unwrap_or_default(),
            end_x: layout.filter.map_or(bounds.right(), |f| f.left()),
            strip: bounds,
            color: self.color,
            scale_bits: self.scale.to_bits(),
        };
        let (width, radius) = (self.width, self.radius);
        self.strip.update(cx, |strip, _| {
            if strip.rule.as_ref().is_some_and(|(k, _)| *k == key) {
                return;
            }
            #[cfg(any(test, feature = "test-hooks"))]
            {
                strip.rule_builds += 1;
            }
            strip.rule = build_rule(&key, width, radius).map(|path| (key, path));
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((key, path)) = &self.strip.read(cx).rule {
            window.paint_path(path.clone(), key.color);
        }
    }
}

/// Draws a quarter-circle corner from `from` to `to` around the sharp corner `corner`.
fn arc(b: &mut PathBuilder, from: Point<Pixels>, corner: Point<Pixels>, to: Point<Pixels>) {
    b.cubic_bezier_to(
        to,
        from + (corner - from) * QUARTER_ARC_K,
        to + (corner - to) * QUARTER_ARC_K,
    );
}

/// Goes from `from` along a straight edge to the sharp `corner`, then turns towards `next`
/// with a corner of radius `r` (none when `r` is 0).
fn turn(
    b: &mut PathBuilder,
    from: Point<Pixels>,
    corner: Point<Pixels>,
    next: Point<Pixels>,
    r: Pixels,
) {
    if r <= px(0.) {
        b.line_to(corner);
        return;
    }
    // Edges are axis aligned, so the larger component is the length.
    let toward = |p: Point<Pixels>, q: Point<Pixels>| {
        let len = (q.x - p.x).abs().max((q.y - p.y).abs());
        if len == px(0.) {
            p
        } else {
            p + (q - p) * (r / len)
        }
    };
    let start = toward(corner, from);
    let end = toward(corner, next);
    b.line_to(start);
    arc(b, start, corner, end);
}

/// The outline and line as one closed shape, in window coordinates. About 14 segments.
///
/// Outer edge left to right: up the strip's left edge, along the line's top, a fillet up into
/// the active tab's left side, over the rounded top, down the right side, a fillet out onto the
/// line, along the line to `end_x` (with a fillet up into the filter slot's left side when it
/// is shown). Back along the bottom, the inner edge of the outline is the open bottom.
fn build_rule(key: &AccentRuleKey, w: Pixels, r: Pixels) -> Option<Path<Pixels>> {
    let strip = key.strip;
    let (x0, bottom) = (strip.left(), strip.bottom());
    let line_top = bottom - w;
    let end_x = key.end_x.max(x0);
    let filter = end_x < strip.right();
    let a = key.active;
    let has_active = a.size.width > px(0.) && a.left() >= x0 && a.right() <= end_x;

    let mut b = PathBuilder::fill();
    b.move_to(point(x0, bottom));
    b.line_to(point(x0, line_top));
    let mut at = point(x0, line_top);
    if has_active {
        let (l, rt, top) = (a.left(), a.right(), a.top());
        let ro = r.min(a.size.width / 2.).min((line_top - top) / 2.);
        let fl = r.min(l - x0);
        let gap = end_x - rt;
        let fr = if filter { r.min(gap / 2.) } else { r.min(gap) };

        turn(&mut b, at, point(l, line_top), point(l, top), fl);
        turn(
            &mut b,
            point(l, line_top),
            point(l, top),
            point(rt, top),
            ro,
        );
        turn(
            &mut b,
            point(l, top),
            point(rt, top),
            point(rt, line_top),
            ro,
        );
        turn(
            &mut b,
            point(rt, top),
            point(rt, line_top),
            point(end_x, line_top),
            fr,
        );
        at = point(rt, line_top);
        line_end(&mut b, at, end_x, line_top, filter, r.min(gap - fr));
        b.line_to(point(end_x, bottom));

        let ri = (ro - w).max(px(0.));
        let inner_top = top + w;
        b.line_to(point(rt - w, bottom));
        turn(
            &mut b,
            point(rt - w, bottom),
            point(rt - w, inner_top),
            point(l + w, inner_top),
            ri,
        );
        turn(
            &mut b,
            point(rt - w, inner_top),
            point(l + w, inner_top),
            point(l + w, bottom),
            ri,
        );
        b.line_to(point(l + w, bottom));
    } else {
        line_end(&mut b, at, end_x, line_top, filter, r.min(end_x - x0));
        b.line_to(point(end_x, bottom));
    }
    b.close();
    match b.build() {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::warn!(%err, "tab strip accent rule path failed");
            None
        }
    }
}

/// The line's top edge from `at` to `end_x`; with a filter slot it ends in a fillet of radius
/// `f` up into the slot's left side.
fn line_end(
    b: &mut PathBuilder,
    at: Point<Pixels>,
    end_x: Pixels,
    line_top: Pixels,
    filter: bool,
    f: Pixels,
) {
    if filter {
        turn(b, at, point(end_x, line_top), point(end_x, line_top - f), f);
    } else {
        b.line_to(point(end_x, line_top));
    }
}
