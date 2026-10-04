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
