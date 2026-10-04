//! Property tests for the per-process parsers (ADR-013).

mod common;

use common::{fixture_bytes, mutated};
use proptest::prelude::*;
use tl_collect::proc::pid_stat::{PidStat, parse_pid_stat};
use tl_collect::proc::status::{PidStatus, parse_status};

proptest! {
    #[test]
    fn stat_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut out = PidStat::default();
        let _ = parse_pid_stat(&bytes, &mut out);
    }

    #[test]
    fn status_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=10_240)) {
        let mut out = PidStatus::default();
        let _ = parse_status(&bytes, &mut out);
    }

    #[test]
    fn status_mutated_fixture_never_panics(
        bytes in mutated(fixture_bytes("process-edge", "proc/4201/status"))
    ) {
        let mut out = PidStatus::default();
        let _ = parse_status(&bytes, &mut out);
    }
}
