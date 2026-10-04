use proptest::prelude::*;

// == sockets (E06-A1) ==

proptest! {
    #[test]
    fn prop_sock_diag_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=65536)) {
        use tl_collect::net::sock_diag::{parse_dump, Family, Proto};
        let mut out = Vec::new();
        for family in [Family::V4, Family::V6] {
            for proto in [Proto::Tcp, Proto::Udp] {
                let _ = parse_dump(&bytes, family, proto, &mut out);
            }
        }
    }
}

// == startup (E06-A2) ==

// == inventory (E06-A3) ==

// == gpu_drm (E06-A3) ==

proptest! {
    #[test]
    fn netsys_prop_harness_runs(_bytes in any::<Vec<u8>>()) {
        prop_assert!(true);
    }
}
