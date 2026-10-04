//! The NVML backend (ADR-007, PERF-062, PERF-071).
//!
//! `hello` starts NVML and lists the devices. `sample` reads one set of metrics per device
//! (E13-003).

use nvml_wrapper::enum_wrappers::device::{Clock, PcieUtilCounter, TemperatureSensor};
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::{Device, Nvml, cuda_driver_version_major, cuda_driver_version_minor};
use tl_model::gpu_proto::{GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, Reading};

use crate::sampler::{Backend, HelloInfo, InitFailed, now_boottime_ns};

/// One device that `hello` listed. NVML device handles borrow `Nvml`, so the slot keeps the index.
struct DeviceSlot {
    index: u32,
    uuid: [u8; 16],
    /// NVML timestamp of the newest process sample seen, for `process_utilization_stats`.
    // E13-004 (ndy.6) reads this field.
    #[allow(dead_code)]
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
    })
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

/// Reads one device's metrics for a sample (PERF-063 to PERF-070). Each call is independent, so
/// one failed metric never skips the others.
pub fn device_sample(dev: &Device<'_>, uuid: [u8; 16]) -> GpuDeviceSample {
    // One call fills both utilisation fields, so they share its `Reading`.
    let (util_pct, mem_util_pct) = match reading(dev.utilization_rates()) {
        Reading::Ok(u) => (Reading::Ok(u.gpu), Reading::Ok(u.memory)),
        Reading::Unsupported => (Reading::Unsupported, Reading::Unsupported),
        Reading::Failed => (Reading::Failed, Reading::Failed),
    };
    GpuDeviceSample {
        uuid,
        util_pct,
        mem_util_pct,
        mem_used: reading(dev.memory_info().map(|m| m.used)),
        enc_pct: reading(dev.encoder_utilization().map(|u| u.utilization)),
        dec_pct: reading(dev.decoder_utilization().map(|u| u.utilization)),
        temp_c: reading(dev.temperature(TemperatureSensor::Gpu)),
        power_mw: reading(dev.power_usage()),
        clock_gfx_mhz: reading(dev.clock_info(Clock::Graphics)),
        clock_mem_mhz: reading(dev.clock_info(Clock::Memory)),
        fan_pct: reading(dev.fan_speed(0)),
        // NVML reports KB/s over a 20 ms window.
        pcie_rx_kbps: reading(dev.pcie_throughput(PcieUtilCounter::Receive)),
        pcie_tx_kbps: reading(dev.pcie_throughput(PcieUtilCounter::Send)),
    }
}

/// The sample for a device whose handle could not be read: every metric is `Failed`.
fn failed_sample(uuid: [u8; 16]) -> GpuDeviceSample {
    GpuDeviceSample {
        uuid,
        util_pct: Reading::Failed,
        mem_util_pct: Reading::Failed,
        mem_used: Reading::Failed,
        enc_pct: Reading::Failed,
        dec_pct: Reading::Failed,
        temp_c: Reading::Failed,
        power_mw: Reading::Failed,
        clock_gfx_mhz: Reading::Failed,
        clock_mem_mhz: Reading::Failed,
        fan_pct: Reading::Failed,
        pcie_rx_kbps: Reading::Failed,
        pcie_tx_kbps: Reading::Failed,
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
        // E13-004 (ndy.6) fills the process rows.
        (t_ns, devices, processes.then(Vec::new))
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
    fn real_sample_under_50ms() {
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
            assert!(took < std::time::Duration::from_millis(50), "{took:?}");
        }
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
