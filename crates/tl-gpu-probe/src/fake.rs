//! The `--fake <script>` backend (ADR-007 verification, E13-005).
//!
//! A test double for the engine's integration tests. It runs through the real `sampler::run`
//! loop, so the framing path is the real one. Each script makes the probe reply like a one-GPU
//! machine, hang, crash or break the protocol at a set point. Built only with the `fake` feature;
//! release builds and the packaged binary leave it out.
//!
//! `--api-versions` in a fake build answers from [`API_VERSIONS_ENV`] when it is set: `1` writes
//! one fixed device, `sleep` hangs for 60 s, `crash` ends with `SIGSEGV` (E13-010).

use std::io::Write;
use std::process::{ExitCode, exit};
use std::thread::sleep;
use std::time::Duration;

use rustix::process::{
    DumpableBehavior, Signal, getpid, getppid, kill_process, set_dumpable_behavior,
};
use tl_model::gpu_proto::{
    ApiVersions, GpuApiVersions, GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure,
    Reading,
};
use tl_model::wire::{MAX_PROBE_FRAME, write_frame};

use crate::sampler::{Backend, HelloInfo, InitFailed, PROTO, now_boottime_ns, open_fd};

pub const SCRIPTS: &[&str] = &[
    "normal",
    "sleep-on-sample",
    "sleep-on-hello",
    "segv-on-sample",
    "segv-on-hello",
    "oversize-on-sample",
    "init-library-not-found",
    "init-driver-not-loaded",
    "init-no-permission",
    "exit-after-3",
    "proto-mismatch",
];

/// Selects the fake `--api-versions` reply: `1`, `sleep` or `crash`.
pub const API_VERSIONS_ENV: &str = "TL_FAKE_API_VERSIONS";

/// How long the `sleep` reply hangs. The engine's deadline is 5 s.
const API_VERSIONS_SLEEP: Duration = Duration::from_secs(60);

const UUID: [u8; 16] = [0x42; 16];
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Script {
    Normal,
    SleepOnSample,
    SleepOnHello,
    SegvOnSample,
    SegvOnHello,
    OversizeOnSample,
    Init(InitFailure),
    ExitAfter3,
    ProtoMismatch,
}

impl Script {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "normal" => Script::Normal,
            "sleep-on-sample" => Script::SleepOnSample,
            "sleep-on-hello" => Script::SleepOnHello,
            "segv-on-sample" => Script::SegvOnSample,
            "segv-on-hello" => Script::SegvOnHello,
            "oversize-on-sample" => Script::OversizeOnSample,
            "init-library-not-found" => Script::Init(InitFailure::LibraryNotFound),
            "init-driver-not-loaded" => Script::Init(InitFailure::DriverNotLoaded),
            "init-no-permission" => Script::Init(InitFailure::NoPermission),
            "exit-after-3" => Script::ExitAfter3,
            "proto-mismatch" => Script::ProtoMismatch,
            _ => return None,
        })
    }
}

pub struct FakeBackend {
    script: Script,
    /// Sample requests seen so far.
    samples: u32,
    /// The protocol fd, for `oversize-on-sample`, which writes a raw prefix past the framing.
    fd: i32,
}

impl FakeBackend {
    /// Builds the backend for `s`. An unknown script prints the list to stderr and exits 2.
    pub fn from_script(s: &str) -> Self {
        match Self::try_from_script(s) {
            Some(b) => b,
            None => {
                eprintln!("unknown fake script {s:?}; scripts: {}", SCRIPTS.join(", "));
                exit(2);
            }
        }
    }

    fn try_from_script(s: &str) -> Option<Self> {
        Some(Self {
            script: Script::parse(s)?,
            samples: 0,
            fd: -1,
        })
    }

    /// Sets the protocol fd that `oversize-on-sample` writes its broken frame to.
    pub fn with_fd(mut self, fd: i32) -> Self {
        self.fd = fd;
        self
    }

    fn device_sample(seq: u32) -> GpuDeviceSample {
        GpuDeviceSample {
            uuid: UUID,
            util_pct: Reading::Ok(seq % 100),
            mem_util_pct: Reading::Ok(seq % 50),
            mem_used: Reading::Ok(2 * GIB + u64::from(seq) * MIB),
            mem_reserved: Reading::Ok(380 * MIB),
            enc_pct: Reading::Ok(0),
            dec_pct: Reading::Ok(0),
            enc_sessions: Reading::Ok(0),
            jpeg_pct: Reading::Unsupported,
            ofa_pct: Reading::Unsupported,
            temp_c: Reading::Ok(45),
            power_mw: Reading::Ok(31_900),
            clock_gfx_mhz: Reading::Ok(1905),
            clock_mem_mhz: Reading::Ok(7001),
            clock_video_mhz: Reading::Ok(1_620),
            pstate: Reading::Ok(3),
            // SwPowerCap.
            clock_event_reasons: Reading::Ok(0x4),
            fan_pct: Reading::Unsupported,
            fan_rpm: Reading::Unsupported,
            pcie_rx_kbps: Reading::Ok(1000),
            pcie_tx_kbps: Reading::Ok(2000),
            pcie_gen: Reading::Ok(4),
            pcie_width: Reading::Ok(16),
            energy_mj: Reading::Ok(4_464_000_000),
        }
    }

    /// One row for the probe's parent, so the engine test maps a live pid.
    fn process_sample() -> GpuProcessSample {
        let pid = getppid().map_or(0, |p| p.as_raw_nonzero().get() as u32);
        GpuProcessSample {
            uuid: UUID,
            pid,
            sm_pct: Reading::Ok(12),
            mem_pct: Reading::Unsupported,
            enc_pct: Reading::Unsupported,
            dec_pct: Reading::Unsupported,
            mem_used: Reading::Ok(256 * MIB),
        }
    }

    /// Writes a length prefix of `MAX_PROBE_FRAME + 1` and 16 zero bytes, then exits 0.
    fn write_oversize_and_exit(&self) -> ! {
        if let Some(mut f) = open_fd(self.fd) {
            let len = (MAX_PROBE_FRAME + 1) as u32;
            let _ = f.write_all(&len.to_le_bytes());
            let _ = f.write_all(&[0u8; 16]);
        }
        exit(0);
    }
}

fn segv() -> ! {
    // Not dumpable, so the kernel skips the core dump and does not start systemd-coredump.
    // RLIMIT_CORE 0 is not enough: the kernel still starts a piped core handler. While the
    // handler reads the dump the probe's socket stays open, and the engine sees a deadline
    // instead of the crash (8fx.37).
    let _ = set_dumpable_behavior(DumpableBehavior::NotDumpable);
    // The std runtime's stack overflow handler catches the first SIGSEGV, sees no guard page
    // fault, restores the default action and returns. The second one ends the process.
    let _ = kill_process(getpid(), Signal::SEGV);
    let _ = kill_process(getpid(), Signal::SEGV);
    // Only reached if the signal was blocked.
    exit(139);
}

impl Backend for FakeBackend {
    fn hello(&mut self) -> Result<HelloInfo, InitFailed> {
        match self.script {
            Script::SleepOnHello => sleep(Duration::from_secs(5)),
            Script::SegvOnHello => segv(),
            Script::Init(kind) => {
                return Err(InitFailed {
                    kind,
                    message: format!("fake {kind:?}"),
                });
            }
            _ => {}
        }
        Ok(HelloInfo {
            driver: "615.1".to_owned(),
            nvml: "13.615.1".to_owned(),
            devices: vec![GpuDeviceInfo {
                uuid: UUID,
                name: "Fake GPU 5070".to_owned(),
                pci_bus_id: "00000000:2D:00.0".to_owned(),
                mem_total: 16 * GIB,
                power_limit_mw: Some(300_000),
                cuda_version: None,
                vbios: None,
                architecture: None,
                clock_gfx_max_mhz: Some(3_105),
                clock_mem_max_mhz: Some(14_001),
                power_max_limit_mw: Some(350_000),
                power_min_limit_mw: Some(100_000),
                slowdown_temp_c: Some(88),
                shutdown_temp_c: Some(95),
                pcie_max_gen: Some(5),
                pcie_max_width: Some(16),
            }],
        })
    }

    fn hello_proto(&self) -> u16 {
        match self.script {
            // A probe of the previous version, as left installed after an upgrade.
            Script::ProtoMismatch => PROTO - 1,
            _ => PROTO,
        }
    }

    fn sample(
        &mut self,
        seq: u32,
        processes: bool,
    ) -> (u64, Vec<GpuDeviceSample>, Option<Vec<GpuProcessSample>>) {
        self.samples += 1;
        let first = self.samples == 1;
        match self.script {
            Script::SleepOnSample if first => sleep(Duration::from_secs(2)),
            Script::SegvOnSample if first => segv(),
            Script::OversizeOnSample if first => self.write_oversize_and_exit(),
            Script::ExitAfter3 if self.samples > 3 => exit(0),
            _ => {}
        }
        (
            now_boottime_ns(),
            vec![Self::device_sample(seq)],
            processes.then(|| vec![Self::process_sample()]),
        )
    }
}

/// The fake `--api-versions` reply, or `None` when [`API_VERSIONS_ENV`] is not set and the real
/// mode should run. An unknown value prints the accepted ones and exits 2.
pub fn api_versions_from_env() -> Option<ExitCode> {
    let value = std::env::var_os(API_VERSIONS_ENV)?;
    Some(match value.to_str() {
        Some("1") => {
            let mut out = std::io::stdout().lock();
            let written = write_frame(&mut out, &fake_api_versions(), MAX_PROBE_FRAME)
                .map_err(|e| e.to_string())
                .and_then(|()| out.flush().map_err(|e| e.to_string()));
            match written {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("tl-gpu-probe: writing the fake api-versions frame failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Some("sleep") => {
            sleep(API_VERSIONS_SLEEP);
            ExitCode::SUCCESS
        }
        Some("crash") => segv(),
        _ => {
            eprintln!("unknown {API_VERSIONS_ENV} value {value:?}; values: 1, sleep, crash");
            ExitCode::from(2)
        }
    })
}

/// The one device the fake `--api-versions` reports, on the fake GPU's PCI bus ID.
fn fake_api_versions() -> ApiVersions {
    ApiVersions {
        devices: vec![GpuApiVersions {
            pci_bus_id: "00000000:2D:00.0".to_owned(),
            vulkan_api: Some("1.4.312".to_owned()),
            vulkan_driver: Some("NVIDIA 615.71.09".to_owned()),
            opengl: Some("4.6.0 NVIDIA 615.71.09".to_owned()),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_script_builds() {
        for s in SCRIPTS {
            let b = FakeBackend::try_from_script(s)
                .unwrap_or_else(|| panic!("script {s} does not build"));
            assert_eq!(b.samples, 0);
        }
        assert!(FakeBackend::try_from_script("bogus").is_none());
    }

    #[test]
    fn fake_api_versions_round_trips() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &fake_api_versions(), MAX_PROBE_FRAME).expect("write");
        let got: ApiVersions =
            tl_model::wire::read_frame(&mut &buf[..], MAX_PROBE_FRAME).expect("read");
        assert_eq!(got, fake_api_versions());
        assert_eq!(got.devices[0].pci_bus_id, "00000000:2D:00.0");
    }
}
