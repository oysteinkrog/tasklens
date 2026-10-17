//! LabRuntime tests for the `ProcessDetail` worker through `engine_root` on fixture roots
//! (ADR-013, ADR-008). Each test copies `owner-desktop` into the target tmpdir so it can add
//! `smaps_rollup` files.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::fs;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use asupersync::Cx;
use fake_sources::FakeLab;
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::session::engine_root;
use tl_engine::update::EngineMsg;
use tl_engine::workers::{process_detail, processes};
use tl_model::command::Command;
use tl_model::demand::{ColumnSet, Demand, DetailDemand};
use tl_model::details::{
    DetailTab, DetailTarget, ScanId, ScanPhase, StringEncoding, StringScanOpts, StringScanState,
    TabPayload, TabSlot,
};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::time::BootId;
use tl_model::validity::{ProcessField, Validity};

fn fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tl-collect/tests/fixtures/"
    ))
}

/// Copies every entry of `src` over `dst`, replacing files and links that exist in both.
fn overlay(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create fixture dir");
    for entry in fs::read_dir(src).expect("list fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path()).expect("read fixture symlink");
            let _ = fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
        } else if file_type.is_dir() {
            overlay(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}

/// A writable copy of one or more tl-collect fixtures under the target tmpdir. Later names win
/// for files in both. The directory name holds the process id and a counter, so parallel test
/// runs never share a copy. Dropping the root removes the copy.
struct FixtureRoot {
    dir: PathBuf,
}

impl FixtureRoot {
    fn new(names: &[&str]) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("lab-details-{}-{n}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove old fixture copy");
        }
        for name in names {
            overlay(&fixtures().join(name), &dir.join("root"));
        }
        FixtureRoot { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    fn pid_dir(&self, pid: u32) -> PathBuf {
        self.root().join(format!("proc/{pid}"))
    }

    fn write(&self, pid: u32, file: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.pid_dir(pid).join(file), bytes).expect("write fixture file");
    }

    /// The fixture root, with app lookups on the tl-collect app fixtures and the icon cache in
    /// this copy, so no test writes the real `~/.cache`.
    fn config(&self) -> Arc<EngineConfig> {
        let share = |name: &str| fixtures().join(name).join("usr/share");
        let dirs = [share("apps-desktop"), share("apps-icons")];
        let paths = AppPaths {
            home: fixtures().join("apps-icons/home"),
            xdg_data_home: None,
            xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
            xdg_config_home: None,
            icon_cache: self.dir.join("cache/icons.toml"),
        };
        Arc::new(
            EngineConfig::new(EngineMode::Full, self.dir.join("state"))
                .with_sys_root(SysRoot::at(&self.root()))
                .with_app_paths(paths),
        )
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        // Best effort: a failed cleanup must not hide the test result.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const ROLLUP: &[u8] =
    include_bytes!("../../tl-collect/tests/fixtures/process-edge/proc/4201/smaps_rollup");

fn demand(columns: &[ColumnId], keys: &[ProcessKey]) -> Demand {
    let mut set = ColumnSet::default();
    for &c in columns {
        set.insert(c);
    }
    Demand {
        any_surface_visible: true,
        process_table: true,
        process_columns: set,
        detail_keys: keys.into(),
        ..Demand::default()
    }
}

/// E10-A2: with `pss` shown and 3 `detail_keys`, `coverage.pss_uss` lists exactly those 3 keys
/// and their records have `Valid` PSS. Every other record keeps `WarmingUp`.
#[test]
fn engine_publishes_pss_coverage() {
    let root = FixtureRoot::new(&["owner-desktop"]);
    fs::create_dir_all(root.root().join("sys/fs/cgroup")).expect("create cgroup dir");
    // owner-desktop has no smaps_rollup; give 3 processes the one from process-edge 4201.
    let pids: Vec<u32> = {
        let mut all: Vec<u32> = fs::read_dir(root.root().join("proc"))
            .expect("list fixture proc")
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok()))
            .collect();
        all.sort_unstable();
        all.into_iter().take(3).collect()
    };
    assert_eq!(pids.len(), 3);
    for &pid in &pids {
        root.write(pid, "smaps_rollup", ROLLUP);
    }

    let mut fl = FakeLab::new();
    let (ep, ui) = fake_sources::root_endpoints(
        root.config(),
        vec![processes::spec(), process_detail::spec()],
    );
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });

    // The first run only learns the keys of the 3 processes.
    ui.demand_tx
        .send(demand(&[], &[]))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));
    let snap = ui.latest.load_full();
    let mut keys: Vec<ProcessKey> = snap
        .processes
        .records
        .iter()
        .filter(|r| pids.contains(&r.key.pid))
        .map(|r| r.key)
        .collect();
    assert_eq!(keys.len(), 3);
    keys.sort();

    ui.demand_tx
        .send(demand(&[ColumnId::Pss], &keys))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(4), Duration::from_millis(10));

    let snap = ui.latest.load_full();
    assert_eq!(&*snap.processes.coverage.pss_uss, keys.as_slice());
    for r in snap.processes.records.iter() {
        let expect = if keys.contains(&r.key) {
            assert_eq!(r.pss_bytes, 2_097_152);
            Validity::Valid
        } else {
            Validity::WarmingUp
        };
        assert_eq!(
            r.validity.get(ProcessField::Pss),
            expect,
            "pid {}",
            r.key.pid
        );
    }

    handle.abort();
    fl.advance(Duration::from_millis(10));
    assert!(handle.is_finished(), "engine_root returned after cancel");
    drop(ui);
    fl.finish();
}

/// process-edge 4201. The worker checks only the start ticks.
fn edge_key() -> ProcessKey {
    ProcessKey {
        start_ticks: 351_220,
        boot_id: BootId(1),
        pid: 4201,
    }
}

fn tab_demand(tab: DetailTab) -> Demand {
    Demand {
        any_surface_visible: true,
        details: Arc::from([DetailDemand {
            target: DetailTarget::Process(edge_key()),
            tab,
        }]),
        ..Demand::default()
    }
}

/// The slot of `tab` of process-edge 4201 in the latest snapshot.
fn slot(ui: &fake_sources::UiSide, tab: DetailTab) -> TabSlot {
    let snap = ui.latest.load_full();
    snap.details
        .targets
        .get(&DetailTarget::Process(edge_key()))
        .map(|t| t.tab(tab).clone())
        .unwrap_or_default()
}

fn env_vars(slot: &TabSlot) -> Vec<(String, String)> {
    match slot.payload.as_deref() {
        Some(TabPayload::Environ(e)) => e
            .vars
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
        other => panic!("expected an Environ payload, got {other:?}"),
    }
}

fn start_detail_engine(
    root: &FixtureRoot,
    interval_ms: u32,
) -> (
    FakeLab,
    fake_sources::UiSide,
    asupersync::runtime::TaskHandle<()>,
) {
    let mut fl = FakeLab::new();
    let (ep, ui) = fake_sources::root_endpoints(root.config(), vec![process_detail::spec()]);
    ui.settings_tx
        .send(EngineSettings {
            interval_ms,
            ..EngineSettings::default()
        })
        .expect("watch has receivers");
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });
    (fl, ui, handle)
}

fn stop(mut fl: FakeLab, ui: fake_sources::UiSide, handle: asupersync::runtime::TaskHandle<()>) {
    handle.abort();
    fl.advance(Duration::from_millis(10));
    assert!(handle.is_finished(), "engine_root returned after cancel");
    drop(ui);
    fl.finish();
}

/// E10-004: the Threads tab reaches the snapshot, and once the CPU column is warm an unchanged
/// tab keeps its `Arc` and version.
#[test]
fn engine_publishes_threads_tab_and_keeps_its_arc() {
    let root = FixtureRoot::new(&["process-edge"]);
    let (mut fl, ui, handle) = start_detail_engine(&root, 500);
    ui.demand_tx
        .send(tab_demand(DetailTab::Threads))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));

    let first = slot(&ui, DetailTab::Threads);
    assert_eq!(first.validity, Validity::Valid);
    let Some(TabPayload::Threads(rows)) = first.payload.as_deref() else {
        panic!("expected a Threads payload, got {:?}", first.payload);
    };
    let tids: Vec<u32> = rows.iter().map(|r| r.tid).collect();
    assert_eq!(tids, vec![4201, 4211]);

    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));
    let later = slot(&ui, DetailTab::Threads);
    assert_eq!(later.version, first.version);
    assert!(Arc::ptr_eq(
        later.payload.as_ref().expect("payload"),
        first.payload.as_ref().expect("payload")
    ));
    stop(fl, ui, handle);
}

/// E10-004: Environment is read once on open and again only on `DetailRefresh`.
#[test]
fn engine_environment_once_then_on_refresh() {
    let root = FixtureRoot::new(&["process-edge"]);
    root.write(4201, "environ", b"A=1\0");
    let (mut fl, ui, handle) = start_detail_engine(&root, 500);
    ui.demand_tx
        .send(tab_demand(DetailTab::Environment))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));
    assert_eq!(
        env_vars(&slot(&ui, DetailTab::Environment)),
        vec![("A".into(), "1".into())]
    );

    root.write(4201, "environ", b"B=2\0");
    fl.run_for(Duration::from_secs(3), Duration::from_millis(10));
    assert_eq!(
        env_vars(&slot(&ui, DetailTab::Environment)),
        vec![("A".into(), "1".into())]
    );

    ui.msgs_tx
        .try_send(EngineMsg::Command(Command::DetailRefresh {
            target: DetailTarget::Process(edge_key()),
            tab: DetailTab::Environment,
        }))
        .expect("the queue has room for the message");
    fl.run_for(Duration::from_secs(1), Duration::from_millis(10));
    assert_eq!(
        env_vars(&slot(&ui, DetailTab::Environment)),
        vec![("B".into(), "2".into())]
    );
    stop(fl, ui, handle);
}

/// A window that closes and opens again between two worker polls gets its tabs again: the
/// reducer drops the payloads on the close and sends a `DetailOpened` for each tab on the
/// reopen. With a 4 s interval the worker samples at 0 s, 4 s and 8 s, and the reducer ticks
/// every second.
#[test]
fn engine_reopened_tabs_between_polls_publish_again() {
    let root = FixtureRoot::new(&["process-edge"]);
    root.write(4201, "environ", b"A=1\0");
    let (mut fl, ui, handle) = start_detail_engine(&root, 4000);
    let open = Demand {
        any_surface_visible: true,
        details: Arc::from([
            DetailDemand {
                target: DetailTarget::Process(edge_key()),
                tab: DetailTab::Fds,
            },
            DetailDemand {
                target: DetailTarget::Process(edge_key()),
                tab: DetailTab::Environment,
            },
        ]),
        ..Demand::default()
    };
    ui.demand_tx
        .send(open.clone())
        .expect("watch has receivers");
    // Past the 4 s sample, which also serves the refresh of the first open.
    fl.run_for(Duration::from_millis(4300), Duration::from_millis(10));
    assert!(slot(&ui, DetailTab::Fds).payload.is_some());
    assert_eq!(
        env_vars(&slot(&ui, DetailTab::Environment)),
        vec![("A".into(), "1".into())]
    );

    // Closed for one reducer tick, which drops the payloads. The worker does not poll.
    ui.demand_tx
        .send(Demand {
            any_surface_visible: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    fl.run_for(Duration::from_millis(1200), Duration::from_millis(10));
    assert!(slot(&ui, DetailTab::Fds).payload.is_none());
    assert!(slot(&ui, DetailTab::Environment).payload.is_none());

    root.write(4201, "environ", b"B=2\0");
    ui.demand_tx.send(open).expect("watch has receivers");
    // Past the 8 s sample.
    fl.run_for(Duration::from_millis(3000), Duration::from_millis(10));
    let fds = slot(&ui, DetailTab::Fds);
    assert_eq!(fds.validity, Validity::Valid);
    assert!(matches!(fds.payload.as_deref(), Some(TabPayload::Fds(_))));
    assert_eq!(
        env_vars(&slot(&ui, DetailTab::Environment)),
        vec![("B".into(), "2".into())]
    );
    stop(fl, ui, handle);
}

fn general_rchar(slot: &TabSlot) -> u64 {
    match slot.payload.as_deref() {
        Some(TabPayload::General(g)) => {
            assert_eq!(g.io_rchar.validity, Validity::Valid);
            g.io_rchar.value
        }
        other => panic!("expected a General payload, got {other:?}"),
    }
}

/// tasklens-dwy.13: General reaches the snapshot beside the demanded tab. An unchanged General
/// keeps its `Arc` and version, and a rewritten `io` file gives a new payload.
#[test]
fn engine_general_follows_fixture_changes() {
    let root = FixtureRoot::new(&["process-edge"]);
    let (mut fl, ui, handle) = start_detail_engine(&root, 500);
    ui.demand_tx
        .send(tab_demand(DetailTab::Threads))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));
    let first = slot(&ui, DetailTab::General);
    assert_eq!(first.validity, Validity::Valid);
    assert_eq!(general_rchar(&first), 48_213_377);

    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));
    let same = slot(&ui, DetailTab::General);
    assert_eq!(same.version, first.version);
    assert!(Arc::ptr_eq(
        same.payload.as_ref().expect("payload"),
        first.payload.as_ref().expect("payload")
    ));

    let io = fs::read_to_string(root.pid_dir(4201).join("io")).expect("read io");
    root.write(4201, "io", io.replace("rchar: 48213377", "rchar: 50000000"));
    fl.run_for(Duration::from_secs(1), Duration::from_millis(10));
    let changed = slot(&ui, DetailTab::General);
    assert_eq!(general_rchar(&changed), 50_000_000);
    assert!(changed.version > first.version);
    stop(fl, ui, handle);
}

// Strings scan (tasklens-dwy.9) ---------------------------------------------------------------

/// The start address of the fake memory region of the Strings tests.
const MEM_BASE: u64 = 0x1000;

/// Gives process-edge 4201 one private anonymous region of `len` bytes at `MEM_BASE`, and a
/// sparse `mem` file that holds each `(offset, bytes)` at that offset in the region. `ProcMem`
/// reads `mem` with `pread` at the address, so this file is the memory the scan sees.
fn write_memory(root: &FixtureRoot, len: u64, data: &[(u64, &[u8])]) {
    root.write(
        4201,
        "maps",
        format!(
            "{MEM_BASE:08x}-{:08x} rw-p 00000000 00:00 0\n",
            MEM_BASE + len
        ),
    );
    let mem = fs::File::create(root.pid_dir(4201).join("mem")).expect("create mem");
    mem.set_len(MEM_BASE + len).expect("size mem");
    for (offset, bytes) in data {
        mem.write_all_at(bytes, MEM_BASE + offset)
            .expect("write mem");
    }
}

fn write_ptrace_scope(root: &FixtureRoot, scope: u8) {
    let dir = root.root().join("proc/sys/kernel/yama");
    fs::create_dir_all(&dir).expect("create yama dir");
    fs::write(dir.join("ptrace_scope"), format!("{scope}\n")).expect("write ptrace_scope");
}

fn strings_demand(key: ProcessKey) -> Demand {
    Demand {
        any_surface_visible: true,
        details: Arc::from([DetailDemand {
            target: DetailTarget::Process(key),
            tab: DetailTab::Strings,
        }]),
        ..Demand::default()
    }
}

fn strings_slot(ui: &fake_sources::UiSide, key: ProcessKey) -> TabSlot {
    let snap = ui.latest.load_full();
    snap.details
        .targets
        .get(&DetailTarget::Process(key))
        .map(|t| t.tab(DetailTab::Strings).clone())
        .unwrap_or_default()
}

fn scan_state(slot: &TabSlot) -> Option<Arc<StringScanState>> {
    match slot.payload.as_deref() {
        Some(TabPayload::Strings(state)) => Some(Arc::clone(state)),
        None => None,
        other => panic!("expected a Strings payload, got {other:?}"),
    }
}

fn send(ui: &fake_sources::UiSide, cmd: Command) {
    ui.msgs_tx
        .try_send(EngineMsg::Command(cmd))
        .expect("the queue has room for the message");
}

/// Starts the engine on `root` with a 500 ms interval, opens the Strings tab of `key`, and
/// sends scan 1 of `key` with `opts`.
fn begin_scan(
    root: &FixtureRoot,
    key: ProcessKey,
    opts: StringScanOpts,
) -> (
    FakeLab,
    fake_sources::UiSide,
    asupersync::runtime::TaskHandle<()>,
) {
    let (mut fl, ui, handle) = start_detail_engine(root, 500);
    ui.demand_tx
        .send(strings_demand(key))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(1), Duration::from_millis(10));
    send(
        &ui,
        Command::StringScan {
            scan: ScanId(1),
            key,
            opts,
        },
    );
    (fl, ui, handle)
}

/// Runs the lab in 10 ms steps for at most `limit` and returns each new Strings payload of
/// `key` with its validity. Stops after the first payload that is not `Scanning`.
fn collect_scan(
    fl: &mut FakeLab,
    ui: &fake_sources::UiSide,
    key: ProcessKey,
    limit: Duration,
) -> Vec<(Validity, Arc<StringScanState>)> {
    let mut seen = Vec::new();
    let mut version = strings_slot(ui, key).version;
    let end = fl.now_ms() + u64::try_from(limit.as_millis()).expect("short limit");
    while fl.now_ms() < end {
        fl.advance(Duration::from_millis(10));
        let slot = strings_slot(ui, key);
        if slot.version == version {
            continue;
        }
        version = slot.version;
        if let Some(state) = scan_state(&slot) {
            let done = state.phase != ScanPhase::Scanning;
            seen.push((slot.validity, state));
            if done {
                break;
            }
        }
    }
    seen
}

/// Address, encoding and value of every hit, in scan order.
fn hits(state: &StringScanState) -> Vec<(u64, StringEncoding, String)> {
    state
        .chunks
        .iter()
        .flat_map(|chunk| chunk.iter())
        .map(|h| (h.address, h.encoding, h.value.to_string()))
        .collect()
}

/// tasklens-dwy.9: with `ptrace_scope` 0 the scan streams payloads with rising progress, and the
/// last one is `Finished` with exactly the strings in memory.
#[test]
fn string_scan_streams_progress() {
    let root = FixtureRoot::new(&["process-edge"]);
    write_ptrace_scope(&root, 0);
    let strings: [(u64, &[u8]); 4] = [
        (0x10, b"alpha string"),
        (20 << 20, b"bravo string"),
        (40 << 20, b"delta string"),
        ((64 << 20) - 32, b"gamma string"),
    ];
    write_memory(&root, 64 << 20, &strings);
    let (mut fl, ui, handle) = begin_scan(&root, edge_key(), StringScanOpts::default());

    let payloads = collect_scan(&mut fl, &ui, edge_key(), Duration::from_secs(5));
    assert!(payloads.len() >= 2, "{} payloads", payloads.len());
    let pcts: Vec<u8> = payloads.iter().map(|(_, s)| s.progress_pct).collect();
    assert!(pcts.windows(2).all(|w| w[0] <= w[1]), "{pcts:?}");
    assert!(pcts[0] < 100, "{pcts:?}");
    for (validity, state) in &payloads {
        assert_eq!(*validity, Validity::Valid);
        assert_eq!(state.scan, ScanId(1));
    }
    let (_, last) = payloads.last().expect("a payload");
    assert_eq!(last.phase, ScanPhase::Finished);
    assert_eq!(last.progress_pct, 100);
    assert_eq!(last.found, 4);
    let expected: Vec<(u64, StringEncoding, String)> = strings
        .iter()
        .map(|&(offset, bytes)| {
            let text = std::str::from_utf8(bytes).expect("ASCII");
            (MEM_BASE + offset, StringEncoding::Ascii, text.to_string())
        })
        .collect();
    assert_eq!(hits(last), expected);
    stop(fl, ui, handle);
}

/// tasklens-dwy.9: with `ptrace_scope` 1 and a target that is not a descendant of tasklens, one
/// `Restricted` payload arrives and the memory is not scanned.
#[test]
fn string_scan_restricted_under_yama() {
    let root = FixtureRoot::new(&["process-edge"]);
    write_ptrace_scope(&root, 1);
    write_memory(&root, 1 << 20, &[(0x10, b"secret value")]);
    let (mut fl, ui, handle) = begin_scan(&root, edge_key(), StringScanOpts::default());
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));

    let slot = strings_slot(&ui, edge_key());
    assert_eq!(slot.version, 1, "exactly one payload");
    assert_eq!(slot.validity, Validity::Restricted);
    let state = scan_state(&slot).expect("a Strings payload");
    assert_eq!(state.scan, ScanId(1));
    assert_eq!(
        state.phase,
        ScanPhase::Failed("Strings unavailable: needs administrator rights".into())
    );
    assert_eq!(state.found, 0);
    assert!(state.chunks.is_empty());
    stop(fl, ui, handle);
}

/// tasklens-dwy.9: with `ptrace_scope` 1 a target whose parent chain reaches tasklens is
/// scanned.
#[test]
fn string_scan_descendant_allowed_under_yama() {
    let root = FixtureRoot::new(&["process-edge"]);
    write_ptrace_scope(&root, 1);
    write_memory(&root, 1 << 20, &[(0x10, b"child memory")]);
    // 4201's parent is 4100. Make tasklens the parent of 4100.
    let me = std::process::id();
    let stat = fs::read_to_string(root.pid_dir(4201).join("stat")).expect("read stat");
    let parent = stat.replace(
        "4201 (Web Content) S 4100",
        &format!("4100 (parent) S {me}"),
    );
    fs::create_dir_all(root.pid_dir(4100)).expect("create parent dir");
    root.write(4100, "stat", parent);
    let (mut fl, ui, handle) = begin_scan(&root, edge_key(), StringScanOpts::default());

    let payloads = collect_scan(&mut fl, &ui, edge_key(), Duration::from_secs(2));
    let (validity, last) = payloads.last().expect("a payload");
    assert_eq!(*validity, Validity::Valid);
    assert_eq!(last.phase, ScanPhase::Finished);
    assert_eq!(
        hits(last),
        vec![(
            MEM_BASE + 0x10,
            StringEncoding::Ascii,
            "child memory".to_string()
        )]
    );
    stop(fl, ui, handle);
}

/// tasklens-dwy.9: `CancelStringScan` after the first chunks stops the scan. The final payload
/// is `Cancelled`, and nothing changes after it.
#[test]
fn string_scan_cancel() {
    let root = FixtureRoot::new(&["process-edge"]);
    write_ptrace_scope(&root, 0);
    // One string every 4 MiB.
    let data: Vec<(u64, &[u8])> = (0..16).map(|i| (i << 22, &b"some string"[..])).collect();
    write_memory(&root, 64 << 20, &data);
    let (mut fl, ui, handle) = begin_scan(&root, edge_key(), StringScanOpts::default());

    // Cancel as soon as the first payload arrives.
    let version = strings_slot(&ui, edge_key()).version;
    while strings_slot(&ui, edge_key()).version == version {
        assert!(fl.now_ms() < 10_000, "no Strings payload");
        fl.advance(Duration::from_millis(10));
    }
    let first = scan_state(&strings_slot(&ui, edge_key())).expect("a Strings payload");
    assert_eq!(first.phase, ScanPhase::Scanning);
    send(&ui, Command::CancelStringScan(ScanId(1)));

    let rest = collect_scan(&mut fl, &ui, edge_key(), Duration::from_secs(2));
    let (validity, last) = rest.last().expect("a final payload");
    assert_eq!(*validity, Validity::Valid);
    assert_eq!(last.phase, ScanPhase::Cancelled);
    assert!(last.progress_pct < 100, "{}", last.progress_pct);
    assert!(last.found < 16, "{}", last.found);

    let at_cancel = strings_slot(&ui, edge_key());
    fl.run_for(Duration::from_secs(1), Duration::from_millis(10));
    let later = strings_slot(&ui, edge_key());
    assert_eq!(later.version, at_cancel.version);
    assert_eq!(
        scan_state(&later).expect("payload").found,
        last.found,
        "no chunk is read after the cancel"
    );
    stop(fl, ui, handle);
}

/// tasklens-dwy.9: memory with 1,000,001 strings stops the scan at 1,000,000 with
/// `StoppedLimit`, which the UI shows as "String scan stopped: not enough memory.".
#[test]
fn string_scan_limit() {
    let root = FixtureRoot::new(&["process-edge"]);
    write_ptrace_scope(&root, 0);
    let data = b"abcd\0".repeat(1_000_001);
    let len = (data.len() as u64).next_multiple_of(4096);
    write_memory(&root, len, &[(0, &data)]);
    let opts = StringScanOpts {
        ascii: true,
        utf8: false,
        utf16le: false,
        ..StringScanOpts::default()
    };
    let (mut fl, ui, handle) = begin_scan(&root, edge_key(), opts);

    let payloads = collect_scan(&mut fl, &ui, edge_key(), Duration::from_secs(5));
    let (validity, last) = payloads.last().expect("a payload");
    assert_eq!(*validity, Validity::Valid);
    assert_eq!(last.phase, ScanPhase::StoppedLimit);
    assert_eq!(last.found, 1_000_000);
    let kept: usize = last.chunks.iter().map(|c| c.len()).sum();
    assert_eq!(kept, 1_000_000);
    stop(fl, ui, handle);
}

/// tasklens-dwy.9: a kernel thread is never scanned.
#[test]
fn string_scan_kernel_thread() {
    let root = FixtureRoot::new(&["process-edge"]);
    // process-edge pid 2 is kthreadd.
    let kthread = ProcessKey {
        start_ticks: 19,
        boot_id: BootId(1),
        pid: 2,
    };
    let (mut fl, ui, handle) = begin_scan(&root, kthread, StringScanOpts::default());

    let payloads = collect_scan(&mut fl, &ui, kthread, Duration::from_secs(2));
    assert_eq!(payloads.len(), 1);
    let (validity, state) = &payloads[0];
    assert_eq!(*validity, Validity::Unsupported);
    assert_eq!(
        state.phase,
        ScanPhase::Failed("Strings unavailable for this process.".into())
    );
    stop(fl, ui, handle);
}
