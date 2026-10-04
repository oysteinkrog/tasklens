//! Reader for udev database files under /run/udev/data/ (R-140). No libudev (ADR-001).
//!
//! File content is untrusted. Values are stored as given and callers sanitize them before
//! display (ADR-015). `ID_SERIAL`, `ID_SERIAL_SHORT` and `ID_WWN` are hardware serials: never
//! log them.

use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::sysroot::read_file;
use crate::CollectError;

pub const UDEV_FILE_LIMIT: usize = 64 * 1024;

/// Keys any caller may ask for (R-140 item 1, plus ID_PATH for the by-path key fallback).
///
/// The fixture capture allowlist (R-266) keeps the same 15 keys. A new key goes here, in
/// `xtask/src/capture_fixture.rs` and in the ADR-016 udev row in the same change.
pub const UDEV_KEYS: [&str; 15] = [
    "ID_MODEL",
    "ID_MODEL_FROM_DATABASE",
    "ID_VENDOR_FROM_DATABASE",
    "ID_BUS",
    "ID_WWN",
    "ID_SERIAL",
    "ID_SERIAL_SHORT",
    "ID_FS_TYPE",
    "ID_FS_LABEL",
    "ID_PART_TABLE_TYPE",
    "ID_PART_ENTRY_TYPE",
    "ID_PART_ENTRY_FLAGS",
    "ID_PART_ENTRY_OFFSET",
    "ID_PART_ENTRY_SIZE",
    "ID_PATH",
];

/// The `E:` properties kept from one udev database file, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UdevProps(pub SmallVec<[(Arc<str>, Arc<str>); 8]>);

impl UdevProps {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| &**k == key)
            .map(|(_, v)| &**v)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Reads one udev database file (at most UDEV_FILE_LIMIT bytes), keeps only `E:KEY=value` lines whose
/// key is in `keys`, and stores them in `out` (cleared first). A missing file is `Ok` with `out` empty.
pub fn read_udev_props(
    path: &Path,
    keys: &[&str],
    out: &mut UdevProps,
) -> Result<(), CollectError> {
    debug_assert!(
        keys.iter().all(|k| UDEV_KEYS.contains(k)),
        "udev key outside UDEV_KEYS (R-266)"
    );
    out.clear();
    let mut buf = Vec::new();
    let truncated = match read_file(path, &mut buf, UDEV_FILE_LIMIT) {
        Ok((_, truncated)) => truncated,
        Err(CollectError::NotFound) => return Ok(()),
        Err(err) => return Err(err),
    };
    parse_udev_props(&buf, truncated, keys, out);
    Ok(())
}

/// Parses file bytes into `out` (not cleared here). When `truncated`, the cut last line is dropped.
fn parse_udev_props(bytes: &[u8], truncated: bool, keys: &[&str], out: &mut UdevProps) {
    let bytes = if truncated {
        match memchr::memrchr(b'\n', bytes) {
            Some(end) => &bytes[..end],
            None => return,
        }
    } else {
        bytes
    };
    for line in bytes.split(|&b| b == b'\n') {
        let Some(rest) = line.strip_prefix(b"E:") else {
            continue;
        };
        let Some(eq) = memchr::memchr(b'=', rest) else {
            continue;
        };
        let (name, value) = (&rest[..eq], &rest[eq + 1..]);
        let Some(key) = keys.iter().find(|k| k.as_bytes() == name) else {
            continue;
        };
        if out.get(key).is_some() {
            continue;
        }
        let value: Arc<str> = String::from_utf8_lossy(value).into();
        out.0.push((Arc::from(*key), value));
    }
}

/// Database entry names (udev layout, R-140 item 1).
pub fn block_entry(major: u32, minor: u32) -> String {
    format!("b{major}:{minor}")
}

pub fn net_entry(ifindex: u32) -> String {
    format!("n{ifindex}")
}

/// For example `pci_entry("0000:2d:00.0")` is `"+pci:0000:2d:00.0"`.
pub fn pci_entry(slot: &str) -> String {
    format!("+pci:{slot}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SysRoot;
    use proptest::prelude::*;
    use std::fs;

    fn fixture() -> SysRoot {
        SysRoot::at(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/udev-basic"))
    }

    fn entry_path(root: &SysRoot, entry: &str) -> std::path::PathBuf {
        root.run_path(&format!("udev/data/{entry}"))
    }

    #[test]
    fn reads_listed_keys_only() {
        let root = fixture();
        let mut out = UdevProps::default();
        read_udev_props(
            &entry_path(&root, &block_entry(8, 0)),
            &["ID_BUS", "ID_MODEL"],
            &mut out,
        )
        .unwrap();
        assert_eq!(out.get("ID_BUS"), Some("ata"));
        assert_eq!(out.get("ID_MODEL"), Some("WDC_WD10EZEX-08W"));
        assert_eq!(out.get("ID_WWN"), None);
        assert_eq!(out.0.len(), 2);

        read_udev_props(&entry_path(&root, &net_entry(2)), &UDEV_KEYS, &mut out).unwrap();
        assert_eq!(out.get("ID_VENDOR_FROM_DATABASE"), Some("Intel Corporation"));
        assert_eq!(
            out.get("ID_MODEL_FROM_DATABASE"),
            Some("Ethernet Controller I225-V")
        );
        assert_eq!(out.get("ID_BUS"), None, "out is cleared between reads");

        read_udev_props(
            &entry_path(&root, &pci_entry("0000:2d:00.0")),
            &["ID_MODEL_FROM_DATABASE"],
            &mut out,
        )
        .unwrap();
        assert_eq!(
            out.get("ID_MODEL_FROM_DATABASE"),
            Some("Navi 31 [Radeon RX 7900 XTX]")
        );
    }

    #[test]
    fn missing_file_is_empty() {
        let mut out = UdevProps::default();
        out.0.push((Arc::from("ID_BUS"), Arc::from("stale")));
        let path = entry_path(&fixture(), &block_entry(259, 7));
        read_udev_props(&path, &UDEV_KEYS, &mut out).unwrap();
        assert_eq!(out, UdevProps::default());
    }

    #[test]
    fn first_value_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b8:16");
        fs::write(&path, "E:ID_BUS=usb\nE:ID_BUS=ata\nE:ID_MODEL\nQ:x=y\n").unwrap();
        let mut out = UdevProps::default();
        read_udev_props(&path, &UDEV_KEYS, &mut out).unwrap();
        assert_eq!(out.get("ID_BUS"), Some("usb"));
        assert_eq!(out.get("ID_MODEL"), None, "a line without '=' is skipped");
        assert_eq!(out.0.len(), 1);
    }

    #[test]
    fn limit_64k() {
        // 70 KiB: filler, then one E: line that starts 14 bytes before the limit, then a key
        // past the limit.
        let filler: &[u8] = b"S:disk/by-id/filler\n";
        let start = UDEV_FILE_LIMIT - 14;
        let mut content = b"E:ID_BUS=ata\n".to_vec();
        while start - content.len() > filler.len() + 3 {
            content.extend_from_slice(filler);
        }
        let pad = start - content.len() - 3;
        content.extend_from_slice(b"S:");
        content.extend(std::iter::repeat_n(b'x', pad));
        content.push(b'\n');
        assert_eq!(content.len(), start);
        content.extend_from_slice(b"E:ID_MODEL=cut_by_the_limit\n");
        content.extend_from_slice(b"E:ID_PATH=past_the_limit\n");
        while content.len() < 70 * 1024 {
            content.extend_from_slice(filler);
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b8:32");
        fs::write(&path, &content).unwrap();
        let mut out = UdevProps::default();
        read_udev_props(&path, &UDEV_KEYS, &mut out).unwrap();
        assert_eq!(out.get("ID_BUS"), Some("ata"));
        assert_eq!(out.get("ID_MODEL"), None, "the cut last line is dropped");
        assert_eq!(out.get("ID_PATH"), None, "bytes past the limit are not read");
    }

    #[test]
    fn non_utf8_value_is_lossy() {
        let mut out = UdevProps::default();
        parse_udev_props(b"E:ID_MODEL=a\xffb\r", false, &UDEV_KEYS, &mut out);
        assert_eq!(out.get("ID_MODEL"), Some("a\u{fffd}b\r"));
    }

    #[test]
    fn permission_denied_is_restricted() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n3");
        fs::write(&path, "E:ID_BUS=pci\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let mut out = UdevProps::default();
        let err = read_udev_props(&path, &UDEV_KEYS, &mut out).unwrap_err();
        assert_eq!(err.validity(), tl_model::validity::Validity::Restricted);
        assert!(out.0.is_empty());
    }

    #[test]
    fn entry_names() {
        assert_eq!(block_entry(259, 0), "b259:0");
        assert_eq!(net_entry(2), "n2");
        assert_eq!(pci_entry("0000:2d:00.0"), "+pci:0000:2d:00.0");
    }

    proptest! {
        #[test]
        fn no_panic_on_arbitrary_bytes(
            bytes in proptest::collection::vec(any::<u8>(), 0..4096),
            truncated in any::<bool>(),
        ) {
            let mut out = UdevProps::default();
            parse_udev_props(&bytes, truncated, &UDEV_KEYS, &mut out);
            for (i, (key, _)) in out.0.iter().enumerate() {
                prop_assert!(UDEV_KEYS.contains(&&**key));
                prop_assert!(out.0[..i].iter().all(|(k, _)| k != key));
            }
        }
    }
}
