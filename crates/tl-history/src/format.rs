//! On-disk and in-RAM history types and the segment header codec (ADR-012, R-169 OI-2 and OI-3).
//!
//! Command lines are never stored (ADR-015): `HistMeta` has no cmdline field.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tl_model::ids::{DeviceIdx, ProcessKey, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::time::BootId;
use tl_model::validity::ProcessField;

use crate::error::HistoryError;

/// First 4 bytes of every segment file.
pub const MAGIC: [u8; 4] = *b"TLH\x01";
/// Last 4 bytes of a segment file with a footer.
pub const FOOTER_MAGIC: [u8; 4] = *b"TLHF";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 64;
/// Length of one block in seconds.
pub const BLOCK_SECS: u64 = 60;
/// Length of one segment file in seconds.
pub const SEGMENT_SECS: u64 = 600;
pub const ZSTD_LEVEL: i32 = 3;
/// `FieldPatch` thresholds (R-169 OI-3).
pub const CPU_THRESHOLD_PERMILLE: u16 = 1;
pub const MEM_THRESHOLD_KIB: u32 = 256;
pub const RATE_THRESHOLD_KIBPS: u32 = 1;

/// Process text id. Assigned from 0 per segment (store) or per window lifetime (rolling window).
pub type MetaId = u32;

/// One 60 s block of history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub start_t_ns: u64,
    /// First block after a suspend, resume or engine epoch change.
    pub gap_before: bool,
    /// MemTotal at block start.
    pub mem_total_kib: u64,
    /// Present in the first block of a segment and when the catalog changed.
    pub catalog: Option<Vec<HistDevice>>,
    /// Every meta id first referenced in this block.
    pub new_meta: Vec<(MetaId, HistMeta)>,
    /// Every live process at block start.
    pub keyframe: Vec<HistProcess>,
    /// One per process sweep in the block.
    pub deltas: Vec<SweepDelta>,
    /// 1 Hz system series: (ms offset from `start_t_ns`, value). NaN marks a gap.
    pub system: Vec<(SeriesId, Vec<(u32, f32)>)>,
}

/// Text of one process. No command line, ever (ADR-015).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistMeta {
    pub comm: Arc<str>,
    pub name: Arc<str>,
    pub exe_basename: Option<Arc<str>>,
    pub app_id: Option<Arc<str>>,
    pub exe_path: Option<Arc<str>>,
    pub user: Arc<str>,
    pub uid: u32,
    pub system: bool,
}

/// One device of the catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistDevice {
    pub idx: DeviceIdx,
    /// 0 CpuPackage, 1 CpuLogical, 2 Memory, 3 Gpu, 4 Disk, 5 Net, 6 Sensor, 7 Fan, 8 Battery.
    pub kind: u8,
    pub parent: Option<DeviceIdx>,
    pub name: Arc<str>,
}

/// One process at one point in time, in stored units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistProcess {
    pub key: ProcessKey,
    pub meta: MetaId,
    pub ppid: u32,
    pub cpu_permille: u16,
    pub mem_anon_kib: u32,
    pub io_r_kibps: u32,
    pub io_w_kibps: u32,
    pub net_kibps: u32,
    pub gpu_pct: u8,
}

/// Changes of one process sweep against the block keyframe.
///
/// Indexes in `changed` and `exited` point into the block's process list: the keyframe takes
/// `0..keyframe.len()` and each `started` entry appends the next index in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SweepDelta {
    /// Milliseconds from `Block.start_t_ns`.
    pub dt_ms: u32,
    pub changed: Vec<(u32, FieldPatch)>,
    pub started: Vec<HistProcess>,
    pub exited: Vec<u32>,
}

/// Fields of one process that crossed their threshold. `None` means unchanged.
///
/// On disk: one `u8` presence mask (bit 0 ppid, 1 cpu, 2 mem, 3 io_r, 4 io_w, 5 net, 6 gpu),
/// then only the present values in that order, as postcard writes them. A mask with bit 7 set
/// does not decode.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldPatch {
    pub ppid: Option<u32>,
    pub cpu_permille: Option<u16>,
    pub mem_anon_kib: Option<u32>,
    pub io_r_kibps: Option<u32>,
    pub io_w_kibps: Option<u32>,
    pub net_kibps: Option<u32>,
    pub gpu_pct: Option<u8>,
}

/// Mask bits of the `FieldPatch` fields, in field order.
const PATCH_PPID: u8 = 1 << 0;
const PATCH_CPU: u8 = 1 << 1;
const PATCH_MEM: u8 = 1 << 2;
const PATCH_IO_R: u8 = 1 << 3;
const PATCH_IO_W: u8 = 1 << 4;
const PATCH_NET: u8 = 1 << 5;
const PATCH_GPU: u8 = 1 << 6;
const PATCH_ALL: u8 = 0x7f;
/// The mask plus at most 7 values.
const PATCH_MAX_ELEMS: usize = 8;

impl FieldPatch {
    fn mask(&self) -> u8 {
        let bit = |present: bool, b: u8| if present { b } else { 0 };
        bit(self.ppid.is_some(), PATCH_PPID)
            | bit(self.cpu_permille.is_some(), PATCH_CPU)
            | bit(self.mem_anon_kib.is_some(), PATCH_MEM)
            | bit(self.io_r_kibps.is_some(), PATCH_IO_R)
            | bit(self.io_w_kibps.is_some(), PATCH_IO_W)
            | bit(self.net_kibps.is_some(), PATCH_NET)
            | bit(self.gpu_pct.is_some(), PATCH_GPU)
    }
}

impl Serialize for FieldPatch {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mask = self.mask();
        // postcard writes a tuple as its elements with no length.
        let mut t = s.serialize_tuple(1 + mask.count_ones() as usize)?;
        t.serialize_element(&mask)?;
        if let Some(v) = &self.ppid {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.cpu_permille {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.mem_anon_kib {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.io_r_kibps {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.io_w_kibps {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.net_kibps {
            t.serialize_element(v)?;
        }
        if let Some(v) = &self.gpu_pct {
            t.serialize_element(v)?;
        }
        t.end()
    }
}

impl<'de> Deserialize<'de> for FieldPatch {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct PatchVisitor;

        impl<'de> serde::de::Visitor<'de> for PatchVisitor {
            type Value = FieldPatch;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a FieldPatch presence mask and its values")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<FieldPatch, A::Error> {
                use serde::de::Error;
                let mask: u8 = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(0, &self))?;
                let mut read = 1usize;
                if mask & !PATCH_ALL != 0 {
                    return Err(A::Error::custom("FieldPatch mask has bit 7 set"));
                }
                fn field<'de, A: serde::de::SeqAccess<'de>, T: Deserialize<'de>>(
                    seq: &mut A,
                    mask: u8,
                    bit: u8,
                    read: &mut usize,
                    exp: &PatchVisitor,
                ) -> Result<Option<T>, A::Error> {
                    if mask & bit == 0 {
                        return Ok(None);
                    }
                    let v = seq.next_element()?;
                    let v = v.ok_or_else(|| serde::de::Error::invalid_length(*read, exp))?;
                    *read += 1;
                    Ok(Some(v))
                }
                Ok(FieldPatch {
                    ppid: field(&mut seq, mask, PATCH_PPID, &mut read, &self)?,
                    cpu_permille: field(&mut seq, mask, PATCH_CPU, &mut read, &self)?,
                    mem_anon_kib: field(&mut seq, mask, PATCH_MEM, &mut read, &self)?,
                    io_r_kibps: field(&mut seq, mask, PATCH_IO_R, &mut read, &self)?,
                    io_w_kibps: field(&mut seq, mask, PATCH_IO_W, &mut read, &self)?,
                    net_kibps: field(&mut seq, mask, PATCH_NET, &mut read, &self)?,
                    gpu_pct: field(&mut seq, mask, PATCH_GPU, &mut read, &self)?,
                })
            }
        }

        // The length is an upper bound: the visitor reads only the values the mask names.
        d.deserialize_tuple(PATCH_MAX_ELEMS, PatchVisitor)
    }
}

/// One footer entry: a block start and the byte offset of its length field from file start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FooterEntry {
    pub start_t_ns: u64,
    pub offset: u64,
}

/// The fixed 64-byte header of a segment file, little endian.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegmentHeader {
    pub boot_id: BootId,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME at segment start.
    pub realtime_offset_ns: i64,
    pub start_t_ns: u64,
}

impl SegmentHeader {
    /// Bytes 0..4 magic, 4..6 version, 6..14 boot id, 14..22 realtime offset, 22..30 start,
    /// 30..64 zero.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        b[6..14].copy_from_slice(&self.boot_id.0.to_le_bytes());
        b[14..22].copy_from_slice(&self.realtime_offset_ns.to_le_bytes());
        b[22..30].copy_from_slice(&self.start_t_ns.to_le_bytes());
        b
    }

    /// Wrong length or magic gives `BadHeader`, a version other than 1 gives `Version(v)`.
    /// Bytes 30..64 are ignored.
    pub fn decode(b: &[u8]) -> Result<SegmentHeader, HistoryError> {
        if b.len() != HEADER_LEN || b[0..4] != MAGIC {
            return Err(HistoryError::BadHeader);
        }
        let version = u16::from_le_bytes([b[4], b[5]]);
        if version != FORMAT_VERSION {
            return Err(HistoryError::Version(version));
        }
        let u64_at = |i: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[i..i + 8]);
            a
        };
        Ok(SegmentHeader {
            boot_id: BootId(u64::from_le_bytes(u64_at(6))),
            realtime_offset_ns: i64::from_le_bytes(u64_at(14)),
            start_t_ns: u64::from_le_bytes(u64_at(22)),
        })
    }
}

/// `seg-<start boottime seconds, 10 digits>.tlh`.
pub fn segment_file_name(start_t_ns: u64) -> String {
    format!("seg-{:010}.tlh", start_t_ns / 1_000_000_000)
}

/// The boot id as 16 lowercase hex digits.
pub fn boot_dir_name(boot: BootId) -> String {
    format!("{:016x}", boot.0)
}

/// Converts a record to stored units. A field whose validity is not `Valid` is stored as 0.
pub fn hist_process(rec: &ProcessRecord, meta: MetaId) -> HistProcess {
    let valid = |f: ProcessField| rec.validity.get(f).is_valid();
    let or_zero = |f: ProcessField, v: f32| if valid(f) { v } else { 0.0 };
    // `as` saturates and maps NaN to 0.
    let kibps = |bps: f32| (bps / 1024.0).round() as u32;
    let cpu = or_zero(ProcessField::Cpu, rec.cpu_pct);
    let gpu = or_zero(ProcessField::GpuUtil, rec.gpu_util_pct);
    let mem_kib = if valid(ProcessField::RssAnon) {
        rec.rss_anon_bytes / 1024
    } else {
        0
    };
    HistProcess {
        key: rec.key,
        meta,
        ppid: rec.ppid,
        cpu_permille: ((cpu * 10.0).round() as u16).min(1000),
        mem_anon_kib: u32::try_from(mem_kib).unwrap_or(u32::MAX),
        io_r_kibps: kibps(or_zero(ProcessField::IoRead, rec.io_read_bps)),
        io_w_kibps: kibps(or_zero(ProcessField::IoWrite, rec.io_write_bps)),
        net_kibps: kibps(
            or_zero(ProcessField::NetRx, rec.net_rx_bps)
                + or_zero(ProcessField::NetTx, rec.net_tx_bps),
        ),
        gpu_pct: (gpu.round() as u8).min(100),
    }
}

impl FieldPatch {
    /// The fields of `cur` that crossed their threshold against the last stored value, or `None`
    /// when no field did.
    pub fn diff(stored: &HistProcess, cur: &HistProcess) -> Option<FieldPatch> {
        fn by<T: Copy + PartialEq>(stored: T, cur: T, crossed: bool) -> Option<T> {
            (stored != cur && crossed).then_some(cur)
        }
        let rate = |s: u32, c: u32| {
            by(
                s,
                c,
                s.abs_diff(c) >= RATE_THRESHOLD_KIBPS || (s == 0) != (c == 0),
            )
        };
        let patch = FieldPatch {
            ppid: by(stored.ppid, cur.ppid, true),
            cpu_permille: by(
                stored.cpu_permille,
                cur.cpu_permille,
                stored.cpu_permille.abs_diff(cur.cpu_permille) >= CPU_THRESHOLD_PERMILLE,
            ),
            mem_anon_kib: by(
                stored.mem_anon_kib,
                cur.mem_anon_kib,
                stored.mem_anon_kib.abs_diff(cur.mem_anon_kib) >= MEM_THRESHOLD_KIB,
            ),
            io_r_kibps: rate(stored.io_r_kibps, cur.io_r_kibps),
            io_w_kibps: rate(stored.io_w_kibps, cur.io_w_kibps),
            net_kibps: rate(stored.net_kibps, cur.net_kibps),
            gpu_pct: by(stored.gpu_pct, cur.gpu_pct, true),
        };
        let empty = patch.ppid.is_none()
            && patch.cpu_permille.is_none()
            && patch.mem_anon_kib.is_none()
            && patch.io_r_kibps.is_none()
            && patch.io_w_kibps.is_none()
            && patch.net_kibps.is_none()
            && patch.gpu_pct.is_none();
        (!empty).then_some(patch)
    }

    /// Writes every `Some` field into `p`.
    pub fn apply(&self, p: &mut HistProcess) {
        if let Some(v) = self.ppid {
            p.ppid = v;
        }
        if let Some(v) = self.cpu_permille {
            p.cpu_permille = v;
        }
        if let Some(v) = self.mem_anon_kib {
            p.mem_anon_kib = v;
        }
        if let Some(v) = self.io_r_kibps {
            p.io_r_kibps = v;
        }
        if let Some(v) = self.io_w_kibps {
            p.io_w_kibps = v;
        }
        if let Some(v) = self.net_kibps {
            p.net_kibps = v;
        }
        if let Some(v) = self.gpu_pct {
            p.gpu_pct = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::validity::{FieldValidity, Validity};

    const NONE: FieldPatch = FieldPatch {
        ppid: None,
        cpu_permille: None,
        mem_anon_kib: None,
        io_r_kibps: None,
        io_w_kibps: None,
        net_kibps: None,
        gpu_pct: None,
    };

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 100,
            boot_id: BootId(7),
            pid: 42,
        }
    }

    fn proc(cpu_permille: u16, mem_anon_kib: u32) -> HistProcess {
        HistProcess {
            key: key(),
            meta: 0,
            ppid: 1,
            cpu_permille,
            mem_anon_kib,
            io_r_kibps: 0,
            io_w_kibps: 0,
            net_kibps: 0,
            gpu_pct: 0,
        }
    }

    fn record() -> ProcessRecord {
        let text: Arc<str> = Arc::from("x");
        ProcessRecord {
            key: key(),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text.clone(),
                name: text.clone(),
                exe: None,
                cmdline: None,
                user: text.clone(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: text,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::default(),
            nice: 0,
            threads: 1,
            cpu_pct: 12.34,
            rss_anon_bytes: 2_097_152,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 1024.0,
            net_tx_bps: 1024.0,
            gpu_util_pct: 41.6,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn header() -> SegmentHeader {
        SegmentHeader {
            boot_id: BootId(0x0123_4567_89ab_cdef),
            realtime_offset_ns: -5,
            start_t_ns: 42,
        }
    }

    #[test]
    fn header_roundtrip() {
        let b = header().encode();
        assert_eq!(&b[0..4], b"TLH\x01");
        assert_eq!(&b[4..6], &[1, 0]);
        assert!(b[30..64].iter().all(|&x| x == 0));
        assert_eq!(SegmentHeader::decode(&b).unwrap(), header());

        // Reserved bytes are ignored.
        let mut b = b;
        b[40] = 0xff;
        assert_eq!(SegmentHeader::decode(&b).unwrap(), header());
    }

    #[test]
    fn header_bad_magic() {
        let mut b = header().encode();
        b[3] = 0x02;
        assert!(matches!(
            SegmentHeader::decode(&b),
            Err(HistoryError::BadHeader)
        ));
        let b = header().encode();
        assert!(matches!(
            SegmentHeader::decode(&b[..63]),
            Err(HistoryError::BadHeader)
        ));
    }

    #[test]
    fn header_bad_version() {
        let mut b = header().encode();
        b[4] = 2;
        b[5] = 0;
        assert!(matches!(
            SegmentHeader::decode(&b),
            Err(HistoryError::Version(2))
        ));
    }

    #[test]
    fn segment_name_format() {
        assert_eq!(segment_file_name(12_345_000_000_000), "seg-0000012345.tlh");
        assert_eq!(segment_file_name(999_999_999), "seg-0000000000.tlh");
    }

    #[test]
    fn boot_dir_name_is_16_lower_hex() {
        assert_eq!(
            boot_dir_name(BootId(0x0123_4567_89AB_CDEF)),
            "0123456789abcdef"
        );
        assert_eq!(boot_dir_name(BootId(0xab)), "00000000000000ab");
    }

    #[test]
    fn patch_thresholds_cpu_mem_rates() {
        // CPU unchanged, memory up 255 KiB: below every threshold.
        assert_eq!(FieldPatch::diff(&proc(100, 1000), &proc(100, 1255)), None);

        let patch = FieldPatch::diff(&proc(100, 1000), &proc(100, 1256)).unwrap();
        assert_eq!(
            patch,
            FieldPatch {
                mem_anon_kib: Some(1256),
                ..NONE
            }
        );

        let patch = FieldPatch::diff(&proc(100, 1000), &proc(101, 1000)).unwrap();
        assert_eq!(
            patch,
            FieldPatch {
                cpu_permille: Some(101),
                ..NONE
            }
        );

        let mut cur = proc(100, 1000);
        cur.io_w_kibps = 0;
        cur.net_kibps = 0;
        let mut stored = cur.clone();
        stored.io_w_kibps = 10;
        cur.io_w_kibps = 11;
        cur.ppid = 2;
        cur.gpu_pct = 5;
        let patch = FieldPatch::diff(&stored, &cur).unwrap();
        assert_eq!(
            patch,
            FieldPatch {
                ppid: Some(2),
                io_w_kibps: Some(11),
                gpu_pct: Some(5),
                ..NONE
            }
        );
    }

    #[test]
    fn patch_to_and_from_zero() {
        let stored = proc(0, 0);
        let mut cur = stored.clone();
        cur.io_r_kibps = 1;
        let patch = FieldPatch::diff(&stored, &cur).unwrap();
        assert_eq!(patch.io_r_kibps, Some(1));

        let patch = FieldPatch::diff(&cur, &stored).unwrap();
        assert_eq!(
            patch,
            FieldPatch {
                io_r_kibps: Some(0),
                ..NONE
            }
        );
    }

    #[test]
    fn patch_apply_roundtrip() {
        let stored = proc(100, 1000);
        let mut cur = proc(250, 4096);
        cur.ppid = 9;
        cur.io_r_kibps = 3;
        cur.io_w_kibps = 4;
        cur.net_kibps = 5;
        cur.gpu_pct = 60;
        let patch = FieldPatch::diff(&stored, &cur).unwrap();
        let mut p = stored.clone();
        patch.apply(&mut p);
        assert_eq!(p, cur);

        // The patch survives postcard.
        let bytes = postcard::to_allocvec(&patch).unwrap();
        assert_eq!(postcard::from_bytes::<FieldPatch>(&bytes).unwrap(), patch);
    }

    #[test]
    fn hist_process_conversion() {
        let p = hist_process(&record(), 3);
        assert_eq!(p.key, key());
        assert_eq!(p.meta, 3);
        assert_eq!(p.ppid, 1);
        assert_eq!(p.cpu_permille, 123);
        assert_eq!(p.mem_anon_kib, 2048);
        assert_eq!(p.net_kibps, 2);
        assert_eq!(p.gpu_pct, 42);
        assert_eq!(p.io_r_kibps, 0);

        // Clamped to 0..=1000 and 0..=100.
        let mut rec = record();
        rec.cpu_pct = 150.0;
        rec.gpu_util_pct = 250.0;
        let p = hist_process(&rec, 0);
        assert_eq!(p.cpu_permille, 1000);
        assert_eq!(p.gpu_pct, 100);

        // A field that is not Valid is stored as 0.
        let mut rec = record();
        rec.validity.set(ProcessField::Cpu, Validity::Restricted);
        rec.validity.set(ProcessField::RssAnon, Validity::WarmingUp);
        rec.validity.set(ProcessField::NetTx, Validity::Stale);
        let p = hist_process(&rec, 0);
        assert_eq!(p.cpu_permille, 0);
        assert_eq!(p.mem_anon_kib, 0);
        assert_eq!(p.net_kibps, 1);
        assert_eq!(p.gpu_pct, 42);
    }

    /// The patch with exactly the fields of `mask` present, each at value `v` cut to its type.
    fn patch_of(mask: u8, v: u32) -> FieldPatch {
        let on = |bit: u8| mask & bit != 0;
        FieldPatch {
            ppid: on(PATCH_PPID).then_some(v),
            cpu_permille: on(PATCH_CPU).then_some(v as u16),
            mem_anon_kib: on(PATCH_MEM).then_some(v),
            io_r_kibps: on(PATCH_IO_R).then_some(v),
            io_w_kibps: on(PATCH_IO_W).then_some(v),
            net_kibps: on(PATCH_NET).then_some(v),
            gpu_pct: on(PATCH_GPU).then_some(v as u8),
        }
    }

    #[test]
    fn patch_mask_roundtrip_all_128() {
        for mask in 0..=PATCH_ALL {
            for v in [0, 1, 300, u32::MAX] {
                let p = patch_of(mask, v);
                assert_eq!(p.mask(), mask);
                let bytes = postcard::to_stdvec(&p).unwrap();
                assert_eq!(bytes[0], mask);
                assert_eq!(postcard::from_bytes::<FieldPatch>(&bytes).unwrap(), p);
                // Inside a list the next entry starts right after the present values.
                let list = vec![(7u32, p.clone()), (9, patch_of(PATCH_ALL ^ mask, v))];
                let bytes = postcard::to_stdvec(&list).unwrap();
                let back: Vec<(u32, FieldPatch)> = postcard::from_bytes(&bytes).unwrap();
                assert_eq!(back, list);
            }
        }
    }

    #[test]
    fn patch_mask_writes_only_present_values() {
        assert_eq!(postcard::to_stdvec(&NONE).unwrap(), vec![0]);
        let p = FieldPatch {
            cpu_permille: Some(5),
            mem_anon_kib: Some(300),
            ..NONE
        };
        // Mask, cpu 5 (1 byte), mem 300 (2-byte varint).
        assert_eq!(postcard::to_stdvec(&p).unwrap(), vec![0b110, 5, 0xac, 0x02]);
    }

    #[test]
    fn patch_bad_mask_or_short_input_fails() {
        assert!(postcard::from_bytes::<FieldPatch>(&[0x80]).is_err());
        assert!(postcard::from_bytes::<FieldPatch>(&[0xff, 1, 1, 1, 1, 1, 1, 1]).is_err());
        assert!(postcard::from_bytes::<FieldPatch>(&[]).is_err());
        // Mask names cpu and mem, input ends after cpu.
        assert!(postcard::from_bytes::<FieldPatch>(&[0b110, 5]).is_err());
    }

    proptest::proptest! {
        #[test]
        fn patch_roundtrip_any(
            fields in (
                proptest::option::of(proptest::prelude::any::<u32>()),
                proptest::option::of(proptest::prelude::any::<u16>()),
                proptest::option::of(proptest::prelude::any::<u32>()),
                proptest::option::of(proptest::prelude::any::<u32>()),
                proptest::option::of(proptest::prelude::any::<u32>()),
                proptest::option::of(proptest::prelude::any::<u32>()),
                proptest::option::of(proptest::prelude::any::<u8>()),
            ),
            index in proptest::prelude::any::<u32>(),
        ) {
            let (ppid, cpu_permille, mem_anon_kib, io_r_kibps, io_w_kibps, net_kibps, gpu_pct) =
                fields;
            let p = FieldPatch {
                ppid,
                cpu_permille,
                mem_anon_kib,
                io_r_kibps,
                io_w_kibps,
                net_kibps,
                gpu_pct,
            };
            let list = vec![(index, p.clone()), (index, NONE), (0, p)];
            let bytes = postcard::to_stdvec(&list).unwrap();
            let back: Vec<(u32, FieldPatch)> = postcard::from_bytes(&bytes).unwrap();
            proptest::prop_assert_eq!(back, list);
        }
    }
}
