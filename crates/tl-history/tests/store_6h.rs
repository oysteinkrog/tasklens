//! Six hours of synthetic history through `Recorder` into a `Store` (ADR-012 verification,
//! U-11, U-22).
//!
//! The generator is deterministic (xorshift, seed 42) and uses synthetic timestamps, so no real
//! time passes. 2,000 processes, 200 of them changing each second, one replaced every 30 s, and
//! 120 system series at 1 Hz. The store is written once per test binary into
//! `CARGO_TARGET_TMPDIR/store_6h` and every test reopens it.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tl_history::segment::{FOOTER_TRAILER_LEN, SegmentReader};
use tl_history::{
    HEADER_LEN, HistProcess, MAX_POINTS, MEM_THRESHOLD_KIB, Metric, Recorder, Store, StoreConfig,
    boot_dir_name, state_at, system_series, top_at,
};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::history::RecordSink;
use tl_model::ids::{DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::series::Sample;
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::{FieldValidity, Validity};

const SEC: u64 = 1_000_000_000;
const MIN: u64 = 60 * SEC;
const SECS: u64 = 6 * 3600;
const PROCS: usize = 2000;
const CHANGING: usize = 200;
const REPLACE_EVERY_SECS: u64 = 30;
const SERIES: usize = 120;
const SEED: u64 = 42;
const CHECKS: usize = 100;
const TOP_CHECKS: usize = 50;
const TOP_N: usize = 10;
const BOOT: BootId = BootId(0x6b00_7000_0000_0042);
const REALTIME_OFFSET_NS: i64 = 1_700_000_000_000_000_000;
const MEM_TOTAL_KIB: u64 = 64 << 20;
/// ADR-012 verification: 6 h of this load fits in 64 MB.
const DISK_LIMIT: u64 = 64 << 20;
/// PROC-090 budget for one hover answer.
const TOP_AT_BUDGET: Duration = Duration::from_millis(50);

/// xorshift64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    fn chance(&mut self, one_in: u64) -> bool {
        self.next() % one_in == 0
    }

    /// `v` moved by `lo..=hi` up or down, kept in `0..=max`.
    fn walk(&mut self, v: u64, lo: u64, hi: u64, max: u64) -> u64 {
        let step = self.range(lo, hi);
        if self.chance(2) {
            v.saturating_add(step).min(max)
        } else {
            v.saturating_sub(step)
        }
    }
}

/// Distinct seconds in `0..SECS`, in draw order.
fn pick_seconds(n: usize) -> Vec<u64> {
    let mut rng = Rng(SEED);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let s = rng.next() % SECS;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// One process in stored units.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Vals {
    key: ProcessKey,
    ppid: u32,
    cpu_permille: u16,
    mem_anon_kib: u32,
    io_r_kibps: u32,
    io_w_kibps: u32,
    net_kibps: u32,
    gpu_pct: u8,
}

impl Vals {
    fn of(p: &HistProcess) -> Vals {
        Vals {
            key: p.key,
            ppid: p.ppid,
            cpu_permille: p.cpu_permille,
            mem_anon_kib: p.mem_anon_kib,
            io_r_kibps: p.io_r_kibps,
            io_w_kibps: p.io_w_kibps,
            net_kibps: p.net_kibps,
            gpu_pct: p.gpu_pct,
        }
    }

    /// The `FieldPatch` rule (R-169 OI-3), written out here so the test does not reuse the
    /// crate's code: a field is stored when it moved past its threshold against the last stored
    /// value. CPU (1 permille), rates (1 KiB/s or to or from zero), ppid and GPU store every
    /// change of their integer value; memory needs 256 KiB.
    fn store_from(&mut self, cur: &Vals) {
        self.ppid = cur.ppid;
        self.cpu_permille = cur.cpu_permille;
        if self.mem_anon_kib.abs_diff(cur.mem_anon_kib) >= MEM_THRESHOLD_KIB {
            self.mem_anon_kib = cur.mem_anon_kib;
        }
        self.io_r_kibps = cur.io_r_kibps;
        self.io_w_kibps = cur.io_w_kibps;
        self.net_kibps = cur.net_kibps;
        self.gpu_pct = cur.gpu_pct;
    }
}

fn key(id: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 10_000 + u64::from(id),
        boot_id: BOOT,
        pid: 1_000 + id,
    }
}

fn name(id: u32) -> String {
    format!("proc{id}")
}

fn id_of(k: &ProcessKey) -> u32 {
    k.pid - 1_000
}

fn new_record(id: u32, rng: &mut Rng) -> ProcessRecord {
    let text: Arc<str> = Arc::from(name(id));
    let mut r = ProcessRecord {
        key: key(id),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: text.clone(),
            name: text.clone(),
            exe: Some(Arc::from(format!("/usr/bin/proc{id}"))),
            cmdline: None,
            user: Arc::from("alice"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            search_lower: text,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::default(),
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
        validity: FieldValidity::all(Validity::Valid),
    };
    let v = Vals {
        key: r.key,
        ppid: 1,
        cpu_permille: rng.range(0, 200) as u16,
        mem_anon_kib: rng.range(1024, 2_000_000) as u32,
        io_r_kibps: 0,
        io_w_kibps: 0,
        net_kibps: 0,
        gpu_pct: 0,
    };
    set_vals(&mut r, &v);
    r
}

/// Writes stored units into the record so that `hist_process` gives them back exactly.
fn set_vals(r: &mut ProcessRecord, v: &Vals) {
    r.ppid = v.ppid;
    r.cpu_pct = f32::from(v.cpu_permille) / 10.0;
    r.rss_anon_bytes = u64::from(v.mem_anon_kib) * 1024;
    r.io_read_bps = v.io_r_kibps as f32 * 1024.0;
    r.io_write_bps = v.io_w_kibps as f32 * 1024.0;
    r.net_rx_bps = v.net_kibps as f32 * 1024.0;
    r.net_tx_bps = 0.0;
    r.gpu_util_pct = f32::from(v.gpu_pct);
}

/// One random-walk step of a changing process. Every step is past the field's threshold.
fn step(v: &mut Vals, rng: &mut Rng) {
    v.cpu_permille = rng.walk(u64::from(v.cpu_permille), 1, 20, 1000) as u16;
    let mem_step = u64::from(MEM_THRESHOLD_KIB) * 2;
    v.mem_anon_kib = rng.walk(u64::from(v.mem_anon_kib), mem_step, 4096, 16 << 20) as u32;
    if rng.chance(4) {
        v.io_r_kibps = rng.walk(u64::from(v.io_r_kibps), 1, 64, 1 << 20) as u32;
    }
    if rng.chance(4) {
        v.io_w_kibps = rng.walk(u64::from(v.io_w_kibps), 1, 64, 1 << 20) as u32;
    }
    if rng.chance(4) {
        v.net_kibps = rng.walk(u64::from(v.net_kibps), 1, 64, 1 << 20) as u32;
    }
    if rng.chance(8) {
        v.gpu_pct = rng.range(0, 100) as u8;
    }
}

/// One system series: a random walk inside `0..=max`.
struct Series {
    id: SeriesId,
    value: f32,
    max: f32,
}

fn entry(idx: u16, identity: DeviceIdentity, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(format!("synthetic:{idx}")),
        net_kind: None,
    }
}

/// 40 devices with 120 series: the CPU package (4), 32 logical CPUs (3 each), memory (4), one
/// disk (4), one network interface (2), one GPU (7), a fan, a sensor and a battery (1 each).
fn devices() -> (DeviceCatalog, Vec<Series>) {
    let mut entries = Vec::new();
    let mut series = Vec::new();
    let mut add = |idx: u16, identity, name: &str, metrics: &[(MetricId, f32)]| {
        entries.push(entry(idx, identity, name));
        for &(metric, max) in metrics {
            series.push(Series {
                id: SeriesId {
                    device: DeviceIdx(idx),
                    metric,
                },
                value: max / 2.0,
                max,
            });
        }
    };
    let pct = 100.0;
    add(
        0,
        DeviceIdentity::CpuPackage,
        "CPU",
        &[
            (MetricId::CPU_TOTAL, pct),
            (MetricId::CPU_KERNEL, pct),
            (MetricId::CPU_FREQ, 5_000.0),
            (MetricId::CPU_TEMP, 100.0),
        ],
    );
    for i in 0..32u16 {
        add(
            1 + i,
            DeviceIdentity::CpuLogical { id: i },
            &format!("CPU {i}"),
            &[
                (MetricId::CPU_TOTAL, pct),
                (MetricId::CPU_KERNEL, pct),
                (MetricId::CPU_FREQ, 5_000.0),
            ],
        );
    }
    let mem = MEM_TOTAL_KIB as f32 * 1024.0;
    add(
        33,
        DeviceIdentity::Memory,
        "Memory",
        &[
            (MetricId::MEM_USED, mem),
            (MetricId::MEM_COMMIT, mem),
            (MetricId::SWAP_USED, mem / 4.0),
            (MetricId::MEM_CACHED, mem),
        ],
    );
    add(
        34,
        DeviceIdentity::Disk {
            major: 259,
            minor: 0,
            name: Arc::from("nvme0n1"),
            wwn_or_serial: None,
        },
        "nvme0n1",
        &[
            (MetricId::DISK_ACTIVE, pct),
            (MetricId::DISK_READ, 2e9),
            (MetricId::DISK_WRITE, 2e9),
            (MetricId::DISK_LATENCY, 0.01),
        ],
    );
    add(
        35,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: 2,
            name: Arc::from("eth0"),
        },
        "eth0",
        &[(MetricId::NET_RX, 1.25e8), (MetricId::NET_TX, 1.25e8)],
    );
    add(
        36,
        DeviceIdentity::Gpu {
            uuid: [7; 16],
            pci_bus_id: Arc::from("0000:01:00.0"),
        },
        "GPU",
        &[
            (MetricId::GPU_UTIL, pct),
            (MetricId::GPU_MEM, 2.4e10),
            (MetricId::GPU_ENC, pct),
            (MetricId::GPU_DEC, pct),
            (MetricId::GPU_TEMP, 100.0),
            (MetricId::GPU_POWER, 450.0),
            (MetricId::GPU_CLOCK, 3_000.0),
        ],
    );
    add(
        37,
        DeviceIdentity::Fan {
            chip: Arc::from("nct6798"),
            index: 1,
        },
        "Fan 1",
        &[(MetricId::FAN_RPM, 3_000.0)],
    );
    add(
        38,
        DeviceIdentity::Sensor {
            chip: Arc::from("nct6798"),
            label: Arc::from("SYSTIN"),
        },
        "SYSTIN",
        &[(MetricId::SENSOR_TEMP, 100.0)],
    );
    add(
        39,
        DeviceIdentity::Battery {
            native_path: Arc::from("BAT0"),
        },
        "Battery",
        &[(MetricId::BATTERY_PCT, pct)],
    );
    let catalog = DeviceCatalog {
        version: 1,
        entries: entries.into(),
    };
    (catalog, series)
}

/// What the generator wrote, for the tests to check against.
struct Run {
    root: PathBuf,
    /// Stored state after the sweep at each checked second, sorted by key.
    expected: HashMap<u64, Vec<Vals>>,
    span: (TimeNs, TimeNs),
    blocks: usize,
    elapsed: Duration,
}

impl Run {
    fn cfg(&self) -> StoreConfig {
        config(&self.root)
    }

    fn boot_dir(&self) -> PathBuf {
        self.root.join(boot_dir_name(BOOT))
    }

    fn open(&self) -> Store {
        open_store(self.cfg(), self.span.1)
    }
}

fn config(root: &Path) -> StoreConfig {
    StoreConfig {
        root: root.to_path_buf(),
        boot: BOOT,
        retention_ns: 24 * 3600 * SEC,
        max_disk_bytes: 1 << 30,
        load_existing: true,
    }
}

fn open_store(cfg: StoreConfig, now: TimeNs) -> Store {
    let (store, report) = Store::open(cfg, now, Instant::now() + Duration::from_secs(60));
    assert_eq!(report.problem, None);
    store
}

static RUN: OnceLock<Run> = OnceLock::new();

fn run() -> &'static Run {
    RUN.get_or_init(generate)
}

/// Writes the 6 h store. The directory is reused across runs, so a second test binary running
/// this file at the same time would clash.
fn generate() -> Run {
    let started = Instant::now();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("store_6h");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();

    let checks = pick_seconds(CHECKS);
    let mut rng = Rng(SEED);
    let (catalog, mut series) = devices();
    assert_eq!(series.len(), SERIES);

    let mut next_id = 0u32;
    let mut records: Vec<ProcessRecord> = Vec::with_capacity(PROCS);
    let mut actual: Vec<Vals> = Vec::with_capacity(PROCS);
    for _ in 0..PROCS {
        let r = new_record(next_id, &mut rng);
        next_id += 1;
        actual.push(Vals::of(&tl_history::hist_process(&r, 0)));
        records.push(r);
    }
    // The generator's model of the last stored value per process.
    let mut stored: HashMap<ProcessKey, Vals> = HashMap::with_capacity(PROCS);
    let mut expected = HashMap::new();

    let mut store = open_store(config(&root), 0);
    let mut rec = Recorder::new(BOOT);
    rec.set_mem_total_kib(MEM_TOTAL_KIB);
    rec.set_realtime_offset_ns(REALTIME_OFFSET_NS);
    rec.on_device_catalog(&catalog);
    let mut blocks = 0usize;
    let mut samples = Vec::with_capacity(SERIES);

    for s in 0..SECS {
        let t = s * SEC;
        let block_start = t % MIN == 0;
        // A block that the store will put into a new segment starts a new meta scope. Before the
        // first append the store has no segment, and the first block opens a fresh scope anyway.
        if block_start && blocks > 0 && store.will_rotate(t) {
            rec.reset_meta_scope();
        }

        let mut exited = None;
        if s > 0 {
            for _ in 0..CHANGING {
                let i = rng.next() as usize % PROCS;
                step(&mut actual[i], &mut rng);
                set_vals(&mut records[i], &actual[i]);
            }
            if s % REPLACE_EVERY_SECS == 0 {
                let i = rng.next() as usize % PROCS;
                records.remove(i);
                exited = Some(actual.remove(i).key);
                let r = new_record(next_id, &mut rng);
                next_id += 1;
                actual.push(Vals::of(&tl_history::hist_process(&r, 0)));
                records.push(r);
            }
        }
        samples.clear();
        for x in &mut series {
            let step = x.max / 50.0;
            let r = (rng.next() % 10_001) as f32 / 10_000.0;
            x.value = (x.value + (r - 0.5) * step).clamp(0.0, x.max);
            samples.push((x.id, Sample::valid(t, x.value)));
        }

        rec.on_system_sample(t, &samples);
        let sweep: Arc<[ProcessRecord]> = records.clone().into();
        rec.on_process_sweep(t, &sweep);
        while let Some(c) = rec.take_closed() {
            store.append(c.block, c.realtime_offset_ns).unwrap();
            blocks += 1;
        }

        if block_start {
            stored.clear();
            stored.extend(actual.iter().map(|v| (v.key, v.clone())));
        } else {
            if let Some(k) = exited {
                stored.remove(&k);
            }
            for v in &actual {
                stored
                    .entry(v.key)
                    .and_modify(|s| s.store_from(v))
                    .or_insert_with(|| v.clone());
            }
        }
        if checks.contains(&s) {
            let mut state: Vec<Vals> = stored.values().cloned().collect();
            state.sort();
            expected.insert(s, state);
        }
    }
    if let Some(c) = rec.flush() {
        store.append(c.block, c.realtime_offset_ns).unwrap();
        blocks += 1;
    }
    store.close().unwrap();
    assert_eq!(rec.dropped_blocks(), 0);
    let span = store.span().unwrap();
    Run {
        root,
        expected,
        span,
        blocks,
        elapsed: started.elapsed(),
    }
}

fn segment_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "tlh"))
        .collect();
    paths.sort();
    paths
}

/// Frame_Content_Size of a zstd frame header (RFC 8878 section 3.1.1.1), or `None` when the
/// frame does not carry it.
fn zstd_content_size(frame: &[u8]) -> Option<u64> {
    if frame.get(0..4)? != [0x28, 0xb5, 0x2f, 0xfd] {
        return None;
    }
    let descriptor = *frame.get(4)?;
    let single_segment = descriptor & 0x20 != 0;
    let dict_len = [0, 1, 2, 4][usize::from(descriptor & 0x03)];
    let fcs_len = match (descriptor >> 6, single_segment) {
        (0, true) => 1,
        (0, false) => return None,
        (1, _) => 2,
        (2, _) => 4,
        _ => 8,
    };
    let at = 5 + usize::from(!single_segment) + dict_len;
    let bytes = frame.get(at..at + fcs_len)?;
    let v = bytes
        .iter()
        .enumerate()
        .fold(0u64, |v, (i, b)| v | u64::from(*b) << (8 * i));
    Some(if fcs_len == 2 { v + 256 } else { v })
}

/// Postcard bytes before compression and the block frames on disk, over every segment.
fn raw_and_compressed(dir: &Path) -> (u64, u64) {
    let (mut raw, mut compressed) = (0u64, 0u64);
    for path in segment_paths(dir) {
        let reader = SegmentReader::open(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        for e in &reader.index {
            let at = e.offset as usize;
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let frame = &bytes[at + 8..at + 8 + len];
            raw += zstd_content_size(frame).unwrap();
            compressed += len as u64;
        }
    }
    (raw, compressed)
}

#[test]
fn six_hours_under_64_mb() {
    let run = run();
    let dir = run.boot_dir();
    let files = segment_paths(&dir);
    let disk: u64 = files.iter().map(|p| fs::metadata(p).unwrap().len()).sum();
    let (raw, compressed) = raw_and_compressed(&dir);
    let store = run.open();
    assert_eq!(store.disk_bytes(), disk);
    eprintln!(
        "store_6h: {} blocks in {} segments, generated in {:.1} s",
        run.blocks,
        files.len(),
        run.elapsed.as_secs_f64()
    );
    eprintln!(
        "store_6h: disk {disk} bytes ({:.2} MiB), block frames {compressed} bytes, \
         postcard {raw} bytes, compression ratio {:.2}:1",
        disk as f64 / f64::from(1 << 20),
        raw as f64 / compressed as f64
    );
    assert_eq!(run.blocks as u64, SECS / 60);
    assert_eq!(files.len() as u64, SECS / 600);
    assert!(disk < DISK_LIMIT, "6 h store takes {disk} bytes");
}

#[test]
fn random_time_reconstruction_matches() {
    let run = run();
    let store = run.open();
    assert_eq!(run.span, (0, (SECS - 1) * SEC));
    assert_eq!(store.span(), Some(run.span));
    let seconds = pick_seconds(CHECKS);
    for s in seconds {
        let t = s * SEC;
        let (blocks, metas) = store.blocks(t, t).unwrap();
        assert_eq!(blocks.len(), 1, "second {s}");
        let frame = state_at(&blocks[0], t).unwrap();
        for p in &frame.procs {
            let meta = metas.get(p.meta).unwrap();
            assert_eq!(&*meta.name, name(id_of(&p.key)), "second {s}");
        }
        let mut got: Vec<Vals> = frame.procs.iter().map(Vals::of).collect();
        got.sort();
        let want = &run.expected[&s];
        assert_eq!(got.len(), PROCS, "second {s}");
        assert!(got == *want, "state differs at second {s}");
    }
}

#[test]
fn truncated_last_block_recovered() {
    let run = run();
    let damaged = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let dir = damaged.path().join(boot_dir_name(BOOT));
    fs::create_dir(&dir).unwrap();
    let originals = segment_paths(&run.boot_dir());
    for p in &originals {
        fs::copy(p, dir.join(p.file_name().unwrap())).unwrap();
    }
    let last = dir.join(originals.last().unwrap().file_name().unwrap());
    let original = SegmentReader::open(originals.last().unwrap()).unwrap();
    assert!(!original.recovered);
    let n = original.index.len();
    assert_eq!(n, 10);

    // Drop the footer, then the last 3 bytes of the last block.
    let bytes = fs::read(&last).unwrap();
    let t = bytes.len() - FOOTER_TRAILER_LEN;
    let footer_len = u32::from_le_bytes(bytes[t..t + 4].try_into().unwrap()) as usize;
    let blocks_end = (t - footer_len) as u64;
    assert!(blocks_end > HEADER_LEN as u64);
    OpenOptions::new()
        .write(true)
        .open(&last)
        .unwrap()
        .set_len(blocks_end - 3)
        .unwrap();

    let r = SegmentReader::open(&last).unwrap();
    assert!(r.recovered);
    assert_eq!(r.index, original.index[..n - 1]);

    let good = run.open();
    let bad = open_store(config(damaged.path()), run.span.1);
    let seg_start = original.header.start_t_ns;
    let (want, _) = good.blocks(seg_start, u64::MAX).unwrap();
    let (got, _) = bad.blocks(seg_start, u64::MAX).unwrap();
    assert_eq!(want.len(), n);
    assert_eq!(got.len(), n - 1);
    let same = got.iter().zip(&want).all(|(a, b)| **a == **b);
    assert!(same, "a recovered block differs from the written one");
    let last_start = original.index[n - 2].start_t_ns;
    assert_eq!(bad.span(), Some((0, last_start + MIN - SEC)));
    // Every earlier segment is untouched.
    let (early, _) = bad.blocks(0, seg_start - 1).unwrap();
    assert_eq!(early.len(), run.blocks - n);
}

#[test]
fn top_at_within_50ms_release() {
    let run = run();
    let store = run.open();
    let mut slowest = Duration::ZERO;
    for s in pick_seconds(TOP_CHECKS) {
        let t = s * SEC;
        let started = Instant::now();
        let (blocks, _) = store.blocks(t, t).unwrap();
        let frame = state_at(&blocks[0], t).unwrap();
        let top = top_at(&frame, Metric::Cpu, TOP_N, store.deleted());
        let took = started.elapsed();
        slowest = slowest.max(took);

        let mut want = run.expected[&s].clone();
        want.sort_by(|a, b| b.cpu_permille.cmp(&a.cpu_permille).then(a.key.cmp(&b.key)));
        let want: Vec<ProcessKey> = want.iter().take(TOP_N).map(|v| v.key).collect();
        let got: Vec<ProcessKey> = top.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(got, want, "second {s}");

        #[cfg(not(debug_assertions))]
        assert!(took <= TOP_AT_BUDGET, "top_at at second {s} took {took:?}");
    }
    eprintln!(
        "store_6h: slowest top_at {slowest:?} over {TOP_CHECKS} cold seconds (budget \
         {TOP_AT_BUDGET:?})"
    );
}

#[test]
fn whole_span_decimation_caps() {
    let run = run();
    let store = run.open();
    let (from, to) = run.span;
    let (blocks, _) = store.blocks(from, to).unwrap();
    assert_eq!(blocks.len(), run.blocks);
    let points = system_series(blocks.iter().map(|b| &**b), Metric::Cpu, from, to);
    assert!(points.len() <= MAX_POINTS, "{} points", points.len());
    // 21,600 s in buckets of 22 s.
    assert!(points.len() > MAX_POINTS - 50, "{} points", points.len());
    assert!(points.iter().all(|p| p.min <= p.max && p.max <= 100.0));
}
