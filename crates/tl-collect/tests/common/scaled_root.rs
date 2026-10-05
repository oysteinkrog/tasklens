//! The 2,000-process fake root for the sweep bench and its 15 ms gate (ADR-016, ADR-014).
//!
//! Included with `#[path]` from `benches/sweep.rs` and `tests/fixture_process.rs`, so it uses
//! only `std`, `tempfile` and `tl-collect`.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_collect::apps::classify::ProtectInputs;
use tl_collect::proc::meta::NumericUserNames;
use tl_collect::proc::sweep::{ProcessSampler, SweepCtx, SweepInputs, SweepOutput};
use tl_model::time::BootId;

/// Pid directories in the scaled root.
pub const SCALED_PIDS: usize = 2_000;

/// The first pid given to a replica. Replica `i` gets `REPLICA_PID_BASE + i`.
pub const REPLICA_PID_BASE: u32 = 100_000;

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A sampler on the scaled root after two warm-up sweeps, with the dirs it reads from.
pub struct Scaled {
    pub sampler: ProcessSampler,
    pub out: SweepOutput,
    /// Kept so the root and the icon cache live as long as the sampler.
    pub root_dir: tempfile::TempDir,
    pub cache_dir: tempfile::TempDir,
}

impl Scaled {
    /// One sweep as the bench measures it: no PSS, no detail keys.
    pub fn sweep(&mut self, want_fd_count: bool) {
        let protect = ProtectInputs::default();
        let inp = SweepInputs {
            protect: &protect,
            detail_keys: &[],
            want_pss: false,
            window_pids: None,
            wm_class: None,
        };
        self.sampler
            .sweep(&sweep_ctx(want_fd_count), &inp, &mut self.out);
    }
}

pub fn sweep_ctx(want_fd_count: bool) -> SweepCtx {
    SweepCtx {
        boot_id: BootId(1),
        self_uid: 1000,
        ticks_per_sec: 100,
        online_cpus: 32,
        want_fd_count,
    }
}

/// Builds the scaled root on disk under `CARGO_TARGET_TMPDIR` (not tmpfs `/tmp`), opens a
/// sampler on it and runs two warm-up sweeps so every key has a baseline and metadata.
pub fn scaled() -> Scaled {
    let root_dir = tempfile::Builder::new()
        .prefix("sweep-2000-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("create scaled root dir");
    build_scaled_root(root_dir.path());
    let cache_dir = tempfile::Builder::new()
        .prefix("sweep-icons-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("create icon cache dir");

    let root = SysRoot::at(root_dir.path());
    let sampler = ProcessSampler::new(
        root,
        &app_paths(cache_dir.path()),
        Box::new(NumericUserNames::new()),
    )
    .expect("open sampler");
    let mut s = Scaled {
        sampler,
        out: SweepOutput::default(),
        root_dir,
        cache_dir,
    };
    s.sweep(false);
    s.sweep(false);
    assert_eq!(s.out.records.len(), SCALED_PIDS, "every pid swept");
    s
}

fn app_paths(cache: &Path) -> AppPaths {
    let share = |name: &str| fixture_dir(name).join("usr/share");
    let dirs = [share("apps-desktop"), share("apps-icons")];
    AppPaths {
        home: fixture_dir("apps-icons").join("home"),
        xdg_data_home: None,
        xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
        xdg_config_home: None,
        icon_cache: cache.join("icons.toml"),
    }
}

/// Writes `<dst>/proc/<pid>/` for the `owner-desktop` processes plus replicas until there are
/// exactly `SCALED_PIDS` pid directories, and `<dst>/sys/fs/cgroup`. Replica `i` copies
/// original `i % n` with pid `REPLICA_PID_BASE + i` and start ticks offset by `i`.
pub fn build_scaled_root(dst: &Path) {
    let src = fixture_dir("owner-desktop");
    let src_proc = src.join("proc");
    let mut pids: Vec<u32> = fs::read_dir(&src_proc)
        .expect("read owner-desktop proc")
        .filter_map(|e| e.expect("proc entry").file_name().to_str()?.parse().ok())
        .collect();
    pids.sort_unstable();
    assert!(
        !pids.is_empty() && pids.len() <= SCALED_PIDS,
        "owner-desktop has {} pids",
        pids.len()
    );

    let dst_proc = dst.join("proc");
    for &pid in &pids {
        copy_tree(
            &src_proc.join(pid.to_string()),
            &dst_proc.join(pid.to_string()),
        );
    }
    for i in 0..SCALED_PIDS - pids.len() {
        let orig = pids[i % pids.len()];
        let new_pid = REPLICA_PID_BASE + u32::try_from(i).expect("replica index fits u32");
        let from = src_proc.join(orig.to_string());
        let to = dst_proc.join(new_pid.to_string());
        copy_tree(&from, &to);
        let stat = fs::read_to_string(from.join("stat")).expect("read stat");
        fs::write(to.join("stat"), replica_stat(&stat, new_pid, i as u64)).expect("write stat");
        let status = fs::read_to_string(from.join("status")).expect("read status");
        fs::write(to.join("status"), replica_status(&status, new_pid)).expect("write status");
    }

    // owner-desktop was captured without a cgroup tree. The sampler needs the directory to
    // open, so an empty one stands in when the fixture has none.
    let cg_src = src.join("sys/fs/cgroup");
    let cg_dst = dst.join("sys/fs/cgroup");
    fs::create_dir_all(&cg_dst).expect("create cgroup dir");
    if cg_src.is_dir() {
        copy_tree(&cg_src, &cg_dst);
    }

    let count = fs::read_dir(&dst_proc)
        .expect("read scaled proc")
        .filter(|e| {
            let e = e.as_ref().expect("scaled proc entry");
            e.file_name()
                .to_str()
                .is_some_and(|n| n.parse::<u32>().is_ok())
                && e.file_type().expect("file type").is_dir()
        })
        .count();
    assert_eq!(count, SCALED_PIDS, "pid directories in the scaled root");
}

/// `stat` with field 1 (pid) set to `pid` and field 22 (start_ticks) increased by `offset`.
fn replica_stat(stat: &str, pid: u32, offset: u64) -> String {
    let open = stat.find(" (").expect("stat has a comm");
    let close = stat.rfind(')').expect("stat has a close paren");
    let mut fields: Vec<String> = stat[close + 2..]
        .trim_end()
        .split(' ')
        .map(str::to_owned)
        .collect();
    // fields[0] is field 3.
    let start: u64 = fields[22 - 3].parse().expect("start_ticks is a number");
    fields[22 - 3] = (start + offset).to_string();
    format!("{pid}{} {}\n", &stat[open..=close], fields.join(" "))
}

/// `status` with the `Tgid:` and `Pid:` lines set to `pid`.
fn replica_status(status: &str, pid: u32) -> String {
    let mut out = String::with_capacity(status.len() + 8);
    for line in status.lines() {
        if line.starts_with("Tgid:") {
            out.push_str(&format!("Tgid:\t{pid}"));
        } else if line.starts_with("Pid:") {
            out.push_str(&format!("Pid:\t{pid}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Copies `src` into `dst` recursively. Symlinks are copied as symlinks.
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dir");
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let kind = entry.file_type().expect("file type");
        if kind.is_dir() {
            copy_tree(&from, &to);
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&from).expect("read link"), &to)
                .expect("create symlink");
        } else {
            fs::copy(&from, &to).expect("copy file");
        }
    }
}
