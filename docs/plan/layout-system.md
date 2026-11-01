# Configurable Layout System: Design Draft

Status: design, approved in outline by Oystein (2026-10-31). Nothing here is built. After approval it becomes
ADR-019, decision D-041 and an epic of beads.

## What Oystein asked for (2026-10-31)

- Much more freedom in the layout inside every Overview box, not only where the boxes sit.
- The same freedom on every resource page (CPU, Memory, GPU, Disk, Network).
- Full choice of which graphs a box shows. Example: the GPU box.
- Every metric and sub-resource (VRAM, each GPU engine, each core, each disk, clocks, power,
  temperature) can be shown as a graph, a load bar, numbers and stats, or a mix.

## What exists today

- The Overview is a 12-column snap grid of 7 box kinds. Edit Layout moves and resizes boxes.
- Each box has 1 to 5 fixed modes (`cpu_box::MODES` and the others), an Arrange choice and one
  inner splitter. The per-box extras are kind-specific fields in `OverviewGridSetting`.
- Resource pages have a fixed section order and a few toggles (`gpu_optional_graphs`,
  `cpu_secondary_graph`).
- `ChartSpec` (series, `YScale`, `ValueFormat`) already describes "a metric as a chart". Nothing
  like it exists for a bar or a numbers row. `spark_row` draws a label, bar, value and optional
  sparkline.

## Proposal

### 1. Widgets

A widget shows one metric source in one display form.

- **Source**: a `SeriesId` (device and metric), or a group of them: all cores, all engines of
  a GPU, all disks, all adapters. A group widget draws one item per member.
- **Form**, any combination of:
  - `chart`: a history chart over the box's range, with the existing `YScale` and series
    options (fill, line, dashed, kernel overlay).
  - `bar`: a load bar, as the core rows and the VRAM row draw today.
  - `value`: the current value, formatted with the metric's `ValueFormat`.
  - `stats`: min, average and peak over the range.
  - `label`: the source's name.
- A row-style widget (label, bar, value, sparkline) is the current core row and VRAM row, made
  general. A chart widget is today's box chart, made general.

### 2. One grid everywhere

- Every Overview box and every resource page holds a grid of widgets. It uses the same snap-grid
  rules as the Overview page: whole cells, move and resize in Edit Layout, rows grow to fill
  (D-037, D-040), nothing overlaps.
- The Overview page grid of boxes stays as it is. A box becomes a titled container of widgets.
- A resource page becomes one container of widgets, with its Details grid as a widget too.

### 3. Editing

- Edit Layout on any page or box shows a grip on each widget. Drag to move, drag the corner to
  resize, gear for the widget menu.
- The widget menu: Source, Form (check items for chart, bar, value, stats, label), chart
  options, Remove.
- "Add widget" lists the sources the device has, grouped by device.
- "Reset" puts a box or page back to its default layout.

### 4. Presets

- Today's modes become presets: each one is a saved widget layout. The defaults stay as they
  look now, so nothing changes until the user edits.
- A user's own layout can be saved as a named preset and applied to another box of the same
  kind (inferred need, to confirm).

### 5. Settings

- New schema `[[performance.layouts]]`: one entry per container (`overview.<box id>`,
  `page.cpu`, `page.gpu.<device>` and so on) with a list of widgets `{source, forms, col, row,
  cols, rows, options}`.
- The current `mode`, `split`, `arrange`, `power`, `clocks` and `hidden` fields are read once
  and converted to the new form, as the legacy `rows` form was (no data loss on upgrade).
- Invalid entries are dropped on load with a warning, as today.

### 6. Rules that do not change

- The render contract of UI-PRF-017 and UI-PRF-019: a sample never re-renders the page; each
  widget is a cached view; a layout change rebuilds only its container.
- The idle CPU budget and the per-frame budgets in the bench must still pass.

## Implementation rules (from the Astra review, 2026-10-31)

1. **One entity per widget.** Each widget is a GPUI entity keyed by a stable widget id. Samples
   go straight to the widgets that show them; no page or container render subscribes to
   samples. A layout edit keeps the entities of widgets it did not touch.
2. **Durable sources.** Settings store a source selector (device identity plus metric), never a
   runtime `SeriesId`. A device that is gone shows its widget as unavailable; nothing in the
   layout is deleted by a hotplug.
3. **Metric metadata.** Each metric declares its unit, `ValueFormat`, chart scale, whether it is
   a gauge or a counter, the bar's denominator (or no bar), and how min, average and peak are
   computed over the range. A form the metric does not support is not offered.
4. **Widget composition.** A fixed set of arrangements for the chosen forms (for example label
   and value on top, bar beside, chart below), each with a minimum size. An empty form set is
   not allowed. The Details grid is its own widget type, not a metric widget.
5. **Groups later.** Phase 1 has single-metric widgets only. Group widgets (all cores, all
   engines) come later, with member order, limits and scrolling defined then. Phase 2 defines them
   in ADR-019 "Phase 2".
6. **Shared data.** Widgets that show the same source share its history and its statistics;
   chart geometry is cached by data revision and size. Hidden widgets do no work.
7. **Migration.** The layout settings get a schema version. Conversion is deterministic and
   idempotent and is done per container, only for containers the running build supports; the
   others keep their old settings. Absent layout and empty layout are different states. The
   original file is kept before the first converted write.
8. **Inner grid.** Each container has its own column count, minimum cell size and rules for
   insert, resize and shrink, tested at small windows and at 1.5x and 2x scale.
9. **Editing.** Editing boxes on the page and editing widgets inside a box are separate modes.
   Inner editing shows a placement preview, has keyboard moves, and works as a transaction
   with Undo, Apply and Cancel. Reset asks first. Only applied changes are saved.
10. **Presets.** Built-in presets are separate from saved user layouts. Named user presets come
    after phase 2.
11. **Gates.** Each phase ends with migration fixtures for every old mode, a visual check
    against the old look, a hotplug test, and a counter test that a sample renders no page or
    container and keeps widget entities.

## Delivery phases

1. The GPU box only: single-metric widgets (any GPU engine, VRAM, clocks, power, temperature)
   with any mix of forms, the inner snap grid, inner editing with Apply, Cancel and Undo
   (add, configure, remove, move, resize, reset), the versioned settings with conversion of
   the GPU box's old settings, and the render-budget and hotplug gates.
2. All Overview boxes move to widgets. The old modes become presets.
3. Resource pages move to widgets, starting with GPU and CPU.
4. Named presets, copy layout between boxes, polish.

## Decided by Oystein (2026-10-31)

1. Inside every box and every resource page, widgets sit on the same snap grid as the Overview
   page and can be dragged and resized freely in Edit Layout.
2. A widget shows any mix of chart, bar, value, stats and label.
3. Phase 1 starts with the GPU box (leader default; Oystein can change the order).
