//! Migration fixtures for every old GPU box setting (phase 1 gate, tasklens-lhliq.8, ADR-019).
//! Each file in `tests/fixtures/layouts` is a `settings.toml` as 1.1.0 wrote it. Loading it must
//! give the expected flow layout, converting twice must change nothing, and the old keys must
//! stay in the file for an older build.

use tl_model::layout::{DeviceSelector, GPU_BOX_CONTAINER};
use tl_model::settings::{LAYOUTS_CONVERTED_KEY, Settings};

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/layouts/{name}.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn load(text: &str) -> (Settings, Vec<&'static str>) {
    let mut s: Settings = toml::from_str(text).expect("valid TOML");
    let changed = s.clamp();
    (s, changed)
}

/// One row per fixture: name, preset id, arrange, split, widgets per slot.
type Row = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<f64>,
    usize,
);

const ROWS: [Row; 9] = [
    ("mode_engine_charts", "engine_charts", None, None, 13),
    ("mode_chart_engines", "chart_engines", None, None, 15),
    ("mode_chart", "chart", None, None, 2),
    ("arrange_only", "engine_charts", Some("vertical"), None, 13),
    ("split_only", "engine_charts", None, Some(0.6), 13),
    // Video Encode/Decode hides enc and dec, Graphics hides gpu: 13 - 3.
    ("hidden_engines", "engine_charts", None, None, 10),
    // VRAM hidden in `chart` leaves the utilization chart only.
    ("hidden_vram_chart", "chart", None, None, 1),
    // chart_engines 15, less VRAM and Compute.
    (
        "every_key",
        "chart_engines",
        Some("horizontal"),
        Some(0.35),
        13,
    ),
    // The bad mode and the out-of-range split are dropped.
    ("invalid_values", "engine_charts", None, None, 13),
];

#[test]
fn every_old_gpu_setting_converts_to_its_flow_layout() {
    for (name, id, arrange, split, per_slot) in ROWS {
        let (s, changed) = load(&fixture(name));
        assert!(
            changed.contains(&LAYOUTS_CONVERTED_KEY),
            "{name}: {changed:?}"
        );
        let l = s
            .performance
            .layouts
            .known(GPU_BOX_CONTAINER)
            .unwrap_or_else(|| panic!("{name}: no overview.gpu entry"));
        let p = l
            .preset
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: not a flow layout"));
        assert_eq!(p.id, id, "{name}");
        assert_eq!(p.arrange.as_deref(), arrange, "{name}");
        assert_eq!(p.split, split, "{name}");
        for slot in 0..=1u8 {
            let n = l
                .widgets
                .iter()
                .filter(|w| w.source.device == DeviceSelector::Slot(slot))
                .count();
            assert_eq!(n, per_slot, "{name}: slot {slot}");
        }
        assert_eq!(l.widgets.len(), per_slot * 2, "{name}");
        assert!(l.widgets.iter().all(|w| w.cell.is_none()), "{name}");
        // Ids run 1..=n and the next id follows.
        let ids: Vec<u32> = l.widgets.iter().map(|w| w.id).collect();
        assert_eq!(ids, (1..=ids.len() as u32).collect::<Vec<_>>(), "{name}");
        assert_eq!(l.next_id as usize, ids.len() + 1, "{name}");
    }
}

#[test]
fn conversion_twice_changes_nothing() {
    for (name, ..) in ROWS {
        let (first, _) = load(&fixture(name));
        let text = toml::to_string(&first).expect("serializes");
        let (second, changed) = load(&text);
        assert!(
            !changed.contains(&LAYOUTS_CONVERTED_KEY),
            "{name}: {changed:?}"
        );
        assert_eq!(second, first, "{name}");
        let mut third = second.clone();
        assert!(third.clamp().is_empty(), "{name}");
        assert_eq!(third, second, "{name}");
    }
}

#[test]
fn old_keys_stay_for_an_older_build() {
    for (name, ..) in ROWS {
        if name == "invalid_values" {
            continue;
        }
        let (s, _) = load(&fixture(name));
        let text = toml::to_string(&s).expect("serializes");
        let old: toml::Table = toml::from_str(&fixture(name)).expect("fixture parses");
        let kept: toml::Table = toml::from_str(&text).expect("output parses");
        let grid = |t: &toml::Table| {
            t["performance"]["overview"]["grid"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["kind"].as_str() == Some("gpu")))
                .cloned()
                .expect("the gpu grid entry")
        };
        let (old, kept) = (grid(&old), grid(&kept));
        for key in ["mode", "arrange", "split", "hidden"] {
            if let Some(v) = old.get(key) {
                assert_eq!(kept.get(key), Some(v), "{name}: {key}");
            }
        }
    }
}

#[test]
fn a_gpu_box_with_no_old_key_converts_nothing() {
    let (s, changed) = load(&fixture("no_gpu_keys"));
    assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY), "{changed:?}");
    assert!(s.performance.layouts.is_empty());
    let text = toml::to_string(&s).expect("serializes");
    assert!(!text.contains("layouts"), "{text}");
}
