# UI spec: Performance tab and graph surfaces (UI-PRF)

This file turns `docs/spec/merged/perf.md` (PERF-001 to PERF-093 and PERF-105 to PERF-108) into build instructions for `crates/tl-ui/src/`. It covers every graph surface in tasklens: the chart element, the device sidebar, every device page, summary mode, the minigraph panel, detached graph windows, the mini window, the tray graph rasterizer, the content of the Additional Setup window, and the exact composition of the phase-0 bench scenes `perf-cpu`, `bridge` and `static`.

Not in this file: the Information tab (PERF-094 to PERF-104, system tabs spec), the title bar, menus, tab strip and toolbar (shell spec), the Processes context menu items that create minigraphs and tray graphs (processes spec), and the Settings window pages (shell spec). The Additional Setup window frame (title, pin, close) is the shell's; its rows are here.

Precedence: `docs/DECISIONS.md`, then `docs/RECONCILIATION.md`, then `docs/ARCHITECTURE.md` (ADR-009 and ADR-014 above all), then `perf.md`, then this file. Where this file states a number that none of them give, it is listed under "Decided here" at the end.

## How to read this file

- Names of colours, spacing, text styles, radii and motion are the shared names from `docs/ui/foundation.md`: `TlTheme` fields (`window_bg`, `list_bg`, `row_hover`, `row_selected`, `divider`, `menu_bg`, `text_primary`, `text_secondary`, `accent`, `value_heat`, `graph.*`), `status.success|warning|danger|info`, `sp1..sp8` (2, 4, 6, 8, 12, 16, 24, 32), `t_xs..t_xl` (11, 12, 13, 14, 16, 20), weights 400/500/600, `r_sm` 4, `r_md` 6, `m_fast` 120 ms, `m_base` 200 ms, `m_slow` 300 ms, `ease_out_cubic`. Every length is logical px at `ui_scale_pct = 100` and is multiplied by `ui_scale` once (ADR-009 "Metrics and scale").
- Component types are the foundation's: `TlButton`, `TlMenu`, `TlToast`, `TlDialog`, `TlKeyValueGrid`, `TlTooltip`, `TlSwitch`, `TlTag`. Where a gpui-kit widget is used directly, its kit name is given.
- "Measured" means read from `docs/taskslinger-reference/performance.png` (2720 × 1680 px; the window is treated as 1360 × 840 logical at 200%, so one logical px is 2 image px) or from the evidence frames at `/home/oystein/work/tasklens-ref/evidence-frames/` (1728 px wide, treated as 1280 logical at 135%). "Spec" means the value comes from `perf.md`, an R entry or an ADR. "Chosen" means this file picked it.
- Validity words and glyphs are PERF-013 and R-050. Number formats are canonical table (d). Settings and state keys are canonical table (a). Keys are canonical table (b).
- "Mask bits" names the `ComponentMask` bits a view notifies on. Bits are named after `Snapshot` fields: `SERIES`, `SYSTEM`, `DEVICES`, `HEALTH`, `PROCESSES`, `WINDOWS`. A chart view also checks `SnapshotChanged.changed_series` for its own `SeriesId`s (ADR-004 hub rule), so a snapshot that changes other series does not re-render it.
- Actions live in `crates/tl-ui/src/pages/performance/actions.rs` under the `perf` namespace (`tasklens::perf::*`) and in `pages/minigraph/actions.rs` under `minigraph`. Data-carrying actions use the gpui-kit `#[derive(Action)]` form. Every menu item, button and key dispatches the same action (coding-guides rule 10).

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

- **Interaction**: none on P1. Right-click bubbles to the page (page context menu, UI-PRF-021) or the card. Double-click on a page chart area opens the mini window (P3, R-058): the page's graph container has `.on_click` checking `event.click_count == 2` and dispatches `perf::ShowMiniWindow`. Hover is UI-PRF-004.

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

| Field | Default Dark | Used by |
|---|---|---|
| `cpu_fill` | `#0D546D` | CPU total fill, per-core fills, CPU minigraph, CPU sidebar card |
| `cpu_line` | `#0A7096` | CPU lines |
| `kernel_line` | `#C1514E` | kernel overlay (no fill) |
| `mem_fill` | `#7E6A9A` at alpha 0.85 | memory fill |
| `mem_line` | `#7E6A9A` | memory line, committed dashed, dirty thin |
| `gpu_fill` | `#508A59` | GPU fills, VRAM |
| `gpu_line` | `#71B768` | GPU lines |
| `disk_fill` | `#D9B567` at alpha 0.85 | disk |
| `disk_line` | `#D9B567` | disk line, write dashed |
| `net_fill` | `#359CAB` at alpha 0.85 | download fill |
| `net_line` | `#359CAB` | download line, upload dashed (tinted) |
| `grid` | `text_secondary` at alpha 0.12 | grid hairlines |

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
