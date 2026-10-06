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

#[test]
fn kv_grid_fallback_words_per_validity() {
    use Fallback::*;
    use Validity::*;
    let cases: [(Validity, Fallback, Option<&str>, &str, Option<&str>, bool); 8] = [
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
    for n in 0..14 {
        let selector: &'static str = Box::leak(format!("TlKv-row-{n}").into_boxed_str());
        let b = cx.debug_bounds(selector).expect("pair drawn");
        let (line, column) = (n / 3, n % 3);
        assert_eq!(b.size.height, px(46.), "pair {n} height");
        assert_eq!(b.origin.y, px(46. * line as f32), "pair {n} y");
        if line == 0 {
            xs.push(b.origin.x);
        } else {
            assert_eq!(b.origin.x, xs[column], "pair {n} column");
        }
    }
    assert_eq!(xs.len(), 3);
    assert!(xs[0] < xs[1] && xs[1] < xs[2], "three columns: {xs:?}");
    // 14 pairs fill 5 lines: the last line holds pairs 12 and 13.
    assert!(cx.debug_bounds("TlKv-row-14").is_none());
}
