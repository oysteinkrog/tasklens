//! Widgets: one metric of one device in one or more forms (ADR-019, D-041, UI-PRF-061).
//!
//! Phase 1 builds them for the Overview GPU box. `meta` says what each metric is: its unit,
//! format, chart scale, bar denominator and stats rule, and so which forms it offers.

pub mod meta;
