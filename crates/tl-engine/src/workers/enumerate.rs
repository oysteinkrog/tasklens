//! Device enumeration for disks, network interfaces and hwmon sensors and fans (ADR-008).
//!
//! The `Disk`, `Net` and `Sensors` workers rescan every 5 s while their devices are demanded or
//! history records (R-052, PERF-005). There is no uevent socket in v1. These helpers are plain
//! blocking code: the workers call them inside their own blocking job and send the lists to the
//! reducer's catalog. A device that leaves and comes back with the same identity gets the next
//! generation, so counters are never subtracted across a replug (ADR-003).

use std::hash::Hash;
use std::path::Path;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use tl_collect::parse::block::{BlockChild, BlockDevice};
use tl_collect::parse::hwmon::{FanInput, GPU_HWMON_NAMES, HwmonChip, TempInput};
use tl_collect::parse::netif::NetIface;
use tl_collect::parse::udev::{UdevProps, block_entry, read_udev_props};
use tl_collect::sysroot::read_trimmed;
use tl_model::devices::DeviceIdentity;
use tl_model::time::TimeNs;

use crate::update::{DeviceDesc, DeviceKinds, DeviceList};

pub const ENUM_PERIOD_NS: u64 = 5_000_000_000;

/// The udev keys the disk persistent key needs (PERF-005, R-140 item 3).
const DISK_KEY_UDEV: [&str; 3] = ["ID_WWN", "ID_SERIAL", "ID_PATH"];

/// Decides when the next enumeration is due. The first call is always due.
#[derive(Default)]
pub struct EnumClock {
    last_ns: Option<u64>,
}

impl EnumClock {
    /// True when no enumeration ran yet or the last one is at least `ENUM_PERIOD_NS` old.
    /// Records `now_boot_ns` as the last enumeration when it returns true.
    pub fn due(&mut self, now_boot_ns: u64) -> bool {
        let due = match self.last_ns {
            None => true,
            Some(last) => now_boot_ns.saturating_sub(last) >= ENUM_PERIOD_NS,
        };
        if due {
            self.last_ns = Some(now_boot_ns);
        }
        due
    }

    /// The next `due` returns true (RefreshNow, `reset_baselines`).
    pub fn force(&mut self) {
        self.last_ns = None;
    }
}

/// Per-identity generation counter, kept by the worker for its own samplers.
/// Same rule as the reducer catalog: present, then absent, then present bumps the generation
/// by 1.
pub struct GenTracker<K: Eq + Hash + Clone> {
    /// Generation, and whether the key was in the last list.
    gens: FxHashMap<K, (u32, bool)>,
}

impl<K: Eq + Hash + Clone> Default for GenTracker<K> {
    fn default() -> Self {
        Self {
            gens: FxHashMap::default(),
        }
    }
}

impl<K: Eq + Hash + Clone> GenTracker<K> {
    /// Marks exactly `present` as present; returns the generation of each, in input order.
    pub fn update(&mut self, present: &[K]) -> Vec<u32> {
        let out = present
            .iter()
            .map(|key| match self.gens.get_mut(key) {
                Some((generation, seen)) => {
                    // Back after at least one list without it: a new generation.
                    if !*seen {
                        *generation = generation.wrapping_add(1);
                        *seen = true;
                    }
                    *generation
                }
                None => {
                    self.gens.insert(key.clone(), (0, true));
                    0
                }
            })
            .collect();
        let now: FxHashSet<&K> = present.iter().collect();
        for (key, (_, seen)) in &mut self.gens {
            if !now.contains(key) {
                *seen = false;
            }
        }
        out
    }
}

pub fn disk_identity(d: &BlockDevice) -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: d.major,
        minor: d.minor,
        name: d.name.clone(),
        wwn_or_serial: None,
    }
}

pub fn disk_child_identity(c: &BlockChild) -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: c.major,
        minor: c.minor,
        name: c.name.clone(),
        wwn_or_serial: None,
    }
}

pub fn net_identity(i: &NetIface) -> DeviceIdentity {
    DeviceIdentity::Net {
        netns_ino: i.netns_ino,
        ifindex: i.ifindex,
        name: i.name.clone(),
    }
}

pub fn sensor_identity(chip: &HwmonChip, t: &TempInput) -> DeviceIdentity {
    DeviceIdentity::Sensor {
        chip: chip.name.clone(),
        label: temp_label(t),
    }
}

pub fn fan_identity(chip: &HwmonChip, f: &FanInput) -> DeviceIdentity {
    DeviceIdentity::Fan {
        chip: chip.name.clone(),
        index: f.n,
    }
}

/// The label, or `temp<n>` when the chip gives none.
fn temp_label(t: &TempInput) -> Arc<str> {
    match &t.label {
        Some(label) => label.clone(),
        None => format!("temp{}", t.n).into(),
    }
}

/// PERF-005 persistent key (R-155 item 7): `disk:<ID_WWN>`, else `disk:<ID_SERIAL>`, else
/// `disk:path:<ID_PATH>`, else `disk:<kernel name>`. Children use the same rule on their own
/// udev entry.
pub fn disk_key(name: &str, udev: &UdevProps) -> Arc<str> {
    if let Some(wwn) = udev.get("ID_WWN").filter(|v| !v.is_empty()) {
        format!("disk:{wwn}").into()
    } else if let Some(serial) = udev.get("ID_SERIAL").filter(|v| !v.is_empty()) {
        format!("disk:{serial}").into()
    } else if let Some(path) = udev.get("ID_PATH").filter(|v| !v.is_empty()) {
        format!("disk:path:{path}").into()
    } else {
        format!("disk:{name}").into()
    }
}

/// `net:<MAC>` when `mac` (sysfs `address`) is present and not all zero, else
/// `net:<interface name>`. `NetIface` has no address field, so the caller passes the MAC
/// (leader decision, hjn.5).
pub fn net_key(i: &NetIface, mac: Option<&str>) -> Arc<str> {
    match mac.filter(|m| m.bytes().any(|b| b != b'0' && b != b':')) {
        Some(mac) => format!("net:{mac}").into(),
        None => format!("net:{}", i.name).into(),
    }
}

/// `fan:<hwmon name>:<device path>:<n>`. The device path is empty when the chip has no
/// `device` link.
pub fn fan_key(chip: &HwmonChip, f: &FanInput) -> Arc<str> {
    let path = chip.device_path.as_deref().unwrap_or("");
    format!("fan:{}:{path}:{}", chip.name, f.n).into()
}

/// `sensor:<hwmon name>:<device path>:<label>` for temperatures. PERF-005 names no key for
/// temperatures; this one only fills the mandatory field with a stable value.
pub fn sensor_key(chip: &HwmonChip, t: &TempInput) -> Arc<str> {
    let path = chip.device_path.as_deref().unwrap_or("");
    format!("sensor:{}:{path}:{}", chip.name, temp_label(t)).into()
}

/// The disk key for one block device, read from `<udev_root>/udev/data/b<major>:<minor>`.
/// A file that cannot be read gives the kernel-name fallback.
fn disk_key_from_udev(udev_root: &Path, name: &str, major: u32, minor: u32) -> Arc<str> {
    let mut props = UdevProps::default();
    let path = udev_root.join("udev/data").join(block_entry(major, minor));
    if read_udev_props(&path, &DISK_KEY_UDEV, &mut props).is_err() {
        props.clear();
    }
    disk_key(name, &props)
}

/// Full DISKS list: every disk, then its children with `parent = Some(disk identity)`.
/// `udev_root` is `SysRoot.run`.
pub fn disk_list(t_ns: TimeNs, disks: &[BlockDevice], udev_root: &Path) -> DeviceList {
    let mut entries = Vec::new();
    for disk in disks {
        let identity = disk_identity(disk);
        entries.push(DeviceDesc {
            identity: identity.clone(),
            display_name: disk.name.clone(),
            parent: None,
            persistent_key: disk_key_from_udev(udev_root, &disk.name, disk.major, disk.minor),
            net_kind: None,
        });
        for child in &disk.children {
            entries.push(DeviceDesc {
                identity: disk_child_identity(child),
                display_name: child.name.clone(),
                parent: Some(identity.clone()),
                persistent_key: disk_key_from_udev(
                    udev_root,
                    &child.name,
                    child.major,
                    child.minor,
                ),
                net_kind: None,
            });
        }
    }
    DeviceList {
        t_ns,
        kinds: DeviceKinds::DISKS,
        entries,
    }
}

/// Full NETS list: every interface `netif::enumerate` returned (it already leaves out `lo`).
/// The MAC for the key comes from `<sys_root>/class/net/<name>/address`; `sys_root` is
/// `SysRoot.sys`. The `sys_root` argument is a leader decision (hjn.5), the same shape as
/// `disk_list`'s `udev_root`.
pub fn net_list(t_ns: TimeNs, ifaces: &[NetIface], sys_root: &Path) -> DeviceList {
    let mut buf = Vec::new();
    let entries = ifaces
        .iter()
        .map(|iface| {
            let address = sys_root
                .join("class/net")
                .join(&*iface.name)
                .join("address");
            let mac = read_trimmed(&address, &mut buf)
                .ok()
                .and_then(|bytes| std::str::from_utf8(bytes).ok());
            DeviceDesc {
                identity: net_identity(iface),
                display_name: iface.name.clone(),
                parent: None,
                persistent_key: net_key(iface, mac),
                net_kind: None,
            }
        })
        .collect();
    DeviceList {
        t_ns,
        kinds: DeviceKinds::NETS,
        entries,
    }
}

/// Full SENSORS | FANS list from the chips, skipping chips named in `GPU_HWMON_NAMES`
/// (their readings belong to the GPU page, PERF-068).
pub fn sensor_list(t_ns: TimeNs, chips: &[HwmonChip]) -> DeviceList {
    let mut entries = Vec::new();
    for chip in chips {
        if GPU_HWMON_NAMES.contains(&&*chip.name) {
            continue;
        }
        for temp in &chip.temps {
            entries.push(DeviceDesc {
                identity: sensor_identity(chip, temp),
                display_name: format!("{} {}", chip.name, temp_label(temp)).into(),
                parent: None,
                persistent_key: sensor_key(chip, temp),
                net_kind: None,
            });
        }
        for fan in &chip.fans {
            let display_name = match &fan.label {
                Some(label) => format!("{} {label}", chip.name),
                None => format!("{} fan{}", chip.name, fan.n),
            };
            entries.push(DeviceDesc {
                identity: fan_identity(chip, fan),
                display_name: display_name.into(),
                parent: None,
                persistent_key: fan_key(chip, fan),
                net_kind: None,
            });
        }
    }
    DeviceList {
        t_ns,
        kinds: DeviceKinds::SENSORS | DeviceKinds::FANS,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str) -> NetIface {
        NetIface {
            name: name.into(),
            ifindex: 5,
            netns_ino: 4026531840,
            kind: tl_collect::parse::netif::IfaceKind::Other,
            arphrd_type: 1,
        }
    }

    fn udev(pairs: &[(&str, &str)]) -> UdevProps {
        UdevProps(
            pairs
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        )
    }

    #[test]
    fn enum_clock_every_5s() {
        let mut clock = EnumClock::default();
        assert!(clock.due(0));
        assert!(!clock.due(4_999_999_999));
        assert!(clock.due(5_000_000_000));
    }

    #[test]
    fn enum_clock_force() {
        let mut clock = EnumClock::default();
        assert!(clock.due(1_000));
        clock.force();
        assert!(clock.due(1_001));
        assert!(!clock.due(1_002));
    }

    #[test]
    fn gen_tracker_replug_bumps() {
        let mut gens = GenTracker::default();
        assert_eq!(gens.update(&["sda"]), [0]);
        assert_eq!(gens.update(&[]), Vec::<u32>::new());
        assert_eq!(gens.update(&["sda"]), [1]);
        // Absent for several lists still bumps once.
        gens.update(&[]);
        gens.update(&[]);
        assert_eq!(gens.update(&["sda"]), [2]);
    }

    #[test]
    fn gen_tracker_stable_while_present() {
        let mut gens = GenTracker::default();
        assert_eq!(gens.update(&["sda", "sdb"]), [0, 0]);
        assert_eq!(gens.update(&["sdb"]), [0]);
        assert_eq!(gens.update(&["sdb", "sda"]), [0, 1]);
        assert_eq!(gens.update(&["sda", "sdb"]), [1, 0]);
    }

    #[test]
    fn disk_key_order() {
        let both = udev(&[
            ("ID_SERIAL", "Samsung_SSD_970"),
            ("ID_WWN", "0x5002538e40a1b2c3"),
        ]);
        assert_eq!(&*disk_key("nvme0n1", &both), "disk:0x5002538e40a1b2c3");
        let serial = udev(&[("ID_SERIAL", "Samsung_SSD_970")]);
        assert_eq!(&*disk_key("nvme0n1", &serial), "disk:Samsung_SSD_970");
        let path = udev(&[("ID_PATH", "pci-0000:01:00.0-nvme-1")]);
        assert_eq!(
            &*disk_key("nvme0n1", &path),
            "disk:path:pci-0000:01:00.0-nvme-1"
        );
        assert_eq!(&*disk_key("nvme0n1", &UdevProps::default()), "disk:nvme0n1");
    }

    #[test]
    fn net_key_mac_or_name() {
        assert_eq!(
            &*net_key(&iface("wg0"), Some("00:00:00:00:00:00")),
            "net:wg0"
        );
        assert_eq!(&*net_key(&iface("wg0"), None), "net:wg0");
        assert_eq!(
            &*net_key(&iface("eth0"), Some("a8:a1:59:12:34:56")),
            "net:a8:a1:59:12:34:56"
        );
    }
}
