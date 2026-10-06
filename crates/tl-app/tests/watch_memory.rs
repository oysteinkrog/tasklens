//! Tests for the low-memory watcher logic and meminfo reader (E32-001).

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/watch_memory.rs"]
mod watch_memory;

use proptest::prelude::*;
use watch_memory::{
    MemInfo, MemInfoReader, PSI_TRIGGER, WatchDecision, WatchEvent, WatchState,
    mem_available_critical, parse_meminfo,
};

/// The start of a real `/proc/meminfo` (Linux 7.2, 64 GiB machine).
const REAL_MEMINFO: &str = "\
MemTotal:       65753652 kB
MemFree:        16958744 kB
MemAvailable:   24129960 kB
Buffers:           77460 kB
Cached:          7754720 kB
SwapCached:        11392 kB
Active:         21430212 kB
Inactive:       20215680 kB
SwapTotal:      67108860 kB
SwapFree:       66982396 kB
Dirty:              1588 kB
HugePages_Total:       0
HugePages_Free:        0
Hugepagesize:       2048 kB
DirectMap4k:     1093180 kB
DirectMap2M:    37529600 kB
DirectMap1G:    29360128 kB
";

#[test]
fn parse_meminfo_real_layout() {
    assert_eq!(
        parse_meminfo(REAL_MEMINFO.as_bytes()),
        Some(MemInfo {
            total_kib: 65_753_652,
            available_kib: 24_129_960
        })
    );
    let text = "MemTotal: 16000000 kB\nMemFree: 1 kB\nMemAvailable: 400000 kB\n";
    assert_eq!(
        parse_meminfo(text.as_bytes()),
        Some(MemInfo {
            total_kib: 16_000_000,
            available_kib: 400_000
        })
    );
}

#[test]
fn parse_meminfo_missing_field() {
    let no_avail = "MemTotal:       65753652 kB\nMemFree:        16958744 kB\n";
    assert_eq!(parse_meminfo(no_avail.as_bytes()), None);
    let zero_total = "MemTotal: 0 kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(zero_total.as_bytes()), None);
    let not_a_number = "MemTotal: lots kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(not_a_number.as_bytes()), None);
    let overflow = "MemTotal: 99999999999999999999999 kB\nMemAvailable: 100 kB\n";
    assert_eq!(parse_meminfo(overflow.as_bytes()), None);
    assert_eq!(parse_meminfo(b""), None);
}

#[test]
fn threshold_boundary_3_percent() {
    assert!(mem_available_critical(&MemInfo {
        total_kib: 1000,
        available_kib: 29
    }));
    assert!(!mem_available_critical(&MemInfo {
        total_kib: 1000,
        available_kib: 30
    }));
    // No overflow at the extremes.
    assert!(!mem_available_critical(&MemInfo {
        total_kib: u64::MAX,
        available_kib: u64::MAX
    }));
}

#[test]
fn state_one_rescue_at_a_time() {
    let mut s = WatchState::default();
    assert_eq!(
        s.on_event(WatchEvent::PsiTrigger),
        WatchDecision::StartRescue
    );

    s.rescue_started();
    assert_eq!(s.on_event(WatchEvent::PsiTrigger), WatchDecision::Nothing);
    let critical = MemInfo {
        total_kib: 1000,
        available_kib: 1,
    };
    assert_eq!(
        s.on_event(WatchEvent::MemCheck(critical)),
        WatchDecision::Nothing
    );

    assert_eq!(s.on_event(WatchEvent::RescueExited), WatchDecision::Nothing);
    assert_eq!(
        s.on_event(WatchEvent::PsiTrigger),
        WatchDecision::StartRescue
    );

    let mut fresh = WatchState::default();
    let fine = MemInfo {
        total_kib: 1000,
        available_kib: 500,
    };
    assert_eq!(
        fresh.on_event(WatchEvent::MemCheck(fine)),
        WatchDecision::Nothing
    );
    assert_eq!(
        fresh.on_event(WatchEvent::MemCheck(critical)),
        WatchDecision::StartRescue
    );
}

#[test]
fn psi_trigger_string_exact() {
    assert_eq!(PSI_TRIGGER, "full 200000 2000000");
}

/// Reads the live `/proc/meminfo` (read only) twice through one reader.
#[test]
fn reader_reads_proc_meminfo_twice() {
    let mut reader = MemInfoReader::open().expect("open /proc/meminfo");
    for _ in 0..2 {
        let m = reader.read().expect("parse /proc/meminfo");
        assert!(m.total_kib > 0);
        assert!(m.available_kib <= m.total_kib);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn prop_parse_meminfo_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        let _ = parse_meminfo(&bytes);
    }

    /// Malformed text around the two real lines: still no panic, and when it parses,
    /// the critical check does not panic either.
    #[test]
    fn prop_parse_meminfo_noisy_lines_never_panic(
        prefix in "[ -~\n]{0,200}",
        total in "[0-9 kB]{0,30}",
        avail in "[0-9 kB]{0,30}",
    ) {
        let text = format!("{prefix}\nMemTotal:{total}\nMemAvailable:{avail}\n");
        if let Some(m) = parse_meminfo(text.as_bytes()) {
            let _ = mem_available_critical(&m);
        }
    }
}
