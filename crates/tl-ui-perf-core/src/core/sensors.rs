//! Sensor lookups that the Fan page, the status bar rules and the widgets share: the fan's
//! default temperature sensor, a sensor's catalog device and the CPU temperature's `hot_c`
//! (PERF-077). Moved from `sensors.rs` and `status_bar/rules.rs` (tasklens-zljr5.22).

use std::sync::Arc;

use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::Snapshot;

/// `<hwmon name>:<device path>:temp<n>`, the persistent key of a temperature sensor without its
/// `sensor:` prefix (table (a) `[fans]`).
pub type SensorRef = Arc<str>;

/// The fan's chip and path (`<hwmon name>:<path>`) and index from its persistent key
/// `fan:<chip>:<path>:<n>`.
pub fn fan_key_parts(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("fan:")?.rsplit_once(':')
}

/// The default sensor of the fan with persistent key `fan_key` (PERF-077): the temperature
/// `temp<n>` of the same chip and path, only when the chip has exactly one fan.
pub fn default_sensor(snap: &Snapshot, fan_key: &str) -> Option<SensorRef> {
    let (chip_path, n) = fan_key_parts(fan_key)?;
    let fans = snap
        .devices
        .entries
        .iter()
        .filter(|e| matches!(e.identity, DeviceIdentity::Fan { .. }))
        .filter(|e| fan_key_parts(&e.persistent_key).is_some_and(|(c, _)| c == chip_path))
        .count();
    if fans != 1 {
        return None;
    }
    let sensor = format!("{chip_path}:temp{n}");
    sensor_device(snap, &sensor).map(|_| SensorRef::from(sensor))
}

/// The catalog device of `sensor` (a [`SensorRef`]).
pub fn sensor_device(snap: &Snapshot, sensor: &str) -> Option<DeviceIdx> {
    snap.devices
        .entries
        .iter()
        .find(|e| {
            e.persistent_key
                .strip_prefix("sensor:")
                .is_some_and(|k| k == sensor)
        })
        .map(|e| e.idx)
}

/// The hwmon chips the engine reads the CPU temperature from, in its order (tl-collect
/// `CPU_TEMP_CHIPS`), and the labels it prefers on them.
const CPU_TEMP_CHIPS: [&str; 4] = ["k10temp", "zenpower", "coretemp", "cpu_thermal"];
const CPU_TEMP_LABELS: [&str; 2] = ["Tctl", "Package id 0"];

/// The CPU temperature's `hot_c`: the sensor the engine reads `CpuTotals.temp_c` from, the
/// first CPU chip in the engine's order, the `Tctl` or `Package id 0` input on it, else its
/// first input.
pub fn cpu_hot_c(devices: &DeviceCatalog) -> Option<f32> {
    let sensors = || {
        devices.entries.iter().filter_map(|e| match &e.identity {
            DeviceIdentity::Sensor { chip, label } if e.present => Some((chip, label, e.hot_c)),
            _ => None,
        })
    };
    let chip = CPU_TEMP_CHIPS
        .iter()
        .find(|wanted| sensors().any(|(chip, _, _)| &**chip == **wanted))?;
    let on_chip = || sensors().filter(|(c, _, _)| &***c == *chip);
    on_chip()
        .find(|(_, label, _)| CPU_TEMP_LABELS.contains(&&***label))
        .or_else(|| on_chip().next())
        .and_then(|(_, _, hot)| hot)
}
