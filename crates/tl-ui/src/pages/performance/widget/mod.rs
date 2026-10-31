//! Widgets: one metric of one device in one or more forms (ADR-019, D-041, UI-PRF-061).
//!
//! Phase 1 builds them for the Overview GPU box. `meta` says what each metric is: its unit,
//! format, chart scale, bar denominator and stats rule, and so which forms it offers.
//! `compose` picks a widget's arrangement for its forms and size. `source` holds the data of
//! each source, shared by the widgets that show it, and `metric_widget` is the widget entity.

pub mod compose;
pub mod meta;
pub mod metric_widget;
pub mod source;
