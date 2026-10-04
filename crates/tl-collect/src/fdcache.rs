//! File descriptor cache for /proc/<pid>/smaps_rollup, at most 128 entries (ADR-016).
//!
//! Only keys in `Demand.detail_keys` are cached. `proc/smaps.rs` owns the reading and the
//! identity check; this module is only the container. When the cache is full, `insert` hands the
//! fd back and the caller reads through it once and drops it. There is no eviction.

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use tl_model::ids::ProcessKey;

/// Most open `smaps_rollup` fds the cache holds (ADR-016).
pub const FD_CACHE_CAP: usize = 128;

#[derive(Default)]
pub struct FdCache {
    map: FxHashMap<ProcessKey, OwnedFd>,
    scratch: FxHashSet<ProcessKey>,
}

impl FdCache {
    /// Creates an empty cache with room for 128 entries.
    pub fn new() -> Self {
        let mut map = FxHashMap::default();
        map.reserve(FD_CACHE_CAP);
        Self {
            map,
            scratch: FxHashSet::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, key: &ProcessKey) -> Option<BorrowedFd<'_>> {
        self.map.get(key).map(|fd| fd.as_fd())
    }

    /// Stores the fd. Replaces (and closes) an existing fd for the same key.
    /// When the cache holds 128 other keys, returns Err(fd) and stores nothing.
    pub fn insert(&mut self, key: ProcessKey, fd: OwnedFd) -> Result<(), OwnedFd> {
        if self.map.len() >= FD_CACHE_CAP && !self.map.contains_key(&key) {
            return Err(fd);
        }
        self.map.insert(key, fd);
        Ok(())
    }

    /// Drops (closes) the entry. Returns whether one existed. Used when the identity check fails.
    pub fn remove(&mut self, key: &ProcessKey) -> bool {
        self.map.remove(key).is_some()
    }

    /// Drops every entry whose key is not in `keys` (the current Demand.detail_keys).
    pub fn retain_demanded(&mut self, keys: &[ProcessKey]) {
        self.scratch.clear();
        self.scratch.extend(keys.iter().copied());
        let scratch = &self.scratch;
        self.map.retain(|k, _| scratch.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fd::AsRawFd;
    use rustix::fs::{Mode, OFlags};
    use tl_model::time::BootId;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + u64::from(pid),
            boot_id: BootId(7),
            pid,
        }
    }

    fn dev_null() -> OwnedFd {
        rustix::fs::open("/dev/null", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).unwrap()
    }

    fn full_cache() -> FdCache {
        let mut cache = FdCache::new();
        for pid in 0..FD_CACHE_CAP as u32 {
            assert!(cache.insert(key(pid), dev_null()).is_ok());
        }
        cache
    }

    #[test]
    fn insert_up_to_cap() {
        let mut cache = FdCache::new();
        assert!(cache.is_empty());
        for pid in 0..FD_CACHE_CAP as u32 {
            assert!(cache.insert(key(pid), dev_null()).is_ok());
        }
        assert_eq!(cache.len(), 128);
        assert!(!cache.is_empty());
    }

    #[test]
    fn insert_over_cap_returns_fd() {
        let mut cache = full_cache();
        let fd = dev_null();
        let raw = fd.as_raw_fd();
        let back = cache.insert(key(10_000), fd).unwrap_err();
        assert_eq!(back.as_raw_fd(), raw);
        assert_eq!(cache.len(), 128);
        assert!(cache.get(&key(10_000)).is_none());
    }

    #[test]
    fn reinsert_replaces() {
        let mut cache = full_cache();
        let fd = dev_null();
        let raw = fd.as_raw_fd();
        assert!(cache.insert(key(5), fd).is_ok());
        assert_eq!(cache.len(), 128);
        assert_eq!(cache.get(&key(5)).map(|fd| fd.as_raw_fd()), Some(raw));
    }

    #[test]
    fn retain_demanded_drops_others() {
        let mut cache = FdCache::new();
        let (a, b, c) = (key(1), key(2), key(3));
        for k in [a, b, c] {
            assert!(cache.insert(k, dev_null()).is_ok());
        }
        cache.retain_demanded(&[b]);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&a).is_none());
        assert!(cache.get(&b).is_some());
        assert!(cache.get(&c).is_none());
    }

    #[test]
    fn remove_entry() {
        let mut cache = FdCache::new();
        let a = key(1);
        assert!(cache.insert(a, dev_null()).is_ok());
        assert!(cache.remove(&a));
        assert!(cache.get(&a).is_none());
        assert!(!cache.remove(&a));
    }
}
