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
                // Load after receive (ADR-005).
                let snap = receiver.latest();
                if this.update(cx, |hub, cx| hub.install(snap, cx)).is_err() {
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

        let current = client.latest();
        let mut live_sensors = FxHashSet::default();
        note_live_sensors(&mut live_sensors, &current);
        Self {
            current,
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

    /// Installs `snap` if its sequence is newer, then emits `SnapshotChanged` when any component
    /// version differs from the installed snapshot. Never notifies views.
    pub fn install(&mut self, snap: Arc<Snapshot>, cx: &mut Context<Self>) {
        if snap.sequence <= self.current.sequence {
            return;
        }
        note_live_sensors(&mut self.live_sensors, &snap);
        let mut changed_series = SmallVec::new();
        let mask = snap.changed_since(&self.current, &mut changed_series);
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

/// The app's hub, set once by `TasklensUi::init`.
pub struct GlobalHub(pub Entity<SnapshotHub>);

impl Global for GlobalHub {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fake_engine::FakeEngine;
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
