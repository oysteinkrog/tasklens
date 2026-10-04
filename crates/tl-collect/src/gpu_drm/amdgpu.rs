//! AMD GPU metrics from sysfs, hwmon and `gpu_metrics` (PERF-064 to PERF-069, PERF-074).
//!
//! Untested on real AMD hardware; the `drm-amd` fixture carries the tests. A file that is missing,
//! unreadable or does not parse leaves its field `None`, which the UI shows as "Not supported".

use std::path::{Path, PathBuf};

use smallvec::SmallVec;

use super::DrmCard;
use crate::sysroot::{SMALL_FILE_LIMIT, read_file, read_i64, read_trimmed, read_u64};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmdSample {
    /// `device/gpu_busy_percent`.
    pub gpu_busy_pct: Option<u8>,
    /// `device/mem_busy_percent`.
    pub mem_busy_pct: Option<u8>,
    /// `device/mem_info_vram_used`, bytes.
    pub vram_used: Option<u64>,
    /// `device/mem_info_vram_total`, bytes.
    pub vram_total: Option<u64>,
    /// `device/mem_info_gtt_used`, bytes.
    pub gtt_used: Option<u64>,
    /// `device/mem_info_gtt_total`, bytes.
    pub gtt_total: Option<u64>,
    /// hwmon `freq1_input`, Hz / 1_000_000.
    pub sclk_mhz: Option<u32>,
    /// hwmon `freq2_input`, Hz / 1_000_000.
    pub mclk_mhz: Option<u32>,
    /// Last line of `device/pp_dpm_sclk`.
    pub sclk_max_mhz: Option<u32>,
    /// Last line of `device/pp_dpm_mclk`.
    pub mclk_max_mhz: Option<u32>,
    /// hwmon `power1_input`, else `power1_average`, microwatts.
    pub power_uw: Option<u64>,
    /// hwmon `power1_cap`, microwatts.
    pub power_cap_uw: Option<u64>,
    /// Millidegrees C, from hwmon `temp1_input` to `temp3_input` with a known `temp*_label`.
    pub temps: SmallVec<[(AmdTemp, i32); 3]>,
    /// hwmon `fan1_input`.
    pub fan_rpm: Option<u32>,
    /// hwmon `pwm1`, 0 to 255.
    pub fan_pwm: Option<u8>,
    /// `device/gpu_metrics`.
    pub metrics: Option<GpuMetrics>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmdTemp {
    Edge,
    Junction,
    Mem,
}

/// Fields of `device/gpu_metrics`. A field the struct revision lacks, or that holds `0xFFFF`, is
/// `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuMetrics {
    pub format_revision: u8,
    pub content_revision: u8,
    pub average_gfx_activity: Option<u16>,
    pub average_umc_activity: Option<u16>,
    /// No struct from v1_0 to v1_3 or v2_0 to v2_4 has `vcn_activity`, so this stays empty for
    /// the revisions parsed today.
    pub vcn_activity: SmallVec<[u16; 4]>,
    pub temperature_edge: Option<u16>,
    pub temperature_hotspot: Option<u16>,
    pub temperature_mem: Option<u16>,
}

/// Cap for every file read here, `gpu_metrics` included.
const READ_LIMIT: usize = 4096;

/// Value the firmware writes for a field it does not report.
const NOT_REPORTED: u16 = 0xFFFF;

/// Byte offsets in the kernel structs of `drivers/gpu/drm/amd/include/kgd_pp_interface.h`
/// (x86_64 layout, natural alignment).
struct MetricsOffsets {
    temperature_edge: Option<usize>,
    temperature_hotspot: Option<usize>,
    temperature_mem: Option<usize>,
    average_gfx_activity: usize,
    average_umc_activity: Option<usize>,
}

/// `struct gpu_metrics_v1_0`: a u64 `system_clock_counter` comes first, padded to offset 8.
const V1_0: MetricsOffsets = MetricsOffsets {
    temperature_edge: Some(16),
    temperature_hotspot: Some(18),
    temperature_mem: Some(20),
    average_gfx_activity: 28,
    average_umc_activity: Some(30),
};

/// `struct gpu_metrics_v1_1`, `gpu_metrics_v1_2` and `gpu_metrics_v1_3`: the same leading fields.
const V1_1_TO_V1_3: MetricsOffsets = MetricsOffsets {
    temperature_edge: Some(4),
    temperature_hotspot: Some(6),
    temperature_mem: Some(8),
    average_gfx_activity: 16,
    average_umc_activity: Some(18),
};

/// `struct gpu_metrics_v2_0`: a u64 `system_clock_counter` at offset 8 comes before the
/// temperatures. v2 (APU) structs have no edge, hotspot or mem temperature and no UMC activity.
const V2_0: MetricsOffsets = MetricsOffsets {
    temperature_edge: None,
    temperature_hotspot: None,
    temperature_mem: None,
    average_gfx_activity: 40,
    average_umc_activity: None,
};

/// `struct gpu_metrics_v2_1` to `gpu_metrics_v2_4`: `system_clock_counter` moved after the
/// activity fields.
const V2_1_TO_V2_4: MetricsOffsets = MetricsOffsets {
    temperature_edge: None,
    temperature_hotspot: None,
    temperature_mem: None,
    average_gfx_activity: 28,
    average_umc_activity: None,
};

/// Fills `out` for one AMD card. `out` is reset first, so a file that stops being readable
/// clears its field.
pub fn read_amd(card: &DrmCard, sys: &Path, out: &mut AmdSample) {
    let mut buf = Vec::with_capacity(READ_LIMIT);
    let mut path = sys.join(format!("class/drm/card{}/device", card.card));

    out.gpu_busy_pct = read_num(&mut path, "gpu_busy_percent", &mut buf);
    out.mem_busy_pct = read_num(&mut path, "mem_busy_percent", &mut buf);
    out.vram_used = read_num(&mut path, "mem_info_vram_used", &mut buf);
    out.vram_total = read_num(&mut path, "mem_info_vram_total", &mut buf);
    out.gtt_used = read_num(&mut path, "mem_info_gtt_used", &mut buf);
    out.gtt_total = read_num(&mut path, "mem_info_gtt_total", &mut buf);
    out.sclk_max_mhz = read_dpm(&mut path, "pp_dpm_sclk", &mut buf);
    out.mclk_max_mhz = read_dpm(&mut path, "pp_dpm_mclk", &mut buf);
    path.push("gpu_metrics");
    out.metrics = read_file(&path, &mut buf, READ_LIMIT)
        .ok()
        .and_then(|_| parse_gpu_metrics(&buf));
    path.pop();

    out.sclk_mhz = None;
    out.mclk_mhz = None;
    out.power_uw = None;
    out.power_cap_uw = None;
    out.temps.clear();
    out.fan_rpm = None;
    out.fan_pwm = None;
    let Some(hwmon) = &card.hwmon else {
        return;
    };
    let mut path = hwmon.clone();
    out.sclk_mhz = read_mhz(&mut path, "freq1_input", &mut buf);
    out.mclk_mhz = read_mhz(&mut path, "freq2_input", &mut buf);
    out.power_uw = read_num(&mut path, "power1_input", &mut buf)
        .or_else(|| read_num(&mut path, "power1_average", &mut buf));
    out.power_cap_uw = read_num(&mut path, "power1_cap", &mut buf);
    for (input, label) in [
        ("temp1_input", "temp1_label"),
        ("temp2_input", "temp2_label"),
        ("temp3_input", "temp3_label"),
    ] {
        let Some(kind) = read_label(&mut path, label, &mut buf) else {
            continue;
        };
        path.push(input);
        let value = read_i64(&path, &mut buf).ok();
        path.pop();
        if let Some(milli) = value.and_then(|v| i32::try_from(v).ok()) {
            out.temps.push((kind, milli));
        }
    }
    out.fan_rpm = read_num(&mut path, "fan1_input", &mut buf);
    out.fan_pwm = read_num(&mut path, "pwm1", &mut buf);
}

/// Reads `dir/name` as one unsigned number that fits `T`.
fn read_num<T: TryFrom<u64>>(dir: &mut PathBuf, name: &str, buf: &mut Vec<u8>) -> Option<T> {
    dir.push(name);
    let value = read_u64(dir, buf).ok();
    dir.pop();
    value.and_then(|v| T::try_from(v).ok())
}

fn read_mhz(dir: &mut PathBuf, name: &str, buf: &mut Vec<u8>) -> Option<u32> {
    read_num::<u64>(dir, name, buf).and_then(|hz| u32::try_from(hz / 1_000_000).ok())
}

fn read_dpm(dir: &mut PathBuf, name: &str, buf: &mut Vec<u8>) -> Option<u32> {
    dir.push(name);
    let value = read_file(dir, buf, SMALL_FILE_LIMIT)
        .ok()
        .and_then(|_| parse_dpm_max_mhz(buf));
    dir.pop();
    value
}

fn read_label(dir: &mut PathBuf, name: &str, buf: &mut Vec<u8>) -> Option<AmdTemp> {
    dir.push(name);
    let kind = match read_trimmed(dir, buf) {
        Ok(b"edge") => Some(AmdTemp::Edge),
        Ok(b"junction") => Some(AmdTemp::Junction),
        Ok(b"mem") => Some(AmdTemp::Mem),
        _ => None,
    };
    dir.pop();
    kind
}

/// Parses `device/gpu_metrics`. The header is `structure_size: u16 LE`, `format_revision: u8`,
/// `content_revision: u8`. Format 1 content 0 to 3 and format 2 content 0 to 4 are parsed; any
/// other revision, or a buffer shorter than `structure_size`, gives `None`.
pub fn parse_gpu_metrics(bytes: &[u8]) -> Option<GpuMetrics> {
    let header = bytes.get(..4)?;
    let size = usize::from(u16::from_le_bytes([header[0], header[1]]));
    let (format_revision, content_revision) = (header[2], header[3]);
    let offsets = match (format_revision, content_revision) {
        (1, 0) => &V1_0,
        (1, 1..=3) => &V1_1_TO_V1_3,
        (2, 0) => &V2_0,
        (2, 1..=4) => &V2_1_TO_V2_4,
        _ => return None,
    };
    // Fields past `structure_size` are not part of the struct and read as `None`.
    let body = bytes.get(..size)?;
    let field = |offset: Option<usize>| {
        let raw = body.get(offset?..offset? + 2)?;
        let value = u16::from_le_bytes([raw[0], raw[1]]);
        (value != NOT_REPORTED).then_some(value)
    };
    Some(GpuMetrics {
        format_revision,
        content_revision,
        average_gfx_activity: field(Some(offsets.average_gfx_activity)),
        average_umc_activity: field(offsets.average_umc_activity),
        vcn_activity: SmallVec::new(),
        temperature_edge: field(offsets.temperature_edge),
        temperature_hotspot: field(offsets.temperature_hotspot),
        temperature_mem: field(offsets.temperature_mem),
    })
}

/// The MHz value on the last non-empty line of `pp_dpm_sclk` or `pp_dpm_mclk`:
/// `"2: 2500Mhz *"` gives 2500.
pub fn parse_dpm_max_mhz(text: &[u8]) -> Option<u32> {
    let line = text
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .rfind(|l| !l.is_empty())?;
    let colon = line.iter().position(|&b| b == b':')?;
    let rest = line[colon + 1..].trim_ascii_start();
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || !rest[digits..].to_ascii_lowercase().starts_with(b"mhz") {
        return None;
    }
    std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()
}

/// Main temperature: Junction when present, else Edge (PERF-068). Source text per PERF-069:
/// "hwmon amdgpu: junction", "hwmon amdgpu: edge" or "hwmon amdgpu: mem".
pub fn main_temp(s: &AmdSample) -> Option<(AmdTemp, i32)> {
    let find = |kind| s.temps.iter().copied().find(|&(k, _)| k == kind);
    find(AmdTemp::Junction).or_else(|| find(AmdTemp::Edge))
}

pub fn temp_source_text(t: AmdTemp) -> &'static str {
    match t {
        AmdTemp::Edge => "hwmon amdgpu: edge",
        AmdTemp::Junction => "hwmon amdgpu: junction",
        AmdTemp::Mem => "hwmon amdgpu: mem",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_drm::{VENDOR_AMD, discover_cards};

    fn fixture_sys() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-amd/sys")
    }

    fn sample_from_fixture() -> AmdSample {
        let sys = fixture_sys();
        let cards = discover_cards(&sys);
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert_eq!(cards[0].vendor, VENDOR_AMD);
        assert_eq!(cards[0].driver.as_deref(), Some("amdgpu"));
        let mut sample = AmdSample::default();
        read_amd(&cards[0], &sys, &mut sample);
        sample
    }

    /// A v1_3 blob of the struct's real size with every u16 field set to 0xFFFF.
    fn v1_3_blob() -> Vec<u8> {
        let mut blob = vec![0xFF; 120];
        blob[..4].copy_from_slice(&[120, 0, 1, 3]);
        blob
    }

    #[test]
    fn fixture_sample() {
        let s = sample_from_fixture();
        let expected = AmdSample {
            gpu_busy_pct: Some(37),
            vram_used: Some(3_006_477_107),
            vram_total: Some(17_095_983_104),
            sclk_mhz: Some(1905),
            sclk_max_mhz: Some(2500),
            power_uw: Some(31_900_000),
            power_cap_uw: Some(300_000_000),
            // temp3 has no label, so it is ignored.
            temps: SmallVec::from_slice(&[(AmdTemp::Edge, 52_000), (AmdTemp::Junction, 61_000)]),
            ..AmdSample::default()
        };
        assert_eq!(s, expected);
    }

    #[test]
    fn junction_preferred() {
        let s = sample_from_fixture();
        assert_eq!(main_temp(&s), Some((AmdTemp::Junction, 61_000)));
        assert_eq!(temp_source_text(AmdTemp::Junction), "hwmon amdgpu: junction");
        let edge_only = AmdSample {
            temps: SmallVec::from_slice(&[(AmdTemp::Mem, 70_000), (AmdTemp::Edge, 52_000)]),
            ..AmdSample::default()
        };
        assert_eq!(main_temp(&edge_only), Some((AmdTemp::Edge, 52_000)));
        assert_eq!(main_temp(&AmdSample::default()), None);
    }

    #[test]
    fn power_average_fallback() {
        let hwmon = fixture_sys().join("class/drm/card0/device/hwmon/hwmon3");
        assert!(!hwmon.join("power1_input").exists());
        assert!(hwmon.join("power1_average").exists());
        assert_eq!(sample_from_fixture().power_uw, Some(31_900_000));
    }

    #[test]
    fn unknown_metrics_revision() {
        let mut blob = v1_3_blob();
        blob[2] = 9;
        assert_eq!(parse_gpu_metrics(&blob), None);
        // Known format, unknown content revision.
        let mut blob = v1_3_blob();
        blob[3] = 4;
        assert_eq!(parse_gpu_metrics(&blob), None);
        // Buffer shorter than structure_size.
        assert_eq!(parse_gpu_metrics(&v1_3_blob()[..119]), None);
        assert_eq!(parse_gpu_metrics(&[120, 0, 1]), None);
    }

    #[test]
    fn metrics_v1_3_fields() {
        let mut blob = v1_3_blob();
        blob[16..18].copy_from_slice(&4200u16.to_le_bytes());
        blob[4..6].copy_from_slice(&5100u16.to_le_bytes());
        let m = parse_gpu_metrics(&blob).expect("v1_3 parses");
        assert_eq!((m.format_revision, m.content_revision), (1, 3));
        assert_eq!(m.average_gfx_activity, Some(4200));
        assert_eq!(m.temperature_hotspot, None);
        assert_eq!(m.temperature_edge, Some(5100));
        assert_eq!(m.average_umc_activity, None);
        assert!(m.vcn_activity.is_empty());
    }

    #[test]
    fn metrics_v1_0_and_v2_offsets() {
        let mut blob = vec![0xFF; 80];
        blob[..4].copy_from_slice(&[80, 0, 1, 0]);
        blob[28..30].copy_from_slice(&1234u16.to_le_bytes());
        blob[30..32].copy_from_slice(&55u16.to_le_bytes());
        let m = parse_gpu_metrics(&blob).expect("v1_0 parses");
        assert_eq!(m.average_gfx_activity, Some(1234));
        assert_eq!(m.average_umc_activity, Some(55));

        let mut blob = vec![0; 168];
        blob[..4].copy_from_slice(&[168, 0, 2, 4]);
        blob[28..30].copy_from_slice(&777u16.to_le_bytes());
        let m = parse_gpu_metrics(&blob).expect("v2_4 parses");
        assert_eq!(m.average_gfx_activity, Some(777));
        assert_eq!(m.temperature_edge, None);
        assert_eq!(m.average_umc_activity, None);

        let mut blob = vec![0; 120];
        blob[..4].copy_from_slice(&[120, 0, 2, 0]);
        blob[40..42].copy_from_slice(&900u16.to_le_bytes());
        assert_eq!(
            parse_gpu_metrics(&blob).and_then(|m| m.average_gfx_activity),
            Some(900)
        );
    }

    #[test]
    fn dpm_max() {
        assert_eq!(parse_dpm_max_mhz(b"0: 500Mhz\n1: 1905Mhz\n2: 2500Mhz *\n"), Some(2500));
        assert_eq!(parse_dpm_max_mhz(b"0: 96Mhz *\n1: 1000MHz\n\n"), Some(1000));
        assert_eq!(parse_dpm_max_mhz(b""), None);
        assert_eq!(parse_dpm_max_mhz(b"2: fast"), None);
        assert_eq!(parse_dpm_max_mhz(b"2: 2500"), None);
    }
}
