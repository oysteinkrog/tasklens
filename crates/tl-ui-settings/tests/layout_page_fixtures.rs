//! Migration fixtures for the 1.3.0 page settings (ADR-019 "Phase 3", tasklens-hzire.2). Each
//! `page_*.toml` file in `tests/fixtures/layouts` is a `settings.toml` as 1.3.0 wrote it, one
//! per combination of the old page keys. Loading it with every page kind supported must give
//! the preset's widget list; converting twice must change nothing; the old keys must stay in
//! the file for 1.3.0.
//!
//! `PAGE_KINDS` is empty until the page beads add their kinds, so these tests run the `*_for`
//! functions with every page kind, as a build with every page would.

use tl_ui_settings::layout::{ContainerLayout, DeviceSelector, LayoutEntry, WidgetContent};
use tl_ui_settings::layout_boxes::DeviceKind;
use tl_ui_settings::layout_pages::{
    PAGE_DEVICE_KINDS, convert_old_page_settings_for, convert_page, page_needs_conversion_for,
};
use tl_ui_settings::settings::{LAYOUTS_CONVERTED_KEY, Settings};

const ALL: &[DeviceKind] = &PAGE_DEVICE_KINDS;
const GPU: &str = "page.gpu:GPU-0a1b2c3d";
const GPU_KEY: &str = "gpu:GPU-0a1b2c3d";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/layouts/{name}.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Loads `text` as a build with every page kind would: `Settings::clamp`, the layouts read
/// with every page kind, then the CPU and Memory page conversion. Returns whether any page
/// converted. The kinds of this build already convert in `Settings::clamp` (the CPU page since
/// tasklens-hzire.5), which reports it.
fn load(text: &str) -> (Settings, bool) {
    let mut s: Settings = toml::from_str(text).expect("valid TOML");
    let by_clamp = s.clamp().contains(&LAYOUTS_CONVERTED_KEY);
    s.performance.layouts.load_for(ALL);
    let converted = convert_old_page_settings_for(&mut s.performance, ALL);
    (s, by_clamp || converted)
}

/// Builds the GPU page `GPU` as the window does the first time: converts it when it must.
fn build_gpu_page(s: &mut Settings) -> bool {
    if !page_needs_conversion_for(&s.performance, GPU, ALL) {
        return false;
    }
    let layout = convert_page(&s.performance, GPU).expect("a GPU page");
    s.performance.layouts.set(layout);
    true
}

/// One widget as "<device> <metric> [forms]" or "fixed <name>".
fn widgets(l: &ContainerLayout) -> Vec<String> {
    l.widgets
        .iter()
        .map(|w| match &w.content {
            WidgetContent::Metric(s) => {
                let DeviceSelector::Key(k) = &s.device else {
                    panic!("a page widget names a key");
                };
                let forms: Vec<&str> = w.forms.names().collect();
                format!("{k} {} [{}]", s.metric.as_str(), forms.join(","))
            }
            WidgetContent::Fixed(f) => format!("fixed {}", f.name()),
            WidgetContent::Group(g) => format!("group {}", g.members.name()),
        })
        .collect()
}

fn cvl(device: &str, key: &str) -> String {
    format!("{device} {key} [chart,value,label]")
}

const CPU_MODES: [&str; 5] = [
    "overall",
    "per_core",
    "all_cores_overlaid",
    "all_cores_stacked",
    "board",
];
const CPU_SECONDARY: [&str; 5] = ["none", "power", "clock", "clocks", "temperature"];

/// The widget list ADR-019 phase 3 gives the CPU page for `mode` and `secondary`.
fn cpu_widgets(mode: &str, secondary: &str) -> Vec<String> {
    let mut out: Vec<String> = match mode {
        "overall" => vec![cvl("cpu", "total")],
        "per_core" => vec!["fixed cpu_core_grid".into()],
        "all_cores_overlaid" => vec!["cpu cores_overlaid [chart,label]".into()],
        "all_cores_stacked" => vec!["cpu cores_stacked [chart,label]".into()],
        "board" => vec![cvl("cpu", "total"), "fixed cpu_core_board".into()],
        _ => unreachable!(),
    };
    match secondary {
        "power" => out.extend([
            cvl("cpu", "power_draw"),
            cvl("cpu", "temp"),
            cvl("cpu", "clock"),
        ]),
        "clock" => out.push(cvl("cpu", "clock")),
        "clocks" => out.push("cpu core_clocks [chart,label]".into()),
        "temperature" => out.push(cvl("cpu", "temp")),
        _ => {}
    }
    out.push("fixed details".into());
    out
}

/// The widget list for the Memory page.
fn memory_widgets(view: &str, swap: bool, bar: bool) -> Vec<String> {
    let mut out = vec![if view == "apps" {
        "fixed memory_by_app".to_owned()
    } else {
        cvl("memory", "physical")
    }];
    out.push(cvl("memory", "commit_pct"));
    if swap {
        out.push(cvl("memory", "swap"));
    }
    if bar {
        out.push("fixed memory_bar".into());
    }
    for f in [
        "details",
        "memory_compressed",
        "swap_devices",
        "memory_modules",
    ] {
        out.push(format!("fixed {f}"));
    }
    out
}

/// The widget list for a GPU page.
fn gpu_widgets(power: bool, clocks: bool) -> Vec<String> {
    let mut out = vec![
        "fixed gpu_engine_grid".to_owned(),
        cvl(GPU_KEY, "memory"),
        cvl(GPU_KEY, "temp"),
    ];
    if power {
        out.push(cvl(GPU_KEY, "power"));
    }
    if clocks {
        out.push(cvl(GPU_KEY, "clocks"));
    }
    out.push("fixed details".into());
    out.push("fixed gpu_displays".into());
    out
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Every Memory fixture: name, view, swap, bar.
fn memory_rows() -> Vec<(String, &'static str, bool, bool)> {
    let mut rows = Vec::new();
    for view in ["usage", "apps"] {
        for swap in [false, true] {
            for bar in [false, true] {
                let name = format!("page_memory_{view}_swap_{}_bar_{}", on(swap), on(bar));
                rows.push((name, view, swap, bar));
            }
        }
    }
    rows
}

const GPU_ROWS: [(&str, bool, bool); 4] = [
    ("page_gpu_none", false, false),
    ("page_gpu_power", true, false),
    ("page_gpu_clocks", false, true),
    ("page_gpu_power_clocks", true, true),
];

/// The page's entry, checked as a converted flow layout with ids from 1.
fn entry_of<'a>(s: &'a Settings, container: &str, name: &str) -> &'a ContainerLayout {
    let l = s
        .performance
        .layouts
        .known(container)
        .unwrap_or_else(|| panic!("{name}: no {container} entry"));
    assert!(l.widgets.iter().all(|w| w.cell.is_none()), "{name}");
    let ids: Vec<u32> = l.widgets.iter().map(|w| w.id).collect();
    assert_eq!(ids, (1..=ids.len() as u32).collect::<Vec<_>>(), "{name}");
    assert_eq!(l.next_id as usize, ids.len() + 1, "{name}");
    assert_eq!(l.version, 2, "{name}");
    l
}

#[test]
fn every_cpu_fixture_converts_to_its_preset() {
    for mode in CPU_MODES {
        for secondary in CPU_SECONDARY {
            let name = format!("page_cpu_{mode}_{secondary}");
            let (s, converted) = load(&fixture(&name));
            if (mode, secondary) == ("per_core", "power") {
                // Every key at its default: nothing converts and nothing is written.
                assert!(!converted, "{name}");
                assert!(s.performance.layouts.is_empty(), "{name}");
                continue;
            }
            assert!(converted, "{name}");
            let l = entry_of(&s, "page.cpu", &name);
            assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some(mode));
            assert_eq!(widgets(l), cpu_widgets(mode, secondary), "{name}");
            // Only the CPU page converts.
            assert_eq!(s.performance.layouts.0.len(), 1, "{name}");
        }
    }
}

#[test]
fn every_memory_fixture_converts_to_its_preset() {
    for (name, view, swap, bar) in memory_rows() {
        let (s, converted) = load(&fixture(&name));
        if (view, swap, bar) == ("usage", true, true) {
            assert!(!converted, "{name}");
            assert!(s.performance.layouts.is_empty(), "{name}");
            continue;
        }
        assert!(converted, "{name}");
        let l = entry_of(&s, "page.memory", &name);
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some(view));
        assert_eq!(widgets(l), memory_widgets(view, swap, bar), "{name}");
        assert_eq!(s.performance.layouts.0.len(), 1, "{name}");
    }
}

#[test]
fn every_gpu_fixture_converts_when_the_page_is_built() {
    for (name, power, clocks) in GPU_ROWS {
        let (mut s, converted) = load(&fixture(name));
        // Load does not know the GPUs: nothing converts there.
        assert!(!converted, "{name}");
        let built = build_gpu_page(&mut s);
        if !power && !clocks {
            assert!(!built, "{name}");
            assert!(s.performance.layouts.is_empty(), "{name}");
            continue;
        }
        assert!(built, "{name}");
        let l = entry_of(&s, GPU, name);
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("default"));
        assert_eq!(widgets(l), gpu_widgets(power, clocks), "{name}");
        // Building the page again converts nothing.
        let before = s.clone();
        assert!(!build_gpu_page(&mut s), "{name}");
        assert_eq!(s, before, "{name}");
    }
}

/// Every fixture name.
fn all_names() -> Vec<String> {
    let mut names = Vec::new();
    for mode in CPU_MODES {
        for secondary in CPU_SECONDARY {
            names.push(format!("page_cpu_{mode}_{secondary}"));
        }
    }
    names.extend(memory_rows().into_iter().map(|(n, ..)| n));
    names.extend(GPU_ROWS.iter().map(|(n, ..)| (*n).to_owned()));
    names
}

#[test]
fn page_conversion_twice_changes_nothing() {
    for name in all_names() {
        let (mut first, _) = load(&fixture(&name));
        build_gpu_page(&mut first);
        let text = toml::to_string(&first).expect("serializes");
        let (mut second, converted) = load(&text);
        assert!(!converted, "{name}");
        assert!(!build_gpu_page(&mut second), "{name}");
        assert_eq!(second, first, "{name}");
        let mut third = second.clone();
        assert!(!convert_old_page_settings_for(&mut third.performance, ALL));
        assert_eq!(third, second, "{name}");
    }
}

#[test]
fn old_page_keys_stay_for_1_3_0() {
    for name in all_names() {
        let (mut s, _) = load(&fixture(&name));
        build_gpu_page(&mut s);
        let text = toml::to_string(&s).expect("serializes");
        let old: toml::Table = toml::from_str(&fixture(&name)).expect("fixture parses");
        let kept: toml::Table = toml::from_str(&text).expect("output parses");
        for key in [
            "cpu_graph_mode",
            "cpu_secondary_graph",
            "memory_view",
            "memory_swap_graph",
            "memory_composition",
            "gpu_optional_graphs",
        ] {
            if let Some(v) = old["performance"].get(key) {
                assert_eq!(kept["performance"].get(key), Some(v), "{name}: {key}");
            }
        }
        // A build without the page kinds keeps the page entries as read. A kind this build
        // supports (GPU since tasklens-hzire.4) reads as known; the file stays the same.
        let mut older: Settings = toml::from_str(&text).expect("TOML");
        older.clamp();
        for e in &older.performance.layouts.0 {
            match e {
                LayoutEntry::Known(l) => {
                    assert!(
                        tl_ui_settings::layout::is_layout_container(&l.container),
                        "{name}"
                    );
                }
                _ => assert!(matches!(e, LayoutEntry::Kept(_)), "{name}"),
            }
        }
        assert_eq!(toml::to_string(&older).expect("serializes"), text, "{name}");
    }
}
