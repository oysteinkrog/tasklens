//! The machine status rules of the status bar (UI-PRF-054, D-030): the five states with their
//! thresholds and windows, the 10 s hold, the chip tones and the top process for the reason.
//!
//! [`Reading::from_snapshot`] takes what the rules need from one snapshot. [`StatusRules::step`]
//! runs once per second of snapshot time: it tracks how long each rule has been true, picks the
//! highest state whose window has passed, and holds a shown state for at least [`HOLD_S`]
//! before it may drop. The same rules, limited to one box's device, give the Overview box state
//! edges. Nothing here draws or formats text.

use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::{ProcessSnapshot, Snapshot};
use tl_model::system::Metric;
use tl_model::time::TimeNs;

use crate::pages::performance::overview::layout::BoxKind;

/// CPU, GPU, disk active time or network use above this percent gives `Busy`.
pub const BUSY_PCT: f32 = 80.;
/// How long a `Busy` rule must hold, s.
pub const BUSY_FOR_S: u64 = 30;
/// Memory PSI `some` avg10 at or above this percent gives `Pressure`.
pub const PRESSURE_PSI_PCT: f32 = 5.;
/// Swap-in above this many pages per second gives `Pressure`.
pub const SWAP_IN_PPS: f32 = 500.;
/// How long the swap-in rule must hold, s.
pub const SWAP_IN_FOR_S: u64 = 10;
/// How long a temperature at or above its threshold must hold for `Hot`, s.
pub const HOT_FOR_S: u64 = 30;
/// CPU above this percent gives `Saturated`.
pub const SATURATED_CPU_PCT: f32 = 95.;
/// How long the CPU `Saturated` rule must hold, s.
pub const SATURATED_FOR_S: u64 = 60;
/// Memory PSI `full` avg10 at or above this percent gives `Saturated`.
pub const SATURATED_PSI_FULL_PCT: f32 = 2.;
/// A shown state holds at least this long before it may drop, s.
pub const HOLD_S: u64 = 10;
/// A process sweep older than this is not running: the slowest refresh is 10 s.
const SWEEP_FRESH_NS: u64 = 12 * NS;

const NS: u64 = 1_000_000_000;

/// The hwmon chips the engine reads the CPU temperature from, in its order (tl-collect
/// `CPU_TEMP_CHIPS`), and the labels it prefers on them.
const CPU_TEMP_CHIPS: [&str; 4] = ["k10temp", "zenpower", "coretemp", "cpu_thermal"];
const CPU_TEMP_LABELS: [&str; 2] = ["Tctl", "Package id 0"];

/// The machine states, lowest first. The highest true one wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum MachineState {
    #[default]
    Calm,
    Busy,
    Pressure,
    Hot,
    Saturated,
}

impl MachineState {
    /// `status.warning` for Busy and Pressure, `status.danger` for Hot and Saturated.
    pub fn tone(self) -> Tone {
        match self {
            Self::Calm => Tone::Neutral,
            Self::Busy | Self::Pressure => Tone::Warning,
            Self::Hot | Self::Saturated => Tone::Danger,
        }
    }
}

/// The colour of a chip, an edge or the pill.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Warning,
    Danger,
}

/// What a rule watches.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// CPU total percent.
    Cpu,
    /// GPU utilization percent.
    Gpu(DeviceIdx),
    /// Disk active time percent.
    Disk(DeviceIdx),
    /// Network use: the busier direction as a percent of the link speed.
    Net(DeviceIdx),
    /// Memory PSI `some` avg10 percent.
    MemPressure,
    /// Memory PSI `full` avg10 percent.
    MemStall,
    /// Pages swapped in per second.
    SwapIn,
    CpuTemp,
    GpuTemp(DeviceIdx),
    CpuThrottle,
    GpuThrottle(DeviceIdx),
}

impl Source {
    /// The Overview box that shows this source's device.
    pub fn box_kind(self) -> BoxKind {
        match self {
            Self::Cpu | Self::CpuTemp | Self::CpuThrottle => BoxKind::Cpu,
            Self::MemPressure | Self::MemStall | Self::SwapIn => BoxKind::Memory,
            Self::Gpu(_) | Self::GpuTemp(_) | Self::GpuThrottle(_) => BoxKind::Gpu,
            Self::Disk(_) => BoxKind::Disk,
            Self::Net(_) => BoxKind::Net,
        }
    }
}

/// One rule: the state it gives and what it watches.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rule {
    pub state: MachineState,
    pub source: Source,
}

impl Rule {
    /// How long the rule must be true before it counts, ns.
    fn window_ns(self) -> u64 {
        let s = match (self.state, self.source) {
            (MachineState::Busy, _) => BUSY_FOR_S,
            (MachineState::Pressure, Source::SwapIn) => SWAP_IN_FOR_S,
            (MachineState::Hot, Source::CpuTemp | Source::GpuTemp(_)) => HOT_FOR_S,
            (MachineState::Saturated, Source::Cpu) => SATURATED_FOR_S,
            _ => 0,
        };
        s * NS
    }
}

/// Why the shown state is what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Reason {
    pub rule: Rule,
    /// The value at the last step: percent, °C or pages/s. NaN for a throttling flag.
    pub value: f32,
    /// How long the rule has been true, ns.
    pub for_ns: u64,
    /// The top process by the rule's resource, while a process sweep runs.
    pub process: Option<Arc<str>>,
}

/// One GPU's values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuReading {
    pub idx: DeviceIdx,
    pub util_pct: Option<f32>,
    pub temp_c: Option<f32>,
    /// The device catalog's `hot_c` (ADR-016).
    pub hot_c: Option<f32>,
    pub throttling: bool,
}

/// One disk's values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiskReading {
    pub idx: DeviceIdx,
    pub active_pct: Option<f32>,
    pub temp_c: Option<f32>,
    pub hot_c: Option<f32>,
}

/// What the rules and chips read from one snapshot. `None` is a value that is not valid now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// CLOCK_BOOTTIME of the snapshot, ns. Also the uptime.
    pub t_ns: TimeNs,
    pub cpu_pct: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub cpu_hot_c: Option<f32>,
    pub cpu_throttling: bool,
    pub gpus: SmallVec<[GpuReading; 2]>,
    pub disks: SmallVec<[DiskReading; 4]>,
    /// Per adapter with a link speed: the busier direction as a percent of it.
    pub nets: SmallVec<[(DeviceIdx, f32); 4]>,
    pub psi_some_pct: Option<f32>,
    pub psi_full_pct: Option<f32>,
    pub swap_in_pps: Option<f32>,
    pub load1: Option<f32>,
    pub logical_cpus: u32,
}

fn valid(m: Metric<f32>) -> Option<f32> {
    (m.validity.is_valid() && m.value.is_finite()).then_some(m.value)
}

fn flag(m: Metric<bool>) -> bool {
    m.validity.is_valid() && m.value
}

/// The CPU temperature's `hot_c`: the sensor the engine reads `CpuTotals.temp_c` from, the
/// first CPU chip in the engine's order, the `Tctl` or `Package id 0` input on it, else its
/// first input.
pub fn cpu_hot_c(devices: &DeviceCatalog) -> Option<f32> {
    let sensors = || {
        devices.entries.iter().filter_map(|e| match &e.identity {
            DeviceIdentity::Sensor { chip, label } if e.present => Some((chip, label, e.hot_c)),
            _ => None,
        })
    };
    let chip = CPU_TEMP_CHIPS
        .iter()
        .find(|wanted| sensors().any(|(chip, _, _)| &**chip == **wanted))?;
    let on_chip = || sensors().filter(|(c, _, _)| &***c == *chip);
    on_chip()
        .find(|(_, label, _)| CPU_TEMP_LABELS.contains(&&***label))
        .or_else(|| on_chip().next())
        .and_then(|(_, _, hot)| hot)
}

impl Reading {
    pub fn from_snapshot(snap: &Snapshot) -> Self {
        let sys = &snap.system;
        let hot = |idx: DeviceIdx| snap.devices.get(idx).and_then(|e| e.hot_c);
        let gpus = sys
            .gpus
            .iter()
            .map(|(idx, g)| GpuReading {
                idx: *idx,
                util_pct: valid(g.util_pct),
                temp_c: valid(g.temp_c),
                hot_c: hot(*idx),
                throttling: flag(g.throttling),
            })
            .collect();
        let disks = sys
            .disks
            .iter()
            .map(|(idx, d)| DiskReading {
                idx: *idx,
                active_pct: valid(d.active_pct),
                temp_c: valid(d.temp_c),
                hot_c: hot(*idx),
            })
            .collect();
        let nets = sys
            .nets
            .iter()
            .filter_map(|(idx, n)| {
                let speed = n.link_speed_mbps;
                if !speed.validity.is_valid() || speed.value == 0 {
                    return None;
                }
                let rate = |m: Metric<f64>| if m.validity.is_valid() { m.value } else { 0. };
                let busier = rate(n.rx_bps).max(rate(n.tx_bps));
                let pct = busier * 8. / (f64::from(speed.value) * 1e6) * 100.;
                pct.is_finite().then_some((*idx, pct as f32))
            })
            .collect();
        let load = sys.cpu.load_avg;
        Self {
            t_ns: snap.published_ns,
            cpu_pct: valid(sys.cpu.util_pct),
            cpu_temp_c: valid(sys.cpu.temp_c),
            cpu_hot_c: cpu_hot_c(&snap.devices),
            cpu_throttling: flag(sys.cpu.throttling),
            gpus,
            disks,
            nets,
            psi_some_pct: valid(sys.memory.psi_some_pct),
            psi_full_pct: valid(sys.memory.psi_full_pct),
            swap_in_pps: valid(sys.memory.swap_in_pps),
            load1: (load.validity.is_valid() && load.value[0].is_finite()).then_some(load.value[0]),
            logical_cpus: sys.cpu_topology.online.count(),
        }
    }

    /// Every rule that is true now, with its value, in a fixed order.
    fn true_rules(&self, out: &mut SmallVec<[(Rule, f32); 16]>) {
        use MachineState::{Busy, Hot, Pressure, Saturated};
        let mut push = |state, source, value| out.push((Rule { state, source }, value));
        if let Some(v) = self.cpu_pct {
            if v > SATURATED_CPU_PCT {
                push(Saturated, Source::Cpu, v);
            }
            if v > BUSY_PCT {
                push(Busy, Source::Cpu, v);
            }
        }
        if let Some(v) = self.psi_full_pct.filter(|v| *v >= SATURATED_PSI_FULL_PCT) {
            push(Saturated, Source::MemStall, v);
        }
        if let (Some(t), Some(hot)) = (self.cpu_temp_c, self.cpu_hot_c)
            && t >= hot
        {
            push(Hot, Source::CpuTemp, t);
        }
        if self.cpu_throttling {
            push(Hot, Source::CpuThrottle, f32::NAN);
        }
        for g in &self.gpus {
            if let (Some(t), Some(hot)) = (g.temp_c, g.hot_c)
                && t >= hot
            {
                push(Hot, Source::GpuTemp(g.idx), t);
            }
            if g.throttling {
                push(Hot, Source::GpuThrottle(g.idx), f32::NAN);
            }
            if let Some(v) = g.util_pct.filter(|v| *v > BUSY_PCT) {
                push(Busy, Source::Gpu(g.idx), v);
            }
        }
        if let Some(v) = self.psi_some_pct.filter(|v| *v >= PRESSURE_PSI_PCT) {
            push(Pressure, Source::MemPressure, v);
        }
        if let Some(v) = self.swap_in_pps.filter(|v| *v > SWAP_IN_PPS) {
            push(Pressure, Source::SwapIn, v);
        }
        for d in &self.disks {
            if let Some(v) = d.active_pct.filter(|v| *v > BUSY_PCT) {
                push(Busy, Source::Disk(d.idx), v);
            }
        }
        for &(idx, v) in &self.nets {
            if v > BUSY_PCT {
                push(Busy, Source::Net(idx), v);
            }
        }
    }
}

/// A shown state and when it was entered.
#[derive(Clone, Debug, Default, PartialEq)]
struct Held {
    state: MachineState,
    since_ns: TimeNs,
    reason: Option<Reason>,
}

impl Held {
    /// Rises at once; drops only after the state was shown [`HOLD_S`].
    fn step(&mut self, raw: MachineState, reason: Option<Reason>, now: TimeNs) {
        let drop_ok = now.saturating_sub(self.since_ns) >= HOLD_S * NS;
        if raw > self.state || (raw < self.state && drop_ok) {
            self.state = raw;
            self.since_ns = now;
            self.reason = reason;
        } else if raw == self.state {
            self.reason = reason;
        }
    }
}

/// The box kinds that get a state edge, in [`StatusRules::boxes`] order.
pub const EDGE_BOXES: [BoxKind; 5] = [
    BoxKind::Cpu,
    BoxKind::Memory,
    BoxKind::Gpu,
    BoxKind::Disk,
    BoxKind::Net,
];

/// The rule tracker. One per status bar.
#[derive(Debug, Default)]
pub struct StatusRules {
    /// Each true rule and when it became true.
    since: SmallVec<[(Rule, TimeNs); 16]>,
    shown: Held,
    /// Per box in [`EDGE_BOXES`] order.
    boxes: [Held; 5],
    last_ns: Option<TimeNs>,
}

impl StatusRules {
    /// Whether `t_ns` is at least a second after the last step (or the first, or after the clock
    /// went back).
    pub fn due(&self, t_ns: TimeNs) -> bool {
        self.last_ns
            .is_none_or(|last| t_ns < last || t_ns - last >= NS)
    }

    /// One evaluation. `processes` names the top process while a sweep runs. Returns whether
    /// the shown state, its reason or a box state changed.
    pub fn step(&mut self, r: &Reading, processes: Option<&ProcessSnapshot>) -> bool {
        let now = r.t_ns;
        if self.last_ns.is_some_and(|last| now < last) {
            // A new engine session: start over.
            *self = Self::default();
        }
        if self.last_ns.is_none() {
            self.shown.since_ns = now;
            for b in &mut self.boxes {
                b.since_ns = now;
            }
        }
        self.last_ns = Some(now);
        let mut truths = SmallVec::new();
        r.true_rules(&mut truths);
        self.since
            .retain(|(rule, _)| truths.iter().any(|(t, _)| t == rule));
        for (rule, _) in &truths {
            if !self.since.iter().any(|(r, _)| r == rule) {
                self.since.push((*rule, now));
            }
        }
        // Rules whose window has passed, with value and how long they held.
        let active: SmallVec<[(Rule, f32, u64); 16]> = truths
            .iter()
            .filter_map(|&(rule, value)| {
                let since = self.since.iter().find(|(r, _)| *r == rule)?.1;
                let held = now - since;
                (held >= rule.window_ns()).then_some((rule, value, held))
            })
            .collect();
        let sweep = processes.filter(|p| {
            p.header.validity.is_valid()
                && !p.records.is_empty()
                && now.saturating_sub(p.header.observed_end) <= SWEEP_FRESH_NS
        });
        let before = (self.shown.clone(), self.boxes.clone());
        let (raw, reason) = pick(&active, |_| true, sweep);
        self.shown.step(raw, reason, now);
        for (held, kind) in self.boxes.iter_mut().zip(EDGE_BOXES) {
            let (raw, reason) = pick(&active, |s| s.box_kind() == kind, None);
            held.step(raw, reason, now);
        }
        before != (self.shown.clone(), self.boxes.clone())
    }

    pub fn state(&self) -> MachineState {
        self.shown.state
    }

    /// `None` while Calm.
    pub fn reason(&self) -> Option<&Reason> {
        self.shown.reason.as_ref()
    }

    /// The state of `kind`'s device, for its edge. `TopProcs` is always Calm.
    pub fn box_state(&self, kind: BoxKind) -> MachineState {
        EDGE_BOXES
            .iter()
            .position(|k| *k == kind)
            .map_or(MachineState::Calm, |i| self.boxes[i].state)
    }
}

/// The highest state among the active rules `keep` accepts, and the reason: the rule of that
/// state with the highest value (a throttling flag last).
fn pick(
    active: &[(Rule, f32, u64)],
    keep: impl Fn(Source) -> bool,
    sweep: Option<&ProcessSnapshot>,
) -> (MachineState, Option<Reason>) {
    let kept = || active.iter().filter(|(r, _, _)| keep(r.source));
    let Some(state) = kept().map(|(r, _, _)| r.state).max() else {
        return (MachineState::Calm, None);
    };
    let rank = |v: f32| if v.is_nan() { f32::NEG_INFINITY } else { v };
    let best =
        kept()
            .filter(|(r, _, _)| r.state == state)
            .fold(None::<&(Rule, f32, u64)>, |best, cur| match best {
                Some(b) if rank(b.1) >= rank(cur.1) => Some(b),
                _ => Some(cur),
            });
    let reason = best.map(|&(rule, value, for_ns)| Reason {
        rule,
        value,
        for_ns,
        process: sweep
            .and_then(|p| top_process(&p.records, rule.source))
            .map(|r| Arc::clone(&r.meta.name)),
    });
    (state, reason)
}

/// The live process that uses most of `source`'s resource, `None` when none uses any.
pub fn top_process(records: &[ProcessRecord], source: Source) -> Option<&ProcessRecord> {
    let by: fn(&ProcessRecord) -> f64 = match source {
        Source::Cpu | Source::CpuTemp | Source::CpuThrottle => |r| f64::from(r.cpu_pct),
        Source::Gpu(_) | Source::GpuTemp(_) | Source::GpuThrottle(_) => {
            |r| f64::from(r.gpu_util_pct)
        }
        Source::Disk(_) => |r| f64::from(r.io_read_bps) + f64::from(r.io_write_bps),
        Source::Net(_) => |r| f64::from(r.net_rx_bps) + f64::from(r.net_tx_bps),
        Source::MemPressure | Source::MemStall => |r| r.rss_anon_bytes as f64,
        Source::SwapIn => |r| r.swap_bytes as f64,
    };
    records
        .iter()
        .filter(|r| !r.flags.contains(ProcFlags::EXITED))
        .map(|r| (by(r), r))
        .filter(|(v, _)| v.is_finite() && *v > 0.)
        .fold(None::<(f64, &ProcessRecord)>, |best, cur| match best {
            Some(b) if b.0 >= cur.0 => Some(b),
            _ => Some(cur),
        })
        .map(|(_, r)| r)
}

/// What a chip shows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ChipKind {
    Uptime,
    Load,
    CpuTemp,
    GpuTemp(DeviceIdx),
    MemPressure,
    SwapIn,
    /// The hottest disk.
    DiskTemp(DeviceIdx),
}

/// One chip: its value now, its tone and the threshold its tone uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chip {
    pub kind: ChipKind,
    /// Seconds for Uptime, the load average for Load, °C, percent or pages/s.
    pub value: f64,
    pub tone: Tone,
    /// The warning or danger threshold, `None` when the chip has none.
    pub threshold: Option<f32>,
}

/// A temperature chip: red at or above `hot`, neutral without one.
fn temp_chip(kind: ChipKind, t: f32, hot: Option<f32>) -> Chip {
    Chip {
        kind,
        value: f64::from(t),
        tone: if hot.is_some_and(|h| t >= h) {
            Tone::Danger
        } else {
            Tone::Neutral
        },
        threshold: hot,
    }
}

/// The chips in bar order. A chip whose value is not valid is left out.
pub fn chips(r: &Reading, out: &mut SmallVec<[Chip; 8]>) {
    out.clear();
    out.push(Chip {
        kind: ChipKind::Uptime,
        value: (r.t_ns / NS) as f64,
        tone: Tone::Neutral,
        threshold: None,
    });
    if let Some(load) = r.load1 {
        out.push(Chip {
            kind: ChipKind::Load,
            value: f64::from(load),
            tone: Tone::Neutral,
            threshold: None,
        });
    }
    if let Some(t) = r.cpu_temp_c {
        out.push(temp_chip(ChipKind::CpuTemp, t, r.cpu_hot_c));
    }
    for g in &r.gpus {
        if let Some(t) = g.temp_c {
            out.push(temp_chip(ChipKind::GpuTemp(g.idx), t, g.hot_c));
        }
    }
    if let Some(v) = r.psi_some_pct {
        out.push(Chip {
            kind: ChipKind::MemPressure,
            value: f64::from(v),
            tone: if v >= PRESSURE_PSI_PCT {
                Tone::Warning
            } else {
                Tone::Neutral
            },
            threshold: Some(PRESSURE_PSI_PCT),
        });
    }
    if let Some(v) = r.swap_in_pps {
        out.push(Chip {
            kind: ChipKind::SwapIn,
            value: f64::from(v),
            tone: if v > SWAP_IN_PPS {
                Tone::Warning
            } else {
                Tone::Neutral
            },
            threshold: Some(SWAP_IN_PPS),
        });
    }
    let hottest = r.disks.iter().filter_map(|d| Some((d, d.temp_c?))).fold(
        None::<(&DiskReading, f32)>,
        |best, cur| match best {
            Some(b) if b.1 >= cur.1 => Some(b),
            _ => Some(cur),
        },
    );
    if let Some((d, t)) = hottest {
        out.push(temp_chip(ChipKind::DiskTemp(d.idx), t, d.hot_c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::DeviceEntry;
    use tl_model::ids::ProcessKey;
    use tl_model::process::{GroupKind, ProcState, ProcessMeta};
    use tl_model::snapshot::ComponentHeader;
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;
    use tl_model::validity::Validity;

    const GPU: DeviceIdx = DeviceIdx(2);

    fn calm(t_s: u64) -> Reading {
        Reading {
            t_ns: t_s * NS,
            cpu_pct: Some(10.),
            cpu_temp_c: Some(50.),
            psi_some_pct: Some(0.),
            psi_full_pct: Some(0.),
            swap_in_pps: Some(0.),
            load1: Some(1.),
            logical_cpus: 8,
            gpus: SmallVec::from_slice(&[GpuReading {
                idx: GPU,
                util_pct: Some(5.),
                temp_c: Some(40.),
                hot_c: Some(90.),
                throttling: false,
            }]),
            ..Reading::default()
        }
    }

    /// Steps `rules` once per second from `from` to `to` (inclusive) with `f` applied to a calm
    /// reading.
    fn run(rules: &mut StatusRules, from: u64, to: u64, f: impl Fn(&mut Reading)) {
        for t in from..=to {
            let mut r = calm(t);
            f(&mut r);
            rules.step(&r, None);
        }
    }

    #[test]
    fn status_bar_state_rules() {
        // Busy: CPU above 80% for 30 s, not before.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 29, |r| r.cpu_pct = Some(88.));
        assert_eq!(rules.state(), MachineState::Calm, "29 s is not enough");
        run(&mut rules, 30, 30, |r| r.cpu_pct = Some(88.));
        assert_eq!(rules.state(), MachineState::Busy);
        let reason = rules.reason().expect("reason");
        assert_eq!(reason.rule.source, Source::Cpu);
        assert_eq!(reason.for_ns, 30 * NS);
        assert_eq!(rules.box_state(BoxKind::Cpu), MachineState::Busy);
        assert_eq!(rules.box_state(BoxKind::Memory), MachineState::Calm);

        // The 10 s hold: CPU drops at 31 s, the state stays Busy until 40 s.
        run(&mut rules, 31, 39, |_| {});
        assert_eq!(rules.state(), MachineState::Busy, "held");
        run(&mut rules, 40, 40, |_| {});
        assert_eq!(rules.state(), MachineState::Calm, "dropped after 10 s");

        // Saturated: CPU above 95% for 60 s; Busy comes first at 30 s.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 59, |r| r.cpu_pct = Some(98.));
        assert_eq!(rules.state(), MachineState::Busy);
        run(&mut rules, 60, 60, |r| r.cpu_pct = Some(98.));
        assert_eq!(rules.state(), MachineState::Saturated);

        // Saturated at once from memory PSI full, which outranks Pressure from PSI some.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 0, |r| {
            r.psi_full_pct = Some(2.);
            r.psi_some_pct = Some(9.);
        });
        assert_eq!(rules.state(), MachineState::Saturated);
        assert_eq!(
            rules.reason().expect("reason").rule.source,
            Source::MemStall
        );
        assert_eq!(rules.box_state(BoxKind::Memory), MachineState::Saturated);

        // Pressure: PSI some at or above 5% at once; swap-in above 500 pages/s for 10 s.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 0, |r| r.psi_some_pct = Some(5.));
        assert_eq!(rules.state(), MachineState::Pressure);
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 9, |r| r.swap_in_pps = Some(800.));
        assert_eq!(rules.state(), MachineState::Calm);
        run(&mut rules, 10, 10, |r| r.swap_in_pps = Some(800.));
        assert_eq!(rules.state(), MachineState::Pressure);
        assert_eq!(rules.reason().expect("reason").rule.source, Source::SwapIn);

        // Hot: a GPU at its threshold for 30 s (the 30 s hot window); throttling at once.
        let mut rules = StatusRules::default();
        let hot = |r: &mut Reading| r.gpus[0].temp_c = Some(91.);
        run(&mut rules, 0, 29, hot);
        assert_eq!(rules.state(), MachineState::Calm, "hot window not passed");
        run(&mut rules, 30, 30, hot);
        assert_eq!(rules.state(), MachineState::Hot);
        assert_eq!(rules.box_state(BoxKind::Gpu), MachineState::Hot);
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 0, |r| r.cpu_throttling = true);
        assert_eq!(rules.state(), MachineState::Hot);
        assert!(rules.reason().expect("reason").value.is_nan());

        // A dip resets a window: 20 s busy, 1 s idle, then 29 s busy is not Busy.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 19, |r| r.cpu_pct = Some(90.));
        run(&mut rules, 20, 20, |_| {});
        run(&mut rules, 21, 49, |r| r.cpu_pct = Some(90.));
        assert_eq!(rules.state(), MachineState::Calm);

        // A higher state replaces a lower one at once, inside the hold.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 0, |r| r.psi_some_pct = Some(6.));
        run(&mut rules, 1, 1, |r| {
            r.psi_some_pct = Some(6.);
            r.cpu_throttling = true;
        });
        assert_eq!(rules.state(), MachineState::Hot);
        assert_eq!(rules.box_state(BoxKind::Disk), MachineState::Calm);
        assert_eq!(rules.box_state(BoxKind::TopProcs), MachineState::Calm);
    }

    #[test]
    fn status_bar_disk_and_net_busy() {
        let mut rules = StatusRules::default();
        let disk = DeviceIdx(3);
        let net = DeviceIdx(5);
        run(&mut rules, 0, 30, |r| {
            r.disks.push(DiskReading {
                idx: disk,
                active_pct: Some(85.),
                temp_c: None,
                hot_c: None,
            });
            r.nets.push((net, 90.));
        });
        assert_eq!(rules.state(), MachineState::Busy);
        // The busier one names the reason.
        assert_eq!(
            rules.reason().expect("reason").rule.source,
            Source::Net(net)
        );
        assert_eq!(rules.box_state(BoxKind::Disk), MachineState::Busy);
        assert_eq!(rules.box_state(BoxKind::Net), MachineState::Busy);
        assert_eq!(rules.box_state(BoxKind::Cpu), MachineState::Calm);
    }

    #[test]
    fn status_bar_hot_needs_threshold() {
        // 97 °C without hot_c: never Hot, and its chip stays neutral.
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 120, |r| {
            r.cpu_temp_c = Some(97.);
            r.cpu_hot_c = None;
        });
        assert_eq!(rules.state(), MachineState::Calm);
        assert_eq!(rules.box_state(BoxKind::Cpu), MachineState::Calm);
        let mut r = calm(0);
        r.cpu_temp_c = Some(97.);
        let mut out = SmallVec::new();
        chips(&r, &mut out);
        let chip = out
            .iter()
            .find(|c| c.kind == ChipKind::CpuTemp)
            .expect("CPU temperature chip");
        assert_eq!(chip.tone, Tone::Neutral);
        assert_eq!(chip.threshold, None);
        // With a threshold the same temperature is red, and Hot after 30 s.
        r.cpu_hot_c = Some(95.);
        chips(&r, &mut out);
        let chip = out.iter().find(|c| c.kind == ChipKind::CpuTemp).unwrap();
        assert_eq!(chip.tone, Tone::Danger);
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 30, |r| {
            r.cpu_temp_c = Some(97.);
            r.cpu_hot_c = Some(95.);
        });
        assert_eq!(rules.state(), MachineState::Hot);
    }

    #[test]
    fn status_bar_chip_tones() {
        let mut r = calm(3_700);
        r.psi_some_pct = Some(5.);
        r.swap_in_pps = Some(600.);
        r.gpus[0].temp_c = Some(92.);
        r.disks.push(DiskReading {
            idx: DeviceIdx(3),
            active_pct: Some(1.),
            temp_c: Some(41.),
            hot_c: None,
        });
        r.disks.push(DiskReading {
            idx: DeviceIdx(4),
            active_pct: Some(1.),
            temp_c: Some(48.),
            hot_c: None,
        });
        let mut out = SmallVec::new();
        chips(&r, &mut out);
        let kinds: Vec<ChipKind> = out.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                ChipKind::Uptime,
                ChipKind::Load,
                ChipKind::CpuTemp,
                ChipKind::GpuTemp(GPU),
                ChipKind::MemPressure,
                ChipKind::SwapIn,
                ChipKind::DiskTemp(DeviceIdx(4)),
            ]
        );
        let tone = |k| out.iter().find(|c| c.kind == k).unwrap().tone;
        assert_eq!(tone(ChipKind::Uptime), Tone::Neutral);
        assert_eq!(tone(ChipKind::GpuTemp(GPU)), Tone::Danger);
        assert_eq!(tone(ChipKind::MemPressure), Tone::Warning);
        assert_eq!(tone(ChipKind::SwapIn), Tone::Warning);
        assert_eq!(tone(ChipKind::DiskTemp(DeviceIdx(4))), Tone::Neutral);
        assert_eq!(out[0].value, 3_700.);
    }

    fn record(pid: u32, name: &str, cpu: f32, gpu: f32) -> ProcessRecord {
        let name: Arc<str> = Arc::from(name);
        let user: Arc<str> = Arc::from("user");
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: BootId(0),
                pid,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: Arc::clone(&name),
                name: Arc::clone(&name),
                exe: None,
                cmdline: None,
                user,
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: name,
            }),
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: cpu,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: gpu,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn sweep(t_ns: TimeNs, records: Vec<ProcessRecord>) -> ProcessSnapshot {
        ProcessSnapshot {
            header: ComponentHeader {
                version: 1,
                observed_start: t_ns,
                observed_end: t_ns,
                validity: Validity::Valid,
            },
            records: Arc::from(records),
            ..ProcessSnapshot::default()
        }
    }

    #[test]
    fn status_bar_reason_names_top_process() {
        let busy_gpu = |r: &mut Reading| r.gpus[0].util_pct = Some(88.);
        let mut rules = StatusRules::default();
        run(&mut rules, 0, 29, busy_gpu);
        let mut r = calm(30);
        busy_gpu(&mut r);
        let mut exited = record(9, "gone", 0., 99.);
        exited.flags = ProcFlags::EXITED;
        let procs = sweep(
            30 * NS,
            vec![
                record(1, "firefox", 40., 3.),
                record(2, "blender", 20., 80.),
                exited,
            ],
        );
        rules.step(&r, Some(&procs));
        let reason = rules.reason().expect("reason");
        assert_eq!(reason.rule.source, Source::Gpu(GPU));
        assert_eq!(
            reason.process.as_deref(),
            Some("blender"),
            "by GPU, not CPU"
        );

        // No sweep running (the last one is 20 s old): no process.
        let mut r = calm(50);
        busy_gpu(&mut r);
        rules.step(&r, Some(&procs));
        assert_eq!(rules.reason().expect("reason").process, None);
        // No process data at all.
        let mut r = calm(51);
        busy_gpu(&mut r);
        rules.step(&r, None);
        assert_eq!(rules.reason().expect("reason").process, None);
    }

    #[test]
    fn cpu_hot_c_reads_the_engine_sensor() {
        let sensor = |idx: u16, chip: &str, label: &str, hot: Option<f32>| DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Sensor {
                chip: Arc::from(chip),
                label: Arc::from(label),
            },
            generation: 0,
            display_name: Arc::from(label),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: hot,
        };
        let catalog = |entries: Vec<DeviceEntry>| DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        };
        let c = catalog(vec![
            sensor(10, "nvme", "Composite", Some(85.)),
            sensor(11, "coretemp", "Core 0", Some(80.)),
            sensor(12, "coretemp", "Package id 0", Some(100.)),
        ]);
        assert_eq!(cpu_hot_c(&c), Some(100.));
        // k10temp comes before coretemp; k10temp without the files has no threshold.
        let c = catalog(vec![
            sensor(11, "coretemp", "Package id 0", Some(100.)),
            sensor(13, "k10temp", "Tctl", None),
        ]);
        assert_eq!(cpu_hot_c(&c), None);
        assert_eq!(
            cpu_hot_c(&catalog(vec![sensor(1, "nvme", "x", Some(1.))])),
            None
        );
    }
}
