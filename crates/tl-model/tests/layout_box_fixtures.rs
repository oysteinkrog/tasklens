//! Migration fixtures for the old modes of the Overview CPU, Memory, Disk and Network boxes
//! (ADR-019 "Phase 2", tasklens-6swdr.2). Each file in `tests/fixtures/layouts` named after a
//! box is a `settings.toml` as 1.2.0 wrote it. Loading it must give the preset's widget list,
//! converting twice must change nothing, and the old keys must stay in the file for an older
//! build.

use tl_model::layout::{
    ContainerLayout, DeviceSelector, GroupItems, WidgetContent, overview_container,
};
use tl_model::settings::{LAYOUTS_CONVERTED_KEY, LAYOUTS_KEY, Settings};

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

/// One widget as text: "<device> <metric> [forms]" for a metric widget, "group <members>
/// <device> <metric> [forms] <items> limit=<n> arrange=<a>" for a group, "fixed <name>".
fn describe(content: &WidgetContent, forms: Vec<&str>) -> String {
    let device = |d: &DeviceSelector| match d {
        DeviceSelector::Key(k) => k.to_string(),
        DeviceSelector::Slot(n) => format!("slot{n}"),
    };
    let forms = forms.join(",");
    match content {
        WidgetContent::Metric(s) => format!("{} {} [{forms}]", device(&s.device), s.metric.0),
        WidgetContent::Group(g) => {
            let items = match g.options.items {
                GroupItems::Rows => "rows",
                GroupItems::Cells => "cells",
            };
            format!(
                "group {} {} {} [{forms}] {items} limit={:?} arrange={:?}",
                g.members.name(),
                device(&g.device),
                g.metric.0,
                g.options.limit,
                g.options.arrange.as_deref()
            )
        }
        WidgetContent::Fixed(f) => format!("fixed {}", f.name()),
    }
}

fn widgets(l: &ContainerLayout) -> Vec<String> {
    l.widgets
        .iter()
        .map(|w| describe(&w.content, w.forms.names().collect()))
        .collect()
}

/// The preset of a row: id, arrange, split, power_split, power_placement.
type Preset = (
    &'static str,
    Option<&'static str>,
    Option<f64>,
    Option<f64>,
    Option<&'static str>,
);

/// One fixture: its name, the box kind, the expected preset and widgets.
struct Row {
    name: &'static str,
    kind: &'static str,
    preset: Preset,
    widgets: &'static [&'static str],
}

const TOTAL: &str = "cpu total [chart]";
const CORE_ROWS: &str = "group cores cpu load [chart,bar,value,label] rows limit=None arrange=None";
const CLOCKS: &str = "cpu core_clocks [chart,label]";
const POWER: &str = "cpu power_draw [chart,value,label]";
const MEM_ROWS: [&str; 5] = [
    "memory used [chart,bar,value,label]",
    "memory cached [chart,bar,value,label]",
    "memory available [chart,bar,value,label]",
    "memory swap [chart,bar,value,label]",
    "memory commit [chart,bar,value,label]",
];

const ROWS: &[Row] = &[
    Row {
        name: "cpu_chart_cores",
        kind: "cpu",
        preset: (
            "chart_cores",
            Some("auto"),
            Some(0.4),
            Some(0.3),
            Some("under_graph"),
        ),
        widgets: &[TOTAL, CORE_ROWS, CLOCKS, POWER],
    },
    Row {
        name: "cpu_chart",
        kind: "cpu",
        preset: ("chart", None, Some(0.5), None, None),
        widgets: &[TOTAL, CLOCKS],
    },
    Row {
        name: "cpu_cores",
        kind: "cpu",
        preset: ("cores", None, Some(0.6), Some(0.25), None),
        widgets: &[CORE_ROWS, POWER],
    },
    Row {
        name: "cpu_core_charts",
        kind: "cpu",
        preset: (
            "core_charts",
            Some("vertical"),
            Some(0.35),
            None,
            Some("full_width"),
        ),
        widgets: &[
            "group cores cpu load [chart,value,label] cells limit=None arrange=Some(\"vertical\")",
            POWER,
        ],
    },
    Row {
        name: "cpu_power_only",
        kind: "cpu",
        preset: ("chart_cores", None, None, None, None),
        widgets: &[TOTAL, CORE_ROWS, POWER],
    },
    Row {
        name: "memory_rows",
        kind: "memory",
        preset: ("rows", None, Some(0.3), None, None),
        widgets: &MEM_ROWS,
    },
    Row {
        name: "memory_chart_rows",
        kind: "memory",
        preset: ("chart_rows", Some("horizontal"), Some(0.35), None, None),
        widgets: &[
            "memory used [chart]",
            "memory used [bar,value,label]",
            "memory cached [bar,value,label]",
            "memory available [bar,value,label]",
            "memory swap [bar,value,label]",
            "memory commit [bar,value,label]",
        ],
    },
    Row {
        name: "memory_chart",
        kind: "memory",
        preset: ("chart", None, None, None, None),
        widgets: &["memory composition [chart]"],
    },
    Row {
        name: "memory_charts",
        kind: "memory",
        preset: ("charts", Some("vertical"), Some(0.5), None, None),
        widgets: &[
            "memory used [chart,value,label]",
            "memory cached [chart,value,label]",
            "memory available [chart,value,label]",
            "memory swap [chart,value,label]",
            "memory commit [chart,value,label]",
        ],
    },
    Row {
        name: "memory_apps",
        kind: "memory",
        preset: ("apps", None, None, None, None),
        widgets: &["fixed memory_by_app"],
    },
    Row {
        name: "disk_disk_charts",
        kind: "disk",
        preset: ("disk_charts", Some("vertical"), Some(0.4), None, None),
        widgets: &[
            "group disks aggregate:disk io [chart,value,label] cells limit=Some(8) arrange=Some(\"vertical\")",
        ],
    },
    Row {
        name: "disk_rows",
        kind: "disk",
        preset: ("rows", None, Some(0.6), None, None),
        widgets: &[
            "group disks aggregate:disk io [chart,value,label] rows limit=Some(8) arrange=None",
        ],
    },
    Row {
        name: "disk_total_chart",
        kind: "disk",
        preset: ("total_chart", Some("horizontal"), None, None, None),
        widgets: &["aggregate:disk io [chart]"],
    },
    Row {
        name: "net_stacked",
        kind: "net",
        preset: ("stacked", None, Some(0.4), None, None),
        widgets: &[
            "aggregate:net rx [chart,value,label]",
            "aggregate:net tx [chart,value,label]",
        ],
    },
    Row {
        name: "net_columns",
        kind: "net",
        preset: ("columns", Some("horizontal"), Some(0.6), None, None),
        widgets: &[
            "aggregate:net rx [chart,value,label]",
            "aggregate:net tx [chart,value,label]",
        ],
    },
    Row {
        name: "net_combined",
        kind: "net",
        preset: ("combined", None, None, None, None),
        widgets: &["aggregate:net traffic [chart,value,label]"],
    },
    Row {
        name: "net_adapter_charts",
        kind: "net",
        preset: ("adapter_charts", Some("horizontal"), Some(0.5), None, None),
        widgets: &[
            "group adapters aggregate:net traffic [chart,value,label] cells limit=None arrange=Some(\"horizontal\")",
        ],
    },
];

fn layout<'a>(s: &'a Settings, row: &Row) -> &'a ContainerLayout {
    s.performance
        .layouts
        .known(&overview_container(row.kind))
        .unwrap_or_else(|| panic!("{}: no {} entry", row.name, row.kind))
}

#[test]
fn every_old_box_mode_converts_to_its_preset() {
    for row in ROWS {
        let name = row.name;
        let (s, changed) = load(&fixture(name));
        assert_eq!(changed, vec![LAYOUTS_CONVERTED_KEY], "{name}");
        let l = layout(&s, row);
        let p = l.preset.as_ref().expect("a flow layout");
        let (id, arrange, split, power_split, power_placement) = row.preset;
        assert_eq!(p.id, id, "{name}");
        assert_eq!(p.arrange.as_deref(), arrange, "{name}");
        assert_eq!(p.split, split, "{name}");
        assert_eq!(p.power_split, power_split, "{name}");
        assert_eq!(p.power_placement.as_deref(), power_placement, "{name}");
        assert_eq!(widgets(l), row.widgets, "{name}");
        assert!(l.widgets.iter().all(|w| w.cell.is_none()), "{name}");
        let ids: Vec<u32> = l.widgets.iter().map(|w| w.id).collect();
        assert_eq!(ids, (1..=ids.len() as u32).collect::<Vec<_>>(), "{name}");
        assert_eq!(l.next_id as usize, ids.len() + 1, "{name}");
        assert_eq!(l.version, 2, "{name}");
        // Only the box with old keys converts: the processes box has none.
        assert_eq!(s.performance.layouts.0.len(), 1, "{name}");
    }
}

#[test]
fn box_conversion_twice_changes_nothing() {
    for row in ROWS {
        let name = row.name;
        let (first, _) = load(&fixture(name));
        let text = toml::to_string(&first).expect("serializes");
        assert!(text.contains("version = 2"), "{name}: {text}");
        let (second, changed) = load(&text);
        assert!(changed.is_empty(), "{name}: {changed:?}");
        assert_eq!(second, first, "{name}");
        let mut third = second.clone();
        assert!(third.clamp().is_empty(), "{name}");
        assert_eq!(third, second, "{name}");
        assert!(!changed.contains(&LAYOUTS_KEY), "{name}");
    }
}

#[test]
fn old_box_keys_stay_for_an_older_build() {
    for row in ROWS {
        let name = row.name;
        let (s, _) = load(&fixture(name));
        let text = toml::to_string(&s).expect("serializes");
        let old: toml::Table = toml::from_str(&fixture(name)).expect("fixture parses");
        let kept: toml::Table = toml::from_str(&text).expect("output parses");
        let grid = |t: &toml::Table| {
            t["performance"]["overview"]["grid"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["kind"].as_str() == Some(row.kind)))
                .cloned()
                .expect("the box's grid entry")
        };
        let (old, kept) = (grid(&old), grid(&kept));
        for key in [
            "mode",
            "arrange",
            "split",
            "power",
            "power_split",
            "power_placement",
            "clocks",
        ] {
            assert_eq!(kept.get(key), old.get(key), "{name}: {key}");
        }
    }
}
