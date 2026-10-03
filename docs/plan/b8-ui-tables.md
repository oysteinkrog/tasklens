# Batch B8 ui-tables: bead plan for E22, E27, E28

This file lists every bead of batch B8: the process table page (E22), the shared system-table
frame with the Services page and Service Properties (E27), and the Startup Apps, Connections and
Information pages (E28). The rules are in `docs/research/brief-beads.md` and
`docs/plan/00-epics.md`. Epic and anchor bead IDs come from `docs/plan/anchors.md`.

Conventions used in every bead of this batch:

- Bead priority equals the phase number (P1 = 1 and so on). The E22-A1 anchor is P0.
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, plus `ui`, `security` or `perf-gate`
  where they apply.
- Cross-batch dependencies point only at anchor beads. Dependencies inside the batch may point at
  any bead of the batch.
- Each epic section has the plan-key to bead-ID table, then the open items for the leader.
