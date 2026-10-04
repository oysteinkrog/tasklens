//! Validity states and per-field validity (ADR-003).
//!
//! Missing data is never zero: every displayed value carries one of these states. The glyphs and
//! words for each state live in tl-ui (PERF-013), not here. The numbers are frozen because history
//! files store them (ADR-012).

use serde::{Deserialize, Serialize};

/// The state of one value. The default is `WarmingUp`, so a value that was never sampled reads as
/// "collecting", never as a valid zero.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Validity {
    Valid = 0,
    /// Source running, no rate yet (needs two samples).
    #[default]
    WarmingUp = 1,
    /// Last value older than the stale threshold max(3 × period, 1.5 s) (ADR-008).
    Stale = 2,
    /// Permission denied (other user's process, root-only file).
    Restricted = 3,
    /// Hardware or driver does not provide this metric.
    Unsupported = 4,
    /// Device or process is gone.
    Missing = 5,
    /// Collection error; see `SourceHealth.last_error`.
    Failed = 6,
}

impl Validity {
    pub const fn is_valid(self) -> bool {
        matches!(self, Validity::Valid)
    }

    /// Discriminant as u8 (0..=6), for history files and value-bit caches.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of `as_u8`; `None` for 7..=255.
    pub const fn from_u8(v: u8) -> Option<Validity> {
        match v {
            0 => Some(Validity::Valid),
            1 => Some(Validity::WarmingUp),
            2 => Some(Validity::Stale),
            3 => Some(Validity::Restricted),
            4 => Some(Validity::Unsupported),
            5 => Some(Validity::Missing),
            6 => Some(Validity::Failed),
            _ => None,
        }
    }
}

/// Index into `FieldValidity`. Order is frozen.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ProcessField {
    Cpu = 0,
    RssAnon = 1,
    Rss = 2,
    Pss = 3,
    Uss = 4,
    Swap = 5,
    IoRead = 6,
    IoWrite = 7,
    NetRx = 8,
    NetTx = 9,
    GpuUtil = 10,
    GpuMem = 11,
    Threads = 12,
}

pub const PROCESS_FIELDS: usize = 13;

impl ProcessField {
    /// All 13 fields in index order.
    pub const ALL: [ProcessField; PROCESS_FIELDS] = [
        ProcessField::Cpu,
        ProcessField::RssAnon,
        ProcessField::Rss,
        ProcessField::Pss,
        ProcessField::Uss,
        ProcessField::Swap,
        ProcessField::IoRead,
        ProcessField::IoWrite,
        ProcessField::NetRx,
        ProcessField::NetTx,
        ProcessField::GpuUtil,
        ProcessField::GpuMem,
        ProcessField::Threads,
    ];
}

/// One validity state per numeric field of a process record, so one unreadable field (for
/// example PSS of another user's process) shows its own glyph. 13 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct FieldValidity(pub [Validity; PROCESS_FIELDS]);

impl FieldValidity {
    /// Every field set to `v`.
    pub const fn all(v: Validity) -> FieldValidity {
        FieldValidity([v; PROCESS_FIELDS])
    }

    pub fn get(&self, f: ProcessField) -> Validity {
        self.0[f as usize]
    }

    pub fn set(&mut self, f: ProcessField, v: Validity) {
        self.0[f as usize] = v;
    }
}

impl Default for FieldValidity {
    fn default() -> Self {
        FieldValidity::all(Validity::WarmingUp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_VALIDITY: [(Validity, u8); 7] = [
        (Validity::Valid, 0),
        (Validity::WarmingUp, 1),
        (Validity::Stale, 2),
        (Validity::Restricted, 3),
        (Validity::Unsupported, 4),
        (Validity::Missing, 5),
        (Validity::Failed, 6),
    ];

    #[test]
    fn default_is_warming_up() {
        assert_eq!(Validity::default(), Validity::WarmingUp);
        assert!(!Validity::default().is_valid());
        assert!(Validity::Valid.is_valid());
    }

    #[test]
    fn discriminants_frozen() {
        for (v, n) in ALL_VALIDITY {
            assert_eq!(v.as_u8(), n, "{v:?}");
        }
    }

    #[test]
    fn from_u8_roundtrip_and_reject() {
        for (v, _) in ALL_VALIDITY {
            assert_eq!(Validity::from_u8(v.as_u8()), Some(v));
        }
        for n in 7..=u8::MAX {
            assert_eq!(Validity::from_u8(n), None, "{n}");
        }
    }

    #[test]
    fn field_validity_default_and_set() {
        let mut fv = FieldValidity::default();
        for f in ProcessField::ALL {
            assert_eq!(fv.get(f), Validity::WarmingUp);
        }
        fv.set(ProcessField::Pss, Validity::Restricted);
        assert_eq!(fv.get(ProcessField::Pss), Validity::Restricted);
        for f in ProcessField::ALL {
            if f != ProcessField::Pss {
                assert_eq!(fv.get(f), Validity::WarmingUp, "{f:?}");
            }
        }
    }

    #[test]
    fn process_field_order_frozen() {
        use ProcessField::*;
        assert_eq!(PROCESS_FIELDS, 13);
        let expected = [
            Cpu, RssAnon, Rss, Pss, Uss, Swap, IoRead, IoWrite, NetRx, NetTx, GpuUtil, GpuMem,
            Threads,
        ];
        assert_eq!(ProcessField::ALL, expected);
        for (i, f) in ProcessField::ALL.iter().enumerate() {
            assert_eq!(*f as usize, i, "{f:?}");
        }
    }
}
