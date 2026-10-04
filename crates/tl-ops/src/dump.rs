//! Process memory dumps (PROC-075, R-004, R-145).
//!
//! `gdb` runs by fixed path with `gcore` as a batch command and writes the core straight into a
//! file descriptor the caller opened, so the file belongs to the user and the UI can watch its
//! size grow. Core files hold process memory: they are created with mode `0600`, never read by
//! tasklens and never logged. gdb's output goes to `/dev/null`.

use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::process::{Command, Stdio};

use rustix::fs::{Mode, OFlags};
use tl_model::{actions::DumpMode, ids::ProcessKey, time::BootId};

use crate::error::{OpsError, OpsResult};
use crate::process::open_verified;

pub const GDB_PATH: &str = "/usr/bin/gdb";
pub const GCORE_PATH: &str = "/usr/bin/gcore";
/// The `coredump_filter` value that dumps every mapping type.
pub const FULL_COREDUMP_FILTER: &str = "0x1ff";

const PTRACE_SCOPE_PATH: &str = "/proc/sys/kernel/yama/ptrace_scope";

/// True when both `gcore` and `gdb` exist at their fixed paths.
pub fn dump_available() -> bool {
    dump_available_with(Path::new(GDB_PATH), Path::new(GCORE_PATH))
}

fn dump_available_with(gdb: &Path, gcore: &Path) -> bool {
    gcore.exists() && gdb.exists()
}

/// Creates `out` (it must not exist) with mode `0600` and dumps the process into it. On error the
/// new file is removed.
pub fn dump_to_path(key: ProcessKey, boot: BootId, mode: DumpMode, out: &Path) -> OpsResult {
    let fd = rustix::fs::open(
        out,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let result = dump_to_fd(key, boot, mode, fd);
    if result.is_err() {
        let _ = rustix::fs::unlink(out);
    }
    result
}

/// Dumps the verified process into `out`, which is closed when this returns. Blocks until gdb
/// exits.
pub fn dump_to_fd(key: ProcessKey, boot: BootId, mode: DumpMode, out: OwnedFd) -> OpsResult {
    let gdb = Path::new(GDB_PATH);
    let gcore = Path::new(GCORE_PATH);
    let result = if dump_available_with(gdb, gcore) {
        read_ptrace_scope().and_then(|scope| {
            let euid = rustix::process::geteuid().as_raw();
            dump_to_fd_with(gdb, gcore, scope, euid, key, boot, mode, out)
        })
    } else {
        Err(OpsError::Unsupported("gdb is not installed"))
    };
    tracing::debug!(pid = key.pid, ?mode, ok = result.is_ok(), error = ?result.as_ref().err(), "dump_to_fd");
    result
}

/// `dump_to_fd` with the tool paths, ptrace scope and euid passed in, so tests can use a fake gdb.
#[allow(clippy::too_many_arguments)]
fn dump_to_fd_with(
    gdb: &Path,
    gcore: &Path,
    scope: u8,
    euid: u32,
    key: ProcessKey,
    boot: BootId,
    mode: DumpMode,
    out: OwnedFd,
) -> OpsResult {
    if !dump_available_with(gdb, gcore) {
        return Err(OpsError::Unsupported("gdb is not installed"));
    }
    ptrace_allowed(scope, euid)?;
    let _pidfd = open_verified(key, boot)?;

    let filter_path = format!("/proc/{}/coredump_filter", key.pid);
    let saved_filter = match mode {
        DumpMode::Full => {
            let saved = std::fs::read_to_string(&filter_path)?;
            std::fs::write(&filter_path, FULL_COREDUMP_FILTER)?;
            Some(saved)
        }
        DumpMode::Mini => None,
    };

    let result = run_gdb(gdb, key.pid, &out);

    if let Some(saved) = saved_filter {
        // The kernel prints the filter as bare hex digits with leading zeros but parses with base
        // 0, which would read them as octal, so the restore adds `0x`.
        let restored = std::fs::write(&filter_path, format!("0x{}", saved.trim()));
        // A gdb error wins over a restore error.
        return result.and(restored.map_err(OpsError::from));
    }
    result
}

/// Runs `gdb --nx --nh --batch -p <pid> -ex "gcore /proc/<self>/fd/<n>"` with a cleared
/// environment and every standard stream on `/dev/null`, then checks that a core was written.
fn run_gdb(gdb: &Path, pid: u32, out: &OwnedFd) -> OpsResult {
    let target = format!("gcore /proc/{}/fd/{}", std::process::id(), out.as_raw_fd());
    let status = Command::new(gdb)
        .args(["--nx", "--nh", "--batch", "-p"])
        .arg(pid.to_string())
        .arg("-ex")
        .arg(target)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(OpsError::Os(libc::EIO));
    }
    if rustix::fs::fstat(out)?.st_size == 0 {
        return Err(OpsError::Os(libc::EIO));
    }
    Ok(())
}

/// Reads the Yama ptrace scope. A missing file (no Yama) gives 0, text that does not parse gives
/// `Os(EIO)`.
fn read_ptrace_scope() -> OpsResult<u8> {
    match std::fs::read_to_string(PTRACE_SCOPE_PATH) {
        Ok(text) => text.trim().parse().map_err(|_| OpsError::Os(libc::EIO)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

/// Whether gdb may attach under Yama `scope` for a caller with `euid`. Reads nothing.
pub(crate) fn ptrace_allowed(scope: u8, euid: u32) -> OpsResult {
    match (scope, euid) {
        (3.., _) => Err(OpsError::Unsupported("ptrace is disabled on this system")),
        (_, 0) | (0, _) => Ok(()),
        _ => Err(OpsError::PermissionDenied),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;
    use std::process::Child;
    use std::sync::MutexGuard;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::process::read_start_ticks;
    use crate::sys::tests::lock_children;

    const BOOT: BootId = BootId(0x1234_5678_9abc_def0);

    /// A fresh directory under the system temp dir, removed on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("tl-ops-dump-test-{}-{n}", std::process::id()));
            std::fs::create_dir(&dir).expect("create test dir");
            TestDir(dir)
        }

        /// Writes an executable `/bin/sh` script and returns its path.
        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("chmod script");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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

        fn key(&self) -> ProcessKey {
            let pid = self.child.id();
            ProcessKey {
                start_ticks: read_start_ticks(pid).expect("read start ticks"),
                boot_id: BOOT,
                pid,
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn open_out(path: &Path) -> OwnedFd {
        rustix::fs::open(
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .expect("open output")
    }

    #[test]
    fn ptrace_allowed_table() {
        let unsupported = Err(OpsError::Unsupported("ptrace is disabled on this system"));
        assert_eq!(ptrace_allowed(1, 1000), Err(OpsError::PermissionDenied));
        assert_eq!(ptrace_allowed(2, 1000), Err(OpsError::PermissionDenied));
        assert_eq!(ptrace_allowed(0, 1000), Ok(()));
        assert_eq!(ptrace_allowed(0, 0), Ok(()));
        assert_eq!(ptrace_allowed(1, 0), Ok(()));
        assert_eq!(ptrace_allowed(2, 0), Ok(()));
        assert_eq!(ptrace_allowed(3, 0), unsupported);
        assert_eq!(ptrace_allowed(3, 1000), unsupported);
    }

    #[test]
    fn dump_to_path_refuses_existing_file() {
        let dir = TestDir::new();
        let out = dir.0.join("existing.core");
        std::fs::write(&out, b"keep me").expect("write existing file");
        let key = ProcessKey {
            start_ticks: 1,
            boot_id: BOOT,
            pid: std::process::id(),
        };
        assert_eq!(
            dump_to_path(key, BOOT, DumpMode::Mini, &out),
            Err(OpsError::Os(libc::EEXIST))
        );
        assert_eq!(std::fs::read(&out).expect("read existing file"), b"keep me");
    }

    #[test]
    fn missing_gdb_is_unsupported() {
        let dir = TestDir::new();
        let child = ChildGuard::sleep();
        let out = open_out(&dir.0.join("out.core"));
        let missing = dir.0.join("no-gdb");
        let result = dump_to_fd_with(
            &missing,
            Path::new("/bin/sh"),
            0,
            1000,
            child.key(),
            BOOT,
            DumpMode::Mini,
            out,
        );
        assert_eq!(result, Err(OpsError::Unsupported("gdb is not installed")));
    }

    #[test]
    fn shifted_key_is_identity_changed() {
        let dir = TestDir::new();
        let child = ChildGuard::sleep();
        let marker = dir.0.join("gdb-ran");
        let gdb = dir.script("gdb", &format!(": > '{}'\nexit 1", marker.display()));
        let mut key = child.key();
        key.start_ticks += 1;
        let out = open_out(&dir.0.join("out.core"));
        let result = dump_to_fd_with(&gdb, &gdb, 0, 1000, key, BOOT, DumpMode::Full, out);
        assert_eq!(result, Err(OpsError::IdentityChanged));
        assert!(!marker.exists(), "gdb was started");
    }

    #[test]
    fn scope1_user_is_permission_denied_and_cleans_up() {
        if read_ptrace_scope() != Ok(1) || rustix::process::geteuid().is_root() || !dump_available()
        {
            eprintln!("skipped: needs ptrace_scope 1, a normal user and gdb");
            return;
        }
        let dir = TestDir::new();
        let child = ChildGuard::sleep();
        let out = dir.0.join("denied.core");
        assert_eq!(
            dump_to_path(child.key(), BOOT, DumpMode::Mini, &out),
            Err(OpsError::PermissionDenied)
        );
        assert!(!out.exists(), "the empty output file was left behind");
    }

    #[test]
    fn full_mode_restores_filter_on_failure() {
        let dir = TestDir::new();
        let child = ChildGuard::sleep();
        let gdb = dir.script("gdb", "exit 1");
        let key = child.key();
        let filter_path = format!("/proc/{}/coredump_filter", key.pid);
        let before = std::fs::read_to_string(&filter_path).expect("read filter");
        assert_ne!(
            before.trim(),
            "000001ff",
            "the test needs a filter other than 0x1ff"
        );

        let out_path = dir.0.join("out.core");
        let out = open_out(&out_path);
        let result = dump_to_fd_with(&gdb, &gdb, 0, 1000, key, BOOT, DumpMode::Full, out);

        assert_eq!(result, Err(OpsError::Os(libc::EIO)));
        assert_eq!(
            std::fs::read_to_string(&filter_path).expect("read filter"),
            before
        );
        let meta = std::fs::metadata(&out_path).expect("stat output");
        assert_eq!(meta.mode() & 0o777, 0o600);
    }
}
