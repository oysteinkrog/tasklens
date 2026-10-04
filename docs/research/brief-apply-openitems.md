# Shared brief: apply the settled open items (round 7)

The open items from the bead round are settled in `docs/plan/open-items-engine.md` (OI-E01 to
OI-E32) and `docs/plan/open-items-ui.md` (OI-U01 to OI-U65). The owner answered the owner-decision
topics; the answers are in `docs/plan/leader-notes.md` under "Owner answers on 2026-10-04". Every
owner answer took the recommended option, except the PKGBUILD licence, which is MIT.

Fixed numbering (do not renumber):

- OI-E<n> is R-(138 + n): OI-E01 is R-139, OI-E32 is R-170.
- OI-U<n> is R-(170 + n): OI-U01 is R-171, OI-U65 is R-235.
- Owner answers are D-012 to D-020 in `docs/DECISIONS.md`, in this order: D-012 themes (OI-U40),
  D-013 share codes (OI-U41), D-014 app icon (OI-U42), D-015 `--force-full-memory-mode` (OI-E14),
  D-016 `--app-id` scope (OI-E21), D-017 PKGBUILD licence MIT (OI-E21), D-018 TaskSlinger INI
  import (OI-E21), D-019 polkit texts (OI-E30), D-020 history chart (OI-E31).

## Roles

- **Docs agent** (the only one editing docs): adds R-139 to R-235 to `docs/RECONCILIATION.md`
  (one entry per OI topic: question, answer with values, basis) and D-012 to D-020 to
  `docs/DECISIONS.md`, then applies every "Changes" doc edit to ARCHITECTURE, the merged specs,
  the UI specs and `docs/plan/00-epics.md` as current-state text (no strikethrough, no "was"
  notes). Adds the unverified items that remain to the U-nn list.
- **Bead agents** (the only ones running `br` writes): apply every bead change in their file's
  "Changes" lines, "New beads" table and "Edges" section. One agent per file.

## Bead rules

- Every `br` write uses `--no-auto-flush --lock-timeout 180000`. Run one `br` command at a time.
  Never open `.beads/beads.db` directly. `.beads/issues.jsonl` is stale; read beads with
  `br show <id> --json`.
- Edit a bead by fetching its description, changing only the affected lines, and writing the whole
  text back with `br update <id> --description "<text>"`. Cite the R or D number next to each
  changed value. Keep the bead template from `docs/research/brief-beads.md`.
- Remove `blocked-open-item` labels from beads the settlement unblocks (`br update <id>
  --remove-label blocked-open-item`, check `br update --help` for the exact flag).
- New beads follow `docs/research/brief-beads.md` in full (template, labels, priority by phase,
  parent epic, build rule `heavy-build cargo ...` with the exit 137 and 75 sentence). Record each
  new plan key and bead ID in the batch plan file that owns the epic, and commit that file.
- Add edges with `br dep add <bead> <depends-on>`, remove with `br dep remove`. Close duplicate
  beads with `br close <id> --reason "duplicate of <id> (R-nnn)"`.
- Keep a done-file in your scratch folder and make the script resumable: the terminal restarts
  often.
- At the end run `br dep cycles` (must be empty).

## Commits

Commit only your own files, with a pathspec, after each logical chunk:
`git -C /home/oystein/work/tasklens commit -m "<message>" -- <files>` (after `git add`), ending
with:
`Co-Authored-By: Claude <your model name> <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`
Never switch branches, reset, rebase or amend. The leader commits `.beads/`. No builds, no
`cass`.

Final message, short: what you changed (counts), anything you could not apply and why.

Writing rules: plain, clear language; short sentences; no em dashes; no hype words; exact values.
