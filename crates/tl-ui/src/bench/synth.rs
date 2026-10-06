//! Deterministic synthetic sources for the phase-0 bench scenes (ADR-014, UI-PRF-053).
//!
//! Everything starts from [`SEED`], so two runs produce the same data. The PRNG is SplitMix64
//! plus Box-Muller, written here with no PRNG crate (R-142 item 8). The value rules come from
//! UI-PRF-053 and R-160 OI-2.

use std::f64::consts::PI;

use tl_model::series::Sample;
use tl_model::time::TimeNs;

/// PRNG seed for every bench scene.
pub const SEED: u64 = 42;

/// Logical CPUs in the synthetic `perf-cpu` source.
pub const CORES: usize = 32;

/// Sidebar cards in order. The CPU card uses the 10 Hz total from [`CpuSynth`].
pub const DEVICE_CARDS: [&str; 9] = [
    "CPU",
    "Memory",
    "GPU 0",
    "Disk 0",
    "Disk 1",
    "Disk 2",
    "Disk 3",
    "enp38s0",
    "tailscale0",
];

/// Samples per 10 Hz series in the prefill (60 s range).
pub const PREFILL_10HZ: usize = 600;
/// Samples per 1 Hz series in the prefill (60 s range).
pub const PREFILL_1HZ: usize = 60;

const NS_PER_S: u64 = 1_000_000_000;
const PERIOD_10HZ_NS: u64 = 100_000_000;
const PERIOD_1HZ_NS: u64 = NS_PER_S;

/// Burst probability per second for disks and networks (R-160 OI-2).
const BURST_P: f32 = 0.1;
/// Network value outside a burst, in Mbps.
const NET_IDLE_MBPS: f32 = 0.01;

/// Up time at the first stats sample: 2 days, 2h 38m 15s (the UI-PRF-024 example).
const UPTIME_START_S: u64 = 2 * 86_400 + 2 * 3_600 + 38 * 60 + 15;

/// Deterministic PRNG: SplitMix64 for integers, Box-Muller for Gaussian noise.
#[derive(Clone, Debug)]
pub struct BenchRng {
    state: u64,
}

impl BenchRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// SplitMix64.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1), from the top 24 bits.
    pub fn next_f32_unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Uniform in [lo, hi). Rounding never returns `hi`.
    pub fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        let v = lo + (hi - lo) * self.next_f32_unit();
        if v < hi { v } else { hi.next_down() }
    }

    /// Normal with mean 0 and standard deviation `sigma` (Box-Muller, one value per call).
    pub fn gaussian(&mut self, sigma: f32) -> f32 {
        // u1 in (0, 1] keeps ln away from 0.
        let u1 = 1.0 - (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let u2 = (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
        (z * sigma as f64) as f32
    }

    /// True with probability `p`.
    fn chance(&mut self, p: f32) -> bool {
        self.next_f32_unit() < p
    }
}

/// One 10 Hz CPU sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuFrame {
    pub per_core: [f32; CORES],
    pub per_core_kernel: [f32; CORES],
    pub total: f32,
    pub total_kernel: f32,
}

/// 32 cores at 10 Hz: `clamp(base_c + 25 sin(2 pi t / P_c) + N(0, 4), 0, 100)` (UI-PRF-053).
#[derive(Clone, Debug)]
pub struct CpuSynth {
    base: [f32; CORES],
    period_s: [f32; CORES],
    /// Noise source, seeded from the caller's rng after the per-core draws.
    rng: BenchRng,
}

impl CpuSynth {
    /// Draws `base_c` and then `P_c` for each core, in core order.
    pub fn new(rng: &mut BenchRng) -> Self {
        let mut base = [0.0; CORES];
        let mut period_s = [0.0; CORES];
        for (b, p) in base.iter_mut().zip(&mut period_s) {
            *b = rng.uniform(5.0, 60.0);
            *p = rng.uniform(3.0, 17.0);
        }
        Self {
            base,
            period_s,
            rng: BenchRng::new(rng.next_u64()),
        }
    }

    /// Per-core base load in percent, in [5, 60).
    pub fn bases(&self) -> &[f32; CORES] {
        &self.base
    }

    /// Per-core sine period in seconds, in [3, 17).
    pub fn periods_s(&self) -> &[f32; CORES] {
        &self.period_s
    }

    pub fn sample(&mut self, t_s: f64) -> CpuFrame {
        let mut per_core = [0.0f32; CORES];
        let mut per_core_kernel = [0.0f32; CORES];
        let cores = self.base.iter().zip(&self.period_s);
        for ((v, k), (&base, &period_s)) in per_core.iter_mut().zip(&mut per_core_kernel).zip(cores)
        {
            let wave = 25.0 * (2.0 * PI * t_s / period_s as f64).sin();
            *v = (base + wave as f32 + self.rng.gaussian(4.0)).clamp(0.0, 100.0);
            *k = (0.2 * *v + self.rng.gaussian(1.0)).clamp(0.0, 100.0);
        }
        CpuFrame {
            per_core,
            per_core_kernel,
            total: mean(&per_core),
            total_kernel: mean(&per_core_kernel),
        }
    }
}

fn mean(values: &[f32; CORES]) -> f32 {
    values.iter().sum::<f32>() / CORES as f32
}

/// One 1 Hz sample for the sidebar cards after CPU, in [`DEVICE_CARDS`] order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceFrame {
    pub memory_pct: f32,
    pub gpu_pct: f32,
    /// Disk 0 to Disk 3, percent busy.
    pub disk_pct: [f32; 4],
    /// enp38s0 and tailscale0, Mbps.
    pub net_mbps: [f32; 2],
}

impl DeviceFrame {
    /// The 8 values in card order (Memory to tailscale0).
    pub fn values(&self) -> [f32; 8] {
        let [d0, d1, d2, d3] = self.disk_pct;
        let [n0, n1] = self.net_mbps;
        [self.memory_pct, self.gpu_pct, d0, d1, d2, d3, n0, n1]
    }
}

/// Memory, GPU, disk and network cards at 1 Hz (UI-PRF-053, R-160 OI-2).
#[derive(Clone, Debug)]
pub struct DeviceSynth {
    memory_pct: f32,
}

impl Default for DeviceSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceSynth {
    pub fn new() -> Self {
        Self { memory_pct: 55.0 }
    }

    pub fn sample_1hz(&mut self, rng: &mut BenchRng) -> DeviceFrame {
        // Memory: random walk, uniform step in [-1, 1], clamped to [0, 100].
        self.memory_pct = (self.memory_pct + rng.uniform(-1.0, 1.0)).clamp(0.0, 100.0);
        // GPU: 10% +- 8, a fresh draw each second.
        let gpu_pct = rng.uniform(2.0, 18.0);
        let mut disk_pct = [0.0; 4];
        for d in &mut disk_pct {
            if rng.chance(BURST_P) {
                // (0, 100], so a burst is never 0.
                *d = 100.0 * (1.0 - rng.next_f32_unit());
            }
        }
        let mut net_mbps = [NET_IDLE_MBPS; 2];
        for n in &mut net_mbps {
            if rng.chance(BURST_P) {
                *n = rng.uniform(0.01, 5.0);
            }
        }
        DeviceFrame {
            memory_pct: self.memory_pct,
            gpu_pct,
            disk_pct,
            net_mbps,
        }
    }
}

/// Details rows 1 to 12 of UI-PRF-024.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsFrame {
    /// Row 1, from [`CpuFrame::total`].
    pub utilization_pct: f32,
    /// Row 2, from [`CpuFrame::total_kernel`].
    pub kernel_pct: f32,
    /// Row 3.
    pub speed_mhz: u32,
    /// Row 4.
    pub processes: u32,
    /// Row 5.
    pub threads: u32,
    /// Row 6.
    pub handles: u32,
    /// Row 7.
    pub up_time_s: u64,
    /// Row 8, static.
    pub base_speed_mhz: u32,
    /// Row 9, static.
    pub sockets: u32,
    /// Row 10 is a P2 row, hidden in P0.
    pub power_draw_w: Option<f32>,
    /// Row 11.
    pub temperature_c: f32,
    /// Row 12: 1, 5 and 15 minutes.
    pub load_avg: [f32; 3],
}

/// The live Details values at 1 Hz (UI-PRF-053, R-160 OI-2).
#[derive(Clone, Debug)]
pub struct StatsSynth {
    start_t_s: f64,
    load_avg: [f32; 3],
}

impl StatsSynth {
    /// `start_t_s` is the scene time at which Up time reads [`UPTIME_START_S`].
    pub fn new(start_t_s: f64) -> Self {
        Self {
            start_t_s,
            load_avg: [1.24, 0.98, 0.87],
        }
    }

    pub fn sample_1hz(&mut self, rng: &mut BenchRng, t_s: f64, cpu: &CpuFrame) -> StatsFrame {
        let speed_mhz = jitter_count(rng, 4631, 200);
        let processes = jitter_count(rng, 295, 3);
        let threads = jitter_count(rng, 5_596, 20);
        let handles = jitter_count(rng, 253_248, 500);
        let temperature_c = rng.uniform(50.0, 54.0);
        for l in &mut self.load_avg {
            *l = (*l + rng.uniform(-0.05, 0.05)).max(0.0);
        }
        let elapsed_s = (t_s - self.start_t_s).max(0.0) as u64;
        StatsFrame {
            utilization_pct: cpu.total,
            kernel_pct: cpu.total_kernel,
            speed_mhz,
            processes,
            threads,
            handles,
            up_time_s: UPTIME_START_S + elapsed_s,
            base_speed_mhz: 4200,
            sockets: 1,
            power_draw_w: None,
            temperature_c,
            load_avg: self.load_avg,
        }
    }
}

/// A fresh uniform draw in [x - n, x + n], rounded.
fn jitter_count(rng: &mut BenchRng, x: u32, n: u32) -> u32 {
    rng.uniform((x - n) as f32, (x + n) as f32).round() as u32
}

/// 60 s of history for every chart, ending at the scene start.
#[derive(Clone, Debug)]
pub struct PrefilledSeries {
    /// 32 series of [`PREFILL_10HZ`] samples.
    pub per_core: Vec<Vec<Sample>>,
    /// 32 series of [`PREFILL_10HZ`] samples.
    pub per_core_kernel: Vec<Vec<Sample>>,
    /// [`PREFILL_10HZ`] samples. Also feeds the CPU sidebar card.
    pub total: Vec<Sample>,
    /// [`PREFILL_10HZ`] samples.
    pub total_kernel: Vec<Sample>,
    /// 8 series of [`PREFILL_1HZ`] samples, Memory to tailscale0 in [`DEVICE_CARDS`] order.
    pub devices: Vec<Vec<Sample>>,
}

/// Runs the generators over the 60 s before `t0` so decimation and the geometry rebuild work
/// from frame 1 (UI-PRF-053). The last sample of every series has timestamp `t0`. `t0` must be at
/// least 60 s.
pub fn prefill(
    cpu: &mut CpuSynth,
    devices: &mut DeviceSynth,
    rng: &mut BenchRng,
    t0: TimeNs,
) -> PrefilledSeries {
    let fast = || Vec::with_capacity(PREFILL_10HZ);
    let mut out = PrefilledSeries {
        per_core: (0..CORES).map(|_| fast()).collect(),
        per_core_kernel: (0..CORES).map(|_| fast()).collect(),
        total: fast(),
        total_kernel: fast(),
        devices: (0..8).map(|_| Vec::with_capacity(PREFILL_1HZ)).collect(),
    };
    for i in 0..PREFILL_10HZ {
        let t_ns = t0 - (PREFILL_10HZ - 1 - i) as u64 * PERIOD_10HZ_NS;
        let f = cpu.sample(seconds(t_ns));
        for (series, &v) in out.per_core.iter_mut().zip(&f.per_core) {
            series.push(Sample::valid(t_ns, v));
        }
        for (series, &v) in out.per_core_kernel.iter_mut().zip(&f.per_core_kernel) {
            series.push(Sample::valid(t_ns, v));
        }
        out.total.push(Sample::valid(t_ns, f.total));
        out.total_kernel.push(Sample::valid(t_ns, f.total_kernel));
    }
    for i in 0..PREFILL_1HZ {
        let t_ns = t0 - (PREFILL_1HZ - 1 - i) as u64 * PERIOD_1HZ_NS;
        let f = devices.sample_1hz(rng);
        for (series, value) in out.devices.iter_mut().zip(f.values()) {
            series.push(Sample::valid(t_ns, value));
        }
    }
    out
}

/// Scene time in seconds for a timestamp; the generators take `t_s` from this.
pub fn seconds(t_ns: TimeNs) -> f64 {
    t_ns as f64 / NS_PER_S as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT_10HZ: f64 = 0.1;

    #[test]
    fn synth_is_deterministic() {
        let mut a = BenchRng::new(SEED);
        let mut b = BenchRng::new(SEED);
        for _ in 0..1_000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }

        let mut ca = CpuSynth::new(&mut BenchRng::new(SEED));
        let mut cb = CpuSynth::new(&mut BenchRng::new(SEED));
        for i in 0..1_000 {
            let t = i as f64 * DT_10HZ;
            let (fa, fb) = (ca.sample(t), cb.sample(t));
            let bits = |f: &CpuFrame| -> Vec<u32> {
                f.per_core
                    .iter()
                    .chain(&f.per_core_kernel)
                    .map(|v| v.to_bits())
                    .collect()
            };
            assert_eq!(bits(&fa), bits(&fb), "sample {i}");
            assert_eq!(fa.total.to_bits(), fb.total.to_bits());
            assert_eq!(fa.total_kernel.to_bits(), fb.total_kernel.to_bits());
        }
    }

    #[test]
    fn cpu_values_in_range() {
        let mut cpu = CpuSynth::new(&mut BenchRng::new(SEED));
        assert!(cpu.bases().iter().all(|b| (5.0..60.0).contains(b)));
        assert!(cpu.periods_s().iter().all(|p| (3.0..17.0).contains(p)));
        for i in 0..1_000 {
            let f = cpu.sample(i as f64 * DT_10HZ);
            let all = f.per_core.iter().chain(&f.per_core_kernel);
            assert!(all.into_iter().all(|v| (0.0..=100.0).contains(v)));
            let mean = f.per_core.iter().map(|&v| v as f64).sum::<f64>() / CORES as f64;
            assert!((f.total as f64 - mean).abs() <= 1e-4, "sample {i}");
        }
    }

    #[test]
    fn prefill_lengths_and_spacing() {
        let mut rng = BenchRng::new(SEED);
        let mut cpu = CpuSynth::new(&mut rng);
        let mut dev = DeviceSynth::new();
        let t0: TimeNs = 3_600 * NS_PER_S + 123;
        let p = prefill(&mut cpu, &mut dev, &mut rng, t0);

        let check = |s: &[Sample], len: usize, step: u64| {
            assert_eq!(s.len(), len);
            assert_eq!(s.last().map(|x| x.t_ns), Some(t0));
            for w in s.windows(2) {
                assert_eq!(w[1].t_ns - w[0].t_ns, step);
            }
            assert!(s.iter().all(Sample::is_valid));
        };
        assert_eq!(p.per_core.len(), CORES);
        assert_eq!(p.per_core_kernel.len(), CORES);
        for s in p.per_core.iter().chain(&p.per_core_kernel) {
            check(s, PREFILL_10HZ, PERIOD_10HZ_NS);
        }
        check(&p.total, PREFILL_10HZ, PERIOD_10HZ_NS);
        check(&p.total_kernel, PREFILL_10HZ, PERIOD_10HZ_NS);
        assert_eq!(p.devices.len(), DEVICE_CARDS.len() - 1);
        for s in &p.devices {
            check(s, PREFILL_1HZ, PERIOD_1HZ_NS);
        }
    }

    #[test]
    fn stats_ranges() {
        let mut rng = BenchRng::new(SEED);
        let mut cpu = CpuSynth::new(&mut rng);
        let mut stats = StatsSynth::new(0.0);
        for i in 0..100 {
            let t = i as f64;
            let f = cpu.sample(t);
            let s = stats.sample_1hz(&mut rng, t, &f);
            assert!(
                (292..=298).contains(&s.processes),
                "processes {}",
                s.processes
            );
            assert!(
                (5_576..=5_616).contains(&s.threads),
                "threads {}",
                s.threads
            );
            assert!(
                (252_748..=253_748).contains(&s.handles),
                "handles {}",
                s.handles
            );
            assert!(
                (4_431..=4_831).contains(&s.speed_mhz),
                "speed {}",
                s.speed_mhz
            );
            assert!(
                (50.0..=54.0).contains(&s.temperature_c),
                "temp {}",
                s.temperature_c
            );
            assert!(s.load_avg.iter().all(|&l| l >= 0.0));
            assert_eq!(s.up_time_s, UPTIME_START_S + i);
            assert_eq!(s.utilization_pct, f.total);
            assert_eq!(s.kernel_pct, f.total_kernel);
        }
    }

    #[test]
    fn disk_burst_probability() {
        let mut rng = BenchRng::new(SEED);
        let mut dev = DeviceSynth::new();
        let n = 10_000;
        let mut nonzero = [0u32; 4];
        for _ in 0..n {
            let f = dev.sample_1hz(&mut rng);
            for (count, &v) in nonzero.iter_mut().zip(&f.disk_pct) {
                assert!((0.0..=100.0).contains(&v));
                if v != 0.0 {
                    *count += 1;
                }
            }
            assert!((0.0..=100.0).contains(&f.memory_pct));
            assert!((2.0..18.0).contains(&f.gpu_pct));
        }
        for (d, &count) in nonzero.iter().enumerate() {
            let frac = count as f64 / n as f64;
            assert!((frac - 0.1).abs() <= 0.02, "disk {d}: {frac}");
        }
    }
}
