//! The Environment tab of Process Properties: the process's environment block with secret
//! masking, reveal, copy and Refresh (UI-PWN-016, UI-PWN-018, PROC-083, R-099, ADR-015).
//!
//! Secrets must not leave by accident. A variable whose name contains `TOKEN`, `SECRET`,
//! `PASSWORD`, `PASSWD`, `KEY`, `COOKIE` or `CREDENTIAL` (any case) shows "••••••" until its row
//! is revealed. Every way out of the tab follows the same rule: the cell text, the table's own
//! copy, the row menu copies, Ctrl+C and the filter all see "••••••" (the filter sees the name
//! only) while the row is masked. A masked value is never put in a tooltip or a log. Reveal lasts
//! until the row is hidden again, Refresh is pressed or the window closes; it is never saved.
//! `properties.env_mask = false` turns masking off.
//!
//! The Value column is the table's name column, so its cell can carry the eye button at its
//! right end. The button shows on the hovered and the focus row while it is masked, and stays on
//! a revealed row.
//!
//! The engine reads the block when the tab first opens and on Refresh only.

use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::command::Command;
use tl_model::details::{DetailTab, DetailTarget, EnvDetail, TabPayload, env_name_is_masked};
use tl_model::ids::ProcessKey;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{FilterChanged, NO_MATCH, PropsFilter, lowercase};
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::TlMenu;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::table::actions::{CopyRow, CopyRowsAsTable, ToggleExpand};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};
use crate::windows::actions::PropsToggleRevealEnv;

gpui_kit::actions!(tasklens_env, [EnvCopyName, EnvCopyValue, EnvCopyAll]);

/// What a masked value shows and copies: six U+2022.
pub const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
/// The id of the "(read failed)" row.
pub const READ_FAILED_ROW: u32 = u32::MAX;
/// Footer height, × ui_scale.
const FOOTER_H: f32 = 24.;
/// The eye button: 24 × 24, icon 14.
const EYE_ICON: f32 = 14.;
const REFRESH_ICON: f32 = 12.;
const INFO_ICON: f32 = 14.;

/// True when `name` looks like a secret (R-099): it contains, ignoring case, `TOKEN`,
/// `SECRET`, `PASSWORD`, `PASSWD`, `KEY`, `COOKIE` or `CREDENTIAL`.
pub fn is_secret_name(name: &str) -> bool {
    env_name_is_masked(name)
}

/// The columns, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EnvColumn {
    Name,
    Value,
}

impl EnvColumn {
    pub const ALL: [EnvColumn; 2] = [Self::Name, Self::Value];

    /// Header key and width at 100%. The table has no flex column; Value starts wide.
    fn meta(self) -> (&'static str, f32) {
        match self {
            Self::Name => ("pwn-env-col-name", 240.),
            Self::Value => ("pwn-env-col-value", 480.),
        }
    }
}

impl TableColumn for EnvColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Value => "value",
        }
    }

    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnvSort {
    pub column: EnvColumn,
    pub ascending: bool,
}

impl Default for EnvSort {
    /// Name ascending.
    fn default() -> Self {
        Self {
            column: EnvColumn::Name,
            ascending: true,
        }
    }
}

impl TableSort<EnvColumn> for EnvSort {
    fn column(&self) -> EnvColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The tab's words, translated once per strings version.
pub struct EnvWords {
    strings_version: Option<u64>,
    pub headers: [SharedString; 2],
    pub loading: SharedString,
    pub empty: SharedString,
    pub no_match: SharedString,
    pub unavailable: SharedString,
    pub read_failed: SharedString,
    pub masked_tip: SharedString,
    pub reveal: SharedString,
    pub hide: SharedString,
    pub at_start: SharedString,
    pub refresh: SharedString,
    pub truncated: SharedString,
    pub copy_name: SharedString,
    pub copy_value: SharedString,
    pub copy_all: SharedString,
    pub copied: SharedString,
}

impl EnvWords {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            strings_version: cx.try_global::<Strings>().map(|s| s.strings_version),
            headers: EnvColumn::ALL.map(|c| t(c.meta().0)),
            loading: t("pwn-env-loading"),
            empty: t("pwn-env-empty"),
            no_match: t(NO_MATCH),
            unavailable: t("pwn-env-unavailable"),
            read_failed: t("pwn-env-read-failed"),
            masked_tip: t("pwn-env-masked-tooltip"),
            reveal: t("pwn-env-reveal"),
            hide: t("pwn-env-hide"),
            at_start: t("pwn-env-at-start"),
            refresh: t("pwn-env-refresh"),
            truncated: t("pwn-env-truncated"),
            copy_name: t("pwn-env-menu-copy-name"),
            copy_value: t("pwn-env-menu-copy-value"),
            copy_all: t("pwn-env-menu-copy-all"),
            copied: t("pwn-env-toast-copied"),
        }
    }
}

/// `properties.env_mask`, `true` without the settings global.
fn mask_setting(cx: &App) -> bool {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).properties.env_mask
    } else {
        true
    }
}

/// The errno text of a failed read, such as "Permission denied (os error 13)".
pub fn errno_text(errno: i32) -> SharedString {
    std::io::Error::from_raw_os_error(errno).to_string().into()
}

// ---- The table ----

pub struct EnvDelegate {
    vars: Arc<[(Box<str>, Box<str>)]>,
    /// Per variable: its name looks like a secret. Computed once per payload.
    masked: Arc<[bool]>,
    /// Masking is on (`properties.env_mask`).
    mask_on: bool,
    revealed: FxHashSet<u32>,
    /// The errno of a failed read: one "(read failed)" row.
    read_errno: Option<i32>,
    order: Vec<u32>,
    filtered: Option<Arc<[u32]>>,
    view: Vec<u32>,
    index: FxHashMap<u32, u32>,
    loading: bool,
    exited: bool,
    /// The hovered and the focus row, for the eye button.
    hot: [Option<u32>; 2],
    menu_row: Option<u32>,
    version: u64,
    sort: EnvSort,
    specs: Vec<ColumnSpec<EnvColumn>>,
    layout: ColumnLayout<EnvColumn>,
    words: Rc<EnvWords>,
    masked_tip: Option<AnyView>,
    read_failed_tip: Option<AnyView>,
    /// `text_secondary`, for the "(read failed)" name.
    secondary: Hsla,
    tab: WeakEntity<EnvironTab>,
}

impl EnvDelegate {
    fn new(
        words: Rc<EnvWords>,
        mask_on: bool,
        secondary: Hsla,
        tab: WeakEntity<EnvironTab>,
    ) -> Self {
        let specs = specs(&words);
        let layout =
            ColumnLayout::defaults(TableId::Processes, &specs, &EnvColumn::ALL, &EnvColumn::ALL);
        Self {
            vars: Arc::from([]),
            masked: Arc::from([]),
            mask_on,
            revealed: FxHashSet::default(),
            read_errno: None,
            order: Vec::new(),
            filtered: None,
            view: Vec::new(),
            index: FxHashMap::default(),
            loading: true,
            exited: false,
            hot: [None; 2],
            menu_row: None,
            version: 0,
            sort: EnvSort::default(),
            specs,
            layout,
            words,
            masked_tip: None,
            read_failed_tip: None,
            secondary,
            tab,
        }
    }

    /// Variable `i` shows "••••••" now.
    pub fn is_masked(&self, i: u32) -> bool {
        self.mask_on
            && self.masked.get(i as usize).copied().unwrap_or(false)
            && !self.revealed.contains(&i)
    }

    /// Variable `i` can be revealed or hidden.
    pub fn is_secret(&self, i: u32) -> bool {
        self.mask_on && self.masked.get(i as usize).copied().unwrap_or(false)
    }

    pub fn is_revealed(&self, i: u32) -> bool {
        self.revealed.contains(&i)
    }

    /// The payload row of `name`.
    pub fn row_named(&self, name: &str) -> Option<u32> {
        self.vars
            .iter()
            .position(|(n, _)| &**n == name)
            .map(|i| i as u32)
    }

    /// The row ids shown, in order.
    pub fn view_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.view.iter().copied()
    }

    /// The value as shown and copied: "••••••" while masked.
    pub fn shown_value(&self, i: u32) -> &str {
        if i == READ_FAILED_ROW {
            return "";
        }
        if self.is_masked(i) {
            return MASK;
        }
        self.vars.get(i as usize).map_or("", |(_, v)| v)
    }

    pub fn name(&self, i: u32) -> &str {
        if i == READ_FAILED_ROW {
            return &self.words.read_failed;
        }
        self.vars.get(i as usize).map_or("", |(n, _)| n)
    }

    /// `NAME=value` with the mask rule.
    pub fn line(&self, i: u32) -> String {
        format!("{}={}", self.name(i), self.shown_value(i))
    }

    /// The variable count: the tab count.
    pub fn count(&self) -> usize {
        self.vars.len()
    }

    fn set_detail(&mut self, d: &EnvDetail) {
        self.vars = d.vars.clone();
        self.masked = d.vars.iter().map(|(n, _)| is_secret_name(n)).collect();
        self.read_errno = d.read_errno;
        self.revealed.clear();
        self.loading = false;
        self.filtered = None;
        self.resort();
    }

    fn compare(&self, a: u32, b: u32) -> Ordering {
        let by = match self.sort.column {
            EnvColumn::Name => Ordering::Equal,
            // Masked values sort as "••••••", so the order says nothing about them.
            EnvColumn::Value => self.shown_value(a).cmp(self.shown_value(b)),
        };
        by.then_with(|| self.name(a).cmp(self.name(b)))
    }

    fn resort(&mut self) {
        let mut order: Vec<u32> = (0..self.vars.len() as u32).collect();
        order.sort_by(|&a, &b| {
            let o = self.compare(a, b);
            if self.sort.ascending { o } else { o.reverse() }
        });
        self.order = order;
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        self.view.clear();
        if self.read_errno.is_some() {
            self.view.push(READ_FAILED_ROW);
        } else {
            let keep = self.filtered.as_ref().map(|f| {
                let mut shown = vec![false; self.vars.len()];
                for &i in f.iter() {
                    if let Some(s) = shown.get_mut(i as usize) {
                        *s = true;
                    }
                }
                shown
            });
            self.view.extend(
                self.order
                    .iter()
                    .copied()
                    .filter(|&i| keep.as_ref().is_none_or(|k| k[i as usize])),
            );
        }
        self.index.clear();
        for (pos, &i) in self.view.iter().enumerate() {
            self.index.insert(i, pos as u32);
        }
        self.version += 1;
    }

    /// The eye button for row `i`, or `None`.
    fn eye(&self, i: u32) -> Option<AnyElement> {
        if !self.is_secret(i) || self.exited {
            return None;
        }
        let revealed = self.is_revealed(i);
        if !revealed && !self.hot.contains(&Some(i)) {
            return None;
        }
        let (glyph, name) = if revealed {
            (TlIcon::EyeOff, self.words.hide.clone())
        } else {
            (TlIcon::Eye, self.words.reveal.clone())
        };
        let tab = self.tab.clone();
        Some(
            TlButton::new(("env-eye", i as usize), TlButtonKind::Icon)
                .size(TlButtonSize::Sm)
                .icon(glyph)
                .icon_size(px(EYE_ICON))
                .tooltip(name)
                .tab_stop(false)
                .on_click(move |_, _, cx| {
                    let _ = tab.update(cx, |t, cx| t.toggle_reveal(i, cx));
                })
                .into_any_element(),
        )
    }
}

fn specs(words: &EnvWords) -> Vec<ColumnSpec<EnvColumn>> {
    EnvColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align: ColumnAlign::Left,
                kind: ColumnKind::Text,
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

impl TreeTableDelegate for EnvDelegate {
    type RowId = u32;
    type Column = EnvColumn;
    type Sort = EnvSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        self.view[ix]
    }
    fn row_index(&self, id: u32) -> Option<usize> {
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
    fn columns(&self) -> &ColumnLayout<EnvColumn> {
        &self.layout
    }
    fn column_spec(&self, id: EnvColumn) -> &ColumnSpec<EnvColumn> {
        let i = EnvColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    /// The Name cell; the Value cell is the name cell. Its text is the masked form too, for the
    /// table's own copy.
    fn cell(&self, ix: usize, col: EnvColumn, out: &mut CellOut) {
        let i = self.view[ix];
        let text = match col {
            EnvColumn::Name => self.name(i),
            EnvColumn::Value => self.shown_value(i),
        };
        out.bits = (u64::from(i) << 1) | u64::from(self.is_masked(i));
        out.validity = Validity::Valid;
        if i == READ_FAILED_ROW && col == EnvColumn::Name {
            out.text_color = Some(self.secondary);
        }
        if out.want_text {
            out.text.push_str(text);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let i = self.view[ix];
        let masked = self.is_masked(i);
        let value = self.shown_value(i);
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: if masked {
                SharedString::default()
            } else {
                value.to_owned().into()
            },
            count_run: None,
            // `text_secondary`.
            secondary: masked.then(|| SharedString::new_static(MASK)),
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: self.eye(i),
        }
    }

    fn name_column(&self) -> Option<EnvColumn> {
        Some(EnvColumn::Value)
    }

    fn header_total(&self, _: EnvColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> EnvSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    fn context_menu(
        &mut self,
        targets: &[u32],
        _: Option<EnvColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let w = self.words.clone();
        self.menu_row = targets.first().copied();
        let real = self.menu_row.filter(|&i| i != READ_FAILED_ROW);
        let mut menu = menu
            .item(w.copy_name.clone(), Box::new(EnvCopyName))
            .item(w.copy_value.clone(), Box::new(EnvCopyValue))
            .item(w.copy_all.clone(), Box::new(EnvCopyAll));
        if let Some(i) = real.filter(|&i| self.is_secret(i)) {
            let label = if self.is_revealed(i) {
                &w.hide
            } else {
                &w.reveal
            };
            menu = menu.separator();
            menu = if self.exited {
                menu.item_disabled(label.clone(), None)
            } else {
                menu.item(label.clone(), Box::new(PropsToggleRevealEnv))
            };
        }
        menu
    }

    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    fn on_sort(&mut self, col: EnvColumn, cx: &mut Context<TlTreeTable<Self>>) {
        self.sort = EnvSort {
            column: col,
            ascending: self.sort.column != col || !self.sort.ascending,
        };
        self.resort();
        cx.notify();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<EnvColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    /// Masked rows: "Hidden because the name looks like a secret"; the read-failed row: the
    /// errno text. Never a value.
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        let i = self.view[ix];
        if i == READ_FAILED_ROW {
            return self.read_failed_tip.clone();
        }
        self.is_masked(i).then(|| self.masked_tip.clone()).flatten()
    }

    fn empty_text(&self) -> Option<SharedString> {
        let w = &self.words;
        Some(if self.loading {
            w.loading.clone()
        } else if self.vars.is_empty() {
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

// ---- The tab ----

/// The tab count changed: the variable count.
pub struct EnvCountChanged(pub u32);

pub struct EnvironTab {
    key: ProcessKey,
    table: Entity<TlTreeTable<EnvDelegate>>,
    filter: Entity<PropsFilter>,
    /// Lowercase haystacks per variable: the name, plus the value when it is not masked.
    haystacks: Arc<[Box<str>]>,
    last_payload: Option<Arc<TabPayload>>,
    truncated: bool,
    unavailable: bool,
    /// Refresh was pressed and no new payload has come yet.
    refreshing: bool,
    exited: bool,
    words: Rc<EnvWords>,
    weak: WeakEntity<Self>,
    _subs: SmallVec<[Subscription; 6]>,
}

impl EventEmitter<EnvCountChanged> for EnvironTab {}

fn toast(window: &mut Window, cx: &mut App, text: SharedString) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
    }
}

impl EnvironTab {
    pub fn new(
        key: ProcessKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(EnvWords::new(cx));
        let weak = cx.entity().downgrade();
        let mut delegate = EnvDelegate::new(
            words.clone(),
            mask_setting(cx),
            current(cx).text_secondary,
            weak.clone(),
        );
        delegate.masked_tip = Some(TlTooltip::text(words.masked_tip.clone()).view(cx));
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
                    toast(window, cx, this.words.copied.clone());
                }
            },
        ));
        // The eye button follows the hovered and the focus row.
        subs.push(cx.observe(&table, |_, table, cx| {
            let hot = {
                let t = table.read(cx);
                let hover = t.hover_row().map(|ix| t.delegate.row_id(ix));
                [hover, t.selection.focus()]
            };
            if table.read(cx).delegate.hot != hot {
                table.update(cx, |t, cx| {
                    t.delegate_mut().hot = hot;
                    cx.notify();
                });
            }
        }));
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = cx.try_global::<Strings>().map(|s| s.strings_version);
            if this.words.strings_version != version {
                this.words = Rc::new(EnvWords::new(cx));
                let words = this.words.clone();
                let tip = TlTooltip::text(words.masked_tip.clone()).view(cx);
                this.table.update(cx, |t, cx| {
                    let d = t.delegate_mut();
                    d.specs = specs(&words);
                    d.words = words;
                    d.masked_tip = Some(tip);
                    d.version += 1;
                    t.on_new_version(cx);
                });
            }
        }));
        if cx.has_global::<TlSettings>() {
            subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
                let on = mask_setting(cx);
                this.set_mask(on, cx);
            }));
        }
        Self {
            key,
            table,
            filter,
            haystacks: Arc::from([]),
            last_payload: None,
            truncated: false,
            unavailable: false,
            refreshing: false,
            exited: false,
            words,
            weak,
            _subs: subs,
        }
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn table(&self) -> &Entity<TlTreeTable<EnvDelegate>> {
        &self.table
    }

    pub fn filter(&self) -> &Entity<PropsFilter> {
        &self.filter
    }

    pub fn words(&self) -> &Rc<EnvWords> {
        &self.words
    }

    /// The tab count: the variable count.
    pub fn count(&self, cx: &App) -> u32 {
        self.table.read(cx).delegate.count() as u32
    }

    /// Refresh is waiting for the next payload.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Takes the tab's payload. The same `Arc` again does nothing; while exited it stays frozen.
    /// A new payload masks every row again.
    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let same = match (&self.last_payload, payload) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let Some(TabPayload::Environ(detail)) = payload.map(|p| &**p) else {
            return;
        };
        self.last_payload = payload.cloned();
        let detail = detail.clone();
        self.truncated = detail.truncated;
        self.unavailable = detail.unavailable;
        self.refreshing = false;
        let tip = detail
            .read_errno
            .map(|e| TlTooltip::text(errno_text(e)).view(cx));
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.set_detail(&detail);
            d.read_failed_tip = tip;
            t.on_new_version(cx);
        });
        self.rebuild_haystacks(cx);
        let n = self.count(cx);
        cx.emit(EnvCountChanged(n));
        cx.notify();
    }

    /// The haystacks from the current mask state; the filter runs again when it has text.
    fn rebuild_haystacks(&mut self, cx: &mut Context<Self>) {
        let d = &self.table.read(cx).delegate;
        self.haystacks = (0..d.vars.len() as u32)
            .map(|i| {
                let (name, value) = &d.vars[i as usize];
                let mut hay = String::with_capacity(name.len() + value.len() + 1);
                hay.push_str(name);
                if !d.is_masked(i) {
                    hay.push(' ');
                    hay.push_str(value);
                }
                lowercase(&hay).into_boxed_str()
            })
            .collect();
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        }
    }

    /// Reveals or hides secret row `i`.
    pub fn toggle_reveal(&mut self, i: u32, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let changed = self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            if !d.is_secret(i) {
                return false;
            }
            if !d.revealed.remove(&i) {
                d.revealed.insert(i);
            }
            if d.sort.column == EnvColumn::Value {
                d.resort();
            } else {
                d.version += 1;
            }
            t.on_new_version(cx);
            true
        });
        if changed {
            self.rebuild_haystacks(cx);
            cx.notify();
        }
    }

    /// `properties.env_mask` changed (here or in another window).
    pub fn set_mask(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate.mask_on == on {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.mask_on = on;
            d.revealed.clear();
            d.resort();
            t.on_new_version(cx);
        });
        self.rebuild_haystacks(cx);
        cx.notify();
    }

    /// Refresh: asks the engine to read the block again and masks every row. The button shows
    /// loading until the next payload.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            if !d.revealed.is_empty() {
                d.revealed.clear();
                d.resort();
                t.on_new_version(cx);
            }
        });
        self.rebuild_haystacks(cx);
        self.refreshing = true;
        let cmd = Command::DetailRefresh {
            target: DetailTarget::Process(self.key),
            tab: DetailTab::Environment,
        };
        match cx.try_global::<EngineGlobal>() {
            Some(engine) => {
                if let Err(err) = engine.0.send(cmd) {
                    warn!(target: "tasklens::ui", ?err, "environment refresh not sent");
                }
            }
            None => warn!(target: "tasklens::ui", "no engine, environment refresh not sent"),
        }
        cx.notify();
    }

    /// Freezes the rows and disables Refresh and reveal once the process exited.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            self.table
                .update(cx, |t, _| t.delegate_mut().exited = exited);
            cx.notify();
        }
    }

    /// The filter bar tools: the "Environment at process start" note and Refresh.
    pub fn filter_tool(&self, cx: &App) -> AnyElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let mut note = div().text_color(theme.text_secondary);
        *note.text_style() = t_sm(&m);
        let weak = self.weak.clone();
        h_flex()
            .items_center()
            .gap(m.sp[4])
            .child(
                h_flex()
                    .items_center()
                    .gap(m.sp[2])
                    .child(icon(
                        TlIcon::Info,
                        px(INFO_ICON * m.ui_scale),
                        theme.text_secondary,
                    ))
                    .child(note.child(self.words.at_start.clone())),
            )
            .child(
                TlButton::new("EnvRefresh", TlButtonKind::Ghost)
                    .size(TlButtonSize::Sm)
                    .icon(TlIcon::RotateCcw)
                    .icon_size(px(REFRESH_ICON * m.ui_scale))
                    .label(self.words.refresh.clone())
                    .loading(self.refreshing)
                    .disabled(self.exited)
                    .on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |t, cx| t.refresh(cx));
                    }),
            )
            .into_any_element()
    }

    fn run_filter(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.filter.read(cx).is_current(generation) {
            return;
        }
        if self.filter.read(cx).needle().is_empty() {
            self.apply_filter(generation, None, cx);
            return;
        }
        let rows = self.haystacks.clone();
        let task = self.filter.update(cx, |f, cx| f.run_filter(rows, cx));
        cx.spawn(async move |this, cx| {
            if let Some((generation, keep)) = task.await {
                let _ = this.update(cx, |this, cx| this.apply_filter(generation, Some(keep), cx));
            }
        })
        .detach();
    }

    fn apply_filter(&mut self, generation: u64, keep: Option<Arc<[u32]>>, cx: &mut Context<Self>) {
        if !self.filter.update(cx, |f, _| f.accept(generation)) {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.filtered = keep;
            d.rebuild_view();
            t.on_new_version(cx);
        });
    }

    /// Copy as table: the table would copy the Value column's primary text, which is empty
    /// while masked, so the tab writes the rows itself with "••••••".
    fn on_copy_as_table(
        &mut self,
        _: &CopyRowsAsTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = {
            let t = self.table.read(cx);
            let d = &t.delegate;
            let mut ids: Vec<(usize, u32)> = t
                .selection
                .ids()
                .filter_map(|i| d.row_index(i).map(|ix| (ix, i)))
                .collect();
            if ids.is_empty() {
                return;
            }
            ids.sort_unstable();
            let mut text = self.words.headers.join("\t");
            for &(_, i) in &ids {
                text.push('\n');
                text.push_str(d.name(i));
                text.push('\t');
                text.push_str(d.shown_value(i));
            }
            text
        };
        cx.stop_propagation();
        self.copy(text, window, cx);
    }

    fn copy(&self, text: String, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, self.words.copied.clone());
    }

    /// Ctrl+C: the selected rows as `NAME=value` lines in table order, masked values masked.
    fn on_copy_rows(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let t = self.table.read(cx);
            let d = &t.delegate;
            let mut ids: Vec<(usize, u32)> = t
                .selection
                .ids()
                .filter_map(|i| d.row_index(i).map(|ix| (ix, i)))
                .collect();
            ids.sort_unstable();
            ids.iter()
                .map(|&(_, i)| d.line(i))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if !text.is_empty() {
            self.copy(text, window, cx);
        }
    }

    fn on_copy_name(&mut self, _: &EnvCopyName, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let d = &self.table.read(cx).delegate;
            d.menu_row.map(|i| d.name(i).to_owned())
        };
        if let Some(text) = text {
            self.copy(text, window, cx);
        }
    }

    fn on_copy_value(&mut self, _: &EnvCopyValue, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let d = &self.table.read(cx).delegate;
            d.menu_row.map(|i| d.shown_value(i).to_owned())
        };
        if let Some(text) = text {
            self.copy(text, window, cx);
        }
    }

    /// Copy all as NAME=value: every row passing the filter, in table order.
    fn on_copy_all(&mut self, _: &EnvCopyAll, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let d = &self.table.read(cx).delegate;
            d.view
                .iter()
                .filter(|&&i| i != READ_FAILED_ROW)
                .map(|&i| d.line(i))
                .collect::<Vec<_>>()
                .join("\n")
        };
        self.copy(text, window, cx);
    }

    /// The menu item, or Space on the focus row.
    fn on_toggle_reveal(
        &mut self,
        _: &PropsToggleRevealEnv,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = {
            let t = self.table.read(cx);
            t.delegate.menu_row.or_else(|| t.selection.focus())
        };
        if let Some(i) = row {
            self.toggle_reveal(i, cx);
        }
    }
}

impl Render for EnvironTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let root = v_flex()
            .debug_selector(|| "PropsEnvironment".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_rows))
            .on_action(cx.listener(Self::on_copy_name))
            .on_action(cx.listener(Self::on_copy_value))
            .on_action(cx.listener(Self::on_copy_all))
            .on_action(cx.listener(Self::on_toggle_reveal))
            // Space on a secret row reveals it; the table's own Space toggles a tree row, and
            // these rows have none.
            .capture_action(cx.listener(Self::on_copy_as_table))
            .capture_action(cx.listener(|this, _: &ToggleExpand, _, cx| {
                let focus = {
                    let t = this.table.read(cx);
                    t.selection.focus().filter(|&i| t.delegate.is_secret(i))
                };
                if let Some(i) = focus {
                    this.toggle_reveal(i, cx);
                    cx.stop_propagation();
                }
            }));
        if self.unavailable {
            let mut text = div()
                .debug_selector(|| "EnvUnavailable".into())
                .text_color(theme.text_secondary);
            *text.text_style() = t_base(&m);
            return root
                .items_center()
                .justify_center()
                .child(text.child(self.words.unavailable.clone()));
        }
        root.child(div().flex_1().min_h_0().child(self.table.clone()))
            .when(self.truncated, |d| {
                let mut text = div().text_color(theme.text_secondary);
                *text.text_style() = t_sm(&m);
                d.child(
                    div()
                        .debug_selector(|| "EnvTruncated".into())
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(FOOTER_H * m.ui_scale))
                        .px(m.sp[5])
                        .border_t_1()
                        .border_color(theme.divider)
                        .child(text.child(self.words.truncated.clone())),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_masks_patterns_case_insensitive() {
        for name in [
            "api_key",
            "GH_TOKEN",
            "MyPassword",
            "SESSION_COOKIE",
            "db_passwd",
        ] {
            assert!(is_secret_name(name), "{name}");
        }
        for name in ["PATH", "HOME", "LANG", "XDG_SESSION_TYPE"] {
            assert!(!is_secret_name(name), "{name}");
        }
        assert!(is_secret_name("aws_Secret_access"));
        assert!(is_secret_name("GIT_CREDENTIAL_HELPER"));
    }
}
