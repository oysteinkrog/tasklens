# Bead Model Rubric

Owner decision, 2026-10-15. Beads are written or prepared by Opus at least, in enough detail that Sonnet can
implement and verify them. Each bead carries one label that says which model implements it.

## Labels

| Label | Use when |
|---|---|
| `model:sonnet` | Default. After preparation, the bead leaves nothing to decide: named files and functions, a precedent to copy, concrete steps, named tests. |
| `model:opus` | Any of: async, cancellation or supervision in the engine (asupersync regions, Cx, LabRuntime timing); concurrency or locking; perf work and GPUI render caching internals; security-relevant code (tl-helper, polkit, privileges, signals to other users' processes, SockDestroy); a cross-crate model change with judgement calls left; a bead a Sonnet lane failed twice. |
| `model:fable` | Only with a written reason in the bead: frontier-hard work (a new algorithm, a design with no precedent), or a bead Opus failed twice. |

When unsure between two labels, pick the lower one if the bead text can be made explicit enough, and the higher
one if the uncertainty is in the domain itself (timing, safety, perf), not in the wording.

## What a prepared bead contains

Added as a comment headed `Prepared (model:<x>)`, without rewriting the original text:

1. Files to edit, each with the functions or line ranges, and which ones are chain edits.
2. The precedent to copy: an existing file:line that already does the same kind of thing.
3. Steps in order, small enough that each one is a single edit.
4. Tests: test file, test names, and what each asserts, mapped to the acceptance criteria.
5. The exact check commands (heavy-build clippy and test invocations).
6. Pitfalls found while reading: names that differ from the bead text, missing model fields, other beads touching the same files.

If preparation finds a decision the docs do not settle, the bead is not labelled. It gets a comment headed
`Needs leader decision` with the question and the options, and stays open for the leader.

## Escalation

- A `model:sonnet` bead that fails review twice gets one `model:opus` attempt before it is held.
- A `model:opus` bead that fails twice is held for the leader, who may consult Fable or Astra.
- Reviews: GPT-6 Astra through Codex (read-only); a Claude model of the same label when Astra is unavailable.
