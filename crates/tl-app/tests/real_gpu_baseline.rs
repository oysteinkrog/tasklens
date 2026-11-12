//! GPU power baseline for the bench (E01-023, ADR-014 measurement tools, open question 7).
//!
//! `#[ignore]`, owner machine only. Run it with tasklens closed and a release probe:
//!
//! ```text
//! cargo build --release -p tl-gpu-probe
//! TASKLENS_GPU_PROBE=$PWD/target/release/tl-gpu-probe \
//!     cargo test -p tl-app --test real_gpu_baseline -- --ignored --nocapture
//! ```
//!
//! It samples NVML power and utilization at 1 Hz for 30 s through the same client the bench
//! uses and prints the means, with the same rules as the bench report: power is the sum over the
//! GPUs, utilization the highest GPU. Report only, no gate.

use std::process::Command;
use std::time::{Duration, Instant};

use tl_engine::gpu::client::ProbeClient;
use tl_engine::gpu::{PROBE_ENV, PROBE_NAME};

const SAMPLES: u32 = 30;
const PERIOD: Duration = Duration::from_secs(1);

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
#[ignore = "owner machine: an NVIDIA GPU, a release tl-gpu-probe and tasklens closed"]
fn real_gpu_baseline() {
    let running = Command::new("pgrep")
        .args(["-x", "tasklens"])
        .output()
        .expect("pgrep runs");
    assert!(
        running.stdout.is_empty(),
        "close tasklens first; running pids: {}",
        String::from_utf8_lossy(&running.stdout).trim()
    );

    let probe = tl_engine::gpu::locate().unwrap_or_else(|| {
        panic!("set {PROBE_ENV} to a release build, for example target/release/{PROBE_NAME}")
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
        if n < SAMPLES {
            std::thread::sleep((start + PERIOD * n).saturating_duration_since(Instant::now()));
        }
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
