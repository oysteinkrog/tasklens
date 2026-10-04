//! SnapshotHub integration tests with `FakeEngine` and `TestAppContext` (E16-003, ADR-005,
//! ADR-010, ADR-013). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div,
};
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::SeriesView;
use tl_model::snapshot::{SeriesSet, Snapshot};
use tl_ui::hub::{ComponentMask, HiddenChanged, SnapshotChanged, SnapshotHub};
use tl_ui::testing::fake_engine::FakeEngine;

fn hub_with_engine(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Entity<SnapshotHub>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(client, cx));
    (engine, hub)
}

/// Records every `SnapshotChanged` the hub emits.
fn record_changes(
    hub: &Entity<SnapshotHub>,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Vec<SnapshotChanged>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    cx.update(|cx| {
        cx.subscribe(hub, move |_, ev: &SnapshotChanged, _| {
            sink.borrow_mut().push(ev.clone());
        })
        .detach();
    });
    seen
}

/// Records every `HiddenChanged` the hub emits.
fn record_hidden(hub: &Entity<SnapshotHub>, cx: &mut TestAppContext) -> Rc<RefCell<Vec<bool>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    cx.update(|cx| {
        cx.subscribe(hub, move |_, ev: &HiddenChanged, _| {
            sink.borrow_mut().push(ev.hidden);
        })
        .detach();
    });
    seen
}

fn sid(device: u16, metric: MetricId) -> SeriesId {
    SeriesId {
        device: DeviceIdx(device),
        metric,
    }
}

fn view(id: SeriesId, version: u64) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id,
        version,
        period_ns: 1_000_000_000,
        samples: Arc::from([]),
    })
}

/// Replaces the series set with `views` (sorted by id) and the given set header version.
fn set_series(s: &mut Snapshot, header_version: u64, mut views: Vec<Arc<SeriesView>>) {
    views.sort_by_key(|v| v.id);
    let mut set = SeriesSet::default();
    set.header.version = header_version;
    set.series = views.into();
    s.series = Arc::new(set);
}

#[gpui_kit::test]
fn ignores_old_sequence(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let mut snap = Snapshot::clone(&engine.latest());
    snap.sequence = 5;
    Arc::make_mut(&mut snap.processes).header.version = 1;
    engine.publish(snap);
    cx.run_until_parked();
    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), 5);
    assert_eq!(seen.borrow().len(), 1);

    for sequence in [5, 4] {
        let mut old = Snapshot::clone(&engine.latest());
        old.sequence = sequence;
        Arc::make_mut(&mut old.processes).header.version += 10;
        let old = Arc::new(old);
        hub.update(cx, |h, cx| h.install(Arc::clone(&old), cx));
        cx.run_until_parked();
        hub.read_with(cx, |h, _| {
            assert_eq!(h.current().sequence, 5);
            assert_eq!(h.current().processes.header.version, 1);
        });
    }
    assert_eq!(seen.borrow().len(), 1);
}

#[gpui_kit::test]
fn mask_per_component(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let bumps: [(ComponentMask, fn(&mut Snapshot)); 13] = [
        (ComponentMask::SYSTEM, |s| {
            Arc::make_mut(&mut s.system).header.version += 1
        }),
        (ComponentMask::SERIES, |s| {
            Arc::make_mut(&mut s.series).header.version += 1
        }),
        (ComponentMask::PROCESSES, |s| {
            Arc::make_mut(&mut s.processes).header.version += 1
        }),
        (ComponentMask::TABLE, |s| {
            Arc::make_mut(&mut s.table).header.version += 1
        }),
        (ComponentMask::CONNECTIONS, |s| {
            Arc::make_mut(&mut s.connections).header.version += 1
        }),
        (ComponentMask::SERVICES, |s| {
            Arc::make_mut(&mut s.services).header.version += 1
        }),
        (ComponentMask::STARTUP, |s| {
            Arc::make_mut(&mut s.startup).header.version += 1
        }),
        (ComponentMask::INVENTORY, |s| {
            Arc::make_mut(&mut s.inventory).header.version += 1
        }),
        (ComponentMask::DEVICES, |s| {
            Arc::make_mut(&mut s.devices).version += 1
        }),
        (ComponentMask::HEALTH, |s| {
            Arc::make_mut(&mut s.health).version += 1
        }),
        (ComponentMask::WINDOWS, |s| {
            Arc::make_mut(&mut s.windows).header.version += 1
        }),
        (ComponentMask::DETAILS, |s| {
            Arc::make_mut(&mut s.details).header.version += 1
        }),
        (ComponentMask::HISTORY, |s| {
            Arc::make_mut(&mut s.history).header.version += 1
        }),
    ];

    for (i, (bit, bump)) in bumps.into_iter().enumerate() {
        engine.publish_with(bump);
        cx.run_until_parked();
        let seen = seen.borrow();
        assert_eq!(seen.len(), i + 1, "one event for {bit:?}");
        let ev = &seen[i];
        assert_eq!(ev.mask, bit);
        assert!(ev.changed_series.is_empty());
    }
    // A process change sets PROCESSES and not TABLE.
    assert!(!seen.borrow()[2].mask.contains(ComponentMask::TABLE));
}

#[gpui_kit::test]
fn changed_series_lists_only_changed(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let ids = [
        sid(0, MetricId::CPU_TOTAL),
        sid(0, MetricId::CPU_KERNEL),
        sid(1, MetricId::CPU_TOTAL),
        sid(1, MetricId::CPU_KERNEL),
        sid(0, MetricId::MEM_USED),
    ];
    engine.publish_with(|s| set_series(s, 1, ids.iter().map(|&id| view(id, 1)).collect()));
    cx.run_until_parked();
    assert_eq!(seen.borrow().len(), 1);

    // Bump two of the five CPU-side series and add one new; the set header stays the same.
    let added = sid(2, MetricId::CPU_TOTAL);
    engine.publish_with(|s| {
        let mut views: Vec<_> = ids
            .iter()
            .map(|&id| {
                let version = if id == ids[1] || id == ids[2] { 2 } else { 1 };
                view(id, version)
            })
            .collect();
        views.push(view(added, 1));
        set_series(s, 1, views);
    });
    cx.run_until_parked();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    let ev = &seen[1];
    assert_eq!(ev.mask, ComponentMask::SERIES);
    let mut expected = vec![ids[1], ids[2], added];
    expected.sort();
    assert_eq!(ev.changed_series.as_slice(), expected.as_slice());
    assert!(!ev.changed_series.spilled());
}

#[gpui_kit::test]
fn ten_thousand_publishes_monotonic(cx: &mut TestAppContext) {
    const LAST: u64 = 10_000;
    let (engine, hub) = hub_with_engine(cx);

    let observed = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&observed);
    cx.update(|cx| {
        cx.subscribe(&hub, move |hub, _: &SnapshotChanged, cx| {
            sink.borrow_mut().push(hub.read(cx).current().sequence);
        })
        .detach();
    });

    // The publisher wakes the hub task from another thread, which the test scheduler rejects as
    // non-deterministic unless parking is allowed.
    cx.executor().allow_parking();
    let publisher = Arc::clone(&engine);
    let thread = std::thread::spawn(move || {
        for _ in 0..LAST {
            publisher.publish_with(|s| Arc::make_mut(&mut s.system).header.version += 1);
        }
    });
    while !thread.is_finished() {
        cx.run_until_parked();
        std::thread::yield_now();
    }
    thread.join().expect("publisher thread");
    cx.run_until_parked();

    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), LAST);
    let observed = observed.borrow();
    assert_eq!(observed.last(), Some(&LAST));
    assert!(
        observed.windows(2).all(|w| w[0] < w[1]),
        "sequence went backwards"
    );
}

/// Draws nothing and notifies itself only when `PROCESSES` changed.
struct ProcessView {
    renders: Rc<Cell<u32>>,
}

impl ProcessView {
    fn new(hub: &Entity<SnapshotHub>, renders: Rc<Cell<u32>>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(hub, |_, _, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::PROCESSES) {
                cx.notify();
            }
        })
        .detach();
        Self { renders }
    }
}

impl Render for ProcessView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child("processes")
    }
}

#[gpui_kit::test]
fn cpu_only_change_does_not_notify_process_view(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let renders = Rc::new(Cell::new(0));
    let counter = Rc::clone(&renders);
    let hub_for_view = hub.clone();
    let (_view, cx) = cx.add_window_view(move |_, cx| ProcessView::new(&hub_for_view, counter, cx));
    cx.run_until_parked();
    let base = renders.get();
    assert!(base >= 1, "the window drew once");

    let cpu = sid(0, MetricId::CPU_TOTAL);
    engine.publish_with(|s| set_series(s, 1, vec![view(cpu, 1)]));
    cx.run_until_parked();
    assert_eq!(renders.get(), base, "CPU-only change must not render");

    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert_eq!(renders.get(), base + 1, "process change renders once");
}

#[gpui_kit::test]
fn hidden_after_1000ms_pending_without_render(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let hidden = record_hidden(&hub, cx);

    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));

    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    assert!(hub.read_with(cx, |h, _| h.is_hidden()));
    assert_eq!(*hidden.borrow(), vec![true]);

    // More time passes: still one event. Snapshots keep installing while hidden.
    cx.executor().advance_clock(Duration::from_millis(2000));
    cx.run_until_parked();
    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), 2);
    assert_eq!(*hidden.borrow(), vec![true]);

    hub.update(cx, |h, cx| h.note_render(cx));
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));
    assert_eq!(*hidden.borrow(), vec![true, false]);
}

#[gpui_kit::test]
fn no_hidden_without_pending_change(cx: &mut TestAppContext) {
    let (_engine, hub) = hub_with_engine(cx);
    let hidden = record_hidden(&hub, cx);

    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
    }
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));
    assert!(hidden.borrow().is_empty());
}
