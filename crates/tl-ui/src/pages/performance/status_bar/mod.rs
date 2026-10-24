//! The machine status bar (UI-PRF-054, D-030): a 64 px strip with the status pill (a state word
//! and the reason), the chips for uptime, load, temperatures, memory pressure and swap-in, and a
//! warning count button while any chip is red.
//!
//! One [`StatusBar`] entity serves the app; [`status_bar`] makes it on first use. It reads the
//! snapshot once per second ([`rules`] holds the states and their windows), formats its text
//! and notifies only when that text changed. It also gives each Overview box its state for the
//! 2 px top edge, and emits [`EdgesChanged`] only when one of those changed.
//!
//! Placement (`performance.status_bar`): the Performance tab draws the bar above its page panel
//! for `overview` (the Overview page only) and `performance`; the main window draws it under the
//! tab strip for `all_tabs`. Each host reports whether it shows the bar with
//! [`StatusBar::set_shown`]; while any host shows it, the bar sends its demand part.

pub mod rules;

use std::fmt::Write as _;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Div, ElementId, Entity, EntityId,
    EventEmitter, FocusHandle, FontWeight, Global, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Render, SharedString, Stateful, Styled as _,
    Subscription, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::DeviceCatalog;
use tl_model::ids::DeviceIdx;
use tl_model::settings::StatusBarPlacement;
use tl_model::snapshot::Snapshot;
use tl_model::units;

use self::rules::{
    Chip, ChipKind, MachineState, Reading, Reason, Source, StatusRules, Tone, chips,
};
use super::actions::{PerfSelectDevice, PerfStatusOpen};
use super::overview::OVERVIEW_KEY;
use super::overview::layout::BoxKind;
use super::{DeviceKey, PerformancePage};
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, numeric_style, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::main_window_view;

/// The bar's debug selector.
pub const BAR_ID: &str = "perf-status-bar";
/// The status pill's debug selector.
pub const PILL_ID: &str = "perf-status-pill";
/// The warning count button's debug selector.
pub const WARNINGS_ID: &str = "perf-status-warnings";
/// The bar's height in one row, logical px × ui_scale.
pub const BAR_H: f32 = 64.;
const PILL_H: f32 = 40.;
const CHIP_H: f32 = 28.;

/// The debug selector of a chip: `perf-status-chip-cpu-temp`, `perf-status-chip-gpu-temp-2`.
pub fn chip_id(kind: ChipKind) -> String {
    let name = match kind {
        ChipKind::Uptime => "uptime".into(),
        ChipKind::Load => "load".into(),
        ChipKind::CpuTemp => "cpu-temp".into(),
        ChipKind::GpuTemp(idx) => format!("gpu-temp-{}", idx.0),
        ChipKind::MemPressure => "mem-pressure".into(),
        ChipKind::SwapIn => "swap-in".into(),
        ChipKind::DiskTemp(_) => "disk-temp".into(),
    };
    format!("perf-status-chip-{name}")
}

/// `performance.status_bar`, the default before settings load.
pub fn placement(cx: &App) -> StatusBarPlacement {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).performance.status_bar
    } else {
        StatusBarPlacement::default()
    }
}

/// A place that can draw the bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Under the tab strip of the main window (`all_tabs`).
    Main,
    /// Above the Performance tab's page panel (`overview`, `performance`).
    Performance,
}

/// Whether `slot` draws the bar under `placement`. `overview_shown` is whether the Performance
/// tab shows the Overview page.
pub fn shows_in(slot: Slot, placement: StatusBarPlacement, overview_shown: bool) -> bool {
    match (slot, placement) {
        (Slot::Main, StatusBarPlacement::AllTabs) => true,
        (Slot::Performance, StatusBarPlacement::Performance) => true,
        (Slot::Performance, StatusBarPlacement::Overview) => overview_shown,
        _ => false,
    }
}

/// The app's one bar and the hub it reads.
struct SharedBar {
    hub: EntityId,
    bar: Entity<StatusBar>,
}

impl Global for SharedBar {}

/// The status bar of `hub`, made on first use.
pub fn status_bar(
    hub: &Entity<SnapshotHub>,
    demand: &Entity<DemandAggregator>,
    cx: &mut App,
) -> Entity<StatusBar> {
    if let Some(shared) = cx.try_global::<SharedBar>()
        && shared.hub == hub.entity_id()
    {
        return shared.bar.clone();
    }
    let bar = cx.new(|cx| StatusBar::new(hub.clone(), demand.clone(), cx));
    cx.set_global(SharedBar {
        hub: hub.entity_id(),
        bar: bar.clone(),
    });
    bar
}

/// One or more Overview box states changed.
#[derive(Clone, Copy, Debug)]
pub struct EdgesChanged;

/// The fixed words, translated once per strings version.
struct Words {
    version: u64,
    states: [SharedString; 5],
    reason: Template,
    reason_process: Template,
    secs: Template,
    mins: Template,
    hours: Template,
    cpu: Template,
    gpu: Template,
    disk: Template,
    net: Template,
    mem_pressure: Template,
    mem_stall: Template,
    swap_in: Template,
    cpu_temp: Template,
    gpu_temp: Template,
    cpu_throttle: SharedString,
    gpu_throttle: SharedString,
    chip_uptime: SharedString,
    chip_load: SharedString,
    chip_cpu: SharedString,
    chip_gpu: SharedString,
    chip_mem: SharedString,
    chip_swap: SharedString,
    chip_disk: SharedString,
    pages_s: Template,
    tip_calm: SharedString,
    tip_busy_cpu: SharedString,
    tip_busy_gpu: SharedString,
    tip_busy_disk: SharedString,
    tip_busy_net: SharedString,
    tip_pressure: SharedString,
    tip_swap: SharedString,
    tip_throttle: SharedString,
    tip_saturated_cpu: SharedString,
    tip_saturated_mem: SharedString,
    tip_uptime: SharedString,
    tip_load: SharedString,
    tip_cpu_temp: Template,
    tip_gpu_temp: Template,
    tip_disk_temp: Template,
    tip_no_threshold: SharedString,
    tip_mem_chip: SharedString,
    tip_swap_chip: SharedString,
    warnings: Template,
    warning_row: Template,
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

impl Words {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        let tpl = |key: &str, args: &[&str]| Template::new(cx, key, args);
        Self {
            version: strings_version(cx),
            states: [
                t("perf-status-calm"),
                t("perf-status-busy"),
                t("perf-status-pressure"),
                t("perf-status-hot"),
                t("perf-status-saturated"),
            ],
            reason: tpl("perf-status-reason", &["state", "what", "time"]),
            reason_process: tpl(
                "perf-status-reason-process",
                &["state", "what", "time", "process"],
            ),
            secs: tpl("perf-status-secs", &["n"]),
            mins: tpl("perf-status-mins", &["n"]),
            hours: tpl("perf-status-hours", &["h", "m"]),
            cpu: tpl("perf-status-what-cpu", &["v"]),
            gpu: tpl("perf-status-what-gpu", &["v"]),
            disk: tpl("perf-status-what-disk", &["v"]),
            net: tpl("perf-status-what-net", &["v"]),
            mem_pressure: tpl("perf-status-what-mem-pressure", &["v"]),
            mem_stall: tpl("perf-status-what-mem-stall", &["v"]),
            swap_in: tpl("perf-status-what-swap-in", &["v"]),
            cpu_temp: tpl("perf-status-what-cpu", &["v"]),
            gpu_temp: tpl("perf-status-what-gpu", &["v"]),
            cpu_throttle: t("perf-status-what-cpu-throttle"),
            gpu_throttle: t("perf-status-what-gpu-throttle"),
            chip_uptime: t("perf-status-chip-uptime"),
            chip_load: t("perf-status-chip-load"),
            chip_cpu: t("perf-status-chip-cpu"),
            chip_gpu: t("perf-status-chip-gpu"),
            chip_mem: t("perf-status-chip-mem-pressure"),
            chip_swap: t("perf-status-chip-swap-in"),
            chip_disk: t("perf-status-chip-disk"),
            pages_s: tpl("perf-status-pages-s", &["n"]),
            tip_calm: t("perf-status-tip-calm"),
            tip_busy_cpu: t("perf-status-tip-busy-cpu"),
            tip_busy_gpu: t("perf-status-tip-busy-gpu"),
            tip_busy_disk: t("perf-status-tip-busy-disk"),
            tip_busy_net: t("perf-status-tip-busy-net"),
            tip_pressure: t("perf-status-tip-pressure"),
            tip_swap: t("perf-status-tip-swap"),
            tip_throttle: t("perf-status-tip-throttle"),
            tip_saturated_cpu: t("perf-status-tip-saturated-cpu"),
            tip_saturated_mem: t("perf-status-tip-saturated-mem"),
            tip_uptime: t("perf-status-tip-uptime"),
            tip_load: t("perf-status-tip-load"),
            tip_cpu_temp: tpl("perf-status-tip-cpu-temp", &["t"]),
            tip_gpu_temp: tpl("perf-status-tip-gpu-temp", &["t"]),
            tip_disk_temp: tpl("perf-status-tip-disk-temp", &["t"]),
            tip_no_threshold: t("perf-status-tip-no-threshold"),
            tip_mem_chip: t("perf-status-tip-mem-chip"),
            tip_swap_chip: t("perf-status-tip-swap-chip"),
            warnings: Template::counted(cx, "perf-status-warnings", &["n"], 0),
            warning_row: tpl("perf-status-warning-row", &["value", "threshold"]),
        }
    }
}

/// "45 s", "12 min", "2 h 5 min".
fn write_duration(w: &Words, out: &mut String, ns: u64) {
    let s = ns / 1_000_000_000;
    let _ = if s < 60 {
        w.secs.write(out, &[&s])
    } else if s < 3600 {
        w.mins.write(out, &[&(s / 60)])
    } else {
        w.hours.write(out, &[&(s / 3600), &(s % 3600 / 60)])
    };
}

/// A whole percent, "88%".
fn whole_pct(v: f32) -> String {
    format!("{}%", v.clamp(0., 100.).round() as u32)
}

/// One chip as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipText {
    pub kind: ChipKind,
    pub label: SharedString,
    pub value: SharedString,
    pub tone: Tone,
    pub tooltip: SharedString,
    /// The value and threshold line of the warning popup, for a red chip.
    pub warning: Option<SharedString>,
    /// The page a click opens.
    pub target: Option<DeviceKey>,
}

/// Everything the bar draws. The bar notifies only when this changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarText {
    pub state: MachineState,
    /// "Busy: GPU 88% for 12 min (blender)", or "Calm".
    pub pill: SharedString,
    pub pill_tip: SharedString,
    pub pill_target: Option<DeviceKey>,
    pub chips: SmallVec<[ChipText; 8]>,
    /// "2 warnings" while any chip is red, else empty.
    pub warnings: SharedString,
}

impl BarText {
    /// The number of red chips.
    pub fn warning_count(&self) -> usize {
        self.chips.iter().filter(|c| c.tone == Tone::Danger).count()
    }
}

fn cpu_key() -> DeviceKey {
    DeviceKey(DeviceKey::CPU.into())
}

fn memory_key() -> DeviceKey {
    DeviceKey(DeviceKey::MEMORY.into())
}

/// The page key of the device at `idx`.
fn device_key(devices: &DeviceCatalog, idx: DeviceIdx) -> Option<DeviceKey> {
    devices
        .get(idx)
        .and_then(|e| DeviceKey::from_identity(&e.identity))
}

fn source_target(source: Source, devices: &DeviceCatalog) -> Option<DeviceKey> {
    match source {
        Source::Cpu | Source::CpuTemp | Source::CpuThrottle => Some(cpu_key()),
        Source::MemPressure | Source::MemStall | Source::SwapIn => Some(memory_key()),
        Source::Gpu(idx)
        | Source::GpuTemp(idx)
        | Source::GpuThrottle(idx)
        | Source::Disk(idx)
        | Source::Net(idx) => device_key(devices, idx),
    }
}

/// "GPU 88%", "Swap-in 812 pages/s", "CPU throttling".
fn write_what(w: &Words, out: &mut String, reason: &Reason) {
    let v = reason.value;
    let pct = || whole_pct(v);
    let mut temp = String::new();
    units::celsius(&mut temp, v);
    let _ = match reason.rule.source {
        Source::Cpu => w.cpu.write(out, &[&pct()]),
        Source::Gpu(_) => w.gpu.write(out, &[&pct()]),
        Source::Disk(_) => w.disk.write(out, &[&pct()]),
        Source::Net(_) => w.net.write(out, &[&pct()]),
        Source::MemPressure => w.mem_pressure.write(out, &[&pct_1dp(v)]),
        Source::MemStall => w.mem_stall.write(out, &[&pct_1dp(v)]),
        Source::SwapIn => {
            let mut rate = String::new();
            let _ = w.pages_s.write(&mut rate, &[&(v.round() as u64)]);
            w.swap_in.write(out, &[&rate])
        }
        Source::CpuTemp => w.cpu_temp.write(out, &[&temp]),
        Source::GpuTemp(_) => w.gpu_temp.write(out, &[&temp]),
        Source::CpuThrottle => out.write_str(&w.cpu_throttle),
        Source::GpuThrottle(_) => out.write_str(&w.gpu_throttle),
    };
}

/// "1.2%".
fn pct_1dp(v: f32) -> String {
    let mut s = String::new();
    units::percent(&mut s, v);
    s
}

fn celsius(v: f32) -> String {
    let mut s = String::new();
    units::celsius(&mut s, v);
    s
}

fn pill_tip(w: &Words, reason: Option<&Reason>, reading: &Reading) -> SharedString {
    let Some(reason) = reason else {
        return w.tip_calm.clone();
    };
    // A temperature only gives Hot with a threshold, so `t` is set here.
    let hot = |tip: &Template, t: Option<f32>| {
        let mut s = String::new();
        let _ = tip.write(&mut s, &[&t.map(celsius).unwrap_or_default()]);
        SharedString::from(s)
    };
    match (reason.rule.state, reason.rule.source) {
        (MachineState::Saturated, Source::Cpu) => w.tip_saturated_cpu.clone(),
        (MachineState::Saturated, _) => w.tip_saturated_mem.clone(),
        (_, Source::Cpu) => w.tip_busy_cpu.clone(),
        (_, Source::Gpu(_)) => w.tip_busy_gpu.clone(),
        (_, Source::Disk(_)) => w.tip_busy_disk.clone(),
        (_, Source::Net(_)) => w.tip_busy_net.clone(),
        (_, Source::MemPressure | Source::MemStall) => w.tip_pressure.clone(),
        (_, Source::SwapIn) => w.tip_swap.clone(),
        (_, Source::CpuTemp) => hot(&w.tip_cpu_temp, reading.cpu_hot_c),
        (_, Source::GpuTemp(idx)) => hot(
            &w.tip_gpu_temp,
            reading
                .gpus
                .iter()
                .find(|g| g.idx == idx)
                .and_then(|g| g.hot_c),
        ),
        (_, Source::CpuThrottle | Source::GpuThrottle(_)) => w.tip_throttle.clone(),
    }
}

fn chip_text(w: &Words, chip: &Chip, reading: &Reading, devices: &DeviceCatalog) -> ChipText {
    let mut value = String::new();
    let v = chip.value as f32;
    let threshold_temp = |tip: &Template| -> SharedString {
        match chip.threshold {
            Some(t) => {
                let mut s = String::new();
                let _ = tip.write(&mut s, &[&celsius(t)]);
                s.into()
            }
            None => w.tip_no_threshold.clone(),
        }
    };
    let (label, tooltip, target) = match chip.kind {
        ChipKind::Uptime => {
            units::short_age(&mut value, chip.value as u64);
            (w.chip_uptime.clone(), w.tip_uptime.clone(), Some(cpu_key()))
        }
        ChipKind::Load => {
            let _ = write!(value, "{:.2} / {}", chip.value, reading.logical_cpus);
            (w.chip_load.clone(), w.tip_load.clone(), Some(cpu_key()))
        }
        ChipKind::CpuTemp => {
            units::celsius(&mut value, v);
            (
                w.chip_cpu.clone(),
                threshold_temp(&w.tip_cpu_temp),
                Some(cpu_key()),
            )
        }
        ChipKind::GpuTemp(idx) => {
            units::celsius(&mut value, v);
            (
                w.chip_gpu.clone(),
                threshold_temp(&w.tip_gpu_temp),
                device_key(devices, idx),
            )
        }
        ChipKind::MemPressure => {
            units::percent(&mut value, v);
            (
                w.chip_mem.clone(),
                w.tip_mem_chip.clone(),
                Some(memory_key()),
            )
        }
        ChipKind::SwapIn => {
            let _ = w.pages_s.write(&mut value, &[&(v.round() as u64)]);
            (
                w.chip_swap.clone(),
                w.tip_swap_chip.clone(),
                Some(memory_key()),
            )
        }
        ChipKind::DiskTemp(idx) => {
            units::celsius(&mut value, v);
            (
                w.chip_disk.clone(),
                threshold_temp(&w.tip_disk_temp),
                device_key(devices, idx),
            )
        }
    };
    let warning = (chip.tone == Tone::Danger).then(|| {
        let mut s = String::new();
        let t = chip.threshold.map(celsius).unwrap_or_default();
        let _ = w.warning_row.write(&mut s, &[&value, &t]);
        SharedString::from(s)
    });
    ChipText {
        kind: chip.kind,
        label,
        value: value.into(),
        tone: chip.tone,
        tooltip,
        warning,
        target,
    }
}

/// The status bar. See the module docs.
pub struct StatusBar {
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    rules: StatusRules,
    words: Words,
    text: BarText,
    /// The window each slot shows the bar in, `None` while it does not.
    shown: [Option<AnyWindowHandle>; 2],
    /// The warning popup's actions come back here.
    focus: FocusHandle,
    _sub: Subscription,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
}

impl EventEmitter<EdgesChanged> for StatusBar {}

impl StatusBar {
    fn new(
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            if ev
                .mask
                .intersects(ComponentMask::SYSTEM | ComponentMask::DEVICES)
            {
                this.evaluate(false, cx);
            }
        });
        let id = cx.entity_id();
        let release = demand.clone();
        cx.on_release(move |_, cx| release.update(cx, |d, cx| d.remove(id, cx)))
            .detach();
        let mut this = Self {
            hub,
            demand,
            id,
            rules: StatusRules::default(),
            words: Words::new(cx),
            text: BarText::default(),
            shown: [None; 2],
            focus: cx.focus_handle(),
            _sub: sub,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
        };
        this.evaluate(true, cx);
        this
    }

    /// The snapshot's values once per second: new states, edges and text. `force` evaluates
    /// even within the second (the first build).
    fn evaluate(&mut self, force: bool, cx: &mut Context<Self>) {
        let snap = std::sync::Arc::clone(self.hub.read(cx).current());
        if !force && !self.rules.due(snap.published_ns) {
            return;
        }
        let reading = Reading::from_snapshot(&snap);
        let edges_before: SmallVec<[MachineState; 5]> = rules::EDGE_BOXES
            .iter()
            .map(|k| self.rules.box_state(*k))
            .collect();
        self.rules.step(&reading, Some(&*snap.processes));
        let edges_after: SmallVec<[MachineState; 5]> = rules::EDGE_BOXES
            .iter()
            .map(|k| self.rules.box_state(*k))
            .collect();
        if edges_before != edges_after {
            cx.emit(EdgesChanged);
        }
        if self.words.version != strings_version(cx) {
            self.words = Words::new(cx);
        }
        let text = self.build_text(&reading, &snap);
        if text != self.text {
            self.text = text;
            cx.notify();
        }
    }

    fn build_text(&self, reading: &Reading, snap: &Snapshot) -> BarText {
        let w = &self.words;
        let state = self.rules.state();
        let reason = self.rules.reason();
        let mut pill = String::new();
        let word = &w.states[state as usize];
        match reason {
            None => pill.push_str(word),
            Some(reason) => {
                let mut what = String::new();
                write_what(w, &mut what, reason);
                let mut time = String::new();
                write_duration(w, &mut time, reason.for_ns);
                let _ = match &reason.process {
                    Some(p) => w.reason_process.write(&mut pill, &[word, &what, &time, &p]),
                    None => w.reason.write(&mut pill, &[word, &what, &time]),
                };
            }
        }
        let pill_target = match reason {
            Some(r) => source_target(r.rule.source, &snap.devices),
            None => Some(DeviceKey(OVERVIEW_KEY.into())),
        };
        let mut list = SmallVec::new();
        chips(reading, &mut list);
        let chips: SmallVec<[ChipText; 8]> = list
            .iter()
            .map(|c| chip_text(w, c, reading, &snap.devices))
            .collect();
        let red = chips.iter().filter(|c| c.tone == Tone::Danger).count();
        let mut warnings = String::new();
        if red > 0 {
            let _ = w.warnings.write_count(&mut warnings, red as u64, &[&red]);
        }
        BarText {
            state,
            pill: pill.into(),
            pill_tip: pill_tip(w, reason, reading),
            pill_target,
            chips,
            warnings: warnings.into(),
        }
    }

    /// What the bar draws now.
    pub fn text(&self) -> &BarText {
        &self.text
    }

    /// The state of `kind`'s box for its top edge.
    pub fn box_state(&self, kind: BoxKind) -> MachineState {
        self.rules.box_state(kind)
    }

    /// Everything the bar shows: CPU totals, memory, GPUs, sensors and disks.
    pub fn demand_part() -> Demand {
        Demand {
            any_surface_visible: true,
            cpu: CpuDemand::Total,
            memory: true,
            gpus: true,
            sensors: true,
            disks: true,
            ..Demand::default()
        }
    }

    /// `slot` shows the bar in `window`, or (`None`) no longer shows it. The demand part follows
    /// the first slot that shows it.
    pub fn set_shown(&mut self, slot: Slot, window: Option<AnyWindowHandle>, cx: &mut App) {
        let i = slot as usize;
        if self.shown[i] == window {
            return;
        }
        let before = self.shown.iter().find_map(|w| *w);
        self.shown[i] = window;
        let after = self.shown.iter().find_map(|w| *w);
        if before == after {
            return;
        }
        let id = self.id;
        match after {
            Some(window) => self
                .demand
                .update(cx, |d, cx| d.set(id, Some(window), Self::demand_part(), cx)),
            None => self.demand.update(cx, |d, cx| d.remove(id, cx)),
        }
    }

    /// Whether any slot shows the bar.
    pub fn is_shown(&self) -> bool {
        self.shown.iter().any(Option::is_some)
    }

    /// A pill, chip or popup row was clicked.
    fn on_open(&mut self, a: &PerfStatusOpen, window: &mut Window, cx: &mut Context<Self>) {
        open_page(a.key.clone(), window, cx);
    }

    /// The popup under the warning button: one row per red chip with its value and threshold.
    fn open_warnings(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows: SmallVec<[(SharedString, SharedString, Option<DeviceKey>); 4]> = self
            .text
            .chips
            .iter()
            .filter_map(|c| Some((c.label.clone(), c.warning.clone()?, c.target.clone())))
            .collect();
        if rows.is_empty() {
            return;
        }
        let menu = TlMenu::build(window, cx, self.focus.clone(), move |mut menu, _, _| {
            for (label, value, target) in &rows {
                menu = match target {
                    Some(key) => menu
                        .item(label.clone(), Box::new(PerfStatusOpen { key: key.clone() }))
                        .value(value.clone()),
                    None => menu.item_disabled(format!("{label} {value}"), None),
                };
            }
            menu
        });
        open_context_menu_at(ev.position, window, cx, menu);
    }
}

/// Selects `key` on the Performance tab, switching to that tab first when the main window shows
/// another. Without a main window (a test host) the action goes up from the bar.
pub fn open_page(key: DeviceKey, window: &mut Window, cx: &mut App) {
    let perf = main_window_view(cx).and_then(|main| {
        main.update(cx, |m, cx| m.select_tab(Tab::Performance, window, cx));
        main.read(cx)
            .page_entity::<PerformancePage>(Tab::Performance)
    });
    match perf {
        Some(perf) => {
            let sidebar = perf.read(cx).sidebar.clone();
            sidebar.update(cx, |s, cx| s.select(key, cx));
        }
        None => window.dispatch_action(Box::new(PerfSelectDevice { key }), cx),
    }
}

fn tone_color(tone: Tone, theme: &TlTheme) -> Option<Hsla> {
    match tone {
        Tone::Neutral => None,
        Tone::Warning => Some(theme.status.warning),
        Tone::Danger => Some(theme.status.danger),
    }
}

/// The colour of a state's edge or pill; `None` for Calm.
pub fn state_color(state: MachineState, theme: &TlTheme) -> Option<Hsla> {
    tone_color(state.tone(), theme)
}

/// `el` opens `target` on a left click.
fn opens(el: Stateful<Div>, target: Option<DeviceKey>) -> Stateful<Div> {
    match target {
        Some(key) => el
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                open_page(key.clone(), window, cx);
            }),
        None => el,
    }
}

fn with_tip(
    id: &'static str,
    key: impl Into<SharedString>,
    el: Stateful<Div>,
    tip: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let key: SharedString = key.into();
    let state = HoverTooltip::state(ElementId::Name(format!("{id}:{key}").into()), window, cx);
    HoverTooltip::attach(
        &state,
        el,
        DELAY_CONTROL,
        TlTooltip::text(tip).build(),
        window,
        cx,
    )
}

fn pill(
    text: &BarText,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let s = m.ui_scale;
    let color = state_color(text.state, theme).unwrap_or(theme.status.success);
    let glyph = match text.state {
        MachineState::Calm => TlIcon::CircleCheck,
        MachineState::Busy | MachineState::Pressure | MachineState::Hot => TlIcon::TriangleAlert,
        MachineState::Saturated => TlIcon::CircleX,
    };
    let mut label = div();
    *label.text_style() = label_style(m, m.text.md, FontWeight::SEMIBOLD);
    let label = label
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .child(text.pill.clone());
    let el = div()
        .id(PILL_ID)
        .debug_selector(|| PILL_ID.into())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink(1.)
        .min_w_0()
        .h(px(PILL_H * s))
        .px(m.sp[5])
        .gap(m.sp[3])
        .rounded(px(PILL_H / 2. * s))
        .bg(theme.list_bg)
        .border_1()
        .border_color(if text.state == MachineState::Calm {
            theme.divider
        } else {
            color
        })
        .child(icon(glyph, m.icon.md, color))
        .child(label);
    let el = opens(el, text.pill_target.clone());
    with_tip(PILL_ID, "", el, text.pill_tip.clone(), window, cx).into_any_element()
}

fn chip(
    c: &ChipText,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let s = m.ui_scale;
    let id = chip_id(c.kind);
    let color = tone_color(c.tone, theme);
    let mut label = div();
    *label.text_style() = t_sm(m);
    let label = label
        .text_color(theme.text_secondary)
        .child(c.label.clone());
    let mut value = div();
    *value.text_style() = numeric_style(m, m.text.sm);
    let value = value
        .text_color(color.unwrap_or(theme.text_primary))
        .child(c.value.clone());
    let selector = id.clone();
    let el = div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || selector.clone())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .whitespace_nowrap()
        .h(px(CHIP_H * s))
        .px(m.sp[5])
        .gap(m.sp[3])
        .rounded(px(CHIP_H / 2. * s))
        .bg(theme.list_bg)
        .border_1()
        .border_color(color.unwrap_or(theme.divider))
        .child(label)
        .child(value);
    let el = opens(el, c.target.clone());
    with_tip("perf-status-chip", id, el, c.tooltip.clone(), window, cx).into_any_element()
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let pill = pill(&self.text, &m, &theme, window, cx);
        let chips: SmallVec<[AnyElement; 8]> = self
            .text
            .chips
            .iter()
            .map(|c| chip(c, &m, &theme, window, cx))
            .collect();
        let warnings = (!self.text.warnings.is_empty()).then(|| {
            let mut label = div();
            *label.text_style() = t_sm(&m);
            div()
                .id(WARNINGS_ID)
                .debug_selector(|| WARNINGS_ID.into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .h(px(CHIP_H * s))
                .px(m.sp[5])
                .gap(m.sp[3])
                .rounded(px(CHIP_H / 2. * s))
                .bg(theme.list_bg)
                .border_1()
                .border_color(theme.status.danger)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_warnings(ev, window, cx);
                    }),
                )
                .child(icon(TlIcon::TriangleAlert, m.icon.sm, theme.status.danger))
                .child(
                    label
                        .text_color(theme.text_primary)
                        .child(self.text.warnings.clone()),
                )
        });
        div()
            .id(BAR_ID)
            .debug_selector(|| BAR_ID.into())
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_open))
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap(m.sp[3])
            .w_full()
            .min_h(px(BAR_H * s))
            .p(m.sp[4])
            .bg(theme.toolbar)
            .border_b_1()
            .border_color(theme.divider)
            .child(pill)
            .children(chips)
            .child(div().flex_1())
            .children(warnings)
    }
}
