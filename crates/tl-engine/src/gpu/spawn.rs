//! Spawns the `tl-gpu-probe` child over a socketpair (ADR-007 "Spawn", E13-008).
//!
//! The child gets its end of a `SOCK_STREAM` socketpair on fd 3, a cleared environment and
//! `SIGKILL` as its parent-death signal, so a crashed engine never leaves a probe behind.
//! `asupersync::process` is never used here (zombie bug `bi2462.114`).
//!
//! This is the only file in `tl-engine` allowed to contain `unsafe` (ADR-001 rule 4). The one
//! `unsafe` API is `CommandExt::pre_exec`.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use rustix::io::{Errno, FdFlags, dup2, fcntl_setfd};
use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
use rustix::process::{Signal, getpid, getppid, set_parent_process_death_signal};

use super::ProbeError;

/// The fd that holds the socket in the child.
pub const CHILD_FD: RawFd = 3;

/// Environment variable whose value the child gets as `RUST_LOG` (ADR-015).
const LOG_ENV: &str = "TASKLENS_LOG";

/// A running probe child and the parent end of its socket.
///
/// Dropping it while the child still runs sends `SIGKILL` and reaps the child, so no zombie is
/// left. The orderly shutdown path waits for the child before the drop.
pub struct ProbeChild {
    pub child: Child,
    /// Parent end of the socketpair, `CLOEXEC`.
    pub sock: OwnedFd,
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // The wait is bounded: SIGKILL cannot be caught or ignored.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The child's whole environment: `PATH` and `LANG` copied from this process when set, and
/// `RUST_LOG` set to `$TASKLENS_LOG` when that is set.
pub fn child_env() -> Vec<(&'static str, OsString)> {
    let mut env = Vec::with_capacity(3);
    for key in ["PATH", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            env.push((key, value));
        }
    }
    if let Some(value) = std::env::var_os(LOG_ENV) {
        env.push(("RUST_LOG", value));
    }
    env
}

/// Starts `probe --fd 3 <extra_args>` with its socket on fd 3.
///
/// Blocking (fork and exec): callers run it inside `spawn_blocking`. Every failure is
/// [`ProbeError::Spawn`], and on failure both socket ends are closed.
pub fn spawn_probe(probe: &Path, extra_args: &[&OsStr]) -> Result<ProbeChild, ProbeError> {
    let (parent, child_end) = socketpair(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|e| spawn_error(&io::Error::from(e)))?;

    let mut cmd = Command::new(probe);
    cmd.arg("--fd")
        .arg(CHILD_FD.to_string())
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .env_clear()
        .envs(child_env());
    set_child_setup(
        &mut cmd,
        child_end.as_raw_fd(),
        getpid().as_raw_nonzero().get(),
    );

    let child = cmd.spawn().map_err(|e| spawn_error(&e))?;
    drop(child_end);
    Ok(ProbeChild {
        child,
        sock: parent,
    })
}

fn spawn_error(e: &io::Error) -> ProbeError {
    ProbeError::Spawn(e.to_string().into())
}

/// Installs [`child_setup`] as the `pre_exec` hook.
#[allow(unsafe_code)]
fn set_child_setup(cmd: &mut Command, child_raw: RawFd, parent_pid: i32) {
    // SAFETY: the hook runs in the forked child before exec, where only async-signal-safe calls
    // are allowed. `child_setup` makes only raw system calls through rustix (`dup2`, `fcntl`,
    // `prctl`, `getppid`), allocates nothing (errors are `from_raw_os_error`) and touches no
    // locks. The closure captures two integers.
    unsafe {
        cmd.pre_exec(move || child_setup(child_raw, parent_pid));
    }
}

/// Runs in the child between fork and exec: moves `child_raw` to fd 3 without `CLOEXEC`, sets
/// the parent-death signal to `SIGKILL`, and fails if the parent `parent_pid` is already gone.
#[allow(unsafe_code)]
fn child_setup(child_raw: RawFd, parent_pid: i32) -> io::Result<()> {
    if child_raw == CHILD_FD {
        // dup2 onto itself would leave CLOEXEC set, so clear it directly.
        // SAFETY: fd 3 is the open child end of the socketpair; the parent keeps it open until
        // spawn returns, and the forked child has a copy.
        let fd = unsafe { BorrowedFd::borrow_raw(CHILD_FD) };
        fcntl_setfd(fd, FdFlags::empty())?;
    } else {
        // SAFETY: `child_raw` is the open child end of the socketpair (see above).
        let source = unsafe { BorrowedFd::borrow_raw(child_raw) };
        // SAFETY: rustix's `dup2` takes the target as `&mut OwnedFd` but only passes its number
        // to the system call. `ManuallyDrop` keeps it from ever being closed here. dup2
        // replaces whatever fd 3 was in this child, and the new fd 3 has no CLOEXEC flag.
        let mut target = ManuallyDrop::new(unsafe { OwnedFd::from_raw_fd(CHILD_FD) });
        dup2(source, &mut target)?;
    }
    set_parent_process_death_signal(Some(Signal::KILL))?;
    // The parent may have died between fork and prctl, and then the signal never comes.
    if getppid().map(|p| p.as_raw_nonzero().get()) != Some(parent_pid) {
        return Err(Errno::SRCH.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tl_model::gpu_proto::{GPU_PROTO_VERSION, ProbeRequest, ProbeResponse};
    use tl_model::wire::{MAX_PROBE_FRAME, read_frame, write_frame};

    use super::*;
    use crate::gpu::PROBE_ENV;

    /// Serializes the tests in this module so their fds do not mix in the fd counts.
    static LOCK: Mutex<()> = Mutex::new(());

    /// The probe binary: `$TASKLENS_GPU_PROBE`, else the workspace debug build. `None` skips
    /// the test with a note, because the probe is a separate binary crate.
    fn test_probe() -> Option<PathBuf> {
        let path = std::env::var_os(PROBE_ENV)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/tl-gpu-probe")
            });
        if path.is_file() {
            Some(path)
        } else {
            eprintln!(
                "skipped: no probe at {} (build it with `cargo build -p tl-gpu-probe`)",
                path.display()
            );
            None
        }
    }

    fn spawn(probe: &Path) -> ProbeChild {
        spawn_probe(probe, &[]).expect("spawn probe")
    }

    fn self_fd_count() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .expect("read /proc/self/fd")
            .count()
    }

    #[test]
    fn fd3_is_the_socket() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        let child = spawn(&probe);
        assert!(child.child.id() > 0);

        let mut sock = UnixStream::from(child.sock.try_clone().expect("clone sock"));
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let hello = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        write_frame(&mut sock, &hello, MAX_PROBE_FRAME).expect("write hello");
        let resp: ProbeResponse = read_frame(&mut sock, MAX_PROBE_FRAME).expect("read reply");
        // The real probe answers InitFailed on a machine without a usable NVIDIA driver. Any
        // reply proves the child reads and writes the socket on fd 3.
        match resp {
            ProbeResponse::Hello { proto, .. } => assert_eq!(proto, GPU_PROTO_VERSION),
            ProbeResponse::InitFailed { .. } => {}
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn child_env_is_minimal() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let expected: BTreeSet<Vec<u8>> = child_env()
            .into_iter()
            .map(|(k, v)| {
                let mut entry = k.as_bytes().to_vec();
                entry.push(b'=');
                entry.extend_from_slice(v.as_encoded_bytes());
                entry
            })
            .collect();
        for (key, _) in child_env() {
            assert!(["PATH", "LANG", "RUST_LOG"].contains(&key), "{key}");
        }
        let Some(probe) = test_probe() else { return };
        let child = spawn(&probe);

        let environ = std::fs::read(format!("/proc/{}/environ", child.child.id())).unwrap();
        let got: BTreeSet<Vec<u8>> = environ
            .split(|&b| b == 0)
            .filter(|e| !e.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn no_leaked_fds() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        let child = spawn(&probe);

        // The probe opens nothing before the first Hello, and `spawn` returns after exec.
        let fds: BTreeSet<i32> = std::fs::read_dir(format!("/proc/{}/fd", child.child.id()))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(fds, BTreeSet::from([0, 1, 2, 3]));
    }

    #[test]
    fn drop_kills_and_reaps() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        let child = spawn(&probe);
        let pid = child.child.id();
        let stat = format!("/proc/{pid}/stat");
        assert!(Path::new(&stat).exists());

        drop(child);
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            match std::fs::read_to_string(&stat) {
                Err(_) => break,
                Ok(s) => {
                    let state = s.rsplit_once(") ").map(|(_, rest)| &rest[..1]);
                    assert_ne!(state, Some("Z"), "zombie after drop");
                }
            }
            assert!(
                Instant::now() < deadline,
                "pid {pid} still present after 200 ms"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn missing_binary_leaks_nothing() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let missing = Path::new("/nonexistent/tasklens/tl-gpu-probe");
        // Other test modules open and close fds on their own threads, so one equal count in a
        // few tries passes. A real leak adds two fds on every try.
        let equal = (0..5).any(|_| {
            let before = self_fd_count();
            let r = spawn_probe(missing, &[]);
            assert!(matches!(r, Err(ProbeError::Spawn(_))));
            self_fd_count() == before
        });
        assert!(equal, "socket fds leaked on a failed spawn");
    }
}
