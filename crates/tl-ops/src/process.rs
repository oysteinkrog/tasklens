//! Verified process operations: signals, nice, scheduling, affinity and I/O priority.
//!
//! Every operation first proves the target is still the captured process (ADR-006). It opens a
//! pidfd, which pins the process, then re-reads the start time and compares it with the
//! `ProcessKey`. Signals then go through that pidfd, so they can never hit a reused PID. There is
//! no fallback to `kill(pid)`.

use std::os::fd::{AsFd, OwnedFd};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::process::{Pid, PidfdFlags};
use tl_model::{actions::Signal, ids::ProcessKey, time::BootId};

use crate::error::{OpsError, OpsResult};
use crate::sys;

/// Opens a pidfd for `key` and proves it is the same process: same boot, same pid and the same
/// start time read after the pidfd is open. Any mismatch, or a process that is gone, gives
/// `IdentityChanged`.
pub fn open_verified(key: ProcessKey, boot: BootId) -> OpsResult<OwnedFd> {
    let result = open_verified_inner(key, boot);
    tracing::debug!(pid = key.pid, ok = result.is_ok(), error = ?result.as_ref().err(), "open_verified");
    result
}

fn open_verified_inner(key: ProcessKey, boot: BootId) -> OpsResult<OwnedFd> {
    if key.boot_id != boot || key.pid == 0 {
        return Err(OpsError::IdentityChanged);
    }
    let Some(pid) = i32::try_from(key.pid).ok().and_then(Pid::from_raw) else {
        return Err(OpsError::IdentityChanged);
    };
    let pidfd = match rustix::process::pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(Errno::SRCH) => return Err(OpsError::IdentityChanged),
        Err(e) => return Err(e.into()),
    };
    match read_start_ticks(key.pid) {
        Ok(ticks) if ticks == key.start_ticks => Ok(pidfd),
        Ok(_) | Err(OpsError::Os(libc::ENOENT)) | Err(OpsError::NoSuchProcess) => {
            Err(OpsError::IdentityChanged)
        }
        Err(e) => Err(e),
    }
}

/// Reads the start time (clock ticks since boot) from `/proc/<pid>/stat`. A line that does not
/// parse gives `Os(EIO)`.
pub fn read_start_ticks(pid: u32) -> OpsResult<u64> {
    let path = format!("/proc/{pid}/stat");
    let fd = rustix::fs::open(
        path.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut buf = [0u8; 4096];
    let n = rustix::io::read(&fd, &mut buf)?;
    parse_start_ticks(&buf[..n]).ok_or(OpsError::Os(libc::EIO))
}

/// Field 22 (`starttime`) of a `/proc/<pid>/stat` line. The name in field 2 may hold spaces and
/// `)`, so the scan starts after the last `)`.
pub(crate) fn parse_start_ticks(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    let rest = stat.get(close + 1..)?.strip_prefix(b" ")?;
    let token = rest.split(|&b| b == b' ').nth(19)?;
    let token = token.strip_suffix(b"\n").unwrap_or(token);
    if token.is_empty() || !token.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(token).ok()?.parse().ok()
}

/// True when the process behind `pidfd` has exited (zombie or reaped). Does not block.
// The PID-based calls of E07-004 use this; until then only the tests do.
#[allow(dead_code)]
pub(crate) fn has_exited(pidfd: &OwnedFd) -> OpsResult<bool> {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    let zero = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut fds, Some(&zero))?;
    Ok(fds[0].revents().contains(PollFlags::IN))
}

/// The x86_64 signal number. `Other(n)` must be 1..=64.
pub fn signal_number(sig: Signal) -> OpsResult<i32> {
    match sig {
        Signal::Other(n) if !(1..=64).contains(&n) => {
            Err(OpsError::InvalidInput("signal number out of range"))
        }
        sig => Ok(sig.number()),
    }
}

/// Sends `sig` to the verified process through its pidfd.
pub fn send_signal(key: ProcessKey, boot: BootId, sig: Signal) -> OpsResult {
    let number = signal_number(sig)?;
    let pidfd = open_verified(key, boot)?;
    let result = sys::pidfd_send_signal_raw(pidfd.as_fd(), number).map_err(OpsError::from);
    tracing::debug!(pid = key.pid, signal = number, ok = result.is_ok(), error = ?result.as_ref().err(), "send_signal");
    result
}

/// Stops the process with SIGSTOP.
pub fn suspend(key: ProcessKey, boot: BootId) -> OpsResult {
    send_signal(key, boot, Signal::Stop)
}

/// Continues the process with SIGCONT.
pub fn resume(key: ProcessKey, boot: BootId) -> OpsResult {
    send_signal(key, boot, Signal::Cont)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, ExitStatus};
    use std::sync::MutexGuard;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::sys::tests::lock_children;

    const BOOT: BootId = BootId(0x1234_5678_9abc_def0);

    /// A `sleep 60` child that is killed and reaped on drop, even when an assertion fails.
    struct ChildGuard {
        child: Child,
        _lock: MutexGuard<'static, ()>,
    }

    impl ChildGuard {
        fn sleep() -> Self {
            let lock = lock_children();
            let child = Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn sleep");
            ChildGuard { child, _lock: lock }
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn key(&self) -> ProcessKey {
            ProcessKey {
                start_ticks: read_start_ticks(self.pid()).expect("read start ticks"),
                boot_id: BOOT,
                pid: self.pid(),
            }
        }

        fn wait(&mut self) -> ExitStatus {
            self.child.wait().expect("wait child")
        }

        fn state(&self) -> u8 {
            let stat = std::fs::read(format!("/proc/{}/stat", self.pid())).expect("read stat");
            let close = stat.iter().rposition(|&b| b == b')').expect("no )");
            stat[close + 2]
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Polls `cond` every 5 ms for up to 5 s.
    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn parse_start_ticks_name_with_spaces_and_parens() {
        let line =
            b"1234 (a b) c) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 98765 1000 100";
        assert_eq!(parse_start_ticks(line), Some(98765));
        let with_newline = b"1 (x) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 42\n";
        assert_eq!(parse_start_ticks(with_newline), Some(42));
    }

    #[test]
    fn parse_start_ticks_truncated_is_none() {
        // Cut after field 20 (`num_threads`).
        let line = b"1234 (a b) c) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1";
        assert_eq!(parse_start_ticks(line), None);
        assert_eq!(parse_start_ticks(b"1234 (abc"), None);
        assert_eq!(parse_start_ticks(b""), None);
    }

    #[test]
    fn parse_start_ticks_non_numeric_is_none() {
        let line = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 98x65 1000";
        assert_eq!(parse_start_ticks(line), None);
        let negative = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 -5 1000";
        assert_eq!(parse_start_ticks(negative), None);
        let double_space = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0  98765";
        assert_eq!(parse_start_ticks(double_space), None);
    }

    #[test]
    fn boot_mismatch_is_identity_changed() {
        // Our own process: if the boot check ran after pidfd_open, this key would verify.
        let pid = std::process::id();
        let key = ProcessKey {
            start_ticks: read_start_ticks(pid).expect("read own start ticks"),
            boot_id: BootId(1),
            pid,
        };
        assert_eq!(
            open_verified(key, BootId(2)).err(),
            Some(OpsError::IdentityChanged)
        );
        assert_eq!(
            open_verified(ProcessKey { pid: 0, ..key }, BootId(1)).err(),
            Some(OpsError::IdentityChanged)
        );
    }

    #[test]
    fn open_verified_true_key_opens() {
        let child = ChildGuard::sleep();
        let pidfd = open_verified(child.key(), BOOT).expect("open_verified");
        assert_eq!(has_exited(&pidfd), Ok(false));
    }

    /// Regression test for PID reuse protection: a key whose start time does not match the live
    /// process must never verify.
    #[test]
    fn open_verified_shifted_start_ticks_is_identity_changed() {
        let child = ChildGuard::sleep();
        let mut key = child.key();
        key.start_ticks += 1;
        assert_eq!(
            open_verified(key, BOOT).err(),
            Some(OpsError::IdentityChanged)
        );
        assert_eq!(
            send_signal(key, BOOT, Signal::Kill),
            Err(OpsError::IdentityChanged)
        );
        assert_ne!(child.state(), b'Z', "the signal reached the child");
    }

    #[test]
    fn open_verified_gone_pid_is_identity_changed() {
        let mut child = ChildGuard::sleep();
        let key = child.key();
        child.child.kill().expect("kill child");
        child.wait();
        assert_eq!(
            open_verified(key, BOOT).err(),
            Some(OpsError::IdentityChanged)
        );
    }

    #[test]
    fn signal_numbers_table() {
        let table = [
            (Signal::Term, 15),
            (Signal::Kill, 9),
            (Signal::Hup, 1),
            (Signal::Int, 2),
            (Signal::Quit, 3),
            (Signal::Stop, 19),
            (Signal::Cont, 18),
            (Signal::Usr1, 10),
            (Signal::Usr2, 12),
            (Signal::Other(1), 1),
            (Signal::Other(34), 34),
            (Signal::Other(64), 64),
        ];
        for (sig, number) in table {
            assert_eq!(signal_number(sig), Ok(number), "{sig:?}");
        }
        for n in [0, 65, 255] {
            assert_eq!(
                signal_number(Signal::Other(n)),
                Err(OpsError::InvalidInput("signal number out of range"))
            );
        }
    }

    #[test]
    fn send_term_ends_child() {
        let mut child = ChildGuard::sleep();
        assert_eq!(send_signal(child.key(), BOOT, Signal::Term), Ok(()));
        assert_eq!(child.wait().signal(), Some(15));
    }

    #[test]
    fn suspend_resume_changes_state() {
        let child = ChildGuard::sleep();
        let key = child.key();
        assert_eq!(suspend(key, BOOT), Ok(()));
        assert!(wait_for(|| child.state() == b'T'), "child did not stop");
        assert_eq!(resume(key, BOOT), Ok(()));
        assert!(wait_for(|| child.state() != b'T'), "child did not resume");
    }

    #[test]
    fn realtime_signal_34_delivered() {
        let mut child = ChildGuard::sleep();
        assert_eq!(send_signal(child.key(), BOOT, Signal::Other(34)), Ok(()));
        assert_eq!(child.wait().signal(), Some(34));
    }

    #[test]
    fn has_exited_tracks_child() {
        let child = ChildGuard::sleep();
        let pidfd = open_verified(child.key(), BOOT).expect("open_verified");
        assert_eq!(has_exited(&pidfd), Ok(false));
        sys::pidfd_send_signal_raw(pidfd.as_fd(), libc::SIGKILL).expect("kill child");
        // The child is a zombie until the guard reaps it on drop.
        assert!(
            wait_for(|| has_exited(&pidfd) == Ok(true)),
            "pidfd never became readable"
        );
        assert_eq!(child.state(), b'Z');
    }
}
