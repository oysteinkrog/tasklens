//! Panic hook and crash report files (ADR-015, R-049, APP-055).
//!
//! A report holds the version, thread, panic message, location, backtrace and the last log
//! lines. It never holds process metadata, settings contents, command lines or environment.

use std::any::Any;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::logging::RecentLines;
use crate::paths;

/// How many reports `prune` keeps.
pub const KEEP_REPORTS: usize = 20;

const PREFIX: &str = "crash-";
const REPORT_EXT: &str = ".txt";
const SEEN_EXT: &str = ".seen";

/// The fields of one report, already turned into text.
pub struct ReportFields<'a> {
    pub version: &'a str,
    pub git_sha: &'a str,
    pub thread: &'a str,
    pub message: &'a str,
    pub location: &'a str,
    pub backtrace: &'a str,
}

/// Installs the panic hook. It writes a report to `crash_dir`, prunes old reports, calls
/// `on_main_thread_panic` when the main thread panicked, then calls the previous hook.
/// `recent` is empty until logging starts; until then the report has no log lines.
pub fn install_hook(
    crash_dir: PathBuf,
    recent: Arc<OnceLock<RecentLines>>,
    on_main_thread_panic: fn(),
) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let lines = recent.get().map(RecentLines::snapshot).unwrap_or_default();
            let _ = write_report(&crash_dir, info, &lines, now_unix());
            let _ = prune(&crash_dir);
            if std::thread::current().name() == Some("main") {
                on_main_thread_panic();
            }
        }));
        previous(info);
    }));
}

/// Writes `<crash_dir>/crash-<now_unix>.txt` with mode 0600 and returns its path. When that
/// name is taken, `-1`, `-2` and so on go before `.txt`.
pub fn write_report(
    crash_dir: &Path,
    info: &PanicHookInfo<'_>,
    recent: &[String],
    now_unix: u64,
) -> io::Result<PathBuf> {
    let current = std::thread::current();
    let message = payload_text(info.payload());
    let location = info.location().map_or_else(
        || "unknown".to_owned(),
        |loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
    );
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let fields = ReportFields {
        version: env!("CARGO_PKG_VERSION"),
        git_sha: env!("TASKLENS_GIT_SHA"),
        thread: current.name().unwrap_or("unnamed"),
        message,
        location: &location,
        backtrace: &backtrace,
    };
    let text = format_report(&fields, recent);

    paths::ensure_dir(crash_dir)?;
    let mut suffix = 0u32;
    loop {
        let name = if suffix == 0 {
            format!("{PREFIX}{now_unix}{REPORT_EXT}")
        } else {
            format!("{PREFIX}{now_unix}-{suffix}{REPORT_EXT}")
        };
        let path = crash_dir.join(name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(paths::FILE_MODE)
            .open(&path);
        match file {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                return Ok(path);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => suffix += 1,
            Err(err) => return Err(err),
        }
    }
}

/// Returns the report text: the header fields one per line, then the backtrace and the log
/// lines (oldest first) as sections.
pub fn format_report(fields: &ReportFields<'_>, recent: &[String]) -> String {
    let mut text = format!(
        "tasklens {} ({})\nthread: {}\nmessage: {}\nlocation: {}\n\nbacktrace:\n{}\n\nlast log lines:\n",
        fields.version,
        fields.git_sha,
        fields.thread,
        fields.message,
        fields.location,
        fields.backtrace.trim_end(),
    );
    for line in recent {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Keeps the newest `KEEP_REPORTS` `crash-*.txt` files, ordered by the time in the name and
/// then by the same-second suffix. Removes the older ones and their `.seen` markers.
pub fn prune(crash_dir: &Path) -> io::Result<()> {
    let mut reports: Vec<((u64, u32), String)> = Vec::new();
    for entry in std::fs::read_dir(crash_dir)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(REPORT_EXT) else {
            continue;
        };
        if let Some(order) = report_order(stem) {
            reports.push((order, stem.to_owned()));
        }
    }
    reports.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let mut result = Ok(());
    for (_, stem) in reports.iter().skip(KEEP_REPORTS) {
        if let Err(err) = std::fs::remove_file(crash_dir.join(format!("{stem}{REPORT_EXT}"))) {
            result = Err(err);
        }
        match std::fs::remove_file(crash_dir.join(format!("{stem}{SEEN_EXT}"))) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => result = Err(err),
            _ => {}
        }
    }
    result
}

/// Parses `crash-<t>` or `crash-<t>-<n>` into `(t, n)`, with `n` 0 when there is no suffix.
fn report_order(stem: &str) -> Option<(u64, u32)> {
    let rest = stem.strip_prefix(PREFIX)?;
    let (time, suffix) = match rest.split_once('-') {
        Some((time, suffix)) => (time, digits(suffix)?.parse().ok()?),
        None => (rest, 0),
    };
    Some((digits(time)?.parse().ok()?, suffix))
}

fn digits(text: &str) -> Option<&str> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then_some(text)
}

fn payload_text(payload: &(dyn Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text
    } else {
        "Box<dyn Any>"
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
