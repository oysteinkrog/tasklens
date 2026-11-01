//! Widgets: one metric of one device in one or more forms (ADR-019, D-041, UI-PRF-061).
//!
//! Phase 1 builds them for the Overview GPU box. `meta` says what each metric is: its unit,
//! format, chart scale, bar denominator and stats rule, and so which forms it offers.
//! `compose` picks a widget's arrangement for its forms and size. `source` holds the data of
//! each source, shared by the widgets that show it, and `metric_widget` is the widget entity.
//! `grid` holds the inner snap grid rules and `container` places a container's widgets.
//! `edit` is Edit Widgets, the editing of a container's widgets, and `menu` its menus and
//! confirmations.
//!
//! Phase 2 adds group widgets: `members` lists a group's members, `group_fill` lays its items
//! out and `group_widget` is the group entity.

pub mod compose;
pub mod container;
pub mod edit;
pub mod grid;
pub mod group_fill;
pub mod group_widget;
pub mod members;
pub mod menu;
pub mod meta;
pub mod metric_widget;
pub mod source;
