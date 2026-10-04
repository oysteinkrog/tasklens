# UI-side open items, settled

Settles the open items raised by the UI bead batches: `b5-ui-foundation.md` (E15 to E18, items 1
to 38), `b6-ui-performance.md` (E19 items 1 to 5, E24 items 6 to 10, E25-OI-1 to 8, E26-OI-1 to 7),
`b7-ui-shell.md` (E20 items 1 to 13, E21 items 1 to 11, E23 items 1 to 8) and `b8-ui-tables.md`
(E22 items 1 to 19, E27 items 1 to 9, E28 items 1 to 15), plus the cross-batch notes in
`leader-notes.md`. Rules and precedence: `docs/research/brief-openitems.md`.

Raiser IDs are written `B5-13` (batch 5, item 13), `B7-E21-4` (batch 7, E21 list, item 4),
`E25-OI-4` and `E26-OI-3` (the B6 lists keep their own IDs). Bead IDs are `tasklens-<epic>.<n>`;
the `tasklens-` prefix is dropped in the text and kept in the Edges section.

Engine-side topics are owned by `open-items-engine.md` (oi-engine, OI-Enn IDs). This file points to them as
"see engine side" and lists them once in OI-U65 with their OI-E IDs. Topics that have a UI half and an engine half
state the UI half here.

Tags: [verified] the cited doc or source says it; [inferred] follows from the docs or the kit
source; [guess] a writer-level choice no doc settles.

---

## A. The tl-app to tl-ui handoff

### OI-U01 `UiInit` and `AppEnv`: everything tl-app hands to `TasklensUi::init`
- Raised by: B5-4, B5-5, B5-29, B7-E21-1, B7-E21-2, B7-E21-5 (UI half), B7-E21-9, B7-E23-3 (UI half)
- Question: ADR-009 gives `TasklensUi::init(cx, client, settings)`, ADR-011 gives one persist callback. The UI beads need state, the themes text, theme persistence, the AppRequest receiver, queued toasts, and about ten things only tl-app has (debug flag, directories, crash report, autostart, kglobalaccel, TaskSlinger import, build info, adapter probe, synchronous settings write, run history). What is the one contract?
- Answer: `TasklensUi::init(cx: &mut App, client: Arc<dyn EngineClient>, init: UiInit)` as E16-A1 (fut.1) already writes it, with one added field `env: AppEnv`. Final shape, all in `crates/tl-ui/src/app.rs`:
  ```rust
  pub struct UiInit {
      pub settings: Settings,
      pub state: UiState,
      pub themes_json: Option<Arc<str>>,                 // raw themes.json text, None when missing
      pub persist: Box<dyn Fn(&Settings, &UiState)>,     // ADR-011, tl-app debounces 500 ms
      pub persist_themes: Box<dyn Fn(PersistTheme)>,     // PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}
      pub app_requests: async_channel::Receiver<AppRequest>,
      pub startup_toasts: Vec<(ToastKindTag, SharedString)>,
      pub env: AppEnv,
  }
  pub struct BuildInfo { pub version: &'static str, pub git_hash: &'static str,
                         pub build_date: &'static str, pub gpui_rev: &'static str,
                         pub licenses_json: &'static str }        // cargo-about output, embedded by tl-app
  pub struct CrashReport { pub path: PathBuf, pub text: Arc<str>, pub at: SystemTime }
  pub struct AdapterInfo { pub name: String, pub backend: String, pub device_type: String }
  pub struct RunHistoryEntry { pub command: String, pub executed_at: String /* RFC 3339 */, pub elevated: bool }
  pub struct ImportSummary { pub settings: u32, pub themes: u32 }
  pub struct AppEnv {
      pub debug: bool,                                   // --debug or TASKLENS_DEBUG (table c)
      pub config_dir: PathBuf, pub state_dir: PathBuf, pub logs_dir: PathBuf,
      pub build: BuildInfo,
      pub pending_crash: Option<CrashReport>,            // UI-SHL-093; None when the .seen marker exists
      pub mark_crash_seen: Arc<dyn Fn()>,
      pub write_settings_now: Arc<dyn Fn(&Settings) -> Result<(), String>>,   // synchronous, for Reset
      pub set_autostart: Arc<dyn Fn(bool) -> Result<(), String>>,
      pub global_shortcut: Option<GlobalShortcutApi>,    // None outside KDE
      pub import_taskslinger: Arc<dyn Fn() -> Result<ImportSummary, String>>,
      pub list_adapters: Arc<dyn Fn() -> Vec<AdapterInfo>>,
      pub run_history: Option<Box<dyn RunHistoryStore>>, // None in tests; tl-app builds it on write_atomic (OI-E08, OI-E32)
      pub now_rfc3339: Arc<dyn Fn() -> String>,          // local time, for executedAt; drops if jiff joins the tl-ui row (OI-E04)
  }
  pub trait RunHistoryStore { fn load(&self) -> Vec<RunHistoryEntry>;
                              fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String>; }
  pub struct GlobalShortcutApi { pub get: Arc<dyn Fn() -> Option<String>>,
                                 pub set: Arc<dyn Fn(Option<&str>) -> Result<(), String>> }
  impl Global for AppEnv {}
  ```
  `init` step 8 installs `AppEnv` as a Global next to `BootSettings`. Every closure runs on the main thread; none is `Send`. `PersistTheme` is declared in `app.rs` by E16-A1 and moved to `theme/user_store.rs` with a re-export by E17-008 (lgw.10), as B5-29 says. Reset settings (UI-SHL-072) calls `write_settings_now` before replacing `TlSettings` and shows "Failed to reset settings" on `Err`, with nothing changed. `executedAt` comes from `now_rfc3339`, so tl-ui needs no date crate and `tl_model::units` gains nothing; OI-E32 proposes `jiff` in tl-ui instead, and if OI-E04 lists it there the closure goes and tl-ui formats the time itself. The store sits inside `AppEnv`, not as a bare `UiInit` field, so every tl-app hand-off has one home. The adapter probe runs in tl-app (`wgpu` stays out of tl-ui). A `FakeEngine` test gets `AppEnv::test()` with empty closures and `debug = false`.
- Basis: ADR-001 tl-ui dependency row [verified]; ADR-009 and ADR-011 (the contract is extended, not changed) [verified]; UI-SHL-072, UI-SHL-092, UI-SHL-093, UI-PWN-028, APP-053, PROC-095 for the field list [verified]; field types [inferred].
- Owner decision: no
- Changes: ARCHITECTURE.md ADR-009 "Entry point" and ADR-011: replace the `(cx, client, settings)` signature with `UiInit` and name `AppEnv` and `persist_themes`. Bead fut.1: add `env: AppEnv` and the types above to the Deliverable and the Global to step 8. Beads bj6.8, bj6.14, bj6.17, bj6.42, bj6.44, bj6.45 (E21-006, 011, 015, 038, 040, 041), bj6.3 (Reset) and gma.31 (E23-028): replace "needs an interface from tl-app" with the named `AppEnv` member. Engine side fills every member (E33-A1 cr0.1, E33-A2 cr0.2, see engine side).

### OI-U02 `tl_ui::run(UiLaunch) -> UiExit`
- Raised by: B5-38
- Question: tl-app cannot depend on gpui-kit, so who opens the application?
- Answer: see engine side (OI-E15). UI half: E16-012 (fut.15) builds `tl_ui::run(UiLaunch) -> UiExit` with `UiLaunch { init: UiInit, open_mode: OpenMode, start: StartOptions }` and `UiExit::{Quit, RendererFailed(String), DeviceLost}`, exactly as B5-38 states. `debug` and `on_quit` are not in `UiLaunch` (`debug` is `AppEnv.debug`). OI-E15 adds one optional field `platform: Option<Arc<dyn PlatformOps>>`, which the UI accepts as given.
- Basis: B5-38, leader-notes "E33 needs `tl_ui::run(UiLaunch)` fut.15" [verified].
- Owner decision: no
- Changes: edge `cr0.1` depends on `fut.15` (Edges). No doc edit beyond OI-U01's ADR-009 line.

### OI-U03 Crates the UI batches asked for
- Raised by: B5-2 (`unicode-segmentation`), B5-22 (`async-channel`), B5-30 (`image`), E19-4 (thread CPU time), B7-E21-9 (`wgpu`)
- Question: five UI beads need a crate that is not in the tl-ui row of ADR-001.
- Answer: see engine side (OI-E04 crate rows, OI-E06 thread CPU time, OI-E32 for `unicode-segmentation`, `image` and `wgpu`). OI-E04 puts `async-channel` in the tl-ui row, so `UiInit.app_requests` stays an `async_channel::Receiver` (OI-U01); E15-003 (ug9.6) stays blocked until `unicode-segmentation` is decided; E16-005 (fut.8) stops and reports if it needs `image`; `wgpu` is settled on the UI side by `AppEnv::list_adapters` (OI-U01), so tl-ui never needs it; `FrameStats` reports 0 thread CPU until `tl_model::time::thread_cpu_ns()` exists (E19-005, yrf.7).
- Basis: ADR-001 [verified]; B5-2 notes the crate is already in Cargo.lock through gpui-base [verified by the writer].
- Owner decision: no
- Changes: none here; the engine file carries the row edits.

---

## B. Action names and where they live

### OI-U04 `RefreshCurrentTab` is the F5 action
- Raised by: B7-E20-1
- Question: UI-SHL-120 names F5 `RefreshNow`, which is also `Command::RefreshNow` in tl-model. E20-002, E27 and E28 use `RefreshCurrentTab`.
- Answer: `RefreshCurrentTab`, declared in E20-002 (jqk.4) `window/actions.rs`, bound to F5 in the Global context. UI-SYS-085 already uses this name.
- Basis: INDEX.md action vocabulary lists `RefreshCurrentTab` [verified]; UI-SHL-120 is the odd one out [verified].
- Owner decision: no
- Changes: shell.md UI-SHL-120: `RefreshNow` to `RefreshCurrentTab`. No bead change.

### OI-U05 `ShowSystemInformation` is the Help action
- Raised by: B7-E21-3, B8-E28-6
- Question: shell.md UI-SHL-041 says `OpenSystemInformation`; INDEX.md and UI-SYS-096 say `ShowSystemInformation`.
- Answer: `ShowSystemInformation`, declared in E20-002 (jqk.4), dispatched by the Help menu (E21), handled by E28-A3 (9o0.3) by switching to the Information tab.
- Basis: INDEX.md action vocabulary [verified]; UI-SYS-096 says shell.md owns it [verified].
- Owner decision: no
- Changes: shell.md UI-SHL-041: rename the action. No bead change.

### OI-U06 `WindowKey::Main` and the caption on `ToggleAlwaysOnTop`
- Raised by: B7-E20-2, B7-E20-3, B5-27
- Question: the pin action carries a `WindowKey`, but there is no key for the main window, and the KWin keep-above command needs the window caption, which the action does not carry.
- Answer: E16-008 (fut.12) adds `Main` to `WindowKey`. `Main` is never stored in `WindowRegistry` and has no cap class (`cap_class()` returns `None`). The action becomes `ToggleAlwaysOnTop { window: WindowKey, caption: SharedString }` with `#[action(no_json)]` (B5-27, `WindowKey` holds tl-model types without `JsonSchema`). The caption is the exact window title at dispatch time; the main window passes `"tasklens"`. E20-022 (jqk.24) handles `Main` only. One new E20 bead handles secondary windows: it reads the key, finds the caption in the action, sends `KWinCommand::KeepAbove { caption, on }` through the engine, and keeps the pin state in memory per key (R-073). Where no KWin bridge exists the pin is disabled with tooltip "Not supported by this desktop" (R-073, OI-U07).
- Basis: UI-FND-021 pin dispatch, UI-SHL-005, R-073 [verified]; the caption field [inferred from PERF-083, which finds windows by pid and caption].
- Owner decision: no
- Changes: foundation.md UI-FND-021 and UI-PWN-001: action gains `caption`. Bead fut.12: add `Main`, add `caption`. New bead, plan key E20-029: "ToggleAlwaysOnTop for secondary windows through the KWin bridge", epic E20, phase P2, files `window/main.rs` (handler arm only) and `components/window_frame.rs` (pin button passes its caption), depends on jqk.24, fut.12, y6k.2.

### OI-U07 No X11 keep-above in v1
- Raised by: B7-E20-4, E26-OI-5
- Question: APP-009, UI-SHL-005 and PERF-093 name an X11 `_NET_WM_STATE_ABOVE` path; tl-model has no command for it and gpui-pre 0.3.7 exposes no such call.
- Answer: dropped for v1. On X11 the pin, View > Window > Always On Top and the mini window's `ToggleMiniAlwaysOnTop` behave as "elsewhere": disabled with tooltip "Not supported by this desktop". KDE (through the bridge) is the only platform with keep-above.
- Basis: ADR-004 and ADR-010 "no X11 EWMH fallback for window features in v1", R-046, R-076 [verified]; ADR precedes the merged spec, so APP-009's X11 line yields.
- Owner decision: no
- Changes: sys-app.md APP-009, shell.md UI-SHL-005 and UI-SHL-036, performance.md UI-PRF-045 (mini window), RECONCILIATION.md R-073: delete the X11 clause, keep "KDE via bridge, else disabled". No bead change (E20-022 and E26 mini-window beads are already KWin only).

### OI-U08 `QuitMode::Explicit` exists in gpui-pre 0.3.7
- Raised by: B7-E20-5
- Question: ADR-010 requires the app to survive its last window closing. Does the pinned gpui offer that?
- Answer: yes. `gpui-pre-0.3.7/src/app.rs:389` defines `pub enum QuitMode { Default, LastWindowClosed, Explicit }`; `Application::with_quit_mode(self, mode)` (line 227) and `App::set_quit_mode(&mut self, mode)` (line 1754) set it; the window-close path (line 2922) quits on empty only for `LastWindowClosed`, or for `Default` off macOS. E16-012 (fut.15) calls `.with_quit_mode(QuitMode::Explicit)` on the `Application` before `run`; E20-A1 (jqk.1) needs no fallback and drops its "report if the app quits" clause. The UI calls `cx.quit()` itself for `UiExit::Quit` (File > Exit, close rows that quit, last detached window closed).
- Basis: gpui source on disk [verified]; `docs/research/claude-gpui-apis.md:58` [verified].
- Owner decision: no
- Changes: bead jqk.1: delete the fallback clause. Bead fut.15: add the `with_quit_mode` line.

### OI-U09 Actions declared once, outside the page that uses them
- Raised by: B5-26, B5-28, B8-E22-15, E26 note on `ResetMinigraphLayout`
- Question: several actions are named by more than one epic. Who declares each?
- Answer: one declaration each; everyone else imports.
  | Action | Declared by | File |
  |---|---|---|
  | `StartHelperSession`, `EndHelperSession` | E16-A3 (fut.3) | `components/shield.rs`; E20 `actions.rs` re-exports |
  | `CloseSecondaryWindow` | E16-008 (fut.12) | `components/window_frame.rs`, Ctrl+W in secondary windows; independent of E20's `CloseWindow` |
  | `ToggleHideSystem`, `CycleViewMode`, `SetViewMode`, `FocusFilter`, `ClearFilter`, `StartFilter` routing | E20-002 (jqk.4) | `window/actions.rs`; E22 builds the buttons only |
  | `ResetMinigraphLayout` | E26-002 (xdh.4) | `pages/minigraph/actions.rs`; E21-003 (bj6.6) dispatches it |
  | Table actions (`MoveFocus`, `CopyCell`, `ResetColumns`, `SetColumnResize`, ...) | E18-A1 (rsf.1) | `table/actions.rs` |
  | `EnableAllColumns { table: TableId }` | E18 (OI-U10) | `table/actions.rs` |
  All live in the one `tasklens` action namespace (R-128).
- Basis: INDEX.md action vocabulary and owners, R-128 [verified]; UI-PRC-009 and UI-PRC-019 place the shell actions in `pages/processes/actions.rs`, which INDEX.md overrides [verified].
- Owner decision: no
- Changes: processes.md UI-PRC-009 Build line and UI-PRC-019: say the shell actions are imported from `window/actions.rs`. No bead change.

### OI-U10 `EnableAllColumns` belongs to the table
- Raised by: B7-E21-8
- Question: the Debug menu item (UI-SHL-042) changes column visibility, which is E18 state.
- Answer: E18-A2 (rsf.2) declares `EnableAllColumns { table: TableId }` in `table/actions.rs` and handles it in `TlTreeTable` by setting every column of the owner's column set visible and persisting through the normal `[columns.<table>]` path. E21's Debug menu dispatches it with the active tab's `TableId`; on a tab without a table the item is disabled with tooltip "This tab has no table" (same text as UI-SHL-033).
- Basis: UI-SHL-042 names the action [verified]; placement [inferred from UI-SHL-034, where `ResetColumns { table }` follows the same pattern].
- Owner decision: no
- Changes: bead rsf.2: add the action and handler plus test `enable_all_columns_shows_every_column`. Bead for the Debug menu (E21, UI-SHL-042): dispatch, do not declare.

---

## C. Tree table and the Processes page

### OI-U11 The column picker file lives in `table/`
- Raised by: B5-3
- Question: 00-epics §2 puts `pages/processes/columns_window.rs` under E18, but that directory is created by E22-A1, which depends on E18-A2: a cycle.
- Answer: `crates/tl-ui/src/table/columns_window.rs`, owned by E18-A2 (rsf.2). The picker is table-generic (UI-PRC-007 binds it to a `TableId`), so nothing in it is Processes-specific.
- Basis: 00-epics §2 ownership table [verified]; R-106 (`WindowKey::Columns`, one window bound to the active table) [verified].
- Owner decision: no
- Changes: 00-epics §2: move the file into the E18 `src/table/...` row. processes.md UI-PRC-007 Build line: new path. Bead rsf.2 already uses the new path (B5 says so); no bead change.

### OI-U12 Scrollbar width is 16
- Raised by: B5-14
- Question: processes.md "Shared geometry" and the UI-PRC-001 diagram say 12; UI-PRC-001 anatomy, UI-PRC-010 and UI-FND-003 say 16.
- Answer: 16 logical px at 100% scale, for every scrollbar. UI-FND-003 is the component authority.
- Basis: UI-FND-003 and UI-PRC-010 anatomy "kit Scrollbar 16 px" [verified].
- Owner decision: no
- Changes: processes.md "Shared geometry" and the UI-PRC-001 diagram: 12 to 16. No bead change.

### OI-U13 Column picker: heading rows 28, filter md 28
- Raised by: B5-32, B5-33
- Question: UI-PRC-007 gives group headings 24 px and the filter as both "sm" and "md, height 28"; `uniform_list` needs one row height.
- Answer: every picker row is 28 px (group headings included, heading text bottom-aligned); the filter is `TlTextInput` md, 28 px. Rows stay 28 per UI-PRC-007.
- Basis: UI-PRC-007 anatomy "md, height 28" [verified]; `uniform_list` fixed row height [verified, gpui `elements/uniform_list.rs`]; heading height [inferred].
- Owner decision: no
- Changes: processes.md UI-PRC-007: headings 28, filter md only. No bead change.

### OI-U14 Delegate trait additions and header-menu actions
- Raised by: B5-16, B5-34
- Question: E18-005 adds `table_version()` and `guide_lines()` to the UI-PRC-001 delegate; header menu items (Resize Behavior, Reset to default, Hide This Column) and `OpenProperties` need a handler.
- Answer: the `TreeTableDelegate` trait gains `fn table_version(&self) -> u64` and `fn guide_lines(&self) -> bool` (E18-005, rsf.6). `TlTreeTable` handles `ResetColumns { table }` itself when the owner called `set_default_layout`, handles `SetColumnResize` (writes `tables.column_resize` through `TlSettings`), `HideColumn { col }`, `CopyCell { col }`, `CopyRow`, `CollapseAll` and `ResizeColumnsToFit`, and routes `OpenProperties` to `delegate.on_activate()`. The owner pages (E22, E27, E28) implement `on_activate`, `context_menu` and the copy formats; they do not re-handle the table actions. UI-SHL-034 dispatches with the active tab's `TableId`.
- Basis: UI-PRC-001 delegate, UI-PRC-006 header menu, UI-SHL-034 [verified]; the split follows B5-16's text [verified].
- Owner decision: no
- Changes: processes.md UI-PRC-001: add the two trait methods. Beads pdu.1, hk6.1, 9o0.1: say "the table handles the header-menu actions; implement `on_activate` and `context_menu`". No new bead.

### OI-U15 Icons the foundation adds or drops
- Raised by: B5-35, B5-37, B5-17
- Question: E18-009 needs Lucide `arrow-up` and `arrow-down`, which UI-FND-003 does not list; seven Lucide icons are outside the kit bundle; `shield-locked.svg` is in the E15-A1 list but UI-FND-028 uses Lucide `shield`.
- Answer: UI-FND-003's Lucide list gains `arrow-up` and `arrow-down` (sort indicators, UI-PRC-005). E15-005 (ug9.8) embeds `x`, `pin`, `pin-off`, `lock`, `shield`, `shield-check`, `rotate-ccw-clock`, `arrow-up`, `arrow-down` with the kit's `icon_assets!` macro. `assets/icons/shield-locked.svg` is not created; the locked shield is Lucide `shield` (UI-FND-028).
- Basis: UI-FND-003 icon list, UI-FND-028, UI-PRC-005 [verified].
- Owner decision: no
- Changes: foundation.md UI-FND-003: add the two arrows, note the nine embedded icons. Bead ug9.1 (E15-A1): drop `shield-locked.svg` from its asset list. Bead ug9.8: add the two arrows.

### OI-U16 `TlTreeTable::scroll_handle` for the bench
- Raised by: B8-E22-19, leader-notes B8 report
- Question: the E01 `table-2000` scene (E01-008, wlf.10) must scroll the production process table.
- Answer: E18-A1 (rsf.1) exposes `pub fn scroll_handle(&self) -> UniformListScrollHandle` (a clone; the handle is `Rc`-backed). E22-A1 (pdu.1) exposes `ProcessesPage::scroll_handle(&self, cx: &App) -> UniformListScrollHandle` next to `table()`, with test `scroll_handle_drives_table`. E01-008 calls the page method and drives it with `scroll_to_item_with_offset`.
- Basis: gpui `UniformListScrollHandle(pub Rc<RefCell<..>>)` [verified]; UI-PRC-010 Build line (`scroll: UniformListScrollHandle`) [verified].
- Owner decision: no
- Changes: bead rsf.1: add the accessor. Edge `wlf.10` depends on `pdu.1` (Edges).

### OI-U17 First visible row across a scale change: one E18 bead
- Raised by: B7-E20-7
- Question: UI-SHL-130 step 5 and test `scale_keeps_first_row` belong to `tree_table.rs`, and E18 has no bead for them.
- Answer: new E18 bead. `TlTreeTable` gains `pub fn first_visible_row(&self) -> Option<(RowId, usize)>` (the anchor UI-PRC-010 already records) and `pub fn restore_first_visible_row(&mut self, anchor, cx)`, which calls `scroll_to_item_with_offset(ix, ScrollStrategy::Top, 0)` after the new metrics are applied. E20-013 (jqk.15) calls them around the metrics re-apply, for the active table only. Test `scale_keeps_first_row` lives in the new bead.
- Basis: UI-SHL-130 step 5 and UI-PRC-010 anchor [verified].
- Owner decision: no
- Changes: new bead, plan key E18-017: "Keep the first visible row across a UI scale change", epic E18, phase P1, file `table/tree_table.rs`, depends on rsf.1. Edge `jqk.15` depends on the new bead.

### OI-U18 One filter debounce, in the shell
- Raised by: B7-E20-6
- Question: the shell debounces 100 ms before `FilterChanged` (UI-SHL-024) and E22-024 debounces another 100 ms.
- Answer: E22-024 (pdu.27) drops its own debounce and sends `Command::SetTableQuery` on every `FilterChanged`. UI-PRC-016's "100 ms debounce" is the shell's.
- Basis: UI-SHL-024, UI-SYS-002 ("already debounced 100 ms by the shell") [verified].
- Owner decision: no
- Changes: processes.md UI-PRC-016 Interaction and Build: the debounce and its `Task` are the shell's. Bead pdu.27: remove the debounce and rename test `filter_debounce_100ms_new_generation` to `filter_changed_sends_new_generation`.

### OI-U19 `SHIPPED_PHASE` and End Process on app rows before P2
- Raised by: B8-E22-4, B8-E22-17
- Question: how does code decide "a phase has not shipped", and what does Shift+Delete do on an app or unit row in P1?
- Answer: `pub const SHIPPED_PHASE: u8 = 1;` in `pages/processes/delegate.rs`, raised by E22-042 to E22-044 as each phase's columns land; columns and menu items with a higher phase are not built. Menu items appear when the bead that adds them lands, so they need no constant. In P1 `EndProcess` on an app or unit row is a no-op (no toast); E22-035 (pdu.38) turns it into Kill App.
- Basis: UI-PRC-013 "items whose phase has not shipped are not built" [verified]; the mechanism [inferred].
- Owner decision: no
- Changes: none; both beads already carry this.

### OI-U20 Name cell geometry: gap 6 after the chevron and after the icon
- Raised by: B8-E22-2
- Question: UI-PRC-002's app-row and unit-row sketches differ by 4 px and the UI-PRC-003 test formula omits the icon gap.
- Answer: one rule for every row kind: indent = depth × 20; chevron 16 wide starting at indent + 6; gap 6; icon 16 (unit) or 24 (app); gap 6; text. For an app row at depth 0 that gives chevron 6..22, icon 28..52, text 58 before the row's left padding of 20 (so 26..42, 48..72, 78 as the app sketch says). The unit sketch and the test formula are corrected to the same gaps.
- Basis: UI-PRC-002 app sketch [verified]; the generalisation [inferred].
- Owner decision: no
- Changes: processes.md UI-PRC-002 unit sketch (icon 48..64, text 70) and UI-PRC-003 formula. Bead pdu.9 already uses gap 6; no change.

### OI-U21 Picker group of each process column
- Raised by: B8-E22-3
- Question: PROC-024 gives 13 headings and the Objects members only.
- Answer: E22-002 (pdu.6)'s mapping: each PROC column section goes under its own heading; Start time, Type and the package and file columns under Image; User, PID, Parent PID and Session under Identity. The mapping is a table in the bead and is the source for `[columns.processes]` defaults.
- Basis: PROC-024 [verified]; the mapping [guess, writer-level].
- Owner decision: no
- Changes: processes.md UI-PRC-007: add the mapping table from pdu.6 so the doc, not the bead, is the reference.

### OI-U22 Subtree for End Process Tree and the bwrap first descendant
- Raised by: B8-E22-8
- Question: UI-PRC-013 says the subtree comes from "the engine forest", which is not in the snapshot.
- Answer: the UI walks `ProcessSnapshot.records` by `ppid`, matching parents by `pid` among non-exited records, breadth first; the result is capped at 256 keys (the `ActionRequest` limit). For a bwrap row the "first descendant" is the first non-bwrap record in that walk, ties by lowest PID. See engine side if `ProcessRecord` gains a children index; until then this walk stands.
- Basis: ADR-005 256 targets [verified]; the walk [inferred].
- Owner decision: no
- Changes: processes.md UI-PRC-013 Interaction: replace "from the engine forest" with the walk. No bead change.

### OI-U23 Flat `TlTreeTable` instead of `TlStaticTable`
- Raised by: E25-OI-3
- Question: UI-PRF-030 names a sortable `TlStaticTable`; foundation.md has no such component.
- Answer: no new component. The SMART attribute table is a `TlTreeTable` with a flat delegate (`RowKind::Plain`, no expansion, no selection actions) and E18-A2 header sorting done in the delegate (the rows are a small in-memory `Vec`, sorted by the delegate on header click). Same for any other static table in the Performance tab.
- Basis: UI-PRF-030 [verified]; UI-SYS-002 shows the same flat-delegate pattern [verified].
- Owner decision: no
- Changes: performance.md UI-PRF-030: `TlStaticTable` to "`TlTreeTable` with a flat delegate". No bead change (E25-018, o55.19, already does this).

### OI-U24 Toolbar slots: E20 defines the trait, pages draw the buttons
- Raised by: B8-E22-11
- Question: UI-SHL-010 draws slots 2 and 3 inside `TitleBarView` from a `ToolbarModel`; 00-epics gives E22 `toolbar.rs` and a `ToolbarContent` registration.
- Answer: E20-003 (jqk.5) defines `ToolbarContent` and `ToolbarModel` and draws only the slot frame; each page epic registers one `ToolbarContent` that draws its own buttons (E22-022, pdu.25, draws the Processes buttons). E20 draws no page button. The chain-edit rule of 00-epics §2 (one `ToolbarContent` registration per page epic) stands.
- Basis: 00-epics §2 chain-edit rule [verified]; UI-SHL-010 [verified].
- Owner decision: no
- Changes: shell.md UI-SHL-010: "slots 2 and 3 are filled by the active page's `ToolbarContent`". No bead change.

---

## D. Toasts, action tracking and texts

### OI-U25 Toast top is 72 in every window for P1
- Raised by: B5-8
- Question: the kit `NotificationSettings` is app-global, so the main window top 72 and the secondary-window top 42 (UI-FND-020 rule 9) cannot both hold.
- Answer: 72 × `ui_scale` in every window for P1 (66 with server decorations, as UI-SHL-100 says, also app-wide). Rule 9 is kept as the target for P2: a kit change (per-window `NotificationSettings`) or `ToastQueue::new(window, cx, top)` taking the per-window top once E16-006's own queue no longer goes through the kit global. No bead now.
- Basis: UI-FND-020 rule 9, UI-SHL-100 [verified]; R-109 and R-113 precedent (kit limits are accepted and recorded) [verified].
- Owner decision: no
- Changes: foundation.md UI-FND-020 rule 9 and shell.md UI-SHL-100: "P1: 72 in every window; secondary windows get 42 when the queue takes a per-window top (P2)". Bead fut.9 (E16-006): `ToastQueue::new` keeps the `top` parameter but every caller passes the shell's value in P1.

### OI-U26 `message(&ActionError)`, reject texts and deadlines
- Raised by: B5-9, B5-23, B8-E22-6
- Question: the docs give only the `PermissionDenied` text; the tracker also needs reject texts for five `RejectReason`s, deadlines for four action kinds, and the short reason used inside partial-failure summaries.
- Answer: in `tl-ui/src/action_tracker.rs`:
  | `ActionError` | `message()` |
  |---|---|
  | `PermissionDenied` | Permission denied. This process belongs to another user. |
  | `NoSuchProcess` | The process has already exited. |
  | `Partial { done, total }` | Done for {done} of {total} targets. |
  | `UnitJob { result, error_name }` | The service job ended with result: {result}. (the error name feeds E27's D-Bus mapping, not this text) |
  | `Os(code)` | The system call failed (error {code}). |
  | `Internal(text)` | Internal error: {text}. |

  | `RejectReason` | toast text (Warning) |
  |---|---|
  | `Busy` | Another action on this process is still running. |
  | `TooManyTargets` | Too many processes selected. The limit is 256. |
  | `IdentityChanged` | The selected process changed; wait for the list to update and try again. |
  | `KernelThread` | Kernel threads cannot be signalled. |
  | `Protected` | This process is protected. Confirm the warning to end it. |
  | `NotOwner` | This process belongs to another user. Administrator access is needed. |
  | `Unsupported` | This action is not available for this process. |
  | `AlreadyRunning` (OI-E10, startup entries) | Already running (kind Info, not Warning) |

  Deadlines (`deadline_for`): 2 s per target for signal, nice, affinity, ioprio and freeze writes; 120 s for every call that may show a polkit prompt, which includes `SockDestroy`, `UnitWriteRecovery` and `UnitRemoveRecovery`; `Dump` has no deadline (`None`), because a core of several GiB is a normal case and the progress toast "Writing core… 1.2 GiB" is its liveness signal. `short_reason(&err)` for summaries ("Ended 4 of 5 processes (1 permission denied)") is the first sentence of `message()` lower-cased without its full stop; `PermissionDenied` gives "permission denied" as today.
- Basis: ADR-006 deadline table, ADR-015 `ActionError`, PROC-058 texts [verified]; `IdentityChanged` text is PROC-058's [verified]; the other texts [guess]; `Dump` deadline [inferred from PROC-075].
- Owner decision: no
- Changes: ARCHITECTURE.md ADR-015: add the `message()` table. RECONCILIATION.md R-121 accepted strings: append the seven reject texts. Bead fut.3: replace "proposed" with these values; add `short_reason`. Bead pdu.20 (E22-017) uses `short_reason`.

### OI-U27 `ActionTracker` contract: per-request texts, summary threshold, Details dialog, Progress hook, restricted refusals
- Raised by: B5-24, B7-E23-8, B8-E27-3 (UI half), B8-E27-4, B7-E20-8
- Question: five batches need the same tracker API: per-request texts, a summary threshold that differs between process and system actions, a Details dialog for partial summaries, a Progress toast without the auto-dismiss cap, result events by request id, and a caller for `StatusChip::note_restricted_refusal`.
- Answer: E16-A3 (fut.3) as written, with these fixed points:
  - `ActionTracker::track(&mut self, request, queue: WeakEntity<ToastQueue>, texts: ToastTexts, cx) -> Result<ActionId, CommandError>` returns the id. `ToastTexts` gains `pub toast_key: Option<ToastKey>` so grouped requests (affinity on several targets, the SetSched then SetNice pair) share one toast that replaces in place.
  - `ToastTexts.summary_min_targets` defaults to 4 (R-114); process actions pass 2 (UI-PRC-015 "Ended 4 of 5 processes"). A `ToastTexts` whose `started` is `None` pushes no Progress toast (UI-SYS-045 and UI-SYS-046 want none for startup and connection actions); E27's `SysActions` passes `None` when `pending_one` is empty.
  - Progress toasts (`ToastKind::Progress`) never auto-dismiss and do not count against the 4-visible cap (E16-006, fut.9).
  - The tracker emits `ActionTrackerEvent::{Started(ActionId), Finished { id, ok, total, failed }}` through `cx.emit`, so `SysActions` (E27-006, hk6.9) and the dialogs (E23) subscribe by id instead of reading toasts.
  - "Details" on a partial summary opens `TlDialog` titled "tasklens", non-destructive, one line per failed target "<target label>: <message(err)>", one button "Close" (B8-E27-4; same dialog fut.3 already names).
  - On `Failed(PermissionDenied)` when `HealthSnapshot.helper == HelperState::Missing`, the tracker calls `StatusChip::note_restricted_refusal(cx)` through the `ShellState` global if present (E20-016, jqk.18); in tests without a shell it is a no-op.
  - The D-Bus error name inside `ActionError::UnitJob` is engine side (E11); the UI maps whatever string arrives.
- Basis: fut.3 Deliverable [verified by `br show`]; R-114, UI-PRC-015, UI-SYS-007, UI-SYS-045, UI-SYS-046 [verified]; event enum and `toast_key` [inferred].
- Owner decision: no
- Changes: bead fut.3: return `ActionId`, add `toast_key`, the event enum, the `note_restricted_refusal` call, the no-Progress rule. Bead jqk.18: name fut.3 as the caller. Beads hk6.9, gma.5 to gma.7 (E23-002 to 004): subscribe to `ActionTrackerEvent`. system-tabs.md UI-SYS-007: describe the Details dialog.

### OI-U28 Confirmation precedence when several rules match
- Raised by: B8-E22-7
- Question: UI-PRC-014 gives only "protected wins over tree".
- Answer: Protected, then End tree, then Kill app, then several targets, then another user's process. The winning row's title and buttons are used; the count N is still shown where the winning text has one.
- Basis: UI-PRC-014 "protected wins" [verified]; the rest of the order [inferred, most destructive first].
- Owner decision: no
- Changes: processes.md UI-PRC-014: add the order sentence. No bead change (pdu.20 carries it).

### OI-U29 Copy details rounding and clipboard failure
- Raised by: B8-E22-9, B8-E27-2
- Question: rounding and glyph words in the TSV, and whether a clipboard failure can be reported.
- Answer: whole MiB rounded to nearest; TSV writes the words `restricted`, `n/a` and an empty cell for the other glyphs (R-050). `App::write_to_clipboard(&self, item: ClipboardItem)` returns `()` in gpui-pre 0.3.7 (`app.rs:1549`), so no failure can be detected. One wrapper `tl_ui::util::clipboard_write(cx, text) -> Result<(), ()>` returns `Ok(())` today; every copy path shows the success toast through it, the "Failed to copy to clipboard" and "Could not copy" texts stay in Fluent and unused, and no read-back check is added.
- Basis: gpui source [verified]; R-050 [verified]; APP-044 lists the failure text [verified].
- Owner decision: no
- Changes: processes.md UI-PRC-015 and system-tabs.md: footnote that the copy-failure toasts are unreachable in v1. Bead pdu.2 (E22-A2) hosts the wrapper (it is the first copier); hk6.8 and hk6.14 (E27-005, 016) call it.

### OI-U30 Fluent key prefix and slug rule
- Raised by: B5-31, B8-E22-18
- Question: R-132 fixes `<area>-<entry>-<slug>` but not the area prefix of the foundation nor how the slug is formed.
- Answer: areas: `fnd` (foundation), `shl` (shell), `prc` (processes), `pwn` (process windows), `prf` (performance), `sys` (system tabs), `app` (tl-app strings passed through). The slug is the kebab-case of the first five words of the English text, letters and digits only, for example `prc-015-process-terminated`. A key that collides inside one entry gets a numeric suffix `-2`.
- Basis: R-132 [verified]; the rule [guess, writer-level, already used by every bead].
- Owner decision: no
- Changes: RECONCILIATION.md R-132: add the area list and the slug rule. No bead change.

### OI-U31 Small texts no doc gives
- Raised by: B5-20, B5-25, B7-E21-10
- Question: stepper button tooltips, the shield's starting-state tooltip, the TaskSlinger import failure toast.
- Answer: stepper minus "Decrease", plus "Increase" (UI-FND-014). Shield starting state tooltip "Waiting for authentication…" (the accepted string, R-121). TaskSlinger import failure: Error toast "Could not import TaskSlinger settings" (success stays APP-034's "Imported N settings and M themes").
- Basis: R-121 accepted strings [verified]; the two others [guess, writer-level].
- Owner decision: no
- Changes: foundation.md UI-FND-014 and UI-FND-028, shell.md UI-SHL-040 (import row): add the texts. R-121: append "Decrease", "Increase", "Could not import TaskSlinger settings". No bead change.

### OI-U32 "Grant access" without the ellipsis
- Raised by: B7-E23-7
- Question: R-121 lists "Grant access…"; UI-PWN-018 and the PROC texts use "Grant access".
- Answer: "Grant access". The button opens no further dialog of its own (the polkit prompt is the system's), so the UI-FND-050 ellipsis rule does not apply.
- Basis: UI-PWN-018 [verified]; R-050 and the ellipsis convention [verified].
- Owner decision: no
- Changes: RECONCILIATION.md R-121: replace "Grant access…" with "Grant access". No bead change.

### OI-U33 Startup and connection action toasts
- Raised by: B8-E28-10, B8-E27-6
- Question: no busy, unknown or summary strings exist for the startup, connection and the Reload, Reset failed and startup-type service actions.
- Answer: R-114's pattern extended, exact strings:
  - Busy: "A startup action is already in progress.", "A connection action is already in progress." (the Services one is R-114's).
  - Unknown: "Outcome unknown" (PROC-058) for all.
  - Services: "Reloading { $n } services...", "{ $n } services reloaded", "{ $ok } of { $n } services reloaded. { $failed } failed."; "Resetting { $n } services...", "{ $n } services reset", "{ $ok } of { $n } services reset. { $failed } failed."; startup type: "Changing startup type of { $n } services...", "Startup type changed for { $n } services", "Startup type changed for { $ok } of { $n } services. { $failed } failed.".
  - Startup Apps: "{ $n } startup entries disabled", "{ $n } startup entries enabled", "{ $n } startup entries launched", "{ $n } startup entries deleted", and "{ $ok } of { $n } startup entries disabled. { $failed } failed." forms.
  - Connections: "{ $n } TCP connections closed" and "{ $ok } of { $n } TCP connections closed. { $failed } failed.".
  - No Progress toast for startup and connection actions (UI-SYS-045, UI-SYS-046): `ToastTexts.started = None` (OI-U27).
- Basis: R-114 pattern, UI-SYS-007, PROC-058 [verified]; the new strings [inferred from the pattern].
- Owner decision: no
- Changes: RECONCILIATION.md R-114: add the three service forms and the two system-table families. Beads hk6.15, hk6.16, 9o0.7, 9o0.15 already carry them; no change.

### OI-U34 Status text "| N selected" and the copy block for kernel sockets
- Raised by: B8-E28-8, B8-E28-14
- Question: when the selected-count suffix appears on Startup Apps and Connections; what the copy block says for kernel-owned and unresolved sockets.
- Answer: " | N selected" from 2 selected rows, as the Processes text (APP-029) does. Copy block: kernel-owned socket "PID: -" and "Process: No owner (kernel)"; unresolved (restricted) socket "PID: ?" and "Process: Restricted".
- Basis: APP-029, SYS-027, SYS-036, UI-SYS-049, UI-SYS-066 [verified]; the kernel and restricted lines [inferred from R-050's words].
- Owner decision: no
- Changes: system-tabs.md UI-SYS-049, UI-SYS-067 (status) and UI-SYS-066 (copy): add the rule and lines. No bead change.

### OI-U35 Session type wording
- Raised by: B8-E28-5
- Question: UI-SYS-088 shows "user (graphical)" but logind Type is x11, wayland, mir, tty or unspecified.
- Answer: "<Class> (graphical)" for x11, wayland and mir; "<Class> (text)" for tty; "<Class> (<Type>)" otherwise. Formatted in the UI from the two raw strings; see engine side for the fields (`InventorySnapshot`).
- Basis: UI-SYS-088 [verified]; mapping [inferred].
- Owner decision: no
- Changes: system-tabs.md UI-SYS-088: add the mapping. No bead change.

### OI-U36 Jump target texts for callers without their own
- Raised by: B8-E22-12
- Question: `--select-pid`, the D-Bus `select-pid` action and the window picker reach the jump target without a "not visible" text.
- Answer: `JumpTexts::default()` in E22-A3 (pdu.3): not found "Process not found in current view" (PROC-071); not visible "Process not found in current view" with a "Show" button that clears the filter, turns Hide system processes off and selects the row; switched "Switched to Processes tab". Callers with their own texts (Go to parent, Services, Connections, Handle Search) pass theirs.
- Basis: PROC-071 [verified]; reuse of the same text for the hidden case [inferred].
- Owner decision: no
- Changes: processes.md UI-PRC-022: list the default texts. No bead change.

### OI-U37 Which block device the I/O priority "Scheduler:" line names
- Raised by: B8-E22-13
- Question: UI-PRC-020 names "the active block scheduler" without saying which device.
- Answer: the device holding the root filesystem, as a capability string in the snapshot. See engine side for the field (the UI does not read `/sys`).
- Basis: UI-PRC-020 [verified]; choice [inferred].
- Owner decision: no
- Changes: processes.md UI-PRC-020: "the scheduler of the device holding /". No bead change.

### OI-U38 Combo filter at 12 items or more
- Raised by: B5-21
- Question: UI-FND-017 says the filter appears "above 12 items" and in the same sentence gives the 12-item language list one.
- Answer: 12 or more items show the filter.
- Basis: UI-FND-017 [verified, self-contradictory; the example wins].
- Owner decision: no
- Changes: foundation.md UI-FND-017: "12 or more items". No bead change.

---

## E. Themes and foundation leftovers

### OI-U39 Theme store needs: the animation requester, the E19 edge, new error and event variants
- Raised by: B5-18, B5-13, B5-19
- Question: the theme crossfade (E17-006) and preview coalescing (E17-A2) need the `AnimationDriver` to accept a requester that is not a view entity; E17-006 therefore depends on E19-A1, which the E17 row of 00-epics lacks; `ThemeError` and `ThemeEvent` gain variants the E21 toast mapping must cover.
- Answer: `AnimationDriver` (E19-005, yrf.7) accepts any `EntityId` in `request(id)` and does not require the id to be a view; it also gains `request_callback(id: EntityId, f: impl Fn(&mut Window, &mut App) + 'static)`. The theme store reserves one `EntityId` from a zero-sized entity it creates at init (UI-FND-040 already says "reserved EntityId"). E17-006 (lgw.8) depends on E19-A1 (yrf.1). `ThemeError` gains `BuiltinCannotBeDeleted` and `BuiltinCannotBeEdited`; `ThemeEvent` gains `Imported { count: u32 }`. E21 toast mapping: "Built-in themes cannot be deleted" (Warning), "Built-in themes cannot be edited. Copy it first." (Warning), "Imported { $count } themes" (Success).
- Basis: UI-FND-040 crossfade and reserved id [verified]; UI-FND-041 to 043 [verified]; texts [guess, writer-level].
- Owner decision: no
- Changes: 00-epics §4: add E19-A1 to the E17 row. Edge `lgw.8` depends on `yrf.1` (Edges). foundation.md UI-FND-040: list the two error and one event variant. Bead yrf.7: add `request_callback` and the "any EntityId" note. Bead for the theme menu toasts (E21, UI-SHL-038): add the three texts.

### OI-U40 themes.json from TaskSlinger (U-18) blocks E17-005
- Raised by: B5-10
- Question: the 10 named built-in palettes are exported from TaskSlinger under Wine by the owner (U-18); E17-005 (lgw.7) cannot start without the file.
- Answer: blocked until the owner supplies `assets/themes/builtin.json`. Recommended: the owner runs the export once and commits the file; E17-005 then parses it at build time. Alternative: E17-005 ships the two default themes only (Default Light, Default Dark) and the other eight land in a follow-up bead when the file arrives, which unblocks P0 and P1.
- Basis: RECONCILIATION U-18 [verified].
- Owner decision: yes (owner step; recommended: ship the two defaults now, follow-up bead for the eight)
- Changes: bead lgw.7: split into "two defaults from hand-written values" (P0) and a new bead E17-015 "Eight built-in palettes from the TaskSlinger export", P2, file `theme/builtin.rs`, depends on lgw.7, blocked-open-item until the file exists.

### OI-U41 One TS1 share code (U-17) blocks E17-010
- Raised by: B5-11
- Question: the TS1 share-code encoding is unknown until the owner generates one code in TaskSlinger (U-17).
- Answer: blocked. Recommended: the owner generates one code from a known theme and records it in RECONCILIATION U-17; E17-010 (lgw.12) decodes it. Alternative: ship import of the tasklens format (`TL1`, documented in UI-FND-043) only and leave TS1 for P3.
- Basis: U-17 [verified].
- Owner decision: yes (owner step; recommended: TL1 now, TS1 when the code exists)
- Changes: bead lgw.12: `blocked-open-item` label, note the TL1-only path; the E21 share-code beads (bj6.32 to bj6.34, E21-029 to 031) depend on lgw.11 as leader-notes say.

### OI-U42 `assets/icons/tasklens.svg` artwork
- Raised by: B5-12
- Question: no document specifies the application icon.
- Answer: owner choice. Recommended: a placeholder icon for P0 and P1 (a 256 px rounded square in the accent colour with a white monospace "tl"), so E15-022 (ug9.25) is not blocked, and a follow-up bead replaces the file when the owner supplies artwork. Alternative: keep E15-022 blocked.
- Basis: none in the docs [verified absent].
- Owner decision: yes (recommended: placeholder now)
- Changes: bead ug9.25: build the placeholder, drop `blocked-open-item`. New bead E15-026 "Replace the placeholder application icon", P3, files `assets/icons/tasklens.svg`, depends on ug9.25, blocked-open-item until artwork exists.

### OI-U43 Recording U-nn outcomes
- Raised by: B5-15
- Question: where a resolved U-nn item is recorded.
- Answer: append the outcome to the item's U row in RECONCILIATION.md (the "U-1 to U-50" table), one line "Outcome (date): ...". No other place.
- Basis: RECONCILIATION U table [verified].
- Owner decision: no
- Changes: none.

### OI-U44 Priority of spec-P1 components inside P0 anchors
- Raised by: B5-1
- Question: E15-A1 and E15-A2 contain spec-P1 and P2 components; E18-A1 and A2 (P0) need E16-006, 007, 008, A2 and 009.
- Answer: accept. Those E16 beads carry priority 0. Shield stays in E16-A3 (P1) and the preview contract in E17-A2 (P2).
- Basis: 00-epics phase rules [verified].
- Owner decision: no
- Changes: none (already set in the beads).

### OI-U45 RTL golden needs a Hebrew font on CI
- Raised by: B5-36
- Question: E18-013 (rsf.15) renders a Hebrew golden; CI may lack the font.
- Answer: accept the bead's rule: the case is skipped with a printed reason when `font_id("Noto Sans Hebrew")` fails. Engine side (E02 CI image) may add the font later.
- Basis: UI-FND-050 bundled fonts [verified].
- Owner decision: no
- Changes: none.

---

## F. Shell, windows and settings

### OI-U46 Double-click on a process row expands, it does not open Properties
- Raised by: B7-E23-1
- Question: UI-PRC-002 says double-click runs `OpenProperties`; UI-PWN-003 says double-click is not bound and expands, like TaskSlinger.
- Answer: double-click toggles expansion on a row with children and does nothing on a leaf. Properties open with Enter, Alt+Enter, Ctrl+I, the menu item and the delegate's `on_activate` for the system tables (UI-SYS-002 lists double-click there, which stays: those rows have no children).
- Basis: D-008 lists Enter, Ctrl+I and Alt+Enter only [verified]; UI-PWN-003 and the source app [verified]; UI-SYS-002 [verified].
- Owner decision: no
- Changes: processes.md UI-PRC-002 and UI-PRC-008: double-click toggles expansion. Bead pdu.1 (E22-A1) gesture binding: `ToggleExpand`.

### OI-U47 Health items: sources per tab and display names
- Raised by: B7-E20-9, B7-E21-6
- Question: UI-SHL-051 says "sources the active tab uses" without a list; UI-SHL-050 names 12 sources but `SourceId` has more.
- Answer: per tab: Processes: `Processes` plus every source a visible minigraph draws; Performance: `Cpu`, `Memory`, `Disks`, `Network`, `Gpu`, `Sensors` plus `Battery` when a battery page exists; Services: both systemd managers and `Journal`; Startup Apps: `Startup`; Connections: `Connections`; Information: `Inventory`. Display names: CPU, Memory, Disks, Network, GPU, Sensors, Processes, Services, Startup, Connections, Inventory, KWin, Logind, UDisks, UPower, NetworkManager, Public IP, History, Journal, Battery; any other variant shows its Rust name.
- Basis: UI-SHL-050, UI-SHL-051 [verified]; the mapping [inferred].
- Owner decision: no
- Changes: shell.md UI-SHL-050 and UI-SHL-051: add both lists. No bead change (jqk.18 carries the mapping).

### OI-U48 `WindowRegistry` key queries and the keep-detached close row
- Raised by: B7-E20-10, E26-OI-3, E26-OI-7
- Question: E20-028 must know whether any `DetachedGraph(_)` window is open and be told when one closes; E26-018 provides `any_detached_open`; `DetachedGraph(u8)` caps ids at 255.
- Answer: E16-A2 (fut.2) adds `pub fn has_key(&self, pred: impl Fn(&WindowKey) -> bool) -> bool` and `pub fn count(&self, pred) -> usize` to `WindowRegistry`, and calls `cx.notify()` on every insert and removal. E20-028 (jqk.30) uses `has_key(|k| matches!(k, WindowKey::DetachedGraph(_)))` directly and observes the registry; `any_detached_open` in E26-018 (xdh.20) becomes a one-line wrapper, and that bead un-ignores `keep_detached_on_close_keeps_app`, so xdh.20 depends on jqk.30. `DetachedGraph(u8)` is accepted: state.toml `[[detached_windows]].id` is a `u8` too.
- Basis: UI-SHL-113 row 2, PERF-084, APP-045 [verified]; jqk.30 Deliverable [verified by `br show`].
- Owner decision: no
- Changes: bead fut.2: add the two accessors and notify. Bead jqk.30: use `has_key`. Bead xdh.20: wrapper plus test. Edge `xdh.20` depends on `jqk.30` (Edges).

### OI-U49 Start hidden and `ToggleMainWindow` before the tray exists
- Raised by: B7-E20-12, B7-E20-13, E24-8
- Question: `--start-minimized` can hide only when a tray exists; `tray_available` is false until E31; the tray click action is defined only with a tray.
- Answer: accept both E20 rules. `tray_available(cx)` returns `TrayModel.available` and is `false` until E31-009 fills it (E24-023, crw.25, stubs the same `Some(false)`). Without a tray `--start-minimized` opens the window and minimizes it (APP-048) and a close that would quit minimizes instead when `ToggleMainWindow` arrives while visible.
- Basis: APP-048, UI-SHL-113 [verified].
- Owner decision: no
- Changes: shell.md UI-SHL-110 step 6: add the no-tray rule. No bead change.

### OI-U50 Keyboard Shortcuts window headings
- Raised by: B7-E21-4
- Question: UI-SHL-095 lists seven headings; table (b) has more contexts.
- Answer: headings in order: Global, Tables, Processes, History, Services, Startup Apps, Connections, Information, Performance, Properties windows (PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog), Dialogs (RunDialog, Picker, ColumnPicker, HandleSearch, JournalList, LowMemory). A heading with no shipped bindings is not drawn.
- Basis: table (b) contexts [verified]; grouping [inferred].
- Owner decision: no
- Changes: shell.md UI-SHL-095: the eleven headings. No bead change.

### OI-U51 Fans group: partial entries and empty state
- Raised by: B7-E21-7
- Question: UI-SHL-066 says an empty label plus "None" removes the entry but not what a half-set entry stores or what the group shows with no fans.
- Answer: store `[[sensors.fans]]` entries with `key`, optional `label` (omitted when empty) and optional `temp` (omitted for None); an entry with neither is removed. With no fan in the snapshot the group is hidden. Keys follow PERF-005 (`fan:<hwmon name>:<device path>:<n>`; see OI-U59 for the fallback form).
- Basis: UI-SHL-066, table (a) [verified]; storage shape [inferred].
- Owner decision: no
- Changes: shell.md UI-SHL-066 and table (a) `[[sensors.fans]]`: optional fields. No bead change (bj6.41).

### OI-U52 Passwordless setup step
- Raised by: B7-E21-11
- Question: the `SetupStep` variant for `polkit-rule on|off` and the result stream belong to E03 and E29.
- Answer: see engine side (OI-E11: `SetupStep::PolkitRule { on }`, results in `HealthSnapshot.setup`). UI half: E21-042 (bj6.46) dispatches `Command::RunSetup { step }` and reads the result from `HealthSnapshot.setup` on the `HEALTH` bit; it already depends on qfx.2 and qfx.3 (verified with `br show`).
- Basis: UI-PRF-052 `Command::RunSetup`, `SetupEvent` [verified].
- Owner decision: no
- Changes: none.

---

## G. Process windows (E23)

### OI-U53 Set Affinity is P1 inside a P2 anchor
- Raised by: B7-E23-2
- Answer: accept. E23-003 (gma.6) and E23-001, 002, 004 (gma.4, gma.5, gma.7) are P1 and schedulable before E23-A2 (gma.2) closes; the anchor stays P2.
- Basis: PROC-067, UI-PWN-021 P1 [verified].
- Owner decision: no
- Changes: none.

### OI-U54 Dump file name sanitising
- Raised by: B7-E23-4
- Question: `{name}` in `<name>_<pid>_<yyyymmdd_hhmmss>.core` can contain `/` and control characters.
- Answer: replace `/`, `\0` and every control character with `_`, cut the name part to 64 bytes on a char boundary, and use `process` when the result is empty. The timestamp is local time from `AppEnv::now_rfc3339` reformatted (OI-U01). The write itself is engine side (gcore, helper).
- Basis: PROC-075, UI-PWN-034 [verified]; rule [inferred].
- Owner decision: no
- Changes: process-windows.md UI-PWN-034: add the rule. No bead change (gma.36).

### OI-U55 "Shared libraries only" keeps the Primary and "Outside loaded modules" nodes
- Raised by: B7-E23-5
- Answer: with the box on, the Primary node (the executable's own mapping) and the "Outside loaded modules" node stay visible; only file-backed library mappings are filtered by the PROC-081 patterns.
- Basis: PROC-081 [verified]; choice [inferred].
- Owner decision: no
- Changes: process-windows.md Memory maps entry: one sentence. No bead change (gma.23).

### OI-U56 After "Grant access" succeeds
- Raised by: B7-E23-6
- Question: UI-PWN-018 says the window re-sends its demand "with the helper flag", which `DetailDemand` lacks.
- Answer: no flag. When `HealthSnapshot.helper` changes to `Unlocked`, every open Properties window whose General payload is `Restricted` sends `Command::DetailRefresh { target, tab }` once for its active tab. Routing Restricted targets through the helper is engine side (OI-E32 confirms: no `DetailDemand` flag, the UI resends `DetailRefresh`).
- Basis: UI-PWN-002 `DetailRefresh`, UI-FND-028 `HelperState` [verified].
- Owner decision: no
- Changes: process-windows.md UI-PWN-018: replace "with the helper flag" with the refresh rule. No bead change (gma.28).

### OI-U57 VirusTotal and dump handlers reach the Processes page by chain edit
- Raised by: B8-E22-14
- Answer: E22-038 and E22-039 (pdu.41, pdu.42) add the menu items and dispatch `OpenVirusTotal` and `CreateDump`; E23-032 (gma.35) and E23-034 (gma.37) register the handlers in `pages/processes/mod.rs` under the E23 row of the chain-edit rule. No new bead.
- Basis: 00-epics §2 `pages/processes/mod.rs` sharing rule names E23 [verified].
- Owner decision: no
- Changes: none; confirm gma.35 and gma.37 depend on pdu.41 and pdu.42 (Edges).

---

## H. Charts, Performance, minigraphs, system tables

### OI-U58 Chart validity and the E19 model constants
- Raised by: E19-5, E19-1, E19-2, E19-3, E19-4, E25-OI-2
- Question: the chart cannot tell Failed from Restricted from Unsupported by itself; the pages need `MetricId` constants, the `TIER_1HZ` bit, `ceil_pow2` and `ceil_125`, and thread CPU time that tl-model does not yet define.
- Answer: E19-5 accepted: the owner page calls `ChartView::set_state` from the `SourceHealth` of the source it knows; the chart derives only WarmingUp and Stale. The four model items are settled in OI-E32: the `MetricId` numbers 5, 6, 7, 14, 47 to 49, 52 to 58 and 61; `TIER_1HZ` is bit 15 with `with_tier_1hz()` and `is_tier_1hz()`; `ceil_pow2` and `ceil_125` land in `tl_model::units` through E03-012. UI consequences that hold whatever the engine answer: E24 and E25 beads name series by their UI-PRF names and stop and report when a constant is missing; E19-013 (yrf.15) waits for the `TIER_1HZ` bit; E19-004 (yrf.6) defines nothing privately and stops and reports if `tl_model::units::{ceil_pow2, ceil_125}` are absent; E19-005 reports 0 thread CPU until `thread_cpu_ns` exists (OI-U03).
- Basis: UI-PRF-001, UI-PRF-006, ADR-014 [verified].
- Owner decision: no
- Changes: none here.

### OI-U59 Performance frame items
- Raised by: E24-6, E24-7, E24-10, E24-9
- Answer: E24-6 accepted: where `DeviceIdentity` has no by-path or device path, the UI key is `disk:<name>` and `fan:<chip>:<index>`; PERF-005 gets those as its fallback forms. E24-7 accepted: `DevicePage::set_active(&mut self, active: bool, cx)` is on the trait from E24-006 (crw.8); E25-010 (o55.11) adds it only if missing. E24-10 accepted: `CpuPage.bench_show_both` is a public field in crw.11; the paint hook is `AnimationDriver.paint_log` behind `cfg(any(test, feature = "test-hooks"))`, so E01-001 (wlf.3) declares `bench = ["test-hooks"]` in `tl-ui/Cargo.toml`. E24-9 (fan `connected` flag on the record) is see engine side.
- Basis: PERF-005, UI-PRF-053, leader-notes E01 lines [verified].
- Owner decision: no
- Changes: performance.md PERF-005 (merged spec) and UI-PRF-011: add the fallback key forms. Bead crw.8: `set_active` on the trait. Bead wlf.3: the feature line.

### OI-U60 Device page items
- Raised by: E25-OI-4, E25-OI-5, E25-OI-7, E25-OI-8, E25-OI-1, E25-OI-6
- Answer: E25-OI-4: edge `o55.22` depends on `bj6.20` (`request_public_ip_fetch` lives in E21-017 and shows the notice window first; E28-036, 9o0.32, calls the same helper). E25-OI-5 accepted: PWM is pre-scaled into the RPM axis through a derived `SeriesView` kept by the page; if E19 gains a per-series `YScale` the derived series goes. E25-OI-7 accepted: the UPower `Type` and `State` word mapping lives in `pages/perf/battery.rs` (E25-027, o55.28). E25-OI-8: `GridStyle.vertical_at: Option<Arc<[f32]>>` and `ChartSeries.gap_style: GapStyle::{Break, Dashed}` are added to E19-001's types by E25-032 (o55.33) as a same-batch chain edit; no E19 follow-up bead. E25-OI-1 (SMART, eject, public IP commands and records) and E25-OI-6 (`LaunchWhat::DbusActivate`) are see engine side; the five E25 beads that need them stop and report.
- Basis: UI-PRF-034, PERF-060, UI-SYS-095 [verified].
- Owner decision: no
- Changes: edge `o55.22` depends on `bj6.20` (Edges). Bead yrf.3 (E19-001): a note that E25-032 extends `GridStyle` and `ChartSeries`.

### OI-U61 Minigraph items
- Raised by: E26-OI-4, E26-OI-6, E26-OI-1, E26-OI-2
- Answer: E26-OI-4: edge `xdh.12` depends on `bj6.35` and E26-010 calls the shared Graph Color submenu builder. E26-OI-6 accepted: card reorder is pointer-only; Reset Minigraph Layout is the keyboard path. E26-OI-1 (aggregate devices) and E26-OI-2 (per-process series) are see engine side; E26-003 (xdh.5) and E26-011 (xdh.13) stop and report if undecided.
- Basis: UI-PRF-040 to 044 [verified].
- Owner decision: no
- Changes: edge `xdh.12` depends on `bj6.35` (Edges).

### OI-U62 Services items
- Raised by: E27-1, E27-8, E27-9, E27-7, E27-5
- Answer: E27-1 accepted: on `CommandError::Busy`, `SysQuery.last_sent` still becomes the new generation (older views are refused), `pending_resend = true`, and the next snapshot resends once with a fresh generation; on `EngineStopped` nothing is resent. E27-8: `ToastKey` values at or above `0x8000_0000_0000_0000` are fixed keys reserved by E16-006 (fut.9); the tracker's generated keys (`id << 8 | index`) never reach that range because `ActionId` stays below 2^55; `SERVICES_LOADING_TOAST = ToastKey(0x8000_0000_0000_0101)`; the E28 pages take `..0201` (Startup) and `..0301` (Connections). E27-9 accepted: `hk6.23` depends on `gma.1` (exists) and E23-A1 joins the E27 row of 00-epics §4. E27-7: the UI sends `UnitEnable` or `UnitDisable` when leaving Masked; the unmask inside that request is see engine side. E27-5 (Services `ColumnId` constants) is see engine side.
- Basis: UI-SYS-002, UI-SYS-007, UI-FND-020 `ToastKey` [verified]; the reserved range [inferred].
- Owner decision: no
- Changes: 00-epics §4: E23-A1 in the E27 row. foundation.md UI-FND-020: the fixed-key range. Bead fut.9: document the range.

### OI-U63 Startup Apps, Connections and Information items
- Raised by: E28-7, E28-15, E28-1, E28-2, E28-3, E28-4, E28-9, E28-11, E28-12, E28-13
- Answer: E28-7: the Information skeleton follows UI-SYS-085 (every section shows its band and an 8-line skeleton block); shell.md's "4 section blocks 120 tall" is corrected. E28-15 accepted: E27-A2, E29-A2, E29-A3, E25-A1 and E03-A2 join the E28 row of 00-epics §4. The other eight (public IP command and `SourceId`, `InventorySnapshot` fields, `committed_peak`, the 64-bit rule, `ActionError::AlreadyRunning`, system-table `ColumnId`s, startup entry fields, connection display fields) are see engine side; the E28 beads read "the E03 field of that meaning" and stop and report when it is missing. UI half of E28-9: OI-E10 makes it `RejectReason::AlreadyRunning`, not an `ActionError`; the tracker maps it to the Info toast "Already running" (OI-U26 table).
- Basis: UI-SYS-085, UI-SHL-111, SYS-023 [verified].
- Owner decision: no
- Changes: shell.md UI-SHL-111: skeleton per UI-SYS-085. 00-epics §4: the five E28 row additions.

### OI-U64 E22 epic-row additions and the build-rule note
- Raised by: B8-E22-5, B8-E22-16
- Answer: E19-A1 and E01-A1 join the E22 row of 00-epics §4 (edges exist). B8-E22-16 needs no answer: the leader is rewriting the build lines in every bead.
- Basis: 00-epics §4 [verified].
- Owner decision: no
- Changes: 00-epics §4: E22 row.

### OI-U65 Topics settled on the engine side
- Raised by: the items below
- Answer: see engine side (`open-items-engine.md`) for every one of these. Listed here so no raiser is lost. IDs confirmed by oi-engine on 2026-10-04 (OI-E32 landed in engine commit 6ff1ce2).
  | Raiser | Topic | Engine ID |
  |---|---|---|
  | E25-OI-1, B8-E28-1, B7-E20-11 | `Command::Device(DeviceCommand)` keyed by kernel disk name; public IP in `InventorySnapshot.public_ip`; E20-024 sends the fetch command at startup | OI-E01 |
  | B8-E22-1 | `ProcessRecord`, `TableRow`, `GroupRow`, `TableTotals`, `ProcessTableView` fields | OI-E03 |
  | B5-22, E19-4 | `async-channel` in tl-ui and tl-app; `tl_model::time::thread_cpu_ns()` through rustix | OI-E04, OI-E06 |
  | B5-2, B5-30, B7-E21-9 | `unicode-segmentation = "1.12"` added to tl-ui; `image` not added (gpui-kit image path first); no `wgpu`, tl-app probes through GPUI | OI-E04 items 15 to 17 |
  | B5-38 | E33 `UiLaunch` (OI-U02 has the UI half) | OI-E15 |
  | B5-6 | `SystemSnapshot.color_scheme` and its portal reader | OI-E32 |
  | B5-7 | `HealthSnapshot.helper` exists in as2.13 and gains `helper_start` | OI-E10 |
  | B7-E21-5 | `tl-app/build.rs` and the licence file (OI-U01 names the fields) | OI-E32 |
  | B7-E23-3 | `write_atomic` for run history (OI-U01 names the callbacks) | OI-E08 |
  | E19-1, E19-2, E19-3, E25-OI-2 | `MetricId` constants, `TIER_1HZ` bit, `ceil_pow2`, `ceil_125` | OI-E32 |
  | E24-9 | fan `connected` flag | OI-E32 |
  | E25-OI-6 | `LaunchWhat::DbusActivate` | OI-E32 |
  | E26-OI-1, E26-OI-2 | aggregate devices, per-process series | OI-E16 |
  | B7-E21-11 | `SetupStep::PolkitRule { on }`, results in `HealthSnapshot.setup` | OI-E11 |
  | B7-E23-6 (engine half) | `DetailRefresh` routed through the helper | OI-E32 |
  | B8-E22-10 | KWin pick result in `WindowsSnapshot.pick`; command result | OI-E11, OI-E32 |
  | B8-E27-3 (engine half), B8-E27-7 | `ActionError::UnitJob { result, error_name }`, unmask inside `UnitEnable` and `UnitDisable` | OI-E32 |
  | B8-E27-5, B8-E28-11 | `SysColumnId` in `table.rs`, separate from the process `ColumnId` | OI-E32 |
  | B8-E28-2, B8-E28-12, B8-E28-13 | `InventorySnapshot`, startup and connection record fields (E03-055, E03-056) | OI-E18 |
  | B8-E28-3 | `committed_peak` (reducer running max) | OI-E28 |
  | B8-E28-4 | 64-bit rule | OI-E32 |
  | B8-E28-9 | `RejectReason::AlreadyRunning` | OI-E10 |
  | B7-E20-4 (engine half) | no X11 keep-above command in v1 | OI-E32 |
  | udev, gcore, helper hook, PSI, unsafe | not raised by a UI item; listed for completeness | OI-E02, OI-E07, OI-E09, OI-E13, OI-E05 |
- Owner decision: no
- Changes: none here.

---

## Edges

Lines for the leader; each is new unless marked "exists" (checked with `br dep list` on 2026-10-04).

```
br dep add tasklens-cr0.1  tasklens-fut.15   # E33-A1 calls tl_ui::run(UiLaunch) (OI-U02)
br dep add tasklens-o55.22 tasklens-bj6.20   # E25-021 calls request_public_ip_fetch from E21-017 (OI-U60)
br dep add tasklens-9o0.32 tasklens-bj6.20   # E28-036 calls the same helper (OI-U60)
br dep add tasklens-xdh.12 tasklens-bj6.35   # E26-010 uses the shared Graph Color submenu (OI-U61)
br dep add tasklens-xdh.20 tasklens-jqk.30   # E26-018 wraps has_key and un-ignores the keep-detached test (OI-U48)
br dep add tasklens-jqk.15 <E18-017>         # scale change calls restore_first_visible_row (OI-U17)
br dep add <E18-017>       tasklens-rsf.1    # new E18 bead builds on the tree table (OI-U17)
br dep add <E20-029>       tasklens-jqk.24   # secondary-window keep-above extends the Main handler (OI-U06)
br dep add <E20-029>       tasklens-fut.12   # needs WindowKey::Main and the caption field (OI-U06)
br dep add <E20-029>       tasklens-y6k.2    # KWin bridge sends KeepAbove (OI-U06)
br dep add <E17-015>       tasklens-lgw.7    # eight palettes after the two defaults (OI-U40)
br dep add <E15-026>       tasklens-ug9.25   # final icon replaces the placeholder (OI-U42)
br dep add tasklens-gma.35 tasklens-pdu.41   # optional: handler after the menu item (OI-U57); anchor edge to pdu.2 exists
br dep add tasklens-gma.37 tasklens-pdu.42   # optional, same (OI-U57)
# exists: tasklens-lgw.8 -> tasklens-yrf.1 (OI-U39); tasklens-wlf.10 -> tasklens-pdu.1 (OI-U16);
#         tasklens-hk6.23 -> tasklens-gma.1 (OI-U62); tasklens-bj6.46 -> tasklens-qfx.2, qfx.3 (OI-U52)
```

New beads named above: E18-017 (OI-U17), E20-029 (OI-U06), E17-015 (OI-U40), E15-026 (OI-U42).

## Summary

| OI | Title | Owner decision | Beads unblocked or changed |
|---|---|---|---|
| OI-U01 | `UiInit` and `AppEnv` | no | fut.1, bj6.3, bj6.8, bj6.14, bj6.17, bj6.42, bj6.44, bj6.45, gma.31 |
| OI-U02 | `tl_ui::run(UiLaunch)` | no | fut.15, cr0.1 |
| OI-U03 | Crates the UI asked for | no | engine side; ug9.6, fut.8 wait |
| OI-U04 | `RefreshCurrentTab` | no | none |
| OI-U05 | `ShowSystemInformation` | no | none |
| OI-U06 | `WindowKey::Main`, caption on pin | no | fut.12, new E20-029 |
| OI-U07 | No X11 keep-above in v1 | no | none |
| OI-U08 | `QuitMode::Explicit` verified | no | jqk.1, fut.15 |
| OI-U09 | Action declaration homes | no | none |
| OI-U10 | `EnableAllColumns` in E18 | no | rsf.2 |
| OI-U11 | Picker file in `table/` | no | none |
| OI-U12 | Scrollbar 16 | no | none |
| OI-U13 | Picker rows 28, filter md | no | none |
| OI-U14 | Delegate methods, header actions | no | pdu.1, hk6.1, 9o0.1 |
| OI-U15 | Icons added and dropped | no | ug9.1, ug9.8 |
| OI-U16 | `scroll_handle` for the bench | no | rsf.1, wlf.10 |
| OI-U17 | First row across scale change | no | new E18-017, jqk.15 |
| OI-U18 | One filter debounce | no | pdu.27 |
| OI-U19 | `SHIPPED_PHASE` | no | none |
| OI-U20 | Name cell gaps | no | none |
| OI-U21 | Picker groups | no | none |
| OI-U22 | Subtree walk | no | none |
| OI-U23 | Flat table, no `TlStaticTable` | no | none |
| OI-U24 | Toolbar slot ownership | no | none |
| OI-U25 | Toast top 72 everywhere | no | fut.9 |
| OI-U26 | `message()`, reject texts, deadlines | no | fut.3, pdu.20 |
| OI-U27 | `ActionTracker` contract | no | fut.3, jqk.18, hk6.9, gma.5, gma.6, gma.7 |
| OI-U28 | Confirmation precedence | no | none |
| OI-U29 | Copy rounding, clipboard | no | pdu.2, hk6.8, hk6.14 |
| OI-U30 | Fluent prefixes and slugs | no | none |
| OI-U31 | Small texts | no | none |
| OI-U32 | "Grant access" | no | none |
| OI-U33 | System-table action toasts | no | none |
| OI-U34 | "N selected", kernel sockets | no | none |
| OI-U35 | Session type wording | no | none |
| OI-U36 | Jump default texts | no | none |
| OI-U37 | Scheduler line device | no | none |
| OI-U38 | Combo filter at 12 | no | none |
| OI-U39 | Theme store needs | no | yrf.7, E21 theme toasts |
| OI-U40 | themes.json (U-18) | yes | lgw.7 split, new E17-015 |
| OI-U41 | TS1 code (U-17) | yes | lgw.12 |
| OI-U42 | Application icon artwork | yes | ug9.25, new E15-026 |
| OI-U43 | Recording U-nn | no | none |
| OI-U44 | P0 pull-forward | no | none |
| OI-U45 | RTL golden font | no | none |
| OI-U46 | Double-click expands | no | pdu.1 |
| OI-U47 | Health items per tab | no | none |
| OI-U48 | Registry queries, close row 2 | no | fut.2, jqk.30, xdh.20 |
| OI-U49 | Start hidden without tray | no | none |
| OI-U50 | Shortcut headings | no | none |
| OI-U51 | Fans group storage | no | none |
| OI-U52 | Passwordless setup step | no | engine side |
| OI-U53 | Set Affinity P1 | no | none |
| OI-U54 | Dump name sanitising | no | none |
| OI-U55 | Shared libraries nodes | no | none |
| OI-U56 | After Grant access | no | none |
| OI-U57 | VirusTotal and dump handlers | no | none |
| OI-U58 | Chart validity, E19 constants | no | engine side |
| OI-U59 | Performance frame items | no | crw.8, wlf.3 |
| OI-U60 | Device page items | no | o55.22, 9o0.32, yrf.3 |
| OI-U61 | Minigraph items | no | xdh.12 |
| OI-U62 | Services items | no | fut.9 |
| OI-U63 | Startup, Connections, Information | no | engine side |
| OI-U64 | E22 row additions | no | none |
| OI-U65 | Engine-side pointer list | no | none |
