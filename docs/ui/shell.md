# tasklens UI spec: application shell

This file specifies the main window shell of tasklens down to the pixel, key and Action. It covers the window frame, the client-drawn title bar, the toolbar, the main tab strip, every menu, the status chip, the filter box, the Settings window, theme management, the shell's secondary windows, toasts, startup, single instance, the main tray icon, the focus model, the keyboard map and runtime scale changes.

ID prefix: UI-SHL. Phase labels follow R-072 (P0 to P4).

## Inputs and precedence

- Binding, in order: `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-103, canonical tables a to d), `docs/ARCHITECTURE.md` (ADR-009 and ADR-010 above all), then the merged specs `docs/spec/merged/sys-app.md` (APP entries), `perf.md` and `proc.md` (their shell parts).
- Tokens and components come from `docs/ui/foundation.md`. This file never defines a colour or token value. It names `TlTheme` fields (`titlebar`, `tab_bg`, `accent` and so on), status colours (`status.danger` and so on), spacing `sp1` to `sp8`, text `t_xs` to `t_xl`, radii `r_sm` and `r_md`, motion `m_fast`, `m_base`, `m_slow` and `ease_out_cubic`.
- Components are the foundation's `Tl*` types: `TlButton`, `TlIconButton`, `TlToggleButton`, `TlMenu`, `TlMenuItem`, `TlToast`, `TlDialog`, `TlWindowFrame`, `TlTooltip`, `TlInput`, `TlCheckbox`, `TlSwitch`, `TlSlider`, `TlKbd`, `TlSkeleton`, `TlFocusRing`. Where the foundation names one differently, its name wins and this file is read with that name.

## Conventions used in every entry

- **Sizes** are logical px at `ui_scale_pct = 100`. Every size in this file is multiplied by `ui_scale = ui_scale_pct / 100` unless the entry says "fixed". Hairlines (1 px dividers, 1 px borders) are 1 physical px and are never scaled.
- **Provenance tags** after a value: `[M]` measured from a reference image, `[S]` taken from a spec, ADR, R or D entry, `[C]` chosen here (also listed under "Decided here" at the end).
- **Measurement basis.** `docs/taskslinger-reference/processes.png`, `performance.png` and `information.png` are 2720 × 1680 physical px of a 1360 × 840 logical window at scale 2.0. A measured value is the physical span divided by 2, rounded to the nearest logical px. Colours sampled from those images (for example the title bar `#27292B`, tab strip `#202224`, header `#2B3134`, accent `#0077A6`) only identify which `TlTheme` slot a region uses; values belong to the foundation file.
- **Actions.** Every command is one GPUI `Action` in `crates/tl-ui/src/actions.rs`, namespace `tasklens`. Menu items, toolbar buttons and key bindings dispatch the same Action (widgets rule 10). Data-carrying actions derive `Action` with `serde::Deserialize` and `schemars::JsonSchema` as GPUI requires.
- **Settings.** `TlSettings` is the GPUI `Global` that holds the parsed `settings.toml` (`Settings`), the parsed `state.toml` (`UiState`) and one `u64` version per settings section (`general`, `theme`, `appearance`, `motion`, `units`, `processes`, `tables`, `properties`, `performance`, `fans`, `graphs`, `history`, `window`, `tray`, `startup`, `network`, `privilege`, `rendering`, `services`, `lowmem`, `debug`). A write goes through `TlSettings::update(cx, |s| ..)`, which bumps the touched section's version, calls the tl-app persist callback (500 ms debounce, ADR-011, APP-033) and, if an `EngineSettings` field changed, calls `EngineClient::set_settings`. Views observe the global with `cx.observe_global::<TlSettings>` and call `cx.notify()` only if a section they draw changed version.
- **Two generic setting actions** cover every plain toggle and choice: `ToggleSetting(SettingKey)` flips a bool key; `SetSetting { key: SettingKey, value: SettingValue }` writes a choice. `SettingKey` is an enum with one variant per key of canonical table (a) (for example `SettingKey::MotionAnimations` for `motion.animations`). `SettingValue` is `Bool(bool) | Int(i64) | Str(SharedString) | StrList(Arc<[SharedString]>)`. Commands that need extra work (a toast, a confirm window, a restart notice, an engine command) have their own named Action, listed in each entry.
- **Snapshot bits.** `ComponentMask` has one bit per `Snapshot` field in declaration order, with constants in `hub.rs`: `SYSTEM`, `SERIES`, `PROCESSES`, `TABLE`, `CONNECTIONS`, `SERVICES`, `STARTUP`, `INVENTORY`, `DEVICES`, `HEALTH`, `WINDOWS`.
- **Strings** go through Fluent message keys (APP-043). This file gives the English text. Message keys are `shell-<entry>-<name>`, for example `shell-titlebar-pin-on`.
- **Reduced motion** means `App::reduce_motion()` is true (gpui-kit `base::reduce_motion` reads the portal `org.freedesktop.appearance reduced-motion`). "Animations on" means `motion.animations && !cx.reduce_motion()` (R-017).
- **Toasts** in this file are pushed with `TlToast` through `window.push_notification`. Toast placement is UI-SHL-100.

## Source files this spec creates (crates/tl-ui/src/)

| File | Entity or item | Entries |
|---|---|---|
| `window/main.rs` | `MainWindow` (Entity, one per main window) | 001, 002, 007 |
| `window/titlebar.rs` | `TitleBarView` (Entity, cached) | 003 to 006 |
| `window/menubar.rs` | `MenuBar` (Entity) and the menu builders | 030 to 042 |
| `window/toolbar.rs` | `Toolbar` (RenderOnce, inside `TitleBarView`) | 010 to 014 |
| `window/tabstrip.rs` | `TabStrip` (Entity, cached) | 020 to 025 |
| `window/filter.rs` | `FilterBox` (Entity) | 024 |
| `window/status.rs` | `StatusChip` (Entity, cached) | 050, 051 |
| `window/secondary.rs` | `SecondaryWindow<V>` frame helper | 090, 091 |
| `windows/{settings.rs, theme_editor.rs, share_theme.rs, about.rs, crash_report.rs, setup.rs, shortcuts.rs, public_ip.rs, collector_health.rs, confirm.rs}` | one Entity each | 060 to 099 |
| `actions.rs` | every Action and `bind_keys` | 120 |
| `focus.rs` | `FocusModel` helpers | 121 to 124 |
| `settings_global.rs` | `TlSettings` Global | conventions |
| `tray_bridge.rs` | glue to `tl-tray` (tl-tray owns the SNI) | 113 |

---

## 1. Main window, frame and title bar

### UI-SHL-001 Main window layout and resizing rules

- **Implements**: APP-001, APP-002, APP-010, APP-027, APP-029, R-022, ADR-009 (MainWindow), ADR-010.
- **Anatomy** (client decorations, default; sizes at 100%):

```text
x=0                                                                       x=W
+--------------------------------------------------------------------------+
| TITLE BAR  h=34 [S][M33]  bg=titlebar                                    |  UI-SHL-003
| [ic][File|View|Settings|Help][tb tb tb tb]   ...drag area...   [pin][_][□][x] |
+--------------------------------------------------------------------------+
| TAB STRIP  h=30 [M]  bg=tab_bg                                           |  UI-SHL-020
| [Processes ⟲]  Performance  Services  Startup Apps  Connections  Information   [incl][Filter: ____] |
+==========================================================================+ <- 2 px accent line
| PAGE AREA  h = H - 34 - 30   (active page only, ADR-009)                 |
|                                                                          |
|                                                                          |
| [STATUS CHIP] (floats, bottom-left, h=26)                                |  UI-SHL-050
+--------------------------------------------------------------------------+
```

  Parts:
  - Title bar: full width, height 34 [S APP-001; M 33]. UI-SHL-003.
  - Tab strip: full width, height 30 [M 29.5]. Includes the 2 px accent line at its bottom edge. UI-SHL-020.
  - Page area: fills the rest. Background `window_bg`. Each page owns its own content and padding.
  - Status chip: absolutely positioned over the page area, bottom-left, UI-SHL-050.
  - Toast layer, menu layer, tooltip layer: gpui-kit Root overlays (UI-SHL-100).
  - Window border and shadow: gpui-kit `WindowBorder`, applied by Root under client decorations: 1 physical px border, shadow inset 20 px on untiled sides [S widgets §2], 4 px resize hit band [S].
- **Resizing rules**:
  - Minimum window size 800 × 500 [S R-022]. GPUI enforces it through `window_min_size`.
  - Title bar and tab strip keep their height at every window size. Only the page area grows or shrinks vertically.
  - Horizontal shrink order in the title bar (UI-SHL-003): the drag area shrinks first, down to 24 px; then the toolbar buttons stay; menus never collapse. At 800 px the title bar content is: icon 16 + 12 padding + menus about 262 + separator and toolbar 4 × 32 + caption buttons 4 × 36 = about 570 px, so it always fits [C].
  - Horizontal shrink order in the tab strip (UI-SHL-020): the filter box keeps its width; tabs that do not fit move into the "»" overflow menu (UI-SHL-022).
  - Maximized or tiled (`Decorations::Client { tiling }` with a tiled side): `WindowBorder` drops the shadow and resize band on that side; the layout is otherwise identical.
- **States**: active window, inactive window (UI-SHL-007), maximized (caption maximize icon becomes restore, UI-SHL-005), fullscreen (not offered; KWin can still force it, and layout is unchanged).
- **Interaction**: resize from the 4 px band on every edge and corner (`start_window_resize(edge)`), with the edge cursor (`ResizeLeftRight`, `ResizeUpDown`, `ResizeUpLeftDownRight`, `ResizeUpRightDownLeft`).
- **Data binding**: `UiState.window.{width,height,maximized}` (state.toml). `MainWindow` reads `TlSettings` sections `appearance`, `general`, `window`.
- **Build**: `MainWindow` is an Entity in `window/main.rs`. Its `render` returns `v_flex().size_full().bg(theme.window_bg)` with three children: `self.titlebar.clone()` (cached Entity, `.cached(StyleRefinement::default().w_full().h(px(34.0 * s)))`), `self.tabstrip.clone()` (cached, height 30 × s), and the page container `div().relative().flex_1().min_h_0()` that holds the active page Entity (`.cached(size_full)`, ADR-009) and the `StatusChip` Entity absolutely positioned. Page entities are created on first visit and kept (ADR-009).
- **Performance**: a snapshot install notifies only the views whose components changed. `MainWindow::render` itself is re-run only on tab change, settings change or window activation change; it does no per-frame work and allocates nothing (all children are retained Entities).
- **Accessibility**: window title (for KWin task switcher) is "tasklens" on the main window [C]; secondary windows use their APP-045 titles.
- **Tests**: `main_window_layout_regions` (TestAppContext, window 1280 × 800): title bar bounds height 34, tab strip 30, page area 736; at `ui_scale_pct = 150` they are 51, 45 and 704. `main_window_min_size` asserts the `WindowOptions.window_min_size` is 800 × 500.
- **Phase**: P1.
- **Decided here**: shrink order; main window title "tasklens".

### UI-SHL-002 Window creation, size and geometry restore

- **Implements**: APP-001 (mechanism), APP-010, R-022, ADR-010 (Decorations, Window size persistence, zed#46378).
- **Behaviour**:
  - Opened once by `TasklensUi::init` through `gpui_kit::open_window(opts, cx, |w, cx| cx.new(|cx| MainWindow::new(w, cx)))`.
  - `opts` is exactly:

```rust
WindowOptions {
    window_decorations: Some(match settings.appearance.decorations {
        Decorations::Client => WindowDecorations::Client,
        Decorations::Server => WindowDecorations::Server,
    }),
    window_background: WindowBackgroundAppearance::Opaque,
    window_min_size: Some(size(px(800.), px(500.))),
    app_id: Some("io.github.oysteinkrog.Tasklens".into()),
    window_bounds: Some(restored_bounds),
    focus: !start_hidden,
    show: !start_hidden,
    ..TitleBar::window_options()
}
```

  - `restored_bounds`: `WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size })` with `size = (max(state.width, 800), max(state.height, 500))`, or `WindowBounds::Maximized(..)` with the same size when `state.maximized`. The origin is ignored by Wayland; tasklens never sets a display (zed#46378).
  - Save: `cx.observe_window_bounds(window, ..)` records `window.window_bounds()`; a 500 ms debounce task writes `[window] width, height, maximized` to `UiState` (logical px, rounded to integers). On close the latest value is written without waiting. Position is never saved [S].
  - `start_hidden` is true when `--start-minimized` was given and a tray setting is on (APP-048). If `--start-minimized` is given and no tray setting is on, the window opens shown and `window.minimize_window()` runs after the first frame callback [S APP-048].
- **States**: none visible.
- **Data binding**: reads `appearance.decorations` once at startup (a change needs a restart, UI-SHL-006). Writes `UiState.window`.
- **Build**: `window/main.rs` function `open_main_window(cx, settings, state) -> Result<WindowHandle<MainWindow>>`.
- **Performance**: bounds observation is O(1) per configure event; the debounce task is replaced, not stacked (keep one `Task<()>` field and overwrite it).
- **Tests**: `geometry_restore_clamps` (unit): state 640 × 400 gives bounds 800 × 500; `geometry_save_debounced` (TestAppContext): three bound changes within 200 ms write `UiState` once, 500 ms after the last.
- **Phase**: P1.

### UI-SHL-003 Client-drawn title bar layout

- **Implements**: APP-001, APP-002, APP-008, D-006, R-031, PERF-014, PROC-016.
- **Anatomy** (height 34, bg `titlebar`; x positions measured on `processes.png` at 1360 px width):

```text
|12|16|8| File |1| View |1| Settings |1| Help |1|4|[◈][▦][☰][+]|  drag area (flex)  |[pin][ — ][ □ ][ × ]|
 pad ic gap  menu buttons h=26, px=16, t_md     sep  toolbar 30×28, gap 2          caption 36×34 each
```

  Part list, left to right:
  1. Left padding 12 [S gpui-kit Linux padding].
  2. App icon: 16 × 16 [S APP-002], app SVG `assets/icons/tasklens.svg`, vertically centred. Not interactive except as drag area (mouse-down propagates). Measured reference has no icon (TaskSlinger draws none); APP-002 requires it [S wins].
  3. Gap 8 (`sp4`).
  4. Menu buttons "File", "View", "Settings", "Help" (and "Debug" when enabled, UI-SHL-042): UI-SHL-030. Each is 26 tall, padding-x 16 (`sp6`) [M: File, View, Settings, Help centres at 35, 105, 184, 262; separators at 71, 141, 228, 297], label `t_md` regular in `text_primary` [C from M: cap height 11 logical px].
  5. Between two menu buttons, and after the last one: a vertical separator 1 physical px wide, 16 tall [M], colour `divider`, vertically centred.
  6. Gap 4 (`sp2`) [M 5].
  7. Toolbar (UI-SHL-010): icon buttons 30 × 28 [C from M pitch 31], gap 2 (`sp1`), icon 16 [M 15].
  8. Drag area: `flex_1()`, minimum 24, empty. This is the only purpose-built drag region; every other part that does not stop mouse-down also drags (icon, separators, padding).
  9. Caption buttons (UI-SHL-005): pin, minimize, maximize or restore, close. Each 36 wide [M: centres at 1234, 1270, 1306, 1342] × 34 tall, flush to the right edge.
- **States**:
  - Window active: as above.
  - Window inactive: menu labels and toolbar icons in `text_secondary`; title bar background unchanged (UI-SHL-007).
  - Debug menu present: "Debug" button after "Help", with its own separator.
- **Interaction**: see UI-SHL-004 (drag, double-click, window menu), UI-SHL-030 (menus), UI-SHL-010 (toolbar), UI-SHL-005 (caption buttons).
- **Data binding**: `TitleBarView` re-renders when: the active tab changes (toolbar content), `TlSettings` sections `window` (pin) or `debug` change, the shield state changes (`HelperState` from `ActionTracker`), the KWin bridge availability changes (`ComponentMask::WINDOWS` validity flip only), window activation changes, or `window.is_maximized()` flips.
- **Build**: `TitleBarView` (Entity) in `window/titlebar.rs`, rendered through gpui-kit `TitleBar::new()` for drag, double-click and window-menu handling, with `.child(..)` for parts 1 to 8. The kit's own window controls are suppressed: `TitleBar` draws controls only under `Decorations::Client`, so tasklens renders its own caption buttons (part 9) as the last child and sets the kit controls off by using `TitleBar::new().controls(false)` if that builder exists, or otherwise by wrapping a plain `h_flex` that copies the kit's drag logic (`title_bar.rs:326-396`: `should_move` state, `start_window_move` on next mouse move, `zoom_window` on double-click, `show_window_menu(pos)` on right-click). The implementer checks `title_bar.rs` in gpui-component 0.7.0; if there is no switch to hide the kit controls, use the copied-logic `h_flex` (decided: own drag code, about 60 lines, `window/titlebar.rs::drag_handlers`).
  - Embedded in `MainWindow` with `.cached(..)`.
  - Height constant `TITLEBAR_H = 34.0`, multiplied by `ui_scale`.
- **Performance**: no snapshot subscription at all except the `WINDOWS` validity flip (checked in the hub subscriber with one comparison). Renders a fixed number of elements (about 20). Strings are `SharedString` constants created once.
- **Accessibility**: every toolbar and caption button has an accessible name equal to its tooltip text. Keyboard access to menus is F10 and Alt+letter (UI-SHL-030). The title bar has no Tab stops (TaskSlinger has none; toolbar buttons are reachable by their Actions and menus) [C].
- **Tests**: `titlebar_parts_order` (render test): children in order icon, File, View, Settings, Help, toolbar, drag, pin, minimize, maximize, close. `titlebar_toolbar_by_tab`: Processes shows 4 toolbar buttons (shield and picker only when available), Performance 2, and the drag area width changes accordingly (no reserved space, PROC-016).
- **Phase**: P1 (pin P2, shield P3, picker P3).
- **Decided here**: menu label size `t_md`; toolbar button 30 × 28 with gap 2; caption buttons 36 wide; title bar not a Tab stop; own drag code when kit controls cannot be hidden.

### UI-SHL-004 Title bar drag region, double-click and window menu

- **Implements**: APP-001, APP-002, ADR-010 (CSD checklist).
- **Behaviour**:
  - Left mouse-down on any title bar part that does not stop propagation (icon, padding, separators, drag area) records `should_move = true`. The next mouse move calls `window.start_window_move()`; KWin then owns the drag (snapping, quick tiling to edges).
  - Menu buttons, toolbar buttons and caption buttons call `cx.stop_propagation()` and `window.prevent_default()` on mouse-down, so they never start a drag.
  - Double-click (click count 2) on a drag part calls `window.zoom_window()` (maximize or restore).
  - Right-click on a drag part calls `window.show_window_menu(event.position)`, the KWin window menu.
  - Right-click on a menu, toolbar or caption button does nothing [C].
  - Middle-click: nothing [C].
- **States**: the cursor stays the default arrow over the title bar (widgets rule 7).
- **Build**: `window/titlebar.rs::drag_handlers` (see UI-SHL-003).
- **Tests**: `titlebar_double_click_zooms` (TestAppContext with the test platform's window): simulate two clicks on the drag area; assert `zoom_window` called once. `titlebar_button_no_drag`: mouse-down plus move on the File button does not call `start_window_move`.
- **Phase**: P1. The phase-0 CSD checklist (ADR-010) verifies drag, double-click, restore, resize from all edges and corners, right-click window menu and tiling on KWin; any failure switches the default to server decorations (UI-SHL-006).

### UI-SHL-005 Caption buttons (pin, minimize, maximize or restore, close)

- **Implements**: APP-008, APP-009, APP-047 (close behaviour), R-073, PROC-056.
- **Anatomy**: four buttons, each 36 × 34 [M], no gap, flush right. Icons 16 × 16, centred, colour `text_primary` (window active) or `text_secondary` (inactive).

  | Button | Icon (off) | Icon (on) | Tooltip | Action |
  |---|---|---|---|---|
  | Pin | Lucide `pin-off` [M: crossed pin] | Lucide `pin`, drawn in `text_selected_primary` on an `accent` fill | "Keep on top" / "Keep on top (on)" [C]; disabled: "Not supported by this desktop" [S] | `ToggleAlwaysOnTop` |
  | Minimize | Lucide `minus` | | "Minimize" | `MinimizeWindow` |
  | Maximize | Lucide `square` | Lucide `copy` (restore look) when maximized | "Maximize" / "Restore" | `ZoomWindow` |
  | Close | Lucide `x` | | "Close" | `CloseWindow` |

- **States**:
  - Default: transparent background.
  - Hover: background `tab_hover` (pin, minimize, maximize); close hover background `status.danger`, icon `text_selected_primary` [C, kit uses `danger`].
  - Pressed: hover background with 0.8 opacity [C].
  - Pin on: fill `accent` across the full 36 × 34 cell [S APP-008 "accent fill when on"]; hover on a pinned pin keeps the accent fill and lightens it with the foundation's hover overlay.
  - Pin disabled (no KWin bridge and not X11): icon at the foundation's disabled opacity, no hover, tooltip "Not supported by this desktop".
  - Minimize and maximize are hidden (not disabled) when `window.window_controls().minimize` or `.maximize` is false [S APP-008]; the remaining buttons stay flush right.
  - Keyboard focus: caption buttons are not Tab stops [C].
- **Interaction**:
  - Pin: `ToggleAlwaysOnTop` flips `window.always_on_top` and sends `Command::KWin(KWinCommand::KeepAbove { pid: self_pid, caption: None, on })` on KDE; on X11 it sends `_NET_WM_STATE_ABOVE` through the tl-engine X11 helper (APP-009). Settings write is immediate. No toast [C].
  - Minimize: `window.minimize_window()`, or, when `tray.minimize_to_tray` is on and the tray is up, hide the window (UI-SHL-113).
  - Maximize: `window.zoom_window()`.
  - Close: dispatches `CloseWindow`, which follows the close behaviour in UI-SHL-113 (quit, hide to tray, or hide while detached graphs are open).
- **Data binding**: `window.always_on_top`; pin availability from `Snapshot.windows.header.validity` (KDE) or the X11 backend flag. `ComponentMask::WINDOWS` triggers a re-render only when that availability flips.
- **Build**: `window/titlebar.rs::caption_buttons()`, each a `TlIconButton` variant "caption" (foundation) or, if the foundation has no such variant, `div().id(..).w(36).h(34)` with `.on_click` and `.hover(..)`.
- **Performance**: static elements; no per-frame work.
- **Accessibility**: tooltip text doubles as accessible name; the pin tooltip includes the state.
- **Tests**: `pin_toggles_setting` (TestAppContext, fake engine): click pin; `window.always_on_top == true` and the fake engine received `KWin(KeepAbove{on:true})`. `caption_hides_unsupported`: fake `window_controls` with `maximize: false` renders three buttons.
- **Phase**: P1 (minimize, maximize, close); P2 (pin).
- **Decided here**: pin tooltip text "Keep on top"; hover colours; caption buttons not Tab stops.

### UI-SHL-006 Server-decorations variant ("Use system title bar")

- **Implements**: APP-001 (server mode), R-021, ADR-010 (predetermined fallback), APP-023 (restart rule).
- **Conflict note**: APP-001 puts menus and toolbar in a 28 px row under the KWin title bar, then the tab strip. ADR-010 says the tab strip is the first row. APP-001 is the later, more specific text and keeps the menus reachable; this entry follows APP-001 [C].
- **Anatomy** (`appearance.decorations = "server"`, or a compositor without xdg-decoration that forces server):

```text
+--------------------------------------------------------------------------+
| (KWin title bar, drawn by the compositor)                                |
+--------------------------------------------------------------------------+
| MENU ROW h=28 bg=titlebar                                                |
|12|16|8| File | View | Settings | Help |4| [toolbar] ......(no drag)...... [pin] |
+--------------------------------------------------------------------------+
| TAB STRIP h=30 (UI-SHL-020)                                              |
```

  - Menu row height 28 [S]. Same parts as UI-SHL-003 items 1 to 7, then a flex spacer, then only the pin button (36 × 28) [S "pin stays in that row"].
  - Menu buttons are 24 tall in this row [C] (28 minus 2 × 2 padding).
  - No drag, double-click or window-menu handling in this row (KWin's own title bar does that).
- **Behaviour**:
  - Setting change in the Settings window (UI-SHL-062) writes `appearance.decorations` and pushes the Info toast "Restart tasklens to apply the title bar change" [S R-083]. The running window keeps its current mode until restart.
  - If the compositor ignores the request (GNOME without xdg-decoration gives client mode), GPUI reports `Decorations::Client` and tasklens renders UI-SHL-003 regardless of the setting [S APP-001]. The layout choice reads `window.window_decorations()` on each render, never the setting.
- **Build**: `TitleBarView::render` branches on `window.window_decorations()`: `Decorations::Server` renders `menu_row()`; `Decorations::Client{..}` renders UI-SHL-003. A compositor configure event that switches mode re-renders the title bar (subscribe with `window.observe_window_appearance` is not enough; the root re-renders on configure, and `TitleBarView` is not cached across a decoration change: its cache key includes the decoration kind) [C].
- **Tests**: `server_mode_menu_row`: with test window decorations `Server`, title bar height is 28 and contains no minimize, maximize or close buttons, and contains the pin.
- **Phase**: P1.
- **Decided here**: menu row button height 24; follow APP-001 over ADR-010 on row order.

### UI-SHL-007 Active and inactive window appearance

- **Implements**: APP-027 (accent outline), ADR-010; reference video frames (`scratchpad/ts/cus_sheet_02.png`) where the accent outline is shown only in some frames.
- **Behaviour** [C, inferred from the video frames]:
  - Active window: tab strip accent outline and line in `accent` (UI-SHL-020); menu labels, toolbar icons and caption icons in `text_primary`.
  - Inactive window: accent outline and line drawn in `divider`; labels and icons in `text_secondary`. The active tab keeps its `header` fill, so the active tab stays readable.
  - Ctrl-pause releases on deactivation (UI-SHL-123).
  - `WindowBorder` dims its shadow when inactive (kit behaviour).
- **Data binding**: `window.is_window_active()`, observed with `cx.observe_window_activation(window, ..)` in `MainWindow::new`, which notifies `TitleBarView` and `TabStrip` (both read the flag in render).
- **Tests**: `inactive_window_uses_divider_outline` (render test): with `is_window_active = false`, the tab strip outline paint colour equals `theme.divider`.
- **Phase**: P1.
- **Decided here**: the whole entry is inferred from the reference video and chosen here.

---

## 2. Toolbar

### UI-SHL-010 Toolbar slot and per-tab content

- **Implements**: APP-002, R-031, PERF-014, PROC-016, APP-003, APP-004, APP-005, APP-006.
- **Anatomy**: a horizontal row inside the title bar (UI-SHL-003 part 7), height 28, buttons 30 × 28 [C from M pitch 31], gap 2 (`sp1`), vertically centred in the 34 px bar. Order, left to right [M processes.png: shield, grid, stacked bars, plus]:

  | Slot | Button | Shown on | Entry |
  |---|---|---|---|
  | 1 | Shield | every tab, from P3, only when `/usr/lib/tasklens/tl-helper` exists | UI-SHL-011 |
  | 2 | Hide system processes (toggle) | Processes tab only (including History mode) | UI-SHL-012 |
  | 3 | View mode cycler | Processes tab only; disabled in History mode | UI-SHL-013 |
  | 4 | Window picker "+" | every tab, from P3, only with the KWin bridge | UI-SHL-014 |

  - A hidden button takes no space (`.when(cond, ..)`, not `.invisible()`), so the drag area grows [S PROC-016].
  - All four use `TlIconButton` in the foundation's "toolbar" (ghost) style: transparent fill, icon 16 in `text_primary`, radius `r_sm`.
- **States** (common to all four):
  - Hover: fill `menu_hover`.
  - Pressed: fill `menu_hover` at full strength plus the foundation's pressed overlay.
  - Toggled on (slot 2 only): fill `accent` at the foundation's "selected subtle" alpha, icon `accent` [C].
  - Disabled: icon at the foundation's disabled opacity; no hover; tooltip still shows (reason text).
  - Focus: toolbar buttons are not Tab stops [C]. Keyboard users reach the same Actions through View > Process View, the Settings menu and table (b) keys.
  - Window inactive: icons `text_secondary`.
- **Interaction**: click dispatches the button's Action (below). Tooltips use `TlTooltip` with the 500 ms delay and, where the Action has a binding, the key hint (`Tooltip::action`).
- **Data binding**: `MainWindow.active_tab`, `UiState.processes.history_mode`, `processes.hide_system`, `processes.mode`, `HelperState` (UI-SHL-011), KWin availability (UI-SHL-014).
- **Build**: `Toolbar` is a `RenderOnce` struct built inside `TitleBarView::render` from a small `ToolbarModel { tab, history_mode, hide_system, mode, helper: HelperState, picker_available, picking }` that `TitleBarView` keeps up to date.
- **Performance**: at most 4 buttons; no allocations beyond the element tree; tooltip text are `SharedString` constants except the view-mode tooltip, which is one of three precomputed `SharedString`s.
- **Tests**: `toolbar_slots_per_tab` (render test): for each of the six tabs with helper installed and bridge up, assert the visible slot list: Processes [1,2,3,4], every other tab [1,4]. `toolbar_history_mode`: Processes in History mode shows slot 3 disabled with tooltip "History shows a flat list".
- **Phase**: P1 (slots 2 and 3), P3 (slots 1 and 4).
- **Decided here**: toggled-on look of slot 2; toolbar buttons not Tab stops.

### UI-SHL-011 Shield (unlock privileged actions)

- **Implements**: APP-003, APP-049 (unlock at launch), R-002, R-003, table (b) "Shift held at launch".
- **Anatomy**: `TlIconButton` 30 × 28. Icon: Lucide `shield` (locked) in `text_primary`; Lucide `shield-check` (unlocked) in `accent` [C; APP-003 names them "shield" and "shield-hi"; TaskSlinger's two-colour Windows shield ships nowhere, D-005].
- **States**:
  - Locked: `shield`, tooltip "Unlock privileged actions" [S].
  - Starting (pkexec prompt open): icon `shield` with a 2 px `accent` ring around the button [C]; button inert (`loading` semantics) until the helper answers or fails. No spinner (widgets perf note: spinners request frames).
  - Unlocked: `shield-check` in `accent`, tooltip "Privileged actions unlocked (click to lock)" [S].
  - Hidden: before P3, and when `tl-helper` is not installed [S].
- **Interaction**:
  - Click while locked: Action `UnlockPrivileged`. The UI calls `ActionTracker::start_helper_session()`, which sends the engine command that runs `pkexec /usr/lib/tasklens/tl-helper --session`. Results:
    - Success: state Unlocked, Success toast "Privileged actions unlocked" [C].
    - Cancelled by the user (pkexec exit 126): state Locked, Info toast "Authentication cancelled" [S APP-044].
    - Failure (exit 127 or other): state Locked, Error toast "Could not start the administrator helper" [C].
  - Click while unlocked: Action `LockPrivileged`, ends the session (closes the helper's stdin). State Locked, Info toast "Privileged actions locked" [C].
  - Helper idle exit (300 s) or crash: state Locked, no toast [C] (the next privileged action starts a new session).
  - Shift held at launch, or `privilege.unlock_at_launch = true`: `UnlockPrivileged` runs once after the first frame (UI-SHL-110 step 9).
- **Data binding**: `HelperState { Unavailable, Locked, Starting, Unlocked }` held by `ActionTracker` and changed by `ActionEvent`s; `TitleBarView` subscribes to `ActionTracker` events of kind `HelperStateChanged`.
- **Build**: `window/toolbar.rs::shield_button()`.
- **Accessibility**: accessible name equals the tooltip text for the current state.
- **Tests**: `shield_states` (TestAppContext with fake engine): click; fake engine replies Started; icon is `shield-check` and the toast "Privileged actions unlocked" was pushed. Replay with Cancelled; toast "Authentication cancelled".
- **Phase**: P3.
- **Decided here**: icons; starting ring; toasts "Privileged actions unlocked", "Privileged actions locked", "Could not start the administrator helper".

### UI-SHL-012 Hide system processes toggle

- **Implements**: APP-004, PROC-046, R-031.
- **Anatomy**: `TlIconButton` 30 × 28, toggle. Icon Lucide `layout-grid` [C; matches the four-square glyph measured in processes.png].
- **States**:
  - Off (`processes.hide_system = false`, showing all): plain toolbar look; tooltip "Hide system processes" [S].
  - On (hiding): toggled-on look (UI-SHL-010); tooltip "Show system processes" [S].
  - History mode: stays active and filters the History table [S].
- **Interaction**: click dispatches `ToggleHideSystem`. It flips `processes.hide_system` (an `EngineSettings` field, so the engine projection rebuilds) and pushes the Info toast "Hiding system processes" or "Showing system processes" [S]. Selection is kept by identity (process spec).
- **Data binding**: `processes.hide_system`.
- **Tests**: `hide_system_toggle_toast`: click; setting true; toast text "Hiding system processes"; engine received `set_settings` with `hide_system = true`.
- **Phase**: P1.

### UI-SHL-013 View mode cycler

- **Implements**: APP-005, PROC-015, PROC-016, R-010.
- **Anatomy**: `TlIconButton` 30 × 28. The icon shows the current mode [C]: `grouped` Lucide `rows-2` [M: two stacked bars in processes.png, default mode], `tree` Lucide `list-tree`, `flat` Lucide `list`.
- **States**:
  - Normal: tooltip "Toggle view mode (current: Task Manager Grouping)" with the current name [S].
  - History mode: disabled, tooltip "History shows a flat list" [S PROC-016]. (APP-002 says both buttons stay active in History mode, but R-031 only covers the hide button; PROC-016 is the specific rule.)
- **Interaction**: click dispatches `CycleViewMode`: Tree View, then Task Manager Grouping, then List View, then Tree View (TaskSlinger string order) [S]. It writes `processes.mode` and pushes the Info toast "View mode: Tree View" (or the new name) [S]. `SetViewMode(TableMode)` is the radio-item Action in View > Process View; both write the same key.
- **Data binding**: `processes.mode`; the process table sends `SetTableQuery` with a new generation on change (process spec).
- **Tests**: `view_mode_cycle_order`: three clicks from `grouped` give `flat`, `tree`, `grouped`, with toasts "View mode: List View", "View mode: Tree View", "View mode: Task Manager Grouping".
- **Phase**: P1.
- **Decided here**: per-mode icons.

### UI-SHL-014 Window picker "+"

- **Implements**: APP-006, PROC-055, R-046, R-076, table (b) Picker context.
- **Anatomy**: `TlIconButton` 30 × 28, icon Lucide `plus` [M]. Tooltip "Select process by window" [S PROC-055].
- **States**:
  - Hidden: no KWin bridge (`Snapshot.windows.header.validity == Unsupported`) [S].
  - Idle: plain.
  - Picking: toggled-on look (UI-SHL-010) until the pick ends [C].
- **Interaction**:
  - Click dispatches `StartWindowPick`: sends `Command::KWin(KWinCommand::Pick)`, sets `picking = true`, pushes the Info toast "Click a window to select its process. Hold CTRL while clicking to terminate process. Esc cancels." [S PROC-055], and pushes the key context `Picker` on the main window's root focus so Esc maps to `CancelWindowPick`.
  - The result handling (select row, Ctrl+click confirmation, toasts "No window found", "No window selected", "Unknown process") belongs to PROC-055. Where APP-006 says Ctrl+click ends the process "then gives a toast" and PROC-055 says "after a confirmation", this spec follows PROC-055 (the process spec owns action confirmations, PROC-059) [C].
  - Click while picking: dispatches `CancelWindowPick` [C].
- **Data binding**: `ComponentMask::WINDOWS` (availability); the pick reply arrives as an `ActionEvent` or a `KWinEvent` from the engine (ADR-005).
- **Tests**: `picker_hidden_without_bridge`; `picker_esc_cancels` (fake engine: Esc while picking sends `KWin(CancelPick)` and the toast "No window selected").
- **Phase**: P3.
- **Decided here**: picking look; second click cancels; PROC-055 wins over APP-006 on the Ctrl+click confirmation.

---

## 3. Main tab strip

### UI-SHL-020 Tab strip layout and accent outline

- **Implements**: APP-027, R-023, ADR-009 (six tabs), PERF-094 (Information copy icon), PROC-088 (history icon).
- **Anatomy** (height 30 [M], bg `tab_bg`; text `t_md` [C from M]):

```text
x=0
 _______________                                                             ____________________
/ Processes  ⟲  \  Performance   Services   Startup Apps   Connections   Information   [incl] / Filter: ________ (x)
+               +==========================================================================+                   (to right edge)
 \_ active tab: fill=header, accent outline 2 px, top corners r_md, open bottom     line 2 px  filter box outline
```

  Parts:
  - Tab: height 30, padding-x 22 [M, from label centres 98, 192, 304, 417, 536, 664 at 1360 px], label `t_md` regular. Inactive label `text_primary` [M white text on every tab]; active label `text_primary` medium weight [C].
  - Tab icons (part of the tab, after the label, gap 6 `sp3`, 16 × 16):
    - Processes: history toggle, Lucide `rotate-ccw-clock` (UI-SHL-023).
    - Information: copy-all, Lucide `copy` (UI-SHL-023). Shown only while Information is the active tab [M information.png].
  - Active tab: fill `header` [M: active fill equals the table header colour, so the tab joins the page header below]; a 2 px [M] outline in `accent` on top, left and right sides, top corners radius `r_md`, bottom open.
  - Accent line: 2 px high [M], colour `accent`, along the bottom edge of the strip, from x = 0 to the active tab's left edge, and from the active tab's right edge to the filter box's left edge (or to the window's right edge on Performance, which has no filter box [M performance.png]). Where the line meets the active tab's sides and the filter box's left side, the corner is rounded with radius `r_md` (outward curve, as measured) [M].
  - Filter box (UI-SHL-024) at the right, its own accent outline on top and left sides, joined to the line.
  - "Include children" checkbox (UI-SHL-025) left of the filter box when shown.
  - Overflow "»" button (UI-SHL-022) when tabs do not fit.
- **Tab set and order** [S R-023]: Processes, Performance, Services, Startup Apps, Connections, Information. A tab whose page is not built yet in the current phase is not shown (P1 shows Processes and Performance only; Information arrives P2 with the System Information fields; Services, Startup Apps and Connections in P2) [S ADR-009].
- **Label in History mode**: the Processes tab reads "History" [S].
- **States**:
  - Inactive tab hover: fill `tab_hover`; no outline.
  - Inactive tab pressed: fill `tab_hover`.
  - Active tab: as above; hover does not change it.
  - Window inactive: outline and line in `divider` (UI-SHL-007).
  - Keyboard focus: tabs are not focusable (gpui-kit tabs are not, and table b gives Ctrl+1..6, Ctrl+Tab) [S widgets §1].
  - Disabled: never (unbuilt tabs are absent).
- **Interaction**:
  - Left click on a tab dispatches `SelectTab(i)` with i = 0..5 in the order above.
  - Mouse-down on a tab stops propagation (no window drag) [S widgets §1].
  - Middle-click, right-click: nothing [C].
  - Mouse wheel over the strip: nothing [C] (no horizontal tab scrolling; overflow uses the "»" menu).
  - Keys (table b): Ctrl+1..Ctrl+6, Alt+1..Alt+6 (`SelectTab(i)`), Ctrl+Tab (`NextTab`), Ctrl+Shift+Tab (`PrevTab`). Next and previous wrap around and skip tabs that are not shown [C].
  - `SelectTab` on the already active tab: nothing [C].
  - Every switch writes `UiState.tabs.last_tab` [S] and updates demand (ADR-008: the newly active page's components become visible; the old page's demand drops).
- **Data binding**: `MainWindow.active_tab: TabId`; `UiState.processes.history_mode`; `TlSettings` versions `motion` (animation), `appearance` (scale).
- **Build**: custom `TabStrip` Entity in `window/tabstrip.rs`, embedded `.cached(..)`. Not gpui-kit `TabBar`: the kit has no outline-joined-to-line variant, and its `Outline` variant draws a full border [C]. The strip is `h_flex()` of `div` tabs plus a custom paint element `AccentRule` (implements `Element`) that paints the outline and line in one `paint_path` after layout, using the active tab's bounds and the filter box's bounds recorded in `prepaint` (`window.insert_hitbox` is not needed; bounds come from child layout ids). The path is rebuilt only when those bounds, the colour or the scale change; it is cached in the `TabStrip` (`Option<(AccentRuleKey, Path<Pixels>)>`).
- **Performance**: re-renders only on tab change, history-mode change, filter focus change (width animation, UI-SHL-024), window activation, theme, scale or language change. Zero snapshot subscriptions. The accent path build is O(1) (about 12 path segments).
- **Accessibility**: each tab's accessible name is its label; the history icon and copy icon have their own names (UI-SHL-023).
- **Tests**: `tabstrip_order_and_labels` (render test, all phases on): six labels in order. `tabstrip_history_label`: with `history_mode = true`, first label "History". `tabstrip_keys`: Ctrl+3 selects Services; Ctrl+Tab from Information wraps to Processes; with only P1 tabs, Ctrl+Tab from Performance goes to Processes. `accent_rule_bounds`: with Processes active and filter box at x = 1060..1280, the rule path starts at x = 0 and ends at 1060.
- **Phase**: P1.
- **Decided here**: custom strip instead of kit `TabBar`; active label weight medium; wrap-around and skip rule; no wheel scroll.

### UI-SHL-021 Tab switch animation

- **Implements**: APP-027, R-017, ADR-009 (only the active page renders), table (a) `motion.tab_switch_ms`.
- **Behaviour**:
  - On switch with animations on and `motion.tab_switch_ms > 0`:
    - The accent outline slides from the old tab's bounds to the new tab's bounds: x and width interpolate over `tab_switch_ms` (default 250) with `ease_out_cubic` [S duration; C easing].
    - Content crossfade: the new page fades in from opacity 0 to 1 over the same duration [S "crossfades content"]. The old page is not rendered during the fade (ADR-009 renders only the active page), so the crossfade is a fade-in over `window_bg` [C].
  - With animations off, reduced motion on, or `tab_switch_ms = 0`: the outline jumps and the page appears at full opacity in the same frame [S].
  - A new switch during an animation starts from the current interpolated outline position [C].
- **Build**: GPUI `with_animation(ElementId::from(("tab-switch", generation)), Animation::new(Duration::from_millis(ms)).with_easing(ease_out_cubic), |el, t| ..)` on the page container (opacity) and on the `AccentRule` (geometry, which rebuilds the path each animation frame only during the animation). The animation is one-shot, so frames stop when it ends. This is the one permitted use of `Animation` in the shell besides toasts and the filter width; it is not a continuous redraw (ADR-009 animation driver rule applies to charts).
- **Performance**: during the 250 ms, each frame rebuilds a 12-segment path and sets an opacity: well under 0.1 ms. Charts in the new page start their own driver requests after the first render.
- **Tests**: `tab_switch_instant_when_reduced_motion` (TestAppContext with `set_reduce_motion(true)`): after `SelectTab(1)` and one frame, the page container opacity is 1.0 and no animation is running.
- **Phase**: P2 (the switch is instant in P1) [S APP-027].
- **Decided here**: easing; crossfade realized as fade-in; restart-from-current rule.

### UI-SHL-022 Tab overflow menu

- **Implements**: APP-027 ("»" menu).
- **Behaviour**:
  - The strip measures each tab's width at layout (label shaped once per language and scale change and cached as `Pixels`). Available width = window width − filter box width − include-children checkbox width (if shown) − 16 (left margin of the filter box) − 28 (overflow button).
  - If the tabs do not fit, trailing tabs that do not fit move into the overflow menu; the active tab is always kept visible by swapping it into the last visible slot [C].
  - Overflow button: `TlIconButton` 28 × 30, Lucide `chevrons-right` [C], placed right after the last visible tab, tooltip "More tabs" [C].
  - Click opens a `TlMenu` listing the hidden tabs with the same labels; choosing one dispatches `SelectTab(i)`.
  - At the minimum width of 800 px with all six tabs, the tabs need about 680 px at 100% and the filter box needs 236, so the overflow appears at 100% scale below about 960 px of window width [C, estimate from M label widths].
- **Build**: part of `TabStrip`; the menu uses `TlMenu` (PopupMenu with `.menu(label, Box::new(SelectTab(i)))`).
- **Tests**: `overflow_keeps_active_tab`: window 800 px, Information active: Information is visible and the menu lists the tabs that do not fit.
- **Phase**: P1.
- **Decided here**: active tab always visible; icon and tooltip.

### UI-SHL-023 History toggle and Information copy icon on tabs

- **Implements**: APP-027, PROC-088, PROC-086 (toast with Enable), D-010, PERF-094.
- **Anatomy**:
  - History icon: 16 × 16 Lucide `rotate-ccw-clock` inside the Processes tab after the label, gap 6. Hit area 22 × 22 centred on the icon [C]. Colour `text_secondary`; in History mode `accent` [C].
  - Copy icon: 16 × 16 Lucide `copy` inside the Information tab after the label, gap 6, shown only while Information is active [M]. Hit area 22 × 22. Colour `text_secondary`.
- **States**: hover on the icon: icon `text_primary` and a `tab_hover` rounded square (`r_sm`) behind the hit area. Pressed: same. Disabled: never.
- **Interaction**:
  - History icon click dispatches `ToggleHistoryMode`. If the Processes tab is not active, it also selects it [C]. It flips `UiState.processes.history_mode` (persisted) and the tab label. When turning History mode on with `history.enabled = false`: the 10-minute RAM window shows (D-010) and an Info toast "History recording is off" appears with an "Enable" button that dispatches `ToggleSetting(HistoryEnabled)` [S PROC-086]. The toast has autohide off because it has an action button (kit behaviour) and closes when the user clicks Enable or its close button.
  - Copy icon click dispatches `CopyInformationAll` (owned by PERF-094: copies every section; toast "Copied to clipboard").
  - Clicks on the icons stop propagation so the tab click does not also fire.
  - Tooltips: "Show history" / "Show live processes" [C] and "Copy all" [C].
- **Data binding**: `UiState.processes.history_mode`, `history.enabled`.
- **Tests**: `history_toggle_selects_processes`: on Performance, clicking the icon (via action dispatch) selects Processes and sets history mode. `history_off_toast`: with recording off, toggling on pushes "History recording is off" with one action button.
- **Phase**: P3 (history), P2 (copy icon, with Information).
- **Decided here**: icon colours, hit areas, tooltips, select-tab-on-toggle.

### UI-SHL-024 Filter box

- **Implements**: APP-028, PROC-044, R-043, ADR-009 (debounce and generation), table (b) Ctrl+F, Esc, printable characters.
- **Anatomy** [M information.png, processes.png]:

```text
 ____________________________________
/ Filter:  [text............] (x)     |   <- flush with the window's right edge
  outline: accent 2 px on top and left, top-left corner r_md, joined to the strip line
```

  - Placement: right end of the tab strip, flush with the window's right edge, full strip height 30 minus the 2 px line (28) [M].
  - Width: 220 unfocused [M: 221 at 1360 px], 280 focused [S APP-028]. APP-028 says 160 unfocused; the measured TaskSlinger box is 220, and this spec uses the measured value [C, listed as an open item].
  - Label "Filter:" `t_md` `text_primary` [M], left padding 10 [C], gap 6 to the input.
  - Input: `TlInput` without its own border or background (the outline is the border), text `t_md`, fill `tab_bg` unfocused and `input_focus` focused [C]. Placeholder: none [M].
  - Clear button: Lucide `circle-x` 14 × 14 in `text_secondary`, right padding 8, shown only when the text is not empty (`Input::cleanable(true)` behaviour) [S].
  - Shown on every tab except Performance [M performance.png]. Each tab keeps its own filter text for the session [S]; switching tabs swaps the text into the single `InputState` with `set_value` (which does not emit Change) [C].
- **States**:
  - Unfocused empty, unfocused with text (text stays visible, clear button visible), focused (width 280, fill `input_focus`), hover (cursor I-beam over the input part only).
  - Width animation: 220 to 280 on focus and back on blur, over 150 ms [S] with `ease_out_cubic` [C]; instant with animations off or reduced motion [S]. The tabs to its left re-layout each animation frame; the overflow rule (UI-SHL-022) applies to the final width only, so no tab jumps into the menu mid-animation [C].
  - Window inactive: outline in `divider`.
- **Interaction**:
  - Click anywhere in the box focuses the input.
  - Ctrl+F (`FocusFilter`, Global) focuses it and selects all its text [C].
  - Type-to-filter (UI-SHL-124): a printable key while a table has focus moves focus here and inserts the character.
  - Typing: `InputEvent::Change` starts or restarts a 100 ms debounce task [S]; when it fires, the active page receives `FilterChanged { tab, text }`. The process page sends `SetTableQuery` with a new generation (ADR-009); other pages filter as their specs say.
  - Esc in the box (`FilterEscape`, handled before the global Esc chain): if text is not empty, clear it (Change fires, debounce runs at once with an empty query) and move focus to the active page's table; if text is empty, move focus to the table and let the global Esc chain continue only on a further Esc [S R-043].
  - Enter: moves focus to the table and selects the first row [C].
  - Down arrow: same as Enter [C].
- **Data binding**: `FilterBox { input: Entity<InputState>, per_tab: [SharedString; 6], debounce: Option<Task<()>> }`. Reads only its own state; never subscribes to snapshots.
- **Build**: `FilterBox` Entity in `window/filter.rs`, embedded in `TabStrip`. Width animation with GPUI `with_animation` keyed on the focus generation.
- **Performance**: keystroke cost is the kit input's own (one rope edit, one notify on the InputState) plus one timer reset. The text is read with `text()` (no allocation) and converted to `Arc<str>` lowercased once per debounced query.
- **Accessibility**: the input's accessible name is "Filter"; the clear button is "Clear filter".
- **Tests**: `filter_debounce_100ms` (TestAppContext): type three characters 30 ms apart; exactly one query after 100 ms of quiet, with the latest generation. `filter_esc_rule`: text "fire", Esc: text empty, focus on table; second Esc goes to the global chain. `filter_per_tab_text`: text on Processes survives a switch to Services and back. `filter_hidden_on_performance`.
- **Phase**: P1.
- **Decided here**: unfocused width 220 (deviation from APP-028's 160, open item 1); fills; Enter and Down behaviour; select-all on Ctrl+F.

### UI-SHL-025 "Include children" checkbox placement

- **Implements**: PROC-045, APP-028.
- **Anatomy** [M cus_sheet_03.png: label then checkbox, left of "Filter:"]: label "Include children" `t_sm` `text_primary`, gap 6, then a `TlCheckbox` 14 × 14, then 16 px to the filter box. Vertically centred in the strip.
- **Shown**: on the Processes tab only, while the filter text is not empty or `processes.mode = tree` [S PROC-045].
- **Interaction**: click on the label or box dispatches `ToggleSetting(ProcessesFilterIncludeChildren)`; the process spec owns the toasts ("Including filtered process children." / "Hiding unmatched filtered process children."). Not a Tab stop [C]; keyboard users use the same Action from View > Process View? No: the process spec has no menu item for it, so the checkbox is a Tab stop after the filter input [C].
- **Tests**: `include_children_visibility`: hidden in grouped mode with an empty filter, shown in tree mode.
- **Phase**: P1.
- **Decided here**: label size `t_sm`, spacing, Tab stop after the filter input.
