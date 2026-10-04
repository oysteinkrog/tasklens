//! The NVML backend (ADR-007, PERF-062, PERF-071).
//!
//! `hello` starts NVML and lists the devices. `sample` reads one set of metrics per device
//! (E13-003).

use nvml_wrapper::error::NvmlError;
use nvml_wrapper::{Nvml, cuda_driver_version_major, cuda_driver_version_minor};
use tl_model::gpu_proto::{GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure};

use crate::sampler::{Backend, HelloInfo, InitFailed, now_boottime_ns};

/// One device that `hello` listed. NVML device handles borrow `Nvml`, so the slot keeps the index.
// E13-003 (ndy.5) reads these fields in `sample`.
#[allow(dead_code)]
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
    })
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
        // E13-003 (ndy.5) fills the device and process samples.
        (now_boottime_ns(), Vec::new(), processes.then(Vec::new))
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
