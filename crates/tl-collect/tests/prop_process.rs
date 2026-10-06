//! Property tests for the per-process parsers (ADR-013).

mod common;

use common::{fixture_bytes, mutated};
use proptest::prelude::*;
use tl_collect::apps::desktop::{exec_binary_in, parse_entry};
use tl_collect::apps::icons::{kde_icon_theme, parse_icon_cache, parse_index_theme};
use tl_collect::proc::detail::group_name;
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

proptest! {
    #[test]
    fn io_stat_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut out = tl_collect::proc::cgroup::CgroupStats::default();
        tl_collect::proc::cgroup::parse_io_stat(&bytes, &mut out);
        prop_assert!(out.io_valid);
        let _ = tl_collect::proc::cgroup::parse_memory_stat_anon(&bytes);
    }
}

proptest! {
    #[test]
    fn passwd_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        use tl_collect::proc::meta::{PasswdUserNames, UserNames};
        let mut names = PasswdUserNames::from_bytes(&bytes);
        for uid in [0, 1, 1000, u32::MAX] {
            prop_assert!(!names.name(uid).is_empty());
        }
    }
}

proptest! {
    #[test]
    fn app_unit_arbitrary_names_never_panic(name in "(?s).{0,256}") {
        use tl_collect::apps::cgroup_app::{is_not_app, parse_app_unit};
        for prefix in ["", "app-", "flatpak-", "dbus-:", "snap."] {
            for suffix in ["", ".scope", ".service"] {
                let unit = format!("{prefix}{name}{suffix}");
                let _ = is_not_app(&unit);
                if let Some(id) = parse_app_unit(&unit) {
                    prop_assert!(!id.app_id.is_empty());
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn sweep_random_fake_root_never_panics(
        procs in prop::collection::vec(
            prop::collection::vec(prop::collection::vec(any::<u8>(), 0..=512), 4),
            1..=20,
        )
    ) {
        use tl_collect::SysRoot;
        use tl_collect::proc::sweep::{SweepCore, SweepCtx};
        use tl_model::time::BootId;

        let tmp = tempfile::tempdir().expect("tempdir");
        for (i, files) in procs.iter().enumerate() {
            let dir = tmp.path().join(format!("proc/{}", 100 + i));
            std::fs::create_dir_all(&dir).expect("create proc dir");
            for (name, bytes) in ["stat", "statm", "status", "io"].iter().zip(files) {
                std::fs::write(dir.join(name), bytes).expect("write file");
            }
        }
        let ctx = SweepCtx {
            boot_id: BootId(1),
            self_uid: 1000,
            ticks_per_sec: 100,
            online_cpus: 4,
            want_fd_count: true,
            want_status_fields: true,
            want_io: true,
        };
        let mut core = SweepCore::new(SysRoot::at(tmp.path()));
        let mut out = Vec::new();
        core.sweep_core(&ctx, &mut out);
        core.sweep_core(&ctx, &mut out);
        prop_assert!(out.len() <= procs.len());
    }
}

proptest! {
    #[test]
    fn smaps_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=8192)) {
        let mut out = Vec::new();
        let _ = tl_collect::proc::smaps::parse_smaps(&bytes, &mut out);
    }

    #[test]
    fn smaps_mutated_fixture_never_panics(
        bytes in mutated(fixture_bytes("process-edge", "proc/4201/smaps"))
    ) {
        let mut out = Vec::new();
        let _ = tl_collect::proc::smaps::parse_smaps(&bytes, &mut out);
    }
}

proptest! {
    #[test]
    fn fdinfo_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        use tl_collect::proc::detail::{FdInfo, FdType, parse_fdinfo};

        let mut row = FdInfo::new(3, FdType::File, std::sync::Arc::from(""), None);
        parse_fdinfo(&bytes, &mut row);
        prop_assert!(row.fdinfo_ok);
    }
}

proptest! {
    #[test]
    fn maps_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=8192)) {
        use tl_collect::proc::detail::{group_maps, parse_maps};

        let (mut regions, mut nodes) = (Vec::new(), Vec::new());
        let _ = parse_maps(&bytes, &mut regions);
        let n = regions.len();
        group_maps(&mut regions, Some("/usr/bin/foo"), None, &mut nodes);
        prop_assert_eq!(regions.len(), n);
        prop_assert_eq!(nodes.last().map_or(0, |node| node.regions.end as usize), n);
    }

    #[test]
    fn maps_mutated_fixture_never_panics(
        bytes in mutated(fixture_bytes("process-edge", "proc/4201/maps"))
    ) {
        let mut out = Vec::new();
        let _ = tl_collect::proc::detail::parse_maps(&bytes, &mut out);
    }
}

proptest! {
    #[test]
    fn pkg_parsers_arbitrary_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        use std::path::Path;
        use tl_collect::pkg::{parse_desc, parse_qkk, parse_qo};

        let _ = parse_desc(&bytes);
        let mut with_header = b"%NAME%\nx\n\n%VERSION%\n".to_vec();
        with_header.extend_from_slice(&bytes);
        let _ = parse_desc(&with_header);
        let _ = parse_qo(&bytes, &[Path::new("/usr/bin/x"), Path::new("")]);
        let mut owned = b"/usr/bin/x is owned by ".to_vec();
        owned.extend_from_slice(&bytes);
        let _ = parse_qo(&owned, &[Path::new("/usr/bin/x")]);
        let _ = parse_qkk("x", &bytes, &bytes);
        let mut summary = b"x: ".to_vec();
        summary.extend_from_slice(&bytes);
        let _ = parse_qkk("x", &summary, b"");
    }
}

/// Package backend that owns nothing.
struct NoPackages;

impl tl_collect::pkg::PkgBackend for NoPackages {
    fn owners(
        &mut self,
        paths: &[&std::path::Path],
    ) -> Result<tl_collect::pkg::PathOwners, tl_collect::CollectError> {
        Ok(paths.iter().map(|p| (p.to_path_buf(), None)).collect())
    }
    fn info(
        &mut self,
        _name: &str,
    ) -> Result<Option<tl_model::process::PackageInfo>, tl_collect::CollectError> {
        Ok(None)
    }
    fn verify(
        &mut self,
        _name: &str,
    ) -> Result<tl_collect::pkg::VerifyOutcome, tl_collect::CollectError> {
        Ok(tl_collect::pkg::VerifyOutcome::Unmodified)
    }
    fn db_stamp(&mut self) -> Option<i64> {
        None
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Security regression: every P2 file is untrusted input (E05-035).
    #[test]
    fn extras_p2_arbitrary_files_never_panic(
        sessionid in prop::collection::vec(any::<u8>(), 0..=64),
        label in prop::collection::vec(any::<u8>(), 0..=8192),
        oom in prop::collection::vec(any::<u8>(), 0..=64),
        status in prop::collection::vec(any::<u8>(), 0..=4096),
        elf in prop::collection::vec(any::<u8>(), 0..=32),
        stat_bytes in prop::collection::vec(any::<u8>(), 0..=512),
    ) {
        use std::fs;
        use std::sync::Arc;
        use rustix::fd::AsFd;
        use rustix::fs::{Mode, OFlags};
        use tl_collect::SysRoot;
        use tl_collect::pkg::PackageIndex;
        use tl_collect::proc::extras_p2::ExtrasP2Reader;
        use tl_collect::proc::meta::{ExeLink, RawMeta, ReadBufs};
        use tl_collect::proc::sweep::PidDir;
        use tl_model::ids::ProcessKey;
        use tl_model::time::BootId;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proc/7");
        fs::create_dir_all(dir.join("attr")).unwrap();
        fs::create_dir_all(dir.join("task/7")).unwrap();
        fs::create_dir_all(tmp.path().join("sys/kernel/security")).unwrap();
        fs::write(tmp.path().join("sys/kernel/security/lsm"), b"apparmor").unwrap();
        fs::write(dir.join("sessionid"), &sessionid).unwrap();
        fs::write(dir.join("attr/current"), &label).unwrap();
        fs::write(dir.join("oom_score"), &oom).unwrap();
        fs::write(dir.join("oom_score_adj"), &oom).unwrap();
        fs::write(dir.join("task/7/status"), &status).unwrap();
        let exe = tmp.path().join("exe-file");
        fs::write(&exe, &elf).unwrap();
        std::os::unix::fs::symlink(&exe, dir.join("exe")).unwrap();

        let mut st = PidStat::default();
        let _ = parse_pid_stat(&stat_bytes, &mut st);
        let mut parsed = PidStatus::default();
        let _ = parse_status(&status, &mut parsed);
        let root = SysRoot::at(tmp.path());
        let proc_fd = rustix::fs::open(
            &root.proc,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        let meta = RawMeta {
            comm: Arc::from("x"),
            exe: ExeLink::Path {
                path: Arc::from(String::from_utf8_lossy(&label).as_ref()),
                deleted: false,
            },
            cmdline: None,
            argv: Default::default(),
            cgroup: None,
            cgroup_denied: false,
        };
        let mut pkg = PackageIndex::new(Box::new(NoPackages));
        let p = ExtrasP2Reader::new().read(
            &root,
            PidDir { proc_fd: proc_fd.as_fd(), pid: 7 },
            ProcessKey { start_ticks: 1, boot_id: BootId(1), pid: 7 },
            &st,
            Some(&parsed),
            &meta,
            &mut pkg,
            std::time::Instant::now(),
            &mut ReadBufs::default(),
        );
        prop_assert!(p.key.is_some());
    }
}

proptest! {
    #[test]
    fn etc_group_arbitrary_bytes_never_panic(
        gid in any::<u32>(),
        bytes in prop::collection::vec(any::<u8>(), 0..=4096)
    ) {
        if let Some(name) = group_name(gid, &bytes) {
            prop_assert!(!name.is_empty());
            prop_assert!(!name.contains('\n'));
        }
    }

    #[test]
    fn etc_group_mutated_fixture_never_panics(
        gid in prop_oneof![Just(968u32), Just(998u32), any::<u32>()],
        bytes in mutated(fixture_bytes("process-edge", "etc/group"))
    ) {
        let _ = group_name(gid, &bytes);
    }
}
