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
- **Interaction**: click on the label or box dispatches `ToggleSetting(ProcessesFilterIncludeChildren)`; the process spec owns the toasts ("Including filtered process children." / "Hiding unmatched filtered process children."). The process spec gives it no menu item, so the checkbox is a Tab stop right after the filter input, and Space toggles it when focused [C].
- **Tests**: `include_children_visibility`: hidden in grouped mode with an empty filter, shown in tree mode.
- **Phase**: P1.
- **Decided here**: label size `t_sm`, spacing, Tab stop after the filter input.

---

## 4. Menus

### UI-SHL-030 Menu bar behaviour and keyboard

- **Implements**: APP-002, APP-012, APP-017, APP-022, APP-024, APP-026, table (b) F10, Alt+F, Alt+V, Alt+S, Alt+H, Esc.
- **Anatomy**: the menu buttons in UI-SHL-003 part 4. Each button: `TlButton` ghost, height 26, padding-x 16, label `t_md`. Open state: fill `menu_hover` (stays pressed while its popup is open, widgets rule 9).
- **States**: default (transparent), hover (`menu_hover`), open (`menu_hover`), window inactive (label `text_secondary`). No disabled state (a menu always has at least one item).
- **Interaction**:
  - Click a closed menu button: open its popup below the button, left edges aligned, 0 px gap (popup top = title bar bottom) [C].
  - Click the open menu's button: close it.
  - While any menu is open, hovering another menu button opens that menu and closes the first (classic menu bar) [S gpui-kit `AppMenuBar::handle_hover` behaviour, copied].
  - Keys: F10 opens File (`OpenMenu(MenuId::File)`); Alt+F, Alt+V, Alt+S, Alt+H open File, View, Settings, Help (`OpenMenu(..)`) [S table b]. Alt+D opens Debug when present [C]. When a menu opens from the keyboard, its first enabled item is highlighted [C].
  - Inside an open popup (key context `TlMenu`, kit `PopupMenu` bindings): Up and Down move the highlight and skip separators, headers and disabled rows; Enter or Space activates; Right opens a submenu; Left closes a submenu, or at the top level moves to the previous menu (wrapping); Right at the top level on an item without a submenu moves to the next menu (wrapping) [S kit `SelectLeft`/`SelectRight` in `MenuBar` context]; Home and End jump to the first and last item; Esc closes the innermost popup, and focus returns to the element that had focus before the menu opened (widgets rule 16).
  - Activating an item closes the whole menu chain, then dispatches the Action to the focus that was active before the menu opened (`PopupMenu::action_context(previous_focus)`), so table-context Actions (for example Collapse all) reach the table.
  - Pressing and releasing Alt alone does nothing [C] (no Windows-style mnemonic underlines).
- **Data binding**: `MenuBar { open: Option<MenuId>, popup: Option<Entity<PopupMenu>>, return_focus: Option<FocusHandle> }`. Menus are built fresh each time they open, from `TlSettings` and app state at that moment (cheap; widgets perf note).
- **Build**: custom `MenuBar` Entity in `window/menubar.rs`, modelled on gpui-kit `menu/app_menu_bar.rs` (open/hover/left/right logic) but with tasklens-built `PopupMenu`s, because the kit's `AppMenuBar` reads static `OwnedMenu`s from `cx.set_menus` and cannot show live checks, sliders or painted theme rows [C]. Popup shown with `deferred(anchored().anchor(Anchor::TopLeft).snap_to_window_with_margin(px(8.)))`.
- **Performance**: building a menu is about 30 to 60 elements once per open; nothing runs while menus are closed.
- **Accessibility**: each popup row has its label as accessible name; check and radio states are part of it ("Tree View, selected").
- **Tests**: `menubar_hover_switch`: open File, hover View: View open, File closed. `menubar_alt_keys`: Alt+V opens View with "Columns" highlighted. `menubar_left_right_wrap`: in File, Left opens Help (or Debug when present). `menu_action_reaches_table`: focus table, open View > Process View > Collapse all; the table receives `CollapseAll`.
- **Phase**: P1.
- **Decided here**: zero gap under the button; keyboard highlight of the first item; own `MenuBar` instead of kit `AppMenuBar`; Alt alone does nothing; Alt+D for Debug.

### UI-SHL-031 Menu popup rows (visual spec for every menu)

- **Implements**: APP-012 to APP-026 (all menu entries), reference video frames (menus in `cus_sheet_01.png` to `cus_sheet_03.png`).
- **Anatomy** (`TlMenu` / kit `PopupMenu` restyled through the theme map; values in px at 100%):

```text
+--------------------------------------------------+  bg=menu_bg, border 1 px divider, radius r_sm, shadow (shadow_intensity_pct)
| pad-y 4                                          |
| [✓]  Label text                    Ctrl+N   [›]  |  row h=26, pad-x 8, check col 20, gap 4, hint t_sm text_secondary
| ------------------------------------------------ |  separator: 1 px divider, margin-y 4, inset 8
|      HEADER                                      |  header row h=22, t_xs semibold text_secondary, uppercase off
| [ ]  UI Scale       [====o--------]   100%       |  slider row h=30 (UI-SHL-037)
+--------------------------------------------------+
```

  - Popup: min width 200 [C, kit default 120 is too narrow for TaskSlinger labels], max width 360, max height = window height − 16, scrollable when taller. Padding-y 4 (`sp2`).
  - Row: height 26 [C], padding-x 8 (`sp4`), radius `r_sm` for the hover fill, label `t_base` `text_primary`.
  - Check column: 20 wide at the left of every row of a menu that has any check or radio row; check icon Lucide `check` 14; radio uses Lucide `dot` 14 [C]. Rows without a check leave the column empty so labels align [M reference].
  - Icon column: none in tasklens menus (labels only, like TaskSlinger), except the theme rows' copy icon (UI-SHL-038).
  - Shortcut hint: right-aligned, `t_sm` `text_secondary`, from the Action's binding (kit `Kbd` lookup), 24 px minimum gap from the label.
  - Submenu row: chevron Lucide `chevron-right` 14 at the right in `text_secondary`. Submenu opens to the right, top-aligned with its row minus 4 (so first rows line up), flipping left when there is no room (kit behaviour). Hover delay before opening 0 ms; leaving towards the submenu keeps it open (kit's own pointer logic).
  - Labels with ">" in the merged spec ("Columns >") show the chevron; the ">" character itself is not drawn [C].
  - Labels ending in "…" or "..." keep the exact spec text [S].
  - Separator: 1 px `divider`, 4 px space above and below, inset 8 from both sides.
  - Group header (Settings menu groups): height 22, label `t_xs` semibold `text_secondary`, not interactive [C].
- **States**:
  - Highlighted (hover or keyboard): row fill `menu_hover`, label `text_primary`.
  - Disabled: label and hint at the foundation's disabled opacity, no highlight on hover, still shows its tooltip if it has one (kit rows support tooltips through `menu_element`).
  - Checked: check icon in `accent` [C].
- **Motion**: the popup opens with the kit's 150 ms fade and 8 px slide when animations are on; instant otherwise. No exit animation (kit).
- **Build**: kit `PopupMenu` with tasklens builder helpers in `window/menubar.rs`: `check(label, checked, action)`, `radio(label, selected, action)`, `header(label)`, `slider_row(..)`, `theme_row(..)`. The kit's row height and paddings are set through `PopupMenu` styling (`.min_w(px(200.))`, and the theme map's `popover` colours); if the kit row height is not 26 at `Size::Small`, the helpers render rows with `menu_element` and own `div` styling [C].
- **Tests**: `menu_row_metrics` (render test): a check row is 26 tall, the check column 20 wide, the hint right-aligned.
- **Phase**: P1.
- **Decided here**: row height 26, min width 200, check and radio icons, header style, chevron replacing ">".

### UI-SHL-032 File menu

- **Implements**: APP-012, APP-013 (dropped), APP-014, APP-015, APP-016, PROC-095, PROC-096.
- **Items** (in order; an item exists only from its phase, so the menu never has dead items [S]):

  | # | Label | Shortcut | Action | Phase | Notes |
  |---|---|---|---|---|---|
  | 1 | Run... | Ctrl+N | `OpenRunDialog` | P2 | Opens or focuses "tasklens Run" (UI-SHL-097) |
  | 2 | Search File Handles... | | `OpenHandleSearch` | P3 | Opens or focuses "tasklens Handle Search" (UI-SHL-098) |
  | 3 | Open System Monitor | | `OpenSystemMonitor` | P4 | Hidden when none of `plasma-systemmonitor`, `gnome-system-monitor`, `missioncenter` is on `PATH` (checked once at startup, cached). Failure toast (Error) "Failed to launch System Monitor" [S] |
  | — | separator | | | P2 | Only when an item above it exists |
  | 4 | Exit tasklens | Ctrl+Q | `Quit` | P1 | Quits even with tray settings on (UI-SHL-113) |

  - There is no "Run Installer" item (APP-013 DROP).
- **Tests**: `file_menu_p1_items`: with only P1 features, the menu holds exactly "Exit tasklens" and no separator.
- **Phase**: P1 (item 4), others as listed.

### UI-SHL-033 View menu (root)

- **Implements**: APP-017, R-089.
- **Items**: submenus in order "Columns", "Process View", "Window", "Appearance", "Theme", "Graphs" [S]. Alt+V opens it.
  - Columns: UI-SHL-034 (P1). Disabled when the active tab has no table (Performance, Information), with tooltip "This tab has no table" [C].
  - Process View: UI-SHL-035 (P1). Enabled on every tab (it changes settings that apply when the Processes tab shows) [C].
  - Window: UI-SHL-036 (P2: Always On Top; P3: Performance mini window). Absent in P1 [S].
  - Appearance: UI-SHL-037 (P2). Theme: UI-SHL-038 (P2). Graphs: UI-SHL-039 (P1).
- **Phase**: P1.
- **Decided here**: Columns disabled on table-less tabs with tooltip.

### UI-SHL-034 View > Columns

- **Implements**: APP-018, PROC-024, PROC-025, PROC-026, R-065, R-075.
- **Items** (acting on the active tab's table: Processes, History, Services, Startup Apps or Connections):

  | # | Label | Kind | Action | Notes |
  |---|---|---|---|---|
  | 1 | Select Columns… | item | `OpenColumnPicker { table }` | Opens the "Columns" picker popover anchored under the header's right end (the table spec owns the picker) |
  | 2 | Resize columns to fit content | item | `ResizeColumnsToFit { table }` | Template strings per table (d) |
  | 3 | Resize Behavior | submenu | | |
  | 3a | Proportional | radio | `SetSetting { TablesColumnResize, "proportional" }` | |
  | 3b | Last Column | radio | `SetSetting { TablesColumnResize, "last_column" }` | |
  | — | separator | | | |
  | 4 | Reset to default | item | `ResetColumns { table }` | Toast (Info) "Columns reset to default" [S] |

  - "Hide This Column" is only in the header right-click menu [S], which the table specs own.
- **Tests**: `columns_menu_targets_active_table`: on Services, `ResetColumns` carries `TableId::Services`.
- **Phase**: P1.

### UI-SHL-035 View > Process View

- **Implements**: APP-019, PROC-014, PROC-017, PROC-020, PROC-043, R-010.
- **Items**:

  | # | Label | Kind | Bound key | Action |
  |---|---|---|---|---|
  | 1 | Tree View | radio | `processes.mode = tree` | `SetViewMode(Tree)` |
  | 2 | Task Manager Grouping | radio | `grouped` | `SetViewMode(Grouped)` |
  | 3 | List View | radio | `flat` | `SetViewMode(Flat)` |
  | — | separator | | | |
  | 4 | Tree Connector Lines | check | `processes.tree_guide_lines` | `ToggleSetting(ProcessesTreeGuideLines)` |
  | 5 | Highlight New and Exited Processes | check | `processes.highlight_new_exited` | `ToggleSetting(ProcessesHighlightNewExited)` |
  | — | separator | | | |
  | 6 | Collapse all | item (Ctrl+L) | | `CollapseAll` (Table context) |

  - `SetViewMode` writes the key and pushes the same toast as the cycler ("View mode: Tree View") [C, consistent with UI-SHL-013].
  - In History mode items 1 to 3 are disabled with tooltip "History shows a flat list" [C, from PROC-016].
  - Collapse all is disabled when the Processes tab is not active [C].
- **Phase**: P1.
- **Decided here**: toast on radio change; disabled rules.

### UI-SHL-036 View > Window

- **Implements**: APP-020, APP-009, PERF-093, R-058, R-073.
- **Items**:

  | # | Label | Kind | Shortcut | Action | Phase |
  |---|---|---|---|---|---|
  | 1 | Always On Top | check (`window.always_on_top`) | | `ToggleAlwaysOnTop` | P2 |
  | 2 | Performance mini window | check (mini window open) | Ctrl+Shift+M | `ToggleMiniWindow` | P3 |

  - Item 1 disabled with tooltip "Not supported by this desktop" where the pin is disabled (UI-SHL-005) [S].
  - Item 2 is checked while the mini window exists; closing the mini window unchecks it [S PERF-093]. The mini window itself is the Performance spec's.
- **Phase**: P2, P3.

### UI-SHL-037 View > Appearance (sliders and checks)

- **Implements**: APP-040, R-020, R-077, ADR-009 (Metrics and scale), PROC-040.
- **Items** [S order; M reference frames show the same rows with the value at the right]:

  | # | Label | Kind | Key | Range, step, format |
  |---|---|---|---|---|
  | 1 | UI Scale | slider row | `appearance.ui_scale_pct` | 75 to 200, step 5, "%d%%" |
  | 2 | Row Height | slider row | `appearance.row_height_pct` | 80 to 150, step 5, "%d%%" |
  | 3 | Rounded Corners | check | `appearance.rounded_corners` | |
  | 4 | Value Color Coding | check | `appearance.heat_coloring` | |
  | 5 | Value Color Coding Intensity | slider row | `appearance.heat_intensity_pct` | 0 to 100, step 5 [C], "%d%%" |
  | 6 | Drop Shadow Intensity | slider row | `appearance.shadow_intensity_pct` | 0 to 100, step 5 [C], "%d%%" |

- **Slider row anatomy** (height 30 [C]):

```text
| [chk col 20] Label (w=150, t_base, ellipsis) [==========o------] (w=120)  100% (w=40, right, t_sm tnum text_secondary) |
```

  - Label column 150 wide, truncated with "…" [M "Value Color Coding I…" in the reference].
  - `TlSlider` 120 wide, track and thumb from the foundation.
  - Value text 40 wide, right-aligned, tabular figures.
- **Interaction**:
  - Dragging previews live: each `SliderEvent::Change` applies the value to `TlSettings` in memory without persisting [S]. `SliderEvent::Release` persists (one save) [S].
  - UI Scale release pushes the Info toast "UI scale changed to 120%" (the value) [S]. Ctrl+Plus, Ctrl+Minus, Ctrl+0 do the same steps from anywhere (UI-SHL-130).
  - Keyboard on a highlighted slider row: Left and Right step by the step value, Home and End jump to min and max; each key press counts as a release (persist plus toast for UI Scale) [C].
  - The popup stays open while dragging and after release; clicking a check row toggles it and also keeps the popup open [C, matches TaskSlinger's Appearance submenu which stays open in the reference video].
  - Applying a UI Scale change re-lays out the menu itself; the popup stays anchored to its row [C].
- **Effects**: UI-SHL-130 (scale and row height), UI-SHL-131 (radius), heat colouring and shadow per their owners.
- **Build**: `slider_row` helper renders a `menu_element` row holding a kit `Slider` whose `SliderState` entities are created once per `MenuBar` and reused across menu opens (entities are never created in render) [C].
- **Tests**: `appearance_slider_persist_on_release`: three Change events then Release: one persist call, final value saved, toast "UI scale changed to 115%".
- **Phase**: P2 (menu), P1 (keys Ctrl+Plus/Minus/0).
- **Decided here**: steps of 5 for the two 0..100 sliders; row geometry; popup stays open; keyboard on slider rows.

### UI-SHL-038 View > Theme

- **Implements**: APP-035, APP-036, APP-037, APP-038, R-068, R-069; reference `cus_sheet_02.png` (each theme row painted in its own palette, copy icon at right).
- **Items**:

  | # | Label | Kind | Action |
  |---|---|---|---|
  | 1..12 | Default Dark, Vivid Nightfall, Catppuccin Frappé, Catppuccin Macchiato, Catppuccin Mocha, Dracula, OLED Black, Default Light, Windows 7 Aero, Golden Summer Fields, Soft Pink Delight, Catppuccin Latte | theme row (radio) | `SetTheme(name)` |
  | 13.. | custom themes from `themes.json`, in file order | theme row (radio) | `SetTheme(name)` |
  | — | separator | | |
  | | Create copy | item | `CopyTheme(active)` |
  | | Edit colors | item | `EditTheme(active)` |
  | | Share theme | item | `ShareTheme(active)` |
  | | Delete theme | item | `DeleteTheme(active)` |
  | — | separator | | |
  | | Import theme | item | `ImportTheme` |
  | | Sync theme with system | check (`theme.follow_system`) | `ToggleSetting(ThemeFollowSystem)` |
  | | Preferred Light Theme | submenu: every theme name as radio (`theme.light`) | `SetSetting { ThemeLight, name }` |
  | | Preferred Dark Theme | submenu: every theme name as radio (`theme.dark`) | `SetSetting { ThemeDark, name }` |

  - In P1 the list holds only Default Dark and Default Light (R-068).
  - APP-036 lists four per-theme items. This spec places them once, acting on the active theme, so they are reachable from the keyboard without per-row submenus [C]. The copy icon on every row is the per-row "Create copy" shortcut [S].
- **Theme row anatomy** (height 26, full popup width):
  - Background: that theme's `menu_bg` [C; reference shows each row in its own palette].
  - Label: theme name, `t_base`, that theme's `text_primary`.
  - Check column: radio dot on the active theme (`theme.active`), in that theme's `accent`.
  - Copy icon: Lucide `copy` 14, at the right, 22 × 22 hit area, that theme's `text_secondary`; tooltip "Create copy". Click dispatches `CopyTheme(name)` and does not apply the theme.
  - Highlight: the row's own `menu_hover` from that theme.
- **Behaviour**:
  - `SetTheme(name)` writes `theme.active`, runs the theme switch (UI-SHL-080) and, if `theme.follow_system` is on, turns it off with the Info toast "Theme sync turned off" [C] (an explicit choice overrides sync).
  - `CopyTheme(name)` creates "<name> copy" (or "<name> copy 2", 3, ...) in `themes.json`, makes it active, Success toast "Theme copied" [S].
  - `EditTheme(active)`: built-in active theme gives Info toast "Copy a theme before editing it" [S]; a custom theme opens the Theme Editor (UI-SHL-081). Absent before P3.
  - `ShareTheme(active)` opens "tasklens Share Theme" (UI-SHL-082). Absent before P3.
  - `DeleteTheme(active)`: built-in gives Info toast "Built-in themes cannot be deleted" [S]; custom opens the confirm window "Delete this custom theme?" (UI-SHL-084).
  - `ImportTheme` reads the clipboard (UI-SHL-083). Absent before P3.
- **Build**: `theme_row` helper (custom `menu_element`) reads the other theme's `TlTheme` from the `ThemeRegistry` (all themes parsed once at startup and after every `themes.json` write).
- **Performance**: up to about 30 rows; colours are already parsed `Hsla`; no allocation per frame (menu is static while open).
- **Tests**: `theme_rows_painted`: the "Dracula" row background equals the Dracula theme's `menu_bg`. `delete_builtin_toast`. `set_theme_turns_off_sync`.
- **Phase**: P2 (themes, sync), P3 (Edit, Share, Import).
- **Decided here**: per-theme actions act on the active theme; copy naming "<name> copy N"; explicit choice turns sync off with toast "Theme sync turned off".

### UI-SHL-039 View > Graphs

- **Implements**: APP-021, PERF-080, PERF-084, R-089.
- **Items**:

  | # | Label | Kind | Key | Phase |
  |---|---|---|---|---|
  | 1 | CPU Graph | check | `graphs.minigraph_cpu` | P1 |
  | 2 | Memory Graph | check | `graphs.minigraph_memory` | P1 |
  | 3 | Disk Graph | check | `graphs.minigraph_disk` | P1 |
  | 4 | Network Graph | check | `graphs.minigraph_network` | P1 |
  | 5 | GPU Graph | check | `graphs.minigraph_gpu` | P1 |
  | 6 | VRAM Graph | check | `graphs.minigraph_vram` | P1 |
  | 7 | GPU Temperature Graph | check | `graphs.minigraph_gpu_temperature` | P1 |
  | — | separator | | | |
  | 8 | Keep detached graphs open when closing main window | check | `graphs.keep_detached_on_close` | P3 |
  | 9 | Reset Minigraph Layout | item | `ResetMinigraphLayout`; toast (Success) "Minigraph layout reset" [S] | P1 |

  - Each check dispatches `ToggleSetting(<key>)`. GPU, VRAM and GPU temperature are disabled with tooltip "No GPU found" when the device catalog has no GPU [C].
- **Phase**: P1 (items 1 to 7, 9), P3 (item 8).
- **Decided here**: disabled rule without a GPU.

### UI-SHL-040 Settings menu

- **Implements**: APP-022, APP-046, APP-048, APP-049, APP-043, PROC-086, PROC-087, PROC-097, R-015, R-025, R-028, R-058, D-003.
- **Items** (group headers are UI-SHL-031 header rows; Alt+S opens the menu):

  | Group | Label | Kind | Key or Action | Phase |
  |---|---|---|---|---|
  | **Monitoring** | Refresh | submenu radio: 0.25s, 0.5s, 1.0s, 2.0s, 5.0s, 10.0s | `processes.interval_ms` 250..10000 | P1 |
  | | Network transfer unit | submenu radio: Bits per second, Bytes per second | `units.network` | P1 |
  | | Auto-scale network transfer rates | check | `units.network_auto_scale` | P1 |
  | | Record history | check | `history.enabled` (`ToggleRecordHistory`) | P3 |
  | | History storage | submenu radio: 1 hour, 2 hours, 3 hours, 4 hours, 6 hours, 8 hours, 12 hours | `history.retention_h` | P3 |
  | | Save history to disk | check, disabled while Record history is off (tooltip "Turn on Record history first") [C] | `history.persist` | P3 |
  | | Rendering GPU | submenu radio (UI-SHL-040a) | `rendering.gpu` | P4 |
  | **Motion & interaction** | Animations | check | `motion.animations` | P1 |
  | | Smooth Graphs | check | `motion.smooth_graphs` | P1 |
  | | Smooth Scroll | check | `motion.smooth_scroll` | P1 |
  | | Tab Switch | slider row 0 to 500, step 10, "%d ms" | `motion.tab_switch_ms` | P2 |
  | | Close with Esc | check | `window.close_with_esc` | P1 |
  | **Window & startup** | Close to tray | check | `tray.close_to_tray` | P3 |
  | | Minimize to tray | check | `tray.minimize_to_tray` | P3 |
  | | Always show tray icon | check | `tray.always_show` | P3 |
  | | Start at login | check | `startup.start_at_login` (`ToggleStartAtLogin`) | P2 |
  | | Start minimized | check, indented by 16, disabled unless Start at login is on | `startup.start_minimized` | P2 |
  | | Default tab | submenu radio: Processes, Performance, Services, Startup Apps, Connections, Information, Last used | `general.start_tab` | P1 |
  | | Unlock privileged actions at launch | check | `privilege.unlock_at_launch` | P3 |
  | | Skip password prompts (experimental) | check (`TogglePasswordless`) | `privilege.passwordless` | P4 |
  | | Auto-fetch Public IP on launch | check | `network.auto_fetch_public_ip` | P2 |
  | | Open rescue window on critical memory pressure | check | `lowmem.enabled` | P3 |
  | **Language** | Language: | submenu radio: System default, then the locales by their own names (UI-SHL-131) | `general.language` | P1 (P4 for locales other than English) |
  | — | separator | | | |
  | | All settings… | item, Ctrl+, | `OpenSettings` | P2 |
  | | Reset Settings to Defaults | item | `ResetSettings` (UI-SHL-091 confirm) | P1 |

  - Default tab lists only tabs built in the current phase plus "Last used" [C].
  - Refresh labels are exactly "0.25s", "0.5s", "1.0s", "2.0s", "5.0s", "10.0s" [S].
  - Named Actions with extra work:
    - `ToggleRecordHistory`: writes `history.enabled`; Info toast "History recording on" or "History recording off" [C].
    - `ToggleStartAtLogin`: writes or deletes `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop` through tl-app (`write_atomic`) [S APP-048]; failure: Error toast "Could not change the autostart entry" and the setting is reverted [C].
    - `TogglePasswordless`: turning on first opens the modal confirm (UI-SHL-091) with the text "tasklens actions that change other users' processes, system services and network connections will run without asking for a password. Continue?" and buttons "Skip prompts" / "Cancel" [S]; then runs `pkexec /usr/lib/tasklens/tasklens-setup polkit-rule on`; success writes the key, cancel leaves it off with toast "Authentication cancelled". Turning off runs `polkit-rule off` without a confirm [S].
    - Changing `rendering.gpu`: Info toast "Restart tasklens to use the new GPU" [S].
  - The Additional Setup window is not in this menu [S R-028].
- **UI-SHL-040a Rendering GPU submenu**: rows "Auto", one row per adapter name, "Software renderer". On open, rows "Auto" and "Software renderer" show at once, plus a disabled row "Detecting adapters…" [C]; a background task runs the short-lived wgpu adapter probe (APP-046) and calls `PopupMenu::rebuild` with the adapter rows when it returns (timeout 2 s, after which the row reads "No adapters found") [C]. Each adapter row stores `pci:<bus id>`.
- **Tests**: `settings_menu_refresh_radio`: choosing "2.0s" writes 2000 and the engine receives `EngineSettings.interval_ms = 2000`. `start_minimized_disabled`: disabled while `startup.start_at_login = false`.
- **Phase**: P1 (as listed per row).
- **Decided here**: indent 16; Save history disabled rule; toasts for Record history and autostart failure; Default tab lists built tabs only; adapter probe UX.

### UI-SHL-041 Help menu

- **Implements**: APP-024, APP-030, APP-041, APP-042, APP-053, R-028.
- **Items** (Alt+H):

  | # | Label | Shortcut | Action | Phase |
  |---|---|---|---|---|
  | 1 | Keyboard Shortcuts | Ctrl+? | `OpenShortcuts` (UI-SHL-095) | P2 |
  | 2 | System Information… | | `OpenSystemInformation`: selects the Information tab and scrolls to the "Operating System" section [S APP-041] | P2 |
  | 3 | Additional setup… | | `OpenAdditionalSetup` (UI-SHL-094) | P3 |
  | 4 | Documentation | | `OpenDocumentation`: `xdg-open /usr/share/doc/tasklens/README.md` through the shared launcher; failure Error toast "Could not open the documentation" [C] | P4 |
  | — | separator | | | P2 |
  | 5 | About tasklens… | | `OpenAbout` (UI-SHL-092) | P2 |

  - In P1 the Help menu button is absent because it has no items [C, from the "no dead items" rule].
- **Phase**: P2.
- **Decided here**: documentation failure toast; Help absent in P1.

### UI-SHL-042 Debug menu

- **Implements**: APP-026, R-071, ADR-014 (frame stats).
- **Shown**: only with `--debug` or `TASKLENS_DEBUG=1` [S]. Button label "Debug", after Help, Alt+D [C].
- **Items**:

  | # | Label | Kind | Action | Notes |
  |---|---|---|---|---|
  | 1 | Show FPS | check (`debug.show_fps`) | `ToggleSetting(DebugShowFps)` | Overlay UI-SHL-140 |
  | 2 | Continuous rendering | check (session only) | `ToggleContinuousRendering` | Requests a frame every vsync through `request_animation_frame` on the main window (debug only) [S] |
  | 3 | Enable all columns | item | `EnableAllColumns` | Makes every column of the active table visible; Info toast "All columns enabled" [C] |
  | 4 | Add random process minigraphs | item | `AddRandomMinigraphs` | Toasts "Added %ld random process minigraphs." / "No process minigraphs available." [S] |
  | — | separator | | | |
  | 5 | Collector health | item | `OpenCollectorHealth` | UI-SHL-099 |

- **Phase**: P4 (menu); "Show FPS" exists from P1 as a developer flag reachable by `--debug` [S].
- **Decided here**: Alt+D; "All columns enabled" toast.

---

## 5. Status chip

### UI-SHL-050 Status chip (per-tab status text)

- **Implements**: APP-029, PROC-051, PROC-050, SYS-016, SYS-027, SYS-036, R-042, table (d) counts.
- **Form**: APP-029 calls for a compact 22 px bar built on gpui-kit `StatusBar`. The TaskSlinger reference draws a floating chip over the bottom-left of the table instead [M processes.png: chip at x 0..112, y 813..840, 1 px border, rounded top-right corner], and shows nothing on Performance and Information [M performance.png, information.png]. This spec follows the reference: a floating chip, hidden when it has no text [C, open item 2].
- **Anatomy**:

```text
                                        (page content continues under the chip)
+------------------------------+ <- top-right corner radius r_md; other corners square (flush with window edges)
| 296 Processes | 3 processes selected [PAUSED - Ctrl]  · GPU data stale |
+------------------------------+
x=0, bottom=0
```

  - Position: absolute, left 0, bottom 0 of the page area [M].
  - Height 26 [M 27 including border] [C rounded to 26]. Padding-x 12 (`sp5`) [M text starts at x 6.5 logical from the 1 px border, measured 7]: use left padding 8 (`sp4`) and right padding 12 [C].
  - Background `tab_bg` [M #202224]; border 1 physical px `divider` on top and right sides only [M]; top-right corner radius `r_md` [M].
  - Text `t_sm` [C from M cap height] in `text_primary`, tabular figures for the counts.
  - Segments, separated by " | " in `text_secondary` [S PROC-051 " | "]:
    1. Tab text (below).
    2. "[PAUSED - Ctrl]" in `accent` [S R-042], preceded by one space, while Ctrl-paused.
    3. Health items (UI-SHL-051) in `status.warning`, preceded by " · " [C].
  - Width: fits content; max width = page width − 200 [C], text truncated at the end with "…".
- **Tab text** (owners in brackets):
  - Processes [PROC-051]: "296 Processes"; while filtering "41 / 296 Processes"; then " | 3 processes selected" when 2 or more rows are selected; then " | 42 restricted" when restricted rows exist (clickable from P3, tooltip "Some values for other users' processes need administrator rights. Click to enable.").
  - History mode: the process spec's history text (PROC-088 owns it); if it defines none, the chip shows the Processes text for the history table [C].
  - Services [SYS-016], Startup Apps [SYS-027], Connections [SYS-036]: their own strings.
  - Performance, Information: empty [S].
  - Counts use thin-space grouping, for example "12 345 Processes" (U+2009) [S table d].
- **Visibility**: the chip is hidden when all three segments are empty (so Performance and Information normally show nothing). It appears on any tab while paused or while a health item exists [C].
- **Overlap with content**: the Processes, Services, Startup Apps and Connections tables add bottom padding equal to the chip height (26 × scale) inside their scroll content, so the last row can scroll clear of the chip [C]. Minigraph cards (PERF-080) sit bottom-right and do not collide at 800 px width with the 200 px reserve [C].
- **States**:
  - Hover over the restricted segment (P3) or a health item: underline and pointer arrow cursor stays default (widgets rule 7: not a link) [C]; tooltip shows.
  - Pressed: nothing visible beyond the click action.
- **Interaction**: click on " | 42 restricted" (P3) dispatches `UnlockPrivileged` [S]. Click on a health item dispatches `OpenCollectorHealth` [S APP-029]. Right-click: nothing.
- **Data binding**: the active page provides `fn status_text(&self, out: &mut String)`; `StatusChip` re-renders when the page notifies with `StatusChanged` (pages emit it only when their counts or selection count change), on Ctrl-pause flips, and on `ComponentMask::HEALTH`.
- **Build**: `StatusChip` Entity in `window/status.rs`, positioned by `MainWindow` with `.absolute().left_0().bottom_0()`, embedded `.cached(..)`. Text is one `SharedString` rebuilt into a reused `String` buffer only when inputs change.
- **Performance**: at most one text shaping per change; with 1 Hz sweeps the count changes at most once per second.
- **Accessibility**: the chip text is readable as plain text; the clickable parts have tooltips and act on Enter when reached through the keyboard (they are Tab stops only when clickable) [C].
- **Tests**: `status_chip_processes_text`: 296 rows, filter matching 41, 3 selected: text "41 / 296 Processes | 3 processes selected". `status_chip_hidden_on_performance`. `status_chip_paused`: Ctrl held with the window active appends "[PAUSED - Ctrl]".
- **Phase**: P1.
- **Decided here**: chip form instead of a full-width bar (open item 2); sizes; segment separator " · " for health; visibility rule; table bottom padding.

### UI-SHL-051 Health items in the chip

- **Implements**: APP-029, ADR-015 (no toast for collection errors), R-071, PERF-013 words.
- **Behaviour**:
  - Source: `Snapshot.health` (`HealthSnapshot`, one `SourceHealth` per `SourceId`).
  - An item is shown for each source that the active tab or a visible surface uses and whose state is Stale, Failed or Disabled. Texts [C, extending APP-029's examples]:
    - Stale: "<Source> data stale", for example "GPU data stale" [S example].
    - Failed: "<Source> unavailable".
    - Disabled (restart budget spent, ADR-004): "<Source> paused after errors".
  - Source display names: CPU, Memory, Disks, Network, GPU, Sensors, Processes, Services, Startup, Connections, Inventory, KWin [C].
  - At most two items show; more become "+N more" [C].
  - Restricted process counts are not repeated here; they are in the Processes tab text (PROC-051) [C].
  - Tooltip on each item: the source's `last_error` text, or "No new data for 12 s" for Stale [C].
  - Healthy: no item [S].
- **Tests**: `health_items_limit`: three failed sources: two items plus "+1 more".
- **Phase**: P1.
- **Decided here**: the three state texts, source names, the limit of two, tooltips, restricted count excluded.

---

## 6. Settings window

### UI-SHL-060 Settings window frame

- **Implements**: APP-023, APP-045, R-028.
- **Anatomy**:
  - Separate top-level window, title "tasklens Settings" [S], `WindowKind::Floating` with the main window as parent, not modal [S APP-045].
  - Default size 900 × 640, minimum 720 × 480 [C]. Opens centred on the main window where the compositor allows (Wayland places it; tasklens requests no position) [C].
  - Frame: `TlWindowFrame` (UI-SHL-090) with title, pin and close.
  - Body: gpui-kit `Settings::new("settings").sidebar_width(px(220.))` [S widget; C width]: left sidebar with a search field on top and the page list, right the active page.
  - Pages in order [S]: General, Appearance, Motion, Units, Processes, Performance, Graphs, Services, Startup and window, Privileges, Advanced. Page icons (Lucide, 16): `settings`, `palette`, `activity`, `scaling`, `list-tree`, `gauge`, `chart-area` (falls back to `activity` if missing) [C], `cog` → use `settings-2`, `app-window`, `shield`, `sliders-horizontal`.
  - Page content: groups (`SettingGroup` with a title, `t_sm` semibold `text_secondary`), items (`SettingItem` title `t_base` `text_primary`, description `t_sm` `text_secondary`), control at the right.
- **Rules**:
  - Every key of `settings.toml` in table (a) appears exactly once, with the same label as its menu item where one exists [S]. `state.toml` keys never appear [S].
  - Changes apply live, except `appearance.decorations` and `rendering.gpu`, which show the description "Takes effect after restart" under the item and push their restart toast on change [S].
  - Search (kit) matches item titles, descriptions and the keywords listed per item below.
  - Opening it again (Ctrl+, or "All settings…") focuses the existing window [C].
  - Esc closes the window only if `window.close_with_esc` is on [C, the global Esc rule applied to this window].
  - Controls bind to `TlSettings` through the kit `SettingField` get and set closures (`Fn(&App) -> T`, `Fn(T, &mut App)`), which call `TlSettings::update`. Sliders persist on release, like the menu sliders.
- **Build**: `windows/settings.rs`, Entity `SettingsWindow` holding the kit `Settings` builder output; slider and input state entities created in `SettingsWindow::new`.
- **Performance**: the window renders only on input and on `TlSettings` changes; it subscribes to no snapshot data except the device catalog (Performance page device lists) and the GPU engine list.
- **Accessibility**: Tab moves through the search field, the page list, then the page's controls in reading order; Up and Down move in the page list.
- **Tests**: `settings_every_key_once` (unit over the page model): the set of keys bound by all items equals the set of `settings.toml` keys in table (a) (with `fans.*` counted once for the Fans group), with no duplicates. `settings_restart_toast`: toggling "Use system title bar" pushes "Restart tasklens to apply the title bar change".
- **Phase**: P2 (window); each item from its feature's phase.
- **Decided here**: size, sidebar width, icons, refocus, Esc rule.

### UI-SHL-061 Settings page: General

| Group | Label | Control | Key | Values and labels | Phase |
|---|---|---|---|---|---|
| Startup | Default tab | dropdown | `general.start_tab` | Processes, Performance, Services, Startup Apps, Connections, Information, Last used | P2 |
| Language | Language | dropdown | `general.language` | System default; English (United States); Deutsch; Español; Français; 日本語; 한국어; Português (Brasil); Русский; Українська; 简体中文; Norsk bokmål | P2 (P4 for non-English) |

- Language description: "Numbers and dates keep the same format in every language." [C, R-081].
- **Decided here**: locale display names in their own language.

### UI-SHL-062 Settings page: Appearance

| Group | Label | Control | Key | Range or values | Phase |
|---|---|---|---|---|---|
| Theme | Theme | dropdown of all theme names | `theme.active` | built-ins then customs | P2 |
| Theme | Sync theme with system | switch | `theme.follow_system` | | P2 |
| Theme | Preferred Light Theme | dropdown | `theme.light` | all themes | P2 |
| Theme | Preferred Dark Theme | dropdown | `theme.dark` | all themes | P2 |
| Size | UI Scale | slider with "%d%%" | `appearance.ui_scale_pct` | 75..=200 step 5 | P2 |
| Size | Row Height | slider | `appearance.row_height_pct` | 80..=150 step 5 | P2 |
| Look | Rounded Corners | switch | `appearance.rounded_corners` | | P2 |
| Look | Value Color Coding | switch | `appearance.heat_coloring` | | P2 |
| Look | Value Color Coding Intensity | slider | `appearance.heat_intensity_pct` | 0..=100 step 5 | P2 |
| Look | Drop Shadow Intensity | slider | `appearance.shadow_intensity_pct` | 0..=100 step 5 | P2 |
| Window | Use system title bar | switch (on = `server`) | `appearance.decorations` | client, server; "Takes effect after restart" | P2 |

### UI-SHL-063 Settings page: Motion

| Label | Control | Key | Range | Phase |
|---|---|---|---|---|
| Animations | switch | `motion.animations` | | P2 |
| Smooth Graphs | switch | `motion.smooth_graphs` | | P2 |
| Smooth Scroll | switch | `motion.smooth_scroll` | | P2 |
| Smooth scroll amount | slider "%d" | `motion.smooth_scroll_amount` | 1..=50 step 1; disabled while Smooth Scroll is off | P2 |
| Tab Switch | slider "%d ms" | `motion.tab_switch_ms` | 0..=500 step 10 | P2 |

- Page description at the top: "Your system's reduced motion setting turns all of these off." [C, R-017]. While reduced motion is on, the page shows an Info line "Reduced motion is on in your system settings" [C].

### UI-SHL-064 Settings page: Units

| Label | Control | Key | Values | Phase |
|---|---|---|---|---|
| Network transfer unit | dropdown | `units.network` | Bits per second, Bytes per second | P2 |
| Auto-scale network transfer rates | switch | `units.network_auto_scale` | | P2 |

### UI-SHL-065 Settings page: Processes

| Group | Label | Control | Key | Values | Phase |
|---|---|---|---|---|---|
| Refresh | Refresh | dropdown | `processes.interval_ms` | 0.25s, 0.5s, 1.0s, 2.0s, 5.0s, 10.0s | P2 |
| View | Process View | dropdown | `processes.mode` | Tree View, Task Manager Grouping, List View | P2 |
| View | Tree Connector Lines | switch | `processes.tree_guide_lines` | | P2 |
| View | Highlight New and Exited Processes | switch | `processes.highlight_new_exited` | | P2 |
| View | Include children | switch | `processes.filter_include_children` | | P2 |
| View | Hide system processes | switch | `processes.hide_system` | | P2 |
| View | Show subtree totals on parent processes | switch | `processes.subtree_totals` | | P2 |
| Tables (every table) | Resize Behavior | dropdown | `tables.column_resize` | Proportional, Last Column | P2 |
| Tables (every table) | Show column separators | switch | `tables.column_separators` | | P4 |
| Tables (every table) | Dim zero values | switch | `tables.dim_zero_values` | | P2 |
| Properties windows | Hide unnamed handles | switch | `properties.hide_unnamed_handles` | | P2 |
| Properties windows | Shared libraries only | switch | `properties.shared_objects_only` | | P2 |
| Properties windows | Mask secret environment values | switch | `properties.env_mask` | | P2 |
| Dumps | Dump type | dropdown | `processes.dump_type` | Mini, Full | P3 |
| History | Record history | switch | `history.enabled` | | P3 |
| History | History storage | dropdown | `history.retention_h` | 1, 2, 3, 4, 6, 8, 12 hours | P3 |
| History | Save history to disk | switch | `history.persist` | disabled while Record history is off | P3 |
| History | History disk limit | number input, suffix "MiB" | `history.max_disk_mb` | 64..=4096 step 64 [C] | P3 |

- **Decided here**: label "Mask secret environment values"; the disk limit range 64..=4096 (table a gives only the default 256 and says "not in menus", so the window shows it).

### UI-SHL-066 Settings page: Performance

| Group | Label | Control | Key | Values | Phase |
|---|---|---|---|---|---|
| Graphs | Graph time range | dropdown | `performance.graph_range_s` | 30 seconds, 1 minute, 5 minutes (P2), 10 minutes (P2) | P2 |
| Graphs | Curved lines | switch | `performance.curved_lines` | | P2 |
| Graphs | Scale network graphs dynamically | switch | `performance.network_dynamic_scale` | off = link speed | P2 |
| CPU | CPU graph | dropdown | `performance.cpu_graph_mode` | Overall, Per core, All cores overlaid, All cores stacked | P2 |
| CPU | Kernel times | switch | `performance.cpu_kernel` | | P2 |
| CPU | Secondary graph | dropdown | `performance.cpu_secondary_graph` | None, Power draw, Clock speed, Temperature | P2 |
| Memory | Show memory composition | switch | `performance.memory_composition` | | P2 |
| Memory | Show swap usage | switch | `performance.memory_swap_graph` | | P2 |
| GPU | Optional GPU graphs | two checkboxes "Power", "Clocks" | `performance.gpu_optional_graphs` | subset of power, clocks | P2 |
| GPU | Hidden engine graphs | custom: checklist per GPU of its engines (checked = shown) | `performance.hidden_gpu_engines` | engine keys | P2 |
| Devices | Disks / GPUs / Fans / Batteries | four switches | `performance.show_disks`, `show_gpus`, `show_fans`, `show_batteries` | | P2 |
| Devices | Wired / Wireless / VPN and tunnel / Virtual / Other | five switches, indented 16 under a "Network adapters" label | `performance.show_network_wired`, `_wireless`, `_vpn`, `_virtual`, `_other` | | P2 |
| Devices | Show disconnected fans | switch | `performance.show_disconnected_fans` | | P3 |
| Sidebar | Device order and visibility | custom list (below) | `performance.device_order`, `performance.device_overrides` | | P2 |
| Fans | one row per fan: label input and temperature sensor dropdown | custom (below) | `fans.<key>` | | P3 |

- **Device order list** [C]: one row per device in the catalog (name, type), 28 tall, with a drag handle (Lucide `grip-vertical`) to reorder, and a three-way segmented control "Default", "Show", "Hide" bound to `device_overrides` (Default removes the key). Buttons below: "Reset order" (clears `device_order`, toast "Sidebar order reset") and "Show all" (clears `device_overrides`). Keyboard: Alt+Up and Alt+Down move the focused row.
- **Fans group** [C]: per fan, a text input "Label" (empty = default name) and a dropdown "Temperature sensor" listing hwmon temperatures as "<label> (<chip>) 45 °C", plus "None". Writes `fans."<key>" = { label, temp }`; an empty label and "None" remove the entry.
- **Decided here**: labels "Kernel times", "Secondary graph", "Hidden engine graphs", the device list control, the fans group form.

### UI-SHL-067 Settings page: Graphs

| Label | Control | Key | Phase |
|---|---|---|---|
| CPU Graph | switch | `graphs.minigraph_cpu` | P2 |
| Memory Graph | switch | `graphs.minigraph_memory` | P2 |
| Disk Graph | switch | `graphs.minigraph_disk` | P2 |
| Network Graph | switch | `graphs.minigraph_network` | P2 |
| GPU Graph | switch | `graphs.minigraph_gpu` | P2 |
| VRAM Graph | switch | `graphs.minigraph_vram` | P2 |
| GPU Temperature Graph | switch | `graphs.minigraph_gpu_temperature` | P2 |
| Keep detached graphs open when closing main window | switch | `graphs.keep_detached_on_close` | P3 |

- A "Reset Minigraph Layout" button sits below the list (same Action as the menu) [C].

### UI-SHL-068 Settings page: Services

| Label | Control | Key | Values | Phase |
|---|---|---|---|---|
| Unit types | ten checkboxes in two columns | `services.unit_types` | Services, Sockets, Timers, Mounts, Automounts, Paths, Targets, Swaps, Slices, Scopes (stored as `service`, `socket`, `timer`, `mount`, `automount`, `path`, `target`, `swap`, `slice`, `scope`) | P2 |
| Show only these states | four checkboxes | `services.filter_states` | Running, Failed, Stopped, Disabled; none checked = all states | P2 |

- Unchecking the last unit type is refused with the Info toast "At least one unit type must stay selected" [C].

### UI-SHL-069 Settings page: Startup and window

| Group | Label | Control | Key | Phase |
|---|---|---|---|---|
| Window | Close with Esc | switch | `window.close_with_esc` | P2 |
| Window | Always On Top | switch | `window.always_on_top` (same Action as the pin) | P2 |
| Tray | Close to tray | switch | `tray.close_to_tray` | P3 |
| Tray | Minimize to tray | switch | `tray.minimize_to_tray` | P3 |
| Tray | Always show tray icon | switch | `tray.always_show` | P3 |
| Login | Start at login | switch | `startup.start_at_login` | P2 |
| Login | Start minimized | switch, indented 16, disabled unless Start at login is on | `startup.start_minimized` | P2 |
| Login | Open with Ctrl+Esc | switch, no settings key; reads and writes kglobalaccel (APP-051); hidden outside KDE | (none) | P4 |
| Network | Auto-fetch Public IP on launch | switch | `network.auto_fetch_public_ip` | P2 |
| Memory | Open rescue window on critical memory pressure | switch | `lowmem.enabled` | P3 |

- "Open with Ctrl+Esc" on failure to stick (U-16 fallback) shows the description "Log out and back in to apply" [S R-080].

### UI-SHL-070 Settings page: Privileges

| Label | Control | Key | Phase |
|---|---|---|---|
| Unlock privileged actions at launch | switch | `privilege.unlock_at_launch` | P3 |
| Skip password prompts (experimental) | switch (`TogglePasswordless`, with the confirm window) | `privilege.passwordless` | P4 |
| Additional setup… | button | opens UI-SHL-094 | P3 |

- The page shows the helper status line "Administrator helper: Installed" or "Not installed" [C, same check as the setup window row].

### UI-SHL-071 Settings page: Advanced

| Label | Control | Key | Phase |
|---|---|---|---|
| Rendering GPU | dropdown (Auto, adapters, Software renderer), "Takes effect after restart" | `rendering.gpu` | P4 |
| Do not ask again for public IP fetches | switch | `network.public_ip_notice_accepted` | P2 |
| Show FPS | switch | `debug.show_fps` | P2 |
| Import from TaskSlinger… | button (APP-034: portal file chooser for `UserSettings.ini` or `themes.json`; toast "Imported N settings and M themes") | (none) | P4 |
| Open settings folder | button: `xdg-open $XDG_CONFIG_HOME/tasklens` | (none) [C] | P2 |
| Reset Settings to Defaults | button (danger style), same confirm as the menu | (none) | P2 |

- **Decided here**: the "Open settings folder" button; Show FPS is always listed here so the key appears once.

### UI-SHL-072 Reset settings confirmation

- **Implements**: APP-022.
- **Behaviour**: `ResetSettings` opens the generic confirm window (UI-SHL-091) titled "tasklens", text "Reset all settings to their defaults? Themes and history are kept.", buttons "Reset" (danger, default) and "Cancel" [S]. On Reset: `settings.toml` is replaced by defaults (atomic write), `TlSettings` reloads, the theme and scale re-apply, `EngineSettings` re-sent, Success toast "Settings reset to defaults"; on write failure Error toast "Failed to reset settings" and nothing changes [S]. `state.toml`, `themes.json` and history are untouched [S].
- **Tests**: `reset_settings_keeps_state`: after reset, `UiState` columns and window size unchanged, `settings.ui_scale_pct == 100`.
- **Phase**: P1.
