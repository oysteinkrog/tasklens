//! Protocol tests of the built helper (E29-A1). Each test starts `tl-helper --session` as the
//! test user with stdin and stdout on one end of a socketpair, as the engine does (ADR-007).

use std::io::{BufRead, BufReader, IoSlice, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::MemfdFlags;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use tl_model::actions::{DumpMode, Family, FourTuple, IoPriority, Proto, SchedPolicy, Signal};
use tl_model::details::StringScanOpts;
use tl_model::helper_proto::{
    DropInKey, HELPER_PROTO_VERSION, HelperCgroup, HelperError, HelperFrame, HelperReply,
    HelperReplyBody, HelperRequest, HelperTarget, ProcFiles,
};
use tl_model::ids::ProcessKey;
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_model::wire::{self, MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL};

const HELPER: &str = env!("CARGO_BIN_EXE_tl-helper");
const IDLE_ENV: &str = "TL_HELPER_IDLE_MS";
/// Longest wait for a reply before a test fails instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

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
    child: ChildGuard,
    sock: UnixStream,
}

/// Starts the helper with `TL_HELPER_IDLE_MS` set only when `idle_ms` is given.
fn spawn(idle_ms: Option<u64>) -> Helper {
    let (sock, helper_end) = UnixStream::pair().expect("socketpair");
    let stdout = helper_end.try_clone().expect("clone helper end");
    let mut cmd = Command::new(HELPER);
    cmd.arg("--session")
        .stdin(Stdio::from(OwnedFd::from(helper_end)))
        .stdout(Stdio::from(OwnedFd::from(stdout)))
        .stderr(Stdio::piped())
        .env_remove(IDLE_ENV);
    if let Some(ms) = idle_ms {
        cmd.env(IDLE_ENV, ms.to_string());
    }
    let child = ChildGuard(cmd.spawn().expect("spawn tl-helper"));
    // Dropping the command closes the test's copies of the helper's end.
    drop(cmd);
    sock.set_read_timeout(Some(READ_TIMEOUT))
        .expect("read timeout");
    Helper { child, sock }
}

fn send(sock: &mut UnixStream, seq: u32, request: HelperRequest) {
    wire::write_frame(sock, &HelperFrame { seq, request }, MAX_HELPER_FRAME).expect("send frame");
}

/// Sends one frame in one `sendmsg` with `fd` as `SCM_RIGHTS`.
fn send_with_fd(sock: &UnixStream, seq: u32, request: HelperRequest, fd: BorrowedFd<'_>) {
    let mut bytes = Vec::new();
    wire::encode_frame(&HelperFrame { seq, request }, MAX_HELPER_FRAME, &mut bytes)
        .expect("encode frame");
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    let fds = [fd];
    assert!(control.push(SendAncillaryMessage::ScmRights(&fds)));
    let sent = rustix::net::sendmsg(
        sock.as_fd(),
        &[IoSlice::new(&bytes)],
        &mut control,
        SendFlags::empty(),
    )
    .expect("sendmsg");
    assert_eq!(sent, bytes.len());
}

/// Reads every reply of one request. Fails on any frame over 65_536 bytes.
fn recv(sock: &mut UnixStream) -> Vec<HelperReply> {
    wire::read_reply(
        sock,
        MAX_HELPER_FRAME,
        MAX_HELPER_REPLY_TOTAL,
        |r: &HelperReply| r.last,
    )
    .expect("read reply")
}

fn hello(sock: &mut UnixStream, seq: u32) {
    send(
        sock,
        seq,
        HelperRequest::Hello {
            proto: HELPER_PROTO_VERSION,
        },
    );
    let replies = recv(sock);
    assert_eq!(
        replies,
        [HelperReply {
            seq,
            last: true,
            body: HelperReplyBody::Hello {
                proto: HELPER_PROTO_VERSION,
            },
        }]
    );
}

fn boot_id() -> BootId {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("read boot id");
    BootId::from_uuid_text(&text).expect("parse boot id")
}

/// A `sleep 30` child of the test and its process key.
fn sleeper() -> (ChildGuard, ProcessKey) {
    let child = ChildGuard(
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sleep"),
    );
    let pid = child.pid();
    let key = ProcessKey {
        start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
        boot_id: boot_id(),
        pid,
    };
    (child, key)
}

fn read_stderr(child: &mut ChildGuard) -> String {
    let mut text = String::new();
    if let Some(stderr) = child.0.stderr.take() {
        BufReader::new(stderr)
            .read_line(&mut text)
            .expect("read stderr");
    }
    text
}

#[test]
fn hello_handshake() {
    let mut h = spawn(None);
    hello(&mut h.sock, 5);
}

/// Security regression: no request runs before a matching `Hello`.
#[test]
fn first_request_must_be_hello() {
    let mut h = spawn(None);
    let (_sleep, key) = sleeper();
    send(
        &mut h.sock,
        1,
        HelperRequest::Signal {
            target: HelperTarget::Process(key),
            signal: Signal::Cont,
        },
    );
    assert_eq!(
        recv(&mut h.sock),
        [HelperReply {
            seq: 1,
            last: true,
            body: HelperReplyBody::Err(HelperError::NotAllowed),
        }]
    );
    let status = h.child.wait_exit(Duration::from_secs(5)).expect("exit");
    assert_eq!(status.code(), Some(3));
}

#[test]
fn wrong_version_refused() {
    let mut h = spawn(None);
    send(
        &mut h.sock,
        0,
        HelperRequest::Hello {
            proto: HELPER_PROTO_VERSION + 1,
        },
    );
    let replies = recv(&mut h.sock);
    assert_eq!(
        replies[0].body,
        HelperReplyBody::Err(HelperError::NotAllowed)
    );
    let status = h.child.wait_exit(Duration::from_secs(5)).expect("exit");
    assert_eq!(status.code(), Some(3));
}

/// Security regression: an oversize length prefix ends the session before any payload is read.
#[test]
fn oversize_frame_exits() {
    let mut h = spawn(None);
    h.sock
        .write_all(&65_537u32.to_le_bytes())
        .expect("write prefix");
    let status = h.child.wait_exit(Duration::from_secs(5)).expect("exit");
    assert_eq!(status.code(), Some(3));
    assert_eq!(read_stderr(&mut h.child), "tl-helper: protocol error\n");
}

#[test]
fn eof_exits() {
    let Helper {
        mut child,
        mut sock,
    } = spawn(None);
    hello(&mut sock, 0);
    let closed = Instant::now();
    drop(sock);
    let status = child
        .wait_exit(Duration::from_secs(5))
        .expect("exit after EOF");
    let elapsed = closed.elapsed();
    assert_eq!(status.code(), Some(0));
    // 2 s holds on a busy machine and stays below the 5 s `wait_exit` limit (tasklens-cr0.32).
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn idle_exit() {
    let mut h = spawn(Some(200));
    hello(&mut h.sock, 0);
    let answered = Instant::now();
    let status = h
        .child
        .wait_exit(Duration::from_secs(5))
        .expect("idle exit");
    let elapsed = answered.elapsed();
    assert_eq!(status.code(), Some(0));
    // The helper's idle clock starts before the client sees the reply, so allow a little under 200.
    // The upper bound is 2 s so a busy machine does not fail it (tasklens-cr0.32).
    assert!(
        (Duration::from_millis(180)..=Duration::from_secs(2)).contains(&elapsed),
        "{elapsed:?}"
    );
}

/// Security regression: part of a frame does not hold the helper past its idle limit. The frame
/// is still short at the deadline, so the helper exits 3.
#[test]
fn partial_frame_cannot_hold_idle_exit() {
    let mut h = spawn(Some(200));
    hello(&mut h.sock, 0);
    let answered = Instant::now();
    h.sock.write_all(&[1]).expect("write one prefix byte");
    let status = h
        .child
        .wait_exit(Duration::from_secs(5))
        .expect("exit with a partial frame");
    let elapsed = answered.elapsed();
    assert_eq!(status.code(), Some(3));
    // Exit 3 shows the deadline ended it. The upper bound is 2 s for a busy machine and stays
    // below the 5 s `wait_exit` limit (tasklens-cr0.32).
    assert!(
        (Duration::from_millis(200)..=Duration::from_secs(2)).contains(&elapsed),
        "{elapsed:?}"
    );
    assert_eq!(read_stderr(&mut h.child), "tl-helper: protocol error\n");
}

/// Security regression: part of a frame during a scan does not stall the scan's cancel check.
/// The scan ends, and the kept bytes finish the next request once the rest arrives. The helper
/// scans itself, the one process it may read as the test user under Yama ptrace scope 1.
#[test]
fn partial_frame_does_not_stall_scan() {
    let mut h = spawn(None);
    hello(&mut h.sock, 0);
    let pid = h.child.pid();
    let key = ProcessKey {
        start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
        boot_id: boot_id(),
        pid,
    };
    let mut next = Vec::new();
    wire::encode_frame(
        &HelperFrame {
            seq: 2,
            request: HelperRequest::Hello {
                proto: HELPER_PROTO_VERSION,
            },
        },
        MAX_HELPER_FRAME,
        &mut next,
    )
    .expect("encode frame");
    let mut bytes = Vec::new();
    wire::encode_frame(
        &HelperFrame {
            seq: 1,
            request: HelperRequest::ScanStrings {
                key,
                opts: StringScanOpts::default(),
            },
        },
        MAX_HELPER_FRAME,
        &mut bytes,
    )
    .expect("encode frame");
    bytes.extend_from_slice(&next[..1]);
    h.sock.write_all(&bytes).expect("write scan and one byte");

    let replies = recv(&mut h.sock);
    assert!(
        replies
            .iter()
            .all(|r| r.seq == 1 && matches!(r.body, HelperReplyBody::Strings(_))),
        "{:?}",
        replies.iter().map(|r| (r.seq, r.last)).collect::<Vec<_>>()
    );

    h.sock.write_all(&next[1..]).expect("write the rest");
    // Rule "hello twice".
    assert_eq!(
        recv(&mut h.sock),
        [HelperReply {
            seq: 2,
            last: true,
            body: HelperReplyBody::Err(HelperError::NotAllowed),
        }]
    );
}

/// The helper dies with its parent (`PR_SET_PDEATHSIG`). A non-interactive shell starts a
/// background job with stdin on /dev/null, so the script passes the socket through fd 3.
#[test]
fn parent_death_kills_helper() {
    let (mut sock, helper_end) = UnixStream::pair().expect("socketpair");
    sock.set_read_timeout(Some(READ_TIMEOUT))
        .expect("read timeout");
    let stdout = helper_end.try_clone().expect("clone helper end");
    let script =
        format!("exec 3<&0; '{HELPER}' --session <&3 3<&- & echo $! >&2; exec 3<&-; sleep 30");
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::from(OwnedFd::from(helper_end)))
        .stdout(Stdio::from(OwnedFd::from(stdout)))
        .stderr(Stdio::piped())
        .env_remove(IDLE_ENV);
    let mut sh = ChildGuard(cmd.spawn().expect("spawn sh"));
    drop(cmd);
    let pid: u32 = read_stderr(&mut sh).trim().parse().expect("helper pid");

    // The Hello reply proves the helper has run its prctl.
    hello(&mut sock, 0);
    sh.0.kill().expect("kill sh");
    sh.0.wait().expect("reap sh");

    let deadline = Instant::now() + Duration::from_secs(1);
    let gone = loop {
        if helper_gone(pid) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(gone, "helper {pid} still runs after its parent died");
}

/// True when `pid` no longer exists or is a zombie.
fn helper_gone(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    let state = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.chars().next());
    state == Some('Z')
}

#[test]
fn multi_frame_reply() {
    let mut h = spawn(None);
    hello(&mut h.sock, 0);
    let (_sleep, key) = sleeper();
    send(
        &mut h.sock,
        1,
        HelperRequest::ReadProcessFiles {
            key,
            files: ProcFiles::MAPS | ProcFiles::SMAPS,
        },
    );
    // `recv` fails on any frame payload over 65_536 bytes.
    let replies = recv(&mut h.sock);
    assert!(replies.len() >= 2, "{} replies", replies.len());
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(reply.seq, 1);
        assert_eq!(reply.last, i + 1 == replies.len());
        match &reply.body {
            HelperReplyBody::File { file, .. } => {
                assert!(*file == ProcFiles::MAPS || *file == ProcFiles::SMAPS);
            }
            other => panic!("not a File body: {other:?}"),
        }
    }
}

/// The index of each variant. Fails to compile when a variant is added, so this test must
/// send it too.
fn variant_index(request: &HelperRequest) -> usize {
    match request {
        HelperRequest::Hello { .. } => 0,
        HelperRequest::Signal { .. } => 1,
        HelperRequest::SetNice { .. } => 2,
        HelperRequest::SetSched { .. } => 3,
        HelperRequest::SetAffinity { .. } => 4,
        HelperRequest::SetIoPriority { .. } => 5,
        HelperRequest::FreezeGroup(_) => 6,
        HelperRequest::ThawGroup(_) => 7,
        HelperRequest::KillGroup(_) => 8,
        HelperRequest::ReadProcessFiles { .. } => 9,
        HelperRequest::ScanStrings { .. } => 10,
        HelperRequest::Dump { .. } => 11,
        HelperRequest::ResolveSocketInodes { .. } => 12,
        HelperRequest::SockDestroy { .. } => 13,
        HelperRequest::WriteServiceDropIn { .. } => 14,
        HelperRequest::NetnsSockDiagDump { .. } => 15,
        HelperRequest::CancelScan { .. } => 16,
    }
}

const VARIANTS: usize = 17;

/// One well-formed request of each of the 17 variants. Every reply decodes, the session stays
/// up, and the requests that must be refused are refused. Nothing here changes the system, even
/// as root: the cgroup path fails validation, the unit name is invalid and the dump key is stale.
#[test]
fn every_request_variant_has_an_arm() {
    let mut h = spawn(None);
    let (_sleep, key) = sleeper();
    let target = HelperTarget::Process(key);
    let mut seen = [false; VARIANTS];

    hello(&mut h.sock, 0);
    seen[variant_index(&HelperRequest::Hello { proto: 0 })] = true;

    let mut cpu0 = [0u64; 16];
    cpu0[0] = 1;
    // "user.slice" is not a scope or service, so validation refuses it.
    let user_slice = HelperCgroup {
        rel_path: "user.slice".to_string(),
        cgroup_id: 0,
    };
    let refused = Some(HelperReplyBody::Err(HelperError::NotAllowed));
    let requests: Vec<(HelperRequest, Option<HelperReplyBody>)> = vec![
        (
            HelperRequest::Signal {
                target,
                signal: Signal::Cont,
            },
            None,
        ),
        (HelperRequest::SetNice { target, nice: 5 }, None),
        (
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Other,
                rt_prio: 0,
            },
            None,
        ),
        (
            HelperRequest::SetAffinity {
                target,
                set: CpuSet(cpu0),
            },
            None,
        ),
        (
            HelperRequest::SetIoPriority {
                key,
                prio: IoPriority::BestEffort(4),
            },
            None,
        ),
        (
            HelperRequest::FreezeGroup(user_slice.clone()),
            refused.clone(),
        ),
        (
            HelperRequest::ThawGroup(user_slice.clone()),
            refused.clone(),
        ),
        (HelperRequest::KillGroup(user_slice), refused.clone()),
        (
            HelperRequest::ReadProcessFiles {
                key,
                files: ProcFiles::STATUS,
            },
            None,
        ),
        (
            HelperRequest::ScanStrings {
                key,
                opts: StringScanOpts {
                    min_len: 4,
                    ascii: true,
                    utf8: false,
                    utf16le: false,
                    include_file_backed: false,
                },
            },
            None,
        ),
        (HelperRequest::ResolveSocketInodes { inodes: vec![1] }, None),
        (
            HelperRequest::SockDestroy {
                family: Family::Inet,
                proto: Proto::Tcp,
                four_tuple: FourTuple {
                    local: "127.0.0.1:1".parse().expect("local"),
                    remote: "127.0.0.1:2".parse().expect("remote"),
                },
                cookie: 0,
            },
            None,
        ),
        (
            HelperRequest::WriteServiceDropIn {
                unit: "bad/name".to_string(),
                keys: vec![(DropInKey::Restart, "no".to_string())],
            },
            refused.clone(),
        ),
        (
            HelperRequest::NetnsSockDiagDump { netns_ino: 0 },
            Some(HelperReplyBody::Err(HelperError::NoSuchProcess)),
        ),
        (
            HelperRequest::CancelScan { seq: 999 },
            Some(HelperReplyBody::Ok),
        ),
        (HelperRequest::Hello { proto: 1 }, refused),
    ];

    let mut seq = 1;
    for (request, expected) in requests {
        seen[variant_index(&request)] = true;
        let label = format!("{request:?}");
        send(&mut h.sock, seq, request);
        check_replies(&recv(&mut h.sock), seq, expected.as_ref(), &label);
        seq += 1;
    }

    // Dump with a stale key: refused before gdb could run.
    let memfd = rustix::fs::memfd_create("tl-helper-test", MemfdFlags::CLOEXEC).expect("memfd");
    let stale = ProcessKey {
        start_ticks: key.start_ticks + 1,
        ..key
    };
    let dump = HelperRequest::Dump {
        key: stale,
        mode: DumpMode::Mini,
    };
    seen[variant_index(&dump)] = true;
    send_with_fd(&h.sock, seq, dump, memfd.as_fd());
    check_replies(
        &recv(&mut h.sock),
        seq,
        Some(&HelperReplyBody::Err(HelperError::IdentityChanged)),
        "Dump",
    );

    assert!(seen.iter().all(|s| *s), "not every variant sent: {seen:?}");
    assert!(
        h.child.0.try_wait().expect("try_wait").is_none(),
        "the helper exited"
    );
}

/// Every reply has `seq`, only the final one has `last`, and a single reply equals `expected`
/// when given.
fn check_replies(
    replies: &[HelperReply],
    seq: u32,
    expected: Option<&HelperReplyBody>,
    label: &str,
) {
    assert!(!replies.is_empty(), "{label}: no reply");
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(reply.seq, seq, "{label}");
        assert_eq!(reply.last, i + 1 == replies.len(), "{label}");
    }
    if let Some(expected) = expected {
        assert_eq!(replies.len(), 1, "{label}");
        assert_eq!(&replies[0].body, expected, "{label}");
    }
}

fn policy_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/io.github.oysteinkrog.Tasklens.policy")
}

/// The polkit policy has exactly four actions, each tied to one fixed program, none defaulting
/// to `yes`, with the owner-confirmed texts (D-019). The fourth is the SMART read
/// (q4x76.33), so SMART does not use the generic `org.freedesktop.policykit.exec` action.
#[test]
fn policy_file_has_four_actions() {
    let path = policy_path();
    let text = std::fs::read_to_string(&path).expect("read the policy file");

    match Command::new("xmllint")
        .arg("--noout")
        .arg("--nonet")
        .arg(&path)
        .output()
    {
        Ok(out) => assert!(
            out.status.success(),
            "xmllint failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(_) => eprintln!("xmllint not installed, well-formedness check skipped"),
    }

    assert_eq!(text.matches("<action id=\"").count(), 4);
    assert_eq!(text.matches(" id=\"").count(), 4, "no other id anywhere");
    assert!(
        !text.contains("smartctl"),
        "no action names smartctl itself"
    );
    assert!(!text.contains(">yes<"));

    let expected = [
        (
            "io.github.oysteinkrog.tasklens.helper",
            "tl-helper",
            "Run tasklens administrator actions",
            "Authentication is required to let tasklens act on other users' processes, system services and network connections.",
        ),
        (
            "io.github.oysteinkrog.tasklens.setup",
            "tasklens-setup",
            "Change tasklens system setup",
            "Authentication is required to change system settings for tasklens.",
        ),
        (
            "io.github.oysteinkrog.tasklens.netmon",
            "tl-netmon",
            "Run the tasklens network monitor",
            "Authentication is required to run the tasklens network monitor.",
        ),
        (
            "io.github.oysteinkrog.tasklens.smart",
            "tasklens-smart",
            "Read disk SMART data with tasklens",
            "Authentication is required to let tasklens read SMART health data from a disk.",
        ),
    ];
    let pieces: Vec<&str> = text.split("<action id=\"").skip(1).collect();
    assert_eq!(pieces.len(), expected.len());
    for (piece, (id, bin, description, message)) in pieces.iter().zip(expected) {
        assert!(piece.starts_with(&format!("{id}\"")), "{id}");
        assert!(
            piece.contains("<allow_active>auth_admin_keep</allow_active>"),
            "{id}: allow_active"
        );
        assert!(
            piece.contains(&format!(
                "<annotate key=\"org.freedesktop.policykit.exec.path\">/usr/lib/tasklens/{bin}</annotate>"
            )),
            "{id}: exec path"
        );
        assert!(
            piece.contains(&format!("<description>{description}</description>")),
            "{id}"
        );
        assert!(
            piece.contains(&format!("<message>{message}</message>")),
            "{id}"
        );
    }
}
