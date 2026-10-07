//! The real engine on a fixture root, wired to the UI as `main.rs` does (tasklens-pdu.52).
//!
//! Every UI test uses `FakeEngine`, so nothing checked that the Processes tab asks the real
//! engine for the process sweep. This test starts `Engine::start` on a copy of the
//! `owner-desktop` fixture with `process-edge` laid over it (as the engine lab tests use), runs
//! `TasklensUi::init` with the engine as the client, opens the main window on the Processes tab
//! and waits for a filled process table with CPU and Memory totals.
//!
//! GPUI's test scheduler fails a test when another thread wakes one of its tasks, so the UI gets
//! the engine through `Pumped`, whose wake channel the test thread feeds.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{TestAppContext, VisualTestContext};
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::EngineHandle;
use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode};
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, Tab};
use tl_model::demand::Demand;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::settings::EngineSettings;
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::i18n::init_strings;
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::table::tree_table::{RowKind, TreeTableDelegate as _};
use tl_ui::window::main::{main_window_view, open_main_window_on};

/// How long the engine gets to fill the table. The bead asks for 2 s in the real app; a debug
/// test build under a loaded machine gets more.
const DEADLINE: Duration = Duration::from_secs(20);
/// Fake time moved per step: past the 100 ms demand flush, below the 1 s hidden check.
const STEP: Duration = Duration::from_millis(100);

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

/// Moves the `cpu` line of the copied `proc/stat` on by 100 busy and 100 idle ticks per call.
/// A frozen fixture gives a zero tick delta, and the CPU total then stays `WarmingUp`.
struct CpuTicks {
    path: PathBuf,
    text: String,
    calls: u64,
}

impl CpuTicks {
    fn new(root: &Path) -> Self {
        let path = root.join("proc/stat");
        let text = fs::read_to_string(&path).expect("read proc/stat");
        Self {
            path,
            text,
            calls: 0,
        }
    }

    fn advance(&mut self) {
        self.calls += 1;
        let add = 100 * self.calls;
        let (first, rest) = self.text.split_once('\n').expect("proc/stat has lines");
        let mut fields: Vec<String> = first.split_whitespace().map(str::to_owned).collect();
        assert_eq!(fields[0], "cpu");
        for ix in [1, 4] {
            let n: u64 = fields[ix].parse().expect("tick count");
            fields[ix] = (n + add).to_string();
        }
        fs::write(&self.path, format!("{}\n{rest}", fields.join(" "))).expect("write proc/stat");
    }
}

/// The engine on `root`, app lookups on the tl-collect app fixtures and the icon cache in
/// `dir`, so the test never writes the real `~/.cache`. No helper task.
fn start_engine(dir: &Path, root: &Path) -> tl_engine::EngineHandle {
    let share = |name: &str| fixtures().join(name).join("usr/share");
    let dirs = [share("apps-desktop"), share("apps-icons")];
    let paths = AppPaths {
        home: fixtures().join("apps-icons/home"),
        xdg_data_home: None,
        xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
        xdg_config_home: None,
        icon_cache: dir.join("cache/icons.toml"),
    };
    let config = EngineConfig::new(EngineMode::Full, dir.join("state"))
        .with_history_dir(dir.join("history"))
        .with_sys_root(SysRoot::at(root))
        .with_app_paths(paths);
    let hooks = EngineHooks {
        helper: None,
        ..EngineHooks::default()
    };
    Engine::start(config, hooks).expect("engine starts")
}

/// The engine handle with UI-side channels that only the test thread wakes. `pump` forwards the
/// engine's wakes. No action is sent here, so the action event channel stays empty.
struct Pumped {
    engine: EngineHandle,
    engine_wake: async_channel::Receiver<()>,
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    _events_tx: async_channel::Sender<ActionEvent>,
    events_rx: async_channel::Receiver<ActionEvent>,
}

impl Pumped {
    fn new(engine: EngineHandle) -> Self {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, events_rx) = async_channel::bounded(1);
        Self {
            engine_wake: engine.wake(),
            engine,
            wake_tx,
            wake_rx,
            _events_tx: events_tx,
            events_rx,
        }
    }

    /// Passes a pending engine wake on, on the calling thread.
    fn pump(&self) {
        if self.engine_wake.try_recv().is_ok() {
            let _ = self.wake_tx.try_send(());
        }
    }
}

impl EngineClient for Pumped {
    fn latest(&self) -> Arc<Snapshot> {
        self.engine.latest()
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.events_rx.clone()
    }

    fn set_demand(&self, demand: Demand) {
        self.engine.set_demand(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        self.engine.set_settings(settings);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        self.engine.send(cmd)
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        self.engine.journal(query)
    }
}

/// One step: fake time for the UI timers, a drawn frame with its next-frame callbacks (the hub
/// counts the window as shown from those, ADR-008), and real time for the engine threads.
fn step(vcx: &mut VisualTestContext, cpu: &mut CpuTicks, pumped: &Pumped) {
    cpu.advance();
    pumped.pump();
    vcx.executor().advance_clock(STEP);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
    });
    std::thread::sleep(Duration::from_millis(20));
}

#[gpui_kit::test]
fn processes_tab_fills_from_real_engine(cx: &mut TestAppContext) {
    // On disk under target, not in the tmpfs `/tmp`.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
    let root = dir.path().join("root");
    overlay(&fixtures().join("owner-desktop"), &root);
    overlay(&fixtures().join("process-edge"), &root);
    let mut cpu_ticks = CpuTicks::new(&root);
    let engine = start_engine(dir.path(), &root);
    let pumped = Arc::new(Pumped::new(engine.clone()));
    let client: Arc<dyn EngineClient> = pumped.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        init_strings(cx, "en-US");
        cx.set_reduce_motion(true);
    });
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Processes, cx))
        .expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();

    let page = vcx.update(|_, cx| {
        main_window_view(cx)
            .expect("main window")
            .read(cx)
            .page_entity::<ProcessesPage>(Tab::Processes)
            .expect("the Processes page is built")
    });
    // Process and app rows shown, records, CPU validity, Memory validity. The empty table already has
    // the group headers and the Interrupts row, so only process and app rows count.
    let read = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let table = page.read(cx).table().read(cx);
            let snap = table.delegate.snapshot();
            let totals = &snap.table.totals;
            let d = &table.delegate;
            (
                (0..d.row_count())
                    .filter(|&ix| matches!(d.row_kind(ix), RowKind::Process | RowKind::App))
                    .count(),
                snap.processes.records.len(),
                totals.cpu_pct.validity,
                totals.memory_pct.validity,
            )
        })
    };

    let begin = Instant::now();
    let mut last = read(&mut vcx);
    while begin.elapsed() < DEADLINE {
        let (rows, records, cpu, memory) = last;
        if rows > 0 && records > 0 && cpu == Validity::Valid && memory == Validity::Valid {
            break;
        }
        step(&mut vcx, &mut cpu_ticks, &pumped);
        last = read(&mut vcx);
    }
    let demand_on = vcx.update(|_, cx| {
        cx.global::<tl_ui::demand::GlobalDemand>()
            .0
            .read(cx)
            .merged()
            .process_table
    });
    let hidden = vcx.update(|_, cx| cx.global::<tl_ui::hub::GlobalHub>().0.read(cx).is_hidden());
    let engine_records = engine.latest().processes.records.len();
    let report = engine.shutdown(Duration::from_millis(1500));

    let (rows, records, cpu, memory) = last;
    eprintln!(
        "rows {rows}, records {records}, engine records {engine_records}, cpu {cpu:?}, \
         memory {memory:?}, hidden {hidden}"
    );
    assert!(!hidden, "the hub counts the drawn window as shown");
    assert!(demand_on, "the Processes tab sends process_table demand");
    assert!(records > 0, "the sweep published process records");
    assert!(rows > 0, "the table shows process rows");
    assert_eq!(cpu, Validity::Valid, "CPU total filled");
    assert_eq!(memory, Validity::Valid, "Memory total filled");
    assert!(report.drained, "engine drained on shutdown");
}
