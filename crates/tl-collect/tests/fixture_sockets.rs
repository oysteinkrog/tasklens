//! sock_diag tests (E06-003) and socket map tests (E06-004). The `real_*` tests open a
//! NETLINK_SOCK_DIAG socket or walk /proc on the real system and run with
//! `cargo test -p tl-collect -- --ignored`.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};

use netlink_packet_core::{Emitable, NLMSG_DONE, NetlinkHeader};
use netlink_packet_sock_diag::inet::{InetResponseHeader, SocketId};
use netlink_packet_sock_diag::{AF_INET, AF_INET6, INET_DIAG_INFO, SOCK_DIAG_BY_FAMILY};
use tl_collect::SysRoot;
use tl_collect::net::sock_diag::{Family, Proto, SockDiag, SockRecord, parse_dump};
use tl_collect::net::socket_map::SocketMap;
use tl_collect::net::{
    ConnCounts, ConnectionTable, IfNames, Owner, TcpAttribution, tcp_state_label, udp_state_label,
};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_model::validity::Validity;

fn dump_all() -> Vec<SockRecord> {
    let mut diag = SockDiag::open().unwrap();
    let mut out = Vec::new();
    diag.dump_all(&mut out).unwrap();
    out
}

#[test]
#[ignore = "real system: opens a NETLINK_SOCK_DIAG socket"]
fn real_dump_sees_tcp_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let out = dump_all();
    let found = out.iter().any(|r| {
        r.proto == Proto::Tcp
            && r.family == Family::V4
            && r.sport == port
            && r.state == 10
            && r.inode != 0
    });
    assert!(found, "no TCP V4 listener on port {port}");
}

#[test]
#[ignore = "real system: opens a NETLINK_SOCK_DIAG socket"]
fn real_dump_sees_udp6() {
    let socket = UdpSocket::bind("[::1]:0").unwrap();
    let port = socket.local_addr().unwrap().port();

    let out = dump_all();
    let found = out.iter().any(|r| {
        r.proto == Proto::Udp
            && r.family == Family::V6
            && r.sport == port
            && r.src == IpAddr::V6(Ipv6Addr::LOCALHOST)
    });
    assert!(found, "no UDP V6 socket on port {port}");
}

#[test]
#[ignore = "real system: opens a NETLINK_SOCK_DIAG socket"]
fn real_tcp_info_bytes_acked() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = listener.local_addr().unwrap().port();
    let mut sender = TcpStream::connect(("127.0.0.1", server_port)).unwrap();
    let sender_port = sender.local_addr().unwrap().port();
    let (mut receiver, _) = listener.accept().unwrap();

    sender.write_all(&[0x5A; 1000]).unwrap();
    let mut got = [0u8; 1000];
    receiver.read_exact(&mut got).unwrap();

    // The ACK for the last segment can trail the read by a moment.
    let mut diag = SockDiag::open().unwrap();
    let mut out = Vec::new();
    let mut acked = 0;
    for _ in 0..50 {
        diag.dump_tcp(&mut out).unwrap();
        acked = out
            .iter()
            .find(|r| r.sport == sender_port && r.dport == server_port)
            .and_then(|r| r.tcp_info)
            .map_or(0, |i| i.bytes_acked);
        if acked >= 1000 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(acked >= 1000, "bytes_acked {acked}");
    assert!(out.iter().all(|r| r.proto == Proto::Tcp));
}

#[test]
#[ignore = "real system: walks /proc/<pid>/fd, must run as a normal user"]
fn real_socket_map_pid1_restricted() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let inode = rustix::fs::fstat(&listener).unwrap().st_ino;

    let mut map = SocketMap::new();
    map.rebuild(&SysRoot::host(), BootId(0)).unwrap();
    assert_eq!(
        map.restricted.get(&1),
        Some(&0),
        "pid 1 not restricted as uid 0"
    );
    let owners = map.owners_of(inode);
    assert!(
        owners.iter().any(|k| k.pid == std::process::id()),
        "listener inode {inode} owners {owners:?}"
    );
}

#[test]
#[ignore = "real system: walks /proc/<pid>/fd"]
#[allow(
    clippy::print_stdout,
    reason = "the bead asks the test to print the walk time (U-23)"
)]
fn real_socket_map_timing() {
    let mut map = SocketMap::new();
    map.rebuild(&SysRoot::host(), BootId(0)).unwrap();
    // Second walk: the maps and buffers are warm, as in the engine loop.
    map.rebuild(&SysRoot::host(), BootId(0)).unwrap();
    println!(
        "socket map rebuild: {} us, {} inodes, {} restricted pids",
        map.elapsed_ns / 1000,
        map.owners.len(),
        map.restricted.len()
    );
    assert!(map.elapsed_ns > 0);
}

// == fixture tests (E06-A1) ==
//
// `sockets-basic/dump/*.bin` are netlink dump replies in native byte order, one file per family
// and protocol, each ending with NLMSG_DONE. `write_dump_fixtures` writes them.

const BOOT: BootId = BootId(1);
const SEC: u64 = 1_000_000_000;
const NLMSG_HDR_LEN: usize = 16;
/// `struct tcp_info` length on current kernels.
const TCP_INFO_LEN: usize = 232;
const TCPI_BYTES_ACKED: usize = 120;
const TCPI_BYTES_RECEIVED: usize = 128;

/// The four dump files in `dump_all` order.
const DUMPS: [(&str, Family, Proto); 4] = [
    ("tcp4.bin", Family::V4, Proto::Tcp),
    ("tcp6.bin", Family::V6, Proto::Tcp),
    ("udp4.bin", Family::V4, Proto::Udp),
    ("udp6.bin", Family::V6, Proto::Udp),
];

fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sockets-basic")
}

fn align4(len: usize) -> usize {
    (len + 3) & !3
}

/// One netlink message of `msg_type` around `payload`, padded to 4 bytes.
fn msg(msg_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut header = NetlinkHeader::default();
    header.length = (NLMSG_HDR_LEN + payload.len()) as u32;
    header.message_type = msg_type;
    let mut out = vec![0u8; align4(NLMSG_HDR_LEN + payload.len())];
    header.emit(&mut out[..NLMSG_HDR_LEN]);
    out[NLMSG_HDR_LEN..NLMSG_HDR_LEN + payload.len()].copy_from_slice(payload);
    out
}

struct Sock {
    state: u8,
    src: IpAddr,
    sport: u16,
    dst: IpAddr,
    dport: u16,
    ifindex: u32,
    inode: u32,
    cookie: u64,
    /// `(bytes_acked, bytes_received)` for an `INET_DIAG_INFO` attribute.
    tcp_info: Option<(u64, u64)>,
}

fn diag_msg(s: &Sock) -> Vec<u8> {
    let mut id = match s.src {
        IpAddr::V4(_) => SocketId::new_v4(),
        IpAddr::V6(_) => SocketId::new_v6(),
    };
    id.source_address = s.src;
    id.source_port = s.sport;
    id.destination_address = s.dst;
    id.destination_port = s.dport;
    id.interface_id = s.ifindex;
    id.cookie[..4].copy_from_slice(&(s.cookie as u32).to_ne_bytes());
    id.cookie[4..].copy_from_slice(&((s.cookie >> 32) as u32).to_ne_bytes());
    let header = InetResponseHeader {
        family: if s.src.is_ipv4() { AF_INET } else { AF_INET6 },
        state: s.state,
        timer: None,
        socket_id: id,
        recv_queue: 0,
        send_queue: 0,
        uid: 1000,
        inode: s.inode,
    };
    let mut p = vec![0u8; header.buffer_len()];
    header.emit(&mut p);
    if let Some((acked, received)) = s.tcp_info {
        let mut info = [0u8; TCP_INFO_LEN];
        info[TCPI_BYTES_ACKED..TCPI_BYTES_ACKED + 8].copy_from_slice(&acked.to_ne_bytes());
        info[TCPI_BYTES_RECEIVED..TCPI_BYTES_RECEIVED + 8].copy_from_slice(&received.to_ne_bytes());
        let len = 4 + TCP_INFO_LEN;
        p.extend_from_slice(&(len as u16).to_ne_bytes());
        p.extend_from_slice(&INET_DIAG_INFO.to_ne_bytes());
        p.extend_from_slice(&info);
        p.resize(align4(p.len()), 0);
    }
    msg(SOCK_DIAG_BY_FAMILY, &p)
}

fn dump_file(socks: &[Sock]) -> Vec<u8> {
    let mut out: Vec<u8> = socks.iter().flat_map(diag_msg).collect();
    out.extend(msg(NLMSG_DONE, &0i32.to_ne_bytes()));
    out
}

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

const V4_ANY: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
const V6_ANY: IpAddr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);

#[test]
fn write_dump_fixtures() {
    if std::env::var_os("TL_WRITE_FIXTURES").is_none_or(|v| v != "1") {
        return;
    }
    let tcp4 = [
        Sock {
            state: 10,
            src: V4_ANY,
            sport: 22,
            dst: V4_ANY,
            dport: 0,
            ifindex: 0,
            inode: 5001,
            cookie: 1,
            tcp_info: None,
        },
        Sock {
            state: 1,
            src: v4(192, 168, 1, 10),
            sport: 51000,
            dst: v4(140, 82, 112, 3),
            dport: 443,
            ifindex: 0,
            inode: 5002,
            cookie: 2,
            tcp_info: Some((10000, 20000)),
        },
        Sock {
            state: 6,
            src: v4(127, 0, 0, 1),
            sport: 40000,
            dst: v4(127, 0, 0, 1),
            dport: 8080,
            ifindex: 0,
            inode: 0,
            cookie: 3,
            tcp_info: None,
        },
    ];
    let tcp6 = [Sock {
        state: 10,
        src: V6_ANY,
        sport: 631,
        dst: V6_ANY,
        dport: 0,
        ifindex: 0,
        inode: 5003,
        cookie: 4,
        tcp_info: None,
    }];
    let udp4 = [Sock {
        state: 7,
        src: V4_ANY,
        sport: 5353,
        dst: V4_ANY,
        dport: 0,
        ifindex: 0,
        inode: 5004,
        cookie: 5,
        tcp_info: None,
    }];
    let udp6 = [Sock {
        state: 7,
        src: IpAddr::V6("fe80::1".parse().unwrap()),
        sport: 546,
        dst: V6_ANY,
        dport: 0,
        ifindex: 3,
        inode: 0,
        cookie: 6,
        tcp_info: None,
    }];
    let dir = root_dir().join("dump");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, socks) in [
        ("tcp4.bin", &tcp4[..]),
        ("tcp6.bin", &tcp6[..]),
        ("udp4.bin", &udp4[..]),
        ("udp6.bin", &udp6[..]),
    ] {
        std::fs::write(dir.join(name), dump_file(socks)).unwrap();
    }
}

/// Every record of the four dump files, in `dump_all` order.
fn load_dumps() -> Vec<SockRecord> {
    let mut out = Vec::new();
    for (name, family, proto) in DUMPS {
        let bytes = std::fs::read(root_dir().join("dump").join(name)).unwrap();
        assert!(
            parse_dump(&bytes, family, proto, &mut out).unwrap(),
            "{name}: no NLMSG_DONE"
        );
    }
    out
}

fn socket_map() -> SocketMap {
    let mut map = SocketMap::new();
    map.rebuild(&SysRoot::at(&root_dir()), BOOT).unwrap();
    map
}

fn table() -> ConnectionTable {
    let ifnames = IfNames::read(&SysRoot::at(&root_dir())).unwrap();
    let mut table = ConnectionTable::new();
    table.update(&load_dumps(), &socket_map(), &ifnames);
    table
}

/// The Connections tab columns of one row.
#[derive(Debug)]
#[allow(dead_code, reason = "read through the Debug snapshot")]
struct Row {
    cookie: u64,
    proto: Proto,
    state: &'static str,
    local: String,
    foreign: String,
    owner: Option<Vec<u32>>,
    interface: Option<String>,
}

#[test]
fn fixture_connection_table_snapshot() {
    let rows: Vec<Row> = table()
        .records
        .iter()
        .map(|r| Row {
            cookie: r.cookie,
            proto: r.sock.proto,
            state: match r.sock.proto {
                Proto::Tcp => tcp_state_label(r.sock.state),
                Proto::Udp => udp_state_label(r.sock.state),
            },
            local: r.local.to_string(),
            foreign: r.foreign.to_string(),
            // None is the kernel owner.
            owner: match &r.owner {
                Owner::Kernel => None,
                Owner::Processes(keys) => Some(keys.iter().map(|k| k.pid).collect()),
                Owner::Restricted { uid } => panic!("restricted uid {uid}"),
            },
            interface: r.interface.as_deref().map(str::to_owned),
        })
        .collect();
    insta::assert_debug_snapshot!("connection_table", rows);
}

#[test]
fn fixture_attribution_shared() {
    let map = socket_map();
    let mut records = load_dumps();
    let mut att = TcpAttribution::new();
    att.update(&records, &map, 10 * SEC);
    assert_eq!(att.validity, Validity::WarmingUp);

    for r in &mut records {
        if let Some(info) = r.tcp_info.as_mut() {
            info.bytes_acked += 2000;
        }
    }
    att.update(&records, &map, 11 * SEC);
    assert_eq!(att.validity, Validity::Valid);
    for (pid, start_ticks) in [(100, 5000), (200, 6000)] {
        let key = ProcessKey {
            start_ticks,
            boot_id: BOOT,
            pid,
        };
        let rate = att.per_process[&key];
        assert_eq!(rate.tx_bps, 2000.0, "pid {pid}");
        assert_eq!(rate.rx_bps, 0.0, "pid {pid}");
        assert!(rate.shared, "pid {pid}");
    }
    // Only the ESTABLISHED socket has tcp_info. It counts in full for both of its owners,
    // and once in the total.
    assert_eq!(att.total_tx_bps, 2000.0);
}

#[test]
fn fixture_counts() {
    assert_eq!(
        table().counts,
        ConnCounts {
            total: 6,
            tcp: 4,
            udp: 2,
            established: 1,
            listening: 2,
        }
    );
}
