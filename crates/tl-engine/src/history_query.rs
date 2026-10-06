//! Answers to `Command::HistoryQuery` (ADR-012 "Reading", PROC-088 to PROC-091, R-103).
//!
//! The answers read whichever history is active: the store while recording is on, the rolling
//! window while it is off. `answer_frame` builds the `HistoryView` rows, totals, series, stack,
//! overview and gaps; `answer_top` builds the top-K hover. The history worker runs both on the
//! blocking pool, merges the last `HistoryTop` into the last view and sends it to the reducer.
//!
//! Range rules: `Live` describes the newest second, `At(t)` the second `t` falls in, clamped
//! into the retained span, and `Span` the averages and peaks over the range. The rows of a second
//! show the last sweep inside it. Deleted keys never appear in rows, series or the top list.
//!
//! A store read error answers with what could be read and `store_state =
//! Problem(EmptyOrTruncated)` (R-249). History data holds no command line (ADR-015).

use std::borrow::Cow;
use std::sync::Arc;

use tl_history::error::HistoryError;
use tl_history::format::{BLOCK_SECS, Block, HistMeta, MetaId, SEGMENT_SECS};
use tl_history::read::{self, DeletedKeys, HistProcessF, MetaTable, Metric, Point};
use tl_history::rolling::RollingWindow;
use tl_history::store::Store;
use tl_model::command::HistoryWhat;
use tl_model::history::{
    HistoryLoadProblem, HistoryMetric, HistoryPeaks, HistoryPoint, HistoryProcessRow, HistoryRange,
    HistorySeries, HistorySource, HistoryStackBucket, HistoryStoreState, HistoryTop, HistoryTopRow,
    HistoryTotals, HistoryView,
};
use tl_model::ids::ProcessKey;
use tl_model::snapshot::ComponentHeader;
use tl_model::time::{BootId, TimeNs};

const NS_PER_SEC: u64 = 1_000_000_000;
const NS_PER_MS: u64 = 1_000_000;
const BLOCK_NS: u64 = BLOCK_SECS * NS_PER_SEC;
const SEGMENT_NS: u64 = SEGMENT_SECS * NS_PER_SEC;
/// Most per-process series in one view (PROC-088).
const MAX_KEY_SERIES: usize = 8;
/// Seconds per `system_series` call when averaging: short enough for 1 s buckets.
const MEAN_CHUNK_NS: u64 = 1000 * NS_PER_SEC;
/// The recorder's open block gets meta ids from here up, clear of the store's ids.
const OPEN_META_BASE: MetaId = 1 << 31;

/// The history a query reads.
pub(crate) enum QuerySource<'a> {
    None,
    Rolling(&'a RollingWindow),
    /// `open` is the recorder's open block (`Recorder::open_snapshot`), not yet in the store.
    Store {
        store: &'a Store,
        open: Option<Block>,
    },
}

/// Worker state every answer needs.
pub(crate) struct QueryCtx {
    /// Rows of another boot get `previous_boot = true`.
    pub boot: BootId,
    /// Boottime of the query. `At(t)` past it describes the newest second.
    pub now_t_ns: TimeNs,
    pub store_state: HistoryStoreState,
    pub realtime_offset_ns: i64,
    /// `header.version` of the answer.
    pub version: u64,
}

/// The second the rows describe, or the range they average.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sel {
    /// `eval` is the last nanosecond of the second, clamped to the span end.
    At {
        frame_t: TimeNs,
        eval: TimeNs,
    },
    Span {
        from: TimeNs,
        to: TimeNs,
    },
}

impl Sel {
    fn bounds(self) -> (TimeNs, TimeNs) {
        match self {
            Sel::At { eval, .. } => (eval, eval),
            Sel::Span { from, to } => (from, to),
        }
    }
}

fn select(range: HistoryRange, span: (TimeNs, TimeNs), now: TimeNs) -> Sel {
    let at = |t: TimeNs| {
        let t = t.clamp(span.0, span.1);
        let frame_t = t / NS_PER_SEC * NS_PER_SEC;
        Sel::At {
            frame_t,
            eval: frame_t.saturating_add(NS_PER_SEC - 1).min(span.1),
        }
    };
    match range {
        HistoryRange::Live => at(span.1),
        HistoryRange::At(t) => at(t.min(now)),
        HistoryRange::Span { from, to } => Sel::Span {
            from: from.min(to).max(span.0),
            to: from.max(to).min(span.1),
        },
    }
}

/// Blocks loaded for one answer, in time order, with the metas and deleted keys to read them.
struct Data<'a> {
    blocks: Vec<Arc<Block>>,
    metas: Cow<'a, MetaTable>,
    deleted: Cow<'a, DeletedKeys>,
    /// A store read failed; the blocks hold what could be read.
    read_failed: bool,
}

impl Data<'_> {
    fn iter(&self) -> impl DoubleEndedIterator<Item = &Block> + Clone {
        self.blocks.iter().map(|b| &**b)
    }

    /// The newest block that covers `t`.
    fn block_at(&self, t: TimeNs) -> Option<&Block> {
        self.iter()
            .rev()
            .find(|b| b.start_t_ns <= t && t < b.start_t_ns.saturating_add(BLOCK_NS))
    }
}

/// Oldest and newest retained time.
fn span(src: &QuerySource) -> Option<(TimeNs, TimeNs)> {
    match src {
        QuerySource::None => None,
        QuerySource::Rolling(window) => window.span(),
        QuerySource::Store { store, open } => {
            let open = open.as_ref().map(|b| (b.start_t_ns, block_end(b)));
            match (store.span(), open) {
                (Some(s), Some(o)) => Some((s.0.min(o.0), s.1.max(o.1))),
                (s, o) => s.or(o),
            }
        }
    }
}

/// Time of the last event in a block.
fn block_end(b: &Block) -> TimeNs {
    let at = |ms: u32| {
        b.start_t_ns
            .saturating_add(u64::from(ms).saturating_mul(NS_PER_MS))
    };
    let sweep = b.deltas.last().map_or(b.start_t_ns, |d| at(d.dt_ms));
    let system = b
        .system
        .iter()
        .filter_map(|(_, points)| points.last().map(|(ms, _)| at(*ms)));
    system.fold(sweep, TimeNs::max)
}

/// Loads the blocks that overlap [from, to], plus the earlier blocks whose catalog the system
/// totals need: the whole rolling window, or one segment back in the store, since the first
/// block of each segment carries the catalog.
fn load<'s>(src: &'s QuerySource<'_>, from: TimeNs, to: TimeNs) -> Data<'s> {
    let empty = || Data {
        blocks: Vec::new(),
        metas: Cow::Owned(MetaTable::default()),
        deleted: Cow::Owned(DeletedKeys::default()),
        read_failed: false,
    };
    match src {
        QuerySource::None => empty(),
        QuerySource::Rolling(window) => match window.blocks(0, to) {
            Ok((blocks, metas)) => Data {
                blocks,
                metas: Cow::Borrowed(metas),
                deleted: Cow::Borrowed(window.deleted()),
                read_failed: false,
            },
            Err(e) => {
                tracing::debug!("history window read failed: {e}");
                Data {
                    deleted: Cow::Borrowed(window.deleted()),
                    read_failed: true,
                    ..empty()
                }
            }
        },
        QuerySource::Store { store, open } => {
            let mut read_failed = false;
            let (mut blocks, mut metas) = match store.blocks(from.saturating_sub(SEGMENT_NS), to) {
                Ok(read) => read,
                Err(e) => {
                    // The error text holds a user path: debug level only.
                    tracing::debug!("history store read failed: {e}");
                    read_failed = true;
                    (Vec::new(), MetaTable::default())
                }
            };
            let open = open
                .as_ref()
                .filter(|b| b.start_t_ns <= to && b.start_t_ns.saturating_add(BLOCK_NS) > from);
            if let Some(open) = open {
                match open_block(store, open, &mut metas) {
                    Ok(b) => blocks.push(Arc::new(b)),
                    Err(e) => {
                        tracing::debug!("history store read failed: {e}");
                        read_failed = true;
                    }
                }
            }
            Data {
                blocks,
                metas: Cow::Owned(metas),
                deleted: Cow::Borrowed(store.deleted()),
                read_failed,
            }
        }
    }
}

/// The recorder's open block with every meta id moved up by `OPEN_META_BASE`, and those metas
/// added to `metas`. The open block shares the meta scope of the store's newest segment, unless
/// it starts a new segment.
fn open_block(store: &Store, open: &Block, metas: &mut MetaTable) -> Result<Block, HistoryError> {
    let mut scope = MetaTable::default();
    if !store.will_rotate(open.start_t_ns)
        && let Some((_, end)) = store.span()
    {
        // Only the newest stored block covers `end`, so only its segment is read, unshifted.
        scope = store.blocks(end, end)?.1;
    }
    scope.absorb(open);
    let mut b = open.clone();
    let mut new_meta: Vec<(MetaId, HistMeta)> = Vec::new();
    let started = b.deltas.iter_mut().flat_map(|d| d.started.iter_mut());
    for p in b.keyframe.iter_mut().chain(started) {
        let id = p.meta.saturating_add(OPEN_META_BASE);
        if let Some(m) = scope.get(p.meta) {
            new_meta.push((id, m.clone()));
        }
        p.meta = id;
    }
    b.new_meta = new_meta;
    metas.absorb(&b);
    Ok(b)
}

/// The view with source, state, span and header filled, and nothing else.
fn base_view(
    src: &QuerySource,
    ctx: &QueryCtx,
    id: u64,
    range: HistoryRange,
    span: Option<(TimeNs, TimeNs)>,
) -> HistoryView {
    let source = match src {
        QuerySource::None => HistorySource::None,
        QuerySource::Rolling(_) => HistorySource::RollingWindow,
        QuerySource::Store { .. } => HistorySource::Store,
    };
    let (observed_start, observed_end) = span.unwrap_or_default();
    HistoryView {
        header: ComponentHeader {
            version: ctx.version,
            observed_start,
            observed_end,
            ..ComponentHeader::default()
        },
        query_id: id,
        source,
        store_state: ctx.store_state,
        span,
        range,
        realtime_offset_ns: ctx.realtime_offset_ns,
        ..HistoryView::default()
    }
}

/// Answers a Frame query. Any other `what` gets the view without rows or series. With no data
/// yet, `rows` is empty and `span` and `frame_t` are `None` (PROC-091).
pub(crate) fn answer_frame(
    src: QuerySource,
    ctx: &QueryCtx,
    id: u64,
    range: &HistoryRange,
    what: &HistoryWhat,
) -> HistoryView {
    let span = span(&src);
    let mut view = base_view(&src, ctx, id, *range, span);
    let (HistoryWhat::Frame { view: v, .. }, Some(span)) = (what, span) else {
        return view;
    };
    let sel = select(*range, span, ctx.now_t_ns);
    let (lo, hi) = sel.bounds();
    let mut from = v.0.min(v.1).min(lo);
    let to = v.0.max(v.1).max(hi);
    if matches!(what, HistoryWhat::Frame { overview: true, .. }) {
        from = from.min(span.0);
    }
    let data = load(&src, from, to);
    fill_frame(&mut view, &data, ctx, sel, what);
    view
}

/// Fills rows, totals, series, stack, overview and gaps of a view whose `span` is set.
fn fill_frame(view: &mut HistoryView, data: &Data, ctx: &QueryCtx, sel: Sel, what: &HistoryWhat) {
    let HistoryWhat::Frame {
        metric,
        view: v,
        keys,
        overview,
    } = what
    else {
        return;
    };
    let Some(span) = view.span else {
        return;
    };
    if data.read_failed {
        view.store_state = HistoryStoreState::Problem(HistoryLoadProblem::EmptyOrTruncated);
    }
    let m = Metric::from(*metric);
    let deleted = &*data.deleted;
    let mut rows = Vec::new();
    match sel {
        Sel::At { frame_t, eval } => {
            view.frame_t = Some(frame_t);
            let block = data.block_at(eval);
            if let Some(frame) = block.and_then(|b| read::state_at(b, eval)) {
                let live = frame.procs.iter().filter(|p| !deleted.contains(&p.key));
                for p in live {
                    let v = HistProcessF::from_stored(p);
                    rows.push(row(p.key, data.metas.get(p.meta), &v, None, ctx));
                }
            }
            view.totals = block.map(|b| HistoryTotals {
                cpu_pct: system_at(data, Metric::Cpu, frame_t),
                mem_used_bytes: system_at(data, Metric::Memory, frame_t) as u64,
                mem_total_bytes: b.mem_total_kib.saturating_mul(1024),
            });
        }
        Sel::Span { from, to } => {
            for s in read::range_stats(data.iter(), from, to, deleted) {
                let peak = HistoryPeaks {
                    cpu_pct: s.peak.cpu_pct,
                    mem_anon_bytes: s.peak.mem_anon_bytes,
                    disk_bps: s.peak.disk_bps,
                    net_bps: s.peak.net_bps,
                    gpu_pct: s.peak.gpu_pct,
                };
                rows.push(row(s.key, data.metas.get(s.meta), &s.avg, Some(peak), ctx));
            }
            view.totals = span_totals(data, from, to);
        }
    }
    view.process_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    // Selected keys absent from the rows come last, with zero values (the UI shows "-").
    for k in keys.iter().filter(|k| !deleted.contains(k)) {
        if rows.iter().any(|r| r.key == *k) {
            continue;
        }
        let meta = find_meta(data, *k).and_then(|id| data.metas.get(id));
        let mut r = row(*k, meta, &HistProcessF::default(), None, ctx);
        r.present = false;
        rows.push(r);
    }
    view.rows = rows.into();

    let (from, to) = (v.0.min(v.1), v.0.max(v.1));
    let system = read::system_series(data.iter(), m, from, to);
    let width = read::bucket_width_ns(from, to);
    view.gaps = gaps(&system, data, from, to, width).into();
    let mut series = vec![HistorySeries {
        key: None,
        metric: *metric,
        points: points(&system),
    }];
    let selected = keys.iter().filter(|k| !deleted.contains(k));
    for k in selected.take(MAX_KEY_SERIES) {
        let p = read::process_series(data.iter(), *k, m, from, to, deleted);
        series.push(HistorySeries {
            key: Some(*k),
            metric: *metric,
            points: points(&p),
        });
    }
    view.series = series.into();
    view.stack = read::stack(data.iter(), m, from, to, deleted)
        .into_iter()
        .map(|(t_ns, top, other)| HistoryStackBucket { t_ns, top, other })
        .collect();
    if *overview {
        view.overview = points(&read::system_series(data.iter(), m, span.0, span.1));
    }
}

/// Answers a TopAt query: the `n` largest processes at the second `t` falls in.
pub(crate) fn answer_top(
    src: QuerySource,
    ctx: &QueryCtx,
    id: u64,
    t: TimeNs,
    metric: HistoryMetric,
    n: u8,
) -> HistoryTop {
    let mut top = HistoryTop {
        query_id: id,
        t_ns: t / NS_PER_SEC * NS_PER_SEC,
        metric,
        cpu_total_pct: 0.0,
        mem_total_bytes: 0,
        rows: Arc::from([]),
    };
    let Some(span) = span(&src) else {
        return top;
    };
    let Sel::At { frame_t, eval } = select(HistoryRange::At(t), span, ctx.now_t_ns) else {
        return top;
    };
    top.t_ns = frame_t;
    let data = load(&src, eval, eval);
    fill_top(&mut top, &data, eval, n);
    top
}

fn fill_top(top: &mut HistoryTop, data: &Data, eval: TimeNs, n: u8) {
    let Some(block) = data.block_at(eval) else {
        return;
    };
    top.cpu_total_pct = system_at(data, Metric::Cpu, top.t_ns);
    top.mem_total_bytes = block.mem_total_kib.saturating_mul(1024);
    let Some(frame) = read::state_at(block, eval) else {
        return;
    };
    let ranked = read::top_at(&frame, top.metric.into(), usize::from(n), &data.deleted);
    top.rows = ranked
        .into_iter()
        .map(|(key, meta, value)| {
            let meta = data.metas.get(meta);
            HistoryTopRow {
                key,
                name: meta.map_or_else(|| Arc::from(""), |m| m.name.clone()),
                app_id: meta.and_then(|m| m.app_id.clone()),
                value,
            }
        })
        .collect();
}

fn row(
    key: ProcessKey,
    meta: Option<&HistMeta>,
    v: &HistProcessF,
    peak: Option<HistoryPeaks>,
    ctx: &QueryCtx,
) -> HistoryProcessRow {
    let text = |f: fn(&HistMeta) -> &Arc<str>| meta.map_or_else(|| Arc::from(""), |m| f(m).clone());
    HistoryProcessRow {
        key,
        name: text(|m| &m.name),
        comm: text(|m| &m.comm),
        user: text(|m| &m.user),
        exe: meta.and_then(|m| m.exe_path.clone()),
        app_id: meta.and_then(|m| m.app_id.clone()),
        system: meta.is_some_and(|m| m.system),
        present: true,
        previous_boot: key.boot_id != ctx.boot,
        cpu_pct: v.cpu_pct,
        mem_anon_bytes: v.mem_anon_bytes,
        disk_bps: v.disk_bps,
        net_bps: v.net_bps,
        gpu_pct: v.gpu_pct,
        peak,
    }
}

/// Meta id of the newest stored state of `key` in the loaded blocks.
fn find_meta(data: &Data, key: ProcessKey) -> Option<MetaId> {
    data.iter().rev().find_map(|b| {
        let started = b.deltas.iter().rev().flat_map(|d| d.started.iter());
        started
            .chain(b.keyframe.iter())
            .find(|p| p.key == key)
            .map(|p| p.meta)
    })
}

/// System total of `m` in the second starting at `t`. 0 when there is no value.
fn system_at(data: &Data, m: Metric, t: TimeNs) -> f32 {
    let end = t.saturating_add(NS_PER_SEC - 1);
    read::system_series(data.iter(), m, t, end)
        .last()
        .map(|p| p.max)
        .filter(|v| !v.is_nan())
        .unwrap_or(0.0)
}

/// Mean of the per-second system total of `m` over [from, to], gaps left out.
fn system_mean(data: &Data, m: Metric, from: TimeNs, to: TimeNs) -> Option<f64> {
    let (mut sum, mut n) = (0.0f64, 0u64);
    let mut start = from;
    while start <= to {
        let end = start.saturating_add(MEAN_CHUNK_NS - 1).min(to);
        // Under 1,023 s per call, every bucket is one second, so `max` is the value.
        for p in read::system_series(data.iter(), m, start, end) {
            if !p.max.is_nan() {
                sum += f64::from(p.max);
                n += 1;
            }
        }
        if end == to {
            break;
        }
        start = end + 1;
    }
    (n > 0).then(|| sum / n as f64)
}

/// Averages over a span (R-169 OI-5, D-020). `None` when no block overlaps it.
fn span_totals(data: &Data, from: TimeNs, to: TimeNs) -> Option<HistoryTotals> {
    let mem_total_kib = data
        .iter()
        .filter(|b| b.start_t_ns <= to && b.start_t_ns.saturating_add(BLOCK_NS) > from)
        .map(|b| b.mem_total_kib)
        .max()?;
    Some(HistoryTotals {
        cpu_pct: system_mean(data, Metric::Cpu, from, to).unwrap_or(0.0) as f32,
        mem_used_bytes: system_mean(data, Metric::Memory, from, to).unwrap_or(0.0) as u64,
        mem_total_bytes: mem_total_kib.saturating_mul(1024),
    })
}

/// NaN buckets of the system series and the time before every `gap_before` block, inside
/// [from, to], sorted and merged.
fn gaps(
    system: &[Point],
    data: &Data,
    from: TimeNs,
    to: TimeNs,
    width: u64,
) -> Vec<(TimeNs, TimeNs)> {
    let mut out: Vec<(TimeNs, TimeNs)> = system
        .iter()
        .filter(|p| p.max.is_nan())
        .map(|p| (p.t_ns, p.t_ns.saturating_add(width)))
        .collect();
    for pair in data.blocks.windows(2) {
        if pair[1].gap_before {
            out.push((block_end(&pair[0]), pair[1].start_t_ns));
        }
    }
    let mut clipped: Vec<(TimeNs, TimeNs)> = out
        .into_iter()
        .map(|(a, b)| (a.max(from), b.min(to)))
        .filter(|(a, b)| a < b)
        .collect();
    clipped.sort_unstable();
    let mut merged: Vec<(TimeNs, TimeNs)> = Vec::with_capacity(clipped.len());
    for (a, b) in clipped {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

fn points(p: &[Point]) -> Arc<[HistoryPoint]> {
    p.iter()
        .map(|p| HistoryPoint {
            t_ns: p.t_ns,
            min: p.min,
            max: p.max,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_history::block::BlockBuilder;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::history::{HISTORY_MAX_POINTS, RecordSink};
    use tl_model::ids::{DeviceIdx, GroupKey, MetricId, SeriesId};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::series::Sample;
    use tl_model::validity::{FieldValidity, Validity};

    const SEC: u64 = NS_PER_SEC;
    const BOOT: BootId = BootId(7);
    /// Start of the synthetic history.
    const T0: u64 = 1000 * SEC;

    fn key(i: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + u64::from(i),
            boot_id: BOOT,
            pid: 100 + i,
        }
    }

    fn record(k: ProcessKey, cpu_pct: f32, mem_mib: u64) -> ProcessRecord {
        let text: Arc<str> = Arc::from(format!("proc{}", k.pid));
        ProcessRecord {
            key: k,
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text.clone(),
                name: text.clone(),
                exe: Some(Arc::from(format!("/usr/bin/proc{}", k.pid))),
                cmdline: None,
                user: Arc::from("alice"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                search_lower: text,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::default(),
            nice: 0,
            threads: 1,
            cpu_pct,
            rss_anon_bytes: mem_mib << 20,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from("dev"),
            net_kind: None,
        }
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
            ]),
        }
    }

    fn system(t: u64, cpu: f32, mem_used: f32) -> [(SeriesId, Sample); 2] {
        let id = |device, metric| SeriesId {
            device: DeviceIdx(device),
            metric,
        };
        [
            (id(0, MetricId::CPU_TOTAL), Sample::valid(t, cpu)),
            (id(1, MetricId::MEM_USED), Sample::valid(t, mem_used)),
        ]
    }

    fn ctx() -> QueryCtx {
        QueryCtx {
            boot: BOOT,
            now_t_ns: u64::MAX,
            store_state: HistoryStoreState::Off,
            realtime_offset_ns: 5,
            version: 3,
        }
    }

    /// A window with one sweep and one system sample per second for `secs` seconds from `T0`.
    /// `sweep(i)` gives the processes of second `i`.
    fn window(secs: u64, sweep: impl Fn(u64) -> Vec<ProcessRecord>) -> RollingWindow {
        let mut w = RollingWindow::new(BOOT);
        w.set_mem_total_kib(8 << 20);
        w.on_device_catalog(&catalog());
        for i in 0..secs {
            let t = T0 + i * SEC;
            w.on_system_sample(t, &system(t, 25.0, 2e9));
            w.on_process_sweep(t, &Arc::from(sweep(i)));
        }
        w
    }

    /// Four steady processes; key(9) lives for the first `ends` seconds.
    fn steady(ends: u64) -> impl Fn(u64) -> Vec<ProcessRecord> {
        move |i| {
            let mut v: Vec<_> = (0..4u32)
                .map(|k| record(key(k), (k + 1) as f32, 10 * u64::from(k + 1)))
                .collect();
            if i < ends {
                v.push(record(key(9), 50.0, 5));
            }
            v
        }
    }

    fn frame(metric: HistoryMetric, view: (TimeNs, TimeNs), keys: &[ProcessKey]) -> HistoryWhat {
        HistoryWhat::Frame {
            metric,
            view,
            keys: Arc::from(keys),
            overview: false,
        }
    }

    #[test]
    fn live_frame_from_window() {
        let w = window(300, steady(200));
        let last = T0 + 299 * SEC;
        let what = frame(HistoryMetric::Cpu, (T0, last), &[]);
        let v = answer_frame(
            QuerySource::Rolling(&w),
            &ctx(),
            11,
            &HistoryRange::Live,
            &what,
        );
        assert_eq!(v.source, HistorySource::RollingWindow);
        assert_eq!(v.query_id, 11);
        assert_eq!(v.header.version, 3);
        assert_eq!(v.realtime_offset_ns, 5);
        assert_eq!(v.span, Some((T0, last)));
        assert_eq!(v.frame_t, Some(last));
        assert_eq!(v.rows.len(), 4);
        assert_eq!(v.process_count, 4);
        assert!(v.rows.iter().all(|r| r.present && !r.previous_boot));
        let r3 = v.rows.iter().find(|r| r.key == key(3)).unwrap();
        assert_eq!(&*r3.name, "proc103");
        assert_eq!(r3.exe.as_deref(), Some("/usr/bin/proc103"));
        assert_eq!(r3.cpu_pct, 4.0);
        assert_eq!(r3.mem_anon_bytes, 40 << 20);
        assert!(r3.peak.is_none());
        let totals = v.totals.unwrap();
        assert_eq!(totals.cpu_pct, 25.0);
        assert_eq!(totals.mem_used_bytes, 2_000_000_000);
        assert_eq!(totals.mem_total_bytes, 8 << 30);
        assert_eq!(v.series.len(), 1);
        assert!(v.series[0].key.is_none());
        assert_eq!(v.series[0].points.len(), 300);
        assert!(!v.stack.is_empty());
        assert!(v.gaps.is_empty());
        assert!(v.overview.is_empty());
    }

    #[test]
    fn absent_selected_key_listed_last() {
        // key(9) exits at T0 + 180 s, 2 minutes before the newest second.
        let w = window(300, steady(180));
        let now = T0 + 299 * SEC;
        let what = frame(HistoryMetric::Cpu, (T0, now), &[key(9), key(1)]);
        let at = HistoryRange::At(now - 30 * SEC + 400_000_000);
        let v = answer_frame(QuerySource::Rolling(&w), &ctx(), 1, &at, &what);
        assert_eq!(v.frame_t, Some(now - 30 * SEC));
        assert_eq!(v.rows.len(), 5);
        assert_eq!(v.process_count, 4);
        let last = v.rows.last().unwrap();
        assert_eq!(last.key, key(9));
        assert!(!last.present);
        assert_eq!(&*last.name, "proc109");
        assert_eq!((last.cpu_pct, last.mem_anon_bytes), (0.0, 0));
        assert!(v.rows[..4].iter().all(|r| r.present));
        // The system total, then one series per selected key.
        assert_eq!(v.series.len(), 3);
        assert_eq!(v.series[1].key, Some(key(9)));
        assert_eq!(v.series[1].points.len(), 180);
    }

    #[test]
    fn previous_boot_marked() {
        let old = ProcessKey {
            boot_id: BootId(6),
            ..key(5)
        };
        let w = window(30, move |_| {
            vec![record(key(0), 1.0, 1), record(old, 2.0, 1)]
        });
        let what = frame(HistoryMetric::Cpu, (T0, T0 + 29 * SEC), &[]);
        let v = answer_frame(
            QuerySource::Rolling(&w),
            &ctx(),
            1,
            &HistoryRange::Live,
            &what,
        );
        let flag = |k: ProcessKey| v.rows.iter().find(|r| r.key == k).unwrap().previous_boot;
        assert!(flag(old));
        assert!(!flag(key(0)));
    }

    #[test]
    fn series_capped_1024() {
        // 6 h of 1 Hz data in 60 s blocks, as a store would hold.
        let secs = 6 * 3600u64;
        let mut b = BlockBuilder::new(BOOT);
        b.on_device_catalog(&catalog());
        let mut blocks = Vec::new();
        for i in 0..secs {
            let t = T0 + i * SEC;
            blocks.extend(b.on_system_sample(t, &system(t, (i % 100) as f32, 1e9)));
            let sweep = [record(key(0), (i % 50) as f32, 1), record(key(1), 3.0, 2)];
            blocks.extend(b.on_process_sweep(t, &sweep));
        }
        blocks.extend(b.flush());
        let mut metas = MetaTable::default();
        for block in &blocks {
            metas.absorb(block);
        }
        let data = Data {
            blocks: blocks.into_iter().map(Arc::new).collect(),
            metas: Cow::Owned(metas),
            deleted: Cow::Owned(DeletedKeys::default()),
            read_failed: false,
        };
        let span = (T0, T0 + (secs - 1) * SEC);
        let what = HistoryWhat::Frame {
            metric: HistoryMetric::Cpu,
            view: span,
            keys: Arc::from([key(0), key(1)]),
            overview: true,
        };
        let mut v = HistoryView {
            span: Some(span),
            ..HistoryView::default()
        };
        let sel = select(HistoryRange::Live, span, u64::MAX);
        fill_frame(&mut v, &data, &ctx(), sel, &what);
        assert_eq!(v.series.len(), 3);
        for s in v.series.iter() {
            assert!(!s.points.is_empty());
            assert!(s.points.len() <= HISTORY_MAX_POINTS, "{}", s.points.len());
        }
        assert!(v.stack.len() <= HISTORY_MAX_POINTS);
        assert!(!v.overview.is_empty() && v.overview.len() <= HISTORY_MAX_POINTS);
        // The spike-preserving min/max covers the whole 0..=99 range per bucket.
        assert!(v.series[0].points.iter().any(|p| p.max == 99.0));
    }

    #[test]
    fn span_has_peaks() {
        let w = window(300, steady(300));
        let from = T0 + 60 * SEC;
        let to = from + 162 * SEC;
        let what = frame(HistoryMetric::Memory, (T0, T0 + 299 * SEC), &[]);
        let range = HistoryRange::Span { from, to };
        let v = answer_frame(QuerySource::Rolling(&w), &ctx(), 1, &range, &what);
        assert_eq!(v.frame_t, None);
        assert_eq!(v.range, range);
        assert_eq!(v.rows.len(), 5);
        assert!(v.rows.iter().all(|r| r.peak.is_some()));
        let r9 = v.rows.iter().find(|r| r.key == key(9)).unwrap();
        assert_eq!(r9.peak.unwrap().cpu_pct, 50.0);
        let totals = v.totals.unwrap();
        assert_eq!(totals.cpu_pct, 25.0);
        assert_eq!(totals.mem_used_bytes, 2_000_000_000);
    }

    #[test]
    fn deleted_keys_gone() {
        let mut w = window(120, steady(120));
        let now = T0 + 119 * SEC;
        let k = key(9);
        w.delete_keys(&[k]);
        let what = frame(HistoryMetric::Cpu, (T0, now), &[k, key(0)]);
        for range in [HistoryRange::Live, HistoryRange::Span { from: T0, to: now }] {
            let v = answer_frame(QuerySource::Rolling(&w), &ctx(), 1, &range, &what);
            assert!(!v.rows.is_empty());
            assert!(v.rows.iter().all(|r| r.key != k));
            assert!(v.series.iter().all(|s| s.key != Some(k)));
            assert_eq!(v.series.len(), 2);
        }
        let top = answer_top(
            QuerySource::Rolling(&w),
            &ctx(),
            2,
            now,
            HistoryMetric::Cpu,
            3,
        );
        assert_eq!(top.rows.len(), 3);
        assert!(top.rows.iter().all(|r| r.key != k));
        // key(9) has the most CPU, so without it key(3) leads.
        assert_eq!(top.rows[0].key, key(3));
    }

    #[test]
    fn top_at_memory_sorted() {
        let w = window(90, steady(90));
        let t = T0 + 45 * SEC + 300_000_000;
        let top = answer_top(
            QuerySource::Rolling(&w),
            &ctx(),
            4,
            t,
            HistoryMetric::Memory,
            3,
        );
        assert_eq!(top.query_id, 4);
        assert_eq!(top.t_ns, T0 + 45 * SEC);
        assert_eq!(top.metric, HistoryMetric::Memory);
        assert_eq!(top.rows.len(), 3);
        let keys: Vec<_> = top.rows.iter().map(|r| r.key).collect();
        assert_eq!(keys, vec![key(3), key(2), key(1)]);
        assert!(top.rows.windows(2).all(|p| p[0].value >= p[1].value));
        assert_eq!(top.rows[0].value, (40u64 << 20) as f32);
        assert_eq!(&*top.rows[0].name, "proc103");
        assert_eq!(top.mem_total_bytes, 8 << 30);
        assert_eq!(top.cpu_total_pct, 25.0);
    }

    #[test]
    fn empty_window_no_rows() {
        let w = RollingWindow::new(BOOT);
        let what = frame(HistoryMetric::Cpu, (0, 60 * SEC), &[key(1)]);
        let v = answer_frame(
            QuerySource::Rolling(&w),
            &ctx(),
            1,
            &HistoryRange::Live,
            &what,
        );
        assert_eq!(v.source, HistorySource::RollingWindow);
        assert!(v.rows.is_empty());
        assert!(v.span.is_none());
        assert!(v.frame_t.is_none());
        assert!(v.series.is_empty());
        let top = answer_top(
            QuerySource::Rolling(&w),
            &ctx(),
            2,
            0,
            HistoryMetric::Cpu,
            3,
        );
        assert!(top.rows.is_empty());
        let none = answer_frame(QuerySource::None, &ctx(), 1, &HistoryRange::Live, &what);
        assert_eq!(none.source, HistorySource::None);
        assert!(none.rows.is_empty() && none.span.is_none());
    }

    #[test]
    fn gap_before_block_reported() {
        let mut w = window(90, steady(90));
        let resume = T0 + 200 * SEC;
        w.mark_gap(T0 + 90 * SEC);
        for i in 0..30u64 {
            let t = resume + i * SEC;
            w.on_system_sample(t, &system(t, 10.0, 1e9));
            w.on_process_sweep(t, &Arc::from(steady(0)(i)));
        }
        let end = resume + 29 * SEC;
        let what = frame(HistoryMetric::Cpu, (T0, end), &[]);
        let v = answer_frame(
            QuerySource::Rolling(&w),
            &ctx(),
            1,
            &HistoryRange::Live,
            &what,
        );
        assert_eq!(&*v.gaps, &[(T0 + 89 * SEC, resume)]);
    }

    /// A Frame answer for a 10-minute view of 2,000 processes stays within 50 ms (PROC-090).
    /// Measured at 12 to 17 ms at load average 31 to 50 on 32 cores.
    #[cfg(not(debug_assertions))]
    #[test]
    fn frame_answer_within_budget() {
        let w = window(600, |i| {
            (0..2000u32)
                .map(|k| {
                    // One process in 20 is busy and changes every second.
                    let busy = k % 20 == 0;
                    let cpu = if busy {
                        ((u64::from(k) + i) % 7) as f32
                    } else {
                        0.5
                    };
                    record(key(k), cpu, 4)
                })
                .collect()
        });
        let now = T0 + 599 * SEC;
        let what = frame(HistoryMetric::Cpu, (T0, now), &[key(1), key(2)]);
        let started = std::time::Instant::now();
        let v = answer_frame(
            QuerySource::Rolling(&w),
            &ctx(),
            1,
            &HistoryRange::Live,
            &what,
        );
        let frame_ms = started.elapsed().as_millis();
        assert_eq!(v.rows.len(), 2000);
        let started = std::time::Instant::now();
        let top = answer_top(
            QuerySource::Rolling(&w),
            &ctx(),
            2,
            now - 60 * SEC,
            HistoryMetric::Cpu,
            3,
        );
        let top_ms = started.elapsed().as_millis();
        assert_eq!(top.rows.len(), 3);
        assert!(frame_ms <= 50, "frame took {frame_ms} ms");
        assert!(top_ms <= 50, "top took {top_ms} ms");
    }
}
