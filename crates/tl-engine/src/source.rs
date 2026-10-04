//! The `Source` trait and the sampling loop every periodic worker runs (ADR-004, ADR-008).
//!
//! Each turn the loop re-reads demand and settings, sleeps until the next absolute deadline,
//! applies baseline resets and forwarded commands from the control watch, runs one sample on
//! the blocking pool, and sends the result to the reducer. Missed ticks are skipped, never
//! caught up. An off source wakes every `IDLE_RECHECK` and does no I/O.
//!
//! Never hold a `watch` `Ref` across another channel call. This module only uses
//! `borrow_and_clone()` (bead `c11v6v`).
//!
//! `WorkerCommand::StringScan` and `HandleSearch` carry search patterns. Never log them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::JoinError;
use asupersync::time::{Interval, MissedTickBehavior, interval, sleep_until};
use asupersync::{Cx, Outcome};
use tl_model::command::{DeviceCommand, HistoryWhat, KWinCommand, SetupStep};
use tl_model::demand::Demand;
use tl_model::details::{DetailTab, DetailTarget, ScanId, SearchId, StringScanOpts};
use tl_model::health::{SourceId, SourceState};
use tl_model::history::HistoryRange;
use tl_model::ids::ProcessKey;
use tl_model::settings::EngineSettings;
use tl_model::time::now_boot_ns;

use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// An off source wakes this often to re-read demand. It does no I/O on these wakes.
pub const IDLE_RECHECK: Duration = Duration::from_millis(500);
/// A job is late when it runs longer than this fraction of the period.
pub const LATE_FRACTION: f64 = 0.8;
/// This many late jobs in a row set `SourceState::Late`.
pub const LATE_COUNT: u32 = 3;

/// The channel ends a worker generation owns. Each factory clones these per generation; a
/// restartable worker never owns an `mpsc::Receiver` (ADR-004).
#[derive(Clone)]
pub struct WorkerIo {
    /// The reducer's queue: asupersync mpsc, capacity 256, shared with the UI command path.
    pub updates: mpsc::Sender<EngineMsg>,
    pub demand: watch::Receiver<Demand>,
    pub settings: watch::Receiver<EngineSettings>,
    pub control: watch::Receiver<WorkerControl>,
    // E08-009: shared
}

impl WorkerIo {
    pub fn new(
        updates: mpsc::Sender<EngineMsg>,
        demand: watch::Receiver<Demand>,
        settings: watch::Receiver<EngineSettings>,
        control: watch::Receiver<WorkerControl>,
        // E08-009: shared
    ) -> Self {
        Self {
            updates,
            demand,
            settings,
            control,
            // E08-009: shared
        }
    }
}

/// Reducer-to-worker control (R-166 B3-1).
#[derive(Clone, Default, Debug)]
pub struct WorkerControl {
    /// Bumped on resume and on `Command::ResetBaselines`.
    pub reset_generation: u64,
    /// The newest 64 forwarded commands, in ascending `seq` order.
    pub commands: Arc<[QueuedCommand]>,
    /// Made once per session. The reducer clones the current `WorkerControl` when it forwards a
    /// command, so every value in the watch shares this `Arc` (R-276).
    pub progress: Arc<CommandProgress>,
}

/// `progress` compares by identity: workers write to it, so its contents are not part of the
/// value the reducer sends.
impl PartialEq for WorkerControl {
    fn eq(&self, other: &Self) -> bool {
        self.reset_generation == other.reset_generation
            && self.commands == other.commands
            && Arc::ptr_eq(&self.progress, &other.progress)
    }
}

/// Per source, the seq of the last command a worker started and the last one it finished
/// (R-276). A new generation resumes after `started`. When `started > done`, the command at
/// `started` was running when the old generation died, so it is reported and never rerun.
#[derive(Debug)]
pub struct CommandProgress {
    started: [AtomicU64; SourceId::COUNT],
    done: [AtomicU64; SourceId::COUNT],
}

impl Default for CommandProgress {
    fn default() -> Self {
        Self {
            started: [const { AtomicU64::new(0) }; SourceId::COUNT],
            done: [const { AtomicU64::new(0) }; SourceId::COUNT],
        }
    }
}

impl CommandProgress {
    pub fn started(&self, id: SourceId) -> u64 {
        self.started[id.index()].load(Ordering::Acquire)
    }

    pub fn done(&self, id: SourceId) -> u64 {
        self.done[id.index()].load(Ordering::Acquire)
    }

    fn set_started(&self, id: SourceId, seq: u64) {
        self.started[id.index()].store(seq, Ordering::Release);
    }

    fn set_done(&self, id: SourceId, seq: u64) {
        self.done[id.index()].store(seq, Ordering::Release);
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct QueuedCommand {
    pub seq: u64,
    pub target: SourceId,
    pub command: WorkerCommand,
}

#[derive(Clone, PartialEq, Debug)]
pub enum WorkerCommand {
    RefreshNow,
    Reprobe,
    KWin(KWinCommand),
    DetailRefresh {
        target: DetailTarget,
        tab: DetailTab,
    },
    ComputeHashes(ProcessKey),
    VerifyPackage(ProcessKey),
    StringScan {
        scan: ScanId,
        key: ProcessKey,
        opts: StringScanOpts,
    },
    CancelStringScan(ScanId),
    HandleSearch {
        search: SearchId,
        pattern: Arc<str>,
        include_maps: bool,
        privileged: bool,
    },
    CancelHandleSearch(SearchId),
    HistoryQuery {
        id: u64,
        range: HistoryRange,
        what: HistoryWhat,
    },
    HistoryDelete {
        keys: Arc<[ProcessKey]>,
    },
    SetupStatus,
    RunSetup {
        step: SetupStep,
    },
    /// Goes to the `Udisks`, `Inventory` or `UPower` worker by variant (R-139 item 3).
    Device(DeviceCommand),
}

impl WorkerCommand {
    /// The variant name, for error text. Never includes arguments, which can hold search
    /// patterns.
    pub fn kind(&self) -> &'static str {
        match self {
            WorkerCommand::RefreshNow => "RefreshNow",
            WorkerCommand::Reprobe => "Reprobe",
            WorkerCommand::KWin(_) => "KWin",
            WorkerCommand::DetailRefresh { .. } => "DetailRefresh",
            WorkerCommand::ComputeHashes(_) => "ComputeHashes",
            WorkerCommand::VerifyPackage(_) => "VerifyPackage",
            WorkerCommand::StringScan { .. } => "StringScan",
            WorkerCommand::CancelStringScan(_) => "CancelStringScan",
            WorkerCommand::HandleSearch { .. } => "HandleSearch",
            WorkerCommand::CancelHandleSearch(_) => "CancelHandleSearch",
            WorkerCommand::HistoryQuery { .. } => "HistoryQuery",
            WorkerCommand::HistoryDelete { .. } => "HistoryDelete",
            WorkerCommand::SetupStatus => "SetupStatus",
            WorkerCommand::RunSetup { .. } => "RunSetup",
            WorkerCommand::Device(_) => "Device",
        }
    }
}

/// A periodic sampler. `sample` runs on the blocking pool; the other methods run on the async
/// worker and must not block.
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    /// `None` means the source is off.
    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration>;
    /// Runs on the blocking pool. Takes `self` by value so no lock is needed.
    fn sample(self) -> (Self, SourceUpdate)
    where
        Self: Sized;
    /// Called when `reset_generation` rose or the period changed. The next sample reports
    /// rates as `WarmingUp` (ADR-008).
    fn reset_baselines(&mut self) {}
    /// A forwarded command. Returns true to sample at once.
    fn command(&mut self, _cmd: &WorkerCommand) -> bool {
        false
    }
    /// Device lists and details, drained after each sample and sent before it (R-139 item 8).
    fn take_extra(&mut self) -> Option<SourceUpdate> {
        None
    }
}

/// The sampling loop (ADR-004). Returns `Outcome::Ok(())` when `cx` is cancelled and
/// `Outcome::Err` when a job cannot be spawned or the reducer's queue is closed, so the
/// supervisor restarts the worker.
pub async fn run<S: Source>(cx: Cx, io: WorkerIo, mut src: S) -> Outcome<(), WorkerError> {
    let id = src.id();
    let mut period = src.period(
        &io.demand.borrow_and_clone(),
        &io.settings.borrow_and_clone(),
    );
    let mut iv = new_interval(&cx, period);

    // A new generation resumes after the last command a generation started (R-276). Later
    // queued commands run once. The started one is skipped; if it never finished, it was
    // running when the old generation died, so it is reported instead of rerun.
    let control = io.control.borrow_and_clone();
    let progress = Arc::clone(&control.progress);
    let mut reset_generation = control.reset_generation;
    let mut last_seq = progress.started(id);
    let crashed = (last_seq > progress.done(id))
        .then(|| control.commands.iter().find(|c| c.seq == last_seq))
        .map(|found| found.map(|c| c.command.kind()));
    drop(control);
    if let Some(kind) = crashed {
        progress.set_done(id, last_seq);
        // A skipped RefreshNow is dropped silently: running it again is harmless.
        if kind != Some("RefreshNow") {
            let text = format!(
                "{} was not run again after the worker restarted",
                kind.unwrap_or("A command")
            );
            let health = SourceUpdate::Health {
                id,
                state: SourceState::Failed,
                last_error: Some(text.into()),
                t_ns: now_boot_ns(),
            };
            if let Some(stop) = send(&cx, &io, health).await {
                return stop;
            }
        }
    }

    let mut late = Lateness::default();

    loop {
        let next = src.period(
            &io.demand.borrow_and_clone(),
            &io.settings.borrow_and_clone(),
        );
        if next != period {
            if period.is_some() && next.is_some() {
                src.reset_baselines();
            }
            period = next;
            iv = new_interval(&cx, period);
        }

        sleep_until(iv.deadline()).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
        iv.tick(cx.now());

        let control = io.control.borrow_and_clone();
        if control.reset_generation > reset_generation {
            reset_generation = control.reset_generation;
            src.reset_baselines();
        }
        let mut forced = false;
        // The newest command that waits for this turn's sample before it counts as done.
        let mut pending_done = None;
        for queued in control.commands.iter().filter(|c| c.seq > last_seq) {
            if queued.target != id {
                continue;
            }
            progress.set_started(id, queued.seq);
            let wants_sample = match queued.command {
                WorkerCommand::RefreshNow => true,
                ref command => src.command(command),
            };
            forced |= wants_sample;
            if wants_sample || pending_done.is_some() {
                pending_done = Some(queued.seq);
            } else {
                progress.set_done(id, queued.seq);
            }
        }
        if let Some(newest) = control.commands.last() {
            last_seq = last_seq.max(newest.seq);
        }
        drop(control);

        if period.is_none() && !forced {
            continue;
        }

        let started = cx.now();
        let mut job = match cx.spawn_blocking(move |_| src.sample()) {
            Ok(job) => job,
            Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
        };
        let (s, update) = match job.join(&cx).await {
            Ok(done) => done,
            Err(JoinError::Cancelled(reason)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Cancelled(reason);
            }
            Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
            Err(JoinError::PolledAfterCompletion) => {
                return Outcome::Err(WorkerError::Other("job polled after completion".into()));
            }
        };
        src = s;

        if let Some(p) = period {
            let took = Duration::from_nanos(cx.now().duration_since(started));
            if let Some(state) = late.record(took, p) {
                let health = SourceUpdate::Health {
                    id,
                    state,
                    last_error: None,
                    t_ns: now_boot_ns(),
                };
                if let Some(stop) = send(&cx, &io, health).await {
                    return stop;
                }
            }
        }

        while let Some(extra) = src.take_extra() {
            if let Some(stop) = send(&cx, &io, extra).await {
                return stop;
            }
        }
        if let Some(stop) = send(&cx, &io, update).await {
            return stop;
        }
        if let Some(seq) = pending_done {
            progress.set_done(id, seq);
        }
    }
}

fn new_interval(cx: &Cx, period: Option<Duration>) -> Interval {
    let mut iv = interval(cx.now(), period.unwrap_or(IDLE_RECHECK));
    iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    iv
}

/// Sends one update. Returns the loop's result when the loop must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

/// The `Late` rule: `LATE_COUNT` jobs in a row longer than `LATE_FRACTION` of the period.
#[derive(Default)]
struct Lateness {
    streak: u32,
    reported: bool,
}

impl Lateness {
    /// Returns the state to report, once per change.
    fn record(&mut self, took: Duration, period: Duration) -> Option<SourceState> {
        if took > period.mul_f64(LATE_FRACTION) {
            self.streak = self.streak.saturating_add(1);
            if self.streak >= LATE_COUNT && !self.reported {
                self.reported = true;
                return Some(SourceState::Late);
            }
        } else {
            self.streak = 0;
            if self.reported {
                self.reported = false;
                return Some(SourceState::Running);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_control_default_is_empty() {
        let control = WorkerControl::default();
        assert_eq!(control.reset_generation, 0);
        assert!(control.commands.is_empty());
    }

    #[test]
    fn late_after_three_slow_jobs_then_running_once() {
        let period = Duration::from_millis(100);
        let slow = Duration::from_millis(250);
        let fast = Duration::from_millis(10);
        let mut late = Lateness::default();
        assert_eq!(late.record(slow, period), None);
        assert_eq!(late.record(slow, period), None);
        assert_eq!(late.record(slow, period), Some(SourceState::Late));
        assert_eq!(late.record(slow, period), None);
        assert_eq!(late.record(fast, period), Some(SourceState::Running));
        assert_eq!(late.record(fast, period), None);
    }

    #[test]
    fn job_at_exactly_the_threshold_is_on_time() {
        let period = Duration::from_millis(100);
        let mut late = Lateness::default();
        for _ in 0..LATE_COUNT {
            assert_eq!(late.record(Duration::from_millis(80), period), None);
        }
    }
}
