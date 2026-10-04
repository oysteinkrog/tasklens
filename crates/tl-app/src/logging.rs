//! Logging setup (ADR-015): an `EnvFilter` from `TASKLENS_LOG`, a daily log file kept 7 days,
//! stderr when it is a terminal, and the last 200 lines in memory for the crash report.
//!
//! Targets used across tasklens: `tasklens::engine`, `tasklens::source::<name>`,
//! `tasklens::action`, `tasklens::ui`, `tasklens::frame`, `tasklens::probe`, `tasklens::perf`.
//!
//! Never log command lines, environment, working directories, open file paths, executable paths
//! of other users' processes, window titles, or user names other than the current user.

use std::collections::VecDeque;
use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

use crate::paths;

pub const DEFAULT_FILTER: &str =
    "info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,zbus=warn";
pub const RING_LINES: usize = 200;

const FILE_PREFIX: &str = "tasklens";
const FILE_SUFFIX: &str = "log";
const MAX_LOG_FILES: usize = 7;

/// Keeps the file writer thread alive. Held in `main` and dropped last, so buffered lines are
/// flushed. Empty when the log folder could not be used.
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// The most recent log lines, oldest first, at most `RING_LINES`.
#[derive(Clone, Default)]
pub struct RecentLines(Arc<Mutex<VecDeque<String>>>);

impl RecentLines {
    /// Returns a copy of the kept lines, oldest first.
    pub fn snapshot(&self) -> Vec<String> {
        let lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().cloned().collect()
    }

    fn push(&self, line: String) {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == RING_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

/// Sets the global subscriber. Fails only if a global subscriber was already set.
pub fn init(logs_dir: &Path) -> anyhow::Result<(LogGuard, RecentLines)> {
    let var = std::env::var("TASKLENS_LOG").ok();
    let (filter, invalid_var) = parse_filter(var.as_deref());
    let ring = RecentLines::default();

    let (file, guard) = match open_log_file(logs_dir) {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(writer), Some(guard))
        }
        None => (None, None),
    };
    let file_ok = file.is_some();
    let stderr = std::io::stderr().is_terminal();

    let stderr_layer = stderr.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_writer(std::io::stderr)
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer(file))
        .with(ring_layer(ring.clone()))
        .with(stderr_layer)
        .try_init()?;

    if invalid_var {
        tracing::warn!("TASKLENS_LOG is invalid; using the default filter");
    }
    if !file_ok {
        tracing::warn!("Log folder is not writable; logging to stderr only");
    }
    Ok((LogGuard { _file: guard }, ring))
}

/// Returns the filter for a `TASKLENS_LOG` value, or `DEFAULT_FILTER` when it is unset, empty
/// or invalid. `RUST_LOG` is never read.
pub fn build_filter(var: Option<&str>) -> EnvFilter {
    parse_filter(var).0
}

/// Returns the filter and whether a set value was invalid.
fn parse_filter(var: Option<&str>) -> (EnvFilter, bool) {
    let parse = |dirs: &str| EnvFilter::builder().parse(dirs);
    match var.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => match parse(value) {
            Ok(filter) => (filter, false),
            Err(_) => (default_filter(), true),
        },
        None => (default_filter(), false),
    }
}

fn default_filter() -> EnvFilter {
    EnvFilter::builder()
        .parse(DEFAULT_FILTER)
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Creates the log folder (0700) and the daily appender, then sets 0600 on the log files.
/// Returns `None` when either step fails.
fn open_log_file(logs_dir: &Path) -> Option<RollingFileAppender> {
    paths::ensure_dir(logs_dir).ok()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(logs_dir)
        .ok()?;
    restrict_log_files(logs_dir);
    Some(appender)
}

/// Sets mode 0600 on every `tasklens.*.log` file in `logs_dir`. The appender creates the
/// current file when it is built.
fn restrict_log_files(logs_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("tasklens.") && name.ends_with(".log") {
            let mode = std::fs::Permissions::from_mode(paths::FILE_MODE);
            let _ = std::fs::set_permissions(entry.path(), mode);
        }
    }
}

fn file_layer<S>(file: Option<NonBlocking>) -> Option<impl Layer<S>>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    file.map(|writer| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
    })
}

/// Formats each event like the file layer and keeps it in `ring`. The fmt layer formats into
/// a reused per-thread buffer and hands the whole line to `RingWriter` in one write.
fn ring_layer<S>(ring: RecentLines) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(ring)
}

impl<'a> MakeWriter<'a> for RecentLines {
    type Writer = RingWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RingWriter(self)
    }
}

pub struct RingWriter<'a>(&'a RecentLines);

impl std::io::Write for RingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let line = text.strip_suffix('\n').unwrap_or(&text);
        self.0.push(line.to_owned());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directives(filter: &EnvFilter) -> Vec<String> {
        let mut parts: Vec<String> = filter.to_string().split(',').map(String::from).collect();
        parts.sort();
        parts
    }

    fn ring_subscriber(filter: EnvFilter, ring: &RecentLines) -> impl tracing::Subscriber {
        tracing_subscriber::registry()
            .with(filter)
            .with(ring_layer(ring.clone()))
    }

    #[test]
    fn default_filter_string() {
        let mut expected: Vec<String> = DEFAULT_FILTER.split(',').map(String::from).collect();
        expected.sort();
        assert_eq!(directives(&build_filter(None)), expected);
    }

    #[test]
    fn tasklens_log_used() {
        let ring = RecentLines::default();
        tracing::subscriber::with_default(
            ring_subscriber(build_filter(Some("debug")), &ring),
            || tracing::debug!("debug event"),
        );
        let lines = ring.snapshot();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("debug event"));
    }

    #[test]
    fn invalid_tasklens_log_falls_back() {
        let (filter, invalid) = parse_filter(Some("tasklens=notalevel"));
        assert!(invalid);
        assert_eq!(directives(&filter), directives(&build_filter(None)));
        assert!(!parse_filter(Some("debug")).1);
        assert!(!parse_filter(None).1);
    }

    // `build_filter` takes the value as an argument and never reads the environment, so
    // `RUST_LOG=trace` cannot widen it. The default filter drops trace and debug events.
    #[test]
    fn rust_log_ignored() {
        let ring = RecentLines::default();
        tracing::subscriber::with_default(ring_subscriber(build_filter(None), &ring), || {
            tracing::trace!("trace event");
            tracing::debug!("debug event");
            tracing::info!("info event");
        });
        let lines = ring.snapshot();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("info event"));
    }

    #[test]
    fn ring_keeps_last_200() {
        let ring = RecentLines::default();
        tracing::subscriber::with_default(ring_subscriber(build_filter(None), &ring), || {
            for i in 1..=250 {
                tracing::info!("event {i}");
            }
        });
        let lines = ring.snapshot();
        assert_eq!(lines.len(), RING_LINES);
        assert!(lines[0].ends_with("event 51"), "{}", lines[0]);
        assert!(lines[199].ends_with("event 250"), "{}", lines[199]);
        assert!(lines.iter().all(|line| !line.contains('\u{1b}')));
    }

    #[test]
    fn ring_skips_filtered_events() {
        let ring = RecentLines::default();
        let filter = build_filter(Some("warn"));
        tracing::subscriber::with_default(ring_subscriber(filter, &ring), || {
            tracing::info!("dropped");
            tracing::warn!("kept");
        });
        let lines = ring.snapshot();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("kept"));
    }

    #[test]
    fn file_written_with_prefix_and_suffix() {
        let dir = tempfile::tempdir().expect("temp dir");
        let logs = dir.path().join("logs");
        let appender = open_log_file(&logs).expect("appender");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let subscriber = tracing_subscriber::registry()
            .with(build_filter(None))
            .with(file_layer(Some(writer)));
        tracing::subscriber::with_default(subscriber, || tracing::info!("file event"));
        drop(guard);

        let names: Vec<String> = std::fs::read_dir(&logs)
            .expect("read logs")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let name = &names[0];
        assert!(name.starts_with("tasklens.") && name.ends_with(".log"), "{name}");
        // tasklens.yyyy-mm-dd.log
        assert_eq!(name.len(), "tasklens.".len() + 10 + ".log".len(), "{name}");

        let path = logs.join(name);
        let text = std::fs::read_to_string(&path).expect("read log");
        assert!(text.contains("file event"));
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(&logs).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn unusable_logs_dir_gives_no_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"").expect("write");
        // A path below a regular file cannot be created.
        assert!(open_log_file(&blocker.join("logs")).is_none());
    }
}
