# Shared brief: beads (round 5)

tasklens is a native Linux task manager (Rust, GPUI via gpui-kit 0.7.0, asupersync engine). The
owner wants "everything beaded and fully planned" so that "implementation agents" do not "make any
significant decisions". A bead is one unit of work for one implementation agent, done in one
commit. The agent sees only the bead text plus the repo, so every bead must stand alone.

## Inputs (precedence order)

1. `docs/DECISIONS.md` (D-001 to D-011)
2. `docs/RECONCILIATION.md` (R-001 onward, canonical tables a to d, unverified list U-nn)
3. `docs/ARCHITECTURE.md` (ADR-001 to ADR-016: crates, module layout, types, budgets, tests)
4. `docs/spec/merged/{proc,perf,sys-app}.md` (feature behaviour)
5. `docs/ui/*.md` and `docs/ui/INDEX.md` (UI behaviour, vocabulary, coverage)
6. `docs/plan/00-epics.md` (epic map, file ownership, anchor beads; written first in this round)

## Bead rules

- **Size**: 1 to 3 source files plus their tests. One responsibility. One commit. If a file is
  large (for example the process delegate), split by feature into several beads that touch it in
  sequence with explicit dependencies.
- **Self-contained**: the description repeats every value the implementer needs (labels, sizes,
  keys, constants, formats, exact toast text, struct fields, function signatures). Cite the source
  IDs too, but never write "see the spec" in place of the value.
- **No decisions left**: if the docs do not decide something the bead needs, stop and list it as
  an open item for the leader. Do not invent.
- **Tests**: every bead names its tests (file, test name, what it asserts). Engine logic uses
  LabRuntime tests, parsers use fixtures plus proptest, UI uses `TestAppContext` tests, per
  ADR-013. Security-relevant beads (helper, actions, privileges, sanitising) need a regression test.
- **Phase**: P0 (spike) to P4, from the specs. Bead priority follows phase: P0 spike = 0, P1 = 1,
  P2 = 2, P3 = 3, P4 = 4. Within a phase, foundations that block many beads may go one higher.
- **Labels**: `phase-pN`, `crate-<name>` (for example `crate-tl-ui`), `area-<epic slug>`, and
  `ui` when it renders, `security` when it touches privileges or untrusted input, `perf-gate` when
  it carries an ADR-014 gate.
- **Dependencies**: a bead depends on every bead whose output it compiles against or tests
  against. No cycles. Prefer depending on the anchor beads named in `00-epics.md` for cross-epic
  edges.
- **Large-file rule**: for a bead that creates a large file, the implementer commits the file
  before building or testing (CLAUDE.md). Say so in the Notes of such beads.
- **Build rule** (goes into the Notes of every bead that builds or tests): run every cargo
  build, test, check, clippy or doc through `heavy-build`, for example
  `heavy-build cargo test -p tl-model`. It waits for the machine-wide build lock and runs in a
  memory-capped scope. On exit 137 (OOM kill) retry once, then report. On exit 75 (low disk) stop
  and report.

## Bead description template

```markdown
## Context
What this builds and why, in 2 to 5 sentences. Sources: spec IDs, UI IDs, ADR, R and D entries.

## Deliverable
Exactly what exists when done: types with fields and signatures, functions, elements, constants,
strings. Values written out in full.

## Acceptance Criteria
- **Given** <specific setup> **When** <one action> **Then** <observable result>
(One per behaviour. Negative and edge cases each get their own. No prose criteria.)

## Files
- `crates/<crate>/src/<path>.rs`: create or change, and what
- `crates/<crate>/tests/<name>.rs`: tests

## Tests
- `<file>::<test_name>`: what it asserts

## Cross-cutting
- Errors: exact error states and user text, or N/A
- Validity: which Validity states this value can show, or N/A
- Accessibility: keyboard path and focus, or N/A
- Performance: budget and what not to allocate, or N/A
- Privacy: what must never be logged or copied, or N/A

## Notes
Gotchas, API pointers (file:line in the gpui or asupersync sources), build rule.
```

## Creating beads

- Use `br create` with `--title`, `-t task` (or `epic` for epics), `-p`, `-l`, `--parent <epic>`,
  `--lock-timeout 30000`, and `-d` with the full description (use a heredoc or `--file`). Add
  `--silent` to get just the ID. Several agents create beads at the same time, so always pass the
  lock timeout.
- Add dependencies with `br dep add <bead> <depends-on>` after both exist.
- Keep a mapping table in your plan file: plan key (for example `E07-012`) to bead ID.
- At the end run `br dep cycles` (must be empty) and `br lint` for your beads.
- Never run bare `bv`. Never run git commands that change branches or history. Commit your plan
  file with a pathspec. The leader commits `.beads/` itself.

## Commits

Commit your plan file after each epic you finish:
`git -C /home/oystein/work/tasklens commit -m "<message>" -- <file>` (after `git add`), ending the
message with:
`Co-Authored-By: Claude <your model name> <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`

Machine rules: no builds in this round. `cass` by name only, never index.

Writing rules: plain, clear language; short sentences; no em dashes; no hype words; exact values.
