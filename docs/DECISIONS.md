# tasklens decisions log

Decisions made by Oystein, newest first. Implementation agents must not reopen these.

## 2026-10-02

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
