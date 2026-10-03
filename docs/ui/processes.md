# UI spec: Processes tab (UI-PRC)

This file specifies the Processes tab of tasklens down to sizes, tokens, states, keys and GPUI
building blocks. It realizes the merged PROC entries for the table, columns, header, filter,
selection, menu, action UI and History mode (`docs/spec/merged/proc.md`). Precedence:
`docs/DECISIONS.md`, then `docs/RECONCILIATION.md` (tables a to d), then `docs/ARCHITECTURE.md`
(ADR-009, ADR-010, ADR-014), then the merged spec, then this file's "Decided here" choices.

Out of scope here, referenced by ID only: Process Properties and App Properties windows
(PROC-077 to PROC-085), the Run dialog (PROC-095), Handle Search (PROC-096), the low-memory
window (PROC-098), the title bar, tab strip, shield and "+" picker button (APP-002, APP-003,
APP-006, `docs/ui/shell.md`), the minigraph panel (PERF-080 to PERF-087, `docs/ui/performance.md`)
and the shared tokens and components (`docs/ui/foundation.md`).

## Conventions used in this file

- Tokens: `TlTheme` colour fields (`list_bg`, `row_alt`, `row_hover`, `row_selected`, `header`,
  `divider`, `text_primary`, `text_secondary`, `text_selected_primary`,
  `text_selected_secondary`, `value_heat`, `accent`, `menu_bg`, `menu_hover`, `scroll_track`,
  `scroll_thumb`) and `status.success`, `status.warning`, `status.danger`, `status.info` from
  `docs/ui/foundation.md`. Spacing `sp1`..`sp8` = 2, 4, 6, 8, 12, 16, 24, 32. Text `t_xs` 11,
  `t_sm` 12, `t_base` 13, `t_md` 14, `t_lg` 16. Radii `r_sm` 4, `r_md` 6. Motion `m_fast` 120 ms,
  `m_base` 200 ms, `m_slow` 300 ms, `ease_out_cubic`. All sizes are logical px at
  `ui_scale_pct = 100` and are multiplied by `ui_scale = ui_scale_pct / 100` (ADR-009
  `UiMetrics`).
- `row_h` = `28 × row_height_pct/100 × ui_scale` (R-020). Line height for `t_base` is 18.
- Shared components (type names from the foundation file): `TlButton`, `TlIconButton`, `TlMenu`
  (gpui-kit `PopupMenu` wrapper), `TlToast` (`push_notification` wrapper with `id1` replace
  semantics, top centre), `TlDialog` (gpui-kit `AlertDialog` wrapper), `TlTooltip`, `TlCheckbox`,
  `TlInput`, `TlSwitch`, `TlSlider`, `TlScrollbar`, `TlWindow` (secondary pinnable window frame,
  APP-045).
- Measured values come from `docs/taskslinger-reference/processes.png` (2720 × 1680, row pitch
  53 physical px, so 1 logical px at `row_h` 28 ≈ 1.9 physical px) and
  `tasklens-ref/evidence-frames/advanced-02.png`, `advanced-11.png`, `customize-28.png`. Each
  value says "measured", "spec" or "decided".
- Every GPUI `Action` named here lives in `crates/tl-ui/src/pages/processes/actions.rs`, namespace
  `tasklens`, and is dispatched identically from key binding, menu item and toolbar button
  (table b, rule "one command, one Action").
- "Phase" uses P0 to P4 as in the merged spec (R-072).

## Shared geometry decided for this surface

| Name | Value | Origin |
|---|---|---|
| `row_h` | 28 (scaled) | spec R-020 |
| `header_h` | 44 = 2 × 18 line height + 2 × `sp2` | decided (measured ≈ 46) |
| `group_row_h` | = `row_h` | decided (measured ≈ 36; `uniform_list` needs one height, see UI-PRC-002) |
| `indent` per tree level | 18 | measured (34 physical ÷ 1.9) |
| chevron box | 16 × 16, hit area 20 × `row_h` | decided |
| app icon | 24 | spec PROC-006 |
| process, unit, kernel icon | 16 | spec PROC-006 |
| icon to text gap | `sp3` 6 | decided |
| cell horizontal padding | 8 each side | spec PROC-023 |
| sort arrow | 12 wide, `sp1` after the label | spec PROC-023 (12 px reservation) |
| resize handle hit width | 8 (4 each side of the boundary) | decided |
| scrollbar width | 12 | decided |
| menu item height | 28, min width 240 | decided (measured menu pitch = row pitch) |
| tooltip delay | 600 ms rows, 500 ms controls | spec PROC-039, kit default |

---

## A. The tree-table component

### UI-PRC-001 TlTreeTable: generic virtualized tree table

- **Implements**: PROC-013; PROC-022, PROC-025 (column model and persistence hooks); ADR-009
  "Process tree-table". Reused by Services, Startup Apps, Connections and History (R-075).
- **Anatomy**:

  ```text
  ┌──────────────────────────────────────────────────────────────┬──┐
  │ header (TlTreeTableHeader, header_h = 44)                    │  │  ← UI-PRC-005
  ├──────────────────────────────────────────────────────────────┤sb│
  │ body: uniform_list, N rows × row_h                           │12│
  │   row 0 (group header)                                       │  │
  │   row 1 (app row, depth 1)                                   │  │
  │   row 2 (process row, depth 2)                               │  │
  │   ...                                                        │  │
  ├──────────────────────────────────────────────────────────────┴──┤
  │ horizontal scrollbar (only when Σ widths > viewport)   12       │
  └─────────────────────────────────────────────────────────────────┘
  ```

  Parts:
  - Background `list_bg`, fills the tab content area below the tab strip and above the status bar.
  - Header: `header_h`, background `header`, bottom border 1 px `divider`.
  - Body: `uniform_list` (GPUI `elements/uniform_list.rs:22`), item height `row_h`, horizontal
    sizing `ListHorizontalSizingBehavior::Unconstrained` so rows can be wider than the viewport,
    `track_scroll(&UniformListScrollHandle)`.
  - Vertical scrollbar: `TlScrollbar::vertical(&scroll_handle)` 12 wide, track `scroll_track`,
    thumb `scroll_thumb`, thumb radius `r_sm`, min thumb length 24, mode Always (decided; the
    reference shows the scrollbar at all times).
  - Horizontal scrollbar: same style, 12 high, shown only when the total column width exceeds the
    viewport width; header and body share one `x_offset: Pixels`.
- **Delegate trait** (`crates/tl-ui/src/table/tree_table.rs`):

  ```rust
  pub trait TreeTableDelegate: 'static {
      type RowId: Copy + Eq + Hash;                 // identity, never an index
      fn row_count(&self) -> usize;
      fn row_id(&self, ix: usize) -> Self::RowId;
      fn row_index(&self, id: Self::RowId) -> Option<usize>;   // FxHashMap, rebuilt per table version
      fn row_kind(&self, ix: usize) -> RowKind;     // GroupHeader | App | Unit | Process | KernelThread | Pseudo | Plain
      fn row_depth(&self, ix: usize) -> u8;
      fn row_expandable(&self, ix: usize) -> Option<bool>;      // None = no chevron, Some(expanded)
      fn row_flags(&self, ix: usize) -> RowFlags;   // NEW, EXITED, STALE, RESTRICTED, SUSPENDED, FROZEN, LAST_SIBLING, ANCESTOR_CONTINUES(bitset per depth)
      fn columns(&self) -> &ColumnLayout;           // ids, order, visible, widths, x offsets (Arc<[Pixels]>)
      fn column_spec(&self, id: ColumnId) -> &ColumnSpec;      // label, align, kind, sortable, min width template, tooltip
      fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut); // writes value bits + validity + text into the caller buffer
      fn name_cell(&self, ix: usize) -> NameCell;   // icon, primary text, secondary text, qualifiers, badges
      fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool; // false = no line 2
      fn sort(&self) -> SortSpec;
      fn selectable(&self, ix: usize) -> bool;      // false for group headers
      fn context_menu(&mut self, targets: &[Self::RowId], clicked_col: ColumnId, menu: TlMenu, w, cx) -> TlMenu;
      fn on_toggle_expand(&mut self, id: Self::RowId, cx);
      fn on_sort(&mut self, col: ColumnId, cx);
      fn on_columns_changed(&mut self, layout: &ColumnLayout, cx);   // persist
      fn on_visible_range(&mut self, ids: &[Self::RowId], cx);       // demand (max 512 ids)
      fn on_activate(&mut self, id: Self::RowId, cx);                // Enter / double-click
      fn row_tooltip(&self, ix: usize) -> Option<AnyView>;
      fn empty_text(&self) -> Option<SharedString>;                  // UI-PRC-024
  }
  ```

  `TlTreeTable<D>` is an `Entity` holding: `delegate: D`, `selection: Selection<D::RowId>`
  (UI-PRC-008), `scroll: UniformListScrollHandle`, `x_offset`, `focus: FocusHandle`,
  `text_cache: CellTextCache` (ADR-009), `header_state` (drag, resize), `hover_row:
  Option<usize>`, `paused: bool` (UI-PRC-017 sets it), `expanded` is owned by the delegate.
  The process delegate is UI-PRC-002.
- **States**: default; empty (UI-PRC-024); loading is never shown on the Processes tab
  (PROC-021); focused table draws a 1 px `accent` inset ring around the whole body only when focus
  came from the keyboard (Tab or Ctrl+F then Esc) and no row is selected; otherwise focus shows
  through the selected row's `row_selected` fill (rule "selection by fill, never a bar").
- **Interaction**: see UI-PRC-005 (header), UI-PRC-008 (selection), UI-PRC-009 (keys),
  UI-PRC-010 (scrolling), UI-PRC-013 (menu). Focus order inside the tab: filter input, Include
  children checkbox (when shown), table body. `key_context("Table")` on the body div and
  `key_context("ProcessTable")` added by the process page (table b).
- **Data binding**: generic; the delegate reads a snapshot component. The table re-renders when
  the delegate calls `cx.notify()`; the process delegate does so on `ComponentMask::PROCESSES`
  changes and on `ProcessTableView.query_generation` match (UI-PRC-002).
- **Build**: custom element tree. File `crates/tl-ui/src/table/tree_table.rs` (element and entity),
  `table/columns.rs` (`ColumnLayout`, `ColumnSpec`, templates, widths), `table/selection.rs`,
  `table/cells.rs` (cell painters), `table/header.rs`. Cached: `CellTextCache`
  (`text_cache.rs`), per-column x offsets `Arc<[Pixels]>` (rebuilt on layout change), the
  `row_index` map (rebuilt per table version), measured template widths per (font, size, scale).
- **Performance**: the body is not `.cached()` (it scrolls). Per frame: build only the visible
  range of rows (viewport height / `row_h` + 1, about 36 rows at 1000 px). Each row is one `div`
  with absolute-positioned fixed-width cell children; no `format!` in render (text comes from
  `CellTextCache` as `SharedString`); no per-cell element id except the Name cell (hover and
  tooltip) and the window button. Hit testing: one hitbox per row for click, hover and
  right-click; the Name cell and window button add one each. Target from ADR-014 `table-2000`:
  UI thread p99 ≤ 2.0 ms per frame while scrolling at 1,200 px/s with 1 Hz full data changes
  (UI-PRC-023 has the plan).
- **Accessibility**: everything reachable from the keyboard (UI-PRC-009). The selected row text
  uses `text_selected_primary` on `row_selected` (contrast is the theme's duty, foundation file).
  Tooltips on every icon-only control. GPUI has no screen reader on Linux (U-5), so the status
  bar text (UI-PRC-018) repeats the counts as text.
- **Tests**: `tree_table_renders_visible_range_only` (render 2,000 rows in a 720 px viewport,
  assert at most 27 row elements built); `tree_table_identity_selection_survives_resort`
  (select id X, reorder delegate rows, assert selected set unchanged and `row_index(X)` moves);
  `tree_table_reports_visible_demand` (scroll, assert `on_visible_range` called with range ± 1
  viewport capped at 512); `tree_table_text_cache_hit_rate` (10 identical table versions, hit rate
  above 95%, ADR-009).
- **Phase**: P1.
- **Decided here**: delegate trait shape; scrollbar mode Always and width 12; keyboard focus ring
  only without a selection; horizontal scrollbar only on overflow.

### UI-PRC-002 Process table delegate and row anatomy

- **Implements**: PROC-004 (group header rows), PROC-008 (app rows), PROC-009 (unit rows,
  Kernel threads row), PROC-012 (pseudo rows), PROC-014 (view modes as rendered), PROC-018,
  PROC-019, PROC-021, PROC-043 (row tints, visuals in UI-PRC-011), PROC-063 (suspended glyph),
  PROC-064 (frozen state), ADR-009 `TableRow`.
- **Anatomy** of every row kind (x positions at depth 0; each depth adds `indent` = 18 to the
  Name cell content only, other cells never move):

  ```text
  Group header (Grouped mode only)                                  row_h
  ┌──────────────────────────────────────────────────────────────────┐
  │▾ Apps (11)                                                       │  chevron x=8..24, label x=30
  └──────────────────────────────────────────────────────────────────┘
  App row (depth 1 in Grouped, depth 0 in Tree/Flat)
  ┌──────────────────────────────────────────────────────────────────┐
  │   ▸ [24px icon] Firefox (14)  Mozilla Firefox - Inbox   [⧉]│ HAILMAR… │      │ 3.3% │…
  └──────────────────────────────────────────────────────────────────┘
       chevron x=26..42, icon x=48..72, text x=78, PID blank
  Unit row
  │   ▸ [16] PipeWire Multimedia Service (3)                  │          │      │ 0%  │…
       chevron x=26..42, icon x=52..68 (vertically centred, 16), text x=74
  Process row (depth 2 under an app)
  │       ▸ [16] firefox (Web Content) ⏸                       │ oystein  │ 4321 +3 │ 0.4% │…
  Kernel-thread row (child of "Kernel threads (N)")
  │       [chip 16] [kworker/0:1]                               │ root     │ 1234   │ 0%  │…
  Pseudo row
  │     [16 cpu icon] Interrupts                                │ root     │        │ 0.3% │ n/a │…
  Exited row:  same as its kind, text text_secondary, tint status.danger 35%, not actionable
  New row:     same as its kind, tint status.success 35%
  ```

  Parts common to all rows:
  - Height `row_h`; background: even rows `list_bg`, odd rows `row_alt` (zebra, counted over the
    flattened row index so it stays stable while scrolling); hover `row_hover`; selected
    `row_selected` (UI-PRC-008). Tints for NEW and EXITED are drawn over the zebra colour and
    under the selection colour (UI-PRC-011).
  - Text `t_base` 13 regular `text_primary`; numeric cells add `tnum`.
  - Cells are absolutely positioned at the `ColumnLayout` x offsets with the column width; each
    cell has 8 px padding left and right and clips its text with an ellipsis (`text_ellipsis`,
    `overflow_hidden`, `whitespace_nowrap`).
  - Column separators (PROC-042, P4): 1 px `divider` quads at each column boundary when
    `tables.column_separators` is on.
  - Group header row (`RowKind::GroupHeader`): label `t_md` 14 semibold 600 `text_primary`,
    text "Apps (11)", "Background processes (99)", "System processes (14)" (counts after
    filtering, PROC-044). Chevron `ChevronDown` when expanded, `ChevronRight` when collapsed, 16 px
    `text_primary`, at x 8. No zebra: background always `list_bg`; a 1 px `divider` line across the
    full row width at the row's top edge, except for the first row. No other cell renders. Not
    selectable, not hoverable (no `row_hover`), click anywhere toggles expand. Decided: the row is
    `row_h` tall, not TaskSlinger's ≈ 36, because `uniform_list` needs one item height; the top
    divider stands in for the extra air.
  - App row: Name cell content: chevron (always present, apps have at least one child), icon 24,
    primary text `<desktop Name> (N)` plus qualifiers `(Flatpak)` `(Snap)` `(Wine)`, then from P2
    the secondary text (window title, `text_secondary`, after `sp4` 8 gap, same line, ellipsis
    shared with the primary text: the primary text never truncates before the secondary text is
    gone). PID cell blank. User shows the owner. Type "App". Metrics per PROC-008.
  - Unit row: like the app row with a 16 px service icon (Lucide `Cog`), text `<Description> (N)`
    or `<unit name> (N)` before P2; Type "Unit".
  - "Kernel threads (N)" row: unit-row shape with the chip icon (Lucide `Cpu`), collapsed by
    default, sits first in System processes.
  - Process row: optional chevron (only when the row has children in the current mode), icon 16
    (app icon inherited, generic executable icon Lucide `Binary`, or chip for kernel threads),
    name per PROC-007, then the state glyph when it applies: `Pause` 12 px `text_secondary` for
    State T (suspended), `Snowflake` 12 px `text_secondary` for Frozen (app and unit rows). PID
    cell per PROC-018: collapsed parent `"11528 +18"` where `+18` is drawn in `text_secondary`;
    expanded parent and leaf show the PID only.
  - Pseudo row: 16 px icon (`Activity` for Interrupts, `Archive` for Memory compression), no
    chevron, PID blank, Type "Pseudo", User "root".
  - Subtree totals (PROC-019, P2): when `processes.subtree_totals` is on and the row has children,
    aggregated cells render `Σ ` (U+03A3 and a space) in `text_secondary` before the value.
- **States**: default, hover (`row_hover` fill, Name-cell window button appears), selected
  (UI-PRC-008), focused-selected (same as selected; the keyboard focus row is the selection
  `focus` id and gets a 1 px `accent` inset ring only when it is not in the selected set, which
  happens after Ctrl+click deselect), disabled (never; exited rows are "not actionable", drawn
  per UI-PRC-011), stale (every metric cell text at 60% opacity, the ADR-003 "dimmed value
  stale"), restricted (lock glyph cells, UI-PRC-004), new and exited (UI-PRC-011).
- **Interaction**: click selects (UI-PRC-008); double-click on a row runs `OpenProperties` for
  process, app and unit rows and `ToggleExpand` for group headers; chevron click runs
  `ToggleExpand { id }` without changing the selection; click on the Name cell of a group header
  also toggles. Right-click: UI-PRC-013. Hover 600 ms over the Name cell: UI-PRC-012.
- **Data binding**: reads `Snapshot.processes: Arc<ProcessTableView>` (rows, groups, totals,
  records), `Snapshot.windows` (titles, P2), the icon path cache (ADR-016), settings
  `processes.mode`, `processes.tree_guide_lines`, `processes.subtree_totals`,
  `tables.*`, `appearance.heat_*`. Sends `Command::SetTableQuery(TableQuery)` on mode, sort,
  filter and expansion changes with a new `generation`; shows a view only when
  `query_generation` equals the last sent, else keeps the previous rows (ADR-009). Re-renders on
  `ComponentMask::PROCESSES | WINDOWS | SETTINGS`.
- **Build**: `crates/tl-ui/src/pages/processes/table.rs` (`ProcessTableDelegate`), rows drawn
  by `table/cells.rs`. Icons through `img(path)` for app and process icons (rasterized once per
  size by GPUI), Lucide through `gpui_kit_assets::IconName`. Cached: `CellTextCache` keyed by
  `(NodeId, ColumnId)`; the Name cell caches `(primary SharedString, secondary SharedString)` per
  `NodeId` and re-shapes only when the strings change.
- **Performance**: zero allocations per visible row per frame once cached: the row div, cell divs
  and `SharedString` clones are the only per-frame work. Icons are `Arc<RenderImage>` sprites.
  Expanded state changes do not rebuild caches.
- **Accessibility**: chevron has tooltip "Expand" / "Collapse"; the state glyphs have tooltips
  "Suspended" and "Frozen". Row text alternatives: the row tooltip (UI-PRC-012).
- **Tests**: `process_rows_render_each_kind` (fake snapshot with one row of every kind, assert
  Name-cell icon size 24 for app, 16 for others, PID blank for app, unit, group and pseudo rows,
  `+N` present only on the collapsed parent); `group_header_row_not_selectable`;
  `subtree_totals_prefix_only_with_children`.
- **Phase**: P1 (P2: window titles, pseudo rows, subtree totals, Frozen glyph, unit Description).
- **Decided here**: group header height equals `row_h`; zebra counted on flattened index; glyph
  icons `Pause`, `Snowflake`, `Binary`, `Cog`, `Cpu`, `Activity`, `Archive`; secondary text
  truncation order; stale cells at 60% opacity; chevron click does not select.

### UI-PRC-003 Name cell: indent, chevron, connector lines, icon, texts, window button

- **Implements**: PROC-006 (icons), PROC-007 (names), PROC-008 (app Name cell), PROC-017 (Tree
  Connector Lines), PROC-053 (Name-cell window button and popup), PROC-063 (pause glyph).
- **Anatomy** (depth d, Name column width W):

  ```text
  x = 8 + d×18 ─┐
  ┌────────────┼──────────────────────────────────────────────────────────┐
  │ │ │  ▸  [icon] Primary text (N) (Wine)  Secondary window title…  [⧉] │
  │ │ └──┘ connector elbow (when tree_guide_lines)                         │
  └──────────────────────────────────────────────────────────────────────┘
    guide lines at x = 8 + k×18 + 8 (chevron centre of ancestor k), 1 px divider
  ```

  - Indent: `8 + depth × 18` from the cell's left edge (8 is the cell padding).
  - Chevron: 16 × 16 Lucide `ChevronRight` (collapsed) or `ChevronDown` (expanded),
    `text_secondary`, `text_primary` on hover of its 20 × `row_h` hit area; absent when the row
    has no children, but its 16 px width is still reserved so icons align within one depth.
  - Connector lines (PROC-017, off by default): for each ancestor depth k < d whose
    `ANCESTOR_CONTINUES` bit is set, a 1 px vertical `divider` quad at x = `8 + k×18 + 8` over
    the full row height. For the row's own depth: a vertical quad from the row top to the row
    centre (and to the bottom when not `LAST_SIBLING`) plus a horizontal quad from that x to the
    chevron's left edge at the row centre. Drawn before the icon. Each quad is a `paint_quad`
    with 1 physical px snapping. Group header rows draw none; depth-0 rows under a group draw
    from the group chevron centre (x = 16).
  - Icon: 24 × 24 for app rows (vertically centred), 16 × 16 for every other row, `sp3` 6 before
    the text. Missing icon file: generic `Binary` glyph `text_secondary`.
  - Primary text: `t_base` regular `text_primary` (selected: `text_selected_primary`), one
    line, ellipsis. The count suffix `(N)` and the qualifiers are part of the same run.
  - Secondary text (app rows, P2): `t_base` regular `text_secondary` (selected:
    `text_selected_secondary`), after an `sp4` 8 gap, ellipsis; hidden entirely when the cell is
    narrower than the primary text + 8 + 40 px.
  - Window button (P2, KWin bridge only): `TlIconButton` ghost, 20 × 20, Lucide `AppWindow` 14 px,
    placed at the cell's right edge inset `sp2` 4, vertically centred; visible on row hover and on
    selected rows for rows that own at least one top-level window (process rows: own PID; app
    rows: any member PID). Tooltip "Windows (N)". Clicking opens a `TlMenu` popup anchored
    below the button with one item per window, label = caption or "Untitled window", icon none,
    item action `ActivateWindow { internal_id }`. Width min 240, max 480, captions ellipsised.
  - State glyphs after the primary text (UI-PRC-002).
- **States**: hover (chevron brightens, window button shows), selected (text tokens switch,
  button shows), restricted row (name still shown; the lock lives in metric cells), exited (text
  `text_secondary`, strikethrough is not used).
- **Interaction**: chevron click or Space or Right/Left arrows toggle expand (UI-PRC-009); window
  button click opens the popup; Enter in the popup activates; Esc closes it and returns focus to
  the table. Dispatch: `ToggleExpand { id }`, `ActivateWindow { internal_id }` → `Command::KWin
  (KWinCommand::Activate)`; toasts "Process window shown." or "No window found" (PROC-054).
- **Data binding**: `ProcessMeta.name`, desktop Name, qualifiers, `WindowsSnapshot` by PID,
  `processes.tree_guide_lines`. Re-render on `PROCESSES | WINDOWS | SETTINGS`.
- **Build**: `table/cells.rs::paint_name_cell` draws connector quads and chevron with
  `paint_quad` and `paint_svg` inside a `canvas`; icon and text are `img` and a `div` child so
  GPUI handles ellipsis. The popup is a `TlMenu` built in `pages/processes/menu.rs::window_popup`.
- **Performance**: connector quads are at most `depth + 2` per row; bit checks only. No per-frame
  string work.
- **Accessibility**: window button has a tooltip and is a tab stop only while visible on the
  focused row.
- **Tests**: `name_cell_indent_by_depth` (assert text x = 8 + 18 × depth + 16 + 6 + icon);
  `connector_lines_only_when_enabled`; `window_button_hidden_without_bridge`.
- **Phase**: P1 (window button and secondary text P2).
- **Decided here**: chevron width always reserved; secondary text hide threshold; window button
  icon and size; popup width bounds.

### UI-PRC-004 Cell rendering per column type

- **Implements**: PROC-003 (formats, table d), PROC-018 (PID +N), PROC-019 (Σ), PROC-027 to
  PROC-036 (per-column display rules), PROC-040 (heat tint), PROC-041 (dim zero), PROC-042
  (separators), R-050 (validity glyphs).
- **Anatomy**: a cell is `[padding 8][content][padding 8]` inside its column width. Content by
  `ColumnKind`:

  | Kind | Align | Font | Content |
  |---|---|---|---|
  | Text (Name handled by UI-PRC-003; User, Description, Image name, Image path, Command line, Type, Priority, Affinity, State, Unit, Cgroup, App ID, TTY, Working directory, Package, Packager, Package status, Architecture, Security label, Elevated, Sandbox, Session, Version, Power usage, Power usage trend, GPU engine, I/O priority, Scheduling policy, Container) | left | `t_base` regular | plain text, ellipsis. Image path ending " (deleted)" draws the suffix in `status.danger`. |
  | Integer id (PID, Parent PID, Process group, SID, Nice, OOM score, OOM adj, Last CPU) | right | `t_base` `tnum` | digits; PID adds ` +N` in `text_secondary` on collapsed parents |
  | Percent (CPU, CPU kernel, CPU user, CPU one core, GPU, GPU 0..15, GPU encode, GPU decode) | right | `tnum` | `%.1f%%`, `0%` at measured zero |
  | Memory size (Memory, Working set *, Commit *, Virtual *, Page tables, Swap, PSS, USS, VRAM, GPU memory shared, File size) | right | `tnum` | IEC two decimals, `512 B` under 1 KiB |
  | Rate (Disk, Disk read/write/total rate, I/O read/write rate, Network, Network send/receive) | right | `tnum` | `0.4 MiB/s`, `2.3 Mbps`; zero `0 MiB/s` or `0 Mbps` |
  | Count (Threads, Handles, Page faults*, Major faults*, Context switches*, Cycles*, Disk reads/writes*, I/O ops, Windows, X11 resources) | right | `tnum` | thin-space grouped digits |
  | Bytes cumulative (Disk read/write/total bytes and deltas, I/O read/write bytes and deltas) | right | `tnum` | IEC two decimals |
  | Duration (CPU time, CPU time kernel, CPU time user) | right | `tnum` | `h:mm:ss` |
  | Time (Start time, File modified time, Build date) | right | `tnum` | `HH:MM:SS` today, else `YYYY-MM-DD HH:MM` |

  - Validity glyphs replace the content, same alignment as the column: `…` (`text_secondary`)
    warming up; Lucide `Lock` 12 px `text_secondary` restricted, tooltip "Needs administrator
    access." (P3: "Needs administrator access. Click the shield to grant it."); `n/a`
    `text_secondary` unsupported, tooltip from the column (for example "Linux counts storage
    operations per cgroup, not per process", or "Needs setup" linking to the setup window);
    `-` `text_secondary` missing; `!` `status.warning` failed, tooltip with the error text; stale
    value: the normal text at 60% opacity.
  - Heat tint (PROC-040): for Percent, Memory size, Rate and Power kinds only, when
    `appearance.heat_coloring` is on and the cell holds a valid non-zero value: a `paint_quad`
    filling the whole cell bounds (no padding inset, no radius) with `value_heat` at
    `alpha = heat_intensity_pct/100 × clamp(value / reference, 0, 1)` where the reference is the
    PROC-040 table per column kind (CPU `400/ncpu` %, one-core 400%, memory 10% of MemTotal,
    disk and I/O 100 MiB/s, network 10 MiB/s or 80 Mbps, GPU 100%, VRAM 25% of total, power
    25 W). Drawn above the zebra and highlight tints, below the selection fill (the selected row
    shows no heat; decided, matches the reference screenshot where the selected row is flat
    blue). Counts, ids, text and durations are never tinted.
  - Dim zero (PROC-041, P2): a measured zero draws its text at 30% opacity when
    `tables.dim_zero_values` is on. Never applied to glyphs.
  - Σ prefix (UI-PRC-002).
  - Cell tooltips (hover 600 ms on the cell, not only the Name cell): Command line cell shows
    the first 512 characters wrapped at 80 columns; Network cell "shared" note; Memory cell on
    app rows "Anonymous memory charged to this app's cgroup"; Priority "nice N"; Power
    "≈ 3.4 W (estimate)"; Disk write "cancelled: 1.2 MiB"; User "euid N" when it differs; VRAM
    on GPU N columns "VRAM on GPU 0: 1.22 GiB"; Memory compression row "original 1.2 GiB,
    compressed 400 MiB, ratio 3.1"; OOM score "oom_score_adj N". Other cells have no tooltip.
- **States**: valid, each validity glyph, stale, zero (dimmed when enabled), selected (text uses
  `text_selected_primary`; `text_secondary` parts use `text_selected_secondary`).
- **Interaction**: none beyond tooltips; right-click records `clicked_col` for "Copy"
  (UI-PRC-013).
- **Data binding**: `ProcessRecord` fields and `GroupRow` aggregates via `TableRow.record` and
  `TableRow.group`; formatting in `tl-model::units` writing into a stack buffer, then interned
  into `CellTextCache` only when `(value_bits, validity)` changed (ADR-009).
- **Build**: `table/cells.rs::paint_metric_cell(out: &CellOut, spec: &ColumnSpec, selected, bounds)`
  draws the heat quad then the text as a `div` child with `text_right()` and
  `font_features(tnum)`; glyph icons via `Icon::new(IconName::Lock).xsmall()`.
- **Performance**: per visible cell: one cache lookup, zero or one `paint_quad`, one text child.
  Formatting runs only for changed values; at 2,000 rows × 10 columns changing at 1 Hz that is
  about 400 visible cells formatted per second, well under budget.
- **Accessibility**: glyph tooltips carry the meaning as text; colour is never the only signal
  (heat is additive to the number).
- **Tests**: `cell_formats_match_table_d` (one assertion per kind and per zero form);
  `heat_alpha_clamped_and_intensity_scaled` (CPU 25% at reference 12.5% gives alpha 0.5 at
  intensity 50); `validity_glyphs_never_show_zero`; `dim_zero_only_for_measured_zero`.
- **Phase**: P1 (dim zero P2, separators P4).
- **Decided here**: heat quad covers the full cell and is hidden on selected rows; stale at 60%
  opacity; cell tooltip list; wrapped command-line tooltip at 80 columns.
