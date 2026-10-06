//! `ChartView`: the entity behind one chart (ADR-009, UI-PRF-001).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Context, IntoElement, Render, Window, div};
use smallvec::SmallVec;
use tl_model::ids::SeriesId;
use tl_model::series::SeriesView;

use super::{ChartCache, ChartLabels, ChartSpec, ChartState, labels};

pub struct ChartView {
    pub spec: ChartSpec,
    pub labels: ChartLabels,
    pub state: ChartState,
    pub(crate) cache: Rc<RefCell<ChartCache>>,
    // E19-004: scale_state: scale::ScaleState (added by yrf.6 as a chain edit, R-262)
    /// Filled by E19-009; empty struct until then.
    pub(crate) text: labels::LabelCache,
    /// Resolved from the newest snapshot.
    pub(crate) series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
    /// Reused formatter buffer.
    pub(crate) last_value_text: String,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
}

impl ChartView {
    pub fn new(spec: ChartSpec, labels: ChartLabels, _cx: &mut Context<Self>) -> Self {
        Self {
            spec,
            labels,
            state: ChartState::WarmingUp,
            cache: Rc::default(),
            text: labels::LabelCache::default(),
            series: SmallVec::new(),
            last_value_text: String::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // E19-007: element::ChartElement::new(cx.entity().clone())
        div()
    }
}
