//! The device page frame every Performance page renders through: the header row with the type
//! word, the model and the page toggles, the graph area, the "Details" heading and grid, and the
//! page blocks, inside a scrolling column (docs/ui/performance.md UI-PRF-010, UI-PRF-020;
//! PERF-008, PERF-012, PERF-014, R-058).
//!
//! The header words and the "Details" heading are `en-US.ftl` keys, translated once per strings
//! version into the [`PageWords`] global that every page shares.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, ElementId, Entity, FocusHandle, FontWeight,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
};
use smallvec::SmallVec;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::actions::PerfShowMiniWindow;
use super::cpu::CpuPage;
use super::details::{KeyValueModel, Template, render_details};
use super::disk::DiskPage;
use super::memory::MemoryPage;
use super::net::NetPage;
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Page padding on every side, × ui_scale. `page_pad`.
pub const PAGE_PAD: f32 = 16.;
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
/// Height of the CPU Overall chart, × ui_scale.
pub const GRAPH_H_OVERALL: f32 = 300.;
/// Height of a secondary chart under the main one, × ui_scale.
pub const SECONDARY_GRAPH_H: f32 = 120.;

/// Minimum width of a header toggle, × ui_scale (measured).
const TOGGLE_MIN_W: f32 = 122.;
/// Height of the "Details" heading row, × ui_scale.
const DETAILS_HEADING_H: f32 = 24.;

/// Element id of the graph area.
pub const GRAPH_AREA_ID: &str = "perf-graph-area";

/// The header words and the "Details" heading, translated once per strings version. Shared by
/// every page as a global; read it with [`page_words`].
#[derive(Clone, Debug)]
pub struct PageWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    pub processor: SharedString,
    pub memory: SharedString,
    pub gpu: SharedString,
    /// "Disk {$n}".
    pub disk: Template,
    /// "Fan {$n}".
    pub fan: Template,
    pub battery: SharedString,
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
}

impl Global for PageWords {}

impl Default for PageWords {
    /// The en-US words.
    fn default() -> Self {
        Self::build(None, |key| tr_en(key, &[]), Template::en)
    }
}

impl PageWords {
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
            processor: t("perf-header-processor"),
            memory: t("perf-header-memory"),
            gpu: t("perf-header-gpu"),
            disk: template("perf-header-disk", &["n"]),
            fan: template("perf-header-fan", &["n"]),
            battery: t("perf-header-battery"),
            aggregate_disk: t("perf-card-aggregate-storage"),
            aggregate_gpu: t("perf-card-aggregate-graphics"),
            disks_count: template("perf-header-disks-count", &["k"]),
            adapters_count: template("perf-header-adapters-count", &["k"]),
            unknown_processor: t("perf-unknown-processor"),
            unknown_gpu: t("perf-unknown-gpu"),
            unknown_adapter: t("perf-unknown-adapter"),
            details: t("perf-details-heading"),
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

/// One device page. The page entity owns its charts, Details model and blocks; the frame lays
/// them out. Every method runs inside the page's own render.
pub trait DevicePage: 'static {
    fn key(&self) -> &DeviceKey;
    /// (type word, model text), from [`header_text`].
    fn header(&self) -> (SharedString, SharedString);
    /// Header buttons, right-aligned. Each is a `TlButton` with a tooltip.
    fn toggles(&self, w: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 2]>;
    fn graphs(&self, w: &mut Window, cx: &mut App) -> AnyElement;
    fn details(&self) -> &KeyValueModel;
    /// Tables, module blocks and links under the Details grid.
    fn blocks(&self, w: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 3]>;
    /// Page-specific items for UI-PRF-015 slot 5.
    fn menu_items(&self, menu: TlMenu, w: &mut Window, cx: &mut App) -> TlMenu;
    /// Appends the page's copy text (E24-016, E25).
    fn copy_text(&self, out: &mut String);
    /// Header toggles, then block controls (UI-SHL-121).
    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]>;
    /// Called with true when the page becomes the shown page and false when it stops being
    /// shown (R-229; E25-010 wires the calls).
    fn set_active(&mut self, active: bool, cx: &mut App) {
        let _ = (active, cx);
    }
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
            let word = details
                .and_then(|b| shown(&b.kind, Validity::Valid))
                .unwrap_or(w.battery.as_ref());
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
        let Self { page, m, theme } = self;
        let s = m.ui_scale;
        let key = page.key();
        let (word, model) = page.header();
        let details_heading = page_words(cx).details.clone();

        let header = h_flex()
            .debug_selector(|| "perf-page-header".into())
            .flex_none()
            .w_full()
            .h(px(PAGE_HEADER_H * s))
            .gap(m.sp[4])
            .child(
                div()
                    .debug_selector(|| "perf-page-type".into())
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(theme.text_primary)
                    .map_text(label_style(m, m.text.lg, FontWeight::MEDIUM))
                    .child(word),
            )
            .child(model_text(model, key, m, theme, window, cx))
            .child(
                h_flex()
                    .debug_selector(|| "perf-page-toggles".into())
                    .flex_none()
                    .gap(m.sp[4])
                    .children(page.toggles(window, cx).into_iter().enumerate().map(
                        |(i, toggle)| {
                            // Column direction stretches the button to the minimum width.
                            v_flex()
                                .debug_selector(move || format!("perf-page-toggle-{i}"))
                                .min_w(px(TOGGLE_MIN_W * s))
                                .child(toggle)
                        },
                    )),
            );

        let graph_area = div()
            .id(GRAPH_AREA_ID)
            .debug_selector(|| GRAPH_AREA_ID.into())
            .w_full()
            .on_click(|ev, window, cx| {
                if ev.click_count() == 2 {
                    window.dispatch_action(Box::new(PerfShowMiniWindow), cx);
                }
            })
            .child(page.graphs(window, cx));

        let heading = div()
            .debug_selector(|| "perf-page-details-heading".into())
            .flex()
            .items_center()
            .h(px(DETAILS_HEADING_H * s))
            .text_color(theme.text_primary)
            .map_text(label_style(m, m.text.md, FontWeight::MEDIUM))
            .child(details_heading);

        // E24-012: `.context_menu` with the page menu (UI-PRF-015) goes on this column.
        v_flex()
            .min_w(px(PAGE_MIN_W * s))
            .p(px(PAGE_PAD * s))
            .child(header)
            .child(div().h(m.sp[4]))
            .child(graph_area)
            .child(div().h(px(DETAILS_GAP * s)))
            .child(heading)
            .child(div().h(m.sp[4]))
            .child(render_details(page.details(), m, cx))
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
        .debug_selector(|| "perf-page-model".into())
        .relative()
        .min_w_0()
        .text_color(theme.text_secondary)
        .map_text(label_style(m, m.text.lg, FontWeight::NORMAL))
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
/// without a page yet gets [`PlaceholderPage`].
pub fn build_page(
    key: &DeviceKey,
    row: &DeviceRow,
    hub: &Entity<SnapshotHub>,
    demand: &Entity<DemandAggregator>,
    cx: &mut App,
) -> AnyView {
    // E25 adds one arm per kind.
    match row.kind {
        DeviceKind::Cpu => cx
            .new(|cx| CpuPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Memory => cx
            .new(|cx| MemoryPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Disk => cx
            .new(|cx| DiskPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        DeviceKind::Net => cx
            .new(|cx| NetPage::new(key.clone(), row.clone(), hub.clone(), demand.clone(), cx))
            .into(),
        _ => cx.new(|cx| PlaceholderPage::new(key.clone(), cx)).into(),
    }
}

/// The frame with an empty header, graph area and Details grid, for a kind whose page is not
/// built yet.
pub struct PlaceholderPage {
    key: DeviceKey,
    details: KeyValueModel,
    focus: FocusHandle,
}

impl PlaceholderPage {
    pub fn new(key: DeviceKey, cx: &mut Context<Self>) -> Self {
        Self {
            key,
            details: KeyValueModel::default(),
            focus: cx.focus_handle(),
        }
    }
}

impl DevicePage for PlaceholderPage {
    fn key(&self) -> &DeviceKey {
        &self.key
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
