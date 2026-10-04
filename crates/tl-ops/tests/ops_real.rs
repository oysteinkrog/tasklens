//! Real-system tests for the process, cgroup, file, launch and dump operations (ADR-006, ADR-013).
//! They need a normal user on the owner machine, so every test is `#[ignore]`. Run them with
//! `cargo test -p tl-ops -- --ignored`.
//!
//! Each test spawns its own children and kills and reaps them in a `Drop` guard. No signal is
//! ever sent to a process the test did not spawn, and only scopes the test started are frozen,
//! thawed or killed.

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tl_model::actions::{CgroupRef, DumpMode, IoPriority, Signal};
use tl_model::ids::ProcessKey;
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_ops::cgroup::{CGROUP_ROOT, freeze_group, kill_group, own_cgroup, thaw_group};
use tl_ops::dump::{GDB_PATH, dump_to_path};
use tl_ops::fs::{
    EnableOutcome, StartupRoots, autostart_disable_system, autostart_enable_override,
};
use tl_ops::launch::{ExitKind, Spawned, spawn_detached, wait_exit};
use tl_ops::process::{kill_tree, send_signal, set_affinity, set_io_priority, set_nice};
use tl_ops::{OpsError, PRIVATE_FILE_MODE, write_atomic};

/// `PF_KTHREAD` in `/proc/<pid>/stat` field 9.
const PF_KTHREAD: u64 = 0x0020_0000;

/// `CAP_SYS_NICE` bit in `CapEff`.
const CAP_SYS_NICE: u32 = 23;

/// The folded boot id of this boot (ADR-003).
fn boot_id() -> BootId {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("read boot_id");
    BootId::from_uuid_text(&text).expect("parse boot_id")
}

/// The fields of `/proc/<pid>/stat` after the name, so index 0 is field 3 (`state`). None when
/// the process is gone.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    stat_fields_at(&format!("/proc/{pid}/stat"))
}

fn stat_fields_at(path: &str) -> Option<Vec<String>> {
    let stat = std::fs::read_to_string(path).ok()?;
    let close = stat.rfind(')')?;
    Some(
        stat[close + 1..]
            .split_whitespace()
            .map(String::from)
            .collect(),
    )
}

/// Field `n` (1-based, as in proc(5)) of `/proc/<pid>/stat`.
fn stat_field(pid: u32, n: usize) -> Option<String> {
    stat_fields(pid)?.get(n - 3).cloned()
}

/// The key of `pid`, from `/proc/<pid>/stat` field 22.
fn key_of(pid: u32, boot: BootId) -> ProcessKey {
    let start_ticks = stat_field(pid, 22)
        .expect("read stat")
        .parse()
        .expect("parse starttime");
    ProcessKey {
        start_ticks,
        boot_id: boot,
        pid,
    }
}

/// True while `key` still names a live process: `/proc/<pid>` exists with the same start time
/// and is not a zombie.
fn is_alive(key: ProcessKey) -> bool {
    let Some(fields) = stat_fields(key.pid) else {
        return false;
    };
    fields[0] != "Z" && fields.get(19).and_then(|s| s.parse().ok()) == Some(key.start_ticks)
}

/// Polls `cond` every 10 ms until it holds or `limit` passes.
fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// A child of this test, killed and reaped on drop even when an assertion fails. Processes it
/// started itself (`grandchildren`) are killed through a verified pidfd, so a reused PID is never
/// hit.
struct ChildGuard {
    child: Child,
    reaped: bool,
    grandchildren: Vec<ProcessKey>,
    boot: BootId,
}

impl ChildGuard {
    fn spawn(program: &str, args: &[&str]) -> Self {
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn child");
        ChildGuard {
            child,
            reaped: false,
            grandchildren: Vec::new(),
            boot: boot_id(),
        }
    }

    fn sleep() -> Self {
        Self::spawn("sleep", &["60"])
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn key(&self) -> ProcessKey {
        key_of(self.pid(), self.boot)
    }

    fn wait(&mut self) -> std::process::ExitStatus {
        let status = self.child.wait().expect("wait child");
        self.reaped = true;
        status
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        for &key in &self.grandchildren {
            let _ = send_signal(key, self.boot, Signal::Kill);
        }
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The nice value of every thread of `pid`, from `/proc/<pid>/task/*/stat` field 19.
fn all_nice(pid: u32) -> Vec<i32> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/task")).expect("read task dir") {
        let path = entry.expect("task entry").path().join("stat");
        let fields = stat_fields_at(path.to_str().expect("utf-8 path")).expect("read task stat");
        out.push(fields[16].parse().expect("parse nice"));
    }
    out
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_open_verified_true_key_signals() {
    let mut child = ChildGuard::sleep();
    let key = child.key();
    assert_eq!(send_signal(key, child.boot, Signal::Term), Ok(()));
    let status = child.wait();
    assert_eq!(status.signal(), Some(15), "wait status {status:?}");
}

/// Regression test for PID reuse protection on the real kernel.
#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_shifted_start_ticks_is_identity_changed() {
    let child = ChildGuard::sleep();
    let mut key = child.key();
    key.start_ticks += 1;
    assert_eq!(
        send_signal(key, child.boot, Signal::Kill),
        Err(OpsError::IdentityChanged)
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(is_alive(child.key()), "the child no longer runs");
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_set_nice_5_every_tid() {
    let child = ChildGuard::sleep();
    // The child inherits our nice. A daemon such as ananicy may have raised it above 5, and
    // lowering it needs privilege, so the target is then the current value.
    let current = all_nice(child.pid()).into_iter().max().expect("no threads");
    let target = i8::try_from(current.max(5)).expect("nice fits i8");
    assert_eq!(set_nice(child.key(), child.boot, target), Ok(()));
    let nice = all_nice(child.pid());
    assert!(
        nice.iter().all(|&n| n == i32::from(target)),
        "nice values {nice:?}"
    );
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_exited_child_never_succeeds() {
    let mut child = ChildGuard::spawn("true", &[]);
    // A zombie keeps its stat line, so this reads the start time even when `true` already exited.
    let key = child.key();
    assert!(child.wait().success());
    let result = set_nice(key, child.boot, 5);
    assert!(
        matches!(
            result,
            Err(OpsError::IdentityChanged) | Err(OpsError::OutcomeUnknown(_))
        ),
        "result {result:?}"
    );
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_set_affinity_cpu0() {
    let child = ChildGuard::sleep();
    let mut set = CpuSet::empty();
    set.insert(0);
    assert_eq!(set_affinity(child.key(), child.boot, &set), Ok(()));
    let status =
        std::fs::read_to_string(format!("/proc/{}/status", child.pid())).expect("read status");
    let allowed = status
        .lines()
        .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
        .expect("no Cpus_allowed_list");
    assert_eq!(allowed.trim(), "0");
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_io_priority_idle() {
    let child = ChildGuard::sleep();
    assert_eq!(
        set_io_priority(child.key(), child.boot, IoPriority::Idle),
        Ok(())
    );
    let out = Command::new("ionice")
        .args(["-p", &child.pid().to_string()])
        .output()
        .expect("run ionice");
    assert!(out.status.success(), "ionice failed: {out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "idle");
}

/// The pids whose parent (stat field 4) is `ppid`.
fn children_of(ppid: u32) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir("/proc").expect("read /proc").flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        if stat_field(pid, 4).and_then(|s| s.parse().ok()) == Some(ppid) {
            out.insert(pid);
        }
    }
    out
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_kill_tree_parent_two_children() {
    let mut parent = ChildGuard::spawn("sh", &["-c", "sleep 60 & sleep 60 & wait"]);
    let mut kids = BTreeSet::new();
    assert!(
        wait_for(Duration::from_secs(5), || {
            // A forked child runs `sh` until it execs `sleep`, so wait for both to be `sleep`.
            kids = children_of(parent.pid());
            kids.len() == 2
                && kids.iter().all(|&pid| {
                    std::fs::read_to_string(format!("/proc/{pid}/comm"))
                        .is_ok_and(|c| c.trim() == "sleep")
                })
        }),
        "children {kids:?}"
    );
    let boot = parent.boot;
    parent.grandchildren = kids.iter().map(|&pid| key_of(pid, boot)).collect();
    let mut keys = vec![parent.key()];
    keys.extend(parent.grandchildren.iter().copied());

    let results = kill_tree(&keys, boot);
    assert_eq!(results, vec![Ok(()), Ok(()), Ok(())]);

    let status = parent.wait();
    assert_eq!(status.signal(), Some(9), "parent wait status {status:?}");
    // The grandchildren are reparented and reaped by the nearest subreaper.
    assert!(
        wait_for(Duration::from_secs(2), || keys
            .iter()
            .all(|&k| !is_alive(k))),
        "a target still runs: {:?}",
        keys.iter().filter(|&&k| is_alive(k)).collect::<Vec<_>>()
    );
}

/// True when `CapEff` in `/proc/self/status` has `cap`.
fn has_effective_cap(cap: u32) -> bool {
    let status = std::fs::read_to_string("/proc/self/status").expect("read own status");
    let eff = status
        .lines()
        .find_map(|l| l.strip_prefix("CapEff:"))
        .expect("no CapEff");
    let bits = u64::from_str_radix(eff.trim(), 16).expect("parse CapEff");
    bits & (1 << cap) != 0
}

/// The first user-space process with real and effective uid 0, skipping PID 1, kernel threads
/// and zombies.
fn find_root_process() -> Option<u32> {
    for entry in std::fs::read_dir("/proc").expect("read /proc").flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == 1 {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue;
        };
        let root = status
            .lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .is_some_and(|uids| uids.split_whitespace().take(2).all(|u| u == "0"));
        if !root {
            continue;
        }
        let Some(fields) = stat_fields(pid) else {
            continue;
        };
        let flags: u64 = fields[6].parse().unwrap_or(PF_KTHREAD);
        if fields[0] != "Z" && flags & PF_KTHREAD == 0 {
            return Some(pid);
        }
    }
    None
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_other_user_process_permission_denied() {
    if rustix::process::geteuid().is_root() || has_effective_cap(CAP_SYS_NICE) {
        eprintln!("skipped: this process may change the nice value of any process");
        return;
    }
    let Some(pid) = find_root_process() else {
        eprintln!("skipped: no root-owned user-space process found");
        return;
    };
    eprintln!("root process pid {pid}");
    let key = key_of(pid, boot_id());
    assert_eq!(
        set_nice(key, key.boot_id, 10),
        Err(OpsError::PermissionDenied)
    );
}

/// Numbers the test scopes of this process, so parallel tests get distinct unit names.
static SCOPE_COUNTER: AtomicU32 = AtomicU32::new(0);

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// The cgroup v2 path of `pid` relative to the cgroup root. None when the process is gone.
fn cgroup_of(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("0::"))?;
    Some(line.trim_start_matches('/').to_owned())
}

/// A `CgroupRef` for `rel_path` with the directory's current inode.
fn cgroup_ref(rel_path: &str) -> CgroupRef {
    let meta = std::fs::metadata(format!("{CGROUP_ROOT}/{rel_path}")).expect("stat cgroup dir");
    CgroupRef {
        rel_path: Arc::from(rel_path),
        cgroup_id: meta.ino(),
    }
}

/// The `frozen` value in the cgroup's `cgroup.events`. None when the file is gone or has no
/// valid `frozen` line.
fn events_frozen(rel_path: &str) -> Option<bool> {
    let text = std::fs::read_to_string(format!("{CGROUP_ROOT}/{rel_path}/cgroup.events")).ok()?;
    match text.lines().find_map(|l| l.strip_prefix("frozen "))? {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// True when the cgroup has no processes. A removed cgroup counts as empty.
fn procs_empty(rel_path: &str) -> bool {
    match std::fs::read_to_string(format!("{CGROUP_ROOT}/{rel_path}/cgroup.procs")) {
        Ok(text) => text.trim().is_empty(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A transient user scope `tasklens-test-<pid>-<n>.scope` running `sleep 60`. On drop it kills
/// the scope through `kill_group`, reaps the child and stops the unit, so no test scope stays
/// behind even when an assertion fails.
struct ScopeGuard {
    child: ChildGuard,
    unit: String,
    cg: CgroupRef,
}

impl ScopeGuard {
    fn start() -> Self {
        let n = SCOPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let unit = format!("tasklens-test-{}-{n}", std::process::id());
        let unit_arg = format!("--unit={unit}");
        // `systemd-run --scope` moves itself into the new scope and then execs the command, so
        // the child pid becomes the `sleep` pid.
        let child = ChildGuard::spawn(
            "systemd-run",
            &["--user", "--scope", "--quiet", &unit_arg, "sleep", "60"],
        );
        let pid = child.pid();
        eprintln!("unit {unit}.scope pid {pid}");
        let suffix = format!("/{unit}.scope");
        let mut path = None;
        let found = wait_for(Duration::from_secs(2), || {
            path = cgroup_of(pid);
            path.as_deref().is_some_and(|p| p.ends_with(&suffix))
        });
        let mut guard = ScopeGuard {
            child,
            unit,
            cg: CgroupRef {
                rel_path: Arc::from(""),
                cgroup_id: 0,
            },
        };
        assert!(found, "the child did not move into {suffix}: {path:?}");
        let rel_path = path.expect("cgroup path");
        let u = uid();
        let app_slice = format!("user.slice/user-{u}.slice/user@{u}.service/app.slice/");
        assert!(
            rel_path.starts_with(&app_slice),
            "systemd put the scope outside app.slice: {rel_path}"
        );
        guard.cg = cgroup_ref(&rel_path);
        guard
    }

    fn rel_path(&self) -> &str {
        &self.cg.rel_path
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if self.cg.cgroup_id != 0 {
            let _ = kill_group(&self.cg, uid());
        }
        // `ChildGuard` kills and reaps the child itself; reap it here so the scope empties
        // before systemd is asked to stop it.
        if !self.child.reaped {
            let _ = self.child.child.kill();
            let _ = self.child.child.wait();
            self.child.reaped = true;
        }
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{}.scope", self.unit)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_cgroup_freeze_thaw_kill() {
    let mut scope = ScopeGuard::start();
    let u = uid();

    assert_eq!(freeze_group(&scope.cg, u), Ok(()));
    assert_eq!(events_frozen(scope.rel_path()), Some(true));

    assert_eq!(thaw_group(&scope.cg, u), Ok(()));
    assert!(
        wait_for(Duration::from_secs(2), || events_frozen(scope.rel_path())
            == Some(false)),
        "frozen after thaw: {:?}",
        events_frozen(scope.rel_path())
    );

    assert_eq!(kill_group(&scope.cg, u), Ok(()));
    let status = scope.child.wait();
    assert_eq!(status.signal(), Some(9), "wait status {status:?}");
    assert!(
        wait_for(Duration::from_secs(2), || procs_empty(scope.rel_path())),
        "the scope still has processes"
    );
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_cgroup_stale_inode_refused() {
    let scope = ScopeGuard::start();
    let stale = CgroupRef {
        rel_path: scope.cg.rel_path.clone(),
        cgroup_id: scope.cg.cgroup_id + 1,
    };
    assert_eq!(freeze_group(&stale, uid()), Err(OpsError::IdentityChanged));
    assert_eq!(events_frozen(scope.rel_path()), Some(false));
    assert_eq!(kill_group(&scope.cg, uid()), Ok(()));
}

/// Regression test: tasklens never freezes its own session or app scope.
#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_cgroup_session_scope_refused() {
    let own = own_cgroup().expect("read own cgroup");
    eprintln!("own cgroup leaf {}", own.rsplit('/').next().unwrap_or(""));
    let cg = cgroup_ref(&own);
    let result = freeze_group(&cg, uid());
    assert!(
        matches!(result, Err(OpsError::NotOwner) | Err(OpsError::Protected)),
        "result {result:?}"
    );
}

/// A directory the test creates with mode `0700` and removes on drop, even when an assertion
/// fails.
struct TempDir(PathBuf);

impl TempDir {
    /// Creates `<parent>/tasklens-test-<pid>-<name>`. The parent must exist.
    fn create_in(parent: &Path, name: &str) -> Self {
        let path = parent.join(format!("tasklens-test-{}-{name}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create test directory");
        TempDir(path)
    }

    /// A directory under cargo's per-target scratch directory, never under the real config.
    fn scratch(name: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(parent).expect("create target tmp directory");
        Self::create_in(parent, name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The permission bits of `path`, without following a symlink.
fn file_mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).expect("stat file").mode() & 0o7777
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_write_atomic_in_xdg_config() {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".config")
        });
    let dir = TempDir(config_home.join(format!("tasklens-test-{}", std::process::id())));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir.0)
        .expect("create test directory");
    assert_eq!(file_mode(&dir.0), 0o700);

    let path = dir.0.join("settings.toml");
    let bytes = b"[general]\nrefresh_ms = 1000\n";
    assert_eq!(write_atomic(&path, bytes, PRIVATE_FILE_MODE), Ok(()));
    assert_eq!(std::fs::read(&path).expect("read back"), bytes);
    assert_eq!(file_mode(&path), 0o600);

    let entries: Vec<_> = std::fs::read_dir(&dir.0)
        .expect("list test directory")
        .map(|e| e.expect("dir entry").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("settings.toml")]);
}

/// Every `[Desktop Entry]` line of `path` with surrounding spaces removed.
fn desktop_entry_lines(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).expect("read desktop file");
    let mut in_entry = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            lines.push(line.trim().to_string());
        }
    }
    lines
}

/// Regression test: the override goes only under the given root, never the real
/// `~/.config/autostart`.
#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_autostart_disable_enable_roundtrip() {
    let system_dir = Path::new("/etc/xdg/autostart");
    let mut names: Vec<_> = std::fs::read_dir(system_dir)
        .map(|it| {
            it.filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|n| Path::new(n).extension().is_some_and(|x| x == "desktop"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let Some(name) = names.first() else {
        eprintln!("skip: /etc/xdg/autostart has no .desktop file");
        return;
    };

    let tmp = TempDir::scratch("autostart");
    let system_copy_dir = tmp.0.join("system");
    std::fs::create_dir(&system_copy_dir).expect("create system copy directory");
    let system_file = system_copy_dir.join(name);
    std::fs::copy(system_dir.join(name), &system_file).expect("copy system entry");

    let config_home = tmp.0.join("config");
    std::fs::create_dir(&config_home).expect("create config home");
    let roots = StartupRoots::from_config_home(&config_home);

    let target = autostart_disable_system(&system_file, &roots).expect("disable");
    assert_eq!(target, roots.autostart.join(name));
    assert_eq!(file_mode(&roots.autostart), 0o700);
    assert_eq!(file_mode(&target), 0o600);
    let lines = desktop_entry_lines(&target);
    assert!(lines.iter().any(|l| l == "Hidden=true"), "no Hidden=true");
    assert!(
        lines.iter().any(|l| l == "X-Tasklens-Override=true"),
        "no X-Tasklens-Override=true"
    );

    assert_eq!(
        autostart_enable_override(&target, &roots),
        Ok(EnableOutcome::OverrideDeleted)
    );
    assert!(!target.exists(), "override still present after enable");
}

/// Field 6 (`session`) of `/proc/<pid>/stat`.
fn session_of(pid: u32) -> u32 {
    stat_field(pid, 6)
        .expect("read stat")
        .parse()
        .expect("parse session")
}

/// A child of `spawn_detached`, killed through its pidfd and reaped on drop if the test has not
/// reaped it.
struct SpawnedGuard {
    child: Spawned,
    reaped: bool,
}

impl SpawnedGuard {
    fn kill_and_wait(&mut self) -> ExitKind {
        rustix::process::pidfd_send_signal(&self.child.pidfd, rustix::process::Signal::KILL)
            .expect("kill spawned child");
        let exit = wait_exit(&self.child).expect("wait spawned child");
        self.reaped = true;
        exit
    }
}

impl Drop for SpawnedGuard {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = rustix::process::pidfd_send_signal(
                &self.child.pidfd,
                rustix::process::Signal::KILL,
            );
            let _ = wait_exit(&self.child);
        }
    }
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_spawn_detached_survives_parent_session() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let argv = vec!["/usr/bin/sleep".to_string(), "30".to_string()];
    let mut guard = SpawnedGuard {
        child: spawn_detached(&argv, Some(&home)).expect("spawn sleep"),
        reaped: false,
    };
    let pid = guard.child.pid;

    assert_eq!(session_of(pid), pid);
    assert_ne!(session_of(pid), session_of(std::process::id()));
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/cwd")).expect("read child cwd"),
        home
    );

    assert_eq!(guard.kill_and_wait(), ExitKind::Signaled(9));
}

#[test]
#[ignore = "real system: run with --ignored on the owner machine"]
fn real_dump_as_root_writes_core() {
    if !rustix::process::geteuid().is_root() {
        eprintln!("skip: the dump test needs root");
        return;
    }
    if !Path::new(GDB_PATH).exists() {
        eprintln!("skip: {GDB_PATH} is not installed");
        return;
    }

    let child = ChildGuard::sleep();
    let tmp = TempDir::scratch("dump");
    let out = tmp.0.join("sleep.core");
    assert_eq!(
        dump_to_path(child.key(), child.boot, DumpMode::Mini, &out),
        Ok(())
    );

    let meta = std::fs::metadata(&out).expect("stat core");
    assert!(meta.len() > 0, "core file is empty");
    assert_eq!(meta.mode() & 0o7777, 0o600);
    let mut magic = [0u8; 4];
    std::fs::File::open(&out)
        .expect("open core")
        .read_exact(&mut magic)
        .expect("read core header");
    assert_eq!(magic, [0x7f, b'E', b'L', b'F']);
}
