//! Chart scales that the device pages and the widgets share. Moved from `net.rs`
//! (tasklens-zljr5.22).

use tl_ui_settings::settings::NetworkUnit;

use crate::chart::YScale;

/// The Throughput chart's Y scale (UI-PRF-031): the link speed when the scale follows it and the
/// speed is known, otherwise auto, with powers of two under bytes (UI-PRF-001 fallback).
pub fn throughput_scale(
    dynamic_scale: bool,
    link_speed_bps: Option<f32>,
    unit: NetworkUnit,
) -> YScale {
    match link_speed_bps {
        Some(speed) if !dynamic_scale => YScale::LinkSpeed(speed),
        _ => YScale::AutoRate {
            iec: unit == NetworkUnit::Bytes,
        },
    }
}
