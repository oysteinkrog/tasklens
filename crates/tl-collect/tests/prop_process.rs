//! Property tests for the per-process parsers (ADR-013).

use proptest::prelude::*;
use tl_collect::proc::pid_stat::{PidStat, parse_pid_stat};

proptest! {
    #[test]
    fn stat_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut out = PidStat::default();
        let _ = parse_pid_stat(&bytes, &mut out);
    }
}
