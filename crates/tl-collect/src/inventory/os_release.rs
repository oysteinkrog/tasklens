//! Operating System section of the Information tab: os-release, kernel, machine ID, install date
//! and package manager version, plus the NIS domain for the System section (PERF-095, PERF-097,
//! ADR-016 "Information").
//!
//! Hostnames, user, desktop and session come from D-Bus and the environment in the engine (E12).
//! Every failure gives `None`, which the UI shows as "Not reported".

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::fs::{AtFlags, Mode, OFlags, StatxFlags};

use crate::sanitize::sanitize_text;

/// Read limit for os-release, in bytes.
const OS_RELEASE_MAX: u64 = 64 * 1024;
/// Most bytes kept from one string value.
const VALUE_MAX: usize = 256;
/// Read limit for the first line of pacman.log, in bytes.
const PACMAN_LOG_MAX: u64 = 4096;
/// Read limit for machine-id, in bytes.
const MACHINE_ID_MAX: u64 = 64;
/// Read limit for the `--version` output, in bytes.
const VERSION_OUTPUT_MAX: u64 = 4096;
/// Wall limit for one `--version` run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the child is polled while it runs.
const POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsRelease {
    /// `PRETTY_NAME`.
    pub pretty_name: Option<Arc<str>>,
    /// `NAME`.
    pub name: Option<Arc<str>>,
    /// `ID`.
    pub id: Option<Arc<str>>,
    /// `VERSION_ID`. `None` means the UI shows "Rolling".
    pub version_id: Option<Arc<str>>,
    /// `BUILD_ID`. `None` means "Not reported".
    pub build_id: Option<Arc<str>>,
    /// `LOGO`, an icon name the UI looks up in the icon theme.
    pub logo: Option<Arc<str>>,
}

/// Parses os-release text (freedesktop os-release syntax). Unknown keys are ignored.
pub fn parse_os_release(bytes: &[u8]) -> OsRelease {
    let mut out = OsRelease::default();
    for line in bytes.split(|&b| b == b'\n') {
        let line = line.trim_ascii_start();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let Some(eq) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let slot = match &line[..eq] {
            b"PRETTY_NAME" => &mut out.pretty_name,
            b"NAME" => &mut out.name,
            b"ID" => &mut out.id,
            b"VERSION_ID" => &mut out.version_id,
            b"BUILD_ID" => &mut out.build_id,
            b"LOGO" => &mut out.logo,
            _ => continue,
        };
        *slot = clean(&unquote(&line[eq + 1..]));
    }
    out
}

/// Reads `<etc>/os-release`, else `<usr_lib>/os-release`.
pub fn read_os_release(etc: &Path, usr_lib: &Path) -> Option<OsRelease> {
    let mut buf = Vec::new();
    [etc, usr_lib]
        .into_iter()
        .find(|dir| read_capped(&dir.join("os-release"), OS_RELEASE_MAX, &mut buf))
        .map(|_| parse_os_release(&buf))
}

#[derive(Clone, Debug, PartialEq)]
pub struct KernelInfo {
    /// `uname -r`.
    pub release: Arc<str>,
    /// `uname -v`.
    pub version: Arc<str>,
    /// Raw `uname -m`, for example "x86_64". The UI writes "x86_64 (64-bit)".
    pub machine: Arc<str>,
    /// NIS domain; `None` when empty or "(none)".
    pub nis_domain: Option<Arc<str>>,
}

/// Reads the kernel fields from `uname`.
pub fn read_kernel() -> KernelInfo {
    let uts = rustix::system::uname();
    let text = |raw: &[u8]| -> Arc<str> {
        let mut out = String::new();
        sanitize_text(raw, VALUE_MAX, &mut out);
        Arc::from(out)
    };
    let domain = uts.domainname().to_bytes();
    KernelInfo {
        release: text(uts.release().to_bytes()),
        version: text(uts.version().to_bytes()),
        machine: text(uts.machine().to_bytes()),
        nis_domain: (!domain.is_empty() && domain != b"(none)").then(|| text(domain)),
    }
}

/// The first 32 hex characters of `<etc>/machine-id`. Never log the result.
pub fn read_machine_id(etc: &Path) -> Option<Arc<str>> {
    let mut buf = Vec::new();
    if !read_capped(&etc.join("machine-id"), MACHINE_ID_MAX, &mut buf) {
        return None;
    }
    let id = buf.get(..32)?;
    if !id.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    // All ASCII hex digits, so this is valid UTF-8.
    std::str::from_utf8(id).ok().map(Arc::from)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallDateSource {
    /// Birth time of the root directory.
    RootBirthTime,
    /// Time stamp on the first line of pacman.log.
    PacmanLog,
}

/// Install date in Unix seconds and the source used: the birth time of `root`, else the first
/// line of `pacman_log`.
pub fn read_install_date(root: &Path, pacman_log: &Path) -> Option<(i64, InstallDateSource)> {
    read_install_date_with(root, pacman_log, true)
}

/// [`read_install_date`] with the birth time step switchable, so tests can reach the pacman.log
/// step on filesystems that report birth time.
fn read_install_date_with(
    root: &Path,
    pacman_log: &Path,
    use_birth_time: bool,
) -> Option<(i64, InstallDateSource)> {
    if use_birth_time && let Some(secs) = root_birth_time(root) {
        return Some((secs, InstallDateSource::RootBirthTime));
    }
    let mut buf = Vec::new();
    if !read_capped(pacman_log, PACMAN_LOG_MAX, &mut buf) {
        return None;
    }
    let first = buf.split(|&b| b == b'\n').next()?;
    parse_pacman_stamp(first).map(|secs| (secs, InstallDateSource::PacmanLog))
}

/// `statx(root, "", AT_EMPTY_PATH, STATX_BTIME)`; `None` when the filesystem does not report a
/// positive birth time.
fn root_birth_time(root: &Path) -> Option<i64> {
    let fd = rustix::fs::open(root, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).ok()?;
    let st = rustix::fs::statx(&fd, "", AtFlags::EMPTY_PATH, StatxFlags::BTIME).ok()?;
    let has_btime = st.stx_mask & StatxFlags::BTIME.bits() != 0;
    (has_btime && st.stx_btime.tv_sec > 0).then_some(st.stx_btime.tv_sec)
}

/// Parses a line that starts with `[YYYY-MM-DDTHH:MM:SS+HHMM]` (or `-HHMM`) into Unix seconds.
fn parse_pacman_stamp(line: &[u8]) -> Option<i64> {
    let s = line.get(..26)?;
    let shape_ok = s[0] == b'['
        && s[5] == b'-'
        && s[8] == b'-'
        && s[11] == b'T'
        && s[14] == b':'
        && s[17] == b':'
        && matches!(s[20], b'+' | b'-')
        && s[25] == b']';
    if !shape_ok {
        return None;
    }
    let num = |from: usize, len: usize| -> Option<i64> {
        s[from..from + len].iter().try_fold(0i64, |acc, &b| {
            b.is_ascii_digit().then(|| acc * 10 + i64::from(b - b'0'))
        })
    };
    let (year, month, day) = (num(1, 4)?, num(6, 2)?, num(9, 2)?);
    let (hour, min, sec) = (num(12, 2)?, num(15, 2)?, num(18, 2)?);
    let (off_h, off_m) = (num(21, 2)?, num(23, 2)?);
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && min < 60
        && sec < 61
        && off_h < 24
        && off_m < 60;
    if !valid {
        return None;
    }
    let offset = (off_h * 3600 + off_m * 60) * if s[20] == b'-' { -1 } else { 1 };
    let local = days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec;
    Some(local - offset)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The package managers we look for, in order: program, output label, the text before the
/// version in its `--version` output.
const PACKAGE_MANAGERS: [(&str, &str, &str); 3] = [
    ("pacman", "pacman", "Pacman v"),
    ("dpkg-query", "dpkg", "version "),
    ("rpm", "rpm", "RPM version "),
];

/// "pacman 7.0.0", "dpkg 1.21.22" or "rpm 4.19.1": the first of the three programs found in
/// `path_dirs`, run with `--version`. Blocks for up to 2 s.
pub fn package_manager_version(path_dirs: &[PathBuf]) -> Option<Arc<str>> {
    let (exe, label, marker) = PACKAGE_MANAGERS.iter().find_map(|&(prog, label, marker)| {
        path_dirs
            .iter()
            .map(|dir| dir.join(prog))
            .find(|p| p.is_file())
            .map(|p| (p, label, marker))
    })?;
    let output = run_version(&exe)?;
    let version = token_after(&output, marker.as_bytes())?;
    let mut text = String::new();
    sanitize_text(version, VALUE_MAX, &mut text);
    (!text.is_empty()).then(|| Arc::from(format!("{label} {text}")))
}

/// Runs `exe --version` with stdin and stderr null. `None` on a spawn error, a timeout or a
/// failed read.
fn run_version(exe: &Path) -> Option<Vec<u8>> {
    let mut child = spawn_version(exe)?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            // Timed out or failed. Do not read stdout here: a grandchild may still hold it open.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = Vec::new();
    child
        .stdout
        .take()?
        .take(VERSION_OUTPUT_MAX)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Spawns `exe --version`, retrying briefly on `ETXTBSY`. That error can happen when another
/// thread forked while `exe` was still open for writing.
fn spawn_version(exe: &Path) -> Option<Child> {
    for _ in 0..3 {
        let spawned = Command::new(exe)
            .arg("--version")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => return Some(child),
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => std::thread::sleep(POLL),
            Err(_) => return None,
        }
    }
    None
}

/// The whitespace-free token right after the first `marker` in `text`.
fn token_after<'a>(text: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    let start = memchr::memmem::find(text, marker)? + marker.len();
    let rest = &text[start..];
    let end = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Removes os-release quoting. Double quotes unescape `\"`, `\\`, `` \` `` and `\$`; single
/// quotes are literal; unquoted values are taken as is.
fn unquote(raw: &[u8]) -> Vec<u8> {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    match raw.first() {
        Some(b'"') => {
            let mut out = Vec::with_capacity(raw.len());
            let mut iter = raw[1..].iter().copied();
            while let Some(b) = iter.next() {
                match b {
                    b'"' => break,
                    b'\\' => match iter.next() {
                        Some(c @ (b'"' | b'\\' | b'`' | b'$')) => out.push(c),
                        Some(c) => out.extend_from_slice(&[b'\\', c]),
                        None => out.push(b'\\'),
                    },
                    _ => out.push(b),
                }
            }
            out
        }
        Some(b'\'') => {
            let inner = &raw[1..];
            let end = inner
                .iter()
                .position(|&b| b == b'\'')
                .unwrap_or(inner.len());
            inner[..end].to_vec()
        }
        _ => raw.to_vec(),
    }
}

/// Sanitizes a value; empty values become `None`.
fn clean(raw: &[u8]) -> Option<Arc<str>> {
    let mut out = String::new();
    sanitize_text(raw, VALUE_MAX, &mut out);
    (!out.is_empty()).then(|| Arc::from(out))
}

/// Reads at most `max` bytes of `path` into `buf`. False when the file cannot be read.
fn read_capped(path: &Path, max: u64, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(f) => f.take(max).read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dmi-desktop")
    }

    /// Writes an executable shell script named `name` into `dir`.
    fn script(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn parse_cachyos() {
        let r = parse_os_release(
            b"PRETTY_NAME=\"CachyOS Linux\"\nID=cachyos\nBUILD_ID=rolling\nLOGO=cachyos\n",
        );
        assert_eq!(r.pretty_name.as_deref(), Some("CachyOS Linux"));
        assert_eq!(r.id.as_deref(), Some("cachyos"));
        assert_eq!(r.build_id.as_deref(), Some("rolling"));
        assert_eq!(r.logo.as_deref(), Some("cachyos"));
        assert_eq!(r.version_id, None);

        let fx = read_os_release(&fixture().join("etc"), Path::new("/nonexistent")).unwrap();
        assert_eq!(fx.name.as_deref(), Some("CachyOS Linux"));
        assert_eq!(fx.version_id, None);
    }

    #[test]
    fn quoted_escapes() {
        let r = parse_os_release(b"NAME=\"A \\\"quoted\\\" \\\\ name\"\n");
        assert_eq!(r.name.as_deref(), Some("A \"quoted\" \\ name"));
        let r = parse_os_release(b"# c\nID='it\\s'\nVERSION_ID=\"\\$x\\`\"\nBUILD_ID=a b\n");
        assert_eq!(r.id.as_deref(), Some("it\\s"));
        assert_eq!(r.version_id.as_deref(), Some("$x`"));
        assert_eq!(r.build_id.as_deref(), Some("a b"));
    }

    #[test]
    fn usr_lib_fallback() {
        let etc = tempfile::tempdir().unwrap();
        let usr_lib = tempfile::tempdir().unwrap();
        std::fs::write(usr_lib.path().join("os-release"), "ID=fallback\n").unwrap();
        let r = read_os_release(etc.path(), usr_lib.path()).unwrap();
        assert_eq!(r.id.as_deref(), Some("fallback"));
        assert_eq!(read_os_release(etc.path(), etc.path()), None);
    }

    #[test]
    fn pacman_version_parse() {
        let dir = tempfile::tempdir().unwrap();
        script(
            dir.path(),
            "pacman",
            "echo ' .--.                  Pacman v7.0.0 - libalpm v15.0.0'",
        );
        let dirs = [PathBuf::from("/nonexistent"), dir.path().to_path_buf()];
        assert_eq!(
            package_manager_version(&dirs).as_deref(),
            Some("pacman 7.0.0")
        );

        let other = tempfile::tempdir().unwrap();
        script(other.path(), "rpm", "echo 'RPM version 4.19.1'");
        script(
            other.path(),
            "dpkg-query",
            "echo 'Debian dpkg-query package management program query tool version 1.21.22 (amd64).'",
        );
        let dirs = [other.path().to_path_buf()];
        assert_eq!(
            package_manager_version(&dirs).as_deref(),
            Some("dpkg 1.21.22")
        );
        assert_eq!(
            token_after(b"RPM version 4.19.1\n", b"RPM version "),
            Some(&b"4.19.1"[..])
        );
        assert_eq!(package_manager_version(&[]), None);
    }

    #[test]
    fn pacman_timeout() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "pacman", "sleep 5\necho 'Pacman v7.0.0'");
        let start = Instant::now();
        assert_eq!(package_manager_version(&[dir.path().to_path_buf()]), None);
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn pacman_log_date() {
        let log = fixture().join("var/log/pacman.log");
        assert_eq!(
            read_install_date_with(Path::new("/nonexistent"), &log, false),
            Some((1_714_551_753, InstallDateSource::PacmanLog))
        );
        assert_eq!(
            parse_pacman_stamp(b"[2024-05-01T10:22:33-0130] x"),
            Some(1_714_558_953 + 5400)
        );
        assert_eq!(
            parse_pacman_stamp(b"[2024-05-01 10:22] [PACMAN] old format"),
            None
        );
        assert_eq!(parse_pacman_stamp(b"[2024-13-01T10:22:33+0200]"), None);
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("pacman.log");
        std::fs::write(&bad, "garbage\n[2024-05-01T10:22:33+0200]\n").unwrap();
        assert_eq!(read_install_date_with(tmp.path(), &bad, false), None);
    }

    #[test]
    fn machine_id() {
        assert_eq!(
            read_machine_id(&fixture().join("etc")).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("machine-id"), "not-hex\n").unwrap();
        assert_eq!(read_machine_id(tmp.path()), None);
    }
}
