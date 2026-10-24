//! Device metric history (ADR-017, D-030): three tiers per series, each in its own segment files.
//!
//! Series are keyed by `(device key, MetricId)`, never by `SeriesId`. The device key is the
//! device's `persistent_key`, so a series continues across restarts, reboots and hotplug.
//!
//! | Tier | Point | Kept | Values per point | Segment |
//! |---|---|---|---|---|
//! | 1 s | 1 s | 1 h | the per-second maximum | 10 min |
//! | 10 s | 10 s | 24 h | mean, min and max of the ten 1 s points | 1 h |
//! | 1 min | 60 s | 7 d | mean of the six 10 s means weighted by valid count, min, max | 24 h |
//!
//! The 10 s and 1 min points also store how many valid 1 s values they hold. A read weights each
//! point by that count, so a minute split by a restart, half in each session's segment, reads as
//! the mean of all its seconds.
//!
//! Layout: `root/1s/`, `root/10s/` and `root/1m/`, each with `seg-<CLOCK_REALTIME start seconds,
//! 10 digits>.tlm` files. Directories have mode 0700, files 0600. A segment is a 64-byte header
//! and one frame per 60 s block, `u32 LE length | u32 LE crc32 | zstd frame` of a postcard
//! `MetricBlock`. There is no footer: a reader keeps every block before the first bad frame. A
//! new segment starts at every engine start, so one segment never spans two boots.
//!
//! RAM holds the last hour of the 1 s tier (one ring per series, by realtime second) and the
//! minutes not yet written. The 10 s and 1 min tiers are read from disk on request into a span
//! cache of at most `MAX_POINTS` buckets per series, which `tick` drops 60 s after its last
//! request.
//!
//! Every call that touches files does blocking I/O. The engine calls the store only on the
//! blocking pool.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tl_model::ids::MetricId;
use tl_model::series::{Sample, SampleFlags};
use tl_model::time::{BootId, TimeNs};

use crate::block::{FRAME_PREFIX_LEN, MAX_BLOCK_RAW};
use crate::error::HistoryError;
use crate::format::{HEADER_LEN, ZSTD_LEVEL};
use crate::read::MAX_POINTS;

/// First 4 bytes of every metric segment file.
pub const METRIC_MAGIC: [u8; 4] = *b"TLM\x01";
pub const METRIC_FORMAT_VERSION: u16 = 1;
/// Length of one block: 60 s.
pub const METRIC_BLOCK_NS: u64 = 60 * NS_PER_SEC;
/// Points in the 1 s ring of each series: 1 h.
pub const RING_SECS: usize = 3600;
/// A minute is written once `tick` is this far past its end, so samples that arrive a little
/// late still land in it.
pub const FLUSH_GRACE_NS: u64 = 2 * NS_PER_SEC;
/// `tick` drops a span that nobody requested for this long.
pub const SPAN_IDLE_NS: u64 = 60 * NS_PER_SEC;
/// Most spans cached at once; the least recently used goes first.
pub const MAX_SPANS: usize = 4;
/// `history.device_metrics_max_mb = 64` in bytes.
pub const DEFAULT_MAX_DISK_BYTES: u64 = 64 << 20;

const NS_PER_SEC: u64 = 1_000_000_000;
/// Name collisions tried before a new segment fails (two starts in one second).
const NAME_TRIES: i64 = 16;

/// One of the three stored resolutions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    S1,
    S10,
    M1,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::S1, Tier::S10, Tier::M1];

    /// The header byte: 0 = 1 s, 1 = 10 s, 2 = 1 min.
    pub const fn code(self) -> u8 {
        match self {
            Tier::S1 => 0,
            Tier::S10 => 1,
            Tier::M1 => 2,
        }
    }

    pub const fn from_code(code: u8) -> Option<Tier> {
        match code {
            0 => Some(Tier::S1),
            1 => Some(Tier::S10),
            2 => Some(Tier::M1),
            _ => None,
        }
    }

    pub const fn point_ns(self) -> u64 {
        match self {
            Tier::S1 => NS_PER_SEC,
            Tier::S10 => 10 * NS_PER_SEC,
            Tier::M1 => 60 * NS_PER_SEC,
        }
    }

    /// 60, 6 or 1.
    pub const fn points_per_block(self) -> usize {
        (METRIC_BLOCK_NS / self.point_ns()) as usize
    }

    /// 10 min, 1 h or 24 h.
    pub const fn segment_ns(self) -> u64 {
        match self {
            Tier::S1 => 600 * NS_PER_SEC,
            Tier::S10 => 3600 * NS_PER_SEC,
            Tier::M1 => 86_400 * NS_PER_SEC,
        }
    }

    /// 1 h, 24 h or 7 d.
    pub const fn retention_ns(self) -> u64 {
        match self {
            Tier::S1 => 3600 * NS_PER_SEC,
            Tier::S10 => 86_400 * NS_PER_SEC,
            Tier::M1 => 7 * 86_400 * NS_PER_SEC,
        }
    }

    pub const fn dir_name(self) -> &'static str {
        match self {
            Tier::S1 => "1s",
            Tier::S10 => "10s",
            Tier::M1 => "1m",
        }
    }

    /// The 10 s and 1 min tiers store min and max beside the mean.
    pub const fn has_min_max(self) -> bool {
        !matches!(self, Tier::S1)
    }

    const fn index(self) -> usize {
        self.code() as usize
    }
}

/// The fixed 64-byte header of a metric segment, little endian.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricHeader {
    pub tier: Tier,
    pub boot_id: BootId,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME at segment start.
    pub realtime_offset_ns: i64,
    /// Start of the first block, CLOCK_BOOTTIME.
    pub start_t_ns: u64,
}

impl MetricHeader {
    /// Bytes 0..4 magic, 4..6 version, 6 tier, 8..16 boot id, 16..24 realtime offset, 24..32
    /// start, every other byte zero.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&METRIC_MAGIC);
        b[4..6].copy_from_slice(&METRIC_FORMAT_VERSION.to_le_bytes());
        b[6] = self.tier.code();
        b[8..16].copy_from_slice(&self.boot_id.0.to_le_bytes());
        b[16..24].copy_from_slice(&self.realtime_offset_ns.to_le_bytes());
        b[24..32].copy_from_slice(&self.start_t_ns.to_le_bytes());
        b
    }

    /// Wrong length, magic or tier byte gives `BadHeader`, a version other than 1 gives
    /// `Version(v)`. Reserved bytes are ignored.
    pub fn decode(b: &[u8]) -> Result<MetricHeader, HistoryError> {
        if b.len() != HEADER_LEN || b[0..4] != METRIC_MAGIC {
            return Err(HistoryError::BadHeader);
        }
        let version = u16::from_le_bytes([b[4], b[5]]);
        if version != METRIC_FORMAT_VERSION {
            return Err(HistoryError::Version(version));
        }
        let tier = Tier::from_code(b[6]).ok_or(HistoryError::BadHeader)?;
        let u64_at = |i: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[i..i + 8]);
            a
        };
        Ok(MetricHeader {
            tier,
            boot_id: BootId(u64::from_le_bytes(u64_at(8))),
            realtime_offset_ns: i64::from_le_bytes(u64_at(16)),
            start_t_ns: u64::from_le_bytes(u64_at(24)),
        })
    }
}

/// `seg-<CLOCK_REALTIME start seconds, 10 digits>.tlm`.
pub fn metric_segment_file_name(start_realtime_s: i64) -> String {
    format!("seg-{:010}.tlm", start_realtime_s.max(0))
}

/// One series of a segment catalog. Blocks refer to it by `slot`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistSeries {
    pub slot: u16,
    pub device_key: Arc<str>,
    pub device_name: Arc<str>,
    pub metric: MetricId,
}

/// The points of one series in one block: 60, 6 or 1 by tier. An invalid point stores 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesColumn {
    pub slot: u16,
    /// Bit `i % 8` of byte `i / 8` is set when point `i` is valid.
    pub valid: Vec<u8>,
    /// The value in the 1 s tier, the mean in the others.
    pub mean: Vec<f32>,
    /// 10 s and 1 min tiers only.
    pub min: Option<Vec<f32>>,
    /// 10 s and 1 min tiers only.
    pub max: Option<Vec<f32>>,
    /// Valid 1 s values in each point: 1 to 10 or 1 to 60, 0 for an invalid point. 10 s and
    /// 1 min tiers only.
    pub count: Option<Vec<u8>>,
}

/// One 60 s block. Point `i` of a column is at `start_t_ns + i * point`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricBlock {
    pub start_t_ns: u64,
    /// First block after a suspend or an engine start.
    pub gap_before: bool,
    /// Every series of this session. In the first block of a segment and after a change.
    pub catalog: Option<Vec<HistSeries>>,
    /// Only series with at least one valid point.
    pub columns: Vec<SeriesColumn>,
    /// 1 min tier only.
    pub events: Vec<HistEvent>,
}

/// Kinds of timeline events (UI-PRF-057).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    MemoryPressure,
    SwapStorm,
    Heat,
    FullLoad,
    Suspend,
    Boot,
    OomKill,
    Crash,
}

/// One stored event. `start_t_ns` is CLOCK_BOOTTIME of the boot whose segment holds it. It is
/// stored when it ends, in a block that ends after its start, so it starts between its boot
/// and the end of its segment. Process names are `comm` only (ADR-015: no command lines).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistEvent {
    pub kind: EventKind,
    pub start_t_ns: u64,
    pub duration_ms: u32,
    pub peak: f32,
    pub device_key: Option<Arc<str>>,
    pub process: Option<Arc<str>>,
}

/// An event placed on CLOCK_REALTIME.
#[derive(Clone, Debug, PartialEq)]
pub struct RealtimeEvent {
    pub start_realtime_ns: i64,
    pub event: HistEvent,
}

/// What a stored series is keyed by. `metric` never carries `MetricId::TIER_1HZ`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeriesKey {
    pub device_key: Arc<str>,
    pub metric: MetricId,
}

/// One bucket of a read span. `mean`, `min` and `max` are NaN for a gap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricPoint {
    /// Bucket start.
    pub t_realtime_ns: i64,
    pub mean: f32,
    pub min: f32,
    pub max: f32,
}

/// The answer to `MetricHistory::read`.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSpan {
    pub tier: Tier,
    /// Bucket width: a whole number of tier points.
    pub step_ns: u64,
    /// One point per bucket from the bucket of `from` to the bucket of `to`. Sorted by key;
    /// only series with a valid point in the span.
    pub series: Vec<(SeriesKey, Arc<[MetricPoint]>)>,
    /// Events that start inside the span, oldest first. Filled for the 1 min tier only.
    pub events: Vec<RealtimeEvent>,
}

/// Encodes `b` as postcard, compresses it with zstd level 3 and prefixes length and crc32.
pub fn encode_metric_block(b: &MetricBlock) -> Result<Vec<u8>, HistoryError> {
    let raw = postcard::to_stdvec(b)?;
    let compressed = zstd::bulk::compress(&raw, ZSTD_LEVEL).map_err(HistoryError::Zstd)?;
    let len = u32::try_from(compressed.len())
        .map_err(|_| HistoryError::Zstd(std::io::Error::other("compressed block exceeds 4 GiB")))?;
    let mut bytes = Vec::with_capacity(FRAME_PREFIX_LEN + compressed.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&compressed).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    Ok(bytes)
}

/// Decodes the frame at the start of `frame`. `offset` only goes into errors. Returns the block
/// and the bytes consumed. Never panics on bad input.
pub fn decode_metric_block(
    frame: &[u8],
    offset: u64,
) -> Result<(MetricBlock, usize), HistoryError> {
    FrameDecoder::new()?.decode(frame, offset)
}

/// Decodes frames with one zstd context and one output buffer, so a read of thousands of
/// blocks does not set them up again for each block.
struct FrameDecoder {
    zstd: zstd::bulk::Decompressor<'static>,
    raw: Vec<u8>,
}

impl FrameDecoder {
    fn new() -> Result<FrameDecoder, HistoryError> {
        Ok(FrameDecoder {
            zstd: zstd::bulk::Decompressor::new().map_err(HistoryError::Zstd)?,
            raw: Vec::new(),
        })
    }

    fn decode(&mut self, frame: &[u8], offset: u64) -> Result<(MetricBlock, usize), HistoryError> {
        let Some((prefix, rest)) = frame.split_first_chunk::<FRAME_PREFIX_LEN>() else {
            return Err(HistoryError::Truncated(offset));
        };
        let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
        let crc = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
        let Some(compressed) = rest.get(..len) else {
            return Err(HistoryError::Truncated(offset));
        };
        if crc32fast::hash(compressed) != crc {
            return Err(HistoryError::Crc(offset));
        }
        let capacity = match zstd::zstd_safe::get_frame_content_size(compressed) {
            Ok(Some(n)) if n > MAX_BLOCK_RAW as u64 => {
                return Err(HistoryError::Zstd(std::io::Error::other(
                    "block exceeds 64 MiB",
                )));
            }
            Ok(Some(n)) => n as usize,
            _ => MAX_BLOCK_RAW,
        };
        self.raw.clear();
        self.raw.reserve(capacity);
        self.zstd
            .decompress_to_buffer(compressed, &mut self.raw)
            .map_err(HistoryError::Zstd)?;
        let block = postcard::from_bytes(&self.raw)?;
        Ok((block, FRAME_PREFIX_LEN + len))
    }
}

/// Offsets of the good frames of a whole segment file: every frame from byte 64 up to the first
/// that runs past the end or fails its crc.
fn scan_frames(bytes: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut off = HEADER_LEN;
    while let Some((prefix, rest)) = bytes
        .get(off..)
        .and_then(|b| b.split_first_chunk::<FRAME_PREFIX_LEN>())
    {
        let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
        let crc = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
        let Some(body) = rest.get(..len) else { break };
        if crc32fast::hash(body) != crc {
            break;
        }
        out.push(off as u64);
        off += FRAME_PREFIX_LEN + len;
    }
    out
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> HistoryError + '_ {
    move |source| HistoryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Boottime `t` of a boot with `offset` on CLOCK_REALTIME.
fn rt(t: u64, offset: i64) -> i64 {
    (t as i64).saturating_add(offset)
}

fn minute_of(t: u64) -> u64 {
    t / METRIC_BLOCK_NS * METRIC_BLOCK_NS
}

fn bit(map: &[u8], i: usize) -> bool {
    map.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)
}

fn bitmap(n: usize, valid: impl Fn(usize) -> bool) -> Vec<u8> {
    let mut map = vec![0u8; n.div_ceil(8)];
    for i in (0..n).filter(|&i| valid(i)) {
        map[i / 8] |= 1 << (i % 8);
    }
    map
}

/// The valid 1 s values behind point `i`: its stored count, 1 in the 1 s tier, which stores
/// none, and at least 1 for a valid point.
fn weight_at(count: &Option<Vec<u8>>, i: usize) -> u32 {
    count
        .as_ref()
        .and_then(|c| c.get(i))
        .map_or(1, |&n| u32::from(n).max(1))
}

/// Bucket width for `[from, to]` in `tier`: a whole number of tier points, at least one, and at
/// most `MAX_POINTS` buckets aligned to multiples of the width (the ADR-012 `bucket_width_ns`
/// rule).
pub fn step_ns(tier: Tier, from_realtime_ns: i64, to_realtime_ns: i64) -> u64 {
    let span = to_realtime_ns.saturating_sub(from_realtime_ns).max(0) as u64;
    let per_point = span.div_ceil(MAX_POINTS as u64 - 1);
    per_point.div_ceil(tier.point_ns()).max(1) * tier.point_ns()
}

/// The three tier columns of one series for one minute of 1 s values (NaN = no value).
fn aggregate(slot: u16, secs: &[f32; 60]) -> [SeriesColumn; 3] {
    let s1 = SeriesColumn {
        slot,
        valid: bitmap(60, |i| !secs[i].is_nan()),
        mean: secs
            .iter()
            .map(|v| if v.is_nan() { 0.0 } else { *v })
            .collect(),
        min: None,
        max: None,
        count: None,
    };
    let mut n10 = [0u32; 6];
    let mut mean10 = [0f32; 6];
    let mut min10 = [0f32; 6];
    let mut max10 = [0f32; 6];
    for (j, chunk) in secs.as_chunks::<10>().0.iter().enumerate() {
        let mut sum = 0f64;
        let (mut lo, mut hi) = (f32::NAN, f32::NAN);
        for v in chunk.iter().filter(|v| !v.is_nan()) {
            n10[j] += 1;
            sum += f64::from(*v);
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        if n10[j] > 0 {
            mean10[j] = (sum / f64::from(n10[j])) as f32;
            min10[j] = lo;
            max10[j] = hi;
        }
    }
    let s10 = SeriesColumn {
        slot,
        valid: bitmap(6, |j| n10[j] > 0),
        mean: mean10.to_vec(),
        min: Some(min10.to_vec()),
        max: Some(max10.to_vec()),
        count: Some(n10.iter().map(|&n| n as u8).collect()),
    };
    let n: u32 = n10.iter().sum();
    let (mut sum, mut lo, mut hi) = (0f64, f32::NAN, f32::NAN);
    for j in (0..6).filter(|&j| n10[j] > 0) {
        sum += f64::from(mean10[j]) * f64::from(n10[j]);
        lo = lo.min(min10[j]);
        hi = hi.max(max10[j]);
    }
    let m1 = SeriesColumn {
        slot,
        valid: bitmap(1, |_| n > 0),
        mean: vec![if n > 0 {
            (sum / f64::from(n)) as f32
        } else {
            0.0
        }],
        min: Some(vec![if n > 0 { lo } else { 0.0 }]),
        max: Some(vec![if n > 0 { hi } else { 0.0 }]),
        count: Some(vec![n as u8]),
    };
    [s1, s10, m1]
}

/// Slot to series key and device name, as the newest catalog of a segment says.
type SlotTable = Vec<Option<(SeriesKey, Arc<str>)>>;

fn slot_table(catalog: &[HistSeries]) -> SlotTable {
    let len = catalog
        .iter()
        .map(|s| usize::from(s.slot) + 1)
        .max()
        .unwrap_or(0);
    let mut table = vec![None; len];
    for s in catalog {
        let key = SeriesKey {
            device_key: Arc::clone(&s.device_key),
            metric: s.metric.base(),
        };
        table[usize::from(s.slot)] = Some((key, Arc::clone(&s.device_name)));
    }
    table
}

/// One indexed segment file.
#[derive(Clone)]
struct Seg {
    path: PathBuf,
    header: MetricHeader,
    /// Offsets of the good frames, in file order.
    frames: Vec<u64>,
    size: u64,
    /// Start of the last block plus 60 s; the header start without a block.
    end_t_ns: u64,
}

impl Seg {
    /// Reads the header and indexes the frames up to the first bad one. A last frame whose crc
    /// matches but that does not decode is dropped too.
    fn open(path: PathBuf) -> Result<Seg, HistoryError> {
        let bytes = fs::read(&path).map_err(io_err(&path))?;
        let head = bytes.get(..HEADER_LEN).ok_or(HistoryError::BadHeader)?;
        let header = MetricHeader::decode(head)?;
        let mut frames = scan_frames(&bytes);
        let mut end_t_ns = header.start_t_ns;
        while let Some(&off) = frames.last() {
            match decode_metric_block(&bytes[off as usize..], off) {
                Ok((b, _)) => {
                    end_t_ns = b.start_t_ns.saturating_add(METRIC_BLOCK_NS);
                    break;
                }
                Err(_) => {
                    frames.pop();
                }
            }
        }
        Ok(Seg {
            path,
            header,
            frames,
            size: bytes.len() as u64,
            end_t_ns,
        })
    }

    fn start_rt(&self) -> i64 {
        rt(self.header.start_t_ns, self.header.realtime_offset_ns)
    }

    fn end_rt(&self) -> i64 {
        rt(self.end_t_ns, self.header.realtime_offset_ns)
    }

    /// Decodes the good blocks in file order and calls `f` with each block, the slot table in
    /// force and a number that changes whenever the table does. Stops at the first block that
    /// does not decode.
    fn read_blocks(
        &self,
        mut f: impl FnMut(&MetricBlock, &SlotTable, u32),
    ) -> Result<(), HistoryError> {
        let bytes = fs::read(&self.path).map_err(io_err(&self.path))?;
        let mut decoder = FrameDecoder::new()?;
        let mut table = SlotTable::new();
        let mut generation = 0u32;
        for &off in &self.frames {
            let Some(frame) = bytes.get(off as usize..) else {
                break;
            };
            let Ok((block, _)) = decoder.decode(frame, off) else {
                break;
            };
            if let Some(catalog) = &block.catalog {
                table = slot_table(catalog);
                generation = generation.wrapping_add(1);
            }
            f(&block, &table, generation);
        }
        Ok(())
    }
}

/// `seg-*.tlm` files in `dir`.
fn segment_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("seg-") && name.ends_with(".tlm") && entry.file_type()?.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// The open segment of one tier. It is the last of that tier's segments.
struct Writer {
    file: File,
    /// Catalog version the segment last wrote; `None` until its first block.
    catalog_version: Option<u64>,
}

/// One series known to this session: registered by the engine or loaded into the ring.
struct Series {
    key: SeriesKey,
    name: Arc<str>,
    /// Registered in this session, so it is in the catalog written to disk.
    live: bool,
    /// The last hour of 1 s points at `realtime second mod RING_SECS`. NaN is a gap.
    ring: Option<Box<[f32]>>,
    /// Boottime second of the point being built and its running maximum (NaN while no valid
    /// sample came).
    open: Option<(u64, f32)>,
    /// Last closed second; later samples from it are dropped.
    closed: Option<u64>,
    /// Realtime second after the newest point loaded from disk. Ring points before it are
    /// already stored, so a minute written after a restart leaves them out.
    stored_to: i64,
}

/// Accumulated buckets of one series in a span. Bucket `idx` lives at `idx mod len`.
struct Col {
    /// Sum of each point's mean times its weight.
    sum: Vec<f32>,
    min: Vec<f32>,
    max: Vec<f32>,
    /// Sum of the weights: the valid 1 s values behind the bucket.
    n: Vec<u32>,
}

impl Col {
    fn new(len: usize) -> Col {
        Col {
            sum: vec![0.0; len],
            min: vec![f32::NAN; len],
            max: vec![f32::NAN; len],
            n: vec![0; len],
        }
    }

    /// Adds a point with `weight` valid 1 s values. `f32::min` and `f32::max` ignore NaN, so the
    /// first point sets min and max.
    fn add(&mut self, i: usize, mean: f32, min: f32, max: f32, weight: u32) {
        self.sum[i] += mean * weight as f32;
        self.n[i] += weight;
        self.min[i] = self.min[i].min(min);
        self.max[i] = self.max[i].max(max);
    }

    fn clear(&mut self, i: usize) {
        self.sum[i] = 0.0;
        self.min[i] = f32::NAN;
        self.max[i] = f32::NAN;
        self.n[i] = 0;
    }
}

/// One overlay point: realtime, mean, min, max and valid 1 s values.
type OverlayPoint = (i64, f32, f32, f32, u32);

/// Points and events of the minutes not yet written, laid over a span when it is read.
#[derive(Default)]
struct Overlay {
    points: HashMap<SeriesKey, Vec<OverlayPoint>>,
    events: Vec<RealtimeEvent>,
}

/// A cached read of the 10 s or 1 min tier: `len` buckets of `step` ns from bucket `lo`.
struct Span {
    tier: Tier,
    step: u64,
    lo: i64,
    len: usize,
    /// No block on disk was newer than the span when it was filled, so blocks written since
    /// slide it forward.
    live: bool,
    keys: HashMap<SeriesKey, usize>,
    cols: Vec<(SeriesKey, Col)>,
    events: Vec<RealtimeEvent>,
    last_used: TimeNs,
}

impl Span {
    fn hi(&self) -> i64 {
        self.lo + self.len as i64 - 1
    }

    fn step_i(&self) -> i64 {
        self.step as i64
    }

    fn pos(&self, idx: i64) -> usize {
        idx.rem_euclid(self.len as i64) as usize
    }

    fn col(&mut self, key: &SeriesKey) -> usize {
        if let Some(&i) = self.keys.get(key) {
            return i;
        }
        let i = self.cols.len();
        self.cols.push((key.clone(), Col::new(self.len)));
        self.keys.insert(key.clone(), i);
        i
    }

    /// Moves the span forward so `hi` is its newest bucket. Buckets that enter start empty.
    fn slide_to(&mut self, hi: i64) {
        let old_hi = self.hi();
        if hi <= old_hi {
            return;
        }
        let lo = hi - self.len as i64 + 1;
        for idx in (old_hi + 1).max(lo)..=hi {
            let p = self.pos(idx);
            for (_, c) in &mut self.cols {
                c.clear(p);
            }
        }
        self.lo = lo;
        let from = lo * self.step_i();
        self.events.retain(|e| e.start_realtime_ns >= from);
    }

    /// Folds one point with `weight` valid 1 s values. A point outside the span is dropped,
    /// unless `slide` and it is newer.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        col: usize,
        t_rt: i64,
        mean: f32,
        min: f32,
        max: f32,
        weight: u32,
        slide: bool,
    ) {
        let idx = t_rt.div_euclid(self.step_i());
        if slide && idx > self.hi() {
            self.slide_to(idx);
        }
        if idx < self.lo || idx > self.hi() {
            return;
        }
        let p = self.pos(idx);
        self.cols[col].1.add(p, mean, min, max, weight);
    }

    fn add_event(&mut self, e: RealtimeEvent, slide: bool) {
        let idx = e.start_realtime_ns.div_euclid(self.step_i());
        if slide && idx > self.hi() {
            self.slide_to(idx);
        }
        if idx >= self.lo && idx <= self.hi() {
            self.events.push(e);
        }
    }

    /// Folds one block. `cols[slot]` is the span column of each slot; `offset` is the block's
    /// realtime offset.
    fn add_block(&mut self, b: &MetricBlock, cols: &[Option<usize>], offset: i64, slide: bool) {
        let point = self.tier.point_ns();
        for c in &b.columns {
            let Some(&Some(col)) = cols.get(usize::from(c.slot)) else {
                continue;
            };
            for (i, &mean) in c.mean.iter().enumerate() {
                if !bit(&c.valid, i) {
                    continue;
                }
                let at = |v: &Option<Vec<f32>>| v.as_ref().and_then(|v| v.get(i).copied());
                let min = at(&c.min).unwrap_or(mean);
                let max = at(&c.max).unwrap_or(mean);
                let weight = weight_at(&c.count, i);
                let t = rt(b.start_t_ns + i as u64 * point, offset);
                self.add(col, t, mean, min, max, weight, slide);
            }
        }
        for e in &b.events {
            let start_realtime_ns = rt(e.start_t_ns, offset);
            let event = e.clone();
            self.add_event(
                RealtimeEvent {
                    start_realtime_ns,
                    event,
                },
                slide,
            );
        }
    }

    /// The span as read, from bucket `lo` to `hi`, with `overlay` folded in.
    fn output(&self, lo: i64, hi: i64, overlay: &Overlay) -> MetricSpan {
        let step = self.step_i();
        let in_span = |idx: i64| idx >= lo && idx <= hi;
        let empty = Col::new(self.len);
        let mut keys: Vec<&SeriesKey> = self.cols.iter().map(|(k, _)| k).collect();
        keys.extend(
            overlay
                .points
                .keys()
                .filter(|k| !self.keys.contains_key(*k)),
        );
        let mut series = Vec::new();
        for key in keys {
            let col = self.keys.get(key).map_or(&empty, |&i| &self.cols[i].1);
            let mut extra: HashMap<i64, (f32, u32, f32, f32)> = HashMap::new();
            for &(t, mean, min, max, w) in overlay.points.get(key).into_iter().flatten() {
                let idx = t.div_euclid(step);
                if in_span(idx) {
                    let e = extra.entry(idx).or_insert((0.0, 0, f32::NAN, f32::NAN));
                    *e = (e.0 + mean * w as f32, e.1 + w, e.2.min(min), e.3.max(max));
                }
            }
            let mut any = false;
            let points: Arc<[MetricPoint]> = (lo..=hi)
                .map(|idx| {
                    let p = self.pos(idx);
                    let (mut sum, mut n) = (col.sum[p], col.n[p]);
                    let (mut min, mut max) = (col.min[p], col.max[p]);
                    if let Some(e) = extra.get(&idx) {
                        sum += e.0;
                        n += e.1;
                        min = min.min(e.2);
                        max = max.max(e.3);
                    }
                    any |= n > 0;
                    let mean = if n > 0 { sum / n as f32 } else { f32::NAN };
                    MetricPoint {
                        t_realtime_ns: idx * step,
                        mean,
                        min: if n > 0 { min } else { f32::NAN },
                        max: if n > 0 { max } else { f32::NAN },
                    }
                })
                .collect();
            if any {
                series.push((key.clone(), points));
            }
        }
        series.sort_by(|a, b| a.0.cmp(&b.0));
        let (from, to) = (lo * step, (hi + 1) * step);
        let mut events: Vec<RealtimeEvent> = self
            .events
            .iter()
            .chain(&overlay.events)
            .filter(|e| e.start_realtime_ns >= from && e.start_realtime_ns < to)
            .cloned()
            .collect();
        events.sort_by_key(|e| e.start_realtime_ns);
        MetricSpan {
            tier: self.tier,
            step_ns: self.step,
            series,
            events,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MetricConfig {
    /// `$XDG_STATE_HOME/tasklens/metrics`.
    pub root: PathBuf,
    pub boot: BootId,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME for this engine session. Every new segment header and
    /// every conversion of this session's times uses it.
    pub realtime_offset_ns: i64,
    /// `history.device_metrics_max_mb` in bytes.
    pub max_disk_bytes: u64,
    /// `fdatasync` after every block, so a crash loses at most one minute. Only tests turn it
    /// off.
    pub sync: bool,
}

/// Result of `MetricHistory::open`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricLoadReport {
    /// Segments indexed, all tiers.
    pub segments: usize,
    /// Valid 1 s points loaded into the ring.
    pub ring_points: usize,
    /// Boot of the newest segment on disk. When it differs from this boot, a `Boot` event at
    /// this boot's start is queued for the next 1 min block.
    pub previous_boot: Option<BootId>,
    /// The deadline passed while loading the ring; the ring starts empty.
    pub timed_out: bool,
    /// Files with another format version, left alone.
    pub other_version: usize,
}

/// A series handle from `MetricHistory::register`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MetricSlot(pub u16);

/// The device metric history of ADR-017.
pub struct MetricHistory {
    cfg: MetricConfig,
    /// Per tier, oldest first. While the tier's writer is set, the last one is open.
    segs: [Vec<Seg>; 3],
    writers: [Option<Writer>; 3],
    series: Vec<Series>,
    by_key: HashMap<SeriesKey, u16>,
    /// Bumps when a series registers or renames.
    catalog_version: u64,
    /// Realtime second after the newest ring point; the ring covers the hour before it.
    ring_end: i64,
    /// Boottime minute starts with closed points not yet written.
    pending: BTreeSet<u64>,
    /// End of the newest written minute. Points before it are dropped.
    written_to: u64,
    /// Per tier: the next block gets `gap_before`.
    gap_pending: [bool; 3],
    /// Events for the next 1 min block.
    events: Vec<HistEvent>,
    spans: Vec<Span>,
    /// `tick` applies retention and the cap at this boottime even when it wrote nothing, so
    /// segments still expire after the samples stop.
    next_enforce_t_ns: u64,
}

impl MetricHistory {
    /// Creates `root/1s`, `root/10s` and `root/1m` (0700) if needed, indexes every segment,
    /// applies retention and the cap, and loads the last hour of the 1 s tier into the ring.
    /// When `deadline` passes during the ring load, the ring starts empty. Fails only when a
    /// directory cannot be created or read.
    pub fn open(
        cfg: MetricConfig,
        now_t_ns: TimeNs,
        deadline: Instant,
    ) -> Result<(MetricHistory, MetricLoadReport), HistoryError> {
        let mut h = MetricHistory {
            cfg,
            segs: [Vec::new(), Vec::new(), Vec::new()],
            writers: [None, None, None],
            series: Vec::new(),
            by_key: HashMap::new(),
            catalog_version: 0,
            ring_end: i64::MIN,
            pending: BTreeSet::new(),
            written_to: 0,
            gap_pending: [true; 3],
            events: Vec::new(),
            spans: Vec::new(),
            next_enforce_t_ns: 0,
        };
        let mut report = MetricLoadReport::default();
        for tier in Tier::ALL {
            let dir = h.cfg.root.join(tier.dir_name());
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(io_err(&dir))?;
            for path in segment_files(&dir).map_err(io_err(&dir))? {
                match Seg::open(path.clone()) {
                    Ok(seg) if seg.header.tier == tier => h.segs[tier.index()].push(seg),
                    Err(HistoryError::Version(_)) => report.other_version += 1,
                    // A short file or a wrong magic or tier: a start that crashed before its
                    // header was written, or not ours to read.
                    Ok(_) | Err(HistoryError::BadHeader) => {
                        let _ = fs::remove_file(&path);
                    }
                    Err(_) => {}
                }
            }
            h.segs[tier.index()]
                .sort_by(|a, b| (a.start_rt(), &a.path).cmp(&(b.start_rt(), &b.path)));
        }
        report.previous_boot = h
            .segs
            .iter()
            .flatten()
            .max_by_key(|s| s.end_rt())
            .map(|s| s.header.boot_id);
        if report.previous_boot.is_some_and(|boot| boot != h.cfg.boot) {
            h.events.push(HistEvent {
                kind: EventKind::Boot,
                start_t_ns: 0,
                duration_ms: 0,
                peak: 0.0,
                device_key: None,
                process: None,
            });
        }
        // A failed delete stays indexed and is tried again at the next tick.
        let _ = h.enforce(now_t_ns);
        report.segments = h.segs.iter().map(Vec::len).sum();

        match h.load_ring(now_t_ns, deadline) {
            Some(points) => report.ring_points = points,
            None => {
                report.timed_out = true;
                for s in &mut h.series {
                    s.ring = None;
                    s.stored_to = i64::MIN;
                }
                h.ring_end = i64::MIN;
            }
        }
        Ok((h, report))
    }

    /// Loads the 1 s points of the last hour. `None` when the deadline passed.
    fn load_ring(&mut self, now_t_ns: TimeNs, deadline: Instant) -> Option<usize> {
        let now_rt = rt(now_t_ns, self.cfg.realtime_offset_ns);
        let from_s = now_rt.div_euclid(NS_PER_SEC as i64) - RING_SECS as i64;
        let from_rt = from_s * NS_PER_SEC as i64;
        let segs: Vec<Seg> = self.segs[Tier::S1.index()]
            .iter()
            .filter(|s| s.end_rt() > from_rt)
            .cloned()
            .collect();
        let mut points = 0usize;
        for seg in segs {
            if Instant::now() >= deadline {
                return None;
            }
            let offset = seg.header.realtime_offset_ns;
            let mut slots: Vec<Option<usize>> = Vec::new();
            let mut generation = u32::MAX;
            let read = seg.read_blocks(|b, table, g| {
                if g != generation {
                    generation = g;
                    slots = table
                        .iter()
                        .map(|e| e.as_ref().and_then(|(k, n)| self.series_index(k, n)))
                        .collect();
                }
                for c in &b.columns {
                    let Some(&Some(idx)) = slots.get(usize::from(c.slot)) else {
                        continue;
                    };
                    for (i, &v) in c.mean.iter().enumerate() {
                        let t = rt(b.start_t_ns + i as u64 * NS_PER_SEC, offset);
                        let r = t.div_euclid(NS_PER_SEC as i64);
                        if bit(&c.valid, i) && r >= from_s && self.ring_write(idx, r, v) {
                            let s = &mut self.series[idx];
                            s.stored_to = s.stored_to.max(r + 1);
                            points += 1;
                        }
                    }
                }
            });
            // A segment that cannot be read leaves its seconds empty.
            let _ = read;
        }
        Some(points)
    }

    /// The registry index of `key`, added as not live when new. `None` when 65,536 series exist.
    fn series_index(&mut self, key: &SeriesKey, name: &Arc<str>) -> Option<usize> {
        if let Some(&slot) = self.by_key.get(key) {
            return Some(usize::from(slot));
        }
        let slot = u16::try_from(self.series.len()).ok()?;
        self.series.push(Series {
            key: key.clone(),
            name: Arc::clone(name),
            live: false,
            ring: None,
            open: None,
            closed: None,
            stored_to: i64::MIN,
        });
        self.by_key.insert(key.clone(), slot);
        Some(usize::from(slot))
    }

    /// The handle for `(device_key, metric)`. A series seen before, in this session or in the
    /// loaded ring, keeps its points. `None` when 65,536 series exist.
    pub fn register(
        &mut self,
        device_key: &Arc<str>,
        device_name: &Arc<str>,
        metric: MetricId,
    ) -> Option<MetricSlot> {
        let key = SeriesKey {
            device_key: Arc::clone(device_key),
            metric: metric.base(),
        };
        let idx = self.series_index(&key, device_name)?;
        let s = &mut self.series[idx];
        if !s.live || s.name != *device_name {
            s.live = true;
            s.name = Arc::clone(device_name);
            self.catalog_version += 1;
        }
        u16::try_from(idx).ok().map(MetricSlot)
    }

    /// Folds one live sample into the 1 s point of its boottime second: the maximum of the valid
    /// samples. A sample that is not valid is no input. `GAP_BEFORE` ends the point being built;
    /// later samples of a closed second are dropped.
    pub fn push(&mut self, slot: MetricSlot, s: Sample) {
        let idx = usize::from(slot.0);
        if idx >= self.series.len() {
            return;
        }
        let sec = s.t_ns / NS_PER_SEC;
        if s.flags.contains(SampleFlags::GAP_BEFORE) {
            self.close_point(idx);
        }
        match self.series[idx].open {
            Some((open, _)) if sec < open => return,
            Some((open, _)) if sec > open => {
                self.close_point(idx);
                self.series[idx].open = Some((sec, f32::NAN));
            }
            Some(_) => {}
            None => {
                let closed = self.series[idx].closed.is_some_and(|c| sec <= c);
                if closed || sec * NS_PER_SEC < self.written_to {
                    return;
                }
                self.series[idx].open = Some((sec, f32::NAN));
            }
        }
        if s.is_valid()
            && s.value.is_finite()
            && let Some((_, v)) = &mut self.series[idx].open
        {
            *v = if v.is_nan() { s.value } else { v.max(s.value) };
        }
    }

    /// Ends every point being built and marks the next block of each tier `gap_before`. For a
    /// suspend or a sampling stop.
    pub fn mark_gap(&mut self) {
        for idx in 0..self.series.len() {
            self.close_point(idx);
        }
        self.gap_pending = [true; 3];
    }

    /// Queues an event for the next 1 min block. `start_t_ns` is CLOCK_BOOTTIME of this boot.
    pub fn push_event(&mut self, e: HistEvent) {
        self.events.push(e);
    }

    /// Realtime second of boottime second `sec` in this session.
    fn rt_sec(&self, sec: u64) -> i64 {
        (sec as i64).saturating_add(self.cfg.realtime_offset_ns.div_euclid(NS_PER_SEC as i64))
    }

    fn close_point(&mut self, idx: usize) {
        let Some((sec, v)) = self.series[idx].open.take() else {
            return;
        };
        self.series[idx].closed = Some(sec);
        if v.is_nan() || sec * NS_PER_SEC < self.written_to {
            return;
        }
        let r = self.rt_sec(sec);
        if self.ring_write(idx, r, v) {
            self.pending.insert(minute_of(sec * NS_PER_SEC));
        }
    }

    /// Writes one 1 s point at realtime second `r`. False when `r` is an hour or more older than
    /// the newest point.
    fn ring_write(&mut self, idx: usize, r: i64, v: f32) -> bool {
        if r >= self.ring_end {
            self.ring_advance(r + 1);
        }
        if r < self.ring_end.saturating_sub(RING_SECS as i64) {
            return false;
        }
        let ring = self.series[idx]
            .ring
            .get_or_insert_with(|| vec![f32::NAN; RING_SECS].into_boxed_slice());
        ring[r.rem_euclid(RING_SECS as i64) as usize] = v;
        true
    }

    /// Moves the ring end to `end`, clearing the seconds that enter.
    fn ring_advance(&mut self, end: i64) {
        let from = self.ring_end.max(end - RING_SECS as i64);
        for r in from..end {
            let i = r.rem_euclid(RING_SECS as i64) as usize;
            for s in &mut self.series {
                if let Some(ring) = &mut s.ring {
                    ring[i] = f32::NAN;
                }
            }
        }
        self.ring_end = end;
    }

    /// The ring value at realtime second `r`, NaN outside the last hour.
    fn ring_at(ring: &[f32], ring_end: i64, r: i64) -> f32 {
        if r >= ring_end || r < ring_end.saturating_sub(RING_SECS as i64) {
            return f32::NAN;
        }
        ring[r.rem_euclid(RING_SECS as i64) as usize]
    }

    /// The 1 s values of the live series in boottime minute `m` that are not on disk yet, for
    /// each series with one.
    fn minute_values(&self, m: u64) -> Vec<(usize, [f32; 60])> {
        let r0 = self.rt_sec(m / NS_PER_SEC);
        let mut out = Vec::new();
        for (idx, s) in self.series.iter().enumerate() {
            let (true, Some(ring)) = (s.live, &s.ring) else {
                continue;
            };
            let mut secs = [f32::NAN; 60];
            for (i, v) in secs.iter_mut().enumerate() {
                let r = r0 + i as i64;
                if r >= s.stored_to {
                    *v = Self::ring_at(ring, self.ring_end, r);
                }
            }
            if secs.iter().any(|v| !v.is_nan()) {
                out.push((idx, secs));
            }
        }
        out
    }

    /// Writes every minute that ended `FLUSH_GRACE_NS` or more before `now_t_ns`, moves the 1 s
    /// ring up to now, applies retention and the cap after a write and at least once a minute,
    /// and drops spans idle for `SPAN_IDLE_NS`. Call it about once a second. Returns the first error; the other minutes
    /// and tiers go on.
    pub fn tick(&mut self, now_t_ns: TimeNs) -> Result<(), HistoryError> {
        let cutoff = now_t_ns.saturating_sub(FLUSH_GRACE_NS);
        for idx in 0..self.series.len() {
            let due = self.series[idx]
                .open
                .is_some_and(|(sec, _)| minute_of(sec * NS_PER_SEC) + METRIC_BLOCK_NS <= cutoff);
            if due {
                self.close_point(idx);
            }
        }
        let mut first_err = None;
        let mut wrote = false;
        while let Some(&m) = self.pending.first() {
            if m + METRIC_BLOCK_NS > cutoff {
                break;
            }
            self.pending.pop_first();
            wrote = true;
            if let Err(e) = self.flush_minute(m) {
                first_err.get_or_insert(e);
            }
        }
        // Events with no minute to ride on get a block of their own.
        let last = minute_of(cutoff);
        if self.pending.is_empty()
            && last >= METRIC_BLOCK_NS
            && self.events.iter().any(|e| e.start_t_ns < last)
        {
            wrote = true;
            if let Err(e) = self.flush_minute(last - METRIC_BLOCK_NS) {
                first_err.get_or_insert(e);
            }
        }
        // Without samples the ring would keep its last hour forever. Every pending minute that is
        // due was written above; the others are less than a minute old.
        let now_end = self.rt_sec(now_t_ns / NS_PER_SEC) + 1;
        if now_end > self.ring_end {
            self.ring_advance(now_end);
        }
        if wrote || now_t_ns >= self.next_enforce_t_ns {
            self.next_enforce_t_ns = now_t_ns.saturating_add(METRIC_BLOCK_NS);
            if let Err(e) = self.enforce(now_t_ns) {
                first_err.get_or_insert(e);
            }
        }
        self.spans
            .retain(|s| now_t_ns.saturating_sub(s.last_used) < SPAN_IDLE_NS);
        first_err.map_or(Ok(()), Err)
    }

    /// Engine shutdown: ends every point, writes every minute not yet written (the newest one
    /// partly) and the queued events, and closes the segments.
    pub fn close(&mut self, now_t_ns: TimeNs) -> Result<(), HistoryError> {
        for idx in 0..self.series.len() {
            self.close_point(idx);
        }
        let mut first_err = None;
        while let Some(m) = self.pending.pop_first() {
            if let Err(e) = self.flush_minute(m) {
                first_err.get_or_insert(e);
            }
        }
        if !self.events.is_empty() {
            let newest = self.events.iter().map(|e| e.start_t_ns).max().unwrap_or(0);
            let m = self.written_to.max(minute_of(now_t_ns.max(newest)));
            if let Err(e) = self.flush_minute(m) {
                first_err.get_or_insert(e);
            }
        }
        if let Err(e) = self.enforce(now_t_ns) {
            first_err.get_or_insert(e);
        }
        for tier in Tier::ALL {
            if let Err(e) = self.close_writer(tier) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    fn close_writer(&mut self, tier: Tier) -> Result<(), HistoryError> {
        let Some(w) = self.writers[tier.index()].take() else {
            return Ok(());
        };
        if !self.cfg.sync {
            return Ok(());
        }
        let path = self.segs[tier.index()].last().map(|s| s.path.clone());
        w.file.sync_all().map_err(io_err(&path.unwrap_or_default()))
    }

    /// Builds and writes the three blocks of boottime minute `m`. Queued events that start
    /// before the minute ends go into its 1 min block; the others wait for a later one. A minute
    /// written before gets a 1 min block with the events only, so no point is stored twice.
    fn flush_minute(&mut self, m: u64) -> Result<(), HistoryError> {
        let mut columns: [Vec<SeriesColumn>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let values = if m + METRIC_BLOCK_NS <= self.written_to {
            Vec::new()
        } else {
            self.minute_values(m)
        };
        for (idx, secs) in values {
            let slot = u16::try_from(idx).unwrap_or(u16::MAX);
            for (tier, col) in aggregate(slot, &secs).into_iter().enumerate() {
                columns[tier].push(col);
            }
        }
        self.written_to = self.written_to.max(m + METRIC_BLOCK_NS);
        let (events, later): (Vec<HistEvent>, Vec<HistEvent>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|e| e.start_t_ns < m + METRIC_BLOCK_NS);
        self.events = later;
        let mut first_err = None;
        for (tier, cols) in Tier::ALL.into_iter().zip(columns) {
            let events = if tier == Tier::M1 {
                events.clone()
            } else {
                Vec::new()
            };
            if cols.is_empty() && events.is_empty() {
                continue;
            }
            let block = MetricBlock {
                start_t_ns: m,
                gap_before: self.gap_pending[tier.index()],
                catalog: None,
                columns: cols,
                events,
            };
            if let Err(e) = self.append(tier, block) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    fn catalog(&self) -> Vec<HistSeries> {
        self.series
            .iter()
            .enumerate()
            .filter(|(_, s)| s.live)
            .map(|(slot, s)| HistSeries {
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
                device_key: Arc::clone(&s.key.device_key),
                device_name: Arc::clone(&s.name),
                metric: s.key.metric,
            })
            .collect()
    }

    /// Creates the tier's next segment with its header. Two starts in one realtime second take
    /// the next free second for the name.
    fn create_segment(&mut self, tier: Tier, start_t_ns: u64) -> Result<(), HistoryError> {
        let dir = self.cfg.root.join(tier.dir_name());
        let header = MetricHeader {
            tier,
            boot_id: self.cfg.boot,
            realtime_offset_ns: self.cfg.realtime_offset_ns,
            start_t_ns,
        };
        let secs = rt(start_t_ns, self.cfg.realtime_offset_ns).div_euclid(NS_PER_SEC as i64);
        for k in 0..NAME_TRIES {
            let path = dir.join(metric_segment_file_name(secs + k));
            let opened = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path);
            let mut file = match opened {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_err(&path)(e)),
            };
            if let Err(e) = file.write_all(&header.encode()) {
                let _ = fs::remove_file(&path);
                return Err(io_err(&path)(e));
            }
            self.segs[tier.index()].push(Seg {
                path,
                header,
                frames: Vec::new(),
                size: HEADER_LEN as u64,
                end_t_ns: start_t_ns,
            });
            self.writers[tier.index()] = Some(Writer {
                file,
                catalog_version: None,
            });
            return Ok(());
        }
        Err(io_err(&dir.join(metric_segment_file_name(secs)))(
            std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        ))
    }

    /// Writes `block` to the tier's open segment, rotating at the tier's segment length, then
    /// folds it into the live spans of the tier. After a failed write the segment keeps its good
    /// blocks and the next block starts a new segment.
    fn append(&mut self, tier: Tier, mut block: MetricBlock) -> Result<(), HistoryError> {
        let t = tier.index();
        let rotate = self.segs[t]
            .last()
            .is_some_and(|s| block.start_t_ns >= s.header.start_t_ns + tier.segment_ns());
        if self.writers[t].is_some() && rotate {
            self.close_writer(tier)?;
        }
        if self.writers[t].is_none() {
            self.create_segment(tier, block.start_t_ns)?;
        }
        let version = self.catalog_version;
        let catalog_due = self.writers[t]
            .as_ref()
            .is_some_and(|w| w.catalog_version != Some(version));
        if catalog_due {
            block.catalog = Some(self.catalog());
        }
        let bytes = encode_metric_block(&block)?;
        let sync = self.cfg.sync;
        let (Some(w), Some(seg)) = (self.writers[t].as_mut(), self.segs[t].last_mut()) else {
            unreachable!("create_segment set a writer and its segment");
        };
        let written = w
            .file
            .write_all(&bytes)
            .and_then(|()| if sync { w.file.sync_data() } else { Ok(()) });
        if let Err(e) = written {
            self.writers[t] = None;
            if let Ok(m) = fs::metadata(&seg.path) {
                seg.size = m.len();
            }
            return Err(io_err(&seg.path)(e));
        }
        w.catalog_version = Some(version);
        seg.frames.push(seg.size);
        seg.size += bytes.len() as u64;
        seg.end_t_ns = block.start_t_ns + METRIC_BLOCK_NS;
        self.gap_pending[t] = false;

        let offset = self.cfg.realtime_offset_ns;
        let series = &self.series;
        for span in self.spans.iter_mut().filter(|s| s.tier == tier) {
            let mut cols: Vec<Option<usize>> = Vec::new();
            for c in &block.columns {
                let slot = usize::from(c.slot);
                if cols.len() <= slot {
                    cols.resize(slot + 1, None);
                }
                cols[slot] = series.get(slot).map(|s| span.col(&s.key));
            }
            let live = span.live;
            span.add_block(&block, &cols, offset, live);
        }
        Ok(())
    }

    /// Deletes the segments whose end is a tier retention or more before now, then the closed
    /// segments with the oldest end, of any tier, while the store is over its cap. An open
    /// segment is closed first when it expired (no block for a whole retention), and the next
    /// block starts a new one; the cap never deletes an open segment. A segment whose delete
    /// fails stays indexed; the first error is returned at the end.
    fn enforce(&mut self, now_t_ns: TimeNs) -> Result<(), HistoryError> {
        let now_rt = rt(now_t_ns, self.cfg.realtime_offset_ns);
        let mut first_err = None;
        for tier in Tier::ALL {
            let expired = self.writers[tier.index()].is_some()
                && self.segs[tier.index()].last().is_some_and(|s| {
                    now_rt.saturating_sub(s.end_rt()) >= tier.retention_ns() as i64
                });
            if expired && let Err(e) = self.close_writer(tier) {
                first_err.get_or_insert(e);
            }
        }
        let mut remove = |seg: &Seg| match fs::remove_file(&seg.path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                first_err.get_or_insert(io_err(&seg.path)(e));
                false
            }
        };
        for tier in Tier::ALL {
            let t = tier.index();
            let closed = self.segs[t].len() - usize::from(self.writers[t].is_some());
            let retention = tier.retention_ns() as i64;
            let mut i = 0;
            let mut left = closed;
            while i < left {
                let seg = &self.segs[t][i];
                if now_rt.saturating_sub(seg.end_rt()) >= retention && remove(seg) {
                    self.segs[t].remove(i);
                    left -= 1;
                } else {
                    i += 1;
                }
            }
        }
        let mut failed: Vec<PathBuf> = Vec::new();
        while self.disk_bytes() > self.cfg.max_disk_bytes {
            let oldest = Tier::ALL
                .into_iter()
                .flat_map(|tier| {
                    let t = tier.index();
                    let closed = self.segs[t].len() - usize::from(self.writers[t].is_some());
                    (0..closed).map(move |i| (t, i))
                })
                .filter(|&(t, i)| !failed.contains(&self.segs[t][i].path))
                .min_by_key(|&(t, i)| self.segs[t][i].end_rt());
            let Some((t, i)) = oldest else { break };
            if remove(&self.segs[t][i]) {
                self.segs[t].remove(i);
            } else {
                failed.push(self.segs[t][i].path.clone());
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Reads `[from, to]` of one tier in at most `MAX_POINTS` buckets per series (`step_ns`). The
    /// 1 s tier comes from the ring. The 10 s and 1 min tiers come from a span cache that is
    /// filled from disk once and then follows new blocks; the minutes not yet written are laid
    /// over it. `now_t_ns` marks the span used.
    pub fn read(
        &mut self,
        tier: Tier,
        from_realtime_ns: i64,
        to_realtime_ns: i64,
        now_t_ns: TimeNs,
    ) -> Result<MetricSpan, HistoryError> {
        let to_realtime_ns = to_realtime_ns.max(from_realtime_ns);
        let step = step_ns(tier, from_realtime_ns, to_realtime_ns);
        let lo = from_realtime_ns.div_euclid(step as i64);
        let hi = to_realtime_ns.div_euclid(step as i64);
        if tier == Tier::S1 {
            return Ok(self.read_ring(step, lo, hi));
        }
        let len = (hi - lo + 1) as usize;
        let hit = self.spans.iter().position(|s| {
            s.tier == tier
                && s.step == step
                && s.len == len
                && (s.lo == lo || (s.live && lo > s.lo))
        });
        let i = match hit {
            Some(i) => i,
            None => {
                let span = self.fill(tier, step, lo, len, now_t_ns)?;
                if self.spans.len() >= MAX_SPANS
                    && let Some(old) =
                        (0..self.spans.len()).min_by_key(|&i| self.spans[i].last_used)
                {
                    self.spans.remove(old);
                }
                self.spans.push(span);
                self.spans.len() - 1
            }
        };
        let overlay = if self.spans[i].live {
            self.overlay(tier)
        } else {
            Overlay::default()
        };
        let span = &mut self.spans[i];
        span.last_used = now_t_ns;
        span.slide_to(hi);
        Ok(span.output(lo, hi, &overlay))
    }

    /// A new span of `len` buckets from `lo`, filled from the tier's segments.
    fn fill(
        &self,
        tier: Tier,
        step: u64,
        lo: i64,
        len: usize,
        now_t_ns: TimeNs,
    ) -> Result<Span, HistoryError> {
        let mut span = Span {
            tier,
            step,
            lo,
            len,
            live: false,
            keys: HashMap::new(),
            cols: Vec::new(),
            events: Vec::new(),
            last_used: now_t_ns,
        };
        let from = lo * step as i64;
        let to = (span.hi() + 1) * step as i64;
        let segs = &self.segs[tier.index()];
        // An event can start before its segment, at the earliest at its boot (boottime 0), so
        // the 1 min tier also reads the later segments of a boot that started before `to`, for
        // their events only.
        let first_rt = |s: &Seg| match tier {
            Tier::M1 => s.header.realtime_offset_ns,
            _ => s.start_rt(),
        };
        for seg in segs
            .iter()
            .filter(|s| s.end_rt() > from && first_rt(s) < to)
        {
            let offset = seg.header.realtime_offset_ns;
            let points = seg.start_rt() < to;
            let mut cols: Vec<Option<usize>> = Vec::new();
            let mut generation = u32::MAX;
            seg.read_blocks(|b, table, g| {
                if points && g != generation {
                    generation = g;
                    cols = table
                        .iter()
                        .map(|e| e.as_ref().map(|(k, _)| span.col(k)))
                        .collect();
                }
                span.add_block(b, &cols, offset, false);
            })?;
        }
        span.live = segs.iter().all(|s| s.end_rt() <= to);
        Ok(span)
    }

    /// The 10 s or 1 min points of the minutes not yet written, and the queued events.
    fn overlay(&self, tier: Tier) -> Overlay {
        let mut overlay = Overlay::default();
        let offset = self.cfg.realtime_offset_ns;
        let point = tier.point_ns();
        for &m in &self.pending {
            for (idx, secs) in self.minute_values(m) {
                let col = &aggregate(0, &secs)[tier.index()];
                let points = overlay
                    .points
                    .entry(self.series[idx].key.clone())
                    .or_default();
                for (i, &mean) in col.mean.iter().enumerate() {
                    if !bit(&col.valid, i) {
                        continue;
                    }
                    let at = |v: &Option<Vec<f32>>| v.as_ref().and_then(|v| v.get(i).copied());
                    let t = rt(m + i as u64 * point, offset);
                    points.push((
                        t,
                        mean,
                        at(&col.min).unwrap_or(mean),
                        at(&col.max).unwrap_or(mean),
                        weight_at(&col.count, i),
                    ));
                }
            }
        }
        if tier == Tier::M1 {
            overlay.events = self
                .events
                .iter()
                .map(|e| RealtimeEvent {
                    start_realtime_ns: rt(e.start_t_ns, offset),
                    event: e.clone(),
                })
                .collect();
        }
        overlay
    }

    /// The 1 s tier from the ring, bucketed like the other tiers.
    fn read_ring(&self, step: u64, lo: i64, hi: i64) -> MetricSpan {
        let sec = NS_PER_SEC as i64;
        let len = (hi - lo + 1) as usize;
        let first = (lo * step as i64)
            .div_euclid(sec)
            .max(self.ring_end.saturating_sub(RING_SECS as i64));
        let last = ((hi + 1) * step as i64 - 1)
            .div_euclid(sec)
            .min(self.ring_end.saturating_sub(1));
        let mut series = Vec::new();
        for s in &self.series {
            let Some(ring) = &s.ring else { continue };
            let mut col = Col::new(len);
            let mut any = false;
            for r in first..=last {
                let v = Self::ring_at(ring, self.ring_end, r);
                if !v.is_nan() {
                    let idx = (r * sec).div_euclid(step as i64);
                    col.add((idx - lo) as usize, v, v, v, 1);
                    any = true;
                }
            }
            if !any {
                continue;
            }
            let points: Arc<[MetricPoint]> = (0..len)
                .map(|i| {
                    let n = col.n[i];
                    let gap = n == 0;
                    MetricPoint {
                        t_realtime_ns: (lo + i as i64) * step as i64,
                        mean: if gap { f32::NAN } else { col.sum[i] / n as f32 },
                        min: if gap { f32::NAN } else { col.min[i] },
                        max: if gap { f32::NAN } else { col.max[i] },
                    }
                })
                .collect();
            series.push((s.key.clone(), points));
        }
        series.sort_by(|a, b| a.0.cmp(&b.0));
        MetricSpan {
            tier: Tier::S1,
            step_ns: step,
            series,
            events: Vec::new(),
        }
    }

    /// Start of the oldest stored segment of `tier` on CLOCK_REALTIME ("since 09:14").
    pub fn since_realtime_ns(&self, tier: Tier) -> Option<i64> {
        self.segs[tier.index()]
            .iter()
            .filter(|s| !s.frames.is_empty())
            .map(Seg::start_rt)
            .min()
    }

    /// Bytes of every indexed segment, all tiers.
    pub fn disk_bytes(&self) -> u64 {
        self.segs.iter().flatten().map(|s| s.size).sum()
    }

    /// Indexed segments of `tier`, oldest first: path, start and end on CLOCK_REALTIME.
    pub fn segments(&self, tier: Tier) -> Vec<(PathBuf, i64, i64)> {
        self.segs[tier.index()]
            .iter()
            .map(|s| (s.path.clone(), s.start_rt(), s.end_rt()))
            .collect()
    }

    /// Spans held in the cache.
    pub fn cached_spans(&self) -> usize {
        self.spans.len()
    }

    /// `history.device_metrics_max_mb` changed. It applies at the next write.
    pub fn set_max_disk_bytes(&mut self, max_disk_bytes: u64) {
        self.cfg.max_disk_bytes = max_disk_bytes;
    }
}

/// Deletes the whole `metrics/` directory: `history.device_metrics` was turned off. A missing
/// directory is no error.
pub fn purge(root: &Path) -> Result<(), HistoryError> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(root)(e)),
    }
}
