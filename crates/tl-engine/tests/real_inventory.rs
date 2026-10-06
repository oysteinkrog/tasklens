//! The Inventory file sections on the owner machine (E12-016).
//!
//! Run with `heavy-build cargo test -p tl-engine --test real_inventory -- --ignored`. It only
//! reads files: no dmidecode, sudo or pkexec. The machine ID and the board serial are never
//! printed.
//!
//! `inventory_hostnamed_real` reads hostnamed's properties on the system bus. It changes no
//! state.

use std::time::Duration;

use asupersync::runtime::RuntimeBuilder;
use tl_collect::SysRoot;
use tl_collect::inventory::InventoryPaths;
use tl_engine::workers::inventory::{SystemEnv, read_system_bus, scan_files};
use tl_model::validity::Validity;

#[test]
#[ignore = "reads the owner machine's hardware"]
fn inventory_real_host() {
    let snap = scan_files(&InventoryPaths::system(), &SysRoot::host());
    assert_eq!(snap.os.name.validity, Validity::Valid);
    assert_eq!(snap.processor.cores.validity, Validity::Valid);
    assert!(snap.processor.cores.value > 0);
    assert!(
        snap.modules
            .iter()
            .any(|m| m.kind.validity == Validity::Valid && &*m.kind.value == "DDR4"),
        "no DDR4 module among {} modules",
        snap.modules.len()
    );
    assert_eq!(snap.board.serial.validity, Validity::Restricted);
}

#[test]
#[ignore = "reads hostnamed on the owner machine's system bus"]
fn inventory_hostnamed_real() {
    let rt = RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds");
    let (tx, rx) = std::sync::mpsc::channel();
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let _ = tx.send(read_system_bus(&cx, &SystemEnv::from_process()).await);
        })
        .expect("task spawns");
    let bus = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("read_system_bus finished");
    let hostname = std::fs::read_to_string("/etc/hostname").expect("read /etc/hostname");
    assert_eq!(bus.static_name.validity, Validity::Valid);
    // Compared without printing the value.
    assert!(
        &*bus.static_name.value == hostname.trim(),
        "static hostname differs"
    );
}

/// UPower on the owner machine, a desktop with no system battery (a mouse battery is skipped).
/// Read-only: it lists devices and reads their properties.
#[test]
#[ignore = "needs the owner machine's system bus with UPower"]
fn upower_real_no_battery() {
    use std::path::PathBuf;
    use std::time::Duration;

    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::RuntimeBuilder;
    use asupersync::{Cx, time};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::source::{WorkerControl, WorkerIo};
    use tl_engine::update::{DeviceDetailsUpdate, DeviceKinds, EngineMsg, SourceUpdate};
    use tl_engine::workers::dbus_upower;
    use tl_model::demand::Demand;
    use tl_model::health::{SourceId, SourceState};
    use tl_model::settings::EngineSettings;

    let rt = RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds");
    let join = rt.handle().spawn_checked(async {
        let cx = Cx::current().expect("task has a Cx");
        let (updates, mut rx) = mpsc::channel::<EngineMsg>(64);
        let demand = Demand {
            any_surface_visible: true,
            ..Demand::default()
        };
        let (_demand_tx, demand) = watch::channel(demand);
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let mut worker = cx
            .spawn(move |cx| async move {
                let _ = dbus_upower::run(cx, io, PathBuf::from("/sys")).await;
            })
            .expect("worker spawns");
        let mut got = Vec::new();
        while got.len() < 3 {
            let msg = time::timeout(cx.now(), Duration::from_secs(10), rx.recv(&cx))
                .await
                .expect("a message within 10 s")
                .expect("channel open");
            if let EngineMsg::Source(update) = msg {
                got.push(update);
            }
        }
        worker.abort();
        let _ = worker.join(&cx).await;
        got
    });
    let got = rt.block_on(join).expect("task completes");
    assert!(
        matches!(
            &got[0],
            SourceUpdate::Health {
                id: SourceId::UPower,
                state: SourceState::Running,
                last_error: None,
                ..
            }
        ),
        "{:?}",
        got[0]
    );
    assert!(
        matches!(&got[1], SourceUpdate::Devices(list)
            if list.kinds == DeviceKinds::BATTERIES && list.entries.is_empty()),
        "{:?}",
        got[1]
    );
    assert!(
        matches!(&got[2], SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Batteries(b))
            if b.is_empty()),
        "{:?}",
        got[2]
    );
}
