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
//!
//! [`HostRegistry`] holds one context per enabled `[[hosts]]` entry (tasklens-q4x76.27). Each
//! remote host gets its own hub, demand aggregator, action tracker and shell event bus, built
//! once and kept while the entry exists, so a window bound to it keeps working after the main
//! window closes or shows another host.

use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, Global};
use tl_model::client::EngineClient;
use tl_model::settings::HostEntry;
use tracing::{info, warn};

use crate::action_tracker::{ActionTracker, GlobalActionTracker};
use crate::app::{AppEnv, EngineGlobal};
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::hosts_api::HostLinkStatus;
use crate::hub::{GlobalHub, SnapshotHub};
use crate::settings_global::TlSettings;
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
    /// The host's name when it is a remote host; `None` for the local host.
    remote: Option<Arc<str>>,
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
            remote: None,
        }
    }

    /// A remote host named `name` on `client`: a new hub, demand aggregator, action tracker and
    /// shell event bus. The tracker's action ids start at `first_action_id`, so no two trackers
    /// hand out the same id and the app-wide toast buttons reach only the tracker that sent it.
    pub fn remote(
        name: Arc<str>,
        client: Arc<dyn EngineClient>,
        first_action_id: u64,
        cx: &mut App,
    ) -> Self {
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
        let tracker = cx.new(|cx| {
            let tracker = ActionTracker::new(Arc::clone(&client), Some(hub.clone()), cx);
            tracker.start_ids_at(first_action_id);
            tracker
        });
        let events = cx.new(|_| ShellEvents);
        Self {
            client: Some(client),
            hub: Some(hub),
            demand: Some(demand),
            tracker: Some(tracker),
            events: Some(events),
            remote: Some(name),
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
            remote: None,
        }
    }

    /// The host's name when it is a remote host; `None` for the local host.
    pub fn remote_name(&self) -> Option<&Arc<str>> {
        self.remote.as_ref()
    }

    /// True for a host other than the PC this window runs on.
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// `title` as a window shows it: unchanged for the local host, `"<title> - <host>"` for a
    /// remote one (leader decision on tasklens-q4x76.11, Q2).
    pub fn titled(&self, title: &str) -> String {
        with_host_name(title, self.remote.as_deref())
    }

    /// Both contexts are the same host: the same hub, or with no hub on either, the same client.
    pub fn same_host(&self, other: &HostContext) -> bool {
        match (&self.hub, &other.hub) {
            (Some(a), Some(b)) => a == b,
            (None, None) => match (&self.client, &other.client) {
                (Some(a), Some(b)) => same_client(a, b),
                (None, None) => self.remote == other.remote,
                _ => false,
            },
            _ => false,
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

/// `title` with `" - <host>"` when `remote` names a remote host.
pub fn with_host_name(title: &str, remote: Option<&str>) -> String {
    match remote {
        Some(name) => format!("{title} - {name}"),
        None => title.to_owned(),
    }
}

/// The two handles point at the same client. Compares the data pointers only.
fn same_client(a: &Arc<dyn EngineClient>, b: &Arc<dyn EngineClient>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

/// One host tab: the host's name and its context (UI-SHL-008).
#[derive(Clone)]
pub struct HostTab {
    pub name: Arc<str>,
    pub host: HostContext,
}

/// The first action id of the `n`th remote host's tracker. The local tracker counts from 1.
const fn first_action_id(n: u64) -> u64 {
    n << 48
}

/// One context per enabled `[[hosts]]` entry, in the order of the entries (UI-SHL-008). The
/// local host is not stored: [`HostRegistry::tabs`] puts it first. Clients come from
/// `AppEnv::hosts`; without it (the rescue window, most tests) there is only the local host.
#[derive(Default)]
pub struct HostRegistry {
    remote: Vec<HostTab>,
    /// The enabled entries of the last sync.
    synced: Option<Vec<HostEntry>>,
    /// Remote contexts made so far; spaces their action ids apart.
    made: u64,
    local_name: Option<Arc<str>>,
}

impl Global for HostRegistry {}

/// This PC's host name from the kernel, or "localhost" when it cannot be read.
fn read_local_name() -> Arc<str> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map_or_else(|| Arc::from("localhost"), Arc::from)
}

impl HostRegistry {
    /// The local host's tab name: this PC's host name, read once.
    pub fn local_name(cx: &mut App) -> Arc<str> {
        let reg = cx.default_global::<HostRegistry>();
        Arc::clone(reg.local_name.get_or_insert_with(read_local_name))
    }

    /// Every host tab, the local host first.
    pub fn tabs(cx: &mut App) -> Vec<HostTab> {
        let name = Self::local_name(cx);
        let mut tabs = vec![HostTab {
            name,
            host: HostContext::local(cx),
        }];
        tabs.extend(cx.global::<HostRegistry>().remote.iter().cloned());
        tabs
    }

    /// The name of the remote host whose hub is `hub`; `None` for the local host and an
    /// unknown hub. For windows that are opened with a hub alone.
    pub fn remote_name_of(hub: &Entity<SnapshotHub>, cx: &App) -> Option<Arc<str>> {
        cx.try_global::<HostRegistry>()?
            .remote
            .iter()
            .find(|t| t.host.hub() == Some(hub))
            .map(|t| Arc::clone(&t.name))
    }

    /// The link status of the named remote host. The local host and a host the manager does
    /// not know count as connected and failed.
    pub fn status(host: &HostContext, cx: &App) -> HostLinkStatus {
        let Some(name) = host.remote_name() else {
            return HostLinkStatus::Connected;
        };
        let status = cx
            .try_global::<AppEnv>()
            .and_then(|env| env.hosts.as_ref())
            .and_then(|hosts| (hosts.status)(name));
        status.unwrap_or_else(|| HostLinkStatus::Failed(Arc::from("not started")))
    }

    /// Makes the remote hosts match the enabled `[[hosts]]` entries: keeps a host whose client
    /// is unchanged, builds a context for a new entry or one whose client was replaced, and
    /// stops the hosts that are gone. An entry the host manager refuses is logged and left out.
    /// Returns true when the list of hosts changed.
    pub fn sync(cx: &mut App) -> bool {
        let wanted: Vec<HostEntry> = if cx.has_global::<TlSettings>() {
            TlSettings::settings(cx)
                .hosts
                .iter()
                .filter(|e| e.enabled)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if cx.default_global::<HostRegistry>().synced.as_ref() == Some(&wanted) {
            return false;
        }
        let api = cx
            .try_global::<AppEnv>()
            .and_then(|env| env.hosts.as_ref())
            .map(|h| (Arc::clone(&h.connect), Arc::clone(&h.remove)));
        let old = std::mem::take(&mut cx.global_mut::<HostRegistry>().remote);
        let mut made = cx.global::<HostRegistry>().made;
        let mut remote = Vec::with_capacity(wanted.len());
        if let Some((connect, _)) = &api {
            for entry in &wanted {
                let client = match connect(entry) {
                    Ok(client) => client,
                    Err(why) => {
                        warn!(target: "tasklens::ui", host = %entry.name, "host left out: {why}");
                        continue;
                    }
                };
                let kept = old.iter().find(|t| {
                    *t.name == *entry.name
                        && t.host.client().is_some_and(|c| same_client(c, &client))
                });
                let tab = match kept {
                    Some(tab) => tab.clone(),
                    None => {
                        made += 1;
                        let name: Arc<str> = Arc::from(entry.name.as_str());
                        info!(target: "tasklens::ui", host = %name, "host context built");
                        let host = HostContext::remote(
                            Arc::clone(&name),
                            client,
                            first_action_id(made),
                            cx,
                        );
                        HostTab { name, host }
                    }
                };
                remote.push(tab);
            }
        }
        if let Some((_, remove)) = &api {
            for gone in old
                .iter()
                .filter(|t| !remote.iter().any(|r| r.name == t.name))
            {
                remove(&gone.name);
            }
        }
        let changed = old.len() != remote.len()
            || old
                .iter()
                .zip(&remote)
                .any(|(a, b)| a.name != b.name || !a.host.same_host(&b.host));
        let reg = cx.global_mut::<HostRegistry>();
        reg.remote = remote;
        reg.synced = Some(wanted);
        reg.made = made;
        changed
    }

    /// Replaces the named remote host's client with a fresh connection and gives the tab a new
    /// context on it (the tab context menu, UI-SHL-008). The old context's windows keep their
    /// old client. Returns false when there is no such remote host or the manager refuses.
    pub fn reconnect(name: &str, cx: &mut App) -> bool {
        let Some(reconnect) = cx
            .try_global::<AppEnv>()
            .and_then(|env| env.hosts.as_ref())
            .map(|h| Arc::clone(&h.reconnect))
        else {
            return false;
        };
        let Some(ix) = cx
            .default_global::<HostRegistry>()
            .remote
            .iter()
            .position(|t| &*t.name == name)
        else {
            return false;
        };
        let client = match reconnect(name) {
            Ok(client) => client,
            Err(why) => {
                warn!(target: "tasklens::ui", host = %name, "reconnect refused: {why}");
                return false;
            }
        };
        let reg = cx.global_mut::<HostRegistry>();
        reg.made += 1;
        let made = reg.made;
        let name: Arc<str> = Arc::clone(&reg.remote[ix].name);
        let host = HostContext::remote(Arc::clone(&name), client, first_action_id(made), cx);
        cx.global_mut::<HostRegistry>().remote[ix] = HostTab { name, host };
        true
    }
}
