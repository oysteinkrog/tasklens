//! Postcard round trips through `wire` frames for every serializable message of the crate: the
//! GPU probe, the privileged helper and the serde types of `actions.rs` (E03-070).

use std::fmt::Debug;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tl_model::actions::{
    ActionId, DumpMode, Family, FourTuple, IoPriority, Proto, SchedPolicy, Signal,
};
use tl_model::details::{StringEncoding, StringScanOpts};
use tl_model::gpu_proto::{
    ApiVersions, GpuApiVersions, GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure,
    ProbeRequest, ProbeResponse, Reading,
};
use tl_model::helper_proto::{
    DropInKey, HELPER_PROTO_VERSION, HelperCgroup, HelperError, HelperFrame, HelperReply,
    HelperReplyBody, HelperRequest, HelperTarget, ProcFiles,
};
use tl_model::ids::ProcessKey;
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_model::wire::{
    MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, MAX_PROBE_FRAME, encode_frame, read_frame, read_reply,
};

/// Frames `msg`, reads it back and checks it is equal and that nothing is left over.
fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + Debug>(msg: &T, max: usize) {
    let mut buf = Vec::new();
    encode_frame(msg, max, &mut buf).expect("encode");
    let mut reader = buf.as_slice();
    let back: T = read_frame(&mut reader, max).expect("decode");
    assert_eq!(&back, msg);
    assert!(reader.is_empty(), "{msg:?} left bytes unread");
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 123_456_789,
        boot_id: BootId(0xDEAD_BEEF_0123_4567),
        pid,
    }
}

fn cgroup() -> HelperCgroup {
    HelperCgroup {
        rel_path: "user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox.scope"
            .to_string(),
        cgroup_id: 4242,
    }
}

fn four_tuple_v6() -> FourTuple {
    FourTuple {
        local: SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 2, 3, 4),
            443,
            0x0001_2345,
            3,
        )),
        remote: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 50_000, 0, 0)),
    }
}

fn four_tuple_v6_global() -> FourTuple {
    FourTuple {
        local: SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            443,
            0,
            0,
        )),
        remote: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 50_000, 0, 0)),
    }
}

fn four_tuple_v4() -> FourTuple {
    FourTuple {
        local: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 2), 22)),
        remote: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 7), 61_000)),
    }
}

fn readings<T: Copy>(v: T) -> [Reading<T>; 3] {
    [Reading::Ok(v), Reading::Unsupported, Reading::Failed]
}

fn device_sample(r32: Reading<u32>, r64: Reading<u64>) -> GpuDeviceSample {
    GpuDeviceSample {
        uuid: [7; 16],
        util_pct: r32,
        mem_util_pct: r32,
        mem_used: r64,
        mem_reserved: r64,
        enc_pct: r32,
        dec_pct: r32,
        enc_sessions: r32,
        temp_c: r32,
        power_mw: r32,
        clock_gfx_mhz: r32,
        clock_mem_mhz: r32,
        fan_pct: r32,
        pcie_rx_kbps: r32,
        pcie_tx_kbps: r32,
    }
}

#[test]
fn probe_messages_roundtrip() {
    let requests = [
        ProbeRequest::Hello { proto: 1 },
        ProbeRequest::Sample {
            seq: 9,
            processes: true,
        },
        ProbeRequest::Sample {
            seq: u32::MAX,
            processes: false,
        },
        ProbeRequest::Shutdown,
    ];
    for msg in &requests {
        roundtrip(msg, MAX_PROBE_FRAME);
    }

    roundtrip(
        &ProbeResponse::Hello {
            proto: 1,
            driver: "580.82.09".to_string(),
            nvml: "12.580.82.09".to_string(),
            devices: vec![GpuDeviceInfo {
                uuid: [1; 16],
                name: "NVIDIA GeForce RTX 4080".to_string(),
                pci_bus_id: "00000000:01:00.0".to_string(),
                mem_total: 17_171_480_576,
                power_limit_mw: Some(320_000),
                cuda_version: Some("13.0".to_string()),
                vbios: None,
                architecture: Some("Ada Lovelace".to_string()),
            }],
        },
        MAX_PROBE_FRAME,
    );

    for kind in [
        InitFailure::LibraryNotFound,
        InitFailure::DriverNotLoaded,
        InitFailure::NoPermission,
        InitFailure::Other,
    ] {
        roundtrip(
            &ProbeResponse::InitFailed {
                kind,
                message: "nvmlInit failed".to_string(),
            },
            MAX_PROBE_FRAME,
        );
    }

    for (r32, r64) in readings(55u32).into_iter().zip(readings(1u64 << 40)) {
        roundtrip(
            &ProbeResponse::Sample {
                seq: 3,
                t_ns: 1_000_000_007,
                devices: vec![device_sample(r32, r64)],
                processes: Some(vec![GpuProcessSample {
                    uuid: [7; 16],
                    pid: 4321,
                    sm_pct: r32,
                    mem_pct: r32,
                    enc_pct: r32,
                    dec_pct: r32,
                    mem_used: r64,
                }]),
            },
            MAX_PROBE_FRAME,
        );
    }
    roundtrip(
        &ProbeResponse::Sample {
            seq: 4,
            t_ns: 0,
            devices: Vec::new(),
            processes: None,
        },
        MAX_PROBE_FRAME,
    );
}

#[test]
fn api_versions_roundtrip() {
    roundtrip(
        &ApiVersions {
            devices: vec![
                GpuApiVersions {
                    pci_bus_id: "0000:01:00.0".to_string(),
                    vulkan_api: Some("1.4.312".to_string()),
                    vulkan_driver: Some("580.82.09".to_string()),
                    opengl: Some("4.6.0 NVIDIA 580.82.09".to_string()),
                },
                GpuApiVersions {
                    pci_bus_id: "0000:0c:00.0".to_string(),
                    vulkan_api: None,
                    vulkan_driver: None,
                    opengl: None,
                },
            ],
        },
        MAX_PROBE_FRAME,
    );
    roundtrip(
        &ApiVersions {
            devices: Vec::new(),
        },
        MAX_PROBE_FRAME,
    );
}

/// One request of each of the 17 variants.
fn every_request() -> Vec<HelperRequest> {
    let mut set = CpuSet::empty();
    set.insert(0);
    set.insert(1023);
    vec![
        HelperRequest::Hello {
            proto: HELPER_PROTO_VERSION,
        },
        HelperRequest::Signal {
            target: HelperTarget::Process(key(100)),
            signal: Signal::Other(34),
        },
        HelperRequest::SetNice {
            target: HelperTarget::Thread {
                process: key(100),
                tid: 101,
            },
            nice: -20,
        },
        HelperRequest::SetSched {
            target: HelperTarget::Process(key(100)),
            policy: SchedPolicy::Fifo,
            rt_prio: 99,
        },
        HelperRequest::SetAffinity {
            target: HelperTarget::Thread {
                process: key(100),
                tid: 102,
            },
            set,
        },
        HelperRequest::SetIoPriority {
            key: key(100),
            prio: IoPriority::BestEffort(4),
        },
        HelperRequest::FreezeGroup(cgroup()),
        HelperRequest::ThawGroup(cgroup()),
        HelperRequest::KillGroup(cgroup()),
        HelperRequest::ReadProcessFiles {
            key: key(100),
            files: ProcFiles::ENVIRON | ProcFiles::SMAPS | ProcFiles::ROOT,
        },
        HelperRequest::ScanStrings {
            key: key(100),
            opts: StringScanOpts {
                min_len: 8,
                ascii: true,
                utf8: false,
                utf16le: true,
                include_file_backed: true,
            },
        },
        HelperRequest::Dump {
            key: key(100),
            mode: DumpMode::Full,
        },
        HelperRequest::ResolveSocketInodes {
            inodes: vec![1, 77_123, u64::MAX],
        },
        HelperRequest::SockDestroy {
            family: Family::Inet6,
            proto: Proto::Tcp,
            four_tuple: four_tuple_v6(),
            cookie: 0x0102_0304_0506_0708,
        },
        HelperRequest::WriteServiceDropIn {
            unit: "sshd.service".to_string(),
            keys: vec![
                (DropInKey::Restart, "on-failure".to_string()),
                (DropInKey::RestartSec, "5s".to_string()),
                (DropInKey::OnFailure, "notify@%n.service".to_string()),
            ],
        },
        HelperRequest::NetnsSockDiagDump {
            netns_ino: 4_026_531_840,
        },
        HelperRequest::CancelScan { seq: 11 },
    ]
}

#[test]
fn helper_requests_roundtrip() {
    let requests = every_request();
    assert_eq!(requests.len(), 17);
    for (seq, request) in requests.into_iter().enumerate() {
        roundtrip(
            &HelperFrame {
                seq: seq as u32,
                request,
            },
            MAX_HELPER_FRAME,
        );
    }
    roundtrip(
        &HelperFrame {
            seq: 0,
            request: HelperRequest::WriteServiceDropIn {
                unit: "sshd.service".to_string(),
                keys: Vec::new(),
            },
        },
        MAX_HELPER_FRAME,
    );
}

#[test]
fn helper_replies_roundtrip() {
    let errors = [
        HelperError::IdentityChanged,
        HelperError::PermissionDenied,
        HelperError::NoSuchProcess,
        HelperError::NotAllowed,
        HelperError::Partial { done: 3, total: 7 },
        HelperError::Os(13),
        HelperError::TooLarge,
    ];
    let mut bodies = vec![
        HelperReplyBody::Hello {
            proto: HELPER_PROTO_VERSION,
        },
        HelperReplyBody::Ok,
        HelperReplyBody::File {
            file: ProcFiles::ENVIRON,
            data: b"HOME=/root\0PATH=/usr/bin\0".to_vec(),
        },
        HelperReplyBody::Strings(vec![
            (0x7fff_0000_1000, StringEncoding::Ascii, "hello".to_string()),
            (0x5555_0000_2000, StringEncoding::Utf8, "blåbær".to_string()),
            (
                0x5555_0000_3000,
                StringEncoding::Utf16Le,
                "wide".to_string(),
            ),
        ]),
        HelperReplyBody::SocketOwners(vec![(77_123, key(200)), (77_124, key(201))]),
        HelperReplyBody::NetlinkDump(vec![0x4c, 0, 0, 0, 0x14, 0, 2, 0]),
    ];
    bodies.extend(errors.into_iter().map(HelperReplyBody::Err));
    for (seq, body) in bodies.into_iter().enumerate() {
        roundtrip(
            &HelperReply {
                seq: seq as u32,
                last: seq % 2 == 0,
                body,
            },
            MAX_HELPER_FRAME,
        );
    }
}

#[test]
fn action_serde_types_roundtrip() {
    let max = MAX_HELPER_FRAME;
    roundtrip(&ActionId(1), max);
    roundtrip(&ActionId(u64::MAX), max);
    for signal in [
        Signal::Term,
        Signal::Kill,
        Signal::Hup,
        Signal::Int,
        Signal::Quit,
        Signal::Stop,
        Signal::Cont,
        Signal::Usr1,
        Signal::Usr2,
        Signal::Other(34),
    ] {
        roundtrip(&signal, max);
    }
    for policy in [
        SchedPolicy::Other,
        SchedPolicy::Batch,
        SchedPolicy::Idle,
        SchedPolicy::Fifo,
        SchedPolicy::Rr,
    ] {
        roundtrip(&policy, max);
    }
    for prio in [
        IoPriority::None,
        IoPriority::Realtime(0),
        IoPriority::BestEffort(7),
        IoPriority::Idle,
    ] {
        roundtrip(&prio, max);
    }
    for mode in [DumpMode::Mini, DumpMode::Full] {
        roundtrip(&mode, max);
    }
    for family in [Family::Inet, Family::Inet6] {
        roundtrip(&family, max);
    }
    for proto in [Proto::Tcp, Proto::Udp] {
        roundtrip(&proto, max);
    }
    roundtrip(&four_tuple_v4(), max);
    roundtrip(&four_tuple_v6_global(), max);
}

/// serde's own `SocketAddrV6` drops the scope id in binary formats; `FourTuple` keeps it (R-270).
#[test]
fn four_tuple_keeps_ipv6_scope_id() {
    let v6 = four_tuple_v6();
    let SocketAddr::V6(local) = v6.local else {
        unreachable!("four_tuple_v6 builds V6 addresses");
    };
    assert_eq!(local.scope_id(), 3);
    roundtrip(&v6, MAX_HELPER_FRAME);
}

#[test]
fn helper_reply_sequence_read_reply() {
    let replies: Vec<HelperReply> = (0..3)
        .map(|i| HelperReply {
            seq: 5,
            last: i == 2,
            body: HelperReplyBody::File {
                file: ProcFiles::SMAPS,
                data: vec![i as u8; 1000],
            },
        })
        .collect();
    let mut buf = Vec::new();
    for reply in &replies {
        encode_frame(reply, MAX_HELPER_FRAME, &mut buf).expect("encode");
    }
    let mut reader = buf.as_slice();
    let back = read_reply(
        &mut reader,
        MAX_HELPER_FRAME,
        MAX_HELPER_REPLY_TOTAL,
        |r: &HelperReply| r.last,
    )
    .expect("read_reply");
    assert_eq!(back, replies);
    assert!(reader.is_empty());
}
