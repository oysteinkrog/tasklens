//! The per-tab filter bar of the Properties windows: the field, the 100 ms debounce, the
//! generation rule and the "nothing matches" block (UI-PWN-005, PROC-077, PROC-084, ADR-009).
//!
//! Each tab owns one [`PropsFilter`]. The owner precomputes its lowercase haystacks once per
//! payload version (`Arc<[Box<str>]>`, built with [`lowercase`]). A `Change` in the field starts a
//! 100 ms timer; when it fires the generation goes up by 1 and the filter emits [`FilterChanged`].
//! The owner then calls [`PropsFilter::run_filter`] and applies the result only if
//! [`PropsFilter::accept`] returns true, so a late result of an older generation is dropped and the
//! previous rows stay visible until the new ones arrive.
//!
//! Filter text lives as long as the window and is never saved. Filtering sends no `Command`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Subscription, Task, Window, div,
    px,
};

use crate::components::empty::TlEmptyState;
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::i18n::{tr, tr_args};
use crate::theme::{current, metrics};

gpui_kit::actions!(
    props_filter,
    [
        /// Empties the filter of the active tab. The "Clear filter" button of the empty state
        /// dispatches it.
        ClearPropsFilter
    ]
);

/// Quiet time after the last `Change` before the filter runs (ADR-009).
pub const DEBOUNCE: Duration = Duration::from_millis(100);
/// Tables with at most this many rows filter on the UI thread; larger ones on the background
/// executor.
pub const SYNC_MAX_ROWS: usize = 2_000;
/// Rows per chunk of a background filter. A stale generation stops between chunks.
pub const CHUNK_ROWS: usize = 50_000;
/// Fixed field width, × ui_scale.
const FIELD_WIDTH: f32 = 280.;
/// Bar height, × ui_scale.
const BAR_HEIGHT: f32 = 32.;

/// Fluent key of the default empty-state text.
pub const NO_MATCH: &str = "pwn-props-filter-no-match";
/// Fluent key of the Threads tab empty-state text.
pub const NO_MATCH_THREADS: &str = "pwn-props-filter-no-match-threads";
const PLACEHOLDER: &str = "pwn-props-filter-placeholder";
const ICON_TOOLTIP: &str = "pwn-props-filter-icon-tooltip";
const NAME: &str = "pwn-props-filter-name";

/// Emitted when the debounce fires. `needle` is the lowercase filter text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterChanged {
    pub generation: u64,
    pub needle: SharedString,
}

/// One tab's filter state.
pub struct PropsFilter {
    pub input: Entity<InputState>,
    debounce: Option<Task<()>>,
    generation: u64,
    applied_generation: u64,
    needle_lower: SharedString,
    /// The latest generation, read by background filters between chunks.
    latest: Arc<AtomicU64>,
    /// The tab label, for the accessible name "Filter {tab label}".
    tab_label: SharedString,
    _sub: Subscription,
}

impl EventEmitter<FilterChanged> for PropsFilter {}

impl PropsFilter {
    /// `tab_label` is the tab's label without its count, such as "Threads".
    pub fn new(
        tab_label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule(cx);
            }
        });
        Self {
            input,
            debounce: None,
            generation: 0,
            applied_generation: 0,
            needle_lower: SharedString::default(),
            latest: Arc::new(AtomicU64::new(0)),
            tab_label: tab_label.into(),
            _sub: sub,
        }
    }

    /// The generation of the last debounced change. 0 before the first one.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The generation whose result the owner shows.
    pub fn applied_generation(&self) -> u64 {
        self.applied_generation
    }

    /// The accessible name of the field, "Filter {tab label}".
    pub fn accessible_name(&self, cx: &App) -> SharedString {
        tr_args(cx, NAME, &[("tab", self.tab_label.to_string().into())])
    }

    /// The lowercase filter text the last debounce saw.
    pub fn needle(&self) -> &SharedString {
        &self.needle_lower
    }

    /// True when `generation` is the latest one.
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// Records `generation` as shown and returns true when it is the latest; returns false and
    /// changes nothing for a stale one.
    pub fn accept(&mut self, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.applied_generation = generation;
        true
    }

    /// Empties the field. The `Change` it emits runs the filter after the debounce.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.clean(window, cx));
    }

    /// Restarts the debounce. Replacing the task drops the pending one.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, |this, cx| this.fire(cx)).ok();
        }));
    }

    fn fire(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        let text = self.input.read(cx).text().to_string();
        self.needle_lower = lowercase(&text).into();
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Relaxed);
        cx.emit(FilterChanged {
            generation: self.generation,
            needle: self.needle_lower.clone(),
        });
    }

    /// The indexes of the rows that contain the current needle, tagged with the current
    /// generation. Up to [`SYNC_MAX_ROWS`] rows the task is already done; above that the work runs
    /// on the background executor in chunks of [`CHUNK_ROWS`] and gives `None` once a newer
    /// generation exists.
    pub fn run_filter(
        &mut self,
        rows: Arc<[Box<str>]>,
        cx: &mut Context<Self>,
    ) -> Task<Option<(u64, Arc<[u32]>)>> {
        let generation = self.generation;
        let needle = self.needle_lower.clone();
        if rows.len() <= SYNC_MAX_ROWS {
            let mut out = Vec::new();
            collect(&rows, 0, &needle, &mut out);
            return Task::ready(Some((generation, out.into())));
        }
        let latest = self.latest.clone();
        cx.background_spawn(async move {
            let mut out = Vec::new();
            for (i, chunk) in rows.chunks(CHUNK_ROWS).enumerate() {
                if latest.load(Ordering::Relaxed) != generation {
                    return None;
                }
                collect(chunk, i * CHUNK_ROWS, &needle, &mut out);
            }
            Some((generation, out.into()))
        })
    }
}

/// Appends the index of every row in `rows` that matches, offset by `start`.
fn collect(rows: &[Box<str>], start: usize, needle: &str, out: &mut Vec<u32>) {
    out.extend(
        rows.iter()
            .enumerate()
            .filter(|(_, row)| matches(row, needle))
            .map(|(i, _)| (start + i) as u32),
    );
}

/// Substring match on text already lowered with [`lowercase`]. An empty needle matches all.
pub fn matches(haystack_lower: &str, needle_lower: &str) -> bool {
    haystack_lower.contains(needle_lower)
}

/// Unicode simple lowercase: each char maps to one char, so "İ" gives "i" and "Σ" gives "σ".
pub fn lowercase(text: &str) -> String {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// The filter bar: 32 high, the 280 wide field on the left and the tab's `tools` right-aligned.
/// It handles [`ClearPropsFilter`] while focus is inside it; the owner's tab root handles it for
/// focus elsewhere with [`on_clear`].
pub fn filter_bar(
    filter: &Entity<PropsFilter>,
    tools: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let theme = current(cx);
    let m = *metrics(cx);
    let f = filter.read(cx);
    let input = f.input.clone();
    let name = f.accessible_name(cx);

    div()
        .debug_selector(|| "PropsFilter-bar".into())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .w_full()
        .h(px(BAR_HEIGHT * m.ui_scale))
        .px(m.sp[5])
        .py(m.sp[2])
        .bg(theme.window_bg)
        .border_b_1()
        .border_color(theme.divider)
        .on_action(on_clear(filter))
        .child(
            TlTextInput::new("props-filter", &input)
                .size(TlInputSize::Sm)
                .width(px(FIELD_WIDTH * m.ui_scale))
                .prefix_icon(TlIcon::Search)
                .prefix_tooltip(tr(cx, ICON_TOOLTIP))
                .aria_label(name)
                .placeholder(tr(cx, PLACEHOLDER))
                .clear_button(true)
                .clean_on_escape(true),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .children(tools),
        )
}

/// The [`ClearPropsFilter`] handler for `filter`, for the owner's tab root.
pub fn on_clear(
    filter: &Entity<PropsFilter>,
) -> impl Fn(&ClearPropsFilter, &mut Window, &mut App) + 'static {
    let filter = filter.clone();
    move |_, window, cx| filter.update(cx, |f, cx| f.clear(window, cx))
}

/// The block shown when no row matches. `text` is a Fluent key: [`NO_MATCH`], or
/// [`NO_MATCH_THREADS`] for the Threads tab. Its "Clear filter" button dispatches
/// [`ClearPropsFilter`].
pub fn no_match(text: &'static str, cx: &App) -> impl IntoElement {
    TlEmptyState::new(tr(cx, text)).clear_filter(Box::new(ClearPropsFilter))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        FocusHandle, Modifiers, Render, TestAppContext, VisualTestContext, prelude::FluentBuilder,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    type Result3 = Rc<RefCell<Option<Option<(u64, Arc<[u32]>)>>>>;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        crate::i18n::init_strings(cx, "en-US");
    }

    /// A tab: the bar, then the empty state when `empty` is set. The root holds focus, as the
    /// tab's table does in the window, and handles `ClearPropsFilter` like the owner.
    struct Host {
        filter: Entity<PropsFilter>,
        empty: bool,
        focus: FocusHandle,
        fired: Rc<RefCell<Vec<FilterChanged>>>,
        _sub: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("root")
                .track_focus(&self.focus)
                .on_action(on_clear(&self.filter))
                .size(px(900.))
                .flex()
                .flex_col()
                .child(filter_bar(&self.filter, None, cx))
                .when(self.empty, |d| d.child(no_match(NO_MATCH, cx)))
        }
    }

    fn host(cx: &mut TestAppContext, empty: bool) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let filter = cx.new(|cx| PropsFilter::new("Threads", window, cx));
            let fired = Rc::new(RefCell::new(Vec::new()));
            let sub = cx.subscribe(&filter, {
                let fired = fired.clone();
                move |_, _, event: &FilterChanged, _| fired.borrow_mut().push(event.clone())
            });
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            Host {
                filter,
                empty,
                focus,
                fired,
                _sub: sub,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx)
    }

    fn filter(view: &Entity<Host>, cx: &mut VisualTestContext) -> Entity<PropsFilter> {
        cx.update(|_, cx| view.read(cx).filter.clone())
    }

    /// Types `text` at the caret; emits one `Change`.
    fn type_text(view: &Entity<Host>, cx: &mut VisualTestContext, text: &str) {
        let f = filter(view, cx);
        cx.update(|window, cx| {
            let input = f.read(cx).input.clone();
            input.update(cx, |s, cx| s.insert(text.to_owned(), window, cx));
        });
    }

    fn advance(cx: &mut VisualTestContext, ms: u64) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
    }

    fn rows(items: &[&str]) -> Arc<[Box<str>]> {
        items
            .iter()
            .map(|s| lowercase(s).into_boxed_str())
            .collect()
    }

    fn many_rows(n: usize) -> Arc<[Box<str>]> {
        (0..n)
            .map(|i| format!("row {i}").into_boxed_str())
            .collect()
    }

    /// Starts `run_filter` and stores its output once the task finishes.
    fn start(
        f: &Entity<PropsFilter>,
        cx: &mut VisualTestContext,
        rows: Arc<[Box<str>]>,
    ) -> (bool, Result3) {
        let out: Result3 = Rc::new(RefCell::new(None));
        let ready = cx.update(|_, cx| {
            let task = f.update(cx, |f, cx| f.run_filter(rows, cx));
            let ready = task.is_ready();
            let out = out.clone();
            cx.spawn(async move |_| *out.borrow_mut() = Some(task.await))
                .detach();
            ready
        });
        (ready, out)
    }

    #[test]
    fn matches_case_insensitive_unicode() {
        assert!(matches(&lowercase("ÄBC"), &lowercase("äb")));
        assert!(matches(&lowercase("ÄBC"), &lowercase("ÄB")));
        assert!(matches(&lowercase("abc"), ""), "empty needle matches all");
        assert!(!matches(&lowercase("abc"), &lowercase("a*c")), "no globs");
        assert_eq!(lowercase("İ"), "i", "simple lowercase maps one char to one");
    }

    #[gpui_kit::test]
    fn filter_pid_after_debounce(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, false);
        type_text(&view, cx, "PID");
        advance(cx, 100);
        let f = filter(&view, cx);
        assert_eq!(cx.update(|_, cx| f.read(cx).needle().clone()), "pid");
        let (ready, out) = start(&f, cx, rows(&["pid", "ppid", "user"]));
        assert!(ready);
        cx.run_until_parked();
        let (generation, idx) = out.borrow_mut().take().flatten().expect("a result");
        assert_eq!(generation, 1);
        assert_eq!(&*idx, &[0, 1]);
    }

    #[gpui_kit::test]
    fn debounce_runs_once(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, false);
        let fired = cx.update(|_, cx| view.read(cx).fired.clone());
        type_text(&view, cx, "a");
        advance(cx, 50);
        type_text(&view, cx, "b");
        advance(cx, 99);
        assert!(fired.borrow().is_empty(), "still inside the quiet time");
        advance(cx, 1);
        assert_eq!(
            *fired.borrow(),
            vec![FilterChanged {
                generation: 1,
                needle: "ab".into()
            }]
        );
        advance(cx, 500);
        assert_eq!(fired.borrow().len(), 1, "the filter ran once");
    }

    #[gpui_kit::test]
    fn props_filter_stale_generation_ignored(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, false);
        let f = filter(&view, cx);
        // Generation 1 to 3.
        for text in ["r", "o", "w"] {
            type_text(&view, cx, text);
            advance(cx, 100);
        }
        assert_eq!(cx.update(|_, cx| f.read(cx).generation()), 3);
        let (ready3, out3) = start(&f, cx, many_rows(3_000));
        assert!(!ready3, "generation 3 runs in the background");

        // Generation 4 fires before the executor runs generation 3.
        cx.update(|window, cx| {
            let input = f.read(cx).input.clone();
            input.update(cx, |s, cx| s.set_value("row 1", window, cx));
            f.update(cx, |f, cx| f.fire(cx));
        });
        let (ready4, out4) = start(&f, cx, rows(&["row 1", "row 2"]));
        assert!(ready4);
        cx.run_until_parked();

        let (g4, idx4) = out4.borrow_mut().take().flatten().expect("generation 4");
        assert_eq!((g4, &*idx4), (4, &[0u32][..]));
        assert!(cx.update(|_, cx| f.update(cx, |f, _| f.accept(g4))));
        assert_eq!(out3.borrow_mut().take(), Some(None), "generation 3 stops");
        assert!(!cx.update(|_, cx| f.update(cx, |f, _| f.accept(3))));
        assert_eq!(cx.update(|_, cx| f.read(cx).applied_generation()), 4);
    }

    #[gpui_kit::test]
    fn background_above_2000_rows(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, false);
        let f = filter(&view, cx);
        type_text(&view, cx, "row 1");
        advance(cx, 100);

        let (ready, out) = start(&f, cx, many_rows(2_000));
        assert!(ready, "2,000 rows filter synchronously");
        cx.run_until_parked();
        assert!(out.borrow().is_some());

        let (ready, out) = start(&f, cx, many_rows(3_000));
        assert!(!ready, "3,000 rows filter on the background executor");
        assert!(out.borrow().is_none(), "no result before the executor runs");
        cx.run_until_parked();
        let (_, idx) = out.borrow_mut().take().flatten().expect("a result");
        // "row 1", "row 10".."row 19", "row 100".."row 199", "row 1000".."row 1999".
        assert_eq!(idx.len(), 1 + 10 + 100 + 1_000);
        assert_eq!(idx[0], 1);
    }

    #[gpui_kit::test]
    fn accessible_name_names_the_tab(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, false);
        let f = filter(&view, cx);
        assert_eq!(
            cx.update(|_, cx| f.read(cx).accessible_name(cx)),
            "Filter Threads"
        );
    }

    #[gpui_kit::test]
    fn empty_state_clear_button(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, true);
        type_text(&view, cx, "zzz");
        advance(cx, 100);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let text = cx
            .debug_bounds("TlEmptyState-text")
            .expect("empty state text");
        assert!(text.size.width > px(0.));
        assert_eq!(
            cx.update(|_, cx| tr(cx, NO_MATCH)),
            "No properties match the current filter."
        );
        let button = cx
            .debug_bounds("TlButton-TlEmptyState-clear-filter")
            .expect("Clear filter button");
        cx.simulate_click(button.center(), Modifiers::none());
        cx.run_until_parked();

        let f = filter(&view, cx);
        assert_eq!(
            cx.update(|_, cx| f.read(cx).input.read(cx).text().to_string()),
            ""
        );
        advance(cx, 100);
        assert_eq!(cx.update(|_, cx| f.read(cx).needle().clone()), "");
    }
}
