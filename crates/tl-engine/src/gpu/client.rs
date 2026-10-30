//! Blocking client for the `tl-gpu-probe` protocol (ADR-007 "Request rules", "Failure and
//! restart", "Shutdown"; E13-009).
//!
//! Exactly one request is in flight: every call takes `&mut self`. A request is written as one
//! frame, then the client reads one frame. Before every read of it the client waits with `poll`
//! until the socket is readable, the deadline passes or the call is cancelled. On any failure
//! the client gives the child up before the error is returned: the child moves into a reaper
//! that kills it and reaps it on a background thread, so the supervisor can start a new one, no
//! zombie is left, and no caller ever waits for a killed child to exit (tasklens-cr0.30).
//!
//! The probe is forked on its own long-lived thread, `tl-gpu-spawn`, never on the blocking pool
//! (R-283). `PR_SET_PDEATHSIG` fires when the forking thread exits, and pool threads retire after
//! 10 s idle. The thread lives until the client is dropped, after the child is given up.
//!
//! [`api_versions`] runs the one-shot `--api-versions` mode in its own process (E13-010). It
//! needs no fd 3 and no `pre_exec`, so it is forked from the calling thread.
//!
//! The `_or_cancel` forms take a cancel check, which every wait calls at least once per
//! [`CANCEL_POLL`]. On cancel the child is given up at once and the call returns
//! [`ProbeError::Cancelled`], so engine shutdown never waits for a slow NVML call or for the
//! driver to tear down a killed probe (tasklens-cr0.30).

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::Errno;
use rustix::time::Timespec;
use tl_model::gpu_proto::{
    ApiVersions, GPU_PROTO_VERSION, GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure,
    ProbeRequest, ProbeResponse,
};
use tl_model::wire::{MAX_PROBE_FRAME, WireError, read_frame, write_frame};

use super::spawn::{ProbeChild, child_env, kill_and_reap_in_background, spawn_probe};
use super::{CANCEL_POLL, HELLO_DEADLINE, ProbeError, SAMPLE_DEADLINE, SHUTDOWN_GRACE};

/// Name of the thread that forks the probe and stays alive while it runs (R-283).
pub const SPAWN_THREAD_NAME: &str = "tl-gpu-spawn";

/// How often `shutdown` checks whether the child has exited.
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

/// How long a child that closed its socket gets to exit on its own, so its own status is
/// reported. Checked without blocking, every [`SHUTDOWN_POLL`], and cut short by a cancel.
const EXIT_STATUS_GRACE: Duration = Duration::from_millis(100);

/// The status text of a child that was still running when it was given up.
const KILLED: &str = "killed";

/// The most `--api-versions` output kept: one frame of [`MAX_PROBE_FRAME`] and its prefix.
const MAX_API_VERSIONS_OUTPUT: usize = MAX_PROBE_FRAME + 4;

pub struct ProbeClient {
    /// Dropped first: gives the child up if it still runs.
    child: ProbeChild,
    /// A `CLOEXEC` dup of `child.sock` for `Read` and `Write`.
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
        Self::start_or_cancel(probe, extra_args, &never)
    }

    /// [`start_with_args`](Self::start_with_args) that stops waiting for `Hello` when `cancel`
    /// returns true.
    pub fn start_or_cancel(
        probe: &Path,
        extra_args: &[&OsStr],
        cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeClient, ProbeError> {
        let mut client = Self::connect(probe, extra_args)?;
        client.hello_or_cancel(cancel)?;
        Ok(client)
    }

    /// Starts the probe without sending `Hello`.
    fn connect(probe: &Path, extra_args: &[&OsStr]) -> Result<ProbeClient, ProbeError> {
        let (child, spawner) = SpawnThread::spawn(probe, extra_args)?;
        // `try_clone` sets CLOEXEC (F_DUPFD_CLOEXEC); a plain dup would leak into later children.
        let file = child
            .sock
            .try_clone()
            .map(File::from)
            .map_err(|e| ProbeError::Spawn(format!("dup: {e}").into()))?;
        Ok(ProbeClient {
            child,
            file,
            info: None,
            init_failed: None,
            seq: 0,
            _spawner: spawner,
        })
    }

    /// Sends `Hello { proto: GPU_PROTO_VERSION }` and reads the reply.
    pub fn hello(&mut self) -> Result<HelloOutcome, ProbeError> {
        self.hello_or_cancel(&never)
    }

    fn hello_or_cancel(&mut self, cancel: &dyn Fn() -> bool) -> Result<HelloOutcome, ProbeError> {
        let req = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        match self.request(&req, HELLO_DEADLINE, cancel)? {
            ProbeResponse::Hello { proto, .. } if proto != GPU_PROTO_VERSION => {
                self.child.give_up();
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
        self.sample_or_cancel(processes, &never)
    }

    /// [`sample`](Self::sample) that stops waiting when `cancel` returns true.
    pub fn sample_or_cancel(
        &mut self,
        processes: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeResponseSample, ProbeError> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let req = ProbeRequest::Sample { seq, processes };
        match self.request(&req, SAMPLE_DEADLINE, cancel)? {
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

    /// Sends `Shutdown` (errors ignored) and checks every [`SHUTDOWN_POLL`], for at most
    /// [`SHUTDOWN_GRACE`], whether the child has exited. A child that still runs then is given
    /// up: killed and reaped on a background thread.
    pub fn shutdown(mut self) {
        let _ = write_frame(&mut self.file, &ProbeRequest::Shutdown, MAX_PROBE_FRAME);
        let status = self.exit_status_within(SHUTDOWN_GRACE, &never);
        tracing::debug!(%status, "GPU probe shut down");
    }

    /// Gives the child up at once, without the `Shutdown` request and the [`SHUTDOWN_GRACE`]
    /// wait: it is killed and reaped on a background thread. For a cancelled worker.
    pub fn kill(mut self) {
        self.child.give_up();
        tracing::debug!("GPU probe killed");
    }

    fn request(
        &mut self,
        req: &ProbeRequest,
        deadline: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeResponse, ProbeError> {
        let start = Instant::now();
        if let Err(e) = write_frame(&mut self.file, req, MAX_PROBE_FRAME) {
            // EPIPE or ECONNRESET: the child closed its end, most likely because it exited.
            if let Some(status) = self.child.exited() {
                return Err(ProbeError::Exited(status_text(status)));
            }
            return Err(self.protocol(&e.to_string()));
        }

        // Every read of the frame waits with `poll` first, so a child that stops mid-frame
        // cannot hold this thread past a cancel or the deadline.
        let mut reader = CancelRead {
            file: &self.file,
            start,
            deadline,
            cancel,
            stopped: None,
        };
        let got = read_frame::<_, ProbeResponse>(&mut reader, MAX_PROBE_FRAME);
        let stopped = reader.stopped;
        match (got, stopped) {
            (Ok(resp), _) => Ok(resp),
            (Err(_), Some(Stop::Cancelled)) => {
                self.child.give_up();
                Err(ProbeError::Cancelled)
            }
            (Err(_), Some(Stop::Deadline)) => {
                self.child.give_up();
                Err(ProbeError::Deadline(deadline))
            }
            (Err(WireError::Eof | WireError::Truncated), None) => Err(ProbeError::Exited(
                self.exit_status_within(EXIT_STATUS_GRACE, cancel),
            )),
            (Err(e), None) => Err(self.protocol(&e.to_string())),
        }
    }

    /// Gives the child up, then returns [`ProbeError::Protocol`] with `text`.
    fn protocol(&mut self, text: &str) -> ProbeError {
        self.child.give_up();
        tracing::debug!(text, "GPU probe protocol error");
        ProbeError::Protocol(text.into())
    }

    /// The child's own status if it exits within `grace`, checked without blocking every
    /// [`SHUTDOWN_POLL`]. Otherwise, or as soon as `cancel` returns true, the child is given up
    /// and the text is [`KILLED`].
    fn exit_status_within(&mut self, grace: Duration, cancel: &dyn Fn() -> bool) -> Arc<str> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.exited() {
                return status_text(status);
            }
            if cancel() || start.elapsed() >= grace {
                self.child.give_up();
                return KILLED.into();
            }
            thread::sleep(SHUTDOWN_POLL);
        }
    }
}

/// Why a [`CancelRead`] stopped before the frame was complete.
#[derive(Clone, Copy, Debug)]
enum Stop {
    Cancelled,
    Deadline,
}

/// Reads the probe socket, waiting with `poll` in [`CANCEL_POLL`] slices before every read. A
/// cancel or a missed deadline ends the read with an error and is recorded in `stopped`, so a
/// frame that arrives in pieces, or stops halfway, never holds the caller (tasklens-cr0.30).
struct CancelRead<'a> {
    file: &'a File,
    start: Instant,
    deadline: Duration,
    cancel: &'a dyn Fn() -> bool,
    stopped: Option<Stop>,
}

impl CancelRead<'_> {
    fn stop(&mut self, why: Stop) -> std::io::Error {
        self.stopped = Some(why);
        // Not `Interrupted`: `read_exact` would retry that.
        std::io::Error::other(format!("{why:?}"))
    }
}

impl Read for CancelRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if (self.cancel)() {
                return Err(self.stop(Stop::Cancelled));
            }
            let Some(left) = remaining(self.deadline, self.start.elapsed()) else {
                return Err(self.stop(Stop::Deadline));
            };
            let mut fds = [PollFd::new(self.file, PollFlags::IN)];
            match poll(&mut fds, Some(&timespec(left.min(CANCEL_POLL)))) {
                // Timed out: the next turn checks for cancel and the deadline again.
                Ok(0) | Err(Errno::INTR) => {}
                // HUP or ERR with nothing to read: the child is exiting. Read as EOF.
                Ok(_) if !fds[0].revents().contains(PollFlags::IN) => return Ok(0),
                Ok(_) => break,
                Err(e) => return Err(e.into()),
            }
        }
        // Readable, so this read does not block.
        let mut file = self.file;
        file.read(buf)
    }
}

/// Runs `probe --api-versions` and reads its one `ApiVersions` frame from stdout. Blocking.
///
/// The whole run, from spawn to exit, is bounded by `deadline`. On a missed deadline, too much
/// output or any read error the child is killed and reaped before the error is returned. A
/// non-zero exit or a signal is [`ProbeError::Exited`].
pub fn api_versions(probe: &Path, deadline: Duration) -> Result<ApiVersions, ProbeError> {
    api_versions_or_cancel(probe, deadline, &never)
}

/// [`api_versions`] that kills the child and returns [`ProbeError::Cancelled`] when `cancel`
/// returns true.
pub fn api_versions_or_cancel(
    probe: &Path,
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<ApiVersions, ProbeError> {
    let start = Instant::now();
    let mut child = Command::new(probe)
        .arg("--api-versions")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env_clear()
        .envs(child_env())
        .spawn()
        .map_err(|e| ProbeError::Spawn(e.to_string().into()))?;
    let run = read_api_versions_output(&mut child, start, deadline, cancel).and_then(|output| {
        wait_until(&mut child, start, deadline, cancel).map(|status| (output, status))
    });
    let (output, status) = match run {
        Ok(run) => run,
        Err(e) => {
            kill_and_reap_in_background(child);
            return Err(e);
        }
    };
    if !status.success() {
        return Err(ProbeError::Exited(status_text(status)));
    }
    read_frame::<_, ApiVersions>(&mut &output[..], MAX_PROBE_FRAME)
        .map_err(|e| ProbeError::Protocol(e.to_string().into()))
}

/// Reads the child's stdout until EOF, waiting with `poll` for at most the rest of `deadline`.
/// The caller gives the child up on `Err`.
fn read_api_versions_output(
    child: &mut Child,
    start: Instant,
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<u8>, ProbeError> {
    let Some(mut stdout) = child.stdout.take() else {
        return Err(ProbeError::Spawn("no stdout pipe".into()));
    };
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancel() {
            return Err(ProbeError::Cancelled);
        }
        let Some(left) = remaining(deadline, start.elapsed()) else {
            return Err(ProbeError::Deadline(deadline));
        };
        let mut fds = [PollFd::new(&stdout, PollFlags::IN)];
        match poll(&mut fds, Some(&timespec(left.min(CANCEL_POLL)))) {
            // Timed out: the next turn checks for cancel and the deadline again.
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {}
            Err(e) => return Err(ProbeError::Protocol(format!("poll: {e}").into())),
        }
        // Readable or hung up, so this read does not block.
        match stdout.read(&mut chunk) {
            Ok(0) => return Ok(output),
            Ok(n) if output.len() + n > MAX_API_VERSIONS_OUTPUT => {
                return Err(ProbeError::Protocol("api-versions output too large".into()));
            }
            Ok(n) => output.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(ProbeError::Protocol(format!("read: {e}").into())),
        }
    }
}

/// Waits for the child to exit within the rest of `deadline`, checking every
/// [`SHUTDOWN_POLL`] without blocking. The caller gives a child that closed stdout but keeps
/// running up on `Err`: at the deadline, on cancel or when the check fails.
fn wait_until(
    child: &mut Child,
    start: Instant,
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<ExitStatus, ProbeError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if cancel() => return Err(ProbeError::Cancelled),
            Ok(None) if remaining(deadline, start.elapsed()).is_none() => {
                return Err(ProbeError::Deadline(deadline));
            }
            Ok(None) => thread::sleep(SHUTDOWN_POLL),
            Err(e) => return Err(ProbeError::Exited(format!("wait failed: {e}").into())),
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

/// The cancel check of the plain forms: never cancelled.
fn never() -> bool {
    false
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    use tl_model::gpu_proto::GpuApiVersions;

    use super::*;
    use crate::gpu::{API_VERSIONS_DEADLINE, PROBE_ENV};

    /// Serializes the process tests so their timings do not disturb each other.
    static LOCK: Mutex<()> = Mutex::new(());

    /// A directory of small shell scripts under `target/tmp`, removed on drop.
    struct Scripts(PathBuf);

    impl Scripts {
        fn new(name: &str) -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/tmp")
                .join(format!(
                    "tl-engine-api-versions-{name}-{}",
                    std::process::id()
                ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }

        /// A wrapper that runs `probe` with `TL_FAKE_API_VERSIONS=value`, because
        /// `api_versions` clears the child's environment. `exec` keeps the pid and the status.
        /// The pid is written to [`Scripts::pid`].
        fn fake(&self, probe: &Path, value: &str) -> PathBuf {
            let body = format!(
                "echo $$ > '{}'\nexport TL_FAKE_API_VERSIONS={value}\nexec '{}' \"$@\"",
                self.0.join("pid").display(),
                probe.display()
            );
            self.script(&format!("fake-{value}"), &body)
        }
    }

    impl Scripts {
        /// The pid the last [`Scripts::fake`] wrapper ran as.
        fn pid(&self) -> u32 {
            let text = fs::read_to_string(self.0.join("pid")).expect("read pid");
            text.trim().parse().expect("pid")
        }
    }

    impl Drop for Scripts {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The probe built with `--features fake`: `$TASKLENS_GPU_PROBE`, else the workspace debug
    /// build. `None` skips the test with a note.
    fn fake_probe() -> Option<PathBuf> {
        let path = std::env::var_os(PROBE_ENV)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/tl-gpu-probe")
            });
        let fake = Command::new(&path)
            .args(["--fake", "no-such-script"])
            .stderr(Stdio::piped())
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stderr).contains("unknown fake script"));
        if !fake {
            eprintln!(
                "skipped: no fake probe at {} (build it with \
                 `cargo build -p tl-gpu-probe --features fake`)",
                path.display()
            );
        }
        fake.then_some(path)
    }

    /// No `/proc/<pid>` entry, or one that is not a zombie.
    fn reaped(pid: u32) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => true,
            Ok(stat) => stat.rsplit_once(") ").map(|(_, rest)| &rest[..1]) != Some("Z"),
        }
    }

    #[test]
    fn api_versions_fake_ok() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = fake_probe() else { return };
        let scripts = Scripts::new("ok");
        let got = api_versions(&scripts.fake(&probe, "1"), API_VERSIONS_DEADLINE).expect("ok");
        assert_eq!(
            got.devices,
            vec![GpuApiVersions {
                pci_bus_id: "00000000:2D:00.0".to_owned(),
                vulkan_api: Some("1.4.312".to_owned()),
                vulkan_driver: Some("NVIDIA 615.71.09".to_owned()),
                opengl: Some("4.6.0 NVIDIA 615.71.09".to_owned()),
            }]
        );
    }

    #[test]
    fn api_versions_deadline() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = fake_probe() else { return };
        let scripts = Scripts::new("deadline");
        let start = Instant::now();
        let got = api_versions(&scripts.fake(&probe, "sleep"), API_VERSIONS_DEADLINE);
        let took = start.elapsed();
        assert!(matches!(got, Err(ProbeError::Deadline(d)) if d == API_VERSIONS_DEADLINE));
        assert!(took >= API_VERSIONS_DEADLINE, "{took:?}");
        // 2 s of slack for a busy machine, still below the 60 s the fake reply hangs.
        assert!(
            took < API_VERSIONS_DEADLINE + Duration::from_secs(2),
            "{took:?}"
        );
        assert!(reaped_soon(scripts.pid()), "the probe was not reaped");
    }

    #[test]
    fn api_versions_crash() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = fake_probe() else { return };
        let scripts = Scripts::new("crash");
        let got = api_versions(&scripts.fake(&probe, "crash"), API_VERSIONS_DEADLINE);
        assert!(
            matches!(&got, Err(ProbeError::Exited(text)) if &**text == "signal 11"),
            "{got:?}"
        );
        assert!(reaped(scripts.pid()), "the probe was not reaped");
    }

    #[test]
    fn api_versions_nonzero_exit_and_garbage() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("exit");
        let got = api_versions(&scripts.script("exit3", "exit 3"), API_VERSIONS_DEADLINE);
        assert!(
            matches!(&got, Err(ProbeError::Exited(text)) if &**text == "status 3"),
            "{got:?}"
        );
        // Exit 0 without a frame.
        let got = api_versions(&scripts.script("empty", "exit 0"), API_VERSIONS_DEADLINE);
        assert!(matches!(got, Err(ProbeError::Protocol(_))), "{got:?}");
    }

    #[test]
    fn api_versions_output_too_large_kills() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("large");
        // Writes zeros without end; the read stops past the cap.
        let script = scripts.script("zeros", "exec cat /dev/zero");
        let got = api_versions(&script, API_VERSIONS_DEADLINE);
        assert!(
            matches!(&got, Err(ProbeError::Protocol(text))
                if &**text == "api-versions output too large"),
            "{got:?}"
        );
    }

    #[test]
    fn api_versions_closed_stdout_still_bounded() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("closed");
        // Closes stdout at once, then keeps running.
        let script = scripts.script("closed", "exec >&-\nexec sleep 30");
        let deadline = Duration::from_millis(500);
        let start = Instant::now();
        let got = api_versions(&script, deadline);
        assert!(matches!(got, Err(ProbeError::Deadline(_))), "{got:?}");
        assert!(start.elapsed() < deadline + Duration::from_secs(2));
    }

    /// A cancel check that turns true 50 ms after it is made, once the script wrote its pid,
    /// and remembers when it first did. Tests hold the time after it fired to 2 s: loose enough
    /// for a busy machine, and still below the slow path each test rules out (`HELLO_DEADLINE`
    /// 3 s, `API_VERSIONS_DEADLINE` 5 s, [`SLOW_EXIT`] 3.6 s).
    struct CancelAfter {
        made: Instant,
        pid_file: PathBuf,
        fired: std::cell::Cell<Option<Instant>>,
    }

    impl CancelAfter {
        fn new(scripts: &Scripts) -> Self {
            Self {
                made: Instant::now(),
                pid_file: scripts.0.join("pid"),
                fired: std::cell::Cell::new(None),
            }
        }

        fn check(&self) -> bool {
            if self.fired.get().is_none()
                && self.made.elapsed() >= Duration::from_millis(50)
                && self.pid_file.exists()
            {
                self.fired.set(Some(Instant::now()));
            }
            self.fired.get().is_some()
        }

        /// Time from the first true check until now.
        fn since_fired(&self) -> Duration {
            self.fired.get().expect("cancel fired").elapsed()
        }
    }

    /// [`reaped`] within 5 s: a given-up probe is reaped on a background thread.
    fn reaped_soon(pid: u32) -> bool {
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_secs(5) {
            if reaped(pid) {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// A probe that never answers: it writes its pid and sleeps.
    fn silent_probe(scripts: &Scripts) -> PathBuf {
        let pid = scripts.0.join("pid");
        scripts.script(
            "silent",
            &format!("echo $$ > '{}'\nexec sleep 30", pid.display()),
        )
    }

    #[test]
    fn start_cancel_kills_and_reaps() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("start-cancel");
        let probe = silent_probe(&scripts);
        let cancel = CancelAfter::new(&scripts);
        let got = ProbeClient::start_or_cancel(&probe, &[], &|| cancel.check());
        let took = cancel.since_fired();
        assert!(matches!(got, Err(ProbeError::Cancelled)), "{:?}", got.err());
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!(reaped_soon(scripts.pid()), "the probe was not reaped");
    }

    #[test]
    fn api_versions_cancel_kills_and_reaps() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("api-cancel");
        let probe = silent_probe(&scripts);
        let cancel = CancelAfter::new(&scripts);
        let got = api_versions_or_cancel(&probe, API_VERSIONS_DEADLINE, &|| cancel.check());
        let took = cancel.since_fired();
        assert!(matches!(got, Err(ProbeError::Cancelled)), "{got:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!(reaped_soon(scripts.pid()), "the probe was not reaped");
    }

    /// How long [`slow_exit_wait`] takes: the longest exit of a killed probe measured on the
    /// office desktop, while the driver tore down its GPU context.
    const SLOW_EXIT: Duration = Duration::from_millis(3600);

    /// A reaper wait for a child that takes [`SLOW_EXIT`] to exit after `SIGKILL`.
    fn slow_exit_wait(pid: rustix::process::Pid) -> Result<(), Errno> {
        thread::sleep(SLOW_EXIT);
        rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty()).map(drop)
    }

    /// The pid still has a `/proc` entry, running or a zombie: nobody has reaped it yet.
    fn not_reaped_yet(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    #[test]
    fn cancel_during_slow_exit_returns_at_once() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("slow-exit-cancel");
        let probe = silent_probe(&scripts);
        let mut client = ProbeClient::connect(&probe, &[]).expect("connect");
        client.child.set_reap_wait(slow_exit_wait);
        let pid = client.child.id();
        let cancel = CancelAfter::new(&scripts);
        let got = client.hello_or_cancel(&|| cancel.check());
        let took = cancel.since_fired();
        assert!(matches!(got, Err(ProbeError::Cancelled)), "{got:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        drop(client);
        assert!(not_reaped_yet(pid), "the caller waited for the child");
        assert!(reaped_soon(pid), "the probe was not reaped");
    }

    #[test]
    fn deadline_during_slow_exit_does_not_block() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("slow-exit-deadline");
        let probe = silent_probe(&scripts);
        let mut client = ProbeClient::connect(&probe, &[]).expect("connect");
        client.child.set_reap_wait(slow_exit_wait);
        let pid = client.child.id();
        let deadline = Duration::from_millis(200);
        let req = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        let start = Instant::now();
        let got = client.request(&req, deadline, &never);
        let took = start.elapsed();
        assert!(
            matches!(got, Err(ProbeError::Deadline(d)) if d == deadline),
            "{got:?}"
        );
        assert!(took < deadline + Duration::from_secs(2), "{took:?}");
        drop(client);
        assert!(not_reaped_yet(pid), "the caller waited for the child");
        assert!(reaped_soon(pid), "the probe was not reaped");
    }

    /// A probe that sends part of a frame and stops: the 4-byte prefix for 16 payload bytes and
    /// 2 of them. The pid is written after the bytes, so [`CancelAfter`] fires only once the
    /// partial frame is on the socket.
    fn partial_frame_probe(scripts: &Scripts) -> PathBuf {
        let pid = scripts.0.join("pid");
        scripts.script(
            "partial",
            &format!(
                "printf '\\020\\000\\000\\000\\001\\002' >&3\necho $$ > '{}'\nexec sleep 30",
                pid.display()
            ),
        )
    }

    #[test]
    fn cancel_during_partial_frame_returns_at_once() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("partial-cancel");
        let probe = partial_frame_probe(&scripts);
        let cancel = CancelAfter::new(&scripts);
        let start = Instant::now();
        let got = ProbeClient::start_or_cancel(&probe, &[], &|| cancel.check());
        let took = cancel.since_fired();
        assert!(matches!(got, Err(ProbeError::Cancelled)), "{:?}", got.err());
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!(start.elapsed() < HELLO_DEADLINE, "{:?}", start.elapsed());
        assert!(reaped_soon(scripts.pid()), "the probe was not reaped");
    }

    #[test]
    fn deadline_during_partial_frame() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scripts = Scripts::new("partial-deadline");
        let probe = partial_frame_probe(&scripts);
        let mut client = ProbeClient::connect(&probe, &[]).expect("connect");
        let pid = client.child.id();
        // The partial frame is on the socket before the request starts.
        let begin = Instant::now();
        while !scripts.0.join("pid").exists() && begin.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(5));
        }
        let deadline = Duration::from_millis(200);
        let req = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        let start = Instant::now();
        let got = client.request(&req, deadline, &never);
        let took = start.elapsed();
        assert!(
            matches!(got, Err(ProbeError::Deadline(d)) if d == deadline),
            "{got:?}"
        );
        assert!(took < deadline + Duration::from_secs(2), "{took:?}");
        drop(client);
        assert!(reaped_soon(pid), "the probe was not reaped");
    }

    #[test]
    fn api_versions_missing_binary_is_spawn() {
        let got = api_versions(
            Path::new("/nonexistent/tasklens/tl-gpu-probe"),
            API_VERSIONS_DEADLINE,
        );
        assert!(matches!(got, Err(ProbeError::Spawn(_))), "{got:?}");
    }

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

    /// The socket copy keeps `CLOEXEC`, so it never leaks into a child spawned later.
    #[test]
    fn socket_copy_is_cloexec() {
        let probe = std::env::var_os(crate::gpu::PROBE_ENV)
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/tl-gpu-probe")
            });
        if !probe.is_file() {
            eprintln!("skipped: no probe at {}", probe.display());
            return;
        }
        let client = ProbeClient::start(&probe).expect("start probe");
        let flags = rustix::io::fcntl_getfd(&client.file).unwrap();
        assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    }
}
