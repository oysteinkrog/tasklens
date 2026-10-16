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
use tl_engine::workers::{process_detail, processes};
use tl_model::demand::{ColumnSet, Demand};
use tl_model::ids::{ColumnId, ProcessKey};
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
