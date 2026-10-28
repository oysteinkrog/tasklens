//! The Overview page's opt-in Top processes box (UI-PRF-017, D-022, crw.45): the 10 busiest
//! processes by CPU, one row each with name, PID and a CPU meter.
//!
//! The page builds the box only while the saved layout (`[performance.overview]`) places it,
//! and only then asks for `Demand.process_top`. On each `PROCESSES` change the box picks the top rows with
//! a bounded selection over the records (no sort of the whole list) and hashes what the rows
//! show. It formats text and notifies only when that hash changes, so a tick with the same top
//! 10 at the same whole percents renders nothing. A click on a row selects that process on the
//! Processes tab. Pointer movement never notifies.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::fmt::Write as _;
use std::hash::{Hash as _, Hasher as _};
#[cfg(any(test, feature = "test-hooks"))]
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use rustc_hash::FxHasher;
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::ids::ProcessKey;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::ProcessSnapshot;

use super::layout::BoxMode;
use super::parts::{Meter, box_frame, edit_chrome};
use super::picked::{box_mask, box_snapshot};
use super::range::RangeText;
use super::timeline::OverviewPause;
use crate::chart::labels::LABEL_ROW;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::text::style::{numeric_style, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::main_window_view;

/// The box's debug selector.
pub const PROCS_BOX_ID: &str = "perf-overview-box-processes";

/// The box's layout modes (UI-PRF-019). `list` is the only one.
pub const MODES: &[BoxMode] = &[BoxMode {
    id: "list",
    label: "perf-overview-mode-list",
    split: None,
}];

/// The prefix of each process row's debug selector, followed by the PID.
pub const PROC_ROW_ID_PREFIX: &str = "perf-overview-proc-row:";

/// Rows shown.
pub const TOP_N: usize = 10;

const KEY_TITLE: &str = "perf-overview-box-processes";
const KEY_NAME: &str = "processes-column-name";
const KEY_PID: &str = "processes-column-pid";
const KEY_CPU: &str = "processes-column-cpu";

/// The PID column, logical px before `ui_scale`.
const PID_W: f32 = 64.;
/// The CPU text column, logical px before `ui_scale`.
const CPU_W: f32 = 48.;
/// The CPU meter column, logical px before `ui_scale`.
const METER_W: f32 = 64.;

/// One process row.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcRow {
    pub key: ProcessKey,
    pub name: SharedString,
    pub pid: SharedString,
    pub cpu: SharedString,
    /// CPU as a fraction of the machine.
    pub fraction: f32,
}

/// The indexes of the `n` live records with the highest CPU, highest first. Ties keep record
/// order, which is `ProcessKey` order, as the engine breaks ties.
pub fn top_by_cpu(records: &[ProcessRecord], n: usize) -> SmallVec<[usize; TOP_N]> {
    let mut top: SmallVec<[(f32, usize); TOP_N]> = SmallVec::new();
    for (i, r) in records.iter().enumerate() {
        if r.flags.contains(ProcFlags::EXITED) || !r.cpu_pct.is_finite() {
            continue;
        }
        let cpu = r.cpu_pct;
        if top.len() == n && top.last().is_none_or(|&(c, _)| cpu <= c) {
            continue;
        }
        let at = top.iter().position(|&(c, _)| cpu > c).unwrap_or(top.len());
        if top.len() == n {
            top.pop();
        }
        top.insert(at, (cpu, i));
    }
    top.into_iter().map(|(_, i)| i).collect()
}

/// The CPU percent shown, whole percents.
fn cpu_whole(pct: f32) -> u32 {
    pct.clamp(0., 100.).round() as u32
}

/// The Top processes box.
pub struct ProcsBox {
    rows: SmallVec<[ProcRow; TOP_N]>,
    title: SharedString,
    header: [SharedString; 3],
    /// Hash of the shown rows' keys, names and whole CPU percents.
    rows_bits: u64,
    style_version: u64,
    strings_version: u64,
    hub: Entity<SnapshotHub>,
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
    /// The process the last row click asked the Processes tab to select.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_jump: Rc<Cell<Option<ProcessKey>>>,
}

impl ProcsBox {
    pub fn new(hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let snap = box_snapshot(hub, cx);
        let mut this = Self {
            rows: SmallVec::new(),
            title: tr(cx, KEY_TITLE),
            header: [tr(cx, KEY_NAME), tr(cx, KEY_PID), tr(cx, KEY_CPU)],
            rows_bits: 0,
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            hub: hub.clone(),
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_pause),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_jump: Rc::new(Cell::new(None)),
        };
        this.sync_rows(&snap.processes, true);
        this
    }

    pub fn rows(&self) -> &[ProcRow] {
        &self.rows
    }

    /// "Top processes", the column names, then "name\tPID\tCPU" per row.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\n');
        for (i, h) in self.header.iter().enumerate() {
            if i > 0 {
                out.push('\t');
            }
            out.push_str(h);
        }
        out.push('\n');
        for r in &self.rows {
            let _ = writeln!(out, "{}\t{}\t{}", r.name, r.pid, r.cpu);
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if !box_mask(ev.mask, cx).contains(ComponentMask::PROCESSES) {
            return;
        }
        let snap = box_snapshot(&hub, cx);
        if self.sync_rows(&snap.processes, false) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The page was held at a time or made live. The device history keeps no processes, so
    /// while paused the box has no rows.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = box_snapshot(&self.hub, cx);
        if self.sync_rows(&snap.processes, false) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// New rows when what they show changed, or always with `force`. True when the rows were
    /// built again.
    fn sync_rows(&mut self, procs: &ProcessSnapshot, force: bool) -> bool {
        let top = top_by_cpu(&procs.records, TOP_N);
        let mut h = FxHasher::default();
        for &i in &top {
            let r = &procs.records[i];
            r.key.hash(&mut h);
            r.meta.name.hash(&mut h);
            h.write_u32(cpu_whole(r.cpu_pct));
        }
        let bits = h.finish();
        if !force && bits == self.rows_bits {
            return false;
        }
        self.rows_bits = bits;
        self.rows = top
            .iter()
            .map(|&i| {
                let r = &procs.records[i];
                ProcRow {
                    key: r.key,
                    name: SharedString::from(r.meta.name.to_string()),
                    pid: r.key.pid.to_string().into(),
                    cpu: format!("{}%", cpu_whole(r.cpu_pct)).into(),
                    fraction: (r.cpu_pct / 100.).clamp(0., 1.),
                }
            })
            .collect();
        true
    }

    /// A new theme or UI scale.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        cx.notify();
    }

    /// A new language: new title and column names.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        self.header = [tr(cx, KEY_NAME), tr(cx, KEY_PID), tr(cx, KEY_CPU)];
        let snap = box_snapshot(&self.hub, cx);
        self.sync_rows(&snap.processes, true);
        cx.notify();
    }
}

/// Selects `key` on the Processes tab (PROC-071). Nothing happens when that tab is not built.
fn select_process(key: ProcessKey, window: &mut Window, cx: &mut App) {
    let page =
        main_window_view(cx).and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes));
    if let Some(page) = page {
        page.update(cx, |p, cx| {
            p.select_by_key(key, JumpTexts::default(), window, cx)
        });
    }
}

/// One row: name, PID, CPU text and meter. `key` `None` is the column header.
fn row_element(
    key: Option<ProcessKey>,
    texts: [&SharedString; 3],
    fraction: Option<f32>,
    m: &UiMetrics,
    theme: &TlTheme,
    #[cfg(any(test, feature = "test-hooks"))] last_jump: &Rc<Cell<Option<ProcessKey>>>,
) -> AnyElement {
    let s = m.ui_scale;
    let header = key.is_none();
    let (color, style) = if header {
        (theme.text_secondary, t_xs(m))
    } else {
        (theme.text_primary, numeric_style(m, m.text.sm))
    };
    let right = |text: &SharedString, w: f32| {
        let mut d = div()
            .flex_none()
            .w(px(w * s))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_color(color)
            .child(text.clone());
        *d.text_style() = style.clone();
        d
    };
    let mut name = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(if header {
            theme.text_secondary
        } else {
            theme.text_primary
        })
        .child(texts[0].clone());
    *name.text_style() = if header { t_xs(m) } else { t_sm(m) };
    let meter = div().flex_none().w(px(METER_W * s));
    let meter = match fraction {
        Some(f) => meter.child(Meter {
            fraction: Some(f),
            color: theme.graph.cpu_line,
        }),
        None => meter,
    };
    let row = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * s))
        .child(name)
        .child(right(texts[1], PID_W))
        .child(right(texts[2], CPU_W))
        .child(meter);
    let Some(key) = key else {
        return row.into_any_element();
    };
    #[cfg(any(test, feature = "test-hooks"))]
    let last_jump = Rc::clone(last_jump);
    row.debug_selector(move || format!("{PROC_ROW_ID_PREFIX}{}", key.pid))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            #[cfg(any(test, feature = "test-hooks"))]
            last_jump.set(Some(key));
            select_process(key, window, cx);
        })
        .into_any_element()
}

impl Render for ProcsBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let mut rows: SmallVec<[AnyElement; TOP_N + 1]> = SmallVec::new();
        let [n, p, c] = &self.header;
        rows.push(row_element(
            None,
            [n, p, c],
            None,
            &m,
            &theme,
            #[cfg(any(test, feature = "test-hooks"))]
            &self.last_jump,
        ));
        for r in &self.rows {
            rows.push(row_element(
                Some(r.key),
                [&r.name, &r.pid, &r.cpu],
                Some(r.fraction),
                &m,
                &theme,
                #[cfg(any(test, feature = "test-hooks"))]
                &self.last_jump,
            ));
        }
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .children(rows)
            .into_any_element();
        box_frame(
            PROCS_BOX_ID,
            None,
            self.title.clone(),
            SharedString::default(),
            SharedString::default(),
            &RangeText::default(),
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .size_full()
    }
}
