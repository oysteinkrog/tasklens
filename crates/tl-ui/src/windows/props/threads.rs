//! The Threads tab of Process Properties: a flat table of the process's threads with fixed
//! columns, sorting, the tab filter, selection by TID and copy (UI-PWN-010, PROC-079, R-097).
//!
//! `PropsWindow` feeds the tab its payload, the online CPUs and the exited state. The rows are
//! sorted again only when the payload or the sort changes, and filtered again only when the
//! payload or the filter text changes. Cell text goes through the table's `CellTextCache`, keyed
//! by TID, column and value bits, so an unchanged cell is not formatted again.
//!
//! The words are `en-US.ftl` keys, translated once per strings version into [`ThreadWords`].
//! Thread names are sanitized by the collector and drawn in the Name cell, which marks U+FFFD
//! (UI-FND-053).

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyView, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    px,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::SchedPolicy;
use tl_model::details::{TabPayload, ThreadRow};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcState;
use tl_model::system::CpuSet;
use tl_model::units;
use tl_model::validity::Validity;

use super::filter::{FilterChanged, NO_MATCH_THREADS, PropsFilter, lowercase};
use crate::components::menu::TlMenu;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::table::actions::CopyRow;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};

/// The columns of the Threads table, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ThreadColumn {
    Tid,
    Name,
    State,
    Cpu,
    UserTime,
    KernelTime,
    Priority,
    BasePriority,
    Affinity,
    LastCpu,
    CtxSwitches,
    WaitChannel,
}

impl ThreadColumn {
    pub const ALL: [ThreadColumn; 12] = [
        Self::Tid,
        Self::Name,
        Self::State,
        Self::Cpu,
        Self::UserTime,
        Self::KernelTime,
        Self::Priority,
        Self::BasePriority,
        Self::Affinity,
        Self::LastCpu,
        Self::CtxSwitches,
        Self::WaitChannel,
    ];

    /// Header key, width at 100%, alignment, and the header tooltip key.
    fn meta(self) -> (&'static str, f32, ColumnAlign, Option<&'static str>) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Tid => ("pwn-threads-col-tid", 80., Right, None),
            Self::Name => ("pwn-threads-col-name", 160., Left, None),
            Self::State => ("pwn-threads-col-state", 96., Left, None),
            Self::Cpu => ("pwn-threads-col-cpu", 70., Right, None),
            Self::UserTime => (
                "pwn-threads-col-utime",
                104.,
                Right,
                Some("pwn-threads-tip-utime"),
            ),
            Self::KernelTime => (
                "pwn-threads-col-stime",
                112.,
                Right,
                Some("pwn-threads-tip-stime"),
            ),
            Self::Priority => ("pwn-threads-col-prio", 100., Left, None),
            Self::BasePriority => ("pwn-threads-col-base", 100., Right, None),
            Self::Affinity => ("pwn-threads-col-aff", 110., Left, None),
            Self::LastCpu => ("pwn-threads-col-lastcpu", 72., Right, None),
            Self::CtxSwitches => (
                "pwn-threads-col-ctx",
                140.,
                Right,
                Some("pwn-threads-tip-ctx"),
            ),
            Self::WaitChannel => (
                "pwn-threads-col-wchan",
                160.,
                Left,
                Some("pwn-threads-tip-wchan"),
            ),
        }
    }
}

impl TableColumn for ThreadColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Tid => "tid",
            Self::Name => "name",
            Self::State => "state",
            Self::Cpu => "cpu",
            Self::UserTime => "utime",
            Self::KernelTime => "stime",
            Self::Priority => "prio",
            Self::BasePriority => "base",
            Self::Affinity => "aff",
            Self::LastCpu => "lastcpu",
            Self::CtxSwitches => "ctx",
            Self::WaitChannel => "wchan",
        }
    }

    /// The Threads layout is never saved, so no stored key maps to it.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }
}

/// The sort of the Threads table, kept for the window's life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ThreadSort {
    pub column: ThreadColumn,
    pub ascending: bool,
}

impl Default for ThreadSort {
    /// Thread ID ascending.
    fn default() -> Self {
        Self {
            column: ThreadColumn::Tid,
            ascending: true,
        }
    }
}

impl TableSort<ThreadColumn> for ThreadSort {
    fn column(&self) -> ThreadColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The tab's words, translated once per strings version.
pub struct ThreadWords {
    strings_version: Option<u64>,
    /// Header labels in [`ThreadColumn::ALL`] order.
    pub headers: [SharedString; 12],
    /// Header tooltips in [`ThreadColumn::ALL`] order.
    pub tips: [Option<SharedString>; 12],
    pub loading: SharedString,
    pub empty: SharedString,
    pub no_match: SharedString,
    /// Running, Sleeping, Disk sleep, Stopped, Zombie, Idle, Unknown.
    pub states: [SharedString; 7],
    pub variable: SharedString,
    /// "Realtime {$prio}".
    pub realtime: Template,
    pub all_cpus: SharedString,
    pub restricted: SharedString,
    pub copied: SharedString,
}

const KEY_STATES: [&str; 7] = [
    "pwn-threads-state-running",
    "pwn-threads-state-sleeping",
    "pwn-threads-state-disk-sleep",
    "pwn-threads-state-stopped",
    "pwn-threads-state-zombie",
    "pwn-threads-state-idle",
    "pwn-threads-state-unknown",
];

impl ThreadWords {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            strings_version: strings_version(cx),
            headers: ThreadColumn::ALL.map(|c| t(c.meta().0)),
            tips: ThreadColumn::ALL.map(|c| c.meta().3.map(t)),
            loading: t("pwn-threads-loading"),
            empty: t("pwn-threads-empty"),
            no_match: t(NO_MATCH_THREADS),
            states: KEY_STATES.map(t),
            variable: t("pwn-threads-prio-variable"),
            realtime: Template::new(cx, "pwn-threads-prio-realtime", &["prio"]),
            all_cpus: t("pwn-threads-affinity-all"),
            restricted: t("pwn-threads-restricted"),
            copied: t("pwn-threads-copied"),
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

fn state_index(state: ProcState) -> usize {
    match state {
        ProcState::Running => 0,
        ProcState::Sleeping => 1,
        ProcState::DiskSleep => 2,
        ProcState::Stopped => 3,
        ProcState::Zombie => 4,
        ProcState::Idle => 5,
        ProcState::Other => 6,
    }
}

/// The kernel name of `policy`, for the Priority tooltip. Not translated.
pub fn policy_name(policy: SchedPolicy) -> &'static str {
    match policy {
        SchedPolicy::Other => "SCHED_OTHER",
        SchedPolicy::Batch => "SCHED_BATCH",
        SchedPolicy::Idle => "SCHED_IDLE",
        SchedPolicy::Fifo => "SCHED_FIFO",
        SchedPolicy::Rr => "SCHED_RR",
    }
}

fn is_realtime(policy: SchedPolicy) -> bool {
    matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr)
}

/// `Variable` for the time-sharing policies, `Realtime 50` for FIFO and RR.
pub fn fmt_priority(out: &mut String, words: &ThreadWords, policy: SchedPolicy, rt_prio: u8) {
    if is_realtime(policy) {
        let _ = words.realtime.write(out, &[&rt_prio]);
    } else {
        out.push_str(&words.variable);
    }
}

/// The base priority: rt_priority for FIFO and RR, else nice.
fn base_priority(r: &ThreadRow) -> i32 {
    if is_realtime(r.policy) {
        i32::from(r.rt_prio)
    } else {
        i32::from(r.nice)
    }
}

/// `1 204 / 33`: voluntary and involuntary switches, grouped with thin spaces.
pub fn fmt_ctx(out: &mut String, voluntary: u64, involuntary: u64) {
    units::count(out, voluntary);
    out.push_str(" / ");
    units::count(out, involuntary);
}

/// The wait channel symbol, `-` for `0` or none.
pub fn fmt_wchan(out: &mut String, wchan: &str) {
    match wchan.trim() {
        "" | "0" => out.push('-'),
        w => out.push_str(w),
    }
}

/// `0-7,16-23` from `set`.
fn cpu_list(out: &mut String, set: &CpuSet) {
    let mut run: Option<(u16, u16)> = None;
    let mut first = true;
    let mut flush = |out: &mut String, (a, b): (u16, u16)| {
        if !first {
            out.push(',');
        }
        first = false;
        if a == b {
            let _ = write!(out, "{a}");
        } else {
            let _ = write!(out, "{a}-{b}");
        }
    };
    for cpu in 0..CpuSet::MAX_CPUS {
        if !set.contains(cpu) {
            continue;
        }
        run = match run {
            Some((a, b)) if b + 1 == cpu => Some((a, cpu)),
            Some(r) => {
                flush(out, r);
                Some((cpu, cpu))
            }
            None => Some((cpu, cpu)),
        };
    }
    if let Some(r) = run {
        flush(out, r);
    }
}

/// Orders rows `a` and `b` by `col`, then by TID.
fn compare(a: &ThreadRow, b: &ThreadRow, col: ThreadColumn) -> Ordering {
    let by = match col {
        ThreadColumn::Tid => Ordering::Equal,
        ThreadColumn::Name => a.comm.cmp(&b.comm),
        ThreadColumn::State => state_index(a.state).cmp(&state_index(b.state)),
        ThreadColumn::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
        ThreadColumn::UserTime => a.utime_ms.cmp(&b.utime_ms),
        ThreadColumn::KernelTime => a.stime_ms.cmp(&b.stime_ms),
        ThreadColumn::Priority => {
            (is_realtime(a.policy), a.rt_prio).cmp(&(is_realtime(b.policy), b.rt_prio))
        }
        ThreadColumn::BasePriority => base_priority(a).cmp(&base_priority(b)),
        ThreadColumn::Affinity => a.cpus.cmp(&b.cpus),
        ThreadColumn::LastCpu => a.last_cpu.cmp(&b.last_cpu),
        ThreadColumn::CtxSwitches => (a.vol_ctx + a.invol_ctx).cmp(&(b.vol_ctx + b.invol_ctx)),
        ThreadColumn::WaitChannel => a.wchan.cmp(&b.wchan),
    };
    by.then(a.tid.cmp(&b.tid))
}

/// The Threads table's rows, columns and sort.
pub struct ThreadsDelegate {
    /// The payload rows, in payload order.
    rows: Arc<[ThreadRow]>,
    /// Row indexes in sort order.
    order: Vec<u32>,
    /// Row indexes the filter keeps, `None` for all.
    filtered: Option<Arc<[u32]>>,
    /// Row indexes shown, in sort order.
    view: Vec<u32>,
    /// TID to position in `view`.
    index: FxHashMap<u32, u32>,
    /// True before the first payload.
    loading: bool,
    version: u64,
    sort: ThreadSort,
    specs: Vec<ColumnSpec<ThreadColumn>>,
    layout: ColumnLayout<ThreadColumn>,
    /// The online CPUs as a list, for the `All` affinity form. Empty until known.
    online: String,
    words: Rc<ThreadWords>,
}

impl ThreadsDelegate {
    fn new(words: Rc<ThreadWords>) -> Self {
        let specs = specs(&words);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &ThreadColumn::ALL,
            &ThreadColumn::ALL,
        );
        Self {
            rows: Arc::from([]),
            order: Vec::new(),
            filtered: None,
            view: Vec::new(),
            index: FxHashMap::default(),
            loading: true,
            version: 0,
            sort: ThreadSort::default(),
            specs,
            layout,
            online: String::new(),
            words,
        }
    }

    /// The payload rows, in payload order.
    pub fn rows(&self) -> &Arc<[ThreadRow]> {
        &self.rows
    }

    /// The TIDs shown, in view order.
    pub fn view_tids(&self) -> impl Iterator<Item = u32> + '_ {
        self.view.iter().map(|&i| self.rows[i as usize].tid)
    }

    /// The text the `col` cell of thread `tid` shows, `None` when the thread is not shown.
    pub fn cell_text(&self, tid: u32, col: ThreadColumn) -> Option<String> {
        let ix = self.row_index(tid)?;
        let mut out = String::new();
        self.write_text(self.row(ix), col, &mut out);
        Some(out)
    }

    fn row(&self, ix: usize) -> &ThreadRow {
        &self.rows[self.view[ix] as usize]
    }

    fn set_rows(&mut self, rows: Arc<[ThreadRow]>) {
        self.rows = rows;
        self.loading = false;
        self.filtered = None;
        self.resort();
    }

    fn set_filtered(&mut self, filtered: Option<Arc<[u32]>>) {
        self.filtered = filtered;
        self.rebuild_view();
    }

    fn set_words(&mut self, words: Rc<ThreadWords>) {
        self.specs = specs(&words);
        self.words = words;
        self.version += 1;
    }

    /// Sorts the row indexes by the current sort, then rebuilds the view.
    fn resort(&mut self) {
        let rows = &self.rows;
        let ThreadSort { column, ascending } = self.sort;
        self.order.clear();
        self.order.extend(0..rows.len() as u32);
        self.order.sort_unstable_by(|&a, &b| {
            let o = compare(&rows[a as usize], &rows[b as usize], column);
            if ascending { o } else { o.reverse() }
        });
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        self.view.clear();
        match &self.filtered {
            None => self.view.extend_from_slice(&self.order),
            Some(keep) => {
                let mut shown = vec![false; self.rows.len()];
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
        for (pos, &i) in self.view.iter().enumerate() {
            self.index.insert(self.rows[i as usize].tid, pos as u32);
        }
        self.version += 1;
    }

    /// The display text of `col` for row `r`, as the cell shows it.
    fn write_text(&self, r: &ThreadRow, col: ThreadColumn, out: &mut String) {
        let w = &self.words;
        match col {
            ThreadColumn::Tid => {
                let _ = write!(out, "{}", r.tid);
            }
            ThreadColumn::Name => out.push_str(&r.comm),
            ThreadColumn::State => out.push_str(&w.states[state_index(r.state)]),
            ThreadColumn::Cpu => {
                let _ = write!(out, "{:.1}", r.cpu_pct.max(0.0));
            }
            ThreadColumn::UserTime => units::count(out, r.utime_ms),
            ThreadColumn::KernelTime => units::count(out, r.stime_ms),
            ThreadColumn::Priority => fmt_priority(out, w, r.policy, r.rt_prio),
            ThreadColumn::BasePriority => {
                let _ = write!(out, "{}", base_priority(r));
            }
            ThreadColumn::Affinity => {
                if !self.online.is_empty() && *r.cpus == *self.online {
                    out.push_str(&w.all_cpus);
                } else {
                    out.push_str(&r.cpus);
                }
            }
            ThreadColumn::LastCpu => {
                let _ = write!(out, "{}", r.last_cpu);
            }
            ThreadColumn::CtxSwitches => fmt_ctx(out, r.vol_ctx, r.invol_ctx),
            ThreadColumn::WaitChannel => fmt_wchan(out, &r.wchan),
        }
    }

    /// The focused row as `Label: value` lines, one per column.
    fn row_lines(&self, ix: usize) -> String {
        let r = self.row(ix);
        let mut out = String::new();
        for (i, &col) in ThreadColumn::ALL.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&self.words.headers[i]);
            out.push_str(": ");
            self.write_text(r, col, &mut out);
        }
        out
    }
}

fn specs(words: &ThreadWords) -> Vec<ColumnSpec<ThreadColumn>> {
    ThreadColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width, align, _) = id.meta();
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
                tooltip: words.tips[i].clone(),
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

impl TreeTableDelegate for ThreadsDelegate {
    type RowId = u32;
    type Column = ThreadColumn;
    type Sort = ThreadSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        self.row(ix).tid
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
    fn columns(&self) -> &ColumnLayout<ThreadColumn> {
        &self.layout
    }
    fn column_spec(&self, id: ThreadColumn) -> &ColumnSpec<ThreadColumn> {
        let i = ThreadColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: ThreadColumn, out: &mut CellOut) {
        let r = self.row(ix);
        out.bits = match col {
            ThreadColumn::Tid => u64::from(r.tid),
            ThreadColumn::Name | ThreadColumn::Affinity | ThreadColumn::WaitChannel => {
                // Text cells: the pointer identifies the text, the length guards a reuse.
                let s: &str = match col {
                    ThreadColumn::Name => &r.comm,
                    ThreadColumn::Affinity => &r.cpus,
                    _ => &r.wchan,
                };
                (s.as_ptr() as u64) ^ ((s.len() as u64) << 48)
            }
            ThreadColumn::State => state_index(r.state) as u64,
            ThreadColumn::Cpu => u64::from(r.cpu_pct.to_bits()),
            ThreadColumn::UserTime => r.utime_ms,
            ThreadColumn::KernelTime => r.stime_ms,
            ThreadColumn::Priority => {
                (u64::from(is_realtime(r.policy)) << 8) | u64::from(r.rt_prio)
            }
            ThreadColumn::BasePriority => base_priority(r) as u32 as u64,
            ThreadColumn::LastCpu => u64::from(r.last_cpu),
            ThreadColumn::CtxSwitches => r.vol_ctx.wrapping_mul(1_000_003) ^ r.invol_ctx,
        };
        // A restricted row hides only its wait channel (R-050); the rest reads as valid.
        if r.validity == Validity::Restricted {
            if col == ThreadColumn::WaitChannel {
                out.validity = Validity::Restricted;
                out.tooltip = Some(self.words.restricted.clone());
                return;
            }
            out.validity = Validity::Valid;
        } else {
            out.validity = r.validity;
        }
        if col == ThreadColumn::Priority && out.want_tooltip {
            out.tooltip = Some(SharedString::new_static(policy_name(r.policy)));
        }
        if out.want_text {
            self.write_text(r, col, &mut out.text);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::from(self.row(ix).comm.clone()),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<ThreadColumn> {
        Some(ThreadColumn::Name)
    }

    fn header_total(&self, _: ThreadColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> ThreadSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    /// E23-017 adds the row menu.
    fn context_menu(
        &mut self,
        _: &[u32],
        _: Option<ThreadColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }

    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    /// A new column sorts descending first; the same column flips.
    fn on_sort(&mut self, col: ThreadColumn, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = if self.sort.column == col {
            ThreadSort {
                column: col,
                ascending: !self.sort.ascending,
            }
        } else {
            ThreadSort {
                column: col,
                ascending: false,
            }
        };
        self.resort();
    }

    /// Widths live in memory only.
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<ThreadColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}

    /// Enter does nothing on the Threads tab.
    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        let w = &self.words;
        Some(if self.loading {
            w.loading.clone()
        } else if self.rows.is_empty() {
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

/// The Threads tab view. `PropsWindow` builds it with the process key and the tab's filter.
pub struct ThreadsTab {
    key: ProcessKey,
    table: Entity<TlTreeTable<ThreadsDelegate>>,
    filter: Entity<PropsFilter>,
    /// Lowercase `TID name state wchan` per payload row, for the filter.
    haystacks: Arc<[Box<str>]>,
    last_payload: Option<Arc<TabPayload>>,
    online: CpuSet,
    exited: bool,
    words: Rc<ThreadWords>,
    _subs: SmallVec<[Subscription; 3]>,
}

impl ThreadsTab {
    pub fn new(
        key: ProcessKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(ThreadWords::new(cx));
        let delegate = ThreadsDelegate::new(words.clone());
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
            if this.words.strings_version != strings_version(cx) {
                this.words = Rc::new(ThreadWords::new(cx));
                let words = this.words.clone();
                this.table.update(cx, |t, cx| {
                    t.delegate_mut().set_words(words);
                    t.on_new_version(cx);
                });
            }
        }));
        Self {
            key,
            table,
            filter,
            haystacks: Arc::from([]),
            last_payload: None,
            online: CpuSet::empty(),
            exited: false,
            words,
            _subs: subs,
        }
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn table(&self) -> &Entity<TlTreeTable<ThreadsDelegate>> {
        &self.table
    }

    pub fn words(&self) -> &Rc<ThreadWords> {
        &self.words
    }

    /// Takes the tab's payload. The same `Arc` again does nothing; while exited the rows stay
    /// frozen.
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
        self.last_payload = payload.cloned();
        let Some(TabPayload::Threads(rows)) = payload.map(|p| &**p) else {
            return;
        };
        let rows = rows.clone();
        let mut hay = String::new();
        self.haystacks = rows
            .iter()
            .map(|r| {
                hay.clear();
                let _ = write!(hay, "{} {} ", r.tid, r.comm);
                hay.push_str(&self.words.states[state_index(r.state)]);
                hay.push(' ');
                fmt_wchan(&mut hay, &r.wchan);
                lowercase(&hay).into_boxed_str()
            })
            .collect();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_rows(rows);
            t.on_new_version(cx);
        });
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        }
    }

    /// The online CPUs, for the `All` affinity form.
    pub fn set_online_cpus(&mut self, online: CpuSet, cx: &mut Context<Self>) {
        if online == self.online {
            return;
        }
        self.online = online;
        let mut text = String::new();
        cpu_list(&mut text, &online);
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.online = text;
            d.version += 1;
            t.on_new_version(cx);
        });
    }

    /// Freezes the rows once the process exited.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            cx.notify();
        }
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Runs the filter of `generation` over the payload rows and shows the result if it is
    /// still the latest.
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
            t.delegate_mut().set_filtered(keep);
            t.on_new_version(cx);
        });
    }

    /// Ctrl+C: the focused row as `Label: value` lines.
    fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let t = self.table.read(cx);
            let Some(focus) = t.selection.focus() else {
                return;
            };
            let Some(ix) = t.delegate.row_index(focus) else {
                return;
            };
            t.delegate.row_lines(ix)
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast_copied(window, cx);
    }

    fn toast_copied(&self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.words.copied.clone();
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
        }
    }
}

impl Render for ThreadsTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "PropsThreads".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_row))
            .child(self.table.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_ctx_format() {
        let mut s = String::new();
        fmt_ctx(&mut s, 1204, 33);
        assert_eq!(s, "1\u{2009}204 / 33");
        s.clear();
        fmt_ctx(&mut s, 0, 0);
        assert_eq!(s, "0 / 0");
    }

    #[test]
    fn threads_wchan_zero_is_dash() {
        let mut s = String::new();
        fmt_wchan(&mut s, "0");
        assert_eq!(s, "-");
        s.clear();
        fmt_wchan(&mut s, "futex_wait_queue");
        assert_eq!(s, "futex_wait_queue");
    }

    #[test]
    fn threads_cpu_list_ranges() {
        let mut set = CpuSet::empty();
        for c in [0, 1, 2, 3, 8, 10, 11] {
            set.insert(c);
        }
        let mut s = String::new();
        cpu_list(&mut s, &set);
        assert_eq!(s, "0-3,8,10-11");
    }
}
