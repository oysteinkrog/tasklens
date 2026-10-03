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
