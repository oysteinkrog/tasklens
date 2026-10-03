# tasklens UI spec: Services, Startup Apps, Connections and Information tabs

This file is the detailed UI spec for four tabs and their secondary windows. ID prefix: `UI-SYS`.
An implementer should be able to type the code from it without making design decisions.

Sources, in order of precedence: `docs/DECISIONS.md`, `docs/RECONCILIATION.md` (R entries and
canonical tables a to d), `docs/ARCHITECTURE.md` (ADR-003, ADR-005, ADR-008, ADR-009, ADR-010),
then the merged specs `docs/spec/merged/sys-app.md` (SYS-001 to SYS-037, APP-041, APP-059) and
`docs/spec/merged/perf.md` (PERF-094 to PERF-104, PERF-013).

Sibling UI files, written at the same time:
- `docs/ui/foundation.md`: tokens, the `Tl*` components, focus ring, toasts, secondary window frame.
- `docs/ui/processes.md`: `TlTreeTable` (header, rows, selection, column picker, group rows), the
  process row renderer and the process context menu. This file gives only the delegate details.
- `docs/ui/shell.md`: title bar, tab strip, filter box and status bar frame.

If a component or action name here differs from the one in those files, theirs wins and this file
is corrected to match. The names this file assumes are listed in "Names used from other files".

## Conventions

- Sizes are logical px at 100% UI scale. Every size is multiplied by `ui_scale_pct / 100`.
  Row height is `row_h = 28 × row_height_pct/100 × ui_scale_pct/100` (R-020).
- Spacing tokens `sp1`..`sp8`, text tokens `t_xs`..`t_xl`, radii `r_sm`, `r_md`, motion `m_fast`,
  `m_base`, `m_slow` and the colour tokens are the shared vocabulary from the brief. Colour tokens
  are `TlTheme` fields; status colours are `status.success`, `status.warning`, `status.danger`,
  `status.info`.
- "Measured" means read from `docs/taskslinger-reference/information.png` or `processes.png`.
  Both are 2720 × 1680 physical px. The window is TaskSlinger's 1280 × 800 default at a device
  scale near 2.1, so 1 logical px is about 2.1 physical px. Measured values are rounded to the
  nearest spacing token and say so. "Spec" means taken from a merged spec entry. "Chosen" means
  decided in this file; every chosen value is also in the "Decided here" list at the end.
- Missing values: table cells use the ADR-003 glyphs (`…`, lock icon, `n/a`, `-`, `!`).
  Key/value fields in Properties windows and Information rows use the PERF-013 words
  ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable",
  "Unknown") in `text_secondary` (R-050). Never a zero.
- Numbers follow canonical table (d): tabular figures (`tnum`), right-aligned in tables, thin-space
  (U+2009) grouping for counts, percent `%.1f%%`, sizes IEC two decimals.
- Strings in double quotes are exact UI text. A string marked "(new)" is written by this file and
  is not in the merged specs; it is listed in "Decided here".
- Action names are GPUI `Action` types. They are written `module::Name` and live in
  `crates/tl-ui/src/actions/` (one module per page). Menu items and keys dispatch the same Action
  (widgets rule 10).
- Snapshot component bits are written `ComponentMask::SERVICES`, `::STARTUP`, `::CONNECTIONS`,
  `::INVENTORY`, `::PROCESSES`, `::SYSTEM`, `::HEALTH`, `::DEVICES` (one bit per `Snapshot`
  component, ADR-005).

## Names used from other files

| Name | Owner | Used for |
|---|---|---|
| `TlTreeTable<D: TlTreeDelegate>` | processes.md | the three system tables |
| `TlTreeDelegate` hooks `row_count`, `row`, `paint_cell`, `context_menu`, `on_activate`, `tooltip` | processes.md | delegate details below |
| `TlColumnPicker` ("Columns" picker) | processes.md | View > Columns for the three tables (APP-018) |
| shared table actions `table::OpenProperties`, `table::CopyCell`, `table::CopyRow`, `table::CopyRowsAsTable`, `table::SelectAll`, `table::CollapseAll`, `table::OpenContextMenu` | processes.md | same keys in every table |
| process row renderer `ProcessRowCells` and process context menu `process_menu(..)` | processes.md | service child rows |
| `TlButton`, `TlIconButton`, `TlToggleChip`, `TlSelect`, `TlCheckbox`, `TlInput`, `TlNumberInput` | foundation.md | controls |
| `TlMenu` (context, dropdown and submenu) | foundation.md | menus |
| `TlToast` (id, kind, text, optional action button) | foundation.md | toasts (APP-044) |
| `TlDialog` (modal confirm window) | foundation.md | confirmations (APP-045) |
| `TlSecondaryWindow` (pinnable top-level window frame with title, pin, close) | foundation.md | Properties windows |
| `TlTabStrip` (inner tabs of a secondary window) | foundation.md | Properties tabs |
| `TlKeyValueGrid` (label/value rows, validity words, copy affordance) | foundation.md | Properties and Information rows |
| `TlSectionHeader` (accent bar, title, trailing buttons) | foundation.md | Information sections |
| `TlSkeleton` | foundation.md | first-load placeholders (R-060) |
| `TlEmptyState` (icon, heading, text, optional button) | foundation.md | empty and error states |
| `TlBanner` (info or warning strip inside a pane) | foundation.md | journal access banner |
| `TlLink` | foundation.md | clickable values |
| `TlTag` | foundation.md | relation tags in Dependencies |
| `StatusBarText` slot | shell.md | left status bar text per tab (APP-029) |
| `FilterBox` per-tab text | shell.md | the tab filter (APP-028) |

---

# A. Shared parts of the three system tables

### UI-SYS-001 System table page frame

- **Implements**: SYS-001, SYS-004, SYS-016, SYS-027, SYS-028, SYS-036, APP-029, APP-059, ADR-009
  (page entities), R-060.
- **Anatomy** (Services shown; Startup Apps and Connections have no filter strip):

  ```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ tab strip and filter box (shell.md)                                       │
  ├──────────────────────────────────────────────────────────────────────────┤
  │ filter strip, 28 px (Services only, UI-SYS-011)                           │
  ├──────────────────────────────────────────────────────────────────────────┤
  │ table header, 1 line, 28 px                                               │
  ├──────────────────────────────────────────────────────────────────────────┤
  │ table body (uniform_list), rows row_h                                     │
  │ ...                                                                       │
  ├──────────────────────────────────────────────────────────────────────────┤
  │ status bar, 22 px (shell.md), left text from this page                    │
  └──────────────────────────────────────────────────────────────────────────┘
  ```

  | Part | Size | Colours | Notes |
  |---|---|---|---|
  | Page root | fills the content area | `window_bg` | `v_flex`, `size_full`, no padding |
  | Filter strip | full width × 28 | `toolbar` background, 1 px `divider` bottom border | UI-SYS-011 |
  | Header | full width × 28 (one line; chosen) | `header` background, `divider` bottom border, `text_primary` `t_base` medium | the three tables have no totals line, so `TlTreeTableConfig::header_lines = 1` |
  | Body | rest | `list_bg`, alternate rows `row_alt` | `TlTreeTable` body, not cached (owns scrolling) |
  | Status bar text | in the shell's 22 px bar | `text_secondary` `t_sm` | UI-SYS-022, UI-SYS-049, UI-SYS-067 |

- **States**:
  - Loading (first data not yet in this session): UI-SYS-004.
  - Empty: UI-SYS-005.
  - Ctrl held while the main window is active: the table renders from the paused snapshot and the
    status bar appends "[PAUSED - Ctrl]" in `accent` (R-042). The shell owns the text; the page
    reads `SnapshotHub::view_snapshot()` which returns the paused one while paused.
- **Interaction**: the page has key context `ServiceTable` (Services), `StartupTable` (Startup Apps,
  chosen), `ConnectionTable` (Connections, chosen), each nested inside `Table`. All three also get
  the `Table` bindings of table (b). F5 dispatches `page::Refresh`, handled per page (UI-SYS-022,
  UI-SYS-049, UI-SYS-067). A printable key with the table focused starts filtering (shell
  `FilterBox`).
- **Data binding**:
  - Services: `Snapshot.services` (`ComponentMask::SERVICES`), plus `::PROCESSES` only while a
    service row is expanded (child rows).
  - Startup Apps: `Snapshot.startup` (`::STARTUP`).
  - Connections: `Snapshot.connections` (`::CONNECTIONS`), plus `::PROCESSES` for owner names and
    icons.
  - On first visit and on every tab activation the page sets its demand flag
    (`Demand.services`, `.startup`, `.connections`) through `DemandAggregator`, and clears it when
    the tab is left. On first visit it also sends `Command::RefreshNow(SourceId::…)` (ADR-005).
- **Build**: one entity per page in `crates/tl-ui/src/pages/services.rs`, `startup.rs`,
  `connections.rs` (ADR-009). Each page owns an `Entity<TlTreeTable<…Delegate>>` and, for Services,
  the filter strip state. The page root is embedded with `.cached(size_full)`; the table body is not
  cached. Entities, subscriptions and focus handles are created in `new`.
- **Performance**: the page re-renders only when its mask bits change. The engine sorts and
  filters (UI-SYS-002); the UI formats only visible cells through `CellTextCache`.
- **Accessibility**: Tab order: filter strip controls (Services), then the table. The table shows
  the shared focus ring when focused (foundation.md).
- **Tests**: `sys_page_sets_demand_only_while_visible` (switch tabs, assert `Demand` flags);
  `sys_page_not_rerendered_for_unrelated_mask` (install a snapshot that changes only `SYSTEM`,
  assert the page's render counter did not move).
- **Phase**: P2.
- **Decided here**: one-line 28 px header for the three tables; key contexts `StartupTable` and
  `ConnectionTable`.

### UI-SYS-002 Table query and delegate pattern

- **Implements**: SYS-004, SYS-017, SYS-027, SYS-036, R-011, ADR-009 (engine sorts and filters).
- **Anatomy**: not visual. Each system table follows the process table's split: the engine builds
  sorted, filtered, flattened rows; the UI keeps expansion, selection and the query.

  ```rust
  // tl-model (new, chosen): one query type for the three system tables
  pub enum SysTable { Services, Startup, Connections }
  pub struct SysTableQuery {
      pub table: SysTable,
      pub generation: u64,
      pub sort: SortSpec,                 // column id + Asc/Desc
      pub filter: Arc<str>,               // lowercase text filter from the FilterBox
      pub expanded: Arc<[SysNodeId]>,     // UI-owned expansion
      pub services: Option<ServiceFilter>,// Services only (UI-SYS-011)
  }
  pub enum SysNodeId {
      ServiceGroup(Manager),              // System | User
      Unit(UnitRef),
      UnitProcess(UnitRef, ProcessKey),   // child process row under a unit
      Startup(StartupRef),
      Conn(u64),                          // socket cookie
  }
  pub struct SysRow { pub node: SysNodeId, pub depth: u8, pub expanded: bool,
                      pub has_children: bool, pub record: Option<u32> }
  ```

  `ServicesSnapshot.rows`, `StartupSnapshot.rows` and `ConnectionsSnapshot.rows` are
  `Arc<[SysRow]>` with `query_generation: u64`, next to the records they index.
- **Interaction**: the UI debounces filter text 100 ms, then sends
  `Command::SetSysTableQuery(SysTableQuery)` with a new generation. Header clicks, expand and
  collapse, and filter strip changes send at once (no debounce). The UI shows a view only when its
  `query_generation` equals the latest sent; until then it keeps the previous rows (ADR-009 rule).
- **Delegate details** (`TlTreeDelegate` implementation per table):

  | Hook | Services | Startup Apps | Connections |
  |---|---|---|---|
  | Row id | `SysNodeId::{ServiceGroup, Unit, UnitProcess}` | `SysNodeId::Startup` | `SysNodeId::Conn` |
  | Selectable rows | `Unit`, `UnitProcess` (group rows are not selectable for actions) | all | all |
  | Expandable | group rows; unit rows with a non-empty cgroup | none | none |
  | `paint_cell` | UI-SYS-015, UI-SYS-016 | UI-SYS-042 | UI-SYS-062 |
  | `context_menu` | UI-SYS-017 (unit), process menu (child) | UI-SYS-044 | UI-SYS-063 |
  | `on_activate` (Enter, Alt+Enter, Ctrl+I, double-click) | open Service Properties (unit); Process Properties (child) | open Startup Properties | open Connection Properties |
  | `tooltip` (600 ms on the Name cell) | Description, unit file path, "Manager: System" or "Manager: User" | Command and Location | all owner PIDs when shared (SYS-030) |

- **Data binding**: reads `rows` and `query_generation` from the table's component.
- **Build**: `crates/tl-ui/src/pages/sys_table.rs` holds the shared query sender, debounce task and
  generation check. Delegates live in each page file.
- **Performance**: no sort, filter or allocation per frame in the UI. Delegates format through
  `CellTextCache` keyed by `(SysNodeId, ColumnId)`.
- **Accessibility**: as `TlTreeTable`.
- **Tests**: `sys_query_debounces_filter_100ms`; `sys_query_ignores_stale_generation` (install rows
  with an older generation, assert the old rows still show); `services_group_rows_not_selectable`
  (Ctrl+A selects only unit and child rows).
- **Phase**: P2.
- **Decided here**: `SysTableQuery`, `SysNodeId`, `Command::SetSysTableQuery`. These extend ADR-005
  and are listed as an open item for the architecture owner.

### UI-SYS-003 Status rendering in system tables

- **Implements**: SYS-003 (status mapping), SYS-020 (startup status), SYS-029 (connection states),
  R-050.
- **Anatomy** of a status cell:

  ```
  │ ● Running            │   dot 8×8, gap sp3 (6), label t_base
  ```

  | Part | Size | Colour | Notes |
  |---|---|---|---|
  | Dot | 8 × 8, fully round, vertically centred | per table below | filled or a 1.5 px ring |
  | Label | text, ellipsis on overflow | per table below | never colour alone: the label always shows |

  Services status (chosen colours; labels from SYS-003):

  | Status | Dot | Label colour |
  |---|---|---|
  | Running | filled `status.success` | `text_primary` |
  | Active | ring `status.success` | `text_primary` |
  | Starting, Stopping, Reloading | filled `status.info` | `text_primary` |
  | Stopped | ring `text_secondary` | `text_secondary` |
  | Failed | filled `status.danger` | `status.danger` |
  | Maintenance | filled `status.warning` | `text_primary` |

  Startup status (chosen): Enabled filled `status.success` / `text_primary`; Disabled ring
  `text_secondary` / `text_secondary`; Masked filled `status.warning` / `text_primary`; Not in this
  desktop ring `text_secondary` / `text_secondary`; Missing program filled `status.warning` /
  `text_primary`.

  Connections State column has no dot (chosen; TaskSlinger shows plain text). Text colours:
  LISTENING `status.info`; ESTABLISHED `text_primary`; SYN-SENT, SYN-RECEIVED, FIN-WAIT-1,
  FIN-WAIT-2, CLOSE-WAIT, CLOSING, LAST-ACK `text_primary`; TIME-WAIT and CLOSED `text_secondary`;
  UNKNOWN `status.warning`; UDP UNCONNECTED and CONNECTED `text_secondary`.

- **States**: on a selected row, the dot keeps its colour and the label uses
  `text_selected_primary` (or `text_selected_secondary` where it was secondary); `status.danger`
  labels stay `status.danger` on a selected row (chosen, so a failure stays visible). Stale data
  (source `Stale`): the whole cell uses `text_secondary` and the dot is drawn at 50% alpha.
- **Interaction**: none. Tooltip on the Status cell of a service: "<ActiveState> (<SubState>)",
  for example "active (running)" (chosen).
- **Data binding**: the record's status enum; never a string from the engine.
- **Build**: `crates/tl-ui/src/table/cells.rs` `paint_status_cell(dot: DotStyle, label:
  &SharedString, color)`. Dot is a `div` with `rounded_full` (8 px) and `bg` or `border_1p5`.
  Labels are static `SharedString`s from the message catalogue, so no allocation per frame.
- **Performance**: no allocation; labels are interned once per language.
- **Accessibility**: the label carries the meaning; colour is a second cue only.
- **Tests**: render test `status_cell_colors_per_state` (each state maps to the token above);
  `status_cell_selected_keeps_danger`.
- **Phase**: P2.
- **Decided here**: all colours in this entry; the Status tooltip.

### UI-SYS-004 First-load skeleton for system tables

- **Implements**: R-060, APP-059, SYS-016, SYS-027, SYS-028.
- **Anatomy**: the header is drawn normally. The body shows 12 skeleton rows (chosen), each `row_h`
  tall:

  ```
  │ ▭▭▭▭▭▭▭▭▭▭▭▭▭     ▭▭▭▭▭▭▭▭▭▭        ▭▭▭▭▭      ▭▭▭▭▭▭   │
  ```

  | Part | Size | Colour |
  |---|---|---|
  | Bar per visible column | height 10, `r_sm` corners, width 70% of the Name column and 40% of other columns, left inset `sp4` (right-aligned for numeric columns) | `TlSkeleton` default (foundation.md) |
  | Row background | `row_h` | `list_bg` / `row_alt` alternating |

- **States**: shown once per session per tab until the component's `header.validity` is not
  `WarmingUp` or rows arrive. Never shown again after that, even if the tab is left and re-entered
  (APP-059). If the source fails first, UI-SYS-005 error state replaces the skeleton.
  Reduced motion: `TlSkeleton` draws a static bar (no pulse).
  Services also pushes the toast "Loading service data..." (Info, `TlToast` id
  `ToastId::ServicesLoading`, autohide off) on first visit and removes it when the skeleton goes
  (SYS-016).
- **Interaction**: keys and clicks on the body do nothing while the skeleton shows; the header
  is live (sort clicks are queued in the query).
- **Data binding**: `component.header.validity` and `rows.is_empty()`; a per-page
  `first_data_seen: bool` that never resets in the session.
- **Build**: the page renders a plain `v_flex` of 12 skeleton rows in place of the `TlTreeTable`
  body. `TlSkeleton` is the kit `Skeleton`; it is allowed here because it is not a table cell of
  live data (ADR-009 forbids it in cells and charts) and it stops requesting frames once removed.
- **Performance**: the kit Skeleton requests frames while visible (2 s loop). It is visible for
  well under 1 s on this machine (assumed). Twelve rows × up to 7 bars is 84 quads.
- **Accessibility**: the status bar text reads "Loading…" (chosen) while the skeleton shows.
- **Tests**: `skeleton_shows_once_per_session` (first visit shows it; data arrives; leave and
  return; assert no skeleton); `services_loading_toast_removed_on_data`.
- **Phase**: P2.
- **Decided here**: 12 rows, bar widths, "Loading…" status text.

### UI-SYS-005 Empty and error states for system tables

- **Implements**: SYS-001 (privilege line), SYS-004, SYS-036, SYS-028, APP-028.
- **Anatomy**: `TlEmptyState` centred in the body area, header still shown:

  ```
            (icon 32)
       Heading, t_md semibold
   Text, t_base text_secondary, max width 420
          [ Button ]
  ```

  | Case | Icon | Heading | Text | Button |
  |---|---|---|---|---|
  | Services, neither manager answers | `server` | "systemd not found" (spec) | "tasklens lists services from systemd. Neither the system manager nor the user manager answered." (new) | "Retry" → `page::Refresh` |
  | Any table, text filter or state filters exclude every row | `funnel-x` | "No matches" (new) | "No services match the current filters." / "No startup entries match the filter." / "No connections match the filter." (new) | "Clear filters" (new): clears the filter text and, on Services, resets the strip to defaults |
  | Startup Apps, no entries at all | `app-window` | "No startup entries" (new) | "Nothing starts automatically when you log in." (new) | none |
  | Connections, no sockets | `network` | "No connections" (new) | none | none |
  | Source `Failed` before first data | `circle-alert` in `status.warning` | "Could not load services" / "Could not load startup entries" / "Could not load connections" (new) | `SourceHealth.last_error`, one line, ellipsis | "Retry" → `page::Refresh` |

- **States**: the empty state replaces only the body. When data arrives, rows replace it without
  animation. A `Failed` source after first data keeps the last rows and shows the status bar health
  text (APP-029); no empty state.
- **Interaction**: Tab reaches the button; Enter or Space presses it.
- **Data binding**: `rows.is_empty()`, the query filter, `header.validity`, `SourceHealth`.
- **Build**: `TlEmptyState` from foundation.md.
- **Performance**: static; cached with the page.
- **Accessibility**: the heading is the accessible name of the region.
- **Tests**: `empty_state_filter_clear_resets_strip`; `services_systemd_missing_state`.
- **Phase**: P2.
- **Decided here**: every "(new)" string in the table above.

### UI-SYS-006 Copy commands in system tables

- **Implements**: SYS-009, SYS-025, SYS-032, R-012, R-091 (applied to these tables), APP-044.
- **Anatomy**: no own visuals; menu items and keys.
- **Interaction**:

  | Command | Menu label | Keys | Action | Result |
  |---|---|---|---|---|
  | Copy the clicked cell | "Copy" | none | `table::CopyCell { column }` (column captured at right-click) | cell text as shown (formatted), toast "Copied: <value>" with the value truncated to 60 chars plus "…" |
  | Copy row | "Copy row" | Ctrl+C | `table::CopyRow` | the fixed block of the table (UI-SYS-020, UI-SYS-048, UI-SYS-066); multi-select blocks joined by one blank line |
  | Copy rows as table | "Copy rows as table" | Ctrl+Shift+C | `table::CopyRowsAsTable` | TSV of the visible columns in view order, header line first, one line per selected row in view order |

  - Toasts on success: Services "Service details copied."; Startup "Copied to clipboard"
    (chosen, the spec has no startup row toast); Connections "Connection copied" (spec). On clipboard
    failure: Services "Could not copy service details."; others "Failed to copy to clipboard".
  - With nothing selected (keyboard path only): "Select a service row first.", "Select a startup
    row first.", "Select a connection row first." (APP-044 variants).
  - Group rows are skipped when copying. Child process rows under a service copy the process
    "Copy details" block (processes.md).
  - TSV values use the cell text without validity glyphs: a glyph cell becomes empty (chosen).
- **Data binding**: the selected records at command time (not at menu open), except
  `CopyCell`, which captures the clicked row and column at menu open.
- **Build**: `crates/tl-ui/src/pages/sys_copy.rs` with one `fn block_<table>(record, &mut String)`
  per table. Writes with `cx.write_to_clipboard(ClipboardItem::new_string(..))`.
- **Performance**: runs once per command; one `String` sized by `capacity = rows × 256`.
- **Accessibility**: keyboard path exists for every copy command except "Copy" (cell), which needs
  the pointer to name a cell.
- **Tests**: `copy_row_services_block_format` (golden text); `copy_rows_as_table_tsv_header`;
  `copy_skips_group_rows`.
- **Phase**: P2.
- **Decided here**: "Copied: " truncation at 60 chars; startup copy toast; empty TSV for glyph cells;
  "Copy rows as table" in all three row menus.

### UI-SYS-007 Action tracking, progress and polkit prompt states

- **Implements**: SYS-005, SYS-006, SYS-007, SYS-013 (P3 edit), SYS-022, SYS-023, SYS-024,
  SYS-031, ADR-005 (`ActionTracker`), APP-044.
- **Anatomy**: no new widget. Progress is shown in three places:
  1. A `TlToast` per action, replaced in place by id (`ActionToast` keyed by `ActionId.0`, or by
     `(ActionId.0, target index)` for multi-target requests).
  2. The row's own cell while the action is pending (Services Status cell, UI-SYS-018).
  3. The menu: the same action on the same target is disabled while pending, with tooltip
     "A service action is already in progress." (Services) (spec text).
- **States** (one toast lifecycle):

  | ActionState | Toast kind | Icon | Autohide |
  |---|---|---|---|
  | request sent, no event yet | Info | `loader-circle` (spins at 1 turn per 1 s; static with reduced motion or `motion.animations = false`) | off |
  | `Started` | unchanged | unchanged | off |
  | `Succeeded` | Success | `check` | 5 s |
  | `Failed(..)` | Error | `circle-x` | 8 s |
  | `OutcomeUnknown(..)` | Warning | `triangle-alert` | 8 s |
  | `Rejected(Busy)` | Warning | `triangle-alert` | 5 s |
  | other `Rejected` | Error | `circle-x` | 8 s |

  Polkit prompt state: tasklens cannot see the prompt. While polkit-kde-authentication-agent shows
  its dialog the toast stays in the request-sent state. When the call returns:
  - polkit dismissed by the user → "Authentication cancelled" (Warning, 5 s) (spec);
  - not authorized → "<failure text>: not authorized" (Error), for example
    "Failed to start service: not authorized" (spec);
  - no agent running (`InteractiveAuthorizationRequired`) → "<failure text>: no authentication
    agent is running" (new, Error).
  How the engine tells "dismissed" from "not authorized" is open item O-3.

- **Interaction**: a toast with a result button ("View logs" on service failures, "Show" on
  not-visible jumps) keeps autohide off until closed (kit behaviour). Clicking the toast body does
  nothing.
- **Multi-select** (chosen): with 1 to 3 targets, one toast per target (spec). With 4 or more
  targets, one summary toast (to stay under the 4-toast stack limit): "Starting 6 services..."
  then "6 services started" or "4 of 6 services started. 2 failed." (new; same pattern for stop,
  restart, enable, disable). Its "Details" button (new) opens a `TlDialog` titled "tasklens" that
  lists each failed unit with its error text.
- **Data binding**: `ActionTracker` receives `ActionEvent`s; pages subscribe to
  `ActionTracker` events `PendingChanged { target: ActionTarget }` to repaint the affected row.
  Pending map `FxHashMap<ActionTarget, ActionId>` gives the row overlay.
- **Build**: `crates/tl-ui/src/actions.rs` (ADR-009 path). Toast text comes from a per-kind table
  `fn toast_text(kind: &ActionKind, phase: Phase, n: usize) -> SharedString`.
- **Performance**: events are rare; each repaints one toast and at most the visible rows.
- **Accessibility**: toasts are announced as text; result buttons are focusable from the toast
  stack (kit).
- **Tests**: `action_toast_replaced_in_place` (Started then Succeeded share the id);
  `multi_target_summary_toast_at_4`; `pending_target_disables_same_action`.
- **Phase**: P2 (P3 for helper-backed actions).
- **Decided here**: icons and autohide per state; the 4-target summary rule and its strings; the
  no-agent string.

---

# B. Services tab

### UI-SYS-010 Services page layout

- **Implements**: SYS-001, SYS-004, SYS-016, R-023 (tab 3).
- **Anatomy** (1280 × 800 window, 100% scale):

  ```
  ┌ tab strip (shell) ──────────────────────────────────────────── Filter: [        ] ┐
  ├───────────────────────────────────────────────────────────────────────────────────┤
  │ [Running][Failed][Stopped][Disabled] │ Unit types ▾ │ [System][User] │ ⊘ Remove filters │  28
  ├ Name ↑ ─────── Display name ───────── Status ── Startup type ─ PID ── User ── Path ┤  28
  │ ⌄ System services (214)                                                           │  row_h
  │   › ⚙ accounts-daemon.service  Accounts Service  ● Running  Enabled  812  root  /usr/l… │
  │   › ⚙ avahi-daemon.service     Avahi mDNS/DNS-SD ● Running  Enabled  790  avahi /usr/b… │
  │     ⏲ fstrim.timer             Discard unused…   ○ Active   Enabled       root  fstrim… │
  │ ⌄ User services (41)                                                              │
  │   › ⚙ pipewire.service         PipeWire Multim…  ● Running  Enabled  2210 oystein …  │
  ├───────────────────────────────────────────────────────────────────────────────────┤
  │ 255 Total | 198 Running | 57 Stopped                                               │  22
  └───────────────────────────────────────────────────────────────────────────────────┘
  ```

  Parts: UI-SYS-011 (strip), UI-SYS-013 (columns), UI-SYS-014 (group rows), UI-SYS-015 (unit rows),
  UI-SYS-016 (child rows), UI-SYS-022 (status bar).
- **States**: loading UI-SYS-004; empty and error UI-SYS-005; partial manager failure UI-SYS-014.
- **Interaction**: Tab order: strip toggles left to right, Unit types, System, User, Remove filters,
  then the table. Key context `ServiceTable` inside `Table`.
- **Data binding**: `Snapshot.services: Arc<ServicesSnapshot>`, `ComponentMask::SERVICES`. The page
  sets `Demand.services = true` while the tab is active.
- **Build**: `ServicesPage` entity in `crates/tl-ui/src/pages/services.rs`; fields
  `table: Entity<TlTreeTable<ServicesDelegate>>`, `strip: ServiceFilterState`,
  `unit_types_menu_open: bool`, `first_data_seen: bool`, `_subs: Vec<Subscription>`.
- **Performance**: ADR-008 resync every 60 s while visible; the UI work is one row rebuild per
  services version.
- **Accessibility**: see parts.
- **Tests**: `services_page_layout_golden` (render test at 1280 × 800 with the fake engine's 20-unit
  fixture, compares the element tree of the strip and header).
- **Phase**: P2.
- **Decided here**: none beyond the parts.

### UI-SYS-011 Services filter strip

- **Implements**: SYS-004, SYS-002, R-019 keys `services.filter_states`, `services.unit_types`.
- **Anatomy** (height 28, `toolbar` background, padding x `sp4`, gap `sp3`, 1 px `divider` bottom
  border; groups separated by a 1 × 16 `divider` rule with `sp3` on both sides):

  ```
  │ [Running][Failed][Stopped][Disabled] │ [Unit types ▾] │ [System][User] │ [⊘ Remove filters] │
  ```

  | Part | Widget | Size | Text | Notes |
  |---|---|---|---|---|
  | State toggles | `TlToggleChip` × 4 | height 22, padding x `sp4`, `r_sm` | "Running", "Failed", "Stopped", "Disabled" (`t_sm`) | multi-select; none on = all states |
  | Unit types | `TlButton` ghost, small, with `chevron-down` caret, opens a `TlMenu` of check items | height 22 | "Unit types" plus " (4)" when not all types are on (chosen) | UI-SYS-012 |
  | Manager toggles | `TlToggleChip` × 2 | as above | "System", "User" | both on by default |
  | Remove filters | `TlButton` ghost small, icon `funnel-x` 14 | height 22 | "Remove filters" | disabled when the strip is at its defaults |

  `TlToggleChip` look (foundation.md owns the values; repeated here for the reviewer): off =
  transparent fill, 1 px `divider` border, `text_secondary`; on = `accent` at 18% alpha fill,
  1 px `accent` border, `text_primary`; hover = `tab_hover` fill; focus = shared focus ring.

- **States**:
  - Default: no state toggle on, unit types Services, Sockets, Timers, Mounts, System and User on.
  - Turning both System and User off is allowed; the table then shows UI-SYS-005 "No matches"
    (chosen; no forced toggle).
  - Strip disabled (all controls `disabled`) while the skeleton shows (UI-SYS-004).
- **Interaction**:
  - Click or Space toggles a chip and sends the query at once. State toggles combine with OR, the
    result with the text filter using AND (SYS-004). Child rows inherit their unit's state.
  - "Remove filters" sets state toggles off, unit types to the default four, System and User on.
    It does not clear the text filter (chosen; Esc does that).
  - Actions: `services::ToggleStateFilter { state }`, `services::ToggleManagerFilter { manager }`,
    `services::ToggleUnitType { kind }`, `services::RemoveFilters`. No key bindings (table b has
    none).
- **Data binding**: writes `settings.toml` `services.filter_states` and `services.unit_types` on
  each change (debounced save per ADR-011). System and User toggles are session-only (chosen; table
  a has no key for them). Sends `SysTableQuery.services = Some(ServiceFilter { states, unit_types,
  system: bool, user: bool })`.
- **Build**: a `h_flex` rendered by `ServicesPage`, cached with the page. State classification
  (running, failed, stopped = enabled and not running, else disabled) is done in the engine.
- **Performance**: a click sends one command; no UI filtering.
- **Accessibility**: each chip is a toggle with its label as name and `aria`-like `toggled(bool)`
  set on the kit button; tooltip on "Remove filters": "Show all units" (new).
- **Tests**: `strip_state_or_text_and` (fake engine echoes the query; assert `ServiceFilter`);
  `remove_filters_restores_defaults`; `strip_persists_states_and_types`.
- **Phase**: P2.
- **Decided here**: chip look reference; " (4)" count suffix; both managers off allowed; Remove
  filters keeps the text filter; manager toggles not persisted; "Show all units" tooltip.

### UI-SYS-012 Unit types dropdown

- **Implements**: SYS-002.
- **Anatomy**: `TlMenu` anchored below the "Unit types" button, min width 180, check items:
  "Services", "Sockets", "Timers", "Mounts", "Automounts", "Paths", "Targets", "Swaps", "Slices",
  "Scopes", then a separator and "Reset to default" (new).
- **States**: each item checked per `services.unit_types`. Unchecking the last checked item is
  refused: the item stays checked and the toast "At least one unit type must stay selected" (new)
  shows.
- **Interaction**: click toggles an item and keeps the menu open (chosen, so several types can be
  changed at once); Esc or a click outside closes it. Up/Down move, Space or Enter toggles.
  Action `services::ToggleUnitType { kind }`.
- **Data binding**: `services.unit_types` (array of `service socket timer mount automount path
  target swap slice scope`).
- **Build**: `TlMenu` with `menu_with_check`; the "keep open" behaviour uses the item's `on_click`
  plus `PopupMenu::rebuild`.
- **Performance**: built on open only.
- **Accessibility**: the button shows the pressed state while the menu is open (widgets rule 9).
- **Tests**: `unit_types_last_item_refused`; `unit_types_menu_stays_open_on_toggle`.
- **Phase**: P2.
- **Decided here**: "Reset to default" item; last-item refusal toast; menu stays open.

### UI-SYS-013 Services columns

- **Implements**: SYS-003, SYS-017, R-011, R-075, APP-018, table (d).
- **Anatomy**: column definitions (`ColumnId` values are the spec ids):

  | Id | Header label | Default width | Min width | Align | Format | Visible by default |
  |---|---|---|---|---|---|---|
  | `name` | "Name" | 250 | 120 | left | unit id, with icon | yes |
  | `display_name` | "Display name" | 300 | 60 | left | `Description` | yes |
  | `status` | "Status" | 100 | 80 | left | UI-SYS-003 | yes |
  | `startup_type` | "Startup type" | 120 | 80 | left | SYS-003 words | yes |
  | `pid` | "PID" | 80 | template `"999999"` | right | integer, blank when 0 | yes |
  | `user` | "User" | 200 | 60 | left | user name | yes |
  | `path` | "Path" | 260 | 60 | left | SYS-003 per type | yes |
  | `manager` | "Manager" | 80 | 60 | left | "System" or "User" | no |
  | `unit_type` | "Unit type" | 90 | 60 | left | "Service", "Socket", "Timer", "Mount", "Automount", "Path", "Target", "Swap", "Slice", "Scope" | no |
  | `sub_state` | "Sub state" | 100 | 60 | left | raw `SubState` | no |
  | `load_state` | "Load state" | 100 | 60 | left | raw `LoadState` | no |
  | `cpu` | "CPU" | 63 | template `"100.0%"` | right | `%.1f%%`, "0%" at zero | no |
  | `memory` | "Memory" | 92 | template `"999 MiB"` widened to `"999.99 MiB"` | right | IEC two decimals | no |
  | `disk` | "Disk" | 82 | template `"xxx MiB/s"` | right | MiB/s one decimal | no |
  | `tasks` | "Tasks" | 72 | template `"999999"` | right | count, thin-space grouped | no |
  | `active_since` | "Active since" | 150 | template `"9999-99-99 99:99:99"` | left | `YYYY-MM-DD HH:MM:SS` local | no |
  | `unit_file` | "Unit file" | 300 | 60 | left | `FragmentPath` | no |
  | `triggered_by` | "Triggered by" | 200 | 60 | left | comma-separated unit ids | no |
  | `description` | "Description" | 300 | 60 | left | `Description` (same as Display name; kept because SYS-003 lists it) | no |

  Widths of the seven default columns are spec (SYS-017); widths of optional columns are chosen.
- **States**:
  - Sort: default `name` ascending (R-011); the header shows "Name ↑". Groups stay grouped while
    sorting; rows sort within each group; child process rows sort by PID ascending always (chosen).
  - Resource columns show `…` while warming up and `-` for units without a cgroup.
  - Heat colouring applies to `cpu`, `memory`, `disk` with the process references (ADR-009) when
    `appearance.heat_coloring` is on (chosen). `tasks` is a count and is not tinted.
  - `tables.dim_zero_values` and `tables.column_separators` apply (R-075).
- **Interaction**: header click sorts (direction rules as `TlTreeTable`); right-click on a header
  opens the shared header menu ("Select Columns…", "Hide This Column", "Resize columns to fit
  content", "Resize Behavior", "Reset to default"); View > Columns acts on this table while the
  Services tab is active (APP-018). The column picker lists these columns without headings.
- **Data binding**: `state.toml` `[sort.services]`, `[columns.services]` (order, visible, widths).
  Showing any of `cpu`, `memory`, `disk`, `tasks` sets `Demand.service_cgroup_stats = true`
  (new field, open item O-1); hiding all four clears it.
- **Build**: `ServicesDelegate::columns() -> &'static [ColumnDef]` in `pages/services.rs`.
- **Performance**: resource columns cost 1 Hz cgroup reads in the engine only while shown.
- **Accessibility**: header cells have the label as accessible name; the sort arrow is also in the
  tooltip "Sorted ascending" / "Sorted descending" (shared, processes.md).
- **Tests**: `services_default_columns_and_widths`; `services_sort_keeps_groups`;
  `services_cgroup_demand_follows_columns`.
- **Phase**: P2.
- **Decided here**: optional column widths and minimums; child rows always by PID; heat colouring
  on unit resource columns; `Demand.service_cgroup_stats`.

### UI-SYS-014 Services group rows (System and User manager)

- **Implements**: SYS-001, R-019 key `services.group_expanded`.
- **Anatomy**: same renderer as the process group row (processes.md: chevron 16, label `t_base`
  semibold, `text_primary`, full-width row, no cells). Label text:
  - "System services (N)" and "User services (N)". N counts unit rows after all filters, thin-space
    grouped.
  - Manager not reachable: "System services" followed by `sp3` and the secondary text "System
    manager not available" (or "User manager not available"), with a `circle-alert` icon 14 in
    `status.warning` before it. No count, no chevron.
  - Manager reconnecting after a bus drop: label plus " (reconnecting…)" in `text_secondary`
    (new). Rows from the last good data stay, drawn in `text_secondary` (stale).
- **States**: hover `row_hover`; group rows take keyboard focus (for Space, Left, Right) but are not
  selected for actions (UI-SYS-002). Empty group (filters remove all units): the group row is
  hidden (chosen).
- **Interaction**: click on the chevron or double-click on the row toggles; Space toggles; Right
  expands, Left collapses. Ctrl+L (Collapse all) collapses unit rows and keeps the two group rows
  expanded (chosen). Action `table::ToggleExpand`.
- **Data binding**: `state.toml` `[services] group_expanded = { system = true, user = true }`,
  saved on each toggle. Manager health from `ServicesSnapshot.managers[System|User].state`
  (`Connected | Unavailable | Reconnecting`, new field).
- **Build**: delegate row kind `GroupRow`; the paint is the shared group row.
- **Performance**: two rows.
- **Accessibility**: accessible name "System services, 214 units, expanded" (chosen).
- **Tests**: `group_row_unavailable_label`; `group_expand_state_persisted`;
  `collapse_all_keeps_groups_open`.
- **Phase**: P2.
- **Decided here**: reconnecting suffix; empty groups hidden; Ctrl+L keeps groups open;
  `managers` health field.

### UI-SYS-015 Service unit row

- **Implements**: SYS-001, SYS-003, SYS-005 (live), UI-SYS-003.
- **Anatomy** (depth 1 under the group row; indent per depth from processes.md):

  ```
  │   › ⚙ sshd.service      OpenSSH Daemon     ● Running   Enabled    812   root   /usr/bin/sshd │
  │   ^ ^ name                                  status      startup    pid   user   path         │
  │   | icon 16                                                                                   │
  │   chevron 16 (only with child processes)                                                      │
  ```

  | Cell | Content | Style |
  |---|---|---|
  | name | chevron (when the cgroup has processes), unit icon 16, unit id | `text_primary` `t_base`, ellipsis at end |
  | display_name | `Description` | `text_primary`; empty description shows nothing |
  | status | UI-SYS-003 dot and label; pending overlay from UI-SYS-018 | |
  | startup_type | word | Enabled, Enabled (runtime), Static, Indirect, Generated, Transient, Alias, Linked: `text_primary`; Disabled and "-": `text_secondary`; Masked: `status.warning` |
  | pid | MainPID | right, `tnum`; blank when 0 |
  | user | owner | ellipsis |
  | path | per SYS-003 | ellipsis at end; full value in tooltip |

  Unit icons (Lucide, chosen): service `cog`, socket `plug`, timer `clock`, mount and automount
  `hard-drive`, path `folder-open`, target `circle-dot`, swap `memory-stick`, slice and scope `box`.
  Icon colour `text_secondary`; on a selected row `text_selected_secondary`.
- **States**: default `list_bg`/`row_alt`; hover `row_hover`; selected `row_selected` with
  `text_selected_primary`; focused row gets the shared row focus outline; stale (manager
  reconnecting) all text `text_secondary`. Failed units keep the danger status label.
- **Interaction**: click selects; Ctrl+click, Shift+click per table rules; double-click or Enter
  opens Service Properties (`table::OpenProperties`); right-click opens UI-SYS-017. Hover tooltip on
  the name cell after 600 ms: three lines "<Description>", "<FragmentPath>", "Manager: System"
  (chosen).
- **Data binding**: `ServicesSnapshot.units[record]`: `unit: UnitRef`, `description`, `status:
  ServiceStatus`, `startup: StartupType`, `main_pid`, `user`, `path`, `kind: UnitKind`,
  `has_cgroup_procs: bool`, resource fields with `Validity`.
- **Build**: `ServicesDelegate::paint_cell`; strings come from the snapshot as `Arc<str>` wrapped in
  `SharedString` without copying; numbers through `CellTextCache`.
- **Performance**: no allocation for unchanged cells.
- **Accessibility**: row accessible name "<id>, <Status>, <Startup type>" (chosen).
- **Tests**: `unit_row_icon_per_kind`; `unit_row_pid_blank_when_zero`;
  `unit_row_masked_warning_color`.
- **Phase**: P2.
- **Decided here**: icons, startup-type colours, tooltip lines, accessible name.

### UI-SYS-016 Service child process rows

- **Implements**: SYS-001 (cgroup subtree), SYS-006 (Ctrl+E on child), PROC-057 (menu reuse).
- **Anatomy**: depth 2 under the unit row. Cells reuse `ProcessRowCells` mapped onto the Services
  columns:

  | Services column | Child row content |
  |---|---|
  | name | process icon 16 and display name (processes.md naming) |
  | display_name | process Description |
  | status | empty |
  | startup_type | empty |
  | pid | PID |
  | user | process user |
  | path | exe path |
  | cpu, memory, disk, tasks | process CPU, Memory (RssAnon, D-004), Disk, Threads |
  | others | empty |

  Processes in nested cgroups are listed flat under the unit (chosen), sorted by PID ascending.
- **States**: as process rows (processes.md), including the `…` and lock glyphs and the new or
  exited highlight. A child that exits disappears at the next services update after its linger
  time.
- **Interaction**: the full process context menu (PROC-057) with the same keys; Ctrl+E on a child
  row runs End Task on the process (table b). Enter opens Process Properties.
- **Data binding**: `ServicesSnapshot.units[i].cgroup_pids: Arc<[ProcessKey]>` (new) joined with
  `Snapshot.processes`. While at least one unit row is expanded the page sets
  `Demand.process_table = true` and adds the visible child keys to `detail_keys` (chosen), so the
  sweep runs; it clears it when none is expanded.
- **Build**: delegate row kind `ProcessChild`; paint delegates to `ProcessRowCells::paint`.
- **Performance**: the process sweep runs only while a unit is expanded on a visible Services tab.
- **Accessibility**: as process rows.
- **Tests**: `child_rows_use_process_menu`; `ctrl_e_on_child_ends_process`;
  `expanding_unit_enables_process_demand`.
- **Phase**: P2.
- **Decided here**: flat list of nested cgroup processes; column mapping; process demand rule;
  `cgroup_pids` field.

### UI-SYS-017 Services context menu

- **Implements**: SYS-008, SYS-006, SYS-007, SYS-009, SYS-010, R-012, R-091 (applied).
- **Anatomy**: `TlMenu`, min width 220, items in this order (keys shown right-aligned by the kit):

  | # | Label | Key hint | Action | Shown when | Enabled when |
  |---|---|---|---|---|---|
  | 1 | "Start" | Ctrl+S | `services::Start` | always | any target not active and not pending |
  | 2 | "Stop" | Ctrl+E | `services::Stop` | always | any target active and not pending |
  | 3 | "Restart" | Ctrl+R | `services::Restart` | always | any target active and not pending |
  | 4 | "Reload" | | `services::Reload` | any target has `CanReload` | that target active |
  | 5 | "Reset failed state" | | `services::ResetFailed` | any target Failed | always |
  | 6 | "Startup type" › | | submenu, UI-SYS-019 | always | see UI-SYS-019 |
  | – | separator | | | | |
  | 7 | "View logs" | | `services::ViewLogs` | always | one target; disabled on OpenRC |
  | 8 | "Properties" | Enter | `table::OpenProperties` | always | always |
  | 9 | "File properties" | | `services::FileProperties` | always | binary or unit file known |
  | 10 | "Open file location" | | `services::OpenFileLocation` | always | binary known |
  | 11 | "Open unit file" | | `services::OpenUnitFile` | always | `FragmentPath` not empty |
  | – | separator | | | | |
  | 12 | "Copy" | | `table::CopyCell` | always | always |
  | 13 | "Copy row" | Ctrl+C | `table::CopyRow` | always | always |
  | 14 | "Copy rows as table" | Ctrl+Shift+C | `table::CopyRowsAsTable` | always | always |
  | 15 | "Jump to process" | | `services::JumpToProcess` | always | one target with MainPID ≠ 0 |

  Items have no icons (chosen; TaskSlinger menus have none). The kit hides a separator next to a
  hidden item.
- **States**: disabled items use `text_secondary` at 50% alpha and have a tooltip naming the reason:
  "A service action is already in progress." (pending), "Service not running" (Jump with MainPID 0,
  spec toast text reused), "Service image unavailable" (no ExecStart, spec text reused).
- **Interaction**:
  - Right-click on an unselected row selects it first; the menu captures the target
    `SmallVec<[UnitRef; 1]>` at open and stays bound to it (ADR-009).
  - Shift+F10 or the Menu key opens it for the selection at the focused row's left edge.
  - Child process rows show the process menu instead. A mixed selection (units and child rows)
    shows the unit menu and acts on the unit rows only (chosen).
  - File properties: `FileManager1.ShowItemProperties` on the binary (else the unit file). Toasts
    "Opened service file properties." / "Could not open service file properties." (spec).
  - Open file location: `FileManager1.ShowItems` on the binary. Toasts "Opening service file
    location." / "Could not open service file location."; no ExecStart: "Service image
    unavailable" (spec).
  - Open unit file: `xdg-open <FragmentPath>`. Toast on failure "Could not open the unit file"
    (new). No success toast (chosen).
- **Data binding**: target records at menu open; `ActionTracker` pending map.
- **Build**: `ServicesDelegate::context_menu` builds the `TlMenu` with data-carrying actions so key
  hints show (widgets research section 4).
- **Performance**: built on open.
- **Accessibility**: full keyboard path through Shift+F10; every item reachable by Up/Down.
- **Tests**: `services_menu_order_and_labels` (golden label list); `menu_reload_hidden_without_canreload`;
  `menu_target_captured_at_open` (rows re-sort while the menu is open; Start acts on the captured unit).
- **Phase**: P2.
- **Decided here**: enablement rules for multi-select; no icons; disabled tooltips; mixed
  selection rule; "Could not open the unit file"; no success toast for Open unit file.

### UI-SYS-018 Start, Stop, Restart, Reload and Reset failed flows

- **Implements**: SYS-005, SYS-006, ADR-005, UI-SYS-007.
- **Anatomy**: toast plus row overlay. The overlay replaces the Status cell label while the action
  is pending:

  ```
  │ ● Starting…  │   dot filled status.info, label text_primary
  ```

- **States and texts** (spec unless marked new):

  | Action | Pending toast and overlay label | Success toast | Failure toast |
  |---|---|---|---|
  | Start | "Starting service..." / "Starting…" | "Service started" | "Failed to start service: <result>" + "View logs" |
  | Stop | "Stopping service..." / "Stopping…" | "Service stopped" | "Failed to stop service: <result>" + "View logs" |
  | Restart | "Restarting service..." / "Restarting…" | "Service restarted" | "Failed to restart service: <result>" + "View logs" |
  | Reload | "Reloading service..." (new) / "Reloading…" | "Service reloaded" (new) | "Failed to reload service: <result>" (new) + "View logs" |
  | Reset failed state | "Clearing failed state..." (new) / none | "Failed state cleared" (new) | "Failed to clear failed state: <error>" (new) |

  - `<result>` is the systemd job result word (`failed`, `timeout`, `canceled`, `dependency`,
    `skipped`, `invalid`) or the D-Bus error text, or "not authorized".
  - `OutcomeUnknown`: "Service action finished." (Warning) and the engine resyncs.
  - `Rejected(Busy)`: "A service action is already in progress." (Warning).
  - The overlay starts when the request is sent and ends at the final event; the Status cell then
    shows the snapshot state.
  - No confirmation dialog for any of these (spec).
- **Interaction**: keys Ctrl+S, Ctrl+E, Ctrl+R in `ServiceTable` on unit rows (table b). "View logs"
  opens Service Properties on the Logs tab for that unit (UI-SYS-030).
- **Data binding**: `ActionRequest { kind: UnitStart | UnitStop | UnitRestart | UnitReload |
  UnitResetFailed, targets: [Unit(UnitRef)] }`. `UnitReload` and `UnitResetFailed` are new
  `ActionKind` variants (open item O-2).
- **Build**: handlers in `pages/services.rs` call `ActionTracker::submit(req, ToastSpec)`.
- **Performance**: none per frame.
- **Accessibility**: toast text states the outcome; the overlay label is text.
- **Tests**: `start_flow_toasts_and_overlay` (fake engine emits Started then Succeeded);
  `failed_start_shows_view_logs`; `busy_rejection_toast`.
- **Phase**: P2.
- **Decided here**: overlay labels; Reload and Reset failed strings.

### UI-SYS-019 Startup type submenu and flow

- **Implements**: SYS-007, SYS-011 (combo shares the flow).
- **Anatomy**: submenu "Startup type" › with three check items: "Enabled", "Disabled", "Masked".
  The current value is checked. "Enabled (runtime)" counts as Enabled.
- **States**:
  - Static, Indirect, Generated, Transient: the "Startup type" item is disabled with tooltip "This
    unit has no install section" (spec).
  - No unit file ("-"): disabled with tooltip "This unit has no unit file" (new).
  - Multi-select with different values: no item checked (chosen).
  - Pending: toast "Changing startup type..." (new), then "Startup type changed" or "Failed to
    change startup type" (spec), with ": <error>" appended to the failure (chosen). Polkit may
    prompt twice for system units (spec); the toast stays pending through both.
- **Interaction**: choosing the current value does nothing. Choosing another sends one request.
  Action `services::SetStartupType { startup: UnitStartup::{Enabled, Disabled, Masked} }`.
- **Data binding**: `ActionKind::UnitEnable`, `UnitDisable` (ADR-005) and new `UnitMask`,
  `UnitUnmask` (open item O-2); leaving Masked sends Unmask first, then the chosen kind, as one
  request (engine sequence).
- **Build**: submenu built in `ServicesDelegate::context_menu`.
- **Performance**: none.
- **Accessibility**: check items announce their checked state.
- **Tests**: `startup_submenu_disabled_for_static`; `startup_change_from_masked_unmasks_first`.
- **Phase**: P2.
- **Decided here**: "Changing startup type...", "This unit has no unit file", error suffix, no check
  on mixed selection.

### UI-SYS-020 Services copy formats

- **Implements**: SYS-009, R-012, UI-SYS-006.
- **Anatomy**: "Copy row" block per unit, exactly:

  ```
  Name: <id>
  Display name: <Description>
  Status: <Status>
  Start type: <Startup type>
  PID: <MainPID>
  User: <User>
  Binary: <ExecStart[0].path>
  ```

  Empty values print nothing after the colon and space ("PID: " when MainPID is 0, chosen).
- **States**: success "Service details copied."; failure "Could not copy service details." (spec).
- **Interaction**: Ctrl+C, menu "Copy row". Ctrl+Shift+C TSV per UI-SYS-006.
- **Data binding**: unit records.
- **Build**: `sys_copy::block_service`.
- **Performance**: once per command.
- **Accessibility**: n/a.
- **Tests**: `service_copy_block_golden`.
- **Phase**: P2.
- **Decided here**: empty value rendering.

### UI-SYS-021 Jump between Services and Processes

- **Implements**: SYS-010, PROC-071, SYS-032 (Go to service from Connections).
- **Anatomy**: no own visuals.
- **Interaction**:
  - "Jump to process" (`services::JumpToProcess`): calls the PROC-071 entry point with the MainPID's
    `ProcessKey`. Toasts: "Switched to service process." on success; "Service process not visible
    in the current process view." with a "Show" button when filtered or hidden; "Service not
    running" when MainPID is 0 (spec).
  - Arriving at Services ("Go to service" from Processes or Connections): the page switches the
    tab, removes strip filters only if they hide the unit (chosen: it never removes the text
    filter), expands the manager group, selects the unit, scrolls it to the vertical centre, and
    focuses the table. Toasts "Switched to Services tab" (when coming from another tab) and
    "Switched to service." (spec). If the unit is not listed: "Service not found in current view"
    (spec). If the text filter hides it: the same toast with a "Show" button that clears the text
    filter, then selects (chosen, mirrors PROC-071).
- **Data binding**: `UnitRef` from the process's cgroup (engine). Selection waits for the next view
  whose `query_generation` matches before scrolling (so the row exists).
- **Build**: `ServicesPage::select_unit(UnitRef, origin: JumpOrigin, cx)`; registered with the main
  window router as the target of `GoToService`.
- **Performance**: one query and one scroll.
- **Accessibility**: focus moves to the table on arrival.
- **Tests**: `go_to_service_expands_and_selects`; `go_to_service_filtered_shows_button`;
  `jump_to_process_main_pid_zero_toast`.
- **Phase**: P2.
- **Decided here**: strip filter removal rule; "Show" button for text-filter case.

### UI-SYS-022 Services status bar text and refresh

- **Implements**: SYS-016, APP-029.
- **Anatomy**: left text in the shell status bar, `t_sm`, `text_secondary`:
  "255 Total | 198 Running | 57 Stopped", plus " | 3 selected" with a selection. Counts are unit
  rows after all filters (child rows not counted), thin-space grouped. Running counts ActiveState
  `active` and `reloading`; Stopped counts `inactive` and `failed`; Starting and Stopping count in
  Total only.
- **States**: while the skeleton shows: "Loading…". While paused: shell appends "[PAUSED - Ctrl]".
- **Interaction**: F5 (`page::Refresh`) sends `RefreshNow(SystemdSystem)` and
  `RefreshNow(SystemdUser)`, then shows "Services refreshed." when the next services version
  arrives or after 2 s, whichever is first (chosen).
- **Data binding**: counts precomputed by the engine in `ServicesSnapshot.counts` (new field), so
  the UI does not iterate rows.
- **Build**: `ServicesPage::status_text(&self) -> SharedString`, rebuilt only when counts or the
  selection size change.
- **Performance**: one short string per change.
- **Accessibility**: plain text.
- **Tests**: `services_status_text_counts`; `f5_refresh_toast_after_update`.
- **Phase**: P2.
- **Decided here**: count rules for transitional states; F5 toast timing; `counts` field.

### UI-SYS-023 Services keyboard summary

- **Implements**: table (b) rows for Table and ServiceTable, D-008, R-043.
- **Interaction** (all in the `ServiceTable` and `Table` contexts):

  | Keys | Action | On unit row | On child row | On group row |
  |---|---|---|---|---|
  | Up, Down, PgUp, PgDn, Home, End | move focus and selection | yes | yes | focus only |
  | Shift+Up/Down, Shift+click, Ctrl+click | extend or toggle | yes | yes | skipped |
  | Ctrl+A | select all visible unit and child rows | | | |
  | Right / Left | expand or first child / collapse or parent | yes | parent | yes |
  | Space | toggle expand | yes | no | yes |
  | Enter, Alt+Enter, Ctrl+I | Properties | Service Properties | Process Properties | toggle (chosen) |
  | Ctrl+S / Ctrl+E / Ctrl+R | Start / Stop / Restart | yes | Ctrl+E ends the process; others nothing | nothing |
  | Ctrl+C / Ctrl+Shift+C | Copy row / as table | yes | process block | skipped |
  | Ctrl+L | collapse all (groups stay open) | | | |
  | Shift+F10, Menu | context menu | unit menu | process menu | none |
  | F5 | refresh | | | |
  | printable key | start filtering | | | |
  | Esc | global rule | | | |

- **Tests**: `services_keymap_contexts` (each binding resolves to the listed action in
  `ServiceTable`).
- **Phase**: P2.
- **Decided here**: Enter on a group row toggles it.

### UI-SYS-024 OpenRC variant of the Services tab

- **Implements**: SYS-018.
- **Anatomy**: same page. Differences:
  - One group row "OpenRC services (N)" (new) replaces the two manager groups; the System and User
    chips and the Unit types dropdown are hidden.
  - Columns available: Name, Status, Startup type, PID, User, Path. Status uses Running, Stopped,
    Failed (from `rc-status`). Startup type shows "Enabled" (in runlevel `default`) or "Disabled".
  - Menu: Start, Stop, Restart, Startup type › (Enabled, Disabled), separator, Properties, File
    properties, Open file location, separator, Copy, Copy row, Copy rows as table. No Reload, Reset
    failed state, View logs, Open unit file, Jump to process (no cgroups).
  - Properties shows only the General tab.
- **States**: actions prompt through pkexec; cancellation gives "Authentication cancelled".
- **Data binding**: `ServicesSnapshot.backend = OpenRc`.
- **Build**: the delegate checks `backend` when building columns and menus.
- **Tests**: `openrc_hides_systemd_only_items`.
- **Phase**: P4.
- **Decided here**: OpenRC group label and reduced menu.

---
