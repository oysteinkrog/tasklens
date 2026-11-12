//! GPU power baseline for the bench (E01-023, ADR-014 measurement tools, open question 7).
//!
//! `#[ignore]`, owner machine only. Run it with no bench or test tasklens running (the installed
//! copy may run) and a `perf` probe (ADR-002):
//!
//! ```text
//! cargo build --profile perf -p tl-gpu-probe
//! TASKLENS_GPU_PROBE=$PWD/target/perf/tl-gpu-probe \
//!     cargo test -p tl-app --test real_gpu_baseline -- --ignored --nocapture
//! ```
//!
//! It samples NVML power and utilization at 1 Hz for 30 s (samples at 0 s to 29 s, window end at
//! 30 s) through the same client the bench uses and prints the means, with the same rules as the
//! bench report: power is the sum over the GPUs, utilization the highest GPU. Report only, no
//! gate.

use std::path::Path;
use std::time::{Duration, Instant};

use tl_engine::gpu::client::ProbeClient;
use tl_engine::gpu::{PROBE_ENV, PROBE_NAME};

const SAMPLES: u32 = 30;
const PERIOD: Duration = Duration::from_secs(1);
/// The owner's installed copy, which keeps running during the baseline.
const INSTALLED: &str = "/usr/bin/tasklens";

/// Pids of processes named `tasklens`, split into the installed copy and every other binary (a
/// bench or test build).
fn tasklens_pids() -> (Vec<u32>, Vec<u32>) {
    let (mut installed, mut other) = (Vec::new(), Vec::new());
    for entry in std::fs::read_dir("/proc").expect("read /proc").flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim_end() != "tasklens" {
            continue;
        }
        match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(exe) if exe == Path::new(INSTALLED) => installed.push(pid),
            _ => other.push(pid),
        }
    }
    (installed, other)
}

/// utime + stime of `pid` in clock ticks; the name in field 2 can hold spaces.
fn ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

#[test]
#[ignore = "owner machine: an NVIDIA GPU, a perf tl-gpu-probe and no bench or test tasklens"]
fn real_gpu_baseline() {
    // Leader rule (2026-11-12): "tasklens closed" means no bench or test instance runs. The
    // owner's installed copy (GUI and bridge) and tasklens-engine.service keep running, and the
    // test prints their pids so the results can name them.
    let (installed, other) = tasklens_pids();
    assert!(
        other.is_empty(),
        "close the bench or test tasklens first; running pids: {other:?}"
    );
    eprintln!("installed tasklens pids left running: {installed:?}");

    let probe = tl_engine::gpu::locate().unwrap_or_else(|| {
        panic!("set {PROBE_ENV} to a perf build, for example target/perf/{PROBE_NAME}")
    });
    let mut client = ProbeClient::start(&probe).expect("probe starts");
    assert!(
        client.info.is_some(),
        "Hello is not Ready: {:?}",
        client.init_failed
    );
    let pid = client.pid();

    let ticks_at = ticks(pid).expect("probe stat");
    let start = Instant::now();
    let (mut power_sum, mut power_n) = (0.0, 0u32);
    let (mut util_sum, mut util_n) = (0.0, 0u32);
    for n in 1..=SAMPLES {
        let reply = client.sample(false).expect("sample");
        let power: Vec<u32> = reply
            .devices
            .iter()
            .filter_map(|d| d.power_mw.ok())
            .collect();
        if !power.is_empty() {
            power_sum += power.iter().map(|&mw| f64::from(mw)).sum::<f64>() / 1000.0;
            power_n += 1;
        }
        if let Some(util) = reply.devices.iter().filter_map(|d| d.util_pct.ok()).max() {
            util_sum += f64::from(util);
            util_n += 1;
        }
        // Sleep after the last sample too: each sample stands for one 1 s interval, so the
        // window ends at 30 s, as a 30 s bench window does.
        std::thread::sleep((start + PERIOD * n).saturating_duration_since(Instant::now()));
    }
    let wall = start.elapsed().as_secs_f64();
    let dticks = ticks(pid).expect("probe stat").saturating_sub(ticks_at);
    client.shutdown();

    assert_eq!(power_n, SAMPLES, "every sample has a power reading");
    assert_eq!(util_n, SAMPLES, "every sample has a utilization reading");
    let probe_cpu = 100.0 * dticks as f64 / rustix::param::clock_ticks_per_second() as f64 / wall;
    eprintln!(
        "GPU baseline over {SAMPLES} samples at 1 Hz ({wall:.1} s): power {:.2} W, utilization {:.1} %, probe CPU {probe_cpu:.3} %",
        power_sum / f64::from(power_n),
        util_sum / f64::from(util_n),
    );
}
