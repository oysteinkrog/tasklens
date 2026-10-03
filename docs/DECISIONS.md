# tasklens decisions log

Decisions made by Oystein, newest first. Implementation agents must not reopen these.

## 2026-10-03

These answer the owner questions in `docs/RECONCILIATION.md`.

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
