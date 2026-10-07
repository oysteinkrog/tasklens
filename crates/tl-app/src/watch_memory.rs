//! Low-memory watcher for `tasklens --watch-memory` (E32-001, PROC-097, R-062).
//!
//! This file holds the pure decision logic, the `/proc/meminfo` reader and the run loop (E32-A1).
//! The watcher runs while the system is short of memory, so the loop allocates nothing per turn
//! and nothing panics on malformed input. The watcher never kills anything itself; it only decides when to
//! open the rescue window. ADR-001: tl-app may not depend on tl-collect, so it reads
//! `/proc/meminfo` itself. Only `std`, `rustix` and `tracing` are used, so an integration test can
//! include this file by path.

use std::os::fd::OwnedFd;

use rustix::fs::{Mode, OFlags};

/// PSI trigger written to /proc/pressure/memory: full stall above 10% over a 2 s window (R-062 as changed by R-151).
pub const PSI_TRIGGER: &str = "full 200000 2000000";
/// How often MemAvailable is checked (R-062).
pub const MEMAVAIL_CHECK: std::time::Duration = std::time::Duration::from_secs(5);
/// MemAvailable below this percent of MemTotal is critical (R-062).
pub const MEMAVAIL_MIN_PCT: u64 = 3;

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
    fd: OwnedFd,
    buf: [u8; MEMINFO_BUF_LEN],
}

impl MemInfoReader {
    pub fn open() -> std::io::Result<Self> {
        Self::open_path(c"/proc/meminfo")
    }

    /// Opens another file with the meminfo layout (tests use this).
    pub fn open_path(path: &std::ffi::CStr) -> std::io::Result<Self> {
        let fd = rustix::fs::open(path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
        Ok(Self {
            fd,
            buf: [0; MEMINFO_BUF_LEN],
        })
    }

    /// Reads and parses the file. None on a read error or an unparsable file.
    pub fn read(&mut self) -> Option<MemInfo> {
        match self.try_read() {
            Ok(m) => Some(m),
            Err(err) => {
                tracing::warn!(%err, "reading /proc/meminfo failed");
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
            match rustix::io::pread(&self.fd, rest, len as u64) {
                Ok(0) => break,
                Ok(n) => len += n,
                Err(rustix::io::Errno::INTR) => {}
                Err(err) => return Err(err.into()),
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
                Err(e) => {
                    tracing::warn!(target: "tasklens::engine", "reading /proc/meminfo failed: {e}");
                }
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
        tracing::warn!(
            target: "tasklens::engine",
            "memory is low but neither WAYLAND_DISPLAY nor DISPLAY is set; no rescue window"
        );
        return 0;
    }
    match io.spawn_rescue() {
        Ok(()) => {
            state.rescue_started();
            1
        }
        Err(e) => {
            tracing::warn!(target: "tasklens::engine", "starting tasklens --low-memory failed: {e}");
            0
        }
    }
}

const PSI_PATH: &std::ffi::CStr = c"/proc/pressure/memory";

/// Opens /proc/pressure/memory and writes PSI_TRIGGER. The trigger lives as long as the fd.
pub fn open_psi_trigger() -> std::io::Result<OwnedFd> {
    let fd = rustix::fs::open(
        PSI_PATH,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let written = rustix::io::write(&fd, PSI_TRIGGER.as_bytes())?;
    if written != PSI_TRIGGER.len() {
        return Err(std::io::ErrorKind::WriteZero.into());
    }
    Ok(fd)
}

/// The real `WatchIo`: the PSI fd, `/proc/meminfo` and the rescue child.
struct RealIo {
    /// None when the trigger is closed or could not be set up.
    psi: Option<OwnedFd>,
    /// false when the first trigger write failed: the watcher then runs on MemAvailable only.
    psi_enabled: bool,
    /// Last time the trigger was (re)opened, to reopen at most once per MEMAVAIL_CHECK.
    psi_opened_at: std::time::Instant,
    /// Set after a failed reopen was logged, so it is logged once until a reopen succeeds.
    psi_reopen_warned: bool,
    meminfo: Option<MemInfoReader>,
    /// Resolved once at start, so a later package update does not leave a "(deleted)" path.
    exe: Option<std::path::PathBuf>,
    child: Option<std::process::Child>,
}

impl RealIo {
    fn new() -> Self {
        let (psi, psi_enabled) = match open_psi_trigger() {
            Ok(fd) => (Some(fd), true),
            Err(e) => {
                tracing::warn!(
                    target: "tasklens::engine",
                    "PSI trigger not available ({e}); checking MemAvailable every 5 s only"
                );
                (None, false)
            }
        };
        let meminfo = match MemInfoReader::open() {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(target: "tasklens::engine", "opening /proc/meminfo failed: {e}");
                None
            }
        };
        let exe = match std::env::current_exe() {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(target: "tasklens::engine", "finding the tasklens binary failed: {e}");
                None
            }
        };
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
                    tracing::warn!(target: "tasklens::engine", "reopening the PSI trigger failed: {e}");
                }
            }
        }
    }
}

impl WatchIo for RealIo {
    fn wait(&mut self, timeout: std::time::Duration) -> WaitResult {
        use rustix::event::{PollFd, PollFlags, Timespec};
        self.maybe_reopen_psi();
        let Some(fd) = &self.psi else {
            std::thread::sleep(timeout);
            return WaitResult::Timeout;
        };
        // The loop never asks for more than MEMAVAIL_CHECK.
        let ts = Timespec::try_from(timeout).unwrap_or(Timespec {
            tv_sec: 5,
            tv_nsec: 0,
        });
        let mut fds = [PollFd::new(fd, PollFlags::PRI)];
        let revents = match rustix::event::poll(&mut fds, Some(&ts)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => return WaitResult::Timeout,
            Ok(_) => fds.first().map_or(PollFlags::empty(), PollFd::revents),
            Err(e) => {
                tracing::warn!(target: "tasklens::engine", "polling the PSI trigger failed: {e}");
                PollFlags::ERR
            }
        };
        if revents.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
            self.psi = None;
            WaitResult::PsiGone
        } else if revents.contains(PollFlags::PRI) {
            WaitResult::PsiFired
        } else {
            WaitResult::Timeout
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
                tracing::warn!(target: "tasklens::engine", "checking the rescue window failed: {e}");
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
}

/// Entry for `--watch-memory`. Starts no GPUI, no engine and no file logger; `warn` and above go
/// to stderr, so the user unit's journal holds them (R-152 OI-4). Runs until SIGTERM.
pub fn run_watch_memory() -> std::process::ExitCode {
    // Fails harmlessly when a subscriber is already set.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
    if let Err(e) =
        rustix::mm::mlockall(rustix::mm::MlockAllFlags::CURRENT | rustix::mm::MlockAllFlags::FUTURE)
    {
        tracing::warn!(target: "tasklens::engine", "mlockall failed: {e}");
    }
    let mut io = RealIo::new();
    run_loop(&mut io, None);
    std::process::ExitCode::SUCCESS
}
