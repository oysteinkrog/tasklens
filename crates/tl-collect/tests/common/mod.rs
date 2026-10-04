//! Shared helpers for `tl-collect` integration tests: fixture paths, fixture copies that a test
//! can rewrite between two samples, and proptest strategies (ADR-013).
//!
//! Each `tests/*.rs` file pulls this in with `mod common;` and uses only some helpers.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use proptest::sample::Index;
use tl_collect::SysRoot;

/// Bytes that `mutated` inserts: the separators and digits procfs parsers split on.
const INSERT_BYTES: &[u8] = b"\n \t():-0123456789";

/// <CARGO_MANIFEST_DIR>/tests/fixtures/<name>
///
/// An absolute `name` is used as is, so a test can build a throwaway fixture in a tempdir.
pub fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// SysRoot::at(fixture_dir(name))
pub fn fixture(name: &str) -> SysRoot {
    SysRoot::at(&fixture_dir(name))
}

/// SysRoot::at(fixture_dir(name).join(format!("step{n}")))
pub fn fixture_step(name: &str, n: u32) -> SysRoot {
    SysRoot::at(&step_dir(name, n))
}

/// Bytes of <fixture>/<rel>; panics with the path in the message if missing (tests only).
pub fn fixture_bytes(name: &str, rel: &str) -> Vec<u8> {
    let path = fixture_dir(name).join(rel);
    match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => panic!("fixture file {}: {e}", path.display()),
    }
}

/// Copies a fixture (or one step of it) into a fresh tempdir so a test can rewrite files in place
/// between two samples. Symlinks are copied as symlinks.
pub fn fixture_copy(name: &str, step: Option<u32>) -> tempfile::TempDir {
    let src = match step {
        Some(n) => step_dir(name, n),
        None => fixture_dir(name),
    };
    let dst = tempfile::tempdir().expect("create tempdir");
    copy_tree(&src, dst.path());
    dst
}

/// Overwrites the files of `dst` with those of fixture `name` step `n`, in place (same inodes).
pub fn apply_step(dst: &Path, name: &str, n: u32) {
    let src = step_dir(name, n);
    for rel in walk(&src) {
        let from = src.join(&rel);
        let to = dst.join(&rel);
        let kind = fs::symlink_metadata(&from)
            .expect("stat step entry")
            .file_type();
        if kind.is_dir() {
            fs::create_dir_all(&to).expect("create step dir");
        } else if kind.is_symlink() {
            if fs::symlink_metadata(&to).is_err() {
                let target = fs::read_link(&from).expect("read step symlink");
                std::os::unix::fs::symlink(target, &to).expect("create step symlink");
            }
        } else {
            let bytes = fs::read(&from).expect("read step file");
            // Truncate and rewrite the existing file. Never delete and recreate it, so a
            // `KeptFile` holding it sees the new content.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&to)
                .expect("open copy file for writing");
            std::io::Write::write_all(&mut file, &bytes).expect("write copy file");
        }
    }
}

/// Strategy: take `seed`, cut it at a random point, flip up to 16 random bytes, insert up to 8
/// bytes drawn from b"\n \t():-0123456789", and sometimes append 0 to 512 random bytes.
pub fn mutated(seed: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    (
        any::<Index>(),
        prop::collection::vec((any::<Index>(), any::<u8>()), 0..=16),
        prop::collection::vec((any::<Index>(), prop::sample::select(INSERT_BYTES)), 0..=8),
        prop::option::of(prop::collection::vec(any::<u8>(), 0..=512)),
    )
        .prop_map(move |(cut, flips, inserts, tail)| {
            let mut bytes = seed.clone();
            bytes.truncate(cut.index(bytes.len() + 1));
            if !bytes.is_empty() {
                for (at, mask) in flips {
                    let i = at.index(bytes.len());
                    bytes[i] ^= mask;
                }
            }
            for (at, byte) in inserts {
                let i = at.index(bytes.len() + 1);
                bytes.insert(i, byte);
            }
            if let Some(tail) = tail {
                bytes.extend_from_slice(&tail);
            }
            bytes
        })
}

/// Strategy: 0..=8192 arbitrary bytes.
pub fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=8192)
}

fn step_dir(name: &str, n: u32) -> PathBuf {
    fixture_dir(name).join(format!("step{n}"))
}

/// Every entry under `root`, relative to it, each directory before its contents. Symlinks are
/// listed but not followed.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = root.join(&rel);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => panic!("read fixture dir {}: {e}", dir.display()),
        };
        for entry in entries {
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

fn copy_tree(src: &Path, dst: &Path) {
    for rel in walk(src) {
        let from = src.join(&rel);
        let to = dst.join(&rel);
        let kind = fs::symlink_metadata(&from)
            .expect("stat fixture entry")
            .file_type();
        if kind.is_dir() {
            fs::create_dir_all(&to).expect("create copy dir");
        } else if kind.is_symlink() {
            let target = fs::read_link(&from).expect("read fixture symlink");
            std::os::unix::fs::symlink(target, &to).expect("create copy symlink");
        } else {
            fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}
