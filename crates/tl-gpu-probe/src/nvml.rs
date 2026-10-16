//! The NVML backend (ADR-007, PERF-062, PERF-071).
//!
//! `hello` starts NVML and lists the devices. `sample` reads one set of metrics per device
//! (E13-003) and, when asked, one row per GPU process (E13-004).

use std::collections::BTreeMap;

use nvml_wrapper::enum_wrappers::device::{
    Clock, PcieUtilCounter, PerformanceState, TemperatureSensor, TemperatureThreshold,
};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::struct_wrappers::device::{MemoryInfo, ProcessInfo, ProcessUtilizationSample};
use nvml_wrapper::{Device, Nvml, cuda_driver_version_major, cuda_driver_version_minor};
use tl_model::gpu_proto::{GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, Reading};

use crate::sampler::{Backend, HelloInfo, InitFailed, now_boottime_ns};

/// One device that `hello` listed. NVML device handles borrow `Nvml`, so the slot keeps the index.
struct DeviceSlot {
    index: u32,
    uuid: [u8; 16],
    /// NVML timestamp of the newest process sample seen, for `process_utilization_stats`.
    last_process_ts: u64,
}

#[derive(Default)]
pub struct NvmlBackend {
    /// `None` until `hello` starts NVML.
    nvml: Option<Nvml>,
    devices: Vec<DeviceSlot>,
}

impl NvmlBackend {
    /// Does not start NVML: `hello` does, so the engine's 3 s Hello deadline covers `nvmlInit`.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Parses `"GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"` into 16 bytes. `"MIG-…"` and any other
/// form give `None`.
pub fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let rest = s.strip_prefix("GPU-")?;
    let groups: Vec<&str> = rest.split('-').collect();
    let lens: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    if lens != [8, 4, 4, 4, 12] {
        return None;
    }
    let hex: Vec<u8> = groups.concat().into_bytes();
    let mut out = [0u8; 16];
    for (byte, pair) in out.iter_mut().zip(hex.as_chunks::<2>().0) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        *byte = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// Maps an `nvmlInit` error to the ADR-007 kind. The message is the error's own text.
pub fn classify_init_error(e: &NvmlError) -> (InitFailure, String) {
    let kind = match e {
        NvmlError::LibraryNotFound | NvmlError::LibloadingError(_) => InitFailure::LibraryNotFound,
        NvmlError::DriverNotLoaded => InitFailure::DriverNotLoaded,
        NvmlError::NoPermission => InitFailure::NoPermission,
        _ => InitFailure::Other,
    };
    (kind, e.to_string())
}

fn init_failed(e: &NvmlError) -> InitFailed {
    let (kind, message) = classify_init_error(e);
    InitFailed { kind, message }
}

/// Reads one device's identity. Any read error skips the device; the optional PERF-071 fields
/// become `None` when their call fails.
fn read_device(nvml: &Nvml, index: u32, cuda: Option<&str>) -> Option<GpuDeviceInfo> {
    let device = nvml.device_by_index(index).ok()?;
    let uuid = parse_uuid(&device.uuid().ok()?)?;
    let power_limit_mw = match device.power_management_limit() {
        Ok(mw) => Some(mw),
        Err(NvmlError::NotSupported) => None,
        Err(_) => return None,
    };
    Some(GpuDeviceInfo {
        uuid,
        name: device.name().ok()?,
        pci_bus_id: device.pci_info().ok()?.bus_id,
        mem_total: device.memory_info().ok()?.total,
        power_limit_mw,
        cuda_version: cuda.map(str::to_owned),
        vbios: device.vbios_version().ok(),
        architecture: device.architecture().ok().map(|a| a.to_string()),
        clock_gfx_max_mhz: device.max_clock_info(Clock::Graphics).ok(),
        clock_mem_max_mhz: device.max_clock_info(Clock::Memory).ok(),
        power_max_limit_mw: device
            .power_management_limit_constraints()
            .ok()
            .map(|c| c.max_limit),
        slowdown_temp_c: device
            .temperature_threshold(TemperatureThreshold::Slowdown)
            .ok(),
        shutdown_temp_c: device
            .temperature_threshold(TemperatureThreshold::Shutdown)
            .ok(),
        // nvmlDeviceGetGpuMaxPcieLinkGeneration (the card's own maximum) has no safe wrapper,
        // and this file may not hold `unsafe` (R-143). The engine prefers sysfs `max_link_speed`.
        pcie_max_gen: device.max_pcie_link_gen().ok(),
        pcie_max_width: device.max_pcie_link_width().ok(),
    })
}

/// P0 to P15 as 0 to 15. NVML's "unknown" state is `Unsupported`.
pub fn pstate_reading(r: Reading<PerformanceState>) -> Reading<u8> {
    let n = match r {
        Reading::Ok(state) => match state {
            PerformanceState::Zero => 0,
            PerformanceState::One => 1,
            PerformanceState::Two => 2,
            PerformanceState::Three => 3,
            PerformanceState::Four => 4,
            PerformanceState::Five => 5,
            PerformanceState::Six => 6,
            PerformanceState::Seven => 7,
            PerformanceState::Eight => 8,
            PerformanceState::Nine => 9,
            PerformanceState::Ten => 10,
            PerformanceState::Eleven => 11,
            PerformanceState::Twelve => 12,
            PerformanceState::Thirteen => 13,
            PerformanceState::Fourteen => 14,
            PerformanceState::Fifteen => 15,
            PerformanceState::Unknown => return Reading::Unsupported,
        },
        Reading::Unsupported => return Reading::Unsupported,
        Reading::Failed => return Reading::Failed,
    };
    Reading::Ok(n)
}

/// Maps one NVML call result to a `Reading`. `NotSupported` is `Unsupported`; every other error
/// is `Failed`, so a missing metric is never zero.
pub fn reading<T>(r: Result<T, NvmlError>) -> Reading<T> {
    match r {
        Ok(v) => Reading::Ok(v),
        Err(NvmlError::NotSupported) => Reading::Unsupported,
        Err(_) => Reading::Failed,
    }
}

/// Used and hardware reserved bytes from one `nvmlDeviceGetMemoryInfo_v2` call (PERF-064).
pub fn memory_readings(r: Reading<MemoryInfo>) -> (Reading<u64>, Reading<u64>) {
    match r {
        Reading::Ok(m) => (Reading::Ok(m.used), Reading::Ok(m.reserved)),
        Reading::Unsupported => (Reading::Unsupported, Reading::Unsupported),
        Reading::Failed => (Reading::Failed, Reading::Failed),
    }
}

/// Reads one device's metrics for a sample (PERF-063 to PERF-070). Each call is independent, so
/// one failed metric never skips the others.
pub fn device_sample(dev: &Device<'_>, uuid: [u8; 16]) -> GpuDeviceSample {
    // One call fills both utilisation fields, so they share its `Reading`.
    let (util_pct, mem_util_pct) = match reading(dev.utilization_rates()) {
        Reading::Ok(u) => (Reading::Ok(u.gpu), Reading::Ok(u.memory)),
        Reading::Unsupported => (Reading::Unsupported, Reading::Unsupported),
        Reading::Failed => (Reading::Failed, Reading::Failed),
    };
    let (mem_used, mem_reserved) = memory_readings(reading(dev.memory_info()));
    GpuDeviceSample {
        uuid,
        util_pct,
        mem_util_pct,
        mem_used,
        mem_reserved,
        enc_pct: reading(dev.encoder_utilization().map(|u| u.utilization)),
        dec_pct: reading(dev.decoder_utilization().map(|u| u.utilization)),
        // The session count of nvmlDeviceGetEncoderStats, the same count
        // nvmlDeviceGetEncoderSessions returns, without the session list.
        enc_sessions: reading(dev.encoder_stats().map(|s| s.session_count)),
        temp_c: reading(dev.temperature(TemperatureSensor::Gpu)),
        power_mw: reading(dev.power_usage()),
        clock_gfx_mhz: reading(dev.clock_info(Clock::Graphics)),
        clock_mem_mhz: reading(dev.clock_info(Clock::Memory)),
        clock_video_mhz: reading(dev.clock_info(Clock::Video)),
        pstate: pstate_reading(reading(dev.performance_state())),
        fan_pct: reading(dev.fan_speed(0)),
        fan_rpm: reading(dev.fan_speed_rpm(0)),
        // NVML reports KB/s measured over a 20 ms window, and each call blocks for that window.
        // The two calls run one after the other, so they add about 42 ms to a sample (R-277).
        pcie_rx_kbps: reading(dev.pcie_throughput(PcieUtilCounter::Receive)),
        pcie_tx_kbps: reading(dev.pcie_throughput(PcieUtilCounter::Send)),
        pcie_gen: reading(dev.current_pcie_link_gen()),
        pcie_width: reading(dev.current_pcie_link_width()),
        energy_mj: reading(dev.total_energy_consumption()),
    }
}

/// Reads one device's process rows (E13-004, ADR-007). `last_seen` is the NVML timestamp of the
/// newest utilisation sample seen so far; it moves to the newest one this call returns.
pub fn process_samples(
    dev: &Device<'_>,
    uuid: [u8; 16],
    last_seen: &mut u64,
) -> Vec<GpuProcessSample> {
    let (util, util_supported) = match dev.process_utilization_stats(*last_seen) {
        Ok(v) => {
            if let Some(newest) = v.iter().map(|s| s.timestamp).max() {
                *last_seen = newest;
            }
            (v, Reading::Ok(()))
        }
        Err(NvmlError::NotSupported) => (Vec::new(), Reading::Unsupported),
        // No process ran on the device since `last_seen`.
        Err(NvmlError::NotFound) => (Vec::new(), Reading::Ok(())),
        Err(_) => (Vec::new(), Reading::Failed),
    };
    let graphics = dev.running_graphics_processes();
    let compute = dev.running_compute_processes();
    let mem_failed = graphics.is_err() || compute.is_err();
    let mut rows = merge_process_rows(
        &util,
        graphics.as_deref().unwrap_or_default(),
        compute.as_deref().unwrap_or_default(),
        uuid,
        util_supported,
    );
    // A sum over one list only would show too little memory, so a failed list fails every row.
    if mem_failed {
        for row in &mut rows {
            row.mem_used = Reading::Failed;
        }
    }
    rows
}

/// Merges one device's utilisation samples and process lists into one row per pid, in pid order.
/// Pid 0 is dropped. Device utilisation is never divided among processes (ADR-007).
pub fn merge_process_rows(
    util: &[ProcessUtilizationSample],
    graphics: &[ProcessInfo],
    compute: &[ProcessInfo],
    uuid: [u8; 16],
    util_supported: Reading<()>,
) -> Vec<GpuProcessSample> {
    // Per pid: the newest utilisation sample, and the memory sum (`None` once an entry is
    // `Unavailable`, or when the pid is in neither list).
    struct Acc<'a> {
        util: Option<&'a ProcessUtilizationSample>,
        mem: Option<Option<u64>>,
    }
    let mut by_pid: BTreeMap<u32, Acc<'_>> = BTreeMap::new();
    for s in util {
        let acc = by_pid.entry(s.pid).or_insert(Acc {
            util: None,
            mem: None,
        });
        if acc.util.is_none_or(|old| s.timestamp > old.timestamp) {
            acc.util = Some(s);
        }
    }
    for p in graphics.iter().chain(compute) {
        let acc = by_pid.entry(p.pid).or_insert(Acc {
            util: None,
            mem: None,
        });
        let used = match p.used_gpu_memory {
            UsedGpuMemory::Used(bytes) => Some(bytes),
            UsedGpuMemory::Unavailable => None,
        };
        acc.mem = Some(match acc.mem {
            None => used,
            Some(sum) => sum.zip(used).map(|(a, b)| a.saturating_add(b)),
        });
    }
    by_pid.remove(&0);

    // The reading for a pid without a utilisation sample: idle when supported, else the state.
    let no_sample = match util_supported {
        Reading::Ok(()) => Reading::Ok(0),
        Reading::Unsupported => Reading::Unsupported,
        Reading::Failed => Reading::Failed,
    };
    by_pid
        .into_iter()
        .map(|(pid, acc)| {
            let pct = |f: fn(&ProcessUtilizationSample) -> u32| {
                acc.util.map_or(no_sample, |s| Reading::Ok(f(s)))
            };
            GpuProcessSample {
                uuid,
                pid,
                sm_pct: pct(|s| s.sm_util),
                mem_pct: pct(|s| s.mem_util),
                enc_pct: pct(|s| s.enc_util),
                dec_pct: pct(|s| s.dec_util),
                mem_used: match acc.mem {
                    Some(Some(bytes)) => Reading::Ok(bytes),
                    Some(None) | None => Reading::Unsupported,
                },
            }
        })
        .collect()
}

/// The sample for a device whose handle could not be read: every metric is `Failed`.
fn failed_sample(uuid: [u8; 16]) -> GpuDeviceSample {
    GpuDeviceSample {
        uuid,
        util_pct: Reading::Failed,
        mem_util_pct: Reading::Failed,
        mem_used: Reading::Failed,
        mem_reserved: Reading::Failed,
        enc_pct: Reading::Failed,
        dec_pct: Reading::Failed,
        enc_sessions: Reading::Failed,
        temp_c: Reading::Failed,
        power_mw: Reading::Failed,
        clock_gfx_mhz: Reading::Failed,
        clock_mem_mhz: Reading::Failed,
        clock_video_mhz: Reading::Failed,
        pstate: Reading::Failed,
        fan_pct: Reading::Failed,
        fan_rpm: Reading::Failed,
        pcie_rx_kbps: Reading::Failed,
        pcie_tx_kbps: Reading::Failed,
        pcie_gen: Reading::Failed,
        pcie_width: Reading::Failed,
        energy_mj: Reading::Failed,
    }
}

impl NvmlBackend {
    /// One `GpuDeviceSample` per slot, in index order. Empty before `hello`.
    fn device_samples(&self) -> Vec<GpuDeviceSample> {
        let Some(nvml) = &self.nvml else {
            return Vec::new();
        };
        self.devices
            .iter()
            .map(|slot| match nvml.device_by_index(slot.index) {
                Ok(dev) => device_sample(&dev, slot.uuid),
                Err(_) => failed_sample(slot.uuid),
            })
            .collect()
    }

    /// The process rows of every slot, in slot order. Moves each slot's `last_process_ts`.
    fn process_rows(&mut self) -> Vec<GpuProcessSample> {
        let Some(nvml) = &self.nvml else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for slot in &mut self.devices {
            // A device whose handle cannot be read this sample has no rows.
            if let Ok(dev) = nvml.device_by_index(slot.index) {
                rows.extend(process_samples(&dev, slot.uuid, &mut slot.last_process_ts));
            }
        }
        rows
    }
}

impl Backend for NvmlBackend {
    fn hello(&mut self) -> Result<HelloInfo, InitFailed> {
        let nvml = match self.nvml.take() {
            Some(nvml) => nvml,
            None => Nvml::init().map_err(|e| init_failed(&e))?,
        };
        let nvml = &*self.nvml.insert(nvml);
        let driver = nvml.sys_driver_version().map_err(|e| init_failed(&e))?;
        let nvml_version = nvml.sys_nvml_version().map_err(|e| init_failed(&e))?;
        // The CUDA version the driver supports, as "major.minor" (for example "13.0").
        let cuda = nvml.sys_cuda_driver_version().ok().map(|v| {
            format!(
                "{}.{}",
                cuda_driver_version_major(v),
                cuda_driver_version_minor(v)
            )
        });
        let count = nvml.device_count().map_err(|e| init_failed(&e))?;

        self.devices.clear();
        let mut devices = Vec::new();
        for index in 0..count {
            if let Some(info) = read_device(nvml, index, cuda.as_deref()) {
                self.devices.push(DeviceSlot {
                    index,
                    uuid: info.uuid,
                    last_process_ts: 0,
                });
                devices.push(info);
            }
        }
        Ok(HelloInfo {
            driver,
            nvml: nvml_version,
            devices,
        })
    }

    fn sample(
        &mut self,
        _seq: u32,
        processes: bool,
    ) -> (u64, Vec<GpuDeviceSample>, Option<Vec<GpuProcessSample>>) {
        let t_ns = now_boottime_ns();
        let devices = self.device_samples();
        let rows = processes.then(|| self.process_rows());
        (t_ns, devices, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_table() {
        assert_eq!(
            parse_uuid("GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"),
            Some([
                0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x60, 0x71, 0x82, 0x93, 0xa4, 0xb5, 0xc6, 0xd7,
                0xe8, 0xf9
            ])
        );
        assert_eq!(
            parse_uuid("GPU-0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9"),
            parse_uuid("GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9")
        );
        assert_eq!(parse_uuid("MIG-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"), None);
        assert_eq!(parse_uuid("GPU-zz"), None);
        assert_eq!(parse_uuid(""), None);
        assert_eq!(parse_uuid("GPU-"), None);
        // Right lengths, one non-hex digit.
        assert_eq!(parse_uuid("GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8fg"), None);
        // Group lengths moved around.
        assert_eq!(parse_uuid("GPU-0a1b2c3d4-e5f-6071-8293-a4b5c6d7e8f9"), None);
        // A sixth group.
        assert_eq!(
            parse_uuid("GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9-00"),
            None
        );
        // Multi-byte characters must not panic.
        assert_eq!(
            parse_uuid("GPU-ééééé ééé-4e5f-6071-8293-a4b5c6d7e8f9"),
            None
        );
    }

    #[test]
    fn classify_init_error_table() {
        let table = [
            (NvmlError::LibraryNotFound, InitFailure::LibraryNotFound),
            (NvmlError::DriverNotLoaded, InitFailure::DriverNotLoaded),
            (NvmlError::NoPermission, InitFailure::NoPermission),
            (NvmlError::Unknown, InitFailure::Other),
            (NvmlError::NotSupported, InitFailure::Other),
        ];
        for (err, want) in table {
            let (kind, message) = classify_init_error(&err);
            assert_eq!(kind, want, "{err:?}");
            assert_eq!(message, err.to_string());
        }
    }

    #[test]
    #[ignore = "owner machine with an RTX 5070"]
    fn real_hello_5070() {
        let mut backend = NvmlBackend::new();
        let info = match backend.hello() {
            Ok(info) => info,
            Err(e) => panic!("hello failed: {:?} {}", e.kind, e.message),
        };
        eprintln!("driver {} nvml {}", info.driver, info.nvml);
        assert_eq!(info.devices.len(), 1);
        let d = &info.devices[0];
        eprintln!(
            "name {:?} bus {} mem_total {} power_limit_mw {:?} cuda {:?} vbios {:?} arch {:?}",
            d.name,
            d.pci_bus_id,
            d.mem_total,
            d.power_limit_mw,
            d.cuda_version,
            d.vbios,
            d.architecture
        );
        eprintln!("{d:?}");
        assert!(d.clock_gfx_max_mhz.is_some());
        assert!(d.pcie_max_width.is_some());
        // U-35: the power limit NVML reports on the owner machine.
        eprintln!("U-35 power_limit_mw = {:?}", d.power_limit_mw);
        assert!(d.name.contains("5070"), "{}", d.name);
        assert_ne!(d.uuid, [0u8; 16]);
        assert!(d.pci_bus_id.ends_with("2D:00.0"), "{}", d.pci_bus_id);
        assert!(d.mem_total > 8 << 30, "{}", d.mem_total);
        assert!(d.power_limit_mw.is_some());
        assert_eq!(backend.devices.len(), 1);
        assert_eq!(backend.devices[0].uuid, d.uuid);
    }

    #[test]
    fn reading_mapping() {
        assert_eq!(reading(Ok(5u32)), Reading::Ok(5));
        assert_eq!(
            reading::<u32>(Err(NvmlError::NotSupported)),
            Reading::Unsupported
        );
        assert_eq!(reading::<u32>(Err(NvmlError::Unknown)), Reading::Failed);
        assert_eq!(reading::<u32>(Err(NvmlError::GpuLost)), Reading::Failed);
        assert_eq!(
            reading::<u64>(Err(NvmlError::NoPermission)),
            Reading::Failed
        );
    }

    #[test]
    fn memory_readings_split() {
        let info = MemoryInfo {
            free: 0,
            reserved: 380 << 20,
            total: 16 << 30,
            used: 3 << 30,
            version: 2,
        };
        assert_eq!(
            memory_readings(Reading::Ok(info)),
            (Reading::Ok(3 << 30), Reading::Ok(380 << 20))
        );
        assert_eq!(
            memory_readings(Reading::Unsupported),
            (Reading::Unsupported, Reading::Unsupported)
        );
        assert_eq!(
            memory_readings(Reading::Failed),
            (Reading::Failed, Reading::Failed)
        );
    }

    #[test]
    fn failed_sample_has_no_new_value() {
        let s = failed_sample([1; 16]);
        assert_eq!(s.mem_reserved, Reading::Failed);
        assert_eq!(s.enc_sessions, Reading::Failed);
        assert_eq!(s.fan_rpm, Reading::Failed);
        assert_eq!(s.clock_video_mhz, Reading::Failed);
        assert_eq!(s.pstate, Reading::Failed);
        assert_eq!(s.pcie_gen, Reading::Failed);
        assert_eq!(s.pcie_width, Reading::Failed);
        assert_eq!(s.energy_mj, Reading::Failed);
    }

    #[test]
    fn pstate_mapping() {
        assert_eq!(
            pstate_reading(Reading::Ok(PerformanceState::Zero)),
            Reading::Ok(0)
        );
        assert_eq!(
            pstate_reading(Reading::Ok(PerformanceState::Three)),
            Reading::Ok(3)
        );
        assert_eq!(
            pstate_reading(Reading::Ok(PerformanceState::Fifteen)),
            Reading::Ok(15)
        );
        assert_eq!(
            pstate_reading(Reading::Ok(PerformanceState::Unknown)),
            Reading::Unsupported
        );
        assert_eq!(pstate_reading(Reading::Unsupported), Reading::Unsupported);
        assert_eq!(pstate_reading(Reading::Failed), Reading::Failed);
    }

    /// Starts NVML and returns the backend after `hello`, with one device or more.
    fn real_backend() -> NvmlBackend {
        let mut backend = NvmlBackend::new();
        if let Err(e) = backend.hello() {
            panic!("hello failed: {:?} {}", e.kind, e.message);
        }
        assert!(!backend.devices.is_empty());
        backend
    }

    #[test]
    #[ignore = "owner machine with an RTX 5070"]
    fn real_device_sample_fields() {
        let backend = real_backend();
        let nvml = backend.nvml.as_ref().unwrap();
        let slot = &backend.devices[0];
        let dev = nvml.device_by_index(slot.index).unwrap();
        let mem_total = dev.memory_info().unwrap().total;
        let s = device_sample(&dev, slot.uuid);
        eprintln!("{s:?}");
        assert_eq!(s.uuid, slot.uuid);
        assert!(matches!(s.util_pct, Reading::Ok(_)), "{:?}", s.util_pct);
        assert!(matches!(s.power_mw, Reading::Ok(_)), "{:?}", s.power_mw);
        assert!(
            matches!(s.clock_gfx_mhz, Reading::Ok(_)),
            "{:?}",
            s.clock_gfx_mhz
        );
        assert!(
            matches!(s.clock_mem_mhz, Reading::Ok(_)),
            "{:?}",
            s.clock_mem_mhz
        );
        let Reading::Ok(temp) = s.temp_c else {
            panic!("temp_c {:?}", s.temp_c);
        };
        assert!((10..=110).contains(&temp), "{temp}");
        let Reading::Ok(used) = s.mem_used else {
            panic!("mem_used {:?}", s.mem_used);
        };
        assert!(used <= mem_total, "{used} > {mem_total}");
        let Reading::Ok(reserved) = s.mem_reserved else {
            panic!("mem_reserved {:?}", s.mem_reserved);
        };
        assert!(reserved < mem_total, "{reserved} >= {mem_total}");
        assert!(
            matches!(s.enc_sessions, Reading::Ok(_)),
            "{:?}",
            s.enc_sessions
        );
        assert!(matches!(s.pstate, Reading::Ok(0..=15)), "{:?}", s.pstate);
        assert!(matches!(s.pcie_gen, Reading::Ok(1..=6)), "{:?}", s.pcie_gen);
        assert!(matches!(s.pcie_width, Reading::Ok(_)), "{:?}", s.pcie_width);
        assert!(matches!(s.energy_mj, Reading::Ok(_)), "{:?}", s.energy_mj);
        // U-35: whether the owner card reports a fan speed.
        eprintln!("U-35 fan_pct = {:?}", s.fan_pct);
        assert!(
            matches!(s.fan_pct, Reading::Ok(_) | Reading::Unsupported),
            "{:?}",
            s.fan_pct
        );
    }

    #[test]
    #[ignore = "owner machine with an RTX 5070"]
    fn real_sample_under_100ms() {
        let mut backend = real_backend();
        for i in 0..2 {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let start = std::time::Instant::now();
            let (t_ns, devices, processes) = backend.sample(i, false);
            let took = start.elapsed();
            eprintln!("sample {i}: {took:?}, t_ns {t_ns}");
            assert_eq!(devices.len(), backend.devices.len());
            assert!(processes.is_none());
            assert!(took < std::time::Duration::from_millis(100), "{took:?}");
        }
    }

    const UUID: [u8; 16] = [7; 16];
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;

    fn util(pid: u32, timestamp: u64, sm: u32, mem: u32) -> ProcessUtilizationSample {
        ProcessUtilizationSample {
            pid,
            timestamp,
            sm_util: sm,
            mem_util: mem,
            enc_util: 0,
            dec_util: 0,
        }
    }

    fn proc_info(pid: u32, used: UsedGpuMemory) -> ProcessInfo {
        ProcessInfo {
            pid,
            used_gpu_memory: used,
            gpu_instance_id: None,
            compute_instance_id: None,
        }
    }

    #[test]
    fn merge_rows_sum_memory_and_newest_util() {
        let rows = merge_process_rows(
            &[util(200, 3, 5, 1), util(100, 4, 30, 10)],
            &[proc_info(100, UsedGpuMemory::Used(GIB))],
            &[
                proc_info(100, UsedGpuMemory::Used(512 * MIB)),
                proc_info(300, UsedGpuMemory::Unavailable),
            ],
            UUID,
            Reading::Ok(()),
        );
        let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [100, 200, 300]);
        assert_eq!(
            rows[0],
            GpuProcessSample {
                uuid: UUID,
                pid: 100,
                sm_pct: Reading::Ok(30),
                mem_pct: Reading::Ok(10),
                enc_pct: Reading::Ok(0),
                dec_pct: Reading::Ok(0),
                mem_used: Reading::Ok(GIB + 512 * MIB),
            }
        );
        assert_eq!(rows[1].sm_pct, Reading::Ok(5));
        assert_eq!(rows[1].mem_used, Reading::Unsupported);
        assert_eq!(rows[2].sm_pct, Reading::Ok(0));
        assert_eq!(rows[2].enc_pct, Reading::Ok(0));
        assert_eq!(rows[2].mem_used, Reading::Unsupported);
        assert!(rows.iter().all(|r| r.uuid == UUID));
    }

    #[test]
    fn merge_newest_sample_wins() {
        for samples in [
            [util(100, 5, 11, 1), util(100, 9, 22, 2)],
            [util(100, 9, 22, 2), util(100, 5, 11, 1)],
        ] {
            let rows = merge_process_rows(&samples, &[], &[], UUID, Reading::Ok(()));
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].sm_pct, Reading::Ok(22));
            assert_eq!(rows[0].mem_pct, Reading::Ok(2));
        }
    }

    #[test]
    fn merge_util_unsupported_keeps_memory() {
        for state in [Reading::Unsupported, Reading::Failed] {
            let rows = merge_process_rows(
                &[],
                &[proc_info(100, UsedGpuMemory::Used(GIB))],
                &[],
                UUID,
                state,
            );
            assert_eq!(rows.len(), 1);
            let want = match state {
                Reading::Unsupported => Reading::Unsupported,
                _ => Reading::Failed,
            };
            let r = &rows[0];
            assert_eq!(r.pid, 100);
            assert_eq!(
                [r.sm_pct, r.mem_pct, r.enc_pct, r.dec_pct],
                [want; 4],
                "{state:?}"
            );
            assert_eq!(r.mem_used, Reading::Ok(GIB));
        }
    }

    #[test]
    fn merge_drops_pid_zero() {
        let rows = merge_process_rows(
            &[util(0, 1, 50, 5), util(42, 1, 1, 1)],
            &[proc_info(0, UsedGpuMemory::Used(GIB))],
            &[proc_info(0, UsedGpuMemory::Used(GIB))],
            UUID,
            Reading::Ok(()),
        );
        let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [42]);
    }

    /// Kills and reaps the child the test started when it goes out of scope.
    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "owner machine with an RTX 5070 and a running GPU process"]
    fn real_process_utilization_u12() {
        let mut backend = real_backend();
        // With a display, start glxgears so a known pid is busy on the GPU. Without one, use a
        // process that already has a graphics context (the compositor, for example).
        let has_display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let child = has_display
            .then(|| {
                std::process::Command::new("glxgears")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok()
            })
            .flatten()
            .map(ChildGuard);
        if child.is_some() {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        let pid = match &child {
            Some(c) => c.0.id(),
            None => {
                let nvml = backend.nvml.as_ref().unwrap();
                let dev = nvml.device_by_index(backend.devices[0].index).unwrap();
                let running = dev.running_graphics_processes().unwrap_or_default();
                match running.iter().find(|p| p.pid != 0) {
                    Some(p) => p.pid,
                    None => {
                        eprintln!("skipped: no display for glxgears and no GPU process running");
                        return;
                    }
                }
            }
        };
        eprintln!("watching pid {pid} (glxgears: {})", child.is_some());

        let (_, _, first) = backend.sample(0, true);
        let ts_first = backend.devices[0].last_process_ts;
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (_, _, second) = backend.sample(1, true);
        let ts_second = backend.devices[0].last_process_ts;
        eprintln!("first {first:?}");
        eprintln!("second {second:?}");
        eprintln!("last_seen {ts_first} -> {ts_second}");

        let second = second.expect("processes: true gives Some");
        let row = second
            .iter()
            .find(|r| r.pid == pid)
            .unwrap_or_else(|| panic!("no row for pid {pid}"));
        assert!(matches!(row.sm_pct, Reading::Ok(_)), "{:?}", row.sm_pct);
        // U-12: per-process utilisation works unprivileged on this driver.
        eprintln!("U-12 verified: pid {pid} sm_pct {:?}", row.sm_pct);
        assert!(ts_second > ts_first, "{ts_first} -> {ts_second}");

        // `processes: false` leaves `last_seen` alone.
        let (_, _, none) = backend.sample(2, false);
        assert!(none.is_none());
        assert_eq!(backend.devices[0].last_process_ts, ts_second);
    }

    #[test]
    fn hello_without_library_is_init_failed() {
        match NvmlBackend::new().hello() {
            Err(e) => {
                assert_eq!(e.kind, InitFailure::LibraryNotFound, "{}", e.message);
            }
            Ok(_) => {
                eprintln!("NVML initialised on this machine; the missing-library case is skipped");
                // Still check that a library that cannot be loaded maps to LibraryNotFound.
                let err = Nvml::builder()
                    .lib_path("libnvidia-ml-tasklens-missing.so.1".as_ref())
                    .init()
                    .expect_err("a missing library must fail to load");
                assert_eq!(classify_init_error(&err).0, InitFailure::LibraryNotFound);
            }
        }
    }
}
