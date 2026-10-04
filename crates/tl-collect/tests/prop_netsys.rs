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
    fn prop_fdinfo_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=4096)) {
        let _ = tl_collect::gpu_drm::fdinfo::parse_fdinfo(1, &bytes);
    }
}

proptest! {
    #[test]
    fn prop_gpu_metrics_never_panics(
        size in any::<u16>(),
        format in 0u8..=3,
        content in 0u8..=5,
        tail in proptest::collection::vec(any::<u8>(), 0..=4092),
        raw in proptest::collection::vec(any::<u8>(), 0..=4096),
    ) {
        let _ = tl_collect::gpu_drm::amdgpu::parse_gpu_metrics(&raw);
        let mut blob = Vec::with_capacity(4 + tail.len());
        blob.extend_from_slice(&size.to_le_bytes());
        blob.extend_from_slice(&[format, content]);
        blob.extend_from_slice(&tail);
        let _ = tl_collect::gpu_drm::amdgpu::parse_gpu_metrics(&blob);
        let _ = tl_collect::gpu_drm::amdgpu::parse_dpm_max_mhz(&raw);
    }
}

proptest! {
    #[test]
    fn netsys_prop_harness_runs(_bytes in any::<Vec<u8>>()) {
        prop_assert!(true);
    }
}
