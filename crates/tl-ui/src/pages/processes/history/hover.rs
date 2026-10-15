//! The top-3 hover card of the History strip (UI-PRC-034, PROC-090, R-063).
//!
//! Hovering the plot shows the time and the 3 processes that used the most of the active metric
//! at that second. [`TopController`] keeps the latest wanted second: 32 ms after it last changed,
//! and with no query in flight, it sends one `TopAt` query for it. Only an answer for the wanted
//! second replaces the card; a late answer for a second the pointer left is dropped.
//!
//! The engine answers a `TopAt` query with the last view cloned and `top` set, so the page stores
//! it and repaints only the strip (see [`top_only`]). The strip's prepaint calls [`defer_card`],
//! which rebuilds the card texts only when their inputs change and draws the card as a deferred
//! overlay from cached shaped lines. While no answer for the hovered second is in, the previous
//! card stays; after 150 ms its rows read "…".

use core::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, App, AsyncApp, AvailableSpace, Bounds, BoxShadow, ContentMask, Context, Element,
    ElementId, Entity, FontWeight, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Point, Size, Style, Styled as _, StyledImage as _, Task, WeakEntity, Window, fill, img,
    point, px, size, svg,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, HistoryWhat};
use tl_model::history::{HistoryMetric, HistoryRange, HistoryTop, HistoryView};
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::time::TimeNs;
use tl_model::units::{self, NetUnit};

use super::{HistoryPage, NS_PER_SEC, QUERY_TIMEOUT, clock};
use crate::chart::labels::{CachedLine, LineStyle, paint_line};
use crate::hub::GlobalHub;
use crate::i18n::Strings;
use crate::pages::performance::details::Template;
use crate::settings_global::{Section, TlSettings};
use crate::table::cells::IconSourceCache;
use crate::theme::{TlTheme, UiMetrics};

/// TopAt query ids start here, so they never equal a Frame query id. With no last view the
/// engine answers a TopAt query with a frame that carries the TopAt id.
pub const TOP_ID_BASE: u64 = 1 << 48;
/// A query goes out this long after the last change of the hovered second.
pub const DEBOUNCE: Duration = Duration::from_millis(32);
/// The rows read "…" this long after the hovered second changed without an answer.
pub const STALE_AFTER: Duration = Duration::from_millis(150);
/// Processes per card.
const TOP_N: u8 = 3;

/// Card sizes in logical px at 100% scale.
const CARD_W: f32 = 260.0;
/// Card left edge to the right of the hover hairline.
const GAP_X: f32 = 12.0;
/// Card top below the strip top.
const GAP_Y: f32 = 8.0;
const ROW_H: f32 = 20.0;
const ICON: f32 = 16.0;
/// Width of the rank column.
const RANK_W: f32 = 14.0;
/// A row, and the CPU total value, while no answer for the hovered second is in.
const WAITING: &str = "…";

/// The card value texts, translated once per `strings_version`. `Default` is en-US.
#[derive(Clone, Debug)]
pub struct TopWords {
    /// "CPU total {$value}".
    pub total: Template,
    /// "CPU: {$value}".
    pub cpu: Template,
    /// "Memory: {$used} / {$total}".
    pub memory: Template,
    /// "Disk: {$value}".
    pub disk: Template,
    /// "Network: {$value}".
    pub network: Template,
    /// "GPU: {$value}".
    pub gpu: Template,
}

const KEY_TOTAL: &str = "history-top-cpu-total";
const KEY_CPU: &str = "history-top-cpu";
const KEY_MEMORY: &str = "history-top-memory";
const KEY_DISK: &str = "history-top-disk";
const KEY_NETWORK: &str = "history-top-network";
const KEY_GPU: &str = "history-top-gpu";

impl Default for TopWords {
    fn default() -> Self {
        Self {
            total: Template::en(KEY_TOTAL, &["value"]),
            cpu: Template::en(KEY_CPU, &["value"]),
            memory: Template::en(KEY_MEMORY, &["used", "total"]),
            disk: Template::en(KEY_DISK, &["value"]),
            network: Template::en(KEY_NETWORK, &["value"]),
            gpu: Template::en(KEY_GPU, &["value"]),
        }
    }
}

impl TopWords {
    /// Every text through `tr`.
    pub fn new(cx: &App) -> Self {
        Self {
            total: Template::new(cx, KEY_TOTAL, &["value"]),
            cpu: Template::new(cx, KEY_CPU, &["value"]),
            memory: Template::new(cx, KEY_MEMORY, &["used", "total"]),
            disk: Template::new(cx, KEY_DISK, &["value"]),
            network: Template::new(cx, KEY_NETWORK, &["value"]),
            gpu: Template::new(cx, KEY_GPU, &["value"]),
        }
    }
}

/// `units.network` and `units.network_auto_scale`.
pub fn net_unit(settings: &Settings) -> (NetUnit, bool) {
    let unit = match settings.units.network {
        NetworkUnit::Bits => NetUnit::Bits,
        NetworkUnit::Bytes => NetUnit::Bytes,
    };
    (unit, settings.units.network_auto_scale)
}

/// Writes the value of one row for `metric` (R-063): "CPU: 12.3%", "Memory: 1.22 / 62.70 GiB",
/// "Disk: 4.2 MiB/s", "Network: 12.0 Mbps", "GPU: 41.0%". `value` is percent, bytes or bytes per
/// second; `mem_total` is the machine's memory in bytes.
pub fn value_text(
    w: &TopWords,
    out: &mut String,
    metric: HistoryMetric,
    value: f32,
    mem_total: u64,
    net: (NetUnit, bool),
) {
    let bps = f64::from(value);
    match metric {
        HistoryMetric::Cpu => w.cpu.write_each(out, |_, out| units::percent(out, value)),
        HistoryMetric::Gpu => w.gpu.write_each(out, |_, out| units::percent(out, value)),
        HistoryMetric::Disk => w
            .disk
            .write_each(out, |_, out| units::rate_mib_fixed(out, bps)),
        HistoryMetric::Network => w
            .network
            .write_each(out, |_, out| units::net_rate(out, bps, net.0, net.1)),
        HistoryMetric::Memory => {
            let used = value.max(0.0) as u64;
            w.memory.write_each(out, |i, out| match i {
                0 => used_in_unit_of(out, used, mem_total),
                _ => units::bytes(out, mem_total),
            });
        }
    }
}

/// `used` in the IEC unit `units::bytes` picks for `total`, two decimals and no unit, so
/// "1.22" goes before "62.70 GiB". Under 1 KiB both are whole bytes.
fn used_in_unit_of(out: &mut String, used: u64, total: u64) {
    let mut t = String::new();
    units::bytes(&mut t, total);
    let exp = match t.rsplit(' ').next() {
        Some("KiB") => 1,
        Some("MiB") => 2,
        Some("GiB") => 3,
        Some("TiB") => 4,
        _ => 0,
    };
    if exp == 0 {
        let _ = write!(out, "{used}");
    } else {
        let _ = write!(out, "{:.2}", used as f64 / 1024f64.powi(exp));
    }
}

/// The card's top-left corner for a hairline at window x `hairline_x`: 12 px right of the
/// hairline and 8 px below the strip top, or left of the hairline when the card would cross the
/// strip's right edge.
pub fn card_origin(hairline_x: Pixels, strip: Bounds<Pixels>, scale: f32) -> Point<Pixels> {
    let (gap, w) = (px(GAP_X * scale), px(CARD_W * scale));
    let mut x = hairline_x + gap;
    if x + w > strip.right() {
        x = hairline_x - gap - w;
    }
    point(x, strip.top() + px(GAP_Y * scale))
}

/// True when `new` answers a TopAt query and changes nothing else of `old`: the engine clones
/// its last view and sets `top`. Such a view needs no table install and no strip rebuild.
pub fn top_only(old: &HistoryView, new: &HistoryView) -> bool {
    let top_changed = match (&old.top, &new.top) {
        (_, None) => false,
        (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
        (None, Some(_)) => true,
    };
    top_changed
        && old.query_id == new.query_id
        && old.span == new.span
        && Arc::ptr_eq(&old.rows, &new.rows)
        && Arc::ptr_eq(&old.series, &new.series)
        && Arc::ptr_eq(&old.stack, &new.stack)
        && Arc::ptr_eq(&old.gaps, &new.gaps)
        && Arc::ptr_eq(&old.overview, &new.overview)
}

/// The hovered second and metric a TopAt query asks for.
type TopKey = (TimeNs, HistoryMetric);

/// The TopAt state of the card, latest wins: `wanted` is what the hairline shows, `in_flight`
/// the one query out and `shown` the answer on the card. Only `wanted` is ever sent, and only an
/// answer for `wanted` replaces the card.
pub struct TopController {
    next_id: u64,
    /// The second and metric under the pointer. `None` while the card is hidden.
    wanted: Option<TopKey>,
    /// Id and key of the TopAt query sent and not yet answered.
    in_flight: Option<(u64, TopKey)>,
    sent_at: Option<Instant>,
    /// The answer on the card.
    shown: Option<Arc<HistoryTop>>,
    /// True when the last `send` was refused with `Busy`.
    busy: bool,
    /// True once `wanted` went 150 ms without an answer.
    stale: bool,
    /// Runs for 32 ms after `wanted` changed.
    debounce: Option<Task<()>>,
    stale_timer: Option<Task<()>>,
    /// Expiry of the query in flight, and the `Busy` retry.
    timer: Option<Task<()>>,
}

impl Default for TopController {
    fn default() -> Self {
        Self {
            next_id: TOP_ID_BASE,
            wanted: None,
            in_flight: None,
            sent_at: None,
            shown: None,
            busy: false,
            stale: false,
            debounce: None,
            stale_timer: None,
            timer: None,
        }
    }
}

impl TopController {
    /// Id of the TopAt query in flight.
    pub fn in_flight(&self) -> Option<u64> {
        self.in_flight.map(|(id, _)| id)
    }

    /// The second and metric under the pointer, `None` while the card is hidden.
    pub fn wanted(&self) -> Option<(TimeNs, HistoryMetric)> {
        self.wanted
    }

    /// The second and metric of the answer on the card.
    pub fn shown(&self) -> Option<(TimeNs, HistoryMetric)> {
        self.shown.as_deref().map(|top| (top.t_ns, top.metric))
    }

    pub fn stale(&self) -> bool {
        self.stale
    }

    /// Takes the answer `view.top` carries. Frees the slot when it answers the query in flight,
    /// and shows it when it is for `wanted`. Any other answer is dropped and the card keeps the
    /// previous one. True when the card changed.
    fn on_view(&mut self, view: &HistoryView) -> bool {
        let Some(top) = view.top.as_ref() else {
            return false;
        };
        if self.in_flight.is_some_and(|(id, _)| id == top.query_id) {
            self.in_flight = None;
            self.sent_at = None;
        }
        let is_wanted = self.wanted == Some((top.t_ns, top.metric));
        if !is_wanted || self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, top)) {
            return false;
        }
        self.shown = Some(Arc::clone(top));
        self.stale = false;
        self.stale_timer = None;
        true
    }

    /// Frees the slot when the query in flight went unanswered for 1,000 ms.
    fn expire(&mut self, now: Instant) {
        if self
            .sent_at
            .is_some_and(|sent| now.saturating_duration_since(sent) >= QUERY_TIMEOUT)
        {
            self.in_flight = None;
            self.sent_at = None;
        }
    }

    /// Sends `wanted` when the slot is free, the debounce has passed and the card does not show
    /// it yet. `Busy` tries again later, `EngineStopped` gives up.
    fn send_wanted(&mut self, client: &dyn EngineClient, now: Instant) {
        self.busy = false;
        let Some(at) = self.wanted else {
            return;
        };
        if self.in_flight.is_some() || self.debounce.is_some() || self.shown() == Some(at) {
            return;
        }
        let id = self.next_id;
        let (t, metric) = at;
        let cmd = Command::HistoryQuery {
            id,
            range: HistoryRange::At(t),
            what: HistoryWhat::TopAt {
                t,
                metric,
                n: TOP_N,
            },
        };
        match client.send(cmd) {
            Ok(()) => {
                self.next_id += 1;
                self.in_flight = Some((id, at));
                self.sent_at = Some(now);
            }
            Err(CommandError::Busy) => self.busy = true,
            Err(CommandError::EngineStopped) => {}
        }
    }
}

impl HistoryPage {
    /// The pointer is over the plot at second `t`. A new second or metric restarts the 32 ms
    /// debounce and, unless the card shows it, the 150 ms stale timer.
    pub(crate) fn top_hover(&mut self, t: TimeNs, cx: &mut Context<Self>) {
        let at = (t, self.state.metric);
        if self.top.wanted == Some(at) {
            return;
        }
        self.top.wanted = Some(at);
        self.top.debounce = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(DEBOUNCE).await;
                let _ = this.update(cx, |page, cx| {
                    page.top.debounce = None;
                    page.top_drive(cx);
                });
            },
        ));
        if self.top.shown() == Some(at) {
            self.top.stale = false;
            self.top.stale_timer = None;
        } else {
            self.top.stale_timer = Some(cx.spawn(
                async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor().timer(STALE_AFTER).await;
                    let _ = this.update(cx, |page, cx| page.top_go_stale(cx));
                },
            ));
        }
    }

    /// The card hides: pointer left, drag start or wheel. Nothing is wanted, so nothing is sent
    /// and a late answer is dropped. A query in flight keeps only its 1,000 ms expiry.
    pub(crate) fn top_clear(&mut self) {
        let top = &mut self.top;
        top.wanted = None;
        top.shown = None;
        top.busy = false;
        top.stale = false;
        top.debounce = None;
        top.stale_timer = None;
        if top.in_flight.is_none() {
            top.timer = None;
        }
    }

    /// A view came in: takes the TopAt answer it carries, sends `wanted` when the slot came free
    /// and repaints the strip when the card changed.
    pub(crate) fn on_top_view(&mut self, view: &HistoryView, cx: &mut Context<Self>) {
        let had_flight = self.top.in_flight.is_some();
        let changed = self.top.on_view(view);
        if had_flight && self.top.in_flight.is_none() {
            self.top_drive(cx);
        }
        if changed {
            self.notify_strip(cx);
        }
    }

    fn top_go_stale(&mut self, cx: &mut Context<Self>) {
        self.top.stale_timer = None;
        if self.top.wanted.is_some() && self.top.wanted != self.top.shown() {
            self.top.stale = true;
            self.notify_strip(cx);
        }
    }

    /// Expires an old query, sends `wanted` when it may go, then arms the timer for the expiry
    /// of the query in flight or the `Busy` retry.
    fn top_drive(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.top.expire(now);
        self.top.send_wanted(&*self.client, now);
        let wait = match self.top.sent_at {
            Some(sent) => Some(QUERY_TIMEOUT.saturating_sub(now.saturating_duration_since(sent))),
            None if self.top.busy => Some(DEBOUNCE),
            None => None,
        };
        self.top.timer = wait.map(|wait| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |page, cx| page.top_drive(cx));
            })
        });
    }

    /// Repaints the strip view only, or the page while the strip view does not exist yet.
    fn notify_strip(&mut self, cx: &mut Context<Self>) {
        match self.strip_view.clone() {
            Some(strip) => strip.update(cx, |_, cx| cx.notify()),
            None => cx.notify(),
        }
    }

    /// The TopAt controller.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn top(&self) -> &TopController {
        &self.top
    }

    /// The card texts and where it was last painted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn card(&self) -> &CardCache {
        &self.card
    }
}

/// One process row of the card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowText {
    /// "1" to "3", empty while stale.
    pub rank: String,
    /// "<name> (<pid>)", or "…" while stale.
    pub name: String,
    /// "CPU: 12.3%", empty while stale.
    pub value: String,
    /// The app icon file, when the app has one.
    pub icon: Option<Arc<str>>,
    /// False while stale: no icon at all.
    pub has_icon: bool,
}

/// Every text of the card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardTexts {
    /// "HH:MM:SS".
    pub time: String,
    /// "  (-4m 12s)", drawn after the time.
    pub offset: String,
    /// "CPU total 12.5%".
    pub total: String,
    pub rows: SmallVec<[RowText; 3]>,
}

impl CardTexts {
    fn clear(&mut self) {
        self.time.clear();
        self.offset.clear();
        self.total.clear();
        self.rows.clear();
    }
}

/// What the card texts are built from. A rebuild happens only when it changes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CardKey {
    /// Address of the shown `HistoryTop`.
    top: usize,
    stale_t: Option<TimeNs>,
    now: TimeNs,
    realtime_offset_ns: i64,
    strings_version: u64,
    units_version: u64,
}

/// The shaped lines of one row.
#[derive(Default)]
struct RowLines {
    rank: CachedLine,
    name: CachedLine,
    value: CachedLine,
}

/// The card's texts and shaped lines, kept on the page between frames.
#[derive(Default)]
pub struct CardCache {
    key: Option<CardKey>,
    words: Option<(u64, TopWords)>,
    texts: CardTexts,
    time: CachedLine,
    offset: CachedLine,
    total: CachedLine,
    rows: SmallVec<[RowLines; 3]>,
    icons: IconSourceCache,
    /// Text builds over every frame.
    #[cfg(any(test, feature = "test-hooks"))]
    pub builds: u32,
    /// The card of the last frame that showed it, window logical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub painted: Option<Bounds<Pixels>>,
}

impl CardCache {
    pub fn texts(&self) -> &CardTexts {
        &self.texts
    }

    /// Rebuilds the texts when their inputs changed and re-shapes the lines whose text changed.
    #[allow(clippy::too_many_arguments)]
    fn sync(
        &mut self,
        view: &HistoryView,
        top: Option<&Arc<HistoryTop>>,
        stale_t: Option<TimeNs>,
        units_version: u64,
        net: (NetUnit, bool),
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &App,
    ) {
        let strings_version = cx.try_global::<Strings>().map_or(0, |s| s.strings_version);
        if self
            .words
            .as_ref()
            .is_none_or(|(v, _)| *v != strings_version)
        {
            let words = match cx.try_global::<Strings>() {
                Some(_) => TopWords::new(cx),
                None => TopWords::default(),
            };
            self.words = Some((strings_version, words));
        }
        let key = CardKey {
            top: top.map_or(0, |t| Arc::as_ptr(t) as usize),
            stale_t,
            now: view.span.map_or(0, |s| s.1),
            realtime_offset_ns: view.realtime_offset_ns,
            strings_version,
            units_version,
        };
        if self.key != Some(key) {
            self.key = Some(key);
            self.build(view, top.map(|t| &**t), stale_t, net, cx);
            #[cfg(any(test, feature = "test-hooks"))]
            {
                self.builds += 1;
            }
        }
        let style = |weight, color, tnum| LineStyle {
            size: m.text.sm,
            weight,
            color,
            tnum,
        };
        let normal = FontWeight::NORMAL;
        let t = &self.texts;
        self.time.sync(
            window,
            &t.time,
            style(FontWeight::SEMIBOLD, theme.text_primary, true),
        );
        self.offset
            .sync(window, &t.offset, style(normal, theme.text_secondary, true));
        self.total
            .sync(window, &t.total, style(normal, theme.text_secondary, true));
        self.rows.resize_with(t.rows.len(), RowLines::default);
        for (lines, row) in self.rows.iter_mut().zip(&t.rows) {
            lines
                .rank
                .sync(window, &row.rank, style(normal, theme.text_secondary, true));
            lines
                .name
                .sync(window, &row.name, style(normal, theme.text_primary, false));
            lines
                .value
                .sync(window, &row.value, style(normal, theme.text_primary, true));
        }
    }

    /// The texts for the answer `top`, or for the hovered second `stale_t` with "…" rows.
    fn build(
        &mut self,
        view: &HistoryView,
        top: Option<&HistoryTop>,
        stale_t: Option<TimeNs>,
        net: (NetUnit, bool),
        cx: &App,
    ) {
        let words = match &self.words {
            Some((_, w)) => w,
            None => return,
        };
        let Some(t) = stale_t.or(top.map(|top| top.t_ns)) else {
            return;
        };
        let texts = &mut self.texts;
        texts.clear();
        clock(&mut texts.time, t, view.realtime_offset_ns);
        let now = view.span.map_or(t, |s| s.1);
        texts.offset.push_str("  (-");
        units::short_age(&mut texts.offset, now.saturating_sub(t) / NS_PER_SEC);
        texts.offset.push(')');
        match (stale_t, top) {
            (None, Some(top)) => {
                let pct = top.cpu_total_pct;
                words
                    .total
                    .write_each(&mut texts.total, |_, out| units::percent(out, pct));
                let processes = cx
                    .try_global::<GlobalHub>()
                    .map(|hub| Arc::clone(&hub.0.read(cx).current().processes));
                let icon_of = |app: &str| {
                    let records = processes.as_ref()?.records.iter();
                    records
                        .filter(|r| r.meta.app_id.as_deref() == Some(app))
                        .find_map(|r| r.meta.icon.clone())
                };
                for (i, row) in top.rows.iter().take(usize::from(TOP_N)).enumerate() {
                    let mut r = RowText {
                        has_icon: true,
                        icon: row.app_id.as_deref().and_then(icon_of),
                        ..RowText::default()
                    };
                    let _ = write!(r.rank, "{}", i + 1);
                    let _ = write!(r.name, "{} ({})", row.name, row.key.pid);
                    value_text(
                        words,
                        &mut r.value,
                        top.metric,
                        row.value,
                        top.mem_total_bytes,
                        net,
                    );
                    texts.rows.push(r);
                }
            }
            _ => {
                words
                    .total
                    .write_each(&mut texts.total, |_, out| out.push_str(WAITING));
                let n = top
                    .map_or(0, |top| top.rows.len())
                    .clamp(1, usize::from(TOP_N));
                for _ in 0..n {
                    texts.rows.push(RowText {
                        name: WAITING.to_owned(),
                        ..RowText::default()
                    });
                }
            }
        }
    }

    /// Card size for the current rows.
    fn size(&self, m: &UiMetrics) -> Size<Pixels> {
        let rows = self.texts.rows.len();
        size(px(CARD_W * m.ui_scale), row_top(m, rows) + m.sp[4])
    }

    /// The icon of each row that has one, and its offset from the card origin.
    fn icon_elements(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
    ) -> SmallVec<[(Point<Pixels>, AnyElement); 3]> {
        let s = m.ui_scale;
        let side = px(ICON * s);
        let color = theme.text_secondary;
        let binary = move || {
            svg()
                .path(IconName::Binary.path())
                .size(side)
                .text_color(color)
        };
        let mut out = SmallVec::new();
        for (i, row) in self.texts.rows.iter().enumerate() {
            if !row.has_icon {
                continue;
            }
            let at = point(
                m.sp[4] + px(RANK_W * s),
                row_top(m, i) + px((ROW_H - ICON) / 2.0 * s),
            );
            let el = match &row.icon {
                Some(path) => img(self.icons.get(path))
                    .size(side)
                    .with_fallback(move || binary().into_any_element())
                    .into_any_element(),
                None => binary().into_any_element(),
            };
            out.push((at, el));
        }
        out
    }

    /// Paints the text lines of a card at `bounds`.
    fn paint(
        &self,
        bounds: Bounds<Pixels>,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut App,
    ) {
        let s = m.ui_scale;
        let lh = line_h(m);
        let left = bounds.left() + m.sp[4];
        let right = bounds.right() - m.sp[4];
        let mut y = bounds.top() + m.sp[4];
        paint_line(window, cx, &self.time, point(left, y), lh);
        paint_line(
            window,
            cx,
            &self.offset,
            point(left + self.time.width(), y),
            lh,
        );
        y += lh;
        paint_line(window, cx, &self.total, point(left, y), lh);
        y += lh + m.sp[2];
        window.paint_quad(fill(
            Bounds::new(point(left, y), size(right - left, px(1.0))),
            theme.divider,
        ));
        let row_h = px(ROW_H * s);
        let name_x = left + px((RANK_W + ICON) * s) + m.sp[2];
        for (i, row) in self.rows.iter().enumerate() {
            let y = bounds.top() + row_top(m, i);
            paint_line(window, cx, &row.rank, point(left, y), row_h);
            let value_x = right - row.value.width();
            paint_line(window, cx, &row.value, point(value_x, y), row_h);
            let clip_right = if row.value.width() > px(0.) {
                value_x - m.sp[2]
            } else {
                right
            };
            let clip = Bounds::from_corners(point(name_x, y), point(clip_right, y + row_h));
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                paint_line(window, cx, &row.name, point(name_x, y), row_h);
            });
        }
    }
}

/// Height of one text line in `t_sm`.
fn line_h(m: &UiMetrics) -> Pixels {
    m.text.sm * m.line_height
}

/// Top of row `i` from the card top: padding, the time and total lines, the divider with a
/// `sp2` margin on each side, then 20 px rows.
fn row_top(m: &UiMetrics, i: usize) -> Pixels {
    m.sp[4] + line_h(m) * 2.0 + m.sp[2] * 2.0 + px(1.0) + px(ROW_H * m.ui_scale) * i as f32
}

/// Builds this frame's card and draws it as a deferred overlay above the page. Shown while the
/// pointer is over the plot, no drag runs, the view has frames and there is an answer or the
/// hovered second went stale. Call from the strip's prepaint: `defer_draw` works only there.
pub(super) fn defer_card(
    page: &Entity<HistoryPage>,
    strip: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let m = *cx.global::<UiMetrics>();
    let theme = crate::theme::current(cx).clone();
    let (units_version, net) = match cx.try_global::<TlSettings>() {
        Some(tl) => (
            tl.version(Section::Units),
            net_unit(TlSettings::settings(cx)),
        ),
        None => (0, net_unit(&Settings::default())),
    };
    let placed = page.update(cx, |p, cx| {
        let HistoryPage {
            view,
            scrubber,
            top,
            card,
            ..
        } = p;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            card.painted = None;
        }
        let hx = scrubber
            .hover_x
            .filter(|x| *x >= px(0.) && *x < strip.size.width)?;
        if scrubber.drag.is_some() {
            return None;
        }
        let view = view.as_deref().filter(|v| v.span.is_some())?;
        let stale_t = top.wanted.filter(|_| top.stale).map(|(t, _)| t);
        if stale_t.is_none() && top.shown.is_none() {
            return None;
        }
        let shown = top.shown.as_ref();
        card.sync(
            view,
            shown,
            stale_t,
            units_version,
            net,
            &m,
            &theme,
            window,
            cx,
        );
        let origin = card_origin(strip.left() + hx, strip, m.ui_scale);
        Some((origin, card.size(&m), card.icon_elements(&m, &theme)))
    });
    let Some((origin, size, icons)) = placed else {
        return;
    };
    let mut card = TopCard {
        page: page.clone(),
        size,
        icons,
        metrics: m,
        theme,
    }
    .into_any_element();
    card.layout_as_root(AvailableSpace::min_size(), window, cx);
    window.defer_draw(card, origin, 1, None);
}

/// The card element: a `TlTooltip`-like panel painting the page's cached lines and the icons.
struct TopCard {
    page: Entity<HistoryPage>,
    size: Size<Pixels>,
    /// Offset from the card origin and the icon element.
    icons: SmallVec<[(Point<Pixels>, AnyElement); 3]>,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
}

impl IntoElement for TopCard {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TopCard {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(self.size.width.into(), self.size.height.into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let side = AvailableSpace::Definite(px(ICON * self.metrics.ui_scale));
        for (at, icon) in &mut self.icons {
            icon.prepaint_as_root(bounds.origin + *at, size(side, side), window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let (m, theme) = (&self.metrics, &self.theme);
        let radius = m.radius_small;
        window.paint_drop_shadows(
            bounds,
            radius.into(),
            &[BoxShadow {
                color: theme.derived.shadow,
                offset: point(px(0.), px(2.)),
                blur_radius: px(8.),
                spread_radius: px(0.),
                inset: false,
            }],
        );
        window.paint_quad(
            fill(bounds, theme.menu_bg)
                .corner_radii(radius)
                .border_widths(px(1.))
                .border_color(theme.divider),
        );
        self.page.update(cx, |p, cx| {
            p.card.paint(bounds, m, theme, window, cx);
            #[cfg(any(test, feature = "test-hooks"))]
            {
                p.card.painted = Some(bounds);
            }
        });
        for (_, icon) in &mut self.icons {
            icon.paint(window, cx);
        }
    }
}
