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

proptest! {
    #[test]
    fn prop_parse_dump_never_panics(
        msg_type in prop_oneof![Just(20u16), Just(2u16), Just(3u16), any::<u16>()],
        claimed in any::<bool>(),
        payload in proptest::collection::vec(any::<u8>(), 0..=1024),
        family_byte in prop_oneof![Just(2u8), Just(10u8), any::<u8>()],
    ) {
        use tl_collect::net::sock_diag::{parse_dump, Family, Proto};
        // One netlink message around arbitrary bytes, so the inet_diag and attribute parsers
        // see the input and not only the netlink header check.
        let mut payload = payload;
        if let Some(first) = payload.first_mut() {
            *first = family_byte;
        }
        let len = 16 + payload.len();
        let claimed_len = if claimed { len as u32 } else { (len as u32).wrapping_add(4) };
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(&claimed_len.to_ne_bytes());
        buf.extend_from_slice(&msg_type.to_ne_bytes());
        buf.extend_from_slice(&[0u8; 10]);
        buf.extend_from_slice(&payload);
        let mut out = Vec::new();
        for family in [Family::V4, Family::V6] {
            for proto in [Proto::Tcp, Proto::Udp] {
                let _ = parse_dump(&buf, family, proto, &mut out);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn prop_socket_link_never_panics(
        target in prop_oneof![
            proptest::collection::vec(1u8..=255, 1..=48),
            "socket:\\[[0-9]{0,24}\\]x?".prop_map(String::into_bytes),
        ],
    ) {
        use std::os::unix::ffi::OsStrExt;
        use tl_collect::SysRoot;
        use tl_collect::net::SocketMap;
        use tl_model::time::BootId;
        // The link parser is private, so it runs through a rebuild of a fake /proc whose only
        // fd link points at `target`.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let fd = dir.path().join("proc/100/fd");
        std::fs::create_dir_all(&fd).unwrap();
        std::fs::write(
            dir.path().join("proc/100/stat"),
            "100 (p) S 1 100 100 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5000 0 0\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&target), fd.join("3")).unwrap();
        let mut map = SocketMap::new();
        map.rebuild(&SysRoot::at(dir.path()), BootId(1)).unwrap();
        prop_assert!(map.owners.len() <= 1);
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

proptest! {
    #[test]
    fn prop_unit_file_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=128 * 1024)) {
        use tl_collect::startup::user_units::parse_unit_file;
        let _ = parse_unit_file(&bytes);
        for header in [&b"[Service]\nExecStart="[..], b"[Install]\nWantedBy="] {
            let mut file = header.to_vec();
            file.extend_from_slice(&bytes);
            let _ = parse_unit_file(&file);
        }
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

proptest! {
    #[test]
    fn prop_edid_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=512), header in any::<bool>()) {
        use tl_collect::inventory::displays::edid_monitor_name;
        let _ = edid_monitor_name(&bytes);
        let mut edid = bytes;
        if header && edid.len() >= 8 {
            edid[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        }
        if let Some(name) = edid_monitor_name(&edid) {
            prop_assert!(!name.is_empty() && name.bytes().all(|b| (0x20..=0x7E).contains(&b)));
        }
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
