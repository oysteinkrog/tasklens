//! `tasklens --bench <scene> --duration 30 --warmup 5 --json <out>` (ADR-014, table (c),
//! APP-052): runs one bench scene.
//!
//! Compiled only with the `bench` feature. tl-app supplies the clocks tl-ui cannot read, starts
//! the real engine for the `bridge` scene only, runs the scene in its GPUI application and shuts
//! the engine down after it (ADR-010). `main.rs` sets up logging, the panic hook and the
//! arguments before calling [`run`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustix::time::{ClockId, clock_gettime};
use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::demand::{CpuDemand, Demand};
use tl_ui::bench::{BenchClocks, BenchOptions, BenchScene};
use tracing::{info, warn};

use crate::paths::Paths;

/// `--bench`, `--duration`, `--warmup` and `--json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchArgs {
    pub scene: String,
    pub duration_s: u32,
    pub warmup_s: u32,
    pub json: Option<PathBuf>,
}

/// Runs the scene and returns the process exit code: 0 on a finished run, 1 on a scene
/// failure, 2 on an unknown scene.
pub fn run(args: BenchArgs) -> i32 {
    let scene = match parse_scene(&args.scene) {
        Ok(scene) => scene,
        Err(text) => {
            eprintln!("{text}");
            return 2;
        }
    };

    if let Some(text) = lock_refusal(session_locked()) {
        eprintln!("bench failed: {text}");
        return 1;
    }

    let engine = if scene == BenchScene::Bridge {
        match start_engine() {
            Ok(engine) => Some(engine),
            Err(text) => {
                eprintln!("bench failed: {text}");
                return 1;
            }
        }
    } else {
        None
    };
    let real = engine
        .as_ref()
        .map(|e| Arc::new(e.clone()) as Arc<dyn EngineClient>);

    let opts = BenchOptions {
        duration_s: args.duration_s,
        warmup_s: args.warmup_s,
        json: args.json,
    };
    let clocks = BenchClocks {
        thread_cpu_ns,
        process_cpu_ns,
    };
    let code = match tl_ui::bench::run_app(scene, opts, clocks, real) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("bench failed: {e}");
            1
        }
    };

    if let Some(engine) = engine {
        let report = engine.shutdown(Duration::from_millis(1500));
        info!(target: "tasklens::perf", drained = report.drained, torn_down = report.torn_down,
            elapsed_ms = report.elapsed.as_millis() as u64, "bench engine shutdown");
    }
    code
}

/// The scene named `name`, or the error line for an unknown name.
fn parse_scene(name: &str) -> Result<BenchScene, String> {
    BenchScene::from_arg(name).ok_or_else(|| {
        format!("unknown bench scene \"{name}\"; expected perf-cpu, table-2000, bridge or static")
    })
}

/// Why a run must not start: a locked session gets no frame callbacks from the compositor, so
/// every frame figure would be 0. `None` when unlocked or unknown.
fn lock_refusal(locked: Option<bool>) -> Option<&'static str> {
    (locked == Some(true)).then_some("session is locked; frames will not be delivered")
}

/// logind's `LockedHint` over this user's graphical sessions (wayland or x11): true when any is
/// locked. `None` when logind cannot be asked; the run then goes ahead with a warning.
fn session_locked() -> Option<bool> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;

    const LOGIN1: &str = "org.freedesktop.login1";
    let ask = || -> zbus::Result<bool> {
        let conn = Connection::system()?;
        let manager = Proxy::new(
            &conn,
            LOGIN1,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )?;
        let sessions: Vec<(String, u32, String, String, OwnedObjectPath)> =
            manager.call("ListSessions", &())?;
        let uid = rustix::process::getuid().as_raw();
        let mut locked = false;
        for (_, session_uid, _, _, path) in sessions {
            if session_uid != uid {
                continue;
            }
            let session = Proxy::new(&conn, LOGIN1, path, "org.freedesktop.login1.Session")?;
            let kind: String = session.get_property("Type")?;
            if (kind == "wayland" || kind == "x11") && session.get_property::<bool>("LockedHint")? {
                locked = true;
            }
        }
        Ok(locked)
    };
    match ask() {
        Ok(locked) => Some(locked),
        Err(e) => {
            warn!(target: "tasklens::perf", "cannot read the session lock state: {e}");
            None
        }
    }
}

/// The real engine for `bridge`, with the demand the Performance tab's CPU page sends (ADR-008):
/// a surface visible, per-core CPU and fast (10 Hz) CPU sampling. No history directory and no
/// helper: the run records nothing.
fn start_engine() -> Result<EngineHandle, String> {
    let paths =
        Paths::from_env().map_err(|e| format!("cannot resolve the state directory: {e}"))?;
    let config = EngineConfig::new(EngineMode::Full, paths.state_dir);
    let engine = Engine::start(config, EngineHooks::default())
        .map_err(|e| format!("cannot start the engine: {e}"))?;
    engine.set_demand(Demand {
        any_surface_visible: true,
        cpu: CpuDemand::PerCore,
        cpu_fast: true,
        ..Demand::default()
    });
    info!(target: "tasklens::perf", "bench engine started");
    Ok(engine)
}

/// `CLOCK_THREAD_CPUTIME_ID` of the calling thread, in ns.
fn thread_cpu_ns() -> u64 {
    ns(clock_gettime(ClockId::ThreadCPUTime))
}

/// `CLOCK_PROCESS_CPUTIME_ID` (utime + stime of the process), in ns.
fn process_cpu_ns() -> u64 {
    ns(clock_gettime(ClockId::ProcessCPUTime))
}

fn ns(t: rustix::time::Timespec) -> u64 {
    match (u64::try_from(t.tv_sec), u64::try_from(t.tv_nsec)) {
        (Ok(s), Ok(n)) => s * 1_000_000_000 + n,
        _ => {
            warn!(target: "tasklens::perf", "negative clock value");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn unknown_scene_exit_2() {
        assert_eq!(
            parse_scene("charts"),
            Err(
                "unknown bench scene \"charts\"; expected perf-cpu, table-2000, bridge or static"
                    .to_owned()
            )
        );
        let code = run(BenchArgs {
            scene: "charts".to_owned(),
            duration_s: 30,
            warmup_s: 5,
            json: None,
        });
        assert_eq!(code, 2, "no window is opened for an unknown scene");
        assert_eq!(parse_scene("perf-cpu"), Ok(BenchScene::PerfCpu));
        assert_eq!(parse_scene("table-2000"), Ok(BenchScene::Table2000));
    }

    #[test]
    fn locked_session_refused() {
        assert_eq!(
            lock_refusal(Some(true)),
            Some("session is locked; frames will not be delivered")
        );
        assert_eq!(lock_refusal(Some(false)), None);
        assert_eq!(
            lock_refusal(None),
            None,
            "unknown state does not block a run"
        );
    }

    #[test]
    fn thread_cpu_clock_moves() {
        let before = thread_cpu_ns();
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(10) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        let delta = thread_cpu_ns() - before;
        assert!(
            (5_000_000..=50_000_000).contains(&delta),
            "delta {delta} ns"
        );
        assert!(process_cpu_ns() >= delta);
    }
}
