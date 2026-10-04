//! CPU topology from /sys/devices/system/cpu (ADR-003, ADR-008, PERF-028 to PERF-036).
//!
//! Read once at start and again on CPU hotplug, so allocation is fine here.

use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};

use crate::sysroot::{read_i64, read_trimmed};
use crate::{CollectError, SysRoot};

const CPU_DIR: &str = "devices/system/cpu";

const LIST_ERROR: CollectError = CollectError::Parse {
    file: "cpu list",
    what: "bad cpu list",
};
const SIZE_ERROR: CollectError = CollectError::Parse {
    file: "cache size",
    what: "bad size",
};
const TOO_MANY_CORES: CollectError = CollectError::Parse {
    file: "cpu topology",
    what: "too many cores",
};

/// Parses a kernel CPU list such as "0-3,8,10-11" (whitespace trimmed; empty input gives empty).
///
/// Clears `out` first. A range whose end is below its start is an error.
pub fn parse_cpu_list(s: &[u8], out: &mut SmallVec<[u16; 64]>) -> Result<(), CollectError> {
    out.clear();
    let s = s.trim_ascii();
    if s.is_empty() {
        return Ok(());
    }
    for part in s.split(|b| *b == b',') {
        let (start, end) = match part.iter().position(|b| *b == b'-') {
            Some(i) => (parse_u16(&part[..i])?, parse_u16(&part[i + 1..])?),
            None => {
                let id = parse_u16(part)?;
                (id, id)
            }
        };
        if end < start {
            return Err(LIST_ERROR);
        }
        out.extend(start..=end);
    }
    Ok(())
}

fn parse_u16(s: &[u8]) -> Result<u16, CollectError> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return Err(LIST_ERROR);
    }
    std::str::from_utf8(s)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(LIST_ERROR)
}

/// Parses a sysfs cache `size`: digits plus an optional `K`, `M` or `G` suffix. No suffix means
/// bytes, rounded down to KiB.
pub fn parse_cache_size_kib(s: &[u8]) -> Result<u64, CollectError> {
    let s = s.trim_ascii();
    let (digits, mul_kib, bytes) = match s.last() {
        Some(b'K') => (&s[..s.len() - 1], 1, false),
        Some(b'M') => (&s[..s.len() - 1], 1024, false),
        Some(b'G') => (&s[..s.len() - 1], 1024 * 1024, false),
        _ => (s, 1, true),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(SIZE_ERROR);
    }
    let n: u64 = std::str::from_utf8(digits)
        .ok()
        .and_then(|d| d.parse().ok())
        .ok_or(SIZE_ERROR)?;
    if bytes {
        Ok(n / 1024)
    } else {
        n.checked_mul(mul_kib).ok_or(SIZE_ERROR)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheLevel {
    pub instances: u16,
    pub instance_kib: u64,
    pub total_kib: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheSummary {
    pub l1d: Option<CacheLevel>,
    pub l1i: Option<CacheLevel>,
    pub l2: Option<CacheLevel>,
    pub l3: Option<CacheLevel>,
    pub l4: Option<CacheLevel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogicalCpu {
    pub id: u16,
    pub online: bool,
    pub package: Option<u16>,
    pub core_id: Option<u16>,
    /// Position of this CPU in its sorted `thread_siblings_list`.
    pub thread_index: Option<u8>,
    pub capacity: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct TopologyInfo {
    pub topology: Arc<CpuTopology>,
    /// One per id in `present`, ascending.
    pub logical: Vec<LogicalCpu>,
    /// Distinct packages among online CPUs.
    pub sockets: u16,
    /// Distinct (package, core_id) among online CPUs.
    pub cores: u16,
    /// Online CPUs.
    pub logical_count: u16,
    pub caches: CacheSummary,
}

/// The cache slot a sysfs `cache/indexM` entry fills.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    L1d,
    L1i,
    L2,
    L3,
    L4,
}

/// (package, core_id) and the online CPU ids of that core.
type CoreGroup = ((u16, u16), SmallVec<[u16; 2]>);

/// One deduplicated cache instance.
struct CacheInstance {
    slot: Slot,
    level: u8,
    kind: SmallVec<[u8; 16]>,
    shared: SmallVec<[u16; 64]>,
    kib: u64,
}

pub fn read_topology(root: &SysRoot) -> Result<TopologyInfo, CollectError> {
    let cpu_dir = root.sys_path(CPU_DIR);
    let mut buf = Vec::new();
    let mut list = SmallVec::<[u16; 64]>::new();

    parse_cpu_list(read_trimmed(&cpu_dir.join("online"), &mut buf)?, &mut list)?;
    let mut online = CpuSet::empty();
    for &id in &list {
        online.insert(id);
    }

    let present_bytes = match read_trimmed(&cpu_dir.join("present"), &mut buf) {
        Err(CollectError::NotFound) => match read_trimmed(&cpu_dir.join("possible"), &mut buf) {
            Err(CollectError::NotFound) => {
                return Err(CollectError::Parse {
                    file: "cpu/present",
                    what: "missing",
                });
            }
            other => other?,
        },
        other => other?,
    };
    let mut present = SmallVec::<[u16; 64]>::new();
    parse_cpu_list(present_bytes, &mut present)?;
    present.sort_unstable();
    present.dedup();

    let mut logical = Vec::with_capacity(present.len());
    for &id in &present {
        logical.push(read_logical(
            &cpu_dir,
            id,
            online.contains(id),
            &mut buf,
            &mut list,
        )?);
    }

    let caches = read_caches(&cpu_dir, &logical, &mut buf)?;
    let mut l3: Vec<&CacheInstance> = caches.iter().filter(|c| c.slot == Slot::L3).collect();
    l3.sort_by_key(|c| c.shared.iter().copied().min());

    let core_list = read_optional_set(&root.sys_path("devices/cpu_core/cpus"), &mut buf)?;
    let atom_list = read_optional_set(&root.sys_path("devices/cpu_atom/cpus"), &mut buf)?;

    // Capacity rule: only when the online CPUs do not all report the same value.
    let capacities: Vec<u32> = logical
        .iter()
        .filter(|c| c.online)
        .filter_map(|c| c.capacity)
        .collect();
    let max_capacity = capacities.iter().copied().max();
    let mixed_capacity = capacities.iter().any(|&c| Some(c) != max_capacity);

    // Distinct (package, core_id) among online CPUs, each with its online CPU ids ascending.
    let mut keyed: Vec<CoreGroup> = Vec::new();
    for cpu in logical.iter().filter(|c| c.online) {
        let (Some(package), Some(core_id)) = (cpu.package, cpu.core_id) else {
            continue;
        };
        match keyed.iter_mut().find(|(key, _)| *key == (package, core_id)) {
            Some((_, ids)) => ids.push(cpu.id),
            None => keyed.push(((package, core_id), SmallVec::from_slice(&[cpu.id]))),
        }
    }
    keyed.sort_by_key(|(key, _)| *key);

    let mut cores = Vec::with_capacity(keyed.len());
    for (index, (_, ids)) in keyed.into_iter().enumerate() {
        let first = ids.first().copied().unwrap_or_default();
        let l3_group = l3
            .iter()
            .position(|c| c.shared.contains(&first))
            .unwrap_or(0);
        let kind = if ids.iter().all(|&id| core_list.contains(id)) && !core_list.is_empty() {
            CoreKind::Performance
        } else if ids.iter().all(|&id| atom_list.contains(id)) && !atom_list.is_empty() {
            CoreKind::Efficiency
        } else if mixed_capacity {
            let capacity = logical
                .iter()
                .find(|c| c.id == first)
                .and_then(|c| c.capacity);
            if capacity.is_some() && capacity == max_capacity {
                CoreKind::Performance
            } else {
                CoreKind::Efficiency
            }
        } else {
            CoreKind::Standard
        };
        cores.push(CoreInfo {
            id: u16::try_from(index).map_err(|_| TOO_MANY_CORES)?,
            logical: ids,
            l3_group: u16::try_from(l3_group).map_err(|_| TOO_MANY_CORES)?,
            kind,
        });
    }

    let mut packages: SmallVec<[u16; 8]> = logical
        .iter()
        .filter(|c| c.online)
        .filter_map(|c| c.package)
        .collect();
    packages.sort_unstable();
    packages.dedup();
    let logical_count = logical.iter().filter(|c| c.online).count();

    Ok(TopologyInfo {
        topology: Arc::new(CpuTopology {
            online,
            cores: cores.as_slice().into(),
        }),
        sockets: u16::try_from(packages.len()).map_err(|_| TOO_MANY_CORES)?,
        cores: u16::try_from(cores.len()).map_err(|_| TOO_MANY_CORES)?,
        logical_count: u16::try_from(logical_count).map_err(|_| TOO_MANY_CORES)?,
        caches: summarize(&caches),
        logical,
    })
}

/// Reads `cpuN/topology/*` (online CPUs only) and `cpuN/cpu_capacity`. Missing files give `None`.
fn read_logical(
    cpu_dir: &Path,
    id: u16,
    online: bool,
    buf: &mut Vec<u8>,
    list: &mut SmallVec<[u16; 64]>,
) -> Result<LogicalCpu, CollectError> {
    let dir = cpu_dir.join(format!("cpu{id}"));
    let capacity = read_optional_number(&dir.join("cpu_capacity"), buf)?;
    let mut cpu = LogicalCpu {
        id,
        online,
        package: None,
        core_id: None,
        thread_index: None,
        capacity,
    };
    if !online {
        return Ok(cpu);
    }
    let topo = dir.join("topology");
    cpu.package = read_optional_number(&topo.join("physical_package_id"), buf)?;
    cpu.core_id = read_optional_number(&topo.join("core_id"), buf)?;
    match read_trimmed(&topo.join("thread_siblings_list"), buf) {
        Ok(bytes) => {
            parse_cpu_list(bytes, list)?;
            list.sort_unstable();
            list.dedup();
            cpu.thread_index = list
                .iter()
                .position(|&s| s == id)
                .and_then(|i| u8::try_from(i).ok());
        }
        Err(CollectError::NotFound) => {}
        Err(e) => return Err(e),
    }
    Ok(cpu)
}

/// A missing file gives `None`, and so does a negative value (a package id of -1 means unknown).
fn read_optional_number<T: TryFrom<i64>>(
    path: &Path,
    buf: &mut Vec<u8>,
) -> Result<Option<T>, CollectError> {
    match read_i64(path, buf) {
        Ok(n) if n < 0 => Ok(None),
        Ok(n) => T::try_from(n).map(Some).map_err(|_| CollectError::Parse {
            file: "cpu topology",
            what: "number out of range",
        }),
        Err(CollectError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A CPU list file that may be missing (the hybrid `cpu_core` and `cpu_atom` lists).
fn read_optional_set(path: &Path, buf: &mut Vec<u8>) -> Result<CpuSet, CollectError> {
    let mut set = CpuSet::empty();
    match read_trimmed(path, buf) {
        Ok(bytes) => {
            let mut list = SmallVec::<[u16; 64]>::new();
            parse_cpu_list(bytes, &mut list)?;
            for id in list {
                set.insert(id);
            }
        }
        Err(CollectError::NotFound) => {}
        Err(e) => return Err(e),
    }
    Ok(set)
}

/// Every distinct cache instance seen by the online CPUs, deduplicated by
/// (level, type, shared_cpu_list), in the order found (CPU ascending, then index ascending).
fn read_caches(
    cpu_dir: &Path,
    logical: &[LogicalCpu],
    buf: &mut Vec<u8>,
) -> Result<Vec<CacheInstance>, CollectError> {
    let mut out: Vec<CacheInstance> = Vec::new();
    for cpu in logical.iter().filter(|c| c.online) {
        let cache_dir = cpu_dir.join(format!("cpu{}/cache", cpu.id));
        let entries = match std::fs::read_dir(&cache_dir) {
            Ok(entries) => entries,
            Err(e) => match CollectError::from(e) {
                CollectError::NotFound => continue,
                other => return Err(other),
            },
        };
        let mut indexes: SmallVec<[u32; 8]> = SmallVec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(n) = name
                .to_str()
                .and_then(|n| n.strip_prefix("index"))
                .and_then(|n| n.parse().ok())
            {
                indexes.push(n);
            }
        }
        indexes.sort_unstable();

        for index in indexes {
            let dir = cache_dir.join(format!("index{index}"));
            let level: u8 = match read_optional_number(&dir.join("level"), buf)? {
                Some(level) => level,
                None => continue,
            };
            let kind = match read_trimmed(&dir.join("type"), buf) {
                Ok(bytes) => SmallVec::<[u8; 16]>::from_slice(bytes),
                Err(CollectError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let slot = match (level, kind.as_slice()) {
                (1, b"Data" | b"Unified") => Slot::L1d,
                (1, b"Instruction") => Slot::L1i,
                (2, b"Unified") => Slot::L2,
                (3, b"Unified") => Slot::L3,
                (4, b"Unified") => Slot::L4,
                _ => continue,
            };
            let mut shared = SmallVec::<[u16; 64]>::new();
            match read_trimmed(&dir.join("shared_cpu_list"), buf) {
                Ok(bytes) => parse_cpu_list(bytes, &mut shared)?,
                Err(CollectError::NotFound) => continue,
                Err(e) => return Err(e),
            }
            shared.sort_unstable();
            shared.dedup();
            let seen = out
                .iter()
                .any(|c| c.level == level && c.kind == kind && c.shared == shared);
            if seen {
                continue;
            }
            let kib = match read_trimmed(&dir.join("size"), buf) {
                Ok(bytes) => parse_cache_size_kib(bytes)?,
                Err(CollectError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            out.push(CacheInstance {
                slot,
                level,
                kind,
                shared,
                kib,
            });
        }
    }
    Ok(out)
}

fn summarize(instances: &[CacheInstance]) -> CacheSummary {
    let level = |slot: Slot| {
        let mut found = instances.iter().filter(|c| c.slot == slot);
        let first = found.next()?;
        let mut out = CacheLevel {
            instances: 1,
            instance_kib: first.kib,
            total_kib: first.kib,
        };
        for c in found {
            out.instances = out.instances.saturating_add(1);
            out.total_kib = out.total_kib.saturating_add(c.kib);
        }
        Some(out)
    };
    CacheSummary {
        l1d: level(Slot::L1d),
        l1i: level(Slot::L1i),
        l2: level(Slot::L2),
        l3: level(Slot::L3),
        l4: level(Slot::L4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn list(s: &str) -> Result<Vec<u16>, CollectError> {
        let mut out = SmallVec::new();
        parse_cpu_list(s.as_bytes(), &mut out)?;
        Ok(out.to_vec())
    }

    #[test]
    fn cpu_list_cases() {
        assert_eq!(list("0-3,8,10-11").unwrap(), [0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(list("").unwrap(), Vec::<u16>::new());
        assert_eq!(list(" \n").unwrap(), Vec::<u16>::new());
        assert_eq!(list("0-31\n").unwrap(), (0..32).collect::<Vec<_>>());
        assert_eq!(list("5").unwrap(), [5]);
        for bad in ["3-1", "a", "1,,2", "1-", "-1", "1-2-3", "65536", "0,"] {
            assert!(
                matches!(list(bad), Err(CollectError::Parse { .. })),
                "{bad:?}"
            );
        }

        // `out` is cleared before parsing.
        let mut out = SmallVec::from_slice(&[9, 9]);
        parse_cpu_list(b"1", &mut out).unwrap();
        assert_eq!(out.as_slice(), [1]);
    }

    proptest! {
        #[test]
        fn prop_cpu_list_never_panics(
            bytes in prop_oneof![
                prop::collection::vec(any::<u8>(), 0..=64),
                prop::collection::vec(prop::sample::select(b"0123456789,- \n".to_vec()), 0..=24),
            ]
        ) {
            let mut out = SmallVec::new();
            if parse_cpu_list(&bytes, &mut out).is_ok() {
                // A non-blank list that parses always names at least one CPU.
                prop_assert!(bytes.trim_ascii().is_empty() || !out.is_empty());
            }
        }
    }
}
