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
    let key = rollup_key(4201, 351_220);
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    assert_eq!(
        cache
            .get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs)
            .1,
        MetaChange::New
    );
    let reads = meta_file_reads();
    for _ in 1..META_REFRESH_SWEEPS {
        let (_, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
        assert_eq!(change, MetaChange::Unchanged);
    }
    assert_eq!(meta_file_reads(), reads, "no file re-read");
    let (meta, change) = cache.get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs);
    assert_eq!(change, MetaChange::Refreshed);
    assert_eq!(meta.cmdline.as_deref(), Some("firefox -P default"));
    assert!(meta_file_reads() > reads);
    assert_eq!(
        cache
            .get_or_refresh(key, dir.as_fd(), b"Web Content", &mut bufs)
            .1,
        MetaChange::Unchanged,
        "the counter restarts"
    );

    cache.retain_live(&Default::default());
    assert!(cache.is_empty());
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
