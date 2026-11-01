//! The Overview Top processes and Timeline boxes on widgets (ADR-019 phase 2 "Fixed widgets",
//! UI-PRF-061 phase 2): the default layout is the fixed widget alone, filling the body; Edit
//! Widgets adds a metric widget beside it, removes it and applies; the fixed widget's demand
//! follows whether it is placed; Reset restores the built-in default. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Pixels,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
use tl_model::layout::{
    ContainerLayout, FixedWidget, PROCESSES_BOX_CONTAINER, TIMELINE_BOX_CONTAINER, WidgetContent,
};
use tl_model::layout_boxes::METRIC_CPU_TOTAL;
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{PerfWidgetEditBox, WidgetSource};
use tl_ui::pages::performance::overview::fixed_box::FixedBox;
use tl_ui::pages::performance::overview::procs_box::PROC_ROW_ID_PREFIX;
use tl_ui::pages::performance::overview::timeline::BANDS_ID;
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::WidgetContainer;
use tl_ui::pages::performance::widget::edit::{WidgetEditor, default_layout};
use tl_ui::pages::performance::widget::metric_widget::widget_id;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

struct Host {
    page: Entity<OverviewPage>,
    sidebar: Entity<DeviceSidebar>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                self.page
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(
                div()
                    .absolute()
                    .w(px(0.))
                    .h(px(0.))
                    .overflow_hidden()
                    .child(self.sidebar.clone()),
            )
    }
}

fn entry(idx: u16, identity: DeviceIdentity, key: &str, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

/// The CPU package at 0 and memory at 1.
fn catalog(version: u64) -> DeviceCatalog {
    DeviceCatalog {
        version,
        entries: Arc::from(vec![
            entry(0, DeviceIdentity::CpuPackage, "cpu", "CPU"),
            entry(1, DeviceIdentity::Memory, "memory", "Memory"),
        ]),
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it.
fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1));
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        settings.motion.smooth_graphs = false;
        // Only the Top processes and the Timeline boxes: the other boxes' widgets would draw
        // `perf-widget-<id>` too and shadow theirs.
        let mut overview =
            OverviewLayoutSetting::with_top_procs(settings.performance.overview_row_unit_px);
        overview
            .grid
            .retain(|b| b.kind == "processes" || b.kind == "timeline");
        settings.performance.overview = Some(overview);
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let sidebar = cx.new(|cx| DeviceSidebar::new(hub.clone(), demand.clone(), cx));
        let page = cx.new(|cx| {
            OverviewPage::new(
                DeviceKey(Arc::from(OVERVIEW_KEY)),
                hub,
                demand,
                &sidebar,
                cx,
            )
        });
        Host { page, sidebar }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(w), px(h)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    draw(vcx);
    let page = vcx.read_entity(&host, |h, _| h.page.clone());
    Rig {
        engine,
        page,
        vcx,
        _host: host,
        _driver: driver,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn near(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < 1.
}

fn proc_record(pid: u32, cpu_pct: f32) -> ProcessRecord {
    let name: Arc<str> = Arc::from(format!("proc{pid}"));
    ProcessRecord {
        key: ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        },
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.clone(),
            name: name.clone(),
            exe: None,
            cmdline: None,
            user: Arc::from("user"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: name,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct,
        rss_anon_bytes: 0,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 0.,
        net_tx_bps: 0.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.,
        validity: FieldValidity::all(Validity::Valid),
    }
}

/// Which box a test works on.
#[derive(Clone, Copy)]
enum Which {
    Procs,
    Timeline,
}

impl Which {
    fn kind(self) -> &'static str {
        match self {
            Self::Procs => "processes",
            Self::Timeline => "timeline",
        }
    }

    fn container(self) -> &'static str {
        match self {
            Self::Procs => PROCESSES_BOX_CONTAINER,
            Self::Timeline => TIMELINE_BOX_CONTAINER,
        }
    }

    fn fixed(self) -> FixedWidget {
        match self {
            Self::Procs => FixedWidget::Processes,
            Self::Timeline => FixedWidget::Timeline,
        }
    }

    fn fixed_name(self) -> &'static str {
        self.kind()
    }

    /// The debug selector of something only the content draws.
    fn content_id(self) -> String {
        match self {
            Self::Procs => format!("{PROC_ROW_ID_PREFIX}12"),
            Self::Timeline => BANDS_ID.to_owned(),
        }
    }
}

impl Rig<'_> {
    fn page_action(&mut self, action: &dyn gpui_kit::Action) {
        let page = self.page.clone();
        self.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(action, window, cx);
        });
        draw(self.vcx);
    }

    fn frame(&mut self, w: Which) -> Entity<FixedBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| match w {
            Which::Procs => page
                .read(cx)
                .procs_frame()
                .expect("the Top processes box")
                .clone(),
            Which::Timeline => page.read(cx).timeline_frame().clone(),
        })
    }

    fn container(&mut self, w: Which) -> Entity<WidgetContainer> {
        let frame = self.frame(w);
        self.vcx.update(|_, cx| frame.read(cx).container().clone())
    }

    fn editor(&mut self, w: Which) -> Entity<WidgetEditor> {
        let frame = self.frame(w);
        self.vcx.update(|_, cx| frame.read(cx).editor().clone())
    }

    fn flow(&mut self, w: Which) -> ContainerLayout {
        let frame = self.frame(w);
        self.vcx.update(|_, cx| frame.read(cx).flow().clone())
    }

    fn edit(&mut self, w: Which) -> Entity<WidgetEditor> {
        self.page_action(&PerfWidgetEditBox {
            kind: w.kind().into(),
        });
        let editor = self.editor(w);
        assert!(self.vcx.read_entity(&editor, |e, _| e.is_editing()));
        editor
    }

    fn demand(&mut self) -> tl_model::demand::Demand {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).demand_part())
    }

    fn entry(&mut self, w: Which) -> Option<ContainerLayout> {
        self.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(w.container())
                .cloned()
        })
    }

    /// 12 processes: pid `10 + i` at `i` % CPU.
    fn publish_procs(&mut self) {
        self.engine.publish_with(|s| {
            let version = s.processes.header.version + 1;
            let records: Vec<ProcessRecord> =
                (0..12u32).map(|i| proc_record(10 + i, i as f32)).collect();
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                records: Arc::from(records),
                ..ProcessSnapshot::default()
            });
        });
        draw(self.vcx);
    }
}

fn source(metric: &str) -> WidgetSource {
    WidgetSource {
        device: Arc::from("cpu"),
        metric: Arc::from(metric),
    }
}

/// The default of each box is its preset's fixed widget alone, drawn by a `FixedWidgetView`
/// that fills the whole body, with the content's own rows or bands, and the page asks for the
/// widget's demand.
#[gpui_kit::test]
fn overview_fixed_default_is_the_widget_alone(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 1000.);
    r.publish_procs();
    for w in [Which::Procs, Which::Timeline] {
        let flow = r.flow(w);
        assert_eq!(flow.container, w.container());
        assert_eq!(flow.widgets.len(), 1, "{flow:?}");
        assert_eq!(flow.widgets[0].content, WidgetContent::Fixed(w.fixed()));
        assert!(flow.preset.is_some(), "a flow layout");
        let id = flow.widgets[0].id;

        let container = r.container(w);
        let (placed, area) = r.vcx.update(|_, cx| {
            let c = container.read(cx);
            (c.placement().bounds(id), c.area())
        });
        let placed = placed.expect("the fixed widget is placed");
        assert!(
            near(placed.size.width, area.width) && near(placed.size.height, area.height),
            "{placed:?} in {area:?}"
        );
        assert!(
            r.vcx.update(|_, cx| container.read(cx).fixed(id).is_some()),
            "a FixedWidgetView"
        );
        assert!(r.vcx.debug_bounds(leak(widget_id(id))).is_some());
        let content = leak(w.content_id());
        assert!(r.vcx.debug_bounds(content).is_some(), "{content}");
        let frame = r.frame(w);
        assert!(r.vcx.update(|_, cx| frame.read(cx).is_placed()));
    }
    let d = r.demand();
    assert!(d.process_top && d.device_timeline, "{d:?}");
}

/// A metric widget added beside the fixed widget: Apply saves a grid layout that holds both,
/// the box draws both by their entities, and Reset restores the built-in default with no entry.
#[gpui_kit::test]
fn overview_fixed_metric_beside_then_reset(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 1000.);
    r.publish_procs();
    for w in [Which::Procs, Which::Timeline] {
        let editor = r.edit(w);
        let fixed_id = r.flow(w).widgets[0].id;
        // The fixed widget is pinned where it was drawn, so it is already in the layout.
        let working = r
            .vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .expect("a working layout");
        assert!(
            working.widgets.iter().any(|x| x.id == fixed_id),
            "{working:?}"
        );
        assert!(
            r.vcx
                .update(|_, cx| editor.read(cx).fixed_add(cx))
                .is_none(),
            "the widget is offered only while the box has none"
        );
        let groups = r.vcx.update(|_, cx| editor.read(cx).source_groups(cx));
        assert!(
            groups.iter().any(|g| &*g.key == "cpu"),
            "the CPU sources: {groups:?}"
        );
        let added = r
            .vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(METRIC_CPU_TOTAL), cx)))
            .expect("a CPU widget added");
        assert_ne!(added, fixed_id);
        assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
        draw(r.vcx);

        let entry = r.entry(w).expect("an entry");
        assert!(entry.preset.is_none(), "a grid layout");
        let ids: Vec<u32> = entry.widgets.iter().map(|x| x.id).collect();
        assert!(ids.contains(&fixed_id) && ids.contains(&added), "{ids:?}");
        let container = r.container(w);
        let placed = r.vcx.update(|_, cx| container.read(cx).placed_ids());
        assert!(
            placed.contains(&fixed_id) && placed.contains(&added),
            "{placed:?}"
        );
        let both = r.vcx.update(|_, cx| {
            (
                container.read(cx).fixed(fixed_id).is_some(),
                container.read(cx).widget(added).is_some(),
            )
        });
        assert_eq!(both, (true, true));
        assert!(r.vcx.debug_bounds(leak(widget_id(added))).is_some());
        assert!(r.vcx.debug_bounds(leak(w.content_id())).is_some());

        // Reset in Edit Widgets and Apply: the built-in default, no entry.
        let editor = r.edit(w);
        assert!(
            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.reset_confirmed(cx)))
        );
        assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
        draw(r.vcx);
        assert!(r.entry(w).is_none(), "no entry after Reset");
        let frame = r.frame(w);
        assert!(r.vcx.update(|_, cx| frame.read(cx).grid().is_none()));
        let container = r.container(w);
        let layout = r.vcx.update(|_, cx| container.read(cx).layout().clone());
        assert_eq!(layout, default_layout(w.container()));
        assert!(r.vcx.debug_bounds(leak(w.content_id())).is_some());
        assert!(
            r.vcx
                .update(|_, cx| container.read(cx).widget(added).is_none())
        );
    }
}

/// Removing the fixed widget drops its demand and shows the user's other widgets; Add Widget
/// offers it again.
#[gpui_kit::test]
fn overview_fixed_remove_drops_demand(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 1000.);
    r.publish_procs();
    for w in [Which::Procs, Which::Timeline] {
        let editor = r.edit(w);
        let fixed_id = r.flow(w).widgets[0].id;
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(METRIC_CPU_TOTAL), cx)))
            .expect("a CPU widget added");
        r.vcx.update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.select(fixed_id, cx);
                assert!(e.remove_selected(cx));
            })
        });
        let offered = r.vcx.update(|_, cx| editor.read(cx).fixed_add(cx));
        assert_eq!(
            offered.map(|a| a.name.to_string()),
            Some(w.fixed_name().to_owned())
        );
        assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
        draw(r.vcx);
        let frame = r.frame(w);
        assert!(!r.vcx.update(|_, cx| frame.read(cx).is_placed()));
        let d = r.demand();
        match w {
            Which::Procs => assert!(!d.process_top, "{d:?}"),
            Which::Timeline => assert!(!d.device_timeline, "{d:?}"),
        }
        assert!(r.vcx.debug_bounds(leak(w.content_id())).is_none());

        // Add it again from Add Widget.
        let editor = r.edit(w);
        let again = r
            .vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed(w.fixed_name(), cx)))
            .expect("the fixed widget is added again");
        assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
        draw(r.vcx);
        let frame = r.frame(w);
        assert!(r.vcx.update(|_, cx| frame.read(cx).is_placed()));
        assert!(r.vcx.debug_bounds(leak(widget_id(again))).is_some());
        let d = r.demand();
        assert!(d.process_top || matches!(w, Which::Timeline), "{d:?}");
        assert!(d.device_timeline || matches!(w, Which::Procs), "{d:?}");
        // Back to the built-in default for the next box.
        let editor = r.edit(w);
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.reset_confirmed(cx)));
        r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
        draw(r.vcx);
    }
}

/// A sample that changes the Top processes rows renders the content only: the box notifies
/// nothing.
#[gpui_kit::test]
fn overview_fixed_sample_does_not_notify_the_box(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 1000.);
    r.publish_procs();
    let frame = r.frame(Which::Procs);
    let before = r.vcx.update(|_, cx| frame.read(cx).notify_count.get());
    r.engine.publish_with(|s| {
        let version = s.processes.header.version + 1;
        let records: Vec<ProcessRecord> = (0..12u32)
            .map(|i| proc_record(10 + i, (11 - i) as f32))
            .collect();
        s.processes = Arc::new(ProcessSnapshot {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            records: Arc::from(records),
            ..ProcessSnapshot::default()
        });
    });
    draw(r.vcx);
    let after = r.vcx.update(|_, cx| frame.read(cx).notify_count.get());
    assert_eq!(before, after, "a sample never notifies the box");
}
