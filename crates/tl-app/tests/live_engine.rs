//! The real engine on a fixture root, wired to the UI as `main.rs` does (tasklens-pdu.52,
//! tasklens-hk6.6).
//!
//! Every UI test uses `FakeEngine`, so nothing checked that a page asks the real engine for its
//! data. The Processes test starts `Engine::start` on a copy of the
//! `owner-desktop` fixture with `process-edge` laid over it (as the engine lab tests use), runs
//! `TasklensUi::init` with the engine as the client, opens the main window on the Processes tab
//! and waits for a filled process table with CPU and Memory totals. The Startup test activates
//! the Startup page state and waits for the view built for its query. The Startup page test opens
//! the main window on the Startup tab and waits for the page's table and status text.
//! The Services test (tasklens-hk6.12) expands a unit on a lone Services table and waits for
//! the process sweep its demand turns on.
//!
//! GPUI's test scheduler fails a test when another thread wakes one of its tasks, so the UI gets
//! the engine through `Pumped`, whose wake channel the test thread feeds.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::Root;
use gpui_kit::{AppContext as _, Context, Entity, TestAppContext, VisualTestContext, Window};
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::EngineHandle;
use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode, ShutdownReport};
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, Tab};
use tl_model::demand::Demand;
use tl_model::health::{ManagerState, SourceId};
use tl_model::ids::ProcessKey;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::settings::EngineSettings;
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{ServicesSnapshot, Snapshot};
use tl_model::table::{
    Manager, ServiceColumn, StartupColumn, SysColumnId, SysNodeId, SysRow, SysSortSpec, SysTable,
    UnitRef,
};
use tl_model::units;
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::demand::GlobalDemand;
use tl_ui::hub::GlobalHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::pages::services::delegate::{ServicesDelegate, row_key};
use tl_ui::pages::startup::StartupPage;
use tl_ui::pages::sys_table::{SysBody, SysDelegate as _, SysPageState, SysQuery};
use tl_ui::table::cells::CellOut;
use tl_ui::table::tree_table::{RowKind, TlTreeTable, TreeTableDelegate as _};
use tl_ui::window::main::{
    MainWindow, PageSlot, main_window_view, open_main_window_on, set_test_page,
};

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
    // Prints the engine workers' exit lines (`tasklens::engine`, debug) with `--nocapture`.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("tasklens::engine=debug")
        .with_test_writer()
        .try_init();
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

/// A copy of `owner-desktop` with `process-edge` over it, in a tempdir on disk under target
/// (not in the tmpfs `/tmp`). Returns the tempdir and the root inside it.
fn fixture_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
    let root = dir.path().join("root");
    overlay(&fixtures().join("owner-desktop"), &root);
    overlay(&fixtures().join("process-edge"), &root);
    (dir, root)
}

/// `TasklensUi::init` with `client`, as `main.rs` runs it.
fn init_ui(cx: &mut TestAppContext, client: Arc<dyn EngineClient>) {
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
}

#[gpui_kit::test]
fn processes_tab_fills_from_real_engine(cx: &mut TestAppContext) {
    let (dir, root) = fixture_root();
    let mut cpu_ticks = CpuTicks::new(&root);
    let engine = start_engine(dir.path(), &root);
    let pumped = Arc::new(Pumped::new(engine.clone()));
    init_ui(cx, pumped.clone());
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
        cx.global::<GlobalDemand>()
            .0
            .read(cx)
            .merged()
            .process_table
    });
    let hidden = vcx.update(|_, cx| cx.global::<GlobalHub>().0.read(cx).is_hidden());
    let engine_records = engine.latest().processes.records.len();
    let report = engine.shutdown();

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
    assert_stopped(&report);
}

/// These tests are about the UI, not about shutdown, so they check that shutdown returned and
/// teardown finished, never `drained`. The drain bound is fixed at 1 s by ADR-004 and a single
/// engine thread starved by a loaded machine can miss it. Drain is tested in `tl-engine`
/// (`lab_bridge`, `shutdown_during_start`).
fn assert_stopped(report: &ShutdownReport) {
    // Diagnostic for tasklens-cr0.30: `--nocapture` shows `drained` and the worker-exit lines.
    eprintln!("shutdown report: {report:?}");
    assert!(report.torn_down, "engine torn down on shutdown: {report:?}");
}

/// The Startup page state against the real engine: activation turns the startup demand on and
/// sends the query, and the engine answers with a view for that generation. The scan reads the
/// host's XDG autostart folders, so the rows may be empty; the view's generation and validity
/// are what is checked.
#[gpui_kit::test]
fn startup_view_arrives_from_real_engine(cx: &mut TestAppContext) {
    let (dir, root) = fixture_root();
    let engine = start_engine(dir.path(), &root);
    let pumped = Arc::new(Pumped::new(engine.clone()));
    let client: Arc<dyn EngineClient> = pumped.clone();
    init_ui(cx, Arc::clone(&client));
    let demand = cx.update(|cx| cx.global::<GlobalDemand>().0.clone());
    let mut page = SysPageState::new(Tab::Startup, &[SourceId::Startup]);
    let sort = SysSortSpec {
        column: SysColumnId::Startup(StartupColumn::Name),
        ascending: true,
    };
    let mut query = SysQuery::new(SysTable::Startup, sort, None);
    cx.update(|cx| page.on_activated(&mut query, &demand, cx));
    assert!(query.last_sent() > 0, "the first visit sends the query");

    let read = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let snap = cx.global::<GlobalHub>().0.read(cx).current().clone();
            (
                snap.startup.query_generation,
                snap.startup.header.validity,
                snap.startup.rows.len(),
            )
        })
    };
    let begin = Instant::now();
    let mut last = read(cx);
    while begin.elapsed() < DEADLINE {
        let (generation, validity, _) = last;
        if query.accepts(generation) && validity == Validity::Valid {
            break;
        }
        pumped.pump();
        cx.executor().advance_clock(STEP);
        cx.run_until_parked();
        query.on_snapshot(&*client);
        std::thread::sleep(Duration::from_millis(20));
        last = read(cx);
    }
    let demand_on = cx.update(|cx| demand.read(cx).merged());
    let report = engine.shutdown();

    let (generation, validity, rows) = last;
    eprintln!(
        "generation {generation} (sent {}), validity {validity:?}, rows {rows}",
        query.last_sent()
    );
    assert!(demand_on.any_surface_visible && demand_on.startup);
    assert!(query.accepts(generation), "the view is for the query sent");
    assert_eq!(validity, Validity::Valid, "the startup scan ran");
    assert!(page.note_data(validity, rows), "first data seen");
    assert_stopped(&report);
}

/// The Startup arm of the page match, until tasklens-9o0.13 registers it in `app.rs`.
fn startup_page(window: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
    let hub = cx.global::<GlobalHub>().0.clone();
    let demand = cx.global::<GlobalDemand>().0.clone();
    let client = Arc::clone(&cx.global::<tl_ui::app::EngineGlobal>().0);
    let page = cx.new(|cx| StartupPage::new(&hub, &demand, client, window, cx));
    PageSlot::new(page, cx)
}

/// The Startup page against the real engine (tasklens-9o0.7, leader comment 159): the main window
/// opens on the Startup tab, the page turns its demand on, and its table and status text fill
/// from the engine's scan. The scan reads the host's autostart folders, so the rows and counts are
/// compared with the engine's own view, not with fixed numbers.
#[gpui_kit::test]
fn startup_page_fills_from_real_engine(cx: &mut TestAppContext) {
    let (dir, root) = fixture_root();
    let mut cpu_ticks = CpuTicks::new(&root);
    let engine = start_engine(dir.path(), &root);
    let pumped = Arc::new(Pumped::new(engine.clone()));
    init_ui(cx, pumped.clone());
    cx.update(|cx| set_test_page(cx, Tab::Startup, startup_page));
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Startup, cx))
        .expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();

    let page = vcx.update(|_, cx| {
        main_window_view(cx)
            .expect("main window")
            .read(cx)
            .page_entity::<StartupPage>(Tab::Startup)
            .expect("the Startup page is built")
    });
    // Page body, page status text, page rows, then the hub's startup rows and counts.
    let read = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let page = page.read(cx);
            let mut status = String::new();
            page.status_text(&mut status);
            let rows = page.table().read(cx).delegate.row_count();
            let snap = cx.global::<GlobalHub>().0.read(cx).current().clone();
            (
                page.body(),
                status,
                rows,
                snap.startup.rows.len(),
                snap.startup.counts,
            )
        })
    };

    let begin = Instant::now();
    let mut last = read(&mut vcx);
    while begin.elapsed() < DEADLINE {
        let (body, status, ..) = &last;
        if *body != SysBody::Skeleton && status != "Loading…" {
            break;
        }
        step(&mut vcx, &mut cpu_ticks, &pumped);
        last = read(&mut vcx);
    }
    let demand_on = vcx.update(|_, cx| cx.global::<GlobalDemand>().0.read(cx).merged());
    let report = engine.shutdown();

    let (body, status, rows, hub_rows, counts) = last;
    eprintln!("body {body:?}, status {status:?}, rows {rows}, hub rows {hub_rows}, {counts:?}");
    assert!(
        demand_on.any_surface_visible && demand_on.startup,
        "the Startup tab sends startup demand"
    );
    assert!(
        matches!(body, SysBody::Rows | SysBody::NoEntries),
        "the page left the skeleton for data"
    );
    assert_eq!(rows, hub_rows, "the table shows the engine's rows");
    let mut total = String::new();
    units::count(&mut total, u64::from(counts.total));
    assert!(
        status.starts_with(&format!("{total} Total | ")),
        "the status text shows the engine's counts"
    );
    assert_stopped(&report);
}

/// `base` with a services list of one expanded sshd.service unit and one child row per key.
fn with_unit(base: &Snapshot, unit: &UnitRef, children: &[ProcessKey]) -> Arc<Snapshot> {
    let mut rows = vec![
        SysRow {
            node: SysNodeId::ServiceGroup(Manager::System),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        },
        SysRow {
            node: SysNodeId::Unit(unit.clone()),
            depth: 1,
            expanded: true,
            has_children: true,
            record: None,
        },
    ];
    rows.extend(children.iter().map(|key| SysRow {
        node: SysNodeId::UnitProcess(unit.clone(), *key),
        depth: 2,
        expanded: false,
        has_children: false,
        record: None,
    }));
    let mut snap = base.clone();
    snap.services = Arc::new(ServicesSnapshot {
        rows: rows.into(),
        managers: [ManagerState::Connected; 2],
        ..ServicesSnapshot::default()
    });
    Arc::new(snap)
}

/// Services child rows against the real engine (tasklens-hk6.12, leader comment 161). No
/// Processes page exists: expanding a unit on the active Services table turns on the process
/// sweep, a child row shows the engine's record, and collapsing the unit withdraws the demand.
#[gpui_kit::test]
fn services_child_demand_runs_process_sweep(cx: &mut TestAppContext) {
    let (dir, root) = fixture_root();
    let mut cpu_ticks = CpuTicks::new(&root);
    let engine = start_engine(dir.path(), &root);
    let pumped = Arc::new(Pumped::new(engine.clone()));
    init_ui(cx, pumped.clone());
    let slot = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    let handle = cx.add_window(move |window, cx| {
        let delegate = ServicesDelegate::new(&UiState::default());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        *keep.borrow_mut() = Some(table.clone());
        Root::new(table, window, cx)
    });
    let table: Entity<TlTreeTable<ServicesDelegate>> =
        slot.borrow().clone().expect("the services table");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();

    let unit = UnitRef {
        manager: Manager::System,
        name: Arc::from("sshd.service"),
    };
    let install = |vcx: &mut VisualTestContext, snap: Arc<Snapshot>| {
        vcx.update(|_, cx| {
            table.update(cx, |t, cx| {
                t.delegate.install(&snap);
                t.on_new_version(cx);
            });
        });
    };
    let toggle = |vcx: &mut VisualTestContext| {
        let id = row_key(&SysNodeId::Unit(unit.clone()));
        vcx.update(|_, cx| {
            table.update(cx, |t, cx| t.delegate_mut().on_toggle_expand(id, cx));
        });
    };
    let merged = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| cx.global::<GlobalDemand>().0.read(cx).merged())
    };
    install(&mut vcx, with_unit(&engine.latest(), &unit, &[]));
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| t.delegate_mut().set_active(true, cx));
    });
    let before = merged(&mut vcx);
    toggle(&mut vcx);

    let begin = Instant::now();
    while begin.elapsed() < DEADLINE {
        if !engine.latest().processes.records.is_empty() && merged(&mut vcx).process_table {
            break;
        }
        step(&mut vcx, &mut cpu_ticks, &pumped);
    }
    let expanded = merged(&mut vcx);
    let latest = engine.latest();
    let child = latest.processes.records.first().map(|r| r.key);

    // A child row for an engine record shows its PID and is reported as a detail key.
    let mut pid_text = String::new();
    let mut detail_keys = Vec::new();
    if let Some(key) = child {
        install(&mut vcx, with_unit(&latest, &unit, &[key]));
        step(&mut vcx, &mut cpu_ticks, &pumped);
        pid_text = vcx.update(|_, cx| {
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            let d = &table.read(cx).delegate;
            d.cell(2, SysColumnId::Services(ServiceColumn::Pid), &mut out);
            out.text
        });
        detail_keys = merged(&mut vcx).detail_keys.to_vec();
    }

    toggle(&mut vcx);
    step(&mut vcx, &mut cpu_ticks, &pumped);
    let collapsed = merged(&mut vcx);
    let report = engine.shutdown();

    eprintln!(
        "records {}, child {child:?}, pid text {pid_text:?}, detail keys {detail_keys:?}",
        latest.processes.records.len()
    );
    assert!(
        before.any_surface_visible && before.services && !before.process_table,
        "no sweep before a unit is expanded"
    );
    assert!(
        expanded.process_table,
        "an expanded unit asks for the sweep"
    );
    let key = child.expect("the sweep published process records");
    assert_eq!(
        pid_text,
        key.pid.to_string(),
        "the child row shows the engine's PID"
    );
    assert_eq!(detail_keys, [key], "the visible child is a detail key");
    assert!(
        collapsed.any_surface_visible && collapsed.services,
        "the page stays visible"
    );
    assert!(
        !collapsed.process_table,
        "collapsing the last unit withdraws the sweep"
    );
    assert!(collapsed.detail_keys.is_empty());
    assert_stopped(&report);
}
