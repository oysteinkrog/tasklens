# Shared brief: merged feature specs (round 3)

tasklens is a native Linux task manager (Rust, GPUI via gpui-kit, asupersync engine) that replicates
every feature of TaskSlinger 0.9.2 and Mission Center 1.2.0. Implementation agents must not make
significant decisions later, so the merged specs must be complete and consistent.

Read first, fully:
- `docs/DECISIONS.md` (D-001 to D-010, binding)
- `docs/RECONCILIATION.md` (R-001 to R-074, owner answers, canonical tables a to d, unverified list).
  It overrides every spec and ADR where it speaks.
- `docs/ARCHITECTURE.md` (ADR-001 to ADR-016)

Precedence: DECISIONS, then RECONCILIATION, then ARCHITECTURE on engineering structure, then the
source specs.

Your job: produce one merged spec for your area that:
1. Contains every feature from your source files, merged by subject. Nothing is dropped unless a
   resolution or decision drops it; then keep a one-line entry saying "Dropped: R-0xx".
2. Applies every resolution, decision and canonical-table value. Use the canonical settings keys,
   shortcuts, names, paths and number formats exactly. Never copy a superseded value.
3. Uses new stable IDs with your prefix, numbered from 001 in reading order, and gives each entry a
   "Was:" line listing every old ID it absorbs (for example "Was: v1 PROC-020, r2 PROC-020, add
   PROC-102"), so reviewers can trace it.
4. Keeps the entry shape: Source, Behaviour, Linux data/mechanism, Verdict, Phase (P0 to P4 per
   R-072), Privilege, and Open question only if something is still undecided (give the
   recommended answer and add it to the end list).
5. Ends with: a summary table (ID, name, verdict, phase); an old-ID to new-ID map covering every
   old ID in your sources; and a list of anything still open.

How to write the output (the terminal restarts often and kills long replies):
- Write the file directly with the Write tool at the path you were given, starting with the
  header and the first section, then add later sections with Edit (append) as you finish each one.
- After each section, commit it: `git -C /home/oystein/work/tasklens add <file>` then
  `git -C /home/oystein/work/tasklens commit -m "<message>" -- <file>`, ending the message with
  these two lines:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`
- Commit only your own file, always with the pathspec. Never switch branches, reset, rebase or
  amend. Other agents commit to the same branch at the same time.
- If the Write tool is blocked, send the content to "team-lead" with SendMessage instead, in parts
  under 10,000 characters labelled "PART n: <section>".
- Your final message is short: the file path, entry count and the open list.

Machine rules: do not run cass. Do not run builds.

Writing rules: plain, clear language; short sentences; no em dashes; no hype words; exact labels,
units and numbers.
