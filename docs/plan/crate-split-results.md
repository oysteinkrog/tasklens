# Crate split results: edit-loop times before and after

Bead tasklens-zljr5.47. Measured 2026-11-03 on commit d834998ff (after beads .43, .44, .46 and
.50). The old numbers are the 2026-11-01 baseline from FINDINGS.md section 1 (also in
`crate-split.md` section 1).

## Method

Same as the baseline: warm target, make one edit, time the command, repeat three times, take the
median. `INCR=1` is `CARGO_INCREMENTAL=1` (the current setting). `INCR=0` is
`CARGO_INCREMENTAL=0`. Each series starts with one untimed warm run.

Differences from the baseline run, so the numbers are read correctly:

- The run used a private git worktree with its own `target/`, so no other lane's build or edit
  touched it. The cold build was therefore a real cold build.
- Edits: a comment edit appends one comment line. A body edit appends a new private function. A
  new test appends one `#[test]` function. Edited files: `tl-ui-kit/src/components/button.rs`
  (comment), `tl-ui-overview/src/overview/mem_box.rs` (body), `tl-model/src/units.rs` and
  `tl-ui-settings/src/layout.rs` (body), `tl-ui-overview/tests/overview/overview_widgets.rs`
  (test), `tl-ui-kit/src/components/badge.rs` (body).
- Every command ran through `heavy-build` with `--locked`. Time is measured inside the job, so
  waiting in the queue is not counted.
- The machine was busy the whole time. Load average (1 minute) before each run is in the raw
  log. It was 10 to 17 for the cold build and the INCR=1 leaf rows, 26 to 44 for the INCR=0 rows
  and the later rows, and peaked at 142 once. Trust the ratios more than the seconds, and trust
  the unit counts most.
- The old table has no unit counts, only "9 crates rebuilt" for the tl-model row.

## Old table (2026-11-01, one `tl-ui` crate)

| Edit | Command | INCR=0 | INCR=1 |
|---|---|---|---|
| leaf file comment | `check -p tl-ui` | 23 s | 7 s |
| leaf file comment | `build -p tl-app` | 122 s | 16 s |
| leaf file comment | `test --no-run` (ui binary) | 259 s | 23 s |
| leaf body | `build -p tl-app` | 125 s | 24 s |
| leaf body | `test --no-run` | 223 s | 34 s |
| tl-model edit | `build` (9 crates rebuilt) | 141 s | 17 s |
| new test fn | `test --no-run` | 85 s | 37 s |
| components/badge.rs | `test --no-run` | 186 s | 71 s |

## New table (2026-11-03, layered crates)

Median of three runs. "Units" is the number of crates cargo compiled or checked.

| Edit | Old command | New command | INCR=0 | INCR=1 | Units |
|---|---|---|---|---|---|
| leaf comment, kit component | `check -p tl-ui` | `check -p tl-ui-kit` | 1.8 s | 0.9 s | 1 |
| leaf comment, kit component | `build -p tl-app` | `build -p tl-app` | 48.7 s | 7.6 s | 10 |
| leaf comment, kit component | `test --no-run` (ui binary) | `test -p tl-ui-kit --no-run` | 17.6 s | 2.8 s | 2 |
| leaf body, Overview box | `build -p tl-app` | `build -p tl-app` | 40.9 s | 5.5 s | 4 |
| leaf body, Overview box | `test -p tl-ui --test ui --no-run` | `test -p tl-ui-overview --no-run` | 24.3 s | 3.3 s | 1 |
| tl-model edit | `build -p tl-app` | `build -p tl-app` | not measured | 22.5 s | 19 |
| tl-ui-settings edit | (new row) | `build -p tl-app` | not measured | 14.8 s | 12 |
| new test fn | `test --no-run` (ui binary) | `test -p tl-ui-overview --no-run` | not measured | 5.4 s | 1 |
| components/badge.rs body | `test --no-run` | `test -p tl-ui-kit --no-run` | not measured | 4.1 s | 2 |
| components/badge.rs body | `test --no-run` | `test -p tl-ui-perf --no-run` | not measured | 8.4 s | 7 |

Individual runs (seconds):

| Row | INCR=0 | INCR=1 |
|---|---|---|
| check kit | 1.6, 2.0, 1.8 | 0.9, 1.0, 0.9 |
| build app, kit comment | 45.9, 48.7, 65.0 | 7.6, 6.8, 7.6 |
| test kit, kit comment | 21.2, 17.1, 17.6 | 2.8, 2.8, 2.8 |
| build app, overview body | 39.5, 40.9, 42.9 | 5.8, 5.5, 5.3 |
| test overview, overview body | 77.0, 21.7, 24.3 | 2.9, 4.9, 3.3 |
| build app, tl-model edit | | 22.5, 17.8, 33.2 |
| build app, tl-ui-settings edit | | 14.8, 13.4, 15.8 |
| test overview, new test fn | | 3.6, 6.9, 5.4 |
| test kit, badge body | | 4.1, 3.5, 10.6 |
| test perf, badge body | | 8.4, 8.1, 8.5 |

The 77.0 s run is an outlier: load average was 142 at that moment. The median ignores it.

## Ratio per row (old time divided by new time)

| Row | INCR=0 | INCR=1 |
|---|---|---|
| leaf comment, check | 12.8x | 7.8x |
| leaf comment, build tl-app | 2.5x | 2.1x |
| leaf comment, test --no-run | 14.7x | 8.2x |
| leaf body, build tl-app | 3.1x | 4.4x |
| leaf body, test --no-run | 9.2x | 10.3x |
| tl-model edit, build tl-app | not measured | 0.8x (slower) |
| new test fn, test --no-run | not measured | 6.9x |
| badge.rs, test -p tl-ui-kit | not measured | 17x |
| badge.rs, test -p tl-ui-perf | not measured | 8.5x |

## Other rows

| Measure | Old | New |
|---|---|---|
| Cold `cargo build -p tl-app` | 381 s, 873 units | 424 s, 668 units |
| Test binary, tl-ui `ui` | 366 MB (80 files) | split, see below |
| Total `target/` after these builds | not recorded | 14 GB (private worktree, debug only) |

Cold build: 668 units against 873 (205 fewer, 23 percent). The wall time is 11 percent higher.
The load was 10 when it started and rose to 21 during the next job, and the old run was also on a
loaded machine, so the seconds do not tell which is faster. The cold build is the only row where
the split does not show a gain in seconds. It was not expected to: the same code is compiled, in
more crates with more link steps. The reason for the lower unit count was not checked.

New test binaries (debug, `target/debug/deps`, newest build of each):

| Crate | Test binary | Size |
|---|---|---|
| tl-ui-kit | `kit` (integration tests) | 70.9 MB |
| tl-ui-kit | `tl_ui_kit` (unit tests) | 105.0 MB |
| tl-ui-overview | `overview` (integration tests) | 95.8 MB |
| tl-ui-overview | `tl_ui_overview` (unit tests) | 36.4 MB |
| tl-ui-perf | `perf` (integration tests) | 162.2 MB |
| tl-ui-perf | `tl_ui_perf` (unit tests) | 46.1 MB |

The other test crates (base, chart, table, perf-core, procs, sys, tl-ui) were not built, because no
row needs them. The old single `ui` binary was 366 MB. A lane links only the binary of the crate
it edits. The largest one measured, `perf`, is 44 percent of 366 MB; `kit` is 19 percent and
`overview` is 26 percent.

## Feature flip

`build -p tl-app` (no features), then `test -p tl-ui-kit --no-run` (dev features):

- Starting from the cold `build -p tl-app`, the first `test -p tl-ui-kit --no-run` compiled 117
  units in 99 s. Of the tl-ui crates, only `tl-ui-base` and `tl-ui-kit` were compiled a second
  time (a variant with `test-hooks`), plus `tl-ui-pagehost` and `tl-ui-testkit`, which the app
  build does not use. The rest of the 117 are GPUI and its dependencies, built again with gpui-kit
  `test-support`.
- No other tl-ui crate (chart, table, perf-core, overview, perf, procs, sys, tl-ui) compiled
  twice in this flip. They compile twice only when a test target of that crate is built, since
  `test-hooks` is then on.
- After that, `build -p tl-app` and `test -p tl-ui-kit --no-run` each took under 1 s and compiled
  nothing: both variants stay in `target/` side by side.

The GPUI stack still has two variants (with and without `test-support`), as the old plan says. The
lane commands in ADR-020 keep a lane on one variant.

## What the numbers say

- A kit comment edit checks in 0.9 s and builds the kit test binary in 2.8 s (INCR=1). The old
  one-crate numbers were 7 s and 23 s.
- An Overview edit now rebuilds one crate for its tests (3.3 s), against 34 s.
- The `build -p tl-app` rows gain less (2 to 4x), because cargo recompiles every crate above the
  edited one and then links tl-app. A kit edit rebuilds 10 units, a box edit 4.
- Settings and tl-model edits are the weak row. `tl-model` is at the bottom of the graph, so an
  edit rebuilds 19 units (old: 9) and takes 22.5 s against 17 s. A `tl-ui-settings` edit rebuilds 12 units
  in 14.8 s. These crates change rarely, but a lane that does change them pays for all the
  layers above.
- INCR=0 is not a good setting for the split: every measured row is 6 to 7 times slower than INCR=1 (build and test rows; check rows 2x).
  The current INCR=1 setting is the right one.

## Not measured

- INCR=0 for the tl-model, tl-ui-settings, new-test and badge.rs rows. The bead asks for INCR=0
  only on the two leaf rows.
- The old baseline had no unit counts, so the unit comparison exists only for the cold build and
  the tl-model row.

## Raw data

The raw log (every run, load average before it, units compiled and the crates by name) was kept
at `/var/tmp/claude/zljr5-47/results.tsv` and is not committed.
