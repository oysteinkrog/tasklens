//! CLOCK_BOOTTIME timestamps and the boot id (ADR-003).

use rustix::time::{ClockId, Timespec, clock_gettime};
use serde::{Deserialize, Serialize};

/// Nanoseconds of CLOCK_BOOTTIME. Every sample and snapshot timestamp uses this clock.
pub type TimeNs = u64;

/// CLOCK_BOOTTIME now, via rustix::time::clock_gettime(ClockId::Boottime).
pub fn now_boot_ns() -> TimeNs {
    timespec_ns(clock_gettime(ClockId::Boottime))
}

/// CLOCK_MONOTONIC now, via rustix::time::clock_gettime(ClockId::Monotonic). Used only for suspend detection.
pub fn now_mono_ns() -> u64 {
    timespec_ns(clock_gettime(ClockId::Monotonic))
}

/// CPU time the calling thread has used, via
/// rustix::time::clock_gettime(ClockId::ThreadCPUTime) (R-144). For frame CPU measurements.
pub fn thread_cpu_ns() -> u64 {
    timespec_ns(clock_gettime(ClockId::ThreadCPUTime))
}

/// The clocks never go below zero, so the fields are never negative.
fn timespec_ns(ts: Timespec) -> u64 {
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Folded from /proc/sys/kernel/random/boot_id: first 8 bytes XOR last 8 bytes of the UUID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BootId(pub u64);

impl BootId {
    /// Parses the text of /proc/sys/kernel/random/boot_id (36-char UUID, optional trailing
    /// newline, hex digits upper or lower case, hyphens at positions 8, 13, 18, 23) and folds it.
    /// Returns None for any other input. Pure: tl-model does no file I/O; tl-collect reads the file.
    pub fn from_uuid_text(text: &str) -> Option<BootId> {
        let text = text.strip_suffix('\n').unwrap_or(text);
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        let mut b = [0u8; 16];
        let mut nibbles = 0usize;
        for (i, &c) in bytes.iter().enumerate() {
            if matches!(i, 8 | 13 | 18 | 23) {
                if c != b'-' {
                    return None;
                }
                continue;
            }
            let v = (c as char).to_digit(16)? as u8;
            b[nibbles / 2] |= if nibbles.is_multiple_of(2) { v << 4 } else { v };
            nibbles += 1;
        }
        let mut hi = [0u8; 8];
        let mut lo = [0u8; 8];
        hi.copy_from_slice(&b[0..8]);
        lo.copy_from_slice(&b[8..16]);
        Some(BootId(u64::from_be_bytes(hi) ^ u64::from_be_bytes(lo)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const UUID: &str = "00112233-4455-6677-8899-aabbccddeeff";

    #[test]
    fn boot_id_fold_known_value() {
        assert_eq!(
            BootId::from_uuid_text("00112233-4455-6677-8899-aabbccddeeff\n"),
            Some(BootId(0x0011_2233_4455_6677 ^ 0x8899_aabb_ccdd_eeff))
        );
    }

    #[test]
    fn boot_id_case_insensitive() {
        let upper = UUID.to_ascii_uppercase();
        assert!(BootId::from_uuid_text(UUID).is_some());
        assert_eq!(BootId::from_uuid_text(&upper), BootId::from_uuid_text(UUID));
    }

    #[test]
    fn boot_id_rejects_bad_input() {
        let bad = [
            "",
            "\n",
            &UUID[..35],
            "00112233-4455-6677-8899-aabbccddeef",
            "00112233-4455-6677-8899-aabbccddeeff0",
            "00112233-4455-6677-8899-aabbccddeefg",
            "0011223-34455-6677-8899-aabbccddeeff",
            "00112233-4455-6677-88990aabbccddeeff",
            "00112233-4455-6677-8899-aabbccddeeff\n\n",
            "00112233-4455-6677-8899-aabbccddeeff\r\n",
            " 00112233-4455-6677-8899-aabbccddeeff",
            "+0112233-4455-6677-8899-aabbccddeeff",
        ];
        for text in bad {
            assert_eq!(BootId::from_uuid_text(text), None, "{text:?}");
        }
    }

    #[test]
    fn boot_id_trailing_newline_optional() {
        let with = format!("{UUID}\n");
        assert!(BootId::from_uuid_text(UUID).is_some());
        assert_eq!(BootId::from_uuid_text(&with), BootId::from_uuid_text(UUID));
    }

    #[test]
    fn clocks_monotonic() {
        let boot_a = now_boot_ns();
        let mono_a = now_mono_ns();
        let boot_b = now_boot_ns();
        let mono_b = now_mono_ns();
        assert!(boot_b >= boot_a);
        assert!(mono_b >= mono_a);
        // Read mono first so boottime, which includes suspend time, is read later.
        let mono = now_mono_ns();
        let boot = now_boot_ns();
        assert!(boot >= mono);
    }

    #[test]
    fn thread_cpu_rises_with_work() {
        let before = thread_cpu_ns();
        let start = std::time::Instant::now();
        let mut x = 0u64;
        // Spin until the thread's own CPU clock has advanced 1 ms. Wall time is only a cap, so a
        // descheduled thread on a busy machine does not fail the test.
        let mut used = 0;
        while used < 1_000_000 && start.elapsed() < std::time::Duration::from_secs(30) {
            x = std::hint::black_box(x.wrapping_add(1));
            used = thread_cpu_ns().saturating_sub(before);
        }
        assert!(used >= 1_000_000, "{used} ns of CPU after {:?} of wall time", start.elapsed());
    }

    proptest! {
        #[test]
        fn boot_id_never_panics(text in any::<String>()) {
            let _ = BootId::from_uuid_text(&text);
        }
    }
}
