//! CPU page: the Details grid rows (E24, UI-PRF-024; PERF-033 to PERF-036, PERF-039, PERF-040).

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{CacheInfo, CacheLevelInfo, CpuStaticInfo, SystemSnapshot};
use tl_model::system::{Metric, SensorKind, SensorReading};
use tl_model::validity::Validity;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu_details::{
    CpuLiveInputs, CpuSensor, ROW_L1, ROW_L2, ROW_L3, ROW_LOAD_AVERAGE, ROW_SPEED, ROW_TEMPERATURE,
    ROW_UP_TIME, cpu_details_model, cpu_temp_sensors, update_live, update_static,
};
use tl_ui::pages::performance::details::{KeyValueModel, KvRow};

const S: u64 = 1_000_000_000;

fn row<'a>(model: &'a KeyValueModel, label: &str) -> &'a KvRow {
    model
        .rows
        .iter()
        .find(|r| r.label == label)
        .expect("row exists")
}

fn value(model: &KeyValueModel, label: &str) -> String {
    row(model, label).value.to_string()
}

fn tip(model: &KeyValueModel, label: &str) -> Option<String> {
    row(model, label).tooltip.as_ref().map(|t| t.to_string())
}

/// A snapshot with every CPU value Valid, observed at 10 s.
fn snapshot() -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.observed_end = 10 * S;
    let c = &mut sys.cpu;
    c.util_pct = Metric::valid(18.6);
    c.kernel_pct = Metric::valid(2.3);
    c.freq_mhz = Metric::valid(4631);
    c.freq_fastest_mhz = Metric::valid(5050);
    c.base_mhz = Metric::valid(4200);
    c.temp_c = Metric::valid(52.0);
    c.processes = Metric::valid(295);
    c.threads = Metric::valid(5596);
    c.handles = Metric::valid(253_248);
    c.load_avg = Metric::valid([1.24, 0.98, 0.87]);
    c.sockets = Metric::valid(1);
    sys.boot_time_s = 1_790_000_000;
    sys.sensors = Arc::from(
        [(3, 52.0), (4, 48.0), (5, 50.0)]
            .map(|(i, c)| {
                (
                    DeviceIdx(i),
                    SensorReading {
                        device: DeviceIdx(i),
                        kind: SensorKind::Temperature,
                        value: Metric::valid(c),
                        pwm_pct: Metric::with(Validity::Unsupported),
                        fan_control: None,
                        fan_connected: false,
                    },
                )
            })
            .as_slice(),
    );
    sys
}

fn sensor_entry(idx: u16, chip: &str, label: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity: DeviceIdentity::Sensor {
            chip: Arc::from(chip),
            label: Arc::from(label),
        },
        generation: 0,
        display_name: Arc::from(label),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
    }
}

/// k10temp sensors at 3, 4 and 5, and a board chip at 2 that is not the CPU's.
fn sensors() -> Vec<CpuSensor> {
    let catalog = DeviceCatalog {
        version: 1,
        entries: Arc::from(vec![
            DeviceEntry {
                idx: DeviceIdx(0),
                identity: DeviceIdentity::CpuPackage,
                generation: 0,
                display_name: Arc::from("CPU"),
                present: true,
                parent: None,
                persistent_key: Arc::from("cpu"),
                net_kind: None,
            },
            sensor_entry(1, "coretemp", "Package id 0"),
            sensor_entry(2, "nct6799", "SYSTIN"),
            sensor_entry(3, "k10temp", "Tctl"),
            sensor_entry(4, "k10temp", "Tccd1"),
            sensor_entry(5, "k10temp", "Tccd2"),
        ]),
    };
    let mut out = Vec::new();
    cpu_temp_sensors(&catalog, &mut out);
    out
}

fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
    Some(CacheLevelInfo {
        instances,
        instance_kib,
        total_kib: u64::from(instances) * instance_kib,
    })
}

#[gpui_kit::test]
fn cpu_details_order_p1(cx: &mut TestAppContext) {
    let model = cpu_details_model();
    cx.update(|cx| {
        init_strings(cx, "en-US");
        model.refresh_labels(cx);
    });
    let labels: Vec<String> = model.to_kv_rows().map(|r| r.label.to_string()).collect();
    assert_eq!(
        labels,
        [
            "Utilization",
            "Kernel time",
            "Speed",
            "Processes",
            "Threads",
            "Handles",
            "Up time",
            "Base speed",
            "Sockets",
            "Temperature",
            "Load average",
            "Cores",
            "Logical processors",
            "L1 cache",
            "L2 cache",
            "L3 cache",
        ]
    );
}

#[test]
fn cpu_uptime_and_loadavg_formats() {
    let mut model = cpu_details_model();
    let sys = snapshot();
    let s = sensors();
    let inp = |now| CpuLiveInputs {
        now_boot_ns: now,
        temp_sensors: &s,
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp(180_095 * S)));
    assert_eq!(value(&model, ROW_UP_TIME), "2 days, 2h 1m 35s");
    let boot = tip(&model, ROW_UP_TIME).expect("boot time tooltip");
    assert!(boot.starts_with("Boot time "), "{boot}");
    assert_eq!(boot.len(), "Boot time 2026-09-21 13:46:40".len(), "{boot}");
    update_live(&mut model, &sys, &inp(2_295 * S));
    assert_eq!(value(&model, ROW_UP_TIME), "0h 38m 15s");

    assert_eq!(value(&model, ROW_LOAD_AVERAGE), "1.24 0.98 0.87");
    assert_eq!(
        tip(&model, ROW_LOAD_AVERAGE).as_deref(),
        Some("1, 5 and 15 minutes")
    );

    // The other live rows, as UI-PRF-024 shows them.
    let shown: Vec<String> = model.rows[..9]
        .iter()
        .map(|r| r.value.to_string())
        .collect();
    assert_eq!(
        shown,
        [
            "18.6%",
            "2.3%",
            "4631 MHz",
            "295",
            "5\u{2009}596",
            "253\u{2009}248",
            "0h 38m 15s",
            "4200 MHz",
            "1",
        ]
    );
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Fastest core: 5050 MHz")
    );
    assert_eq!(value(&model, ROW_TEMPERATURE), "52 °C");
    assert_eq!(
        tip(&model, ROW_TEMPERATURE).as_deref(),
        Some("Tctl 52 °C · Tccd1 48 °C · Tccd2 50 °C")
    );
}

#[test]
fn cache_tooltips() {
    let mut model = cpu_details_model();
    let info = CpuStaticInfo {
        cores: 8,
        logical: 16,
        caches: CacheInfo {
            l1d: level(16, 32),
            l1i: level(16, 32),
            l2: level(8, 1024),
            l3: level(1, 98_304),
            l4: None,
        },
        ..CpuStaticInfo::default()
    };
    assert!(update_static(&mut model, &info));
    assert_eq!(value(&model, ROW_L1), "1 MiB");
    assert_eq!(
        tip(&model, ROW_L1).as_deref(),
        Some("16 × 32 KiB data + 16 × 32 KiB instruction")
    );
    assert_eq!(value(&model, ROW_L2), "8 MiB");
    assert_eq!(tip(&model, ROW_L2).as_deref(), Some("8 × 1 MiB"));
    assert_eq!(value(&model, ROW_L3), "96 MiB");
    assert_eq!(
        tip(&model, ROW_L3).as_deref(),
        Some("98\u{2009}304 KiB · 1 × 96 MiB")
    );
    assert_eq!(value(&model, "perf-cpu-row-cores"), "8");
    assert_eq!(value(&model, "perf-cpu-row-logical-processors"), "16");
    assert!(!update_static(&mut model, &info), "same info: no change");

    let with_l4 = CpuStaticInfo {
        caches: CacheInfo {
            l4: level(1, 131_072),
            ..info.caches
        },
        ..info
    };
    assert!(update_static(&mut model, &with_l4));
    assert_eq!(
        tip(&model, ROW_L3).as_deref(),
        Some("98\u{2009}304 KiB · 1 × 96 MiB · L4 128 MiB")
    );
}

#[test]
fn cpu_temperature_hidden_or_unavailable() {
    let mut model = cpu_details_model();
    let mut sys = snapshot();
    sys.cpu.temp_c = Metric::with(Validity::Unsupported);
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert!(row(&model, ROW_TEMPERATURE).hidden);
    assert!(
        !model
            .to_kv_rows()
            .any(|r| r.label.as_ref() == ROW_TEMPERATURE)
    );

    sys.cpu.temp_c = Metric::with(Validity::Failed);
    let inp = CpuLiveInputs {
        cpu_error: Some("EIO"),
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp));
    assert!(!row(&model, ROW_TEMPERATURE).hidden);
    assert_eq!(value(&model, ROW_TEMPERATURE), "Unavailable");
    assert_eq!(
        tip(&model, ROW_TEMPERATURE).as_deref(),
        Some("Could not read: EIO")
    );
}

#[test]
fn cpu_details_update_is_change_only() {
    let mut model = cpu_details_model();
    let sys = snapshot();
    let s = sensors();
    let inp = CpuLiveInputs {
        now_boot_ns: 100 * S,
        temp_sensors: &s,
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp));
    assert!(!update_live(&mut model, &sys, &inp));

    // A new up time second changes only that row.
    let later = CpuLiveInputs {
        now_boot_ns: 101 * S,
        ..inp
    };
    assert!(update_live(&mut model, &sys, &later));
    assert!(!update_live(&mut model, &sys, &later));

    // Stale keeps the value; the age tooltip changes as time passes.
    let mut stale = sys.clone();
    stale.cpu.util_pct.validity = Validity::Stale;
    assert!(update_live(&mut model, &stale, &later));
    assert_eq!(
        row(&model, "perf-cpu-row-utilization").validity,
        Validity::Stale
    );
    assert_eq!(value(&model, "perf-cpu-row-utilization"), "18.6%");
    assert!(!update_live(&mut model, &stale, &later));
}

#[test]
fn cpu_speed_bogomips_marker() {
    let mut model = cpu_details_model();
    let mut sys = snapshot();
    sys.cpu.freq_mhz = Metric::valid(4200);
    let inp = CpuLiveInputs {
        bogomips: true,
        ..CpuLiveInputs::default()
    };
    update_live(&mut model, &sys, &inp);
    assert_eq!(value(&model, ROW_SPEED), "4200 MHz (?)");
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Reported by the kernel at boot; the current clock is not available")
    );

    // The real clock comes back: the marker and the tooltip go.
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert_eq!(value(&model, ROW_SPEED), "4200 MHz");
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Fastest core: 5050 MHz")
    );
}
