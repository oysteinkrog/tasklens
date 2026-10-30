//! The journal stream on the real system (ADR-013, E14-010, U-49, R-167 item 8).
//!
//! Run on the owner machine with
//! `heavy-build cargo test -p tl-engine --test real_journal -- --ignored`. The user-unit tests
//! start the transient user unit `tasklens-journal-test`, which writes `line-0` to `line-999`
//! to the user journal, and stop it again on drop. They only read the journal through
//! `EngineClient::journal`.
//!
//! The unit name is fixed, so an earlier run in the same boot leaves its lines in the journal.
//! Entries older than the fixture's start are ignored, and the tests hold one lock so they
//! never run the unit twice at once.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::journal::{JournalAccess, JournalEntry, JournalEvent, JournalHandle, JournalQuery};
use tl_model::table::{Manager, UnitRef};

const UNIT: &str = "tasklens-journal-test";
const LINES: usize = 1000;
/// Upper bound for every wait in this file.
const WAIT: Duration = Duration::from_secs(30);
const MAX_LOAD_OLDER: usize = 10;

/// The user-unit tests share one unit name, so they run one at a time.
static UNIT_LOCK: Mutex<()> = Mutex::new(());

/// The transient unit. Stopped on drop, also when a test panics.
struct Fixture {
    /// Wall clock before the unit started, in microseconds since the Unix epoch.
    start_us: u64,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn start() -> Self {
        let lock = UNIT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        // A unit left over from an aborted run would make `systemd-run` fail.
        stop_unit();
        let _ = Command::new("systemctl")
            .args(["--user", "reset-failed", UNIT])
            .stderr(Stdio::null())
            .status();
        let start_us = now_us();
        let script = format!(
            "i=0; while [ $i -lt {LINES} ]; do echo line-$i; i=$((i+1)); sleep 0.01; done; sleep 600"
        );
        let status = Command::new("systemd-run")
            .args([
                "--user",
                &format!("--unit={UNIT}"),
                "--property=Type=simple",
                "/bin/sh",
                "-c",
                &script,
            ])
            .status()
            .expect("systemd-run runs");
        assert!(status.success(), "systemd-run failed: {status}");
        Self {
            start_us,
            _lock: lock,
        }
    }

    /// The `line-N` numbers in `entries` written by this run of the unit.
    fn line_numbers(&self, entries: &[JournalEntry]) -> Vec<usize> {
        entries
            .iter()
            .filter(|e| e.realtime_us >= self.start_us)
            .filter_map(|e| e.message.strip_prefix("line-")?.parse().ok())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        stop_unit();
    }
}

fn stop_unit() {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", UNIT])
        .stderr(Stdio::null())
        .status();
}

fn now_us() -> u64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970");
    u64::try_from(since.as_micros()).expect("time fits u64")
}

fn start_engine(name: &str) -> EngineHandle {
    let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-state"));
    let config = EngineConfig::new(EngineMode::LowMemory, state_dir);
    Engine::start(config, EngineHooks::default()).expect("engine starts")
}

fn stop_engine(engine: EngineHandle) {
    let report = engine.shutdown();
    // Not a shutdown test: `drained` is left to the shutdown tests (ADR-004 fixed budgets).
    assert!(report.torn_down, "engine shutdown: {report:?}");
}

fn query(manager: Manager, name: &str, follow: bool) -> JournalQuery {
    JournalQuery {
        unit: UnitRef {
            manager,
            name: Arc::from(name),
        },
        follow,
    }
}

/// Every event that arrives before `deadline`, or until `done` returns true for one of them.
/// Panics on an `Error` event.
fn collect(
    handle: &JournalHandle,
    deadline: Instant,
    mut done: impl FnMut(&JournalEvent) -> bool,
) -> (Vec<JournalEvent>, bool) {
    let events = handle.events();
    let mut got = Vec::new();
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(JournalEvent::Error(e)) => panic!("journal error: {e:?}"),
            Ok(event) => {
                let stop = done(&event);
                got.push(event);
                if stop {
                    return (got, true);
                }
            }
            Err(_) if !events.is_closed() => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => panic!("journal event channel closed"),
        }
    }
    (got, false)
}

/// True when `event` is a batch holding `line-999` from this run.
fn has_last_line(fixture: &Fixture, event: &JournalEvent) -> bool {
    match event {
        JournalEvent::Batch { entries, .. } => fixture.line_numbers(entries).contains(&(LINES - 1)),
        _ => false,
    }
}

/// The `line-N` numbers of every non-older batch, in arrival order.
fn newer_lines(fixture: &Fixture, events: &[JournalEvent]) -> Vec<usize> {
    let mut lines = Vec::new();
    for event in events {
        if let JournalEvent::Batch {
            entries,
            older: false,
            ..
        } = event
        {
            lines.extend(fixture.line_numbers(entries));
        }
    }
    lines
}

/// Checks that `lines` is `first..=last` with every number once and in order. The message
/// names the first missing or repeated line.
fn check_run(lines: &[usize], first: usize, last: usize) {
    assert!(!lines.is_empty(), "no lines arrived");
    let mut expected = first;
    for (i, &n) in lines.iter().enumerate() {
        if n < expected {
            panic!("line-{n} repeated at position {i} (expected line-{expected})");
        }
        if n > expected {
            panic!("line-{expected} missing (line-{n} arrived at position {i})");
        }
        expected += 1;
    }
    assert_eq!(
        expected,
        last + 1,
        "line-{expected} missing at the end (last line-{})",
        expected - 1
    );
}

/// U-49: turning follow off and on resumes with `--after-cursor` without gaps or duplicates.
#[test]
#[ignore = "owner machine: starts a transient user unit and reads the journal"]
fn follow_resume_no_gap_no_duplicate() {
    let fixture = Fixture::start();
    let engine = start_engine("real_journal-follow");
    let handle = engine.journal(query(Manager::User, &format!("{UNIT}.service"), true));

    let (mut events, _) = collect(&handle, Instant::now() + Duration::from_secs(2), |_| false);
    handle.set_follow(false);
    let (paused, _) = collect(&handle, Instant::now() + Duration::from_secs(1), |_| false);
    events.extend(paused);
    handle.set_follow(true);
    let (resumed, found) = collect(&handle, Instant::now() + WAIT, |e| {
        has_last_line(&fixture, e)
    });
    events.extend(resumed);
    assert!(found, "line-{} did not arrive within {WAIT:?}", LINES - 1);

    let lines = newer_lines(&fixture, &events);
    let first = *lines.first().expect("some lines arrived");
    eprintln!(
        "follow: {} lines from line-{first}, {} events",
        lines.len(),
        events.len()
    );
    check_run(&lines, first, LINES - 1);

    drop(handle);
    stop_engine(engine);
}

/// R-167 item 8: Load older pages back with `--reverse --cursor` until the start.
#[test]
#[ignore = "owner machine: starts a transient user unit and reads the journal"]
fn load_older_pages_back() {
    let fixture = Fixture::start();
    let engine = start_engine("real_journal-older");
    let unit = format!("{UNIT}.service");
    let follower = engine.journal(query(Manager::User, &unit, true));
    let (_, found) = collect(&follower, Instant::now() + WAIT, |e| {
        has_last_line(&fixture, e)
    });
    assert!(found, "line-{} did not arrive within {WAIT:?}", LINES - 1);
    drop(follower);

    // A new handle opened now loads line-800 to line-999 first, so the walk back crosses
    // several full pages.
    let handle = engine.journal(query(Manager::User, &unit, false));
    let (events, got) = collect(&handle, Instant::now() + WAIT, |e| {
        matches!(e, JournalEvent::Batch { older: false, .. })
    });
    assert!(got, "no initial batch within {WAIT:?}");
    let newer = newer_lines(&fixture, &events);

    // Each page is older than the one before, so pages are stacked in front.
    let mut older: Vec<usize> = Vec::new();
    let mut calls = 0;
    loop {
        assert!(
            calls < MAX_LOAD_OLDER,
            "no exhausted batch after {MAX_LOAD_OLDER} load_older calls"
        );
        handle.load_older(None);
        calls += 1;
        let (page, got) = collect(&handle, Instant::now() + WAIT, |e| {
            matches!(e, JournalEvent::Batch { older: true, .. })
        });
        assert!(got, "load_older call {calls}: no batch within {WAIT:?}");
        let Some(JournalEvent::Batch {
            entries, exhausted, ..
        }) = page.last()
        else {
            unreachable!("collect stopped on an older batch");
        };
        let mut lines = fixture.line_numbers(entries);
        lines.extend(older);
        older = lines;
        // An earlier run of the unit in this boot means the journal goes on past this run's
        // start; a page that reaches before it ends the walk as well.
        let before_start = entries
            .first()
            .is_some_and(|e| e.realtime_us < fixture.start_us);
        if *exhausted || before_start {
            eprintln!("load older: {calls} calls, exhausted {exhausted}");
            break;
        }
    }

    older.extend(newer);
    check_run(&older, 0, LINES - 1);

    drop(handle);
    stop_engine(engine);
}

/// The groups of the current user, from `id -Gn`.
fn groups() -> Vec<String> {
    let out = Command::new("id").arg("-Gn").output().expect("id runs");
    assert!(out.status.success(), "id -Gn failed");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// SYS-015: the first event of a system unit stream is `Access`, matching the user's groups.
#[test]
#[ignore = "owner machine: reads the system journal"]
fn system_unit_access_event() {
    let reader = groups()
        .iter()
        .any(|g| ["wheel", "adm", "systemd-journal"].contains(&g.as_str()));
    let expected = if reader {
        JournalAccess::All
    } else {
        JournalAccess::OwnOnly
    };

    let engine = start_engine("real_journal-access");
    let handle = engine.journal(query(Manager::System, "systemd-journald.service", false));
    let (events, got) = collect(&handle, Instant::now() + WAIT, |_| true);
    assert!(got, "no event within {WAIT:?}");
    match &events[0] {
        JournalEvent::Access(access) => assert_eq!(*access, expected),
        other => panic!("first event is not Access: {other:?}"),
    }

    drop(handle);
    stop_engine(engine);
}
