# tasklens UI spec: application shell

This file specifies the main window shell of tasklens down to the pixel, key and Action. It covers the window frame, the client-drawn title bar, the toolbar, the main tab strip, every menu, the status chip, the filter box, the Settings window, theme management, the shell's secondary windows, toasts, startup, single instance, the main tray icon, the focus model, the keyboard map and runtime scale changes.

ID prefix: UI-SHL. Phase labels follow R-072 (P0 to P4).

## Inputs and precedence

- Binding, in order: `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-103, canonical tables a to d), `docs/ARCHITECTURE.md` (ADR-009 and ADR-010 above all), then the merged specs `docs/spec/merged/sys-app.md` (APP entries), `perf.md` and `proc.md` (their shell parts).
- Tokens and components come from `docs/ui/foundation.md`. This file never defines a colour or token value. It names `TlTheme` fields (`titlebar`, `tab_bg`, `accent` and so on), status colours (`status.danger` and so on), spacing `sp1` to `sp8`, text `t_xs` to `t_xl`, radii `r_sm` and `r_md`, motion `m_fast`, `m_base`, `m_slow` and `ease_out_cubic`.
- Components are the foundation's types (`docs/ui/foundation.md` section 2): `TlButton` with its kinds `Primary`, `Secondary`, `Danger`, `Ghost`, `Icon`, `Toggle { on }`, `Segment { on }` (UI-FND-010), `TlCheckbox`, `TlSwitch`, `TlSlider`, `TlTextInput` and `TlFilterBox` (UI-FND-016), `TlSelect`, `TlTooltip`, `TlToast` and its `ToastQueue` (UI-FND-020), `TlDialog` and `TlSecondaryWindow` (UI-FND-021), `TlMenu` (UI-FND-022), `TlSpinner`, `TlSkeleton`, `TlShield` (UI-FND-028), `TlFocusRing`, plus the kit `Kbd` (no `TlKbd` wrapper). Component visuals (heights, fills, hover and pressed looks, focus rings) are the foundation's; this file sets only placement, size overrides it names explicitly, content and behaviour.
- Theme state is the foundation's `ThemeStore` Global (UI-FND-040); metrics are `UiMetrics` (UI-FND-003), including icon sizes `i_xs` 12, `i_sm` 14, `i_md` 16, `i_lg` 20.

## Conventions used in every entry

- **Sizes** are logical px at `ui_scale_pct = 100`. Every size in this file is multiplied by `ui_scale = ui_scale_pct / 100` unless the entry says "fixed". Hairlines (1 px dividers, 1 px borders) are 1 physical px and are never scaled.
- **Provenance tags** after a value: `[M]` measured from a reference image, `[S]` taken from a spec, ADR, R or D entry, `[C]` chosen here (also listed under "Decided here" at the end).
- **Measurement basis.** `~/work/tasklens-ref/taskslinger/processes.png`, `performance.png` and `information.png` are 2720 × 1680 physical px of a 1360 × 840 logical window at scale 2.0. A measured value is the physical span divided by 2, rounded to the nearest logical px. Colours sampled from those images (for example the title bar `#27292B`, tab strip `#202224`, header `#2B3134`, accent `#0077A6`) only identify which `TlTheme` slot a region uses; values belong to the foundation file.
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
| foundation `components/window_frame.rs`, `components/dialog.rs`, `window/registry.rs` | `TlSecondaryWindow`, `TlDialog`, `WindowRegistry` | 090, 091 |
| `window/{settings.rs, theme_editor.rs, share_theme.rs, about.rs, crash_report.rs, setup.rs, shortcuts.rs, public_ip.rs, collector_health.rs}` | one Entity each | 060 to 099 |
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
  - `start_hidden` is true when `--start-minimized` was given and a tray setting is on (APP-048). If `--start-minimized` is given and no tray setting is on, the window opens shown and `window.minimize_window()` runs after the first frame callback [S APP-048]. `tray_available(cx)` returns `TrayModel.available` and is `false` until E31 fills it; without a tray, a close that would quit minimizes instead when `ToggleMainWindow` arrives while the window is visible (R-219).
- **States**: none visible.
- **Data binding**: reads `appearance.decorations` once at startup (a change needs a restart, UI-SHL-006). Writes `UiState.window`.
- **Build**: `window/main.rs` function `open_main_window(cx, settings, state) -> Result<WindowHandle<MainWindow>>`.
- **Performance**: bounds observation is O(1) per configure event; the debounce task is replaced, not stacked (keep one `Task<()>` field and overwrite it).
- **Tests**: `geometry_restore_clamps` (unit): state 640 × 400 gives bounds 800 × 500; `geometry_save_debounced` (TestAppContext): three bound changes within 200 ms write `UiState` once, 500 ms after the last.
- **Phase**: P1.

### UI-SHL-003 Client-drawn title bar layout

This is the mode with "Use system title bar" off. The default is the server mode in UI-SHL-006 (D-035).

- **Implements**: APP-001, APP-002, APP-008, D-006, R-031, PERF-014, PROC-016.
- **Anatomy** (height 34, bg `titlebar`; x positions measured on `processes.png` at 1360 px width):

```text
|12|16|8| File |1| View |1| Settings |1| Help |1|4|[◈][▦][☰][+]|  drag area (flex)  |[pin][ - ][ □ ][ × ]|
 pad ic gap  menu buttons h=26, px=16, t_md     sep  toolbar 24×24, gap 7          caption 36×34 each
```

  Part list, left to right:
  1. Left padding 12 [S gpui-kit Linux padding].
  2. App icon: 16 × 16 [S APP-002], app SVG `assets/icons/tasklens.svg`, vertically centred. Not interactive except as drag area (mouse-down propagates). Measured reference has no icon (TaskSlinger draws none); APP-002 requires it [S wins].
  3. Gap 8 (`sp4`).
  4. Menu buttons "File", "View", "Settings", "Help" (and "Debug" when enabled, UI-SHL-042): UI-SHL-030. Each is 26 tall, padding-x 16 (`sp6`) [M: File, View, Settings, Help centres at 35, 105, 184, 262; separators at 71, 141, 228, 297], label `t_md` at weight 400 in `text_primary` [C from M: cap height 11 logical px].
  5. Between two menu buttons, and after the last one: a vertical separator 1 physical px wide, 16 tall [M], colour `divider`, vertically centred.
  6. Gap 4 (`sp2`) [M 5].
  7. Toolbar (UI-SHL-010): `TlButton` 24 × 24 (UI-FND-010), gap 7 so the pitch equals the measured 31 [M], icon `i_md` 16 [M 15].
  8. Drag area: `flex_1()`, minimum 24, empty. This is the only purpose-built drag region; every other part that does not stop mouse-down also drags (icon, separators, padding).
  9. Caption buttons (UI-SHL-005): pin, minimize, maximize or restore, close. Each 36 wide [M: centres at 1234, 1270, 1306, 1342] × 34 tall, flush to the right edge.
- **States**:
  - Window active: as above.
  - Window inactive: menu labels and toolbar icons in `text_secondary`; title bar background unchanged (UI-SHL-007).
  - Debug menu present: "Debug" button after "Help", with its own separator.
- **Interaction**: see UI-SHL-004 (drag, double-click, window menu), UI-SHL-030 (menus), UI-SHL-010 (toolbar), UI-SHL-005 (caption buttons).
- **Data binding**: `TitleBarView` re-renders when: the active tab changes (toolbar content), `TlSettings` sections `window` (pin) or `debug` change, the helper state changes (`HealthSnapshot.helper`, `ComponentMask::HEALTH`), the KWin bridge availability changes (`ComponentMask::WINDOWS` validity flip only), window activation changes, or `window.is_maximized()` flips.
- **Build**: `TitleBarView` (Entity) in `window/titlebar.rs`, rendered through gpui-kit `TitleBar::new()` for drag, double-click and window-menu handling, with `.child(..)` for parts 1 to 8. The kit's own window controls are suppressed: `TitleBar` draws controls only under `Decorations::Client`, so tasklens renders its own caption buttons (part 9) as the last child and sets the kit controls off by using `TitleBar::new().controls(false)` if that builder exists, or otherwise by wrapping a plain `h_flex` that copies the kit's drag logic (`title_bar.rs:326-396`: `should_move` state, `start_window_move` on next mouse move, `zoom_window` on double-click, `show_window_menu(pos)` on right-click). The implementer checks `title_bar.rs` in gpui-component 0.7.0; if there is no switch to hide the kit controls, use the copied-logic `h_flex` (decided: own drag code, about 60 lines, `window/titlebar.rs::drag_handlers`).
  - Embedded in `MainWindow` with `.cached(..)`.
  - Height constant `TITLEBAR_H = 34.0`, multiplied by `ui_scale`.
- **Performance**: no snapshot subscription at all except the `WINDOWS` validity flip (checked in the hub subscriber with one comparison). Renders a fixed number of elements (about 20). Strings are `SharedString` constants created once.
- **Accessibility**: every toolbar and caption button has an accessible name equal to its tooltip text. Keyboard access to menus is F10 and Alt+letter (UI-SHL-030). The title bar has no Tab stops (TaskSlinger has none; toolbar buttons are reachable by their Actions and menus) [C].
- **Tests**: `titlebar_parts_order` (render test): children in order icon, File, View, Settings, Help, toolbar, drag, pin, minimize, maximize, close. `titlebar_toolbar_by_tab`: Processes shows 4 toolbar buttons (shield and picker only when available), Performance 2, and the drag area width changes accordingly (no reserved space, PROC-016).
- **Phase**: P1 (pin P2, shield P3, picker P3).
- **Decided here**: menu label `t_md` at weight 400 (the token default is 500); toolbar gap 7; caption buttons 36 wide; title bar not a Tab stop; own drag code when kit controls cannot be hidden.

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
  | Pin | Lucide `pin-off` [M: crossed pin] | Lucide `pin` in `accent` on the `TlButton::Toggle` on look (`accent` at alpha 0.25, 1 px `accent` border) | "Always on top" / "Always on top (on)" (UI-FND-021); disabled: "Not supported by this desktop" [S] | `ToggleAlwaysOnTop { window: WindowKey::Main }` |
  | Minimize | Lucide `minus` | | "Minimize" | `MinimizeWindow` |
  | Maximize | Lucide `square` | Lucide `copy` (restore look) when maximized | "Maximize" / "Restore" | `ZoomWindow` |
  | Close | Lucide `x` | | "Close" | `CloseWindow` |

- **States**:
  - Default: transparent background.
  - Hover: background `tab_hover` (pin, minimize, maximize); close hover background `status.danger` with a white icon (same as UI-FND-021).
  - Pressed: hover background with 0.8 opacity [C].
  - Pin on: the Toggle on look across the full 36 × 34 cell, the same look as the secondary-window pin (UI-FND-021) [S APP-008 "accent fill when on"].
  - Pin disabled (no KWin bridge; X11 included, R-177): icon at the foundation's disabled opacity, no hover, tooltip "Not supported by this desktop".
  - Minimize and maximize are hidden (not disabled) when `window.window_controls().minimize` or `.maximize` is false [S APP-008]; the remaining buttons stay flush right.
  - Keyboard focus: caption buttons are not Tab stops [C].
- **Interaction**:
  - Pin: `ToggleAlwaysOnTop { window: WindowKey::Main, caption: "tasklens" }` (R-176) flips `window.always_on_top` and sends `Command::KWin(KWinCommand::KeepAbove { pid: self_pid, caption: None, on })` on KDE; there is no X11 path in v1 (R-177). Settings write is immediate. No toast [C].
  - Minimize: `window.minimize_window()`, or, when `tray.minimize_to_tray` is on and the tray is up, hide the window (UI-SHL-113).
  - Maximize: `window.zoom_window()`.
  - Close: dispatches `CloseWindow`, which follows the close behaviour in UI-SHL-113 (quit, hide to tray, or hide while detached graphs are open).
- **Data binding**: `window.always_on_top`; pin availability from `Snapshot.windows.header.validity` (KDE). `ComponentMask::WINDOWS` triggers a re-render only when that availability flips.
- **Build**: `window/titlebar.rs::caption_buttons()`, the pin a `TlButton::Toggle` and the other three `div().id(..).w(36).h(34)` cells with `.on_click` and the hover fills above (the kit caption buttons are 34 wide, UI-FND-021).
- **Performance**: static elements; no per-frame work.
- **Accessibility**: tooltip text doubles as accessible name; the pin tooltip includes the state.
- **Tests**: `pin_toggles_setting` (TestAppContext, fake engine): click pin; `window.always_on_top == true` and the fake engine received `KWin(KeepAbove{on:true})`. `caption_hides_unsupported`: fake `window_controls` with `maximize: false` renders three buttons.
- **Phase**: P1 (minimize, maximize, close); P2 (pin).
- **Decided here**: caption buttons 36 wide on the main window (secondary windows keep the kit's 34, UI-FND-021); caption buttons not Tab stops.

### UI-SHL-006 Server-decorations variant ("Use system title bar")

- **Default**: this is the default mode (D-035). `appearance.decorations` defaults to `"server"`, and "Use system title bar" is on in a new install. UI-SHL-003 is the mode with the setting off.
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
- **Anatomy**: a horizontal row inside the title bar (UI-SHL-003 part 7): buttons 24 × 24 (UI-FND-010), gap 7 so the pitch equals the measured 31 [M], vertically centred in the 34 px bar. Order, left to right [M processes.png: shield, grid, stacked bars, plus]:

  | Slot | Button | Shown on | Entry |
  |---|---|---|---|
  | 1 | Shield | every tab, from P3, only when `/usr/lib/tasklens/tl-helper` exists | UI-SHL-011 |
  | 2 | Hide system processes (toggle) | Processes tab only (including History mode) | UI-SHL-012 |
  | 3 | View mode cycler | Processes tab only; disabled in History mode | UI-SHL-013 |
  | 4 | Window picker "+" | every tab, from P3, only with the KWin bridge | UI-SHL-014 |

  - A hidden button takes no space (`.when(cond, ..)`, not `.invisible()`), so the drag area grows [S PROC-016].
  - Slot 1 is `TlShield` (UI-FND-028); slots 2 and 4 are `TlButton::Toggle`; slot 3 is `TlButton::Icon`. Icons `i_md` 16.
  - Slots 2 and 3 are filled by the active page's `ToolbarContent`: E20 defines `ToolbarContent` and `ToolbarModel` and draws only the slot frame; each page epic registers one `ToolbarContent` that draws its own buttons (R-194).
- **States** (common to all four): the `TlButton` states of UI-FND-010 (hover `tab_hover`, pressed `header`, Toggle on: `accent` at alpha 0.25 with a 1 px `accent` border, disabled at opacity 0.5 with its tooltip still shown). Toolbar buttons are built with `.tab_stop(false)`: they are not Tab stops [C]; keyboard users reach the same Actions through View > Process View, the Settings menu and table (b) keys. Window inactive: icons `text_secondary`.
- **Interaction**: click dispatches the button's Action (below). Tooltips use `TlTooltip` with the 500 ms delay and, where the Action has a binding, the key hint (`Tooltip::action`).
- **Data binding**: `MainWindow.active_tab`, `UiState.processes.history_mode`, `processes.hide_system`, `processes.mode`, `HealthSnapshot.helper` (UI-SHL-011), KWin availability (UI-SHL-014).
- **Build**: `Toolbar` is a `RenderOnce` struct built inside `TitleBarView::render` from a small `ToolbarModel { tab, history_mode, hide_system, mode, helper: HelperState, picker_available, picking }` that `TitleBarView` keeps up to date.
- **Performance**: at most 4 buttons; no allocations beyond the element tree; tooltip text are `SharedString` constants except the view-mode tooltip, which is one of three precomputed `SharedString`s.
- **Tests**: `toolbar_slots_per_tab` (render test): for each of the six tabs with helper installed and bridge up, assert the visible slot list: Processes [1,2,3,4], every other tab [1,4]. `toolbar_history_mode`: Processes in History mode shows slot 3 disabled with tooltip "History shows a flat list".
- **Phase**: P1 (slots 2 and 3), P3 (slots 1 and 4).
- **Decided here**: gap 7; toolbar buttons not Tab stops.

### UI-SHL-011 Shield (unlock privileged actions)

- **Implements**: APP-003, APP-049 (unlock at launch), R-002, R-003, table (b) "Shift held at launch".
- **Component**: `TlShield` (UI-FND-028) owns the look, states, tooltips, the `StartHelperSession` and `EndHelperSession` Actions, the starting spinner and the failure toasts ("Authentication cancelled", "Could not start the privileged helper"). This entry fixes only its place and launch behaviour.
- **Placement**: toolbar slot 1, every tab, hidden before P3 and when `/usr/lib/tasklens/tl-helper` is missing [S].
- **Launch**: Shift held at launch (UI-SHL-110 step 6) or `privilege.unlock_at_launch = true` dispatches `StartHelperSession` once after the first frame (UI-SHL-110 step 11) [S].
- **Other entry points**: the Processes status chip's " | 42 restricted" segment (UI-SHL-050) and the "Retry as administrator" toast button (PROC-058) dispatch the same Action.
- **Data binding**: `HealthSnapshot.helper` (`ComponentMask::HEALTH`).
- **Tests**: `shield_slot_every_tab` (render test): with the helper present, slot 1 is present on all six tabs.
- **Phase**: P3.

### UI-SHL-012 Hide system processes toggle

- **Implements**: APP-004, PROC-046, R-031.
- **Anatomy**: `TlButton::Toggle` 24 × 24. Icon Lucide `layout-grid` [C; matches the four-square glyph measured in processes.png].
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
- **Anatomy**: `TlButton::Icon` 24 × 24. The icon shows the current mode [C]: `grouped` Lucide `rows-2` [M: two stacked bars in processes.png, default mode], `tree` Lucide `list-tree`, `flat` Lucide `list`.
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
- **Anatomy**: `TlButton::Toggle` 24 × 24 (on while picking), icon Lucide `plus` [M]. Tooltip "Select process by window" [S PROC-055].
- **States**:
  - Hidden: no KWin bridge (`Snapshot.windows.header.validity == Unsupported`) [S].
  - Idle: plain.
  - Picking: toggled-on look (UI-SHL-010) until the pick ends [C].
- **Interaction**:
  - Click dispatches `StartWindowPick`: sends `Command::KWin(KWinCommand::Pick)`, sets `picking = true`, pushes the Info toast "Click a window to select its process. Hold CTRL while clicking to terminate process. Esc cancels." [S PROC-055], and pushes the key context `Picker` on the main window's root focus so Esc maps to `CancelWindowPick`.
  - The result handling (select row, Ctrl+click confirmation, toasts "No window found", "No window selected", "Unknown process") belongs to PROC-055 and is an E22 bead that reads `WindowsSnapshot.pick` and calls `MainWindow::end_window_pick` when done (R-239). Where APP-006 says Ctrl+click ends the process "then gives a toast" and PROC-055 says "after a confirmation", this spec follows PROC-055 (the process spec owns action confirmations, PROC-059) [C].
  - Click while picking: dispatches `CancelWindowPick` [C].
- **Data binding**: `ComponentMask::WINDOWS` (availability); the pick reply arrives in `WindowsSnapshot.pick` as `PickResult { seq, outcome: Picked { pid } | NoWindow | Cancelled }`; the request is `KWinCommand::Pick { seq }`, cancel is `CancelPick { seq }` (R-149).
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
  - Tab: height 30, padding-x 22 [M, from label centres 98, 192, 304, 417, 536, 664 at 1360 px], label `t_md` at weight 400 (the token default is 500) [C from M]. Inactive label `text_primary` [M white text on every tab]; active label `text_primary` medium weight [C].
  - Tab icons (part of the tab, after the label, gap 6 `sp3`, `i_sm` 14 × 14 per UI-FND-003):
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
  - Overflow button: `TlButton::Icon` 24 × 24, Lucide `chevrons-right` [C], placed right after the last visible tab, tooltip "More tabs" [C].
  - Click opens a `TlMenu` listing the hidden tabs with the same labels; choosing one dispatches `SelectTab(i)`.
  - At the minimum width of 800 px with all six tabs, the tabs need about 680 px at 100% and the filter box needs 236, so the overflow appears at 100% scale below about 960 px of window width [C, estimate from M label widths].
- **Build**: part of `TabStrip`; the menu uses `TlMenu` (PopupMenu with `.menu(label, Box::new(SelectTab(i)))`).
- **Tests**: `overflow_keeps_active_tab`: window 800 px, Information active: Information is visible and the menu lists the tabs that do not fit.
- **Phase**: P1.
- **Decided here**: active tab always visible; icon and tooltip.

### UI-SHL-023 History toggle and Information copy icon on tabs

- **Implements**: APP-027, PROC-088, PROC-086 (toast with Enable), D-010, PERF-094.
- **Anatomy**:
  - History icon: `i_sm` 14 Lucide `rotate-ccw-clock` inside the Processes tab after the label, gap 6. The kit's Lucide set has no `history` glyph (UI-FND-003 lists `history`); `rotate-ccw-clock` is the replacement, recorded by the icon bead [C]. Hit area 22 × 22 centred on the icon [C]. Colour `text_secondary`; in History mode `accent` [C].
  - Copy icon: `i_sm` 14 Lucide `copy` inside the Information tab after the label, gap 6, shown only while Information is active [M]. Hit area 22 × 22. Colour `text_secondary`.
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
  - Width: the input is 160 unfocused and 280 focused, and stays 280 while it holds text [S APP-028, UI-FND-016]. With the label and paddings the outlined region is about 220 wide unfocused, which matches the measured 221 at 1360 px [M].
  - Left padding 10 [C] before the "Filter:" label; gap 6 (`sp3`) between label and input.
  - Input: `TlFilterBox` (UI-FND-016, label "Filter:" in its own style) with its 1 px border turned off inside the tab strip, because the accent outline is the border here [C]; fill `tab_bg` unfocused and `input_focus` focused. Placeholder: none [M].
  - Clear button: as UI-FND-016 (Lucide `x` `i_sm`, 20 × 20 hit area), right padding 8, shown only when the text is not empty [S].
  - Shown on every tab except Performance [M performance.png]. Each tab keeps its own filter text for the session [S]; switching tabs swaps the text into the single `InputState` with `set_value` (which does not emit Change) [C].
- **States**:
  - Unfocused empty, unfocused with text (text stays visible, clear button visible), focused (width 280, fill `input_focus`), hover (cursor I-beam over the input part only).
  - Width animation: 160 to 280 on focus, back to 160 on blur when empty, over 150 ms `ease_out_cubic` (UI-FND-016); instant with animations off or reduced motion [S]. The tabs to its left re-layout each animation frame; the overflow rule (UI-SHL-022) applies to the final width only, so no tab jumps into the menu mid-animation [C].
  - Window inactive: outline in `divider`.
- **Interaction**:
  - Click anywhere in the box focuses the input.
  - Ctrl+F (`FocusFilter`, Global) focuses it and selects all its text [C].
  - Type-to-filter (UI-SHL-124): a printable key while a table has focus moves focus here and inserts the character.
  - Typing: `InputEvent::Change` starts or restarts a 100 ms debounce task [S]; when it fires, the active page receives `FilterChanged { tab, text }`. The process page sends `SetTableQuery` with a new generation (ADR-009); other pages filter as their specs say.
  - Esc in the box (`FilterEscape`, handled before the global Esc chain): if text is not empty, clear it (Change fires, debounce runs at once with an empty query) and move focus to the active page's table; if text is empty, move focus to the table and let the global Esc chain continue only on a further Esc [S R-043].
  - Enter: moves focus to the table, keeps the filter, and selects the first row when nothing is selected [C; processes.md UI-PRC-016 agrees].
  - Down arrow: same as Enter [C].
- **Data binding**: `FilterBox { input: Entity<InputState>, per_tab: [SharedString; 6], debounce: Option<Task<()>> }`. Reads only its own state; never subscribes to snapshots.
- **Build**: `FilterBox` Entity in `window/filter.rs`, embedded in `TabStrip`. The width animation is the `TlFilterBox` one (UI-FND-016).
- **Performance**: keystroke cost is the kit input's own (one rope edit, one notify on the InputState) plus one timer reset. The text is read with `text()` (no allocation) and converted to `Arc<str>` lowercased once per debounced query.
- **Accessibility**: the input's accessible name is "Filter"; the clear button is "Clear filter".
- **Tests**: `filter_debounce_100ms` (TestAppContext): type three characters 30 ms apart; exactly one query after 100 ms of quiet, with the latest generation. `filter_esc_rule`: text "fire", Esc: text empty, focus on table; second Esc goes to the global chain. `filter_per_tab_text`: text on Processes survives a switch to Services and back. `filter_hidden_on_performance`.
- **Phase**: P1.
- **Decided here**: inner border off in the tab strip; fills; Enter and Down behaviour; select-all on Ctrl+F.

### UI-SHL-025 "Include children" checkbox placement

- **Implements**: PROC-045, APP-028.
- **Anatomy** [M cus_sheet_03.png: label then checkbox, left of "Filter:"]: label "Include children" `t_sm` `text_primary`, gap 6, then a `TlCheckbox` 16 × 16 (UI-FND-012), then 16 px to the filter box. Vertically centred in the strip.
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
  - Inside an open popup, keys inside one menu are UI-FND-022's. The menu bar adds two:
    - Left at the top level moves to the previous menu, wrapping.
    - Right at the top level, on an item without a submenu, moves to the next menu, wrapping [S kit `SelectLeft`/`SelectRight` in the `MenuBar` context].
  - Esc closes the innermost popup. When the last popup closes, focus returns to the element that had it before the menu opened (widgets rule 16).
  - Activating an item closes the whole menu chain, then dispatches the Action to the focus that was active before the menu opened (`PopupMenu::action_context(previous_focus)`), so table-context Actions (for example Collapse all) reach the table.
  - Pressing and releasing Alt alone does nothing [C] (no Windows-style mnemonic underlines).
- **Data binding**: `MenuBar { open: Option<MenuId>, popup: Option<Entity<PopupMenu>>, return_focus: Option<FocusHandle> }`. Menus are built fresh each time they open, from `TlSettings` and app state at that moment (cheap; widgets perf note).
- **Build**: custom `MenuBar` Entity in `window/menubar.rs`, modelled on gpui-kit `menu/app_menu_bar.rs` (open/hover/left/right logic) but with tasklens-built `PopupMenu`s, because the kit's `AppMenuBar` reads static `OwnedMenu`s from `cx.set_menus` and cannot show live checks, sliders or painted theme rows [C]. Popup shown with `deferred(anchored().anchor(Anchor::TopLeft).snap_to_window_with_margin(px(8.)))`.
- **Performance**: building a menu is about 30 to 60 elements once per open; nothing runs while menus are closed.
- **Accessibility**: each popup row has its label as accessible name; check and radio states are part of it ("Tree View, selected").
- **Tests**: `menubar_hover_switch`: open File, hover View: View open, File closed. `menubar_alt_keys`: Alt+V opens View with "Columns" highlighted. `menubar_left_right_wrap`: in File, Left opens Help (or Debug when present). `menu_action_reaches_table`: focus table, open View > Process View > Collapse all; the table receives `CollapseAll`.
- **Phase**: P1.
- **Decided here**: zero gap under the button; keyboard highlight of the first item; own `MenuBar` instead of kit `AppMenuBar`; Alt alone does nothing; Alt+D for Debug.

### UI-SHL-031 Menu popups in the menu bar

- **Implements**: APP-012 to APP-026 (all menu entries), reference video frames (menus in `cus_sheet_01.png` to `cus_sheet_03.png`).
- **Component**: every menu bar popup is a `TlMenu` (UI-FND-022). Row height 26, check column 20, min width 180, max width 360, check mark `i_xs`, radio mark, header rows (22 high, `t_xs` `text_secondary`), separators, submenu chevrons and placement, hover and disabled looks, the 120 ms fade, keyboard navigation and automatic shortcut hints are all UI-FND-022's.
- **Shell rules on top of UI-FND-022**:
  - Labels written with ">" in the merged spec ("Columns >") are submenus; the ">" itself is replaced by the component's chevron [C].
  - Labels ending in "…" or "..." keep the exact spec text [S].
  - The Settings menu group names (Monitoring, Motion & interaction, Window & startup, Language) are `TlMenu` header rows.
  - Slider rows (View > Appearance, Settings > Tab Switch) are shell rows, specified in UI-SHL-037.
  - Theme rows are `TlMenu::painted_item` rows (UI-SHL-038).
  - The popup's action context is the focus that was active before the menu opened, so table actions such as Collapse all reach the table (UI-SHL-030).
- **Build**: menu builders in `window/menubar.rs` call `TlMenu` only; the one shell helper is `slider_row(..)`.
- **Phase**: P1.
- **Decided here**: chevron replaces ">".

### UI-SHL-032 File menu

- **Implements**: APP-012, APP-013 (dropped), APP-014, APP-015, APP-016, PROC-095, PROC-096.
- **Items** (in order; an item exists only from its phase, so the menu never has dead items [S]):

  | # | Label | Shortcut | Action | Phase | Notes |
  |---|---|---|---|---|---|
  | 1 | Run... | Ctrl+N | `OpenRunDialog` | P2 | Opens or focuses "tasklens Run" (UI-SHL-097) |
  | 2 | Search File Handles... | | `OpenHandleSearch` | P3 | Opens or focuses "tasklens Handle Search" (UI-SHL-098) |
  | 3 | Open System Monitor | | `OpenSystemMonitor` | P4 | Hidden when none of `plasma-systemmonitor`, `gnome-system-monitor`, `missioncenter` is on `PATH` (checked once at startup, cached). Failure toast (Error) "Failed to launch System Monitor" [S] |
  | sep | separator | | | P2 | Only when an item above it exists |
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
  | 1 | Select Columns… | item | `OpenColumnPicker { table }` | Opens the column picker window "tasklens Columns" (processes.md UI-PRC-007, `WindowKey::Columns`), bound to the active tab's table |
  | 2 | Resize columns to fit content | item | `ResizeColumnsToFit { table }` | Template strings per table (d) |
  | 3 | Resize Behavior | submenu | | |
  | 3a | Proportional | radio | `SetSetting { TablesColumnResize, "proportional" }` | |
  | 3b | Last Column | radio | `SetSetting { TablesColumnResize, "last_column" }` | |
  | sep | separator | | | |
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
  | sep | separator | | | |
  | 4 | Tree Connector Lines | check | `processes.tree_guide_lines` | `ToggleSetting(ProcessesTreeGuideLines)` |
  | 5 | Highlight New and Exited Processes | check | `processes.highlight_new_exited` | `ToggleSetting(ProcessesHighlightNewExited)` |
  | sep | separator | | | |
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

- **Slider row anatomy** (UI-FND-014 slider row geometry: height 28):

```text
| [chk col 20] Label (min w=140, t_base, ellipsis) [==========o------] (track 120 x 4)  100% (w=40, right, t_sm tnum text_secondary) |
```

  - Label column at least 140 wide (UI-FND-014), truncated with "…" [M "Value Color Coding I…" in the reference].
  - `TlSlider` track 120 × 4, thumb from UI-FND-014.
  - Value text 40 wide, right-aligned, tabular figures.
- **Interaction**:
  - Dragging previews live: each `SliderEvent::Change` applies the value to `TlSettings` in memory without persisting [S]. `SliderEvent::Release` persists (one save) [S].
  - UI Scale release pushes the Info toast "UI scale changed to 120%" (the value) [S]. Ctrl+Plus, Ctrl+Minus, Ctrl+0 do the same steps from anywhere (UI-SHL-130).
  - Keyboard on a highlighted slider row: Left and Right step by the step value, Home and End jump to min and max; each key press counts as a release (persist plus toast for UI Scale) [C].
  - The popup stays open while dragging and after release; clicking a check row toggles it and also keeps the popup open [C, matches TaskSlinger's Appearance submenu which stays open in the reference video].
  - Applying a UI Scale change re-lays out the menu itself; the popup stays anchored to its row [C].
- **Effects**: UI-SHL-130 (scale and row height), UI-SHL-131 (radius), heat colouring and shadow per their owners.
- **Build**: `slider_row` helper renders a `menu_element` row holding a `TlSlider` (kit `Slider`) whose `SliderState` entities are created once per `MenuBar` and reused across menu opens (entities are never created in render) [C].
- **Tests**: `appearance_slider_persist_on_release`: three Change events then Release: one persist call, final value saved, toast "UI scale changed to 115%".
- **Phase**: P2 (menu), P1 (keys Ctrl+Plus/Minus/0).
- **Decided here**: steps of 5 for the two 0..100 sliders; popup stays open; keyboard on slider rows.

### UI-SHL-038 View > Theme

- **Implements**: APP-035, APP-036, APP-037, APP-038, R-068, R-069; reference `cus_sheet_02.png` (each theme row painted in its own palette, copy icon at right).
- **Items**:

  | # | Label | Kind | Action |
  |---|---|---|---|
  | 1..12 | Default Dark, Vivid Nightfall, Catppuccin Frappé, Catppuccin Macchiato, Catppuccin Mocha, Dracula, OLED Black, Default Light, Windows 7 Aero, Golden Summer Fields, Soft Pink Delight, Catppuccin Latte | theme row (radio) | `ActivateTheme { name }` |
  | 13.. | custom themes from `themes.json`, in file order | theme row (radio) | `ActivateTheme { name }` |
  | sep | separator | | |
  | | Create copy | item | `CopyTheme(active)` |
  | | Edit colors | item | `EditTheme(active)` |
  | | Share theme | item | `ShareTheme(active)` |
  | | Delete theme | item | `DeleteTheme(active)` |
  | sep | separator | | |
  | | Import theme | item | `ImportTheme` |
  | | Sync theme with system | check (`theme.follow_system`) | `ToggleSetting(ThemeFollowSystem)` |
  | | Preferred Light Theme | submenu: every theme name as radio (`theme.light`) | `SetSetting { ThemeLight, name }` |
  | | Preferred Dark Theme | submenu: every theme name as radio (`theme.dark`) | `SetSetting { ThemeDark, name }` |

  - In P1 the list holds only Default Dark and Default Light (R-068).
  - APP-036 lists four per-theme items. This spec places them once, acting on the active theme, so they are reachable from the keyboard without per-row submenus [C]. The copy icon on every row is the per-row "Create copy" shortcut [S].
- **Theme row anatomy**: `TlMenu::painted_item(label, bg, fg, suffix_icon, action)` (UI-FND-022), 26 high:
  - bg = that theme's `menu_bg`, fg = that theme's `text_primary` [M reference shows each row in its own palette].
  - Radio mark on the active theme.
  - Suffix: Lucide `copy` `i_sm`, tooltip "Create copy"; clicking it dispatches `CopyTheme(name)` and does not apply the theme [S].
  - A user theme whose name shadows a built-in shows "(custom)" after its name (UI-FND-040).
  - While the Theme Editor previews a theme, that row shows " (editing)" and `ActivateTheme` is refused with the toast "Finish or cancel the theme editor first" (UI-FND-043).
- **Behaviour**:
  - `ActivateTheme { name }` writes `theme.active`, runs the theme switch (UI-SHL-080) and, if `theme.follow_system` is on, turns it off with the Info toast "Theme sync turned off" [C] (an explicit choice overrides sync).
  - `CopyTheme(name)` creates "<name> copy" (or "<name> copy 2", 3, ...) in `themes.json`, makes it active, Success toast "Theme copied" [S].
  - `EditTheme(active)`: built-in active theme gives Info toast "Copy a theme before editing it" [S]; a custom theme opens the Theme Editor (UI-SHL-081). Absent before P3.
  - `ShareTheme(active)` opens "tasklens Share Theme" (UI-SHL-082). Absent before P3.
  - `DeleteTheme(active)`: built-in gives Info toast "Built-in themes cannot be deleted" [S]; custom opens the confirm window "Delete this custom theme?" (UI-SHL-084).
  - `ImportTheme` reads the clipboard (UI-SHL-083). Absent before P3.
- **Build**: rows read each theme's colours from `ThemeStore` (UI-FND-040), which parses all themes at startup and after every `themes.json` write.
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
  | sep | separator | | | |
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
  | | Start minimized | check, indented by 16, disabled unless Start at login is on; toggling it while Start at login is on calls `AppEnv.set_autostart(true, minimized)` again so the `Exec` line gains or loses `--start-minimized` (R-236) | `startup.start_minimized` | P2 |
  | | Default tab | submenu radio: Processes, Performance, Services, Startup Apps, Connections, Information, Last used | `general.start_tab` | P1 |
  | | Unlock privileged actions at launch | check | `privilege.unlock_at_launch` | P3 |
  | | Skip password prompts (experimental) | check (`TogglePasswordless`) | `privilege.passwordless` | P4 |
  | | Auto-fetch Public IP on launch | check | `network.auto_fetch_public_ip` | P2 |
  | | Open rescue window on critical memory pressure | check | `lowmem.enabled` | P3 |
  | **Language** | Language: | submenu radio: System default, then the locales by their own names (UI-SHL-131) | `general.language` | P1 (P4 for locales other than English) |
  | sep | separator | | | |
  | | All settings… | item, Ctrl+, | `OpenSettings` | P2 |
  | | Reset Settings to Defaults | item | `ResetSettings` (UI-SHL-091 confirm) | P1 |

  - Default tab lists only tabs built in the current phase plus "Last used" [C].
  - Refresh labels are exactly "0.25s", "0.5s", "1.0s", "2.0s", "5.0s", "10.0s" [S].
  - Named Actions with extra work:
    - `ToggleRecordHistory`: writes `history.enabled`; Info toast "History recording on" or "History recording off" [C].
    - `ToggleStartAtLogin`: calls `AppEnv.set_autostart(enabled, minimized)`, which writes or deletes `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop` through tl-app (`write_atomic`), with `--start-minimized` on the `Exec` line when `minimized` is true (R-236) [S APP-048]; failure: Error toast "Could not change the autostart entry" and the setting is reverted [C].
    - `TogglePasswordless`: turning on first opens the modal confirm (UI-SHL-091) with the text "tasklens actions that change other users' processes, system services and network connections will run without asking for a password. Continue?" and buttons "Skip prompts" / "Cancel" [S]; then runs `pkexec /usr/lib/tasklens/tasklens-setup polkit-rule on`; success writes the key, cancel leaves it off with toast "Authentication cancelled". Turning off runs `polkit-rule off` without a confirm [S].
    - Changing `rendering.gpu`: Info toast "Restart tasklens to use the new GPU" [S].
  - The Additional Setup window is not in this menu [S R-028].
- **UI-SHL-040a Rendering GPU submenu**: rows "Auto", one row per adapter name, "Software renderer". On open, rows "Auto" and "Software renderer" show at once, plus a disabled row "Detecting adapters…" [C]; a background task runs the short-lived wgpu adapter probe (APP-046) and calls `PopupMenu::rebuild` with the adapter rows when it returns (timeout 2 s, after which the row reads "No adapters found") [C]. Each adapter row stores `pci:<bus id>` when `AdapterInfo.pci_bus_id` is known, otherwise `name:<adapter name>` (R-237).
- **Tests**: `settings_menu_refresh_radio`: choosing "2.0s" writes 2000 and the engine receives `EngineSettings.interval_ms = 2000`. `start_minimized_disabled`: disabled while `startup.start_at_login = false`.
- **Phase**: P1 (as listed per row).
- **Decided here**: indent 16; Save history disabled rule; toasts for Record history and autostart failure; Default tab lists built tabs only; adapter probe UX.

### UI-SHL-041 Help menu

- **Implements**: APP-024, APP-030, APP-041, APP-042, APP-053, R-028.
- **Items** (Alt+H):

  | # | Label | Shortcut | Action | Phase |
  |---|---|---|---|---|
  | 1 | Keyboard Shortcuts | Ctrl+? | `OpenShortcuts` (UI-SHL-095) | P2 |
  | 2 | System Information… | | `ShowSystemInformation` (declared by E20-002, R-175): selects the Information tab and scrolls to the "Operating System" section [S APP-041] | P2 |
  | 3 | Additional setup… | | `OpenAdditionalSetup` (UI-SHL-094) | P3 |
  | 4 | Documentation | | `OpenDocumentation`: `xdg-open /usr/share/doc/tasklens/README.md` through the shared launcher; failure Error toast "Could not open the documentation" [C] | P4 |
  | sep | separator | | | P2 |
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
  | 4 | Add random process minigraphs | item | `AddRandomMinigraphs` (declared and handled by E26, R-238; this menu only dispatches) | Toasts "Added %ld random process minigraphs." / "No process minigraphs available." [S] |
  | sep | separator | | | |
  | 5 | Collector health | item | `OpenCollectorHealth` | UI-SHL-099 |

- **Phase**: P4 (menu); "Show FPS" exists from P1 as a developer flag reachable by `--debug` [S].
- **Decided here**: Alt+D; "All columns enabled" toast.

---

## 5. Status chip

### UI-SHL-050 Status chip (per-tab status text)

- **Implements**: APP-029, PROC-051, PROC-050, SYS-016, SYS-027, SYS-036, R-042, table (d) counts.
- **Form**: APP-029 calls for a compact 22 px bar built on gpui-kit `StatusBar`. The TaskSlinger reference draws a floating chip over the bottom-left of the table instead [M processes.png: chip at x 0..112, y 813..840, 1 px border, rounded top-right corner], and shows nothing on Performance and Information [M performance.png, information.png]. This spec follows the reference: a floating chip, hidden when it has no text [C, R-107].
- **Anatomy**:

```text
                                        (page content continues under the chip)
+------------------------------+ <- top-right corner radius r_md; other corners square (flush with window edges)
| 296 Processes | 3 processes selected [PAUSED - Ctrl]  · GPU data stale |
+------------------------------+
x=0, bottom=0
```

  - Position: absolute, left 0, bottom 0 of the page area [M].
  - Height 26 [M 27 including the border; C rounded to 26]. Left padding 8 (`sp4`) [M text starts 7 px from the left edge], right padding 12 (`sp5`) [C].
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
- **Interaction**: click on " | 42 restricted" (P3) dispatches `StartHelperSession` [S]. Click on a health item dispatches `OpenCollectorHealth` [S APP-029]. Right-click: nothing.
- **Data binding**: the active page provides `fn status_text(&self, out: &mut String)`; `StatusChip` re-renders when the page notifies with `StatusChanged` (pages emit it only when their counts or selection count change), on Ctrl-pause flips, and on `ComponentMask::HEALTH`.
- **Build**: `StatusChip` Entity in `window/status.rs`, positioned by `MainWindow` with `.absolute().left_0().bottom_0()`, embedded `.cached(..)`. Text is one `SharedString` rebuilt into a reused `String` buffer only when inputs change.
- **Performance**: at most one text shaping per change; with 1 Hz sweeps the count changes at most once per second.
- **Accessibility**: the chip text is readable as plain text; the clickable parts have tooltips and act on Enter when reached through the keyboard (they are Tab stops only when clickable) [C].
- **Tests**: `status_chip_processes_text`: 296 rows, filter matching 41, 3 selected: text "41 / 296 Processes | 3 processes selected". `status_chip_hidden_on_performance`. `status_chip_paused`: Ctrl held with the window active appends "[PAUSED - Ctrl]".
- **Phase**: P1.
- **Decided here**: chip form instead of a full-width bar (R-107); sizes; segment separator " · " for health; visibility rule; table bottom padding.

### UI-SHL-051 Health items in the chip

- **Implements**: APP-029, ADR-015 (no toast for collection errors), R-071, PERF-013 words.
- **Behaviour**:
  - Source: `Snapshot.health` (`HealthSnapshot`, one `SourceHealth` per `SourceId`).
  - An item is shown for each source that the active tab or a visible surface uses and whose state is Stale, Failed or Disabled. Texts [C, extending APP-029's examples]:
    - Stale: "<Source> data stale", for example "GPU data stale" [S example].
    - Failed: "<Source> unavailable".
    - Disabled (restart budget spent, ADR-004): "<Source> paused after errors".
  - Sources per tab (R-217): Processes: `Processes` plus every source a visible minigraph draws; Performance: `Cpu`, `Memory`, `Disks`, `Network`, `Gpu`, `Sensors`, plus `Battery` when a battery page exists; Services: both systemd managers and `Journal`; Startup Apps: `Startup`; Connections: `Connections`; Information: `Inventory`.
  - Source display names: CPU, Memory, Disks, Network, GPU, Sensors, Processes, Services, Startup, Connections, Inventory, KWin, Logind, UDisks, UPower, NetworkManager, Public IP, History, Journal, Battery; any other variant shows its Rust name [C, R-217].
  - At most two items show; more become "+N more" [C].
  - Restricted process counts are not repeated here on the Processes tab; they are in its tab text (PROC-051). On other tabs, for 10 s after a privileged action was refused with EPERM while tl-helper is missing, the item "Restricted: 42 processes" shows (N = refused targets) [S APP-029, UI-FND-028].
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
  - Frame: `TlSecondaryWindow` (UI-SHL-090, UI-FND-021) with title, pin and close.
  - Body: gpui-kit `Settings::new("settings").sidebar_width(px(220.))` [S widget; C width]: left sidebar with a search field on top and the page list, right the active page.
  - Pages in order [S]: General, Appearance, Motion, Units, Processes, Performance, Graphs, Services, Startup and window, Privileges, Advanced. Page icons (Lucide, 16), in page order [C]: `settings`, `palette`, `activity`, `scaling`, `list-tree`, `gauge`, `chart-area`, `server`, `app-window`, `shield`, `sliders-horizontal`.
  - Page content: groups (`SettingGroup` with a title, `t_sm` semibold `text_secondary`), items (`SettingItem` title `t_base` `text_primary`, description `t_sm` `text_secondary`), control at the right.
- **Rules**:
  - Every key of `settings.toml` in table (a) appears exactly once, with the same label as its menu item where one exists [S]. `state.toml` keys never appear [S].
  - Changes apply live, except `appearance.decorations` and `rendering.gpu`, which show the description "Takes effect after restart" under the item and push their restart toast on change [S].
  - Search (kit) matches item titles, descriptions and the keywords listed per item below.
  - Opening it again (Ctrl+, or "All settings…") focuses the existing window [C].
  - Esc closes the window (tool window rule, UI-FND-021).
  - Controls bind to `TlSettings` through the kit `SettingField` get and set closures (`Fn(&App) -> T`, `Fn(T, &mut App)`), which call `TlSettings::update`. Sliders persist on release, like the menu sliders.
- **Build**: `window/settings.rs`, Entity `SettingsWindow` holding the kit `Settings` builder output; slider and input state entities created in `SettingsWindow::new`.
- **Performance**: the window renders only on input and on `TlSettings` changes; it subscribes to no snapshot data except the device catalog (Performance page device lists) and the GPU engine list.
- **Accessibility**: Tab moves through the search field, the page list, then the page's controls in reading order; Up and Down move in the page list.
- **Tests**: `settings_every_key_once` (unit over the page model): the set of keys bound by all items equals the set of `settings.toml` keys in table (a) (with `fans.*` counted once for the Fans group), with no duplicates. `settings_restart_toast`: toggling "Use system title bar" pushes "Restart tasklens to apply the title bar change".
- **Phase**: P2 (window); each item from its feature's phase.
- **Decided here**: size, sidebar width, icons, refocus.

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
| Smooth Graphs | switch | `motion.smooth_graphs` | Under system reduce motion the switch keeps its value and a `t_sm` `text_secondary` note under it reads "Turned off by the system reduce-motion setting" (UI-PRF-003) | P2 |
| Save power on battery | switch | `motion.save_power_on_battery` | A `t_sm` `text_secondary` note under it reads "On battery, CPU values update once a second and graphs do not slide" | P2 |
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
| Overview | Show top processes box | switch | `performance.overview_top_processes` | | P2 |
| GPU | Optional GPU graphs | two checkboxes "Power", "Clocks" | `performance.gpu_optional_graphs` | subset of power, clocks | P2 |
| GPU | Hidden engine graphs | custom: checklist per GPU of its engines (checked = shown) | `performance.hidden_gpu_engines` | engine keys | P2 |
| Devices | Disks / GPUs / Fans / Batteries | four switches | `performance.show_disks`, `show_gpus`, `show_fans`, `show_batteries` | | P2 |
| Devices | Wired / Wireless / VPN and tunnel / Virtual / Other | five switches, indented 16 under a "Network adapters" label | `performance.show_network_wired`, `_wireless`, `_vpn`, `_virtual`, `_other` | | P2 |
| Devices | Show disconnected fans | switch | `performance.show_disconnected_fans` | | P3 |
| Sidebar | Device order and visibility | custom list (below) | `performance.device_order`, `performance.device_overrides` | | P2 |
| Fans | one row per fan: label input and temperature sensor dropdown | custom (below) | `fans.<key>` | | P3 |

- **Device order list** [C]: one row per device in the catalog (name, type), 28 tall, with a drag handle (Lucide `grip-vertical`) to reorder, and a three-way segmented control "Default", "Show", "Hide" bound to `device_overrides` (Default removes the key). Buttons below: "Reset order" (clears `device_order`, toast "Sidebar order reset") and "Show all" (clears `device_overrides`). Keyboard: Alt+Up and Alt+Down move the focused row.
- **Fans group** [C]: per fan, a text input "Label" (empty = default name) and a dropdown "Temperature sensor" listing hwmon temperatures as "<label> (<chip>) 45 °C", plus "None". Writes `fans."<key>" = { label, temp }` with both fields optional (`label` omitted when empty, `temp` omitted for None); an entry with neither is removed. With no fan in the snapshot the group is hidden. Keys follow PERF-005 (R-221).
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
| Login | Start minimized | switch, indented 16, disabled unless Start at login is on; rewrites the autostart entry through `AppEnv.set_autostart(true, minimized)` (R-236) | `startup.start_minimized` | P2 |
| Login | Open with Ctrl+Esc | switch, no settings key; reads and writes kglobalaccel through `AppEnv.global_shortcut` (APP-051, R-171); disabled with "Not available" when it is `None`; hidden outside KDE | (none) | P4 |
| Network | Auto-fetch Public IP on launch | switch | `network.auto_fetch_public_ip` | P2 |
| Memory | Open rescue window on critical memory pressure (tooltip "systemd-oomd may end processes before the rescue window opens.", plus " The watcher runs with normal OOM priority." when `state.lowmem.oom_score_adjust` is 0; an install or remove failure reverts the switch and sends the desktop notification "Could not set up the rescue window"; R-152) | switch | `lowmem.enabled` | P3 |

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
| Rendering GPU | dropdown (Auto, adapters from `AppEnv.list_adapters`, Software renderer), "Takes effect after restart"; stores `pci:<bus id>` when the adapter has one, else `name:<adapter name>` (R-237) | `rendering.gpu` | P4 |
| Do not ask again for public IP fetches | switch | `network.public_ip_notice_accepted` | P2 |
| Show FPS | switch | `debug.show_fps` | P2 |
| Import from TaskSlinger… | button (APP-034: portal file chooser for `UserSettings.ini` or `themes.json`; toast "Imported N settings and M themes"; failure toast "Could not import TaskSlinger settings" (Error, R-201); plain booleans and in-range integers only until the owner supplies `UserSettings.ini`, D-018; the button calls `AppEnv.import_taskslinger`, R-171) | (none) | P4 |
| Open settings folder | button: `xdg-open $XDG_CONFIG_HOME/tasklens` | (none) [C] | P2 |
| Reset Settings to Defaults | button (danger style), same confirm as the menu | (none) | P2 |

- **Decided here**: the "Open settings folder" button; Show FPS is always listed here so the key appears once.

### UI-SHL-072 Reset settings confirmation

- **Implements**: APP-022.
- **Behaviour**: `ResetSettings` opens the generic confirm window (UI-SHL-091) titled "tasklens", text "Reset all settings to their defaults? Themes and history are kept.", buttons "Reset" (danger, default) and "Cancel" [S]. On Reset: `settings.toml` is replaced by defaults (atomic write), `TlSettings` reloads, the theme and scale re-apply, `EngineSettings` re-sent, Success toast "Settings reset to defaults"; on write failure Error toast "Failed to reset settings" and nothing changes [S]. `state.toml`, `themes.json` and history are untouched [S].
- **Tests**: `reset_settings_keeps_state`: after reset, `UiState` columns and window size unchanged, `settings.ui_scale_pct == 100`.
- **Phase**: P1.

---

## 7. Themes, accents and theme management

### UI-SHL-080 Theme application and follow-system (shell rules)

- **Implements**: APP-035, APP-036, R-017, R-068, R-069, ADR-009 (Theme model, theme switch).
- **Owner**: `ThemeStore::activate` (UI-FND-040) owns resolving, the 200 ms crossfade (Hsla lerp driven by the `AnimationDriver`, `style_version` bumped at start and end), the instant path with animations off, writing `theme.active`, and follow-system (portal `color-scheme` from the engine through the snapshot; 0 and 1 pick `theme.dark`, 2 picks `theme.light`).
- **Shell rules**:
  - Every theme choice in the shell (View > Theme rows, the Settings page dropdown) dispatches `ActivateTheme { name }`.
  - An explicit choice while `theme.follow_system` is on turns sync off and pushes the Info toast "Theme sync turned off" [C], so the user's pick is not overwritten at the next portal change.
  - Turning sync on applies the matching preferred theme at once [C].
  - The shell turns `ThemeEvent::{Activated, Deleted, Copied, ImportFailed, Imported { count }}` into the APP-036 and APP-038 toasts (UI-SHL-038, UI-SHL-083, UI-SHL-084), plus "Imported { $count } themes" (Success), and `ThemeError::BuiltinCannotBeDeleted` and `BuiltinCannotBeEdited` into "Built-in themes cannot be deleted" and "Built-in themes cannot be edited. Copy it first." (Warning; R-209).
- **Tests**: `explicit_choice_turns_off_sync`: with sync on, `ActivateTheme("Dracula")` sets `theme.follow_system = false` and pushes "Theme sync turned off".
- **Phase**: P1 (Default Dark and Default Light, instant switch), P2 (crossfade, other themes, sync).
- **Decided here**: explicit choice turns sync off; turning sync on applies at once.

### UI-SHL-081 Theme Editor window

- **Implements**: APP-037, APP-035 (22 slots plus accent), APP-045.
- **Anatomy** (window "tasklens Theme Editor", `WindowKind::Floating`, not modal, default 820 × 560, minimum 720 × 480 [C]):

```text
+--------------------------------------------------------------------------------------+
| tasklens Theme Editor                                                  [pin] [x]     |  TlSecondaryWindow
+--------------------------------------------------------------------------------------+
| Name: [ Default Dark copy                    ]                                       |  h=32, pad 16
+----------------------+---------------------------------+-----------------------------+
| SLOTS (w=240)        | PICKER (flex, min 280)          | PREVIEW (w=260)             |
| [■] Window backgr.. #1E1F22 | +-----------------+ +--+   | [Processes][Performance]    |
| [■] Title bar   #27292B | |  S/V plane      | |H |   | Name        CPU   Memory    |
| ...22 rows + Accent  |  |  200 x 200      | |16|   | row (alt)   ...             |
|  row h=26            |  +-----------------+ +--+   | row hover                   |
|                      |  R 39  G 41  B 43           | row selected                |
|                      |  H 210 S 009 V 017          | [graph 236 x 60]            |
|                      |  Hex [#27292B]  hint         |                             |
+----------------------+---------------------------------+-----------------------------+
| [Random] [Share] [Import]                                       [Cancel] [Done]      |  footer h=48
+--------------------------------------------------------------------------------------+
```

  - Name input: `TlTextInput`, label "Name:", width fills, max 64 characters [C].
  - Slot list: 23 rows (the 22 slots in APP-035 order with their labels, then "Accent"), each 26 tall: swatch 14 × 14 `r_sm` with a 1 px `divider` border, label `t_base`, hex value `t_xs` tabular `text_secondary` right-aligned. Selected row fill `row_selected`, text `text_selected_primary`. Up and Down move the selection when the list has focus.
  - Picker: saturation (x) and value (y) plane 200 × 200 with a 10 px ring cursor; vertical hue strip 16 × 200 to its right with a 2 px bar cursor; both custom elements. Readouts "R %d G %d B %d" and "H %03d S %03d V %03d" [S] in `t_sm` tabular. Hex input with placeholder "Enter a 6-digit hex color" [S]; accepts with or without "#", applies on Enter or blur; invalid text shows a `status.danger` border and the text "Enter a 6-digit hex color" below it, and does not apply [C].
  - Preview: a mini tab strip (two tabs, the active one with the accent outline), a mini table (header plus 4 rows: normal, alternate, hover, selected, with secondary text), and a 236 × 60 static graph (fill and line from the theme's graph palette), all painted from the draft theme.
  - Footer buttons: left "Random", "Share", "Import"; right "Cancel", "Done" (primary, Enter) [S labels].
- **Behaviour**:
  - Opened by `EditTheme(name)` for a custom theme, which calls `ThemeStore::begin_preview` (UI-FND-043). A second `EditTheme` for the same theme focuses the open editor; for a different theme it cancels the first preview, closes that editor and opens a new one (UI-FND-043 rule 1).
  - Edits apply live to the whole app [S] through `ThemeStore::set_preview_slot` on every change; the store applies at most one change per frame (UI-FND-043 rule 3).
  - Keyboard on the plane: arrows move 1 unit of S or V (Shift: 10); on the hue strip Up and Down move hue by 1 degree (Shift: 10) [C].
  - Random: randomizes the 23 editable values under UI-FND-043's 4.5:1 contrast rule; toast "Random theme generated." [S].
  - Share: opens UI-SHL-082 with the draft encoded.
  - Import: reads the clipboard as in UI-SHL-083, and loads the result into the preview (name included) instead of adding a new theme [C].
  - Cancel or the close button: `ThemeStore::cancel_preview`, then close [S].
  - Done: validates the name (not empty; not equal to another theme's name, case-insensitive), then `ThemeStore::commit_preview` (saves to `themes.json` and activates), closes, Success toast "Theme saved" [C]. Invalid name: the input shows `status.danger` and the text "Choose a name that no other theme uses" and Done is disabled [C].
  - Built-in themes never open here ("Copy a theme before editing it", UI-SHL-038).
- **Build**: `window/theme_editor.rs`, Entity `ThemeEditor { token: PreviewToken, selected_slot: ThemeSlot, plane: Entity<ColorPlane>, hue: Entity<HueStrip>, hex: Entity<InputState>, name: Entity<InputState> }`. `ColorPlane` paints with two `paint_quad` linear gradients (white to hue horizontally, transparent to black vertically) [C]. The kit `ColorPicker` is not used: it is a popover with featured colours, not the TaskSlinger layout.
- **Performance**: plane paint is 2 quads; a drag costs one preview application per frame (UI-FND-043).
- **Accessibility**: Tab order: Name, slot list, plane, hue strip, hex input, Random, Share, Import, Cancel, Done.
- **Tests**: `editor_cancel_restores`: edit a slot, Cancel: `ThemeStore::current()` equals the theme active before opening. `editor_hex_invalid`: "12345" does not apply. `editor_done_duplicate_name` disables Done.
- **Phase**: P3.
- **Decided here**: layout, sizes, keyboard steps, toast "Theme saved", name validation text, own colour plane.

### UI-SHL-082 Share Theme window

- **Implements**: APP-038, APP-045.
- **Anatomy**: "tasklens Share Theme", `WindowKind::Dialog` modal to its parent (the main window or the editor), 520 × 220 [C]. Body: text "Copy this code and send it to someone who uses tasklens." [C], then a read-only multi-line `TlTextInput` (3 lines, wraps, `t_sm`) holding `TL1.` + base64url(JSON `{"v":1,"name":…,"accent":"#…","SLOT":"#RRGGBB",…}`) [S]. Footer: "Copy" (primary, Enter), "Close".
- **Behaviour**: Copy writes the code to the clipboard and pushes the toast "Theme code copied" in this window [S]. The code is selected on open so Ctrl+C also works [C].
- **Phase**: P3.

### UI-SHL-083 Import theme from the clipboard

- **Implements**: APP-038, R-079 (verify U-17).
- **Behaviour**: `ImportTheme` reads `cx.read_from_clipboard()` text and accepts, in order: a `TL1.` code; a `TS1.` code (only if U-17 verification passed; otherwise not accepted); raw TaskSlinger `themes.json` text (`{"themes":[...]}`, every theme in it is imported) [S]. Missing slots take the "Default Dark" value (or "Default Light" when the imported `WINDOW_BG` lightness is above 50%) [C]. A name that already exists gets " 2", " 3" and so on [C]. Imported themes are written to `themes.json`; the first becomes active; Success toast "Theme imported" [S]. Anything else: Info toast "Clipboard does not contain a theme code" [S].
- **Tests**: `import_tl1_roundtrip`: share then import gives an equal theme named "<name> 2".
- **Phase**: P3.

### UI-SHL-084 Delete theme confirmation

- **Implements**: APP-036, APP-045.
- **Behaviour**: generic confirm (UI-SHL-091) titled "tasklens", text "Delete this custom theme?" [S], buttons "Delete" (danger) and "Cancel". On Delete: removed from `themes.json`; if it was active, apply `theme.dark` or `theme.light` matching the deleted theme's mode (falling back to "Default Dark" or "Default Light" when the preferred theme is the deleted one); a `theme.light` or `theme.dark` that named it resets to its default; Success toast "Theme deleted" [S].
- **Phase**: P2.

### UI-SHL-085 Graph Color submenu (accent colours)

- **Implements**: APP-039, R-083, PERF-081, PERF-090.
- **Anatomy** (shared builder `graph_color_menu(current: GraphColor, action: fn(GraphColor) -> Box<dyn Action>)` in `window/menubar.rs`, used by the minigraph card menu and any in-app graph menu):
  - First row "Accent (theme default)" [S], then the 17 named colours in APP-039 order: Coral, Orange, Amber, Yellow, Lime, Green, Mint, Teal, Cyan, Blue, Indigo, Violet, Lavender, Fuchsia, Pink, Rose, Slate.
  - Each row: radio check column, a 12 × 12 swatch with radius `r_sm` (the accent row shows the current theme's `accent`), then the name.
  - Stored value: `accent` or the lower-case name [S R-083].
- **Tray variant**: the tray graph icon menu (dbusmenu through ksni) shows the same names as radio items without swatches, because dbusmenu items here carry text only [C].
- **Phase**: P3.

---

## 8. Secondary windows

### UI-SHL-090 Secondary windows (shell rules on top of `TlSecondaryWindow`)

- **Implements**: APP-045, APP-009, R-073, PROC-056, D-006.
- **Component**: `TlSecondaryWindow` (UI-FND-021) owns the frame: 34 px title bar with icon and title, the kit caption buttons (34 wide), the pin `TlButton::Toggle` with tooltip "Always on top", the server-decorations pin fallback, the window kinds (Dialog for confirmations, Floating for tools and Properties), modality, Esc rules (confirmation: Cancel; tool window: close; Run: cancel), Ctrl+W, and `WindowRegistry` (refocus of an open window, closing children with the main window).
- **Shell rules**:
  - Every shell window in this section is opened through `TlSecondaryWindow::open` with the title from the APP-045 table [S], the kind, the default size and the minimum size given in its entry.
  - Each window has its own kit Root, so toasts in it use its own `ToastQueue` (UI-SHL-100).
  - A shell window that is already open is focused instead of opened again (`WindowRegistry` keys `Settings`, `ThemeEditor`, `ShareTheme`, `About`, `CrashReport`, `AdditionalSetup`, `Shortcuts`, `PublicIpNotice`, `Run`, `HandleSearch`, `CollectorHealth`) [C].
- **Tests**: `secondary_window_refocus`: `OpenAbout` twice leaves one About window.
- **Phase**: P1 (frame), each window with its feature.
- **Decided here**: the registry keys of shell windows.

### UI-SHL-091 Confirmation windows used by the shell

- **Implements**: APP-045, APP-022, APP-036, APP-049, R-083.
- **Component**: `TlDialog::confirm` (UI-FND-021): window title "tasklens" [S], width 420, Cancel (Secondary) left of the action button (Primary, or Danger for destructive actions), Enter runs the focused button, Esc cancels, focus starts on Cancel for destructive confirmations (so Enter there cancels; the destructive button needs Tab then Enter, or a click) and on the action button otherwise.
- **Shell uses** (texts and buttons are the owners' [S]):

  | Use | Text | Action button | Destructive |
  |---|---|---|---|
  | Reset settings (UI-SHL-072) | "Reset all settings to their defaults? Themes and history are kept." | Reset | yes |
  | Delete theme (UI-SHL-084) | "Delete this custom theme?" | Delete | yes |
  | Skip password prompts (UI-SHL-040) | "tasklens actions that change other users' processes, system services and network connections will run without asking for a password. Continue?" | Skip prompts | no |

- **Tests**: `reset_confirm_focuses_cancel`: the Reset confirm opens with focus on Cancel; Esc closes it without resetting.
- **Phase**: P1.

### UI-SHL-092 About window

- **Implements**: APP-053, R-078, D-005.
- **Anatomy**: "tasklens About", Floating, 440 × 380, not resizable [C]. Centred column, padding 24:
  - App icon 64 × 64.
  - "tasklens" in `t_xl` semibold.
  - "Version: 0.1.0 (a1b2c3d, 2026-10-03)" in `t_sm` `text_secondary` [S format].
  - Buttons row: "Copy version info", "Third-party licenses" [S].
  - Links row (`TlButton` link variant): "Project repository" (opens the repository URL from `CARGO_PKG_REPOSITORY`) and "Open log folder" (`xdg-open $XDG_STATE_HOME/tasklens/logs`) [S].
- **Behaviour**:
  - "Copy version info" copies a block with version, git hash, build date, GPUI revision, kernel, NVIDIA driver, compositor, session type and scale [S], toast "Copied to clipboard".
  - "Third-party licenses" swaps the body for a scrollable list (crate name, version, licence) from the embedded `cargo-about` output, with a "Back" button at the top [C].
  - No TaskSlinger or Mission Center branding [S].
- **Phase**: P2.

### UI-SHL-093 Crash Report window

- **Implements**: APP-055, R-049, ADR-015.
- **Anatomy**: "tasklens Crash Report", Floating, 480 × 220 [C]. Icon Lucide `triangle-alert` 24 in `status.warning`. Text "tasklens closed unexpectedly last time." [S] in `t_base`; second line in `t_sm` `text_secondary`: "If tasklens crashed without a report, run coredumpctl info tasklens in a terminal." [C wording, S content]. Footer: "Open log folder", "Copy report", "Close" (default) [S].
- **Behaviour**:
  - Shown once after the main window's first frame when `$XDG_STATE_HOME/tasklens/crash/` holds a `crash-<t>.txt` without a matching `crash-<t>.seen` file; on show, the `.seen` marker is written [C] (no settings or state key is needed).
  - "Copy report" copies the newest report's text; toast "Copied to clipboard". "Open log folder" opens `$XDG_STATE_HOME/tasklens/logs`.
  - Not shown when tasklens starts hidden to the tray; it shows the next time the main window opens [C].
- **Phase**: P1.
- **Decided here**: the `.seen` marker; deferral while hidden; second-line wording.

### UI-SHL-094 Additional Setup window (frame)

- **Implements**: APP-042, PERF-105, R-028.
- **Anatomy**: "tasklens Additional Setup", Floating, 640 × 520, minimum 560 × 400 [C]. Scrollable column of row cards, padding 16, gap 12. Row card (owned content per PERF-105 to PERF-108): background `header`, radius `r_md`, padding 12; line 1: title `t_md` semibold left, status text `t_sm` right (colours: "Enabled" and "Enabled (existing rule)" `status.success`, "Not enabled" `text_secondary`, "Not needed" `text_secondary`); line 2: description `t_sm` `text_secondary`; line 3: buttons "Enable" or "Revert" (primary), "View script", "Copy command" [S]. Busy state while pkexec runs: the row's buttons disabled and the status text "Waiting for authentication…" [C]. Result area under the buttons: "Done. No restart needed." in `status.success`, or the script output in a read-only 6-line `TlTextInput` with "Copy output" and "Retry" [S].
- **Opened by**: Help > "Additional setup…", Settings > Privileges "Additional setup…", every "Needs setup" link (which also scrolls to the row) [S].
- **Phase**: P3.
- **Decided here**: sizes and row card styling, busy text.

### UI-SHL-095 Keyboard Shortcuts window

- **Implements**: APP-030, APP-031, R-083.
- **Anatomy**: "tasklens Keyboard Shortcuts", Floating, 560 × 640, minimum 420 × 360 [C]. Search input at top (placeholder "Search shortcuts") [C]. Scrollable list grouped by context with headings in this order [S, R-220]: Global, Tables, Processes, History, Services, Startup Apps, Connections, Information, Performance, Properties windows (PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog), Dialogs (RunDialog, Picker, ColumnPicker, HandleSearch, JournalList, LowMemory). A heading with no shipped bindings is not drawn. Each row 28 tall: description `t_base` left; key caps right as kit `Kbd` elements from `Kbd::binding_for_action` (several bindings separated by " or ") [S].
- **Behaviour**: rows come from the live keymap, so every row in table (b) appears once; actions not built in the current phase are absent. Search matches description and key text. Opened by Help > "Keyboard Shortcuts" and Ctrl+? [S].
- **Phase**: P2.

### UI-SHL-096 Public IP Notice window

- **Implements**: APP-056, R-058, PERF-060.
- **Anatomy**: "tasklens Public IP Notice", Dialog (modal), 460 wide [C]. Text lines [S]: "tasklens will fetch your public IP address from an online service.", "Service: https://api.ipify.org", "Continue only if you want this computer to contact that service." Checkbox "Do not ask again for public IP fetches" (`network.public_ip_notice_accepted`). Buttons "Continue" (primary, default) / "Cancel".
- **Behaviour**: shown before the first fetch while the key is false. Continue runs the fetch (owned by PERF-060) and writes the key when the box is checked [S].
- **Phase**: P2.

### UI-SHL-097 Run dialog window frame

- **Implements**: APP-014, PROC-095, R-061.
- **Frame**: "tasklens Run", Floating (pinnable, not modal), 440 × 210, not resizable [C]. Content, keys (RunDialog context) and toasts are PROC-095's [S]. Opened by File > "Run..." and Ctrl+N; a second Ctrl+N focuses it [C].
- **Phase**: P2.

### UI-SHL-098 Search File Handles window frame

- **Implements**: APP-015, PROC-096, R-061.
- **Frame**: "tasklens Handle Search", Floating, not modal, 860 × 560, minimum 560 × 360 [C], the sizes in docs/ui/process-windows.md (UI-PWN-001). Content per PROC-096 [S].
- **Phase**: P3.

### UI-SHL-099 Collector Health window

- **Implements**: APP-026, APP-029, R-071.
- **Anatomy**: title "tasklens Collector Health" [C, APP-045 pattern], Floating, 720 × 360 [C]. A table with columns Source (140), State (100), Last data (100, age as "3s ago"), Restarts (80, right-aligned), Last error (fills, truncated, full text in tooltip) [C]. Rows sorted by `SourceId` order. A source in state Off (not demanded) has no row. State text uses `status.success` (Running), `status.warning` (Late, Stale, Disabled), `status.danger` (Failed), `text_secondary` (Unsupported) [C]. Last data shows "…" in `text_secondary` until the source sends data. Restarts is `SourceHealth.restarts`.
- **Source names**, in `SourceId` order: CPU, Memory, Disks, Network, GPU (NVIDIA), GPU (DRM), Sensors, Processes, Process details, Connections, Services (system), Services (user), Sessions, Startup, Inventory, KWin, Storage devices, Power, NetworkManager, History, Journal [C, settles B7 E21 open item 6].
- **Behaviour**: opened from the Debug menu and from health items in the status chip [S]. Re-renders on `ComponentMask::HEALTH` and once per second for the ages [C].
- **Phase**: P1 (from the status chip), P4 (Debug menu).
- **Decided here**: title, columns, colours, source names.

---

## 9. Toasts

### UI-SHL-100 Toast placement in the shell

- **Implements**: APP-044, R-017, ADR-015 (no toast for collection errors), widgets §10.
- **Component**: `TlToast` and `ToastQueue` (UI-FND-020) own the card look, type icons, durations (5 s; Error 8 s; action toasts 15 s; Progress until replaced), the 4-visible limit with a wait list, hover pause, replace-in-place keys, coalescing, motion and reduced motion, and System delivery while the main window is hidden.
- **Placement** (the shell's numbers, which UI-FND-020 takes from this file):
  - Main window: horizontally centred, top = title bar 34 + tab strip 30 + `sp4` 8 = 72 at 100% scale, times `ui_scale` [S APP-044 "8 px under the tab bar"]. The shell exports `TAB_STRIP_H = 30.0` and `TITLEBAR_H = 34.0` from `window/main.rs` for this.
  - Server decorations: top = menu row 28 + tab strip 30 + 8 = 66.
  - Secondary windows: 72 × `ui_scale` as well in P1 (66 with server decorations), because the kit `NotificationSettings` is app-global; 42 (title bar 34 + 8) once the queue takes a per-window top in P2 (UI-FND-020 rule 9, R-195).
  - The toast globals are re-set on every scale change (UI-SHL-130).
- **Shell toasts**: every toast text in this file is listed with its entry; the shared texts of APP-044 ("Copied to clipboard", "Copied: <value>", "Failed to copy to clipboard", "No process selected", "Select a process row first.", "Opening file location", "Failed to open file location", "Could not open properties.", "Authentication cancelled") are pushed by their owners through the same queue.
- **Tests**: `toast_top_follows_tab_strip`: at `ui_scale_pct = 150` the stack top is 108.
- **Phase**: P1.

---

## 10. Startup, single instance and tray

### UI-SHL-110 Startup sequence and first frame

- **Implements**: APP-052, APP-055, APP-059, APP-011, APP-048, APP-049, ADR-010, ADR-014 (first frame gates: 200 ms warm, 500 ms cold).
- **Sequence** (`tl-app/src/main.rs`, then `TasklensUi::init`):
  1. Install the panic hook (APP-055). Set up logging (APP-054).
  2. Parse arguments with clap (APP-052). Unknown arguments print usage and exit 2 (no window).
  3. NVIDIA X11 environment rule (ADR-010) and, in P4, the `rendering.gpu` variables.
  4. Single instance (UI-SHL-112): forward and exit 0 if another copy runs.
  5. Load `settings.toml`, `state.toml`, `themes.json` (APP-033). Corrupt or newer-file toasts are queued, not shown yet.
  6. Note the Shift key state for "Shift held at launch" (P3). On X11 tl-app reads it with `XkbGetState`. Wayland gives a client no modifier state before its window has focus, so on Wayland the main window counts Shift as held when its first `on_modifiers_changed` event within 500 ms of first focus has `shift` set [C, verify U-43, R-124].
  7. Start the engine (`EngineHandle::start`) on its own threads; do not wait for any collector.
  8. `Application::run`: load fonts, `gpui_kit::init`, register the asset source, build `ThemeStore` and activate the theme without crossfade (UI-FND-040), `bind_keys` (UI-SHL-120), set the toast globals, create `SnapshotHub`, `DemandAggregator`, `ActionTracker`.
  9. Open the main window (UI-SHL-002) on the start tab: `--tab` if given, else `general.start_tab` (`last_used` reads `UiState.tabs.last_tab`) [S]. Only that page Entity is created.
  10. First frame renders the chrome and the start page with whatever the hub has (probably nothing): no waiting for collectors [S APP-059]. The page shows its own first-data state (process table: header and empty body, then rows with `…` in rate cells after the first sweep; Performance: charts with "Collecting…" readouts; P2 tabs: skeletons, UI-SHL-111).
  11. After the first frame callback, in this order:
      - log `info!(target: "tasklens::perf", first_frame_ms)`;
      - show the queued toasts (settings errors);
      - show the Crash Report window if due (UI-SHL-093);
      - run `--select-pid`: select and scroll through the process jump target (PROC-071) as soon as the pid appears in the table; after 3 s without it, Info toast "Process <pid> not found" [C];
      - start the helper session if Shift was held or `privilege.unlock_at_launch` is on (P3);
      - start the public IP auto-fetch if it is enabled and the notice was accepted (P2).
- **Start hidden or minimized**: UI-SHL-002 (`--start-minimized`).
- **`--low-memory`**: opens the rescue window (PROC-098) instead of the main window [S]. `--watch-memory` starts no GPUI [S].
- **`--debug`** or `TASKLENS_DEBUG=1`: Debug menu present (UI-SHL-042).
- **Performance**: steps 1 to 9 must fit the ADR-014 first-frame gate (200 ms warm). Theme JSON parse, settings parse and font loading are the main costs; fonts are loaded from bytes embedded with `include_bytes!` [C]. No page except the start page is constructed before the first frame.
- **Tests**: `startup_tab_arg_wins` (TestAppContext): `--tab services` with `start_tab = performance` opens Services. `first_frame_no_wait`: with a fake engine that never publishes, the first frame renders within one frame cycle.
- **Phase**: P1 (P2 and P3 steps as marked).
- **Decided here**: queued toasts after first frame; `--select-pid` wait of 3 s and its toast; Shift detection path (R-124, verify U-43).

### UI-SHL-111 Loading placeholders (frame rule)

- **Implements**: APP-059, R-060, ADR-009 (Skeleton never in table cells or charts).
- **Behaviour**: the shell guarantees the first frame never waits. Services, Startup Apps, Connections and Information show `TlSkeleton` blocks until their first data, once per session: 12 skeleton rows of row height with widths cycling 60%, 40%, 75% of the name column for tables, and, for Information, every section's band with an 8-line skeleton block (UI-SYS-085, R-233) [C]. Skeletons are never shown again when switching between tabs whose data is loaded [S]. Services also shows the toast "Loading service data..." [S SYS-016]. The process table uses no skeleton [S].
- **Performance**: the kit Skeleton animates (2 s loop) and requests frames while visible; at most one skeleton page is visible at a time and only until first data (typically under 500 ms) [S widgets].
- **Phase**: P2.
- **Decided here**: skeleton counts and widths.

### UI-SHL-112 Single-instance activation

- **Implements**: APP-011, APP-052, R-047, ADR-010.
- **Behaviour** (UI side; the D-Bus part lives in tl-app):
  - `Activate`: if the main window was removed (closed to the tray, or hidden while detached graphs stay), reopen it (UI-SHL-002 path, same tab as before hiding) [S]; if minimized or behind other windows, call `window.activate_window()` and, when the KWin bridge is up, also send `KWinCommand::Activate` for the main window [S]. The XDG activation token from the second launch is passed to `activate_window` where GPUI supports it [S].
  - `ActivateAction("tab", s)`: `SelectTab` for that tab after activating [S].
  - `ActivateAction("select-pid", u)`: activate, select Processes, run the process jump (same 3 s rule as UI-SHL-110) [S].
  - `ActivateAction("quit")`: `Quit` [S].
  - `--start-minimized` from a second launch is ignored [S].
  - On Wayland KWin may only flash the taskbar entry; that is accepted [S].
- **Build**: a GPUI task in `TasklensUi::init` receives `AppRequest` values from tl-app's `async_channel::bounded(4)` and dispatches the Actions above.
- **Tests**: `activate_reopens_hidden_window` (TestAppContext): with the main window removed, sending `Activate` opens a new main window on the last tab.
- **Phase**: P1.

### UI-SHL-113 Close behaviour, minimize to tray and the main tray icon

- **Implements**: APP-047, APP-048, PERF-084, PERF-090, PERF-091, PERF-092, R-027, ADR-010 (Close behaviour, Tray).
- **Close decision** (for `CloseWindow` from the close button, Ctrl+W, or Esc-close; and for the compositor's close request through `on_window_should_close`) [S, ordered as ADR-010 plus PERF-084]:

  | # | Condition | Result |
  |---|---|---|
  | 1 | tray is running and `tray.close_to_tray` | remove the main window; app and engine keep running; demand drops to tray and detached graphs |
  | 2 | `graphs.keep_detached_on_close` and at least one detached graph window is open | remove the main window; app keeps running until the last detached window closes, then quits |
  | 3 | otherwise | `cx.quit()` |

  - `Quit` (Ctrl+Q, File > Exit tasklens, tray "Exit tasklens") always quits [S].
  - Before removing the window, its size is saved (UI-SHL-002).
- **Minimize**: with `tray.minimize_to_tray` on and the tray running, the minimize caption button removes the window like row 1 [S]. A compositor-initiated minimize (taskbar click, KWin shortcut) cannot be intercepted on Wayland and minimizes normally [C].
- **Main tray icon** (SNI through `tl-tray`, owned by the tray crate; this entry fixes its UI contract):
  - Shown when no tray graph icon exists and either the main window is hidden by row 1 or minimize-to-tray, or `tray.always_show` is on [C, reading of APP-047 "keeps the icon while the window is visible"]. Hidden while any tray graph icon exists [S PERF-091].
  - Icon: the app icon at 22 × 22 and 44 × 44 ARGB32 [S].
  - Tooltip: five lines joined by `\n`: "CPU: 12.3%", "RAM: 45.6%", "GPU: 3.0%", "Disk: 0.4%", "Network: 2.3 Mbps" (R-161), updated at most 1 Hz while shown, network in the `units.network` unit [S].
  - Left click: toggle the main window (hide by row 1 rules if visible and focused; else show and activate) [S].
  - Menu [S R-027]: "Show Main Window", "Tray icon graphs >" (PERF-088 owns the submenu), separator, "Exit tasklens".
  - Re-register when `org.kde.StatusNotifierWatcher` changes owner (PERF-092) [S].
- **Hidden-window effects**: collection drops to tray and detached-graph demand (ADR-008); toasts reroute (UI-SHL-100).
- **Build**: `tray_bridge.rs` sends `TrayModel { tooltip, menu_state }` to `tl-tray` and receives `TrayEvent::{Activate, ShowMainWindow, Exit, ..}` over channels; tl-ui never links zbus or ksni.
- **Tests**: `close_to_tray_keeps_app`: with tray on and close-to-tray on, `CloseWindow` removes the window and the app stays alive; `Quit` ends it. `close_quits_by_default`.
- **Phase**: P1 (rows 3 and Quit), P3 (tray, rows 1 and 2).
- **Decided here**: icon visibility rule; compositor minimize not intercepted.

### UI-SHL-114 Desktop notifications from the shell

- **Implements**: APP-055, ADR-015, APP-044.
- **Texts** [S]: "tasklens stopped after an unrecoverable error." (after a crash, sent by the panic path); "tasklens could not start the renderer" (GPU init failure, also on stderr, exit 1); "tasklens lost the graphics device and must restart" (device loss, then one relaunch). Sent through `org.freedesktop.Notifications.Notify` from tl-app, which works without a GPUI window [S].
- **Phase**: P1.

---

## 11. Keyboard and focus

### UI-SHL-120 Key contexts and the binding table

- **Implements**: APP-031, canonical table (b), R-042, R-045, D-008, PERF-007.
- **Contexts**: "Global" (set on the main window's root element), "Table" (every table's focus element), "ProcessTable" (added on the process table, together with "Table"), "ServiceTable", "Performance", "RunDialog", "Picker" [S], plus the kit contexts of `PopupMenu`, `Input` and `Dialog`. Bindings are registered once in `actions.rs::bind_keys(cx)` before any menu is built (widgets rule 10).
- **Shell bindings** (keys in GPUI keystroke syntax; Actions in `tasklens::`):

  | Keys | Context | Action | Notes |
  |---|---|---|---|
  | `ctrl-1`..`ctrl-6`, `alt-1`..`alt-6` | Global | `SelectTab(0..5)` | tabs not shown are ignored |
  | `ctrl-tab` / `ctrl-shift-tab` | Global | `NextTab` / `PrevTab` | |
  | `ctrl-n` | Global | `OpenRunDialog` | P2 |
  | `ctrl-q` | Global | `Quit` | |
  | `ctrl-w` | Global | `CloseWindow` | in a secondary window closes that window [C] |
  | `ctrl-,` | Global | `OpenSettings` | P2 |
  | `ctrl-?` and `ctrl-shift-/` | Global | `OpenShortcuts` | both bound [C] |
  | `f10` | Global | `OpenMenu(File)` | |
  | `alt-f` / `alt-v` / `alt-s` / `alt-h` | Global | `OpenMenu(File/View/Settings/Help)` | `alt-d` Debug when present [C] |
  | `ctrl-f` | Global | `FocusFilter` | no-op on Performance (no filter box) [C] |
  | `f5` | Global | `RefreshCurrentTab` (R-174) | sends `Command::RefreshNow(source of the active tab)`; Info toast none [C] |
  | `escape` | Global | `EscapeChain` | UI-SHL-122 |
  | `ctrl-=`, `ctrl-+`, `ctrl-shift-=` | Global | `ScaleUp` | UI-SHL-130 |
  | `ctrl--` | Global | `ScaleDown` | |
  | `ctrl-0` | Global | `ScaleReset` | |
  | `ctrl-shift-m` | Global | `ToggleMiniWindow` | P3 |
  | `escape` | Picker | `CancelWindowPick` | P3; Picker context sits above Global while picking |

  The table, process, service, Performance, RunDialog and history-scrubber rows of table (b) are bound by their owners with the same mechanism; the shell only guarantees the contexts exist on the right focus elements.
- **Hold Ctrl**: not a binding; UI-SHL-123.
- **Shift held at launch**: UI-SHL-110 step 6.
- **Conflicts resolved by context depth**: Ctrl+C is Table "Copy row" in tables and Performance "Copy device info" on the Performance page; Ctrl+E is End Task on ProcessTable and Stop on a ServiceTable service row [S]. Ctrl+W, Ctrl+N and Ctrl+Q are not intercepted by `Input` (the kit input does not bind them) [C, verify against the kit keymap in the bead].
- **Tests**: `keymap_matches_table_b` (unit): for every table (b) row the expected binding resolves to the expected Action in the stated context (uses `cx.bindings_for_action`).
- **Phase**: P1 (Global and Table keys), P2, P3 as listed.
- **Decided here**: both Ctrl+? spellings, Ctrl+W in secondary windows, F5 without toast, Ctrl+F on Performance.

### UI-SHL-121 Focus model and Tab order

- **Implements**: APP-058, APP-028, widgets rule 16.
- **Focus targets in the main window** (Tab order, Shift+Tab reverses):
  1. Filter input (not on Performance).
  2. "Include children" checkbox (when shown).
  3. The active page's targets in the page's own order (for Processes: the table; for Performance: the device sidebar, then the page header buttons; for Services: the filter strip, then the table).
  4. Clickable status chip segments (when present).
  Tab from the last target wraps to the first. Title bar, toolbar, tabs and caption buttons are not Tab stops [C]; all their commands have keys or menu paths.
- **Initial focus**: when the main window opens and after every tab switch, focus goes to the page's primary target (the table on table tabs, the device sidebar on Performance, the first section on Information) [C], so arrow keys and type-to-filter work at once.
- **Return focus**: menus, popovers and in-window dialogs return focus to the element that had it [S widgets rule 16]. Secondary windows are separate windows; closing one returns focus to its parent window, which restores its last focused element (GPUI keeps it per window).
- **Focus ring**: `TlFocusRing` (foundation) on keyboard focus only (not after mouse clicks); the table's focused row uses the table spec's own focus style.
- **Build**: `MainWindow` holds `root_focus: FocusHandle` with `.key_context("Global")` and `.track_focus`; each page exposes `fn primary_focus(&self) -> FocusHandle` and `fn focus_order(&self) -> SmallVec<[FocusHandle; 4]>`. Tab and Shift+Tab are handled by GPUI's tab index on handles built with `.tab_stop(true)` in this order [C].
- **Tests**: `tab_order_processes`: from the filter, Tab reaches the table, then (with 2+ selected rows) nothing else, then wraps to the filter. `focus_after_tab_switch`: after `SelectTab(2)`, the services table is focused.
- **Phase**: P1.
- **Decided here**: chrome not in the Tab order; initial focus rule.

### UI-SHL-122 Esc chain

- **Implements**: table (b) Esc, R-043, APP-045, APP-009.
- **Order** (first match wins) [S table b, with shell detail C]:
  1. A menu or popover is open: close the innermost one (handled by the kit before `EscapeChain` sees the key).
  2. An in-window dialog is open: close it (kit).
  3. Window picking is active: `CancelWindowPick` (Picker context).
  4. The filter input has focus: its own rule (UI-SHL-024): clear text, or if empty, move focus to the table. Stops here.
  5. The active tab's filter has text (focus elsewhere): clear it and focus the table [S].
  6. `window.close_with_esc` is on: `CloseWindow` (with the close decision of UI-SHL-113) [S].
  7. Otherwise nothing.
- **Tests**: `esc_chain_order`: filter text set, focus on table: Esc clears the filter; second Esc with `close_with_esc = false` does nothing; with it on, the window closes.
- **Phase**: P1.

### UI-SHL-123 Hold Ctrl to pause

- **Implements**: R-042, PROC-050, ADR-009 (Ctrl-pause), APP-029.
- **Behaviour**:
  - `MainWindow` registers `on_modifiers_changed` on its root. When `control` becomes true while `window.is_window_active()`, it sets `paused = true` and emits `PauseChanged(true)`; when `control` becomes false, or the window deactivates, it emits `PauseChanged(false)` [S].
  - Pages that freeze (the process and history tables; other tables per their specs) subscribe and render from a retained `Arc<Snapshot>`; the status chip appends "[PAUSED - Ctrl]" [S].
  - Ctrl pressed as part of a shortcut (Ctrl+F) still pauses for the press duration; that is TaskSlinger's behaviour and accepted [C].
  - Secondary windows do not pause [S "main window active"].
- **Tests**: `ctrl_pause_releases_on_deactivate`.
- **Phase**: P1.

### UI-SHL-124 Type-to-filter routing

- **Implements**: APP-028, PROC-044, table (b) "Printable character".
- **Behaviour**: each table's focus element has an `on_key_down` handler (after its bindings) that, when `keystroke.key_char` is one printable character, the modifiers are none or Shift only, and the character is not a space (Space toggles expand, D-008), calls `FilterBox::begin_typing(ch)`: focus the filter input, move the cursor to the end, and insert the character [S]. It never fires while an input, menu or dialog has focus [S]. The Performance page does not route (no filter box) [C].
- **Tests**: `type_to_filter`: focus table, press "f": filter text "f", focus in the input. `space_not_routed`.
- **Phase**: P1.

---

## 12. Runtime scale, language and motion

### UI-SHL-130 UI scale and row height changes at runtime

- **Implements**: APP-040, R-020, R-077, ADR-009 (Metrics and scale), table (b) Ctrl+Plus, Ctrl+Minus, Ctrl+0, APP-058.
- **Behaviour**:
  - `ScaleUp` / `ScaleDown` step `appearance.ui_scale_pct` by 5 within 75..=200; `ScaleReset` sets 100. Each change that alters the value pushes the Info toast "UI scale changed to 125%" with id `ui-scale` so repeated presses replace one toast [S text; C id]. At a limit nothing changes and no toast shows [C].
  - Applying a new `ui_scale_pct` or `row_height_pct`, or a change of `window.scale_factor()`:
    1. Recompute `UiMetrics` (ADR-009): `row_height = 28 × row_height_pct/100 × ui_scale`, `base_text = 13 × ui_scale`, `radius_small = 4 × ui_scale` and `radius_card = 6 × ui_scale` (0 when `rounded_corners` is off).
    2. `Theme::update`: `font_size = base_text`, radius values; kit Root sets `rem_size` on its next render.
    3. Bump `style_version`; clear `CellTextCache`; drop cached tab label widths and the accent path.
    4. Re-set the toast globals (UI-SHL-100).
    5. Tables keep the first visible row at the top: each table records its first visible row identity before the change and scrolls to it after layout [C].
    6. `window.refresh()` on every tasklens window.
  - Slider drags apply steps 1 to 6 per change event (preview) and persist on release (UI-SHL-037). Key presses persist with the normal 500 ms debounce.
  - The compositor scale (110% here) is applied once by GPUI and never multiplied again [S].
  - Default from the system font (P4): when `appearance.ui_scale_pct` was never set, it is KDE's general font point size / 10 × 100, rounded to a multiple of 5 and clamped to 75..=200 [S APP-058].
- **Performance**: a scale change is a full re-layout of open windows plus one chart geometry rebuild each; acceptable for a user action. Nothing per frame.
- **Tests**: `scale_keys_step_and_clamp`: from 195, ScaleUp twice: 200 then 200 with one toast total. `row_height_metrics`: 120% row height at 150% scale gives 50.4 px rows. `scale_keeps_first_row`.
- **Phase**: P1 (keys and metrics), P2 (sliders), P4 (default from font).
- **Decided here**: toast id, limit behaviour, first-row anchoring.

### UI-SHL-131 Language change at runtime

- **Implements**: APP-043, R-081.
- **Behaviour**: `SetSetting { GeneralLanguage, .. }` loads the Fluent bundle for the locale (embedded `.ftl`, English fallback for missing keys), replaces the `Strings` Global, bumps `strings_version`, drops cached label widths (tabs, menus), and refreshes every window, so all text changes without a restart [S]. Numbers, dates and units do not change [S R-081]. "System default" follows `LC_ALL`, `LC_MESSAGES`, `LANG` read at startup; a system locale change applies at the next start [S].
- **Language submenu labels**: "System default" then each locale in its own language (UI-SHL-061 list) [C].
- **Tests**: `language_live_switch`: switching to de-DE changes the tab label "Processes" to its German key value in the same frame and leaves "1.0s" unchanged.
- **Phase**: P1 (key system and English), P4 (other locales).

### UI-SHL-132 Motion in the shell and reduced motion

- **Implements**: R-017, APP-058, ADR-009 (animation driver rule).
- **Every animation the shell owns**:

  | Animation | Duration and easing | Gate | With animations off or reduced motion |
  |---|---|---|---|
  | Tab indicator slide and page fade-in (UI-SHL-021) | `motion.tab_switch_ms` (250), `ease_out_cubic` | `motion.animations` | instant |
  | Filter box width (UI-SHL-024) | 150 ms, `ease_out_cubic` | `motion.animations` | instant |
  | Theme crossfade (UI-FND-040) | 200 ms | `motion.animations` | single step |
  | Toast enter and exit (UI-SHL-100, UI-FND-020) | enter 400 ms (kit), exit 200 ms | `motion.animations` | appear and disappear in one frame |
  | Menu popup open (UI-SHL-031, UI-FND-022) | 120 ms fade, no slide | `motion.animations` | instant |
  | Skeleton shimmer (UI-SHL-111) | kit 2 s loop | always while loading | static skeleton blocks (no shimmer) [C] |
  | Smooth scroll | `motion.smooth_scroll_amount` × 10 ms | `motion.smooth_scroll` | jump scrolling |
  | Chart sliding | per frame | `motion.smooth_graphs` | step per sample |

  - No shell animation is continuous; each one ends and stops requesting frames.
- **Tests**: `no_frames_when_idle` (TestAppContext): after all animations finish and with no snapshot changes, no frame is requested for 1 s.
- **Phase**: P1 (gates), P2 (animations that ship in P2).
- **Decided here**: static skeleton under reduced motion.

---

## 13. Debug overlay

### UI-SHL-140 FPS overlay

- **Implements**: APP-026 ("Show FPS"), ADR-014 (FrameStats), ADR-009 (AnimationDriver).
- **Anatomy**: absolutely positioned in the page area, top-right, 8 px inset [C]. Background `menu_bg` at 90% opacity, radius `r_sm`, padding 4 × 8, text `t_xs` tabular `text_primary`: "144 fps · frame p50 0.62 ms · p99 1.40 ms · stalls 0" [C].
- **Behaviour**: shown while `debug.show_fps` is on. Reads `AnimationDriver.stats` (frame intervals and UI-thread CPU time per frame, ADR-014) and re-renders at most twice per second, on a 500 ms timer that runs only while the overlay is shown [C]. The overlay never requests frames itself; with no animation running, its fps reads "idle" [C].
- **Phase**: P1 (developer flag).
- **Decided here**: placement, text format, 2 Hz refresh, "idle".

---

## 14. Summary table

| ID | Name | Implements (main) | Phase |
|---|---|---|---|
| UI-SHL-001 | Main window layout and resizing rules | APP-001, APP-002, APP-010, APP-027, APP-029, R-022 | P1 |
| UI-SHL-002 | Window creation, size and geometry restore | APP-001, APP-010, APP-048, R-022 | P1 |
| UI-SHL-003 | Client-drawn title bar layout | APP-001, APP-002, APP-008, D-006 | P1 (pin P2, shield and picker P3) |
| UI-SHL-004 | Title bar drag region, double-click and window menu | APP-001, APP-002 | P1 |
| UI-SHL-005 | Caption buttons | APP-008, APP-009, APP-047, R-073, PROC-056 | P1, pin P2 |
| UI-SHL-006 | Server-decorations variant | APP-001, R-021 | P1 |
| UI-SHL-007 | Active and inactive window appearance | APP-027 | P1 |
| UI-SHL-010 | Toolbar slot and per-tab content | APP-002, PERF-014, PROC-016, R-031 | P1, P3 |
| UI-SHL-011 | Shield | APP-003, APP-049, PROC-058 | P3 |
| UI-SHL-012 | Hide system processes toggle | APP-004, PROC-046 | P1 |
| UI-SHL-013 | View mode cycler | APP-005, PROC-015 | P1 |
| UI-SHL-014 | Window picker "+" | APP-006, PROC-055 | P3 |
| UI-SHL-020 | Tab strip layout and accent outline | APP-027, R-023 | P1 |
| UI-SHL-021 | Tab switch animation | APP-027, R-017 | P2 |
| UI-SHL-022 | Tab overflow menu | APP-027 | P1 |
| UI-SHL-023 | History toggle and Information copy icon | APP-027, PROC-086, PROC-088, PERF-094, D-010 | P2, P3 |
| UI-SHL-024 | Filter box | APP-028, PROC-044, R-043 | P1 |
| UI-SHL-025 | "Include children" placement | PROC-045 | P1 |
| UI-SHL-030 | Menu bar behaviour and keyboard | APP-012, APP-017, APP-022, APP-024, APP-026 | P1 |
| UI-SHL-031 | Menu popups in the menu bar | APP-012 to APP-026 | P1 |
| UI-SHL-032 | File menu | APP-012 to APP-016, PROC-095, PROC-096 | P1 to P4 |
| UI-SHL-033 | View menu (root) | APP-017, R-089 | P1 |
| UI-SHL-034 | View > Columns | APP-018, PROC-024 to PROC-026 | P1 |
| UI-SHL-035 | View > Process View | APP-019, PROC-014, PROC-017, PROC-020, PROC-043 | P1 |
| UI-SHL-036 | View > Window | APP-020, APP-009, PERF-093 | P2, P3 |
| UI-SHL-037 | View > Appearance | APP-040, R-020, R-077 | P2 |
| UI-SHL-038 | View > Theme | APP-035 to APP-038 | P2, P3 |
| UI-SHL-039 | View > Graphs | APP-021, PERF-080, PERF-084 | P1, P3 |
| UI-SHL-040 | Settings menu | APP-022, APP-043, APP-046, APP-048, APP-049, PROC-086, PROC-087, PROC-097 | P1 to P4 |
| UI-SHL-041 | Help menu | APP-024, APP-030, APP-041, APP-042, APP-053 | P2 to P4 |
| UI-SHL-042 | Debug menu | APP-026 | P4 (Show FPS P1) |
| UI-SHL-050 | Status chip | APP-029, PROC-050, PROC-051, SYS-016, SYS-027, SYS-036 | P1 |
| UI-SHL-051 | Health items in the chip | APP-029, PERF-013 | P1 |
| UI-SHL-060 | Settings window frame | APP-023, APP-032 | P2 |
| UI-SHL-061 | Settings page: General | APP-023, APP-032, APP-043 | P2 |
| UI-SHL-062 | Settings page: Appearance | APP-023, APP-032, APP-036, APP-040, R-021 | P2 |
| UI-SHL-063 | Settings page: Motion | APP-023, APP-032, R-017 | P2 |
| UI-SHL-064 | Settings page: Units | APP-023, APP-032, R-015 | P2 |
| UI-SHL-065 | Settings page: Processes | APP-023, APP-032, PROC-099, R-075 | P2 to P4 |
| UI-SHL-066 | Settings page: Performance | APP-023, APP-032, PERF-004, PERF-016, PERF-021, PERF-027, PERF-032, PERF-041, PERF-063, PERF-076, PERF-077 | P2, P3 |
| UI-SHL-067 | Settings page: Graphs | APP-023, APP-032, PERF-080, PERF-084 | P2, P3 |
| UI-SHL-068 | Settings page: Services | APP-023, APP-032, SYS-002, SYS-004 | P2 |
| UI-SHL-069 | Settings page: Startup and window | APP-023, APP-032, APP-047, APP-048, APP-051, PROC-097 | P2 to P4 |
| UI-SHL-070 | Settings page: Privileges | APP-023, APP-049, R-028 | P3, P4 |
| UI-SHL-071 | Settings page: Advanced | APP-023, APP-034, APP-046, APP-056 | P2, P4 |
| UI-SHL-072 | Reset settings confirmation | APP-022 | P1 |
| UI-SHL-080 | Theme application and follow-system (shell rules) | APP-035, APP-036 | P1, P2 |
| UI-SHL-081 | Theme Editor window | APP-037 | P3 |
| UI-SHL-082 | Share Theme window | APP-038 | P3 |
| UI-SHL-083 | Import theme from the clipboard | APP-038, R-079 | P3 |
| UI-SHL-084 | Delete theme confirmation | APP-036 | P2 |
| UI-SHL-085 | Graph Color submenu | APP-039, PERF-081, PERF-090 | P3 |
| UI-SHL-090 | Secondary windows (shell rules) | APP-045, R-073 | P1 |
| UI-SHL-091 | Confirmation windows used by the shell | APP-045, APP-022, APP-036, APP-049 | P1 |
| UI-SHL-092 | About window | APP-053, R-078 | P2 |
| UI-SHL-093 | Crash Report window | APP-055, R-049 | P1 |
| UI-SHL-094 | Additional Setup window (frame) | APP-042, PERF-105 | P3 |
| UI-SHL-095 | Keyboard Shortcuts window | APP-030, APP-031 | P2 |
| UI-SHL-096 | Public IP Notice window | APP-056, PERF-060 | P2 |
| UI-SHL-097 | Run dialog window frame | APP-014, PROC-095 | P2 |
| UI-SHL-098 | Search File Handles window frame | APP-015, PROC-096 | P3 |
| UI-SHL-099 | Collector Health window | APP-026, APP-029 | P1, P4 |
| UI-SHL-100 | Toast placement in the shell | APP-044 | P1 |
| UI-SHL-110 | Startup sequence and first frame | APP-052, APP-055, APP-059, PROC-098 | P1 to P3 |
| UI-SHL-111 | Loading placeholders (frame rule) | APP-059, R-060 | P2 |
| UI-SHL-112 | Single-instance activation | APP-011, R-047 | P1 |
| UI-SHL-113 | Close behaviour, minimize to tray, main tray icon | APP-047, PERF-084, PERF-088, PERF-090 to PERF-092 | P1, P3 |
| UI-SHL-114 | Desktop notifications from the shell | APP-055 | P1 |
| UI-SHL-120 | Key contexts and the binding table | APP-031, table (b), PERF-007 | P1 to P3 |
| UI-SHL-121 | Focus model and Tab order | APP-058 | P1 |
| UI-SHL-122 | Esc chain | table (b), R-043 | P1 |
| UI-SHL-123 | Hold Ctrl to pause | R-042, PROC-050 | P1 |
| UI-SHL-124 | Type-to-filter routing | APP-028, PROC-044 | P1 |
| UI-SHL-130 | UI scale and row height at runtime | APP-040, APP-058, R-020 | P1, P2, P4 |
| UI-SHL-131 | Language change at runtime | APP-043, R-081 | P1, P4 |
| UI-SHL-132 | Motion in the shell and reduced motion | R-017, APP-058 | P1, P2 |
| UI-SHL-140 | FPS overlay | APP-026, ADR-014 | P1 |

Entry count: 77.

## 15. Coverage

Every merged spec ID in this file's scope, and the entries that realize it.

**APP (sys-app.md), all 59:**

| ID | Realized by |
|---|---|
| APP-001 | UI-SHL-001, 002, 003, 004, 006 |
| APP-002 | UI-SHL-003, 004, 010 |
| APP-003 | UI-SHL-010, 011 |
| APP-004 | UI-SHL-012 |
| APP-005 | UI-SHL-013 |
| APP-006 | UI-SHL-014 |
| APP-007 | DROP in the merged spec; no UI. Nothing in the title bar shows an update badge (UI-SHL-003 part list has none) |
| APP-008 | UI-SHL-005 |
| APP-009 | UI-SHL-005, 036, 090 |
| APP-010 | UI-SHL-001, 002 |
| APP-011 | UI-SHL-112 |
| APP-012 | UI-SHL-030, 032 |
| APP-013 | DROP; UI-SHL-032 records that the File menu has no Run Installer item |
| APP-014 | UI-SHL-032, 097 |
| APP-015 | UI-SHL-032, 098 |
| APP-016 | UI-SHL-032 |
| APP-017 | UI-SHL-033 |
| APP-018 | UI-SHL-034 |
| APP-019 | UI-SHL-035 |
| APP-020 | UI-SHL-036 |
| APP-021 | UI-SHL-039 |
| APP-022 | UI-SHL-040, 072, 091 |
| APP-023 | UI-SHL-060 to 071 |
| APP-024 | UI-SHL-041 |
| APP-025 | DROP; no Help item for licences (UI-SHL-041 item list) |
| APP-026 | UI-SHL-042, 099, 140 |
| APP-027 | UI-SHL-020, 021, 022, 023 |
| APP-028 | UI-SHL-024, 025, 124 |
| APP-029 | UI-SHL-050, 051 |
| APP-030 | UI-SHL-095 |
| APP-031 | UI-SHL-120, 122, 123, 124 |
| APP-032 | UI-SHL-060 to 071 (every settings.toml key once); conventions (`TlSettings`) |
| APP-033 | Conventions (`TlSettings`, debounce, persist callback); UI-SHL-002 (state.toml window keys) |
| APP-034 | UI-SHL-071 |
| APP-035 | UI-SHL-038, 080, 081 |
| APP-036 | UI-SHL-038, 080, 084 |
| APP-037 | UI-SHL-081 |
| APP-038 | UI-SHL-082, 083 |
| APP-039 | UI-SHL-085 |
| APP-040 | UI-SHL-037, 130 |
| APP-041 | UI-SHL-041 (Help item); the fields themselves are PERF-095 |
| APP-042 | UI-SHL-094 |
| APP-043 | UI-SHL-040, 061, 131 |
| APP-044 | UI-SHL-100 (with UI-FND-020) |
| APP-045 | UI-SHL-090, 091, and every window entry 060, 081, 082, 092 to 099 |
| APP-046 | UI-SHL-040 (040a), 071 |
| APP-047 | UI-SHL-005, 113 |
| APP-048 | UI-SHL-002, 040, 069, 110 |
| APP-049 | UI-SHL-011, 040, 070, 091 |
| APP-050 | DROP; no updater or installer UI anywhere in the shell |
| APP-051 | UI-SHL-069 |
| APP-052 | UI-SHL-110, 112 |
| APP-053 | UI-SHL-092 |
| APP-054 | UI-SHL-092 and 093 ("Open log folder"); logging itself has no UI |
| APP-055 | UI-SHL-093, 114 |
| APP-056 | UI-SHL-096, 071 |
| APP-057 | DROP; no G-SYNC item in the Settings menu (UI-SHL-040 item list) |
| APP-058 | UI-SHL-121, 130, 132 |
| APP-059 | UI-SHL-110, 111 |

**Shell parts of perf.md:**

| ID | Realized by |
|---|---|
| PERF-004 | UI-SHL-066 (Devices group) |
| PERF-007 | UI-SHL-120 (Performance context exists on the page focus; the page binds F1 to F9, Ctrl+T, Ctrl+M) |
| PERF-013 | UI-SHL-051 (health words) |
| PERF-014 | UI-SHL-010 |
| PERF-016, PERF-021, PERF-027, PERF-032, PERF-041, PERF-063, PERF-076, PERF-077 | UI-SHL-066 (their settings keys) |
| PERF-060 | UI-SHL-096 |
| PERF-080 | UI-SHL-039, 067 |
| PERF-081 | UI-SHL-085 |
| PERF-084 | UI-SHL-039, 067, 113 |
| PERF-088 | UI-SHL-113 ("Tray icon graphs >" entry point) |
| PERF-090 | UI-SHL-085 (tray variant), 113 |
| PERF-091 | UI-SHL-113 |
| PERF-092 | UI-SHL-113 |
| PERF-093 | UI-SHL-036 |
| PERF-094 | UI-SHL-023 (copy icon) |
| PERF-105 | UI-SHL-094 |

**Shell parts of proc.md:**

| ID | Realized by |
|---|---|
| PROC-014 | UI-SHL-035 |
| PROC-015 | UI-SHL-013 |
| PROC-016 | UI-SHL-010, 013 |
| PROC-017 | UI-SHL-035 |
| PROC-020 | UI-SHL-035 |
| PROC-024, PROC-025, PROC-026 | UI-SHL-034 |
| PROC-043 | UI-SHL-035 |
| PROC-044 | UI-SHL-024, 124 |
| PROC-045 | UI-SHL-025 |
| PROC-046 | UI-SHL-012 |
| PROC-050 | UI-SHL-050, 123 |
| PROC-051 | UI-SHL-050 |
| PROC-055 | UI-SHL-014 |
| PROC-056 | UI-SHL-005, 090 |
| PROC-058 | UI-SHL-011 (Retry as administrator dispatches the shield Action) |
| PROC-086, PROC-087 | UI-SHL-023, 040, 065 |
| PROC-088 | UI-SHL-023, 050 |
| PROC-095 | UI-SHL-032, 097 |
| PROC-096 | UI-SHL-032, 098 |
| PROC-097 | UI-SHL-040, 069 |
| PROC-098 | UI-SHL-110 (`--low-memory` opens the rescue window instead of the main window) |
| PROC-099 | UI-SHL-065 |

No ID in scope is unmapped.

## 16. Decided here (all in one list)

Values and rules this file chose, for reviewers:

1. Main window title "tasklens"; title bar shrink order (UI-SHL-001).
2. Menu bar labels `t_md` at weight 400; menu buttons 26 high with padding 16 and 1 px separators 16 high (UI-SHL-003, measured).
3. Toolbar buttons 24 × 24 with gap 7 (pitch 31 measured); toolbar, title bar, tabs and caption buttons are not Tab stops (UI-SHL-003, 010, 121).
4. Own title bar drag code when the kit `TitleBar` cannot hide its controls (UI-SHL-003).
5. Main window caption buttons 36 wide (measured), pin uses the Toggle on look and tooltip "Always on top" (UI-SHL-005).
6. Right-click and middle-click on title bar buttons do nothing (UI-SHL-004).
7. Server-decorations menu row: buttons 24 high; row order follows APP-001 over ADR-010 (UI-SHL-006).
8. Inactive window: accent outline and line in `divider`, labels `text_secondary` (UI-SHL-007, inferred from the video).
9. View mode icons `rows-2`, `list-tree`, `list`; hide-system icon `layout-grid` (UI-SHL-012, 013).
10. Picker look while picking; second click cancels; PROC-055's Ctrl+click confirmation wins over APP-006 (UI-SHL-014).
11. Custom tab strip instead of kit `TabBar`; tab padding 22 (measured); active tab fill `header`; tab wrap-around; no wheel scrolling (UI-SHL-020).
12. Tab switch easing `ease_out_cubic`, crossfade realized as page fade-in, restart from the current position (UI-SHL-021).
13. Overflow menu keeps the active tab visible; icon `chevrons-right`, tooltip "More tabs" (UI-SHL-022).
14. History icon `rotate-ccw-clock` (no `history` glyph in the kit set), colours, tooltips "Show history" / "Show live processes", "Copy all"; toggling history selects Processes (UI-SHL-023).
15. Filter box: inner border off in the tab strip, Enter and Down move to the table, Ctrl+F selects all (UI-SHL-024).
16. "Include children" label `t_sm` and Tab stop after the filter (UI-SHL-025).
17. Own `MenuBar` entity; first item highlighted when opened by key; Alt alone does nothing; Alt+D for Debug (UI-SHL-030).
18. Columns submenu disabled on table-less tabs; Process View radios disabled in History mode and give the cycler's toast (UI-SHL-033, 035).
19. Appearance: steps of 5 on the 0..100 sliders; popup stays open while adjusting; keyboard on slider rows (UI-SHL-037).
20. Theme menu: per-theme actions act on the active theme; copy names "<name> copy N"; explicit choice turns sync off with "Theme sync turned off" (UI-SHL-038, 080).
21. Graph checks disabled without a GPU (UI-SHL-039).
22. Settings menu: Start minimized indent 16; Save history disabled while recording is off; toasts "History recording on/off" and "Could not change the autostart entry"; Default tab lists built tabs only; adapter probe UX (UI-SHL-040).
23. Help menu absent in P1; documentation failure toast (UI-SHL-041).
24. Debug: "All columns enabled" toast (UI-SHL-042).
25. Status as a floating chip with sizes, separators, visibility rule and table bottom padding (UI-SHL-050; R-107).
26. Health item texts, source names, limit of two, restricted item rule (UI-SHL-051).
27. Settings window size 900 × 640, sidebar 220, page icons, labels written here ("Kernel times", "Secondary graph", "Hidden engine graphs", "Mask secret environment values", "History disk limit" 64..=4096), device order list and fans group forms, locale display names, "Open settings folder" button, Show FPS on the Advanced page (UI-SHL-060 to 071).
28. Theme Editor layout, sizes, keyboard steps, "Theme saved" toast, name validation text, own colour plane (UI-SHL-081).
29. Share window size and helper sentence; import naming " 2"; missing slots fill from Default Dark or Default Light (UI-SHL-082, 083).
30. Delete theme falls back to the preferred theme of the same mode (UI-SHL-084).
31. Tray graph colour menu without swatches (UI-SHL-085).
32. Window registry keys for shell windows (UI-SHL-090).
33. About window size and licence list view; Crash Report `.seen` marker and deferral while hidden (UI-SHL-092, 093).
34. Additional Setup sizes, row card styling, busy text (UI-SHL-094).
35. Keyboard Shortcuts search field; Collector Health title, columns and colours (UI-SHL-095, 099).
36. Startup: queued toasts after the first frame; `--select-pid` waits 3 s, then "Process <pid> not found" (UI-SHL-110).
37. Skeleton counts and widths; static skeleton under reduced motion (UI-SHL-111, 132).
38. Tray icon visibility rule; compositor minimize not intercepted (UI-SHL-113).
39. Both Ctrl+? spellings bound; Ctrl+W closes secondary windows; F5 without toast; Ctrl+F does nothing on Performance (UI-SHL-120).
40. Initial focus goes to the page's primary target on open and on tab switch (UI-SHL-121).
41. Ctrl+shortcut presses pause briefly (UI-SHL-123).
42. UI scale toast id `ui-scale`, no toast at the limits, first visible row kept (UI-SHL-130).
43. FPS overlay placement, text and 2 Hz refresh (UI-SHL-140).

## 17. Open items

None. Settled in `docs/RECONCILIATION.md`: status chip form (R-107); status text size `t_sm`
(UI-FND-003); Shift held at launch on Wayland (R-124, verify U-43); kit `TitleBar` controls and
kit `Input` global shortcuts (R-125, verify U-44 and U-45); toolbar and chrome Tab stops (R-108).
