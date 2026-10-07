//! Information tab (UI-SYS-080 to UI-SYS-096).
//!
//! `InformationPage` holds the ten cached [`InfoSection`]s in one vertical scroll region. When the
//! content is 880 px wide or more (× ui_scale) they sit in two equal columns, the hardware
//! sections on the right; narrower, they stack in [`InfoSectionId::ALL`] order (UI-SYS-080).
//!
//! While the tab is shown the page sends its demand part (inventory, CPU totals, memory, GPUs,
//! disks, with `any_surface_visible`) and withdraws it on leave (ADR-008). The first activation of
//! the session also sends `RefreshNow(Inventory)`. An INVENTORY change rebuilds every section's
//! rows; a SYSTEM or DEVICES change rebuilds only System, Memory, Disk drives and Graphics, the
//! sections with live rows. A section whose rows did not change does not re-render (UI-SYS-085).
//!
//! The shell filter's text for this tab narrows the rows by the [`filter`] rules; a section left
//! with no rows is hidden, and with every section hidden the page shows "No information matches
//! the filter." with "Clear filter" (UI-SYS-084). The page keeps the full rows for copying.
//!
//! `CopyInformationSection` and `CopyInformationAll` copy the full rows in the [`copy`] format and
//! show "Copied to clipboard". Ctrl+C copies a text selection when there is one, else everything.
//! `App::write_to_clipboard` reports no failure, so there is no failure toast (R-199).
//!
//! Until the inventory is first Valid or Stale every section shows its band and an 8-line
//! skeleton; this never comes back in the session. An inventory that Failed before any data shows
//! "Could not load system information" with a "Retry" button instead of the sections. F5 sends
//! `RefreshNow(Inventory)` and shows "Information refreshed." when the next inventory version
//! arrives, or 2 s later at the latest.

pub mod actions;
pub mod copy;
pub mod filter;
pub mod section;
pub mod sections;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::TextSelection;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity,
    EventEmitter, FocusHandle, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Subscription, Task, Window, canvas, div, point, px,
};
use smallvec::SmallVec;
use tl_model::command::{Command, Tab};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::health::SourceId;
use tl_model::snapshot::ComponentMask;
use tl_model::validity::Validity;
use tracing::debug;

use self::actions::{
    CopyInformationAll, CopyInformationSection, INFORMATION_CONTEXT, InformationScrollBottom,
    InformationScrollLineDown, InformationScrollLineUp, InformationScrollPageDown,
    InformationScrollPageUp, InformationScrollTop,
};
use self::filter::filter_section;
use self::section::{InfoRow, InfoSection, InfoSectionId};
use self::sections::{InfoInputs, InfoWords};
use crate::actions::{ClearFilter, RefreshCurrentTab};
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::empty::TlEmptyState;
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::t_base;
use crate::theme::{current, metrics};
use crate::window::main::{FilterChanged, GlobalShellEvents, ShellPage};
use crate::window::status::StatusChanged;

/// Content width from which the sections sit in two columns, × ui_scale.
const TWO_COLUMNS_FROM: f32 = 880.;
/// Narrowest column in the two-column layout, × ui_scale.
const COLUMN_MIN: f32 = 360.;
/// The latest the refresh toast waits for a new inventory version.
pub const REFRESH_TOAST_AFTER: Duration = Duration::from_secs(2);
const LEFT: [InfoSectionId; 5] = [
    InfoSectionId::OperatingSystem,
    InfoSectionId::Processor,
    InfoSectionId::System,
    InfoSectionId::BiosFirmware,
    InfoSectionId::Motherboard,
];
const RIGHT: [InfoSectionId; 5] = [
    InfoSectionId::Memory,
    InfoSectionId::MemoryModules,
    InfoSectionId::DiskDrives,
    InfoSectionId::Graphics,
    InfoSectionId::NetworkAdapters,
];
/// The sections with live rows (UI-SYS-085).
const LIVE: [InfoSectionId; 4] = [
    InfoSectionId::System,
    InfoSectionId::Memory,
    InfoSectionId::DiskDrives,
    InfoSectionId::Graphics,
];
const KEY_REFRESHED: &str = "information-refreshed";
const KEY_ERROR: &str = "information-error-load";
const KEY_RETRY: &str = "information-error-retry";
const KEY_FILTER_EMPTY: &str = "information-filter-empty";
const KEY_COPIED: &str = "information-toast-copied";

/// The page's own texts, translated once per strings version.
struct PageWords {
    version: Option<u64>,
    refreshed: SharedString,
    error: SharedString,
    retry: SharedString,
    filter_empty: SharedString,
    copied: SharedString,
}

impl PageWords {
    fn new(cx: &App) -> Self {
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        let t = |key| match version {
            Some(_) => tr(cx, key),
            None => tr_en(key, &[]),
        };
        Self {
            version,
            refreshed: t(KEY_REFRESHED),
            error: t(KEY_ERROR),
            retry: t(KEY_RETRY),
            filter_empty: t(KEY_FILTER_EMPTY),
            copied: t(KEY_COPIED),
        }
    }
}

pub struct InformationPage {
    sections: [Entity<InfoSection>; 10],
    /// Each section's drawn height. The sections are cached views, which take their size from
    /// the page.
    heights: [Rc<Cell<Pixels>>; 10],
    /// Every built row, before the filter. Copy reads these.
    full_rows: [Arc<[InfoRow]>; 10],
    /// The filter text, lowercased.
    filter: Arc<str>,
    /// The sections the filter leaves visible.
    shown: [bool; 10],
    scroll: ScrollHandle,
    focus: FocusHandle,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    window: AnyWindowHandle,
    active: bool,
    first_visit_done: bool,
    /// Set when the inventory is first Valid or Stale. Never resets in the session (APP-059).
    first_data_seen: bool,
    /// The inventory Failed before any data: the error state shows.
    failed: bool,
    /// The inventory version when F5 was pressed, and the 2 s fallback timer.
    refresh_pending: Option<(u64, Task<()>)>,
    /// The content width at the last paint, measured each frame.
    width: Rc<Cell<Pixels>>,
    words: InfoWords,
    page_words: PageWords,
    _subs: SmallVec<[Subscription; 4]>,
}

impl InformationPage {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        demand: &Entity<DemandAggregator>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = cx.entity_id();
        let sections = InfoSectionId::ALL.map(|id| {
            cx.new(|cx| {
                let mut s = InfoSection::new(id, cx);
                s.set_loading(true, cx);
                s
            })
        });
        let heights = InfoSectionId::ALL
            .map(|id| sections[id as usize].update(cx, |s, _| s.measured_height(owner)));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(hub, |this, _, ev: &SnapshotChanged, cx| {
            this.on_snapshot(ev.mask, cx);
        }));
        subs.push(cx.observe_global::<Strings>(|this, cx| this.on_strings(cx)));
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Information {
                    this.apply_filter(Arc::from(ev.text.as_ref()), cx);
                }
            }));
        }
        let mut words = InfoWords::default();
        words.refresh(cx);
        let mut page = Self {
            sections,
            heights,
            full_rows: std::array::from_fn(|_| Arc::from([])),
            filter: Arc::from(""),
            shown: [true; 10],
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            hub: hub.clone(),
            demand: demand.clone(),
            window: window.window_handle(),
            active: false,
            first_visit_done: false,
            first_data_seen: false,
            failed: false,
            refresh_pending: None,
            width: Rc::new(Cell::new(Pixels::ZERO)),
            words,
            page_words: PageWords::new(cx),
            _subs: subs,
        };
        page.rebuild(&InfoSectionId::ALL, cx);
        page.note_inventory(cx);
        page
    }

    /// Turns the page's demand part on or off. The first activation of the session also asks the
    /// Inventory worker for a scan.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        let source = cx.entity_id();
        if !active {
            self.demand.update(cx, |d, cx| d.remove(source, cx));
            return;
        }
        let window = Some(self.window);
        self.demand
            .update(cx, |d, cx| d.set(source, window, demand_part(), cx));
        if !self.first_visit_done {
            self.first_visit_done = true;
            send_refresh(cx);
        }
    }

    /// The status chip is empty on Information (UI-SHL-050).
    pub fn status_text(&self, _out: &mut String) {}

    pub fn section(&self, id: InfoSectionId) -> &Entity<InfoSection> {
        &self.sections[id as usize]
    }

    /// The rows of section `id` before the filter.
    pub fn full_rows(&self, id: InfoSectionId) -> &Arc<[InfoRow]> {
        &self.full_rows[id as usize]
    }

    /// Every section's full rows as copy text, in [`InfoSectionId::ALL`] order.
    pub fn copy_all_text(&self) -> String {
        let sections = InfoSectionId::ALL.map(|id| (id, Arc::clone(&self.full_rows[id as usize])));
        let mut out = String::with_capacity(copy::COPY_CAPACITY);
        copy::all_text(&sections, &mut out);
        out
    }

    /// Section `id`'s full rows as copy text.
    pub fn copy_section_text(&self, id: InfoSectionId) -> String {
        let mut out = String::with_capacity(copy::COPY_CAPACITY);
        copy::section_text(id, &self.full_rows[id as usize], &mut out);
        out
    }

    fn on_copy_section(
        &mut self,
        action: &CopyInformationSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.copy_section_text(action.section);
        self.write_clipboard(text, window, cx);
    }

    /// Ctrl+C and the tab copy icon: a text selection when there is one, else every section.
    fn on_copy_all(&mut self, _: &CopyInformationAll, window: &mut Window, cx: &mut Context<Self>) {
        let selected = TextSelection::selected_text(window, cx);
        let selected = selected.trim();
        let text = if selected.is_empty() {
            self.copy_all_text()
        } else {
            selected.to_owned()
        };
        self.write_clipboard(text, window, cx);
    }

    fn write_clipboard(&self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            let copied = self.page_words.copied.clone();
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, copied, None, None, cx);
            });
        }
    }

    /// Whether the filter leaves section `id` visible.
    pub fn is_shown(&self, id: InfoSectionId) -> bool {
        self.shown[id as usize]
    }

    /// Filters every section by `text` (UI-SYS-084). The text is lowercased once; sections whose
    /// visible rows do not change do not re-render.
    pub fn apply_filter(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        let lower: Arc<str> = Arc::from(text.to_lowercase());
        if lower == self.filter {
            return;
        }
        self.filter = lower;
        for id in InfoSectionId::ALL {
            self.show_rows(id, cx);
        }
    }

    /// Sets section `id`'s visible rows from its full rows and the filter.
    fn show_rows(&mut self, id: InfoSectionId, cx: &mut Context<Self>) {
        let n = id as usize;
        let rows = filter_section(&id.title(), &self.full_rows[n], &self.filter);
        let shown = self.filter.is_empty() || !rows.is_empty();
        self.sections[n].update(cx, |s, cx| s.set_rows(rows, cx));
        if self.shown[n] != shown {
            self.shown[n] = shown;
            cx.notify();
        }
    }

    pub fn first_data_seen(&self) -> bool {
        self.first_data_seen
    }

    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// F5: asks for a new scan and shows "Information refreshed." on the next inventory version,
    /// or after 2 s.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        send_refresh(cx);
        let version = self.hub.read(cx).current().inventory.header.version;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_TOAST_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh_pending.take().is_some() {
                    this.show_refreshed(cx);
                }
            });
        });
        self.refresh_pending = Some((version, timer));
    }

    fn on_snapshot(&mut self, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.contains(ComponentMask::INVENTORY) {
            self.rebuild(&InfoSectionId::ALL, cx);
            self.note_inventory(cx);
            let version = self.hub.read(cx).current().inventory.header.version;
            if self
                .refresh_pending
                .as_ref()
                .is_some_and(|(at, _)| version > *at)
            {
                self.refresh_pending = None;
                self.show_refreshed(cx);
            }
        } else if mask.contains(ComponentMask::SYSTEM) || mask.contains(ComponentMask::DEVICES) {
            self.rebuild(&LIVE, cx);
        }
    }

    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        if self.page_words.version != version {
            self.page_words = PageWords::new(cx);
            cx.notify();
        }
        if self.words.refresh(cx) {
            self.rebuild(&InfoSectionId::ALL, cx);
        }
    }

    /// Builds the rows of `ids` from the current snapshot and shows them through the filter.
    /// Sections whose visible rows are unchanged do not notify.
    fn rebuild(&mut self, ids: &[InfoSectionId], cx: &mut Context<Self>) {
        let snap = Arc::clone(self.hub.read(cx).current());
        let inputs = InfoInputs {
            inv: &snap.inventory,
            sys: &snap.system,
            devices: &snap.devices,
            words: &self.words,
        };
        for &id in ids {
            let mut rows: Vec<InfoRow> = Vec::new();
            builder(id)(&inputs, &mut rows);
            self.full_rows[id as usize] = Arc::from(rows);
        }
        let logo = ids
            .contains(&InfoSectionId::OperatingSystem)
            .then(|| sections::os_logo(&inputs));
        for &id in ids {
            self.show_rows(id, cx);
        }
        if let Some(logo) = logo {
            self.sections[InfoSectionId::OperatingSystem as usize]
                .update(cx, |s, cx| s.set_logo(logo, cx));
        }
    }

    /// Follows the inventory validity: the first Valid or Stale ends the skeleton for the session;
    /// Failed before that shows the error state.
    fn note_inventory(&mut self, cx: &mut Context<Self>) {
        let validity = self.hub.read(cx).current().inventory.header.validity;
        if !self.first_data_seen && matches!(validity, Validity::Valid | Validity::Stale) {
            self.first_data_seen = true;
            for s in &self.sections {
                s.update(cx, |s, cx| {
                    s.set_loading(false, cx);
                    s.set_copy_enabled(true, cx);
                });
            }
        }
        let failed = !self.first_data_seen && validity == Validity::Failed;
        if self.failed != failed {
            self.failed = failed;
            cx.notify();
        }
    }

    fn show_refreshed(&mut self, cx: &mut Context<Self>) {
        let text = self.page_words.refreshed.clone();
        let key = ToastKey(cx.entity_id().as_u64());
        let _ = self.window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(key), None, cx)
                });
            }
        });
    }

    fn scroll_by(&mut self, dy: Pixels, cx: &mut Context<Self>) {
        let max = self.scroll.max_offset().y.max(Pixels::ZERO);
        let y = (self.scroll.offset().y - dy).clamp(-max, Pixels::ZERO);
        self.scroll.set_offset(point(self.scroll.offset().x, y));
        cx.notify();
    }

    fn on_line_up(&mut self, _: &InformationScrollLineUp, _: &mut Window, cx: &mut Context<Self>) {
        let row = metrics(cx).row_height;
        self.scroll_by(-row, cx);
    }

    fn on_line_down(
        &mut self,
        _: &InformationScrollLineDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = metrics(cx).row_height;
        self.scroll_by(row, cx);
    }

    fn page_step(&self, cx: &App) -> Pixels {
        let row = metrics(cx).row_height;
        (self.scroll.bounds().size.height - row).max(row)
    }

    fn on_page_up(&mut self, _: &InformationScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        let step = self.page_step(cx);
        self.scroll_by(-step, cx);
    }

    fn on_page_down(
        &mut self,
        _: &InformationScrollPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = self.page_step(cx);
        self.scroll_by(step, cx);
    }

    fn on_top(&mut self, _: &InformationScrollTop, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll
            .set_offset(point(self.scroll.offset().x, Pixels::ZERO));
        cx.notify();
    }

    fn on_bottom(&mut self, _: &InformationScrollBottom, _: &mut Window, cx: &mut Context<Self>) {
        let max = self.scroll.max_offset().y.max(Pixels::ZERO);
        self.scroll.set_offset(point(self.scroll.offset().x, -max));
        cx.notify();
    }

    /// Section `id` as a cached view, as tall as it drew last time.
    fn section_view(&self, id: InfoSectionId) -> AnyElement {
        let height = self.heights[id as usize].get();
        self.sections[id as usize]
            .clone()
            .cached(StyleRefinement::default().w_full().h(height))
            .into_any_element()
    }

    fn column(&self, ids: &[InfoSectionId], cx: &App) -> AnyElement {
        let m = metrics(cx);
        v_flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(COLUMN_MIN * m.ui_scale))
            .gap(m.sp[8])
            .children(
                ids.iter()
                    .filter(|id| self.shown[**id as usize])
                    .map(|id| self.section_view(*id)),
            )
            .into_any_element()
    }

    fn error_state(&self, cx: &App) -> AnyElement {
        let m = metrics(cx);
        let theme = current(cx);
        let mut text = div().debug_selector(|| "InformationPage-error-text".into());
        *text.text_style() = t_base(m);
        v_flex()
            .debug_selector(|| "InformationPage-error".into())
            .size_full()
            .items_center()
            .justify_center()
            .gap(m.sp[4])
            .p(m.sp[8])
            .text_color(theme.text_secondary)
            .child(
                Icon::new(IconName::CircleAlert)
                    .size(m.icon.lg)
                    .text_color(theme.status.warning),
            )
            .child(text.child(self.page_words.error.clone()))
            .child(
                TlButton::new("information-retry", TlButtonKind::Secondary)
                    .size(TlButtonSize::Sm)
                    .label(self.page_words.retry.clone())
                    .action(Box::new(RefreshCurrentTab)),
            )
            .into_any_element()
    }
}

/// The page's demand part while it is shown. The CPU totals carry the System section's process
/// and handle counts.
fn demand_part() -> Demand {
    Demand {
        any_surface_visible: true,
        cpu: CpuDemand::Total,
        inventory: true,
        memory: true,
        gpus: true,
        disks: true,
        ..Demand::default()
    }
}

fn send_refresh(cx: &App) {
    let Some(client) = cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0)) else {
        return;
    };
    if let Err(e) = client.send(Command::RefreshNow(SourceId::Inventory)) {
        debug!(target: "tasklens::ui", error = ?e, "inventory refresh not sent");
    }
}

fn builder(id: InfoSectionId) -> fn(&InfoInputs, &mut Vec<InfoRow>) {
    match id {
        InfoSectionId::OperatingSystem => sections::build_os,
        InfoSectionId::Processor => sections::build_processor,
        InfoSectionId::System => sections::build_system,
        InfoSectionId::Memory => sections::build_memory,
        InfoSectionId::MemoryModules => sections::build_modules,
        InfoSectionId::DiskDrives => sections::build_disks,
        InfoSectionId::BiosFirmware => sections::build_bios,
        InfoSectionId::Motherboard => sections::build_motherboard,
        InfoSectionId::Graphics => sections::build_graphics,
        InfoSectionId::NetworkAdapters => sections::build_network,
    }
}

impl EventEmitter<StatusChanged> for InformationPage {}

impl Render for InformationPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let content = if self.failed {
            self.error_state(cx)
        } else if !self.shown.contains(&true) {
            div()
                .debug_selector(|| "InformationPage-filter-empty".into())
                .w_full()
                .child(
                    TlEmptyState::new(self.page_words.filter_empty.clone())
                        .clear_filter(Box::new(ClearFilter)),
                )
                .into_any_element()
        } else if self.width.get() >= px(TWO_COLUMNS_FROM * m.ui_scale) {
            h_flex()
                .debug_selector(|| "InformationPage-columns".into())
                .w_full()
                .items_start()
                .gap(m.sp[7])
                .child(self.column(&LEFT, cx))
                .child(self.column(&RIGHT, cx))
                .into_any_element()
        } else {
            v_flex()
                .debug_selector(|| "InformationPage-column".into())
                .w_full()
                .gap(m.sp[8])
                .children(
                    InfoSectionId::ALL
                        .iter()
                        .filter(|id| self.shown[**id as usize])
                        .map(|id| self.section_view(*id)),
                )
                .into_any_element()
        };

        // The layout depends on the content width: measure it each paint, redraw when it moved.
        let measured = Rc::clone(&self.width);
        let pad = m.sp[7] * 2.0;
        let id = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                let w = (bounds.size.width - pad).max(px(0.));
                if measured.get() != w {
                    measured.set(w);
                    cx.notify(id);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        div()
            .debug_selector(|| "InformationPage".into())
            .relative()
            .size_full()
            .child(measure)
            .child(
                div()
                    .id("information-scroll")
                    .key_context(INFORMATION_CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(cx.listener(Self::on_line_up))
                    .on_action(cx.listener(Self::on_line_down))
                    .on_action(cx.listener(Self::on_page_up))
                    .on_action(cx.listener(Self::on_page_down))
                    .on_action(cx.listener(Self::on_top))
                    .on_action(cx.listener(Self::on_bottom))
                    .on_action(cx.listener(Self::on_copy_section))
                    .on_action(cx.listener(Self::on_copy_all))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .w_full()
                            .pt(m.sp[6])
                            .px(m.sp[7])
                            .pb(m.sp[7])
                            .child(content),
                    ),
            )
            .vertical_scrollbar(&self.scroll)
    }
}

impl ShellPage for InformationPage {
    fn status_text(&self, out: &mut String) {
        InformationPage::status_text(self, out);
    }

    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        InformationPage::set_active(self, active, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        InformationPage::refresh(self, cx);
    }
}
