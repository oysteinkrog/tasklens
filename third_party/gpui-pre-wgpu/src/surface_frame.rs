//! Decides what the renderer does with an acquired surface texture, and
//! counts surface reconfigures. No wgpu types, so the tests need no GPU.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Window in which more than `RECONFIGURE_WARN_COUNT` reconfigures warn.
pub(crate) const RECONFIGURE_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const RECONFIGURE_WARN_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcquireStatus {
    Success,
    Suboptimal,
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameAction {
    Present,
    Reconfigure,
    Skip,
    Invalid,
}

/// A Suboptimal frame of the configured size is presented. Only a changed
/// size, Outdated or Lost rebuilds the swapchain.
pub(crate) fn frame_action(
    status: AcquireStatus,
    frame_size: (u32, u32),
    configured: (u32, u32),
) -> FrameAction {
    match status {
        AcquireStatus::Success => FrameAction::Present,
        AcquireStatus::Suboptimal if frame_size == configured => FrameAction::Present,
        AcquireStatus::Suboptimal | AcquireStatus::Outdated | AcquireStatus::Lost => {
            FrameAction::Reconfigure
        }
        AcquireStatus::Timeout | AcquireStatus::Occluded => FrameAction::Skip,
        AcquireStatus::Validation => FrameAction::Invalid,
    }
}

#[derive(Default)]
pub(crate) struct ReconfigureTracker {
    pub(crate) total: u64,
    recent: VecDeque<Instant>,
    warned_at: Option<Instant>,
}

impl ReconfigureTracker {
    /// Records one reconfigure. Returns true when a warning is due: more than
    /// 10 in the last 60 s and no warning in the last 60 s.
    pub(crate) fn record(&mut self, now: Instant) -> bool {
        self.total += 1;
        self.recent.push_back(now);
        while let Some(&first) = self.recent.front() {
            if now.saturating_duration_since(first) > RECONFIGURE_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        let quiet = self
            .warned_at
            .is_none_or(|w| now.saturating_duration_since(w) > RECONFIGURE_WINDOW);
        if self.recent.len() > RECONFIGURE_WARN_COUNT && quiet {
            self.warned_at = Some(now);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_action_per_status() {
        let s = (800, 600);
        let n = (900, 600);
        assert_eq!(frame_action(AcquireStatus::Success, s, s), FrameAction::Present);
        assert_eq!(frame_action(AcquireStatus::Suboptimal, s, s), FrameAction::Present);
        assert_eq!(frame_action(AcquireStatus::Suboptimal, n, s), FrameAction::Reconfigure);
        assert_eq!(frame_action(AcquireStatus::Outdated, s, s), FrameAction::Reconfigure);
        assert_eq!(frame_action(AcquireStatus::Lost, s, s), FrameAction::Reconfigure);
        assert_eq!(frame_action(AcquireStatus::Timeout, s, s), FrameAction::Skip);
        assert_eq!(frame_action(AcquireStatus::Occluded, s, s), FrameAction::Skip);
        assert_eq!(frame_action(AcquireStatus::Validation, s, s), FrameAction::Invalid);
    }

    fn run(t: &mut ReconfigureTracker, t0: Instant, start: u64, n: u64, step: u64) -> Vec<bool> {
        (0..n)
            .map(|i| t.record(t0 + Duration::from_secs(start + i * step)))
            .collect()
    }

    #[test]
    fn ten_reconfigures_in_window_do_not_warn() {
        let mut t = ReconfigureTracker::default();
        let r = run(&mut t, Instant::now(), 0, 10, 1);
        assert!(r.iter().all(|w| !w));
        assert_eq!(t.total, 10);
    }

    #[test]
    fn eleventh_in_window_warns_once() {
        let mut t = ReconfigureTracker::default();
        let r = run(&mut t, Instant::now(), 0, 13, 1);
        assert_eq!(r.iter().position(|w| *w), Some(10));
        assert_eq!(r.iter().filter(|w| **w).count(), 1);
        assert_eq!(t.total, 13);
    }

    #[test]
    fn spread_out_reconfigures_do_not_warn() {
        let mut t = ReconfigureTracker::default();
        let r = run(&mut t, Instant::now(), 0, 30, 7);
        assert!(r.iter().all(|w| !w));
        assert_eq!(t.total, 30);
    }

    #[test]
    fn warns_again_after_window() {
        let mut t = ReconfigureTracker::default();
        let t0 = Instant::now();
        let r = run(&mut t, t0, 0, 11, 1);
        assert!(r[10]);
        // Second burst starts 61 s after the warning (at 10 s).
        let r = run(&mut t, t0, 71, 11, 1);
        assert_eq!(r.iter().filter(|w| **w).count(), 1);
        assert!(r[10]);
        assert_eq!(t.total, 22);
    }
}
