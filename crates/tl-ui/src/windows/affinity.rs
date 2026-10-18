//! Set Affinity dialog for one process target (UI-PWN-021, PROC-067, R-039, UI-PRC-021, ADR-003,
//! ADR-008, R-119, E23-002).
//!
//! The dialog shows the CPU topology as core tiles grouped by L3 cache (or P-cores and E-cores
//! on a hybrid CPU), keeps a CPU list field in sync with the checkboxes, and sends one
//! `SetAffinity` action through the `ActionTracker`. While it is open, its targets are in
//! `Demand.affinity_targets`, so the engine reads their current mask and cpuset limit into
//! `DetailsSnapshot.affinity`.
//!
//! Several targets and thread targets (E23-005, UI-PWN-022): a CPU is ON when every target has
//! it, MIXED when only some do, OFF when none. A click moves MIXED to ON, ON to OFF and OFF to
//! ON; only Reset brings MIXED back. OK sends one request per group of targets whose result
//! masks are equal, all under one toast key, so one toast shows the results in turn (R-197).
//!
//! The grid is one focus stop: the arrow keys move a cursor over the CPUs, Space toggles the
//! CPU under it and Shift+Space its group. The CPU checkboxes inside are no tab stops.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{ElementExt as _, Root};
use gpui_kit::component::Icon;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Entity,
    FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription, WeakEntity,
    Window, WindowHandle, div, px, size,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionRequest, ActionState, ActionTarget};
use tl_model::demand::Demand;
use tl_model::details::AffinityInfo;
use tl_model::ids::ProcessKey;
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::system::{CoreKind, CpuSet, CpuTopology};
use tracing::warn;

use crate::action_tracker::{GlobalActionTracker, SummaryTexts, ToastTexts, short_reason};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::checkbox::{CheckboxLabelStyle, TlCheckbox};
use crate::components::focus::TlFocusRing;
use crate::components::icons::{AppIcon, TlIcon};
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::TlSecondaryWindow;
use crate::demand::GlobalDemand;
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::Strings;
use crate::pages::performance::details::Template;
use crate::text::style::{label_style, t_sm, t_xs};
use crate::theme::{current, metrics};
use crate::windows::actions::{
    AFFINITY_DIALOG_CONTEXT, AffinityApply, AffinityEnd, AffinityHome, AffinityMoveDown,
    AffinityMoveLeft, AffinityMoveRight, AffinityMoveUp, AffinityReset, AffinityToggle,
    AffinityToggleGroup,
};
use crate::windows::frame::{AFFINITY_HEIGHT, PwnWindow, open_pwn};

/// Targets captured when the menu opens (`Demand.affinity_targets` max).
pub const MAX_TARGETS: usize = 256;
/// The highest CPU number a list may name.
pub const MAX_CPU: u16 = CpuSet::MAX_CPUS - 1;
/// Core tiles per row.
pub const TILES_PER_ROW: usize = 8;

/// Heights and sizes at 100% scale (UI-PWN-021).
const SUBTITLE_H: f32 = 24.;
const ALL_ROW_H: f32 = 32.;
const GROUP_HEADER_H: f32 = 24.;
const TILE_W: f32 = 72.;
const TILE_H: f32 = 52.;
const TILE_GAP: f32 = 4.;
const CPU_ROW_H: f32 = 24.;
/// The P-core and E-core icon in a hybrid group header, × ui_scale. Tiles have no icon: a
/// 10 px icon before "CPU n" does not fit the 64 px tile interior.
const GROUP_ICON: f32 = 12.;
const VALIDATION_H: f32 = 20.;
const HELPER_H: f32 = 16.;
const FOOTER_H: f32 = 52.;
const LIST_W: f32 = 200.;
const ALERT_ICON: f32 = 14.;
const RESET_ICON: f32 = 12.;

const KEY_TITLE: &str = "pwn-affinity-title";
const KEY_LOADING: &str = "pwn-affinity-subtitle-loading";
const KEY_ALL: &str = "pwn-affinity-all-processors";
const KEY_CPU_LIST: &str = "pwn-affinity-cpu-list";
const KEY_PLACEHOLDER: &str = "pwn-affinity-placeholder";
const KEY_GROUP_CCD: &str = "pwn-affinity-group-ccd";
const KEY_GROUP_P: &str = "pwn-affinity-group-pcores";
const KEY_GROUP_E: &str = "pwn-affinity-group-ecores";
const KEY_GROUP_CPUS: &str = "pwn-affinity-group-cpus";
const KEY_CPU: &str = "pwn-affinity-cpu";
const KEY_CORE_TIP: &str = "pwn-affinity-core-tooltip";
const KEY_CORE_TIP_ONE: &str = "pwn-affinity-core-tooltip-one";
const KEY_PCORE_TIP: &str = "pwn-affinity-pcore-tooltip";
const KEY_PCORE_TIP_ONE: &str = "pwn-affinity-pcore-tooltip-one";
const KEY_ECORE_TIP: &str = "pwn-affinity-ecore-tooltip";
const KEY_ECORE_TIP_ONE: &str = "pwn-affinity-ecore-tooltip-one";
const KEY_VALIDATION: &str = "pwn-affinity-validation";
const KEY_INVALID_LIST: &str = "pwn-affinity-invalid-list";
const KEY_CPUSET_TIP: &str = "pwn-affinity-cpuset-tooltip";
const KEY_OFFLINE_TIP: &str = "pwn-affinity-offline-tooltip";
const KEY_OK: &str = "pwn-affinity-ok";
const KEY_CANCEL: &str = "pwn-affinity-cancel";
const KEY_TOAST_OK: &str = "pwn-affinity-toast-success";
const KEY_TOAST_FAIL: &str = "pwn-affinity-toast-failure";
const KEY_SUB_PROCESS: &str = "pwn-affinity-subtitle-process";
const KEY_SUB_PROCESSES: &str = "pwn-affinity-subtitle-processes";
const KEY_SUB_THREAD: &str = "pwn-affinity-subtitle-thread";
const KEY_SUB_THREADS: &str = "pwn-affinity-subtitle-threads";
const KEY_SUB_APP: &str = "pwn-affinity-subtitle-app";
const KEY_RESET: &str = "pwn-affinity-reset";
const KEY_MIXED_HELPER: &str = "pwn-affinity-mixed-helper";
const KEY_MIXED_TIP: &str = "pwn-affinity-mixed-tooltip";
const KEY_TOAST_MANY: &str = "pwn-affinity-toast-success-many";
const KEY_TOAST_PARTIAL: &str = "pwn-affinity-toast-partial";
const KEY_TOAST_THREADS: &str = "pwn-affinity-toast-threads";
const KEY_TOAST_PARTIAL_THREADS: &str = "pwn-affinity-toast-partial-threads";

// ---- CPU lists ----

/// Parses a CPU list such as `0-7,16-23`: comma-separated CPU numbers and `a-b` ranges with
/// `a <= b`, each at most `max_cpu`. Spaces around an item are allowed. An empty text, an empty
/// item or anything else is an error.
#[allow(clippy::result_unit_err)] // The bead fixes this signature; the caller only needs valid or not.
pub fn parse_cpu_list(text: &str, max_cpu: u16) -> Result<CpuSet, ()> {
    let max_cpu = max_cpu.min(MAX_CPU);
    let mut set = CpuSet::empty();
    for item in text.split(',') {
        let item = item.trim();
        let (a, b) = match item.split_once('-') {
            Some((a, b)) => (cpu_number(a, max_cpu)?, cpu_number(b, max_cpu)?),
            None => {
                let n = cpu_number(item, max_cpu)?;
                (n, n)
            }
        };
        if a > b {
            return Err(());
        }
        for cpu in a..=b {
            set.insert(cpu);
        }
    }
    Ok(set)
}

/// One CPU number: ASCII digits only, at most `max_cpu`.
fn cpu_number(text: &str, max_cpu: u16) -> Result<u16, ()> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    text.parse::<u16>().ok().filter(|&n| n <= max_cpu).ok_or(())
}

/// Writes `set` as the canonical list into `out` (cleared first): ascending, runs of two or more
/// CPUs merged into `a-b`, such as `0-7,16-23`. An empty set writes nothing.
pub fn format_cpu_list(set: &CpuSet, out: &mut String) {
    use std::fmt::Write as _;
    out.clear();
    let mut cpus = set.iter().peekable();
    while let Some(start) = cpus.next() {
        let mut end = start;
        while cpus.peek() == Some(&(end + 1)) {
            end += 1;
            cpus.next();
        }
        if !out.is_empty() {
            out.push(',');
        }
        let _ = if end > start {
            write!(out, "{start}-{end}")
        } else {
            write!(out, "{start}")
        };
    }
}

fn cpu_list(set: &CpuSet) -> String {
    let mut out = String::new();
    format_cpu_list(set, &mut out);
    out
}

// ---- State ----

/// A CPU is not in the mask (of any target).
pub const OFF: u8 = 0;
/// A CPU is in the mask (of every target).
pub const ON: u8 = 1;
/// A CPU is in the mask of some targets only (E23-005).
pub const MIXED: u8 = 2;

/// One mark per CPU: [`OFF`], [`ON`] or [`MIXED`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CpuTriState(pub [u8; CpuSet::MAX_CPUS as usize]);

impl Default for CpuTriState {
    fn default() -> Self {
        Self([OFF; CpuSet::MAX_CPUS as usize])
    }
}

impl CpuTriState {
    /// `ON` for every CPU in `set`, else `OFF`.
    pub fn from_set(set: &CpuSet) -> Self {
        let mut out = Self::default();
        for cpu in set.iter() {
            out.0[usize::from(cpu)] = ON;
        }
        out
    }

    pub fn get(&self, cpu: u16) -> u8 {
        self.0.get(usize::from(cpu)).copied().unwrap_or(OFF)
    }

    pub fn set(&mut self, cpu: u16, mark: u8) {
        if let Some(slot) = self.0.get_mut(usize::from(cpu)) {
            *slot = mark;
        }
    }

    /// The CPUs marked `ON`.
    pub fn on_set(&self) -> CpuSet {
        self.marked(ON)
    }

    /// The CPUs marked `MIXED`.
    pub fn mixed_set(&self) -> CpuSet {
        self.marked(MIXED)
    }

    fn marked(&self, mark: u8) -> CpuSet {
        let mut set = CpuSet::empty();
        for (cpu, &m) in self.0.iter().enumerate() {
            if m == mark {
                set.insert(cpu as u16);
            }
        }
        set
    }

    /// The marks for `masks` inside `allowed`: ON in every mask, MIXED in some, else OFF.
    pub fn from_masks(masks: &[CpuSet], allowed: &CpuSet) -> Self {
        let Some((first, rest)) = masks.split_first() else {
            return Self::default();
        };
        let (every, any) = rest.iter().fold((*first, *first), |(every, any), m| {
            (every.intersection(m), union(&any, m))
        });
        let mut out = Self::from_set(&every.intersection(allowed));
        for cpu in any.intersection(allowed).iter() {
            if out.get(cpu) != ON {
                out.set(cpu, MIXED);
            }
        }
        out
    }
}

/// The CPUs in `a` or `b`.
fn union(a: &CpuSet, b: &CpuSet) -> CpuSet {
    let mut out = *a;
    for cpu in b.iter() {
        out.insert(cpu);
    }
    out
}

/// The click cycle: MIXED to ON, ON to OFF, OFF to ON.
pub fn next_mark(mark: u8) -> u8 {
    if mark == ON { OFF } else { ON }
}

/// Per target, the mask OK sets: the ON CPUs plus the MIXED CPUs the target has now. Targets
/// with equal result masks are grouped, in target order.
pub fn group_masks(
    state: &CpuTriState,
    targets: &[ActionTarget],
    initial: &[CpuSet],
) -> SmallVec<[(CpuSet, SmallVec<[ActionTarget; 4]>); 4]> {
    let on = state.on_set();
    let mixed = state.mixed_set();
    let mut out: SmallVec<[(CpuSet, SmallVec<[ActionTarget; 4]>); 4]> = SmallVec::new();
    for (i, target) in targets.iter().enumerate() {
        let current = initial.get(i).copied().unwrap_or_default();
        let mask = union(&on, &current.intersection(&mixed));
        match out.iter_mut().find(|(m, _)| *m == mask) {
            Some((_, group)) => group.push(target.clone()),
            None => out.push((mask, smallvec::smallvec![target.clone()])),
        }
    }
    out
}

/// Who the dialog is for; [`subtitle`] turns it into the subtitle line (UI-PWN-022).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AffinitySubject {
    /// `firefox (PID 4321)`.
    Process { name: SharedString, pid: u32 },
    /// `4 processes`.
    Processes(usize),
    /// `Thread 4325 of firefox (PID 4321)`.
    Thread {
        tid: u32,
        name: SharedString,
        pid: u32,
    },
    /// `3 threads of firefox (PID 4321)`.
    Threads {
        n: usize,
        name: SharedString,
        pid: u32,
    },
    /// `Firefox (34 processes)`.
    App { name: SharedString, n: usize },
}

/// The subtitle line for `subject`.
pub fn subtitle(subject: &AffinitySubject, cx: &mut App) -> SharedString {
    let w = affinity_words(cx);
    match subject {
        AffinitySubject::Process { name, pid } => fill(&w.sub_process, &[name, pid]),
        AffinitySubject::Processes(n) => fill(&w.sub_processes, &[n]),
        AffinitySubject::Thread { tid, name, pid } => fill(&w.sub_thread, &[tid, name, pid]),
        AffinitySubject::Threads { n, name, pid } => fill(&w.sub_threads, &[n, name, pid]),
        AffinitySubject::App { name, n } => fill(&w.sub_app, &[name, n]),
    }
}

// ---- Topology groups ----

/// What a group header says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupLabel {
    /// `CCD {n}: CPU {list}`, one per L3 cache when there are two or more.
    Ccd(u16),
    /// `P-cores: CPU {list}` on a hybrid CPU.
    PCores,
    /// `E-cores: CPU {list}` on a hybrid CPU.
    ECores,
    /// `CPUs: {list}`: one L3 cache, or no L3 data.
    Cpus,
}

/// One core tile: the first logical CPU on top, its SMT sibling below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CpuTile {
    /// `CoreInfo.id`.
    pub core: u16,
    pub top: u16,
    pub bottom: Option<u16>,
    pub kind: CoreKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CpuGroup {
    pub label: GroupLabel,
    /// Every CPU in the group's tiles.
    pub cpus: CpuSet,
    /// In core order.
    pub tiles: Vec<CpuTile>,
}

/// The icon and its letter for a hybrid group header.
fn core_icon(label: GroupLabel) -> Option<(AppIcon, char)> {
    match label {
        GroupLabel::PCores => Some((AppIcon::CpuPCore, 'P')),
        GroupLabel::ECores => Some((AppIcon::CpuECore, 'E')),
        _ => None,
    }
}

/// The groups of `topology`: P-cores and E-cores when any core is hybrid, else one group per
/// L3 cache when there are two or more, else one `CPUs` group. The collector reports a
/// missing L3 as group 0 for every core, so one L3 and no L3 data look the same.
pub fn cpu_groups(topology: &CpuTopology) -> Vec<CpuGroup> {
    let hybrid = topology
        .cores
        .iter()
        .any(|c| matches!(c.kind, CoreKind::Performance | CoreKind::Efficiency));
    let mut keys: Vec<(u16, GroupLabel)> = Vec::new();
    let key_of = |kind: CoreKind, l3: u16| -> (u16, GroupLabel) {
        if hybrid {
            match kind {
                CoreKind::Efficiency => (1, GroupLabel::ECores),
                _ => (0, GroupLabel::PCores),
            }
        } else {
            (l3, GroupLabel::Ccd(l3))
        }
    };
    for core in topology.cores.iter() {
        let key = key_of(core.kind, core.l3_group);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.sort_unstable_by_key(|(order, _)| *order);
    let single_l3 = !hybrid && keys.len() <= 1;

    let mut groups: Vec<CpuGroup> = keys
        .iter()
        .enumerate()
        .map(|(i, (_, label))| CpuGroup {
            label: match label {
                _ if single_l3 => GroupLabel::Cpus,
                GroupLabel::Ccd(_) => GroupLabel::Ccd(i as u16),
                other => *other,
            },
            cpus: CpuSet::empty(),
            tiles: Vec::new(),
        })
        .collect();
    for core in topology.cores.iter() {
        let key = key_of(core.kind, core.l3_group);
        let Some(i) = keys.iter().position(|k| *k == key) else {
            continue;
        };
        let group = &mut groups[i];
        // A core with more than two threads gets one tile per pair.
        for pair in core.logical.chunks(2) {
            let tile = CpuTile {
                core: core.id,
                top: pair[0],
                bottom: pair.get(1).copied(),
                kind: core.kind,
            };
            group.cpus.insert(tile.top);
            if let Some(b) = tile.bottom {
                group.cpus.insert(b);
            }
            group.tiles.push(tile);
        }
    }
    groups
}

/// One visual line of CPUs: the top or bottom row of one tile row. `None` is a tile without a
/// CPU in that row.
type GridLine = SmallVec<[Option<u16>; TILES_PER_ROW]>;

/// The grid as visual lines, top to bottom, for the arrow keys.
fn grid_lines(groups: &[CpuGroup]) -> Vec<GridLine> {
    let mut lines = Vec::new();
    for group in groups {
        for row in group.tiles.chunks(TILES_PER_ROW) {
            lines.push(row.iter().map(|t| Some(t.top)).collect());
            if row.iter().any(|t| t.bottom.is_some()) {
                lines.push(row.iter().map(|t| t.bottom).collect());
            }
        }
    }
    lines
}

/// A cursor move in the grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridMove {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// Where `cursor` goes on `mv`. Left and Right stay in the visual line; Up and Down go to the
/// nearest CPU in the same column of the next line that has one; Home and End go to the first
/// and last CPU. A cursor not in the grid goes to the first CPU.
fn move_cursor(lines: &[GridLine], cursor: Option<u16>, mv: GridMove) -> Option<u16> {
    let first = lines.iter().flatten().flatten().next().copied();
    let last = lines
        .iter()
        .rev()
        .flat_map(|l| l.iter().rev())
        .flatten()
        .next()
        .copied();
    let pos = cursor.and_then(|cpu| {
        lines.iter().enumerate().find_map(|(li, line)| {
            line.iter()
                .position(|c| *c == Some(cpu))
                .map(|col| (li, col))
        })
    });
    let Some((li, col)) = pos else {
        return first;
    };
    let line = &lines[li];
    let found = match mv {
        GridMove::Home => first,
        GridMove::End => last,
        GridMove::Left => line[..col].iter().rev().flatten().next().copied(),
        GridMove::Right => line[col + 1..].iter().flatten().next().copied(),
        GridMove::Up => lines[..li]
            .iter()
            .rev()
            .find_map(|l| nearest_in_line(l, col)),
        GridMove::Down => lines[li + 1..].iter().find_map(|l| nearest_in_line(l, col)),
    };
    found.or(cursor)
}

/// The CPU at `col` in `line`, else the nearest one to the left, else to the right.
fn nearest_in_line(line: &GridLine, col: usize) -> Option<u16> {
    let col = col.min(line.len().saturating_sub(1));
    line.get(col)
        .copied()
        .flatten()
        .or_else(|| line[..col].iter().rev().flatten().next().copied())
        .or_else(|| line.get(col + 1..)?.iter().flatten().next().copied())
}

// ---- Words (R-132) ----

/// The dialog texts, translated once per strings version by [`affinity_words`].
pub struct AffinityWords {
    pub title: SharedString,
    loading: Template,
    pub all_processors: SharedString,
    pub cpu_list: SharedString,
    pub placeholder: SharedString,
    group_ccd: Template,
    group_p: Template,
    group_e: Template,
    group_cpus: Template,
    cpu: Template,
    /// Core tooltips with two CPUs and with one: Standard, Performance, Efficiency.
    core_tip: [(Template, Template); 3],
    pub validation: SharedString,
    pub invalid_list: SharedString,
    pub cpuset_tooltip: SharedString,
    pub offline_tooltip: SharedString,
    pub ok: SharedString,
    pub cancel: SharedString,
    pub toast_success: SharedString,
    pub toast_failure: SharedString,
    sub_process: Template,
    sub_processes: Template,
    sub_thread: Template,
    sub_threads: Template,
    sub_app: Template,
    pub reset: SharedString,
    pub mixed_helper: SharedString,
    pub mixed_tooltip: SharedString,
    /// Several processes: "Selected process affinity updated."
    pub toast_success_many: SharedString,
    /// "Affinity updated for {ok} of {total} processes. {failed} failed: {reason}".
    toast_partial: Template,
    pub toast_threads: SharedString,
    /// "Affinity updated for {ok} of {total} threads. {failed} failed: {reason}".
    toast_partial_threads: Template,
}

impl AffinityWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        Self {
            title: t(KEY_TITLE),
            loading: tmpl(KEY_LOADING, &["subtitle"]),
            all_processors: t(KEY_ALL),
            cpu_list: t(KEY_CPU_LIST),
            placeholder: t(KEY_PLACEHOLDER),
            group_ccd: tmpl(KEY_GROUP_CCD, &["n", "list"]),
            group_p: tmpl(KEY_GROUP_P, &["list"]),
            group_e: tmpl(KEY_GROUP_E, &["list"]),
            group_cpus: tmpl(KEY_GROUP_CPUS, &["list"]),
            cpu: tmpl(KEY_CPU, &["n"]),
            core_tip: [
                (KEY_CORE_TIP, KEY_CORE_TIP_ONE),
                (KEY_PCORE_TIP, KEY_PCORE_TIP_ONE),
                (KEY_ECORE_TIP, KEY_ECORE_TIP_ONE),
            ]
            .map(|(two, one)| (tmpl(two, &["k", "a", "b"]), tmpl(one, &["k", "a"]))),
            validation: t(KEY_VALIDATION),
            invalid_list: t(KEY_INVALID_LIST),
            cpuset_tooltip: t(KEY_CPUSET_TIP),
            offline_tooltip: t(KEY_OFFLINE_TIP),
            ok: t(KEY_OK),
            cancel: t(KEY_CANCEL),
            toast_success: t(KEY_TOAST_OK),
            toast_failure: t(KEY_TOAST_FAIL),
            sub_process: tmpl(KEY_SUB_PROCESS, &["name", "pid"]),
            sub_processes: tmpl(KEY_SUB_PROCESSES, &["n"]),
            sub_thread: tmpl(KEY_SUB_THREAD, &["tid", "name", "pid"]),
            sub_threads: tmpl(KEY_SUB_THREADS, &["n", "name", "pid"]),
            sub_app: tmpl(KEY_SUB_APP, &["name", "n"]),
            reset: t(KEY_RESET),
            mixed_helper: t(KEY_MIXED_HELPER),
            mixed_tooltip: t(KEY_MIXED_TIP),
            toast_success_many: t(KEY_TOAST_MANY),
            toast_partial: tmpl(KEY_TOAST_PARTIAL, &["ok", "total", "failed", "reason"]),
            toast_threads: t(KEY_TOAST_THREADS),
            toast_partial_threads: tmpl(
                KEY_TOAST_PARTIAL_THREADS,
                &["ok", "total", "failed", "reason"],
            ),
        }
    }

    fn en() -> Self {
        Self::with(|key| crate::i18n::tr_en(key, &[]), Template::en)
    }

    fn translated(cx: &App) -> Self {
        Self::with(
            |key| crate::i18n::tr(cx, key),
            |key, args| Template::new(cx, key, args),
        )
    }

    /// `{subtitle} - reading current affinity…`.
    pub fn loading(&self, subtitle: &str) -> SharedString {
        fill(&self.loading, &[&subtitle])
    }

    /// The header text of `group`.
    pub fn group(&self, group: &CpuGroup) -> SharedString {
        let list = cpu_list(&group.cpus);
        match group.label {
            GroupLabel::Ccd(n) => fill(&self.group_ccd, &[&n, &list]),
            GroupLabel::PCores => fill(&self.group_p, &[&list]),
            GroupLabel::ECores => fill(&self.group_e, &[&list]),
            GroupLabel::Cpus => fill(&self.group_cpus, &[&list]),
        }
    }

    /// "Affinity updated for {ok} of {total} processes. {failed} failed: {reason}".
    pub fn partial(&self, ok: u16, total: u16, reason: &str, threads: bool) -> SharedString {
        let template = if threads {
            &self.toast_partial_threads
        } else {
            &self.toast_partial
        };
        fill(template, &[&ok, &total, &(total - ok), &reason])
    }

    /// `CPU {n}`.
    pub fn cpu(&self, n: u16) -> SharedString {
        fill(&self.cpu, &[&n])
    }

    /// `Core {k}: CPU {a} and CPU {b}`, or `Core {k}: CPU {a}` without SMT. On a hybrid CPU it
    /// starts with `P-core` or `E-core` instead of `Core`.
    pub fn core_tooltip(&self, tile: &CpuTile) -> SharedString {
        let (two, one) = &self.core_tip[match tile.kind {
            CoreKind::Standard => 0,
            CoreKind::Performance => 1,
            CoreKind::Efficiency => 2,
        }];
        match tile.bottom {
            Some(b) => fill(two, &[&tile.core, &tile.top, &b]),
            None => fill(one, &[&tile.core, &tile.top]),
        }
    }
}

fn fill(template: &Template, values: &[&dyn std::fmt::Display]) -> SharedString {
    let mut out = String::new();
    let _ = template.write(&mut out, values);
    out.into()
}

struct AffinityWordsCache {
    version: u64,
    words: Rc<AffinityWords>,
}

impl Global for AffinityWordsCache {}

/// The texts for the current strings. Translated only when the strings version changed; without
/// strings they are in en-US.
pub fn affinity_words(cx: &mut App) -> Rc<AffinityWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(AffinityWords::en());
    };
    if let Some(cache) = cx.try_global::<AffinityWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(AffinityWords::translated(cx));
    cx.set_global(AffinityWordsCache {
        version,
        words: words.clone(),
    });
    words
}

/// [`affinity_words`] for read-outs that only have `&App`: the cached texts when current, else
/// freshly translated ones (not cached).
fn words_now(cx: &App) -> Rc<AffinityWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(AffinityWords::en());
    };
    match cx.try_global::<AffinityWordsCache>() {
        Some(cache) if cache.version == version => cache.words.clone(),
        _ => Rc::new(AffinityWords::translated(cx)),
    }
}

// ---- The dialog ----

/// What one CPU checkbox shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuView {
    pub checked: bool,
    /// Set for some of the targets only: the checkbox shows its indeterminate mark.
    pub mixed: bool,
    pub disabled: bool,
    pub tooltip: Option<SharedString>,
}

/// Why a CPU checkbox is disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CpuBlock {
    Loading,
    Cpuset,
    Offline,
}

pub struct AffinityDialog {
    /// Captured when the menu opened, at most [`MAX_TARGETS`].
    targets: SmallVec<[ActionTarget; 1]>,
    /// The mask of each target, once `DetailsSnapshot.affinity` has it.
    initial: Vec<CpuSet>,
    /// The caller's masks per target (thread targets: `ThreadRow.cpus`), used instead of the
    /// process masks in `DetailsSnapshot.affinity`.
    given: Option<Vec<CpuSet>>,
    state: CpuTriState,
    /// The marks when the masks arrived; Reset returns to them.
    initial_state: CpuTriState,
    list_input: Entity<InputState>,
    loading: bool,
    subtitle: SharedString,
    topology: std::sync::Arc<CpuTopology>,
    groups: Vec<CpuGroup>,
    lines: Vec<GridLine>,
    /// Online CPUs inside every target's cpuset limit.
    allowed: CpuSet,
    /// The CPU list field holds text that is not a valid list of allowed CPUs.
    list_invalid: bool,
    cursor: Option<u16>,
    /// "All processors", focused when the dialog opens.
    all_focus: FocusHandle,
    grid_focus: FocusHandle,
    cancel_focus: FocusHandle,
    /// The parent window's toast queue; results show there (UI-PRC-015).
    parent_toasts: Option<WeakEntity<ToastQueue>>,
    /// The window height last asked for.
    fitted: Rc<std::cell::Cell<Option<Pixels>>>,
    _subs: Vec<Subscription>,
}

/// Opens the Set Affinity dialog for `targets`, modal to `parent`. `subtitle` is
/// `{name} (PID {pid})`. Returns the open error and opens nothing on failure.
pub fn open(
    targets: SmallVec<[ActionTarget; 1]>,
    subtitle: SharedString,
    parent: AnyWindowHandle,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    open_with_masks(targets, subtitle, None, parent, cx)
}

/// [`open`] with the current mask of each target given by the caller, one per target in order:
/// thread targets pass `ThreadRow.cpus`. The cpuset limit still comes from the process's
/// `AffinityInfo`.
pub fn open_with_masks(
    mut targets: SmallVec<[ActionTarget; 1]>,
    subtitle: SharedString,
    mut given: Option<Vec<CpuSet>>,
    parent: AnyWindowHandle,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    targets.truncate(MAX_TARGETS);
    if let Some(given) = &mut given {
        given.truncate(MAX_TARGETS);
    }
    let title = affinity_words(cx).title.clone();
    let parent_toasts = parent
        .update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
        .ok()
        .flatten()
        .map(|q| q.downgrade());
    open_pwn(
        cx,
        PwnWindow::SetAffinity,
        title,
        Some(parent),
        move |window, cx| {
            cx.new(|cx| AffinityDialog::new(targets, subtitle, given, parent_toasts, window, cx))
        },
    )
}

impl AffinityDialog {
    /// The content of a dialog opened by [`open`], while it is open.
    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    fn new(
        targets: SmallVec<[ActionTarget; 1]>,
        subtitle: SharedString,
        given: Option<Vec<CpuSet>>,
        parent_toasts: Option<WeakEntity<ToastQueue>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list_input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &list_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => this.on_list_changed(cx),
                InputEvent::PressEnter { .. } => this.apply(window, cx),
                _ => {}
            },
        ));
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        if let Some(hub) = &hub {
            subs.push(cx.subscribe_in(
                hub,
                window,
                |this, hub, ev: &SnapshotChanged, window, cx| {
                    if ev
                        .mask
                        .intersects(ComponentMask::SYSTEM | ComponentMask::DETAILS)
                    {
                        let snap = hub.read(cx).current().clone();
                        if this.take_snapshot(&snap, window, cx) {
                            cx.notify();
                        }
                    }
                },
            ));
        }

        // Demand: the engine reads the targets' masks while the dialog is open (ADR-008).
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let id = cx.entity_id();
            let part = Demand {
                any_surface_visible: true,
                affinity_targets: {
                    let mut keys: Vec<ProcessKey> = Vec::new();
                    for key in target_keys(&targets) {
                        if !keys.contains(&key) {
                            keys.push(key);
                        }
                    }
                    keys.into()
                },
                ..Demand::default()
            };
            let handle = window.window_handle();
            demand.update(cx, |d, cx| d.set(id, Some(handle), part, cx));
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }

        let grid_focus = cx.focus_handle().tab_stop(true);
        let all_focus = cx.focus_handle();
        all_focus.focus(window, cx);
        let mut this = Self {
            targets,
            initial: Vec::new(),
            given,
            state: CpuTriState::default(),
            initial_state: CpuTriState::default(),
            list_input,
            loading: true,
            subtitle,
            topology: Default::default(),
            groups: Vec::new(),
            lines: Vec::new(),
            allowed: CpuSet::empty(),
            list_invalid: false,
            cursor: None,
            all_focus,
            grid_focus,
            cancel_focus: cx.focus_handle(),
            parent_toasts,
            fitted: Rc::default(),
            _subs: subs,
        };
        if let Some(hub) = hub {
            let snap = hub.read(cx).current().clone();
            this.take_snapshot(&snap, window, cx);
        }
        this
    }

    /// Takes the topology and the targets' affinity from `snap`. False when nothing the dialog
    /// shows changed.
    fn take_snapshot(
        &mut self,
        snap: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        let topology = &snap.system.cpu_topology;
        if !std::sync::Arc::ptr_eq(topology, &self.topology) {
            self.topology = topology.clone();
            self.groups = cpu_groups(topology);
            self.lines = grid_lines(&self.groups);
            if self.cursor.is_none_or(|c| !self.has_cpu(c)) {
                self.cursor = move_cursor(&self.lines, None, GridMove::Home);
            }
            changed = true;
        }

        let infos: Option<Vec<AffinityInfo>> = target_keys(&self.targets)
            .map(|key| snap.details.affinity.get(&key).copied())
            .collect();
        let mut allowed = self.topology.online;
        if let Some(infos) = &infos {
            for info in infos {
                if !info.cpuset_limit.is_empty() {
                    allowed = allowed.intersection(&info.cpuset_limit);
                }
            }
        }
        if allowed != self.allowed {
            self.allowed = allowed;
            changed = true;
        }
        if self.loading
            && let Some(infos) = infos
            && !self.targets.is_empty()
        {
            self.loading = false;
            self.initial = match &self.given {
                Some(given) if given.len() == self.targets.len() => given.clone(),
                _ => infos.iter().map(|i| i.mask).collect(),
            };
            self.state = CpuTriState::from_masks(&self.initial, &self.allowed);
            self.initial_state = self.state.clone();
            self.write_list(window, cx);
            changed = true;
        }
        changed
    }

    fn has_cpu(&self, cpu: u16) -> bool {
        self.groups.iter().any(|g| g.cpus.contains(cpu))
    }

    fn block(&self, cpu: u16) -> Option<CpuBlock> {
        if !self.topology.online.contains(cpu) {
            Some(CpuBlock::Offline)
        } else if !self.allowed.contains(cpu) {
            Some(CpuBlock::Cpuset)
        } else if self.loading {
            Some(CpuBlock::Loading)
        } else {
            None
        }
    }

    // ---- Read-outs, also for tests ----

    pub fn targets(&self) -> &[ActionTarget] {
        &self.targets
    }

    pub fn initial(&self) -> &[CpuSet] {
        &self.initial
    }

    pub fn state(&self) -> &CpuTriState {
        &self.state
    }

    pub fn groups(&self) -> &[CpuGroup] {
        &self.groups
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn list_input(&self) -> &Entity<InputState> {
        &self.list_input
    }

    pub fn grid_focus(&self) -> &FocusHandle {
        &self.grid_focus
    }

    pub fn all_focus(&self) -> &FocusHandle {
        &self.all_focus
    }

    pub fn cursor(&self) -> Option<u16> {
        self.cursor
    }

    /// The CPUs checked now.
    pub fn checked(&self) -> CpuSet {
        self.state.on_set()
    }

    /// The CPU list field text.
    pub fn list_text(&self, cx: &App) -> String {
        self.list_input.read(cx).text().to_string()
    }

    /// The field holds an invalid list; the helper text shows.
    pub fn list_invalid(&self) -> bool {
        self.list_invalid
    }

    /// "All processors": checked when every allowed CPU is, `None` (indeterminate) when some are.
    pub fn all_state(&self) -> Option<bool> {
        let on = self.checked().intersection(&self.allowed).count();
        if on == 0 {
            Some(false)
        } else if on == self.allowed.count() {
            Some(true)
        } else {
            None
        }
    }

    /// A group header checkbox: like [`Self::all_state`] over the group's allowed CPUs.
    pub fn group_state(&self, group: usize) -> Option<bool> {
        let Some(group) = self.groups.get(group) else {
            return Some(false);
        };
        let allowed = group.cpus.intersection(&self.allowed);
        let on = self.checked().intersection(&allowed).count();
        if on == 0 {
            Some(false)
        } else if on == allowed.count() {
            Some(true)
        } else {
            None
        }
    }

    /// Nothing is checked: the validation line shows.
    /// With several targets: no ON and no MIXED CPU.
    pub fn nothing_checked(&self) -> bool {
        !self.loading && self.checked().is_empty() && self.state.mixed_set().is_empty()
    }

    pub fn ok_enabled(&self) -> bool {
        !self.loading && !self.list_invalid && !self.nothing_checked()
    }

    /// Some CPU is MIXED: the helper under the field shows.
    pub fn has_mixed(&self) -> bool {
        !self.loading && !self.state.mixed_set().is_empty()
    }

    /// Reset shows with several targets.
    pub fn reset_shown(&self) -> bool {
        self.targets.len() > 1
    }

    /// The marks Reset returns to.
    pub fn initial_state(&self) -> &CpuTriState {
        &self.initial_state
    }

    /// Per target, ON set, OFF cleared and MIXED kept from that target's current mask; targets
    /// with equal results grouped.
    pub fn resulting_masks(&self) -> SmallVec<[(CpuSet, SmallVec<[ActionTarget; 4]>); 4]> {
        group_masks(&self.state, &self.targets, &self.initial)
    }

    /// The OK tooltip while OK is disabled by an empty selection.
    pub fn ok_tooltip(&self, cx: &App) -> Option<SharedString> {
        self.nothing_checked()
            .then(|| words_now(cx).validation.clone())
    }

    /// The subtitle line, with the loading suffix while the masks are read.
    pub fn subtitle(&self, cx: &App) -> SharedString {
        if self.loading {
            words_now(cx).loading(&self.subtitle)
        } else {
            self.subtitle.clone()
        }
    }

    /// What CPU `cpu`'s checkbox shows.
    pub fn cpu_view(&self, cpu: u16, cx: &App) -> CpuView {
        let words = words_now(cx);
        self.cpu_view_in(cpu, &words)
    }

    fn cpu_view_in(&self, cpu: u16, words: &AffinityWords) -> CpuView {
        let block = self.block(cpu);
        let tooltip = match block {
            Some(CpuBlock::Offline) => Some(words.offline_tooltip.clone()),
            Some(CpuBlock::Cpuset) => Some(words.cpuset_tooltip.clone()),
            _ => None,
        };
        let outside = matches!(block, Some(CpuBlock::Offline | CpuBlock::Cpuset));
        let mixed = !outside && self.state.get(cpu) == MIXED;
        let tooltip = tooltip.or_else(|| mixed.then(|| words.mixed_tooltip.clone()));
        CpuView {
            mixed,
            checked: !outside && self.state.get(cpu) == ON,
            disabled: block.is_some(),
            tooltip,
        }
    }

    // ---- Changes ----

    /// Rewrites the field with the canonical list of the checked CPUs.
    fn write_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cpu_list(&self.checked());
        self.list_invalid = false;
        // `set_value` emits no Change event, so this does not come back as a field edit.
        self.list_input
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// A field edit: a valid list of allowed CPUs replaces the grid, anything else marks the
    /// field invalid and leaves the grid at its last valid state.
    fn on_list_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.list_input.read(cx).text().to_string();
        match parse_cpu_list(&text, MAX_CPU) {
            Ok(set) if set.intersection(&self.allowed) == set => {
                self.state = CpuTriState::from_set(&set);
                self.list_invalid = false;
            }
            _ => self.list_invalid = true,
        }
        cx.notify();
    }

    /// Sets CPU `cpu` to `on` when it can be changed.
    pub fn set_cpu(&mut self, cpu: u16, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.block(cpu).is_some() {
            return;
        }
        self.state.set(cpu, if on { ON } else { OFF });
        self.write_list(window, cx);
        cx.notify();
    }

    /// A click on CPU `cpu`: MIXED to ON, ON to OFF, OFF to ON.
    pub fn cycle_cpu(&mut self, cpu: u16, window: &mut Window, cx: &mut Context<Self>) {
        let on = next_mark(self.state.get(cpu)) == ON;
        self.set_cpu(cpu, on, window, cx);
    }

    /// Reset: every CPU back to its mark when the dialog opened, MIXED included.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.state = self.initial_state.clone();
        self.write_list(window, cx);
        cx.notify();
    }

    /// Sets every allowed CPU in `cpus` to `on`.
    fn set_many(&mut self, cpus: CpuSet, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        for cpu in cpus.intersection(&self.allowed).iter() {
            self.state.set(cpu, if on { ON } else { OFF });
        }
        self.write_list(window, cx);
        cx.notify();
    }

    /// "All processors": checks every allowed CPU, or unchecks them all when all are checked.
    pub fn toggle_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = self.all_state() != Some(true);
        self.set_many(self.allowed, on, window, cx);
    }

    /// A group checkbox: checks the whole group, or unchecks it when fully checked.
    pub fn toggle_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cpus) = self.groups.get(group).map(|g| g.cpus) else {
            return;
        };
        let on = self.group_state(group) != Some(true);
        self.set_many(cpus, on, window, cx);
    }

    /// Sends one `SetAffinity` request per group of [`Self::resulting_masks`] and closes the
    /// dialog. Every request uses the first request's toast key, so one toast shows and each
    /// result replaces it (R-197). Does nothing while OK is disabled.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ok_enabled() {
            return;
        }
        let groups = self.resulting_masks();
        let words = affinity_words(cx);
        let threads = self
            .targets
            .iter()
            .all(|t| matches!(t, ActionTarget::Thread { .. }));
        let queue = self
            .parent_toasts
            .clone()
            .or_else(|| ToastQueue::of_window(window, cx).map(|q| q.downgrade()));
        match (
            cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()),
            queue,
        ) {
            (Some(tracker), Some(queue)) => {
                tracker.update(cx, |t, cx| {
                    let mut key = None;
                    for (mask, targets) in groups {
                        let id = t.next_id();
                        let key = *key.get_or_insert(ToastKey(id.0));
                        let texts = toast_texts(&words, targets.len(), threads, key);
                        let request = ActionRequest {
                            id,
                            kind: ActionKind::SetAffinity(mask),
                            targets: targets.into_iter().collect(),
                            privileged: false,
                        };
                        t.track_or_toast(request, queue.clone(), texts, cx);
                    }
                });
            }
            _ => warn!(target: "tasklens::ui", "affinity not sent: no action tracker or toasts"),
        }
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window) {
        window.remove_window();
    }

    // ---- Keys ----

    /// Runs `f` when the grid has focus, else lets the key go on (to the field or a button).
    fn in_grid(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>),
    ) {
        if self.grid_focus.is_focused(window) {
            f(self, window, cx);
        } else {
            cx.propagate();
        }
    }

    fn move_to(&mut self, mv: GridMove, window: &mut Window, cx: &mut Context<Self>) {
        self.in_grid(window, cx, |this, _, cx| {
            let next = move_cursor(&this.lines, this.cursor, mv);
            if next != this.cursor {
                this.cursor = next;
                cx.notify();
            }
        });
    }

    fn on_left(&mut self, _: &AffinityMoveLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::Left, window, cx);
    }

    fn on_right(&mut self, _: &AffinityMoveRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::Right, window, cx);
    }

    fn on_up(&mut self, _: &AffinityMoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::Up, window, cx);
    }

    fn on_down(&mut self, _: &AffinityMoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::Down, window, cx);
    }

    fn on_home(&mut self, _: &AffinityHome, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::Home, window, cx);
    }

    fn on_end(&mut self, _: &AffinityEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(GridMove::End, window, cx);
    }

    fn on_toggle(&mut self, _: &AffinityToggle, window: &mut Window, cx: &mut Context<Self>) {
        self.in_grid(window, cx, |this, window, cx| {
            if let Some(cpu) = this.cursor {
                this.cycle_cpu(cpu, window, cx);
            }
        });
    }

    fn on_toggle_group(
        &mut self,
        _: &AffinityToggleGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.in_grid(window, cx, |this, window, cx| {
            let group = this
                .cursor
                .and_then(|cpu| this.groups.iter().position(|g| g.cpus.contains(cpu)));
            if let Some(group) = group {
                this.toggle_group(group, window, cx);
            }
        });
    }

    /// Enter runs OK, except on Cancel, which Enter activates itself.
    fn on_apply(&mut self, _: &AffinityApply, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            cx.propagate();
            return;
        }
        self.apply(window, cx);
    }

    // ---- Layout ----

    /// The content height the dialog wants at `scale`, before the 360 to 640 clamp.
    fn natural_height(&self, scale: f32, gap: f32, pad: f32) -> f32 {
        let groups: f32 = self
            .groups
            .iter()
            .map(|g| {
                let rows = g.tiles.len().div_ceil(TILES_PER_ROW) as f32;
                GROUP_HEADER_H * scale
                    + gap
                    + rows * TILE_H * scale
                    + (rows - 1.).max(0.) * TILE_GAP * scale
            })
            .sum::<f32>()
            + gap * (self.groups.len().saturating_sub(1)) as f32;
        let mut body = (SUBTITLE_H + ALL_ROW_H) * scale + 1. + groups + 3. * gap;
        if self.list_invalid {
            body += HELPER_H * scale + gap;
        }
        if self.has_mixed() {
            body += HELPER_H * scale + gap;
        }
        if self.nothing_checked() {
            body += VALIDATION_H * scale + gap;
        }
        body + 2. * pad + FOOTER_H * scale
    }
}

/// The toast texts of one request with `n` targets, under the dialog's shared `key`.
fn toast_texts(words: &Rc<AffinityWords>, n: usize, threads: bool, key: ToastKey) -> ToastTexts {
    let success = if threads {
        words.toast_threads.clone()
    } else if n > 1 {
        words.toast_success_many.clone()
    } else {
        words.toast_success.clone()
    };
    let mut texts = ToastTexts::new(success.clone(), words.toast_failure.clone());
    texts.toast_key = Some(key);
    // Two or more targets: one summary toast on the shared key, not one toast per target.
    {
        let words = words.clone();
        texts.summary_min_targets = 2;
        texts.summary = Some(SummaryTexts {
            started_kind: ToastKind::Info,
            ..SummaryTexts::new(
                SharedString::default(),
                success,
                move |ok, total, failures: &[(u16, &ActionState)]| {
                    let reason = failures
                        .iter()
                        .find_map(|(_, state)| match state {
                            ActionState::Failed(err) => Some(short_reason(err)),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let kind = if ok == 0 {
                        ToastKind::Error
                    } else {
                        ToastKind::Warning
                    };
                    (kind, words.partial(ok, total, &reason, threads))
                },
            )
        });
    }
    texts
}

/// The process keys of `targets` (a thread target counts as its process).
fn target_keys(targets: &[ActionTarget]) -> impl Iterator<Item = ProcessKey> + '_ {
    targets.iter().filter_map(|t| match t {
        ActionTarget::Process(key) => Some(*key),
        ActionTarget::Thread { process, .. } => Some(*process),
        _ => None,
    })
}

impl Render for AffinityDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let words = affinity_words(cx);
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let view = cx.entity().downgrade();

        // Subtitle.
        let mut subtitle = div()
            .debug_selector(|| "AffinitySubtitle".into())
            .flex_none()
            .flex()
            .items_center()
            .h(px(SUBTITLE_H * s))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_secondary);
        *subtitle.text_style() = t_sm(&m);
        let subtitle = subtitle.child(if self.loading {
            words.loading(&self.subtitle)
        } else {
            self.subtitle.clone()
        });

        // "All processors" and the CPU list field.
        let all = {
            let view = view.clone();
            TlCheckbox::new("AffinityAll")
                .focus_handle(self.all_focus.clone())
                .checked(self.all_state())
                .label(words.all_processors.clone())
                .disabled(self.loading)
                .on_change(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.toggle_all(window, cx));
                })
        };
        let mut list_label = div().text_color(theme.text_secondary);
        *list_label.text_style() = t_sm(&m);
        let list_label = list_label.child(words.cpu_list.clone());
        let all_row = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .h(px(ALL_ROW_H * s))
            .child(all)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(m.sp[4])
                    .child(list_label)
                    .child(
                        TlTextInput::new("AffinityCpuList", &self.list_input)
                            .size(TlInputSize::Sm)
                            .width(px(LIST_W * s))
                            .invalid(self.list_invalid)
                            .aria_label(words.cpu_list.clone())
                            .placeholder(words.placeholder.clone()),
                    ),
            );
        let helper = self.list_invalid.then(|| {
            let mut helper = div()
                .debug_selector(|| "AffinityListHelper".into())
                .flex_none()
                .flex()
                .justify_end()
                .h(px(HELPER_H * s))
                .text_color(theme.status.danger);
            *helper.text_style() = t_xs(&m);
            helper.child(words.invalid_list.clone())
        });
        let mixed_helper = self.has_mixed().then(|| {
            let mut helper = div()
                .debug_selector(|| "AffinityMixedHelper".into())
                .flex_none()
                .flex()
                .justify_end()
                .h(px(HELPER_H * s))
                .text_color(theme.text_secondary);
            *helper.text_style() = t_xs(&m);
            helper.child(words.mixed_helper.clone())
        });

        // Groups and tiles.
        let grid_focused = self.grid_focus.is_focused(window);
        let mut grid = div()
            .id("AffinityGrid")
            .debug_selector(|| "AffinityGrid".into())
            .track_focus(&self.grid_focus)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .overflow_y_scroll();
        for (gi, group) in self.groups.iter().enumerate() {
            let group_check = {
                let view = view.clone();
                TlCheckbox::new(("AffinityGroup", gi))
                    .checked(self.group_state(gi))
                    .tab_stop(false)
                    .disabled(self.loading)
                    .on_change(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.toggle_group(gi, window, cx));
                    })
            };
            let mut label = div().text_color(theme.text_primary);
            *label.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
            let kind_icon = core_icon(group.label).map(|(icon, mark)| {
                div()
                    .debug_selector(move || format!("AffinityGroupIcon-{mark}"))
                    .flex_none()
                    .child(
                        Icon::new(icon)
                            .size(px(GROUP_ICON * s))
                            .text_color(theme.text_secondary),
                    )
            });
            let header = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(m.sp[3])
                .h(px(GROUP_HEADER_H * s))
                .child(group_check)
                .children(kind_icon)
                .child(label.child(words.group(group)));

            let tiles = group.tiles.iter().enumerate().map(|(ti, tile)| {
                let rows = [Some(tile.top), tile.bottom].into_iter().map(|cpu| {
                    let row = div().flex_none().flex().items_center().h(px(CPU_ROW_H * s));
                    let Some(cpu) = cpu else {
                        return row.into_any_element();
                    };
                    let cpu_view = self.cpu_view_in(cpu, &words);
                    let view = view.clone();
                    let check = TlCheckbox::new(("AffinityCpu", usize::from(cpu)))
                        .checked((!cpu_view.mixed).then_some(cpu_view.checked))
                        .label(words.cpu(cpu))
                        .label_style(CheckboxLabelStyle::SmallNumeric)
                        .tab_stop(false)
                        .disabled(cpu_view.disabled)
                        .on_change(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.cursor = Some(cpu);
                                this.cycle_cpu(cpu, window, cx);
                            });
                        });
                    let mut row = row
                        .id(("AffinityCpuRow", usize::from(cpu)))
                        .debug_selector(move || format!("AffinityCpuRow-{cpu}"))
                        .child(
                            div()
                                .debug_selector(move || format!("AffinityCpuCheck-{cpu}"))
                                .flex_none()
                                .child(check),
                        );
                    if grid_focused && self.cursor == Some(cpu) {
                        row = TlFocusRing::ring(row, m.radius_small, &theme);
                    }
                    match cpu_view.tooltip {
                        Some(tip) => {
                            let state = HoverTooltip::state(
                                ("AffinityCpuTip", usize::from(cpu)),
                                window,
                                cx,
                            );
                            HoverTooltip::attach(
                                &state,
                                row,
                                DELAY_CONTROL,
                                TlTooltip::text(tip).build(),
                                window,
                                cx,
                            )
                            .into_any_element()
                        }
                        None => row.into_any_element(),
                    }
                });
                let tile_div = div()
                    .id(ElementId::from(("AffinityTile", gi * 4096 + ti)))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .w(px(TILE_W * s))
                    .h(px(TILE_H * s))
                    // 2 px a side: the 72 px tile must hold a two-digit "CPU 31" checkbox (66 px).
                    .px(m.sp[1])
                    .overflow_hidden()
                    .rounded(m.radius_small)
                    .bg(theme.list_bg)
                    .children(rows);
                let state = HoverTooltip::state(("AffinityTileTip", gi * 4096 + ti), window, cx);
                HoverTooltip::attach(
                    &state,
                    tile_div,
                    DELAY_CONTROL,
                    TlTooltip::text(words.core_tooltip(tile)).build(),
                    window,
                    cx,
                )
                .into_any_element()
            });
            let tiles: Vec<AnyElement> = tiles.collect();
            grid = grid.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(m.sp[2])
                    .child(header)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(TILE_GAP * s))
                            .w(px((TILE_W * TILES_PER_ROW as f32
                                + TILE_GAP * (TILES_PER_ROW as f32 - 1.))
                                * s))
                            .children(tiles),
                    ),
            );
        }

        let validation = self.nothing_checked().then(|| {
            let mut text = div().text_color(theme.status.danger);
            *text.text_style() = t_sm(&m);
            div()
                .debug_selector(|| "AffinityValidation".into())
                .flex_none()
                .flex()
                .items_center()
                .gap(m.sp[3])
                .h(px(VALIDATION_H * s))
                .child(
                    Icon::new(IconName::CircleAlert)
                        .size(px(ALERT_ICON * s))
                        .text_color(theme.status.danger),
                )
                .child(text.child(words.validation.clone()))
        });

        // Footer.
        let cancel = {
            let view = view.clone();
            TlButton::new("AffinityCancel", TlButtonKind::Secondary)
                .label(words.cancel.clone())
                .focus_handle(self.cancel_focus.clone())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, _| this.cancel(window));
                })
        };
        let mut ok = {
            let view = view.clone();
            TlButton::new("AffinityOk", TlButtonKind::Primary)
                .label(words.ok.clone())
                .disabled(!self.ok_enabled())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.apply(window, cx));
                })
        };
        if self.nothing_checked() {
            ok = ok.tooltip(words.validation.clone());
        }
        let reset = self.reset_shown().then(|| {
            TlButton::new("AffinityReset", TlButtonKind::Ghost)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::RotateCcw)
                .icon_size(px(RESET_ICON * s))
                .label(words.reset.clone())
                .disabled(self.loading)
                .action(Box::new(AffinityReset))
        });
        let footer = div()
            .debug_selector(|| "AffinityFooter".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(m.sp[4])
            .h(px(FOOTER_H * s))
            .px(m.sp[6])
            .border_t_1()
            .border_color(theme.divider)
            .children(reset)
            .child(div().flex_1())
            .child(cancel)
            .child(ok);

        // The window height follows the content inside the 360 to 640 range; the group area
        // scrolls beyond that.
        let natural = px(self.natural_height(s, f32::from(m.sp[4]), f32::from(m.sp[6])));
        let fitted = self.fitted.clone();
        let fit = move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let want = (bounds.top() + natural)
                .clamp(px(AFFINITY_HEIGHT.0 * s), px(AFFINITY_HEIGHT.1 * s));
            if fitted.get() != Some(want) {
                fitted.set(Some(want));
                window.defer(cx, move |window, _| {
                    let width = window.bounds().size.width;
                    window.resize(size(width, want));
                });
            }
        };

        div()
            .id("AffinityDialog")
            .key_context(AFFINITY_DIALOG_CONTEXT)
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_home))
            .on_action(cx.listener(Self::on_end))
            .on_action(cx.listener(Self::on_toggle))
            .on_action(cx.listener(Self::on_toggle_group))
            .on_action(cx.listener(Self::on_apply))
            .on_action(cx.listener(|this, _: &AffinityReset, window, cx| {
                this.reset(window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.window_bg)
            .child(
                div()
                    .on_prepaint(fit)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(m.sp[4])
                    .p(m.sp[6])
                    .child(subtitle)
                    .child(all_row)
                    .children(helper)
                    .children(mixed_helper)
                    .child(div().flex_none().h(px(1.)).bg(theme.divider))
                    .child(grid)
                    .children(validation),
            )
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use proptest::prelude::*;
    use tl_model::system::CoreInfo;

    use super::*;

    fn set(cpus: &[u16]) -> CpuSet {
        let mut s = CpuSet::empty();
        for &c in cpus {
            s.insert(c);
        }
        s
    }

    fn fmt(s: &CpuSet) -> String {
        cpu_list(s)
    }

    #[test]
    fn parse_and_format_cpu_list() {
        for bad in [
            "", "a", "3-1", "0,,1", "0-x", "-1", "1-", "0 1", "1024", "+1",
        ] {
            assert!(parse_cpu_list(bad, MAX_CPU).is_err(), "{bad:?}");
        }
        assert!(parse_cpu_list("8", 7).is_err());
        let s = parse_cpu_list("0-7, 16-23", MAX_CPU).expect("valid");
        assert_eq!(fmt(&s), "0-7,16-23");
        assert_eq!(fmt(&set(&[0, 1, 2, 3, 5])), "0-3,5");
        assert_eq!(fmt(&set(&[1023])), "1023");
        assert_eq!(fmt(&CpuSet::empty()), "");
        assert_eq!(
            parse_cpu_list("4,0-1", MAX_CPU).map(|s| fmt(&s)),
            Ok("0-1,4".to_string())
        );
    }

    proptest! {
        #[test]
        fn format_parse_roundtrip(words in proptest::array::uniform16(any::<u64>()), keep in 0u32..64) {
            // Thin the bits so runs and gaps both occur.
            let mut s = CpuSet(words);
            for (i, w) in s.0.iter_mut().enumerate() {
                *w &= words[(i + 1) % 16].rotate_left(keep);
            }
            let text = fmt(&s);
            if s.is_empty() {
                prop_assert!(parse_cpu_list(&text, MAX_CPU).is_err());
            } else {
                let back = parse_cpu_list(&text, MAX_CPU).expect("canonical text parses");
                prop_assert_eq!(fmt(&back), text);
                prop_assert_eq!(back, s);
            }
        }
    }

    fn core(id: u16, logical: &[u16], l3: u16, kind: CoreKind) -> CoreInfo {
        CoreInfo {
            id,
            logical: SmallVec::from_slice(logical),
            l3_group: l3,
            kind,
        }
    }

    fn two_ccd() -> CpuTopology {
        let cores: Vec<CoreInfo> = (0..16)
            .map(|i| core(i, &[i, i + 16], i / 8, CoreKind::Standard))
            .collect();
        CpuTopology {
            online: set(&(0..32).collect::<Vec<_>>()),
            cores: Arc::from(cores),
        }
    }

    #[test]
    fn groups_by_l3_and_hybrid() {
        let groups = cpu_groups(&two_ccd());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, GroupLabel::Ccd(0));
        assert_eq!(fmt(&groups[0].cpus), "0-7,16-23");
        assert_eq!(fmt(&groups[1].cpus), "8-15,24-31");
        assert_eq!(groups[0].tiles.len(), 8);
        assert_eq!(
            (groups[0].tiles[0].top, groups[0].tiles[0].bottom),
            (0, Some(16))
        );

        let one = CpuTopology {
            online: set(&[0, 1, 2, 3]),
            cores: Arc::from(vec![
                core(0, &[0, 1], 0, CoreKind::Standard),
                core(1, &[2, 3], 0, CoreKind::Standard),
            ]),
        };
        let groups = cpu_groups(&one);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, GroupLabel::Cpus);

        let hybrid = CpuTopology {
            online: set(&[0, 1, 2, 3]),
            cores: Arc::from(vec![
                core(0, &[0, 1], 0, CoreKind::Performance),
                core(1, &[2], 0, CoreKind::Efficiency),
                core(2, &[3], 0, CoreKind::Efficiency),
            ]),
        };
        let groups = cpu_groups(&hybrid);
        let labels: Vec<GroupLabel> = groups.iter().map(|g| g.label).collect();
        assert_eq!(labels, [GroupLabel::PCores, GroupLabel::ECores]);
        assert_eq!(groups[1].tiles[0].bottom, None);
    }

    #[test]
    fn cursor_moves() {
        let lines = grid_lines(&cpu_groups(&two_ccd()));
        // Group 0: tops 0-7, bottoms 16-23; group 1: tops 8-15, bottoms 24-31.
        assert_eq!(lines.len(), 4);
        let mv = |c: u16, m: GridMove| move_cursor(&lines, Some(c), m);
        assert_eq!(mv(0, GridMove::Right), Some(1));
        assert_eq!(mv(0, GridMove::Left), Some(0));
        assert_eq!(mv(0, GridMove::Down), Some(16));
        assert_eq!(mv(16, GridMove::Down), Some(8));
        assert_eq!(mv(8, GridMove::Up), Some(16));
        assert_eq!(mv(5, GridMove::End), Some(31));
        assert_eq!(mv(31, GridMove::Home), Some(0));
        assert_eq!(move_cursor(&lines, None, GridMove::Down), Some(0));
    }
}
