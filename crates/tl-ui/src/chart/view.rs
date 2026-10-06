//! `ChartView`: the entity behind one chart (ADR-009, UI-PRF-001).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Context, IntoElement, Render, Window};
use smallvec::SmallVec;
use tl_model::ids::SeriesId;
use tl_model::series::SeriesView;

use super::element::ChartElement;
use super::{ChartCache, ChartLabels, ChartSpec, ChartState, geometry, labels, scale};

pub struct ChartView {
    pub spec: ChartSpec,
    pub labels: ChartLabels,
    pub state: ChartState,
    pub(crate) cache: Rc<RefCell<ChartCache>>,
    /// Buffers reused by every geometry rebuild.
    pub(crate) scratch: geometry::Scratch,
    /// Right edge the cached geometry was built for. A stale series draws its tail up to it, so
    /// the element rebuilds when it moves while a series is stale.
    pub(crate) built_right_edge_ns: u64,
    /// Y scale memory across rebuilds (E19-004, R-262).
    pub(crate) scale_state: scale::ScaleState,
    /// Reused axis max text buffer for `scale::compute`.
    pub(crate) axis_text: String,
    /// Filled by E19-009; empty struct until then.
    #[allow(dead_code, reason = "read from E19-009 and E19-010 on")]
    pub(crate) text: labels::LabelCache,
    /// Resolved from the newest snapshot, one per `spec.series` entry.
    pub(crate) series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
    /// Reused formatter buffer.
    #[allow(dead_code, reason = "read from E19-009 and E19-010 on")]
    pub(crate) last_value_text: String,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// `geometry::build` calls.
    #[cfg(any(test, feature = "test-hooks"))]
    pub build_count: std::cell::Cell<u32>,
    /// `paint_path` calls, over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub path_paints: std::cell::Cell<u32>,
    /// Colours handed to `paint_path` by the last paint, in paint order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub path_colors: RefCell<SmallVec<[gpui_kit::Hsla; 8]>>,
    /// Right edge of the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_right_edge_ns: std::cell::Cell<u64>,
}

impl ChartView {
    pub fn new(spec: ChartSpec, labels: ChartLabels, _cx: &mut Context<Self>) -> Self {
        Self {
            spec,
            labels,
            state: ChartState::WarmingUp,
            cache: Rc::default(),
            scratch: geometry::Scratch::default(),
            built_right_edge_ns: 0,
            scale_state: scale::ScaleState::default(),
            axis_text: String::new(),
            text: labels::LabelCache::default(),
            series: SmallVec::new(),
            last_value_text: String::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            build_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_paints: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_colors: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            last_right_edge_ns: std::cell::Cell::new(0),
        }
    }

    /// Replaces the spec. Notifies only when it changed.
    pub fn set_spec(&mut self, spec: ChartSpec, cx: &mut Context<Self>) {
        if self.spec != spec {
            self.spec = spec;
            cx.notify();
        }
    }

    pub fn set_labels(&mut self, labels: ChartLabels, cx: &mut Context<Self>) {
        if self.labels != labels {
            self.labels = labels;
            cx.notify();
        }
    }

    /// Owner-supplied validity (Failed, Restricted, Unsupported). The view derives WarmingUp and
    /// Stale itself (E19-008).
    pub fn set_state(&mut self, state: ChartState, cx: &mut Context<Self>) {
        if self.state != state {
            self.state = state;
            cx.notify();
        }
    }

    pub fn series_ids(&self) -> impl Iterator<Item = SeriesId> + '_ {
        self.spec.series.iter().map(|s| s.id)
    }

    /// Sets the resolved series directly, one per `spec.series` entry. E19-008 binds them from the
    /// hub snapshot.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_series_for_test(
        &mut self,
        series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
        cx: &mut Context<Self>,
    ) {
        self.series = series;
        cx.notify();
    }

    /// Number of built paths in the geometry cache (fills, strokes and their stale sets).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_path_count(&self) -> usize {
        let c = self.cache.borrow();
        c.fills.len() + c.strokes.len() + c.stale_fills.len() + c.stale_strokes.len()
    }

    /// True while the geometry cache still holds its default key.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cache_key_is_default(&self) -> bool {
        self.cache.borrow().key == super::ChartGeometryKey::default()
    }
}

impl Render for ChartView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ChartElement::new(cx.entity())
    }
}
