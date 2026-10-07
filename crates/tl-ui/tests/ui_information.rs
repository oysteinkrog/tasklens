//! Information section entity (E28-030, UI-SYS-081, UI-SYS-082, UI-FND-025).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    Modifiers, ParentElement, Pixels, Render, SharedString, StyleRefinement, Styled,
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

#[ignore = "TlKeyValueGrid indents only blocks with a heading (tasklens-9o0.21 question)"]
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

#[ignore = "TlKeyValueGrid draws one line per value (tasklens-9o0.21 question)"]
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
