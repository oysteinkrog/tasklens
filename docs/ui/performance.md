# UI spec: Performance tab and graph surfaces (UI-PRF)

This file turns `docs/spec/merged/perf.md` (PERF-001 to PERF-093 and PERF-105 to PERF-108) into build instructions for `crates/tl-ui/src/`. It covers every graph surface in tasklens: the chart element, the device sidebar, every device page, summary mode, the minigraph panel, detached graph windows, the mini window, the tray graph rasterizer, the content of the Additional Setup window, and the exact composition of the phase-0 bench scenes `perf-cpu`, `bridge` and `static`.

Not in this file: the Information tab (PERF-094 to PERF-104, system tabs spec), the title bar, menus, tab strip and toolbar (shell spec), the Processes context menu items that create minigraphs and tray graphs (processes spec), and the Settings window pages (shell spec). The Additional Setup window frame (title, pin, close) is the shell's; its rows are here.

Precedence: `docs/DECISIONS.md`, then `docs/RECONCILIATION.md`, then `docs/ARCHITECTURE.md` (ADR-009 and ADR-014 above all), then `perf.md`, then this file. Where this file states a number that none of them give, it is listed under "Decided here" at the end.

## How to read this file

- Names of colours, spacing, text styles, radii and motion are the shared names from `docs/ui/foundation.md`: `TlTheme` fields (`window_bg`, `list_bg`, `row_hover`, `row_selected`, `divider`, `menu_bg`, `text_primary`, `text_secondary`, `accent`, `value_heat`, `graph.*`), `status.success|warning|danger|info`, `sp1..sp8` (2, 4, 6, 8, 12, 16, 24, 32), `t_xs..t_xl` (11, 12, 13, 14, 16, 20), weights 400/500/600, `r_sm` 4, `r_md` 6, `m_fast` 120 ms, `m_base` 200 ms, `m_slow` 300 ms, `ease_out_cubic`. Every length is logical px at `ui_scale_pct = 100` and is multiplied by `ui_scale` once (ADR-009 "Metrics and scale").
- Component types are the foundation's: `TlButton` (kinds `Primary`, `Secondary`, `Danger`, `Ghost`, `Icon`, `Toggle { on }`, `Segment { on }`), `TlMenu`, `ToastQueue` toasts with `ToastKey`, `TlDialog`, `TlSecondaryWindow`, `TlKeyValueGrid`, `TlTooltip`, `TlSwitch`, `TlBadge`; flat tables are the process spec's `TlTreeTable` (UI-PRC-001). Where a gpui-kit widget is used directly, its kit name is given.
- "Measured" means read from `docs/taskslinger-reference/performance.png` (2720 × 1680 px; the window is treated as 1360 × 840 logical at 200%, so one logical px is 2 image px) or from the evidence frames at `/home/oystein/work/tasklens-ref/evidence-frames/` (1728 px wide, treated as 1280 logical at 135%). "Spec" means the value comes from `perf.md`, an R entry or an ADR. "Chosen" means this file picked it.
- Validity words and glyphs are PERF-013 and R-050. Number formats are canonical table (d). Settings and state keys are canonical table (a). Keys are canonical table (b).
- "Mask bits" names the `ComponentMask` bits a view notifies on. Bits are named after `Snapshot` fields: `SERIES`, `SYSTEM`, `DEVICES`, `HEALTH`, `PROCESSES`, `WINDOWS`. A chart view also checks `SnapshotChanged.changed_series` for its own `SeriesId`s (ADR-004 hub rule), so a snapshot that changes other series does not re-render it.
- Actions live in `crates/tl-ui/src/pages/performance/actions.rs` and `pages/minigraph/actions.rs`, all in the single `tasklens` namespace (shell.md rule); their names start with `Perf` or `Minigraph`. Window-level actions (`ShowMainWindow`, `ToggleMainWindow`, `ToggleMiniWindow`, `OpenAdditionalSetup`, `SetSetting`) are the shell's. Data-carrying actions use the gpui-kit `#[derive(Action)]` form. Every menu item, button and key dispatches the same action (coding-guides rule 10).

Shared sizes used by more than one entry (all chosen unless marked):

| Name | Value | Note |
|---|---|---|
| `chart_pad` | sp3 (6) | inset from the chart box to the heading, value and legend text |
| `chart_label_row` | 18 | height of the heading and value row inside a chart |
| `page_pad` | sp6 (16) | padding of a device page on all sides (measured 13 to 17 at 200%, rounded to the scale) |
| `page_header_h` | 36 | page header row height; title is vertically centred |
| `details_gap` | sp7 (24) | space between the graph area and the "Details" heading |
| `kv_row_h` | 46 | stats grid row pitch: label 18 + value 20 + sp4 gap (measured 46) |
| `kv_col_gap` | sp6 (16) | gap between the three stats columns |
| `grid_cell_gap` | sp4 (8) | gap between per-core and engine cells (measured 8) |
| `grid_cell_min_w` | 120 | PERF-028 (spec) |
| `grid_cell_h` | 112 | per-core and engine cell height (measured 123 at 200%; chosen 112 so 4 rows fit a 800 px window with the stats grid) |
| `graph_h_single` | 240 | height of a page with one main graph (network Throughput) |
| `graph_h_stacked` | 150 | height of each graph in a vertical stack (memory, disk, GPU rows 2 and 3, fan, battery) |
| `graph_h_overall` | 300 | height of the CPU "Overall" chart and of overlaid and stacked modes |
| `secondary_graph_h` | 120 | CPU secondary graph (PERF-032) |
| `card_w` | nav_width − 2 × sp4 | sidebar card width |
| `card_h` | 80 | sidebar card height (measured 78 at 200%) |
| `card_graph` | 116 × 56 | sidebar mini graph (PERF-001, spec) |
| `mini_card` | 180 × 90 | minigraph card (PERF-080, spec) |

---

## Part A: the chart element

### UI-PRF-001 ChartView and ChartElement

- **Implements**: PERF-015, PERF-016 (range), PERF-017 (tier selection), PERF-020, PERF-021 (P4 hook), PERF-022, PERF-024, PERF-026; ADR-009 chart rules 1 to 10.
- **Anatomy** (a page chart, 100% scale; W × H is the element's box):

```
┌──────────────────────────────────────────────────────── 1 px outline, line colour @ 40%, r_sm when rounded
│ Heading (t_sm 500)                     12.0%  (t_sm 400 tnum)│  ← chart_label_row 18, inset chart_pad 6
│ ── Total  ── Kernel  (legend, t_xs)        100 Mbps (t_xs 2nd)│  ← second row: legend left, axis max right (only when shown)
│ ┊    ┊    ┊    ┊    ┊    ┊    ┊    ┊    ┊    ┊     75% ─────  │  ← vertical grid every range/10, horizontal at 25/50/75
│ ┊    ┊    ┊    ┊    ┊    ┊  ╱╲  ┊    ┊    ┊    ┊     50% ─────  │
│ ┊    ┊    ┊  ╱╲ ┊   ╱╲   ╱  ╲╱╲ ┊    ┊    ┊    ┊     25% ─────  │  ← fill under the line down to the baseline
│ ┊____┊__╱╲╱__╲╱╲_╱__╲_╱_______╲╱╲__┊_╱╲_┊____┊_____________ │  ← baseline = inner bottom
│ Utilization over 60 seconds (t_xs 2nd)                       │  ← range label, inset chart_pad from left and bottom
└───────────────────────────────────────────────────────────────┘
```

  Parts:
  - **Box**: fills the bounds the parent gives. Background `window_bg` on pages and summary tiles, `list_bg` inside sidebar and minigraph cards (so the chart matches its card). Outline: 1 px quad border in the first series' `line` colour at alpha 0.40, corner radius `r_sm` when `appearance.rounded_corners` is on, else 0 (PERF-015).
  - **Plot area**: the box inset by 1 px on all sides. Data rows cover the full plot area height; labels are drawn on top of the plot (they overlay the data, as in `performance.png`).
  - **Heading**: `t_sm` 500 `text_primary`, baseline-aligned in the first `chart_label_row`, left inset `chart_pad`. Hidden when `ChartLabels.heading` is `None` (per-core cells pass "CPU N"; minigraph cards pass their label).
  - **Value**: `t_sm` 400 tnum `text_primary`, right-aligned in the same row, right inset `chart_pad`. Shows the newest sample formatted per table (d). Validity other than `Valid` shows the ADR-003 glyph (`…`, lock icon 12 px, `n/a`, `-`, `!`) in `text_secondary`; `Stale` shows the last value in `text_secondary` with tooltip "Last updated {age} ago".
  - **Legend** (optional, first chart of a page only, PERF-026): second row, left inset `chart_pad`, items 12 px apart: a 12 × 2 px line swatch in the series line colour (dashed 3/2 when the series is dashed), sp2 gap, label `t_xs` 400 `text_secondary`. In `performance.png` the legend is stacked vertically ("Total" over "Kernel"); tasklens stacks vertically when the chart is narrower than 200 px and lays out horizontally otherwise (chosen).
  - **Axis maximum** (PERF-019): `t_xs` 400 `text_secondary`, right-aligned in the second row. Shown only on large graphs (plot height ≥ 100) and only when the scale is not a fixed 0..100 percent scale: AutoRate, sticky temperature, power, clocks, commit, stacked CPU ("3200%"). Text is the rounded ceiling with its unit ("100 Mbps", "4 GiB/s", "90 °C", "300 W", "3105 MHz").
  - **Range label** (PERF-016): `t_xs` 400 `text_secondary`, bottom-left, inset `chart_pad`. Text is `"<word> over <range>"` with `<word>` from `ChartLabels.range_word` ("Utilization", "Throughput", "Memory usage", "Power draw", "Clock speed", "Temperature", "Fan speed", "Percentage", "Commit", "Swap usage", "Active time") and `<range>` one of "30 seconds", "60 seconds", "5 minutes", "10 minutes". Hidden when the plot height is under 100 or the width under 160 (sidebar and minigraph cards never show it).
  - **Grid**: hairline quads, 1 physical px, colour `graph.grid`. Vertical lines at absolute times `t = k × (range_ns / 10)`, drawn at `x(t)` so they scroll with the data (ADR-009 rule 8). Horizontal lines at 25%, 50% and 75% of the plot height. No grid when the plot height is under 40 (tray icons; sidebar cards keep it, PERF-001 "faint grid").
  - **Fill**: one closed polygon per valid run, from the line down to the baseline, colour `ChartSeries.fill` (per-type palette, PERF-024). Painted before strokes.
  - **Line**: `PathBuilder::stroke(px(1.0))` per run, colour `ChartSeries.line`. Dashed series use `dash_array(&[px(5.0), px(5.0)])` (PERF-026).
  - **Gap** (PERF-022): no fill and no line between two runs. A dotted baseline is drawn across the gap: 1 × 1 physical px quads every 3 physical px along the plot bottom, colour = line colour at alpha 0.40. The dotted baseline is quads, not a path.
  - **Stale tail**: samples flagged `STALE`, and the span from the last fresh sample to the right edge when the series is stale, draw fill and line at alpha × 0.5. Implemented as a second run set (`stale_fill`, `stale_stroke`) in the cache, painted with the colour's alpha halved.
  - **Baseline**: 1 physical px quad in the line colour at alpha 0.40 along the plot bottom, under the fill (chosen; keeps a zero series visible).

- **States**:
  - Default: as above.
  - Hover (P2, UI-PRF-004): a 1 physical px vertical marker and dots; the chart gains a hitbox only when hover is enabled for it.
  - Pressed: none. Focused: none (charts are not focusable; their values are repeated as text in the stats grid).
  - Selected: not applicable to the chart itself (cards carry selection).
  - Disabled: not applicable.
  - Loading (`WarmingUp`, no sample yet): plot empty, grid and outline drawn, value shows `…`. No skeleton, no spinner.
  - Empty (series absent from the snapshot, for example engine hidden): the parent does not create the chart.
  - Error (`Failed`): value shows `!` in `status.warning`; plot shows the dotted baseline only; tooltip "Could not read: {last_error}".
  - Restricted: value shows the lock icon; plot shows the dotted baseline; tooltip per R-050.
  - Unsupported (`n/a`): the parent normally hides the chart; when a chart must stay (CPU secondary graph fallback has its own rule, UI-PRF-025), the plot shows the dotted baseline and the value `n/a`.
  - Missing (device gone): the parent removes the chart (UI-PRF-012 hotplug animation).

- **Interaction**: none on P1. Right-click bubbles to the page (page context menu, UI-PRF-021) or the card. Double-click on a page chart area opens the mini window (P3, R-058): the page's graph container has `.on_click` checking `event.click_count == 2` and dispatches `PerfShowMiniWindow`. Hover is UI-PRF-004.

- **Data binding**: `ChartSpec.series[i].id: SeriesId` resolves to `Snapshot.series` (binary search on the sorted `Arc<[Arc<SeriesView>]>`). Tier: `range_ns ≤ 60 s` reads the fast tier, 300 s and 600 s read the 1 Hz tier (PERF-017; the engine publishes both as two `SeriesView`s whose `MetricId` differs by a `TIER_1HZ` flag bit, chosen). Re-render on `SERIES` when `changed_series` contains one of its ids, on `style_version` change, on size change (prepaint sees new bounds), and on every animation frame while moving. `ChartSpec.range_ns` comes from `performance.graph_range_s` for page and sidebar charts, from `[[minigraphs]] range_s` for minigraph cards, from `[[tray_graphs]] range_s` for tray graphs.

- **Build**: custom element. Files `chart/view.rs` (`ChartView` entity: owns `Rc<RefCell<ChartCache>>`, `ChartSpec`, `ChartLabels`, cached `ShapedLine`s for heading, value, legend, axis max and range label, each with the last text they were shaped from), `chart/element.rs` (`ChartElement: Element`, `RequestLayoutState = ()`, `PrepaintState = Option<Hitbox>`), `chart/geometry.rs` (path building, decimation), `chart/scale.rs` (UI-PRF-006), `chart/grid.rs` (grid quads and the `ChartGridElement`, UI-PRF-002).

  Paint plan per frame (`ChartElement::paint`), in order:
  1. Compute `right_edge_ns` (UI-PRF-005). Compute the `ChartGeometryKey`: series versions, physical width and height, `scale_factor` bits, y min and max bits (after UI-PRF-006), `style_version`. If it differs from `cache.key`, rebuild geometry (step 2); else skip to step 3.
  2. **Geometry build** (chart-local, `x = (t_ns − anchor_ns) × px_per_ns`, `anchor_ns = newest_sample_ns`): for each series, walk samples from newest back until `t < right_edge − range_ns − 2 × period` (two samples of overscan so the left edge never shows a bare end). Split into runs at `GAP_BEFORE` or a sample without `VALID`. Decimate (UI-PRF-007). Emit `PathBuilder::fill()` polygons for fill (down to `y = plot_height`) and `PathBuilder::stroke(px(1.0))` for lines. Series that share a colour and dash pattern within one element merge into one path (rule 7). Build the dotted-gap list as `SmallVec<[Range<f32>; 4]>` of x spans. Store `anchor_ns`.
  3. `paint_quad` background, then the grid quads: vertical lines at `x(k × range/10)` for the k values inside the plot, horizontal at 25/50/75%.
  4. `with_content_mask(plot_bounds)`: clone `cache.fill`, translate by `dx = plot_right − (right_edge_ns − anchor_ns) × px_per_ns` and `dy = plot_top`, `paint_path`. Same for `stale_fill` at half alpha, then `stroke`, then `stale_stroke`. Dotted-gap quads translated the same way.
  5. Baseline quad, outline quad (border-only `quad(..)` with the radius).
  6. Labels: paint the cached `ShapedLine`s. The value line is re-shaped only when its text changed (checked with the formatter writing into a reused `String` and comparing bytes). Heading, legend and range label change only on settings or device changes.
  7. If smooth and moving, `AnimationDriver::request(entity_id)`.

  Clone-and-translate follows gpui-component `crates/base/src/plot/path_cache.rs` (ADR-009 rule 4).

- **Performance**: per frame while moving: one `Path` clone per path (2 to 4 clones), 3 to 13 grid quads, 5 or fewer `ShapedLine::paint` calls, zero allocations beyond the path clones. Geometry rebuild only on new data: at 10 Hz that is 10 rebuilds per second per chart; at 1 Hz, one. The 32-core grid is one element (UI-PRF-002), so the `perf-cpu` scene rebuilds at most 3 paths per 100 ms tick. ADR-014 gate: p50 ≤ 1.0 ms UI thread per frame on `perf-cpu`.

- **Accessibility**: no focus ring (not focusable). Every chart's current value is also shown as text (the value label) and, on pages, in the Details grid. The hover tooltip (P2) is text. Tooltip on the value label for non-Valid states per R-050.

- **Tests**:
  - `chart_geometry_splits_runs_at_gaps`: a series with `GAP_BEFORE` at index 50 yields two subpaths and one dotted span.
  - `chart_geometry_keeps_spike_under_decimation`: 2,000 samples into 200 px with one 100% sample keeps a point at 100.
  - `chart_bucket_alignment_is_stable`: two builds 100 ms apart produce identical bucket boundaries in absolute time.
  - `chart_not_rerendered_on_unrelated_series`: render counter stays at 1 after a snapshot that changes another `SeriesId`.
  - `chart_value_label_reshapes_only_on_text_change`: shape counter increments once for two snapshots with the same formatted value.
  - `chart_stale_tail_half_alpha`: `painted_quads()` / path colours after a stale snapshot carry alpha × 0.5.
- **Phase**: P1 (hover P2, curved lines P4, 5 and 10 minute ranges P2).
- **Decided here**: `chart_pad` 6, label row 18, legend layout rule, axis max visibility rule, range label visibility rule, baseline quad, dotted-gap quad spacing 3 px, `TIER_1HZ` metric flag bit, overscan of 2 periods.

### UI-PRF-002 ChartGridElement (merged multi-chart element)

- **Implements**: PERF-028 (per-core grid), PERF-063 (engine grid), PERF-001 sidebar graphs (one element per device kind), ADR-009 rule 7.
- **Anatomy**: a rectangle holding `n` cells laid out in `cols` columns with `grid_cell_gap` between cells, each cell drawn exactly as a UI-PRF-001 chart (box, outline, grid, heading "CPU N" or engine name, value, legend in cell 0 only).

```
cols = min(ceil(sqrt(2n)), floor((W + gap) / (grid_cell_min_w + gap)))      (PERF-028)
rows = ceil(n / cols)
cell_w = (W − (cols − 1) × gap) / cols,  cell_h = grid_cell_h
element height = rows × cell_h + (rows − 1) × gap
```

  16 CPUs give 6 columns in 3 rows (matches `performance.png`); 32 give 8 columns in 4 rows.
- **States**: as UI-PRF-001 per cell. An offline CPU cell (validity `Missing` for that core, PERF-028): no plot, the cell shows "Offline" centred in `t_sm` `text_secondary`; the outline uses `divider`.
- **Interaction**: hover on a cell heading shows the `TlTooltip` "Core C, thread T, 4200 MHz" (PERF-028); the element inserts one hitbox per heading rect only when the pointer is inside the element bounds (checked in prepaint from `window.mouse_position()`), so idle frames carry no hitboxes. Right-click bubbles to the page.
- **Data binding**: `Vec<SeriesId>` for the fill series (per-core total), optional second list for overlays (kernel). Re-renders on `SERIES` when any of its ids is in `changed_series`; with 10 Hz CPU data that is every tick, which is the design.
- **Build**: `chart/grid.rs` `ChartGridElement: Element`, owned by a `ChartGridView` entity with one `ChartCache` whose `fill`, `stroke` and `overlay_stroke` paths contain every cell (each cell's subpaths are offset by the cell origin at build time; only one translate per frame for the scroll offset, applied with the content mask set to the whole element and each cell's run clipped at build time to its cell width). Headings and values are `ShapedLine`s in two `Vec`s reused across frames; the value texts re-shape only when changed (per-core percent changes 10 times a second, which is the 32-label case ADR-014 budgets for).
- **Performance**: 3 path clones per frame for 32 cells plus 32 × (grid 3 to 13 quads) ≈ 400 quads, 64 label paints. Fill path built with one `PathBuilder` per colour. Budget per ADR-014 `perf-cpu` gate.
- **Accessibility**: per-core values are text labels; the Details grid repeats the total.
- **Tests**: `grid_columns_for_16_is_6`, `grid_columns_for_32_is_8`, `grid_columns_drop_when_narrow` (W = 500 → cols = 3 with min width 120), `grid_offline_cell_draws_no_path`, `grid_one_fill_path_for_all_cells` (path count == 3 for 32 cells with kernel on).
- **Phase**: P1.
- **Decided here**: `grid_cell_h` 112, offline text, hitbox-only-when-hovered rule.

### UI-PRF-003 Chart labels and legends

- **Implements**: PERF-015 (heading and value), PERF-016 (range label), PERF-019 (axis max), PERF-026 (legends), PERF-030 (stacked axis label).
- **Anatomy**: `ChartLabels { heading: Option<SharedString>, value_fmt: ValueFormat, range_word: &'static str, legend: SmallVec<[(SharedString, LegendStyle); 2]>, show_axis_max: bool }`. `ValueFormat` is one of `Percent`, `Bytes`, `RateIec`, `RateNetwork`, `Mhz`, `Celsius`, `Watts`, `Rpm`, `Count`, mapped to `tl-model::units`. Legend pairs per page, exact strings and styles:

| Page / chart | Legend | Style |
|---|---|---|
| CPU overall and per-core cell 0 | "Total", "Kernel" | solid cpu_line; solid `#C1514E` |
| Network Throughput | "Download", "Upload" | solid filled net_line; dashed, lighter tint (net_line mixed 30% toward white) |
| Disk Transfer rate | "Read", "Write" | solid filled disk_line; dashed disk_line |
| GPU Memory | "Dedicated", "Shared" | solid filled gpu_line; dashed gpu_line |
| GPU Video engines (when drawn as one chart) | "Encode", "Decode" | solid; dashed |
| GPU Clocks | "GPU clock", "Memory clock" | solid; dashed |
| Fan speed | "RPM", "PWM" | solid filled; dashed |
| Memory Physical memory | "Used", "Committed" | solid filled mem_line; dashed mem_line (the dirty series is a thin 1 px line, no legend, mem_line at alpha 0.6) |

  The legend hides when the chart is a sidebar card, a minigraph card (the CPU minigraph shows "Total"/"Kernel" in its hover tooltip instead, PERF-080), a summary tile narrower than 200 px, or a tray icon.
- **States**: text follows the chart's validity rules (UI-PRF-001).
- **Interaction**: none.
- **Data binding**: labels change on settings (`graph_range_s`, `units.network`), on device rename, and on value change.
- **Build**: part of `ChartView` (UI-PRF-001). Shaped with `window.text_system().shape_line(..)` into cached `ShapedLine`s; font `"Noto Sans"`, `tnum` on the value line.
- **Performance**: see UI-PRF-001.
- **Accessibility**: these labels are the chart's text alternative.
- **Tests**: `legend_strings_match_spec` (table above), `range_label_text` ("Utilization over 60 seconds", "Throughput over 5 minutes").
- **Phase**: P1.
- **Decided here**: dashed upload tint rule, dirty-line alpha, legend hiding rule.

### UI-PRF-004 Hover readout

- **Implements**: PERF-023.
- **Anatomy**:

```
 plot ─────────────┼───────────        vertical marker: 1 physical px, text_secondary @ 0.8
                   ● ← dot r=3 px, series line colour, 1 px window_bg ring
          ┌────────────────────────┐
          │ -12 s                  │  t_xs 500 text_primary
          │ ── Total      23.4%    │  t_xs 400, swatch 12×2, value tnum right-aligned
          │ ── Kernel      3.1%    │
          │ avg 18.2%  peak 61.0%  │  t_xs 400 text_secondary
          │ 600 samples            │
          └────────────────────────┘  bg menu_bg, 1 px divider, r_sm, padding sp3, min width 140
```

  Time offset text: `-12 s` (seconds, no decimals) for ranges up to 60 s; `-4 m 12 s` for 5 and 10 minutes. "avg", "peak" and "samples" are computed over the visible window of the first series; with two series the avg and peak line shows the first series only (TaskSlinger shows one). The readout box is placed 12 px right of the marker, flipped to the left when it would cross the plot's right edge, and clamped inside the chart box vertically.
- **States**: visible only while the pointer is inside the plot and the chart has at least one valid sample in view; hidden on mouse leave, on any mouse button down, and when the window loses focus.
- **Interaction**: pointer move inside the plot updates the marker to the nearest sample in time (`t = right_edge_ns − (plot_right − x) × range_ns / width`, then nearest index by binary search). No click behaviour.
- **Data binding**: the chart's own series; the marker follows the data as it scrolls, so each animation frame recomputes `x(t)` for the pinned `t` (the hovered sample time stays fixed, the marker moves left with the data until the pointer moves again).
- **Build**: `chart/element.rs`: `insert_hitbox(plot_bounds, HitboxBehavior::Normal)` only when `ChartSpec.hover` is true (ADR-009 rule 1); `on_mouse_move` recorded through `window.on_mouse_event` inside paint. The readout is painted inside the same element with `paint_layer`: one quad for the box and up to five `ShapedLine`s re-shaped only when the hovered sample index changes. No kit `Tooltip` (its 500 ms delay and single-string body do not fit).
- **Performance**: per frame while hovered: one binary search, up to five shape calls when the index changed, otherwise paints only. No work when not hovered.
- **Accessibility**: the readout is text. Keyboard users get the same numbers from the stats grid (avg and peak are not available there; accepted).
- **Tests**: `hover_picks_nearest_sample`, `hover_box_flips_at_right_edge`, `hover_hidden_without_valid_samples`.
- **Phase**: P2.
- **Decided here**: box geometry, offset text format, first-series rule for avg and peak, marker pinned in data time.

### UI-PRF-005 Smooth scrolling, the animation driver and time mapping

- **Implements**: PERF-020; ADR-009 "Animation driver"; ADR-010 zed#64499 detection.
- **Behaviour**: when `motion.smooth_graphs` is on, `App::reduce_motion()` is off and the window is visible, every chart whose newest sample is younger than `2 × period + range_ns / width_px` (it is "moving") requests the next frame from the window's `AnimationDriver`. The driver arms one `window.on_next_frame` and notifies every requester in that callback, so one compositor frame drives every chart in the window. When nothing requests, no frames are drawn.
  - Time mapping (ADR-009 rule 5): `right_edge_ns = min(now_boot_ns() − period_ns, newest_sample_ns)` when sliding, else `newest_sample_ns`. The chart therefore draws one sample period behind the newest data, so the newest segment is always complete and never "grows" at the right edge. Numeric readouts always show the newest sample.
  - With smooth graphs off, the chart redraws once per new sample and the data jumps one step.
  - Reduced motion: identical to smooth graphs off. The setting is still shown as on in Settings; a note under the switch reads "Turned off by the system reduce-motion setting" (shell spec owns the settings page; this spec owns the behaviour).
- **Window visibility**: the driver stops arming when the hub marks the window hidden (ADR-010 hidden state) and resumes on the next render.
- **Stall detection**: the driver records the time of each frame callback; if a frame was requested and no callback arrives for 250 ms while visible, it logs `frame stall` and increments `FrameStats.stalls` (ADR-010). No UI is shown for a stall.
- **Build**: `anim.rs` `AnimationDriver` (Entity, one per window): `requests: FxHashSet<EntityId>`, `armed: bool`, `last_frame: Option<Instant>`, `stats: FrameStats`. `ChartView::render` calls `driver.request(cx.entity_id())` under the conditions above. Detached graph windows and the mini window each get their own driver.
- **Performance**: one `on_next_frame` per window per frame; the set is cleared and refilled each frame (no allocation after warm-up: `FxHashSet` keeps its capacity).
- **Tests**: `driver_requests_no_frame_when_static` (ADR-009 verification), `driver_one_frame_for_many_charts` (32 charts → one `on_next_frame` per frame), `right_edge_is_one_period_behind`.
- **Phase**: P1.
- **Decided here**: the reduce-motion note text.

### UI-PRF-006 Y scale and axis rounding

- **Implements**: PERF-018, PERF-019, PERF-030 (stacked max), ADR-009 rule 9.
- **Behaviour** (`chart/scale.rs`, `YScale` plus a `ScaleState` kept per `ChartView`):

| Kind | Rule | Axis max text |
|---|---|---|
| `Fixed { min: 0, max: 100 }` | percentages | none |
| `FixedTotal(total)` | memory, VRAM, dedicated and shared GPU memory | none |
| `Commit(limit)` | `max(limit, peak in view)`; label says "may exceed 100%" through the heading tooltip | `"{percent of limit}%"` when above 100% |
| `AutoRate { iec: bool }` | ceiling above the visible max: power of two for IEC (disk, network in bytes), 1-2-5 × 10ⁿ for SI (network in bits); rises at once; falls only after the visible max has stayed below half the ceiling for 10 s (`ScaleState.below_half_since`) | the ceiling with unit |
| `LinkSpeed(bps)` | network with `performance.network_dynamic_scale = false` and a known link speed; else `AutoRate` | link speed ("1 Gbps") |
| `StickyTemp` | starts 35..45 °C; max rises to the ceiling of the visible max rounded up to the next 5 °C; falls after 10 s below half the span; min stays the floor of visible min rounded down to 5 °C, never above 35 | `"{max} °C"` |
| `StickyUp { hw_max }` | power, clocks, fan RPM: 0 to `hw_max` when known (U-35), else sticky up from 0 with 1-2-5 rounding | `"{max} W"`, `"{max} MHz"`, `"{max} RPM"` |
| `Stacked(n)` | 0 to 100 × n | `"{100 n}%"` |
| `Symmetric` | battery power: `−m..m`, `m` = 1-2-5 ceiling of visible |value| max; zero line drawn at mid height | `"±{m} W"` |

  Rounding functions live in `tl-model::units`: `ceil_pow2(f64) -> f64`, `ceil_125(f64) -> f64`. The scale is computed from the visible samples once per geometry rebuild, not per frame; the 10 s fall timer is checked on each rebuild (new data arrives at least every second while visible).
- **Tests**: `autorate_rises_immediately`, `autorate_falls_after_10s_below_half`, `ceil_125` (1234 → 2000, 60 → 100, 0.3 → 0.5), `ceil_pow2` (3 MiB/s → 4 MiB/s), `sticky_temp_initial_range`, `link_speed_fallback_when_unknown`.
- **Phase**: P1.
- **Decided here**: temperature rounding to 5 °C steps and its fall rule, symmetric battery label, Commit axis text.

### UI-PRF-007 Decimation, gaps and validity on charts

- **Implements**: PERF-017, PERF-022, ADR-009 rules 2 and 3, R-051.
- **Behaviour**:
  - `bucket_ns = range_ns / physical_width`; `bucket = t_ns / bucket_ns` (absolute time, so buckets do not shimmer).
  - A run with more than one sample per bucket keeps, per bucket, the min and max in time order plus the run's first and last sample. Buckets with one sample keep it. This gives at most `2 × width + 2` points per run.
  - 10 Hz data over 60 s is 600 samples; a 116 px sidebar graph at 110% has 128 physical px, so the sidebar decimates about 5:1 and page charts (≥ 600 physical px) do not decimate at all.
  - Gap: a sample without `VALID`, or with `GAP_BEFORE`, ends the current run. Consecutive invalid samples form one gap. The dotted baseline spans from the last valid sample's x to the next valid sample's x.
  - Stale: the series is stale when `now_boot_ns − newest.t_ns > max(3 × period_ns, 1.5 s)` (R-051). The tail from the last fresh sample to the right edge is drawn as the stale set at half alpha with a flat continuation at the last value, so the user sees the line "hang" rather than vanish; the readout shows the last value in `text_secondary` with tooltip "Last updated {age} ago" (short age form).
  - After suspend and resume the engine sets `GAP_BEFORE`; the chart shows the gap with the dotted baseline (no special marker).
- **Tests**: `decimation_point_count_bound`, `gap_dotted_span_coordinates`, `stale_flat_continuation_present`.
- **Phase**: P1.
- **Decided here**: flat continuation for the stale tail.

### UI-PRF-008 Graph palette and per-card colour

- **Implements**: PERF-024, APP-039 (values), ADR-009 `GraphPalette`.
- **Behaviour**: `TlTheme.graph` fields and the Default Dark values:

| Field | Default Dark (UI-FND-001) | Used by |
|---|---|---|
| `cpu_fill` | `#0D546D` | CPU total fill, per-core fills, CPU minigraph, CPU sidebar card |
| `cpu_line` | `#0A7096` | CPU lines |
| `kernel_line` | `#C1514E` | kernel overlay (no fill) |
| `mem_fill` | `#7E6A9A` | memory fill |
| `mem_line` | `#A58CC4` | memory line, committed dashed, dirty thin |
| `gpu_fill` | `#508A59` | GPU fills, VRAM |
| `gpu_line` | `#71B768` | GPU lines |
| `disk_fill` | `#8A7340` | disk |
| `disk_line` | `#D9B567` | disk line, write dashed |
| `net_fill` | `#1E5C66` | download fill |
| `net_line` | `#359CAB` | download line, upload dashed (tinted) |
| `grid` | `#2A3033` | grid hairlines |
| `chart_bg` | `#1D2223` | plot background of every chart and mini graph |

  The values are UI-FND-001's (foundation wins; fills are pre-darkened opaque colours, so no
  per-frame alpha blending). Default Light values are in the same table there.

  Fans use the "Violet" accent (`#8B5CF6`) and batteries "Mint" (`#3EB489`) as line, with the fill at alpha 0.85 of the same hue (PERF-024). Where the spec gives one colour for a type, fill = line at alpha 0.85 (chosen; the CPU pair is the only two-value pair in the spec). Two devices of the same type share a colour.
  Per-card colour (P3): `color = "accent"` uses `TlTheme.accent`; a named colour uses the APP-039 table; the fill is the chosen colour at alpha 0.85, the line the colour itself. Themes override every field through `TlTheme.graph`.
- **Build**: `theme/palette.rs` with `GraphPalette` and `fn accent_color(name: &str) -> Option<Hsla>` over the 17 names in lower case.
- **Tests**: `accent_names_round_trip` (17 names), `palette_default_dark_values`.
- **Phase**: P1 palette; P3 per-card colour.
- **Decided here**: fill alpha 0.85 for single-colour types, grid alpha 0.12.

### UI-PRF-009 Curved lines (P4 hook)

- **Implements**: PERF-021.
- **Behaviour**: with `performance.curved_lines = true`, `geometry.rs` replaces straight segments with a monotone cubic (Fritsch-Carlson) through the decimated points of each run, emitted with `PathBuilder::cubic_bezier_to`, 4 subdivisions per segment (chosen). Fill polygons follow the same curve. The geometry key's `style_version` includes this setting, so toggling it rebuilds once.
- **Tests**: `monotone_curve_never_overshoots` (max of the curve ≤ max of the points).
- **Phase**: P4.
- **Decided here**: 4 subdivisions.

---

## Part B: the Performance tab frame, sidebar and summary mode

### UI-PRF-010 Performance tab layout

- **Implements**: PERF-006, PERF-007 (sidebar toggle), PERF-012, PERF-014, R-090.
- **Anatomy** (window content below the tab strip; sizes at 100%):

```
┌─ sidebar (nav_width, default 252) ─┬┬─ page (flex 1, min 400) ───────────────────────────────┐
│ sp4 pad                            ││ page_pad 16                                             │
│ ┌ card ───────────────────┐        ││ ┌ header row 36 ────────────────────────────────────┐   │
│ │ 116×56 graph │ heading  │ 80 h   ││ │ Processor  AMD Ryzen 7 7800X3D   [Per core ▾][Kernel on] │
│ │              │ line 1   │        ││ └───────────────────────────────────────────────────┘   │
│ │              │ line 2   │        ││ sp4                                                     │
│ └─────────────────────────┘        ││ ┌ graph area ───────────────────────────────────────┐   │
│ sp4 gap                            ││ │ (per page, Part C)                                │   │
│ ┌ card ───────────────────┐        ││ └───────────────────────────────────────────────────┘   │
│ │ ...                     │        ││ details_gap 24                                          │
│ └─────────────────────────┘        ││ Details (t_md 500)                                      │
│ ...                                ││ ┌ TlKeyValueGrid, 3 columns, kv_row_h 46 ──────────┐   │
│                                    ││ │ Utilization   Kernel time   Speed                 │   │
│                                    ││ │ 18.6%         2.3%          4631 MHz              │   │
│                                    ││ └───────────────────────────────────────────────────┘   │
│ [vertical scrollbar, 16 px kit]    ││ (page-specific blocks: tables, modules)                 │
└────────────────────────────────────┴┴─────────────────────────────────────────────────────────┘
                                      ↑ splitter: 1 px divider line, 8 px hit area (4 each side)
```

  Parts:
  - **Sidebar**: background `window_bg`; width `state.performance.nav_width` (180..=400, default 252, PERF-006); padding `sp4` on all sides; cards stacked with `sp4` gaps; vertical scroll with the kit scrollbar in `ScrollbarMode::Scrolling` (UI-FND-003: every non-table scroll region). A pencil `TlButton` (ghost, icon `pencil`, 24 px square) sits at the top right of the sidebar from P2 (edit mode, UI-PRF-013); in P1 the sidebar has no header row and cards start at `sp4`.
  - **Splitter**: gpui-kit `h_resizable("perf-split").with_state(&state)` with two panels: the sidebar `resizable_panel().size(px(nav_width)).size_range(px(180.)..px(400.)).flex_none()` and the page. The handle is the kit's 1 px line (`divider`) with 4 px hit padding each side and the `col-resize` cursor during the drag. `on_resize` (fires on mouse-up) writes `nav_width` to `state.toml` through the settings entity (500 ms debounce, ADR-011). On start the saved width is clamped to 180..=400.
  - **Page**: `window_bg`; vertical scroll (kit scrollbar, `Scrolling` mode); padding `page_pad`; children: header row, graph area, Details heading, `TlKeyValueGrid`, page blocks.
  - **Sidebar hidden** (F9, Ctrl+T): the sidebar panel gets `.visible(false)`; the page fills the width. Not persisted (R-090). Animation: width from 252 to 0 over `m_base` with `ease_out_cubic`, page reflows each frame; reduced motion: instant.
  - **Toolbar** (shell owns the toolbar; stated here for the layout contract, PERF-014): on the Performance tab the title bar shows only the shield and the "+" picker. The page header carries the page toggles.
- **States**: default; sidebar focused (UI-PRF-012); sidebar hidden; summary mode (UI-PRF-016) replaces the whole layout.
- **Interaction**: F9 and Ctrl+T dispatch `PerfToggleSidebar`. Ctrl+M `PerfToggleSummaryMode`. Ctrl+C `PerfCopyDeviceInfo` (UI-PRF-022). F1..F8 `PerfSelectVisibleDevice { index: u8 }`. Up and Down when the sidebar has focus `PerfSelectPrevDevice` / `PerfSelectNextDevice`. Focus order inside the tab: sidebar list (one tab stop, roving selection) → page header toggles → page blocks with controls (Display unit, Fetch, Show SMART data, Eject). Tab out of the last control returns to the shell's tab strip.
- **Data binding**: `state.performance.nav_width`, `state.performance.selected_device`. The `PerformancePage` entity (`pages/performance/mod.rs`) owns: `sidebar: Entity<DeviceSidebar>`, `pages: FxHashMap<DeviceKey, AnyView>` (device pages created on first visit and kept, ADR-009), `selected: DeviceKey`, `summary: Option<Entity<SummaryView>>`, `sidebar_visible: bool`. It re-renders on `DEVICES` (device list changed) and on selection; the active page renders itself.
- **Build**: `pages/performance/mod.rs`. Each of sidebar, header and the Details block is embedded with `.cached(StyleRefinement::default().size_full())` or its fixed size (ADR-009). The graph area is not cached as a whole (charts cache themselves); the per-page graph container is a plain `div`.
- **Performance**: a selection change swaps one `AnyView` in a `match`; no page is rebuilt. The resize handle re-lays out only on drag frames.
- **Accessibility**: the sidebar has one focus handle with `key_context("Performance")`; the selected card draws the focus ring (UI-PRF-011). Every toggle is a `TlButton` with a tooltip.
- **Tests**: `nav_width_persists_and_clamps` (save 500 → load 400), `toggle_sidebar_not_persisted` (state file unchanged after F9), `f3_selects_third_visible_card_skipping_hidden`.
- **Phase**: P1.
- **Decided here**: sidebar padding `sp4`, scrollbar mode Hover, sidebar hide animation, focus order, page minimum width 400.

### UI-PRF-011 Device sidebar card

- **Implements**: PERF-001, PERF-005 (gap after re-plug), PERF-008 (headings), PERF-013 (glyphs), R-055, PERF-053 (disk temperature on line 2), PERF-054 (network state).
- **Anatomy** (card_w × 80; measured proportions from `performance.png`: graph 116 × 56 at left with sp5 (12) inset, text column starts 8 px after the graph):

```
┌──────────────────────────────────────────────────────┐ bg list_bg, 1 px divider, r_md
│ sp5 ┌────────────────┐ sp4  CPU                  t_md 500 text_primary   │
│     │ 12.0%  (value) │      AMD Ryzen 7 7800X3…  t_sm 400 text_secondary│  single line, ellipsis
│     │  ▁▂▃▅▃▂▁▂▃     │      4631 MHz · 52 °C     t_sm 400 text_secondary│
│     └────────────────┘ 116×56, chart with grid, no heading, value at top-left                │
└──────────────────────────────────────────────────────┘
```

  Parts:
  - **Mini graph**: a UI-PRF-001 chart in a 116 × 56 box, `ChartLabels { heading: None, value shown at top-left, no legend, no range label, no axis max }`. The value label sits top-left inside the graph (`performance.png` shows "12.0%" over the graph's top-left), `t_sm` 500 `text_primary`. Grid on. Outline per UI-PRF-001.
  - **Text column**: three lines, left-aligned, vertically centred in the card: heading `t_md` 500 `text_primary` (18 px line); line 1 and line 2 `t_sm` 400 `text_secondary` (16 px lines); each one line with `text_ellipsis` and `overflow_hidden`. Network line 2 uses the `accent` colour (R-055) because TaskSlinger draws the rate in the accent.
  - Headings and lines per device (PERF-001, R-055):

| Kind | Heading | Line 1 | Line 2 | Graph series | Value |
|---|---|---|---|---|---|
| CPU | "CPU" | model, or "Unknown processor" | "4631 MHz · 52 °C" (temperature omitted when no CPU hwmon) | total CPU, kernel overlay when `cpu_kernel` | `11.9%` |
| Memory | "Memory" | "34.68 GiB / 63.16 GiB" | "Commit 69.0% of limit" | used (filled), committed dashed | `55.0%` (used/total) |
| GPU | "GPU N" | name, or "Unknown GPU" | "52 °C" | 3D/Compute engine (or the max engine, PERF-009 rule) | `10.9%` |
| Disk | "Disk N" | "NVMe - /" (kind, " - ", primary mount; no mount: kind only) | model; from P2 "<model> · 41 °C" | active time | `0.1%` |
| Network | `<ifname>` | type ("Ethernet", "Wi-Fi", "Tunnel", …) | `↓ 1.2 Mbps ↑ 0.3 Mbps` when both non-zero, else total rate, in `accent`; disconnected: the state word ("Disconnected") in `text_secondary` | download filled, upload dashed | total rate, e.g. `0.01 Mbps` |
| Fan | "Fan N" or label | label (or chip) | "1200 RPM · 45 °C" | RPM | `1200 RPM` |
| Battery | kind ("Battery", "Mouse", "UPS") | vendor and model | "85.0% · Discharging" | percentage (peripheral) or power (system battery) | `85.0%` |
| Aggregate (P4) | "Aggregate storage" / "Aggregate graphics" | "3 disks" / "2 adapters" | "Multiple" when kinds differ, else the kind | busiest disk active / highest engine | percent |

  - Numbering: "Disk N", "GPU N", "Fan N" count from 0 in sidebar order (`performance.png` shows "GPU 0", "Disk 0"), re-numbered when the order changes.
  - Disconnected interface: the whole card at opacity 0.6 (PERF-001).
  - Tooltip (`TlTooltip`, 500 ms): the full model on line 1 when it is truncated; network cards add the NetworkManager connection name on its own line when known. The Memory card says "Memory the system has promised to programs, as a share of the commit limit. It can pass 100% because Linux allows overcommit."
- **States**:
  - Default: bg `list_bg`, border 1 px `divider`.
  - Hover: bg `row_hover`.
  - Pressed: bg `row_selected` at alpha 0.6 for the press duration.
  - Selected: border 2 px `accent` (measured 2 px at 200% as a 4 px line), drawn as an outline quad inside the card bounds so the card does not grow; bg `list_bg`. Text colours unchanged (TaskSlinger keeps them).
  - Focused (sidebar has keyboard focus): the selected card also gets the focus ring: a second outline 1 px `accent` at alpha 0.5, 2 px outside the border. When the sidebar is not focused, only the 2 px border shows.
  - Disabled: not used (hidden cards are not rendered outside edit mode).
  - Loading (`WarmingUp`): value `…`, lines show their static text; rate lines show `…`.
  - Error / Restricted / Unsupported on a line value: the ADR-003 glyph in place of that value only ("4631 MHz · n/a").
  - Missing: card removed (UI-PRF-012).
  - Edit mode (UI-PRF-013): the card gains a drag handle and a switch and loses the selected border.
- **Interaction**: click → `PerfSelectDevice { key }`. Right-click → UI-PRF-014 menu. Double-click: none (keeps the single click fast; the mini window opens from the page graph area). Drag: edit mode only.
- **Data binding**: newest sample of the card's series from `Snapshot.series`; static text from `Snapshot.devices` (`DeviceCatalog`) and `Snapshot.system` (CPU model, clock, temperature). Card keys are the PERF-005 persistent keys (`DeviceEntry.persistent_key`); where the identity has no by-path or device path the fallback forms are `disk:<name>` and `fan:<chip>:<index>` (R-229). Re-render on `SERIES` for its own ids, on `SYSTEM` (CPU clock and temperature lines; memory totals) and `DEVICES`.
- **Build**: `pages/performance/sidebar.rs`: `DeviceCard` is a `RenderOnce` struct built by `DeviceSidebar::render` from `DeviceCatalog`; its chart is a `ChartView` entity kept in `DeviceSidebar.cards: IndexMap<DeviceKey, CardState { chart: Entity<ChartView>, lines: [SharedString; 3] }>`, created when a device appears. All card graphs of one device kind could share a `ChartGridElement` (ADR-009 rule 7); since the cards are separate rows, each card keeps its own `ChartView` but the sidebar is a single `.cached()` entity that re-renders on data change. The line strings re-format only when their value bits change (same rule as `CellTextCache`).
- **Performance**: 9 cards at 1 Hz (CPU card at 10 Hz when `cpu_fast`): 9 path clone sets per frame while sliding; text re-shape only when the formatted text changes.
- **Accessibility**: the card's text lines are the graph's alternative. Selection is keyboard-reachable (Up, Down, F1..F8).
- **Tests**: `card_lines_cpu`, `card_lines_network_both_directions` ("↓ 1.2 Mbps ↑ 0.3 Mbps"), `card_lines_network_one_direction` (total only), `card_disconnected_opacity`, `card_value_glyph_for_restricted`.
- **Phase**: P1 (fan and battery cards P2 with their pages; aggregate P4).
- **Decided here**: `card_h` 80, inset `sp5`, text line heights, selected border 2 px inside, focus ring form, pressed tint, double-click does nothing on cards, numbering from 0.

### UI-PRF-012 Sidebar list: selection, keys, hotplug and visibility

- **Implements**: PERF-002 (hide rule side effects), PERF-004 (type defaults), PERF-005, PERF-007 (F1..F8, Up/Down), PERF-008 (order), R-052 (5 s rescan feeds this).
- **Behaviour**:
  - **Order**: `performance.device_order` (device keys) first, in that order; devices not listed follow in type order CPU, Memory, GPU, Disk, Network, Fans, Batteries, then aggregates; within a type, by `DeviceCatalog` order (PCI address, kernel name, ifindex).
  - **Visibility**: a device is visible when `device_overrides[key]` is `"show"`, or when there is no override and its type default (`show_disks`, `show_gpus`, `show_fans`, `show_batteries`, `show_network_*` by PERF-054 type class) is on. CPU and Memory are visible unless overridden `"hide"`. `lo` never appears. Aggregates (P4) default hidden.
  - **Selection**: `state.performance.selected_device`. If the saved key is missing or hidden at start, select the first visible card. When the selected device vanishes or is hidden, select the next visible card below it, else the one above (PERF-002, PERF-005).
  - **Keys** (context `Performance`): F1..F8 select the Nth visible card; Up and Down move selection when the sidebar has focus; Home and End select first and last (chosen, matching Table conventions); Enter and Space do nothing extra (selection already switched the page). Selecting scrolls the card into view (`ScrollHandle::scroll_to_item` Nearest).
  - **Hotplug add**: a new card appears at its place with an insert animation: height 0 → 80 and opacity 0 → 1 over `m_base`, `ease_out_cubic`; the cards below slide down with the same curve. Reduced motion or `motion.animations = false`: instant.
  - **Hotplug remove**: the card fades to opacity 0 and its height collapses 80 → 0 over `m_base`; cards below slide up. Reduced motion: instant. The device's page entity is dropped once the animation ends.
  - **Re-plug**: a device that returns with a new generation is a new card at its saved order position; its series start with `GAP_BEFORE`, so the mini graph shows the gap (PERF-005).
  - **Empty sidebar**: cannot happen (at least one page visible, PERF-002).
- **Data binding**: `Snapshot.devices` (`DEVICES` bit), settings `device_order`, `device_overrides`, `show_*`. Demand: the sidebar adds `memory`, `disks`, `nets`, `gpus`, `sensors` to `DemandAggregator` for each visible card type, and `cpu: Total`.
- **Build**: `pages/performance/sidebar.rs` `DeviceSidebar` entity. The list is a `v_flex` of cards (9 to 15 items; `uniform_list` is not needed). Animations use the GPUI `Animation` on the card wrapper's height and opacity, driven by `with_animation` (respects `reduce_motion`), not by the chart driver.
- **Tests**: `order_saved_then_type_default`, `hidden_devices_not_counted_by_f_keys`, `selected_device_removed_selects_next`, `device_appears_in_saved_slot`.
- **Phase**: P1 (hotplug for disks and network); P2 for GPU, fans, batteries.
- **Decided here**: Home and End, animation forms, next-below-else-above rule when the selected card goes.

### UI-PRF-013 Sidebar edit mode

- **Implements**: PERF-003, PERF-008 (group names, empty states), PERF-002 (at-least-one rule).
- **Anatomy**:

```
┌ sidebar ─────────────────────────────────────┐
│ Select pages            [Enable All] [Disable All]│  header: t_md 500; two TlButton small ghost
│ [Reset to Default]                      [Done]│  second row; Done is TlButton small primary
│ ────────────────────────────────────────────  │  1 px divider
│ PROCESSOR                 t_xs 600 text_secondary, letter-spacing 0.5, sp4 top          │
│ ┌ ⋮⋮ ┌graph┐ CPU                  [●  ]  ┐   │  drag handle (grip-vertical icon 16 px) + card + TlSwitch
│ └─────────────────────────────────────────┘   │
│ MEMORY                                        │
│ ┌ ⋮⋮ ┌graph┐ Memory               [●  ]  ┐   │
│ DISKS                                         │
│ ┌ ⋮⋮ ┌graph┐ Disk 0               [●  ]  ┐   │
│ ┌ ⋮⋮ ┌graph┐ Disk 1               [  ○]  ┐   │  switch off = hidden card, card at opacity 0.6
│ GRAPHICS ADAPTERS                             │
│   No graphics adapter found   (t_sm text_secondary, sp4 indent)                          │
│ NETWORK ADAPTERS ...  FANS ...  BATTERIES ... │
└───────────────────────────────────────────────┘
```

  - Group names (PERF-008): Processor, Memory, Disks, Graphics adapters, Network adapters, Fans, Batteries, and from P4 "Aggregate" for the two aggregate cards. Drawn in upper case `t_xs` 600 `text_secondary`.
  - Empty-state texts (PERF-008): "No graphics adapter found", "No network adapter found"; for Disks, Fans and Batteries: "No disk found", "No fan found", "No battery found" (chosen, same pattern).
  - Each card row: 24 px drag handle column (icon `grip-vertical`, `text_secondary`, cursor `grab`) + the UI-PRF-011 card (no selected border) + a `TlSwitch` (kit `Switch` small, 28 × 16) aligned right, vertically centred. Hidden cards appear with the switch off and the card at opacity 0.6.
  - Drop indicator: a 3 px `accent` line (PERF-003) across the sidebar width at the drop position between two cards; drops are limited to the dragged card's own group (chosen: a disk cannot move into the Fans group, because numbering and type defaults are per group).
- **States**: entering edit mode: the header appears (height 0 → 64 over `m_base`) and handles and switches fade in over `m_fast`; leaving reverses. Reduced motion: instant. While a drag is in progress the dragged card follows the pointer as a semi-transparent (0.8) copy and the original row shows at opacity 0.3.
- **Interaction**:
  - Pencil button or menu "Select Pages" → `PerfEnterEditMode`. "Done" or Esc → `PerfExitEditMode` (Esc is handled before the global Esc rule while edit mode is on).
  - Switch → `PerfSetPageVisible { key, visible }`. Turning off the last visible page is refused: the switch snaps back and a `ToastQueue` toast shows "At least one performance page must stay visible" (PERF-002).
  - "Enable All" → `PerfEnableAllPages`; "Disable All" → `PerfDisableAllPages` (keeps the current selected page visible and shows the same toast once); "Reset to Default" → `PerfResetPagesToDefault` (clears `device_order` and `device_overrides`).
  - Drag with the handle: GPUI `on_drag` with a `DraggedCard { key }` payload; `drag_over` on card rows computes the drop index from pointer y against the row midpoint; drop → `PerfMoveDevice { key, before: Option<DeviceKey> }`.
  - Keyboard reorder: with a card row focused, Alt+Up and Alt+Down move it one slot (chosen; keeps the feature keyboard-reachable).
  - Clicking a card in edit mode selects its page as usual.
- **Data binding**: writes `performance.device_order` and `performance.device_overrides` (`"show"` when the type default is off, entry removed when the default is on, `"hide"` to hide; PERF-002).
- **Build**: part of `DeviceSidebar`, `edit_mode: bool`, `drag: Option<DragState>`. Group headers and empty states are plain `div`s. Switch is the kit `Switch`.
- **Tests**: `edit_mode_groups_and_empty_states`, `last_visible_refused_with_toast`, `reorder_within_group_only`, `reset_clears_order_and_overrides`, `show_override_written_only_when_default_off`.
- **Phase**: P2.
- **Decided here**: header layout and button variants, group header style, missing empty-state texts, same-group drop rule, Alt+Up/Down, drag ghost opacities.

### UI-PRF-014 Card context menu

- **Implements**: PERF-002, PERF-007 ("View >" is on the page menu, not here).
- **Anatomy**: `TlMenu` (kit `PopupMenu`) at the pointer, items in order:
  1. "Select Pages" → `PerfEnterEditMode`
  2. "Hide This Page" → `PerfSetPageVisible { key, visible: false }`
  3. "Show This Page" → `PerfSetPageVisible { key, visible: true }` (present only in edit mode and only on hidden cards)
  4. separator
  5. "Add tray icon graph" → `PerfAddTrayGraph { source: key }` (P3; disabled with tooltip "No system tray available" when no `StatusNotifierWatcher` is on the bus, PERF-088)
- **States**: "Hide This Page" is disabled when the card is the only visible one (tooltip "At least one performance page must stay visible").
- **Interaction**: right-click selects nothing (the card keeps its state) and opens the menu; Shift+F10 on the focused card opens it too. Hiding the selected page selects the next visible card (UI-PRF-012).
- **Build**: `.context_menu(..)` on the card row.
- **Tests**: `card_menu_items_default`, `card_menu_show_only_in_edit_mode_hidden`.
- **Phase**: P1 (item 5 P3).
- **Decided here**: separator and item 5 placement, disabled-instead-of-toast for the last page in the menu (the toast still shows when the switch is used).

### UI-PRF-015 Page jump "View >" submenu and page context menu

- **Implements**: PERF-007 (View submenu), PERF-010 (Graph Summary View item), PERF-011 (Copy), PERF-016 (Time range), PERF-088 (Add tray icon graph), plus page-specific items defined in Part C.
- **Anatomy**: `TlMenu` opened by right-click anywhere on a device page (header, graph area, Details). Common items, in order; page-specific items are inserted where Part C says:
  1. "Copy" → `PerfCopyDeviceInfo` (Ctrl+C hint)
  2. "Graph Summary View" (check item, checked while on) → `PerfToggleSummaryMode` (Ctrl+M hint)
  3. "Time range >" → radio items "30 seconds", "1 minute", "5 minutes", "10 minutes" → `PerfSetGraphRange { secs }` (300 and 600 disabled before P2)
  4. separator
  5. page-specific items (CPU: graph mode and kernel; Memory: show composition, show swap; GPU: select engines, hide this engine; Fan: rename, sensor; Network: Network settings)
  6. separator
  7. "View >" → "Processor", "Memory", "Disks", "Graphics adapters", "Network adapters", "Fans", "Batteries" → `PerfJumpToType { kind }`; a type with no visible device is disabled (PERF-007)
  8. "Add tray icon graph" → `PerfAddTrayGraph { source }` (P3)
- **Build**: `pages/performance/menu.rs` `fn page_menu(menu: PopupMenu, page: &dyn DevicePage, ..) -> PopupMenu`; each page contributes `fn menu_items(&self, menu) -> menu`.
- **Tests**: `page_menu_common_order`, `view_submenu_disables_empty_types`.
- **Phase**: P1 (View submenu P2, tray P3).
- **Decided here**: item order and separators.

### UI-PRF-016 Graph summary mode

- **Implements**: PERF-010.
- **Anatomy** (replaces the whole window content, including the tab strip; the title bar stays with its caption buttons, menus hidden):

```
┌ window_bg, padding sp6 ──────────────────────────────────────────────┐
│ ┌ tile ─────────┐ ┌ tile ─────────┐ ┌ tile ─────────┐                 │
│ │ CPU     12.0% │ │ Memory  55.0% │ │ GPU 0   10.9% │  heading t_md 500 left, value t_md 400 tnum right │
│ │ ▁▂▃▅▃▂▁▂▃▂▁▂ │ │ ▇▇▇▇▇▇▇▇▇▇▇▇ │ │ ▁▁▂▁▁▂▁▁▁▂▁▁ │  chart fills the tile below the label row          │
│ └───────────────┘ └───────────────┘ └───────────────┘                 │
│ ┌ tile ─────────┐ ┌ tile ─────────┐ ...                                │
│ gap sp4 between tiles                                                 │
└───────────────────────────────────────────────────────────────────────┘
cols = ceil(sqrt(n)), rows = ceil(n / cols); tile_w = (W − 2 sp6 − (cols−1) sp4) / cols; tile_h likewise over H.
Minimum tile 200 × 120; when the window is too small for that, cols shrink until tiles fit, and the grid scrolls vertically.
```

  - One tile per **visible** device, in sidebar order, showing its main graph (same series as the sidebar card) with heading = card heading plus line 1 in `text_secondary` after a `·` when the tile is at least 280 px wide ("GPU 0 · NVIDIA GeForce RTX 4080"), and the current value. Legend shown when the tile is at least 200 px wide; range label shown when tile height ≥ 160.
  - A thin 2 px `accent` line under the heading row marks the tile of the device that was selected when summary mode opened (chosen, so the user can tell where Esc returns).
- **States**: entering: the sidebar and page fade out over `m_fast` and the tiles fade in over `m_base`; reduced motion: instant. The tab strip is hidden and tab switching is blocked (Ctrl+1..6, Ctrl+Tab are swallowed by the summary view's `key_context("PerformanceSummary")`, PERF-010).
- **Interaction**: Esc or Ctrl+M → `PerfToggleSummaryMode` (off). Click on a tile → leaves summary mode and selects that device (chosen). Right-click on a tile → the page menu (UI-PRF-015) for that device. Double-click → mini window (R-058 "Performance graph area" includes tiles). Window resize re-tiles.
- **Data binding**: visible devices and their main series. Demand is unchanged (same series as the sidebar).
- **Build**: `pages/performance/summary.rs` `SummaryView` entity, one `ChartView` per tile created on entry (reusing the sidebar cards' `ChartView` entities is not done, because the tile size differs and geometry is keyed by size anyway). The main window's render matches on `summary.is_some()` before the tab strip (shell spec hook: `MainWindow::render` asks the Performance page for `summary_view()`).
- **Performance**: n tiles at 1 Hz plus CPU at 10 Hz; same costs as the sidebar.
- **Accessibility**: Esc always exits; the mode is announced by the window title suffix " · Graph Summary" (chosen).
- **Tests**: `summary_tiles_all_visible_devices`, `summary_blocks_tab_switch`, `summary_esc_returns_to_selected_device`, `summary_tile_grid_for_9` (3 × 3).
- **Phase**: P2.
- **Decided here**: tile grid rule and minimums, heading composition, accent marker, click-to-select, title suffix.

### UI-PRF-017 Overview page

- **Implements**: owner decision D-022 (no PERF requirement yet). Follows the page panel (UI-PRF-010) and the `tile_grid` pattern (UI-PRF-016).
- **Anatomy** (a page in the Performance tab, in the page panel like any device page, with no header row):

```
┌ page panel, padding PAGE_PAD 16, gap sp4 8 ──────────────────────────┐
│ ┌ CPU (cols 0..1) ───────────────────────┐ ┌ MEM (col 2) ──────────┐ │
│ │ CPU   3.40 GHz · 52 °C          12.0%  │ │ Memory          55.0% │ │
│ │ ▁▂▃▅▃▂▁▂▃▂▁▂ total chart │ C0 ▇▇  12%   │ │ ▇▇▇▇▇▇ used chart     │ │
│ │                          │ C1 ▇   4%    │ │ Used      ▇▇▇▇  8 GiB │ │
│ └────────────────────────────────────────┘ └───────────────────────┘ │
│ ┌ GPU ───────────┐ ┌ DISK ──────────────┐ ┌ NET ─────────────────┐  │
│ │ util chart      │ │ IO chart, rows     │ │ down and up charts    │  │
│ │ VRAM ▇▇ 3/16 GiB│ │ nvme0n1 R W ▇ busy │ │ totals since boot     │  │
│ └─────────────────┘ └────────────────────┘ └───────────────────────┘ │
└───────────────────────────────────────────────────────────────────────┘
```

- **Layout** (`pages/performance/overview/layout.rs`, data, so a later editor can change it):
  - `OverviewLayout { columns, rows: [RowSpec { weight, min_h }], boxes: [BoxSpec { kind, row, col, col_span }] }`; `validate` rejects an empty layout, a box outside the grid, two boxes on one cell and a repeated kind.
  - Default (`btop_default`): 3 columns. Row 0: weight 3, minimum 280: CPU (cols 0 to 1) and Memory (col 2). Row 1: weight 2, minimum 220: GPU, Disk and Net, one column each.
  - `place(layout, present, area, scale)`: padding `PAGE_PAD` on every side, gap 8. A row's present boxes share the row width by `col_span` in `col` order, so with every box present a box is `span·unit + (span − 1)·gap` wide, `unit = (W − 2·pad − (cols − 1)·gap) / cols`. An absent kind is dropped and its row neighbours widen; a row with no present box is dropped. Row heights split the height by weight; if a row comes out below its minimum, every row takes its minimum and the page scrolls. Below `STACK_W` (720) the boxes stack in one column, full width, each with its row's minimum, and the page scrolls when taller than the area. `content_h` includes the padding.
  - Examples: 1200 × 900: CPU `(16, 16, 776, 516)`, Memory `(800, 16, 384, 516)`, row 1 at `y = 540`, 344 high, boxes 384 wide. Without a GPU, Disk and Net are 580 wide. 700 × 900 stacks to 668 wide with `content_h` 1284. 1200 × 500 uses 280 and 220 and scrolls with `content_h` 540.
- **Box frame and theme tokens**: all five device boxes use the same `box_frame` component. The frame is `list_bg` with a 1 px rounded border in the device accent and `radius_card`. The 24 px title line starts at the top border. Its title sits in a `list_bg` tab with accent-coloured side and bottom borders, `t_md` 500 text, and `sp4` horizontal padding. The rest of the title line carries the device's key stats: a `t_sm` `text_secondary` summary that ellipsizes and a right-aligned `t_md` 400 tabular value. Body padding is `sp[5]`, with `sp2` between the title line and body. The device accents are theme tokens, not new hard-coded colours: CPU `graph.cpu_line`, Memory `graph.mem_line`, GPU `graph.gpu_line`, Disk `graph.disk_line`, and Network `graph.net_line`. The same token is used by that device's frame, charts, meters and sidebar mini-chart in both Default Dark and Default Light. Top processes uses the CPU token.
- **Charts**: the plot uses `graph.chart_bg` and the faint `graph.grid` token. No chart draws a caption such as "Utilization over 60 seconds". Each box draws one time axis for all charts in it, with only "60 seconds ago" at the left and "Now" at the right. Charts and meters use the box's device accent token.
- **Box contents**:
  - **CPU**: the `CPU_TOTAL` chart (plus `CPU_KERNEL` when `performance.cpu_kernel`), 0 to 100, on the left (40%, CPU box only, crw.53), and one row per logical CPU on the right (under the chart below 560 px box width). Rows flow left to right into as many columns as fit, with cells at least 180 px wide and 8 px apart. A cell is the name "C{n}" (28 px), a 48 px sparkline of the core's `CPU_TOTAL` over the box's time range (filled, 0 to 100, 2 px space above and below), a 6 px meter that takes the rest, the integer percent (36 px) and the clock "3.9 GHz" (44 px), with 4 px gaps, all in `graph.cpu_line` and `t_xs` `text_secondary`. The clock is the newest valid sample of the core's `CPU_FREQ` series with one decimal, or "…" without one. A cell narrower than 188 px (172 px of fixed parts plus a 16 px meter) drops the meter and its sparkline takes the room. Rows are 18 px; when they do not fit they share the height down to 12 px, and below 14 px only the meters show. At 1200 × 900 the box body is 750 × 456 and the side part 438 px wide: 2 columns of 215 px, so 32 threads fill 16 rows (288 px) without scrolling. Each sparkline is a cached `ChartView`, rebuilt only when its series version changes; the box places them in a flex grid over the painted cells and re-renders that grid only when the column count, row count or row mode changes (`parts::class_probe`). Value: total CPU percent. Line: frequency and temperature, each only when Valid or Stale.
  - **Memory**: the `MEM_USED` chart and four meter rows: Used, Available, Cached and Swap ("No swap" without swap). Value: percent used.
  - **GPU**: one section per visible sidebar GPU card, at most 2. The title line shows the first GPU's name and utilization. Each section has the `GPU_UTIL` chart, then one row per engine the device reports, then a VRAM row, then a "62 °C · 120 W" row when the temperature or the power is known (crw.47). GPUs left over (past 2, or when the second section does not fit) get a "+n more" row under the last section, as in the Disk box. An engine row is the engine name (72 px), a meter (percent / 100), the percent (80 px) and a 96 px sparkline of the engine's series over the box's time range, in `graph.gpu_line`, with 8 px gaps. The VRAM row leaves the sparkline column empty so the meters line up. A body narrower than 304 px (72 + 32 px meter + 80 + 96 + 3 × 8, `parts::meter_fits`) drops the meter from the engine rows and keeps the sparkline: name (72 px), sparkline (the rest) and percent (48 px). The VRAM row keeps its meter. At the 720 px page width the box body is 198 px wide, so each sparkline is 62 px; at 1200 px the body is 358 px and the rows show their meters. Engines come from `gpu::shown_engines`: reported engines in the vendor's order, without those in `performance.hidden_gpu_engines`. A shared encode/decode engine (`enc_dec_shared`) is one "Video Encode/Decode" row with the encode value; it is hidden only when both "enc" and "dec" are. Each sparkline is a cached `ChartView`, built only when its series version changes; the rows and charts are rebuilt only when the shown engine list changes, never for a text change.
  - **GPU fit**: each body row costs 18 + 4 px and a later section adds a 22 px heading row, with 12 px between sections. The box body is the box height minus 60 px (2 border, 24 title, 4 + 12 padding, 18 time axis). Each section is as high as its content with a 48 px chart, and the spare height goes to the charts in equal parts. The second section shows only when both fit in full. When one section with its rows and the "+n more" row does not fit, it shows only its busiest engine (the highest current percent) and a "+n more engines" row instead of its other engines. One GPU with 3 engines, VRAM and the temperature and power row needs 48 + 5 × 22 = 158 px: it fits the 220 px row minimum (body 160) and the default 1200 × 900 (box 344, body 284). With a second such GPU, the first section and "+1 more" need 180 px: at 220 px the box shows the busiest engine and "+2 more engines" (158 px), at 1200 × 900 all three engines. Both sections need 158 + 12 + 22 + 158 = 350 px, so they show from a 410 px box up.
  - **Disk** (crw.55): one section per visible sidebar disk card that is a whole disk, at most 8, then "+n more". Partitions never get a section. There is no aggregate chart. A section is a header row and, when expanded, a Read row and a Write row. The header is the name (`t_sm` 500, `text_primary`), the model (`t_sm`, `text_secondary`, takes the rest and ellipsizes) and the active percent (76 px, right-aligned, tabular). The model is the device details' model, else the inventory drive's, only when Valid or Stale and not blank; without one the header shows the name only, and the box reads `DEVICE_DETAILS` and `INVENTORY` for it. A Read or Write row is the word (72 px), a sparkline of the disk's `DISK_READ` or `DISK_WRITE` series over the box's time range (filled, auto-scaled IEC rates, `graph.disk_line`, 18 px high, takes the rest) and the current rate (76 px, right-aligned). Rows have no meter. A click on a header opens that disk (crw.44). Each sparkline is a cached `ChartView`, built only when its disk gets a section, never for a text or catalog change.
  - **Disk fit**: every line (header, Read, Write, "+n more") costs 18 + 4 px, so a body of `h` px holds `floor((h + 4) / 22)` lines. The box body is the box height minus 60 px, as in the GPU box. When the headers and the "+n more" line do not fit, the box shows as many headers as fit with one line left for "+n more", none expanded. Otherwise the spare lines expand sections, two lines each: the busiest disks by active percent first (unknown lowest, ties to sidebar order). Sections stay in sidebar order. There is no chevron and no manual toggle. 3 disks need 9 lines (194 px, a 254 px box). At the 220 px row minimum (body 160, 7 lines) 2 of 3 sections expand; at 1200 × 900 (body 284, 13 lines) all 3 do. A resize re-renders the box only when the number of shown or expanded sections changes (`parts::class_probe`).
  - **Net**: download and upload charts on the aggregate network device, current rates and the totals since boot, over the interfaces whose class `show_network_*` shows.
- **Data binding**: CPU from `SERIES` (per-core samples) and `SYSTEM`; the other boxes from `SYSTEM` and `DEVICES`. The GPU and Disk boxes list the sidebar's visible cards only. A box notifies only when its text changed (hash compare) or, for the core rows, when a 1% bucket or a 0.1 GHz bucket changed.
- **Render contract** (GPUI 0.3.7 marks every ancestor of a notifying view dirty, crw.34; no GPUI fork): an idle tick renders nothing. `OverviewPage` never notifies on a tick; it re-renders only on resize or when a box kind appears or disappears. Each box is a cached child view, so a box that did not change builds no geometry. The page render does fixed work: no snapshot reads and no text formatting.
- **Chart motion**: the Overview charts step at the sample rate and never slide, even with `motion.smooth_graphs` on, because a sliding chart would re-render its box and the page every animation frame (crw.46). Device pages and sidebar cards keep sliding. Revisit after wlf.31 (pixel-paced scrolling).
- **Demand**: while the page is active, `any_surface_visible`, CPU per core with `cpu_fast` (10 Hz CPU), memory, disks, nets and GPUs, plus `process_top` while the Top processes box is placed. Nothing while inactive; the part is removed on release.
- **Top processes box** (opt-in, crw.45): `BoxKind::TopProcs`, not in `btop_default`. With `performance.overview_top_processes = true` the layout is `with_top_procs`: the default plus a third row (weight 2, at least 260 px) where the box spans all 3 columns. The box shows the 10 busiest live processes by CPU, with name, PID, whole CPU percent and a meter (CPU / 100). It picks them with a bounded selection over `Snapshot.processes.records` on each `PROCESSES` change, at `processes.interval_ms` (1 Hz by default). It notifies only when the hash of the shown keys, names and whole percents changes. A left click on a row selects that process on the Processes tab (`ProcessesPage::select_by_key`, PROC-071). `Demand.process_top` starts only the process sweep: no table history window. Off, the box is not built and the page asks for no process data.
- **Sidebar and keys**: the Overview card is the first card by default and moves with `device_order`. It shows the CPU-total mini graph, line 1 "CPU, memory, GPU, disk, network" and an empty line 2. F1 opens Overview and F2 CPU. New profiles keep `selected_device = "cpu"`. Graph summary mode (UI-PRF-016) has no Overview tile.
- **Not yet**: saving the layout and the layout editor.
- **Tests**: `layout.rs::tests::{overview_default_layout_validates, overview_place_default_1200x900, overview_place_without_gpu_widens_row, overview_place_narrow_stacks, overview_place_short_uses_min_heights, overview_validate_rejects_overlap_and_out_of_grid}`; the page and box beads add theirs.
- **Phase**: P1.
- **Decided here**: see D-022 for the owner decisions; the anatomy sketch and the copy of the box contents follow the E24 Overview beads.

---

## Part C: device pages

### UI-PRF-020 Device page frame

- **Implements**: PERF-008 (page headers), PERF-012, PERF-014 (toggles in the header).
- **Anatomy** (inside the page scroll area, padding `page_pad`):

```
┌ header row, page_header_h 36 ─────────────────────────────────────────────────────────┐
│ Processor   AMD Ryzen 7 7800X3D 8-Core Processor            [Per core ▾] [Kernel on]  │
│ ↑ t_lg 500 text_primary   ↑ t_lg 400 text_secondary, sp4 after   ↑ TlButton small, sp4 gap │
└───────────────────────────────────────────────────────────────────────────────────────┘
sp4
[graph area: page-specific, Part C entries]
details_gap 24
Details                      t_md 500 text_primary, 24 px row
sp4
[TlKeyValueGrid 3 columns]
sp6
[page blocks: tables, module blocks, links]
page_pad bottom
```

  - **Header text** (PERF-008): "Processor" + model; "Memory"; "Disk N" + model + ` (nvme0n1)` in `text_secondary` (kernel name); "Graphics adapter" + GPU name; `<ifname>` + adapter name; "Fan N" + chip; battery kind + vendor and model. The first word group is `t_lg` 500 `text_primary`; the rest `t_lg` 400 `text_secondary`, one line, ellipsis. Tooltip with the full text when truncated. `performance.png` shows the model only; tasklens adds the type word per PERF-008 (spec wins).
  - **Toggles**: right-aligned `TlButton::Secondary` sm (24 px) buttons, or `TlButton::Segment { on }` for on/off toggles, 122 px minimum width (measured), `sp4` apart. Dropdown buttons show the kit caret. Which buttons each page has is in its entry.
  - **"Details"** heading (PERF-008): `t_md` 500.
  - **Details grid**: UI-PRF-021.
- **States**: a page whose device went `Missing` is removed by the sidebar; while the removal animation runs the page keeps its last content.
- **Interaction**: right-click anywhere → UI-PRF-015. Double-click on the graph area → `PerfShowMiniWindow` (P3). Text in the Details grid is selectable (PERF-012): values are rendered with the kit `Label`-free plain text in a `div().selectable()`-style GPUI text selection is not available in gpui-pre 0.3.7 for arbitrary divs [inferred from the API map, which lists no text selection on div]; tasklens therefore gives every value a right-click "Copy value" item and a click-to-select behaviour: clicking a value selects its text visually (bg `row_selected`, text `text_selected_primary`) and Ctrl+C copies that value instead of the device info while the selection is active; Esc clears it. Recorded under Open items.
- **Data binding**: header from `DeviceCatalog`; re-render on `DEVICES`.
- **Build**: `pages/performance/page.rs` `DevicePageFrame` (RenderOnce) used by every page entity; trait `DevicePage { fn header(&self) -> (SharedString, SharedString); fn toggles(..); fn graphs(..); fn details(&self) -> &KeyValueModel; fn blocks(..); fn menu_items(..); fn copy_text(&self, out: &mut String) }`.
- **Tests**: `page_header_strings` for each kind.
- **Phase**: P1.
- **Decided here**: header text styles, toggle size, selection fallback for values.

### UI-PRF-021 Details grid (TlKeyValueGrid on device pages)

- **Implements**: PERF-012, PERF-013, R-050.
- **Anatomy**:

```
col 1 (flex 1)           kv_col_gap    col 2 (flex 1)            col 3 (flex 1)
Utilization                            Kernel time               Speed             ← label t_sm 400 text_secondary, 18 px
18.6%                                  2.3%                      4631 MHz          ← value t_base 400 text_primary tnum, 20 px
(sp4 row gap; row pitch kv_row_h 46; measured 46 at 200%)
```

  - Rows fill left to right, top to bottom, in the order each page's entry gives. A row that a device cannot supply shows the PERF-013 word in `text_secondary` (`Collecting…`, `Restricted`, `Needs setup`, `Not supported`, `Not reported`, `Unavailable`, `Unknown`), so the grid shape is stable, except the rows PERF-013 lists as hidden when missing (CPU power with no RAPL zone, CPU temperature with no CPU hwmon, disk WWN, serial, rotation rate, battery charge thresholds, GPU "Max PCI Express" when equal).
  - Composite values keep one line: "46.20 GiB / 67.20 GiB", "1905 / 3105 MHz", "31.9 / 300 W".
  - Tooltips (`TlTooltip`): the reason for a fallback word ("Needs administrator access.", "Not available on this device", "Could not read: {last_error}", `Needs setup` → "Needs setup. Open Additional Setup…" which is a link row: clicking the value dispatches `OpenAdditionalSetup`); row-specific tooltips are listed per page.
  - Stale values: the last value in `text_secondary` with tooltip "Last updated {age} ago".
- **States**: value hover: underline dotted `text_secondary` only when the value has a tooltip or a click action; link values (`Needs setup`, "Fetch", "Display unit") in `accent`.
- **Build**: `TlKeyValueGrid` from the foundation (`columns(3)`, `row_h(46)`); model `KeyValueModel { rows: Vec<KvRow { label: &'static str, value: SharedString, validity: Validity, tooltip: Option<SharedString>, action: Option<Box<dyn Action>> }> }` kept in the page entity and updated on snapshot; a value string re-formats only when its bits change (`FxHashMap<&'static str, u64>` per page).
- **Performance**: 12 to 25 rows; text re-shape only on change. The grid is inside the page's `.cached()` Details block, which re-renders at most at 1 Hz.
- **Tests**: `kv_grid_fallback_words_per_validity`, `kv_grid_hidden_rows_rule`.
- **Phase**: P1.

### UI-PRF-022 Copy device info

- **Implements**: PERF-011, R-012, D-007.
- **Behaviour**: `PerfCopyDeviceInfo` (Ctrl+C in the `Performance` context, page menu "Copy") writes plain text: line 1 the page header words ("Processor"), line 2 the model, then every Details row as `    Label: value` (4-space indent, label as shown, value as shown or the PERF-013 word), in grid order, then each page block in its own paragraph with the block heading as a line and the same indented rows (partitions and swap tables as `    <col>: <value>` per row separated by blank lines). Memory module serials never appear (they are never read). Toast `TlToast` info "Copied metrics to clipboard".
- **Build**: `DevicePage::copy_text(&self, out: &mut String)`; `cx.write_to_clipboard(ClipboardItem::new_string(text))`.
- **Tests**: `copy_text_cpu_format` (golden under `tests/golden/copy_cpu.txt`, updated with `TL_UPDATE_GOLDEN`).
- **Phase**: P2.

### UI-PRF-023 CPU page: header, modes and graph area

- **Implements**: PERF-027, PERF-028, PERF-029, PERF-030, PERF-031, PERF-033 (Utilization value in the header chart), ADR-014 fixture.
- **Anatomy**:

```
Processor  AMD Ryzen 7 7800X3D 8-Core Processor           [Per core ▾] [Kernel on]
sp4
mode = overall:
┌ one UI-PRF-001 chart, W × graph_h_overall 300 ─────────────────────────────────────┐
│ Utilization (heading)                                                   18.6%      │
│ ── Total  ── Kernel                                                                │
│ ...                                                                                │
│ Utilization over 60 seconds                                                        │
└────────────────────────────────────────────────────────────────────────────────────┘
mode = per_core:
┌ UI-PRF-002 grid: n cells, cols = ceil(sqrt(2n)) capped by width, grid_cell_h 112 ──┐
│ CPU 0   6.1% │ CPU 1  3.0% │ CPU 2 14.1% │ CPU 3 18.2% │ CPU 4 11.1% │ CPU 5  9.1% │
│ ── Total     │             │             │             │             │             │
│ ── Kernel    │             │             │             │             │             │
│ ...          │ ...         │ ...         │ ...         │ ...         │ ...         │
└────────────────────────────────────────────────────────────────────────────────────┘
mode = all_cores_overlaid: one chart W × 300, n lines each at alpha max(1/n, 0.08), no fill, no kernel line
mode = all_cores_stacked: one chart W × 300, n stacked filled bands (cumulative sums), Stacked(n) scale, axis max "1600%"
sp4
[secondary graph, UI-PRF-025, W × 120]  (P2; absent when choice is "None")
```

  - **Mode button** (`TlButton::Secondary` sm with the dropdown caret): label is the current mode name; menu radio items "Overall", "Per core", "All cores overlaid", "All cores stacked" → `PerfSetCpuGraphMode { mode }`. Default "Per core". The two extra modes are disabled with tooltip "Available in a later version" before P2 (chosen).
  - **Kernel toggle** (`TlButton::Segment { on }`): label "Kernel on" / "Kernel off" → `PerfToggleCpuKernel`. Default on. In overlaid and stacked modes the button is disabled (PERF-029: no kernel line there) and keeps its label.
  - **Per-core cell**: heading "CPU N", value `6.1%`, legend "Total"/"Kernel" in cell 0 only (PERF-028, `performance.png`), fill `cpu_fill`, line `cpu_line`, kernel overlay `kernel_line` solid 1 px no fill when on (PERF-029). Core marks (PERF-031, P2): a 12 px icon after the heading: Lucide `zap` for P cores and `leaf` for E cores on hybrid CPUs; Lucide `star` for the top-ranked AMD preferred cores with tooltip "Preferred core (rank 236)"; no icon otherwise. Icons use `text_secondary`.
  - **Heading tooltip** (PERF-028): "Core C, thread T, 4200 MHz" (integer MHz, table d).
  - **Offline CPU**: cell shows "Offline" (UI-PRF-002).
  - **Mode change animation**: cross-fade `m_base` between the old and new graph area; reduced motion: instant. Height change animates with the same duration.
- **States**: per UI-PRF-001 and UI-PRF-002. `WarmingUp` on first open: cells show grid and `…`.
- **Interaction**: page menu (UI-PRF-015) page-specific items: "Graph >" with the four mode radio items, "Kernel times" check item → `PerfToggleCpuKernel`, "Secondary graph >" (UI-PRF-025). Hover readout (P2) per cell.
- **Data binding**: `SeriesId`s for `cpu.total`, `cpu.kernel`, `cpu.core[N]`, `cpu.core_kernel[N]` from the CPU device; `Snapshot.system.cpu` for the header, clock and topology. Demand: `cpu: PerCore` in per-core, overlaid and stacked modes, `Total` in overall; `cpu_fast = true` while the page is visible and a chart is moving (smooth on); when smooth is off `cpu_fast` is still true so per-core percent labels update at 10 Hz (chosen: matches the ADR-014 fixture "per-core percent labels updated at 10 Hz").
- **Build**: `pages/performance/cpu.rs` `CpuPage` entity: `overall: Entity<ChartView>`, `grid: Entity<ChartGridView>`, `overlaid: Option<Entity<ChartView>>`, `stacked: Option<Entity<ChartView>>` (created on first use), `secondary: Option<Entity<ChartView>>`, `details: KeyValueModel`. Settings keys `performance.cpu_graph_mode`, `performance.cpu_kernel`.
- **Performance**: this is the `perf-cpu` scene. Budget per ADR-014. The grid is one element (3 paths). Per-core value labels: 32 `ShapedLine`s re-shaped only when text changes (at 10 Hz most change each tick; shaping 32 short strings per 100 ms is inside the budget; the bead measures it).
- **Accessibility**: mode and kernel buttons are focusable with tooltips "Graph mode" and "Show kernel time". Each cell's value is text.
- **Tests**: `cpu_mode_button_labels`, `cpu_kernel_disabled_in_overlaid`, `cpu_grid_16_cores_6_cols`, `cpu_core_mark_icons`, `cpu_demand_percore_only_when_needed` (overall mode sends `Total`).
- **Phase**: P1 (overall, per core, kernel); P2 (overlaid, stacked, marks, secondary graph).
- **Decided here**: `graph_h_overall` 300, cross-fade on mode change, mark icons, disabled-mode tooltip, `cpu_fast` with smooth off.

### UI-PRF-024 CPU page: Details grid

- **Implements**: PERF-033, PERF-034, PERF-035, PERF-036, PERF-037, PERF-038, PERF-039, PERF-040.
- **Rows** (exact labels, grid order, 3 columns left to right):

| # | Label | Value example | Tooltip / notes | Phase |
|---|---|---|---|---|
| 1 | Utilization | `18.6%` | | P1 |
| 2 | Kernel time | `2.3%` | | P1 |
| 3 | Speed | `4631 MHz` | "Fastest core: 5050 MHz"; BogoMIPS-only: value `4200 MHz (?)`, tooltip "Reported by the kernel at boot; the current clock is not available" | P1 |
| 4 | Processes | `295` | | P1 |
| 5 | Threads | `5 596` | | P1 |
| 6 | Handles | `253 248` | "Open file descriptors, system-wide" | P1 |
| 7 | Up time | `2 days, 2h 38m 15s` | tooltip "Boot time 2026-10-01 07:12:33" | P1 |
| 8 | Base speed | `4200 MHz` | | P1 |
| 9 | Sockets | `1` | | P1 |
| 10 | Power draw | `31.9 W` | "Package 31.9 W · Core 18.2 W"; `Needs setup` link when RAPL exists but is unreadable; row hidden when no RAPL zone | P2 |
| 11 | Temperature | `52 °C` | "Tctl 52 °C · Tccd1 48 °C · Tccd2 50 °C"; row hidden when no CPU hwmon | P1 |
| 12 | Load average | `1.24 0.98 0.87` | "1, 5 and 15 minutes" | P1 |
| 13 | Cores | `8` | | P1 |
| 14 | Logical processors | `16` | | P1 |
| 15 | Virtualization | `Enabled (KVM available)` | | P2 |
| 16 | Running in VM | `No` | | P2 |
| 17 | Security features | `Secure Boot, IOMMU` | "12 mitigated, 0 vulnerable, 3 not affected" plus "Could not read: lockdown" when a source failed | P2 |
| 18 | Hypervisor | `None` | | P2 |
| 19 | L1 cache | `512 KiB` | "16 × 32 KiB data + 16 × 32 KiB instruction" | P1 |
| 20 | L2 cache | `8 MiB` | "8 × 1 MiB" | P1 |
| 21 | L3 cache | `96 MiB` | "98 304 KiB · 1 × 96 MiB"; L4 appended when present | P1 |
| 22 | Cpufreq driver | `amd-pstate-epp` | | P2 |
| 23 | Governor | `powersave` | | P2 |
| 24 | Power preference | `balance_performance` | | P2 |
| 25 | Platform profile | `balanced` | | P2 |
| 26 | Microcode | `0xa601206` | | P2 |

  Rows 1 to 12 are the live group; the grid re-renders at 1 Hz (`SYSTEM`). Rows 13 to 26 are static (`DEVICES`). Before P2 the P2 rows are not shown (the grid has 14 rows).
- **Build**: `CpuPage::details`, formatter calls from `tl-model::units` (`fmt_percent`, `fmt_mhz`, `fmt_count`, `fmt_uptime`, `fmt_watts`, `fmt_celsius`, `fmt_cache`).
- **Tests**: `cpu_details_order_p1`, `cpu_details_order_p2`, `cache_tooltips`.
- **Phase**: as per row.
- **Decided here**: tooltip texts for Up time, Load average and BogoMIPS.

### UI-PRF-025 CPU secondary graph

- **Implements**: PERF-032, PERF-038, PERF-039, PERF-040.
- **Anatomy**: one UI-PRF-001 chart, W × `secondary_graph_h` 120, under the main graph area with `sp4` gap. Heading and range word per choice:

| Choice | Heading | Series | Scale | Range label |
|---|---|---|---|---|
| Power draw | "Power draw" | package W (fill cpu_fill, line cpu_line); core W as a dashed line | `StickyUp { hw_max: None }` | "Power draw over 60 seconds" |
| Clock speed | "Clock speed" | average MHz filled; maximum-core MHz dashed | `StickyUp { hw_max: cpuinfo_max_freq }` | "Clock speed over 60 seconds" |
| Temperature | "Temperature" | main sensor °C | `StickyTemp` | "Temperature over 60 seconds" |
| None | chart absent | | | |

  Axis max shown at top right (UI-PRF-003). Value label is the newest value (`31.9 W`, `4631 MHz`, `52 °C`).
- **States**: the saved choice is `performance.cpu_secondary_graph` (default "power"). When power is not available the graph shows Temperature, else Clock speed, without changing the saved key (PERF-032); the heading then carries a `TlTooltip` "Power draw is not available. Showing Temperature." (chosen). When nothing is available the chart is absent.
- **Interaction**: page menu "Secondary graph >" radio items "None", "Power draw", "Clock speed", "Temperature" → `PerfSetCpuSecondaryGraph { choice }`. "Power draw" is disabled with tooltip "Needs setup" when RAPL is unreadable; the tooltip's text is a link row that opens the Additional Setup window (clicking the disabled item does nothing; a second menu item "Set up CPU power draw…" appears directly under it in that case and dispatches `OpenAdditionalSetup`, chosen so the link is reachable from the menu).
- **Data binding**: `cpu.power_pkg`, `cpu.power_core`, `cpu.clock_avg`, `cpu.clock_max`, `cpu.temp` series at 1 Hz. Demand: `sensors` while the choice is temperature; RAPL is part of the CPU worker.
- **Build**: `CpuPage.secondary`.
- **Tests**: `secondary_fallback_without_power`, `secondary_menu_disabled_needs_setup`.
- **Phase**: P2.
- **Decided here**: height 120, fallback tooltip, "Set up CPU power draw…" item.

### UI-PRF-026 Memory page: graphs

- **Implements**: PERF-041, PERF-026 (legend), PERF-018 (scales).
- **Anatomy**: a vertical stack of up to three UI-PRF-001 charts, each W × `graph_h_stacked` 150, `sp4` apart:

```
┌ Physical memory                                    34.68 GiB ┐  heading; value = used (table d)
│ ── Used  ── Committed                                  (legend; dirty: thin line, no legend)
│ fill mem_fill (used), dashed mem_line (committed), 1 px mem_line @0.6 (dirty)
│ Memory usage over 60 seconds                                  │  scale FixedTotal(MemTotal)
└───────────────────────────────────────────────────────────────┘
┌ Commit                                                 69.0% ┐  value = Committed_AS / CommitLimit
│ fill mem_fill; scale Commit(limit); axis max text when > 100% │  heading tooltip "Memory the system has promised to programs, as a share of the commit limit. It can pass 100% because Linux allows overcommit."
│ Commit over 60 seconds                                        │
└───────────────────────────────────────────────────────────────┘
┌ Swap                                                 1.20 GiB ┐  hidden when SwapTotal = 0 or memory_swap_graph = false
│ fill mem_fill; scale FixedTotal(SwapTotal)                    │
│ Swap usage over 60 seconds                                    │
└───────────────────────────────────────────────────────────────┘
sp6
[composition bar, UI-PRF-027]  (hidden when memory_composition = false)
```

- **Interaction**: page menu page-specific items: "Show memory composition" (check) → `PerfToggleMemoryComposition`; "Show swap usage" (check) → `PerfToggleMemorySwapGraph`. Hiding and showing animates height 150 → 0 over `m_base`; reduced motion: instant.
- **Data binding**: series `mem.used`, `mem.committed`, `mem.dirty`, `mem.commit_pct`, `swap.used`; totals from `Snapshot.system.memory` (`SYSTEM`). Demand `memory`.
- **Build**: `pages/performance/memory.rs` `MemoryPage` with three `ChartView`s.
- **Tests**: `memory_swap_graph_hidden_without_swap`, `commit_axis_text_above_100`.
- **Phase**: P1.
- **Decided here**: stacked height 150, commit heading tooltip.

### UI-PRF-027 Memory composition bar

- **Implements**: PERF-042.
- **Anatomy**:

```
Memory composition      t_sm 400 text_secondary (caption above the bar, chosen)
┌──────────────┬──┬───┬─────────────┬──────────────┐  height 24, r_sm, 1 px divider outline
│ In use       │▒▒│███│ Standby     │ Free         │  segments left to right: In use, In use (compressed), Modified, Standby, Free
└──────────────┴──┴───┴─────────────┴──────────────┘
  mem_line      mem_line  status.   mem_line @0.35  window_bg (free stays the background colour)
  @1.0          hatched   warning
                @0.8      @0.8
```

  - Segment widths are proportional to bytes over MemTotal; a non-zero segment is at least 2 px wide (chosen). Segment labels (`t_xs` 400 `text_selected_primary` on dark segments, `text_secondary` on Free) are drawn only when the segment is at least 72 px wide; otherwise the tooltip carries the name.
  - Hatching for compressed: 1 px diagonal lines every 4 px in `window_bg` at alpha 0.5 painted with `paint_path` once per rebuild (chosen, matches "In use (compressed)" being a sub-state of In use).
  - Tooltips (`TlTooltip`, exact one-liners, PERF-042):
    - "In use: 20.11 GiB. Memory used by running programs and the kernel."
    - "In use (compressed): 1.02 GiB. Memory held compressed in zram or zswap."
    - "Modified: 120.00 MiB. Dirty pages waiting to be written to disk."
    - "Standby: 12.40 GiB. Cache the kernel can drop."
    - "Free: 28.60 GiB. Unused memory."
- **States**: `WarmingUp`: bar outline only, caption "Collecting…". Hover on a segment: segment at alpha +0.15 and the tooltip.
- **Build**: `pages/performance/memory.rs` `CompositionBar` custom element (quads plus one hatch path); one hitbox per segment only while the pointer is inside the bar (same rule as UI-PRF-002).
- **Tests**: `composition_segments_sum_to_total`, `composition_min_width_2px`, `composition_tooltip_texts`.
- **Phase**: P1.
- **Decided here**: all geometry, colours, caption, tooltip sentences.

### UI-PRF-028 Memory page: Details grid and blocks

- **Implements**: PERF-043, PERF-044, PERF-045, PERF-046, D-007.
- **Details rows** (grid order):

| # | Label | Value example | Notes | Phase |
|---|---|---|---|---|
| 1 | In use | `20.11 GiB` | | P1 |
| 2 | Available | `41.20 GiB` | | P1 |
| 3 | Committed | `46.20 GiB / 67.20 GiB` | | P1 |
| 4 | Cached | `12.40 GiB` | | P1 |
| 5 | Kernel slab (reclaimable) | `1.10 GiB` | tooltip "SReclaimable" | P1 |
| 6 | Kernel slab (unreclaimable) | `420.00 MiB` | tooltip "SUnreclaim" | P1 |
| 7 | Speed | `3200 MT/s` | configured speed | P2 |
| 8 | Slots used | `2 of 4` | | P2 |
| 9 | Form factor | `DIMM` | | P2 |
| 10 | Type | `DDR4` | | P2 |
| 11 | Hardware reserved | `0 B` | tooltip "Sum of module sizes minus MemTotal" | P2 |
| 12 | Swap used | `1.20 GiB` | | P1 |
| 13 | Swap available | `30.80 GiB` | SwapFree | P1 |
| 14 | Compressed | `1.02 GiB` | shown only when ≥ 1 MiB compressed | P1 |
| 15 | Savings | `2.30 GiB` | shown with Compressed | P1 |
| 16 | Huge pages | `2 048 × 2 MiB` | shown only when non-zero | P1 |

- **Blocks** below the grid, each with a `t_md` 500 heading and `sp6` above:
  1. **"Compressed memory"** (PERF-044, P2): one `TlKeyValueGrid` (3 columns) per zram device with a `t_sm` 500 sub-heading "zram0": rows Device, Size, Stored, Compressed, Memory used, Ratio (`3.4 : 1`), Algorithm. Then a 3-column grid "zswap": row "Enabled" (`No`); when enabled also Pool size and Stored. Hidden entirely when there is no zram device and zswap is disabled (chosen).
  2. **"Swap devices"** (PERF-045, P2): a flat `TlTreeTable` (UI-PRC-001: `HeaderLines::One` 28 px header, `columns_fixed`, `RowKind::Plain` rows `row_h` with `row_alt` striping, `ScrollbarMode::Always`) with columns Name (flex), Type (80), Size (90, right), Used (90, right), Priority (70, right). Empty: the block is hidden when `/proc/swaps` has no entries.
  3. **"Memory modules"** (PERF-046, P2): a line "Slots used: 2 of 4" (`t_sm` `text_secondary`), then one collapsible block per populated DIMM: header row 28 px with a chevron `TlButton` ghost 20 px and the title "Memory module N" (`t_sm` 500); body a 3-column `TlKeyValueGrid` with rows Slot, Capacity, Speed, Configured speed (only when it differs), Type, Form factor, Manufacturer, Part number, Rank, Voltage. Blocks start collapsed when there are more than 4 modules; expand state is not persisted. When the udev keys are missing: the block list is replaced by one grid row "Module details" = `Not reported`. No serial anywhere (D-007).
- **Build**: `MemoryPage.details`, `MemoryPage.blocks`. The module block uses a GPUI `div` with the chevron rotating 90° over `m_fast` (reduced motion: instant).
- **Tests**: `memory_details_order`, `memory_hidden_rows_rule`, `modules_collapsed_when_more_than_4`, `no_serial_in_copy_text`.
- **Phase**: as per row and block.
- **Decided here**: block visibility rules, table column widths, module header layout, Huge pages format.

### UI-PRF-029 Disk page: graphs and header buttons

- **Implements**: PERF-047, PERF-048, PERF-051 (button), PERF-052 (button), PERF-053.
- **Anatomy**:

```
Disk 0  WD_BLACK SN850X 1000GB  (nvme0n1)                 [Show SMART data] [Eject]
sp4
┌ Active time                                             12.0% ┐  W × 150, fill disk_fill, line disk_line, Fixed 0..100
│ Active time over 60 seconds                                   │
└───────────────────────────────────────────────────────────────┘
┌ Transfer rate                                     12.3 MiB/s  ┐  W × 150; value = read + write
│ ── Read  ── Write (dashed)                       16 MiB/s     │  axis max, AutoRate { iec: true }
│ Throughput over 60 seconds                                    │
└───────────────────────────────────────────────────────────────┘
```

  - **Show SMART data** (`TlButton::Secondary` sm): present only when SMART is available for the drive (UDisks2 ATA or NVMe interface present, or the P3 smartctl fallback applies); → `PerfShowSmart { key }` opens the SMART window (UI-PRF-030).
  - **Eject** (`TlButton::Secondary` sm, icon `eject`): present only for ejectable or removable drives; visibility updates live (PERF-052); → `PerfEject { key }`.
- **States**: per UI-PRF-001. A disk with no mounted filesystem still has both graphs.
- **Interaction**: page menu: no page-specific items in P1; P2 adds "Show SMART data…" and "Eject" mirroring the buttons.
- **Data binding**: series `disk.active`, `disk.read`, `disk.write` from `/proc/diskstats` at 1 Hz (`SERIES`); `DeviceCatalog` for kind, model, removable (`DEVICES`). Demand `disks`.
- **Build**: `pages/performance/disk.rs` `DiskPage` with two `ChartView`s.
- **Tests**: `disk_buttons_visibility`, `disk_transfer_value_is_sum`.
- **Phase**: P1 (buttons P2).
- **Decided here**: Transfer rate value = read + write.

### UI-PRF-030 Disk page: Details grid, partitions table, SMART window and "Disk is in use" window

- **Implements**: PERF-049, PERF-050, PERF-051, PERF-052, PERF-053, R-036, D-009.
- **Details rows** (grid order): Active time (`12.0%`), Average response time (`0.42 ms`), Read speed (`4.2 MiB/s`), Write speed, Total read (`812.40 GiB`), Total written, Capacity (`931.51 GiB`), Formatted (`920.10 GiB`), Used space, Free space, System disk (`Yes`/`No`), Swap (`Yes`/`No`), Hardware name (`WD_BLACK SN850X 1000GB`), Type (`NVMe`), Interface (`nvme`), WWN, Serial, Rotation rate (`7200 RPM`), Temperature (`41 °C`). WWN, Serial and Rotation rate hidden when missing. Phases: rows 1 to 4, 7 and 2 are P1; the rest P2.
- **Partitions table** (PERF-050, P2): heading `"3 partitions - GPT"` (`t_md` 500; `"1 partition - MBR"`; `"No partitions"` when the table is empty, chosen). flat `TlTreeTable` (as the Swap devices table) columns and widths: Partition 120, Size 90 right, Offset 110 right, Device 110, Mount path flex (tooltip "Additional mount points: /a, /b" when more), Label 120, File system 90, Usage 140 (a 12 px high bar: `accent` fill over `divider` track with `r_sm`, plus `42.1%` text right of it in `t_xs`), Flags 110, Type flex. Rows ordered by offset; "Unallocated" rows show `-` in every column except Size, Offset and Type (`Unallocated`), in `text_secondary`. Column widths are not resizable (static table).
- **SMART window** (PERF-051): own window "tasklens SMART Data" (a `TlSecondaryWindow` tool window, UI-FND-021, `WindowKey::Smart(DeviceIdx)`, 760 × 520 default, minimum 560 × 360). Content: a header `TlKeyValueGrid` with 4 columns: Last updated (`2m 5s ago`), Powered on (`1 day, 4h 12m 0s`, uptime format), Drive health (`OK` in `status.success`, `FAILING` in `status.danger`, `Unknown`), Self-test status (`Completed without error`, `In progress 40%`). A `TlButton` small "Refresh" (→ `PerfSmartRefresh { key }`; shows a 16 px spinner icon inside the button while the request runs) at the right. Then a `TlTreeTable` with a flat delegate (`RowKind::Plain`, no expansion, no selection actions; the delegate sorts its small `Vec` on header click, Desc first; no `TlStaticTable` exists, R-193) with the ATA columns ID (40, right), Attribute (flex), Value (80, right), Normalized (90, right), Threshold (80, right), Worst (70, right), Type (80), Updates (80), Assessment (110: `OK` `text_primary`, `Failed in the past` `status.warning`, `FAILING` `status.danger`). NVMe shows a two-column key/value list instead (Attribute flex, Value 160 right) with the PERF-051 rows in order. Failure: the table area shows "Failed to get SMART data" (`t_md` 500) and the reason (`t_sm` `text_secondary`) centred. Fallback (P3): when UDisks2 lacks SMART, the header shows a `TlButton` "Read with smartctl (admin)" → `PerfSmartReadSmartctl { key }`.
- **"Disk is in use" window** (PERF-052): own window "tasklens Disk is in use" (`TlSecondaryWindow` tool window, `WindowKey::DiskInUse(DeviceIdx)`, 520 × 400): text line "These programs are using <Disk N> (<model>):", then a list grouped by app: group header row (app icon 16 px, app name `t_sm` 500), process rows 28 px (name, PID in `text_secondary`, then the open paths joined with ", " in `t_xs` `text_secondary`, ellipsis, tooltip with all paths), each with an "End Task" `TlButton` small danger-outline at the right; holding Shift relabels every row button to "End Process" (SIGKILL). Footer buttons right-aligned: "Close" (default), "Retry", "End All" (danger). "End All" with more than one process and any action on another user's or a protected process ask first with the R-036 dialogs; a single End Task on the user's own process does not ask (D-009). After an end action the unmount retries automatically after 500 ms. A note line "Processes of other users may also hold this disk" appears above the footer in `text_secondary` when any `/proc/*/fd` read failed with EACCES. Success closes the window and shows the toast "Disk ejected".
- **Build**: `pages/performance/disk.rs` for the page; `windows/smart.rs` and `windows/disk_in_use.rs` for the windows (opened with `TlSecondaryWindow::open` and tracked in `WindowRegistry`).
- **Tests**: `disk_details_order`, `partitions_heading_text`, `unallocated_row_rendering`, `smart_assessment_colours`, `disk_in_use_shift_relabels`.
- **Phase**: P1 rows; P2 the rest; P3 smartctl.
- **Decided here**: all table widths, usage bar, window sizes, "No partitions", button variants, retry delay, Shift relabel behaviour.

### UI-PRF-031 Network page

- **Implements**: PERF-054, PERF-055, PERF-056, PERF-057, PERF-058, PERF-059, PERF-060, PERF-061.
- **Anatomy**:

```
enp38s0  Intel I350 Gigabit Network Connection                       (no toggles)
sp4
┌ Throughput                                               1.5 Mbps ┐  W × graph_h_single 240; value = download + upload
│ ── Download  ── Upload (dashed, tinted)                 10 Mbps   │  scale AutoRate { iec: false } or LinkSpeed
│ Throughput over 60 seconds                                        │
└───────────────────────────────────────────────────────────────────┘
details_gap
Details
[grid]
sp6
Network settings      (link row, accent, Lucide `external-link` 12 px; P3, KDE and GNOME only)
```

- **Details rows** (grid order): Download speed (`1.2 Mbps`), Upload speed, Total downloaded (`12.40 GiB`), Total uploaded, Adapter name (`Intel I350 Gigabit Network Connection` or `Unknown adapter`), Interface name (`enp38s0`), Connection type (`Ethernet`), Connection status (`Connected`), Link speed (`1000 Mbps`; Wi-Fi: `RX 866 Mbps · TX 866 Mbps`; -1: `Not reported`), IPv4 address (`192.168.1.10/24`; several joined with ", "), IPv6 address (global first; link-local in the tooltip "Link-local: fe80::…"), MAC address, Interface index (`2`), Driver (`igb`), Display unit (link value, UI below), Public IP (UI below). Wi-Fi inserts after Connection status: SSID, Signal (`-52 dBm (78%)` with a 16 px 4-bar icon before the text: bars lit by strength ≥ 25/50/75/100 in `accent`, unlit in `divider`), RX bitrate, TX bitrate, Frequency (`5180 MHz, channel 36`), Security (`WPA2`). Phases: rates and totals P1; Display unit P1; the rest P2; Network settings link P3.
- **Display unit row** (PERF-057): value text `Mbps`, `MiB/s`, `bits/s, auto` or `bytes/s, auto` in `accent`; tooltip "Applies to all network values"; click opens a `TlMenu` under the value with radio items "Bits per second", "Bytes per second" → `SetSetting { UnitsNetwork, unit }` (global setting `units.network`).
- **Public IP row** (PERF-060): value `Not fetched` (`text_secondary`) followed by a `TlButton::Secondary` sm "Fetch" inline (24 px high, `sp3` left margin). Fetching: button shows the spinner icon and is disabled; value `Fetching…`. Result: `203.0.113.4 (fetched 14:02)`; with IPv6 a second line `2001:db8::4 (fetched 14:02)` (chosen). Failure: `Unavailable` with tooltip "Could not reach api.ipify.org: {error}". The first click opens the "tasklens Public IP Notice" window (APP-056, shell spec) and fetches only after "Continue".
- **Network settings** link (PERF-061): `PerfOpenNetworkSettings`; hidden on desktops other than KDE and GNOME; also a page menu item.
- **Data binding**: series `net.rx`, `net.tx`; `DeviceCatalog` for type, state, adapter name; `Snapshot.system.net[ifindex]` for addresses and Wi-Fi (`SYSTEM`); demand `nets`. The `LinkSpeed` scale reads `performance.network_dynamic_scale` and the sysfs speed.
- **States**: disconnected interface: graph shows a gap after the last sample (the engine marks samples invalid when operstate is down, R-051 rule set), header unchanged, Connection status row shows the state word. `tailscale0` with operstate unknown and no carrier: `Unknown` (PERF-054).
- **Build**: `pages/performance/net.rs` `NetPage` with one `ChartView` and a `KeyValueModel`; the signal icon is a tiny custom element of 4 quads.
- **Tests**: `net_details_order_ethernet`, `net_details_order_wifi`, `display_unit_values`, `public_ip_states`, `link_speed_scale_unknown_falls_back`.
- **Phase**: as per row.
- **Decided here**: `graph_h_single` 240, Throughput value = sum, IPv6 second line, signal icon geometry, Fetch button size, failure tooltip.

### UI-PRF-032 GPU page: graph rows

- **Implements**: PERF-062, PERF-063, PERF-064, PERF-065, PERF-066, PERF-067, PERF-068, R-054.
- **Anatomy**:

```
Graphics adapter  NVIDIA GeForce RTX 4080                                        (no toggles)
sp4
row 1: UI-PRF-002 grid of engine cells (cols = ceil(sqrt(2n)) capped by width, grid_cell_h 112)
┌ 3D / Compute  10.9% │ Memory controller  4.0% │ Video Encode  0.0% │ Video Decode  0.0% │ JPEG 0.0% │ Optical flow 0.0% ┐
sp4
row 2: two charts side by side, each (W − sp4) / 2 × graph_h_stacked 150
┌ Memory                        2.80 GiB / 15.92 GiB ┐ ┌ Temperature                               52 °C ┐
│ ── Dedicated  ── Shared (dashed; absent on NVIDIA)│ │ StickyTemp; axis max "90 °C"                      │
│ FixedTotal(total); "Memory usage over 60 seconds" │ │ "Temperature over 60 seconds"                     │
└───────────────────────────────────────────────────┘ └───────────────────────────────────────────────────┘
sp4
row 3 (optional, P2): same split; present per performance.gpu_optional_graphs
┌ Power                                31.9 / 300 W ┐ ┌ Clocks                           1905 / 3105 MHz ┐
│ StickyUp { hw_max: power limit }                  │ │ ── GPU clock  ── Memory clock (dashed); hw_max   │
│ "Power draw over 60 seconds"                      │ │ "Clock speed over 60 seconds"                     │
└───────────────────────────────────────────────────┘ └───────────────────────────────────────────────────┘
```

  - Engine names per vendor exactly as PERF-063 (NVIDIA: "3D / Compute", "Memory controller", "Video Encode", "Video Decode", "JPEG", "Optical flow"; AMD: "Graphics", "Compute", "Memory controller", "Video Encode", "Video Decode", "JPEG", "DMA"; Intel: "Render/3D", "Compute", "Copy", "Video", "Video Processing"). Engines the device does not report are not listed; `NVML_ERROR_NOT_SUPPORTED` hides the engine. Hidden engines (`performance.hidden_gpu_engines`) are not drawn and the grid re-tiles. Fills `gpu_fill`, lines `gpu_line`.
  - When only one optional graph is on, it takes the full width of row 3.
  - Engine cell tooltip (heading): "Own processes only" on AMD when any fdinfo read failed (PERF-074); the Clocks heading tooltip shows the P-state and throttle reasons ("P3 · Throttled: power cap") and the Power heading tooltip the limit range and energy ("Limit range 100 to 350 W · 1.24 kWh since boot", PERF-066, PERF-067).
- **States**: probe restarting: every GPU chart value shows `!` with tooltip "GPU monitor restarting" and the plots show a gap (PERF-062). Probe missing: values `n/a` with tooltip "GPU probe not installed"; the engine grid shows one cell "3D / Compute" with `n/a` so the page keeps its shape (chosen).
- **Interaction**: page menu page-specific items: "Select GPU engine graphs >" (check items: each engine name, then separator, "Power", "Clocks") → `PerfToggleGpuEngine { key }` and `PerfToggleGpuOptionalGraph { which }`; on an engine cell the menu also has "Hide this engine graph" → `PerfToggleGpuEngine { key }` (PERF-063). Unchecking the last engine is allowed (row 1 collapses to height 0 with the `m_base` animation).
- **Data binding**: series `gpu.engine[k]`, `gpu.mem_dedicated`, `gpu.mem_shared`, `gpu.temp`, `gpu.power`, `gpu.clock_gpu`, `gpu.clock_mem` at 1 Hz (`SERIES`); totals and limits from `Snapshot.system.gpu[idx]` (`SYSTEM`). Demand `gpus`.
- **Build**: `pages/performance/gpu.rs` `GpuPage`: `engines: Entity<ChartGridView>`, `memory`, `temperature`, `power: Option<..>`, `clocks: Option<..>`.
- **Tests**: `gpu_engine_names_nvidia`, `gpu_hidden_engine_retiles`, `gpu_probe_restart_glyphs`, `gpu_optional_row_full_width_when_one`.
- **Phase**: P1 (3D/Compute, encode, decode engines, Memory, Temperature); P2 (other engines, hiding, Power, Clocks).
- **Decided here**: row 2 split, placeholder engine cell when the probe is missing, tooltip texts.

### UI-PRF-033 GPU page: Details grid and displays list

- **Implements**: PERF-064 to PERF-073.
- **Details rows** (grid order; TaskSlinger rows first, Mission Center rows after):

| # | Label | Value example | Notes | Phase |
|---|---|---|---|---|
| 1 | Utilization | `10.9%` | the 3D/Compute engine | P1 |
| 2 | Dedicated memory | `2.80 GiB / 15.92 GiB` | | P1 |
| 3 | Shared memory | `Not supported` on NVIDIA | | P1 |
| 4 | Hardware reserved memory | `380.00 MiB` | | P1 |
| 5 | Video encode | `0.0%` | tooltip "2 active sessions" (NVIDIA) | P1 |
| 6 | Video decode | `0.0%` | one row "Video encode/decode" when shared | P1 |
| 7 | Temperature | `52 °C` | tooltip lists all sensors on AMD | P1 |
| 8 | Temperature source | `NVML (GPU core)` | PERF-069 values | P1 |
| 9 | Fan speed | `0% (0 RPM)` | | P2 |
| 10 | Slowdown temperature | `88 °C` | | P2 |
| 11 | Shutdown temperature | `95 °C` | | P2 |
| 12 | Memory temperature | `Not reported` | | P2 |
| 13 | Clock speed | `1905 / 3105 MHz` | tooltip "P3" | P2 |
| 14 | Memory speed | `7001 / 14001 MHz` | | P2 |
| 15 | Video clock | `1620 MHz` | | P2 |
| 16 | Power draw | `31.9 / 300 W` | tooltip "max 350 W · 1.24 kWh since boot" | P2 |
| 17 | PCI Express | `Gen 4 x16` | | P2 |
| 18 | Max PCI Express | `Gen 5 x16` | hidden when equal to row 17 | P2 |
| 19 | PCIe throughput | `↓ 12.3 MiB/s ↑ 1.2 MiB/s` | | P2 |
| 20 | Driver version | `615.71.09` | | P2 |
| 21 | Kernel module | `nvidia 615.71.09` | | P2 |
| 22 | CUDA version | `13.2` | NVIDIA only; else row hidden (chosen) | P2 |
| 23 | Vulkan version | `1.4.312` | | P2 |
| 24 | OpenGL version | `4.6.0 NVIDIA 615.71.09` | | P2 |
| 25 | Physical location | `PCI bus 45, device 0, function 0` | | P2 |
| 26 | VBIOS version | `95.03.3A.00.1B` | | P2 |
| 27 | UUID | `GPU-…` | tooltip shows the full UUID; cell truncates | P2 |
| 28 | Architecture | `Ada Lovelace` | | P2 |

- **Displays block** (PERF-072, PERF-073, P2): heading "Displays" (`t_md` 500), one row per enabled display, 24 px, `t_sm` `text_primary`, format exactly `<monitor name> · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%`, with ` (primary)` after the name on the primary. Missing orientation prints `Not reported` in its slot; missing bit depth prints `Max bit depth Not reported`. Empty: "No display connected" (`text_secondary`).
- **Tests**: `gpu_details_order`, `gpu_max_pcie_hidden_when_equal`, `display_row_format`.
- **Phase**: per row.
- **Decided here**: CUDA row hidden off NVIDIA, display row height, empty text.

### UI-PRF-034 Fan page

- **Implements**: PERF-076, PERF-077, PERF-108 (link when no fans).
- **Anatomy**:

```
Fan 1  nct6797                                                           (no toggles)
sp4
┌ Fan speed                                               1200 RPM ┐  W × 150; RPM filled (Violet), PWM % dashed on a second 0..100 axis (right side, no labels; chosen)
│ ── RPM  ── PWM                                           1800 RPM │  axis max = fanN_max when present (StickyUp hw_max), else sticky 1-2-5
│ Fan speed over 60 seconds                                         │
└───────────────────────────────────────────────────────────────────┘
┌ Temperature                                                45 °C ┐  W × 150; the paired sensor; StickyTemp
│ Temperature over 60 seconds                                       │  no sensor: plot empty, centred text "Choose a sensor" t_sm text_secondary, value "-"
└───────────────────────────────────────────────────────────────────┘
```

- **Details rows**: Fan speed (`1200 RPM`), PWM (`45%`), Control mode (`Automatic` / `Manual` / `Full speed`), Temperature (`45 °C`; `Not reported` when no sensor is paired), Chip (`nct6797`).
- **Interaction**: page menu page-specific items (P3): "Rename fan…" → `PerfRenameFan { key }` opens a `TlDialog` with one text input (default the current label, placeholder "Fan name") and buttons "Rename" (primary) and "Cancel"; empty input restores the default label. "Temperature sensor >" → radio items, one per hwmon temperature `"<chip> <label>  48 °C"` (value right-aligned in `text_secondary`) plus "None" → `PerfSetFanTempSensor { key, sensor }`.
- **Sidebar visibility**: fans that read 0 RPM for the whole session and have no label are hidden unless `performance.show_disconnected_fans` (Settings, shell spec).
- **Data binding**: series `fan.rpm`, `fan.pwm`, `temp.<sensor>` at 1 Hz; demand `sensors`.
- **Build**: `pages/performance/sensors.rs` `FanPage`.
- **Tests**: `fan_choose_a_sensor_state`, `fan_rename_empty_restores_default`.
- **Phase**: P2 (page); P3 (rename and sensor menu).
- **Decided here**: dual axis for PWM, rename dialog layout, sensor item format.

### UI-PRF-035 Battery and power device page

- **Implements**: PERF-078, PERF-079.
- **Anatomy**:

```
Mouse  Logitech MX Master 3S                                              (no toggles)
sp4
system battery:  ┌ Power                                     -12.4 W ┐  W × 150; Symmetric scale, zero line at mid height (1 px divider),
                 │ charge positive filled above the line, discharge filled below (Mint)   axis "±20 W"
                 │ Power draw over 60 seconds                        │
peripheral:      ┌ Percentage                                  85.0% ┐  W × 150; Fixed 0..100
                 │ Percentage over 60 seconds                        │
sp4
system battery with ≥ 2 history points (P3):
┌ Battery charge over the past week                               ┐  W × 150; x axis = 7 days, vertical grid at local midnight,
│ day labels "Mon" … under the plot in t_xs text_secondary (chosen) │  line Mint, dashed across gaps (dash 5/5), no fill, Fixed 0..100
└───────────────────────────────────────────────────────────────────┘
```

- **Details rows** (grid order): Percentage (`85.0%`), Energy (`42.10 Wh`), Voltage (`11.80 V`), Power (`12.4 W`), State (`Discharging`), Time to empty or Time to full (`2h 14m`, short age form; label changes with state; `Unknown` when UPower reports 0), Charge cycles (`312`), Serial, Type (`Lithium ion`), Powering system (`Yes`/`No`), Technology, Capacity (`92.0%`), Energy empty, Energy full, Energy full (design), Voltage min (design), Voltage max (design), Charge threshold (`Enabled`/`Disabled`), Charge start threshold (`75%`), Charge end threshold (`80%`), Temperature. Threshold rows hidden when unsupported.
- **Data binding**: UPower properties through `Snapshot.system.batteries[idx]` (`SYSTEM`) and series `bat.power` / `bat.percent`; the week history is a separate `Arc<[Sample]>` on the battery record refreshed every 10 min. Demand `sensors` (batteries ride on the sensors demand bit, chosen; the engine's UPower worker is event-driven anyway).
- **Build**: `pages/performance/sensors.rs` `BatteryPage`. The week chart is a `ChartView` with `range_ns = 7 days` and `smooth = false` (it never slides).
- **Tests**: `battery_graph_kind_by_type`, `battery_time_label_follows_state`, `week_chart_hidden_under_2_points`.
- **Phase**: P2 (P3 history).
- **Decided here**: symmetric chart fill rule, week chart labels, demand bit.

### UI-PRF-036 Aggregate pages (P4)

- **Implements**: PERF-009, PERF-008 ("Aggregate", "Multiple").
- **Anatomy**: "Aggregate storage": header "Aggregate storage" + "3 disks"; graphs Active time (busiest disk, Fixed 0..100) and Transfer rate (sum, AutoRate IEC) as UI-PRF-029; Details rows: Active time, Read speed, Write speed, Total read, Total written, Capacity (sum), Used space, Free space, Disks (`nvme0n1, nvme1n1, sda`), Type (`Multiple` when kinds differ). "Aggregate graphics": header "Aggregate graphics" + "2 adapters"; graphs Utilization (highest engine, Fixed) and Memory (VRAM sum, FixedTotal); Details rows: Utilization, Dedicated memory (sum), Adapters (names), Type (`Multiple` when vendors differ). Both exist only with two or more devices and are hidden by default (`device_overrides` keys `aggregate:disk`, `aggregate:gpu`).
- **Build**: `pages/performance/aggregate.rs`.
- **Tests**: `aggregate_exists_only_with_two_devices`, `aggregate_type_multiple`.
- **Phase**: P4.
- **Decided here**: aggregate Details rows.

---

## Part D: minigraphs, detached graph windows and the mini window

### UI-PRF-040 Minigraph panel

- **Implements**: PERF-080, PERF-082, R-089, APP-021 (View > Graphs items are the shell's; their effect is here).
- **Anatomy** (over the Processes tab table, anchored bottom right; measured from `advanced-12.png`: cards 180 × 90 logical, 8 px apart, 16 px from the right and bottom edges of the table area):

```
                                                 table area
                                      ┌ card ──────────┐ ┌ card ──────────┐
                                      │ Network 0.07 Mbps│ │ Disk   0.54 MiB/s│   row 1 (filled from the bottom row upward)
                                      └────────────────┘ └────────────────┘
                                      ┌ card ──────────┐ ┌ card ──────────┐
                                      │ Memory   40.5% │ │ CPU      88.6% │   row 0 (bottom)
                                      └────────────────┘ └────────────────┘ sp6 16
                                                                          sp6 16
slot numbering: slot 0 = bottom right, 1 = left of it, …, up to 4 per row, then the row above; at most 3 rows × 4 columns = 12 cards.
```

  - **Default cards and slots** (PERF-080): CPU slot 0, Memory slot 1, Disk slot 2, Network slot 3, GPU slot 4, VRAM slot 5; GPU temp off by default (slot 6 when enabled). In `advanced-12.png` the user's layout is CPU 0, Memory 1, Disk 2, Network 3.
  - The panel is a `div().absolute()` overlay in the Processes page, `pointer-events` only on the cards (the gaps pass clicks to the table).
  - Panel visibility: only on the Processes tab (also in History mode). Hidden during Ctrl-pause? No: the cards keep sliding (they draw from live series, not the paused table snapshot; chosen).
- **States**: panel empty (every card hidden): nothing is drawn. Full grid: adding shows the toast "No space for more minigraphs". Reset: toast "Minigraph layout reset".
- **Interaction**: View > Graphs check items (shell) dispatch `MinigraphToggleSystemCard { source }`; "Reset Minigraph Layout" dispatches `MinigraphResetLayout` (restores the six default cards and slots, removes process cards, resets range and colour per card; chosen: a reset means the default state). Card interactions in UI-PRF-041 to UI-PRF-043.
- **Data binding**: `state.toml` `[[minigraphs]]` entries with `detached = false` and settings `graphs.minigraph_*`. Demand: the panel adds `cpu: Total`, `memory`, `disks`, `nets`, `gpus` for the system cards it shows; process cards add their identities (PERF-085).
- **Build**: `pages/minigraph/panel.rs` `MinigraphPanel` entity owned by the Processes page (processes spec mounts it; this spec owns it). Cards are `MinigraphCard` entities in `IndexMap<MinigraphId, Entity<MinigraphCard>>`; `MinigraphId` is the index in `[[minigraphs]]` kept stable through a `u32` id assigned at load.
- **Performance**: 6 cards at 1 Hz, CPU at 10 Hz when `cpu_fast` (the Processes tab does not set `cpu_fast`; the CPU card runs at 1 Hz there, so the CPU minigraph slides at 1 Hz data, matching the ADR-008 schedule).
- **Accessibility**: cards are not focusable; every card's value is text; the same series are reachable on the Performance tab.
- **Tests**: `panel_slots_fill_bottom_right_first`, `panel_full_toast`, `panel_reset_restores_defaults`.
- **Phase**: P1 (P2 drag and slots beyond the default six, P3 process cards).
- **Decided here**: margins, slot order, reset semantics, behaviour under Ctrl-pause.

### UI-PRF-041 Minigraph card

- **Implements**: PERF-080, PERF-085, PERF-086, PERF-087, PERF-024 (colour), R-066.
- **Anatomy** (`mini_card` 180 × 90 × ui_scale):

```
┌────────────────────────────────────────┐  bg list_bg, 1 px divider, r_md, drop shadow
│ CPU                             88.6%  │  label t_sm 500 text_primary left; value t_sm 400 tnum right; row 20, inset sp3
│ ▂▃▅▇▇▅▃▂▁▂▃▅▇▇▆▅▃▂▁▁▂▃▅▇▇▆▅▃▂▁▂▃▅▇▇▅▃ │  chart fills the whole card under the label row (labels overlay the plot top)
└────────────────────────────────────────┘
```

  - **Shadow**: `BoxShadow { offset (0, 4), blur 12, spread 0, color black @ (0.35 × shadow_intensity_pct / 100) }` (chosen; "Drop Shadow Intensity" is `appearance.shadow_intensity_pct`). Painted with `paint_drop_shadows` by the card wrapper.
  - **Chart**: UI-PRF-001 with `heading: None`, value drawn by the card (not the chart), no legend, no range label, no axis max, grid on, outline on. Series per source:

| `source` | Label | Series | Scale | Value |
|---|---|---|---|---|
| `cpu` | "CPU" | total filled + kernel line `#C1514E` | Fixed | `88.6%` |
| `memory` | "Memory" | used | FixedTotal | `40.5%` |
| `disk` | "Disk" | busiest disk active time | Fixed | `0.54 MiB/s` is TaskSlinger's; tasklens shows the aggregate transfer rate as the value and active time as the graph (PERF-080 says Disk = aggregate active time; the reference frame shows a rate). Value = sum of transfer rates, graph = active time (chosen; both facts shown) |
| `network` | "Network" | sum over visible physical interfaces, download filled, upload dashed | AutoRate / LinkSpeed | total rate `0.07 Mbps` |
| `gpu`, `gpu:<key>` | "GPU" | 3D/Compute of that GPU | Fixed | percent |
| `vram`, `vram:<key>` | "VRAM" | dedicated memory | FixedTotal | `2.80 GiB` |
| `gpu_temp`, `gpu_temp:<key>` | "GPU temp" | temperature | StickyTemp | `52 °C` |
| `process:…` | `<name> (<pid>) · <col>` | the process's column series (engine-sampled) | by column | column value |
| `name:<image>` | `<name> by name · <col>` | sum over matching processes | by column | value |
| `app:<id>` | `<App> by app · <col>` | app cgroup sum | by column | value |

  - Column labels for process cards (PERF-087): `CPU`, `Memory`, `RSS`, `RSS shared`, `Disk`, `Network`, `GPU`, `VRAM`, `GPU shared`. The label truncates with an ellipsis in the middle of the name part when too long, keeping the ` · <col>` suffix (chosen).
  - `Process <pid>` is the name until it has been read (PERF-085).
  - **CPU card tooltip** (PERF-080): hovering the card shows a `TlTooltip` with two lines "── Total 88.6%" and "── Kernel 4.1%" (swatches as in UI-PRF-003).
  - **Colour**: `color = accent` → the type palette for system cards (CPU uses `cpu_*`, Memory `mem_*`, …) and `TlTheme.accent` for process cards (chosen: the spec's "Accent" for a system card means the default type colour, so TaskSlinger's defaults are kept); a named colour overrides both fill (α 0.85) and line.
- **States**:
  - Default, hover (border `accent` @ 0.5; chosen), dragging (UI-PRF-043).
  - "No match" (PERF-086): value text `No match` in `text_secondary`; the graph records a gap.
  - "Ended" (PERF-086): value text `Ended` in `text_secondary`; the graph stops at its last sample; the card stays until "Remove graph".
  - Restricted field (other user's process): value shows the lock glyph; the graph shows a gap.
  - Boot ID mismatch after restart: "Ended" (R-100).
- **Interaction**: right-click → UI-PRF-042. Drag → UI-PRF-043. Hover → CPU tooltip or the P2 hover readout (UI-PRF-004) on every card.
- **Data binding**: series by `source`; the engine adds demand for process identities (PERF-085). Re-render on `SERIES` for its ids, `PROCESSES` for name and exit state of process cards.
- **Build**: `pages/minigraph/card.rs` `MinigraphCard` entity with one `ChartView`; label and value are `ShapedLine`s cached in the card.
- **Tests**: `card_labels_system`, `card_labels_process` (three forms), `card_no_match_and_ended_states`, `card_color_named_overrides`, `card_ended_on_boot_id_mismatch`.
- **Phase**: P1 (system cards); P3 (process cards, colour).
- **Decided here**: shadow, hover border, Disk card value, label truncation, accent meaning for system cards.

### UI-PRF-042 Minigraph card context menu

- **Implements**: PERF-081, PERF-082 (Reset), PERF-083 (Detach/Attach), PERF-088 (tray).
- **Anatomy**: `TlMenu`, items in order:
  1. "Hide graph" → `MinigraphHide { id }` (system cards: turns the View > Graphs check off; process cards: same as Remove, item hidden on them)
  2. "Remove graph" → `MinigraphRemove { id }` (process-source cards only)
  3. "Time range >" → radio "30 seconds", "1 minute", "5 minutes", "10 minutes" → `MinigraphSetRange { id, secs }` (5 and 10 minutes disabled before P2)
  4. "Graph color >" → radio "Accent", then the 17 APP-039 names, each with a 12 px colour dot before the label (`PopupMenuItem` with `.icon` replaced by a coloured square; chosen) → `MinigraphSetColor { id, color }` (P3)
  5. separator
  6. "Detach" (attached cards) or "Attach to main window" (detached cards) → `MinigraphDetach { id }` / `MinigraphAttach { id }` (P3)
  7. "Add tray icon graph" → `PerfAddTrayGraph { source }` (P3; disabled with tooltip "No system tray available" when no watcher)
  8. separator
  9. "Reset minigraph layout" → `MinigraphResetLayout`
- **Tests**: `card_menu_order_system`, `card_menu_order_process`.
- **Phase**: P1 items 1, 3, 9; P3 the rest.
- **Decided here**: separators, colour dots, Hide hidden on process cards.

### UI-PRF-043 Move and reorder cards

- **Implements**: PERF-082, PERF-083 (drag past the edge detaches).
- **Behaviour**: mouse-down on a card (not on the label text, anywhere else) starts a GPUI drag with payload `DraggedMinigraph { id }` after 4 px of movement. The card follows the pointer at opacity 0.85 with its shadow; its home slot shows an empty outline (1 px `accent` dashed, `r_md`). Moving over another slot (pointer inside that slot's rect) previews the shift: other cards move to make room, animated over `m_fast` (reduced motion: instant). Drop → `MinigraphMoveToSlot { id, slot }`; cards shift to fill. Dropping on an empty slot beyond the last card places it in the next free slot (no holes; chosen). Dragging the pointer past the main window's edge by more than 24 px and releasing detaches the card (P3): `MinigraphDetach { id }` with the new window placed under the pointer where the compositor allows (KDE through the bridge; else compositor placement).
- **Tests**: `drag_shifts_cards`, `drag_no_holes`, `drag_past_edge_detaches`.
- **Phase**: P2 (P3 detach by drag).
- **Decided here**: 4 px threshold, ghost opacity, 24 px edge margin, no-holes rule.

### UI-PRF-044 Detached graph windows

- **Implements**: PERF-083, PERF-084, R-084, R-086.
- **Anatomy**:

```
┌ window "tasklens Graph 1", borderless, Floating, client decorations, no title bar ┐
│ sp2                                                                               │
│ ┌ card 180 × 90 ─┐                                                                │
│ └────────────────┘                                                                │
│ sp2                                                                               │
│ ┌ card 180 × 90 ─┐   (stacked vertically when "Add graph >" adds more)            │
│ └────────────────┘                                                                │
└───────────────────────────────────────────────────────────────────────────────────┘
window size = 180 + 2 sp2 wide, n × 90 + (n + 1) sp2 high, × ui_scale; saved in [[detached_windows]] width, height
background window_bg; 1 px divider border; no shadow (the compositor's); the cards drop their own shadow here (chosen)
```

- **States**: kept above other windows on KDE (`keepAbove` through the bridge), position restored on KDE only. The window is not resizable (`is_resizable: false`). Minimising the main window does not affect it. With `graphs.keep_detached_on_close` on, closing the main window hides it and the app keeps running while any detached window is open (PERF-084).
- **Interaction**: dragging anywhere on the window body (including over a card, with no 4 px threshold here) calls `window.start_window_move()`. Right-click → `TlMenu`: "Show Main Window" → `ShowMainWindow`; "Close graph" → `MinigraphCloseDetachedWindow { window }` (closes the window and re-attaches every card in it to the main panel; PERF-083 keeps "Remove graph" as the destructive item, so closing never loses a card, chosen); "Attach to main window" → `MinigraphAttach { id }` for the clicked card; "Add graph >" → submenu of the system card sources not already in this window ("CPU", "Memory", "Disk", "Network", "GPU", "VRAM", "GPU temp") → `MinigraphAddToWindow { window, source }`; "Remove graph" → `MinigraphRemove { id }` for the clicked card (removes it from the layout entirely); "Exit tasklens" → `Quit`. Each card also keeps the UI-PRF-042 items "Time range >" and "Graph color >" between "Attach to main window" and "Add graph >" (chosen).
- **Data binding**: `[[minigraphs]]` entries with `detached = true` and `window = n`; `[[detached_windows]]` for size and KDE position. Demand continues from these cards while the main window is hidden (ADR-008).
- **Build**: `windows/detached_graph.rs` `DetachedGraphWindow` entity opened with `WindowOptions { kind: WindowKind::Floating, window_decorations: Some(Client), titlebar: None, is_resizable: false, is_movable: true, app_id: Some("io.github.oysteinkrog.Tasklens"), window_background: Opaque, .. }` and `window.set_window_title("tasklens Graph <n>")`. Its own `AnimationDriver`. The KWin bridge worker finds it by pid and caption (R-086) and applies `keepAbove` and `frameGeometry`; moves are read back from `WindowsSnapshot` (`WINDOWS` bit) and saved debounced 500 ms.
- **Tests**: `detached_window_size_for_two_cards`, `close_graph_reattaches`, `keep_detached_on_close_keeps_app`, `detached_state_round_trip`.
- **Phase**: P3.
- **Decided here**: padding `sp2`, not resizable, card shadow inside, "Close graph" semantics, extra per-card items.

### UI-PRF-045 Performance mini window

- **Implements**: PERF-093, R-058, R-073 (pin semantics for "Always on top").
- **Anatomy**:

```
┌ window "tasklens Mini Window", borderless Floating, width 260 ─┐
│ sp4                                                            │
│ ┌ UI-PRF-011 card, width 260 − 2 sp4 = 244, height 80 ───────┐ │  one card per visible sidebar card, in sidebar order
│ └────────────────────────────────────────────────────────────┘ │
│ sp4 gap                                                        │
│ ┌ card ──────────────────────────────────────────────────────┐ │
│ ...                                                            │
│ sp4                                                            │
└────────────────────────────────────────────────────────────────┘
height = n × 80 + (n + 1) × sp4, capped at 90% of the output's work area height; beyond that the list scrolls (kit scrollbar)
bg window_bg; 1 px divider border; not resizable
```

  Cards are the same `DeviceCard` as the sidebar with these differences: no selected border (there is no selection), the hover state shows a `pointer` cursor and `row_hover` bg, and the heading row gains no extra controls.
- **States**: "Always on top" on by default when opened; not persisted (PERF-093). On KDE via the bridge; elsewhere, X11 included, the menu item is disabled with tooltip "Not supported by this desktop" (R-177).
- **Interaction**: open with View > Window > "Performance mini window" (check item, shell), Ctrl+Shift+M (`ToggleMiniWindow`), the tray item "Show mini window", or a double-click on the Performance graph area. Click on a card → `ShowMainWindow` then `PerfSelectDevice { key }` (opens the main window on that page; the mini window stays open, chosen). Double-click on the window background or a card → `ShowMainWindow`. Right-click → `TlMenu`: "Always on top" (check) → `window::ToggleMiniAlwaysOnTop`; "Show Main Window"; "Close" → `window::CloseMiniWindow`. Closing unchecks the View menu item. Esc closes it (chosen; it is a secondary window and `Esc` closes dialogs by the global rule).
- **Data binding**: visible sidebar cards and their series; demand continues while the main window is hidden (PERF-093).
- **Build**: `windows/mini.rs` `MiniWindow` entity with its own `AnimationDriver`; caption "tasklens Mini Window" for the bridge match (R-086).
- **Tests**: `mini_window_lists_visible_cards`, `mini_window_card_click_opens_main_on_page`, `mini_window_close_unchecks_menu`.
- **Phase**: P3.
- **Decided here**: card width, height cap and scroll, Esc closes, mini window stays open on card click.

---

## Part E: tray graph icons, the Additional Setup rows and the bench scenes

### UI-PRF-050 Tray graph icon rasterizer

- **Implements**: PERF-088, PERF-091, PERF-092 (re-register keeps the same pixels), R-027, ADR-010 tray paragraph.
- **Anatomy** (22 × 22 px; the 44 × 44 icon is the same drawing at 2× with every size doubled):

```
col:  0 1 2 ... 18 19 20 21
row 0 . . . . . . . . . . .   row 0 and row 21: transparent margin (1 px)
row 1 . ▁ ▁ ▂ ▂ ▃ ▂ ▁ ▁ █ .   columns 1..=18: history, one column per bucket, newest at column 18
...   . ▂ ▃ ▅ ▇ ▆ ▅ ▃ ▂ █ .   columns 19..=20: the current-value bar, 2 px wide, full line colour
row 20. █ █ █ █ █ █ █ █ █ █ .
row 21. . . . . . . . . . .
```

  - **Background**: fully transparent (ARGB 0).
  - **History columns** (`cols = 18` at 22 px, 36 at 44 px): the series over `range_s` is resampled into `cols` buckets aligned to absolute time (bucket max, same rule as UI-PRF-007); each column is filled from the bottom to `height × value / max` with the fill colour (line colour at alpha 0.55), and its top pixel row carries the line colour at alpha 1.0 (the "1 px line", ADR-010). Gaps and invalid buckets are left transparent.
  - **Value bar**: the two right-most inner columns (four at 44 px) are a solid bar in the line colour at alpha 1.0 from the bottom to the current value (PERF-088 "current value as a filled bar height"), separated from the history by nothing (adjacent); when the value is `WarmingUp` or invalid the bar is drawn as a 1 px baseline only.
  - **Supersampling**: each icon is rasterized at 2× (44 × 44 for the 22 px icon, 88 × 88 for the 44 px icon) and box-filtered down, which anti-aliases the column tops (ADR-010: "no anti-aliasing beyond 2×2 supersampling").
  - **Colour**: the graph's `color` (accent or named, APP-039) resolved through UI-PRF-008; `accent` on a tray graph means the type palette line colour for a page source (`cpu_line` for CPU), `TlTheme.accent` for process sources (same rule as UI-PRF-041).
  - **Scale**: as the matching chart (Fixed for percent, FixedTotal for memory, AutoRate for rates with the same 10 s fall rule, kept in the tray graph's own `ScaleState`).
- **Update rule**: the `tl-tray` thread receives a snapshot pointer from a 1 Hz GPUI timer (`EngineClient::latest()`), rasterizes each graph, compares the new ARGB buffer with the previous one byte for byte, and calls `ksni` `set_icon` only when it differs (PERF-088, ADR-010). With a 1 Hz source every second changes the newest column, so in practice the icon updates once per second while the value moves and not at all when the series is flat (`WarmingUp`, "Ended", idle network at exactly 0).
- **States**: "No match" and "Ended" process sources (PERF-086): the icon freezes on its last pixels; the tooltip (UI-PRF-051) carries the state word. Probe or source `Failed`: the icon shows the baseline only.
- **Build**: `crates/tl-tray/src/raster.rs`: `pub fn render(series: &[Sample], now_ns: u64, range_ns: u64, scale: &mut ScaleState, color: Rgba, size: u32, out: &mut Vec<u8>)`, pure CPU, no allocation after the first call (`out` is reused; the 2× buffer is a `thread_local!`). ARGB32 in network byte order as `ksni::Icon { width, height, data }` expects. The tray thread owns one `Vec<u8>` per icon size per graph.
- **Performance**: 22 px icon: 44 × 44 × 4 bytes = 7.7 KiB at 2×; 44 px icon 31 KiB. Both per graph per second, well under 50 µs on this machine. Nothing touches the GPUI thread except the 1 Hz timer that hands over the `Arc<Snapshot>`.
- **Accessibility**: the tooltip repeats the value as text (UI-PRF-051).
- **Tests**: `raster_22_has_transparent_margin`, `raster_value_bar_height`, `raster_identical_input_identical_bytes` (update rule), `raster_gap_columns_transparent`, `raster_44_is_2x_of_22_layout`.
- **Phase**: P3.
- **Decided here**: margin 1 px, history 18 columns, value bar 2 px, fill alpha 0.55, baseline for invalid, the accent meaning for page sources.

### UI-PRF-051 Tray graph tooltips and menus

- **Implements**: PERF-089, PERF-090, PERF-091, R-027, R-067.
- **Tooltip** (SNI `ToolTip` `(icon, title, description)`, `NewToolTip` at most 1 Hz, only when the text changed): title is the graph title, description the value text:

| Source | Title | Description |
|---|---|---|
| page graph | `<Title> Usage` ("CPU Usage", "Memory Usage", "Disk Usage", "Network Usage", "GPU Usage") | `18.6%` (so the tooltip reads "CPU Usage: 18.6%" in panels that join title and description with ": "; panels that show two lines show them as two lines) |
| process | `<name> (PID n)` | `CPU 3.2%` |
| name filter | `3 procs` | `firefox` (reads "3 procs: firefox") |
| app | `<App>` | `CPU 3.2%` |
| main icon (APP-047) | `tasklens` | five lines "CPU: 11.9%", "RAM: 54.9%", "GPU: 3.0%", "Disk: 1.2%", "Network: 0.3 Mbps" |

  "No match" and "Ended" replace the value part (`firefox: No match`).
- **Menu** (dbusmenu on `/MenuBar`, PERF-090 order):
  1. "Graph color >" → radio: "Accent", then the 17 names (checked = current) → `tray::SetColor { n, color }`; the icon redraws at once (next tick, ≤ 1 s).
  2. "Time range >" → radio "30 seconds", "1 minute", "5 minutes", "10 minutes" → `tray::SetRange { n, secs }`.
  3. "Close tray icon graph" → `tray::Remove { n }`; toast in the main window "Tray icon graph removed".
  4. "Tray icon graphs >" → check items for the page sources "CPU", "Memory", "Disk", "Network", "GPU", "VRAM", "GPU temp" (checked = a tray graph with that source exists) → `tray::ToggleSource { source }`; process-source graphs are listed below a separator by their label with a check and can be unchecked (removed) here too.
  5. separator
  6. "Show Main Window" → `ShowMainWindow`
  7. "Show mini window" → `ToggleMiniWindow`
  8. "Exit tasklens" → `Quit`
  The main tray icon menu (APP-047): "Show Main Window", "Tray icon graphs >", separator, "Exit tasklens". While any graph icon exists the main icon is not registered (PERF-091).
- **Toasts** (`ToastQueue` of the main window; UI-FND-020 rule 8 applies while it is hidden, R-161): "Tray icon graph added", "Tray icon graph removed", "Tray icon graph already exists" (adding a source that exists), "Unable to add tray icon graph" (registration failed; the item stays out of `[[tray_graphs]]`).
- **Left click** (SNI `Activate`): toggles the main window (`ToggleMainWindow`).
- **Re-registration** (PERF-092): on `NameOwnerChanged` for `org.kde.StatusNotifierWatcher` the tray thread calls `RegisterStatusNotifierItem` for every item again with its last rendered icon, within 1 s.
- **Build**: `crates/tl-tray/src/{lib.rs, item.rs, menu.rs}`; actions are forwarded to the GPUI thread through an `async_channel::bounded(16)` and dispatched as GPUI actions on the main window, or on the app when the window is closed.
- **Tests**: `tray_tooltip_forms`, `tray_menu_order_graph_icon`, `tray_menu_order_main_icon`, `tray_main_icon_hidden_with_graphs`, integration `tray_reregisters_on_watcher_restart` (fake watcher drops and reappears).
- **Phase**: P3.
- **Decided here**: title/description split of the tooltip, process entries in "Tray icon graphs >", toast delivery when hidden.

### UI-PRF-052 Additional Setup window: rows and states

- **Implements**: PERF-105, PERF-106, PERF-107, PERF-108, R-028, R-029. The window frame (title "tasklens Additional Setup", size, pin, close) is a `TlSecondaryWindow` tool window (UI-FND-021, `WindowKey::AdditionalSetup`); the content below is this spec's.
- **Anatomy** (content width 560, padding `page_pad`; each row is a card):

```
┌ row card: bg list_bg, 1 px divider, r_md, padding sp5 ─────────────────────────────────────┐
│ CPU power draw                                           [Enabled (existing rule)] (TlBadge)  │  title t_md 500; status tag right
│ Reads the CPU energy counters (RAPL) for the Power draw row and graph.   t_sm text_secondary│  one or two lines of description
│ sp4                                                                                          │
│ [Enable]  [View script]  [Copy command]                                 buttons left, sp4 gap│  TlButton small: Enable/Revert primary when it applies, others outline
└──────────────────────────────────────────────────────────────────────────────────────────────┘
sp4 between cards
```

  Rows, in order, with their statuses and buttons:

| Row | Description text | Status values (TlBadge) | Buttons |
|---|---|---|---|
| CPU power draw | "Reads the CPU energy counters (RAPL) for the Power draw row and graph." | `Enabled` (success), `Enabled (existing rule)` (success, tooltip "Readable through /etc/udev/rules.d/99-powercap.rules. tasklens leaves that file alone."), `Not enabled` (warning), `Not needed` (neutral; no RAPL zone, buttons hidden) | "Enable" (when Not enabled) or "Revert" (when Enabled by the tasklens rule; hidden for an existing rule), "View script", "Copy command" |
| Journal access | "Lets tasklens read service logs. Run this command, then log out and back in:" followed by a code line `sudo usermod -aG systemd-journal $USER` (mono, `t_sm`, bg `input`, padding sp3, r_sm) | `Enabled` (user is in `systemd-journal`), `Not enabled` | "Copy" (copies the command). No Enable: tasklens never changes groups. |
| Administrator helper | "Needed for actions on other users' processes and for protected reads." | `Installed` (success), `Not installed` (warning, tooltip "/usr/lib/tasklens/tl-helper was not found") | none |
| Fan sensors (P4) | "Finds motherboard sensor chips and loads their drivers." | `Not needed` (any hwmon fan exists), `Not enabled`, `Enabled`; when lm_sensors is missing the status is `Not available` and the description reads "Install lm_sensors to detect fan sensors" | "Run sensor detection" (→ `RunSetupStep { step: SensorsDetect }`), "Revert", "View script", "Copy command" |
| Per-process network | "tasklens attributes TCP traffic per process through sock_diag without setup. Exact numbers for all traffic need the network helper, which arrives in a later version." | `Not needed` | none |

  - "Enable" on CPU power draw first opens a `TlDialog` confirm: title "Enable CPU power draw?", body "Lets every local user read CPU energy counters. This can leak information about other programs (CVE-2020-8694).", buttons "Enable" (primary) and "Cancel" (PERF-106). "Revert" opens: "Revert CPU power draw?", body "Removes the tasklens udev rule. A reboot may be needed before the files return to mode 0400.", buttons "Revert" (danger) and "Cancel".
  - "View script" opens a read-only `TlSecondaryWindow` tool window (640 × 480) titled "tasklens-setup <subcommand>" with the script text in a mono scroll area and a "Copy" button. "Copy command" copies `pkexec /usr/lib/tasklens/tasklens-setup rapl-enable` (or the row's subcommand) and shows the toast "Command copied".
  - **Running**: the row's buttons are disabled, the status tag shows a 12 px spinner icon and the text `Running…`; the pkexec prompt is the system's.
  - **Success**: status updates, a line under the buttons reads "Done. No restart needed." in `status.success`, and the engine re-probes the source at once (`Command::ReprobeSource(SourceId::Cpu)`), so the Power draw row changes within 2 s.
  - **Failure**: a block under the buttons: "Setup failed" (`t_sm` 500 `status.danger`), the script output in a mono scroll area (max height 160), buttons "Copy output" and "Retry". A cancelled pkexec prompt (exit 126 or 127) shows no failure block; the row keeps its old status (PERF-105).
- **Interaction**: Tab order follows the rows top to bottom, buttons left to right. Esc closes the window (shell). Opened from Help > "Additional setup…", Settings > Privileges, and every `Needs setup` link (`OpenAdditionalSetup { focus_row: Option<SetupRow> }` scrolls that row into view and flashes its border `accent` for `m_slow`, reduced motion: no flash; chosen).
- **Data binding**: `Snapshot.health` (`HEALTH` bit) for the RAPL readability and helper presence; a one-shot `Command::SetupStatus` query on open for the journal group and lm_sensors presence (the engine answers with a `SetupStatus` struct; chosen so the UI reads no system files itself, ADR-001 rule).
- **Build**: `windows/setup.rs` `SetupWindow` entity; rows are `RenderOnce` `SetupRow`s; the script runs through `Command::RunSetup { step }` in the engine's blocking pool with `pkexec`, streaming stdout and stderr lines back as `SetupEvent`s.
- **Tests**: `setup_rows_order_and_statuses`, `setup_enable_confirm_text`, `setup_cancelled_pkexec_keeps_status`, `setup_failure_block_shows_output`.
- **Phase**: P3 (Fan sensors row P4).
- **Decided here**: card layout, description sentences, tag colours, confirm texts beyond the spec's warning sentence, "Command copied" toast, focus-row flash, `SetupStatus` command.

### UI-PRF-053 Bench scenes: perf-cpu, bridge and static

- **Implements**: ADR-014 "Phase-0 fixture" and gates; ADR-009 verification.
- **Common fixture**: window 1600 × 1000 logical, opened on the 144 Hz output at 110%, focused, `motion.smooth_graphs = true`, theme Default Dark, `ui_scale_pct = 100`, `graph_range_s = 60`. Synthetic data from `tl-ui/src/bench/` with PRNG seed 42 (an in-tree SplitMix64 plus Box-Muller in `bench/synth.rs`, no PRNG crate; deterministic across runs; R-142). The bench harness replaces the real `EngineClient` with `BenchEngine` (ADR-013 fake engine with a scripted publisher) except in `bridge`. The Processes tab and all other tabs are not built in these scenes (they are never visited).
- **Scene `perf-cpu`**, exact composition:
  - Performance tab, CPU page selected, sidebar visible at `nav_width = 252`, `cpu_graph_mode = "overall"` is **not** used: the scene shows the per-core grid **and** the overall chart at once. To get both, the bench sets a scene flag `BenchScene::PerfCpu` that renders the CPU page with the total CPU chart (W × 300, fill + line + kernel line) above the 32-core grid (8 × 4, kernel overlays on). This is a bench-only layout (`CpuPage.bench_show_both`), not a user setting.
  - Synthetic CPU: 32 logical CPUs, 10 Hz samples; per core `value = clamp(base_c + 25 sin(2π t / P_c) + noise, 0, 100)` with `base_c` in 5..60, `P_c` in 3..17 s, Gaussian noise σ 4; kernel = 0.2 × value + noise σ 1; total = mean of cores. 60 s range (600 samples per series) pre-filled before the first frame so decimation and the geometry rebuild run from frame 1.
  - Device sidebar with 9 cards: CPU, Memory, GPU 0, Disk 0, Disk 1, Disk 2, Disk 3, enp38s0, tailscale0; each sidebar mini graph at 1 Hz synthetic data (memory 55% ± 1 random walk; GPU 10% ± 8; disks bursts 0..100% with p = 0.1 per second; networks 0.01..5 Mbps bursts). Value rules (R-160): "X ± N" is a fresh uniform draw each second; random-walk steps are uniform in [-1, 1] for memory and [-0.05, 0.05] for load average; a network burst happens with p = 0.1 per second, else the value is 0.01 Mbps; the placeholder catalog is 16 cores × 2 threads, disks 259:0 to 259:3, ifindex 2 and 3. The CPU card's graph receives the 10 Hz total.
  - Stats block: the Details grid with 12 live values (rows 1 to 12 of UI-PRF-024; rows 13 to 26 static) updated at 1 Hz from the synthetic source (Processes 295 ± 3, Threads 5 596 ± 20, Handles 253 248 ± 500, Up time counting, Speed 4631 ± 200 MHz, Temperature 52 ± 2 °C, Load average random walk).
  - Per-core percent labels change at 10 Hz (every sample re-formats the value).
  - Duration 30 s after a 5 s warm-up (`--duration 30 --warmup 5`). Measures per ADR-014: UI thread CPU per frame (p50, p99), missed frames, process CPU, RSS. Gates: p50 ≤ 1.0 ms, p99 ≤ 2.0 ms, 0 missed frames in the median run, ≤ 30% of one core.
- **Scene `bridge`**: identical to `perf-cpu`, but the total CPU series (the overall chart and the CPU sidebar card) comes from the real `tl-engine` `Cpu` worker at 10 Hz through the real `EngineClient`. The 32 per-core series stay synthetic (the machine has 16 logical CPUs). Extra measure: wake-to-render latency, timestamped from `Snapshot.published_ns` to the first `ChartElement::paint` that includes that sample; gate p99 ≤ 20 ms. The bench verifies that 10 Hz samples arrive (≥ 295 samples in 30 s).
- **Scene `static`**: `perf-cpu` with `motion.smooth_graphs = false`. Charts redraw once per sample (10 Hz for the CPU series, 1 Hz for the rest); the animation driver must request no frames (asserted: `FrameStats.requested == 0`). Gate: process CPU ≤ 2% of one core.
- **Outputs**: `--json <out>` writes `{ scene, frames, frame_cpu_ms: { p50, p99, max }, missed_frames, long_missed_frames, process_cpu_pct, probe_cpu_pct, rss_mb, pss_mb (MB = 1,000,000 bytes), stalls, warmup_stalls (stalls during the warm-up, kept apart from `stalls`), window_active (the bench window was active at the start and end of the measured window; bench windows set `inactive_frame_interval: None`, so an inactive window is not capped at 30 fps), bridge_latency_ms: { p50, p99 } | null, gpu: { power_w, util_pct } | null }`. `cargo xtask bench` runs each scene 3 times and writes `target/bench/<date>-<git sha>.json` plus a table in `docs/spike/phase0-results.md`.
- **Build**: `tl-ui/src/bench/{mod.rs, scenes.rs, synth.rs, stats.rs}` behind `--features bench`; `tl-app` parses `--bench <scene>` (APP-052 form per table c).
- **Tests**: `synth_is_deterministic` (two runs with seed 42 produce identical first 1,000 samples), `bench_scene_perf_cpu_builds_32_cells_and_9_cards` (render test counts elements).
- **Phase**: P0.
- **Decided here**: the bench-only "both charts" layout, the synthetic formulas, the sidebar device list, the latency measurement points, the JSON shape.

---

## Summary table

| ID | Name | Implements | Phase |
|---|---|---|---|
| UI-PRF-001 | ChartView and ChartElement | PERF-015, 016, 017, 020, 021, 022, 024, 026 | P1 |
| UI-PRF-002 | ChartGridElement | PERF-028, 063, 001 | P1 |
| UI-PRF-003 | Chart labels and legends | PERF-015, 016, 019, 026, 030 | P1 |
| UI-PRF-004 | Hover readout | PERF-023 | P2 |
| UI-PRF-005 | Smooth scrolling, animation driver, time mapping | PERF-020 | P1 |
| UI-PRF-006 | Y scale and axis rounding | PERF-018, 019, 030 | P1 |
| UI-PRF-007 | Decimation, gaps and validity on charts | PERF-017, 022 | P1 |
| UI-PRF-008 | Graph palette and per-card colour | PERF-024 | P1/P3 |
| UI-PRF-009 | Curved lines | PERF-021 | P4 |
| UI-PRF-010 | Performance tab layout | PERF-006, 007, 012, 014 | P1 |
| UI-PRF-011 | Device sidebar card | PERF-001, 005, 008, 013, 053, 054 | P1/P2/P4 |
| UI-PRF-012 | Sidebar list: selection, keys, hotplug, visibility | PERF-002, 004, 005, 007, 008 | P1/P2 |
| UI-PRF-013 | Sidebar edit mode | PERF-003, 008, 002 | P2 |
| UI-PRF-014 | Card context menu | PERF-002, 088 | P1/P3 |
| UI-PRF-015 | Page jump submenu and page context menu | PERF-007, 010, 011, 016, 088 | P1/P2/P3 |
| UI-PRF-016 | Graph summary mode | PERF-010 | P2 |
| UI-PRF-017 | Overview page | D-022 | P1 |
| UI-PRF-020 | Device page frame | PERF-008, 012, 014 | P1 |
| UI-PRF-021 | Details grid | PERF-012, 013 | P1 |
| UI-PRF-022 | Copy device info | PERF-011 | P2 |
| UI-PRF-023 | CPU page: header, modes, graph area | PERF-027, 028, 029, 030, 031, 033 | P1/P2 |
| UI-PRF-024 | CPU page: Details grid | PERF-033 to 040 | P1/P2 |
| UI-PRF-025 | CPU secondary graph | PERF-032, 038, 039, 040 | P2 |
| UI-PRF-026 | Memory page: graphs | PERF-041 | P1 |
| UI-PRF-027 | Memory composition bar | PERF-042 | P1 |
| UI-PRF-028 | Memory page: Details grid and blocks | PERF-043, 044, 045, 046 | P1/P2 |
| UI-PRF-029 | Disk page: graphs and header buttons | PERF-047, 048, 051, 052, 053 | P1/P2 |
| UI-PRF-030 | Disk page: Details, partitions, SMART, Disk in use | PERF-049, 050, 051, 052, 053 | P1/P2/P3 |
| UI-PRF-031 | Network page | PERF-054 to 061 | P1/P2/P3 |
| UI-PRF-032 | GPU page: graph rows | PERF-062 to 068 | P1/P2 |
| UI-PRF-033 | GPU page: Details grid and displays | PERF-064 to 073 | P1/P2 |
| UI-PRF-034 | Fan page | PERF-076, 077, 108 | P2/P3 |
| UI-PRF-035 | Battery and power device page | PERF-078, 079 | P2/P3 |
| UI-PRF-036 | Aggregate pages | PERF-009, 008 | P4 |
| UI-PRF-040 | Minigraph panel | PERF-080, 082 | P1/P2/P3 |
| UI-PRF-041 | Minigraph card | PERF-080, 085, 086, 087, 024 | P1/P3 |
| UI-PRF-042 | Minigraph card context menu | PERF-081, 082, 083, 088 | P1/P3 |
| UI-PRF-043 | Move and reorder cards | PERF-082, 083 | P2/P3 |
| UI-PRF-044 | Detached graph windows | PERF-083, 084 | P3 |
| UI-PRF-045 | Performance mini window | PERF-093 | P3 |
| UI-PRF-050 | Tray graph icon rasterizer | PERF-088, 091, 092 | P3 |
| UI-PRF-051 | Tray graph tooltips and menus | PERF-089, 090, 091, 092 | P3 |
| UI-PRF-052 | Additional Setup window: rows and states | PERF-105, 106, 107, 108 | P3/P4 |
| UI-PRF-053 | Bench scenes perf-cpu, bridge, static | ADR-014 | P0 |

## Coverage: merged spec IDs in scope to UI entries

PERF-094 to PERF-104 (Information tab) belong to the system tabs spec and are not listed. The AMD and Intel backends (PERF-074, PERF-075) have no UI of their own beyond the GPU page entries; they are mapped to those.

| PERF | UI entries |
|---|---|
| PERF-001 | UI-PRF-011, UI-PRF-002 |
| PERF-002 | UI-PRF-012, UI-PRF-013, UI-PRF-014 |
| PERF-003 | UI-PRF-013 |
| PERF-004 | UI-PRF-012 |
| PERF-005 | UI-PRF-011, UI-PRF-012 |
| PERF-006 | UI-PRF-010 |
| PERF-007 | UI-PRF-010, UI-PRF-012, UI-PRF-015 |
| PERF-008 | UI-PRF-011, UI-PRF-012, UI-PRF-013, UI-PRF-020, UI-PRF-036 |
| PERF-009 | UI-PRF-036 |
| PERF-010 | UI-PRF-015, UI-PRF-016 |
| PERF-011 | UI-PRF-015, UI-PRF-022 |
| PERF-012 | UI-PRF-010, UI-PRF-020, UI-PRF-021 |
| PERF-013 | UI-PRF-011, UI-PRF-021 |
| PERF-014 | UI-PRF-010, UI-PRF-020 |
| PERF-015 | UI-PRF-001, UI-PRF-003 |
| PERF-016 | UI-PRF-001, UI-PRF-003, UI-PRF-015 |
| PERF-017 | UI-PRF-001, UI-PRF-007 |
| PERF-018 | UI-PRF-006 |
| PERF-019 | UI-PRF-003, UI-PRF-006 |
| PERF-020 | UI-PRF-001, UI-PRF-005 |
| PERF-021 | UI-PRF-009 |
| PERF-022 | UI-PRF-001, UI-PRF-007 |
| PERF-023 | UI-PRF-004 |
| PERF-024 | UI-PRF-008, UI-PRF-041 |
| PERF-025 | UI-PRF-003 (`ValueFormat`), UI-PRF-031 (Display unit row) |
| PERF-026 | UI-PRF-001, UI-PRF-003 |
| PERF-027 | UI-PRF-023 |
| PERF-028 | UI-PRF-002, UI-PRF-023 |
| PERF-029 | UI-PRF-023 |
| PERF-030 | UI-PRF-003, UI-PRF-006, UI-PRF-023 |
| PERF-031 | UI-PRF-023 |
| PERF-032 | UI-PRF-025 |
| PERF-033 | UI-PRF-023, UI-PRF-024 |
| PERF-034 | UI-PRF-024 |
| PERF-035 | UI-PRF-024 |
| PERF-036 | UI-PRF-024 |
| PERF-037 | UI-PRF-024 |
| PERF-038 | UI-PRF-024, UI-PRF-025 |
| PERF-039 | UI-PRF-024, UI-PRF-025 |
| PERF-040 | UI-PRF-024, UI-PRF-025 |
| PERF-041 | UI-PRF-026 |
| PERF-042 | UI-PRF-027 |
| PERF-043 | UI-PRF-028 |
| PERF-044 | UI-PRF-028 |
| PERF-045 | UI-PRF-028 |
| PERF-046 | UI-PRF-028 |
| PERF-047 | UI-PRF-029 |
| PERF-048 | UI-PRF-029 |
| PERF-049 | UI-PRF-030 |
| PERF-050 | UI-PRF-030 |
| PERF-051 | UI-PRF-029, UI-PRF-030 |
| PERF-052 | UI-PRF-029, UI-PRF-030 |
| PERF-053 | UI-PRF-011, UI-PRF-030 |
| PERF-054 | UI-PRF-011, UI-PRF-012, UI-PRF-031 |
| PERF-055 | UI-PRF-003, UI-PRF-031 |
| PERF-056 | UI-PRF-031 |
| PERF-057 | UI-PRF-031 |
| PERF-058 | UI-PRF-031 |
| PERF-059 | UI-PRF-031 |
| PERF-060 | UI-PRF-031 |
| PERF-061 | UI-PRF-031 |
| PERF-062 | UI-PRF-032 |
| PERF-063 | UI-PRF-002, UI-PRF-032 |
| PERF-064 | UI-PRF-032, UI-PRF-033 |
| PERF-065 | UI-PRF-032, UI-PRF-033 |
| PERF-066 | UI-PRF-032, UI-PRF-033 |
| PERF-067 | UI-PRF-032, UI-PRF-033 |
| PERF-068 | UI-PRF-032, UI-PRF-033 |
| PERF-069 | UI-PRF-033 |
| PERF-070 | UI-PRF-033 |
| PERF-071 | UI-PRF-033 |
| PERF-072 | UI-PRF-033 |
| PERF-073 | UI-PRF-033 |
| PERF-074 | UI-PRF-032 (engine tooltip "Own processes only"), UI-PRF-033 |
| PERF-075 | UI-PRF-032, UI-PRF-033 |
| PERF-076 | UI-PRF-034 |
| PERF-077 | UI-PRF-034 |
| PERF-078 | UI-PRF-035 |
| PERF-079 | UI-PRF-035 |
| PERF-080 | UI-PRF-040, UI-PRF-041 |
| PERF-081 | UI-PRF-042 |
| PERF-082 | UI-PRF-040, UI-PRF-042, UI-PRF-043 |
| PERF-083 | UI-PRF-042, UI-PRF-043, UI-PRF-044 |
| PERF-084 | UI-PRF-044 |
| PERF-085 | UI-PRF-041 |
| PERF-086 | UI-PRF-041, UI-PRF-050 |
| PERF-087 | UI-PRF-041 |
| PERF-088 | UI-PRF-014, UI-PRF-015, UI-PRF-042, UI-PRF-050 |
| PERF-089 | UI-PRF-051 |
| PERF-090 | UI-PRF-051 |
| PERF-091 | UI-PRF-050, UI-PRF-051 |
| PERF-092 | UI-PRF-050, UI-PRF-051 |
| PERF-093 | UI-PRF-045 |
| PERF-105 | UI-PRF-052 |
| PERF-106 | UI-PRF-052 |
| PERF-107 | UI-PRF-052 |
| PERF-108 | UI-PRF-034, UI-PRF-052 |

Also realized: R-055 (UI-PRF-011), R-058 (UI-PRF-001 double-click, UI-PRF-045), R-066 (UI-PRF-041), R-067 (UI-PRF-051), R-084 and R-086 (UI-PRF-044, UI-PRF-045), R-089 (UI-PRF-040), R-090 (UI-PRF-010), R-100 (UI-PRF-041), D-007 (UI-PRF-022, UI-PRF-028), D-009 (UI-PRF-030), ADR-009 chart rules 1 to 10 (UI-PRF-001 to UI-PRF-007), ADR-014 fixture (UI-PRF-053).

## Decided here (all values this file chose; reviewers check these)

Shared sizes: `chart_pad` 6, `chart_label_row` 18, `page_pad` 16, `page_header_h` 36, `details_gap` 24, `kv_row_h` 46, `kv_col_gap` 16, `grid_cell_gap` 8, `grid_cell_h` 112, `graph_h_single` 240, `graph_h_stacked` 150, `graph_h_overall` 300, `secondary_graph_h` 120, `card_h` 80, sidebar padding `sp4`, page minimum width 400.

Chart: legend vertical under 200 px width, axis-max visibility rule (plot ≥ 100 high and non-fixed scale), range-label visibility rule (≥ 100 high and ≥ 160 wide), baseline quad at 40% alpha, dotted-gap quads every 3 physical px, stale tail as a flat continuation at half alpha, 2-period overscan, `TIER_1HZ` metric flag bit, dashed upload tint (30% toward white), dirty line alpha 0.6, fill alpha 0.85 for single-colour types, grid alpha 0.12, temperature scale rounding to 5 °C with the same 10 s fall rule, symmetric battery axis label "±N W", Commit axis text above 100%, curved-line subdivisions 4, hover box geometry and first-series rule for avg and peak, marker pinned in data time, reduce-motion note text under the Smooth graphs switch.

Sidebar: card inset `sp5`, text line heights 18/16/16, selected border 2 px drawn inside, focus ring 1 px accent at 0.5 outside, pressed tint, hover `row_hover`, no double-click on cards, device numbering from 0, Home and End keys, hotplug animations (`m_base` height and opacity), next-below-else-above selection rule, edit-mode header layout and button variants, missing empty-state texts ("No disk found", "No fan found", "No battery found"), drops limited to the same group, Alt+Up/Down keyboard reorder, drag ghost opacities, card menu separator and item 5, disabled "Hide This Page" for the last card, page menu order and separators, scrollbar mode Hover, sidebar hide animation.

Summary mode: tile grid `cols = ceil(sqrt(n))` with 200 × 120 minimum, heading composition, accent marker under the selected tile, click selects and exits, window title suffix " · Graph Summary".

Pages: header text styles, toggle size 24 × ≥ 122, selectable-value fallback (click selects, Ctrl+C copies it), cross-fade on CPU mode change, core-mark icons (`zap`, `leaf`, `star`), disabled-mode tooltip before P2, `cpu_fast` kept on with smooth off, CPU tooltip texts (Up time, Load average, BogoMIPS), secondary graph fallback tooltip and the "Set up CPU power draw…" menu item, commit heading tooltip, composition bar geometry, colours, caption, hatch and tooltip sentences, memory block visibility rules, swap table widths, module block header, Huge pages format, disk Transfer rate value = read + write, partition table widths, usage bar, "No partitions", SMART and Disk-in-use window sizes and button variants, 500 ms retry delay, Shift relabel, network Throughput value = sum, IPv6 second line, Wi-Fi signal icon, Fetch button size and failure tooltip, GPU row 2 split and placeholder cell when the probe is missing, GPU tooltip texts, CUDA row hidden off NVIDIA, display row height and empty text, fan PWM second axis, rename dialog, sensor item format, battery symmetric fill rule, week chart day labels, batteries on the `sensors` demand bit, aggregate Details rows.

Minigraphs: panel margins 16, slot order bottom right first, reset restores defaults and removes process cards, cards keep sliding under Ctrl-pause, shadow `(0, 4, 12)` at 0.35 × intensity, hover border accent 0.5, Disk card value = aggregate rate with active-time graph, middle-ellipsis label truncation, "Accent" means the type palette on system cards, card menu separators and colour dots, Hide hidden on process cards, drag threshold 4 px, ghost 0.85, 24 px detach margin, no-holes rule, detached window padding `sp2`, not resizable, "Close graph" re-attaches, per-card range and colour items in the detached menu, mini window card width 244, height cap 90% of the work area, Esc closes it, it stays open on card click.

Tray and setup: icon margin 1 px, 18 history columns, 2 px value bar, fill alpha 0.55, baseline for invalid, tooltip title/description split, process entries under "Tray icon graphs >", toast delivery when the window is hidden, setup card layout, description sentences, tag colours, confirm dialog titles and the revert body, "Command copied" toast, focus-row flash, `SetupStatus` and `RunSetup` commands.

Bench: bench-only layout with the overall chart above the grid, synthetic formulas and device list, latency measurement points, JSON shape.

## Open items

None. Settled in `docs/RECONCILIATION.md` R-126: selectable Details text ships through the
UI-PRF-020 fallback unless the kit gains text selection first (verify U-46); legends are vertical
under 200 px and horizontal above (UI-PRF-003); the Disk minigraph graphs active time and shows
the aggregate rate (UI-PRF-041); per-core labels update at 10 Hz with a 5 Hz fallback if the
`perf-cpu` bead's p99 fails (verify U-47); `cpu_fast` stays on with smooth graphs off.
