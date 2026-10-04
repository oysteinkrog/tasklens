//! The device catalog with generations (ADR-003, ADR-008, PERF-005).
//!
//! Every `DeviceIdentity` gets a stable `DeviceIdx` for the session. A device that disappears
//! keeps its index with `present = false`. One that returns with the same identity keeps the
//! index and gets `generation + 1`, so counters are never subtracted across generations. The
//! CPU package, each logical CPU and the three aggregate devices are devices too (R-154).

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::system::CpuSet;

use crate::update::{DeviceKinds, DeviceList};

/// A broken engine invariant. The reducer returns it and the session restarts with a new
/// epoch (ADR-015, R-262).
#[derive(Debug)]
pub struct InvariantViolation(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceChange {
    Added,
    Removed,
    Returned,
}

pub struct Catalog {
    entries: Vec<DeviceEntry>,
    by_identity: FxHashMap<DeviceIdentity, DeviceIdx>,
    version: u64,
    dirty: bool,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

impl Catalog {
    /// Entry 0 is the CPU package, then the disk, network and GPU aggregates, all present.
    pub fn new() -> Self {
        let mut catalog = Catalog {
            entries: Vec::new(),
            by_identity: FxHashMap::default(),
            version: 0,
            dirty: false,
        };
        let fixed = [
            (DeviceIdentity::CpuPackage, "CPU", "cpu"),
            (
                DeviceIdentity::Aggregate(AggregateKind::Disks),
                "Disk",
                "aggregate:disk",
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Nets),
                "Network",
                "aggregate:net",
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Gpus),
                "GPU",
                "aggregate:gpu",
            ),
        ];
        for (identity, name, key) in fixed {
            // Four entries never overflow a u16 index.
            let _ = catalog.push(identity, Arc::from(name), None, Arc::from(key), None);
        }
        catalog
    }

    pub fn idx(&self, id: &DeviceIdentity) -> Option<DeviceIdx> {
        self.by_identity.get(id).copied()
    }

    /// Adds the device, or marks it present again. Returns `true` for a new device or a new
    /// generation. A known parent identity sets `parent`; an unknown one leaves it `None`.
    pub fn ensure(
        &mut self,
        id: &DeviceIdentity,
        display_name: &str,
        parent: Option<&DeviceIdentity>,
        persistent_key: &str,
        net_kind: Option<NetKind>,
    ) -> Result<(DeviceIdx, bool), InvariantViolation> {
        let parent = parent.and_then(|p| self.idx(p));
        let Some(idx) = self.idx(id) else {
            let idx = self.push(
                id.clone(),
                Arc::from(display_name),
                parent,
                Arc::from(persistent_key),
                net_kind,
            )?;
            return Ok((idx, true));
        };
        let entry = &mut self.entries[usize::from(idx.0)];
        let mut changed = false;
        let mut regenerated = false;
        if !entry.present {
            entry.present = true;
            entry.generation += 1;
            regenerated = true;
            changed = true;
        }
        if &*entry.display_name != display_name {
            entry.display_name = Arc::from(display_name);
            changed = true;
        }
        if &*entry.persistent_key != persistent_key {
            entry.persistent_key = Arc::from(persistent_key);
            changed = true;
        }
        if entry.parent != parent {
            entry.parent = parent;
            changed = true;
        }
        if entry.net_kind != net_kind {
            entry.net_kind = net_kind;
            changed = true;
        }
        if changed {
            self.touch();
        }
        Ok((idx, regenerated))
    }

    /// Apply a full list for `kinds`: new identities are added, missing ones become
    /// present=false, returning ones bump generation.
    pub fn apply_list(
        &mut self,
        list: &DeviceList,
    ) -> Result<Vec<(DeviceIdx, DeviceChange)>, InvariantViolation> {
        let mut changes = Vec::new();
        let mut seen = vec![false; self.entries.len() + list.entries.len()];
        // Parents are set in a second pass, so a partition listed before its disk still
        // links to it.
        for desc in &list.entries {
            let was_known = self.idx(&desc.identity).is_some();
            let (idx, fresh) = self.ensure(
                &desc.identity,
                &desc.display_name,
                None,
                &desc.persistent_key,
                desc.net_kind,
            )?;
            seen[usize::from(idx.0)] = true;
            if fresh {
                let change = if was_known {
                    DeviceChange::Returned
                } else {
                    DeviceChange::Added
                };
                changes.push((idx, change));
            }
        }
        for desc in &list.entries {
            let parent = desc.parent.as_ref().and_then(|p| self.idx(p));
            if let Some(idx) = self.idx(&desc.identity) {
                let entry = &mut self.entries[usize::from(idx.0)];
                if entry.parent != parent {
                    entry.parent = parent;
                    self.touch();
                }
            }
        }
        let mut removed = false;
        for (entry, &seen) in self.entries.iter_mut().zip(&seen) {
            if seen || !entry.present || !covered(list.kinds, &entry.identity) {
                continue;
            }
            entry.present = false;
            changes.push((entry.idx, DeviceChange::Removed));
            removed = true;
        }
        if removed {
            self.touch();
        }
        Ok(changes)
    }

    /// Syncs the `CpuLogical` devices with `online`; offline CPUs become present=false.
    /// Returns whether anything changed.
    pub fn set_cpus(&mut self, online: &CpuSet) -> Result<bool, InvariantViolation> {
        let before = self.version;
        for id in online.iter() {
            let identity = DeviceIdentity::CpuLogical { id };
            if self
                .idx(&identity)
                .is_some_and(|idx| self.entries[usize::from(idx.0)].present)
            {
                continue;
            }
            let name = format!("CPU {id}");
            let key = format!("cpu:{id}");
            self.ensure(&identity, &name, None, &key, None)?;
        }
        let mut offline = false;
        for entry in &mut self.entries {
            if let DeviceIdentity::CpuLogical { id } = entry.identity
                && entry.present
                && !online.contains(id)
            {
                entry.present = false;
                offline = true;
            }
        }
        if offline {
            self.touch();
        }
        Ok(self.version != before)
    }

    /// The generation of `idx`, or 0 for an unknown index.
    pub fn generation(&self, idx: DeviceIdx) -> u32 {
        self.entries
            .get(usize::from(idx.0))
            .map_or(0, |e| e.generation)
    }

    /// The catalog version. It rises with every change.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The entry of `idx`, or `None` for an unknown index.
    pub fn entry(&self, idx: DeviceIdx) -> Option<&DeviceEntry> {
        self.entries.get(usize::from(idx.0))
    }

    /// `Some` when the catalog changed since the last publish.
    pub fn publish(&mut self) -> Option<Arc<DeviceCatalog>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(Arc::new(DeviceCatalog {
            version: self.version,
            entries: self.entries.as_slice().into(),
        }))
    }

    fn push(
        &mut self,
        identity: DeviceIdentity,
        display_name: Arc<str>,
        parent: Option<DeviceIdx>,
        persistent_key: Arc<str>,
        net_kind: Option<NetKind>,
    ) -> Result<DeviceIdx, InvariantViolation> {
        let raw = u16::try_from(self.entries.len())
            .map_err(|_| InvariantViolation("device index overflow"))?;
        let idx = DeviceIdx(raw);
        self.by_identity.insert(identity.clone(), idx);
        self.entries.push(DeviceEntry {
            idx,
            identity,
            generation: 0,
            display_name,
            present: true,
            parent,
            persistent_key,
            net_kind,
        });
        self.touch();
        Ok(idx)
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.version += 1;
    }
}

/// Whether a list for `kinds` is complete for this identity.
fn covered(kinds: DeviceKinds, identity: &DeviceIdentity) -> bool {
    let kind = match identity {
        DeviceIdentity::Disk { .. } => DeviceKinds::DISKS,
        DeviceIdentity::Net { .. } => DeviceKinds::NETS,
        DeviceIdentity::Sensor { .. } => DeviceKinds::SENSORS,
        DeviceIdentity::Fan { .. } => DeviceKinds::FANS,
        DeviceIdentity::Gpu { .. } => DeviceKinds::GPUS,
        DeviceIdentity::Battery { .. } => DeviceKinds::BATTERIES,
        DeviceIdentity::CpuLogical { .. } => DeviceKinds::CPUS,
        DeviceIdentity::CpuPackage | DeviceIdentity::Memory | DeviceIdentity::Aggregate(_) => {
            return false;
        }
    };
    kinds.contains(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::DeviceDesc;

    fn disk(name: &str, minor: u32) -> DeviceIdentity {
        DeviceIdentity::Disk {
            major: 8,
            minor,
            name: Arc::from(name),
            wwn_or_serial: None,
        }
    }

    fn net(name: &str, ifindex: u32) -> DeviceIdentity {
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex,
            name: Arc::from(name),
        }
    }

    fn desc(identity: DeviceIdentity, key: &str) -> DeviceDesc {
        DeviceDesc {
            identity,
            display_name: Arc::from("device"),
            parent: None,
            persistent_key: Arc::from(key),
            net_kind: None,
        }
    }

    fn list(kinds: DeviceKinds, entries: Vec<DeviceDesc>) -> DeviceList {
        DeviceList {
            t_ns: 0,
            kinds,
            entries,
        }
    }

    fn cpus(range: std::ops::Range<u16>) -> CpuSet {
        let mut set = CpuSet::empty();
        for cpu in range {
            set.insert(cpu);
        }
        set
    }

    fn entry(c: &Catalog, id: &DeviceIdentity) -> DeviceEntry {
        let idx = c.idx(id).expect("device is in the catalog");
        c.entry(idx).expect("entry exists").clone()
    }

    #[test]
    fn returning_device_keeps_idx_and_bumps_generation() {
        let mut c = Catalog::new();
        let sda = disk("sda", 0);
        let with = list(DeviceKinds::DISKS, vec![desc(sda.clone(), "disk:sda")]);
        let without = list(DeviceKinds::DISKS, vec![]);

        let changes = c.apply_list(&with).unwrap();
        let idx = c.idx(&sda).unwrap();
        assert_eq!(changes, vec![(idx, DeviceChange::Added)]);
        assert!(entry(&c, &sda).present);
        assert_eq!(c.generation(idx), 0);

        let changes = c.apply_list(&without).unwrap();
        assert_eq!(changes, vec![(idx, DeviceChange::Removed)]);
        assert!(!entry(&c, &sda).present);
        assert_eq!(c.generation(idx), 0);

        let changes = c.apply_list(&with).unwrap();
        assert_eq!(changes, vec![(idx, DeviceChange::Returned)]);
        assert_eq!(c.idx(&sda), Some(idx));
        assert!(entry(&c, &sda).present);
        assert_eq!(c.generation(idx), 1);
    }

    #[test]
    fn list_scoped_by_kinds() {
        let mut c = Catalog::new();
        let eno1 = net("eno1", 2);
        c.apply_list(&list(
            DeviceKinds::NETS,
            vec![desc(eno1.clone(), "net:eno1")],
        ))
        .unwrap();
        let before = entry(&c, &eno1);

        let changes = c
            .apply_list(&list(
                DeviceKinds::DISKS,
                vec![desc(disk("sda", 0), "disk:sda")],
            ))
            .unwrap();
        assert_eq!(changes.len(), 1);
        let after = entry(&c, &eno1);
        assert!(after.present);
        assert_eq!(after.generation, before.generation);
        // The fixed devices are never removed by a list.
        let published = c.publish().unwrap();
        assert!(published.entries[..4].iter().all(|e| e.present));
    }

    #[test]
    fn cpu_hotplug_marks_missing() {
        let mut c = Catalog::new();
        assert!(c.set_cpus(&cpus(0..32)).unwrap());
        let first = c.publish().unwrap();
        let cpu31 = DeviceIdentity::CpuLogical { id: 31 };
        assert!(first.find(&cpu31).unwrap().present);

        assert!(c.set_cpus(&cpus(0..31)).unwrap());
        assert!(!entry(&c, &cpu31).present);
        let second = c.publish().unwrap();
        assert!(second.version > first.version);
        assert!(!second.find(&cpu31).unwrap().present);
        assert!(
            second
                .find(&DeviceIdentity::CpuLogical { id: 30 })
                .unwrap()
                .present
        );

        // The same set again changes nothing.
        assert!(!c.set_cpus(&cpus(0..31)).unwrap());
        assert!(c.publish().is_none());
    }

    #[test]
    fn publish_none_when_unchanged() {
        let mut c = Catalog::new();
        let first = c.publish().unwrap();
        assert!(c.publish().is_none());
        let l = list(DeviceKinds::DISKS, vec![desc(disk("sda", 0), "disk:sda")]);
        c.apply_list(&l).unwrap();
        let second = c.publish().unwrap();
        c.apply_list(&l).unwrap();
        assert!(c.publish().is_none());
        // Earlier snapshots stay valid and unchanged.
        assert_eq!(first.entries.len(), 4);
        assert_eq!(second.entries.len(), 5);
    }

    #[test]
    fn partition_parent_link() {
        let mut c = Catalog::new();
        let sda = disk("sda", 0);
        let sda1 = disk("sda1", 1);
        let mut part = desc(sda1.clone(), "disk:sda1");
        part.parent = Some(sda.clone());
        // The partition comes first, so the parent is resolved after the whole list.
        c.apply_list(&list(
            DeviceKinds::DISKS,
            vec![part, desc(sda.clone(), "disk:sda")],
        ))
        .unwrap();
        assert_eq!(entry(&c, &sda1).parent, c.idx(&sda));
        assert!(c.idx(&sda).is_some());
    }

    #[test]
    fn persistent_key_stored() {
        let mut c = Catalog::new();
        let sda = disk("sda", 0);
        let eno1 = net("eno1", 2);
        c.apply_list(&list(
            DeviceKinds::DISKS,
            vec![desc(sda.clone(), "disk:0x5002538e40a1b2c3")],
        ))
        .unwrap();
        let mut wired = desc(eno1.clone(), "net:eno1");
        wired.net_kind = Some(NetKind::Wired);
        c.apply_list(&list(DeviceKinds::NETS, vec![wired])).unwrap();

        let published = c.publish().unwrap();
        let e = published.find(&sda).unwrap();
        assert_eq!(&*e.persistent_key, "disk:0x5002538e40a1b2c3");
        assert_eq!(e.net_kind, None);
        assert_eq!(
            published.find(&eno1).unwrap().net_kind,
            Some(NetKind::Wired)
        );
    }

    #[test]
    fn new_catalog_has_aggregates() {
        let mut c = Catalog::new();
        let published = c.publish().expect("a new catalog publishes once");
        let expected = [
            (DeviceIdentity::CpuPackage, "cpu"),
            (
                DeviceIdentity::Aggregate(AggregateKind::Disks),
                "aggregate:disk",
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Nets),
                "aggregate:net",
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Gpus),
                "aggregate:gpu",
            ),
        ];
        assert_eq!(published.entries.len(), expected.len());
        for (i, (identity, key)) in expected.into_iter().enumerate() {
            let e = &published.entries[i];
            assert_eq!(e.idx, DeviceIdx(i as u16));
            assert_eq!(e.identity, identity);
            assert_eq!(&*e.persistent_key, key);
            assert!(e.present);
            assert_eq!(e.generation, 0);
        }
        assert_eq!(&*published.entries[0].display_name, "CPU");
    }

    #[test]
    fn index_overflow_is_invariant_violation() {
        let mut c = Catalog::new();
        let mut last = Ok((DeviceIdx(0), true));
        for i in 0..=u32::from(u16::MAX) {
            last = c.ensure(&disk("sdx", i), "disk", None, "disk:sdx", None);
            if last.is_err() {
                break;
            }
        }
        let err = last.expect_err("the index space runs out");
        assert_eq!(err.0, "device index overflow");
    }
}
