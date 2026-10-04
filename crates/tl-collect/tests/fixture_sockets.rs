//! sock_diag tests (E06-003). The `real_*` tests open a NETLINK_SOCK_DIAG socket on the real
//! system and run with `cargo test -p tl-collect -- --ignored`.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv6Addr, TcpListener, TcpStream, UdpSocket};

use tl_collect::net::sock_diag::{Family, Proto, SockDiag, SockRecord};

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
