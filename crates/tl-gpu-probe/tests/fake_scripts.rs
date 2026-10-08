//! Runs the real `tl-gpu-probe` binary with each `--fake` script over a socketpair.
//!
//! No `unsafe`: the test clears `CLOEXEC` on the child's end and passes its number as `--fd`, so
//! the child inherits it without `pre_exec` or `dup2`.

#![cfg(feature = "fake")]
#![allow(clippy::unwrap_used)]

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::io::{FdFlags, fcntl_setfd};
use tl_model::gpu_proto::{InitFailure, ProbeRequest, ProbeResponse};
use tl_model::wire::{MAX_PROBE_FRAME, WireError, read_frame, write_frame};

/// Held from clearing `CLOEXEC` until the spawn returns, so a child started by a parallel test
/// never inherits another test's socket.
static SPAWN: Mutex<()> = Mutex::new(());

const PROTO: u16 = tl_model::gpu_proto::GPU_PROTO_VERSION;

fn spawn(script: &str) -> (UnixStream, Child) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let _guard = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
    fcntl_setfd(&theirs, FdFlags::empty()).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_tl-gpu-probe"))
        .args(["--fake", script, "--fd"])
        .arg(theirs.as_raw_fd().to_string())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    // The child has its copy; drop ours so only the child holds that end.
    drop(theirs);
    ours.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (ours, child)
}

fn send(c: &mut UnixStream, req: &ProbeRequest) {
    write_frame(c, req, MAX_PROBE_FRAME).unwrap();
}

fn recv(c: &mut UnixStream) -> ProbeResponse {
    read_frame(c, MAX_PROBE_FRAME).unwrap()
}

fn sample(seq: u32) -> ProbeRequest {
    ProbeRequest::Sample {
        seq,
        processes: true,
    }
}

/// Waits for the child with a 10 s limit.
fn wait(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("probe did not exit within 10 s");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn hello_ok(c: &mut UnixStream) {
    send(c, &ProbeRequest::Hello { proto: PROTO });
    match recv(c) {
        ProbeResponse::Hello { proto, devices, .. } => {
            assert_eq!(proto, PROTO);
            assert_eq!(devices.len(), 1);
            assert_eq!(devices[0].uuid, [0x42; 16]);
            assert_eq!(devices[0].name, "Fake GPU 5070");
        }
        other => panic!("unexpected response {other:?}"),
    }
}

#[test]
fn normal_hello_and_samples() {
    let (mut c, mut child) = spawn("normal");
    hello_ok(&mut c);
    let mut last_mem = 0;
    for seq in 1..=3 {
        send(&mut c, &sample(seq));
        match recv(&mut c) {
            ProbeResponse::Sample {
                seq: got,
                t_ns,
                devices,
                processes,
            } => {
                assert_eq!(got, seq);
                assert!(t_ns > 0);
                let mem = devices[0].mem_used.ok().unwrap();
                assert!(mem > last_mem);
                last_mem = mem;
                let rows = processes.unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].pid, std::process::id());
            }
            other => panic!("unexpected response {other:?}"),
        }
    }
    send(&mut c, &ProbeRequest::Shutdown);
    assert_eq!(wait(&mut child).code(), Some(0));
}

#[test]
fn proto_mismatch_previous_version() {
    let (mut c, mut child) = spawn("proto-mismatch");
    send(&mut c, &ProbeRequest::Hello { proto: PROTO });
    match recv(&mut c) {
        ProbeResponse::Hello { proto, .. } => assert_eq!(proto, PROTO - 1),
        other => panic!("unexpected response {other:?}"),
    }
    send(&mut c, &ProbeRequest::Shutdown);
    assert_eq!(wait(&mut child).code(), Some(0));
}

#[test]
fn init_failure_kinds() {
    for (script, want) in [
        ("init-library-not-found", InitFailure::LibraryNotFound),
        ("init-driver-not-loaded", InitFailure::DriverNotLoaded),
        ("init-no-permission", InitFailure::NoPermission),
    ] {
        let (mut c, mut child) = spawn(script);
        send(&mut c, &ProbeRequest::Hello { proto: PROTO });
        match recv(&mut c) {
            ProbeResponse::InitFailed { kind, .. } => assert_eq!(kind, want, "{script}"),
            other => panic!("{script}: unexpected response {other:?}"),
        }
        send(&mut c, &ProbeRequest::Shutdown);
        assert_eq!(wait(&mut child).code(), Some(0));
    }
}

#[test]
fn segv_dies_with_signal_11() {
    let (mut c, mut child) = spawn("segv-on-sample");
    hello_ok(&mut c);
    send(&mut c, &sample(1));
    assert_eq!(wait(&mut child).signal(), Some(11));
}

#[test]
fn oversize_frame_is_too_large() {
    let (mut c, mut child) = spawn("oversize-on-sample");
    hello_ok(&mut c);
    send(&mut c, &sample(1));
    let err = read_frame::<_, ProbeResponse>(&mut c, MAX_PROBE_FRAME).unwrap_err();
    assert!(
        matches!(err, WireError::TooLarge { len, .. } if len == MAX_PROBE_FRAME + 1),
        "{err:?}"
    );
    assert_eq!(wait(&mut child).code(), Some(0));
}

#[test]
fn exit_after_three() {
    let (mut c, mut child) = spawn("exit-after-3");
    hello_ok(&mut c);
    for seq in 1..=3 {
        send(&mut c, &sample(seq));
        assert!(matches!(recv(&mut c), ProbeResponse::Sample { seq: s, .. } if s == seq));
    }
    send(&mut c, &sample(4));
    assert_eq!(wait(&mut child).code(), Some(0));
}

#[test]
fn sleep_on_sample_delays() {
    let (mut c, mut child) = spawn("sleep-on-sample");
    hello_ok(&mut c);
    let start = Instant::now();
    send(&mut c, &sample(1));
    assert!(matches!(recv(&mut c), ProbeResponse::Sample { seq: 1, .. }));
    assert!(start.elapsed() >= Duration::from_secs(2));
    let start = Instant::now();
    send(&mut c, &sample(2));
    assert!(matches!(recv(&mut c), ProbeResponse::Sample { seq: 2, .. }));
    assert!(start.elapsed() < Duration::from_secs(1));
    send(&mut c, &ProbeRequest::Shutdown);
    assert_eq!(wait(&mut child).code(), Some(0));
}

#[test]
fn unknown_script_exits_2_with_list() {
    let out = Command::new(env!("CARGO_BIN_EXE_tl-gpu-probe"))
        .args(["--fake", "bogus", "--fd", "99"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("normal") && stderr.contains("proto-mismatch"),
        "{stderr}"
    );
}
