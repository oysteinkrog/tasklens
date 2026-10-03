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
- `docs/ui/shell.md`: title bar, tab strip, filter box and status chip.

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
| `TlTreeTable<D: TreeTableDelegate>` (UI-PRC-001) | processes.md | the three system tables |
| `TreeTableDelegate` hooks `row_count`, `row_id`, `row_index`, `row_kind`, `row_depth`, `row_expandable`, `row_flags`, `columns`, `column_spec`, `cell`, `name_cell`, `sort`, `selectable`, `context_menu`, `on_toggle_expand`, `on_sort`, `on_columns_changed`, `on_visible_range`, `on_activate`, `row_tooltip`, `empty_text`, `header_lines` (`HeaderLines::One` gives a 28 px header), `columns_fixed` | processes.md | delegate details below |
| `RowKind::{GroupHeader, Unit, Process, Plain}` | processes.md | row kinds |
| Column picker window (UI-PRC-007) and header menu (UI-PRC-006) | processes.md | View > Columns for the three tables (APP-018) |
| shared table actions `OpenProperties`, `CopyCell { col }`, `CopyDetails` (Ctrl+C; menu label "Copy row" in these tables, R-012), `CopyRowsAsTable`, `SelectAll`, `CollapseAll`, `ToggleExpand`, `OpenContextMenu`, `MoveFocus*` (namespace `tasklens`) | processes.md | same keys in every table |
| process row delegate cells (UI-PRC-002, UI-PRC-004) and the process row menu (UI-PRC-013) | processes.md | service child rows |
| `ProcessesPage::select_by_key(key, source)` (UI-PRC-022) | processes.md | jumps to a process |
| `TlButton` kinds `Primary`, `Secondary`, `Danger`, `Ghost`, `Icon`, `Toggle`, `Segment` (UI-FND-010) | foundation.md | buttons; `Segment` is the filter strip toggle |
| `TlSelect`, `TlCheckbox`, `TlTextInput`, `TlStepper` (UI-FND-015 to UI-FND-017) | foundation.md | form controls |
| `TlMenu` (UI-FND-022) | foundation.md | context, dropdown and sub menus |
| `ToastQueue::push(kind, text, key, action)` with kinds Info, Success, Warning, Error, Progress and `ToastKey` (UI-FND-020) | foundation.md | toasts (APP-044) |
| `TlDialog` and `TlSecondaryWindow` (UI-FND-021) | foundation.md | confirmations and Properties windows |
| `TlTabs` (UI-FND-018) | foundation.md | Properties window tabs |
| `TlKeyValueGrid` inline layout, its section title band and its link values (UI-FND-025) | foundation.md | Properties fields and Information sections |
| `TlSkeletonRows`, `TlSkeletonBlock` (UI-FND-024) | foundation.md | first-load placeholders (R-060) |
| `TlEmptyState` (UI-FND-029) | foundation.md | empty and error states |
| `TlBadge` `Neutral` (UI-FND-027) | foundation.md | relation tags in Dependencies |
| `TlSpinner` (UI-FND-023) | foundation.md | pending buttons |
| `TlLink` | this file | shorthand for a `TlKeyValueGrid` link value (`status.info`, underline on hover, pointer cursor, UI-FND-025) used outside a grid too |
| `TlBanner` | this file, UI-SYS-038 | an info strip inside a pane: height 32, padding x `sp5`, `status.info` at alpha 0.12 fill, 1 px `status.info` bottom border, icon `info` 14 in `status.info`, text `t_sm` `text_primary`, trailing buttons `TlButton::Secondary` sm |
| Status chip, page method `fn status_text(&self, out: &mut String)` and event `StatusChanged` (UI-SHL-050) | shell.md | status text per tab (APP-029) |
| `FilterBox` and the page event `FilterChanged { tab, text }` (UI-SHL-024) | shell.md | the tab filter (APP-028) |
| Information tab copy icon, action `CopyInformationAll` (UI-SHL-023) | shell.md | copy all |

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
  │                                                                          │
  │ status chip, 26 px, floating bottom-left (shell.md UI-SHL-050)            │
  └──────────────────────────────────────────────────────────────────────────┘
  ```

  | Part | Size | Colours | Notes |
  |---|---|---|---|
  | Page root | fills the content area | `window_bg` | `v_flex`, `size_full`, no padding |
  | Filter strip | full width × 28 | `toolbar` background, 1 px `divider` bottom border | UI-SYS-011 |
  | Header | full width × 28 (one line; chosen) | `header` background, `divider` bottom border, `text_primary` `t_base` medium | the three tables have no totals line, so the delegate returns `HeaderLines::One` (header_h 28, UI-PRC-001) |
  | Body | rest | `list_bg`, alternate rows `row_alt` | `TlTreeTable` body, not cached (owns scrolling) |
  | Status text | in the shell's floating status chip (UI-SHL-050) | chip styling is the shell's | the page implements `status_text(&self, out: &mut String)` and emits `StatusChanged` only when counts or the selection size change; UI-SYS-022, UI-SYS-049, UI-SYS-067. The table body adds bottom padding equal to the chip height (UI-SHL-050) |

- **States**:
  - Loading (first data not yet in this session): UI-SYS-004.
  - Empty: UI-SYS-005.
  - Ctrl held while the main window is active: the table renders from the paused snapshot and the
    status chip appends "[PAUSED - Ctrl]" in `accent` (R-042). The shell owns that segment; the
    table is paused through `TlTreeTable.paused` (UI-PRC-017) and the delegate reads the paused
    snapshot the hub keeps (ADR-009).
- **Interaction**: the page has key context `ServiceTable` (Services), `StartupTable` (Startup Apps,
  chosen), `ConnectionTable` (Connections, chosen), each nested inside `Table`. All three also get
  the `Table` bindings of table (b). F5 (Global "Refresh current tab now") is handled by the active
  page (UI-SYS-022, UI-SYS-049, UI-SYS-067). A printable key with the table focused starts filtering
  (shell `FilterBox`, which then sends `FilterChanged { tab, text }` to the page after its 100 ms
  debounce).
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
- **Interaction**: on `FilterChanged` (already debounced 100 ms by the shell `FilterBox`) the page
  sends `Command::SetSysTableQuery(SysTableQuery)` with a new generation. Header clicks, expand and
  collapse, and filter strip changes send at once (no debounce). The UI shows a view only when its
  `query_generation` equals the latest sent; until then it keeps the previous rows (ADR-009 rule).
- **Delegate details** (`TreeTableDelegate` implementation per table):

  | Hook | Services | Startup Apps | Connections |
  |---|---|---|---|
  | Row id | `SysNodeId::{ServiceGroup, Unit, UnitProcess}` | `SysNodeId::Startup` | `SysNodeId::Conn` |
  | Selectable rows | `Unit`, `UnitProcess` (group rows are not selectable for actions) | all | all |
  | Expandable | group rows; unit rows with a non-empty cgroup | none | none |
  | `cell`, `name_cell` | UI-SYS-015, UI-SYS-016 | UI-SYS-042 | UI-SYS-062 |
  | `context_menu` | UI-SYS-017 (unit), process menu (child) | UI-SYS-044 | UI-SYS-063 |
  | `on_activate` (Enter, Alt+Enter, Ctrl+I, double-click) | open Service Properties (unit); Process Properties (child) | open Startup Properties | open Connection Properties |
  | `row_tooltip` (600 ms on the Name cell) | Description, unit file path, "Manager: System" or "Manager: User" | Command and Location | all owner PIDs when shared (SYS-030) |

- **Data binding**: reads `rows` and `query_generation` from the table's component.
- **Build**: `crates/tl-ui/src/pages/sys_table.rs` holds the shared query sender and the
  generation check. Delegates return `HeaderLines::One` and `RowKind::GroupHeader`, `Unit`,
  `Process` (Services) or `Plain` (Startup Apps, Connections). Delegates live in each page file.
- **Performance**: no sort, filter or allocation per frame in the UI. Delegates format through
  `CellTextCache` keyed by `(SysNodeId, ColumnId)`.
- **Accessibility**: as `TlTreeTable`.
- **Tests**: `sys_query_sent_on_filter_changed`; `sys_query_ignores_stale_generation` (install rows
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
- **Anatomy**: the header is drawn normally. The body shows `TlSkeletonRows` (UI-FND-024):
  `min(rows that fit, 12)` rows, each `row_h` tall, one bar per visible column:

  ```
  │ ▭▭▭▭▭▭▭▭▭▭▭▭▭     ▭▭▭▭▭▭▭▭▭▭        ▭▭▭▭▭      ▭▭▭▭▭▭   │
  ```

  | Part | Size | Colour |
  |---|---|---|
  | Bar per visible column | height 10, `r_sm`, width from the foundation pattern `[0.6, 0.45, 0.8, 0.5, 0.7]` of the column width, inset `sp4` | `skeleton` token (UI-FND-002) |
  | Row background | `row_h` | `list_bg` / `row_alt` alternating |

- **States**: shown once per session per tab until the component's `header.validity` is not
  `WarmingUp` or rows arrive. Never shown again after that, even if the tab is left and re-entered
  (APP-059). If the source fails first, UI-SYS-005 error state replaces the skeleton.
  Reduced motion: static at opacity 0.7 (UI-FND-024).
  Services also pushes the toast "Loading service data..." (`ToastQueue::push` kind Progress, key
  `ToastKey::SERVICES_LOADING`) on first visit and removes it when the skeleton goes (SYS-016).
- **Interaction**: keys and clicks on the body do nothing while the skeleton shows; the header
  is live (sort clicks are queued in the query).
- **Data binding**: `component.header.validity` and `rows.is_empty()`; a per-page
  `first_data_seen: bool` that never resets in the session.
- **Build**: the page renders `TlSkeletonRows` in place of the `TlTreeTable` body (outside the
  `uniform_list`, as UI-FND-024 requires). The kit `Skeleton` under it is allowed here because it
  is not a live table cell (ADR-009 forbids it in cells and charts) and it stops requesting frames
  once removed.
- **Performance**: the kit Skeleton requests frames while visible (2 s loop). It is visible for
  well under 1 s on this machine (assumed). Twelve rows × up to 7 bars is 84 quads.
- **Accessibility**: the status chip text reads "Loading…" (chosen) while the skeleton shows.
- **Tests**: `skeleton_shows_once_per_session` (first visit shows it; data arrives; leave and
  return; assert no skeleton); `services_loading_toast_removed_on_data`.
- **Phase**: P2.
- **Decided here**: "Loading…" status text.

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
  | Services, neither manager answers | `server` | "systemd not found" (spec) | "tasklens lists services from systemd. Neither the system manager nor the user manager answered." (new) | "Retry" → `RefreshCurrentTab` |
  | Any table, text filter or state filters exclude every row | `funnel-x` | "No matches" (new) | "No services match the current filters." / "No startup entries match the filter." / "No connections match the filter." (new) | Startup Apps and Connections: "Clear filter" dispatching `ClearFilter` (UI-FND-029). Services: "Clear filters" (new) dispatching `services::ClearAllFilters`, which clears the text and resets the strip to defaults |
  | Startup Apps, no entries at all | `app-window` | "No startup entries" (new) | "Nothing starts automatically when you log in." (new) | none |
  | Connections, no sockets | `network` | "No connections" (new) | none | none |
  | Source `Failed` before first data | `circle-alert` in `status.warning` | "Could not load services" / "Could not load startup entries" / "Could not load connections" (new) | `SourceHealth.last_error`, one line, ellipsis | "Retry" → `RefreshCurrentTab` |

- **States**: the empty state replaces only the body. When data arrives, rows replace it without
  animation. A `Failed` source after first data keeps the last rows and shows the status chip health
  item (APP-029, UI-SHL-051); no empty state.
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
  | Copy the clicked cell | "Copy" | none | `CopyCell { col }` (column captured at right-click) | cell text as shown (formatted), toast "Copied: <value>" with the value truncated to 60 chars plus "…" |
  | Copy row | "Copy row" | Ctrl+C | `CopyDetails` | the fixed block of the table (UI-SYS-020, UI-SYS-048, UI-SYS-066); multi-select blocks joined by one blank line |
  | Copy rows as table | "Copy rows as table" | Ctrl+Shift+C | `CopyRowsAsTable` | TSV of the visible columns in view order, header line first, one line per selected row in view order |

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
  1. A toast per action through `ToastQueue::push`, replaced in place by `ToastKey` (the
     `ActionId.0`, or a hash of `(ActionId.0, target index)` for multi-target requests).
  2. The row's own cell while the action is pending (Services Status cell, UI-SYS-018).
  3. The menu: the same action on the same target is disabled while pending, with tooltip
     "A service action is already in progress." (Services) (spec text).
- **States** (one toast lifecycle):

  | ActionState | Toast kind (UI-FND-020 icons and durations) |
  |---|---|
  | request sent, no event yet | Progress (spinner; static with reduced motion) |
  | `Started` | Progress, unchanged |
  | `Succeeded` | Success, 5 s |
  | `Failed(..)` | Error, 8 s (15 s when it has a "View logs" button) |
  | `OutcomeUnknown(..)` | Warning, 5 s |
  | `Rejected(Busy)` | Warning, 5 s |
  | other `Rejected` | Error, 8 s |

  Service job toasts are exempt from the 120 s Progress cap in UI-FND-020, because SYS-005 says
  there is no client-side timeout (systemd enforces `TimeoutStartSec`). The toast stays Progress
  until the final event. Every other action here keeps the 120 s cap.

  Polkit prompt state: tasklens cannot see the prompt. While polkit-kde-authentication-agent shows
  its dialog the toast stays in the request-sent state. When the call returns:
  - polkit dismissed by the user → "Authentication cancelled" (Warning, 5 s) (spec);
  - not authorized → "<failure text>: not authorized" (Error), for example
    "Failed to start service: not authorized" (spec);
  - no agent running (`InteractiveAuthorizationRequired`) → "<failure text>: no authentication
    agent is running" (new, Error).
  How the engine tells "dismissed" from "not authorized" is open item O-3.

- **Interaction**: a toast with a result button ("View logs" on service failures, "Show" on
  not-visible jumps) lasts 15 s (UI-FND-020). Clicking the toast body does nothing.
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
- **Decided here**: kind per state; the Progress-cap exemption for service jobs; the 4-target
  summary rule and its strings; the no-agent string.

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
  UI-SYS-016 (child rows), UI-SYS-022 (status text).
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
  | State toggles | `TlButton::Segment` × 4 | height 22, padding x `sp4`, `r_sm` | "Running", "Failed", "Stopped", "Disabled" (`t_sm`) | multi-select; none on = all states |
  | Unit types | `TlButton` ghost, small, with `chevron-down` caret, opens a `TlMenu` of check items | height 22 | "Unit types" plus " (4)" when not all types are on (chosen) | UI-SYS-012 |
  | Manager toggles | `TlButton::Segment` × 2 | as above | "System", "User" | both on by default |
  | Remove filters | `TlButton` ghost small, icon `funnel-x` 14 | height 22 | "Remove filters" | disabled when the strip is at its defaults |

  `TlButton::Segment` look (foundation.md owns the values; repeated here for the reviewer): off =
  transparent fill, 1 px `divider` border, `text_secondary`; on = `accent` at alpha 0.25 fill,
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
  expanded (chosen). Action `ToggleExpand`.
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
  opens Service Properties (`OpenProperties`); right-click opens UI-SYS-017. Hover tooltip on
  the name cell after 600 ms: three lines "<Description>", "<FragmentPath>", "Manager: System"
  (chosen).
- **Data binding**: `ServicesSnapshot.units[record]`: `unit: UnitRef`, `description`, `status:
  ServiceStatus`, `startup: StartupType`, `main_pid`, `user`, `path`, `kind: UnitKind`,
  `has_cgroup_procs: bool`, resource fields with `Validity`.
- **Build**: `ServicesDelegate::cell`; strings come from the snapshot as `Arc<str>` wrapped in
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
  | 8 | "Properties" | Enter | `OpenProperties` | always | always |
  | 9 | "File properties" | | `services::FileProperties` | always | binary or unit file known |
  | 10 | "Open file location" | | `services::OpenFileLocation` | always | binary known |
  | 11 | "Open unit file" | | `services::OpenUnitFile` | always | `FragmentPath` not empty |
  | – | separator | | | | |
  | 12 | "Copy" | | `CopyCell { col }` | always | always |
  | 13 | "Copy row" | Ctrl+C | `CopyDetails` | always | always |
  | 14 | "Copy rows as table" | Ctrl+Shift+C | `CopyRowsAsTable` | always | always |
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

### UI-SYS-022 Services status text and refresh

- **Implements**: SYS-016, APP-029.
- **Anatomy**: the tab text segment of the shell status chip (UI-SHL-050 owns font and colours):
  "255 Total | 198 Running | 57 Stopped", plus " | 3 selected" with a selection. Counts are unit
  rows after all filters (child rows not counted), thin-space grouped. Running counts ActiveState
  `active` and `reloading`; Stopped counts `inactive` and `failed`; Starting and Stopping count in
  Total only.
- **States**: while the skeleton shows: "Loading…". While paused: shell appends "[PAUSED - Ctrl]".
- **Interaction**: F5 (`RefreshCurrentTab`) sends `RefreshNow(SystemdSystem)` and
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

## Service Properties window

### UI-SYS-030 Service Properties window frame

- **Implements**: SYS-011 (frame), APP-045, R-073, SYS-015 ("View logs" entry).
- **Anatomy** (default 720 × 560, minimum 560 × 440, resizable; chosen):

  ```
  ┌ sshd.service Properties                                   📌  ✕ ┐  title bar (foundation)
  ├──────────────────────────────────────────────────────────────────┤
  │  General   Log On   Recovery   Dependencies   Logs                │  TlTabs, 32
  ├──────────────────────────────────────────────────────────────────┤
  │                                                                  │
  │  tab content, padding sp6 (16)                                    │
  │                                                                  │
  ├──────────────────────────────────────────────────────────────────┤
  │                                       [  OK  ] [Cancel] [Apply] │  footer 48, padding sp5
  └──────────────────────────────────────────────────────────────────┘
  ```

  | Part | Widget | Size | Colours |
  |---|---|---|---|
  | Frame | `TlSecondaryWindow` (`WindowKind::Floating`, own gpui-kit Root) | as above | `window_bg` |
  | Title | "<id> Properties", for example "sshd.service Properties" (spec) | | |
  | Pin | title bar pin (R-073) | | |
  | Tab strip | `TlTabs`, tabs "General", "Log On", "Recovery", "Dependencies", "Logs" | 32 high | `tab_bg`, active `accent` underline per foundation |
  | Content | per tab | fills | `window_bg` |
  | Footer | `h_flex` right-aligned, gap `sp4`, top border 1 px `divider` | 48 | `toolbar` |
  | OK | `TlButton` primary, min width 80 | 28 high | |
  | Cancel | `TlButton` default, min width 80 | | |
  | Apply | `TlButton` default, min width 80 | | disabled unless a field is changed |

- **States**:
  - Opening the same unit again focuses the existing window (chosen, as PROC-077).
  - At 10 open windows, an 11th open shows "Too many service properties windows are already open"
    in the main window (spec).
  - Unit removed or no longer loaded while open: the title appends " (not loaded)" (new), values
    stay as last seen in `text_secondary`, the Start, Stop, Restart buttons and Apply are disabled.
  - OpenRC: only the General tab is shown (SYS-018).
  - A tab whose data is still arriving shows "Collecting…" in its value fields.
- **Interaction**:
  - Opened by Properties (Enter, Alt+Enter, Ctrl+I, double-click) on a unit row, by "View logs"
    (opens on Logs), by a Dependencies row (UI-SYS-035) and by the failure toast "View logs".
  - With several unit rows selected, Properties opens one window per unit up to the cap (chosen,
    as PROC-048).
  - OK: applies pending changes (UI-SYS-031 Startup, UI-SYS-034 Recovery) and closes when every
    apply succeeds; on a failure the window stays open on the failing tab. Cancel and Esc close
    without applying, with no question (chosen). Enter in a non-text field triggers OK.
  - Ctrl+Tab / Ctrl+Shift+Tab switch tabs inside the window (chosen, matches the main window keys);
    Ctrl+1..Ctrl+5 select a tab (chosen).
  - Toasts for actions started from this window appear in this window (its own Root).
- **Data binding**: while open the window adds its `UnitRef` to `Demand.unit_details` (new, max
  10; open item O-1). The engine fetches `GetAll` once and follows `PropertiesChanged`, publishing
  `ServicesSnapshot.details: Arc<FxHashMap<UnitRef, Arc<UnitDetails>>>`. The window re-renders on
  `ComponentMask::SERVICES` only when its own `UnitDetails` Arc changed (pointer compare).
- **Build**: `ServicePropertiesWindow` entity in
  `crates/tl-ui/src/windows/service_properties/{mod.rs, general.rs, logon.rs, recovery.rs,
  dependencies.rs, logs.rs}`. A per-app registry `PropertiesWindows` holds open handles by
  `UnitRef` for focus-existing and the cap.
- **Performance**: one window renders only when its unit's details change; the Logs tab has its
  own rules (UI-SYS-036).
- **Accessibility**: Tab order: tab strip, tab content, footer buttons. Focus starts on the tab
  strip. Esc closes.
- **Tests**: `service_props_cap_10`; `service_props_focus_existing`;
  `service_props_ok_applies_then_closes`; `service_props_not_loaded_title`.
- **Phase**: P2.
- **Decided here**: sizes; focus-existing; " (not loaded)"; one window per selected unit; tab keys;
  Cancel without question.

### UI-SYS-031 Service Properties: General tab

- **Implements**: SYS-011 (General), SYS-007 (Startup combo), SYS-006 (buttons), R-050.
- **Anatomy**: `TlKeyValueGrid`, label column 120 (chosen), row height 28, label `text_secondary`
  `t_base` with a trailing ":", value `text_primary` `t_base`, values selectable.

  ```
  Name:       sshd.service
  Display:    OpenSSH Daemon
  Desc:       man:sshd(8)  man:sshd_config(5)              (TlLink each)
  Path:       /usr/bin/sshd -D                              (wraps, max 3 lines)
  Unit file:  /usr/lib/systemd/system/sshd.service          (TlLink)
  Drop-ins:   /etc/systemd/system/sshd.service.d/x.conf     (one TlLink per line)
  Startup:    [ Enabled          ▾ ]                        (TlSelect, width 200)
  Status:     ● Running (running) since 2026-10-03 09:12:44
              [Start] [Stop] [Restart]                      (TlButton small, gap sp4)
  Main PID:   812                                           (TlLink)
  Memory:     5.23 MiB
  Tasks:      1
  CPU time:   0:00:02
  Manager:    System
  ```

  Labels are the spec labels exactly: "Name", "Display", "Desc", "Path", "Unit file", "Drop-ins",
  "Startup", "Status", "Main PID", "Memory", "Tasks", "CPU time", "Manager".
- **States**:
  - Desc with no Documentation: "Not reported" (R-050 word). Drop-ins empty: "None" (chosen;
    an empty list is a real value). Main PID 0: "None" (chosen).
  - Status for inactive units: "Stopped (dead)" with no "since" part (chosen).
  - MemoryCurrent or TasksCurrent unset (systemd reports `u64::MAX`): "Not reported".
  - Startup combo: values "Enabled", "Disabled", "Masked". For Static, Indirect, Generated,
    Transient the combo is disabled, shows the current word, tooltip "This unit has no install
    section". Changing the value marks the tab dirty and enables Apply.
  - Start, Stop, Restart enablement as UI-SYS-017; pending state shows the overlay label in the
    Status line and disables all three.
- **Interaction**:
  - `man:` links run `xdg-open man:<page>`; `https:` and `http:` links run `xdg-open`. Failure
    toast "Could not open the link" (new).
  - Unit file and Drop-in links run `xdg-open <path>`; failure "Could not open the unit file" (new,
    shared with UI-SYS-017).
  - Main PID link: activates the main window and runs the PROC-071 jump with UI-SYS-021 toasts.
  - Apply or OK with a changed Startup: runs the UI-SYS-019 flow; on success the toast "Changes
    applied" (spec) replaces "Startup type changed" in this window; failure "Failed to change
    startup type".
  - Right-click on a value: `TlMenu` with "Copy value" (new), toast "Copied to clipboard".
- **Data binding**: `UnitDetails` fields `id`, `description`, `documentation`, `exec_start_argv`,
  `fragment_path`, `drop_in_paths`, `startup`, `active_state`, `sub_state`,
  `active_enter_timestamp`, `main_pid`, `memory_current`, `tasks_current`, `cpu_usage_nsec`,
  `manager`.
- **Build**: `general.rs`; the grid is `TlKeyValueGrid` with custom value elements for the combo,
  buttons and links.
- **Performance**: renders on details change only (PropertiesChanged, about once a second for
  Memory while the unit is busy).
- **Accessibility**: the combo has the accessible name "Startup type"; links have their text as
  name.
- **Tests**: `general_tab_labels_golden`; `startup_combo_disabled_for_static`;
  `apply_startup_toast_changes_applied`.
- **Phase**: P2.
- **Decided here**: label width; "None" for empty drop-ins and Main PID 0; "Stopped (dead)"; link
  failure strings; "Copy value" menu.

### UI-SYS-032 Service Properties: Log On tab

- **Implements**: SYS-012, R-050.
- **Anatomy**: read-only `TlKeyValueGrid`, label column 160 (chosen), labels "Log on", "Group",
  "Supplementary groups", "Runs as", "PAM service", "Manager", "Session" (spec).
  Values: "root", "dynamic user (<name>)", group list comma-separated, "uid 1000, gid 1000" (chosen
  format for Runs as), "System manager" or "User manager (oystein)", "graphical session" or "none".
- **States**: a missing value shows "Not reported" (R-050 words; this overrides SYS-012's "N/A"
  because R-050 sets the words for every Properties field). Runs as when MainPID is 0: "Not
  running" (chosen).
- **Interaction**: right-click "Copy value".
- **Data binding**: `UnitDetails` `user`, `group`, `dynamic_user`, `supplementary_groups`,
  `pam_name`, `main_pid_ids`, `manager`, `graphical_session`.
- **Build**: `logon.rs`.
- **Performance**: static.
- **Accessibility**: plain grid.
- **Tests**: `logon_missing_shows_not_reported`.
- **Phase**: P2.
- **Decided here**: label width; Runs as format; "Not running".

### UI-SYS-033 Service Properties: Recovery tab (view)

- **Implements**: SYS-013 (P2 view).
- **Anatomy**: read-only `TlKeyValueGrid`, label column 200 (chosen), rows and value formats:

  | Label (spec) | Value format |
  |---|---|
  | "On failure" | the `Restart=` word, for example "on-failure" |
  | "Restart service after" | "100 ms"; with steps: "100 ms, rising to 5000 ms in 5 steps" (chosen) |
  | "Give up after" | "5 starts within 10 s" |
  | "When giving up" | `StartLimitAction` word |
  | "Failure action" | `FailureAction` word |
  | "Run on failure" | one `TlLink` per OnFailure unit; "None" when empty |
  | "Restarts so far" | `NRestarts` count |

  Below the grid, `t_sm` `text_secondary`: "Editing recovery settings comes in a later version."
  (new) in P2 only. User units show the same.
- **States**: values missing: "Not reported".
- **Interaction**: Run on failure links open that unit's Properties.
- **Data binding**: `UnitDetails.recovery: RecoverySettings`.
- **Build**: `recovery.rs` (view mode).
- **Tests**: `recovery_view_formats`.
- **Phase**: P2.
- **Decided here**: value formats; P2 note text.

### UI-SYS-034 Service Properties: Recovery drop-in editor

- **Implements**: SYS-013 (P3 edit), R-004 `WriteServiceDropIn`, R-002, ADR-011 `write_atomic`.
- **Anatomy** (replaces UI-SYS-033 from P3):

  ```
  On failure:            [ on-failure        ▾ ]
  Restart service after: [   100 ] ms
  Give up after:         [     5 ] starts within [   10 ] s
  When giving up:        [ none              ▾ ]
  Failure action:        [ none              ▾ ]
  Run on failure:        [ failure-notify@%n.service             ]
  Restarts so far:       3
  ─────────────────────────────────────────────────────────────
  Saved as /etc/systemd/system/sshd.service.d/50-tasklens-recovery.conf     [Remove tasklens settings]
  ```

  | Field | Widget | Range or values |
  |---|---|---|
  | On failure | `TlSelect` width 200 | no, on-success, on-failure, on-abnormal, on-watchdog, on-abort, always |
  | Restart service after | `TlStepper` width 100, suffix "ms" | 0..=3 600 000, step 100 |
  | Give up after | two `TlStepper` width 80 | starts 0..=1000; within 0..=86 400 s |
  | When giving up, Failure action | `TlSelect` width 200 | none, reboot, reboot-force, reboot-immediate, poweroff, poweroff-force, poweroff-immediate, exit, exit-force |
  | Run on failure | `TlTextInput`, full width | unit names separated by spaces |
  | Restarts so far | text | read-only |
  | Saved as | `t_sm` `text_secondary`, selectable | the drop-in path from table (c) |
  | Remove tasklens settings (new) | `TlButton` small, default | shown only when the tasklens drop-in exists |

- **States**:
  - Clean / dirty: Apply enabled only when a value differs from the unit's current value.
  - Invalid: Run on failure tokens must match `^[A-Za-z0-9:_.\\@%-]{1,255}\.(service|socket|target|timer|mount|path|slice|scope)$`.
    Invalid input gets a 1 px `status.danger` border and the line below it, `t_sm`
    `status.danger`: "Enter unit names separated by spaces, for example failure-notify@%n.service"
    (new). Apply and OK are disabled while invalid.
  - A reboot or poweroff value in "When giving up" or "Failure action" shows a `t_sm`
    `status.warning` line under the field: "The computer will restart or power off when this
    happens." (new).
  - System unit, helper not installed: all controls disabled, tooltip "Needs the tasklens helper"
    (spec), and a line above the grid with `TlLink` "Additional setup…" opening the Additional
    Setup window.
  - System unit, helper installed but locked: controls enabled; Apply starts a helper session
    (polkit prompt for `io.github.oysteinkrog.tasklens.helper`).
  - Saving: toast "Saving recovery settings..." (new), then "Recovery settings saved" (new) or
    "Failed to save recovery settings: <error>" (new); polkit cancel "Authentication cancelled".
- **Interaction**: Apply or OK writes the drop-in with only the changed keys plus the keys already
  in the tasklens drop-in, then the engine calls `Manager.Reload()`. "Remove tasklens settings"
  deletes the tasklens drop-in after a `TlDialog` titled "tasklens": heading "Remove tasklens
  recovery settings?", text "The unit goes back to the settings in its unit file.", buttons
  "Remove" (danger) and "Cancel" (all new).
- **Data binding**: `ActionKind::UnitWriteRecovery(RecoveryEdit)` and
  `UnitRemoveRecovery` (new; open item O-2). `UnitDetails.recovery_dropin_exists: bool` (new).
- **Build**: `recovery.rs` (edit mode); form state in a `RecoveryForm` struct created on tab open,
  reset on Cancel.
- **Performance**: n/a.
- **Accessibility**: each control has its row label as accessible name; validation text is linked
  as the description.
- **Tests**: `recovery_invalid_unit_blocks_apply`; `recovery_helper_missing_disables`;
  `recovery_remove_confirm`.
- **Phase**: P3.
- **Decided here**: all ranges, the validation pattern, the warning and status strings, the remove
  flow.

### UI-SYS-035 Service Properties: Dependencies tab

- **Implements**: SYS-014.
- **Anatomy**: one vertical scroll region with three blocks:

  ```
  Depends on:                                      (t_base semibold, margin-bottom sp3)
  │ ● system.slice                      [Requires] │  rows 28
  │ ○ network-online.target             [Wants]    │
  Dependents:
  │ ● multi-user.target                 [WantedBy] │
  › Ordering (12)                                     (collapsed header row, chevron 16)
  ```

  | Part | Size | Style |
  |---|---|---|
  | Heading | `t_base` semibold `text_primary`; top margin `sp6` except the first | "Depends on:", "Dependents:" (spec) |
  | Row | `row_h`, padding x `sp4`, hover `row_hover`, selected `row_selected` | dot 8 per UI-SYS-003 service status, gap `sp3`, unit id `t_base`, tag right-aligned |
  | Tag | `TlBadge` small outline, `t_xs` | relation word: Requires, Requisite, Wants, BindsTo, PartOf, Upholds / RequiredBy, RequisiteOf, WantedBy, BoundBy, ConsistsOf, UpheldBy / After, Before |
  | Ordering header | `row_h`, chevron 16, "Ordering (N)" (chosen label with count) | collapsed by default |
  | Empty list | "(None)" in `text_secondary` (spec) | |

  A unit with several relations shows one row per relation (chosen). Rows are sorted by unit id.
- **States**: units not loaded in the snapshot: ring dot `text_secondary`.
- **Interaction**: click selects a row; Up/Down move; double-click or Enter opens that unit's
  Properties (counts toward the cap); right-click "Copy value" copies the unit id. Space or Enter
  on the Ordering header toggles it.
- **Data binding**: `UnitDetails.deps: Arc<[DepEntry { unit: UnitRef, relation: Relation }]>`;
  state dots from `ServicesSnapshot.units` by `UnitRef` lookup.
- **Build**: `dependencies.rs`; a `uniform_list` per block is not needed (lists are short); a plain
  `v_flex` with `overflow_y_scrollbar()`. If a block has more than 200 rows (chosen threshold) it
  uses `uniform_list`.
- **Performance**: rebuilt only when details change.
- **Accessibility**: rows are focusable; accessible name "<unit>, <relation>, <status>".
- **Tests**: `deps_empty_shows_none`; `deps_double_click_opens_props`.
- **Phase**: P2.
- **Decided here**: one row per relation; "Ordering (N)"; 200-row threshold.

### UI-SYS-036 Service Properties: Logs tab toolbar and list

- **Implements**: SYS-015, R-074.
- **Anatomy**:

  ```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ [⌕ Filter logs          ✕] [⤓ Follow] [Load older]          [⧉] [>_]     │  toolbar 32
  ├──────────────────────────────────────────────────────────────────────────┤
  │ ⓘ Only your own log messages are visible. Add your user to …  [Copy command] │  banner (when limited)
  ├──────────────────────────────────────────────────────────────────────────┤
  │ 2026-10-03 09:12:44  info     Server listening on 0.0.0.0 port 22.        │  lines 18
  │ 2026-10-03 09:13:02  warning  error: kex_exchange_identification: …      │
  │ 2026-10-03 09:13:05  err      fatal: Timeout before authentication (+2 lines) │
  │                                                     [ ↓ 12 new lines ]    │  pill
  └──────────────────────────────────────────────────────────────────────────┘
  ```

  | Part | Widget | Size | Notes |
  |---|---|---|---|
  | Toolbar | `h_flex`, gap `sp4`, padding y `sp2` | 32 | `toolbar` background, bottom border |
  | Filter | `TlTextInput` small with `search` prefix, cleanable, placeholder "Filter logs" (new) | width 220 | |
  | Follow | `TlButton::Segment` with icon `arrow-down-to-line` 14, label "Follow" (spec) | 24 | on by default |
  | Load older | `TlButton` small, label "Load older" (spec) | 24 | |
  | Copy | `TlButton::Icon` `copy` 14, tooltip "Copy logs to clipboard" (spec) | 24 × 24 | right side |
  | Open in terminal | `TlButton::Icon` `terminal` 14, tooltip "Open in terminal" (spec) | 24 × 24 | |
  | Banner | `TlBanner` info, `t_sm` | 32 | UI-SYS-038 |
  | List | `uniform_list`, font "Noto Sans Mono" `t_sm` (12), line height 18 (chosen) | fills | `list_bg`, no alternate rows |
  | Time column | 19 characters wide, `text_secondary` | | `YYYY-MM-DD HH:MM:SS` local (table d) |
  | Priority column | 7 characters wide, gap `sp4` each side | | words emerg, alert, crit, err, warning, notice, info, debug (journalctl names) |
  | Message | rest, one line, ellipsis | | multi-line messages show the first line plus " (+N lines)" in `text_secondary` |
  | New-lines pill | `TlButton` small, rounded, `accent` fill, `text_primary`, 16 above the bottom, centred | 24 | "↓ N new lines" (new) |

  Colouring by priority (spec plus chosen detail): 0 to 3 → priority word and message in
  `status.danger`; 4 → `status.warning`; 5 and 6 → `text_primary`; 7 → `text_secondary`. The time
  column is always `text_secondary`.
- **States**: UI-SYS-037 (follow and paging) and UI-SYS-038 (loading, empty, errors, access).
  Selected lines: `row_selected` with `text_selected_primary`; danger and warning colours stay.
- **Interaction**:
  - Filter: case-insensitive substring over the message and priority word, 100 ms debounce, applied
    to loaded lines only.
  - Lines: click selects one line, Shift+click a range, Ctrl+click toggles, Ctrl+A selects all
    shown lines, Ctrl+C copies the selected lines (key context `JournalList`, chosen). Hover over a
    message for 600 ms shows the full message (first 2000 characters) and "PID <n>" in a tooltip.
  - Copy button: copies all shown lines (after the filter) as
    "<time> <priority> <message>" with full multi-line messages, toast "Copied to clipboard".
  - Open in terminal: runs `xdg-terminal-exec journalctl -u <id> -b` (or `--user-unit <id>`), else
    `konsole -e journalctl …`. Failure toast "Could not open a terminal" (new).
- **Data binding**: `EngineClient::journal(JournalQuery { unit: UnitRef, follow: bool }) ->
  JournalHandle` (new; open item O-4). The handle yields `JournalEvent::{Batch { entries, older,
  exhausted }, Access(JournalAccess), Error(JournalError)}`. Entries:
  `JournalEntry { realtime_us, priority: u8, pid: Option<u32>, message: Arc<str>, cursor: Arc<str> }`.
  Dropping the handle kills the journalctl child.
- **Build**: `logs.rs`, entity `LogsView` owns `lines: VecDeque<LogLine>` (formatted time and
  priority `SharedString`s made once on arrival), `filtered: Vec<u32>`, `handle:
  Option<JournalHandle>`, `stick_to_bottom: bool`, `new_since_unstick: u32`.
- **Performance**: lines are formatted once; the list draws only visible lines; re-filtering
  10 000 lines is a linear scan off the render path (on input debounce), under 2 ms (estimate).
- **Accessibility**: toolbar buttons have their spec labels as accessible names; Follow is a
  toggle; the list is focusable and keyboard selectable.
- **Tests**: `logs_priority_colors`; `logs_filter_debounced`; `logs_copy_format`;
  `logs_multiline_suffix`.
- **Phase**: P2.
- **Decided here**: mono font, sizes, priority words, multi-line rendering, tooltip, key context
  `JournalList`, filter placeholder, failure toast, `JournalHandle` API.

### UI-SYS-037 Logs tab: follow mode and Load older

- **Implements**: SYS-015, R-074.
- **States and behaviour**:
  - Open: the tab requests the last 200 entries of the current boot. With Follow on (default) the
    engine keeps a `journalctl -f` child and appends batches.
  - Stick to bottom: when the list is scrolled to within one line of the bottom, new lines scroll
    it so the newest line stays visible. Scrolling up by more than one line unsticks; new lines
    then only count into the pill "↓ N new lines". Clicking the pill (or pressing End) scrolls to
    the bottom and sticks again.
  - Follow off: the engine kills the `-f` child at once; no new lines arrive; the pill hides.
    Follow on again: the engine resumes after the last cursor (`--after-cursor`), so no lines are
    lost or doubled.
  - Leaving the Logs tab or closing the window drops the handle (child killed, spec). Coming back
    to the tab keeps the loaded lines and resumes from the last cursor (chosen).
  - Load older: fetches 200 entries before the oldest loaded one; the button shows "Loading…"
    (new) and is disabled meanwhile; lines are inserted at the top and the scroll position is kept
    on the line that was first. When a fetch returns fewer than 200, the button reads "No older
    entries" (new) and stays disabled.
  - Cap: at most 10 000 lines are kept (chosen). In follow mode the oldest lines are dropped beyond
    the cap and Load older is enabled again.
  - Reduced motion: the pill appears without fade; scrolling to the bottom is instant either way.
- **Interaction**: End key in the list = click on the pill; Home scrolls to the top.
- **Data binding**: `JournalHandle::load_older(&self)` and `set_follow(&self, bool)` (new).
- **Build**: in `LogsView`; the scroll position check uses `UniformListScrollHandle`.
- **Performance**: appends are batched by the engine at most every 100 ms (chosen), so a chatty
  unit causes at most 10 list updates per second.
- **Accessibility**: the pill is focusable and has the same label as its text.
- **Tests**: `follow_sticks_to_bottom`; `follow_unstick_counts_new_lines`;
  `load_older_keeps_anchor`; `follow_off_kills_child` (fake engine records the drop).
- **Phase**: P2.
- **Decided here**: stick threshold, pill, resume on return, "Loading…", "No older entries",
  10 000-line cap, 100 ms batching.

### UI-SYS-038 Logs tab: loading, empty, error and access states

- **Implements**: SYS-015 (privilege banner), APP-042 / PERF-105 ("Journal access" row).
- **States** (all in the list area unless noted; text `t_base` `text_secondary`, centred):

  | State | Display |
  |---|---|
  | First fetch running | "Loading logs…" (new); no skeleton (the fetch is short) |
  | No entries this boot | "No log entries for this unit in the current boot." (new) |
  | Filter hides all | "No log entries match the filter." (new) |
  | `journalctl` not found | `TlEmptyState` icon `circle-alert`, heading "journalctl not found" (new), no button |
  | 5 s timeout | `TlEmptyState` heading "Reading the journal took too long." (new), button "Retry" |
  | Non-zero exit | `TlEmptyState` heading "Could not read the journal" (new), text = first stderr line, button "Retry" |
  | Own entries only (user not in `wheel`, `adm` or `systemd-journal`) | `TlBanner` (info) above the list: "Only your own log messages are visible. Add your user to the systemd-journal group to see all." (spec), button "Copy command" (new) copying `sudo usermod -aG systemd-journal $USER` with toast "Copied to clipboard"; from P3 also a `TlLink` "Additional setup…" opening the Additional Setup window |

  The banner cannot be dismissed (chosen; it states a fact about the data shown).
- **Interaction**: Retry re-requests the last 200 entries.
- **Data binding**: `JournalEvent::Access(JournalAccess::OwnOnly)` and `Error(..)`.
- **Build**: `logs.rs`.
- **Tests**: `logs_access_banner_shown`; `logs_timeout_retry`.
- **Phase**: P2 (Additional setup link P3).
- **Decided here**: all "(new)" strings; banner not dismissable; no skeleton.

---

# C. Startup Apps tab

### UI-SYS-040 Startup Apps page layout

- **Implements**: SYS-019, SYS-020, SYS-027, R-023 (tab 4), R-082.
- **Anatomy** (no filter strip):

  ```
  ┌ tab strip (shell) ──────────────────────────────────────────── Filter: [        ] ┐
  ├ Name ─────────── Publisher ────── Status ─ Startup impact ─ Type ──────── Location ─ Command ┤ 28
  │ ▣ Discord        discord 0.0.111  ● Enabled  Not measured   Autostart (user)  ~/.config/autostart  /usr/bin/discord --start-minimized │
  │ ▣ KDE Connect    kdeconnect 25.08 ● Enabled  Low            Autostart (system) /etc/xdg/autostart   kdeconnectd │
  │ ⚙ syncthing      syncthing 2.0.3  ● Enabled  Medium         systemd user unit  ~/.config/systemd/user/… │
  │ ▤ ssh-agent.sh   User             ○ Disabled None           Plasma pre-startup script ~/.config/plasma-workspace/env │
  ├────────────────────────────────────────────────────────────────────────────────────┤
  │ 12 Total | 9 Enabled | 3 Disabled                                                   │ 22
  └────────────────────────────────────────────────────────────────────────────────────┘
  ```

  Flat list, no groups, no tree.
- **States**: loading UI-SYS-004; empty and error UI-SYS-005. Live rescans (inotify, user manager
  `UnitFilesChanged`) update rows in place; selection is kept by `StartupRef` (R-082).
- **Interaction**: key context `StartupTable` inside `Table`. Tab order: table only.
- **Data binding**: `Snapshot.startup` (`ComponentMask::STARTUP`); `Demand.startup = true` while the
  tab is active (rescans and inotify stop when it is not, R-082).
- **Build**: `StartupPage` in `crates/tl-ui/src/pages/startup.rs` with
  `table: Entity<TlTreeTable<StartupDelegate>>`.
- **Performance**: rows change only on rescans.
- **Accessibility**: as tables.
- **Tests**: `startup_page_layout_golden`; `startup_rescan_keeps_selection`.
- **Phase**: P2.
- **Decided here**: none beyond the parts.

### UI-SYS-041 Startup columns

- **Implements**: SYS-020, SYS-027, R-011.
- **Anatomy**:

  | Id | Header label | Default width | Min width | Align | Content |
  |---|---|---|---|---|---|
  | `name` | "Name" | 220 | 120 | left | icon 16 and the localised `Name` or unit id |
  | `publisher` | "Publisher" | 180 | 60 | left | "name version", "User", or empty |
  | `status` | "Status" | 90 | 80 | left | UI-SYS-003 startup status |
  | `impact` | "Startup impact" | 100 | 80 | left | UI-SYS-043 |
  | `type` | "Type" | 190 | 60 | left | Type words from SYS-020 |
  | `location` | "Location" | 260 | 60 | left | directory path; `$HOME` shown as `~` (chosen) |
  | `command` | "Command" | 170 | 60 | left | `Exec`, `ExecStart` or script path |

  Display order is as listed (spec). All seven are visible by default; there are no optional
  columns. Widths are spec.
- **States**:
  - Default sort `status` ascending, then `name` ascending (R-011). Status order ascending:
    Enabled, Disabled, Masked, Not in this desktop, Missing program (chosen; Enabled first per
    spec). Impact order ascending: High, Medium, Low, None, Not measured (chosen).
  - Text columns ellipsis at the end; Location and Command show the full value in a tooltip.
- **Interaction**: header click sorts; header right-click opens the shared header menu; View >
  Columns acts on this table.
- **Data binding**: `state.toml` `[sort.startup]`, `[columns.startup]`.
- **Build**: `StartupDelegate::columns()`.
- **Tests**: `startup_default_sort_status_then_name`; `startup_columns_widths`.
- **Phase**: P2.
- **Decided here**: sort orders for Status and Startup impact; `~` for `$HOME`.

### UI-SYS-042 Startup row anatomy

- **Implements**: SYS-019, SYS-020.
- **Anatomy**:

  | Cell | Content | Style |
  |---|---|---|
  | name | icon 16 (the entry's `Icon=` resolved by the engine; fallback Lucide `app-window` for autostart entries, `cog` for systemd user units, `file-code` for Plasma scripts), gap `sp3`, name | `text_primary` |
  | publisher | text | `text_primary`; "User" in `text_secondary` |
  | status | dot and label (UI-SYS-003) | |
  | impact | UI-SYS-043 | |
  | type | text | `text_primary`; "Autostart (system, overridden)" in `text_secondary` |
  | location | text | `text_secondary` |
  | command | text | `text_primary` |

  Shadowed system entries (overridden) are drawn with all text in `text_secondary` (chosen) so the
  effective user entry stands out.
- **States**: hover, selected, focused as tables. Pending enable or disable: the Status cell shows
  an `status.info` dot and "Updating…" (new) until the result event.
- **Interaction**: double-click or Enter opens Startup Properties. Name tooltip after 600 ms: the
  full command and the source file path (two lines).
- **Data binding**: `StartupSnapshot.entries[record]`: `id: StartupRef`, `name`, `icon_path`,
  `publisher`, `status`, `impact: Impact`, `impact_detail`, `kind: StartupKind`, `location`,
  `command`, `source_path`, `can_toggle`, `can_delete`, `can_run`, `overridden_by_user: bool`.
- **Build**: `StartupDelegate::cell`; icons with `img(path)` at 16 × ui scale.
- **Performance**: icons are cached by GPUI's image cache by path.
- **Accessibility**: row name "<Name>, <Status>, <Type>".
- **Tests**: `startup_row_fallback_icons`; `startup_overridden_row_secondary`.
- **Phase**: P2.
- **Decided here**: fallback icons; overridden row styling; "Updating…".

### UI-SYS-043 Startup impact display

- **Implements**: SYS-021.
- **Anatomy**: text only (no bar), `t_base`:

  | Value | Colour |
  |---|---|
  | High | `status.warning` |
  | Medium | `text_primary` |
  | Low | `text_primary` |
  | None | `text_secondary` |
  | Not measured | `text_secondary` |

  Tooltip (600 ms) for High, Medium, Low, three lines (spec content, chosen wording):
  ```
  Activation time: 412 ms
  Started 3.2 s after login
  CPU time: 0:00:01
  ```
  For units of `Type=simple` or `Type=exec` a fourth line: "For this unit type only the time to
  start the program is measured." (new). For None: "Disabled entries do not run at login." (new).
  For Not measured: "This entry did not start through systemd in this session." (new).
- **States**: the value is computed once when the tab opens and on `UnitFilesChanged`; no live
  updates otherwise.
- **Interaction**: none besides the tooltip; sorting per UI-SYS-041.
- **Data binding**: `entry.impact`, `entry.impact_detail { activation_ms, start_offset_ms, cpu_ns,
  fork_exec_only: bool }`.
- **Build**: `StartupDelegate::cell` plus `tooltip`.
- **Tests**: `impact_tooltip_lines`; `impact_thresholds_1000_300` (fake data 999 ms → Medium,
  1000 ms → High, 299 ms → Low).
- **Phase**: P2.
- **Decided here**: colours; tooltip wording; the three new tooltip strings.

### UI-SYS-044 Startup context menu

- **Implements**: SYS-022 to SYS-025, R-091 (applied).
- **Anatomy**: `TlMenu`, min width 220:

  | # | Label | Key hint | Action | Enabled when |
  |---|---|---|---|---|
  | 1 | "Disable" or "Enable" (by state; both shown for a mixed selection) | | `startup::Disable` / `startup::Enable` | `can_toggle` for at least one target |
  | 2 | "Run now" | | `startup::RunNow` | `can_run` for at least one target |
  | – | separator | | | |
  | 3 | "Copy" | | `CopyCell { col }` | always |
  | 4 | "Copy row" | Ctrl+C | `CopyDetails` | always |
  | 5 | "Copy rows as table" | Ctrl+Shift+C | `CopyRowsAsTable` | always |
  | 6 | "Copy command" | | `startup::CopyCommand` | one target |
  | 7 | "Open file location" | | `startup::OpenFileLocation` | one target |
  | 8 | "Properties" | Enter | `OpenProperties` | always |
  | – | separator | | | |
  | 9 | "Delete entry…" | Delete | `startup::Delete` | `can_delete` for at least one target |

- **States**: disabled items carry a tooltip with the reason:
  - Not in this desktop: "This entry is not used in this desktop (OnlyShowIn or NotShowIn)." (new)
  - Missing program: "The program for this entry is not installed (TryExec)." (new)
  - Masked systemd unit: "This unit is masked. Change its startup type in the Services tab." (new)
  - Delete on a system XDG entry: "System entries cannot be deleted. Use Disable instead." (new)
  - Delete on a package-owned unit: "This unit belongs to a package. Use Disable instead." (new)
- **Interaction**: as UI-SYS-017 (capture at open, Shift+F10). The Delete key in `StartupTable`
  opens the delete confirmation (chosen key; table b has no startup row). On an entry that cannot
  be deleted, Delete shows the toast "This startup entry cannot be deleted" (spec).
- **Data binding**: captured `SmallVec<[StartupRef; 1]>`.
- **Build**: `StartupDelegate::context_menu`.
- **Tests**: `startup_menu_order`; `startup_menu_mixed_shows_both`; `delete_key_on_system_entry_toast`.
- **Phase**: P2.
- **Decided here**: both labels for mixed selection; reason tooltips; Delete key.

### UI-SYS-045 Enable and disable flow

- **Implements**: SYS-022.
- **States**: no pending toast (the action takes at most 2 s); the row shows "Updating…" while
  pending. Results: "Startup entry enabled." / "Startup entry disabled." (spec); failure "Failed to
  update startup entry: <error>" (spec text plus the error, chosen). Targets whose state would not
  change are skipped. With 4 or more targets, the UI-SYS-007 summary rule applies: "3 startup
  entries enabled", "2 of 3 startup entries enabled. 1 failed." (new pattern).
- **Interaction**: no confirmation. The change applies at next login; running programs are not
  stopped (no text says so in the toast; the Properties window shows Status only).
- **Data binding**: `ActionKind::StartupEnable` / `StartupDisable` (ADR-005), targets
  `ActionTarget::Startup(StartupRef)`.
- **Build**: `pages/startup.rs` handlers through `ActionTracker`.
- **Tests**: `startup_enable_toast`; `startup_disable_skips_already_disabled`.
- **Phase**: P2.
- **Decided here**: error suffix; skip rule; summary strings.

### UI-SYS-046 Run now flow

- **Implements**: SYS-023.
- **States**: result toasts only: "Startup entry launched." / "Failed to launch startup entry"
  (spec) with ": <error>" appended (chosen) / "Already running" (spec, Info). No row change.
- **Interaction**: works whatever the Status. Multi-select launches each target (summary rule at 4
  or more: "4 startup entries launched" (new)).
- **Data binding**: `ActionKind::StartupRunNow` (new; open item O-2).
- **Tests**: `run_now_already_running_toast`.
- **Phase**: P2.
- **Decided here**: error suffix; summary string; new ActionKind.

### UI-SYS-047 Delete startup entry confirmation

- **Implements**: SYS-024, APP-045.
- **Anatomy**: `TlDialog` modal window titled "tasklens Delete Startup Entry" (spec), width 440
  (chosen), height fits content:

  ```
  ┌ tasklens Delete Startup Entry                          📌 ✕ ┐
  │  ⚠  Delete startup entry                                    │  heading t_md semibold
  │     Permanently remove this startup entry?                  │  t_base
  │     The autostart file will be deleted. The app stays       │  type line, t_base text_secondary
  │     installed.                                              │
  │     Discord                                                 │  entry name, t_base medium
  │     ~/.config/autostart/discord.desktop                     │  source path, t_sm text_secondary, selectable
  │                                         [Cancel] [Delete]   │  footer
  └─────────────────────────────────────────────────────────────┘
  ```

  Icon `triangle-alert` 20 in `status.warning`. "Delete" is a danger `TlButton`; "Cancel" is
  default. Type lines (spec): user XDG entry "The autostart file will be deleted. The app stays
  installed."; user systemd unit "The unit will be disabled and its unit file deleted."; Plasma
  script "The script file will be deleted.".

  Multi-select (chosen): heading "Delete startup entries" (new), text "Permanently remove these
  <n> startup entries?" (new), then up to 5 names, then "and <n> more" (new). Entries that cannot
  be deleted are left out and a line says "<n> entries cannot be deleted and are skipped." (new).
- **States**: focus starts on "Cancel" (chosen); Enter triggers the focused button; Esc cancels.
  While deleting, both buttons are disabled and "Delete" shows a 14 px spinner (static with reduced
  motion); the window closes on the result.
- **Interaction**: results "Startup entry deleted." / "Failed to delete startup entry" (spec) in the
  main window.
- **Data binding**: `ActionKind::StartupDelete` (new; open item O-2).
- **Build**: `crates/tl-ui/src/windows/confirm_startup_delete.rs` using `TlDialog`.
- **Tests**: `delete_dialog_texts_per_type`; `delete_dialog_focus_on_cancel`;
  `delete_multi_skips_undeletable`.
- **Phase**: P2.
- **Decided here**: width; name and path lines; focus on Cancel; multi-select texts.

### UI-SYS-048 Startup copy formats and Open file location

- **Implements**: SYS-025, R-012.
- **Anatomy**: "Copy row" block, exactly:

  ```
  Name: …
  Publisher: …
  Status: …
  Startup impact: …
  Type: …
  Location: …
  Command: …
  Resolved path: …
  ```

  Location prints the full path (no `~`; chosen, so the copy is usable in a shell).
- **Interaction**:
  - "Copy command" copies the Command value; toast "Startup command copied." (spec).
  - "Open file location": `FileManager1.ShowItems` on the source file; toasts "Opening file
    location" / "Failed to open file location" (APP-044); file gone: "Startup file location
    unavailable." (spec).
- **Tests**: `startup_copy_block_golden`; `open_location_missing_file_toast`.
- **Phase**: P2.
- **Decided here**: full path in copy.

### UI-SYS-049 Startup status text, refresh and keyboard

- **Implements**: SYS-027.
- **Anatomy**: status text "12 Total | 9 Enabled | 3 Disabled", plus " | 2 selected". Disabled
  counts every entry that is not Enabled (chosen, so Total = Enabled + Disabled). Counts follow the
  text filter.
- **Interaction**: F5 sends `RefreshNow(Startup)`; toast "Startup entries refreshed." (spec) when the
  next startup version arrives or after 2 s. Keys: table (b) Table rows; Enter, Alt+Enter, Ctrl+I
  Properties; Delete delete entry (UI-SYS-044); Ctrl+C, Ctrl+Shift+C copy. Space does nothing (no
  tree).
- **Data binding**: `StartupSnapshot.counts` (new).
- **Tests**: `startup_status_text`; `startup_keymap`.
- **Phase**: P2.
- **Decided here**: Disabled count rule; refresh toast timing.

### UI-SYS-050 Startup Properties window

- **Implements**: SYS-026, APP-045.
- **Anatomy**: `TlSecondaryWindow` titled "Startup properties: <Name>" (spec), default 620 × 520,
  minimum 480 × 360 (chosen), not modal. Content: `TlKeyValueGrid` in a scroll region, padding
  `sp6`, label column 220, one copy button per line (`TlButton::Icon` `copy` 14 in a 20 × 20 hit
  area at the line's right end, `text_secondary` at 60% alpha, full alpha on row hover or focus).
  Footer 48 with one `TlButton` "Close" (default).

  | Label | Value |
  |---|---|
  | "Name" | name |
  | "Publisher" | publisher, or "Not reported" |
  | "Status / Enabled / State code" | for example "Enabled / Yes / active (running)"; State code "-" when there is no generated unit |
  | "Startup impact" | "Low (212 ms)"; "Not measured" without ms |
  | "Type / Source / Scope" | for example "Autostart (user) / XDG autostart / user"; Source words "XDG autostart", "systemd user unit", "Plasma pre-startup script", "Plasma logout script" (chosen) |
  | "Location" | directory |
  | "Source path" | file path |
  | "Resolved path" | binary path found on PATH, or "Not found" (chosen) |
  | "Command" | command, wraps |
  | "Can run / Can toggle / Can delete / File exists" | "Yes / Yes / No / Yes" |
  | "OnlyShowIn" | list, shown only when set |
  | "NotShowIn" | list, shown only when set |
  | "TryExec" | value, shown only when set |
  | "Autostart phase" | `X-KDE-autostart-phase`, shown only when set |
  | "Generated unit" | unit name, shown only for desktop entries |
  | "Overrides" | system path, shown only when a user file shadows one |

- **States**: values refresh from the snapshot on rescans. Entry removed: the title appends
  " (removed)" (new) and values stay in `text_secondary`. Up to 10 windows (chosen); the 11th shows
  "Too many startup properties windows are already open" (new). Opening the same entry focuses its
  window.
- **Interaction**: copy button copies that line's value; toast "Copied to clipboard". Esc and
  "Close" close. Ctrl+C with a line focused copies its value (chosen).
- **Data binding**: the entry in `Snapshot.startup` by `StartupRef`; `ComponentMask::STARTUP`.
- **Build**: `crates/tl-ui/src/windows/startup_properties.rs`.
- **Performance**: re-renders only when its entry changes (pointer compare).
- **Accessibility**: each copy button has the accessible name "Copy <label>".
- **Tests**: `startup_props_lines_golden`; `startup_props_removed_title`; `startup_props_cap`.
- **Phase**: P2.
- **Decided here**: sizes; Source words; "Not found"; " (removed)"; cap and its toast; optional
  rows shown only when set; label for X-KDE-autostart-phase.

---

# D. Connections tab

### UI-SYS-060 Connections page layout

- **Implements**: SYS-028, SYS-036, R-023 (tab 5), R-042.
- **Anatomy** (no filter strip):

  ```
  ┌ tab strip (shell) ──────────────────────────────────────────── Filter: [        ] ┐
  ├ Proto ↑ ─ Address local ───────── Address foreign ─────── State ──────── PID ──── Process ┤ 28
  │ TCP      0.0.0.0:22                *:*                     LISTENING        812   ⚙ sshd      │
  │ TCP      192.168.1.10:51234        140.82.112.4:443        ESTABLISHED     4321   ◉ firefox   │
  │ TCP      [::1]:631                 *:*                     LISTENING        690   ⚙ cupsd     │
  │ TCP      192.168.1.10:40012        34.107.221.82:443       TIME-WAIT          -   No owner (kernel) │
  │ UDP      0.0.0.0:5353              *:*                     UNCONNECTED      790   ⚙ avahi-daemon │
  ├──────────────────────────────────────────────────────────────────────────────────┤
  │ 214 Total | 168 TCP | 46 UDP | 97 EST | 31 LISTEN                                 │ 22
  └──────────────────────────────────────────────────────────────────────────────────┘
  ```

  Flat list. Rows keep identity by socket cookie across the 2 s refresh, so selection and scroll
  position are stable.
- **States**: loading UI-SYS-004; empty and error UI-SYS-005. Ctrl held: frozen on the paused
  snapshot (R-042). Rows that appear or disappear are not highlighted (chosen; TaskSlinger does not
  highlight them).
- **Interaction**: key context `ConnectionTable` inside `Table`.
- **Data binding**: `Snapshot.connections` (`ComponentMask::CONNECTIONS`) and `Snapshot.processes`
  (`::PROCESSES`) for owner names and icons. `Demand.connections = true` while the tab is active or
  a Connection Properties window is open.
- **Build**: `ConnectionsPage` in `crates/tl-ui/src/pages/connections.rs` with
  `table: Entity<TlTreeTable<ConnectionsDelegate>>`.
- **Performance**: one row rebuild per 2 s; visible cells only re-format when values change
  (`CellTextCache`). Addresses and states rarely change, so the hit rate stays high.
- **Accessibility**: as tables.
- **Tests**: `connections_selection_stable_across_refresh` (two snapshots with the same cookies in a
  new order; selection stays on the cookie); `connections_paused_on_ctrl`.
- **Phase**: P2.
- **Decided here**: no new or closed highlight.

### UI-SYS-061 Connections columns

- **Implements**: SYS-029, SYS-030, SYS-035, SYS-036, SYS-037, R-011, table (d).
- **Anatomy**:

  | Id | Header label | Default width | Min width | Align | Content | Default |
  |---|---|---|---|---|---|---|
  | `proto` | "Proto" | 90 | 60 | left | "TCP" or "UDP" | visible |
  | `local` | "Address local" | 260 | 120 | left, `tnum` | `a.b.c.d:port`, `[v6%scope]:port`, `0.0.0.0:*`, `[::]:*` | visible |
  | `foreign` | "Address foreign" | 260 | 120 | left, `tnum` | as local; `*:*` for listeners and unconnected UDP | visible |
  | `state` | "State" | 120 | 90 | left | SYS-029 labels, UI-SYS-003 colours | visible |
  | `pid` | "PID" | 80 | template `"999999 +99"` | right, `tnum` | UI-SYS-062 | visible |
  | `process` | "Process" | 180 | 60 | left | icon 16 and display name | visible |
  | `user` | "User" | 120 | 60 | left | user from the diag uid | hidden |
  | `family` | "Family" | 70 | 60 | left | "IPv4" or "IPv6" | hidden |
  | `local_port` | "Local port" | 90 | template `"99999"` | right | port number | hidden |
  | `foreign_port` | "Foreign port" | 90 | template `"99999"` | right | port, empty for `*` | hidden |
  | `recv_q` | "Recv-Q" | 80 | template `"999999"` | right | count, thin-space grouped | hidden |
  | `send_q` | "Send-Q" | 80 | template `"999999"` | right | count | hidden |
  | `interface` | "Interface" | 100 | 60 | left | `if_indextoname`, empty for 0 | hidden |
  | `inode` | "Inode" | 100 | template `"9999999999"` | right | integer, no grouping (chosen; it is an id) | hidden |
  | `service` | "Service" | 200 | 60 | left | owning unit id | hidden |
  | `remote_host` | "Remote host" | 240 | 60 | left | UI-SYS-069 | hidden, P4 |
  | `netns` | "Namespace" | 160 | 60 | left | UI-SYS-070 | only with "All" namespaces, P4 |

  Default-column widths are spec; optional widths are chosen.
- **States**: default sort `proto` ascending, then local port ascending (R-011). Sort keys (chosen):
  addresses sort by family, address bytes, then port; State sorts in the SYS-029 table order
  (LISTENING, SYN-SENT, SYN-RECEIVED, ESTABLISHED, FIN-WAIT-1, FIN-WAIT-2, CLOSE-WAIT, CLOSING,
  LAST-ACK, TIME-WAIT, CLOSED, UNKNOWN, UNCONNECTED, CONNECTED); PID sorts unknown ("?") and none
  ("-") after all numbers.
- **Interaction**: header click sorts; shared header menu; the column picker shows the Remote host
  entry with a second line "Sends reverse DNS queries for foreign addresses." (new) in
  `text_secondary`.
- **Data binding**: `state.toml` `[sort.connections]`, `[columns.connections]`.
- **Build**: `ConnectionsDelegate::columns()`.
- **Tests**: `connections_default_sort`; `connections_address_sort_numeric`.
- **Phase**: P2 (remote_host and netns P4).
- **Decided here**: optional widths; sort keys; inode format; picker note.

### UI-SYS-062 Connection row anatomy

- **Implements**: SYS-029, SYS-030.
- **Anatomy**:

  | Cell | Normal | Shared socket | Kernel-owned (TIME-WAIT, inode 0) | Other user's socket, unresolved |
  |---|---|---|---|---|
  | pid | "4321" | "4321 +2" (`+N` in `text_secondary`) | "-" `text_secondary` | "?" `text_secondary` |
  | process | icon 16, display name | owner with the lowest PID | "No owner (kernel)" `text_secondary`, no icon | lock icon 12 (ADR-003 Restricted glyph), `text_secondary` |

  - Tooltip on PID or Process for a shared socket: one line per owner "PID <n>  <name>" (all
    owners, max 16 lines then "and <n> more", chosen).
  - Tooltip for unresolved: "Owned by <user>. Needs the tasklens helper to identify the process"
    (spec). From P3 with the helper unlocked these rows resolve to normal owners.
  - Addresses use `tnum` so digits line up; IPv4-mapped IPv6 show in IPv4 form (spec).
- **States**: hover, selected, focused as tables. Pending close: State cell shows "Closing…" (new)
  in `status.info` until the row goes or the action fails.
- **Interaction**: double-click or Enter opens Connection Properties.
- **Data binding**: `ConnectionsSnapshot.records[i]`: `cookie`, `proto`, `family`, `local`,
  `foreign`, `state`, `owners: SmallVec<[ProcessKey; 1]>`, `owner_validity: Validity`, `uid`,
  `inode`, `ifindex`, `recv_q`, `send_q`, `unit: Option<UnitRef>`, `tcp: Option<TcpInfoView>`.
  Owner name and icon come from `Snapshot.processes` by `ProcessKey`.
- **Build**: `ConnectionsDelegate::cell`. Addresses are formatted by the engine once per
  socket (`Arc<str>`) since they never change for a cookie (chosen; saves UI formatting).
- **Performance**: the address strings are shared, so a refresh with unchanged sockets formats
  nothing.
- **Accessibility**: row name "<Proto> <local> to <foreign>, <State>, <Process>".
- **Tests**: `conn_row_shared_pid_plus_n`; `conn_row_kernel_owner`; `conn_row_restricted_lock`.
- **Phase**: P2 (helper resolution P3).
- **Decided here**: shared-owner pick and tooltip cap; "Closing…"; engine-formatted addresses.

### UI-SYS-063 Connections context menu

- **Implements**: SYS-032, SYS-031, SYS-033, SYS-010, R-012, R-091 (applied).
- **Anatomy**: `TlMenu`, min width 220:

  | # | Label | Key hint | Action | Shown / enabled |
  |---|---|---|---|---|
  | 1 | "Copy" | | `CopyCell { col }` | always (chosen addition, R-012) |
  | 2 | "Copy row" | Ctrl+C | `CopyDetails` | always |
  | 3 | "Copy rows as table" | Ctrl+Shift+C | `CopyRowsAsTable` | always |
  | 4 | "Copy local address" | | `connections::CopyLocal` | one target |
  | 5 | "Copy foreign address" | | `connections::CopyForeign` | one target; disabled for `*:*` |
  | – | separator | | | |
  | 6 | "Close TCP connection" | | `connections::CloseTcp` | shown from P3 for TCP targets; see UI-SYS-064 |
  | – | separator | | | |
  | 7 | "Jump to process" | | `connections::JumpToProcess` | one target; disabled with no owner (tooltip "No owning process for this connection") |
  | 8 | "Go to service" | | `connections::GoToService` | shown when the owner belongs to a systemd unit |
  | 9 | "Properties" | Enter | `OpenProperties` | always |

- **States**: disabled tooltips as listed; restricted owner: Jump is disabled with the
  Restricted tooltip.
- **Interaction**: capture at open; Shift+F10. "Copy local address" copies the address as shown,
  toast "Local address copied"; "Copy foreign address" toast "Foreign address copied" (spec).
- **Data binding**: captured `SmallVec<[u64; 1]>` cookies plus the records at open.
- **Build**: `ConnectionsDelegate::context_menu`.
- **Tests**: `conn_menu_order`; `conn_menu_close_hidden_before_p3` (feature flag off).
- **Phase**: P2 (item 6 from P3).
- **Decided here**: "Copy" cell item.

### UI-SYS-064 Close TCP connection flow

- **Implements**: SYS-031, R-002, R-004 `SockDestroy`.
- **States of the menu item**:

  | Condition | Item |
  |---|---|
  | Before P3 | hidden |
  | UDP target, or TIME-WAIT | hidden (spec: shown for TCP in any state except TIME-WAIT) |
  | Helper not installed | disabled, tooltip "Needs the tasklens helper" (spec) |
  | Kernel answered EOPNOTSUPP earlier this session | disabled, tooltip "Kernel lacks CONFIG_INET_DIAG_DESTROY" (spec) |
  | Pending close on this cookie | disabled, tooltip "Closing…" |
  | otherwise | enabled |

- **Flow**:
  1. Targets that are LISTENING need confirmation: `TlDialog` window titled "tasklens" (APP-045),
     heading "Close listening socket?", text "The program will stop accepting new connections on
     <addr>." (spec), buttons "Close socket" (danger) and "Cancel"; focus on "Cancel" (chosen).
     With several listening targets: heading "Close <n> listening sockets?" (new) and one address
     per line (max 5, then "and <n> more").
  2. Toast "TCP connection close requested" (spec; Info, pending icon).
  3. If the helper session is locked, polkit asks for `io.github.oysteinkrog.tasklens.helper`; the
     toast stays pending. Cancel: "Authentication cancelled".
  4. Result, replacing the toast: "TCP connection closed" (Success) or "Failed to close TCP
     connection: <errno text>" (Error) (spec). EOPNOTSUPP also sets the session flag above.
  5. The row disappears at the next refresh. Until then it shows "Closing…".
- **Data binding**: `ActionKind::SockDestroy` with target `ActionTarget::Socket { cookie,
  family, proto, four_tuple }` (new; open item O-2). `ConnectionsSnapshot.destroy_supported:
  Option<bool>` (new).
- **Build**: handler in `pages/connections.rs`; dialog through `TlDialog`.
- **Tests**: `close_listening_requires_confirm`; `close_eopnotsupp_disables_item`;
  `close_toast_sequence`.
- **Phase**: P3.
- **Decided here**: Cancel focus; plural heading; UDP hidden rather than disabled.

### UI-SYS-065 Jump to owning process and Go to service

- **Implements**: SYS-033, SYS-032, SYS-010, PROC-071.
- **Interaction**:
  - "Jump to process" uses the PROC-071 entry point with the owner `ProcessKey` (the lowest PID when
    shared). Toasts: "Switched to owning process." / "Owning process not visible in the current
    process view." with "Show" / "No owning process for this connection" / "Process not found in
    current view" (spec).
  - "Go to service" calls UI-SYS-021 with the owner's `UnitRef`, with its toasts.
- **Tests**: `conn_jump_uses_lowest_pid`; `conn_go_to_service`.
- **Phase**: P2.
- **Decided here**: lowest PID for shared sockets.

### UI-SYS-066 Connections copy format

- **Implements**: SYS-032, R-012.
- **Anatomy**: "Copy row" block, exactly:

  ```
  Protocol: …
  Local address: …
  Local port: …
  Foreign address: …
  Foreign port: …
  State: …
  PID: …
  Process: …
  ```

  Addresses without the port; IPv6 without brackets (chosen). `*` ports print "*". Shared sockets
  print all PIDs comma-separated (chosen). Toast "Connection copied" (spec).
- **Tests**: `conn_copy_block_golden`.
- **Phase**: P2.
- **Decided here**: address and port split rules; all PIDs.

### UI-SYS-067 Connections status text, filter and refresh

- **Implements**: SYS-036, SYS-037 (dropdown place).
- **Anatomy**: status text "214 Total | 168 TCP | 46 UDP | 97 EST | 31 LISTEN", plus " | 3
  selected". Counts follow the filter. From P4 the Namespaces control follows the text after `sp6`
  (UI-SYS-070).
- **Interaction**:
  - Filter: case-insensitive substring over Proto, both addresses with ports as shown, State, PID,
    Process and User (spec). Engine-side.
  - F5: `RefreshNow(Connections)`; toast "Connections refreshed." (spec) when the next connections
    version arrives or after 2 s.
- **Data binding**: `ConnectionsSnapshot.counts` (new).
- **Tests**: `connections_status_text`; `connections_filter_matches_port`.
- **Phase**: P2.
- **Decided here**: refresh toast timing.

### UI-SYS-068 Connection Properties window

- **Implements**: SYS-034, APP-045.
- **Anatomy**: `TlSecondaryWindow` titled "Connection properties: <Proto> <State>" (spec), for
  example "Connection properties: TCP ESTABLISHED", default 560 × 520, minimum 440 × 360 (chosen).
  `TlKeyValueGrid`, label column 200, copy button per line as UI-SYS-050. Footer 48 with "Close".

  | Label | Value |
  |---|---|
  | "Protocol / State / Family" | "TCP / ESTABLISHED / IPv4" |
  | "Local" | "<addr> Port <n>" |
  | "Foreign" | "<addr> Port <n>", or "*" for listeners |
  | "Process" | `TlLink` "<name> PID <n>" that jumps (UI-SYS-065); "No owner (kernel)"; or "Restricted" |
  | "User" | name |
  | "Inode" | integer |
  | "Interface" | name, or "None" |
  | "Recv-Q / Send-Q" | "0 / 0" |
  | "RTT" (TCP) | "0.42 ms (var 0.10 ms)" |
  | "Congestion window" (TCP) | "10 segments" (chosen unit word) |
  | "Bytes sent / received" (TCP) | "1.22 MiB / 34.68 MiB" |
  | "Retransmits" (TCP) | count |
  | "Last data received" (TCP) | "1 240 ms ago" |
  | "Closable here" | "Yes", or "No (<reason>)": "TIME-WAIT", "UDP", "needs the tasklens helper", "kernel lacks CONFIG_INET_DIAG_DESTROY"; before P3 the reason is "not supported in this version" (chosen) |

- **States**: refreshes every 2 s by cookie. When the socket is gone: the title appends " (closed)"
  (spec), values stay in `text_secondary`. Up to 10 windows (chosen); the 11th shows "Too many
  connection properties windows are already open" (new). Same cookie focuses its window. A missing
  tcp_info value shows "Not reported".
- **Interaction**: as UI-SYS-050; the Process link jumps.
- **Data binding**: the record in `Snapshot.connections` by cookie; `Demand.connections` stays on
  while any such window is open.
- **Build**: `crates/tl-ui/src/windows/connection_properties.rs`.
- **Performance**: renders at most every 2 s.
- **Accessibility**: as UI-SYS-050.
- **Tests**: `conn_props_closed_title`; `conn_props_tcp_lines_only_for_tcp`.
- **Phase**: P2.
- **Decided here**: sizes, unit words, reason texts, cap and its toast.

### UI-SYS-069 Remote host column

- **Implements**: SYS-035.
- **Anatomy**: text cell. Lookup pending: the address in `text_secondary`. Name found: the name in
  `text_primary`, tooltip the address. No name: the address in `text_primary`. Listeners and `*`
  show empty.
- **States**: lookups run only while the column is visible (chosen) and only for visible rows plus
  one viewport of overscan (chosen, at most 4 at once per spec).
- **Data binding**: `Demand.remote_host_cookies` (new, follows the visible-range rule of ADR-009).
- **Tests**: `remote_host_pending_shows_address`.
- **Phase**: P4.
- **Decided here**: visible-rows-only lookups.

### UI-SYS-070 Namespaces dropdown

- **Implements**: SYS-037.
- **Anatomy**: `TlButton::Ghost` sm (24 high) with a `chevron-down` `i_xs` caret, label
  "Namespaces: Host only" or "Namespaces: All" (chosen), opening a `TlMenu` with check items
  "Host only", "All". SYS-037 puts it in the status bar. The shell's status chip (UI-SHL-050) has
  no slot for a control, so this file asks the shell for an optional page control slot drawn
  after the tab text inside the chip (open item O-6). Until that exists, it is drawn by the
  Connections page as its own floating element at the bottom-right of the table body, `sp4` from
  both edges, `menu_bg` fill, 1 px `divider` border, `r_md` (chosen fallback).
- **States**: without the helper, "All" is disabled with tooltip "Needs the tasklens helper". With
  "All", the `netns` column is shown and listed first after Proto (chosen); switching back hides it.
- **Interaction**: choosing "All" with the helper locked starts a helper session (polkit). Cancel:
  "Authentication cancelled", the choice stays "Host only".
- **Data binding**: `Demand.connections_all_netns: bool` (new). Session-only (chosen; no key in
  table a).
- **Tests**: `netns_all_adds_column`.
- **Phase**: P4.
- **Decided here**: label, column position, session-only.

---
