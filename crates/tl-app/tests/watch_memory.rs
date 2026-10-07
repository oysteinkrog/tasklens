//! Tests for the low-memory watcher logic and meminfo reader (E32-001), the
//! `tasklens-lowmem.service` unit (E32-002) and the watcher loop (E32-A1).

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/watch_memory.rs"]
mod watch_memory;

#[path = "../src/atomic.rs"]
mod atomic;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/lowmem_unit.rs"]
mod lowmem_unit;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/paths.rs"]
mod paths;

use proptest::prelude::*;
use watch_memory::{
    MEMAVAIL_CHECK, MemInfo, MemInfoReader, PSI_TRIGGER, WaitResult, WatchDecision, WatchEvent,
    WatchIo, WatchState, mem_available_critical, parse_meminfo, run_loop,
};

/// The start of a real `/proc/meminfo` (Linux 7.2, 64 GiB machine).
const REAL_MEMINFO: &str = "\
MemTotal:       65753652 kB
MemFree:        16958744 kB
MemAvailable:   24129960 kB
Buffers:           77460 kB
Cached:          7754720 kB
SwapCached:        11392 kB
Active:         21430212 kB
Inactive:       20215680 kB
SwapTotal:      67108860 kB
SwapFree:       66982396 kB
Dirty:              1588 kB
HugePages_Total:       0
HugePages_Free:        0
Hugepagesize:       2048 kB
DirectMap4k:     1093180 kB
DirectMap2M:    37529600 kB
DirectMap1G:    29360128 kB
";

#[test]
fn parse_meminfo_real_layout() {
    assert_eq!(
        parse_meminfo(REAL_MEMINFO.as_bytes()),
        Some(MemInfo {
            total_kib: 65_753_652,
            available_kib: 24_129_960
        })
    );
    let text = "MemTotal: 16000000 kB\nMemFree: 1 kB\nMemAvailable: 400000 kB\n";
    assert_eq!(
        parse_meminfo(text.as_bytes()),
        Some(MemInfo {
            total_kib: 16_000_000,
            available_kib: 400_000
        })
    );
}

#[test]
fn parse_meminfo_missing_field() {
    let no_avail = "MemTotal:       65753652 kB\nMemFree:        16958744 kB\n";
    assert_eq!(parse_meminfo(no_avail.as_bytes()), None);
    let zero_total = "MemTotal: 0 kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(zero_total.as_bytes()), None);
    let not_a_number = "MemTotal: lots kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(not_a_number.as_bytes()), None);
    let overflow = "MemTotal: 99999999999999999999999 kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(overflow.as_bytes()), None);
    assert_eq!(parse_meminfo(b""), None);
}

#[test]
fn threshold_boundary_3_percent() {
    assert!(mem_available_critical(&MemInfo {
        total_kib: 1000,
        available_kib: 29
    }));
    assert!(!mem_available_critical(&MemInfo {
        total_kib: 1000,
        available_kib: 30
    }));
    // No overflow at the extremes.
    assert!(!mem_available_critical(&MemInfo {
        total_kib: u64::MAX,
        available_kib: u64::MAX
    }));
}

#[test]
fn state_one_rescue_at_a_time() {
    let mut s = WatchState::default();
    assert_eq!(
        s.on_event(WatchEvent::PsiTrigger),
        WatchDecision::StartRescue
    );

    s.rescue_started();
    assert_eq!(s.on_event(WatchEvent::PsiTrigger), WatchDecision::Nothing);
    let critical = MemInfo {
        total_kib: 1000,
        available_kib: 1,
    };
    assert_eq!(
        s.on_event(WatchEvent::MemCheck(critical)),
        WatchDecision::Nothing
    );

    assert_eq!(s.on_event(WatchEvent::RescueExited), WatchDecision::Nothing);
    assert_eq!(
        s.on_event(WatchEvent::PsiTrigger),
        WatchDecision::StartRescue
    );

    let mut fresh = WatchState::default();
    let fine = MemInfo {
        total_kib: 1000,
        available_kib: 500,
    };
    assert_eq!(
        fresh.on_event(WatchEvent::MemCheck(fine)),
        WatchDecision::Nothing
    );
    assert_eq!(
        fresh.on_event(WatchEvent::MemCheck(critical)),
        WatchDecision::StartRescue
    );
}

#[test]
fn psi_trigger_string_exact() {
    assert_eq!(PSI_TRIGGER, "full 200000 2000000");
}

/// Reads the live `/proc/meminfo` (read only) twice through one reader.
#[test]
fn reader_reads_proc_meminfo_twice() {
    let mut reader = MemInfoReader::open().expect("open /proc/meminfo");
    for _ in 0..2 {
        let m = reader.read().expect("parse /proc/meminfo");
        assert!(m.total_kib > 0);
        assert!(m.available_kib <= m.total_kib);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn prop_parse_meminfo_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        let _ = parse_meminfo(&bytes);
    }

    /// Malformed text around the two real lines: still no panic, and when it parses,
    /// the critical check does not panic either.
    #[test]
    fn prop_parse_meminfo_noisy_lines_never_panic(
        prefix in "[ -~\n]{0,200}",
        total in "[0-9 kB]{0,30}",
        avail in "[0-9 kB]{0,30}",
    ) {
        let text = format!("{prefix}\nMemTotal:{total}\nMemAvailable:{avail}\n");
        if let Some(m) = parse_meminfo(text.as_bytes()) {
            let _ = mem_available_critical(&m);
        }
    }
}

// E32-002: the lowmem unit. Every automatic test works in a scratch folder and never talks to
// the user's systemd manager.

/// The environment `unit_path_from` sees: `HOME=/home/u` plus `xdg` as `XDG_CONFIG_HOME`.
fn env_with(xdg: Option<&str>) -> impl Fn(&str) -> Option<std::ffi::OsString> {
    move |name| match name {
        "HOME" => Some("/home/u".into()),
        "XDG_CONFIG_HOME" => xdg.map(Into::into),
        _ => None,
    }
}

#[test]
fn render_unit_replaces_placeholders() {
    let text = lowmem_unit::render_unit(std::path::Path::new("/usr/bin/tasklens"), -900);
    assert!(
        text.lines()
            .any(|l| l == "ExecStart=/usr/bin/tasklens --watch-memory")
    );
    assert!(text.lines().any(|l| l == "OOMScoreAdjust=-900"));
    assert!(!text.contains('@'), "placeholder left in:\n{text}");
    let fallback = lowmem_unit::render_unit(std::path::Path::new("/usr/bin/tasklens"), 0);
    assert!(fallback.lines().any(|l| l == "OOMScoreAdjust=0"));
}

#[test]
fn render_unit_quotes_spaces() {
    let text = lowmem_unit::render_unit(std::path::Path::new("/home/u/my builds/tasklens"), -900);
    assert!(
        text.lines()
            .any(|l| l == r#"ExecStart="/home/u/my builds/tasklens" --watch-memory"#),
        "{text}"
    );
    // systemd expands % and $ in ExecStart, so both are doubled.
    let odd = lowmem_unit::render_unit(std::path::Path::new("/opt/a%b$c/tasklens"), -900);
    assert!(
        odd.lines()
            .any(|l| l == "ExecStart=/opt/a%%b$$c/tasklens --watch-memory")
    );
    let quote = lowmem_unit::render_unit(std::path::Path::new("/opt/a \"b/tasklens"), -900);
    assert!(
        quote
            .lines()
            .any(|l| l == r#"ExecStart="/opt/a \"b/tasklens" --watch-memory"#),
        "{quote}"
    );
}

#[test]
fn unit_path_xdg_fallback() {
    let expected = std::path::PathBuf::from("/home/u/.config/systemd/user/tasklens-lowmem.service");
    for xdg in [None, Some(""), Some("relative/path")] {
        assert_eq!(
            lowmem_unit::unit_path_from(env_with(xdg)).unwrap(),
            expected,
            "{xdg:?}"
        );
    }
    assert_eq!(
        lowmem_unit::unit_path_from(env_with(Some("/cfg"))).unwrap(),
        std::path::PathBuf::from("/cfg/systemd/user/tasklens-lowmem.service")
    );
    assert!(lowmem_unit::unit_path_from(|_| None).is_err());
}

#[test]
fn unit_file_write_and_remove_in_scratch_dir() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch
        .path()
        .join("systemd/user")
        .join(lowmem_unit::UNIT_NAME);
    let exec = std::path::Path::new("/usr/bin/tasklens");
    lowmem_unit::write_unit_file(&path, exec, -900).unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o644);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&scratch.path().join("systemd")), 0o700);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        lowmem_unit::render_unit(exec, -900)
    );
    // The fallback rewrite replaces the file in place.
    lowmem_unit::write_unit_file(&path, exec, 0).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("OOMScoreAdjust=0\n")
    );
    lowmem_unit::remove_unit_file(&path).unwrap();
    assert!(!path.exists());
    lowmem_unit::remove_unit_file(&path).unwrap();
}

/// Owner machine only: installs, enables and starts the real unit in the user's own systemd
/// session, then removes it. Run by hand with `--ignored`; it prints the OOMScoreAdjust value
/// the user manager accepted (U-24).
#[test]
#[ignore = "changes the user's real systemd session"]
#[allow(clippy::print_stdout, reason = "prints the U-24 result for the owner")]
fn real_install_enable_remove() {
    let installed = lowmem_unit::install_and_enable().expect("install_and_enable");
    println!(
        "U-24: accepted OOMScoreAdjust={}",
        installed.oom_score_adjust
    );
    let conn = zbus::blocking::Connection::session().unwrap();
    let reply = conn
        .call_method(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            Some("org.freedesktop.systemd1.Manager"),
            "GetUnitFileState",
            &(lowmem_unit::UNIT_NAME,),
        )
        .unwrap();
    let state: String = reply.body().deserialize().unwrap();
    assert_eq!(state, "enabled");

    lowmem_unit::disable_and_remove().expect("first disable_and_remove");
    lowmem_unit::disable_and_remove().expect("second disable_and_remove");
    assert!(!lowmem_unit::unit_path().unwrap().exists());
}

// E32-A1: the watcher loop. The loop tests use a fake `WatchIo` with a fake clock; nothing here
// spawns a process or touches /proc/pressure.

/// A scripted `WatchIo`. `wait` returns `fallback` on every call and moves the fake clock by the
/// timeout it was given.
struct FakeIo {
    clock: std::time::Instant,
    fallback: WaitResult,
    /// None makes `read_meminfo` fail.
    meminfo: Option<MemInfo>,
    /// How many `rescue_alive` calls return true after a spawn (None = the child never exits).
    child_lives: Option<u32>,
    child_left: Option<u32>,
    child_running: bool,
    /// The first this many spawns fail.
    spawn_failures: u32,
    display: bool,
    timeouts: Vec<std::time::Duration>,
    meminfo_reads: u32,
    spawn_attempts: u32,
}

impl FakeIo {
    fn new(fallback: WaitResult) -> Self {
        Self {
            clock: std::time::Instant::now(),
            fallback,
            meminfo: Some(MemInfo {
                total_kib: 1_000_000,
                available_kib: 500_000,
            }),
            child_lives: None,
            child_left: None,
            child_running: false,
            spawn_failures: 0,
            display: true,
            timeouts: Vec::new(),
            meminfo_reads: 0,
            spawn_attempts: 0,
        }
    }
}

impl WatchIo for FakeIo {
    fn wait(&mut self, timeout: std::time::Duration) -> WaitResult {
        self.timeouts.push(timeout);
        self.clock += match self.fallback {
            WaitResult::Timeout => timeout,
            _ => std::time::Duration::from_millis(100),
        };
        self.fallback
    }

    fn read_meminfo(&mut self) -> std::io::Result<MemInfo> {
        self.meminfo_reads += 1;
        self.meminfo
            .ok_or_else(|| std::io::Error::other("fake read error"))
    }

    fn spawn_rescue(&mut self) -> std::io::Result<()> {
        self.spawn_attempts += 1;
        if self.spawn_attempts <= self.spawn_failures {
            return Err(std::io::Error::other("fake spawn error"));
        }
        self.child_running = true;
        self.child_left = self.child_lives;
        Ok(())
    }

    fn rescue_alive(&mut self) -> bool {
        if !self.child_running {
            return false;
        }
        match &mut self.child_left {
            None => true,
            Some(0) => {
                self.child_running = false;
                false
            }
            Some(n) => {
                *n -= 1;
                true
            }
        }
    }

    fn now(&mut self) -> std::time::Instant {
        self.clock
    }

    fn has_display(&mut self) -> bool {
        self.display
    }
}

/// Collects formatted log lines for one test.
#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs `f` with a subscriber that writes to a buffer, and returns the log lines.
fn with_logs(f: impl FnOnce()) -> Vec<String> {
    let buf = LogBuf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn loop_starts_one_rescue() {
    // PSI fires on every turn and the child stays alive: one rescue only.
    let mut io = FakeIo::new(WaitResult::PsiFired);
    assert_eq!(run_loop(&mut io, Some(10)), 1);
    assert_eq!(io.spawn_attempts, 1);
    assert_eq!(io.timeouts.len(), 10);
    assert!(io.timeouts.iter().all(|&t| t <= MEMAVAIL_CHECK));
}

#[test]
fn loop_restarts_after_child_exit() {
    let mut io = FakeIo::new(WaitResult::PsiFired);
    io.child_lives = Some(1);
    // Turn 1 spawns, turn 2 sees the child alive, turn 3 sees it gone and spawns again.
    assert_eq!(run_loop(&mut io, Some(3)), 2);
    assert_eq!(io.spawn_attempts, 2);
}

#[test]
fn loop_memavailable_path() {
    let mut io = FakeIo::new(WaitResult::Timeout);
    io.meminfo = Some(MemInfo {
        total_kib: 1_000_000,
        available_kib: 20_000,
    });
    // One turn waits one full MEMAVAIL_CHECK, then checks.
    assert_eq!(run_loop(&mut io, Some(1)), 1);
    assert_eq!(io.timeouts, [MEMAVAIL_CHECK]);
    assert_eq!(io.meminfo_reads, 1);
}

#[test]
fn loop_survives_meminfo_error() {
    let mut io = FakeIo::new(WaitResult::Timeout);
    io.meminfo = None;
    let logs = with_logs(|| assert_eq!(run_loop(&mut io, Some(5)), 0));
    assert_eq!(io.meminfo_reads, 5);
    assert_eq!(io.spawn_attempts, 0);
    let warns: Vec<_> = logs
        .iter()
        .filter(|l| l.contains("WARN") && l.contains("/proc/meminfo"))
        .collect();
    assert_eq!(warns.len(), 5, "{logs:?}");
}

#[test]
fn loop_retries_after_spawn_failure() {
    let mut io = FakeIo::new(WaitResult::PsiFired);
    io.spawn_failures = 1;
    let logs = with_logs(|| assert_eq!(run_loop(&mut io, Some(3)), 1));
    // The failed first spawn left the state at "not running", so turn 2 tried again.
    assert_eq!(io.spawn_attempts, 2);
    assert!(logs.iter().any(|l| l.contains("WARN")), "{logs:?}");
}

#[test]
fn no_spawn_without_display() {
    let mut io = FakeIo::new(WaitResult::PsiFired);
    io.display = false;
    let logs = with_logs(|| assert_eq!(run_loop(&mut io, Some(3)), 0));
    assert_eq!(io.spawn_attempts, 0);
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("WAYLAND_DISPLAY")),
        "{logs:?}"
    );
}

/// Starts the real binary with `--watch-memory` (the arm in main.rs, E32 m1f.5).
#[test]
fn real_rss_under_4mib() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tasklens"))
        .arg("--watch-memory")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let pid = child.id();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
    let threads = std::fs::read_dir(format!("/proc/{pid}/task")).map(Iterator::count);
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let _ = child.wait();
    let status = status.unwrap();
    let rss_kib: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse().ok())
        .expect("VmRSS in /proc/<pid>/status");
    assert!(rss_kib < 4096, "VmRSS {rss_kib} kB");
    // One thread: no GPUI, no engine, no log writer thread.
    assert_eq!(threads.unwrap(), 1);
}

/// With a fake owner of the bus name, `--watch-memory` does not forward to it and keeps running.
/// A live tasklens that already owns the name counts as the owner.
#[test]
#[ignore = "needs a session bus; starts the real binary"]
fn watch_memory_no_single_instance() {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    let conn = zbus::blocking::Connection::session().expect("session bus");
    let reply = conn
        .request_name_with_flags(
            "io.github.oysteinkrog.Tasklens",
            RequestNameFlags::DoNotQueue.into(),
        )
        .expect("RequestName");
    assert!(
        matches!(
            reply,
            RequestNameReply::PrimaryOwner
                | RequestNameReply::Exists
                | RequestNameReply::AlreadyOwner
        ),
        "{reply:?}"
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tasklens"))
        .arg("--watch-memory")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let exited = child.try_wait();
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(exited.unwrap(), None, "the watcher exited");
}

/// U-51 (R-151): an unprivileged PSI trigger is accepted on the owner machine (kernel 7.2).
#[test]
#[ignore = "owner machine only, run as a normal user"]
fn psi_trigger_accepted_unprivileged() {
    let fd = watch_memory::open_psi_trigger().expect("PSI trigger write");
    drop(fd);
}
