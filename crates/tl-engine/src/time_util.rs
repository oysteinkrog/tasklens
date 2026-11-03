//! A timeout that also ends when the task is cancelled (tasklens-25fwy).
//!
//! asupersync 0.6.2 changed `time::timeout`: its deadline no longer ends when the task is
//! cancelled, so a task parked in `timeout(cx.now(), 2 s, ...)` holds a shutdown for up to 2 s.
//! The engine drains in `DRAIN_BUDGET` (1 s), so any such wait longer than about 1 s made the
//! drain fail. 0.5.0 ended the wait at once on cancel, and the engine was written for that.
//!
//! [`timeout`] and [`timeout_at`] bring that back. They poll the inner future, then a `Sleep`
//! through its own `Future::poll`, which ends at once when the task is cancelled (except for a
//! `Timeout` or `Deadline` cancel, and except while the `Cx` is masked). Every timed wait in
//! tl-engine goes through them; the test `no_raw_timeout_in_src` fails on a direct
//! `time::timeout`.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use asupersync::Cx;
use asupersync::time;
use asupersync::types::Time;

/// Why a timed wait ended without the inner future's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitEnd {
    /// The deadline passed.
    Elapsed,
    /// The task was cancelled before the deadline. The caller should stop.
    Cancelled,
}

/// Runs `fut` for at most `d`. Ends early with `Err(WaitEnd::Cancelled)` when the task is
/// cancelled.
pub(crate) async fn timeout<F: Future>(cx: &Cx, d: Duration, fut: F) -> Result<F::Output, WaitEnd> {
    timeout_at(cx, cx.now().saturating_add_nanos(nanos(d)), fut).await
}

/// Runs `fut` until `deadline` at most. Ends early with `Err(WaitEnd::Cancelled)` when the task
/// is cancelled.
pub(crate) async fn timeout_at<F: Future>(
    cx: &Cx,
    deadline: Time,
    fut: F,
) -> Result<F::Output, WaitEnd> {
    let mut fut = pin!(fut);
    let mut sleep = pin!(time::sleep_until(deadline));
    poll_fn(|task| {
        // As in `time::timeout`: past the deadline the wait has elapsed, even if the inner
        // future is ready now. Exactly at the deadline, completed work wins.
        if cx.now() > deadline {
            return Poll::Ready(Err(WaitEnd::Elapsed));
        }
        if let Poll::Ready(out) = fut.as_mut().poll(task) {
            return Poll::Ready(Ok(out));
        }
        match sleep.as_mut().poll(task) {
            Poll::Ready(()) if cx.now() < deadline => Poll::Ready(Err(WaitEnd::Cancelled)),
            Poll::Ready(()) => Poll::Ready(Err(WaitEnd::Elapsed)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use asupersync::runtime::{RootDrainOutcome, RuntimeBuilder};
    use asupersync::types::Budget;
    use asupersync::{CancelKind, CancelReason, LabConfig, LabRuntime};

    use crate::bridge::DRAIN_BUDGET;

    type Slot<T> = Arc<Mutex<Option<T>>>;
    /// The wait's result and the virtual time it took.
    type WaitSlot = Slot<(Result<(), WaitEnd>, Duration)>;

    /// The D-Bus `READ_TIMEOUT` that the kwin serve loop waits in.
    const LONG: Duration = Duration::from_secs(2);

    fn lab() -> LabRuntime {
        LabRuntime::new(LabConfig::new(7).max_steps(100_000))
    }

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(nanos(d));
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    /// Spawns a task that runs `timeout(cx, LONG, pending)` and stores the result and the
    /// virtual time it took. `masked` runs the whole wait with cancellation masked.
    fn spawn_wait(
        lab: &mut LabRuntime,
        masked: bool,
    ) -> (WaitSlot, asupersync::runtime::TaskHandle<()>) {
        let out = WaitSlot::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let start = cx.now();
                let mut wait = pin!(timeout(&cx, LONG, std::future::pending::<()>()));
                let result = if masked {
                    poll_fn(|task| cx.masked(|| wait.as_mut().poll(task))).await
                } else {
                    wait.await
                };
                let took = Duration::from_nanos(cx.now().duration_since(start));
                *sink.lock().expect("slot lock") = Some((result, took));
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        (out, handle)
    }

    #[test]
    fn ready_future_wins() {
        let mut lab = lab();
        let out: Slot<Result<u32, WaitEnd>> = Slot::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let result = timeout(&cx, LONG, std::future::ready(7)).await;
                *sink.lock().expect("slot lock") = Some(result);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert_eq!(out.lock().expect("slot lock").take(), Some(Ok(7)));
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn elapses_at_the_deadline() {
        let mut lab = lab();
        let (out, _handle) = spawn_wait(&mut lab, false);
        advance(&mut lab, LONG - Duration::from_millis(1));
        assert!(out.lock().expect("slot lock").is_none(), "done before 2 s");
        advance(&mut lab, Duration::from_millis(1));
        let (result, took) = out.lock().expect("slot lock").take().expect("done at 2 s");
        assert_eq!(result, Err(WaitEnd::Elapsed));
        assert_eq!(took, LONG);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    /// Regression (tasklens-25fwy): a shutdown cancel ends the wait with no time passing.
    #[test]
    fn shutdown_cancel_ends_the_wait_at_once() {
        let mut lab = lab();
        let (out, handle) = spawn_wait(&mut lab, false);
        advance(&mut lab, Duration::from_millis(100));
        assert!(out.lock().expect("slot lock").is_none(), "still waiting");

        handle.abort_with_reason(CancelReason::new(CancelKind::Shutdown));
        lab.run_until_idle();
        let (result, took) = out
            .lock()
            .expect("slot lock")
            .take()
            .expect("the wait ended with no time advance");
        assert_eq!(result, Err(WaitEnd::Cancelled));
        assert_eq!(took, Duration::from_millis(100));
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    /// While masked (the D-Bus release and close paths), a cancel does not end the wait.
    #[test]
    fn masked_wait_runs_to_its_deadline() {
        let mut lab = lab();
        let (out, handle) = spawn_wait(&mut lab, true);
        handle.abort();
        lab.run_until_idle();
        assert!(
            out.lock().expect("slot lock").is_none(),
            "masked wait goes on"
        );
        advance(&mut lab, LONG);
        let (result, took) = out.lock().expect("slot lock").take().expect("done at 2 s");
        assert_eq!(result, Err(WaitEnd::Elapsed));
        assert_eq!(took, LONG);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    /// On the real runtime, as `EngineHandle::shutdown` drains: a task parked in a 2 s wait (the
    /// stand-in for the kwin worker's `next_signal`) lets the root region drain well inside
    /// `DRAIN_BUDGET`. With asupersync's own `time::timeout` this drain ran out (tasklens-25fwy).
    #[test]
    fn root_drains_while_a_task_waits() {
        let rt = RuntimeBuilder::new()
            .worker_threads(2)
            .build()
            .expect("runtime builds");
        let (parked_tx, parked) = std::sync::mpsc::channel();
        let (ended_tx, ended) = std::sync::mpsc::channel();
        rt.handle()
            .try_spawn_with_cx(move |cx| async move {
                let _ = parked_tx.send(());
                let result = timeout(&cx, LONG, std::future::pending::<()>()).await;
                let _ = ended_tx.send(result);
            })
            .expect("task spawns");
        parked
            .recv_timeout(Duration::from_secs(5))
            .expect("the task started");
        // Let it park inside the wait.
        std::thread::sleep(Duration::from_millis(50));

        let begin = Instant::now();
        let outcome = rt.drain_root_region(DRAIN_BUDGET);
        let took = begin.elapsed();
        assert_eq!(outcome, RootDrainOutcome::Quiescent, "took {took:?}");
        assert!(took < DRAIN_BUDGET / 4, "drain took {took:?}");
        assert_eq!(
            ended.recv_timeout(Duration::from_secs(1)),
            Ok(Err(WaitEnd::Cancelled))
        );
        assert!(rt.shutdown_timeout(crate::bridge::TEARDOWN_BUDGET));
    }

    /// The asupersync timeout items that ignore cancel since 0.6.2, used anywhere in `src`
    /// outside this file. Returns `path:line: text` for each.
    fn raw_timeouts(path: &str, text: &str) -> Vec<String> {
        const ITEMS: [&str; 3] = ["timeout", "timeout_at", "TimeoutFuture"];
        let mut found = Vec::new();
        let code: Vec<&str> = text
            .lines()
            .map(|line| {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") { "" } else { line }
            })
            .collect();
        for (i, line) in code.iter().enumerate() {
            let mut rest = *line;
            while let Some(at) = rest.find("time::") {
                let before = &rest[..at];
                let after = &rest[at + "time::".len()..];
                rest = after;
                // `time_util::` and `other_time::` are not asupersync's `time`.
                if before.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let item: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let listed = if after.starts_with('{') {
                    // A `use asupersync::time::{...}` list, maybe over several lines.
                    let mut list = String::from(after);
                    for next in &code[i + 1..] {
                        if list.contains('}') {
                            break;
                        }
                        list.push_str(next);
                    }
                    let list = list.split('}').next().unwrap_or_default().to_owned();
                    list.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|word| ITEMS.contains(&word))
                } else {
                    ITEMS.contains(&item.as_str())
                };
                if listed {
                    found.push(format!("{path}:{}: {}", i + 1, line.trim()));
                    break;
                }
            }
        }
        found
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("list src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Guard (tasklens-25fwy): every timed wait in tl-engine uses this module, so a shutdown is
    /// not held by a wait that ignores cancel.
    #[test]
    fn no_raw_timeout_in_src() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        assert!(files.len() > 20, "found {} files", files.len());
        let mut found = Vec::new();
        for file in files {
            if file.ends_with("time_util.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("read source");
            found.extend(raw_timeouts(&file.display().to_string(), &text));
        }
        assert!(
            found.is_empty(),
            "use crate::time_util::timeout, not asupersync's time::timeout:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn raw_timeouts_finds_each_form() {
        let text = "\
let a = time::timeout(cx.now(), d, f).await;
let b = asupersync::time::timeout_at(t, f).await;
use asupersync::time::{Interval,
    timeout};
use asupersync::time::{self, TimeoutFuture};
// time::timeout in a comment
let c = crate::time_util::timeout(cx, d, f).await;
let d = time::sleep(cx.now(), d).await;
use asupersync::time::{sleep_until, interval};
";
        let found = raw_timeouts("x.rs", text);
        let lines: Vec<&str> = found
            .iter()
            .map(|f| f.split(": ").next().unwrap_or_default())
            .collect();
        assert_eq!(lines, ["x.rs:1", "x.rs:2", "x.rs:3", "x.rs:5"], "{found:?}");
    }
}
