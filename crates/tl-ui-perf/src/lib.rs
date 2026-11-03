//! tl-ui-perf: the Performance page, its device pages and the mini window, moved from tl-ui
//! (ADR-020). tl-ui re-exports them at their old paths, `pages::performance` and `windows::mini`.
//! `performance::core`, `performance::overview` and `performance::widget` stay reachable through
//! re-exports in `performance`.

pub mod mini;
pub mod performance;
