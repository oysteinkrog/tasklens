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
