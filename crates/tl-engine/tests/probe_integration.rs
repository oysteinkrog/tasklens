//! Integration tests for the GPU probe (ADR-007 verification list, ADR-013, E13-008 to E13-011).
//!
//! Two kinds of test share this file:
//! - `client_*`, `api_versions_*`, `spawn_*` and `pdeathsig_*` run the real `tl-gpu-probe`,
//!   built with `--features fake`, on wall time. They hold [`REAL`] so deadline timings are not
//!   disturbed by parallel spawns, and each one checks that no probe child is left behind.
//! - `worker_*`, `supervised_*` and `explore_*` run the `GpuNvidia` worker in a `LabRuntime` on
//!   virtual time over a scripted [`FakeLink`]. More worker cases (resent `GpuInfo`,
//!   `RefreshNow`) are unit tests in `workers/gpu_nvidia.rs`.
//! - `engine_gpu_*` run the whole engine (`engine_root` with the reducer) in a lab, with the
//!   `GpuNvidia` spec over a [`FakeLink`] as its only worker (E13-A2).

#[path = "fake_sources.rs"]
mod fake_sources;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use asupersync::lab::{LabConfig, LabRuntime};
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::supervision::ManagedGeneration;
use asupersync::time::{TimeSource, VirtualClock};
use asupersync::types::{Budget, Outcome, RegionId, Time};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::controller::SourceController;
use tl_engine::dbus::SharedHandles;
use tl_engine::gpu::client::{HelloInfo, HelloOutcome, ProbeClient, ProbeResponseSample};
use tl_engine::gpu::spawn::{child_env, spawn_probe};
use tl_engine::gpu::{
    API_VERSIONS_DEADLINE, HELLO_DEADLINE, NOT_INSTALLED, PROBE_ENV, ProbeError, SAMPLE_DEADLINE,
    SHUTDOWN_GRACE,
};
use tl_engine::session::engine_root;
use tl_engine::source::{WorkerControl, WorkerIo};
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{EngineMsg, GpuSample, SourceUpdate, WorkerError};
use tl_engine::workers::WorkerSpec;
use tl_engine::workers::gpu_nvidia::{self, PARK_RECHECK, PERIOD, ProbeLink, run_with};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::gpu_proto::{
    ApiVersions, GPU_PROTO_VERSION, GpuApiVersions, GpuDeviceInfo, GpuDeviceSample,
    GpuProcessSample, InitFailure, ProbeRequest, ProbeResponse, Reading,
};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::series::{Sample, SampleFlags};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::system::Metric;
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::wire::{MAX_PROBE_FRAME, read_frame, write_frame};

// Real fake binary ----------------------------------------------------------------------------

/// Serialises every test that spawns the real probe.
static REAL: Mutex<()> = Mutex::new(());

const PROBE_COMM: &str = "tl-gpu-probe";

/// Set in the second copy of this test binary that `pdeathsig_kills_orphan` starts.
const ORPHAN_ENV: &str = "TL_PROBE_TEST_ORPHAN";

fn real() -> MutexGuard<'static, ()> {
    REAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// `$TASKLENS_GPU_PROBE` if set, else the workspace target dir binary. That one is built once
/// per test run with `cargo build -p tl-gpu-probe --features fake`, so a plain build without the
/// `fake` feature is replaced.
fn fake_probe() -> PathBuf {
    static PROBE: OnceLock<PathBuf> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            if let Some(path) = std::env::var_os(PROBE_ENV).filter(|p| !p.is_empty()) {
                return PathBuf::from(path);
            }
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let run = "cargo build -p tl-gpu-probe --features fake";
            let out = Command::new(cargo)
                .args(["build", "-p", "tl-gpu-probe", "--features", "fake"])
                .current_dir(&workspace)
                .stdin(Stdio::null())
                .output()
                .unwrap_or_else(|e| panic!("could not start `{run}`: {e}"));
            assert!(
                out.status.success(),
                "`{run}` failed ({}); run it by hand, or set {PROBE_ENV}:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map_or_else(|| workspace.join("target"), PathBuf::from);
            let path = target.join("debug").join(PROBE_COMM);
            assert!(path.is_file(), "`{run}` left no {}", path.display());
            path
        })
        .clone()
}

fn start(script: &str) -> Result<ProbeClient, ProbeError> {
    ProbeClient::start_with_args(&fake_probe(), &[OsStr::new("--fake"), OsStr::new(script)])
}

/// `(state, ppid)` from `/proc/<pid>/stat`, or `None` when the process is gone.
fn stat(pid: u32) -> Option<(char, u32)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = text.rsplit_once(") ")?;
    let mut fields = rest.split(' ');
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    Some((state, ppid))
}

fn comm(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let start = text.find('(')?;
    let end = text.rfind(')')?;
    Some(text[start + 1..end].to_owned())
}

/// Every child of this process named `tl-gpu-probe`, zombies included.
fn probe_children() -> Vec<u32> {
    let me = std::process::id();
    let mut pids: Vec<u32> = std::fs::read_dir("/proc")
        .expect("read /proc")
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&pid| {
            comm(pid).as_deref() == Some(PROBE_COMM) && stat(pid).is_some_and(|s| s.1 == me)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The one probe child this test started.
fn child_pid() -> u32 {
    let kids = probe_children();
    assert_eq!(kids.len(), 1, "probe children {kids:?}");
    kids[0]
}

/// `/proc/<pid>/stat` absent, or present with a state other than `Z`, polled for 500 ms.
fn no_zombie(pid: u32) -> bool {
    let end = Instant::now() + Duration::from_millis(500);
    loop {
        match stat(pid) {
            None => return true,
            Some((state, _)) if Instant::now() > end => return state != 'Z',
            Some(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// The end of every real-binary test: `no_zombie` for each known pid, and no probe child of
/// this process left at all, running or not.
fn assert_clean(pids: &[u32]) {
    for &pid in pids {
        assert!(no_zombie(pid), "pid {pid} is a zombie");
    }
    let end = Instant::now() + Duration::from_millis(500);
    while !probe_children().is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(probe_children(), Vec::<u32>::new(), "probe children left");
}

#[test]
fn client_normal_hello_and_samples() {
    let _g = real();
    let mut c = start("normal").expect("start");
    let info = c.info.clone().expect("hello info");
    assert_eq!(info.devices.len(), 1);
    assert!(!info.driver.is_empty());
    for want in 1..=3 {
        let s = c.sample(false).expect("sample");
        assert_eq!(s.seq, want);
        assert_eq!(s.devices.len(), 1);
        assert!(s.processes.is_none());
    }
    let s = c.sample(true).expect("process sample");
    assert_eq!(s.processes.map(|p| p.len()), Some(1));
    let pid = child_pid();
    c.shutdown();
    assert_clean(&[pid]);
}

#[test]
fn client_sample_deadline_kills_and_reaps() {
    let _g = real();
    let mut c = start("sleep-on-sample").expect("start");
    let pid = child_pid();
    let t = Instant::now();
    let r = c.sample(false);
    let took = t.elapsed();
    assert!(
        matches!(r, Err(ProbeError::Deadline(d)) if d == SAMPLE_DEADLINE),
        "{r:?}"
    );
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_millis(700),
        "{took:?}"
    );
    // Killed and reaped before `sample` returned.
    assert_eq!(stat(pid), None);
    drop(c);
    assert_clean(&[pid]);
}

#[test]
fn client_hello_deadline() {
    let _g = real();
    let t = Instant::now();
    let r = start("sleep-on-hello");
    assert!(
        matches!(r, Err(ProbeError::Deadline(d)) if d == HELLO_DEADLINE),
        "{:?}",
        r.err()
    );
    let took = t.elapsed();
    assert!(
        took < HELLO_DEADLINE + Duration::from_millis(300),
        "{took:?}"
    );
    assert_clean(&[]);
}

#[test]
fn client_segv_is_exited_signal_11() {
    let _g = real();
    let mut c = start("segv-on-sample").expect("start");
    let pid = child_pid();
    let r = c.sample(false);
    assert!(
        matches!(&r, Err(ProbeError::Exited(s)) if &**s == "signal 11"),
        "{r:?}"
    );
    drop(c);
    assert_clean(&[pid]);
}

#[test]
fn client_oversize_is_protocol() {
    let _g = real();
    let mut c = start("oversize-on-sample").expect("start");
    let pid = child_pid();
    let r = c.sample(false);
    assert!(matches!(&r, Err(ProbeError::Protocol(_))), "{r:?}");
    drop(c);
    assert_clean(&[pid]);
}

#[test]
fn client_proto_mismatch() {
    let _g = real();
    let r = start("proto-mismatch");
    assert!(
        matches!(r, Err(ProbeError::VersionMismatch(v)) if v == GPU_PROTO_VERSION - 1),
        "{:?}",
        r.err()
    );
    assert_clean(&[]);
}

#[test]
fn client_init_failed_returns_client() {
    let _g = real();
    let c = start("init-library-not-found").expect("start");
    let pid = child_pid();
    assert!(c.info.is_none());
    assert!(matches!(
        &c.init_failed,
        Some((InitFailure::LibraryNotFound, _))
    ));
    c.shutdown();
    assert_clean(&[pid]);
}

#[test]
fn client_exit_after_three() {
    let _g = real();
    let mut c = start("exit-after-3").expect("start");
    let pid = child_pid();
    for _ in 0..3 {
        c.sample(false).expect("sample");
    }
    let r = c.sample(false);
    assert!(
        matches!(&r, Err(ProbeError::Exited(s)) if &**s == "status 0"),
        "{r:?}"
    );
    drop(c);
    assert_clean(&[pid]);
}

#[test]
fn client_shutdown_graceful() {
    let _g = real();
    let c = start("normal").expect("start");
    let pid = child_pid();
    let t = Instant::now();
    c.shutdown();
    assert!(t.elapsed() < SHUTDOWN_GRACE, "{:?}", t.elapsed());
    assert_eq!(stat(pid), None);
    assert_clean(&[pid]);
}

/// R-283: the child's forking thread lives as long as the client, so the parent-death signal
/// does not fire when the thread that called `start` ends.
#[test]
fn client_survives_calling_thread() {
    let _g = real();
    let mut c = std::thread::spawn(|| start("normal").expect("start"))
        .join()
        .expect("start thread");
    std::thread::sleep(Duration::from_millis(100));
    let pid = child_pid();
    assert_eq!(c.sample(false).expect("sample").seq, 1);
    c.shutdown();
    assert_clean(&[pid]);
}

/// A `/bin/sh` wrapper that runs the fake probe with `TL_FAKE_API_VERSIONS=<value>`.
/// `api_versions` clears the child's environment, so the variable cannot come from here.
fn api_wrapper(value: &str) -> PathBuf {
    let probe = fake_probe();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "tl-gpu-probe-api-{value}-{}.sh",
        std::process::id()
    ));
    let script = format!(
        "#!/bin/sh\nTL_FAKE_API_VERSIONS={value}\nexport TL_FAKE_API_VERSIONS\nexec '{}' \"$@\"\n",
        probe.display()
    );
    std::fs::write(&path, script).expect("write wrapper");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[test]
fn api_versions_fake_ok() {
    let _g = real();
    let wrapper = api_wrapper("1");
    let r = tl_engine::gpu::client::api_versions(&wrapper, API_VERSIONS_DEADLINE);
    let versions = r.expect("api versions");
    assert_eq!(versions.devices.len(), 1);
    assert_eq!(versions.devices[0].pci_bus_id, "00000000:2D:00.0");
    assert_eq!(versions.devices[0].vulkan_api.as_deref(), Some("1.4.312"));
    assert_clean(&[]);
}

#[test]
fn api_versions_deadline() {
    let _g = real();
    let wrapper = api_wrapper("sleep");
    let t = Instant::now();
    let r = tl_engine::gpu::client::api_versions(&wrapper, API_VERSIONS_DEADLINE);
    let took = t.elapsed();
    assert!(
        matches!(r, Err(ProbeError::Deadline(d)) if d == API_VERSIONS_DEADLINE),
        "{r:?}"
    );
    assert!(
        took >= API_VERSIONS_DEADLINE && took < API_VERSIONS_DEADLINE + Duration::from_millis(500),
        "{took:?}"
    );
    assert_clean(&[]);
}

#[test]
fn api_versions_crash() {
    let _g = real();
    let wrapper = api_wrapper("crash");
    let r = tl_engine::gpu::client::api_versions(&wrapper, API_VERSIONS_DEADLINE);
    assert!(
        matches!(&r, Err(ProbeError::Exited(s)) if &**s == "signal 11"),
        "{r:?}"
    );
    assert_clean(&[]);
}

/// Runs only in the copy of this binary that `pdeathsig_kills_orphan` starts. It spawns the
/// probe with the `sleep-on-hello` script, sends `Hello` so the probe sleeps for 5 s instead of
/// reading the socket, prints the probe's pid and exits without killing it.
#[test]
fn pdeathsig_orphan_helper() {
    if std::env::var_os(ORPHAN_ENV).is_none() {
        return;
    }
    let probe = fake_probe();
    let child = spawn_probe(
        &probe,
        &[OsStr::new("--fake"), OsStr::new("sleep-on-hello")],
    )
    .expect("spawn probe");
    let mut sock = UnixStream::from(child.sock.try_clone().expect("clone sock"));
    let hello = ProbeRequest::Hello {
        proto: GPU_PROTO_VERSION,
    };
    write_frame(&mut sock, &hello, MAX_PROBE_FRAME).expect("write hello");
    let mut out = std::io::stdout().lock();
    writeln!(out, "probe-pid {}", child.child.id()).expect("write pid");
    out.flush().expect("flush");
    // No cleanup: neither the kill in `ProbeChild::drop` nor closing the socket runs.
    std::mem::forget(child);
    std::mem::forget(sock);
    std::process::exit(0);
}

#[test]
fn pdeathsig_kills_orphan() {
    let _g = real();
    let probe = fake_probe();
    let me = std::env::current_exe().expect("current exe");
    let out = Command::new(me)
        .args(["--exact", "pdeathsig_orphan_helper", "--nocapture"])
        .env(ORPHAN_ENV, "1")
        .env(PROBE_ENV, &probe)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("run helper");
    let gone_from = Instant::now();
    assert!(out.status.success(), "helper: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pid: u32 = stdout
        .lines()
        .find_map(|l| l.strip_prefix("probe-pid ")?.parse().ok())
        .unwrap_or_else(|| panic!("no probe pid in helper output: {stdout}"));
    // The orphan is reaped by init or a subreaper, not by us, so a dead but not yet reaped
    // probe (`Z`) also counts as killed. Without the signal it would sleep on for 5 s.
    loop {
        match stat(pid) {
            None => break,
            Some(('Z', _)) => break,
            Some(_) if comm(pid).as_deref() != Some(PROBE_COMM) => break,
            Some(_) => {}
        }
        assert!(
            gone_from.elapsed() < Duration::from_millis(100),
            "probe {pid} alive 100 ms after its parent exited"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_clean(&[]);
}

/// E13-008: the socket on fd 3, no other fd of the test process, the minimal environment, and a
/// drop that kills and reaps.
#[test]
fn spawn_fd3_env_and_fds() {
    let _g = real();
    let probe = fake_probe();
    let child = spawn_probe(&probe, &[OsStr::new("--fake"), OsStr::new("normal")]).expect("spawn");
    let pid = child.child.id();

    // `/proc` is read only after the `Hello` reply. `spawn` returns when exec closes the
    // CLOEXEC fds, which is before the kernel has loaded the new image, so `environ` may still
    // be empty then (`env_end` is 0). By the reply, ld.so has closed its fds and `open_fd` has
    // closed its pidfd and keeps a copy of fd 3.
    let mut sock = UnixStream::from(child.sock.try_clone().expect("clone sock"));
    sock.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let hello = ProbeRequest::Hello {
        proto: GPU_PROTO_VERSION,
    };
    write_frame(&mut sock, &hello, MAX_PROBE_FRAME).expect("write hello");
    let reply: ProbeResponse = read_frame(&mut sock, MAX_PROBE_FRAME).expect("read hello");
    assert!(
        matches!(reply, ProbeResponse::Hello { proto, .. } if proto == GPU_PROTO_VERSION),
        "{reply:?}"
    );

    let target = |fd: i32| std::fs::read_link(format!("/proc/{pid}/fd/{fd}"));
    let mut fds: Vec<i32> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .expect("read fd dir")
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    fds.sort_unstable();
    let targets: Vec<String> = fds
        .iter()
        .map(|&fd| format!("{fd}: {:?}", target(fd)))
        .collect();
    let fd3 = target(3).expect("readlink fd 3");
    assert!(fd3.to_string_lossy().starts_with("socket:["), "{fd3:?}");
    // fds 0 to 3, plus the probe's own copy of the fd 3 socket and nothing else.
    let (low, high): (Vec<i32>, Vec<i32>) = fds.iter().partition(|&&fd| fd <= 3);
    assert_eq!(low, vec![0, 1, 2, 3], "{targets:?}");
    assert_eq!(high.len(), 1, "{targets:?}");
    assert!(
        high.iter()
            .all(|&fd| target(fd).ok().as_ref() == Some(&fd3)),
        "{targets:?}"
    );

    let environ = std::fs::read(format!("/proc/{pid}/environ")).expect("read environ");
    let mut got: Vec<Vec<u8>> = environ
        .split(|&b| b == 0)
        .filter(|e| !e.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    let mut want: Vec<Vec<u8>> = child_env()
        .into_iter()
        .map(|(k, v)| {
            let mut entry = k.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(v.as_encoded_bytes());
            entry
        })
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);

    drop(child);
    // The drop kills the child and reaps it on a background thread.
    let end = Instant::now() + Duration::from_millis(500);
    while stat(pid).is_some() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(stat(pid), None, "drop kills and reaps");
    drop(sock);
    assert_clean(&[pid]);
}

// LabRuntime worker ---------------------------------------------------------------------------

const UUID: [u8; 16] = [0x42; 16];
const FAKE_PATH: &str = "/fake/tl-gpu-probe";

/// What the next call of one kind returns. Each call takes the first step of its kind; with
/// none left, `start` is `Ready`, `sample` succeeds and `api_versions` fails.
enum LinkStep {
    Hello(HelloOutcome),
    StartErr(ProbeError),
    /// `Ok` is a normal sample.
    Sample(Result<(), ProbeError>),
    Api(Result<ApiVersions, ProbeError>),
}

/// One call the worker made. The number is the connection: 1 for the first `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LinkCall {
    Start(u32),
    Sample {
        conn: u32,
        processes: bool,
    },
    ApiVersions(u32),
    Shutdown(u32),
    /// The stop of a cancelled generation.
    Kill(u32),
}

/// The script and the call log, shared by every connection the worker starts. [`Conn`] is the
/// `ProbeLink` the worker holds; `ProbeLink::start` returns `Self` by value, so the shared part
/// cannot be the link itself.
struct FakeLink {
    script: Mutex<VecDeque<LinkStep>>,
    calls: Mutex<Vec<LinkCall>>,
    /// Sample times for the engine tests: the base plus the clock's virtual time. `None` gives
    /// the connection's sample number.
    time: Option<(Arc<VirtualClock>, TimeNs)>,
}

impl FakeLink {
    fn new(script: impl IntoIterator<Item = LinkStep>) -> Arc<Self> {
        Arc::new(FakeLink {
            script: Mutex::new(script.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
            time: None,
        })
    }

    /// Samples stamped `base + clock`. The base is an hour after the real boot time now, so
    /// the reducer's stale check, which reads the real clock, never sees an old sample.
    fn timed(script: impl IntoIterator<Item = LinkStep>, clock: &Arc<VirtualClock>) -> Arc<Self> {
        Arc::new(FakeLink {
            script: Mutex::new(script.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
            time: Some((Arc::clone(clock), now_boot_ns() + 3_600 * 1_000_000_000)),
        })
    }

    fn take(&self, kind: impl Fn(&LinkStep) -> bool) -> Option<LinkStep> {
        let mut script = self.script.lock().expect("script lock");
        let at = script.iter().position(kind)?;
        script.remove(at)
    }

    fn log(&self, call: LinkCall) {
        self.calls.lock().expect("calls lock").push(call);
    }

    fn calls(&self) -> Vec<LinkCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn count(&self, f: impl Fn(&LinkCall) -> bool) -> usize {
        self.calls().iter().filter(|c| f(c)).count()
    }

    fn starts(&self) -> usize {
        self.count(|c| matches!(c, LinkCall::Start(_)))
    }

    fn samples(&self) -> Vec<bool> {
        self.calls()
            .iter()
            .filter_map(|c| match c {
                LinkCall::Sample { processes, .. } => Some(*processes),
                _ => None,
            })
            .collect()
    }

    /// `shutdown` calls per connection, for every connection started.
    fn shutdowns_per_conn(&self) -> Vec<usize> {
        let starts = u32::try_from(self.starts()).expect("fits");
        (1..=starts)
            .map(|conn| self.count(|c| *c == LinkCall::Shutdown(conn)))
            .collect()
    }

    /// `kill` calls per connection, for every connection started.
    fn kills_per_conn(&self) -> Vec<usize> {
        let starts = u32::try_from(self.starts()).expect("fits");
        (1..=starts)
            .map(|conn| self.count(|c| *c == LinkCall::Kill(conn)))
            .collect()
    }
}

struct Conn {
    link: Arc<FakeLink>,
    id: u32,
    seq: u32,
}

impl ProbeLink for Conn {
    type Starter = Arc<FakeLink>;

    fn start(
        link: Arc<FakeLink>,
        probe: &Path,
        _cancel: &dyn Fn() -> bool,
    ) -> Result<(Self, HelloOutcome), ProbeError> {
        assert_eq!(probe, Path::new(FAKE_PATH));
        let id = u32::try_from(link.starts()).expect("fits") + 1;
        link.log(LinkCall::Start(id));
        let hello = match link.take(|s| matches!(s, LinkStep::Hello(_) | LinkStep::StartErr(_))) {
            Some(LinkStep::StartErr(e)) => return Err(e),
            Some(LinkStep::Hello(hello)) => hello,
            _ => HelloOutcome::Ready(hello_info()),
        };
        Ok((Conn { link, id, seq: 0 }, hello))
    }

    fn sample(
        &mut self,
        processes: bool,
        _cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeResponseSample, ProbeError> {
        self.link.log(LinkCall::Sample {
            conn: self.id,
            processes,
        });
        if let Some(LinkStep::Sample(Err(e))) = self.link.take(|s| matches!(s, LinkStep::Sample(_)))
        {
            return Err(e);
        }
        self.seq += 1;
        let t_ns = match &self.link.time {
            Some((clock, base)) => base + clock.now().as_nanos(),
            None => u64::from(self.seq),
        };
        Ok(ProbeResponseSample {
            seq: self.seq,
            t_ns,
            devices: vec![device_sample()],
            processes: processes.then(|| vec![process_sample()]),
        })
    }

    fn api_versions(
        &self,
        probe: &Path,
        _cancel: &dyn Fn() -> bool,
    ) -> Result<ApiVersions, ProbeError> {
        assert_eq!(probe, Path::new(FAKE_PATH));
        self.link.log(LinkCall::ApiVersions(self.id));
        match self.link.take(|s| matches!(s, LinkStep::Api(_))) {
            Some(LinkStep::Api(r)) => r,
            _ => Err(ProbeError::Exited("signal 11".into())),
        }
    }

    fn shutdown(self) {
        self.link.log(LinkCall::Shutdown(self.id));
    }

    fn kill(self) {
        self.link.log(LinkCall::Kill(self.id));
    }
}

fn device_info() -> GpuDeviceInfo {
    GpuDeviceInfo {
        uuid: UUID,
        name: "NVIDIA GeForce RTX 4070".into(),
        pci_bus_id: "00000000:2D:00.0".into(),
        mem_total: 16 << 30,
        power_limit_mw: Some(200_000),
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
    }
}

fn hello_info() -> HelloInfo {
    HelloInfo {
        proto: GPU_PROTO_VERSION,
        driver: "615.71.09".into(),
        nvml: "13.615.71.09".into(),
        devices: Arc::from([device_info()]),
    }
}

fn device_sample() -> GpuDeviceSample {
    GpuDeviceSample {
        uuid: UUID,
        util_pct: Reading::Ok(37),
        mem_util_pct: Reading::Ok(12),
        mem_used: Reading::Ok(3 << 30),
        mem_reserved: Reading::Unsupported,
        enc_pct: Reading::Ok(0),
        dec_pct: Reading::Ok(0),
        enc_sessions: Reading::Unsupported,
        jpeg_pct: Reading::Ok(0),
        ofa_pct: Reading::Unsupported,
        temp_c: Reading::Ok(51),
        power_mw: Reading::Ok(31_900),
        clock_gfx_mhz: Reading::Ok(2_100),
        clock_mem_mhz: Reading::Ok(10_501),
        clock_video_mhz: Reading::Ok(1_620),
        pstate: Reading::Ok(3),
        clock_event_reasons: Reading::Ok(0),
        fan_pct: Reading::Ok(30),
        fan_rpm: Reading::Ok(1_100),
        pcie_rx_kbps: Reading::Ok(1_000),
        pcie_tx_kbps: Reading::Ok(500),
        pcie_gen: Reading::Ok(4),
        pcie_width: Reading::Ok(16),
        energy_mj: Reading::Ok(4_464_000_000),
    }
}

fn process_sample() -> GpuProcessSample {
    GpuProcessSample {
        uuid: UUID,
        pid: 4242,
        sm_pct: Reading::Ok(5),
        mem_pct: Reading::Ok(1),
        enc_pct: Reading::Ok(0),
        dec_pct: Reading::Ok(0),
        mem_used: Reading::Ok(1 << 20),
    }
}

fn api_fixture() -> ApiVersions {
    ApiVersions {
        devices: vec![GpuApiVersions {
            pci_bus_id: "00000000:2D:00.0".into(),
            vulkan_api: Some("1.4.312".into()),
            vulkan_driver: Some("NVIDIA 615.71.09".into()),
            opengl: Some("4.6.0 NVIDIA 615.71.09".into()),
        }],
    }
}

fn visible_gpus() -> Demand {
    Demand {
        any_surface_visible: true,
        gpus: true,
        ..Demand::default()
    }
}

/// A flag of its own per worker, so parallel tests do not share the process-wide one.
fn fresh_once() -> &'static AtomicBool {
    Box::leak(Box::new(AtomicBool::new(false)))
}

/// The lab's root region, created on first use. A lab has only one.
fn root(lab: &mut LabRuntime) -> RegionId {
    match lab.state.root_region {
        Some(root) => root,
        None => lab.state.create_root_region(Budget::INFINITE),
    }
}

/// Worker inputs whose senders the test keeps: updates, demand and control.
fn worker_io(
    demand: Demand,
) -> (
    WorkerIo,
    mpsc::Receiver<EngineMsg>,
    watch::Sender<Demand>,
    watch::Sender<WorkerControl>,
) {
    let (updates, rx) = mpsc::channel::<EngineMsg>(256);
    let (demand_tx, demand_rx) = watch::channel(demand);
    let (_settings_tx, settings) = watch::channel(EngineSettings::default());
    let (control_tx, control_rx) = watch::channel(WorkerControl::default());
    let io = WorkerIo::new(
        updates,
        demand_rx,
        settings,
        control_rx,
        SharedHandles::default(),
    );
    (io, rx, demand_tx, control_tx)
}

/// What [`lab_worker`] returns: the lab, the update queue, the demand sender and the task.
type LabWorker = (
    LabRuntime,
    mpsc::Receiver<EngineMsg>,
    watch::Sender<Demand>,
    TaskHandle<Outcome<(), WorkerError>>,
);

/// One `run_with` call as a lab root task, run until idle.
fn lab_worker(link: Arc<FakeLink>, demand: Demand) -> LabWorker {
    lab_worker_with(link, demand, true, fresh_once())
}

fn lab_worker_with(
    link: Arc<FakeLink>,
    demand: Demand,
    installed: bool,
    api_once: &'static AtomicBool,
) -> LabWorker {
    let mut lab = LabRuntime::new(LabConfig::new(0xe13).max_steps(10_000_000));
    let (io, rx, demand_tx, _control) = worker_io(demand);
    let locate = move || installed.then(|| PathBuf::from(FAKE_PATH));
    let root = root(&mut lab);
    let (task, handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("lab task has a Cx");
            run_with::<Conn>(cx, io, locate, link, api_once).await
        })
        .expect("create task");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_until_idle();
    (lab, rx, demand_tx, handle)
}

fn advance(lab: &mut LabRuntime, d: Duration) {
    lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
    // `run_until_idle` stops when no task is ready; one step fires the due timers.
    lab.step_for_test();
    lab.run_until_idle();
}

fn drain(rx: &mut mpsc::Receiver<EngineMsg>) -> Vec<SourceUpdate> {
    let mut got = Vec::new();
    while let Ok(EngineMsg::Source(update)) = rx.try_recv() {
        got.push(update);
    }
    got
}

fn outcome(handle: &mut TaskHandle<Outcome<(), WorkerError>>) -> Option<Outcome<(), WorkerError>> {
    handle.try_join().expect("worker task joins")
}

fn is_health(u: &SourceUpdate, want: SourceState, text: Option<&str>) -> bool {
    matches!(
        u,
        SourceUpdate::Health { id: SourceId::GpuNvidia, state, last_error, .. }
            if *state == want && last_error.as_deref() == text
    )
}

fn gpu_samples(updates: &[SourceUpdate]) -> Vec<&GpuSample> {
    updates
        .iter()
        .filter_map(|u| match u {
            SourceUpdate::Gpu(s) => Some(s),
            _ => None,
        })
        .collect()
}

#[test]
fn worker_not_installed_parks() {
    let link = FakeLink::new([]);
    let (mut lab, mut rx, _demand, mut handle) =
        lab_worker_with(Arc::clone(&link), visible_gpus(), false, fresh_once());
    let got = drain(&mut rx);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        is_health(&got[0], SourceState::Unsupported, Some(NOT_INSTALLED)),
        "{got:?}"
    );
    for _ in 0..10 {
        advance(&mut lab, PARK_RECHECK);
    }
    assert!(
        outcome(&mut handle).is_none(),
        "still parked after 10 minutes"
    );
    assert!(link.calls().is_empty(), "{:?}", link.calls());
    assert!(drain(&mut rx).is_empty());
}

/// ADR-007 once retried `Hello` every 300 s here. R-252 made `LibraryNotFound` and
/// `DriverNotLoaded` terminal: the worker shuts the child down and parks, and still has not
/// retried after 300 s. `NoPermission` and `Other` are `Err`, which the supervisor restarts.
#[test]
fn worker_init_retry_300s() {
    for kind in [InitFailure::LibraryNotFound, InitFailure::DriverNotLoaded] {
        let link = FakeLink::new([LinkStep::Hello(HelloOutcome::InitFailed {
            kind,
            message: "fake".into(),
        })]);
        let (mut lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
        let text = format!("{kind:?}");
        let got = drain(&mut rx);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            is_health(&got[0], SourceState::Unsupported, Some(&text)),
            "{got:?}"
        );
        for _ in 0..6 {
            advance(&mut lab, PARK_RECHECK);
        }
        assert!(
            outcome(&mut handle).is_none(),
            "{kind:?}: parked after 360 s"
        );
        assert_eq!(
            link.calls(),
            vec![LinkCall::Start(1), LinkCall::Shutdown(1)],
            "{kind:?}"
        );
        assert!(drain(&mut rx).is_empty());
    }
    for kind in [InitFailure::NoPermission, InitFailure::Other] {
        let link = FakeLink::new([LinkStep::Hello(HelloOutcome::InitFailed {
            kind,
            message: "fake".into(),
        })]);
        let (_lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
        let out = outcome(&mut handle).expect("returned at once");
        assert!(
            matches!(&out, Outcome::Err(WorkerError::Probe(_))),
            "{kind:?}: {out:?}"
        );
        assert_eq!(
            link.calls(),
            vec![LinkCall::Start(1), LinkCall::Shutdown(1)]
        );
        assert!(drain(&mut rx).is_empty());
    }
}

#[test]
fn worker_samples_at_1hz_and_process_flag() {
    let link = FakeLink::new([]);
    let (mut lab, mut rx, demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
    let got = drain(&mut rx);
    assert!(
        matches!(&got[0], SourceUpdate::GpuInfo(info) if info.devices.len() == 1),
        "{got:?}"
    );
    assert!(is_health(&got[1], SourceState::Running, None), "{got:?}");
    assert_eq!(gpu_samples(&got).len(), 1, "first sample at 0 s: {got:?}");
    for second in 1..10 {
        advance(&mut lab, PERIOD);
        let got = drain(&mut rx);
        let gpu = gpu_samples(&got);
        assert_eq!(gpu.len(), 1, "one sample at {second} s: {got:?}");
        assert!(gpu[0].processes.is_none());
        assert_eq!(gpu[0].devices.len(), 1);
    }
    assert_eq!(link.samples(), vec![false; 10]);

    demand
        .send(Demand {
            gpu_process: true,
            ..visible_gpus()
        })
        .expect("demand sends");
    advance(&mut lab, PERIOD);
    let got = drain(&mut rx);
    let gpu = gpu_samples(&got);
    assert_eq!(gpu.len(), 1, "{got:?}");
    assert_eq!(gpu[0].processes.as_ref().map(Vec::len), Some(1));
    assert_eq!(link.samples().last(), Some(&true));
    assert!(outcome(&mut handle).is_none());
}

#[test]
fn worker_idle_sends_nothing() {
    let link = FakeLink::new([]);
    let (mut lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), Demand::default());
    let got = drain(&mut rx);
    assert_eq!(got.len(), 2, "GpuInfo and Running only: {got:?}");
    for _ in 0..60 {
        advance(&mut lab, PERIOD);
    }
    assert!(link.samples().is_empty());
    assert!(drain(&mut rx).is_empty());
    assert!(outcome(&mut handle).is_none());
}

#[test]
fn worker_sample_error_restarts() {
    let link = FakeLink::new([LinkStep::Sample(Err(ProbeError::Deadline(SAMPLE_DEADLINE)))]);
    let (_lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
    let out = outcome(&mut handle).expect("returned after the first sample");
    assert!(
        matches!(&out, Outcome::Err(WorkerError::Probe(text)) if text.contains("deadline")),
        "{out:?}"
    );
    // The client already reaped the child, so no `shutdown`.
    assert_eq!(
        link.calls(),
        vec![
            LinkCall::Start(1),
            LinkCall::ApiVersions(1),
            LinkCall::Sample {
                conn: 1,
                processes: false
            },
        ]
    );
    assert!(gpu_samples(&drain(&mut rx)).is_empty());

    // A failed spawn is `Err` too, with nothing to shut down.
    let link = FakeLink::new([LinkStep::StartErr(ProbeError::Spawn("ENOENT".into()))]);
    let (_lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
    let out = outcome(&mut handle).expect("returned after start");
    assert!(
        matches!(&out, Outcome::Err(WorkerError::Probe(text)) if text.contains("spawn")),
        "{out:?}"
    );
    assert_eq!(link.calls(), vec![LinkCall::Start(1)]);
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn worker_cancel_shuts_down_once() {
    let link = FakeLink::new([]);
    let (mut lab, _rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
    for _ in 0..3 {
        advance(&mut lab, PERIOD);
    }
    handle.abort();
    lab.run_until_idle();
    for _ in 0..5 {
        if handle.is_finished() {
            break;
        }
        advance(&mut lab, PERIOD);
    }
    // The join of an aborted task reports the abort, whatever the body returned.
    let out = handle.try_join();
    assert!(
        matches!(
            out,
            Ok(Some(Outcome::Ok(()))) | Err(JoinError::Cancelled(_))
        ),
        "{out:?}"
    );
    // A cancelled worker kills the probe at once instead of asking it to exit.
    assert_eq!(link.kills_per_conn(), vec![1]);
    assert_eq!(link.shutdowns_per_conn(), vec![0]);
    assert!(lab.run_until_quiescent_with_report().lab_test_passed());
}

#[test]
fn worker_api_versions_once() {
    let once = fresh_once();
    let link = FakeLink::new([LinkStep::Api(Ok(api_fixture()))]);
    let (_lab, mut rx, _demand, _handle) =
        lab_worker_with(Arc::clone(&link), visible_gpus(), true, once);
    let got = drain(&mut rx);
    let api = got
        .iter()
        .position(|u| matches!(u, SourceUpdate::GpuApiVersions(v) if *v == api_fixture()))
        .expect("GpuApiVersions sent");
    let first_gpu = got
        .iter()
        .position(|u| matches!(u, SourceUpdate::Gpu(_)))
        .expect("a first sample");
    assert!(matches!(&got[0], SourceUpdate::GpuInfo(_)), "{got:?}");
    assert!(api < first_gpu, "{got:?}");
    assert_eq!(link.count(|c| matches!(c, LinkCall::ApiVersions(_))), 1);

    // A later generation in the same process does not run it again.
    let again = FakeLink::new([LinkStep::Api(Ok(api_fixture()))]);
    let (mut lab, mut rx, _demand, _handle) =
        lab_worker_with(Arc::clone(&again), visible_gpus(), true, once);
    advance(&mut lab, PERIOD);
    let got = drain(&mut rx);
    assert!(
        !got.iter()
            .any(|u| matches!(u, SourceUpdate::GpuApiVersions(_))),
        "{got:?}"
    );
    assert_eq!(gpu_samples(&got).len(), 2, "{got:?}");
    assert_eq!(again.count(|c| matches!(c, LinkCall::ApiVersions(_))), 0);
}

#[test]
fn worker_api_versions_error_is_quiet() {
    let link = FakeLink::new([LinkStep::Api(Err(ProbeError::Deadline(
        API_VERSIONS_DEADLINE,
    )))]);
    let (mut lab, mut rx, _demand, mut handle) = lab_worker(Arc::clone(&link), visible_gpus());
    let got = drain(&mut rx);
    assert_eq!(link.count(|c| matches!(c, LinkCall::ApiVersions(_))), 1);
    assert_eq!(
        got.len(),
        3,
        "GpuInfo, Running and the first sample: {got:?}"
    );
    assert!(matches!(&got[0], SourceUpdate::GpuInfo(_)), "{got:?}");
    assert!(is_health(&got[1], SourceState::Running, None), "{got:?}");
    assert!(matches!(&got[2], SourceUpdate::Gpu(_)), "{got:?}");
    advance(&mut lab, PERIOD);
    assert_eq!(gpu_samples(&drain(&mut rx)).len(), 1);
    assert!(outcome(&mut handle).is_none());
}

// Supervised worker ---------------------------------------------------------------------------

/// How one generation ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Ok,
    Err,
    Cancelled,
    Panicked,
}

/// One generation: virtual ms at start, and how it ended once it has.
#[derive(Clone, Copy, Debug)]
struct Generation {
    start_ms: u64,
    end: Option<End>,
}

type Generations = Arc<Mutex<Vec<Generation>>>;

/// The `GpuNvidia` worker under a `SourceController`, as a lab root task.
struct SupervisedRig {
    lab: LabRuntime,
    rx: mpsc::Receiver<EngineMsg>,
    _demand: watch::Sender<Demand>,
    _control: watch::Sender<WorkerControl>,
    handle: TaskHandle<()>,
    generations: Generations,
    updates: Vec<SourceUpdate>,
}

impl SupervisedRig {
    fn new(config: LabConfig, link: Arc<FakeLink>) -> Self {
        let mut lab = LabRuntime::new(config);
        let (io, rx, demand, control) = worker_io(visible_gpus());
        let updates = io.updates.clone();
        let generations = Generations::default();
        let gens = Arc::clone(&generations);
        let once = fresh_once();
        let factory: WorkerFactory = Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
            let io = io.clone();
            let link = Arc::clone(&link);
            let gens = Arc::clone(&gens);
            async move {
                let index = {
                    let mut gens = gens.lock().expect("generations lock");
                    gens.push(Generation {
                        start_ms: cx.now().as_nanos() / 1_000_000,
                        end: None,
                    });
                    gens.len() - 1
                };
                let locate = || Some(PathBuf::from(FAKE_PATH));
                let out = run_with::<Conn>(cx, io, locate, link, once).await;
                let end = match &out {
                    Outcome::Ok(()) => End::Ok,
                    Outcome::Err(_) => End::Err,
                    Outcome::Cancelled(_) => End::Cancelled,
                    Outcome::Panicked(_) => End::Panicked,
                };
                gens.lock().expect("generations lock")[index].end = Some(end);
                out
            }
        });
        let ctl = SourceController::new(SourceId::GpuNvidia, factory, updates);
        let root = root(&mut lab);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("lab task has a Cx");
                ctl.run(cx).await;
            })
            .expect("create task");
        lab.scheduler.lock().schedule(task, 0);
        let mut rig = SupervisedRig {
            lab,
            rx,
            _demand: demand,
            _control: control,
            handle,
            generations,
            updates: Vec::new(),
        };
        rig.lab.run_until_idle();
        rig.drain();
        rig
    }

    fn drain(&mut self) {
        self.updates.extend(drain(&mut self.rx));
    }

    /// Fires every timer up to `ms` of virtual time, one deadline at a time.
    fn run_to(&mut self, ms: u64) {
        let end = Time::from_nanos(ms * 1_000_000);
        loop {
            self.lab.run_until_idle();
            self.drain();
            match self.lab.next_timer_deadline() {
                Some(next) if next <= end => {
                    self.lab.advance_time_to(next);
                    self.lab.step_for_test();
                }
                _ => break,
            }
        }
        self.lab.advance_time_to(end);
        self.lab.step_for_test();
        self.lab.run_until_idle();
        self.drain();
    }

    fn generations(&self) -> Vec<Generation> {
        self.generations.lock().expect("generations lock").clone()
    }

    /// Cancels the controller the way engine shutdown does.
    fn stop(&mut self) {
        self.handle.abort();
        self.lab.run_until_idle();
        self.drain();
    }
}

fn first_sample_fails_twice() -> Arc<FakeLink> {
    FakeLink::new([
        LinkStep::Sample(Err(ProbeError::Deadline(SAMPLE_DEADLINE))),
        LinkStep::Sample(Err(ProbeError::Exited("signal 11".into()))),
    ])
}

/// ADR-004 backoff: 0.5 s, then 1 s. The reducer's "Stale then Failed" is E09's test.
#[test]
fn supervised_worker_backoff_half_then_one_second() {
    let link = first_sample_fails_twice();
    let mut rig = SupervisedRig::new(
        LabConfig::new(0xe13_0b0f).max_steps(10_000_000),
        Arc::clone(&link),
    );
    rig.run_to(5_000);
    let gens = rig.generations();
    let starts: Vec<u64> = gens.iter().map(|g| g.start_ms).collect();
    let first = starts[0];
    let offsets: Vec<u64> = starts.iter().map(|s| s - first).collect();
    assert_eq!(offsets, vec![0, 500, 1_500], "{gens:?}");
    let ends: Vec<Option<End>> = gens.iter().map(|g| g.end).collect();
    assert_eq!(ends, vec![Some(End::Err), Some(End::Err), None]);
    // `--api-versions` ran in the first generation only.
    assert_eq!(
        link.count(|c| matches!(c, LinkCall::ApiVersions(_))),
        1,
        "{:?}",
        link.calls()
    );
    // The third generation samples at 1 Hz from 1.5 s: 1.5, 2.5, 3.5 and 4.5 s.
    let samples = gpu_samples(&rig.updates).len();
    assert_eq!(samples, 4, "{:?}", rig.updates);

    rig.stop();
    assert!(rig.handle.is_finished(), "controller returns after cancel");
    assert_eq!(rig.generations()[2].end, Some(End::Ok));
    assert_eq!(link.kills_per_conn(), vec![0, 0, 1]);
    assert_eq!(link.shutdowns_per_conn(), vec![0, 0, 0]);
    assert!(rig.lab.run_until_quiescent_with_report().lab_test_passed());
}

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]`, which asupersync 0.5.0 does
/// not re-export (see `lab_supervise.rs`): seeds 0..16, two workers, light chaos and a
/// quiescence check per seed.
#[test]
fn explore_worker_restart() {
    for seed in 0..16 {
        let link = first_sample_fails_twice();
        let config = LabConfig::new(seed)
            .worker_count(2)
            .max_steps(10_000_000)
            .with_light_chaos();
        let mut rig = SupervisedRig::new(config, Arc::clone(&link));
        rig.run_to(10_000);
        // A chaos cancel of the controller ends the run early.
        let ended_early = rig.handle.is_finished();
        let gens = rig.generations();
        assert!(!gens.is_empty(), "seed {seed}");
        for (i, g) in gens.iter().enumerate() {
            if g.end == Some(End::Err) {
                assert!(
                    i + 1 < gens.len() || ended_early,
                    "seed {seed}: generation {i} ended with Err and was not restarted: {gens:?}"
                );
            }
        }
        rig.stop();
        let stops = link
            .shutdowns_per_conn()
            .into_iter()
            .zip(link.kills_per_conn());
        for (conn, (shutdowns, kills)) in stops.enumerate() {
            let n = shutdowns + kills;
            assert!(
                n <= 1,
                "seed {seed}: connection {} stopped {n} times",
                conn + 1
            );
        }
        assert!(
            rig.lab.run_until_quiescent_with_report().lab_test_passed(),
            "seed {seed}"
        );
    }
}

// Engine ------------------------------------------------------------------------------------

thread_local! {
    /// The link and `--api-versions` flag `engine_gpu_factory` hands to each generation. The lab
    /// runs every task on the test thread.
    static ENGINE_LINK: RefCell<Option<(Arc<FakeLink>, &'static AtomicBool)>> =
        const { RefCell::new(None) };
}

/// `WorkerSpec::factory` is a fn pointer, so the link comes from `ENGINE_LINK`.
fn engine_gpu_factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let (link, once) = ENGINE_LINK
        .with(|l| l.borrow().clone())
        .expect("spec_with set the link");
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let link = Arc::clone(&link);
        async move {
            let locate = || Some(PathBuf::from(FAKE_PATH));
            run_with::<Conn>(cx, io, locate, link, once).await
        }
    })
}

/// `gpu_nvidia::spec()` with the factory swapped for one that runs the real worker body over
/// `link`.
fn spec_with(link: Arc<FakeLink>) -> WorkerSpec {
    ENGINE_LINK.with(|l| *l.borrow_mut() = Some((link, fresh_once())));
    WorkerSpec {
        factory: engine_gpu_factory,
        ..gpu_nvidia::spec()
    }
}

/// The engine root in a lab, with the `GpuNvidia` spec over `link` as its only worker.
struct EngineRig {
    fake: fake_sources::FakeLab,
    ui: fake_sources::UiSide,
    root: TaskHandle<()>,
}

impl EngineRig {
    /// `link` is made by the closure from the lab's clock. `demand` is set before the engine
    /// starts.
    fn start(
        seed: u64,
        demand: Demand,
        link: impl FnOnce(&Arc<VirtualClock>) -> Arc<FakeLink>,
    ) -> (Self, Arc<FakeLink>) {
        let mut fake =
            fake_sources::FakeLab::with_config(LabConfig::new(seed).max_steps(10_000_000));
        let link = link(&fake.clock);
        let config = Arc::new(EngineConfig::new(
            EngineMode::Full,
            "/var/tmp/claude/ndy.2-state".into(),
        ));
        let (ep, ui) = fake_sources::root_endpoints(config, vec![spec_with(Arc::clone(&link))]);
        ui.demand_tx.send(demand).expect("demand sends");
        let root = fake.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            engine_root(cx, ep).await;
        });
        (EngineRig { fake, ui, root }, link)
    }

    fn set_demand(&self, demand: Demand) {
        self.ui.demand_tx.send(demand).expect("demand sends");
    }

    /// Runs `total` of virtual time in 100 ms steps.
    fn run_for(&mut self, total: Duration) {
        self.fake.run_for(total, Duration::from_millis(100));
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.ui.latest.load_full()
    }

    /// Cancels the root, runs to quiescence and checks the lab's oracles.
    fn finish(mut self) {
        self.root.abort();
        self.fake.advance(Duration::from_millis(10));
        assert!(self.root.is_finished(), "engine_root returned after cancel");
        self.fake.finish();
    }
}

fn gpu_identity(snap: &Snapshot) -> DeviceIdentity {
    let gpus: Vec<&DeviceIdentity> = snap
        .devices
        .entries
        .iter()
        .map(|e| &e.identity)
        .filter(|id| matches!(id, DeviceIdentity::Gpu { .. }))
        .collect();
    assert_eq!(gpus.len(), 1, "{gpus:?}");
    assert!(
        matches!(gpus[0], DeviceIdentity::Gpu { uuid, .. } if *uuid == UUID),
        "{gpus:?}"
    );
    gpus[0].clone()
}

/// The `GPU_UTIL` samples of the one GPU.
fn gpu_util(snap: &Snapshot) -> Vec<Sample> {
    let idx = snap
        .devices
        .find(&gpu_identity(snap))
        .expect("the GPU is in the catalog")
        .idx;
    snap.series
        .get(SeriesId {
            device: idx,
            metric: MetricId::GPU_UTIL,
        })
        .map(|view| view.samples.to_vec())
        .unwrap_or_default()
}

/// E13-A2: the registered spec, the real worker body and the reducer, end to end.
#[test]
fn engine_gpu_end_to_end_lab() {
    let (mut rig, link) =
        EngineRig::start(0xe13a2, visible_gpus(), |clock| FakeLink::timed([], clock));
    // Samples at 0, 1 and 2 s.
    rig.run_for(Duration::from_secs(3) - Duration::from_millis(1));
    let snap = rig.latest();
    assert_eq!(
        snap.health.get(SourceId::GpuNvidia).state,
        SourceState::Running
    );
    let idx = snap
        .devices
        .find(&gpu_identity(&snap))
        .expect("the GPU is in the catalog")
        .idx;
    assert_eq!(gpu_util(&snap).len(), 3, "{:?}", gpu_util(&snap));
    assert!(gpu_util(&snap).iter().all(|s| s.value == 37.0));
    // The reducer replaces the worker's placeholder index with the catalog index.
    assert_eq!(snap.system.gpus.len(), 1);
    let (row_idx, totals) = &snap.system.gpus[0];
    assert_eq!(*row_idx, idx);
    assert_eq!(totals.device, idx);
    assert_eq!(totals.mem_total, Metric::valid(16 << 30));
    assert_eq!(link.samples(), vec![false; 3]);

    rig.set_demand(Demand {
        gpu_process: true,
        ..visible_gpus()
    });
    rig.run_for(Duration::from_secs(1));
    assert_eq!(link.samples().last(), Some(&true), "{:?}", link.calls());

    rig.set_demand(Demand::default());
    rig.run_for(Duration::from_millis(100));
    let samples = link.samples().len();
    let util = gpu_util(&rig.latest()).len();
    rig.run_for(Duration::from_secs(10));
    assert_eq!(link.samples().len(), samples, "no request while idle");
    assert_eq!(
        gpu_util(&rig.latest()).len(),
        util,
        "no Gpu message while idle"
    );
    assert_eq!(link.starts(), 1);
    rig.finish();
}

/// E13-A2 failure table through the engine: a sample deadline ends the generation, the
/// supervisor starts a new one 0.5 s later, and samples resume on the new connection. The
/// restart reaches the reducer as `Failed` then `Running`, so the first sample of the new
/// generation has `GAP_BEFORE` (R-289).
#[test]
fn engine_gpu_failure_table_lab() {
    let (mut rig, link) = EngineRig::start(0xe13a3, visible_gpus(), |clock| {
        FakeLink::timed(
            [
                LinkStep::Sample(Ok(())),
                LinkStep::Sample(Ok(())),
                LinkStep::Sample(Err(ProbeError::Deadline(SAMPLE_DEADLINE))),
            ],
            clock,
        )
    });
    rig.run_for(Duration::from_millis(1_900));
    let snap = rig.latest();
    assert_eq!(
        snap.health.get(SourceId::GpuNvidia).state,
        SourceState::Running
    );
    assert_eq!(gpu_util(&snap).len(), 2);

    // The deadline at 2 s, the restart at 2.5 s and its first sample at once.
    rig.run_for(Duration::from_millis(700));
    let snap = rig.latest();
    assert_eq!(link.starts(), 2, "{:?}", link.calls());
    assert_eq!(
        snap.health.get(SourceId::GpuNvidia).state,
        SourceState::Running
    );
    let util = gpu_util(&snap);
    assert_eq!(util.len(), 3, "{util:?}");
    let gap_ns = util[2].t_ns - util[1].t_ns;
    assert_eq!(gap_ns, 1_500_000_000, "{util:?}");
    assert!(util.iter().all(|s| s.flags.contains(SampleFlags::VALID)));
    let gaps: Vec<bool> = util
        .iter()
        .map(|s| s.flags.contains(SampleFlags::GAP_BEFORE))
        .collect();
    assert_eq!(gaps, [false, false, true], "{util:?}");
    assert_eq!(
        link.calls()
            .iter()
            .filter(|c| matches!(c, LinkCall::Sample { conn: 2, .. }))
            .count(),
        1
    );

    rig.run_for(Duration::from_secs(2));
    assert_eq!(gpu_util(&rig.latest()).len(), 5);
    assert_eq!(
        rig.latest().health.get(SourceId::GpuNvidia).state,
        SourceState::Running
    );
    rig.finish();
}
