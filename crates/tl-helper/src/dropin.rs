//! `WriteServiceDropIn`: write or remove the recovery drop-in of a system service as root
//! (SYS-013, R-004, R-119, R-248).
//!
//! The only file this touches is `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`. The
//! helper never trusts the engine: it re-runs `validate_drop_in`, parses every value into a
//! `RecoveryEdit`, range-checks it, and builds the text itself with `tl_ops::dropin::render`.
//! Every refusal is `Err(NotAllowed)` and happens before any file is touched. The quoted rule
//! names in comments are for readers and tests only; they are never sent.
//!
//! The drop-in directory is opened with `O_NOFOLLOW` relative to a dirfd of the systemd
//! directory, so a symlink or other file put at `<unit>.d` is refused and nothing is written
//! through it. The file is then written through `/proc/self/fd/<dirfd>`, which names the
//! directory that was opened even if the name is swapped afterwards.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;
use tl_model::actions::RecoveryEdit;
use tl_model::helper_proto::{DropInKey, HelperError, HelperReplyBody, validate_drop_in};
use tl_ops::OpsError;

/// The one file name tasklens writes in a drop-in directory.
const DROPIN_FILE: &str = "50-tasklens-recovery.conf";

const DROPIN_DIR_MODE: u32 = 0o755;
const DROPIN_FILE_MODE: u32 = 0o644;

/// Longest unit name before its suffix, in bytes.
const MAX_UNIT_STEM_LEN: usize = 255;
const MAX_ON_FAILURE_UNITS: usize = 32;
const MAX_RESTART_USEC: u64 = 3_600_000_000;
const MAX_RESTART_STEPS: u32 = 1000;
const MAX_START_LIMIT_BURST: u32 = 1000;
const MAX_START_LIMIT_INTERVAL_USEC: u64 = 86_400_000_000;

const RESTART_WORDS: [&str; 7] = [
    "no",
    "on-success",
    "on-failure",
    "on-abnormal",
    "on-watchdog",
    "on-abort",
    "always",
];

const ACTION_WORDS: [&str; 9] = [
    "none",
    "reboot",
    "reboot-force",
    "reboot-immediate",
    "poweroff",
    "poweroff-force",
    "poweroff-immediate",
    "exit",
    "exit-force",
];

const ON_FAILURE_SUFFIXES: [&str; 8] = [
    ".service", ".socket", ".target", ".timer", ".mount", ".path", ".slice", ".scope",
];

/// Answers `WriteServiceDropIn`. Empty `keys` removes the drop-in (`UnitRemoveRecovery`).
/// Tests use `write_service_dropin_in` with a temp dir instead.
pub fn write_service_dropin(unit: &str, keys: &[(DropInKey, String)]) -> HelperReplyBody {
    /// The directory that holds system unit drop-ins.
    const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";
    let flags = OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC;
    match rustix::fs::open(SYSTEM_UNIT_DIR, flags, Mode::empty()) {
        Ok(root) => write_service_dropin_in(root.as_fd(), unit, keys),
        Err(e) => HelperReplyBody::Err(HelperError::Os(e.raw_os_error())),
    }
}

/// `write_service_dropin` with the systemd directory given as `root`. Tests pass a temp dir.
fn write_service_dropin_in(
    root: BorrowedFd<'_>,
    unit: &str,
    keys: &[(DropInKey, String)],
) -> HelperReplyBody {
    let result = check(unit, keys).and_then(|()| {
        let dir_name = format!("{unit}.d");
        if keys.is_empty() {
            remove(root, &dir_name)
        } else {
            write(root, &dir_name, &tl_ops::dropin::render(keys))
        }
    });
    match result {
        Ok(()) => HelperReplyBody::Ok,
        Err(e) => HelperReplyBody::Err(e),
    }
}

/// Every check, in order, before any file is touched.
fn check(unit: &str, keys: &[(DropInKey, String)]) -> Result<(), HelperError> {
    validate_drop_in(unit, keys)?;
    // Rule "bad unit name".
    if !unit_name_ok(unit) {
        return Err(HelperError::NotAllowed);
    }
    // Rule "bad key value".
    let edit = parse_keys(keys).ok_or(HelperError::NotAllowed)?;
    check_edit(&edit)
}

/// `^[A-Za-z0-9:_.\\@-]{1,255}\.service$`, not starting with '.'.
fn unit_name_ok(unit: &str) -> bool {
    let Some(stem) = unit.strip_suffix(".service") else {
        return false;
    };
    (1..=MAX_UNIT_STEM_LEN).contains(&stem.len())
        && !unit.starts_with('.')
        && stem.bytes().all(|b| unit_name_byte(b) || b == b'.')
}

/// A byte allowed in a unit name: ASCII letters and digits, ':', '_', '\', '@' and '-'. '.' is
/// checked by the callers.
fn unit_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'\\' | b'@' | b'-')
}

/// `^[A-Za-z0-9:_.\\@%-]{1,255}\.(service|socket|target|timer|mount|path|slice|scope)$`.
fn on_failure_unit_ok(token: &str) -> bool {
    ON_FAILURE_SUFFIXES.iter().any(|suffix| {
        token.strip_suffix(suffix).is_some_and(|stem| {
            (1..=MAX_UNIT_STEM_LEN).contains(&stem.len())
                && stem
                    .bytes()
                    .all(|b| unit_name_byte(b) || b == b'.' || b == b'%')
        })
    })
}

/// Parses the key list into a `RecoveryEdit`. `None` when a key is given twice or a value does
/// not parse.
fn parse_keys(keys: &[(DropInKey, String)]) -> Option<RecoveryEdit> {
    let mut edit = RecoveryEdit::default();
    for (key, value) in keys {
        let value = value.as_str();
        let fresh = match key {
            DropInKey::Restart => edit.restart.replace(Arc::from(value)).is_none(),
            DropInKey::RestartSec => edit.restart_usec.replace(parse_usec(value)?).is_none(),
            DropInKey::RestartSteps => edit.restart_steps.replace(parse_int(value)?).is_none(),
            DropInKey::RestartMaxDelaySec => edit
                .restart_max_delay_usec
                .replace(parse_usec(value)?)
                .is_none(),
            DropInKey::StartLimitBurst => {
                edit.start_limit_burst.replace(parse_int(value)?).is_none()
            }
            DropInKey::StartLimitIntervalSec => edit
                .start_limit_interval_usec
                .replace(parse_usec(value)?)
                .is_none(),
            DropInKey::StartLimitAction => {
                edit.start_limit_action.replace(Arc::from(value)).is_none()
            }
            DropInKey::FailureAction => edit.failure_action.replace(Arc::from(value)).is_none(),
            DropInKey::OnFailure => {
                let units: Arc<[Arc<str>]> = value.split(' ').map(Arc::from).collect();
                edit.on_failure.replace(units).is_none()
            }
        };
        if !fresh {
            return None;
        }
    }
    Some(edit)
}

/// A decimal integer: one or more ASCII digits, nothing else.
fn parse_int<T: std::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `<n>us`.
fn parse_usec(text: &str) -> Option<u64> {
    parse_int(text.strip_suffix("us")?)
}

/// The value rules of SYS-013 and UI-SYS-034.
fn check_edit(edit: &RecoveryEdit) -> Result<(), HelperError> {
    let in_range = |value: Option<u64>, max: u64| value.is_none_or(|v| v <= max);
    let word_ok = |value: &Option<Arc<str>>, words: &[&str]| {
        value.as_deref().is_none_or(|v| words.contains(&v))
    };
    let checks = [
        // Rule "bad Restart value".
        word_ok(&edit.restart, &RESTART_WORDS),
        // Rule "Restart* out of range".
        in_range(edit.restart_usec, MAX_RESTART_USEC)
            && in_range(edit.restart_max_delay_usec, MAX_RESTART_USEC)
            && in_range(edit.restart_steps.map(u64::from), MAX_RESTART_STEPS.into()),
        // Rule "StartLimit out of range".
        in_range(
            edit.start_limit_burst.map(u64::from),
            MAX_START_LIMIT_BURST.into(),
        ) && in_range(
            edit.start_limit_interval_usec,
            MAX_START_LIMIT_INTERVAL_USEC,
        ),
        // Rule "bad action value".
        word_ok(&edit.start_limit_action, &ACTION_WORDS)
            && word_ok(&edit.failure_action, &ACTION_WORDS),
        // Rule "bad OnFailure unit".
        edit.on_failure.as_deref().is_none_or(|units| {
            units.len() <= MAX_ON_FAILURE_UNITS && units.iter().all(|u| on_failure_unit_ok(u))
        }),
    ];
    if checks.iter().all(|ok| *ok) {
        Ok(())
    } else {
        Err(HelperError::NotAllowed)
    }
}

/// Writes the drop-in file, creating `<unit>.d` when it is missing.
fn write(root: BorrowedFd<'_>, dir_name: &str, text: &str) -> Result<(), HelperError> {
    let created = match rustix::fs::statat(root, dir_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => false,
        // Rule "drop-in directory is not a directory".
        Ok(_) => return Err(HelperError::NotAllowed),
        Err(Errno::NOENT) => {
            match rustix::fs::mkdirat(root, dir_name, Mode::from_raw_mode(DROPIN_DIR_MODE)) {
                Ok(()) => true,
                // Someone else made it. `open_dropin_dir` checks what it is.
                Err(Errno::EXIST) => false,
                Err(e) => return Err(os_error(e)),
            }
        }
        Err(e) => return Err(os_error(e)),
    };
    let dir = open_dropin_dir(root, dir_name)?;
    if created {
        // mkdirat applies the umask.
        rustix::fs::fchmod(&dir, Mode::from_raw_mode(DROPIN_DIR_MODE)).map_err(os_error)?;
    }
    let path = PathBuf::from(format!("/proc/self/fd/{}/{DROPIN_FILE}", dir.as_raw_fd()));
    tl_ops::write_atomic(&path, text.as_bytes(), DROPIN_FILE_MODE).map_err(ops_error)
}

/// Removes the drop-in file, then `<unit>.d` if that left it empty. Absent is fine.
fn remove(root: BorrowedFd<'_>, dir_name: &str) -> Result<(), HelperError> {
    let dir = match open_dropin_dir(root, dir_name) {
        Err(HelperError::Os(errno)) if errno == Errno::NOENT.raw_os_error() => return Ok(()),
        other => other?,
    };
    match rustix::fs::unlinkat(&dir, DROPIN_FILE, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => {}
        Err(e) => return Err(os_error(e)),
    }
    drop(dir);
    match rustix::fs::unlinkat(root, dir_name, AtFlags::REMOVEDIR) {
        // Other drop-ins are still in it.
        Ok(()) | Err(Errno::NOENT | Errno::NOTEMPTY | Errno::EXIST) => Ok(()),
        Err(e) => Err(os_error(e)),
    }
}

/// Opens `<unit>.d` without following a symlink. A symlink or a non-directory there is
/// `NotAllowed` (rule "drop-in directory is not a directory").
fn open_dropin_dir(root: BorrowedFd<'_>, dir_name: &str) -> Result<OwnedFd, HelperError> {
    let flags = OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC;
    match rustix::fs::openat(root, dir_name, flags, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(Errno::LOOP | Errno::NOTDIR) => Err(HelperError::NotAllowed),
        Err(e) => Err(os_error(e)),
    }
}

fn os_error(e: Errno) -> HelperError {
    match e {
        Errno::PERM | Errno::ACCESS => HelperError::PermissionDenied,
        e => HelperError::Os(e.raw_os_error()),
    }
}

fn ops_error(e: OpsError) -> HelperError {
    match e {
        OpsError::PermissionDenied => HelperError::PermissionDenied,
        OpsError::Os(errno) => HelperError::Os(errno),
        _ => HelperError::Os(Errno::IO.raw_os_error()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop. It stands in for
    /// /etc/systemd/system.
    struct TestRoot {
        path: PathBuf,
        fd: OwnedFd,
    }

    impl TestRoot {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("tl-helper-dropin-test-{}-{n}", std::process::id()));
            std::fs::create_dir(&path).expect("create test root");
            let flags = OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC;
            let fd = rustix::fs::open(&path, flags, Mode::empty()).expect("open test root");
            TestRoot { path, fd }
        }

        fn write(&self, unit: &str, keys: &[(DropInKey, String)]) -> HelperReplyBody {
            write_service_dropin_in(self.fd.as_fd(), unit, keys)
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.path)
                .expect("read test root")
                .map(|e| {
                    e.expect("dir entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn keys(pairs: &[(DropInKey, &str)]) -> Vec<(DropInKey, String)> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    fn restart() -> Vec<(DropInKey, String)> {
        keys(&[(DropInKey::Restart, "on-failure")])
    }

    const NOT_ALLOWED: HelperReplyBody = HelperReplyBody::Err(HelperError::NotAllowed);

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn unit_name_rules() {
        let root = TestRoot::new();
        let long = format!("{}.service", "a".repeat(256));
        for unit in [
            "../../etc/passwd",
            "sshd.service/x",
            ".hidden.service",
            "",
            ".service",
            "sshd",
            "sshd.socket",
            "a b.service",
            "a%n.service",
            "a\n.service",
            "a..b.service",
            long.as_str(),
        ] {
            assert_eq!(root.write(unit, &restart()), NOT_ALLOWED, "unit {unit:?}");
            assert_eq!(
                root.write(unit, &[]),
                NOT_ALLOWED,
                "remove for unit {unit:?}"
            );
        }
        assert!(root.entries().is_empty());
        for unit in ["sshd.service", "getty@tty1.service", r"sys-x\x2dy.service"] {
            assert!(unit_name_ok(unit), "unit {unit:?}");
        }
    }

    #[test]
    fn duplicate_key_refused() {
        let root = TestRoot::new();
        let twice = keys(&[
            (DropInKey::Restart, "always"),
            (DropInKey::RestartSec, "5us"),
            (DropInKey::Restart, "on-failure"),
        ]);
        assert_eq!(root.write("sshd.service", &twice), NOT_ALLOWED);
        assert!(root.entries().is_empty());
    }

    #[test]
    fn newline_injection_refused() {
        let root = TestRoot::new();
        let cases = [
            keys(&[(DropInKey::Restart, "always\nExecStart=/bin/sh")]),
            keys(&[(DropInKey::OnFailure, "a.service\n[Service]")]),
            keys(&[(DropInKey::OnFailure, "a.service [Service]")]),
            keys(&[(DropInKey::FailureAction, "reboot\rExecStart=/bin/sh")]),
            keys(&[(DropInKey::RestartSec, "5us\n")]),
        ];
        for case in &cases {
            assert_eq!(root.write("sshd.service", case), NOT_ALLOWED, "{case:?}");
        }
        assert!(root.entries().is_empty());
    }

    #[test]
    fn value_rules() {
        let bad = [
            (DropInKey::Restart, "sometimes"),
            (DropInKey::Restart, ""),
            (DropInKey::RestartSec, "5"),
            (DropInKey::RestartSec, "5s"),
            (DropInKey::RestartSec, "-5us"),
            (DropInKey::RestartSec, "+5us"),
            (DropInKey::RestartSec, "us"),
            (DropInKey::RestartSec, "3600000001us"),
            (DropInKey::RestartMaxDelaySec, "3600000001us"),
            (DropInKey::RestartSteps, "1001"),
            (DropInKey::RestartSteps, "1us"),
            (DropInKey::StartLimitBurst, "1001"),
            (DropInKey::StartLimitBurst, "99999999999999999999"),
            (DropInKey::StartLimitIntervalSec, "86400000001us"),
            (DropInKey::StartLimitAction, "halt"),
            (DropInKey::FailureAction, "Reboot"),
            (DropInKey::OnFailure, ""),
            (DropInKey::OnFailure, "a.service  b.service"),
            (DropInKey::OnFailure, " a.service"),
            (DropInKey::OnFailure, "a.device"),
            (DropInKey::OnFailure, "../x.service"),
        ];
        for (key, value) in bad {
            assert_eq!(
                check("sshd.service", &keys(&[(key, value)])),
                Err(HelperError::NotAllowed),
                "{key:?}={value:?}"
            );
        }
        let too_many = vec!["a.service"; MAX_ON_FAILURE_UNITS + 1].join(" ");
        assert_eq!(
            check("sshd.service", &keys(&[(DropInKey::OnFailure, &too_many)])),
            Err(HelperError::NotAllowed)
        );
        let good = keys(&[
            (DropInKey::Restart, "on-abort"),
            (DropInKey::RestartSec, "3600000000us"),
            (DropInKey::RestartSteps, "1000"),
            (DropInKey::RestartMaxDelaySec, "0us"),
            (DropInKey::StartLimitBurst, "0"),
            (DropInKey::StartLimitIntervalSec, "86400000000us"),
            (DropInKey::StartLimitAction, "poweroff-immediate"),
            (DropInKey::FailureAction, "exit-force"),
            (
                DropInKey::OnFailure,
                &vec!["failure-notify@%n.service"; MAX_ON_FAILURE_UNITS].join(" "),
            ),
        ]);
        assert_eq!(check("sshd.service", &good), Ok(()));
    }

    #[test]
    fn write_then_remove_in_temp_root() {
        let root = TestRoot::new();
        let edit = keys(&[
            (DropInKey::Restart, "on-failure"),
            (DropInKey::RestartSec, "100000us"),
        ]);
        assert_eq!(root.write("sshd.service", &edit), HelperReplyBody::Ok);
        let dir = root.path.join("sshd.service.d");
        let file = dir.join(DROPIN_FILE);
        assert_eq!(mode_of(&dir), DROPIN_DIR_MODE);
        assert_eq!(mode_of(&file), DROPIN_FILE_MODE);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read drop-in"),
            tl_ops::dropin::render(&edit)
        );
        assert_eq!(
            std::fs::read_dir(&dir).expect("read drop-in dir").count(),
            1,
            "no temp file left"
        );

        // A second write replaces the file.
        assert_eq!(root.write("sshd.service", &restart()), HelperReplyBody::Ok);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read drop-in"),
            tl_ops::dropin::render(&restart())
        );

        assert_eq!(root.write("sshd.service", &[]), HelperReplyBody::Ok);
        assert!(root.entries().is_empty());
        // Removing again is fine.
        assert_eq!(root.write("sshd.service", &[]), HelperReplyBody::Ok);
    }

    #[test]
    fn remove_keeps_other_dropins() {
        let root = TestRoot::new();
        assert_eq!(root.write("sshd.service", &restart()), HelperReplyBody::Ok);
        let other = root.path.join("sshd.service.d/10-admin.conf");
        std::fs::write(&other, "[Service]\n").expect("write other drop-in");
        assert_eq!(root.write("sshd.service", &[]), HelperReplyBody::Ok);
        assert!(other.exists());
        assert!(!root.path.join("sshd.service.d").join(DROPIN_FILE).exists());
    }

    #[test]
    fn symlinked_dropin_dir_refused() {
        let root = TestRoot::new();
        let target = root.path.join("elsewhere");
        std::fs::create_dir(&target).expect("create link target");
        std::os::unix::fs::symlink(&target, root.path.join("sshd.service.d")).expect("symlink");
        assert_eq!(root.write("sshd.service", &restart()), NOT_ALLOWED);
        assert_eq!(root.write("sshd.service", &[]), NOT_ALLOWED);
        assert_eq!(std::fs::read_dir(&target).expect("read target").count(), 0);

        // A dangling link and a plain file are refused too.
        std::fs::remove_file(root.path.join("sshd.service.d")).expect("remove link");
        std::os::unix::fs::symlink("/nonexistent-tasklens", root.path.join("sshd.service.d"))
            .expect("dangling symlink");
        assert_eq!(root.write("sshd.service", &restart()), NOT_ALLOWED);
        std::fs::write(root.path.join("cron.service.d"), "").expect("plain file");
        assert_eq!(root.write("cron.service", &restart()), NOT_ALLOWED);
        assert_eq!(root.write("cron.service", &[]), NOT_ALLOWED);
    }

    #[test]
    fn symlinked_dropin_file_is_replaced_not_followed() {
        let root = TestRoot::new();
        let dir = root.path.join("sshd.service.d");
        std::fs::create_dir(&dir).expect("create drop-in dir");
        let victim = root.path.join("victim");
        std::fs::write(&victim, "keep").expect("write victim");
        std::os::unix::fs::symlink(&victim, dir.join(DROPIN_FILE)).expect("symlink");
        assert_eq!(root.write("sshd.service", &restart()), HelperReplyBody::Ok);
        assert_eq!(
            std::fs::read_to_string(&victim).expect("read victim"),
            "keep"
        );
        let meta = std::fs::symlink_metadata(dir.join(DROPIN_FILE)).expect("stat");
        assert!(meta.file_type().is_file());
    }
}
