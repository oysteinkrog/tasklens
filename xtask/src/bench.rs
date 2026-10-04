//! `cargo xtask bench`: runs every bench scene a few times, keeps the median run per scene,
//! writes `target/bench/<date>-<sha>.json` and prints a Markdown table (ADR-014, UI-PRF-053).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};

/// Scenes run when no `--scene` is given, in this order.
const DEFAULT_SCENES: [&str; 4] = ["perf-cpu", "table-2000", "bridge", "static"];
const DEFAULT_RUNS: u32 = 3;
/// A gated metric more than this many percent worse than the old summary fails `--compare`.
const REGRESSION_LIMIT_PCT: f64 = 20.0;

/// Gated metrics, as paths into a bench report. Lower is better for all of them.
const GATED_METRICS: [&str; 6] = [
    "frame_cpu_ms.p50",
    "frame_cpu_ms.p99",
    "missed_frames",
    "process_cpu_pct",
    "rss_mb",
    "bridge_latency_ms.p99",
];

#[derive(Debug)]
pub struct BenchCmd {
    pub scenes: Vec<String>,
    pub runs: u32,
    pub compare: Option<PathBuf>,
}

/// Parses `[--scene <name>]... [--runs <n>] [--compare <old.json>]`.
pub fn parse_args(args: &[String]) -> anyhow::Result<BenchCmd> {
    let mut scenes = Vec::new();
    let mut runs = DEFAULT_RUNS;
    let mut compare = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scene" => match args.next() {
                Some(name) => scenes.push(name.clone()),
                None => bail!("bench: --scene needs a scene name"),
            },
            "--runs" => match args.next().and_then(|n| n.parse::<u32>().ok()) {
                Some(n) if n > 0 => runs = n,
                _ => bail!("bench: --runs needs a positive integer"),
            },
            "--compare" => match args.next() {
                Some(path) => compare = Some(PathBuf::from(path)),
                None => bail!("bench: --compare needs a file path"),
            },
            other => bail!("bench: unknown argument: {other}"),
        }
    }
    if scenes.is_empty() {
        scenes = DEFAULT_SCENES.iter().map(|s| (*s).to_owned()).collect();
    }
    Ok(BenchCmd {
        scenes,
        runs,
        compare,
    })
}

pub fn run(cmd: BenchCmd) -> anyhow::Result<()> {
    let root = workspace_root()?;
    build(&root)?;

    let tmp = root.join("target/bench/tmp");
    std::fs::create_dir_all(&tmp)
        .with_context(|| format!("bench: cannot create {}", tmp.display()))?;
    let binary = root.join("target/release/tasklens");
    let reports = run_scenes(&cmd.scenes, cmd.runs, &tmp, |scene, out| {
        Command::new(&binary)
            .args([
                "--bench",
                scene,
                "--duration",
                "30",
                "--warmup",
                "5",
                "--json",
            ])
            .arg(out)
            .current_dir(&root)
            .status()
            .with_context(|| format!("bench: cannot start {}", binary.display()))
    })?;

    let summary = summarize(&utc_date(SystemTime::now()), &git_sha(&root)?, reports)?;
    let out = root.join(format!(
        "target/bench/{}-{}.json",
        summary["date"].as_str().unwrap_or_default(),
        summary["git_sha"].as_str().unwrap_or_default()
    ));
    std::fs::write(&out, serde_json::to_string_pretty(&summary)?)
        .with_context(|| format!("bench: cannot write {}", out.display()))?;
    println!("{}", table(&summary));
    println!("Summary written to {}", out.display());

    if let Some(old_path) = cmd.compare {
        let text = std::fs::read_to_string(&old_path)
            .with_context(|| format!("bench: cannot read {}", old_path.display()))?;
        let old: Value = serde_json::from_str(&text)
            .with_context(|| format!("bench: {} is not valid JSON", old_path.display()))?;
        let result = compare(&old, &summary);
        for line in &result.lines {
            println!("{line}");
        }
        if let Some((scene, metric)) = result.regression {
            bail!("bench: regression above 20% in {scene} {metric}");
        }
    }
    Ok(())
}

fn workspace_root() -> anyhow::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .context("bench: xtask has no parent directory")
}

/// Builds the bench binary once. With `TL_BUILD_WRAP=1` the build runs through `heavy-build`
/// (R-160 OI-10).
fn build(root: &Path) -> anyhow::Result<()> {
    let cargo_args = ["build", "--release", "-p", "tl-app", "--features", "bench"];
    let mut cmd = if std::env::var("TL_BUILD_WRAP").as_deref() == Ok("1") {
        let mut c = Command::new("heavy-build");
        c.arg("cargo");
        c
    } else {
        Command::new("cargo")
    };
    let status = cmd
        .args(cargo_args)
        .current_dir(root)
        .status()
        .context("bench: cannot start the build")?;
    if !status.success() {
        bail!("bench: build failed with exit code {}", exit_code(status));
    }
    Ok(())
}

/// Runs each scene `runs` times, one at a time, and reads the report each run wrote.
/// `run_one` gets the scene name and the report path. Returns `(scene, reports)` in scene order.
fn run_scenes(
    scenes: &[String],
    runs: u32,
    tmp: &Path,
    mut run_one: impl FnMut(&str, &Path) -> anyhow::Result<ExitStatus>,
) -> anyhow::Result<Vec<(String, Vec<Value>)>> {
    let mut all = Vec::with_capacity(scenes.len());
    for scene in scenes {
        let mut reports = Vec::with_capacity(runs as usize);
        for i in 1..=runs {
            let out = tmp.join(format!("{scene}-{i}.json"));
            let status = run_one(scene, &out)?;
            if !status.success() {
                bail!(
                    "bench: {scene} run {i} failed with exit code {}",
                    exit_code(status)
                );
            }
            let text = std::fs::read_to_string(&out)
                .with_context(|| format!("bench: cannot read {}", out.display()))?;
            let report: Value = serde_json::from_str(&text)
                .with_context(|| format!("bench: {} is not valid JSON", out.display()))?;
            reports.push(report);
        }
        all.push((scene.clone(), reports));
    }
    Ok(all)
}

fn exit_code(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "none (killed by a signal)".to_owned(), |c| c.to_string())
}

/// The metric the median run is picked by: `process_cpu_pct` for `static`, else
/// `frame_cpu_ms.p99`.
fn median_key(scene: &str) -> &'static str {
    if scene == "static" {
        "process_cpu_pct"
    } else {
        "frame_cpu_ms.p99"
    }
}

/// Index of the median run by `median_key(scene)`. With an even count it takes the lower middle.
fn median_index(scene: &str, reports: &[Value]) -> anyhow::Result<usize> {
    let key = median_key(scene);
    let mut keyed = Vec::with_capacity(reports.len());
    for (i, report) in reports.iter().enumerate() {
        let v = metric(report, key)
            .with_context(|| format!("bench: {scene} run {} has no {key}", i + 1))?;
        keyed.push((v, i));
    }
    if keyed.is_empty() {
        bail!("bench: {scene} has no runs");
    }
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keyed[(keyed.len() - 1) / 2].1)
}

/// Builds `{ date, git_sha, scenes: { <scene>: { median, runs } } }`.
fn summarize(
    date: &str,
    git_sha: &str,
    reports: Vec<(String, Vec<Value>)>,
) -> anyhow::Result<Value> {
    let mut scenes = Map::new();
    for (scene, runs) in reports {
        let median = runs[median_index(&scene, &runs)?].clone();
        scenes.insert(scene, json!({ "median": median, "runs": runs }));
    }
    Ok(json!({ "date": date, "git_sha": git_sha, "scenes": scenes }))
}

/// Reads a number at a dotted path such as `frame_cpu_ms.p99`. `None` when missing or null.
fn metric(report: &Value, path: &str) -> Option<f64> {
    path.split('.')
        .try_fold(report, |v, key| v.get(key))
        .and_then(Value::as_f64)
}

/// Markdown table, one row per scene, values from the median run.
fn table(summary: &Value) -> String {
    let mut out = String::from(
        "| Scene | frame CPU p50 ms | p99 ms | missed | process CPU % | RSS MB | stalls | bridge p99 ms |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    let empty = Map::new();
    let scenes = summary["scenes"].as_object().unwrap_or(&empty);
    for (scene, entry) in scenes {
        let median = &entry["median"];
        let cell =
            |path: &str| metric(median, path).map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"));
        out.push_str(&format!(
            "| {scene} | {} | {} | {} | {} | {} | {} | {} |\n",
            cell("frame_cpu_ms.p50"),
            cell("frame_cpu_ms.p99"),
            cell("missed_frames"),
            cell("process_cpu_pct"),
            cell("rss_mb"),
            cell("stalls"),
            cell("bridge_latency_ms.p99"),
        ));
    }
    out
}

struct Comparison {
    lines: Vec<String>,
    /// First `(scene, metric)` that got more than 20% worse.
    regression: Option<(String, String)>,
}

/// Compares the median run of every scene in both summaries. A metric missing or null on
/// either side is skipped. An old value of 0 with a new value above 0 counts as a regression.
fn compare(old: &Value, new: &Value) -> Comparison {
    let mut lines = Vec::new();
    let mut regression = None;
    let empty = Map::new();
    let new_scenes = new["scenes"].as_object().unwrap_or(&empty);
    for (scene, entry) in new_scenes {
        let Some(old_median) = old["scenes"].get(scene).map(|e| &e["median"]) else {
            lines.push(format!("{scene}: not in the old summary"));
            continue;
        };
        let new_median = &entry["median"];
        for path in GATED_METRICS {
            let (Some(o), Some(n)) = (metric(old_median, path), metric(new_median, path)) else {
                continue;
            };
            let (text, worse) = if o == 0.0 {
                if n > 0.0 {
                    ("new, was 0".to_owned(), true)
                } else {
                    ("+0.00%".to_owned(), false)
                }
            } else {
                let pct = (n - o) / o * 100.0;
                (format!("{pct:+.2}%"), pct > REGRESSION_LIMIT_PCT)
            };
            lines.push(format!("{scene} {path}: {o:.2} -> {n:.2} ({text})"));
            if worse && regression.is_none() {
                regression = Some((scene.clone(), path.to_owned()));
            }
        }
    }
    Comparison { lines, regression }
}

/// `git rev-parse --short=12 HEAD`, with `-dirty` when `git status --porcelain` prints anything.
fn git_sha(root: &Path) -> anyhow::Result<String> {
    let rev = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(root)
        .output()
        .context("bench: cannot run git rev-parse")?;
    if !rev.status.success() {
        bail!(
            "bench: git rev-parse failed with exit code {}",
            exit_code(rev.status)
        );
    }
    let mut sha = String::from_utf8_lossy(&rev.stdout).trim().to_owned();
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .context("bench: cannot run git status")?;
    if !status.status.success() {
        bail!(
            "bench: git status failed with exit code {}",
            exit_code(status.status)
        );
    }
    if !status.stdout.iter().all(u8::is_ascii_whitespace) {
        sha.push_str("-dirty");
    }
    Ok(sha)
}

/// UTC date `YYYY-MM-DD` of `now`.
fn utc_date(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to `(year, month, day)` in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // d is in 1..=31 and m in 1..=12, so the casts cannot truncate.
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(scene: &str, p50: f64, p99: f64, cpu: f64) -> Value {
        json!({
            "scene": scene,
            "frames": 4320,
            "frame_cpu_ms": { "p50": p50, "p99": p99, "max": p99 * 2.0 },
            "missed_frames": 0,
            "long_missed_frames": 0,
            "process_cpu_pct": cpu,
            "probe_cpu_pct": 0.1,
            "rss_mb": 120.0,
            "pss_mb": 110.0,
            "stalls": 0,
            "bridge_latency_ms": null,
            "gpu": null
        })
    }

    fn summary_with_p99(p99: f64) -> Value {
        summarize(
            "2026-10-04",
            "abc",
            vec![(
                "perf-cpu".to_owned(),
                vec![report("perf-cpu", 0.5, p99, 20.0)],
            )],
        )
        .unwrap()
    }

    #[test]
    fn parse_args_defaults() {
        let cmd = parse_args(&[]).unwrap();
        assert_eq!(cmd.scenes, DEFAULT_SCENES);
        assert_eq!(cmd.scenes.len(), 4);
        assert_eq!(cmd.runs, 3);
        assert!(cmd.compare.is_none());
    }

    #[test]
    fn parse_args_rejects_bad_runs() {
        for bad in ["x", "0", "-1"] {
            let err = parse_args(&["--runs".to_owned(), bad.to_owned()]).unwrap_err();
            assert_eq!(err.to_string(), "bench: --runs needs a positive integer");
        }
        let err = parse_args(&["--runs".to_owned()]).unwrap_err();
        assert_eq!(err.to_string(), "bench: --runs needs a positive integer");
    }

    #[test]
    fn median_by_p99() {
        let runs = vec![
            report("perf-cpu", 0.5, 1.0, 20.0),
            report("perf-cpu", 0.5, 1.4, 20.0),
            report("perf-cpu", 0.5, 1.2, 20.0),
        ];
        assert_eq!(median_index("perf-cpu", &runs).unwrap(), 2);
    }

    #[test]
    fn median_static_by_process_cpu() {
        // p99 order would pick run 1; process CPU order picks run 3.
        let runs = vec![
            report("static", 0.5, 1.2, 3.0),
            report("static", 0.5, 1.0, 1.0),
            report("static", 0.5, 1.4, 2.0),
        ];
        assert_eq!(median_index("static", &runs).unwrap(), 2);
    }

    #[test]
    fn compare_flags_regression_over_20pct() {
        let result = compare(&summary_with_p99(1.0), &summary_with_p99(1.25));
        let (scene, metric) = result.regression.unwrap();
        assert_eq!(
            format!("bench: regression above 20% in {scene} {metric}"),
            "bench: regression above 20% in perf-cpu frame_cpu_ms.p99"
        );
    }

    #[test]
    fn compare_passes_under_20pct() {
        let result = compare(&summary_with_p99(1.0), &summary_with_p99(1.15));
        assert!(result.regression.is_none());
        assert!(
            result
                .lines
                .contains(&"perf-cpu frame_cpu_ms.p99: 1.00 -> 1.15 (+15.00%)".to_owned())
        );
    }

    #[test]
    fn summary_json_shape() {
        let runs = vec![
            report("perf-cpu", 0.5, 1.0, 20.0),
            report("perf-cpu", 0.6, 1.4, 21.0),
            report("perf-cpu", 0.7, 1.2, 22.0),
        ];
        let summary = summarize(
            "2026-10-04",
            "0123456789ab-dirty",
            vec![("perf-cpu".to_owned(), runs.clone())],
        )
        .unwrap();
        assert_eq!(
            summary,
            json!({
                "date": "2026-10-04",
                "git_sha": "0123456789ab-dirty",
                "scenes": { "perf-cpu": { "median": runs[2], "runs": runs } }
            })
        );
        let table = table(&summary);
        assert!(table.contains("| perf-cpu | 0.70 | 1.20 | 0.00 | 22.00 | 120.00 | 0.00 | - |"));
    }

    #[test]
    fn child_failure_stops_the_command() {
        use std::os::unix::process::ExitStatusExt;
        let tmp = workspace_root()
            .unwrap()
            .join(format!("target/xtask-bench-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let scenes = vec!["bridge".to_owned(), "static".to_owned()];
        let mut calls = 0;
        let err = run_scenes(&scenes, 3, &tmp, |scene, out| {
            calls += 1;
            if calls == 2 {
                return Ok(ExitStatus::from_raw(1 << 8));
            }
            std::fs::write(out, report(scene, 0.5, 1.0, 1.0).to_string())?;
            Ok(ExitStatus::from_raw(0))
        })
        .unwrap_err();
        std::fs::remove_dir_all(&tmp).unwrap();
        assert_eq!(
            err.to_string(),
            "bench: bridge run 2 failed with exit code 1"
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn utc_date_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_730), (2026, 10, 4));
    }
}
