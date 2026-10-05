//! sock_diag tests (E06-003) and socket map tests (E06-004). The `real_*` tests open a
//! NETLINK_SOCK_DIAG socket or walk /proc on the real system and run with
//! `cargo test -p tl-collect -- --ignored`.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv6Addr, TcpListener, TcpStream, UdpSocket};

use tl_collect::SysRoot;
use tl_collect::net::sock_diag::{Family, Proto, SockDiag, SockRecord};
use tl_collect::net::socket_map::SocketMap;
use tl_model::time::BootId;

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
