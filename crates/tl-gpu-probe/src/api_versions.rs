//! The one-shot `--api-versions` mode (ADR-007, R-087).
//!
//! Reads the Vulkan API version, the Vulkan driver info and the OpenGL `GL_VERSION` per GPU,
//! writes one `ApiVersions` frame to stdout and exits 0. Vulkan comes from `ash` and EGL from
//! `khronos-egl`, both loaded at run time, so a machine without them gets an empty list. This runs
//! in its own short-lived process, so a driver crash costs only these rows.
//!
//! This is one of the five files allowed to hold `unsafe` (ADR-001 rule 4, R-143).
#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

use ash::vk;
use khronos_egl as egl;
use tl_model::gpu_proto::{ApiVersions, GpuApiVersions};
use tl_model::wire::{MAX_PROBE_FRAME, write_frame};

// EGL extension values (EGL_EXT_platform_device, EGL_EXT_device_drm).
const EGL_PLATFORM_DEVICE_EXT: egl::Enum = 0x313F;
const EGL_DRM_DEVICE_FILE_EXT: egl::Int = 0x3233;
const GL_VERSION: u32 = 0x1F02;
/// Upper bound on EGL devices asked for in one `eglQueryDevicesEXT` call.
const MAX_EGL_DEVICES: usize = 32;

type EglDevice = *mut c_void;
type QueryDevicesExt =
    unsafe extern "system" fn(egl::Int, *mut EglDevice, *mut egl::Int) -> egl::Boolean;
type QueryDeviceStringExt = unsafe extern "system" fn(EglDevice, egl::Int) -> *const c_char;
type GetPlatformDisplayExt =
    unsafe extern "system" fn(egl::Enum, *mut c_void, *const egl::Int) -> egl::EGLDisplay;
type GlGetString = unsafe extern "system" fn(u32) -> *const u8;

/// Collects the versions, writes one frame to stdout and exits 0. A panic anywhere in the
/// collection still exits 0, with an empty list.
pub fn run() -> ExitCode {
    let devices = catch_unwind(collect).unwrap_or_default();
    let mut out = std::io::stdout().lock();
    let written = write_frame(&mut out, &ApiVersions { devices }, MAX_PROBE_FRAME)
        .map_err(|e| e.to_string())
        .and_then(|()| out.flush().map_err(|e| e.to_string()));
    if let Err(e) = written {
        // The engine treats a missing frame as Unknown fields (E13-011).
        eprintln!("tl-gpu-probe: writing the api-versions frame failed: {e}");
    }
    ExitCode::SUCCESS
}

/// One entry per GPU seen by Vulkan, then one per GPU seen only by EGL.
pub fn collect() -> Vec<GpuApiVersions> {
    let mut out: Vec<GpuApiVersions> = vulkan_devices()
        .into_iter()
        .map(|(pci_bus_id, api, driver)| GpuApiVersions {
            opengl: opengl_version(&pci_bus_id),
            pci_bus_id,
            vulkan_api: Some(api),
            vulkan_driver: driver,
        })
        .collect();
    for pci_bus_id in egl_pci_bus_ids() {
        if out.iter().all(|d| d.pci_bus_id != pci_bus_id) {
            out.push(GpuApiVersions {
                opengl: opengl_version(&pci_bus_id),
                pci_bus_id,
                vulkan_api: None,
                vulkan_driver: None,
            });
        }
    }
    out
}

/// `(pci_bus_id, api version, driver info)` for each Vulkan device that has
/// `VK_EXT_pci_bus_info`. A GPU listed by two drivers is kept once, first one wins.
pub fn vulkan_devices() -> Vec<(String, String, Option<String>)> {
    // SAFETY: loads the system Vulkan loader. The loader's global constructors run here, which
    // is the reason this mode runs in its own process.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        return Vec::new();
    };
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let info = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: `info` and `app` are valid for the call and name no layers or extensions.
    let Ok(instance) = (unsafe { entry.create_instance(&info, None) }) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, Option<String>)> = Vec::new();
    // SAFETY: `instance` is live until `destroy_instance` below.
    let physical = unsafe { instance.enumerate_physical_devices() }.unwrap_or_default();
    for pd in physical {
        if let Some(dev) = vulkan_device(&instance, pd)
            && out.iter().all(|d| d.0 != dev.0)
        {
            out.push(dev);
        }
    }
    // SAFETY: no object made from `instance` is still alive and it is not used after this.
    unsafe { instance.destroy_instance(None) };
    out
}

fn vulkan_device(
    instance: &ash::Instance,
    pd: vk::PhysicalDevice,
) -> Option<(String, String, Option<String>)> {
    // SAFETY: `pd` came from `enumerate_physical_devices` on this live instance.
    let props = unsafe { instance.get_physical_device_properties(pd) };
    // vkGetPhysicalDeviceProperties2 is core only from Vulkan 1.1.
    if props.api_version < vk::API_VERSION_1_1 {
        return None;
    }
    // SAFETY: as above.
    let exts = unsafe { instance.enumerate_device_extension_properties(pd) }.ok()?;
    let has = |name: &CStr| exts.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
    if !has(vk::EXT_PCI_BUS_INFO_NAME) {
        return None;
    }
    // VkPhysicalDeviceDriverProperties is core from Vulkan 1.2.
    let has_driver =
        has(vk::KHR_DRIVER_PROPERTIES_NAME) || props.api_version >= vk::API_VERSION_1_2;

    let mut pci = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
    let mut driver = vk::PhysicalDeviceDriverProperties::default();
    let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut pci);
    if has_driver {
        props2 = props2.push_next(&mut driver);
    }
    // SAFETY: the device supports Vulkan 1.1 and every chained struct belongs to an extension
    // the device lists (or to core 1.2), so the driver may write all of them.
    unsafe { instance.get_physical_device_properties2(pd, &mut props2) };
    let api = version_string(props2.properties.api_version);

    let driver_info = if has_driver {
        driver
            .driver_info_as_c_str()
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let id = pci_bus_id_from_vk(
        pci.pci_domain,
        pci.pci_bus,
        pci.pci_device,
        pci.pci_function,
    );
    Some((id, api, driver_info))
}

/// The `GL_VERSION` string of an OpenGL context on the EGL device of the GPU at `pci_bus_id`.
/// `None` when EGL, the device or any step of making the context is missing.
pub fn opengl_version(pci_bus_id: &str) -> Option<String> {
    let want = normalise_pci_slot(pci_bus_id)?;
    // Any failure here costs only the OpenGL field, not the Vulkan rows.
    catch_unwind(AssertUnwindSafe(|| {
        let lib = load_egl()?;
        let ext = DeviceExt::load(&lib)?;
        let dev = ext.devices().into_iter().find(|(_, id)| *id == want)?.0;
        gl_version_on_device(&lib, &ext, dev)
    }))
    .ok()
    .flatten()
}

/// PCI bus IDs of the GPUs that EGL lists as DRM devices.
fn egl_pci_bus_ids() -> Vec<String> {
    catch_unwind(|| {
        let Some(lib) = load_egl() else {
            return Vec::new();
        };
        let Some(ext) = DeviceExt::load(&lib) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = ext.devices().into_iter().map(|(_, id)| id).collect();
        ids.dedup();
        ids
    })
    .unwrap_or_default()
}

/// NVML form of a Vulkan PCI address, for example `00000000:2D:00.0`.
pub fn pci_bus_id_from_vk(domain: u32, bus: u32, device: u32, function: u32) -> String {
    format!("{domain:08X}:{bus:02X}:{device:02X}.{function}")
}

/// A Vulkan version number as `major.minor.patch`, for example `1.4.312`.
pub fn version_string(v: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(v),
        vk::api_version_minor(v),
        vk::api_version_patch(v)
    )
}

/// Turns a PCI slot name in any common form (`0000:2d:00.0` from sysfs, `00000000:2D:00.0` from
/// NVML) into the NVML form. `None` when it does not parse.
fn normalise_pci_slot(s: &str) -> Option<String> {
    let (domain, rest) = s.trim().split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    Some(pci_bus_id_from_vk(
        u32::from_str_radix(domain, 16).ok()?,
        u32::from_str_radix(bus, 16).ok()?,
        u32::from_str_radix(device, 16).ok()?,
        function.parse().ok()?,
    ))
}

/// PCI slot of a DRM node such as `/dev/dri/card1`, read from `/sys/class/drm/card1/device`.
fn pci_slot_of_drm_node(node: &str) -> Option<String> {
    let name = std::path::Path::new(node).file_name()?;
    let link = std::fs::read_link(
        std::path::Path::new("/sys/class/drm")
            .join(name)
            .join("device"),
    )
    .ok()?;
    normalise_pci_slot(link.file_name()?.to_str()?)
}

fn load_egl() -> Option<egl::DynamicInstance<egl::EGL1_5>> {
    // SAFETY: loads the system libEGL. Like the Vulkan loader, its constructors run in this
    // short-lived process only.
    unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() }.ok()
}

/// The EGL device extension entry points.
struct DeviceExt {
    query_devices: QueryDevicesExt,
    query_device_string: QueryDeviceStringExt,
    get_platform_display: GetPlatformDisplayExt,
}

impl DeviceExt {
    fn load(lib: &egl::DynamicInstance<egl::EGL1_5>) -> Option<Self> {
        let client = lib.query_string(None, egl::EXTENSIONS).ok()?.to_bytes();
        let has = |name: &[u8]| client.split(|&b| b == b' ').any(|e| e == name);
        if !has(b"EGL_EXT_platform_device")
            || !(has(b"EGL_EXT_device_enumeration") || has(b"EGL_EXT_device_base"))
        {
            return None;
        }
        let query_devices = lib.get_proc_address("eglQueryDevicesEXT")?;
        let query_device_string = lib.get_proc_address("eglQueryDeviceStringEXT")?;
        let get_platform_display = lib.get_proc_address("eglGetPlatformDisplayEXT")?;
        // SAFETY: the client extension string lists these extensions, so each pointer is the
        // entry point of that name with the signature the extension specification gives.
        unsafe {
            Some(Self {
                query_devices: std::mem::transmute::<extern "system" fn(), QueryDevicesExt>(
                    query_devices,
                ),
                query_device_string: std::mem::transmute::<
                    extern "system" fn(),
                    QueryDeviceStringExt,
                >(query_device_string),
                get_platform_display: std::mem::transmute::<
                    extern "system" fn(),
                    GetPlatformDisplayExt,
                >(get_platform_display),
            })
        }
    }

    /// Each EGL device with a DRM node, paired with the NVML-form PCI bus ID of that node.
    fn devices(&self) -> Vec<(EglDevice, String)> {
        let mut devs: Vec<EglDevice> = vec![std::ptr::null_mut(); MAX_EGL_DEVICES];
        let mut n: egl::Int = 0;
        // SAFETY: `devs` holds MAX_EGL_DEVICES slots and the driver writes at most that many.
        let ok =
            unsafe { (self.query_devices)(MAX_EGL_DEVICES as egl::Int, devs.as_mut_ptr(), &mut n) };
        if ok != egl::TRUE {
            return Vec::new();
        }
        devs.truncate(usize::try_from(n).unwrap_or(0).min(MAX_EGL_DEVICES));
        devs.into_iter()
            .filter_map(|dev| {
                let node = self.device_string(dev, EGL_DRM_DEVICE_FILE_EXT)?;
                Some((dev, pci_slot_of_drm_node(&node)?))
            })
            .collect()
    }

    fn device_string(&self, dev: EglDevice, name: egl::Int) -> Option<String> {
        // SAFETY: `dev` came from eglQueryDevicesEXT. An unknown name returns null, which is
        // checked before reading.
        let p = unsafe { (self.query_device_string)(dev, name) };
        if p.is_null() {
            return None;
        }
        // SAFETY: a non-null result is a NUL-terminated string owned by EGL for the device's life.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Makes an OpenGL context on `dev`, reads `GL_VERSION` and tears everything down.
fn gl_version_on_device(
    lib: &egl::DynamicInstance<egl::EGL1_5>,
    ext: &DeviceExt,
    dev: EglDevice,
) -> Option<String> {
    let attribs = [egl::NONE];
    // SAFETY: `dev` came from eglQueryDevicesEXT and the attribute list ends with EGL_NONE.
    let raw = unsafe { (ext.get_platform_display)(EGL_PLATFORM_DEVICE_EXT, dev, attribs.as_ptr()) };
    if raw == egl::NO_DISPLAY {
        return None;
    }
    // SAFETY: `raw` is a display handle EGL just returned.
    let display = unsafe { egl::Display::from_ptr(raw) };
    lib.initialize(display).ok()?;
    let version = gl_version_on_display(lib, display);
    let _ = lib.terminate(display);
    version
}

fn gl_version_on_display(
    lib: &egl::DynamicInstance<egl::EGL1_5>,
    display: egl::Display,
) -> Option<String> {
    lib.bind_api(egl::OPENGL_API).ok()?;
    let config_attribs = [
        egl::SURFACE_TYPE,
        egl::PBUFFER_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_BIT,
        egl::NONE,
    ];
    let config = lib.choose_first_config(display, &config_attribs).ok()??;
    let context = lib
        .create_context(display, config, None, &[egl::NONE])
        .ok()?;

    let surfaceless = lib
        .query_string(Some(display), egl::EXTENSIONS)
        .is_ok_and(|s| {
            s.to_bytes()
                .split(|&b| b == b' ')
                .any(|e| e == b"EGL_KHR_surfaceless_context")
        });
    let surface = if surfaceless {
        None
    } else {
        let pbuffer = [egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE];
        match lib.create_pbuffer_surface(display, config, &pbuffer) {
            Ok(s) => Some(s),
            Err(_) => {
                let _ = lib.destroy_context(display, context);
                return None;
            }
        }
    };

    let version = lib
        .make_current(display, surface, surface, Some(context))
        .ok()
        .and_then(|()| read_gl_version(lib));

    let _ = lib.make_current(display, None, None, None);
    if let Some(s) = surface {
        let _ = lib.destroy_surface(display, s);
    }
    let _ = lib.destroy_context(display, context);
    version
}

/// `glGetString(GL_VERSION)` on the current context.
fn read_gl_version(lib: &egl::DynamicInstance<egl::EGL1_5>) -> Option<String> {
    let f = lib.get_proc_address("glGetString")?;
    // SAFETY: eglGetProcAddress returns the GL entry point of that name, whose signature is
    // `const GLubyte *glGetString(GLenum)`.
    let get_string = unsafe { std::mem::transmute::<extern "system" fn(), GlGetString>(f) };
    // SAFETY: a context is current on this thread, so the GL call is valid.
    let p = unsafe { get_string(GL_VERSION) };
    if p.is_null() {
        return None;
    }
    // SAFETY: a non-null GL_VERSION result is a static NUL-terminated string owned by the driver.
    let s = unsafe { CStr::from_ptr(p.cast::<c_char>()) };
    Some(s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use tl_model::wire::read_frame;

    #[test]
    fn pci_bus_id_format() {
        assert_eq!(pci_bus_id_from_vk(0, 0x2d, 0, 0), "00000000:2D:00.0");
        assert_eq!(
            normalise_pci_slot("0000:2d:00.0").as_deref(),
            Some("00000000:2D:00.0")
        );
        assert_eq!(
            normalise_pci_slot("00000000:2D:00.0").as_deref(),
            Some("00000000:2D:00.0")
        );
        assert_eq!(normalise_pci_slot("not a slot"), None);
    }

    #[test]
    fn version_string_format() {
        assert_eq!(
            version_string(vk::make_api_version(0, 1, 4, 312)),
            "1.4.312"
        );
    }

    /// The probe binary next to this test binary (`target/<profile>/tl-gpu-probe`).
    fn probe_binary() -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe.parent().and_then(|d| d.parent()).expect("target dir");
        dir.join("tl-gpu-probe")
    }

    #[test]
    #[ignore = "needs the owner machine (RTX GPU at 2D:00.0, driver 615)"]
    fn real_api_versions_owner() {
        let bin = probe_binary();
        assert!(bin.exists(), "build the binary first: {}", bin.display());
        let start = Instant::now();
        let out = std::process::Command::new(&bin)
            .arg("--api-versions")
            .output()
            .expect("run tl-gpu-probe");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "took {:?}",
            start.elapsed()
        );
        assert!(out.status.success(), "exit {:?}", out.status);
        let frame: ApiVersions =
            read_frame(&mut out.stdout.as_slice(), MAX_PROBE_FRAME).expect("one frame");
        assert_eq!(frame.devices.len(), 1, "{frame:?}");
        let d = &frame.devices[0];
        assert_eq!(d.pci_bus_id, "00000000:2D:00.0");
        assert!(
            d.vulkan_api.as_deref().is_some_and(|v| v.starts_with("1.")),
            "{d:?}"
        );
        assert!(
            d.vulkan_driver
                .as_deref()
                .is_some_and(|v| v.contains("615")),
            "{d:?}"
        );
        assert!(
            d.opengl.as_deref().is_some_and(|v| v.starts_with("4.")),
            "{d:?}"
        );
    }

    #[test]
    fn no_drivers_gives_empty() {
        // SAFETY: loads the system Vulkan loader only to see whether one exists.
        let vulkan = unsafe { ash::Entry::load() }.is_ok();
        if vulkan || load_egl().is_some() {
            eprintln!("skipped: a Vulkan loader or libEGL is present");
            return;
        }
        assert!(collect().is_empty());
    }
}
