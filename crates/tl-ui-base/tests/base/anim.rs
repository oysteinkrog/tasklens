//! `AnimationDriver` tests with `TestAppContext` (E19, ADR-009, ADR-010, ADR-014, R-209). The test
//! platform delivers no frames; `Window::simulate_next_frame` stands in for the compositor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{
    AppContext, Context, Entity, EntityId, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use tl_ui_base::anim::{AnimationDriver, Pacing};
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

thread_local! {
    static CPU_CALLS: Cell<u64> = const { Cell::new(0) };
}

/// A thread CPU clock that counts its calls and moves 1 ms per call.
fn counting_clock() -> u64 {
    CPU_CALLS.with(|c| {
        c.set(c.get() + 1);
        c.get() * 1_000_000
    })
}

#[gpui_kit::test]
fn driver_frame_cpu_only_while_sampling(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    driver.update(vcx, |d, _| d.set_cpu_clock(counting_clock));
    let plain = vcx.new(|_| Plain).entity_id();
    let run_frames = |vcx: &mut VisualTestContext, n: usize| {
        for _ in 0..n {
            request(vcx, plain);
            frame(vcx);
        }
    };

    run_frames(vcx, 3);
    assert_eq!(
        CPU_CALLS.with(Cell::get),
        0,
        "no clock call while not sampling"
    );

    driver.update(vcx, |d, _| d.stats.sampling = true);
    run_frames(vcx, 3);
    assert_eq!(CPU_CALLS.with(Cell::get), 3);
    let cpu: Vec<u64> = vcx.read_entity(&driver, |d, _| {
        d.stats.samples.iter().map(|s| s.thread_cpu_ns).collect()
    });
    // The first sampled frame has no previous clock reading.
    assert_eq!(cpu, [0, 1_000_000, 1_000_000]);

    driver.update(vcx, |d, _| d.stats.sampling = false);
    run_frames(vcx, 2);
    assert_eq!(CPU_CALLS.with(Cell::get), 3, "stops with sampling");
}

#[gpui_kit::test]
fn driver_frame_cpu_from_the_real_clock(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    let plain = vcx.new(|_| Plain).entity_id();
    driver.update(vcx, |d, _| d.stats.sampling = true);
    for _ in 0..3 {
        request(vcx, plain);
        frame(vcx);
        // Work between frames, as layout and paint would be.
        let start = std::time::Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(2) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
    }
    let last = vcx.read_entity(&driver, |d, _| {
        d.stats.samples.last().map(|s| s.thread_cpu_ns)
    });
    assert!(last.is_some_and(|ns| ns > 0), "{last:?}");
}

// Pixel pacing (wlf.31). A chart that is notified paints and requests again; `Paced` stands in
// for it: the test requests again for every requester whose notify count went up.

const MS: u64 = 1_000_000;

fn request_paced(vcx: &mut VisualTestContext, id: EntityId, step_ms: u64) {
    vcx.update(|window, cx| {
        AnimationDriver::global(window, cx)
            .update(cx, |d, cx| d.request_paced(id, step_ms * MS, window, cx))
    });
}

fn set_pacing(driver: &Entity<AnimationDriver>, vcx: &mut VisualTestContext, pacing: Pacing) {
    driver.update(vcx, |d, _| d.set_pacing(pacing));
}

fn armed(driver: &Entity<AnimationDriver>, vcx: &mut VisualTestContext) -> bool {
    vcx.read_entity(driver, |d, _| d.is_armed())
}

fn notify_frames(driver: &Entity<AnimationDriver>, vcx: &mut VisualTestContext) -> u64 {
    vcx.read_entity(driver, |d, _| d.stats.notify_frames)
}

/// Paced requesters with their step and a notify counter each.
struct Paced {
    ids: Vec<(EntityId, u64)>,
    counts: Rc<RefCell<Vec<u32>>>,
    seen: Vec<u32>,
    _subs: Vec<gpui_kit::Subscription>,
    _entities: Vec<Entity<Plain>>,
}

impl Paced {
    fn new(vcx: &mut VisualTestContext, steps_ms: &[u64]) -> Self {
        let counts = Rc::new(RefCell::new(vec![0u32; steps_ms.len()]));
        let mut ids = Vec::new();
        let mut subs = Vec::new();
        let mut entities = Vec::new();
        for (i, &step) in steps_ms.iter().enumerate() {
            let e = vcx.new(|_| Plain);
            let counts = Rc::clone(&counts);
            subs.push(vcx.update(|_, cx| cx.observe(&e, move |_, _| counts.borrow_mut()[i] += 1)));
            ids.push((e.entity_id(), step));
            entities.push(e);
        }
        for &(id, step) in &ids {
            request_paced(vcx, id, step);
        }
        Self {
            ids,
            seen: vec![0; steps_ms.len()],
            counts,
            _subs: subs,
            _entities: entities,
        }
    }

    /// Requests again for every requester notified since the last call, as its paint would.
    fn repaint(&mut self, vcx: &mut VisualTestContext) {
        let counts = self.counts.borrow().clone();
        for (i, &(id, step)) in self.ids.iter().enumerate() {
            if counts[i] > self.seen[i] {
                request_paced(vcx, id, step);
            }
        }
        self.seen = counts;
    }

    fn notified(&self) -> Vec<u32> {
        self.counts.borrow().clone()
    }
}

/// Runs `total_ms` of a compositor that offers a frame every `vsync_ms` while one is armed.
/// Returns the frames delivered.
fn run_vsync(
    driver: &Entity<AnimationDriver>,
    vcx: &mut VisualTestContext,
    paced: &mut Paced,
    vsync_ms: u64,
    total_ms: u64,
) -> u32 {
    let mut frames = 0;
    for t in 1..=total_ms {
        advance(vcx, 1);
        if t % vsync_ms == 0 && armed(driver, vcx) {
            frames += u32::try_from(frame(vcx)).unwrap_or(0);
            paced.repaint(vcx);
        }
    }
    frames
}

#[test]
fn pacing_parse() {
    use std::ffi::OsStr;
    assert_eq!(Pacing::parse(Some(OsStr::new("timer"))), Pacing::Timer);
    assert_eq!(Pacing::parse(Some(OsStr::new("skip"))), Pacing::Skip);
    assert_eq!(Pacing::parse(Some(OsStr::new("on"))), Pacing::Off);
    assert_eq!(Pacing::parse(None), Pacing::Off);
    assert_eq!(Pacing::Skip.as_str(), "skip");
}

#[gpui_kit::test]
fn driver_pacing_off_request_paced_is_request(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    assert_eq!(vcx.read_entity(&driver, |d, _| d.pacing()), Pacing::Off);
    let mut paced = Paced::new(vcx, &[30]);
    assert!(armed(&driver, vcx), "armed at once, as request() does");
    assert!(!vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    let frames = run_vsync(&driver, vcx, &mut paced, 7, 70);
    assert_eq!(frames, 10, "a frame every vsync");
    assert_eq!(paced.notified(), [10]);
    assert_eq!(vcx.read_entity(&driver, |d, _| d.stats.min_step_ns), 0);
}

#[gpui_kit::test]
fn driver_paced_skip_notifies_once_per_step(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Skip);
    let mut paced = Paced::new(vcx, &[30]);
    assert!(armed(&driver, vcx), "skip keeps one frame armed per vsync");
    let frames = run_vsync(&driver, vcx, &mut paced, 7, 301);
    // Due at 30 ms, notified by the first frame at or after 26 ms: every 4th frame, 28 ms.
    assert_eq!(frames, 43, "a frame callback every vsync");
    assert_eq!(paced.notified(), [10]);
    assert_eq!(notify_frames(&driver, vcx), 10);
    assert_eq!(
        vcx.read_entity(&driver, |d, _| d.stats.min_step_ns),
        30 * MS
    );
}

#[gpui_kit::test]
fn driver_paced_timer_arms_no_frame_before_step(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Timer);
    let paced = Paced::new(vcx, &[30]);
    assert!(!armed(&driver, vcx), "no frame until the step");
    assert!(vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    advance(vcx, 25);
    assert!(!armed(&driver, vcx), "25 ms: still before the step");
    advance(vcx, 2);
    assert!(armed(&driver, vcx), "27 ms: within 4 ms of the step");
    assert_eq!(frame(vcx), 1);
    assert_eq!(paced.notified(), [1]);
    assert_eq!(notify_frames(&driver, vcx), 1);
    assert!(!vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
}

#[gpui_kit::test]
fn driver_paced_fastest_chart_wins(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Timer);
    let mut paced = Paced::new(vcx, &[30, 300]);
    let frames = run_vsync(&driver, vcx, &mut paced, 7, 300);
    // The 30 ms chart draws every 28 ms; the 300 ms chart is due at 296 ms, after the last vsync.
    assert_eq!(paced.notified(), [10, 0]);
    assert_eq!(frames, 10, "no frame that notifies nothing");
    assert_eq!(notify_frames(&driver, vcx), 10);
    assert_eq!(
        vcx.read_entity(&driver, |d, _| d.stats.min_step_ns),
        30 * MS
    );
}

/// Charts that paint in one frame request a little apart; they still draw in one frame.
#[gpui_kit::test]
fn driver_paced_merges_charts_painted_together(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Timer);
    let paced = Paced::new(vcx, &[30, 30, 30]);
    // Due at 30, 30.7 and 31.5 ms, as three paints a few hundred microseconds apart give.
    for (i, extra_us) in [(1, 700), (2, 1_500)] {
        let (id, _) = paced.ids[i];
        vcx.update(|window, cx| {
            AnimationDriver::global(window, cx).update(cx, |d, cx| {
                d.request_paced(id, 30 * MS + extra_us * 1_000, window, cx)
            })
        });
    }
    advance(vcx, 26);
    assert!(armed(&driver, vcx), "armed 4 ms before the first step");
    assert_eq!(frame(vcx), 1);
    assert_eq!(paced.notified(), [1, 1, 1], "one frame draws all three");
    assert_eq!(notify_frames(&driver, vcx), 1);
}

#[gpui_kit::test]
fn driver_unpaced_request_keeps_vsync_with_pacing_on(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Timer);
    let e = vcx.new(|_| Plain);
    let hits = Rc::new(Cell::new(0u32));
    let counter = Rc::clone(&hits);
    let _sub = vcx.update(|_, cx| cx.observe(&e, move |_, _| counter.set(counter.get() + 1)));
    request(vcx, e.entity_id());
    assert!(armed(&driver, vcx), "request() arms at once");
    assert!(!vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    assert_eq!(frame(vcx), 1);
    assert_eq!(hits.get(), 1);
    assert_eq!(notify_frames(&driver, vcx), 1);
}

#[gpui_kit::test]
fn driver_paced_counts_no_missed(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    driver.update(vcx, |d, _| d.stats.period_hint_ns = 6_944_444);
    set_pacing(&driver, vcx, Pacing::Timer);
    let mut paced = Paced::new(vcx, &[30]);
    run_vsync(&driver, vcx, &mut paced, 7, 300);
    let (_, frames, stalls, missed) = stats(&driver, vcx);
    assert_eq!(frames, 10);
    assert_eq!(
        (stalls, missed),
        (0, 0),
        "28 ms intervals are not missed frames"
    );
}

#[gpui_kit::test]
fn driver_paced_hidden_arms_no_timer(cx: &mut TestAppContext) {
    let (driver, vcx) = rig(cx);
    set_pacing(&driver, vcx, Pacing::Timer);
    let paced = Paced::new(vcx, &[30]);
    assert!(vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    driver.update(vcx, |d, _| d.set_hidden(true));
    assert!(!vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    let (id, _) = paced.ids[0];
    request_paced(vcx, id, 30);
    assert!(!vcx.read_entity(&driver, |d, _| d.has_pixel_timer()));
    advance(vcx, 100);
    assert!(!armed(&driver, vcx), "no frame while hidden");
    assert_eq!(stats(&driver, vcx).2, 0, "no stall while hidden");

    driver.update(vcx, |d, _| d.set_hidden(false));
    request_paced(vcx, id, 30);
    advance(vcx, 27);
    assert!(armed(&driver, vcx), "the next request paces again");
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
