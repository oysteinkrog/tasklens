//! App Properties and its General tab (UI-PWN-019, PROC-085).
//!
//! The window keeps one app detail demand, switches it with the active tab, and freezes the last
//! values when its group leaves the process table. The General tab formats its seven live usage
//! strings only when their source values change.

use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem,
    Context, Entity, EntityId, FocusHandle, Focusable as _, FontWeight, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, StyledImage as _, Subscription,
    Window, WindowHandle, canvas, div, img, point, prelude::FluentBuilder as _, px,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::command::Tab;
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{AppGeneralDetail, AppKind, DetailTab, DetailTarget, TabPayload};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{GroupRow, ProcFlags, ProcessRecord, is_row_member};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{ComponentMask, ProcessSnapshot, Snapshot};
use tl_model::table::Total;
use tl_model::units::{self, NetUnit};
use tl_model::validity::{ProcessField, Validity};
use tracing::warn;

use super::actions::{
    KV_GRID_CONTEXT, PROPS_WINDOW_CONTEXT, PropsActivateField, PropsCloseWindow,
    PropsCopyAllFields, PropsCopyValue, PropsEscape, PropsFieldDown, PropsFieldEnd, PropsFieldHome,
    PropsFieldLeft, PropsFieldMenu, PropsFieldRight, PropsFieldUp, PropsFocusFilter, PropsNextTab,
    PropsPrevTab, PropsRefreshNow, PropsSelectTab,
};
use super::frame::{EscStep, PwnWindow, app_props_title, esc_step, pwn_window_spec};
use super::props::filter::{
    FilterChanged, NO_MATCH, PropsFilter, filter_bar, lowercase, matches, on_clear,
};
use super::props::general::{escape_controls, go_to_service};
use crate::action_tracker::ToastTexts;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord, kv_words,
    row_copy_text, section_copy_text, shown_text_in, value_copy_form,
};
use crate::components::menu::{
    TlMenu, close_context_menu, context_menu_open, open_context_menu_at,
};
use crate::components::tabs::{TlTabSpec, TlTabs};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::TlSecondaryWindow;
use crate::host::HostContext;
use crate::hub::SnapshotChanged;
use crate::i18n::{Strings, tr};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::settings_global::TlSettings;
use crate::table::actions::CopyRowsAsTable;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};
use crate::text::style::{label_style, t_sm};
use crate::theme::{current, metrics};
use crate::window::main::{ShellState, main_window_view, push_main_toast};
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

gpui_kit::actions!(
    tasklens,
    [
        AppGeneralOpenDesktop,
        AppGeneralGoToService,
        AppGeneralCopyRow,
        AppGeneralCopyPanel
    ]
);

const TABS: [DetailTab; 2] = [DetailTab::AppGeneral, DetailTab::AppProcesses];
const PANEL_BREAKPOINT: f32 = 640.;
const ROW_HEIGHT: f32 = 26.;
const LABEL_MIN: f32 = 96.;
const LABEL_MAX: f32 = 150.;
const HEADER_HEIGHT: f32 = 64.;
const ICON_SIZE: f32 = 48.;
const BANNER_HEIGHT: f32 = 24.;
const TITLE_BAR_HEIGHT: f32 = 34.;
const STRIP_HEIGHT: f32 = 32.;
const TOAST_GAP: f32 = 8.;

pub const TOO_MANY_APP_WINDOWS: &str = "Too many app properties windows are already open";
pub const APP_EXITED_BANNER: &str =
    "This app has no running processes. Values are frozen at the last update.";

/// Whether the two General panels use the horizontal layout.
pub fn panels_side_by_side(body_width: f32, ui_scale: f32) -> bool {
    body_width >= PANEL_BREAKPOINT * ui_scale
}

#[derive(Clone)]
struct AppWords {
    general: SharedString,
    processes: SharedString,
    general_tip: SharedString,
    processes_tip: SharedString,
    too_many: SharedString,
    exited: SharedString,
    app: SharedString,
    unit: SharedString,
    go_to_service: SharedString,
    labels: [SharedString; 13],
    cmdline: SharedString,
    copy: SharedString,
    copied: SharedString,
    opening: SharedString,
    open_failed: SharedString,
    read: SharedString,
    write: SharedString,
    receive: SharedString,
    send: SharedString,
}

impl AppWords {
    fn new(cx: &App) -> Rc<Self> {
        let t = |key| tr(cx, key);
        Rc::new(Self {
            general: t("pwn-app-props-tab-general"),
            processes: t("pwn-app-props-tab-processes"),
            general_tip: t("pwn-app-props-tab-general-tooltip"),
            processes_tip: t("pwn-app-props-tab-processes-tooltip"),
            too_many: t("pwn-app-props-too-many-windows"),
            exited: t("pwn-app-props-exited-banner"),
            app: t("pwn-app-props-kind-app"),
            unit: t("pwn-app-props-kind-unit"),
            go_to_service: t("pwn-general-link-unit"),
            labels: [
                t("pwn-app-props-label-app-id"),
                t("pwn-app-props-label-type"),
                t("pwn-app-props-label-desktop-file"),
                t("pwn-app-props-label-unit"),
                t("pwn-app-props-label-cgroup"),
                t("pwn-app-props-label-processes"),
                t("pwn-app-props-label-cpu"),
                t("pwn-app-props-label-memory"),
                t("pwn-app-props-label-swap"),
                t("pwn-app-props-label-disk"),
                t("pwn-app-props-label-network"),
                t("pwn-app-props-label-gpu"),
                t("pwn-app-props-label-vram"),
            ],
            cmdline: t("pwn-app-props-command-line"),
            copy: t("pwn-app-props-copy"),
            copied: t("pwn-general-toast-value-copied"),
            opening: t("pwn-general-toast-opening"),
            open_failed: t("pwn-general-toast-open-failed"),
            read: t("pwn-app-props-rate-read"),
            write: t("pwn-app-props-rate-write"),
            receive: t("pwn-app-props-rate-receive"),
            send: t("pwn-app-props-rate-send"),
        })
    }
}

/// Opens App Properties, focuses an existing window, or refuses a ninth app window. The window
/// shows `host` and stays on it.
pub fn open_app_properties(host: &HostContext, group: GroupKey, cx: &mut App) {
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        warn!(target: "tasklens::ui", "no window registry, app properties not opened");
        return;
    };
    let words = AppWords::new(cx);
    let row = host.hub().and_then(|hub| {
        hub.read(cx)
            .current()
            .table
            .groups
            .iter()
            .find(|row| row.key == group)
            .cloned()
    });
    let source = cx.active_window();
    let mut opened = None;
    let outcome = registry.update(cx, |registry, cx| {
        registry.open_or_focus(WindowKey::AppProperties(group), cx, |cx| {
            let Some(row) = row else {
                return Err(gpui_kit::gpui::private::anyhow::anyhow!(
                    "app group is not running"
                ));
            };
            let title: SharedString = host.titled(&app_props_title(&row.name, false)).into();
            // The key lets the title bar pin send keep-above for this window (R-073, D-031).
            let spec = pwn_window_spec(cx, PwnWindow::AppProperties, title, None)
                .key(WindowKey::AppProperties(group));
            let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
                cx.new(|cx| AppPropsWindow::new(host, group, row, window, cx))
            })?;
            opened = Some(handle);
            Ok(handle.into())
        })
    });
    match outcome {
        OpenOutcome::Opened => {
            if let Some(handle) = opened {
                place_toasts(handle, cx);
            }
        }
        OpenOutcome::Focused => {}
        OpenOutcome::CapReached(_) => {
            let queue = source.and_then(|window| {
                window
                    .update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
                    .ok()
                    .flatten()
            });
            if let Some(queue) = queue {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Warning, words.too_many.clone(), None, None, cx)
                });
            } else {
                push_main_toast(cx, ToastKind::Warning, words.too_many.clone(), None, None);
            }
        }
        OpenOutcome::Failed => {
            warn!(target: "tasklens::ui", group = group.0, "app properties target disappeared");
        }
    }
}

fn place_toasts(handle: WindowHandle<Root>, cx: &mut App) {
    let Some(frame) = TlSecondaryWindow::<AppPropsWindow>::find(handle, cx) else {
        return;
    };
    let scale = metrics(cx).ui_scale;
    let (bar, toasts) = {
        let frame = frame.read(cx);
        let bar = if frame.has_title_bar() {
            TITLE_BAR_HEIGHT
        } else {
            0.
        };
        (bar, frame.toasts().clone())
    };
    toasts.update(cx, |q, cx| {
        q.set_top(px((bar + STRIP_HEIGHT + TOAST_GAP) * scale), cx)
    });
}

#[derive(Clone)]
struct LiveUsage {
    name: Arc<str>,
    kind: AppKind,
    icon_path: Option<Arc<str>>,
    cpu_pct: f32,
    net_rx_bps: f32,
    net_tx_bps: f32,
    gpu_pct: f32,
    vram_bytes: u64,
}

fn value(text: SharedString, validity: Validity) -> TlValue {
    let mut value = TlValue::new(text);
    value.validity = validity;
    value
}

fn total_value<T>(total: &Total<T>, format: impl FnOnce(&T, &mut String)) -> TlValue {
    let mut text = String::new();
    if total.validity.is_valid() || total.validity == Validity::Stale {
        format(&total.value, &mut text);
    }
    value(text.into(), total.validity)
}

fn worst(a: Validity, b: Validity) -> Validity {
    if a.is_valid() { b } else { a }
}

fn section(rows: Vec<TlKvRow>) -> Rc<[TlKvSection]> {
    Rc::from([TlKvSection {
        title: None,
        blocks: vec![TlKvBlock {
            heading: None,
            rows,
            indent: None,
        }],
    }])
}

/// The General child view. It is public so the UI tests can feed payloads without an engine.
pub struct AppGeneralTab {
    /// The host this tab shows. Set when it opens; never changes.
    host: HostContext,
    group: GroupKey,
    payload: Option<Arc<AppGeneralDetail>>,
    live: Option<LiveUsage>,
    words: Rc<AppWords>,
    filter: Entity<PropsFilter>,
    needle: SharedString,
    identity: Rc<[TlKvSection]>,
    usage: Rc<[TlKvSection]>,
    width: Rc<Cell<Pixels>>,
    side_by_side: bool,
    exited: bool,
    network_unit: NetUnit,
    copy_focus: FocusHandle,
    grid_focus: FocusHandle,
    cursor: Option<(usize, usize)>,
    panel_bounds: Rc<[Cell<Bounds<Pixels>>; 2]>,
    cmdline_text: SharedString,
    cmdline_validity: Validity,
    _subs: Vec<Subscription>,
}

impl AppGeneralTab {
    pub fn new(
        host: &HostContext,
        group: GroupKey,
        filter: Entity<PropsFilter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.subscribe(&filter, |this, filter, event: &FilterChanged, cx| {
            if filter.update(cx, |f, _| f.accept(event.generation)) {
                this.needle = event.needle.clone();
                this.rebuild();
                cx.notify();
            }
        });
        let network_unit = if cx.has_global::<TlSettings>() {
            match TlSettings::settings(cx).units.network {
                NetworkUnit::Bits => NetUnit::Bits,
                NetworkUnit::Bytes => NetUnit::Bytes,
            }
        } else {
            NetUnit::Bits
        };
        let mut this = Self {
            host: host.clone(),
            group,
            payload: None,
            live: None,
            words: AppWords::new(cx),
            filter,
            needle: SharedString::default(),
            identity: section(Vec::new()),
            usage: section(Vec::new()),
            width: Rc::new(Cell::new(px(0.))),
            side_by_side: false,
            exited: false,
            network_unit,
            copy_focus: cx.focus_handle(),
            grid_focus: cx.focus_handle(),
            cursor: None,
            panel_bounds: Rc::new(std::array::from_fn(|_| Cell::new(Bounds::default()))),
            cmdline_text: SharedString::default(),
            cmdline_validity: Validity::WarmingUp,
            _subs: vec![sub],
        };
        this.rebuild();
        this
    }

    pub fn group(&self) -> GroupKey {
        self.group
    }

    pub fn filter(&self) -> &Entity<PropsFilter> {
        &self.filter
    }

    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        let next = payload.and_then(|p| match p.as_ref() {
            TabPayload::AppGeneral(detail) => Some(detail.clone()),
            _ => None,
        });
        let same = match (&self.payload, &next) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.payload = next;
        let (cmdline_text, cmdline_validity) = self.payload.as_ref().map_or_else(
            || (SharedString::default(), Validity::WarmingUp),
            |detail| {
                let validity = detail.primary_cmdline.validity;
                let mut shown = String::new();
                if validity.is_valid() || validity == Validity::Stale {
                    escape_controls(&mut shown, &detail.primary_cmdline.value);
                }
                (shown.into(), validity)
            },
        );
        self.cmdline_text = cmdline_text;
        self.cmdline_validity = cmdline_validity;
        self.rebuild();
        cx.notify();
    }

    pub fn set_group(&mut self, row: Option<&GroupRow>, cx: &mut Context<Self>) {
        let next = row.map(|row| LiveUsage {
            name: row.name.clone(),
            kind: if row.kind == tl_model::process::GroupRowKind::Unit {
                AppKind::Unit
            } else {
                AppKind::App
            },
            icon_path: row.icon.clone(),
            cpu_pct: row.cpu_pct,
            net_rx_bps: row.net_rx_bps,
            net_tx_bps: row.net_tx_bps,
            gpu_pct: row.gpu_util_pct,
            vram_bytes: row.gpu_mem_bytes,
        });
        let unchanged = self.live.as_ref().zip(next.as_ref()).is_some_and(|(a, b)| {
            Arc::ptr_eq(&a.name, &b.name)
                && a.kind == b.kind
                && a.icon_path == b.icon_path
                && a.cpu_pct.to_bits() == b.cpu_pct.to_bits()
                && a.net_rx_bps.to_bits() == b.net_rx_bps.to_bits()
                && a.net_tx_bps.to_bits() == b.net_tx_bps.to_bits()
                && a.gpu_pct.to_bits() == b.gpu_pct.to_bits()
                && a.vram_bytes == b.vram_bytes
        });
        if unchanged || (self.live.is_none() && next.is_none()) {
            return;
        }
        self.live = next;
        self.rebuild();
        cx.notify();
    }

    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            self.rebuild();
            cx.notify();
        }
    }

    pub fn swap_text(&self) -> SharedString {
        self.usage
            .first()
            .and_then(|s| s.blocks.first())
            .and_then(|b| b.rows.iter().find(|row| row.label == self.words.labels[8]))
            .map_or_else(SharedString::default, |r| r.value.text.clone())
    }

    pub fn panels_are_side_by_side(&self) -> bool {
        self.side_by_side
    }

    pub fn grid_focus(&self) -> &FocusHandle {
        &self.grid_focus
    }

    pub fn rows(&self, panel: usize) -> &[TlKvRow] {
        let sections = if panel == 0 {
            &self.identity
        } else {
            &self.usage
        };
        sections
            .first()
            .and_then(|section| section.blocks.first())
            .map_or(&[], |block| block.rows.as_slice())
    }

    pub fn set_cursor(&mut self, panel: usize, row: usize, cx: &mut Context<Self>) {
        let len = self.rows(panel).len();
        if len == 0 {
            return;
        }
        let next = Some((panel, row.min(len - 1)));
        if self.cursor != next {
            self.cursor = next;
            cx.notify();
        }
    }

    fn ensure_cursor(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cursor.is_some() {
            return false;
        }
        if let Some(panel) = (0..2).find(|&panel| !self.rows(panel).is_empty()) {
            self.set_cursor(panel, 0, cx);
        }
        true
    }

    fn step_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.ensure_cursor(cx) {
            return;
        }
        if let Some((panel, row)) = self.cursor {
            self.set_cursor(panel, row.saturating_add_signed(delta), cx);
        }
    }

    fn step_panel(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.ensure_cursor(cx) {
            return;
        }
        let Some((panel, row)) = self.cursor else {
            return;
        };
        let next = if forward {
            (panel + 1..2).find(|&p| !self.rows(p).is_empty())
        } else {
            (0..panel).rev().find(|&p| !self.rows(p).is_empty())
        };
        if let Some(next) = next {
            self.set_cursor(next, row, cx);
        }
    }

    fn row_end(&mut self, last: bool, cx: &mut Context<Self>) {
        self.ensure_cursor(cx);
        if let Some((panel, _)) = self.cursor {
            self.set_cursor(panel, if last { usize::MAX } else { 0 }, cx);
        }
    }

    fn cursor_row(&self) -> Option<&TlKvRow> {
        let (panel, row) = self.cursor?;
        self.rows(panel).get(row)
    }

    fn copy(&self, text: String, value: bool, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let toast = if value {
            self.words.copied.clone()
        } else {
            tr(cx, "pwn-general-toast-copied")
        };
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, toast, None, None, cx)
            });
        }
    }

    fn value_text(&self, row: &TlKvRow, cx: &mut App) -> String {
        shown_text_in(&row.value, &kv_words(cx))
            .map(|text| value_copy_form(&text).into_owned())
            .unwrap_or_default()
    }

    fn copy_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.cursor_row().map(|row| self.value_text(row, cx)) {
            self.copy(text, true, window, cx);
        }
    }

    fn copy_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self
            .cursor_row()
            .and_then(|row| row_copy_text(row, &kv_words(cx)))
        {
            self.copy(text, false, window, cx);
        }
    }

    fn copy_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((panel, _)) = self.cursor else {
            return;
        };
        let sections = if panel == 0 {
            &self.identity
        } else {
            &self.usage
        };
        let text = sections.first().map_or_else(String::new, |section| {
            section_copy_text(section, &kv_words(cx))
        });
        self.copy(text, false, window, cx);
    }

    fn copy_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let words = kv_words(cx);
        let mut lines = Vec::new();
        for panel in 0..2 {
            for row in self.rows(panel) {
                if let Some(text) = row_copy_text(row, &words) {
                    lines.push(text);
                }
            }
        }
        lines.push(format!("{}: {}", self.words.cmdline, self.cmdline_text));
        self.copy(lines.join("\n"), false, window, cx);
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let linked = |index: usize| {
            self.cursor_row().is_some_and(|row| {
                row.label == self.words.labels[index] && row.value.link.is_some()
            })
        };
        if linked(2) {
            self.open_desktop(window, cx);
        } else if linked(3) {
            self.go_to_unit(cx);
        } else {
            self.copy_value(window, cx);
        }
    }

    /// The Unit link: the Services page selects the app's unit.
    fn go_to_unit(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        if let Some(unit) = self.payload.as_ref().and_then(|d| d.unit.clone()) {
            go_to_service(&unit, cx);
        }
    }

    fn open_field_menu_at(
        &mut self,
        panel: usize,
        row: usize,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rows(panel).get(row).is_none() {
            return;
        }
        self.set_cursor(panel, row, cx);
        let position = position.unwrap_or_else(|| {
            let m = metrics(cx);
            let bounds = self.panel_bounds[panel].get();
            point(
                bounds.left() + m.sp[4] * 2.,
                bounds.top() + m.sp[3] + px(ROW_HEIGHT * m.ui_scale) * (row + 1) as f32,
            )
        });
        let menu = TlMenu::build(window, cx, self.grid_focus.clone(), |menu, _, cx| {
            menu.item(tr(cx, "pwn-general-copy-value"), Box::new(PropsCopyValue))
                .item(tr(cx, "pwn-general-copy-row"), Box::new(AppGeneralCopyRow))
                .item(
                    tr(cx, "pwn-general-copy-section"),
                    Box::new(AppGeneralCopyPanel),
                )
                .separator()
                .item(tr(cx, "pwn-general-copy-all"), Box::new(PropsCopyAllFields))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    fn on_menu_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_cursor(cx);
        if let Some((panel, row)) = self.cursor {
            self.open_field_menu_at(panel, row, None, window, cx);
        }
    }

    fn shown(&self, label: SharedString, mut value: TlValue) -> Option<TlKvRow> {
        if self.exited && value.link.is_some() {
            value.link = None;
            value.tooltip = None;
            if value.validity == Validity::Valid {
                value.validity = Validity::Stale;
            }
        }
        let haystack = lowercase(&format!("{label} {}", value.text));
        matches(&haystack, &self.needle).then_some(TlKvRow { label, value })
    }

    fn plain(&self, index: usize, text: impl Into<SharedString>) -> Option<TlKvRow> {
        self.shown(self.words.labels[index].clone(), TlValue::new(text))
    }

    fn collecting(&self, index: usize) -> Option<TlKvRow> {
        self.shown(
            self.words.labels[index].clone(),
            value(SharedString::default(), Validity::WarmingUp),
        )
    }

    fn not_reported(&self, index: usize, path: bool) -> Option<TlKvRow> {
        let mut value = value(SharedString::default(), Validity::Unsupported);
        value.word = Some(UnsupportedWord::NotReported);
        value.path = path;
        self.shown(self.words.labels[index].clone(), value)
    }

    fn rebuild(&mut self) {
        let mut identity = Vec::with_capacity(6);
        let mut usage = Vec::with_capacity(7);
        if let Some(detail) = &self.payload {
            identity.push(match &detail.app_id {
                Some(id) => self.plain(0, id.clone()),
                None => self.not_reported(0, false),
            });
            identity.push(self.plain(
                1,
                match detail.kind {
                    AppKind::App => self.words.app.clone(),
                    AppKind::Unit => self.words.unit.clone(),
                },
            ));
            if let Some(path) = &detail.desktop_file {
                let mut desktop = TlValue::new(SharedString::from(path));
                desktop.path = true;
                desktop.link = Some(Box::new(AppGeneralOpenDesktop));
                identity.push(self.shown(self.words.labels[2].clone(), desktop));
            } else {
                identity.push(self.not_reported(2, true));
            }
            if let Some(unit) = &detail.unit {
                let mut link = TlValue::new(unit.name.clone());
                link.link = Some(Box::new(AppGeneralGoToService));
                link.tooltip = Some(self.words.go_to_service.clone());
                identity.push(self.shown(self.words.labels[3].clone(), link));
            }
            identity.push(match &detail.cgroup {
                Some(cgroup) => self.plain(4, cgroup.clone()),
                None => self.not_reported(4, true),
            });
            identity.push(self.plain(5, detail.processes.to_string()));
            usage.push(self.live.as_ref().and_then(|live| {
                let mut text = String::new();
                units::percent(&mut text, live.cpu_pct);
                self.plain(6, text)
            }));
            usage.push(self.shown(
                self.words.labels[7].clone(),
                total_value(&detail.memory_anon_bytes, |v, out| units::bytes(out, *v)),
            ));
            usage.push(self.shown(
                self.words.labels[8].clone(),
                total_value(&detail.swap_bytes, |v, out| units::bytes(out, *v)),
            ));
            usage.push(self.shown(
                self.words.labels[9].clone(),
                total_value(
                    &Total {
                        value: (detail.disk_read_bps.value, detail.disk_write_bps.value),
                        validity: worst(
                            detail.disk_read_bps.validity,
                            detail.disk_write_bps.validity,
                        ),
                    },
                    |(read, write), out| {
                        out.push_str(&self.words.read);
                        out.push(' ');
                        units::rate_mib_fixed(out, *read);
                        out.push_str(", ");
                        out.push_str(&self.words.write);
                        out.push(' ');
                        units::rate_mib_fixed(out, *write);
                    },
                ),
            ));
            usage.push(self.live.as_ref().and_then(|live| {
                let mut text = self.words.receive.to_string();
                text.push(' ');
                units::net_rate(
                    &mut text,
                    f64::from(live.net_rx_bps),
                    self.network_unit,
                    false,
                );
                text.push_str(", ");
                text.push_str(&self.words.send);
                text.push(' ');
                units::net_rate(
                    &mut text,
                    f64::from(live.net_tx_bps),
                    self.network_unit,
                    false,
                );
                self.plain(10, text)
            }));
            usage.push(self.live.as_ref().and_then(|live| {
                let mut text = String::new();
                units::percent(&mut text, live.gpu_pct);
                self.plain(11, text)
            }));
            usage.push(self.live.as_ref().and_then(|live| {
                let mut text = String::new();
                units::bytes(&mut text, live.vram_bytes);
                self.plain(12, text)
            }));
        } else {
            identity.extend((0..=5).map(|i| self.collecting(i)));
            usage.extend((6..=12).map(|i| self.collecting(i)));
        }
        self.identity = section(identity.into_iter().flatten().collect());
        self.usage = section(usage.into_iter().flatten().collect());
        self.cursor = self.cursor.and_then(|(panel, row)| {
            let len = self.rows(panel).len();
            (len > 0).then(|| (panel, row.min(len - 1)))
        });
    }

    fn open_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let Some(path) = self.payload.as_ref().and_then(|d| d.desktop_file.clone()) else {
            return;
        };
        let Some(tracker) = self.host.tracker().cloned() else {
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            return;
        };
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager {
                path,
                properties: false,
            },
            targets: SmallVec::new(),
            privileged: false,
        };
        let texts = ToastTexts::new(SharedString::default(), self.words.open_failed.clone());
        if tracker
            .update(cx, |t, cx| {
                t.track_or_toast(request, queue.downgrade(), texts, cx)
            })
            .is_some()
        {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Info, self.words.opening.clone(), None, None, cx)
            });
        }
    }

    fn header(&self, cx: &App) -> AnyElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let scale = m.ui_scale;
        let (name, kind, icon_path) = self.payload.as_ref().map_or_else(
            || match &self.live {
                Some(live) => (
                    SharedString::from(&live.name),
                    match live.kind {
                        AppKind::App => self.words.app.clone(),
                        AppKind::Unit => self.words.unit.clone(),
                    },
                    live.icon_path.clone(),
                ),
                None => (SharedString::default(), self.words.app.clone(), None),
            },
            |detail| {
                (
                    SharedString::from(&detail.name),
                    match detail.kind {
                        AppKind::App => self.words.app.clone(),
                        AppKind::Unit => self.words.unit.clone(),
                    },
                    detail.icon_path.clone(),
                )
            },
        );
        let icon_text: SharedString = format!("{name} icon").into();
        let icon_size = px(ICON_SIZE * scale);
        let fallback_color = theme.text_secondary;
        let fallback = move || {
            Icon::new(IconName::AppWindow)
                .size(icon_size)
                .text_color(fallback_color)
                .into_any_element()
        };
        let icon = icon_path.map_or_else(fallback, |path| {
            img(ImageSource::from(Arc::<Path>::from(Path::new(
                path.as_ref(),
            ))))
            .size(icon_size)
            .with_fallback(fallback)
            .into_any_element()
        });
        let mut title = div().truncate().text_color(theme.text_primary);
        *title.text_style() = label_style(&m, m.text.lg, FontWeight::SEMIBOLD);
        let mut subtitle = div().truncate().text_color(theme.text_secondary);
        *subtitle.text_style() = t_sm(&m);
        h_flex()
            .debug_selector(|| "AppGeneralTab-header".into())
            .h(px(HEADER_HEIGHT * scale))
            .px(m.sp[5])
            .gap(m.sp[4])
            .items_center()
            .child(
                div()
                    .id("app-general-icon")
                    .aria_label(icon_text)
                    .child(icon),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(title.child(name))
                    .child(subtitle.child(kind)),
            )
            .into_any_element()
    }

    fn command_line(&self, cx: &App) -> AnyElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let text = self.cmdline_text.clone();
        let validity = self.cmdline_validity;
        let valid = validity.is_valid() || validity == Validity::Stale;
        let copy_text = text.clone();
        let copied = self.words.copied.clone();
        let button = TlButton::new("app-general-cmdline-copy", TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .icon(TlIcon::Copy)
            .label(self.words.copy.clone())
            .focus_handle(self.copy_focus.clone())
            .disabled(!valid)
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| {
                        q.push(ToastKind::Success, copied.clone(), None, None, cx)
                    });
                }
            });
        let mut heading = div().flex_1().min_w_0().text_color(theme.text_secondary);
        *heading.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
        let mut body = div()
            .w_full()
            .whitespace_normal()
            .text_color(theme.text_primary);
        *body.text_style() = t_sm(&m);
        let shown = match validity {
            Validity::Valid | Validity::Stale => text,
            Validity::Restricted => tr(cx, "fnd-kv-restricted"),
            Validity::Unsupported => tr(cx, "fnd-kv-not-reported"),
            Validity::Failed => tr(cx, "fnd-kv-unavailable"),
            _ => tr(cx, "fnd-kv-collecting"),
        };
        v_flex()
            .debug_selector(|| "AppGeneralTab-cmdline".into())
            .w_full()
            .p(m.sp[4])
            .bg(theme.list_bg)
            .rounded(m.radius_small)
            .child(
                h_flex()
                    .h(px(ROW_HEIGHT * m.ui_scale))
                    .items_center()
                    .child(heading.child(self.words.cmdline.clone()))
                    .child(button),
            )
            .child(body.child(shown))
            .into_any_element()
    }
}

impl Render for AppGeneralTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let measured = self.width.get();
        let body_width = if measured > px(0.) {
            measured
        } else {
            window.viewport_size().width - m.sp[5] * 2.
        };
        let body_width_f32 = f32::from(body_width);
        self.side_by_side = panels_side_by_side(body_width_f32, m.ui_scale);
        let label_width =
            px((LABEL_MAX * m.ui_scale).min((body_width_f32 / 4.).max(LABEL_MIN * m.ui_scale)));
        let copied = self.words.copied.clone();
        let focused = self.grid_focus.contains_focused(window, cx);
        let cursor = self.cursor;
        let view = cx.entity().downgrade();
        let grid = |id, panel_ix: usize, sections: &[TlKvSection]| {
            let copied = copied.clone();
            let view_for_press = view.clone();
            let view_for_menu = view.clone();
            TlKeyValueGrid::new(id, KvLayout::Inline)
                .sections(sections)
                .row_height(px(ROW_HEIGHT * m.ui_scale))
                .label_width(label_width)
                .copy_on_click(true)
                .cursor(
                    cursor
                        .filter(|(panel, _)| *panel == panel_ix && focused)
                        .map(|(_, row)| row),
                )
                .on_copied(move |_, window, cx| {
                    if let Some(queue) = ToastQueue::of_window(window, cx) {
                        queue.update(cx, |q, cx| {
                            q.push(ToastKind::Success, copied.clone(), None, None, cx)
                        });
                    }
                })
                .on_value_mouse_down(move |row, _, cx| {
                    let _ =
                        view_for_press.update(cx, |this, cx| this.set_cursor(panel_ix, row, cx));
                })
                .on_context_menu(move |row, position, window, cx| {
                    let _ = view_for_menu.update(cx, |this, cx| {
                        this.open_field_menu_at(panel_ix, row, Some(position), window, cx);
                    });
                })
        };
        let panel_bounds = self.panel_bounds.clone();
        let panel = |id: &'static str, panel_ix: usize, grid: TlKeyValueGrid| {
            let bounds = panel_bounds.clone();
            div()
                .debug_selector(move || id.into())
                .relative()
                .min_w_0()
                .when(self.side_by_side, |d| d.flex_1().flex_basis(px(0.)))
                .when(!self.side_by_side, |d| d.w_full())
                .bg(theme.list_bg)
                .rounded(m.radius_small)
                .px(m.sp[4])
                .py(m.sp[3])
                .child(
                    canvas(
                        move |value, _, _| bounds[panel_ix].set(value),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(grid)
                .into_any_element()
        };
        let identity = panel(
            "AppGeneralTab-identity",
            0,
            grid("app-identity", 0, &self.identity),
        );
        let usage = panel("AppGeneralTab-usage", 1, grid("app-usage", 1, &self.usage));
        let panels: AnyElement = if self.side_by_side {
            h_flex()
                .w_full()
                .items_start()
                .gap(m.sp[4])
                .child(identity)
                .child(usage)
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap(m.sp[4])
                .child(identity)
                .child(usage)
                .into_any_element()
        };
        let width = self.width.clone();
        let pad = m.sp[5] * 2.;
        div()
            .debug_selector(|| "AppGeneralTab".into())
            .on_action(cx.listener(|this, _: &AppGeneralOpenDesktop, window, cx| {
                this.open_desktop(window, cx);
            }))
            .on_action(cx.listener(|this, _: &AppGeneralGoToService, _, cx| {
                this.go_to_unit(cx);
            }))
            .relative()
            .size_full()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let inner = (bounds.size.width - pad).max(px(0.));
                        if width.get() != inner {
                            width.set(inner);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div().size_full().overflow_y_scrollbar().child(
                    v_flex().w_full().child(self.header(cx)).child(
                        v_flex()
                            .w_full()
                            .px(m.sp[5])
                            .py(m.sp[4])
                            .child(
                                div()
                                    .id("app-general-fields")
                                    .key_context(KV_GRID_CONTEXT)
                                    .track_focus(&self.grid_focus)
                                    .on_action(cx.listener(|this, _: &PropsFieldUp, _, cx| {
                                        this.step_row(-1, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &PropsFieldDown, _, cx| {
                                        this.step_row(1, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &PropsFieldLeft, _, cx| {
                                        this.step_panel(false, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &PropsFieldRight, _, cx| {
                                        this.step_panel(true, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &PropsFieldHome, _, cx| {
                                        this.row_end(false, cx)
                                    }))
                                    .on_action(cx.listener(|this, _: &PropsFieldEnd, _, cx| {
                                        this.row_end(true, cx)
                                    }))
                                    .on_action(cx.listener(
                                        |this, _: &PropsActivateField, window, cx| {
                                            this.activate(window, cx)
                                        },
                                    ))
                                    .on_action(cx.listener(
                                        |this, _: &PropsCopyValue, window, cx| {
                                            this.copy_value(window, cx)
                                        },
                                    ))
                                    .on_action(cx.listener(
                                        |this, _: &AppGeneralCopyRow, window, cx| {
                                            this.copy_row(window, cx)
                                        },
                                    ))
                                    .on_action(cx.listener(
                                        |this, _: &AppGeneralCopyPanel, window, cx| {
                                            this.copy_panel(window, cx)
                                        },
                                    ))
                                    .on_action(cx.listener(
                                        |this, _: &PropsCopyAllFields, window, cx| {
                                            this.copy_all(window, cx)
                                        },
                                    ))
                                    .on_action(cx.listener(
                                        |this, _: &PropsFieldMenu, window, cx| {
                                            this.on_menu_key(window, cx)
                                        },
                                    ))
                                    .child(panels),
                            )
                            .child(div().mt(m.sp[4]).child(self.command_line(cx))),
                    ),
                ),
            )
    }
}

// ---- Processes tab (UI-PWN-020, PROC-085) ----

/// The longest command line the table shows, in bytes (UI-PWN-020).
const CMDLINE_SHOWN: usize = 1024;

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct AppProcsCopyCell(AppProcColumn);

#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct AppProcsOpenProps;

#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct AppProcsGoTo;

/// The columns of the member table, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppProcColumn {
    Name,
    Pid,
    Cpu,
    Memory,
    Disk,
    User,
    Cmdline,
}

impl AppProcColumn {
    pub const ALL: [AppProcColumn; 7] = [
        Self::Name,
        Self::Pid,
        Self::Cpu,
        Self::Memory,
        Self::Disk,
        Self::User,
        Self::Cmdline,
    ];

    /// Header key, width at 100%, minimum width, alignment. The command line takes the most room.
    fn meta(self) -> (&'static str, f32, f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Name => ("pwn-app-procs-col-name", 220., 120., Left),
            Self::Pid => ("pwn-app-procs-col-pid", 80., 60., Right),
            Self::Cpu => ("pwn-app-procs-col-cpu", 70., 60., Right),
            Self::Memory => ("pwn-app-procs-col-memory", 100., 70., Right),
            Self::Disk => ("pwn-app-procs-col-disk", 90., 70., Right),
            Self::User => ("pwn-app-procs-col-user", 110., 60., Left),
            Self::Cmdline => ("pwn-app-procs-col-cmdline", 480., 160., Left),
        }
    }
}

impl TableColumn for AppProcColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Pid => "pid",
            Self::Cpu => "cpu",
            Self::Memory => "mem",
            Self::Disk => "disk",
            Self::User => "user",
            Self::Cmdline => "cmd",
        }
    }

    /// The layout is never saved, so no stored key maps to it.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }

    fn min_width(self) -> Option<f32> {
        Some(self.meta().2)
    }
}

/// The sort of the member table, kept for the window's life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AppProcSort {
    pub column: AppProcColumn,
    pub ascending: bool,
}

impl Default for AppProcSort {
    /// PID ascending.
    fn default() -> Self {
        Self {
            column: AppProcColumn::Pid,
            ascending: true,
        }
    }
}

impl TableSort<AppProcColumn> for AppProcSort {
    fn column(&self) -> AppProcColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The tab's words, translated once per strings version.
struct AppProcWords {
    strings_version: Option<u64>,
    headers: [SharedString; 7],
    loading: SharedString,
    empty: SharedString,
    no_match: SharedString,
    open: SharedString,
    go_to: SharedString,
    copy: SharedString,
    copy_table: SharedString,
    copied: SharedString,
}

impl AppProcWords {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            strings_version: cx.try_global::<Strings>().map(|s| s.strings_version),
            headers: AppProcColumn::ALL.map(|c| t(c.meta().0)),
            loading: t("pwn-app-procs-loading"),
            empty: t("pwn-app-procs-empty"),
            no_match: t(NO_MATCH),
            open: t("pwn-app-procs-menu-open"),
            go_to: t("pwn-app-procs-menu-go-to"),
            copy: t("pwn-app-procs-menu-copy"),
            copy_table: t("pwn-app-procs-menu-copy-table"),
            copied: t("pwn-app-procs-copied"),
        }
    }
}

/// The first 1 KiB of a command line, cut on a character boundary.
fn shown_cmdline(cmdline: &str) -> &str {
    if cmdline.len() <= CMDLINE_SHOWN {
        return cmdline;
    }
    let mut end = CMDLINE_SHOWN;
    while !cmdline.is_char_boundary(end) {
        end -= 1;
    }
    &cmdline[..end]
}

fn record_cmdline(record: &ProcessRecord) -> &str {
    record.meta.cmdline.as_deref().map_or("", shown_cmdline)
}

fn disk_bps(record: &ProcessRecord) -> f32 {
    record.io_read_bps + record.io_write_bps
}

fn compare(a: &ProcessRecord, b: &ProcessRecord, col: AppProcColumn) -> Ordering {
    let by = match col {
        AppProcColumn::Pid => Ordering::Equal,
        AppProcColumn::Name => a.meta.name.cmp(&b.meta.name),
        AppProcColumn::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
        AppProcColumn::Memory => a.rss_anon_bytes.cmp(&b.rss_anon_bytes),
        AppProcColumn::Disk => disk_bps(a).total_cmp(&disk_bps(b)),
        AppProcColumn::User => a.meta.user.cmp(&b.meta.user),
        AppProcColumn::Cmdline => record_cmdline(a).cmp(record_cmdline(b)),
    };
    // ProcessKey orders by start time first, so ties and the PID column use the PID.
    by.then(a.key.pid.cmp(&b.key.pid)).then(a.key.cmp(&b.key))
}

fn app_proc_specs(words: &AppProcWords) -> Vec<ColumnSpec<AppProcColumn>> {
    AppProcColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width, _, align) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align,
                kind: match align {
                    ColumnAlign::Left => ColumnKind::Text,
                    ColumnAlign::Right => ColumnKind::Numeric,
                },
                sortable: true,
                template: None,
                default_width: px(width),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

/// What the row menu captured for its handlers.
#[derive(Clone)]
struct CapturedProcs {
    keys: SmallVec<[ProcessKey; 1]>,
    cell: Option<String>,
}

/// The member table's rows, columns and sort. Rows are positions in `members`, which are
/// indexes into the process records of `procs`.
pub struct AppProcsDelegate {
    /// The host this table shows. Set when it opens; never changes.
    host: HostContext,
    procs: Arc<ProcessSnapshot>,
    members: Arc<[u32]>,
    /// Positions in `members`, in sort order.
    order: Vec<u32>,
    /// Positions in `members` the filter keeps, `None` for all.
    filtered: Option<Arc<[u32]>>,
    /// Positions in `members` shown, in sort order.
    view: Vec<u32>,
    index: FxHashMap<ProcessKey, u32>,
    /// True before the first member pass.
    loading: bool,
    version: u64,
    sort: AppProcSort,
    specs: Vec<ColumnSpec<AppProcColumn>>,
    layout: ColumnLayout<AppProcColumn>,
    words: Rc<AppProcWords>,
    captured: Option<CapturedProcs>,
}

impl AppProcsDelegate {
    fn new(host: &HostContext, words: Rc<AppProcWords>) -> Self {
        let specs = app_proc_specs(&words);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &AppProcColumn::ALL,
            &AppProcColumn::ALL,
        );
        Self {
            host: host.clone(),
            procs: Arc::new(ProcessSnapshot::default()),
            members: Arc::from([]),
            order: Vec::new(),
            filtered: None,
            view: Vec::new(),
            index: FxHashMap::default(),
            loading: true,
            version: 0,
            sort: AppProcSort::default(),
            specs,
            layout,
            words,
            captured: None,
        }
    }

    /// Stands in for the row menu: captures `keys` as the menu's targets.
    #[cfg(feature = "test-hooks")]
    pub fn capture_for_test(&mut self, keys: &[ProcessKey]) {
        self.captured = Some(CapturedProcs {
            keys: keys.iter().copied().collect(),
            cell: None,
        });
    }

    /// The keys shown, in view order.
    pub fn view_keys(&self) -> impl Iterator<Item = ProcessKey> + '_ {
        (0..self.view.len()).map(|ix| self.record(ix).key)
    }

    /// The text the `col` cell of process `key` shows, `None` when the process is not shown.
    pub fn cell_text(&self, key: ProcessKey, col: AppProcColumn) -> Option<String> {
        let ix = self.row_index(key)?;
        let mut out = String::new();
        write_text(self.record(ix), col, &mut out);
        Some(out)
    }

    fn record(&self, ix: usize) -> &ProcessRecord {
        let member = self.members[self.view[ix] as usize];
        &self.procs.records[member as usize]
    }

    fn set_members(&mut self, procs: Arc<ProcessSnapshot>, members: Arc<[u32]>) {
        self.procs = procs;
        self.members = members;
        self.loading = false;
        self.filtered = None;
        self.resort();
    }

    fn set_filtered(&mut self, filtered: Option<Arc<[u32]>>) {
        self.filtered = filtered;
        self.rebuild_view();
    }

    /// Gives the command line column the width the other columns leave in a body `body_px` wide
    /// (logical px at the layout's scale), never below its minimum. Returns true when the width
    /// changed.
    fn fit_flex_column(&mut self, body_px: f32) -> bool {
        let scale = self.layout.ui_scale().max(0.1);
        let flex = AppProcColumn::Cmdline;
        let others: f32 = AppProcColumn::ALL
            .iter()
            .filter(|&&c| c != flex)
            .map(|c| self.layout.widths.get(c).map_or(0.0, |w| w.as_f32()))
            .sum();
        let min = flex.meta().2;
        let target = (body_px / scale - others).max(min);
        let current = self.layout.widths.get(&flex).map_or(0.0, |w| w.as_f32());
        if (target - current).abs() < 0.5 {
            return false;
        }
        self.layout.widths.insert(flex, px(target));
        self.layout.rebuild(scale);
        self.version += 1;
        true
    }

    fn set_words(&mut self, words: Rc<AppProcWords>) {
        self.specs = app_proc_specs(&words);
        self.words = words;
        self.version += 1;
    }

    fn resort(&mut self) {
        let AppProcSort { column, ascending } = self.sort;
        let (procs, members) = (&self.procs, &self.members);
        let record = |pos: u32| &procs.records[members[pos as usize] as usize];
        self.order.clear();
        self.order.extend(0..members.len() as u32);
        self.order.sort_unstable_by(|&a, &b| {
            let o = compare(record(a), record(b), column);
            if ascending { o } else { o.reverse() }
        });
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        self.view.clear();
        match &self.filtered {
            None => self.view.extend_from_slice(&self.order),
            Some(keep) => {
                let mut shown = vec![false; self.members.len()];
                for &i in keep.iter() {
                    if let Some(s) = shown.get_mut(i as usize) {
                        *s = true;
                    }
                }
                self.view
                    .extend(self.order.iter().copied().filter(|&i| shown[i as usize]));
            }
        }
        self.index.clear();
        for pos in 0..self.view.len() {
            let key = self.record(pos).key;
            self.index.insert(key, pos as u32);
        }
        self.version += 1;
    }
}

fn write_text(r: &ProcessRecord, col: AppProcColumn, out: &mut String) {
    match col {
        AppProcColumn::Name => out.push_str(&r.meta.name),
        AppProcColumn::Pid => {
            let _ = write!(out, "{}", r.key.pid);
        }
        AppProcColumn::Cpu => units::percent(out, r.cpu_pct.max(0.0)),
        AppProcColumn::Memory => units::bytes(out, r.rss_anon_bytes),
        AppProcColumn::Disk => units::rate_mib_fixed(out, f64::from(disk_bps(r).max(0.0))),
        AppProcColumn::User => out.push_str(&r.meta.user),
        AppProcColumn::Cmdline => out.push_str(record_cmdline(r)),
    }
}

/// A text cell's identity bits: the pointer identifies the text, the length guards a reuse.
fn text_bits(text: &str) -> u64 {
    (text.as_ptr() as u64) ^ ((text.len() as u64) << 48)
}

impl TreeTableDelegate for AppProcsDelegate {
    type RowId = ProcessKey;
    type Column = AppProcColumn;
    type Sort = AppProcSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> ProcessKey {
        self.record(ix).key
    }
    fn row_index(&self, id: ProcessKey) -> Option<usize> {
        self.index.get(&id).map(|&p| p as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<AppProcColumn> {
        &self.layout
    }
    fn column_spec(&self, id: AppProcColumn) -> &ColumnSpec<AppProcColumn> {
        let i = AppProcColumn::ALL
            .iter()
            .position(|c| *c == id)
            .unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: AppProcColumn, out: &mut CellOut) {
        let r = self.record(ix);
        out.bits = match col {
            AppProcColumn::Name => text_bits(&r.meta.name),
            AppProcColumn::Pid => u64::from(r.key.pid),
            AppProcColumn::Cpu => u64::from(r.cpu_pct.to_bits()),
            AppProcColumn::Memory => r.rss_anon_bytes,
            AppProcColumn::Disk => u64::from(disk_bps(r).to_bits()),
            AppProcColumn::User => text_bits(&r.meta.user),
            AppProcColumn::Cmdline => text_bits(record_cmdline(r)),
        };
        out.validity = match col {
            AppProcColumn::Cpu => r.validity.get(ProcessField::Cpu),
            AppProcColumn::Memory => r.validity.get(ProcessField::RssAnon),
            AppProcColumn::Disk => worst(
                r.validity.get(ProcessField::IoRead),
                r.validity.get(ProcessField::IoWrite),
            ),
            _ => Validity::Valid,
        };
        if out.want_text {
            write_text(r, col, &mut out.text);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let r = self.record(ix);
        NameCell {
            icon: r
                .meta
                .icon
                .as_ref()
                .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i))),
            icon_size: IconSlot::Small,
            primary: SharedString::from(&r.meta.name),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<AppProcColumn> {
        Some(AppProcColumn::Name)
    }

    fn header_total(&self, _: AppProcColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> AppProcSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    fn context_menu(
        &mut self,
        ids: &[ProcessKey],
        clicked_col: Option<AppProcColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let keys: SmallVec<[ProcessKey; 1]> = ids
            .iter()
            .copied()
            .filter(|key| self.index.contains_key(key))
            .collect();
        if keys.is_empty() {
            return menu;
        }
        let cell = clicked_col.and_then(|col| self.cell_text(keys[0], col));
        let one = keys.len() == 1;
        self.captured = Some(CapturedProcs { keys, cell });
        let w = self.words.clone();
        let mut menu = menu.item(w.open.clone(), Box::new(AppProcsOpenProps));
        menu = if one {
            menu.item(w.go_to.clone(), Box::new(AppProcsGoTo))
        } else {
            menu.item_disabled(w.go_to.clone(), None)
        };
        menu = menu.separator();
        if let Some(col) = clicked_col {
            menu = menu.item(w.copy.clone(), Box::new(AppProcsCopyCell(col)));
        }
        menu.item(w.copy_table.clone(), Box::new(CopyRowsAsTable))
    }

    fn on_toggle_expand(&mut self, _: ProcessKey, _: &mut Context<TlTreeTable<Self>>) {}

    /// A new column sorts descending first; the same column flips.
    fn on_sort(&mut self, col: AppProcColumn, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = AppProcSort {
            column: col,
            ascending: self.sort.column == col && !self.sort.ascending,
        };
        self.resort();
    }

    /// Widths live in memory only.
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<AppProcColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[ProcessKey], _: &mut Context<TlTreeTable<Self>>) {}

    /// Enter, Alt+Enter, Ctrl+I and double-click open the row's Process Properties (D-008).
    fn on_activate(&mut self, id: ProcessKey, cx: &mut Context<TlTreeTable<Self>>) {
        crate::windows::props::open_process_properties(&self.host, id, cx);
    }

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        let w = &self.words;
        Some(if self.loading {
            w.loading.clone()
        } else if self.members.is_empty() {
            w.empty.clone()
        } else {
            w.no_match.clone()
        })
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

/// Record indexes of the live processes that belong to the row `group`, unfiltered. Only the
/// active Processes tab collects them, once per sweep.
fn member_indexes(procs: &ProcessSnapshot, group: GroupKey) -> impl Iterator<Item = u32> + '_ {
    procs
        .records
        .iter()
        .enumerate()
        .filter(move |(i, r)| {
            !r.flags.contains(ProcFlags::EXITED) && is_row_member(r, procs.extras.get(*i), group)
        })
        .map(|(i, _)| i as u32)
}

/// How many live processes belong to the row `group`, unfiltered. The label count while the
/// Processes tab is not active: it counts only, with no member list and no table work.
fn count_members(procs: &ProcessSnapshot, group: GroupKey) -> u32 {
    // A snapshot never holds more than u32::MAX records.
    member_indexes(procs, group).count() as u32
}

/// The Processes child view. It lists the member processes of the window's row while it is the
/// active tab, and does no work while it is not.
pub struct AppProcessesTab {
    /// The host this tab shows. Set when it opens; never changes.
    host: HostContext,
    group: GroupKey,
    table: Entity<TlTreeTable<AppProcsDelegate>>,
    filter: Entity<PropsFilter>,
    active: bool,
    exited: bool,
    /// How many member passes ran, for the tests.
    passes: u32,
    /// Member count of the last pass.
    member_count: u32,
    /// Window width the flex column was last fitted to.
    flex_body: f32,
    /// Bumped by every member pass. A filter result for an older pass is dropped.
    members_epoch: u64,
    words: Rc<AppProcWords>,
    _subs: SmallVec<[Subscription; 3]>,
}

impl AppProcessesTab {
    pub fn new(
        host: &HostContext,
        group: GroupKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(AppProcWords::new(cx));
        let delegate = AppProcsDelegate::new(host, words.clone());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&filter, |this, _, ev: &FilterChanged, cx| {
            this.run_filter(ev.generation, cx);
        }));
        subs.push(cx.subscribe_in(
            &table,
            window,
            |this, _, ev: &TreeTableEvent, window, cx| {
                if matches!(ev, TreeTableEvent::CopiedRows { .. }) {
                    this.toast_copied(window, cx);
                }
            },
        ));
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = cx.try_global::<Strings>().map(|s| s.strings_version);
            if this.words.strings_version != version {
                this.words = Rc::new(AppProcWords::new(cx));
                let words = this.words.clone();
                this.table.update(cx, |t, cx| {
                    t.delegate_mut().set_words(words);
                    t.on_new_version(cx);
                });
            }
        }));
        Self {
            host: host.clone(),
            group,
            table,
            filter,
            active: false,
            exited: false,
            passes: 0,
            member_count: 0,
            flex_body: 0.0,
            members_epoch: 0,
            words,
            _subs: subs,
        }
    }

    pub fn table(&self) -> &Entity<TlTreeTable<AppProcsDelegate>> {
        &self.table
    }

    /// How many member passes ran so far.
    pub fn passes(&self) -> u32 {
        self.passes
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Member count of the last pass, or `None` while the tab is not active.
    pub fn member_count(&self) -> Option<u32> {
        self.active.then_some(self.member_count)
    }

    /// The tab became the active one or stopped being it. Becoming active collects the members
    /// from `snapshot` at once.
    pub fn set_active(
        &mut self,
        active: bool,
        snapshot: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) {
        if self.active == active {
            return;
        }
        self.active = active;
        if let Some(snapshot) = snapshot.filter(|_| active) {
            self.take_snapshot(snapshot, cx);
        }
    }

    /// Freezes the rows once the row left the process table.
    pub fn set_exited(&mut self, exited: bool) {
        self.exited = exited;
    }

    /// One sweep: while the tab is active, one linear pass over the records picks the members.
    pub fn take_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        if !self.active || self.exited {
            return;
        }
        self.passes += 1;
        self.members_epoch += 1;
        let procs = snapshot.processes.clone();
        let group = self.group;
        let members: Arc<[u32]> = member_indexes(&procs, group).collect();
        self.member_count = members.len() as u32;
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_members(procs, members);
            t.on_new_version(cx);
        });
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        }
    }

    /// The lowercase `name pid user command line` of each member, for the filter.
    fn haystacks(&self, cx: &App) -> Arc<[Box<str>]> {
        let d = &self.table.read(cx).delegate;
        let mut hay = String::new();
        d.members
            .iter()
            .map(|&i| {
                let r = &d.procs.records[i as usize];
                hay.clear();
                let _ = write!(hay, "{} {} {} ", r.meta.name, r.key.pid, r.meta.user);
                hay.push_str(record_cmdline(r));
                lowercase(&hay).into_boxed_str()
            })
            .collect()
    }

    fn run_filter(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.filter.read(cx).is_current(generation) {
            return;
        }
        if self.filter.read(cx).needle().is_empty() {
            self.apply_filter(generation, self.members_epoch, None, cx);
            return;
        }
        let epoch = self.members_epoch;
        let rows = self.haystacks(cx);
        let task = self.filter.update(cx, |f, cx| f.run_filter(rows, cx));
        cx.spawn(async move |this, cx| {
            if let Some((generation, keep)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.apply_filter(generation, epoch, Some(keep), cx)
                });
            }
        })
        .detach();
    }

    /// `epoch` is the member pass the `keep` positions were computed for. Positions of an older
    /// pass index other members, so they are dropped.
    fn apply_filter(
        &mut self,
        generation: u64,
        epoch: u64,
        keep: Option<Arc<[u32]>>,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.members_epoch {
            return;
        }
        if !self.filter.update(cx, |f, _| f.accept(generation)) {
            return;
        }
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_filtered(keep);
            t.on_new_version(cx);
        });
    }

    fn toast_copied(&self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.words.copied.clone();
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
        }
    }

    fn captured(&self, cx: &App) -> Option<CapturedProcs> {
        self.table.read(cx).delegate.captured.clone()
    }

    fn on_copy_cell(&mut self, _: &AppProcsCopyCell, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.captured(cx).and_then(|c| c.cell) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast_copied(window, cx);
    }

    fn on_open_props(&mut self, _: &AppProcsOpenProps, _: &mut Window, cx: &mut Context<Self>) {
        let Some(captured) = self.captured(cx) else {
            return;
        };
        for key in captured.keys {
            super::props::open_process_properties(&self.host, key, cx);
        }
    }

    /// "Go to process": the Processes page selects the process through its jump target
    /// (PROC-071), and the main window comes to the front.
    fn on_go_to(&mut self, _: &AppProcsGoTo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.captured(cx).and_then(|c| c.keys.first().copied()) else {
            return;
        };
        go_to_process(key, cx);
    }
}

/// Selects `key` on the Processes tab and brings the main window to the front.
fn go_to_process(key: ProcessKey, cx: &mut App) {
    let main = cx.try_global::<ShellState>().and_then(|s| s.main_window);
    let page =
        main_window_view(cx).and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes));
    let (Some(main), Some(page)) = (main, page) else {
        warn!(target: "tasklens::ui", "no Processes page for Go to process");
        return;
    };
    let _ = main.update(cx, |_, window, cx| {
        window.activate_window();
        page.update(cx, |p, cx| {
            p.select_by_key(key, JumpTexts::default(), window, cx)
        });
    });
}

impl Render for AppProcessesTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The command line column is the flex column: it takes the width the others leave.
        let body = window.viewport_size().width.as_f32();
        if self.flex_body != body {
            self.flex_body = body;
            self.table.update(cx, |t, cx| {
                if t.delegate_mut().fit_flex_column(body) {
                    t.on_new_version(cx);
                }
            });
        }
        div()
            .debug_selector(|| "AppPropsProcesses".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_open_props))
            .on_action(cx.listener(Self::on_go_to))
            .child(self.table.clone())
    }
}

/// Root entity of one App Properties window.
pub struct AppPropsWindow {
    /// The host this window shows. Set when it opens; never changes.
    host: HostContext,
    group: GroupKey,
    active: DetailTab,
    general: Entity<AppGeneralTab>,
    processes: Entity<AppProcessesTab>,
    filters: [Entity<PropsFilter>; 2],
    exited: bool,
    name: SharedString,
    member_count: u32,
    words: Rc<AppWords>,
    focus: FocusHandle,
    demand: Option<EntityId>,
    _subs: Vec<Subscription>,
}

impl AppPropsWindow {
    fn new(
        host: &HostContext,
        group: GroupKey,
        row: GroupRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = AppWords::new(cx);
        let filters = [
            cx.new(|cx| PropsFilter::new(words.general.clone(), window, cx)),
            cx.new(|cx| PropsFilter::new(words.processes.clone(), window, cx)),
        ];
        let general = cx.new(|cx| AppGeneralTab::new(host, group, filters[0].clone(), cx));
        general.update(cx, |g, cx| g.set_group(Some(&row), cx));
        let processes =
            cx.new(|cx| AppProcessesTab::new(host, group, filters[1].clone(), window, cx));
        let mut subs = Vec::new();
        let hub = host.hub().cloned();
        if let Some(hub) = &hub {
            subs.push(cx.subscribe_in(
                hub,
                window,
                |this, hub, event: &SnapshotChanged, window, cx| {
                    this.take_snapshot(&hub.read(cx).current().clone(), event.mask, window, cx);
                },
            ));
        }
        let mut this = Self {
            host: host.clone(),
            group,
            active: DetailTab::AppGeneral,
            general,
            processes,
            filters,
            exited: false,
            name: SharedString::from(&row.name),
            member_count: row.members,
            words,
            focus: cx.focus_handle(),
            demand: None,
            _subs: subs,
        };
        if let Some(demand) = host.demand().cloned() {
            let id = cx.entity_id();
            this.demand = Some(id);
            this.register_demand(window, cx);
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }
        if let Some(hub) = hub {
            this.take_snapshot(
                &hub.read(cx).current().clone(),
                ComponentMask::ALL,
                window,
                cx,
            );
        }
        this.focus_filter(window, cx);
        this
    }

    pub fn find(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(WindowHandle::new(handle.window_id()), cx)
            .map(|frame| frame.read(cx).content().clone())
    }

    pub fn group(&self) -> GroupKey {
        self.group
    }

    pub fn active(&self) -> DetailTab {
        self.active
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn exited_banner_text(&self) -> &str {
        &self.words.exited
    }

    pub fn general(&self) -> &Entity<AppGeneralTab> {
        &self.general
    }

    pub fn processes(&self) -> &Entity<AppProcessesTab> {
        &self.processes
    }

    pub fn filter(&self, tab: DetailTab) -> &Entity<PropsFilter> {
        &self.filters[usize::from(tab == DetailTab::AppProcesses)]
    }

    pub fn tab_text(&self, tab: DetailTab) -> String {
        if tab == DetailTab::AppProcesses {
            let mut count = String::from("[");
            units::count(&mut count, u64::from(self.member_count));
            count.push(']');
            format!("{} {count}", self.words.processes)
        } else {
            self.words.general.to_string()
        }
    }

    pub fn select_tab(&mut self, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        if !TABS.contains(&tab) || tab == self.active {
            return;
        }
        self.active = tab;
        self.sync_processes_tab(cx);
        self.register_demand(window, cx);
        self.focus_filter(window, cx);
        cx.notify();
    }

    /// Tells the Processes tab whether it is active; becoming active collects its members from
    /// the current snapshot. Then takes the label count from the right source.
    fn sync_processes_tab(&mut self, cx: &mut Context<Self>) {
        let active = self.active == DetailTab::AppProcesses;
        let snapshot = self.host.hub().map(|hub| hub.read(cx).current().clone());
        self.processes.update(cx, |tab, cx| {
            tab.set_active(active, snapshot.as_deref(), cx)
        });
        if let Some(snapshot) = snapshot {
            self.sync_member_count(&snapshot, cx);
        }
    }

    /// The label is the unfiltered member count. While the Processes tab is active it is the
    /// length of the tab's member list, so the two agree. While it is not, a count-only scan
    /// gives it, with no member list and no table work. The table row's count is not used: it
    /// follows the main table's filter.
    fn sync_member_count(&mut self, snapshot: &Snapshot, cx: &App) {
        self.member_count = match self.processes.read(cx).member_count() {
            Some(count) => count,
            None => count_members(&snapshot.processes, self.group),
        };
    }

    fn register_demand(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.demand.filter(|_| !self.exited) else {
            return;
        };
        let Some(demand) = self.host.demand().cloned() else {
            return;
        };
        let part = Demand {
            any_surface_visible: true,
            details: Arc::from([DetailDemand {
                target: DetailTarget::App(self.group),
                tab: self.active,
            }]),
            ..Demand::default()
        };
        demand.update(cx, |d, cx| {
            d.set(id, Some(window.window_handle()), part, cx)
        });
    }

    fn remove_demand(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.demand.take() else {
            return;
        };
        if let Some(demand) = self.host.demand().cloned() {
            demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }

    fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = &self.filters[usize::from(self.active == DetailTab::AppProcesses)];
        let input = filter.read(cx).input.clone();
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn take_snapshot(
        &mut self,
        snapshot: &Snapshot,
        mask: ComponentMask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        if mask.intersects(ComponentMask::TABLE) {
            let row = snapshot
                .table
                .groups
                .iter()
                .find(|row| row.key == self.group);
            match row {
                Some(row) => {
                    self.general.update(cx, |g, cx| g.set_group(Some(row), cx));
                }
                None => {
                    self.mark_exited(window, cx);
                    return;
                }
            }
        }
        if mask.intersects(ComponentMask::PROCESSES) {
            self.processes
                .update(cx, |tab, cx| tab.take_snapshot(snapshot, cx));
            self.sync_member_count(snapshot, cx);
        }
        if mask.intersects(ComponentMask::DETAILS) {
            let slot = snapshot
                .details
                .targets
                .get(&DetailTarget::App(self.group))
                .map(|detail| detail.tab(DetailTab::AppGeneral));
            if slot.is_some_and(|slot| slot.validity == Validity::Missing) {
                self.mark_exited(window, cx);
                return;
            }
            let payload = slot.and_then(|slot| slot.payload.as_ref());
            if let Some(TabPayload::AppGeneral(detail)) = payload.map(Arc::as_ref) {
                self.name = SharedString::from(&detail.name);
            }
            self.general.update(cx, |g, cx| g.set_payload(payload, cx));
        }
        cx.notify();
    }

    fn mark_exited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.processes.update(cx, |tab, _| tab.set_exited(true));
        self.general.update(cx, |g, cx| g.set_exited(true, cx));
        self.remove_demand(cx);
        let title: SharedString = self.host.titled(&app_props_title(&self.name, true)).into();
        let frame = window.root::<Root>().flatten().and_then(|root| {
            root.read(cx)
                .view()
                .clone()
                .downcast::<TlSecondaryWindow<Self>>()
                .ok()
        });
        if let Some(frame) = frame {
            frame.update(cx, |f, cx| f.set_title(title, window, cx));
        } else {
            window.set_window_title(&title);
        }
        cx.notify();
    }

    fn on_select_tab(
        &mut self,
        action: &PropsSelectTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = usize::from(action.0)
            .checked_sub(1)
            .and_then(|i| TABS.get(i))
            .copied()
        {
            self.select_tab(tab, window, cx);
        }
    }

    fn on_next_tab(&mut self, _: &PropsNextTab, window: &mut Window, cx: &mut Context<Self>) {
        let tab = if self.active == DetailTab::AppGeneral {
            DetailTab::AppProcesses
        } else {
            DetailTab::AppGeneral
        };
        self.select_tab(tab, window, cx);
    }

    fn on_prev_tab(&mut self, _: &PropsPrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.on_next_tab(&PropsNextTab, window, cx);
    }

    fn on_refresh(&mut self, _: &PropsRefreshNow, _: &mut Window, _: &mut Context<Self>) {
        if self.exited {
            return;
        }
        if let Some(engine) = self.host.client() {
            let command = tl_model::command::Command::DetailRefresh {
                target: DetailTarget::App(self.group),
                tab: self.active,
            };
            if let Err(error) = engine.send(command) {
                warn!(target: "tasklens::ui", ?error, "app properties refresh not sent");
            }
        }
    }

    fn on_escape(&mut self, _: &PropsEscape, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.filters[usize::from(self.active == DetailTab::AppProcesses)].clone();
        let input = filter.read(cx).input.clone();
        let focused_non_empty = {
            let input = input.read(cx);
            input.focus_handle(cx).contains_focused(window, cx)
                && !input.text().to_string().is_empty()
        };
        match esc_step(context_menu_open(window, cx), false, focused_non_empty) {
            EscStep::ClosePopup => close_context_menu(window, cx),
            EscStep::ClearFilter => filter.update(cx, |f, cx| f.clear(window, cx)),
            EscStep::CloseWindow => window.remove_window(),
            EscStep::CancelScan => {}
        }
    }

    fn banner(&self, cx: &App) -> AnyElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let warning = theme.status.warning;
        let mut text = div().text_color(theme.text_primary);
        *text.text_style() = t_sm(&m);
        h_flex()
            .debug_selector(|| "AppPropsExitedBanner".into())
            .h(px(BANNER_HEIGHT * m.ui_scale))
            .w_full()
            .px(m.sp[5])
            .gap(m.sp[3])
            .bg(warning.opacity(0.15))
            .border_b_1()
            .border_color(warning.opacity(0.40))
            .child(
                Icon::new(IconName::CircleOff)
                    .size(px(14. * m.ui_scale))
                    .text_color(warning),
            )
            .child(text.child(self.words.exited.clone()))
            .into_any_element()
    }
}

impl Render for AppPropsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let count = {
            let mut text = String::from("[");
            units::count(&mut text, u64::from(self.member_count));
            text.push(']');
            SharedString::from(text)
        };
        let specs = vec![
            TlTabSpec::new(self.words.general.clone()).tooltip(self.words.general_tip.clone()),
            TlTabSpec::new(self.words.processes.clone())
                .count(count)
                .tooltip(self.words.processes_tip.clone()),
        ];
        let selected = usize::from(self.active == DetailTab::AppProcesses);
        let this = cx.weak_entity();
        let strip = TlTabs::new((
            "app-props-tabs",
            window.window_handle().window_id().as_u64(),
        ))
        .tabs(specs)
        .selected(selected)
        .on_change(move |index, window, cx| {
            if let Some(tab) = TABS.get(index).copied() {
                let _ = this.update(cx, |this, cx| this.select_tab(tab, window, cx));
            }
        });
        let filter = &self.filters[selected];
        let body = if self.active == DetailTab::AppGeneral {
            AnyView::from(self.general.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        } else {
            AnyView::from(self.processes.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        };
        div()
            .id("AppPropsWindow")
            .key_context(PROPS_WINDOW_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_select_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(
                cx.listener(|this, _: &PropsFocusFilter, window, cx| this.focus_filter(window, cx)),
            )
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(|_, _: &PropsCloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(Self::on_escape))
            .on_action(on_clear(filter))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(div().flex_none().w_full().child(strip))
            .child(filter_bar(filter, None, cx))
            .when(self.exited, |root| root.child(self.banner(cx)))
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}
