//! Real-system tests of the built helper as root (E29-012, ADR-013 real-system layer). They
//! prove the root paths the unprivileged tests cannot reach: a signal to another user's process,
//! a stale key of another user, files of another user's process, freeze, thaw and kill of a
//! system scope, `SOCK_DESTROY` on a loopback connection and the system drop-in write.
//!
//! Every test is `#[ignore]`. Without root each one prints "skipped: needs root" and passes.
//! The leader runs the root path on the owner machine. Build the binary as the normal user, then
//! run it with sudo, so `target/` gets no root-owned files:
//!
//! ```text
//! heavy-build cargo ltest -p tl-helper --test real_helper_root --no-run
//! sudo <printed test binary> --ignored --test-threads=1
//! ```
//!
//! Each test starts `tl-helper --session` directly as root on a socketpair (no pkexec), as the
//! engine does (ADR-007), and sends real frames. Each test cleans up in a guard's `Drop`: no
//! child, scope, unit file or drop-in is left behind, even when an assertion fails. The only
//! files touched under `/etc/systemd/system` are `tasklens-test.service` and its `.d` directory.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use tl_model::actions::{Family, FourTuple, Proto, Signal};
use tl_model::helper_proto::{
    DropInKey, HELPER_PROTO_VERSION, HelperCgroup, HelperError, HelperFrame, HelperReply,
    HelperReplyBody, HelperRequest, HelperTarget, ProcFiles,
};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_model::wire::{self, MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL};

const HELPER: &str = env!("CARGO_BIN_EXE_tl-helper");
const IDLE_ENV: &str = "TL_HELPER_IDLE_MS";
/// Longest wait for a reply before a test fails instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// The uid and gid of `nobody`.
const NOBODY: u32 = 65534;
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const FREEZE_UNIT: &str = "tasklens-test-freeze";
const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";
const TEST_SERVICE: &str = "tasklens-test.service";
const DROPIN_FILE: &str = "50-tasklens-recovery.conf";
const TEST_SERVICE_TEXT: &str =
    "[Unit]\nDescription=tasklens test unit\n[Service]\nExecStart=/usr/bin/sleep 600\n";
const ECONNABORTED: i32 = 103;
const ECONNRESET: i32 = 104;

/// True, after printing why, when the test does not run as root.
fn needs_root() -> bool {
    if rustix::process::geteuid().is_root() {
        return false;
    }
    println!("skipped: needs root");
    true
}

/// A child that is killed and reaped on drop, even when an assertion fails.
struct ChildGuard(Child);

impl ChildGuard {
    /// Waits up to `limit` for the child to exit.
    fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.0.try_wait().expect("try_wait") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A started helper and the engine's end of its socketpair.
struct Helper {
    _child: ChildGuard,
    sock: UnixStream,
    seq: u32,
}

impl Helper {
    /// Starts the helper and completes the `Hello` handshake.
    fn start() -> Self {
        let (sock, helper_end) = UnixStream::pair().expect("socketpair");
        let stdout = helper_end.try_clone().expect("clone helper end");
        let mut cmd = Command::new(HELPER);
        cmd.arg("--session")
            .stdin(Stdio::from(OwnedFd::from(helper_end)))
            .stdout(Stdio::from(OwnedFd::from(stdout)))
            .stderr(Stdio::inherit())
            .env_remove(IDLE_ENV);
        let child = ChildGuard(cmd.spawn().expect("spawn tl-helper"));
        // Dropping the command closes the test's copies of the helper's end.
        drop(cmd);
        sock.set_read_timeout(Some(READ_TIMEOUT))
            .expect("read timeout");
        let mut helper = Helper {
            _child: child,
            sock,
            seq: 0,
        };
        let replies = helper.call(HelperRequest::Hello {
            proto: HELPER_PROTO_VERSION,
        });
        assert_eq!(
            bodies(&replies),
            [HelperReplyBody::Hello {
                proto: HELPER_PROTO_VERSION,
            }]
        );
        helper
    }

    /// Sends one request and reads every reply to it. Checks `seq` and `last` on each reply.
    fn call(&mut self, request: HelperRequest) -> Vec<HelperReply> {
        let seq = self.seq;
        self.seq += 1;
        wire::write_frame(
            &mut self.sock,
            &HelperFrame { seq, request },
            MAX_HELPER_FRAME,
        )
        .expect("send frame");
        let replies = wire::read_reply(
            &mut self.sock,
            MAX_HELPER_FRAME,
            MAX_HELPER_REPLY_TOTAL,
            |r: &HelperReply| r.last,
        )
        .expect("read reply");
        assert!(!replies.is_empty(), "no reply to seq {seq}");
        for (i, reply) in replies.iter().enumerate() {
            assert_eq!(reply.seq, seq);
            assert_eq!(reply.last, i + 1 == replies.len());
        }
        replies
    }

    /// Sends one request that must get exactly one reply, and returns its body.
    fn call_one(&mut self, request: HelperRequest) -> HelperReplyBody {
        let label = format!("{request:?}");
        let replies = self.call(request);
        assert_eq!(replies.len(), 1, "{label}: {replies:?}");
        replies.into_iter().next().expect("one reply").body
    }
}

fn bodies(replies: &[HelperReply]) -> Vec<HelperReplyBody> {
    replies.iter().map(|r| r.body.clone()).collect()
}

fn boot_id() -> BootId {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("read boot id");
    BootId::from_uuid_text(&text).expect("parse boot id")
}

fn key_of(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
        boot_id: boot_id(),
        pid,
    }
}

/// A `sleep 60` child that runs as `nobody` (uid and gid 65534), and its process key. The child
/// is the test's own child, so its pid is `Child::id` (`runuser` would put a root parent between
/// them).
fn nobody_child() -> (ChildGuard, ProcessKey) {
    let child = ChildGuard(
        Command::new("sleep")
            .arg("60")
            .uid(NOBODY)
            .gid(NOBODY)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sleep as nobody"),
    );
    let pid = child.pid();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("read status");
    let uid_line = status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .expect("Uid line");
    assert!(
        uid_line.trim_start().starts_with(&NOBODY.to_string()),
        "child uid: {uid_line}"
    );
    (child, key_of(pid))
}

fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// The cgroup v2 path of `pid`, without the leading `/`.
fn cgroup_of(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("0::"))?;
    Some(line.strip_prefix('/').unwrap_or(line).to_owned())
}

/// The `frozen` value in the cgroup's `cgroup.events`.
fn events_frozen(rel_path: &str) -> Option<bool> {
    let text = std::fs::read_to_string(format!("{CGROUP_ROOT}/{rel_path}/cgroup.events")).ok()?;
    match text.lines().find_map(|l| l.strip_prefix("frozen "))? {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// Runs `systemctl <args>` and ignores the result.
fn systemctl(args: &[&str]) {
    let _ = Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The system scope `tasklens-test-freeze.scope` running `sleep 60`. `systemd-run` moves itself
/// into the scope and then execs `sleep`, so the child pid is the sleep's pid. On drop it thaws
/// the cgroup, kills and reaps the child and stops the unit.
struct ScopeGuard {
    child: ChildGuard,
    cg: Option<HelperCgroup>,
}

impl ScopeGuard {
    fn start() -> Self {
        let scope = format!("{FREEZE_UNIT}.scope");
        // Clears a scope left by a crashed run.
        systemctl(&["stop", &scope]);
        systemctl(&["reset-failed", &scope]);
        let unit_arg = format!("--unit={FREEZE_UNIT}");
        let child = ChildGuard(
            Command::new("systemd-run")
                .args(["--scope", "--quiet", "--collect", &unit_arg, "sleep", "60"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .expect("spawn systemd-run"),
        );
        let mut guard = ScopeGuard { child, cg: None };
        let suffix = format!("/{scope}");
        let mut path = None;
        let found = wait_for(Duration::from_secs(2), || {
            path = cgroup_of(guard.child.pid());
            path.as_deref().is_some_and(|p| p.ends_with(&suffix))
        });
        assert!(found, "the child did not move into {suffix}: {path:?}");
        let rel_path = path.expect("cgroup path");
        assert_eq!(rel_path, format!("system.slice/{scope}"));
        let ino = std::fs::metadata(format!("{CGROUP_ROOT}/{rel_path}"))
            .expect("stat cgroup dir")
            .ino();
        guard.cg = Some(HelperCgroup {
            rel_path,
            cgroup_id: ino,
        });
        guard
    }

    fn cg(&self) -> HelperCgroup {
        self.cg.clone().expect("scope cgroup")
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if let Some(cg) = &self.cg {
            let _ = std::fs::write(format!("{CGROUP_ROOT}/{}/cgroup.freeze", cg.rel_path), "0");
        }
        let _ = self.child.0.kill();
        let _ = self.child.0.wait();
        systemctl(&["stop", &format!("{FREEZE_UNIT}.scope")]);
    }
}

/// `/etc/systemd/system/tasklens-test.service`, written by this test and never started. On drop
/// it removes the unit's drop-in directory and the unit file.
struct TestServiceGuard {
    unit_file: PathBuf,
    dropin_dir: PathBuf,
}

impl TestServiceGuard {
    fn install() -> Self {
        let guard = TestServiceGuard {
            unit_file: Path::new(SYSTEM_UNIT_DIR).join(TEST_SERVICE),
            dropin_dir: Path::new(SYSTEM_UNIT_DIR).join(format!("{TEST_SERVICE}.d")),
        };
        // Clears a drop-in left by a crashed run; the unit file is overwritten.
        remove_dir_if_present(&guard.dropin_dir);
        std::fs::write(&guard.unit_file, TEST_SERVICE_TEXT).expect("write test unit");
        guard
    }
}

impl Drop for TestServiceGuard {
    fn drop(&mut self) {
        remove_dir_if_present(&self.dropin_dir);
        let _ = std::fs::remove_file(&self.unit_file);
    }
}

fn remove_dir_if_present(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("remove {}: {e}", dir.display()),
    }
}

#[test]
#[ignore = "real system as root"]
fn signal_other_users_process() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let (mut child, key) = nobody_child();
    let body = helper.call_one(HelperRequest::Signal {
        target: HelperTarget::Process(key),
        signal: Signal::Term,
    });
    assert_eq!(body, HelperReplyBody::Ok);
    let status = child.wait_exit(Duration::from_secs(5));
    assert_eq!(status.and_then(|s| s.signal()), Some(15), "{status:?}");
}

#[test]
#[ignore = "real system as root"]
fn stale_key_other_user_rejected() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let (mut child, key) = nobody_child();
    let stale = ProcessKey {
        start_ticks: key.start_ticks + 1,
        ..key
    };
    let body = helper.call_one(HelperRequest::Signal {
        target: HelperTarget::Process(stale),
        signal: Signal::Term,
    });
    assert_eq!(body, HelperReplyBody::Err(HelperError::IdentityChanged));
    // Gives a wrongly sent signal time to land before the alive check.
    let status = child.wait_exit(Duration::from_millis(500));
    assert!(status.is_none(), "the child ended: {status:?}");
}

#[test]
#[ignore = "real system as root"]
fn read_files_other_user() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let (_child, key) = nobody_child();
    let want = ProcFiles::ENVIRON | ProcFiles::IO | ProcFiles::EXE;
    let replies = helper.call(HelperRequest::ReadProcessFiles { key, files: want });
    let mut got = ProcFiles::empty();
    for reply in &replies {
        match &reply.body {
            HelperReplyBody::File { file, .. } => {
                assert_eq!(file.bits().count_ones(), 1, "{file:?}");
                got |= *file;
            }
            other => panic!("not a File body: {other:?}"),
        }
    }
    assert_eq!(got, want);
}

#[test]
#[ignore = "real system as root"]
fn freeze_other_users_scope() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let mut scope = ScopeGuard::start();
    let cg = scope.cg();

    let body = helper.call_one(HelperRequest::FreezeGroup(cg.clone()));
    assert_eq!(body, HelperReplyBody::Ok);
    assert_eq!(events_frozen(&cg.rel_path), Some(true));

    let body = helper.call_one(HelperRequest::ThawGroup(cg.clone()));
    assert_eq!(body, HelperReplyBody::Ok);
    let thawed = wait_for(Duration::from_secs(2), || {
        events_frozen(&cg.rel_path) == Some(false)
    });
    assert!(thawed, "frozen: {:?}", events_frozen(&cg.rel_path));

    let body = helper.call_one(HelperRequest::KillGroup(cg));
    assert_eq!(body, HelperReplyBody::Ok);
    let status = scope.child.wait_exit(Duration::from_secs(5));
    assert_eq!(status.and_then(|s| s.signal()), Some(9), "{status:?}");
}

#[test]
#[ignore = "real system as root"]
fn sock_destroy_loopback() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let mut client =
        TcpStream::connect(listener.local_addr().expect("listener addr")).expect("connect");
    let (_server, _) = listener.accept().expect("accept");
    let four_tuple = FourTuple {
        local: client.local_addr().expect("local addr"),
        remote: client.peer_addr().expect("peer addr"),
    };
    let cookie = rustix::net::sockopt::socket_cookie(&client).expect("socket cookie");

    let body = helper.call_one(HelperRequest::SockDestroy {
        family: Family::Inet,
        proto: Proto::Tcp,
        four_tuple,
        cookie,
    });
    assert_eq!(body, HelperReplyBody::Ok);

    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut buf = [0u8; 16];
    let got = client.read(&mut buf);
    let errno = got.as_ref().err().and_then(std::io::Error::raw_os_error);
    assert!(
        errno == Some(ECONNABORTED) || errno == Some(ECONNRESET),
        "read after destroy: {got:?}"
    );
}

#[test]
#[ignore = "real system as root"]
fn dropin_write_and_remove() {
    if needs_root() {
        return;
    }
    let mut helper = Helper::start();
    let service = TestServiceGuard::install();
    let keys = vec![(DropInKey::Restart, "on-failure".to_string())];

    let body = helper.call_one(HelperRequest::WriteServiceDropIn {
        unit: TEST_SERVICE.to_string(),
        keys: keys.clone(),
    });
    assert_eq!(body, HelperReplyBody::Ok);
    let file = service.dropin_dir.join(DROPIN_FILE);
    let mode = std::fs::metadata(&file).expect("stat drop-in").mode() & 0o7777;
    assert_eq!(mode, 0o644, "mode {mode:o}");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read drop-in"),
        tl_ops::dropin::render(&keys)
    );

    let body = helper.call_one(HelperRequest::WriteServiceDropIn {
        unit: TEST_SERVICE.to_string(),
        keys: Vec::new(),
    });
    assert_eq!(body, HelperReplyBody::Ok);
    assert!(!file.exists(), "drop-in file left");
    assert!(!service.dropin_dir.exists(), "drop-in directory left");
}
