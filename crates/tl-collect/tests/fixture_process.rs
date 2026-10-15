//! Fixture tests for the per-process readers (E05). `process-edge` holds hand-made edge cases,
//! `owner-desktop` is a real capture.

mod common;

use std::fs;

use common::{fixture_bytes, fixture_dir};
use rustix::fd::AsFd;
use rustix::fs::{Mode, OFlags};
use tl_collect::CollectError;
use tl_collect::proc::pid_stat::{
    PidStat, is_kernel_thread, map_state, parse_pid_stat, read_start_ticks, state_label,
};
use tl_collect::proc::status::{
    PidStatus, STATUS_READ_MAX, commit_bytes, parse_status, ws_shared_bytes,
};
use tl_model::process::ProcState;

fn stat(fixture: &str, pid: u32) -> PidStat {
    let mut out = PidStat::default();
    parse_pid_stat(
        &fixture_bytes(fixture, &format!("proc/{pid}/stat")),
        &mut out,
    )
    .expect("parse stat");
    out
}

#[test]
fn stat_comm_with_spaces_and_parens() {
    let s = stat("process-edge", 4202);
    assert_eq!(s.pid, 4202);
    assert_eq!(s.comm_raw.as_slice(), b"a) (b) c");
    assert_eq!(s.state, b'R');
    assert_eq!(s.ppid, 4201);
    assert_eq!(s.utime, 1234);
    assert_eq!(s.stime, 567);
    assert_eq!(s.priority, 19);
    assert_eq!(s.nice, -1);
    assert_eq!(s.num_threads, 4);
    assert_eq!(s.start_ticks, 98765);
    assert_eq!(s.processor, 5);

    let s = stat("process-edge", 4201);
    assert_eq!(s.comm_raw.as_slice(), b"Web Content");
    assert_eq!(s.ppid, 4100);
    assert_eq!(s.minflt, 52311);
    assert_eq!(s.majflt, 12);
    assert_eq!(s.num_threads, 27);
    assert_eq!(s.start_ticks, 351_220);
    assert_eq!(s.processor, 3);
}

#[test]
fn stat_invalid_utf8_comm_kept_raw() {
    let s = stat("process-edge", 4203);
    assert_eq!(s.comm_raw.as_slice(), [0xff, 0xfe]);
    assert_eq!(s.ppid, 1);
    assert_eq!(s.start_ticks, 400_000);
}

#[test]
fn stat_truncated_is_parse_error() {
    let buf = fixture_bytes("process-edge", "proc/4201/stat");
    // After the last `)` come a space and then fields 3, 4, ...: field 15 ends at the 14th space.
    let close = buf.iter().rposition(|&b| b == b')').unwrap();
    let cut = buf[close..]
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b' ')
        .nth(13)
        .map(|(i, _)| close + i)
        .unwrap();
    let truncated = &buf[..cut];
    assert!(truncated.ends_with(b" 1204"), "cut after stime");

    let mut out = PidStat::default();
    let err = parse_pid_stat(truncated, &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 4201,
            file: "stat"
        }
    ));

    let err = parse_pid_stat(b"x (a) S 1", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "stat"
        }
    ));
    let err = parse_pid_stat(b"", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "stat"
        }
    ));
}

#[test]
fn stat_kernel_thread_detection() {
    let kworker = stat("process-edge", 57);
    assert_eq!(kworker.comm_raw.as_slice(), b"kworker/0:1");
    assert!(is_kernel_thread(&kworker));
    assert!(is_kernel_thread(&stat("process-edge", 2)));
    assert!(!is_kernel_thread(&stat("process-edge", 4201)));
}

#[test]
fn stat_state_letters() {
    assert_eq!(state_label(b't'), "Tracing stop");
    assert_eq!(map_state(b't'), ProcState::Stopped);
    assert_eq!(state_label(b'X'), "Dead");
    assert_eq!(map_state(b'X'), ProcState::Other);
    assert_eq!(state_label(b'?'), "Unknown");
    assert_eq!(map_state(b'D'), ProcState::DiskSleep);
}

#[test]
fn stat_read_start_ticks_from_dirfd() {
    let dir = rustix::fs::open(
        fixture_dir("process-edge").join("proc/4202"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    let mut buf = Vec::new();
    assert_eq!(read_start_ticks(dir.as_fd(), &mut buf).unwrap(), 98765);
}

#[test]
fn stat_owner_desktop_snapshot() {
    let proc_dir = fixture_dir("owner-desktop").join("proc");
    let mut pids: Vec<u32> = fs::read_dir(&proc_dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.parse().ok())
        .filter(|pid: &u32| proc_dir.join(pid.to_string()).join("stat").exists())
        .collect();
    pids.sort_unstable();
    assert!(!pids.is_empty());

    let rows: Vec<(u32, u32, char, u32)> = pids
        .iter()
        .map(|&pid| {
            let s = stat("owner-desktop", pid);
            assert_eq!(s.pid, pid);
            (s.pid, s.ppid, char::from(s.state), s.num_threads)
        })
        .collect();
    insta::assert_debug_snapshot!(rows);
}

fn status(fixture: &str, pid: u32) -> PidStatus {
    let mut out = PidStatus::default();
    parse_status(
        &fixture_bytes(fixture, &format!("proc/{pid}/status")),
        &mut out,
    )
    .expect("parse status");
    out
}

#[test]
fn status_memory_fields_in_bytes() {
    let s = status("process-edge", 4201);
    assert_eq!(s.uid, [1000; 4]);
    assert_eq!(s.gid, [1000; 4]);
    assert_eq!(s.groups.as_slice(), [10, 998, 1000]);
    assert_eq!(s.threads, 27);
    assert_eq!(s.rss_anon, Some(1_263_616));
    assert_eq!(s.rss_file, Some(198_520 * 1024));
    assert_eq!(s.rss_shmem, Some(45_182 * 1024));
    assert_eq!(s.vm_rss, Some(244_936 * 1024));
    assert_eq!(s.vm_swap, Some(5120 * 1024));
    assert_eq!(s.vm_hwm, Some(301_220 * 1024));
    assert_eq!(s.vm_peak, Some(2_734_592 * 1024));
    assert_eq!(s.vm_size, Some(2_667_832 * 1024));
    assert_eq!(s.vm_data, Some(100 * 1024));
    assert_eq!(s.vm_stk, Some(132 * 1024));
    assert_eq!(s.vm_pte, Some(1388 * 1024));
    assert_eq!(commit_bytes(&s), Some(237_568));
    assert_eq!(ws_shared_bytes(&s), Some((198_520 + 45_182) * 1024));
    assert_eq!(s.cpus_allowed_list.as_slice(), b"0-7,16-23");
    assert_eq!(s.cap_eff, 0);
    assert_eq!(s.cap_bnd, 0x1ff_ffff_ffff);
    assert_eq!(s.no_new_privs, Some(true));
    assert_eq!(s.seccomp, Some(2));
    assert_eq!(s.seccomp_filters, Some(3));
    assert_eq!(
        s.speculation_store_bypass.as_slice(),
        b"thread force mitigated"
    );
    assert_eq!(
        s.speculation_indirect_branch.as_slice(),
        b"conditional force disabled"
    );
    assert_eq!(s.x86_thread_features.as_deref(), Some(&b"shstk wrss"[..]));
    assert_eq!(s.voluntary_ctxt_switches, Some(88120));
    assert_eq!(s.nonvoluntary_ctxt_switches, Some(1542));
}

#[test]
fn status_kernel_thread_has_no_rss() {
    let s = status("process-edge", 57);
    assert_eq!(s.rss_anon, None);
    assert_eq!(s.vm_rss, None);
    assert_eq!(commit_bytes(&s), None);
    assert_eq!(ws_shared_bytes(&s), None);
    assert!(s.groups.is_empty());
    assert_eq!(s.x86_thread_features, None);
    assert_eq!(s.threads, 1);
}

#[test]
fn status_setuid_uids_and_caps() {
    let s = status("process-edge", 4204);
    assert_eq!(s.uid, [1000, 0, 0, 0]);
    assert_ne!(s.cap_eff, 0);
}

#[test]
fn status_missing_uid_is_error() {
    let buf = fixture_bytes("process-edge", "proc/4201/status");
    let without_uid: Vec<u8> = buf
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !line.starts_with(b"Uid:"))
        .flatten()
        .copied()
        .collect();
    let mut out = PidStatus::default();
    let err = parse_status(&without_uid, &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "status"
        }
    ));
    assert!(parse_status(b"", &mut out).is_err());
}

#[test]
fn status_long_file_parsed_up_to_cut() {
    // A 20 KiB file: the real lines, filler lines with unknown keys, then a `VmSwap:` line that
    // the cut at STATUS_READ_MAX splits after `VmSwap:\t9`. The partial line must be skipped.
    let mut buf = fixture_bytes("process-edge", "proc/4201/status");
    let swap_line = b"VmSwap:\t99999 kB\n";
    let filler_len = STATUS_READ_MAX - 9 - buf.len();
    buf.extend_from_slice(b"Filler:");
    buf.resize(buf.len() + filler_len - 8, b'x');
    buf.push(b'\n');
    buf.extend_from_slice(swap_line);
    while buf.len() < 20 * 1024 {
        buf.extend_from_slice(b"Filler:\t1\n");
    }
    let cut = &buf[..STATUS_READ_MAX];
    assert!(cut.ends_with(b"VmSwap:\t9"));

    let mut out = PidStatus::default();
    parse_status(cut, &mut out).expect("parse cut status");
    assert_eq!(out.uid, [1000; 4]);
    assert_eq!(out.rss_anon, Some(1_263_616));
    assert_eq!(out.vm_swap, Some(5120 * 1024));
    assert_eq!(out.nonvoluntary_ctxt_switches, Some(1542));
}

#[test]
fn status_owner_desktop_snapshot() {
    let proc_dir = fixture_dir("owner-desktop").join("proc");
    let mut pids: Vec<u32> = fs::read_dir(&proc_dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.parse().ok())
        .filter(|pid: &u32| proc_dir.join(pid.to_string()).join("status").exists())
        .collect();
    pids.sort_unstable();
    assert!(!pids.is_empty());

    // (pid, real uid, threads, RssAnon, VmRSS)
    type Row = (u32, u32, u32, Option<u64>, Option<u64>);
    let rows: Vec<Row> = pids
        .iter()
        .map(|&pid| {
            let s = status("owner-desktop", pid);
            (pid, s.uid[0], s.threads, s.rss_anon, s.vm_rss)
        })
        .collect();
    insta::assert_debug_snapshot!(rows);
}

use tl_collect::SysRoot;
use tl_collect::proc::smaps::{
    CoverageBuilder, PssReader, Rollup, RollupRead, apply_rollups, parse_rollup,
};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

/// start_ticks of process-edge 4201, which `fake_procs` copies into every fake process.
const FAKE_START_TICKS: u64 = 351_220;

fn rollup_key(pid: u32, start_ticks: u64) -> ProcessKey {
    ProcessKey {
        start_ticks,
        boot_id: BootId(1),
        pid,
    }
}

/// A proc root with `n` processes (pids 10000..) that all have the stat and smaps_rollup of
/// process-edge 4201.
fn fake_procs(n: u32) -> (tempfile::TempDir, SysRoot, Vec<ProcessKey>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stat = fixture_bytes("process-edge", "proc/4201/stat");
    let rollup = fixture_bytes("process-edge", "proc/4201/smaps_rollup");
    let mut keys = Vec::new();
    for pid in 10_000..10_000 + n {
        let dir = tmp.path().join(format!("proc/{pid}"));
        fs::create_dir_all(&dir).expect("create proc dir");
        fs::write(dir.join("stat"), &stat).expect("write stat");
        fs::write(dir.join("smaps_rollup"), &rollup).expect("write smaps_rollup");
        keys.push(rollup_key(pid, FAKE_START_TICKS));
    }
    let root = SysRoot::at(tmp.path());
    (tmp, root, keys)
}

fn own_key() -> ProcessKey {
    let pid = std::process::id();
    let dir = rustix::fs::open(
        format!("/proc/{pid}"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("open own proc dir");
    let ticks = read_start_ticks(dir.as_fd(), &mut Vec::new()).expect("own start ticks");
    rollup_key(pid, ticks)
}

#[test]
fn rollup_parse_bytes() {
    let mut out = Rollup::default();
    parse_rollup(
        &fixture_bytes("process-edge", "proc/4201/smaps_rollup"),
        &mut out,
    )
    .unwrap();
    assert_eq!(out.pss, 2_097_152);
    assert_eq!(out.uss, 1_572_864);
    assert_eq!(out.swap, 0);
}

#[test]
fn rollup_fd_cached_and_reused() {
    let root = SysRoot::host();
    let keys = [own_key()];
    let mut reader = PssReader::new();
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        reader.read(&root, &keys, &mut buf, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, keys[0]);
        assert!(
            matches!(out[0].1, RollupRead::Ok(r) if r.pss > 0),
            "{out:?}"
        );
        assert_eq!(reader.cached(), 1);
        assert_eq!(reader.opens(), 1);
    }
}

/// Security regression: a reused pid never yields another process's PSS.
#[test]
fn rollup_identity_mismatch_gone() {
    let root = SysRoot::host();
    let real = own_key();
    let wrong = rollup_key(real.pid, real.start_ticks + 1);
    let mut reader = PssReader::new();
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    reader.read(&root, &[wrong], &mut buf, &mut out);
    assert_eq!(out, vec![(wrong, RollupRead::Gone)]);
    assert_eq!(reader.cached(), 0);
    assert_eq!(reader.opens(), 0);

    // Same in a fixture root, where the key's start time differs from the stat file.
    let (_tmp, root, keys) = fake_procs(1);
    let wrong = rollup_key(keys[0].pid, FAKE_START_TICKS - 1);
    reader.read(&root, &[wrong], &mut buf, &mut out);
    assert_eq!(out, vec![(wrong, RollupRead::Gone)]);
    assert_eq!(reader.cached(), 0);
}

#[test]
fn rollup_cache_cap_128() {
    let (_tmp, root, keys) = fake_procs(130);
    let mut reader = PssReader::new();
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    reader.read(&root, &keys, &mut buf, &mut out);
    assert_eq!(out.len(), 130);
    assert!(out.iter().all(|(_, r)| matches!(r, RollupRead::Ok(_))));
    assert_eq!(reader.cached(), 128);
    assert_eq!(reader.opens(), 130);

    // Second call: the first 128 come from the cache, the last two are opened again.
    reader.read(&root, &keys, &mut buf, &mut out);
    assert!(out.iter().all(|(_, r)| matches!(r, RollupRead::Ok(_))));
    assert_eq!(reader.cached(), 128);
    assert_eq!(reader.opens(), 132);
}

#[test]
fn rollup_drop_left_demand() {
    let (_tmp, root, keys) = fake_procs(2);
    let mut reader = PssReader::new();
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    reader.read(&root, &keys, &mut buf, &mut out);
    assert_eq!(reader.cached(), 2);

    reader.read(&root, &keys[1..], &mut buf, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(reader.cached(), 1);
    assert_eq!(reader.opens(), 2);

    // The dropped key is opened again when it comes back.
    reader.read(&root, &keys, &mut buf, &mut out);
    assert_eq!(reader.cached(), 2);
    assert_eq!(reader.opens(), 3);
}

#[test]
fn rollup_apply_and_coverage() {
    use std::sync::Arc;
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::validity::{FieldValidity, ProcessField, Validity};

    let meta = Arc::new(ProcessMeta {
        comm: "t".into(),
        name: "t".into(),
        exe: None,
        cmdline: None,
        user: "user".into(),
        cgroup: None,
        app_id: None,
        icon: None,
        description: None,
        desktop_path: None,
        search_lower: "t".into(),
    });
    let mut records: Vec<ProcessRecord> = (1..=5u32)
        .map(|pid| ProcessRecord {
            key: rollup_key(pid, 100 * u64::from(pid)),
            ppid: 1,
            uid: 1000,
            meta: meta.clone(),
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::default(),
        })
        .collect();
    let rollup = |pss: u64| {
        RollupRead::Ok(Rollup {
            pss,
            uss: pss / 2,
            swap: 0,
        })
    };
    let results = [
        (records[3].key, rollup(4096)),
        (records[0].key, rollup(1024)),
        (records[2].key, rollup(2048)),
    ];
    let mut builder = CoverageBuilder::new();
    apply_rollups(&mut records, &results, &mut builder);
    let coverage = builder.build();

    let mut expected = vec![records[0].key, records[2].key, records[3].key];
    expected.sort();
    assert_eq!(&*coverage.pss_uss, expected.as_slice());
    for (i, record) in records.iter().enumerate() {
        let covered = matches!(i, 0 | 2 | 3);
        assert_eq!(coverage.has_pss_uss(&record.key), covered);
        let want = if covered {
            Validity::Valid
        } else {
            Validity::WarmingUp
        };
        assert_eq!(record.validity.get(ProcessField::Pss), want);
        assert_eq!(record.validity.get(ProcessField::Uss), want);
    }
    assert_eq!(records[2].pss_bytes, 2048);
    assert_eq!(records[2].uss_bytes, 1024);
    assert_eq!(records[1].pss_bytes, 0);

    // Restricted sets that validity and does not cover the key.
    let restricted = [(records[1].key, RollupRead::Restricted)];
    apply_rollups(&mut records, &restricted, &mut builder);
    assert_eq!(
        records[1].validity.get(ProcessField::Pss),
        Validity::Restricted
    );
    assert!(builder.build().pss_uss.is_empty());
}

use std::os::unix::fs::PermissionsExt;

use tl_collect::proc::io::{IoRead, PidIo, disk_bytes, parse_io, read_io};
use tl_collect::proc::statm::{PidStatm, parse_statm};

fn open_dir(path: &std::path::Path) -> rustix::fd::OwnedFd {
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("open proc dir")
}

#[test]
fn io_fields_parsed() {
    let mut out = PidIo::default();
    parse_io(&fixture_bytes("process-edge", "proc/4201/io"), &mut out).expect("parse io");
    assert_eq!(
        out,
        PidIo {
            rchar: 48_213_377,
            wchar: 9_120_044,
            syscr: 61_204,
            syscw: 18_840,
            read_bytes: 31_571_968,
            write_bytes: 6_627_328,
            cancelled_write_bytes: 1_048_576,
        }
    );

    let dir = open_dir(&fixture_dir("process-edge").join("proc/4201"));
    let mut buf = Vec::new();
    assert_eq!(read_io(dir.as_fd(), &mut buf).unwrap(), IoRead::Ok(out));
}

#[test]
fn io_missing_key_is_error() {
    let full = fixture_bytes("process-edge", "proc/4201/io");
    let without: Vec<u8> = full
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !line.starts_with(b"cancelled_write_bytes"))
        .flatten()
        .copied()
        .collect();
    let mut out = PidIo::default();
    let err = parse_io(&without, &mut out).unwrap_err();
    assert!(matches!(err, CollectError::ProcParse { file: "io", .. }));
}

/// Security regression (ADR-016): an unreadable `io` must be Restricted, never zeros.
#[test]
fn io_eacces_is_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("4201");
    fs::create_dir(&dir).expect("create dir");
    for name in ["stat", "statm", "io"] {
        let rel = format!("proc/4201/{name}");
        fs::write(dir.join(name), fixture_bytes("process-edge", &rel)).expect("copy file");
    }
    fs::set_permissions(dir.join("io"), fs::Permissions::from_mode(0o000)).expect("chmod");

    let fd = open_dir(&dir);
    let mut buf = Vec::new();
    assert_eq!(read_io(fd.as_fd(), &mut buf).unwrap(), IoRead::Restricted);
}

#[test]
fn io_missing_file_is_gone() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fd = open_dir(tmp.path());
    let mut buf = Vec::new();
    assert_eq!(read_io(fd.as_fd(), &mut buf).unwrap(), IoRead::Gone);
}

#[test]
fn disk_bytes_saturates() {
    let io = PidIo {
        read_bytes: 10,
        write_bytes: 5,
        cancelled_write_bytes: 20,
        ..PidIo::default()
    };
    assert_eq!(disk_bytes(&io), 0);
    let io = PidIo {
        cancelled_write_bytes: 3,
        ..io
    };
    assert_eq!(disk_bytes(&io), 12);
}

#[test]
fn statm_fields_parsed() {
    let bytes = fixture_bytes("process-edge", "proc/4201/statm");
    let fields: Vec<u64> = std::str::from_utf8(&bytes)
        .expect("utf8")
        .split_ascii_whitespace()
        .map(|f| f.parse().expect("number"))
        .collect();
    let mut out = PidStatm::default();
    parse_statm(&bytes, &mut out).expect("parse statm");
    assert_eq!(out.resident, fields[1]);
    assert_eq!(
        out,
        PidStatm {
            size: 666_958,
            resident: 61_234,
            shared: 60_925,
            text: 181,
            data: 58,
        }
    );
}

use tl_collect::proc::cgroup::{
    CgroupPath, UserPlacement, app_slice_unit, parse_cgroup_file, placement, unescape_unit,
};

fn cgroup(pid: u32) -> CgroupPath {
    parse_cgroup_file(&fixture_bytes(
        "process-edge",
        &format!("proc/{pid}/cgroup"),
    ))
    .expect("parse cgroup")
}

#[test]
fn cgroup_kde_app_unit() {
    let p = cgroup(4205);
    assert_eq!(
        &*p.rel,
        "user.slice/user-1000.slice/user@1000.service/app.slice/app-chrome\\x2dwork@00bca6.service"
    );
    assert_eq!(
        p.unit.as_deref(),
        Some("app-chrome\\x2dwork@00bca6.service")
    );
    let slices: Vec<&str> = p.slices.iter().map(|s| &**s).collect();
    assert_eq!(slices, ["user.slice", "user-1000.slice", "app.slice"]);
    assert_eq!(
        app_slice_unit(&p, 1000),
        Some("app-chrome\\x2dwork@00bca6.service")
    );

    let firefox = cgroup(4201);
    assert_eq!(firefox.unit.as_deref(), Some("app-firefox@3a1b.service"));
    assert_eq!(
        app_slice_unit(&firefox, 1000),
        Some("app-firefox@3a1b.service")
    );

    let root = cgroup(57);
    assert_eq!(&*root.rel, "");
    assert_eq!(root.unit, None);
    assert!(root.slices.is_empty());
    assert_eq!(app_slice_unit(&root, 1000), None);
}

#[test]
fn cgroup_unescape_x2d() {
    let mut out = String::from("stale");
    unescape_unit("app-chrome\\x2dwork@00bca6.service", &mut out);
    assert_eq!(out, "app-chrome-work@00bca6.service");
    unescape_unit("a\\x2Db\\x2fc", &mut out);
    assert_eq!(out, "a-b/c");
    for malformed in ["bad\\x2", "bad\\xg1", "bad\\", "bad\\y2d", "\\x"] {
        unescape_unit(malformed, &mut out);
        assert_eq!(out, malformed);
    }
}

#[test]
fn cgroup_placement_classes() {
    assert_eq!(
        placement(&cgroup(4205)),
        UserPlacement::InUserManager { uid: 1000 }
    );
    assert_eq!(
        placement(&cgroup(4206)),
        UserPlacement::InSessionScope { uid: 1000 }
    );
    let pipewire = cgroup(4207);
    assert_eq!(placement(&pipewire), UserPlacement::OutsideUserSlice);
    assert_eq!(pipewire.unit.as_deref(), Some("pipewire.service"));
    assert_eq!(placement(&cgroup(57)), UserPlacement::OutsideUserSlice);
    for other in ["0::/init.scope\n", "0::/machine.slice/vm.scope\n"] {
        let p = parse_cgroup_file(other.as_bytes()).expect("parse");
        assert_eq!(placement(&p), UserPlacement::OutsideUserSlice);
    }
    let mismatch =
        parse_cgroup_file(b"0::/user.slice/user-1000.slice/user@1001.service/app.slice/a.service")
            .expect("parse");
    assert_eq!(placement(&mismatch), UserPlacement::OutsideUserSlice);
    assert_eq!(app_slice_unit(&mismatch, 1000), None);
}

#[test]
fn cgroup_v1_only_is_none() {
    assert_eq!(parse_cgroup_file(b"12:cpu:/foo\n"), None);
    assert_eq!(parse_cgroup_file(b"12:cpu:/foo\n3:memory:/bar\n"), None);
    assert_eq!(parse_cgroup_file(b""), None);
    let hybrid = parse_cgroup_file(b"12:cpu:/foo\n0::/system.slice/sshd.service\n").expect("v2");
    assert_eq!(&*hybrid.rel, "system.slice/sshd.service");
}

#[test]
fn cgroup_uid_not_hard_coded() {
    let p = cgroup(4205);
    assert_eq!(app_slice_unit(&p, 1001), None);
    let other = parse_cgroup_file(
        b"0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-x.slice/app-y.scope\n",
    )
    .expect("parse");
    assert_eq!(
        placement(&other),
        UserPlacement::InUserManager { uid: 1001 }
    );
    assert_eq!(app_slice_unit(&other, 1001), Some("app-x.slice"));
    assert_eq!(app_slice_unit(&other, 1000), None);
}

use tl_collect::proc::cgroup::{CgroupRoot, CgroupStats, CgroupWant, memory_current_opens};

const FIREFOX_CGROUP: &str =
    "user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@3a1b.service";

const WANT_ALL: CgroupWant = CgroupWant {
    memory_stat: true,
    swap: true,
    memory_current: true,
    cpu_stat: true,
    io_stat: true,
    pids: true,
    events: true,
};

fn cgroup_root() -> CgroupRoot {
    CgroupRoot::open(&fixture_dir("process-edge").join("sys")).expect("open cgroup root")
}

/// A throwaway `sys/fs/cgroup/<name>` holding `files`. Returns the tempdir and the root.
fn cgroup_tmp(name: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, CgroupRoot) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("sys/fs/cgroup").join(name);
    fs::create_dir_all(&dir).expect("create cgroup dir");
    for (file, body) in files {
        fs::write(dir.join(file), body).expect("write cgroup file");
    }
    let root = CgroupRoot::open(&tmp.path().join("sys")).expect("open cgroup root");
    (tmp, root)
}

#[test]
fn cgroup_stats_app_scope() {
    let root = cgroup_root();
    let mut buf = Vec::new();
    let mut out = CgroupStats::default();
    root.read_stats(FIREFOX_CGROUP, WANT_ALL, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.anon, Some(524_288_000));
    assert_eq!(out.swap_current, Some(4096));
    assert_eq!(
        out.memory_current, None,
        "the fixture has no memory.current"
    );
    assert_eq!(out.usage_usec, Some(123_456_789));
    assert!(out.io_valid);
    assert_eq!(out.rbytes, 1_048_576 + 4096);
    assert_eq!(out.wbytes, 2_097_152 + 8192);
    assert_eq!(out.rios, 100 + 3);
    assert_eq!(out.wios, 200 + 5);
    assert_eq!(out.pids_current, Some(14));
    assert_eq!(out.frozen, Some(false));
    assert!(!out.denied);

    let ino = root.dir_ino(FIREFOX_CGROUP).expect("dir ino");
    assert_ne!(ino, 0);
    assert_ne!(ino, root.dir_ino("").expect("root ino"));
}

#[test]
fn cgroup_stats_missing_swap_is_none() {
    let (_tmp, root) = cgroup_tmp(
        "system.slice/sshd.service",
        &[("memory.stat", "anon 4096\n"), ("io.stat", "")],
    );
    let mut buf = Vec::new();
    let mut out = CgroupStats::default();
    root.read_stats("system.slice/sshd.service", WANT_ALL, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.swap_current, None);
    assert_eq!(out.anon, Some(4096));
    assert!(out.io_valid, "an empty io.stat is valid");
    assert_eq!((out.rbytes, out.wbytes, out.rios, out.wios), (0, 0, 0, 0));
    assert_eq!(out.usage_usec, None);
    assert_eq!(out.pids_current, None);
    assert_eq!(out.frozen, None);
    assert!(!out.denied);
}

#[test]
fn cgroup_stats_memory_current_not_opened() {
    let (_tmp, root) = cgroup_tmp("a.scope", &[("memory.current", "8192\n")]);
    let mut buf = Vec::new();
    let mut out = CgroupStats::default();
    let want = CgroupWant {
        memory_current: false,
        ..WANT_ALL
    };

    let before = memory_current_opens();
    let fixture = cgroup_root();
    fixture
        .read_stats(FIREFOX_CGROUP, want, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.memory_current, None);
    root.read_stats("a.scope", want, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.memory_current, None);
    assert_eq!(memory_current_opens(), before);

    root.read_stats("a.scope", WANT_ALL, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.memory_current, Some(8192));
    assert_eq!(memory_current_opens(), before + 1);
}

#[test]
fn cgroup_stats_missing_dir_is_error() {
    let root = cgroup_root();
    let mut buf = Vec::new();
    let mut out = CgroupStats::default();
    let err = root
        .read_stats("system.slice/nope.service", WANT_ALL, &mut buf, &mut out)
        .expect_err("missing dir");
    assert!(
        matches!(
            err,
            CollectError::ProcRead {
                file: "cgroup",
                errno
            } if errno == rustix::io::Errno::NOENT.raw_os_error()
        ),
        "{err:?}"
    );
    let mut pids = Vec::new();
    assert!(matches!(
        root.read_procs("system.slice/nope.service", true, &mut buf, &mut pids),
        Err(CollectError::ProcRead { file: "cgroup", .. })
    ));
}

#[test]
fn cgroup_procs_recursive() {
    let root = cgroup_root();
    let mut buf = Vec::new();
    let mut out = Vec::new();
    root.read_procs(FIREFOX_CGROUP, true, &mut buf, &mut out)
        .expect("read procs");
    assert_eq!(out, [4201, 4210]);

    out.clear();
    root.read_procs(FIREFOX_CGROUP, false, &mut buf, &mut out)
        .expect("read procs");
    assert_eq!(out, [4201]);
}

#[test]
fn cgroup_events_frozen() {
    let (_tmp, root) = cgroup_tmp("a.scope", &[("cgroup.events", "populated 1\nfrozen 1\n")]);
    let mut buf = Vec::new();
    let mut out = CgroupStats::default();
    root.read_stats("a.scope", WANT_ALL, &mut buf, &mut out)
        .expect("read stats");
    assert_eq!(out.frozen, Some(true));
}

use tl_collect::proc::meta::{
    ExeLink, META_REFRESH_SWEEPS, MetaCache, MetaChange, ReadBufs, meta_file_reads,
};

fn edge_dir(pid: u32) -> rustix::fd::OwnedFd {
    open_dir(&fixture_dir("process-edge").join(format!("proc/{pid}")))
}

#[test]
fn meta_first_read() {
    let dir = edge_dir(4201);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, change) = cache.get_or_refresh(
        rollup_key(4201, 351_220),
        dir.as_fd(),
        b"Web Content",
        &mut bufs,
    );
    assert_eq!(change, MetaChange::New);
    assert_eq!(&*meta.comm, "Web Content");
    assert_eq!(meta.cmdline.as_deref(), Some("firefox -P default"));
    let argv: Vec<&str> = meta.argv.iter().map(|a| &**a).collect();
    assert_eq!(argv, ["firefox", "-P", "default"]);
    assert_eq!(
        meta.exe,
        ExeLink::Path {
            path: "/usr/lib/firefox/firefox".into(),
            deleted: false
        }
    );
    let cgroup = meta.cgroup.as_ref().expect("cgroup");
    assert_eq!(cgroup.unit.as_deref(), Some("app-firefox@3a1b.service"));
    assert!(!meta.cgroup_denied);
}

#[test]
fn meta_refresh_every_10_sweeps() {
    let dir = edge_dir(4201);
    let mut bufs = ReadBufs::default();
    // The first refresh comes after 10 - (pid % 10) calls, then every 10 (R-064 as a maximum).
    for (pid, first) in [(4201, 9), (4200, 10), (4209, 1)] {
        let key = rollup_key(pid, 351_220);
        let mut cache = MetaCache::new();
        let new = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
        assert_eq!(new.1, MetaChange::New);
        let reads = meta_file_reads();
        for _ in 1..first {
            let (_, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
            assert_eq!(change, MetaChange::Unchanged, "pid {pid}");
        }
        assert_eq!(meta_file_reads(), reads, "no file re-read");
        let (meta, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
        assert_eq!(
            change,
            MetaChange::Refreshed,
            "pid {pid} after {first} calls"
        );
        assert_eq!(meta.cmdline.as_deref(), Some("firefox -P default"));
        assert!(meta_file_reads() > reads);
        for _ in 1..META_REFRESH_SWEEPS {
            let (_, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
            assert_eq!(change, MetaChange::Unchanged, "the counter restarts");
        }
        let (_, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
        assert_eq!(
            change,
            MetaChange::Refreshed,
            "then every {META_REFRESH_SWEEPS}"
        );

        cache.retain_live(&Default::default());
        assert!(cache.is_empty());
    }
}

#[test]
fn meta_refresh_spread_across_sweeps() {
    let dir = edge_dir(4201);
    let mut bufs = ReadBufs::default();
    let mut cache = MetaCache::new();
    let keys: Vec<ProcessKey> = (1000..1200).map(|pid| rollup_key(pid, 351_220)).collect();
    for &key in &keys {
        cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
    }
    let mut last = vec![0u32; keys.len()];
    for sweep in 1..=30u32 {
        let mut refreshed = 0;
        for (i, &key) in keys.iter().enumerate() {
            let (_, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
            if change == MetaChange::Refreshed {
                refreshed += 1;
                last[i] = sweep;
            }
            assert!(
                sweep - last[i] < META_REFRESH_SWEEPS,
                "no entry older than 10 sweeps"
            );
        }
        assert_eq!(
            refreshed,
            keys.len() / 10,
            "sweep {sweep}: a tenth refreshes"
        );
    }
}

#[test]
fn meta_refresh_on_comm_change() {
    let dir = edge_dir(4201);
    let key = rollup_key(4201, 351_220);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    assert_eq!(
        cache.get_or_refresh(key, dir.as_fd(), b"sh", &mut bufs).1,
        MetaChange::New
    );
    assert_eq!(
        cache.get_or_refresh(key, dir.as_fd(), b"bash", &mut bufs).1,
        MetaChange::Refreshed
    );
    assert_eq!(
        cache.get_or_refresh(key, dir.as_fd(), b"bash", &mut bufs).1,
        MetaChange::Unchanged
    );
}

#[test]
fn meta_deleted_exe_kept() {
    let dir = edge_dir(4208);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, _) = cache.get_or_refresh(rollup_key(4208, 1), dir.as_fd(), b"foo", &mut bufs);
    assert_eq!(
        meta.exe,
        ExeLink::Path {
            path: "/usr/bin/foo (deleted)".into(),
            deleted: true
        }
    );
}

#[test]
fn meta_cmdline_over_32k_truncated() {
    let dir = edge_dir(4208);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, _) = cache.get_or_refresh(rollup_key(4208, 1), dir.as_fd(), b"foo", &mut bufs);
    let cmdline = meta.cmdline.as_deref().expect("cmdline");
    assert_eq!(cmdline.chars().count(), 32_768 + 1);
    assert!(cmdline.starts_with("longarg aaa"));
    assert!(cmdline.ends_with("a\u{2026}"));
    assert_eq!(&*meta.argv[0], "longarg");
    assert_eq!(meta.argv[1].len(), 4096 + "\u{2026}".len());
}

#[test]
fn meta_kernel_thread_has_no_cmdline() {
    let dir = edge_dir(57);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, _) =
        cache.get_or_refresh(rollup_key(57, 140), dir.as_fd(), b"kworker/0:1", &mut bufs);
    assert_eq!(meta.cmdline, None);
    assert!(meta.argv.is_empty());
    assert_eq!(meta.exe, ExeLink::None);
    assert_eq!(&*meta.comm, "kworker/0:1");
}

#[test]
fn meta_comm_falls_back_to_stat() {
    // process-edge 4202 has a stat file but no comm file.
    let dir = edge_dir(4202);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, _) = cache.get_or_refresh(rollup_key(4202, 98_765), dir.as_fd(), b"a\nb", &mut bufs);
    assert_eq!(&*meta.comm, "a b");
    assert_eq!(meta.cmdline, None);
    assert_eq!(meta.cgroup, None);
    assert!(!meta.cgroup_denied);
}

use std::sync::atomic::{AtomicU64, Ordering};

use tl_collect::proc::fdcount::fd_count_fallbacks;
use tl_collect::proc::sweep::{IoState, SweepCore, SweepCtx, SweptProc};
use tl_model::validity::{ProcessField, Validity};

/// A settable clock for `SweepCore::with_clock`, one per test so parallel tests do not share it.
macro_rules! sweep_clock {
    ($name:ident) => {
        mod $name {
            use super::*;
            static BOOT: AtomicU64 = AtomicU64::new(0);
            static MONO: AtomicU64 = AtomicU64::new(0);
            pub fn now() -> (u64, u64) {
                (BOOT.load(Ordering::SeqCst), MONO.load(Ordering::SeqCst))
            }
            pub fn set(boot_ns: u64, mono_ns: u64) {
                BOOT.store(boot_ns, Ordering::SeqCst);
                MONO.store(mono_ns, Ordering::SeqCst);
            }
        }
    };
}

const SEC: u64 = 1_000_000_000;

fn sweep_ctx(want_fd_count: bool) -> SweepCtx {
    SweepCtx {
        boot_id: BootId(1),
        self_uid: 1000,
        ticks_per_sec: 100,
        online_cpus: 32,
        want_fd_count,
        want_status_fields: true,
        want_io: true,
    }
}

/// A proc root holding a copy of process-edge 4201 (stat, statm, status, io).
fn sweep_root() -> (tempfile::TempDir, SysRoot) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("proc/4201");
    fs::create_dir_all(&dir).expect("create proc dir");
    for name in ["stat", "statm", "status", "io"] {
        let rel = format!("proc/4201/{name}");
        fs::write(dir.join(name), fixture_bytes("process-edge", &rel)).expect("copy file");
    }
    let root = SysRoot::at(tmp.path());
    (tmp, root)
}

/// Rewrites stat fields 14 (utime), 15 (stime) and 22 (start_ticks) of the 4201 copy.
fn set_stat(root: &SysRoot, utime: u64, stime: u64, start_ticks: u64) {
    let line =
        String::from_utf8(fixture_bytes("process-edge", "proc/4201/stat")).expect("stat is UTF-8");
    let close = line.rfind(')').expect("stat has a close paren");
    let mut fields: Vec<String> = line[close + 2..]
        .trim_end()
        .split(' ')
        .map(str::to_owned)
        .collect();
    // fields[0] is field 3.
    fields[14 - 3] = utime.to_string();
    fields[15 - 3] = stime.to_string();
    fields[22 - 3] = start_ticks.to_string();
    let new = format!("{} {}\n", &line[..=close], fields.join(" "));
    fs::write(root.proc_path("4201/stat"), new).expect("write stat");
}

fn only(out: &[SweptProc]) -> &SweptProc {
    assert_eq!(out.len(), 1, "one process expected");
    &out[0]
}

sweep_clock!(cpu_rate_clock);

#[test]
fn sweep_cpu_rate_from_elapsed_time() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root.clone()).with_clock(cpu_rate_clock::now);
    let ctx = sweep_ctx(false);
    let mut out = Vec::new();

    set_stat(&root, 8812, 1204, 351_220);
    cpu_rate_clock::set(10 * SEC, 10 * SEC);
    core.sweep_core(&ctx, &mut out);

    set_stat(&root, 8812 + 30, 1204 + 20, 351_220);
    cpu_rate_clock::set(11 * SEC, 11 * SEC);
    let (start, end) = core.sweep_core(&ctx, &mut out);
    assert_eq!((start, end), (11 * SEC, 11 * SEC));

    let p = only(&out);
    assert_eq!(p.key.pid, 4201);
    assert_eq!(p.key.start_ticks, 351_220);
    assert!((p.cpu_pct - 1.5625).abs() < 0.001, "cpu_pct {}", p.cpu_pct);
    assert!((p.cpu_relative_pct - 50.0).abs() < 0.001);
    assert!((p.cpu_user_pct - 0.9375).abs() < 0.001);
    assert!((p.cpu_kernel_pct - 0.625).abs() < 0.001);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::Valid);
    // io did not change: a valid zero rate.
    assert_eq!(p.io_read_bps, 0.0);
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::IoWrite), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::RssAnon), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::Threads), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::Pss), Validity::WarmingUp);
    assert_eq!(p.statm.resident, 61_234);
}

#[test]
fn sweep_first_sample_warming_up() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root);
    let mut out = Vec::new();
    core.sweep_core(&sweep_ctx(false), &mut out);
    let p = only(&out);
    assert_eq!(p.cpu_pct, 0.0);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::WarmingUp);
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::WarmingUp);
    assert!(matches!(p.io, IoState::Valid(io) if io.read_bytes == 31_571_968));

    // reset_baselines makes the next sweep a first sample again.
    core.sweep_core(&sweep_ctx(false), &mut out);
    assert_eq!(only(&out).validity.get(ProcessField::Cpu), Validity::Valid);
    core.reset_baselines();
    core.sweep_core(&sweep_ctx(false), &mut out);
    assert_eq!(
        only(&out).validity.get(ProcessField::Cpu),
        Validity::WarmingUp
    );
}

sweep_clock!(reset_clock);

#[test]
fn sweep_counter_reset_warming_up() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root.clone()).with_clock(reset_clock::now);
    let ctx = sweep_ctx(false);
    let mut out = Vec::new();

    set_stat(&root, 8812, 1204, 351_220);
    reset_clock::set(10 * SEC, 10 * SEC);
    core.sweep_core(&ctx, &mut out);

    set_stat(&root, 100, 1204, 351_220);
    reset_clock::set(11 * SEC, 11 * SEC);
    core.sweep_core(&ctx, &mut out);
    let p = only(&out);
    assert_eq!(p.cpu_pct, 0.0);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::WarmingUp);

    set_stat(&root, 200, 1204, 351_220);
    reset_clock::set(12 * SEC, 12 * SEC);
    core.sweep_core(&ctx, &mut out);
    let p = only(&out);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::Valid);
    assert!((p.cpu_relative_pct - 100.0).abs() < 0.001);
}

sweep_clock!(suspend_clock);

/// ADR-008: a 5 s boottime gap with a 10 ms monotonic gap is a suspend and gives no rate.
#[test]
fn sweep_suspend_gap_no_rate() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root.clone()).with_clock(suspend_clock::now);
    let ctx = sweep_ctx(false);
    let mut out = Vec::new();

    set_stat(&root, 8812, 1204, 351_220);
    suspend_clock::set(10 * SEC, 10 * SEC);
    core.sweep_core(&ctx, &mut out);

    set_stat(&root, 8812 + 400, 1204, 351_220);
    suspend_clock::set(15 * SEC, 10 * SEC + 10_000_000);
    core.sweep_core(&ctx, &mut out);
    let p = only(&out);
    assert_eq!(p.cpu_pct, 0.0);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::WarmingUp);
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::WarmingUp);
}

#[test]
fn sweep_pid_reuse_new_key() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root.clone());
    let ctx = sweep_ctx(false);
    let mut out = Vec::new();

    set_stat(&root, 8812, 1204, 351_220);
    core.sweep_core(&ctx, &mut out);
    let old = only(&out).key;
    assert!(core.has_baseline(&old));

    set_stat(&root, 10, 5, 900_000);
    core.sweep_core(&ctx, &mut out);
    let p = only(&out);
    assert_ne!(p.key, old);
    assert_eq!(p.key.start_ticks, 900_000);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::WarmingUp);
    assert!(!core.has_baseline(&old));
    assert!(core.has_baseline(&p.key));
}

#[test]
fn sweep_vanished_pid_skipped() {
    let (tmp, root) = sweep_root();
    let proc_dir = tmp.path().join("proc");
    // Gone before openat: a dangling entry. Gone before stat: an empty directory.
    std::os::unix::fs::symlink(tmp.path().join("nowhere"), proc_dir.join("4299")).expect("symlink");
    fs::create_dir(proc_dir.join("4298")).expect("create dir");
    fs::create_dir(proc_dir.join("self")).expect("create dir");

    let mut core = SweepCore::new(root);
    let mut out = Vec::new();
    core.sweep_core(&sweep_ctx(false), &mut out);
    assert_eq!(only(&out).key.pid, 4201);

    // A missing proc directory gives an empty result.
    let mut core = SweepCore::new(SysRoot::at(&tmp.path().join("missing")));
    core.sweep_core(&sweep_ctx(false), &mut out);
    assert!(out.is_empty());
}

/// Security regression (ADR-016): an unreadable `io` gives Restricted, never a zero rate.
#[test]
fn sweep_io_restricted_not_zero() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let (_tmp, root) = sweep_root();
    fs::set_permissions(root.proc_path("4201/io"), fs::Permissions::from_mode(0o000))
        .expect("chmod");
    let mut core = SweepCore::new(root);
    let mut out = Vec::new();
    for _ in 0..2 {
        core.sweep_core(&sweep_ctx(false), &mut out);
        let p = only(&out);
        assert_eq!(p.io, IoState::Restricted);
        assert_eq!(p.validity.get(ProcessField::IoRead), Validity::Restricted);
        assert_eq!(p.validity.get(ProcessField::IoWrite), Validity::Restricted);
        assert_eq!(p.io_read_bps, 0.0);
    }
    assert_eq!(only(&out).validity.get(ProcessField::Cpu), Validity::Valid);
}

#[test]
fn sweep_fd_count_only_when_wanted() {
    let (_tmp, root) = sweep_root();
    // An empty `fd` file has st_size 0, so a count would take the readdir fallback, which
    // `fd_count_fallbacks` counts. No other test in this binary reads fd counts.
    fs::write(root.proc_path("4201/fd"), b"").expect("write fd");
    let mut core = SweepCore::new(root);
    let mut out = Vec::new();

    let before = fd_count_fallbacks();
    core.sweep_core(&sweep_ctx(false), &mut out);
    assert!(out.iter().all(|p| p.fd_count.is_none()));
    assert_eq!(fd_count_fallbacks(), before, "fd was stat-ed");

    core.sweep_core(&sweep_ctx(true), &mut out);
    assert!(only(&out).fd_count.is_some());
    assert_eq!(fd_count_fallbacks(), before + 1);
}

use tl_collect::proc::detail::{DetailRead, ThreadInfo, ThreadReader, WchanValue, thread_exists};

/// process-edge 4201, which has threads 4201 and 4211.
fn thread_key(start_ticks: u64) -> ProcessKey {
    rollup_key(4201, start_ticks)
}

fn thread_row(out: &[ThreadInfo], tid: u32) -> &ThreadInfo {
    out.iter()
        .find(|t| t.tid == tid)
        .unwrap_or_else(|| panic!("thread {tid} missing"))
}

/// Rewrites stat field 14 (utime) of thread 4211 under `root`.
fn set_thread_utime(root: &SysRoot, utime: u64) {
    let rel = "4201/task/4211/stat";
    let line = fs::read_to_string(root.proc_path(rel)).expect("read thread stat");
    let close = line.rfind(')').expect("stat has a close paren");
    let mut fields: Vec<&str> = line[close + 2..].trim_end().split(' ').collect();
    let utime = utime.to_string();
    // fields[0] is field 3.
    fields[14 - 3] = &utime;
    let new = format!("{} {}\n", &line[..=close], fields.join(" "));
    fs::write(root.proc_path(rel), new).expect("write thread stat");
}

sweep_clock!(thread_cpu_clock);

#[test]
fn threads_rows_and_cpu() {
    let tmp = common::fixture_copy("process-edge", None);
    let root = SysRoot::at(tmp.path());
    let mut reader = ThreadReader::new().with_clock(thread_cpu_clock::now);
    let mut bufs = ReadBufs::default();
    let mut out = Vec::new();
    let key = thread_key(FAKE_START_TICKS);

    thread_cpu_clock::set(10 * SEC, 10 * SEC);
    let r = reader.read(&root, key, 100, 32, &mut bufs, &mut out);
    assert_eq!(r.unwrap(), DetailRead::Ok);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|t| t.cpu_validity == Validity::WarmingUp));
    assert!(out.iter().all(|t| t.cpu_pct == 0.0));

    let t = thread_row(&out, 4211);
    assert_eq!(&*t.comm, "DOM Worker");
    assert_eq!(t.state, b'S');
    assert_eq!(t.utime_ms, 3000);
    assert_eq!(t.stime_ms, 500);
    assert_eq!(t.nice, -5);
    assert_eq!(t.policy, 0);
    assert_eq!(t.rt_prio, 0);
    assert_eq!(t.last_cpu, 7);
    assert_eq!(&*t.cpus, "8-11");
    assert_eq!(t.vol_ctx, Some(5120));
    assert_eq!(t.invol_ctx, Some(77));
    assert_eq!(&*thread_row(&out, 4201).comm, "Web Content");

    // 10 more ticks on 4211 over 1 s: 100 × 10 / 100 / 1 / 32 = 0.3125%.
    set_thread_utime(&root, 310);
    thread_cpu_clock::set(11 * SEC, 11 * SEC);
    let r = reader.read(&root, key, 100, 32, &mut bufs, &mut out);
    assert_eq!(r.unwrap(), DetailRead::Ok);
    assert_eq!(out.len(), 2);
    let t = thread_row(&out, 4211);
    assert_eq!(t.cpu_validity, Validity::Valid);
    assert!((t.cpu_pct - 0.3125).abs() < 1e-6, "cpu_pct {}", t.cpu_pct);
    assert_eq!(t.utime_ms, 3100);
    let main = thread_row(&out, 4201);
    assert_eq!(main.cpu_validity, Validity::Valid);
    assert_eq!(main.cpu_pct, 0.0);

    // A vanished thread drops out; its baseline goes with it.
    fs::remove_dir_all(root.proc_path("4201/task/4211")).expect("remove thread");
    thread_cpu_clock::set(12 * SEC, 12 * SEC);
    reader
        .read(&root, key, 100, 32, &mut bufs, &mut out)
        .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].tid, 4201);
    assert_eq!(out[0].cpu_validity, Validity::Valid);
}

#[test]
fn threads_wchan_values() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let mut reader = ThreadReader::new();
    let mut out = Vec::new();
    let r = reader.read(
        &root,
        thread_key(FAKE_START_TICKS),
        100,
        32,
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Ok);
    assert_eq!(
        thread_row(&out, 4201).wchan,
        WchanValue::Name("do_epoll_wait".into())
    );
    assert_eq!(thread_row(&out, 4211).wchan, WchanValue::Running);
}

/// Security regression (ADR-016): an unreadable `wchan` is Restricted and the row stays.
#[test]
fn threads_wchan_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let tmp = common::fixture_copy("process-edge", None);
    let root = SysRoot::at(tmp.path());
    fs::set_permissions(
        root.proc_path("4201/task/4211/wchan"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("chmod");
    let mut out = Vec::new();
    let r = ThreadReader::new().read(
        &root,
        thread_key(FAKE_START_TICKS),
        100,
        32,
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Ok);
    assert_eq!(out.len(), 2);
    let t = thread_row(&out, 4211);
    assert_eq!(t.wchan, WchanValue::Restricted);
    assert_eq!(&*t.comm, "DOM Worker");
}

/// Security regression (R-097): a pid whose start time differs is another process, so nothing
/// of it is read.
#[test]
fn threads_identity_mismatch_gone() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let mut reader = ThreadReader::new();
    let mut bufs = ReadBufs::default();
    let mut out = Vec::new();
    reader
        .read(
            &root,
            thread_key(FAKE_START_TICKS),
            100,
            32,
            &mut bufs,
            &mut out,
        )
        .unwrap();
    assert_eq!(out.len(), 2);

    let r = reader.read(
        &root,
        thread_key(FAKE_START_TICKS + 1),
        100,
        32,
        &mut bufs,
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Gone);
    assert!(out.is_empty());

    let r = reader.read(&root, rollup_key(4299, 1), 100, 32, &mut bufs, &mut out);
    assert_eq!(r.unwrap(), DetailRead::Gone);
    assert!(out.is_empty());
}

#[test]
fn threads_live_self() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let rx = rx.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let _ = rx.lock().expect("lock").recv();
            })
        })
        .collect();
    barrier.wait();

    let key = own_key();
    let mut out = Vec::new();
    let r = ThreadReader::new().read(
        &SysRoot::host(),
        key,
        rustix::param::clock_ticks_per_second(),
        1,
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Ok);
    assert!(out.len() >= 3, "{} threads", out.len());
    assert!(out.iter().any(|t| t.tid == key.pid));

    let dir = open_dir(&std::path::PathBuf::from(format!("/proc/{}", key.pid)));
    assert!(thread_exists(dir.as_fd(), key.pid));
    assert!(!thread_exists(dir.as_fd(), 999_999_999));

    drop(tx);
    for w in workers {
        w.join().expect("join worker");
    }
}

// ---- E05-024: full smaps ----

use tl_collect::proc::smaps::{SmapsRead, SmapsRegion, parse_smaps, read_smaps, rss_by_start};

#[test]
fn smaps_regions_parsed() {
    let mut out = Vec::new();
    parse_smaps(&fixture_bytes("process-edge", "proc/4201/smaps"), &mut out).expect("parse");
    assert_eq!(
        out,
        vec![
            SmapsRegion {
                start: 0x55d0_c1a2_e000,
                end: 0x55d0_c1b4_f000,
                rss: 1024 * 1024,
                pss: 1024 * 1024,
                swap: 8 * 1024,
            },
            SmapsRegion {
                start: 0x7f3a_1000_0000,
                end: 0x7f3a_1020_0000,
                rss: 2560 * 1024,
                pss: 512 * 1024,
                swap: 0,
            },
            SmapsRegion {
                start: 0x7f3a_2000_0000,
                end: 0x7f3a_2002_1000,
                rss: 512 * 1024,
                pss: 512 * 1024,
                swap: 16 * 1024,
            },
        ]
    );
    let by_start = rss_by_start(&out);
    assert_eq!(by_start.len(), 3);
    assert_eq!(by_start[&0x7f3a_1000_0000], 2560 * 1024);
}

#[test]
fn smaps_path_with_spaces() {
    let buf = b"7f00aa000000-7f00aa001000 r--p 00001000 08:01 42                         /opt/My App/lib.so\n\
Rss:                   4 kB\n\
Pss:                   2 kB\n\
Swap:                  0 kB\n";
    let mut out = Vec::new();
    parse_smaps(buf, &mut out).expect("parse");
    assert_eq!(
        out,
        vec![SmapsRegion {
            start: 0x7f00_aa00_0000,
            end: 0x7f00_aa00_1000,
            rss: 4096,
            pss: 2048,
            swap: 0,
        }]
    );
}

#[test]
fn smaps_bad_header_error() {
    let mut out = Vec::new();
    for buf in [
        &b"this is not a header line\nRss: 4 kB\n"[..],
        b"zzzz-1000 rw-p 00000000 00:00 0\n",
        b"1000-2000 rw-p\n",
        b"Rss: 4 kB\n",
    ] {
        let err = parse_smaps(buf, &mut out).unwrap_err();
        assert!(
            matches!(err, CollectError::ProcParse { file: "smaps", .. }),
            "{err:?}"
        );
    }
}

#[test]
fn smaps_self_has_rss() {
    let dir = open_dir(&std::path::PathBuf::from(format!(
        "/proc/{}",
        std::process::id()
    )));
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    assert_eq!(
        read_smaps(dir.as_fd(), &mut buf, &mut out).expect("read own smaps"),
        SmapsRead::Ok
    );
    assert!(out.iter().any(|r| r.rss > 0), "{} regions", out.len());
}

// ---- E05-026: file descriptors ----

use tl_collect::proc::detail::{FdInfo, FdType, access_text, decode_flags, read_fds};

fn fd_row(out: &[FdInfo], fd: u32) -> &FdInfo {
    out.iter()
        .find(|r| r.fd == fd)
        .unwrap_or_else(|| panic!("fd {fd} missing"))
}

/// The fds of process-edge 4201.
fn fixture_fds() -> Vec<FdInfo> {
    let mut out = Vec::new();
    let r = read_fds(
        &SysRoot::at(&fixture_dir("process-edge")),
        thread_key(FAKE_START_TICKS),
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.expect("read fixture fds"), DetailRead::Ok);
    out
}

#[test]
fn fd_flags_decode() {
    let w = decode_flags(0o2104002);
    assert_eq!(w.access, "Read/write");
    assert_eq!(w.status.as_slice(), ["Non-blocking"]);
    assert!(w.cloexec);

    let w = decode_flags(0o10002000);
    assert_eq!(w.access, "Path only");
    assert_eq!(w.status.as_slice(), ["Append", "Path"]);
    assert!(!w.cloexec);

    let w = decode_flags(0o4010000 | 0o40000 | 0o20200000 | 1);
    assert_eq!(w.access, "Write only");
    assert_eq!(w.status.as_slice(), ["Sync", "Direct", "Temp file"]);

    // O_DSYNC alone is not O_SYNC, and O_DIRECTORY alone is not O_TMPFILE.
    let w = decode_flags(0o10000 | 0o200000);
    assert_eq!(w.access, "Read only");
    assert!(w.status.is_empty());
}

#[test]
fn fd_access_text() {
    let mut s = String::new();
    access_text(0o2104002, &mut s);
    assert_eq!(s, "RW +NB CLOEXEC");
    access_text(0, &mut s);
    assert_eq!(s, "R");
    access_text(0o2001, &mut s);
    assert_eq!(s, "W +A");
}

#[test]
fn fd_kinds_classified() {
    let out = fixture_fds();
    let fds: Vec<u32> = out.iter().map(|r| r.fd).collect();
    assert_eq!(fds, [0, 1, 2, 3, 4, 12]);
    let kinds: Vec<FdType> = out.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            FdType::Socket,
            FdType::Pipe,
            FdType::EventFd,
            FdType::MemFd,
            FdType::Drm,
            FdType::File,
        ]
    );
    assert_eq!(fd_row(&out, 0).inode, Some(1234));
    assert_eq!(fd_row(&out, 1).inode, Some(99));
    assert_eq!(fd_row(&out, 0).kind.label(), "socket");
    assert_eq!(&*fd_row(&out, 3).target, "/memfd:wayland-shm (deleted)");
    assert_eq!(&*fd_row(&out, 12).target, "/home/user1/notes.txt");
    assert_eq!(fd_row(&out, 2).eventfd_count, Some(0x2a));
    let drm = fd_row(&out, 4);
    assert_eq!(drm.drm_driver.as_deref(), Some("amdgpu"));
    assert_eq!(drm.drm_client_id, Some(17));

    // Security regression (R-097): another start time is another process.
    let mut out = Vec::new();
    let r = read_fds(
        &SysRoot::at(&fixture_dir("process-edge")),
        thread_key(FAKE_START_TICKS + 1),
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Gone);
    assert!(out.is_empty());
}

#[test]
fn fdinfo_fields() {
    let out = fixture_fds();
    let f = fd_row(&out, 12);
    assert!(f.fdinfo_ok);
    assert_eq!(f.pos, Some(42));
    assert_eq!(f.flags, Some(0o2100000));
    assert_eq!(f.mnt_id, Some(31));
    assert_eq!(f.inode, Some(777));
    // The link inode wins over `ino:`.
    assert_eq!(fd_row(&out, 0).inode, Some(1234));
}

#[test]
fn fdinfo_missing_row_kept() {
    let out = fixture_fds();
    let p = fd_row(&out, 1);
    assert_eq!(p.kind, FdType::Pipe);
    assert!(!p.fdinfo_ok);
    assert_eq!(p.pos, None);
    assert_eq!(p.flags, None);
}

#[test]
fn fd_dir_denied_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let tmp = common::fixture_copy("process-edge", None);
    let root = SysRoot::at(tmp.path());
    let fd_dir = root.proc_path("4201/fd");
    fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o000)).expect("chmod");
    let mut out = Vec::new();
    let r = read_fds(
        &root,
        thread_key(FAKE_START_TICKS),
        &mut ReadBufs::default(),
        &mut out,
    );
    fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o755)).expect("chmod back");
    assert_eq!(r.unwrap(), DetailRead::Restricted);
    assert!(out.is_empty());
}

#[test]
fn fd_live_socket() {
    use std::os::fd::AsRawFd;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let fd = u32::try_from(listener.as_raw_fd()).expect("fd number");
    let mut out = Vec::new();
    let r = read_fds(
        &SysRoot::host(),
        own_key(),
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Ok);
    let row = fd_row(&out, fd);
    assert_eq!(row.kind, FdType::Socket);
    assert!(row.inode.is_some());
    assert!(row.fdinfo_ok);
    assert!(out.windows(2).all(|w| w[0].fd < w[1].fd));
}

// ---- E05-027: memory maps ----

use tl_collect::proc::detail::{MapNodeInfo, MapRegionInfo, group_maps, parse_maps, read_maps};

const MAPS_GROUPING: &[u8] = b"\
555555554000-555555556000 r--p 00000000 08:01 1001                       /usr/bin/foo
555555556000-55555555a000 r-xp 00002000 08:01 1001                       /usr/bin/foo
55555555a000-55555557b000 rw-p 00000000 00:00 0                          [heap]
7f0000000000-7f0000021000 rw-p 00000000 00:00 0\x20
7f0000100000-7f0000101000 rw-s 00000000 00:01 2002                       /memfd:x (deleted)
7f0000200000-7f0000228000 r--p 00000000 08:01 3003                       /usr/lib/libc.so.6
7f0000228000-7f00003bd000 r-xp 00028000 08:01 3003                       /usr/lib/libc.so.6
7f00003bd000-7f0000415000 r--p 001bd000 08:01 3003                       /usr/lib/libc.so.6
7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0                          [stack]
";

fn grouped(buf: &[u8], exe: Option<&str>) -> (Vec<MapRegionInfo>, Vec<MapNodeInfo>) {
    let (mut regions, mut nodes) = (Vec::new(), Vec::new());
    parse_maps(buf, &mut regions).expect("parse maps");
    group_maps(&mut regions, exe, None, &mut nodes);
    (regions, nodes)
}

fn region_sum(regions: &[MapRegionInfo], node: &MapNodeInfo) -> u64 {
    regions[node.regions.start as usize..node.regions.end as usize]
        .iter()
        .map(|r| r.end - r.start)
        .sum()
}

#[test]
fn maps_nodes_grouped_and_ordered() {
    let (regions, nodes) = grouped(MAPS_GROUPING, Some("/usr/bin/foo"));
    let names: Vec<&str> = nodes.iter().map(|n| &*n.name).collect();
    assert_eq!(
        names,
        ["foo (Primary)", "libc.so.6", "Outside loaded modules"]
    );
    assert!(nodes[0].primary && !nodes[0].outside && !nodes[0].shared_object);
    assert!(!nodes[1].primary && nodes[1].shared_object);
    assert!(nodes[2].outside && nodes[2].path.is_none());
    assert_eq!(
        nodes.iter().map(|n| n.regions.len()).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(nodes[0].size, 0x6000);
    assert_eq!(nodes[1].size, 0x215000);
    assert_eq!(nodes[2].size, 0x21000 + 0x21000 + 0x1000 + 0x21000);
    for node in &nodes {
        assert_eq!(node.size, region_sum(&regions, node), "{}", node.name);
        assert_eq!(node.base, regions[node.regions.start as usize].start);
        assert_eq!(node.rss, None);
    }
    assert_eq!(nodes[0].base, 0x5555_5555_4000);
    assert_eq!(&nodes[0].perms, b"r--p");
    assert_eq!(nodes[1].offset, 0);
    assert_eq!(
        nodes.last().map(|n| n.regions.end as usize),
        Some(regions.len())
    );
}

#[test]
fn maps_primary_deleted_exe() {
    let buf = b"555555554000-555555556000 r-xp 00000000 08:01 1001 /usr/bin/foo (deleted)\n";
    let (_, nodes) = grouped(buf, Some("/usr/bin/foo"));
    assert_eq!(&*nodes[0].name, "foo (Primary)");
    assert!(nodes[0].primary);
}

#[test]
fn maps_path_with_spaces() {
    let buf = b"7f0000000000-7f0000001000 r-xp 00000000 08:01 42      /opt/My App/libx.so\n";
    let mut regions = Vec::new();
    parse_maps(buf, &mut regions).expect("parse maps");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].path.as_deref(), Some("/opt/My App/libx.so"));
    assert_eq!(regions[0].inode, 42);
    assert_eq!(&regions[0].perms, b"r-xp");
}

#[test]
fn maps_wine_dll_shared_object() {
    let buf = b"\
7f0000000000-7f0000010000 r--p 00000000 08:01 7 /home/u/.wine/drive_c/windows/system32/kernel32.dll
7f0000010000-7f0000020000 r--p 00000000 08:01 8 /home/u/.wine/drive_c/Game/GAME.EXE
7f0000020000-7f0000030000 r--p 00000000 08:01 9 /home/u/data.bin
";
    let (_, nodes) = grouped(buf, None);
    assert_eq!(&*nodes[0].name, "kernel32.dll");
    assert!(nodes[0].shared_object);
    assert!(nodes[1].shared_object);
    assert!(!nodes[2].shared_object);
}

#[test]
fn maps_bad_line_is_parse_error() {
    let mut regions = Vec::new();
    let err = parse_maps(b"7f00-7e00 r-xp 0 08:01 1 /x\n", &mut regions).unwrap_err();
    assert!(matches!(err, CollectError::ProcParse { file: "maps", .. }));
    let err = parse_maps(b"7f00-7f10 r-x 0 08:01 1\n", &mut regions).unwrap_err();
    assert!(matches!(err, CollectError::ProcParse { file: "maps", .. }));
}

#[test]
fn maps_rss_summed() {
    let (mut regions, mut nodes) = (Vec::new(), Vec::new());
    let r = read_maps(
        &SysRoot::at(&fixture_dir("process-edge")),
        thread_key(FAKE_START_TICKS),
        true,
        &mut ReadBufs::default(),
        &mut regions,
        &mut nodes,
    );
    assert_eq!(r.expect("read fixture maps"), DetailRead::Ok);
    let names: Vec<&str> = nodes.iter().map(|n| &*n.name).collect();
    assert_eq!(names, ["libexample.so.1", "Outside loaded modules"]);
    assert_eq!(nodes[0].rss, Some(2560 * 1024));
    assert_eq!(nodes[1].rss, Some((1024 + 512) * 1024));
    for node in &nodes {
        let sum: u64 = regions[node.regions.start as usize..node.regions.end as usize]
            .iter()
            .map(|r| r.rss.expect("region rss"))
            .sum();
        assert_eq!(node.rss, Some(sum), "{}", node.name);
    }

    read_maps(
        &SysRoot::at(&fixture_dir("process-edge")),
        thread_key(FAKE_START_TICKS),
        false,
        &mut ReadBufs::default(),
        &mut regions,
        &mut nodes,
    )
    .expect("read fixture maps");
    assert!(nodes.iter().all(|n| n.rss.is_none()));
    assert!(regions.iter().all(|r| r.rss.is_none()));
}

#[test]
fn maps_wrong_start_ticks_gone() {
    let (mut regions, mut nodes) = (Vec::new(), Vec::new());
    let r = read_maps(
        &SysRoot::at(&fixture_dir("process-edge")),
        thread_key(FAKE_START_TICKS + 1),
        false,
        &mut ReadBufs::default(),
        &mut regions,
        &mut nodes,
    );
    assert_eq!(r.expect("read"), DetailRead::Gone);
    assert!(regions.is_empty() && nodes.is_empty());
}

#[test]
fn maps_live_self_has_primary() {
    let (mut regions, mut nodes) = (Vec::new(), Vec::new());
    let r = read_maps(
        &SysRoot::host(),
        own_key(),
        true,
        &mut ReadBufs::default(),
        &mut regions,
        &mut nodes,
    );
    assert_eq!(r.expect("read own maps"), DetailRead::Ok);
    assert!(nodes[0].primary, "first node {}", nodes[0].name);
    assert!(nodes.iter().filter(|n| n.primary).count() == 1);
    assert!(nodes.iter().any(|n| n.shared_object));
    assert!(nodes.iter().all(|n| n.rss.is_some()));
}

// ---- Process sampler (E05-A1) ----

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tl_collect::apps::AppPaths;
use tl_collect::apps::classify::{ProcClass, ProtectInputs};
use tl_collect::proc::meta::PasswdUserNames;
use tl_collect::proc::sweep::{ProcessSampler, SweepInputs, SweepOutput};
use tl_model::process::{GroupKind, ProcFlags, ProcessRecord};

const FIREFOX_UNIT: &str =
    "sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@3a1b.service";

/// Copies every entry of `src` over `dst`, replacing files that exist in both.
fn overlay(src: &Path, dst: &Path) {
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let kind = entry.file_type().expect("file type");
        if kind.is_dir() {
            fs::create_dir_all(&to).expect("create dir");
            overlay(&from, &to);
        } else if kind.is_symlink() {
            let _ = fs::remove_file(&to);
            std::os::unix::fs::symlink(fs::read_link(&from).expect("read link"), &to)
                .expect("create symlink");
        } else {
            fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// `owner-desktop` with `process-edge` copied over it. process-edge wins for pids in both.
fn merged_root() -> (tempfile::TempDir, SysRoot) {
    let tmp = tempfile::tempdir().expect("tempdir");
    overlay(&fixture_dir("owner-desktop"), tmp.path());
    overlay(&fixture_dir("process-edge"), tmp.path());
    let root = SysRoot::at(tmp.path());
    (tmp, root)
}

fn sampler_paths(cache: &Path) -> AppPaths {
    let share = |name: &str| fixture_dir(name).join("usr/share");
    let dirs: Vec<PathBuf> = vec![share("apps-desktop"), share("apps-icons")];
    AppPaths {
        home: fixture_dir("apps-icons").join("home"),
        xdg_data_home: None,
        xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
        xdg_config_home: None,
        icon_cache: cache.join("icons.toml"),
    }
}

fn sampler(root: SysRoot, cache: &Path) -> ProcessSampler {
    let users = PasswdUserNames::from_bytes(
        b"root:x:0:0::/root:/bin/sh\nuser1:x:1000:1000::/home/user1:/bin/sh\n",
    );
    ProcessSampler::new(root, &sampler_paths(cache), Box::new(users)).expect("open sampler")
}

fn no_inputs(protect: &ProtectInputs) -> SweepInputs<'_> {
    SweepInputs {
        protect,
        detail_keys: &[],
        want_pss: false,
        window_pids: None,
        wm_class: None,
    }
}

/// Runs `n` sweeps 1 s apart on the test clock and returns the last output.
fn sweep_n(s: &mut ProcessSampler, set: fn(u64, u64), n: u64) -> SweepOutput {
    let protect = ProtectInputs::default();
    let mut out = SweepOutput::default();
    for i in 1..=n {
        set(i * SEC, i * SEC);
        s.sweep(&sweep_ctx(false), &no_inputs(&protect), &mut out);
    }
    out
}

fn index_of(out: &SweepOutput, pid: u32) -> usize {
    out.records
        .iter()
        .position(|r| r.key.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} not swept"))
}

sweep_clock!(full_snapshot_clock);

#[test]
fn sweep_full_snapshot() {
    let (_tmp, root) = merged_root();
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(full_snapshot_clock::now);
    let out = sweep_n(&mut s, full_snapshot_clock::set, 2);

    assert_eq!(out.records.len(), out.extras.len());
    assert_eq!(out.records.len(), out.side.len());
    assert!(
        out.records.windows(2).all(|w| w[0].key < w[1].key),
        "records sorted by ProcessKey"
    );
    let rows: Vec<_> = out
        .records
        .iter()
        .zip(&out.side)
        .map(|(r, side)| {
            (
                r.key.pid,
                r.meta.name.to_string(),
                r.meta.user.to_string(),
                side.class,
                r.flags,
                r.state,
                r.threads,
                r.validity.get(ProcessField::Cpu),
                r.rss_anon_bytes,
            )
        })
        .collect();
    insta::assert_debug_snapshot!(rows);
}

sweep_clock!(app_member_clock);

#[test]
fn sweep_app_member_grouped() {
    let (_tmp, root) = merged_root();
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(app_member_clock::now);
    let out = sweep_n(&mut s, app_member_clock::set, 1);
    let i = index_of(&out, 4201);
    let firefox = GroupKind::App {
        app_id: Arc::from("firefox"),
    };
    assert_eq!(out.records[i].group, firefox.key());
    assert_eq!(out.side[i].class, ProcClass::App);
    assert!(out.side[i].is_app_primary);
    assert_eq!(&*out.records[i].meta.name, "Firefox");
    assert_eq!(out.records[i].meta.app_id.as_deref(), Some("firefox"));
    let desktop = out.records[i].meta.desktop_path.as_deref();
    assert!(
        desktop.is_some_and(|p| p.ends_with("/applications/firefox.desktop")),
        "firefox desktop path: {desktop:?}"
    );

    let unit = out
        .apps
        .iter()
        .find(|a| a.rel.ends_with("app-firefox@3a1b.service"))
        .expect("firefox unit sampled");
    assert_eq!(unit.group_key, firefox.key());
    assert_eq!(unit.app_name.as_deref(), Some("Firefox"));
    let model = unit.to_model();
    assert_eq!(model.group, firefox.key());
    assert_eq!(model.anon_bytes, 524_288_000);
    assert_eq!(model.pids, 14);
    assert_eq!(
        model.validity.get(ProcessField::IoRead),
        Validity::WarmingUp
    );
}

sweep_clock!(euid_clock);

#[test]
fn sweep_record_euid_only_when_it_differs() {
    let (tmp, root) = merged_root();
    // 4201 gets the setuid status of 4204 (Uid: 1000 0 0 0); 4209 keeps an equal euid.
    fs::copy(
        fixture_dir("process-edge").join("proc/4204/status"),
        tmp.path().join("proc/4201/status"),
    )
    .expect("copy status");
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(euid_clock::now);
    let out = sweep_n(&mut s, euid_clock::set, 1);

    let setuid = &out.records[index_of(&out, 4201)];
    assert_eq!(setuid.uid, 1000);
    assert_eq!(setuid.euid, Some(0));
    assert_eq!(out.records[index_of(&out, 4209)].euid, None);
}

sweep_clock!(cancelled_clock);

#[test]
fn sweep_record_cancelled_write_rate() {
    let (tmp, root) = merged_root();
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(cancelled_clock::now);
    let protect = ProtectInputs::default();
    let mut out = SweepOutput::default();

    cancelled_clock::set(SEC, SEC);
    s.sweep(&sweep_ctx(false), &no_inputs(&protect), &mut out);
    let first = &out.records[index_of(&out, 4201)];
    assert_eq!(first.cancelled_write_bps, 0.0);
    assert_eq!(
        first.validity.get(ProcessField::IoWrite),
        Validity::WarmingUp
    );

    // 2 MiB more written and 1 MiB more cancelled over 2 s.
    let io = String::from_utf8(fixture_bytes("process-edge", "proc/4201/io"))
        .expect("io UTF-8")
        .replace("write_bytes: 6627328", "write_bytes: 8724480")
        .replace(
            "cancelled_write_bytes: 1048576",
            "cancelled_write_bytes: 2097152",
        );
    fs::write(tmp.path().join("proc/4201/io"), io).expect("write io");
    cancelled_clock::set(3 * SEC, 3 * SEC);
    s.sweep(&sweep_ctx(false), &no_inputs(&protect), &mut out);

    let r = &out.records[index_of(&out, 4201)];
    assert_eq!(r.validity.get(ProcessField::IoWrite), Validity::Valid);
    assert_eq!(r.cancelled_write_bps, 524_288.0);
    assert_eq!(r.io_write_bps, 524_288.0);
}

sweep_clock!(kernel_thread_clock);

#[test]
fn sweep_kernel_thread_record() {
    let (_tmp, root) = merged_root();
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(kernel_thread_clock::now);
    let out = sweep_n(&mut s, kernel_thread_clock::set, 1);
    let r = &out.records[index_of(&out, 57)];
    assert_eq!(&*r.meta.name, "[kworker/0:1]");
    assert_eq!(r.meta.desktop_path, None);
    assert!(r.flags.contains(ProcFlags::KERNEL_THREAD));
    assert_eq!(r.validity.get(ProcessField::RssAnon), Validity::Unsupported);
    assert_eq!(out.side[index_of(&out, 57)].class, ProcClass::KernelThread);
}

sweep_clock!(other_user_clock);

/// Security regression (ADR-016): another user's unreadable `io` gives Restricted, not zero.
#[test]
fn sweep_other_user_flags_and_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let (_tmp, root) = merged_root();
    // owner-desktop pid 1 runs as root and has no io file in the capture.
    let io = root.proc_path("1/io");
    fs::write(&io, fixture_bytes("process-edge", "proc/4201/io")).expect("write io");
    fs::set_permissions(&io, fs::Permissions::from_mode(0o000)).expect("chmod");
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(other_user_clock::now);
    let out = sweep_n(&mut s, other_user_clock::set, 2);
    let r = &out.records[index_of(&out, 1)];
    assert_eq!(r.uid, 0);
    assert!(r.flags.contains(ProcFlags::OTHER_USER));
    assert!(r.flags.contains(ProcFlags::PROTECTED), "pid 1 is protected");
    assert_eq!(&*r.meta.user, "root");
    assert_eq!(r.validity.get(ProcessField::IoRead), Validity::Restricted);
    assert_eq!(r.validity.get(ProcessField::IoWrite), Validity::Restricted);
    assert_eq!(r.io_read_bps, 0.0);
}

sweep_clock!(meta_arc_clock);

#[test]
fn sweep_meta_arc_reused() {
    let (_tmp, root) = merged_root();
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(meta_arc_clock::now);
    let first = sweep_n(&mut s, meta_arc_clock::set, 1);
    let mut second = SweepOutput::default();
    meta_arc_clock::set(2 * SEC, 2 * SEC);
    let protect = ProtectInputs::default();
    s.sweep(&sweep_ctx(false), &no_inputs(&protect), &mut second);

    assert_eq!(first.records.len(), second.records.len());
    for (a, b) in first.records.iter().zip(&second.records) {
        assert_eq!(a.key, b.key);
        // pid % 10 == 9 is due its first (staggered) metadata refresh in sweep 2.
        if a.key.pid % 10 == 9 {
            continue;
        }
        assert!(
            Arc::ptr_eq(&a.meta, &b.meta),
            "pid {} got a new meta",
            a.key.pid
        );
    }
}

sweep_clock!(frozen_clock);

#[test]
fn sweep_frozen_app_members() {
    let (tmp, root) = merged_root();
    fs::write(
        tmp.path().join(FIREFOX_UNIT).join("cgroup.events"),
        "populated 1\nfrozen 1\n",
    )
    .expect("write cgroup.events");
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path()).with_clock(frozen_clock::now);
    let out = sweep_n(&mut s, frozen_clock::set, 1);

    assert!(out.extras[index_of(&out, 4201)].frozen);
    let members = [4201];
    for (r, x) in out.records.iter().zip(&out.extras) {
        if !members.contains(&r.key.pid) {
            assert!(!x.frozen, "pid {} is not in the frozen unit", r.key.pid);
        }
    }
    let unit = out
        .apps
        .iter()
        .find(|a| a.rel.ends_with("app-firefox@3a1b.service"))
        .expect("firefox unit sampled");
    assert_eq!(unit.stats.frozen, Some(true));
}

/// A `MakeWriter` that keeps every formatted line.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

sweep_clock!(never_logs_clock);

/// Security regression (ADR-015): a full sweep never logs a command line, at any level.
#[test]
fn sweep_never_logs_cmdline() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let (tmp, root) = merged_root();
    // A missing io file and an unreadable cmdline give the sweep things to log.
    let _ = fs::remove_file(tmp.path().join("proc/4209/io"));
    let out = tracing::subscriber::with_default(subscriber, || {
        tracing::trace!(probe = "yms1-capture-works");
        let cache = tempfile::tempdir().expect("tempdir");
        let mut s = sampler(root, cache.path()).with_clock(never_logs_clock::now);
        sweep_n(&mut s, never_logs_clock::set, 2)
    });

    let r = &out.records[index_of(&out, 4209)];
    assert!(
        r.meta
            .cmdline
            .as_deref()
            .is_some_and(|c| c.contains("secret-token-123")),
        "the fixture command line was read"
    );
    let text = String::from_utf8_lossy(&captured.0.lock().expect("capture lock")).into_owned();
    assert!(text.contains("yms1-capture-works"), "capture works");
    assert!(
        !text.contains("secret-token-123"),
        "a command line was logged"
    );
}

#[test]
fn sweep_pss_for_detail_keys() {
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(SysRoot::host(), cache.path());
    let key = own_key();
    let protect = ProtectInputs::default();
    let inp = SweepInputs {
        protect: &protect,
        detail_keys: &[key],
        want_pss: true,
        window_pids: None,
        wm_class: None,
    };
    let ctx = SweepCtx {
        self_uid: rustix::process::getuid().as_raw(),
        ..sweep_ctx(false)
    };
    let mut out = SweepOutput::default();
    s.sweep(&ctx, &inp, &mut out);

    let r: &ProcessRecord = out
        .records
        .iter()
        .find(|r| r.key == key)
        .expect("own process swept");
    assert_eq!(r.validity.get(ProcessField::Pss), Validity::Valid);
    assert_eq!(r.validity.get(ProcessField::Uss), Validity::Valid);
    assert!(r.pss_bytes > 0);
    assert!(out.coverage.has_pss_uss(&key));
    assert_eq!(out.coverage.pss_uss.len(), 1);
}

// ---- Sweep gate (E05-023) ----

#[path = "common/scaled_root.rs"]
mod scaled_root;

/// The R-286 gate: the median of 50 steady sweeps of the 2,000-process root with default columns
/// (no Swap, no I/O) is at most 20 ms. Owner machine, release build only:
/// `cargo test -p tl-collect --release --test fixture_process sweep_2000_under_gate -- --ignored`.
#[test]
#[ignore = "perf gate: owner machine, release build"]
fn sweep_2000_under_gate() {
    const GATE: std::time::Duration = std::time::Duration::from_millis(20);
    let mut scaled = scaled_root::scaled();
    let mut times: Vec<std::time::Duration> = (0..50)
        .map(|_| {
            let t = std::time::Instant::now();
            scaled.sweep(false);
            t.elapsed()
        })
        .collect();
    assert_eq!(scaled.out.records.len(), scaled_root::SCALED_PIDS);
    times.sort_unstable();
    let median = times[times.len() / 2];
    assert!(
        median <= GATE,
        "median steady sweep {median:?} over the {GATE:?} gate (min {:?}, max {:?})",
        times[0],
        times[times.len() - 1]
    );
}

#[test]
fn scaled_root_has_2000_pid_dirs() {
    let dir = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("tempdir");
    // Asserts the count itself.
    scaled_root::build_scaled_root(dir.path());
}

// ---- Fewer files per process (R-286) ----

use tl_collect::proc::sweep::{io_reads, status_reads};

/// A proc root with copies of process-edge 4201 (stat, statm, status, io) at pids 10 to 29.
fn stagger_root() -> (tempfile::TempDir, SysRoot, Vec<u32>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pids: Vec<u32> = (10..30).collect();
    for pid in &pids {
        let dir = tmp.path().join(format!("proc/{pid}"));
        fs::create_dir_all(&dir).expect("create proc dir");
        for name in ["stat", "statm", "status", "io"] {
            let rel = format!("proc/4201/{name}");
            fs::write(dir.join(name), fixture_bytes("process-edge", &rel)).expect("copy file");
        }
    }
    let root = SysRoot::at(tmp.path());
    (tmp, root, pids)
}

fn default_columns() -> SweepCtx {
    SweepCtx {
        want_status_fields: false,
        want_io: false,
        ..sweep_ctx(false)
    }
}

fn swept(out: &[SweptProc], pid: u32) -> &SweptProc {
    out.iter()
        .find(|p| p.key.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} not swept"))
}

sweep_clock!(rss_anon_clock);

#[test]
fn sweep_rss_anon_from_statm() {
    let (_tmp, root) = sweep_root();
    let mut core = SweepCore::new(root).with_clock(rss_anon_clock::now);
    let mut out = Vec::new();
    rss_anon_clock::set(SEC, SEC);
    core.sweep_core(&default_columns(), &mut out);
    let p = only(&out);
    // process-edge 4201 statm: size 666958, resident 61234, shared 60925, data 58.
    let page = rustix::param::page_size() as u64;
    assert_eq!(p.rss_anon_bytes, (61_234 - 60_925) * page);
    assert_eq!(p.rss_bytes, 61_234 * page);
    assert_eq!(p.rss_shared_bytes, 60_925 * page);
    assert_eq!(p.commit_bytes, 58 * page);
    assert_eq!(p.vsize_bytes, 666_958 * page);
    assert_eq!(p.validity.get(ProcessField::RssAnon), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::Rss), Validity::Valid);
    assert_eq!(
        p.status.vm_rss,
        Some(p.rss_bytes),
        "VmRSS is statm resident"
    );
}

sweep_clock!(stagger_clock);

#[test]
fn sweep_status_staggered() {
    let (_tmp, root, pids) = stagger_root();
    let mut core = SweepCore::new(root.clone()).with_clock(stagger_clock::now);
    let mut out = Vec::new();
    let ctx = default_columns();

    // Sweep 1: every pid is new, so every status is read.
    let before = status_reads();
    stagger_clock::set(SEC, SEC);
    core.sweep_core(&ctx, &mut out);
    assert_eq!(status_reads() - before, 20);
    assert!(out.iter().all(|p| p.status_fresh));
    assert_eq!(swept(&out, 13).status.vm_swap, Some(5_242_880));

    // Pid 13 swaps more. Sweep 2 reads only pids 12 and 22; 13 keeps its old value and validity.
    let status = String::from_utf8(fixture_bytes("process-edge", "proc/4201/status"))
        .expect("status is UTF-8")
        .replace("VmSwap:\t    5120 kB", "VmSwap:\t    8192 kB");
    fs::write(root.proc_path("13/status"), status).expect("write status");
    let before = status_reads();
    stagger_clock::set(2 * SEC, 2 * SEC);
    core.sweep_core(&ctx, &mut out);
    assert_eq!(status_reads() - before, 2);
    let mut fresh: Vec<u32> = out
        .iter()
        .filter(|p| p.status_fresh)
        .map(|p| p.key.pid)
        .collect();
    fresh.sort_unstable();
    assert_eq!(fresh, [12, 22]);
    let p13 = swept(&out, 13);
    assert_eq!(p13.status.vm_swap, Some(5_242_880));
    assert_eq!(p13.validity.get(ProcessField::Swap), Validity::Valid);
    assert_eq!(
        p13.status.uid[0], 1000,
        "the real uid comes from the last status read"
    );

    // Sweep 3 is pid 13's turn.
    stagger_clock::set(3 * SEC, 3 * SEC);
    core.sweep_core(&ctx, &mut out);
    let p13 = swept(&out, 13);
    assert!(p13.status_fresh);
    assert_eq!(p13.status.vm_swap, Some(8 * 1024 * 1024));

    // A demanded status-only field reads every pid.
    let before = status_reads();
    stagger_clock::set(4 * SEC, 4 * SEC);
    let demand = SweepCtx {
        want_status_fields: true,
        ..default_columns()
    };
    core.sweep_core(&demand, &mut out);
    assert_eq!(status_reads() - before, 20);
    assert_eq!(out.len(), pids.len());
}

sweep_clock!(io_demand_clock);

#[test]
fn sweep_io_on_demand() {
    let (_tmp, root, _pids) = stagger_root();
    let mut core = SweepCore::new(root).with_clock(io_demand_clock::now);
    let mut out = Vec::new();

    // Not demanded: no io file is opened and the rates are WarmingUp.
    let before = io_reads();
    io_demand_clock::set(SEC, SEC);
    core.sweep_core(&default_columns(), &mut out);
    io_demand_clock::set(2 * SEC, 2 * SEC);
    core.sweep_core(&default_columns(), &mut out);
    assert_eq!(io_reads() - before, 0);
    let p = swept(&out, 10);
    assert_eq!(p.io, IoState::WarmingUp);
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::WarmingUp);
    assert_eq!(p.validity.get(ProcessField::Cpu), Validity::Valid);

    // Turned on: read for every pid, WarmingUp for one interval, then Valid.
    let ctx = SweepCtx {
        want_io: true,
        ..default_columns()
    };
    let before = io_reads();
    io_demand_clock::set(3 * SEC, 3 * SEC);
    core.sweep_core(&ctx, &mut out);
    assert_eq!(io_reads() - before, 20);
    let p = swept(&out, 10);
    assert!(matches!(p.io, IoState::Valid(_)));
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::WarmingUp);

    io_demand_clock::set(4 * SEC, 4 * SEC);
    core.sweep_core(&ctx, &mut out);
    let p = swept(&out, 10);
    assert_eq!(p.validity.get(ProcessField::IoRead), Validity::Valid);
    assert_eq!(p.validity.get(ProcessField::IoWrite), Validity::Valid);
}

// ---- P2 per-process columns (E05-035) ----

use std::time::{Duration, Instant};

use tl_collect::pkg::{PackageIndex, PathOwners, PkgBackend, VerifyOutcome};
use tl_collect::proc::extras_p2::{ExtrasP2Reader, elf_reads};
use tl_collect::proc::meta::RawMeta;
use tl_collect::proc::sweep::PidDir;
use tl_model::process::{PackageInfo, PackageStatus, ProcessExtrasP2};

/// Package backend that owns nothing and counts `owners` calls.
#[derive(Default)]
struct NoPackages {
    calls: Arc<Mutex<usize>>,
}

impl PkgBackend for NoPackages {
    fn owners(&mut self, paths: &[&Path]) -> Result<PathOwners, CollectError> {
        *self.calls.lock().expect("calls lock") += 1;
        Ok(paths.iter().map(|p| (p.to_path_buf(), None)).collect())
    }
    fn info(&mut self, _name: &str) -> Result<Option<PackageInfo>, CollectError> {
        Ok(None)
    }
    fn verify(&mut self, _name: &str) -> Result<VerifyOutcome, CollectError> {
        Ok(VerifyOutcome::Unmodified)
    }
    fn db_stamp(&mut self) -> Option<i64> {
        Some(1)
    }
}

fn no_packages() -> PackageIndex {
    PackageIndex::new(Box::new(NoPackages::default()))
}

fn raw_meta(exe: ExeLink, cgroup: Option<CgroupPath>) -> RawMeta {
    RawMeta {
        comm: Arc::from("comm"),
        exe,
        cmdline: None,
        argv: Default::default(),
        cgroup,
        cgroup_denied: false,
    }
}

fn exe_path(path: &str) -> ExeLink {
    let deleted = path.ends_with(" (deleted)");
    ExeLink::Path {
        path: Arc::from(path),
        deleted,
    }
}

/// Opens `<root>/proc` and runs one `ExtrasP2Reader::read` for `pid` with the fixture stat and
/// status of that pid.
fn read_p2(
    reader: &mut ExtrasP2Reader,
    root: &SysRoot,
    pid: u32,
    meta: &RawMeta,
    pkg: &mut PackageIndex,
) -> ProcessExtrasP2 {
    let proc_fd = open_dir(&root.proc);
    let mut st = PidStat::default();
    parse_pid_stat(
        &fs::read(root.proc_path(&format!("{pid}/stat"))).expect("read stat"),
        &mut st,
    )
    .expect("parse stat");
    let mut status = PidStatus::default();
    let status_ok = fs::read(root.proc_path(&format!("{pid}/status")))
        .ok()
        .is_some_and(|b| parse_status(&b, &mut status).is_ok());
    reader.read(
        root,
        PidDir {
            proc_fd: proc_fd.as_fd(),
            pid,
        },
        rollup_key(pid, st.start_ticks),
        &st,
        status_ok.then_some(&status),
        meta,
        pkg,
        Instant::now(),
        &mut ReadBufs::default(),
    )
}

/// A copy of process-edge 4201 at `pid` under `tmp`.
fn copy_4201(tmp: &Path, pid: u32) {
    let dir = tmp.join(format!("proc/{pid}"));
    fs::create_dir_all(&dir).expect("create proc dir");
    for name in ["stat", "status"] {
        let rel = format!("proc/4201/{name}");
        fs::write(dir.join(name), fixture_bytes("process-edge", &rel)).expect("copy file");
    }
}

#[test]
fn extras_p2_basic_fields() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let mut reader = ExtrasP2Reader::new();
    let meta = raw_meta(ExeLink::None, None);
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut no_packages());
    assert_eq!(p.key.map(|k| k.pid), Some(4201));
    assert_eq!(p.session_id, Some(3));
    assert_eq!(p.oom_score, Some(667));
    assert_eq!(p.oom_score_adj, Some(200));
    assert_eq!(p.tty.as_deref(), Some("pts/1"));
    assert_eq!(p.cwd.as_deref(), Some("/home/user1"));
    assert_eq!(p.sched_policy_text.as_deref(), Some("Normal"));
    // Uid 1000 everywhere and CapEff 0.
    assert_eq!(p.elevated, Some(false));
    // The fixture has no /sys/kernel/security/lsm, so no major LSM is active.
    assert_eq!(p.security_label, None);
    assert_eq!(p.power_w, None);
    assert_eq!(p.power_trend_w, None);
}

#[test]
fn extras_p2_security_label_with_apparmor() {
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_4201(tmp.path(), 4201);
    let dir = tmp.path().join("proc/4201/attr");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("current"), b"firefox (enforce)\n\0").unwrap();
    fs::create_dir_all(tmp.path().join("sys/kernel/security")).unwrap();
    fs::write(
        tmp.path().join("sys/kernel/security/lsm"),
        b"capability,landlock,lockdown,yama,apparmor,bpf",
    )
    .unwrap();
    let root = SysRoot::at(tmp.path());
    let meta = raw_meta(ExeLink::None, None);
    let p = read_p2(
        &mut ExtrasP2Reader::new(),
        &root,
        4201,
        &meta,
        &mut no_packages(),
    );
    assert_eq!(p.security_label.as_deref(), Some("firefox (enforce)"));
}

#[test]
fn extras_p2_elf_cached() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut header = vec![0u8; 64];
    header[..5].copy_from_slice(b"\x7fELF\x02");
    header[18] = 62;
    let exe = tmp.path().join("foo");
    fs::write(&exe, &header).unwrap();
    for pid in [100, 101] {
        copy_4201(tmp.path(), pid);
        std::os::unix::fs::symlink(&exe, tmp.path().join(format!("proc/{pid}/exe"))).unwrap();
    }
    let root = SysRoot::at(tmp.path());
    let meta = raw_meta(exe_path(exe.to_str().unwrap()), None);
    let mut reader = ExtrasP2Reader::new();
    let mut pkg = no_packages();
    let before = elf_reads();
    let a = read_p2(&mut reader, &root, 100, &meta, &mut pkg);
    let b = read_p2(&mut reader, &root, 101, &meta, &mut pkg);
    assert_eq!(elf_reads(), before + 1, "header read once per (dev, inode)");
    for p in [&a, &b] {
        assert_eq!(p.elf_class, Some(64));
        assert_eq!(p.elf_machine, Some(62));
        assert_eq!(p.exe_size_bytes, Some(64));
        assert!(p.exe_mtime_s.is_some());
    }
}

#[test]
fn extras_p2_ctx_switch_delta() {
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_4201(tmp.path(), 4201);
    let write_tasks = |v: [u64; 2], n: [u64; 2]| {
        for (i, tid) in [1, 2].into_iter().enumerate() {
            let dir = tmp.path().join(format!("proc/4201/task/{tid}"));
            fs::create_dir_all(&dir).unwrap();
            let text = format!(
                "Name:\tt\nvoluntary_ctxt_switches:\t{}\nnonvoluntary_ctxt_switches:\t{}\n",
                v[i], n[i]
            );
            fs::write(dir.join("status"), text).unwrap();
        }
    };
    let root = SysRoot::at(tmp.path());
    let meta = raw_meta(ExeLink::None, None);
    let mut reader = ExtrasP2Reader::new();
    let mut pkg = no_packages();

    write_tasks([10, 5], [1, 2]);
    let first = read_p2(&mut reader, &root, 4201, &meta, &mut pkg);
    assert_eq!(first.ctx_switches_voluntary, Some(15));
    assert_eq!(first.ctx_switches_involuntary, Some(3));
    assert_eq!(first.ctx_switches_delta, None);

    write_tasks([20, 5], [1, 2]);
    let second = read_p2(&mut reader, &root, 4201, &meta, &mut pkg);
    assert_eq!(second.ctx_switches_voluntary, Some(25));
    assert_eq!(second.ctx_switches_involuntary, Some(3));
    assert_eq!(second.ctx_switches_delta, Some(10));

    // A dead key loses its baseline.
    reader.retain_live(&Default::default());
    let third = read_p2(&mut reader, &root, 4201, &meta, &mut pkg);
    assert_eq!(third.ctx_switches_delta, None);
}

#[test]
fn extras_p2_sandbox_seccomp() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // process-edge 4201 has Seccomp 2 and NoNewPrivs 1. PID 1 shares its user namespace.
    copy_4201(tmp.path(), 4201);
    for pid in [1, 4201] {
        let ns = tmp.path().join(format!("proc/{pid}/ns"));
        fs::create_dir_all(&ns).unwrap();
        std::os::unix::fs::symlink("user:[4026531837]", ns.join("user")).unwrap();
    }
    let root = SysRoot::at(tmp.path());
    let meta = raw_meta(ExeLink::None, Some(cgroup(4201)));
    let mut reader = ExtrasP2Reader::new();
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut no_packages());
    assert_eq!(p.sandbox.as_deref(), Some("seccomp + NoNewPrivs"));

    // A different user namespace than PID 1's is bwrap.
    let link = tmp.path().join("proc/4201/ns/user");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("user:[4026532000]", &link).unwrap();
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut no_packages());
    assert_eq!(p.sandbox.as_deref(), Some("bwrap"));

    // No readable ns/user: unknown, unless the cgroup names a Flatpak scope.
    fs::remove_file(&link).unwrap();
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut no_packages());
    assert_eq!(p.sandbox, None);
    let flatpak = CgroupPath {
        rel: Arc::from(
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.x.y-1.scope",
        ),
        unit: None,
        slices: Default::default(),
    };
    let meta = raw_meta(exe_path("/app/bin/y"), Some(flatpak));
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut no_packages());
    assert_eq!(p.sandbox.as_deref(), Some("Flatpak"));
    assert_eq!(p.package_status, Some(PackageStatus::Flatpak));
}

#[test]
fn extras_p2_deleted_exe_status() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let meta = raw_meta(exe_path("/usr/bin/foo (deleted)"), None);
    let mut pkg = no_packages();
    let p = read_p2(&mut ExtrasP2Reader::new(), &root, 4201, &meta, &mut pkg);
    assert_eq!(p.package_status, Some(PackageStatus::DeletedUpgraded));
    assert_eq!(p.package, None);
    assert!(!pkg.due(Instant::now() + Duration::from_secs(60)));
}

#[test]
fn extras_p2_package_miss_queues() {
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_4201(tmp.path(), 4201);
    let exe = tmp.path().join("bar");
    fs::write(&exe, b"#!/bin/sh\n").unwrap();
    std::os::unix::fs::symlink(&exe, tmp.path().join("proc/4201/exe")).unwrap();
    let root = SysRoot::at(tmp.path());
    let meta = raw_meta(exe_path(exe.to_str().unwrap()), None);
    let mut pkg = no_packages();
    let mut reader = ExtrasP2Reader::new();

    let p = read_p2(&mut reader, &root, 4201, &meta, &mut pkg);
    assert_eq!(p.package, None);
    assert_eq!(p.package_status, None);
    assert_eq!(p.elf_class, None, "not an ELF file");
    assert!(
        pkg.due(Instant::now() + Duration::from_secs(60)),
        "path queued"
    );

    // After the batch the index knows the path is not packaged.
    assert_eq!(pkg.run_batch().unwrap(), 1);
    let p = read_p2(&mut reader, &root, 4201, &meta, &mut pkg);
    assert_eq!(p.package_status, Some(PackageStatus::NotPackaged));
}

#[test]
fn extras_p2_only_detail_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for pid in [4201, 4202, 4203] {
        let dir = tmp.path().join(format!("proc/{pid}"));
        fs::create_dir_all(&dir).unwrap();
        for name in ["stat", "statm", "status", "comm", "cmdline", "sessionid"] {
            let bytes = fixture_bytes("process-edge", &format!("proc/4201/{name}"));
            let bytes = if name == "stat" {
                String::from_utf8(bytes)
                    .unwrap()
                    .replacen("4201", &pid.to_string(), 1)
                    .into_bytes()
            } else {
                bytes
            };
            fs::write(dir.join(name), bytes).unwrap();
        }
    }
    fs::create_dir_all(tmp.path().join("sys/fs/cgroup")).unwrap();
    let root = SysRoot::at(tmp.path());
    let cache = tempfile::tempdir().expect("tempdir");
    let mut s = sampler(root, cache.path());
    *s.pkg_mut() = no_packages();
    let protect = ProtectInputs::default();
    let key = rollup_key(4201, FAKE_START_TICKS);
    let inp = SweepInputs {
        detail_keys: &[key],
        ..no_inputs(&protect)
    };
    let mut out = SweepOutput::default();
    s.sweep(&sweep_ctx(false), &inp, &mut out);
    assert_eq!(out.records.len(), 3);
    assert_eq!(out.extras_p2.len(), 3);
    let at = index_of(&out, 4201);
    for (i, p) in out.extras_p2.iter().enumerate() {
        assert_eq!(p.key.is_some(), i == at, "entry {i}");
    }
    assert_eq!(out.extras_p2[at].key, Some(key));
    assert_eq!(out.extras_p2[at].session_id, Some(3));

    s.sweep(&sweep_ctx(false), &no_inputs(&protect), &mut out);
    assert_eq!(out.records.len(), 3);
    assert!(out.extras_p2.is_empty());
}

// ---- E05-028: security tab ----

use std::os::unix::fs::symlink;

use tl_collect::proc::detail::{
    ADMIN_GROUPS, CAPS, Elevated, LsmLabel, NsState, RISKY_CAPS, SecurityInfo, group_name,
    read_security,
};

/// A copy of process-edge with a `stat` for 4204 and 4212 (4201's, pid replaced). The committed
/// fixture has no `stat` for them, so the sweep tests do not see them.
fn security_root() -> (tempfile::TempDir, SysRoot) {
    let tmp = common::fixture_copy("process-edge", None);
    let stat = String::from_utf8(fixture_bytes("process-edge", "proc/4201/stat")).expect("utf-8");
    for pid in [4204, 4212] {
        let path = tmp.path().join(format!("proc/{pid}/stat"));
        fs::write(path, stat.replacen("4201", &pid.to_string(), 1)).expect("write stat");
    }
    let root = SysRoot::at(tmp.path());
    (tmp, root)
}

fn security(root: &SysRoot, pid: u32) -> SecurityInfo {
    let mut out = SecurityInfo::default();
    let r = read_security(
        root,
        rollup_key(pid, FAKE_START_TICKS),
        &[],
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.expect("read security"), DetailRead::Ok);
    out
}

fn replace_with_symlink(target: &Path, link: &Path) {
    if link.is_dir() && !link.is_symlink() {
        fs::remove_dir_all(link).expect("remove dir");
    } else {
        let _ = fs::remove_file(link);
    }
    symlink(target, link).expect("create symlink");
}

#[test]
fn security_elevated_root() {
    let (_tmp, root) = security_root();
    let s = security(&root, 4204);
    assert_eq!(s.uid, [1000, 0, 0, 0]);
    assert_ne!(s.cap_eff, 0);
    assert_eq!(s.elevated, Elevated::Root);
    assert_eq!(s.groups.as_slice(), [1000]);
    assert_eq!(s.no_new_privs, Some(false));
    assert_eq!(s.seccomp, Some(0));
    assert_eq!(&*s.speculation_store_bypass, "thread vulnerable");
    assert_eq!(&*s.speculation_indirect_branch, "conditional enabled");
    // 4204 has no `ns/` directory.
    assert_eq!(s.namespaces, [NsState::Restricted; 8]);
}

#[test]
fn security_capabilities_risky() {
    let (_tmp, root) = security_root();
    let s = security(&root, 4212);
    assert_eq!(s.uid, [1000; 4]);
    assert_eq!(s.cap_eff, 0x3000);
    assert_eq!(s.elevated, Elevated::Capabilities);
    let held = s.cap_eff | s.cap_amb;
    let risky: Vec<&str> = RISKY_CAPS
        .iter()
        .filter(|&&bit| held >> bit & 1 == 1)
        .map(|&bit| CAPS[usize::from(bit)].0)
        .collect();
    assert_eq!(risky, ["NET_ADMIN"]);
    let effective: Vec<&str> = (0..CAPS.len())
        .filter(|&bit| s.cap_eff >> bit & 1 == 1)
        .map(|bit| CAPS[bit].0)
        .collect();
    assert_eq!(effective, ["NET_ADMIN", "NET_RAW"]);
}

#[test]
fn security_elevated_setuid_and_no() {
    let (tmp, root) = security_root();
    let status = tmp.path().join("proc/4212/status");
    let text = fs::read_to_string(&status).unwrap();
    let no_caps = text.replace("CapEff:\t0000000000003000", "CapEff:\t0000000000000000");
    fs::write(&status, &no_caps).unwrap();
    assert_eq!(security(&root, 4212).elevated, Elevated::No);
    fs::write(
        &status,
        no_caps.replace("Uid:\t1000\t1000", "Uid:\t1000\t1001"),
    )
    .unwrap();
    assert_eq!(security(&root, 4212).elevated, Elevated::Setuid);
}

#[test]
fn security_loginuid_unset() {
    let (_tmp, root) = security_root();
    let s = security(&root, 4204);
    assert_eq!(s.loginuid, None);
    assert_eq!(s.sessionid, None);
    let s = security(&root, 4212);
    assert_eq!(s.loginuid, Some(1000));
    assert_eq!(s.sessionid, Some(3));
}

#[test]
fn security_ns_shared_and_isolated() {
    let (_tmp, root) = security_root();
    let s = security(&root, 4212);
    assert_eq!(s.namespaces[2], NsState::SharedWithInit, "net");
    assert_eq!(s.namespaces[1], NsState::Isolated(4_026_532_300), "pid");
    for i in [0, 3, 4, 5, 6, 7] {
        assert_eq!(s.namespaces[i], NsState::SharedWithInit, "namespace {i}");
    }
}

#[test]
fn security_ns_same_as_tasklens() {
    let (tmp, root) = security_root();
    fs::remove_dir_all(tmp.path().join("proc/1")).unwrap();
    let s = security(&root, 4212);
    assert_eq!(s.namespaces[1], NsState::SameAsTasklens, "pid");
    assert_eq!(s.namespaces[0], NsState::SameAsTasklens, "user");
    // tasklens's own net namespace differs in the fixture.
    assert_eq!(s.namespaces[2], NsState::Isolated(4_026_531_840), "net");

    // `self` as a symlink to a numeric sibling, as on the host.
    let proc = tmp.path().join("proc");
    fs::rename(proc.join("self"), proc.join("9999")).unwrap();
    symlink("9999", proc.join("self")).unwrap();
    let s = security(&root, 4212);
    assert_eq!(
        s.namespaces[1],
        NsState::SameAsTasklens,
        "pid via self link"
    );
}

#[test]
fn security_lsm_not_applicable() {
    let (tmp, root) = security_root();
    assert_eq!(security(&root, 4212).lsm_label, LsmLabel::NotApplicable);

    let lsm = tmp.path().join("sys/kernel/security/lsm");
    fs::write(&lsm, "capability,landlock,lockdown,yama,apparmor,bpf\n").unwrap();
    assert_eq!(
        security(&root, 4212).lsm_label,
        LsmLabel::Label(Arc::from("unconfined"))
    );
    // An active LSM and no `attr/current` reads as Restricted.
    assert_eq!(security(&root, 4204).lsm_label, LsmLabel::Restricted);

    fs::remove_file(&lsm).unwrap();
    assert_eq!(security(&root, 4212).lsm_label, LsmLabel::NotApplicable);
}

#[test]
fn security_group_name() {
    let group = fixture_bytes("process-edge", "etc/group");
    assert_eq!(group_name(968, &group).as_deref(), Some("docker"));
    assert_eq!(group_name(998, &group).as_deref(), Some("wheel"));
    assert_eq!(group_name(0, &group).as_deref(), Some("root"));
    assert_eq!(group_name(4242, &group), None);
    assert_eq!(group_name(5, b"bad line\n:x:5:\nx:5\n"), None);
    assert_eq!(
        group_name(7, b"ev\x1b[31mil\n:x:7:\nev\x1bil:x:7:\n").as_deref(),
        Some("ev\u{FFFD}il")
    );
}

#[test]
fn caps_table_complete() {
    assert_eq!(CAPS.len(), 41);
    assert_eq!(CAPS[21], ("SYS_ADMIN", "Broad system administration"));
    assert_eq!(CAPS[0].0, "CHOWN");
    assert_eq!(CAPS[40].0, "CHECKPOINT_RESTORE");
    let names: std::collections::HashSet<&str> = CAPS.iter().map(|c| c.0).collect();
    assert_eq!(names.len(), CAPS.len(), "names are unique");
    assert!(
        CAPS.iter()
            .all(|(n, d)| !n.starts_with("CAP_") && !d.is_empty())
    );
    let risky: Vec<&str> = RISKY_CAPS.iter().map(|&b| CAPS[usize::from(b)].0).collect();
    assert_eq!(
        risky,
        [
            "SYS_ADMIN",
            "SYS_PTRACE",
            "DAC_OVERRIDE",
            "DAC_READ_SEARCH",
            "SYS_MODULE",
            "SYS_RAWIO",
            "SETUID",
            "SETGID",
            "NET_ADMIN",
            "BPF",
            "PERFMON",
            "SYS_BOOT",
            "CHOWN",
            "FOWNER"
        ]
    );
    let root_equivalent: Vec<&str> = ADMIN_GROUPS.iter().filter(|g| g.1).map(|g| g.0).collect();
    assert_eq!(root_equivalent, ["docker", "libvirt", "lxd", "disk"]);
}

/// A proc root with only `<pid>/stat` and `<pid>/status` (`Uid: 1000 1000 1000 1000`).
fn dumpable_root() -> (tempfile::TempDir, SysRoot, PathBuf) {
    let (tmp, root) = security_root();
    let dir = tmp.path().join("proc/4212");
    (tmp, root, dir)
}

#[test]
fn security_dumpable_rule() {
    let (_tmp, root, dir) = dumpable_root();
    let owner = std::os::unix::fs::MetadataExt::uid(&fs::metadata(&dir).unwrap());
    if owner == 0 {
        // Running as root: the tempdir is root-owned, so the rule fires.
        assert_eq!(security(&root, 4212).dumpable, Some(false));
    } else {
        assert_eq!(security(&root, 4212).dumpable, None);
    }
}

#[test]
#[ignore = "needs root to chown the fixture directory to uid 0"]
fn security_dumpable_rule_root_owned() {
    let (tmp, root, dir) = dumpable_root();
    std::os::unix::fs::chown(&dir, Some(0), None).expect("chown needs root");
    assert_eq!(security(&root, 4212).dumpable, Some(false));
    // A real uid of 0 is never reported.
    let status = dir.join("status");
    let text = fs::read_to_string(&status).unwrap();
    fs::write(&status, text.replace("Uid:\t1000\t1000", "Uid:\t0\t1000")).unwrap();
    assert_eq!(security(&root, 4212).dumpable, None);
    drop(tmp);
}

#[test]
fn security_symlinks_not_followed() {
    let (tmp, root) = security_root();
    let outside = tempfile::tempdir().expect("tempdir");
    fs::write(outside.path().join("label"), "stolen\n").unwrap();
    fs::write(outside.path().join("id"), "0\n").unwrap();
    fs::write(
        outside.path().join("status"),
        fixture_bytes("process-edge", "proc/4204/status"),
    )
    .unwrap();
    fs::create_dir(outside.path().join("ns")).unwrap();
    symlink("pid:[4026532300]", outside.path().join("ns/pid")).unwrap();
    fs::write(
        tmp.path().join("sys/kernel/security/lsm"),
        "capability,selinux\n",
    )
    .unwrap();

    let pid_dir = tmp.path().join("proc/4212");
    replace_with_symlink(&outside.path().join("label"), &pid_dir.join("attr/current"));
    replace_with_symlink(&outside.path().join("id"), &pid_dir.join("loginuid"));
    replace_with_symlink(&outside.path().join("ns"), &pid_dir.join("ns"));
    let s = security(&root, 4212);
    assert_eq!(s.lsm_label, LsmLabel::Restricted);
    assert_eq!(s.loginuid, None);
    assert_eq!(s.namespaces, [NsState::Restricted; 8]);

    // `attr` itself as a symlink.
    replace_with_symlink(outside.path(), &pid_dir.join("attr"));
    assert_eq!(security(&root, 4212).lsm_label, LsmLabel::Restricted);

    // A symlinked PID 1 is not read, so the comparison falls back to tasklens.
    fs::remove_file(pid_dir.join("ns")).unwrap();
    fs::create_dir(pid_dir.join("ns")).unwrap();
    symlink("pid:[4026532300]", pid_dir.join("ns/pid")).unwrap();
    replace_with_symlink(outside.path(), &tmp.path().join("proc/1"));
    let s = security(&root, 4212);
    assert_eq!(s.namespaces[1], NsState::SameAsTasklens);

    // `self` pointing out of the root is not followed, so there is no reference.
    let proc = tmp.path().join("proc");
    replace_with_symlink(outside.path(), &proc.join("self"));
    let s = security(&root, 4212);
    assert_eq!(s.namespaces[1], NsState::Restricted);

    // A symlinked `status` is an error, never a read of the target.
    replace_with_symlink(&outside.path().join("status"), &pid_dir.join("status"));
    let mut out = SecurityInfo::default();
    let r = read_security(
        &root,
        rollup_key(4212, FAKE_START_TICKS),
        &[],
        &mut ReadBufs::default(),
        &mut out,
    );
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn security_large_and_bad_files() {
    let (tmp, root) = security_root();
    let pid_dir = tmp.path().join("proc/4212");
    fs::write(pid_dir.join("loginuid"), "1".repeat(1 << 20)).unwrap();
    fs::write(pid_dir.join("sessionid"), "12x").unwrap();
    fs::write(pid_dir.join("attr/current"), "a".repeat(1 << 20)).unwrap();
    fs::write(tmp.path().join("sys/kernel/security/lsm"), "smack").unwrap();
    for (i, name) in ["user", "pid", "net"].iter().enumerate() {
        let link = pid_dir.join("ns").join(name);
        fs::remove_file(&link).unwrap();
        let target = match i {
            0 => "pid:[4026531837]".to_string(),
            1 => format!("pid:[{}]", "9".repeat(200)),
            _ => "net:[]".to_string(),
        };
        symlink(target, &link).unwrap();
    }
    let s = security(&root, 4212);
    assert_eq!(s.loginuid, None);
    assert_eq!(s.sessionid, None);
    match &s.lsm_label {
        LsmLabel::Label(l) => {
            assert!(l.ends_with('\u{2026}'));
            assert!(l.len() <= 4096 + 3);
        }
        other => panic!("label {other:?}"),
    }
    assert_eq!(s.namespaces[..3], [NsState::Restricted; 3]);
    assert_eq!(s.namespaces[3], NsState::SharedWithInit);

    // A FIFO does not block the read.
    fs::remove_file(pid_dir.join("loginuid")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        pid_dir.join("loginuid"),
        rustix::fs::FileType::Fifo,
        Mode::from_raw_mode(0o644),
        0,
    )
    .unwrap();
    assert_eq!(security(&root, 4212).loginuid, None);
}

#[test]
fn security_gone_and_bad_status() {
    let (tmp, root) = security_root();
    let mut out = SecurityInfo::default();
    let mut bufs = ReadBufs::default();
    let r = read_security(
        &root,
        rollup_key(4212, FAKE_START_TICKS + 1),
        &[],
        &mut bufs,
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Gone);
    assert_eq!(out, SecurityInfo::default());
    let r = read_security(&root, rollup_key(7777, 1), &[], &mut bufs, &mut out);
    assert_eq!(r.unwrap(), DetailRead::Gone);

    fs::remove_file(tmp.path().join("proc/4212/status")).unwrap();
    let r = read_security(
        &root,
        rollup_key(4212, FAKE_START_TICKS),
        &[],
        &mut bufs,
        &mut out,
    );
    assert_eq!(r.unwrap(), DetailRead::Gone);

    fs::write(tmp.path().join("proc/4212/status"), b"\xff\xfe junk").unwrap();
    let r = read_security(
        &root,
        rollup_key(4212, FAKE_START_TICKS),
        &[],
        &mut bufs,
        &mut out,
    );
    assert!(matches!(
        r,
        Err(CollectError::ProcParse {
            pid: 4212,
            file: "status"
        })
    ));
}

#[test]
fn security_live_self() {
    let mut out = SecurityInfo::default();
    let r = read_security(
        &SysRoot::host(),
        own_key(),
        &[],
        &mut ReadBufs::default(),
        &mut out,
    );
    assert_eq!(r.expect("read own security"), DetailRead::Ok);
    let uid = rustix::process::getuid().as_raw();
    assert_eq!(out.uid[0], uid);
    // user, pid and net exist on every supported kernel and are readable for tasklens itself.
    for i in 0..3 {
        assert!(
            matches!(
                out.namespaces[i],
                NsState::SharedWithInit | NsState::SameAsTasklens | NsState::Isolated(_)
            ),
            "namespace {i}: {:?}",
            out.namespaces[i]
        );
    }
    if uid != 0 {
        assert_eq!(out.dumpable, None);
    }
}

#[test]
fn security_ns_no_reference_restricted() {
    let (tmp, root) = security_root();
    let proc = tmp.path().join("proc");
    fs::remove_dir_all(proc.join("1")).unwrap();
    // `self` has a pid link only, so the other namespaces have no reference.
    for name in ["user", "net", "mnt", "ipc", "uts", "cgroup", "time"] {
        fs::remove_file(proc.join("self/ns").join(name)).unwrap();
    }
    let s = security(&root, 4212);
    assert_eq!(s.namespaces[1], NsState::SameAsTasklens, "pid");
    for i in [0, 2, 3, 4, 5, 6, 7] {
        assert_eq!(s.namespaces[i], NsState::Restricted, "namespace {i}");
    }

    // No `self` at all: every namespace is Restricted.
    fs::remove_dir_all(proc.join("self")).unwrap();
    assert_eq!(security(&root, 4212).namespaces, [NsState::Restricted; 8]);
}

// ---- Handle walk and pipe peers (E05-031) ----

use rustc_hash::{FxHashMap, FxHashSet};
use tl_collect::proc::handles::{
    HandleHitInfo, HandleWalk, HitKind, PipePeers, StepResult, find_pipe_peers,
};

/// Runs a walk over `root` to `Done` and returns its hits, sorted by pid, kind and path.
fn walk_all(root: &SysRoot, pattern: &str, include_maps: bool) -> (Vec<HandleHitInfo>, HandleWalk) {
    let mut walk = HandleWalk::start(root, pattern, include_maps);
    let mut bufs = ReadBufs::default();
    let mut hits = Vec::new();
    while walk.step(root, BootId(7), 64, &mut bufs, &mut hits) == StepResult::More {}
    hits.sort_by(|a, b| (a.pid, a.kind.label(), &a.path).cmp(&(b.pid, b.kind.label(), &b.path)));
    (hits, walk)
}

fn hit_summary(hits: &[HandleHitInfo]) -> Vec<(u32, HitKind, Option<u32>, &str)> {
    hits.iter()
        .map(|h| (h.pid, h.kind, h.fd, &*h.path))
        .collect()
}

#[test]
fn handles_substring_hit() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let (hits, walk) = walk_all(&root, "notes", true);
    assert_eq!(
        hit_summary(&hits),
        [(4201, HitKind::Fd, Some(12), "/home/user1/notes.txt")]
    );
    let hit = &hits[0];
    assert_eq!(&*hit.comm, "Web Content");
    assert_eq!(
        hit.key,
        ProcessKey {
            start_ticks: 351_220,
            boot_id: BootId(7),
            pid: 4201,
        }
    );
    // The other process-edge pids have no `stat` or no `fd/`, so they count as gone.
    let p = walk.progress();
    assert_eq!((p.searched, p.total, p.skipped), (2, 2, 0));
    // 4201 has 6 fds and 4214 has 2.
    assert_eq!(p.handles, 8);
}

#[test]
fn handles_glob_case_insensitive() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    for include_maps in [false, true] {
        let (hits, _) = walk_all(&root, "*.TXT", include_maps);
        assert_eq!(
            hit_summary(&hits),
            [(4201, HitKind::Fd, Some(12), "/home/user1/notes.txt")]
        );
    }
    // A glob covers the whole path, so a glob without a leading `*` finds nothing here.
    let (hits, _) = walk_all(&root, "notes.t?t", true);
    assert!(hits.is_empty());
    let (hits, _) = walk_all(&root, "/HOME/*/NOTES.T?T", true);
    assert_eq!(hits.len(), 1);
}

#[test]
fn handles_include_maps() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let (hits, _) = walk_all(&root, "user1", true);
    assert_eq!(
        hit_summary(&hits),
        [
            (4201, HitKind::Cwd, None, "/home/user1"),
            (4201, HitKind::Fd, Some(12), "/home/user1/notes.txt"),
        ]
    );

    // `/usr/bin/cat` is mapped twice but gives one Mmap hit.
    let (hits, _) = walk_all(&root, "bin/cat", true);
    assert_eq!(
        hit_summary(&hits),
        [
            (4214, HitKind::Exe, None, "/usr/bin/cat"),
            (4214, HitKind::Mmap, None, "/usr/bin/cat"),
        ]
    );

    let (hits, _) = walk_all(&root, "libexample", true);
    assert_eq!(
        hit_summary(&hits),
        [(4201, HitKind::Mmap, None, "/usr/lib/libexample.so.1")]
    );

    // `/` matches both root links and every other path.
    let (hits, _) = walk_all(&root, "/", true);
    let roots: Vec<u32> = hits
        .iter()
        .filter(|h| h.kind == HitKind::Root)
        .map(|h| h.pid)
        .collect();
    assert_eq!(roots, [4201, 4214]);
    assert_eq!(HitKind::Root.label(), "root");
    assert_eq!(HitKind::Mmap.label(), "mmap");
}

#[test]
fn handles_exclude_maps() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    for pattern in ["user1", "bin/cat", "libexample", "/", ""] {
        let (hits, _) = walk_all(&root, pattern, false);
        assert!(
            hits.iter().all(|h| h.kind == HitKind::Fd),
            "{pattern}: {hits:?}"
        );
    }
    // An empty pattern matches every fd.
    let (hits, _) = walk_all(&root, "", false);
    assert_eq!(hits.len(), 8);
}

#[test]
fn handles_step_budget() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let mut walk = HandleWalk::start(&root, "pipe", false);
    let pids = walk.progress().total;
    assert!(pids > 2, "{pids}");
    let mut bufs = ReadBufs::default();
    let mut hits = Vec::new();
    let mut calls = 0;
    loop {
        let before = walk.progress().searched;
        let r = walk.step(&root, BootId(7), 1, &mut bufs, &mut hits);
        calls += 1;
        assert!(walk.progress().searched - before <= 1);
        if r == StepResult::Done {
            break;
        }
        assert!(calls < pids, "More after every pid was walked");
    }
    // One call per listed pid.
    assert_eq!(calls, pids);
    assert_eq!(walk.progress().searched, 2);
    let mut pipe_pids: Vec<u32> = hits.iter().map(|h| h.pid).collect();
    pipe_pids.sort_unstable();
    assert_eq!(pipe_pids, [4201, 4214]);
    // A finished walk stays done.
    assert_eq!(
        walk.step(&root, BootId(7), 1, &mut bufs, &mut hits),
        StepResult::Done
    );
}

#[test]
fn handles_restricted_skipped() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let tmp = common::fixture_copy("process-edge", None);
    let fd_dir = tmp.path().join("proc/4214/fd");
    fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o000)).expect("chmod fd");
    let root = SysRoot::at(tmp.path());
    let (hits, walk) = walk_all(&root, "", true);
    fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o755)).expect("restore fd");

    let p = walk.progress();
    assert_eq!((p.searched, p.total, p.skipped), (1, 2, 1));
    // Nothing of the skipped pid, not even its cwd or exe.
    assert!(hits.iter().all(|h| h.pid == 4201), "{hits:?}");
}

#[test]
fn handles_symlinked_pid_dir_not_followed() {
    // Security regression: a pid entry that is a symlink is never followed.
    let tmp = common::fixture_copy("process-edge", None);
    let proc = tmp.path().join("proc");
    symlink(proc.join("4201"), proc.join("4300")).expect("link pid");
    let root = SysRoot::at(tmp.path());
    let (hits, walk) = walk_all(&root, "notes", true);
    assert!(hits.iter().all(|h| h.pid == 4201), "{hits:?}");
    let p = walk.progress();
    assert_eq!((p.searched, p.skipped), (2, 1));
}

#[test]
fn handles_missing_root_is_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = SysRoot::at(tmp.path());
    let (hits, walk) = walk_all(&root, "x", true);
    assert!(hits.is_empty());
    assert_eq!(walk.progress(), Default::default());
}

#[test]
fn pipe_peers_found() {
    let root = SysRoot::at(&fixture_dir("process-edge"));
    let inodes: FxHashSet<u64> = [99, 12_345].into_iter().collect();
    let mut bufs = ReadBufs::default();
    let mut out: PipePeers = FxHashMap::default();
    find_pipe_peers(&root, &inodes, 4201, &mut bufs, &mut out);
    assert_eq!(out.len(), 1);
    let peers: Vec<(u32, &str)> = out[&99].iter().map(|(p, c)| (*p, &**c)).collect();
    assert_eq!(peers, [(4214, "cat")]);

    // From the other end, 4201 is the peer.
    find_pipe_peers(&root, &inodes, 4214, &mut bufs, &mut out);
    let peers: Vec<(u32, &str)> = out[&99].iter().map(|(p, c)| (*p, &**c)).collect();
    assert_eq!(peers, [(4201, "Web Content")]);

    // No inodes: `out` is cleared and stays empty.
    find_pipe_peers(&root, &FxHashSet::default(), 4214, &mut bufs, &mut out);
    assert!(out.is_empty());
}

// ---- E05-032: strings scanner ----

mod strings_scan {
    use tl_collect::proc::detail::parse_maps;
    use tl_collect::proc::strings::{
        CHUNK, Encoding, MAX_TEXT_BYTES, MemSource, ProcMem, ScanEnd, ScanOpts, StringHitInfo,
        StringScanner, VecMem, select_regions,
    };
    use tl_collect::{CollectError, SysRoot};

    /// Runs `scanner` to its end. Every step reads at most one chunk, so the step count is
    /// bounded by the region bytes.
    fn run(mut scanner: StringScanner, mem: &mut dyn MemSource) -> (ScanEnd, Vec<StringHitInfo>) {
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        for _ in 0..10_000 {
            if let Some(end) = scanner.step(mem, &mut buf, &mut out) {
                assert!(buf.len() <= CHUNK);
                return (end, out);
            }
        }
        panic!("scan did not end");
    }

    fn scan(opts: ScanOpts, base: u64, bytes: Vec<u8>) -> (ScanEnd, Vec<StringHitInfo>) {
        let end = base + bytes.len() as u64;
        run(
            StringScanner::new(opts, vec![(base, end)]),
            &mut VecMem(base, bytes),
        )
    }

    #[test]
    fn strings_region_selection() {
        let maps = b"00400000-00401000 rw-p 00000000 00:00 0 [heap]\n\
7f0000000000-7f0000001000 rw-p 00000000 00:00 0\n\
7f0000100000-7f0000200000 r-xp 00000000 103:02 1234 /usr/lib/libc.so.6\n\
7ffd00000000-7ffd00002000 r--p 00000000 00:00 0 [vvar]\n\
7f0000300000-7f0000400000 rw-s 00000000 00:05 99 /dev/dri/card1\n";
        let mut regions = Vec::new();
        parse_maps(maps, &mut regions).expect("parse maps");
        let mut out = Vec::new();
        select_regions(&regions, false, &mut out);
        assert_eq!(
            out,
            [(0x40_0000, 0x40_1000), (0x7f00_0000_0000, 0x7f00_0000_1000)]
        );
        select_regions(&regions, true, &mut out);
        assert_eq!(
            out,
            [
                (0x40_0000, 0x40_1000),
                (0x7f00_0000_0000, 0x7f00_0000_1000),
                (0x7f00_0010_0000, 0x7f00_0020_0000),
            ]
        );
    }

    #[test]
    fn strings_ascii_hit() {
        let (end, hits) = scan(
            ScanOpts::default(),
            0x1000,
            b"\0\0hello world\0ab\0".to_vec(),
        );
        assert_eq!(end, ScanEnd::Finished);
        assert_eq!(
            hits,
            [StringHitInfo {
                address: 0x1002,
                encoding: Encoding::Ascii,
                length: 11,
                value: "hello world".into(),
            }]
        );
    }

    #[test]
    fn strings_utf16le_hit() {
        let mut bytes = vec![0u8; 4];
        bytes.extend("Wine path".encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend([0, 0]);
        let (end, hits) = scan(ScanOpts::default(), 0x2000, bytes);
        assert_eq!(end, ScanEnd::Finished);
        assert_eq!(
            hits,
            [StringHitInfo {
                address: 0x2004,
                encoding: Encoding::Utf16Le,
                length: 9,
                value: "Wine path".into(),
            }]
        );
    }

    #[test]
    fn strings_utf8_hit() {
        let (end, hits) = scan(ScanOpts::default(), 0x3000, "grüße\0".as_bytes().to_vec());
        assert_eq!(end, ScanEnd::Finished);
        assert_eq!(
            hits,
            [StringHitInfo {
                address: 0x3000,
                encoding: Encoding::Utf8,
                length: 5,
                value: "grüße".into(),
            }]
        );
    }

    #[test]
    fn strings_chunk_boundary_once() {
        let base = 0x10_0000;
        let mut bytes = vec![0u8; 2 * CHUNK];
        bytes[CHUNK - 3..CHUNK + 3].copy_from_slice(b"abcdef");
        let (end, hits) = scan(ScanOpts::default(), base, bytes);
        assert_eq!(end, ScanEnd::Finished);
        assert_eq!(
            hits,
            [StringHitInfo {
                address: base + CHUNK as u64 - 3,
                encoding: Encoding::Ascii,
                length: 6,
                value: "abcdef".into(),
            }]
        );
    }

    #[test]
    fn strings_long_value_is_cut() {
        let mut bytes = vec![b'x'; 5000];
        bytes.push(0);
        let (_, hits) = scan(ScanOpts::default(), 0x1000, bytes);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].length, 5000);
        assert_eq!(hits[0].value.chars().count(), 4097);
        assert!(hits[0].value.ends_with('\u{2026}'));
    }

    #[test]
    fn strings_stop_limit() {
        let mut bytes = Vec::new();
        for i in 0..20 {
            bytes.extend(format!("str{i:02}\0").bytes());
        }
        let scanner = StringScanner::new(ScanOpts::default(), vec![(0x1000, 0x1000 + 120)])
            .with_limits(10, MAX_TEXT_BYTES);
        let (end, hits) = run(scanner, &mut VecMem(0x1000, bytes));
        assert_eq!(end, ScanEnd::Stopped);
        assert_eq!(hits.len(), 10);
    }

    struct Unreadable;

    impl MemSource for Unreadable {
        fn read(&mut self, _: u64, _: &mut [u8]) -> Result<usize, CollectError> {
            Err(CollectError::PermissionDenied)
        }
    }

    #[test]
    fn strings_unreadable_regions_fail() {
        let regions = vec![(0x1000, 0x2000), (0x10_0000, 0x10_0000 + 3 * CHUNK as u64)];
        let scanner = StringScanner::new(ScanOpts::default(), regions.clone());
        assert_eq!(run(scanner, &mut Unreadable), (ScanEnd::Failed, Vec::new()));

        // One readable region is enough for Finished; the hole is skipped.
        let scanner = StringScanner::new(ScanOpts::default(), regions);
        let (end, hits) = run(scanner, &mut VecMem(0x1000, b"\0kept string\0".to_vec()));
        assert_eq!(end, ScanEnd::Finished);
        assert_eq!(hits.len(), 1);
        assert_eq!(&*hits[0].value, "kept string");
    }

    static PROBE: [u8; 32] = *b"tasklens-strings-probe\0\0\0\0\0\0\0\0\0\0";

    #[test]
    fn strings_proc_mem_reads_self() {
        let probe = std::hint::black_box(&PROBE);
        let mut mem = ProcMem::open(&SysRoot::host(), super::own_key()).expect("open own mem");
        let mut buf = [0u8; 32];
        let n = mem
            .read(probe.as_ptr() as u64, &mut buf)
            .expect("read own mem");
        assert_eq!(n, 32);
        assert_eq!(&buf, probe);
    }
}

mod strings_random {
    use tl_collect::proc::strings::{Encoding, ScanEnd, ScanOpts, StringScanner, VecMem};

    /// 4 MiB of xorshift bytes with all encodings on. All encodings together must stay under
    /// 20,000 hits per MiB, so random data does not flood the scan.
    #[test]
    fn strings_random_bytes_under_cap() {
        const SIZE: usize = 4 * 1024 * 1024;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let bytes: Vec<u8> = (0..SIZE)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        let base = 0x10_0000;
        let mut scanner = StringScanner::new(ScanOpts::default(), vec![(base, base + SIZE as u64)]);
        let mut mem = VecMem(base, bytes);
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        let mut end = None;
        while end.is_none() {
            end = scanner.step(&mut mem, &mut buf, &mut out);
        }
        let count = |e| out.iter().filter(|h| h.encoding == e).count();
        eprintln!(
            "random 4 MiB: ascii {} utf8 {} utf16le {}",
            count(Encoding::Ascii),
            count(Encoding::Utf8),
            count(Encoding::Utf16Le)
        );
        assert_eq!(end, Some(ScanEnd::Finished));
        assert!(out.len() < 4 * 20_000, "{} hits", out.len());
    }
}

mod strings_utf16_alphabet {
    use tl_collect::proc::strings::{
        Encoding, ScanEnd, ScanOpts, StringHitInfo, StringScanner, VecMem,
    };

    fn utf16_hits(text: &str) -> Vec<StringHitInfo> {
        let mut bytes = vec![0u8; 2];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend([0, 0]);
        let base = 0x4000;
        let end = base + bytes.len() as u64;
        let mut scanner = StringScanner::new(ScanOpts::default(), vec![(base, end)]);
        let mut mem = VecMem(base, bytes);
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        assert_eq!(
            scanner.step(&mut mem, &mut buf, &mut out),
            Some(ScanEnd::Finished)
        );
        out.retain(|h| h.encoding == Encoding::Utf16Le);
        out
    }

    #[test]
    fn strings_utf16le_cyrillic_hit() {
        assert_eq!(
            utf16_hits("Привет"),
            [StringHitInfo {
                address: 0x4002,
                encoding: Encoding::Utf16Le,
                length: 6,
                value: "Привет".into(),
            }]
        );
    }

    #[test]
    fn strings_utf16le_spaces_and_digits_stay_in_run() {
        assert_eq!(
            utf16_hits("Привет мир 2024"),
            [StringHitInfo {
                address: 0x4002,
                encoding: Encoding::Utf16Le,
                length: 15,
                value: "Привет мир 2024".into(),
            }]
        );
        // An ASCII letter still ends the Cyrillic run.
        let hits = utf16_hits("Привет, Wine");
        let values: Vec<&str> = hits.iter().map(|h| &*h.value).collect();
        assert_eq!(values, ["Привет,", "Wine"]);
    }

    #[test]
    fn strings_utf16le_high_byte_change_splits_run() {
        // "Wine" has high byte 0 and "привет" has 0x04, so each part is its own run.
        let hits = utf16_hits("Wineпривет");
        let values: Vec<&str> = hits.iter().map(|h| &*h.value).collect();
        assert_eq!(values, ["Wine", "привет"]);
        assert_eq!(hits[1].address, 0x4002 + 8);
    }
}

// Environment, cwd and root readers (E05-029).

mod environ {
    use std::fs;

    use rustix::fd::AsFd;
    use rustix::fs::{Mode, OFlags};
    use tl_collect::SysRoot;
    use tl_collect::proc::detail::{
        ENVIRON_MAX, EnvInfo, EnvRead, LinkRead, read_cwd, read_environ, read_root,
    };

    use super::{FAKE_START_TICKS, rollup_key};
    use crate::common;

    fn read(root: &SysRoot) -> (EnvRead, EnvInfo) {
        let mut out = EnvInfo::default();
        let r = read_environ(
            root,
            rollup_key(4201, FAKE_START_TICKS),
            &mut Vec::new(),
            &mut out,
        );
        (r.expect("read environ"), out)
    }

    fn copy_with_environ(bytes: &[u8]) -> (tempfile::TempDir, SysRoot) {
        let tmp = common::fixture_copy("process-edge", None);
        fs::write(tmp.path().join("proc/4201/environ"), bytes).expect("write environ");
        let root = SysRoot::at(tmp.path());
        (tmp, root)
    }

    #[test]
    fn environ_vars_parsed() {
        let root = SysRoot::at(common::fixture_dir("process-edge"));
        let (r, out) = read(&root);
        assert_eq!(r, EnvRead::Ok);
        let vars: Vec<(&str, &str)> = out.vars.iter().map(|(n, v)| (&**n, &**v)).collect();
        assert_eq!(
            vars,
            [
                ("HOME", "/home/user1"),
                ("PATH", "/usr/bin"),
                ("API_KEY", "abc"),
                ("EMPTY", ""),
            ]
        );
        assert!(!out.truncated);
    }

    #[test]
    fn environ_truncated_at_1mib() {
        let mut bytes = Vec::new();
        let mut written = Vec::new();
        let mut i = 0;
        while bytes.len() < 2 * ENVIRON_MAX {
            let entry = format!("VAR_{i:06}={}", "x".repeat(40));
            bytes.extend_from_slice(entry.as_bytes());
            bytes.push(0);
            written.push(entry);
            i += 1;
        }
        let (_tmp, root) = copy_with_environ(&bytes);
        let (r, out) = read(&root);
        assert_eq!(r, EnvRead::Ok);
        assert!(out.truncated);
        assert!(!out.vars.is_empty() && out.vars.len() < written.len());
        for (n, (name, value)) in out.vars.iter().enumerate() {
            assert_eq!(format!("{name}={value}"), written[n]);
        }
    }

    #[test]
    fn environ_empty_is_unavailable() {
        let (_tmp, root) = copy_with_environ(b"");
        let (r, out) = read(&root);
        assert_eq!(r, EnvRead::Empty);
        assert!(out.vars.is_empty());
    }

    #[test]
    fn environ_values_sanitized() {
        let (_tmp, root) = copy_with_environ("NAME=a\u{202e}b\0".as_bytes());
        let (r, out) = read(&root);
        assert_eq!(r, EnvRead::Ok);
        assert_eq!(&*out.vars[0].1, "ab");
        assert!(!out.vars[0].1.contains('\u{202e}'));
    }

    /// Security regression: the environment never reaches logs through `Debug`.
    #[test]
    fn environ_debug_redacted() {
        let info = EnvInfo {
            vars: vec![("SECRET".into(), "hunter2".into())],
            truncated: false,
        };
        let text = format!("{info:?}");
        assert!(!text.contains("SECRET"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn cwd_and_root_links() {
        let dir = rustix::fs::open(
            common::fixture_dir("process-edge").join("proc/4201"),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open 4201");
        let mut buf = Vec::new();
        assert_eq!(
            read_cwd(dir.as_fd(), &mut buf),
            LinkRead::Path("/home/user1".into())
        );
        assert_eq!(read_root(dir.as_fd(), &mut buf), LinkRead::Path("/".into()));
    }
}
