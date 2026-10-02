# tasklens decisions log

Decisions made by Oystein, newest first. Implementation agents must not reopen these.

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
