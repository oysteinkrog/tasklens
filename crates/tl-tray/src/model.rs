//! Tray model state held by the service.
//!
//! `Service::apply` turns each `TrayModel` into ksni item changes: one item per tray graph plus
//! the optional main icon (PERF-088, PERF-091). An item is updated only when its pixels, tooltip
//! or menu source changed (UI-PRF-050).

use std::collections::BTreeMap;
use std::sync::Arc;

use tl_model::series::Sample;
use tl_model::tray::{TrayEvent, TrayGraphFrame, TrayGraphId, TrayGraphState, TrayModel};

use crate::MainIcon;
use crate::item::{ItemKind, MenuSource, TrayItem};
use crate::raster;

/// Spawns, updates and shuts down tray items. The real backend wraps ksni; tests use a fake that
/// counts calls.
pub(crate) trait ItemBackend {
    type Handle;
    fn spawn(&mut self, item: TrayItem) -> Result<Self::Handle, ksni::Error>;
    fn update(&mut self, handle: &Self::Handle, f: &mut dyn FnMut(&mut TrayItem));
    /// Asks the item to stop. Does not wait.
    fn shutdown(&mut self, handle: &Self::Handle);
    /// Waits until every item asked to stop has stopped.
    fn wait_shutdown(&mut self, handle: &Self::Handle);
}

pub(crate) struct KsniBackend;

impl ItemBackend for KsniBackend {
    type Handle = ksni::blocking::Handle<TrayItem>;

    fn spawn(&mut self, item: TrayItem) -> Result<Self::Handle, ksni::Error> {
        ksni::blocking::TrayMethods::spawn(item)
    }

    fn update(&mut self, handle: &Self::Handle, f: &mut dyn FnMut(&mut TrayItem)) {
        handle.update(f);
    }

    fn shutdown(&mut self, handle: &Self::Handle) {
        drop(handle.shutdown());
    }

    fn wait_shutdown(&mut self, handle: &Self::Handle) {
        handle.shutdown().wait();
    }
}

struct ItemState<H> {
    handle: H,
    icon_22: Vec<u8>,
    icon_44: Vec<u8>,
    title: Arc<str>,
    description: Arc<str>,
    menu: MenuSource,
}

pub(crate) struct Service<B: ItemBackend> {
    backend: B,
    events: async_channel::Sender<TrayEvent>,
    main_icon: MainIcon,
    graphs: BTreeMap<TrayGraphId, ItemState<B::Handle>>,
    main: Option<ItemState<B::Handle>>,
    scratch_22: Vec<u8>,
    scratch_44: Vec<u8>,
}

/// What changed for one existing item.
#[derive(Default)]
struct Changes {
    pixels: bool,
    tooltip: bool,
    menu: bool,
}

impl Changes {
    fn any(&self) -> bool {
        self.pixels || self.tooltip || self.menu
    }
}

impl<B: ItemBackend> Service<B> {
    pub(crate) fn new(
        backend: B,
        events: async_channel::Sender<TrayEvent>,
        main_icon: MainIcon,
    ) -> Self {
        Self {
            backend,
            events,
            main_icon,
            graphs: BTreeMap::new(),
            main: None,
            scratch_22: Vec::new(),
            scratch_44: Vec::new(),
        }
    }

    pub(crate) fn apply(&mut self, model: &TrayModel) {
        for frame in model.graphs.iter() {
            self.apply_graph(frame, model);
        }

        let graphs = &model.graphs;
        let backend = &mut self.backend;
        self.graphs.retain(|id, state| {
            let keep = graphs.iter().any(|frame| frame.id == *id);
            if !keep {
                backend.shutdown(&state.handle);
            }
            keep
        });

        self.apply_main(model);
    }

    /// Asks every item to stop and waits for each.
    pub(crate) fn shutdown_all(&mut self) {
        for state in self.graphs.values().chain(self.main.as_ref()) {
            self.backend.shutdown(&state.handle);
        }
        for state in self.graphs.values().chain(self.main.as_ref()) {
            self.backend.wait_shutdown(&state.handle);
        }
        self.graphs.clear();
        self.main = None;
    }

    fn apply_graph(&mut self, frame: &TrayGraphFrame, model: &TrayModel) {
        let menu = MenuSource {
            color: frame.color.clone(),
            range_s: frame.range_s,
            state: model.menu.clone(),
        };
        let frozen = matches!(frame.state, TrayGraphState::NoMatch | TrayGraphState::Ended);

        let Some(state) = self.graphs.get_mut(&frame.id) else {
            // A new item has no last pixels to keep, so it is drawn even when frozen.
            render_into(frame, &mut self.scratch_22, &mut self.scratch_44);
            let item = TrayItem {
                kind: ItemKind::Graph(frame.id),
                icon_22: self.scratch_22.clone(),
                icon_44: self.scratch_44.clone(),
                tooltip_title: frame.tooltip_title.clone(),
                tooltip_description: frame.tooltip_description.clone(),
                menu: menu.clone(),
                events: self.events.clone(),
            };
            match self.backend.spawn(item) {
                Ok(handle) => {
                    self.graphs.insert(
                        frame.id,
                        ItemState {
                            handle,
                            icon_22: self.scratch_22.clone(),
                            icon_44: self.scratch_44.clone(),
                            title: frame.tooltip_title.clone(),
                            description: frame.tooltip_description.clone(),
                            menu,
                        },
                    );
                }
                Err(_) => {
                    // The UI drains events every frame and sends models with `try_send`, so this
                    // waits at most one frame. It fails only when the UI end is gone.
                    let _ = self
                        .events
                        .send_blocking(TrayEvent::RegisterFailed(frame.id));
                }
            }
            return;
        };

        let mut changes = Changes::default();
        if !frozen {
            render_into(frame, &mut self.scratch_22, &mut self.scratch_44);
            if self.scratch_22 != state.icon_22 || self.scratch_44 != state.icon_44 {
                // Swap, not copy: the stored buffers take the new pixels and the old ones become
                // the next scratch buffers.
                std::mem::swap(&mut self.scratch_22, &mut state.icon_22);
                std::mem::swap(&mut self.scratch_44, &mut state.icon_44);
                changes.pixels = true;
            }
        }
        if *state.title != *frame.tooltip_title || *state.description != *frame.tooltip_description
        {
            state.title = frame.tooltip_title.clone();
            state.description = frame.tooltip_description.clone();
            changes.tooltip = true;
        }
        if state.menu != menu {
            state.menu = menu;
            changes.menu = true;
        }
        push_changes(&mut self.backend, state, &changes);
    }

    fn apply_main(&mut self, model: &TrayModel) {
        let wanted = model.show_main_icon && model.graphs.is_empty();

        match (&mut self.main, wanted) {
            (None, false) => {}
            (Some(state), false) => {
                self.backend.shutdown(&state.handle);
                self.main = None;
            }
            (None, true) => {
                let menu = MenuSource {
                    state: model.menu.clone(),
                    ..MenuSource::default()
                };
                let item = TrayItem {
                    kind: ItemKind::Main,
                    icon_22: self.main_icon.argb_22.clone(),
                    icon_44: self.main_icon.argb_44.clone(),
                    tooltip_title: model.main_tooltip_title.clone(),
                    tooltip_description: model.main_tooltip_description.clone(),
                    menu: menu.clone(),
                    events: self.events.clone(),
                };
                // A main icon that fails to register is tried again with the next model.
                if let Ok(handle) = self.backend.spawn(item) {
                    self.main = Some(ItemState {
                        handle,
                        icon_22: self.main_icon.argb_22.clone(),
                        icon_44: self.main_icon.argb_44.clone(),
                        title: model.main_tooltip_title.clone(),
                        description: model.main_tooltip_description.clone(),
                        menu,
                    });
                }
            }
            (Some(state), true) => {
                let mut changes = Changes::default();
                if *state.title != *model.main_tooltip_title
                    || *state.description != *model.main_tooltip_description
                {
                    state.title = model.main_tooltip_title.clone();
                    state.description = model.main_tooltip_description.clone();
                    changes.tooltip = true;
                }
                if state.menu.state != model.menu {
                    state.menu.state = model.menu.clone();
                    changes.menu = true;
                }
                push_changes(&mut self.backend, state, &changes);
            }
        }
    }
}

/// Draws the 22 and 44 px icons of `frame`. `Failed` or no series draws the baseline only.
fn render_into(frame: &TrayGraphFrame, out_22: &mut Vec<u8>, out_44: &mut Vec<u8>) {
    let samples: &[Sample] = match (&frame.series, frame.state) {
        (_, TrayGraphState::Failed) | (None, _) => &[],
        (Some(series), _) => &series.samples,
    };
    for (size, out) in [(22, out_22), (44, out_44)] {
        raster::render(
            samples,
            frame.now_ns,
            frame.range_ns,
            frame.max,
            frame.line_rgb,
            size,
            out,
        );
    }
}

/// Copies the changed parts of `state` into the live item with one `update` call. The icon is
/// copied into the item's existing buffers, so no allocation happens.
fn push_changes<B: ItemBackend>(backend: &mut B, state: &ItemState<B::Handle>, changes: &Changes) {
    if !changes.any() {
        return;
    }
    backend.update(&state.handle, &mut |item: &mut TrayItem| {
        if changes.pixels {
            item.icon_22.clear();
            item.icon_22.extend_from_slice(&state.icon_22);
            item.icon_44.clear();
            item.icon_44.extend_from_slice(&state.icon_44);
        }
        if changes.tooltip {
            item.tooltip_title = state.title.clone();
            item.tooltip_description = state.description.clone();
        }
        if changes.menu {
            item.menu = state.menu.clone();
        }
    });
}

/// Fake backend that keeps items in memory and counts calls.
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Arc, Mutex};

    use super::{ItemBackend, TrayItem};

    #[derive(Default)]
    pub(crate) struct Log {
        /// Every item spawned, by handle. `None` once shut down.
        pub items: Vec<Option<TrayItem>>,
        /// Update calls per handle.
        pub updates: Vec<usize>,
        pub shutdowns: Vec<usize>,
        pub waits: Vec<usize>,
        /// The next spawn fails when set.
        pub fail_next_spawn: bool,
    }

    impl Log {
        pub fn live(&self) -> Vec<&TrayItem> {
            self.items.iter().flatten().collect()
        }

        pub fn total_updates(&self) -> usize {
            self.updates.iter().sum()
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct FakeBackend(pub Arc<Mutex<Log>>);

    impl FakeBackend {
        pub fn log(&self) -> std::sync::MutexGuard<'_, Log> {
            self.0.lock().expect("log lock")
        }
    }

    impl ItemBackend for FakeBackend {
        type Handle = usize;

        fn spawn(&mut self, item: TrayItem) -> Result<usize, ksni::Error> {
            let mut log = self.log();
            if log.fail_next_spawn {
                log.fail_next_spawn = false;
                return Err(ksni::Error::WontShow);
            }
            log.items.push(Some(item));
            log.updates.push(0);
            Ok(log.items.len() - 1)
        }

        fn update(&mut self, handle: &usize, f: &mut dyn FnMut(&mut TrayItem)) {
            let mut log = self.log();
            log.updates[*handle] += 1;
            if let Some(item) = log.items[*handle].as_mut() {
                f(item);
            }
        }

        fn shutdown(&mut self, handle: &usize) {
            let mut log = self.log();
            log.shutdowns.push(*handle);
            log.items[*handle] = None;
        }

        fn wait_shutdown(&mut self, handle: &usize) {
            self.log().waits.push(*handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::SeriesView;
    use tl_model::tray::TrayMenuState;

    use super::fake::FakeBackend;
    use super::*;

    const S: u64 = 1_000_000_000;
    const NOW: u64 = 100 * S;

    fn series(values: &[f32]) -> Arc<SeriesView> {
        let n = values.len() as u64;
        let samples: Vec<Sample> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| Sample::valid(NOW - (n - 1 - i as u64) * S, v))
            .collect();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            },
            version: 1,
            period_ns: S,
            samples: Arc::from(samples),
        })
    }

    fn frame(id: u32, values: &[f32]) -> TrayGraphFrame {
        TrayGraphFrame {
            id: TrayGraphId(id),
            series: Some(series(values)),
            now_ns: NOW,
            range_ns: 60 * S,
            max: 100.0,
            line_rgb: [0x35, 0x84, 0xe4],
            state: TrayGraphState::Live,
            tooltip_title: Arc::from("CPU Usage"),
            tooltip_description: Arc::from("18.6%"),
            color: Arc::from("accent"),
            range_s: 60,
        }
    }

    fn model(show_main_icon: bool, graphs: Vec<TrayGraphFrame>) -> TrayModel {
        TrayModel {
            show_main_icon,
            main_tooltip_title: Arc::from("tasklens"),
            main_tooltip_description: Arc::from("CPU 12%"),
            graphs: Arc::from(graphs),
            menu: TrayMenuState::default(),
        }
    }

    fn main_icon() -> MainIcon {
        MainIcon {
            argb_22: vec![7; 22 * 22 * 4],
            argb_44: vec![9; 44 * 44 * 4],
        }
    }

    fn service() -> (
        Service<FakeBackend>,
        FakeBackend,
        async_channel::Receiver<TrayEvent>,
    ) {
        let backend = FakeBackend::default();
        let (tx, rx) = async_channel::bounded(16);
        (Service::new(backend.clone(), tx, main_icon()), backend, rx)
    }

    fn graph_item(backend: &FakeBackend, id: u32) -> (Vec<u8>, Vec<u8>) {
        let log = backend.log();
        let item = log
            .live()
            .into_iter()
            .find(|item| item.kind == ItemKind::Graph(TrayGraphId(id)))
            .expect("graph item");
        (item.icon_22.clone(), item.icon_44.clone())
    }

    #[test]
    fn tray_main_icon_hidden_with_graphs() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(
            true,
            vec![frame(1, &[10.0, 20.0]), frame(2, &[30.0, 40.0])],
        ));
        let log = backend.log();
        let kinds: Vec<ItemKind> = log.live().iter().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            [
                ItemKind::Graph(TrayGraphId(1)),
                ItemKind::Graph(TrayGraphId(2))
            ]
        );
        assert!(svc.main.is_none());
    }

    #[test]
    fn main_icon_follows_flag_without_graphs() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(true, vec![]));
        {
            let log = backend.log();
            let live = log.live();
            assert_eq!(live.len(), 1);
            assert_eq!(live[0].kind, ItemKind::Main);
            assert_eq!(live[0].icon_22, main_icon().argb_22);
            assert_eq!(live[0].icon_44, main_icon().argb_44);
            assert_eq!(&*live[0].tooltip_title, "tasklens");
        }

        svc.apply(&model(false, vec![]));
        assert!(backend.log().live().is_empty());

        let (mut svc, backend, _rx) = service();
        svc.apply(&model(false, vec![]));
        assert!(backend.log().items.is_empty());

        // A graph appearing removes the main icon; the graph leaving brings it back.
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(true, vec![]));
        svc.apply(&model(true, vec![frame(1, &[10.0])]));
        assert_eq!(backend.log().shutdowns, [0]);
        svc.apply(&model(true, vec![]));
        let log = backend.log();
        let kinds: Vec<ItemKind> = log.live().iter().map(|item| item.kind).collect();
        assert_eq!(kinds, [ItemKind::Main]);
    }

    #[test]
    fn identical_model_no_update() {
        let (mut svc, backend, _rx) = service();
        let m = model(false, vec![frame(1, &[10.0, 20.0]), frame(2, &[30.0])]);
        svc.apply(&m);
        svc.apply(&m);
        assert_eq!(backend.log().total_updates(), 0);

        let (mut svc, backend, _rx) = service();
        let m = model(true, vec![]);
        svc.apply(&m);
        svc.apply(&m);
        assert_eq!(backend.log().total_updates(), 0);
    }

    #[test]
    fn changed_pixels_update_only_that_item() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(
            false,
            vec![frame(1, &[10.0, 20.0]), frame(2, &[30.0, 40.0])],
        ));
        let before_1 = graph_item(&backend, 1);
        let before_2 = graph_item(&backend, 2);

        svc.apply(&model(
            false,
            vec![frame(1, &[10.0, 90.0]), frame(2, &[30.0, 40.0])],
        ));
        assert_eq!(backend.log().updates, [1, 0]);
        let after_1 = graph_item(&backend, 1);
        assert_ne!(after_1, before_1);
        assert_eq!(graph_item(&backend, 2), before_2);

        // The live item holds exactly what a fresh render gives.
        let (mut want_22, mut want_44) = (Vec::new(), Vec::new());
        render_into(&frame(1, &[10.0, 90.0]), &mut want_22, &mut want_44);
        assert_eq!(after_1, (want_22, want_44));
    }

    #[test]
    fn tooltip_and_menu_changes_update_once() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(false, vec![frame(1, &[10.0])]));
        let mut changed = frame(1, &[10.0]);
        changed.tooltip_description = Arc::from("50.0%");
        changed.color = Arc::from("teal");
        svc.apply(&model(false, vec![changed]));
        let log = backend.log();
        assert_eq!(log.updates, [1]);
        let item = log.live()[0];
        assert_eq!(&*item.tooltip_description, "50.0%");
        assert_eq!(&*item.menu.color, "teal");
    }

    #[test]
    fn ended_graph_freezes_pixels() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(false, vec![frame(1, &[10.0, 20.0])]));
        let before = graph_item(&backend, 1);

        for state in [TrayGraphState::Ended, TrayGraphState::NoMatch] {
            let mut later = frame(1, &[10.0, 20.0, 95.0, 5.0]);
            later.now_ns += 3 * S;
            later.state = state;
            svc.apply(&model(false, vec![later]));
            assert_eq!(graph_item(&backend, 1), before);
            assert_eq!(backend.log().total_updates(), 0);
        }
    }

    #[test]
    fn failed_graph_draws_baseline() {
        let (mut svc, backend, _rx) = service();
        let mut failed = frame(1, &[50.0, 60.0]);
        failed.state = TrayGraphState::Failed;
        svc.apply(&model(false, vec![failed.clone()]));
        failed.series = None;
        let (mut want_22, mut want_44) = (Vec::new(), Vec::new());
        render_into(&failed, &mut want_22, &mut want_44);
        assert_eq!(graph_item(&backend, 1), (want_22, want_44));
    }

    #[test]
    fn removed_graph_shuts_down() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(true, vec![frame(1, &[10.0]), frame(2, &[20.0])]));
        svc.apply(&model(true, vec![frame(2, &[20.0])]));
        let log = backend.log();
        assert_eq!(log.shutdowns, [0]);
        let kinds: Vec<ItemKind> = log.live().iter().map(|item| item.kind).collect();
        assert_eq!(kinds, [ItemKind::Graph(TrayGraphId(2))]);
        assert!(!svc.graphs.contains_key(&TrayGraphId(1)));
    }

    #[test]
    fn spawn_failure_reports_register_failed() {
        let (mut svc, backend, rx) = service();
        backend.log().fail_next_spawn = true;
        svc.apply(&model(false, vec![frame(4, &[10.0])]));
        assert_eq!(rx.try_recv(), Ok(TrayEvent::RegisterFailed(TrayGraphId(4))));
        assert!(svc.graphs.is_empty());
        assert!(backend.log().items.is_empty());

        // The next model tries again.
        svc.apply(&model(false, vec![frame(4, &[10.0])]));
        assert_eq!(svc.graphs.len(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn shutdown_all_stops_and_waits_for_every_item() {
        let (mut svc, backend, _rx) = service();
        svc.apply(&model(true, vec![frame(1, &[10.0]), frame(2, &[20.0])]));
        svc.shutdown_all();
        let log = backend.log();
        assert_eq!(log.shutdowns, [0, 1]);
        assert_eq!(log.waits, [0, 1]);
        assert!(log.live().is_empty());
    }
}
