# Batch B7 ui-shell: bead plan for E20, E21, E23

This file lists every bead of batch B7: the main window frame (E20), the menus and the Settings window with the other shell windows (E21), and the process windows and dialogs (E23). The rules are in `docs/research/brief-beads.md` and `docs/plan/00-epics.md`. Epic and anchor bead IDs come from `docs/plan/anchors.md`.

Totals: E20 30 beads, E21 47 beads, E23 38 beads, 115 in all, each count including the epic's anchors.

Conventions used in every bead of this batch:

- Bead priority equals the phase number. P0 beads exist only in E20 (the frame that the E01 first-frame gate measures).
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, `ui`, plus `perf-gate` on E20-008 and `security` on E20-026, four E21 beads and 16 E23 beads (the ones that start privileged actions or handle the unlock).
- Cross-batch dependencies point at anchor beads in `anchors.md`, except the edges listed under each epic's "Edges for the leader".
- In the "Depends on" column, plan keys are beads of this batch, and short IDs such as `fut.1` are `tasklens-fut.1` in another batch. E16-A1 to E16-A3 are `fut.1` to `fut.3`, E15-A1 and E15-A2 are `ug9.1` and `ug9.2`, E17-A1 and E17-A2 are `lgw.1` and `lgw.2`, E03-A2 is `as2.2`, E19-A1 is `yrf.1`.
- Every bead that adds user-visible strings appends its Fluent keys to `assets/locales/en-US.ftl` (chain edit, R-132) in the same commit.
- Every bead's Notes carry the build rule: cargo builds and tests run through `heavy-build`, with one retry on exit 137 and a stop on exit 75.
- Every bead commits with a pathspec, `git commit -m "..." -- <files>`.

## E20 ui-shell-frame

Epic bead `tasklens-jqk`. 30 beads (2 anchors, 28 others), labels `crate-tl-ui`, `area-ui-shell-frame`, `ui`, plus `perf-gate` on E20-008 and `security` on E20-026. Covers UI-SHL-001 to UI-SHL-007, UI-SHL-010 to UI-SHL-014 (placement), UI-SHL-020 to UI-SHL-025, UI-SHL-050, UI-SHL-051, UI-SHL-100 (placement), UI-SHL-110 (UI steps 8 to 11), UI-SHL-111 (frame rule), UI-SHL-112, UI-SHL-113 (rows 2, 3 and Quit), UI-SHL-120 to UI-SHL-124, UI-SHL-130, UI-SHL-132, UI-SHL-140, and the APP, R and D entries of 00-epics section 1. Files: `actions.rs`, `focus.rs`, `settings_global.rs`, `window/{main,titlebar,toolbar,tabstrip,filter,status}.rs`, tests `ui_shell.rs`, `ui_keymap.rs`, `ui_focus.rs`, plus one-line chain edits in `app.rs`, `lib.rs`, `window/mod.rs` and `assets/locales/en-US.ftl`.

Anchor layout. E20-A1 (P0) is the last bead of the P0 frame: it depends on E20-001 to E20-008, so when it closes `TlSettings`, every shell Action, the title bar, toolbar slot, tab strip, status chip, focus rules and `MainWindow` all exist. E20-A2 (P1) closes the filter group (E20-001, E20-009 and its own type-to-filter routing). `TlSettings` is a P0 bead (E20-001) under both anchors, because the P0 window reads its size and start tab from it.

Contracts other epics use (all under E20-A1 unless marked):
- Page match: `fn page_factory(tab) -> Option<PageFactory>` in `window/main.rs`, one arm per page epic, of the form `Tab::Services => Some(|window, cx| { let page = cx.new(|cx| ServicesPage::new(window, cx)); PageSlot::new(page, cx) })`. A tab with no arm is not built. Each page epic also writes `impl ShellPage for <Page>` (`status_text`, `status_actions`, `primary_focus`, `focus_order`, `set_active`, `refresh`, all with defaults except `primary_focus`).
- Toolbar: `fn toolbar_for(tab) -> Box<dyn ToolbarContent>`, one arm per page epic with page buttons; `DefaultToolbar` otherwise. Slots 1 (shield) and 4 (picker) are drawn by the shell on every tab.
- Status: pages emit `StatusChanged` on their own entity; clickable chip parts are `StatusAction` values (E22's " | 42 restricted" link and "Administrator mode" badge).
- Shell bus: `GlobalShellEvents` emits `FilterChanged { tab, text }` (after the shell's 100 ms debounce, E20-009) and `PauseChanged(bool)` (E20-012). `ShellState` holds `main_window`, `active_tab`, `built_tabs`, `history_mode`, `paused`.
- `push_main_toast(cx, kind, text, key, action)`, `reopen_main_window(cx)`, `run_select_pid` (E20-018), `request_launch_unlock` (E20-026), `end_window_pick` (E20-027).
- Settings (anchor E20-A2): `TlSettings::{update, preview, toggle, set, settings, state, version, state_version}`, `Section`, `SettingKey` (85 variants, `ALL`, `as_str`, `section`, `kind`), `SettingValue`, `animations_on`.
- Chain-edit points for E31 (tray): `close_decision` row 1 comment and `tray_available` body in `window/main.rs` (E20-A1), the `minimize_main` comment in `window/titlebar.rs` (E20-004), and the last comment in `reopen_main_window` (E20-A1). E20-028 fills row 2.
- First-frame gate (E01): E20-008 logs `info!(target: "tasklens::perf", first_frame_ms = ..)` once per process after the first frame, measured from `/proc/self/stat` field 22 to the first frame callback. It is P0 and under E20-A1, which E01-017 already depends on.

### Beads

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E20-A1 | tasklens-jqk.1 | Main window frame: close decision, window actions, reopen and size saving | P0 | E20-001, E20-002, E20-003, E20-004, E20-005, E20-006, E20-007, E20-008, fut.2 |
| E20-A2 | tasklens-jqk.2 | Filter box and settings global: type-to-filter routing and U-45 check | P1 | E20-001, E20-009, E20-A1 |
| E20-001 | tasklens-jqk.3 | TlSettings global, SettingKey and SettingValue (settings_global.rs) | P0 | fut.1, as2.2 |
| E20-002 | tasklens-jqk.4 | Window-level Actions and bind_keys_global (actions.rs) | P0 | E20-001, fut.1, as2.2 |
| E20-003 | tasklens-jqk.5 | Toolbar slot: ToolbarContent, ToolbarModel and Toolbar (toolbar.rs) | P0 | E20-002, ug9.1, lgw.1, as2.2 |
| E20-004 | tasklens-jqk.6 | Title bar: drag region, caption buttons and menu bar slot (titlebar.rs, U-44) | P0 | E20-001, E20-002, E20-003, ug9.1, lgw.1, fut.1 |
| E20-005 | tasklens-jqk.7 | Tab strip with accent outline and tab selection (tabstrip.rs) | P0 | E20-001, E20-002, ug9.1, lgw.1 |
| E20-006 | tasklens-jqk.8 | Status chip, StatusChanged and StatusAction (status.rs) | P0 | E20-002, ug9.1, ug9.2, lgw.1, fut.1 |
| E20-007 | tasklens-jqk.9 | Focus rules: initial focus and Esc chain decision table (focus.rs) | P0 | E20-002, fut.1 |
| E20-008 | tasklens-jqk.10 | MainWindow layout, page match, shell events, window open and first_frame_ms (window/main.rs) | P0 | E20-001, E20-002, E20-003, E20-004, E20-005, E20-006, E20-007, fut.1, fut.2, yrf.1, as2.2 |
| E20-009 | tasklens-jqk.11 | Filter box with per-tab text and 100 ms debounce (filter.rs) | P1 | E20-A1, ug9.1 |
| E20-010 | tasklens-jqk.12 | Include children checkbox next to the filter box | P1 | E20-009 |
| E20-011 | tasklens-jqk.13 | Esc chain handler in the main window | P1 | E20-007, E20-009, E20-A1 |
| E20-012 | tasklens-jqk.14 | Hold Ctrl to pause: PauseChanged and the paused chip segment | P1 | E20-006, E20-A1 |
| E20-013 | tasklens-jqk.15 | Runtime UI scale keys, metrics re-apply and toast top | P1 | E20-005, E20-A1, lgw.1, fut.2 |
| E20-014 | tasklens-jqk.16 | Server-decorations menu row (Use system title bar) | P1 | E20-004, E20-A1 |
| E20-015 | tasklens-jqk.17 | Tab overflow menu | P1 | E20-005, E20-009, E20-010, fut.3 |
| E20-016 | tasklens-jqk.18 | Health items in the status chip | P1 | E20-006, E20-A1, as2.2 |
| E20-017 | tasklens-jqk.19 | Hide system processes and view mode handlers | P1 | E20-001, E20-A1 |
| E20-018 | tasklens-jqk.20 | Startup step 11: queued toasts, crash report, --select-pid, --start-minimized | P1 | E20-A1, bj6.8, pdu.3 |
| E20-019 | tasklens-jqk.21 | Single-instance activation: AppRequest handler | P1 | E20-018, E20-A1 |
| E20-020 | tasklens-jqk.22 | FPS overlay for debug.show_fps | P1 | E20-A1, yrf.1 |
| E20-021 | tasklens-jqk.23 | Tab switch animation | P2 | E20-005, E20-A1 |
| E20-022 | tasklens-jqk.24 | Always-on-top pin caption button and ToggleAlwaysOnTop handler | P2 | E20-004, E20-014, E20-A1, fut.2 |
| E20-023 | tasklens-jqk.25 | Information tab copy-all icon | P2 | E20-005, E20-015, E20-A1, 9o0.3 |
| E20-024 | tasklens-jqk.26 | Startup public IP auto-fetch | P2 | E20-018, E20-A1 |
| E20-025 | tasklens-jqk.27 | History toggle icon on the Processes tab and ToggleHistoryMode handler | P3 | E20-005, E20-015, E20-A1 |
| E20-026 | tasklens-jqk.28 | Shield toolbar slot and unlock at launch | P3 | E20-003, E20-004, E20-018, fut.3 |
| E20-027 | tasklens-jqk.29 | Window picker toolbar slot and picking mode | P3 | E20-003, E20-004, E20-011, E20-A1 |
| E20-028 | tasklens-jqk.30 | Close decision row 2: keep detached graph windows | P3 | E20-A1, fut.2 |
| E20-029 | tasklens-jqk.31 | ToggleAlwaysOnTop for secondary windows through the KWin bridge (R-176) | P2 | E20-022, fut.12, y6k.2 |

### Edges for the leader

E20 beads depend on these beads outside E20's row of 00-epics section 4: E19-A1 `tasklens-yrf.1` (E20-008 creates the window's `AnimationDriver`; E20-020 reads its stats), E22-A3 `tasklens-pdu.3` (E20-018 `--select-pid` calls `select_by_pid`), E28-A3 `tasklens-9o0.3` (E20-023 dispatches `CopyInformationAll`), E21-006 `tasklens-bj6.8` (E20-018 calls `show_pending`; same batch). None of these depends back on an E20 bead that needs them.

Edges other batches should add:
1. **E31-009 (`tasklens-wox.11`)**: approve a chain-edit rule for E31 on `window/main.rs` and `window/titlebar.rs` (00-epics section 2). Its insertion points are in E20-A1 and E20-004, both covered by its existing edge on E20-A1. No new edge needed.
2. **E29-030 (`tasklens-qfx.32`)** must depend on E20-026 and call `MainWindow::request_launch_unlock` when it detects Shift on Wayland, instead of adding its own step-11 dispatch.
3. **E30-A3 (`tasklens-8fx.3`)** must depend on E20-025. E20-025 is the only `ToggleHistoryMode` handler (it selects Processes and flips `UiState.processes.history_mode`); E30 observes the state instead of registering a handler. This answers E30-OI-7.
4. **E22** must call `MainWindow::end_window_pick` when the PROC-055 pick result arrives (E20-027), and its page-arm beads write `impl ShellPage for ProcessesPage`. The same `impl ShellPage` applies to E24, E27 and E28 page-arm beads.

### Open items

1. **`RefreshCurrentTab` instead of `RefreshNow`.** UI-SHL-120 names the F5 Action `RefreshNow`; E27 and E28 beads use `RefreshCurrentTab`, and `RefreshNow` is also an engine `Command` variant. E20-002 declares `RefreshCurrentTab`. Recommendation: accept and edit UI-SHL-120.
2. **`WindowKey::Main`.** UI-SHL-005 and E21-008 dispatch `ToggleAlwaysOnTop { window: WindowKey::Main }`, but E16-A2's `WindowKey` has no `Main`. Recommendation: add `Main` to `WindowKey` in E16-008 (never stored in the registry, no cap class). E20-022 stops if it is missing.
3. **Keep-above for secondary windows.** E16-008 leaves the `ToggleAlwaysOnTop` handler to the shell, but `KWinCommand::KeepAbove` needs the window caption and the Action carries only the key. E20-022 handles `Main` only. Recommendation: add `caption: SharedString` to the Action in E16-008 and a follow-up E20 bead for the secondary case.
4. **X11 keep-above.** APP-009 and UI-SHL-005 name an X11 `_NET_WM_STATE_ABOVE` path "through the tl-engine X11 helper"; tl-model `command.rs` has no command for it. E20-022 is KWin only. Recommendation: drop the X11 path (X11 is best effort, ADR-010) or add a command in E03.
5. **`QuitMode::Explicit`.** ADR-010 requires it; E20-A1 looks for the gpui-pre 0.3.7 API and reports if the app quits when its last window closes, which would block close rows 1 and 2.
6. **Double debounce on the Processes filter.** The shell debounces 100 ms before `FilterChanged` (UI-SHL-024); E22-024 (`tasklens-pdu.24`) debounces another 100 ms. Recommendation: E22-024 drops its own debounce.
7. **First visible row across a scale change.** UI-SHL-130 step 5 and test `scale_keeps_first_row` belong to the table (E18 `tree_table.rs`), which has no such bead. Recommendation: one E18 follow-up bead.
8. **Restricted health item caller.** `StatusChip::note_restricted_refusal` (E20-016) has no caller; the ActionTracker (E16-A3) knows about EPERM refusals. Recommendation: E16-A3 calls it (or emits an event the chip subscribes to) when a refusal happens with tl-helper missing.
9. **Health items: sources per tab and names.** UI-SHL-051 says "sources the active tab uses" without a list. E20-016 decides the mapping (Processes: processes plus the minigraph sources; Performance: CPU, memory, disk, network, GPU, sensors; Services: both systemd managers; Startup Apps, Connections, Information: their one source) and the names E21 open item 6 recommends. Confirm.
10. **`WindowRegistry` key listing and notify.** E20-028 needs to know whether any `DetachedGraph(_)` window is open and to be told when one closes. Recommendation: E16-A2 adds a `has_key(|k| ..)` accessor and calls `cx.notify()` on removal.
11. **`SourceId::PublicIp`.** E20-024 sends `Command::RefreshNow(SourceId::PublicIp)` as E03-056 records; E03 lists the public IP source as an open item. E20-024 stops if the variant is missing.
12. **Start hidden before the tray exists.** `--start-minimized` hides the window only when a tray setting is on and `tray_available` is true; `tray_available` returns false until E31-009 fills it, so P1 to P2 always open shown and then minimize (APP-048).
13. **`ToggleMainWindow` while visible without a tray.** UI-SHL-113 defines the tray click only with a tray. E20-A1 minimizes the window when the close decision would quit. Confirm.

## E21 ui-shell-menus-settings

Epic bead `tasklens-bj6`. 47 beads (2 anchors, 45 others), against an estimate of 40: the Settings window is split by page, and the menus by phase, so each bead stays within one to three files. Labels `crate-tl-ui`, `area-ui-shell-menus-settings`, `ui`, plus `security` on E21-031, E21-033, E21-035 and E21-042. Covers the menu bar and every menu (UI-SHL-030 to UI-SHL-042) and the Settings window (UI-SHL-060 to UI-SHL-072). It also covers the other shell windows in UI-SHL-080 to UI-SHL-096 and UI-SHL-099: About, Crash Report, Collector Health, Keyboard Shortcuts, Public IP Notice, Theme Editor and Share Theme. The APP entries that 00-epics section 1 gives E21 are included. Files: `window/menubar.rs`, `window/menus_*.rs`, `window/settings/*.rs`, `window/{about,confirm,crash_report,collector_health,shortcuts,public_ip,theme_editor,share_theme}.rs`, tests `ui_menus.rs`, `ui_settings_window.rs` and `ui_shell_windows.rs`, plus chain edits in `app.rs`, `window/mod.rs` and `assets/locales/en-US.ftl`.

Anchor layout. E21-A1 (P1) is the `MenuBar` entity and the File menu; every other menu bead depends on it and adds its menu through the builder registry it creates. It depends on E20-A1, because the menu bar sits in the title bar through E20-004's `MenuBarFactory`. E21-A2 (P2) is the Advanced page and the window-level key checks. It depends on the frame (E21-018), every P2 page bead (E21-019 to E21-026) and the reset confirmation (E21-005). So when it closes, every P2 page file exists for other epics to chain-edit. P3 and P4 rows (E21-033 to E21-043) come after the anchor.

The shell pieces E21 needs (menu bar slot, menu row height, every shell Action, `TlSettings`, the keep-above check) are E20 beads in this same batch, so E21 has no open request on another batch for them.

Two E21 beads also depend on E20-013 (runtime scale, `tasklens-jqk.15`), which the table does not show: E21-005, because a reset puts the UI scale back to its default, and E21-009, because the UI Scale slider applies the scale live.

### Beads

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E21-A1 | tasklens-bj6.1 | Menu bar entity and File menu | P1 | jqk.1, fut.1, fut.3, ug9.1, lgw.1 |
| E21-A2 | tasklens-bj6.2 | Settings window: Advanced page and P2 key checks | P2 | E21-005, E21-018, E21-019, E21-020, E21-021, E21-022, E21-023, E21-024, E21-025, E21-026, jqk.2 |
| E21-005 | tasklens-bj6.3 | Reset settings confirmation and ResetSettings action | P1 | fut.1, fut.2, jqk.2, lgw.1, as2.2 |
| E21-001 | tasklens-bj6.4 | View menu root and View > Columns submenu | P1 | E21-A1, rsf.2, jqk.1, jqk.2 |
| E21-002 | tasklens-bj6.5 | View > Process View submenu | P1 | E21-001, pdu.1, rsf.1, jqk.2 |
| E21-003 | tasklens-bj6.6 | View > Graphs submenu (P1 items) | P1 | E21-001, xdh.1, jqk.2, as2.2 |
| E21-004 | tasklens-bj6.7 | Settings menu with every P1 item | P1 | E21-A1, E21-005, jqk.2 |
| E21-006 | tasklens-bj6.8 | Crash Report window | P1 | fut.1, fut.2, fut.3, ug9.1, cr0.1, as2.2 |
| E21-007 | tasklens-bj6.9 | Collector Health window | P1 | fut.1, fut.2, ug9.1, as2.1, jqk.1 |
| E21-044 | tasklens-bj6.10 | File menu: Run... item | P2 | E21-A1, jqk.1 |
| E21-008 | tasklens-bj6.11 | View > Window: Always On Top item | P2 | E21-001, fut.2, jqk.1 |
| E21-009 | tasklens-bj6.12 | Menu slider rows and View > Appearance submenu | P2 | E21-001, ug9.2, jqk.2 |
| E21-010 | tasklens-bj6.13 | Settings menu P2 rows: Tab Switch, Public IP auto-fetch, All settings | P2 | E21-004, E21-009 |
| E21-011 | tasklens-bj6.14 | Settings menu: Start at login and Start minimized | P2 | E21-004, cr0.2, jqk.2 |
| E21-012 | tasklens-bj6.15 | View > Theme submenu: painted theme rows, ActivateTheme, sync | P2 | E21-001, lgw.1, lgw.2, jqk.2 |
| E21-013 | tasklens-bj6.16 | View > Theme: Create copy and Delete theme with confirmation | P2 | E21-012, E21-005, lgw.2, fut.2 |
| E21-015 | tasklens-bj6.17 | About window | P2 | fut.1, fut.2, fut.3, ug9.1, ug9.2, cr0.1 |
| E21-014 | tasklens-bj6.18 | Help menu with P2 items | P2 | E21-A1, E21-015, jqk.1, 9o0.3 |
| E21-016 | tasklens-bj6.19 | Keyboard Shortcuts window | P2 | fut.1, fut.2, ug9.1, jqk.1 |
| E21-017 | tasklens-bj6.20 | Public IP Notice window and fetch entry point | P2 | fut.2, ug9.1, ug9.2, jqk.2 |
| E21-018 | tasklens-bj6.21 | Settings window frame, page list and PageBuilder | P2 | fut.1, fut.2, ug9.1, ug9.2, jqk.1, jqk.2 |
| E21-019 | tasklens-bj6.22 | Settings window: General and Units pages | P2 | E21-018 |
| E21-020 | tasklens-bj6.23 | Settings window: Appearance page | P2 | E21-018, E21-012 |
| E21-021 | tasklens-bj6.24 | Settings window: Motion page | P2 | E21-018, ug9.2 |
| E21-022 | tasklens-bj6.25 | Settings window: Processes page (P2 rows) | P2 | E21-018 |
| E21-023 | tasklens-bj6.26 | Settings window: Performance page (P2 rows) | P2 | E21-018, as2.2 |
| E21-024 | tasklens-bj6.27 | Settings window: device order and visibility list | P2 | E21-023 |
| E21-025 | tasklens-bj6.28 | Settings window: Graphs and Services pages | P2 | E21-018, xdh.1 |
| E21-026 | tasklens-bj6.29 | Settings window: Startup and window page (P2 rows) | P2 | E21-018, E21-011, E21-008 |
| E21-027 | tasklens-bj6.30 | Theme Editor window frame, slot list, Cancel and Done | P3 | E21-012, lgw.2, fut.2, ug9.1, ug9.2 |
| E21-028 | tasklens-bj6.31 | Theme Editor colour plane, hue strip, readouts and hex input | P3 | E21-027 |
| E21-030 | tasklens-bj6.32 | Share Theme window | P3 | lgw.2, fut.2, ug9.1 |
| E21-031 | tasklens-bj6.33 | View > Theme P3 items and clipboard theme import | P3 | E21-013, E21-027, E21-030, lgw.2 |
| E21-029 | tasklens-bj6.34 | Theme Editor preview pane, Random, Share and Import | P3 | E21-028, E21-030, E21-031 |
| E21-032 | tasklens-bj6.35 | Graph Color submenu builder | P3 | E21-A1, lgw.1 |
| E21-045 | tasklens-bj6.36 | File menu: Search File Handles... item | P3 | E21-044 |
| E21-033 | tasklens-bj6.37 | Settings menu P3 rows: history, tray, unlock at launch, rescue window | P3 | E21-010 |
| E21-034 | tasklens-bj6.38 | View and Help menu P3 rows: mini window, keep detached graphs, Additional setup | P3 | E21-008, E21-003, E21-014, qfx.3 |
| E21-035 | tasklens-bj6.39 | Settings window: Privileges page | P3 | E21-A2, qfx.3 |
| E21-036 | tasklens-bj6.40 | Settings window P3 rows: Dump type, disconnected fans, keep detached graphs | P3 | E21-022, E21-023, E21-025, E21-A2 |
| E21-037 | tasklens-bj6.41 | Settings window: Fans group on the Performance page | P3 | E21-023, E21-A2 |
| E21-038 | tasklens-bj6.42 | Debug menu | P4 | E21-A1, E21-007, rsf.1, xdh.1 |
| E21-039 | tasklens-bj6.43 | File > Open System Monitor and Help > Documentation | P4 | E21-045, E21-034, y6k.3 |
| E21-040 | tasklens-bj6.44 | Rendering GPU submenu and Advanced page row | P4 | E21-004, E21-A2 |
| E21-041 | tasklens-bj6.45 | Settings window P4 rows: column separators, Ctrl+Esc, TaskSlinger import | P4 | E21-022, E21-026, E21-A2, cr0.2 |
| E21-042 | tasklens-bj6.46 | Skip password prompts menu item, Privileges row and confirmation | P4 | E21-033, E21-035, E21-005, qfx.2, qfx.3 |
| E21-043 | tasklens-bj6.47 | Settings window check: every settings.toml key exactly once | P4 | E21-035, E21-036, E21-037, E21-040, E21-041, E21-042 |

### Edges for the leader

E21 depends only on anchors across epics. These are the places where E21 needs something from a non-anchor bead of another batch.

1. **Items 1 to 5 of the first draft (menu bar slot, shell Actions, `TlSettings` API, keep-above check, crash report trigger) are settled inside B7 by E20:** `MenuBarFactory` and `MenuRowHeight` (E20-004, E20-014), the Actions in `actions.rs` (E20-002), `ShellState` (E20-008), `TlSettings::update` / `preview` / `toggle` / `set` with `SettingKey::ALL`, `as_str`, `kind` (E20-001), `keep_above_supported` (E20-004), and `show_pending` called by E20-018 (which depends on E21-006). The E21 bead texts were updated to name these. Two extra edges were added: E21-005 and E21-009 depend on E20-013 (the scale re-apply observer).
2. **E26 uses `graph_color_menu` (E21-032).** The E26 card-menu colour bead must depend on E21-032.
3. **E25 and E28 Fetch buttons use `request_public_ip_fetch` (E21-017).** The Network page Fetch bead (E25) and the Information tab Fetch bead (E28) must depend on E21-017.
4. **E29 chain edit to `settings/privileges.rs`** must depend on E21-035 (the file is created there, in P3), not only on E21-A2.
5. **E21-043 (full every-key check)** must depend on the E30 bead that adds the History rows to `settings/processes.rs`, the E31 bead that adds the tray rows and the E32 bead that adds the rescue-window row to `settings/startup_window.rs`.
6. **E17 share-code codec.** E21-029, E21-030 and E21-031 call the `TL1.`/`TS1.` codec in `theme/share_code.rs`, which is not in an E17 anchor (E17-A2 lists `user_store.rs` and `preview.rs`). They need a dependency on E17's share-code bead.
7. **E18 `EnableAllColumns`** (see open item 8 below) and **E26 `AddRandomMinigraphs`** are dispatched by E21-038; if they are not in E18-A1 and E26-A1, E21-038 needs the beads that declare them.

### Open items

1. **tl-app to tl-ui handoff.** Several E21 beads need things only tl-app has, and there is no decided interface (the only one in the docs is the persist callback `Box<dyn Fn(&Settings, &State)>`): the `--debug`/`TASKLENS_DEBUG` flag (E21-038), the logs and config directories (E21-006, E21-015, E21-A2), the pending crash report and its mark-seen call (E21-006), the autostart writer (E21-011), kglobalaccel get and set (E21-041), the TaskSlinger import (E21-041), build info (E21-015) and the adapter probe (E21-040). Recommendation: one `AppEnv` struct passed into `TasklensUi::init` (E16-A1) and stored as a Global, filled by E33 (E33-A1 and E33-A2), with plain fields for flags, paths and build info and `Arc<dyn Fn ..>` members for the calls.
2. **Reset settings failure.** UI-SHL-072 wants the Error toast "Failed to reset settings" with nothing changed on a failed write, but the persist callback is fire-and-forget with a 500 ms debounce. Recommendation: an `AppEnv::write_settings_now(&Settings) -> Result<(), String>` used by the reset path before `TlSettings` is replaced.
3. **Name of the System Information Action (decided in E20-002, confirm).** shell.md UI-SHL-041 says `OpenSystemInformation`; system-tabs.md UI-SYS-096 says `ShowSystemInformation` and that shell.md owns it. Recommendation: `ShowSystemInformation`, declared in E20-A1 `actions.rs`, handled by E28-A3.
4. **Keyboard Shortcuts headings.** UI-SHL-095 lists seven headings (Global, Tables, Processes, Services, Performance, Dialogs, Startup) but table (b) has more contexts (ConnectionTable, StartupTable, History, Information, ColumnPicker, PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog, HandleSearch, JournalList, LowMemory). Recommendation: headings Global, Tables, Processes, History, Services, Startup, Connections, Information, Performance, Properties windows (PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog), Dialogs (RunDialog, Picker, ColumnPicker, HandleSearch, JournalList, LowMemory).
5. **About build values.** APP-053 wants a git hash, build date, GPUI revision and an embedded `cargo-about` licence list, but no file in section 2 owns a `build.rs` or the generated licence file. Recommendation: `crates/tl-app/build.rs` (E33) emits `TASKLENS_GIT_HASH`, `TASKLENS_BUILD_DATE`, `TASKLENS_GPUI_REV` (from `Cargo.lock`) and writes the licence JSON that tl-app embeds and passes through `AppEnv`.
6. **Collector Health source names.** UI-SHL-051 names 12 sources; `SourceId` has more (logind, udisks, UPower, NetworkManager, public IP, history and others). Recommendation: "Logind", "UDisks", "UPower", "NetworkManager", "Public IP", "History", "Journal", and the `SourceId` variant name for any other. Decided (tasklens-bj6.9): UI-SHL-099 in `docs/ui/shell.md` lists all 21 names.
7. **Fans group partial entries and empty state** (E21-037). UI-SHL-066 says an empty label plus "None" removes the entry but not what is stored when only one is set, nor what the group shows with no fans. Recommendation: store `{ label = "" , temp = ".." }` or `{ label = ".." }` (omit `temp` for None); hide the group when there are no fans.
8. **`EnableAllColumns` owner.** The Debug menu item needs a handler that changes column visibility, which is E18's state. Recommendation: E18 declares and handles `EnableAllColumns { table: TableId }` in `table/actions.rs`.
9. **Rendering GPU adapter probe location** (E21-040). `wgpu` is not in tl-ui's allowed row. Recommendation: tl-app runs the probe (`AppEnv::list_adapters`).
10. **TaskSlinger import failure toast** (E21-041). APP-034 gives only the success text "Imported N settings and M themes". Recommendation: Error toast "Could not import TaskSlinger settings".
11. **Passwordless setup step and result** (E21-042). The `SetupStep` variant for `polkit-rule on|off` and how the UI learns the result belong to E03 and E29. Recommendation: `SetupStep::PolkitRule(bool)` and the E29 `SetupEvent` result stream; E21-042 waits on E29-A2 and E29-A3.

## E23 ui-process-windows

Epic bead `tasklens-gma`. Covers UI-PWN-001 to UI-PWN-031 and UI-PWN-034 to UI-PWN-036 (UI-PWN-032 and 033 are E32), UI-SHL-097 and 098, UI-PRC-021, PROC-065 to PROC-067, PROC-069 (dialog side), PROC-074, PROC-075, PROC-077 to PROC-085, PROC-095, PROC-096, APP-014, APP-015, U-15 (noted in the Run bead). All code is in `crates/tl-ui/src/windows/` plus chain edits to `pages/processes/{menu,mod}.rs` (E22), `windows/mod.rs`, `app.rs` and `assets/locales/en-US.ftl` (E16). Tests: `tests/ui_props.rs`, `tests/ui_dialogs_process.rs`, `tests/ui_run.rs`, `tests/ui_handle_search.rs`.

Anchor layout: E23-A1 is the `PropsWindow` integration bead (it closes after the frame, actions, filter and General layout beads). E23-A2 is `windows/signal.rs` and depends on the Set Affinity and Custom priority beads, so when it closes all three dialog files exist. E23-A3 is `windows/notices.rs` with the VirusTotal flow; the P3 dump toasts extend it later.

Action naming: an Action named in the UI docs keeps that name (`SetAffinity`, `SendSignal { signo }`, `OpenVirusTotal`, `CreateDump`, `OpenCustomPriority`, `PropsSelectTab`, `SetThreadNice`, `SetThreadSched`, `SignalThread`, `RunCommand`, ...); a name the docs do not give gets the R-133 prefix. E23 declares every Action in `windows/actions.rs` except the shell's (`OpenProperties`, `OpenRunDialog`, `OpenHandleSearch`, `FocusFilter`) and `TlShield`'s (`StartHelperSession`).

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E23-A1 | tasklens-gma.1 | Process Properties window and General tab (PropsWindow, props/mod.rs) | P2 |
| E23-A2 | tasklens-gma.2 | Action dialogs: Send Signal submenu and Real-time signal dialog (windows/signal.rs) | P2 |
| E23-A3 | tasklens-gma.3 | Notices: VirusTotal notice and hashing toast (windows/notices.rs) | P2 |
| E23-001 | tasklens-gma.4 | Process window Actions and key bindings (windows/actions.rs) | P1 |
| E23-002 | tasklens-gma.5 | Process window frame specs, titles and Esc order (windows/frame.rs) | P1 |
| E23-003 | tasklens-gma.6 | Set Affinity dialog for one process (windows/affinity.rs) | P1 |
| E23-004 | tasklens-gma.7 | E23: add Set Affinity item and handler to pages/processes/menu.rs and mod.rs | P1 |
| E23-005 | tasklens-gma.8 | Set Affinity with several targets, mixed marks and thread targets | P2 |
| E23-006 | tasklens-gma.9 | Custom priority dialog and Realtime confirmation (windows/priority.rs) | P2 |
| E23-007 | tasklens-gma.10 | E23: add Custom priority item and handler to pages/processes/menu.rs and mod.rs | P2 |
| E23-008 | tasklens-gma.11 | E23: add Send Signal submenu and handlers to pages/processes/menu.rs and mod.rs | P2 |
| E23-009 | tasklens-gma.12 | Properties per-tab filter bar and debounce (windows/props/filter.rs) | P2 |
| E23-010 | tasklens-gma.13 | Properties General tab layout, fields and formatters (props/general.rs) | P2 |
| E23-011 | tasklens-gma.14 | Properties General tab copy, links, field menu and keyboard | P2 |
| E23-012 | tasklens-gma.15 | Properties General tab hashes and Verify against package | P2 |
| E23-013 | tasklens-gma.16 | Properties General tab window block | P2 |
| E23-014 | tasklens-gma.17 | Properties and App Properties Unit link to Go to service | P2 |
| E23-015 | tasklens-gma.18 | E23: add OpenProperties handler and Properties item to pages/processes/mod.rs and menu.rs | P2 |
| E23-016 | tasklens-gma.19 | Properties Threads tab table (props/threads.rs) | P2 |
| E23-017 | tasklens-gma.20 | Properties Threads row menu and thread actions | P2 |
| E23-018 | tasklens-gma.21 | Properties File descriptors tab with details pane (props/fds.rs) | P2 |
| E23-019 | tasklens-gma.22 | Properties File descriptors Go to connection item | P2 |
| E23-020 | tasklens-gma.23 | Properties Memory maps tab (props/maps.rs) | P2 |
| E23-021 | tasklens-gma.24 | Properties Security tab summary (props/security.rs) | P2 |
| E23-022 | tasklens-gma.25 | Properties Security tab Capabilities and Groups tables | P2 |
| E23-023 | tasklens-gma.26 | Properties Environment tab with secret masking (props/environ.rs) | P2 |
| E23-024 | tasklens-gma.27 | Properties Strings tab scan, progress and results (props/strings.rs) | P3 |
| E23-025 | tasklens-gma.28 | Properties Grant access and Enable buttons for restricted tabs | P3 |
| E23-026 | tasklens-gma.29 | App Properties window and General tab (windows/app_props.rs) | P2 |
| E23-027 | tasklens-gma.30 | App Properties Processes tab | P2 |
| E23-028 | tasklens-gma.31 | Run dialog command history store and matcher (windows/run_history.rs) | P2 |
| E23-029 | tasklens-gma.32 | Run window with command combo and launch (windows/run.rs) | P2 |
| E23-030 | tasklens-gma.33 | Search File Handles window, search and progress (windows/handle_search.rs) | P3 |
| E23-031 | tasklens-gma.34 | Search File Handles results table and row menu | P3 |
| E23-032 | tasklens-gma.35 | E23: add Open in VirusTotal item and handler to pages/processes/menu.rs and mod.rs | P2 |
| E23-033 | tasklens-gma.36 | Dump confirmation, progress and result toasts (windows/notices.rs) | P3 |
| E23-034 | tasklens-gma.37 | E23: add Create dump file item and handler to pages/processes/menu.rs and mod.rs | P3 |
| E23-035 | tasklens-gma.38 | Regression test: process window confirmations use UI-PRC-014 texts | P3 |

38 beads (3 anchors, 35 others). Anchor titles were renamed from the placeholders to say what each delivers; plan keys and IDs are unchanged.

### Edges for the leader

- Cross-batch anchors used beyond the E23 row of 00-epics section 4: E20-A2 `tasklens-jqk.2` (settings global for `properties.*` switches and the VirusTotal state flag: E23-018, E23-020, E23-023, E23-A3), E27-A2 `tasklens-hk6.2` (Go to service Action: E23-014), E28-A2 `tasklens-9o0.2` (Go to connection by inode: E23-019). None creates a cycle (E27 and E28 do not depend on E23). All three edges are already added.
- No edge to an E20 or E21 non-anchor bead was needed.
- Tell B8 (E22): E23 adds row-menu items 5 (Send Signal), 7 (Set Affinity…), 19 (Properties), 22 (Open in VirusTotal), 23 (Create dump file) and "Custom…" in the Set Priority submenu, and declares their Actions (`SendSignal { signo }`, `SetAffinity`, `OpenVirusTotal`, `CreateDump`, `OpenCustomPriority`) in `windows/actions.rs`. E22 must not declare these, or GPUI fails on duplicate names. E22 keeps Realtime and Idle in Set Priority and owns `MenuTargets`, `confirm.rs` (`ConfirmKind::Realtime`, the dump and VirusTotal rows) and `select_by_key`. 00-epics names only the Properties, affinity, priority and signal items for E23; the VirusTotal and dump items were unassigned and E23 took them because it owns their flows.

### Open items

1. **Double-click on a process row.** UI-PRC-002 says double-click runs `OpenProperties`; UI-PWN-003 says double-click is not bound (it expands, like TaskSlinger). Affects E22's gesture binding; E23-015 works either way. Recommendation: follow UI-PWN-003 (D-008 lists only Enter, Ctrl+I, Alt+Enter).
2. **Set Affinity is P1 but anchor E23-A2 is P2.** PROC-067 and UI-PWN-021 make the one-target dialog P1. It is the non-anchor bead E23-003 (P1) with E23-001, E23-002 and E23-004 also P1; E23-A2 stays P2 as asked. Recommendation: accept.
3. **Run history write route and timestamp** (E23-028). ADR-001 forbids `tl-ui -> tl-ops`, so `write_atomic` is out of reach, and `tl-ui` has no date crate for `executedAt`. Recommendation: `tl-app` passes a `RunHistoryStore` callback (load, save) into `TasklensUi::init` built on `write_atomic`, and `executedAt` is RFC 3339 local time from a `tl_model::units` helper.
4. **Dump file name sanitising** (E23-033). `{name}` can contain `/`. Recommendation: replace `/` and control characters with `_`, cut to 64 bytes.
5. **"Shared libraries only" and the Primary and "Outside loaded modules" nodes** (E23-020). PROC-081 lists only the kept file patterns. Recommendation: keep both nodes visible with the box on.
6. **After "Grant access" succeeds** (E23-025). UI-PWN-018 says the window re-sends its demand "with the helper flag", but `DetailDemand` has no such flag. Recommendation: no flag; the UI sends `Command::DetailRefresh { target, tab }` once when health reports the helper session active, and E29's chain edit in `process_detail.rs` routes Restricted targets through the helper.
7. **"Grant access" or "Grant access…".** The accepted strings list (R-121) has "Grant access…"; UI-PWN-018 and the PROC texts use "Grant access". Beads use "Grant access". Recommendation: drop the "…" entry from the list.
8. **Toast texts through `ActionTracker`.** The dialog beads pass their success and failure texts to the tracker when tracking a request; the E16-A3 tracker API must accept per-request texts (including the joined toast id for grouped affinity requests and the SetSched-then-SetNice pair). Recommendation: confirm with B5 that `ActionTracker::track` takes a texts struct and an optional shared toast id.
