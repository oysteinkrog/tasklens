//! Debug selector counts for page tests (tasklens-6swdr.11, ADR-019). Every debug selector goes
//! through `tl_ui_base::selector::TlSelector`, so a test can count how many painted elements
//! share one. GPUI keeps one `debug_bounds` entry per selector, so a second element with the
//! same selector hides the first from every test that looks it up.

use std::collections::BTreeMap;

use gpui_kit::VisualTestContext;
use tl_ui_base::selector::{tag_frame, untag_frame};

/// Every selector the window of `vcx` paints in one frame, with how many elements painted it.
/// The frame renders every view fresh, so cached views count too (see `tl_ui_base::selector`).
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
