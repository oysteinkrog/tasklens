# tasklens decisions log

Decisions made by Oystein, newest first. Implementation agents must not reopen these.

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
