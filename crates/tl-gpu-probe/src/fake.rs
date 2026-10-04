//! The `--fake <script>` backend (ADR-007 verification, E13-005).
//!
//! A test double for the engine's integration tests. It runs through the real `sampler::run`
//! loop, so the framing path is the real one. Each script makes the probe reply like a one-GPU
//! machine, hang, crash or break the protocol at a set point. Built only with the `fake` feature;
//! release builds and the packaged binary leave it out.

use std::io::Write;
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

use rustix::process::{Signal, getpid, getppid, kill_process};
use tl_model::gpu_proto::{GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, Reading};
use tl_model::wire::MAX_PROBE_FRAME;

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
            enc_pct: Reading::Ok(0),
            dec_pct: Reading::Ok(0),
            temp_c: Reading::Ok(45),
            power_mw: Reading::Ok(31_900),
            clock_gfx_mhz: Reading::Ok(1905),
            clock_mem_mhz: Reading::Ok(7001),
            fan_pct: Reading::Unsupported,
            pcie_rx_kbps: Reading::Ok(1000),
            pcie_tx_kbps: Reading::Ok(2000),
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
            }],
        })
    }

    fn hello_proto(&self) -> u16 {
        match self.script {
            Script::ProtoMismatch => 99,
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
}
