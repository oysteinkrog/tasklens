//! X11 resources column (PROC-035, pdu.88): the XRes resource count of each X11 client, summed
//! per process.
//!
//! The processes sweep calls [`X11Res::counts`] only while the column is shown, and only for the
//! visible rows. It opens the session's `DISPLAY` once and keeps the connection while the column
//! stays shown. One sweep sends `XResQueryClientIds` with the local PID mask, then one
//! `XResQueryClientResources` per client of a visible PID, pipelined.
//!
//! The sweep thread must never hang on the display, so every wait has a deadline
//! ([`SWEEP_DEADLINE`] per sweep, through [`TimedStream`]). Only local sockets are used, and the
//! socket connect does not block. Any error drops the connection, and the next try waits
//! [`RETRY_AFTER`].
//!
//! The cookie comes from the session's `XAUTHORITY` file (or `~/.Xauthority`), read here rather
//! than through x11rb, which reads only this process's environment: under the engine service the
//! session values come from the user manager (`DesktopSession::var`). The cookie is a secret. It
//! is never logged.

use std::cell::Cell;
use std::io::{self, IoSlice};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use rustc_hash::{FxHashMap, FxHashSet};
use x11rb::connection::Connection as _;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb::utils::RawFdContainer;

use crate::desktop::DesktopSession;

/// All X11 waits of one sweep end by this time after the sweep starts.
pub const SWEEP_DEADLINE: Duration = Duration::from_millis(200);
/// After an error, the next connect waits this long.
pub const RETRY_AFTER: Duration = Duration::from_secs(10);
/// The only auth method read from the Xauthority file.
const MIT_COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";
/// Xauthority family values: `FamilyLocal` and `FamilyWild`.
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;
/// A larger Xauthority file is not read.
const XAUTH_MAX: u64 = 64 * 1024;

/// The cookie for `display` from Xauthority file bytes: the first `MIT-MAGIC-COOKIE-1` entry with
/// that display number and a local or wild family. `None` when there is none or the bytes are
/// cut short before it.
pub fn parse_xauthority(bytes: &[u8], display: u16) -> Option<Vec<u8>> {
    fn u16_at(bytes: &[u8], at: &mut usize) -> Option<u16> {
        let b = bytes.get(*at..*at + 2)?;
        *at += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn field<'a>(bytes: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
        let len = usize::from(u16_at(bytes, at)?);
        let f = bytes.get(*at..*at + len)?;
        *at += len;
        Some(f)
    }
    let number = display.to_string();
    let mut at = 0;
    while at < bytes.len() {
        let family = u16_at(bytes, &mut at)?;
        let _address = field(bytes, &mut at)?;
        let num = field(bytes, &mut at)?;
        let name = field(bytes, &mut at)?;
        let data = field(bytes, &mut at)?;
        if matches!(family, FAMILY_LOCAL | FAMILY_WILD)
            && num == number.as_bytes()
            && name == MIT_COOKIE
        {
            return Some(data.to_vec());
        }
    }
    None
}

/// A [`DefaultStream`] whose waits end at a deadline with `io::ErrorKind::TimedOut`.
pub struct TimedStream {
    inner: DefaultStream,
    deadline: Cell<Instant>,
}

impl TimedStream {
    /// The deadline starts now, so set it before the first wait.
    pub fn new(inner: DefaultStream) -> Self {
        Self {
            inner,
            deadline: Cell::new(Instant::now()),
        }
    }

    /// Every later wait ends at `deadline`.
    pub fn set_deadline(&self, deadline: Instant) {
        self.deadline.set(deadline);
    }
}

impl Stream for TimedStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        use rustix::io::Errno;

        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        loop {
            let left = self
                .deadline
                .get()
                .saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "X11 reply deadline passed",
                ));
            }
            let timeout = Timespec::try_from(left).map_err(|_| io::ErrorKind::InvalidInput)?;
            let mut fds = [PollFd::new(&self.inner, flags)];
            match poll(&mut fds, Some(&timeout)) {
                Ok(0) | Err(Errno::INTR) => {}
                Ok(_) => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn read(&self, buf: &mut [u8], fd_storage: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(buf, fd_storage)
    }

    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(buf, fds)
    }

    fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(bufs, fds)
    }
}

/// A non-blocking connect to a local X socket: the abstract name first, then the path. A full
/// listen queue fails at once (`EAGAIN`) instead of waiting.
fn connect_local(path: &str) -> io::Result<UnixStream> {
    use rustix::net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
    };

    let try_addr = |addr: &SocketAddrUnix| -> io::Result<UnixStream> {
        let fd = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )?;
        connect(&fd, addr)?;
        Ok(UnixStream::from(fd))
    };
    SocketAddrUnix::new_abstract_name(path.as_bytes())
        .map_err(io::Error::from)
        .and_then(|a| try_addr(&a))
        .or_else(|_| try_addr(&SocketAddrUnix::new(path)?))
}

struct Conn {
    /// The `DISPLAY` value this connection is for.
    display: String,
    conn: RustConnection<TimedStream>,
}

/// The XRes connection of the processes sweep.
#[derive(Default)]
pub struct X11Res {
    conn: Option<Conn>,
    /// No connect before this time, after an error.
    retry_at: Option<Instant>,
}

impl X11Res {
    /// Drops the connection, for when the column is hidden.
    pub fn disconnect(&mut self) {
        self.conn = None;
    }

    /// The summed XRes resource count of each pid in `pids` that owns an X11 client of the
    /// session's display. Empty when there is no display, while waiting after an error, or on
    /// an error, which drops the connection.
    pub fn counts(
        &mut self,
        desktop: &DesktopSession,
        pids: &FxHashSet<u32>,
    ) -> FxHashMap<u32, u32> {
        let deadline = Instant::now() + SWEEP_DEADLINE;
        let Some(display) = desktop.var("DISPLAY") else {
            self.conn = None;
            return FxHashMap::default();
        };
        if self.conn.as_ref().is_some_and(|c| c.display != display) {
            self.conn = None;
        }
        if self.conn.is_none() {
            if self.retry_at.is_some_and(|t| Instant::now() < t) {
                return FxHashMap::default();
            }
            match connect(desktop, &display, deadline) {
                Ok(conn) => self.conn = Some(Conn { display, conn }),
                Err(e) => {
                    tracing::debug!(error = %e, "X11 resources: no connection");
                    self.retry_at = Some(Instant::now() + RETRY_AFTER);
                    return FxHashMap::default();
                }
            }
        }
        let Some(conn) = &self.conn else {
            return FxHashMap::default();
        };
        conn.conn.stream().set_deadline(deadline);
        match query(&conn.conn, pids) {
            Ok(counts) => counts,
            Err(e) => {
                tracing::debug!(error = %e, "X11 resources: query failed");
                self.conn = None;
                self.retry_at = Some(Instant::now() + RETRY_AFTER);
                FxHashMap::default()
            }
        }
    }
}

type Error = Box<dyn std::error::Error>;

/// Opens `display` (local sockets only) with the session's cookie, and checks for XRes 1.2.
fn connect(
    desktop: &DesktopSession,
    display: &str,
    deadline: Instant,
) -> Result<RustConnection<TimedStream>, Error> {
    let parsed = parse_display(Some(display))?;
    let unix = parsed
        .connect_instruction()
        .find_map(|a| match a {
            ConnectAddress::Socket(path) => connect_local(&path).ok(),
            _ => None,
        })
        .ok_or("no local X11 socket")?;
    let (stream, _peer) = DefaultStream::from_unix_stream(unix)?;
    let stream = TimedStream::new(stream);
    stream.set_deadline(deadline);
    let cookie = xauthority_path(desktop)
        .and_then(|path| read_capped(&path))
        .and_then(|bytes| parse_xauthority(&bytes, parsed.display));
    let (name, data) = match cookie {
        Some(data) => (MIT_COOKIE.to_vec(), data),
        None => (Vec::new(), Vec::new()),
    };
    let conn = RustConnection::connect_to_stream_with_auth_info(
        stream,
        usize::from(parsed.screen),
        name,
        data,
    )?;
    let version = conn.res_query_version(1, 2)?.reply()?;
    if (version.server_major, version.server_minor) < (1, 2) {
        return Err("XRes 1.2 not supported".into());
    }
    Ok(conn)
}

/// The session's `XAUTHORITY`, else `$HOME/.Xauthority`.
fn xauthority_path(desktop: &DesktopSession) -> Option<std::path::PathBuf> {
    desktop.var("XAUTHORITY").map(Into::into).or_else(|| {
        std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".Xauthority"))
    })
}

/// The Xauthority file's bytes, or `None` unless it is a regular file of at most [`XAUTH_MAX`]
/// bytes. The open does not block: `O_NONBLOCK` makes a FIFO open return at once (there is no
/// writer to wait for), and the type check then refuses it, so a FIFO, socket or device at
/// `XAUTHORITY` can never hang the sweep. Reading a regular file of this size does not wait.
fn read_capped(path: &std::path::Path) -> Option<Vec<u8>> {
    use rustix::fs::{FileType, Mode, OFlags};
    use std::io::Read as _;

    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let stat = rustix::fs::fstat(&fd).ok()?;
    let size = u64::try_from(stat.st_size).ok()?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || size > XAUTH_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    // `take` keeps the cap if the file grows after the fstat.
    std::fs::File::from(fd)
        .take(XAUTH_MAX)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// One `XResQueryClientIds`, then the resources of each client whose pid is in `pids`.
fn query(
    conn: &RustConnection<TimedStream>,
    pids: &FxHashSet<u32>,
) -> Result<FxHashMap<u32, u32>, Error> {
    let spec = ClientIdSpec {
        client: 0,
        mask: ClientIdMask::LOCAL_CLIENT_PID,
    };
    let ids = conn.res_query_client_ids(&[spec])?.reply()?;
    let mut cookies = Vec::new();
    for id in &ids.ids {
        if !id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID) {
            continue;
        }
        let Some(&pid) = id.value.first() else {
            continue;
        };
        if pids.contains(&pid) {
            cookies.push((pid, conn.res_query_client_resources(id.spec.client)?));
        }
    }
    conn.flush()?;
    let mut counts = FxHashMap::default();
    for (pid, cookie) in cookies {
        let total = cookie
            .reply()?
            .types
            .iter()
            .fold(0u32, |sum, t| sum.saturating_add(t.count));
        let entry: &mut u32 = counts.entry(pid).or_default();
        *entry = entry.saturating_add(total);
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(family: u16, number: &str, name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = family.to_be_bytes().to_vec();
        for f in [b"host".as_slice(), number.as_bytes(), name, data] {
            out.extend_from_slice(&u16::try_from(f.len()).unwrap().to_be_bytes());
            out.extend_from_slice(f);
        }
        out
    }

    #[test]
    fn xauthority_picks_the_display_and_cookie() {
        let mut file = entry(FAMILY_LOCAL, "0", MIT_COOKIE, b"zero");
        file.extend(entry(FAMILY_LOCAL, "4", b"XDM-AUTHORIZATION-1", b"other"));
        file.extend(entry(FAMILY_LOCAL, "4", MIT_COOKIE, b"four"));
        file.extend(entry(FAMILY_LOCAL, "4", MIT_COOKIE, b"later"));
        assert_eq!(parse_xauthority(&file, 0).as_deref(), Some(&b"zero"[..]));
        assert_eq!(parse_xauthority(&file, 4).as_deref(), Some(&b"four"[..]));
        assert_eq!(parse_xauthority(&file, 7), None);
    }

    #[test]
    fn xauthority_takes_wild_and_skips_other_families() {
        let mut file = entry(0, "1", MIT_COOKIE, b"inet");
        file.extend(entry(FAMILY_WILD, "1", MIT_COOKIE, b"wild"));
        assert_eq!(parse_xauthority(&file, 1).as_deref(), Some(&b"wild"[..]));
    }

    #[test]
    fn xauthority_truncated_is_none() {
        let file = entry(FAMILY_LOCAL, "0", MIT_COOKIE, b"cookie");
        for cut in 0..file.len() {
            assert_eq!(parse_xauthority(&file[..cut], 0), None, "cut at {cut}");
        }
        assert_eq!(parse_xauthority(&[], 0), None);
    }

    #[test]
    fn timed_stream_ends_at_the_deadline() {
        let (a, _b) = UnixStream::pair().unwrap();
        let (stream, _) = DefaultStream::from_unix_stream(a).unwrap();
        let stream = TimedStream::new(stream);
        let start = Instant::now();
        stream.set_deadline(start + SWEEP_DEADLINE);
        let err = stream.poll(PollMode::Readable).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let took = start.elapsed();
        assert!(took >= SWEEP_DEADLINE, "{took:?}");
        assert!(
            took < SWEEP_DEADLINE + Duration::from_secs(2),
            "{took:?}"
        );
        // A readable stream returns at once.
        let (a, b) = UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut &b, b"x").unwrap();
        let (stream, _) = DefaultStream::from_unix_stream(a).unwrap();
        let stream = TimedStream::new(stream);
        stream.set_deadline(Instant::now() + SWEEP_DEADLINE);
        stream.poll(PollMode::Readable).unwrap();
    }

    /// A temp dir under `target/tmp`, removed on drop.
    fn scratch() -> tempfile::TempDir {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("x11res-")
            .tempdir_in(base)
            .expect("tempdir")
    }

    /// `read_capped` on another thread, so a regression fails the test instead of hanging it.
    fn read_within_1s(path: std::path::PathBuf) -> Option<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_capped(&path));
        });
        rx.recv_timeout(Duration::from_secs(1))
            .expect("read_capped blocked")
    }

    #[test]
    fn xauthority_fifo_is_refused_without_blocking() {
        let dir = scratch();
        let fifo = dir.path().join("Xauthority");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .expect("mkfifo");
        assert_eq!(read_within_1s(fifo), None);
    }

    #[test]
    fn xauthority_reads_only_small_regular_files() {
        let dir = scratch();
        let file = dir.path().join("Xauthority");
        let bytes = entry(FAMILY_LOCAL, "0", MIT_COOKIE, b"cookie");
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(read_within_1s(file.clone()), Some(bytes));
        // Too large: refused, not cut short.
        let size = usize::try_from(XAUTH_MAX).unwrap() + 1;
        std::fs::write(&file, vec![0u8; size]).unwrap();
        assert_eq!(read_within_1s(file), None);
        // A directory and a missing path.
        assert_eq!(read_within_1s(dir.path().to_path_buf()), None);
        assert_eq!(read_within_1s(dir.path().join("missing")), None);
    }

    #[test]
    fn no_display_gives_nothing() {
        let desktop = DesktopSession::new(crate::desktop::DesktopSource::None);
        let mut x11 = X11Res::default();
        assert!(x11.counts(&desktop, &FxHashSet::default()).is_empty());
        assert!(x11.conn.is_none());
    }

    /// Needs an X display with an X client (run it in the nested session, see pdu.88). Counts
    /// the resources of every client of the display and expects at least one non-zero total.
    #[test]
    #[ignore = "needs DISPLAY"]
    fn x11res_live() {
        let desktop = DesktopSession::new(crate::desktop::DesktopSource::Process);
        let pids: FxHashSet<u32> = std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        let mut x11 = X11Res::default();
        let counts = x11.counts(&desktop, &pids);
        assert!(x11.conn.is_some(), "no connection to DISPLAY");
        assert!(counts.values().any(|&n| n > 0), "{counts:?}");
        // The connection is kept and answers again.
        let again = x11.counts(&desktop, &pids);
        assert!(!again.is_empty());
    }
}
