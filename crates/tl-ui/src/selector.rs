//! Debug selectors (ADR-019).
//!
//! Every tl-ui debug selector is set through [`TlSelector::tl_selector`], never through GPUI's
//! `debug_selector` directly. A source sweep in the ui tests fails on any other call.
//!
//! GPUI 0.3.7 keeps one `debug_bounds` entry per selector: when two painted elements share a
//! selector, the later one silently wins. Its list of every painted selector is private. So under
//! the `test-hooks` feature this module can tag one frame: [`tag_frame`] draws a window with
//! every view rendered fresh and gives each selector set during that frame a tag unique in the
//! frame. GPUI records the tagged selector only for an element it paints, so a test keeps the
//! tags whose bounds exist and counts the painted elements per selector. Elements that are built
//! but not painted (for example the row a list builds only to measure its height) drop out.
//!
//! Cached views: [`tag_frame`] calls `Window::refresh`, which makes every cached view render
//! again in the next draw instead of reusing its last paint. So reused paint cannot hide a
//! selector from the tagged frame. [`untag_frame`] draws once more the same way with tagging
//! off, so later lookups by plain selector work again.
//!
//! Outside a tagged frame the helper costs one thread-local read and then does what
//! `debug_selector` does: nothing in a build without GPUI's `test-support`.

use gpui_kit::InteractiveElement;

/// Sets an element's debug selector. Use this instead of `debug_selector`.
pub trait TlSelector: Sized {
    /// Sets the selector `f` returns. `f` runs only in builds that keep debug selectors.
    fn tl_selector(self, f: impl FnOnce() -> String) -> Self;
}

impl<E: InteractiveElement> TlSelector for E {
    #[inline]
    fn tl_selector(self, f: impl FnOnce() -> String) -> Self {
        #[cfg(feature = "test-hooks")]
        if tagging::active() {
            let tagged = tagging::push(f());
            return self.debug_selector(move || tagged);
        }
        self.debug_selector(f)
    }
}

#[cfg(feature = "test-hooks")]
pub use tagging::{TaggedSelector, tag_frame, untag_frame};

#[cfg(feature = "test-hooks")]
mod tagging {
    use std::cell::RefCell;

    use gpui_kit::{App, Window};

    /// Joins a selector and its tag. A selector never holds this character.
    const SEP: char = '\u{1f}';

    /// One selector set during a tagged frame.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TaggedSelector {
        /// The selector the code set.
        pub name: String,
        /// The selector GPUI recorded for it in the tagged frame. Its bounds exist only when the
        /// element was painted.
        pub tagged: String,
    }

    thread_local! {
        /// The selectors set so far in the tagged frame; `None` outside one.
        static LOG: RefCell<Option<Vec<TaggedSelector>>> = const { RefCell::new(None) };
    }

    /// True while [`tag_frame`] draws.
    pub(super) fn active() -> bool {
        LOG.with_borrow(Option::is_some)
    }

    /// Logs `name` in the tagged frame and returns its tagged form.
    pub(super) fn push(name: String) -> String {
        LOG.with_borrow_mut(|log| {
            let log = log.get_or_insert_with(Vec::new);
            let tagged = format!("{name}{SEP}{}", log.len());
            log.push(TaggedSelector {
                name,
                tagged: tagged.clone(),
            });
            tagged
        })
    }

    /// Draws `window` once with every view rendered fresh and every selector tagged, and
    /// returns the selectors set during that frame in the order they were set. Call
    /// [`untag_frame`] when done.
    pub fn tag_frame(window: &mut Window, cx: &mut App) -> Vec<TaggedSelector> {
        LOG.with_borrow_mut(|log| *log = Some(Vec::new()));
        window.refresh();
        window.draw(cx).clear(cx);
        LOG.with_borrow_mut(Option::take).unwrap_or_default()
    }

    /// Draws `window` once more with every view rendered fresh and plain selectors.
    pub fn untag_frame(window: &mut Window, cx: &mut App) {
        LOG.with_borrow_mut(|log| *log = None);
        window.refresh();
        window.draw(cx).clear(cx);
    }
}
