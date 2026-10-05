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

proptest! {
    #[test]
    fn prop_desktop_entry_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..=128 * 1024),
        locale in "[a-z_A-Z.@0-9-]{0,16}",
    ) {
        use tl_collect::startup::autostart::{locale_match_list, parse_desktop_entry};
        let list = locale_match_list(&locale);
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        let _ = parse_desktop_entry(&bytes, &refs);
        let mut file = b"[Desktop Entry]\n".to_vec();
        file.extend_from_slice(&bytes);
        let _ = parse_desktop_entry(&file, &refs);
    }
}

// == inventory (E06-A3) ==

proptest! {
    #[test]
    fn prop_cpuinfo_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        use tl_collect::inventory::cpuinfo::{identifier, parse_cpuinfo, CpuInfo};
        let mut info = CpuInfo::default();
        parse_cpuinfo(&bytes, &mut info);
        let mut text = String::new();
        let _ = identifier(&info, &mut text);
    }
}

proptest! {
    #[test]
    fn prop_os_release_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        let _ = tl_collect::inventory::os_release::parse_os_release(&bytes);
    }
}

proptest! {
    #[test]
    fn prop_udev_dmi_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        let _ = tl_collect::inventory::modules::parse_udev_dmi(&bytes);
    }
}

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
