//! Performance tab: the Details grid model and its rendering (E24, PERF-011 to PERF-013,
//! UI-PRF-021).

use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::validity::Validity;
use tl_ui::actions::OpenAdditionalSetup;
use tl_ui::components::kv_grid::{self, shown_text};
use tl_ui::pages::performance::details::{Fallback, KeyValueModel, fallback_word, render_details};
use tl_ui::theme::metrics;

const ROW: &str = "Speed";

/// Value, tooltip, link, and whether the action is `OpenAdditionalSetup`.
fn read(model: &KeyValueModel) -> (String, Option<String>, bool, bool) {
    let row = &model.rows[0];
    let setup = row
        .action
        .as_ref()
        .is_some_and(|a| a.partial_eq(&OpenAdditionalSetup));
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
        assert_eq!(fallback_word(validity, fallback).0, word, "{what}");

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
            shown_text(&row.value).as_deref(),
            Some(word),
            "{what}: grid"
        );
        assert_eq!(row.value.link.is_some(), link, "{what}: grid link");
    }

    // Stale keeps the value (its own test below); Valid has no word.
    assert_eq!(fallback_word(Valid, NotReported), ("", None));
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
    assert_eq!(shown_text(&row.value).as_deref(), Some("4631 MHz"));
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
        AnyElement, App, AppContext as _, Context, FocusHandle, Focusable, InteractiveElement as _,
        IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _,
        Render, SharedString, Styled as _, TestAppContext, TestDispatcher, VisualTestContext,
        Window, div, px,
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
    use tl_ui::pages::performance::page::{DevicePage, DevicePageFrame, header_text};
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
            strings(header_text(DeviceKind::Cpu, 0, &cpu, &snap)),
            ("Processor".into(), "Unknown processor".into())
        );
        assert_eq!(
            strings(header_text(DeviceKind::Net, 0, &net, &snap)),
            ("enp38s0".into(), "Unknown adapter".into())
        );
        assert_eq!(
            strings(header_text(
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
                strings(header_text(kind, n, e, &snap)),
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
        use gpui_kit::AnyView;
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
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Memory,
            idx: DeviceIdx(1),
            generation: 0,
            net_class: None,
        };
        let view = cx.update(|cx| build_page(&key, &row, &hub, &demand, cx));
        let (_, vcx) = cx.add_window_view(move |_, _| Host(view));
        draw(vcx);
        assert!(vcx.debug_bounds("perf-page-header").is_some());
        assert!(vcx.debug_bounds("perf-graph-area").is_some());
    }
}
