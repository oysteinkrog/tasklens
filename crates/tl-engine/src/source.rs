//! The `Source` trait and the sampling loop every periodic worker runs (ADR-004, ADR-008).
//!
//! Each turn the loop re-reads demand and settings, sleeps until the next absolute deadline,
//! applies baseline resets and forwarded commands from the control watch, runs one sample on
//! the source's own sampling thread, and sends the result to the reducer. Missed ticks are
//! skipped, never caught up. An off source wakes every `IDLE_RECHECK` and does no I/O. A source
//! whose `wakes_on_demand` is true also wakes at once when demand changes.
//!
//! Each source has its own sampling thread, started at its first sample and asleep between
//! samples (ADR-004, tasklens-wlf.30). A hung `sample` stalls only that source: after
//! `LATE_COUNT` periods its health goes `Late`, and the other sources keep their rate. The
//! shared blocking pool is for actions, the launcher, the opener and the journal. Without a
//! blocking pool (LabRuntime) the sample goes through `spawn_blocking`, which runs it inline.
//!
//! Never hold a `watch` `Ref` across another channel call. This module only uses
//! `borrow_and_clone()` (bead `c11v6v`).
//!
//! `WorkerCommand::StringScan` and `HandleSearch` carry search patterns. Never log them.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, oneshot, watch};
use asupersync::runtime::JoinError;
use asupersync::time::{Interval, MissedTickBehavior, interval, sleep_until, timeout, timeout_at};
use asupersync::{Cx, Outcome, PanicPayload};
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
    /// D-Bus handles owned by `dbus/mod.rs`, such as the systemd manager connections (R-166).
    pub shared: crate::dbus::SharedHandles,
}

impl WorkerIo {
    pub fn new(
        updates: mpsc::Sender<EngineMsg>,
        demand: watch::Receiver<Demand>,
        settings: watch::Receiver<EngineSettings>,
        control: watch::Receiver<WorkerControl>,
        shared: crate::dbus::SharedHandles,
    ) -> Self {
        Self {
            updates,
            demand,
            settings,
            control,
            shared,
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
///
/// A worker copies the command list and starts each command later, so the reducer may push a
/// command out of the list in between. `claim` and `evict` take one lock, so each command is
/// either claimed by a worker or evicted by the reducer, never both (ADR-018).
#[derive(Debug)]
pub struct CommandProgress {
    started: [AtomicU64; SourceId::COUNT],
    done: [AtomicU64; SourceId::COUNT],
    /// Per source, the newest seq the reducer evicted before a worker claimed it.
    evicted: Mutex<[u64; SourceId::COUNT]>,
}

impl Default for CommandProgress {
    fn default() -> Self {
        Self {
            started: [const { AtomicU64::new(0) }; SourceId::COUNT],
            done: [const { AtomicU64::new(0) }; SourceId::COUNT],
            evicted: Mutex::new([0; SourceId::COUNT]),
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

    pub(crate) fn set_started(&self, id: SourceId, seq: u64) {
        self.started[id.index()].store(seq, Ordering::Release);
    }

    pub(crate) fn set_done(&self, id: SourceId, seq: u64) {
        self.done[id.index()].store(seq, Ordering::Release);
    }

    fn evicted(&self) -> MutexGuard<'_, [u64; SourceId::COUNT]> {
        self.evicted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The worker of `id` starts command `seq`: marks it started and returns true, or returns
    /// false when the reducer evicted it first. Commands of one source are claimed in `seq`
    /// order.
    pub fn claim(&self, id: SourceId, seq: u64) -> bool {
        let evicted = self.evicted();
        if seq <= evicted[id.index()] {
            return false;
        }
        self.started[id.index()].store(seq, Ordering::Release);
        true
    }

    /// The reducer pushes command `seq` of `id` out of the list: returns true when no worker
    /// claimed it, which then never runs. False when a worker claimed it and owns its end.
    pub fn evict(&self, id: SourceId, seq: u64) -> bool {
        let mut evicted = self.evicted();
        if seq <= self.started(id) {
            return false;
        }
        evicted[id.index()] = evicted[id.index()].max(seq);
        true
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
    /// `app_id` is the app id the sending session announced as a window on this host. Keep
    /// above and frame geometry act only on that pid's window with this app id (q4x76.12).
    /// `None` from the local session and from sessions with no own windows. `bridge` is the
    /// KWin bridge generation the command was queued for (`DesktopSession::command_generation`);
    /// any other bridge drops it unrun (ADR-018 "Generations").
    KWin {
        cmd: KWinCommand,
        app_id: Option<Arc<str>>,
        bridge: u64,
    },
    DetailRefresh {
        target: DetailTarget,
        tab: DetailTab,
    },
    /// From the reducer, not the user: this process tab opened since the last tick.
    DetailOpened {
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
            WorkerCommand::KWin { .. } => "KWin",
            WorkerCommand::DetailRefresh { .. } => "DetailRefresh",
            WorkerCommand::DetailOpened { .. } => "DetailOpened",
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

/// A periodic sampler. `sample` runs on the source's own sampling thread; the other methods
/// run on the async worker and must not block.
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    /// `None` means the source is off.
    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration>;
    /// Runs on the sampling thread. Takes `self` by value so no lock is needed.
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
    /// True when a demand change must end the wait for the next sample at once, so a long
    /// period does not delay the switch to a new one. Other sources keep the old rate until
    /// the sample already scheduled.
    fn wakes_on_demand(&self) -> bool {
        false
    }
}

/// The sampling loop (ADR-004). Returns `Outcome::Ok(())` when `cx` is cancelled and
/// `Outcome::Err` when the sampling thread cannot be started or the reducer's queue is closed,
/// so the supervisor restarts the worker. On return the sampling thread ends; a thread stuck in
/// `sample` is left detached and ends when that sample returns.
pub async fn run<S: Source>(cx: Cx, mut io: WorkerIo, mut src: S) -> Outcome<(), WorkerError> {
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
    let mut sampler: Option<Sampler<S>> = None;

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

        if src.wakes_on_demand() {
            match timeout_at(iv.deadline(), io.demand.changed(&cx)).await {
                // Read the new demand at the top of the loop; the deadline stays.
                Ok(Ok(())) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    continue;
                }
                // The demand sender is gone: wait for the deadline as the other sources do.
                Ok(Err(_)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    sleep_until(iv.deadline()).await;
                }
                Err(_) => {}
            }
        } else {
            sleep_until(iv.deadline()).await;
        }
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
            if !progress.claim(id, queued.seq) {
                continue;
            }
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
        let (s, update) = if cx.blocking_pool_handle().is_none() {
            // LabRuntime: `spawn_blocking` runs the sample inline in a child task, which keeps
            // the lab's schedule and catches a panic.
            let job_name = crate::controller::source_name(id);
            let mut job = match cx.spawn_blocking(move |_| {
                let _running = crate::bridge::job_running(job_name);
                src.sample()
            }) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            match job.join(&cx).await {
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
            }
        } else {
            let thread = match &mut sampler {
                Some(thread) => thread,
                slot @ None => match Sampler::spawn(id) {
                    Ok(thread) => slot.insert(thread),
                    Err(e) => {
                        return Outcome::Err(WorkerError::Other(
                            format!("sampling thread: {e}").into(),
                        ));
                    }
                },
            };
            let running = crate::bridge::job_running(crate::controller::source_name(id));
            let Some(mut done) = thread.start(src, running) else {
                return Outcome::Err(WorkerError::Other("sampling thread ended".into()));
            };
            // A sample still running after `LATE_COUNT` periods is hung: report `Late` now,
            // not when it returns.
            let mut hung_after = period.map(|p| p * LATE_COUNT);
            let got = loop {
                let Some(wait) = hung_after.take() else {
                    break done.recv(&cx).await;
                };
                match timeout(cx.now(), wait, done.recv(&cx)).await {
                    Ok(got) => break got,
                    Err(_) => {
                        if let Some(state) = late.hung() {
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
                }
            };
            match got {
                Ok(Some(done)) => done,
                Ok(None) => return Outcome::Panicked(PanicPayload::new("sample panicked")),
                Err(oneshot::RecvError::Cancelled) => return Outcome::Ok(()),
                Err(e) => {
                    return Outcome::Err(WorkerError::Other(
                        format!("sampling thread: {e}").into(),
                    ));
                }
            }
        };
        src = s;
        // A job that ran past the next deadline must not start the next job at once. Skip to
        // the first deadline on the grid that is still in the future.
        let now = cx.now();
        if now > iv.deadline() {
            iv.tick(now);
        }

        if let Some(p) = period {
            let took = Duration::from_nanos(now.duration_since(started));
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

/// A source's own sampling thread. It sleeps on its job channel between samples. Dropping the
/// `Sampler` ends an idle thread at once. A thread stuck in `sample` is never joined: it ends
/// when the sample returns and finds the worker gone, so it never holds up shutdown.
struct Sampler<S> {
    jobs: std::sync::mpsc::Sender<Job<S>>,
}

/// One sample request, its shutdown trace guard and where its result goes. `None` means
/// `sample` panicked.
type Job<S> = (
    S,
    crate::bridge::RunGuard,
    oneshot::Sender<Option<(S, SourceUpdate)>>,
);

impl<S: Source> Sampler<S> {
    fn spawn(id: SourceId) -> std::io::Result<Self> {
        let (jobs, requests) = std::sync::mpsc::channel::<Job<S>>();
        std::thread::Builder::new()
            .name(format!("tl-src-{id:?}"))
            .spawn(move || {
                while let Ok((src, running, reply)) = requests.recv() {
                    let done = std::panic::catch_unwind(AssertUnwindSafe(|| src.sample())).ok();
                    drop(running);
                    // A failed send means the worker is gone; the next `recv` ends the thread.
                    let _ = reply.send_blocking(done);
                }
            })?;
        Ok(Self { jobs })
    }

    /// Hands `src` to the thread. Fails when the thread has ended. `running` ends when the
    /// sample returns.
    fn start(
        &self,
        src: S,
        running: crate::bridge::RunGuard,
    ) -> Option<oneshot::Receiver<Option<(S, SourceUpdate)>>> {
        let (reply, done) = oneshot::channel();
        self.jobs.send((src, running, reply)).ok()?;
        Some(done)
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

    /// A sample that has not returned after `LATE_COUNT` periods. Returns `Late` once. The
    /// next on-time job reports `Running` again.
    fn hung(&mut self) -> Option<SourceState> {
        if self.reported {
            return None;
        }
        self.reported = true;
        Some(SourceState::Late)
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
    #[test]
    fn hung_sample_reports_late_once_then_running_after_an_on_time_job() {
        let period = Duration::from_millis(100);
        let mut late = Lateness::default();
        assert_eq!(late.hung(), Some(SourceState::Late));
        assert_eq!(late.hung(), None);
        assert_eq!(late.record(Duration::from_secs(5), period), None);
        assert_eq!(
            late.record(Duration::from_millis(10), period),
            Some(SourceState::Running)
        );
    }
}
