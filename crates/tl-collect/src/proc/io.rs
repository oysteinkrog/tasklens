//! Parser for /proc/<pid>/io (PROC-002, PROC-031, ADR-016).
//!
//! The kernel lets a user read `io` only for processes they may ptrace, so other users'
//! processes give EACCES or EPERM. That is `IoRead::Restricted`, never zeros.

use memchr::memchr;
use rustix::buffer::spare_capacity;
use rustix::fd::{AsFd, BorrowedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::CollectError;
use crate::proc::{is_denied, is_gone};
use crate::sysroot::SMALL_FILE_LIMIT;

const FILE: &str = "io";

const PARSE_ERROR: CollectError = CollectError::ProcParse { pid: 0, file: FILE };

/// All seven keys seen.
const ALL_KEYS: u8 = 0x7f;

/// The counters of one `io` file. Bytes and syscall counts since the process started.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PidIo {
    pub rchar: u64,
    pub wchar: u64,
    pub syscr: u64,
    pub syscw: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub cancelled_write_bytes: u64,
}

/// The result of reading one `io` file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IoRead {
    Ok(PidIo),
    /// EACCES or EPERM: another user's process.
    Restricted,
    /// ENOENT or ESRCH: the process exited.
    Gone,
}

/// Parses `key: value` lines into `out`. Unknown keys are skipped. A missing key, or a known
/// key with a value that is not a decimal number, is `ProcParse { pid: 0, file: "io" }`.
pub fn parse_io(buf: &[u8], out: &mut PidIo) -> Result<(), CollectError> {
    let mut seen = 0u8;
    let mut rest = buf;
    while !rest.is_empty() {
        let (line, tail) = match memchr(b'\n', rest) {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = tail;
        let Some(colon) = memchr(b':', line) else {
            continue;
        };
        let (bit, slot) = match &line[..colon] {
            b"rchar" => (0, &mut out.rchar),
            b"wchar" => (1, &mut out.wchar),
            b"syscr" => (2, &mut out.syscr),
            b"syscw" => (3, &mut out.syscw),
            b"read_bytes" => (4, &mut out.read_bytes),
            b"write_bytes" => (5, &mut out.write_bytes),
            b"cancelled_write_bytes" => (6, &mut out.cancelled_write_bytes),
            _ => continue,
        };
        *slot = parse_u64(line[colon + 1..].trim_ascii()).ok_or(PARSE_ERROR)?;
        seen |= 1 << bit;
    }
    if seen == ALL_KEYS {
        Ok(())
    } else {
        Err(PARSE_ERROR)
    }
}

/// Reads and parses `io` relative to an open `/proc/<pid>` directory. EACCES or EPERM on open
/// or read is `Restricted`, ENOENT or ESRCH is `Gone`, any other errno is
/// `ProcRead { file: "io", errno }`.
pub fn read_io(dirfd: BorrowedFd<'_>, buf: &mut Vec<u8>) -> Result<IoRead, CollectError> {
    match read_file(dirfd, buf) {
        Ok(()) => {}
        Err(e) if is_denied(e) => return Ok(IoRead::Restricted),
        Err(e) if is_gone(e) => return Ok(IoRead::Gone),
        Err(e) => {
            return Err(CollectError::ProcRead {
                file: FILE,
                errno: e.raw_os_error(),
            });
        }
    }
    let mut io = PidIo::default();
    parse_io(buf, &mut io)?;
    Ok(IoRead::Ok(io))
}

/// PROC-031 Disk: bytes read plus bytes written minus cancelled writes, at least 0.
pub fn disk_bytes(io: &PidIo) -> u64 {
    io.read_bytes
        .saturating_add(io.write_bytes)
        .saturating_sub(io.cancelled_write_bytes)
}

/// Reads `io` into `buf` until EOF or `SMALL_FILE_LIMIT` bytes. The real file is about 200 bytes.
fn read_file(dirfd: BorrowedFd<'_>, buf: &mut Vec<u8>) -> Result<(), Errno> {
    buf.clear();
    let fd = rustix::fs::openat(dirfd, FILE, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    while buf.len() < SMALL_FILE_LIMIT {
        if buf.len() == buf.capacity() {
            buf.reserve(SMALL_FILE_LIMIT - buf.len());
        }
        if rustix::io::read(fd.as_fd(), spare_capacity(buf))? == 0 {
            break;
        }
    }
    buf.truncate(SMALL_FILE_LIMIT);
    Ok(())
}

/// Decimal digits only, at least one, no overflow.
fn parse_u64(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |n, &d| {
        if d.is_ascii_digit() {
            n.checked_mul(10)?.checked_add(u64::from(d - b'0'))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_value_is_error() {
        let mut out = PidIo::default();
        let buf = b"rchar: x\nwchar: 1\nsyscr: 1\nsyscw: 1\nread_bytes: 1\nwrite_bytes: 1\n\
                    cancelled_write_bytes: 1\n";
        assert!(matches!(
            parse_io(buf, &mut out),
            Err(CollectError::ProcParse { pid: 0, file: "io" })
        ));
    }

    #[test]
    fn unknown_key_skipped() {
        let mut out = PidIo::default();
        let buf = b"rchar: 1\nwchar: 2\nsyscr: 3\nsyscw: 4\nread_bytes: 5\nwrite_bytes: 6\n\
                    cancelled_write_bytes: 7\nfuture_key: 8";
        parse_io(buf, &mut out).unwrap();
        assert_eq!(out.cancelled_write_bytes, 7);
    }
}
