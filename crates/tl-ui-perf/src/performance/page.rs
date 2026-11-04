//! The device page frame every Performance page renders through: the header row with the type
//! word, the model and the page toggles, the graph area, the "Details" heading and grid, and the
//! page blocks, inside a scrolling column (docs/ui/performance.md UI-PRF-010, UI-PRF-020;
//! PERF-008, PERF-012, PERF-014, R-058).
//!
//! The header words and the "Details" heading are `en-US.ftl` keys, translated once per strings
//! version into the [`PageWords`] global that every page shares.
//!
//! The header ends in the range switch (1 min to 7 d) the Overview has. It writes the one shared
//! range, `performance.overview_range`, which every page chart draws ([`page_range_ns`]); 1 h,
//! 24 h and 7 d read the device metric history, which the page asks for with
//! [`page_history_request`]. A chart whose series that history does not hold stays on its live
//! window ([`live_range_ns`]) and shows [`live_only_note`] (crw.105).
//!
//! A page whose body is a widget container (ADR-019 phase 3, [`DevicePage::widget_editor`])
//! keeps its header row at the top and only the body under it scrolls: the container's widget
//! editor fills the rest of the page, with the edit toolbar between the header and the body in
//! Edit Widgets. The graph area, the Details grid and the blocks are the container's widgets
//! then; [`page_body`] draws the Details grid of the 1.3.0 frame for the Details widget.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, ElementId, Entity, FocusHandle, FontWeight,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, canvas, div,
    px,
};
use smallvec::SmallVec;
use tl_model::demand::DeviceHistoryRequest;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;
use tl_ui_settings::settings::OverviewRange;

use super::actions::PerfShowMiniWindow;
use super::aggregate::{AggregateDiskPage, AggregateGpuPage};
use super::card::CardValue;
use super::cpu::CpuPage;
use super::details::{KeyValueModel, render_details};
use super::disk::DiskPage;
use super::gpu::GpuPage;
use super::memory::MemoryPage;
use super::net::NetPage;
use super::sensors::{BatteryPage, FanPage};
use super::sidebar::DeviceSidebar;
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::performance::overview::parts::title_value_style;
use crate::performance::overview::range::{
    PAGE_SWITCH, SwitchWords, current_range, history_kept, history_request, page_range_switch,
};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::{SnapshotChanged, SnapshotHub};
use tl_ui_base::i18n::{Strings, tr, tr_en};
use tl_ui_chart::chart::{ChartView, NetFormat};
use tl_ui_kit::components::icons::{TlIcon, icon};
use tl_ui_kit::components::menu::TlMenu;
use tl_ui_kit::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
// page_range_ns moved to overview/range.rs (tasklens-zljr5.22). This keeps the old path working.
pub use crate::performance::overview::range::page_range_ns;
use tl_ui_base::selector::TlSelector;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::template::Template;
use tl_ui_base::text::style::{label_style, t_sm};
use tl_ui_base::theme::store::ThemeStore;
use tl_ui_base::theme::{TlTheme, UiMetrics, current, metrics};

pub use super::core::{GRAPH_H_OVERALL, PAGE_PAD};
// The DevicePage trait moved into overview/ (tasklens-zljr5.22).
pub use super::overview::device_page::{DetailsLinkClick, DetailsValuePrefix, DevicePage};
/// Height of the header row, × ui_scale. `page_header_h`.
pub const PAGE_HEADER_H: f32 = 36.;
/// Space between the graph area and the "Details" heading, × ui_scale. `details_gap`.
pub const DETAILS_GAP: f32 = 24.;
/// The page scrolls sideways below this width, × ui_scale.
pub const PAGE_MIN_W: f32 = 400.;
/// Height of a page's only chart, × ui_scale.
pub const GRAPH_H_SINGLE: f32 = 240.;
/// Height of each of two stacked charts, × ui_scale.
pub const GRAPH_H_STACKED: f32 = 150.;
/// Height of a secondary chart under the main one, × ui_scale.
pub const SECONDARY_GRAPH_H: f32 = 120.;

/// Minimum width of a header toggle, × ui_scale (measured).
const TOGGLE_MIN_W: f32 = 122.;
/// Height of the "Details" heading row, × ui_scale.
const DETAILS_HEADING_H: f32 = 24.;

/// Element id of the graph area.
pub const GRAPH_AREA_ID: &str = "perf-graph-area";
/// Debug selector of the range switch at the right end of the header.
pub const RANGE_ID: &str = "perf-page-range-switch";
/// Debug selector of the "Live only" note over a chart.
pub const LIVE_ONLY_ID: &str = "perf-live-only";

/// The range of a page chart that reads only the live series: the shared range at 1 min and
/// 5 min, `performance.graph_range_s` (the sidebar tiles' window) at 1 h, 24 h and 7 d.
pub fn live_range_ns(cx: &App) -> u64 {
    let range = current_range(cx);
    if range.from_history() && cx.has_global::<TlSettings>() {
        u64::from(TlSettings::settings(cx).performance.graph_range_s) * 1_000_000_000
    } else {
        range.span_ns()
    }
}

/// What a page whose charts draw `range_ns` asks the device metric history for: the range
/// ending now for 1 h, 24 h and 7 d, nothing for 1 min and 5 min.
pub fn page_history_request(range_ns: u64) -> Option<DeviceHistoryRequest> {
    OverviewRange::ALL
        .into_iter()
        .find(|r| r.span_ns() == range_ns)
        .and_then(|r| history_request(r, None, 0))
}

/// "Live only", right-aligned over a chart whose series the device metric history does not hold,
/// while the shared range is 1 h, 24 h or 7 d. `None` at 1 min and 5 min.
pub fn live_only_note(m: &UiMetrics, cx: &mut App) -> Option<AnyElement> {
    if !current_range(cx).from_history() {
        return None;
    }
    let text = page_words(cx).live_only.clone();
    let color = current(cx).text_secondary;
    let mut note = div();
    *note.text_style() = t_sm(m);
    Some(
        h_flex()
            .w_full()
            .justify_end()
            .child(
                note.tl_selector(|| LIVE_ONLY_ID.into())
                    .text_color(color)
                    .child(text),
            )
            .into_any_element(),
    )
}

/// The header words and the "Details" heading, translated once per strings version. Shared by
/// every page as a global; read it with [`page_words`].
#[derive(Clone, Debug)]
pub struct PageWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    /// The Overview page's title; it has no header row (D-022).
    pub overview: SharedString,
    pub processor: SharedString,
    pub memory: SharedString,
    pub gpu: SharedString,
    /// "Disk {$n}".
    pub disk: Template,
    /// "Fan {$n}".
    pub fan: Template,
    pub battery: SharedString,
    /// The battery kind words, in [`BATTERY_KINDS`] order.
    battery_kinds: Vec<SharedString>,
    pub aggregate_disk: SharedString,
    pub aggregate_gpu: SharedString,
    /// "{$k} disks".
    pub disks_count: Template,
    /// "{$k} adapters".
    pub adapters_count: Template,
    pub unknown_processor: SharedString,
    pub unknown_gpu: SharedString,
    pub unknown_adapter: SharedString,
    pub details: SharedString,
    /// The header's range switch.
    pub range: SwitchWords,
    /// "Live only" over a chart the device metric history does not serve.
    pub live_only: SharedString,
}

impl Global for PageWords {}

impl Default for PageWords {
    /// The en-US words.
    fn default() -> Self {
        Self::build(None, |key| tr_en(key, &[]), Template::en)
    }
}

/// The UPower device kinds the engine stores in `BatteryDetails.kind`, with the Fluent key of
/// each kind word. Any other kind shows the last word, "Other".
pub const BATTERY_KINDS: [(&str, &str); 9] = [
    ("Battery", "perf-bat-kind-battery"),
    ("UPS", "perf-bat-kind-ups"),
    ("Mouse", "perf-bat-kind-mouse"),
    ("Keyboard", "perf-bat-kind-keyboard"),
    ("Headset", "perf-bat-kind-headset"),
    ("Gaming input", "perf-bat-kind-gaming"),
    ("Phone", "perf-bat-kind-phone"),
    ("Tablet", "perf-bat-kind-tablet"),
    ("Other", "perf-bat-kind-other"),
];

impl PageWords {
    /// The translated word of the battery `kind` the engine stored.
    pub fn battery_kind(&self, kind: &str) -> &SharedString {
        let i = BATTERY_KINDS
            .iter()
            .position(|(k, _)| *k == kind)
            .unwrap_or(BATTERY_KINDS.len() - 1);
        &self.battery_kinds[i]
    }

    /// The words in the current strings; en-US without strings (tests).
    fn new(cx: &App) -> Self {
        let version = strings_version(cx);
        if version.is_none() {
            return Self::default();
        }
        Self::build(
            version,
            |key| tr(cx, key),
            |key, args| Template::new(cx, key, args),
        )
    }

    fn build(
        version: Option<u64>,
        t: impl Fn(&str) -> SharedString,
        template: impl Fn(&str, &[&str]) -> Template,
    ) -> Self {
        Self {
            version,
            overview: t("perf-card-heading-overview"),
            processor: t("perf-header-processor"),
            memory: t("perf-header-memory"),
            gpu: t("perf-header-gpu"),
            disk: template("perf-header-disk", &["n"]),
            fan: template("perf-header-fan", &["n"]),
            battery: t("perf-header-battery"),
            battery_kinds: BATTERY_KINDS.iter().map(|(_, key)| t(key)).collect(),
            aggregate_disk: t("perf-card-aggregate-storage"),
            aggregate_gpu: t("perf-card-aggregate-graphics"),
            disks_count: template("perf-header-disks-count", &["k"]),
            adapters_count: template("perf-header-adapters-count", &["k"]),
            unknown_processor: t("perf-unknown-processor"),
            unknown_gpu: t("perf-unknown-gpu"),
            unknown_adapter: t("perf-unknown-adapter"),
            details: t("perf-details-heading"),
            range: SwitchWords::build(&t),
            live_only: t("perf-page-live-only"),
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

/// The page words, translated again first when the strings changed.
pub fn page_words(cx: &mut App) -> &PageWords {
    let version = strings_version(cx);
    if cx
        .try_global::<PageWords>()
        .is_none_or(|w| w.version != version)
    {
        let words = PageWords::new(cx);
        cx.set_global(words);
    }
    cx.global::<PageWords>()
}

/// The text of a present inventory or details field: Valid or Stale and not empty.
fn shown(text: &str, validity: Validity) -> Option<&str> {
    let ok = matches!(validity, Validity::Valid | Validity::Stale) && !text.trim().is_empty();
    ok.then_some(text)
}

/// The header strings for a device page (PERF-008, UI-PRF-020) in `words` ([`page_words`]).
/// `number` is the device's index among the visible devices of its kind
/// ([`super::type_number`]). The model text falls back to "Unknown processor", "Unknown GPU" or
/// "Unknown adapter" (PERF-013).
pub fn header_text(
    words: &PageWords,
    kind: DeviceKind,
    number: u16,
    entry: &DeviceEntry,
    snap: &Snapshot,
) -> (SharedString, SharedString) {
    let idx = entry.idx;
    let w = words;
    match kind {
        DeviceKind::Overview => (w.overview.clone(), SharedString::default()),
        DeviceKind::Cpu => {
            let p = &snap.inventory.processor.name;
            let model = shown(&p.value, p.validity).unwrap_or(w.unknown_processor.as_ref());
            (w.processor.clone(), model.trim().to_owned().into())
        }
        DeviceKind::Memory => (w.memory.clone(), SharedString::default()),
        DeviceKind::Gpu => {
            let name =
                shown(&entry.display_name, Validity::Valid).unwrap_or(w.unknown_gpu.as_ref());
            (w.gpu.clone(), name.to_owned().into())
        }
        DeviceKind::Disk => {
            let kernel = match &entry.identity {
                DeviceIdentity::Disk { name, .. } => &**name,
                _ => &*entry.display_name,
            };
            let details = snap
                .device_details
                .disks
                .iter()
                .find(|(i, _)| *i == idx)
                .and_then(|(_, d)| shown(&d.model.value, d.model.validity));
            let model = details.or_else(|| {
                snap.inventory
                    .disks
                    .iter()
                    .find(|d| d.device == Some(idx))
                    .and_then(|d| shown(&d.model.value, d.model.validity))
            });
            // No model: the kernel name alone.
            let text = match model {
                Some(model) => format!("{} ({kernel})", model.trim()),
                None => kernel.to_owned(),
            };
            (numbered(&w.disk, number), text.into())
        }
        DeviceKind::Net => {
            let ifname = match &entry.identity {
                DeviceIdentity::Net { name, .. } => &**name,
                _ => &*entry.display_name,
            };
            let details = snap
                .device_details
                .nets
                .iter()
                .find(|(i, _)| *i == idx)
                .and_then(|(_, n)| shown(&n.adapter_name.value, n.adapter_name.validity));
            let adapter = details
                .or_else(|| {
                    snap.inventory
                        .network
                        .iter()
                        .find(|n| n.device == Some(idx))
                        .and_then(|n| shown(&n.adapter_name.value, n.adapter_name.validity))
                })
                .unwrap_or(w.unknown_adapter.as_ref());
            (ifname.to_owned().into(), adapter.trim().to_owned().into())
        }
        DeviceKind::Fan => {
            let chip = match &entry.identity {
                DeviceIdentity::Fan { chip, .. } => &**chip,
                _ => &*entry.display_name,
            };
            (numbered(&w.fan, number), chip.to_owned().into())
        }
        DeviceKind::Battery => {
            let details = snap
                .device_details
                .batteries
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, b)| b);
            let word = details.map_or(w.battery.as_ref(), |b| w.battery_kind(&b.kind).as_ref());
            let mut text = String::new();
            if let Some(b) = details {
                for part in [&b.vendor, &b.model] {
                    if let Some(part) = shown(&part.value, part.validity) {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(part.trim());
                    }
                }
            }
            if text.is_empty() {
                text.push_str(&entry.display_name);
            }
            (word.to_owned().into(), text.into())
        }
        DeviceKind::AggregateDisk => {
            let k = present_count(snap, |id| matches!(id, DeviceIdentity::Disk { .. }));
            (w.aggregate_disk.clone(), numbered(&w.disks_count, k))
        }
        DeviceKind::AggregateGpu => {
            let k = present_count(snap, |id| matches!(id, DeviceIdentity::Gpu { .. }));
            (w.aggregate_gpu.clone(), numbered(&w.adapters_count, k))
        }
    }
}

/// Present catalog entries without a parent (so no partitions) whose identity matches.
fn present_count(snap: &Snapshot, f: impl Fn(&DeviceIdentity) -> bool) -> usize {
    snap.devices
        .entries
        .iter()
        .filter(|e| e.present && e.parent.is_none() && f(&e.identity))
        .count()
}

/// `template` with `n` as its one argument.
fn numbered(template: &Template, n: impl std::fmt::Display) -> SharedString {
    let mut s = String::new();
    let _ = template.write(&mut s, &[&n]);
    s.into()
}

/// Lays out a [`DevicePage`]. Built by the page in its own render:
/// `DevicePageFrame { page: self, m: &m, theme: &theme }.render(window, cx)`.
///
/// It is not a `RenderOnce`, because that trait needs `'static` and the frame borrows the page.
pub struct DevicePageFrame<'a> {
    pub page: &'a dyn DevicePage,
    pub m: &'a UiMetrics,
    pub theme: &'a TlTheme,
}

impl DevicePageFrame<'_> {
    pub fn render(self, window: &mut Window, cx: &mut App) -> AnyElement {
        let header = page_header(self.page, self.m, self.theme, window, cx);
        self.layout(header, window, cx)
    }

    /// As [`Self::render`], with the header drawn by `header`, the page's [`PageHeaderView`].
    /// The view is cached, so a frame that draws the page again for its moving charts reuses
    /// the header (tasklens-wlf.40).
    pub fn render_with_header(
        self,
        header: &Entity<PageHeaderView>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let h = px(PAGE_HEADER_H * self.m.ui_scale);
        let header = header
            .clone()
            .cached(gpui_kit::StyleRefinement::default().w_full().h(h))
            .into_any_element();
        let header = div()
            .flex_none()
            .w_full()
            .h(h)
            .child(header)
            .into_any_element();
        self.layout(header, window, cx)
    }

    fn layout(self, header: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
        let Self { page, m, theme } = self;
        let s = m.ui_scale;
        let key = page.key();
        let details_heading = page_words(cx).details.clone();

        if let Some(editor) = page.widget_editor() {
            // The header stays; the widget container under it scrolls (ADR-019 phase 3).
            return v_flex()
                .min_w(px(PAGE_MIN_W * s))
                .size_full()
                .child(
                    div()
                        .flex_none()
                        .px(px(PAGE_PAD * s))
                        .pt(px(PAGE_PAD * s))
                        .child(header),
                )
                .child(div().flex_none().h(m.sp[4]))
                .child(
                    div()
                        .tl_selector(|| PAGE_BODY_ID.into())
                        .flex_1()
                        .min_h_0()
                        .px(px(PAGE_PAD * s))
                        .child(
                            editor
                                .clone()
                                .cached(gpui_kit::StyleRefinement::default().size_full()),
                        ),
                )
                .id(ElementId::Name(SharedString::from(key.0.clone())))
                .into_any_element();
        }

        let graph_area = div()
            .id(GRAPH_AREA_ID)
            .tl_selector(|| GRAPH_AREA_ID.into())
            .w_full()
            .on_click(|ev, window, cx| {
                if ev.click_count() == 2 {
                    window.dispatch_action(Box::new(PerfShowMiniWindow), cx);
                }
            })
            .child(page.graphs(window, cx));

        // The page menu (UI-PRF-015) opens from the page panel of `PerformancePage`.
        v_flex()
            .min_w(px(PAGE_MIN_W * s))
            .p(px(PAGE_PAD * s))
            .child(header)
            .child(div().h(m.sp[4]))
            .child(graph_area)
            .child(div().h(px(DETAILS_GAP * s)))
            .child(details_part(page, details_heading, m, theme, cx))
            .children(
                page.blocks(window, cx)
                    .into_iter()
                    .map(|block| div().pt(m.sp[6]).child(block)),
            )
            .overflow_y_scrollbar()
            .id(ElementId::Name(SharedString::from(key.0.clone())))
            .size_full()
            .into_any_element()
    }
}

/// The header row of `page`: the device icon, the type word, the model, the main value, the page
/// toggles and the range switch (UI-PRF-010, UI-PRF-058).
pub fn page_header(
    page: &dyn DevicePage,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let s = m.ui_scale;
    let key = page.key();
    let (word, model) = page.header();
    let range_words = page_words(cx).range.clone();
    h_flex()
        .tl_selector(|| "perf-page-header".into())
        .flex_none()
        .w_full()
        .h(px(PAGE_HEADER_H * s))
        .items_center()
        .gap(m.sp[4])
        .children(key.kind().icon().map(|name| {
            div()
                .tl_selector(|| "perf-page-icon".into())
                .flex_none()
                .child(icon(name, m.icon.lg, key.kind().icon_color(theme)))
        }))
        .child(
            div()
                .tl_selector(|| "perf-page-type".into())
                .flex_none()
                .whitespace_nowrap()
                // The style first: it replaces the whole text refinement, colour included.
                .map_text(label_style(m, m.text.lg, FontWeight::MEDIUM))
                .text_color(theme.text_primary)
                .child(word),
        )
        .child(model_text(model, key, m, theme, window, cx))
        .children(header_value(key, window, cx))
        .child(
            h_flex()
                .tl_selector(|| "perf-page-toggles".into())
                .flex_none()
                .gap(m.sp[4])
                .children(
                    page.toggles(window, cx)
                        .into_iter()
                        .enumerate()
                        .map(|(i, toggle)| {
                            // Column direction stretches the button to the minimum width.
                            v_flex()
                                .tl_selector(move || format!("perf-page-toggle-{i}"))
                                .min_w(px(TOGGLE_MIN_W * s))
                                .child(toggle)
                        }),
                ),
        )
        .child(
            div()
                .tl_selector(|| RANGE_ID.into())
                .flex_none()
                .child(page_range_switch(
                    PAGE_SWITCH,
                    &range_words,
                    page.hub(),
                    window,
                    cx,
                )),
        )
        .into_any_element()
}

/// Draws a page's header for its [`PageHeaderView`]. `None` once the page is gone.
pub type HeaderDraw = Rc<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>;

/// A device page's header row as a view of its own, drawn cached by
/// [`DevicePageFrame::render_with_header`] (tasklens-wlf.40). The moving charts under the
/// header draw the page again on every frame; the header then keeps its last layout and paint.
/// It draws again when the page notifies, when the settings, strings, theme or scale change,
/// when the sidebar changes its cards, when a snapshot turns the device history on or off, and
/// when the header value changes ([`HeaderValue`]).
pub struct PageHeaderView {
    draw: HeaderDraw,
    /// `history.device_metrics` of the host at the last snapshot; the range switch disables
    /// 24 h and 7 d without it.
    history_kept: bool,
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl PageHeaderView {
    /// The header of `page`, whose snapshots come from `hub`. The page may be leased now (it
    /// makes its header in its render), but not while the header renders: the header is a child
    /// view, so it renders after the page's own render returned.
    pub fn new<P: DevicePage + Render>(
        page: &Entity<P>,
        hub: Option<Entity<SnapshotHub>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = SmallVec::new();
        subs.push(cx.observe(page, |_, _, cx| cx.notify()));
        subs.push(cx.observe_global::<TlSettings>(|_, cx| cx.notify()));
        subs.push(cx.observe_global::<UiMetrics>(|_, cx| cx.notify()));
        subs.push(cx.observe_global::<Strings>(|_, cx| cx.notify()));
        subs.push(cx.observe_global::<ThemeStore>(|_, cx| cx.notify()));
        if let Some(sidebar) = cx.try_global::<PerfSidebar>().and_then(|s| s.0.upgrade()) {
            subs.push(cx.observe(&sidebar, |_, _, cx| cx.notify()));
        }
        let kept = history_kept(hub.as_ref(), cx);
        if let Some(hub) = hub {
            subs.push(
                cx.subscribe(&hub, |this: &mut Self, hub, _: &SnapshotChanged, cx| {
                    let kept = history_kept(Some(&hub), cx);
                    if kept != this.history_kept {
                        this.history_kept = kept;
                        cx.notify();
                    }
                }),
            );
        }
        let page = page.downgrade();
        let draw: HeaderDraw = Rc::new(move |window: &mut Window, cx: &mut App| {
            let page = page.upgrade()?;
            let m = *metrics(cx);
            let theme = current(cx).clone();
            Some(page.update(cx, |page, cx| page_header(page, &m, &theme, window, cx)))
        });
        Self {
            draw,
            history_kept: kept,
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
        }
    }
}

impl Render for PageHeaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        div().size_full().children((self.draw)(window, cx))
    }
}

/// The debug selector of a widget page's body, under its header.
pub const PAGE_BODY_ID: &str = "perf-page-body";

/// The "Details" heading, `sp4` and the Details grid of `page`, with its link values, prefixes
/// and suffixes (UI-PRF-020, UI-PRF-021): the 1.3.0 frame draws it under the graph area, and
/// the Details widget of a widget page draws it in its `DetailsView`.
pub fn details_part(
    page: &dyn DevicePage,
    heading: SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
    cx: &App,
) -> AnyElement {
    let s = m.ui_scale;
    let heading = div()
        .tl_selector(|| "perf-page-details-heading".into())
        .flex()
        .flex_none()
        .items_center()
        .h(px(DETAILS_HEADING_H * s))
        .map_text(label_style(m, m.text.md, FontWeight::MEDIUM))
        .text_color(theme.text_primary)
        .child(heading);
    let mut grid = render_details(page.details(), m, cx);
    if let Some(focus) = page.details_link_focus() {
        grid = grid.link_focus(focus);
    }
    if let Some(prefix) = page.details_value_prefix() {
        grid = grid.value_prefix(move |n, w, cx| prefix(n, w, cx));
    }
    if let Some(suffix) = page.details_value_suffix() {
        grid = grid.value_suffix(move |n, w, cx| suffix(n, w, cx));
    }
    let grid = match page.details_link_click() {
        Some(click) => grid.on_link_click(move |n, b, w, cx| click(n, b, w, cx)),
        None => grid,
    };
    v_flex()
        .w_full()
        .child(heading)
        .child(div().flex_none().h(m.sp[4]))
        .child(grid)
        .into_any_element()
}

/// The Details draw of the widget page `page`: [`details_part`] from the page's model, for its
/// `DetailsView`. `None` once the page is gone.
pub fn details_draw<P: DevicePage + Render>(
    page: WeakEntity<P>,
) -> crate::performance::widget::details_widget::DetailsDraw {
    Rc::new(move |_window: &mut Window, cx: &mut App| {
        let page = page.upgrade()?;
        let heading = page_words(cx).details.clone();
        let m = *metrics(cx);
        let theme = current(cx).clone();
        Some(details_part(page.read(cx), heading, &m, &theme, cx))
    })
}

/// The sidebar whose cards give the page headers their values (UI-PRF-058). Set by
/// `PerformancePage::new`.
pub struct PerfSidebar(pub WeakEntity<DeviceSidebar>);

impl Global for PerfSidebar {}

/// The main value at the right of the page header: the value of the device's sidebar card, so
/// the two always agree. `None` while the device has no card.
fn header_value(key: &DeviceKey, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let sidebar = cx.try_global::<PerfSidebar>()?.0.upgrade()?;
    let chart = sidebar
        .read(cx)
        .cards()
        .iter()
        .find(|c| c.row.key == *key && !c.is_removing())?
        .chart
        .clone();
    let id = ElementId::NamedChild(
        std::sync::Arc::new(ElementId::Name(SharedString::from(key.0.clone()))),
        "perf-page-value".into(),
    );
    let view = window.use_keyed_state(id, cx, |_, cx| HeaderValue::new(chart.clone(), cx));
    view.update(cx, |v, cx| v.set_chart(chart, cx));
    Some(view.into_any_element())
}

/// The page header value, `t_lg` 600 `tnum` in the card value colours (UI-PRF-058). A view of
/// its own that draws again on each new sample of the card's chart, which is not in the page's
/// element tree.
struct HeaderValue {
    chart: Entity<ChartView>,
    /// The value of the last render. The chart notifies on every animation frame while it
    /// moves; the view draws again only when this changes (tasklens-wlf.40).
    shown: Option<CardValue>,
    _observe: Subscription,
}

impl HeaderValue {
    fn new(chart: Entity<ChartView>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&chart, |this, chart, cx| {
                let now = CardValue::of(chart.read(cx), NetFormat::of(cx));
                if now != this.shown {
                    this.shown = now;
                    cx.notify();
                }
            }),
            shown: CardValue::of(chart.read(cx), NetFormat::of(cx)),
            chart,
        }
    }

    /// Follows a card that got a new chart. The page draws this view in the same frame.
    fn set_chart(&mut self, chart: Entity<ChartView>, cx: &mut Context<Self>) {
        if self.chart != chart {
            *self = Self::new(chart, cx);
        }
    }
}

impl Render for HeaderValue {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let el = div()
            .tl_selector(|| "perf-page-value".into())
            .flex()
            .flex_none()
            .items_center();
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        VALUE_RENDERS.with(|n| n.set(n.get() + 1));
        self.shown = CardValue::of(self.chart.read(cx), NetFormat::of(cx));
        let Some(v) = self.shown.clone() else {
            return el;
        };
        if v.validity == Validity::Restricted {
            return el.child(icon(TlIcon::Lock, m.icon.md, theme.text_secondary));
        }
        el.map_text(title_value_style(&m))
            .whitespace_nowrap()
            .text_color(v.color(theme))
            .child(v.text)
    }
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
thread_local! {
    /// Renders of every [`HeaderValue`] on this thread.
    static VALUE_RENDERS: Cell<u32> = const { Cell::new(0) };
}

/// Renders of the page header values on this thread, for the frame cost tests
/// (tasklens-wlf.40).
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub fn header_value_renders() -> u32 {
    VALUE_RENDERS.with(Cell::get)
}

/// Sets the text style of an element in one call.
trait MapText: Sized {
    fn map_text(self, style: gpui_kit::TextStyleRefinement) -> Self;
}

impl<E: gpui_kit::Styled> MapText for E {
    fn map_text(mut self, style: gpui_kit::TextStyleRefinement) -> Self {
        *self.text_style() = style;
        self
    }
}

/// The model text: one line in `text_secondary` with an end ellipsis. A canvas behind the text
/// compares the full text width with the box in prepaint; when the text is cut, the next render
/// attaches a hover tooltip with the full text. A change of that state notifies the page view.
fn model_text(
    model: SharedString,
    key: &DeviceKey,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = ElementId::NamedChild(
        std::sync::Arc::new(ElementId::Name(SharedString::from(key.0.clone()))),
        "perf-page-model".into(),
    );
    let cut: Rc<Cell<bool>> = window
        .use_keyed_state((id.clone(), "cut"), cx, |_, _| Rc::new(Cell::new(false)))
        .read(cx)
        .clone();

    let measure = {
        let cut = Rc::clone(&cut);
        let text = model.clone();
        canvas(
            move |bounds, window, cx| {
                let style = window.text_style();
                let font_size = style.font_size.to_pixels(window.rem_size());
                let run = style.to_run(text.len());
                let full = window
                    .text_system()
                    .shape_line(text.clone(), font_size, &[run], None)
                    .width;
                let now = full > bounds.size.width;
                if cut.replace(now) != now {
                    cx.notify(window.current_view());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    };

    let inner = div()
        .tl_selector(|| "perf-page-model".into())
        .relative()
        .min_w_0()
        .map_text(label_style(m, m.text.lg, FontWeight::NORMAL))
        .text_color(theme.text_secondary)
        .truncate()
        .child(model.clone())
        .child(measure);
    // The tooltip hangs off a plain outer box: inside the truncating box it would inherit
    // `nowrap` and the ellipsis.
    let outer = div().flex().flex_1().min_w_0().child(inner);
    if !cut.get() {
        return outer.into_any_element();
    }
    let state = HoverTooltip::state(id, window, cx);
    HoverTooltip::attach(
        &state,
        outer,
        DELAY_CONTROL,
        TlTooltip::text(model).build(),
        window,
        cx,
    )
    .into_any_element()
}

/// The page view for `row`. Each device kind adds its arm as a chain edit (R-262); a kind
/// without a page yet gets [`PlaceholderPage`]. `hub` and `demand` are `host`'s; the pages
/// that send commands of their own keep `host`.
pub fn build_page(
    host: &HostContext,
    key: &DeviceKey,
    row: &DeviceRow,
    hub: &Entity<SnapshotHub>,
    demand: &Entity<DemandAggregator>,
    cx: &mut App,
) -> AnyView {
    match row.kind {
        DeviceKind::Cpu => cx
            .new(|cx| CpuPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Memory => cx
            .new(|cx| MemoryPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Disk => cx
            .new(|cx| {
                DiskPage::new(
                    host,
                    key.clone(),
                    row.clone(),
                    hub.clone(),
                    demand.clone(),
                    cx,
                )
            })
            .into(),
        DeviceKind::Net => cx
            .new(|cx| {
                NetPage::new(
                    host,
                    key.clone(),
                    row.clone(),
                    hub.clone(),
                    demand.clone(),
                    cx,
                )
            })
            .into(),
        DeviceKind::Gpu => cx
            .new(|cx| GpuPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Fan => cx
            .new(|cx| FanPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Battery => cx
            .new(|cx| {
                BatteryPage::new(
                    host,
                    key.clone(),
                    row.clone(),
                    hub.clone(),
                    demand.clone(),
                    cx,
                )
            })
            .into(),
        // An aggregate page needs two or more devices of its type (PERF-009).
        DeviceKind::AggregateDisk
            if aggregate_has_members(hub, cx, |id| matches!(id, DeviceIdentity::Disk { .. })) =>
        {
            cx.new(|cx| {
                AggregateDiskPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx)
            })
            .into()
        }
        DeviceKind::AggregateGpu
            if aggregate_has_members(hub, cx, |id| matches!(id, DeviceIdentity::Gpu { .. })) =>
        {
            cx.new(|cx| {
                AggregateGpuPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx)
            })
            .into()
        }
        _ => cx
            .new(|cx| PlaceholderPage::new(key.clone(), hub.clone(), cx))
            .into(),
    }
}

/// Two or more present devices of a type, the rule for an aggregate page.
fn aggregate_has_members(
    hub: &Entity<SnapshotHub>,
    cx: &App,
    f: impl Fn(&DeviceIdentity) -> bool,
) -> bool {
    present_count(hub.read(cx).current(), f) >= 2
}

/// The frame with an empty header, graph area and Details grid, for a kind whose page is not
/// built yet.
pub struct PlaceholderPage {
    key: DeviceKey,
    hub: Entity<SnapshotHub>,
    details: KeyValueModel,
    focus: FocusHandle,
}

impl PlaceholderPage {
    pub fn new(key: DeviceKey, hub: Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        Self {
            key,
            hub,
            details: KeyValueModel::default(),
            focus: cx.focus_handle(),
        }
    }
}

impl DevicePage for PlaceholderPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn header(&self) -> (SharedString, SharedString) {
        (SharedString::default(), SharedString::default())
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div().into_any_element()
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

impl Render for PlaceholderPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div().size_full().track_focus(&self.focus).child(
            DevicePageFrame {
                page: self,
                m: &m,
                theme: &theme,
            }
            .render(window, cx),
        )
    }
}
