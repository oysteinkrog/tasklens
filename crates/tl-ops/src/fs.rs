//! Atomic file writes and startup entry files.
//!
//! `write_atomic` writes `<name>.tmp-<pid>` in the target's directory, syncs it, renames it over
//! the target and syncs the directory (ADR-011). A crash leaves either the old file or the new
//! one, never a partial file.
//!
//! The startup functions enable and disable autostart entries and Plasma scripts without ever
//! editing a system file (SYS-022, SYS-024, ADR-016). Desktop files are untrusted input: the
//! code only edits the named keys and never runs anything from them. Every function checks that
//! its target sits directly in one of the user's startup directories before any I/O.

use std::ffi::{OsStr, OsString};
use std::fs::{DirBuilder, OpenOptions};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use crate::error::{OpsError, OpsResult};

/// The mode for settings, state and every other private file tasklens writes (ADR-011).
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Replaces `path` with `bytes` atomically. The new file gets exactly `mode`, whatever the
/// umask. The parent directory must exist.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> OpsResult {
    let result = write_atomic_inner(path, bytes, mode);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    tracing::debug!(file = %name, ok = result.is_ok(), error = ?result.as_ref().err(), "atomic write");
    result
}

fn write_atomic_inner(path: &Path, bytes: &[u8], mode: u32) -> OpsResult {
    let tmp = temp_path(path)?;
    let mode = Mode::from_raw_mode(mode);
    let fd = create_temp(&tmp, mode)?;
    if let Err(e) = fill_and_rename(fd, &tmp, path, bytes, mode) {
        let _ = rustix::fs::unlink(&tmp);
        return Err(e);
    }
    // A bare file name has an empty parent, which means the current directory.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dir = rustix::fs::open(
        parent,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    rustix::fs::fsync(&dir)?;
    Ok(())
}

/// Creates `tmp` exclusively. A leftover file from an earlier run with the same pid is removed
/// and the create is tried once more.
fn create_temp(tmp: &Path, mode: Mode) -> OpsResult<OwnedFd> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC;
    match rustix::fs::open(tmp, flags, mode) {
        Ok(fd) => Ok(fd),
        Err(Errno::EXIST) => {
            rustix::fs::unlink(tmp)?;
            Ok(rustix::fs::open(tmp, flags, mode)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Sets the mode, writes all of `bytes`, syncs and closes the file, then renames it over
/// `path`.
fn fill_and_rename(fd: OwnedFd, tmp: &Path, path: &Path, bytes: &[u8], mode: Mode) -> OpsResult {
    rustix::fs::fchmod(&fd, mode)?;
    let mut rest = bytes;
    while !rest.is_empty() {
        match rustix::io::write(&fd, rest) {
            Ok(0) => return Err(OpsError::Os(libc::EIO)),
            Ok(n) => rest = &rest[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    rustix::fs::fsync(&fd)?;
    drop(fd);
    rustix::fs::rename(tmp, path)?;
    Ok(())
}

/// `<parent>/<file name>.tmp-<pid>`.
pub(crate) fn temp_path(path: &Path) -> OpsResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or(OpsError::InvalidInput("path has no file name"))?;
    let parent = path
        .parent()
        .ok_or(OpsError::InvalidInput("path has no parent"))?;
    let mut tmp_name = OsString::from(name);
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    Ok(parent.join(tmp_name))
}

/// The user directories that hold startup entries. All are under `$XDG_CONFIG_HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupRoots {
    /// `$XDG_CONFIG_HOME/autostart`
    pub autostart: PathBuf,
    /// `$XDG_CONFIG_HOME/systemd/user`
    pub systemd_user: PathBuf,
    /// `$XDG_CONFIG_HOME/plasma-workspace/env`
    pub plasma_env: PathBuf,
    /// `$XDG_CONFIG_HOME/plasma-workspace/shutdown`
    pub plasma_shutdown: PathBuf,
}

impl StartupRoots {
    /// The four roots under `config_home`, which is `$XDG_CONFIG_HOME`.
    pub fn from_config_home(config_home: &Path) -> Self {
        StartupRoots {
            autostart: config_home.join("autostart"),
            systemd_user: config_home.join("systemd").join("user"),
            plasma_env: config_home.join("plasma-workspace").join("env"),
            plasma_shutdown: config_home.join("plasma-workspace").join("shutdown"),
        }
    }
}

/// What `autostart_enable_override` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableOutcome {
    /// The file was a tasklens override and is gone, so the system entry applies again.
    OverrideDeleted,
    /// The file was rewritten with `Hidden=false`.
    Rewritten,
}

/// Desktop files larger than this are refused (R-165).
pub const MAX_DESKTOP_FILE: u64 = 1024 * 1024;

/// The key that marks a user override written by `autostart_disable_system` (SYS-022).
const OVERRIDE_KEY: &str = "X-Tasklens-Override";

/// The mode for a new `autostart` directory.
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Disables a system autostart entry by writing a user override with `Hidden=true` and the
/// tasklens marker. Returns the path of the new user file.
pub fn autostart_disable_system(system_file: &Path, roots: &StartupRoots) -> OpsResult<PathBuf> {
    let result = disable_system_inner(system_file, roots);
    log_result("autostart disable system", system_file, &result);
    result
}

fn disable_system_inner(system_file: &Path, roots: &StartupRoots) -> OpsResult<PathBuf> {
    let name = system_file.file_name().ok_or(OpsError::NotOwner)?;
    let target = roots.autostart.join(name);
    confine_desktop(&target, roots)?;
    let (text, _) = read_desktop(system_file)?;
    match rustix::fs::lstat(&target) {
        Ok(_) => return Err(OpsError::InvalidInput("user entry already exists")),
        Err(Errno::NOENT) => {}
        Err(e) => return Err(e.into()),
    }
    let text = desktop_set_key(&text, "Hidden", "true")?;
    let text = desktop_set_key(&text, OVERRIDE_KEY, "true")?;
    DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIR_MODE)
        .create(&roots.autostart)?;
    write_atomic(&target, text.as_bytes(), PRIVATE_FILE_MODE)?;
    Ok(target)
}

/// Enables an autostart entry that has a user file. A tasklens override is deleted; any other
/// user file gets `Hidden=false` and loses `X-GNOME-Autostart-enabled=false`.
pub fn autostart_enable_override(
    user_file: &Path,
    roots: &StartupRoots,
) -> OpsResult<EnableOutcome> {
    let result = enable_override_inner(user_file, roots);
    log_result("autostart enable", user_file, &result);
    result
}

fn enable_override_inner(user_file: &Path, roots: &StartupRoots) -> OpsResult<EnableOutcome> {
    confine_desktop(user_file, roots)?;
    let (text, mode) = read_desktop(user_file)?;
    if desktop_get_key(&text, OVERRIDE_KEY) == Some("true") {
        rustix::fs::unlink(user_file)?;
        return Ok(EnableOutcome::OverrideDeleted);
    }
    let mut text = desktop_set_key(&text, "Hidden", "false")?;
    if desktop_get_key(&text, "X-GNOME-Autostart-enabled") == Some("false") {
        text = desktop_remove_key(&text, "X-GNOME-Autostart-enabled")?;
    }
    write_atomic(user_file, text.as_bytes(), mode)?;
    Ok(EnableOutcome::Rewritten)
}

/// Sets `Hidden=true` or `Hidden=false` in a user autostart file, keeping its mode.
pub fn autostart_set_hidden(user_file: &Path, hidden: bool, roots: &StartupRoots) -> OpsResult {
    let result = set_hidden_inner(user_file, hidden, roots);
    log_result("autostart set hidden", user_file, &result);
    result
}

fn set_hidden_inner(user_file: &Path, hidden: bool, roots: &StartupRoots) -> OpsResult {
    confine_desktop(user_file, roots)?;
    let (text, mode) = read_desktop(user_file)?;
    let value = if hidden { "true" } else { "false" };
    let text = desktop_set_key(&text, "Hidden", value)?;
    write_atomic(user_file, text.as_bytes(), mode)
}

/// Enables a Plasma env or shutdown script by setting the owner's execute bit, or disables it
/// by clearing all three execute bits.
pub fn script_set_enabled(script: &Path, enabled: bool, roots: &StartupRoots) -> OpsResult {
    let result = script_set_enabled_inner(script, enabled, roots);
    log_result("script set enabled", script, &result);
    result
}

fn script_set_enabled_inner(script: &Path, enabled: bool, roots: &StartupRoots) -> OpsResult {
    confine(script, &[&roots.plasma_env, &roots.plasma_shutdown])?;
    let stat = rustix::fs::lstat(script)?;
    if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink {
        return Err(OpsError::NotOwner);
    }
    let mode = stat.st_mode & 0o7777;
    let mode = if enabled { mode | 0o100 } else { mode & !0o111 };
    rustix::fs::chmodat(
        rustix::fs::CWD,
        script,
        Mode::from_raw_mode(mode),
        AtFlags::empty(),
    )?;
    Ok(())
}

/// Deletes a user startup file. A symlink is removed itself, never followed.
pub fn delete_startup_file(path: &Path, roots: &StartupRoots) -> OpsResult {
    let result = delete_inner(path, roots);
    log_result("delete startup file", path, &result);
    result
}

fn delete_inner(path: &Path, roots: &StartupRoots) -> OpsResult {
    confine(
        path,
        &[
            &roots.autostart,
            &roots.systemd_user,
            &roots.plasma_env,
            &roots.plasma_shutdown,
        ],
    )?;
    match rustix::fs::unlink(path) {
        Ok(()) => Ok(()),
        Err(Errno::NOENT) => Err(OpsError::Os(libc::ENOENT)),
        Err(e) => Err(e.into()),
    }
}

/// Logs the file name and the result, never the contents.
fn log_result<T>(op: &str, path: &Path, result: &OpsResult<T>) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    tracing::debug!(file = %name, ok = result.is_ok(), error = ?result.as_ref().err(), "{op}");
}

/// Checks that `path` is an autostart `.desktop` file directly in `roots.autostart`.
fn confine_desktop(path: &Path, roots: &StartupRoots) -> OpsResult {
    confine(path, &[&roots.autostart])?;
    let name = path.file_name().ok_or(OpsError::NotOwner)?;
    if !name.as_bytes().ends_with(b".desktop") {
        return Err(OpsError::NotOwner);
    }
    Ok(())
}

/// Checks that the parent of `path` equals one of `roots`, comparing components without
/// resolving anything, and that the file name is not `.` or `..`.
fn confine(path: &Path, roots: &[&Path]) -> OpsResult {
    let name = path.file_name().ok_or(OpsError::NotOwner)?;
    if name == OsStr::new(".") || name == OsStr::new("..") {
        return Err(OpsError::NotOwner);
    }
    let parent = path.parent().ok_or(OpsError::NotOwner)?;
    if roots
        .iter()
        .any(|root| parent.components().eq(root.components()))
    {
        Ok(())
    } else {
        Err(OpsError::NotOwner)
    }
}

/// Reads a desktop file of at most `MAX_DESKTOP_FILE` bytes. Returns the text and the file's
/// permission bits. `O_NONBLOCK` keeps a FIFO from blocking the read.
fn read_desktop(path: &Path) -> OpsResult<(String, u32)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let mode = file.metadata()?.mode() & 0o7777;
    let mut bytes = Vec::new();
    file.take(MAX_DESKTOP_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DESKTOP_FILE {
        return Err(OpsError::InvalidInput("desktop file too large"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| OpsError::InvalidInput("desktop file is not UTF-8"))?;
    Ok((text, mode))
}

/// The line range of the `[Desktop Entry]` group: the header line up to the next line that
/// starts with `[`, or the end.
fn desktop_entry_group(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines
        .iter()
        .position(|l| l.trim_end_matches(['\n', '\r']) == "[Desktop Entry]")?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    Some((start, end))
}

/// Splits a `Key=value` line into the key and the value, both without surrounding spaces or
/// tabs. Comments and group headers give `None`.
fn split_key_line(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') || line.starts_with('[') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((
        key.trim_matches([' ', '\t']),
        value.trim_matches([' ', '\t', '\r', '\n']),
    ))
}

fn line_has_key(line: &str, key: &str) -> bool {
    split_key_line(line).is_some_and(|(k, _)| k == key)
}

/// Sets `key=value` in the `[Desktop Entry]` group. The first matching line is replaced and
/// later duplicates removed; with no match the line goes after the group's last non-blank
/// line. Every other line is kept byte for byte.
pub(crate) fn desktop_set_key(text: &str, key: &str, value: &str) -> OpsResult<String> {
    if [key, value].iter().any(|s| s.contains(['\n', '\r'])) {
        return Err(OpsError::InvalidInput("key or value contains a line break"));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (start, end) =
        desktop_entry_group(&lines).ok_or(OpsError::InvalidInput("no [Desktop Entry] group"))?;
    let has_match = lines[start + 1..end].iter().any(|l| line_has_key(l, key));
    // The header line is not blank, so this always finds a line.
    let insert_after = (start..end)
        .rev()
        .find(|&i| !lines[i].trim().is_empty())
        .unwrap_or(start);
    let new_line = format!("{key}={value}\n");
    let mut out = String::with_capacity(text.len() + new_line.len() + 1);
    let mut replaced = false;
    for (i, line) in lines.iter().enumerate() {
        if i > start && i < end && line_has_key(line, key) {
            if !replaced {
                out.push_str(&new_line);
                replaced = true;
            }
            continue;
        }
        out.push_str(line);
        if i == insert_after && !has_match {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&new_line);
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Removes every `key` line in the `[Desktop Entry]` group.
pub(crate) fn desktop_remove_key(text: &str, key: &str) -> OpsResult<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (start, end) =
        desktop_entry_group(&lines).ok_or(OpsError::InvalidInput("no [Desktop Entry] group"))?;
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > start && i < end && line_has_key(line, key) {
            continue;
        }
        out.push_str(line);
    }
    Ok(out)
}

/// The trimmed value of the first `key` line in the `[Desktop Entry]` group.
pub(crate) fn desktop_get_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (start, end) = desktop_entry_group(&lines)?;
    lines[start + 1..end]
        .iter()
        .copied()
        .find_map(|l| split_key_line(l).filter(|(k, _)| *k == key).map(|(_, v)| v))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("tl-ops-fs-test-{}-{n}", std::process::id()));
            std::fs::create_dir(&dir).expect("create test dir");
            TestDir(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_files(dir: &Path) -> Vec<OsString> {
        std::fs::read_dir(dir)
            .expect("read test dir")
            .map(|e| e.expect("dir entry").file_name())
            .filter(|n| n.to_string_lossy().contains(".tmp-"))
            .collect()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).expect("stat").permissions().mode() & 0o7777
    }

    #[test]
    fn temp_path_appends_pid() {
        let tmp = temp_path(Path::new("/a/settings.toml")).expect("temp path");
        let want = format!("/a/settings.toml.tmp-{}", std::process::id());
        assert_eq!(tmp, PathBuf::from(want));
    }

    #[test]
    fn temp_path_without_file_name_is_invalid() {
        assert_eq!(
            temp_path(Path::new("/")),
            Err(OpsError::InvalidInput("path has no file name"))
        );
        assert_eq!(
            temp_path(Path::new("/a/..")),
            Err(OpsError::InvalidInput("path has no file name"))
        );
    }

    #[test]
    fn write_atomic_creates_with_mode_and_no_temp_left() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        write_atomic(&target, b"a = 1\n", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"a = 1\n");
        assert_eq!(mode_of(&target), 0o600);
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_replaces_by_rename() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        std::fs::write(&target, b"old").expect("seed");
        let old_ino = std::fs::metadata(&target).expect("stat").ino();
        write_atomic(&target, b"new", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"new");
        assert_ne!(std::fs::metadata(&target).expect("stat").ino(), old_ino);
    }

    #[test]
    fn write_atomic_removes_stale_temp() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        let stale = temp_path(&target).expect("temp path");
        std::fs::write(&stale, b"stale").expect("seed stale");
        write_atomic(&target, b"a = 1\n", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"a = 1\n");
        assert!(!stale.exists());
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_missing_parent_is_enoent() {
        let dir = TestDir::new();
        let missing = dir.0.join("missing");
        let target = missing.join("settings.toml");
        assert_eq!(
            write_atomic(&target, b"x", PRIVATE_FILE_MODE),
            Err(OpsError::Os(libc::ENOENT))
        );
        assert!(!missing.exists());
        assert_eq!(std::fs::read_dir(&dir.0).expect("read dir").count(), 0);
    }

    #[test]
    fn write_atomic_onto_directory_cleans_temp() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        std::fs::create_dir(&target).expect("create target dir");
        assert_eq!(
            write_atomic(&target, b"x", PRIVATE_FILE_MODE),
            Err(OpsError::Os(libc::EISDIR))
        );
        assert!(target.is_dir());
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_mode_ignores_umask() {
        // The umask is process-wide, so the test does not change it. The `fchmod` step makes
        // the result exact whatever the umask is.
        let dir = TestDir::new();
        let target = dir.0.join("autostart.desktop");
        write_atomic(&target, b"[Desktop Entry]\n", 0o640).expect("write");
        assert_eq!(mode_of(&target), 0o640);
    }

    #[test]
    fn write_atomic_large_payload() {
        let dir = TestDir::new();
        let target = dir.0.join("themes.json");
        let payload: Vec<u8> = (0..4 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        write_atomic(&target, &payload, PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), payload);
    }

    /// Startup roots under a test dir, with every root created.
    fn startup_roots(dir: &TestDir) -> StartupRoots {
        let roots = StartupRoots::from_config_home(&dir.0.join("config"));
        for root in [
            &roots.autostart,
            &roots.systemd_user,
            &roots.plasma_env,
            &roots.plasma_shutdown,
        ] {
            std::fs::create_dir_all(root).expect("create root");
        }
        roots
    }

    fn read_string(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    #[test]
    fn disable_system_writes_override_keeping_comments_and_groups() {
        let dir = TestDir::new();
        let roots = StartupRoots::from_config_home(&dir.0.join("config"));
        let system_dir = dir.0.join("etc-xdg-autostart");
        std::fs::create_dir(&system_dir).expect("create system dir");
        let system_file = system_dir.join("foo.desktop");
        let system_text = "[Desktop Entry]\nName=Foo\n# keep me\nExec=foo\n[Desktop Action new]\nExec=foo --new\n";
        std::fs::write(&system_file, system_text).expect("seed");

        let target = autostart_disable_system(&system_file, &roots).expect("disable");

        assert_eq!(target, roots.autostart.join("foo.desktop"));
        assert_eq!(
            read_string(&target),
            "[Desktop Entry]\nName=Foo\n# keep me\nExec=foo\nHidden=true\n\
             X-Tasklens-Override=true\n[Desktop Action new]\nExec=foo --new\n"
        );
        assert_eq!(mode_of(&target), 0o600);
        assert_eq!(mode_of(&roots.autostart), 0o700);
        assert_eq!(read_string(&system_file), system_text);
    }

    #[test]
    fn enable_deletes_tasklens_override() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let user_file = roots.autostart.join("foo.desktop");
        std::fs::write(
            &user_file,
            "[Desktop Entry]\nExec=foo\nHidden=true\nX-Tasklens-Override = true\n",
        )
        .expect("seed");

        assert_eq!(
            autostart_enable_override(&user_file, &roots),
            Ok(EnableOutcome::OverrideDeleted)
        );
        assert!(!user_file.exists());
    }

    #[test]
    fn enable_rewrites_foreign_override() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let user_file = roots.autostart.join("foo.desktop");
        std::fs::write(
            &user_file,
            "[Desktop Entry]\nExec=foo\nHidden=true\nX-GNOME-Autostart-enabled=false\n",
        )
        .expect("seed");
        std::fs::set_permissions(&user_file, std::fs::Permissions::from_mode(0o640))
            .expect("chmod");

        assert_eq!(
            autostart_enable_override(&user_file, &roots),
            Ok(EnableOutcome::Rewritten)
        );
        assert_eq!(
            read_string(&user_file),
            "[Desktop Entry]\nExec=foo\nHidden=false\n"
        );
        assert_eq!(mode_of(&user_file), 0o640);
    }

    #[test]
    fn set_hidden_ignores_localized_keys() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let user_file = roots.autostart.join("foo.desktop");
        std::fs::write(
            &user_file,
            "[Desktop Entry]\nHidden = false\nHidden[de]=x\nExec=foo\n",
        )
        .expect("seed");
        std::fs::set_permissions(&user_file, std::fs::Permissions::from_mode(0o644))
            .expect("chmod");

        autostart_set_hidden(&user_file, true, &roots).expect("set hidden");

        assert_eq!(
            read_string(&user_file),
            "[Desktop Entry]\nHidden=true\nHidden[de]=x\nExec=foo\n"
        );
        assert_eq!(mode_of(&user_file), 0o644);
    }

    #[test]
    fn disable_system_refuses_existing_user_entry() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let system_file = dir.0.join("foo.desktop");
        std::fs::write(&system_file, "[Desktop Entry]\nExec=foo\n").expect("seed system");
        let user_file = roots.autostart.join("foo.desktop");
        std::fs::write(&user_file, "[Desktop Entry]\nExec=bar\n").expect("seed user");

        assert_eq!(
            autostart_disable_system(&system_file, &roots),
            Err(OpsError::InvalidInput("user entry already exists"))
        );
        assert_eq!(read_string(&user_file), "[Desktop Entry]\nExec=bar\n");
    }

    #[test]
    fn paths_outside_roots_are_not_owner() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let text = "[Desktop Entry]\nExec=evil\n";
        // `autostart/../evil.desktop` resolves to this file.
        let evil = roots
            .autostart
            .parent()
            .expect("parent")
            .join("evil.desktop");
        std::fs::write(&evil, text).expect("seed evil");
        std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let other_dir = dir.0.join("other");
        std::fs::create_dir(&other_dir).expect("create other");
        let other = other_dir.join("x.desktop");
        std::fs::write(&other, text).expect("seed other");
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        // A file in autostart without the `.desktop` suffix.
        let plain = roots.autostart.join("evil.sh");
        std::fs::write(&plain, text).expect("seed plain");

        let dotdot = roots.autostart.join("..").join("evil.desktop");
        let dotdot_root = roots.autostart.join("..");
        let desktop_targets = [&dotdot, &other, &plain, &dotdot_root];
        for path in desktop_targets {
            assert_eq!(
                autostart_enable_override(path, &roots),
                Err(OpsError::NotOwner)
            );
            assert_eq!(
                autostart_set_hidden(path, true, &roots),
                Err(OpsError::NotOwner)
            );
        }
        // A system path ending in `..` has no file name to copy.
        assert_eq!(
            autostart_disable_system(&other_dir.join(".."), &roots),
            Err(OpsError::NotOwner)
        );
        assert_eq!(
            autostart_disable_system(&plain, &roots),
            Err(OpsError::NotOwner)
        );
        let env_dotdot = roots.plasma_env.join("..").join("evil.desktop");
        for path in [&env_dotdot, &other, &evil, &dotdot] {
            assert_eq!(
                script_set_enabled(path, true, &roots),
                Err(OpsError::NotOwner)
            );
            assert_eq!(delete_startup_file(path, &roots), Err(OpsError::NotOwner));
        }
        // An autostart file is not a script.
        assert_eq!(
            script_set_enabled(&plain, true, &roots),
            Err(OpsError::NotOwner)
        );

        assert_eq!(read_string(&evil), text);
        assert_eq!(mode_of(&evil), 0o644);
        assert_eq!(read_string(&other), text);
        assert_eq!(mode_of(&other), 0o644);
        assert_eq!(read_string(&plain), text);
        assert!(!roots.autostart.join("x.desktop").exists());
        assert!(!roots.autostart.join("evil.desktop").exists());
    }

    #[test]
    fn script_exec_bit_toggle() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let script = roots.plasma_env.join("env.sh");
        std::fs::write(&script, "export A=1\n").expect("seed");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        script_set_enabled(&script, true, &roots).expect("enable");
        assert_eq!(mode_of(&script), 0o744);
        script_set_enabled(&script, false, &roots).expect("disable");
        assert_eq!(mode_of(&script), 0o644);

        let shutdown = roots.plasma_shutdown.join("stop.sh");
        std::fs::write(&shutdown, "true\n").expect("seed");
        std::fs::set_permissions(&shutdown, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        script_set_enabled(&shutdown, false, &roots).expect("disable");
        assert_eq!(mode_of(&shutdown), 0o644);
    }

    #[test]
    fn script_symlink_is_not_owner() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let outside = dir.0.join("outside.sh");
        std::fs::write(&outside, "true\n").expect("seed");
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let link = roots.plasma_env.join("link.sh");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        assert_eq!(
            script_set_enabled(&link, true, &roots),
            Err(OpsError::NotOwner)
        );
        assert_eq!(mode_of(&outside), 0o644);
    }

    #[test]
    fn set_key_without_group_is_invalid() {
        let err = Err(OpsError::InvalidInput("no [Desktop Entry] group"));
        assert_eq!(desktop_set_key("Name=Foo\n", "Hidden", "true"), err);
        assert_eq!(
            desktop_set_key("[Desktop Action new]\nExec=x\n", "Hidden", "true"),
            err
        );
        assert_eq!(desktop_set_key("", "Hidden", "true"), err);
    }

    #[test]
    fn set_key_rejects_line_breaks() {
        assert_eq!(
            desktop_set_key("[Desktop Entry]\n", "Hidden", "true\nExec=evil"),
            Err(OpsError::InvalidInput("key or value contains a line break"))
        );
    }

    #[test]
    fn too_large_desktop_file_is_invalid() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let system_file = dir.0.join("big.desktop");
        let mut text = String::from("[Desktop Entry]\n");
        text.push_str(&"#".repeat(2 * 1024 * 1024));
        std::fs::write(&system_file, text).expect("seed");

        assert_eq!(
            autostart_disable_system(&system_file, &roots),
            Err(OpsError::InvalidInput("desktop file too large"))
        );
        assert!(!roots.autostart.join("big.desktop").exists());
    }

    #[test]
    fn non_utf8_desktop_file_is_invalid() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let system_file = dir.0.join("bad.desktop");
        std::fs::write(&system_file, b"[Desktop Entry]\nName=\xff\n").expect("seed");

        assert_eq!(
            autostart_disable_system(&system_file, &roots),
            Err(OpsError::InvalidInput("desktop file is not UTF-8"))
        );
    }

    #[test]
    fn delete_unlinks_symlink_not_target() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let outside = dir.0.join("outside.desktop");
        std::fs::write(&outside, "[Desktop Entry]\n").expect("seed");
        let link = roots.autostart.join("link.desktop");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        delete_startup_file(&link, &roots).expect("delete");

        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_eq!(read_string(&outside), "[Desktop Entry]\n");
        assert_eq!(
            delete_startup_file(&link, &roots),
            Err(OpsError::Os(libc::ENOENT))
        );
    }

    #[test]
    fn delete_removes_unit_file() {
        let dir = TestDir::new();
        let roots = startup_roots(&dir);
        let unit = roots.systemd_user.join("foo.service");
        std::fs::write(&unit, "[Unit]\n").expect("seed");

        delete_startup_file(&unit, &roots).expect("delete");
        assert!(!unit.exists());
    }

    #[test]
    fn set_key_removes_duplicates_and_inserts_at_group_end() {
        let text = "# head\n[Desktop Entry]\nHidden=false\nName=Foo\nHidden=x\n\n\
                    [Other]\nHidden=keep\n";
        assert_eq!(
            desktop_set_key(text, "Hidden", "true"),
            Ok("# head\n[Desktop Entry]\nHidden=true\nName=Foo\n\n[Other]\nHidden=keep\n".into())
        );

        // No match: insert after the last non-blank line, before the blank line.
        let text = "[Desktop Entry]\nName=Foo\n\n[Other]\nA=1\n";
        assert_eq!(
            desktop_set_key(text, "Hidden", "true"),
            Ok("[Desktop Entry]\nName=Foo\nHidden=true\n\n[Other]\nA=1\n".into())
        );

        // No trailing newline at the end of the group.
        assert_eq!(
            desktop_set_key("[Desktop Entry]\nName=Foo", "Hidden", "true"),
            Ok("[Desktop Entry]\nName=Foo\nHidden=true\n".into())
        );
        assert_eq!(
            desktop_set_key("[Desktop Entry]\nHidden=false", "Hidden", "true"),
            Ok("[Desktop Entry]\nHidden=true\n".into())
        );

        // A commented-out key does not match.
        assert_eq!(
            desktop_set_key("[Desktop Entry]\n#Hidden=true\n", "Hidden", "false"),
            Ok("[Desktop Entry]\n#Hidden=true\nHidden=false\n".into())
        );
    }

    #[test]
    fn remove_and_get_key_stay_in_group() {
        let text = "[Desktop Entry]\nA = 1 \nB=2\nA=3\nA[de]=4\n[Other]\nA=5\n";
        assert_eq!(desktop_get_key(text, "A"), Some("1"));
        assert_eq!(desktop_get_key(text, "C"), None);
        assert_eq!(desktop_get_key("[Other]\nA=5\n", "A"), None);
        assert_eq!(
            desktop_remove_key(text, "A"),
            Ok("[Desktop Entry]\nB=2\nA[de]=4\n[Other]\nA=5\n".into())
        );
    }
}
