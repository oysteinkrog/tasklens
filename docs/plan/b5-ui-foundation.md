# Batch B5 ui-foundation: bead plan for E15, E16, E17, E18

This file lists every bead of batch B5: the theme model (E17), the tokens and shared components
(E15), the app core with hub, demand, toasts, windows and menus (E16), and the generic tree table
(E18). The rules are in `docs/research/brief-beads.md` and `docs/plan/00-epics.md`. Epic and anchor
bead IDs come from `docs/plan/anchors.md`.

Conventions used in every bead of this batch:

- Titles start with the plan key, for example `E15-008 TlButton (U-41)`.
- Bead priority equals the phase number. A bead that a P0 anchor needs gets priority 0 and the
  label `phase-p0`, and its Context names the spec phase it comes from (see open item 1).
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, plus `ui` when it renders, `security`
  when it handles untrusted text, `perf-gate` when it carries an ADR-014 gate.
- An anchor bead is the last real work bead of its group. It depends on every other bead of the
  group, so other batches can depend on the anchor alone.
- Each bead adds its own `pub mod` line to `crates/tl-ui/src/lib.rs` or to the `mod.rs` it lives
  under. All those files belong to this batch, so these edits need no cross-batch bead.
- A bead that settles a U-nn item appends one sentence to that row of the "Unverified
  assumptions" table in `docs/RECONCILIATION.md`: "Verified by <bead id>: <result>." (open item 15).
- Cross-batch dependencies point only at anchors. Dependencies inside the batch point at the exact
  bead a bead compiles against, so P0 work does not wait for whole P1 groups.

Plan keys are listed per epic below. The ID column is filled once the beads exist.

## E17 ui-theme

| Key | Bead | Title | Phase | Prio | Files | Depends on |
|---|---|---|---|---|---|---|
| E17-001 | `tasklens-lgw.3` | TlTheme, GraphPalette, status colours and derived tints | P0 | 0 | `theme/mod.rs`, `theme/palette.rs`, `theme/derived.rs` | E02-A1 |
| E17-002 | `tasklens-lgw.4` | Built-in theme JSON and strict loader | P0 | 0 | `assets/themes/builtin.json`, `theme/builtin.rs` | E17-001 |
| E17-003 | `tasklens-lgw.5` | UiMetrics global | P0 | 0 | `theme/metrics.rs` | E17-001, E03-A2 |
| E17-004 | `tasklens-lgw.6` | Kit Theme mapping in map.rs (U-8) | P0 | 0 | `theme/map.rs` | E17-001, E17-003 |
| E17-A1 | `tasklens-lgw.1` | ThemeStore and instant theme switch | P0 | 0 | `theme/store.rs`, `tests/ui_theme.rs` | E17-002, E17-004 |
| E17-005 | `tasklens-lgw.7` | Ten named built-in themes (U-18) | P2 | 2 | `assets/themes/builtin.json`, `theme/builtin.rs` | E17-A1 |
| E17-006 | `tasklens-lgw.8` | Theme crossfade | P2 | 2 | `theme/store.rs` | E17-A1, E19-A1 |
| E17-007 | `tasklens-lgw.9` | Follow system light and dark | P2 | 2 | `theme/store.rs`, `app.rs` | E17-A1, E16-A1 |
| E17-008 | `tasklens-lgw.10` | User themes file: load, repair, copy, delete, save | P2 | 2 | `theme/user_store.rs`, `tests/ui_theme.rs` | E17-A1 |
| E17-A2 | `tasklens-lgw.2` | Theme editor live-preview contract | P2 (spec P3) | 2 | `theme/preview.rs` | E17-006, E17-007, E17-008 |
| E17-009 | `tasklens-lgw.11` | TL1 share codes and theme import | P3 | 3 | `theme/share_code.rs`, `theme/user_store.rs` | E17-A2 |
| E17-010 | `tasklens-lgw.12` | TaskSlinger TS1 share codes (U-17) | P3 | 3 | `theme/share_code.rs` | E17-009 |
| E17-011 | `tasklens-lgw.13` | Import themes from a TaskSlinger themes.json | P4 | 4 | `theme/import_ts.rs` | E17-008 |
| E17-012 | `tasklens-lgw.14` | Theme render goldens | P1 | 1 | `tests/render_theme.rs`, `tests/golden/theme_*.png` | E17-A1, E16-005 |

## E15 ui-foundation

| Key | Bead | Title | Phase | Prio | Files | Depends on |
|---|---|---|---|---|---|---|
| E15-001 | `tasklens-ug9.4` | Bundled fonts and start-up face check (U-7, U-40) | P0 | 0 | `assets/fonts/*`, `text/mod.rs`, `text/fonts.rs` | E17-A1 |
| E15-002 | `tasklens-ug9.5` | Numeric and label text styles, TextMetricsCache | P0 | 0 | `text/style.rs`, `text/metrics.rs` | E15-001 |
| E15-003 | `tasklens-ug9.6` | Middle ellipsis for paths | P0 | 0 | `text/ellipsis.rs` | E15-002 |
| E15-004 | `tasklens-ug9.7` | Untrusted text display rules | P0 | 0 | `text/untrusted.rs` | E15-002 |
| E15-005 | `tasklens-ug9.8` | Component module, TlFocusRing, icons and asset source (U-42) | P0 | 0 | `components/mod.rs`, `components/focus.rs`, `components/icons.rs`, `assets/icons/shield-unlocked.svg` | E17-A1 |
| E15-006 | `tasklens-ug9.9` | TlProgress and TlSpinner | P0 (spec P1, P2) | 0 | `components/progress.rs` | E15-005 |
| E15-007 | `tasklens-ug9.10` | TlTooltip | P0 | 0 | `components/tooltip.rs` | E15-002, E15-005 |
| E15-008 | `tasklens-ug9.11` | TlButton (U-41) | P0 | 0 | `components/button.rs` | E15-005, E15-006, E15-007 |
| E15-009 | `tasklens-ug9.12` | TlCheckbox | P0 | 0 | `components/checkbox.rs` | E15-005 |
| E15-010 | `tasklens-ug9.13` | TlSwitch | P0 (spec P2) | 0 | `components/switch.rs` | E15-005 |
| E15-011 | `tasklens-ug9.14` | TlTabs | P0 (spec P2) | 0 | `components/tabs.rs` | E15-002, E15-008 |
| E15-A1 | `tasklens-ug9.1` | TlTextInput, TlFilterBox and the base-control test suite | P0 | 0 | `components/input.rs`, `tests/ui_components.rs` | E15-001 to E15-011 |
| E15-012 | `tasklens-ug9.15` | TlValidityGlyph | P0 | 0 | `components/validity.rs` | E15-002, E15-005, E15-007, E03-A1 |
| E15-013 | `tasklens-ug9.16` | TlSkeletonRows and TlSkeletonBlock | P0 (spec P1) | 0 | `components/skeleton.rs` | E15-005 |
| E15-014 | `tasklens-ug9.17` | TlEmptyState | P0 | 0 | `components/empty.rs` | E15-008 |
| E15-015 | `tasklens-ug9.18` | TlLink | P0 (spec P2) | 0 | `components/link.rs` | E15-002, E15-005 |
| E15-016 | `tasklens-ug9.19` | TlKeyValueGrid and TlValue | P0 | 0 | `components/kv_grid.rs` | E15-008, E15-012, E15-013, E15-015 |
| E15-017 | `tasklens-ug9.20` | TlBadge | P0 (spec P2) | 0 | `components/badge.rs` | E15-002 |
| E15-018 | `tasklens-ug9.21` | TlBanner | P0 (spec P2) | 0 | `components/banner.rs` | E15-008 |
| E15-019 | `tasklens-ug9.22` | TlRadio and TlRadioGroup | P0 (spec P2) | 0 | `components/radio.rs` | E15-005 |
| E15-020 | `tasklens-ug9.23` | TlSlider | P0 (spec P2) | 0 | `components/slider.rs` | E15-005 |
| E15-021 | `tasklens-ug9.24` | TlStepper | P0 (spec P2) | 0 | `components/stepper.rs` | E15-A1 |
| E15-A2 | `tasklens-ug9.2` | TlSelect, TlCombo and the display-component test suite | P0 (spec P2) | 0 | `components/select.rs`, `tests/ui_components.rs` | E15-A1, E15-012 to E15-021 |
| E15-A3 | `tasklens-ug9.3` | Component gallery and headless render tests (U-4) | P1 | 1 | `components/gallery.rs`, `tests/render_gallery.rs`, `tests/golden/gallery_*.png` | E15-A2, E16-005 |
| E15-022 | `tasklens-ug9.25` | App logo tasklens.svg | P1 | 1 | `assets/icons/tasklens.svg`, `components/icons.rs` | E15-005 |

## E16 ui-app-core

| Key | Bead | Title | Phase | Prio | Files | Depends on |
|---|---|---|---|---|---|---|
| E16-001 | `tasklens-fut.4` | tl-ui Cargo features, testing module and FakeEngine | P0 | 0 | `Cargo.toml`, `testing/mod.rs`, `testing/fake_engine.rs` | E17-001, E03-A2 |
| E16-002 | `tasklens-fut.5` | Fluent string layer and English catalogue | P0 (spec P1) | 0 | `i18n.rs`, `assets/locales/en-US.ftl`, root `Cargo.toml` pins | E17-001, E02-A1, E03-A2 |
| E16-003 | `tasklens-fut.6` | SnapshotHub, ComponentMask install and hidden state | P0 | 0 | `hub.rs`, `tests/ui_hub.rs` | E16-001 |
| E16-004 | `tasklens-fut.7` | DemandAggregator | P0 | 0 | `demand.rs`, `tests/ui_demand.rs` | E16-003 |
| E16-A1 | `tasklens-fut.1` | TasklensUi::init and the module roots | P0 | 0 | `app.rs`, `window/mod.rs`, `pages/mod.rs`, `windows/mod.rs` | E16-002, E16-004, E15-A1, E15-A2, E17-A1 |
| E16-012 | `tasklens-fut.15` | tl_ui::run(UiLaunch) entry point for tl-app | P0 | 0 | `app.rs`, `lib.rs`, `tests/ui_hub.rs` | E16-A1 |
| E16-005 | `tasklens-fut.8` | Headless render helpers and golden compare | P1 | 1 | `testing/render.rs`, `Cargo.toml` | E16-001, E15-001 |
| E16-006 | `tasklens-fut.9` | ToastQueue and toast rendering | P0 (spec P1) | 0 | `components/toast.rs`, `tests/ui_toasts.rs` | E16-A1 |
| E16-008 | `tasklens-fut.12` | TlSecondaryWindow frame and pin | P0 (spec P1) | 0 | `components/window_frame.rs` | E16-A1, E16-006 |
| E16-007 | `tasklens-fut.10` | TlDialog confirmation windows | P0 (spec P1) | 0 | `components/dialog.rs`, `tests/ui_dialogs.rs` | E16-008 |
| E16-A2 | `tasklens-fut.2` | WindowRegistry and WindowKey | P0 (spec P1) | 0 | `window/registry.rs`, `tests/ui_dialogs.rs` | E16-007, E16-008 |
| E16-009 | `tasklens-fut.13` | TlMenu builder | P0 (spec P1) | 0 | `components/menu.rs` | E16-A1 |
| E16-010 | `tasklens-fut.14` | TlShield | P1 (spec P3) | 1 | `components/shield.rs` | E16-A1, E03-A2 |
| E16-A3 | `tasklens-fut.3` | ActionTracker and message(&ActionError) | P1 | 1 | `action_tracker.rs`, `tests/ui_actions.rs` | E16-006, E16-009, E16-010 |
| E16-011 | `tasklens-fut.11` | Gallery pages for the E16 components | P1 | 1 | `components/gallery.rs`, `tests/render_gallery.rs`, `tests/golden/gallery_*.png` | E15-A3, E16-A2, E16-A3 |

## E18 ui-tree-table

| Key | Bead | Title | Phase | Prio | Files | Depends on |
|---|---|---|---|---|---|---|
| E18-001 | `tasklens-rsf.3` | Selection by identity | P0 | 0 | `table/mod.rs`, `table/selection.rs`, `tests/ui_selection.rs` | E16-A1 |
| E18-002 | `tasklens-rsf.4` | CellTextCache | P0 | 0 | `text_cache.rs` | E16-A1, E03-A1 |
| E18-003 | `tasklens-rsf.5` | ColumnLayout, ColumnSpec, widths and column state by id | P0 | 0 | `table/columns.rs` | E16-A1, E15-002, E03-A2 |
| E18-004 | `tasklens-rsf.7` | Cell painters: text, path and numeric cells | P0 | 0 | `table/cells.rs` | E18-002, E18-003, E15-A2 |
| E18-005 | `tasklens-rsf.6` | Tree-table delegate trait and row types | P0 | 0 | `table/tree_table.rs` | E18-001, E18-003, E16-009 |
| E18-006 | `tasklens-rsf.8` | TlTreeTable body and one-line header | P0 | 0 | `table/tree_table.rs`, `table/header.rs` | E18-004, E18-005 |
| E18-007 | `tasklens-rsf.9` | Mouse and keyboard selection, table actions and keymap | P0 | 0 | `table/actions.rs`, `table/tree_table.rs` | E18-006 |
| E18-008 | `tasklens-rsf.10` | Scroll anchoring, keep-visible, jump and visible-range demand | P0 | 0 | `table/tree_table.rs` | E18-007, E16-004 |
| E18-A1 | `tasklens-rsf.1` | Context menu capture, Copy rows as table and the tree-table suite | P0 | 0 | `table/tree_table.rs`, `tests/ui_tree_table.rs` | E18-008, E16-009 |
| E18-009 | `tasklens-rsf.11` | Two-line header: totals, sort arrows, sort click | P0 | 0 | `table/header.rs` | E18-A1 |
| E18-010 | `tasklens-rsf.12` | Column resize and fit | P0 | 0 | `table/header.rs`, `table/columns.rs` | E18-009 |
| E18-011 | `tasklens-rsf.13` | Column reorder drag | P0 | 0 | `table/header.rs` | E18-010 |
| E18-012 | `tasklens-rsf.14` | Header context menu | P0 | 0 | `table/header.rs`, `table/actions.rs` | E18-011, E16-006 |
| E18-A2 | `tasklens-rsf.2` | Column picker window "tasklens Columns" | P0 | 0 | `table/columns_window.rs` (open item 3), `tests/ui_columns.rs` | E18-012, E16-A2, E15-A1 |
| E18-013 | `tasklens-rsf.15` | Table render goldens | P1 | 1 | `tests/render_table.rs`, `tests/golden/table_*.png` | E18-A1, E16-005 |
| E18-014 | `tasklens-rsf.16` | Column separators | P4 | 4 | `table/tree_table.rs`, `table/header.rs` | E18-A2 |


## Open items for the leader

1. P0 pull-forward: E15-A1 and E15-A2 contain spec-P1/P2 components; E18-A1/A2 (P0) need E16-006,
   E16-007, E16-008, E16-A2 and E16-009, so those are priority 0. Shield (spec P3) sits in E16-A3
   (P1); the preview contract (spec P3) sits in E17-A2 (P2).
2. `unicode-segmentation` is not in the tl-ui row of ADR-001 but UI-FND-052a needs grapheme
   boundaries. Recommend adding `unicode-segmentation = "1.12"` (already in Cargo.lock through
   gpui-base) to the workspace pins and the tl-ui row. E15-003 is blocked on it.
3. `pages/processes/columns_window.rs` cannot compile before E22-A1 creates
   `pages/processes/mod.rs`, and E22 depends on E18-A2: a cycle. Recommend the file lives at
   `crates/tl-ui/src/table/columns_window.rs` (E18).
4. `TasklensUi::init` signature: ADR-009 gives `(cx, client, settings)`; the UI also needs state,
   themes text, persist callbacks, the AppRequest receiver and queued start-up toasts. Proposed
   `UiInit` above; B1 (E33) must match.
5. Themes persistence: ADR-011 only has `Fn(&Settings, &State)`. UI-FND-042 needs
   `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}`. Proposed `persist_themes` callback in `UiInit`.
6. `system.color_scheme` (portal color-scheme 0/1/2) has no field in ADR-003 and no engine worker.
   E17-007 needs both. Recommend `SystemSnapshot.color_scheme: u8` (E03) fed by a portal reader in
   E14's session-bus code.
7. `HealthSnapshot.helper: HelperState { Missing, Locked, Starting, Unlocked }` (UI-FND-028) is not
   in ADR-003. E16-010 needs it from E03.
8. The kit `NotificationSettings` is app-global, so the main window top 72 and the secondary
   window top 42 (UI-FND-020 rule 9) cannot both hold. Recommend 72 in every window for P1.
9. `message(&ActionError)` text is given only for `PermissionDenied`. Proposed:
   NoSuchProcess "The process has already exited."; Partial "Done for {done} of {total} targets.";
   UnitJob "The service job ended with result: {result}."; Os "The system call failed (error {code}).";
   Internal "Internal error: {text}.".
10. U-18: the 10 named theme palettes come from TaskSlinger under Wine (owner step). E17-005 is
    blocked until the owner supplies the exported themes.json.
11. U-17: TS1 encoding unknown. E17-010 is blocked until the owner generates one TS1 code.
12. `assets/icons/tasklens.svg` artwork is not specified anywhere. E15-022 is blocked.
13. Epic-graph addition: E17-006 depends on E19-A1 (crossfade uses the AnimationDriver).
14. Scrollbar width: processes.md "Shared geometry" says 12, UI-PRC-001 anatomy and UI-FND-003 say
    16. Beads use 16.
15. U-nn outcomes are recorded by appending to the U row in docs/RECONCILIATION.md. Confirm.
16. Header menu "Resize Behavior" writes `tables.column_resize` and "Reset to default" needs the
    table's defaults; both are table actions. The table handles `ResetColumns` itself when the owner set defaults with
    `TlTreeTable::set_default_layout`, and `OpenProperties` calls the delegate's `on_activate`.
    B8 (E22, E27, E28) must handle `SetColumnResize`, `CopyRow`, `CopyCell` and `CollapseAll`.
17. `assets/icons/shield-locked.svg` is in the E15-A1 anchor list, but UI-FND-028 uses the Lucide
    `shield` for the locked state. It is not created.
18. E17-006 (crossfade) and E17-A2 (per-frame preview coalescing) need the E19-A1
    `AnimationDriver` to accept a requester that is not a view entity. Recommend B6 gives E19-A1 a
    `request_callback(id, Fn(&mut Window, &mut App))` or treats a reserved `EntityId` as one.
19. `ThemeError` gains `BuiltinCannotBeDeleted` and `BuiltinCannotBeEdited`, and `ThemeEvent` gains
    `Imported { count }`. The E21 toast mapping must cover them.
20. The stepper's minus and plus buttons need tooltips; no doc gives text. E15-021 uses "Decrease"
    and "Increase".
21. UI-FND-017 says the combo filter appears above 12 items but gives the 12-item language list one.
    E15-A2 shows it at 12 or more items.
22. `async-channel` is not in the tl-ui row of ADR-001, but `UiInit` and FakeEngine name it.
    Recommend adding it to the row.
23. ActionTracker needs texts the docs do not give: reject texts for `Protected`, `Busy`,
    `TooManyTargets`, `NotOwner` and `Unsupported`, and deadlines for `Dump`, `SockDestroy`,
    `UnitWriteRecovery` and `UnitRemoveRecovery` (proposed 120 s). E16-A3 carries proposals.
24. R-114 says a summary toast from 4 targets; UI-PRC-015 shows a summary for multi-select process
    actions. `ToastTexts.summary_min_targets` defaults to 4 and process actions pass 2.
25. TlShield has no tooltip text for the starting state. E16-010 reuses the accepted string
    "Waiting for authentication…".
26. `StartHelperSession` and `EndHelperSession` are declared in `components/shield.rs`; E20's
    `actions.rs` re-exports them and does not declare them again.
27. `ToggleAlwaysOnTop` uses `#[action(no_json)]` because `WindowKey` holds tl-model types without
    `JsonSchema`.
28. E16-008 adds a local `CloseSecondaryWindow` action for Ctrl+W in secondary windows, so it does
    not depend on E20's `CloseWindow`.
29. `PersistTheme` is declared in `app.rs` by E16-A1 (P0); E17-008 moves it into
    `theme/user_store.rs` and re-exports it.
30. E16-005 may need the `image` crate by name for PNG read and write; it is not in the tl-ui row.
    The bead stops and reports if so.
31. String keys added by the foundation use the area prefix `fnd-`, for example `fnd-menu-no-items`.
32. The column picker uses `uniform_list`, which needs one row height: group headings are 28 px
    with the text at the bottom instead of 24 px (UI-PRC-007).
33. UI-PRC-007 gives the picker filter as "md, height 28" and as "sm"; E18-A2 uses md, 28 px.
34. E18-005 adds `table_version()` and `guide_lines()` to the UI-PRC-001 delegate trait; both are
    needed for the row index, the text cache and the connector lines.
35. E18-009 needs Lucide `arrow-up` and `arrow-down`, which are not in the UI-FND-003 icon list.
36. The RTL golden in E18-013 needs a Hebrew-capable font on the CI image; the bead skips that case
    and says so if none is present.
37. Seven Lucide icons the foundation uses (`x`, `pin`, `pin-off`, `lock`, `shield`,
    `shield-check`, `rotate-ccw-clock`) are not in the kit's default bundle; E15-005 embeds them
    with the kit's `icon_assets!` macro.
38. tl-app cannot depend on gpui-kit, so E16-012 adds `tl_ui::run(UiLaunch) -> UiExit` (B1 E33-OI-1).
    `UiLaunch` holds `UiInit` plus `OpenMode` and `StartOptions`; `UiExit` is `Quit`,
    `RendererFailed(String)` or `DeviceLost`. A renderer failure is detected as a panic before the
    first window opens; device loss is reported by E33-012 through `request_exit`. B1 must build
    this shape and add the edge E33-A1 to E16-012. `debug` and `on_quit` from the B1 proposal are
    dropped.
