//! Program launch and Desktop Entry Exec parsing.
//!
//! `parse_exec` turns a desktop entry's `Exec` value into an argument vector as the Desktop Entry
//! Specification 1.5 says ("Possible value types" and "The Exec key"). No shell is involved:
//! `$`, `;`, `|`, `&` and `` ` `` reach the program as literal characters.
//!
//! The Run dialog and Startup "Run now" go through `shell_argv`, `script_argv` or `admin_argv`,
//! then `spawn_detached` and `wait_exit` (SYS-023, PROC-095, ADR-006). Never log a command text,
//! argument, environment value or working directory (ADR-015).

use std::ffi::CString;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rustix::io::Errno;
use rustix::process::{Pid, PidfdFlags, WaitId, WaitIdOptions};

use crate::error::{OpsError, OpsResult};
use crate::sys;

/// The shell that runs typed commands and startup scripts.
pub const SH_PATH: &str = "/bin/sh";
/// The polkit helper that runs a command as an administrator.
pub const PKEXEC_PATH: &str = "/usr/bin/pkexec";
/// pkexec(1): not authorized, or authentication failed.
pub const PKEXEC_NOT_AUTHORIZED: i32 = 127;
/// pkexec(1): the user dismissed the authentication dialog.
pub const PKEXEC_DISMISSED: i32 = 126;
/// The variables passed through `pkexec env`, in this order.
pub const ADMIN_ENV_KEYS: [&str; 3] = ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"];

/// A child started by `spawn_detached`. Pass it to `wait_exit` so it is reaped.
#[derive(Debug)]
pub struct Spawned {
    /// The child's process id.
    pub pid: u32,
    /// A pidfd for the child, opened before it could be reaped.
    pub pidfd: OwnedFd,
}

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// The exit code.
    Exited(i32),
    /// The number of the signal that ended it.
    Signaled(i32),
}

/// Checks a command typed by the user: not blank, and no C0 or C1 control character or DEL.
pub fn validate_command_text(text: &str) -> OpsResult {
    if text.trim_ascii().is_empty() {
        return Err(OpsError::InvalidInput("command is empty"));
    }
    let invalid = |c: char| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}');
    if text.chars().any(invalid) {
        return Err(OpsError::InvalidInput("command contains invalid text"));
    }
    Ok(())
}

/// `["/bin/sh", "-c", text]`, with `text` unchanged.
pub fn shell_argv(text: &str) -> OpsResult<Vec<String>> {
    validate_command_text(text)?;
    Ok(vec![SH_PATH.to_owned(), "-c".to_owned(), text.to_owned()])
}

/// `["/bin/sh", path]` for an absolute UTF-8 script path.
pub fn script_argv(script: &Path) -> OpsResult<Vec<String>> {
    match script.to_str() {
        Some(path) if script.is_absolute() => Ok(vec![SH_PATH.to_owned(), path.to_owned()]),
        _ => Err(OpsError::InvalidInput("script path is not absolute UTF-8")),
    }
}

/// `pkexec env KEY=value... /bin/sh -c text`. `env` holds the caller's value of each key in
/// `ADMIN_ENV_KEYS`; an unset or empty value is left out. `text` is only ever the single argument
/// after `-c`.
pub fn admin_argv(text: &str, env: &[(&str, Option<&str>)]) -> OpsResult<Vec<String>> {
    validate_command_text(text)?;
    let mut argv = vec![PKEXEC_PATH.to_owned(), "env".to_owned()];
    for key in ADMIN_ENV_KEYS {
        let value = env
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| *v)
            .filter(|v| !v.is_empty());
        if let Some(value) = value {
            if value.contains(['\0', '\n']) {
                return Err(OpsError::InvalidInput(
                    "environment value contains invalid text",
                ));
            }
            argv.push(format!("{key}={value}"));
        }
    }
    argv.extend([SH_PATH.to_owned(), "-c".to_owned(), text.to_owned()]);
    Ok(argv)
}

/// A change to a child's environment, as `Command::env` and `Command::env_remove` make it:
/// `(key, Some(value))` sets the variable, `(key, None)` removes it.
pub type EnvChange = (String, Option<String>);

/// Starts `argv` in a new session with stdio on `/dev/null`, default signal handling and `cwd`
/// as its working directory. The caller must pass the result to `wait_exit`.
pub fn spawn_detached(argv: &[String], cwd: Option<&Path>) -> OpsResult<Spawned> {
    spawn_detached_env(argv, cwd, &[])
}

/// `spawn_detached` with `env` applied to this process's environment for the child. The program
/// itself is spawned, never a wrapper, so a missing or non-executable program fails here.
pub fn spawn_detached_env(
    argv: &[String],
    cwd: Option<&Path>,
    env: &[EnvChange],
) -> OpsResult<Spawned> {
    if argv.is_empty() {
        return Err(OpsError::InvalidInput("empty argument list"));
    }
    let args = argv
        .iter()
        .map(|arg| CString::new(arg.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| OpsError::InvalidInput("argument contains a NUL byte"))?;
    let cwd = match cwd {
        Some(dir) if dir.is_dir() => Some(
            CString::new(dir.as_os_str().as_bytes())
                .map_err(|_| OpsError::InvalidInput("working directory does not exist"))?,
        ),
        Some(_) => return Err(OpsError::InvalidInput("working directory does not exist")),
        None => None,
    };

    let envp = if env.is_empty() {
        None
    } else {
        Some(child_envp(std::env::vars_os(), env)?)
    };

    let result = sys::spawn_setsid(&args, cwd.as_deref(), envp.as_deref())
        .map_err(OpsError::from)
        .and_then(open_child_pidfd);
    tracing::debug!(pid = result.as_ref().ok().map(|s| s.pid), ok = result.is_ok(), error = ?result.as_ref().err(), "spawn_detached");
    result
}

/// `KEY=value` entries: `base` without the keys `env` names, then each variable `env` sets.
fn child_envp(
    base: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    env: &[EnvChange],
) -> OpsResult<Vec<CString>> {
    for (key, value) in env {
        if key.is_empty() || key.contains(['=', '\0']) {
            return Err(OpsError::InvalidInput("environment key is invalid"));
        }
        if value.as_deref().is_some_and(|v| v.contains('\0')) {
            return Err(OpsError::InvalidInput(
                "environment value contains invalid text",
            ));
        }
    }
    let entry = |key: &[u8], value: &[u8]| {
        let mut bytes = Vec::with_capacity(key.len() + 1 + value.len());
        bytes.extend_from_slice(key);
        bytes.push(b'=');
        bytes.extend_from_slice(value);
        CString::new(bytes)
    };
    let mut out = Vec::new();
    for (key, value) in base {
        if env.iter().any(|(k, _)| k.as_bytes() == key.as_bytes()) {
            continue;
        }
        // The process environment cannot hold a NUL, so this never skips an entry.
        if let Ok(e) = entry(key.as_bytes(), value.as_bytes()) {
            out.push(e);
        }
    }
    for (key, value) in env {
        if let Some(value) = value
            && let Ok(e) = entry(key.as_bytes(), value.as_bytes())
        {
            out.push(e);
        }
    }
    Ok(out)
}

/// Opens a pidfd for a child we have not reaped yet, so its pid cannot have been reused. If that
/// fails, the child is killed and reaped so it is not left behind.
fn open_child_pidfd(pid: u32) -> OpsResult<Spawned> {
    let raw = Pid::from_raw(pid as i32).ok_or(OpsError::Os(libc::EINVAL))?;
    match rustix::process::pidfd_open(raw, PidfdFlags::empty()) {
        Ok(pidfd) => Ok(Spawned { pid, pidfd }),
        Err(errno) => {
            let _ = rustix::process::kill_process(raw, rustix::process::Signal::KILL);
            while matches!(
                rustix::process::waitpid(Some(raw), rustix::process::WaitOptions::empty()),
                Err(Errno::INTR)
            ) {}
            Err(errno.into())
        }
    }
}

/// Blocks until the child ends and reaps it.
pub fn wait_exit(child: &Spawned) -> OpsResult<ExitKind> {
    let status = loop {
        match rustix::process::waitid(WaitId::PidFd(child.pidfd.as_fd()), WaitIdOptions::EXITED) {
            Err(Errno::INTR) => continue,
            other => break other,
        }
    };
    let result = match status {
        Ok(Some(status)) => match (status.exit_status(), status.terminating_signal()) {
            (Some(code), _) => Ok(ExitKind::Exited(code)),
            (None, Some(signo)) => Ok(ExitKind::Signaled(signo)),
            (None, None) => Err(OpsError::OutcomeUnknown("child state not reported")),
        },
        Ok(None) => Err(OpsError::OutcomeUnknown("child state not reported")),
        Err(errno) => Err(errno.into()),
    };
    tracing::debug!(pid = child.pid, result = ?result, "wait_exit");
    result
}

/// The entry values that the `%i`, `%c` and `%k` field codes expand to.
pub struct ExecContext<'a> {
    /// The entry's Name, already chosen by the caller.
    pub name: &'a str,
    /// The entry's Icon value, raw.
    pub icon: Option<&'a str>,
    /// Absolute path of the .desktop file.
    pub desktop_file: Option<&'a str>,
}

/// One argument after splitting, before field codes are applied.
struct Word {
    text: String,
    quoted: bool,
}

/// Parses the value that follows `Exec=` into an argument vector.
///
/// Never log `raw` or the result: they are command lines (ADR-015).
pub fn parse_exec(raw: &str, ctx: &ExecContext<'_>) -> OpsResult<Vec<String>> {
    if raw.contains('\0') {
        return Err(OpsError::InvalidInput("Exec contains a NUL byte"));
    }
    let words = split_words(&unescape_marked(raw))?;

    let mut args = Vec::with_capacity(words.len());
    for word in words {
        if word.quoted {
            args.push(word.text);
            continue;
        }
        match word.text.as_str() {
            "%f" | "%F" | "%u" | "%U" => {}
            "%i" => {
                if let Some(icon) = ctx.icon.filter(|i| !i.is_empty()) {
                    args.push("--icon".to_owned());
                    args.push(icon.to_owned());
                }
            }
            text => {
                let expanded = expand_codes(text, ctx)?;
                if !expanded.is_empty() {
                    args.push(expanded);
                }
            }
        }
    }

    if args.is_empty() {
        return Err(OpsError::InvalidInput("Exec is empty"));
    }
    Ok(args)
}

/// Applies the spec "string" escapes: `\s`, `\n`, `\t`, `\r` and `\\`. Any other `\x` pair is
/// kept as both characters.
#[allow(dead_code, reason = "no caller yet; parse_exec uses unescape_marked")]
pub(crate) fn unescape_string_value(raw: &str) -> String {
    unescape_marked(raw).into_iter().map(|(c, _)| c).collect()
}

/// `unescape_string_value`, with each character marked `true` when it came from `\s`. That space
/// belongs to its argument and does not separate arguments.
fn unescape_marked(raw: &str) -> Vec<(char, bool)> {
    let mut out = Vec::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push((c, false));
            continue;
        }
        match chars.next() {
            Some('s') => out.push((' ', true)),
            Some('n') => out.push(('\n', false)),
            Some('t') => out.push(('\t', false)),
            Some('r') => out.push(('\r', false)),
            Some('\\') => out.push(('\\', false)),
            Some(other) => {
                out.push(('\\', false));
                out.push((other, false));
            }
            None => out.push(('\\', false)),
        }
    }
    out
}

/// Splits on runs of spaces outside quotes. A space that came from `\s` does not split. Inside a
/// quoted argument `\"`, `` \` ``, `\$` and `\\` give the second character; any other backslash
/// is literal.
fn split_words(value: &[(char, bool)]) -> OpsResult<Vec<Word>> {
    let mut words = Vec::new();
    let mut chars = value.iter().copied().peekable();
    loop {
        while chars.next_if_eq(&(' ', false)).is_some() {}
        let Some(&(first, _)) = chars.peek() else {
            break;
        };

        if first == '"' {
            chars.next();
            let mut text = String::new();
            loop {
                match chars.next().map(|(c, _)| c) {
                    None => return Err(OpsError::InvalidInput("unterminated quote in Exec")),
                    Some('"') => break,
                    Some('\\') => match chars.peek().map(|&(c, _)| c) {
                        Some(e @ ('"' | '`' | '$' | '\\')) => {
                            chars.next();
                            text.push(e);
                        }
                        _ => text.push('\\'),
                    },
                    Some(c) => text.push(c),
                }
            }
            // The spec requires a quoted argument to be quoted as a whole.
            if chars.peek().is_some_and(|&p| p != (' ', false)) {
                return Err(OpsError::InvalidInput(
                    "quote inside an unquoted Exec argument",
                ));
            }
            words.push(Word { text, quoted: true });
        } else {
            let mut text = String::new();
            while let Some((c, _)) = chars.next_if(|&p| p != (' ', false)) {
                if c == '"' {
                    return Err(OpsError::InvalidInput(
                        "quote inside an unquoted Exec argument",
                    ));
                }
                text.push(c);
            }
            words.push(Word {
                text,
                quoted: false,
            });
        }
    }
    Ok(words)
}

/// Expands the field codes that may appear inside an unquoted argument: `%c`, `%k`, `%%` and the
/// deprecated codes, which are deleted. The standalone codes (`%f %F %u %U %i`) are handled by
/// the caller, so inside a longer argument they are invalid.
fn expand_codes(text: &str, ctx: &ExecContext<'_>) -> OpsResult<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('c') => out.push_str(ctx.name),
            Some('k') => out.push_str(ctx.desktop_file.unwrap_or("")),
            Some('%') => out.push('%'),
            Some('d' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
            _ => return Err(OpsError::InvalidInput("invalid field code in Exec")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use rustix::process::{Signal, WaitOptions};

    use super::*;
    use crate::sys::tests::lock_children;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_owned()).collect()
    }

    fn assert_no_child_left() {
        assert_eq!(
            rustix::process::wait(WaitOptions::NOHANG).map(|r| r.is_some()),
            Err(Errno::CHILD),
            "a child was left behind"
        );
    }

    #[test]
    fn spawn_detached_new_session_devnull_cwd_default_signals() {
        let _children = lock_children();
        let child = spawn_detached(&args(&["sleep", "30"]), Some(Path::new("/tmp"))).unwrap();

        let proc_dir = Path::new("/proc").join(child.pid.to_string());
        let stat = std::fs::read_to_string(proc_dir.join("stat"));
        let status = std::fs::read_to_string(proc_dir.join("status"));
        let cwd = std::fs::read_link(proc_dir.join("cwd"));
        let fds: Vec<_> = (0..3)
            .map(|fd| std::fs::read_link(proc_dir.join("fd").join(fd.to_string())))
            .collect();

        rustix::process::pidfd_send_signal(&child.pidfd, Signal::KILL).unwrap();
        assert_eq!(wait_exit(&child), Ok(ExitKind::Signaled(libc::SIGKILL)));

        // Fields after the `)` that ends comm: state, ppid, pgrp, session (field 6).
        let stat = stat.unwrap();
        let after_comm = &stat[stat.rfind(')').unwrap() + 1..];
        let session: u32 = after_comm
            .split_whitespace()
            .nth(3)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(session, child.pid);
        assert_eq!(
            cwd.unwrap(),
            std::fs::canonicalize("/tmp").expect("canonicalize /tmp")
        );
        for fd in fds {
            assert_eq!(fd.unwrap(), Path::new("/dev/null"));
        }
        let sig_ign = status
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("SigIgn:").map(|v| v.trim().to_owned()));
        assert_eq!(sig_ign.as_deref(), Some("0000000000000000"));
    }

    #[test]
    fn wait_exit_code_and_signal() {
        let _children = lock_children();
        let child = spawn_detached(&args(&["sh", "-c", "exit 3"]), None).unwrap();
        assert_eq!(wait_exit(&child), Ok(ExitKind::Exited(3)));

        let child = spawn_detached(&args(&["sleep", "30"]), None).unwrap();
        rustix::process::pidfd_send_signal(&child.pidfd, Signal::KILL).unwrap();
        assert_eq!(wait_exit(&child), Ok(ExitKind::Signaled(9)));
        assert_no_child_left();
    }

    #[test]
    fn spawn_missing_binary_is_enoent() {
        let _children = lock_children();
        let result = spawn_detached(&args(&["tasklens-no-such-binary-xyz"]), None);
        assert_eq!(result.err(), Some(OpsError::Os(libc::ENOENT)));
        assert_no_child_left();
    }

    #[test]
    fn spawn_missing_cwd_is_invalid() {
        let _children = lock_children();
        let result = spawn_detached(&args(&["true"]), Some(Path::new("/nonexistent")));
        assert_eq!(
            result.err(),
            Some(OpsError::InvalidInput("working directory does not exist"))
        );
        assert_no_child_left();
    }

    #[test]
    fn child_envp_sets_replaces_and_removes() {
        let base = [
            ("HOME", "/h"),
            ("DISPLAY", ":9"),
            ("XDG_ACTIVATION_TOKEN", "t"),
        ]
        .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v)));
        let env = [
            ("DISPLAY".to_owned(), Some(":0".to_owned())),
            ("WAYLAND_DISPLAY".to_owned(), Some("wayland-1".to_owned())),
            ("XDG_ACTIVATION_TOKEN".to_owned(), None),
            ("XAUTHORITY".to_owned(), None),
        ];
        let envp = child_envp(base.into_iter(), &env).expect("valid");
        let envp: Vec<&str> = envp.iter().map(|e| e.to_str().expect("utf-8")).collect();
        assert_eq!(envp, ["HOME=/h", "DISPLAY=:0", "WAYLAND_DISPLAY=wayland-1"]);

        for bad in [
            ("".to_owned(), None),
            ("A=B".to_owned(), None),
            ("A".to_owned(), Some("x\0y".to_owned())),
        ] {
            assert!(child_envp(std::iter::empty(), &[bad]).is_err());
        }
    }

    /// The child gets the set variables, loses the removed ones and keeps the rest.
    #[test]
    fn spawn_detached_env_gives_the_child_its_environment() {
        let _children = lock_children();
        let dir = std::env::temp_dir().join(format!("tl-ops-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let out = dir.join("env");
        let script = format!(
            "printf '%s|%s|%s' \"$WAYLAND_DISPLAY\" \"${{XDG_ACTIVATION_TOKEN-unset}}\" \"$HOME\" > '{}'",
            out.display()
        );
        let env = [
            ("WAYLAND_DISPLAY".to_owned(), Some("wayland-7".to_owned())),
            ("XDG_ACTIVATION_TOKEN".to_owned(), None),
        ];
        let child = spawn_detached_env(&args(&["/bin/sh", "-c", &script]), None, &env).unwrap();
        assert_eq!(wait_exit(&child), Ok(ExitKind::Exited(0)));
        let got = std::fs::read_to_string(&out).expect("child wrote its env");
        let _ = std::fs::remove_dir_all(&dir);
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(got, format!("wayland-7|unset|{home}"));
    }

    #[test]
    fn spawn_rejects_empty_argv_and_nul() {
        assert_eq!(
            spawn_detached(&[], None).err(),
            Some(OpsError::InvalidInput("empty argument list"))
        );
        assert_eq!(
            spawn_detached(&args(&["a\0b"]), None).err(),
            Some(OpsError::InvalidInput("argument contains a NUL byte"))
        );
    }
}
