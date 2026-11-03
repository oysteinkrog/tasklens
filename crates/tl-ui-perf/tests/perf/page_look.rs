//! The page look sweep (tasklens-hzire.10, ADR-019 "Flow placement" and "Inner grid"): every
//! widget of a device page draws the same in the flow layout, in Edit Widgets and after Apply.
//! Each page module calls [`sweep`] with its own page's container and editor. The sweep reads
//! what each metric widget draws (its accent, series count, legend and texts), enters Edit
//! Widgets, applies with no change, then enters again, moves one widget and applies, and checks
//! each time that no widget changed what it draws and none went missing.

use gpui_kit::{AppContext as _, Entity, VisualTestContext};
use tl_ui_chart::chart::ChartView;
use tl_ui_overview::widget::container::WidgetContainer;
use tl_ui_overview::widget::edit::WidgetEditor;
use tl_ui_settings::layout::WidgetContent;

/// What one widget draws that must not change with its placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// The chart's series count and the line colour of each series, as text.
    pub chart: Option<(usize, Vec<String>)>,
    /// The legend names of the chart.
    pub legend: Vec<String>,
    /// The name, value, stats and reason texts of a plain metric widget. A widget with a flow
    /// look draws these itself, so they are left out.
    pub texts: Option<Vec<String>>,
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

fn chart_look(c: &ChartView) -> (usize, Vec<String>) {
    let lines: Vec<String> = c
        .spec
        .series
        .iter()
        .map(|s| format!("{:?}", s.line))
        .collect();
    (lines.len(), lines)
}

/// The look of every placed widget of `container`, by id. Groups and fixed widgets draw their
/// own content in every layout and are listed with no look, so a widget that goes missing still
/// fails the comparison.
pub fn looks(
    vcx: &mut VisualTestContext,
    container: &Entity<WidgetContainer>,
) -> Vec<(u32, Option<Look>)> {
    let ids = vcx.update(|_, cx| container.read(cx).placed_ids());
    ids.into_iter()
        .map(|id| {
            let look = vcx.update(|_, cx| {
                let c = container.read(cx);
                let w = c.widget(id)?;
                let w = w.read(cx);
                let chart = w.chart().map(|c| chart_look(c.read(cx)));
                let legend = w
                    .chart()
                    .map(|c| {
                        c.read(cx)
                            .labels
                            .legend
                            .iter()
                            .map(|(n, _)| n.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let texts = w
                    .flow()
                    .is_none()
                    .then(|| w.texts().iter().map(|t| t.to_string()).collect());
                Some(Look {
                    chart,
                    legend,
                    texts,
                })
            });
            (id, look)
        })
        .collect()
}

/// Runs the sweep on page `page`.
pub fn sweep(
    vcx: &mut VisualTestContext,
    container: &Entity<WidgetContainer>,
    editor: &Entity<WidgetEditor>,
    page: &str,
) {
    let flow = looks(vcx, container);
    assert!(
        flow.iter().any(|(_, l)| l.is_some()),
        "{page}: the page has a metric widget"
    );
    // Edit Widgets pins the layout to the grid and changes no widget.
    vcx.update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(vcx);
    assert!(vcx.read_entity(editor, |e, _| e.is_editing()), "{page}");
    assert_eq!(looks(vcx, container), flow, "{page}: in Edit Widgets");
    // Apply with no change.
    vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    draw(vcx);
    assert_eq!(
        looks(vcx, container),
        flow,
        "{page}: after Apply, no change"
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
    vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    draw(vcx);
    assert_eq!(
        looks(vcx, container),
        flow,
        "{page}: after Apply with a move"
    );
}
