//! The Inventory file sections on the owner machine (E12-016).
//!
//! Run with `heavy-build cargo test -p tl-engine --test real_inventory -- --ignored`. It only
//! reads files: no dmidecode, sudo or pkexec. The machine ID and the board serial are never
//! printed.

use tl_collect::SysRoot;
use tl_collect::inventory::InventoryPaths;
use tl_engine::workers::inventory::scan_files;
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
