//! Strings scan of process memory (PROC-084).
//!
//! The scanner walks the selected `maps` regions one [`CHUNK`] per `step` and finds printable
//! ASCII, UTF-8 and UTF-16LE strings. Each encoding has its own run state that lives across
//! chunks, so a string cut by a chunk boundary is reported once. Memory is bounded: one chunk
//! buffer from the caller and at most [`VALUE_MAX`] bytes of value per encoding. Strings found
//! in memory can hold secrets and are never logged.

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::ids::ProcessKey;

use crate::CollectError;
use crate::proc::detail::{MapRegionInfo, READ_FLAGS, open_beneath};
use crate::proc::pid_stat::read_start_ticks;
use crate::proc::{is_denied, is_gone};
use crate::sanitize::{ELLIPSIS, sanitize_text};
use crate::sysroot::SysRoot;

/// Bytes read per `step`.
pub const CHUNK: usize = 1024 * 1024;
/// The scan stops after this many strings.
pub const MAX_STRINGS: u32 = 1_000_000;
/// The scan stops after this many bytes of value text.
pub const MAX_TEXT_BYTES: u64 = 256 * 1024 * 1024;
/// Value bytes kept per string. A longer string is cut at a char boundary and ends with `…`.
pub const VALUE_MAX: usize = 4096;

const MIN_LEN_LOW: u8 = 4;
const MIN_LEN_HIGH: u8 = 64;

/// Scan options from the Strings tab options bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanOpts {
    /// Minimum string length in characters. The scanner clamps it to 4..=64.
    pub min_len: u8,
    pub ascii: bool,
    pub utf8: bool,
    pub utf16le: bool,
    pub include_file_backed: bool,
}

impl Default for ScanOpts {
    fn default() -> Self {
        Self {
            min_len: MIN_LEN_LOW,
            ascii: true,
            utf8: true,
            utf16le: true,
            include_file_backed: false,
        }
    }
}

/// Puts the `[start, end)` ranges to scan into `out` (cleared first). Keeps readable regions
/// that are private anonymous (no path or `[anon:<name>]`), `[heap]` or `[stack]`, and
/// file-backed regions when `include_file_backed`. `[vvar]`, `[vsyscall]`, `[vdso]` and paths
/// under `/dev/` are always skipped, as are empty regions.
pub fn select_regions(
    maps: &[MapRegionInfo],
    include_file_backed: bool,
    out: &mut Vec<(u64, u64)>,
) {
    out.clear();
    for region in maps {
        if region.perms[0] != b'r' || region.end <= region.start {
            continue;
        }
        let private = region.perms[3] == b'p';
        let keep = match region.path.as_deref() {
            None => private,
            Some("[heap]" | "[stack]") => true,
            Some(path) if path.starts_with("[anon:") => private,
            Some(path) if path.starts_with("/dev/") => false,
            Some(path) => include_file_backed && path.starts_with('/'),
        };
        if keep {
            out.push((region.start, region.end));
        }
    }
}

/// Reads target memory. Returns the bytes read into the front of `buf`. `Ok(0)` or an error
/// means the range cannot be read; the scanner skips it.
pub trait MemSource {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> Result<usize, CollectError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Ascii,
    Utf8,
    Utf16Le,
}

/// One string found by the scan.
#[derive(Clone, Debug, PartialEq)]
pub struct StringHitInfo {
    pub address: u64,
    pub encoding: Encoding,
    /// Length in characters of the whole string, also when `value` was cut.
    pub length: u32,
    /// Sanitized, at most [`VALUE_MAX`] bytes plus `…`.
    pub value: Box<str>,
}

/// How a scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanEnd {
    Finished,
    /// Set by the caller, which stops calling `step`. The scanner never returns it.
    Cancelled,
    /// A limit was reached ("String scan stopped: not enough memory.").
    Stopped,
    /// No region could be read ("Failed to read process memory.").
    Failed,
}

/// The run of one encoding that is being collected.
#[derive(Debug, Default)]
struct Run {
    start: u64,
    chars: u32,
    /// UTF-8 of the first chars, at most [`VALUE_MAX`] bytes.
    bytes: Vec<u8>,
    /// True when a char did not fit in `bytes`.
    cut: bool,
    non_ascii: bool,
}

impl Run {
    fn push(&mut self, addr: u64, c: char) {
        if self.chars == 0 {
            self.start = addr;
        }
        self.chars = self.chars.saturating_add(1);
        self.non_ascii |= !c.is_ascii();
        let mut tmp = [0u8; 4];
        let encoded = c.encode_utf8(&mut tmp).as_bytes();
        if !self.cut && self.bytes.len() + encoded.len() <= VALUE_MAX {
            self.bytes.extend_from_slice(encoded);
        } else {
            self.cut = true;
        }
    }

    fn reset(&mut self) {
        self.chars = 0;
        self.bytes.clear();
        self.cut = false;
        self.non_ascii = false;
    }
}

/// Turns finished runs into hits and counts them against the limits.
#[derive(Debug)]
struct Sink {
    min_len: u32,
    found: u32,
    text_bytes: u64,
    max_strings: u32,
    max_text: u64,
    stopped: bool,
    text: String,
}

impl Sink {
    /// Ends `run`, and reports it when it is long enough. A UTF-8 run of ASCII only is left to
    /// the ASCII encoding.
    fn end(&mut self, run: &mut Run, encoding: Encoding, out: &mut Vec<StringHitInfo>) {
        if run.chars >= self.min_len
            && !self.stopped
            && (encoding != Encoding::Utf8 || run.non_ascii)
        {
            sanitize_text(&run.bytes, VALUE_MAX, &mut self.text);
            if run.cut {
                self.text.push(ELLIPSIS);
            }
            // A run of spaces sanitizes to nothing.
            if !self.text.is_empty() {
                self.found += 1;
                self.text_bytes = self.text_bytes.saturating_add(self.text.len() as u64);
                out.push(StringHitInfo {
                    address: run.start,
                    encoding,
                    length: run.chars,
                    value: self.text.as_str().into(),
                });
                self.stopped = self.found >= self.max_strings || self.text_bytes >= self.max_text;
            }
        }
        run.reset();
    }
}

#[derive(Debug, Default)]
struct Utf8State {
    run: Run,
    /// Bytes of a multi-byte char seen so far.
    pend: [u8; 4],
    pend_len: u8,
    /// Total bytes of the pending char, 0 when none is pending.
    pend_need: u8,
    pend_addr: u64,
}

#[derive(Debug, Default)]
struct Utf16State {
    run: Run,
    /// The low byte read at the last even address.
    low: Option<u8>,
    /// The high byte every unit of the open run shares.
    high: u8,
}

/// Scans process memory for strings, one chunk per [`step`](Self::step). It holds no thread
/// and no fd, so a cancel is the caller not calling `step` again.
#[derive(Debug)]
pub struct StringScanner {
    opts: ScanOpts,
    regions: Vec<(u64, u64)>,
    region: usize,
    /// Bytes of the current region already handled.
    offset: u64,
    total: u64,
    scanned: u64,
    any_read: bool,
    ended: Option<ScanEnd>,
    sink: Sink,
    ascii: Run,
    utf8: Utf8State,
    utf16: Utf16State,
}

impl StringScanner {
    pub fn new(mut opts: ScanOpts, regions: Vec<(u64, u64)>) -> Self {
        opts.min_len = opts.min_len.clamp(MIN_LEN_LOW, MIN_LEN_HIGH);
        let total = regions.iter().fold(0u64, |sum, &(start, end)| {
            sum.saturating_add(end.saturating_sub(start))
        });
        Self {
            opts,
            regions,
            region: 0,
            offset: 0,
            total,
            scanned: 0,
            any_read: false,
            ended: None,
            sink: Sink {
                min_len: u32::from(opts.min_len),
                found: 0,
                text_bytes: 0,
                max_strings: MAX_STRINGS,
                max_text: MAX_TEXT_BYTES,
                stopped: false,
                text: String::new(),
            },
            ascii: Run::default(),
            utf8: Utf8State::default(),
            utf16: Utf16State::default(),
        }
    }

    /// Lowers the stop limits, for tests.
    #[doc(hidden)]
    pub fn with_limits(mut self, max_strings: u32, max_text: u64) -> Self {
        self.sink.max_strings = max_strings;
        self.sink.max_text = max_text;
        self
    }

    /// Strings found so far.
    pub fn found(&self) -> u32 {
        self.sink.found
    }

    /// Bytes scanned or skipped, as a percentage of all region bytes.
    pub fn progress_pct(&self) -> u8 {
        if self.total == 0 {
            return if self.ended.is_some() { 100 } else { 0 };
        }
        let pct = u128::from(self.scanned.min(self.total)) * 100 / u128::from(self.total);
        pct as u8
    }

    /// Reads one chunk of at most [`CHUNK`] bytes into `buf` and appends the strings it ends to
    /// `out`. Returns `None` while there is more to scan, else how the scan ended. A range that
    /// `mem` cannot read is skipped.
    pub fn step(
        &mut self,
        mem: &mut dyn MemSource,
        buf: &mut Vec<u8>,
        out: &mut Vec<StringHitInfo>,
    ) -> Option<ScanEnd> {
        if let Some(end) = self.ended {
            return Some(end);
        }
        if !(self.opts.ascii || self.opts.utf8 || self.opts.utf16le) {
            return self.finish(ScanEnd::Finished);
        }
        // Skip empty regions without a read.
        while let Some(&(start, end)) = self.regions.get(self.region) {
            if start.saturating_add(self.offset) < end {
                break;
            }
            self.next_region(out);
        }
        let Some(&(start, end)) = self.regions.get(self.region) else {
            return self.finish(self.final_end());
        };
        let addr = start + self.offset;
        let len = usize::try_from(end - addr).map_or(CHUNK, |rest| rest.min(CHUNK));
        buf.clear();
        buf.resize(len, 0);
        let n = match mem.read(addr, &mut buf[..len]) {
            Ok(n) => n.min(len),
            Err(_) => 0,
        };
        if n == 0 {
            // A hole: runs do not continue across it.
            self.flush(out);
            self.offset += len as u64;
            self.scanned = self.scanned.saturating_add(len as u64);
        } else {
            self.any_read = true;
            self.feed(addr, &buf[..n], out);
            self.offset += n as u64;
            self.scanned = self.scanned.saturating_add(n as u64);
        }
        if self.sink.stopped {
            return self.finish(ScanEnd::Stopped);
        }
        if start + self.offset >= end {
            self.next_region(out);
            if self.sink.stopped {
                return self.finish(ScanEnd::Stopped);
            }
            if self.region >= self.regions.len() {
                return self.finish(self.final_end());
            }
        }
        None
    }

    fn final_end(&self) -> ScanEnd {
        if self.total > 0 && !self.any_read {
            ScanEnd::Failed
        } else {
            ScanEnd::Finished
        }
    }

    fn finish(&mut self, end: ScanEnd) -> Option<ScanEnd> {
        self.ended = Some(end);
        Some(end)
    }

    fn next_region(&mut self, out: &mut Vec<StringHitInfo>) {
        self.flush(out);
        self.region += 1;
        self.offset = 0;
    }

    /// Ends every open run, at a region end or an unreadable range.
    fn flush(&mut self, out: &mut Vec<StringHitInfo>) {
        self.sink.end(&mut self.ascii, Encoding::Ascii, out);
        self.utf8.pend_need = 0;
        self.utf8.pend_len = 0;
        self.sink.end(&mut self.utf8.run, Encoding::Utf8, out);
        self.utf16.low = None;
        self.sink.end(&mut self.utf16.run, Encoding::Utf16Le, out);
    }

    fn idle(&self) -> bool {
        self.ascii.chars == 0
            && self.utf8.run.chars == 0
            && self.utf8.pend_need == 0
            && self.utf16.run.chars == 0
    }

    fn feed(&mut self, base: u64, data: &[u8], out: &mut Vec<StringHitInfo>) {
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0 && self.idle() {
                // Fast path: zero bytes start nothing. Most memory is zeros.
                let skip = data[i..]
                    .iter()
                    .position(|&b| b != 0)
                    .unwrap_or(data.len() - i);
                i += skip;
                if i == data.len() {
                    self.utf16.low = Some(0);
                    break;
                }
                // The skipped zero before an odd address is the low byte of a UTF-16 unit.
                self.utf16.low = Some(0);
            }
            let b = data[i];
            let addr = base + i as u64;
            if self.opts.ascii {
                self.ascii_byte(addr, b, out);
            }
            if self.opts.utf8 {
                self.utf8_byte(addr, b, out);
            }
            if self.opts.utf16le {
                self.utf16_byte(addr, b, out);
            }
            if self.sink.stopped {
                return;
            }
            i += 1;
        }
    }

    fn ascii_byte(&mut self, addr: u64, b: u8, out: &mut Vec<StringHitInfo>) {
        if is_ascii_text(b) {
            self.ascii.push(addr, char::from(b));
        } else if self.ascii.chars > 0 {
            self.sink.end(&mut self.ascii, Encoding::Ascii, out);
        }
    }

    fn utf8_byte(&mut self, addr: u64, b: u8, out: &mut Vec<StringHitInfo>) {
        let st = &mut self.utf8;
        if st.pend_need > 0 {
            if b & 0xC0 == 0x80 {
                st.pend[usize::from(st.pend_len)] = b;
                st.pend_len += 1;
                if st.pend_len == st.pend_need {
                    let len = usize::from(st.pend_len);
                    st.pend_need = 0;
                    st.pend_len = 0;
                    let c = std::str::from_utf8(&st.pend[..len])
                        .ok()
                        .and_then(|s| s.chars().next());
                    match c {
                        Some(c) if is_printable(c) => st.run.push(st.pend_addr, c),
                        _ => self.sink.end(&mut st.run, Encoding::Utf8, out),
                    }
                }
                return;
            }
            // A broken sequence ends the run. `b` may start a new one.
            st.pend_need = 0;
            st.pend_len = 0;
            self.sink.end(&mut st.run, Encoding::Utf8, out);
        }
        let need = match b {
            0x20..=0x7E => {
                st.run.push(addr, char::from(b));
                return;
            }
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => {
                if st.run.chars > 0 {
                    self.sink.end(&mut st.run, Encoding::Utf8, out);
                }
                return;
            }
        };
        st.pend[0] = b;
        st.pend_len = 1;
        st.pend_need = need;
        st.pend_addr = addr;
    }

    fn utf16_byte(&mut self, addr: u64, b: u8, out: &mut Vec<StringHitInfo>) {
        let st = &mut self.utf16;
        if addr & 1 == 0 {
            st.low = Some(b);
            return;
        }
        let Some(low) = st.low.take() else {
            if st.run.chars > 0 {
                self.sink.end(&mut st.run, Encoding::Utf16Le, out);
            }
            return;
        };
        match utf16_char(low, b) {
            Some(c) => {
                if st.run.chars > 0 && st.high != 0 && b == 0 && is_neutral(low) {
                    // A space, digit or punctuation inside another alphabet's text.
                    st.run.push(addr - 1, c);
                    return;
                }
                if st.run.chars > 0 && st.high != b {
                    // Another alphabet: end this run and start a new one with this unit.
                    self.sink.end(&mut st.run, Encoding::Utf16Le, out);
                }
                st.high = b;
                st.run.push(addr - 1, c);
            }
            None => {
                if st.run.chars > 0 {
                    self.sink.end(&mut st.run, Encoding::Utf16Le, out);
                }
            }
        }
    }
}

/// The low byte of a high-byte-0 UTF-16 unit that may sit inside another alphabet's text: a
/// space, an ASCII digit or ASCII punctuation.
fn is_neutral(low: u8) -> bool {
    low == b' ' || low.is_ascii_digit() || low.is_ascii_punctuation()
}

/// ASCII string bytes: 0x20..=0x7E and tab.
fn is_ascii_text(b: u8) -> bool {
    matches!(b, 0x20..=0x7E | b'\t')
}

fn is_printable(c: char) -> bool {
    !c.is_control() && c != char::REPLACEMENT_CHARACTER
}

/// The char of one UTF-16LE unit, when it counts as string text. Units are read at even
/// addresses only. Surrogates, controls, U+FFFD, U+FFFE and U+FFFF do not count. Nor does a
/// unit whose two bytes are both printable ASCII or tab: that is two ASCII chars, and taking it
/// as one wide char would turn every ASCII string into UTF-16 noise.
///
/// A run also keeps one high byte for its whole length: a unit with another high byte than the
/// first unit ends the run and starts a new one (see `utf16_byte`). Text in one alphabet keeps
/// one high byte (0 is Latin-1; Greek, Cyrillic, Hebrew and Arabic each have their own), but
/// in random data each extra unit matches with odds of about 1 in 256. One exception: inside a
/// run whose high byte is not 0, a space, ASCII digit or ASCII punctuation unit (high byte 0)
/// stays in the run and does not change its high byte, so "Привет мир 2024" is one run. Such a
/// run still starts with a unit of its own alphabet, and an ASCII letter still ends it.
///
/// Known cost: CJK and Hangul text changes high byte almost every char, so it is mostly not
/// found.
fn utf16_char(low: u8, high: u8) -> Option<char> {
    if is_ascii_text(low) && is_ascii_text(high) {
        return None;
    }
    let unit = u32::from(low) | (u32::from(high) << 8);
    if unit == 0xFFFE || unit == 0xFFFF {
        return None;
    }
    char::from_u32(unit).filter(|&c| is_printable(c))
}

/// Memory in a `Vec` at a base address, for tests.
#[derive(Clone, Debug)]
pub struct VecMem(pub u64, pub Vec<u8>);

impl MemSource for VecMem {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> Result<usize, CollectError> {
        let Some(at) = addr.checked_sub(self.0) else {
            return Ok(0);
        };
        let Some(rest) = usize::try_from(at).ok().and_then(|at| self.1.get(at..)) else {
            return Ok(0);
        };
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }
}

/// `/proc/<pid>/mem` of one process. Opening it needs the same ptrace access as
/// `process_vm_readv` (PTRACE_MODE_ATTACH), and the kernel checks it at open.
#[derive(Debug)]
pub struct ProcMem {
    fd: OwnedFd,
}

impl ProcMem {
    /// Opens `mem` of `key` after checking the start time, so a reused pid never reads another
    /// process. EACCES or EPERM is `PermissionDenied` (Restricted). A process that exited or
    /// whose pid was reused is `ProcessGone`.
    pub fn open(root: &SysRoot, key: ProcessKey) -> Result<Self, CollectError> {
        let gone = CollectError::ProcessGone { pid: key.pid };
        let map_err = |e: Errno| {
            if is_gone(e) {
                CollectError::ProcessGone { pid: key.pid }
            } else if is_denied(e) {
                CollectError::PermissionDenied
            } else {
                CollectError::Os(e)
            }
        };
        let dir = rustix::fs::open(
            root.proc.join(key.pid.to_string()),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_err)?;
        match read_start_ticks(dir.as_fd(), &mut Vec::new()) {
            Ok(ticks) if ticks == key.start_ticks => {}
            Ok(_) | Err(CollectError::NotFound) => return Err(gone),
            Err(e) => return Err(e),
        }
        let fd = open_beneath(dir.as_fd(), &["mem"], READ_FLAGS).map_err(map_err)?;
        Ok(Self { fd })
    }
}

impl MemSource for ProcMem {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> Result<usize, CollectError> {
        loop {
            match rustix::io::pread(&self.fd, &mut *buf, addr) {
                Ok(n) => return Ok(n),
                Err(Errno::INTR) => {}
                // An unmapped or unreadable page in the range.
                Err(Errno::IO) => return Ok(0),
                Err(e) => return Err(e.into()),
            }
        }
    }
}
