//! Which device a saved widget key stands for on the host a window shows (tasklens-jmc36, owner
//! decision 2026-11-07 "Match by kind and order", UI-PRF-061 "Another host").
//!
//! Layouts are one shared set for every host, and their widgets save the local device keys. On a
//! remote host tab a key that the host does not have is mapped by kind and order:
//!
//! 1. A saved key that the shown host has is used as it is.
//! 2. Else, on a remote host only, the saved keys of that kind that the shown host lacks are
//!    taken in first-appearance order (widget order as stored); the n-th of them maps to the
//!    n-th device of that kind on the shown host, in sidebar order (disks: whole disks only).
//! 3. With no n-th device the widget stays "Not connected".
//!
//! A device page that borrows the local page's layout pins the local key at its own position to
//! the page's own device; rule 2 counts the other keys only. On the local host a missing key
//! stays "Not connected". Only the kinds whose keys differ between PCs are mapped: disks,
//! adapters, GPUs, fans and batteries. The mapping never writes settings.
//!
//! One resolver serves every lookup: the metric sources ([`super::source::SourceData`]), the
//! engine groups ([`super::members::members`]) and the device pages' layouts.

use std::sync::Arc;

use tl_model::devices::DeviceEntry;
use tl_model::snapshot::Snapshot;
use tl_ui_settings::layout::{ContainerLayout, DeviceSelector};
use tl_ui_settings::layout_boxes::DeviceKind as KeyKind;
use tl_ui_settings::settings::PerformanceSettings;

use super::members::all_rows;
use tl_ui_perf_core::core::{DeviceKey, DeviceKind, DeviceRow, retain_whole_disks};

/// The sidebar kind of a key kind whose keys differ between PCs; `None` for the kinds whose keys
/// are the same on every host (`cpu`, `cpu:<n>`, `memory`, the aggregates).
pub fn remap_kind(kind: KeyKind) -> Option<DeviceKind> {
    match kind {
        KeyKind::Disk => Some(DeviceKind::Disk),
        KeyKind::Net => Some(DeviceKind::Net),
        KeyKind::Gpu => Some(DeviceKind::Gpu),
        KeyKind::Fan => Some(DeviceKind::Fan),
        KeyKind::Battery => Some(DeviceKind::Battery),
        _ => None,
    }
}

/// The sidebar kind of `key` when the key is one that is mapped.
fn kind_of(key: &str) -> Option<DeviceKind> {
    remap_kind(KeyKind::of_key(key))
}

/// What the resolver needs to know about one layout: its saved device keys and the page pin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutKeys {
    /// The distinct saved keys of the mapped kinds, in first-appearance order (metric widgets
    /// and group devices, in widget order).
    keys: Vec<Arc<str>>,
    /// On a remote device page that borrows a local page's layout: the local key at the page's
    /// own position, and the page's own device key it stands for.
    pin: Option<(Arc<str>, Arc<str>)>,
}

impl LayoutKeys {
    /// No keys and no pin: every key is looked up as it is.
    pub const EMPTY: LayoutKeys = LayoutKeys {
        keys: Vec::new(),
        pin: None,
    };

    /// The keys of `layout`, with no pin.
    pub fn of(layout: &ContainerLayout) -> Self {
        let mut keys: Vec<Arc<str>> = Vec::new();
        for w in &layout.widgets {
            let Some(DeviceSelector::Key(k)) = w.content.device() else {
                continue;
            };
            if kind_of(k).is_some() && !keys.iter().any(|s| **s == **k) {
                keys.push(Arc::clone(k));
            }
        }
        Self { keys, pin: None }
    }

    /// The same keys with `pin`.
    pub fn with_pin(mut self, pin: Option<(Arc<str>, Arc<str>)>) -> Self {
        self.pin = pin;
        self
    }

    /// The saved keys, in first-appearance order.
    pub fn keys(&self) -> &[Arc<str>] {
        &self.keys
    }

    /// The page pin: (local key, the page's own key).
    pub fn pin(&self) -> Option<&(Arc<str>, Arc<str>)> {
        self.pin.as_ref()
    }

    fn pinned_local(&self, key: &str) -> bool {
        self.pin.as_ref().is_some_and(|(local, _)| **local == *key)
    }
}

/// The present catalog entry whose persistent key is `key` (or, for a GPU, whose identity gives
/// the key, as phase 1 did). Rule 1.
pub fn find_present<'a>(snap: &'a Snapshot, key: &str) -> Option<&'a DeviceEntry> {
    snap.devices.entries.iter().filter(|e| e.present).find(|e| {
        &*e.persistent_key == key
            || DeviceKey::from_identity(&e.identity).is_some_and(|k| &*k.0 == key)
    })
}

/// The present devices of `kind` on `snap`, in sidebar order before visibility. Disks are whole
/// disks only. `perf` gives `device_order` (local keys; on a remote host the order falls back to
/// kind, then catalog order).
pub fn kind_rows(
    snap: &Snapshot,
    perf: Option<&PerformanceSettings>,
    kind: DeviceKind,
) -> Vec<DeviceRow> {
    let mut rows = all_rows(snap, perf);
    retain_whole_disks(&mut rows, &snap.devices);
    rows.retain(|row| row.kind == kind);
    rows
}

/// The device saved key `key` stands for on `snap`. `remote` says `snap` is a remote host's:
/// only there are missing keys mapped by kind and order. `None`: "Not connected".
pub fn resolve<'a>(
    snap: &'a Snapshot,
    perf: Option<&PerformanceSettings>,
    key: &str,
    keys: &LayoutKeys,
    remote: bool,
) -> Option<&'a DeviceEntry> {
    if let Some(e) = find_present(snap, key) {
        return Some(e);
    }
    if !remote {
        return None;
    }
    let kind = kind_of(key)?;
    if let Some((_, own)) = keys.pin.as_ref().filter(|_| keys.pinned_local(key)) {
        return find_present(snap, own);
    }
    let n = keys
        .keys
        .iter()
        .filter(|k| kind_of(k) == Some(kind) && !keys.pinned_local(k))
        .filter(|k| find_present(snap, k).is_none())
        .position(|k| **k == *key)?;
    let row = kind_rows(snap, perf, kind).into_iter().nth(n)?;
    snap.devices
        .entries
        .iter()
        .find(|e| e.present && e.idx == row.idx)
}

/// The saved key that maps to the shown host's device `shown` on a remote host, so an edit
/// writes the local key the mapping came from (leader decision 2026-11-07). `None` when no
/// saved key maps to it, or on the local host: the edit writes `shown` itself.
pub fn saved_key_for(
    snap: &Snapshot,
    perf: Option<&PerformanceSettings>,
    shown: &str,
    keys: &LayoutKeys,
    remote: bool,
) -> Option<Arc<str>> {
    if !remote {
        return None;
    }
    let idx = find_present(snap, shown)?.idx;
    let pinned = keys.pin.as_ref().map(|(local, _)| local);
    pinned
        .into_iter()
        .chain(keys.keys.iter())
        .filter(|k| find_present(snap, k).is_none())
        .find(|k| resolve(snap, perf, k, keys, true).is_some_and(|e| e.idx == idx))
        .cloned()
}

#[cfg(test)]
mod tests {
    use tl_model::devices::{DeviceCatalog, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::time::BootId;
    use tl_ui_settings::layout::{
        FormSet, LAYOUT_VERSION, MetricKey, SourceSelector, WidgetContent, WidgetOptions,
        WidgetSetting,
    };

    use super::*;

    fn gpu(idx: u16, n: u8) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Gpu {
                uuid: [n; 16],
                pci_bus_id: Arc::from(format!("0000:0{n}:00.0")),
            },
            generation: 0,
            display_name: Arc::from("x"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    fn key(n: u8) -> Arc<str> {
        DeviceKey::from_identity(&gpu(0, n).identity)
            .expect("key")
            .0
    }

    fn snap(entries: Vec<DeviceEntry>) -> Snapshot {
        let mut s = Snapshot::empty(BootId(0), 0);
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        });
        s
    }

    fn layout(keys: &[Arc<str>]) -> ContainerLayout {
        ContainerLayout {
            container: "overview.gpu".into(),
            version: LAYOUT_VERSION,
            preset: None,
            widgets: keys
                .iter()
                .enumerate()
                .map(|(i, k)| WidgetSetting {
                    id: i as u32 + 1,
                    content: WidgetContent::Metric(SourceSelector {
                        device: DeviceSelector::Key(Arc::clone(k)),
                        metric: MetricKey::new("vram"),
                    }),
                    forms: FormSet::VALUE,
                    cell: None,
                    options: WidgetOptions::default(),
                    device_name: None,
                })
                .collect(),
            next_id: keys.len() as u32 + 1,
        }
    }

    #[test]
    fn keys_are_distinct_in_widget_order_and_skip_same_on_every_host() {
        let l = layout(&[
            key(2),
            key(1),
            key(2),
            Arc::from("cpu:3"),
            Arc::from("memory"),
        ]);
        assert_eq!(LayoutKeys::of(&l).keys(), [key(2), key(1)]);
    }

    #[test]
    fn missing_keys_map_by_order_on_a_remote_host_only() {
        let s = snap(vec![gpu(1, 7), gpu(2, 8)]);
        let keys = LayoutKeys::of(&layout(&[key(1), key(2)]));
        let at = |k: &str, remote| resolve(&s, None, k, &keys, remote).map(|e| e.idx);
        assert_eq!(at(&key(1), true), Some(DeviceIdx(1)));
        assert_eq!(at(&key(2), true), Some(DeviceIdx(2)));
        assert_eq!(
            at(&key(1), false),
            None,
            "the local host keeps Not connected"
        );
        assert_eq!(at(&key(7), false), Some(DeviceIdx(1)), "rule 1");
        let one = snap(vec![gpu(1, 7)]);
        assert_eq!(
            resolve(&one, None, &key(2), &keys, true).map(|e| e.idx),
            None
        );
    }

    #[test]
    fn a_present_key_does_not_shift_the_order_and_the_pin_wins() {
        let s = snap(vec![gpu(1, 7), gpu(2, 8)]);
        // The remote key 8 is found by rule 1; key 1 is the first missing key.
        let keys = LayoutKeys::of(&layout(&[key(8), key(1)]));
        assert_eq!(
            resolve(&s, None, &key(1), &keys, true).map(|e| e.idx),
            Some(DeviceIdx(1))
        );
        let keys = LayoutKeys::of(&layout(&[key(1), key(2)])).with_pin(Some((key(2), key(8))));
        assert_eq!(
            resolve(&s, None, &key(2), &keys, true).map(|e| e.idx),
            Some(DeviceIdx(2))
        );
        assert_eq!(
            resolve(&s, None, &key(1), &keys, true).map(|e| e.idx),
            Some(DeviceIdx(1))
        );
        assert_eq!(saved_key_for(&s, None, &key(8), &keys, true), Some(key(2)));
        assert_eq!(saved_key_for(&s, None, &key(8), &keys, false), None);
    }
}
