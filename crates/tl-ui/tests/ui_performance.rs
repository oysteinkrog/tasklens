//! Performance tab: the Details grid model and its rendering (E24, PERF-011 to PERF-013,
//! UI-PRF-021).

use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::validity::Validity;
use tl_ui::actions::OpenAdditionalSetup;
use tl_ui::components::kv_grid::{self, KvWords, shown_text_in};
use tl_ui::pages::performance::details::{
    DetailWords, Fallback, KeyValueModel, fallback_word, render_details,
};
use tl_ui::theme::metrics;

const ROW: &str = "Speed";

/// Value, tooltip, link, and whether the action is `OpenAdditionalSetup`.
fn read(model: &KeyValueModel) -> (String, Option<String>, bool, bool) {
    let row = &model.rows[0];
    let setup = row
        .action
        .as_ref()
        .is_some_and(|a| a.partial_eq(&OpenAdditionalSetup { focus_row: None }));
    (
        row.value.to_string(),
        row.tooltip.as_ref().map(|t| t.to_string()),
        row.link,
        setup,
    )
}

/// Validity, fallback, reason, then the expected word, tooltip and link.
type Case = (
    Validity,
    Fallback,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    bool,
);

#[test]
fn kv_grid_fallback_words_per_validity() {
    use Fallback::*;
    use Validity::*;
    let cases: [Case; 8] = [
        (WarmingUp, NotReported, None, "Collecting…", None, false),
        (
            Restricted,
            NotSupported,
            None,
            "Restricted",
            Some("Needs administrator access."),
            false,
        ),
        (
            Unsupported,
            NeedsSetup,
            None,
            "Needs setup",
            Some("Needs setup. Open Additional Setup…"),
            true,
        ),
        (
            Unsupported,
            NotSupported,
            None,
            "Not supported",
            Some("Not available on this device"),
            false,
        ),
        (Unsupported, NotReported, None, "Not reported", None, false),
        (
            Failed,
            Unavailable,
            Some("EIO"),
            "Unavailable",
            Some("Could not read: EIO"),
            false,
        ),
        (Valid, Unknown, None, "Unknown", None, false),
        (Unsupported, Unknown, None, "Unknown", None, false),
    ];
    for (validity, fallback, reason, word, tooltip, link) in cases {
        let what = format!("{validity:?} {fallback:?}");
        let words = DetailWords::default();
        assert_eq!(fallback_word(&words, validity, fallback).0, word, "{what}");

        let mut model = KeyValueModel::new(&[ROW]);
        assert!(
            model.set_fallback(ROW, validity, fallback, reason),
            "{what}"
        );
        assert_eq!(
            read(&model),
            (word.to_owned(), tooltip.map(str::to_owned), link, link),
            "{what}"
        );
        assert!(
            !model.set_fallback(ROW, validity, fallback, reason),
            "{what}: unchanged"
        );

        // The grid draws the same word, and the link only for Needs setup.
        let row = model.to_kv_rows().next().expect("visible row");
        assert_eq!(
            shown_text_in(&row.value, &KvWords::default()).as_deref(),
            Some(word),
            "{what}: grid"
        );
        assert_eq!(row.value.link.is_some(), link, "{what}: grid link");
    }

    // Stale keeps the value (its own test below); Valid has no word.
    let words = DetailWords::default();
    let (word, tip) = fallback_word(&words, Valid, NotReported);
    assert_eq!((word.as_ref(), tip), ("", None));
}

#[test]
fn kv_grid_stale_keeps_value() {
    let mut model = KeyValueModel::new(&[ROW]);
    model.set_text(ROW, "4631 MHz");
    assert!(model.set_stale(ROW, 125_000_000_000));
    let row = &model.rows[0];
    assert_eq!(row.value, "4631 MHz");
    assert_eq!(row.validity, Validity::Stale);
    assert_eq!(row.tooltip.as_deref(), Some("Last updated 2m 5s ago"));
    assert!(!model.set_stale(ROW, 125_400_000_000), "same short age");
    assert!(model.set_stale(ROW, 126_000_000_000), "new age");

    let row = model.to_kv_rows().next().expect("visible row");
    assert_eq!(
        shown_text_in(&row.value, &KvWords::default()).as_deref(),
        Some("4631 MHz")
    );
}

#[test]
fn kv_grid_hidden_rows_rule() {
    let mut model = KeyValueModel::new(&["Model", "Serial", "Speed"]);
    model.set_text("Model", "X");
    model.set_text("Serial", "S123");
    model.set_text("Speed", "4 GHz");
    model.hide("Serial");

    let labels: Vec<String> = model.to_kv_rows().map(|r| r.label.to_string()).collect();
    assert_eq!(labels, ["Model", "Speed"]);
    let mut copy = String::new();
    model.copy_into(&mut copy);
    assert_eq!(copy, "    Model: X\n    Speed: 4 GHz");

    model.show("Serial");
    assert_eq!(model.to_kv_rows().count(), 3);
}

struct Details(KeyValueModel);

impl Render for Details {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        div()
            .w(px(1000.))
            .h(px(600.))
            .child(render_details(&self.0, &m, cx))
    }
}

const LABELS: [&str; 14] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "r13",
];

#[gpui_kit::test]
fn details_grid_three_columns_46px(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        kv_grid::init(cx);
    });
    let mut model = KeyValueModel::new(&LABELS);
    for (i, label) in LABELS.iter().enumerate() {
        model.set_valid(label, i as u64, |out| out.push_str(label));
    }
    let (_, cx) = cx.add_window_view(move |_, _| Details(model));
    let cx: &mut VisualTestContext = cx;
    // The grid learns its width on the first paint and lays out on the next.
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }

    let mut xs = Vec::new();
    let mut first_line = Vec::new();
    for n in 0..14 {
        let selector: &'static str = Box::leak(format!("TlKv-row-{n}").into_boxed_str());
        let b = cx.debug_bounds(selector).expect("pair drawn");
        let (line, column) = (n / 3, n % 3);
        assert_eq!(b.size.height, px(46.), "pair {n} height");
        assert_eq!(b.origin.y, px(46. * line as f32), "pair {n} y");
        if line == 0 {
            xs.push(b.origin.x);
            first_line.push(b);
        } else {
            assert_eq!(b.origin.x, xs[column], "pair {n} column");
        }
    }
    assert_eq!(xs.len(), 3);
    assert!(xs[0] < xs[1] && xs[1] < xs[2], "three columns: {xs:?}");
    // The column gap is sp6, 16 px at 100%.
    for pair in first_line.windows(2) {
        let gap = f32::from(pair[1].left() - pair[0].right());
        assert!((gap - 16.).abs() < 0.01, "column gap {gap}");
    }
    // 14 pairs fill 5 lines: the last line holds pairs 12 and 13.
    assert!(cx.debug_bounds("TlKv-row-14").is_none());
}

// ---- Device page frame (E24, UI-PRF-020; PERF-008, PERF-013, R-058) ----

mod page_frame {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AnyElement, App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render,
        SharedString, Styled as _, TestAppContext, TestDispatcher, VisualTestContext, Window, div,
        px,
    };
    use smallvec::SmallVec;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{
        DeviceDetailsSnapshot, DiskDetails, InventorySnapshot, NetworkAdapter, ProcessorInfo,
        Snapshot,
    };
    use tl_model::table::Total;
    use tl_model::time::BootId;
    use tl_model::validity::Validity;
    use tl_ui::components::button::{TlButton, TlButtonKind, TlButtonSize};
    use tl_ui::components::kv_grid;
    use tl_ui::components::menu::TlMenu;
    use tl_ui::components::tooltip::DELAY_CONTROL;
    use tl_ui::pages::performance::actions::PerfShowMiniWindow;
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::page::{DevicePage, DevicePageFrame, PageWords, header_text};
    use tl_ui::pages::performance::{DeviceKey, DeviceKind};
    use tl_ui::theme::{current, metrics};

    fn valid(text: &str) -> Total<Arc<str>> {
        Total {
            value: Arc::from(text),
            validity: Validity::Valid,
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, display_name: &str) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from(display_name),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    fn strings(h: (SharedString, SharedString)) -> (String, String) {
        (h.0.to_string(), h.1.to_string())
    }

    #[test]
    fn page_header_strings() {
        let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
        let disk = entry(
            1,
            DeviceIdentity::Disk {
                major: 259,
                minor: 0,
                name: Arc::from("nvme0n1"),
                wwn_or_serial: None,
            },
            "nvme0n1",
        );
        let net = entry(
            2,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: 2,
                name: Arc::from("enp38s0"),
            },
            "enp38s0",
        );
        let gpu = entry(
            3,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:01:00.0"),
            },
            "NVIDIA GeForce RTX 4080",
        );
        let fan = entry(
            4,
            DeviceIdentity::Fan {
                chip: Arc::from("nct6799"),
                index: 2,
            },
            "fan2",
        );
        let memory = entry(5, DeviceIdentity::Memory, "Memory");

        let mut snap = Snapshot::empty(BootId(1), 1);
        // Nothing reported yet: the PERF-013 fallbacks.
        assert_eq!(
            strings(header_text(
                &PageWords::default(),
                DeviceKind::Cpu,
                0,
                &cpu,
                &snap
            )),
            ("Processor".into(), "Unknown processor".into())
        );
        assert_eq!(
            strings(header_text(
                &PageWords::default(),
                DeviceKind::Net,
                0,
                &net,
                &snap
            )),
            ("enp38s0".into(), "Unknown adapter".into())
        );
        assert_eq!(
            strings(header_text(
                &PageWords::default(),
                DeviceKind::Gpu,
                0,
                &entry(3, gpu.identity.clone(), ""),
                &snap
            )),
            ("Graphics adapter".into(), "Unknown GPU".into())
        );

        snap.inventory = Arc::new(InventorySnapshot {
            processor: ProcessorInfo {
                name: valid("AMD Ryzen 7 7800X3D 8-Core Processor"),
                ..ProcessorInfo::default()
            },
            network: Arc::from([NetworkAdapter {
                device: Some(DeviceIdx(2)),
                adapter_name: valid("Intel I350 Gigabit Network Connection"),
                ..NetworkAdapter::default()
            }]),
            ..InventorySnapshot::default()
        });
        snap.device_details = Arc::new(DeviceDetailsSnapshot {
            disks: Arc::from([(
                DeviceIdx(1),
                DiskDetails {
                    model: valid("WD_BLACK SN850X 1000GB"),
                    ..DiskDetails::default()
                },
            )]),
            ..DeviceDetailsSnapshot::default()
        });
        snap.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from([
                cpu.clone(),
                disk.clone(),
                net.clone(),
                gpu.clone(),
                fan.clone(),
                memory.clone(),
            ]),
        });

        let cases: [(DeviceKind, u16, &DeviceEntry, (&str, &str)); 8] = [
            (
                DeviceKind::Cpu,
                0,
                &cpu,
                ("Processor", "AMD Ryzen 7 7800X3D 8-Core Processor"),
            ),
            (DeviceKind::Memory, 0, &memory, ("Memory", "")),
            (
                DeviceKind::Disk,
                0,
                &disk,
                ("Disk 0", "WD_BLACK SN850X 1000GB (nvme0n1)"),
            ),
            (
                DeviceKind::Net,
                0,
                &net,
                ("enp38s0", "Intel I350 Gigabit Network Connection"),
            ),
            (
                DeviceKind::Gpu,
                0,
                &gpu,
                ("Graphics adapter", "NVIDIA GeForce RTX 4080"),
            ),
            (DeviceKind::Fan, 2, &fan, ("Fan 2", "nct6799")),
            (
                DeviceKind::AggregateDisk,
                0,
                &entry(6, disk.identity.clone(), ""),
                ("Aggregate storage", "1 disks"),
            ),
            (
                DeviceKind::AggregateGpu,
                0,
                &entry(7, gpu.identity.clone(), ""),
                ("Aggregate graphics", "1 adapters"),
            ),
        ];
        for (kind, n, e, (word, model)) in cases {
            assert_eq!(
                strings(header_text(&PageWords::default(), kind, n, e, &snap)),
                (word.to_owned(), model.to_owned()),
                "{kind:?}"
            );
        }
    }

    /// A page with `toggles` header buttons and a 240 px graph area.
    struct TestPage {
        key: DeviceKey,
        model: SharedString,
        toggles: usize,
        details: KeyValueModel,
        focus: FocusHandle,
        mini: Rc<Cell<u32>>,
    }

    impl DevicePage for TestPage {
        fn key(&self) -> &DeviceKey {
            &self.key
        }
        fn header(&self) -> (SharedString, SharedString) {
            ("Processor".into(), self.model.clone())
        }
        fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
            (0..self.toggles)
                .map(|i| {
                    TlButton::new(("toggle", i), TlButtonKind::Secondary)
                        .size(TlButtonSize::Sm)
                        .label("Overall")
                        .tooltip("Graph mode")
                        .into_any_element()
                })
                .collect()
        }
        fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
            div().w_full().h(px(240.)).into_any_element()
        }
        fn details(&self) -> &KeyValueModel {
            &self.details
        }
        fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
            SmallVec::new()
        }
        fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
            menu
        }
        fn copy_text(&self, _: &mut String) {}
        fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
            SmallVec::new()
        }
    }

    impl Focusable for TestPage {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for TestPage {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let m = *metrics(cx);
            let theme = current(cx).clone();
            div()
                .size_full()
                .track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &PerfShowMiniWindow, _, _| {
                    this.mini.set(this.mini.get() + 1);
                }))
                .child(
                    DevicePageFrame {
                        page: self,
                        m: &m,
                        theme: &theme,
                    }
                    .render(window, cx),
                )
        }
    }

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        kv_grid::init(cx);
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

    fn open<'a>(
        cx: &'a mut TestAppContext,
        width: f32,
        model: &str,
        toggles: usize,
    ) -> (&'a mut VisualTestContext, Rc<Cell<u32>>) {
        cx.update(setup);
        let mini = Rc::new(Cell::new(0));
        let model: SharedString = model.to_owned().into();
        let (_, vcx) = cx.add_window_view({
            let mini = Rc::clone(&mini);
            move |window, cx| {
                let page = TestPage {
                    key: DeviceKey(Arc::from("cpu")),
                    model,
                    toggles,
                    details: KeyValueModel::new(&["Speed"]),
                    focus: cx.focus_handle(),
                    mini,
                };
                window.focus(&page.focus, cx);
                page
            }
        });
        vcx.simulate_resize(gpui_kit::size(px(width), px(800.)));
        // Hover tooltips show only in the active window.
        vcx.update(|window, _| window.activate_window());
        draw(vcx);
        (vcx, mini)
    }

    #[gpui_kit::test]
    fn page_frame_header_geometry(cx: &mut TestAppContext) {
        let (cx, _) = open(cx, 1000., "AMD Ryzen 7 7800X3D 8-Core Processor", 2);
        let header = cx.debug_bounds("perf-page-header").expect("header");
        assert_eq!(header.size.height, px(36.), "header row height");
        assert_eq!(header.left(), px(16.), "page_pad left");
        assert_eq!(header.right(), px(1000. - 16.), "page_pad right");

        let a = cx.debug_bounds("perf-page-toggle-0").expect("toggle 0");
        let b = cx.debug_bounds("perf-page-toggle-1").expect("toggle 1");
        assert_eq!(b.right(), header.right(), "right-aligned");
        assert_eq!(b.left() - a.right(), px(8.), "sp4 apart");
        for (i, t) in [a, b].into_iter().enumerate() {
            let button = cx
                .debug_bounds(Box::leak(format!("TlButton-toggle-{i}").into_boxed_str()))
                .expect("button");
            assert_eq!(button.size.height, px(24.), "toggle {i} height");
            assert!(
                button.size.width >= px(122.),
                "toggle {i} width {:?}",
                button.size
            );
            assert_eq!(button.size.width, t.size.width, "toggle {i} fills its slot");
        }

        // sp4 under the header, then the graph area.
        let graph = cx.debug_bounds("perf-graph-area").expect("graph area");
        assert_eq!(graph.top(), header.bottom() + px(8.));
    }

    /// UI-PRF-058: the page header starts with a 20 px device icon, then the page word.
    #[gpui_kit::test]
    fn page_frame_header_has_the_device_icon(cx: &mut TestAppContext) {
        let (cx, _) = open(cx, 1000., "AMD Ryzen 7 7800X3D 8-Core Processor", 0);
        let header = cx.debug_bounds("perf-page-header").expect("header");
        let icon = cx.debug_bounds("perf-page-icon").expect("page icon");
        let word = cx.debug_bounds("perf-page-type").expect("page word");
        assert_eq!(icon.size.width, px(20.), "i_lg icon");
        assert_eq!(icon.size.height, px(20.));
        assert_eq!(icon.left(), header.left());
        assert_eq!(word.left() - icon.right(), px(8.), "sp4 gap");
    }

    fn click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>, count: usize) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: count,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: count,
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn graph_area_double_click_dispatches_mini_window(cx: &mut TestAppContext) {
        let (cx, mini) = open(cx, 1000., "Model", 0);
        let at = cx
            .debug_bounds("perf-graph-area")
            .expect("graph area")
            .center();
        click(cx, at, 1);
        assert_eq!(mini.get(), 0, "a single click dispatches nothing");
        click(cx, at, 2);
        assert_eq!(mini.get(), 1, "a double click dispatches once");
        // A click outside the graph area does nothing.
        let header = cx
            .debug_bounds("perf-page-header")
            .expect("header")
            .center();
        click(cx, header, 2);
        assert_eq!(mini.get(), 1);
    }

    #[test]
    fn header_model_truncates_with_tooltip() {
        // The default `TestAppContext` text system gives every glyph zero width, so use the real
        // one with the bundled fonts.
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let mut cx =
            TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            let report = tl_ui::text::fonts::load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        });
        let long = "AMD Ryzen Threadripper PRO 7995WX 96-Cores with an extra long model name \
                    that cannot fit";
        let (cx, _) = open(&mut cx, 500., long, 1);
        let model = cx.debug_bounds("perf-page-model").expect("model text");
        let toggle = cx.debug_bounds("perf-page-toggle-0").expect("toggle");
        assert!(
            model.right() <= toggle.left(),
            "the model stops at the toggles"
        );

        // The canvas found the text cut; the next frame attaches the tooltip.
        draw(cx);
        cx.simulate_mouse_move(model.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        draw(cx);
        assert!(
            cx.debug_bounds("TlTooltip-text").is_some(),
            "tooltip with the full text"
        );
    }

    #[test]
    fn header_model_that_fits_has_no_tooltip() {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let mut cx =
            TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            tl_ui::text::fonts::load_bundled_fonts(cx);
        });
        let (cx, _) = open(&mut cx, 1000., "Short", 1);
        let model = cx.debug_bounds("perf-page-model").expect("model text");
        draw(cx);
        cx.simulate_mouse_move(model.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        draw(cx);
        assert!(cx.debug_bounds("TlTooltip-text").is_none());
    }

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn build_page_placeholder_for_unknown_kind(cx: &mut TestAppContext) {
        use gpui_kit::{AnyView, AppContext as _};
        use tl_model::client::EngineClient;
        use tl_ui::demand::DemandAggregator;
        use tl_ui::hub::SnapshotHub;
        use tl_ui::pages::performance::DeviceRow;
        use tl_ui::pages::performance::page::build_page;
        use tl_ui::testing::fake_engine::FakeEngine;

        struct Host(AnyView);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(setup);
        // Fan and Battery have real pages now (o55.29), so use an aggregate with under two members.
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let key = DeviceKey(Arc::from("aggregate:disk"));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::AggregateDisk,
            idx: DeviceIdx(1),
            generation: 0,
            net_class: None,
            fan_connected: None,
        };
        let view = cx.update(|cx| build_page(&key, &row, &hub, &demand, cx));
        let (_, vcx) = cx.add_window_view(move |_, _| Host(view));
        draw(vcx);
        assert!(vcx.debug_bounds("perf-page-header").is_some());
        assert!(vcx.debug_bounds("perf-graph-area").is_some());
    }

    // ---- Words through tr, cached per strings version (R-132) ----

    /// Every Performance key the details, the grid and the header read, mapped to a marker.
    #[cfg(feature = "test-hooks")]
    const MARKERS: &str = "
perf-fallback-collecting = <collecting>
perf-fallback-restricted = <restricted>
perf-fallback-restricted-tooltip = <restricted-tip>
perf-fallback-needs-setup = <needs-setup>
perf-fallback-needs-setup-tooltip = <needs-setup-tip>
perf-fallback-not-supported = <not-supported>
perf-fallback-not-supported-tooltip = <not-supported-tip>
perf-fallback-not-reported = <not-reported>
perf-fallback-unavailable = <unavailable>
perf-fallback-could-not-read = <could-not-read {$reason}>
perf-fallback-unknown = <unknown>
perf-last-updated = <last-updated {$age}>
perf-details-heading = <details>
perf-header-processor = <processor>
perf-header-disk = <disk {$n}>
perf-header-disks-count = <{$k} disks>
perf-card-aggregate-storage = <aggregate-storage>
perf-unknown-processor = <unknown-processor>
perf-cpu-row-speed = <speed>
perf-cpu-row-sockets = <sockets>
perf-cpu-row-l1 = <l1>
perf-cpu-cell = <cpu {$n}>
perf-cpu-cell-tooltip = <core {$core} thread {$thread} {$mhz} mhz>
perf-cpu-cell-tooltip-noclock = <core {$core} thread {$thread}>
perf-legend-total = <total>
perf-legend-kernel = <kernel>
";

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn performance_words_follow_test_locale(cx: &mut TestAppContext) {
        use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
        use tl_ui::i18n::{init_strings, set_test_catalogue, tr_calls};
        use tl_ui::pages::performance::cpu_grid::{GridWords, cell_specs, heading_tooltip};
        use tl_ui::pages::performance::details::Fallback;
        use tl_ui::pages::performance::page::page_words;

        const SPEED: &str = "perf-cpu-row-speed";
        const SOCKETS: &str = "perf-cpu-row-sockets";
        const L1: &str = "perf-cpu-row-l1";
        let mut model = KeyValueModel::new(&[SPEED, SOCKETS, L1]);
        model.set_fallback(SPEED, Validity::Failed, Fallback::Unavailable, Some("EIO"));
        model.set_fallback(SOCKETS, Validity::Unsupported, Fallback::NeedsSetup, None);
        cx.update(|cx| {
            init_strings(cx, "en-US");
            assert!(model.refresh_strings(cx));
        });
        assert_eq!(model.rows[0].value, "Unavailable");
        assert_eq!(
            model.rows[0].tooltip.as_deref(),
            Some("Could not read: EIO")
        );

        cx.update(|cx| {
            set_test_catalogue(cx, MARKERS);
            let before = tr_calls();
            assert!(model.refresh_strings(cx), "a new strings version");
            let rebuilt = tr_calls();
            assert!(rebuilt > before);
            assert!(!model.refresh_strings(cx), "translated once");
            assert_eq!(tr_calls(), rebuilt, "no tr calls the second time");
        });
        let shown = |model: &KeyValueModel, i: usize| {
            let r = &model.rows[i];
            (
                r.value.to_string(),
                r.tooltip.as_ref().map(|t| t.to_string()),
            )
        };
        assert_eq!(
            shown(&model, 0),
            ("<unavailable>".into(), Some("<could-not-read EIO>".into()))
        );
        assert_eq!(
            shown(&model, 1),
            ("<needs-setup>".into(), Some("<needs-setup-tip>".into()))
        );
        assert_eq!(shown(&model, 2), ("<collecting>".into(), None));
        let labels: Vec<String> = model.to_kv_rows().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["<speed>", "<sockets>", "<l1>"]);

        model.set_text(L1, "1 MiB");
        model.set_stale(L1, 5_000_000_000);
        assert_eq!(model.rows[2].tooltip.as_deref(), Some("<last-updated 5s>"));
        model.set_fallback(L1, Validity::Restricted, Fallback::NotSupported, None);
        assert_eq!(
            shown(&model, 2),
            ("<restricted>".into(), Some("<restricted-tip>".into()))
        );

        // CPU grid: headings, tooltips and legend words.
        let words = cx.update(|cx| GridWords::new(cx));
        let topology = CpuTopology {
            online: {
                let mut set = CpuSet::empty();
                (0..4).for_each(|id| set.insert(id));
                set
            },
            cores: (0..2)
                .map(|c| CoreInfo {
                    id: c,
                    logical: smallvec::smallvec![c, c + 2],
                    l3_group: 0,
                    kind: CoreKind::Standard,
                })
                .collect(),
        };
        let devices: Vec<DeviceIdx> = (0..4).map(|id| DeviceIdx(100 + id)).collect();
        let mut cells = Vec::new();
        cell_specs(&words, &topology, &devices, false, &mut cells);
        assert_eq!(cells[3].heading.as_ref(), "<cpu 3>");
        assert_eq!(cells[3].tooltip.as_deref(), Some("<core 1 thread 1>"));
        let mut tip = String::new();
        heading_tooltip(&words, 1, 0, Some(4200), &mut tip);
        assert_eq!(tip, "<core 1 thread 0 4200 mhz>");
        assert_eq!(
            (words.total.as_ref(), words.kernel.as_ref()),
            ("<total>", "<kernel>")
        );

        // Page header words and the Details heading.
        let disk = entry(
            1,
            DeviceIdentity::Disk {
                major: 259,
                minor: 0,
                name: Arc::from("nvme0n1"),
                wwn_or_serial: None,
            },
            "nvme0n1",
        );
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![disk.clone()]),
        });
        cx.update(|cx| {
            let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
            let w = page_words(cx).clone();
            assert_eq!(w.details.as_ref(), "<details>");
            assert_eq!(
                strings(header_text(&w, DeviceKind::Cpu, 0, &cpu, &snap)),
                ("<processor>".into(), "<unknown-processor>".into())
            );
            assert_eq!(
                strings(header_text(&w, DeviceKind::Disk, 1, &disk, &snap)),
                ("<disk 1>".into(), "nvme0n1".into())
            );
            assert_eq!(
                strings(header_text(&w, DeviceKind::AggregateDisk, 0, &disk, &snap)),
                ("<aggregate-storage>".into(), "<1 disks>".into())
            );
        });
    }

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn page_menu_tray_item_position(cx: &mut TestAppContext) {
        use tl_ui::components::menu::MenuEntryKind;
        use tl_ui::i18n::init_strings;
        use tl_ui::pages::performance::actions::PerfAddTrayGraph;
        use tl_ui::pages::performance::menu::{PageMenuCtx, page_menu};

        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
        });
        let (page, vcx) = cx.add_window_view(|_, cx| TestPage {
            key: DeviceKey(Arc::from("cpu")),
            model: "AMD Ryzen 7 7800X3D 8-Core Processor".into(),
            toggles: 1,
            details: KeyValueModel::new(&["perf-cpu-row-speed"]),
            focus: cx.focus_handle(),
            mini: Rc::default(),
        });
        let tail = |vcx: &mut VisualTestContext, tray: Option<bool>| {
            vcx.update(|window, cx| {
                let focus = page.read(cx).focus.clone();
                let ctx = PageMenuCtx {
                    range_s: 60,
                    summary_on: false,
                    ranges_long_enabled: true,
                    view_types: Some(&[(DeviceKind::Cpu, true)]),
                    clicked_value: false,
                    tray_available: tray,
                    cell_engine: None,
                };
                let menu = TlMenu::build(window, cx, focus, |menu, w, cx| {
                    page.update(cx, |p, cx| page_menu(menu, p, &ctx, w, cx))
                });
                let list = menu.read(cx).entries_for_test(window);
                let n = list.len();
                list[n - 2..]
                    .iter()
                    .map(|e| {
                        (
                            e.kind,
                            e.label.as_ref().map(|l| l.to_string()).unwrap_or_default(),
                            e.tooltip.as_ref().map(|t| t.to_string()),
                            e.action.as_ref().map(|a| {
                                a.partial_eq(&PerfAddTrayGraph {
                                    source: DeviceKey(Arc::from("cpu")),
                                })
                            }),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        };
        let on = tail(vcx, Some(true));
        assert_eq!(on[0].0, MenuEntryKind::Submenu);
        assert_eq!(on[0].1, "View");
        assert_eq!(on[1].0, MenuEntryKind::Item);
        assert_eq!(on[1].1, "Add tray icon graph");
        assert_eq!(on[1].3, Some(true));
        let off = tail(vcx, Some(false));
        assert_eq!(off[1].0, MenuEntryKind::Disabled);
        assert_eq!(off[1].2.as_deref(), Some("No system tray available"));
    }

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn performance_frames_do_no_tr_calls(cx: &mut TestAppContext) {
        use tl_ui::i18n::{init_strings, set_test_catalogue, tr_calls};

        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
        });
        let (page, vcx) = cx.add_window_view(|_, cx| TestPage {
            key: DeviceKey(Arc::from("cpu")),
            model: "AMD Ryzen 7 7800X3D 8-Core Processor".into(),
            toggles: 1,
            details: KeyValueModel::new(&["perf-cpu-row-speed"]),
            focus: cx.focus_handle(),
            mini: Rc::default(),
        });
        let frame = |vcx: &mut VisualTestContext| {
            vcx.update(|window, cx| {
                window.refresh();
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        };
        let frames = |vcx: &mut VisualTestContext| {
            let before = tr_calls();
            for _ in 0..60 {
                frame(vcx);
            }
            tr_calls() - before
        };
        draw(vcx);
        assert_eq!(frames(vcx), 0, "60 unchanged frames");

        let before = tr_calls();
        vcx.update(|_, cx| set_test_catalogue(cx, MARKERS));
        frame(vcx);
        assert!(tr_calls() > before, "the new strings are read");
        assert_eq!(frames(vcx), 0, "60 frames after the switch");
        let label = vcx.update(|_, cx| {
            page.read(cx)
                .details
                .to_kv_rows()
                .next()
                .map(|r| r.label.to_string())
        });
        assert_eq!(label.as_deref(), Some("<speed>"));
    }
}

/// The Performance Actions and key bindings (E24-002, canonical table (b), PERF-010, R-128).
mod perf_keys {
    use std::sync::Arc;

    use gpui_kit::{Action, App, KeyContext, Keystroke, TestAppContext};
    use tl_ui::actions::{SelectTab, bind_keys_global};
    use tl_ui::pages::performance::actions::*;
    use tl_ui::pages::performance::{DeviceKey, DeviceKind};

    /// The Action the keymap resolves `keys` to with `contexts` as the stack (outermost first).
    fn resolve(cx: &App, keys: &str, contexts: &[&str]) -> Option<Box<dyn Action>> {
        let input: Vec<Keystroke> = keys
            .split(' ')
            .map(|k| Keystroke::parse(k).unwrap_or_else(|e| panic!("keystroke {k}: {e:?}")))
            .collect();
        let stack: Vec<KeyContext> = contexts
            .iter()
            .map(|c| KeyContext::parse(c).expect("key context"))
            .collect();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let (bindings, _) = keymap.bindings_for_input(&input, &stack);
        bindings.first().map(|b| b.action().boxed_clone())
    }

    fn assert_resolves(cx: &App, keys: &str, contexts: &[&str], expected: &dyn Action) {
        let got = resolve(cx, keys, contexts);
        assert!(
            got.as_ref().is_some_and(|a| a.partial_eq(expected)),
            "{keys} in {contexts:?}: expected {expected:?}, got {got:?}"
        );
    }

    /// Bound the way `TasklensUi::init` binds them: Global, Table, then Performance.
    fn bound(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys_global(cx);
            tl_ui::table::actions::bind_keys_table(cx);
            bind_keys_performance(cx);
        });
    }

    #[gpui_kit::test]
    fn performance_keys_match_table_b(cx: &mut TestAppContext) {
        bound(cx);
        let stack = &["Global", "Performance"];
        let mut rows: Vec<(String, Box<dyn Action>)> = (0..8u8)
            .map(|i| {
                let action: Box<dyn Action> = Box::new(PerfSelectVisibleDevice { index: i });
                (format!("f{}", i + 1), action)
            })
            .collect();
        let fixed: Vec<(&str, Box<dyn Action>)> = vec![
            ("up", Box::new(PerfSelectPrevDevice)),
            ("down", Box::new(PerfSelectNextDevice)),
            ("home", Box::new(PerfSelectFirstDevice)),
            ("end", Box::new(PerfSelectLastDevice)),
            ("f9", Box::new(PerfToggleSidebar)),
            ("ctrl-t", Box::new(PerfToggleSidebar)),
            ("ctrl-m", Box::new(PerfToggleSummaryMode)),
            ("ctrl-c", Box::new(PerfCopyDeviceInfo)),
            ("alt-up", Box::new(PerfMoveFocusedDevice { delta: -1 })),
            ("alt-down", Box::new(PerfMoveFocusedDevice { delta: 1 })),
        ];
        rows.extend(fixed.into_iter().map(|(k, a)| (k.to_string(), a)));

        cx.read(|cx| {
            for (keys, action) in &rows {
                assert_resolves(cx, keys, stack, action.as_ref());
            }
            // The Table copy is bound too, but not in this context.
            assert_resolves(cx, "ctrl-c", stack, &PerfCopyDeviceInfo);

            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let f3 = keymap
                .bindings_for_action(&PerfSelectVisibleDevice { index: 2 })
                .any(|b| {
                    b.keystrokes().len() == 1
                        && b.keystrokes()[0].unparse() == "f3"
                        && b.predicate()
                            .is_some_and(|p| p.to_string() == "Performance")
                });
            assert!(f3, "f3 in Performance selects visible device 2");
        });
    }

    #[gpui_kit::test]
    fn summary_context_swallows_tab_keys(cx: &mut TestAppContext) {
        bound(cx);
        let summary = &["Global", "Performance", "PerformanceSummary"];
        cx.read(|cx| {
            for i in 1..=6 {
                assert_resolves(cx, &format!("ctrl-{i}"), summary, &NoOp);
                assert_resolves(cx, &format!("alt-{i}"), summary, &NoOp);
            }
            assert_resolves(cx, "ctrl-tab", summary, &NoOp);
            assert_resolves(cx, "ctrl-shift-tab", summary, &NoOp);
            assert_resolves(cx, "escape", summary, &PerfToggleSummaryMode);
            assert_resolves(cx, "ctrl-m", summary, &PerfToggleSummaryMode);

            assert_resolves(cx, "ctrl-2", &["Global"], &SelectTab(1));
            assert_resolves(cx, "ctrl-2", &["Global", "Performance"], &SelectTab(1));
        });
    }

    #[gpui_kit::test]
    fn perf_actions_in_tasklens_namespace(cx: &mut TestAppContext) {
        let key = DeviceKey(Arc::from("disk:wwn-1"));
        let all: Vec<Box<dyn Action>> = vec![
            Box::new(PerfSelectPrevDevice),
            Box::new(PerfSelectNextDevice),
            Box::new(PerfSelectFirstDevice),
            Box::new(PerfSelectLastDevice),
            Box::new(PerfToggleSidebar),
            Box::new(PerfToggleSummaryMode),
            Box::new(PerfCopyDeviceInfo),
            Box::new(PerfCopyValue),
            Box::new(PerfEnterEditMode),
            Box::new(PerfExitEditMode),
            Box::new(PerfEnableAllPages),
            Box::new(PerfDisableAllPages),
            Box::new(PerfResetPagesToDefault),
            Box::new(PerfShowMiniWindow),
            Box::new(PerfToggleCpuKernel),
            Box::new(PerfToggleMemoryComposition),
            Box::new(PerfToggleMemorySwapGraph),
            Box::new(PerfOpenNetworkSettings),
            Box::new(PerfSelectDevice { key: key.clone() }),
            Box::new(PerfSelectVisibleDevice { index: 0 }),
            Box::new(PerfSetGraphRange { secs: 60 }),
            Box::new(PerfJumpToType {
                kind: DeviceKind::Disk,
            }),
            Box::new(PerfSetPageVisible {
                key: key.clone(),
                visible: true,
            }),
            Box::new(PerfMoveDevice {
                key: key.clone(),
                before: None,
            }),
            Box::new(PerfMoveFocusedDevice { delta: 1 }),
            Box::new(PerfAddTrayGraph {
                source: key.clone(),
            }),
            Box::new(PerfSetCpuGraphMode {
                mode: CpuGraphMode::PerCore,
            }),
            Box::new(PerfSetCpuSecondaryGraph {
                choice: CpuSecondaryGraph::Clock,
            }),
            Box::new(PerfShowSmart { key: key.clone() }),
            Box::new(PerfSmartRefresh { key: key.clone() }),
            Box::new(PerfSmartReadSmartctl { key: key.clone() }),
            Box::new(PerfEject { key: key.clone() }),
            Box::new(PerfToggleGpuEngine {
                key: key.clone(),
                engine: Arc::from("render"),
            }),
            Box::new(PerfToggleGpuOptionalGraph {
                key: key.clone(),
                which: GpuOptionalGraph::Clocks,
            }),
            Box::new(PerfRenameFan { key: key.clone() }),
            Box::new(PerfSetFanTempSensor {
                key: key.clone(),
                sensor: None,
            }),
            Box::new(PerfFetchPublicIp { key }),
            Box::new(NoOp),
        ];
        assert_eq!(all.len(), 38);
        cx.read(|cx| {
            let names = cx.all_action_names();
            for action in &all {
                let name = action.name();
                assert!(
                    name.starts_with("tasklens::Perf") || name == "tasklens::NoOp",
                    "{name}"
                );
                assert!(names.contains(&name), "{name} is registered");
            }
        });
    }

    #[gpui_kit::test]
    fn graph_range_round_trips(cx: &mut TestAppContext) {
        let action = PerfSetGraphRange { secs: 300 };
        let json = serde_json::json!({ "secs": 300 });
        cx.read(|cx| {
            let built = cx
                .build_action(action.name(), Some(json))
                .expect("build PerfSetGraphRange");
            assert!(built.partial_eq(&action), "got {built:?}");
        });
    }

    /// Every mirror variant converts to the model enum and back unchanged.
    #[test]
    fn graph_mirrors_round_trip() {
        for m in [
            CpuGraphMode::Overall,
            CpuGraphMode::PerCore,
            CpuGraphMode::AllCoresOverlaid,
            CpuGraphMode::AllCoresStacked,
        ] {
            assert_eq!(CpuGraphMode::from(CpuGraphModeJson::from(m)), m);
        }
        for m in [
            CpuSecondaryGraph::None,
            CpuSecondaryGraph::Power,
            CpuSecondaryGraph::Clock,
            CpuSecondaryGraph::Temperature,
        ] {
            assert_eq!(CpuSecondaryGraph::from(CpuSecondaryGraphJson::from(m)), m);
        }
        for m in [GpuOptionalGraph::Power, GpuOptionalGraph::Clocks] {
            assert_eq!(GpuOptionalGraph::from(GpuOptionalGraphJson::from(m)), m);
        }
    }

    /// Builds the Action of `want` from `json`, as a keymap entry does, and checks it equals
    /// `want`.
    fn assert_builds(cx: &mut TestAppContext, json: serde_json::Value, want: &dyn Action) {
        cx.read(|cx| {
            let built = cx
                .build_action(want.name(), Some(json))
                .unwrap_or_else(|e| panic!("build {}: {e}", want.name()));
            assert!(built.partial_eq(want), "got {built:?}");
        });
    }

    /// The three graph Actions with a `tl_model::settings` field load from JSON (crw.31).
    #[gpui_kit::test]
    fn graph_actions_load_from_keymap_json(cx: &mut TestAppContext) {
        let key = DeviceKey(Arc::from("gpu:GPU-0"));
        assert_builds(
            cx,
            serde_json::json!({ "mode": "per_core" }),
            &PerfSetCpuGraphMode {
                mode: CpuGraphMode::PerCore,
            },
        );
        assert_builds(
            cx,
            serde_json::json!({ "choice": "none" }),
            &PerfSetCpuSecondaryGraph {
                choice: CpuSecondaryGraph::None,
            },
        );
        assert_builds(
            cx,
            serde_json::json!({ "key": "gpu:GPU-0", "which": "clocks" }),
            &PerfToggleGpuOptionalGraph {
                key,
                which: GpuOptionalGraph::Clocks,
            },
        );
        cx.read(|cx| {
            assert!(
                cx.build_action(
                    "tasklens::PerfSetCpuGraphMode",
                    Some(serde_json::json!({ "mode": "PerCore" })),
                )
                .is_err(),
                "the JSON spelling follows the settings file"
            );
        });
    }
}

// ---- The Performance tab: split, sidebar toggle, page switching (E24-A1) ----

#[cfg(feature = "test-hooks")]
mod tab {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, MouseButton, Render,
        TestAppContext, VisualTestContext, Window, div, point, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::components::kv_grid;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::actions::PerfToggleSidebar;
    use tl_ui::pages::performance::cpu::CpuPage;
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::{DeviceKey, PAGE_PANEL_ID, PerformancePage, SIDEBAR_PANEL_ID};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    /// `n` logical CPUs, two per core.
    fn topology(n: u16) -> CpuTopology {
        let half = n / 2;
        let mut online = CpuSet::empty();
        for id in 0..n {
            online.insert(id);
        }
        CpuTopology {
            online,
            cores: (0..half)
                .map(|c| CoreInfo {
                    id: c,
                    logical: smallvec::smallvec![c, c + half],
                    l3_group: 0,
                    kind: CoreKind::Standard,
                })
                .collect(),
        }
    }

    struct Rig<'a> {
        page: Entity<PerformancePage>,
        demand: Entity<DemandAggregator>,
        /// `nav_width` of every persisted state.
        persisted: Rc<RefCell<Vec<u16>>>,
        vcx: &'a mut VisualTestContext,
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

    /// Default settings with the Overview card hidden (D-022), so the CPU page is the first page.
    fn hidden_overview() -> Settings {
        let mut s = Settings::default();
        s.performance.device_overrides.insert(
            tl_ui::pages::performance::overview::OVERVIEW_KEY.to_owned(),
            tl_model::settings::DeviceVisibility::Hide,
        );
        s
    }

    /// Every Global the tab needs over a fake catalog with CPU (`cpus` logical CPUs) and Memory.
    fn install(cx: &mut TestAppContext, state: UiState, cpus: u16) -> Rc<RefCell<Vec<u16>>> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            let mut entries = vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
            ];
            entries.extend((0..cpus).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(entries),
            });
            Arc::make_mut(&mut s.system).cpu_topology = Arc::new(topology(cpus));
        });
        let client: Arc<dyn EngineClient> = engine;
        let persisted: Rc<RefCell<Vec<u16>>> = Rc::default();
        let seen = Rc::clone(&persisted);
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
            cx.set_global(BootSettings {
                settings: hidden_overview(),
                state,
                persist: Some(Box::new(move |_, state| {
                    seen.borrow_mut().push(state.performance.nav_width)
                })),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        persisted
    }

    /// The tab as the window root, `width` × `height`, the sidebar focused.
    fn open(
        cx: &mut TestAppContext,
        state: UiState,
        cpus: u16,
        width: f32,
        height: f32,
    ) -> Rig<'_> {
        let persisted = install(cx, state, cpus);
        let client = cx.read(|cx| Arc::clone(&cx.global::<EngineGlobal>().0));
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let d = demand.clone();
        let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(hub, d, cx));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(width), px(height)));
        let focus = vcx.update(|_, cx| page.read(cx).primary_focus());
        vcx.update(|window, cx| focus.focus(window, cx));
        draw(vcx);
        Rig {
            page,
            demand,
            persisted,
            vcx,
        }
    }

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    fn cpu_page(r: &mut Rig) -> Entity<CpuPage> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.read(cx)
                .page(&key(DeviceKey::CPU))
                .expect("CPU page built")
                .clone()
                .downcast::<CpuPage>()
                .expect("a CpuPage")
        })
    }

    #[gpui_kit::test]
    fn nav_width_persists_and_clamps(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.performance.nav_width = 500;
        let r = open(cx, state, 16, 1280., 800.);
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(sidebar.size.width, px(400.), "500 is clamped to 400");

        let boundary = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").left();
        let y = px(400.);
        r.vcx.simulate_mouse_down(
            point(boundary - px(2.), y),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        r.vcx.simulate_mouse_move(
            point(boundary - px(20.), y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        draw(r.vcx);
        r.vcx.simulate_mouse_move(
            point(px(300.), y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        draw(r.vcx);
        r.vcx
            .simulate_mouse_up(point(px(300.), y), MouseButton::Left, Modifiers::none());
        // tl-app debounces the write by 500 ms; the persist call itself comes at once.
        r.vcx.executor().advance_clock(Duration::from_millis(600));
        draw(r.vcx);

        assert_eq!(r.persisted.borrow().last(), Some(&300));
        let page = r.page.clone();
        let (saved, nav) = r.vcx.update(|_, cx| {
            (
                TlSettings::state(cx).performance.nav_width,
                page.read(cx).nav_width(),
            )
        });
        assert_eq!((saved, nav), (300, 300));
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(sidebar.size.width, px(300.));
    }

    fn sidebar_visible(r: &mut Rig) -> bool {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).sidebar_visible())
    }

    /// Dispatches `PerfToggleSidebar` and lets the `m_base` animation finish.
    fn toggle(r: &mut Rig) {
        r.vcx.dispatch_action(PerfToggleSidebar);
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
    }

    #[gpui_kit::test]
    fn toggle_sidebar_not_persisted(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let before = r.persisted.borrow().len();
        let state = r.vcx.update(|_, cx| TlSettings::state(cx).clone());

        toggle(&mut r);
        assert!(!sidebar_visible(&mut r), "hidden");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert_eq!((page.left(), page.size.width), (px(0.), px(1280.)));

        toggle(&mut r);
        assert!(sidebar_visible(&mut r), "shown again");
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(sidebar.size.width, px(252.));

        assert_eq!(r.persisted.borrow().len(), before, "nothing persisted");
        let after = r.vcx.update(|_, cx| TlSettings::state(cx).clone());
        assert_eq!(after, state);
    }

    #[gpui_kit::test]
    fn toggle_sidebar_animates_width(cx: &mut TestAppContext) {
        let r = open(cx, UiState::default(), 16, 1280., 800.);
        r.vcx.dispatch_action(PerfToggleSidebar);
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(100));
        draw(r.vcx);
        // Before `m_base` ends the sidebar still draws, in the clipping box left of the page.
        // How far the width moved depends on the frame clock, which the test executor does
        // not drive in every feature set, so only the bounds are checked.
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert!(page.left() < px(252.), "page left {:?}", page.left());
        r.vcx.executor().advance_clock(Duration::from_millis(150));
        draw(r.vcx);
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert_eq!(page.left(), px(0.));
    }

    #[gpui_kit::test]
    fn performance_layout_regions(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page panel");
        assert_eq!(sidebar.left(), px(0.));
        assert_eq!(sidebar.size.width, px(252.));
        // The kit's 1 px divider straddles the panel boundary; it takes no width of its own.
        assert_eq!(page.left(), sidebar.right());
        assert!(page.size.width >= px(400.), "page {:?}", page.size);
        assert_eq!(page.right(), px(1280.));

        let cpu = cpu_page(&mut r);
        let word = r.vcx.update(|_, cx| cpu.read(cx).header().0.to_string());
        assert_eq!(word, "Processor");
        let header = r.vcx.debug_bounds("perf-page-header").expect("header");
        assert!(header.left() >= page.left());
    }

    #[gpui_kit::test]
    fn selection_creates_page_once_and_switches_demand(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let page = r.page.clone();
        let demand = r.demand.clone();
        let count = |r: &mut Rig| r.vcx.update(|_, cx| page.read(cx).page_count());
        let cpu_fast = |r: &mut Rig| r.vcx.update(|_, cx| demand.read(cx).merged().cpu_fast);
        assert_eq!(count(&mut r), 1, "the first selection is built");
        assert!(cpu_fast(&mut r), "the CPU page's demand part");
        let cpu = cpu_page(&mut r);

        let sidebar = r.vcx.update(|_, cx| page.read(cx).sidebar().clone());
        let select = |r: &mut Rig, k: &str| {
            let k = key(k);
            r.vcx
                .update(|_, cx| sidebar.update(cx, |s, cx| s.select(k, cx)));
            draw(r.vcx);
        };
        select(&mut r, DeviceKey::MEMORY);
        assert_eq!(count(&mut r), 2);
        let selected = r.vcx.update(|_, cx| page.read(cx).selected().cloned());
        assert_eq!(selected, Some(key(DeviceKey::MEMORY)));
        assert!(!cpu_fast(&mut r), "the CPU page withdrew its demand part");
        assert!(r.vcx.debug_bounds("perf-page-header").is_some());

        select(&mut r, DeviceKey::CPU);
        assert_eq!(count(&mut r), 2, "no new page");
        assert_eq!(cpu_page(&mut r), cpu, "the same CPU page entity");
        assert!(cpu_fast(&mut r), "the CPU page is active again");
    }

    #[gpui_kit::test]
    fn focus_order_sidebar_then_page(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let page = r.page.clone();
        let cpu = cpu_page(&mut r);
        let (order, primary, cpu_order): (Vec<FocusHandle>, FocusHandle, Vec<FocusHandle>) =
            r.vcx.update(|_, cx| {
                let p = page.read(cx);
                (
                    p.focus_order().into_vec(),
                    p.primary_focus(),
                    cpu.read(cx).focus_order().into_vec(),
                )
            });
        assert_eq!(order[0], primary);
        assert_eq!(&order[1..], &cpu_order[..]);
        let focused = r.vcx.update(|window, _| primary.is_focused(window));
        assert!(focused);
    }

    struct FakeProcesses(FocusHandle);

    impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

    impl Render for FakeProcesses {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            div().track_focus(&self.0).size_full()
        }
    }

    impl tl_ui::window::main::ShellPage for FakeProcesses {
        fn primary_focus(&self) -> FocusHandle {
            self.0.clone()
        }
    }

    #[gpui_kit::test]
    fn performance_tab_initial_focus_is_sidebar(cx: &mut TestAppContext) {
        use tl_model::command::Tab;
        use tl_ui::actions::SelectTab;
        use tl_ui::app::StartOptions;
        use tl_ui::demand::GlobalDemand;
        use tl_ui::hub::GlobalHub;
        use tl_ui::window::main::{
            self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
        };
        use tl_ui::window::titlebar::TitleBarProbe;

        install(cx, UiState::default(), 16);
        cx.update(|cx| {
            let client = Arc::clone(&cx.global::<EngineGlobal>().0);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            tl_ui::components::toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            cx.set_global(StartOptions::default());
            set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
                let page = cx.new(|cx| FakeProcesses(cx.focus_handle().tab_stop(true)));
                PageSlot::new(page, cx)
            });
            cx.set_global(TitleBarProbe {
                client: Some(true),
                ..TitleBarProbe::default()
            });
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let vcx = &mut VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(vcx);

        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let built = |vcx: &mut VisualTestContext| {
            vcx.read_entity(&main, |m, _| {
                m.page_entity::<PerformancePage>(Tab::Performance)
            })
        };
        assert!(built(vcx).is_none(), "built on first visit");
        vcx.dispatch_action(SelectTab(Tab::Performance as usize));
        draw(vcx);
        let page = built(vcx).expect("PerformancePage built");
        let focus = vcx.update(|_, cx| page.read(cx).primary_focus());
        assert!(vcx.update(|window, _| focus.is_focused(window)));
        assert!(vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
    }

    #[cfg(feature = "bench")]
    #[gpui_kit::test]
    fn bench_show_both_behind_bench_feature(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::cpu::OVERALL_ID;
        use tl_ui::pages::performance::cpu_grid::GRID_ID;

        let mut r = open(cx, UiState::default(), 32, 1600., 1000.);
        let cpu = cpu_page(&mut r);
        r.vcx.update(|_, cx| {
            cpu.update(cx, |p, cx| {
                p.bench_show_both = true;
                cx.notify();
            })
        });
        draw(r.vcx);
        let overall = r.vcx.debug_bounds(OVERALL_ID).expect("overall chart");
        let grid = r.vcx.debug_bounds(GRID_ID).expect("grid");
        assert_eq!(overall.size.height, px(300.));
        assert!(grid.top() > overall.bottom(), "grid below the chart");
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert!(overall.left() >= page.left() && grid.right() <= page.right());
    }
}

// ---- Graph summary mode (E24-015) ----

#[cfg(feature = "test-hooks")]
mod summary {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement, Modifiers,
        MouseButton, MouseDownEvent, MouseUpEvent, Render, TestAppContext, VisualTestContext,
        Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::demand::Demand;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{
        AppearanceSettings, DeviceVisibility, Settings, ThemeSettings, UiState,
    };
    use tl_model::snapshot::SeriesSet;
    use tl_model::system::Metric;
    use tl_model::validity::Validity;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::components::kv_grid;
    use tl_ui::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui::components::motion::{M_BASE, M_FAST};
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::memory::MemoryPage;
    use tl_ui::pages::performance::overview::OVERVIEW_KEY;
    use tl_ui::pages::performance::summary::{MARKER_ID, SummaryView, tile_grid};
    use tl_ui::pages::performance::{DeviceKey, PerformancePage, SIDEBAR_PANEL_ID};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
    };
    use tl_ui::window::titlebar::TitleBarProbe;

    const GPU: &str = "gpu:pci:0000:2d:00.0";

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    fn disk(idx: u16, name: &str) -> DeviceEntry {
        let identity = DeviceIdentity::Disk {
            major: 8,
            minor: idx.into(),
            name: Arc::from(name),
            wwn_or_serial: None,
        };
        entry(idx, identity, None)
    }

    fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
        let identity = DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: idx.into(),
            name: Arc::from(name),
        };
        entry(idx, identity, Some(kind))
    }

    /// Nine visible cards: CPU, Memory, a GPU, four disks, a wired interface and a VPN tunnel.
    fn catalog() -> Vec<DeviceEntry> {
        let gpu = DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: Arc::from("0000:2d:00.0"),
        };
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            entry(2, gpu, None),
            disk(3, "sda"),
            disk(4, "sdb"),
            disk(5, "sdc"),
            disk(6, "sdd"),
            net(7, "enp38s0", NetKind::Wired),
            net(8, "tailscale0", NetKind::VpnTunnel),
        ]
    }

    struct FakeProcesses(FocusHandle);

    impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

    impl Render for FakeProcesses {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            div().track_focus(&self.0).size_full()
        }
    }

    impl shell::ShellPage for FakeProcesses {
        fn primary_focus(&self) -> FocusHandle {
            self.0.clone()
        }
    }

    struct Rig<'a> {
        main: Entity<MainWindow>,
        page: Entity<PerformancePage>,
        demand: Entity<DemandAggregator>,
        hub: Entity<SnapshotHub>,
        engine: Arc<FakeEngine>,
        /// The last value handed to the `state.toml` persistence callback.
        persisted_state: Rc<RefCell<Option<UiState>>>,
        vcx: &'a mut VisualTestContext,
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

    /// The main window, `width` × `height`, on the Performance tab with the sidebar focused.
    fn open(cx: &mut TestAppContext, width: f32, height: f32, reduce_motion: bool) -> Rig<'_> {
        open_with(cx, width, height, reduce_motion, Settings::default())
    }

    /// [`open`] with `settings`. The Overview card is hidden unless `settings` has an entry
    /// for it (D-022).
    fn open_with(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
        reduce_motion: bool,
        mut settings: Settings,
    ) -> Rig<'_> {
        // Instant tab switches (E20-021): frame counts here must not see the switch fade.
        settings.motion.tab_switch_ms = 0;
        settings
            .performance
            .device_overrides
            .entry(OVERVIEW_KEY.to_owned())
            .or_insert(DeviceVisibility::Hide);
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(catalog()),
            });
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        let persisted_state = Rc::new(RefCell::new(None));
        let persisted = Rc::clone(&persisted_state);
        let (hub, demand) = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(reduce_motion);
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
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: Some(Box::new(move |_, state| {
                    *persisted.borrow_mut() = Some(state.clone());
                })),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub.clone()));
            cx.set_global(GlobalDemand(demand.clone()));
            tl_ui::components::toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            tl_ui::components::menu::bind_keys(cx);
            tl_ui::pages::performance::actions::bind_keys_performance(cx);
            cx.set_global(StartOptions::default());
            set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
                let page = cx.new(|cx| FakeProcesses(cx.focus_handle().tab_stop(true)));
                PageSlot::new(page, cx)
            });
            cx.set_global(TitleBarProbe {
                client: Some(true),
                ..TitleBarProbe::default()
            });
            tl_ui::window::menubar::init(cx);
            shell::init(cx);
            (hub, demand)
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(width), px(height)));
        vcx.update(|window, _| window.activate_window());
        draw(vcx);
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        vcx.dispatch_action(tl_ui::actions::SelectTab(Tab::Performance as usize));
        draw(vcx);
        // Let the tab switch fade (`motion.tab_switch_ms`, timed on the test clock) end, or it
        // asks for a frame on every later draw.
        let tab_ms = vcx.update(|_, cx| TlSettings::settings(cx).motion.tab_switch_ms);
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(u64::from(tab_ms)));
        draw(vcx);
        let page = vcx
            .read_entity(&main, |m, _| {
                m.page_entity::<PerformancePage>(Tab::Performance)
            })
            .expect("PerformancePage built");
        Rig {
            main,
            page,
            demand,
            hub,
            engine,
            persisted_state,
            vcx,
        }
    }

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    fn summary(r: &mut Rig) -> Option<Entity<SummaryView>> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).summary().cloned())
    }

    /// Ctrl+M from the sidebar, then the `m_fast` fade-out when animations are on.
    fn enter(r: &mut Rig) -> Entity<SummaryView> {
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        if summary(r).is_none() {
            r.vcx.executor().advance_clock(M_FAST);
            draw(r.vcx);
        }
        summary(r).expect("summary mode on")
    }

    fn pending(r: &mut Rig) -> bool {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).summary_pending())
    }

    /// Frame callbacks queued per frame, for `n` frames.
    fn frames(r: &mut Rig, n: usize) -> Vec<usize> {
        (0..n)
            .map(|_| {
                r.vcx.run_until_parked();
                r.vcx.update(|window, cx| {
                    let queued = window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                    queued
                })
            })
            .collect()
    }

    fn select(r: &mut Rig, k: &str) {
        let page = r.page.clone();
        let k = key(k);
        r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar.update(cx, |s, cx| s.select(k, cx));
        });
        draw(r.vcx);
    }

    fn selected(r: &mut Rig) -> Option<String> {
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.read(cx).selected().map(|k| k.0.to_string()))
    }

    fn title(r: &mut Rig) -> String {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).window_title.clone())
    }

    fn merged(r: &mut Rig) -> Demand {
        let demand = r.demand.clone();
        r.vcx.update(|_, cx| demand.read(cx).merged())
    }

    fn click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn summary_tiles_all_visible_devices(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        let page = r.page.clone();
        let cards: Vec<String> = r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar
                .read(cx)
                .visible_keys()
                .map(|k| k.0.to_string())
                .collect()
        });
        assert_eq!(cards.len(), 9, "{cards:?}");
        let view = enter(&mut r);
        let tiles: Vec<String> = r.vcx.read_entity(&view, |v, _| {
            v.tiles.iter().map(|t| t.key.0.to_string()).collect()
        });
        assert_eq!(tiles, cards, "one tile per card, in sidebar order");
        for i in 0..9 {
            let id = format!("perf-summary-tile-{i}");
            assert!(r.vcx.debug_bounds(Box::leak(id.into_boxed_str())).is_some());
        }
        // The tab strip and the sidebar are not drawn under the summary.
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
    }

    #[gpui_kit::test]
    fn summary_skips_overview_card(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings
            .performance
            .device_overrides
            .insert(OVERVIEW_KEY.to_owned(), DeviceVisibility::Show);
        let mut r = open_with(cx, 1600., 1000., false, settings);
        let page = r.page.clone();
        let cards: Vec<String> = r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar
                .read(cx)
                .visible_keys()
                .map(|k| k.0.to_string())
                .collect()
        });
        assert_eq!(cards.len(), 10, "{cards:?}");
        assert_eq!(cards[0], OVERVIEW_KEY);
        let view = enter(&mut r);
        let tiles: Vec<String> = r.vcx.read_entity(&view, |v, _| {
            v.tiles.iter().map(|t| t.key.0.to_string()).collect()
        });
        assert_eq!(tiles, cards[1..], "no Overview tile (D-022)");
        // A sidebar notify re-runs the tile sync; the Overview card does not rebuild the tiles.
        let first = r
            .vcx
            .read_entity(&view, |v, _| v.tiles[0].chart.entity_id());
        r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar.update(cx, |_, cx| cx.notify());
        });
        draw(r.vcx);
        let again = r
            .vcx
            .read_entity(&view, |v, _| v.tiles[0].chart.entity_id());
        assert_eq!(first, again);
    }

    #[gpui_kit::test]
    fn summary_tile_grid_for_9(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        let view = enter(&mut r);
        let l = r.vcx.read_entity(&view, |v, _| v.layout());
        assert_eq!((l.cols, l.rows), (3, 3));
        assert!(!l.scrolls);
        let t0 = r.vcx.debug_bounds("perf-summary-tile-0").expect("tile 0");
        let t2 = r.vcx.debug_bounds("perf-summary-tile-2").expect("tile 2");
        let t3 = r.vcx.debug_bounds("perf-summary-tile-3").expect("tile 3");
        assert_eq!(t0.size, t3.size);
        assert_eq!(t2.top(), t0.top(), "three tiles in the first row");
        assert!(t3.top() > t0.bottom(), "tile 3 starts the second row");
        assert_eq!(t3.left(), t0.left());
    }

    /// Each tile's legend names its series in the UI-PRF-003 words and styles: Memory "Used"
    /// solid and "Committed" dashed, Network "Download" solid and "Upload" dashed, CPU "Total"
    /// and "Kernel" only with the kernel line, and none on the one-series tiles. Checked against
    /// the 1.0.0 smoke-test shot of summary mode (o55.43 h).
    #[gpui_kit::test]
    fn summary_tile_legends_match_their_series(cx: &mut TestAppContext) {
        use tl_ui::chart::LegendStyle::{Dashed, Solid};
        use tl_ui::pages::performance::DeviceKind;
        let mut r = open(cx, 1600., 1000., false);
        let view = enter(&mut r);
        let tiles: Vec<_> = r.vcx.update(|_, cx| {
            view.read(cx)
                .tiles
                .iter()
                .map(|t| {
                    let c = t.chart.read(cx);
                    let legend: Vec<_> = c
                        .labels
                        .legend
                        .iter()
                        .map(|(s, style)| (s.to_string(), *style))
                        .collect();
                    let dashed: Vec<bool> = c.spec.series.iter().map(|s| s.dashed).collect();
                    (t.kind, legend, dashed)
                })
                .collect()
        });
        let words = |l: &[(String, tl_ui::chart::LegendStyle)]| -> Vec<_> {
            l.iter().map(|(s, style)| (s.clone(), *style)).collect()
        };
        for (kind, legend, dashed) in &tiles {
            let pairs = |a: &str, sa, b: &str, sb| vec![(a.to_string(), sa), (b.to_string(), sb)];
            match kind {
                DeviceKind::Memory => {
                    assert_eq!(words(legend), pairs("Used", Solid, "Committed", Dashed));
                }
                DeviceKind::Net => {
                    assert_eq!(words(legend), pairs("Download", Solid, "Upload", Dashed));
                }
                DeviceKind::Cpu if dashed.len() > 1 => {
                    assert_eq!(words(legend), pairs("Total", Solid, "Kernel", Solid));
                }
                _ => assert!(legend.is_empty(), "{kind:?}: {legend:?}"),
            }
            if !legend.is_empty() {
                assert_eq!(legend.len(), dashed.len(), "{kind:?}: one item per series");
                for ((_, style), dashed) in legend.iter().zip(dashed) {
                    assert_eq!(
                        *style == Dashed,
                        *dashed,
                        "{kind:?}: swatch matches the line"
                    );
                }
            }
        }
        let kinds: Vec<_> = tiles.iter().map(|t| t.0).collect();
        assert!(kinds.contains(&DeviceKind::Memory) && kinds.contains(&DeviceKind::Net));
    }

    #[gpui_kit::test]
    fn summary_blocks_tab_switch(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        enter(&mut r);
        for keys in ["ctrl-2", "ctrl-1", "ctrl-3", "alt-1", "ctrl-tab"] {
            r.vcx.simulate_keystrokes(keys);
            draw(r.vcx);
            let tab = r.vcx.read_entity(&r.main, |m, _| m.active_tab());
            assert_eq!(tab, Tab::Performance, "{keys}");
            assert!(summary(&mut r).is_some(), "{keys}: summary still on");
        }
    }

    #[gpui_kit::test]
    fn summary_esc_returns_to_selected_device(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        select(&mut r, "memory");
        assert_eq!(title(&mut r), "");
        let view = enter(&mut r);
        assert_eq!(title(&mut r), "tasklens · Graph Summary");
        let marked = r.vcx.read_entity(&view, |v, _| v.selected_on_entry.clone());
        assert_eq!(marked, Some(key("memory")));
        let marker = r.vcx.debug_bounds(MARKER_ID).expect("accent marker");
        let tile = r
            .vcx
            .debug_bounds("perf-summary-tile-1")
            .expect("memory tile");
        assert!(
            tile.contains(&marker.origin),
            "the marker is on the memory tile"
        );

        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(summary(&mut r).is_none());
        assert_eq!(selected(&mut r).as_deref(), Some("memory"));
        assert_eq!(title(&mut r), "tasklens");
        assert!(r.vcx.debug_bounds("perf-summary-tile-0").is_none());
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
    }

    #[gpui_kit::test]
    fn summary_click_selects_and_exits(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        let view = enter(&mut r);
        let i = r
            .vcx
            .read_entity(&view, |v, _| {
                v.tiles.iter().position(|t| t.key.0.as_ref() == GPU)
            })
            .expect("a GPU tile");
        let id = format!("perf-summary-tile-{i}");
        let tile = r
            .vcx
            .debug_bounds(Box::leak(id.into_boxed_str()))
            .expect("GPU tile drawn");
        click(r.vcx, tile.center());
        draw(r.vcx);
        assert!(summary(&mut r).is_none());
        assert_eq!(selected(&mut r).as_deref(), Some(GPU));
        assert_eq!(title(&mut r), "tasklens");
    }

    #[test]
    fn summary_small_window_shrinks_cols_and_scrolls() {
        let l = tile_grid(9, px(500.), px(300.));
        assert_eq!(l.cols, 2, "two 200 px columns fit, three do not");
        assert_eq!(l.rows, 5);
        assert!(l.scrolls);
        assert!(l.tile_w >= px(200.));
        assert_eq!(l.tile_h, px(120.), "the minimum tile height");

        let big = tile_grid(9, px(1600.), px(1000.));
        assert_eq!((big.cols, big.rows), (3, 3));
        assert!(!big.scrolls);
    }

    #[gpui_kit::test]
    fn summary_fade_follows_reduce_motion(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let view = enter(&mut r);
        assert!(
            !r.vcx.read_entity(&view, |v, _| v.fades()),
            "reduce motion: instant"
        );
    }

    /// The sidebar and page fade out over `m_fast` before the tiles show, and the frames stop
    /// once both fades end.
    #[gpui_kit::test]
    fn summary_fades_page_out_then_tiles_in(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        assert!(pending(&mut r), "fading out");
        assert!(summary(&mut r).is_none());
        assert!(
            r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some(),
            "still drawn"
        );
        assert!(frames(&mut r, 1)[0] > 0, "the fade requests frames");

        r.vcx.executor().advance_clock(M_FAST);
        draw(r.vcx);
        assert!(!pending(&mut r));
        let view = summary(&mut r).expect("summary on after m_fast");
        assert!(r.vcx.read_entity(&view, |v, _| v.fades()));
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());

        // GPUI times `with_animation` by the wall clock, not the test clock.
        std::thread::sleep(M_BASE + M_FAST);
        let seen = frames(&mut r, 8);
        assert_eq!(
            &seen[5..],
            &[0, 0, 0],
            "no frames after the fades: {seen:?}"
        );
    }

    /// Ctrl+M again during the fade-out stays on the page.
    #[gpui_kit::test]
    fn summary_toggle_during_fade_cancels(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        assert!(pending(&mut r));
        r.vcx.simulate_keystrokes("ctrl-m");
        r.vcx.executor().advance_clock(M_FAST * 2);
        draw(r.vcx);
        assert!(!pending(&mut r));
        assert!(summary(&mut r).is_none());
        assert_eq!(title(&mut r), "");
    }

    #[gpui_kit::test]
    fn summary_opens_at_once_with_animations_off(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        assert!(!pending(&mut r));
        assert!(summary(&mut r).is_some(), "no fade-out wait");
    }

    #[gpui_kit::test]
    fn summary_hides_title_bar_menus(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        assert!(r.vcx.debug_bounds("TitleBarMenu").is_some());
        enter(&mut r);
        assert!(
            r.vcx.debug_bounds("TitleBarMenu").is_none(),
            "hidden in summary mode"
        );
        r.vcx.simulate_keystrokes("alt-f");
        draw(r.vcx);
        assert!(summary(&mut r).is_some(), "alt-f opens no menu");
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(summary(&mut r).is_none());
        assert!(r.vcx.debug_bounds("TitleBarMenu").is_some(), "shown again");
    }

    #[gpui_kit::test]
    fn summary_fades_in_with_motion(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        let view = enter(&mut r);
        assert!(r.vcx.read_entity(&view, |v, _| v.fades()));
    }

    /// The summary view never notifies on a tick: a snapshot that changes no tile series renders
    /// nothing. A CPU sample notifies the CPU chart only. GPUI then marks every ancestor view
    /// dirty, so the summary view and the other tile charts re-run `render` too, but the other
    /// charts build no new geometry (ADR-009).
    #[gpui_kit::test]
    fn summary_sample_tick_does_not_rerender_view(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let view = enter(&mut r);
        draw(r.vcx);
        let (cpu, memory) = r.vcx.read_entity(&view, |v, _| {
            (v.tiles[0].chart.clone(), v.tiles[1].chart.clone())
        });
        assert_eq!(
            r.vcx.read_entity(&view, |v, _| v.tiles[1].key.clone()),
            key("memory")
        );
        let counts = |r: &mut Rig| {
            let (view, cpu, memory) = (view.clone(), cpu.clone(), memory.clone());
            r.vcx.update(|_, cx| {
                (
                    view.read(cx).render_count.get(),
                    cpu.read(cx).render_count.get(),
                    memory.read(cx).build_count.get(),
                )
            })
        };

        let before = counts(&mut r);
        for _ in 0..3 {
            r.engine
                .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
            draw(r.vcx);
        }
        assert_eq!(
            counts(&mut r),
            before,
            "no tile series changed: nothing renders"
        );

        for version in 1..=3u64 {
            r.engine.publish_with(|s| {
                let id = SeriesId {
                    device: DeviceIdx(0),
                    metric: MetricId::CPU_TOTAL,
                };
                let samples: Vec<Sample> = (0..version)
                    .map(|i| Sample::valid(i * 1_000_000_000, 10.0 * i as f32))
                    .collect();
                let mut set = SeriesSet::default();
                set.header.version = version;
                set.series = Arc::from([Arc::new(SeriesView {
                    id,
                    version,
                    period_ns: 1_000_000_000,
                    samples: Arc::from(samples),
                })]);
                s.series = Arc::new(set);
            });
            draw(r.vcx);
        }
        let after = counts(&mut r);
        assert!(
            after.1 > before.1,
            "the CPU chart re-rendered: {before:?} {after:?}"
        );
        assert_eq!(after.2, before.2, "the memory chart built no geometry");
    }

    /// UI-PRF-058: the page header shows its device's main value, the sidebar card's value, at
    /// the right before the toggles, and draws it again on each new sample.
    #[gpui_kit::test]
    fn page_header_value_follows_the_card(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let publish = |r: &Rig, version: u64, value: f32| {
            r.engine.publish_with(|s| {
                let mut set = SeriesSet::default();
                set.header.version = version;
                set.series = Arc::from([Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(0),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: 1_000_000_000,
                    samples: Arc::from([Sample::valid(1_000_000_000, value)]),
                })]);
                s.series = Arc::new(set);
            });
        };
        publish(&r, 1, 8.0);
        draw(r.vcx);
        let header = r.vcx.debug_bounds("perf-page-header").expect("header");
        let model = r.vcx.debug_bounds("perf-page-model").expect("model");
        let toggles = r.vcx.debug_bounds("perf-page-toggles").expect("toggles");
        let narrow = r.vcx.debug_bounds("perf-page-value").expect("header value");
        assert!(narrow.left() >= model.right(), "{narrow:?} after {model:?}");
        assert_eq!(
            toggles.left() - narrow.right(),
            px(8.),
            "sp4 before the toggles"
        );
        assert!(narrow.top() >= header.top() && narrow.bottom() <= header.bottom());

        // "100.0%" is wider than "8.0%": the value follows the new sample.
        publish(&r, 2, 100.0);
        draw(r.vcx);
        let wide = r.vcx.debug_bounds("perf-page-value").expect("header value");
        assert!(
            wide.size.width > narrow.size.width,
            "{wide:?} after {narrow:?}"
        );
        assert_eq!(
            wide.right(),
            narrow.right(),
            "right-aligned before the toggles"
        );
    }

    fn right_click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    /// One built menu entry: kind, label, tooltip, checked, submenu.
    type Entry = (
        MenuEntryKind,
        String,
        Option<String>,
        bool,
        Option<Entity<TlMenuPopup>>,
    );

    fn entries_of(r: &mut Rig, menu: &Entity<TlMenuPopup>) -> Vec<Entry> {
        r.vcx.update(|window, cx| {
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| {
                    (
                        e.kind,
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.tooltip.map(|t| t.to_string()),
                        e.checked,
                        e.submenu,
                    )
                })
                .collect()
        })
    }

    /// The open context menu's entries.
    fn open_menu(r: &mut Rig) -> Vec<Entry> {
        let menu = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        entries_of(r, &menu)
    }

    fn labels(list: &[Entry]) -> Vec<(MenuEntryKind, &str)> {
        list.iter().map(|e| (e.0, e.1.as_str())).collect()
    }

    /// The Time range submenu of `list`.
    fn ranges(r: &mut Rig, list: &[Entry]) -> Vec<Entry> {
        let sub = list
            .iter()
            .find(|e| e.1 == "Time range")
            .and_then(|e| e.4.clone())
            .expect("a Time range submenu");
        entries_of(r, &sub)
    }

    /// A point inside the page panel's top-left padding, away from every Details value, so the
    /// page menu opens without "Copy value" whatever the values are.
    fn page_panel(r: &mut Rig) -> gpui_kit::Point<gpui_kit::Pixels> {
        let b = r
            .vcx
            .debug_bounds(tl_ui::pages::performance::PAGE_PANEL_ID)
            .expect("the page panel");
        b.origin + gpui_kit::point(gpui_kit::px(6.), gpui_kit::px(6.))
    }

    #[gpui_kit::test]
    fn page_menu_common_order(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        let list = open_menu(&mut r);
        assert_eq!(
            labels(&list),
            [
                (MenuEntryKind::Item, "Copy"),
                (MenuEntryKind::Item, "Graph Summary View"),
                (MenuEntryKind::Submenu, "Time range"),
                (MenuEntryKind::Separator, ""),
                (MenuEntryKind::Submenu, "Graph"),
                (MenuEntryKind::Submenu, "Secondary graph"),
                (MenuEntryKind::Item, "Kernel times"),
                (MenuEntryKind::Separator, ""),
                (MenuEntryKind::Submenu, "View"),
                (MenuEntryKind::Disabled, "Add tray icon graph"),
            ]
        );
        assert!(!list[1].3, "summary mode is off");
        let ranges = ranges(&mut r, &list);
        let checked: Vec<&str> = ranges
            .iter()
            .filter(|e| e.3)
            .map(|e| e.1.as_str())
            .collect();
        assert_eq!(checked, ["1 minute"], "range 60 s");
        assert_eq!(
            selected(&mut r).as_deref(),
            Some("cpu"),
            "selection unchanged"
        );
    }

    #[gpui_kit::test]
    fn page_menu_long_ranges_enabled(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        let list = open_menu(&mut r);
        let ranges = ranges(&mut r, &list);
        let shape: Vec<(MenuEntryKind, &str, Option<String>)> = ranges
            .iter()
            .map(|e| (e.0, e.1.as_str(), e.2.clone()))
            .collect();
        assert_eq!(
            shape,
            [
                (MenuEntryKind::Item, "30 seconds", None),
                (MenuEntryKind::Item, "1 minute", None),
                (MenuEntryKind::Item, "5 minutes", None),
                (MenuEntryKind::Item, "10 minutes", None),
            ]
        );
    }

    /// The View submenu of `list`.
    fn view_items(r: &mut Rig, list: &[Entry]) -> Vec<Entry> {
        let sub = list
            .iter()
            .find(|e| e.1 == "View")
            .and_then(|e| e.4.clone())
            .expect("a View submenu");
        entries_of(r, &sub)
    }

    /// The rig has CPU, Memory, a GPU, disks and network cards, and no fan or battery.
    #[gpui_kit::test]
    fn view_submenu_disables_empty_types(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        let list = open_menu(&mut r);
        let items = view_items(&mut r, &list);
        assert_eq!(
            labels(&items),
            [
                (MenuEntryKind::Item, "Processor"),
                (MenuEntryKind::Item, "Memory"),
                (MenuEntryKind::Item, "Disks"),
                (MenuEntryKind::Item, "Graphics adapters"),
                (MenuEntryKind::Item, "Network adapters"),
                (MenuEntryKind::Disabled, "Fans"),
                (MenuEntryKind::Disabled, "Batteries"),
            ]
        );
    }

    fn menu_is_open(r: &mut Rig) -> bool {
        r.vcx
            .update(|window, cx| open_context_menu(window, cx))
            .is_some()
    }

    /// jqk.37: after Esc closes the page menu, F2 selects the second card at once, with no
    /// click in between.
    #[gpui_kit::test]
    fn page_menu_esc_keeps_keys(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        draw(r.vcx);
        assert!(menu_is_open(&mut r));

        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(!menu_is_open(&mut r), "Esc closed the menu");

        r.vcx.simulate_keystrokes("f2");
        draw(r.vcx);
        assert_ne!(
            selected(&mut r).as_deref(),
            Some("cpu"),
            "F2 ran after the menu closed"
        );
    }

    /// jqk.37: after a picked item closes the page menu, F2 selects the second card and Ctrl+M
    /// turns summary mode on, with no click in between.
    #[gpui_kit::test]
    fn page_menu_pick_keeps_keys(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        draw(r.vcx);
        assert!(menu_is_open(&mut r));

        // "Copy", the first item.
        r.vcx.simulate_keystrokes("down enter");
        draw(r.vcx);
        assert!(!menu_is_open(&mut r), "the picked item closed the menu");

        r.vcx.simulate_keystrokes("f2");
        draw(r.vcx);
        assert_ne!(
            selected(&mut r).as_deref(),
            Some("cpu"),
            "F2 ran after the menu closed"
        );
        enter(&mut r);
    }

    /// jqk.37: with the keys on the device page, F2 shows another page and the keys follow
    /// it, so Ctrl+M still turns summary mode on.
    #[gpui_kit::test]
    fn keys_follow_the_shown_page(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        let at = page_panel(&mut r);
        click(r.vcx, at);
        draw(r.vcx);

        r.vcx.simulate_keystrokes("f2");
        draw(r.vcx);
        assert_ne!(selected(&mut r).as_deref(), Some("cpu"), "F2 ran");
        enter(&mut r);
    }

    #[gpui_kit::test]
    fn jump_to_type_selects_first_visible(cx: &mut TestAppContext) {
        use tl_ui::components::menu::close_context_menu;
        use tl_ui::pages::performance::DeviceKind;
        use tl_ui::pages::performance::actions::PerfJumpToType;

        let mut r = open(cx, 1600., 1000., true);
        let page = r.page.clone();
        let first_net = r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().read(cx);
            sidebar
                .visible_keys()
                .find(|k| {
                    sidebar
                        .card(k)
                        .is_some_and(|c| c.row.kind == DeviceKind::Net)
                })
                .map(|k| k.0.to_string())
        });
        assert!(first_net.is_some(), "the rig has a network card");
        r.vcx.update(close_context_menu);
        r.vcx.dispatch_action(PerfJumpToType {
            kind: DeviceKind::Net,
        });
        draw(r.vcx);
        assert_eq!(selected(&mut r), first_net);

        // No fan: nothing changes.
        r.vcx.dispatch_action(PerfJumpToType {
            kind: DeviceKind::Fan,
        });
        draw(r.vcx);
        assert_eq!(selected(&mut r), first_net);
    }

    /// crw.2: the P1 sidebar actions work together in one app and persist only durable state.
    #[gpui_kit::test]
    fn sidebar_p1_flow(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::{
            PerfOpenCardMenu, PerfSetGraphRange, PerfToggleSidebar,
        };

        let mut r = open(cx, 1600., 1000., true);
        select(&mut r, "disk:sdb");

        // Open the selected card's menu, then run its Hide This Page action.
        r.vcx.dispatch_action(PerfOpenCardMenu);
        draw(r.vcx);
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the card menu is open");
            let hide = menu
                .read(cx)
                .entries_for_test(window)
                .into_iter()
                .find(|entry| entry.label.as_deref() == Some("Hide This Page"))
                .and_then(|entry| entry.action)
                .expect("Hide This Page has an action");
            assert!(
                hide.partial_eq(&tl_ui::pages::performance::actions::PerfSetPageVisible {
                    key: key("disk:sdb"),
                    visible: false,
                })
            );
        });
        r.vcx.simulate_keystrokes("down down enter");
        draw(r.vcx);
        assert_eq!(
            selected(&mut r).as_deref(),
            Some("disk:sdc"),
            "selection moves to the next card"
        );

        r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
        draw(r.vcx);
        let page = r.page.clone();
        let ranges = r.vcx.update(|_, cx| {
            page.read(cx)
                .sidebar()
                .read(cx)
                .cards()
                .iter()
                .map(|card| card.chart.read(cx).spec.range_ns)
                .collect::<Vec<_>>()
        });
        assert!(!ranges.is_empty());
        assert!(
            ranges.iter().all(|&range| range == 30_000_000_000),
            "every sidebar chart has the 30 s range: {ranges:?}"
        );

        r.vcx.dispatch_action(PerfToggleSidebar);
        draw(r.vcx);
        let page = r.page.clone();
        let sidebar_visible = r.vcx.update(|_, cx| page.read(cx).sidebar_visible());
        assert!(!sidebar_visible, "the sidebar is hidden");

        let state = r
            .persisted_state
            .borrow()
            .clone()
            .expect("state.toml persistence ran");
        assert_eq!(state.performance.selected_device, "disk:sdc");
        assert_eq!(
            state.performance,
            tl_model::settings::PerformanceState {
                nav_width: 252,
                selected_device: "disk:sdc".to_owned(),
            },
            "state.toml has no transient sidebar visibility value"
        );
    }

    #[gpui_kit::test]
    fn long_range_switches_tier(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfSetGraphRange;

        let r = open(cx, 1600., 1000., true);
        let id = SeriesId {
            device: DeviceIdx(0),
            metric: MetricId::CPU_TOTAL,
        };
        let view = |id: SeriesId, period_ns: u64| {
            Arc::new(SeriesView {
                id,
                version: 1,
                period_ns,
                samples: Arc::from([Sample::valid(period_ns, 10.0)]),
            })
        };
        r.engine.publish_with(|s| {
            let mut set = SeriesSet::default();
            set.header.version = 1;
            set.series = Arc::from([view(id, 100_000_000)]);
            set.slow = Arc::from([view(
                SeriesId {
                    metric: id.metric.with_tier_1hz(),
                    ..id
                },
                1_000_000_000,
            )]);
            s.series = Arc::new(set);
        });
        draw(r.vcx);
        let page = r.page.clone();
        let period = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                let sidebar = page.read(cx).sidebar().read(cx);
                let card = sidebar
                    .cards()
                    .iter()
                    .find(|c| c.row.key.0.as_ref() == "cpu")
                    .expect("CPU card");
                let chart = card.chart.read(cx);
                (
                    TlSettings::settings(cx).performance.graph_range_s,
                    chart.resolved()[0].as_ref().map(|v| v.period_ns),
                )
            })
        };
        assert_eq!(period(r.vcx), (60, Some(100_000_000)), "60 s reads 10 Hz");

        r.vcx.dispatch_action(PerfSetGraphRange { secs: 600 });
        draw(r.vcx);
        assert_eq!(
            period(r.vcx),
            (600, Some(1_000_000_000)),
            "10 min reads 1 Hz"
        );

        r.vcx.dispatch_action(PerfSetGraphRange { secs: 300 });
        draw(r.vcx);
        assert_eq!(period(r.vcx).0, 300);
    }

    /// `TlMenu` drops a doubled separator, so a page without its own items has one separator
    /// between the common items and View, never two.
    #[gpui_kit::test]
    fn page_menu_single_separator_without_page_items(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        select(&mut r, "disk:sda");
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        let list = open_menu(&mut r);
        assert_eq!(
            labels(&list),
            [
                (MenuEntryKind::Item, "Copy"),
                (MenuEntryKind::Item, "Graph Summary View"),
                (MenuEntryKind::Submenu, "Time range"),
                (MenuEntryKind::Separator, ""),
                (MenuEntryKind::Submenu, "View"),
                (MenuEntryKind::Disabled, "Add tray icon graph"),
            ]
        );
    }

    #[gpui_kit::test]
    fn set_graph_range_updates_every_chart(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfSetGraphRange;

        let r = open(cx, 1600., 1000., true);
        r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
        draw(r.vcx);
        let page = r.page.clone();
        let (range_s, card_ranges) = r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            let ranges: Vec<u64> = sidebar
                .read(cx)
                .cards()
                .iter()
                .map(|c| c.chart.read(cx).spec.range_ns)
                .collect();
            (TlSettings::settings(cx).performance.graph_range_s, ranges)
        });
        assert_eq!(range_s, 30);
        assert_eq!(card_ranges.len(), 9);
        assert!(
            card_ranges.iter().all(|&ns| ns == 30_000_000_000),
            "{card_ranges:?}"
        );
    }

    /// Right-click on a tile opens the page menu's common items (UI-PRF-016); its Time range
    /// items reach the tiles.
    #[gpui_kit::test]
    fn summary_tile_right_click_page_menu(cx: &mut TestAppContext) {
        use tl_ui::components::menu::close_context_menu;
        use tl_ui::pages::performance::actions::PerfSetGraphRange;

        let mut r = open(cx, 1600., 1000., true);
        let view = enter(&mut r);
        let tile = r.vcx.debug_bounds("perf-summary-tile-0").expect("CPU tile");
        right_click(r.vcx, tile.center());
        draw(r.vcx);
        assert!(
            summary(&mut r).is_some(),
            "right-click does not leave summary mode"
        );
        let list = open_menu(&mut r);
        assert_eq!(
            labels(&list),
            [
                (MenuEntryKind::Item, "Copy"),
                (MenuEntryKind::Item, "Graph Summary View"),
                (MenuEntryKind::Submenu, "Time range"),
            ]
        );
        assert!(list[1].3, "checked in summary mode");
        // No page items: the CPU page's Graph and Kernel times stay out (no page is drawn).
        assert!(!list.iter().any(|e| e.1 == "Graph" || e.1 == "Kernel times"));
        assert!(!list.iter().any(|e| e.0 == MenuEntryKind::Separator));

        // The items run on the summary view.
        let focus = r.vcx.update(|_, cx| view.read(cx).focus_handle(cx));
        r.vcx.update(|window, cx| {
            close_context_menu(window, cx);
            focus.focus(window, cx);
        });
        r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
        draw(r.vcx);
        let ranges: Vec<u64> = r.vcx.update(|_, cx| {
            view.read(cx)
                .tiles
                .iter()
                .map(|t| t.chart.read(cx).spec.range_ns)
                .collect()
        });
        assert!(ranges.iter().all(|&ns| ns == 30_000_000_000), "{ranges:?}");
        assert!(summary(&mut r).is_some());
    }

    /// The summary sends its own part (CPU at the fast rate); the hidden device page withdraws
    /// its part; the sidebar keeps its own; a hidden window drops them all.
    #[gpui_kit::test]
    fn summary_demand_on_off_and_hidden(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        select(&mut r, "memory");
        let memory = {
            let page = r.page.clone();
            r.vcx.update(|_, cx| {
                page.read(cx)
                    .page(&key("memory"))
                    .expect("memory page built")
                    .clone()
                    .downcast::<MemoryPage>()
                    .expect("a MemoryPage")
            })
        };
        let off = merged(&mut r);
        assert!(off.any_surface_visible && off.memory);
        assert!(!off.cpu_fast, "the CPU page is not shown");

        enter(&mut r);
        let on = merged(&mut r);
        assert!(on.any_surface_visible);
        assert!(on.cpu_fast, "the CPU tile runs at 10 Hz");
        assert!(
            on.disks && on.nets && on.gpus,
            "the sidebar keeps its part: {on:?}"
        );
        assert!(
            !r.vcx.read_entity(&memory, |p, _| p.is_active()),
            "page part withdrawn"
        );

        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        let back = merged(&mut r);
        assert!(!back.cpu_fast, "the summary part is gone");
        assert!(r.vcx.read_entity(&memory, |p, _| p.is_active()));
        assert_eq!(back, off);

        // Hidden: a change waits over 1 s for a render, and every main-window part drops out.
        enter(&mut r);
        assert!(merged(&mut r).cpu_fast);
        r.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        r.vcx.run_until_parked();
        r.vcx.executor().advance_clock(Duration::from_millis(1600));
        r.vcx.run_until_parked();
        let hub = r.hub.clone();
        assert!(r.vcx.read_entity(&hub, |h, _| h.is_hidden()));
        assert_eq!(merged(&mut r), Demand::default());
    }

    /// Publishes a CPU speed of 4631 MHz and returns the bounds of the CPU page's Speed value.
    fn speed_value(r: &mut Rig) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        use tl_ui::pages::performance::cpu::CpuPage;
        use tl_ui::pages::performance::cpu_details::ROW_SPEED;
        use tl_ui::pages::performance::page::DevicePage as _;

        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.cpu.freq_mhz = tl_model::system::Metric::valid(4631);
        });
        draw(r.vcx);
        let page = r.page.clone();
        let n = r.vcx.update(|_, cx| {
            let cpu = page
                .read(cx)
                .page(&key("cpu"))
                .expect("CPU page built")
                .clone()
                .downcast::<CpuPage>()
                .expect("a CpuPage");
            cpu.read(cx)
                .details()
                .rows
                .iter()
                .filter(|row| !row.hidden)
                .position(|row| row.label == ROW_SPEED)
                .expect("a Speed row")
        });
        let selector: &'static str = format!("TlKv-value-{n}").leak();
        r.vcx.debug_bounds(selector).expect("Speed value drawn")
    }

    fn clipboard(r: &mut Rig) -> Option<String> {
        r.vcx.read_from_clipboard().and_then(|item| item.text())
    }

    /// Details values are selectable text (U-46): a drag over the Speed value and Ctrl+C copy
    /// the value alone.
    #[gpui_kit::test]
    fn value_drag_select_ctrl_c_copies_value(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let value = speed_value(&mut r);
        let (from, to) = (
            gpui_kit::point(value.left() + px(1.), value.center().y),
            gpui_kit::point(value.right() - px(1.), value.center().y),
        );
        r.vcx
            .simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        r.vcx
            .simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        r.vcx
            .simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        draw(r.vcx);
        r.vcx.simulate_keystrokes("ctrl-c");
        r.vcx.run_until_parked();
        assert_eq!(clipboard(&mut r).as_deref(), Some("4631 MHz"));
    }

    /// Without a selection, Ctrl+C on the page goes on to the page's Copy (E24-016), which
    /// copies the page text.
    #[gpui_kit::test]
    fn copy_without_selection_goes_to_page_copy(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let value = speed_value(&mut r);
        // A click focuses the grid but selects nothing.
        r.vcx.simulate_click(value.center(), Modifiers::none());
        draw(r.vcx);
        r.vcx.simulate_keystrokes("ctrl-c");
        r.vcx.run_until_parked();
        let text = clipboard(&mut r).expect("the page copied");
        assert!(text.starts_with("Processor\n"), "the page copy ran: {text}");
    }

    /// A right-click on a value opens the page menu with "Copy value" first; it copies the
    /// value.
    #[gpui_kit::test]
    fn copy_value_context_item(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let value = speed_value(&mut r);
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        right_click(r.vcx, value.center());
        let list = open_menu(&mut r);
        assert_eq!(
            labels(&list)[..2],
            [
                (MenuEntryKind::Item, "Copy value"),
                (MenuEntryKind::Item, "Copy"),
            ]
        );
        r.vcx.simulate_keystrokes("down enter");
        r.vcx.run_until_parked();
        assert_eq!(clipboard(&mut r).as_deref(), Some("4631 MHz"));

        // A right-click off the values has no "Copy value".
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        let list = open_menu(&mut r);
        assert_eq!(labels(&list)[0], (MenuEntryKind::Item, "Copy"));
    }

    /// Publishes the fake CPU values of the copy tests: a model name, every Details value
    /// Valid, and the caches.
    fn publish_cpu(r: &mut Rig, temp: Validity) {
        use tl_model::snapshot::{
            CacheInfo, CacheLevelInfo, CpuStaticInfo, InvField, SystemSnapshot, VirtState,
        };

        fn inv<T>(value: T) -> InvField<T> {
            InvField {
                value,
                validity: Validity::Valid,
            }
        }
        fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
            Some(CacheLevelInfo {
                instances,
                instance_kib,
                total_kib: u64::from(instances) * instance_kib,
            })
        }
        r.engine.publish_with(|s| {
            let sys: &mut SystemSnapshot = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.header.validity = Validity::Valid;
            let c = &mut sys.cpu;
            c.util_pct = Metric::valid(18.6);
            c.kernel_pct = Metric::valid(2.3);
            c.freq_mhz = Metric::valid(4631);
            c.base_mhz = Metric::valid(4200);
            c.temp_c = if temp == Validity::Valid {
                Metric::valid(52.0)
            } else {
                Metric::with(temp)
            };
            c.processes = Metric::valid(295);
            c.threads = Metric::valid(5596);
            c.handles = Metric::valid(253_248);
            c.load_avg = Metric::valid([1.24, 0.98, 0.87]);
            c.sockets = Metric::valid(1);
            let dd = Arc::make_mut(&mut s.device_details);
            dd.header.version += 1;
            dd.cpu = Arc::new(CpuStaticInfo {
                cores: 8,
                logical: 16,
                caches: CacheInfo {
                    l1d: level(8, 32),
                    l1i: level(8, 32),
                    l2: level(8, 1024),
                    l3: level(1, 98_304),
                    ..CacheInfo::default()
                },
                driver: inv(Arc::from("amd-pstate-epp")),
                governor: inv(Arc::from("powersave")),
                epp: inv(Arc::from("balance_performance")),
                platform_profile: inv(Arc::from("balanced")),
                virtualization: inv(VirtState::EnabledKvm),
                running_in_vm: inv(Arc::from("")),
                microcode: inv(Arc::from("0xa601206")),
                rapl: None,
                ..CpuStaticInfo::default()
            });
            let invt = Arc::make_mut(&mut s.inventory);
            invt.header.version += 1;
            invt.processor.name = inv(Arc::from("AMD Ryzen 7 7800X3D 8-Core Processor"));
        });
        draw(r.vcx);
    }

    /// The Up time value the CPU page shows (it comes from the real boot clock).
    fn up_time(r: &mut Rig) -> String {
        use tl_ui::pages::performance::cpu::CpuPage;
        use tl_ui::pages::performance::cpu_details::ROW_UP_TIME;
        use tl_ui::pages::performance::page::DevicePage as _;

        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let cpu = page
                .read(cx)
                .page(&key("cpu"))
                .expect("CPU page built")
                .clone()
                .downcast::<CpuPage>()
                .expect("a CpuPage");
            cpu.read(cx)
                .details()
                .rows
                .iter()
                .find(|row| row.label == ROW_UP_TIME)
                .expect("an Up time row")
                .value
                .to_string()
        })
    }

    #[gpui_kit::test]
    fn copy_text_cpu_format(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfCopyDeviceInfo;

        let mut r = open(cx, 1600., 1000., true);
        publish_cpu(&mut r, Validity::Valid);
        let up = up_time(&mut r);
        r.vcx.dispatch_action(PerfCopyDeviceInfo);
        r.vcx.run_until_parked();
        let expected = format!(
            "Processor\nAMD Ryzen 7 7800X3D 8-Core Processor\n\
             \x20   Utilization: 18.6%\n\
             \x20   Kernel time: 2.3%\n\
             \x20   Speed: 4631 MHz\n\
             \x20   Processes: 295\n\
             \x20   Threads: 5\u{2009}596\n\
             \x20   Handles: 253\u{2009}248\n\
             \x20   Up time: {up}\n\
             \x20   Base speed: 4200 MHz\n\
             \x20   Sockets: 1\n\
             \x20   Temperature: 52 \u{b0}C\n\
             \x20   Load average: 1.24 0.98 0.87\n\
             \x20   Cores: 8\n\
             \x20   Logical processors: 16\n\
             \x20   Virtualization: Enabled (KVM available)\n\
             \x20   Running in VM: No\n\
             \x20   Security features: None\n\
             \x20   Hypervisor: None\n\
             \x20   L1 cache: 512 KiB\n\
             \x20   L2 cache: 8 MiB\n\
             \x20   L3 cache: 96 MiB\n\
             \x20   Cpufreq driver: amd-pstate-epp\n\
             \x20   Governor: powersave\n\
             \x20   Power preference: balance_performance\n\
             \x20   Platform profile: balanced\n\
             \x20   Microcode: 0xa601206\n"
        );
        assert_eq!(clipboard(&mut r).as_deref(), Some(expected.as_str()));
    }

    #[gpui_kit::test]
    fn copy_text_fallback_words(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfCopyDeviceInfo;

        let mut r = open(cx, 1600., 1000., true);
        publish_cpu(&mut r, Validity::Restricted);
        r.vcx.dispatch_action(PerfCopyDeviceInfo);
        r.vcx.run_until_parked();
        let text = clipboard(&mut r).expect("copied");
        assert!(
            text.lines().any(|l| l == "    Temperature: Restricted"),
            "{text}"
        );
    }

    /// Ctrl+C and the menu's Copy put the same text on the clipboard; each pushes the toast
    /// once.
    #[gpui_kit::test]
    fn copy_pushes_toast_once(cx: &mut TestAppContext) {
        use tl_ui::components::toast::ToastQueue;
        use tl_ui::pages::performance::actions::PerfCopyDeviceInfo;

        fn toasts(r: &mut Rig) -> Vec<String> {
            r.vcx.update(|window, cx| {
                let queue = ToastQueue::of_window(window, cx).expect("a toast queue");
                queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(|t| t.to_string())
                    .collect()
            })
        }

        let mut r = open(cx, 1600., 1000., true);
        publish_cpu(&mut r, Validity::Valid);
        assert!(toasts(&mut r).is_empty());
        r.vcx.dispatch_action(PerfCopyDeviceInfo);
        r.vcx.run_until_parked();
        assert_eq!(toasts(&mut r), ["Copied metrics to clipboard"]);
        let by_action = clipboard(&mut r).expect("copied");

        // The menu's Copy (the first item off the values) copies the same text.
        r.vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()))
        });
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        draw(r.vcx);
        r.vcx.simulate_keystrokes("down enter");
        r.vcx.run_until_parked();
        assert_eq!(clipboard(&mut r).as_deref(), Some(by_action.as_str()));
        assert_eq!(
            toasts(&mut r),
            ["Copied metrics to clipboard"],
            "one toast at a time"
        );
    }

    /// A right-click on a value opens only the page menu: the grid's own menu (Copy value,
    /// Copy row, Copy section) stays shut, and the page menu has the focus.
    #[gpui_kit::test]
    fn right_click_value_opens_one_menu(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        let value = speed_value(&mut r);
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        right_click(r.vcx, value.center());
        draw(r.vcx);
        let menu = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("the page menu is open");
        let focused = r
            .vcx
            .update(|window, cx| menu.read(cx).focus_handle(cx).contains_focused(window, cx));
        assert!(
            focused,
            "the page menu has the focus, no other menu took it"
        );
        // The second item of the grid's menu is "Copy row"; the page menu's is "Copy", which
        // copies the page.
        r.vcx.simulate_keystrokes("down down enter");
        r.vcx.run_until_parked();
        let text = clipboard(&mut r).expect("the page Copy ran");
        assert!(
            text.starts_with("Processor\n"),
            "no grid menu item ran: {text}"
        );
    }
}
