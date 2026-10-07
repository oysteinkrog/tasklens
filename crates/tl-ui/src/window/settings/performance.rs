//! Settings window, Performance page, P2 rows (E21-023, UI-SHL-066, PERF-025, R-016, R-053,
//! R-054): graphs, CPU, memory, GPU options, the hidden GPU engine checklist and the device
//! visibility switches.
//!
//! The device order list is E21-024, "Show disconnected fans" E21-036 and the Fans group E21-037.
//! The engine checklist lists every present GPU of the device catalog; the Settings window
//! re-renders on `ComponentMask::DEVICES`, so it follows hotplug.

use std::sync::Arc;

use gpui_kit::{App, SharedString};
use tl_model::devices::DeviceKind;
use tl_model::settings::GRAPH_RANGES_S;
use tl_model::snapshot::Snapshot;

use super::{CheckSection, PageBuilder, PageId};
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::pages::performance::gpu::device_engines;
use crate::settings_global::{SettingKey, SettingValue};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Performance);
    let graphs = tr(cx, "shell-settings-perf-group-graphs");
    let cpu = tr(cx, "shell-settings-perf-group-cpu");
    let memory = tr(cx, "shell-settings-perf-group-memory");
    let gpu = tr(cx, "shell-settings-perf-group-gpu");
    let devices = tr(cx, "shell-settings-perf-group-devices");
    let network = tr(cx, "shell-settings-perf-group-network");

    let ranges: Vec<(SharedString, SettingValue)> = GRAPH_RANGES_S
        .into_iter()
        .zip([
            "shell-settings-perf-range-30",
            "shell-settings-perf-range-60",
            "shell-settings-perf-range-300",
            "shell-settings-perf-range-600",
        ])
        .map(|(s, key)| (tr(cx, key), SettingValue::Int(i64::from(s))))
        .collect();
    page.dropdown(
        graphs.clone(),
        SettingKey::PerformanceGraphRangeS,
        tr(cx, "shell-settings-perf-graph-range"),
        &ranges,
    );
    // Off: the network graph axis is the link speed. UI-SHL-066 gives no description.
    for (key, label) in [
        (
            SettingKey::PerformanceCurvedLines,
            "shell-settings-perf-curved-lines",
        ),
        (
            SettingKey::PerformanceNetworkDynamicScale,
            "shell-settings-perf-network-dynamic-scale",
        ),
    ] {
        page.switch(graphs.clone(), key, tr(cx, label), None);
    }

    let modes = options(
        cx,
        &[
            ("shell-settings-perf-cpu-overall", "overall"),
            ("shell-settings-perf-cpu-per-core", "per_core"),
            ("shell-settings-perf-cpu-overlaid", "all_cores_overlaid"),
            ("shell-settings-perf-cpu-stacked", "all_cores_stacked"),
        ],
    );
    page.dropdown(
        cpu.clone(),
        SettingKey::PerformanceCpuGraphMode,
        tr(cx, "shell-settings-perf-cpu-graph"),
        &modes,
    )
    .switch(
        cpu.clone(),
        SettingKey::PerformanceCpuKernel,
        tr(cx, "shell-settings-perf-kernel-times"),
        None,
    );
    let secondary = options(
        cx,
        &[
            ("shell-settings-perf-secondary-none", "none"),
            ("shell-settings-perf-secondary-power", "power"),
            ("shell-settings-perf-secondary-clock", "clock"),
            ("shell-settings-perf-secondary-temperature", "temperature"),
        ],
    );
    page.dropdown(
        cpu,
        SettingKey::PerformanceCpuSecondaryGraph,
        tr(cx, "shell-settings-perf-secondary-graph"),
        &secondary,
    );

    for (key, label) in [
        (
            SettingKey::PerformanceMemoryComposition,
            "shell-settings-perf-memory-composition",
        ),
        (
            SettingKey::PerformanceMemorySwapGraph,
            "shell-settings-perf-swap",
        ),
    ] {
        page.switch(memory.clone(), key, tr(cx, label), None);
    }

    let optional = CheckSection {
        heading: SharedString::default(),
        options: vec![
            (tr(cx, "shell-settings-perf-gpu-power"), "power".into()),
            (tr(cx, "shell-settings-perf-gpu-clocks"), "clocks".into()),
        ],
    };
    page.checklist(
        gpu.clone(),
        SettingKey::PerformanceGpuOptionalGraphs,
        tr(cx, "shell-settings-perf-gpu-optional"),
        vec![optional],
        false,
        SharedString::default(),
    );
    let engines = engine_sections(cx);
    page.checklist(
        gpu,
        SettingKey::PerformanceHiddenGpuEngines,
        tr(cx, "shell-settings-perf-hidden-engines"),
        engines,
        true,
        tr(cx, "shell-settings-perf-no-gpu"),
    );

    for (key, label) in [
        (
            SettingKey::PerformanceShowDisks,
            "shell-settings-perf-disks",
        ),
        (SettingKey::PerformanceShowGpus, "shell-settings-perf-gpus"),
        (SettingKey::PerformanceShowFans, "shell-settings-perf-fans"),
        (
            SettingKey::PerformanceShowBatteries,
            "shell-settings-perf-batteries",
        ),
    ] {
        page.switch(devices.clone(), key, tr(cx, label), None);
    }
    for (key, label) in [
        (
            SettingKey::PerformanceShowNetworkWired,
            "shell-settings-perf-net-wired",
        ),
        (
            SettingKey::PerformanceShowNetworkWireless,
            "shell-settings-perf-net-wireless",
        ),
        (
            SettingKey::PerformanceShowNetworkVpn,
            "shell-settings-perf-net-vpn",
        ),
        (
            SettingKey::PerformanceShowNetworkVirtual,
            "shell-settings-perf-net-virtual",
        ),
        (
            SettingKey::PerformanceShowNetworkOther,
            "shell-settings-perf-net-other",
        ),
    ] {
        page.switch(network.clone(), key, tr(cx, label), None);
    }
    page
}

/// One section per present GPU of the device catalog: its name, then its engines.
fn engine_sections(cx: &App) -> Vec<CheckSection> {
    let Some(snap) = cx
        .try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).current()))
    else {
        return Vec::new();
    };
    gpu_sections(&snap)
}

fn gpu_sections(snap: &Snapshot) -> Vec<CheckSection> {
    snap.devices
        .present_of(DeviceKind::Gpu)
        .map(|gpu| CheckSection {
            heading: SharedString::from(gpu.display_name.to_string()),
            options: device_engines(snap, gpu.idx)
                .into_iter()
                .map(|(key, name)| (name, SharedString::from(key.to_string())))
                .collect(),
        })
        .collect()
}

/// `(label key, stored value)` pairs as dropdown options.
fn options(cx: &App, pairs: &[(&str, &'static str)]) -> Vec<(SharedString, SettingValue)> {
    pairs
        .iter()
        .map(|(key, value)| (tr(cx, key), SettingValue::Str((*value).into())))
        .collect()
}
