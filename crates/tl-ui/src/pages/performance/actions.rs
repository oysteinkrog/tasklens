//! Performance tab actions. E24-002 (tasklens-crw.4) fills this file with every `Perf*` action;
//! `PerfShowMiniWindow` is declared ahead of it for the page frame (E24-006).

gpui_kit::actions!(
    tasklens,
    [
        /// Opens the Performance mini window (R-058). No handler until E26.
        PerfShowMiniWindow,
    ]
);
