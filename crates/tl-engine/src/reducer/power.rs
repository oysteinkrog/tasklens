//! Per-process power estimate for the Power usage columns (E09-030, PROC-034, R-163 item 9).
//!
//! Each sweep shares measured power among the processes: RAPL package power by CPU ticks, and
//! each GPU's power by the process's utilisation on that GPU. A missing input gives `None`,
//! never a guess. The trend is the time-weighted mean of the last 2 minutes of estimates.
//! A republish between sweeps writes the values of the last sweep again with `fill`.

use std::collections::VecDeque;

use rustc_hash::FxHashMap;
use tl_model::ids::ProcessKey;
use tl_model::process::{ProcessExtras, ProcessExtrasP2, ProcessRecord};
use tl_model::system::CpuTotals;
use tl_model::time::TimeNs;

/// Window of `power_trend_w`.
pub(crate) const POWER_TREND_WINDOW_NS: u64 = 120_000_000_000; // 2 minutes

#[derive(Default)]
pub(crate) struct PowerEstimator {
    /// `utime_ticks + stime_ticks` of every key at the previous sweep.
    prev: FxHashMap<ProcessKey, u64>,
    /// `(sweep time, power_w)` of every key, oldest first.
    trend: FxHashMap<ProcessKey, VecDeque<(TimeNs, f32)>>,
    /// `(power_w, power_trend_w)` of every key at the last sweep, for `fill`.
    last: FxHashMap<ProcessKey, (Option<f32>, Option<f32>)>,
}

/// Per GPU: measured power and each process's utilisation on it.
pub(crate) struct GpuShare<'a> {
    pub power_w: Option<f32>,
    pub util_by_key: &'a FxHashMap<ProcessKey, f32>,
}

impl PowerEstimator {
    /// One estimate per record. `p2` is index-aligned with `records` and is created with defaults
    /// when empty.
    pub(crate) fn on_sweep(
        &mut self,
        t_ns: TimeNs,
        records: &[ProcessRecord],
        extras: &[ProcessExtras],
        cpu: &CpuTotals,
        gpus: &[GpuShare],
        p2: &mut Vec<ProcessExtrasP2>,
    ) {
        if p2.len() != records.len() {
            p2.resize(records.len(), ProcessExtrasP2::default());
        }
        // Ticks come from `extras`. Without them there is no delta and no estimate.
        let aligned = extras.len() == records.len();

        let mut next = FxHashMap::with_capacity_and_hasher(records.len(), Default::default());
        let mut deltas: Vec<Option<u64>> = Vec::with_capacity(records.len());
        let mut total: u64 = 0;
        for (i, r) in records.iter().enumerate() {
            let delta = if aligned {
                let ticks = extras[i].utime_ticks + extras[i].stime_ticks;
                next.insert(r.key, ticks);
                self.prev.get(&r.key).map(|&p| ticks.saturating_sub(p))
            } else {
                None
            };
            total += delta.unwrap_or(0);
            deltas.push(delta);
        }
        self.prev = next;

        let p_pkg = cpu
            .package_power_w
            .validity
            .is_valid()
            .then_some(cpu.package_power_w.value);
        // A GPU without measured power adds no term; a GPU with power and no use adds 0.
        let gpu_sums: Vec<(f32, f32, &FxHashMap<ProcessKey, f32>)> = gpus
            .iter()
            .filter_map(|g| Some((g.power_w?, g.util_by_key.values().sum(), g.util_by_key)))
            .collect();
        let measured = p_pkg.is_some() || !gpu_sums.is_empty();
        self.last.clear();

        for (i, r) in records.iter().enumerate() {
            let power = match deltas[i] {
                Some(delta) if measured => {
                    let cpu_term = match p_pkg {
                        Some(p) if total > 0 => p * delta as f32 / total as f32,
                        _ => 0.0,
                    };
                    let gpu_term: f32 = gpu_sums
                        .iter()
                        .filter(|(_, sum, _)| *sum > 0.0)
                        .map(|(p, sum, util)| p * util.get(&r.key).copied().unwrap_or(0.0) / sum)
                        .sum();
                    Some(cpu_term + gpu_term)
                }
                _ => None,
            };
            let trend = self.push_trend(r.key, t_ns, power);
            p2[i].power_w = power;
            p2[i].power_trend_w = trend;
            self.last.insert(r.key, (power, trend));
        }

        // `prev` now holds this sweep's keys; without ticks it is empty and so are the trends.
        let live = &self.prev;
        self.trend.retain(|k, _| live.contains_key(k));
    }

    /// Writes the values of the last sweep into `p2`, index-aligned with `records`, for a
    /// republish between sweeps. A key the last sweep did not see gets `None`.
    pub(crate) fn fill(&self, records: &[ProcessRecord], p2: &mut Vec<ProcessExtrasP2>) {
        if p2.len() != records.len() {
            p2.resize(records.len(), ProcessExtrasP2::default());
        }
        for (r, e) in records.iter().zip(p2.iter_mut()) {
            (e.power_w, e.power_trend_w) = self.last.get(&r.key).copied().unwrap_or_default();
        }
    }

    /// On resume and on reset_baselines.
    pub(crate) fn reset(&mut self) {
        self.prev.clear();
        self.trend.clear();
        self.last.clear();
    }

    /// Adds `power` to the key's samples and returns the time-weighted mean over the window.
    /// Each sample holds until the next one. `None` clears the samples, so a gap is never
    /// bridged.
    fn push_trend(&mut self, key: ProcessKey, t_ns: TimeNs, power: Option<f32>) -> Option<f32> {
        let Some(power) = power else {
            self.trend.remove(&key);
            return None;
        };
        let samples = self.trend.entry(key).or_default();
        samples.push_back((t_ns, power));
        let start = t_ns.saturating_sub(POWER_TREND_WINDOW_NS);
        // Drop samples whose hold ends at or before the window start.
        while samples.len() >= 2 && samples[1].0 <= start {
            samples.pop_front();
        }
        if samples.len() < 2 {
            return None;
        }
        let (mut area, mut span) = (0.0f64, 0u64);
        for (a, b) in samples.iter().zip(samples.iter().skip(1)) {
            let from = a.0.max(start);
            let dt = b.0.saturating_sub(from);
            area += f64::from(a.1) * dt as f64;
            span += dt;
        }
        (span > 0).then(|| (area / span as f64) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use tl_model::actions::SchedPolicy;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::system::{CpuSet, Metric};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    const SEC: u64 = 1_000_000_000;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn rec(pid: u32) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: "p".into(),
                name: "p".into(),
                exe: None,
                cmdline: None,
                user: "user".into(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: "p".into(),
            }),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
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
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn extras(ticks: u64) -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: ticks,
            stime_ticks: 0,
            start_boot_ns: 0,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::empty(),
            handles: None,
            exe_name: "x".into(),
            exe_path: "/usr/bin/x".into(),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: 0,
            sid: 0,
            last_cpu: 0,
        }
    }

    fn cpu(package: Metric<f32>) -> CpuTotals {
        CpuTotals {
            package_power_w: package,
            ..CpuTotals::default()
        }
    }

    /// One sweep over `(pid, ticks)` rows. Returns `power_w` and `power_trend_w` per row.
    fn sweep(
        est: &mut PowerEstimator,
        t_ns: TimeNs,
        rows: &[(u32, u64)],
        cpu: &CpuTotals,
        gpus: &[GpuShare],
    ) -> Vec<(Option<f32>, Option<f32>)> {
        let records: Vec<ProcessRecord> = rows.iter().map(|&(pid, _)| rec(pid)).collect();
        let x: Vec<ProcessExtras> = rows.iter().map(|&(_, ticks)| extras(ticks)).collect();
        let mut p2 = Vec::new();
        est.on_sweep(t_ns, &records, &x, cpu, gpus, &mut p2);
        assert_eq!(p2.len(), records.len());
        assert!(p2.iter().all(|e| e.key.is_none()));
        p2.iter().map(|e| (e.power_w, e.power_trend_w)).collect()
    }

    fn power(out: &[(Option<f32>, Option<f32>)]) -> Vec<Option<f32>> {
        out.iter().map(|o| o.0).collect()
    }

    #[test]
    fn cpu_share_by_ticks() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        sweep(&mut est, SEC, &[(1, 100), (2, 50)], &cpu, &[]);
        let out = sweep(&mut est, 2 * SEC, &[(1, 130), (2, 60)], &cpu, &[]);
        assert_eq!(power(&out), [Some(15.0), Some(5.0)]);
    }

    #[test]
    fn gpu_share_by_util() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        let util: FxHashMap<ProcessKey, f32> =
            [(key(1), 60.0), (key(2), 20.0)].into_iter().collect();
        let gpus = [GpuShare {
            power_w: Some(100.0),
            util_by_key: &util,
        }];
        sweep(&mut est, SEC, &[(1, 10), (2, 10)], &cpu, &gpus);
        let out = sweep(&mut est, 2 * SEC, &[(1, 10), (2, 10)], &cpu, &gpus);
        assert_eq!(power(&out), [Some(75.0), Some(25.0)]);
    }

    #[test]
    fn restricted_rapl_no_estimate() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::with(Validity::Restricted));
        let util: FxHashMap<ProcessKey, f32> = [(key(1), 50.0)].into_iter().collect();
        let gpus = [GpuShare {
            power_w: None,
            util_by_key: &util,
        }];
        sweep(&mut est, SEC, &[(1, 10), (2, 10)], &cpu, &gpus);
        let out = sweep(&mut est, 2 * SEC, &[(1, 30), (2, 20)], &cpu, &gpus);
        assert_eq!(out, [(None, None), (None, None)]);
    }

    #[test]
    fn first_sight_none() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        sweep(&mut est, SEC, &[(1, 10)], &cpu, &[]);
        let out = sweep(&mut est, 2 * SEC, &[(1, 20), (2, 500)], &cpu, &[]);
        assert_eq!(power(&out), [Some(20.0), None]);
    }

    #[test]
    fn trend_two_minute_mean() {
        let mut est = PowerEstimator::default();
        let base = 1_000 * SEC;
        // One process takes every tick, so its estimate is the package power.
        sweep(
            &mut est,
            base - SEC,
            &[(1, 0)],
            &cpu(Metric::valid(10.0)),
            &[],
        );
        let mut last = (None, None);
        for s in 0..=120u64 {
            let watts = if s < 60 { 10.0 } else { 20.0 };
            let out = sweep(
                &mut est,
                base + s * SEC,
                &[(1, (s + 1) * 10)],
                &cpu(Metric::valid(watts)),
                &[],
            );
            if s == 0 {
                assert_eq!(out[0], (Some(10.0), None));
            }
            last = out[0];
        }
        let trend = last.1.expect("trend");
        assert!((trend - 15.0).abs() < 0.01, "trend {trend}");
        assert!(est.trend[&key(1)].len() <= 122);
    }

    #[test]
    fn reset_clears_baselines() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        sweep(&mut est, SEC, &[(1, 10), (2, 10)], &cpu, &[]);
        sweep(&mut est, 2 * SEC, &[(1, 20), (2, 20)], &cpu, &[]);
        est.reset();
        let out = sweep(&mut est, 3 * SEC, &[(1, 30), (2, 30)], &cpu, &[]);
        assert_eq!(out, [(None, None), (None, None)]);
    }

    #[test]
    fn fill_repeats_last_sweep() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        sweep(&mut est, SEC, &[(1, 10), (2, 10)], &cpu, &[]);
        sweep(&mut est, 2 * SEC, &[(1, 40), (2, 20)], &cpu, &[]);
        let records = [rec(1), rec(3), rec(2)];
        let mut p2 = Vec::new();
        est.fill(&records, &mut p2);
        let power: Vec<Option<f32>> = p2.iter().map(|e| e.power_w).collect();
        assert_eq!(power, [Some(15.0), None, Some(5.0)]);
        est.reset();
        est.fill(&records, &mut p2);
        assert!(p2.iter().all(|e| e.power_w.is_none()));
    }

    #[test]
    fn dead_keys_dropped() {
        let mut est = PowerEstimator::default();
        let cpu = cpu(Metric::valid(20.0));
        sweep(&mut est, SEC, &[(1, 10), (2, 10)], &cpu, &[]);
        sweep(&mut est, 2 * SEC, &[(1, 20), (2, 20)], &cpu, &[]);
        sweep(&mut est, 3 * SEC, &[(1, 30)], &cpu, &[]);
        assert!(!est.prev.contains_key(&key(2)));
        assert!(!est.trend.contains_key(&key(2)));
    }
}
