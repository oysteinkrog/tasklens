//! DemandAggregator integration tests with `FakeEngine` and `TestAppContext` (E16-004, ADR-005,
//! ADR-008, ADR-013). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, EntityId, IntoElement, Render, TestAppContext,
    Window, div,
};
use tl_model::client::EngineClient;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator};
use tl_ui::hub::SnapshotHub;
use tl_ui::testing::fake_engine::FakeEngine;

struct Rig {
    engine: Arc<FakeEngine>,
    hub: Entity<SnapshotHub>,
    agg: Entity<DemandAggregator>,
}

fn rig(cx: &mut TestAppContext) -> Rig {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let agg = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    Rig { engine, hub, agg }
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn window(cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.add_window(|_, _| Empty).into()
}

fn source(n: u64) -> EntityId {
    EntityId::from(n)
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 1000 + pid as u64,
        boot_id: BootId(42),
        pid,
    }
}

fn processes() -> Demand {
    Demand {
        any_surface_visible: true,
        process_table: true,
        detail_keys: vec![key(1), key(2)].into(),
        ..Demand::default()
    }
}

fn performance(cpu: CpuDemand) -> Demand {
    Demand {
        any_surface_visible: true,
        cpu,
        ..Demand::default()
    }
}

fn set(rig: &Rig, cx: &mut TestAppContext, src: EntityId, win: Option<AnyWindowHandle>, d: Demand) {
    rig.agg.update(cx, |a, cx| a.set(src, win, d, cx));
}

fn wait_flush(cx: &mut TestAppContext) {
    cx.executor().advance_clock(DEMAND_COALESCE);
    cx.run_until_parked();
}

/// Makes the hub go hidden: a pending change with no render for more than 1000 ms.
fn go_hidden(rig: &Rig, cx: &mut TestAppContext) {
    rig.engine
        .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    assert!(rig.hub.read_with(cx, |h, _| h.is_hidden()));
}

#[gpui_kit::test]
fn coalesces_within_100ms(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    for i in 1..=5u32 {
        let d = Demand {
            detail_keys: (0..i).map(key).collect(),
            ..processes()
        };
        set(&rig, cx, source(1), Some(main), d);
        cx.executor().advance_clock(Duration::from_millis(15));
        cx.run_until_parked();
    }
    assert!(
        rig.engine.demands().is_empty(),
        "nothing sent before 100 ms"
    );
    wait_flush(cx);
    let sent = rig.engine.demands();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        &sent[0].detail_keys[..],
        &(0..5).map(key).collect::<Vec<_>>()[..]
    );
    assert_eq!(rig.agg.read_with(cx, |a, _| a.merged()), sent[0]);
}

#[gpui_kit::test]
fn not_resent_when_equal(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    set(&rig, cx, source(1), Some(main), processes());
    wait_flush(cx);
    assert_eq!(rig.engine.demands().len(), 1);

    set(&rig, cx, source(1), Some(main), processes());
    wait_flush(cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(rig.engine.demands().len(), 1);
}

#[gpui_kit::test]
fn tab_switch_changes_demand(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    set(&rig, cx, source(1), Some(main), processes());
    wait_flush(cx);

    rig.agg.update(cx, |a, cx| a.remove(source(1), cx));
    set(
        &rig,
        cx,
        source(2),
        Some(main),
        performance(CpuDemand::PerCore),
    );
    wait_flush(cx);
    let sent = rig.engine.demands();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].process_table);
    assert!(!sent[1].process_table);
    assert!(sent[1].detail_keys.is_empty());
    assert_eq!(sent[1].cpu, CpuDemand::PerCore);
    assert!(sent[1].any_surface_visible);
}

#[gpui_kit::test]
fn merges_processes_and_performance(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    set(&rig, cx, source(1), Some(main), processes());
    set(
        &rig,
        cx,
        source(2),
        Some(main),
        performance(CpuDemand::PerCore),
    );
    wait_flush(cx);
    let sent = rig.engine.demands();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].process_table);
    assert_eq!(sent[0].cpu, CpuDemand::PerCore);
    assert_eq!(&sent[0].detail_keys[..], &[key(1), key(2)]);
}

#[gpui_kit::test]
fn hidden_turns_demand_off(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    rig.agg.update(cx, |a, _| a.set_main_window(main));
    set(&rig, cx, source(1), Some(main), processes());
    set(
        &rig,
        cx,
        source(2),
        Some(main),
        performance(CpuDemand::Total),
    );
    wait_flush(cx);
    assert!(rig.engine.demands()[0].any_surface_visible);

    go_hidden(&rig, cx);
    wait_flush(cx);
    let sent = rig.engine.demands();
    assert_eq!(sent.len(), 2);
    assert!(!sent[1].any_surface_visible);
    assert_eq!(
        sent[1],
        Demand {
            device_history: true,
            ..Demand::default()
        },
        "every periodic source off, device history kept"
    );

    // Visible again: the main-window parts count again.
    rig.hub.update(cx, |h, cx| h.note_render(cx));
    wait_flush(cx);
    let sent = rig.engine.demands();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2], sent[0]);
}

#[gpui_kit::test]
fn detached_part_survives_hidden(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let main = window(cx);
    let detached = window(cx);
    rig.agg.update(cx, |a, _| a.set_main_window(main));
    set(&rig, cx, source(1), Some(main), processes());
    set(
        &rig,
        cx,
        source(2),
        Some(detached),
        performance(CpuDemand::Total),
    );
    go_hidden(&rig, cx);
    wait_flush(cx);
    let last = rig
        .engine
        .demands()
        .last()
        .cloned()
        .expect("a demand was sent");
    assert!(last.any_surface_visible);
    assert_eq!(last.cpu, CpuDemand::Total);
    assert!(!last.process_table, "main-window part ignored while hidden");
}

/// Save power on battery (wlf.32): the merged demand and the charts' [`PowerSaving`].
mod battery {
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::demand::power_saving;
    use tl_ui::settings_global::TlSettings;

    use super::*;

    /// A rig with settings, and a CPU page part that wants `cpu_fast`.
    fn battery_rig(cx: &mut TestAppContext) -> (Rig, AnyWindowHandle) {
        cx.update(|cx| {
            let client: Arc<dyn EngineClient> = FakeEngine::new();
            cx.set_global(EngineGlobal(client));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let rig = rig(cx);
        let main = window(cx);
        let part = Demand {
            cpu_fast: true,
            ..performance(CpuDemand::Total)
        };
        set(&rig, cx, source(1), Some(main), part);
        wait_flush(cx);
        (rig, main)
    }

    fn on_battery(rig: &Rig, cx: &mut TestAppContext, on: Option<bool>) {
        rig.engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.device_details);
            d.on_battery = on;
            d.header.version += 1;
        });
        cx.run_until_parked();
        wait_flush(cx);
    }

    fn cpu_fast_sent(rig: &Rig) -> bool {
        rig.engine
            .demands()
            .last()
            .expect("a demand was sent")
            .cpu_fast
    }

    #[gpui_kit::test]
    fn on_battery_turns_cpu_fast_and_sliding_off(cx: &mut TestAppContext) {
        let (rig, _) = battery_rig(cx);
        assert!(cpu_fast_sent(&rig));
        assert!(!cx.read(power_saving));

        on_battery(&rig, cx, Some(true));
        assert!(!cpu_fast_sent(&rig), "1 Hz on battery");
        assert!(cx.read(power_saving), "charts stop sliding");

        on_battery(&rig, cx, Some(false));
        assert!(cpu_fast_sent(&rig), "10 Hz again on mains");
        assert!(!cx.read(power_saving));
    }

    #[gpui_kit::test]
    fn setting_off_ignores_battery(cx: &mut TestAppContext) {
        let (rig, _) = battery_rig(cx);
        cx.update(|cx| {
            TlSettings::update(cx, |d| d.settings.motion.save_power_on_battery = false);
        });
        on_battery(&rig, cx, Some(true));
        assert!(cpu_fast_sent(&rig));
        assert!(!cx.read(power_saving));

        // Turning the setting on applies at once.
        cx.update(|cx| {
            TlSettings::update(cx, |d| d.settings.motion.save_power_on_battery = true);
        });
        cx.run_until_parked();
        wait_flush(cx);
        assert!(!cpu_fast_sent(&rig));
        assert!(cx.read(power_saving));
    }

    #[gpui_kit::test]
    fn no_battery_no_change(cx: &mut TestAppContext) {
        let (rig, _) = battery_rig(cx);
        let sent = rig.engine.demands().len();
        on_battery(&rig, cx, None);
        assert_eq!(rig.engine.demands().len(), sent, "nothing resent");
        assert!(cpu_fast_sent(&rig));
        assert!(!cx.read(power_saving));
    }
}
