# Leader notes for the bead round

Working notes so a terminal restart does not lose them. Items here go into the final open-items
pass once all batch writers finish.

## Cross-batch links reported by writers

- E01 bench flag: `tasklens-crw.11` (`bench_show_both`, CPU page).
- E01 paint hook: `tasklens-yrf.7` and `tasklens-yrf.9` (`AnimationDriver.paint_log`, ChartElement
  paint step 11), exported by `tasklens-yrf.1`. The hooks sit behind the `test-hooks` feature, so
  `tasklens-wlf.3` must declare `bench = ["test-hooks"]` in tl-ui.

## Open items from B1 (infra) still to settle

1. E33-A1: tl-app needs a `tl_ui::run(UiLaunch)` entry point in E16 (asked of B5).
2. Missing allowed-dependency rows or pins: clap and async-channel (tl-app), rustc-hash
   (tl-model), bitflags `serde` feature, rustix `mm` feature, a PRNG and a time-zone crate (tl-ui).
3. `bench` feature lines: E02-005 writes both up front, so the E01 chain-edit bead for the tl-ui
   `bench` line should be dropped.
4. E31 tray: `TrayModel` and `TrayEvent` need a home in tl-model (`tray.rs`); tray start needs
   tl-app `main.rs` edits; close-to-tray needs edits to E20's window `main.rs` and `titlebar.rs`.
5. E32 low memory: no type or `SourceUpdate` variant carries the low-memory sweep data. R-062's
   PSI trigger uses a 1 s window, but unprivileged triggers need multiples of 2 s since Linux
   6.5. `--force-full-memory-mode` behaviour is unspecified.
6. tl-model types with derived fields (GroupRow, system totals, TrayDemand, FourTuple,
   RecoveryEdit, KWinCommand, helper reply) and the proposed ColumnId numbers 1 to 121 need
   review.
7. `write_atomic` comes in E07-A3 (P2) but the P1 settings store needs it.

## Leader jobs at the end

- One pass replacing the old `flock ... systemd-run ...` build line with `heavy-build` in every
  bead description.
- `br sync --flush-only`, then compact the database (WAL reached 626 MB, `.br_history` 410 MB)
  once no writer holds it.
- Collect every batch's open items, settle them (new R entries), apply them to beads.
- Quality pass: `br dep cycles`, `br lint`, orphan and coverage checks.
- Astra review of the plan and beads after Codex returns (Sunday 4 October 11:12), then oracle
  review.
