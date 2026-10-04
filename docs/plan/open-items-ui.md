# UI-side open items, settled

Settles the open items raised by the UI bead batches: `b5-ui-foundation.md` (E15 to E18, items 1
to 38), `b6-ui-performance.md` (E19 items 1 to 5, E24 items 6 to 10, E25-OI-1 to 8, E26-OI-1 to 7),
`b7-ui-shell.md` (E20 items 1 to 13, E21 items 1 to 11, E23 items 1 to 8) and `b8-ui-tables.md`
(E22 items 1 to 19, E27 items 1 to 9, E28 items 1 to 15), plus the cross-batch notes in
`leader-notes.md`. Rules and precedence: `docs/research/brief-openitems.md`.

Raiser IDs are written `B5-13` (batch 5, item 13), `B7-E21-4` (batch 7, E21 list, item 4),
`E25-OI-4` and `E26-OI-3` (the B6 lists keep their own IDs). Bead IDs are `tasklens-<epic>.<n>`;
the `tasklens-` prefix is dropped in the text and kept in the Edges section.

Engine-side topics are owned by `open-items-engine.md` (oi-engine). This file points to them as
"see engine side" and lists them once in OI-U65. Topics that have a UI half and an engine half
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
      pub run_history_load: Arc<dyn Fn() -> Vec<RunHistoryEntry>>,
      pub run_history_save: Arc<dyn Fn(&[RunHistoryEntry]) -> Result<(), String>>,
      pub now_rfc3339: Arc<dyn Fn() -> String>,          // local time, for executedAt
  }
  pub struct GlobalShortcutApi { pub get: Arc<dyn Fn() -> Option<String>>,
                                 pub set: Arc<dyn Fn(Option<&str>) -> Result<(), String>> }
  impl Global for AppEnv {}
  ```
  `init` step 8 installs `AppEnv` as a Global next to `BootSettings`. Every closure runs on the main thread; none is `Send`. `PersistTheme` is declared in `app.rs` by E16-A1 and moved to `theme/user_store.rs` with a re-export by E17-008 (lgw.10), as B5-29 says. Reset settings (UI-SHL-072) calls `write_settings_now` before replacing `TlSettings` and shows "Failed to reset settings" on `Err`, with nothing changed. `executedAt` comes from `now_rfc3339`, so tl-ui needs no date crate and `tl_model::units` gains nothing. The adapter probe runs in tl-app (`wgpu` stays out of tl-ui). A `FakeEngine` test gets `AppEnv::test()` with empty closures and `debug = false`.
- Basis: ADR-001 tl-ui dependency row [verified]; ADR-009 and ADR-011 (the contract is extended, not changed) [verified]; UI-SHL-072, UI-SHL-092, UI-SHL-093, UI-PWN-028, APP-053, PROC-095 for the field list [verified]; field types [inferred].
- Owner decision: no
- Changes: ARCHITECTURE.md ADR-009 "Entry point" and ADR-011: replace the `(cx, client, settings)` signature with `UiInit` and name `AppEnv` and `persist_themes`. Bead fut.1: add `env: AppEnv` and the types above to the Deliverable and the Global to step 8. Beads bj6.8, bj6.14, bj6.17, bj6.42, bj6.44, bj6.45 (E21-006, 011, 015, 038, 040, 041), bj6.3 (Reset) and gma.31 (E23-028): replace "needs an interface from tl-app" with the named `AppEnv` member. Engine side fills every member (E33-A1 cr0.1, E33-A2 cr0.2, see engine side).

### OI-U02 `tl_ui::run(UiLaunch) -> UiExit`
- Raised by: B5-38
- Question: tl-app cannot depend on gpui-kit, so who opens the application?
- Answer: see engine side (E33 UiLaunch). UI half: E16-012 (fut.15) builds `tl_ui::run(UiLaunch) -> UiExit` with `UiLaunch { init: UiInit, open_mode: OpenMode, start: StartOptions }` and `UiExit::{Quit, RendererFailed(String), DeviceLost}`, exactly as B5-38 states. `debug` and `on_quit` are not in `UiLaunch` (`debug` is `AppEnv.debug`).
- Basis: B5-38, leader-notes "E33 needs `tl_ui::run(UiLaunch)` fut.15" [verified].
- Owner decision: no
- Changes: edge `cr0.1` depends on `fut.15` (Edges). No doc edit beyond OI-U01's ADR-009 line.

### OI-U03 Crates the UI batches asked for
- Raised by: B5-2 (`unicode-segmentation`), B5-22 (`async-channel`), B5-30 (`image`), E19-4 (thread CPU time), B7-E21-9 (`wgpu`)
- Question: five UI beads need a crate that is not in the tl-ui row of ADR-001.
- Answer: see engine side (crate allow-list). UI facts the engine answer must keep: `UiInit.app_requests` is an `async_channel::Receiver` (OI-U01), so `async-channel` must be in the tl-ui row or `UiInit` changes to a `Box<dyn Fn() -> Option<AppRequest>>` poll closure; E15-003 (ug9.6) stays blocked until `unicode-segmentation` is decided; E16-005 (fut.8) stops and reports if it needs `image`; `wgpu` is settled on the UI side by `AppEnv::list_adapters` (OI-U01), so tl-ui never needs it; `FrameStats` reports 0 thread CPU until `tl_model::time::thread_cpu_ns()` exists (E19-005, yrf.7).
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
