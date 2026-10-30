//! The fd 3 sampler loop (ADR-007).
//!
//! Reads `ProbeRequest` frames, answers each with one `ProbeResponse` frame and leaves the NVML
//! work to a `Backend`. Every protocol or I/O error ends the loop with exit 0 and no output: the
//! engine owns the restart.

use std::fs::File;
use std::process::ExitCode;

use rustix::process::{PidfdFlags, PidfdGetfdFlags, getpid, pidfd_getfd, pidfd_open};
use rustix::time::{ClockId, clock_gettime};
use tl_model::gpu_proto::{
    GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, ProbeRequest, ProbeResponse,
};
use tl_model::wire::{MAX_PROBE_FRAME, read_frame, write_frame};

pub const PROTO: u16 = tl_model::gpu_proto::GPU_PROTO_VERSION;

/// What `Backend::hello` reports when NVML starts.
pub struct HelloInfo {
    pub driver: String,
    pub nvml: String,
    pub devices: Vec<GpuDeviceInfo>,
}

/// Why `Backend::hello` could not start NVML.
pub struct InitFailed {
    pub kind: InitFailure,
    pub message: String,
}

/// The NVML side of the probe. The real backend and the fake backend implement it.
pub trait Backend {
    fn hello(&mut self) -> Result<HelloInfo, InitFailed>;
    /// The `proto` that a successful `Hello` answers with. Only the fake `proto-mismatch` script
    /// changes it.
    fn hello_proto(&self) -> u16 {
        PROTO
    }
    /// Returns `t_ns` (CLOCK_BOOTTIME), the device samples and, when `processes` is true, the
    /// process samples.
    fn sample(
        &mut self,
        seq: u32,
        processes: bool,
    ) -> (u64, Vec<GpuDeviceSample>, Option<Vec<GpuProcessSample>>);
}

/// CLOCK_BOOTTIME in nanoseconds, for `Backend::sample`.
pub fn now_boottime_ns() -> u64 {
    let ts = clock_gettime(ClockId::Boottime);
    // CLOCK_BOOTTIME is never negative.
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

/// Serves requests on `fd` until `Shutdown`, end of stream or any error, then returns exit 0.
///
/// `make` is called once, on the first `Hello` with the right protocol version.
pub fn run<B: Backend>(fd: i32, make: impl FnOnce() -> B) -> ExitCode {
    let Some(mut f) = open_fd(fd) else {
        return ExitCode::SUCCESS;
    };
    let mut make = Some(make);
    let mut backend: Option<B> = None;
    loop {
        let req: ProbeRequest = match read_frame(&mut f, MAX_PROBE_FRAME) {
            Ok(req) => req,
            // End of stream, an oversize prefix, a bad payload or an I/O error.
            Err(_) => return ExitCode::SUCCESS,
        };
        let resp = match req {
            ProbeRequest::Hello { proto } if proto != PROTO => {
                let resp = ProbeResponse::Hello {
                    proto: PROTO,
                    driver: String::new(),
                    nvml: String::new(),
                    devices: Vec::new(),
                };
                // The engine handles the version mismatch; the probe stops either way.
                let _ = write_frame(&mut f, &resp, MAX_PROBE_FRAME);
                return ExitCode::SUCCESS;
            }
            ProbeRequest::Hello { .. } => {
                let b = match (backend.as_mut(), make.take()) {
                    (Some(b), _) => b,
                    (None, Some(make)) => backend.insert(make()),
                    // A second hello after a failed one: `make` was already used.
                    (None, None) => return ExitCode::SUCCESS,
                };
                match b.hello() {
                    Ok(info) => ProbeResponse::Hello {
                        proto: b.hello_proto(),
                        driver: info.driver,
                        nvml: info.nvml,
                        devices: info.devices,
                    },
                    Err(e) => {
                        // A failed hello does not unlock `Sample`.
                        backend = None;
                        ProbeResponse::InitFailed {
                            kind: e.kind,
                            message: e.message,
                        }
                    }
                }
            }
            ProbeRequest::Sample { seq, processes } => match backend.as_mut() {
                Some(b) => {
                    let (t_ns, devices, processes) = b.sample(seq, processes);
                    ProbeResponse::Sample {
                        seq,
                        t_ns,
                        devices,
                        processes,
                    }
                }
                None => {
                    let resp = ProbeResponse::InitFailed {
                        kind: InitFailure::Other,
                        message: "sample before hello".to_owned(),
                    };
                    let _ = write_frame(&mut f, &resp, MAX_PROBE_FRAME);
                    return ExitCode::SUCCESS;
                }
            },
            ProbeRequest::Shutdown => return ExitCode::SUCCESS,
        };
        // EPIPE and every other write error: the engine is gone or will restart the probe.
        if write_frame(&mut f, &resp, MAX_PROBE_FRAME).is_err() {
            return ExitCode::SUCCESS;
        }
    }
}

/// Gets an owned copy of `fd` without `unsafe`.
///
/// `OwnedFd::from_raw_fd` and `BorrowedFd::borrow_raw` are `unsafe`, and opening
/// `/proc/self/fd/<n>` fails with ENXIO for a socket. `pidfd_getfd` on our own pidfd duplicates
/// the descriptor safely; a process may always do this to itself.
pub(crate) fn open_fd(fd: i32) -> Option<File> {
    let pidfd = pidfd_open(getpid(), PidfdFlags::empty()).ok()?;
    let owned = pidfd_getfd(&pidfd, fd, PidfdGetfdFlags::empty()).ok()?;
    Some(File::from(owned))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use tl_model::gpu_proto::Reading;

    use super::*;

    struct Double;

    fn device_info() -> GpuDeviceInfo {
        GpuDeviceInfo {
            uuid: [7; 16],
            name: "Test GPU".to_owned(),
            pci_bus_id: "00000000:01:00.0".to_owned(),
            mem_total: 8 << 30,
            power_limit_mw: Some(200_000),
            cuda_version: None,
            vbios: None,
            architecture: None,
            clock_gfx_max_mhz: None,
            clock_mem_max_mhz: None,
            power_max_limit_mw: None,
            power_min_limit_mw: None,
            slowdown_temp_c: None,
            shutdown_temp_c: None,
            pcie_max_gen: None,
            pcie_max_width: None,
        }
    }

    fn device_sample() -> GpuDeviceSample {
        GpuDeviceSample {
            uuid: [7; 16],
            util_pct: Reading::Ok(42),
            mem_util_pct: Reading::Ok(10),
            mem_used: Reading::Ok(1 << 30),
            mem_reserved: Reading::Unsupported,
            enc_pct: Reading::Unsupported,
            dec_pct: Reading::Unsupported,
            enc_sessions: Reading::Unsupported,
            jpeg_pct: Reading::Unsupported,
            ofa_pct: Reading::Unsupported,
            temp_c: Reading::Ok(55),
            power_mw: Reading::Failed,
            clock_gfx_mhz: Reading::Ok(1800),
            clock_mem_mhz: Reading::Ok(7000),
            clock_video_mhz: Reading::Unsupported,
            pstate: Reading::Unsupported,
            clock_event_reasons: Reading::Unsupported,
            fan_pct: Reading::Ok(30),
            fan_rpm: Reading::Unsupported,
            pcie_rx_kbps: Reading::Ok(100),
            pcie_tx_kbps: Reading::Ok(200),
            pcie_gen: Reading::Unsupported,
            pcie_width: Reading::Unsupported,
            energy_mj: Reading::Unsupported,
        }
    }

    impl Backend for Double {
        fn hello(&mut self) -> Result<HelloInfo, InitFailed> {
            Ok(HelloInfo {
                driver: "615.1".to_owned(),
                nvml: "13.615.1".to_owned(),
                devices: vec![device_info()],
            })
        }

        fn sample(
            &mut self,
            _seq: u32,
            processes: bool,
        ) -> (u64, Vec<GpuDeviceSample>, Option<Vec<GpuProcessSample>>) {
            (
                now_boottime_ns(),
                vec![device_sample()],
                processes.then(Vec::new),
            )
        }
    }

    /// Runs the probe loop on one end of a socketpair in a thread; returns the other end.
    fn spawn_probe(made: Arc<AtomicBool>) -> (UnixStream, JoinHandle<ExitCode>) {
        let (client, probe) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let code = run(probe.as_raw_fd(), || {
                made.store(true, Ordering::SeqCst);
                Double
            });
            // `run` closed its copy; close ours so the client sees end of stream.
            drop(probe);
            code
        });
        (client, handle)
    }

    fn send(c: &mut UnixStream, req: &ProbeRequest) {
        write_frame(c, req, MAX_PROBE_FRAME).unwrap();
    }

    fn recv(c: &mut UnixStream) -> ProbeResponse {
        read_frame(c, MAX_PROBE_FRAME).unwrap()
    }

    fn assert_closed(c: &mut UnixStream) {
        let mut buf = [0u8; 1];
        assert_eq!(c.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn hello_roundtrip_with_double() {
        let (mut c, h) = spawn_probe(Arc::default());
        send(&mut c, &ProbeRequest::Hello { proto: PROTO });
        assert_eq!(
            recv(&mut c),
            ProbeResponse::Hello {
                proto: PROTO,
                driver: "615.1".to_owned(),
                nvml: "13.615.1".to_owned(),
                devices: vec![device_info()],
            }
        );
        send(&mut c, &ProbeRequest::Shutdown);
        assert_eq!(h.join().unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn proto_mismatch_exits_without_backend() {
        let made = Arc::new(AtomicBool::new(false));
        let (mut c, h) = spawn_probe(made.clone());
        // An engine of the previous version: refused with this probe's version.
        send(&mut c, &ProbeRequest::Hello { proto: PROTO - 1 });
        assert_eq!(
            recv(&mut c),
            ProbeResponse::Hello {
                proto: PROTO,
                driver: String::new(),
                nvml: String::new(),
                devices: Vec::new(),
            }
        );
        assert_eq!(h.join().unwrap(), ExitCode::SUCCESS);
        assert_closed(&mut c);
        assert!(!made.load(Ordering::SeqCst));
    }

    #[test]
    fn sample_before_hello_is_init_failed() {
        let made = Arc::new(AtomicBool::new(false));
        let (mut c, h) = spawn_probe(made.clone());
        send(
            &mut c,
            &ProbeRequest::Sample {
                seq: 1,
                processes: true,
            },
        );
        assert_eq!(
            recv(&mut c),
            ProbeResponse::InitFailed {
                kind: InitFailure::Other,
                message: "sample before hello".to_owned(),
            }
        );
        assert_eq!(h.join().unwrap(), ExitCode::SUCCESS);
        assert_closed(&mut c);
        assert!(!made.load(Ordering::SeqCst));
    }

    #[test]
    fn sample_echoes_seq() {
        let (mut c, h) = spawn_probe(Arc::default());
        send(&mut c, &ProbeRequest::Hello { proto: PROTO });
        recv(&mut c);
        send(
            &mut c,
            &ProbeRequest::Sample {
                seq: 7,
                processes: false,
            },
        );
        match recv(&mut c) {
            ProbeResponse::Sample {
                seq,
                t_ns,
                devices,
                processes,
            } => {
                assert_eq!(seq, 7);
                assert!(t_ns > 0);
                assert_eq!(devices, vec![device_sample()]);
                assert_eq!(processes, None);
            }
            other => panic!("unexpected response {other:?}"),
        }
        send(
            &mut c,
            &ProbeRequest::Sample {
                seq: 8,
                processes: true,
            },
        );
        match recv(&mut c) {
            ProbeResponse::Sample { seq, processes, .. } => {
                assert_eq!(seq, 8);
                assert_eq!(processes, Some(Vec::new()));
            }
            other => panic!("unexpected response {other:?}"),
        }
        drop(c);
        assert_eq!(h.join().unwrap(), ExitCode::SUCCESS);
    }

    /// Waits for the probe thread with a 2 s limit, loose enough for a busy machine
    /// (tasklens-cr0.32).
    fn join_within_2s(h: JoinHandle<ExitCode>) -> ExitCode {
        let start = Instant::now();
        while !h.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "probe did not exit within 2 s"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        h.join().unwrap()
    }

    #[test]
    fn shutdown_and_eof_exit_zero() {
        let (mut c, h) = spawn_probe(Arc::default());
        send(&mut c, &ProbeRequest::Hello { proto: PROTO });
        recv(&mut c);
        send(&mut c, &ProbeRequest::Shutdown);
        assert_eq!(join_within_2s(h), ExitCode::SUCCESS);
        assert_closed(&mut c);

        let (mut c, h) = spawn_probe(Arc::default());
        send(&mut c, &ProbeRequest::Hello { proto: PROTO });
        recv(&mut c);
        drop(c);
        assert_eq!(join_within_2s(h), ExitCode::SUCCESS);
    }

    #[test]
    fn oversize_frame_exits() {
        let made = Arc::new(AtomicBool::new(false));
        let (mut c, h) = spawn_probe(made.clone());
        // A 2 MiB length prefix and no payload: the probe must exit, not wait for the bytes.
        c.write_all(&(2u32 << 20).to_le_bytes()).unwrap();
        assert_eq!(join_within_2s(h), ExitCode::SUCCESS);
        assert_closed(&mut c);
        assert!(!made.load(Ordering::SeqCst));
    }
}
