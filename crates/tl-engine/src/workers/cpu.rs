//! The `Cpu` worker's source (ADR-004, ADR-008): wraps the tl-collect `CpuSampler` and turns
//! each `CpuReading` into a `SourceUpdate::Cpu(CpuSample)`.
//!
//! It samples at 10 Hz while a visible CPU chart animates, at 1 Hz while CPU is otherwise
//! demanded or history recording is on, and not at all otherwise. Clock, temperature, live stats
//! and RAPL power are read at most once per second; fast ticks repeat the last values (PERF-028,
//! PERF-033, PERF-038, PERF-039, PERF-040).
//!
//! It also sends the CPU page's static rows as one `CpuStaticInfo` through `take_extra`: on the
//! first sample of a generation and again after a CPU hotplug (E12-010, PERF-031, PERF-035 to
//! PERF-037, PERF-013). The two D-Bus facts in it are read once per generation, before the
//! sampling loop starts.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use smallvec::SmallVec;
use tl_collect::counters::{Clock, Measured};
use tl_collect::inventory::cpuinfo::{
    CpuInfo, Lockdown, SecurityPaths, SecuritySource, Virtualization, parse_cpuinfo, read_security,
};
use tl_collect::parse::cpufreq::{FreqReading, FreqSource, read_base_mhz, read_static};
use tl_collect::parse::rapl::{RaplReading, RaplStatus, rapl_status};
use tl_collect::parse::topology::{CacheLevel, TopologyInfo};
use tl_collect::sample::cpu::{CpuOpts, CpuReading, CpuSampler, LiveStats};
use tl_collect::sysroot::read_file;
use tl_collect::{CollectError, SysRoot};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    CacheInfo, CacheLevelInfo, CpuStaticInfo, InvField, RaplState, SecurityFeature, VirtState,
    VulnSummary,
};
use tl_model::system::{CpuTopology, CpuTotals, Metric};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{self, Bus, DbusConn, DbusError};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{CpuCoreSample, CpuSample, DeviceDetailsUpdate, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

/// `cpu_fast`: a visible CPU chart animates.
pub const FAST_PERIOD: Duration = Duration::from_millis(100);
pub const SLOW_PERIOD: Duration = Duration::from_secs(1);
/// Clock, temperature, live stats and power are read at most this often.
pub const SLOW_EVERY_NS: u64 = 1_000_000_000;

/// The CPU rate table (ADR-008). The process table keeps it at 1 Hz for the "Interrupts"
/// pseudo row (PROC-012, R-166). `tests/lab_demand.rs` holds an E08-008 copy of this function
/// that predates the pseudo-row rule.
pub fn cpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.cpu == CpuDemand::Off && !d.recording && !d.process_table {
        None
    } else if d.cpu_fast {
        Some(FAST_PERIOD)
    } else {
        Some(SLOW_PERIOD)
    }
}

/// Most bytes read from /proc/cpuinfo.
const CPUINFO_LIMIT: usize = 1 << 20;

const SYSTEMD_NAME: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_INTERFACE: &str = "org.freedesktop.systemd1.Manager";
/// power-profiles-daemon's current name; the path and interface use the same words.
const PPD_NAME: &str = "org.freedesktop.UPower.PowerProfiles";
const PPD_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
/// power-profiles-daemon's legacy name.
const PPD_LEGACY_NAME: &str = "net.hadess.PowerProfiles";
const PPD_LEGACY_PATH: &str = "/net/hadess/PowerProfiles";
/// The bus answers these when no process owns the name.
const NO_OWNER_ERRORS: [&str; 2] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
];

/// D-Bus facts read once per generation before the sampling loop starts.
#[derive(Clone, Debug, Default)]
pub struct CpuBusFacts {
    /// systemd's `Virtualization`: "" on bare metal.
    pub running_in_vm: InvField<Arc<str>>,
    /// power-profiles-daemon's `ActiveProfile`, the Platform profile fallback.
    pub ppd_profile: InvField<Arc<str>>,
}

/// Reads systemd's `Virtualization` and power-profiles-daemon's `ActiveProfile` on the system
/// bus, each within `READ_TIMEOUT`. A connect failure or timeout makes both `Failed`. Never
/// fails the worker. The connection is dropped before this returns.
pub async fn read_bus_facts(cx: &Cx) -> CpuBusFacts {
    let Ok(conn) = dbus::connect(cx, Bus::System).await else {
        return CpuBusFacts {
            running_in_vm: inv_invalid(Validity::Failed),
            ppd_profile: inv_invalid(Validity::Failed),
        };
    };
    let running_in_vm = match read_str_property(
        cx,
        &conn,
        SYSTEMD_NAME,
        SYSTEMD_PATH,
        SYSTEMD_INTERFACE,
        "Virtualization",
    )
    .await
    {
        Ok(value) => inv_valid(value.into()),
        Err(_) => inv_invalid(Validity::Failed),
    };
    let ppd_profile = match read_ppd_profile(cx, &conn, PPD_NAME, PPD_PATH).await {
        Err(e) if no_owner(&e) => {
            match read_ppd_profile(cx, &conn, PPD_LEGACY_NAME, PPD_LEGACY_PATH).await {
                Err(e) if no_owner(&e) => inv_invalid(Validity::Unsupported),
                other => profile_field(other),
            }
        }
        other => profile_field(other),
    };
    drop(conn);
    CpuBusFacts {
        running_in_vm,
        ppd_profile,
    }
}

async fn read_ppd_profile(
    cx: &Cx,
    conn: &DbusConn,
    name: &str,
    path: &str,
) -> Result<String, DbusError> {
    read_str_property(cx, conn, name, path, name, "ActiveProfile").await
}

fn profile_field(r: Result<String, DbusError>) -> InvField<Arc<str>> {
    match r {
        Ok(value) => inv_valid(value.into()),
        Err(_) => inv_invalid(Validity::Failed),
    }
}

fn no_owner(e: &DbusError) -> bool {
    matches!(e, DbusError::Call { name, .. } if NO_OWNER_ERRORS.contains(&&**name))
}

/// One `Properties.Get` of a string property through an uncached proxy, in one `read`.
async fn read_str_property(
    cx: &Cx,
    conn: &DbusConn,
    name: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Result<String, DbusError> {
    conn.read(cx, async {
        let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(conn.raw())
            .destination(name)?
            .path(path)?
            .interface(interface)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        proxy.get_property::<String>(property).await
    })
    .await
}

/// The CPU page's static rows from sysfs, /proc/cpuinfo, the security sources under `/dev` and
/// `facts`. Blocking; runs inside `sample`.
pub fn build_static(root: &SysRoot, topo: &TopologyInfo, facts: &CpuBusFacts) -> CpuStaticInfo {
    build_static_in(root, topo, facts, Path::new("/dev"))
}

/// `build_static` with the `/dev` folder for the security sources given, so tests can point
/// it at a fixture.
pub fn build_static_in(
    root: &SysRoot,
    topo: &TopologyInfo,
    facts: &CpuBusFacts,
    dev: &Path,
) -> CpuStaticInfo {
    let freq = read_static(root);
    let platform_profile = match &freq.platform_profile {
        Measured {
            value: Some(profile),
            validity: Validity::Valid,
        } => inv_valid(profile.clone()),
        _ => facts.ppd_profile.clone(),
    };

    let mut buf = Vec::new();
    let mut cpuinfo = CpuInfo::default();
    let microcode = match read_file(&root.proc_path("cpuinfo"), &mut buf, CPUINFO_LIMIT) {
        Ok(_) => {
            parse_cpuinfo(&buf, &mut cpuinfo);
            match &cpuinfo.microcode {
                Some(microcode) => inv_valid(microcode.clone()),
                None => inv_invalid(Validity::Unsupported),
            }
        }
        Err(e) => inv_invalid(e.validity()),
    };

    let sec = read_security(
        &cpuinfo,
        &SecurityPaths {
            sys: &root.sys,
            proc: &root.proc,
            dev,
        },
    );
    let mut security = SmallVec::new();
    if sec.secure_boot == Some(true) {
        security.push(SecurityFeature::SecureBoot);
    }
    match sec.lockdown {
        Some(Lockdown::Integrity) => security.push(SecurityFeature::LockdownIntegrity),
        Some(Lockdown::Confidentiality) => security.push(SecurityFeature::LockdownConfidentiality),
        Some(Lockdown::None) | None => {}
    }
    if sec.memory_encryption == Some(true) {
        security.push(SecurityFeature::MemoryEncryption);
    }
    if sec.user_shadow_stack {
        security.push(SecurityFeature::UserShadowStack);
    }
    if sec.iommu == Some(true) {
        security.push(SecurityFeature::Iommu);
    }
    let security_unreadable = sec
        .unreadable
        .iter()
        .map(|source| {
            Arc::from(match source {
                SecuritySource::SecureBoot => "Secure Boot",
                SecuritySource::KernelLockdown => "Kernel lockdown",
                SecuritySource::MemoryEncryption => "Memory encryption",
                SecuritySource::IommuSysfs => "IOMMU",
                SecuritySource::Vulnerabilities => "CPU vulnerabilities",
            })
        })
        .collect();

    let caches = &topo.caches;
    CpuStaticInfo {
        cores: topo.cores,
        logical: topo.logical_count,
        caches: CacheInfo {
            l1d: caches.l1d.map(cache_level),
            l1i: caches.l1i.map(cache_level),
            l2: caches.l2.map(cache_level),
            l3: caches.l3.map(cache_level),
            l4: caches.l4.map(cache_level),
        },
        driver: freq_field(&freq.driver),
        governor: freq_field(&freq.governor),
        epp: freq_field(&freq.epp),
        platform_profile,
        prefcore_differs: freq.prefcore_differs(),
        prefcore_rank: freq.prefcore_rank.into(),
        virtualization: inv_valid(match sec.virtualization {
            Virtualization::EnabledKvm => VirtState::EnabledKvm,
            Virtualization::SupportedNoKvm => VirtState::SupportedNoKvm,
            Virtualization::NotSupported => VirtState::NotSupported,
        }),
        running_in_vm: facts.running_in_vm.clone(),
        hypervisor: sec.hypervisor,
        security,
        security_unreadable,
        vulns: sec.vulns.map(|v| VulnSummary {
            mitigated: v.mitigated,
            vulnerable: v.vulnerable,
            not_affected: v.not_affected,
            unknown: v.unknown,
        }),
        microcode,
        rapl: Some(match rapl_status(root) {
            RaplStatus::NoZone => RaplState::NoZone,
            RaplStatus::Unreadable => RaplState::Unreadable,
            RaplStatus::Readable => RaplState::Readable,
        }),
    }
}

fn cache_level(c: CacheLevel) -> CacheLevelInfo {
    CacheLevelInfo {
        instances: c.instances,
        instance_kib: c.instance_kib,
        total_kib: c.total_kib,
    }
}

/// `Valid(None)` (no value in the file) becomes `Unsupported`.
fn freq_field(m: &Measured<Option<Arc<str>>>) -> InvField<Arc<str>> {
    match (&m.value, m.validity) {
        (Some(value), Validity::Valid) => inv_valid(value.clone()),
        (None, Validity::Valid) => inv_invalid(Validity::Unsupported),
        (_, validity) => inv_invalid(validity),
    }
}

fn inv_valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn inv_invalid<T: Default>(validity: Validity) -> InvField<T> {
    InvField {
        value: T::default(),
        validity,
    }
}

/// The values read only on slow ticks, repeated on fast ticks.
#[derive(Default)]
struct SlowFields {
    freq: FreqReading,
    temp_c: Measured<f32>,
    live: LiveStats,
    power: RaplReading,
    /// `freq.per_cpu_mhz` as sent. A new `Arc` only when a value changed.
    per_cpu_mhz: Arc<[Metric<u32>]>,
}

pub struct CpuSource {
    sampler: CpuSampler,
    /// Reused every sample.
    reading: CpuReading,
    /// Written by `period()`, read by `sample()`.
    per_cpu_wanted: Arc<AtomicBool>,
    /// Boot ns of the last sample that read clock, temperature, live stats and power.
    last_slow_ns: Option<TimeNs>,
    last: SlowFields,
    /// Read once in `new`.
    base_mhz: Measured<u32>,
    topology: Arc<CpuTopology>,
    /// True on the first sample and after a topology change.
    send_topology: bool,
    root: SysRoot,
    facts: CpuBusFacts,
    /// True on the first sample and after a topology change.
    send_static: bool,
    /// The `CpuStaticInfo` waiting for `take_extra`.
    pending_static: Option<Arc<CpuStaticInfo>>,
}

impl CpuSource {
    /// Fails only when the sampler cannot read the CPU topology.
    pub fn new(root: SysRoot) -> Result<Self, CollectError> {
        let sampler = CpuSampler::new(root.clone())?;
        let base_mhz = read_base_mhz(&root);
        let topology = sampler.topology().topology.clone();
        Ok(Self {
            sampler,
            reading: CpuReading::default(),
            per_cpu_wanted: Arc::new(AtomicBool::new(false)),
            last_slow_ns: None,
            last: SlowFields::default(),
            base_mhz,
            topology,
            send_topology: true,
            root,
            facts: CpuBusFacts::default(),
            send_static: true,
            pending_static: None,
        })
    }

    /// Sets the D-Bus facts that go into every `CpuStaticInfo`.
    pub fn with_facts(mut self, facts: CpuBusFacts) -> Self {
        self.facts = facts;
        self
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        let per_cpu = self.per_cpu_wanted.load(Ordering::Relaxed);
        let slow = self
            .last_slow_ns
            .is_none_or(|t| clock.boot_ns.saturating_sub(t) >= SLOW_EVERY_NS);
        let opts = CpuOpts {
            per_cpu,
            freq: slow,
            temp: slow,
            live_stats: slow,
            power: slow,
        };
        self.sampler.sample_at(clock, opts, &mut self.reading);
        let r = &self.reading;
        if slow {
            let freq = &mut self.last.freq;
            freq.source = r.freq.source;
            freq.avg_mhz = r.freq.avg_mhz;
            freq.max_mhz = r.freq.max_mhz;
            freq.per_cpu_mhz.clone_from(&r.freq.per_cpu_mhz);
            let sent = &self.last.per_cpu_mhz;
            let same = sent.len() == r.freq.per_cpu_mhz.len()
                && sent
                    .iter()
                    .zip(&r.freq.per_cpu_mhz)
                    .all(|(a, &b)| *a == metric(b));
            if !same {
                self.last.per_cpu_mhz = r.freq.per_cpu_mhz.iter().map(|&m| metric(m)).collect();
            }
            self.last.temp_c = r.temp_c;
            self.last.live = r.live;
            self.last.power = r.power;
            self.last_slow_ns = Some(clock.boot_ns);
        }
        if r.topology_changed {
            self.topology = self.sampler.topology().topology.clone();
            self.send_topology = true;
            self.send_static = true;
        }
        if self.send_static {
            let info = build_static(&self.root, self.sampler.topology(), &self.facts);
            self.pending_static = Some(Arc::new(info));
            self.send_static = false;
        }

        let last = &self.last;
        let totals = CpuTotals {
            util_pct: metric(r.total_pct),
            kernel_pct: metric(r.kernel_pct),
            iowait_pct: metric(r.iowait_pct),
            freq_mhz: metric(last.freq.avg_mhz),
            freq_fastest_mhz: metric(last.freq.max_mhz),
            freq_is_bogomips: last.freq.source == FreqSource::BogoMips,
            per_cpu_mhz: Arc::clone(&last.per_cpu_mhz),
            base_mhz: metric(self.base_mhz),
            temp_c: metric(last.temp_c),
            package_power_w: metric(last.power.package_w),
            core_power_w: metric(last.power.core_w),
            processes: metric(last.live.process_count),
            // Every kernel scheduling entity, from the "running/total" field of /proc/loadavg
            // (PERF-033). WarmingUp before the first slow tick, Missing when loadavg is unreadable.
            threads: Metric {
                value: last.live.load.value.total,
                validity: last.live.load.validity,
            },
            handles: metric(last.live.open_files),
            load_avg: Metric {
                value: [
                    last.live.load.value.one,
                    last.live.load.value.five,
                    last.live.load.value.fifteen,
                ],
                validity: last.live.load.validity,
            },
            sockets: Metric::valid(self.sampler.topology().sockets),
        };
        let irq_pct = if r.interrupts_pct.validity.is_valid() {
            r.interrupts_pct.value
        } else {
            0.0
        };
        let per_cpu = r
            .per_cpu
            .iter()
            .enumerate()
            .map(|(id, core)| CpuCoreSample {
                cpu: u16::try_from(id).unwrap_or(u16::MAX),
                pct: core.total_pct.value,
                kernel_pct: core.kernel_pct.value,
                freq_mhz: valid_value(last.freq.per_cpu_mhz.get(id).copied()),
                validity: core.total_pct.validity,
            })
            .collect();
        let sample = CpuSample {
            t_ns: r.t_ns,
            totals,
            irq_pct,
            per_cpu,
            freq_mhz: valid_value(Some(last.freq.avg_mhz)),
            temp_c: valid_value(Some(last.temp_c)),
            temp_validity: last.temp_c.validity,
            topology: self.send_topology.then(|| self.topology.clone()),
            validity: r.total_pct.validity,
        };
        self.send_topology = false;
        (self, SourceUpdate::Cpu(sample))
    }
}

impl Source for CpuSource {
    fn id(&self) -> SourceId {
        SourceId::Cpu
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.per_cpu_wanted
            .store(d.cpu.wants_per_core() || d.recording, Ordering::Relaxed);
        cpu_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.last_slow_ns = None;
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending_static
            .take()
            .map(|info| SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Cpu(info)))
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Cpu,
        name: "cpu",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// Reads the D-Bus facts, then builds a new `CpuSource` per generation on the blocking pool (it
/// reads sysfs), then `source::run`. Nothing is carried across a restart: each generation starts
/// with fresh baselines. A `CpuSource::new` failure is `WorkerError::Collect`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let facts = read_bus_facts(&cx).await;
            let mut job = match cx.spawn_blocking(move |_| CpuSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(Ok(src)) => src.with_facts(facts),
                Ok(Err(e)) => return Outcome::Err(WorkerError::Collect(e)),
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

fn valid_value<T>(m: Option<Measured<T>>) -> Option<T> {
    m.filter(|m| m.validity.is_valid()).map(|m| m.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        let off = Demand::default();
        assert_eq!(cpu_period(&off, &s), None);

        let fast = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&fast, &s), Some(Duration::from_millis(100)));

        let slow = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: false,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&slow, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            cpu: CpuDemand::Off,
            recording: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&recording, &s), Some(Duration::from_secs(1)));

        let table = Demand {
            cpu: CpuDemand::Off,
            recording: false,
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&table, &s), Some(Duration::from_secs(1)));

        let table_fast = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: true,
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(
            cpu_period(&table_fast, &s),
            Some(Duration::from_millis(100))
        );
    }
}
