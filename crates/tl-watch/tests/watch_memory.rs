//! Tests for the low-memory watcher logic and meminfo reader (E32-001), the watcher loop
//! (E32-A1) and the `tasklens-watch` binary (tasklens-m1f.19). The lowmem unit tests are in
//! `crates/tl-app/tests/lowmem_unit.rs`.

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/watch_memory.rs"]
mod watch_memory;

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
    logs: Vec<String>,
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
            logs: Vec::new(),
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

    fn warn(&mut self, args: std::fmt::Arguments<'_>) {
        self.logs.push(args.to_string());
    }
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
    assert_eq!(run_loop(&mut io, Some(5)), 0);
    assert_eq!(io.meminfo_reads, 5);
    assert_eq!(io.spawn_attempts, 0);
    let warns = io.logs.iter().filter(|l| l.contains("/proc/meminfo"));
    assert_eq!(warns.count(), 5, "{:?}", io.logs);
}

#[test]
fn loop_retries_after_spawn_failure() {
    let mut io = FakeIo::new(WaitResult::PsiFired);
    io.spawn_failures = 1;
    assert_eq!(run_loop(&mut io, Some(3)), 1);
    // The failed first spawn left the state at "not running", so turn 2 tried again.
    assert_eq!(io.spawn_attempts, 2);
    assert!(
        io.logs.iter().any(|l| l.contains("--low-memory failed")),
        "{:?}",
        io.logs
    );
}

#[test]
fn no_spawn_without_display() {
    let mut io = FakeIo::new(WaitResult::PsiFired);
    io.display = false;
    assert_eq!(run_loop(&mut io, Some(3)), 0);
    assert_eq!(io.spawn_attempts, 0);
    assert!(
        io.logs.iter().any(|l| l.contains("WAYLAND_DISPLAY")),
        "{:?}",
        io.logs
    );
}

/// Reads `name:` from `/proc/<pid>/status` in kB.
fn status_kib(status: &str, name: &str) -> u64 {
    status
        .lines()
        .find_map(|l| l.strip_prefix(name))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{name} in /proc/<pid>/status"))
}

/// Starts `tasklens-watch` with its own `XDG_RUNTIME_DIR`, so a real watcher on this machine does
/// not hold the lock. stderr is captured.
fn spawn_watcher(runtime_dir: &std::path::Path) -> std::process::Child {
    std::process::Command::new(env!("CARGO_BIN_EXE_tasklens-watch"))
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start tasklens-watch")
}

/// A scratch `XDG_RUNTIME_DIR` under the target folder, one per test.
fn scratch_runtime_dir(test: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&dir).expect("create the scratch runtime dir");
    dir
}

/// Starts the real `tasklens-watch` binary. After 2 s it has one thread, VmRSS under 4 MiB, and
/// its pages are locked (mlockall succeeded under this shell's RLIMIT_MEMLOCK, 8 MiB by default).
/// Run it on the release build: `cargo test --release -p tl-watch -- --ignored`.
#[test]
#[ignore = "starts the real binary; the 4 MiB target holds for the release build"]
fn real_rss_under_4mib() {
    let mut child = spawn_watcher(&scratch_runtime_dir("real_rss_under_4mib"));
    std::thread::sleep(std::time::Duration::from_secs(2));
    let pid = child.id();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
    let threads = std::fs::read_dir(format!("/proc/{pid}/task")).map(Iterator::count);
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = status.unwrap();
    let rss_kib = status_kib(&status, "VmRSS:");
    let locked_kib = status_kib(&status, "VmLck:");
    assert!(rss_kib < 4096, "VmRSS {rss_kib} kB");
    assert!(!stderr.contains("mlockall failed"), "{stderr}");
    assert!(locked_kib > 0, "VmLck {locked_kib} kB");
    // One thread: no GPUI, no engine, no log writer thread.
    assert_eq!(threads.unwrap(), 1);
}

/// A second watcher with the same `XDG_RUNTIME_DIR` sees the lock and exits with status 0.
#[test]
#[ignore = "starts the real binary"]
fn second_watcher_exits() {
    let dir = scratch_runtime_dir("second_watcher_exits");
    let mut first = spawn_watcher(&dir);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let second = spawn_watcher(&dir).wait_with_output();
    let first_exited = first.try_wait();
    // Kill only the child this test started, before any assert can fail.
    let _ = first.kill();
    let _ = first.wait();
    assert_eq!(first_exited.unwrap(), None, "the first watcher exited");
    let second = second.unwrap();
    assert!(second.status.success(), "{:?}", second.status);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("another tasklens-watch is running"),
        "{stderr}"
    );
}

/// U-51 (R-151): an unprivileged PSI trigger is accepted on the owner machine (kernel 7.2).
#[test]
#[ignore = "owner machine only, run as a normal user"]
fn psi_trigger_accepted_unprivileged() {
    let fd = watch_memory::open_psi_trigger().expect("PSI trigger write");
    drop(fd);
}
