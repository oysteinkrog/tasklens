//! Process display names and search text (PROC-007, PROC-027, R-043).
//!
//! Inputs are already sanitized (E05-002). Every function writes into a caller buffer or returns a
//! slice of its input, so nothing allocates once the buffers have capacity.

use std::sync::Arc;

/// Executable basenames of the Wine loader. A process with one of these as its exe runs a
/// Windows program.
pub const WINE_LOADERS: &[&str] = &["wine64-preloader", "wine-preloader", "wine64", "wine"];

/// Suffix the kernel adds to `/proc/<pid>/exe` when the file was removed.
const DELETED_SUFFIX: &str = " (deleted)";

/// Returns the part of `exe` after the last `/`, with a trailing ` (deleted)` removed.
pub fn exe_basename(exe: &str) -> &str {
    let exe = exe.strip_suffix(DELETED_SUFFIX).unwrap_or(exe);
    exe.rsplit('/').next().unwrap_or(exe)
}

/// True when the exe basename is a Wine loader, or `comm` ends with `.exe` (any case).
pub fn is_wine(exe_base: Option<&str>, comm: &str) -> bool {
    exe_base.is_some_and(|base| WINE_LOADERS.contains(&base)) || ends_with_exe(comm)
}

/// Returns the first argv token that ends with `.exe` (any case), cut to its last segment.
/// Both `/` and `\` split segments, so `C:\Games\game.exe` gives `game.exe`.
pub fn wine_exe_name(argv: &[Arc<str>]) -> Option<&str> {
    wine_exe_path(argv).map(windows_file_name)
}

/// Returns the first argv token that ends with `.exe` (any case), whole:
/// `C:\Program Files\Backblaze\bztransmit.exe`.
pub fn wine_exe_path(argv: &[Arc<str>]) -> Option<&str> {
    argv.iter().find(|t| ends_with_exe(t)).map(|t| &**t)
}

/// The last segment of a Windows or Unix path. Both `/` and `\` split segments.
pub fn windows_file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Returns the image name: the Windows `.exe` name for Wine, else the exe basename.
///
/// `None` means the exe link was unreadable, and the column shows Restricted. For Wine with no
/// `.exe` token in argv, the exe basename is used, so `None` keeps that single meaning.
pub fn image_name<'a>(
    exe: Option<&'a str>,
    argv: &'a [Arc<str>],
    _comm: &'a str,
    wine: bool,
) -> Option<&'a str> {
    if wine && let Some(name) = wine_exe_name(argv) {
        return Some(name);
    }
    exe.map(exe_basename)
}

/// Inputs for [`display_name`].
pub struct NameInput<'a> {
    /// Name of the app this process belongs to, if any.
    pub app_name: Option<&'a str>,
    /// True when this process is the app's primary process.
    pub is_app_primary: bool,
    /// The `/proc/<pid>/exe` link target, `None` when unreadable.
    pub exe: Option<&'a str>,
    /// Command line tokens.
    pub argv: &'a [Arc<str>],
    /// `/proc/<pid>/comm`.
    pub comm: &'a str,
    /// True for a kernel thread.
    pub kernel_thread: bool,
}

/// Writes the display name for `inp` into `out`, which is cleared first.
///
/// In order: a kernel thread gives `[comm]`; an app primary gives the app name; Wine gives the
/// `.exe` name. Otherwise the base is the exe basename, or with no exe the first argv token with
/// one trailing `:` removed, or when that is empty `comm`. When `comm` differs from the base and
/// the base does not start with `comm`, the result is `<base> (<comm>)`. The prefix test keeps the
/// kernel's 15-byte `comm` cut from adding a qualifier.
pub fn display_name(inp: &NameInput<'_>, out: &mut String) {
    out.clear();
    if inp.kernel_thread {
        out.push('[');
        out.push_str(inp.comm);
        out.push(']');
        return;
    }
    if inp.is_app_primary
        && let Some(app) = inp.app_name
    {
        out.push_str(app);
        return;
    }
    let exe_base = inp.exe.map(exe_basename);
    if is_wine(exe_base, inp.comm)
        && let Some(name) = wine_exe_name(inp.argv)
    {
        out.push_str(name);
        return;
    }
    let base = match exe_base {
        Some(base) => base,
        None => inp
            .argv
            .first()
            .map(|t| t.strip_suffix(':').unwrap_or(t))
            .unwrap_or(""),
    };
    let base = if base.is_empty() { inp.comm } else { base };
    out.push_str(base);
    if inp.comm != base && !base.starts_with(inp.comm) {
        out.push_str(" (");
        out.push_str(inp.comm);
        out.push(')');
    }
}

/// Writes the lowercase search text into `out`, which is cleared first.
///
/// Joins the non-empty `parts` with one space. Callers pass them in R-043 order: display name,
/// image name, command line, user, description, app name, unit name.
pub fn search_lower(parts: &[&str], out: &mut String) {
    out.clear();
    for part in parts.iter().filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        for c in part.chars() {
            out.extend(c.to_lowercase());
        }
    }
}

fn ends_with_exe(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".exe")
}
