//! `HostContext`: one host's engine connection as pages, dialogs and menus see it (ADR-018
//! "Hosts", D-038, UI-SHL-008).
//!
//! Every page, dialog and menu gets a `HostContext` when it is built and keeps it. It never reads
//! the engine, hub, demand, action tracker or shell event globals itself. A window or graph that
//! shows one host is bound to that host when it opens and never follows the selected host tab:
//! Properties windows, detached graphs and dialogs stay on the host they came from. Surfaces that
//! are not opened from a host (the tray, the mini window, notices) use [`HostContext::local`].
//!
//! The app-wide globals [`EngineGlobal`], [`GlobalHub`], [`GlobalDemand`],
//! [`GlobalActionTracker`] and [`GlobalShellEvents`] hold the local host. Only this module reads
//! them. With one host the window behaves exactly as it did when every part read them.
//!
//! In the app every part is set. A test may install only some of the globals; a part that was
//! missing when the context was taken is `None`, and code handles it the way it handled the
//! missing global.

use std::sync::Arc;

use gpui_kit::{App, Entity};
use tl_model::client::EngineClient;

use crate::action_tracker::{ActionTracker, GlobalActionTracker};
use crate::app::EngineGlobal;
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::hub::{GlobalHub, SnapshotHub};
use crate::window::main::{GlobalShellEvents, ShellEvents, ShellState};

/// One host's engine client, snapshot hub, demand aggregator, action tracker and shell event bus.
/// Cheap to clone: every part is a handle.
#[derive(Clone, Default)]
pub struct HostContext {
    client: Option<Arc<dyn EngineClient>>,
    hub: Option<Entity<SnapshotHub>>,
    demand: Option<Entity<DemandAggregator>>,
    tracker: Option<Entity<ActionTracker>>,
    events: Option<Entity<ShellEvents>>,
}

impl HostContext {
    /// The local host: the PC this window runs on.
    pub fn local(cx: &App) -> Self {
        Self {
            client: cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0)),
            hub: cx.try_global::<GlobalHub>().map(|g| g.0.clone()),
            demand: cx.try_global::<GlobalDemand>().map(|g| g.0.clone()),
            tracker: cx.try_global::<GlobalActionTracker>().map(|g| g.0.clone()),
            events: cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()),
        }
    }

    /// The host the main window shows. A dialog or menu opened from the main window by an
    /// app-level action takes it when it opens and keeps it. Without a main window, the local
    /// host.
    pub fn shown(cx: &App) -> Self {
        cx.try_global::<ShellState>()
            .and_then(|state| state.shown_host.clone())
            .unwrap_or_else(|| Self::local(cx))
    }

    /// The local host's hub, for code that acts on this PC itself (the keep-above pin, the
    /// main window's own KWin calls). Cheaper than [`HostContext::local`].
    pub fn local_hub(cx: &App) -> Option<&Entity<SnapshotHub>> {
        cx.try_global::<GlobalHub>().map(|g| &g.0)
    }

    /// The local host's engine client, for code that acts on this PC itself.
    pub fn local_client(cx: &App) -> Option<&Arc<dyn EngineClient>> {
        cx.try_global::<EngineGlobal>().map(|g| &g.0)
    }

    /// A context from parts, for code that builds its own hub and for tests.
    pub fn from_parts(
        client: Option<Arc<dyn EngineClient>>,
        hub: Option<Entity<SnapshotHub>>,
        demand: Option<Entity<DemandAggregator>>,
        tracker: Option<Entity<ActionTracker>>,
        events: Option<Entity<ShellEvents>>,
    ) -> Self {
        Self {
            client,
            hub,
            demand,
            tracker,
            events,
        }
    }

    /// The host's engine client.
    pub fn client(&self) -> Option<&Arc<dyn EngineClient>> {
        self.client.as_ref()
    }

    /// The host's snapshot hub.
    pub fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        self.hub.as_ref()
    }

    /// The host's demand aggregator.
    pub fn demand(&self) -> Option<&Entity<DemandAggregator>> {
        self.demand.as_ref()
    }

    /// The host's action tracker.
    pub fn tracker(&self) -> Option<&Entity<ActionTracker>> {
        self.tracker.as_ref()
    }

    /// The host's shell event bus: filter text, the memory group chip and pause.
    pub fn events(&self) -> Option<&Entity<ShellEvents>> {
        self.events.as_ref()
    }
}
