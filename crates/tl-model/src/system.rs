//! Machine-wide totals and CPU topology (ADR-003).
//!
//! These structs hold the current values and the counters the stats grids show. Graph history
//! lives in `series.rs`. Every live field is a `Metric` or carries a `validity` field, so a
//! missing value is never a bare zero.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::ids::{DeviceIdx, MetricId};
use crate::validity::Validity;

/// A value with its validity. Never read `value` unless `validity == Valid` or `Stale`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Metric<T: Copy + Default> {
    pub value: T,
    pub validity: Validity,
}

impl<T: Copy + Default> Metric<T> {
    /// A valid value.
    pub fn valid(v: T) -> Self {
        Self {
            value: v,
            validity: Validity::Valid,
        }
    }

    /// No value, only a state. `value` is `T::default()`.
    pub fn with(validity: Validity) -> Self {
        Self {
            value: T::default(),
            validity,
        }
    }
}

/// CPU affinity mask, up to 1024 CPUs (ADR-005).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct CpuSet(pub [u64; 16]);

impl CpuSet {
    pub const MAX_CPUS: u16 = 1024;

    pub const fn empty() -> CpuSet {
        CpuSet([0; 16])
    }

    /// Adds `cpu`. Ignores `cpu >= 1024`.
    pub fn insert(&mut self, cpu: u16) {
        if cpu < Self::MAX_CPUS {
            self.0[usize::from(cpu / 64)] |= 1 << (cpu % 64);
        }
    }

    /// Removes `cpu`. Ignores `cpu >= 1024`.
    pub fn remove(&mut self, cpu: u16) {
        if cpu < Self::MAX_CPUS {
            self.0[usize::from(cpu / 64)] &= !(1 << (cpu % 64));
        }
    }

    pub fn contains(&self, cpu: u16) -> bool {
        cpu < Self::MAX_CPUS && self.0[usize::from(cpu / 64)] & (1 << (cpu % 64)) != 0
    }

    /// Number of CPUs in the set.
    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    /// CPU ids in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.iter().enumerate().flat_map(|(i, &word)| {
            let base = (i as u16) * 64;
            let mut rest = word;
            std::iter::from_fn(move || {
                if rest == 0 {
                    return None;
                }
                let bit = rest.trailing_zeros() as u16;
                rest &= rest - 1;
                Some(base + bit)
            })
        })
    }

    pub fn intersection(&self, other: &CpuSet) -> CpuSet {
        let mut out = CpuSet::empty();
        for (o, (a, b)) in out.0.iter_mut().zip(self.0.iter().zip(other.0.iter())) {
            *o = a & b;
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum CoreKind {
    Performance,
    Efficiency,
    Standard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreInfo {
    /// Core index in the order of (package, core_id).
    pub id: u16,
    /// Logical CPU ids of this core (SMT siblings).
    pub logical: SmallVec<[u16; 2]>,
    /// Index of the L3 domain (shared_cpu_list of the L3 cache).
    pub l3_group: u16,
    pub kind: CoreKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CpuTopology {
    pub online: CpuSet,
    pub cores: Arc<[CoreInfo]>,
}

#[derive(Clone, Debug, Default)]
pub struct CpuTotals {
    /// 100 × Δbusy / Δtotal (ADR-008); also series CPU_TOTAL.
    pub util_pct: Metric<f32>,
    /// 100 × Δ(system + irq + softirq) / Δtotal; series CPU_KERNEL.
    pub kernel_pct: Metric<f32>,
    /// iowait share, reported in CPU details (ADR-008).
    pub iowait_pct: Metric<f32>,
    /// Mean of cpuinfo_avg_freq over policies (PERF-033, PERF-040).
    pub freq_mhz: Metric<u32>,
    /// Fastest core, for the Speed tooltip (PERF-033).
    pub freq_fastest_mhz: Metric<u32>,
    /// The clock comes from the BogoMIPS lines of /proc/cpuinfo; the UI marks it "(?)" (PERF-033).
    pub freq_is_bogomips: bool,
    /// Current clock per logical CPU, indexed by CPU id, for the CPU grid tooltips: the
    /// policy's clock file, else the cpuinfo lines (BogoMIPS when `freq_is_bogomips`). The
    /// worker reuses the `Arc` while the values do not change.
    pub per_cpu_mhz: Arc<[Metric<u32>]>,
    /// base_frequency, else acpi_cppc nominal_freq, else bios_limit.
    pub base_mhz: Metric<u32>,
    /// Tctl or Package id 0 (PERF-039).
    pub temp_c: Metric<f32>,
    /// RAPL intel-rapl:0 (PERF-038, P2).
    pub package_power_w: Metric<f32>,
    /// RAPL intel-rapl:0:0, for the tooltip (PERF-038, P2).
    pub core_power_w: Metric<f32>,
    /// Numeric /proc entries (PERF-033).
    pub processes: Metric<u32>,
    /// Sum from the sweep, else /proc/stat fallback (PERF-033).
    pub threads: Metric<u32>,
    /// /proc/sys/fs/file-nr field 1 (PERF-033).
    pub handles: Metric<u64>,
    /// /proc/loadavg 1, 5 and 15 min. `load_avg[0]` is also series CPU_LOAD1.
    pub load_avg: Metric<[f32; 3]>,
    /// Distinct physical_package_id.
    pub sockets: Metric<u16>,
    /// /proc/pressure/cpu `some` avg10 in percent; series PSI_CPU_SOME (ADR-016 "System
    /// pressure"). `Unsupported` on a kernel without PSI.
    pub psi_some_pct: Metric<f32>,
    /// True when an Intel `thermal_throttle` count rose since the last read (ADR-016, UI-PRF-054).
    /// `Unsupported` where the files are missing (AMD).
    pub throttling: Metric<bool>,
}

/// All bytes, from /proc/meminfo unless noted (PERF-041 to PERF-043).
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryTotals {
    /// Of the meminfo read as a whole.
    pub validity: Validity,
    pub total: u64,
    pub available: u64,
    /// MemTotal - MemAvailable (MemFree when Available > Total).
    pub used: u64,
    pub free: u64,
    pub cached: u64,
    pub dirty: u64,
    pub writeback: u64,
    /// Committed_AS.
    pub committed: u64,
    /// CommitLimit.
    pub commit_limit: u64,
    pub slab_reclaimable: u64,
    pub slab_unreclaimable: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub huge_pages_total: u64,
    pub huge_pages_free: u64,
    pub huge_page_size: u64,
    pub zswap: u64,
    pub zswapped: u64,
    /// Summed over zram devices; Unsupported when none.
    pub zram: Metric<ZramTotals>,
    /// /proc/pressure/memory `some` avg10 in percent; series PSI_MEM_SOME (ADR-016 "System
    /// pressure"). `Unsupported` on a kernel without PSI.
    pub psi_some_pct: Metric<f32>,
    /// /proc/pressure/memory `full` avg10 in percent; series PSI_MEM_FULL.
    pub psi_full_pct: Metric<f32>,
    /// Pages swapped in per second from the /proc/vmstat `pswpin` delta; series SWAP_IN.
    pub swap_in_pps: Metric<f32>,
}

/// zram `mm_stat` fields 1, 2 and 3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZramTotals {
    pub orig_data: u64,
    pub compr_data: u64,
    pub mem_used: u64,
}

/// One per disk device (PERF-049).
#[derive(Clone, Copy, Debug)]
pub struct DiskTotals {
    pub device: DeviceIdx,
    pub active_pct: Metric<f32>,
    /// Bytes per second.
    pub read_bps: Metric<f64>,
    /// Bytes per second.
    pub write_bps: Metric<f64>,
    /// Δticks / Δops over reads, writes, discards and flushes.
    pub response_ms: Metric<f32>,
    /// Bytes since boot (sectors × 512).
    pub read_total: u64,
    /// Bytes since boot (sectors × 512).
    pub written_total: u64,
    /// /sys/block/<d>/size × 512.
    pub capacity: u64,
    /// Drive temperature (hwmon or SMART); Unsupported when none (R-139).
    pub temp_c: Metric<f32>,
}

/// One per interface (PERF-056).
#[derive(Clone, Copy, Debug)]
pub struct NetTotals {
    pub device: DeviceIdx,
    /// Bytes per second.
    pub rx_bps: Metric<f64>,
    /// Bytes per second.
    pub tx_bps: Metric<f64>,
    /// Bytes since boot.
    pub rx_total: u64,
    /// Bytes since boot.
    pub tx_total: u64,
    /// sysfs speed; -1 gives Unsupported ("Not reported").
    pub link_speed_mbps: Metric<u32>,
    /// sysfs operstate (R-139).
    pub state: LinkState,
}

/// sysfs `operstate`, one variant per kernel value (R-139 names Up, Down, Dormant, Unknown;
/// PERF-054 shows all seven): "up" Up, "down" Down, "testing" Testing, "dormant" Dormant,
/// "notpresent" NotPresent, "lowerlayerdown" LowerLayerDown, anything else Unknown. UI labels per
/// PERF-054 (Connected, Disconnected, ...).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum LinkState {
    Up,
    Down,
    Testing,
    Dormant,
    NotPresent,
    LowerLayerDown,
    #[default]
    Unknown,
}

/// GPU values that do not change while the driver runs (PERF-066 to PERF-071). The worker builds
/// one per probe `Hello` or DRM card scan, and every sample of that device shares it. `None`
/// means the device or driver does not report the value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuStaticInfo {
    /// The CUDA version the driver supports, for example "13.2". NVIDIA only.
    pub cuda_version: Option<Arc<str>>,
    pub vbios: Option<Arc<str>>,
    /// For example "Ada Lovelace". NVIDIA only.
    pub architecture: Option<Arc<str>>,
    /// The card's maximum PCIe generation and lane count (PERF-070).
    pub pcie_max_gen: Option<u8>,
    pub pcie_max_width: Option<u8>,
    /// Maximum graphics and memory clocks, MHz (PERF-066).
    pub clock_gfx_max_mhz: Option<u32>,
    pub clock_mem_max_mhz: Option<u32>,
    /// The highest power limit the card accepts, W (PERF-067 "max 350 W").
    pub power_max_limit_w: Option<f32>,
    /// The lowest power limit the card accepts, W (PERF-067 "Limit range 100 to 350 W"): NVML
    /// `min_limit`, amdgpu hwmon `power1_cap_min`.
    pub power_min_limit_w: Option<f32>,
    /// Temperatures where the card slows down and shuts down, °C (PERF-068).
    pub slowdown_temp_c: Option<f32>,
    pub shutdown_temp_c: Option<f32>,
}

/// One per GPU (PERF-063 to PERF-071).
#[derive(Clone, Debug)]
pub struct GpuTotals {
    pub device: DeviceIdx,
    pub util_pct: Metric<f32>,
    pub mem_util_pct: Metric<f32>,
    pub mem_used: Metric<u64>,
    pub mem_total: Metric<u64>,
    pub shared_used: Metric<u64>,
    pub shared_total: Metric<u64>,
    /// Hardware reserved memory, bytes: NVML `reserved`; Unsupported elsewhere (PERF-064).
    pub mem_reserved: Metric<u64>,
    pub enc_pct: Metric<f32>,
    pub dec_pct: Metric<f32>,
    /// Active NVENC sessions; Unsupported off NVIDIA (PERF-065).
    pub enc_sessions: Metric<u32>,
    /// Encode and decode run on one engine (AMD VCN, Intel video), so `enc_pct` and `dec_pct`
    /// hold the same value and the UI shows one row (PERF-065).
    pub enc_dec_shared: bool,
    pub temp_c: Metric<f32>,
    /// amdgpu hwmon `edge`, `junction` and `mem`, for the Temperature tooltip (PERF-068);
    /// Unsupported where the card has no such sensor.
    pub temp_edge_c: Metric<f32>,
    pub temp_junction_c: Metric<f32>,
    pub temp_mem_c: Metric<f32>,
    pub power_w: Metric<f32>,
    pub power_limit_w: Metric<f32>,
    /// Energy used since the driver loaded, joules: NVML total energy, Intel dGPU hwmon
    /// `energy1_input` (PERF-067).
    pub energy_since_boot: Metric<f64>,
    pub clock_gfx_mhz: Metric<u32>,
    pub clock_mem_mhz: Metric<u32>,
    /// NVML video clock, MHz (PERF-066).
    pub clock_video_mhz: Metric<u32>,
    /// Performance state, 0 for P0 to 15 for P15. NVIDIA only (PERF-066).
    pub pstate: Metric<u8>,
    /// Why the clocks are held down, for the Clocks tooltip (PERF-066). An empty set means the
    /// clocks are not held down. NVIDIA only.
    pub clock_event_reasons: Metric<ClockEventReasons>,
    /// True while `HW_SLOWDOWN` or `SW_THERMAL` is set in the clocks event reasons (UI-PRF-054).
    /// `Unsupported` off NVIDIA.
    pub throttling: Metric<bool>,
    pub fan_pct: Metric<f32>,
    /// NVML fan 0 or amdgpu hwmon `fan1_input` (PERF-068).
    pub fan_rpm: Metric<u32>,
    /// PCIe receive throughput, bytes per second (R-155).
    pub pcie_rx_bps: Metric<f32>,
    /// PCIe transmit throughput, bytes per second (R-155).
    pub pcie_tx_bps: Metric<f32>,
    /// Current PCIe generation and lane count. They drop while the card idles (PERF-070).
    pub pcie_gen: Metric<u8>,
    pub pcie_width: Metric<u8>,
    /// Busy percent of every engine the device reports (PERF-063). The page draws one graph per
    /// reported engine.
    pub engines: GpuEngines,
    /// Some DRM fdinfo read was denied, so the fdinfo-based engines count only the processes
    /// tasklens can read (PERF-074 "Own processes only"). Always false for NVIDIA.
    pub fdinfo_partial: bool,
    /// The device's static values, shared by every sample until the next `Hello` or card scan.
    pub info: Arc<GpuStaticInfo>,
}

bitflags::bitflags! {
    /// Reasons the GPU clocks are held down (PERF-066), from NVML
    /// `nvmlDeviceGetCurrentClocksEventReasons`. Bit values are frozen.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct ClockEventReasons: u16 {
        /// Nothing runs on the GPU.
        const IDLE = 1 << 0;
        /// The applications clocks setting limits the clocks.
        const APP_CLOCKS = 1 << 1;
        /// The software power cap.
        const POWER_CAP = 1 << 2;
        /// Hardware slowdown: high temperature, an external power brake or a power spike.
        const HW_SLOWDOWN = 1 << 3;
        /// Another GPU in the sync boost group holds this one down.
        const SYNC_BOOST = 1 << 4;
        /// Software thermal slowdown.
        const SW_THERMAL = 1 << 5;
        /// Hardware thermal slowdown.
        const HW_THERMAL = 1 << 6;
        /// Hardware power brake slowdown.
        const HW_POWER_BRAKE = 1 << 7;
        /// The display clock setting limits the clocks.
        const DISPLAY_CLOCKS = 1 << 8;
    }
}

/// One GPU engine (PERF-063). Each engine has a fixed key, stored in
/// `performance.hidden_gpu_engines`, and a fixed series. Display order and names are per vendor
/// and belong to the UI.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GpuEngine {
    /// NVIDIA 3D / Compute, AMD Graphics. Same value as `GpuTotals.util_pct`.
    Gpu,
    /// NVIDIA and AMD memory controller. Same value as `GpuTotals.mem_util_pct`.
    MemCtl,
    /// NVIDIA and AMD video encode. Same value as `GpuTotals.enc_pct`.
    Enc,
    /// NVIDIA and AMD video decode. Same value as `GpuTotals.dec_pct`.
    Dec,
    Jpeg,
    /// NVIDIA optical flow accelerator.
    Ofa,
    /// AMD and Intel compute.
    Compute,
    /// AMD SDMA.
    Dma,
    /// Intel Render/3D.
    Render,
    /// Intel copy (blitter).
    Copy,
    /// Intel video, which both encodes and decodes.
    Video,
    /// Intel video-enhance (Video Processing).
    VideoEnhance,
}

impl GpuEngine {
    pub const COUNT: usize = 12;

    pub const ALL: [GpuEngine; Self::COUNT] = [
        Self::Gpu,
        Self::MemCtl,
        Self::Enc,
        Self::Dec,
        Self::Jpeg,
        Self::Ofa,
        Self::Compute,
        Self::Dma,
        Self::Render,
        Self::Copy,
        Self::Video,
        Self::VideoEnhance,
    ];

    /// The frozen settings key. "gpu", "enc" and "dec" are the keys of the first engine graphs.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Gpu => "gpu",
            Self::MemCtl => "memctl",
            Self::Enc => "enc",
            Self::Dec => "dec",
            Self::Jpeg => "jpeg",
            Self::Ofa => "ofa",
            Self::Compute => "compute",
            Self::Dma => "dma",
            Self::Render => "render",
            Self::Copy => "copy",
            Self::Video => "video",
            Self::VideoEnhance => "video-enhance",
        }
    }

    pub fn from_key(key: &str) -> Option<GpuEngine> {
        Self::ALL.into_iter().find(|e| e.key() == key)
    }

    /// Older keys that also hide this engine (R-292). Before hjn.37 an Intel GPU drew "gpu",
    /// "enc" and "dec" graphs: "gpu" is now Render and "enc" with "dec" is now Video. Render and
    /// Video are Intel only, so these keys still name Gpu, Enc and Dec on NVIDIA and AMD.
    pub const fn legacy_keys(self) -> &'static [&'static str] {
        match self {
            Self::Render => &["gpu"],
            Self::Video => &["enc", "dec"],
            _ => &[],
        }
    }

    /// True when `hidden` (`performance.hidden_gpu_engines`) hides this engine: it holds the
    /// engine's key, or every one of its legacy keys. Video needs both "enc" and "dec", as the
    /// shared encode/decode row did before.
    pub fn is_hidden<S: AsRef<str>>(self, hidden: &[S]) -> bool {
        let has = |key: &str| hidden.iter().any(|k| k.as_ref() == key);
        let legacy = self.legacy_keys();
        has(self.key()) || (!legacy.is_empty() && legacy.iter().all(|k| has(k)))
    }

    /// Hides or shows this engine in `hidden`. Showing also removes its legacy keys, so a stored
    /// "gpu" does not keep Render hidden. On a computer with an Intel GPU and an NVIDIA or AMD
    /// GPU, that also shows the other GPU's engine of the same key.
    pub fn set_hidden(self, hidden: &mut Vec<String>, hide: bool) {
        if hide {
            if !self.is_hidden(hidden) {
                hidden.push(self.key().to_owned());
            }
        } else {
            let legacy = self.legacy_keys();
            hidden.retain(|k| k != self.key() && !legacy.contains(&k.as_str()));
        }
    }

    /// The series of this engine.
    pub const fn metric(self) -> MetricId {
        match self {
            Self::Gpu => MetricId::GPU_UTIL,
            Self::MemCtl => MetricId::GPU_ENGINE_MEMCTL,
            Self::Enc => MetricId::GPU_ENC,
            Self::Dec => MetricId::GPU_DEC,
            Self::Jpeg => MetricId::GPU_ENGINE_JPEG,
            Self::Ofa => MetricId::GPU_ENGINE_OFA,
            Self::Compute => MetricId::GPU_ENGINE_COMPUTE,
            Self::Dma => MetricId::GPU_ENGINE_DMA,
            Self::Render => MetricId::GPU_ENGINE_RENDER,
            Self::Copy => MetricId::GPU_ENGINE_COPY,
            Self::Video => MetricId::GPU_ENGINE_VIDEO,
            Self::VideoEnhance => MetricId::GPU_ENGINE_VIDEO_ENHANCE,
        }
    }
}

/// Busy percent per `GpuEngine`, indexed by the enum. `Unsupported` marks an engine the device
/// does not report; the UI does not list it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuEngines([Metric<f32>; GpuEngine::COUNT]);

impl GpuEngines {
    /// No engine reported.
    pub fn none() -> Self {
        Self([Metric::with(Validity::Unsupported); GpuEngine::COUNT])
    }

    pub fn get(&self, engine: GpuEngine) -> Metric<f32> {
        self.0[engine as usize]
    }

    pub fn set(&mut self, engine: GpuEngine, value: Metric<f32>) {
        self.0[engine as usize] = value;
    }

    pub fn is_reported(&self, engine: GpuEngine) -> bool {
        self.get(engine).validity != Validity::Unsupported
    }

    /// The reported engines with their values, in `GpuEngine::ALL` order.
    pub fn reported(&self) -> impl Iterator<Item = (GpuEngine, Metric<f32>)> + '_ {
        GpuEngine::ALL
            .into_iter()
            .map(|e| (e, self.get(e)))
            .filter(|(_, m)| m.validity != Validity::Unsupported)
    }
}

impl Default for GpuEngines {
    fn default() -> Self {
        Self::none()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SensorKind {
    Temperature,
    Fan,
}

/// One per hwmon temperature or fan device (PERF-076).
#[derive(Clone, Copy, Debug)]
pub struct SensorReading {
    pub device: DeviceIdx,
    pub kind: SensorKind,
    /// °C for Temperature, RPM for Fan.
    pub value: Metric<f32>,
    /// Fan only: pwmN / 255 × 100; Unsupported for Temperature.
    pub pwm_pct: Metric<f32>,
    /// Fan only: pwm<n>_enable; None for Temperature or no pwm file (R-139).
    pub fan_control: Option<FanControlMode>,
    /// Fan only: true once the fan reported a non-zero RPM this session or carries a label (R-170).
    pub fan_connected: bool,
    /// Fan only: `fan<n>_max` in RPM when the chip reports it; None for Temperature or no file.
    pub fan_max_rpm: Option<u32>,
}

/// hwmon `pwm<n>_enable`: 0 FullSpeed, 1 Manual, 2 Automatic (R-139). Other values give None.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum FanControlMode {
    FullSpeed,
    Manual,
    Automatic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn set_of(cpus: &[u16]) -> CpuSet {
        let mut s = CpuSet::empty();
        for &c in cpus {
            s.insert(c);
        }
        s
    }

    #[test]
    fn cpuset_insert_contains_iter() {
        let s = set_of(&[0, 63, 64, 1023]);
        assert_eq!(s.count(), 4);
        assert!(s.contains(64));
        assert!(!s.contains(65));
        assert!(!s.is_empty());
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 63, 64, 1023]);
    }

    #[test]
    fn cpuset_ignores_out_of_range() {
        let mut s = set_of(&[5]);
        let before = s;
        s.insert(1024);
        s.insert(u16::MAX);
        assert_eq!(s, before);
        s.remove(1024);
        assert_eq!(s, before);
        assert!(!s.contains(1024));
    }

    #[test]
    fn cpuset_intersection() {
        let a = set_of(&[0, 1, 2]);
        let b = set_of(&[1, 2, 3]);
        assert_eq!(a.intersection(&b), set_of(&[1, 2]));
    }

    #[test]
    fn metric_with_validity() {
        let m = Metric::<f32>::with(Validity::Restricted);
        assert_eq!(m.value, 0.0);
        assert_eq!(m.validity, Validity::Restricted);
        let v = Metric::valid(7u32);
        assert_eq!((v.value, v.validity), (7, Validity::Valid));
    }

    #[test]
    fn metric_default_warming_up() {
        let m = Metric::<u64>::default();
        assert_eq!(m.validity, Validity::WarmingUp);
    }

    #[test]
    fn topology_default_empty() {
        let t = CpuTopology::default();
        assert!(t.online.is_empty());
        assert!(t.cores.is_empty());
    }

    #[test]
    fn link_state_default_unknown() {
        assert_eq!(LinkState::default(), LinkState::Unknown);
    }

    #[test]
    fn gpu_engine_keys_and_series() {
        let keys: Vec<&str> = GpuEngine::ALL.iter().map(|e| e.key()).collect();
        assert_eq!(
            keys,
            [
                "gpu",
                "memctl",
                "enc",
                "dec",
                "jpeg",
                "ofa",
                "compute",
                "dma",
                "render",
                "copy",
                "video",
                "video-enhance"
            ]
        );
        for (i, e) in GpuEngine::ALL.into_iter().enumerate() {
            assert_eq!(e as usize, i);
            assert_eq!(GpuEngine::from_key(e.key()), Some(e));
        }
        assert_eq!(GpuEngine::from_key("engine"), None);
        // The stored keys of the first engine graphs keep their series.
        assert_eq!(GpuEngine::Gpu.metric(), MetricId::GPU_UTIL);
        assert_eq!(GpuEngine::Enc.metric(), MetricId::GPU_ENC);
        assert_eq!(GpuEngine::Dec.metric(), MetricId::GPU_DEC);
        let metrics: BTreeSet<u16> = GpuEngine::ALL.iter().map(|e| e.metric().0).collect();
        assert_eq!(metrics.len(), GpuEngine::COUNT);
    }

    #[test]
    fn gpu_engine_stored_hidden_keys_keep_working() {
        use GpuEngine::*;
        let hidden = |keys: &[&str]| -> Vec<GpuEngine> {
            GpuEngine::ALL
                .into_iter()
                .filter(|e| e.is_hidden(keys))
                .collect()
        };
        // NVIDIA and AMD: the keys name the same engines as before.
        assert_eq!(hidden(&["gpu"]), [Gpu, Render]);
        assert_eq!(hidden(&["enc"]), [Enc]);
        assert_eq!(hidden(&["dec"]), [Dec]);
        // Intel: "gpu" hides Render; Video only when both "enc" and "dec" are hidden.
        assert_eq!(hidden(&["enc", "dec"]), [Enc, Dec, Video]);
        assert_eq!(
            hidden(&["gpu", "enc", "dec"]),
            [Gpu, Enc, Dec, Render, Video]
        );
        // The legacy aliases point only at Intel engines, so NVIDIA and AMD lists never change.
        for e in GpuEngine::ALL {
            for k in e.legacy_keys() {
                assert!(matches!(e, Render | Video), "{e:?} has legacy key {k}");
            }
        }
        assert!(hidden(&[] as &[&str]).is_empty());
        assert_eq!(hidden(&["video-enhance"]), [VideoEnhance]);
    }

    #[test]
    fn gpu_engine_set_hidden_clears_legacy_keys() {
        let mut keys: Vec<String> = vec!["gpu".into(), "enc".into(), "dec".into()];
        // Already hidden through "gpu": no new key.
        GpuEngine::Render.set_hidden(&mut keys, true);
        assert_eq!(keys, ["gpu", "enc", "dec"]);
        GpuEngine::Render.set_hidden(&mut keys, false);
        assert!(!GpuEngine::Render.is_hidden(&keys));
        assert_eq!(keys, ["enc", "dec"]);
        GpuEngine::Video.set_hidden(&mut keys, false);
        assert!(!GpuEngine::Video.is_hidden(&keys));
        assert!(keys.is_empty());
        GpuEngine::Video.set_hidden(&mut keys, true);
        assert_eq!(keys, ["video"]);
        assert!(GpuEngine::Video.is_hidden(&keys));
        assert!(!GpuEngine::Enc.is_hidden(&keys));
        GpuEngine::Video.set_hidden(&mut keys, false);
        assert!(keys.is_empty());
    }

    #[test]
    fn gpu_engines_report_only_set_engines() {
        let mut e = GpuEngines::none();
        assert_eq!(e.reported().count(), 0);
        e.set(GpuEngine::Dma, Metric::valid(12.0));
        e.set(GpuEngine::Gpu, Metric::with(Validity::WarmingUp));
        let got: Vec<GpuEngine> = e.reported().map(|(k, _)| k).collect();
        assert_eq!(got, [GpuEngine::Gpu, GpuEngine::Dma]);
        assert!(e.is_reported(GpuEngine::Gpu));
        assert!(!e.is_reported(GpuEngine::Jpeg));
        assert_eq!(e.get(GpuEngine::Dma), Metric::valid(12.0));
    }

    proptest! {
        #[test]
        fn cpuset_matches_btreeset(ops in proptest::collection::vec((any::<bool>(), 0u16..1100), 0..200)) {
            let mut set = CpuSet::empty();
            let mut model = BTreeSet::new();
            for (add, cpu) in ops {
                if add {
                    set.insert(cpu);
                    if cpu < CpuSet::MAX_CPUS {
                        model.insert(cpu);
                    }
                } else {
                    set.remove(cpu);
                    model.remove(&cpu);
                }
            }
            prop_assert_eq!(set.iter().collect::<Vec<_>>(), model.iter().copied().collect::<Vec<_>>());
            prop_assert_eq!(set.count() as usize, model.len());
            prop_assert_eq!(set.is_empty(), model.is_empty());
            for cpu in 0..1100u16 {
                prop_assert_eq!(set.contains(cpu), model.contains(&cpu));
            }
        }
    }
}
