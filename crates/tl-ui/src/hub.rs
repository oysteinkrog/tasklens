//! `SnapshotHub`: receives engine snapshots, works out which components changed and tells the
//! subscribed views (ADR-005 "UI receiver"). It also detects when the compositor has parked the
//! main window and reports it as hidden (ADR-010 "Hidden state", ADR-008 "Hidden detection").

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AsyncApp, Context, Entity, EventEmitter, Global, WeakEntity};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, SeriesId};
use tl_model::snapshot::Snapshot;
use tl_model::system::SensorKind;
use tl_model::validity::Validity;

pub use tl_model::snapshot::ComponentMask;

/// Calls `window.refresh()` every 1 s while hidden. Stays false until phase 0 settles U-3 (does
/// KWin cause a render when a minimized window is restored). A separate bead flips it if not.
pub const REFRESH_WHILE_HIDDEN: bool = false;

/// Period of the hidden-state check.
const HIDDEN_CHECK_PERIOD: Duration = Duration::from_millis(500);
/// A change pending longer than this without a render means the window is hidden.
const HIDDEN_AFTER: Duration = Duration::from_millis(1000);

/// Emitted after a newer snapshot is installed and at least one component changed.
#[derive(Clone, Debug)]
pub struct SnapshotChanged {
    pub mask: ComponentMask,
    /// Series ids that are new or whose `SeriesView.version` changed, in `SeriesId` order.
    pub changed_series: SmallVec<[SeriesId; 64]>,
}

/// Emitted when the hidden state flips.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HiddenChanged {
    pub hidden: bool,
}

/// Chips report -128 °C or exactly 0 °C for a pin with nothing connected.
pub(crate) fn is_dead_reading(celsius: f32) -> bool {
    celsius <= -128.0 || celsius == 0.0
}

/// Adds every temperature sensor with a valid, real reading in `snap` to `live`.
fn note_live_sensors(live: &mut FxHashSet<DeviceIdx>, snap: &Snapshot) {
    for (idx, r) in snap.system.sensors.iter() {
        if r.kind == SensorKind::Temperature
            && r.value.validity == Validity::Valid
            && !is_dead_reading(r.value.value)
        {
            live.insert(*idx);
        }
    }
}

/// The one entity per app that holds the installed snapshot. Holds exactly one `Arc<Snapshot>`.
pub struct SnapshotHub {
    client: Arc<dyn EngineClient>,
    current: Arc<Snapshot>,
    /// `EngineClient::generation` of `current`.
    generation: u64,
    /// Set when a `SnapshotChanged` is emitted and no render has been noted since.
    pending_notify_since: Option<Instant>,
    last_render: Option<Instant>,
    hidden: bool,
    /// U-3 fallback switch, from `REFRESH_WHILE_HIDDEN`.
    refresh_while_hidden: bool,
    /// Temperature sensors that have read a real value (not -128 °C or 0 °C) in any installed
    /// snapshot since the app started. Pages use it to keep a sensor whose history has expired.
    live_sensors: FxHashSet<DeviceIdx>,
}

impl EventEmitter<SnapshotChanged> for SnapshotHub {}
impl EventEmitter<HiddenChanged> for SnapshotHub {}

impl SnapshotHub {
    /// Installs `client.latest()` and spawns the receive task and the 500 ms hidden-state timer.
    pub fn new(client: Arc<dyn EngineClient>, cx: &mut Context<Self>) -> Self {
        let wake = client.wake();
        let receiver = Arc::clone(&client);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while wake.recv().await.is_ok() {
                // Load after receive (ADR-005). One load gives the snapshot with its own
                // generation, so old data is never installed under a new generation.
                let (generation, snap) = receiver.latest_with_generation();
                if this
                    .update(cx, |hub, cx| hub.install(generation, snap, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor().timer(HIDDEN_CHECK_PERIOD).await;
                tick = tick.wrapping_add(1);
                if this
                    .update(cx, |hub, cx| hub.check_hidden(tick, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let (generation, current) = client.latest_with_generation();
        let mut live_sensors = FxHashSet::default();
        note_live_sensors(&mut live_sensors, &current);
        Self {
            current,
            generation,
            client,
            pending_notify_since: None,
            last_render: None,
            hidden: false,
            refresh_while_hidden: REFRESH_WHILE_HIDDEN,
            live_sensors,
        }
    }

    /// The installed snapshot. Callers must not keep it beyond one render (ADR-005 retention).
    pub fn current(&self) -> &Arc<Snapshot> {
        &self.current
    }

    /// Temperature sensors that have had a real reading since the app started.
    pub fn live_sensors(&self) -> &FxHashSet<DeviceIdx> {
        &self.live_sensors
    }

    /// Installs `snap` if it is newer by `(generation, sequence)`, then emits `SnapshotChanged`
    /// when any component version differs from the installed snapshot. A higher `generation` (a
    /// reconnect, another engine instance or a switch to another engine, ADR-018) starts the
    /// sequence over: `snap` is installed whatever its sequence, and every component and every
    /// series counts as changed, since versions from two generations cannot be compared. A
    /// snapshot from a lower generation is never installed. Never notifies views.
    pub fn install(&mut self, generation: u64, snap: Arc<Snapshot>, cx: &mut Context<Self>) {
        let new_generation = generation > self.generation;
        if generation < self.generation
            || (!new_generation && snap.sequence <= self.current.sequence)
        {
            return;
        }
        note_live_sensors(&mut self.live_sensors, &snap);
        let mut changed_series = SmallVec::new();
        let mask = if new_generation {
            self.generation = generation;
            let blank = Snapshot::empty(snap.boot_id, snap.epoch);
            snap.changed_since(&blank, &mut changed_series) | ComponentMask::ALL
        } else {
            snap.changed_since(&self.current, &mut changed_series)
        };
        self.current = snap;
        if mask == ComponentMask::NONE {
            return;
        }
        if self.pending_notify_since.is_none() {
            self.pending_notify_since = Some(cx.background_executor().now());
        }
        cx.emit(SnapshotChanged {
            mask,
            changed_series,
        });
    }

    /// Called by `MainWindow::render`. Clears the pending change and makes the hub visible.
    pub fn note_render(&mut self, cx: &mut Context<Self>) {
        self.pending_notify_since = None;
        self.last_render = Some(cx.background_executor().now());
        if self.hidden {
            self.hidden = false;
            cx.emit(HiddenChanged { hidden: false });
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// When `note_render` was last called.
    pub fn last_render(&self) -> Option<Instant> {
        self.last_render
    }

    pub fn client(&self) -> &Arc<dyn EngineClient> {
        &self.client
    }

    /// `EngineClient::generation` of the installed snapshot.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// One timer tick. Goes hidden when a change has waited more than 1000 ms for a render.
    fn check_hidden(&mut self, tick: u32, cx: &mut Context<Self>) {
        if self.hidden {
            // Every second tick is 1 s.
            if self.refresh_while_hidden && tick.is_multiple_of(2) {
                cx.refresh_windows();
            }
            return;
        }
        let now = cx.background_executor().now();
        if let Some(since) = self.pending_notify_since
            && now.saturating_duration_since(since) > HIDDEN_AFTER
        {
            self.hidden = true;
            cx.emit(HiddenChanged { hidden: true });
        }
    }
}

/// The local host's hub, set once by `TasklensUi::init`. Read only through
/// [`crate::host::HostContext`].
pub struct GlobalHub(pub Entity<SnapshotHub>);

impl Global for GlobalHub {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::testing::fake_engine::{FakeEngine, empty_snapshot};
    use gpui_kit::{AppContext, TestAppContext};

    #[test]
    fn refresh_while_hidden_starts_off() {
        const { assert!(!REFRESH_WHILE_HIDDEN) };
    }

    #[gpui_kit::test]
    fn pending_since_keeps_first_change(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        engine.publish_with(|s| Arc::make_mut(&mut s.system).header.version += 1);
        cx.run_until_parked();
        let first = hub.read_with(cx, |hub, _| hub.pending_notify_since);
        assert!(first.is_some());

        cx.executor().advance_clock(Duration::from_millis(300));
        engine.publish_with(|s| Arc::make_mut(&mut s.system).header.version += 1);
        cx.run_until_parked();
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 2);
            assert_eq!(hub.pending_notify_since, first);
        });

        hub.update(cx, |hub, cx| hub.note_render(cx));
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.pending_notify_since, None);
            assert!(hub.last_render().is_some());
        });
    }

    /// Counts `SnapshotChanged` events and keeps the last mask.
    fn watch(hub: &Entity<SnapshotHub>, cx: &mut TestAppContext) -> Arc<Mutex<Vec<ComponentMask>>> {
        let masks = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&masks);
        cx.update(|cx| {
            cx.subscribe(hub, move |_, ev: &SnapshotChanged, _| {
                seen.lock().unwrap().push(ev.mask);
            })
            .detach();
        });
        masks
    }

    #[gpui_kit::test]
    fn engine_restart_shows_fresh_data(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let masks = watch(&hub, cx);
        engine.publish_with(|s| {
            Arc::make_mut(&mut s.system).header.version = 3;
            Arc::make_mut(&mut s.system).boot_time_s = 100;
        });
        cx.run_until_parked();
        assert_eq!(*masks.lock().unwrap(), [ComponentMask::SYSTEM]);

        // The new engine's counters start over and reach the same version with other data.
        let mut fresh = empty_snapshot();
        fresh.sequence = 2;
        Arc::make_mut(&mut fresh.system).header.version = 3;
        Arc::make_mut(&mut fresh.system).boot_time_s = 200;
        engine.restart(fresh);
        cx.run_until_parked();
        assert_eq!(masks.lock().unwrap().last(), Some(&ComponentMask::ALL));
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().system.boot_time_s, 200);
        });

        // Within the new generation versions compare again.
        engine.publish_with(|_| {});
        cx.run_until_parked();
        assert_eq!(masks.lock().unwrap().len(), 2);
    }

    #[gpui_kit::test]
    fn new_generation_reinstalls_same_sequence(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let masks = watch(&hub, cx);
        // Within one generation the hub installs by version.
        let mut first = empty_snapshot();
        first.sequence = 1;
        let generation = engine.generation();
        hub.update(cx, |hub, cx| hub.install(generation, Arc::new(first), cx));
        assert!(masks.lock().unwrap().is_empty());
        // A new generation installs a snapshot with the same sequence again, as all changed.
        engine.bump_generation();
        let snap = engine.latest();
        let mut same = Snapshot::clone(&snap);
        same.sequence = 1;
        let generation = engine.generation();
        hub.update(cx, |hub, cx| hub.install(generation, Arc::new(same), cx));
        assert_eq!(*masks.lock().unwrap(), [ComponentMask::ALL]);
        // Within one generation an older sequence is never installed.
        hub.update(cx, |hub, cx| hub.install(generation, snap, cx));
        assert_eq!(masks.lock().unwrap().len(), 1);
    }

    /// A switch from the in-process engine to the engine service (ADR-018 Lifetime): the
    /// service's snapshots number from 1 on a higher generation and are shown, and a late
    /// snapshot of the old engine is dropped.
    #[gpui_kit::test]
    fn higher_generation_starts_sequence_over(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let masks = watch(&hub, cx);
        let with = |sequence: u64, boot_time_s: i64| {
            let mut snap = empty_snapshot();
            snap.sequence = sequence;
            Arc::make_mut(&mut snap.system).header.version = 1;
            Arc::make_mut(&mut snap.system).boot_time_s = boot_time_s;
            Arc::new(snap)
        };
        hub.update(cx, |hub, cx| hub.install(0, with(500, 100), cx));
        assert_eq!(masks.lock().unwrap().len(), 1);

        hub.update(cx, |hub, cx| hub.install(1, with(1, 200), cx));
        assert_eq!(masks.lock().unwrap().last(), Some(&ComponentMask::ALL));
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 1);
            assert_eq!(hub.current().system.boot_time_s, 200);
        });

        // The old engine's last snapshot arrives late: dropped, though its sequence is higher.
        hub.update(cx, |hub, cx| hub.install(0, with(501, 100), cx));
        assert_eq!(masks.lock().unwrap().len(), 2);
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().system.boot_time_s, 200);
        });

        // The new generation goes on by sequence.
        hub.update(cx, |hub, cx| hub.install(1, with(1, 300), cx));
        assert_eq!(masks.lock().unwrap().len(), 2);
        hub.update(cx, |hub, cx| hub.install(1, with(2, 300), cx));
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 2);
        });
    }

    #[gpui_kit::test]
    fn unchanged_versions_emit_nothing(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let events = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = Arc::clone(&events);
        cx.update(|cx| {
            cx.subscribe(&hub, move |_, _: &SnapshotChanged, _| {
                seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .detach();
        });
        engine.publish_with(|_| {});
        cx.run_until_parked();
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 1);
            assert_eq!(hub.pending_notify_since, None);
        });
        assert_eq!(events.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
}
