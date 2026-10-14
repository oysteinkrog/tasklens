//! App Properties and its General tab (UI-PWN-019, PROC-085).
//!
//! The window keeps one app detail demand, switches it with the active tab, and freezes the last
//! values when its group leaves the process table. The General tab formats its seven live usage
//! strings only when their source values change.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem, Context,
    Entity, EntityId, FocusHandle, Focusable as _, FontWeight, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, StyledImage as _, Subscription,
    Window, WindowHandle, canvas, div, img, point, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{AppGeneralDetail, AppKind, DetailTab, DetailTarget, TabPayload};
use tl_model::ids::GroupKey;
use tl_model::process::GroupRow;
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::table::Total;
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;
use tracing::warn;

use super::actions::{
    KV_GRID_CONTEXT, PROPS_WINDOW_CONTEXT, PropsActivateField, PropsCloseWindow,
    PropsCopyAllFields, PropsCopyValue, PropsEscape, PropsFieldDown, PropsFieldEnd, PropsFieldHome,
    PropsFieldLeft, PropsFieldMenu, PropsFieldRight, PropsFieldUp, PropsFocusFilter, PropsNextTab,
    PropsPrevTab, PropsRefreshNow, PropsSelectTab,
};
use super::frame::{EscStep, PwnWindow, app_props_title, esc_step, open_pwn};
use super::props::filter::{FilterChanged, PropsFilter, filter_bar, lowercase, matches, on_clear};
use super::props::general::escape_controls;
use crate::action_tracker::{GlobalActionTracker, ToastTexts};
use crate::app::EngineGlobal;
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
use crate::demand::GlobalDemand;
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, t_sm};
use crate::theme::{current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

gpui_kit::actions!(
    tasklens,
    [
        AppGeneralOpenDesktop,
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

/// Opens App Properties, focuses an existing window, or refuses a ninth app window.
pub fn open_app_properties(group: GroupKey, cx: &mut App) {
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        warn!(target: "tasklens::ui", "no window registry, app properties not opened");
        return;
    };
    let words = AppWords::new(cx);
    let row = cx.try_global::<GlobalHub>().and_then(|hub| {
        hub.0
            .read(cx)
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
            let title = app_props_title(&row.name, false);
            let handle = open_pwn(cx, PwnWindow::AppProperties, title, None, |window, cx| {
                cx.new(|cx| AppPropsWindow::new(group, row, window, cx))
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
    pub fn new(group: GroupKey, filter: Entity<PropsFilter>, cx: &mut Context<Self>) -> Self {
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

    fn rows(&self, panel: usize) -> &[TlKvRow] {
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

    fn set_cursor(&mut self, panel: usize, row: usize, cx: &mut Context<Self>) {
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
        let desktop = self
            .cursor_row()
            .is_some_and(|row| row.label == self.words.labels[2] && row.value.link.is_some());
        if desktop {
            self.open_desktop(window, cx);
        } else {
            self.copy_value(window, cx);
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
                identity.push(self.plain(3, unit.name.clone()));
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
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
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

/// Placeholder child owned by this window. E23-027 adds its table implementation.
pub struct AppProcessesTab;

impl Render for AppProcessesTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// Root entity of one App Properties window.
pub struct AppPropsWindow {
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
    fn new(group: GroupKey, row: GroupRow, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let words = AppWords::new(cx);
        let filters = [
            cx.new(|cx| PropsFilter::new(words.general.clone(), window, cx)),
            cx.new(|cx| PropsFilter::new(words.processes.clone(), window, cx)),
        ];
        let general = cx.new(|cx| AppGeneralTab::new(group, filters[0].clone(), cx));
        general.update(cx, |g, cx| g.set_group(Some(&row), cx));
        let processes = cx.new(|_| AppProcessesTab);
        let mut subs = Vec::new();
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
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
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
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
        self.register_demand(window, cx);
        self.focus_filter(window, cx);
        cx.notify();
    }

    fn register_demand(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.demand.filter(|_| !self.exited) else {
            return;
        };
        let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) else {
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
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
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
                    self.member_count = row.members;
                    self.general.update(cx, |g, cx| g.set_group(Some(row), cx));
                }
                None => {
                    self.mark_exited(window, cx);
                    return;
                }
            }
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
        self.general.update(cx, |g, cx| g.set_exited(true, cx));
        self.remove_demand(cx);
        let title = app_props_title(&self.name, true);
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

    fn on_refresh(&mut self, _: &PropsRefreshNow, _: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        if let Some(engine) = cx.try_global::<EngineGlobal>() {
            let command = tl_model::command::Command::DetailRefresh {
                target: DetailTarget::App(self.group),
                tab: self.active,
            };
            if let Err(error) = engine.0.send(command) {
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
