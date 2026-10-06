//! Real-system network tests. They only read. Run with
//! `cargo test -p tl-collect --test real_net -- --ignored`.
use tl_collect::net::nl80211::Nl80211;

/// The ifindex of every interface that has `/sys/class/net/<if>/phy80211`.
fn sysfs_wifi_ifindexes() -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    dir.flatten()
        .filter(|e| e.path().join("phy80211").exists())
        .filter_map(|e| std::fs::read_to_string(e.path().join("ifindex")).ok())
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

#[test]
#[ignore = "real system: opens a NETLINK_GENERIC socket and dumps nl80211"]
fn nl80211_open_real() {
    let wifi = sysfs_wifi_ifindexes();
    let Some(mut nl) = Nl80211::open().expect("open") else {
        // No Wi-Fi driver loaded.
        assert!(wifi.is_empty(), "sysfs lists Wi-Fi interfaces {wifi:?}");
        return;
    };
    let links = nl.links().expect("links");
    for ifindex in &wifi {
        assert!(
            links.iter().any(|l| l.ifindex == *ifindex),
            "ifindex {ifindex} missing from nl80211"
        );
    }
    for link in &links {
        assert!(link.ifindex > 0);
        assert!(link.ssid.as_ref().is_none_or(|s| s.len() <= 32));
    }
    // A second dump on the same socket works too.
    assert_eq!(nl.links().expect("links again").len(), links.len());
}
