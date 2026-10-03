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
  - **Sidebar**: background `window_bg`; width `state.performance.nav_width` (180..=400, default 252, PERF-006); padding `sp4` on all sides; cards stacked with `sp4` gaps; vertical scroll with the kit scrollbar in `ScrollbarMode::Hover` (chosen; the kit default). A pencil `TlButton` (ghost, icon `pencil`, 24 px square) sits at the top right of the sidebar from P2 (edit mode, UI-PRF-013); in P1 the sidebar has no header row and cards start at `sp4`.
  - **Splitter**: gpui-kit `h_resizable("perf-split").with_state(&state)` with two panels: the sidebar `resizable_panel().size(px(nav_width)).size_range(px(180.)..px(400.)).flex_none()` and the page. The handle is the kit's 1 px line (`divider`) with 4 px hit padding each side and the `col-resize` cursor during the drag. `on_resize` (fires on mouse-up) writes `nav_width` to `state.toml` through the settings entity (500 ms debounce, ADR-011). On start the saved width is clamped to 180..=400.
  - **Page**: `window_bg`; vertical scroll (kit scrollbar, Hover mode); padding `page_pad`; children: header row, graph area, Details heading, `TlKeyValueGrid`, page blocks.
  - **Sidebar hidden** (F9, Ctrl+T): the sidebar panel gets `.visible(false)`; the page fills the width. Not persisted (R-090). Animation: width from 252 to 0 over `m_base` with `ease_out_cubic`, page reflows each frame; reduced motion: instant.
  - **Toolbar** (shell owns the toolbar; stated here for the layout contract, PERF-014): on the Performance tab the title bar shows only the shield and the "+" picker. The page header carries the page toggles.
- **States**: default; sidebar focused (UI-PRF-012); sidebar hidden; summary mode (UI-PRF-016) replaces the whole layout.
- **Interaction**: F9 and Ctrl+T dispatch `perf::ToggleSidebar`. Ctrl+M `perf::ToggleSummaryMode`. Ctrl+C `perf::CopyDeviceInfo` (UI-PRF-022). F1..F8 `perf::SelectVisibleDevice { index: u8 }`. Up and Down when the sidebar has focus `perf::SelectPrevDevice` / `perf::SelectNextDevice`. Focus order inside the tab: sidebar list (one tab stop, roving selection) → page header toggles → page blocks with controls (Display unit, Fetch, Show SMART data, Eject). Tab out of the last control returns to the shell's tab strip.
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
| Memory | "Memory" | "34.68 GiB / 63.16 GiB" | "Commit 69.0%" | used (filled), committed dashed | `55.0%` (used/total) |
| GPU | "GPU N" | name, or "Unknown GPU" | "52 °C" | 3D/Compute engine (or the max engine, PERF-009 rule) | `10.9%` |
| Disk | "Disk N" | "NVMe - /" (kind, " - ", primary mount; no mount: kind only) | model; from P2 "<model> · 41 °C" | active time | `0.1%` |
| Network | `<ifname>` | type ("Ethernet", "Wi-Fi", "Tunnel", …) | `↓ 1.2 Mbps ↑ 0.3 Mbps` when both non-zero, else total rate, in `accent`; disconnected: the state word ("Disconnected") in `text_secondary` | download filled, upload dashed | total rate, e.g. `0.01 Mbps` |
| Fan | "Fan N" or label | label (or chip) | "1200 RPM · 45 °C" | RPM | `1200 RPM` |
| Battery | kind ("Battery", "Mouse", "UPS") | vendor and model | "85.0% · Discharging" | percentage (peripheral) or power (system battery) | `85.0%` |
| Aggregate (P4) | "Aggregate storage" / "Aggregate graphics" | "3 disks" / "2 adapters" | "Multiple" when kinds differ, else the kind | busiest disk active / highest engine | percent |

  - Numbering: "Disk N", "GPU N", "Fan N" count from 0 in sidebar order (`performance.png` shows "GPU 0", "Disk 0"), re-numbered when the order changes.
  - Disconnected interface: the whole card at opacity 0.6 (PERF-001).
  - Tooltip (`TlTooltip`, 500 ms): the full model on line 1 when it is truncated; network cards add the NetworkManager connection name on its own line when known.
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
- **Interaction**: click → `perf::SelectDevice { key }`. Right-click → UI-PRF-014 menu. Double-click: none (keeps the single click fast; the mini window opens from the page graph area). Drag: edit mode only.
- **Data binding**: newest sample of the card's series from `Snapshot.series`; static text from `Snapshot.devices` (`DeviceCatalog`) and `Snapshot.system` (CPU model, clock, temperature). Re-render on `SERIES` for its own ids, on `SYSTEM` (CPU clock and temperature lines; memory totals) and `DEVICES`.
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
  - Pencil button or menu "Select Pages" → `perf::EnterEditMode`. "Done" or Esc → `perf::ExitEditMode` (Esc is handled before the global Esc rule while edit mode is on).
  - Switch → `perf::SetPageVisible { key, visible }`. Turning off the last visible page is refused: the switch snaps back and `TlToast` shows "At least one performance page must stay visible" (PERF-002).
  - "Enable All" → `perf::EnableAllPages`; "Disable All" → `perf::DisableAllPages` (keeps the current selected page visible and shows the same toast once); "Reset to Default" → `perf::ResetPagesToDefault` (clears `device_order` and `device_overrides`).
  - Drag with the handle: GPUI `on_drag` with a `DraggedCard { key }` payload; `drag_over` on card rows computes the drop index from pointer y against the row midpoint; drop → `perf::MoveDevice { key, before: Option<DeviceKey> }`.
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
  1. "Select Pages" → `perf::EnterEditMode`
  2. "Hide This Page" → `perf::SetPageVisible { key, visible: false }`
  3. "Show This Page" → `perf::SetPageVisible { key, visible: true }` (present only in edit mode and only on hidden cards)
  4. separator
  5. "Add tray icon graph" → `perf::AddTrayGraph { source: key }` (P3; disabled with tooltip "No system tray available" when no `StatusNotifierWatcher` is on the bus, PERF-088)
- **States**: "Hide This Page" is disabled when the card is the only visible one (tooltip "At least one performance page must stay visible").
- **Interaction**: right-click selects nothing (the card keeps its state) and opens the menu; Shift+F10 on the focused card opens it too. Hiding the selected page selects the next visible card (UI-PRF-012).
- **Build**: `.context_menu(..)` on the card row.
- **Tests**: `card_menu_items_default`, `card_menu_show_only_in_edit_mode_hidden`.
- **Phase**: P1 (item 5 P3).
- **Decided here**: separator and item 5 placement, disabled-instead-of-toast for the last page in the menu (the toast still shows when the switch is used).

### UI-PRF-015 Page jump "View >" submenu and page context menu

- **Implements**: PERF-007 (View submenu), PERF-010 (Graph Summary View item), PERF-011 (Copy), PERF-016 (Time range), PERF-088 (Add tray icon graph), plus page-specific items defined in Part C.
- **Anatomy**: `TlMenu` opened by right-click anywhere on a device page (header, graph area, Details). Common items, in order; page-specific items are inserted where Part C says:
  1. "Copy" → `perf::CopyDeviceInfo` (Ctrl+C hint)
  2. "Graph Summary View" (check item, checked while on) → `perf::ToggleSummaryMode` (Ctrl+M hint)
  3. "Time range >" → radio items "30 seconds", "1 minute", "5 minutes", "10 minutes" → `perf::SetGraphRange { secs }` (300 and 600 disabled before P2)
  4. separator
  5. page-specific items (CPU: graph mode and kernel; Memory: show composition, show swap; GPU: select engines, hide this engine; Fan: rename, sensor; Network: Network settings)
  6. separator
  7. "View >" → "Processor", "Memory", "Disks", "Graphics adapters", "Network adapters", "Fans", "Batteries" → `perf::JumpToType { kind }`; a type with no visible device is disabled (PERF-007)
  8. "Add tray icon graph" → `perf::AddTrayGraph { source }` (P3)
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
- **Interaction**: Esc or Ctrl+M → `perf::ToggleSummaryMode` (off). Click on a tile → leaves summary mode and selects that device (chosen). Right-click on a tile → the page menu (UI-PRF-015) for that device. Double-click → mini window (R-058 "Performance graph area" includes tiles). Window resize re-tiles.
- **Data binding**: visible devices and their main series. Demand is unchanged (same series as the sidebar).
- **Build**: `pages/performance/summary.rs` `SummaryView` entity, one `ChartView` per tile created on entry (reusing the sidebar cards' `ChartView` entities is not done, because the tile size differs and geometry is keyed by size anyway). The main window's render matches on `summary.is_some()` before the tab strip (shell spec hook: `MainWindow::render` asks the Performance page for `summary_view()`).
- **Performance**: n tiles at 1 Hz plus CPU at 10 Hz; same costs as the sidebar.
- **Accessibility**: Esc always exits; the mode is announced by the window title suffix " · Graph Summary" (chosen).
- **Tests**: `summary_tiles_all_visible_devices`, `summary_blocks_tab_switch`, `summary_esc_returns_to_selected_device`, `summary_tile_grid_for_9` (3 × 3).
- **Phase**: P2.
- **Decided here**: tile grid rule and minimums, heading composition, accent marker, click-to-select, title suffix.

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
  - **Toggles**: right-aligned `TlButton`s, small (24 px), outline variant, 122 px minimum width (measured), `sp4` apart. Dropdown buttons show the kit caret. Which buttons each page has is in its entry.
  - **"Details"** heading (PERF-008): `t_md` 500.
  - **Details grid**: UI-PRF-021.
- **States**: a page whose device went `Missing` is removed by the sidebar; while the removal animation runs the page keeps its last content.
- **Interaction**: right-click anywhere → UI-PRF-015. Double-click on the graph area → `perf::ShowMiniWindow` (P3). Text in the Details grid is selectable (PERF-012): values are rendered with the kit `Label`-free plain text in a `div().selectable()`-style GPUI text selection is not available in gpui-pre 0.3.7 for arbitrary divs [inferred from the API map, which lists no text selection on div]; tasklens therefore gives every value a right-click "Copy value" item and a click-to-select behaviour: clicking a value selects its text visually (bg `row_selected`, text `text_selected_primary`) and Ctrl+C copies that value instead of the device info while the selection is active; Esc clears it. Recorded under Open items.
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
  - Tooltips (`TlTooltip`): the reason for a fallback word ("Needs administrator access.", "Not available on this device", "Could not read: {last_error}", `Needs setup` → "Needs setup. Open Additional Setup…" which is a link row: clicking the value dispatches `app::OpenAdditionalSetup`); row-specific tooltips are listed per page.
  - Stale values: the last value in `text_secondary` with tooltip "Last updated {age} ago".
- **States**: value hover: underline dotted `text_secondary` only when the value has a tooltip or a click action; link values (`Needs setup`, "Fetch", "Display unit") in `accent`.
- **Build**: `TlKeyValueGrid` from the foundation (`columns(3)`, `row_h(46)`); model `KeyValueModel { rows: Vec<KvRow { label: &'static str, value: SharedString, validity: Validity, tooltip: Option<SharedString>, action: Option<Box<dyn Action>> }> }` kept in the page entity and updated on snapshot; a value string re-formats only when its bits change (`FxHashMap<&'static str, u64>` per page).
- **Performance**: 12 to 25 rows; text re-shape only on change. The grid is inside the page's `.cached()` Details block, which re-renders at most at 1 Hz.
- **Tests**: `kv_grid_fallback_words_per_validity`, `kv_grid_hidden_rows_rule`.
- **Phase**: P1.

### UI-PRF-022 Copy device info

- **Implements**: PERF-011, R-012, D-007.
- **Behaviour**: `perf::CopyDeviceInfo` (Ctrl+C in the `Performance` context, page menu "Copy") writes plain text: line 1 the page header words ("Processor"), line 2 the model, then every Details row as `    Label: value` (4-space indent, label as shown, value as shown or the PERF-013 word), in grid order, then each page block in its own paragraph with the block heading as a line and the same indented rows (partitions and swap tables as `    <col>: <value>` per row separated by blank lines). Memory module serials never appear (they are never read). Toast `TlToast` info "Copied metrics to clipboard".
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

  - **Mode button** (`TlButton` small outline with dropdown): label is the current mode name; menu radio items "Overall", "Per core", "All cores overlaid", "All cores stacked" → `perf::SetCpuGraphMode { mode }`. Default "Per core". The two extra modes are disabled with tooltip "Available in a later version" before P2 (chosen).
  - **Kernel toggle** (`TlButton` small outline, pressed look when on): label "Kernel on" / "Kernel off" → `perf::ToggleCpuKernel`. Default on. In overlaid and stacked modes the button is disabled (PERF-029: no kernel line there) and keeps its label.
  - **Per-core cell**: heading "CPU N", value `6.1%`, legend "Total"/"Kernel" in cell 0 only (PERF-028, `performance.png`), fill `cpu_fill`, line `cpu_line`, kernel overlay `kernel_line` solid 1 px no fill when on (PERF-029). Core marks (PERF-031, P2): a 12 px icon after the heading: Lucide `zap` for P cores and `leaf` for E cores on hybrid CPUs; Lucide `star` for the top-ranked AMD preferred cores with tooltip "Preferred core (rank 236)"; no icon otherwise. Icons use `text_secondary`.
  - **Heading tooltip** (PERF-028): "Core C, thread T, 4200 MHz" (integer MHz, table d).
  - **Offline CPU**: cell shows "Offline" (UI-PRF-002).
  - **Mode change animation**: cross-fade `m_base` between the old and new graph area; reduced motion: instant. Height change animates with the same duration.
- **States**: per UI-PRF-001 and UI-PRF-002. `WarmingUp` on first open: cells show grid and `…`.
- **Interaction**: page menu (UI-PRF-015) page-specific items: "Graph >" with the four mode radio items, "Kernel times" check item → `perf::ToggleCpuKernel`, "Secondary graph >" (UI-PRF-025). Hover readout (P2) per cell.
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
- **Interaction**: page menu "Secondary graph >" radio items "None", "Power draw", "Clock speed", "Temperature" → `perf::SetCpuSecondaryGraph { choice }`. "Power draw" is disabled with tooltip "Needs setup" when RAPL is unreadable; the tooltip's text is a link row that opens the Additional Setup window (clicking the disabled item does nothing; a second menu item "Set up CPU power draw…" appears directly under it in that case and dispatches `app::OpenAdditionalSetup`, chosen so the link is reachable from the menu).
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
│ fill mem_fill; scale Commit(limit); axis max text when > 100% │  heading tooltip "Committed memory against the commit limit. It can exceed 100%."
│ Commit over 60 seconds                                        │
└───────────────────────────────────────────────────────────────┘
┌ Swap                                                 1.20 GiB ┐  hidden when SwapTotal = 0 or memory_swap_graph = false
│ fill mem_fill; scale FixedTotal(SwapTotal)                    │
│ Swap usage over 60 seconds                                    │
└───────────────────────────────────────────────────────────────┘
sp6
[composition bar, UI-PRF-027]  (hidden when memory_composition = false)
```

- **Interaction**: page menu page-specific items: "Show memory composition" (check) → `perf::ToggleMemoryComposition`; "Show swap usage" (check) → `perf::ToggleMemorySwapGraph`. Hiding and showing animates height 150 → 0 over `m_base`; reduced motion: instant.
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
  2. **"Swap devices"** (PERF-045, P2): a static table (the foundation's `TlStaticTable`: header row 24 px `header` bg, rows 28 px, `row_alt` striping) with columns Name (flex), Type (80), Size (90, right), Used (90, right), Priority (70, right). Empty: the block is hidden when `/proc/swaps` has no entries.
  3. **"Memory modules"** (PERF-046, P2): a line "Slots used: 2 of 4" (`t_sm` `text_secondary`), then one collapsible block per populated DIMM: header row 28 px with a chevron `TlButton` ghost 20 px and the title "Memory module N" (`t_sm` 500); body a 3-column `TlKeyValueGrid` with rows Slot, Capacity, Speed, Configured speed (only when it differs), Type, Form factor, Manufacturer, Part number, Rank, Voltage. Blocks start collapsed when there are more than 4 modules; expand state is not persisted. When the udev keys are missing: the block list is replaced by one grid row "Module details" = `Not reported`. No serial anywhere (D-007).
- **Build**: `MemoryPage.details`, `MemoryPage.blocks`. The module block uses a GPUI `div` with the chevron rotating 90° over `m_fast` (reduced motion: instant).
- **Tests**: `memory_details_order`, `memory_hidden_rows_rule`, `modules_collapsed_when_more_than_4`, `no_serial_in_copy_text`.
- **Phase**: as per row and block.
- **Decided here**: block visibility rules, table column widths, module header layout, Huge pages format.
