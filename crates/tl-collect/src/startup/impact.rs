//! Startup impact classification from systemd activation timing.
//!
//! Uses the same timestamps as `systemd-analyze --user blame`. The engine reads the unit
//! properties over D-Bus; this module only does the computation.

use std::sync::Arc;

/// Startup impact of one entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImpactRaw {
    High,
    Medium,
    Low,
    None,
    NotMeasured,
}

/// Timing details behind an impact value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ImpactDetailRaw {
    /// ActiveEnter minus InactiveExit, in milliseconds.
    pub activation_ms: u64,
    /// InactiveExit minus the manager's UserspaceTimestampMonotonic, in milliseconds.
    pub start_offset_ms: u64,
    /// CPUUsageNSec. `None` when the bus reports `u64::MAX` (not available).
    pub cpu_ns: Option<u64>,
    /// True when the service Type= is simple or exec, or unset (which means simple).
    pub fork_exec_only: bool,
}

/// Unit properties read from the systemd user manager.
pub struct UnitTiming {
    /// InactiveExitTimestampMonotonic, in microseconds.
    pub inactive_exit_mono_us: u64,
    /// ActiveEnterTimestampMonotonic, in microseconds.
    pub active_enter_mono_us: u64,
    /// Manager UserspaceTimestampMonotonic of the user manager, in microseconds.
    pub userspace_mono_us: u64,
    /// CPUUsageNSec. `u64::MAX` means not available.
    pub cpu_usage_ns: u64,
    /// Service Type= value, `None` when unset.
    pub service_type: Option<Arc<str>>,
}

/// Activation time at or above this is High.
const HIGH_MS: u64 = 1000;
/// Activation time at or above this, and below [`HIGH_MS`], is Medium.
const MEDIUM_MS: u64 = 300;

/// Classifies the startup impact of an entry.
///
/// Disabled entries have no impact. Enabled entries without usable timestamps are NotMeasured.
pub fn classify(
    enabled: bool,
    timing: Option<&UnitTiming>,
) -> (ImpactRaw, Option<ImpactDetailRaw>) {
    if !enabled {
        return (ImpactRaw::None, None);
    }
    let Some(t) = timing else {
        return (ImpactRaw::NotMeasured, None);
    };
    if t.inactive_exit_mono_us == 0
        || t.active_enter_mono_us == 0
        || t.active_enter_mono_us < t.inactive_exit_mono_us
    {
        return (ImpactRaw::NotMeasured, None);
    }

    let activation_ms = (t.active_enter_mono_us - t.inactive_exit_mono_us) / 1000;
    let impact = if activation_ms >= HIGH_MS {
        ImpactRaw::High
    } else if activation_ms >= MEDIUM_MS {
        ImpactRaw::Medium
    } else {
        ImpactRaw::Low
    };
    let detail = ImpactDetailRaw {
        activation_ms,
        start_offset_ms: t.inactive_exit_mono_us.saturating_sub(t.userspace_mono_us) / 1000,
        cpu_ns: (t.cpu_usage_ns != u64::MAX).then_some(t.cpu_usage_ns),
        fork_exec_only: matches!(
            t.service_type.as_deref(),
            None | Some("simple") | Some("exec")
        ),
    };
    (impact, Some(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(activation_ms: u64) -> UnitTiming {
        UnitTiming {
            inactive_exit_mono_us: 8_200_000,
            active_enter_mono_us: 8_200_000 + activation_ms * 1000,
            userspace_mono_us: 5_000_000,
            cpu_usage_ns: 12_345,
            service_type: None,
        }
    }

    #[test]
    fn impact_thresholds_1000_300() {
        for (ms, want) in [
            (999, ImpactRaw::Medium),
            (1000, ImpactRaw::High),
            (299, ImpactRaw::Low),
            (300, ImpactRaw::Medium),
        ] {
            let (impact, detail) = classify(true, Some(&timing(ms)));
            assert_eq!(impact, want, "activation {ms} ms");
            assert_eq!(detail.unwrap().activation_ms, ms);
        }
    }

    #[test]
    fn disabled_is_none() {
        assert_eq!(
            classify(false, Some(&timing(2000))),
            (ImpactRaw::None, None)
        );
        assert_eq!(classify(false, None), (ImpactRaw::None, None));
    }

    #[test]
    fn missing_timestamp_not_measured() {
        let mut t = timing(500);
        t.active_enter_mono_us = 0;
        assert_eq!(classify(true, Some(&t)), (ImpactRaw::NotMeasured, None));

        let mut t = timing(500);
        t.inactive_exit_mono_us = 0;
        assert_eq!(classify(true, Some(&t)), (ImpactRaw::NotMeasured, None));

        let mut t = timing(500);
        t.active_enter_mono_us = t.inactive_exit_mono_us - 1;
        assert_eq!(classify(true, Some(&t)), (ImpactRaw::NotMeasured, None));

        assert_eq!(classify(true, None), (ImpactRaw::NotMeasured, None));
    }

    #[test]
    fn start_offset() {
        let (_, detail) = classify(true, Some(&timing(100)));
        assert_eq!(detail.unwrap().start_offset_ms, 3200);

        let mut t = timing(100);
        t.userspace_mono_us = 9_000_000;
        let (_, detail) = classify(true, Some(&t));
        assert_eq!(detail.unwrap().start_offset_ms, 0);
    }

    #[test]
    fn cpu_not_available() {
        let mut t = timing(100);
        t.cpu_usage_ns = u64::MAX;
        let (_, detail) = classify(true, Some(&t));
        assert_eq!(detail.unwrap().cpu_ns, None);

        let (_, detail) = classify(true, Some(&timing(100)));
        assert_eq!(detail.unwrap().cpu_ns, Some(12_345));
    }

    #[test]
    fn type_oneshot_not_fork_exec() {
        let mut t = timing(100);
        t.service_type = Some(Arc::from("oneshot"));
        let (_, detail) = classify(true, Some(&t));
        assert!(!detail.unwrap().fork_exec_only);

        for ty in [None, Some("simple"), Some("exec")] {
            let mut t = timing(100);
            t.service_type = ty.map(Arc::from);
            let (_, detail) = classify(true, Some(&t));
            assert!(detail.unwrap().fork_exec_only, "type {ty:?}");
        }
    }
}
