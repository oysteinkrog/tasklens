# tasklens: rules for implementation agents

tasklens is a native Linux task manager in Rust: GPUI through `gpui-kit =0.7.0` for the UI,
asupersync 0.5.0 for the engine, stable rustc 1.98.1. It copies every feature of TaskSlinger 0.9.2
and Mission Center 1.2.0. UI and rendering speed come first.

## Where the truth is

Every piece of work is a bead (`br`). The bead text is the spec for your work. When it cites a
source, these win in this order:

1. `docs/DECISIONS.md` (owner decisions D-nnn)
2. `docs/RECONCILIATION.md` (R-nnn, canonical tables a to d, unverified list U-nn)
3. `docs/ARCHITECTURE.md` (ADR-001 to ADR-016: crates, modules, types, budgets, tests)
4. `docs/spec/merged/{proc,perf,sys-app}.md` and `docs/ui/*.md`
5. `docs/plan/00-epics.md` (epics, file ownership, chain-edit rules)

Do not make significant decisions. If the bead and the docs do not settle something you need,
stop, leave the bead open, and report the question in your final message.

## One bead, one commit

- Edit only the files your bead names. Chain edits (a bead that adds a line to a file owned by
  another epic) touch only the lines the bead describes.
- For a large new file, commit it before you build or test.
- Commit with a pathspec, never a bare `git commit`. Other agents share this index:
  `git add <your files> && git commit -m "<crate>(<bead id>): <what>" -- <your files>`.
- Fix-ups go in a follow-up commit. Never amend, reset, rebase, switch branch or stash.
- Close the bead when done: `br close <id> --reason "<what changed, commit sha>"`.

## Beads tool

- Always pass `--lock-timeout 180000` to `br`. Add `--no-auto-flush` to writes.
- Never open `.beads/beads.db` with sqlite3 or Python. Read beads with `br show <id> --json`.
- Never run bare `bv` (it opens a TUI). Use `bv --robot-*` flags only.

## Builds and tests

- Run every cargo build, check, clippy, test, doc and bench through `heavy-build`, for example
  `heavy-build cargo test -p tl-model`. It waits for the machine-wide build lock and caps memory.
  On exit 137 (OOM kill) retry once, then report. On exit 75 (low disk) stop and report.
- Never set `CARGO_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` or `build.build-dir`. Use the workspace
  `target/`.
- Never build or clone in `/tmp`. Scratch work goes in `/var/tmp/claude/`.
- Tests follow ADR-013: engine logic uses `LabRuntime` tests, parsers use fixtures plus proptest,
  UI uses GPUI `TestAppContext` tests. Security-relevant work needs a regression test.
- asupersync 0.5.0 lab runtime: `run_until_idle` does not fire due timers when no task is ready.
  After `advance_time`, call `lab.step_for_test()` once, then `run_until_idle()` (see
  `Rig::advance` in `crates/tl-engine/tests/lab_source_loop.rs`). Cancelling the task that awaits
  `run_supervised` does not stop the supervisor; cancel its region or call the handle's `abort()`.
- A module that has no `mod` line yet is tested through a temporary integration test that pulls
  it in with `#[path]`. Name it `tests/tmp_<bead>_<name>.rs`, never commit it, and delete it
  before you close the bead.
- Before closing: `cargo fmt --check`, `cargo clippy -p <crate> --all-targets -- -D warnings` and
  the bead's tests must pass.

## Safety

- Never kill a process you did not start.
- Never print or commit a secret.
- Ask the leader before adding any crate that ADR-001 and the R entries do not allow.

## Writing

Comments, commit messages and user-facing text: plain, clear language, short sentences, no em
dashes, exact values.
