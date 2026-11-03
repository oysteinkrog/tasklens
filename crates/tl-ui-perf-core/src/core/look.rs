//! Line and fill alphas that the device pages and the widgets draw with alike. Moved from
//! `cpu.rs` and `memory.rs` (tasklens-zljr5.22).

/// Lowest line alpha in the overlaid mode (PERF-029).
pub const OVERLAID_MIN_ALPHA: f32 = 0.08;
/// Band alphas in the stacked mode, alternating. No document gives a per-band colour step, so
/// every band is `cpu_fill` and the alpha alternates (plan open item 7).
pub const STACKED_ALPHAS: [f32; 2] = [0.85, 0.70];

/// Alpha of the dirty-bytes line against `mem_line`.
pub const DIRTY_ALPHA: f32 = 0.6;

/// Alpha of the Network page's dashed upload line against `net_line` (UI-PRF-031).
pub const NET_UPLOAD_ALPHA: f32 = 0.7;
