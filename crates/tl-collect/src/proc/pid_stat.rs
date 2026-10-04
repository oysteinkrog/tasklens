//! Parser for /proc/<pid>/stat (PROC-002, PROC-004, PROC-029, PROC-036, ADR-016).
//!
//! The same format serves /proc/<pid>/task/<tid>/stat. `comm` may hold spaces and parentheses,
//! so it runs from the first `(` to the last `)` and the numbered fields are read after that.

use memchr::{memchr, memrchr};
use rustix::fd::BorrowedFd;
use smallvec::SmallVec;
use tl_model::process::ProcState;

use crate::CollectError;
use crate::sysroot::{SMALL_FILE_LIMIT, read_at};

const FILE: &str = "stat";

/// `task_struct.flags` bit for a kernel thread (include/linux/sched.h).
pub const PF_KTHREAD: u32 = 0x0020_0000;

/// The last field this parser reads (41, `policy`).
const LAST_FIELD: usize = 41;

/// The fields of one stat line that tasklens uses. Numbers in the comments are proc(5) fields.
/// Reuse one value across processes so `comm_raw` keeps its capacity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PidStat {
    /// 1
    pub pid: u32,
    /// 2: the bytes between the first `(` and the last `)`. Untrusted and not sanitized.
    pub comm_raw: SmallVec<[u8; 32]>,
    /// 3: the raw state letter.
    pub state: u8,
    /// 4
    pub ppid: u32,
    /// 5
    pub pgrp: i32,
    /// 6
    pub session: i32,
    /// 7
    pub tty_nr: i32,
    /// 9
    pub flags: u32,
    /// 10
    pub minflt: u64,
    /// 12
    pub majflt: u64,
    /// 14: clock ticks.
    pub utime: u64,
    /// 15: clock ticks.
    pub stime: u64,
    /// 18
    pub priority: i64,
    /// 19
    pub nice: i8,
    /// 20
    pub num_threads: u32,
    /// 22: clock ticks since boot. Half of the process identity (ADR-003 `ProcessKey`).
    pub start_ticks: u64,
    /// 39
    pub processor: i32,
    /// 40
    pub rt_priority: u32,
    /// 41
    pub policy: u32,
}

/// Parses one stat line into `out`. Any missing or non-numeric field is
/// `ProcParse { pid, file: "stat" }`, with `pid` 0 when field 1 cannot be read.
/// Allocates only when `comm_raw` grows past 32 bytes.
pub fn parse_pid_stat(buf: &[u8], out: &mut PidStat) -> Result<(), CollectError> {
    let open = memchr(b'(', buf);
    let pid = open
        .and_then(|i| buf[..i].strip_suffix(b" "))
        .and_then(parse_u64)
        .and_then(|n| u32::try_from(n).ok());
    let err = CollectError::ProcParse {
        pid: pid.unwrap_or(0),
        file: FILE,
    };
    let (Some(pid), Some(open)) = (pid, open) else {
        return Err(err);
    };
    let Some(close) = memrchr(b')', buf).filter(|&i| i > open) else {
        return Err(err);
    };

    out.pid = pid;
    out.comm_raw.clear();
    out.comm_raw.extend_from_slice(&buf[open + 1..close]);

    let rest = &buf[close + 1..];
    let rest = rest.strip_suffix(b"\n").unwrap_or(rest);
    let Some(rest) = rest.strip_prefix(b" ") else {
        return Err(err);
    };

    // Token i is field i + 3.
    let mut fields = rest.split(|&b| b == b' ');
    let mut next_field = 3;
    let mut field = |n: usize| -> Option<&[u8]> {
        while next_field < n {
            fields.next()?;
            next_field += 1;
        }
        next_field += 1;
        fields.next()
    };

    let parsed = (|| {
        let state = field(3)?;
        out.state = match state {
            [letter] => *letter,
            _ => return None,
        };
        out.ppid = unsigned(field(4)?)?;
        out.pgrp = signed(field(5)?)?;
        out.session = signed(field(6)?)?;
        out.tty_nr = signed(field(7)?)?;
        out.flags = unsigned(field(9)?)?;
        out.minflt = unsigned(field(10)?)?;
        out.majflt = unsigned(field(12)?)?;
        out.utime = unsigned(field(14)?)?;
        out.stime = unsigned(field(15)?)?;
        out.priority = signed(field(18)?)?;
        out.nice = signed(field(19)?)?;
        out.num_threads = unsigned(field(20)?)?;
        out.start_ticks = unsigned(field(22)?)?;
        out.processor = signed(field(39)?)?;
        out.rt_priority = unsigned(field(40)?)?;
        out.policy = unsigned(field(LAST_FIELD)?)?;
        Some(())
    })();
    parsed.ok_or(err)
}

/// Field 3 as a `ProcState`. `X` (dead) and unknown letters are `Other`.
pub fn map_state(letter: u8) -> ProcState {
    ProcState::from_stat_char(letter)
}

/// The label for a state letter (PROC-036). "Frozen" comes from the cgroup (E05-007), not here.
pub fn state_label(letter: u8) -> &'static str {
    match letter {
        b'R' => "Running",
        b'S' => "Sleeping",
        b'D' => "Disk sleep",
        b'T' => "Stopped",
        b't' => "Tracing stop",
        b'Z' => "Zombie",
        b'I' => "Idle",
        b'X' => "Dead",
        _ => "Unknown",
    }
}

/// PROC-004 rule 1: the `PF_KTHREAD` flag, a parent of 2 (`kthreadd`), or pid 2 itself.
pub fn is_kernel_thread(s: &PidStat) -> bool {
    s.flags & PF_KTHREAD != 0 || s.ppid == 2 || s.pid == 2
}

/// Reads `stat` relative to an open `/proc/<pid>` directory and returns field 22.
pub fn read_start_ticks(dirfd: BorrowedFd<'_>, buf: &mut Vec<u8>) -> Result<u64, CollectError> {
    read_at(dirfd, FILE, buf, SMALL_FILE_LIMIT)?;
    let mut stat = PidStat::default();
    parse_pid_stat(buf, &mut stat)?;
    Ok(stat.start_ticks)
}

fn unsigned<T: TryFrom<u64>>(token: &[u8]) -> Option<T> {
    T::try_from(parse_u64(token)?).ok()
}

fn signed<T: TryFrom<i64>>(token: &[u8]) -> Option<T> {
    let value = match token.strip_prefix(b"-") {
        Some(digits) => 0i64.checked_sub_unsigned(parse_u64(digits)?)?,
        None => i64::try_from(parse_u64(token)?).ok()?,
    };
    T::try_from(value).ok()
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
    fn signed_bounds() {
        assert_eq!(signed::<i64>(b"-9223372036854775808"), Some(i64::MIN));
        assert_eq!(signed::<i64>(b"-9223372036854775809"), None);
        assert_eq!(signed::<i8>(b"-20"), Some(-20));
        assert_eq!(signed::<i8>(b"200"), None);
        assert_eq!(signed::<i32>(b"-"), None);
        assert_eq!(unsigned::<u32>(b"+1"), None);
        assert_eq!(unsigned::<u64>(b""), None);
    }

    #[test]
    fn missing_close_paren_is_error() {
        let mut out = PidStat::default();
        let err = parse_pid_stat(b"7 (x S 1", &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::ProcParse {
                pid: 7,
                file: "stat"
            }
        ));
        let err = parse_pid_stat(b"7 (x S 1\n) ", &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::ProcParse {
                pid: 7,
                file: "stat"
            }
        ));
    }
}
