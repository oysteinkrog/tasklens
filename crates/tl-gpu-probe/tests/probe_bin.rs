//! Runs the real `tl-gpu-probe` binary in `--fd` mode over a socketpair (E13-A1).
//!
//! On a machine with an NVIDIA driver the probe answers `Hello` with real devices; elsewhere it
//! answers `InitFailed`. Both are fine here. A crash or a protocol error is not. No `unsafe`: the
//! child inherits its end because the test clears `CLOEXEC` and passes the number as `--fd`.

#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::io::{FdFlags, fcntl_setfd};
use tl_model::gpu_proto::{InitFailure, ProbeRequest, ProbeResponse};
use tl_model::wire::{MAX_PROBE_FRAME, read_frame, write_frame};

/// Held from clearing `CLOEXEC` until the spawn returns, so a child started by a parallel test
/// never inherits another test's socket.
static SPAWN: Mutex<()> = Mutex::new(());

const PROTO: u16 = tl_model::gpu_proto::GPU_PROTO_VERSION;

/// Starts the probe with `args` followed by `--fd <n>`. Stderr is piped so tests can check it.
fn spawn(args: &[&str]) -> (UnixStream, Child) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let _guard = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
    fcntl_setfd(&theirs, FdFlags::empty()).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_tl-gpu-probe"))
        .args(args)
        .arg("--fd")
        .arg(theirs.as_raw_fd().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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

/// Waits for the child with a `limit` measured from `start`.
fn wait_within(child: &mut Child, start: Instant, limit: Duration) -> ExitStatus {
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("probe did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn stderr_of(child: &mut Child) -> String {
    let mut s = String::new();
    child.stderr.take().unwrap().read_to_string(&mut s).unwrap();
    s
}

/// Sends `Hello` and checks the answer is a real `Hello` or an expected `InitFailed`.
fn hello_real_or_init_failed_on(c: &mut UnixStream) {
    let start = Instant::now();
    send(c, &ProbeRequest::Hello { proto: PROTO });
    let resp = recv(c);
    let took = start.elapsed();
    match resp {
        ProbeResponse::Hello {
            proto,
            driver,
            nvml,
            devices,
        } => {
            assert_eq!(proto, 1);
            assert!(!devices.is_empty(), "Hello with no devices");
            let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
            eprintln!("Hello in {took:?}: driver {driver}, nvml {nvml}, devices {names:?}");
        }
        ProbeResponse::InitFailed { kind, message } => {
            assert!(
                matches!(
                    kind,
                    InitFailure::LibraryNotFound | InitFailure::DriverNotLoaded
                ),
                "unexpected InitFailed {kind:?}: {message}"
            );
            eprintln!("InitFailed in {took:?}: {kind:?}: {message}");
        }
        other => panic!("unexpected response {other:?}"),
    }
    assert!(took < Duration::from_secs(3), "Hello took {took:?}");
}

#[test]
fn hello_real_or_init_failed() {
    let (mut c, mut child) = spawn(&[]);
    hello_real_or_init_failed_on(&mut c);
    send(&mut c, &ProbeRequest::Shutdown);
    let status = wait_within(&mut child, Instant::now(), Duration::from_secs(10));
    assert_eq!(status.code(), Some(0));
}

#[test]
fn shutdown_exits_zero_fast() {
    let (mut c, mut child) = spawn(&[]);
    hello_real_or_init_failed_on(&mut c);
    let start = Instant::now();
    send(&mut c, &ProbeRequest::Shutdown);
    let status = wait_within(&mut child, start, Duration::from_millis(200));
    assert_eq!(status.code(), Some(0));
}

#[test]
fn eof_exits_zero() {
    let (c, mut child) = spawn(&[]);
    let start = Instant::now();
    drop(c);
    let status = wait_within(&mut child, start, Duration::from_millis(200));
    assert_eq!(status.code(), Some(0));
}

#[test]
fn garbage_prefix_exits_zero() {
    let (mut c, mut child) = spawn(&[]);
    // A length prefix of 4 GiB - 1: far past `MAX_PROBE_FRAME`, so the probe must stop at once.
    c.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
    let status = wait_within(&mut child, Instant::now(), Duration::from_secs(10));
    assert_eq!(status.code(), Some(0));
    // No `Hello` was sent, so NVML was never loaded and the probe itself must print nothing.
    assert_eq!(stderr_of(&mut child), "");
    let mut buf = [0u8; 1];
    assert_eq!(c.read(&mut buf).unwrap(), 0);
}

#[cfg(feature = "fake")]
#[test]
fn every_message_roundtrips_through_binary() {
    let (mut c, mut child) = spawn(&["--fake", "normal"]);
    send(&mut c, &ProbeRequest::Hello { proto: PROTO });
    assert!(matches!(
        recv(&mut c),
        ProbeResponse::Hello { proto: PROTO, ref devices, .. } if devices.len() == 1
    ));
    for seq in 1..=5u32 {
        let want_processes = seq % 2 == 1;
        send(
            &mut c,
            &ProbeRequest::Sample {
                seq,
                processes: want_processes,
            },
        );
        match recv(&mut c) {
            ProbeResponse::Sample {
                seq: got,
                devices,
                processes,
                ..
            } => {
                assert_eq!(got, seq);
                assert_eq!(devices.len(), 1);
                assert_eq!(processes.is_some(), want_processes, "seq {seq}");
            }
            other => panic!("unexpected response {other:?}"),
        }
    }
    let start = Instant::now();
    send(&mut c, &ProbeRequest::Shutdown);
    let status = wait_within(&mut child, start, Duration::from_millis(200));
    assert_eq!(status.code(), Some(0));
}
