//! GPU probe messages (ADR-007).
//!
//! The engine and the `tl-gpu-probe` child talk with these types over `wire.rs` frames of at most
//! `MAX_PROBE_FRAME` bytes. NVML runs only in the probe, so a crash there cannot take down the app.
//! The engine treats every message as untrusted input. Deadlines (Hello 3000 ms, Sample 500 ms)
//! belong to the engine client, not here.

use serde::{Deserialize, Serialize};

use crate::validity::Validity;

/// Protocol version sent in both `Hello` messages. 2 added `mem_reserved` and `enc_sessions`
/// to `GpuDeviceSample`. 3 added the PERF-066 to PERF-070 limits to `GpuDeviceInfo` and fan RPM,
/// video clock, P-state, PCIe link and energy to `GpuDeviceSample`. 4 added the JPEG and OFA
/// engine readings to `GpuDeviceSample`. 5 added the minimum power limit to `GpuDeviceInfo` and
/// the clock event reasons to `GpuDeviceSample`. A probe of another version
/// answers `Hello` with an empty device list, so its reply decodes whatever the device layout,
/// and it is refused there before any sample is decoded.
pub const GPU_PROTO_VERSION: u16 = 5;

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
    /// `nvmlDeviceGetMaxClockInfo` graphics and memory, MHz (PERF-066).
    pub clock_gfx_max_mhz: Option<u32>,
    pub clock_mem_max_mhz: Option<u32>,
    /// `nvmlDeviceGetPowerManagementLimitConstraints` `max_limit` and `min_limit`, milliwatts
    /// (PERF-067).
    pub power_max_limit_mw: Option<u32>,
    pub power_min_limit_mw: Option<u32>,
    /// `nvmlDeviceGetTemperatureThreshold` slowdown and shutdown, °C (PERF-068).
    pub slowdown_temp_c: Option<u32>,
    pub shutdown_temp_c: Option<u32>,
    /// `nvmlDeviceGetMaxPcieLinkGeneration` and `nvmlDeviceGetMaxPcieLinkWidth` (PERF-070). The
    /// generation is the lower of the card's and the host's maximum.
    pub pcie_max_gen: Option<u32>,
    pub pcie_max_width: Option<u32>,
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
    /// `nvmlDeviceGetMemoryInfo_v2` `reserved`, bytes (PERF-064).
    pub mem_reserved: Reading<u64>,
    pub enc_pct: Reading<u32>,
    pub dec_pct: Reading<u32>,
    /// Active NVENC sessions, `nvmlDeviceGetEncoderSessions` (PERF-065).
    pub enc_sessions: Reading<u32>,
    /// `nvmlDeviceGetJpgUtilization` and `nvmlDeviceGetOfaUtilization`, percent (PERF-063).
    /// `Unsupported` on a card or driver without the engine.
    pub jpeg_pct: Reading<u32>,
    pub ofa_pct: Reading<u32>,
    pub temp_c: Reading<u32>,
    pub power_mw: Reading<u32>,
    pub clock_gfx_mhz: Reading<u32>,
    pub clock_mem_mhz: Reading<u32>,
    /// `nvmlDeviceGetClockInfo(NVML_CLOCK_VIDEO)`, MHz (PERF-066).
    pub clock_video_mhz: Reading<u32>,
    /// `nvmlDeviceGetPerformanceState`, 0 to 15 for P0 to P15 (PERF-066).
    pub pstate: Reading<u8>,
    /// `nvmlDeviceGetCurrentClocksEventReasons`, the NVML `nvmlClocksEventReason*` bit mask as
    /// NVML returns it (PERF-066). 0 means the clocks are not held down.
    pub clock_event_reasons: Reading<u64>,
    pub fan_pct: Reading<u32>,
    /// `nvmlDeviceGetFanSpeedRPM` of fan 0 (PERF-068).
    pub fan_rpm: Reading<u32>,
    pub pcie_rx_kbps: Reading<u32>,
    pub pcie_tx_kbps: Reading<u32>,
    /// `nvmlDeviceGetCurrPcieLinkGeneration` and `nvmlDeviceGetCurrPcieLinkWidth` (PERF-070).
    pub pcie_gen: Reading<u32>,
    pub pcie_width: Reading<u32>,
    /// `nvmlDeviceGetTotalEnergyConsumption`, millijoules since the driver loaded (PERF-067).
    pub energy_mj: Reading<u64>,
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
            mem_reserved: Reading::Ok(u64::MAX),
            enc_pct: Reading::Ok(u32::MAX),
            dec_pct: Reading::Ok(u32::MAX),
            enc_sessions: Reading::Ok(u32::MAX),
            jpeg_pct: Reading::Ok(u32::MAX),
            ofa_pct: Reading::Ok(u32::MAX),
            temp_c: Reading::Ok(u32::MAX),
            power_mw: Reading::Ok(u32::MAX),
            clock_gfx_mhz: Reading::Ok(u32::MAX),
            clock_mem_mhz: Reading::Ok(u32::MAX),
            clock_video_mhz: Reading::Ok(u32::MAX),
            pstate: Reading::Ok(u8::MAX),
            clock_event_reasons: Reading::Ok(u64::MAX),
            fan_pct: Reading::Unsupported,
            fan_rpm: Reading::Ok(u32::MAX),
            pcie_rx_kbps: Reading::Ok(u32::MAX),
            pcie_tx_kbps: Reading::Failed,
            pcie_gen: Reading::Ok(u32::MAX),
            pcie_width: Reading::Ok(u32::MAX),
            energy_mj: Reading::Ok(u64::MAX),
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
    fn proto_version_five() {
        assert_eq!(GPU_PROTO_VERSION, 5);
    }

    /// A `Hello` and a sample with and without the JPEG and OFA readings.
    #[test]
    fn round_trips_jpeg_and_ofa() {
        let hello = ProbeResponse::Hello {
            proto: GPU_PROTO_VERSION,
            driver: "580.82.09".to_string(),
            nvml: "12.580.82.09".to_string(),
            devices: Vec::new(),
        };
        let with = GpuDeviceSample {
            jpeg_pct: Reading::Ok(17),
            ofa_pct: Reading::Ok(3),
            ..device(1)
        };
        let without = GpuDeviceSample {
            jpeg_pct: Reading::Unsupported,
            ofa_pct: Reading::Failed,
            ..device(2)
        };
        let sample = ProbeResponse::Sample {
            seq: 1,
            t_ns: 2,
            devices: vec![with, without],
            processes: None,
        };
        for msg in [hello, sample] {
            let mut buf = Vec::new();
            encode_frame(&msg, MAX_PROBE_FRAME, &mut buf).unwrap();
            let back: ProbeResponse = read_frame(&mut buf.as_slice(), MAX_PROBE_FRAME).unwrap();
            assert_eq!(back, msg);
        }
    }

    /// A version 5 `Hello` with and without the minimum power limit, and a sample with and
    /// without the clock event reasons.
    #[test]
    fn version_five_round_trips_min_limit_and_reasons() {
        let info = |power_min_limit_mw| GpuDeviceInfo {
            uuid: [7; 16],
            name: "NVIDIA GeForce RTX 4080".to_string(),
            pci_bus_id: "00000000:01:00.0".to_string(),
            mem_total: 16 << 30,
            power_limit_mw: Some(320_000),
            cuda_version: None,
            vbios: None,
            architecture: None,
            clock_gfx_max_mhz: None,
            clock_mem_max_mhz: None,
            power_max_limit_mw: Some(350_000),
            power_min_limit_mw,
            slowdown_temp_c: None,
            shutdown_temp_c: None,
            pcie_max_gen: None,
            pcie_max_width: None,
        };
        let hello = ProbeResponse::Hello {
            proto: GPU_PROTO_VERSION,
            driver: "580.82.09".to_string(),
            nvml: "12.580.82.09".to_string(),
            devices: vec![info(Some(100_000)), info(None)],
        };
        let with = GpuDeviceSample {
            // SwPowerCap and HwSlowdown.
            clock_event_reasons: Reading::Ok(0x4 | 0x8),
            ..device(1)
        };
        let none = GpuDeviceSample {
            clock_event_reasons: Reading::Ok(0),
            ..device(2)
        };
        let without = GpuDeviceSample {
            clock_event_reasons: Reading::Unsupported,
            ..device(3)
        };
        let sample = ProbeResponse::Sample {
            seq: 1,
            t_ns: 2,
            devices: vec![with, none, without],
            processes: None,
        };
        for msg in [hello, sample] {
            let mut buf = Vec::new();
            encode_frame(&msg, MAX_PROBE_FRAME, &mut buf).unwrap();
            let back: ProbeResponse = read_frame(&mut buf.as_slice(), MAX_PROBE_FRAME).unwrap();
            assert_eq!(back, msg);
        }
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
