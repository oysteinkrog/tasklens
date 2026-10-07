//! The real engine on a fixture root, wired to the UI as `main.rs` does (tasklens-pdu.52).
//!
//! Every UI test uses `FakeEngine`, so nothing checked that the Processes tab asks the real
//! engine for the process sweep. This test starts `Engine::start` on the `owner-desktop` fixture,
//! runs `TasklensUi::init` with the engine as the client, opens the main window on the Processes
//! tab and waits for a filled process table with CPU and Memory totals.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{TestAppContext, VisualTestContext};
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::settings::{Settings, UiState};
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

/// The engine on the `owner-desktop` root, app lookups on the tl-collect app fixtures and the
/// icon cache in `dir`, so the test never writes the real `~/.cache`. No helper task.
fn start_engine(dir: &std::path::Path) -> tl_engine::EngineHandle {
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
        .with_sys_root(SysRoot::at(&fixtures().join("owner-desktop")))
        .with_app_paths(paths);
    let hooks = EngineHooks {
        helper: None,
        ..EngineHooks::default()
    };
    Engine::start(config, hooks).expect("engine starts")
}

/// One step: fake time for the UI timers, a drawn frame with its next-frame callbacks (the hub
/// counts the window as shown from those, ADR-008), and real time for the engine threads.
fn step(vcx: &mut VisualTestContext) {
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
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = start_engine(dir.path());
    let client: Arc<dyn EngineClient> = Arc::new(engine.clone());
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
        step(&mut vcx);
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
