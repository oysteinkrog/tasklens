//! History legend chips: the metric radio chips and the selected-process chips (UI-PRC-035,
//! PROC-088, UI-FND-010).
//!
//! Exactly one metric chip is on. Clicking a chip dispatches the matching `HistoryMetric*`
//! action. Each selected process (at most eight) gets a chip "<name> (<pid>)" with a dot in its
//! series colour and a trailing ×, which dispatches `HistoryClearProcess`. The rest show as one
//! "+N" label.
//!
//! The chips are built on the kit's unstyled base `Button`, with the `Segment { on }` colours
//! of `TlButton`. `TlButton` is not used because it forces its own padding and minimum width
//! and has no slot for a dot.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::{
    Action, AnyElement, App, FocusHandle, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    accesskit::Toggled, div, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::history::{HistoryMetric, HistoryView};
use tl_model::ids::ProcessKey;

use super::actions::{
    HistoryClearProcess, HistoryMetricCpu, HistoryMetricDisk, HistoryMetricGpu,
    HistoryMetricMemory, HistoryMetricNetwork,
};
use super::scrubber::series_color;
use super::{HistoryUiState, MAX_SERIES};
use crate::components::icons::{TlIcon, icon};
use crate::i18n::{tr, tr_en};
use crate::pages::performance::details::Template;
use crate::theme::{TlTheme, current, metrics};

/// Chip height, × ui_scale.
const CHIP_H: f32 = 24.;
/// Dot size in px, × ui_scale.
const DOT: f32 = 8.;
/// Fill alpha of the on chip.
const ON_ALPHA: f32 = 0.25;

/// The five metrics in chip order, with their id suffix and label key.
const METRICS: [(HistoryMetric, &str, &str); 5] = [
    (HistoryMetric::Cpu, "cpu", "history-chip-cpu"),
    (HistoryMetric::Memory, "memory", "history-chip-memory"),
    (HistoryMetric::Disk, "disk", "history-chip-disk"),
    (HistoryMetric::Network, "network", "history-chip-network"),
    (HistoryMetric::Gpu, "gpu", "history-chip-gpu"),
];
const KEY_MORE: &str = "history-chip-more";

/// The chip texts, translated once per `strings_version`. `Default` is en-US.
#[derive(Clone, Debug)]
pub struct LegendWords {
    /// Metric labels in [`METRICS`] order.
    pub metrics: [SharedString; 5],
    /// "+{$count}".
    pub more: Template,
}

impl Default for LegendWords {
    fn default() -> Self {
        Self {
            metrics: METRICS.map(|(_, _, key)| tr_en(key, &[])),
            more: Template::en(KEY_MORE, &["count"]),
        }
    }
}

impl LegendWords {
    /// Every text through `tr`.
    pub fn new(cx: &App) -> Self {
        Self {
            metrics: METRICS.map(|(_, _, key)| tr(cx, key)),
            more: Template::new(cx, KEY_MORE, &["count"]),
        }
    }
}

/// What the legend shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    /// The one active metric.
    pub metric: HistoryMetric,
    /// The visible process chips in selection order: key and "<name> (<pid>)".
    pub chips: SmallVec<[(ProcessKey, SharedString); MAX_SERIES]>,
    /// Selected processes past the visible chips.
    pub more: usize,
}

/// The legend for `state`. A name comes from the rows of `view`, else the chip shows the pid.
pub fn legend_model(state: &HistoryUiState, view: Option<&HistoryView>) -> Legend {
    let chips = state
        .selected
        .iter()
        .take(MAX_SERIES)
        .map(|&key| {
            let name = view
                .and_then(|v| v.rows.iter().find(|r| r.key == key))
                .map(|r| r.name.clone());
            let label = match name {
                Some(name) => format!("{name} ({})", key.pid),
                None => key.pid.to_string(),
            };
            (key, SharedString::from(label))
        })
        .collect();
    Legend {
        metric: state.metric,
        chips,
        more: state.selected.len().saturating_sub(MAX_SERIES),
    }
}

/// The line colour of `metric`.
fn metric_line(theme: &TlTheme, metric: HistoryMetric) -> Hsla {
    let g = &theme.graph;
    match metric {
        HistoryMetric::Cpu => g.cpu_line,
        HistoryMetric::Memory => g.mem_line,
        HistoryMetric::Disk => g.disk_line,
        HistoryMetric::Network => g.net_line,
        HistoryMetric::Gpu => g.gpu_line,
    }
}

fn metric_action(metric: HistoryMetric) -> Box<dyn Action> {
    match metric {
        HistoryMetric::Cpu => Box::new(HistoryMetricCpu),
        HistoryMetric::Memory => Box::new(HistoryMetricMemory),
        HistoryMetric::Disk => Box::new(HistoryMetricDisk),
        HistoryMetric::Network => Box::new(HistoryMetricNetwork),
        HistoryMetric::Gpu => Box::new(HistoryMetricGpu),
    }
}

/// One chip: dot, label and optional trailing ×, dispatching `action` on click.
fn chip(
    id: SharedString,
    label: SharedString,
    dot: Option<Hsla>,
    on: bool,
    clearable: bool,
    action: Box<dyn Action>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let focus: FocusHandle = window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let (bg, border) = if on {
        (theme.accent.alpha(ON_ALPHA), theme.accent)
    } else {
        (theme.toolbar, theme.divider)
    };
    let debug_id = id.clone();
    BaseButton::new(id)
        .track_focus(&focus)
        .key_context("TlButton")
        .debug_selector(move || debug_id.to_string())
        .accessibility_label(label.clone())
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(m.sp[2])
        .h(px(CHIP_H * m.ui_scale))
        .px(m.sp[4])
        .rounded(m.radius_small)
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_size(m.text.sm)
        .text_color(theme.text_primary)
        .when(!on, |b| b.hover(|s| s.bg(theme.row_hover)))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        .when_some(dot, |b, color| {
            b.child(
                div()
                    .flex_none()
                    .size(px(DOT * m.ui_scale))
                    .rounded_full()
                    .bg(color),
            )
        })
        .child(div().whitespace_nowrap().child(label))
        .when(clearable, |b| {
            b.child(
                div()
                    .flex_none()
                    .child(icon(TlIcon::X, m.icon.sm, theme.text_secondary)),
            )
        })
        .into_any_element()
}

/// The legend row: the metric chips, then the process chips and "+N".
pub fn render_legend(
    legend: &Legend,
    words: &LegendWords,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let mut row = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[2]);
    for (i, (metric, name, _)) in METRICS.iter().enumerate() {
        let on = *metric == legend.metric;
        row = row.child(chip(
            SharedString::from(format!("history-chip-{name}")),
            words.metrics[i].clone(),
            on.then(|| metric_line(&theme, *metric)),
            on,
            false,
            metric_action(*metric),
            window,
            cx,
        ));
    }
    for (i, (key, label)) in legend.chips.iter().enumerate() {
        row = row.child(chip(
            SharedString::from(format!("history-chip-proc-{i}")),
            label.clone(),
            Some(series_color(theme.accent, i)),
            false,
            true,
            Box::new(HistoryClearProcess { key: *key }),
            window,
            cx,
        ));
    }
    if legend.more > 0 {
        let mut text = String::new();
        // Writing to a String does not fail.
        let _ = words.more.write(&mut text, &[&legend.more]);
        row = row.child(
            div()
                .flex_none()
                .text_size(m.text.sm)
                .text_color(theme.text_secondary)
                .debug_selector(|| "history-chip-more".into())
                .child(SharedString::from(text)),
        );
    }
    row.into_any_element()
}
