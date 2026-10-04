//! `open_validated_at` against a fake cgroup root built in a temp dir.

use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tl_model::actions::CgroupRef;
use tl_ops::OpsError;
use tl_ops::cgroup::open_validated_at;

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
