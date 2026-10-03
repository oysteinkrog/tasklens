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
