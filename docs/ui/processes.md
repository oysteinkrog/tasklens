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

### UI-PRC-005 Two-line header: totals, sort arrows, resize, reorder

- **Implements**: PROC-038, PROC-047 (sort gestures), PROC-025 (resize and reorder), PROC-023
  (minimum widths), PROC-033 (GPU N header line 2 and tooltip), PROC-030 and PROC-032 (header
  tooltips), ADR-009 "Header".
- **Anatomy** (one header cell per visible column, at the same x offsets as the body cells):

  ```text
  header_h = 44, background `header`, bottom border 1 px `divider`
  ┌──────────┬──────────┬──────────┬─────────┬──────────┬─────────┐
  │ Name     │ User     │   PID ↑  │    CPU  │  Memory  │   Disk  │  line 1: y 4..22
  │          │          │          │  11.9%  │  54.9%   │0.01 MiB/s│ line 2: y 22..40
  └──────────┴──────────┴──────────┴─────────┴──────────┴─────────┘
             ↑ resize handle: 8 px hit band centred on each boundary (4 + 4)
  ```

  - Line 1: column label `t_base` 13 medium 500 `text_primary`, aligned like the column (text
    columns left, numeric right), padding 8, ellipsis. The active sort column appends the sort
    arrow: Lucide `ArrowUp` (ascending) or `ArrowDown` (descending) 12 px `text_primary` after a
    `sp1` 2 gap, inside the 12 px reservation of PROC-023; on right-aligned columns the arrow sits
    at the right edge and the label moves left by 14 (measured: "PID ↑").
  - Line 2: the system total from `ProcessTableView.totals`, `t_base` regular `text_secondary`,
    always right-aligned (measured; also on the left-aligned Name column there is no line 2), same
    validity glyph rules as cells (`n/a` without a GPU, `…` before the second sample). Columns
    without a total leave line 2 empty. Totals per PROC-038: CPU, Memory, Swap, Disk (busy %),
    Network (% of link speed, or a rate), GPU, GPU N, VRAM, Threads, Handles.
  - Header tooltip (hover 500 ms over the cell): Memory "Private memory (RssAnon). Shared and
    file-backed pages are not counted."; Working set total "Resident set size (VmRSS)"; Network
    "Attributed TCP traffic. UDP and other users' sockets are not counted."; Handles "Open file
    descriptors"; GPU N "<adapter name> (<PCI address>)"; unsortable columns "Only measured for
    visible rows"; every other column: the picker's full name when it differs from the label
    (for example `Working set private - "Memory"`), else no tooltip.
  - Resize handle: an 8 px wide hit band centred on each column boundary (and after the last
    column), full `header_h`; cursor `col-resize` on hover; the boundary draws a 1 px `divider`
    line in the header at all times (measured: thin vertical dividers in the header), brightened
    to `accent` while dragging.
  - Reorder drag: pressing on a header cell and moving more than 4 px starts a drag. The dragged
    cell is drawn as a ghost (`header` fill at 85% opacity, label, 1 px `divider` border,
    `r_sm`) following the pointer; a 2 px `accent` insertion line spans the header and body height
    at the nearest boundary. Name cannot be dragged and nothing can be dropped before it
    (decided; Name is also the only column that cannot be hidden).
  - Horizontal scroll: the header is translated by the shared `x_offset` with the body.
- **States**: default; hover (cell fill `row_hover`); pressed (fill `row_selected` at 50%);
  sorted (arrow); unsortable (PSS, USS, Cycles, Cycles delta, Context switches, Context switches
  delta: no hover fill, no arrow, cursor default, tooltip "Only measured for visible rows");
  resizing (boundary `accent`); dragging (ghost and insertion line); disabled: never.
- **Interaction**:
  - Click anywhere on a sortable header cell: `SortByColumn { id }`. First click: descending for
    numeric kinds, ascending for text kinds; each further click flips. Text columns sort
    case-insensitively (engine). For background-computed columns (package, file) the first click
    starts the lookup with a progress tooltip "Looking up N of M…" on the header cell and the sort
    applies when it finishes (PROC-027).
  - Drag a boundary: live width change, clamped to the PROC-023 minimum (template width + 16 +
    12 on the sort column; text columns 60). `tables.column_resize = "proportional"`: the dragged
    boundary moves and all columns to its right share the remaining width proportionally to
    their current widths; `"last_column"`: only the dragged column and the last visible column
    change. Double-click on a boundary fits that column to content (same rule as "Resize columns
    to fit content" for one column; decided). Release: `on_columns_changed` persists widths.
  - Reorder: drop commits `order`; Esc during a drag cancels it.
  - Right-click anywhere on the header: UI-PRC-006.
  - Keyboard: the header is not focusable; sorting from the keyboard goes through View >
    Columns > "Sort by >" in the shell menu (shell spec) and the header menu (UI-PRC-006).
- **Data binding**: `ProcessTableView.totals`, `ColumnLayout` from `state.toml
  [columns.processes]`, `[sort.processes]`; writes both through `on_columns_changed` and
  `on_sort` (atomic save, 500 ms debounce, ADR-011). Re-renders on `PROCESSES` (totals) and
  layout changes.
- **Build**: `crates/tl-ui/src/table/header.rs::TlTreeTableHeader` custom element: one
  `canvas` painting fills, dividers, arrows and the insertion line with `paint_quad` and
  `paint_svg`, plus one text `div` per line per column; hitboxes per cell and per boundary
  (`insert_hitbox`). Drag state in `with_element_state`. The header is `.cached()` as part of the
  page and invalidated by the totals change (1 Hz) and pointer events over it.
- **Performance**: ≤ 2 shaped lines per visible column; totals re-shape only when their cached
  text changes. No work while idle.
- **Accessibility**: header tooltips name the figure; sort state is visible as an arrow and
  repeated in the header menu as a checked "Sort by" item.
- **Tests**: `header_sort_cycle_numeric_desc_first`, `header_sort_cycle_text_asc_first`,
  `resize_clamps_to_template_minimum`, `resize_proportional_vs_last_column`,
  `reorder_cannot_move_name`, `header_line2_blank_for_text_columns`.
- **Phase**: P1.
- **Decided here**: line 2 always right-aligned; boundary lines always drawn in the header;
  ghost and insertion visuals; Name fixed first; double-click boundary fits one column; 4 px drag
  threshold.

### UI-PRC-006 Header context menu

- **Implements**: PROC-026; PROC-022 (Name cannot be hidden); PROC-025 (resize behaviour).
- **Anatomy**: `TlMenu` opened at the pointer, min width 240, item height 28, `menu_bg`, hover
  `menu_hover`, text `t_base`:

  | # | Item | Kind | Action | Disabled when |
  |---|---|---|---|---|
  | 1 | Sort by <label> | check (checked when this column is the sort) | `SortByColumn { id }` | column unsortable |
  | 2 | Select Columns… | item | `SelectColumns` | never |
  | 3 | Hide This Column | item | `HideColumn { id }` | id == name, or only one column visible |
  | | separator | | | |
  | 4 | Resize columns to fit content | item | `FitColumns` | never |
  | 5 | Resize Behavior > | submenu: radio "Proportional", "Last Column" | `SetResizeBehavior { mode }` | never |
  | 6 | Reset to default | item | `ResetColumns` | never |

  Item 1 is added above the PROC-026 list so the keyboard path can sort (decided); the rest is
  the PROC-026 order. The menu names the clicked column in items 1 and 3 ("Hide This Column"
  keeps the spec label; the tooltip on it reads "Hide <label>").
- **States**: as `TlMenu` (foundation). Disabled item 3 shows the tooltip "Name cannot be
  hidden" or "At least one column must stay visible".
- **Interaction**: right-click on any header cell; Shift+F10 while the header is hovered does
  nothing (header is not focusable). Toasts: `HideColumn` on the last visible column (reached
  only through the picker) "At least one column must stay visible"; `ResetColumns` "Columns
  reset to default". `FitColumns`: each visible column takes `max(min width, widest visible cell
  text + 16, header label + 16 + 12)`, measured over the currently visible rows plus the cached
  texts of the last 3 table versions (decided; measuring all 2,000 rows would need a full
  format pass).
- **Data binding**: writes `[columns.processes]` and `tables.column_resize`.
- **Build**: `pages/processes/menu.rs::header_menu(col: ColumnId)`; `TlMenu` over gpui-kit
  `PopupMenu` with `.menu_with_check`, `.submenu`, `.separator`.
- **Performance**: built on open only.
- **Accessibility**: arrow keys, Enter and Esc as every `TlMenu`.
- **Tests**: `header_menu_hide_disabled_on_name`, `reset_columns_restores_r006_defaults`
  (order, visible set and widths equal table a), `fit_columns_never_below_minimum`.
- **Phase**: P1.
- **Decided here**: "Sort by" item; FitColumns measurement scope.

### UI-PRC-007 Column picker window

- **Implements**: PROC-024; PROC-022 (visibility by id, Name fixed, GPU N offered per
  adapter); APP-018 (View > Columns applies to every table: the same window serves the other
  tables with their own registries).
- **Anatomy**: a `TlWindow` (secondary, pinnable, non-modal, APP-045) titled "tasklens Columns",
  default 420 × 560, minimum 320 × 400, resizable:

  ```text
  ┌ tasklens Columns ─────────────────────────────── 📌 ✕ ┐
  │ [🔍 Filter columns…                                 ✕] │  TlInput, sp5 margin, height 28
  │ ┌───────────────────────────────────────────────────┐ │
  │ │ IMAGE                                             │ │  group heading t_xs 11 semibold text_secondary, uppercase
  │ │ [✓] Name                              (always on) │ │  row 28: TlCheckbox + label t_base; note text_secondary right
  │ │ [✓] Description                                   │ │
  │ │ [ ] Image name                                    │ │
  │ │ [ ] Command line                                  │ │
  │ │ IDENTITY                                          │ │
  │ │ [✓] User                                          │ │
  │ │ [✓] PID                                           │ │
  │ │ ...                                               │ │
  │ │ MEMORY                                            │ │
  │ │ [✓] Working set private - "Memory"                │ │
  │ │ [ ] PSS                            n/a on this… │ │
  │ └───────────────────────────────────────────────────┘ │
  │ 9 of 96 columns shown        [Reset to default] [Close]│  footer, sp5 padding
  └─────────────────────────────────────────────────────────┘
  ```

  - Group headings in the fixed order: Image, Identity, Security, Scheduling, CPU, Memory, I/O,
    Disk, Network, GPU, Power, Objects, Linux (PROC-024). Heading row height 24, padding top
    `sp4` 8 (first heading `sp2`).
  - Rows: 28 high, `TlCheckbox` 16 px at x 12, label at x 36, `t_base` `text_primary`; hover
    `row_hover`; the whole row toggles. Name shows a checked, disabled checkbox and the note
    "always on". Columns that are `n/a` on this machine (for example Cycles with
    `perf_event_paranoid` 2, X11 resources without `$DISPLAY`) stay listed, enabled, with the note
    "n/a on this system" in `text_secondary` right-aligned. GPU 0..15 rows exist only for present
    adapters; the label is "GPU 0 (NVIDIA GeForce RTX 5070 Ti)".
  - Alias rows: `Working set private - "Memory"` and `GPU memory dedicated - "VRAM"`; the filter
    matches both halves.
  - Filter input: `TlInput` small, prefix `Search` icon, cleanable, placeholder "Filter columns…";
    matching is case-insensitive substring over label, alias and group name; non-matching rows
    and empty groups hide; no result shows the empty text "No columns match" centred
    `text_secondary`.
  - Footer: count text `t_sm` `text_secondary` left; `TlButton` "Reset to default" (ghost) and
    "Close" (default) right, `sp4` gap.
- **States**: default; filtered; empty; a checkbox hover and focus ring per `TlCheckbox`;
  Name disabled.
- **Interaction**: toggling a checkbox applies at once (no OK): `SetColumnVisible { id, on }`;
  showing a column restores its remembered width or the default (PROC-022) and appends it at
  the end of the order unless it has a remembered position (decided: `order` keeps hidden
  columns in place, `visible` is the filter, so a re-shown column returns to its old slot).
  Hiding the last visible column other than Name is impossible since Name is always on.
  Keyboard: Tab order filter, list, Reset, Close; in the list Up/Down move, Space toggles, Home
  and End jump; typing in the filter narrows; Esc closes the window (or clears the filter first
  when it has text, APP-028 rule). Opened by `SelectColumns` from the header menu, View > Columns
  > "Select Columns…", and the shell; opening again focuses the existing window.
- **Data binding**: the column registry (`ColumnSpec` list with group, label, alias,
  availability), `[columns.<table>]`; the window is bound to the table that opened it and its
  title stays "tasklens Columns" (the list heading shows the table name as the first line, `t_sm`
  `text_secondary`: "Processes table").
- **Build**: `crates/tl-ui/src/pages/processes/columns_window.rs`, a `TlWindow` whose content is
  a `uniform_list` of 28 px rows (headings are rows too) with `TlCheckbox` children; filter via
  `TlInput` with 100 ms debounce.
- **Performance**: 96 rows at most; trivial.
- **Accessibility**: every row is a labelled checkbox; headings are plain text; the count line
  reports the result of each toggle.
- **Tests**: `picker_lists_groups_in_order`, `picker_name_always_on`, `picker_alias_filter_matches
  _both_parts` ("working set" and "memory" both find the Memory row), `picker_gpu_slots_only_for
  _present_adapters`, `picker_toggle_persists_and_restores_width`.
- **Phase**: P1.
- **Decided here**: window title "tasklens Columns" (APP-045 pattern, PROC-024 says "Columns"),
  size, row height 28, immediate apply, re-shown column position, availability note wording,
  table-name heading line.

### UI-PRC-008 Selection: visuals and gestures

- **Implements**: PROC-048; PROC-001 (identity); ADR-009 `table/selection.rs`.
- **Anatomy**:
  - Selected row: full-width fill `row_selected` (over zebra, tints and heat), radius 0, no
    border; text `text_selected_primary`, secondary parts `text_selected_secondary`; icons
    unchanged.
  - Focus row (keyboard caret) when it is not selected (after Ctrl+click toggled it off, or
    before any selection when the table has keyboard focus): 1 px `accent` inset ring on the row,
    `r_sm`.
  - Multi-selection: every selected row is filled; adjacent selected rows merge visually (no
    gap). No count badge in the table; the status bar says "3 processes selected" (UI-PRC-018).
  - Hover on a selected row: no change (selected wins over `row_hover`).
- **States**: none, single, multi, focus-without-selection, selection containing exited rows
  (they stay selected and filled until the linger time removes them), selection containing
  hidden rows (a selected identity that the filter hides stays in the set but is not drawn and
  not counted in "N processes selected"; decided, it reappears when the filter clears).
- **Interaction** (`Selection<RowId> { set: FxHashSet, anchor: Option<RowId>, focus:
  Option<RowId> }`):

  | Gesture | Result |
  |---|---|
  | Click | set = {row}, anchor = focus = row |
  | Ctrl+click | toggle row; focus = row; anchor = row when it was added |
  | Shift+click | set = rows between anchor and row in current view order (selectable rows only, skipping group headers); focus = row; anchor unchanged |
  | Ctrl+Shift+click | add the anchor..row range to the set |
  | Right-click on unselected row | set = {row} then open menu |
  | Right-click on selected row | keep set, open menu for the whole set |
  | Right-click on group header | no selection change; Expand/Collapse menu |
  | Click on empty body area below the last row | clear selection (decided) |
  | Ctrl+A | select every visible selectable row (process, app, unit, pseudo); anchor = first |
  | Esc | handled globally (table b); does not clear the selection |
  | Up/Down | focus moves one selectable row; set = {focus}; anchor = focus |
  | Shift+Up/Down | extend from anchor to the new focus |
  | PgUp/PgDn | move focus by (viewport rows − 1); set = {focus} |
  | Home/End | first / last selectable row; set = {focus} |
  | Ctrl+Space | toggle the focus row (decided, matches Ctrl+click) |

  Group header rows are never in the set; arrow keys skip them. Selection is kept by identity
  through re-sorts, filter changes and view-mode changes (PROC-015). A selected identity whose
  row leaves the view (collapsed parent) stays selected and is drawn again when expanded. Exited
  identities drop out after the linger time (PROC-043). Every selection change notifies the table
  only (narrowest entity).
- **Data binding**: `NodeId` identities from `ProcessTableView.rows`; `row_index` map for
  gesture arithmetic; nothing is persisted.
- **Build**: `crates/tl-ui/src/table/selection.rs` (pure logic, unit-tested without GPUI) and the
  row mouse handlers in `tree_table.rs` (`on_mouse_down` with `event.modifiers`, `click_count`
  for double-click).
- **Performance**: O(visible rows) fill checks per frame; range selection O(range).
- **Accessibility**: the keyboard model covers every gesture; the status bar reports counts.
- **Tests**: `selection_click_ctrl_shift_matrix` (table above), `selection_survives_resort`
  (ADR-009), `selection_skips_group_headers`, `selection_drops_exited_after_linger`,
  `selection_hidden_by_filter_not_counted`.
- **Phase**: P1.
- **Decided here**: empty-area click clears; Ctrl+Space; hidden selected rows not counted;
  focus ring only when the focus row is unselected.

### UI-PRC-009 Keyboard model

- **Implements**: PROC-049; table b (Table and ProcessTable contexts); D-008; PROC-020;
  PROC-044 (type-to-filter); PROC-050 (Ctrl hold, UI-PRC-017).
- **Anatomy**: no visible parts beyond the focus visuals of UI-PRC-008 and the `Kbd` hints that
  `TlMenu` draws from the bindings.
- **Bindings** (bound with `cx.bind_keys` before any menu is built; context strings exactly
  `Table` and `ProcessTable`; the process page div carries both):

  | Keys | Context | Action | Behaviour |
  |---|---|---|---|
  | Up, Down | Table | `MoveFocus { delta: ±1, extend: false }` | UI-PRC-008; keeps the focus row visible (UI-PRC-010) |
  | Shift+Up, Shift+Down | Table | `MoveFocus { delta: ±1, extend: true }` | extend |
  | PgUp, PgDn | Table | `MoveFocusPage { dir, extend }` | by viewport rows − 1 |
  | Home, End | Table | `MoveFocusEdge { end, extend }` | Shift variants extend |
  | Ctrl+A | Table | `SelectAll` | |
  | Right | Table | `ExpandOrChild` | collapsed row with children: expand; expanded: focus first child; leaf: nothing |
  | Left | Table | `CollapseOrParent` | expanded row: collapse; else: focus parent (group header counts as parent of depth-1 rows only for collapsing: Left on a depth-1 row in Grouped mode collapses its group; decided) |
  | Space | Table | `ToggleExpand` on the focus row | no-op on leaves |
  | Ctrl+Space | Table | `ToggleSelectFocus` | decided |
  | Enter, Alt+Enter, Ctrl+I | Table | `OpenProperties` | one window per selected process, cap 8 (PROC-077); on a group header: `ToggleExpand` |
  | Ctrl+C | Table | `CopyDetails` | PROC-070 |
  | Ctrl+Shift+C | Table | `CopyRowsAsTable` | |
  | Ctrl+L | Table | `CollapseAll` | PROC-020 |
  | Shift+F10, Menu | Table | `OpenContextMenu` | at the focus row's Name cell, x = text start, y = row bottom |
  | Delete, Ctrl+E | ProcessTable | `EndTask` | targets = selection |
  | Shift+Delete, Ctrl+X | ProcessTable | `EndProcess` | |
  | printable character (no modifier, or Shift) | ProcessTable | `StartFilter { ch }` | focuses the filter input with the character inserted (UI-PRC-016); ignored when an input, menu or dialog has focus |
  | Ctrl+F | Global | `FocusFilter` | |
  | Esc | Global | shell rule (table b) | first closes menu or dialog; else clears filter and focuses the table |
  | Hold Ctrl | Global | pause (UI-PRC-017) | |

  Tab and Shift+Tab move between the filter input, the Include children checkbox and the table
  (gpui-kit Root handles Tab; the body has `tab_index(0)` and `tab_stop(true)`). Arrow keys
  never move between columns.
- **States**: table focused with a focus row; focused without a row (first Down selects the
  first selectable row); unfocused (keys go nowhere; clicking a row focuses the table).
- **Interaction**: actions carry no row id; they read the table's `selection.focus` and set at
  dispatch time and then capture identities exactly as the menu does (PROC-001).
- **Data binding**: none beyond the selection.
- **Build**: `pages/processes/actions.rs` (`actions!` list and data actions), bindings in
  `pages/processes/mod.rs::bind_keys`, handlers via `.on_action(cx.listener(..))` on the page
  div that also holds `track_focus(&table.focus)`.
- **Performance**: n/a.
- **Accessibility**: this is the accessibility path; every menu item shows its `Kbd` hint; the
  shell's Keyboard Shortcuts window lists table b.
- **Tests**: `keys_move_focus_and_skip_group_headers`, `right_left_expand_collapse_parent`,
  `enter_opens_properties_space_toggles` (D-008), `printable_starts_filter_unless_input_focused`,
  `delete_dispatches_end_task_with_selection`.
- **Phase**: P1.
- **Decided here**: Left on a depth-1 row collapses its group; Ctrl+Space; context menu anchor
  point for Shift+F10; first Down selects the first row.

### UI-PRC-010 Scrolling, scroll anchoring and visible-range demand

- **Implements**: PROC-013 (uniform_list, demand reporting), PROC-071 (scroll the jump target to
  the centre), ADR-009 "Visible demand", ADR-014 `table-2000` scroll scene.
- **Anatomy**: `uniform_list` body with `UniformListScrollHandle`; vertical `TlScrollbar` 12 px
  on the right edge of the body (not over the header); horizontal `TlScrollbar` 12 px at the
  bottom when needed; the header follows `x_offset`.
- **Behaviour**:
  - Wheel: GPUI default line scrolling; `motion.smooth_scroll` and `smooth_scroll_amount` are
    applied by the shell's wheel handler (foundation), the table does nothing extra.
  - Scroll anchoring when rows move (re-sort, new rows above the viewport, group expand above):
    after a new table version is installed, if the previously top-most visible selectable row
    identity still exists, the list is scrolled so that identity keeps its previous y (using
    `scroll_to_item_with_offset(ix, ScrollStrategy::Top, offset_rows)`); otherwise the scroll top
    index is kept. Exception: while the user drags the scrollbar thumb or within 200 ms of a wheel
    event, anchoring is skipped so programmatic scrolls never fight input (decided). While paused
    (UI-PRC-017) nothing moves.
  - Keep selection visible: after a keyboard focus move, `scroll_to_item(ix,
    ScrollStrategy::Nearest)` with one extra row of margin (decided: `_with_offset(.., 1)`), so
    the focus row never sits half under the header or bottom edge. Mouse selection never scrolls.
  - Jump target (PROC-071): `scroll_to_item_strict(ix, ScrollStrategy::Center)` after expanding
    the path, then select.
  - Visible-range demand: the `uniform_list` range callback stores `visible_range`; in the
    page's `after_layout` step (an `on_next_frame` scheduled once per render) the table computes
    identities for `visible_range` widened by one viewport above and below, capped at 512, and
    calls `on_visible_range` only if the identity set changed (compared by a 64-bit hash of the
    sorted keys; decided). It runs again after every table version even when the range did not
    change (ADR-009). The delegate forwards to `DemandAggregator` (`demand.rs`) as
    `Demand::ProcessDetail { keys }`.
  - Scrollbar drag is not frame-capped by the table (kit `Scrollbar.max_fps` is left at
    default).
- **States**: at top (no top shadow), scrolled (a 1 px `divider` line under the header is already
  present; no extra shadow, decided), at bottom, horizontally scrolled (header moves with the
  body), scrollbar hover (thumb 100% opacity) and drag.
- **Interaction**: wheel, scrollbar drag, keyboard moves, programmatic jumps.
- **Data binding**: `visible_range` → `DemandAggregator`; nothing persisted.
- **Build**: `tree_table.rs` (`scroll: UniformListScrollHandle`, `anchor: Option<(RowId,
  usize /*rows from top*/)>` recorded in the range callback), `demand.rs` sink.
- **Performance**: anchoring costs one hash lookup per table version; demand hashing is O(512).
  At 1,200 px/s the visible range changes every frame, so the demand set is hashed per frame
  (≤ 512 keys, ~10 µs) but sent only when changed.
- **Accessibility**: keyboard moves always keep the focus row visible.
- **Tests**: `scroll_anchor_keeps_top_row_on_resort` (sort flips, top identity stays at same y),
  `scroll_anchor_skipped_during_wheel`, `focus_move_scrolls_nearest_with_margin`,
  `jump_target_centres_row`, `visible_demand_capped_512_and_sent_on_change_only`.
- **Phase**: P1.
- **Decided here**: anchoring rule and its input exception; margin row; demand change detection
  by hash; no header shadow.

### UI-PRC-011 New and exited row highlight

- **Implements**: PROC-043; R-041; ADR-009 highlight bullet.
- **Anatomy**: a full-row quad over the zebra colour and under heat and selection:
  - NEW: `status.success` at 35% alpha.
  - EXITED: `status.danger` at 35% alpha; all text in the row switches to `text_secondary`;
    icons at 60% opacity; the chevron stays.
  - Linger `L = clamp(2 × processes.interval_ms, 1000, 3000)` ms from the flag's first
    appearance (engine timestamp in `ProcFlags`, not UI time). Fade: during the last 300 ms the
    alpha goes 35% → 0 with `ease_out_cubic`; with `motion.animations` off or system reduce
    motion on, the tint stays at 35% and switches off at `L`.
  - Exited rows are not actionable: every menu item except Copy, Copy details, Copy all metrics
    and Copy rows as table is disabled with the tooltip "Process exited" (PROC-057 says "except
    Copy"; the three other copy commands only read snapshot text and are kept enabled, decided
    and listed below). Double-click and Enter show the toast "Process exited" instead of opening
    Properties (decided).
- **States**: new, exited, new-and-selected (selection fill wins, tint hidden), exited-and-
  selected (selection fill with `text_selected_secondary` text), setting off
  (`processes.highlight_new_exited = false`: no tints; exited rows still linger and are still
  not actionable, only the colour goes).
- **Interaction**: none. The fade is driven by the `AnimationDriver` (ADR-009): the table requests
  frames only while at least one visible row is inside its 300 ms fade window; otherwise the
  tint is static and costs nothing.
- **Data binding**: `ProcFlags::NEW | EXITED` plus `flag_since_ns` per record; setting
  `processes.highlight_new_exited`, `motion.animations`, `App::reduce_motion()`.
- **Build**: `table/cells.rs::paint_row_tint`; fade alpha computed from `now_boot_ns()` in
  paint; frame requests through `anim.rs::AnimationDriver::request`.
- **Performance**: one quad per flagged visible row; the driver is armed only during fades
  (≤ 300 ms per row event).
- **Accessibility**: the exited state is also carried by the `text_secondary` text and the
  "Process exited" tooltip, not colour alone.
- **Tests**: `highlight_alpha_timeline` (35% until L−300 ms, 0 at L, reduce-motion step),
  `exited_rows_disable_actions_except_copy`, `driver_idle_when_no_fade_active`.
- **Phase**: P1.
- **Decided here**: copy commands stay enabled on exited rows; Enter on an exited row toasts;
  icon opacity 60% on exited rows.

### UI-PRC-012 Row hover tooltip

- **Implements**: PROC-039; R-009; PROC-012 (pseudo-row tooltip).
- **Anatomy**: `TlTooltip` (gpui-kit `Tooltip::element`) anchored below the pointer, max width
  560, padding `sp4` 8, `menu_bg` fill, 1 px `divider` border, `r_sm`, `t_sm` 12 text
  `text_primary`, monospace is not used. Lines in the PROC-039 order, each a separate `div`
  (so wrapping stays per line), labels in `text_secondary` and values in `text_primary`:

  ```text
  /usr/lib/firefox/firefox --name firefox …        (command line, first 512 chars, wraps)
  path: /usr/lib/firefox/firefox
  user: oystein (uid 1000)
  Unit: app-firefox@1234.service (Firefox)
  cgroup: /user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@1234.service
  Package: firefox 143.0-1                          (P2)
  Desktop file: /usr/share/applications/firefox.desktop   (app rows)
  Some values need administrator access.           (restricted rows, status.warning)
  ```

  Pseudo rows: one line, "CPU time spent in hardware and software interrupts" or "Memory held
  by zram compressed swap devices". Group headers: no tooltip. Kernel threads: command line
  line is replaced by `[comm]`.
- **States**: shown after 600 ms of hover over the Name cell with no mouse-down; hidden on
  mouse-down, on leaving the cell, on scroll and on any key. No grace switching between rows
  (GPUI tooltip path, kit note).
- **Interaction**: hover only; the command line is selectable nowhere (copy through the menu).
- **Data binding**: `ProcessMeta` (cmdline, exe, user, unit, cgroup, package, desktop file) and
  `RESTRICTED` flag; built lazily on show.
- **Build**: `.id(("name", node_id)).tooltip(move |w, cx| Tooltip::element(..).build(w, cx))
  .tooltip_show_delay(Duration::from_millis(600))` on the Name cell div.
- **Performance**: the tooltip view is created on show only; the Name cell id is the only
  per-row element id.
- **Accessibility**: the same text is in "Copy all metrics".
- **Tests**: `row_tooltip_lines_in_order_and_optional` (restricted row adds the last line, app row
  adds Desktop file), `row_tooltip_truncates_cmdline_512`.
- **Phase**: P1 (Package line P2).
- **Decided here**: max width 560, label and value colours, pseudo-row sentences, kernel-thread
  first line.

### UI-PRC-013 Row context menu

- **Implements**: PROC-057; PROC-012 (pseudo), PROC-043 (exited), PROC-048 (multi-select
  labels), PROC-054, PROC-060 to PROC-075, PROC-093, PROC-094, PROC-071, PROC-072, PROC-073,
  PROC-074; R-033, R-034, R-091.
- **Anatomy**: `TlMenu` (`PopupMenu`) opened at the pointer (or at the focus row for
  Shift+F10), min width 240, max width 360, item height 28, padding `sp2` 4 vertical, text
  `t_base` `text_primary`, disabled `text_secondary`, hover `menu_hover`, separators 1 px
  `divider` with `sp2` margins, submenu chevron `ChevronRight` 12 px, `Kbd` hints right-aligned
  `text_secondary`. Items, in order, with the rule for each row type. "P" = process row, "A" =
  app row, "U" = unit row, "M" = multi-select (2+ rows, any mix), "X" = exited row, "S" = pseudo
  row, "G" = group header. "hidden" means not built; "disabled" means built with a tooltip.

  | # | Label (P) | A / U label | M label | Action | Hidden when | Disabled when (tooltip) |
  |---|---|---|---|---|---|---|
  | 1 | End Task | End App | End Selected Tasks | `EndTask` | S, G | X ("Process exited") |
  | 2 | End Process | Kill App (all processes) | End Selected Processes | `EndProcess` (A/U: `KillApp`) | S, G; A/U before P2 | X |
  | 3 | End Process Tree | (hidden) | (hidden on M) | `EndProcessTree` | A, U, M, S, G | X |
  | | separator | | | | | |
  | 4 | Suspend Process or Resume Process (one of them, by State T) | Freeze App or Thaw App (by Frozen) | Suspend Selected Tasks or Resume Selected Tasks (Resume when every target is stopped, else Suspend; decided) | `SuspendProcess` / `ResumeProcess` / `FreezeApp` / `ThawApp` | S, G; A/U before P2 | X |
  | 5 | Send Signal > | same (primaries) | same | submenu UI-PRC-020 | S, G; before P2 | X |
  | 6 | Set Priority > | same (tooltip "Applies to the app's current processes") | same | submenu UI-PRC-020 | S, G | X |
  | 7 | Set Affinity… | same | same (P2) | `SetAffinity` opens UI-PRC-021 | S, G; M before P2 | X |
  | 8 | Set I/O Priority > | same | same | submenu UI-PRC-020 | S, G; before P2 | X |
  | | separator | | | | | |
  | 9 | Show Window | same (any member window) | hidden | `ShowWindow` | S, G, M; no KWin bridge | X; no window ("No window") |
  | 10 | Restart Plasma Shell / Restart unit "<unit>" | Restart unit "<unit>" (U only) | hidden | `RestartUnit { unit }` | unless plasmashell row or a unit's MainPID; A, S, G, M; before P2 | X |
  | | separator | | | | | |
  | 11 | Copy | same | same | `CopyCell { col }` | G | never |
  | 12 | Copy details | same | same | `CopyDetails` | G | never |
  | 13 | Copy all metrics | same | same | `CopyAllMetrics` | G | never |
  | 14 | Copy rows as table | same | same | `CopyRowsAsTable` | G, S | never |
  | | separator | | | | | |
  | 15 | Add Minigraph > | same | hidden | submenu: CPU, Memory, Disk, Network, GPU, VRAM → `AddMinigraph { column }` | G, M; before P3 | X |
  | 16 | Add Process Name Graph > | hidden | hidden | `AddNameGraph { column }` | A, U, S, G, M; before P3 | X |
  | 17 | Add App Name Graph > | same (U: unit name) | hidden | `AddAppGraph { column }` | P without app, S, G, M; before P3 | X |
  | 18 | Add tray icon graph > or Remove tray icon graph | same | hidden | `AddTrayGraph { column }` / `RemoveTrayGraph` | S, G, M; before P3 | X |
  | | separator | | | | | |
  | 19 | Properties | same (App Properties) | same (one window each, cap 8) | `OpenProperties` | S, G; before P2 | X |
  | 20 | File properties | hidden | hidden | `FileProperties` | A, U, S, G, M; no FileManager1 service; before P2 | X; exe deleted ("Executable deleted") |
  | 21 | Open file location | hidden | hidden | `OpenFileLocation` | A, U, S, G, M | X; exe deleted ("Executable deleted") |
  | 22 | Open in VirusTotal | hidden | hidden | `OpenVirusTotal` | A, U, S, G, M; before P2 | X |
  | 23 | Create dump file | hidden | hidden | `CreateDump` | A, U, S, G, M; before P3 | X; no gdb ("Install gdb to create dumps") |
  | | separator | | | | | |
  | 24 | Go to parent process | hidden | hidden | `GoToParent` | A, U, S, G, M; ppid 0 | X |
  | 25 | Go to service | same (U only) | hidden | `GoToService` | unless MainPID or U; A, S, G, M; before P2 | X |

  Group header rows: a two-item menu "Expand" / "Collapse" (the one that applies) →
  `ToggleExpand`. Pseudo rows: items 11, 12, 13 and 15 only. Items whose phase has not shipped
  are not built. Leading, trailing and doubled separators collapse (kit behaviour).
  Items 1 to 3 on a protected target keep their label; the confirmation (UI-PRC-014) carries the
  warning. Items 9 and 17 for app rows use the app's PID set.
- **States**: per `TlMenu`; disabled items keep their `Kbd` hint; submenus open on hover after
  150 ms or on Right/Enter.
- **Interaction**: opens on right mouse-up over a row (right-click selects first, UI-PRC-008),
  on Shift+F10 and Menu. On open the menu captures `targets: SmallVec<[NodeId; 1]>` (the
  selection set at that moment, max 256; for item 3 also the subtree keys from the engine
  forest) and `clicked_col`; every item action carries nothing and reads the captured targets
  from the menu's `MenuTargets` state (PROC-001). Esc closes and returns focus to the table.
- **Data binding**: `TableRow` kind, flags (State T, Frozen, EXITED, RESTRICTED), `ppid`,
  `WindowsSnapshot`, `UnitRef` MainPID, FileManager1 presence (engine capability bit), gdb
  presence, tray graph existence for the row's source.
- **Build**: `pages/processes/menu.rs::row_menu(targets, clicked_col, caps) -> TlMenu`; kit
  `PopupMenu` with `.menu`, `.menu_with_disabled`, `.submenu`, `.separator`; `.action_context
  (table.focus)` so `Kbd` hints resolve against the ProcessTable context.
- **Performance**: built on open; ≤ 30 items.
- **Accessibility**: full keyboard operation; every disabled item explains itself in a tooltip.
- **Tests**: `row_menu_items_per_row_kind` (one assertion per row type and multi-select,
  labels and disabled states as in the table), `row_menu_targets_captured_on_open` (rows move
  after open, action still hits the captured keys), `row_menu_hides_unshipped_phases`.
- **Phase**: P1 for the menu and P1 items; other items with their entries.
- **Decided here**: multi-select Suspend/Resume choice rule; min and max width; submenu hover
  delay; copy items enabled on exited rows.

### UI-PRC-014 Confirmation dialogs

- **Implements**: PROC-059, PROC-062, PROC-064, PROC-066 (Realtime), PROC-075 (large dump),
  PROC-074 (VirusTotal notice), PROC-055 (picker Terminate), D-009, R-036.
- **Anatomy**: `TlDialog` (gpui-kit `AlertDialog` through `open_alert_dialog`), width 448,
  title `t_lg` 16 semibold, description `t_base` `text_primary`, buttons right-aligned, the
  destructive button `danger` variant, Cancel `default`. Enter triggers the confirm button
  (decided: no, see below), Esc cancels. Window title for every confirmation is "tasklens"
  (APP-045). Texts, exact:

  | Trigger | Title | Description | Confirm | Cancel |
  |---|---|---|---|---|
  | signal, freeze or kill on a PROTECTED target | End <name>? | Ending <name> will end your desktop session. End it anyway? | End anyway (danger) | Cancel |
  | End Process Tree | End process tree? | End N processes in this tree? | End tree (danger) | Cancel |
  | Kill App | Kill <App>? | Kill all N processes of <App>? | Kill app (danger) | Cancel |
  | End Task or End Process with 2+ targets | End N processes? | End N selected processes? (SIGTERM) / End N selected processes immediately? (SIGKILL) | End tasks / End processes (danger) | Cancel |
  | any action on another user's process | Act on another user's process? | <Action> on <name> (PID n), owned by <user>. Continue? | <Action label> (danger for End and Kill, primary otherwise) | Cancel |
  | Set Priority > Realtime | Set realtime priority? | Realtime priority can freeze the desktop if the process spins. Set it anyway? | Set realtime (danger) | Cancel |
  | Create dump file, VmRSS > 2 GiB | Create a large dump? | <name> uses 3.4 GiB. The process pauses while the core is written and the file will be about that size. Continue? | Create dump (primary) | Cancel |
  | Open in VirusTotal, first use | Open VirusTotal? | This opens virustotal.com with the file's hash. The file is not uploaded. | Open (primary) | Cancel |
  | Window picker Ctrl+click, modifier unknown | Picked <name> (PID n) | What do you want to do with this process? | Select (primary), Terminate (danger) | Cancel |

  Where two rules match (for example End Process Tree on a protected target), the protected
  text wins and N is still shown: "Ending <name> will end your desktop session. End N
  processes in this tree anyway?" (decided).
- **States**: open (modal to its window, overlay 40% black, not overlay-closable, no X button);
  while the action runs the dialog is already closed and the toast carries progress.
- **Interaction**: Enter does not trigger the danger button; Enter is ignored and the focus
  starts on Cancel (decided, protects against a stray Enter on a session-ending action). Tab
  moves between the buttons. Esc cancels. The confirm button dispatches the original action
  with `confirmed = true` so the handler proceeds without re-asking.
- **Data binding**: target names, counts and owners from the captured targets at open time;
  revalidation still happens after confirm (PROC-058).
- **Build**: `pages/processes/confirm.rs::confirm(kind, targets, w, cx)` returning the
  `AlertDialog` builder; `.confirm().ok_text(..).ok_variant(ButtonVariant::Danger)
  .on_ok(..)`.
- **Performance**: n/a.
- **Accessibility**: title and description are plain text; buttons name the result; focus
  starts on Cancel.
- **Tests**: `confirm_text_per_trigger` (table above), `confirm_focus_starts_on_cancel_and_enter
  _ignored`, `single_end_process_own_unprotected_has_no_dialog` (D-009),
  `protected_wins_over_tree_text`.
- **Phase**: P1 (dump and VirusTotal with their entries).
- **Decided here**: button labels, titles, focus on Cancel, Enter ignored, combined protected
  plus tree text, multi-target descriptions.

### UI-PRC-015 Toasts for process actions

- **Implements**: PROC-058, PROC-060 to PROC-075, PROC-093, PROC-094, PROC-015, PROC-045,
  PROC-046, PROC-026, PROC-070, PROC-071, PROC-077 (cap toast), R-035.
- **Anatomy**: `TlToast` at the top centre of the main window (foundation: width 382, top
  margin 16 under the tab strip, `menu_bg`, 1 px `divider`, `r_md`, `t_base`, left icon by
  type, optional one `TlButton` small on the right). One toast per action request, identified
  by `id1::<ActionToast>(request_id)`, so later states replace it in place. Auto-hide 5 s
  (kit fixed) except toasts with a button, which stay until clicked or closed (kit rule) and
  are closed by tasklens after 30 s (decided). Types: info for "requested", success for
  done, error for failures, warning for identity and permission problems.

  | Event | Text | Type | Button |
  |---|---|---|---|
  | End Task sent | End task requested. | info | |
  | process gone | Process terminated | success | |
  | End Task failed | Could not end task. | error | |
  | End Process sent | End process requested. | info | |
  | End Process failed | Could not end process. | error | |
  | End Process Tree sent | End process tree requested. | info | |
  | tree done | Process tree terminated | success | |
  | multi-target End sent | End task requested for selected processes. | info | |
  | multi-target result | Ended 4 of 5 processes (1 permission denied) | warning when any failed, else success | Retry as administrator (P3, when EPERM) |
  | Suspend / Resume | Process suspended. / Process resumed. | success | |
  | Freeze / Thaw / Kill App | App frozen / App thawed / App killed | success | |
  | priority | Priority updated | success | |
  | priority, multi | Selected process priority updated. / Could not update selected process priority. / Priority updated for 3 of 4 processes. 1 failed: permission denied | success / error / warning | Retry as administrator (P3) |
  | affinity | Affinity updated / Could not update selected process affinity. / Selected process affinity updated. | success / error / success | |
  | I/O priority | I/O priority updated / Selected process I/O priority updated. / Could not update selected process I/O priority. | success / success / error | |
  | signal | Sent SIGHUP to firefox (1234) | success | |
  | identity mismatch | The selected process changed; wait for the list to update and try again. | warning | |
  | EPERM | Permission denied | error | Retry as administrator (P3 only) |
  | deadline passed | Outcome unknown | warning | |
  | refused | Refusing to end tasklens itself. / PID 1 (systemd) cannot be ended. / Kernel threads cannot be signalled. | warning | |
  | Show Window | Process window shown. / No window found | success / warning | |
  | Window features without KDE | Window features need KDE Plasma | warning (once per session) | |
  | Restart Plasma Shell | Restarting Plasma Shell... / Plasma Shell restarted / Plasma Shell restart is already in progress. | info / success / warning | |
  | Copy | Copied to clipboard / Selected process values copied. / Process details copied. / Copied metrics to clipboard / Selected process metrics copied. / Failed to copy to clipboard | success / error | |
  | Open file location | Opening file location / Process image unavailable | info / warning | |
  | VirusTotal | Computing SHA-256… (after 500 ms) / SHA-256 unavailable / Process image unavailable | info / error / warning | |
  | dump | Writing core… 1.2 GiB (progress, replaced) / Dump created: <path> / Creating a dump needs administrator rights | info / success / error | Open folder |
  | jump target | Switched to Processes tab / Switched to parent process / Parent process not found in current view / Process not found in current view / <caller text> | info / warning | Show (when hidden by filter or Hide system) |
  | Go to service | Switched to Services tab / Switched to service. / Service not found in current view | info / warning | |
  | Properties | Could not open properties. / Too many process properties windows are already open | error / warning | |
  | view mode | View mode: Tree View / View mode: Task Manager Grouping / View mode: List View | info | |
  | hide system | Hiding system processes / Showing system processes | info | |
  | include children | Including filtered process children. / Hiding unmatched filtered process children. | info | |
  | columns | At least one column must stay visible / Columns reset to default | warning / info | |
  | minigraphs | Minigraph added / Minigraph already exists / No space for more minigraphs / Process name minigraph added / App name minigraph added / Unable to add minigraph | success / warning / warning / success / success / error | |
  | tray graphs | Tray icon graph added / Tray icon graph removed / Tray icon graph already exists / Unable to add tray icon graph / Unable to remove tray icon graph | success / success / warning / error / error | |
  | exited row activated | Process exited | warning | |
  | history | see UI-PRC-036, UI-PRC-037 | | |

- **States**: entering (400 ms kit), visible, counting down (paused while hovered), replaced,
  exiting (200 ms). With `motion.animations` off or reduce motion: no slide, instant show and
  hide.
- **Interaction**: hover pauses; click on the body closes; the button runs its action and
  closes. "Retry as administrator" re-sends the same `ActionRequest` with `privileged = true`
  and only the failed targets. "Show" clears the filter, turns Hide system processes off and
  selects the row. "Open folder" opens the dumps directory through `FileManager1.ShowItems`.
- **Data binding**: `action_events` channel (ADR-005) consumed by `actions.rs::ActionTracker`,
  which owns the toast ids and texts.
- **Build**: `TlToast::show(window, kind, text, button)` → `window.push_notification
  (Notification::<type>(text).id1::<ActionToast>(request_id).action(..), cx)`.
- **Performance**: ≤ 10 toasts live (kit cap).
- **Accessibility**: text first, icon second; buttons are focusable; nothing relies on colour.
- **Tests**: `action_toast_replaces_in_place_per_request`, `toast_texts_match_spec` (string
  table), `retry_button_only_from_p3_and_only_on_eperm`.
- **Phase**: P1 (texts arrive with their features).
- **Decided here**: 30 s cap for button toasts; toast types per event.
