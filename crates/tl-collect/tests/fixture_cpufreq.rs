//! `CpuFreq` and `read_base_mhz` against the captured `owner-desktop` fixture and the hand-made
//! cpufreq edge fixtures (E04-010).

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

mod common;

use std::fs;

use tl_collect::SysRoot;
use tl_collect::counters::Measured;
use tl_collect::parse::cpufreq::{CpuFreq, FreqReading, FreqSource, read_base_mhz};
use tl_model::validity::Validity;

fn read(root: &SysRoot) -> FreqReading {
    let mut freq = CpuFreq::open(root);
    let mut out = FreqReading::default();
    freq.read(&mut out);
    assert_eq!(freq.source(), out.source);
    out
}

fn values(m: &[Measured<u32>]) -> Vec<u32> {
    assert!(m.iter().all(|m| m.validity == Validity::Valid), "{m:?}");
    m.iter().map(|m| m.value).collect()
}

/// kHz file content to MHz, rounded the way the parser rounds.
fn mhz_of(bytes: &[u8]) -> u64 {
    let khz: u64 = std::str::from_utf8(bytes).unwrap().trim().parse().unwrap();
    (khz + 500) / 1000
}

#[test]
fn owner_desktop_average() {
    let out = read(&common::fixture("owner-desktop"));

    let dir = common::fixture_dir("owner-desktop").join("sys/devices/system/cpu/cpufreq");
    let mut sum = 0u64;
    let mut count = 0u64;
    let mut highest = 0usize;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let avg = path.join("cpuinfo_avg_freq");
        let file = if avg.exists() {
            avg
        } else {
            path.join("scaling_cur_freq")
        };
        sum += mhz_of(&fs::read(file).unwrap());
        count += 1;
        let related = fs::read_to_string(path.join("related_cpus")).unwrap();
        for part in related.trim().split(',') {
            let last = part.rsplit('-').next().unwrap();
            highest = highest.max(last.parse().unwrap());
        }
    }
    assert!(count > 0);
    let mean = u32::try_from((sum + count / 2) / count).unwrap();

    assert_eq!(out.avg_mhz, Measured::valid(mean));
    assert_eq!(out.per_cpu_mhz.len(), highest + 1);
    insta::assert_debug_snapshot!(out);
}

#[test]
fn mixed_sources() {
    let out = read(&common::fixture("cpufreq-mixed"));
    assert_eq!(out.source, FreqSource::ScalingCur);
    assert_eq!(values(&out.per_cpu_mhz), [4200, 4200, 2800, 2800]);
    assert_eq!(out.avg_mhz, Measured::valid(3500));
    assert_eq!(out.max_mhz, Measured::valid(4200));
}

#[test]
fn cpuinfo_fallback() {
    let root = common::fixture("no-cpufreq");
    let mut freq = CpuFreq::open(&root);
    assert_eq!(freq.source(), FreqSource::CpuinfoMhz);
    let mut out = FreqReading::default();
    for _ in 0..2 {
        freq.read(&mut out);
        assert_eq!(out.source, FreqSource::CpuinfoMhz);
        assert_eq!(out.avg_mhz, Measured::valid(3500));
        assert_eq!(out.max_mhz, Measured::valid(3600));
        assert_eq!(values(&out.per_cpu_mhz), [3400, 3600]);
    }
}

#[test]
fn bogomips_fallback() {
    let out = read(&common::fixture("bogomips-only"));
    assert_eq!(out.source, FreqSource::BogoMips);
    assert_eq!(out.avg_mhz, Measured::valid(50));
    assert_eq!(values(&out.per_cpu_mhz), [50, 50]);
}

#[test]
fn nothing_readable_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let out = read(&SysRoot::at(dir.path()));
    assert_eq!(out.source, FreqSource::None);
    assert_eq!(out.avg_mhz.validity, Validity::Unsupported);
    assert_eq!(out.max_mhz.validity, Validity::Unsupported);
    assert!(out.per_cpu_mhz.is_empty());
}

#[test]
fn base_speed_order() {
    assert_eq!(
        read_base_mhz(&common::fixture("cpufreq-mixed")),
        Measured::valid(3700)
    );

    let copy = common::fixture_copy("cpufreq-mixed", None);
    let cpu = copy.path().join("sys/devices/system/cpu");
    fs::write(cpu.join("cpufreq/policy0/bios_limit"), "3200000\n").unwrap();
    fs::write(cpu.join("cpufreq/policy0/base_frequency"), "3900000\n").unwrap();
    let root = SysRoot::at(copy.path());
    assert_eq!(read_base_mhz(&root), Measured::valid(3900));

    fs::remove_file(cpu.join("cpufreq/policy0/base_frequency")).unwrap();
    assert_eq!(read_base_mhz(&root), Measured::valid(3700));

    fs::remove_file(cpu.join("cpu0/acpi_cppc/nominal_freq")).unwrap();
    assert_eq!(read_base_mhz(&root), Measured::valid(3200));

    fs::remove_file(cpu.join("cpufreq/policy0/bios_limit")).unwrap();
    assert_eq!(
        read_base_mhz(&root).validity,
        Validity::Unsupported,
        "no base speed file left"
    );
}

#[test]
fn policy_error_is_per_policy() {
    let copy = common::fixture_copy("cpufreq-mixed", None);
    let file = copy
        .path()
        .join("sys/devices/system/cpu/cpufreq/policy2/scaling_cur_freq");
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();

    let out = read(&SysRoot::at(copy.path()));
    let validity: Vec<Validity> = out.per_cpu_mhz.iter().map(|m| m.validity).collect();
    assert_eq!(
        validity,
        [
            Validity::Valid,
            Validity::Valid,
            Validity::Failed,
            Validity::Failed
        ]
    );
    assert_eq!(out.per_cpu_mhz[0], Measured::valid(4200));
    assert_eq!(out.avg_mhz, Measured::valid(4200));
    assert_eq!(out.max_mhz, Measured::valid(4200));
}
