//! What the CPU secondary chart can show, shared by the CPU page and the Overview CPU box
//! (PERF-032, PERF-038). Moved from `cpu.rs` (tasklens-zljr5.22).

use tl_model::snapshot::{CpuStaticInfo, RaplState, Snapshot};
use tl_model::validity::Validity;

/// Whether the CPU power draw can be read (PERF-038).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    Readable,
    /// A RAPL zone exists but the user cannot read it.
    NeedsSetup,
    /// No RAPL zone.
    NoZone,
}

/// What the secondary chart can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecondaryAvail {
    pub power: PowerState,
    /// A CPU hwmon chip reports a temperature.
    pub temperature: bool,
    pub clock: bool,
}

impl SecondaryAvail {
    /// From the static RAPL state, else, before the static info arrives, from the package
    /// power's validity (Restricted is a zone the user cannot read). Temperature and clock are
    /// available unless their validity is Unsupported or Missing.
    pub fn read(snap: &Snapshot, info: &CpuStaticInfo) -> Self {
        let cpu = &snap.system.cpu;
        let power = match info.rapl {
            Some(RaplState::Readable) => PowerState::Readable,
            Some(RaplState::Unreadable) => PowerState::NeedsSetup,
            Some(RaplState::NoZone) => PowerState::NoZone,
            None => match cpu.package_power_w.validity {
                Validity::Valid | Validity::Stale | Validity::WarmingUp => PowerState::Readable,
                Validity::Restricted => PowerState::NeedsSetup,
                _ => PowerState::NoZone,
            },
        };
        let present = |v: Validity| !matches!(v, Validity::Unsupported | Validity::Missing);
        Self {
            power,
            temperature: present(cpu.temp_c.validity),
            clock: present(cpu.freq_mhz.validity),
        }
    }
}
