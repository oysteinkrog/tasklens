//! Counter rules: rates, resets, suspend gap and stale threshold (ADR-008, R-051).

use tl_model::{
    series::SampleFlags,
    time::{TimeNs, now_boot_ns, now_mono_ns},
    validity::Validity,
};

/// Suspend check: (Δboot - Δmono) above this means the machine slept (ADR-008, R-051).
pub const SUSPEND_GAP_NS: u64 = 250_000_000;
/// Lower bound of the stale threshold: max(3 × period, 1.5 s) (ADR-008).
pub const STALE_FLOOR_NS: u64 = 1_500_000_000;

/// One reading of both clocks, taken right before a sampler reads its sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub boot_ns: TimeNs,
    pub mono_ns: u64,
}

impl Clock {
    /// Reads CLOCK_BOOTTIME then CLOCK_MONOTONIC. Call immediately before the read syscalls.
    pub fn now() -> Self {
        let boot_ns = now_boot_ns();
        let mono_ns = now_mono_ns();
        Self { boot_ns, mono_ns }
    }
}

/// True when the boottime delta exceeds the monotonic delta by more than 250 ms.
pub fn resumed_between(prev: Clock, cur: Clock) -> bool {
    let d_boot = cur.boot_ns.saturating_sub(prev.boot_ns);
    let d_mono = cur.mono_ns.saturating_sub(prev.mono_ns);
    d_boot.saturating_sub(d_mono) > SUSPEND_GAP_NS
}

/// True when now - last > max(3 × period, 1.5 s). Saturating arithmetic.
pub fn is_stale(now_ns: TimeNs, last_ns: TimeNs, period_ns: u64) -> bool {
    let threshold = period_ns.saturating_mul(3).max(STALE_FLOOR_NS);
    now_ns.saturating_sub(last_ns) > threshold
}

/// A value with its Validity. Missing data is never zero (ADR-003).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured<T> {
    pub value: T,
    pub validity: Validity,
}

impl<T> Measured<T> {
    pub fn valid(value: T) -> Self {
        Self {
            value,
            validity: Validity::Valid,
        }
    }
}

impl<T: Default> Measured<T> {
    /// `value` is `T::default()`; readers must check `validity` first.
    pub fn invalid(validity: Validity) -> Self {
        Self {
            value: T::default(),
            validity,
        }
    }
}

impl<T: Default> Default for Measured<T> {
    fn default() -> Self {
        Self::invalid(Validity::WarmingUp)
    }
}

/// A per-second rate. `per_sec` is 0.0 unless `validity` is `Valid`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub per_sec: f64,
    pub validity: Validity,
    pub flags: SampleFlags,
}

/// The result of one counter step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    Ok { delta: u64, dt_ns: u64 },
    WarmingUp { flags: SampleFlags },
}

impl Delta {
    fn into_rate(self) -> Rate {
        match self {
            Delta::Ok { delta, dt_ns } => Rate {
                per_sec: delta as f64 / (dt_ns as f64 / 1e9),
                validity: Validity::Valid,
                flags: SampleFlags::VALID,
            },
            Delta::WarmingUp { flags } => Rate {
                per_sec: 0.0,
                validity: Validity::WarmingUp,
                flags,
            },
        }
    }
}

/// One cumulative u64 counter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counter {
    /// (value, t_ns, generation) of the last step.
    prev: Option<(u64, TimeNs, u32)>,
}

impl Counter {
    pub fn step(&mut self, cur: u64, t_ns: TimeNs, generation: u32) -> Delta {
        self.step_inner(cur, t_ns, generation, None)
    }

    /// Like `step` but a decrease is a wrap at `max_range` (RAPL `max_energy_range_uj`).
    pub fn step_wrapping(
        &mut self,
        cur: u64,
        t_ns: TimeNs,
        generation: u32,
        max_range: u64,
    ) -> Delta {
        self.step_inner(cur, t_ns, generation, Some(max_range))
    }

    pub fn update(&mut self, cur: u64, t_ns: TimeNs, generation: u32) -> Rate {
        self.step(cur, t_ns, generation).into_rate()
    }

    pub fn update_wrapping(
        &mut self,
        cur: u64,
        t_ns: TimeNs,
        generation: u32,
        max_range: u64,
    ) -> Rate {
        self.step_wrapping(cur, t_ns, generation, max_range)
            .into_rate()
    }

    /// Forget the baseline (resume, ResetBaselines, rate change). Next step is WarmingUp.
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// The ADR-008 rules in order. Every case stores the new baseline.
    fn step_inner(
        &mut self,
        cur: u64,
        t_ns: TimeNs,
        generation: u32,
        max_range: Option<u64>,
    ) -> Delta {
        let prev = self.prev.replace((cur, t_ns, generation));
        let Some((prev_value, prev_t, prev_gen)) = prev else {
            return Delta::WarmingUp {
                flags: SampleFlags::empty(),
            };
        };
        if generation != prev_gen {
            return Delta::WarmingUp {
                flags: SampleFlags::GAP_BEFORE,
            };
        }
        if t_ns <= prev_t {
            return Delta::WarmingUp {
                flags: SampleFlags::empty(),
            };
        }
        let dt_ns = t_ns - prev_t;
        if cur < prev_value {
            return match max_range {
                Some(max) if max > prev_value => Delta::Ok {
                    delta: cur.saturating_add(max - prev_value),
                    dt_ns,
                },
                _ => Delta::WarmingUp {
                    flags: SampleFlags::RESET,
                },
            };
        }
        Delta::Ok {
            delta: cur - prev_value,
            dt_ns,
        }
    }
}

/// 100 × num / den clamped to 0..=100; None when den == 0.
pub fn ratio_pct(num: u64, den: u64) -> Option<f32> {
    if den == 0 {
        return None;
    }
    let pct = num as f64 * 100.0 / den as f64;
    Some(pct.clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SEC: u64 = 1_000_000_000;
    const MS: u64 = 1_000_000;

    #[test]
    fn first_sample_warming_up() {
        let mut c = Counter::default();
        let r = c.update(100, SEC, 0);
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::empty());
        assert_eq!(r.per_sec, 0.0);
    }

    #[test]
    fn rate_uses_elapsed_time() {
        let mut c = Counter::default();
        c.update(100, SEC, 0);
        let r = c.update(1100, 1_500 * MS, 0);
        assert_eq!(r.per_sec, 2000.0);
        assert_eq!(r.validity, Validity::Valid);
        assert_eq!(r.flags, SampleFlags::VALID);
    }

    /// ADR-008 verification: a counter going backwards yields WarmingUp and RESET.
    #[test]
    fn counter_backwards_is_reset() {
        let mut c = Counter::default();
        c.update(1000, SEC, 0);
        let r = c.update(10, 2 * SEC, 0);
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::RESET);
        let r = c.update(20, 3 * SEC, 0);
        assert_eq!(r.per_sec, 10.0);
        assert_eq!(r.validity, Validity::Valid);
    }

    #[test]
    fn generation_change_gaps() {
        let mut c = Counter::default();
        c.update(100, SEC, 0);
        let r = c.update(200, 2 * SEC, 1);
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::GAP_BEFORE);
        let r = c.update(300, 3 * SEC, 1);
        assert_eq!(r.per_sec, 100.0);
    }

    #[test]
    fn zero_elapsed_no_panic() {
        let mut c = Counter::default();
        c.update(100, 2 * SEC, 0);
        let r = c.update(200, 2 * SEC, 0);
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::empty());
        assert!(r.per_sec.is_finite());
        let r = c.update(300, SEC, 0);
        assert_eq!(r.validity, Validity::WarmingUp);
    }

    #[test]
    fn wrapping_delta() {
        let mut c = Counter::default();
        c.step_wrapping(900, SEC, 0, 1000);
        assert_eq!(
            c.step_wrapping(100, 2 * SEC, 0, 1000),
            Delta::Ok {
                delta: 200,
                dt_ns: SEC
            }
        );
        // A range at or below the baseline cannot be a wrap, so it is a reset.
        let mut c = Counter::default();
        c.step_wrapping(900, SEC, 0, 800);
        assert_eq!(
            c.step_wrapping(100, 2 * SEC, 0, 800),
            Delta::WarmingUp {
                flags: SampleFlags::RESET
            }
        );
    }

    #[test]
    fn reset_forgets_baseline() {
        let mut c = Counter::default();
        c.update(100, SEC, 0);
        assert_eq!(c.update(200, 2 * SEC, 0).validity, Validity::Valid);
        c.reset();
        let r = c.update(300, 3 * SEC, 0);
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::empty());
    }

    #[test]
    fn suspend_gap_detection() {
        let zero = Clock {
            boot_ns: 0,
            mono_ns: 0,
        };
        let at = |extra_ms: u64| Clock {
            boot_ns: SEC + extra_ms * MS,
            mono_ns: SEC,
        };
        // ADR-008 verification case.
        assert!(resumed_between(
            zero,
            Clock {
                boot_ns: 5 * SEC,
                mono_ns: 10 * MS
            }
        ));
        assert!(!resumed_between(zero, at(200)));
        assert!(!resumed_between(zero, at(250)));
        assert!(resumed_between(zero, at(251)));
    }

    #[test]
    fn stale_threshold() {
        let last = 10 * SEC;
        assert!(!is_stale(last + 1_400 * MS, last, 100 * MS));
        assert!(is_stale(last + 1_600 * MS, last, 100 * MS));
        assert!(!is_stale(last + 2_900 * MS, last, SEC));
        assert!(is_stale(last + 3_100 * MS, last, SEC));
    }

    #[test]
    fn ratio_pct_cases() {
        assert_eq!(ratio_pct(1, 0), None);
        assert_eq!(ratio_pct(30, 100), Some(30.0));
        assert_eq!(ratio_pct(150, 100), Some(100.0));
    }

    proptest! {
        #[test]
        fn prop_counter_never_panics(
            steps in proptest::collection::vec((any::<u64>(), any::<u64>(), any::<u32>()), 0..=64)
        ) {
            let mut c = Counter::default();
            let mut w = Counter::default();
            for (cur, t, generation) in steps {
                for r in [c.update(cur, t, generation), w.update_wrapping(cur, t, generation, u64::MAX)] {
                    if r.validity == Validity::Valid {
                        prop_assert!(r.per_sec.is_finite());
                        prop_assert!(r.per_sec >= 0.0);
                    }
                }
            }
        }
    }
}
