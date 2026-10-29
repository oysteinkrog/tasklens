//! The reducer: the single owner of engine state and the only publisher (ADR-004, ADR-005).
//!
//! `run` receives one `EngineMsg`, drains up to `DRAIN_MAX` messages in total with `try_recv`,
//! applies them all, then publishes once. Every component is an `Arc` that is replaced only when
//! its content changed, so a CPU-only publish reuses every other `Arc` (ADR-003). The
//! `RecordSink` hook is `None` in P1 and P2 (ADR-012).
//!
//! Privacy: never log record contents from here. Debug logs carry counts only (ADR-015).

// Chain-edit lines added by later beads: apply (E09-007, 8q0.9); suspend (E09-008, 8q0.10).
pub mod app_detail;
pub mod apply;
pub mod devices;
pub mod health;
pub mod mem_groups;
mod power;
pub mod process_series;
pub mod processes;
pub mod series;
pub mod suspend;
pub mod sys_table;
pub mod table;

use std::sync::Arc;

use arc_swap::ArcSwap;
use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use rustc_hash::{FxHashMap, FxHashSet};
use tl_model::actions::{ActionEvent, ActionId, ActionRequest};
use tl_model::command::SetupStep;
use tl_model::demand::Demand;
use tl_model::details::{
    AffinityInfo, DetailTab, DetailTarget, DetailsSnapshot, HandleSearchState, TabPayload,
    TargetDetail,
};
use tl_model::devices::DeviceCatalog;
use tl_model::gpu_proto::GpuApiVersions;
use tl_model::health::{HealthSnapshot, SourceId};
use tl_model::history::{HistoryView, RecordSink};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcFlags;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, DeviceDetailsSnapshot, DeviceHistory,
    InventorySnapshot, ProcessSeriesSet, ProcessSnapshot, SeriesSet, ServicesSnapshot, Snapshot,
    StartupSnapshot, SystemSnapshot, WindowsSnapshot,
};
use tl_model::table::{ProcessTableView, SysTable, UnitRef};
use tl_model::time::{BootId, now_boot_ns};
use tl_model::validity::Validity;

use crate::client_session::ClientSessions;
use crate::source::WorkerControl;
use crate::update::EngineMsg;
use crate::workers::history::HistorySink;
use crate::workers::metric_history::MetricSink;

pub use devices::InvariantViolation;

/// Messages applied at most per publish (ADR-004).
pub const DRAIN_MAX: usize = 256;

/// The published slot and the one-token wake channel (ADR-005).
pub struct Publisher {
    pub latest: Arc<ArcSwap<Snapshot>>,
    pub wake_tx: async_channel::Sender<()>,
    /// Set when the wake receiver is gone. `run` returns once it is set.
    pub stop_publishing: bool,
}

impl Publisher {
    /// Stores `next`, then sends one wake token. A full channel already holds a token, so that
    /// is fine. A closed channel sets `stop_publishing` (ADR-005).
    pub fn publish(&mut self, next: Snapshot) {
        self.latest.store(Arc::new(next));
        match self.wake_tx.try_send(()) {
            Ok(()) | Err(async_channel::TrySendError::Full(())) => {}
            Err(async_channel::TrySendError::Closed(())) => self.stop_publishing = true,
        }
    }
}

/// Helper session commands routed by `apply.rs` (R-148 item 1). The helper is not a supervised
/// source, so `forward()` does not fit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HelperCommand {
    Start,
    End,
    SetupStatus,
    RunSetup(SetupStep),
}

/// The current `Arc` of every shared snapshot component, header included. The process table
/// is per client session; so are the rows, query generation and counts of `connections`,
/// `services` and `startup`, which stay unused here (ADR-018).
pub struct Components {
    pub system: Arc<SystemSnapshot>,
    pub series: Arc<SeriesSet>,
    pub processes: Arc<ProcessSnapshot>,
    pub connections: Arc<ConnectionsSnapshot>,
    pub services: Arc<ServicesSnapshot>,
    pub startup: Arc<StartupSnapshot>,
    pub inventory: Arc<InventorySnapshot>,
    pub devices: Arc<DeviceCatalog>,
    pub health: Arc<HealthSnapshot>,
    pub windows: Arc<WindowsSnapshot>,
    pub details: Arc<DetailsSnapshot>,
    pub history: Arc<HistoryView>,
    /// R-154.
    pub process_series: Arc<ProcessSeriesSet>,
    /// R-139.
    pub device_details: Arc<DeviceDetailsSnapshot>,
    /// ADR-017.
    pub device_history: Arc<DeviceHistory>,
}

/// Action admission (E11). The reducer holds `None` until E11-A1 installs one.
pub trait ActionGate: Send + 'static {
    fn submit(&mut self, cx: &Cx, req: ActionRequest, view: &AdmissionView<'_>);
    /// The source to refresh after the action, if any.
    fn finished(&mut self, id: ActionId) -> Option<SourceId>;
    /// The capabilities the gate's ops reported when it was built. The session copies them
    /// into the health snapshot once at start. The default is none.
    fn caps(&self) -> tl_model::health::EngineCaps {
        tl_model::health::EngineCaps::empty()
    }
}

/// What admission checks a process target against (ADR-006).
pub struct AdmissionView<'a> {
    pub boot_id: BootId,
    pub live: &'a FxHashMap<ProcessKey, ProcFlags>,
    pub exited_within_linger: &'a FxHashSet<ProcessKey>,
}

pub struct Reducer {
    pub epoch: u32,
    pub boot_id: BootId,
    pub sequence: u64,
    pub series: series::SeriesStore,
    pub devices: devices::Catalog,
    pub health: health::HealthState,
    pub processes: processes::ProcessState,
    /// Sleep and resume state (E09-008).
    pub suspend: suspend::SuspendState,
    /// The open client sessions with their table and system table queries (ADR-018).
    pub clients: ClientSessions,
    pub components: Components,
    /// Reducer-to-worker control (E08-004).
    pub control: watch::Sender<WorkerControl>,
    /// `None` in P1 and P2 (ADR-012).
    pub record_sink: Option<Box<dyn RecordSink>>,
    /// Takes `HistoryQuery` and `HistoryDelete` to the history worker (E30-A2). `None` when the
    /// session runs no history worker.
    pub history_cmd: Option<HistorySink>,
    /// Takes system samples and the device catalog to the `MetricHistory` worker (ADR-017).
    /// `None` when the session runs no such worker.
    pub metric_sink: Option<MetricSink>,
    /// `None` until E11-A1 installs one.
    pub action_gate: Option<Box<dyn ActionGate>>,
    pub events_tx: async_channel::Sender<ActionEvent>,
    /// `None` until E29-020 (qfx.23) sets it (R-148 item 1).
    pub helper_tx: Option<mpsc::Sender<HelperCommand>>,
    /// The engine settings watch. `run` reads it once per publish; `None` uses
    /// `EngineSettings::default()`.
    pub settings_rx: Option<watch::Receiver<EngineSettings>>,
    /// The UI demand watch. `run` reads it once per publish for the PSS keys (E09-006); `None`
    /// leaves PSS as the updates set it.
    pub demand_rx: Option<watch::Receiver<Demand>>,
    /// State only `apply` reads and writes (E09-007).
    pub apply_state: apply::ApplyState,
    /// The non-empty `Description` of every unit a `ServicesUpdate` reported, for the unit row
    /// names of the table (E09-025, PROC-009). Kept across updates.
    pub unit_names: FxHashMap<UnitRef, Arc<str>>,
    /// The non-empty `user` of every unit a `ServicesUpdate` reported, for the User column of
    /// unit rows. Kept across updates.
    pub unit_users: FxHashMap<UnitRef, Arc<str>>,
    /// Properties payloads, affinity masks and handle search state (E09-026).
    pub details: DetailsState,
    /// The last `GpuApiVersions`, merged into every later `Inventory` (E09-027).
    pub api_versions: Vec<GpuApiVersions>,
    dirty: ComponentMask,
    /// Header version of each header component at the last publish, in `HEADER_BITS` order.
    published: [u64; HEADER_BITS.len()],
}

/// The components that carry a `ComponentHeader` and that `build_snapshot` versions itself.
/// `SERIES` comes from `SeriesStore::publish`; `DEVICES` and `HEALTH` have their own `version`.
/// Each client session versions `SESSION_BITS` itself.
const HEADER_BITS: [ComponentMask; 9] = [
    ComponentMask::SYSTEM,
    ComponentMask::PROCESSES,
    ComponentMask::INVENTORY,
    ComponentMask::WINDOWS,
    ComponentMask::DETAILS,
    ComponentMask::HISTORY,
    ComponentMask::PROCESS_SERIES,
    ComponentMask::DEVICE_DETAILS,
    ComponentMask::DEVICE_HISTORY,
];

impl Reducer {
    /// Empty rings, header versions 0, every component `WarmingUp` except `windows`, which is
    /// `Unsupported` until the KWin worker publishes (ADR-003). `helper_tx` is `None`.
    pub fn new(
        epoch: u32,
        boot_id: BootId,
        control: watch::Sender<WorkerControl>,
        events_tx: async_channel::Sender<ActionEvent>,
    ) -> Self {
        let empty = Snapshot::empty(boot_id, epoch);
        let windows = WindowsSnapshot {
            header: ComponentHeader {
                validity: Validity::Unsupported,
                ..empty.windows.header
            },
            ..(*empty.windows).clone()
        };
        Reducer {
            epoch,
            boot_id,
            sequence: 0,
            series: series::SeriesStore::new(),
            devices: devices::Catalog::new(),
            health: health::HealthState::new(),
            processes: processes::ProcessState::new(),
            suspend: suspend::SuspendState::default(),
            clients: ClientSessions::new(&empty),
            components: Components {
                system: empty.system,
                series: empty.series,
                processes: empty.processes,
                connections: empty.connections,
                services: empty.services,
                startup: empty.startup,
                inventory: empty.inventory,
                devices: empty.devices,
                health: empty.health,
                windows: Arc::new(windows),
                details: empty.details,
                history: empty.history,
                process_series: empty.process_series,
                device_details: empty.device_details,
                device_history: empty.device_history,
            },
            control,
            record_sink: None,
            history_cmd: None,
            metric_sink: None,
            action_gate: None,
            events_tx,
            helper_tx: None,
            settings_rx: None,
            demand_rx: None,
            apply_state: apply::ApplyState::default(),
            unit_names: FxHashMap::default(),
            unit_users: FxHashMap::default(),
            details: DetailsState::default(),
            api_versions: Vec::new(),
            dirty: ComponentMask::NONE,
            published: [0; HEADER_BITS.len()],
        }
    }

    /// The loop: receive one message, drain until `DRAIN_MAX` messages in total, apply, publish
    /// once. Returns `Ok(())` when the channel closes, the task is cancelled or the wake
    /// receiver is gone. Returns `Err` on an invariant violation (the session restarts).
    pub async fn run(
        &mut self,
        cx: &Cx,
        rx: &mut mpsc::Receiver<EngineMsg>,
        publisher: &mut Publisher,
    ) -> Result<(), InvariantViolation> {
        while !publisher.stop_publishing {
            let Ok(first) = rx.recv(cx).await else {
                return Ok(());
            };
            let settings = self
                .settings_rx
                .as_ref()
                .map_or_else(EngineSettings::default, watch::Receiver::borrow_and_clone);
            self.apply(cx, first, &settings)?;
            let mut applied = 1;
            while applied < DRAIN_MAX {
                let Ok(msg) = rx.try_recv() else {
                    break;
                };
                self.apply(cx, msg, &settings)?;
                applied += 1;
            }
            if let Some(demand) = self
                .demand_rx
                .as_ref()
                .map(watch::Receiver::borrow_and_clone)
            {
                self.apply_demand(&demand, &settings);
            }
            tracing::debug!(target: "tasklens::engine", applied, "reducer publish");
            // No await between the store and the wake (ADR-005).
            let next = self.build_snapshot();
            if let Some(sink) = self.metric_sink.as_mut() {
                sink.on_snapshot(&next);
            }
            self.clients.publish(&next);
            publisher.publish(next);
        }
        Ok(())
    }

    /// Installs or removes the record sink: E30's recording switch. `None` stops every sink call
    /// from the next applied message on (ADR-012).
    pub fn set_record_sink(&mut self, sink: Option<Box<dyn RecordSink>>) {
        self.record_sink = sink;
    }

    /// Marks a component as changed for the next publish. Called by apply.
    pub fn mark(&mut self, bit: ComponentMask) {
        self.dirty |= bit;
    }

    /// `sequence += 1`. Dirty components get a new version; every other component reuses its
    /// `Arc`. A dirty header component whose `Arc` was not replaced with a higher version gets
    /// `version` one above the last published one; its observed times are the ones apply set.
    /// Returns the local session's snapshot; `ClientSessions::publish` makes the others.
    fn build_snapshot(&mut self) -> Snapshot {
        let mut dirty = std::mem::replace(&mut self.dirty, ComponentMask::NONE);
        self.sequence += 1;

        if let Some(mut next) = self.details.publish() {
            // The header keeps the validity `apply` set; `bump` below sets the version.
            Arc::make_mut(&mut next).header = self.components.details.header;
            self.components.details = next;
            dirty |= ComponentMask::DETAILS;
        }

        if dirty.contains(ComponentMask::SERIES) {
            self.components.series = self.series.publish();
        }
        if dirty.contains(ComponentMask::DEVICES)
            && let Some(next) = self.devices.publish()
        {
            if let Some(sink) = self.record_sink.as_deref_mut() {
                sink.on_device_catalog(&next);
            }
            if let Some(sink) = self.metric_sink.as_mut() {
                sink.on_device_catalog(&next);
            }
            self.components.devices = next;
        }
        if dirty.contains(ComponentMask::HEALTH)
            && let Some(next) = self.health.publish()
        {
            self.components.health = next;
        }

        let c = &mut self.components;
        for (i, bit) in HEADER_BITS.into_iter().enumerate() {
            if !dirty.contains(bit) {
                continue;
            }
            let p = self.published[i];
            self.published[i] = match bit {
                ComponentMask::SYSTEM => bump(&mut c.system, p),
                ComponentMask::PROCESSES => bump(&mut c.processes, p),
                ComponentMask::INVENTORY => bump(&mut c.inventory, p),
                ComponentMask::WINDOWS => bump(&mut c.windows, p),
                ComponentMask::DETAILS => bump(&mut c.details, p),
                ComponentMask::HISTORY => bump(&mut c.history, p),
                ComponentMask::PROCESS_SERIES => bump(&mut c.process_series, p),
                ComponentMask::DEVICE_HISTORY => bump(&mut c.device_history, p),
                _ => bump(&mut c.device_details, p),
            };
        }
        for session in self.clients.iter_mut() {
            session.prepare(dirty, c);
        }

        let local = self.clients.local();
        Snapshot {
            sequence: self.sequence,
            epoch: self.epoch,
            boot_id: self.boot_id,
            published_ns: now_boot_ns(),
            system: Arc::clone(&c.system),
            series: Arc::clone(&c.series),
            processes: Arc::clone(&c.processes),
            table: Arc::clone(local.table()),
            connections: Arc::clone(local.connections()),
            services: Arc::clone(local.services()),
            startup: Arc::clone(local.startup()),
            inventory: Arc::clone(&c.inventory),
            devices: Arc::clone(&c.devices),
            health: Arc::clone(&c.health),
            windows: Arc::clone(&c.windows),
            details: Arc::clone(&c.details),
            history: Arc::clone(&c.history),
            process_series: Arc::clone(&c.process_series),
            device_details: Arc::clone(&c.device_details),
            device_history: Arc::clone(&c.device_history),
        }
    }
}

/// The reducer side of `Snapshot.details` (E09-026, ADR-003). Each tab payload is an `Arc` that
/// only `apply_tab` replaces, so an unchanged tab is pointer-equal across publishes.
#[derive(Default)]
pub struct DetailsState {
    targets: FxHashMap<DetailTarget, TargetDetail>,
    affinity: FxHashMap<ProcessKey, AffinityInfo>,
    handle_search: Option<Arc<HandleSearchState>>,
    dirty: bool,
}

impl DetailsState {
    /// Replaces the `Arc` of one tab and bumps that tab's version.
    pub fn apply_tab(
        &mut self,
        target: DetailTarget,
        tab: DetailTab,
        payload: Arc<TabPayload>,
        validity: Validity,
    ) {
        let slot = &mut self.targets.entry(target).or_default().tabs[tab.index()];
        slot.payload = Some(payload);
        slot.version += 1;
        slot.validity = validity;
        self.dirty = true;
    }

    pub fn apply_affinity(&mut self, entries: Vec<(ProcessKey, AffinityInfo)>) {
        if entries.is_empty() {
            return;
        }
        self.affinity.extend(entries);
        self.dirty = true;
    }

    pub fn apply_handle_search(&mut self, state: Arc<HandleSearchState>) {
        self.handle_search = Some(state);
        self.dirty = true;
    }

    /// Removes the handle search state once the search has ended and the UI no longer shows it.
    pub fn clear_handle_search(&mut self) {
        if self.handle_search.take().is_some() {
            self.dirty = true;
        }
    }

    /// Drops the targets not in `demand.details` and the affinity keys not in
    /// `demand.affinity_targets`, so a closed window frees its payloads.
    pub fn retain(&mut self, demand: &Demand) {
        let targets = self.targets.len();
        let affinity = self.affinity.len();
        self.targets
            .retain(|target, _| demand.details.iter().any(|d| d.target == *target));
        self.affinity
            .retain(|key, _| demand.affinity_targets.contains(key));
        if self.targets.len() != targets || self.affinity.len() != affinity {
            self.dirty = true;
        }
    }

    /// The next component when something changed since the last publish, else `None`. The
    /// header is the default; the reducer sets it.
    pub fn publish(&mut self) -> Option<Arc<DetailsSnapshot>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        Some(Arc::new(DetailsSnapshot {
            header: ComponentHeader::default(),
            targets: self
                .targets
                .iter()
                .map(|(target, detail)| (*target, Arc::new(detail.clone())))
                .collect(),
            affinity: self.affinity.iter().map(|(k, v)| (*k, *v)).collect(),
            handle_search: self.handle_search.clone(),
        }))
    }
}

/// The `sys_queries` slot of `table`.
pub fn sys_table_index(table: SysTable) -> usize {
    match table {
        SysTable::Services => 0,
        SysTable::Startup => 1,
        SysTable::Connections => 2,
    }
}

/// A snapshot component with a `ComponentHeader`.
pub(crate) trait Headed: Clone {
    fn header(&self) -> &ComponentHeader;
    fn header_mut(&mut self) -> &mut ComponentHeader;
}

macro_rules! headed {
    ($($t:ty),* $(,)?) => {$(
        impl Headed for $t {
            fn header(&self) -> &ComponentHeader {
                &self.header
            }
            fn header_mut(&mut self) -> &mut ComponentHeader {
                &mut self.header
            }
        }
    )*};
}

headed!(
    SystemSnapshot,
    ProcessSnapshot,
    ProcessTableView,
    ConnectionsSnapshot,
    ServicesSnapshot,
    StartupSnapshot,
    InventorySnapshot,
    WindowsSnapshot,
    DetailsSnapshot,
    HistoryView,
    ProcessSeriesSet,
    DeviceDetailsSnapshot,
    DeviceHistory,
);

/// Makes the header version of `component` at least `published + 1` and returns it. Apply
/// replaces a changed component with a fresh `Arc`, which is not shared, so `make_mut` does not
/// copy it.
pub(crate) fn bump<T: Headed>(component: &mut Arc<T>, published: u64) -> u64 {
    let version = component.header().version;
    if version > published {
        return version;
    }
    let next = published + 1;
    Arc::make_mut(component).header_mut().version = next;
    next
}
