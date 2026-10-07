//! Information section entity (E28-030, UI-SYS-081, UI-SYS-082, UI-FND-025).

use std::cell::RefCell;
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
        InfoInputs, InfoWords, build_bios, build_motherboard, build_os, build_processor,
        build_system, find_logo,
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
    fn inventory() -> InventorySnapshot {
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

    fn system(uptime_s: u64) -> SystemSnapshot {
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
    use tl_ui::i18n::set_test_catalogue;

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
