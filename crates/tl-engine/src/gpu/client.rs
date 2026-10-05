//! Blocking client for the `tl-gpu-probe` protocol (ADR-007 "Request rules", "Failure and
//! restart", "Shutdown"; E13-009).
//!
//! Exactly one request is in flight: every call takes `&mut self`. A request is written as one
//! frame, then the client waits with `poll` until the socket is readable or the deadline passes,
//! then reads one frame. Any failure kills and reaps the child before the error is returned, so
//! the supervisor can start a new one and no zombie is left.
//!
//! The probe is forked on its own long-lived thread, `tl-gpu-spawn`, never on the blocking pool
//! (R-283). `PR_SET_PDEATHSIG` fires when the forking thread exits, and pool threads retire after
//! 10 s idle. The thread lives until the client is dropped, after the child is reaped.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::{Errno, dup};
use rustix::net::sockopt::{Timeout, set_socket_timeout};
use rustix::time::Timespec;
use tl_model::gpu_proto::{
    GPU_PROTO_VERSION, GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, ProbeRequest,
    ProbeResponse,
};
use tl_model::wire::{MAX_PROBE_FRAME, WireError, read_frame, write_frame};

use super::spawn::{ProbeChild, spawn_probe};
use super::{HELLO_DEADLINE, ProbeError, SAMPLE_DEADLINE, SHUTDOWN_GRACE};

/// Name of the thread that forks the probe and stays alive while it runs (R-283).
pub const SPAWN_THREAD_NAME: &str = "tl-gpu-spawn";

/// How often `shutdown` checks whether the child has exited.
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

/// The smallest receive timeout set for the payload read. Zero would mean "no timeout".
const MIN_READ_TIMEOUT: Duration = Duration::from_millis(1);

pub struct ProbeClient {
    /// Dropped first: kills and reaps the child if it still runs.
    child: ProbeChild,
    /// A dup of `child.sock` for `Read` and `Write`.
    file: File,
    /// Set by a `Ready` hello.
    pub info: Option<HelloInfo>,
    /// Set by an `InitFailed` hello, so the caller can apply the ADR-007 failure table.
    pub init_failed: Option<(InitFailure, Arc<str>)>,
    seq: u32,
    /// Dropped last, after the child is gone.
    _spawner: SpawnThread,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelloInfo {
    pub proto: u16,
    pub driver: Arc<str>,
    pub nvml: Arc<str>,
    pub devices: Arc<[GpuDeviceInfo]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HelloOutcome {
    Ready(HelloInfo),
    InitFailed {
        kind: InitFailure,
        message: Arc<str>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResponseSample {
    pub seq: u32,
    pub t_ns: u64,
    pub devices: Vec<GpuDeviceSample>,
    pub processes: Option<Vec<GpuProcessSample>>,
}

impl ProbeClient {
    /// Starts the probe and runs `Hello` under [`HELLO_DEADLINE`].
    ///
    /// An `InitFailed` reply still returns the client, with `info` `None` and `init_failed`
    /// set, so the caller decides between a terminal state and a retry.
    pub fn start(probe: &Path) -> Result<ProbeClient, ProbeError> {
        Self::start_with_args(probe, &[])
    }

    /// [`start`](Self::start) with extra probe arguments after `--fd 3`, for example
    /// `--fake <script>` in tests.
    pub fn start_with_args(probe: &Path, extra_args: &[&OsStr]) -> Result<ProbeClient, ProbeError> {
        let (child, spawner) = SpawnThread::spawn(probe, extra_args)?;
        let file = dup(&child.sock)
            .map(File::from)
            .map_err(|e| ProbeError::Spawn(format!("dup: {e}").into()))?;
        let mut client = ProbeClient {
            child,
            file,
            info: None,
            init_failed: None,
            seq: 0,
            _spawner: spawner,
        };
        client.hello()?;
        Ok(client)
    }

    /// Sends `Hello { proto: 1 }` and reads the reply.
    pub fn hello(&mut self) -> Result<HelloOutcome, ProbeError> {
        let req = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        match self.request(&req, HELLO_DEADLINE)? {
            ProbeResponse::Hello { proto, .. } if proto != GPU_PROTO_VERSION => {
                self.kill_and_reap();
                Err(ProbeError::VersionMismatch(proto))
            }
            ProbeResponse::Hello {
                proto,
                driver,
                nvml,
                devices,
            } => {
                let info = HelloInfo {
                    proto,
                    driver: driver.into(),
                    nvml: nvml.into(),
                    devices: devices.into(),
                };
                self.info = Some(info.clone());
                self.init_failed = None;
                Ok(HelloOutcome::Ready(info))
            }
            ProbeResponse::InitFailed { kind, message } => {
                let message: Arc<str> = message.into();
                self.info = None;
                self.init_failed = Some((kind, message.clone()));
                Ok(HelloOutcome::InitFailed { kind, message })
            }
            ProbeResponse::Sample { .. } => Err(self.protocol("sample reply to hello")),
        }
    }

    /// Sends `Sample { seq, processes }` with the next `seq` and reads the reply under
    /// [`SAMPLE_DEADLINE`]. A reply with another `seq` is a protocol error.
    pub fn sample(&mut self, processes: bool) -> Result<ProbeResponseSample, ProbeError> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        match self.request(&ProbeRequest::Sample { seq, processes }, SAMPLE_DEADLINE)? {
            ProbeResponse::Sample {
                seq: got,
                t_ns,
                devices,
                processes,
            } if got == seq => Ok(ProbeResponseSample {
                seq,
                t_ns,
                devices,
                processes,
            }),
            ProbeResponse::Sample { seq: got, .. } => {
                Err(self.protocol(&format!("sample seq {got}, expected {seq}")))
            }
            ProbeResponse::Hello { .. } => Err(self.protocol("hello reply to sample")),
            ProbeResponse::InitFailed { .. } => Err(self.protocol("init failed reply to sample")),
        }
    }

    /// Sends `Shutdown` (errors ignored), waits up to [`SHUTDOWN_GRACE`] for the child to
    /// exit, then sends `SIGKILL` and reaps it.
    pub fn shutdown(mut self) {
        let _ = write_frame(&mut self.file, &ProbeRequest::Shutdown, MAX_PROBE_FRAME);
        let start = Instant::now();
        loop {
            match self.child.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if start.elapsed() >= SHUTDOWN_GRACE => break,
                Ok(None) => thread::sleep(SHUTDOWN_POLL),
            }
        }
        let status = self.kill_and_reap();
        tracing::debug!(%status, "GPU probe shut down");
    }

    fn request(
        &mut self,
        req: &ProbeRequest,
        deadline: Duration,
    ) -> Result<ProbeResponse, ProbeError> {
        let start = Instant::now();
        if let Err(e) = write_frame(&mut self.file, req, MAX_PROBE_FRAME) {
            // EPIPE or ECONNRESET: the child closed its end, most likely because it exited.
            if let Ok(Some(status)) = self.child.child.try_wait() {
                return Err(ProbeError::Exited(status_text(status)));
            }
            return Err(self.protocol(&e.to_string()));
        }

        let revents = loop {
            let Some(left) = remaining(deadline, start.elapsed()) else {
                self.kill_and_reap();
                return Err(ProbeError::Deadline(deadline));
            };
            let mut fds = [PollFd::new(&self.child.sock, PollFlags::IN)];
            match poll(&mut fds, Some(&timespec(left))) {
                // Timed out: the next turn sees the deadline as passed.
                Ok(0) => {}
                Ok(_) => break fds[0].revents(),
                Err(Errno::INTR) => {}
                Err(e) => return Err(self.protocol(&format!("poll: {e}"))),
            }
        };
        if !revents.contains(PollFlags::IN) {
            // HUP or ERR with nothing to read: the child is gone.
            return Err(ProbeError::Exited(self.kill_and_reap()));
        }

        // The prefix is there, but the payload may still be on its way. Bound that read by the
        // rest of the deadline so a child that stops mid-frame cannot hold this thread.
        let left = remaining(deadline, start.elapsed()).unwrap_or(MIN_READ_TIMEOUT);
        if let Err(e) = set_socket_timeout(&self.file, Timeout::Recv, Some(left)) {
            return Err(self.protocol(&format!("set read timeout: {e}")));
        }
        match read_frame::<_, ProbeResponse>(&mut self.file, MAX_PROBE_FRAME) {
            Ok(resp) => Ok(resp),
            Err(WireError::Eof | WireError::Truncated) => {
                Err(ProbeError::Exited(self.kill_and_reap()))
            }
            Err(WireError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                self.kill_and_reap();
                Err(ProbeError::Deadline(deadline))
            }
            Err(e) => Err(self.protocol(&e.to_string())),
        }
    }

    /// Kills and reaps the child, then returns [`ProbeError::Protocol`] with `text`.
    fn protocol(&mut self, text: &str) -> ProbeError {
        let status = self.kill_and_reap();
        tracing::debug!(%status, text, "GPU probe protocol error");
        ProbeError::Protocol(text.into())
    }

    /// Sends `SIGKILL` and waits for the child. Returns "status N" or "signal N". A child that
    /// already exited is only reaped, so its own status is reported.
    fn kill_and_reap(&mut self) -> Arc<str> {
        let child = &mut self.child.child;
        let status = match child.try_wait() {
            Ok(Some(status)) => Ok(status),
            _ => {
                let _ = child.kill();
                child.wait()
            }
        };
        match status {
            Ok(status) => status_text(status),
            Err(e) => format!("wait failed: {e}").into(),
        }
    }
}

/// "status N" for a normal exit, "signal N" for a signal.
fn status_text(status: ExitStatus) -> Arc<str> {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("status {code}").into(),
        (None, Some(sig)) => format!("signal {sig}").into(),
        (None, None) => "unknown status".into(),
    }
}

/// Time left of `deadline` after `elapsed`, or `None` when it has passed.
fn remaining(deadline: Duration, elapsed: Duration) -> Option<Duration> {
    deadline.checked_sub(elapsed).filter(|left| !left.is_zero())
}

fn timespec(d: Duration) -> Timespec {
    Timespec {
        tv_sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: d.subsec_nanos().into(),
    }
}

/// The thread that forked the probe. It blocks until this handle is dropped, so the child's
/// parent-death signal fires only when the engine itself dies (R-283).
struct SpawnThread {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl SpawnThread {
    fn spawn(probe: &Path, extra_args: &[&OsStr]) -> Result<(ProbeChild, SpawnThread), ProbeError> {
        let probe: PathBuf = probe.to_path_buf();
        let args: Vec<OsString> = extra_args.iter().map(|a| a.to_os_string()).collect();
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = thread::Builder::new()
            .name(SPAWN_THREAD_NAME.to_owned())
            .spawn(move || {
                let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
                let spawned = spawn_probe(&probe, &args);
                let running = spawned.is_ok();
                // If the receiver is gone, the returned child is dropped here, which kills it.
                if result_tx.send(spawned).is_ok() && running {
                    // Returns when the client drops the sender.
                    let _ = stop_rx.recv();
                }
            })
            .map_err(|e| ProbeError::Spawn(format!("spawn thread: {e}").into()))?;
        let spawner = SpawnThread {
            stop: Some(stop_tx),
            handle: Some(handle),
        };
        let child = result_rx
            .recv()
            .map_err(|_| ProbeError::Spawn("spawn thread ended".into()))??;
        Ok((child, spawner))
    }
}

impl Drop for SpawnThread {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_deadline_math() {
        let d = Duration::from_millis(500);
        assert_eq!(
            remaining(d, Duration::ZERO),
            Some(Duration::from_millis(500))
        );
        // A partial wait, for example a poll cut short by EINTR after 120 ms.
        assert_eq!(
            remaining(d, Duration::from_millis(120)),
            Some(Duration::from_millis(380))
        );
        // Two more EINTR retries: the time left keeps shrinking from the same start.
        assert_eq!(
            remaining(d, Duration::from_millis(499)),
            Some(Duration::from_millis(1))
        );
        assert_eq!(remaining(d, Duration::from_millis(500)), None);
        assert_eq!(remaining(d, Duration::from_millis(900)), None);

        let ts = timespec(Duration::from_millis(3001));
        assert_eq!((ts.tv_sec, ts.tv_nsec), (3, 1_000_000));
    }

    #[test]
    fn status_text_forms() {
        assert_eq!(&*status_text(ExitStatus::from_raw(0)), "status 0");
        assert_eq!(&*status_text(ExitStatus::from_raw(3 << 8)), "status 3");
        assert_eq!(&*status_text(ExitStatus::from_raw(11)), "signal 11");
        assert_eq!(&*status_text(ExitStatus::from_raw(9)), "signal 9");
    }
}
