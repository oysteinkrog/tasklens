# Leader notes for the bead round

Working notes so a terminal restart does not lose them. Items here go into the final open-items
pass once all batch writers finish.

## Cross-batch links reported by writers

- E01 bench flag: `tasklens-crw.11` (`bench_show_both`, CPU page).
- E01 paint hook: `tasklens-yrf.7` and `tasklens-yrf.9` (`AnimationDriver.paint_log`, ChartElement
  paint step 11), exported by `tasklens-yrf.1`. The hooks sit behind the `test-hooks` feature, so
  `tasklens-wlf.3` must declare `bench = ["test-hooks"]` in tl-ui.

- E01 `first_frame_ms` log line: `tasklens-jqk.10` (E20-008).
- E31 tray chain-edit points: `tasklens-jqk.1` (`close_decision`, `reopen_main_window`),
  `tasklens-jqk.10` (`tray_available`), `tasklens-jqk.6` (`titlebar::minimize_main`).
- E33 needs `tl_ui::run(UiLaunch)`: `tasklens-fut.15` (E16-012, P0).

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
- `br sync --flush-only` at the end (writers use `--no-auto-flush`). The database is
  FrankenSQLite (fsqlite), so never touch it with the sqlite3 tool. Its WAL is 640 MB and did
  not shrink with a lone write; a lone write still takes 1.7 s, so it is not urgent. If it
  matters later, rebuild from the flushed JSONL with `br doctor --repair` after a backup.
- Delete old `.beads/.br_history` copies (410 MB) once the round is done, after asking.
- Collect every batch's open items, settle them (new R entries), apply them to beads.
- Quality pass: `br dep cycles`, `br lint`, orphan and coverage checks.
- Astra review of the plan and beads after Codex returns (Sunday 4 October 11:12), then oracle
  review.

## Batch final reports

- B3 (E08 to E11, E13): done, 91 beads, no cycles, plan committed in 17c2c8e. 17 open items are in
  `docs/plan/b3-engine-core.md`, and the beads follow the recommendations. The build line is
  already replaced in 8hz, 8q0, dwy, i3o and ndy. E13-006 is blocked on open item 13 (`unsafe`
  in tl-gpu-probe).
- B4 (E12, E14, E29, E30): done, 123 beads, no cycles, plan in d456386 and e98c11f. Open items:
  E12 has 11 (see the plan file), E14 has 16, E29 19, E30 9. E12-OI-1 (contract additions, first
  `Source::take_extra()`) blocks 21 E12 beads, including the P1 anchor E12-A2; settle it first.
  E12-OI-4 (udev reader) needs a new B2 bead. Build line still old on E14 and on E29 after qfx.8.
- All batches: `br lint` flags only the epic beads (no Success Criteria section). Leader adds them.
