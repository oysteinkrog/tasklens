#![allow(unsafe_code)]
//! Raw syscall wrappers. The only file in this crate allowed to use `unsafe`.
//!
//! `rustix` has no `ioprio_set`, `sched_setattr`, `tgkill` or `posix_spawnp` with
//! `POSIX_SPAWN_SETSID`, so these four live here behind safe functions (ADR-001 rule 4, ADR-006,
//! SYS-023). Errors are the raw `Errno`; callers map it to `OpsError`. Never log `argv` or `cwd`
//! (ADR-015).

// The callers (E07-003, E07-005, E07-014) land in later beads, and some constants exist for
// completeness of the kernel ABI.
#![allow(dead_code)]

use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, BorrowedFd};

use rustix::io::Errno;

pub(crate) const IOPRIO_WHO_PROCESS: i32 = 1;
pub(crate) const IOPRIO_CLASS_SHIFT: u16 = 13;
pub(crate) const IOPRIO_CLASS_NONE: u16 = 0;
pub(crate) const IOPRIO_CLASS_RT: u16 = 1;
pub(crate) const IOPRIO_CLASS_BE: u16 = 2;
pub(crate) const IOPRIO_CLASS_IDLE: u16 = 3;

/// The kernel ioprio value for an I/O class and a level from 0 to 7.
pub(crate) const fn ioprio_value(class: u16, level: u16) -> u16 {
    (class << IOPRIO_CLASS_SHIFT) | level
}

pub(crate) const SCHED_OTHER: u32 = 0;
pub(crate) const SCHED_FIFO: u32 = 1;
pub(crate) const SCHED_RR: u32 = 2;
pub(crate) const SCHED_BATCH: u32 = 3;
pub(crate) const SCHED_IDLE: u32 = 5;
pub(crate) const SCHED_FLAG_RESET_ON_FORK: u64 = 0x01;
pub(crate) const SCHED_ATTR_SIZE_VER0: u32 = 48;

/// Kernel `struct sched_attr`, version 0 (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SchedAttr {
    /// Always `SCHED_ATTR_SIZE_VER0`.
    pub size: u32,
    pub sched_policy: u32,
    pub sched_flags: u64,
    pub sched_nice: i32,
    pub sched_priority: u32,
    /// Always 0.
    pub sched_runtime: u64,
    /// Always 0.
    pub sched_deadline: u64,
    /// Always 0.
    pub sched_period: u64,
}

/// The errno of the last failed libc call on this thread.
fn last_errno() -> Errno {
    Errno::from_raw_os_error(
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO),
    )
}

/// Maps a raw `syscall` return value to `Ok` or the thread's errno.
fn check_syscall(ret: libc::c_long) -> Result<(), Errno> {
    if ret == -1 {
        let errno = last_errno();
        tracing::debug!(errno = errno.raw_os_error(), "syscall failed");
        Err(errno)
    } else {
        Ok(())
    }
}

/// Maps a `posix_spawn*` return value (the error number itself, 0 on success).
fn check_rc(rc: libc::c_int) -> Result<(), Errno> {
    if rc == 0 {
        Ok(())
    } else {
        tracing::debug!(errno = rc, "posix_spawn call failed");
        Err(Errno::from_raw_os_error(rc))
    }
}

/// Sets the I/O priority of one thread (`IOPRIO_WHO_PROCESS` takes a TID).
pub(crate) fn ioprio_set_tid(tid: u32, value: u16) -> Result<(), Errno> {
    // SAFETY: ioprio_set takes three integers and no pointers, so no memory is touched.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_ioprio_set,
            IOPRIO_WHO_PROCESS,
            tid as libc::c_int,
            value as libc::c_int,
        )
    };
    check_syscall(ret)
}

/// The I/O priority of one thread (`IOPRIO_WHO_PROCESS` takes a TID), as the raw kernel value.
pub fn ioprio_get_tid(tid: u32) -> Result<u16, Errno> {
    // SAFETY: ioprio_get takes two integers and no pointers, so no memory is touched.
    let ret =
        unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, tid as libc::c_int) };
    check_syscall(ret)?;
    // The kernel value is 16 bits: class in bits 13 to 15, hint and level below.
    Ok(ret as u16)
}

/// Sets the scheduling policy, flags, nice and priority of one thread.
pub(crate) fn sched_setattr_tid(tid: u32, attr: &SchedAttr) -> Result<(), Errno> {
    debug_assert_eq!(attr.size, SCHED_ATTR_SIZE_VER0);
    // SAFETY: `attr` points to a live, initialised `#[repr(C)]` `SchedAttr` of 48 bytes, which
    // matches the `size` field the kernel reads. The kernel only reads it, and the borrow lasts
    // until the call returns.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_setattr,
            tid as libc::pid_t,
            attr as *const SchedAttr,
            0u32,
        )
    };
    check_syscall(ret)
}

/// Sends `sig` to thread `tid` of thread group `pid`.
pub(crate) fn tgkill(pid: u32, tid: u32, sig: i32) -> Result<(), Errno> {
    // SAFETY: tgkill takes three integers and no pointers, so no memory is touched.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_tgkill,
            pid as libc::pid_t,
            tid as libc::pid_t,
            sig,
        )
    };
    check_syscall(ret)
}

/// Sends `sig` (1..=64, checked by the caller) to the process behind `pidfd`. The raw syscall
/// takes real-time signal numbers that `rustix::process::Signal` cannot name.
pub(crate) fn pidfd_send_signal_raw(pidfd: BorrowedFd<'_>, sig: i32) -> Result<(), Errno> {
    // SAFETY: `pidfd` is a live borrowed descriptor for the duration of the call. The `info`
    // pointer is null, which the kernel accepts and treats as a plain `kill`-style signal, and
    // flags are 0. No memory is read or written.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            sig,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    };
    check_syscall(ret)
}

/// The reverse DNS name of `ip` (SYS-035), through `getnameinfo` with `NI_NAMEREQD`. `Ok(None)`
/// when the address has no name. Loopback, unspecified and link-local addresses return
/// `Ok(None)` without a lookup: their names are local or need a scope id. This call blocks for
/// as long as the resolver takes, so run it on the blocking pool. Never log the name (SYS-035).
pub fn reverse_lookup(ip: IpAddr) -> Result<Option<String>, Errno> {
    let local = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_unicast_link_local(),
    };
    if local {
        return Ok(None);
    }
    let mut storage = MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let len = match ip {
        IpAddr::V4(v4) => {
            let sin = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: 0,
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.octets()),
                },
                sin_zero: [0; 8],
            };
            // SAFETY: `sockaddr_storage` is larger than and aligned for `sockaddr_in`.
            unsafe { storage.as_mut_ptr().cast::<libc::sockaddr_in>().write(sin) };
            std::mem::size_of::<libc::sockaddr_in>()
        }
        IpAddr::V6(v6) => {
            let sin6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: 0,
                sin6_flowinfo: 0,
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.octets(),
                },
                sin6_scope_id: 0,
            };
            // SAFETY: `sockaddr_storage` is larger than and aligned for `sockaddr_in6`.
            unsafe {
                storage
                    .as_mut_ptr()
                    .cast::<libc::sockaddr_in6>()
                    .write(sin6)
            };
            std::mem::size_of::<libc::sockaddr_in6>()
        }
    };
    let mut host = [0 as libc::c_char; libc::NI_MAXHOST as usize];
    // SAFETY: `storage` holds an initialised `sockaddr_in` or `sockaddr_in6` of `len` bytes, and
    // `host` is writable storage of `NI_MAXHOST` bytes. The service buffer is null with length
    // 0, which getnameinfo accepts. Both buffers live until the call returns.
    let rc = unsafe {
        libc::getnameinfo(
            storage.as_ptr().cast::<libc::sockaddr>(),
            len as libc::socklen_t,
            host.as_mut_ptr(),
            libc::NI_MAXHOST,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    match rc {
        0 => {
            // SAFETY: on success getnameinfo wrote a NUL-terminated string into `host`.
            let name = unsafe { CStr::from_ptr(host.as_ptr()) };
            Ok(Some(name.to_string_lossy().into_owned()))
        }
        libc::EAI_NONAME => Ok(None),
        libc::EAI_AGAIN => Err(Errno::AGAIN),
        libc::EAI_MEMORY => Err(Errno::NOMEM),
        libc::EAI_SYSTEM => Err(last_errno()),
        other => {
            tracing::debug!(rc = other, "getnameinfo failed");
            Err(Errno::IO)
        }
    }
}

/// Destroys an initialised `posix_spawnattr_t` on drop.
struct AttrGuard(*mut libc::posix_spawnattr_t);

impl Drop for AttrGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is only built after `posix_spawnattr_init` succeeded on this object,
        // the object outlives the guard, and it is destroyed exactly once, here.
        unsafe { libc::posix_spawnattr_destroy(self.0) };
    }
}

/// Destroys an initialised `posix_spawn_file_actions_t` on drop.
struct FileActionsGuard(*mut libc::posix_spawn_file_actions_t);

impl Drop for FileActionsGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is only built after `posix_spawn_file_actions_init` succeeded on
        // this object, the object outlives the guard, and it is destroyed exactly once, here.
        unsafe { libc::posix_spawn_file_actions_destroy(self.0) };
    }
}

/// Starts `argv[0]` (searched in `PATH`) in a new session with stdin, stdout and stderr on
/// `/dev/null`, default signal dispositions, an empty signal mask, `cwd` when given, and `envp`
/// as its whole environment (`KEY=value` entries), or the current environment when `envp` is
/// `None`. Returns the child pid. The caller reaps the child.
pub(crate) fn spawn_setsid(
    argv: &[CString],
    cwd: Option<&CStr>,
    envp: Option<&[CString]>,
) -> Result<u32, Errno> {
    let Some(file) = argv.first() else {
        return Err(Errno::INVAL);
    };
    let argv_ptrs: Vec<*mut libc::c_char> = argv
        .iter()
        .map(|arg| arg.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    let envp_ptrs: Option<Vec<*mut libc::c_char>> = envp.map(|entries| {
        entries
            .iter()
            .map(|entry| entry.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect()
    });

    // The objects stay in these locals and are used only through raw pointers, so they never
    // move after init. Each guard is declared after its storage and drops first.
    let mut attr_storage = MaybeUninit::<libc::posix_spawnattr_t>::uninit();
    let attr = attr_storage.as_mut_ptr();
    // SAFETY: `attr` is valid writable storage for a `posix_spawnattr_t`; init fills it in.
    check_rc(unsafe { libc::posix_spawnattr_init(attr) })?;
    let _attr_guard = AttrGuard(attr);

    let mut empty = MaybeUninit::<libc::sigset_t>::uninit();
    let mut all = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: both pointers are valid writable storage for a `sigset_t`; these calls fill it in
    // and cannot fail for a valid pointer.
    let (empty, all) = unsafe {
        libc::sigemptyset(empty.as_mut_ptr());
        libc::sigfillset(all.as_mut_ptr());
        (empty.assume_init(), all.assume_init())
    };
    // libc types SETSID as `c_short` and the other flags as `c_int`; all fit in `c_short`.
    let flags = libc::POSIX_SPAWN_SETSID
        | (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as libc::c_short;
    // SAFETY: `attr` is an initialised attribute object and the sigset pointers point to
    // initialised `sigset_t` values that live until these calls return; glibc copies them.
    unsafe {
        check_rc(libc::posix_spawnattr_setflags(attr, flags))?;
        check_rc(libc::posix_spawnattr_setsigmask(attr, &empty))?;
        check_rc(libc::posix_spawnattr_setsigdefault(attr, &all))?;
    }

    let mut actions_storage = MaybeUninit::<libc::posix_spawn_file_actions_t>::uninit();
    let actions = actions_storage.as_mut_ptr();
    // SAFETY: `actions` is valid writable storage for a `posix_spawn_file_actions_t`.
    check_rc(unsafe { libc::posix_spawn_file_actions_init(actions) })?;
    let _actions_guard = FileActionsGuard(actions);

    let dev_null = c"/dev/null";
    // SAFETY: `actions` is an initialised file-action object. `dev_null` and `cwd` are
    // NUL-terminated strings; glibc copies them into the object before returning.
    unsafe {
        check_rc(libc::posix_spawn_file_actions_addopen(
            actions,
            0,
            dev_null.as_ptr(),
            libc::O_RDONLY,
            0,
        ))?;
        check_rc(libc::posix_spawn_file_actions_addopen(
            actions,
            1,
            dev_null.as_ptr(),
            libc::O_WRONLY,
            0,
        ))?;
        check_rc(libc::posix_spawn_file_actions_addopen(
            actions,
            2,
            dev_null.as_ptr(),
            libc::O_WRONLY,
            0,
        ))?;
        if let Some(cwd) = cwd {
            check_rc(libc::posix_spawn_file_actions_addchdir_np(
                actions,
                cwd.as_ptr(),
            ))?;
        }
    }

    let mut pid: libc::pid_t = 0;
    // SAFETY: `file` and every `argv_ptrs` entry point into the `CString`s of `argv`, which live
    // until this call returns, and `argv_ptrs` ends in a null pointer. The same holds for
    // `envp_ptrs` and the `CString`s of `envp`. The child only reads them. `environ` is the
    // process environment as a null-terminated array; tasklens does not call `setenv` after
    // startup, so it is not changed during the call. `attr` and `actions` are initialised
    // objects. `pid` is valid writable storage.
    let rc = unsafe {
        let env: *const *mut libc::c_char = match &envp_ptrs {
            Some(ptrs) => ptrs.as_ptr(),
            None => libc::environ,
        };
        libc::posix_spawnp(
            &mut pid,
            file.as_ptr(),
            actions,
            attr,
            argv_ptrs.as_ptr(),
            env,
        )
    };
    check_rc(rc)?;
    Ok(pid as u32)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;

    /// Held by every test in this crate that starts a child, so `waitpid(-1, ..)` in one test
    /// cannot reap or see another test's child.
    static CHILDREN: Mutex<()> = Mutex::new(());

    pub(crate) fn lock_children() -> std::sync::MutexGuard<'static, ()> {
        CHILDREN.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn own_tid() -> u32 {
        rustix::thread::gettid().as_raw_nonzero().get() as u32
    }

    #[test]
    fn sched_attr_is_48_bytes() {
        assert_eq!(std::mem::size_of::<SchedAttr>(), 48);
        assert_eq!(
            std::mem::size_of::<SchedAttr>(),
            SCHED_ATTR_SIZE_VER0 as usize
        );
    }

    #[test]
    fn ioprio_value_encoding() {
        assert_eq!(ioprio_value(IOPRIO_CLASS_BE, 4), 0x4004);
        assert_eq!(ioprio_value(IOPRIO_CLASS_IDLE, 0), 0x6000);
        assert_eq!(ioprio_value(IOPRIO_CLASS_RT, 0), 0x2000);
        assert_eq!(ioprio_value(IOPRIO_CLASS_NONE, 0), 0);
    }

    #[test]
    fn ioprio_set_own_thread_roundtrip() {
        let tid = own_tid();
        let old = ioprio_get_tid(tid).expect("ioprio_get");
        ioprio_set_tid(tid, ioprio_value(IOPRIO_CLASS_BE, 7)).expect("ioprio_set");
        let now = ioprio_get_tid(tid);
        ioprio_set_tid(tid, old).expect("restore ioprio");
        assert_eq!(now, Ok(0x4007));
    }

    #[test]
    fn tgkill_child_terminates() {
        let _children = lock_children();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let sent = tgkill(pid, pid, libc::SIGTERM);
        let status = child.wait().expect("wait sleep");
        assert_eq!(sent, Ok(()));
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn tgkill_missing_tid_is_esrch() {
        assert_eq!(tgkill(std::process::id(), 0x7fff_fff0, 0), Err(Errno::SRCH));
    }

    #[test]
    fn spawn_setsid_new_session_devnull_cwd() {
        let _children = lock_children();
        let argv = [c"sleep".to_owned(), c"30".to_owned()];
        let pid = spawn_setsid(&argv, Some(c"/tmp"), None).expect("spawn sleep");

        // SAFETY: getsid takes an integer and no pointers.
        let sid = unsafe { libc::getsid(pid as libc::pid_t) };
        let proc_dir = Path::new("/proc").join(pid.to_string());
        let cwd = std::fs::read_link(proc_dir.join("cwd"));
        let fds: Vec<_> = (0..3)
            .map(|fd| std::fs::read_link(proc_dir.join("fd").join(fd.to_string())))
            .collect();

        // SAFETY: kill takes integers only. `pid` is our own unreaped child, so it cannot have
        // been reused by another process.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        let mut status = 0;
        // SAFETY: `status` is valid writable storage for an int.
        let reaped = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
        assert_eq!(reaped, pid as libc::pid_t);

        assert_eq!(sid, pid as libc::pid_t);
        assert_eq!(
            cwd.expect("read cwd link"),
            std::fs::canonicalize("/tmp").expect("canonicalize /tmp")
        );
        for fd in fds {
            assert_eq!(fd.expect("read fd link"), Path::new("/dev/null"));
        }
    }

    #[test]
    fn reverse_lookup_skips_local_addresses() {
        // None of these reach the resolver, so the test sends no DNS query.
        for ip in [
            "127.0.0.1",
            "0.0.0.0",
            "169.254.1.2",
            "::1",
            "::",
            "fe80::1",
        ] {
            let ip: IpAddr = ip.parse().expect("address parses");
            assert_eq!(reverse_lookup(ip), Ok(None), "{ip}");
        }
    }

    #[test]
    fn spawn_setsid_empty_argv_is_einval() {
        assert_eq!(spawn_setsid(&[], None, None), Err(Errno::INVAL));
    }

    #[test]
    fn spawn_setsid_missing_binary_fails() {
        let _children = lock_children();
        let argv = [c"/nonexistent/tasklens-test-binary".to_owned()];
        assert_eq!(spawn_setsid(&argv, None, None), Err(Errno::NOENT));

        let mut status = 0;
        // SAFETY: `status` is valid writable storage for an int.
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        assert_eq!(reaped, -1, "a child was left behind");
        assert_eq!(last_errno(), Errno::CHILD);
    }
}
