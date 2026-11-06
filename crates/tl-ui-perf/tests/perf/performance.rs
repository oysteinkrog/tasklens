//! Performance tab: the Details grid model and its rendering (E24, PERF-011 to PERF-013,
//! UI-PRF-021), the device page frame, the Performance keys, the tab layout, and the graph
//! summary mode with the page menu and copy (`summary`). Moved from the tl-ui `ui` binary
//! (tasklens-zljr5.41); the first visit to the tab and four summary tests stay there, because
//! they go through MainWindow.

mod summary;

use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_model::validity::Validity;
use tl_ui_base::theme::metrics;
use tl_ui_kit::actions::OpenAdditionalSetup;
use tl_ui_kit::components::kv_grid::{self, KvWords, shown_text_in};
use tl_ui_perf::performance::details::{
    DetailWords, Fallback, KeyValueModel, fallback_word, render_details,
};
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};

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
        tl_ui_base::theme::store::init(
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
        AnyElement, App, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
        IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _,
        Render, SharedString, Styled as _, TestAppContext, TestDispatcher, VisualTestContext,
        Window, div, px,
    };
    use smallvec::SmallVec;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::snapshot::{
        DeviceDetailsSnapshot, DiskDetails, InventorySnapshot, NetworkAdapter, ProcessorInfo,
        Snapshot,
    };
    use tl_model::table::Total;
    use tl_model::time::BootId;
    use tl_model::validity::Validity;
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::theme::{current, metrics};
    use tl_ui_kit::components::button::{TlButton, TlButtonKind, TlButtonSize};
    use tl_ui_kit::components::kv_grid;
    use tl_ui_kit::components::menu::TlMenu;
    use tl_ui_kit::components::tooltip::DELAY_CONTROL;
    use tl_ui_perf::performance::actions::PerfShowMiniWindow;
    use tl_ui_perf::performance::details::KeyValueModel;
    use tl_ui_perf::performance::page::{DevicePage, DevicePageFrame, PageWords, header_text};
    use tl_ui_perf::performance::{DeviceKey, DeviceKind};
    use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};

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
        fn hub(&self) -> Option<&Entity<SnapshotHub>> {
            None
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
        tl_ui_base::theme::store::init(
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
        assert_eq!(b.left() - a.right(), px(8.), "sp4 apart");
        // The range switch ends the header (crw.105), sp4 after the toggles.
        let switch = cx
            .debug_bounds("perf-page-range-switch")
            .expect("range switch");
        assert_eq!(switch.right(), header.right(), "right-aligned");
        assert_eq!(switch.left() - b.right(), px(8.), "sp4 before the switch");
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
            let report = tl_ui_base::text::fonts::load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        });
        let long = "AMD Ryzen Threadripper PRO 7995WX 96-Cores with an extra long model name \
                    that cannot fit";
        // 800 px: the range switch also takes header width (crw.105).
        let (cx, _) = open(&mut cx, 800., long, 1);
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
            tl_ui_base::text::fonts::load_bundled_fonts(cx);
        });
        let (cx, _) = open(&mut cx, 1000., "Short", 1);
        let model = cx.debug_bounds("perf-page-model").expect("model text");
        draw(cx);
        cx.simulate_mouse_move(model.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        draw(cx);
        assert!(cx.debug_bounds("TlTooltip-text").is_none());
    }

    #[gpui_kit::test]
    fn build_page_placeholder_for_unknown_kind(cx: &mut TestAppContext) {
        use gpui_kit::{AnyView, AppContext as _};
        use tl_model::client::EngineClient;
        use tl_ui_base::demand::DemandAggregator;
        use tl_ui_base::host::HostContext;
        use tl_ui_base::hub::SnapshotHub;
        use tl_ui_perf::performance::DeviceRow;
        use tl_ui_perf::performance::page::build_page;
        use tl_ui_testkit::fake_engine::FakeEngine;

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
        let view =
            cx.update(|cx| build_page(&HostContext::default(), &key, &row, &hub, &demand, cx));
        let (_, vcx) = cx.add_window_view(move |_, _| Host(view));
        draw(vcx);
        assert!(vcx.debug_bounds("perf-page-header").is_some());
        assert!(vcx.debug_bounds("perf-graph-area").is_some());
    }

    // ---- Words through tr, cached per strings version (R-132) ----

    /// Every Performance key the details, the grid and the header read, mapped to a marker.
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

    #[gpui_kit::test]
    fn performance_words_follow_test_locale(cx: &mut TestAppContext) {
        use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
        use tl_ui_base::i18n::{init_strings, set_test_catalogue, tr_calls};
        use tl_ui_perf::performance::cpu_grid::{GridWords, cell_specs, heading_tooltip};
        use tl_ui_perf::performance::details::Fallback;
        use tl_ui_perf::performance::page::page_words;

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

    #[gpui_kit::test]
    fn page_menu_tray_item_position(cx: &mut TestAppContext) {
        use tl_ui_base::i18n::init_strings;
        use tl_ui_kit::components::menu::MenuEntryKind;
        use tl_ui_perf::performance::actions::PerfAddTrayGraph;
        use tl_ui_perf::performance::menu::{PageMenuCtx, page_menu};

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

    #[gpui_kit::test]
    fn performance_frames_do_no_tr_calls(cx: &mut TestAppContext) {
        use tl_ui_base::i18n::{init_strings, set_test_catalogue, tr_calls};

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
    use tl_ui_kit::actions::{SelectTab, bind_keys_global};
    use tl_ui_perf::performance::actions::*;
    use tl_ui_perf::performance::{DeviceKey, DeviceKind};

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
            tl_ui_table::table::actions::bind_keys_table(cx);
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
            CpuSecondaryGraph::Clocks,
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

    /// The three graph Actions with a `tl_ui_settings::settings` field load from JSON (crw.31).
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

mod tab {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Entity, FocusHandle, Modifiers, MouseButton, TestAppContext,
        VisualTestContext, point, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::demand::DemandAggregator;
    use tl_ui_base::host::HostContext;
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_kit::components::kv_grid;
    use tl_ui_perf::performance::actions::PerfToggleSidebar;
    use tl_ui_perf::performance::cpu::CpuPage;
    use tl_ui_perf::performance::page::DevicePage as _;
    use tl_ui_perf::performance::{
        DeviceKey, PAGE_PANEL_ID, PerformancePage, SIDEBAR_COLLAPSE_ID, SIDEBAR_EXPAND_ID,
        SIDEBAR_PANEL_ID, SIDEBAR_STRIP_W,
    };
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

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
        /// Every persisted state.
        persisted: Rc<RefCell<Vec<UiState>>>,
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
            tl_ui_perf::performance::overview::OVERVIEW_KEY.to_owned(),
            tl_ui_settings::settings::DeviceVisibility::Hide,
        );
        s
    }

    /// Every Global the tab needs over a fake catalog with CPU (`cpus` logical CPUs) and Memory.
    fn install(cx: &mut TestAppContext, state: UiState, cpus: u16) -> Rc<RefCell<Vec<UiState>>> {
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
        let persisted: Rc<RefCell<Vec<UiState>>> = Rc::default();
        let seen = Rc::clone(&persisted);
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
            kv_grid::init(cx);
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings: hidden_overview(),
                state,
                persist: Some(Box::new(move |_, state| {
                    seen.borrow_mut().push(state.clone())
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
        let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
        // The test installs only EngineGlobal; the hub and demand are its own.
        let host =
            HostContext::from_parts(Some(client), Some(hub), Some(demand.clone()), None, None);
        let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(&host, cx));
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
        // Above the collapse arrow, which covers the handle at 384..416.
        let y = px(200.);
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

        let last = r.persisted.borrow().last().map(|s| s.performance.nav_width);
        assert_eq!(last, Some(300));
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

    /// The page panel's left edge and width while the sidebar is hidden: right of the 12 px
    /// expand strip, to the window's right edge (tasklens-o5853).
    fn page_beside_strip(r: &mut Rig) {
        let strip = r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).expect("expand strip");
        assert_eq!(
            (strip.left(), strip.size.width),
            (px(0.), px(SIDEBAR_STRIP_W))
        );
        assert_eq!(
            strip.size.height,
            px(800.),
            "the strip has the tab's height"
        );
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert_eq!(
            (page.left(), page.size.width),
            (px(SIDEBAR_STRIP_W), px(1280. - SIDEBAR_STRIP_W))
        );
    }

    /// The last persisted `sidebar_hidden`.
    fn persisted_hidden(r: &Rig) -> Option<bool> {
        r.persisted
            .borrow()
            .last()
            .map(|s| s.performance.sidebar_hidden)
    }

    /// F9 hides the sidebar and saves `sidebar_hidden`; F9 again brings it back and saves that.
    #[gpui_kit::test]
    fn toggle_sidebar_persists(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        assert!(r.vcx.debug_bounds(SIDEBAR_COLLAPSE_ID).is_some(), "arrow");
        assert!(r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).is_none(), "no strip");

        toggle(&mut r);
        assert!(!sidebar_visible(&mut r), "hidden");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
        assert!(r.vcx.debug_bounds(SIDEBAR_COLLAPSE_ID).is_none());
        page_beside_strip(&mut r);
        let hidden = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).performance.sidebar_hidden);
        assert!(hidden, "state.performance.sidebar_hidden");
        assert_eq!(persisted_hidden(&r), Some(true));

        toggle(&mut r);
        assert!(sidebar_visible(&mut r), "shown again");
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(sidebar.size.width, px(252.));
        assert!(r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).is_none(), "no strip");
        assert_eq!(persisted_hidden(&r), Some(false));
    }

    /// A click on the collapse arrow hides the sidebar; a click on the expand strip brings it
    /// back with the same card selected (tasklens-o5853).
    #[gpui_kit::test]
    fn collapse_arrow_and_expand_strip_clicks(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let page = r.page.clone();
        let sidebar = r.vcx.update(|_, cx| page.read(cx).sidebar().clone());
        r.vcx
            .update(|_, cx| sidebar.update(cx, |s, cx| s.select(key(DeviceKey::MEMORY), cx)));
        draw(r.vcx);

        let arrow = r.vcx.debug_bounds(SIDEBAR_COLLAPSE_ID).expect("arrow");
        let panel = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(arrow.right(), panel.right(), "on the sidebar's right edge");
        assert_eq!(arrow.size, size(px(SIDEBAR_STRIP_W), px(32.)));
        assert_eq!(arrow.center().y, panel.center().y, "centred on its height");
        r.vcx.simulate_click(arrow.center(), Modifiers::none());
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert!(!sidebar_visible(&mut r), "hidden by the arrow");
        page_beside_strip(&mut r);
        assert_eq!(persisted_hidden(&r), Some(true));

        let strip = r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).expect("strip");
        r.vcx.simulate_click(strip.center(), Modifiers::none());
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert!(sidebar_visible(&mut r), "shown by the strip");
        assert_eq!(persisted_hidden(&r), Some(false));
        let selected = r.vcx.update(|_, cx| {
            (
                page.read(cx).selected().cloned(),
                sidebar.read(cx).selected().cloned(),
            )
        });
        assert_eq!(
            selected,
            (Some(key(DeviceKey::MEMORY)), Some(key(DeviceKey::MEMORY))),
            "the same card stays selected"
        );
    }

    /// The arrow's and the strip's rightmost pixel take the click too: the split's resize
    /// handle reaches 4 px left of the page panel, over both (tasklens-o5853 review).
    #[gpui_kit::test]
    fn collapse_arrow_and_expand_strip_right_edge_clicks(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        let arrow = r.vcx.debug_bounds(SIDEBAR_COLLAPSE_ID).expect("arrow");
        let boundary = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").left();
        assert_eq!(
            arrow.right(),
            boundary,
            "the arrow ends where the handle's band begins"
        );
        let at = point(arrow.right() - px(0.5), arrow.center().y);
        r.vcx.simulate_click(at, Modifiers::none());
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert!(!sidebar_visible(&mut r), "hidden by the arrow's right edge");
        assert_eq!(persisted_hidden(&r), Some(true));
        let width = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).performance.nav_width);
        assert_eq!(width, UiState::default().performance.nav_width, "no resize");

        let strip = r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).expect("strip");
        let at = point(strip.right() - px(0.5), strip.center().y);
        r.vcx.simulate_click(at, Modifiers::none());
        draw(r.vcx);
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert!(sidebar_visible(&mut r), "shown by the strip's right edge");
        assert_eq!(persisted_hidden(&r), Some(false));
    }

    /// A saved hidden sidebar stays hidden at the next start (tasklens-o5853).
    #[gpui_kit::test]
    fn hidden_sidebar_restored_at_start(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.performance.sidebar_hidden = true;
        let mut r = open(cx, state, 16, 1280., 800.);
        assert!(!sidebar_visible(&mut r), "hidden at start");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
        page_beside_strip(&mut r);

        toggle(&mut r);
        assert!(sidebar_visible(&mut r), "F9 brings it back");
        let sidebar = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("sidebar panel");
        assert_eq!(sidebar.size.width, px(252.));
    }

    /// A change of `sidebar_hidden` from outside the page, as View > Show sidebar makes on
    /// another tab, hides and shows the sidebar.
    #[gpui_kit::test]
    fn sidebar_follows_state_written_elsewhere(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        r.vcx
            .update(|_, cx| TlSettings::update(cx, |d| d.state.performance.sidebar_hidden = true));
        draw(r.vcx);
        assert!(!sidebar_visible(&mut r));
        page_beside_strip(&mut r);
        r.vcx
            .update(|_, cx| TlSettings::update(cx, |d| d.state.performance.sidebar_hidden = false));
        draw(r.vcx);
        assert!(sidebar_visible(&mut r));
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
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
        assert_eq!(
            page.left(),
            px(SIDEBAR_STRIP_W),
            "right of the expand strip"
        );
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

    #[cfg(feature = "bench")]
    #[gpui_kit::test]
    fn bench_show_both_behind_bench_feature(cx: &mut TestAppContext) {
        use tl_ui_overview::widget::metric_widget::widget_id;
        use tl_ui_perf::performance::cpu_grid::grid_id;

        let mut r = open(cx, UiState::default(), 32, 1600., 1000.);
        let cpu = cpu_page(&mut r);
        r.vcx
            .update(|_, cx| cpu.update(cx, |p, cx| p.set_bench_show_both(true, cx)));
        draw(r.vcx);
        let (overall, grid) = r.vcx.update(|_, cx| {
            let p = cpu.read(cx);
            let total = p.widget_of("total", cx).expect("the total widget");
            let id = p.container_id();
            (widget_id(id, total).leak(), grid_id(id).leak())
        });
        let overall = r.vcx.debug_bounds(overall).expect("overall chart");
        let grid = r.vcx.debug_bounds(grid).expect("grid");
        assert_eq!(overall.size.height, px(300.));
        assert!(grid.top() > overall.bottom(), "grid below the chart");
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert!(overall.left() >= page.left() && grid.right() <= page.right());
    }

    // Compact view (tasklens-qdn1r).

    use gpui_kit::{MouseDownEvent, MouseExitEvent, MouseUpEvent};
    use tl_ui_base::settings_global::SettingKey;
    use tl_ui_base::settings_global::animations_on;
    use tl_ui_kit::actions::{ToggleCompactView, ToggleSetting};
    use tl_ui_kit::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui_perf::performance::status_bar::{
        BAR_ID, THIN_EXIT_ID, THIN_H, THIN_RANGE_ID, chip_id,
    };
    use tl_ui_perf::performance::{
        COMPACT_EXIT_ID, COMPACT_EXIT_IDLE, COMPACT_EXIT_INSET, COMPACT_EXIT_SIZE,
    };
    use tl_ui_settings::settings::StatusBarPlacement;

    fn set_compact(r: &mut Rig, on: bool) {
        r.vcx
            .update(|_, cx| TlSettings::update(cx, |d| d.state.window.compact_view = on));
        draw(r.vcx);
    }

    fn saved_hidden(r: &mut Rig) -> bool {
        r.vcx
            .update(|_, cx| TlSettings::state(cx).performance.sidebar_hidden)
    }

    /// The tab in compact view, as `ToggleCompactView` in tl-ui flips it: the handler is the
    /// main window's, so the test registers the same one, and the generic `ToggleSetting`
    /// handler. `line` is `performance.compact_status_line` (tasklens-i5cj4).
    fn open_compact(cx: &mut TestAppContext, line: bool) -> Rig<'_> {
        let mut state = UiState::default();
        state.window.compact_view = true;
        cx.update(|cx| {
            cx.on_action(|_: &ToggleCompactView, cx| {
                TlSettings::update(cx, |d| {
                    d.state.window.compact_view = !d.state.window.compact_view;
                });
            });
            tl_ui_kit::actions::register_global_handlers(cx);
        });
        let mut r = open(cx, state, 16, 1280., 800.);
        if !line {
            set_line(&mut r, false);
        }
        r
    }

    fn set_line(r: &mut Rig, on: bool) {
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.compact_status_line = on)
        });
        draw(r.vcx);
    }

    fn exit_drawn(r: &mut Rig) -> bool {
        r.vcx.debug_bounds(COMPACT_EXIT_ID).is_some()
    }

    fn move_to(r: &mut Rig, x: f32, y: f32) {
        r.vcx
            .simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        draw(r.vcx);
    }

    fn wait(r: &mut Rig, ms: u64) {
        r.vcx.executor().advance_clock(Duration::from_millis(ms));
        draw(r.vcx);
    }

    /// Compact view hides the sidebar and the status bar without saving the sidebar state. The
    /// strip and F9 still show the sidebar inside it, and leaving it brings back the saved state.
    #[gpui_kit::test]
    fn compact_view_hides_sidebar_and_status_bar(cx: &mut TestAppContext) {
        let mut r = open(cx, UiState::default(), 16, 1280., 800.);
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.status_bar = StatusBarPlacement::Performance
            })
        });
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "status bar before");
        assert!(sidebar_visible(&mut r));

        // Without its status line, compact view draws no status bar (tasklens-i5cj4).
        set_line(&mut r, false);
        set_compact(&mut r, true);
        let page = r.page.clone();
        assert!(r.vcx.update(|_, cx| page.read(cx).is_compact()));
        assert!(!sidebar_visible(&mut r), "the sidebar hides");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
        page_beside_strip(&mut r);
        assert!(r.vcx.debug_bounds(BAR_ID).is_none(), "no status bar");
        assert!(!saved_hidden(&mut r), "the saved state is not touched");

        toggle(&mut r);
        assert!(sidebar_visible(&mut r), "F9 shows it inside compact view");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
        toggle(&mut r);
        assert!(!sidebar_visible(&mut r), "and hides it again");
        assert!(!saved_hidden(&mut r), "neither toggle is saved");
        assert!(
            r.persisted
                .borrow()
                .iter()
                .all(|s| !s.performance.sidebar_hidden),
            "no state.toml with a hidden sidebar"
        );

        set_compact(&mut r, false);
        assert!(sidebar_visible(&mut r), "the saved shown sidebar returns");
        assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "status bar back");
    }

    /// Started in compact view with a saved hidden sidebar: the strip shows the sidebar for now,
    /// and leaving compact view hides it again as it was saved.
    #[gpui_kit::test]
    fn compact_view_at_start_keeps_the_saved_sidebar_state(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.window.compact_view = true;
        state.performance.sidebar_hidden = true;
        let mut r = open(cx, state, 16, 1280., 800.);
        assert!(!sidebar_visible(&mut r), "hidden at start");
        let strip = r.vcx.debug_bounds(SIDEBAR_EXPAND_ID).expect("strip");
        r.vcx.simulate_click(strip.center(), Modifiers::none());
        wait(&mut r, 250);
        assert!(sidebar_visible(&mut r), "the strip shows it");
        assert!(saved_hidden(&mut r), "still saved as hidden");

        set_compact(&mut r, false);
        assert!(!sidebar_visible(&mut r), "hidden again, as saved");
    }

    /// The exit button: none until the mouse moves over the tab, then 28 × 28 in the top-right
    /// corner. It stays while the mouse keeps moving, fades out 2 s after the last move, and
    /// goes at once when the pointer leaves the window.
    #[gpui_kit::test]
    fn compact_exit_button_follows_the_mouse(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, false);
        assert!(!exit_drawn(&mut r), "no button before the mouse moves");

        move_to(&mut r, 600., 400.);
        let b = r.vcx.debug_bounds(COMPACT_EXIT_ID).expect("button");
        assert_eq!(b.size, size(px(COMPACT_EXIT_SIZE), px(COMPACT_EXIT_SIZE)));
        assert_eq!(b.top(), px(COMPACT_EXIT_INSET));
        assert_eq!(b.right(), px(1280. - COMPACT_EXIT_INSET));

        wait(&mut r, 1500);
        move_to(&mut r, 610., 400.);
        wait(&mut r, 1500);
        let page = r.page.clone();
        assert!(exit_drawn(&mut r), "1.5 s after the last move");
        assert!(!r.vcx.update(|_, cx| page.read(cx).exit_button_fading()));

        wait(&mut r, 600);
        let animated = r.vcx.update(|_, cx| animations_on(cx));
        if animated {
            assert!(
                r.vcx.update(|_, cx| page.read(cx).exit_button_fading()),
                "fading 2.1 s after the last move"
            );
            wait(&mut r, 300);
        }
        assert!(!exit_drawn(&mut r), "gone after the fade");

        // A move during the fade or after it brings it back.
        move_to(&mut r, 620., 400.);
        assert!(exit_drawn(&mut r));
        r.vcx.simulate_event(MouseExitEvent {
            position: point(px(620.), px(400.)),
            pressed_button: None,
            modifiers: Modifiers::none(),
        });
        draw(r.vcx);
        assert!(!exit_drawn(&mut r), "the pointer left the window");
        wait(&mut r, COMPACT_EXIT_IDLE.as_millis() as u64 + 500);
        assert!(!exit_drawn(&mut r));

        set_compact(&mut r, false);
        move_to(&mut r, 630., 400.);
        assert!(!exit_drawn(&mut r), "no button outside compact view");
    }

    /// Resting on the exit button keeps it; a click on it ends compact view and the sidebar
    /// comes back.
    #[gpui_kit::test]
    fn compact_exit_button_click_exits(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, false);
        move_to(&mut r, 600., 400.);
        let b = r.vcx.debug_bounds(COMPACT_EXIT_ID).expect("button");
        move_to(&mut r, f32::from(b.center().x), f32::from(b.center().y));
        wait(&mut r, 3000);
        assert!(exit_drawn(&mut r), "kept while the pointer rests on it");

        r.vcx.simulate_click(b.center(), Modifiers::none());
        draw(r.vcx);
        let compact = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).window.compact_view);
        assert!(!compact, "compact view off");
        assert!(sidebar_visible(&mut r), "the sidebar is back");
        assert!(!exit_drawn(&mut r));
    }

    fn page_menu(r: &mut Rig) -> Vec<gpui_kit::SharedString> {
        let at = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").center();
        r.vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        r.vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        r.vcx.run_until_parked();
        let menu: Entity<TlMenuPopup> = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        r.vcx.update(|window, cx| {
            let entries = menu.read(cx).entries_for_test(window);
            let last = entries.last().expect("entries");
            if last.label.as_deref() == Some("Exit compact view") {
                assert!(
                    last.action
                        .as_ref()
                        .is_some_and(|a| a.partial_eq(&ToggleCompactView)),
                    "runs ToggleCompactView"
                );
                let line = &entries[entries.len() - 2];
                assert_eq!(line.label.as_deref(), Some("Show status line"));
                assert!(
                    line.action
                        .as_ref()
                        .is_some_and(|a| a
                            .partial_eq(&ToggleSetting(SettingKey::PerformanceCompactStatusLine))),
                    "toggles performance.compact_status_line"
                );
                assert_eq!(
                    entries[entries.len() - 3].kind,
                    MenuEntryKind::Separator,
                    "after a separator"
                );
            }
            entries.into_iter().filter_map(|e| e.label).collect()
        })
    }

    /// In compact view a right-click on the page ends its menu with "Exit compact view".
    #[gpui_kit::test]
    fn compact_page_menu_offers_exit(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, true);
        let labels = page_menu(&mut r);
        assert_eq!(
            labels.last().map(|l| l.as_ref()),
            Some("Exit compact view"),
            "{labels:?}"
        );
        r.vcx
            .update(tl_ui_kit::components::menu::close_context_menu);
        set_compact(&mut r, false);
        let labels = page_menu(&mut r);
        assert!(
            !labels.iter().any(|l| l.as_ref() == "Exit compact view"),
            "{labels:?}"
        );
    }
    // The thin status line of compact view (tasklens-i5cj4).

    fn bar_chips(r: &mut Rig) -> Vec<String> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.read(cx)
                .status_bar()
                .read(cx)
                .text()
                .chips
                .iter()
                .map(|c| chip_id(c.kind))
                .collect()
        })
    }

    fn line_check(r: &mut Rig) -> bool {
        let at = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").center();
        r.vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        r.vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        r.vcx.run_until_parked();
        let menu: Entity<TlMenuPopup> = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        let checked = r.vcx.update(|window, cx| {
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .find(|e| e.label.as_deref() == Some("Show status line"))
                .expect("the status line entry")
                .checked
        });
        r.vcx
            .update(tl_ui_kit::components::menu::close_context_menu);
        draw(r.vcx);
        checked
    }

    /// With the line on, compact view draws the status bar as one line of at most 22 px at the
    /// top of the tab: the chips of the normal bar in the same order, the range button, and the
    /// exit button at the right end without a mouse move. The hover button stays away, and the
    /// line's exit button ends compact view.
    #[gpui_kit::test]
    fn compact_status_line_is_thin_with_the_exit_button(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, true);
        let bar = r.vcx.debug_bounds(BAR_ID).expect("the status line");
        assert!(bar.size.height <= px(22.), "{:?}", bar.size);
        assert_eq!(bar.size.height, px(THIN_H));
        assert_eq!(bar.top(), px(0.), "at the top of the tab");
        assert_eq!(bar.right(), px(1280.));
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert_eq!(page.top(), bar.bottom(), "the page right under the line");
        let page_entity = r.page.clone();
        assert!(
            r.vcx
                .update(|_, cx| page_entity.read(cx).status_bar().read(cx).is_thin())
        );

        // The same chips as the normal bar, left to right in its order, inside the line.
        let chips = bar_chips(&mut r);
        assert!(!chips.is_empty());
        let mut left = px(0.);
        for id in &chips {
            let b = r
                .vcx
                .debug_bounds(Box::leak(id.clone().into_boxed_str()))
                .unwrap_or_else(|| panic!("{id} drawn"));
            assert!(b.left() > left, "{id} after the one before");
            assert!(
                b.top() >= bar.top() && b.bottom() <= bar.bottom(),
                "{id} in the line"
            );
            left = b.left();
        }
        let range = r.vcx.debug_bounds(THIN_RANGE_ID).expect("range button");
        let exit = r
            .vcx
            .debug_bounds(THIN_EXIT_ID)
            .expect("exit button, no move needed");
        assert!(
            range.right() <= exit.left(),
            "the range button before the exit button"
        );
        assert!(exit.right() <= bar.right() && exit.right() >= bar.right() - px(8.));
        assert!(exit.top() >= bar.top() && exit.bottom() <= bar.bottom());

        move_to(&mut r, 600., 400.);
        assert!(!exit_drawn(&mut r), "no hover button while the line shows");
        assert!(r.vcx.debug_bounds(THIN_EXIT_ID).is_some());

        r.vcx.simulate_click(exit.center(), Modifiers::none());
        draw(r.vcx);
        assert!(
            !r.vcx
                .update(|_, cx| TlSettings::state(cx).window.compact_view),
            "compact view off"
        );
        assert!(r.vcx.debug_bounds(THIN_EXIT_ID).is_none());
        // Normal view: the full bar on the CPU page only with the `performance` placement,
        // which this test does not set, so the default `overview` draws none here.
        let page_entity = r.page.clone();
        assert!(
            !r.vcx
                .update(|_, cx| page_entity.read(cx).status_bar().read(cx).is_thin())
        );
    }

    /// With the line off, compact view draws no status line, the page starts at the top edge,
    /// and the hover exit button works as before.
    #[gpui_kit::test]
    fn compact_status_line_off_starts_the_page_at_the_top(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, false);
        assert!(r.vcx.debug_bounds(BAR_ID).is_none(), "no status line");
        assert!(r.vcx.debug_bounds(THIN_EXIT_ID).is_none());
        let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page");
        assert_eq!(page.top(), px(0.));
        move_to(&mut r, 600., 400.);
        assert!(exit_drawn(&mut r), "the hover button");
    }

    /// The right-click entry "Show status line" toggles `performance.compact_status_line`, which
    /// is saved, and the line follows it both ways.
    #[gpui_kit::test]
    fn compact_status_line_toggles_from_the_page_menu(cx: &mut TestAppContext) {
        let mut r = open_compact(cx, true);
        assert!(line_check(&mut r), "checked while on");
        r.vcx
            .dispatch_action(ToggleSetting(SettingKey::PerformanceCompactStatusLine));
        draw(r.vcx);
        assert!(
            !r.vcx
                .update(|_, cx| TlSettings::settings(cx).performance.compact_status_line),
            "off"
        );
        assert!(r.vcx.debug_bounds(BAR_ID).is_none(), "the line goes");
        assert_eq!(
            r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").top(),
            px(0.)
        );
        assert!(!line_check(&mut r), "unchecked while off");
        r.vcx
            .dispatch_action(ToggleSetting(SettingKey::PerformanceCompactStatusLine));
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "the line is back");
        assert!(r.vcx.debug_bounds(THIN_EXIT_ID).is_some());
    }
}
