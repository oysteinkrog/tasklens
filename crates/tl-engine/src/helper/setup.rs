//! The Additional Setup window's engine side (E29-019, UI-PRF-052, PERF-105 to PERF-108,
//! APP-042, APP-049, R-149, R-247).
//!
//! `probe_status` reads what is set up already. `run_step` runs
//! `pkexec /usr/lib/tasklens/tasklens-setup <subcommand>` and streams its output. The two
//! handlers at the end run them on the blocking pool for `helper_task` and publish every change
//! as `SourceUpdate::Setup`.
//!
//! Security: the program and script paths are fixed and the subcommand comes from the closed
//! `SetupStep` enum, so no user text reaches the command line. `/etc/udev/rules.d/99-powercap.rules`
//! is never opened; only the existence of tasklens's own rule file is checked.
//!
//! Privacy: script output is never logged.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, PipeReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asupersync::Cx;
use asupersync::channel::mpsc;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use tl_collect::SysRoot;
use tl_model::command::{RaplStatus, SetupPhase, SetupRun, SetupState, SetupStatus, SetupStep};

use super::session::PKEXEC_PATH;
use super::{JOIN_SLACK, Lost, run_blocking};
use crate::update::{EngineMsg, SourceUpdate};

/// The setup script pkexec runs as root (table (c)).
pub(crate) const SETUP_SCRIPT: &str = "/usr/lib/tasklens/tasklens-setup";
/// The polkit row of ADR-006.
const RUN_DEADLINE: Duration = Duration::from_secs(120);
/// Captured output is capped at 64 KiB; the oldest whole lines go first (R-149 item 1).
const OUTPUT_CAP: usize = 64 * 1024;
/// At most one `Running` update with new output per 100 ms.
const OUTPUT_EVERY: Duration = Duration::from_millis(100);
/// Longest wait in one `poll` call, so the deadline is checked often.
const POLL_STEP: Duration = Duration::from_millis(100);

const RULE_FILE: &str = "etc/udev/rules.d/99-tasklens-powercap.rules";
const GROUP_FILE: &str = "etc/group";
const HELPER_FILE: &str = "usr/lib/tasklens/tl-helper";
const SENSORS_DETECT: &str = "usr/bin/sensors-detect";
const SENSORS_CONF: &str = "etc/modules-load.d/tasklens-sensors.conf";

/// What is set up on this machine (as2.29, R-149 item 1). Blocking: reads a few dozen sysfs
/// entries.
pub(crate) fn probe_status(root: &SysRoot) -> SetupStatus {
    probe_status_at(root, Path::new("/"))
}

/// `probe_status` with `/etc` and `/usr` under `base`, the test hook.
fn probe_status_at(root: &SysRoot, base: &Path) -> SetupStatus {
    SetupStatus {
        rapl: rapl_status(root, base),
        journal_group: journal_group(base),
        helper_installed: is_regular_file(&base.join(HELPER_FILE)),
        lm_sensors: is_regular_file(&base.join(SENSORS_DETECT)),
        hwmon_fan: hwmon_fan(root),
        sensors_conf: base.join(SENSORS_CONF).exists(),
    }
}

/// `NoZone` without an `intel-rapl:*` zone that has `energy_uj`; `Readable` when every such
/// file opens for reading; `NotReadable` otherwise.
fn rapl_status(root: &SysRoot, base: &Path) -> RaplStatus {
    let Ok(dir) = std::fs::read_dir(root.sys_path("class/powercap")) else {
        return RaplStatus::NoZone;
    };
    let mut zones = 0;
    let mut readable = true;
    for entry in dir.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("intel-rapl:")
        {
            continue;
        }
        let energy = entry.path().join("energy_uj");
        if !energy.is_file() {
            continue;
        }
        zones += 1;
        readable &= File::open(&energy).is_ok();
    }
    match (zones, readable) {
        (0, _) => RaplStatus::NoZone,
        (_, true) => RaplStatus::Readable {
            tasklens_rule: base.join(RULE_FILE).exists(),
        },
        (_, false) => RaplStatus::NotReadable,
    }
}

/// True when this process's supplementary groups contain `systemd-journal`.
fn journal_group(base: &Path) -> bool {
    let Ok(groups) = std::fs::read_to_string(base.join(GROUP_FILE)) else {
        return false;
    };
    let Some(gid) = groups.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some("systemd-journal"))
            .then(|| fields.nth(1)?.parse::<u32>().ok())
            .flatten()
    }) else {
        return false;
    };
    rustix::process::getgroups().is_ok_and(|ids| ids.iter().any(|g| g.as_raw() == gid))
}

/// True when any `hwmon*/fan*_input` exists.
fn hwmon_fan(root: &SysRoot) -> bool {
    let Ok(dir) = std::fs::read_dir(root.sys_path("class/hwmon")) else {
        return false;
    };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("hwmon"))
        .filter_map(|e| std::fs::read_dir(e.path()).ok())
        .flatten()
        .flatten()
        .any(|f| {
            let name = f.file_name();
            let name = name.to_string_lossy();
            name.starts_with("fan") && name.ends_with("_input")
        })
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// Output lines, at most `OUTPUT_CAP` bytes, each ending in `\n`. Pushing past the cap drops
/// whole lines from the front.
#[derive(Default)]
struct Capped {
    lines: VecDeque<Box<str>>,
    len: usize,
}

impl Capped {
    fn push(&mut self, line: &str) {
        let mut line = format!("{line}\n");
        if line.len() > OUTPUT_CAP {
            // One line longer than the cap keeps its end, cut at a character boundary.
            let mut cut = line.len() - OUTPUT_CAP;
            while !line.is_char_boundary(cut) {
                cut += 1;
            }
            line.drain(..cut);
        }
        self.len += line.len();
        self.lines.push_back(line.into_boxed_str());
        while self.len > OUTPUT_CAP {
            let Some(old) = self.lines.pop_front() else {
                break;
            };
            self.len -= old.len();
        }
    }

    fn text(&self) -> Arc<str> {
        let mut text = String::with_capacity(self.len);
        for line in &self.lines {
            text.push_str(line);
        }
        text.into()
    }
}

/// The arguments to pkexec: the script path, then `step.args()` (R-149 item 2).
fn command_args(step: SetupStep) -> Vec<&'static str> {
    let mut args = vec![SETUP_SCRIPT];
    args.extend_from_slice(step.args());
    args
}

/// Runs `pkexec /usr/lib/tasklens/tasklens-setup <step.args()>` with an empty environment and
/// calls `out` with each output line. Blocking, at most 120 s. Returns `Succeeded`, `Cancelled`
/// (pkexec exit 126 or 127) or `Failed`.
pub(crate) fn run_step(step: SetupStep, out: &mut dyn FnMut(&str)) -> SetupPhase {
    run_step_with(
        Path::new(PKEXEC_PATH),
        &command_args(step),
        RUN_DEADLINE,
        out,
    )
    .phase
}

/// The result of one run.
struct Ran {
    phase: SetupPhase,
    /// The child's pid, 0 when it did not start.
    #[cfg_attr(not(test), allow(dead_code))]
    pid: u32,
}

/// `run_step` with any program, arguments and deadline, the test hook.
///
/// `env_clear()` keeps `TASKLENS_SETUP_ROOT` and everything else away from the script.
/// `asupersync::process` is never used (zombie bug `bi2462.114`).
fn run_step_with(
    program: &Path,
    args: &[&str],
    deadline: Duration,
    out: &mut dyn FnMut(&str),
) -> Ran {
    let started = Instant::now();
    let spawned = io::pipe().and_then(|(reader, writer)| {
        let err = writer.try_clone()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::from(err));
        let child = command.spawn()?;
        // Dropping `command` closes this process's copies of the write end.
        drop(command);
        Ok((child, reader))
    });
    let (mut child, reader) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            return Ran {
                phase: SetupPhase::Failed(format!("could not start pkexec: {e}").into()),
                pid: 0,
            };
        }
    };
    let pid = child.id();
    let mut captured = Capped::default();
    let mut emit = |line: &[u8]| {
        let line = String::from_utf8_lossy(line);
        captured.push(&line);
        out(&line);
    };
    let finished = read_lines(reader, started + deadline, &mut emit)
        && wait_until(&mut child, started + deadline);
    if !finished {
        // Once pkexec runs the script as root, SIGKILL from this user fails. The pipe is
        // closed already, so the script dies on its next write.
        let _ = child.kill();
        let _ = child.wait();
        return Ran {
            phase: SetupPhase::Failed("timed out".into()),
            pid,
        };
    }
    let phase = match child.wait().ok().and_then(|s| s.code()) {
        Some(0) => SetupPhase::Succeeded,
        Some(126 | 127) => SetupPhase::Cancelled,
        _ => SetupPhase::Failed(captured.text()),
    };
    Ran { phase, pid }
}

/// Reads `reader` until EOF and calls `emit` per line, without its `\n`. False when `until`
/// passes first. A last line without `\n` is emitted at EOF.
fn read_lines(mut reader: PipeReader, until: Instant, emit: &mut dyn FnMut(&[u8])) -> bool {
    let mut pending = Vec::new();
    let mut buf = vec![0u8; 8192];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let step = left.min(POLL_STEP);
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: i64::from(step.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&reader, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => {}
            Err(_) => return true,
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => 0,
        };
        if n == 0 {
            if !pending.is_empty() {
                emit(&pending);
            }
            return true;
        }
        pending.extend_from_slice(&buf[..n]);
        let mut start = 0;
        while let Some(at) = pending[start..].iter().position(|&b| b == b'\n') {
            emit(&pending[start..start + at]);
            start += at + 1;
        }
        pending.drain(..start);
    }
}

/// Waits for the child until `until`. False when it is still running then.
fn wait_until(child: &mut Child, until: Instant) -> bool {
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) => {}
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(10)));
    }
}

/// The probe `HelperMsg::SetupStatus` runs.
pub type SetupProbe = Arc<dyn Fn() -> SetupStatus + Send + Sync>;
/// The runner `HelperMsg::RunSetup` uses: `run_step` outside tests.
pub type SetupRunner = Arc<dyn Fn(SetupStep, &mut dyn FnMut(&str)) -> SetupPhase + Send + Sync>;

/// The real probe on the host root.
pub(crate) fn real_probe() -> SetupProbe {
    Arc::new(|| probe_status(&SysRoot::host()))
}

/// The real runner.
pub(crate) fn real_runner() -> SetupRunner {
    Arc::new(run_step)
}

async fn publish(cx: &Cx, state: &SetupState, updates: &mpsc::Sender<EngineMsg>) {
    let update = SourceUpdate::Setup(Arc::new(state.clone()));
    let _ = updates.send(cx, EngineMsg::Source(update)).await;
}

/// `HelperMsg::SetupStatus`: probes on the blocking pool and publishes `status`. A probe that
/// does not finish leaves the status as it was.
pub(crate) async fn handle_setup_status(
    cx: &Cx,
    probe: &SetupProbe,
    state: &mut SetupState,
    updates: &mpsc::Sender<EngineMsg>,
) {
    let probe = Arc::clone(probe);
    match run_blocking(cx, JOIN_SLACK, move || probe()).await {
        Ok(status) => {
            state.status = Some(status);
            publish(cx, state, updates).await;
        }
        Err(_) => tracing::debug!(target: "tasklens::action", "setup probe gave no answer"),
    }
}

/// `HelperMsg::RunSetup`: publishes `Running`, runs the step on the blocking pool with output
/// updates at most every 100 ms, publishes the final phase, then probes again. Ignored while a
/// run is in progress.
pub(crate) async fn handle_run_setup(
    cx: &Cx,
    step: SetupStep,
    runner: &SetupRunner,
    probe: &SetupProbe,
    state: &mut SetupState,
    updates: &mpsc::Sender<EngineMsg>,
) {
    if state
        .run
        .as_ref()
        .is_some_and(|run| run.phase == SetupPhase::Running)
    {
        return;
    }
    let seq = state.run.as_ref().map_or(1, |run| run.seq + 1);
    let running = move |output: Arc<str>| SetupRun {
        seq,
        step,
        phase: SetupPhase::Running,
        output,
    };
    state.run = Some(running("".into()));
    publish(cx, state, updates).await;

    let runner = Arc::clone(runner);
    let tx = updates.clone();
    let status = state.status;
    let joined = run_blocking(cx, RUN_DEADLINE + JOIN_SLACK, move || {
        let mut captured = Capped::default();
        let mut last = Instant::now();
        let phase = runner(step, &mut |line| {
            captured.push(line);
            if last.elapsed() >= OUTPUT_EVERY {
                last = Instant::now();
                let state = SetupState {
                    status,
                    run: Some(running(captured.text())),
                };
                let update = SourceUpdate::Setup(Arc::new(state));
                let _ = tx.try_send(EngineMsg::Source(update));
            }
        });
        (phase, captured.text())
    })
    .await;
    let (phase, output) = match joined {
        Ok(done) => done,
        Err(Lost::Timeout) => (SetupPhase::Failed("timed out".into()), "".into()),
        Err(Lost::Gone(reason)) => (SetupPhase::Failed(reason), "".into()),
    };
    state.run = Some(SetupRun {
        seq,
        step,
        phase,
        output,
    });
    publish(cx, state, updates).await;
    handle_setup_status(cx, probe, state, updates).await;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    /// A fake `/` under `target/tmp`, removed on drop.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            fs::create_dir_all(&base).expect("create target/tmp");
            let dir = tempfile::Builder::new()
                .prefix("tl-engine-setup-")
                .tempdir_in(base)
                .expect("tempdir");
            Self { dir }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
            fs::write(path, bytes).expect("write");
        }

        fn probe(&self) -> SetupStatus {
            probe_status_at(&SysRoot::at(self.dir.path()), self.dir.path())
        }
    }

    const SH: &str = "/bin/sh";

    /// Runs `sh -c script` with a 10 s deadline. Returns the outcome and every line `out` saw.
    fn run_sh(script: &str, deadline: Duration) -> (Ran, Vec<String>) {
        let mut lines = Vec::new();
        let ran = run_step_with(Path::new(SH), &["-c", script], deadline, &mut |line| {
            lines.push(line.to_owned());
        });
        (ran, lines)
    }

    #[test]
    fn rapl_existing_rule() {
        let fake = Fake::new();
        fake.write("sys/class/powercap/intel-rapl:0/energy_uj", b"1\n");
        fake.write("sys/class/powercap/intel-rapl:0:0/energy_uj", b"2\n");
        assert_eq!(
            fake.probe().rapl,
            RaplStatus::Readable {
                tasklens_rule: false
            }
        );
        fake.write(RULE_FILE, b"");
        assert_eq!(
            fake.probe().rapl,
            RaplStatus::Readable {
                tasklens_rule: true
            }
        );
    }

    #[test]
    fn rapl_not_readable() {
        let fake = Fake::new();
        fake.write("sys/class/powercap/intel-rapl:0/energy_uj", b"1\n");
        fake.write("sys/class/powercap/intel-rapl:0:0/energy_uj", b"2\n");
        let zone = fake.path("sys/class/powercap/intel-rapl:0:0/energy_uj");
        fs::set_permissions(&zone, fs::Permissions::from_mode(0o200)).expect("chmod");
        assert_eq!(fake.probe().rapl, RaplStatus::NotReadable);
    }

    #[test]
    fn rapl_no_zone() {
        let fake = Fake::new();
        assert_eq!(fake.probe().rapl, RaplStatus::NoZone);
        // A zone directory without `energy_uj` is no zone.
        fs::create_dir_all(fake.path("sys/class/powercap/intel-rapl:0")).expect("mkdir");
        assert_eq!(fake.probe().rapl, RaplStatus::NoZone);
    }

    #[test]
    fn probe_reads_the_other_fields() {
        let fake = Fake::new();
        let none = fake.probe();
        assert!(!none.helper_installed && !none.lm_sensors && !none.hwmon_fan);
        assert!(!none.sensors_conf && !none.journal_group);
        fake.write(HELPER_FILE, b"");
        fake.write(SENSORS_DETECT, b"");
        fake.write(SENSORS_CONF, b"");
        fake.write("sys/class/hwmon/hwmon3/fan1_input", b"900\n");
        let all = fake.probe();
        assert!(all.helper_installed && all.lm_sensors && all.hwmon_fan && all.sensors_conf);
    }

    #[test]
    fn run_ok_streams_lines() {
        let (ran, lines) = run_sh("echo one; echo two >&2; exit 0", Duration::from_secs(10));
        assert_eq!(ran.phase, SetupPhase::Succeeded);
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn run_126_127_cancelled() {
        for code in [126, 127] {
            let (ran, _) = run_sh(&format!("exit {code}"), Duration::from_secs(10));
            assert_eq!(ran.phase, SetupPhase::Cancelled, "exit {code}");
        }
    }

    #[test]
    fn run_failure_output() {
        let (ran, _) = run_sh("echo boom; exit 1", Duration::from_secs(10));
        assert_eq!(ran.phase, SetupPhase::Failed("boom\n".into()));
    }

    #[test]
    fn run_output_cap() {
        // 2000 lines of 61 bytes: about 119 KiB.
        let script = "i=0; while [ $i -lt 2000 ]; do \
                      printf 'line %05d %050d\\n' $i 0; i=$((i+1)); done; exit 1";
        let (ran, lines) = run_sh(script, Duration::from_secs(30));
        assert_eq!(lines.len(), 2000);
        let SetupPhase::Failed(output) = ran.phase else {
            panic!("expected Failed, got {:?}", ran.phase);
        };
        assert!(output.len() <= OUTPUT_CAP, "{} bytes", output.len());
        assert!(output.len() > OUTPUT_CAP - 61);
        assert!(output.starts_with("line "));
        let last = format!("{}\n", lines[1999]);
        assert!(output.ends_with(&last));
    }

    #[test]
    fn run_uses_step_args() {
        let steps = [
            SetupStep::RaplEnable,
            SetupStep::RaplRevert,
            SetupStep::SensorsDetect,
            SetupStep::SensorsRevert,
            SetupStep::PolkitRule { on: true },
            SetupStep::PolkitRule { on: false },
        ];
        for step in steps {
            // `sh -c` sets `$0` to the first argument after the script: the setup script path.
            let mut args = vec!["-c", "printf '%s\\n' \"$0\" \"$@\""];
            args.extend(command_args(step));
            let mut lines = Vec::new();
            let ran = run_step_with(Path::new(SH), &args, Duration::from_secs(10), &mut |l| {
                lines.push(l.to_owned());
            });
            assert_eq!(ran.phase, SetupPhase::Succeeded);
            assert_eq!(lines[0], SETUP_SCRIPT);
            assert_eq!(lines[1..], *step.args(), "{step:?}");
        }
    }

    #[test]
    fn run_timeout_reaps() {
        let (ran, _) = run_sh("exec /usr/bin/sleep 200", Duration::from_secs(1));
        assert_eq!(ran.phase, SetupPhase::Failed("timed out".into()));
        assert_ne!(ran.pid, 0);
        assert!(!Path::new(&format!("/proc/{}", ran.pid)).exists());
    }

    #[test]
    fn missing_program_fails() {
        let mut out = |_: &str| {};
        let ran = run_step_with(
            Path::new("/nonexistent/pkexec"),
            &[],
            Duration::from_secs(1),
            &mut out,
        );
        assert!(matches!(ran.phase, SetupPhase::Failed(_)));
    }

    fn packaging(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging")
            .join(name)
    }

    /// Runs `packaging/tasklens-setup` with `args`, in test mode under `root` when given.
    /// Returns the exit code, stdout and stderr.
    fn script(root: Option<&Path>, args: &[&str]) -> (i32, String, String) {
        let mut command = Command::new(SH);
        command
            .arg(packaging("tasklens-setup"))
            .args(args)
            .env_remove("TASKLENS_SETUP_ROOT");
        if let Some(root) = root {
            command.env("TASKLENS_SETUP_ROOT", root);
        }
        let out = command.output().expect("run tasklens-setup");
        (
            out.status.code().expect("exit code"),
            String::from_utf8(out.stdout).expect("utf-8"),
            String::from_utf8(out.stderr).expect("utf-8"),
        )
    }

    /// Like `script`, with `PKEXEC_UID` set to `uid` (or removed).
    fn script_uid(root: &Path, uid: Option<&str>, args: &[&str]) -> (i32, String, String) {
        let mut command = Command::new(SH);
        command
            .arg(packaging("tasklens-setup"))
            .args(args)
            .env("TASKLENS_SETUP_ROOT", root)
            .env_remove("PKEXEC_UID");
        if let Some(uid) = uid {
            command.env("PKEXEC_UID", uid);
        }
        let out = command.output().expect("run tasklens-setup");
        (
            out.status.code().expect("exit code"),
            String::from_utf8(out.stdout).expect("utf-8"),
            String::from_utf8(out.stderr).expect("utf-8"),
        )
    }

    /// A fake root with `etc/udev/rules.d/`, for test mode.
    fn rules_root() -> Fake {
        let fake = Fake::new();
        fs::create_dir_all(fake.path("etc/udev/rules.d")).expect("mkdir");
        fake
    }

    const UDEV_LINES: [&str; 2] = [
        "would run: udevadm control --reload",
        "would run: udevadm trigger --subsystem-match=powercap --action=add",
    ];

    #[test]
    fn script_rapl_enable_writes_rule() {
        let fake = rules_root();
        let (code, out, _) = script(Some(fake.dir.path()), &["rapl-enable"]);
        assert_eq!(code, 0, "{out}");
        let rule = fake.path(RULE_FILE);
        assert_eq!(
            fs::read(&rule).expect("rule"),
            fs::read(packaging("99-tasklens-powercap.rules")).expect("reference rule")
        );
        let mode = fs::metadata(&rule).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644);
        assert!(
            !fake
                .path("etc/udev/rules.d/99-tasklens-powercap.rules.tmp")
                .exists()
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, [UDEV_LINES[0], UDEV_LINES[1], "Enabled"]);
    }

    #[test]
    fn script_never_touches_99_powercap() {
        let fake = rules_root();
        let other = fake.path("etc/udev/rules.d/99-powercap.rules");
        fs::write(&other, b"# not ours\n").expect("write");
        let before = fs::metadata(&other)
            .expect("stat")
            .modified()
            .expect("mtime");
        for step in ["rapl-enable", "rapl-revert"] {
            let (code, out, err) = script(Some(fake.dir.path()), &[step]);
            assert_eq!(code, 0, "{step}: {out}{err}");
        }
        assert_eq!(fs::read(&other).expect("read"), b"# not ours\n");
        let after = fs::metadata(&other)
            .expect("stat")
            .modified()
            .expect("mtime");
        assert_eq!(before, after);
    }

    #[test]
    fn script_rapl_revert_removes_rule() {
        let fake = rules_root();
        let (code, _, _) = script(Some(fake.dir.path()), &["rapl-enable"]);
        assert_eq!(code, 0);
        assert!(fake.path(RULE_FILE).exists());
        let (code, out, _) = script(Some(fake.dir.path()), &["rapl-revert"]);
        assert_eq!(code, 0, "{out}");
        assert!(!fake.path(RULE_FILE).exists());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                UDEV_LINES[0],
                UDEV_LINES[1],
                "Reverted. A reboot may be needed before the files return to mode 0400.",
            ]
        );
    }

    #[test]
    fn script_requires_root_or_test_root() {
        if rustix::process::geteuid().is_root() {
            // As root the script runs for real; never do that in a test.
            return;
        }
        for step in ["rapl-enable", "rapl-revert", "bogus"] {
            let (code, out, err) = script(None, &[step]);
            assert_eq!(code, 1, "{step}");
            assert_eq!(out, "");
            assert_eq!(err, "tasklens-setup: must run as root (use pkexec)\n");
        }
    }

    #[test]
    fn script_usage_exit_2() {
        let fake = rules_root();
        let usage = "usage: tasklens-setup rapl-enable|rapl-revert|sensors-detect|sensors-revert|polkit-rule on|off|linger on|off|remote-admin on|off|undo\n";
        for args in [&["bogus"][..], &[]] {
            let (code, out, err) = script(Some(fake.dir.path()), args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "");
            assert_eq!(err, usage);
        }
    }

    const POLKIT_RULE: &str = "etc/polkit-1/rules.d/49-tasklens.rules";

    /// The rule `polkit-rule on` writes (APP-049).
    const POLKIT_RULE_TEXT: &str = "\
polkit.addRule(function(action, subject) {
    if (action.id.indexOf(\"io.github.oysteinkrog.tasklens.\") == 0 &&
        subject.local && subject.active && subject.isInGroup(\"wheel\")) {
        return polkit.Result.YES;
    }
});
";

    /// A fake root with `etc/polkit-1/rules.d/`, for test mode.
    fn polkit_root() -> Fake {
        let fake = Fake::new();
        fs::create_dir_all(fake.path("etc/polkit-1/rules.d")).expect("mkdir");
        fake
    }

    #[test]
    fn script_polkit_rule_on_exact_text() {
        let fake = polkit_root();
        let (code, out, err) = script(Some(fake.dir.path()), &["polkit-rule", "on"]);
        assert_eq!(code, 0, "{out}{err}");
        let rule = fake.path(POLKIT_RULE);
        assert_eq!(fs::read(&rule).expect("rule"), POLKIT_RULE_TEXT.as_bytes());
        let mode = fs::metadata(&rule).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644);
        assert!(!fake.path(&format!("{POLKIT_RULE}.tmp")).exists());
        assert_eq!(out.lines().last(), Some("Enabled"));
    }

    #[test]
    fn script_polkit_rule_off() {
        let fake = polkit_root();
        let (code, _, _) = script(Some(fake.dir.path()), &["polkit-rule", "on"]);
        assert_eq!(code, 0);
        assert!(fake.path(POLKIT_RULE).exists());
        let (code, out, err) = script(Some(fake.dir.path()), &["polkit-rule", "off"]);
        assert_eq!(code, 0, "{out}{err}");
        assert!(!fake.path(POLKIT_RULE).exists());
        assert_eq!(out, "Reverted\n");
    }

    #[test]
    fn script_polkit_rule_bad_arg() {
        let fake = polkit_root();
        let usage = "usage: tasklens-setup rapl-enable|rapl-revert|sensors-detect|sensors-revert|polkit-rule on|off|linger on|off|remote-admin on|off|undo\n";
        for args in [&["polkit-rule", "maybe"][..], &["polkit-rule"]] {
            let (code, out, err) = script(Some(fake.dir.path()), args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "");
            assert_eq!(err, usage);
        }
        assert!(!fake.path(POLKIT_RULE).exists());
    }

    #[test]
    fn script_linger_on_and_off_act_for_the_pkexec_user() {
        let fake = Fake::new();
        let (code, out, err) = script_uid(fake.dir.path(), Some("1000"), &["linger", "on"]);
        assert_eq!(code, 0, "{out}{err}");
        assert_eq!(out, "would run: loginctl enable-linger 1000\nEnabled\n");
        let (code, out, err) = script_uid(fake.dir.path(), Some("1000"), &["linger", "off"]);
        assert_eq!(code, 0, "{out}{err}");
        assert_eq!(out, "would run: loginctl disable-linger 1000\nReverted\n");
    }

    /// Security regression: a missing or odd `PKEXEC_UID` never reaches loginctl.
    #[test]
    fn script_linger_refuses_a_bad_pkexec_uid() {
        let fake = Fake::new();
        for uid in [None, Some(""), Some("root"), Some("1000 0"), Some("-1")] {
            let (code, out, err) = script_uid(fake.dir.path(), uid, &["linger", "on"]);
            assert_eq!(code, 1, "{uid:?}");
            assert_eq!(out, "", "{uid:?}");
            assert_eq!(
                err,
                "tasklens-setup: PKEXEC_UID is missing or not a number\n"
            );
        }
    }

    #[test]
    fn script_linger_bad_arg() {
        let fake = Fake::new();
        let (code, out, _) = script_uid(fake.dir.path(), Some("1000"), &["linger", "maybe"]);
        assert_eq!(code, 2);
        assert_eq!(out, "");
    }

    /// Security regression: the rule covers only tasklens's own actions, never a third-party one.
    #[test]
    fn script_polkit_rule_scope() {
        let fake = polkit_root();
        let (code, _, _) = script(Some(fake.dir.path()), &["polkit-rule", "on"]);
        assert_eq!(code, 0);
        let rule = fs::read_to_string(fake.path(POLKIT_RULE)).expect("rule");
        assert!(rule.contains("action.id.indexOf(\"io.github.oysteinkrog.tasklens.\") == 0"));
        assert!(!rule.contains("org.freedesktop."));
        assert!(rule.contains("subject.local && subject.active && subject.isInGroup(\"wheel\")"));
    }

    const LM_CONF: &str = "etc/conf.d/lm_sensors";
    const SAVED: &str = "var/lib/tasklens/sensors-saved";
    const IS_ENABLED: &str = "would run: systemctl is-enabled --quiet lm_sensors.service";
    const DISABLE: &str = "would run: systemctl disable lm_sensors.service";

    /// The end of `sensors-detect --auto` (lm_sensors 3.6) when /etc/conf.d exists: it writes
    /// [`DETECT_CONF`] there and prints no module names.
    const DETECT_OUT_CONF: &str = "\
# sensors-detect version 3.6.0
Driver `nct6775':
  * ISA bus, address 0x290
    Chip `Nuvoton NCT6797D Super IO Sensors' (confidence: 9)

Driver `coretemp' (autoloaded):
  * Chip `Intel digital thermal sensor' (confidence: 9)

Do you want to generate /etc/conf.d/lm_sensors? (YES/no):
Created symlink '/etc/systemd/system/multi-user.target.wants/lm_sensors.service' → '/usr/lib/systemd/system/lm_sensors.service'.
Unloading i2c-dev... OK
";

    /// The /etc/conf.d/lm_sensors that sensors-detect 3.6 writes.
    const DETECT_CONF: &str = "\
# Generated by sensors-detect on Mon Oct  6 12:00:00 2026
# This file is sourced by /etc/init.d/lm_sensors and defines the modules to
# be loaded/unloaded.
#
# The format of this file is a shell script that simply defines variables:
# HWMON_MODULES for hardware monitoring driver modules, and optionally
# BUS_MODULES for any required bus driver module (for example for I2C or SPI).

BUS_MODULES=\"i2c-i801\"
HWMON_MODULES=\"nct6775 coretemp\"
";

    /// The end of `sensors-detect --auto` (lm_sensors 3.6) when /etc/conf.d does not exist.
    const DETECT_OUT_CUT: &str = "\
# sensors-detect version 3.6.0
Driver `nct6775':
  * ISA bus, address 0x290
    Chip `Nuvoton NCT6797D Super IO Sensors' (confidence: 9)

Do you want to generate /etc/conf.d/lm_sensors? (yes/NO):
To load everything that is needed, add this to one of the system
initialization scripts (e.g. /etc/rc.d/rc.local):

#----cut here----
# Adapter drivers
modprobe i2c-i801
# Chip drivers
modprobe nct6775
modprobe coretemp
/usr/bin/sensors -s
#----cut here----

You really should try these commands right now to make sure everything
is working properly. Monitoring programs won't work until the needed
modules are loaded.

Unloading i2c-dev... OK
";

    const DETECT_MODULES: &str = "i2c-i801\nnct6775\ncoretemp\n";

    /// A fake root with `usr/bin/sensors-detect`, `etc/modules-load.d/` and `sensors-detect.out`
    /// holding `output`, for test mode. With `conf`, test mode copies it to [`LM_CONF`] the way
    /// sensors-detect writes that file.
    fn sensors_root(output: &str, conf: Option<&str>) -> Fake {
        let fake = Fake::new();
        fake.write(SENSORS_DETECT, b"");
        fs::create_dir_all(fake.path("etc/modules-load.d")).expect("mkdir");
        fake.write("sensors-detect.out", output.as_bytes());
        if let Some(conf) = conf {
            fs::create_dir_all(fake.path("etc/conf.d")).expect("mkdir");
            fake.write("sensors-detect.conf", conf.as_bytes());
        }
        fake
    }

    /// Runs `sensors-detect` and returns the module file and the modprobe lines.
    fn detect_modules(fake: &Fake) -> (String, Vec<String>) {
        let (code, out, err) = script(Some(fake.dir.path()), &["sensors-detect"]);
        assert_eq!(code, 0, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"would run: sensors-detect --auto"), "{out}");
        assert_eq!(lines.last(), Some(&"Enabled"), "{out}");
        let conf = fake.path(SENSORS_CONF);
        let mode = fs::metadata(&conf).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644);
        assert!(
            !fake
                .path("etc/modules-load.d/tasklens-sensors.conf.tmp")
                .exists()
        );
        let modprobe = out
            .lines()
            .filter(|l| l.starts_with("would run: modprobe"))
            .map(str::to_owned)
            .collect();
        (fs::read_to_string(&conf).expect("conf"), modprobe)
    }

    /// Both ways sensors-detect 3.6 reports modules give the same module list.
    #[test]
    fn script_sensors_detect_conf_d_and_cut_here_agree() {
        let from_conf = detect_modules(&sensors_root(DETECT_OUT_CONF, Some(DETECT_CONF)));
        let from_cut = detect_modules(&sensors_root(DETECT_OUT_CUT, None));
        assert_eq!(from_conf.0, DETECT_MODULES);
        assert_eq!(
            from_conf.1,
            [
                "would run: modprobe i2c-i801",
                "would run: modprobe nct6775",
                "would run: modprobe coretemp",
            ]
        );
        assert_eq!(from_conf, from_cut);
    }

    /// Security regression: only names of letters, digits, `_` and `-` reach modprobe.
    #[test]
    fn script_sensors_bad_module_name_skipped() {
        let bad = ["evil;rm", "$(id)", "a/b", "x`y`"];
        let cut = format!(
            "#----cut here----\nmodprobe nct6775\nmodprobe {}\nmodprobe \n#----cut here----\n",
            bad.join("\nmodprobe ")
        );
        let conf = format!("HWMON_MODULES=\"nct6775 {}\"\n", bad.join(" "));
        for fake in [sensors_root(&cut, None), sensors_root("", Some(&conf))] {
            let (code, out, err) = script(Some(fake.dir.path()), &["sensors-detect"]);
            assert_eq!(code, 0, "{out}{err}");
            for name in bad {
                let skipped = format!("skipped module name: {name}");
                assert!(out.lines().any(|l| l == skipped), "{name}: {out}");
            }
            assert_eq!(
                fs::read_to_string(fake.path(SENSORS_CONF)).expect("conf"),
                "nct6775\n"
            );
            let modprobe: Vec<&str> = out
                .lines()
                .filter(|l| l.starts_with("would run: modprobe"))
                .collect();
            assert_eq!(modprobe, ["would run: modprobe nct6775"]);
        }
    }

    /// `modprobe` lines outside the cut-here block, and a conf.d file this run did not write,
    /// are not read.
    #[test]
    fn script_sensors_none_found() {
        let fake = sensors_root(
            "modprobe foo\nNo modules to load, skipping modules configuration.\n\n",
            None,
        );
        fake.write(LM_CONF, DETECT_CONF.as_bytes());
        let (code, out, err) = script(Some(fake.dir.path()), &["sensors-detect"]);
        assert_eq!(code, 0, "{out}{err}");
        assert_eq!(out.lines().last(), Some("No sensor modules found"));
        assert!(!out.contains("would run: modprobe"));
        assert!(!fake.path(SENSORS_CONF).exists());
        assert!(
            !fake
                .path("etc/modules-load.d/tasklens-sensors.conf.tmp")
                .exists()
        );
    }

    #[test]
    fn script_sensors_missing_lm_sensors() {
        let fake = rules_root();
        let (code, out, err) = script(Some(fake.dir.path()), &["sensors-detect"]);
        assert_eq!(code, 3, "{out}{err}");
        assert_eq!(out, "Install lm_sensors to detect fan sensors\n");
        assert!(!fake.path(SENSORS_CONF).exists());
        assert!(!fake.path(SAVED).exists());
    }

    /// Runs `sensors-revert` and returns its stdout lines.
    fn revert(fake: &Fake) -> Vec<String> {
        let (code, out, err) = script(Some(fake.dir.path()), &["sensors-revert"]);
        assert_eq!(code, 0, "{out}{err}");
        assert!(!fake.path(SENSORS_CONF).exists());
        assert!(!fake.path(SAVED).exists());
        assert!(!fake.path("etc/conf.d/lm_sensors.tasklens.tmp").exists());
        assert_eq!(
            out.lines().last(),
            Some("Reverted. The modules stay loaded until the next reboot.")
        );
        out.lines().map(str::to_owned).collect()
    }

    /// No conf.d file and a disabled service before: revert removes the file sensors-detect
    /// wrote and disables the service it enabled.
    #[test]
    fn script_sensors_revert_removes_new_conf_d() {
        let fake = sensors_root(DETECT_OUT_CONF, Some(DETECT_CONF));
        detect_modules(&fake);
        assert!(fake.path(LM_CONF).exists());
        // sensors-detect enabled the service.
        fake.write("lm_sensors.enabled", b"");
        let lines = revert(&fake);
        assert!(!fake.path(LM_CONF).exists());
        assert!(lines.iter().any(|l| l == IS_ENABLED), "{lines:?}");
        assert!(lines.iter().any(|l| l == DISABLE), "{lines:?}");
    }

    /// A conf.d file and an enabled service before: revert restores the old file byte for byte
    /// and leaves the service enabled.
    #[test]
    fn script_sensors_revert_restores_old_conf_d() {
        let old: &[u8] = b"# mine\nHWMON_MODULES=\"it87\"\n\xff\x00 no newline";
        let fake = sensors_root(DETECT_OUT_CONF, Some(DETECT_CONF));
        fake.write(LM_CONF, old);
        fake.write("lm_sensors.enabled", b"");
        detect_modules(&fake);
        assert_eq!(
            fs::read(fake.path(LM_CONF)).expect("conf"),
            DETECT_CONF.as_bytes()
        );
        let lines = revert(&fake);
        assert_eq!(fs::read(fake.path(LM_CONF)).expect("conf"), old);
        assert!(
            !lines.iter().any(|l| l.contains("systemctl disable")),
            "{lines:?}"
        );
    }

    /// A second detection before a revert keeps the state saved by the first.
    #[test]
    fn script_sensors_revert_after_two_runs() {
        let fake = sensors_root(DETECT_OUT_CONF, Some(DETECT_CONF));
        detect_modules(&fake);
        fake.write(
            "sensors-detect.conf",
            DETECT_CONF.replace("Oct  6", "Oct  7").as_bytes(),
        );
        detect_modules(&fake);
        revert(&fake);
        assert!(!fake.path(LM_CONF).exists());
    }

    /// Without saved state, revert removes only the module file.
    #[test]
    fn script_sensors_revert_without_state() {
        let fake = sensors_root("", None);
        fake.write(SENSORS_CONF, b"nct6775\n");
        fake.write(LM_CONF, b"keep\n");
        let lines = revert(&fake);
        assert_eq!(
            lines,
            ["Reverted. The modules stay loaded until the next reboot."]
        );
        assert_eq!(fs::read(fake.path(LM_CONF)).expect("conf"), b"keep\n");
    }

    #[test]
    fn script_shellcheck_clean() {
        let Ok(out) = Command::new("shellcheck")
            .args(["-s", "sh"])
            .arg(packaging("tasklens-setup"))
            .output()
        else {
            eprintln!("shellcheck not installed, skipped");
            return;
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
