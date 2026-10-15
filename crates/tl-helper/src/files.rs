//! `ReadProcessFiles` and `ScanStrings` as root: the fixed set of `/proc/<pid>` files and the
//! memory strings of any user's process (ADR-006, R-004, R-098, R-144, R-157).
//!
//! There is no path argument. The helper verifies the process with `open_verified`, keeps that
//! pidfd open for the whole call, opens `/proc/<pid>` once as a dirfd and reads every entry
//! relative to it with `O_NOFOLLOW`. After the reads it checks the pidfd once more: a process that
//! exited during the call gets `Err(NoSuchProcess)` and none of the data, so data from another
//! process is never sent.
//!
//! `ReadProcessFiles` reply shape: each requested file, in ascending bit order, gives one or more
//! `File { file, data }` bodies (split at 60 KiB) or exactly one `Err(Os(errno))` body. The data
//! of the whole reply stays under 4 MiB minus 4 KiB of headroom; the file that would pass it and
//! every later one get `Err(Os(EFBIG))`.
//!
//! `ScanStrings` runs tl-collect's `StringScanner` over `/proc/<pid>/mem` and streams `Strings`
//! batches of at most 2,000 strings or 48 KiB, so each fits one 64 KiB frame (R-157). The final
//! reply is the last batch, `Err(TooLarge)` at the scan limits, `Err(NoSuchProcess)` when the
//! process exits, or `Err(Os(errno))` when no region can be read. Kernel threads get
//! `Err(NotAllowed)`.
//!
//! The helper does not log (R-281). File contents, paths, environments and found strings never
//! leave this module except in the reply (ADR-015).

use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Dir, Mode, OFlags};
use rustix::io::Errno;
use tl_collect::proc::strings::{
    self, Encoding, MemSource, ProcMem, ScanEnd, ScanOpts, StringScanner,
};
use tl_collect::proc::{detail, pid_stat};
use tl_collect::{CollectError, SysRoot};
use tl_model::details::{StringEncoding, StringScanOpts};
use tl_model::helper_proto::{HelperError, HelperFrame, HelperReplyBody, HelperRequest, ProcFiles};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_ops::{OpsError, process};

/// The total reply cap for `ReadProcessFiles` (ADR-006).
const REPLY_CAP: usize = 4_194_304;
/// Room left under `REPLY_CAP` for the reply and frame headers.
const HEADROOM: usize = 4096;
/// The most file data one reply carries.
const DATA_BUDGET: usize = REPLY_CAP - HEADROOM;
/// The most data in one `File` body, so each reply fits one 64 KiB frame.
const CHUNK: usize = 60 * 1024;
/// The `ns/` links `NS` reads, in this order. Names the kernel does not have are skipped.
const NS_NAMES: [&str; 10] = [
    "cgroup",
    "ipc",
    "mnt",
    "net",
    "pid",
    "pid_for_children",
    "time",
    "time_for_children",
    "user",
    "uts",
];
/// Strings per `Strings` batch, at most.
const BATCH_STRINGS: usize = 2_000;
/// Counted bytes that end a `Strings` batch. A batch is sent once it reaches this, so it holds
/// at most this plus one entry: 48 KiB plus 4,113 bytes, well under one 64 KiB frame.
const BATCH_BYTES: usize = 48 * 1024;
/// Postcard bytes of one batch entry besides its text, at most: the address varint (10), the
/// encoding (1) and the text length varint (3).
const ENTRY_OVERHEAD: usize = 14;
/// The most `stat` bytes read.
const STAT_MAX: usize = 4096;
/// The most `maps` bytes read, as tl-collect's detail reader.
const MAPS_MAX: usize = 16 * 1024 * 1024;

/// Answers `ReadProcessFiles { key, files }`. Returns the reply bodies in send order; `dispatch`
/// sets `last` on the final one. Unknown bits in `files` are ignored. An empty set gets
/// `Err(NotAllowed)` before any syscall.
pub fn read_process_files(key: ProcessKey, files: ProcFiles, boot: BootId) -> Vec<HelperReplyBody> {
    read_with_hook(key, files, boot, || {})
}

/// `read_process_files` with a hook that runs after the reads and before the final pidfd check.
/// Tests use it to end the process at that point.
fn read_with_hook(
    key: ProcessKey,
    files: ProcFiles,
    boot: BootId,
    before_post_check: impl FnOnce(),
) -> Vec<HelperReplyBody> {
    let files = files & ProcFiles::all();
    // Rule "empty file set".
    if files.is_empty() {
        return vec![HelperReplyBody::Err(HelperError::NotAllowed)];
    }
    let pidfd = match process::open_verified(key, boot) {
        Ok(fd) => fd,
        Err(OpsError::IdentityChanged) => return only(HelperError::IdentityChanged),
        Err(OpsError::NoSuchProcess) => return only(HelperError::NoSuchProcess),
        Err(e) => return every_file(files, ops_errno(&e)),
    };
    let dir = match open_proc_dir(key.pid) {
        Ok(dir) => dir,
        Err(_) if has_exited(&pidfd) => return only(HelperError::NoSuchProcess),
        Err(e) => return every_file(files, e),
    };
    // The pidfd process was alive after the dirfd was open, so the pid was not reused yet and the
    // dirfd is that process's directory.
    if has_exited(&pidfd) {
        return only(HelperError::NoSuchProcess);
    }

    let mut bodies = Vec::new();
    let mut left = DATA_BUDGET;
    let mut over_budget = false;
    for file in ProcFiles::all().iter().filter(|f| files.contains(*f)) {
        if over_budget {
            bodies.push(os_error(Errno::FBIG));
            continue;
        }
        match read_one(&dir, file, left) {
            Ok(data) => {
                left -= data.len();
                push_chunks(&mut bodies, file, data);
            }
            Err(Errno::FBIG) => {
                over_budget = true;
                bodies.push(os_error(Errno::FBIG));
            }
            Err(e) => bodies.push(os_error(e)),
        }
    }

    before_post_check();
    if has_exited(&pidfd) {
        return only(HelperError::NoSuchProcess);
    }
    drop(pidfd);
    bodies
}

/// One `Err` body as the whole reply.
fn only(error: HelperError) -> Vec<HelperReplyBody> {
    vec![HelperReplyBody::Err(error)]
}

fn os_error(errno: Errno) -> HelperReplyBody {
    HelperReplyBody::Err(HelperError::Os(errno.raw_os_error()))
}

/// One `Err(Os(errno))` body for each requested file, for a failure before any file was read.
fn every_file(files: ProcFiles, errno: Errno) -> Vec<HelperReplyBody> {
    files.iter().map(|_| os_error(errno)).collect()
}

/// The errno behind an `open_verified` error other than `IdentityChanged` and `NoSuchProcess`.
fn ops_errno(e: &OpsError) -> Errno {
    match e {
        OpsError::Os(errno) => Errno::from_raw_os_error(*errno),
        OpsError::PermissionDenied => Errno::ACCESS,
        OpsError::NoSuchProcess => Errno::SRCH,
        _ => Errno::IO,
    }
}

/// True when the process behind `pidfd` has exited, or when the check itself fails, so a failed
/// check never lets data through. Does not block.
fn has_exited(pidfd: &OwnedFd) -> bool {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    let zero = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match rustix::event::poll(&mut fds, Some(&zero)) {
        Ok(_) => fds[0]
            .revents()
            .intersects(PollFlags::IN | PollFlags::ERR | PollFlags::NVAL),
        Err(_) => true,
    }
}

/// Appends `data` as `File` bodies of at most `CHUNK` bytes. Empty data gives one empty body.
fn push_chunks(bodies: &mut Vec<HelperReplyBody>, file: ProcFiles, data: Vec<u8>) {
    if data.len() <= CHUNK {
        bodies.push(HelperReplyBody::File { file, data });
        return;
    }
    for chunk in data.chunks(CHUNK) {
        bodies.push(HelperReplyBody::File {
            file,
            data: chunk.to_vec(),
        });
    }
}

/// Reads one file of the set, at most `left` bytes. `Err(FBIG)` when it does not fit.
fn read_one(dir: &OwnedFd, file: ProcFiles, left: usize) -> Result<Vec<u8>, Errno> {
    let name = match file {
        ProcFiles::FD_LIST => return read_fd_list(dir, left),
        ProcFiles::FDINFO => return read_fdinfo(dir, left),
        ProcFiles::NS => return read_ns(dir, left),
        ProcFiles::CWD => return fit(read_link(dir, "cwd")?, left),
        ProcFiles::EXE => return fit(read_link(dir, "exe")?, left),
        ProcFiles::ROOT => return fit(read_link(dir, "root")?, left),
        ProcFiles::IO => "io",
        ProcFiles::ENVIRON => "environ",
        ProcFiles::MAPS => "maps",
        ProcFiles::SMAPS => "smaps",
        ProcFiles::SMAPS_ROLLUP => "smaps_rollup",
        ProcFiles::STATUS => "status",
        // `read_with_hook` passes single known bits only.
        _ => return Err(Errno::INVAL),
    };
    read_capped(open_at(dir, name, OFlags::empty())?, left)
}

fn fit(data: Vec<u8>, left: usize) -> Result<Vec<u8>, Errno> {
    if data.len() > left {
        return Err(Errno::FBIG);
    }
    Ok(data)
}

/// Opens `/proc/<pid>` as a directory.
fn open_proc_dir(pid: u32) -> Result<OwnedFd, Errno> {
    note_open();
    rustix::fs::open(
        format!("/proc/{pid}").as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
}

/// Opens `name` relative to `dir`, never following a symlink.
fn open_at(dir: impl AsFd, name: &str, extra: OFlags) -> Result<OwnedFd, Errno> {
    note_open();
    rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOCTTY | extra,
        Mode::empty(),
    )
}

/// The target of the symlink `name` relative to `dir`, without following it.
fn read_link(dir: impl AsFd, name: &str) -> Result<Vec<u8>, Errno> {
    note_open();
    Ok(rustix::fs::readlinkat(dir, name, Vec::new())?.into_bytes())
}

/// Reads `fd` to the end, at most `left` bytes. `Err(FBIG)` when there is more.
fn read_capped(fd: OwnedFd, left: usize) -> Result<Vec<u8>, Errno> {
    // Most /proc files report size 0, so the size is only a starting capacity.
    let hint = rustix::fs::fstat(&fd)
        .ok()
        .and_then(|st| usize::try_from(st.st_size).ok())
        .unwrap_or(0)
        .min(left);
    let mut data = Vec::with_capacity(hint);
    std::fs::File::from(fd)
        .take(left as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| Errno::from_io_error(&e).unwrap_or(Errno::IO))?;
    fit(data, left)
}

/// The open fd numbers of the process, sorted, from `fd/` (an open dirfd).
fn list_fds(fd_dir: &OwnedFd) -> Result<Vec<u32>, Errno> {
    let mut fds = Vec::new();
    for entry in Dir::read_from(fd_dir)? {
        let entry = entry?;
        if let Some(fd) = entry
            .file_name()
            .to_str()
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        {
            fds.push(fd);
        }
    }
    fds.sort_unstable();
    Ok(fds)
}

/// The postcard size of one `(u32, Vec<u8>)` entry, at most: two varints and the bytes.
fn entry_size(len: usize) -> usize {
    5 + 10 + len
}

/// Encodes a list with postcard, `Err(FBIG)` when it does not fit in `left`.
fn encode<T: serde::Serialize>(list: &T, left: usize) -> Result<Vec<u8>, Errno> {
    fit(postcard::to_allocvec(list).map_err(|_| Errno::IO)?, left)
}

/// `FD_LIST`: postcard `Vec<(u32, Vec<u8>)>` of (fd, link target), sorted by fd. An fd that
/// closes during the read is skipped.
fn read_fd_list(dir: &OwnedFd, left: usize) -> Result<Vec<u8>, Errno> {
    let fd_dir = open_at(dir, "fd", OFlags::DIRECTORY)?;
    let mut list: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut size = 10;
    for fd in list_fds(&fd_dir)? {
        match read_link(&fd_dir, &fd.to_string()) {
            Ok(target) => {
                size += entry_size(target.len());
                if size > left {
                    return Err(Errno::FBIG);
                }
                list.push((fd, target));
            }
            Err(Errno::NOENT) => {}
            Err(e) => return Err(e),
        }
    }
    encode(&list, left)
}

/// `FDINFO`: postcard `Vec<(u32, Vec<u8>)>` of (fd, `fdinfo/<fd>` bytes), sorted by fd. An fd
/// that closes during the read is skipped.
fn read_fdinfo(dir: &OwnedFd, left: usize) -> Result<Vec<u8>, Errno> {
    let fd_dir = open_at(dir, "fd", OFlags::DIRECTORY)?;
    let fds = list_fds(&fd_dir)?;
    drop(fd_dir);
    let info_dir = open_at(dir, "fdinfo", OFlags::DIRECTORY)?;
    let mut list: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut size = 10;
    for fd in fds {
        let room = left.saturating_sub(size + entry_size(0));
        let info = match open_at(&info_dir, &fd.to_string(), OFlags::empty()) {
            Ok(file) => read_capped(file, room),
            Err(e) => Err(e),
        };
        match info {
            Ok(info) => {
                size += entry_size(info.len());
                list.push((fd, info));
            }
            Err(Errno::NOENT) => {}
            Err(e) => return Err(e),
        }
    }
    encode(&list, left)
}

/// `NS`: postcard `Vec<(String, Vec<u8>)>` of (name, link target) for `NS_NAMES`, missing names
/// skipped.
fn read_ns(dir: &OwnedFd, left: usize) -> Result<Vec<u8>, Errno> {
    let ns_dir = open_at(dir, "ns", OFlags::DIRECTORY)?;
    let mut list: Vec<(String, Vec<u8>)> = Vec::new();
    for name in NS_NAMES {
        match read_link(&ns_dir, name) {
            Ok(target) => list.push((name.to_owned(), target)),
            Err(Errno::NOENT) => {}
            Err(e) => return Err(e),
        }
    }
    encode(&list, left)
}

/// Answers `ScanStrings { key, opts }`. Sends every reply through `out`, with `last` true on the
/// final one only, and returns the first error `out` gives. Calls `cancel` before each chunk;
/// true ends the scan with an empty final `Strings`, and found strings not yet sent are dropped.
pub fn scan_strings(
    key: ProcessKey,
    opts: &StringScanOpts,
    boot: BootId,
    out: &mut dyn FnMut(HelperReplyBody, bool) -> std::io::Result<()>,
    cancel: &mut dyn FnMut() -> bool,
) -> std::io::Result<()> {
    scan_with_limits(
        key,
        opts,
        boot,
        out,
        cancel,
        strings::MAX_STRINGS,
        strings::MAX_TEXT_BYTES,
    )
}

/// True when `frame` is the `CancelScan` for the running `ScanStrings` frame `scan_seq` (R-157).
pub fn is_cancel_for(frame: &HelperFrame, scan_seq: u32) -> bool {
    matches!(frame.request, HelperRequest::CancelScan { seq } if seq == scan_seq)
}

/// `scan_strings` with the stop limits as arguments, so tests can lower them.
fn scan_with_limits(
    key: ProcessKey,
    opts: &StringScanOpts,
    boot: BootId,
    out: &mut dyn FnMut(HelperReplyBody, bool) -> std::io::Result<()>,
    cancel: &mut dyn FnMut() -> bool,
    max_strings: u32,
    max_text: u64,
) -> std::io::Result<()> {
    let (pidfd, scanner, mut mem) = match prepare_scan(key, opts, boot) {
        Ok(scan) => scan,
        Err(error) => return out(HelperReplyBody::Err(error), true),
    };
    let mut scanner = scanner.with_limits(max_strings, max_text);

    let mut buf = Vec::with_capacity(strings::CHUNK);
    let mut hits = Vec::new();
    let mut batch: Vec<(u64, StringEncoding, String)> = Vec::new();
    let mut batch_bytes = 0;
    loop {
        if cancel() {
            return out(HelperReplyBody::Strings(Vec::new()), true);
        }
        if has_exited(&pidfd) {
            return out(HelperReplyBody::Err(HelperError::NoSuchProcess), true);
        }
        let end = scanner.step(&mut mem, &mut buf, &mut hits);
        for hit in hits.drain(..) {
            batch_bytes += hit.value.len() + ENTRY_OVERHEAD;
            batch.push((
                hit.address,
                string_encoding(hit.encoding),
                hit.value.into_string(),
            ));
            if batch.len() >= BATCH_STRINGS || batch_bytes >= BATCH_BYTES {
                batch_bytes = 0;
                // `out` takes the body by value, so each sent batch is one exact-size Vec.
                // `append` moves the entries and leaves `batch` empty with its capacity kept.
                let mut sent = Vec::with_capacity(batch.len());
                sent.append(&mut batch);
                out(HelperReplyBody::Strings(sent), false)?;
            }
        }
        let Some(end) = end else {
            continue;
        };
        if has_exited(&pidfd) {
            return out(HelperReplyBody::Err(HelperError::NoSuchProcess), true);
        }
        return match end {
            ScanEnd::Finished => out(HelperReplyBody::Strings(batch), true),
            // `step` never returns it; the cancel path above is the only one.
            ScanEnd::Cancelled => out(HelperReplyBody::Strings(Vec::new()), true),
            ScanEnd::Stopped => {
                if !batch.is_empty() {
                    out(HelperReplyBody::Strings(batch), false)?;
                }
                out(HelperReplyBody::Err(HelperError::TooLarge), true)
            }
            ScanEnd::Failed => {
                let errno = mem.last_errno.unwrap_or(Errno::IO);
                out(HelperReplyBody::Err(os_err(errno)), true)
            }
        };
    }
}

/// Verifies `key`, checks the kernel thread rule, reads the regions and opens `mem`. Returns the
/// pidfd that stays open for the scan, the scanner and the memory source.
fn prepare_scan(
    key: ProcessKey,
    opts: &StringScanOpts,
    boot: BootId,
) -> Result<(OwnedFd, StringScanner, ScanMem), HelperError> {
    let pidfd = match process::open_verified(key, boot) {
        Ok(fd) => fd,
        Err(OpsError::IdentityChanged) => return Err(HelperError::IdentityChanged),
        Err(OpsError::NoSuchProcess) => return Err(HelperError::NoSuchProcess),
        Err(e) => return Err(os_err(ops_errno(&e))),
    };
    let fail = |errno: Errno| {
        if has_exited(&pidfd) {
            HelperError::NoSuchProcess
        } else {
            os_err(errno)
        }
    };
    let dir = open_proc_dir(key.pid).map_err(fail)?;
    // As in `read_with_hook`: alive after the dirfd was open, so the dirfd is this process's.
    if has_exited(&pidfd) {
        return Err(HelperError::NoSuchProcess);
    }

    let stat = open_at(&dir, "stat", OFlags::empty())
        .and_then(|fd| read_capped(fd, STAT_MAX))
        .map_err(fail)?;
    let mut parsed = pid_stat::PidStat::default();
    pid_stat::parse_pid_stat(&stat, &mut parsed).map_err(|_| os_err(Errno::IO))?;
    // Rule "kernel thread".
    if parsed.flags & pid_stat::PF_KTHREAD != 0 {
        return Err(HelperError::NotAllowed);
    }

    let maps = open_at(&dir, "maps", OFlags::empty())
        .and_then(|fd| read_capped(fd, MAPS_MAX))
        .map_err(fail)?;
    let mut infos = Vec::new();
    detail::parse_maps(&maps, &mut infos).map_err(|_| os_err(Errno::IO))?;
    let mut regions = Vec::new();
    strings::select_regions(&infos, opts.include_file_backed, &mut regions);

    // `ProcMem::open` reopens `/proc/<pid>` by path and checks the start time. The pidfd check
    // after it proves the pid was not reused in between.
    let mem = ProcMem::open(&SysRoot::host(), key).map_err(|e| match e {
        CollectError::ProcessGone { .. } | CollectError::NotFound => HelperError::NoSuchProcess,
        CollectError::PermissionDenied => HelperError::PermissionDenied,
        CollectError::Os(errno) => os_err(errno),
        _ => os_err(Errno::IO),
    })?;
    if has_exited(&pidfd) {
        return Err(HelperError::NoSuchProcess);
    }

    let scan_opts = ScanOpts {
        min_len: opts.min_len,
        ascii: opts.ascii,
        utf8: opts.utf8,
        utf16le: opts.utf16le,
        include_file_backed: opts.include_file_backed,
    };
    let scanner = StringScanner::new(scan_opts, regions);
    Ok((
        pidfd,
        scanner,
        ScanMem {
            mem,
            last_errno: None,
        },
    ))
}

/// `ProcMem` that keeps the errno of its last failed read, for the final `Err(Os(errno))` of a
/// scan that could read no region. The scanner itself turns read errors into holes.
struct ScanMem {
    mem: ProcMem,
    last_errno: Option<Errno>,
}

impl MemSource for ScanMem {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> Result<usize, CollectError> {
        let result = self.mem.read(addr, buf);
        if let Err(e) = &result {
            self.last_errno = Some(match e {
                CollectError::Os(errno) => *errno,
                CollectError::PermissionDenied => Errno::ACCESS,
                CollectError::NotFound | CollectError::ProcessGone { .. } => Errno::SRCH,
                _ => Errno::IO,
            });
        }
        result
    }
}

fn string_encoding(encoding: Encoding) -> StringEncoding {
    match encoding {
        Encoding::Ascii => StringEncoding::Ascii,
        Encoding::Utf8 => StringEncoding::Utf8,
        Encoding::Utf16Le => StringEncoding::Utf16Le,
    }
}

fn os_err(errno: Errno) -> HelperError {
    HelperError::Os(errno.raw_os_error())
}

#[cfg(test)]
thread_local! {
    /// Opens and readlinks on this thread, so a test can prove none happened.
    static OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn note_open() {
    #[cfg(test)]
    OPENS.with(|n| n.set(n.get() + 1));
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use tl_model::actions::Signal;
    use tl_model::helper_proto::{HelperReply, HelperTarget};

    use super::*;

    const BOOT: BootId = BootId(0x5eed_0000_0000_0007);
    /// Threads in the mapping child: each adds two mappings (stack and guard page).
    const MAPPING_THREADS: usize = 3000;
    /// Set in the strings child's environment. The markers themselves never go in argv or env.
    const STRINGS_CHILD_ENV: &str = "TL_HELPER_STRINGS_CHILD";
    const ASCII_MARKER: &str = "tasklens-marker-1234";
    const UTF16_MARKER: &str = "wine-marker";
    /// Extra ASCII markers `tl-mark-NN-xyz` in the strings child.
    const MARKERS: usize = 50;
    /// Heap strings of 4,000 `q` in the strings child, so the scan sends several batches.
    const LONG_STRINGS: usize = 100;

    /// A child that is killed and reaped on drop, even when an assertion fails.
    struct ChildGuard(Child);

    impl ChildGuard {
        /// `sleep 60` with `TL_TEST=1`, the crate directory as cwd and `Cargo.toml` open on fd 7.
        fn sleeper() -> Self {
            let mut guard = ChildGuard(
                Command::new("sh")
                    .args(["-c", r#"exec 7<"$TL_FD7"; exec sleep 60"#])
                    .env("TL_TEST", "1")
                    .env("TL_FD7", fd7_target())
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .stdin(Stdio::null())
                    .spawn()
                    .expect("spawn sh"),
            );
            let pid = guard.pid();
            let execed = wait_for(|| {
                std::fs::read(format!("/proc/{pid}/comm")).is_ok_and(|c| c == b"sleep\n")
            });
            assert!(execed, "child never ran sleep");
            assert!(guard.0.try_wait().expect("try_wait").is_none());
            guard
        }

        /// This test binary with many threads, after it printed `ready`.
        fn mapping() -> Self {
            Self::entry(
                "files::tests::mapping_child_entry",
                "TL_HELPER_FILES_THREADS",
                &MAPPING_THREADS.to_string(),
            )
        }

        /// This test binary holding the scan markers in heap memory, after it printed `ready`.
        fn strings() -> Self {
            Self::entry("files::tests::strings_child_entry", STRINGS_CHILD_ENV, "1")
        }

        /// This test binary running only the test `entry` with `env=value`, after it printed
        /// `ready`.
        fn entry(entry: &str, env: &str, value: &str) -> Self {
            let mut child = Command::new(std::env::current_exe().expect("current_exe"))
                .args(["--exact", entry, "--nocapture"])
                .env(env, value)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn child");
            let stderr = child.stderr.take().expect("child stderr");
            let guard = ChildGuard(child);
            let ready = std::io::BufReader::new(stderr)
                .lines()
                .any(|line| line.is_ok_and(|l| l.trim() == "ready"));
            assert!(ready, "{entry} never printed ready");
            guard
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }

        fn key(&self) -> ProcessKey {
            key_of(self.pid())
        }

        fn end(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.end();
        }
    }

    fn key_of(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: process::read_start_ticks(pid).expect("read start ticks"),
            boot_id: BOOT,
            pid,
        }
    }

    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn fd7_target() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
    }

    fn is_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    /// Splits the reply into one slot per requested file and checks the shape: slots in
    /// ascending bit order, each either `File` bodies of that one bit (only the last may be
    /// shorter than `CHUNK`) or exactly one `Err(Os(_))`.
    fn slots(
        bodies: &[HelperReplyBody],
        files: ProcFiles,
    ) -> Vec<(ProcFiles, Result<Vec<u8>, i32>)> {
        let mut out = Vec::new();
        let mut rest = bodies;
        for file in files.iter() {
            match rest.first() {
                Some(HelperReplyBody::Err(HelperError::Os(errno))) => {
                    out.push((file, Err(*errno)));
                    rest = &rest[1..];
                }
                Some(HelperReplyBody::File { .. }) => {
                    let mut data = Vec::new();
                    while let Some(HelperReplyBody::File { file: f, data: d }) = rest.first() {
                        if *f != file {
                            break;
                        }
                        assert!(d.len() <= CHUNK, "chunk over 60 KiB");
                        assert!(data.len() % CHUNK == 0, "short chunk before the last");
                        data.extend_from_slice(d);
                        rest = &rest[1..];
                    }
                    out.push((file, Ok(data)));
                }
                other => panic!("slot {file:?}: unexpected {other:?}"),
            }
        }
        assert!(rest.is_empty(), "bodies left after the last slot: {rest:?}");
        out
    }

    fn data(slots: &[(ProcFiles, Result<Vec<u8>, i32>)], file: ProcFiles) -> Vec<u8> {
        let (_, result) = slots
            .iter()
            .find(|(f, _)| *f == file)
            .expect("slot present");
        result.clone().expect("file read")
    }

    fn encoded_size(bodies: &[HelperReplyBody]) -> usize {
        bodies
            .iter()
            .map(|body| {
                let reply = HelperReply {
                    seq: u32::MAX,
                    last: true,
                    body: body.clone(),
                };
                postcard::to_allocvec(&reply).expect("encode").len()
            })
            .sum()
    }

    fn sleep_binary() -> PathBuf {
        let path = std::env::var_os("PATH").expect("PATH");
        std::env::split_paths(&path)
            .map(|dir| dir.join("sleep"))
            .find(|p| p.is_file())
            .expect("sleep on PATH")
    }

    #[test]
    fn reads_own_child_files() {
        let child = ChildGuard::sleeper();
        let files = ProcFiles::ENVIRON | ProcFiles::CWD | ProcFiles::EXE | ProcFiles::STATUS;
        let bodies = read_process_files(child.key(), files, BOOT);
        let slots = slots(&bodies, files);

        let environ = data(&slots, ProcFiles::ENVIRON);
        assert!(environ.split(|&b| b == 0).any(|v| v == b"TL_TEST=1"));
        let cwd = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).expect("canonicalize cwd");
        assert_eq!(
            data(&slots, ProcFiles::CWD),
            cwd.as_os_str().as_encoded_bytes()
        );
        let exe = std::fs::canonicalize(sleep_binary()).expect("canonicalize sleep");
        assert_eq!(
            data(&slots, ProcFiles::EXE),
            exe.as_os_str().as_encoded_bytes()
        );
        assert!(data(&slots, ProcFiles::STATUS).starts_with(b"Name:"));
    }

    #[test]
    fn fd_list_and_fdinfo() {
        let child = ChildGuard::sleeper();
        let files = ProcFiles::FD_LIST | ProcFiles::FDINFO;
        let bodies = read_process_files(child.key(), files, BOOT);
        let slots = slots(&bodies, files);

        let fds: Vec<(u32, Vec<u8>)> =
            postcard::from_bytes(&data(&slots, ProcFiles::FD_LIST)).expect("decode fd list");
        assert!(
            fds.windows(2).all(|w| w[0].0 < w[1].0),
            "fd list not sorted"
        );
        let target = std::fs::canonicalize(fd7_target()).expect("canonicalize fd 7 target");
        let fd7 = fds.iter().find(|(fd, _)| *fd == 7).expect("fd 7 listed");
        assert_eq!(fd7.1, target.as_os_str().as_encoded_bytes());

        let infos: Vec<(u32, Vec<u8>)> =
            postcard::from_bytes(&data(&slots, ProcFiles::FDINFO)).expect("decode fdinfo");
        let info7 = infos.iter().find(|(fd, _)| *fd == 7).expect("fdinfo 7");
        assert!(info7.1.starts_with(b"pos:"));
    }

    #[test]
    fn reads_ns_links() {
        let child = ChildGuard::sleeper();
        let bodies = read_process_files(child.key(), ProcFiles::NS, BOOT);
        let slots = slots(&bodies, ProcFiles::NS);
        let ns: Vec<(String, Vec<u8>)> =
            postcard::from_bytes(&data(&slots, ProcFiles::NS)).expect("decode ns");
        let pid = ns.iter().find(|(name, _)| name == "pid").expect("pid ns");
        assert!(pid.1.starts_with(b"pid:["));
    }

    #[test]
    fn reads_root_link() {
        let child = ChildGuard::sleeper();
        let bodies = read_process_files(child.key(), ProcFiles::ROOT, BOOT);
        assert_eq!(
            bodies,
            vec![HelperReplyBody::File {
                file: ProcFiles::ROOT,
                data: b"/".to_vec(),
            }]
        );
    }

    #[test]
    fn bit_order_and_error_slots() {
        if is_root() {
            return;
        }
        let files = ProcFiles::IO | ProcFiles::ENVIRON | ProcFiles::ROOT;
        let bodies = read_process_files(key_of(1), files, BOOT);
        let slots = slots(&bodies, files);
        let order: Vec<ProcFiles> = slots.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            order,
            vec![ProcFiles::IO, ProcFiles::ENVIRON, ProcFiles::ROOT]
        );
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[1], os_error(Errno::ACCESS));
    }

    #[test]
    fn stale_key_rejected() {
        let child = ChildGuard::sleeper();
        let files = ProcFiles::all();
        for key in [
            ProcessKey {
                start_ticks: child.key().start_ticks + 1,
                ..child.key()
            },
            ProcessKey {
                boot_id: BootId(BOOT.0 + 1),
                ..child.key()
            },
        ] {
            OPENS.with(|n| n.set(0));
            let bodies = read_process_files(key, files, BOOT);
            assert_eq!(bodies, only(HelperError::IdentityChanged));
            assert_eq!(OPENS.with(|n| n.get()), 0, "a file was opened");
        }
    }

    #[test]
    fn exit_during_read_discards_data() {
        let mut child = ChildGuard::sleeper();
        let files = ProcFiles::ENVIRON | ProcFiles::MAPS | ProcFiles::SMAPS | ProcFiles::STATUS;
        let bodies = read_with_hook(child.key(), files, BOOT, || child.end());
        assert_eq!(bodies, only(HelperError::NoSuchProcess));
    }

    #[test]
    fn budget_caps_reply_at_4_mib() {
        let child = ChildGuard::mapping();
        let pid = child.pid();
        let maps_len = std::fs::read(format!("/proc/{pid}/maps"))
            .expect("read maps")
            .len();
        let smaps_len = std::fs::read(format!("/proc/{pid}/smaps"))
            .expect("read smaps")
            .len();
        assert!(maps_len > CHUNK, "maps must span several chunks");
        assert!(
            maps_len + smaps_len > DATA_BUDGET,
            "smaps fits; add threads"
        );

        let files = ProcFiles::MAPS | ProcFiles::SMAPS;
        let bodies = read_process_files(child.key(), files, BOOT);
        let slots = slots(&bodies, files);
        assert!(!data(&slots, ProcFiles::MAPS).is_empty());
        assert_eq!(slots[1], (ProcFiles::SMAPS, Err(27)));
        assert!(encoded_size(&bodies) <= REPLY_CAP);
    }

    #[test]
    fn one_file_error_keeps_others() {
        if is_root() {
            return;
        }
        let files = ProcFiles::ENVIRON | ProcFiles::STATUS;
        let bodies = read_process_files(key_of(1), files, BOOT);
        let slots = slots(&bodies, files);
        assert_eq!(slots[0], (ProcFiles::ENVIRON, Err(13)));
        assert!(data(&slots, ProcFiles::STATUS).starts_with(b"Name:"));
    }

    #[test]
    fn empty_set_not_allowed() {
        OPENS.with(|n| n.set(0));
        let bodies = read_process_files(key_of(std::process::id()), ProcFiles::empty(), BOOT);
        assert_eq!(bodies, only(HelperError::NotAllowed));
        assert_eq!(OPENS.with(|n| n.get()), 0);
    }

    /// The body of the mapping child. Does nothing in a normal test run.
    #[test]
    fn mapping_child_entry() {
        let Some(count) = std::env::var("TL_HELPER_FILES_THREADS")
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
        else {
            return;
        };
        for _ in 0..count {
            std::thread::Builder::new()
                .stack_size(16 * 1024)
                .spawn(|| {
                    loop {
                        std::thread::park();
                    }
                })
                .expect("spawn thread");
        }
        eprintln!("ready");
        loop {
            std::thread::park();
        }
    }

    type Replies = Vec<(HelperReplyBody, bool)>;

    /// Runs a scan with the given limits and `cancel`, collecting every reply.
    fn scan_limited(
        key: ProcessKey,
        opts: &StringScanOpts,
        cancel: &mut dyn FnMut() -> bool,
        max_strings: u32,
        max_text: u64,
    ) -> Replies {
        let mut replies = Vec::new();
        let mut out = |body: HelperReplyBody, last: bool| {
            replies.push((body, last));
            Ok(())
        };
        scan_with_limits(key, opts, BOOT, &mut out, cancel, max_strings, max_text)
            .expect("out never fails");
        replies
    }

    /// Runs `scan_strings` with no cancel, collecting every reply.
    fn scan(key: ProcessKey, opts: &StringScanOpts) -> Replies {
        let mut replies = Vec::new();
        let mut out = |body: HelperReplyBody, last: bool| {
            replies.push((body, last));
            Ok(())
        };
        scan_strings(key, opts, BOOT, &mut out, &mut || false).expect("out never fails");
        replies
    }

    /// Checks that only the final reply has `last` set.
    fn assert_one_last(replies: &Replies) {
        let (final_reply, rest) = replies.split_last().expect("at least one reply");
        assert!(final_reply.1, "the final reply has last false");
        assert!(
            rest.iter().all(|(_, last)| !last),
            "last set before the end"
        );
    }

    fn found(replies: &Replies) -> Vec<(u64, StringEncoding, String)> {
        replies
            .iter()
            .filter_map(|(body, _)| match body {
                HelperReplyBody::Strings(batch) => Some(batch.iter().cloned()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn scan_finds_ascii_and_utf16() {
        let child = ChildGuard::strings();
        let replies = scan(child.key(), &StringScanOpts::default());
        assert_one_last(&replies);
        assert!(matches!(
            replies.last(),
            Some((HelperReplyBody::Strings(_), true))
        ));
        let found = found(&replies);
        assert!(
            found
                .iter()
                .any(|(_, enc, v)| *enc == StringEncoding::Ascii && v == ASCII_MARKER),
            "ASCII marker not found"
        );
        assert!(
            found
                .iter()
                .any(|(_, enc, v)| *enc == StringEncoding::Utf16Le && v == UTF16_MARKER),
            "UTF-16LE marker not found"
        );
    }

    #[test]
    fn scan_min_len_filters() {
        let child = ChildGuard::strings();
        let opts = StringScanOpts {
            min_len: 30,
            ..StringScanOpts::default()
        };
        let replies = scan(child.key(), &opts);
        assert_one_last(&replies);
        let found = found(&replies);
        assert!(!found.is_empty(), "the 4,000 byte strings are still found");
        assert!(
            found
                .iter()
                .all(|(_, _, v)| !v.contains(ASCII_MARKER) && !v.contains(UTF16_MARKER)),
            "a marker shorter than 30 chars was reported"
        );
    }

    #[test]
    fn scan_cancel_stops() {
        let child = ChildGuard::strings();
        let mut calls = 0;
        let mut cancel = || {
            calls += 1;
            calls >= 2
        };
        let replies = scan_limited(
            child.key(),
            &StringScanOpts::default(),
            &mut cancel,
            strings::MAX_STRINGS,
            strings::MAX_TEXT_BYTES,
        );
        assert_eq!(calls, 2, "the scan read more chunks after the cancel");
        assert_one_last(&replies);
        assert_eq!(
            replies.last(),
            Some(&(HelperReplyBody::Strings(Vec::new()), true))
        );
    }

    #[test]
    fn scan_stale_key_rejected() {
        let child = ChildGuard::strings();
        for key in [
            ProcessKey {
                start_ticks: child.key().start_ticks + 1,
                ..child.key()
            },
            ProcessKey {
                boot_id: BootId(BOOT.0 + 1),
                ..child.key()
            },
        ] {
            OPENS.with(|n| n.set(0));
            let replies = scan(key, &StringScanOpts::default());
            assert_eq!(
                replies,
                vec![(HelperReplyBody::Err(HelperError::IdentityChanged), true)]
            );
            assert_eq!(OPENS.with(|n| n.get()), 0, "a file was opened");
        }
    }

    #[test]
    fn scan_batches_fit_frame() {
        let child = ChildGuard::strings();
        let replies = scan(child.key(), &StringScanOpts::default());
        let batches = replies
            .iter()
            .filter(|(body, _)| matches!(body, HelperReplyBody::Strings(b) if !b.is_empty()))
            .count();
        assert!(batches > 1, "the scan fit one batch; add long strings");
        for (body, last) in replies {
            let reply = HelperReply {
                seq: u32::MAX,
                last,
                body,
            };
            let size = postcard::to_allocvec(&reply).expect("encode").len();
            assert!(size <= 65_536, "reply of {size} bytes");
        }
    }

    #[test]
    fn scan_limit_too_large() {
        let child = ChildGuard::strings();
        let replies = scan_limited(
            child.key(),
            &StringScanOpts::default(),
            &mut || false,
            10,
            u64::MAX,
        );
        assert_one_last(&replies);
        assert_eq!(
            replies.last(),
            Some(&(HelperReplyBody::Err(HelperError::TooLarge), true))
        );
        assert!(found(&replies).len() <= 10);
    }

    #[test]
    fn scan_kernel_thread_not_allowed() {
        // Pid 2 is kthreadd outside containers.
        if std::fs::read("/proc/2/comm").ok().as_deref() != Some(b"kthreadd\n".as_slice()) {
            return;
        }
        let replies = scan(key_of(2), &StringScanOpts::default());
        assert_eq!(
            replies,
            vec![(HelperReplyBody::Err(HelperError::NotAllowed), true)]
        );
    }

    #[test]
    fn cancel_scan_matches_seq() {
        let frame = |request| HelperFrame { seq: 9, request };
        let target = HelperTarget::Process(ProcessKey {
            start_ticks: 1,
            boot_id: BOOT,
            pid: 1,
        });
        assert!(is_cancel_for(
            &frame(HelperRequest::CancelScan { seq: 7 }),
            7
        ));
        assert!(!is_cancel_for(
            &frame(HelperRequest::CancelScan { seq: 8 }),
            7
        ));
        assert!(!is_cancel_for(
            &frame(HelperRequest::Signal {
                target,
                signal: Signal::Term,
            }),
            7
        ));
    }

    /// The body of the strings child. Does nothing in a normal test run.
    #[test]
    fn strings_child_entry() {
        if std::env::var_os(STRINGS_CHILD_ENV).is_none() {
            return;
        }
        let mut kept: Vec<Vec<u8>> = Vec::new();
        // Each string sits at offset 8 of its own zeroed heap buffer, so zeros end it on both
        // sides.
        let mut place = |bytes: &[u8]| {
            let mut buf = vec![0u8; bytes.len() + 16];
            buf[8..8 + bytes.len()].copy_from_slice(bytes);
            kept.push(buf);
        };
        place(ASCII_MARKER.as_bytes());
        let wide: Vec<u8> = UTF16_MARKER
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        place(&wide);
        for n in 0..MARKERS {
            place(format!("tl-mark-{n:02}-xyz").as_bytes());
        }
        for _ in 0..LONG_STRINGS {
            place(&[b'q'; 4000]);
        }
        std::hint::black_box(&kept);
        eprintln!("ready");
        loop {
            std::thread::park();
        }
    }
}
