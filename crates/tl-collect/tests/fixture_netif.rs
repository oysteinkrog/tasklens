//! Network interface enumeration, kinds and state against the `netif-kinds` hand fixture and the
//! captured `owner-desktop` fixture (ADR-016, PERF-054, R-162).
#![allow(clippy::unwrap_used)]
use std::fmt::Write as _;

use tl_collect::parse::netif::{self, IfaceKind, NetIface, OperState};
use tl_model::validity::Validity;

mod common;

const NETNS_INO: u64 = 4_026_531_840;

fn ifaces(name: &str) -> Vec<NetIface> {
    let mut out = Vec::new();
    netif::enumerate(&common::fixture(name), &mut out).unwrap();
    out
}

#[test]
fn netns_inode() {
    assert_eq!(
        netif::read_netns_ino(&common::fixture("netif-kinds")).unwrap(),
        NETNS_INO
    );
}

#[test]
fn kinds_in_order() {
    let list = ifaces("netif-kinds");
    let got: Vec<(&str, IfaceKind)> = list.iter().map(|i| (&*i.name, i.kind)).collect();
    assert_eq!(
        got,
        [
            ("br0", IfaceKind::Bridge),
            ("docker0", IfaceKind::Virtual),
            ("dummy0", IfaceKind::Other),
            ("eth0", IfaceKind::Ethernet),
            ("tailscale0", IfaceKind::Tunnel),
            ("usb0", IfaceKind::UsbNetwork),
            ("veth12ab", IfaceKind::Virtual),
            ("virbr0", IfaceKind::Virtual),
            ("wlan0", IfaceKind::WiFi),
            ("wwan0", IfaceKind::Cellular),
        ]
    );
    assert!(list.iter().all(|i| i.netns_ino == NETNS_INO));
    let eth0 = &list[3];
    assert_eq!((eth0.ifindex, eth0.arphrd_type), (2, 1));
    assert_eq!(list[4].arphrd_type, 65534);
}

#[test]
fn oper_states() {
    let root = common::fixture("netif-kinds");
    let mut buf = Vec::new();
    for (name, want) in [
        ("eth0", OperState::Connected),
        ("wlan0", OperState::Dormant),
        ("dummy0", OperState::Connected),
        ("veth12ab", OperState::Unknown),
        ("br0", OperState::LowerLayerDown),
    ] {
        let got = netif::read_oper_state(&root, name, &mut buf);
        assert_eq!(got.validity, Validity::Valid, "{name}");
        assert_eq!(got.value, want, "{name}");
    }
}

#[test]
fn missing_operstate() {
    let root = common::fixture("netif-kinds");
    let mut buf = Vec::new();
    let got = netif::read_oper_state(&root, "gone0", &mut buf);
    assert_eq!(got.validity, Validity::Missing);
    assert_eq!(got.value, OperState::Unknown);
}

#[test]
fn owner_desktop_snapshot() {
    let root = common::fixture("owner-desktop");
    let list = ifaces("owner-desktop");
    let mut buf = Vec::new();
    let mut report = String::new();
    for iface in &list {
        match &*iface.name {
            "tailscale0" => assert_eq!(iface.kind, IfaceKind::Tunnel),
            "docker0" => assert_eq!(iface.kind, IfaceKind::Virtual),
            "lo" => panic!("lo must not be listed"),
            _ => {}
        }
        let state = netif::read_oper_state(&root, &iface.name, &mut buf);
        writeln!(
            report,
            "{} ifindex={} netns={} type={} kind={:?} state={:?}/{:?}",
            iface.name,
            iface.ifindex,
            iface.netns_ino,
            iface.arphrd_type,
            iface.kind,
            state.value,
            state.validity
        )
        .unwrap();
    }
    insta::assert_snapshot!(report);
}
