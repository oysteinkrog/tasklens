//! The Inventory file sections on the owner machine (E12-016).
//!
//! Run with `heavy-build cargo test -p tl-engine --test real_inventory -- --ignored`. It only
//! reads files: no dmidecode, sudo or pkexec. The machine ID and the board serial are never
//! printed.
//!
//! `inventory_hostnamed_real` reads hostnamed's properties on the system bus. It changes no
//! state.
//!
//! `p2_real_host_once` (E12-A3) starts the whole engine on the host roots with the P2 device
//! demand and prints the P2 workers' health table only, never names, serials or addresses.

use std::path::PathBuf;
use std::time::Duration;

use asupersync::runtime::RuntimeBuilder;
use tl_collect::SysRoot;
use tl_collect::inventory::InventoryPaths;
use tl_engine::workers::inventory::{SystemEnv, read_system_bus, scan_files};
use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
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

/// UDisks2 on the owner machine: two NVMe drives and one SATA drive. Read-only: it calls
/// `GetManagedObjects` only. Failures show names and SMART kinds, never serials or WWNs.
#[test]
#[ignore = "needs the owner machine's system bus with UDisks2"]
fn udisks_real_drives() {
    use std::time::Duration;

    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::RuntimeBuilder;
    use asupersync::{Cx, time};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::source::{WorkerControl, WorkerIo};
    use tl_engine::update::{DeviceDetailsUpdate, EngineMsg, SourceUpdate};
    use tl_engine::workers::dbus_udisks;
    use tl_model::demand::Demand;
    use tl_model::devices::DeviceIdentity;
    use tl_model::health::SourceId;
    use tl_model::settings::EngineSettings;
    use tl_model::snapshot::SmartKind;

    let rt = RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds");
    let join = rt.handle().spawn_checked(async {
        let cx = Cx::current().expect("task has a Cx");
        let (updates, mut rx) = mpsc::channel::<EngineMsg>(64);
        let demand = Demand {
            disks: true,
            ..Demand::default()
        };
        let (_demand_tx, demand) = watch::channel(demand);
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let mut worker = cx
            .spawn(move |cx| async move {
                let _ = dbus_udisks::run(cx, io).await;
            })
            .expect("worker spawns");
        let entries = loop {
            let msg = time::timeout(cx.now(), Duration::from_secs(10), rx.recv(&cx))
                .await
                .expect("a message within 10 s")
                .expect("channel open");
            if let EngineMsg::Source(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
                from: SourceId::Udisks,
                entries,
            })) = msg
            {
                break entries;
            }
        };
        worker.abort();
        let _ = worker.join(&cx).await;
        entries
    });
    let entries = rt.block_on(join).expect("task completes");
    let smart = |want: &str| {
        entries.iter().find_map(|(identity, d)| match identity {
            DeviceIdentity::Disk { name, .. } if &**name == want => Some(d.smart),
            _ => None,
        })
    };
    let kinds: Vec<(String, SmartKind)> = entries
        .iter()
        .filter_map(|(identity, d)| match identity {
            DeviceIdentity::Disk { name, .. } => Some((name.to_string(), d.smart)),
            _ => None,
        })
        .collect();
    assert_eq!(smart("sda"), Some(SmartKind::Ata), "{kinds:?}");
    for nvme in ["nvme0n1", "nvme1n1"] {
        assert!(
            matches!(
                smart(nvme),
                Some(SmartKind::Nvme | SmartKind::NeedsFallback)
            ),
            "{nvme}: {kinds:?}"
        );
    }
}

/// A SMART read of `sda` on the owner machine. Read-only: `update` is false, so no
/// `SmartUpdate` runs and a sleeping disk stays asleep. Failures show validity and texts the
/// worker made, never serials or WWNs.
#[test]
#[ignore = "needs the owner machine's system bus with UDisks2 and a SATA sda"]
fn udisks_smart_real() {
    use std::sync::Arc;
    use std::time::Duration;

    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::RuntimeBuilder;
    use asupersync::{Cx, time};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::source::{QueuedCommand, WorkerCommand, WorkerControl, WorkerIo};
    use tl_engine::update::{DeviceDetailsUpdate, EngineMsg, SourceUpdate};
    use tl_engine::workers::dbus_udisks;
    use tl_model::command::DeviceCommand;
    use tl_model::demand::Demand;
    use tl_model::devices::DeviceIdentity;
    use tl_model::health::SourceId;
    use tl_model::settings::EngineSettings;

    let rt = RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds");
    let join = rt.handle().spawn_checked(async {
        let cx = Cx::current().expect("task has a Cx");
        let (updates, mut rx) = mpsc::channel::<EngineMsg>(64);
        let (_demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let control = WorkerControl {
            commands: Arc::from([QueuedCommand {
                seq: 1,
                target: SourceId::Udisks,
                command: WorkerCommand::Device(DeviceCommand::SmartRead {
                    disk: "sda".into(),
                    update: false,
                }),
            }]),
            ..WorkerControl::default()
        };
        let (_control_tx, control) = watch::channel(control);
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let mut worker = cx
            .spawn(move |cx| async move {
                let _ = dbus_udisks::run(cx, io).await;
            })
            .expect("worker spawns");
        let smart = loop {
            let msg = time::timeout(cx.now(), Duration::from_secs(15), rx.recv(&cx))
                .await
                .expect("a message within 15 s")
                .expect("channel open");
            let EngineMsg::Source(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
                from: SourceId::Udisks,
                entries,
            })) = msg
            else {
                continue;
            };
            let sda = entries.iter().find_map(|(identity, d)| match identity {
                DeviceIdentity::Disk { name, .. } if &**name == "sda" => d.smart_data.clone(),
                _ => None,
            });
            match sda {
                Some(s) if s.validity != tl_model::validity::Validity::WarmingUp => break s,
                _ => {}
            }
        };
        worker.abort();
        let _ = worker.join(&cx).await;
        smart
    });
    let smart = rt.block_on(join).expect("task completes");
    assert_eq!(
        smart.validity,
        Validity::Valid,
        "{}",
        smart.value.health.value
    );
    assert!(
        smart.value.ata.iter().any(|a| a.id == 9),
        "no power-on hours among {} attributes",
        smart.value.ata.len()
    );
}

/// NetworkManager on the owner machine: a wired interface with an active connection. Read-only:
/// it lists devices and reads properties, and never changes a connection. Failures show
/// interface names only, never connection names or SSIDs.
#[test]
#[ignore = "needs the owner machine's system bus with NetworkManager"]
fn nm_real_devices() {
    use std::time::Duration;

    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::RuntimeBuilder;
    use asupersync::{Cx, time};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::source::{WorkerControl, WorkerIo};
    use tl_engine::update::{DeviceDetailsUpdate, EngineMsg, SourceUpdate};
    use tl_engine::workers::dbus_nm;
    use tl_model::demand::Demand;
    use tl_model::devices::{DeviceIdentity, NetKind};
    use tl_model::health::SourceId;
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
            inventory: true,
            ..Demand::default()
        };
        let (_demand_tx, demand) = watch::channel(demand);
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let mut worker = cx
            .spawn(move |cx| async move {
                let _ = dbus_nm::run(cx, io, SysRoot::host()).await;
            })
            .expect("worker spawns");
        let entries = loop {
            let msg = time::timeout(cx.now(), Duration::from_secs(10), rx.recv(&cx))
                .await
                .expect("a message within 10 s")
                .expect("channel open");
            if let EngineMsg::Source(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
                from: SourceId::NetworkManager,
                entries,
            })) = msg
            {
                break entries;
            }
        };
        worker.abort();
        let _ = worker.join(&cx).await;
        entries
    });
    let entries = rt.block_on(join).expect("task completes");
    let names: Vec<String> = entries
        .iter()
        .map(|(identity, d)| match identity {
            DeviceIdentity::Net { name, .. } => format!("{name} {:?}", d.kind),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(
        entries
            .iter()
            .any(|(_, d)| d.kind == NetKind::Wired && d.connection.is_some()),
        "no wired interface with a connection among {names:?}"
    );
    assert!(
        entries.iter().all(|(identity, _)| matches!(
            identity,
            DeviceIdentity::Net { name, .. } if &**name != "lo"
        )),
        "{names:?}"
    );
}

/// The PCI slots of the host's AMD cards, from `/sys/class/drm/card*/device`.
fn amd_slots() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut slots = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        let vendor = std::fs::read_to_string(device.join("vendor")).unwrap_or_default();
        if vendor.trim() != "0x1002" {
            continue;
        }
        let uevent = std::fs::read_to_string(device.join("uevent")).unwrap_or_default();
        if let Some(slot) = uevent
            .lines()
            .find_map(|l| l.strip_prefix("PCI_SLOT_NAME="))
        {
            slots.push(slot.to_owned());
        }
    }
    slots
}

#[test]
#[ignore = "owner machine: starts the engine on the live system and system bus"]
fn p2_real_host_once() {
    let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("real_inventory-state");
    let config = EngineConfig::new(EngineMode::Full, state_dir);
    let engine = Engine::start(config, EngineHooks::default()).expect("engine starts");
    engine.set_demand(Demand {
        disks: true,
        nets: true,
        gpus: true,
        inventory: true,
        any_surface_visible: true,
        ..Demand::default()
    });
    std::thread::sleep(Duration::from_secs(5));
    let snap = engine.latest();

    eprintln!("p2_real_host_once health:");
    for id in [
        SourceId::Inventory,
        SourceId::Udisks,
        SourceId::UPower,
        SourceId::NetworkManager,
        SourceId::GpuDrm,
    ] {
        let h = &snap.health.sources[id.index()];
        eprintln!(
            "  {id:?}: {:?}{}",
            h.state,
            if h.last_error.is_some() {
                " (error)"
            } else {
                ""
            }
        );
    }

    let inv = &snap.inventory;
    assert_eq!(inv.system.static_hostname.validity, Validity::Valid);
    assert_eq!(inv.processor.name.validity, Validity::Valid);
    let details = &snap.device_details;
    assert!(
        details
            .disks
            .iter()
            .any(|(_, d)| d.model.validity == Validity::Valid),
        "no disk with a model among {}",
        details.disks.len()
    );
    assert!(
        details
            .nets
            .iter()
            .any(|(_, n)| n.mac.validity == Validity::Valid),
        "no interface with a MAC among {}",
        details.nets.len()
    );

    let health = |id: SourceId| snap.health.sources[id.index()].state;
    for id in [
        SourceId::Inventory,
        SourceId::Udisks,
        SourceId::NetworkManager,
        SourceId::UPower,
    ] {
        assert_eq!(health(id), SourceState::Running, "{id:?}");
    }
    let slots = amd_slots();
    if slots.is_empty() {
        eprintln!("  no AMD card: GpuDrm util not checked");
    } else {
        assert_eq!(health(SourceId::GpuDrm), SourceState::Running);
        let amd = snap.system.gpus.iter().find(|(idx, _)| {
            snap.devices.get(*idx).is_some_and(|e| {
                slots
                    .iter()
                    .any(|s| e.persistent_key.strip_prefix("gpu:") == Some(s.as_str()))
            })
        });
        let (_, totals) = amd.expect("the AMD card has GPU totals");
        assert_eq!(totals.util_pct.validity, Validity::Valid);
    }

    let report = engine.shutdown(Duration::from_millis(1500));
    assert!(report.clean(), "{report:?}");
}
