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

  - Height 36 [measured 38, rounded to the kit `TabVariant::Tab` md height... no kit size gives 36 for this variant, so the strip is custom; see Build].
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
