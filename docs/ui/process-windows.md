# tasklens UI spec: process windows and dialogs

This file specifies the secondary windows of the processes area: the Process Properties window and its seven tabs, the App Properties window, the Set Affinity, Custom priority and Real-time signal dialogs, the Send Signal submenu, the Run dialog, the Search File Handles window, the low-memory rescue window, the dump toasts and the VirusTotal notice. IDs use the prefix UI-PWN.

Precedence: `docs/DECISIONS.md`, then `docs/RECONCILIATION.md`, then `docs/ARCHITECTURE.md`, then `docs/spec/merged/proc.md` and `sys-app.md`. Component and token names come from `docs/ui/foundation.md` (TlSecondaryWindow, TlTabStrip, TlButton, TlCheckbox, TlInput, TlSelect, TlSpinBox, TlSlider, TlKeyValueGrid, TlMenu, TlToast, TlDialog, TlTooltip, TlProgressBar, TlLink and the tokens of `TlTheme`). Tables use `TlTreeTable` from `docs/ui/processes.md`; this file only says which columns, rows and menus each table has. It does not respecify the component.

## Conventions used in every entry

- **Sizes** are logical px at `ui_scale_pct = 100` and are multiplied by `ui_scale_pct / 100`. The row height `RH` is `28 × row_height_pct/100 × ui_scale_pct/100` (R-020). Spacing tokens `sp1` to `sp8`, text styles `t_xs` to `t_xl`, radii `r_sm` and `r_md`, motion `m_fast`, `m_base`, `m_slow` and `ease_out_cubic` are the shared vocabulary of the UI brief.
- **Measurements.** The TaskSlinger frames in `tasklens-ref/evidence-frames/advanced-*.png` are 1728 × 1080 and show 1.386 screen px per logical px (main-table row pitch 38.8 px = 28 logical). A value marked **[measured]** comes from those frames divided by 1.386; **[spec]** comes from a merged spec, R or D entry; **[chosen]** was decided in this file and is repeated in "Decided here".
- **Colours measured in the TaskSlinger Properties panel** (Default Dark): title row `rgb(40,46,50)` = `titlebar`; inactive tab `rgb(31,32,35)` = `tab_bg`; active tab `rgb(34,38,40)`, which joins the body `rgb(30,34,36)` = `window_bg`; tab separators `rgb(48..52)` = `divider`; key/value column panels `rgb(27,31,33)` = `list_bg`; table section header `rgb(50,54,58)` = `row_hover` tone, table column header `rgb(41,47,51)` = `header`. Labels are `text_secondary`, values `text_primary`.
- **Validity words.** Key/value fields use the PERF-013 words (`Collecting…`, `Restricted`, `Needs setup`, `Not supported`, `Not reported`, `Unavailable`, `Unknown`) in `text_secondary`, never a zero (R-050). Table cells inside these windows use the ADR-003 glyphs, as the process table does.
- **Numbers** follow canonical table (d): tabular figures (`tnum`), right-aligned in table cells, left-aligned in key/value fields.
- **Actions.** Every command names its GPUI `Action`. Actions are declared with `actions!(tl_pwn, [...])` in `crates/tl-ui/src/windows/actions.rs` unless the entry says otherwise. Key contexts used here: `PropsWindow`, `PropsTable`, `AffinityDialog`, `PriorityDialog`, `SignalDialog`, `RunDialog` (table b), `HandleSearch`, `LowMemory`, plus the shared `Table` context of TlTreeTable.
- **Files.** New code lives under `crates/tl-ui/src/windows/`: `frame.rs`, `props/{mod.rs, general.rs, threads.rs, fds.rs, maps.rs, security.rs, environ.rs, strings.rs, filter.rs}`, `app_props.rs`, `affinity.rs`, `priority.rs`, `signal.rs`, `run.rs`, `handle_search.rs`, `low_memory.rs`, `notices.rs`, `actions.rs`. This extends the ADR-009 tree; it is listed in "Decided here".
- **Reduced motion** means `App::reduce_motion()` is true or `motion.animations` is false (R-017).

---

## A. Shared frame and data path

### UI-PWN-001 Secondary window frame

- **Implements:** APP-045, APP-009 (pin on secondary windows), R-073, PROC-056, PROC-077 (frame part), D-006.
- **Anatomy:**

```text
+--------------------------------------------------------------------------+
| [title text, t_base medium, ellipsis]                    [pin] [ x ]     |  32 px title bar, `titlebar`
+--------------------------------------------------------------------------+
| content (window_bg)                                                      |
|                                                                          |
+--------------------------------------------------------------------------+
  1 px border `divider` on all four edges (client-side decorations only)
```

  - Title bar: height 32 [chosen; TaskSlinger's panel title row measures 43, which includes the panel's outer padding]. Background `titlebar`. Left padding `sp5` (12). Title text `t_base` (13) medium 500, `text_primary`, single line, ellipsis at the end, vertically centred.
  - Pin button: TlButton ghost, icon only, 24 × 24, icon Lucide `Pin` 14 px, `text_secondary`. Toggled on: the icon stays `Pin`, the button gets background `accent` at 20% alpha and icon colour `accent`. Placed 8 px left of the close button.
  - Close button: TlButton ghost, icon only, 24 × 24, Lucide `X` 14 px, `text_secondary`; hover background `status.danger` at 85% alpha, icon `text_selected_primary` [chosen, matches the main window caption close].
  - Right padding `sp3` (6). Buttons are vertically centred.
  - No minimize or maximize buttons [chosen; TaskSlinger's panels show only close]. Double-click on the title bar toggles maximize (CSD behaviour from ADR-010). Right-click on the title bar shows the compositor window menu (`window.show_window_menu`).
  - Resize: 6 px invisible resize border on every edge and corner (gpui-kit `WindowBorder`), CSD only.
  - With `appearance.decorations = "server"`, KWin draws the title bar. tasklens then omits its own 32 px bar and shows the pin as the first item of a 28 px strip at the top of the content only for windows whose content has no toolbar; Properties windows put the pin at the right end of the tab strip instead (UI-PWN-004).
- **States:**
  - Pin default: icon `text_secondary`. Hover: background `menu_hover`. Pressed: background `menu_hover` darkened 8% (L − 0.08). Focused (keyboard): 2 px focus ring `accent`, offset 1 px, radius `r_sm`. Toggled on: as above. Disabled (no mechanism, R-073): icon at 40% alpha, no hover, tooltip "Not supported by this desktop".
  - Close: default, hover (danger fill), pressed (danger fill at 100%), focused (ring).
  - Window inactive: title text `text_secondary`; buttons unchanged.
- **Interaction:**
  - Drag on any title bar area that is not a button moves the window (`window.start_window_move()` on left mouse-down).
  - Pin click dispatches `TogglePinWindow`. On KDE it sends `Command::KWin(KWinCommand::KeepAbove { pid: own, caption: <window title>, on: bool })` (APP-009); on X11 it sets `_NET_WM_STATE_ABOVE`. The state is held only in the window entity (only the main window persists, R-073).
  - Close click, Ctrl+W, and Esc (when no menu, popup, select list or sub-dialog in this window is open) dispatch `CloseSecondaryWindow`, which calls `window.remove_window()`.
  - Esc order inside the window: close the topmost popup or menu; else clear a focused, non-empty filter field; else close the window. This is the table (b) Esc rule applied per window.
  - Tab order: content first, then pin, then close (the title bar comes last so Tab from a field goes into the content).
- **Data binding:** none, except the low-memory window, which is always pinned (UI-PWN-032). Pin availability reads `Snapshot.windows.header.validity` (`Unsupported` means no bridge) and the X11 backend flag.
- **Build:** `crates/tl-ui/src/windows/frame.rs`, `pub struct TlSecondaryWindow` (RenderOnce) from the foundation file, given `title: SharedString`, `pinned: bool`, `pin_available: PinAvailability`, and the content element. Windows open with:

```rust
WindowOptions {
    kind: WindowKind::Floating,            // tools and property windows (APP-045)
    window_decorations: Some(WindowDecorations::Client),   // or Server per appearance.decorations
    window_background: WindowBackgroundAppearance::Opaque,
    app_id: Some("io.github.oysteinkrog.Tasklens".into()),
    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
    window_min_size: Some(min_size),
    titlebar: Some(TitlebarOptions { title: Some(title), ..Default::default() }),
    ..Default::default()
}
```

  Modal confirmations use `WindowKind::Dialog` (UI-PWN-025 and others). Each window gets its own gpui-kit `Root`, so toasts and menus are per window (APP-045). Window titles are set with `window.set_window_title` whenever the title text changes (UI-PWN-003).
- **Performance:** the frame is static; it re-renders only on pin change, focus change or title change. Title text is a `SharedString` built once per change.
- **Accessibility:** pin accessible name "Keep window above others", close "Close window". Both have tooltips with those names (500 ms delay). Focus ring on keyboard focus only (`focus_visible`).
- **Tests:** `frame_esc_closes_window_when_nothing_open` (TestAppContext: open a Properties window, dispatch Esc, assert the window count drops by one); `frame_esc_closes_menu_first` (open a row menu, Esc, assert the window stays open and the menu is gone); `frame_pin_disabled_without_bridge` (windows component `Unsupported`, assert the pin button is disabled and its tooltip text).
- **Phase:** P1 (frame, close, Esc); P2 (pin through the bridge).
- **Decided here:** title bar 32 px; pin and close 24 px; no minimize or maximize buttons; pin placement with server decorations.

### UI-PWN-002 Properties data path

- **Implements:** PROC-077 ("the window registers demand for its ProcessKey and the active tab's data"), PROC-078 to PROC-085 data parts, R-097, R-098.
- **Anatomy:** no visible part. This entry fixes how Properties windows get data, because ADR-005 and ADR-008 do not define it yet. It is an architecture addition and is listed in "Open items" for an ADR edit.
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
| Process Properties - firefox - PID 4321                          [pin] [x]   | 32  (UI-PWN-001)
+------------------------------------------------------------------------------+
| General | Threads [97] | File descriptors [412] | Memory maps [210] | Security | Environment [51] | Strings |  36  (UI-PWN-004)
+------------------------------------------------------------------------------+
| [search] Filter                                          [x]  | tab tools    |  32  (UI-PWN-005)
+------------------------------------------------------------------------------+
| tab body (window_bg), padding sp5 (12) left/right, sp4 (8) top/bottom       |
|                                                                              |
+------------------------------------------------------------------------------+
  default 900 x 640, minimum 640 x 420
```

  - Default size 900 × 640 [spec PROC-077]; minimum 640 × 420 [chosen]. Opened centred on the main window's output (`Bounds::centered(None, ..)`); position is never saved (ADR-010).
  - Title: `Process Properties - {name} - PID {pid}` where `{name}` is the PROC-007 display name of the process (not the app name). When the process has exited: `Process Properties - {name} - PID {pid} (not running)` [spec].
- **States:**
  - Loading (first detail payload not yet arrived, at most one sweep): title already set from the snapshot record; each tab shows its own loading state (UI-PWN-006 to UI-PWN-018).
  - Exited (`ProcessKey` no longer in `Snapshot.processes` and `ProcFlags::EXITED` seen, or the detail payload has validity `Missing`): title gains " (not running)"; every value keeps its last shown text (the window keeps the last `Arc<TargetDetail>` and the last `ProcessRecord` it rendered); every action control (Set Affinity link, Verify against package, Threads row actions, Strings Scan, Environment Refresh, Grant access buttons) is disabled with tooltip "The process is not running."; a 24 px banner appears under the tab strip: background `status.warning` at 15% alpha, 1 px bottom border `status.warning` at 40% alpha, Lucide `CircleOff` 14 px `status.warning`, text `t_sm` `text_primary` "This process has exited. Values are frozen at the last update." [chosen]. The window does not close by itself. Demand for the key is removed.
  - Error opening (target exited between click and open, or `/proc/<pid>` cannot be opened): the window is not created; the main window shows the toast "Could not open properties." (Error type) [spec PROC-071].
  - Cap: a ninth process Properties window is not created; the source window shows the toast "Too many process properties windows are already open" (Warning type) [spec]. App Properties windows have their own cap of 8 (UI-PWN-019).
  - Already open for the same `ProcessKey`: the existing window is activated (`window.activate_window()`) and its active tab is not changed [spec].
- **Interaction:**
  - Opened by `OpenProperties` (Enter, Alt+Enter, Ctrl+I on a process row, D-008; the row menu item "Properties"; double-click is not bound, because TaskSlinger double-click expands). The action carries the captured `ProcessKey`.
  - Keys inside the window (context `PropsWindow`): Ctrl+Tab and Ctrl+PgDn next tab, Ctrl+Shift+Tab and Ctrl+PgUp previous tab (`PropsNextTab`, `PropsPrevTab`); Alt+1 to Alt+7 select tab 1 to 7 (`PropsSelectTab(n)`); Ctrl+F focuses the active tab's filter (`FocusFilter`, same action as table b); F5 refreshes the active tab now (`RefreshNow`, sends `Command::DetailRefresh`); Esc per UI-PWN-001; Ctrl+W closes.
  - Initial focus: the active tab's first focusable element (the table for table tabs, the filter for General and Security).
  - Toasts inside the window appear at the top centre of this window, 8 px under the tab strip (APP-044 rule applied per window).
- **Data binding:** `ProcessKey` captured at open. Reads `Snapshot.processes` (bit `PROCESSES`) for the record, `Snapshot.details` (bit `DETAILS`) for tab payloads, `Snapshot.windows` (bit `WINDOWS`) for the General window block. Registers `DetailDemand { target: Process(key), tab: active }` (UI-PWN-002). The registry of open windows is `PropsRegistry` (a GPUI `Global`: `FxHashMap<ProcessKey, WindowHandle<PropsWindow>>` plus `FxHashMap<GroupKey, WindowHandle<AppPropsWindow>>`), cleaned in each window's `on_release`.
- **Build:** `crates/tl-ui/src/windows/props/mod.rs`, `pub struct PropsWindow` (Entity, root view of the window) owning: `key: ProcessKey`, `active: DetailTab`, `tabs: PropsTabs` (one lazily created entity per tab: `GeneralTab`, `ThreadsTab`, `FdsTab`, `MapsTab`, `SecurityTab`, `EnvironTab`, `StringsTab`), `filter: [Entity<InputState>; 7]` (one per tab, created in the constructor), `exited: bool`, `last_record: Option<ProcessRecord>`, `_subs: Vec<Subscription>`. Each tab body is embedded with `.cached(StyleRefinement::default().size_full())`, except table bodies (they own scrolling, ADR-009).
- **Performance:** a snapshot install costs one record lookup by `ProcessKey` (binary search in `records`, sorted by key) and one `Arc` pointer compare per visible tab. Only the active tab renders. No `format!` in render: titles and counts are cached `SharedString`s rebuilt only when their number changes.
- **Accessibility:** the window title names the process, so the window list and the taskbar tooltip identify it. Every tab is reachable by keyboard (Alt+1 to Alt+7 and Ctrl+Tab), because kit tabs are not focusable (widget research section 1).
- **Tests:** `props_open_focuses_existing_window` (open twice for one key, assert one window and that it became active); `props_ninth_window_shows_toast` (open 9, assert 8 windows and the toast text); `props_exited_freezes_values` (remove the key from the fake snapshot, assert title suffix, banner text, values unchanged, Scan button disabled); `props_open_failure_toast` (key missing at open, assert no window and toast "Could not open properties.").
- **Phase:** P2.
- **Decided here:** minimum size 640 × 420; exited banner text and look; Alt+1..7 and Ctrl+PgUp/PgDn tab keys; double-click does not open Properties.

### UI-PWN-004 Properties tab strip with counts

- **Implements:** PROC-077 (tab labels and counts).
- **Anatomy:**

```text
| General | Threads [97] | File descriptors [412] | Memory maps [210] | Security | Environment [51] | Strings |
  <------ equal widths: (window width - 2 x sp5) / 7, each tab 36 high ------>
  2 px separators between tabs, colour `divider`
```

  - Height 36 [measured 38, rounded down to 36]. The kit has no 36 px size for this variant, so the strip is custom (see Build).
  - Seven tabs of equal width [measured: TaskSlinger's six tabs are equal cells 166 logical px wide in a 994 px panel]. Separator 2 px `divider` [measured].
  - Label: `t_base` (13), regular 400, centred, `text_secondary` on inactive tabs, `text_primary` medium 500 on the active tab. Count suffix: one space then `[n]` in the same style, with `n` as a count (thin-space grouping, table d), for example `File descriptors [1 204]`.
  - Background: inactive `tab_bg`; hover `tab_hover`; active `window_bg` (the active tab joins the body) [measured].
  - When the strip is narrower than the sum of label widths, labels truncate with an ellipsis and keep the count: "File descr… [412]" [chosen].
  - Counts shown: Threads (thread count), File descriptors (fd count after the "Hide unnamed handles" filter), Memory maps (number of backing-file nodes after "Shared libraries only"), Environment (variable count). General, Security and Strings show no count [spec]. A count appears only once the tab's payload or the process record has it; before that the label has no suffix (no "[…]" placeholder) [chosen].
  - Counts for Threads use `ProcessRecord.threads` (live). Counts for the other tabs come from the last payload of that tab; a tab never loaded shows no count.
  - With server decorations, the pin button (UI-PWN-001) sits at the right end of this strip, 28 × 28, and the tabs share the remaining width.
- **States:** default, hover (`tab_hover`), pressed (`tab_hover` with L − 0.04), active (as above), disabled: never (all tabs are always available; a tab without access shows its own access state inside the body). Keyboard focus does not land on tabs (kit limitation); the active tab is changed by keys (UI-PWN-003).
- **Interaction:** left click selects the tab (`PropsSelectTab(n)`). Middle and right click do nothing. Tab switch has no animation (TaskSlinger's strip has none) [chosen]; reduced motion is therefore unaffected.
- **Data binding:** reads the counts above; re-renders only when one of the seven count strings changes (the strip is its own cached view `PropsTabStrip` with the counts as `[Option<u32>; 7]`, compared on each update).
- **Build:** custom element in `props/mod.rs` built from `h_flex()` children with `flex_1()` and `min_w_0()`, one `div().id(("props-tab", ix))` per tab with `.on_click`. It is not the kit `TabBar`, because the kit tabs have no 36 px size for this variant and no equal-width mode. The foundation's `TlTabStrip` is used if it offers equal widths and 36 px; otherwise this custom strip is `TlTabStrip`'s equal-width variant and the foundation bead adds it.
- **Performance:** seven shaped labels, re-shaped only when a count changes. No per-frame work.
- **Accessibility:** each tab has the tooltip `{label} (Alt+{n})` after 500 ms.
- **Tests:** `props_tab_counts_format` (counts 1204 and 51 render as "File descriptors [1 204]" and "Environment [51]"); `props_tab_no_count_before_load`.
- **Phase:** P2.
- **Decided here:** 36 px height; equal widths; truncation keeps the count; no count placeholder; no tab animation.

### UI-PWN-005 Per-tab filter bar and refresh behaviour

- **Implements:** PROC-077 (filter per tab, empty state, refresh at `processes.interval_ms`), PROC-084 (filter applies to Value).
- **Anatomy:**

```text
+------------------------------------------------------------------------------+
| [Search 14] Filter                                         [x] | tab tools    |  32 high, padding sp5 / sp3
+------------------------------------------------------------------------------+
```

  - Bar height 32, background `window_bg`, bottom border 1 px `divider`. Padding left and right `sp5` (12), top and bottom `sp2` (4).
  - Filter: TlInput small (24 high), width 280 [chosen], prefix Lucide `Search` 14 px `text_secondary`, placeholder "Filter" in `text_secondary`, cleanable (clear button Lucide `X` 12 px appears when there is text). Background `input`, focused `input_focus` with a 1 px `accent` border.
  - Tab tools sit right-aligned in the same bar, gap `sp4` (8): File descriptors: checkbox "Hide unnamed handles"; Memory maps: checkbox "Shared libraries only"; Environment: "Environment at process start" note and Refresh button; Strings: the scan controls (UI-PWN-017). General, Threads and Security have none.
- **States:**
  - Empty filter: all rows. Non-empty: rows whose searched text contains the filter, case-insensitive (Unicode simple lowercase), substring only, no globs.
  - Nothing matches: the body shows the empty state "No properties match the current filter." centred, `t_base`, `text_secondary`, 48 px from the top of the body [spec text]. For table tabs, the table keeps its header and shows this text in place of rows.
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
- **Build:** `props/filter.rs`, `pub struct PropsFilter` helper: owns the debounce `Task` and the generation counter; one per tab. The input is the foundation's `TlInput` over a kit `InputState` (read with `text()`, not `value()`, widget research section 7).
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
  - Field row: height 26 [measured 25.8], independent of `row_height_pct` (key/value grids are not tables) but multiplied by the UI scale. Label: `t_base` regular, `text_secondary`, ending with ":" [measured], left-aligned, single line. Value: `t_base` regular, `text_primary`, left-aligned, single line, ellipsis at the end, tabular figures for numbers.
  - Label width per column: the widest shaped label in that column plus `sp5` (12), clamped to 96..150 px, computed once per UI scale and cached [chosen; TaskSlinger uses 126 px for all columns, measured, but tasklens labels are longer].
  - Command line block: full width under the panels, `sp4` (8) above it, background `list_bg`, radius `r_sm`, padding `sp4`. Header line 26 high: label "Command line" `t_base` medium `text_secondary`, and a TlButton xsmall ghost "Copy" with Lucide `Copy` 12 px at the right. Body: the full command line (up to 32 KiB, PROC-078, R-008), arguments joined by single spaces, wrapped at any character, `t_sm` (12) monospace off (Noto Sans), `text_primary`, line height 18. It is not in column 1, so long command lines do not stretch a column [chosen; the spec lists it in column 1].
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
- **Accessibility:** values that are truncated show their full text in a tooltip after 500 ms. Hash and path values are selectable through Copy only (no text selection widget) [chosen].
- **Tests:** unit tests in `props/general.rs`: `fmt_priority_other_and_fifo`, `fmt_affinity_all_and_list_threads_differ`, `fmt_user_with_euid`, `fmt_aslr_personality`, `fmt_seccomp_filters`; render tests: `general_three_columns_at_900`, `general_single_column_below_840`, `general_restricted_word_and_tooltip`, `general_cmdline_not_reshaped_on_sweep` (shape counter stays the same across 10 sweeps with an unchanged command line).
- **Phase:** P2.
- **Decided here:** 26 px rows not scaled by row height; label width rule; 840 px breakpoint; command line as a full-width block; Autogroup and Hardening summary as separate rows; Net I/O wording; architecture and hardening value words.

### UI-PWN-007 General tab: copy on click, links and keyboard

- **Implements:** PROC-078 (click copies a value; PPID link; Executable link; Affinity opens Set Affinity), PROC-071, PROC-073, APP-044.
- **Anatomy:** a field row has a value hit area covering the value cell (from the label's right edge to the panel's right padding), 26 high.
  - Plain value hover: background `row_hover` on the value cell, radius `r_sm`, cursor arrow.
  - Link value: text `text_primary` with a 1 px underline in `text_secondary` [measured: TaskSlinger underlines PPID and Executable in the value colour]; hover: text and underline `accent`, cursor pointing hand.
  - Keyboard cursor: 1 px `accent` outline around the whole field row, radius `r_sm`, inset 1 px.
- **States:** default, hover (above), pressed (value background `row_selected` for 120 ms (`m_fast`), none with reduced motion), focused (keyboard cursor), disabled (exited: links plain text `text_secondary`, copying still works).
- **Interaction:**
  - Left click on a plain or hash value: copies the full value (for hashes the full hex string; for truncated values the untruncated text) with `cx.write_to_clipboard`; toast "Process property value copied." (Success, 5 s) [spec]. Clipboard failure: "Failed to copy to clipboard" (Error, 8 s).
  - Left click on a link: PPID dispatches `GoToProcess { key: parent_key }` (the PROC-071 entry point, toast as defined there, the main window is activated); Executable dispatches `OpenFileLocation { key }` (PROC-073, toast "Opening file location"); Unit dispatches `GoToService { unit }`; Affinity dispatches `OpenSetAffinity { targets: [key] }` (UI-PWN-021).
  - Right click on any field opens a TlMenu: "Copy value" (`CopyFieldValue`), "Copy line" (`CopyFieldLine`, copies `Label: value`), "Copy all fields" (`CopyAllFields`, every visible field of all three columns as `Label: value` lines in column order, then `Command line: …`). Toasts: "Process property value copied." for Copy value; "Copied to clipboard" for the other two.
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
  - Verify button: TlButton xsmall (20 high) default variant, label "Verify against package", aligned with the value column, in its own 26 px row.
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
| Window title:  Mozilla Firefox — Start page  |
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
