//! Source health and stale marking (ADR-004, ADR-008, ADR-015).
//!
//! Workers send `Running`, `Late`, `Failed`, `Disabled` and `Unsupported`. The reducer adds
//! `Stale` on `Tick` from the time of the last data update. The UI maps `SourceId` to display
//! names; the engine sends only the id.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::command::SetupState;
use tl_model::health::{
    EngineCaps, HealthSnapshot, HelperStartOutcome, HelperState, SourceHealth, SourceId,
    SourceState,
};
use tl_model::time::TimeNs;

use crate::supervise::RestartCounts;

/// A source is stale after max(3 × period, 1.5 s) without data (ADR-008).
const STALE_MIN_NS: u64 = 1_500_000_000;
const STALE_PERIODS: u64 = 3;

/// `last_error` is shown in a tooltip: one line, at most 200 characters (ADR-015).
const MAX_ERROR_CHARS: usize = 200;

pub struct HealthState {
    by_id: FxHashMap<SourceId, Entry>,
    dirty: bool,
    /// The last published snapshot. `publish` copies the helper, caps and other fields it does
    /// not own from here.
    current: Arc<HealthSnapshot>,
    /// Filled by the source controllers. Read only in `publish`, so a count change alone never
    /// publishes; a restart always changes the state as well (R-289).
    restarts: RestartCounts,
}

struct Entry {
    state: SourceState,
    last_error: Option<Arc<str>>,
    last_ok_ns: Option<TimeNs>,
    period_ns: Option<u64>,
    stale: bool,
    /// When `state` last changed. A source that just came back is not stale until a full
    /// threshold has passed since then, even if its last data is old.
    since_ns: TimeNs,
}

impl Entry {
    fn new() -> Self {
        Self {
            state: SourceState::Off,
            last_error: None,
            last_ok_ns: None,
            period_ns: None,
            stale: false,
            since_ns: 0,
        }
    }

    fn is_stale_at(&self, now_ns: TimeNs) -> bool {
        if self.state != SourceState::Running {
            return false;
        }
        let (Some(last_ok), Some(period)) = (self.last_ok_ns, self.period_ns) else {
            return false;
        };
        let threshold = period.saturating_mul(STALE_PERIODS).max(STALE_MIN_NS);
        let quiet_since = last_ok.max(self.since_ns);
        now_ns.saturating_sub(quiet_since) > threshold
    }

    fn published(&self, restarts: u32) -> SourceHealth {
        let state = if self.stale && self.state == SourceState::Running {
            SourceState::Stale
        } else {
            self.state
        };
        SourceHealth {
            state,
            last_ok_ns: self.last_ok_ns,
            last_error: self.last_error.clone(),
            restarts,
        }
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            by_id: FxHashMap::default(),
            dirty: false,
            current: Arc::new(empty_snapshot()),
            restarts: RestartCounts::default(),
        }
    }

    /// The counts the session's source controllers write (`SourceController::with_restart_counts`).
    pub fn restart_counts(&self) -> RestartCounts {
        self.restarts.clone()
    }

    /// Stores a state update from a worker. A state change clears the stale mark.
    pub fn apply(
        &mut self,
        id: SourceId,
        state: SourceState,
        last_error: Option<Arc<str>>,
        t_ns: TimeNs,
    ) {
        let last_error = last_error.map(one_line);
        let entry = self.by_id.entry(id).or_insert_with(Entry::new);
        if entry.state != state {
            entry.state = state;
            entry.since_ns = t_ns;
            entry.stale = false;
            self.dirty = true;
        }
        if entry.last_error != last_error {
            entry.last_error = last_error;
            self.dirty = true;
        }
    }

    /// Records fresh data from a source. Called by apply.rs for every data update. Returns true
    /// when it cleared a stale mark, so the caller marks `HEALTH` for the next publish.
    pub fn sample_ok(&mut self, id: SourceId, t_ns: TimeNs, period_ns: u64) -> bool {
        let entry = self.by_id.entry(id).or_insert_with(Entry::new);
        entry.last_ok_ns = Some(t_ns);
        entry.period_ns = Some(period_ns);
        if !entry.stale {
            return false;
        }
        entry.stale = false;
        self.dirty = true;
        true
    }

    /// Marks running sources stale or fresh. Returns true when any mark changed.
    pub fn tick(&mut self, now_ns: TimeNs) -> bool {
        let mut changed = false;
        for entry in self.by_id.values_mut() {
            let stale = entry.is_stale_at(now_ns);
            if stale != entry.stale {
                entry.stale = stale;
                changed = true;
            }
        }
        self.dirty |= changed;
        changed
    }

    /// Stores the helper state and, when `start` is `Some`, the start outcome. Returns true when
    /// either changed (E29-020).
    pub fn set_helper(
        &mut self,
        state: HelperState,
        start: Option<(u64, HelperStartOutcome)>,
    ) -> bool {
        let start = start.or_else(|| self.current.helper_start.clone());
        if state == self.current.helper && start == self.current.helper_start {
            return false;
        }
        let next = Arc::make_mut(&mut self.current);
        next.helper = state;
        next.helper_start = start;
        self.dirty = true;
        true
    }

    /// Stores the Additional Setup state. Returns true when it changed (E29-020).
    pub fn set_setup(&mut self, setup: Arc<SetupState>) -> bool {
        if Arc::ptr_eq(&setup, &self.current.setup) || *setup == *self.current.setup {
            return false;
        }
        Arc::make_mut(&mut self.current).setup = setup;
        self.dirty = true;
        true
    }

    /// True when the source is `Failed` or `Disabled`.
    pub fn is_down(&self, id: SourceId) -> bool {
        self.by_id
            .get(&id)
            .is_some_and(|e| matches!(e.state, SourceState::Failed | SourceState::Disabled { .. }))
    }

    /// A new snapshot when anything changed since the last call, else `None`.
    pub fn publish(&mut self) -> Option<Arc<HealthSnapshot>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let sources = std::array::from_fn(|i| {
            let id = SourceId::ALL[i];
            let restarts = self.restarts.get(id);
            self.by_id.get(&id).map_or_else(
                || SourceHealth {
                    restarts,
                    ..SourceHealth::default()
                },
                |entry| entry.published(restarts),
            )
        });
        let next = Arc::new(HealthSnapshot {
            version: self.current.version + 1,
            sources,
            ..(*self.current).clone()
        });
        self.current = next.clone();
        Some(next)
    }

    /// Every source `Failed` with `reason`. The session publishes this after more than 3
    /// restarts in 60 s (ADR-004). `version` is `u64::MAX` because no later health snapshot
    /// follows. The snapshot has no epoch field, so `_epoch` is not stored.
    pub fn all_failed(_epoch: u32, reason: &str) -> Arc<HealthSnapshot> {
        let reason: Arc<str> = one_line(Arc::from(reason));
        Arc::new(HealthSnapshot {
            version: u64::MAX,
            sources: std::array::from_fn(|_| SourceHealth {
                state: SourceState::Failed,
                last_ok_ns: None,
                last_error: Some(reason.clone()),
                restarts: 0,
            }),
            ..empty_snapshot()
        })
    }
}

fn empty_snapshot() -> HealthSnapshot {
    HealthSnapshot {
        version: 0,
        sources: std::array::from_fn(|_| SourceHealth::default()),
        helper: HelperState::default(),
        helper_start: None,
        caps: EngineCaps::empty(),
        setup: Arc::default(),
    }
}

/// Keeps the first line, drops control characters and cuts to 200 characters. Returns the
/// input unchanged when it already fits.
fn one_line(text: Arc<str>) -> Arc<str> {
    let first = text.lines().next().unwrap_or("");
    let fits = first.len() == text.len()
        && first.chars().count() <= MAX_ERROR_CHARS
        && !first.chars().any(char::is_control);
    if fits {
        return text;
    }
    first
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ERROR_CHARS)
        .collect::<String>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;
    const T: TimeNs = 100 * S;

    #[test]
    fn stale_after_threshold_10hz() {
        let mut h = HealthState::new();
        h.apply(SourceId::Cpu, SourceState::Running, None, T);
        h.sample_ok(SourceId::Cpu, T, 100 * MS);
        assert!(!h.tick(T + 1_400 * MS));
        assert_eq!(
            h.publish().map(|s| s.get(SourceId::Cpu).state),
            Some(SourceState::Running)
        );
        assert!(h.tick(T + 1_600 * MS));
        let snap = h.publish().expect("stale is a change");
        assert_eq!(snap.get(SourceId::Cpu).state, SourceState::Stale);
        assert_eq!(snap.get(SourceId::Cpu).last_ok_ns, Some(T));

        // Fresh data clears the mark.
        h.sample_ok(SourceId::Cpu, T + 1_700 * MS, 100 * MS);
        let snap = h.publish().expect("fresh again is a change");
        assert_eq!(snap.get(SourceId::Cpu).state, SourceState::Running);
    }

    #[test]
    fn stale_after_threshold_1hz() {
        let mut h = HealthState::new();
        h.apply(SourceId::Memory, SourceState::Running, None, T);
        h.sample_ok(SourceId::Memory, T, S);
        assert!(!h.tick(T + 2_900 * MS));
        assert!(h.tick(T + 3_100 * MS));
        let snap = h.publish().expect("stale is a change");
        assert_eq!(snap.get(SourceId::Memory).state, SourceState::Stale);
    }

    #[test]
    fn disabled_is_down_until_running() {
        let mut h = HealthState::new();
        h.apply(SourceId::Cpu, SourceState::Running, None, T);
        h.sample_ok(SourceId::Cpu, T, 100 * MS);
        assert!(h.tick(T + 2 * S));
        assert!(!h.is_down(SourceId::Cpu));

        let until = T + 62 * S;
        let err: Arc<str> = Arc::from("read /proc/stat failed");
        h.apply(
            SourceId::Cpu,
            SourceState::Disabled { until },
            Some(err.clone()),
            T + 2 * S,
        );
        assert!(h.is_down(SourceId::Cpu));
        let snap = h.publish().expect("state change");
        assert_eq!(
            snap.get(SourceId::Cpu).state,
            SourceState::Disabled { until }
        );
        assert_eq!(snap.get(SourceId::Cpu).last_error, Some(err));

        h.apply(SourceId::Cpu, SourceState::Running, None, until);
        assert!(!h.is_down(SourceId::Cpu));
        let snap = h.publish().expect("state change");
        assert_eq!(snap.get(SourceId::Cpu).state, SourceState::Running);
        assert_eq!(snap.get(SourceId::Cpu).last_error, None);
        // The last data is 60 s old, but the source only just came back.
        assert!(!h.tick(until + S));

        h.apply(SourceId::Cpu, SourceState::Failed, None, until + S);
        assert!(h.is_down(SourceId::Cpu));
        assert!(!h.is_down(SourceId::Memory));
    }

    #[test]
    fn all_failed_covers_every_source() {
        let snap = HealthState::all_failed(3, "engine restarted too often");
        assert_eq!(snap.sources.len(), SourceId::COUNT);
        for id in SourceId::ALL {
            let s = snap.get(id);
            assert_eq!(s.state, SourceState::Failed);
            assert_eq!(s.last_error.as_deref(), Some("engine restarted too often"));
        }
    }

    #[test]
    fn publish_none_when_unchanged() {
        let mut h = HealthState::new();
        h.apply(SourceId::Disk, SourceState::Running, None, T);
        h.sample_ok(SourceId::Disk, T, S);
        let first = h.publish().expect("first state");
        assert_eq!(first.version, 1);
        assert!(!h.tick(T + S));
        assert!(h.publish().is_none());
        assert!(!h.tick(T + 2 * S));
        assert!(h.publish().is_none());

        // Same state and error again is not a change; data updates alone are not either.
        h.apply(SourceId::Disk, SourceState::Running, None, T + 2 * S);
        h.sample_ok(SourceId::Disk, T + 2 * S, S);
        assert!(h.publish().is_none());
    }

    #[test]
    fn publish_keeps_version_rising() {
        let mut h = HealthState::new();
        h.apply(SourceId::Net, SourceState::Running, None, T);
        let a = h.publish().expect("change");
        h.apply(SourceId::Net, SourceState::Late, None, T + S);
        let b = h.publish().expect("change");
        assert_eq!(b.version, a.version + 1);
        assert_eq!(b.get(SourceId::Net).state, SourceState::Late);
        assert_eq!(b.get(SourceId::Cpu).state, SourceState::Off);
    }

    #[test]
    fn last_error_is_one_line() {
        let mut h = HealthState::new();
        let long = format!("first\u{7}line{}\nsecond line", "x".repeat(300));
        h.apply(
            SourceId::Sensors,
            SourceState::Failed,
            Some(Arc::from(long)),
            T,
        );
        let snap = h.publish().expect("change");
        let err = snap
            .get(SourceId::Sensors)
            .last_error
            .clone()
            .expect("error");
        assert!(err.starts_with("firstline"));
        assert_eq!(err.chars().count(), MAX_ERROR_CHARS);
        assert!(!err.contains('\n'));

        let short: Arc<str> = Arc::from("permission denied");
        assert!(Arc::ptr_eq(&one_line(short.clone()), &short));
    }
}
