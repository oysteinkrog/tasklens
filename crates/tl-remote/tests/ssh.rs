//! The ssh transport (ADR-018 Remote transport, Implementation contracts "Endpoints"): target
//! validation, the exact ssh arguments, the stderr ring's limits, and each named failure from a
//! fake ssh script run through `RemoteClient`.
//!
//! The fake scripts ignore their arguments. Each test that writes and runs one holds `SERIAL`,
//! so no other test forks while a script is open for writing (which makes its exec fail with
//! "Text file busy").

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tl_model::remote_proto::{BootPrefix, HelloTail, RefuseReason, Refused, Want, encode_refused};
use tl_remote::client::{ClientConfig, Connector, LinkStatus, RemoteClient};
use tl_remote::ssh::{
    STDERR_BYTES, STDERR_LINES, SshConnector, StderrRing, TargetError, classify, ssh_command,
    ssh_program, validate_target,
};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// At least the 2 s floor for wall-clock limits in tests (tasklens-cr0.32).
const TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn target_validation_table() {
    let cases: [(&str, Result<(), TargetError>); 9] = [
        ("", Err(TargetError::Empty)),
        ("-oProxyCommand=x", Err(TargetError::LeadingDash)),
        ("a b", Err(TargetError::BadChar)),
        ("a\tb", Err(TargetError::BadChar)),
        ("a\nb", Err(TargetError::BadChar)),
        ("a\u{7f}", Err(TargetError::BadChar)),
        ("home", Ok(())),
        ("me@home.lan", Ok(())),
        ("[::1]", Ok(())),
    ];
    for (target, want) in cases {
        assert_eq!(validate_target(target), want, "target {target:?}");
    }
}

#[test]
fn ssh_args_exact() {
    let cmd = ssh_command(OsStr::new("ssh"), "me@home");
    assert_eq!(cmd.get_program(), "ssh");
    let args: Vec<&OsStr> = cmd.get_args().collect();
    let want = [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        "me@home",
        "tasklens",
        "bridge",
    ];
    assert_eq!(args, want.map(OsStr::new));
}

#[test]
fn ssh_program_env_override() {
    // Test builds read the variable; an empty value means ssh.
    assert_eq!(ssh_program(None), "ssh");
    assert_eq!(ssh_program(Some(OsString::new())), "ssh");
    assert_eq!(ssh_program(Some("/x/fake".into())), "/x/fake");
}

#[test]
fn stderr_ring_limits() {
    let mut ring = StderrRing::new();
    for i in 0..100 {
        ring.push_line(&format!("line {i}"));
    }
    let lines: Vec<&str> = ring.lines().collect();
    assert_eq!(lines.len(), STDERR_LINES);
    assert_eq!(lines[0], "line 36");
    assert_eq!(ring.last_line(), Some("line 99"));

    let mut ring = StderrRing::new();
    let kib = "x".repeat(1023);
    let text = format!("{kib}\n").repeat(20);
    ring.feed(text.as_bytes());
    ring.finish();
    assert!(ring.bytes() <= STDERR_BYTES, "kept {} bytes", ring.bytes());
    assert_eq!(ring.lines().count(), 16);

    // One longer line is cut to the limit; the line end and a split read do not matter.
    let mut ring = StderrRing::new();
    let long = "y".repeat(20 * 1024);
    ring.feed(&long.as_bytes()[..5000]);
    ring.feed(&long.as_bytes()[5000..]);
    ring.feed(b"\r\n");
    let lines: Vec<&str> = ring.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), STDERR_BYTES);
    // The next line goes over the total, so the long one is dropped.
    ring.feed(b"last");
    ring.finish();
    assert_eq!(ring.lines().collect::<Vec<_>>(), ["last"]);
    assert_eq!(ring.bytes(), 4);
}

#[test]
fn classify_order() {
    let mut ring = StderrRing::new();
    ring.push_line("me@home: Permission denied (publickey).");
    ring.push_line("Host key verification failed.");
    // The deadline wins over everything.
    assert_eq!(
        classify(Some(255), &ring, Some(Duration::from_secs(20)), "home").to_string(),
        "home did not answer within 20 s"
    );
    assert_eq!(
        classify(Some(255), &ring, None, "home").to_string(),
        "ssh needs a key for home"
    );
    // "Permission denied" counts only with exit 255.
    assert_eq!(
        classify(Some(1), &ring, None, "home").to_string(),
        "ssh does not trust the host key of home"
    );
    let empty = StderrRing::new();
    assert_eq!(
        classify(Some(127), &empty, None, "home").to_string(),
        "tasklens is not installed on home"
    );
    assert_eq!(
        classify(None, &empty, None, "home").to_string(),
        "ssh to home failed (no exit code)"
    );
}

/// A fake ssh in its own directory, which writes its pid to `pid` first.
struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ssh");
        let pid = dir.path().join("pid");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho $$ > '{}'\n{body}\n", pid.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn program(&self) -> OsString {
        self.dir.path().join("ssh").into_os_string()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn pid(&self) -> u32 {
        let text = std::fs::read_to_string(self.path("pid")).unwrap();
        text.trim().parse().unwrap()
    }
}

fn hello() -> HelloTail {
    HelloTail {
        pid: 1,
        app_id: Arc::from("test"),
        bridge: false,
        want: Want { snapshots: true },
    }
}

/// Runs a client on `connector` and waits for its first `Failed` reason.
fn first_failure(connector: impl Connector) -> String {
    let client = RemoteClient::start(
        connector,
        ClientConfig {
            hello: hello(),
            backoff: vec![Duration::from_secs(60)],
        },
    )
    .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let LinkStatus::Failed(reason) = client.status() {
            return reason.to_string();
        }
        assert!(Instant::now() < deadline, "no failure within {TIMEOUT:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// True when no process `pid` exists, zombies included.
fn gone(pid: u32) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists()
}

/// Runs the fake ssh `body` as host "home" and returns the reason the client shows. The child
/// must be reaped by then.
fn fake_failure(body: &str, deadline: Duration) -> String {
    let _serial = serial();
    let fake = Fake::new(body);
    let reason =
        first_failure(SshConnector::new("home", "me@home", fake.program()).with_deadline(deadline));
    let pid = fake.pid();
    assert!(gone(pid), "ssh child {pid} still exists");
    reason
}

#[test]
fn fake_ssh_key() {
    let reason = fake_failure(
        "echo 'me@home: Permission denied (publickey).' >&2\nexit 255",
        TIMEOUT,
    );
    assert_eq!(reason, "ssh needs a key for home");
}

#[test]
fn fake_ssh_host_key() {
    let reason = fake_failure(
        "echo 'Host key verification failed.' >&2\nexit 255",
        TIMEOUT,
    );
    assert_eq!(reason, "ssh does not trust the host key of home");
}

#[test]
fn fake_ssh_not_installed() {
    let reason = fake_failure(
        "echo 'bash: line 1: tasklens: command not found' >&2\nexit 127",
        TIMEOUT,
    );
    assert_eq!(reason, "tasklens is not installed on home");
}

#[test]
fn fake_ssh_other_exit() {
    let reason = fake_failure(
        "echo 'ssh: connect to host home port 22: Connection refused' >&2\nexit 255",
        TIMEOUT,
    );
    assert_eq!(
        reason,
        "ssh to home failed (exit 255): ssh: connect to host home port 22: Connection refused"
    );
}

#[test]
fn fake_ssh_timeout() {
    let begin = Instant::now();
    let reason = fake_failure("exec sleep 30", Duration::from_millis(300));
    assert_eq!(reason, "home did not answer within 300 ms");
    assert!(begin.elapsed() < Duration::from_secs(5));
}

#[test]
fn fake_ssh_version_refused() {
    let _serial = serial();
    let mut frame = Vec::new();
    encode_refused(
        &Refused {
            prefix: BootPrefix::ours(),
            reason: RefuseReason::Proto,
            text: Arc::from("the engine speaks protocol 99"),
        },
        &mut frame,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let refused = dir.path().join("refused.bin");
    std::fs::write(&refused, &frame).unwrap();
    // The server's answer, then a child that stays until the client ends it.
    let fake = Fake::new(&format!("cat '{}'\nexec sleep 30", refused.display()));
    let reason = first_failure(SshConnector::new("home", "me@home", fake.program()));
    assert_eq!(reason, "the engine speaks protocol 99");
    let pid = fake.pid();
    assert!(gone(pid), "ssh child {pid} still exists");
}

#[test]
fn ssh_missing() {
    // The failed spawn still forks.
    let _serial = serial();
    let reason = first_failure(SshConnector::new(
        "home",
        "me@home",
        "/nonexistent/tasklens-test/ssh".into(),
    ));
    assert_eq!(reason, "ssh is not installed");
}

#[test]
fn bad_target_never_runs_ssh() {
    let reason = first_failure(SshConnector::new(
        "home",
        "-oProxyCommand=x",
        "/nonexistent/tasklens-test/ssh".into(),
    ));
    assert_eq!(reason, "home: the ssh target starts with -");
}

/// Threads in this process that drain an ssh stderr.
fn stderr_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .filter(|comm| comm.trim() == "tl-ssh-stderr")
        .count()
}

/// Waits until no stderr thread is left. Call with `SERIAL` held, so no other test starts one.
fn assert_stderr_threads_end() {
    let deadline = Instant::now() + TIMEOUT;
    while stderr_threads() > 0 {
        assert!(
            Instant::now() < deadline,
            "a stderr thread is still running"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn handshake_write_ends_at_deadline() {
    // ssh never reads its stdin, so the pipe fills and a write would block for good.
    let _serial = serial();
    let fake = Fake::new("exec sleep 30");
    let conn = SshConnector::new("home", "me@home", fake.program())
        .with_deadline(Duration::from_millis(300))
        .connect()
        .unwrap();
    let mut writer = conn.writer;
    let begin = Instant::now();
    let err = writer.write_all(&vec![0u8; 1 << 20]).unwrap_err();
    assert_eq!(err.to_string(), "home did not answer within 300 ms");
    assert!(begin.elapsed() < TIMEOUT);
    let pid = fake.pid();
    assert!(gone(pid), "ssh child {pid} still exists");
    drop(writer);
    drop(conn.reader);
    assert_stderr_threads_end();
}

#[test]
fn stderr_held_open_by_descendant() {
    // ssh exits, but a process it started keeps stderr open for 30 s. The attempt still ends
    // with the named failure, and its stderr thread ends too.
    let _serial = serial();
    let fake = Fake::new(
        "sleep 30 >/dev/null </dev/null &\necho $! > \"$(dirname \"$0\")/bg\"\necho boom >&2\nexit 3",
    );
    let begin = Instant::now();
    let reason = first_failure(SshConnector::new("home", "me@home", fake.program()));
    assert_eq!(reason, "ssh to home failed (exit 3): boom");
    assert!(begin.elapsed() < TIMEOUT);
    assert_stderr_threads_end();
    let bg: i32 = std::fs::read_to_string(fake.path("bg"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_raw(bg).unwrap(),
        rustix::process::Signal::KILL,
    );
}
