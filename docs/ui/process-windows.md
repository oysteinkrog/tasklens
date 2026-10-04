# tasklens UI spec: process windows and dialogs

This file specifies the secondary windows of the processes area: the Process Properties window and its seven tabs, the App Properties window, the Set Affinity, Custom priority and Real-time signal dialogs, the Send Signal submenu, the Run dialog, the Search File Handles window, the low-memory rescue window, the dump toasts and the VirusTotal notice. IDs use the prefix UI-PWN.

Precedence: `docs/DECISIONS.md`, then `docs/RECONCILIATION.md`, then `docs/ARCHITECTURE.md`, then `docs/spec/merged/proc.md` and `sys-app.md`. Component and token names come from `docs/ui/foundation.md` (TlSecondaryWindow, TlDialog, TlTabs, TlButton, TlCheckbox, TlTextInput, TlSelect, TlStepper, TlSlider, TlKeyValueGrid, TlMenu, TlToast, TlTooltip, TlProgress, TlEmptyState, TlFocusRing and the tokens of `TlTheme`). Tables use `TlTreeTable` from `docs/ui/processes.md`; this file only says which columns, rows and menus each table has. It does not respecify the component.

## Conventions used in every entry

- **Sizes** are logical px at `ui_scale_pct = 100` and are multiplied by `ui_scale_pct / 100`. The row height `RH` is `28 × row_height_pct/100 × ui_scale_pct/100` (R-020). Spacing tokens `sp1` to `sp8`, text styles `t_xs` to `t_xl`, radii `r_sm` and `r_md`, motion `m_fast`, `m_base`, `m_slow` and `ease_out_cubic` are the shared vocabulary of the UI brief.
- **Measurements.** The TaskSlinger frames in `tasklens-ref/evidence-frames/advanced-*.png` are 1728 × 1080 and show 1.386 screen px per logical px (main-table row pitch 38.8 px = 28 logical). A value marked **[measured]** comes from those frames divided by 1.386; **[spec]** comes from a merged spec, R or D entry; **[chosen]** was decided in this file and is repeated in "Decided here".
- **Colours measured in the TaskSlinger Properties panel** (Default Dark): title row `rgb(40,46,50)` = `titlebar`; inactive tab `rgb(31,32,35)` = `tab_bg`; active tab `rgb(34,38,40)`, which joins the body `rgb(30,34,36)` = `window_bg`; tab separators `rgb(48..52)` = `divider`; key/value column panels `rgb(27,31,33)` = `list_bg`; table section header `rgb(50,54,58)` = `row_hover` tone, table column header `rgb(41,47,51)` = `header`. Labels are `text_secondary`, values `text_primary`.
- **Validity words.** Key/value fields use the PERF-013 words (`Collecting…`, `Restricted`, `Needs setup`, `Not supported`, `Not reported`, `Unavailable`, `Unknown`) in `text_secondary`, never a zero (R-050). Table cells inside these windows use the ADR-003 glyphs, as the process table does.
- **Numbers** follow canonical table (d): tabular figures (`tnum`), right-aligned in table cells, left-aligned in key/value fields.
- **Actions.** Every command names its GPUI `Action`. Actions are declared in the single `tasklens` namespace in `crates/tl-ui/src/windows/actions.rs`, with the prefixes `Props`, `Affinity`, `Priority`, `Signal`, `Run` and `HandleSearch` (R-128, R-133), unless the entry says otherwise. Key contexts used here: `PropsWindow`, `PropsTable`, `AffinityDialog`, `PriorityDialog`, `SignalDialog`, `RunDialog` (table b), `HandleSearch`, `LowMemory`, plus the shared `Table` context of TlTreeTable.
- **Tables.** Every table in these windows is a `TlTreeTable` (UI-PRC-001) whose owner implements `TreeTableDelegate`: rows are `RowKind::Plain` (or tree rows in Memory maps), `header_lines()` returns `HeaderLines::One` (28 px header, no totals line), `columns_fixed()` returns true (no picker, no Hide or Reset items; resize and sort still work; `on_columns_changed` keeps widths in memory only), and sorting, filtering and expansion are done by the owner. `RowId` per table: TID (Threads), FD number (File descriptors), node or region index (Memory maps), capability number or GID (Security), payload row index (Environment, Strings), `ProcessKey` (App Properties), hit index (Handle Search). `row_index(id)` must be O(1); Strings and Handle Search use the row index itself as the id. Cells are written into the caller's `CellOut` buffer, so the table's text cache skips unchanged cells.
- **Toasts** are pushed through the foundation's `ToastQueue::push(kind, text, key, action)` of the window that shows them (UI-FND-020).
- **Files.** New code lives under `crates/tl-ui/src/windows/`: `frame.rs`, `props/{mod.rs, general.rs, threads.rs, fds.rs, maps.rs, security.rs, environ.rs, strings.rs, filter.rs}`, `app_props.rs`, `affinity.rs`, `priority.rs`, `signal.rs`, `run.rs`, `handle_search.rs`, `low_memory.rs`, `notices.rs`, `actions.rs`. This extends the ADR-009 tree; it is listed in "Decided here".
- **Reduced motion** means `App::reduce_motion()` is true or `motion.animations` is false (R-017).

---

## A. Shared frame and data path

### UI-PWN-001 Secondary window frame (use of TlSecondaryWindow)

- **Implements:** APP-045, APP-009 (pin on secondary windows), R-073, PROC-056, PROC-077 (frame part), D-006.
- **Anatomy:** every window in this file is a `TlSecondaryWindow` from `docs/ui/foundation.md` UI-FND-021: 34 px kit title bar (`titlebar`), title `t_base` 500 with end truncation, pin `TlButton::Toggle` 34 × 34 left of the caption buttons, minimize and maximize only when `window.window_controls()` reports them, close with `status.danger` hover, 1 px `divider` under the bar. This file does not respecify the frame. It fixes only what the foundation leaves to the owner:

| Window | Entry | Kind (UI-FND-021) | Title | Default size | Minimum | Resizable |
|---|---|---|---|---|---|---|
| Process Properties | UI-PWN-003 | Properties | `Process Properties - {name} - PID {pid}` | 900 × 640 | 640 × 400 | yes |
| App Properties | UI-PWN-019 | Properties | `App Properties - {name}` | 720 × 560 | 640 × 400 | yes |
| Set Affinity | UI-PWN-021 | Confirmation-style modal | `tasklens Set Affinity` | 640 × content (360 to 640) | same | no |
| Set Priority (Custom) | UI-PWN-023 | Confirmation-style modal | `tasklens Set Priority` | 440 × content | same | no |
| Real-time signal | UI-PWN-027 | Confirmation-style modal | `tasklens Send Signal` | 360 × content | same | no |
| Confirmations (UI-PRC-014) | UI-PWN-025 | Confirmation | `tasklens` | 420 × content | 360 × 120 | no |
| Run | UI-PWN-028 | Tool | `tasklens Run` | 480 × 248 | 480 × 248 | no |
| Handle Search | UI-PWN-030 | Tool | `tasklens Handle Search` | 860 × 560 | 560 × 360 | yes |
| Low memory | UI-PWN-032 | Tool, no parent | `tasklens - Low memory` | 420 × 560 | 360 × 400 | yes |

  - "Confirmation-style modal" means `WindowKind::Dialog`, modal to the window that opened it, with the `TlDialog` footer, but with form content instead of a message.
  - The Run window is fixed-size, which overrides UI-FND-021's tool-window minimum of 480 × 320 and "resizable: yes" [chosen: its content has a fixed height]. UI-FND-021 carries the exception as its own row (R-127).
  - The App Properties minimum follows the foundation's Properties kind (640 × 400), not a smaller one.
- **States:** as UI-FND-021. The low-memory window starts pinned (UI-PWN-032).
- **Interaction:**
  - Esc order inside every window in this file: close the topmost popup or menu; else cancel a running scan or search (Strings, Handle Search); else clear a focused, non-empty filter field; else close the window (Cancel for modals). This is the table (b) Esc rule applied per window.
  - Ctrl+W closes Properties and tool windows.
  - Pin: `ToggleAlwaysOnTop { window: WindowKey, caption }` (UI-FND-021; the caption is the window title, R-176), state in memory only (R-073); handled by E20-029 through `KWinCommand::KeepAbove`.
- **Data binding:** windows are registered in the foundation's `WindowRegistry` with these keys, added by this file: `WindowKey::Properties(ProcessKey)` (exists), `WindowKey::AppProperties(GroupKey)`, `WindowKey::Run`, `WindowKey::HandleSearch`. Modal dialogs are not registered (they die with their parent).
- **Build:** `TlSecondaryWindow::open(cx, kind, title, size, build_content)` from `components/window_frame.rs`. Each window gets its own gpui-kit `Root`, so toasts and menus are per window (APP-045). Titles that change (Properties exited state) call `window.set_window_title`.
- **Performance:** the frame is static; it re-renders only on pin, focus or title change.
- **Accessibility:** as UI-FND-021.
- **Tests:** `pwn_esc_order_scan_then_filter_then_close` (Strings scan running, filter non-empty: three Esc presses cancel the scan, clear the filter, close the window); `pwn_window_sizes_match_table` (open each window kind in TestAppContext and assert default and minimum sizes).
- **Phase:** P1 (frame), P2 (pin working).
- **Decided here:** the sizes table above; the Run fixed-size exception; Esc order with scans and searches.

### UI-PWN-002 Properties data path

- **Implements:** PROC-077 ("the window registers demand for its ProcessKey and the active tab's data"), PROC-078 to PROC-085 data parts, R-097, R-098.
- **Anatomy:** no visible part. This entry fixes how Properties windows get data, as ARCHITECTURE ADR-003, ADR-005 and ADR-008 now define it (R-119).
- **Data binding:**
  - `Demand` gains `pub details: Arc<[DetailDemand]>`, max 16 entries (8 process windows plus 8 app windows):

```rust
pub struct DetailDemand { pub target: DetailTarget, pub tab: DetailTab }
pub enum DetailTarget { Process(ProcessKey), App(GroupKey) }
pub enum DetailTab { General, Threads, Fds, Maps, Security, Environment, Strings, AppGeneral, AppProcesses }
```

  - `Snapshot` gains `pub details: Arc<DetailsSnapshot>` with its own `ComponentHeader`, and `ComponentMask` gains the bit `DETAILS`. `DetailsSnapshot` holds `FxHashMap<DetailTarget, Arc<TargetDetail>>`; `TargetDetail` holds one `Option<Arc<TabPayload>>` per tab with its own `version: u64` and `validity: Validity`. Unchanged tabs reuse their `Arc`.
  - The `ProcessDetail` worker (ADR-004, `workers/process_detail.rs`) reads the demanded tab of each demanded target at `processes.interval_ms`, except: Memory maps reads `smaps` at most once per second (PROC-081); Environment reads once when the tab first opens and again only on Refresh (PROC-083); Strings never runs from demand (UI-PWN-017). The General tab always also loads (it feeds the title and the exited state).
  - New commands: `Command::DetailRefresh { target: DetailTarget, tab: DetailTab }` (Environment Refresh, F5), `Command::ComputeHashes(ProcessKey)` (sent once per window open; result in the General payload), `Command::VerifyPackage(ProcessKey)`, `Command::StringScan { scan: ScanId, key: ProcessKey, opts: StringScanOpts }`, `Command::CancelStringScan(ScanId)`.
  - Live numbers on the General tab (CPU, memory, I/O, GPU) come from `Snapshot.processes` (bit `PROCESSES`), not from the detail payload. The window block comes from `Snapshot.windows` (bit `WINDOWS`).
  - A window re-renders only when its mask bits change and, for `DETAILS`, only when its own target's tab version changed (checked by comparing the `Arc` pointer of its tab payload with the last one rendered).
- **Build:** `DetailDemand` and `DetailTab` in `tl-model/src/demand.rs`; `DetailsSnapshot` and `TabPayload` in `tl-model/src/details.rs`; each Properties window registers its demand with the `DemandAggregator` in its constructor and removes it in `on_release`. Tab switches update the entry.
- **Performance:** at most 16 demanded targets; payloads are replaced as whole `Arc`s per tab, so an unchanged tab costs one pointer compare per snapshot.
- **Tests:** `props_demand_registered_and_removed` (open and close a window, assert `Demand.details` contains then lacks the key); `props_tab_switch_updates_demand`; `props_ignores_other_targets_detail_change` (render counter unchanged when another key's payload changes).
- **Phase:** P2.
- **Decided here:** the whole data path above (types, bit name, worker cadence, commands).

---

## B. Process Properties window

### UI-PWN-003 Process Properties window

- **Implements:** PROC-077, PROC-071 (failure toast), PROC-056, R-050, R-073, D-008.
- **Anatomy:**

```text
+------------------------------------------------------------------------------+
| Process Properties - firefox - PID 4321                          [pin] [x]   | 34  (UI-PWN-001)
+------------------------------------------------------------------------------+
| General | Threads [97] | File descriptors [412] | Memory maps [210] | Security | Environment [51] | Strings |  32  (UI-PWN-004)
+------------------------------------------------------------------------------+
| [search] Filter                                          [x]  | tab tools    |  32  (UI-PWN-005)
+------------------------------------------------------------------------------+
| tab body (window_bg), padding sp5 (12) left/right, sp4 (8) top/bottom       |
|                                                                              |
+------------------------------------------------------------------------------+
  default 900 x 640, minimum 640 x 400
```

  - Default size 900 × 640 [spec PROC-077]; minimum 640 × 400 (UI-FND-021 Properties kind). Opened centred on the main window's output (`Bounds::centered(None, ..)`); position is never saved (ADR-010).
  - Title: `Process Properties - {name} - PID {pid}` where `{name}` is the PROC-007 display name of the process (not the app name). When the process has exited: `Process Properties - {name} - PID {pid} (not running)` [spec].
- **States:**
  - Loading (first detail payload not yet arrived, at most one sweep): title already set from the snapshot record; each tab shows its own loading state (UI-PWN-006 to UI-PWN-018).
  - Exited (`ProcessKey` no longer in `Snapshot.processes` and `ProcFlags::EXITED` seen, or the detail payload has validity `Missing`): title gains " (not running)"; every value keeps its last shown text (the window keeps the last `Arc<TargetDetail>` and the last `ProcessRecord` it rendered); every action control (Set Affinity link, Verify against package, Threads row actions, Strings Scan, Environment Refresh, Grant access buttons) is disabled with tooltip "The process is not running."; a 24 px banner appears under the tab strip: background `status.warning` at 15% alpha, 1 px bottom border `status.warning` at 40% alpha, Lucide `circle-off` 14 px `status.warning`, text `t_sm` `text_primary` "This process has exited. Values are frozen at the last update." [chosen]. The window does not close by itself. Demand for the key is removed.
  - Error opening (target exited between click and open, or `/proc/<pid>` cannot be opened): the window is not created; the main window shows the toast "Could not open properties." (Error type) [spec PROC-071].
  - Cap: a ninth process Properties window is not created; the source window shows the toast "Too many process properties windows are already open" (Warning type) [spec]. App Properties windows have their own cap of 8 (UI-PWN-019).
  - Already open for the same `ProcessKey`: the existing window is activated (`window.activate_window()`) and its active tab is not changed [spec].
- **Interaction:**
  - Opened by `OpenProperties` (Enter, Alt+Enter, Ctrl+I on a process row, D-008; the row menu item "Properties"; double-click is not bound, because TaskSlinger double-click expands). The action carries the captured `ProcessKey`.
  - Keys inside the window (context `PropsWindow`): tab keys per UI-PWN-004; Ctrl+F focuses the active tab's filter (`FocusFilter`, same action as table b); F5 refreshes the active tab now (`RefreshNow`, sends `Command::DetailRefresh`); Esc per UI-PWN-001; Ctrl+W closes.
  - Initial focus: the active tab's first focusable element (the table for table tabs, the filter for General and Security).
  - Toasts inside the window appear at the top centre of this window, 8 px under the tab strip (APP-044 rule applied per window).
- **Data binding:** `ProcessKey` captured at open. Reads `Snapshot.processes` (bit `PROCESSES`) for the record, `Snapshot.details` (bit `DETAILS`) for tab payloads, `Snapshot.windows` (bit `WINDOWS`) for the General window block. Registers `DetailDemand { target: Process(key), tab: active }` (UI-PWN-002). Open windows are tracked by the foundation's `WindowRegistry` under `WindowKey::Properties(ProcessKey)`; it enforces the cap of 8 and closes the window with the main window (UI-FND-021).
- **Build:** `crates/tl-ui/src/windows/props/mod.rs`, `pub struct PropsWindow` (Entity, root view of the window) owning: `key: ProcessKey`, `active: DetailTab`, `tabs: PropsTabs` (one lazily created entity per tab: `GeneralTab`, `ThreadsTab`, `FdsTab`, `MapsTab`, `SecurityTab`, `EnvironTab`, `StringsTab`), `filter: [Entity<InputState>; 7]` (one per tab, created in the constructor), `exited: bool`, `last_record: Option<ProcessRecord>`, `_subs: Vec<Subscription>`. Each tab body is embedded with `.cached(StyleRefinement::default().size_full())`, except table bodies (they own scrolling, ADR-009).
- **Performance:** a snapshot install costs one record lookup by `ProcessKey` (binary search in `records`, sorted by key) and one `Arc` pointer compare per visible tab. Only the active tab renders. No `format!` in render: titles and counts are cached `SharedString`s rebuilt only when their number changes.
- **Accessibility:** the window title names the process, so the window list and the taskbar tooltip identify it. Every tab is reachable by keyboard (Alt+1 to Alt+7 and Ctrl+Tab), because kit tabs are not focusable (widget research section 1).
- **Tests:** `props_open_focuses_existing_window` (open twice for one key, assert one window and that it became active); `props_ninth_window_shows_toast` (open 9, assert 8 windows and the toast text); `props_exited_freezes_values` (remove the key from the fake snapshot, assert title suffix, banner text, values unchanged, Scan button disabled); `props_open_failure_toast` (key missing at open, assert no window and toast "Could not open properties.").
- **Phase:** P2.
- **Decided here:** exited banner text and look; initial focus; double-click does not open Properties.

### UI-PWN-004 Properties tab strip with counts

- **Implements:** PROC-077 (tab labels and counts).
- **Anatomy:** the foundation's `TlTabs` (UI-FND-018): 32 px strip, equal-width cells (min 80), `t_sm` labels, active cell `window_bg` with a 2 px `accent` underline, inactive `tab_bg`, 1 px `divider` below, overflow scroll plus the "»" menu. This entry fixes only the labels and counts.

```text
| General | Threads [97] | File descriptors [412] | Memory maps [210] | Security | Environment [51] | Strings |
```

  - Labels in this order [spec]: "General", "Threads", "File descriptors", "Memory maps", "Security", "Environment", "Strings".
  - Count suffix: one space then `[n]`, with `n` formatted as a count (thin-space grouping, table d), for example `File descriptors [1 204]`. Counts shown [spec]: Threads (thread count), File descriptors (fd count after the "Hide unnamed handles" filter), Memory maps (backing-file nodes after "Shared libraries only"), Environment (variable count). General, Security and Strings show none.
  - A count appears only once its source has a value; before that the label has no suffix (no "[…]" placeholder) [chosen].
  - Threads uses `ProcessRecord.threads` (live). The other counts come from the last payload of that tab; a tab never loaded shows no count.
  - At the default 900 px width the seven labels fit at full length (about 610 px of text) [measured with the foundation's `t_sm` metrics, estimated].
- **States:** as UI-FND-018. No tab is ever disabled in this window: a tab without access shows its access state inside the body (UI-PWN-018) [chosen; UI-FND-018 allows a disabled Strings tab, which this window does not use].
- **Interaction:** as UI-FND-018 (click, Left and Right in the strip, Ctrl+Tab and Ctrl+Shift+Tab from anywhere in the window). Added here: Alt+1 to Alt+7 select tab 1 to 7 (`PropsSelectTab(n)`) and Ctrl+PgDn and Ctrl+PgUp step tabs [chosen].
- **Data binding:** the owner keeps `[(Option<u32>, SharedString); 7]` and rebuilds a label only when its count changes.
- **Build:** `TlTabs` with stable id `("props-tabs", window_id)`, owned by `PropsWindow` in `props/mod.rs`.
- **Performance:** seven labels, re-shaped only when a count changes.
- **Accessibility:** each cell has the tooltip `{label} (Alt+{n})`.
- **Tests:** `props_tab_counts_format` (counts 1204 and 51 render as "File descriptors [1 204]" and "Environment [51]"); `props_tab_no_count_before_load`; `props_alt_number_selects_tab`.
- **Phase:** P2.
- **Decided here:** no count placeholder; no disabled tabs; Alt+1..7 and Ctrl+PgUp/PgDn.

### UI-PWN-005 Per-tab filter bar and refresh behaviour

- **Implements:** PROC-077 (filter per tab, empty state, refresh at `processes.interval_ms`), PROC-084 (filter applies to Value).
- **Anatomy:**

```text
+------------------------------------------------------------------------------+
| [Search 14] Filter                                         [x] | tab tools    |  32 high, padding sp5 / sp3
+------------------------------------------------------------------------------+
```

  - Bar height 32, background `window_bg`, bottom border 1 px `divider`. Padding left and right `sp5` (12), top and bottom `sp2` (4).
  - Filter: `TlTextInput` sm (24 high, UI-FND-016), fixed width 280 [chosen; the animated `TlFilterBox` belongs to the main tab strip], prefix Lucide `search` `i_sm`, placeholder "Filter", cleanable, `clean_on_escape`. Look and states are the foundation's.
  - Tab tools sit right-aligned in the same bar, gap `sp4` (8): File descriptors: checkbox "Hide unnamed handles"; Memory maps: checkbox "Shared libraries only"; Environment: "Environment at process start" note and Refresh button; Strings: the scan controls (UI-PWN-017). General, Threads and Security have none.
- **States:**
  - Empty filter: all rows. Non-empty: rows whose searched text contains the filter, case-insensitive (Unicode simple lowercase), substring only, no globs.
  - Nothing matches: `TlEmptyState` (UI-FND-029) with the text "No properties match the current filter." [spec text] and its "Clear filter" button. For table tabs, the table keeps its header and shows this text in place of rows. The Threads tab uses its own text "No threads match the current filter." [spec].
  - Focused: `input_focus` and the accent border.
- **Interaction:**
  - Typing debounces 100 ms (same as the main filter, ADR-009), then refilters on the background executor (`cx.background_spawn`) for tables over 2,000 rows, synchronously otherwise. The result is an `Arc<[u32]>` of row indexes, applied only if its generation equals the latest (rule from ADR-009).
  - What the filter matches per tab: General and Security: label or value text of each key/value field (non-matching fields are hidden; a column with no visible field keeps its panel but is empty); Threads: TID, Name, State, Wait channel; File descriptors: FD, Type, Name; Memory maps: node Name and Path (a matching child keeps its parent; a matching parent shows all its children); Environment: Name and Value (masked values match only by name while masked); Strings: Value only [spec].
  - Esc in a focused non-empty filter clears it; a second Esc closes the window (UI-PWN-001).
  - Ctrl+F focuses the active tab's filter.
  - Each tab keeps its own filter text while the window is open; it is not saved.
- **Refresh behaviour** (one rule for all tabs):
  - General live numbers and Threads re-render on each process sweep (`processes.interval_ms`, default 1000 ms) when their values changed.
  - File descriptors, Security: re-read every sweep while the tab is active; the table updates in place and keeps selection by row id (FD number; capability name; group GID).
  - Memory maps: `maps` every sweep, `smaps` RSS at most 1 Hz [spec].
  - Environment: read once when the tab first opens and on Refresh or F5 only [spec].
  - Strings: never refreshes by itself [spec].
  - Hidden tabs are not refreshed (demand covers only the active tab, plus General).
  - Ctrl-pause (hold Ctrl, PROC-050) does not apply to secondary windows [chosen; ADR-009 limits it to the main window].
- **Data binding:** filter text is local state; filtering reads the tab's last payload. No `Command` is sent by filtering.
- **Build:** `props/filter.rs`, `pub struct PropsFilter` helper: owns the debounce `Task` and the generation counter; one per tab. The input is the foundation's `TlTextInput` over a kit `InputState` (read with `text()`, not `value()`, widget research section 7).
- **Performance:** lowercase haystacks are precomputed once per payload version (`Arc<[Box<str>]>`), so a keystroke does no allocation per row except the result index vector. Strings tab filtering of 1,000,000 rows runs off the UI thread and is chunked; the previous result stays visible until the new one arrives.
- **Accessibility:** the filter has the accessible name "Filter {tab label}". Placeholder text is not the only label: the Search icon has the tooltip "Filter".
- **Tests:** `props_filter_hides_general_fields` (filter "pid" leaves PID and PPID); `props_filter_empty_state_text`; `props_filter_masked_env_matches_name_only` (filter on part of a masked value finds nothing); `props_filter_stale_generation_ignored`.
- **Phase:** P2.
- **Decided here:** filter width 280; matched fields per tab; background filtering above 2,000 rows; no Ctrl-pause in secondary windows.

### UI-PWN-006 General tab layout

- **Implements:** PROC-078 (three key/value columns and their fields), R-050, R-096.
- **Anatomy:**

```text
body (scrolls vertically as one region; padding sp5 left/right, sp4 top/bottom)
+------------------------+  +------------------------+  +------------------------+
| Process:   firefox     |  | SHA-256:   3f9a0c...   |  | Disk read bytes: 18 MiB|   rows 26 high
| PID:       4321        |  | MD5:       a0dc71...   |  | ...                    |
| ...                    |  | ...                    |  | ...                    |
+------------------------+  +------------------------+  +------------------------+
 gap sp4 (8) between column panels
+------------------------------------------------------------------------------+
| Command line                                                     [Copy]      |   full-width block
| /usr/lib/firefox/firefox -contentproc -childID 12 -isForBrowser ... (wrapped) |
+------------------------------------------------------------------------------+
```

  - Three column panels of equal width when the body is at least 840 px wide; below 840 px the three panels stack in one column in the order 1, 2, 3 [chosen]. At the default 900 px window the body is 876 px, so three columns show.
  - Column panel: background `list_bg` [measured `rgb(27,31,33)`], radius `r_sm`, padding `sp4` (8) horizontal and `sp3` (6) vertical. Gap between panels `sp4` (8) [measured about 16 including padding].
  - Each column is a `TlKeyValueGrid` in the inline layout (UI-FND-025), with two Properties overrides passed by the owner: row height 26 [measured 25.8 in the TaskSlinger General tab], independent of `row_height_pct` but multiplied by the UI scale, and the label width rule below. The foundation's 28 px rows and 180 px label column would leave about 90 px for values in a 290 px column. The component takes `.row_height(..)` and `.label_width(..)` (UI-FND-025, R-127). Label: `t_base` regular, `text_secondary`, ending with ":" [measured], left-aligned, single line. Value: `t_base` regular, `text_primary`, left-aligned, single line, ellipsis at the end, tabular figures for numbers.
  - Label width per column: the widest shaped label in that column plus `sp5` (12), clamped to 96..150 px, computed once per UI scale and cached [chosen; TaskSlinger uses 126 px for all columns, measured, but tasklens labels are longer]. Label and value styles, validity words, truncation, tooltips and the click-copy flash are the foundation's.
  - Command line block: full width under the panels, `sp4` (8) above it, background `list_bg`, radius `r_sm`, padding `sp4`. Header line 26 high: label "Command line" `t_base` medium `text_secondary`, and a TlButton Ghost sm "Copy" with Lucide `copy` 12 px at the right. Body: the full command line (up to 32 KiB, PROC-078, R-008), arguments joined by single spaces, wrapped at any character, `t_sm` (12) monospace off (Noto Sans), `text_primary`, line height 18. It is not in column 1, so long command lines do not stretch a column [chosen; the spec lists it in column 1].
- **Fields, in order** (label text exactly as shown; value format; source). Missing values show the R-050 words.
  - **Column 1:**
    1. `Process:` display name (PROC-007).
    2. `PID:` count without grouping (`4321`).
    3. `Started:` `YYYY-MM-DD HH:MM:SS` local (table d).
    4. `PPID:` parent PID as a link (UI-PWN-007); `0` for PID 1 and kernel threads, shown as plain text "0 (none)".
    5. `User:` `oystein (1000)`; when euid or suid differ: `oystein (1000), euid root (0), suid root (0)`.
    6. `Priority:` `Nice 0, SCHED_OTHER, kernel prio 120`; realtime: `RT 50, SCHED_FIFO, kernel prio 49`.
    7. `Autogroup:` `autogroup-12 nice 0`; `Not supported` when `/proc/<pid>/autogroup` is absent.
    8. `Affinity:` `All (0-31)` or `0-7,16-23 (16 CPUs)`, plus ` (threads differ)`; a link that opens Set Affinity (UI-PWN-021).
    9. `Hardening:` summary of the enabled items in this order, comma separated: `PIE, NX stack, Full RELRO, Shadow stack, NoNewPrivs, Seccomp`; `None` when none apply.
    10. `PIE:` `Yes` or `No`.
    11. `NX stack:` `Yes` or `No (executable stack)`.
    12. `RELRO:` `Full`, `Partial` or `None`.
    13. `Shadow stack:` `Enabled`, `Disabled` or `Not supported` (no `x86_Thread_features` line).
    14. `ASLR:` `Full (2)`, `Partial (1)` or `Off (0)` from the system setting, plus `, disabled for this process` when the personality has `ADDR_NO_RANDOMIZE`.
    15. `NoNewPrivs:` `Yes` or `No`.
    16. `Seccomp:` `Disabled`, `Strict` or `Filter (3 filters)`.
    17. `Architecture:` `x86-64`, `x86 (32-bit)`, `AArch64` or `Unknown` from ELF `e_machine` and `EI_CLASS`.
    18. `Executable:` absolute path as a link (opens the file location, PROC-073); when the link ends " (deleted)": plain text `{path} (deleted)` in `status.warning`.
    19. `Location:` one PROC-078 category word (`Temp directory`, `User Downloads`, `User data`, `User profile`, `System`, `Optional software`, `Flatpak`, `AppImage`, `Wine prefix`, `Deleted`), or `Other`.
    20. `Package:` `firefox 131.0-1`; `Not packaged` when no package owns the file.
    21. `Description:` package `%DESC%`, else the desktop file `Comment`, else `Not reported`.
    22. `Working dir:` absolute path.
    23. `Root:` shown only when not `/`.
    24. `Cgroup:` the cgroup path relative to the cgroup root, starting with `/`.
    25. `Unit:` unit name as a link to the Services tab (Go to service, PROC-071); hidden when the process belongs to no unit.
    26. `App ID:` desktop app ID; hidden when none.
  - **Column 2:**
    1. `SHA-256:` and 2. `MD5:` (UI-PWN-008).
    3. `Package status:` (UI-PWN-008), with the Verify button row.
    4. `Packager:` `%PACKAGER%` text, hidden when not packaged.
    5. `Threads:` count. 6. `Handles:` fd count.
    7. `Memory:` RssAnon, size. 8. `Working set total:` VmRSS. 9. `Working set shared:` RssFile + RssShmem. 10. `PSS:` 11. `USS:` 12. `Swap:` 13. `Commit:` VmData + VmStk. 14. `Memory %:` RssAnon / MemTotal, percent (one decimal).
    15. `CPU %:` percent of the machine (one decimal). 16. `CPU time:` `h:mm:ss`.
    17. `I/O read bytes:` rchar. 18. `I/O write bytes:` wchar. 19. `I/O read ops:` syscr, count. 20. `I/O write ops:` syscw, count.
  - **Column 3:**
    1. `Disk read bytes:` 2. `Disk write bytes:` 3. `Cancelled write bytes:` sizes.
    4. `Disk read rate:` 5. `Disk write rate:` 6. `Disk total rate:` fixed `MiB/s` one decimal (table d, table form).
    7. `Net I/O:` `Receive 1.2 Mbps, send 0.3 Mbps` in the `units.network` unit [chosen wording].
    8. `GPU %:` percent. 9. `VRAM:` size. 10. `GPU memory shared:` size or `Not supported`. 11. `GPU engine:` engine name (`3D`, `Compute`, `Video decode`, `Video encode`) or `None`.
    12. Window block (UI-PWN-009).
- **States:**
  - Loading: before the first General payload, payload-backed fields show `Collecting…`; record-backed fields (PID, Threads, Memory and the other live numbers) show their values at once.
  - Restricted fields (exe, cwd, root, personality, io, smaps_rollup of other UIDs): `Restricted` in `text_secondary` with the tooltip "Needs administrator access. Click the shield to grant it." (before P3: "Needs administrator access.") [spec R-050].
  - Exited: as UI-PWN-003; all links render as plain text in `text_secondary`.
  - Filtered: UI-PWN-005. Hidden fields leave no gap.
- **Interaction:** UI-PWN-007 (copy, links, keyboard).
- **Data binding:** payload `TabPayload::General(Arc<GeneralDetail>)` for static fields and hashes; `ProcessRecord` for live numbers (bit `PROCESSES`); `Snapshot.windows` (bit `WINDOWS`) for the window block. Re-renders when the General payload `Arc` changes, when this key's record changed values (compare the record's value fields with the last rendered copy), or when this pid's window entries change.
- **Build:** `props/general.rs`, `pub struct GeneralTab` (Entity). Fields are built into `Arc<[KvField]>` per column (`KvField { id: FieldId, label: SharedString, value: SharedString, kind: KvKind, validity: Validity, full: Option<SharedString> }`, `KvKind::{Text, Link(LinkTarget), Hash, Status}`) only when a source changes; each value string is cached by `(FieldId, value bits, validity)` like `CellTextCache`. Rendered with the foundation's `TlKeyValueGrid` (one per column) inside one `v_flex().overflow_y_scroll()` (single scroll owner). The command line block is a separate cached child view `CmdLineBlock`, re-shaped only when the command line text changes.
- **Performance:** a sweep that changes only live numbers re-formats at most 25 values, writing into reused `String` buffers (table d formatters do not allocate when the buffer is large enough). The 32 KiB command line is shaped once per change, never per frame (cached view).
- **Accessibility:** values that are truncated show their full text in a tooltip after 500 ms. Values are selectable text (UI-FND-025).
- **Tests:** unit tests in `props/general.rs`: `fmt_priority_other_and_fifo`, `fmt_affinity_all_and_list_threads_differ`, `fmt_user_with_euid`, `fmt_aslr_personality`, `fmt_seccomp_filters`; render tests: `general_three_columns_at_900`, `general_single_column_below_840`, `general_restricted_word_and_tooltip`, `general_cmdline_not_reshaped_on_sweep` (shape counter stays the same across 10 sweeps with an unchanged command line).
- **Phase:** P2.
- **Decided here:** 26 px rows not scaled by row height; label width rule; 840 px breakpoint; command line as a full-width block; Autogroup and Hardening summary as separate rows; Net I/O wording; architecture and hardening value words.

### UI-PWN-007 General tab: copy on click, links and keyboard

- **Implements:** PROC-078 (click copies a value; PPID link; Executable link; Affinity opens Set Affinity), PROC-071, PROC-073, APP-044.
- **Anatomy:** a field row has a value hit area covering the value cell (from the label's right edge to the panel's right padding), 26 high.
  - Plain value hover: background `row_hover` on the value cell, radius `r_sm`, cursor arrow.
  - Link value: the foundation's link style (UI-FND-025: `status.info`, underline on hover, pointing-hand cursor). TaskSlinger draws links in the value colour with a permanent underline [measured]; the foundation's style wins.
  - Keyboard cursor: 1 px `accent` outline around the whole field row, radius `r_sm`, inset 1 px.
- **States:** default, hover (above), pressed (value background `row_selected` for 120 ms (`m_fast`), none with reduced motion), focused (keyboard cursor), disabled (exited: links plain text `text_secondary`, copying still works).
- **Interaction:**
  - Left click on a plain or hash value: copies the full value (for hashes the full hex string; for truncated values the untruncated text) with `cx.write_to_clipboard`; toast "Process property value copied." (Success, 5 s) [spec]. Clipboard failure: "Failed to copy to clipboard" (Error, 8 s).
  - Left click on a link: PPID dispatches `GoToProcess { key: parent_key }` (the PROC-071 entry point, toast as defined there, the main window is activated); Executable dispatches `OpenFileLocation { key }` (PROC-073, toast "Opening file location"); Unit dispatches `GoToService { unit }`; Affinity dispatches `OpenSetAffinity { targets: [key] }` (UI-PWN-021).
  - Right click on any field opens the foundation's value menu (UI-FND-025: "Copy value", "Copy row", "Copy section", where a section is one column panel) plus one item added here: "Copy all fields" (`CopyAllFields`, every visible field of all three columns as `Label: value` lines in column order, then `Command line: …`). Toasts: "Process property value copied." for Copy value; "Copied to clipboard" for the others.
  - Keyboard (context `PropsWindow > KvGrid`, the grid region is one focus stop with `tab_stop(true)`): Up and Down move the cursor within a column; Left and Right move to the same row index in the neighbouring column (clamped to its last row); Home and End go to the first and last field of the column; Enter activates a link, or copies a plain value; Ctrl+C copies the value; Shift+F10 or the Menu key opens the field menu. Tab leaves the grid to the Command line Copy button, then the Verify button.
  - Command line "Copy" button: copies the full command line; toast "Process property value copied.".
- **Data binding:** none beyond UI-PWN-006. Copy reads the cached full strings.
- **Build:** in `props/general.rs`: one `div().id(("kv", field_id))` per value with `.on_click`, `.on_mouse_down(MouseButton::Right, ..)` and `.cursor(CursorStyle::PointingHand)` only for links. The menu is a TlMenu (`PopupMenu`) built when it opens. Actions in `windows/actions.rs`.
- **Performance:** no per-frame work; hover uses GPUI `.hover(..)` styles.
- **Accessibility:** links have the tooltip naming the action: PPID "Go to parent process", Executable "Open file location", Unit "Go to service", Affinity "Set affinity…". Copying is announced by the toast text.
- **Tests:** `general_click_copies_full_hash` (clipboard holds 64 hex characters, toast text); `general_ppid_link_dispatches_goto` (action captured with the parent key); `general_keyboard_cursor_moves_between_columns`; `general_copy_all_fields_order`.
- **Phase:** P2.
- **Decided here:** field menu items; keyboard model of the grid; link look; pressed flash.

### UI-PWN-008 General tab: hashes and Verify against package

- **Implements:** PROC-078 (SHA-256, MD5, errors, Package status, Verify against package, Packager), R-096, PROC-074 (shared hash cache).
- **Anatomy:**

```text
| SHA-256:         3f9a0c41e2b77d09…            |
| MD5:             a0dc71c745d97b1e…            |
| Package status:  Not verified                 |
|                  [Verify against package]     |   button row, 26 high
| Packager:        Arch Linux <...>             |
```

  - Hash values: the first 16 lowercase hex digits plus "…" [chosen; TaskSlinger shows 16 uppercase digits, measured]. Tabular figures.
  - Verify button: TlButton Secondary sm (24 high), label "Verify against package", aligned with the value column, in its own 26 px row.
- **States:**
  - Hashing (hash not yet in the payload): value `Computing…` in `text_secondary` [chosen; the PERF-013 word `Collecting…` is kept for metrics].
  - Done: hex as above; tooltip shows the full hash.
  - Hash error: both values show `Unavailable` and the tooltip carries the spec text: "Failed to compute one or more hashes" (read error, with the errno text on a second line) or "Executable path unavailable" (no readable exe and no readable argv path) [spec].
  - Package status values [chosen wording]: `Not verified` (initial), `Verifying…`, `Matches package` (`status.success` text), `Does not match package` (`status.danger` text), `Not packaged` (no owner; button hidden), `No checksum in package database` (no mtree sha256 for the file; button disabled with that tooltip).
  - Verify button: default; hover `menu_hover`; pressed; focused (ring); loading while verifying (kit `.loading(true)`, label "Verifying…", inert); disabled when exited (tooltip "The process is not running."), when not packaged (hidden), or when the database has no checksum (tooltip as above).
- **Interaction:**
  - Hashes start automatically when the window opens: the window sends `Command::ComputeHashes(key)` once (UI-PWN-002). The engine reuses the PROC-074 cache keyed by (dev, inode, mtime), so a VirusTotal lookup and Properties share one computation.
  - Verify: click, Enter or Space on the focused button sends `Command::VerifyPackage(key)` (`VerifyPackage` action). The result lands in the General payload. Toast on finish: "Executable matches the package." (Success) or "Executable does not match the package." (Warning) [chosen]. Failure to read the package database: toast "Could not verify against the package." (Error) [chosen].
  - Clicking a hash copies the full hex (UI-PWN-007).
- **Data binding:** `GeneralDetail.hashes: HashState { Pending, Done { sha256: [u8; 32], md5: [u8; 16] }, Failed(HashError) }` and `GeneralDetail.package_check: PackageCheck`. Re-renders on General payload change.
- **Build:** part of `GeneralTab`; the hex strings are formatted once when `HashState` changes.
- **Performance:** hashing runs on the engine's blocking pool; the UI only formats 2 strings once.
- **Accessibility:** button accessible name equals its label; status colours always come with words, never colour alone.
- **Tests:** `hash_pending_then_done_text`; `hash_failed_tooltip_text`; `verify_button_hidden_when_not_packaged`; `verify_result_toasts`.
- **Phase:** P2.
- **Decided here:** 16 lowercase digits; `Computing…`; package status words; verify toasts.

### UI-PWN-009 General tab: window block

- **Implements:** PROC-078 (window block), PROC-052, PROC-053.
- **Anatomy:** the last rows of column 3, after `GPU engine:`, separated by an 8 px gap and a 1 px `divider` line inset `sp4` on both sides.

```text
| Window title:  Start page - Mozilla Firefox |
| Window ID:     {4b1c...}                      |
| Window state:  Normal                         |
| Window class:  firefox                        |
| Window rect:   L:0 T:32 R:1280 B:832          |
| Window size:   1280 x 800                     |
| And 2 more                                    |   plain text row, text_secondary, no label
```

  - Field labels and formats [spec]: `Window title:` caption; `Window ID:` KWin `internalId` as given (a braced UUID); `Window state:` `Minimized`, `Maximized` or `Normal`; `Window class:` `resourceClass`; `Window rect:` `L:%d T:%d R:%d B:%d` in logical px of the frame geometry; `Window size:` `%d x %d`.
  - Several windows: the block shows the first window in KWin stacking order (topmost first) and then the row "And N more" [spec text], where N is the count of the others.
- **States:**
  - No window: a single field `Window:` `No top-level window` [spec text].
  - Bridge missing (`Snapshot.windows` validity `Unsupported`): a single field `Window:` `Not supported`, tooltip "Needs the KDE window bridge." [chosen].
  - Bridge starting (`WarmingUp`): `Window:` `Collecting…`.
  - Exited: frozen as the rest.
- **Interaction:** values copy on click (UI-PWN-007). "And N more" has the tooltip listing up to 10 more captions, one per line [chosen]. Window title is not a link; Show Window stays in the row menu (PROC-054).
- **Data binding:** `Snapshot.windows` entries for this pid (bit `WINDOWS`), filtered by pid only (the window bridge reports pids, and the process key's pid is re-checked against the record).
- **Build:** part of `GeneralTab` column 3 fields.
- **Performance:** at most 7 strings re-formatted when this pid's window entries change; a lookup by pid in the windows component (sorted by pid) per WINDOWS change.
- **Tests:** `window_block_none_text`, `window_block_and_n_more`, `window_block_unsupported_without_bridge`, `window_rect_format`.
- **Phase:** P2 (with the bridge).
- **Decided here:** which window is shown first; "Needs the KDE window bridge." tooltip; "And N more" tooltip.

### UI-PWN-010 Threads tab

- **Implements:** PROC-079 (columns, priority words, states), R-097.
- **Anatomy:** filter bar (UI-PWN-005), then a `TlTreeTable` in flat mode, fixed column set (no picker), single-line header, filling the body. Body padding 0 (the table runs edge to edge under the filter bar) [chosen].

| Column | Header text | Width (min) | Align | Value format |
|---|---|---|---|---|
| tid | Thread ID | 80 (60) | right | integer, no grouping |
| name | Name | 160 (60) | left | thread `comm` |
| state | State | 96 (60) | left | `Running`, `Sleeping`, `Disk sleep`, `Stopped`, `Tracing stop`, `Zombie`, `Dead`, `Idle`, `Parked` |
| cpu | CPU % | 70 (60) | right | percent of the machine, one decimal |
| utime | User time (ms) | 104 (80) | right | count |
| stime | Kernel time (ms) | 112 (80) | right | count |
| prio | Priority | 100 (70) | left | `Variable`, `Realtime 50`, `Deadline` |
| base | Base priority | 100 (70) | right | nice (`-5`, `0`, `19`) or rt_priority |
| aff | Affinity | 110 (60) | left | `All` or a CPU list (`0-7,16-23`) |
| lastcpu | Last CPU | 72 (60) | right | integer |
| ctx | Ctx switches | 140 (100) | right | `1 204 / 33` (voluntary / involuntary, counts) |
| wchan | Wait channel | 160 (60) | left | symbol name, `0` shown as `-` |

  - Widths [chosen] use the canonical template-string minimums for numeric columns (table d) and 60 px for text columns. Column widths can be resized for the life of the window; they are not saved [chosen].
  - The main thread (TID = PID) is drawn with Name in medium 500 weight [chosen].
- **States:**
  - Loading: "Loading threads..." centred in the table body [spec].
  - Empty: "No threads found." [spec] (a zombie has no threads).
  - Filter empty: "No threads match the current filter." [spec].
  - Restricted cells (`wchan` of other UIDs): the ADR-003 lock glyph with the tooltip from R-050.
  - Priority tooltip names the policy: "SCHED_OTHER", "SCHED_BATCH", "SCHED_IDLE", "SCHED_FIFO", "SCHED_RR" or "SCHED_DEADLINE" [spec].
  - Selected, hover, focused rows: TlTreeTable states.
  - Exited: rows frozen, row menu actions disabled.
- **Interaction:**
  - Sorting: click a header (descending first, then ascending, as the process table). Default sort: Thread ID ascending [chosen]. Sort is done in the tab entity (small lists, at most a few thousand threads) and kept by `ColumnId` for the window's life.
  - Selection by TID survives refresh and re-sort.
  - Enter on a row does nothing [chosen]; Shift+F10 or Menu opens the row menu (UI-PWN-011); Ctrl+C copies the focused row as `Label: value` lines for all columns [chosen]; Ctrl+Shift+C copies the selected rows as TSV with a header line (table b).
- **Data binding:** `TabPayload::Threads(Arc<[ThreadRow]>)` (bit `DETAILS`) where `ThreadRow { tid, comm, state, cpu_pct, utime_ms, stime_ms, policy, rt_prio, nice, cpus: CpuListText, last_cpu, vol_ctx, invol_ctx, wchan, validity }`. The count in the tab label comes from `ProcessRecord.threads`.
- **Build:** `props/threads.rs`, `pub struct ThreadsTab` (Entity) owning the sorted index `Arc<[u32]>`, the filter result, the `CellTextCache`-style string cache keyed by `(tid, ColumnId)`, and a `TlTreeTable` entity.
- **Performance:** sorting 2,000 threads is under 0.2 ms; done only when the payload version or sort changes. Cell strings re-format only when value bits change.
- **Accessibility:** headers have tooltips: "User time (ms)": "Time spent in user mode, in milliseconds"; "Kernel time (ms)": "Time spent in the kernel, in milliseconds"; "Ctx switches": "Voluntary / involuntary context switches"; "Wait channel": "Kernel function the thread is sleeping in" [chosen].
- **Tests:** `threads_priority_words` (OTHER gives "Variable", FIFO 50 gives "Realtime 50", DEADLINE gives "Deadline"); `threads_selection_survives_resort`; `threads_loading_and_empty_texts`; `threads_ctx_format`.
- **Phase:** P2.
- **Decided here:** column widths; default sort; main-thread weight; Enter does nothing; header tooltips; Ctrl+C row copy format.

### UI-PWN-011 Threads row menu and thread actions

- **Implements:** PROC-079 (row menu), PROC-066, PROC-067, PROC-059, R-097.
- **Anatomy:** TlMenu (kit `PopupMenu`), item height 26, min width 220.

```text
Copy
Set thread priority          >   Realtime / High / Above normal / Normal / Below normal / Low / Idle / Custom…
Set thread affinity…
Send signal to thread        >   Terminate (TERM) / Kill (KILL) / User 1 (USR1) / User 2 (USR2)
```

  - The current priority class of the right-clicked thread has a check mark (as PROC-066); with several selected threads, a class is checked only when all share it (PROC-069 rule).
  - Signal items carry the secondary suffix "(ends process)" on TERM and KILL, and on USR1 and USR2 (their default action terminates) [chosen per PROC-065 rule].
- **States:** items disabled with tooltip "The process is not running." when exited. "Realtime" and "Custom…" policy items follow PROC-066 privilege rules (enabled; EPERM gives the toast).
- **Interaction:**
  - Right-click selects the row if it is not selected and captures `SmallVec<[ActionTarget; 1]>` of `ActionTarget::Thread { process: key, tid }` for every selected row (max 256), as the process table does.
  - Copy (`CopyCell`): copies the right-clicked cell's text; toast "Copied to clipboard".
  - Set thread priority items dispatch `SetThreadNice(n)` (`ActionKind::SetNice`) or `SetThreadSched` (`ActionKind::SetSched`); "Realtime" first opens the realtime confirmation (UI-PWN-024); "Custom…" opens the Custom priority dialog (UI-PWN-023) for the thread targets.
  - "Set thread affinity…" opens Set Affinity (UI-PWN-021) for the thread targets.
  - "Send signal to thread" items dispatch `SignalThread(Signal)` (`ActionKind::Signal` on thread targets, `tgkill` in the engine). The submenu has the tooltip "A signal that ends a thread ends the whole process." [spec meaning, chosen wording]. Confirmation follows PROC-059 (protected process, other user, or more than one target).
  - Toasts [chosen, following the PROC-058 forms]: "Thread priority updated", "Thread affinity updated", "Sent SIGUSR1 to thread 4325" (one thread) or "Sent SIGUSR1 to 3 threads"; failures use PROC-058 forms ("Permission denied" with "Retry as administrator" from P3; the partial form "Priority updated for 3 of 4 threads. 1 failed: permission denied").
- **Data binding:** sends `Command::Action(ActionRequest { kind, targets: Thread targets, privileged: false })`; results through `ActionTracker` on `action_events`.
- **Build:** menu built in `props/threads.rs` with `PopupMenu::build`; actions in `windows/actions.rs`.
- **Performance:** menu is built only on open.
- **Accessibility:** Shift+F10 and Menu open it on the focused row; arrow keys move; Esc closes and returns focus to the table.
- **Tests:** `thread_menu_captures_thread_targets`; `thread_signal_toast_text_single_and_multi`; `thread_menu_disabled_when_exited`.
- **Phase:** P2.
- **Decided here:** "(ends process)" on USR1 and USR2; toast wordings for thread actions; multi-thread targets.

### UI-PWN-012 File descriptors tab

- **Implements:** PROC-080 (columns, details pane, Hide unnamed handles, states, row menu).
- **Anatomy:**

```text
+------------------------------------------------------------------------------+
| [Filter................]                      [x] Hide unnamed handles      |  filter bar
+------------------------------------------------------------------------------+
| FD | Type      | Name                                    | Access     | Position |  TlTreeTable, flat
| 0  | file      | /dev/null                               | R          |        0 |
| 3  | TCP       | TCP 127.0.0.1:5000 → 10.0.0.2:443 ESTABLISHED | RW+NB CLOEXEC |   |
| ...                                                                          |
+============================================================================== +  4 px splitter
| Raw flags:         0o2104002                                                 |  details pane
| Access mode:       Read/write                                                |  default 132 high
| Status flags:      Append, Non-blocking                                      |
| Descriptor flags:  Close-on-exec                                             |
| Mount ID:          27        Inode:  1234567                                 |
+------------------------------------------------------------------------------+
```

  - Columns [chosen widths]: FD 64 (60) right; Type 96 (60) left; Name flex, min 240, left, ellipsis in the middle for paths [chosen: middle ellipsis keeps the file name]; Access 120 (80) left, text such as `RW +A +NB CLOEXEC`; Position 110 (80) right, count or `-` for sockets and pipes.
  - Type words [spec]: `file`, `directory`, `TCP`, `UDP`, `unix`, `netlink`, `pipe`, `FIFO`, `eventfd`, `eventpoll`, `timerfd`, `signalfd`, `inotify`, `fanotify`, `memfd`, `dmabuf`, `DRM`, `pidfd`, `io_uring`, `anon_inode`, `device`.
  - Hide unnamed handles: TlCheckbox, label "Hide unnamed handles", in the filter bar on the right [spec]; bound to the setting `properties.hide_unnamed_handles` (default true). Changing it writes the setting (500 ms debounced save, ADR-011) and applies to every open Properties window [chosen].
  - Details pane: under the table, split by a 4 px horizontal splitter (kit `v_resizable`, drag handle colour `divider`, hover `accent`). Default height 132 [chosen: 5 rows of 26 plus padding], minimum 80, maximum 50% of the body. Height is kept for the window's life, not saved. Contents: a TlKeyValueGrid with two columns: left `Raw flags:`, `Access mode:`, `Status flags:`, `Descriptor flags:`; right `Mount ID:`, `Inode:`, and for eventfd `Count:` (`eventfd-count`), for DRM `DRM client:` (`drm-client-id`), `DRM driver:` (`drm-driver`) [chosen extras from the fdinfo keys PROC-080 reads].
  - Decoded words [chosen]: access mode `Read only`, `Write only`, `Read/write`, `Path only`; status flags joined by ", " from `Append`, `Non-blocking`, `Sync`, `Direct`, `Path`, `Temp file`; descriptor flags `Close-on-exec` or `None`.
- **States:**
  - Loading: "Loading handles..." [spec].
  - Empty: "No handles reported." [spec].
  - Details pane without a selection: "Select a descriptor to see its flags." centred, `text_secondary` [chosen].
  - Details pane when fdinfo cannot be read: "Handle detail query unavailable." [spec].
  - No access: UI-PWN-018 with the text "Handles unavailable (insufficient privileges)" and the "Grant access" button [spec].
  - Exited: frozen; row menu disabled.
- **Interaction:**
  - Sort: header click; default FD ascending [chosen].
  - Selecting a row fills the details pane (keyboard and mouse).
  - Row menu (TlMenu): "Copy" (cell text; toast "Copied to clipboard"); "Open file location" (enabled for `file` and `directory` rows, `OpenPathLocation { path }`, toast "Opening file location"); "Go to connection" (enabled for `TCP`, `UDP` and `unix` rows; `GoToConnection { inode }`: activates the main window, switches to the Connections tab and selects the socket; toast "Connection not found in current view" when absent [chosen, following PROC-071 wording]).
  - Ctrl+C copies the selected rows as `FD<TAB>Type<TAB>Name<TAB>Access<TAB>Position` lines without header [chosen]; Ctrl+Shift+C adds the header line.
- **Data binding:** `TabPayload::Fds(Arc<FdDetail>)` with `rows: Arc<[FdRow]>` and per-row `fdinfo` fields; the count in the tab label is the visible row count after the hide filter.
- **Build:** `props/fds.rs`, `pub struct FdsTab` (Entity): a `TlTreeTable` and a `FdDetailsPane` cached child view, inside kit `v_resizable("fds-split")`.
- **Performance:** a process with 10,000 fds re-sorts only when the payload version changes; the pane re-renders only on selection or payload change.
- **Accessibility:** the splitter has the tooltip "Drag to resize"; the details pane is reachable by Tab after the table (it is a focus stop only for copy: Ctrl+C copies the pane as lines).
- **Tests:** `fds_hide_unnamed_filters_anon_inode`; `fds_details_decodes_flags` (0o2104002 gives "Read/write", "Non-blocking" and "Close-on-exec"; the large-file bit is not shown); `fds_go_to_connection_enabled_only_for_sockets`; `fds_privilege_state_text_and_button`.
- **Phase:** P2.
- **Decided here:** widths; middle ellipsis; details pane size, layout, extra DRM and eventfd fields, decoded words; setting applies to all windows; copy formats.

### UI-PWN-013 Memory maps tab

- **Implements:** PROC-081 (tree, Primary, Outside loaded modules, columns, Shared libraries only), R-098.
- **Anatomy:** filter bar with the TlCheckbox "Shared libraries only" on the right (setting `properties.shared_objects_only`, default true, applies to all windows) [spec]; with the box on, the Primary node and the "Outside loaded modules" node stay visible and only file-backed library mappings are filtered by the PROC-081 patterns (R-225); then a `TlTreeTable` in tree mode (expand chevrons and indentation from the processes spec).

| Column | Header text | Width (min) | Align | Node row | Region row |
|---|---|---|---|---|---|
| name | Name | 260 (120) | left, tree | file name, `firefox (Primary)`, or `Outside loaded modules` | `7f3a2c000000-7f3a2c021000` |
| base | Base address | 140 (110) | right | lowest start, `0x7f3a2c000000` | start |
| size | Size | 90 (70) | right | sum of region sizes | region size |
| rss | RSS | 90 (70) | right | sum of region RSS | region RSS |
| perms | Perms | 64 (60) | left | empty | `r-xp` |
| offset | Offset | 100 (80) | right | empty | `0x0001a000` |
| path | Path | 280 (60) | left, middle ellipsis | full path | empty |
| desc | Description | 200 (60) | left | package `%DESC%` | empty |
| pkg | Package | 140 (60) | left | `glibc 2.42-1` | empty |

  - Order [spec plus chosen]: the "(Primary)" node first, "Outside loaded modules" last, the others sorted by the active sort (default Base address ascending). Region children are always in address order.
  - Addresses use lowercase hex with `0x` and no padding on node rows [chosen]. Sizes use table (d) memory sizes.
  - "Outside loaded modules" children: `[heap]`, `[stack]`, `[vdso]`, `[vvar]`, `anonymous`, `memfd:<name>` rows [spec], drawn with the region format plus the name in the Name cell (`[heap] 55d3f1a00000-55d3f1c21000`).
  - All nodes start collapsed [chosen]. Expansion state is kept per node path for the window's life.
- **States:** Loading "Loading memory maps..." [chosen]; empty (kernel thread) "No memory maps." [chosen]; filter empty: shared text (UI-PWN-005); no access: "Needs administrator access." with the Grant access button (UI-PWN-018) [spec text]; RSS column before the first `smaps` read: `…` glyph (WarmingUp).
- **Interaction:** Right and Left expand and collapse, Space toggles (table b). Row menu: "Copy" (cell); "Copy path" (node rows); "Open file location" (node rows with a path, `OpenPathLocation`). Toasts as UI-PWN-012.
- **Data binding:** `TabPayload::Maps(Arc<MapsDetail>)` with `nodes: Arc<[MapNode]>` and `regions: Arc<[MapRegion]>`; RSS from `smaps` at most 1 Hz [spec]. The label count is the visible node count.
- **Build:** `props/maps.rs`, `pub struct MapsTab` (Entity) builds the flattened visible row list (`Arc<[TreeRowRef]>`) from nodes, expansion and filter, on payload, expansion, sort or filter change only.
- **Performance:** a large process has about 1,000 mappings; flattening is linear and runs only on the changes above.
- **Accessibility:** "(Primary)" has the tooltip "Main executable"; "Outside loaded modules" has the tooltip "Anonymous, heap, stack and other mappings without a file" [chosen].
- **Tests:** `maps_primary_first_outside_last`; `maps_shared_only_filter_includes_wine_dll`; `maps_rss_warming_up_glyph`; `maps_expand_state_survives_refresh`.
- **Phase:** P2.
- **Decided here:** widths; node versus region cell contents; address format; collapsed default; loading and empty texts; tooltips.

### UI-PWN-014 Security tab: summary

- **Implements:** PROC-082 (summary rows, error, copy toast), R-050.
- **Anatomy:** the Security body is a vertical stack with three regions [chosen, following TaskSlinger's Token tab, measured]: the summary (natural height, at most 45% of the body; scrolls inside when taller), then the Capabilities section, then the Groups section (UI-PWN-015). The two tables share the rest at 60% and 40%.

```text
| User:               oystein (1000)              | Elevated:        No                      |
| Group:              oystein (1000)              | User namespace:  Shared with init        |
| Groups:             12                          | PID namespace:   Isolated (4026532211)   |
| Risky capabilities: None reported               | ...                                      |
| ...                                             |                                          |
```

  - Two key/value columns side by side, no panel background [measured: TaskSlinger's Token summary has none], row height 26, label width rule of UI-PWN-006. Padding `sp5` left and right, `sp4` top.
  - **Left column** (label: value format) [spec content, chosen labels]:
    1. `User:` `oystein (1000)` when real, effective, saved and fs UID are equal; otherwise `real oystein (1000), effective root (0), saved root (0), fs root (0)`.
    2. `Group:` the same for GIDs.
    3. `Groups:` count of supplementary groups.
    4. `Risky capabilities:` comma list of the PROC-082 risky names present in effective or ambient, without the `CAP_` prefix (`SYS_ADMIN, NET_ADMIN`), in `status.warning`; `None reported` otherwise.
    5. `Admin groups:` comma list from the PROC-082 set, each root-equivalent one followed by ` (root-equivalent)` (`wheel, docker (root-equivalent)`); `None reported` otherwise.
    6. `Flags:` comma list of the set flags: `NoNewPrivs`, `Seccomp strict` or `Seccomp filter (3 filters)`, `Dumpable`; `None reported` when none.
    7. `Security label:` `attr/current` text (`unconfined`), or `Not supported` when no LSM exposes it.
    8. `Session:` logind session ID (`2`) or `None`.
    9. `Audit session:` `sessionid`, or `None` for 4294967295.
    10. `Login UID:` `oystein (1000)`, or `None` for 4294967295.
  - **Right column:**
    1. `Elevated:` `Yes (root)`, `Yes (capabilities)`, `Setuid` or `No` [spec].
    2. to 9. `User namespace:`, `PID namespace:`, `Network namespace:`, `Mount namespace:`, `IPC namespace:`, `UTS namespace:`, `Cgroup namespace:`, `Time namespace:` each `Shared with init`, `Isolated (4026532211)` or, without PID 1 access, `Same as tasklens` / `Isolated (inode)` compared with `/proc/self/ns` [spec].
    10. `Store bypass:` the `Speculation_Store_Bypass` text as the kernel gives it (`thread vulnerable`).
    11. `Indirect branch:` the `SpeculationIndirectBranch` text.
- **States:** loading: values `Collecting…`; `ns/*` of other UIDs: `Restricted` (R-050 tooltip); whole tab unreadable: the access state of UI-PWN-018 with the text "Security information unavailable (insufficient privileges)" [spec]; exited: frozen.
- **Interaction:** click on a value copies it, toast "Process property value copied."; if the clipboard write fails, the toast is "Could not copy token summary value." [spec]. Field menu and keyboard as UI-PWN-007.
- **Data binding:** `TabPayload::Security(Arc<SecurityDetail>)`; refreshed every sweep while active.
- **Build:** `props/security.rs`, `pub struct SecurityTab` (Entity) with two TlKeyValueGrids in an `h_flex` inside a `v_flex` with `max_h(relative(0.45))` and `overflow_y_scroll`.
- **Performance:** about 21 strings, rebuilt only on payload change.
- **Accessibility:** "Risky capabilities" values have the tooltip "Capabilities that allow taking over the system"; namespaces have the tooltip "Compared with PID 1" or "Compared with tasklens (PID 1 not readable)" [chosen].
- **Tests:** `security_user_line_equal_and_mixed`; `security_admin_groups_root_equivalent`; `security_ns_same_as_tasklens_without_helper`; `security_copy_failure_toast_text`.
- **Phase:** P2.
- **Decided here:** summary region size and layout; labels; split of session fields; namespace rows; flags wording.

### UI-PWN-015 Security tab: Capabilities and Groups tables

- **Implements:** PROC-082 (tables, "None reported").
- **Anatomy:**

```text
| Capabilities [3]                                                         |  section header 24 high
| Name           | Effective | Permitted | Inheritable | Bounding | Ambient | Description        |
| CAP_NET_RAW    |     ✓     |     ✓     |             |    ✓     |         | Use RAW and PACKET sockets |
| Groups [12]                                                              |  section header 24 high
| Name           | GID     | Kind                                         |
| oystein        |    1000 | Primary                                      |
| docker         |     968 | Supplementary (root-equivalent)              |
```

  - Section header: height 24 [chosen; measured about 20 in TaskSlinger at a smaller font], background `row_hover` [measured], padding left `sp5`, text `t_base` medium `text_secondary`: `Capabilities [n]` and `Groups [n]` (count formatting of table d).
  - Capabilities `TlTreeTable` flat: Name 180 (120) left, the full `CAP_` name; Effective, Permitted, Inheritable, Bounding, Ambient 84 (64) each, centred, Lucide `check` 14 px `text_primary` when set, empty when not; Description flex (min 160) left, the static `linux/capability.h` description.
  - Rows: every capability set in at least one of Effective, Permitted, Inheritable or Ambient [chosen; Bounding alone would list all 41 for most processes]. Default order: rows with Effective set first, then by name [chosen]. Risky capabilities (the PROC-082 list) show the Name in `status.warning` with the tooltip "Risky capability".
  - Groups `TlTreeTable` flat: Name 200 (80), GID 90 (60) right, Kind flex (min 120): `Primary`, `Supplementary`, plus ` (root-equivalent)` for docker, libvirt, lxd and disk, and ` (admin)` for wheel, sudo, root and adm [chosen marking].
- **States:** empty table: "None reported" centred in the table body, `text_secondary` [spec]; loading: "Collecting…"; exited: frozen.
- **Interaction:** sortable headers; row menu "Copy" (cell) and "Copy row" (TSV); Ctrl+C copies the selected rows as TSV; toasts "Copied to clipboard". The tab filter (UI-PWN-005) applies to Name and Description (capabilities) and Name and GID (groups).
- **Data binding:** `SecurityDetail.caps: Arc<[CapRow]>`, `SecurityDetail.groups: Arc<[GroupMembershipRow]>`.
- **Build:** two `TlTreeTable` entities in `props/security.rs`; each owns its own vertical scroll (allowed: they are separate regions with their own headers).
- **Performance:** at most 41 and a few dozen rows.
- **Accessibility:** check-mark cells carry the text alternative "Yes" (used by copy and by the tooltip on hover) [chosen].
- **Tests:** `caps_rows_exclude_bounding_only`; `caps_risky_name_colour`; `groups_kind_marks`; `security_tables_none_reported`.
- **Phase:** P2.
- **Decided here:** which capabilities are rows; order; group kind marks; section header size; check-mark text alternative.

### UI-PWN-016 Environment tab

- **Implements:** PROC-083, R-099, ADR-015.
- **Anatomy:**

```text
| [Filter.............] [x]        (i) Environment at process start   [Refresh] |  filter bar
| Name                      | Value                                              |
| DISPLAY                   | :1                                                 |
| GITHUB_TOKEN              | ••••••                                   [eye]     |
```

  - Filter bar tools (right side): Lucide `info` 14 px `text_secondary` plus the note "Environment at process start" `t_sm` `text_secondary` [spec text]; then TlButton Ghost sm "Refresh" with Lucide `refresh-cw` 12 px.
  - `TlTreeTable` flat: Name 240 (80) left; Value flex (min 160) left, single line, ellipsis at the end. Default sort Name ascending [chosen; TaskSlinger sorts by Name, measured "Name ↑"].
  - Masked value: "••••••" (six U+2022) in `text_secondary` [spec]. On hover or keyboard focus of a masked row, an eye button (TlButton Icon sm (24 × 24), Lucide `eye` 14 px) appears at the right end of the Value cell; on a revealed row it shows Lucide `eye-off` and stays visible.
  - When the environment was cut at 1 MiB, a 24 px footer under the table reads "Showing the first 1 MiB of the environment." in `text_secondary` [chosen].
- **States:**
  - Loading: "Loading environment..." [chosen].
  - Masking off (`properties.env_mask = false`): no masks and no eye buttons.
  - Revealed row: the real value in `text_primary`. Reveal lasts until the row is hidden again, Refresh is pressed, or the window closes [chosen]; it is never saved.
  - No access: UI-PWN-018 with "Environment unavailable (insufficient privileges)" [spec].
  - Zombie or kernel thread: "Environment block unavailable" centred [spec].
  - Read error: one table row with Name "(read failed)" in `text_secondary` and an empty Value; its tooltip carries the errno text [spec string, chosen placement].
  - Exited: frozen; Refresh disabled.
- **Interaction:**
  - Reveal: click the eye, or Space on the focused masked row (`ToggleRevealEnv`) [chosen; Space has no expand meaning in this flat table], or the row menu item "Reveal value" / "Hide value".
  - Row menu: "Copy name", "Copy value", "Copy all as NAME=value", separator, "Reveal value" or "Hide value" (masked rows only). "Copy all" copies every row that passes the filter, in table order [chosen]. A masked value is copied as "••••••" unless its row is revealed [spec R-099]. Toast "Copied to clipboard" for each.
  - Ctrl+C copies the selected rows as `NAME=value` lines with the same masking rule.
  - Refresh (button, F5): sends `Command::DetailRefresh { tab: Environment }`; re-masks all rows; the button shows loading until the new payload arrives (at most one sweep).
- **Data binding:** `TabPayload::Environ(Arc<EnvDetail>)` with `vars: Arc<[(Box<str>, Box<str>)]>`, `truncated: bool`. The mask decision is computed once per payload in the UI (`masked: Arc<[bool]>`) from the R-099 patterns, case-insensitive.
- **Build:** `props/environ.rs`, `pub struct EnvironTab` (Entity) with `revealed: FxHashSet<u32>` (row indexes of the current payload). Values are never logged and never placed in tooltips while masked [chosen].
- **Performance:** a 1 MiB environment has about 10,000 variables at most; mask and lowercase haystacks are computed once per payload.
- **Accessibility:** the eye button's accessible name is "Reveal value" or "Hide value"; masked cells have the tooltip "Hidden because the name looks like a secret".
- **Tests:** `env_masks_patterns_case_insensitive` (`api_key`, `GH_TOKEN`, `MyPassword`, `SESSION_COOKIE` masked; `PATH` not); `env_copy_value_masked_unless_revealed`; `env_refresh_remasks`; `env_mask_off_setting`; `env_read_failed_row`.
- **Phase:** P2.
- **Decided here:** reveal controls and lifetime; loading text; truncation footer; read-error placement; Copy all scope; no secrets in tooltips.

### UI-PWN-017 Strings tab

- **Implements:** PROC-084 (scan, cancel, options, progress, columns, limits, errors, access), R-004.
- **Anatomy:**

```text
| [Filter.....................] [x]                                            |  filter bar (filters Value)
| Minimum length [ 4 ^v]  [x] ASCII [x] UTF-8 [x] UTF-16LE  [ ] Include file-backed mappings   [Scan] |  options bar 32
| [==========------------]  Scanning process memory... 42%  18 234 strings found                |  progress row 24
| Address          | Encoding | Length | Value                                                  |  TlTreeTable flat
| 0x55d3f1a0c2f0   | ASCII    |     14 | /usr/lib/firefox                                       |
```

  - Options bar: height 32, padding `sp5` horizontal, gap `sp4`. "Minimum length" label `t_sm` `text_secondary`, TlStepper width 72, range 4..=64, step 1, default 4 [spec]. Three TlCheckboxes "ASCII", "UTF-8", "UTF-16LE", all on [spec]. TlCheckbox "Include file-backed mappings", off [spec]. Right-aligned: TlButton Primary sm "Scan" (Lucide `scan-search` 14 px), replaced by TlButton Secondary sm "Cancel" (Lucide `square` 12 px) while scanning.
  - Progress row (visible while scanning): `TlProgress` md (6 high, UI-FND-023), width 200, determinate; then the status text `t_sm` `text_secondary`, tabular figures. After the scan the bar is removed from the tree and only the status text stays.
  - Table columns: Address 140 (110) right, `0x` plus lowercase hex; Encoding 90 (70) left; Length 70 (60) right, count; Value flex (min 200) left, single line, first 512 characters with an ellipsis after [chosen]; tabs and newlines inside a value show as `\t` and `\n` [chosen].
- **States:**
  - Before the first scan: body text "Press Scan to search this process's memory for text." centred, `text_secondary` [chosen]. The scan never starts by itself [spec].
  - Scanning: options disabled; Scan becomes Cancel; status "Scanning process memory... 42%  18 234 strings found" [spec text; counts use the thin-space grouping of table d]; rows appear in batches; sorting is off and header clicks show the tooltip "Sorting is available after the scan" [chosen]; rows stay in address order.
  - Finished: status "Scan finished. 18 234 strings found." [chosen]; progress bar hidden; sorting on.
  - Cancelled: status "Scan cancelled. 18 234 strings found." [chosen]; results kept.
  - Limit reached (1,000,000 strings or 256 MiB of text): the scan stops; a 24 px banner above the table, `status.warning` at 15% alpha, text "String scan stopped: not enough memory." [spec]; results kept.
  - Errors: "Failed to read process memory." (body text when nothing was read, or a Warning toast when a partial result exists) [spec text]; kernel threads: "Strings unavailable for this process." body text, options and Scan disabled [spec].
  - No access: UI-PWN-018 with "Strings unavailable: needs administrator rights" and the button "Enable" [spec].
  - No encoding checked: Scan disabled, tooltip "Choose at least one encoding" [chosen].
  - Exited: Scan disabled; a running scan ends with the status "Scan stopped: the process exited. 18 234 strings found." [chosen].
- **Interaction:**
  - Scan (`StartStringScan`): click, or Enter while focus is in the options bar. Sends `Command::StringScan { scan, key, opts }` with a new `ScanId`; results from an older `ScanId` are dropped. A new scan clears the previous results.
  - Cancel (`CancelStringScan`): click, or Esc while a scan runs and focus is in the tab (Esc cancels before it closes the window) [chosen]. Sends `Command::CancelStringScan(scan)`.
  - Switching tabs keeps the scan running; closing the window cancels it.
  - Row menu: "Copy value" (full value, not the 512-character cut), "Copy address", "Copy row" (TSV). Ctrl+C copies the selected values, one per line. Toast "Copied to clipboard".
  - Filter (UI-PWN-005) matches Value only, off the UI thread, chunked.
- **Data binding:** `TabPayload::Strings(Arc<StringScanState>)` with `scan: ScanId`, `progress_pct: u8`, `found: u32`, `chunks: Arc<[Arc<[StringHit]>]>` (append-only chunk list, each new payload reuses the old chunk `Arc`s), `state: Scanning | Finished | Cancelled | Stopped(Limit) | Failed(Error)`. Payloads arrive at most 4 times per second while scanning [chosen].
- **Build:** `props/strings.rs`, `pub struct StringsTab` (Entity). The table reads rows through a chunk index (`row → (chunk, offset)`) so it never copies the hit list. Sorting after the scan runs on the background executor and produces an `Arc<[u32]>` permutation.
- **Performance:** 1,000,000 rows are virtualized by `uniform_list` (only visible rows are formatted); progress updates at most 4 Hz re-render only the progress row and the table's row count. `TlProgress` jumps in 1% buckets without a transition (UI-FND-023), so a 4 Hz update requests no extra frames.
- **Accessibility:** the progress text is the text alternative of the bar; Cancel has the tooltip "Stop the scan (Esc)".
- **Tests:** `strings_never_starts_by_itself`; `strings_progress_text_format`; `strings_stale_scan_results_dropped`; `strings_limit_banner_text`; `strings_kernel_thread_disabled`; `strings_chunks_not_copied` (pointer equality of old chunks across payloads).
- **Phase:** P3.
- **Decided here:** layout of the options and progress rows; empty, finished, cancelled and exited texts; Value cut at 512 characters and escapes; Esc cancels first; 4 Hz batches; custom progress element.

### UI-PWN-018 Tab access states (Restricted, Grant access, Enable)

- **Implements:** PROC-080, PROC-081, PROC-082, PROC-083, PROC-084 access texts; R-050; APP-003 (shield unlock); PROC-076.
- **Anatomy:** replaces the tab body under the filter bar:

```text
                 [ShieldAlert 24]
     Handles unavailable (insufficient privileges)
                  [Grant access]
```

  - Centred block, 64 px from the top of the body. Icon Lucide `shield-alert` 24 px `text_secondary`; `sp4` gap; message `t_base` `text_primary`, centred, max width 420, wraps; `sp5` gap; TlButton Secondary sm with the label from the spec ("Grant access" for File descriptors, Memory maps and Environment; "Enable" for Strings) [spec]; Security has no button (its data comes from world-readable files, so only hidepid causes it) [chosen].
  - Texts per tab [spec]: File descriptors "Handles unavailable (insufficient privileges)"; Memory maps "Needs administrator access."; Security "Security information unavailable (insufficient privileges)"; Environment "Environment unavailable (insufficient privileges)"; Strings "Strings unavailable: needs administrator rights".
- **States:**
  - Before P3 (no helper): the button is not shown [chosen, following R-050's "before P3" tooltip rule].
  - Helper not installed: button disabled, tooltip "Needs administrator helper (not installed)" [spec wording from PROC-068].
  - Waiting for authentication: button loading with label "Waiting for authentication…" [chosen].
  - Authentication cancelled: button back to default; toast "Authentication cancelled" [spec APP-044].
  - Granted: the tab reloads through the helper; the block is replaced by the normal loading state.
- **Interaction:** the button dispatches `StartHelperSession` (the same Action as the title-bar shield, APP-003, UI-FND-028), then, when `HealthSnapshot.helper` changes to `Unlocked`, every open Properties window whose General payload is `Restricted` sends `Command::DetailRefresh { target, tab }` once for its active tab; there is no flag on `DetailDemand` (R-226). Enter or Space on the focused button activates it. Tab order: the button is the first focus stop of the body.
- **Data binding:** tab payload validity `Restricted` triggers this state; helper session state from `Snapshot.health` (bit `HEALTH`).
- **Build:** `props/mod.rs`, `fn access_block(text, button: Option<AccessButton>) -> impl IntoElement` (RenderOnce).
- **Performance:** static.
- **Accessibility:** the icon is decorative; the text carries the meaning.
- **Tests:** `access_block_texts_per_tab`; `access_button_hidden_before_p3` (feature flag off); `access_cancelled_toast`.
- **Phase:** P2 (texts); P3 (buttons).
- **Decided here:** look; Security without button; buttons hidden before P3; waiting label.

---

## C. App Properties window

### UI-PWN-019 App Properties window and General tab

- **Implements:** PROC-085 (window, General tab, live Swap), PROC-077 (frame rules reused), D-008.
- **Anatomy:**

```text
+----------------------------------------------------------------------+
| App Properties - Firefox                                  [pin] [x]  | 32
| General | Processes [34]                                             | 36, two equal tabs
| [Filter........] [x]                                                 | 32
| [icon 48]  Firefox                                                   | 64 header block
|            App                                                       |
| +-- identity panel ----------------+  +-- usage panel --------------+|
| | App ID:        org.mozilla.firefox|  | CPU:      3.4%              ||
| | Type:          App                |  | Memory:   1.22 GiB          ||
| | Desktop file:  /usr/share/...     |  | Swap:     0 B               ||
| | Unit:          app-firefox@...    |  | Disk:     Read 0.4 MiB/s,...||
| | Cgroup:        /user.slice/...    |  | Network:  Receive 1.2 Mbps..||
| | Processes:     34                 |  | GPU:      5.0%              ||
| +-----------------------------------+  | VRAM:     812.00 MiB        ||
|                                         +---------------------------+|
| Command line (primary process)                               [Copy] |
| /usr/lib/firefox/firefox                                            |
+----------------------------------------------------------------------+
  default 720 x 560, minimum 640 x 400
```

  - Title `App Properties - {app name}` [spec]; for unit rows `App Properties - {unit name}`. Exited: ` (not running)` appended.
  - Default size 720 × 560 [chosen], minimum 640 × 400 (UI-FND-021 Properties kind). Frame per UI-PWN-001; tab strip per UI-PWN-004 with two tabs; filter bar per UI-PWN-005 (matches labels and values on General, Name, PID, User and Command line on Processes).
  - Header block: height 64, padding `sp5`. Icon 48 × 48 (`img(path)` from the engine's icon resolution, ADR-016; fallback Lucide `app-window` 48 px `text_secondary`) [spec size]. Name `t_lg` (16) semibold 600 `text_primary`; under it the type word `t_sm` `text_secondary`.
  - Two panels as UI-PWN-006 (identity left, usage right) when the body is at least 640 px wide, stacked below that [chosen].
  - Identity fields: `App ID:`; `Type:` `App` or `Unit` [spec]; `Desktop file:` path as a link (Open file location); `Unit:` link to the Services tab (Go to service), hidden when none; `Cgroup:` path; `Processes:` count.
  - Usage fields, live: `CPU:` percent; `Memory:` cgroup `anon` size [spec]; `Swap:` `memory.swap.current` size, updated live [spec]; `Disk:` `Read 0.4 MiB/s, write 0.1 MiB/s`; `Network:` `Receive 1.2 Mbps, send 0.3 Mbps`; `GPU:` percent; `VRAM:` size [chosen wordings as UI-PWN-006].
  - Command line block: as UI-PWN-006 with the header "Command line (primary process)" [chosen label].
- **States:** loading: payload fields `Collecting…`; group gone from the snapshot: title suffix, banner "This app has no running processes. Values are frozen at the last update." [chosen], links disabled; restricted values: R-050 words.
- **Interaction:**
  - Opened by `OpenProperties` on an app or unit row (Enter, Alt+Enter, Ctrl+I, row menu "Properties") [spec, D-008]. Opening again for the same group focuses the existing window. A ninth App Properties window gives the toast "Too many app properties windows are already open" (Warning) [chosen, mirrors PROC-077].
  - Keys as UI-PWN-003 (Alt+1 and Alt+2 select the tabs). Copy on click, field menu and grid keyboard as UI-PWN-007.
- **Data binding:** `DetailDemand { target: App(group_key), tab: AppGeneral | AppProcesses }`; payload `TabPayload::AppGeneral(Arc<AppGeneralDetail>)` (identity fields and cgroup `memory.stat` anon, `memory.swap.current`, `io.stat` rates); `ProcessTableView.groups` (bit `TABLE`) for CPU, GPU and network sums of the group. Unit rows use the same `GroupKey` the table gives them.
- **Build:** `crates/tl-ui/src/windows/app_props.rs`, `pub struct AppPropsWindow` (Entity) with `AppGeneralTab` and `AppProcessesTab` children; registered in `WindowRegistry` under `WindowKey::AppProperties(GroupKey)`.
- **Performance:** one group lookup per snapshot; at most 7 live strings re-formatted per sweep.
- **Accessibility:** the icon has the text alternative "{app name} icon".
- **Tests:** `app_props_title_for_app_and_unit`; `app_props_swap_updates_live` (two payloads with different swap, assert the text changes); `app_props_ninth_window_toast`; `app_props_two_panels_breakpoint`.
- **Phase:** P2.
- **Decided here:** size; header block; panel split; field wordings; cap toast; banner text.

### UI-PWN-020 App Properties: Processes tab

- **Implements:** PROC-085 (Processes [n] mini table; double-click or Open properties).
- **Anatomy:** `TlTreeTable` flat under the filter bar:

| Column | Header | Width (min) | Align | Value |
|---|---|---|---|---|
| name | Name | 220 (120) | left | 16 px process icon, gap `sp3`, display name |
| pid | PID | 80 (60) | right | integer |
| cpu | CPU | 70 (60) | right | percent |
| mem | Memory | 100 (70) | right | RssAnon size (D-004) |
| disk | Disk | 90 (70) | right | read + write `MiB/s` |
| user | User | 110 (60) | left | user name |
| cmd | Command line | flex (160) | left | first 1 KiB of the command line, ellipsis |

  Tab label count `[n]` is the member process count.
- **States:** loading "Loading processes..." [chosen]; empty "No processes." [chosen]; filter empty: shared text; exited processes leave the list at the next sweep (no exit highlight here) [chosen].
- **Interaction:** default sort PID ascending [chosen]; Enter, Alt+Enter, Ctrl+I and double-click on a row open that process's Properties (UI-PWN-003) [spec]; row menu: "Open properties", "Go to process" (PROC-071 entry point, activates the main window), separator, "Copy" (cell), "Copy rows as table" (Ctrl+Shift+C). No end or priority actions here [chosen: those stay in the main table].
- **Data binding:** `Snapshot.processes` records whose `group` equals the window's `GroupKey` (bit `PROCESSES`), collected only while this tab is active.
- **Build:** `app_props.rs`, `pub struct AppProcessesTab` (Entity) with a `TlTreeTable`.
- **Performance:** one linear pass over the records per sweep while the tab is active (about 2,000 records, under 50 µs); member keys are kept as `Arc<[u32]>` record indexes.
- **Accessibility:** rows announce nothing beyond their cells; the double-click action also has the keyboard path (Enter).
- **Tests:** `app_processes_members_only`; `app_processes_enter_opens_props`; `app_processes_count_label`.
- **Phase:** P2.
- **Decided here:** columns; sort; menu without end or priority actions; texts.

---

## D. Process action dialogs

### UI-PWN-021 Set Affinity dialog

- **Implements:** PROC-067 (dialog, All processors, grid, L3 grouping, SMT pairs, hybrid icons, list field, OK rule, toasts, cpuset limit), PROC-079 (thread affinity), R-039.
- **Anatomy:**

```text
+------------------------------------------------------------------+
| tasklens Set Affinity                                  [pin] [x] | 32
+------------------------------------------------------------------+
| firefox (PID 4321)                                               | t_sm text_secondary, 24
| [x] All processors                      CPU list [0-31         ] | 32
| -----------------------------------------------------------------|
| [-] CCD 0: CPU 0-7, 16-23                                        | group header 24
| +------++------++------++------++------++------++------++------+ |
| |[x]CPU 0|[x]CPU 1| ...                              |[x]CPU 7 | | core tiles 72 x 52, gap 4
| |[x]CPU16|[x]CPU17| ...                              |[x]CPU23 | |   top: first thread, bottom: SMT sibling
| +------++------++------++------++------++------++------++------+ |
| [x] CCD 1: CPU 8-15, 24-31                                       |
| ...                                                              |
| (!) At least one CPU must stay enabled                           | 20, only when nothing is checked
|                                              [Cancel]  [OK]      | footer 52
+------------------------------------------------------------------+
  width 640; height from content, 360 to 640, the group area scrolls beyond that
```

  - Window: `WindowKind::Dialog`, modal to the window that opened it (main window or Properties window), title "tasklens Set Affinity" [chosen, APP-045 naming], not resizable, width 640 [chosen: 8 tiles × 72 + 7 gaps × 4 + 2 × `sp6` padding = 636].
  - Subtitle: target description: `firefox (PID 4321)`; several processes: `4 processes`; threads: `Thread 4325 of firefox (PID 4321)` or `3 threads of firefox (PID 4321)`; app row: `Firefox (34 processes)` [chosen].
  - "All processors": TlCheckbox (checked when every allowed CPU is checked, indeterminate when some are) [spec label]. "CPU list" label `t_sm` `text_secondary` plus TlTextInput sm, width 200, placeholder "0-7,16-23" [spec].
  - Group header: height 24, a TlCheckbox for the whole group (indeterminate when partly checked) plus the label `t_base` medium `text_primary`: `CCD {n}: CPU {list}` for L3 groups [spec]; Intel hybrid: `P-cores: CPU {list}` and `E-cores: CPU {list}` with the app SVG icons `cpu-pcore` and `cpu-ecore` (12 px, `text_secondary`) before the label [spec intent, chosen labels]; a machine without L3 data: one group `CPUs: {list}` [chosen].
  - Core tile: 72 × 52, background `list_bg`, radius `r_sm`, padding `sp2`; two stacked rows of 24: a TlCheckbox with label `CPU {n}` (`t_sm`, tabular figures) for the first thread and, under it, its SMT sibling [spec pairing]. A core without SMT has one row and the lower half empty. Hybrid tiles show the P or E icon (10 px) before the label.
  - Tiles flow 8 per row in each group, ordered by core; groups follow in L3 order. More than 8 cores in a group wrap to new rows.
  - Validation line: Lucide `circle-alert` 14 px `status.danger` and the text "At least one CPU must stay enabled" `t_sm` `status.danger` [spec text], shown only when nothing is checked.
  - Footer: height 52, top border 1 px `divider`, padding `sp6`; right-aligned TlButton "Cancel" (Secondary) and TlButton "OK" (Primary) [spec label], gap `sp4`.
- **States:**
  - Loading the current masks (until the detail data arrives, at most one sweep): every checkbox disabled, the subtitle line ends with " - reading current affinity…" [chosen].
  - CPU outside the cgroup's `cpuset.cpus.effective`: checkbox disabled and unchecked, tooltip "Not allowed by the process's cpuset" [chosen wording; spec limits masks to it].
  - Offline CPU: disabled, tooltip "CPU is offline" [chosen].
  - Nothing checked: validation line shown; OK disabled [spec].
  - Invalid CPU list text: input border `status.danger`, helper text under the input "Use CPU numbers and ranges, for example 0-7,16-23" `t_xs` `status.danger`; OK disabled; the grid keeps its last valid state [chosen].
  - Checkbox states: the foundation's TlCheckbox states (UI-FND-011), including indeterminate (Lucide `minus`), which this dialog uses for mixed CPUs.
  - Target exited while open: OK stays enabled; the action is rejected by revalidation and shows the PROC-058 identity toast.
- **Interaction:**
  - Opened by the row menu "Set Affinity…" (`OpenSetAffinity { targets }`), by the General tab Affinity link, and by the Threads menu "Set thread affinity…". Targets are captured at menu open (max 256).
  - The grid and the list field stay in sync: a checkbox change rewrites the field to the canonical list (ranges merged, ascending); a valid field edit updates the grid on every Change event.
  - Clicking a group checkbox checks the whole group, or unchecks it when it is fully checked.
  - Keyboard (context `AffinityDialog`): Tab order: All processors, CPU list, the grid (one focus stop), Cancel, OK. Inside the grid: Left and Right move between CPUs in the same tile row and across tiles; Up and Down switch between a tile's two rows and then between tile rows and groups; Home and End go to the first and last CPU; Space toggles; Shift+Space toggles the group. Enter triggers OK when enabled; Esc and Cancel close without action.
  - OK (`ApplyAffinity`): one target or all targets with the same result mask: one `ActionRequest { kind: SetAffinity(mask) }`. The dialog closes at once; results come as toasts in the parent window.
  - Toasts [spec PROC-067 and PROC-069]: one process "Affinity updated" or "Could not update selected process affinity."; several "Selected process affinity updated.", "Could not update selected process affinity." or the partial form "Affinity updated for 3 of 4 processes. 1 failed: permission denied" with "Retry as administrator" (P3); threads "Thread affinity updated" [chosen].
- **Data binding:** CPU topology from `SystemSnapshot.cpu_topology` (L3 groups, sibling pairs, hybrid kind, online set); per-target masks and the cpuset limit from `DetailsSnapshot.affinity`, filled while the dialog registers `Demand.affinity_targets` (max 256) [ARCHITECTURE ADR-003 and ADR-008, R-119].
- **Build:** `crates/tl-ui/src/windows/affinity.rs`, `pub struct AffinityDialog` (Entity): `targets`, `initial: Vec<CpuSet>` (one per target), `state: CpuTriState` (`[u8; 1024]` with Off, On, Mixed), `list_input: Entity<InputState>`. Mixed CPUs use TlCheckbox's indeterminate state (UI-FND-011).
- **Performance:** at most 1,024 checkboxes; the grid is rebuilt only on state change; 32 CPUs render about 70 elements.
- **Accessibility:** every checkbox has its label as accessible name; tiles have the tooltip `Core {k}: CPU {a} and CPU {b}`; the validation text is also the OK button tooltip while disabled.
- **Tests:** `affinity_grid_groups_by_l3` (fixture with two L3 groups 0-7,16-23 and 8-15,24-31); `affinity_list_field_sync_both_ways`; `affinity_ok_disabled_when_empty_and_message`; `affinity_cpuset_limited_cpus_disabled`; `affinity_invalid_list_text`.
- **Phase:** P1 (one target); P2 (threads, several targets).
- **Decided here:** window size and layout; tile design; group labels for hybrid and no-L3 machines; subtitle forms; loading, offline, cpuset and invalid-list states; keyboard model; data addition.

### UI-PWN-022 Set Affinity with several targets (mixed marks)

- **Implements:** PROC-069 (Set Affinity applies to all targets; mixed marks; clicking a mixed CPU sets it on all), PROC-067.
- **Anatomy:** the UI-PWN-021 dialog, plus a TlButton Ghost sm "Reset" (Lucide `rotate-ccw` 12 px) at the left end of the footer, shown only with several targets [chosen].
- **States:**
  - Initial: each CPU is checked when every target has it, mixed when only some do, unchecked when none do [spec].
  - Click cycle: mixed to checked (sets it on all targets) [spec]; checked to unchecked; unchecked to checked. A CPU can return to mixed only through Reset, which restores the initial state of every CPU [chosen].
  - The CPU list field shows only the checked CPUs; while any CPU is mixed, the field shows the helper "Mixed CPUs keep each process's current setting" under it in `t_xs` `text_secondary` [chosen].
  - "Nothing checked" for this case means: no checked CPU and no mixed CPU.
- **Interaction:** OK computes each target's new mask: checked CPUs set, unchecked CPUs cleared, mixed CPUs kept from that target's current mask. Targets with equal resulting masks are grouped into one `ActionRequest` each; the `ActionTracker` joins the requests under one toast id, so the user sees one toast with the PROC-069 texts [chosen]. Toast texts as UI-PWN-021.
- **Data binding:** as UI-PWN-021.
- **Build:** in `affinity.rs`; `fn resulting_masks(&self) -> SmallVec<[(CpuSet, SmallVec<[ActionTarget; 4]>); 4]>`.
- **Performance:** grouping is linear in targets (max 256).
- **Accessibility:** a mixed checkbox has the tooltip "Set for some of the selected processes".
- **Tests:** `affinity_mixed_initial_marks`; `affinity_mixed_click_sets_all`; `affinity_reset_restores_mixed`; `affinity_ok_groups_targets_by_mask` (3 targets, 2 distinct results, assert 2 requests and 1 toast id).
- **Phase:** P2.
- **Decided here:** Reset button; click cycle; helper text; grouping of requests under one toast.

### UI-PWN-023 Custom priority dialog

- **Implements:** PROC-066 ("Custom…" dialog with nice slider, policy combo, RT priority 1-99; autogroup tooltip; app rows), PROC-079 (thread targets), PROC-069.
- **Anatomy:**

```text
+----------------------------------------------------------+
| tasklens Set Priority                          [pin] [x] | 32
| firefox (PID 4321)                                       | subtitle 24
| Policy             [Normal (SCHED_OTHER)          v]     | 32
| Nice               [-20 ======o================ 19]  0   | 32
|                    -20 (highest)            19 (lowest)  | 18, t_xs text_secondary
| Realtime priority  [ 1 ^v]                               | 32
| (!) Realtime priority can freeze the desktop if ...      | warning line, FIFO and RR only
|                                [Cancel] [Set priority]   | footer 52
+----------------------------------------------------------+
  width 440, height from content
```

  - `WindowKind::Dialog`, modal to its parent, title "tasklens Set Priority" [chosen], width 440, not resizable. Labels column 128 wide, `t_base` `text_secondary`; controls width 260.
  - Policy: TlSelect with items [chosen labels]: "Normal (SCHED_OTHER)", "Batch (SCHED_BATCH)", "Idle (SCHED_IDLE)", "Realtime FIFO (SCHED_FIFO)", "Realtime round-robin (SCHED_RR)" [spec policies].
  - Nice: TlSlider -20..=19, step 1 [spec], plus the current value as text right of the slider (`t_base`, tabular, width 28, right-aligned). End labels "-20 (highest)" and "19 (lowest)".
  - Realtime priority: TlStepper 1..=99, width 72 [spec range].
  - Warning line (FIFO and RR only): Lucide `triangle-alert` 14 px `status.warning`, text "Realtime priority can freeze the desktop if the process spins." `t_sm` `text_primary`, and on a second line "Child processes start at normal priority." `t_xs` `text_secondary` [chosen, explains the reset-on-fork flag].
  - Footer: "Cancel" and "Set priority" (Primary) [chosen label naming the result].
- **States:**
  - Initial values from the right-clicked target's current policy, nice and rt priority (General payload, thread row, or the record's `nice`).
  - Normal and Batch: Nice enabled; Realtime priority disabled.
  - Idle: Nice disabled with tooltip "Not used by this policy"; Realtime priority disabled.
  - FIFO and RR: Nice disabled (same tooltip); Realtime priority enabled; warning line shown.
  - Lowering nice below the current value, or any realtime policy, without privileges is allowed in the dialog; the engine returns EPERM and the toast follows PROC-058.
- **Interaction:**
  - Opened by "Custom…" in Set Priority (`OpenCustomPriority { targets }`), from the process table, the Threads tab, and app rows (applies to the app's current processes; the subtitle reads `Firefox (34 processes)` and the policy select has the tooltip "Applies to the app's current processes" [spec text]).
  - The Nice slider has the tooltip "With autogroup on, nice only orders processes within the same session." [spec text].
  - Keyboard (context `PriorityDialog`): Tab order Policy, Nice, Realtime priority, Cancel, Set priority; Left and Right move the slider by 1, PgUp and PgDn by 5, Home and End to -20 and 19; Up and Down step the spin box; Enter triggers "Set priority"; Esc cancels.
  - "Set priority" (`ApplyCustomPriority`) [chosen dispatch]:
    - Normal or Batch, same policy as now: one request `SetNice(n)`.
    - Normal or Batch, different policy: `SetSched { policy, rt_prio: 0 }`, then, after it succeeds, `SetNice(n)`; one toast id covers both.
    - Idle: `SetSched { policy: Idle, rt_prio: 0 }`.
    - FIFO or RR: first the realtime confirmation (UI-PWN-024), then `SetSched { policy, rt_prio }`.
    - The dialog closes when the request is sent (after the confirmation for realtime).
  - Toasts [spec]: "Priority updated" (one target), PROC-069 forms for several targets, "Thread priority updated" for threads [chosen].
- **Data binding:** reads current values as above; sends `Command::Action`.
- **Build:** `crates/tl-ui/src/windows/priority.rs`, `pub struct PriorityDialog` (Entity) with `policy: Entity<SelectState<..>>`, `nice: Entity<SliderState>`, `rt: TlStepper` state (UI-FND-015) created in the constructor.
- **Performance:** static dialog.
- **Accessibility:** slider accessible name "Nice value", spin box "Realtime priority"; the disabled reason is in the tooltip.
- **Tests:** `priority_dialog_enables_fields_by_policy`; `priority_dialog_dispatch_same_policy_nice_only`; `priority_dialog_policy_change_then_nice`; `priority_dialog_realtime_confirms_first`.
- **Phase:** P2.
- **Decided here:** layout and labels; button label; dispatch sequence; warning and child-process lines.

### UI-PWN-024 Realtime priority confirmation

- **Implements:** PROC-066 ("Realtime" asks first; SCHED_RR priority 1 with reset on fork).
- **Anatomy:** the process-action confirmation from `docs/ui/processes.md` UI-PRC-014, row "Set Priority > Realtime", with its exact texts: title "Set realtime priority?", description "Realtime priority can freeze the desktop if the process spins. Set it anyway?", buttons "Set realtime" (danger) and "Cancel". This file adds no text of its own.
- **States and keys:** as UI-PRC-014 and UI-FND-021.
- **Interaction:** opened by the Set Priority menu item "Realtime" (row menu or Threads menu) and by the Custom priority dialog for FIFO and RR (UI-PWN-023). Confirm sends `SetSched { policy: Rr, rt_prio: 1 }` for the menu item, or the Custom dialog's policy and priority. Toasts as UI-PWN-023.
- **Data binding:** the captured targets.
- **Build:** `priority.rs` calls the UI-PRC-014 helper `confirm(ConfirmKind::Realtime, targets, ..)` and continues with the request on confirm.
- **Tests:** `realtime_from_custom_dialog_confirms_first`; `realtime_cancel_sends_nothing`.
- **Phase:** P2.
- **Decided here:** nothing beyond the reuse.

### UI-PWN-025 Confirmations used by this file

- **Implements:** APP-045, PROC-059, R-083.
- **Anatomy:** this file draws no confirmation of its own. Every confirmation it triggers uses the process-action confirmation of `docs/ui/processes.md` UI-PRC-014 (texts, titles, button labels, focus rule) inside the `TlDialog` frame of UI-FND-021 (window title "tasklens"). The triggers from this file:

| Trigger | Entry | UI-PRC-014 row |
|---|---|---|
| Realtime priority (menu or Custom dialog) | UI-PWN-024 | Set Priority > Realtime |
| Signal to a protected target or to another user's process | UI-PWN-026, UI-PWN-011 | protected target; another user's process |
| Signal to several targets | UI-PWN-026, UI-PWN-011 | not in UI-PRC-014: title "Send signal to N processes?", text "Send SIG{NAME} to N selected processes?", button "Send signal" (danger when the signal ends processes, primary otherwise) [chosen; to be added to UI-PRC-014's table] |
| End Task or End Process from Handle Search results | UI-PWN-031 | End Task or End Process with 2+ targets; another user's process |
| Large dump | UI-PWN-034 | Create dump file, VmRSS > 2 GiB |
| VirusTotal first use | UI-PWN-036 | Open in VirusTotal, first use |

- **Build:** the UI-PRC-014 helper `pages/processes/confirm.rs::confirm(kind, targets, w, cx)`, called with the parent window's handle (a Properties window, Handle Search or the main window).
- **Tests:** `pwn_triggers_use_prc014_texts` (each trigger opens a confirmation whose title matches the UI-PRC-014 table).
- **Phase:** P1.
- **Decided here:** the several-targets signal confirmation text.

### UI-PWN-026 Send Signal submenu

- **Implements:** PROC-065 (signals, Other signals, Real-time signal…, "(ends process)", toast, app rows), PROC-059, R-039.
- **Anatomy:** a TlMenu submenu of the row menu item "Send Signal" (the row menu itself is specified in `docs/ui/processes.md`). Item height 26; min width 240; label `t_base` `text_primary` on the left; the suffix "(ends process)" `t_sm` `text_secondary` right-aligned [spec text, chosen placement].

```text
Terminate (TERM)            (ends process)
Kill (KILL)                 (ends process)
Hangup (HUP)                (ends process)
Interrupt (INT)             (ends process)
Quit (QUIT)                 (ends process)
Stop (STOP)
Continue (CONT)
User 1 (USR1)               (ends process)
User 2 (USR2)               (ends process)
-----------------------------------------
Other signals                           >
Real-time signal…
```

  - "Other signals" submenu, in this order, label `{NAME} ({number})` with x86-64 numbers [spec list, chosen format]: ABRT (6), ALRM (14), BUS (7), CHLD (17), FPE (8), ILL (4), IO/POLL (29), PIPE (13), PROF (27), PWR (30), SEGV (11), STKFLT (16), SYS (31), TRAP (5), TSTP (20), TTIN (21), TTOU (22), URG (23), VTALRM (26), WINCH (28), XCPU (24), XFSZ (25). Numbers come from `libc` constants at build time, so another architecture shows its own numbers.
  - "(ends process)" appears on signals whose default action terminates (with or without a core): TERM, KILL, HUP, INT, QUIT, USR1, USR2, ABRT, ALRM, BUS, FPE, ILL, IO/POLL, PIPE, PROF, PWR, SEGV, STKFLT, SYS, TRAP, VTALRM, XCPU, XFSZ. Not on STOP, CONT, CHLD, TSTP, TTIN, TTOU, URG, WINCH.
- **States:** hover `menu_hover`; keyboard highlight the same; disabled: the whole "Send Signal" item is disabled with tooltip "Kernel threads cannot be signalled." when every target is a kernel thread [chosen; PROC-059 otherwise refuses with that toast].
- **Interaction:**
  - Each item dispatches `SendSignal(Signal)` with the captured targets (`Signal::Other(n)` for the other list). App rows send to the primary processes (PROC-065).
  - Confirmation per PROC-059 (protected target, another user's process, or more than one target) through UI-PWN-025, which gives the texts.
  - Refusals (tasklens itself, PID 1, kernel threads) show the PROC-059 toasts.
  - Toasts: one process "Sent SIGHUP to firefox (1234)" [spec]; several "Sent SIGHUP to 4 processes" [chosen]; app row "Sent SIGHUP to Firefox (3 processes)" [chosen]; failures in the PROC-058 forms.
- **Data binding:** `Command::Action(ActionRequest { kind: Signal(sig), .. })`.
- **Build:** `crates/tl-ui/src/windows/signal.rs`, `pub fn signal_submenu(menu: PopupMenu, targets: &CapturedTargets, cx) -> PopupMenu`; signal names and default actions in a static table `SIGNALS: [SignalInfo; 31]` (`name`, `number`, `label`, `ends_process`).
- **Performance:** built on menu open only.
- **Accessibility:** keyboard navigation of nested kit menus (Right opens, Left closes); the "(ends process)" suffix is part of the item's accessible text.
- **Tests:** `signal_menu_labels_and_suffixes` (TERM has the suffix, STOP and CHLD do not); `signal_other_numbers_match_libc`; `signal_toast_single_multi_app`.
- **Phase:** P2.
- **Decided here:** suffix placement; label formats; disabled rule for kernel threads; multi and app toast texts; confirmation wording.

### UI-PWN-027 Real-time signal dialog

- **Implements:** PROC-065 ("Real-time signal…" spin box SIGRTMIN+0 to SIGRTMAX, 34 to 64).
- **Anatomy:**

```text
+--------------------------------------------+
| tasklens Send Signal             [pin] [x] | 32
| firefox (PID 4321)                         | subtitle 24
| Signal  [ 34 ^v]  SIGRTMIN+0               | 32
| Real-time signals end the process unless   | t_sm text_secondary
| it handles them.                           |
|                 [Cancel]  [Send signal]    | footer 52
+--------------------------------------------+
  width 360, height from content
```

  - `WindowKind::Dialog`, modal to its parent, title "tasklens Send Signal" [chosen]. TlStepper width 72, range 34..=64 [spec], then the name `t_base` `text_primary`: `SIGRTMIN+{n - 34}`, and for 64 `SIGRTMIN+30 (SIGRTMAX)` [chosen].
  - The range comes from `libc::SIGRTMIN()` and `libc::SIGRTMAX()` at run time; on this system 34 and 64.
- **States:** default value: the last value sent in this session, else 34 [chosen]. Typed values outside the range clamp on blur.
- **Interaction:** Up and Down step; Enter triggers "Send signal" (Primary, default button); Esc cancels. Sends `Signal::Other(n)` with the same confirmation rules and toasts as UI-PWN-026 ("Sent SIGRTMIN+2 to firefox (1234)").
- **Build:** `signal.rs`, `pub struct RtSignalDialog` (Entity).
- **Tests:** `rt_signal_label_for_34_and_64`; `rt_signal_remembers_last_value`.
- **Phase:** P2.
- **Decided here:** title, labels, note text, default value.

---

## E. Run, Search File Handles and low-memory windows

### UI-PWN-028 Run dialog

- **Implements:** PROC-095, APP-014, R-061, table (b) RunDialog keys.
- **Anatomy:**

```text
+------------------------------------------------------------+
| tasklens Run                                     [pin] [x] | 32
| [SquareTerminal 32]  Type the name of a program, folder,   | text t_base, wraps
|                      document, or Internet resource.       |
| Open:  [firefox --private-window                      v]   | 32
|        [ ] Run as administrator                            | 24
|            GUI programs may not start as root on Wayland   | 18, t_xs text_secondary
| [Browse…] [Clear command history]         [OK]  [Cancel]   | footer 52
+------------------------------------------------------------+
  480 x 248, not resizable
```

  - Separate pinnable window, `WindowKind::Floating`, not modal, title "tasklens Run" [spec]. Size 480 × 248 [chosen]. Content padding `sp6`.
  - Icon: Lucide `square-terminal` 32 px `text_secondary` [chosen], gap `sp5` to the text [spec text].
  - "Open:" label `t_base` `text_primary`, width 48; the combo (UI-PWN-029) fills the rest.
  - "Run as administrator": TlCheckbox [spec]; under it, indented to the checkbox label, the note "GUI programs may not start as root on Wayland" [spec].
  - Footer, top border 1 px `divider`: left "Browse…" and "Clear command history" (TlButton Secondary sm); right "OK" (Primary, default button) and "Cancel" [spec].
- **States:**
  - Initial: the combo holds the most recent history command with all text selected, or is empty with no history [chosen].
  - Launching (portal or polkit prompt pending): OK shows loading, other controls disabled.
  - History empty: "Clear command history" disabled.
- **Interaction:**
  - Ctrl+N or File > "Run..." opens it (`OpenRunDialog`); if open, it is activated.
  - Enter runs (`RunCommand`), Shift+Enter runs as administrator (`RunCommandElevated`) [spec]; OK runs with the checkbox state; Esc closes the popup first, then the window; Cancel closes.
  - Validation and toasts [spec texts]: empty text: "Please enter a command" (Warning, in the Run window, window stays); NUL or control character: "Command contains invalid text." (Error, stays); launch failure: "Failed to run command" (Error, stays); polkit prompt cancelled: "Authentication cancelled" (Info, stays); success: the Run window closes and the main window shows "Command launched" (Success) [chosen: closing on success follows the Windows Run box].
  - "Clear command history" (`ClearRunHistory`): clears the file at once, no confirmation; toast "Command history cleared" [spec].
  - "Browse…" (`BrowseRunTarget`): opens the portal file chooser with the filters "Programs" (executable files) and "All files" [spec]; the chosen path replaces the combo text, wrapped in single quotes when it contains a space or a shell character [chosen].
  - Tab order: combo, checkbox, Browse…, Clear command history, OK, Cancel.
- **Data binding:** history from `$XDG_STATE_HOME/tasklens/run-history.json` (read when the window opens, written after each launch and clear); launching goes through the shared launcher (SYS-023) with the PROC-095 rules (URL or existing non-executable path to OpenURI; `kcm_*` to `systemsettings`; else a transient scope; administrator through `pkexec env …`).
- **Build:** `crates/tl-ui/src/windows/run.rs`, `pub struct RunWindow` (Entity) with `input: Entity<InputState>`, `history: Arc<[RunEntry]>`, `elevated: bool`, `pending: Option<Task<()>>`.
- **Performance:** history at most 50 entries; matching is linear on each keystroke.
- **Accessibility:** the combo's accessible name is "Open"; the checkbox note is linked as its description (tooltip shows it too).
- **Tests:** `run_empty_command_toast`; `run_invalid_text_toast`; `run_shift_enter_elevated`; `run_success_closes_and_toasts_in_main`; `run_clear_history_toast_and_disabled`.
- **Phase:** P2.
- **Decided here:** size; icon; initial text; close on success; Browse quoting.

### UI-PWN-029 Run command combo and history

- **Implements:** PROC-095 ("Open:" combo, 5 visible rows, prefix matches first, Up and Down pick a match), R-061.
- **Anatomy:** a TlTextInput md (28 high) with a suffix TlButton Icon sm (24 × 24), Lucide `chevron-down` 14 px; a popup list under it, same width, 5 visible rows of 26 (130 high) [spec count], background `menu_bg`, 1 px border `divider`, radius `r_sm`, shadow per the foundation's popup style. Each row: the command (`t_base`, single line, ellipsis) and, for elevated entries, Lucide `shield-check` 12 px `text_secondary` at the right with the tooltip "Ran as administrator" [chosen].
- **States:** popup closed; open with no highlight; highlighted row `menu_hover`; no matches: popup closed.
- **Interaction:**
  - Typing filters the history: entries that start with the text (case-insensitive) first, newest first, then entries that contain it, newest first [spec order, chosen tie rule]; the popup opens when at least one entry matches and the text is not empty.
  - Down opens the popup (all history when the text is empty) and highlights the next row; Up highlights the previous row; the highlighted command is copied into the input as it moves [spec "pick a match"]; Enter runs the input text; Esc closes the popup.
  - Click on a row puts it in the input and closes the popup; double-click runs it [chosen].
  - The chevron toggles the popup.
- **Data binding:** `RunWindow.history`.
- **Build:** in `run.rs`; the popup is a kit `Popover` anchored to the input, with a plain `v_flex` of up to 50 rows inside a 130 px scroll area (no kit Combobox: its search box and multi-select do not fit) [chosen].
- **Performance:** at most 50 rows.
- **Accessibility:** popup rows are reachable only through Up and Down from the input; the chevron's accessible name is "Show command history".
- **Tests:** `run_combo_prefix_first_then_contains`; `run_combo_up_down_fills_input`; `run_combo_five_rows_visible`.
- **Phase:** P2.
- **Decided here:** popup build; tie rule; elevated marker; double-click runs.

### UI-PWN-030 Search File Handles window

- **Implements:** PROC-096, APP-015, R-061, R-102.
- **Anatomy:**

```text
+------------------------------------------------------------------------------+
| tasklens Handle Search                                            [pin] [x]  | 32
| Pattern: [.libreoffice*lock..............................] [Search] [Clear]  | 40
| [x] Include memory-mapped files, working directories and executables         | 28
| [=======-----]  Retrieving file handles... 42% (18 234 total handles)        | 24
| Process          | PID   | User    | Type | FD  | Path                       | TlTreeTable flat
| soffice.bin      | 8812  | oystein | fd   | 23  | /home/oystein/.~lock.x.odt#|
| ...                                                                          |
| Searched 412 of 530 processes. 118 need administrator rights. [Search all]  [Close] | footer 44
+------------------------------------------------------------------------------+
  default 860 x 560, minimum 560 x 360
```

  - `WindowKind::Floating`, not modal, title "tasklens Handle Search" [spec]. Default 860 × 560, minimum 560 × 360 [chosen]. Padding `sp5`.
  - "Pattern:" label `t_base`, then TlTextInput md (28 high, fills), then TlButton "Search" (Primary) and "Clear" (Secondary) [spec controls].
  - TlCheckbox "Include memory-mapped files, working directories and executables", default on [spec].
  - Progress row: `TlProgress` md 200 × 6 (as UI-PWN-017) plus status text `t_sm` `text_secondary`; hidden before the first search.
  - Footer: the privilege note `t_sm` `text_secondary` and TlButton sm "Search all" on the left (only when processes were skipped); TlButton "Close" on the right [spec]. Top border 1 px `divider`.
- **States:**
  - Before the first search: the table area shows "Enter a file name or path pattern, then press Search." [chosen].
  - Empty pattern: Search disabled.
  - Searching: Search becomes "Stop" (Secondary) [chosen; the walk is cancellable between PIDs]; pattern and checkbox disabled; status texts in order [spec]: "Initializing... please wait.", "Retrieving file handles... 42% (18 234 total handles)", "Searching...".
  - Done: "Found 12 matches", "Found 1 match" [chosen singular], or "No results found." [spec].
  - Stopped: "Search stopped. Found 12 matches" [chosen].
  - Error: toast "Handle search failed" (Error) and status "Handle search failed" [spec].
  - Helper prompt pending (Search all): "Search all" loading; cancelled: toast "Authentication cancelled".
- **Interaction:**
  - Enter in the pattern field or Search (`StartHandleSearch`) starts a search with a new `SearchId`; Stop (`StopHandleSearch`) or Esc while searching stops it (Esc closes the window only when idle) [chosen].
  - Clear (`ClearHandleSearch`) empties the pattern and the results.
  - "Search all" (`HandleSearchAll`) starts the helper (`StartHelperSession`) and searches again with `privileged = true` [spec].
  - Close or Esc when idle closes the window.
  - Opened by File > "Search File Handles..." (`OpenHandleSearch`); activates the existing window if open.
- **Data binding:** `Command::HandleSearch { search: SearchId, pattern: Arc<str>, include_maps: bool, privileged: bool }` and `Command::CancelHandleSearch(SearchId)`; results in `DetailsSnapshot.handle_search: Option<Arc<HandleSearchState>>` (progress, phase, `chunks` of hits, `searched`, `total`, `skipped`) [ARCHITECTURE ADR-003 and ADR-008, R-119]; bit `DETAILS`.
- **Build:** `crates/tl-ui/src/windows/handle_search.rs`, `pub struct HandleSearchWindow` (Entity).
- **Performance:** results are append-only chunks, never copied; status updates at most 4 Hz.
- **Accessibility:** the pattern field has the tooltip "Case-insensitive. * and ? work as wildcards." [chosen]; progress text is the bar's text alternative.
- **Tests:** `handle_search_status_sequence`; `handle_search_singular_match_text`; `handle_search_stop_keeps_results`; `handle_search_footer_only_when_skipped`.
- **Phase:** P3.
- **Decided here:** size; Stop button; empty, stopped and singular texts; Esc rule; wildcard tooltip; data addition.

### UI-PWN-031 Search File Handles results and menu

- **Implements:** PROC-096 (columns, result menu, footer), R-102, PROC-071, PROC-060, PROC-061, PROC-073.
- **Anatomy:** `TlTreeTable` flat:

| Column | Header | Width (min) | Align | Value |
|---|---|---|---|---|
| proc | Process | 200 (100) | left | 16 px process icon, gap `sp3`, display name |
| pid | PID | 80 (60) | right | integer |
| user | User | 110 (60) | left | user name |
| type | Type | 70 (60) | left | `fd`, `mmap`, `cwd`, `exe`, `root` [spec] |
| fd | FD | 60 (60) | right | fd number; empty for other types |
| path | Path | flex (200) | left | full path, middle ellipsis |

  Default sort: Process ascending, then PID, then FD [chosen]. Footer note [spec]: "Searched 412 of 530 processes. 118 need administrator rights." (counts use table d grouping).
- **States:** standard table states; rows of processes that exited after the search stay listed; actions on them fail revalidation with the PROC-058 identity toast.
- **Interaction:**
  - Row menu (TlMenu) [spec labels, R-102]: "End Task" (SIGTERM, PROC-060), "End Process" (SIGKILL, PROC-061), separator, "Go to process" (PROC-071 entry point, activates the main window), separator, "Copy path", "Open file location". Confirmations and toasts follow PROC-059 to PROC-061 (one own process: no confirmation; several rows: "End Selected Tasks" / "End Selected Processes" with confirmation; targets are the distinct process keys of the selected rows).
  - Keys (context `HandleSearch > Table`): Delete and Ctrl+E End Task; Shift+Delete and Ctrl+X End Process; Enter Go to process [chosen]; Ctrl+C copies the selected paths, one per line; Ctrl+Shift+C copies rows as TSV with header.
  - "Copy path" toast "Copied to clipboard"; "Open file location" toast "Opening file location" (file manager `ShowItems` with the path selected).
- **Data binding:** hit rows carry the `ProcessKey` captured during the search; actions go through `Command::Action`.
- **Build:** in `handle_search.rs`.
- **Performance:** virtualized; up to 100,000 hits [chosen cap; the search stops with "Showing the first 100 000 matches." in the status when reached].
- **Accessibility:** menu items show their key bindings (kit `PopupMenu` action keys).
- **Tests:** `handle_menu_labels_r102`; `handle_end_task_multi_confirms`; `handle_go_to_process_dispatches_entry_point`; `handle_hit_cap_status`.
- **Phase:** P3.
- **Decided here:** widths and sort; Enter = Go to process; result cap and its text.

### UI-PWN-032 Low-memory rescue window

- **Implements:** PROC-098 (window, texts, filter, Open Full tasklens, eased text, errors), PROC-097 (started by the watcher), R-062, R-073.
- **Anatomy:**

```text
+--------------------------------------------------+
| tasklens - Low memory                [pin] [x]   | 32, pin shown on
| Your device is running on critically low memory. | t_md semibold, status.danger
| We recommend you close some of the processes     | t_base text_secondary
| below.                                           |
| [Filter...                                   ]   | 32
| Firefox (34)              Memory 12.0% [End Task]| rows 32 (UI-PWN-033)
| ...                                              |
| [Open Full tasklens]                             | footer 52
+--------------------------------------------------+
  420 x 560, minimum 360 x 400
```

  - Title "tasklens - Low memory" [spec]. `WindowKind::Floating` with no parent (it is its own process, `tasklens --low-memory`). Kept above other windows from the start: the pin is shown toggled on and can be turned off [spec PROC-056 and R-073].
  - Size 420 × 560, minimum 360 × 400 [chosen]. Padding `sp6`.
  - Texts [spec]: "Your device is running on critically low memory." (`t_md` 14 semibold, `status.danger`) and "We recommend you close some of the processes below." (`t_base`, `text_secondary`).
  - Filter: TlTextInput md (28 high), placeholder "Filter..." [spec], filters names case-insensitively.
  - Footer: TlButton "Open Full tasklens" (Primary) on the left [spec label].
  - Minimal build [spec]: no charts, no process or app icons, only the two frame icons (pin and close, embedded SVGs); the font set is Noto Sans Regular and SemiBold only.
- **States:**
  - Eased (pressure below the threshold for 10 s): the two top lines are replaced by one line in `status.success`: "Memory pressure has eased. You can open the full tasklens or close this window." [spec text]. The list stays usable. If pressure rises again, the original lines return [chosen].
  - Filter with no match: "No processes match the filter." [chosen].
  - Errors: toast with title "End Task Failed" and message "kill failed: {errno text}" (Error) [spec texts].
- **Interaction:**
  - "Open Full tasklens" (`OpenFullTasklens`): sends `ActionKind::Launch(LaunchSpec { what: LaunchWhat::Command("tasklens"), as_admin: false, cwd: None })`; the started process activates the running instance or becomes the main one (APP-011, R-152); the rescue window stays open [chosen].
  - Esc closes the window (no filter text) or clears the filter first; Ctrl+W closes. Closing ends the `--low-memory` process.
  - Tab order: filter, list (one focus stop), Open Full tasklens, pin, close.
- **Data binding:** the engine runs in a low-memory mode: a stripped sweep (`stat`, `status`, cgroup `memory.current`) every 2 s plus the PSI state, published as `Snapshot.processes` and `Snapshot.health` only [`EngineMode::LowMemory`, ARCHITECTURE ADR-004, R-119].
- **Build:** `crates/tl-ui/src/windows/low_memory.rs`, `pub struct LowMemoryWindow` (Entity) and `pub fn init_low_memory(cx, client, init)`, the minimal init (`NotoSans-Regular` and `NotoSans-SemiBold` only, no icons, `bind_keys_low_memory` only) that `tl_ui::run` calls instead of `TasklensUi::init` when `UiLaunch.open == OpenMode::LowMemory` (R-243); started by `tl-app` for `--low-memory` with `mlockall(MCL_CURRENT)` already done (PROC-098).
- **Performance:** at most 30 rows; one re-render per 2 s sweep; no animation; the window opens in under 200 ms (ADR-014 first-frame gate applies).
- **Accessibility:** End Task buttons carry the row name in their accessible name ("End Task: Firefox").
- **Tests:** `lowmem_texts_and_title`; `lowmem_eased_after_10s` (fake PSI below threshold for 10 s); `lowmem_starts_pinned`; `lowmem_open_full_activates_or_spawns`.
- **Phase:** P3.
- **Decided here:** size; danger colour on the first line; eased replacement and return; window stays open after Open Full tasklens; filter empty text.

### UI-PWN-033 Low-memory process list

- **Implements:** PROC-098 (list of at most 30 apps and processes by memory, "Memory 12.0%", End Task with SIGKILL or KillUnit, no confirmation, tooltip, other users disabled).
- **Anatomy:** each row 32 high (× UI scale, not affected by row height) [chosen], padding `sp4` horizontal, 1 px bottom border `divider`:
  - Name, `t_base` `text_primary`, ellipsis: apps as `{app name} ({process count})`, processes as their display name [chosen].
  - "Memory 12.0%" `t_sm` `text_secondary`, right-aligned, width 100, tabular [spec format].
  - TlButton Danger sm "End Task" width 84 [spec label].
- **States:**
  - Sorted by memory descending (RssAnon + swap for processes, cgroup `memory.current` for apps), at most 30 rows [spec].
  - While the pointer is over the list or a row has keyboard focus, the order is held and only values update; it re-sorts when the pointer leaves and focus moves out [chosen, so the target does not move under the click].
  - Other users' rows: button disabled, tooltip "Needs administrator rights" [spec].
  - Ending: the button shows loading; on success the row is removed at the next sweep.
  - End Task tooltip: "Ends immediately, unsaved work is lost" [spec].
- **Interaction:** End Task (`LowMemEndTask`) sends SIGKILL to a process, or `KillUnit(name, "all", SIGKILL)` to an app, with no confirmation [spec]. Keyboard in the list: Up and Down move a highlight (`row_selected`), Enter or Delete presses End Task on the highlighted row [chosen].
- **Data binding:** `Snapshot.processes` records and app groups from the low-memory sweep; actions through `Command::Action` with identity revalidation (PROC-001).
- **Build:** a plain `v_flex` of up to 30 rows in `low_memory.rs` (no `TlTreeTable`, to keep the window minimal) [chosen].
- **Performance:** 30 rows; strings re-formatted only when the percent changes.
- **Accessibility:** as UI-PWN-032.
- **Tests:** `lowmem_list_caps_30_sorted`; `lowmem_order_held_while_hovered`; `lowmem_other_user_disabled_tooltip`; `lowmem_end_task_no_confirmation`.
- **Phase:** P3.
- **Decided here:** row design; name format for apps; order hold; keyboard; plain list instead of the table component.

---

## F. Dump toasts and VirusTotal notice

### UI-PWN-034 Dump confirmation and progress toast

- **Implements:** PROC-075 (confirmation over 2 GiB, progress toast, gdb missing, administrator rights), R-004.
- **Anatomy:**
  - Confirmation (only when VmRSS is over 2 GiB) [spec]: UI-PRC-014 row "Create dump file, VmRSS > 2 GiB" (title "Create a large dump?", button "Create dump"), through UI-PWN-025.
  - Progress toast: TlToast Info, top centre of the window that started the dump, autohide off while running, id `DumpToast(action_id)`, text "Writing core… 1.2 GiB" [spec], the size in table (d) memory format, updated every 500 ms [chosen].
- **States:**
  - Before the first size read: "Writing core…" [chosen].
  - Menu item states (the row menu is owned by `docs/ui/processes.md`): disabled with tooltip "Install gdb to create dumps" when `/usr/bin/gcore` is missing [spec].
  - Without administrator access: Warning toast "Creating a dump needs administrator rights" [spec], with the "Retry as administrator" button from P3.
- **Interaction:** "Create dump file" in the row menu (`CreateDump { key }`) opens the confirmation when needed, then sends the dump request. The progress size comes from the UI stat-ing the output path every 500 ms on the background executor (`std::fs::metadata(path).len()`) while the action is pending [chosen: ADR-005 action events carry no progress].
- **Data binding:** setting `processes.dump_type` decides the mode (`Mini` or `Full`) [spec]; the output path `$XDG_STATE_HOME/tasklens/dumps/{name}_{pid}_{yyyymmdd_hhmmss}.core` is chosen by the UI and passed in the request, so it can be stat-ed. `{name}`: `/`, `\0` and every control character become `_`, the name part is cut to 64 bytes on a char boundary, and `process` is used when the result is empty; the timestamp is local time formatted with `jiff` (R-224). The engine opens the file itself (`O_EXCL`, mode 0600) and runs gdb's `gcore` into it (R-145).
- **Build:** `crates/tl-ui/src/windows/notices.rs`, `fn start_dump(key, cx)` and the `DumpProgress` task (cancelled when the action ends).
- **Performance:** one `stat` every 500 ms off the UI thread during a dump.
- **Accessibility:** toast text is the only progress indicator; it carries the size as text.
- **Tests:** `dump_confirm_only_over_2gib`; `dump_progress_toast_updates_in_place` (same toast id, new text); `dump_disabled_without_gcore_tooltip`.
- **Phase:** P3.
- **Decided here:** 500 ms stat polling; the UI picks the output path; "Writing core…" before the first size.

### UI-PWN-035 Dump result toasts

- **Implements:** PROC-075 ("Dump created: <path>" with "Open folder").
- **Anatomy:** the progress toast is replaced in place (same id) by:
  - Success: TlToast Success, text "Dump created: {path}" [spec] with `$HOME` shown as `~` [chosen], path ellipsized in the middle at the toast width; action button "Open folder" [spec]. Because the kit turns autohide off for toasts with an action, this toast stays until closed or replaced [consequence of the kit, accepted].
  - Failure: TlToast Error, "Could not create dump file." with the error text on a second line [chosen]; 8 s.
  - Rejected for privileges: as UI-PWN-034.
- **Interaction:** "Open folder" (`OpenDumpFolder`) calls `org.freedesktop.FileManager1.ShowItems(["file://{path}"], "")`, so the file manager opens the dumps folder with the new file selected; toast "Opening file location" [chosen reuse]. Clicking the path text copies the full path, toast "Copied to clipboard" [chosen].
- **Data binding:** the `ActionEvent` final state for the dump action.
- **Build:** `notices.rs`.
- **Tests:** `dump_success_toast_text_and_button`; `dump_failure_toast_text`.
- **Phase:** P3.
- **Decided here:** `~` shortening; failure text; path click copies.

### UI-PWN-036 VirusTotal notice and hashing toast

- **Implements:** PROC-074 (background hash, toast after 500 ms, one-time notice, errors, nothing uploaded), R-095.
- **Anatomy:**
  - Notice: UI-PRC-014 row "Open in VirusTotal, first use" (title "Open VirusTotal?", text "This opens virustotal.com with the file's hash. The file is not uploaded." [spec text], buttons "Open" and "Cancel"), through UI-PWN-025.
  - Hashing toast: TlToast Info "Computing SHA-256…" [spec], id `VtToast(action_id)`, shown only when hashing takes longer than 500 ms [spec].
- **States:**
  - First use (`state.toml [processes] virustotal_notice_accepted = false`): the notice opens before anything is hashed. "Open" sets the flag to true (saved through the state file's debounced save) and continues; Cancel does nothing and keeps the flag false [spec R-095].
  - Later uses: no notice.
  - Hash done: the toast (if shown) is removed and the browser opens `https://www.virustotal.com/gui/file/{sha256}` through the portal OpenURI, falling back to `xdg-open` [spec].
  - Errors (Error toasts, replacing the hashing toast) [spec]: "SHA-256 unavailable", "Process image unavailable"; browser launch failure: "Could not open the browser." [chosen].
- **Interaction:** row menu item "Open in VirusTotal" (`OpenVirusTotal { key }`, menu owned by `docs/ui/processes.md`). Keys in the notice follow UI-PRC-014.
- **Data binding:** `Command::ComputeHashes(key)` (shared with UI-PWN-008; the engine cache by dev, inode and mtime makes a second request instant); the result arrives in the General detail payload for that key, which the UI demands while the request is pending.
- **Build:** `notices.rs`, `fn open_virustotal(key, cx)`.
- **Performance:** hashing off the UI thread; the 500 ms timer is one `Timer::after` task, dropped when the hash arrives first.
- **Accessibility:** the notice states what is sent (the hash) and what is not (the file).
- **Tests:** `vt_notice_shown_once_then_flag_saved`; `vt_cancel_keeps_flag_false`; `vt_toast_only_after_500ms`; `vt_error_toasts`.
- **Phase:** P2.
- **Decided here:** browser failure text; reuse of the General hash path.

---

## Summary table

| ID | Name | Implements | Phase |
|---|---|---|---|
| UI-PWN-001 | Secondary window frame (use of TlSecondaryWindow) | APP-045, APP-009, R-073, PROC-056, PROC-077, D-006 | P1, P2 |
| UI-PWN-002 | Properties data path | PROC-077 to PROC-085 data, R-097, R-098 | P2 |
| UI-PWN-003 | Process Properties window | PROC-077, PROC-071, PROC-056, R-050, D-008 | P2 |
| UI-PWN-004 | Properties tab strip with counts | PROC-077 | P2 |
| UI-PWN-005 | Per-tab filter bar and refresh behaviour | PROC-077, PROC-084 | P2 |
| UI-PWN-006 | General tab layout | PROC-078, R-050, R-096 | P2 |
| UI-PWN-007 | General tab: copy on click, links and keyboard | PROC-078, PROC-071, PROC-073, APP-044 | P2 |
| UI-PWN-008 | General tab: hashes and Verify against package | PROC-078, PROC-074, R-096 | P2 |
| UI-PWN-009 | General tab: window block | PROC-078, PROC-052, PROC-053 | P2 |
| UI-PWN-010 | Threads tab | PROC-079, R-097 | P2 |
| UI-PWN-011 | Threads row menu and thread actions | PROC-079, PROC-066, PROC-067, PROC-059, R-097 | P2 |
| UI-PWN-012 | File descriptors tab | PROC-080 | P2 |
| UI-PWN-013 | Memory maps tab | PROC-081, R-098 | P2 |
| UI-PWN-014 | Security tab: summary | PROC-082, R-050 | P2 |
| UI-PWN-015 | Security tab: Capabilities and Groups tables | PROC-082 | P2 |
| UI-PWN-016 | Environment tab | PROC-083, R-099 | P2 |
| UI-PWN-017 | Strings tab | PROC-084, R-004 | P3 |
| UI-PWN-018 | Tab access states | PROC-080 to PROC-084, PROC-076, APP-003, R-050 | P2, P3 |
| UI-PWN-019 | App Properties window and General tab | PROC-085, PROC-077, D-008 | P2 |
| UI-PWN-020 | App Properties: Processes tab | PROC-085 | P2 |
| UI-PWN-021 | Set Affinity dialog | PROC-067, PROC-079, R-039 | P1, P2 |
| UI-PWN-022 | Set Affinity with several targets | PROC-069, PROC-067 | P2 |
| UI-PWN-023 | Custom priority dialog | PROC-066, PROC-069, PROC-079 | P2 |
| UI-PWN-024 | Realtime priority confirmation | PROC-066 | P2 |
| UI-PWN-025 | Confirmations used by this file | APP-045, PROC-059, R-083 | P1 |
| UI-PWN-026 | Send Signal submenu | PROC-065, PROC-059, R-039 | P2 |
| UI-PWN-027 | Real-time signal dialog | PROC-065 | P2 |
| UI-PWN-028 | Run dialog | PROC-095, APP-014, R-061 | P2 |
| UI-PWN-029 | Run command combo and history | PROC-095, R-061 | P2 |
| UI-PWN-030 | Search File Handles window | PROC-096, APP-015, R-061, R-102 | P3 |
| UI-PWN-031 | Search File Handles results and menu | PROC-096, R-102, PROC-060, PROC-061, PROC-071, PROC-073 | P3 |
| UI-PWN-032 | Low-memory rescue window | PROC-098, PROC-097, R-062, R-073 | P3 |
| UI-PWN-033 | Low-memory process list | PROC-098 | P3 |
| UI-PWN-034 | Dump confirmation and progress toast | PROC-075, R-004 | P3 |
| UI-PWN-035 | Dump result toasts | PROC-075 | P3 |
| UI-PWN-036 | VirusTotal notice and hashing toast | PROC-074, R-095 | P2 |

## Coverage

Every merged spec ID in this file's scope, and the entries that realize it:

- PROC-056 Pin on top for tasklens windows: UI-PWN-001, UI-PWN-003, UI-PWN-032.
- PROC-058 Action pipeline, results and toasts (as used by these windows): UI-PWN-011, UI-PWN-021, UI-PWN-022, UI-PWN-023, UI-PWN-026, UI-PWN-031.
- PROC-059 Protection rules and confirmations (as triggered here): UI-PWN-011, UI-PWN-025, UI-PWN-026, UI-PWN-031.
- PROC-065 Send Signal submenu: UI-PWN-026, UI-PWN-027.
- PROC-066 Set Priority (Custom dialog and Realtime confirmation): UI-PWN-023, UI-PWN-024, UI-PWN-011.
- PROC-067 Set Affinity: UI-PWN-021, UI-PWN-007 (Affinity link), UI-PWN-011.
- PROC-069 Multi-select priority and affinity: UI-PWN-022, UI-PWN-023.
- PROC-071 Jump target (PPID link, Go to process, Properties failure toast): UI-PWN-003, UI-PWN-007, UI-PWN-020, UI-PWN-031.
- PROC-073 Open file location (Executable link, menus): UI-PWN-007, UI-PWN-012, UI-PWN-013, UI-PWN-031.
- PROC-074 Open in VirusTotal: UI-PWN-036, UI-PWN-008 (shared hash).
- PROC-075 Create dump file: UI-PWN-034, UI-PWN-035.
- PROC-076 Privileged helper (Grant access, Enable, Search all, Retry as administrator): UI-PWN-018, UI-PWN-030.
- PROC-077 Process Properties window: UI-PWN-001, UI-PWN-002, UI-PWN-003, UI-PWN-004, UI-PWN-005.
- PROC-078 General tab: UI-PWN-006, UI-PWN-007, UI-PWN-008, UI-PWN-009.
- PROC-079 Threads tab: UI-PWN-010, UI-PWN-011.
- PROC-080 File descriptors tab: UI-PWN-012, UI-PWN-018.
- PROC-081 Memory maps tab: UI-PWN-013, UI-PWN-018.
- PROC-082 Security tab: UI-PWN-014, UI-PWN-015, UI-PWN-018.
- PROC-083 Environment tab: UI-PWN-016, UI-PWN-018.
- PROC-084 Strings tab: UI-PWN-017, UI-PWN-018, UI-PWN-005 (Value filter).
- PROC-085 App Properties window: UI-PWN-019, UI-PWN-020.
- PROC-095 Run dialog: UI-PWN-028, UI-PWN-029.
- PROC-096 Search File Handles: UI-PWN-030, UI-PWN-031.
- PROC-097 Low-memory watcher: UI-PWN-032 (the window it starts). The settings switch "Open rescue window on critical memory pressure" belongs to the Settings window spec, not this file.
- PROC-098 Low-memory rescue window: UI-PWN-032, UI-PWN-033.
- APP-009 Always on top and the pin (secondary windows): UI-PWN-001.
- APP-014 Run dialog frame and privilege path: UI-PWN-028.
- APP-015 Search File Handles frame: UI-PWN-030.
- APP-044 Toasts (texts and placement in these windows): UI-PWN-003, UI-PWN-007, UI-PWN-028, UI-PWN-034, UI-PWN-035, UI-PWN-036.
- APP-045 Dialogs as separate pinnable windows: UI-PWN-001, UI-PWN-025.
- R-050 validity words: UI-PWN-006, UI-PWN-014. R-061: UI-PWN-028, UI-PWN-030. R-073: UI-PWN-001. R-095: UI-PWN-036. R-096: UI-PWN-008. R-097: UI-PWN-010, UI-PWN-011. R-098: UI-PWN-013. R-099: UI-PWN-016. R-102: UI-PWN-031. D-008: UI-PWN-003, UI-PWN-019, UI-PWN-020. D-009: UI-PWN-031.

No in-scope ID is left without an entry.

## Decided here (all entries)

Values chosen in this file, for reviewers to check. Each entry also lists its own.

1. Window sizes and kinds table (UI-PWN-001); App Properties 720 × 560; Run fixed 480 × 248; Handle Search 860 × 560; low memory 420 × 560.
2. Esc order in these windows: popup, then running scan or search, then filter, then close.
3. Properties data path: `Demand.details`, `DetailTab`, `Snapshot.details`, bit `DETAILS`, the commands `DetailRefresh`, `ComputeHashes`, `VerifyPackage`, `StringScan`, `CancelStringScan`, `HandleSearch`, `CancelHandleSearch`, and `Demand.affinity_targets` with `DetailsSnapshot.affinity` (UI-PWN-002, UI-PWN-021, UI-PWN-030).
4. Exited banner text and look; frozen values; actions disabled with "The process is not running." (UI-PWN-003).
5. Tab counts without placeholders; Alt+1..7 and Ctrl+PgUp/PgDn (UI-PWN-004).
6. Filter field 280 px fixed; matched fields per tab; background filtering above 2,000 rows; no Ctrl-pause in secondary windows (UI-PWN-005).
7. General tab: 26 px rows, label width rule (96..150 px), 840 px breakpoint, full-width command line block, Autogroup and Hardening rows, all value words for hardening and architecture, Net I/O wording (UI-PWN-006).
8. Field menu adds "Copy all fields"; keyboard model of the grid (UI-PWN-007).
9. Hash display (16 lowercase digits), "Computing…", package status words and verify toasts (UI-PWN-008).
10. Window block order, "Needs the KDE window bridge." tooltip, "And N more" tooltip (UI-PWN-009).
11. Column widths, default sorts and copy formats for every table (UI-PWN-010, 012, 013, 015, 016, 017, 020, 031).
12. Thread action toasts and "(ends process)" on USR1 and USR2 (UI-PWN-011, UI-PWN-026).
13. File descriptor details pane layout, extra fdinfo fields, decoded flag words (UI-PWN-012).
14. Memory maps node versus region cells, collapsed default (UI-PWN-013).
15. Security layout, labels, capability row rule, group marks (UI-PWN-014, UI-PWN-015).
16. Environment reveal controls and lifetime, read-error row, truncation footer (UI-PWN-016).
17. Strings texts (before, finished, cancelled, exited), 512-character cut, escapes, 4 Hz batches, Esc cancels first (UI-PWN-017).
18. Access block look; buttons hidden before P3; no button on Security (UI-PWN-018).
19. App Properties header block, panel split and wordings (UI-PWN-019, UI-PWN-020).
20. Set Affinity layout (core tiles), group labels for hybrid and no-L3 machines, loading, cpuset, offline and invalid-list states, keyboard, Reset for mixed marks, request grouping under one toast (UI-PWN-021, UI-PWN-022).
21. Custom priority labels, button "Set priority", dispatch sequence (UI-PWN-023).
22. Several-targets signal confirmation text (UI-PWN-025).
23. Real-time signal dialog title, labels and default value (UI-PWN-027).
24. Run: icon, initial text, close on success with the toast in the main window, Browse quoting, combo tie rule, elevated marker (UI-PWN-028, UI-PWN-029).
25. Handle Search: Stop button, texts for empty, stopped and singular results, Enter = Go to process, 100,000-hit cap (UI-PWN-030, UI-PWN-031).
26. Low memory: danger colour of the first line, eased line return, window stays open after "Open Full tasklens", order hold while hovered, keyboard, `EngineMode::LowMemory` (UI-PWN-032, UI-PWN-033).
27. Dump: UI picks the output path and polls its size every 500 ms; "~" shortening; failure text (UI-PWN-034, UI-PWN-035).
28. VirusTotal browser failure text (UI-PWN-036).

## Open items

None. Settled in `docs/RECONCILIATION.md`: the Properties data path, `Demand.affinity_targets`,
the handle search commands and state, `EngineMode::LowMemory`, `SystemSnapshot.cpu_topology` and
dump progress by output file size are current-state text in ARCHITECTURE ADR-003, ADR-004,
ADR-005 and ADR-008 (R-119); the foundation parameters and the Run window row (R-127); the
processes.md overlap (R-127); the confirmation focus rule: focus starts on Cancel and Enter
activates the focused button (R-104).
