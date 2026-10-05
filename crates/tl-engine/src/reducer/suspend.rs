//! Suspend and resume (ADR-008, E09-008).
//!
//! Two signals report a suspend, whichever comes first: logind `PrepareForSleep` (`Sleep`) and
//! the clock check in every worker (`ResumeDetected`, `Δboot − Δmono > 250 ms`). Several workers
//! report the same resume, so a second report within `RESUME_WINDOW_NS` of the last resume does
//! nothing. `apply.rs` turns a `Transition` into the reducer effects.

use tl_model::health::SourceId;
use tl_model::time::TimeNs;

/// Resume reports this close to the last resume belong to it.
pub const RESUME_WINDOW_NS: TimeNs = 2_000_000_000;

/// What a sleep or resume report changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transition {
    /// The machine is going to sleep: series samples become gaps until resume.
    Entering,
    /// The machine woke: gap every series, reset baselines, re-enumerate devices.
    Resumed,
    /// Nothing to do: a repeated report.
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SuspendState {
    /// Between logind `PrepareForSleep(true)` and the resume.
    pub sleeping: bool,
    pub slept_at_ns: Option<TimeNs>,
    pub resumed_at_ns: Option<TimeNs>,
    pub resume_count: u32,
}

impl SuspendState {
    /// Logind `PrepareForSleep(entering)`. A repeated `true` gives `None`. A `false` within
    /// `RESUME_WINDOW_NS` after a clock resume of the same sleep gives `None`.
    pub fn on_sleep(&mut self, entering: bool, t_ns: TimeNs) -> Transition {
        if entering {
            if self.sleeping {
                return Transition::None;
            }
            self.sleeping = true;
            self.slept_at_ns = Some(t_ns);
            return Transition::Entering;
        }
        self.resume(t_ns)
    }

    /// A worker's clock check saw a resume. The first report gives `Resumed`; reports from other
    /// workers within `RESUME_WINDOW_NS` give `None`.
    pub fn on_clock_resume(&mut self, id: SourceId, t_ns: TimeNs) -> Transition {
        let transition = self.resume(t_ns);
        tracing::debug!(target: "tasklens::engine", ?id, ?transition, "clock resume");
        transition
    }

    /// True while sleeping: series samples are appended as gaps.
    pub fn drop_rates(&self) -> bool {
        self.sleeping
    }

    /// A resume that started no new sleep since the last resume and comes within
    /// `RESUME_WINDOW_NS` of it is the same resume.
    fn resume(&mut self, t_ns: TimeNs) -> Transition {
        self.sleeping = false;
        if let Some(resumed) = self.resumed_at_ns
            && self.slept_at_ns.is_none_or(|slept| slept <= resumed)
            && t_ns.saturating_sub(resumed) < RESUME_WINDOW_NS
        {
            return Transition::None;
        }
        self.resumed_at_ns = Some(t_ns);
        self.resume_count += 1;
        Transition::Resumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: TimeNs = 1_000_000_000;

    #[test]
    fn sleep_then_resume() {
        let mut s = SuspendState::default();
        assert_eq!(s.on_sleep(true, SEC), Transition::Entering);
        assert!(s.drop_rates());
        assert_eq!(s.on_sleep(true, SEC), Transition::None);
        assert_eq!(s.on_sleep(false, 10 * SEC), Transition::Resumed);
        assert!(!s.drop_rates());
        assert_eq!(s.resume_count, 1);
    }

    #[test]
    fn clock_resumes_within_window_count_once() {
        let mut s = SuspendState::default();
        assert_eq!(
            s.on_clock_resume(SourceId::Cpu, 10 * SEC),
            Transition::Resumed
        );
        assert_eq!(
            s.on_clock_resume(SourceId::Memory, 10 * SEC + SEC / 20),
            Transition::None
        );
        // Logind's resume signal for the same sleep arrives after the workers saw it.
        assert_eq!(s.on_sleep(false, 11 * SEC), Transition::None);
        assert_eq!(
            s.on_clock_resume(SourceId::Cpu, 13 * SEC),
            Transition::Resumed
        );
        assert_eq!(s.resume_count, 2);
    }

    #[test]
    fn new_sleep_inside_window_is_a_new_resume() {
        let mut s = SuspendState::default();
        assert_eq!(s.on_sleep(false, 10 * SEC), Transition::Resumed);
        assert_eq!(s.on_sleep(true, 10 * SEC + SEC / 2), Transition::Entering);
        assert_eq!(
            s.on_clock_resume(SourceId::Disk, 11 * SEC),
            Transition::Resumed
        );
    }
}
