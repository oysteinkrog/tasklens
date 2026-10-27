//! The "tasklens SMART Data" window (E25, PERF-051, UI-PRF-030).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, DeviceCommand, Tab};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{DiskDetails, SmartAttr, SmartData};
use tl_model::table::Total;
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::PerfShowSmart;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;
use tl_ui::window::main::{main_window_view, open_main_window_on};
use tl_ui::windows::smart::{SmartColumn, SmartWindow, find_smart};

const IDX: DeviceIdx = DeviceIdx(3);

fn key() -> DeviceKey {
    DeviceKey(Arc::from("disk:sda"))
}

fn valid<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn attr(id: u8, name: &str, normalized: i32, assessment: &str) -> SmartAttr {
    SmartAttr {
        id,
        name: Arc::from(name),
        value: 0,
        normalized,
        threshold: 10,
        worst: normalized,
        prefail: id % 2 == 1,
        online: true,
        assessment: Arc::from(assessment),
    }
}

fn data(updated_s: i64, ata: Vec<SmartAttr>, nvme: Vec<(&str, &str)>) -> SmartData {
    SmartData {
        updated_s,
        health: valid(Arc::from("OK")),
        power_on_s: valid(100_800),
        selftest: valid(Arc::from("Completed without error")),
        ata: ata.into(),
        nvme: nvme
            .into_iter()
            .map(|(k, v)| (Arc::<str>::from(k), Arc::<str>::from(v)))
            .collect(),
    }
}

fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![DeviceEntry {
                idx: IDX,
                identity: DeviceIdentity::Disk {
                    major: 8,
                    minor: 0,
                    name: Arc::from("sda"),
                    wwn_or_serial: None,
                },
                generation: 0,
                display_name: Arc::from("sda"),
                present: true,
                parent: None,
                persistent_key: Arc::from(""),
                net_kind: None,
                hot_c: None,
            }]),
        });
    });
    let client: Arc<dyn EngineClient> = engine.clone();
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
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_reduce_motion(true);
    });
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Services, cx))
        .expect("the main window opens");
    let mut main = VisualTestContext::from_window(handle.into(), cx);
    main.update(|window, _| window.activate_window());
    main.run_until_parked();
    (engine, main)
}

/// Publishes the disk record with `smart` in the given validity.
fn publish(
    main: &mut VisualTestContext,
    engine: &FakeEngine,
    smart: Option<(Validity, SmartData)>,
) {
    engine.publish_with(move |s| {
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.disks = Arc::from([(
            IDX,
            DiskDetails {
                smart_data: smart.map(|(validity, d)| Total {
                    value: Arc::new(d),
                    validity,
                }),
                ..DiskDetails::default()
            },
        )]);
    });
    main.run_until_parked();
}

fn open(main: &mut VisualTestContext) -> (VisualTestContext, gpui_kit::Entity<SmartWindow>) {
    main.update(|window, cx| window.dispatch_action(Box::new(PerfShowSmart { key: key() }), cx));
    main.run_until_parked();
    let (handle, view) = main
        .update(|_, cx| find_smart(IDX, cx))
        .expect("the SMART window is open");
    let mut vcx = VisualTestContext::from_window(handle.into(), &main.cx);
    redraw(&mut vcx);
    (vcx, view)
}

fn redraw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

fn smart_reads(engine: &FakeEngine) -> Vec<bool> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Device(DeviceCommand::SmartRead { disk, update }) => {
                assert_eq!(&*disk, "sda");
                Some(update)
            }
            _ => None,
        })
        .collect()
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx.debug_bounds(selector).expect(selector).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn window_count(main: &mut VisualTestContext) -> usize {
    main.update(|_, cx| {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<gpui_kit::base::Root>())
            .filter(|h| {
                tl_ui::components::window_frame::TlSecondaryWindow::<SmartWindow>::find(*h, cx)
                    .is_some()
            })
            .count()
    })
}

#[gpui_kit::test]
fn smart_window_single_instance(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish(&mut main, &engine, None);
    let (_vcx, _view) = open(&mut main);
    assert_eq!(window_count(&mut main), 1);
    // Never read: opening asked for the data once.
    assert_eq!(smart_reads(&engine), vec![false]);
    main.update(|window, cx| window.dispatch_action(Box::new(PerfShowSmart { key: key() }), cx));
    main.run_until_parked();
    assert_eq!(window_count(&mut main), 1);
    assert_eq!(smart_reads(&engine), vec![false]);
}

#[gpui_kit::test]
fn smart_assessment_colours(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    let ata = vec![
        attr(1, "Raw_Read_Error_Rate", 100, "OK"),
        attr(5, "Reallocated_Sector_Ct", 90, "Failed in the past"),
        attr(9, "Power_On_Hours", 80, "Failing"),
    ];
    publish(
        &mut main,
        &engine,
        Some((Validity::Valid, data(1_000, ata, vec![]))),
    );
    let (mut vcx, view) = open(&mut main);
    redraw(&mut vcx);
    let theme = vcx.update(|_, cx| current(cx).clone());
    let table = vcx.update(|_, cx| view.read(cx).table().clone());
    let cells: Vec<_> = vcx.update(|_, cx| {
        table.update(cx, |t, _| {
            let d = t.delegate_mut();
            (0..3)
                .map(|i| d.cell_of(i, SmartColumn::Assessment))
                .collect()
        })
    });
    assert_eq!(cells[0], ("OK".to_owned(), Some(theme.text_primary)));
    assert_eq!(
        cells[1],
        ("Failed in the past".to_owned(), Some(theme.status.warning))
    );
    assert_eq!(cells[2], ("FAILING".to_owned(), Some(theme.status.danger)));
}

#[gpui_kit::test]
fn smart_nvme_rows_order(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    let labels = [
        "Percent used",
        "Available spare",
        "Spare threshold",
        "Total data read",
        "Total data written",
        "Warning temp time",
        "Critical temp time",
        "Controller busy time",
        "Warning temperature",
        "Critical temperature",
        "Temperature sensors",
        "Unsafe shutdowns",
        "Media errors",
        "Error log entries",
        "Power cycles",
    ];
    let rows: Vec<_> = labels.iter().map(|l| (*l, "1")).collect();
    publish(
        &mut main,
        &engine,
        Some((Validity::Valid, data(1_000, vec![], rows))),
    );
    let (mut vcx, view) = open(&mut main);
    redraw(&mut vcx);
    let shown: Vec<String> = vcx.update(|_, cx| {
        view.read(cx)
            .nvme_rows()
            .iter()
            .map(|(l, _)| l.to_string())
            .collect()
    });
    assert_eq!(shown, labels);
    assert!(vcx.debug_bounds("SmartNvmeRow-0").is_some());
}

#[gpui_kit::test]
fn smart_failure_text(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    let mut failed = data(0, vec![], vec![]);
    failed.health = Total {
        value: Arc::from("Operation not permitted"),
        validity: Validity::Failed,
    };
    publish(&mut main, &engine, Some((Validity::Failed, failed)));
    let (mut vcx, view) = open(&mut main);
    redraw(&mut vcx);
    assert!(vcx.debug_bounds("SmartFailure").is_some());
    let reason = vcx.update(|_, cx| view.read(cx).error().cloned());
    assert_eq!(reason.as_deref(), Some("Operation not permitted"));
}

#[gpui_kit::test]
fn smart_refresh_sends_command(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish(
        &mut main,
        &engine,
        Some((
            Validity::Valid,
            data(1_000, vec![attr(1, "A", 1, "OK")], vec![]),
        )),
    );
    let (mut vcx, view) = open(&mut main);
    assert_eq!(smart_reads(&engine), Vec::<bool>::new());
    click(&mut vcx, "SmartRefresh");
    redraw(&mut vcx);
    assert_eq!(smart_reads(&engine), vec![true]);
    assert!(vcx.update(|_, cx| view.read(cx).refreshing()));
    // Disabled while the read runs.
    click(&mut vcx, "SmartRefresh");
    assert_eq!(smart_reads(&engine), vec![true]);
    // A new update time re-enables it.
    publish(
        &mut main,
        &engine,
        Some((
            Validity::Valid,
            data(1_060, vec![attr(1, "A", 1, "OK")], vec![]),
        )),
    );
    redraw(&mut vcx);
    assert!(!vcx.update(|_, cx| view.read(cx).refreshing()));
}

#[gpui_kit::test]
fn smart_refresh_busy_toasts(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish(
        &mut main,
        &engine,
        Some((
            Validity::Valid,
            data(1_000, vec![attr(1, "A", 1, "OK")], vec![]),
        )),
    );
    let (mut vcx, view) = open(&mut main);
    engine.set_busy(true);
    click(&mut vcx, "SmartRefresh");
    main.run_until_parked();
    assert!(!vcx.update(|_, cx| view.read(cx).refreshing()));
    let toasts: Vec<String> = main.update(|_, cx| {
        let v = main_window_view(cx).expect("main window");
        let texts = v.read(cx).toasts().read(cx).visible_texts();
        texts.iter().map(|t| t.to_string()).collect()
    });
    assert!(
        toasts.iter().any(|t| t == "Could not refresh SMART data"),
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn smart_table_sorts(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    let ata = vec![
        attr(1, "A", 100, "OK"),
        attr(5, "B", 50, "OK"),
        attr(9, "C", 75, "OK"),
    ];
    publish(
        &mut main,
        &engine,
        Some((Validity::Valid, data(1_000, ata, vec![]))),
    );
    let (mut vcx, view) = open(&mut main);
    let table = vcx.update(|_, cx| view.read(cx).table().clone());
    let ids = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| table.update(cx, |t, _| t.delegate_mut().ids()))
    };
    assert_eq!(ids(&mut vcx), vec![1, 5, 9]);
    // The header click calls `on_sort`: descending first, then ascending.
    let sort = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            table.update(cx, |t, cx| {
                use tl_ui::table::tree_table::TreeTableDelegate as _;
                t.delegate_mut().on_sort(SmartColumn::Normalized, cx)
            })
        })
    };
    sort(&mut vcx);
    assert_eq!(ids(&mut vcx), vec![1, 9, 5]);
    sort(&mut vcx);
    assert_eq!(ids(&mut vcx), vec![5, 9, 1]);
}
