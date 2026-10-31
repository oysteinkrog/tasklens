//! ADR-018 Budgets on the real engine, measured on the machine that runs them. Both tests are
//! `#[ignore]`: they take minutes, read this host's own /proc and /sys, and their numbers depend
//! on what else runs. Run them one at a time on a quiet machine, with `--nocapture` to see the
//! numbers:
//!
//! ```text
//! cargo test -p tl-server --test budgets -- --ignored --nocapture --test-threads=1
//! ```
//!
//! - `idle_service_cpu_is_within_10_percent_of_the_hidden_window_engine`: the service with no
//!   client uses at most 10% more CPU time than today's in-process engine with the window hidden
//!   to the tray (leader decision on tasklens-q4x76.15, Q3 A).
//! - `wire_bytes_of_the_real_engine_on_this_host`: the bytes a Processes tab session and a
//!   hidden host send, from the snapshots this host's real engine publishes to a session. A cross-check of the synthetic
//!   fixture in `tl-remote/tests/budgets.rs`; it prints and asserts nothing.

#![allow(clippy::unwrap_used)]
// The numbers are the result: the lane and the leader read them with `--nocapture`.
#![allow(clippy::print_stdout)]

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_engine::client_session::ClientSessionId;
use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode, SessionPort};
use tl_model::client::EngineClient;
use tl_model::demand::{ColumnSet, CpuDemand, Demand};
use tl_model::ids::ColumnId;
use tl_model::remote_proto::{ClientMsg, ServerMsg, SnapshotFrame, encode_message};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_remote::delta::DeltaEncoder;
use tl_remote::envelope::FrameWriter;

const BIN: &str = env!("CARGO_BIN_EXE_tasklens-engine");
/// The privileged helper, as `tasklens` and `tasklens-engine` look for it.
const HELPER_PATH: &str = "/usr/lib/tasklens/tl-helper";
/// Each engine runs this long before its CPU time is counted: history open, first sweeps.
const WARM_UP: Duration = Duration::from_secs(10);
/// CPU time is counted over this long (leader decision: 60 s each).
const MEASURE: Duration = Duration::from_secs(60);
/// The wire measurement waits this long first, so every 1 Hz ring (602 samples) is full and the
/// complete frame is as large as it gets.
const RINGS_FULL: Duration = Duration::from_secs(610);

/// What the window asks for when it is hidden to the tray: `DemandAggregator::merged` with no
/// visible part keeps only `recording` (off by default) and sets `device_history` from
/// `history.device_metrics` (crates/tl-ui/src/demand.rs).
fn hidden_window_demand() -> Demand {
    Demand {
        device_history: EngineSettings::default().history.device_metrics,
        ..Demand::default()
    }
}

/// A host tab that is not shown: the title bar value only (ADR-018 Sessions).
fn hidden_host_demand() -> Demand {
    Demand {
        cpu: CpuDemand::Total,
        ..Demand::default()
    }
}

/// The Processes tab: the process table with the default columns
/// (`[columns.processes]` default, crates/tl-ui/src/pages/processes/cells.rs), the title bar CPU
/// value and the sidebar series (CPU, memory, disks, networks, GPUs).
fn processes_tab_demand() -> Demand {
    let mut columns = ColumnSet::EMPTY;
    for c in [
        ColumnId::Name,
        ColumnId::User,
        ColumnId::Pid,
        ColumnId::Cpu,
        ColumnId::Memory,
        ColumnId::Disk,
        ColumnId::Network,
        ColumnId::Gpu,
        ColumnId::Description,
    ] {
        columns.insert(c);
    }
    Demand {
        any_surface_visible: true,
        cpu: CpuDemand::Total,
        memory: true,
        disks: true,
        nets: true,
        gpus: true,
        process_table: true,
        process_columns: columns,
        ..Demand::default()
    }
}

/// utime + stime of `pid` in seconds, from /proc/<pid>/stat fields 14 and 15. All threads.
fn cpu_seconds(pid: &str) -> f64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // Fields after the command name, which may hold spaces; the first is field 3.
    let rest: Vec<&str> = stat[stat.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .collect();
    let ticks: u64 = rest[11].parse::<u64>().unwrap() + rest[12].parse::<u64>().unwrap();
    let hz = clock_ticks();
    ticks as f64 / hz as f64
}

/// Clock ticks per second, `getconf CLK_TCK`.
fn clock_ticks() -> u64 {
    let out = Command::new("getconf").arg("CLK_TCK").output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// CPU seconds `pid` uses over `MEASURE`, with the load average before and after.
fn measure(pid: &str) -> (f64, String, String) {
    let before = loadavg();
    let start = cpu_seconds(pid);
    std::thread::sleep(MEASURE);
    let used = cpu_seconds(pid) - start;
    (used, before, loadavg())
}

fn start_engine(state: &Path) -> EngineHandle {
    let mut config = EngineConfig::new(EngineMode::Full, state.to_path_buf())
        .with_history_dir(state.join("history"));
    if Path::new(HELPER_PATH).exists() {
        config = config.with_helper_path(HELPER_PATH.into());
    }
    let engine = Engine::start(config, EngineHooks::default()).unwrap();
    // As `tasklens` does after the start, with default settings.
    engine.set_settings(EngineSettings::default());
    engine
}

/// Kills the child if the test ends early. Only the child this test started.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "takes about 3 minutes; the leader runs it on a quiet machine"]
fn idle_service_cpu_is_within_10_percent_of_the_hidden_window_engine() {
    let tmp = tempfile::tempdir().unwrap();

    // Today: the window's in-process engine with the window hidden. This test process runs
    // nothing else, so its CPU time is the engine's.
    let window_state = tmp.path().join("window-state");
    std::fs::create_dir(&window_state).unwrap();
    let engine = start_engine(&window_state);
    engine.set_demand(hidden_window_demand());
    std::thread::sleep(WARM_UP);
    let (window, w_before, w_after) = measure("self");
    assert!(engine.shutdown().clean());

    // The service with no client, on the same defaults. Its own state and runtime directories,
    // so it never meets an installed service's lock or socket; the rest of the environment,
    // the session bus included, is this test's, as the in-process engine had it.
    let state = tmp.path().join("state");
    let runtime = tmp.path().join("run");
    for dir in [&state, &runtime] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::set_permissions(&runtime, Permissions::from_mode(0o700)).unwrap();
    let child = Command::new(BIN)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("TASKLENS_LOG", "warn")
        .env_remove("NOTIFY_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut guard = Guard(child);
    let pid = guard.0.id().to_string();
    let socket: PathBuf = runtime.join("tasklens/engine.sock");
    let begin = Instant::now();
    while !socket.exists() {
        assert!(
            begin.elapsed() < Duration::from_secs(10),
            "tasklens-engine did not bind its socket within 10 s"
        );
        assert!(
            guard.0.try_wait().unwrap().is_none(),
            "tasklens-engine exited"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(WARM_UP);
    let (service, s_before, s_after) = measure(&pid);
    let killed = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(killed.success());
    let begin = Instant::now();
    while guard.0.try_wait().unwrap().is_none() {
        assert!(
            begin.elapsed() < Duration::from_secs(10),
            "tasklens-engine did not stop within 10 s of SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let secs = MEASURE.as_secs_f64();
    println!(
        "hidden window engine: {window:.2} s CPU in {secs:.0} s ({:.3}%), load {w_before} -> {w_after}",
        100.0 * window / secs
    );
    println!(
        "service, no client:   {service:.2} s CPU in {secs:.0} s ({:.3}%), load {s_before} -> {s_after}",
        100.0 * service / secs
    );
    println!("ratio service / window: {:.3}", service / window);
    assert!(
        service <= window * 1.10,
        "the service used {service:.2} s of CPU, more than 110% of the hidden window engine's {window:.2} s"
    );
}

/// Bytes on the wire for one session: every snapshot frame with its envelope, and the client's
/// `Ack` frames.
#[derive(Default)]
struct Wire {
    frames: usize,
    server: usize,
    acks: usize,
    complete: usize,
    /// Uncompressed postcard bytes per part, summed over the delta frames.
    parts: Vec<(&'static str, usize)>,
    /// Delta frames that carry each part, in the order of `parts`.
    carried: Vec<usize>,
    /// Uncompressed postcard bytes of each part in the complete frame.
    whole: Vec<(&'static str, usize)>,
}

/// The uncompressed bytes of each part of `frame`: the frame with only that part set, less the
/// frame with none.
fn part_sizes(frame: &SnapshotFrame) -> Vec<(&'static str, usize)> {
    let len = |f: &SnapshotFrame| {
        let mut bytes = Vec::new();
        encode_message(f, &mut bytes).unwrap();
        bytes.len()
    };
    let empty = SnapshotFrame::unchanged(
        frame.frame,
        frame.base,
        frame.epoch,
        frame.boot_id,
        frame.published_ns,
    );
    let none = len(&empty);
    let mut out = Vec::new();
    macro_rules! parts {
        ($($f:ident),*) => {$(
            let mut one = empty.clone();
            one.$f = frame.$f.clone();
            out.push((stringify!($f), len(&one) - none));
        )*};
    }
    parts!(
        system,
        series,
        processes,
        table,
        connections,
        services,
        startup,
        inventory,
        devices,
        health,
        windows,
        details,
        history,
        process_series,
        device_details,
        device_history
    );
    out
}

/// Runs one session for `MEASURE`: one frame per second from the session's newest publish,
/// acknowledged at once, the first frame complete.
fn session_bytes(port: &SessionPort) -> (Wire, Arc<Snapshot>) {
    let mut encoder = DeltaEncoder::default();
    let mut writer = FrameWriter::new().unwrap();
    let mut wire = Wire::default();
    let begin = Instant::now();
    let mut last = port.latest();
    for second in 0..MEASURE.as_secs() {
        std::thread::sleep(
            (begin + Duration::from_secs(second)).saturating_duration_since(Instant::now()),
        );
        last = port.latest();
        let now_ns = begin.elapsed().as_nanos() as u64;
        let built = encoder.next_frame(&last, now_ns).unwrap().unwrap();
        let frame = built.frame;
        let number = frame.frame;
        let mut bytes = Vec::new();
        writer
            .encode_server(&ServerMsg::Snapshot(Box::new(frame.clone())), &mut bytes)
            .unwrap();
        if frame.base == 0 {
            wire.complete += bytes.len();
            wire.whole = part_sizes(&frame);
        } else {
            let sizes = part_sizes(&frame);
            if wire.parts.is_empty() {
                wire.carried = sizes.iter().map(|_| 0).collect();
                wire.parts = sizes.iter().map(|(name, _)| (*name, 0)).collect();
            }
            for ((sum, carried), (_, n)) in wire.parts.iter_mut().zip(&mut wire.carried).zip(sizes)
            {
                sum.1 += n;
                *carried += usize::from(n > 0);
            }
        }
        wire.server += bytes.len();
        wire.frames += 1;
        let mut ack = Vec::new();
        writer
            .encode_client(&ClientMsg::Ack { frame: number }, &mut ack)
            .unwrap();
        wire.acks += ack.len();
        encoder.on_ack(number).unwrap();
    }
    (wire, last)
}

fn report(name: &str, wire: &Wire, last: &Snapshot) {
    let secs = MEASURE.as_secs_f64();
    let total = wire.server + wire.acks;
    println!(
        "{name}: {} frames, {} B server ({} B in the complete frame), {} B acks, {:.0} B/s = {:.2} KiB/s",
        wire.frames,
        wire.server,
        wire.complete,
        wire.acks,
        total as f64 / secs,
        total as f64 / secs / 1024.0
    );
    println!(
        "  {} processes, {} table rows, {} groups, {} series, {} slow series",
        last.processes.records.len(),
        last.table.rows.len(),
        last.table.groups.len(),
        last.series.series.len(),
        last.series.slow.len()
    );
    println!(
        "  {} units, {} windows, {} catalog devices, {} inventory disks, {} inventory network adapters",
        last.services.units.len(),
        last.windows.windows.len(),
        last.devices.entries.len(),
        last.inventory.disks.len(),
        last.inventory.network.len()
    );
    let deltas = (wire.frames - 1).max(1);
    for ((part, n), carried) in wire.parts.iter().zip(&wire.carried) {
        let whole = wire
            .whole
            .iter()
            .find(|(name, _)| name == part)
            .map_or(0, |(_, w)| *w);
        if *n > 0 || whole > 0 {
            println!(
                "  {part}: {whole} B in the complete frame; in {carried} of {deltas} delta frames, {} B per delta frame, uncompressed",
                n / deltas
            );
        }
    }
}

#[test]
#[ignore = "takes about 12 minutes; prints this host's numbers"]
fn wire_bytes_of_the_real_engine_on_this_host() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = start_engine(tmp.path());
    // The bytes are those of a connected session, which gets only the parts its demand asks
    // for; the local window is hidden, as in the service.
    engine.set_demand(hidden_window_demand());
    let port = engine.open_session(ClientSessionId(1)).unwrap();
    port.set_demand(processes_tab_demand());
    std::thread::sleep(RINGS_FULL);
    let (wire, last) = session_bytes(&port);
    report("Processes tab session", &wire, &last);
    port.set_demand(hidden_host_demand());
    std::thread::sleep(Duration::from_secs(5));
    let (wire, last) = session_bytes(&port);
    report("hidden host", &wire, &last);
    drop(port);
    assert!(engine.shutdown().clean());
}
