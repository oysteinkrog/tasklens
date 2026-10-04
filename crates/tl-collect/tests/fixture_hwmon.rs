//! hwmon enumeration and value parsers against the `owner-desktop` capture and the hand-made
//! `hwmon-coretemp`, `fans` and `no-fans` fixtures (PERF-039, PERF-076, PERF-077, PERF-108).

mod common;

use std::fmt::Write as _;

use tl_collect::CollectError;
use tl_collect::parse::hwmon::{
    FanControl, HwmonChip, any_fan, cpu_temp_input, default_temp_pair, enumerate,
    parse_fan_control, parse_millidegrees, parse_pwm_pct,
};

fn chips(name: &str) -> Vec<HwmonChip> {
    let mut out = Vec::new();
    enumerate(&common::fixture(name), &mut out).expect("enumerate hwmon");
    out
}

/// One block per chip, with `dir` relative to the fixture so the snapshot is machine independent.
fn render(fixture: &str, chips: &[HwmonChip]) -> String {
    let base = common::fixture_dir(fixture);
    let mut text = String::new();
    for chip in chips {
        let dir = chip.dir.strip_prefix(&base).expect("chip dir under fixture");
        writeln!(text, "hwmon{} {:?} dir={}", chip.index, chip.name, dir.display()).unwrap();
        writeln!(text, "  device={:?}", chip.device_path).unwrap();
        for t in &chip.temps {
            writeln!(text, "  temp{} label={:?}", t.n, t.label).unwrap();
        }
        for f in &chip.fans {
            writeln!(text, "  fan{} label={:?} has_max={}", f.n, f.label, f.has_max).unwrap();
        }
        for p in &chip.pwms {
            writeln!(text, "  pwm{} has_enable={}", p.n, p.has_enable).unwrap();
        }
    }
    text
}

#[test]
fn owner_desktop_snapshot() {
    let chips = chips("owner-desktop");
    assert!(chips.iter().any(|c| &*c.name == "k10temp"));
    let nct = chips
        .iter()
        .find(|c| &*c.name == "nct6797")
        .expect("nct6797 chip");
    let fans: Vec<u16> = nct.fans.iter().map(|f| f.n).collect();
    assert_eq!(fans, [1, 2, 3, 4, 5, 6, 7]);
    let indexes: Vec<u32> = chips.iter().map(|c| c.index).collect();
    assert!(indexes.is_sorted());
    insta::assert_snapshot!(render("owner-desktop", &chips));
}

#[test]
fn owner_desktop_cpu_temp_is_tctl() {
    let chips = chips("owner-desktop");
    let (i, n) = cpu_temp_input(&chips).expect("cpu temp");
    assert_eq!(&*chips[i].name, "k10temp");
    let temp = chips[i].temps.iter().find(|t| t.n == n).expect("temp input");
    assert_eq!(temp.label.as_deref(), Some("Tctl"));
}

#[test]
fn coretemp_package_label() {
    let chips = chips("hwmon-coretemp");
    assert_eq!(cpu_temp_input(&chips), Some((0, 1)));
    assert_eq!(chips[0].temps[0].label.as_deref(), Some("Package id 0"));
    assert_eq!(chips[0].device_path, None);
}

#[test]
fn any_fan_cases() {
    assert!(!any_fan(&chips("no-fans")));
    assert!(any_fan(&chips("fans")));
    assert!(!any_fan(&[]));
}

#[test]
fn default_pairing_single_fan_only() {
    let chips = chips("fans");
    let single = chips.iter().find(|c| &*c.name == "single").expect("single");
    assert_eq!(default_temp_pair(single, 1), Some(1));
    assert_eq!(default_temp_pair(single, 2), None);
    let nct = chips.iter().find(|c| &*c.name == "nct6797").expect("nct6797");
    assert_eq!(nct.fans.len(), 2);
    assert_eq!(nct.fans[1].label, None);
    assert_eq!(nct.pwms.len(), 1);
    assert!(nct.pwms[0].has_enable);
    assert_eq!(default_temp_pair(nct, 1), None);
}

#[test]
fn pwm_pct() {
    assert_eq!(parse_pwm_pct(b"128\n").unwrap(), 50.19608);
    assert_eq!(parse_pwm_pct(b"0").unwrap(), 0.0);
    assert_eq!(parse_pwm_pct(b"255").unwrap(), 100.0);
    assert!(matches!(
        parse_pwm_pct(b"256"),
        Err(CollectError::Parse { .. })
    ));
}

#[test]
fn fan_control_mapping() {
    let cases = [
        (&b"0"[..], FanControl::FullSpeed),
        (b"1", FanControl::Manual),
        (b"2", FanControl::Automatic),
        (b"5", FanControl::Automatic),
    ];
    for (input, want) in cases {
        assert_eq!(parse_fan_control(input).unwrap(), want);
    }
}

#[test]
fn millidegrees() {
    assert_eq!(parse_millidegrees(b"-5000\n").unwrap(), -5.0);
    assert_eq!(parse_millidegrees(b"48250").unwrap(), 48.25);
}
