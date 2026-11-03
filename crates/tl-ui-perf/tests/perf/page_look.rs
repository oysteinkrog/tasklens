//! The page look sweep (tasklens-hzire.10, ADR-019 "Flow placement" and "Inner grid"): every
//! widget of a device page draws the same in the flow layout, in Edit Widgets and after Apply.
//! Each page module calls [`sweep`] with its own page's container and editor. The sweep enters
//! Edit Widgets, applies with no change, then enters again, moves one widget and applies. At
//! each stage it reads what the last frame drew, by debug selector:
//!
//! - every placed widget's root is drawn at its placed size and place, inside the container's
//!   rows, and the container lies in the editor's visible area, so a hidden, empty or misplaced
//!   widget fails;
//! - in Edit Widgets the editor draws a frame over each widget, at the widget's bounds;
//! - a metric widget's look is the parts it drew (name, value, stats, reason, bar, chart and the
//!   empty-sensor lines), the text of each drawn text part, the fill of a drawn bar, and of a
//!   drawn chart its series colours, legend and the labels its last paint drew (a page chart
//!   draws its heading and value itself) with their colours and texts;
//! - a group's look is its state, its column count, its hidden count, its drawn lines and the
//!   look of each drawn member, each member root drawn inside the group;
//! - a fixed widget's look is the content view it hosts and the lines it drew.
//!
//! The looks after each stage must equal the flow layout's, widget by widget.

use gpui_kit::{AppContext as _, Bounds, Entity, EntityId, Hsla, Pixels, VisualTestContext, px};
use tl_ui_chart::chart::ChartView;
use tl_ui_chart::chart::labels::LabelKind;
use tl_ui_overview::widget::container::{WidgetContainer, container_id};
use tl_ui_overview::widget::edit::{WidgetEditor, editor_id, frame_id};
use tl_ui_overview::widget::metric_widget::{MetricWidget, part_id, widget_id};
use tl_ui_settings::layout::WidgetContent;

/// The parts a metric widget can draw, each with its debug selector suffix.
const METRIC_PARTS: [&str; 8] = [
    "name",
    "value",
    "stats",
    "reason",
    "bar",
    "chart",
    "no-value",
    "choose-sensor",
];

/// The line parts a group or a fixed widget draws besides its members or content.
const OWN_LINES: [&str; 3] = ["name", "reason", "more"];

/// How far a drawn size may be from the placed size, px: the placement rounds to whole px.
const SLACK: f32 = 1.;

/// What a drawn chart shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLook {
    /// The line colour of each series, in order.
    pub lines: Vec<Hsla>,
    /// The legend names.
    pub legend: Vec<String>,
    /// The labels the last paint drew (heading, value, legend, axis max, range), each with its
    /// colour, sorted. The axis ticks are left out.
    pub painted: Vec<(String, Hsla)>,
    /// The text of each painted heading, value and range label.
    pub texts: Vec<(&'static str, String)>,
}

/// What a metric widget, or one member of a group, drew.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricLook {
    /// The parts drawn with a non-empty width, in `METRIC_PARTS` order.
    pub parts: Vec<&'static str>,
    /// The text of each drawn text part.
    pub texts: Vec<(&'static str, String)>,
    /// The bar's fill when the bar is drawn.
    pub bar: Option<String>,
    /// The chart when its part is drawn.
    pub chart: Option<ChartLook>,
}

/// What one widget drew that must not change with its placement.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    Metric(MetricLook),
    Group {
        state: String,
        cols: usize,
        hidden: usize,
        /// The group's own lines drawn ("None found", "+3 more"), with their text.
        lines: Vec<(&'static str, String)>,
        members: Vec<MetricLook>,
    },
    Fixed {
        /// The content view the widget hosts.
        content: Option<EntityId>,
        /// The lines drawn when it has no content.
        lines: Vec<&'static str>,
    },
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// The bounds of debug selector `id` at the last frame, when drawn with a non-empty area.
fn drawn(vcx: &mut VisualTestContext, id: String) -> Option<Bounds<Pixels>> {
    vcx.debug_bounds(Box::leak(id.into_boxed_str()))
        .filter(|b| b.size.width > px(0.) && b.size.height > px(0.))
}

/// Whether `inner` lies in `outer`, with `SLACK` px to spare.
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let (i, o) = (inner, outer);
    f32::from(i.left()) >= f32::from(o.left()) - SLACK
        && f32::from(i.top()) >= f32::from(o.top()) - SLACK
        && f32::from(i.right()) <= f32::from(o.right()) + SLACK
        && f32::from(i.bottom()) <= f32::from(o.bottom()) + SLACK
}

fn near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    inside(a, b) && inside(b, a)
}

fn chart_look(c: &ChartView) -> ChartLook {
    let mut painted: Vec<(String, Hsla)> = c
        .label_cache()
        .painted
        .iter()
        .filter(|l| !matches!(l.kind, LabelKind::XTick(_) | LabelKind::YTick(_)))
        .map(|l| (format!("{:?}", l.kind), l.color))
        .collect();
    painted.sort_by(|a, b| a.0.cmp(&b.0));
    let drew = |kind: LabelKind| c.label_cache().painted.iter().any(|l| l.kind == kind);
    let mut texts = Vec::new();
    if drew(LabelKind::Heading)
        && let Some(h) = &c.labels.heading
    {
        texts.push(("heading", h.to_string()));
    }
    if drew(LabelKind::Value)
        && let Some(v) = c.painted_value_text()
    {
        texts.push(("value", v.to_owned()));
    }
    if drew(LabelKind::Range) {
        texts.push(("range", c.labels.range_word.to_string()));
    }
    ChartLook {
        lines: c.spec.series.iter().map(|s| s.line).collect(),
        legend: c.labels.legend.iter().map(|(n, _)| n.to_string()).collect(),
        painted,
        texts,
    }
}

/// What metric widget `w` drew at the last frame. Its parts carry container `cid`'s selectors.
fn metric_look(vcx: &mut VisualTestContext, cid: &str, w: &Entity<MetricWidget>) -> MetricLook {
    let (id, texts, bar, chart) = vcx.update(|_, cx| {
        let m = w.read(cx);
        (
            m.id(),
            m.texts().map(|t| t.to_string()),
            m.bar_fraction(),
            m.chart().map(|c| chart_look(c.read(cx))),
        )
    });
    let parts: Vec<&'static str> = METRIC_PARTS
        .into_iter()
        .filter(|p| drawn(vcx, part_id(cid, id, p)).is_some())
        .collect();
    let text_of = |p: &str| match p {
        "name" => Some(texts[0].clone()),
        "value" => Some(texts[1].clone()),
        "stats" => Some(texts[2].clone()),
        "reason" => Some(texts[3].clone()),
        _ => None,
    };
    MetricLook {
        texts: parts
            .iter()
            .filter_map(|p| text_of(p).map(|t| (*p, t)))
            .collect(),
        bar: parts
            .contains(&"bar")
            .then(|| bar.map(|f| format!("{f:.3}")))
            .flatten(),
        chart: if parts.contains(&"chart") {
            chart
        } else {
            None
        },
        parts,
    }
}

/// The look of every placed widget of `container`, by id, read from the last frame. Checks on
/// the way that the container is drawn in the editor's visible area, that each widget is drawn
/// at its placed size and place inside the container's rows (a page body taller than the area
/// scrolls), and, in Edit Widgets, under a frame of the editor at its bounds.
pub fn looks(
    vcx: &mut VisualTestContext,
    container: &Entity<WidgetContainer>,
    editor: &Entity<WidgetEditor>,
    stage: &str,
) -> Vec<(u32, Look)> {
    let editing = vcx.read_entity(editor, |e, _| e.is_editing());
    let (cid, placed, content_h, scroll) = vcx.update(|_, cx| {
        let c = container.read(cx);
        // In Edit Widgets the editor's body scrolls the whole container; outside it the
        // container's own body scrolls its rows.
        let scroll = if editing { px(0.) } else { c.scroll_offset() };
        (
            c.id().to_string(),
            c.placement().widgets.clone(),
            c.placement().content_h,
            scroll,
        )
    });
    let area = drawn(vcx, editor_id(&cid)).unwrap_or_else(|| panic!("{stage}: the editor draws"));
    let body =
        drawn(vcx, container_id(&cid)).unwrap_or_else(|| panic!("{stage}: the container draws"));
    // The container starts in the editor's visible area and is no wider; a page body taller
    // than the area scrolls, so a widget below the fold is reached by the scroll.
    assert!(
        inside(
            Bounds::new(body.origin, gpui_kit::size(body.size.width, px(0.))),
            area
        ),
        "{stage}: the container {body:?} lies in the visible area {area:?}"
    );
    let extent = f32::from(body.size.height).max(f32::from(content_h));
    let mut out = Vec::new();
    for p in placed {
        let id = p.id;
        let root = drawn(vcx, widget_id(&cid, id))
            .unwrap_or_else(|| panic!("{stage}: widget {id} is drawn"));
        for (drawn_side, placed_side) in [
            (root.size.width, p.bounds.size.width),
            (root.size.height, p.bounds.size.height),
        ] {
            assert!(
                (f32::from(drawn_side) - f32::from(placed_side)).abs() <= SLACK,
                "{stage}: widget {id} is drawn at {root:?}, placed at {:?}",
                p.bounds
            );
        }
        // Drawn where it is placed in the container, and inside the rows the body scrolls over.
        let (dx, dy) = (
            f32::from(root.origin.x - body.origin.x),
            f32::from(root.origin.y - body.origin.y + scroll),
        );
        assert!(
            (dx - f32::from(p.bounds.origin.x)).abs() <= SLACK
                && (dy - f32::from(p.bounds.origin.y)).abs() <= SLACK,
            "{stage}: widget {id} is drawn at {root:?} in the container at {body:?}, placed at {:?}",
            p.bounds
        );
        assert!(
            f32::from(p.bounds.right()) <= f32::from(body.size.width) + SLACK
                && f32::from(p.bounds.bottom()) <= extent + SLACK,
            "{stage}: widget {id} placed at {:?} lies in the container's rows ({body:?}, {extent} px tall)",
            p.bounds
        );
        if editing {
            let frame = drawn(vcx, frame_id(&cid, id))
                .unwrap_or_else(|| panic!("{stage}: the editor frames widget {id}"));
            assert!(
                near(frame, root),
                "{stage}: widget {id}'s frame {frame:?} is at the widget {root:?}"
            );
        }
        let (metric, group, fixed) = vcx.update(|_, cx| {
            let c = container.read(cx);
            (
                c.widget(id).cloned(),
                c.group(id).cloned(),
                c.fixed(id).cloned(),
            )
        });
        let look = if let Some(w) = metric {
            Look::Metric(metric_look(vcx, &cid, &w))
        } else if let Some(g) = group {
            let (state, cols, hidden, more, reason, members) = vcx.update(|_, cx| {
                let g = g.read(cx);
                (
                    format!("{:?}", g.state()),
                    g.fill().cols,
                    g.hidden(),
                    g.more_text().to_string(),
                    g.reason_text().to_string(),
                    g.items().map(|(_, w)| w.clone()).collect::<Vec<_>>(),
                )
            });
            let lines = OWN_LINES
                .into_iter()
                .filter(|l| drawn(vcx, part_id(&cid, id, l)).is_some())
                .map(|l| {
                    let text = match l {
                        "more" => more.clone(),
                        "reason" => reason.clone(),
                        _ => String::new(),
                    };
                    (l, text)
                })
                .collect();
            let mut looks = Vec::new();
            for m in &members {
                let mid = vcx.read_entity(m, |m, _| m.id());
                // A member scrolled out of the group's body is not drawn; one drawn lies in it.
                let Some(b) = drawn(vcx, widget_id(&cid, mid)) else {
                    continue;
                };
                assert!(
                    inside(b, root),
                    "{stage}: member {mid} of group {id} at {b:?} lies in the group {root:?}"
                );
                looks.push(metric_look(vcx, &cid, m));
            }
            if state.starts_with("Members") {
                assert!(
                    !looks.is_empty(),
                    "{stage}: group {id} draws a member ({state})"
                );
            }
            Look::Group {
                state,
                cols,
                hidden,
                lines,
                members: looks,
            }
        } else if let Some(f) = fixed {
            let content = vcx.update(|_, cx| f.read(cx).content().map(|v| v.entity_id()));
            let lines = OWN_LINES
                .into_iter()
                .filter(|l| drawn(vcx, part_id(&cid, id, l)).is_some())
                .collect();
            Look::Fixed { content, lines }
        } else {
            panic!("{stage}: placed widget {id} has a view");
        };
        out.push((id, look));
    }
    out
}

/// Runs the sweep on page `page`.
pub fn sweep(
    vcx: &mut VisualTestContext,
    container: &Entity<WidgetContainer>,
    editor: &Entity<WidgetEditor>,
    page: &str,
) {
    draw(vcx);
    let flow = looks(vcx, container, editor, &format!("{page} flow"));
    // Every widget draws something, so a sweep that compares empty looks cannot pass.
    for (id, look) in &flow {
        let draws = match look {
            Look::Metric(m) => !m.parts.is_empty(),
            Look::Group { members, lines, .. } => !members.is_empty() || !lines.is_empty(),
            Look::Fixed { content, lines } => content.is_some() || !lines.is_empty(),
        };
        assert!(draws, "{page}: widget {id} draws a part: {look:?}");
    }
    let metrics = || {
        flow.iter().flat_map(|(_, l)| match l {
            Look::Metric(m) => vec![m],
            Look::Group { members, .. } => members.iter().collect(),
            Look::Fixed { .. } => Vec::new(),
        })
    };
    assert!(
        metrics().any(|m| m.chart.as_ref().is_some_and(|c| !c.lines.is_empty())),
        "{page}: the page draws a chart: {flow:?}"
    );
    assert!(
        metrics().any(|m| {
            m.texts.iter().any(|(_, t)| !t.is_empty())
                || m.chart
                    .as_ref()
                    .is_some_and(|c| c.texts.iter().any(|(_, t)| !t.is_empty()))
        }),
        "{page}: the page draws a text: {flow:?}"
    );
    let compare = |after: &[(u32, Look)], stage: &str| {
        let ids = |l: &[(u32, Look)]| l.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut want = ids(&flow);
        let mut got = ids(after);
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want, "{page} {stage}: the same widgets are drawn");
        for (id, look) in &flow {
            let (_, l) = after.iter().find(|(i, _)| i == id).expect("checked above");
            assert_eq!(l, look, "{page} {stage}: widget {id} draws the same");
        }
    };
    // Edit Widgets pins the layout to the grid and changes no widget.
    vcx.update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(vcx);
    assert!(vcx.read_entity(editor, |e, _| e.is_editing()), "{page}");
    let stage = format!("{page} Edit Widgets");
    compare(&looks(vcx, container, editor, &stage), "in Edit Widgets");
    // Apply with no change.
    vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    draw(vcx);
    let stage = format!("{page} Apply");
    compare(
        &looks(vcx, container, editor, &stage),
        "after Apply, no change",
    );
    // Move the first metric widget and apply: the saved grid layout draws the same.
    vcx.update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(vcx);
    let first = vcx
        .update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .find(|w| matches!(w.content, WidgetContent::Metric(_)))
                .map(|w| w.id)
        })
        .unwrap_or_else(|| panic!("{page}: a metric widget"));
    vcx.update(|_, cx| editor.update(cx, |e, cx| e.select(first, cx)));
    let moved = vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.move_selected(0, 1, cx) || e.move_selected(1, 0, cx) || e.move_selected(-1, 0, cx)
        })
    });
    assert!(moved, "{page}: the widget moves");
    draw(vcx);
    let stage = format!("{page} Edit Widgets, moved");
    compare(
        &looks(vcx, container, editor, &stage),
        "in Edit Widgets after a move",
    );
    vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    draw(vcx);
    let stage = format!("{page} Apply, moved");
    let grid = looks(vcx, container, editor, &stage);
    assert!(
        vcx.update(|_, cx| container.read(cx).layout().preset.is_none()),
        "{page}: Apply after a move saves a grid layout"
    );
    compare(&grid, "after Apply with a move");
}
