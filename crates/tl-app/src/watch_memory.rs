//! Low-memory watcher for `tasklens --watch-memory` (E32-001, PROC-097, R-062).
//!
//! This part holds the pure decision logic and the `/proc/meminfo` reader. The watcher runs while
//! the system is short of memory, so nothing here allocates on the heap after setup and nothing
//! panics on malformed input. The watcher never kills anything itself; it only decides when to
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

/// Size of the fixed read buffer. `/proc/meminfo` is about 1.5 KiB; `MemTotal` and
/// `MemAvailable` are in the first three lines, so a longer file that is cut off still parses.
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
        let mut len = 0;
        while len < self.buf.len() {
            let rest = self.buf.get_mut(len..)?;
            match rustix::io::pread(&self.fd, rest, len as u64) {
                Ok(0) => break,
                Ok(n) => len += n,
                Err(rustix::io::Errno::INTR) => {}
                Err(err) => {
                    tracing::warn!(%err, "reading /proc/meminfo failed");
                    return None;
                }
            }
        }
        parse_meminfo(self.buf.get(..len)?)
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
