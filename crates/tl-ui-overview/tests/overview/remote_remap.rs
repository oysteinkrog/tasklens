//! Layout widgets on a remote host tab (tasklens-jmc36, owner decision 2026-11-07 "Match by kind
//! and order"): a saved key the shown host lacks maps to the same-kind same-order device, on a
//! remote host only; the saved settings never change by it; and edits on a remote host write the
//! local keys the mapping came from.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot};
use tl_model::time::now_boot_ns;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_overview::widget::container::WidgetContainer;
use tl_ui_overview::widget::edit::{WidgetEditor, default_layout};
use tl_ui_overview::widget::grid::SlotDevice;
use tl_ui_overview::widget::members::{Members, Remap, members};
use tl_ui_overview::widget::remap::LayoutKeys;
use tl_ui_overview::widget::source::{Availability, SourceStore};
use tl_ui_perf_core::core::DeviceKey;
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FormSet, GPU_BOX_CONTAINER, GroupItems, GroupKind,
    GroupOptions, GroupSelector, LAYOUT_VERSION, METRIC_ENGINE_ANY, MetricKey, SourceSelector,
    WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

/// A device of one of the kinds whose keys differ between PCs: its identity on a host.
fn identity(kind: usize, host: &str, n: u8) -> DeviceIdentity {
    let name = |s: &str| Arc::from(format!("{s}{host}{n}"));
    match kind {
        0 => DeviceIdentity::Disk {
            major: 8,
            minor: u32::from(n),
            name: name("sd"),
            wwn_or_serial: None,
        },
        1 => DeviceIdentity::Net {
            netns_ino: 0,
            ifindex: u32::from(n),
            name: name("eth"),
        },
        2 => {
            let byte = if host == "a" { 0x10 + n } else { 0x20 + n };
            DeviceIdentity::Gpu {
                uuid: [byte; 16],
                pci_bus_id: Arc::from(format!("0000:{byte:02x}:00.0")),
            }
        }
        3 => DeviceIdentity::Fan {
            chip: name("chip"),
            index: 1,
        },
        _ => DeviceIdentity::Battery {
            native_path: name("BAT"),
        },
    }
}

/// The kinds of the sweep, in [`identity`] order, with a metric each.
const KINDS: [(&str, &str); 5] = [
    ("disk", "active"),
    ("net", "rx"),
    ("gpu", "vram"),
    ("fan", "rpm"),
    ("battery", "percent"),
];

fn key(kind: usize, host: &str, n: u8) -> Arc<str> {
    DeviceKey::from_identity(&identity(kind, host, n))
        .expect("a key")
        .0
}

/// The catalog index of device `n` of `kind` on host `host`. The remote host lists its devices
/// in another order of indexes, so a match by index would be wrong.
fn idx(kind: usize, host: &str, n: u8) -> DeviceIdx {
    let base = 20 + 10 * kind as u16;
    DeviceIdx(if host == "a" {
        base + u16::from(n)
    } else {
        base + 5 - u16::from(n)
    })
}

/// The devices of host `host`: `per_kind` of each sweep kind, and with `third_gpu` one more GPU.
fn catalog(host: &str, per_kind: u8, third_gpu: bool, version: u64) -> DeviceCatalog {
    let mut entries = Vec::new();
    for (kind, (word, _)) in KINDS.iter().enumerate() {
        let count = if kind == 2 && third_gpu { 3 } else { per_kind };
        for n in 0..count {
            let identity = identity(kind, host, n);
            entries.push(DeviceEntry {
                idx: idx(kind, host, n),
                persistent_key: key(kind, host, n),
                identity,
                generation: 0,
                display_name: Arc::from(format!("{word} {host}{n}")),
                present: true,
                parent: None,
                net_kind: (kind == 1).then_some(NetKind::Wired),
                hot_c: None,
            });
        }
    }
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

/// A battery's percent series, so a battery widget reads a value.
fn battery_series(host: &str, count: u8) -> Arc<SeriesSet> {
    let now = now_boot_ns();
    let mut series: Vec<Arc<SeriesView>> = (0..count)
        .map(|n| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: idx(4, host, n),
                    metric: MetricId::BATTERY_PCT,
                },
                version: 1,
                period_ns: 1_000_000_000,
                samples: vec![Sample::valid(now, 80.)].into(),
            })
        })
        .collect();
    series.sort_by_key(|v| v.id);
    Arc::new(SeriesSet {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        series: Arc::from(series),
        slow: Arc::from([]),
    })
}

fn publish(engine: &FakeEngine, host: &'static str, per_kind: u8, third_gpu: bool) {
    engine.publish_with(|s: &mut Snapshot| {
        let version = s.devices.version + 1;
        s.devices = Arc::new(catalog(host, per_kind, third_gpu, version));
        s.series = battery_series(host, per_kind);
    });
}

fn widget(id: u32, device: Arc<str>, metric: &str) -> WidgetSetting {
    WidgetSetting {
        id,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(device),
            metric: MetricKey::new(metric),
        }),
        forms: FormSet::VALUE | FormSet::LABEL,
        cell: Some(WidgetCell {
            col: 0,
            row: 2 * (id as u16 - 1),
            cols: 6,
            rows: 2,
        }),
        options: WidgetOptions::default(),
        device_name: None,
    }
}

/// A grid layout of the GPU box with `widgets`.
fn grid(widgets: Vec<WidgetSetting>) -> ContainerLayout {
    ContainerLayout {
        container: GPU_BOX_CONTAINER.into(),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.iter().map(|w| w.id).max().unwrap_or(0) + 1,
        widgets,
    }
}

struct Host {
    editor: Option<Entity<WidgetEditor>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(self.editor.iter().map(|e| {
            div()
                .flex_none()
                .w(px(480.))
                .h(px(900.))
                .child(e.clone().cached(StyleRefinement::default().size_full()))
        }))
    }
}

struct Rig<'a> {
    remote_engine: Arc<FakeEngine>,
    /// The remote host's hub ("b").
    remote: Entity<SnapshotHub>,
    /// The local host's hub ("a"), also the global hub.
    local: Entity<SnapshotHub>,
    host: Entity<Host>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

/// The local host "a" and the remote host "b", two devices of each kind each, and with
/// `third_gpu` a third GPU on "b". The window shows nothing until an editor is added.
fn open(cx: &mut TestAppContext, third_gpu: bool) -> Rig<'_> {
    let local_engine = FakeEngine::new();
    publish(&local_engine, "a", 2, false);
    let remote_engine = FakeEngine::new();
    publish(&remote_engine, "b", 2, third_gpu);
    let local_client: Arc<dyn EngineClient> = local_engine.clone();
    let remote_client: Arc<dyn EngineClient> = remote_engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        cx.set_global(EngineGlobal(Arc::clone(&local_client)));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let local = cx.new(|cx| SnapshotHub::new(local_client, cx));
    let global = local.clone();
    cx.update(|cx| cx.set_global(GlobalHub(global)));
    let remote = cx.new(|cx| SnapshotHub::new_remote(remote_client, cx));
    let (host, vcx) = cx.add_window_view(|_, _| Host { editor: None });
    vcx.simulate_scale_factor_change(1.0);
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    vcx.simulate_resize(size(px(500.), px(1000.)));
    vcx.run_until_parked();
    Rig {
        remote_engine,
        remote,
        local,
        host,
        vcx,
        _driver: driver,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

impl Rig<'_> {
    /// Per widget of `layout` on `hub`: its availability and device, from one store whose
    /// layout keys are `layout`'s.
    fn resolve(
        &mut self,
        hub: &Entity<SnapshotHub>,
        layout: &ContainerLayout,
    ) -> Vec<(Availability, Option<DeviceIdx>)> {
        let hub = hub.clone();
        let layout = layout.clone();
        self.vcx.update(|_, cx| {
            let mut store = SourceStore::new(&hub);
            store.set_layout_keys(&layout, cx);
            layout
                .widgets
                .iter()
                .map(|w| {
                    let s = store.source(w.content.metric().expect("a metric widget"), cx);
                    let s = s.read(cx);
                    (s.availability(), s.device())
                })
                .collect()
        })
    }

    /// An editor around a GPU box container on the remote host showing `layout`, with the
    /// remote GPUs 0 and 1 in the slots, in the window.
    fn editor(&mut self, layout: ContainerLayout) -> Entity<WidgetEditor> {
        let hub = self.remote.clone();
        let host = self.host.clone();
        let e = self.vcx.update(|_, cx| {
            let slots = [key(2, "b", 0), key(2, "b", 1)];
            let store = Rc::new(RefCell::new(SourceStore::new(&hub)));
            store.borrow_mut().set_slots(&slots, cx);
            let c = cx.new(|cx| WidgetContainer::new(GPU_BOX_CONTAINER, layout, &hub, store, cx));
            let e = cx.new(|cx| WidgetEditor::new(c, &hub, cx));
            e.update(cx, |e, _| {
                e.set_slots(
                    slots
                        .iter()
                        .map(|k| SlotDevice {
                            key: Arc::clone(k),
                            name: Arc::from("remote GPU"),
                        })
                        .collect(),
                )
            });
            host.update(cx, |h, cx| {
                h.editor = Some(e.clone());
                cx.notify();
            });
            e
        });
        draw(self.vcx);
        self.vcx
            .update(|window, cx| e.update(cx, |e, cx| e.start(window, cx)));
        draw(self.vcx);
        e
    }

    fn working(&mut self, e: &Entity<WidgetEditor>) -> ContainerLayout {
        self.vcx
            .read_entity(e, |e, _| e.edit().and_then(|x| x.working.clone()))
            .expect("a working layout")
    }

    fn add(&mut self, e: &Entity<WidgetEditor>, device: Arc<str>) -> Arc<str> {
        let source = WidgetSource {
            device,
            metric: Arc::from("vram"),
        };
        let id = self
            .vcx
            .update(|_, cx| e.update(cx, |e, cx| e.add(&source, cx)))
            .expect("added");
        draw(self.vcx);
        let working = self.working(e);
        let w = working
            .widgets
            .iter()
            .find(|w| w.id == id)
            .expect("the widget");
        match w.content.device() {
            Some(DeviceSelector::Key(k)) => Arc::clone(k),
            other => panic!("a key, got {other:?}"),
        }
    }
}

/// Every kind whose keys differ between PCs: a layout keyed to host "a" shows host "b"'s
/// same-kind same-order devices on the remote tab, each Available.
#[gpui_kit::test]
fn remap_sweep_every_kind(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let mut widgets = Vec::new();
    for (kind, (_, metric)) in KINDS.iter().enumerate() {
        for n in 0..2 {
            let id = widgets.len() as u32 + 1;
            widgets.push(widget(id, key(kind, "a", n), metric));
        }
    }
    let layout = grid(widgets);
    let remote = r.remote.clone();
    let got = r.resolve(&remote, &layout);
    let want: Vec<_> = (0..KINDS.len())
        .flat_map(|kind| (0..2).map(move |n| (Availability::Available, Some(idx(kind, "b", n)))))
        .collect();
    assert_eq!(got, want);
}

/// Two distinct GPU keys map to the remote GPU 0 and GPU 1, in the order the layout first
/// names them; a key for which the host has no n-th GPU stays Not connected.
#[gpui_kit::test]
fn remap_two_gpu_keys_map_to_gpu0_gpu1(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let remote = r.remote.clone();
    let layout = grid(vec![
        widget(1, key(2, "a", 0), "vram"),
        widget(2, key(2, "a", 1), "vram"),
        widget(3, key(2, "a", 0), "temp"),
    ]);
    let got = r.resolve(&remote, &layout);
    assert_eq!(
        got.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        [
            Some(idx(2, "b", 0)),
            Some(idx(2, "b", 1)),
            Some(idx(2, "b", 0))
        ]
    );
    // The order is the layout's, not the local sidebar's.
    let layout = grid(vec![
        widget(1, key(2, "a", 1), "vram"),
        widget(2, key(2, "a", 0), "vram"),
    ]);
    let got = r.resolve(&remote, &layout);
    assert_eq!(got[0].1, Some(idx(2, "b", 0)));
    assert_eq!(got[1].1, Some(idx(2, "b", 1)));
}

/// The remote host has one device of each kind: the widget keyed to the second local device of
/// a kind stays Not connected.
#[gpui_kit::test]
fn remap_missing_nth_stays_gone(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let engine = Arc::clone(&r.remote_engine);
    publish(&engine, "b", 1, false);
    r.vcx.run_until_parked();
    let remote = r.remote.clone();
    for (kind, (_, metric)) in KINDS.iter().enumerate() {
        let layout = grid(vec![
            widget(1, key(kind, "a", 0), metric),
            widget(2, key(kind, "a", 1), metric),
        ]);
        let got = r.resolve(&remote, &layout);
        assert_eq!(
            got,
            [
                (Availability::Available, Some(idx(kind, "b", 0))),
                (Availability::Gone, None)
            ],
            "{}",
            KINDS[kind].0
        );
    }
}

/// On the local host the keys resolve as they are, and a key the host lacks stays Not
/// connected even when another device of the kind is free (UI-PRF-061).
#[gpui_kit::test]
fn remap_local_host_unchanged(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let local = r.local.clone();
    for (kind, (_, metric)) in KINDS.iter().enumerate() {
        let layout = grid(vec![
            widget(1, key(kind, "a", 1), metric),
            widget(2, key(kind, "x", 0), metric),
        ]);
        let got = r.resolve(&local, &layout);
        assert_eq!(
            got,
            [
                (Availability::Available, Some(idx(kind, "a", 1))),
                (Availability::Gone, None)
            ],
            "{}",
            KINDS[kind].0
        );
    }
}

/// Showing the remote host never writes settings: the saved layout keeps the local keys.
#[gpui_kit::test]
fn remap_settings_unchanged_after_remote(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let layout = grid(vec![
        widget(1, key(2, "a", 0), "vram"),
        widget(2, key(2, "a", 1), "vram"),
    ]);
    let saved = layout.clone();
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.layouts.set(saved));
    });
    let before = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    let e = r.editor(layout);
    // Showing it, and editing it without a change, leaves the settings as they were.
    r.vcx.update(|_, cx| e.update(cx, |e, cx| e.apply(cx)));
    draw(r.vcx);
    let container = r.vcx.read_entity(&e, |e, _| e.container().clone());
    let shown: Vec<_> = r.vcx.update(|_, cx| {
        let c = container.read(cx);
        [1, 2]
            .iter()
            .filter_map(|id| c.source(*id))
            .map(|s| s.read(cx).device())
            .collect()
    });
    assert_eq!(shown, [Some(idx(2, "b", 0)), Some(idx(2, "b", 1))]);
    let after = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    assert_eq!(before, after);
}

/// An engine group keyed to a local GPU finds its GPU on the remote host by kind and order, and
/// only there.
#[gpui_kit::test]
fn remap_engines_group_on_remote(cx: &mut TestAppContext) {
    let r = open(cx, false);
    let group = GroupSelector {
        device: DeviceSelector::Key(key(2, "a", 1)),
        members: GroupKind::Engines,
        metric: MetricKey::new(METRIC_ENGINE_ANY),
        options: GroupOptions {
            limit: None,
            items: GroupItems::Rows,
            arrange: None,
        },
    };
    let layout = grid(vec![
        widget(1, key(2, "a", 0), "vram"),
        WidgetSetting {
            id: 2,
            content: WidgetContent::Group(group.clone()),
            ..widget(2, key(2, "a", 0), "vram")
        },
    ]);
    let keys = LayoutKeys::of(&layout);
    let remote = r.remote.clone();
    r.vcx.update(|_, cx| {
        let snap = Arc::clone(remote.read(cx).current());
        let on = |remote| Remap {
            keys: &keys,
            remote,
        };
        assert!(matches!(
            members(&group, &snap, None, None, false, on(true)),
            Members::List(_)
        ));
        assert_eq!(
            members(&group, &snap, None, None, false, on(false)),
            Members::Gone,
            "exact keys only on the local host"
        );
    });
}

/// Edit Widgets on the remote host pins the GPU box's flow layout with the local GPU keys, and
/// the pinned widgets still show the remote GPUs.
#[gpui_kit::test]
fn remap_pin_to_grid_writes_local_key(cx: &mut TestAppContext) {
    let mut r = open(cx, false);
    let flow = default_layout(GPU_BOX_CONTAINER);
    let slot_of = |id: u32| match flow
        .widgets
        .iter()
        .find(|w| w.id == id)
        .and_then(|w| w.content.device())
    {
        Some(DeviceSelector::Slot(n)) => *n,
        other => panic!("a slot widget, got {other:?}"),
    };
    // Presets list slot 0 first, so the layout order gives the local GPUs in slot order.
    let first_slot_1 = flow
        .widgets
        .iter()
        .position(|w| w.content.device() == Some(&DeviceSelector::Slot(1)));
    assert!(
        flow.widgets[..first_slot_1.expect("slot 1 widgets")]
            .iter()
            .all(|w| w.content.device() == Some(&DeviceSelector::Slot(0)))
    );
    let e = r.editor(flow.clone());
    let working = r.working(&e);
    assert!(!working.widgets.is_empty());
    let mut slots_seen = [false; 2];
    for w in &working.widgets {
        let n = slot_of(w.id);
        slots_seen[usize::from(n)] = true;
        assert_eq!(
            w.content.device(),
            Some(&DeviceSelector::Key(key(2, "a", n))),
            "widget {} of slot {n} has the local key",
            w.id
        );
    }
    assert_eq!(slots_seen, [true, true]);
    let container = r.vcx.read_entity(&e, |e, _| e.container().clone());
    let devices: Vec<(u8, Option<DeviceIdx>)> = r.vcx.update(|_, cx| {
        let c = container.read(cx);
        working
            .widgets
            .iter()
            .filter_map(|w| Some((slot_of(w.id), c.source(w.id)?.read(cx).device())))
            .collect()
    });
    assert!(!devices.is_empty());
    for (n, device) in devices {
        assert_eq!(device, Some(idx(2, "b", n)));
    }
}

/// Add Widget on the remote host: a GPU a saved key maps to is written with that local key, and
/// a GPU no saved key maps to is written with its own key.
#[gpui_kit::test]
fn remap_add_writes_local_key_when_mapped(cx: &mut TestAppContext) {
    let mut r = open(cx, true);
    let layout = grid(vec![
        widget(1, key(2, "a", 0), "vram"),
        widget(2, key(2, "a", 1), "vram"),
    ]);
    let e = r.editor(layout);
    assert_eq!(r.add(&e, key(2, "b", 1)), key(2, "a", 1));
    assert_eq!(r.add(&e, key(2, "b", 0)), key(2, "a", 0));
    assert_eq!(r.add(&e, key(2, "b", 2)), key(2, "b", 2));
}
