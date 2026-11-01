//! Debug selectors (tasklens-6swdr.11, ADR-019). Every tl-ui debug selector goes through
//! `tl_ui::selector::TlSelector`, so a test can count how many painted elements share one.
//! GPUI keeps one `debug_bounds` entry per selector, so a second element with the same selector
//! hides the first from every test that looks it up.

use std::collections::BTreeMap;

use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div,
};
use tl_ui::selector::{TlSelector as _, tag_frame, untag_frame};

/// Every selector the window of `vcx` paints in one frame, with how many elements painted it.
/// The frame renders every view fresh, so cached views count too (see `tl_ui::selector`).
pub fn drawn_selectors(vcx: &mut VisualTestContext) -> BTreeMap<String, usize> {
    vcx.run_until_parked();
    let tagged = vcx.update(tag_frame);
    let mut drawn = BTreeMap::new();
    for t in tagged {
        let name: &'static str = Box::leak(t.tagged.into_boxed_str());
        if vcx.debug_bounds(name).is_some() {
            *drawn.entry(t.name).or_insert(0) += 1;
        }
    }
    vcx.update(untag_frame);
    drawn
}

/// Asserts the window of `vcx` paints no selector twice, and returns the painted ones.
pub fn assert_none_drawn_twice(vcx: &mut VisualTestContext, what: &str) -> BTreeMap<String, usize> {
    let drawn = drawn_selectors(vcx);
    let twice: Vec<(&String, &usize)> = drawn.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        twice.is_empty(),
        "{what}: {} selectors drawn more than once: {twice:?}",
        twice.len()
    );
    drawn
}

/// No file in `crates/tl-ui/src` calls GPUI's `debug_selector` outside the helper module, so
/// the drawn-selector count sees every selector.
#[test]
fn every_selector_goes_through_the_helper() {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("source file");
                for (n, line) in text.lines().enumerate() {
                    if line.contains("debug_selector(") {
                        out.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let helper = src.join("selector.rs");
    let text = std::fs::read_to_string(&helper).expect("helper module");
    assert!(
        text.contains(".debug_selector("),
        "the helper module sets the selector"
    );
    let mut calls = Vec::new();
    walk(&src, &mut calls);
    let helper = format!("{}:", helper.display());
    calls.retain(|c| !c.starts_with(&helper));
    assert!(
        calls.is_empty(),
        "call `tl_selector` (tl_ui::selector) instead of `debug_selector`: {calls:#?}"
    );
}

/// A view that builds one selector without painting it and paints another one twice.
struct Twice;

impl Render for Twice {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Built but never painted.
        let _dropped = div().tl_selector(|| "unpainted".into());
        div()
            .size_full()
            .child(div().size_4().tl_selector(|| "twice".into()))
            .child(div().size_4().tl_selector(|| "twice".into()))
            .child(div().size_4().tl_selector(|| "once".into()))
            .child(div().size_4().id("x").tl_selector(|| "stateful".into()))
    }
}

/// The count is of painted elements: a selector that is built but not painted does not count,
/// and plain lookups work again after the count.
#[gpui_kit::test]
fn drawn_selectors_count_painted_elements(cx: &mut TestAppContext) {
    let (_, vcx) = cx.add_window_view(|_, _| Twice);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let drawn = drawn_selectors(vcx);
    let want: BTreeMap<String, usize> = [("once", 1), ("stateful", 1), ("twice", 2)]
        .into_iter()
        .map(|(s, n)| (s.to_owned(), n))
        .collect();
    assert_eq!(drawn, want);
    assert!(vcx.debug_bounds("once").is_some(), "plain selector back");
    // Twice in a row gives the same count: the tagged frame leaves nothing behind.
    assert_eq!(drawn_selectors(vcx), want);
}

/// The check names each selector painted more than once.
#[gpui_kit::test]
#[should_panic(expected = "1 selectors drawn more than once: [(\"twice\", 2)]")]
fn assert_none_drawn_twice_names_the_repeats(cx: &mut TestAppContext) {
    let (_, vcx) = cx.add_window_view(|_, _| Twice);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert_none_drawn_twice(vcx, "Twice");
}
