//! `open_validated_at`, the control writes and the frozen wait against a fake cgroup root built
//! in a temp dir.

use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tl_model::actions::CgroupRef;
use tl_ops::OpsError;
use tl_ops::cgroup::{CgroupDir, freeze, freeze_group, kill, open_validated_at, thaw, wait_frozen};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temp dir that is removed on drop.
struct FakeRoot {
    path: PathBuf,
}

impl FakeRoot {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tl-ops-cgroup-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create fake root");
        FakeRoot { path }
    }

    /// Creates the directory chain for `rel` and returns the leaf's inode.
    fn make(&self, rel: &str) -> u64 {
        let dir = self.path.join(rel);
        std::fs::create_dir_all(&dir).expect("create cgroup dirs");
        std::fs::metadata(&dir).expect("stat cgroup dir").ino()
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FakeRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

fn app_slice(uid: u32) -> String {
    format!("user.slice/user-{uid}.slice/user@{uid}.service/app.slice")
}

fn leaf_path(uid: u32) -> String {
    format!("{}/app-test@1.scope", app_slice(uid))
}

fn cg(rel: &str, cgroup_id: u64) -> CgroupRef {
    CgroupRef {
        rel_path: Arc::from(rel),
        cgroup_id,
    }
}

fn unrelated_own(uid: u32) -> String {
    format!("user.slice/user-{uid}.slice/session-2.scope")
}

#[test]
fn fake_root_valid_opens() {
    let root = FakeRoot::new();
    let u = uid();
    let rel = leaf_path(u);
    let ino = root.make(&rel);
    let dir = open_validated_at(root.path(), &cg(&rel, ino), u, &unrelated_own(u)).unwrap();
    let st = rustix::fs::fstat(dir.as_fd()).unwrap();
    assert_eq!(st.st_ino as u64, ino);
}

#[test]
fn fake_root_inode_mismatch_is_identity_changed() {
    // Regression: a cgroup recreated with the same name has a new inode and is refused.
    let root = FakeRoot::new();
    let u = uid();
    let rel = leaf_path(u);
    let ino = root.make(&rel);
    let err = open_validated_at(root.path(), &cg(&rel, ino + 1), u, &unrelated_own(u)).unwrap_err();
    assert_eq!(err, OpsError::IdentityChanged);
}

#[test]
fn fake_root_owner_mismatch_is_not_owner() {
    // The path is valid for uid U+1, but the directories belong to U.
    let root = FakeRoot::new();
    let other = uid().wrapping_add(1);
    let rel = leaf_path(other);
    let ino = root.make(&rel);
    let err =
        open_validated_at(root.path(), &cg(&rel, ino), other, &unrelated_own(other)).unwrap_err();
    assert_eq!(err, OpsError::NotOwner);
}

#[test]
fn fake_root_missing_is_identity_changed() {
    let root = FakeRoot::new();
    let u = uid();
    let rel = leaf_path(u);
    let ino = root.make(&rel);
    std::fs::remove_dir(root.path().join(&rel)).unwrap();
    let err = open_validated_at(root.path(), &cg(&rel, ino), u, &unrelated_own(u)).unwrap_err();
    assert_eq!(err, OpsError::IdentityChanged);
}

#[test]
fn own_cgroup_inside_target_is_protected() {
    let root = FakeRoot::new();
    let u = uid();
    let rel = leaf_path(u);
    let ino = root.make(&rel);
    let own = format!("{rel}/sub");
    let err = open_validated_at(root.path(), &cg(&rel, ino), u, &own).unwrap_err();
    assert_eq!(err, OpsError::Protected);
    let err = open_validated_at(root.path(), &cg(&rel, ino), u, &rel).unwrap_err();
    assert_eq!(err, OpsError::Protected);
}

#[test]
fn symlinked_leaf_is_refused() {
    let root = FakeRoot::new();
    let u = uid();
    let rel = leaf_path(u);
    root.make(&rel);
    let other = format!("{}/other.scope", app_slice(u));
    let other_ino = root.make(&other);
    let leaf = root.path().join(&rel);
    std::fs::remove_dir(&leaf).unwrap();
    symlink(root.path().join(&other), &leaf).unwrap();
    // The ref carries the target's inode, so only `O_NOFOLLOW` stops the open.
    let result = open_validated_at(root.path(), &cg(&rel, other_ino), u, &unrelated_own(u));
    assert!(result.is_err(), "symlinked leaf must not open");
}

/// A validated fake cgroup dir with an empty file for each of `files`.
fn fake_dir(root: &FakeRoot, files: &[&str]) -> (PathBuf, CgroupDir) {
    let u = uid();
    let rel = leaf_path(u);
    let ino = root.make(&rel);
    let path = root.path().join(&rel);
    for name in files {
        std::fs::write(path.join(name), b"").expect("create control file");
    }
    let dir = open_validated_at(root.path(), &cg(&rel, ino), u, &unrelated_own(u))
        .expect("open fake cgroup dir");
    (path, dir)
}

#[test]
fn fake_freeze_thaw_kill_write_values() {
    let root = FakeRoot::new();
    let (path, dir) = fake_dir(&root, &["cgroup.freeze", "cgroup.kill", "cgroup.events"]);
    freeze(&dir).unwrap();
    assert_eq!(std::fs::read(path.join("cgroup.freeze")).unwrap(), b"1");
    thaw(&dir).unwrap();
    assert_eq!(std::fs::read(path.join("cgroup.freeze")).unwrap(), b"0");
    kill(&dir).unwrap();
    assert_eq!(std::fs::read(path.join("cgroup.kill")).unwrap(), b"1");
}

#[test]
fn wait_frozen_sees_late_frozen() {
    let root = FakeRoot::new();
    let (path, dir) = fake_dir(&root, &[]);
    let events = path.join("cgroup.events");
    std::fs::write(&events, b"populated 1\nfrozen 0\n").unwrap();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(120));
        std::fs::write(&events, b"populated 1\nfrozen 1\n").unwrap();
    });
    let start = Instant::now();
    let result = wait_frozen(&dir, Duration::from_secs(2), Duration::from_millis(50));
    let elapsed = start.elapsed();
    writer.join().unwrap();
    assert_eq!(result, Ok(()));
    assert!(elapsed < Duration::from_millis(300), "took {elapsed:?}");
}

#[test]
fn wait_frozen_times_out_outcome_unknown() {
    let root = FakeRoot::new();
    let (path, dir) = fake_dir(&root, &[]);
    std::fs::write(path.join("cgroup.events"), b"populated 1\nfrozen 0\n").unwrap();
    let start = Instant::now();
    let result = wait_frozen(&dir, Duration::from_millis(200), Duration::from_millis(50));
    let elapsed = start.elapsed();
    assert_eq!(
        result,
        Err(OpsError::OutcomeUnknown("freeze still in progress"))
    );
    assert!(elapsed >= Duration::from_millis(200), "took {elapsed:?}");
}

#[test]
fn kill_without_cgroup_kill_is_unsupported() {
    let root = FakeRoot::new();
    let (_path, dir) = fake_dir(&root, &["cgroup.freeze", "cgroup.events"]);
    assert_eq!(
        kill(&dir),
        Err(OpsError::Unsupported("cgroup.kill not available"))
    );
}

#[test]
fn freeze_group_invalid_ref_writes_nothing() {
    // A session scope fails the path check before any file is opened, so nothing is written.
    let u = uid();
    let rel = format!("user.slice/user-{u}.slice/session-2.scope");
    assert_eq!(freeze_group(&cg(&rel, 1), u), Err(OpsError::NotOwner));
}
