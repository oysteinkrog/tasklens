//! Formatted cell texts, cached per row and column (ADR-009 "Text caching").
//!
//! A table cell re-formats only when its value bits or validity change, so a 1 Hz table update
//! formats only the cells that changed and the frame loop never calls `format!`. Middle-ellipsized
//! paths are cached per 8 px width bucket (UI-FND-052). A new `UiMetrics.style_version` (scale,
//! row height or font change) clears everything (UI-FND-003).

use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

use gpui_kit::{Pixels, SharedString};
use rustc_hash::{FxBuildHasher, FxHashMap};
use tl_model::ids::ColumnId;
use tl_model::validity::Validity;

use crate::table::columns::TableColumn;

/// Most cell entries kept after `begin_version` evicts.
pub const MAX_ENTRIES: usize = 8192;

/// An entry not seen in this many table versions is evicted.
pub const KEEP_VERSIONS: u64 = 3;

/// Width of one path-cache bucket in pixels.
const PATH_BUCKET_PX: f32 = 8.0;

struct CacheEntry {
    bits: u64,
    validity: Validity,
    text: SharedString,
    /// Table version of the last lookup.
    last_seen: u64,
}

/// Cache of formatted cell texts for one table. `R` is the table's row identity (`NodeId` for
/// processes), `C` its column id (`ColumnId` for processes, `SysColumnId` for the system tables).
pub struct CellTextCache<R: Eq + Hash, C: TableColumn = ColumnId> {
    cells: FxHashMap<(R, C), CacheEntry>,
    /// Key: row, column, width bucket `floor(width_px / 8)`. Value: FxHash of the source text and
    /// the truncated text.
    paths: FxHashMap<(R, C, u16), (u64, SharedString)>,
    table_version: u64,
    style_version: u64,
    hits: u64,
    misses: u64,
    /// Reused by every `format` call so a miss allocates only the stored text.
    scratch: String,
}

impl<R: Eq + Hash, C: TableColumn> Default for CellTextCache<R, C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Eq + Hash, C: TableColumn> CellTextCache<R, C> {
    pub fn new() -> Self {
        Self {
            cells: FxHashMap::default(),
            paths: FxHashMap::default(),
            table_version: 0,
            style_version: 0,
            hits: 0,
            misses: 0,
            scratch: String::new(),
        }
    }

    /// Call once per new table version (`ProcessTableView.header.version` or the owner's
    /// version). A changed `style_version` clears both maps. A new table version evicts entries
    /// not seen in the last `KEEP_VERSIONS` versions, then the oldest entries above `MAX_ENTRIES`.
    pub fn begin_version(&mut self, table_version: u64, style_version: u64) {
        if style_version != self.style_version {
            self.clear();
            self.style_version = style_version;
        }
        if table_version == self.table_version {
            return;
        }
        self.table_version = table_version;

        let oldest_kept = table_version.saturating_sub(KEEP_VERSIONS - 1);
        self.cells.retain(|_, e| e.last_seen >= oldest_kept);
        if self.cells.len() > MAX_ENTRIES {
            self.evict_oldest(self.cells.len() - MAX_ENTRIES);
        }
        // Path entries carry no version. Rows that left and old width buckets pile up, so the
        // whole path map is dropped once it passes the cell limit.
        if self.paths.len() > MAX_ENTRIES {
            self.paths.clear();
        }
    }

    /// Removes the `excess` entries with the smallest `last_seen`.
    fn evict_oldest(&mut self, excess: usize) {
        let mut seen: Vec<u64> = self.cells.values().map(|e| e.last_seen).collect();
        let (_, &mut threshold, _) = seen.select_nth_unstable(excess - 1);
        let below = seen.iter().filter(|&&v| v < threshold).count();
        // All entries below the threshold go, then entries at it until `excess` are gone.
        let mut at_threshold = excess - below;
        self.cells.retain(|_, e| {
            if e.last_seen < threshold {
                false
            } else if e.last_seen == threshold && at_threshold > 0 {
                at_threshold -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Returns the cached text when `bits` and `validity` match, else calls `format` once with an
    /// empty scratch `String`, stores the result and returns it.
    pub fn get_or_format(
        &mut self,
        row: R,
        col: C,
        bits: u64,
        validity: Validity,
        format: impl FnOnce(&mut String),
    ) -> SharedString {
        let version = self.table_version;
        let entry = self.cells.entry((row, col));
        if let std::collections::hash_map::Entry::Occupied(o) = &entry {
            let e = o.get();
            if e.bits == bits && e.validity == validity {
                self.hits += 1;
                let text = e.text.clone();
                entry.and_modify(|e| e.last_seen = version);
                return text;
            }
        }
        self.misses += 1;
        self.scratch.clear();
        format(&mut self.scratch);
        let text = SharedString::new(&self.scratch);
        let new = CacheEntry {
            bits,
            validity,
            text: text.clone(),
            last_seen: version,
        };
        entry.insert_entry(new);
        text
    }

    /// Names and other `Arc<str>` texts. Stores `SharedString::from(Arc<str>)`, which shares the
    /// `Arc` (texts of 23 bytes or less are stored inline). Hits when the stored text points at the
    /// same `Arc` or has equal contents.
    pub fn get_or_insert_arc(&mut self, row: R, col: C, text: &Arc<str>) -> SharedString {
        let version = self.table_version;
        let entry = self.cells.entry((row, col));
        if let std::collections::hash_map::Entry::Occupied(o) = &entry {
            let stored = &o.get().text;
            if std::ptr::eq(stored.as_ptr(), text.as_ptr()) || stored.as_str() == &**text {
                self.hits += 1;
                let shared = stored.clone();
                entry.and_modify(|e| e.last_seen = version);
                return shared;
            }
        }
        self.misses += 1;
        let shared = SharedString::from(Arc::clone(text));
        let new = CacheEntry {
            bits: 0,
            validity: Validity::Valid,
            text: shared.clone(),
            last_seen: version,
        };
        entry.insert_entry(new);
        shared
    }

    /// Middle-ellipsized path for a cell `width` wide. Calls `truncate` only when the width bucket
    /// `floor(width / 8 px)` or the source text changed.
    pub fn path(
        &mut self,
        row: R,
        col: C,
        width: Pixels,
        source: &Arc<str>,
        truncate: impl FnOnce(&str, Pixels) -> SharedString,
    ) -> SharedString {
        let bucket = (width.as_f32() / PATH_BUCKET_PX).floor() as u16;
        let hash = FxBuildHasher.hash_one(&**source);
        let entry = self.paths.entry((row, col, bucket));
        if let std::collections::hash_map::Entry::Occupied(o) = &entry
            && o.get().0 == hash
        {
            self.hits += 1;
            return o.get().1.clone();
        }
        self.misses += 1;
        let text = truncate(source, width);
        entry.insert_entry((hash, text.clone()));
        text
    }

    /// Every cached cell text with its column. Entries not seen in the last `KEEP_VERSIONS`
    /// table versions are already gone. Path texts are not included: they are cut to a width.
    pub fn cell_texts(&self) -> impl Iterator<Item = (C, &SharedString)> {
        self.cells.iter().map(|((_, c), e)| (*c, &e.text))
    }

    /// Drops all cell and path entries. Counters are kept.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.paths.clear();
    }

    /// `hits / (hits + misses)`, 1.0 before the first lookup.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            1.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// `(hits, misses)` since the cache was created.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Number of cell entries (path entries are not counted).
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui_kit::px;

    use super::*;

    const CPU: ColumnId = ColumnId::ALL[0];

    fn fmt_value(v: u64) -> impl FnOnce(&mut String) {
        move |s: &mut String| {
            use std::fmt::Write;
            write!(s, "{v}").unwrap();
        }
    }

    #[test]
    fn reformats_on_bits_or_validity_change() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        let runs = Cell::new(0);
        let get = |cache: &mut CellTextCache<u32>, bits: u64, validity: Validity| {
            cache.get_or_format(1, CPU, bits, validity, |s| {
                runs.set(runs.get() + 1);
                s.push_str(&bits.to_string());
            })
        };
        let a = get(&mut cache, 42, Validity::Valid);
        let b = get(&mut cache, 42, Validity::Valid);
        assert_eq!(runs.get(), 1);
        assert_eq!(a, b);
        get(&mut cache, 42, Validity::WarmingUp);
        assert_eq!(runs.get(), 2);
        get(&mut cache, 43, Validity::WarmingUp);
        assert_eq!(runs.get(), 3);
    }

    #[test]
    fn hit_rate_above_95_over_10_versions() {
        let mut cache = CellTextCache::<u32>::new();
        for version in 1..=10 {
            if version == 2 {
                // Measure versions 2..=10 only: version 1 is the first fill, all misses.
                cache.hits = 0;
                cache.misses = 0;
            }
            cache.begin_version(version, 1);
            for row in 0..100u32 {
                for &col in &ColumnId::ALL[..9] {
                    let bits = u64::from(row) * 31 + col as u64;
                    cache.get_or_format(row, col, bits, Validity::Valid, fmt_value(bits));
                }
            }
        }
        assert!(cache.hit_rate() > 0.95, "hit rate {}", cache.hit_rate());
    }

    #[test]
    fn evicts_after_3_versions() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        cache.get_or_format(1, CPU, 1, Validity::Valid, fmt_value(1));
        cache.begin_version(3, 1);
        cache.get_or_format(2, CPU, 2, Validity::Valid, fmt_value(2));
        assert_eq!(cache.len(), 2);
        cache.begin_version(5, 1);
        // Row 1 was last seen at version 1 and is gone; row 2 (version 3) is within 3 versions.
        assert_eq!(cache.len(), 1);
        assert!(!cache.cells.contains_key(&(1, CPU)));
        assert!(cache.cells.contains_key(&(2, CPU)));
    }

    #[test]
    fn caps_at_8192() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        for row in 0..9000u32 {
            cache.get_or_format(row, CPU, 0, Validity::Valid, fmt_value(0));
        }
        assert_eq!(cache.len(), 9000);
        cache.begin_version(2, 1);
        assert_eq!(cache.len(), MAX_ENTRIES);
    }

    #[test]
    fn cap_removes_oldest_first() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        for row in 0..1000u32 {
            cache.get_or_format(row, CPU, 0, Validity::Valid, fmt_value(0));
        }
        cache.begin_version(2, 1);
        for row in 1000..9000u32 {
            cache.get_or_format(row, CPU, 0, Validity::Valid, fmt_value(0));
        }
        cache.begin_version(3, 1);
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.cells.values().filter(|e| e.last_seen == 2).count() == 8000);
    }

    #[test]
    fn clears_on_style_version() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        cache.get_or_format(1, CPU, 1, Validity::Valid, fmt_value(1));
        let path: Arc<str> = Arc::from("/usr/lib/systemd/systemd");
        cache.path(1, CPU, px(200.0), &path, |s, _| SharedString::new(s));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.paths.len(), 1);
        cache.begin_version(1, 2);
        assert_eq!(cache.len(), 0);
        assert!(cache.paths.is_empty());
    }

    #[test]
    fn path_width_bucket_8px() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        let path: Arc<str> = Arc::from("/usr/lib/systemd/systemd-journald");
        let runs = Cell::new(0);
        let at = |cache: &mut CellTextCache<u32>, width: f32, source: &Arc<str>| {
            cache.path(1, CPU, px(width), source, |s, _| {
                runs.set(runs.get() + 1);
                SharedString::new(s)
            })
        };
        at(&mut cache, 200.0, &path);
        assert_eq!(runs.get(), 1);
        at(&mut cache, 205.0, &path);
        assert_eq!(runs.get(), 1, "205 px is in bucket 25 like 200 px");
        at(&mut cache, 208.0, &path);
        assert_eq!(runs.get(), 2, "208 px is bucket 26");
        let other: Arc<str> = Arc::from("/usr/bin/bash");
        at(&mut cache, 208.0, &other);
        assert_eq!(runs.get(), 3, "a changed source re-truncates");
    }

    #[test]
    fn arc_names_not_copied() {
        let mut cache = CellTextCache::<u32>::new();
        cache.begin_version(1, 1);
        // Longer than smol_str's 23-byte inline limit, so the text stays on the heap in the Arc.
        let name: Arc<str> = Arc::from("gnome-shell-calendar-server");
        let a = cache.get_or_insert_arc(1, CPU, &name);
        assert!(std::ptr::eq(a.as_ptr(), name.as_ptr()));
        let b = cache.get_or_insert_arc(1, CPU, &name);
        assert!(std::ptr::eq(b.as_ptr(), name.as_ptr()));
        // Equal text in a different Arc is a hit and keeps the stored text.
        let copy: Arc<str> = Arc::from(&*name);
        let c = cache.get_or_insert_arc(1, CPU, &copy);
        assert!(std::ptr::eq(c.as_ptr(), name.as_ptr()));
        assert_eq!(cache.misses, 1);
        assert_eq!(cache.hits, 2);
    }
}
