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
- B7 (E20, E21, E23): done, 115 beads (E20 30, E21 47, E23 38), no cycles, plan in 56b421a.
  16 open items (E23 has 8 more in the plan). Cross-batch edges for the leader to add:
  qfx.32 on jqk.28; 8fx.3 on jqk.27; E22 on jqk.29 (`end_window_pick`); E26 card colour menu on
  E21-032; E25 and E28 Fetch buttons on E21-017; E29 privileges chain edit on E21-035; E21-043 on
  the E30, E31 and E32 settings-row beads; E21-029 to E21-031 on E17's share-code bead. 00-epics
  section 2 needs the E31 chain-edit rule for window/main.rs and window/titlebar.rs.
- B6 (E19, E24, E25, E26): done, 102 beads (E19 18, E24 27, E25 35, E26 22), no cycles, plan in
  2c96816. Open items E24-6 to E24-10, E25-OI-1 to 8, E26-OI-1 to 7 (plan file). Cross-batch
  edges for the leader: o55.22 on bj6.20; xdh.12 on bj6.35; E20's close bead gets the
  `any_detached_open` call (E26-OI-3). Build line still old on 40 E19 and E24 beads.
- Overlaps to settle together: device commands (SMART, eject, public IP) are E12-OI-1, E25-OI-1
  and the E12-A3 placeholder; X11 keep-above is B7 item 4 and E26-OI-5; `QuitMode::Explicit`
  (B7 item 5) is recorded in research/claude-gpui-apis.md:58 (gpui app.rs:202).
- B8 (E22, E27, E28): done, 115 beads (E22 47, E27 35, E28 33), all 227 planned edges in, no
  cycles, plan in 16a07bf. Open items: E22 19, E27 9, E28 15 (plan file). Leader edges:
  wlf.10 (E01-008) on pdu.1 (scroll handle); E18-A1 must expose `TlTreeTable::scroll_handle`
  (check B5). Order the `window/main.rs` chain edits of E27-023, E27-025 and the E28 anchors
  with edges. Public IP fetch API joins the device-command overlap (blocks 9o0.32).
- B2 (E04 to E07): done, 111 beads (E04 32, E05 36, E06 26, E07 17), all edges checked, no
  cycles, plan in 1ac80b1. Open items E04-OI-1 to 13, E05 1 to 13, E06 1 to 11, E07-O1 to O6
  (plan file). Leader work: build line still old on 36 beads; E04-017, E04-020, E04-025 need a
  `## Notes` heading; cr0.11 and cr0.17 depend on iuz.13 instead of E07-A3 (E07-O4); PERF-050
  Partitions table has no bead (E04-OI-13); PERF-037 moves to E06 in 00-epics section 6.
  Check in the quality pass that E10 to E13 beads depend on the B2 anchors they use.
- Overlaps: udev reader is E04-OI-2 and E12-OI-4; ProcessRecord extras are E05 1 and B3 item 5;
  rustix `param` and serde_json join B1 item 2 (dependency rows).
- B5 (E15 to E18): done, 70 beads (E15 25, E16 15, E17 14, E18 16), all 139 edges in, no
  cycles, plan in d69765a. Open items 1 to 38 in the plan file. Leader edge: E33-A1 on fut.15,
  and B1's E33-OI-1 proposal follows the `UiLaunch` shape in fut.15. Check that E18-A1 exposes
  `TlTreeTable::scroll_handle` (B8).

## Owner answers on 2026-10-04

- OI-U40 themes: the two default themes ship at P0 in E17-002 (lgw.4); E17-005 (lgw.7, P2) adds the ten palettes
  when the owner exports `assets/themes/builtin.json`.
- OI-U41 share codes: TL1 import now; TS1 import when the owner supplies a sample code (U-17).
- OI-U42 app icon: placeholder now (256 px rounded square, accent colour, white monospace "tl");
  new P3 bead E15-026 swaps in final artwork.
- Build-line rewrite: done on 405 beads, 0 failures.
- OI-E14 `--force-full-memory-mode`: with `--low-memory` it starts the full engine and keeps the
  rescue window; alone it does nothing. Bead it.
- OI-E21 `--app-id`: isolates everything (config, state, cache, runtime dirs, Wayland app_id,
  bus name). PKGBUILD licence: MIT. TaskSlinger INI import: plain booleans and in-range integers
  only until the owner supplies `UserSettings.ini`.
- OI-E30 polkit texts: the three E29-021 texts as written.
- OI-E31 history chart: stacked top 3 plus "other" per bucket, in the metric colour.

## R numbering for the settled open items

OI-E01 to OI-E32 become R-139 to R-170 (R = 138 + n). OI-U01 to OI-U65 become R-171 to R-235
(R = 170 + n). The owner answers above become D-012 to D-020 in DECISIONS.md.
- v9m.1 (E02-A1): the asupersync feature check must use `cargo tree -p tl-engine -e normal,features -i asupersync` (dev-deps include test-internals). Tell the v9m.1 implementer.
- R-267 (leader, 2026-10-04): fixture capture writes the fixed UUID
  `00000000-0000-4000-8000-000000000001` to `proc/sys/kernel/random/boot_id`, and
  `00000000000040008000000000000001` to a captured machine-id file. Found by the 2dl.10 privacy
  gate. Record in RECONCILIATION and ADR-013 in the next docs pass.
