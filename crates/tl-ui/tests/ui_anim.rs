//! `AnimationDriver` tests with `TestAppContext` (E19, ADR-009, ADR-010, ADR-014, R-209). The test
//! platform delivers no frames; `Window::simulate_next_frame` stands in for the compositor.
//! `driver_paint_log_install_and_take` needs `--features test-hooks`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{
    AppContext, Context, Entity, EntityId, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use tl_ui::anim::AnimationDriver;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Root view that owns the window's driver, as a real window constructor does.
struct Root {
    driver: Entity<AnimationDriver>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A requester that is not a view.
struct Plain;

fn rig(cx: &mut TestAppContext) -> (Entity<AnimationDriver>, &mut VisualTestContext) {
    let (root, vcx) = cx.add_window_view(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Root { driver }
    });
    let driver = vcx.read_entity(&root, |r, _| r.driver.clone());
    (driver, vcx)
}

fn request(vcx: &mut VisualTestContext, id: EntityId) {
    vcx.update(|window, cx| {
        AnimationDriver::global(window, cx).update(cx, |d, cx| d.request(id, window, cx))
    });
}

fn frame(vcx: &mut VisualTestContext) -> usize {
    vcx.update(|window, cx| window.simulate_next_frame(cx))
}

fn advance(vcx: &mut VisualTestContext, ms: u64) {
    vcx.executor().advance_clock(Duration::from_millis(ms));
    vcx.run_until_parked();
}

fn stats(driver: &Entity<AnimationDriver>, vcx: &mut VisualTestContext) -> (u64, u64, u64, u64) {
    vcx.read_entity(driver, |d, _| {
        (
            d.stats.requested,
            d.stats.frames,
            d.stats.stalls,
            d.stats.missed,
        )
    })
}

#[gpui_kit::test]
fn driver_requests_no_frame_when_static(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    advance(vcx, 500);
    assert_eq!(frame(vcx), 0);
    let (requested, frames, _, _) = stats(&driver, vcx);
    assert_eq!((requested, frames), (0, 0));
    assert!(!vcx.read_entity(&driver, |d, _| d.is_armed()));
}

#[gpui_kit::test]
fn driver_one_frame_for_many_charts(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    let counts = Rc::new(RefCell::new(vec![0u32; 32]));
    let mut subs = Vec::new();
    let mut entities = Vec::new();
    for i in 0..32 {
        let e = vcx.new(|_| Plain);
        let counts = Rc::clone(&counts);
        subs.push(vcx.update(|_, cx| cx.observe(&e, move |_, _| counts.borrow_mut()[i] += 1)));
        entities.push(e);
    }
    for e in &entities {
        request(vcx, e.entity_id());
    }
    assert!(vcx.read_entity(&driver, |d, _| d.is_armed()));
    assert_eq!(frame(vcx), 1, "one on_next_frame for 32 requesters");
    let (requested, frames, _, _) = stats(&driver, vcx);
    assert_eq!((requested, frames), (1, 1));
    assert!(
        counts.borrow().iter().all(|&c| c == 1),
        "{:?}",
        counts.borrow()
    );
    assert!(!vcx.read_entity(&driver, |d, _| d.is_armed()));
    drop(subs);
}

#[gpui_kit::test]
fn driver_hidden_stops_arming(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    let id = vcx.new(|_| Plain).entity_id();
    request(vcx, id);
    assert_eq!(stats(&driver, vcx).0, 1);

    driver.update(vcx, |d, _| d.set_hidden(true));
    for _ in 0..5 {
        request(vcx, id);
    }
    assert_eq!(stats(&driver, vcx).0, 1, "no arm while hidden");
    advance(vcx, 1000);
    assert_eq!(stats(&driver, vcx).2, 0, "no stall while hidden");

    driver.update(vcx, |d, _| d.set_hidden(false));
    assert_eq!(stats(&driver, vcx).0, 1, "going visible does not arm");
    request(vcx, id);
    assert_eq!(stats(&driver, vcx).0, 2, "the next request arms");
    assert!(vcx.read_entity(&driver, |d, _| d.is_armed()));
}

#[gpui_kit::test]
fn driver_request_callback_non_view_id(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    let id = vcx.new(|_| Plain).entity_id();
    let first = Rc::new(Cell::new(0u32));
    let second = Rc::new(Cell::new(0u32));
    for counter in [Rc::clone(&first), Rc::clone(&second)] {
        vcx.update(|window, cx| {
            driver.update(cx, |d, cx| {
                d.request_callback(id, move |_, _| counter.set(counter.get() + 1), window, cx)
            })
        });
    }
    assert_eq!(stats(&driver, vcx).0, 1, "one frame armed");
    assert_eq!(frame(vcx), 1);
    assert_eq!((first.get(), second.get()), (0, 1));
    assert_eq!(frame(vcx), 0, "the callback does not run again");
    assert_eq!(second.get(), 1);

    // A callback that wants another frame requests again from inside the callback.
    let again = Rc::new(Cell::new(0u32));
    let counter = Rc::clone(&again);
    let weak = driver.downgrade();
    vcx.update(|window, cx| {
        driver.update(cx, |d, cx| {
            d.request_callback(
                id,
                move |window, cx| {
                    counter.set(counter.get() + 1);
                    let _ = weak.update(cx, |d, cx| d.request(id, window, cx));
                },
                window,
                cx,
            )
        })
    });
    assert_eq!(frame(vcx), 1);
    assert_eq!(again.get(), 1);
    assert!(vcx.read_entity(&driver, |d, _| d.is_armed()), "re-armed");
}

#[gpui_kit::test]
fn driver_counts_stall_after_250ms(cx: &mut TestAppContext) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let _guard = tracing::subscriber::set_default(Capture(Arc::clone(&warnings)));

    let (driver, vcx) = rig(cx);
    let id = vcx.new(|_| Plain).entity_id();
    request(vcx, id);
    advance(vcx, 200);
    assert_eq!(stats(&driver, vcx).2, 0);
    advance(vcx, 100);
    assert_eq!(stats(&driver, vcx).2, 1);
    advance(vcx, 1000);
    assert_eq!(stats(&driver, vcx).2, 1, "one count per stall");
    assert_eq!(stats(&driver, vcx).0, 1, "a stall never re-arms");
    assert_eq!(
        warnings.lock().expect("warnings lock").as_slice(),
        ["frame stall"]
    );

    // The late frame ends the stall; the loop stops once nothing is armed.
    assert_eq!(frame(vcx), 1);
    advance(vcx, 1000);
    assert_eq!(stats(&driver, vcx).2, 1);
}

#[gpui_kit::test]
fn driver_counts_missed_frames(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    driver.update(vcx, |d, _| d.stats.period_hint_ns = 6_944_444);
    let id = vcx.new(|_| Plain).entity_id();
    request(vcx, id);
    frame(vcx);
    request(vcx, id);
    advance(vcx, 5);
    frame(vcx);
    assert_eq!(stats(&driver, vcx).3, 0, "5 ms is on time");
    request(vcx, id);
    advance(vcx, 12);
    frame(vcx);
    assert_eq!(stats(&driver, vcx).3, 1);
    assert_eq!(
        vcx.read_entity(&driver, |d, _| d.stats.last_interval_ns),
        12_000_000
    );
    let taken = driver.update(vcx, |d, _| d.take_stats());
    assert_eq!((taken.frames, taken.missed), (3, 1));
    assert_eq!(stats(&driver, vcx), (0, 0, 0, 0));
    assert_eq!(
        vcx.read_entity(&driver, |d, _| d.stats.period_hint_ns),
        6_944_444
    );
}

#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn driver_paint_log_install_and_take(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    driver.update(vcx, |d, _| d.install_paint_log());
    let log = driver
        .update(vcx, |d, _| d.take_paint_log())
        .expect("installed");
    assert!(log.records.is_empty());
    assert_eq!(log.records.capacity(), 65_536);
    assert!(vcx.read_entity(&driver, |d, _| d.paint_log.is_none()));
}

/// Test subscriber that keeps the messages of `tasklens::frame` warnings.
struct Capture(Arc<Mutex<Vec<String>>>);

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        if meta.target() != "tasklens::frame" || *meta.level() != Level::WARN {
            return;
        }
        struct Message(String, bool);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                match f.name() {
                    "message" => self.0 = format!("{v:?}"),
                    "elapsed_ms" => self.1 = true,
                    _ => {}
                }
            }
        }
        let mut m = Message(String::new(), false);
        event.record(&mut m);
        assert!(m.1, "the warning carries elapsed_ms");
        self.0.lock().expect("warnings lock").push(m.0);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}
