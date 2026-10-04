//! GPU probe messages (ADR-007).
//!
//! The engine and the `tl-gpu-probe` child talk with these types over `wire.rs` frames of at most
//! `MAX_PROBE_FRAME` bytes. NVML runs only in the probe, so a crash there cannot take down the app.
//! The engine treats every message as untrusted input. Deadlines (Hello 3000 ms, Sample 500 ms)
//! belong to the engine client, not here.

use serde::{Deserialize, Serialize};

use crate::validity::Validity;

/// Protocol version sent in both `Hello` messages.
pub const GPU_PROTO_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProbeRequest {
    Hello { proto: u16 },
    Sample { seq: u32, processes: bool },
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProbeResponse {
    Hello {
        proto: u16,
        driver: String,
        nvml: String,
        devices: Vec<GpuDeviceInfo>,
    },
    InitFailed {
        kind: InitFailure,
        message: String,
    },
    Sample {
        seq: u32,
        t_ns: u64,
        devices: Vec<GpuDeviceSample>,
        /// `None` when the request asked for no processes.
        processes: Option<Vec<GpuProcessSample>>,
    },
}

/// Why the probe could not start NVML. The engine maps each kind to the ADR-007 failure table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitFailure {
    LibraryNotFound,
    DriverNotLoaded,
    NoPermission,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuDeviceInfo {
    pub uuid: [u8; 16],
    pub name: String,
    pub pci_bus_id: String,
    pub mem_total: u64,
    pub power_limit_mw: Option<u32>,
    /// The last three are filled from NVML when the call succeeds, else `None` (R-156 item 1).
    pub cuda_version: Option<String>,
    pub vbios: Option<String>,
    pub architecture: Option<String>,
}

/// One NVML reading. Missing is never encoded as zero.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reading<T> {
    Ok(T),
    /// The device or driver does not provide this metric.
    Unsupported,
    /// The NVML call failed this sample.
    Failed,
}

impl<T> Reading<T> {
    pub fn ok(self) -> Option<T> {
        match self {
            Reading::Ok(v) => Some(v),
            Reading::Unsupported | Reading::Failed => None,
        }
    }

    pub fn validity(&self) -> Validity {
        match self {
            Reading::Ok(_) => Validity::Valid,
            Reading::Unsupported => Validity::Unsupported,
            Reading::Failed => Validity::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuDeviceSample {
    pub uuid: [u8; 16],
    pub util_pct: Reading<u32>,
    pub mem_util_pct: Reading<u32>,
    pub mem_used: Reading<u64>,
    pub enc_pct: Reading<u32>,
    pub dec_pct: Reading<u32>,
    pub temp_c: Reading<u32>,
    pub power_mw: Reading<u32>,
    pub clock_gfx_mhz: Reading<u32>,
    pub clock_mem_mhz: Reading<u32>,
    pub fan_pct: Reading<u32>,
    pub pcie_rx_kbps: Reading<u32>,
    pub pcie_tx_kbps: Reading<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuProcessSample {
    pub uuid: [u8; 16],
    pub pid: u32,
    pub sm_pct: Reading<u32>,
    pub mem_pct: Reading<u32>,
    pub enc_pct: Reading<u32>,
    pub dec_pct: Reading<u32>,
    pub mem_used: Reading<u64>,
}

/// Output of the one-shot `--api-versions` mode: one frame on stdout, then exit 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiVersions {
    pub devices: Vec<GpuApiVersions>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuApiVersions {
    /// Key, matched by VK_EXT_pci_bus_info or the EGL device.
    pub pci_bus_id: String,
    /// For example "1.4.312".
    pub vulkan_api: Option<String>,
    /// Vulkan driver properties `driverInfo`.
    pub vulkan_driver: Option<String>,
    /// The GL_VERSION string.
    pub opengl: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{MAX_PROBE_FRAME, encode_frame, read_frame};

    /// A device sample with every reading at its largest value, so the size test is worst case.
    fn device(i: u8) -> GpuDeviceSample {
        GpuDeviceSample {
            uuid: [i; 16],
            util_pct: Reading::Ok(u32::MAX),
            mem_util_pct: Reading::Ok(u32::MAX),
            mem_used: Reading::Ok(u64::MAX),
            enc_pct: Reading::Ok(u32::MAX),
            dec_pct: Reading::Ok(u32::MAX),
            temp_c: Reading::Ok(u32::MAX),
            power_mw: Reading::Ok(u32::MAX),
            clock_gfx_mhz: Reading::Ok(u32::MAX),
            clock_mem_mhz: Reading::Ok(u32::MAX),
            fan_pct: Reading::Unsupported,
            pcie_rx_kbps: Reading::Ok(u32::MAX),
            pcie_tx_kbps: Reading::Failed,
        }
    }

    fn process(i: u32) -> GpuProcessSample {
        GpuProcessSample {
            uuid: [(i % 16) as u8; 16],
            pid: u32::MAX - i,
            sm_pct: Reading::Ok(u32::MAX),
            mem_pct: Reading::Ok(u32::MAX),
            enc_pct: Reading::Ok(u32::MAX),
            dec_pct: Reading::Ok(u32::MAX),
            mem_used: Reading::Ok(u64::MAX),
        }
    }

    fn sample(devices: u8, processes: u32) -> ProbeResponse {
        ProbeResponse::Sample {
            seq: u32::MAX,
            t_ns: u64::MAX,
            devices: (0..devices).map(device).collect(),
            processes: Some((0..processes).map(process).collect()),
        }
    }

    #[test]
    fn reading_validity_and_ok() {
        assert_eq!(
            Reading::<u32>::Unsupported.validity(),
            Validity::Unsupported
        );
        assert_eq!(Reading::<u32>::Failed.validity(), Validity::Failed);
        assert_eq!(Reading::Ok(5u32).validity(), Validity::Valid);
        assert_eq!(Reading::Ok(5u32).ok(), Some(5));
        assert_eq!(Reading::<u32>::Unsupported.ok(), None);
        assert_eq!(Reading::<u32>::Failed.ok(), None);
    }

    #[test]
    fn proto_version_one() {
        assert_eq!(GPU_PROTO_VERSION, 1);
    }

    #[test]
    fn sample_round_trips_through_wire() {
        let msg = sample(2, 3);
        let mut buf = Vec::new();
        encode_frame(&msg, MAX_PROBE_FRAME, &mut buf).unwrap();
        let back: ProbeResponse = read_frame(&mut buf.as_slice(), MAX_PROBE_FRAME).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn large_sample_fits_probe_frame() {
        let msg = sample(16, 4096);
        let mut buf = Vec::new();
        encode_frame(&msg, MAX_PROBE_FRAME, &mut buf).unwrap();
        // 4 prefix bytes plus the payload.
        assert!(buf.len() - 4 < MAX_PROBE_FRAME);
    }
}
