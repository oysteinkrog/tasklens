//! Property tests for the per-process parsers (ADR-013).

mod common;

use common::{fixture_bytes, mutated};
use proptest::prelude::*;
use tl_collect::apps::desktop::{exec_binary_in, parse_entry};
use tl_collect::apps::icons::{kde_icon_theme, parse_icon_cache, parse_index_theme};
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

    #[test]
    fn desktop_parser_arbitrary_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        let _ = parse_entry("fuzz", "/fuzz.desktop", &bytes, &[]);
        let mut with_header = b"[Desktop Entry]\nType=Application\n".to_vec();
        with_header.extend_from_slice(&bytes);
        let _ = parse_entry("fuzz", "/fuzz.desktop", &with_header, &[]);
        let _ = exec_binary_in(&String::from_utf8_lossy(&bytes), &[]);
    }

    #[test]
    fn index_theme_arbitrary_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        let _ = parse_index_theme(&bytes);
        let mut with_header = b"[Icon Theme]\nDirectories=apps/16,16x16/apps\n[apps/16]\n".to_vec();
        with_header.extend_from_slice(&bytes);
        let _ = parse_index_theme(&with_header);
        let _ = kde_icon_theme(&bytes);
    }

    #[test]
    fn icon_cache_parser_arbitrary_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        let _ = parse_icon_cache(&bytes, "breeze-dark");
        let mut with_header = b"version = 1\ntheme = \"breeze-dark\"\n[paths]\n\"a@1\" = \"".to_vec();
        with_header.extend_from_slice(&bytes);
        let _ = parse_icon_cache(&with_header, "breeze-dark");
    }
}

proptest! {
    #[test]
    fn cgroup_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut v2 = b"0::/".to_vec();
        v2.extend_from_slice(&bytes);
        for buf in [&bytes, &v2] {
            if let Some(p) = tl_collect::proc::cgroup::parse_cgroup_file(buf) {
                let _ = tl_collect::proc::cgroup::placement(&p);
                let _ = tl_collect::proc::cgroup::app_slice_unit(&p, 1000);
                let mut out = String::new();
                tl_collect::proc::cgroup::unescape_unit(&p.rel, &mut out);
            }
        }
    }
}

proptest! {
    #[test]
    fn rollup_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut out = tl_collect::proc::smaps::Rollup::default();
        let _ = tl_collect::proc::smaps::parse_rollup(&bytes, &mut out);
    }

    #[test]
    fn rollup_mutated_fixture_never_panics(
        bytes in mutated(fixture_bytes("process-edge", "proc/4201/smaps_rollup"))
    ) {
        let mut out = tl_collect::proc::smaps::Rollup::default();
        let _ = tl_collect::proc::smaps::parse_rollup(&bytes, &mut out);
    }

    #[test]
    fn io_and_statm_arbitrary_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        let mut io = tl_collect::proc::io::PidIo::default();
        let _ = tl_collect::proc::io::parse_io(&bytes, &mut io);
        let mut statm = tl_collect::proc::statm::PidStatm::default();
        let _ = tl_collect::proc::statm::parse_statm(&bytes, &mut statm);
    }
}
