//! Property tests for the system side of `tl-collect` (ADR-013): kept file reads, counter rules
//! and the suspend check, plus self-checks of the shared fixture helpers.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use common::{apply_step, fixture, fixture_bytes, fixture_copy, fixture_step, mutated};
use proptest::prelude::*;
use tl_collect::counters::{Clock, Counter, resumed_between};
use tl_collect::sysroot::KeptFile;
use tl_model::validity::Validity;

proptest! {
    #[test]
    fn prop_kept_file_returns_exact_bytes(
        bytes in mutated(fixture_bytes("owner-desktop", "proc/stat"))
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stat");
        fs::write(&path, &bytes).unwrap();
        let mut kept = KeptFile::new(path);
        let mut buf = Vec::new();
        prop_assert_eq!(kept.read(&mut buf).unwrap(), bytes.len());
        prop_assert_eq!(&buf, &bytes);
    }

    #[test]
    fn prop_counter_sequences(
        steps in prop::collection::vec((any::<u64>(), any::<u64>(), any::<u32>()), 0..=64)
    ) {
        let mut counter = Counter::default();
        for (value, t_ns, generation) in steps {
            let rate = counter.update(value, t_ns, generation);
            if rate.validity == Validity::Valid {
                prop_assert!(rate.per_sec.is_finite());
                prop_assert!(rate.per_sec >= 0.0);
            }
        }
    }

    #[test]
    fn prop_resumed_between_no_panic(
        a in (any::<u64>(), any::<u64>()),
        b in (any::<u64>(), any::<u64>()),
    ) {
        let prev = Clock { boot_ns: a.0, mono_ns: a.1 };
        let cur = Clock { boot_ns: b.0, mono_ns: b.1 };
        resumed_between(prev, cur);
        resumed_between(cur, prev);
    }
}

#[test]
fn owner_desktop_proc_stat_exists() {
    assert!(fixture("owner-desktop").proc_path("stat").exists());
}

#[test]
fn fixture_step_paths() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-step");
    let root = fixture_step("two-step", 1);
    assert_eq!(root.proc, base.join("step1/proc"));
    assert_eq!(root.sys, base.join("step1/sys"));
    assert_eq!(root.run, base.join("step1/run"));
    assert_eq!(root.proc_path("stat"), base.join("step1/proc/stat"));
}

#[test]
fn apply_step_keeps_inodes() {
    // A two-step fixture in a tempdir; the helpers take an absolute fixture name as is.
    let src = tempfile::tempdir().unwrap();
    let write = |rel: &str, content: &str| {
        let path = src.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write("step0/proc/stat", "cpu 1 2 3\n");
    write("step0/sys/class/x/value", "10\n");
    write("step1/proc/stat", "cpu 4 5 6 7 8\n");
    write("step1/sys/class/x/value", "2\n");
    std::os::unix::fs::symlink("../proc/stat", src.path().join("step0/sys/link")).unwrap();
    std::os::unix::fs::symlink("../proc/stat", src.path().join("step1/sys/link")).unwrap();
    let name = src.path().to_str().unwrap();

    let copy = fixture_copy(name, Some(0));
    let files: Vec<PathBuf> = ["proc/stat", "sys/class/x/value"]
        .iter()
        .map(|rel| copy.path().join(rel))
        .collect();
    let link = copy.path().join("sys/link");
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("../proc/stat"));
    assert_eq!(fs::read(&files[0]).unwrap(), b"cpu 1 2 3\n");
    let inodes: Vec<u64> = files
        .iter()
        .map(|p| fs::metadata(p).unwrap().ino())
        .collect();

    // A kept fd opened on step 0 sees the step 1 content after apply_step.
    let mut kept = KeptFile::new(files[0].clone());
    let mut buf = Vec::new();
    kept.read(&mut buf).unwrap();
    assert_eq!(buf, b"cpu 1 2 3\n");

    apply_step(copy.path(), name, 1);

    assert_eq!(fs::read(&files[0]).unwrap(), b"cpu 4 5 6 7 8\n");
    assert_eq!(fs::read(&files[1]).unwrap(), b"2\n");
    for (path, ino) in files.iter().zip(&inodes) {
        assert_eq!(fs::metadata(path).unwrap().ino(), *ino);
    }
    kept.read(&mut buf).unwrap();
    assert_eq!(buf, b"cpu 4 5 6 7 8\n");
}

// Parser cases: added by E04-A3.
