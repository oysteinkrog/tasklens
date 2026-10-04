//! Block frame codec: roundtrip, corrupt input and arbitrary bytes (ADR-012, ADR-013).

use std::sync::Arc;

use proptest::prelude::*;
use tl_history::HistoryError;
use tl_history::block::{FRAME_PREFIX_LEN, decode_block, encode_block};
use tl_history::format::{Block, FieldPatch, HistDevice, HistMeta, HistProcess, SweepDelta};
use tl_model::ids::{DeviceIdx, MetricId, ProcessKey, SeriesId};
use tl_model::time::BootId;

fn meta(i: u32) -> HistMeta {
    HistMeta {
        comm: Arc::from(format!("comm{i}")),
        name: Arc::from(format!("Name {i}")),
        exe_basename: (i % 2 == 0).then(|| Arc::from(format!("exe{i}"))),
        app_id: None,
        exe_path: Some(Arc::from(format!("/usr/bin/exe{i}"))),
        user: Arc::from("alice"),
        uid: 1000 + i,
        system: i == 0,
    }
}

fn process(i: u32) -> HistProcess {
    HistProcess {
        key: ProcessKey {
            start_ticks: 1000 + u64::from(i),
            boot_id: BootId(0xfeed),
            pid: 100 + i,
        },
        meta: i % 3,
        ppid: 1,
        cpu_permille: (i * 7 % 1001) as u16,
        mem_anon_kib: i * 4096,
        io_r_kibps: i,
        io_w_kibps: i * 2,
        net_kibps: i % 5,
        gpu_pct: (i % 101) as u8,
    }
}

/// 3 metas, 50 keyframe processes, 60 deltas and 2 system series.
fn sample_block() -> Block {
    let deltas = (0..60u32)
        .map(|d| SweepDelta {
            dt_ms: d * 1000,
            changed: vec![(
                d % 50,
                FieldPatch {
                    ppid: None,
                    cpu_permille: Some((d * 3) as u16),
                    mem_anon_kib: None,
                    io_r_kibps: Some(d),
                    io_w_kibps: None,
                    net_kibps: None,
                    gpu_pct: Some(1),
                },
            )],
            started: if d % 20 == 0 {
                vec![process(50 + d)]
            } else {
                vec![]
            },
            exited: if d % 25 == 1 { vec![d] } else { vec![] },
        })
        .collect();
    let series = |metric: u16| {
        (
            SeriesId {
                device: DeviceIdx(0),
                metric: MetricId(metric),
            },
            (0..60u32).map(|s| (s * 1000, s as f32 * 1.5)).collect(),
        )
    };
    Block {
        start_t_ns: 600_000_000_000,
        gap_before: false,
        mem_total_kib: 65_746_000,
        catalog: Some(vec![HistDevice {
            idx: DeviceIdx(0),
            kind: 0,
            parent: None,
            name: Arc::from("cpu"),
        }]),
        new_meta: (0..3).map(|i| (i, meta(i))).collect(),
        keyframe: (0..50).map(process).collect(),
        deltas,
        system: vec![series(1), series(2)],
    }
}

/// Builds a frame with a correct crc around `payload`, which need not be valid zstd.
fn frame_with_crc(payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

#[test]
fn block_roundtrip() {
    let b = sample_block();
    let enc = encode_block(&b).unwrap();
    assert_eq!(enc.start_t_ns, b.start_t_ns);
    let len = u32::from_le_bytes(enc.bytes[0..4].try_into().unwrap()) as usize;
    assert_eq!(len + FRAME_PREFIX_LEN, enc.bytes.len());

    let (dec, consumed) = decode_block(&enc.bytes, 64).unwrap();
    assert_eq!(dec, b);
    assert_eq!(consumed, enc.bytes.len());

    // Bytes after the frame are not consumed.
    let mut two = enc.bytes.clone();
    two.extend_from_slice(&enc.bytes);
    let (dec, consumed) = decode_block(&two, 64).unwrap();
    assert_eq!(dec, b);
    assert_eq!(consumed, enc.bytes.len());
}

#[test]
fn crc_mismatch_detected() {
    let mut bytes = encode_block(&sample_block()).unwrap().bytes;
    let mid = FRAME_PREFIX_LEN + (bytes.len() - FRAME_PREFIX_LEN) / 2;
    bytes[mid] ^= 0x01;
    assert!(matches!(decode_block(&bytes, 64), Err(HistoryError::Crc(64))));

    // A flipped crc byte is a mismatch too.
    let mut bytes = encode_block(&sample_block()).unwrap().bytes;
    bytes[4] ^= 0x80;
    assert!(matches!(decode_block(&bytes, 64), Err(HistoryError::Crc(64))));
}

#[test]
fn truncated_detected() {
    let bytes = encode_block(&sample_block()).unwrap().bytes;
    assert!(matches!(
        decode_block(&bytes[..bytes.len() - 1], 128),
        Err(HistoryError::Truncated(128))
    ));
    // A prefix shorter than 8 bytes.
    for n in 0..FRAME_PREFIX_LEN {
        assert!(matches!(
            decode_block(&bytes[..n], 72),
            Err(HistoryError::Truncated(72))
        ));
    }
    // A length field claiming 4 GiB.
    let mut huge = bytes.clone();
    huge[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        decode_block(&huge, 0),
        Err(HistoryError::Truncated(0))
    ));
}

#[test]
fn bad_zstd_with_good_crc_is_zstd_error() {
    let frame = frame_with_crc(b"not a zstd frame at all");
    assert!(matches!(decode_block(&frame, 0), Err(HistoryError::Zstd(_))));
}

#[test]
fn oversized_block_is_zstd_error() {
    // 64 MiB + 1 zero bytes compress to a few KiB; the frame header states the full size.
    let raw = vec![0u8; (64 << 20) + 1];
    let compressed = zstd::bulk::compress(&raw, 3).unwrap();
    let frame = frame_with_crc(&compressed);
    assert!(matches!(decode_block(&frame, 0), Err(HistoryError::Zstd(_))));
}

#[test]
fn valid_zstd_of_garbage_is_decode_error() {
    let compressed = zstd::bulk::compress(&[0xff; 16], 3).unwrap();
    let frame = frame_with_crc(&compressed);
    assert!(matches!(
        decode_block(&frame, 0),
        Err(HistoryError::Decode(_))
    ));
}

fn arb_process() -> impl Strategy<Value = HistProcess> {
    (
        any::<(u64, u64, u32, u32, u32)>(),
        any::<(u16, u32, u32, u32, u32, u8)>(),
    )
        .prop_map(
            |((ticks, boot, pid, meta, ppid), (cpu, mem, r, w, net, gpu))| HistProcess {
                key: ProcessKey {
                    start_ticks: ticks,
                    boot_id: BootId(boot),
                    pid,
                },
                meta,
                ppid,
                cpu_permille: cpu,
                mem_anon_kib: mem,
                io_r_kibps: r,
                io_w_kibps: w,
                net_kibps: net,
                gpu_pct: gpu,
            },
        )
}

fn arb_patch() -> impl Strategy<Value = FieldPatch> {
    any::<(
        Option<u32>,
        Option<u16>,
        Option<u32>,
        Option<u32>,
        Option<u32>,
        Option<u32>,
        Option<u8>,
    )>()
    .prop_map(|(ppid, cpu, mem, r, w, net, gpu)| FieldPatch {
        ppid,
        cpu_permille: cpu,
        mem_anon_kib: mem,
        io_r_kibps: r,
        io_w_kibps: w,
        net_kibps: net,
        gpu_pct: gpu,
    })
}

fn arb_text() -> impl Strategy<Value = Arc<str>> {
    "[a-zA-Z0-9 ._/-]{0,16}".prop_map(Arc::from)
}

fn arb_meta() -> impl Strategy<Value = HistMeta> {
    (
        (arb_text(), arb_text(), proptest::option::of(arb_text())),
        (
            proptest::option::of(arb_text()),
            proptest::option::of(arb_text()),
            arb_text(),
            any::<u32>(),
            any::<bool>(),
        ),
    )
        .prop_map(
            |((comm, name, exe_basename), (app_id, exe_path, user, uid, system))| HistMeta {
                comm,
                name,
                exe_basename,
                app_id,
                exe_path,
                user,
                uid,
                system,
            },
        )
}

fn arb_delta() -> impl Strategy<Value = SweepDelta> {
    (
        any::<u32>(),
        proptest::collection::vec((any::<u32>(), arb_patch()), 0..4),
        proptest::collection::vec(arb_process(), 0..3),
        proptest::collection::vec(any::<u32>(), 0..3),
    )
        .prop_map(|(dt_ms, changed, started, exited)| SweepDelta {
            dt_ms,
            changed,
            started,
            exited,
        })
}

fn arb_block() -> impl Strategy<Value = Block> {
    let device = (
        any::<u16>(),
        any::<u8>(),
        proptest::option::of(any::<u16>()),
        arb_text(),
    )
        .prop_map(|(idx, kind, parent, name)| HistDevice {
            idx: DeviceIdx(idx),
            kind,
            parent: parent.map(DeviceIdx),
            name,
        });
    // Finite values only: NaN never equals itself, so it would fail the equality check.
    let series = (
        any::<(u16, u16)>(),
        proptest::collection::vec((any::<u32>(), -1.0e9f32..1.0e9f32), 0..8),
    )
        .prop_map(|((device, metric), points)| {
            (
                SeriesId {
                    device: DeviceIdx(device),
                    metric: MetricId(metric),
                },
                points,
            )
        });
    (
        (any::<u64>(), any::<bool>(), any::<u64>()),
        proptest::option::of(proptest::collection::vec(device, 0..4)),
        proptest::collection::vec((any::<u32>(), arb_meta()), 0..4),
        proptest::collection::vec(arb_process(), 0..8),
        proptest::collection::vec(arb_delta(), 0..6),
        proptest::collection::vec(series, 0..3),
    )
        .prop_map(
            |((start_t_ns, gap_before, mem_total_kib), catalog, new_meta, keyframe, deltas, system)| {
                Block {
                    start_t_ns,
                    gap_before,
                    mem_total_kib,
                    catalog,
                    new_meta,
                    keyframe,
                    deltas,
                    system,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn prop_decode_arbitrary_bytes_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..=4096),
        offset in any::<u64>(),
    ) {
        let _ = decode_block(&bytes, offset);
        // The same bytes behind a correct prefix reach zstd and postcard.
        let _ = decode_block(&frame_with_crc(&bytes), offset);
    }

    #[test]
    fn prop_block_roundtrip(b in arb_block()) {
        let enc = encode_block(&b).unwrap();
        let (dec, consumed) = decode_block(&enc.bytes, 0).unwrap();
        prop_assert_eq!(dec, b);
        prop_assert_eq!(consumed, enc.bytes.len());
    }
}
