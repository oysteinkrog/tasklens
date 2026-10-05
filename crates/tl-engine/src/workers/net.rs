//! The `Net` worker (ADR-004, ADR-008): wraps the tl-collect `NetSampler` and turns each
//! `NetReading` into a `SourceUpdate::Nets(NetSample)`.
//!
//! It samples `/proc/net/dev` at 1 Hz while network is demanded or history recording is on, and
//! not at all otherwise. The interfaces are enumerated again every 5 s; each enumeration also
//! sends a full `DeviceList` for `NETS` through `take_extra`. Loopback never appears (ADR-016,
//! R-052, PERF-054 to PERF-056).

use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Measured};
use tl_collect::parse::netif::{self, NetIface};
use tl_collect::sample::net::{IfaceId, NetReading, NetSampler};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{NetSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::{self, EnumClock, GenTracker};

pub const PERIOD: Duration = Duration::from_secs(1);

/// The network rate table (ADR-008 row "Disks, network interfaces").
pub fn net_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.nets || d.recording {
        Some(PERIOD)
    } else {
        None
    }
}

pub struct NetSource {
    root: SysRoot,
    sampler: NetSampler,
    /// Reused every sample.
    reading: NetReading,
    /// The last successful enumeration.
    ifaces: Vec<NetIface>,
    /// Identity and sampler id of every interface, in sampler order.
    ids: Vec<(DeviceIdentity, IfaceId)>,
    /// Keyed by (netns_ino, ifindex, name).
    gens: GenTracker<(u64, u32, Arc<str>)>,
    /// When the interfaces are enumerated again.
    clock: EnumClock,
    /// The `Devices` list waiting for `take_extra`.
    pending: Option<SourceUpdate>,
}

impl NetSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: NetSampler::new(root.clone()),
            root,
            reading: NetReading::default(),
            ifaces: Vec::new(),
            ids: Vec::new(),
            gens: GenTracker::default(),
            clock: EnumClock::default(),
            pending: None,
        }
    }

    /// The sampler ids of the last successful enumeration.
    pub fn iface_ids(&self) -> impl Iterator<Item = &IfaceId> + '_ {
        self.ids.iter().map(|(_, id)| id)
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        if self.clock.due(clock.boot_ns) {
            self.enumerate(clock.boot_ns);
        }
        self.sampler.sample_at(clock, &mut self.reading);

        let nets = self
            .ids
            .iter()
            .zip(&self.reading.ifaces)
            .map(|((identity, _), r)| {
                let totals = NetTotals {
                    // The worker cannot know the catalog index; the reducer sets it.
                    device: DeviceIdx(0),
                    rx_bps: metric(r.rx_bps),
                    tx_bps: metric(r.tx_bps),
                    rx_total: r.total_rx_bytes.value,
                    tx_total: r.total_tx_bytes.value,
                    // E12-014 reads the link speed and maps the operstate.
                    link_speed_mbps: Metric::with(Validity::WarmingUp),
                    state: LinkState::Unknown,
                };
                (identity.clone(), totals)
            })
            .collect();
        let sample = NetSample {
            t_ns: self.reading.t_ns,
            nets,
        };
        (self, SourceUpdate::Nets(sample))
    }

    /// Rebuilds `ids`, the sampler's interface list and the pending `Devices` list. A failed
    /// enumeration keeps the previous interfaces and sends no list.
    fn enumerate(&mut self, t_ns: u64) {
        let mut found = Vec::new();
        if netif::enumerate(&self.root, &mut found).is_err() {
            return;
        }
        self.ifaces = found;

        let keys: Vec<(u64, u32, Arc<str>)> = self
            .ifaces
            .iter()
            .map(|i| (i.netns_ino, i.ifindex, i.name.clone()))
            .collect();
        let generations = self.gens.update(&keys);
        self.ids = self
            .ifaces
            .iter()
            .zip(generations)
            .map(|(iface, generation)| {
                let id = IfaceId {
                    name: iface.name.clone(),
                    generation,
                };
                (enumerate::net_identity(iface), id)
            })
            .collect();
        let sampler_ids: Vec<IfaceId> = self.ids.iter().map(|(_, id)| id.clone()).collect();
        self.sampler.set_interfaces(&sampler_ids);
        self.pending = Some(SourceUpdate::Devices(enumerate::net_list(
            t_ns,
            &self.ifaces,
            &self.root.sys,
        )));
    }
}

impl Source for NetSource {
    fn id(&self) -> SourceId {
        SourceId::Net
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        net_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.clock.force();
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending.take()
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Net,
        name: "net",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `NetSource` per generation, built on the blocking pool, then `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| NetSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(src) => src,
                Err(JoinError::Cancelled(reason)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    return Outcome::Cancelled(reason);
                }
                Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
                Err(JoinError::PolledAfterCompletion) => {
                    return Outcome::Err(WorkerError::Other("job polled after completion".into()));
                }
            };
            source::run(cx, io, src).await
        }
    })
}

fn metric<T: Copy + Default>(m: Measured<T>) -> Metric<T> {
    Metric {
        value: m.value,
        validity: m.validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        assert_eq!(net_period(&Demand::default(), &s), None);

        let nets = Demand {
            nets: true,
            ..Demand::default()
        };
        assert_eq!(net_period(&nets, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert_eq!(net_period(&recording, &s), Some(Duration::from_secs(1)));
    }
}
