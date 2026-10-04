//! Layout checks for the captured `owner-desktop` fixture (ADR-013, ADR-015).
//!
//! The fixture is a fake root with `proc/` and `sys/`, captured by
//! `cargo xtask capture-fixture owner-desktop`. These tests never write to it.

use std::fs;
use std::path::{Path, PathBuf};

/// Names that must never appear anywhere in a fixture.
const FORBIDDEN: &[&str] = &[
    "environ",
    "maps",
    "smaps",
    "smaps_rollup",
    "mem",
    "fdinfo",
    "fd",
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/owner-desktop")
}

/// Every entry under `root`, relative to it. Symlinks are listed but not followed.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(root.join(&rel)).expect("read fixture dir") {
            let entry = entry.expect("read fixture dir entry");
            let child = rel.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                stack.push(child.clone());
            }
            out.push(child);
        }
    }
    out
}

/// The numeric `proc/<pid>` directories in the fixture.
fn pid_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root.join("proc"))
        .expect("read fixture proc/")
        .map(|e| e.expect("read proc/ entry"))
        .filter(|e| e.file_type().expect("file type").is_dir())
        .filter(|e| {
            e.file_name()
                .as_encoded_bytes()
                .iter()
                .all(u8::is_ascii_digit)
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn owner_desktop_has_core_files() {
    let root = fixture();
    for rel in [
        "proc/stat",
        "proc/meminfo",
        "proc/diskstats",
        "proc/net/dev",
        "sys/devices/system/cpu/online",
    ] {
        assert!(root.join(rel).is_file(), "missing {rel}");
    }
    assert!(
        pid_dirs(&root).iter().any(|d| d.join("stat").is_file()),
        "no proc/<pid>/stat"
    );
}

#[test]
fn no_forbidden_files() {
    for rel in walk(&fixture()) {
        for comp in rel.iter() {
            let name = comp.to_str().unwrap();
            assert!(
                !FORBIDDEN.contains(&name),
                "forbidden entry {}",
                rel.display()
            );
        }
    }
}

#[test]
fn cmdlines_are_scrubbed() {
    let dirs = pid_dirs(&fixture());
    assert!(!dirs.is_empty());
    for dir in dirs {
        let cmdline = dir.join("cmdline");
        if !cmdline.exists() {
            continue;
        }
        let bytes = fs::read(&cmdline).unwrap();
        let comm = fs::read(dir.join("comm")).unwrap();
        let comm = comm.strip_suffix(b"\n").unwrap_or(&comm);
        let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
        assert_eq!(
            parts,
            [comm, b"--arg".as_slice(), b"".as_slice()],
            "{}",
            cmdline.display()
        );
    }
}

#[test]
fn file_list_snapshot() {
    let mut lines: Vec<String> = walk(&fixture())
        .iter()
        .map(|rel| {
            let mut comps: Vec<String> =
                rel.iter().map(|c| c.to_str().unwrap().to_owned()).collect();
            if comps.len() >= 2
                && comps[0] == "proc"
                && comps[1].bytes().all(|b| b.is_ascii_digit())
            {
                comps[1] = "<pid>".to_owned();
            }
            comps.join("/")
        })
        .collect();
    lines.sort();
    lines.dedup();
    insta::assert_snapshot!(lines.join("\n"));
}
