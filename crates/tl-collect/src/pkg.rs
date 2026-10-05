//! Package owner index (R-163 item 1): which package owns an executable, for the PROC-027
//! package columns, the PROC-078 integrity check and SYS-020's Publisher.
//!
//! The sweep only calls [`PackageIndex::get`] and [`PackageIndex::request`], which never block.
//! The engine runs [`PackageIndex::run_batch`] in its own blocking job when
//! [`PackageIndex::due`] is true. pacman output and `desc` files are untrusted text: reads are
//! bounded, values are sanitized and no shell is used.

use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use tl_model::process::PackageInfo;

use crate::CollectError;
use crate::sanitize::sanitize_text;

/// Wait between the last newly requested path and the next batch.
pub const PKG_DEBOUNCE: Duration = Duration::from_secs(2);
/// Most paths in one `pacman -Qo` call.
pub const PKG_BATCH_MAX: usize = 256;

/// Fixed path of the pacman binary. No shell and no PATH lookup.
const PACMAN: &str = "/usr/bin/pacman";
/// Default pacman local database directory.
const PACMAN_DB: &str = "/var/lib/pacman/local";
/// Read limit for pacman stdout.
const STDOUT_MAX: usize = 1024 * 1024;
/// Read limit for pacman stderr.
const STDERR_MAX: usize = 64 * 1024;
/// Read limit for a `desc` file.
const DESC_MAX: usize = 64 * 1024;
/// Length limit for one sanitized `desc` value, in bytes.
const VALUE_MAX: usize = 4096;
/// Length limit for a package name from pacman output, in bytes.
const NAME_MAX: usize = 256;
/// Length limit for the `Failed` text of a verify, in characters.
const FAILED_MAX_CHARS: usize = 200;
/// Wall limit for one pacman run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval while waiting for pacman to exit.
const POLL: Duration = Duration::from_millis(5);

/// Set once the missing pacman binary was logged, so the log line appears only once.
static PACMAN_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

/// Backend that maps paths to owning packages. Arch is the only v1 backend; `dpkg -S` and
/// `rpm -qf` come in P4 behind the same trait.
pub trait PkgBackend: Send {
    /// One batched owner query. Returns (path, Some(name)) for owned paths, (path, None) for
    /// unowned ones.
    fn owners(&mut self, paths: &[&Path])
    -> Result<Vec<(PathBuf, Option<Arc<str>>)>, CollectError>;
    /// Reads the package's metadata.
    fn info(&mut self, name: &str) -> Result<Option<PackageInfo>, CollectError>;
    /// PROC-078 integrity check.
    fn verify(&mut self, name: &str) -> Result<VerifyOutcome, CollectError>;
    /// Change marker of the package database (mtime of /var/lib/pacman/local).
    fn db_stamp(&mut self) -> Option<i64>;
}

/// Result of the PROC-078 integrity check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyOutcome {
    Unmodified,
    Modified { altered_files: u32 },
    Failed(Arc<str>),
}

/// The Arch backend: `pacman -Qo`, `pacman -Qkk` and the local database `desc` files.
pub struct Pacman {
    /// Default `/var/lib/pacman/local`.
    pub db_dir: PathBuf,
}

impl Default for Pacman {
    fn default() -> Self {
        Self {
            db_dir: PathBuf::from(PACMAN_DB),
        }
    }
}

impl PkgBackend for Pacman {
    fn owners(
        &mut self,
        paths: &[&Path],
    ) -> Result<Vec<(PathBuf, Option<Arc<str>>)>, CollectError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut cmd = Command::new(PACMAN);
        cmd.arg("-Qo").arg("--").args(paths);
        match run(cmd, false) {
            // Exit status 1 is normal when some path is not owned, so the status is not checked.
            Ok(out) => Ok(parse_qo(&out.stdout, paths)),
            Err(RunError::Missing) => {
                if !PACMAN_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
                    tracing::debug!("pkg: pacman not found, every path counts as not packaged");
                }
                Ok(paths.iter().map(|p| (p.to_path_buf(), None)).collect())
            }
            Err(RunError::TimedOut) => Err(CollectError::Command("pacman -Qo timed out")),
            Err(RunError::Io(e)) => Err(e.into()),
        }
    }

    fn info(&mut self, name: &str) -> Result<Option<PackageInfo>, CollectError> {
        let entries = match std::fs::read_dir(&self.db_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut prefix = Vec::with_capacity(name.len() + 1);
        prefix.extend_from_slice(name.as_bytes());
        prefix.push(b'-');
        let mut buf = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_name().as_bytes().starts_with(&prefix) {
                continue;
            }
            buf.clear();
            match std::fs::File::open(entry.path().join("desc")) {
                Ok(file) => {
                    file.take(DESC_MAX as u64).read_to_end(&mut buf)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
            if let Some(info) = parse_desc(&buf)
                && &*info.name == name
            {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    fn verify(&mut self, name: &str) -> Result<VerifyOutcome, CollectError> {
        let mut cmd = Command::new(PACMAN);
        cmd.arg("-Qkk").arg("--").arg(name);
        match run(cmd, true) {
            Ok(out) => Ok(parse_qkk(name, &out.stdout, &out.stderr)),
            Err(RunError::Missing) => Ok(VerifyOutcome::Failed(Arc::from("pacman not found"))),
            Err(RunError::TimedOut) => Ok(VerifyOutcome::Failed(Arc::from("timed out"))),
            Err(RunError::Io(e)) => Err(e.into()),
        }
    }

    fn db_stamp(&mut self) -> Option<i64> {
        rustix::fs::statx(
            rustix::fs::CWD,
            &self.db_dir,
            rustix::fs::AtFlags::empty(),
            rustix::fs::StatxFlags::MTIME,
        )
        .ok()
        .map(|st| st.stx_mtime.tv_sec)
    }
}

/// Parses `pacman -Qo` stdout. Each line is `<path> is owned by <name> <version>`. A requested
/// path that no line names is `(path, None)`. The result keeps the order of `paths`.
pub fn parse_qo(stdout: &[u8], paths: &[&Path]) -> Vec<(PathBuf, Option<Arc<str>>)> {
    const OWNED_BY: &[u8] = b" is owned by ";
    let mut owned: FxHashMap<&[u8], Arc<str>> = FxHashMap::default();
    let mut name = String::new();
    for line in stdout.split(|&b| b == b'\n') {
        let Some(at) = memchr::memmem::rfind(line, OWNED_BY) else {
            continue;
        };
        let path = &line[..at];
        let rest = &line[at + OWNED_BY.len()..];
        let Some(raw_name) = rest.split(|&b| b == b' ').next() else {
            continue;
        };
        if raw_name.is_empty() {
            continue;
        }
        sanitize_text(raw_name, NAME_MAX, &mut name);
        owned.insert(path, Arc::from(name.as_str()));
    }
    paths
        .iter()
        .map(|p| {
            let owner = owned.get(p.as_os_str().as_bytes()).cloned();
            (p.to_path_buf(), owner)
        })
        .collect()
}

/// Parses a pacman `desc` file. A section is a `%KEY%` line followed by value lines up to a
/// blank line. Returns `None` when `%NAME%` or `%VERSION%` is missing or empty.
pub fn parse_desc(bytes: &[u8]) -> Option<PackageInfo> {
    let mut name = None;
    let mut version = None;
    let mut description = None;
    let mut packager = None;
    let mut build_date_s = None;
    let mut url = None;

    let mut lines = bytes.split(|&b| b == b'\n').peekable();
    while let Some(line) = lines.next() {
        let key = line.strip_suffix(b"\r").unwrap_or(line);
        if key.len() < 3 || key[0] != b'%' || key[key.len() - 1] != b'%' {
            continue;
        }
        // The value is the raw bytes from the first value line up to the blank line.
        let start = offset_in(bytes, lines.peek().copied());
        let mut end = start;
        while let Some(&value_line) = lines.peek() {
            if value_line.is_empty() || value_line == b"\r" {
                break;
            }
            end = offset_in(bytes, Some(value_line)) + value_line.len();
            lines.next();
        }
        let raw = &bytes[start.min(end)..end];
        let slot = match &key[1..key.len() - 1] {
            b"NAME" => &mut name,
            b"VERSION" => &mut version,
            b"DESC" => &mut description,
            b"PACKAGER" => &mut packager,
            b"URL" => &mut url,
            b"BUILDDATE" => {
                build_date_s = std::str::from_utf8(raw)
                    .ok()
                    .and_then(|s| s.trim().parse::<i64>().ok());
                continue;
            }
            _ => continue,
        };
        *slot = clean_value(raw);
    }

    let packager = packager.map(|p| match p.rfind(" <") {
        Some(at) if p.ends_with('>') => Arc::from(p[..at].trim_end()),
        _ => p,
    });
    Some(PackageInfo {
        name: name?,
        version: version?,
        description,
        packager: packager.filter(|p| !p.is_empty()),
        build_date_s,
        url,
    })
}

/// Parses `pacman -Qkk <name>` output. The summary line is
/// `<name>: <n> total files, <m> altered files`. Without one the outcome is `Failed` with the
/// first stderr line.
pub fn parse_qkk(name: &str, stdout: &[u8], stderr: &[u8]) -> VerifyOutcome {
    let mut prefix = Vec::with_capacity(name.len() + 2);
    prefix.extend_from_slice(name.as_bytes());
    prefix.extend_from_slice(b": ");
    for line in stdout.split(|&b| b == b'\n') {
        let Some(rest) = line.strip_prefix(prefix.as_slice()) else {
            continue;
        };
        let Ok(rest) = std::str::from_utf8(rest) else {
            continue;
        };
        let Some((total, altered)) = rest.split_once(", ") else {
            continue;
        };
        if !total.ends_with(" total files") {
            continue;
        }
        let Some(count) = altered.trim_end().strip_suffix(" altered files") else {
            continue;
        };
        let Ok(altered_files) = count.parse::<u32>() else {
            continue;
        };
        return if altered_files == 0 {
            VerifyOutcome::Unmodified
        } else {
            VerifyOutcome::Modified { altered_files }
        };
    }
    let first = stderr
        .split(|&b| b == b'\n')
        .find(|l| !l.is_empty())
        .unwrap_or(b"no summary line");
    let mut text = String::new();
    sanitize_text(first, STDERR_MAX, &mut text);
    if let Some((cut, _)) = text.char_indices().nth(FAILED_MAX_CHARS) {
        text.truncate(cut);
    }
    VerifyOutcome::Failed(Arc::from(text))
}

/// Sanitized, non-empty value of a `desc` section.
fn clean_value(raw: &[u8]) -> Option<Arc<str>> {
    let mut out = String::new();
    sanitize_text(raw, VALUE_MAX, &mut out);
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| Arc::from(trimmed))
}

/// Byte offset of `part` inside `whole`, or `whole.len()` for `None`. `part` must be a subslice.
fn offset_in(whole: &[u8], part: Option<&[u8]>) -> usize {
    part.map_or(whole.len(), |p| {
        p.as_ptr() as usize - whole.as_ptr() as usize
    })
}

/// Output of one pacman run.
struct RunOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum RunError {
    /// The binary does not exist.
    Missing,
    /// The wall limit passed; the child was killed.
    TimedOut,
    Io(std::io::Error),
}

/// Runs `cmd` with `LC_ALL=C`, stdin null and stdout piped, reading at most [`STDOUT_MAX`]
/// bytes. Kills the child after [`COMMAND_TIMEOUT`].
fn run(mut cmd: Command, want_stderr: bool) -> Result<RunOutput, RunError> {
    cmd.env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if want_stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => RunError::Missing,
        _ => RunError::Io(e),
    })?;
    let stdout = child.stdout.take().map(|s| read_capped(s, STDOUT_MAX));
    let stderr = child.stderr.take().map(|s| read_capped(s, STDERR_MAX));
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::Io(e));
            }
        }
    };
    // The child is gone, so its pipes close and the readers finish.
    let stdout = stdout.map(join_reader).unwrap_or_default();
    let stderr = stderr.map(join_reader).unwrap_or_default();
    if timed_out {
        return Err(RunError::TimedOut);
    }
    Ok(RunOutput { stdout, stderr })
}

/// Reads up to `max` bytes on a helper thread and drains the rest, so the child never blocks on
/// a full pipe.
fn read_capped<R: Read + Send + 'static>(
    mut src: R,
    max: usize,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut src).take(max as u64).read_to_end(&mut buf);
        let _ = std::io::copy(&mut src, &mut std::io::sink());
        buf
    })
}

fn join_reader(handle: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

/// One cached owner lookup.
struct CacheEntry {
    mtime_s: i64,
    info: Option<Arc<PackageInfo>>,
}

/// Cache of exe path to owning package, filled in debounced batches.
pub struct PackageIndex {
    cache: FxHashMap<Arc<str>, CacheEntry>,
    /// Package metadata by name, shared by every path the package owns.
    infos: FxHashMap<Arc<str>, Arc<PackageInfo>>,
    /// Requested paths with their mtime, waiting for the next batch.
    pending: FxHashMap<Arc<str>, i64>,
    last_new: Option<Instant>,
    stamp: Option<i64>,
    backend: Box<dyn PkgBackend>,
}

impl PackageIndex {
    pub fn new(backend: Box<dyn PkgBackend>) -> Self {
        Self {
            cache: FxHashMap::default(),
            infos: FxHashMap::default(),
            pending: FxHashMap::default(),
            last_new: None,
            stamp: None,
            backend,
        }
    }

    /// Cache lookup; never blocks. `None` = not looked up yet (the cell shows `…`),
    /// `Some(None)` = not packaged.
    pub fn get(&self, path: &str, mtime_s: i64) -> Option<Option<Arc<PackageInfo>>> {
        self.cache
            .get(path)
            .filter(|e| e.mtime_s == mtime_s)
            .map(|e| e.info.clone())
    }

    /// Queues a path that `get` did not know.
    pub fn request(&mut self, path: &str, mtime_s: i64, now: Instant) {
        if self.get(path, mtime_s).is_some() {
            return;
        }
        match self.pending.get_mut(path) {
            Some(m) => *m = mtime_s,
            None => {
                self.pending.insert(Arc::from(path), mtime_s);
                self.last_new = Some(now);
            }
        }
    }

    /// True when a batch is due: pending paths exist and PKG_DEBOUNCE passed since the last new
    /// one.
    pub fn due(&self, now: Instant) -> bool {
        !self.pending.is_empty()
            && self
                .last_new
                .is_none_or(|t| now.saturating_duration_since(t) >= PKG_DEBOUNCE)
    }

    /// Blocking: runs one batch of at most PKG_BATCH_MAX paths through the backend and fills the
    /// cache. Checks `db_stamp` first and clears the cache when it changed. Returns the number of
    /// paths looked up. On a backend error the batch is dropped; the sweep requests those paths
    /// again.
    pub fn run_batch(&mut self) -> Result<usize, CollectError> {
        let stamp = self.backend.db_stamp();
        if stamp != self.stamp {
            self.cache.clear();
            self.infos.clear();
            self.stamp = stamp;
        }
        let batch: Vec<(Arc<str>, i64)> = self
            .pending
            .iter()
            .take(PKG_BATCH_MAX)
            .map(|(p, m)| (p.clone(), *m))
            .collect();
        for (path, _) in &batch {
            self.pending.remove(path);
        }
        if batch.is_empty() {
            return Ok(0);
        }
        let mtimes: FxHashMap<&[u8], i64> = batch.iter().map(|(p, m)| (p.as_bytes(), *m)).collect();
        let paths: Vec<&Path> = batch.iter().map(|(p, _)| Path::new(&**p)).collect();
        let owners = self.backend.owners(&paths)?;
        let mut filled = 0;
        for (path, owner) in owners {
            let Some(path) = path.to_str() else { continue };
            let Some(&mtime_s) = mtimes.get(path.as_bytes()) else {
                continue;
            };
            let info = owner.map(|name| self.info_for(name));
            self.cache
                .insert(Arc::from(path), CacheEntry { mtime_s, info });
            filled += 1;
        }
        tracing::debug!(paths = batch.len(), filled, "pkg: owner batch done");
        Ok(filled)
    }

    /// Blocking: PROC-078 check for one package.
    pub fn verify(&mut self, name: &str) -> Result<VerifyOutcome, CollectError> {
        self.backend.verify(name)
    }

    /// Metadata of `name`, read once per database stamp. A package whose `desc` cannot be read
    /// still counts as packaged, with its name only.
    fn info_for(&mut self, name: Arc<str>) -> Arc<PackageInfo> {
        if let Some(info) = self.infos.get(&name) {
            return info.clone();
        }
        let info = match self.backend.info(&name) {
            Ok(Some(info)) => info,
            Ok(None) | Err(_) => {
                tracing::debug!("pkg: package metadata not readable");
                PackageInfo {
                    name: name.clone(),
                    version: Arc::from(""),
                    description: None,
                    packager: None,
                    build_date_s: None,
                    url: None,
                }
            }
        };
        let info = Arc::new(info);
        self.infos.insert(name, info.clone());
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicI64;

    /// Fake backend: owns the paths in `owned`, records every owners call.
    #[derive(Default)]
    struct Fake {
        owned: Vec<(&'static str, &'static str)>,
        seen: Arc<Mutex<Vec<usize>>>,
        stamp: Arc<AtomicI64>,
    }

    impl PkgBackend for Fake {
        fn owners(
            &mut self,
            paths: &[&Path],
        ) -> Result<Vec<(PathBuf, Option<Arc<str>>)>, CollectError> {
            self.seen.lock().unwrap().push(paths.len());
            Ok(paths
                .iter()
                .map(|p| {
                    let owner = self
                        .owned
                        .iter()
                        .find(|(path, _)| Path::new(path) == *p)
                        .map(|(_, name)| Arc::from(*name));
                    (p.to_path_buf(), owner)
                })
                .collect())
        }

        fn info(&mut self, name: &str) -> Result<Option<PackageInfo>, CollectError> {
            Ok(Some(PackageInfo {
                name: Arc::from(name),
                version: Arc::from("1-1"),
                description: None,
                packager: None,
                build_date_s: None,
                url: None,
            }))
        }

        fn verify(&mut self, _name: &str) -> Result<VerifyOutcome, CollectError> {
            Ok(VerifyOutcome::Unmodified)
        }

        fn db_stamp(&mut self) -> Option<i64> {
            Some(self.stamp.load(Ordering::Relaxed))
        }
    }

    fn firefox_fake() -> Fake {
        Fake {
            owned: vec![("/usr/bin/firefox", "firefox")],
            ..Fake::default()
        }
    }

    #[test]
    fn lookup_after_batch() {
        let mut index = PackageIndex::new(Box::new(firefox_fake()));
        let t0 = Instant::now();
        assert_eq!(index.get("/usr/bin/firefox", 100), None);
        index.request("/usr/bin/firefox", 100, t0);
        index.request("/home/u/a.out", 5, t0);
        assert!(index.due(t0 + Duration::from_secs(2)));
        assert_eq!(index.run_batch().unwrap(), 2);
        let info = index.get("/usr/bin/firefox", 100).unwrap().unwrap();
        assert_eq!(&*info.name, "firefox");
        assert_eq!(index.get("/home/u/a.out", 5), Some(None));
        // A changed mtime is a new file.
        assert_eq!(index.get("/usr/bin/firefox", 101), None);
        assert!(!index.due(t0 + Duration::from_secs(10)));
    }

    #[test]
    fn debounce_2s() {
        let mut index = PackageIndex::new(Box::new(firefox_fake()));
        let t0 = Instant::now();
        assert!(!index.due(t0));
        index.request("/usr/bin/firefox", 100, t0);
        assert!(!index.due(t0 + Duration::from_secs(1)));
        assert!(index.due(t0 + Duration::from_secs(2)));
        // A repeat request of a pending path does not restart the wait.
        index.request("/usr/bin/firefox", 100, t0 + Duration::from_secs(1));
        assert!(index.due(t0 + Duration::from_secs(2)));
        // A new path does.
        index.request("/usr/bin/other", 1, t0 + Duration::from_secs(1));
        assert!(!index.due(t0 + Duration::from_secs(2)));
        assert!(index.due(t0 + Duration::from_secs(3)));
    }

    #[test]
    fn batch_cap_256() {
        let fake = firefox_fake();
        let seen = fake.seen.clone();
        let mut index = PackageIndex::new(Box::new(fake));
        let t0 = Instant::now();
        for i in 0..300 {
            index.request(&format!("/usr/bin/p{i}"), 1, t0);
        }
        assert_eq!(index.run_batch().unwrap(), PKG_BATCH_MAX);
        assert!(seen.lock().unwrap().iter().all(|&n| n <= PKG_BATCH_MAX));
        assert_eq!(index.pending.len(), 44);
        assert_eq!(index.run_batch().unwrap(), 44);
        assert!(index.pending.is_empty());
    }

    #[test]
    fn db_stamp_clears_cache() {
        let fake = firefox_fake();
        let stamp = fake.stamp.clone();
        let mut index = PackageIndex::new(Box::new(fake));
        let t0 = Instant::now();
        index.request("/usr/bin/firefox", 100, t0);
        index.run_batch().unwrap();
        assert!(index.get("/usr/bin/firefox", 100).is_some());
        // Same stamp: the cache stays.
        index.run_batch().unwrap();
        assert!(index.get("/usr/bin/firefox", 100).is_some());
        stamp.store(7, Ordering::Relaxed);
        index.run_batch().unwrap();
        assert_eq!(index.get("/usr/bin/firefox", 100), None);
        assert!(index.infos.is_empty());
    }

    #[test]
    fn parse_qo_lines() {
        let firefox = Path::new("/usr/bin/firefox");
        let aout = Path::new("/home/u/a.out");
        let got = parse_qo(
            b"/usr/bin/firefox is owned by firefox 131.0-1\n",
            &[firefox, aout],
        );
        assert_eq!(
            got,
            vec![
                (firefox.to_path_buf(), Some(Arc::from("firefox"))),
                (aout.to_path_buf(), None),
            ]
        );
        // A path with spaces and an unrelated line.
        let spaced = Path::new("/opt/my app/bin");
        let got = parse_qo(
            b"error: junk\n/opt/my app/bin is owned by my-app 2-1\n",
            &[spaced],
        );
        assert_eq!(got, vec![(spaced.to_path_buf(), Some(Arc::from("my-app")))]);
        // A non-UTF-8 path maps back by bytes.
        let raw = Path::new(OsStr::from_bytes(b"/usr/bin/\xff"));
        let got = parse_qo(b"/usr/bin/\xff is owned by odd 1-1\n", &[raw]);
        assert_eq!(got[0].1.as_deref(), Some("odd"));
    }

    #[test]
    fn parse_desc_fixture() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pkg-db/local");
        let mut pacman = Pacman { db_dir: dir };
        let info = pacman.info("firefox").unwrap().unwrap();
        assert_eq!(&*info.name, "firefox");
        assert_eq!(&*info.version, "131.0-1");
        assert_eq!(info.packager.as_deref(), Some("Jan Doe"));
        assert_eq!(info.build_date_s, Some(1_727_000_000));
        assert_eq!(
            info.description.as_deref(),
            Some("Fast, Private & Safe Web Browser")
        );
        assert_eq!(
            info.url.as_deref(),
            Some("https://www.mozilla.org/firefox/")
        );
        // A prefix match whose %NAME% differs is not the package.
        assert_eq!(pacman.info("fire").unwrap(), None);
        assert_eq!(pacman.info("chromium").unwrap(), None);
    }

    #[test]
    fn parse_desc_sanitizes_and_needs_name() {
        let info =
            parse_desc(b"%NAME%\nx\n\n%VERSION%\n1\n\n%DESC%\nline one\nline\x1btwo\n\n").unwrap();
        assert_eq!(
            info.description.as_deref(),
            Some("line one line\u{FFFD}two")
        );
        assert_eq!(info.packager, None);
        assert!(parse_desc(b"%VERSION%\n1\n").is_none());
        assert!(parse_desc(b"%NAME%\nx\n\n%VERSION%\n\n").is_none());
    }

    #[test]
    fn parse_qkk_summary() {
        let out = b"backup file: firefox: /etc/x (Size mismatch)\nfirefox: 120 total files, 2 altered files\n";
        assert_eq!(
            parse_qkk("firefox", out, b""),
            VerifyOutcome::Modified { altered_files: 2 }
        );
        assert_eq!(
            parse_qkk(
                "firefox",
                b"firefox: 120 total files, 0 altered files\n",
                b""
            ),
            VerifyOutcome::Unmodified
        );
        assert_eq!(
            parse_qkk("firefox", b"", b"error: package 'firefox' was not found\n"),
            VerifyOutcome::Failed(Arc::from("error: package 'firefox' was not found"))
        );
        let long = vec![b'x'; 500];
        let VerifyOutcome::Failed(text) = parse_qkk("firefox", b"", &long) else {
            panic!("expected Failed");
        };
        assert_eq!(text.chars().count(), FAILED_MAX_CHARS);
    }
}
