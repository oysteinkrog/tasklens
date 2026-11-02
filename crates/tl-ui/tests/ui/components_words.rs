//! The key/value grid state words and the CPU Details cache and boot time words come from the
//! catalogue, translated once per strings version (R-132). Split from `components.rs`, which moved
//! to tl-ui-kit: this part reads `tl_ui::pages::performance`, so it stays in the shell crate.

use gpui_kit::{
    App, Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_ui::components::checkbox::bind_keys_components;

fn setup(cx: &mut App) {
    tl_ui_pagehost::install_theme(cx);
    bind_keys_components(cx);
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

use gpui_kit::{MouseButton, SharedString, point};
use tl_model::snapshot::{CacheInfo, CacheLevelInfo, CpuStaticInfo, SystemSnapshot};
use tl_model::validity::Validity;
use tl_ui::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord, kv_words,
    shown_text_in,
};
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::i18n::{init_strings, set_test_catalogue, tr_calls};
use tl_ui::pages::performance::cpu_details::{
    CpuLiveInputs, ROW_L1, ROW_L3, ROW_UP_TIME, cpu_details_model, update_live, update_static,
};
use tl_ui::pages::performance::details::KeyValueModel;

const MARKERS: &str = "
fnd-kv-collecting = <collecting>
fnd-kv-restricted = <restricted>
fnd-kv-unavailable = <unavailable>
fnd-kv-not-supported = <not-supported>
fnd-kv-not-reported = <not-reported>
fnd-kv-needs-setup = <needs-setup>
fnd-kv-no-match = <no-match>
fnd-kv-copy-value = <copy-value>
fnd-kv-copy-row = <copy-row>
fnd-kv-copy-section = <copy-section-in-a-test-locale>
perf-cpu-tip-boot = <boot {$time}>
perf-cpu-tip-l1-data = <data {$size}>
perf-cpu-tip-l1-instruction = <instruction {$size}>
perf-cpu-tip-l4 = <l4 {$size}>
perf-cpu-time-unknown = <unknown>
";

/// One value per non-valid state, in this order.
fn state_values() -> Vec<TlValue> {
    let state = |validity, word| TlValue {
        validity,
        word,
        ..TlValue::new("0")
    };
    vec![
        state(Validity::WarmingUp, None),
        state(Validity::Restricted, None),
        state(Validity::Failed, None),
        state(Validity::Unsupported, None),
        state(Validity::Unsupported, Some(UnsupportedWord::NotReported)),
        state(Validity::Unsupported, Some(UnsupportedWord::NeedsSetup)),
    ]
}

const SHOWN: [&str; 6] = [
    "<collecting>",
    "<restricted>",
    "<unavailable>",
    "<not-supported>",
    "<not-reported>",
    "<needs-setup>",
];

struct Grid;

impl Render for Grid {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = state_values()
            .into_iter()
            .enumerate()
            .map(|(i, value)| TlKvRow {
                label: format!("Row {i}:").into(),
                value,
            })
            .collect();
        let section = TlKvSection {
            title: None,
            blocks: vec![TlKvBlock {
                indent: None,
                heading: None,
                rows,
            }],
        };
        div().size(px(800.)).child(
            TlKeyValueGrid::new("grid", KvLayout::Inline)
                .copy_on_click(true)
                .sections(&[section]),
        )
    }
}

#[gpui_kit::test]
fn kv_grid_words_follow_test_locale(cx: &mut TestAppContext) {
    cx.update(|cx| {
        setup(cx);
        init_strings(cx, "en-US");
        assert_eq!(kv_words(cx).restricted, "Restricted");
        set_test_catalogue(cx, MARKERS);
        let before = tr_calls();
        let words = kv_words(cx);
        assert!(tr_calls() > before, "a new strings version translates");
        let shown: Vec<SharedString> = state_values()
            .iter()
            .filter_map(|v| shown_text_in(v, &words))
            .collect();
        assert_eq!(shown, SHOWN);
    });

    let (_, cx) = cx.add_window_view(|_, _| Grid);
    draw(cx);
    let restricted = cx.debug_bounds("TlKv-value-1").expect("value drawn");
    cx.simulate_click(restricted.center(), Modifiers::none());
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("<restricted>"),
        "the grid draws the translated word"
    );

    let before = tr_calls();
    for _ in 0..60 {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }
    assert_eq!(tr_calls(), before, "60 frames make no tr call");
}

/// A counted text has a form per plural category, and writing it makes no tr call
/// (o55.37).
#[gpui_kit::test]
fn counted_template_plural_forms(cx: &mut TestAppContext) {
    use tl_ui::template::Template;

    const KEY: &str = "perf-gpu-enc-sessions";
    fn written(t: &Template, n: u64) -> String {
        let mut out = String::new();
        t.write_count(&mut out, n, &[&n])
            .expect("a String takes the text");
        out
    }
    cx.update(|cx| {
        setup(cx);
        init_strings(cx, "en-US");
        let t = Template::counted(cx, KEY, &["n"], 0);
        assert_eq!(t, Template::counted_en(KEY, &["n"], 0));
        let before = tr_calls();
        assert_eq!(written(&t, 0), "0 active sessions");
        assert_eq!(written(&t, 1), "1 active session");
        assert_eq!(written(&t, 2), "2 active sessions");
        assert_eq!(written(&t, 21), "21 active sessions");
        assert_eq!(tr_calls(), before, "writes make no tr call");

        // The sample 0 shows twice in the other form, so the next sample marks the count.
        set_test_catalogue(
            cx,
            "perf-gpu-enc-sessions = { $n ->\n    [one] <one {$n}>\n   *[other] <{$n} of 0>\n}\n",
        );
        let t = Template::counted(cx, KEY, &["n"], 0);
        assert_eq!(written(&t, 1), "<one 1>");
        assert_eq!(written(&t, 5), "<5 of 0>");
    });
}

/// An empty filtered grid over a grid with a titled section.
struct OwnText;

impl Render for OwnText {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let section = TlKvSection {
            title: Some("Memory".into()),
            blocks: vec![TlKvBlock {
                indent: None,
                heading: None,
                rows: vec![TlKvRow {
                    label: "Total:".into(),
                    value: TlValue::new("32 GiB"),
                }],
            }],
        };
        div()
            .w(px(800.))
            .child(
                div()
                    .h(px(100.))
                    .child(TlKeyValueGrid::new("empty", KvLayout::Inline).filtered_empty(true)),
            )
            .child(TlKeyValueGrid::new("band", KvLayout::Inline).sections(&[section]))
    }
}

fn width(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    f32::from(cx.debug_bounds(selector).expect(selector).size.width)
}

/// The test text system gives every char the same advance, so a text `width` wide is `text`
/// when it is `text`'s length in chars of the en-US text, up to a pixel of rounding.
fn assert_drawn(width: f32, en_width: f32, en: &str, text: &str) {
    let expected = en_width / en.chars().count() as f32 * text.chars().count() as f32;
    assert!(
        (width - expected).abs() <= 1.,
        "{text} drawn: {width} px, expected {expected} px"
    );
}

fn tooltip_width(cx: &mut VisualTestContext) -> f32 {
    let icon = cx
        .debug_bounds("TlButton-band-copy-section0-icon")
        .expect("copy section button");
    cx.simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(icon.center(), None, Modifiers::none());
    cx.executor().advance_clock(DELAY_CONTROL);
    cx.run_until_parked();
    draw(cx);
    draw(cx);
    width(cx, "TlTooltip-text")
}

#[gpui_kit::test]
fn kv_grid_own_text_follows_test_locale(cx: &mut TestAppContext) {
    const NO_MATCH: &str = "No properties match the current filter.";
    cx.update(|cx| {
        setup(cx);
        init_strings(cx, "en-US");
        let words = kv_words(cx);
        assert_eq!(words.no_match, NO_MATCH);
        assert_eq!(
            [&words.copy_value, &words.copy_row, &words.copy_section],
            ["Copy value", "Copy row", "Copy section"]
        );
    });
    let (_, cx) = cx.add_window_view(|_, _| OwnText);
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    draw(cx);
    let empty_en = width(cx, "TlKv-empty");
    let tip_en = tooltip_width(cx);

    cx.update(|window, cx| {
        window.refresh();
        set_test_catalogue(cx, MARKERS);
        let words = kv_words(cx);
        assert_eq!(
            [
                &words.no_match,
                &words.copy_value,
                &words.copy_row,
                &words.copy_section
            ],
            [
                "<no-match>",
                "<copy-value>",
                "<copy-row>",
                "<copy-section-in-a-test-locale>"
            ]
        );
    });
    draw(cx);
    let empty = width(cx, "TlKv-empty");
    assert_drawn(empty, empty_en, NO_MATCH, "<no-match>");
    let tip = tooltip_width(cx);
    assert_drawn(
        tip,
        tip_en,
        "Copy section",
        "<copy-section-in-a-test-locale>",
    );

    // Frames with the context menu open read the cached words.
    let value = cx.debug_bounds("TlKv-value-0").expect("value drawn");
    cx.simulate_mouse_down(value.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(value.center(), MouseButton::Right, Modifiers::none());
    draw(cx);
    let before = tr_calls();
    for _ in 0..60 {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }
    assert_eq!(tr_calls(), before, "60 frames make no tr call");
    // Items: copy value, copy row, copy section.
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("Memory\n    Total: 32 GiB"),
        "the menu's copy section item"
    );
}

fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
    Some(CacheLevelInfo {
        instances,
        instance_kib,
        total_kib: u64::from(instances) * instance_kib,
    })
}

fn tip(model: &KeyValueModel, label: &str) -> Option<String> {
    let row = model.rows.iter().find(|r| r.label == label)?;
    row.tooltip.as_ref().map(|t| t.to_string())
}

#[gpui_kit::test]
fn cpu_cache_and_boot_words_follow_test_locale(cx: &mut TestAppContext) {
    let mut model = cpu_details_model();
    cx.update(|cx| {
        init_strings(cx, "en-US");
        set_test_catalogue(cx, MARKERS);
        assert!(model.refresh_strings(cx));
    });
    let info = CpuStaticInfo {
        cores: 8,
        logical: 16,
        caches: CacheInfo {
            l1d: level(16, 32),
            l1i: level(16, 32),
            l3: level(1, 98_304),
            l4: level(1, 131_072),
            ..CacheInfo::default()
        },
        ..CpuStaticInfo::default()
    };
    let before = tr_calls();
    assert!(update_static(&mut model, &info));
    assert_eq!(
        tip(&model, ROW_L1).as_deref(),
        Some("<data 16 × 32 KiB> + <instruction 16 × 32 KiB>")
    );
    assert_eq!(
        tip(&model, ROW_L3).as_deref(),
        Some("98\u{2009}304 KiB · 1 × 96 MiB · <l4 128 MiB>")
    );

    // A boot time past what a date holds.
    let sys = SystemSnapshot {
        boot_time_s: i64::MAX,
        ..SystemSnapshot::default()
    };
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert_eq!(
        tip(&model, ROW_UP_TIME).as_deref(),
        Some("<boot <unknown>>")
    );

    for _ in 0..60 {
        update_static(&mut model, &info);
        update_live(&mut model, &sys, &CpuLiveInputs::default());
    }
    assert_eq!(tr_calls(), before, "updates make no tr call");
}
