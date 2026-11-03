//! tl-ui-procs: the Processes page and its dialogs and property windows, moved from tl-ui
//! (ADR-020). The files keep their paths under `src/`, so tl-ui re-exports them at their old
//! paths: `pages::processes` and the process windows under `windows`.

pub mod pages;
pub mod windows;
