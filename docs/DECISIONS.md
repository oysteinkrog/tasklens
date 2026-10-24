# tasklens decisions log

Decisions made by Oystein, newest first. Implementation agents must not reopen these.

## 2026-10-25

### D-032 Network range figures on history ranges

- **Decision**: at 1 h, 24 h and 7 d the Overview Network figures (UI-PRF-055) use the per-point sum of the download and upload means. Min and peak are the smallest and largest of those sums.
- **Why**: the metric history stores each direction with its own min and max. Adding the two maxima reports a total that may never have happened, and the exact total extremes are not stored. Storing a separate total series would add an engine change for one text line.
- **Cost**: a total spike shorter than the tier step (10 s at 24 h, 1 min at 7 d) does not show in the figures. The charts still show it, because they draw each point's min and max.

## 2026-10-24

### D-031: Properties windows are normal windows

Properties windows (process, service, startup, connection) use `WindowKind::Normal`, not
`Floating`. A floating Properties window stayed above the main window and covered its menu
popups, which are drawn inside the main window (tasklens-fut.25). As normal windows, clicking
the main window raises it above them. The title bar pin (R-073) still keeps one on top when the
user asks. Tool windows and the Run window stay `Floating`.

## 2026-10-23

### D-030: Overview status, history and layout redesign

Oystein (2026-10-23, after the mockups and an interview): build a machine status bar (status
pill with the reason, chips for uptime, load, temperatures, memory pressure and swap-in, and a
state edge on each box), with a setting for where it shows (Overview, Performance tab, every
tab); a global time range of 1 min, 5 min, 1 h, 24 h and 7 days with min, average and peak per
box; device metric history kept for 7 days and downsampled (1 s for 1 h, 10 s for 24 h, 1 min
for 7 days); a clickable 24-hour timeline marking pressure and swap storms, heat and throttling,
long full load, and boots, sleeps and crashes, with a setting for what a click does (pause until
Live, preview then return, or open History mode); visual polish across the whole Performance tab
(icons, bigger values, depth and glow, legends and ticks); and a snap grid for free placement.
Order: status bar and polish first, then time range, snap grid, timeline. Specs: UI-PRF-054 to
UI-PRF-058, ADR-017.

### D-029: Memory charts, GPU engine choice, core rows, edit-mode title

Oystein (2026-10-23, using 1.0.3): Memory should be able to show a chart for each of its rows,
arranged vertically, horizontally or in a grid. The GPU box should let him pick which engine
charts to show. The CPU box looks right, but the per-core rows should show the core name and
history again. In Edit Layout mode the title values overlap the box's gear icon. UI-PRF-019 adds
the Memory `charts` mode, the GPU "Engines" submenu, core rows with name, sparkline, bar and
percent that flow into columns, and title values that end before the edit controls.

## 2026-10-22

### D-028: Power draw strip in the Overview CPU box

Oystein (2026-10-22): the CPU page's power draw graph should be available inside the Overview
CPU box, underneath the main graph and the per-core part. UI-PRF-019 adds a "Show power draw"
check item to the CPU box's Layout submenu; off by default, saved per box as `power`.

### D-027: Overview chart stacking choice

Oystein (2026-10-22): the layout inside each Overview box should be more configurable. The Disk
and Network graphs should stack vertically, or at least there should be a choice between
vertical and horizontal stacking. UI-PRF-019 adds an "Arrange" choice per box (Automatic,
Stack vertically, Side by side), saved as `arrange`, with Disk and Net stacked vertically by
default.

## 2026-10-21

### D-026: Overview GPU, Disk and Net defaults

Oystein (2026-10-21, using 1.0.1): the CPU and Memory boxes are fine. The GPU box should show a
chart for every engine (3D, video encode, video decode and the rest), the Disk graphs are much
smaller than the other graphs on the page, and the Net Download and Upload graphs should be
stacked vertically, not side by side. UI-PRF-019 changes the defaults to `engine_charts`,
`disk_charts` and a new `stacked` Net mode, and the chart grids now grow to fill their box.
A box with a saved mode keeps it; only boxes without one get the new defaults.

## 2026-10-17

### D-025: Overview box modes and inner splitters

Oystein (2026-10-18): every Overview box needs its own layout modes and draggable parts. For
example, the Memory bars are too big compared to their graphs when the box is resized, and the
GPU box should be able to show a chart for each engine (3D, encode, decode), not only the main
load. Spec in UI-PRF-019: a "Layout" submenu per box, modes per kind, one inner splitter where a
mode has two parts, all saved per box.

### D-024: CPU box splitter and smoothed core rows

Oystein (2026-10-18, after trying the layout editor): the gap between the CPU chart and the core
rows should be draggable too, and the per-core loads are too noisy and need smoothing. Spec in
UI-PRF-018. Core rows use a 1 s moving average by default, with a setting for Off, Low, Medium
and High; the core sparklines show 1-second means; the total CPU chart stays raw at 10 Hz.

### D-023: Overview layout editing

Oystein wants the Overview page layout and box sizes to be customizable (2026-10-17). He chose
direct dragging over an edit-only mode or settings presets. The spec is UI-PRF-018.

- **Normal use.** The gaps between boxes and between rows are always splitters. Dragging one
  moves size between the two neighbours only.
- **Edit Layout mode.** Entered from the page menu. Boxes move by dragging their title, can be
  hidden and added back, and "Reset Layout" restores the default. Done or Esc leaves.
- **Saved per profile** as `[performance.overview]`. It replaces `overview_top_processes`, which
  migrates once.

## 2026-10-07

### D-022: Performance Overview page

The Performance tab gets an Overview page: five btop-style boxes (CPU, MEM, GPU, DISK, NET) in
the tasklens theme, laid out from data (UI-PRF-017). The leader recorded these decisions:

- **Render contract.** GPUI 0.3.7 marks every ancestor of a notifying view dirty (crw.34), and
  tasklens does not fork GPUI. So an idle tick renders nothing, `OverviewPage` never notifies on a
  tick, a box that did not change builds no geometry, and the page render does fixed work with no
  text formatting.
- **Demand.** `cpu_fast` is true while the page is active, so the CPU box updates at 10 Hz.
- **Sidebar card.** It shows the CPU-total mini graph, a static line 1 "CPU, memory, GPU, disk,
  network" and an empty line 2.
- **Order and keys.** Overview is the first card by default and moves with `device_order`. F1
  opens Overview and F2 opens CPU.
- **Profiles.** New profiles keep `selected_device = "cpu"`.
- **GPU and Disk boxes** list the visible sidebar cards only.
- **Graph summary mode** gets no Overview tile. The page has no header row.
- **Chart motion.** The Overview charts step at the sample rate; smooth sliding is off for them only (crw.46). Revisit after wlf.31.
- **Top processes box** (owner decision 2026-10-07, crw.45). An opt-in box with the 10 busiest processes by CPU, off by default. `performance.overview_top_processes` turns it on until the layout editor exists. While it is placed the page sets `Demand.process_top`, which starts the process sweep without the history window.
- **Later.** Saving the layout and the layout editor come in later beads.
- **Fixed values.** `STACK_W` 720 px; row minimums 280 and 220 px; row weights 3:2; core row
  cell at least 180 px (215 px at 1200 × 900) with a 48 px sparkline, and the CPU chart takes 40%
  of a wide CPU box (crw.53, leader decision 2026-10-15); at most 2 GPUs and 8 disk rows; the
  per-core percent is an integer and its text is re-shaped only when its 1% bucket changes (this
  settles U-47 for Overview), and the per-core GHz text only when its 0.1 GHz bucket changes.

## 2026-10-04

D-012 to D-020 answer the owner-decision topics of the settled open items (`docs/RECONCILIATION.md`
R-139 to R-235). Every answer took the recommended option except D-017.

### D-021: The `LICENSE` copyright holder is Øystein Krog

The MIT `LICENSE` file's body starts `Copyright (c) 2026 Øystein Krog`. Committed in `daa15ab`;
E02-011 (`tasklens-v9m.13`) is closed. (Q-7, R-258.)

### D-020: The history chart stacks the top 3 processes plus "other"

Per bucket the history chart draws the three largest per-process values as a stack and the sum
of the rest as one "other" band, all in the metric colour. Header totals for a selected range are
averages over the range. (OI-E31, R-169.)

### D-019: Polkit prompt texts are the E29-021 texts

The `<description>` and `<message>` of the three polkit actions ship as E29-021 wrote them.
`...tasklens.helper`: "Run tasklens administrator actions" / "Authentication is required to let
tasklens act on other users' processes, system services and network connections."
`...tasklens.setup`: "Change tasklens system setup" / "Authentication is required to change system
settings for tasklens." `...tasklens.netmon`: "Run the tasklens network monitor" / "Authentication
is required to run the tasklens network monitor." No shorter one-line variants. (OI-E30, R-168.)

### D-018: TaskSlinger INI import reads plain booleans and in-range integers only

Until Oystein supplies a `UserSettings.ini` or the inventory section that names the INI sections
and value encodings, E33-018 imports only plain boolean and in-range integer keys. Everything else
is skipped. (OI-E21, R-159.)

### D-017: The PKGBUILD licence is MIT

`license=('MIT')` in `packaging/PKGBUILD`. D-005 keeps the repo private; this is the licence the
package declares, not a publication decision. This went against the "owner's choice, no
recommendation" line in the open item. (OI-E21, R-159.)

### D-016: `--app-id <id>` isolates everything

`--app-id <id>` replaces the `tasklens` directory component in all four XDG roots (config, state,
cache, runtime) and is the Wayland `app_id` and the single-instance bus name. History, logs, crash
reports and the icon cache follow. APP-052's "separate instance and settings dir" means this.
(OI-E21, R-159.)

### D-015: `--force-full-memory-mode` with `--low-memory` runs the full engine

With `--low-memory` it starts the engine in `EngineMode::Full` while keeping the rescue window, so
the rescue window can be tested against real data. Alone it does nothing. It gets a bead.
(OI-E14, R-152.)

### D-014: The application icon is a placeholder for now

E15-022 builds `assets/icons/tasklens.svg` as a 256 px rounded square in the accent colour with
a white monospace "tl". A new P3 bead E15-026 swaps in final artwork when Oystein supplies it.
(OI-U42, R-212.)

### D-013: Theme share codes import TL1 now, TS1 later

E17-010 ships import of the tasklens `TL1` format. TS1 import follows when Oystein generates one
TaskSlinger share code from a known theme and records it in U-17. (OI-U41, R-211.)

### D-012: Two built-in themes ship now, the ten named palettes follow from the TaskSlinger export

Default Light and Default Dark ship at P0 from hand-written values (E17-002). E17-005 adds all ten
named APP-035 palettes when Oystein exports `assets/themes/builtin.json` from TaskSlinger (U-18) and
stays blocked until then. No new bead. (OI-U40, R-210.)

## 2026-10-03

D-007 to D-010 answer the owner questions in `docs/RECONCILIATION.md`.

### D-011: Group header rows are the same height as data rows

Group header rows in the process table ("Apps (N)", "Background processes (N)", "System processes
(N)") use the data row height, so the table can use GPUI's `uniform_list`. They stand out by bold
text, a background band and the count. TaskSlinger's are about 36 px tall; this is a deliberate
small difference for scrolling speed.

### D-010: History mode shows the last 10 minutes when recording is off

While recording is off, tasklens keeps a rolling 10-minute window of process history in RAM, only
while the Processes tab is open. History mode shows that window. This overrides R-025 and restores
r2 PROC-074. Turning recording on still switches to the ADR-012 store.

### D-009: A single End Process on your own process does not ask first

End Process (SIGKILL) on one of your own, unprotected processes shows only the toast "End process
requested." End Process Tree, Kill App, multi-select and protected processes still ask.

### D-008: Enter opens Properties on a process row

Enter, Ctrl+I and Alt+Enter open Properties. Space toggles expand.

### D-007: Memory module serial numbers are hidden everywhere

Not on the Memory page, the Information tab or in any copy text. The udev serial key is not read.

## 2026-10-02

### D-006: tasklens draws its own title bar by default

The default is a client-drawn title bar in TaskSlinger's layout: menus and toolbar beside the
caption buttons. A setting switches to the KWin title bar. This overrides the lower-risk
server-side suggestion in the GPUI research.

### D-005: The name is tasklens and the repo stays private

The app ID is `io.github.oysteinkrog.Tasklens`. The D-Bus names, polkit action prefix
(`io.github.oysteinkrog.tasklens.`) and XDG folders (`tasklens`) follow from it. Publishing later
is Oystein's call. No TaskSlinger name, icons or text ship either way.

### D-004: The default Memory column shows private memory

The default "Memory" column is RssAnon, the closest match to TaskSlinger's private working set.
This overrides the architecture review's RSS recommendation. RSS, PSS and USS stay available as
extra columns, and the header tooltip says which figure the column shows.

### D-003: Process history recording is off by default

Settings has one switch to turn it on. Retention defaults to 6 hours when on. This follows the
architecture review rather than TaskSlinger.

### D-002: Fable does the cross-model reviews until Codex is back

The Codex account is rate-limited until Sunday 4 October 2026 at 11:12. Until then, Fable
subagents do the second-opinion reviews. When Codex is back, GPT-6 Astra reviews the finished
plan and beads one more time.

### D-001: Keep asupersync despite its license rider

asupersync 0.5.0 ships under "MIT License (with OpenAI/Anthropic Rider)". The rider grants no
rights to OpenAI or Anthropic and bars making the software or derivative works available to
them, including for automated analysis and testing. tasklens is planned, built and tested with
Claude and Codex agents.

Oystein reviewed this and chose to keep asupersync and accept the risk. Source:
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/LICENSE`.

Consequence: if the rider has to be honoured later, the engine runtime must be swappable. Keep
asupersync types inside `tl-engine` and out of `tl-model`, `tl-collect` and `tl-ui`.

Oystein (2026-10-21) chose to publish the repo as public at github.com/oysteinkrog/tasklens with
this rider known. The repo holds no asupersync code, only the version pin.
