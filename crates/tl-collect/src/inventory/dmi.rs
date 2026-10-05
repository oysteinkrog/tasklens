//! DMI fields and firmware type for the Information tab's BIOS/Firmware, Motherboard and System
//! sections (PERF-101, PERF-102, ADR-016 "Information", D-007).
//!
//! Only the world-readable fields below and `board_serial` are opened. `product_serial`,
//! `product_uuid`, `chassis_serial` and every other file in the directory are never read. A
//! root-only field is `Restricted`, never empty or zero. `board_serial` is never logged.

use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::sync::Arc;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::sanitize::sanitize_text;

/// Most bytes read from one DMI file.
const READ_MAX: usize = 256;

/// One DMI value as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Value(Arc<str>),
    /// The file exists but needs root ("Restricted", tooltip "Needs administrator access.").
    Restricted,
    /// No file, an empty value or another error ("Not reported").
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FirmwareType {
    Uefi,
    LegacyBios,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DmiInfo {
    /// `<sys>/firmware/efi` exists: `Uefi`.
    pub firmware: FirmwareType,
    pub bios_vendor: Field,
    pub bios_version: Field,
    pub bios_date: Field,
    pub bios_release: Field,
    pub board_vendor: Field,
    pub board_name: Field,
    pub board_version: Field,
    pub board_serial: Field,
    pub sys_vendor: Field,
    pub product_name: Field,
    /// SMBIOS chassis type code from `chassis_type`. The UI maps it to text.
    pub chassis_type: Option<u8>,
}

/// Maps a failed open or read: `EACCES` and `EPERM` are `Restricted`, the rest `Missing`.
pub fn classify_errno(e: Errno) -> Field {
    match e {
        Errno::ACCESS | Errno::PERM => Field::Restricted,
        _ => Field::Missing,
    }
}

/// Reads the DMI fields under `<sys>/class/dmi/id` and the firmware type. Never fails: a missing
/// directory gives `Missing` for every field.
pub fn read_dmi(sys: &Path) -> DmiInfo {
    let firmware = if sys.join("firmware/efi").exists() {
        FirmwareType::Uefi
    } else {
        FirmwareType::LegacyBios
    };
    let dir = rustix::fs::open(
        sys.join("class/dmi/id"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    );
    let mut scratch = String::new();
    let mut field = |name: &str| match &dir {
        Ok(dir) => read_field(dir.as_fd(), name, &mut scratch),
        Err(e) => classify_errno(*e),
    };
    let chassis = field("chassis_type");
    DmiInfo {
        firmware,
        bios_vendor: field("bios_vendor"),
        bios_version: field("bios_version"),
        bios_date: field("bios_date"),
        bios_release: field("bios_release"),
        board_vendor: field("board_vendor"),
        board_name: field("board_name"),
        board_version: field("board_version"),
        board_serial: field("board_serial"),
        sys_vendor: field("sys_vendor"),
        product_name: field("product_name"),
        chassis_type: match chassis {
            Field::Value(text) => text.parse().ok(),
            Field::Restricted | Field::Missing => None,
        },
    }
}

/// Opens `name` in `dir`, reads at most [`READ_MAX`] bytes, trims and sanitizes the value.
fn read_field(dir: BorrowedFd<'_>, name: &str, scratch: &mut String) -> Field {
    let fd = match rustix::fs::openat(dir, name, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => fd,
        Err(e) => return classify_errno(e),
    };
    let mut buf = [0u8; READ_MAX];
    let mut len = 0;
    while len < READ_MAX {
        match rustix::io::read(&fd, &mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(Errno::INTR) => {}
            Err(e) => return classify_errno(e),
        }
    }
    sanitize_text(buf[..len].trim_ascii(), READ_MAX, scratch);
    let value = scratch.trim();
    if value.is_empty() {
        Field::Missing
    } else {
        Field::Value(Arc::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_sys() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dmi-desktop/sys")
    }

    fn value(text: &str) -> Field {
        Field::Value(Arc::from(text))
    }

    #[test]
    fn fixture_values() {
        let info = read_dmi(&fixture_sys());
        assert_eq!(
            info.bios_vendor,
            value("American Megatrends International, LLC.")
        );
        assert_eq!(info.board_name, value("ROG STRIX X570-E GAMING"));
        assert_eq!(info.board_vendor, value("ASUSTeK COMPUTER INC."));
        assert_eq!(info.sys_vendor, value("System manufacturer"));
        assert_eq!(info.product_name, value("System Product Name"));
        assert_eq!(info.board_version, value("Default string"));
        assert_eq!(info.chassis_type, Some(3));
        assert_eq!(info.board_serial, Field::Missing);
        assert_eq!(info.firmware, FirmwareType::Uefi);
    }

    #[test]
    fn empty_is_missing() {
        let info = read_dmi(&fixture_sys());
        assert_eq!(info.bios_release, Field::Missing);
    }

    #[test]
    fn errno_mapping() {
        assert_eq!(classify_errno(Errno::ACCESS), Field::Restricted);
        assert_eq!(classify_errno(Errno::PERM), Field::Restricted);
        assert_eq!(classify_errno(Errno::NOENT), Field::Missing);
        assert_eq!(classify_errno(Errno::IO), Field::Missing);
    }

    #[test]
    fn product_serial_never_read() {
        let sys = fixture_sys();
        let planted = std::fs::read_to_string(sys.join("class/dmi/id/product_serial")).unwrap();
        assert!(planted.contains("SERIAL-MUST-NOT-APPEAR"));
        let info = read_dmi(&sys);
        assert!(!format!("{info:?}").contains("SERIAL-MUST-NOT-APPEAR"));
    }

    #[test]
    fn no_dmi_directory_is_legacy_and_missing() {
        let info = read_dmi(Path::new("/nonexistent-tasklens-sys"));
        assert_eq!(info.firmware, FirmwareType::LegacyBios);
        assert_eq!(info.bios_vendor, Field::Missing);
        assert_eq!(info.chassis_type, None);
    }
}
