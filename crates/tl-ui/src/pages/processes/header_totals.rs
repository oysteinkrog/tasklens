//! Header line 2 of the process table: the system total under each column that has one
//! (PROC-038, UI-PRC-005, PROC-033, PROC-003, R-050).
//!
//! `header_total` follows the cell contract: it always sets `bits` and `validity`, and writes
//! `text` only when `out.want_text` is set, so the table's total cache formats a total only when
//! its value bits or validity change. No total has words, so no string is translated here.

use tl_model::ids::ColumnId;
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::table::{TableTotals, Total};
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;

use super::cells::ProcColumn;
use crate::table::cells::CellOut;

/// Set in `bits` when Network shows a percent of the link speed, so a percent never shares bits
/// with a rate. A rate is a non-negative `f64`, whose sign bit is clear.
const NET_PCT_FLAG: u64 = 1 << 63;

/// A total before formatting.
enum Value {
    Percent(Total<f32>),
    NetRate(Total<f64>),
    Count(Total<u64>),
}

/// Fills `out` with the line 2 total of `col`. False: the column has no line 2.
pub fn header_total(
    col: &ProcColumn,
    totals: &TableTotals,
    settings: &Settings,
    out: &mut CellOut,
) -> bool {
    use ColumnId as C;

    let value = match col.id {
        C::Cpu => Value::Percent(totals.cpu_pct),
        C::Memory => Value::Percent(totals.memory_pct),
        C::Swap => Value::Percent(totals.swap_pct),
        C::Disk => Value::Percent(totals.disk_pct),
        C::Network => match totals.net_link_bps {
            Some(link) if link > 0 => Value::Percent(Total {
                value: (totals.net_bps.value / link as f64 * 100.0) as f32,
                validity: totals.net_bps.validity,
            }),
            _ => Value::NetRate(totals.net_bps),
        },
        C::Gpu => Value::Percent(totals.gpu_pct),
        C::GpuMemDedicated => Value::Percent(totals.vram_pct),
        C::Threads => Value::Count(totals.threads),
        C::Handles => Value::Count(totals.handles),
        id => match gpu_adapter(id) {
            Some(n) => Value::Percent(totals.gpu_adapter_pct[n]),
            None => return false,
        },
    };
    let net_pct = col.id == C::Network;
    let (bits, validity) = match value {
        Value::Percent(t) if net_pct => (u64::from(t.value.to_bits()) | NET_PCT_FLAG, t.validity),
        Value::Percent(t) => (u64::from(t.value.to_bits()), t.validity),
        Value::NetRate(t) => (t.value.to_bits(), t.validity),
        Value::Count(t) => (t.value, t.validity),
    };
    out.bits = bits;
    out.validity = validity;
    if !out.want_text || !matches!(validity, Validity::Valid | Validity::Stale) {
        return true;
    }
    let s = &mut out.text;
    match value {
        Value::Percent(t) => units::percent(s, t.value),
        Value::NetRate(t) => {
            let unit = match settings.units.network {
                NetworkUnit::Bits => NetUnit::Bits,
                NetworkUnit::Bytes => NetUnit::Bytes,
            };
            units::net_rate(s, t.value, unit, settings.units.network_auto_scale);
        }
        Value::Count(t) => units::count(s, t.value),
    }
    true
}

/// The adapter index of a `gpu_0` to `gpu_15` column.
fn gpu_adapter(id: ColumnId) -> Option<usize> {
    let n = (id as usize).checked_sub(ColumnId::Gpu0 as usize)?;
    (id as usize <= ColumnId::Gpu15 as usize).then_some(n)
}
