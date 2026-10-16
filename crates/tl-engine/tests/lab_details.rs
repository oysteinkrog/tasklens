//! LabRuntime tests for the `ProcessDetail` worker through `engine_root` on fixture roots
//! (ADR-013, ADR-008). Each test copies `owner-desktop` into the target tmpdir so it can add
//! `smaps_rollup` files.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::fs;
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
use tl_model::details::{DetailTab, DetailTarget, TabPayload, TabSlot};
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
