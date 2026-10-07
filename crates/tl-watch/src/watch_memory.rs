//! Low-memory watcher run as `tasklens-watch` (E32-001, PROC-097, R-062, tasklens-m1f.19).
//!
//! This file holds the pure decision logic, the `/proc/meminfo` reader and the run loop (E32-A1).
//! The watcher runs while the system is short of memory, so the loop allocates nothing per turn
//! and nothing panics on malformed input. The watcher never kills anything itself; it only decides when to
//! open the rescue window. Only `std` and `libc` are used, so the process stays small and an
//! integration test can include this file by path. Log lines go to stderr, which the user unit's
//! journal keeps (R-152 OI-4).

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// PSI trigger written to /proc/pressure/memory: full stall above 10% over a 2 s window (R-062 as changed by R-151).
pub const PSI_TRIGGER: &str = "full 200000 2000000";
/// How often MemAvailable is checked (R-062).
pub const MEMAVAIL_CHECK: std::time::Duration = std::time::Duration::from_secs(5);
/// MemAvailable below this percent of MemTotal is critical (R-062).
pub const MEMAVAIL_MIN_PCT: u64 = 3;

/// Name of the lock file in `XDG_RUNTIME_DIR` that keeps a second watcher from running.
pub const LOCK_FILE_NAME: &str = "tasklens-watch.lock";
/// The program the rescue launch runs as `tasklens --low-memory`.
pub const RESCUE_PROGRAM: &str = "tasklens";

/// Writes one `warn` line to stderr. Stderr is unbuffered, so this allocates nothing.
pub fn warn(args: fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stderr().lock(), "WARN tasklens-watch: {args}");
}

/// Size of the fixed read buffer. `/proc/meminfo` is about 1.5 KiB. A read that fills the whole
/// buffer counts as failed, because the file did not fit.
pub const MEMINFO_BUF_LEN: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemInfo {
    pub total_kib: u64,
    pub available_kib: u64,
}

/// Parses the `MemTotal:` and `MemAvailable:` lines (value in kB). None if either is missing,
/// not a number, or MemTotal is 0. Never panics on any input.
pub fn parse_meminfo(buf: &[u8]) -> Option<MemInfo> {
    let mut total = None;
    let mut available = None;
    for line in buf.split(|&b| b == b'\n') {
        if total.is_none()
            && let Some(rest) = line.strip_prefix(b"MemTotal:")
        {
            total = Some(parse_kib(rest)?);
        } else if available.is_none()
            && let Some(rest) = line.strip_prefix(b"MemAvailable:")
        {
            available = Some(parse_kib(rest)?);
        }
        if total.is_some() && available.is_some() {
            break;
        }
    }
    let total_kib = total.filter(|&t| t > 0)?;
    Some(MemInfo {
        total_kib,
        available_kib: available?,
    })
}

/// Parses `   16000000 kB`: a decimal number with an optional `kB` unit. None on anything else
/// or on overflow.
fn parse_kib(field: &[u8]) -> Option<u64> {
    let field = field.trim_ascii();
    let digits_end = field
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(field.len());
    let (digits, unit) = field.split_at(digits_end);
    if digits.is_empty() || !matches!(unit.trim_ascii_start(), b"" | b"kB") {
        return None;
    }
    digits.iter().try_fold(0u64, |acc, &d| {
        acc.checked_mul(10)?.checked_add(u64::from(d - b'0'))
    })
}

/// true when available_kib * 100 < total_kib * MEMAVAIL_MIN_PCT (integer math, no floats).
pub fn mem_available_critical(m: &MemInfo) -> bool {
    // u128 so that no real or malformed value can overflow.
    u128::from(m.available_kib) * 100 < u128::from(m.total_kib) * u128::from(MEMAVAIL_MIN_PCT)
}

/// Reads `/proc/meminfo` into a fixed buffer. The file is opened once; each read is a `pread`
/// at offset 0, so a read allocates nothing and opens no new file descriptor.
pub struct MemInfoReader {
    file: File,
    buf: [u8; MEMINFO_BUF_LEN],
}

impl MemInfoReader {
    pub fn open() -> std::io::Result<Self> {
        Self::open_path(Path::new("/proc/meminfo"))
    }

    /// Opens another file with the meminfo layout (tests use this). std opens with O_CLOEXEC.
    pub fn open_path(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            buf: [0; MEMINFO_BUF_LEN],
        })
    }

    /// Reads and parses the file. None on a read error or an unparsable file.
    pub fn read(&mut self) -> Option<MemInfo> {
        match self.try_read() {
            Ok(m) => Some(m),
            Err(err) => {
                warn(format_args!("reading /proc/meminfo failed: {err}"));
                None
            }
        }
    }

    /// Like `read`, but returns the error instead of logging it. A file that fills the whole
    /// buffer or does not parse is `InvalidData`. The errors carry no heap payload.
    pub fn try_read(&mut self) -> std::io::Result<MemInfo> {
        let mut len = 0;
        while len < self.buf.len() {
            let rest = self
                .buf
                .get_mut(len..)
                .ok_or(std::io::ErrorKind::InvalidData)?;
            match self.file.read_at(rest, len as u64) {
                Ok(0) => break,
                Ok(n) => len += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        if len >= self.buf.len() {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        self.buf
            .get(..len)
            .and_then(parse_meminfo)
            .ok_or_else(|| std::io::ErrorKind::InvalidData.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    PsiTrigger,
    MemCheck(MemInfo),
    RescueExited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchDecision {
    StartRescue,
    Nothing,
}

#[derive(Debug, Default)]
pub struct WatchState {
    rescue_running: bool,
}

impl WatchState {
    /// PsiTrigger, or MemCheck with a critical MemInfo, gives StartRescue only when no rescue
    /// window started by this watcher is running. RescueExited clears the running flag.
    pub fn on_event(&mut self, ev: WatchEvent) -> WatchDecision {
        let pressure = match ev {
            WatchEvent::PsiTrigger => true,
            WatchEvent::MemCheck(m) => mem_available_critical(&m),
            WatchEvent::RescueExited => {
                self.rescue_running = false;
                false
            }
        };
        if pressure && !self.rescue_running {
            WatchDecision::StartRescue
        } else {
            WatchDecision::Nothing
        }
    }

    /// Called after the rescue child was spawned.
    pub fn rescue_started(&mut self) {
        self.rescue_running = true;
    }
}

/// What one wait on the PSI trigger ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    PsiFired,
    Timeout,
    PsiGone,
}

/// The I/O the loop needs; the real one uses /proc and std::process, tests use a fake.
pub trait WatchIo {
    /// Block until the PSI fd reports POLLPRI, or until `timeout` passes.
    fn wait(&mut self, timeout: std::time::Duration) -> WaitResult;
    fn read_meminfo(&mut self) -> std::io::Result<MemInfo>;
    fn spawn_rescue(&mut self) -> std::io::Result<()>;
    /// true while the last spawned rescue child has not exited (Child::try_wait).
    fn rescue_alive(&mut self) -> bool;
    /// The current time. A trait method so that tests can run the 5 s check on a fake clock.
    fn now(&mut self) -> std::time::Instant;
    /// true when `WAYLAND_DISPLAY` or `DISPLAY` is set, so a rescue window can open (R-152 OI-6).
    fn has_display(&mut self) -> bool;
    /// Logs one `warn` line. The real one writes to stderr; tests collect the lines.
    fn warn(&mut self, args: fmt::Arguments<'_>);
}

/// One loop turn per wait. MemCheck runs when MEMAVAIL_CHECK has passed since the last check.
/// Stops after `max_turns` (None = forever). Returns the number of rescues started.
pub fn run_loop<I: WatchIo>(io: &mut I, max_turns: Option<u64>) -> u64 {
    let mut state = WatchState::default();
    let mut rescues = 0;
    let mut turns = 0;
    let mut next_check = later(io.now(), MEMAVAIL_CHECK);
    while max_turns.is_none_or(|max| turns < max) {
        turns += 1;
        let timeout = next_check
            .saturating_duration_since(io.now())
            .min(MEMAVAIL_CHECK);
        let waited = io.wait(timeout);
        if !io.rescue_alive() {
            state.on_event(WatchEvent::RescueExited);
        }
        if waited == WaitResult::PsiFired {
            rescues += handle(io, &mut state, WatchEvent::PsiTrigger);
        }
        let now = io.now();
        if now >= next_check {
            next_check = later(now, MEMAVAIL_CHECK);
            match io.read_meminfo() {
                Ok(m) => rescues += handle(io, &mut state, WatchEvent::MemCheck(m)),
                Err(e) => io.warn(format_args!("reading /proc/meminfo failed: {e}")),
            }
        }
    }
    rescues
}

/// `at + d`, or `at` itself if that would overflow.
fn later(at: std::time::Instant, d: std::time::Duration) -> std::time::Instant {
    at.checked_add(d).unwrap_or(at)
}

/// Feeds one event to the state and starts a rescue when it says so. Returns 1 if a rescue
/// started. A failed spawn leaves the state at "not running", so the next trigger tries again.
fn handle<I: WatchIo>(io: &mut I, state: &mut WatchState, ev: WatchEvent) -> u64 {
    if state.on_event(ev) != WatchDecision::StartRescue {
        return 0;
    }
    if !io.has_display() {
        io.warn(format_args!(
            "memory is low but neither WAYLAND_DISPLAY nor DISPLAY is set; no rescue window"
        ));
        return 0;
    }
    match io.spawn_rescue() {
        Ok(()) => {
            state.rescue_started();
            1
        }
        Err(e) => {
            io.warn(format_args!("starting tasklens --low-memory failed: {e}"));
            0
        }
    }
}

const PSI_PATH: &str = "/proc/pressure/memory";

/// Opens /proc/pressure/memory and writes PSI_TRIGGER. The trigger lives as long as the file.
pub fn open_psi_trigger() -> std::io::Result<File> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(PSI_PATH)?;
    // The kernel replaces the last byte written with a NUL, so the trailing NUL is required.
    // Without it the window loses its last digit (200 ms), which an unprivileged write may not
    // use (EINVAL, checked on kernel 7.2).
    let mut buf = [0u8; PSI_TRIGGER.len() + 1];
    if let Some(head) = buf.get_mut(..PSI_TRIGGER.len()) {
        head.copy_from_slice(PSI_TRIGGER.as_bytes());
    }
    let written = file.write(&buf)?;
    if written != buf.len() {
        return Err(std::io::ErrorKind::WriteZero.into());
    }
    Ok(file)
}

/// What one `poll` on the PSI file saw.
enum PollOutcome {
    Timeout,
    Fired,
    Gone,
}

/// Waits for POLLPRI on `file` for up to `timeout`, rounded up to whole milliseconds so that a
/// short remainder does not turn into a busy loop.
#[allow(unsafe_code, reason = "libc::poll; std has no poll")]
fn poll_pri(file: &File, timeout: std::time::Duration) -> std::io::Result<PollOutcome> {
    let ms = i32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX);
    let mut pfd = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLPRI,
        revents: 0,
    };
    // SAFETY: `pfd` is one valid, initialized pollfd that lives across the call, and nfds is 1.
    let n = unsafe { libc::poll(&raw mut pfd, 1, ms) };
    if n < 0 {
        let err = std::io::Error::last_os_error();
        return if err.kind() == std::io::ErrorKind::Interrupted {
            Ok(PollOutcome::Timeout)
        } else {
            Err(err)
        };
    }
    if n == 0 {
        Ok(PollOutcome::Timeout)
    } else if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        Ok(PollOutcome::Gone)
    } else if pfd.revents & libc::POLLPRI != 0 {
        Ok(PollOutcome::Fired)
    } else {
        Ok(PollOutcome::Timeout)
    }
}

/// Locks all current and future pages in memory, so the watcher is never paged out.
#[allow(unsafe_code, reason = "libc::mlockall; std has no mlockall")]
fn mlockall() -> std::io::Result<()> {
    // SAFETY: mlockall takes only flags and touches no Rust-owned memory.
    if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `name` in the folder of this binary, else the first match on `PATH`. Only absolute, executable
/// regular files count.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .collect::<Vec<_>>()
    });
    sibling
        .into_iter()
        .chain(on_path)
        .find(|p| p.is_absolute() && is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// How the single-instance lock ended.
pub enum InstanceLock {
    /// This process holds the lock for as long as the file stays open.
    Held(File),
    /// Another watcher holds it.
    Busy,
}

/// Takes an exclusive `flock` on `dir/LOCK_FILE_NAME` (created with mode 0600).
pub fn lock_instance(dir: &Path) -> std::io::Result<InstanceLock> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(LOCK_FILE_NAME))?;
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock::Held(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(InstanceLock::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The real `WatchIo`: the PSI fd, `/proc/meminfo` and the rescue child.
struct RealIo {
    /// None when the trigger is closed or could not be set up.
    psi: Option<File>,
    /// false when the first trigger write failed: the watcher then runs on MemAvailable only.
    psi_enabled: bool,
    /// Last time the trigger was (re)opened, to reopen at most once per MEMAVAIL_CHECK.
    psi_opened_at: std::time::Instant,
    /// Set after a failed reopen was logged, so it is logged once until a reopen succeeds.
    psi_reopen_warned: bool,
    meminfo: Option<MemInfoReader>,
    /// `tasklens`, resolved once at start (next to this binary, else on `PATH`).
    exe: Option<PathBuf>,
    child: Option<std::process::Child>,
}

impl RealIo {
    fn new() -> Self {
        let (psi, psi_enabled) = match open_psi_trigger() {
            Ok(fd) => (Some(fd), true),
            Err(e) => {
                warn(format_args!(
                    "PSI trigger not available ({e}); checking MemAvailable every 5 s only"
                ));
                (None, false)
            }
        };
        let meminfo = match MemInfoReader::open() {
            Ok(r) => Some(r),
            Err(e) => {
                warn(format_args!("opening /proc/meminfo failed: {e}"));
                None
            }
        };
        let exe = find_program(RESCUE_PROGRAM);
        if exe.is_none() {
            warn(format_args!(
                "{RESCUE_PROGRAM} not found next to tasklens-watch or on PATH"
            ));
        }
        Self {
            psi,
            psi_enabled,
            psi_opened_at: std::time::Instant::now(),
            psi_reopen_warned: false,
            meminfo,
            exe,
            child: None,
        }
    }

    /// After PsiGone, reopens the trigger once per MEMAVAIL_CHECK.
    fn maybe_reopen_psi(&mut self) {
        if self.psi.is_some() || !self.psi_enabled {
            return;
        }
        let now = std::time::Instant::now();
        if now.saturating_duration_since(self.psi_opened_at) < MEMAVAIL_CHECK {
            return;
        }
        self.psi_opened_at = now;
        match open_psi_trigger() {
            Ok(fd) => {
                self.psi = Some(fd);
                self.psi_reopen_warned = false;
            }
            Err(e) => {
                if !self.psi_reopen_warned {
                    self.psi_reopen_warned = true;
                    warn(format_args!("reopening the PSI trigger failed: {e}"));
                }
            }
        }
    }
}

impl WatchIo for RealIo {
    fn wait(&mut self, timeout: std::time::Duration) -> WaitResult {
        self.maybe_reopen_psi();
        let Some(file) = &self.psi else {
            std::thread::sleep(timeout);
            return WaitResult::Timeout;
        };
        // The loop never asks for more than MEMAVAIL_CHECK.
        match poll_pri(file, timeout.min(MEMAVAIL_CHECK)) {
            Ok(PollOutcome::Timeout) => WaitResult::Timeout,
            Ok(PollOutcome::Fired) => WaitResult::PsiFired,
            Ok(PollOutcome::Gone) => {
                self.psi = None;
                WaitResult::PsiGone
            }
            Err(e) => {
                warn(format_args!("polling the PSI trigger failed: {e}"));
                self.psi = None;
                WaitResult::PsiGone
            }
        }
    }

    fn read_meminfo(&mut self) -> std::io::Result<MemInfo> {
        if self.meminfo.is_none() {
            self.meminfo = Some(MemInfoReader::open()?);
        }
        match &mut self.meminfo {
            Some(r) => r.try_read(),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    fn spawn_rescue(&mut self) -> std::io::Result<()> {
        use std::process::{Command, Stdio};
        let exe = self.exe.as_ref().ok_or(std::io::ErrorKind::NotFound)?;
        let child = Command::new(exe)
            .arg("--low-memory")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }

    fn rescue_alive(&mut self) -> bool {
        let Some(child) = &mut self.child else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => {
                self.child = None;
                false
            }
            Err(e) => {
                warn(format_args!("checking the rescue window failed: {e}"));
                self.child = None;
                false
            }
        }
    }

    fn now(&mut self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn has_display(&mut self) -> bool {
        // Runs only when a rescue is due, never on a plain loop turn.
        ["WAYLAND_DISPLAY", "DISPLAY"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
    }

    fn warn(&mut self, args: fmt::Arguments<'_>) {
        warn(args);
    }
}

/// Entry for `tasklens-watch`. Starts no GPUI, no engine and no file logger; `warn` lines go
/// to stderr, so the user unit's journal holds them (R-152 OI-4). Exits at once when another
/// watcher holds the lock in `XDG_RUNTIME_DIR`; otherwise runs until SIGTERM.
pub fn run_watch_memory() -> std::process::ExitCode {
    // Held until the process ends.
    let _lock = match std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        None => {
            warn(format_args!(
                "XDG_RUNTIME_DIR is not set; running without the single-instance lock"
            ));
            None
        }
        Some(dir) => match lock_instance(&dir) {
            Ok(InstanceLock::Held(file)) => Some(file),
            Ok(InstanceLock::Busy) => {
                warn(format_args!("another tasklens-watch is running; exiting"));
                return std::process::ExitCode::SUCCESS;
            }
            Err(e) => {
                warn(format_args!(
                    "taking the single-instance lock failed ({e}); running without it"
                ));
                None
            }
        },
    };
    if let Err(e) = mlockall() {
        warn(format_args!("mlockall failed: {e}"));
    }
    let mut io = RealIo::new();
    run_loop(&mut io, None);
    std::process::ExitCode::SUCCESS
}
