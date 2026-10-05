//! The `Sensors` worker (ADR-004, ADR-008, ADR-016): wraps the tl-collect `SensorsSampler` and
//! turns each `SensorsReading` into a `SourceUpdate::Sensors(SensorSample)`.
//!
//! It reads every hwmon temperature and fan at 1 Hz while sensors are demanded, and not at all
//! otherwise: unlike the other sources, history recording does not keep it on (ADR-008 row
//! "Sensors (hwmon), fans"). The chips are enumerated again every 5 s; each enumeration also
//! sends a full `DeviceList` for `SENSORS | FANS` through `take_extra`. GPU chips are skipped:
//! their temperature and fan belong to the GPU page (PERF-068). CPU temperature stays in the
//! CPU worker (PERF-039).

use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use rustc_hash::FxHashSet;
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Measured};
use tl_collect::sample::sensors::{FanReading, SensorsReading, SensorsSampler, TempReading};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::system::{Metric, SensorKind, SensorReading};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{SensorSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::{self, EnumClock};

pub const PERIOD: Duration = Duration::from_secs(1);

/// The sensors rate table (ADR-008 row "Sensors (hwmon), fans"): recording does not keep
/// sensors on.
pub fn sensors_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.sensors { Some(PERIOD) } else { None }
}

pub struct SensorsSource {
    sampler: SensorsSampler,
    /// Reused every sample.
    reading: SensorsReading,
    /// When the chips are enumerated again.
    clock: EnumClock,
    /// The `Devices` list waiting for `take_extra`.
    pending: Option<SourceUpdate>,
    /// Fans that reported a non-zero RPM this session (R-170 item 5).
    fan_seen: FxHashSet<DeviceIdentity>,
    /// False until the first `sample_at`: `SensorsSampler::new` already ran one rescan.
    started: bool,
}

impl SensorsSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: SensorsSampler::new(root),
            reading: SensorsReading::default(),
            clock: EnumClock::default(),
            pending: None,
            fan_seen: FxHashSet::default(),
            started: false,
        }
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        if self.clock.due(clock.boot_ns) {
            if self.started {
                self.sampler.rescan();
            }
            self.pending = Some(SourceUpdate::Devices(enumerate::sensor_list(
                clock.boot_ns,
                self.sampler.chips(),
            )));
        }
        self.started = true;
        self.sampler.sample_at(clock, &mut self.reading);

        let r = &self.reading;
        let mut readings = Vec::with_capacity(r.temps.len() + r.fans.len());
        readings.extend(r.temps.iter().map(temp_reading));
        for fan in &r.fans {
            let identity = DeviceIdentity::Fan {
                chip: fan.chip.clone(),
                index: fan.n,
            };
            if fan.rpm.validity == Validity::Valid && fan.rpm.value > 0 {
                self.fan_seen.insert(identity.clone());
            }
            let connected = fan.label.is_some() || self.fan_seen.contains(&identity);
            readings.push((identity, fan_reading(fan, connected)));
        }
        let sample = SensorSample {
            t_ns: r.t_ns,
            readings,
        };
        (self, SourceUpdate::Sensors(sample))
    }
}

impl Source for SensorsSource {
    fn id(&self) -> SourceId {
        SourceId::Sensors
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        sensors_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    /// There are no rates; only the next enumeration is forced.
    fn reset_baselines(&mut self) {
        self.clock.force();
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending.take()
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Sensors,
        name: "sensors",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `SensorsSource` per generation, built on the blocking pool (it reads sysfs), then
/// `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| SensorsSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(src) => src,
                Err(JoinError::Cancelled(reason)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    return Outcome::Cancelled(reason);
                }
                Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
                Err(JoinError::PolledAfterCompletion) => {
                    return Outcome::Err(WorkerError::Other("job polled after completion".into()));
                }
            };
            source::run(cx, io, src).await
        }
    })
}

/// Same identity as `enumerate::sensor_identity`: the sampler's label is the `temp<n>_label`
/// text, else `temp<n>`.
fn temp_reading(t: &TempReading) -> (DeviceIdentity, SensorReading) {
    let identity = DeviceIdentity::Sensor {
        chip: t.chip.clone(),
        label: t.label.clone(),
    };
    let reading = SensorReading {
        // The worker cannot know the catalog index; the reducer sets it.
        device: DeviceIdx(0),
        kind: SensorKind::Temperature,
        value: metric(t.celsius),
        pwm_pct: Metric::with(Validity::Unsupported),
        fan_control: None,
        fan_connected: false,
    };
    (identity, reading)
}

/// `fan_control` is filled by E12-030.
fn fan_reading(f: &FanReading, connected: bool) -> SensorReading {
    SensorReading {
        device: DeviceIdx(0),
        kind: SensorKind::Fan,
        value: Metric {
            value: f.rpm.value as f32,
            validity: f.rpm.validity,
        },
        pwm_pct: metric(f.pwm_pct),
        fan_control: None,
        fan_connected: connected,
    }
}

fn metric<T: Copy + Default>(m: Measured<T>) -> Metric<T> {
    Metric {
        value: m.value,
        validity: m.validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        assert_eq!(sensors_period(&Demand::default(), &s), None);

        let sensors = Demand {
            sensors: true,
            ..Demand::default()
        };
        assert_eq!(sensors_period(&sensors, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            sensors: false,
            recording: true,
            ..Demand::default()
        };
        assert_eq!(sensors_period(&recording, &s), None);
    }
}
