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
