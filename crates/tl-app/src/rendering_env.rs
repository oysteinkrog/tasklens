//! `rendering.gpu` as environment variables for GPUI (APP-046, R-070, ADR-010).
//!
//! `main` sets the returned pairs before any thread starts. A `ZED_DEVICE_ID` the user set
//! always wins.

use std::ffi::OsStr;

use tracing::warn;

/// The Arch `vulkan-swrast` lavapipe ICD (R-158 item 5). Current packages ship
/// `lvp_icd.json`, older ones `lvp_icd.x86_64.json`; the Vulkan loader skips a missing file.
const LAVAPIPE_ICD: &str =
    "/usr/share/vulkan/icd.d/lvp_icd.json:/usr/share/vulkan/icd.d/lvp_icd.x86_64.json";

/// The Mesa EGL vendor file. gpui always enables the GL backend, and without this glvnd
/// loads the NVIDIA EGL driver, which `LIBGL_ALWAYS_SOFTWARE` does not affect (cr0.25).
const MESA_EGL_VENDOR: &str = "/usr/share/glvnd/egl_vendor.d/50_mesa.json";

/// The variables to set for `gpu`. `read_device_id` maps a PCI bus id to its device id.
pub fn env_for(
    gpu: &str,
    user_zed_device_id: Option<&OsStr>,
    read_device_id: impl Fn(&str) -> Option<u16>,
) -> Vec<(&'static str, String)> {
    match gpu {
        "auto" => Vec::new(),
        "software" => software_env(),
        _ => match gpu.strip_prefix("pci:") {
            Some(_) if user_zed_device_id.is_some() => Vec::new(),
            Some(bus_id) => match valid_bus_id(bus_id)
                .then(|| read_device_id(bus_id))
                .flatten()
            {
                Some(id) => vec![("ZED_DEVICE_ID", format!("0x{id:04x}"))],
                None => {
                    warn!(target: "tasklens::ui", "Rendering GPU {bus_id} not found; using the default");
                    Vec::new()
                }
            },
            None => {
                warn!(target: "tasklens::ui", gpu, "Unknown rendering.gpu value; using the default");
                Vec::new()
            }
        },
    }
}

/// Vulkan limited to lavapipe and GL to Mesa llvmpipe, so every adapter gpui lists is a
/// software one.
fn software_env() -> Vec<(&'static str, String)> {
    vec![
        ("WGPU_ADAPTER_NAME", "llvmpipe".to_owned()),
        ("VK_ICD_FILENAMES", LAVAPIPE_ICD.to_owned()),
        ("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL_VENDOR.to_owned()),
        ("LIBGL_ALWAYS_SOFTWARE", "1".to_owned()),
    ]
}

/// The device id of the PCI device at `bus_id`, from `/sys/bus/pci/devices/<bus id>/device`
/// (text such as `0x2684`).
pub fn pci_device_id(bus_id: &str) -> Option<u16> {
    if !valid_bus_id(bus_id) {
        return None;
    }
    let text = std::fs::read_to_string(format!("/sys/bus/pci/devices/{bus_id}/device")).ok()?;
    let hex = text.trim();
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    u16::from_str_radix(hex, 16).ok()
}

/// True when `bus_id` only has `[0-9a-fA-F:.]` and cannot leave the devices folder.
fn valid_bus_id(bus_id: &str) -> bool {
    !bus_id.is_empty()
        && !bus_id.contains("..")
        && bus_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(_: &str) -> Option<u16> {
        Some(0x2684)
    }

    #[test]
    fn auto_none() {
        assert!(env_for("auto", None, found).is_empty());
    }

    #[test]
    fn pci_sets_device_id() {
        assert_eq!(
            env_for("pci:0000:01:00.0", None, |bus| {
                assert_eq!(bus, "0000:01:00.0");
                Some(0x2684)
            }),
            vec![("ZED_DEVICE_ID", "0x2684".to_owned())]
        );
    }

    #[test]
    fn user_value_wins() {
        assert!(env_for("pci:0000:01:00.0", Some(OsStr::new("0x1234")), found).is_empty());
    }

    #[test]
    fn bad_bus_id_rejected() {
        let never = |_: &str| -> Option<u16> { panic!("must not read the device file") };
        assert!(env_for("pci:../../etc", None, never).is_empty());
        assert!(env_for("pci:", None, never).is_empty());
        assert!(env_for("pci:0000:01:00.0", None, |_| None).is_empty());
        assert_eq!(pci_device_id("../../etc"), None);
    }

    #[test]
    fn software_limits_vulkan_and_gl() {
        let expected = vec![
            ("WGPU_ADAPTER_NAME", "llvmpipe".to_owned()),
            (
                "VK_ICD_FILENAMES",
                "/usr/share/vulkan/icd.d/lvp_icd.json:/usr/share/vulkan/icd.d/lvp_icd.x86_64.json"
                    .to_owned(),
            ),
            (
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                "/usr/share/glvnd/egl_vendor.d/50_mesa.json".to_owned(),
            ),
            ("LIBGL_ALWAYS_SOFTWARE", "1".to_owned()),
        ];
        assert_eq!(env_for("software", None, found), expected);
        assert_eq!(
            env_for("software", Some(OsStr::new("0x1234")), found),
            expected
        );
    }

    #[test]
    fn unknown_value_none() {
        assert!(env_for("nvidia", None, found).is_empty());
    }
}
