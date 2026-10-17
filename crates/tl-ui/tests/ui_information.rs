//! Information section entity (E28-030, UI-SYS-081, UI-SYS-082, UI-FND-025) and the page
//! (E28-033). The page tests need `--features test-hooks`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    Modifiers, MouseButton, ParentElement, Pixels, Render, SharedString, StyleRefinement, Styled,
    TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::validity::Validity;
use tl_ui::components::kv_grid::{KvWords, TlValue, UnsupportedWord, kv_words, value_look_in};
use tl_ui::i18n::init_strings;
use tl_ui::pages::information::actions::CopyInformationSection;
use tl_ui::pages::information::section::{InfoRow, InfoSection, InfoSectionId, value_copy_text};
use tl_ui::theme::{current, metrics};

type Dispatched = Rc<RefCell<Vec<InfoSectionId>>>;

fn setup(cx: &mut App, scale_pct: u16) {
    gpui_kit::init(cx);
    let appearance = AppearanceSettings {
        ui_scale_pct: scale_pct,
        ..AppearanceSettings::default()
    };
    tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
    tl_ui::components::kv_grid::init(cx);
    init_strings(cx, "en-US");
}

/// Holds one section 600 px wide and records `CopyInformationSection` dispatches.
struct Host {
    section: Entity<InfoSection>,
    dispatched: Dispatched,
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dispatched = self.dispatched.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .w(px(600.))
            .h(px(400.))
            .on_action(move |a: &CopyInformationSection, _, _| {
                dispatched.borrow_mut().push(a.section)
            })
            .child(self.section.clone().cached(StyleRefinement::default()))
    }
}

fn host(
    cx: &mut TestAppContext,
    id: InfoSectionId,
    scale_pct: u16,
    rows: Vec<InfoRow>,
) -> (Entity<InfoSection>, Dispatched, &mut VisualTestContext) {
    cx.update(|cx| setup(cx, scale_pct));
    let dispatched = Dispatched::default();
    let section = cx.new(|cx| {
        let mut s = InfoSection::new(id, cx);
        s.set_rows(Arc::from(rows), cx);
        s.set_copy_enabled(true, cx);
        s
    });
    let (_, vcx) = cx.add_window_view({
        let section = section.clone();
        let dispatched = dispatched.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Host {
                section,
                dispatched,
                focus,
            }
        }
    });
    draw(vcx);
    (section, dispatched, vcx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    draw(cx);
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn row(label: &str, text: &str) -> InfoRow {
    InfoRow {
        label: label.to_owned().into(),
        value: TlValue::new(text.to_owned()),
        live: false,
        block: 0,
        label_tooltip: None,
        label_lines: 1,
        fetch: None,
    }
}

fn with_validity(label: &str, validity: Validity) -> InfoRow {
    InfoRow {
        value: TlValue {
            validity,
            ..TlValue::new("0")
        },
        ..row(label, "0")
    }
}

fn os_rows() -> Vec<InfoRow> {
    vec![row("OS name", "CachyOS Linux"), row("Version", "Rolling")]
}

fn copy_button(cx: &mut VisualTestContext) -> Bounds<Pixels> {
    // `ElementId::from(("info-copy", 0))` prints as "info-copy-0".
    bounds(cx, "TlButton-info-copy-0").expect("copy button drawn")
}

#[gpui_kit::test]
fn info_section_band_layout(cx: &mut TestAppContext) {
    let (section, _, cx) = host(cx, InfoSectionId::OperatingSystem, 100, os_rows());
    let row_h = cx.update(|_, cx| metrics(cx).row_height);
    assert_eq!(row_h, px(28.));

    let band = bounds(cx, "InfoSection-0-band").expect("band drawn");
    assert_eq!(band.size.height, row_h, "band is row_h tall");
    assert_eq!(band.size.width, px(600.), "band spans the column");

    let bar = bounds(cx, "InfoSection-0-bar").expect("accent bar drawn");
    assert_eq!(bar.origin.x, band.origin.x, "bar at x = 0");
    assert_eq!(bar.size.width, px(3.));
    assert_eq!(bar.size.height, row_h, "bar is full band height");

    let title = bounds(cx, "InfoSection-0-title").expect("title drawn");
    assert_eq!(title.origin.x, px(3. + 8.), "title after the bar with sp4");
    assert_eq!(
        InfoSectionId::OperatingSystem.title().as_ref(),
        "Operating System"
    );

    let button = copy_button(cx);
    assert_eq!(button.size.width, px(24.));
    assert_eq!(button.size.height, px(24.));
    assert_eq!(
        button.right(),
        band.right() - px(4.),
        "copy button at the right end, sp2"
    );

    let first_row = bounds(cx, "TlKv-row-0").expect("row drawn");
    assert_eq!(
        first_row.origin.y,
        band.bottom() + px(8.),
        "sp4 between band and row"
    );
    assert_eq!(
        first_row.origin.x,
        band.origin.x + px(8.),
        "label inset sp4"
    );
    assert_eq!(cx.update(|_, cx| section.read(cx).rows().len()), 2);
}

#[gpui_kit::test]
fn info_section_titles(cx: &mut TestAppContext) {
    cx.update(|cx| setup(cx, 100));
    let titles: Vec<String> = InfoSectionId::ALL
        .iter()
        .map(|id| id.title().to_string())
        .collect();
    assert_eq!(
        titles,
        [
            "Operating System",
            "Processor",
            "System",
            "Memory",
            "Memory modules",
            "Disk drives",
            "BIOS/Firmware",
            "Motherboard",
            "Graphics",
            "Network adapters",
        ]
    );
}

#[gpui_kit::test]
fn info_os_logo_omitted_when_missing(cx: &mut TestAppContext) {
    let (section, _, cx) = host(cx, InfoSectionId::OperatingSystem, 100, os_rows());
    assert!(
        bounds(cx, "InfoSection-0-logo").is_none(),
        "no logo without a path"
    );

    cx.update(|_, cx| {
        section.update(cx, |s, cx| {
            s.set_logo(Some("/usr/share/pixmaps/archlinux-logo.png".into()), cx)
        })
    });
    let logo = bounds(cx, "InfoSection-0-logo").expect("logo drawn");
    let title = bounds(cx, "InfoSection-0-title").expect("title drawn");
    assert_eq!(logo.size.width, px(16.));
    assert_eq!(logo.size.height, px(16.));
    assert_eq!(logo.origin.x, title.right() + px(6.), "sp3 after the title");
}

#[gpui_kit::test]
fn info_row_validity_words(cx: &mut TestAppContext) {
    let unsupported = |label: &str, word| InfoRow {
        value: TlValue {
            word,
            ..with_validity(label, Validity::Unsupported).value
        },
        ..row(label, "")
    };
    let rows = vec![
        with_validity("Warming", Validity::WarmingUp),
        with_validity("Restricted", Validity::Restricted),
        unsupported("Needs setup", Some(UnsupportedWord::NeedsSetup)),
        unsupported("Not supported", Some(UnsupportedWord::NotSupported)),
        unsupported("Not reported", Some(UnsupportedWord::NotReported)),
        with_validity("Failed", Validity::Failed),
    ];
    let expected = [
        "Collecting…",
        "Restricted",
        "Needs setup",
        "Not supported",
        "Not reported",
        "Unavailable",
    ];
    let (_, _, cx) = host(cx, InfoSectionId::System, 100, rows.clone());
    let (words, theme) = cx.update(|_, cx| (kv_words(cx), current(cx).clone()));
    for (n, (row, word)) in rows.iter().zip(expected).enumerate() {
        let look = value_look_in(&row.value, &words, &theme).expect("drawn");
        assert_eq!(look.text.as_ref(), word, "{}", row.label);
        assert_eq!(look.color, theme.text_secondary, "{}", row.label);
        let mut copy = String::new();
        value_copy_text(row, &mut copy);
        assert_eq!(copy, word, "{} copy", row.label);
        assert!(
            bounds(cx, &format!("TlKv-value-{n}")).is_some(),
            "{} drawn",
            row.label
        );
    }
}

#[gpui_kit::test]
fn info_block_indent_12(cx: &mut TestAppContext) {
    let mut sub = row("Slot", "P0 CHANNEL A");
    sub.block = 1;
    let rows = vec![row("Total", "32 GiB"), sub];
    let (_, _, cx) = host(cx, InfoSectionId::MemoryModules, 100, rows);
    let indent = |cx: &mut VisualTestContext| {
        let top = bounds(cx, "TlKv-row-0").expect("block 0 row");
        let sub = bounds(cx, "TlKv-row-1").expect("block 1 row");
        sub.origin.x - top.origin.x
    };
    assert_eq!(indent(cx), px(12.), "indent at 100%");

    cx.update(|_, cx| {
        let appearance = AppearanceSettings {
            ui_scale_pct: 150,
            ..AppearanceSettings::default()
        };
        tl_ui::theme::metrics::recompute(&appearance, cx);
        cx.refresh_windows();
    });
    assert_eq!(indent(cx), px(18.), "indent at 150%");
}

#[gpui_kit::test]
fn info_multiline_row_height_and_copy_join(cx: &mut TestAppContext) {
    let two = row("Resolution", "a\nb");
    let mut copy = String::new();
    value_copy_text(&two, &mut copy);
    assert_eq!(copy, "a, b");

    let (_, _, cx) = host(cx, InfoSectionId::Graphics, 100, vec![two]);
    let r = bounds(cx, "TlKv-row-0").expect("row drawn");
    assert_eq!(r.size.height, px(56.), "two lines are 2 × row_h");
}

#[gpui_kit::test]
fn info_copy_button_dispatches_section(cx: &mut TestAppContext) {
    let (_, dispatched, cx) = host(cx, InfoSectionId::OperatingSystem, 100, os_rows());
    let button = copy_button(cx);
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        dispatched.borrow().as_slice(),
        &[InfoSectionId::OperatingSystem]
    );
}

#[gpui_kit::test]
fn info_copy_button_disabled_without_data(cx: &mut TestAppContext) {
    let (section, dispatched, cx) = host(cx, InfoSectionId::OperatingSystem, 100, os_rows());
    cx.update(|_, cx| section.update(cx, |s, cx| s.set_copy_enabled(false, cx)));
    let button = copy_button(cx);
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        dispatched.borrow().is_empty(),
        "a disabled button dispatches nothing"
    );
}

#[gpui_kit::test]
fn info_row_menu_copy_section_dispatches(cx: &mut TestAppContext) {
    let (_, dispatched, cx) = host(cx, InfoSectionId::Processor, 100, os_rows());
    let value = bounds(cx, "TlKv-value-1").expect("value drawn");
    cx.simulate_mouse_down(value.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(value.center(), MouseButton::Right, Modifiers::none());
    draw(cx);
    // Copy value, Copy row, Copy section; the first "down" selects the first item.
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(dispatched.borrow().as_slice(), &[InfoSectionId::Processor]);
}

#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn info_set_rows_same_content_no_rerender(cx: &mut TestAppContext) {
    let (section, _, cx) = host(cx, InfoSectionId::OperatingSystem, 100, os_rows());
    let before = cx.update(|_, cx| section.read(cx).render_count());
    assert!(before > 0, "rendered once");

    cx.update(|_, cx| section.update(cx, |s, cx| s.set_rows(Arc::from(os_rows()), cx)));
    draw(cx);
    assert_eq!(
        cx.update(|_, cx| section.read(cx).render_count()),
        before,
        "equal rows in a new Arc do not re-render"
    );

    let mut changed = os_rows();
    changed[1].value = TlValue::new("2026.10");
    cx.update(|_, cx| section.update(cx, |s, cx| s.set_rows(Arc::from(changed), cx)));
    draw(cx);
    assert!(
        cx.update(|_, cx| section.read(cx).render_count()) > before,
        "new content re-renders"
    );
}

#[test]
fn copy_text_of_valid_and_missing() {
    let mut out = String::from("OS name: ");
    value_copy_text(&row("OS name", "CachyOS Linux"), &mut out);
    assert_eq!(out, "OS name: CachyOS Linux");

    let mut out = String::new();
    value_copy_text(&with_validity("Gone", Validity::Missing), &mut out);
    assert!(out.is_empty(), "Missing rows copy nothing");

    let words = KvWords::default();
    assert_eq!(words.restricted.as_ref(), "Restricted");
    let _: SharedString = InfoSectionId::Processor.title();
}

/// Row builders for OS, Processor, System, BIOS/Firmware and Motherboard (E28-031).
mod row_builders {
    use super::*;
    use tl_model::devices::DeviceCatalog;
    use tl_model::snapshot::{FirmwareType, InvField, InventorySnapshot, SystemSnapshot};
    use tl_model::system::Metric;
    use tl_model::table::Total;
    use tl_ui::pages::information::sections::{
        InfoInputs, InfoWords, apply_hints, build_bios, build_motherboard, build_os,
        build_processor, build_system, find_logo,
    };

    fn ok<T>(value: T) -> InvField<T> {
        Total {
            value,
            validity: Validity::Valid,
        }
    }

    fn s(text: &str) -> InvField<Arc<str>> {
        ok(Arc::from(text))
    }

    fn with<T: Default>(validity: Validity) -> InvField<T> {
        Total {
            value: T::default(),
            validity,
        }
    }

    /// This machine's values (UI-SYS-086 to UI-SYS-088 examples).
    pub(super) fn inventory() -> InventorySnapshot {
        let mut inv = InventorySnapshot::default();
        let os = &mut inv.os;
        os.name = s("CachyOS Linux");
        os.version = with(Validity::Unsupported);
        os.build = with(Validity::Unsupported);
        os.kernel = s("6.17.1-2-cachyos");
        os.kernel_version = s("#1 SMP PREEMPT_DYNAMIC Mon, 06 Oct 2025 10:00:00 +0000");
        os.architecture = s("x86_64");
        os.install_date_s = ok(1_741_976_525);
        os.package_manager = s("pacman 7.0.0");
        os.machine_id = s("0123456789abcdef0123456789abcdef");

        let p = &mut inv.processor;
        p.name = s("AMD Ryzen 9 7950X3D 16-Core Processor");
        p.vendor = s("AuthenticAMD");
        p.architecture = s("x86_64");
        p.family = ok(25);
        p.model = ok(97);
        p.stepping = ok(2);
        p.cores = ok(16);
        p.logical = ok(32);
        p.base_mhz = ok(4200);
        p.max_mhz = ok(5759);
        p.microcode = s("0xa601209");
        p.virtualization = s("svm");
        p.avx512 = ok(true);
        p.bits = ok(64);

        let sy = &mut inv.system;
        sy.pretty_hostname = s("Hawk desktop");
        sy.static_hostname = s("hawk");
        sy.user_name = s("oystein");
        sy.domain = None;
        sy.boot_time_s = ok(1_759_302_760);
        sy.desktop = s("KDE Plasma 6.4.5");
        sy.windowing_system = s("Wayland");
        sy.session_type = s("wayland");
        sy.virtual_terminal = s("2");

        let f = &mut inv.firmware;
        f.firmware_type = ok(Some(FirmwareType::Uefi));
        f.bios_vendor = s("American Megatrends International, LLC.");
        f.bios_version = s("3001");
        f.bios_date = s("2025-02-14");
        f.bios_release = s("5.35");
        f.secure_boot = ok(true);

        let b = &mut inv.board;
        b.manufacturer = s("ASUSTeK COMPUTER INC.");
        b.product = s("ROG STRIX X670E-E GAMING WIFI");
        b.version = s("Rev 1.xx");
        b.system_vendor = s("ASUS");
        b.system_product = s("System Product Name");
        b.chassis_type = s("3");
        b.serial = with(Validity::Restricted);
        inv
    }

    pub(super) fn system(uptime_s: u64) -> SystemSnapshot {
        let mut sys = SystemSnapshot::default();
        sys.header.observed_end = uptime_s * 1_000_000_000;
        sys.header.validity = Validity::Valid;
        sys.cpu.processes = Metric::valid(412);
        sys.cpu.threads = Metric::valid(5596);
        sys.cpu.handles = Metric::valid(253_248);
        sys
    }

    fn rows(
        inv: &InventorySnapshot,
        sys: &SystemSnapshot,
        build: fn(&InfoInputs, &mut Vec<InfoRow>),
    ) -> Vec<InfoRow> {
        let words = InfoWords::default();
        let devices = DeviceCatalog::default();
        let inputs = InfoInputs {
            inv,
            sys,
            devices: &devices,
            words: &words,
        };
        let mut out = Vec::new();
        build(&inputs, &mut out);
        out
    }

    /// Label and shown text of each row, in en-US.
    fn shown(rows: &[InfoRow]) -> Vec<(String, String)> {
        let words = KvWords::default();
        rows.iter()
            .map(|r| {
                let text = tl_ui::components::kv_grid::shown_text_in(&r.value, &words)
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                (r.label.to_string(), text)
            })
            .collect()
    }

    fn value_of(rows: &[InfoRow], label: &str) -> Option<String> {
        shown(rows)
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v)
    }

    fn local(unix_s: i64) -> String {
        let z = jiff::Timestamp::from_second(unix_s)
            .expect("time in range")
            .to_zoned(jiff::tz::TimeZone::system());
        z.strftime("%Y-%m-%d %H:%M:%S").to_string()
    }

    #[test]
    fn info_os_rows_golden() {
        let inv = inventory();
        let got = shown(&rows(&inv, &system(0), build_os));
        let expected = [
            ("OS name", "CachyOS Linux".to_owned()),
            ("Version", "Rolling".to_owned()),
            ("Build", "Not reported".to_owned()),
            ("Kernel", "6.17.1-2-cachyos".to_owned()),
            (
                "Kernel version",
                "#1 SMP PREEMPT_DYNAMIC Mon, 06 Oct 2025 10:00:00 +0000".to_owned(),
            ),
            ("Architecture", "x86_64 (64-bit)".to_owned()),
            ("Install date", local(1_741_976_525)),
            ("Package manager", "pacman 7.0.0".to_owned()),
            ("Machine ID", "0123456789abcdef0123456789abcdef".to_owned()),
        ];
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(l, v)| (l.to_owned(), v))
            .collect();
        assert_eq!(got, expected);
        assert!(
            rows(&inv, &system(0), build_os).iter().all(|r| !r.live),
            "no OS row is live"
        );
    }

    #[gpui_kit::test]
    fn info_os_version_rolling_and_build_not_reported(cx: &mut TestAppContext) {
        cx.update(|cx| setup(cx, 100));
        let rows = rows(&inventory(), &system(0), build_os);
        assert_eq!(rows[1].label.as_ref(), "Version");
        assert_eq!(rows[1].value.text.as_ref(), "Rolling");
        assert_eq!(rows[1].value.validity, Validity::Valid);

        let (words, theme) = cx.update(|cx| (kv_words(cx), current(cx).clone()));
        let build = &rows[2];
        assert_eq!(build.label.as_ref(), "Build");
        let look = value_look_in(&build.value, &words, &theme).expect("drawn");
        assert_eq!(look.text.as_ref(), "Not reported");
        assert_eq!(look.color, theme.text_secondary);

        let mut inv = inventory();
        inv.os.version = s("2026.10");
        inv.os.build = s("rolling-20261001");
        let rows = shown(&super::row_builders::rows(&inv, &system(0), build_os));
        assert_eq!(rows[1].1, "2026.10");
        assert_eq!(rows[2].1, "rolling-20261001");
    }

    #[test]
    fn info_os_package_manager_hidden_when_missing() {
        for missing in [with(Validity::Unsupported), with(Validity::Missing), s("")] {
            let mut inv = inventory();
            inv.os.package_manager = missing;
            let rows = rows(&inv, &system(0), build_os);
            assert_eq!(rows.len(), 8);
            assert!(value_of(&rows, "Package manager").is_none());
        }
    }

    #[test]
    fn info_cpu_flags_summary_format() {
        let cases = [
            ("svm", true, "Virtualization: Yes (AMD-V), AVX-512: Yes"),
            ("vmx", false, "Virtualization: Yes (VT-x), AVX-512: No"),
            ("", false, "Virtualization: No, AVX-512: No"),
        ];
        for (virt, avx, expected) in cases {
            let mut inv = inventory();
            inv.processor.virtualization = s(virt);
            inv.processor.avx512 = ok(avx);
            let rows = rows(&inv, &system(0), build_processor);
            assert_eq!(
                value_of(&rows, "Flags summary").as_deref(),
                Some(expected),
                "{virt}"
            );
        }

        let rows = shown(&rows(&inventory(), &system(0), build_processor));
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Name",
                "Vendor",
                "Architecture",
                "Identifier",
                "Cores",
                "Logical processors",
                "Base speed",
                "Max speed",
                "Microcode",
                "Flags summary"
            ]
        );
        assert_eq!(rows[3].1, "Family 25 Model 97 Stepping 2");
        assert_eq!(rows[6].1, "4200 MHz");
        assert_eq!(rows[7].1, "5759 MHz");

        let mut inv = inventory();
        inv.processor.name = with(Validity::Unsupported);
        let rows = super::row_builders::rows(&inv, &system(0), build_processor);
        assert_eq!(rows[0].value.text.as_ref(), "Unknown processor");
        assert_eq!(rows[0].value.validity, Validity::Valid);
    }

    #[test]
    fn info_system_computer_name_format() {
        let inv = inventory();
        let rows = rows(&inv, &system(0), build_system);
        assert_eq!(
            value_of(&rows, "Computer name").as_deref(),
            Some("Hawk desktop (hawk)")
        );

        for pretty in [s("hawk"), s(""), with(Validity::Unsupported)] {
            let mut inv = inventory();
            inv.system.pretty_hostname = pretty;
            let rows = super::row_builders::rows(&inv, &system(0), build_system);
            assert_eq!(value_of(&rows, "Computer name").as_deref(), Some("hawk"));
        }
    }

    #[test]
    fn info_domain_hidden_when_none() {
        for domain in ["", "(none)"] {
            let mut inv = inventory();
            inv.system.domain = Some(Arc::from(domain));
            let rows = rows(&inv, &system(0), build_system);
            assert!(value_of(&rows, "Domain").is_none(), "{domain:?}");
        }
        let mut inv = inventory();
        inv.system.domain = Some(Arc::from("lan"));
        let rows = rows(&inv, &system(0), build_system);
        assert_eq!(value_of(&rows, "Domain").as_deref(), Some("lan"));
        assert_eq!(rows[2].label.as_ref(), "Domain", "after User name");
    }

    #[test]
    fn info_counts_grouped() {
        let rows = rows(&inventory(), &system(9495), build_system);
        assert_eq!(value_of(&rows, "Process count").as_deref(), Some("412"));
        assert_eq!(
            value_of(&rows, "Thread count").as_deref(),
            Some("5\u{2009}596")
        );
        let handles = rows
            .iter()
            .find(|r| r.label.as_ref() == "Handle count")
            .expect("handle row");
        assert_eq!(handles.value.text.as_ref(), "253\u{2009}248");
        assert_eq!(
            handles.label_tooltip.as_deref(),
            Some("Open file descriptors, system-wide")
        );
        let live: Vec<&str> = rows
            .iter()
            .filter(|r| r.live)
            .map(|r| r.label.as_ref())
            .collect();
        assert_eq!(
            live,
            [
                "System uptime",
                "Process count",
                "Thread count",
                "Handle count"
            ]
        );

        let mut sys = system(9495);
        sys.cpu.threads = Metric {
            value: 5596,
            validity: Validity::Stale,
        };
        sys.cpu.handles = Metric {
            value: 0,
            validity: Validity::WarmingUp,
        };
        let rows = super::row_builders::rows(&inventory(), &sys, build_system);
        assert_eq!(
            value_of(&rows, "Thread count").as_deref(),
            Some("5\u{2009}596"),
            "Stale keeps the last value"
        );
        assert_eq!(
            value_of(&rows, "Handle count").as_deref(),
            Some("Collecting…")
        );

        let labels: Vec<String> = shown(&rows).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            labels,
            [
                "Computer name",
                "User name",
                "System uptime",
                "Boot time",
                "Process count",
                "Thread count",
                "Handle count",
                "Desktop",
                "Windowing system",
                "Session type",
                "Virtual terminal"
            ]
        );
        assert_eq!(value_of(&rows, "Boot time"), Some(local(1_759_302_760)));
    }

    #[test]
    fn info_session_type_with_class() {
        let session = |class: InvField<Arc<str>>, kind: InvField<Arc<str>>| {
            let mut inv = inventory();
            inv.system.session_class = class;
            inv.system.session_type = kind;
            let rows = rows(&inv, &system(0), build_system);
            value_of(&rows, "Session type")
        };
        for (class, kind, expected) in [
            ("user", "wayland", "User (graphical)"),
            ("user", "x11", "User (graphical)"),
            ("greeter", "mir", "Greeter (graphical)"),
            ("user", "tty", "User (text)"),
            ("lock-screen", "unspecified", "Lock screen (unspecified)"),
            ("manager", "wayland", "manager (graphical)"),
        ] {
            assert_eq!(
                session(s(class), s(kind)).as_deref(),
                Some(expected),
                "{class} {kind}"
            );
        }
        assert_eq!(
            session(with(Validity::Unsupported), s("wayland")).as_deref(),
            Some("wayland")
        );
        assert_eq!(session(s(""), s("wayland")).as_deref(), Some("wayland"));
        assert_eq!(
            session(s("user"), with(Validity::Unsupported)).as_deref(),
            Some("Not reported")
        );
    }

    #[test]
    fn info_system_optional_rows_hidden() {
        let mut inv = inventory();
        inv.system.desktop = with(Validity::Unsupported);
        inv.system.virtual_terminal = s("");
        let rows = rows(&inv, &system(0), build_system);
        assert!(value_of(&rows, "Desktop").is_none());
        assert!(value_of(&rows, "Virtual terminal").is_none());
    }

    #[test]
    fn info_uptime_format() {
        for (secs, expected) in [
            (2295, "0h 38m 15s"),
            (95_895, "1 day, 2h 38m 15s"),
            (181_095, "2 days, 2h 18m 15s"),
        ] {
            let rows = rows(&inventory(), &system(secs), build_system);
            assert_eq!(
                value_of(&rows, "System uptime").as_deref(),
                Some(expected),
                "{secs}"
            );
        }
    }

    #[test]
    fn info_bios_rows() {
        let rows = shown(&rows(&inventory(), &system(0), build_bios));
        let expected = [
            ("Firmware type", "UEFI"),
            ("BIOS vendor", "American Megatrends International, LLC."),
            ("BIOS version", "3001"),
            ("BIOS date", "2025-02-14"),
            ("BIOS release", "5.35"),
            ("Secure Boot", "Enabled"),
        ];
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(l, v)| (l.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(rows, expected);

        let mut inv = inventory();
        inv.firmware.firmware_type = ok(Some(FirmwareType::LegacyBios));
        inv.firmware.secure_boot = with(Validity::Unsupported);
        let rows = super::row_builders::rows(&inv, &system(0), build_bios);
        assert_eq!(
            value_of(&rows, "Firmware type").as_deref(),
            Some("Legacy BIOS")
        );
        assert_eq!(
            value_of(&rows, "Secure Boot").as_deref(),
            Some("Not reported")
        );
    }

    #[test]
    fn info_board_serial_restricted() {
        let rows = rows(&inventory(), &system(0), build_motherboard);
        let serial = rows.last().expect("serial row");
        assert_eq!(serial.label.as_ref(), "Serial number");
        assert_eq!(serial.value.validity, Validity::Restricted);
        assert_eq!(
            serial.value.tooltip.as_deref(),
            Some("Needs administrator access.")
        );
        assert_eq!(
            value_of(&rows, "Serial number").as_deref(),
            Some("Restricted")
        );
        let labels: Vec<String> = shown(&rows).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            labels,
            [
                "Manufacturer",
                "Product",
                "Version",
                "System vendor",
                "System product",
                "Chassis type",
                "Serial number"
            ]
        );

        let mut inv = inventory();
        inv.board.serial = s("230912345678");
        let rows = super::row_builders::rows(&inv, &system(0), build_motherboard);
        assert!(rows.last().unwrap().value.tooltip.is_none());
    }

    #[test]
    fn info_restricted_tooltip_with_helper() {
        let mut inv = inventory();
        inv.os.machine_id = with(Validity::Restricted);
        let mut rows = rows(&inv, &system(0), build_os);
        apply_hints(&mut rows, &InfoWords::default(), true);
        let id = rows.iter().find(|r| r.label.as_ref() == "Machine ID");
        assert_eq!(
            id.expect("machine id row").value.tooltip.as_deref(),
            Some("Needs administrator access. Click the shield to grant it.")
        );
    }

    #[test]
    fn info_restricted_tooltip_without_helper() {
        let mut inv = inventory();
        inv.os.machine_id = with(Validity::Restricted);
        let mut rows = rows(&inv, &system(0), build_os);
        apply_hints(&mut rows, &InfoWords::default(), false);
        let id = rows.iter().find(|r| r.label.as_ref() == "Machine ID");
        assert_eq!(
            id.expect("machine id row").value.tooltip.as_deref(),
            Some("Needs administrator access.")
        );
    }

    #[test]
    fn info_board_serial_tooltip_never_shield() {
        let mut rows = rows(&inventory(), &system(0), build_motherboard);
        apply_hints(&mut rows, &InfoWords::default(), true);
        let serial = rows.last().expect("serial row");
        assert_eq!(serial.value.validity, Validity::Restricted);
        assert_eq!(
            serial.value.tooltip.as_deref(),
            Some("Needs administrator access.")
        );
    }

    #[test]
    fn info_chassis_words() {
        for (n, word) in [
            ("3", "Desktop"),
            ("9", "Laptop"),
            ("35", "Mini PC"),
            ("99", "Unknown"),
        ] {
            let mut inv = inventory();
            inv.board.chassis_type = s(n);
            let rows = rows(&inv, &system(0), build_motherboard);
            assert_eq!(
                value_of(&rows, "Chassis type").as_deref(),
                Some(word),
                "{n}"
            );
        }
    }

    #[test]
    fn info_os_logo_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(find_logo("cachyos", &[root]).is_none(), "not found");

        let pixmap = root.join("pixmaps/cachyos.png");
        std::fs::create_dir_all(pixmap.parent().unwrap()).unwrap();
        std::fs::write(&pixmap, b"png").unwrap();
        assert_eq!(find_logo("cachyos", &[root]), Some(pixmap));

        let svg = root.join("icons/hicolor/scalable/apps/cachyos.svg");
        std::fs::create_dir_all(svg.parent().unwrap()).unwrap();
        std::fs::write(&svg, b"<svg/>").unwrap();
        assert_eq!(
            find_logo("cachyos", &[root]),
            Some(svg),
            "hicolor before pixmaps"
        );

        assert!(find_logo("../cachyos", &[root]).is_none());
        assert!(find_logo("", &[root]).is_none());
    }
}

/// Copy button name and hover, row label tooltips (tasklens-9o0.34).
mod names_and_tips {
    use super::*;
    use gpui_kit::point;
    use tl_ui::components::tooltip::DELAY_CONTROL;

    #[gpui_kit::test]
    fn info_copy_button_accessible_name(cx: &mut TestAppContext) {
        let (section, _, cx) = host(cx, InfoSectionId::Processor, 100, os_rows());
        draw(cx);
        assert_eq!(
            cx.update(|_, cx| section.read(cx).copy_button_name().clone())
                .as_ref(),
            "Copy Processor"
        );
        if let Some(tree) = cx.update(|window, _| window.debug_a11y_tree_json()) {
            assert!(tree.contains("Copy Processor"), "the a11y tree names it");
        }
    }

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn info_copy_button_name_follows_strings(cx: &mut TestAppContext) {
        use tl_ui::i18n::set_test_catalogue;

        let (section, _, cx) = host(cx, InfoSectionId::Processor, 100, os_rows());
        draw(cx);
        cx.update(|window, cx| {
            set_test_catalogue(
                cx,
                "information-section-processor = <cpu>\ninformation-copy-section-name = <copy {$section}>\n",
            );
            window.refresh();
        });
        cx.update(|_, cx| section.update(cx, |_, cx| cx.notify()));
        draw(cx);
        assert_eq!(
            cx.update(|_, cx| section.read(cx).copy_button_name().clone())
                .as_ref(),
            "<copy <cpu>>",
            "translated once per strings version"
        );
    }

    #[gpui_kit::test]
    fn info_handle_count_label_tooltip(cx: &mut TestAppContext) {
        let rows = vec![
            row("Thread count", "5 596"),
            InfoRow {
                label_tooltip: Some("Open file descriptors, system-wide".into()),
                ..row("Handle count", "253 248")
            },
        ];
        let (_, _, cx) = host(cx, InfoSectionId::System, 100, rows);
        cx.update(|window, _| window.activate_window());
        draw(cx);
        assert!(
            bounds(cx, "TlKv-label-0").is_none(),
            "a row without a label tooltip has no trigger"
        );
        let label = bounds(cx, "TlKv-label-1").expect("label with a tooltip");
        assert!(
            cx.debug_bounds("TlTooltip-text").is_none(),
            "no tooltip before hover"
        );

        cx.simulate_mouse_move(point(px(590.), px(390.)), None, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_move(label.center(), None, Modifiers::none());
        cx.executor()
            .advance_clock(DELAY_CONTROL - std::time::Duration::from_millis(50));
        cx.run_until_parked();
        draw(cx);
        assert!(
            cx.debug_bounds("TlTooltip-text").is_none(),
            "not before the control delay"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        draw(cx);
        draw(cx);
        assert!(
            cx.debug_bounds("TlTooltip-text").is_some(),
            "shown after the control delay"
        );
    }
}

/// Memory, Memory modules, Disk drives, Graphics and Network adapters rows (tasklens-9o0.23).
mod hardware_builders {
    use super::*;
    use tl_model::devices::DeviceCatalog;
    use tl_model::ids::DeviceIdx;
    use tl_model::snapshot::{
        DiskDrive, DisplayConnector, DisplayMode, GraphicsAdapter, InvField, InventorySnapshot,
        MemoryModule, NetworkAdapter, PublicIpState, SystemSnapshot,
    };
    use tl_model::system::{GpuTotals, Metric};
    use tl_model::table::Total;
    use tl_ui::pages::information::sections::{
        BLOCK_FIRST_LABELS, InfoInputs, InfoWords, build_bios, build_disks, build_graphics,
        build_memory, build_modules, build_motherboard, build_network, build_os, build_processor,
        build_system,
    };

    const GIB: u64 = 1 << 30;

    fn ok<T>(value: T) -> InvField<T> {
        Total {
            value,
            validity: Validity::Valid,
        }
    }

    fn s(text: &str) -> InvField<Arc<str>> {
        ok(Arc::from(text))
    }

    fn module(slot: &str) -> MemoryModule {
        MemoryModule {
            slot: s(slot),
            capacity_bytes: ok(32 * GIB),
            speed_mts: ok(6000),
            kind: s("DDR5"),
            form_factor: s("DIMM"),
            manufacturer: s("Kingston"),
            part_number: s("KF560C30-32"),
            configured_speed_mts: ok(4800),
            rank: ok(2),
            configured_voltage_mv: ok(1100),
        }
    }

    fn disk(device: u16, free: u64, volumes: &[&str]) -> DiskDrive {
        DiskDrive {
            device: Some(DeviceIdx(device)),
            name: Some(Arc::from("nvme0n1")),
            model: s("Samsung SSD 990 PRO 2TB"),
            kind: s("NVMe"),
            interface: s("NVMe"),
            capacity_bytes: ok(2_000_398_934_016),
            free_bytes: ok(free),
            volumes: volumes.iter().map(|v| Arc::from(*v)).collect(),
            serial: s("S6Z2NF0W123456"),
        }
    }

    /// This machine's values (UI-SYS-089 to UI-SYS-095 examples).
    pub(super) fn inventory() -> InventorySnapshot {
        let mut inv = InventorySnapshot::default();
        let m = &mut inv.memory;
        m.total_bytes = ok(67_324_510_208);
        m.page_size = ok(4096);
        m.huge_page_size = ok(2 << 20);
        m.swap_total_bytes = ok(64 * GIB);
        m.zram_bytes = ok(33_661_255_680);
        m.peak_committed_bytes = ok(51_561_123_922);
        inv.modules = vec![
            module("P0 CHANNEL A (DIMM 1)"),
            module("P0 CHANNEL B (DIMM 1)"),
        ]
        .into();
        inv.disks = vec![disk(4, 872_308_051_558, &["/", "/home"])].into();
        inv.graphics = vec![GraphicsAdapter {
            device: Some(DeviceIdx(7)),
            name: s("NVIDIA GeForce RTX 4090"),
            temperature_source: s("NVML (GPU core)"),
            dedicated_bytes: ok(24 * GIB),
            displays: vec![
                DisplayMode {
                    width: 2560,
                    height: 1440,
                    refresh_mhz: 59_951,
                    primary: false,
                    monitor_name: None,
                    connector: DisplayConnector::default(),
                    max_bpc: None,
                    rotation: None,
                    scale: None,
                },
                DisplayMode {
                    width: 3840,
                    height: 2160,
                    refresh_mhz: 144_000,
                    primary: true,
                    monitor_name: None,
                    connector: DisplayConnector::default(),
                    max_bpc: None,
                    rotation: None,
                    scale: None,
                },
            ]
            .into(),
            ..GraphicsAdapter::default()
        }]
        .into();
        inv.network = vec![NetworkAdapter {
            device: Some(DeviceIdx(9)),
            connection: s("enp6s0 (Ethernet)"),
            adapter_name: s("Intel Ethernet Controller I225-V"),
            driver: s("igc"),
            link_speed_mbps: ok(2500),
            ip_addresses: vec![
                Arc::from("192.168.1.10/24"),
                Arc::from("fe80::1c2:3ff:fe4a:5b6c/64"),
            ]
            .into(),
            mac: s("a8:a1:59:12:34:56"),
        }]
        .into();
        inv
    }

    pub(super) fn system() -> SystemSnapshot {
        let mut sys = SystemSnapshot::default();
        sys.header.validity = Validity::Valid;
        let m = &mut sys.memory;
        m.validity = Validity::Valid;
        m.total = 67_324_510_208;
        m.available = 30_537_592_258;
        m.used = 36_759_182_573;
        m.committed = 49_585_198_202;
        m.commit_limit = 72_112_843_243;
        sys.gpus = vec![(
            DeviceIdx(7),
            GpuTotals {
                device: DeviceIdx(7),
                util_pct: Metric::default(),
                mem_util_pct: Metric::default(),
                mem_used: Metric::default(),
                mem_total: Metric::default(),
                shared_used: Metric::default(),
                shared_total: Metric::default(),
                enc_pct: Metric::default(),
                dec_pct: Metric::default(),
                mem_reserved: Metric::default(),
                enc_sessions: Metric::default(),
                enc_dec_shared: false,
                temp_c: Metric::valid(52.2),
                temp_edge_c: Metric::default(),
                temp_junction_c: Metric::default(),
                temp_mem_c: Metric::default(),
                power_w: Metric::default(),
                power_limit_w: Metric::default(),
                clock_gfx_mhz: Metric::default(),
                clock_mem_mhz: Metric::default(),
                fan_pct: Metric::default(),
                pcie_rx_bps: Metric::default(),
                pcie_tx_bps: Metric::default(),
                engines: Default::default(),
                fdinfo_partial: false,
                energy_since_boot: Metric::default(),
                clock_video_mhz: Metric::default(),
                pstate: Metric::default(),
                fan_rpm: Metric::default(),
                pcie_gen: Metric::default(),
                pcie_width: Metric::default(),
                info: Arc::default(),
            },
        )]
        .into();
        sys
    }

    fn rows(
        inv: &InventorySnapshot,
        sys: &SystemSnapshot,
        build: fn(&InfoInputs, &mut Vec<InfoRow>),
    ) -> Vec<InfoRow> {
        let words = InfoWords::default();
        let devices = DeviceCatalog::default();
        let inputs = InfoInputs {
            inv,
            sys,
            devices: &devices,
            words: &words,
        };
        let mut out = Vec::new();
        build(&inputs, &mut out);
        out
    }

    /// Block, label and shown text of each row, in en-US.
    fn shown(rows: &[InfoRow]) -> Vec<(u8, String, String)> {
        let words = KvWords::default();
        rows.iter()
            .map(|r| {
                let text = tl_ui::components::kv_grid::shown_text_in(&r.value, &words)
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                (r.block, r.label.to_string(), text)
            })
            .collect()
    }

    fn value_of(rows: &[InfoRow], label: &str) -> String {
        shown(rows)
            .into_iter()
            .find(|(_, l, _)| l == label)
            .map(|(_, _, v)| v)
            .unwrap_or_else(|| panic!("no {label} row"))
    }

    #[test]
    fn info_memory_rows_golden() {
        let got = rows(&inventory(), &system(), build_memory);
        let want = [
            ("Total physical", "62.70 GiB", false),
            ("Available physical", "28.44 GiB", true),
            ("Memory usage", "54.6%", true),
            ("Committed", "46.18 GiB", true),
            ("Commit limit", "67.16 GiB", false),
            ("Peak committed (since tasklens started)", "48.02 GiB", true),
            ("Page size", "4096 bytes", false),
            ("Huge page size", "2.00 MiB", false),
            ("Swap total", "64.00 GiB", false),
            ("zram size", "31.35 GiB", false),
        ];
        let shown = shown(&got);
        assert_eq!(shown.len(), want.len());
        for ((row, (_, label, text)), (l, t, live)) in got.iter().zip(&shown).zip(want) {
            assert_eq!((label.as_str(), text.as_str()), (l, t));
            assert_eq!(row.live, live, "{l} live");
            assert_eq!(row.block, 0);
        }
    }

    #[test]
    fn info_memory_zram_none() {
        let mut inv = inventory();
        inv.memory.zram_bytes = ok(0);
        let got = rows(&inv, &system(), build_memory);
        assert_eq!(value_of(&got, "zram size"), "None");
        let zram = got
            .iter()
            .find(|r| r.label.as_ref() == "zram size")
            .unwrap();
        assert_eq!(zram.value.validity, Validity::Valid, "None is a Valid text");
    }

    /// D-007 regression. `MemoryModule` has no serial field, so the udev `SERIAL_NUMBER` the
    /// collector skips (tl-collect `inventory::modules` tests) has no path to a row. This checks
    /// the UI side: no "Serial" row in Memory modules and no module serial text in any section.
    #[test]
    fn info_no_dimm_serial() {
        const SERIAL: &str = "ABC123";
        let inv = inventory();
        let sys = system();
        let builders: [fn(&InfoInputs, &mut Vec<InfoRow>); 10] = [
            build_os,
            build_processor,
            build_system,
            build_memory,
            build_modules,
            build_disks,
            build_bios,
            build_motherboard,
            build_graphics,
            build_network,
        ];
        let modules = rows(&inv, &sys, build_modules);
        assert!(
            modules.iter().all(|r| !r.label.contains("Serial")),
            "no serial row in Memory modules"
        );
        for build in builders {
            for (_, label, text) in shown(&rows(&inv, &sys, build)) {
                assert!(!text.contains(SERIAL), "{label} holds a module serial");
            }
        }
    }

    #[test]
    fn info_modules_blocks() {
        let got = rows(&inventory(), &system(), build_modules);
        let firsts: Vec<_> = shown(&got)
            .into_iter()
            .filter(|(_, l, _)| l == "Module")
            .map(|(b, _, v)| (b, v))
            .collect();
        assert_eq!(
            firsts,
            [(1, "Module 1".to_owned()), (2, "Module 2".to_owned())]
        );
        let labels: Vec<_> = shown(&got)
            .into_iter()
            .filter(|(b, _, _)| *b == 2)
            .map(|(_, l, v)| (l, v))
            .collect();
        let want = [
            ("Module", "Module 2"),
            ("Slot", "P0 CHANNEL B (DIMM 1)"),
            ("Capacity", "32.00 GiB"),
            ("Speed", "6000 MT/s"),
            ("Type", "DDR5"),
            ("Form factor", "DIMM"),
            ("Manufacturer", "Kingston"),
            ("Part number", "KF560C30-32"),
        ];
        assert_eq!(
            labels,
            want.map(|(l, v)| (l.to_owned(), v.to_owned())).to_vec()
        );
        assert_eq!(got[0].label.as_ref(), BLOCK_FIRST_LABELS[0]);
    }

    #[test]
    fn info_modules_none() {
        let mut inv = inventory();
        inv.modules = Arc::from([]);
        let got = shown(&rows(&inv, &system(), build_modules));
        assert_eq!(got, [(0, "Modules".to_owned(), "Not reported".to_owned())]);
    }

    #[test]
    fn info_disk_space_format() {
        let got = rows(&inventory(), &system(), build_disks);
        assert_eq!(got[0].label.as_ref(), BLOCK_FIRST_LABELS[1]);
        assert_eq!(value_of(&got, "Space"), "812.40 GiB free of 1.82 TiB");
        assert_eq!(value_of(&got, "Capacity"), "1.82 TiB");
        assert_eq!(value_of(&got, "Type"), "SSD");
        assert_eq!(value_of(&got, "Volumes"), "/, /home");
        assert!(
            got.iter()
                .all(|r| r.live == (r.label.as_ref() == "Space") && r.block == 1)
        );
    }

    #[test]
    fn info_disk_volumes_none() {
        let mut inv = inventory();
        let mut hdd = disk(2, GIB, &[]);
        hdd.kind = s("HDD");
        inv.disks = vec![disk(4, GIB, &["/"]), hdd].into();
        let got = rows(&inv, &system(), build_disks);
        let second: Vec<_> = shown(&got)
            .into_iter()
            .filter(|(b, _, _)| *b == 2)
            .collect();
        let first: Vec<_> = shown(&got)
            .into_iter()
            .filter(|(b, _, _)| *b == 1)
            .collect();
        let find = |rows: &[(u8, String, String)], label: &str| {
            rows.iter().find(|(_, l, _)| l == label).unwrap().2.clone()
        };
        assert_eq!(find(&first, "Volumes"), "None", "device 2 sorts first");
        assert_eq!(find(&first, "Type"), "HDD");
        assert_eq!(find(&second, "Volumes"), "/");
    }

    #[test]
    fn info_gpu_resolution_multiline() {
        let got = rows(&inventory(), &system(), build_graphics);
        assert_eq!(got[0].label.as_ref(), BLOCK_FIRST_LABELS[2]);
        let res = value_of(&got, "Resolution");
        let lines: Vec<_> = res.split('\n').collect();
        assert_eq!(lines, ["2560x1440 @ 60 Hz", "3840x2160 @ 144 Hz (primary)"]);
        assert_eq!(value_of(&got, "Temperature"), "52 °C");
        assert_eq!(value_of(&got, "Dedicated memory"), "24.00 GiB");
        let temp = got
            .iter()
            .find(|r| r.label.as_ref() == "Temperature")
            .unwrap();
        assert!(temp.live);

        let mut inv = inventory();
        let mut nameless = inv.graphics[0].clone();
        nameless.name = InvField {
            value: Arc::from(""),
            validity: Validity::Unsupported,
        };
        inv.graphics = vec![nameless].into();
        assert_eq!(
            value_of(&rows(&inv, &system(), build_graphics), "Name"),
            "Unknown GPU"
        );
    }

    #[test]
    fn info_net_multiple_addresses() {
        let got = rows(&inventory(), &system(), build_network);
        assert_eq!(got[0].label.as_ref(), BLOCK_FIRST_LABELS[3]);
        let want = [
            ("Connection", "enp6s0 (Ethernet)"),
            (
                "Details",
                "Intel Ethernet Controller I225-V, igc, 2500 Mbps",
            ),
            ("IP address", "192.168.1.10/24\nfe80::1c2:3ff:fe4a:5b6c/64"),
            ("MAC address", "a8:a1:59:12:34:56"),
            ("Public IP", "Not fetched"),
        ];
        let got: Vec<_> = shown(&got).into_iter().map(|(_, l, v)| (l, v)).collect();
        assert_eq!(
            got,
            want.map(|(l, v)| (l.to_owned(), v.to_owned())).to_vec()
        );
    }

    fn public_ip_rows(state: PublicIpState) -> Vec<InfoRow> {
        let mut inv = inventory();
        inv.public_ip = state;
        rows(&inv, &system(), build_network)
    }

    fn fetched(v4: Option<&str>, v6: Option<&str>) -> PublicIpState {
        PublicIpState::Fetched {
            v4: v4.map(Arc::from),
            v6: v6.map(Arc::from),
            fetched_at_s: 1_792_000_000,
        }
    }

    #[test]
    fn info_public_ip_states() {
        let last = |state| public_ip_rows(state).pop().expect("a Public IP row");

        let r = last(PublicIpState::NotFetched);
        assert_eq!((r.label.as_ref(), r.block), ("Public IP", 0));
        assert_eq!(r.value.text.as_ref(), "Not fetched");
        assert_eq!(r.value.validity, Validity::Stale);
        assert_eq!(r.fetch, Some(false));

        let r = last(PublicIpState::Fetching);
        assert_eq!(r.value.text.as_ref(), "Fetching…");
        assert_eq!(r.fetch, Some(true));

        let r = last(fetched(Some("203.0.113.4"), None));
        let time = tl_ui::pages::performance::net::local_hh_mm(1_792_000_000);
        assert_eq!(
            r.value.text.as_ref(),
            format!("203.0.113.4 (fetched {time})")
        );
        assert_eq!(r.fetch, Some(false));

        let r = last(PublicIpState::Failed(Arc::from("timed out")));
        assert_eq!(r.value.validity, Validity::Failed);
        assert_eq!(
            r.value.tooltip.as_deref(),
            Some("Could not reach api.ipify.org: timed out")
        );
        assert_eq!(r.fetch, Some(false));
        assert_eq!(
            shown(std::slice::from_ref(&r))[0].2,
            "Unavailable",
            "the grid shows the Failed word"
        );

        // Only the Public IP row carries a button.
        let all = public_ip_rows(PublicIpState::NotFetched);
        assert_eq!(all.iter().filter(|r| r.fetch.is_some()).count(), 1);
    }

    #[test]
    fn info_public_ip_shared_with_network_page() {
        use tl_ui::pages::performance::details::Template;
        use tl_ui::pages::performance::net::public_ip_text;
        let template = Template::en("perf-net-ip-fetched", &["ip", "time"]);
        for (v4, v6) in [
            (Some("203.0.113.4"), None),
            (Some("203.0.113.4"), Some("2001:db8::1")),
        ] {
            let got = public_ip_rows(fetched(v4, v6)).pop().unwrap();
            assert_eq!(
                got.value.text.as_ref(),
                public_ip_text(&template, v4, v6, 1_792_000_000)
            );
        }
    }

    #[gpui_kit::test]
    fn info_public_ip_button_drawn(cx: &mut TestAppContext) {
        for (state, lines) in [
            (PublicIpState::Fetching, 1),
            (fetched(Some("203.0.113.4"), Some("2001:db8::1")), 2),
        ] {
            let got = public_ip_rows(state);
            let at = got.len() - 1;
            let (_, _, cx) = host(cx, InfoSectionId::NetworkAdapters, 100, got);
            let value = bounds(cx, &format!("TlKv-row-{at}")).expect("row drawn");
            assert_eq!(value.size.height, px(28. * lines as f32));
            let button = bounds(cx, "TlButton-info-public-ip-fetch").expect("button drawn");
            assert_eq!(button.size.height, px(24.));
            assert!(button.origin.x > value.origin.x + px(232.));
        }
    }

    #[gpui_kit::test]
    fn info_memory_peak_label_wraps(cx: &mut TestAppContext) {
        let got = rows(&inventory(), &system(), build_memory);
        let peak = got
            .iter()
            .position(|r| r.label.starts_with("Peak committed"))
            .unwrap();
        assert!(
            got.iter()
                .enumerate()
                .all(|(n, r)| r.label_lines == if n == peak { 2 } else { 1 })
        );
        let (_, _, cx) = host(cx, InfoSectionId::Memory, 100, got.clone());
        for n in 0..got.len() {
            let r = bounds(cx, &format!("TlKv-row-{n}")).expect("row drawn");
            let want = if n == peak { px(56.) } else { px(28.) };
            assert_eq!(r.size.height, want, "row {n} height");
        }
    }
}

/// The InformationPage entity: layout, demand, loading, live refresh, F5 toast, error state and
/// keyboard scrolling (tasklens-9o0.24).
#[cfg(feature = "test-hooks")]
mod page {
    use std::cell::Cell;
    use std::time::Duration;

    use super::*;
    use gpui_kit::base::Root;
    use gpui_kit::point;
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::demand::Demand;
    use tl_model::health::SourceId;
    use tl_model::snapshot::{InventorySnapshot, SystemSnapshot};
    use tl_ui::actions::{ClearFilter, RefreshCurrentTab};
    use tl_ui::app::EngineGlobal;
    use tl_ui::components::toast::{self, ToastQueue};
    use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::information::actions::{CopyInformationSection, bind_keys_information};
    use tl_ui::pages::information::{InformationPage, REFRESH_TOAST_AFTER};
    use tl_ui::testing::fake_engine::FakeEngine;

    const S: u64 = 1_000_000_000;

    struct PageHost {
        page: Entity<InformationPage>,
        width: f32,
        height: f32,
        retries: Rc<Cell<usize>>,
        clears: Rc<Cell<usize>>,
    }

    impl Render for PageHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (retries, clears) = (self.retries.clone(), self.clears.clone());
            div()
                .w(px(self.width))
                .h(px(self.height))
                .on_action(move |_: &RefreshCurrentTab, _, _| retries.set(retries.get() + 1))
                .on_action(move |_: &ClearFilter, _, _| clears.set(clears.get() + 1))
                .child(gpui_kit::base::TextSelectionLayer)
                .child(self.page.clone())
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        page: Entity<InformationPage>,
        queue: Entity<ToastQueue>,
        retries: Rc<Cell<usize>>,
        clears: Rc<Cell<usize>>,
        vcx: VisualTestContext,
    }

    fn rig(cx: &mut TestAppContext, width: f32, height: f32) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            setup(cx, 100);
            toast::init(cx);
            bind_keys_information(cx);
            cx.set_global(EngineGlobal(client.clone()));
        });
        let hub = cx.update(|cx| cx.new(|cx| SnapshotHub::new(client.clone(), cx)));
        let demand = cx.update(|cx| cx.new(|cx| DemandAggregator::new(client, &hub, cx)));
        let retries = Rc::new(Cell::new(0));
        let clears = Rc::new(Cell::new(0));
        let slot = Rc::new(RefCell::new(None));
        let window = cx.add_window({
            let (slot, retries, clears) = (slot.clone(), retries.clone(), clears.clone());
            move |window, cx| {
                let page = cx.new(|cx| InformationPage::new(&hub, &demand, window, cx));
                let host = cx.new(|_| PageHost {
                    page: page.clone(),
                    width,
                    height,
                    retries,
                    clears,
                });
                *slot.borrow_mut() = Some(page);
                Root::new(host, window, cx)
            }
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
        let page = slot.borrow_mut().take().expect("page made");
        let mut rig = Rig {
            engine,
            page,
            queue,
            retries,
            clears,
            vcx,
        };
        rig.draw();
        rig
    }

    impl Rig {
        /// Three frames: the page measures its width and the sections their heights, then it
        /// lays out for them.
        fn draw(&mut self) {
            for _ in 0..3 {
                draw(&mut self.vcx);
            }
        }

        fn bounds(&mut self, selector: &str) -> Option<Bounds<Pixels>> {
            bounds(&mut self.vcx, selector)
        }

        fn publish_inventory(&mut self, version: u64, validity: Validity) {
            self.engine.publish_with(|s| {
                let mut inv = InventorySnapshot::default();
                inv.header.version = version;
                inv.header.validity = validity;
                inv.os.name = tl_model::table::Total {
                    value: Arc::from("CachyOS Linux"),
                    validity: Validity::Valid,
                };
                inv.os.kernel = tl_model::table::Total {
                    value: Arc::from("6.17.1"),
                    validity: Validity::Valid,
                };
                s.inventory = Arc::new(inv);
            });
            self.draw();
        }

        fn publish_system(&mut self, version: u64, uptime_s: u64) {
            self.engine.publish_with(|s| {
                let mut sys = SystemSnapshot::default();
                sys.header.version = version;
                sys.header.validity = Validity::Valid;
                sys.header.observed_end = uptime_s * S;
                s.system = Arc::new(sys);
            });
            self.draw();
        }

        fn renders(&mut self, id: InfoSectionId) -> usize {
            let section = self
                .vcx
                .read_entity(&self.page, |p, _| p.section(id).clone());
            self.vcx.read_entity(&section, |s, _| s.render_count())
        }

        fn value(&mut self, id: InfoSectionId, label: &str) -> String {
            let section = self
                .vcx
                .read_entity(&self.page, |p, _| p.section(id).clone());
            self.vcx.read_entity(&section, |s, _| {
                s.rows()
                    .iter()
                    .find(|r| r.label.as_ref() == label)
                    .map(|r| r.value.text.to_string())
                    .unwrap_or_default()
            })
        }

        fn set_active(&mut self, on: bool) {
            self.page
                .update(&mut self.vcx, |p, cx| p.set_active(on, cx));
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.vcx.run_until_parked();
        }

        fn refresh_toasts(&mut self) -> usize {
            self.vcx.update(|_, cx| {
                self.queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .filter(|t| t.as_ref() == "Information refreshed.")
                    .count()
            })
        }

        fn inventory_refreshes(&self) -> usize {
            self.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::RefreshNow(SourceId::Inventory)))
                .count()
        }
    }

    fn section_selector(id: InfoSectionId) -> String {
        format!("InfoSection-{}", id as usize)
    }

    #[gpui_kit::test]
    fn info_two_columns_above_880(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        assert!(r.bounds("InformationPage-columns").is_some());
        let os = r
            .bounds(&section_selector(InfoSectionId::OperatingSystem))
            .unwrap();
        let mem = r.bounds(&section_selector(InfoSectionId::Memory)).unwrap();
        assert_eq!(os.origin.y, mem.origin.y, "both at the top");
        assert!(
            mem.origin.x > os.origin.x + os.size.width,
            "Memory on the right"
        );
        assert_eq!(os.origin.x, px(24.), "left padding 24");
        assert_eq!(os.origin.y, px(16.), "top padding 16");
        let processor = r
            .bounds(&section_selector(InfoSectionId::Processor))
            .unwrap();
        assert_eq!(processor.origin.x, os.origin.x, "Processor under OS");
    }

    #[gpui_kit::test]
    fn info_one_column_order(cx: &mut TestAppContext) {
        let mut r = rig(cx, 879., 800.);
        assert!(r.bounds("InformationPage-column").is_some());
        let mut last: Option<Bounds<Pixels>> = None;
        for id in InfoSectionId::ALL {
            let b = r.bounds(&section_selector(id)).unwrap();
            if let Some(prev) = last {
                assert_eq!(b.origin.x, prev.origin.x, "{id:?} in the same column");
                assert_eq!(
                    b.origin.y,
                    prev.origin.y + prev.size.height + px(32.),
                    "{id:?} 32 below the previous section"
                );
            }
            last = Some(b);
        }
    }

    #[gpui_kit::test]
    fn info_demand_only_while_visible(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.set_active(true);
        let sent = r.engine.demands().last().cloned().expect("demand sent");
        assert!(
            sent.any_surface_visible,
            "the engine collects only for a visible surface"
        );
        assert!(sent.inventory && sent.memory && sent.gpus && sent.disks);
        assert_eq!(
            sent.cpu,
            tl_model::demand::CpuDemand::Total,
            "the System counts come from the CPU totals"
        );
        r.set_active(false);
        assert_eq!(r.engine.demands().last(), Some(&Demand::default()));
    }

    #[gpui_kit::test]
    fn info_refresh_now_on_first_visit_only(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        assert_eq!(r.inventory_refreshes(), 0);
        r.set_active(true);
        assert_eq!(r.inventory_refreshes(), 1);
        r.set_active(false);
        r.set_active(true);
        assert_eq!(r.inventory_refreshes(), 1, "only the first visit");
    }

    #[gpui_kit::test]
    fn info_skeleton_once(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        let loading = |r: &mut Rig| {
            InfoSectionId::ALL.map(|id| {
                let s = r.vcx.read_entity(&r.page, |p, _| p.section(id).clone());
                r.vcx.read_entity(&s, |s, _| s.is_loading())
            })
        };
        assert!(
            loading(&mut r).iter().all(|l| *l),
            "skeleton while warming up"
        );
        for id in InfoSectionId::ALL {
            let band = format!("{}-band", section_selector(id));
            assert!(r.bounds(&band).is_some(), "{id:?} band drawn");
        }
        assert!(r.bounds("TlButton-info-copy-0").is_some());

        r.publish_inventory(1, Validity::Valid);
        assert!(
            loading(&mut r).iter().all(|l| !*l),
            "data ends the skeleton"
        );
        assert!(r.vcx.read_entity(&r.page, |p, _| p.first_data_seen()));
        r.publish_inventory(2, Validity::WarmingUp);
        assert!(
            loading(&mut r).iter().all(|l| !*l),
            "never again in the session"
        );
    }

    #[gpui_kit::test]
    fn info_only_live_sections_rerender(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);
        r.publish_system(1, 100);
        let (os, cpu, system) = (
            r.renders(InfoSectionId::OperatingSystem),
            r.renders(InfoSectionId::Processor),
            r.renders(InfoSectionId::System),
        );
        r.publish_system(2, 101);
        assert_eq!(r.renders(InfoSectionId::OperatingSystem), os);
        assert_eq!(r.renders(InfoSectionId::Processor), cpu);
        assert!(r.renders(InfoSectionId::System) > system);
    }

    #[gpui_kit::test]
    fn info_live_uptime_ticks(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);
        r.publish_system(1, 9_000);
        let first = r.value(InfoSectionId::System, "System uptime");
        r.publish_system(2, 9_001);
        let second = r.value(InfoSectionId::System, "System uptime");
        assert!(!first.is_empty());
        assert_ne!(first, second);
    }

    #[gpui_kit::test]
    fn info_refresh_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);

        // A new version after 300 ms.
        r.page.update(&mut r.vcx, |p, cx| p.refresh(cx));
        r.vcx.run_until_parked();
        assert_eq!(r.inventory_refreshes(), 1);
        r.vcx.executor().advance_clock(Duration::from_millis(300));
        r.vcx.run_until_parked();
        assert_eq!(r.refresh_toasts(), 0);
        r.publish_inventory(2, Validity::Valid);
        assert_eq!(r.refresh_toasts(), 1);
        r.vcx.executor().advance_clock(REFRESH_TOAST_AFTER);
        r.vcx.run_until_parked();
        assert_eq!(r.refresh_toasts(), 1, "one toast per F5");

        // No new version: the toast shows at 2 s.
        r.vcx.executor().advance_clock(Duration::from_secs(10));
        r.vcx.run_until_parked();
        assert_eq!(r.refresh_toasts(), 0, "the first toast is gone");
        r.page.update(&mut r.vcx, |p, cx| p.refresh(cx));
        r.vcx
            .executor()
            .advance_clock(REFRESH_TOAST_AFTER - Duration::from_millis(100));
        r.vcx.run_until_parked();
        assert_eq!(r.refresh_toasts(), 0);
        r.vcx.executor().advance_clock(Duration::from_millis(100));
        r.vcx.run_until_parked();
        assert_eq!(r.refresh_toasts(), 1);
    }

    #[gpui_kit::test]
    fn info_load_error_state(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        assert!(r.bounds("InformationPage-error").is_none());
        r.publish_inventory(1, Validity::Failed);
        assert!(r.bounds("InformationPage-error").is_some());
        assert!(r.bounds("InformationPage-columns").is_none());
        // The action bubbles from the focused element up to the host.
        let focus = r.vcx.read_entity(&r.page, |p, _| p.focus_handle().clone());
        r.vcx.update(|window, cx| window.focus(&focus, cx));
        r.draw();
        let retry = r.bounds("TlButton-information-retry").expect("Retry drawn");
        r.vcx.simulate_click(retry.center(), Modifiers::none());
        r.vcx.run_until_parked();
        assert_eq!(r.retries.get(), 1, "Retry dispatches RefreshCurrentTab");

        // A failure after data keeps the sections.
        r.publish_inventory(2, Validity::Valid);
        assert!(r.bounds("InformationPage-error").is_none());
        r.publish_inventory(3, Validity::Failed);
        assert!(r.bounds("InformationPage-error").is_none());
        assert!(r.bounds("InformationPage-columns").is_some());
    }

    #[gpui_kit::test]
    fn info_keyboard_scrolling(cx: &mut TestAppContext) {
        let mut r = rig(cx, 879., 400.);
        r.publish_inventory(1, Validity::Valid);
        let focus = r.vcx.read_entity(&r.page, |p, _| p.focus_handle().clone());
        r.vcx.update(|window, cx| window.focus(&focus, cx));
        r.draw();
        let scroll = r.vcx.read_entity(&r.page, |p, _| p.scroll_handle().clone());
        let max = scroll.max_offset().y;
        assert!(max > px(0.), "the page is taller than the window");

        r.vcx.simulate_keystrokes("end");
        r.draw();
        assert_eq!(scroll.offset().y, -max, "End goes to the bottom");

        r.vcx.simulate_keystrokes("pageup");
        r.draw();
        let viewport = scroll.bounds().size.height;
        assert_eq!(scroll.offset().y, -max + (viewport - px(28.)));

        r.vcx.simulate_keystrokes("down");
        r.draw();
        assert_eq!(scroll.offset().y, -max + (viewport - px(56.)));

        r.vcx.simulate_keystrokes("home");
        r.draw();
        assert_eq!(scroll.offset().y, px(0.));
    }

    #[gpui_kit::test]
    fn info_filter_page_hides_and_clears(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);
        let shown = |r: &mut Rig| {
            r.vcx
                .read_entity(&r.page, |p, _| InfoSectionId::ALL.map(|id| p.is_shown(id)))
        };
        let filter = |r: &mut Rig, text: &str| {
            let text: Arc<str> = Arc::from(text);
            r.page.update(&mut r.vcx, |p, cx| p.apply_filter(text, cx));
            r.draw();
        };

        filter(&mut r, "kernel");
        let kernel = shown(&mut r);
        assert!(kernel[InfoSectionId::OperatingSystem as usize]);
        assert!(!kernel[InfoSectionId::Processor as usize]);
        let os = r.vcx.read_entity(&r.page, |p, _| {
            p.section(InfoSectionId::OperatingSystem).clone()
        });
        let labels = r.vcx.read_entity(&os, |s, _| {
            s.rows()
                .iter()
                .map(|r| r.label.to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(labels, ["Kernel", "Kernel version"]);
        assert_eq!(
            r.vcx.read_entity(&r.page, |p, _| p
                .full_rows(InfoSectionId::OperatingSystem)
                .len()),
            9,
            "the full rows stay for copying: all nine OS rows"
        );
        assert!(
            r.bounds(&section_selector(InfoSectionId::Processor))
                .is_none()
        );

        filter(&mut r, "KERNEL");
        assert_eq!(shown(&mut r), kernel, "case does not matter");

        filter(&mut r, "zzzz-no-such-text");
        assert!(shown(&mut r).iter().all(|s| !*s));
        assert!(r.bounds("InformationPage-filter-empty").is_some());
        let text = r.bounds("TlEmptyState-text").expect("empty text");
        assert!(text.size.width > px(0.));
        let focus = r.vcx.read_entity(&r.page, |p, _| p.focus_handle().clone());
        r.vcx.update(|window, cx| window.focus(&focus, cx));
        r.draw();
        let clear = r
            .bounds("TlButton-TlEmptyState-clear-filter")
            .expect("Clear filter drawn");
        r.vcx.simulate_click(clear.center(), Modifiers::none());
        r.vcx.run_until_parked();
        assert_eq!(r.clears.get(), 1, "Clear filter dispatches ClearFilter");

        filter(&mut r, "");
        assert!(shown(&mut r).iter().all(|s| *s));
        assert!(r.bounds("InformationPage-filter-empty").is_none());
    }

    fn clipboard(r: &mut Rig) -> Option<String> {
        r.vcx.read_from_clipboard().and_then(|item| item.text())
    }

    fn focus_page(r: &mut Rig) {
        let focus = r.vcx.read_entity(&r.page, |p, _| p.focus_handle().clone());
        r.vcx.update(|window, cx| window.focus(&focus, cx));
        r.draw();
    }

    #[gpui_kit::test]
    fn info_copy_ignores_filter(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);
        let text: Arc<str> = Arc::from("kernel");
        r.page.update(&mut r.vcx, |p, cx| p.apply_filter(text, cx));
        focus_page(&mut r);
        r.vcx.dispatch_action(CopyInformationSection {
            section: InfoSectionId::OperatingSystem,
        });
        r.vcx.run_until_parked();
        let copied = clipboard(&mut r).expect("copied");
        let lines: Vec<&str> = copied.lines().collect();
        assert_eq!(lines[0], "Operating System");
        assert_eq!(lines.len(), 10, "the title and all nine rows: {copied}");
        assert!(copied.contains("OS name: CachyOS Linux"));
        assert!(copied.contains("Kernel: 6.17.1"));
        let toasts = r.vcx.update(|_, cx| r.queue.read(cx).visible_texts());
        assert!(toasts.iter().any(|t| t.as_ref() == "Copied to clipboard"));
    }

    #[gpui_kit::test]
    fn info_ctrl_c_selection_else_all(cx: &mut TestAppContext) {
        let mut r = rig(cx, 1280., 800.);
        r.publish_inventory(1, Validity::Valid);
        // Only the two kernel rows stay, so value 0 is the Kernel value "6.17.1".
        let text: Arc<str> = Arc::from("kernel");
        r.page.update(&mut r.vcx, |p, cx| p.apply_filter(text, cx));
        focus_page(&mut r);
        let value = r.bounds("TlKv-value-0").expect("Kernel value drawn");
        let (from, to) = (
            point(value.left() + px(1.), value.center().y),
            point(value.right() - px(1.), value.center().y),
        );
        r.vcx
            .simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        r.vcx
            .simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        r.vcx
            .simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        r.draw();
        r.vcx.simulate_keystrokes("ctrl-c");
        r.vcx.run_until_parked();
        assert_eq!(clipboard(&mut r).as_deref(), Some("6.17.1"));

        // No selection: everything, whatever the filter.
        r.vcx.update(gpui_kit::base::TextSelection::clear);
        focus_page(&mut r);
        r.vcx.simulate_keystrokes("ctrl-c");
        r.vcx.run_until_parked();
        let all = r.vcx.read_entity(&r.page, |p, _| p.copy_all_text());
        assert_eq!(clipboard(&mut r), Some(all.clone()));
        assert!(all.starts_with("Operating System\nOS name: CachyOS Linux"));
        assert!(all.contains("\n\nNetwork adapters"));
    }
}

/// The Information filter rules (tasklens-9o0.25, UI-SYS-084).
mod filter_rules {
    use super::*;
    use proptest::prelude::*;
    use tl_ui::pages::information::filter::{contains_ci, filter_section};
    use tl_ui::pages::information::sections::BLOCK_FIRST_LABELS;

    fn in_block(block: u8, label: &str, text: &str) -> InfoRow {
        InfoRow {
            block,
            ..row(label, text)
        }
    }

    fn os() -> Vec<InfoRow> {
        vec![
            row("OS name", "CachyOS Linux"),
            row("Version", "Rolling"),
            row("Kernel", "6.17.1-2-cachyos"),
            row("Kernel version", "#1 SMP PREEMPT_DYNAMIC"),
            row("Architecture", "x86_64 (64-bit)"),
        ]
    }

    fn modules() -> Vec<InfoRow> {
        let block = |n: u8, part: &str| {
            vec![
                in_block(n, "Module", &format!("Module {n}")),
                in_block(n, "Slot", &format!("P0 CHANNEL A (DIMM {n})")),
                in_block(n, "Capacity", "32.00 GiB"),
                in_block(n, "Part number", part),
            ]
        };
        let mut rows = block(1, "AAA-111");
        rows.extend(block(2, "BBB-222"));
        rows
    }

    fn labels(rows: &[InfoRow]) -> Vec<(u8, String, String)> {
        rows.iter()
            .map(|r| (r.block, r.label.to_string(), r.value.text.to_string()))
            .collect()
    }

    #[test]
    fn info_filter_hides_empty_sections() {
        let kept = filter_section("Operating System", &os(), "kernel");
        let kept: Vec<_> = kept.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(kept, ["Kernel", "Kernel version"]);
        let cpu = vec![row("Name", "AMD Ryzen 9"), row("Cores", "16")];
        assert!(filter_section("Processor", &cpu, "kernel").is_empty());
        assert_eq!(
            filter_section("Processor", &cpu, "").len(),
            2,
            "empty needle"
        );
    }

    #[test]
    fn info_filter_title_match_keeps_section() {
        let mem = vec![
            row("Total physical", "62.70 GiB"),
            row("Swap total", "64.00 GiB"),
        ];
        assert_eq!(filter_section("Memory", &mem, "memory").len(), 2);
        assert_eq!(
            filter_section("Memory modules", &modules(), "memory").len(),
            modules().len()
        );
    }

    #[test]
    fn info_filter_keeps_block_first_row() {
        let kept = filter_section("Memory modules", &modules(), "bbb");
        assert_eq!(
            labels(&kept),
            [
                (2, "Module".to_owned(), "Module 2".to_owned()),
                (2, "Part number".to_owned(), "BBB-222".to_owned()),
            ]
        );
    }

    #[test]
    fn info_filter_matches_validity_word() {
        let board = vec![
            row("Manufacturer", "ASUSTeK"),
            with_validity("Serial number", Validity::Restricted),
        ];
        let kept = filter_section("Motherboard", &board, "restricted");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].label.as_ref(), "Serial number");
        // A Missing value is not drawn, so it never matches on its value.
        let gone = vec![with_validity("Temperature", Validity::Missing)];
        assert!(filter_section("Graphics", &gone, "0").is_empty());
    }

    #[test]
    fn info_filter_case_insensitive() {
        let upper: Vec<_> = os()
            .into_iter()
            .map(|r| InfoRow {
                label: r.label.to_uppercase().into(),
                ..r
            })
            .collect();
        assert_eq!(
            labels(&filter_section("Operating System", &upper, "kernel")),
            labels(&filter_section("Operating System", &os(), "kernel"))
                .into_iter()
                .map(|(b, l, v)| (b, l.to_uppercase(), v))
                .collect::<Vec<_>>()
        );
    }

    fn shown(r: &InfoRow) -> String {
        let mut out = String::new();
        value_copy_text(r, &mut out);
        out
    }

    proptest! {
        /// Every kept row matches the title (rule 2), its own label or value (rule 3), or is the
        /// first row of a block with a kept matching row (rule 4).
        #[test]
        fn info_filter_kept_rows_follow_the_rules(needle in "[a-z0-9 ]{1,4}") {
            let mut rows = os();
            rows.extend(modules());
            for title in ["Operating System", "Memory modules"] {
                let kept = filter_section(title, &rows, &needle);
                if contains_ci(title, &needle) {
                    prop_assert_eq!(kept.len(), rows.len());
                    continue;
                }
                let matches = |r: &InfoRow| contains_ci(&r.label, &needle)
                    || contains_ci(&shown(r), &needle)
                    || contains_ci(&r.value.text, &needle);
                for r in kept.iter() {
                    let head_of_match = r.block != 0
                        && BLOCK_FIRST_LABELS.contains(&r.label.as_ref())
                        && kept.iter().any(|o| o.block == r.block && matches(o));
                    prop_assert!(matches(r) || head_of_match, "{:?} kept for {:?}", r.label, needle);
                }
            }
        }
    }
}

/// Copy section, copy all and Ctrl+C formats (tasklens-9o0.26, UI-SYS-083, D-007).
mod copy_text {
    use super::*;
    use tl_model::devices::DeviceCatalog;
    use tl_model::snapshot::{InventorySnapshot, NetworkAdapter, SystemSnapshot};
    use tl_model::table::Total;
    use tl_ui::pages::information::copy::{all_text, section_text};
    use tl_ui::pages::information::sections::{
        InfoInputs, InfoWords, build_bios, build_disks, build_graphics, build_memory,
        build_modules, build_motherboard, build_network, build_os, build_processor, build_system,
    };

    /// Compares `actual` with `tests/golden/<name>`. `TL_UPDATE_GOLDEN=1` rewrites the file.
    fn golden(name: &str, actual: &str) {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("TL_UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, actual).expect("write the golden");
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(actual, expected, "{name}");
    }

    /// The OS to Motherboard fixture with the hardware fixture's sections. The two local-time
    /// rows are Unsupported, so the golden does not depend on the time zone.
    fn fixture() -> (InventorySnapshot, SystemSnapshot) {
        let mut inv = super::row_builders::inventory();
        let hw = super::hardware_builders::inventory();
        inv.memory = hw.memory;
        inv.modules = hw.modules;
        inv.disks = hw.disks;
        inv.graphics = hw.graphics;
        inv.network = hw.network;
        let unsupported = Total {
            value: 0,
            validity: Validity::Unsupported,
        };
        inv.os.install_date_s = unsupported;
        inv.system.boot_time_s = unsupported;
        let mut sys = super::row_builders::system(183_495);
        let hw = super::hardware_builders::system();
        sys.memory = hw.memory;
        sys.gpus = hw.gpus;
        (inv, sys)
    }

    fn sections(
        inv: &InventorySnapshot,
        sys: &SystemSnapshot,
    ) -> [(InfoSectionId, Arc<[InfoRow]>); 10] {
        let words = InfoWords::default();
        let devices = DeviceCatalog::default();
        let inputs = InfoInputs {
            inv,
            sys,
            devices: &devices,
            words: &words,
        };
        InfoSectionId::ALL.map(|id| {
            let build: fn(&InfoInputs, &mut Vec<InfoRow>) = match id {
                InfoSectionId::OperatingSystem => build_os,
                InfoSectionId::Processor => build_processor,
                InfoSectionId::System => build_system,
                InfoSectionId::Memory => build_memory,
                InfoSectionId::MemoryModules => build_modules,
                InfoSectionId::DiskDrives => build_disks,
                InfoSectionId::BiosFirmware => build_bios,
                InfoSectionId::Motherboard => build_motherboard,
                InfoSectionId::Graphics => build_graphics,
                InfoSectionId::NetworkAdapters => build_network,
            };
            let mut rows = Vec::new();
            build(&inputs, &mut rows);
            (id, Arc::from(rows))
        })
    }

    #[test]
    fn info_copy_all_format_golden() {
        let (inv, sys) = fixture();
        let mut out = String::new();
        all_text(&sections(&inv, &sys), &mut out);
        assert!(!out.ends_with('\n'), "no trailing blank line");
        golden("info_copy_all.txt", &out);
    }

    #[test]
    fn info_copy_section_format() {
        let (mut inv, sys) = fixture();
        let mut second = NetworkAdapter::clone(&inv.network[0]);
        second.connection = Total {
            value: Arc::from("wlan0 (Wi-Fi)"),
            validity: Validity::Valid,
        };
        second.ip_addresses = vec![Arc::from("10.0.0.5/24")].into();
        inv.network = vec![NetworkAdapter::clone(&inv.network[0]), second].into();
        let rows = &sections(&inv, &sys)[InfoSectionId::NetworkAdapters as usize].1;
        let mut out = String::new();
        section_text(InfoSectionId::NetworkAdapters, rows, &mut out);
        assert_eq!(
            out,
            "Network adapters\n\
             Connection: enp6s0 (Ethernet)\n\
             Details: Intel Ethernet Controller I225-V, igc, 2500 Mbps\n\
             IP address: 192.168.1.10/24, fe80::1c2:3ff:fe4a:5b6c/64\n\
             MAC address: a8:a1:59:12:34:56\n\
             \n\
             Connection: wlan0 (Wi-Fi)\n\
             Details: Intel Ethernet Controller I225-V, igc, 2500 Mbps\n\
             IP address: 10.0.0.5/24\n\
             MAC address: a8:a1:59:12:34:56\n\
             \n\
             Public IP: Not fetched"
        );
    }

    /// D-007 regression: no module serial reaches the copy text. The collector drops the udev
    /// `SERIAL_NUMBER` (tl-collect `inventory::modules` tests) and `MemoryModule` has no serial
    /// field; this checks the copy side.
    #[test]
    fn info_no_dimm_serial_in_copy() {
        let (inv, sys) = fixture();
        let all = sections(&inv, &sys);
        let mut out = String::new();
        all_text(&all, &mut out);
        assert!(!out.contains("ABC123"));
        let mut modules = String::new();
        section_text(
            InfoSectionId::MemoryModules,
            &all[InfoSectionId::MemoryModules as usize].1,
            &mut modules,
        );
        assert!(modules.starts_with("Memory modules\nModule: Module 1\n"));
        assert!(!modules.contains("Serial"), "{modules}");
    }
}

/// `TasklensUi::init` binds the Information keys and the shell builds `InformationPage` for its
/// tab (tasklens-9o0.27).
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn information_keymap_registered(cx: &mut TestAppContext) {
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::information::InformationPage;
    use tl_ui::pages::information::actions::{CopyInformationAll, InformationScrollBottom};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window};

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let mut settings = Settings::default();
    settings.general.start_tab = StartTab::Information;
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();

    cx.update(|cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let bound = |action: &dyn gpui_kit::Action, keys: &str| {
            keymap.bindings_for_action(action).any(|b| {
                b.keystrokes().len() == 1
                    && b.keystrokes()[0].unparse() == keys
                    && b.predicate()
                        .is_some_and(|p| p.to_string() == "Information")
            })
        };
        assert!(
            bound(&InformationScrollBottom, "end"),
            "end scrolls to the bottom"
        );
        assert!(
            bound(&CopyInformationAll, "ctrl-c"),
            "ctrl-c copies everything"
        );
    });

    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    let page = vcx.read_entity(&main, |m, _| {
        m.page_entity::<InformationPage>(Tab::Information)
    });
    assert!(
        page.is_some(),
        "the Information tab builds an InformationPage"
    );
}

/// The Information tab inside `MainWindow`, end to end through the fake engine
/// (tasklens-9o0.3, UI-SYS-080, UI-SYS-096, R-023).
#[cfg(feature = "test-hooks")]
mod main_window {
    use super::*;
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, DeviceCommand, Tab};
    use tl_model::health::SourceId;
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_model::snapshot::{InventorySnapshot, SystemSnapshot};
    use tl_ui::actions::ShowSystemInformation;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::pages::information::InformationPage;
    use tl_ui::pages::information::actions::CopyInformationAll;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::filter::FILTER_DEBOUNCE;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        main: Entity<MainWindow>,
    }

    fn boot(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.general.start_tab = StartTab::Processes;
        let init = UiInit {
            settings,
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        Rig { engine, vcx, main }
    }

    impl Rig {
        fn draw(&mut self) {
            for _ in 0..3 {
                draw(&mut self.vcx);
            }
        }

        fn select(&mut self, tab: Tab) {
            self.main
                .update_in(&mut self.vcx, |m, window, cx| m.select_tab(tab, window, cx));
            self.draw();
        }

        fn active_tab(&mut self) -> Tab {
            self.vcx.read_entity(&self.main, |m, _| m.active_tab())
        }

        fn page(&mut self) -> Option<Entity<InformationPage>> {
            self.vcx.read_entity(&self.main, |m, _| {
                m.page_entity::<InformationPage>(Tab::Information)
            })
        }

        fn bounds(&mut self, selector: &str) -> Option<Bounds<Pixels>> {
            bounds(&mut self.vcx, selector)
        }

        fn publish_inventory(&mut self, version: u64) {
            self.engine.publish_with(|s| {
                let mut inv = InventorySnapshot::default();
                inv.header.version = version;
                inv.header.validity = Validity::Valid;
                inv.os.name = tl_model::table::Total {
                    value: Arc::from("CachyOS Linux"),
                    validity: Validity::Valid,
                };
                s.inventory = Arc::new(inv);
            });
            self.draw();
        }

        fn publish_system(&mut self, version: u64) {
            self.engine.publish_with(|s| {
                let mut sys = SystemSnapshot::default();
                sys.header.version = version;
                sys.header.validity = Validity::Valid;
                s.system = Arc::new(sys);
            });
            self.draw();
        }

        /// Visits the tab and lets the demand reach the engine.
        fn visit(&mut self) {
            self.select(Tab::Information);
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.draw();
        }

        fn offset_y(&mut self) -> Pixels {
            let page = self.page().expect("the Information page exists");
            self.vcx
                .read_entity(&page, |p, _| p.scroll_handle().offset().y)
        }

        fn is_shown(&mut self, id: InfoSectionId) -> bool {
            let page = self.page().expect("the Information page exists");
            self.vcx.read_entity(&page, |p, _| p.is_shown(id))
        }

        fn toasts(&mut self) -> Vec<String> {
            let queue = self.vcx.read_entity(&self.main, |m, _| m.toasts().clone());
            self.vcx.read_entity(&queue, |q, _| {
                q.visible_texts().iter().map(|t| t.to_string()).collect()
            })
        }
    }

    #[gpui_kit::test]
    fn info_public_ip_notice_first(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.publish_inventory(1);
        r.visit();
        let page = r.page().expect("the Information page exists");
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        let button = r
            .bounds("TlButton-info-public-ip-fetch")
            .expect("the Fetch button is drawn");
        let before = r.vcx.update(|_, cx| cx.windows().len());
        r.vcx.simulate_click(button.center(), Modifiers::none());
        r.draw();
        r.vcx.run_until_parked();
        let after = r.vcx.update(|_, cx| cx.windows().len());
        assert_eq!(after, before + 1, "the notice window opened");
        assert!(
            !r.engine
                .commands()
                .iter()
                .any(|c| matches!(c, Command::Device(DeviceCommand::FetchPublicIp))),
            "no fetch before Continue"
        );
    }

    #[gpui_kit::test]
    fn help_system_information_scrolls_to_os(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.publish_inventory(1);
        r.visit();
        let page = r.page().expect("the Information page exists");
        r.vcx.read_entity(&page, |p, _| {
            p.scroll_handle()
                .set_offset(gpui_kit::point(px(0.), px(-120.)))
        });
        r.select(Tab::Processes);

        r.vcx.dispatch_action(ShowSystemInformation);
        r.draw();

        assert_eq!(r.active_tab(), Tab::Information);
        assert_eq!(r.offset_y(), px(0.));
        let os = r.bounds("InfoSection-0").expect("OS section drawn");
        let page_bounds = r.bounds("InformationPage").expect("page drawn");
        assert_eq!(os.origin.y, page_bounds.origin.y + px(16.));
        let focused = r
            .vcx
            .update(|window, cx| page.read(cx).focus_handle().is_focused(window));
        assert!(focused, "the scroll region has focus");
        assert!(r.toasts().is_empty(), "no toast: {:?}", r.toasts());
    }

    #[gpui_kit::test]
    fn help_system_information_clears_hiding_filter(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.publish_inventory(1);
        r.visit();
        let filter = r.vcx.read_entity(&r.main, |m, _| m.filter_box().clone());
        r.vcx.update(|window, cx| {
            let input = filter.read(cx).input().clone();
            input.update(cx, |s, cx| s.focus(window, cx));
        });
        r.vcx.simulate_input("graphics");
        r.vcx.executor().advance_clock(FILTER_DEBOUNCE * 2);
        r.draw();
        assert!(!r.is_shown(InfoSectionId::OperatingSystem));

        r.vcx.dispatch_action(ShowSystemInformation);
        r.draw();

        assert_eq!(
            r.vcx
                .read_entity(&filter, |f, _| f.text(Tab::Information).to_string()),
            ""
        );
        assert!(r.is_shown(InfoSectionId::OperatingSystem));
        assert_eq!(r.offset_y(), px(0.));
    }

    #[gpui_kit::test]
    fn information_tab_mounted_and_kept(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        assert!(r.page().is_none(), "built on first visit");
        r.vcx.simulate_keystrokes("ctrl-6");
        r.draw();
        assert_eq!(r.active_tab(), Tab::Information);
        let first = r.page().expect("the Information page exists");
        let page_bounds = r.bounds("InformationPage").expect("page drawn");
        let window = r.bounds("MainWindowPage").expect("page slot drawn");
        assert!(page_bounds.origin.x >= window.origin.x);
        assert!(page_bounds.origin.y >= window.origin.y);
        assert!(page_bounds.bottom() <= window.bottom());
        assert!(page_bounds.right() <= window.right());

        r.select(Tab::Processes);
        r.vcx.simulate_keystrokes("ctrl-6");
        r.draw();
        assert_eq!(r.active_tab(), Tab::Information);
        let second = r.page().expect("the Information page is kept");
        assert_eq!(first.entity_id(), second.entity_id());
    }

    #[gpui_kit::test]
    fn information_end_to_end_fake_engine(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.visit();
        let page = r.page().expect("the Information page exists");
        let loading = |r: &mut Rig| -> Vec<bool> {
            r.vcx.read_entity(&page, |p, cx| {
                InfoSectionId::ALL
                    .iter()
                    .map(|id| p.section(*id).read(cx).is_loading())
                    .collect()
            })
        };
        assert!(loading(&mut r).iter().all(|l| *l), "skeleton first");

        r.publish_inventory(1);
        r.publish_system(1);
        assert!(loading(&mut r).iter().all(|l| !*l), "then data");

        let refreshes = |r: &Rig| {
            r.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::RefreshNow(SourceId::Inventory)))
                .count()
        };
        let before = refreshes(&r);
        r.vcx.simulate_keystrokes("f5");
        r.draw();
        assert_eq!(refreshes(&r), before + 1);
        r.publish_inventory(2);
        assert!(
            r.toasts().iter().any(|t| t == "Information refreshed."),
            "{:?}",
            r.toasts()
        );

        let focus = r.vcx.read_entity(&page, |p, _| p.focus_handle().clone());
        r.vcx.update(|window, cx| window.focus(&focus, cx));
        r.vcx.dispatch_action(CopyInformationAll);
        r.draw();
        let expected = r.vcx.read_entity(&page, |p, _| p.copy_all_text());
        let copied = r.vcx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(expected.as_str()));
    }

    #[gpui_kit::test]
    fn copy_icon_with_filter_focused_copies_all(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.publish_inventory(1);
        r.visit();
        let page = r.page().expect("page");
        let filter = r.vcx.read_entity(&r.main, |m, _| m.filter_box().clone());
        r.vcx.update(|window, cx| {
            let input = filter.read(cx).input().clone();
            input.update(cx, |s, cx| s.focus(window, cx));
        });
        r.draw();
        let icon = r.bounds("TabStrip-copy").expect("copy icon");
        r.vcx.simulate_click(icon.center(), Modifiers::none());
        r.draw();
        let expected = r.vcx.read_entity(&page, |p, _| p.copy_all_text());
        let copied = r.vcx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(expected.as_str()));
    }

    #[gpui_kit::test]
    fn information_status_chip_empty(cx: &mut TestAppContext) {
        let mut r = boot(cx);
        r.visit();
        let chip = r.vcx.read_entity(&r.main, |m, _| m.status_chip().clone());
        assert_eq!(r.vcx.read_entity(&chip, |c, _| c.drawn_text()), "");
    }
}

/// Holds one section and counts `OpenAdditionalSetup` dispatches.
struct SetupHost {
    section: Entity<InfoSection>,
    setups: Rc<Cell<usize>>,
    focus: FocusHandle,
}

impl Render for SetupHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let setups = self.setups.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .w(px(600.))
            .h(px(400.))
            .on_action(move |_: &tl_ui::actions::OpenAdditionalSetup, _, _| {
                setups.set(setups.get() + 1)
            })
            .child(self.section.clone().cached(StyleRefinement::default()))
    }
}

#[gpui_kit::test]
fn info_needs_setup_link_opens_setup(cx: &mut TestAppContext) {
    use tl_ui::pages::information::sections::{InfoWords, apply_hints};
    cx.update(|cx| setup(cx, 100));
    let mut rows = vec![InfoRow {
        value: TlValue {
            validity: Validity::Unsupported,
            word: Some(UnsupportedWord::NeedsSetup),
            tooltip: Some("reason".into()),
            ..TlValue::new(SharedString::default())
        },
        ..row("Machine ID", "")
    }];
    apply_hints(&mut rows, &InfoWords::default(), true);
    assert_eq!(rows[0].value.tooltip.as_deref(), Some("reason"));
    assert!(rows[0].value.link.is_some());
    let (theme, words) = cx.update(|cx| (current(cx).clone(), kv_words(cx)));
    let look = value_look_in(&rows[0].value, &words, &theme).expect("drawn");
    assert_eq!(look.text.as_ref(), "Needs setup");
    assert!(look.link);

    let setups = Rc::new(Cell::new(0));
    let section = cx.new(|cx| {
        let mut s = InfoSection::new(InfoSectionId::OperatingSystem, cx);
        s.set_rows(Arc::from(rows), cx);
        s
    });
    let (_, vcx) = cx.add_window_view({
        let (section, setups) = (section.clone(), setups.clone());
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            SetupHost {
                section,
                setups,
                focus,
            }
        }
    });
    let value = bounds(vcx, "TlKv-value-0").expect("value drawn");
    vcx.simulate_click(
        gpui_kit::point(value.left() + px(4.), value.center().y),
        Modifiers::none(),
    );
    assert_eq!(setups.get(), 1);
    vcx.update(|window, cx| window.focus_next(cx));
    vcx.simulate_keystrokes("enter");
    assert_eq!(setups.get(), 2);
}
