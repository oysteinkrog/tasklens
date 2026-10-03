# Resume brief for bead writers (after a terminal restart)

A terminal restart killed the bead writers partway. Beads already in the database are kept. You
continue one batch. Read `docs/research/brief-beads.md` first and follow it, then this file.

## Before writing anything

1. List what exists for each of your epics: `br list --parent <epic id> --json --limit 500
   --lock-timeout 180000` (epic IDs in `docs/plan/anchors.md`). Also read your plan file in
   `docs/plan/` if it exists, and your batch's scratch folder
   `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/<batch>/`
   if it exists (earlier writers kept bead drafts, key-to-ID maps and dependency lists there).
2. Compare against the epic's scope in `docs/plan/00-epics.md` (section 1 scope and sources,
   section 2 files, section 6 coverage). Work out what is missing: beads not created, anchors
   still holding the "Placeholder" text, dependencies not added, plan file sections not written.
3. Never create a second bead for work that already has one. If a bead exists but its
   description is incomplete, fix it with `br update`.

## Then

- Finish the missing beads, anchor descriptions and dependencies, as in the main brief.
- Check dependencies of existing beads: `br dep list <id>` for a sample, and re-add any missing
  edge from the drafts or dependency lists in the scratch folder. `br dep add` of an existing
  edge is harmless.
- Write or complete your plan file with the full plan-key to bead-ID table, and commit it after
  each epic (pathspec commit of the plan file only).
- Always pass `--lock-timeout 180000` to `br`. Eight writers share one database lock. Add edges
  one per command; do not run several `br` commands in parallel.
- Keep scripts and drafts in your scratch folder so a later restart can resume from them.
- Build rule for bead Notes: `heavy-build cargo ...` (see the main brief).

Final message, short: beads per epic now, what you added, open items, `br dep cycles` result.
