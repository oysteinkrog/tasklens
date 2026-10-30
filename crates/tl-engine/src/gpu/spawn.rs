//! Spawns the `tl-gpu-probe` child over a socketpair (ADR-007 "Spawn", E13-008).
//!
//! The child gets its end of a `SOCK_STREAM` socketpair on fd 3, a cleared environment and
//! `SIGKILL` as its parent-death signal, so a crashed engine never leaves a probe behind.
//! `asupersync::process` is never used here (zombie bug `bi2462.114`).
//!
//! This is the only file in `tl-engine` allowed to contain `unsafe` (ADR-001 rule 4). The
//! `unsafe` APIs are `CommandExt::pre_exec` and the `libc` calls that mark fds above 3
//! close-on-exec in the child (R-290).

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

use rustix::io::{Errno, FdFlags, dup2, fcntl_setfd};
use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
use rustix::process::{
    Pid, Signal, WaitOptions, getpid, getppid, set_parent_process_death_signal, waitpid,
};

use super::ProbeError;

/// The fd that holds the socket in the child.
pub const CHILD_FD: RawFd = 3;

/// Environment variable whose value the child gets as `RUST_LOG` (ADR-015).
const LOG_ENV: &str = "TASKLENS_LOG";

/// Name of the short-lived thread that reaps a killed probe.
pub const REAP_THREAD_NAME: &str = "tl-gpu-reap";

/// A running probe child and the parent end of its socket.
///
/// The child has exactly one owner at any time (tasklens-cr0.30). [`ProbeChild::give_up`] and
/// the drop move it by value into the reaper, so no code here can signal or wait on its pid
/// after that, and a reused pid is never hit. Neither ever waits for the child.
pub struct ProbeChild {
    /// `None` once the child is given up.
    child: Option<Child>,
    pid: u32,
    /// Parent end of the socketpair, `CLOEXEC`.
    pub sock: OwnedFd,
    /// The wait the reaper runs. Tests replace it to make a child slow to exit.
    reap_wait: WaitFn,
}

impl ProbeChild {
    /// The child's pid, also after it was given up.
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// The exit status if the child has exited, without blocking. `None` while it runs, after
    /// it was given up, or when the check fails.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.as_mut()?.try_wait().ok().flatten()
    }

    /// Gives the child up: a child that still runs gets `SIGKILL` and is reaped on a
    /// background thread. Does nothing the second time.
    pub fn give_up(&mut self) {
        if let Some(child) = self.child.take() {
            reap_with(child, self.reap_wait);
        }
    }

    /// Replaces the reaper's wait, so a test can make the child slow to exit.
    #[cfg(test)]
    pub(crate) fn set_reap_wait(&mut self, wait: WaitFn) {
        self.reap_wait = wait;
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        self.give_up();
    }
}

/// A blocking wait for one child, as `waitpid(pid, 0)`.
pub(crate) type WaitFn = fn(Pid) -> Result<(), Errno>;

/// `waitpid(pid, 0)`, the reaper's wait outside tests.
fn wait_pid(pid: Pid) -> Result<(), Errno> {
    waitpid(Some(pid), WaitOptions::empty()).map(drop)
}

/// Takes a child the caller gives up, sends it `SIGKILL` if it still runs and reaps it on a
/// short-lived thread, [`REAP_THREAD_NAME`]. A killed probe can take seconds to exit while the
/// driver tears down its GPU context (3.6 s measured for `--api-versions` on the office
/// desktop), so neither a cancelled job nor the engine thread ever waits for it
/// (tasklens-cr0.30). The child moves into the thread, so the caller cannot touch its pid again.
pub fn kill_and_reap_in_background(child: Child) {
    reap_with(child, wait_pid);
}

/// [`kill_and_reap_in_background`] with the wait `wait`. Returns the reaper thread, or `None`
/// when the child had already exited or the thread could not start.
fn reap_with(mut child: Child, wait: WaitFn) -> Option<JoinHandle<()>> {
    // `try_wait` reaps a child that has exited, and `Err` means it is not ours to wait on.
    if !matches!(child.try_wait(), Ok(None)) {
        return None;
    }
    let _ = child.kill();
    let pid = child.id();
    let reaper = std::thread::Builder::new()
        .name(REAP_THREAD_NAME.to_owned())
        .spawn(move || reap(child, wait));
    match reaper {
        Ok(handle) => Some(handle),
        Err(e) => {
            // The child moved into the closure that failed to start and is gone, so the pid
            // stays a zombie until this process exits. Waiting here instead would block the
            // caller for as long as the driver takes.
            tracing::warn!(pid, error = %e, "GPU probe left unreaped: no reaper thread");
            None
        }
    }
}

/// Waits for the killed `child` until it is reaped, retrying on `EINTR`. Any other error is
/// logged and ends the wait.
fn reap(child: Child, mut wait: impl FnMut(Pid) -> Result<(), Errno>) {
    let pid = Pid::from_child(&child);
    loop {
        match wait(pid) {
            Ok(()) => return,
            Err(Errno::INTR) => {}
            Err(e) => {
                tracing::warn!(pid = child.id(), error = %e, "GPU probe reap failed");
                return;
            }
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
        pid: child.id(),
        child: Some(child),
        sock: parent,
        reap_wait: wait_pid,
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
    // `prctl`, `getppid`) and libc (`close_range`, `fcntl`), allocates nothing (errors are
    // `from_raw_os_error` or `last_os_error`, both plain integers) and touches no locks. The
    // closure captures two integers.
    unsafe {
        cmd.pre_exec(move || child_setup(child_raw, parent_pid));
    }
}

/// Runs in the child between fork and exec: moves `child_raw` to fd 3 without `CLOEXEC`, marks
/// every fd above 3 `CLOEXEC`, sets the parent-death signal to `SIGKILL`, and fails if the
/// parent `parent_pid` is already gone.
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
    cloexec_above_fd3();
    set_parent_process_death_signal(Some(Signal::KILL))?;
    // The parent may have died between fork and prctl, and then the signal never comes.
    if getppid().map(|p| p.as_raw_nonzero().get()) != Some(parent_pid) {
        return Err(Errno::SRCH.into());
    }
    Ok(())
}

/// Highest fd the fallback in [`cloexec_above_fd3`] reaches.
const FALLBACK_MAX_FD: libc::c_int = 1024;

/// Marks every fd above 3 `CLOEXEC`, so exec closes them all and the probe starts with 0 to 3
/// only. An fd that another thread of this process opened without `CLOEXEC` (zbus receives
/// D-Bus fds that way, R-290) would otherwise leak into the probe.
///
/// The flag is `CLOSE_RANGE_CLOEXEC`, not a plain close: std reports an exec failure to the
/// parent over a `CLOEXEC` pipe above fd 3, and closing it would turn a missing binary into a
/// spawn that looks successful. Kernels before 5.11 reject the flag (`EINVAL`) or the call
/// (`ENOSYS`); then each fd from 4 to [`FALLBACK_MAX_FD`] gets `FD_CLOEXEC` one by one, and
/// `EBADF` for a closed fd is ignored.
#[allow(unsafe_code)]
fn cloexec_above_fd3() {
    let first = (CHILD_FD + 1).cast_unsigned();
    // SAFETY: close_range only changes fd flags in this process and is async-signal-safe. It
    // reads no memory of ours.
    let r = unsafe { libc::close_range(first, u32::MAX, libc::CLOSE_RANGE_CLOEXEC as libc::c_int) };
    if r == 0 {
        return;
    }
    for fd in CHILD_FD + 1..=FALLBACK_MAX_FD {
        // SAFETY: fcntl(F_SETFD) on a raw number is async-signal-safe and changes only that
        // fd's flags; a number that is not open fails with EBADF and changes nothing.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
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
        assert!(child.id() > 0);
        hello(&child);
    }

    /// Sends `Hello` and checks the reply. The real probe answers `InitFailed` on a machine
    /// without a usable NVIDIA driver. Any reply proves the child reads and writes the socket on
    /// fd 3, and that its exec has finished.
    fn hello(child: &ProbeChild) {
        let mut sock = UnixStream::from(child.sock.try_clone().expect("clone sock"));
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let hello = ProbeRequest::Hello {
            proto: GPU_PROTO_VERSION,
        };
        write_frame(&mut sock, &hello, MAX_PROBE_FRAME).expect("write hello");
        let resp: ProbeResponse = read_frame(&mut sock, MAX_PROBE_FRAME).expect("read reply");
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
        // `spawn` returns when exec closes the CLOEXEC fds, before the kernel has loaded the
        // new image, and until then `environ` reads empty. The reply comes after.
        hello(&child);

        let environ = std::fs::read(format!("/proc/{}/environ", child.id())).unwrap();
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
        assert_only_own_fds(&child);
    }

    #[test]
    fn inheritable_fd_does_not_reach_probe() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        // An fd with CLOEXEC cleared, as zbus holds a received D-Bus fd for a moment (R-290).
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        fcntl_setfd(&file, FdFlags::empty()).expect("clear CLOEXEC");
        let child = spawn(&probe);
        assert_only_own_fds(&child);
        drop(file);
    }

    /// Asserts that the probe holds fds 0 to 3 and nothing else of the test process.
    fn assert_only_own_fds(child: &ProbeChild) {
        // `spawn` returns after execve, so any fd the test process held without CLOEXEC is in
        // the child now, and the probe never closes it. The probe also opens fds of its own:
        // ld.so opens each shared library and closes it again, and `open_fd` in tl-gpu-probe
        // opens a pidfd for a moment and keeps a copy of fd 3. So an extra fd is fine when it
        // is the same socket as fd 3 or when it closes within 1 s.
        let pid = child.id();
        let target = |fd: i32| {
            std::fs::read_link(format!("/proc/{pid}/fd/{fd}"))
                .map(|t| t.to_string_lossy().into_owned())
                .ok()
        };
        let socket = target(CHILD_FD).expect("probe has fd 3");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let fds: BTreeSet<i32> = std::fs::read_dir(format!("/proc/{pid}/fd"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_str().unwrap().parse().unwrap())
                .collect();
            assert!(fds.is_superset(&BTreeSet::from([0, 1, 2, 3])), "{fds:?}");
            let extra: Vec<i32> = fds
                .into_iter()
                .filter(|&fd| fd > CHILD_FD)
                .filter(|&fd| target(fd).is_some_and(|t| t != socket))
                .collect();
            if extra.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                let lines: Vec<String> = extra
                    .iter()
                    .map(|&fd| describe_leaked_fd(pid, fd))
                    .collect();
                panic!(
                    "probe has fds that are not 0 to 2 or its own socket {socket}; a test opened \
                     these without CLOEXEC:\n{}",
                    lines.join("\n")
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// One line for a leaked fd: its link target, the fds in this process with the same target
    /// and, for a socket, its `/proc/net/unix` line.
    fn describe_leaked_fd(pid: u32, fd: i32) -> String {
        let target = match std::fs::read_link(format!("/proc/{pid}/fd/{fd}")) {
            Ok(t) => t.to_string_lossy().into_owned(),
            Err(e) => return format!("  fd {fd}: readlink failed: {e}"),
        };
        // The fd may be closed here by now, so an empty list does not clear the suspect.
        let ours: Vec<String> = std::fs::read_dir("/proc/self/fd")
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.to_string_lossy() == target))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let unix = target
            .strip_prefix("socket:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|inode| {
                let table = std::fs::read_to_string("/proc/self/net/unix").ok()?;
                table
                    .lines()
                    .find(|l| l.split_whitespace().nth(6) == Some(inode))
                    .map(str::to_owned)
            });
        format!(
            "  fd {fd}: {target}; same target in test process fds {ours:?}; unix: {}",
            unix.as_deref().unwrap_or("-")
        )
    }

    #[test]
    fn drop_kills_and_reaps() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        let child = spawn(&probe);
        let pid = child.id();
        let stat = format!("/proc/{pid}/stat");
        assert!(Path::new(&stat).exists());

        drop(child);
        // The reap runs on a background thread, so the child may be a zombie for a moment.
        // Within 200 ms its /proc entry is gone.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Path::new(&stat).exists() {
            assert!(
                Instant::now() < deadline,
                "pid {pid} still present after 200 ms"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A killed `sleep 30`, not yet reaped.
    fn killed_child() -> (Child, u32) {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        child.kill().expect("kill sleep");
        let pid = child.id();
        (child, pid)
    }

    #[test]
    fn reap_retries_eintr() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (child, pid) = killed_child();
        let mut calls = 0;
        reap(child, |pid| {
            calls += 1;
            if calls <= 2 {
                Err(Errno::INTR)
            } else {
                wait_pid(pid)
            }
        });
        assert_eq!(calls, 3);
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "pid {pid} not reaped"
        );
    }

    #[test]
    fn reap_ends_on_other_errors() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (child, _) = killed_child();
        let mut calls = 0;
        reap(child, |pid| {
            calls += 1;
            // Reap it for real, so no zombie is left, then report a failed wait.
            wait_pid(pid)?;
            Err(Errno::CHILD)
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn give_up_twice_is_harmless() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(probe) = test_probe() else { return };
        let mut child = spawn(&probe);
        child.give_up();
        child.give_up();
        assert!(child.exited().is_none());
    }

    #[test]
    fn missing_binary_leaks_nothing() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let missing = Path::new("/nonexistent/tasklens/tl-gpu-probe");
        // Other test modules open and close fds on their own threads, so one equal count in
        // 50 tries passes. A real leak adds two fds on every try.
        let equal = (0..50).any(|_| {
            let before = self_fd_count();
            let r = spawn_probe(missing, &[]);
            assert!(matches!(r, Err(ProbeError::Spawn(_))));
            self_fd_count() == before
        });
        assert!(equal, "socket fds leaked on a failed spawn");
    }
}
