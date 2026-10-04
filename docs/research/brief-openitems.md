# Shared brief: settle the bead round's open items (round 6)

The bead writers finished. Each batch plan file in `docs/plan/` ends its epic sections with open
items: things the docs did not decide, so the writers stopped and asked. Your job is to turn them
into settled answers the leader can apply as R-139 onward, so that implementation agents make no
significant decisions.

## Inputs (precedence order)

1. `docs/DECISIONS.md` (D-001 to D-011)
2. `docs/RECONCILIATION.md` (R-001 to R-138, canonical tables a to d, unverified list U-nn)
3. `docs/ARCHITECTURE.md` (ADR-001 to ADR-016)
4. `docs/spec/merged/{proc,perf,sys-app}.md`, `docs/ui/*.md`
5. `docs/plan/00-epics.md`, `docs/plan/anchors.md`, `docs/plan/leader-notes.md`
6. The batch plan files listed in your prompt, and the beads themselves. Read beads only with
   `br show <id> --json` (read-only). Never open `.beads/beads.db` with sqlite3 or Python, and
   never run `br create`, `br update` or `br dep add`: the leader applies all bead changes.
   `.beads/issues.jsonl` is stale; do not trust it.

## What to produce

One file at the path in your prompt. For each settled topic:

```markdown
### OI-<E|U>nn <short plain title>
- Raised by: <batch item IDs, for example B3-13, E12-OI-1, B7-5>
- Question: one or two sentences.
- Answer: the decision, with every value an implementer needs (types, fields, names, numbers).
- Basis: which doc entries support it; tag evidence [verified] (read in source or docs),
  [inferred] or [guess].
- Owner decision: no | yes (only for real product or scope choices; give the recommended answer
  anyway, and the alternatives in one line each)
- Changes: doc edits (file and section) and bead edits (bead ID: what to add or change, or a new
  bead with plan key, title, epic, phase, files and dependencies).
```

Rules:

- Merge duplicates across batches into one topic and list every raiser. Topics in the leader
  notes' "Overlaps" lines are known duplicates.
- Where a writer already wrote a recommendation and the beads follow it, check it against the docs.
  If it holds, accept it in one short entry. Spend your effort on the items that block beads
  (label `blocked-open-item`, or "stop and report" in a bead), contract changes in tl-model, and
  crate or `unsafe` allow-list questions.
- Never invent product behaviour. If the docs and the two source apps (TaskSlinger 0.9.2, Mission
  Center 1.2.0 inventories under `docs/research/`) do not settle it, mark it Owner decision: yes.
- For a library API question, look in the source if it is on disk (`~/.cargo/registry/src`,
  `~/src`), else use the research notes under `docs/research/` and tag [inferred].
- Cross-batch dependency edges and chain-edit ordering go in a final section "Edges", as a list
  of `br dep add <bead> <depends-on>` lines with a reason each.
- End with a summary table (OI ID, title, owner decision yes or no, beads unblocked).

## How to write

The terminal restarts often. Write the file with the Write tool starting with the header and the
first few topics, then append with Edit as you go. Commit after every 10 or so topics:
`git -C /home/oystein/work/tasklens add <file>` then
`git -C /home/oystein/work/tasklens commit -m "<message>" -- <file>`, ending the message with:
`Co-Authored-By: Claude <your model name> <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`

Commit only your file, with the pathspec. Never switch branches, reset, rebase or amend. No builds,
no `cass`. Final message short: file path, topic count, the owner-decision list.

Writing rules: plain, clear language; short sentences; no em dashes; no hype words; exact values.
