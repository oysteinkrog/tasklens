# Batch B6 ui-performance: bead plan for E19, E24, E25, E26

This file lists every bead of batch B6: the chart element (E19), the Performance tab frame with
the sidebar and the CPU page (E24), the device pages with the SMART and eject windows (E25), and
the minigraph panel, detached graph windows and mini window (E26). The rules are in
`docs/research/brief-beads.md` and `docs/plan/00-epics.md`. Epic and anchor bead IDs come from
`docs/plan/anchors.md`.

Conventions used in every bead of this batch:

- Bead priority equals the phase number. The P0 anchors (E19-A1, E19-A2, E24-A1) and the beads
  they depend on are priority 0.
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, plus `ui` when it renders and
  `perf-gate` when it carries an ADR-014 gate. No bead in this batch touches privileges.
- Cross-batch dependencies point only at anchor beads in `anchors.md`. Dependencies inside the
  batch may point at any bead of the batch.
- Every bead that introduces user-visible strings appends its Fluent keys to
  `assets/locales/en-US.ftl` (chain edit, R-132) in the same commit; the bead's Files section says
  so where it applies.
- Every bead's Notes carry the build rule from the brief.

## E19 ui-chart (epic `tasklens-yrf`)

Files owned: `crates/tl-ui/src/chart/{mod,element,view,geometry,grid,scale,labels,hover,grid_element}.rs`,
`crates/tl-ui/src/anim.rs`, `crates/tl-ui/tests/{ui_chart,chart_geometry,ui_anim,render_chart}.rs`.
Chain edits: two `pub mod` lines in `crates/tl-ui/src/lib.rs` (E16-A1), four golden PNGs under
`crates/tl-ui/tests/golden/` (E15-A3).

`chart/grid.rs` holds the grid quads and `chart/grid_element.rs` the `ChartGridElement`, following
the file list in `00-epics.md` section 2 (UI-PRF-002 names `grid.rs` for both).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E19-001 | tasklens-yrf.3 | Chart data types, `ChartSpec`, `ChartCache`, `ChartView` skeleton | P0 | E03-A1, E16-A1, E17-A1 |
| E19-002 | tasklens-yrf.4 | Geometry 1: time mapping, runs, decimation, stale rule | P0 | E19-001 |
| E19-003 | tasklens-yrf.5 | Geometry 2: fill and stroke paths, stale sets, gaps, merge | P0 | E19-002 |
| E19-004 | tasklens-yrf.6 | Y scale rules and axis rounding | P0 | E19-001, E03-A1 |
| E19-005 | tasklens-yrf.7 | `AnimationDriver`, `FrameStats`, stall detection | P0 | E16-A1 |
| E19-006 | tasklens-yrf.8 | Grid quads, baseline, outline, dotted gaps | P0 | E19-002, E17-A1, E15-A1 |
| E19-007 | tasklens-yrf.9 | `ChartElement` paint plan | P0 | E19-003, E19-004, E19-005, E19-006 |
| E19-008 | tasklens-yrf.10 | `ChartView` hub binding, series lookup, derived state | P0 | E19-007, E16-A1 |
| E19-A1 | tasklens-yrf.1 | Anchor: chart element complete with tests | P0 | E19-008, E19-004, E19-005 |
| E19-009 | tasklens-yrf.11 | Chart labels as cached `ShapedLine`s | P0 | E19-A1, E15-A1 |
| E19-010 | tasklens-yrf.12 | `ChartGridElement` and `ChartGridView` | P0 | E19-009 |
| E19-A2 | tasklens-yrf.2 | Anchor: grid element complete, heading tooltips, exports | P0 | E19-010 |
| E19-011 | tasklens-yrf.13 | Headless render goldens | P1 | E19-A2, E15-A3 |
| E19-012 | tasklens-yrf.14 | Overlaid and stacked multi-series charts | P2 | E19-A1 |
| E19-013 | tasklens-yrf.15 | 5 and 10 minute ranges read the 1 Hz tier | P2 | E19-A1 |
| E19-014 | tasklens-yrf.16 | Hover readout | P2 | E19-A2, E15-A1 |
| E19-015 | tasklens-yrf.17 | Hover readout on grid cells | P2 | E19-014 |
| E19-016 | tasklens-yrf.18 | Curved lines hook | P4 | E19-A1 |

18 beads. The anchor title "E19-A2 Grid element, labels, hover" covers the P0 grid and labels;
the P2 hover readout is E19-014 and E19-015, which depend on it.

### E19 open items

1. **`MetricId` constants the pages need are not in ADR-003.** The chart draws series the UI
   files name but ADR-003's frozen list does not define: CPU power package and core, CPU maximum
   core clock, memory dirty bytes, GPU shared memory, GPU memory clock, GPU engines beyond
   `GPU_UTIL`/`GPU_ENC`/`GPU_DEC` (memory controller, JPEG, optical flow, AMD and Intel engines),
   GPU PCIe RX and TX, GPU fan, fan PWM, battery power. Recommendation: E03-A1 adds the constants
   (one number each, appended after the existing ids so nothing is renumbered) and the E12 and E13
   workers publish them. The E24 and E25 beads name the series by their UI-PRF names and say
   "constant from E03-A1"; a bead whose constant is missing when it starts stops and reports.
2. **The `TIER_1HZ` flag bit of `MetricId` has no value.** UI-PRF-001 decides that the 1 Hz tier
   is a second `SeriesView` whose `MetricId` differs by a flag bit, but no document gives the bit.
   Recommendation: bit 15 (`0x8000`) with `MetricId::with_tier_1hz()` and `is_tier_1hz()` in
   `tl-model` (E03-A1) and the reducer publishing both views from P2 (E09). E19-013 is blocked on
   this.
3. **`ceil_pow2` and `ceil_125` live in `tl-model::units`** per UI-PRF-006, but E03's scope text
   lists only the table (d) formats. Recommendation: confirm E03-A1 includes both functions
   (`ceil_125(1234) = 2000`, `ceil_125(60) = 100`, `ceil_125(0.3) = 0.5`, `ceil_pow2(3 MiB) =
   4 MiB`). If not, E19-004 defines them privately in `chart/scale.rs`; the bead says to stop and
   report rather than decide alone.
4. **Thread CPU time for `FrameStats`.** ADR-014 measures UI thread CPU per frame with
   `clock_gettime(CLOCK_THREAD_CPUTIME_ID)`, but `tl-ui`'s allowed dependencies (ADR-001) have no
   `rustix` or `libc`. Recommendation: `tl-model::time::thread_cpu_ns()` (tl-model already has
   `rustix` with the `time` feature), added by E03-A1. Until it exists E19-005 reports 0.
5. **Per-series validity signal.** `SeriesView` has no validity and `SourceHealth` is keyed by
   `SourceId`, so the chart cannot tell Failed from Restricted from Unsupported by itself.
   Resolution used in the beads: the owner page calls `ChartView::set_state` from the health of the
   source it knows; the chart derives only WarmingUp and Stale. No doc change needed; recorded so
   the E24 and E25 beads are consistent.
