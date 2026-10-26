# tasklens UI spec index

This file is the map of the six UI spec files in `docs/ui/`. It names what each file covers, puts
the shared vocabulary in one place and maps every merged spec ID (PROC-001 to PROC-099, PERF-001
to PERF-108, SYS-001 to SYS-037, APP-001 to APP-059) to the UI entries that realize it.

Precedence, highest first: `docs/DECISIONS.md`, `docs/RECONCILIATION.md` (R entries and canonical
tables a to d), `docs/ARCHITECTURE.md`, the merged specs in `docs/spec/merged/`, then the UI files.
Inside the UI files: `foundation.md` is the authority for components and tokens, `processes.md`
for `TlTreeTable` and its delegate, `shell.md` for window-level actions and constants
(consistency pass of 2026-10-03, R-104 to R-128).

## The six files

| File | ID prefix | Entries | Covers |
|---|---|---|---|
| `foundation.md` | UI-FND | 34 (001 to 003, 010 to 032, 040 to 043, 050 to 053) | Colour slots and derived tints, gpui-kit theme mapping, metrics (spacing, text, radii, rows, icons, motion, focus, shadows, scrollbars), every `Tl*` component, the theme pipeline (store, built-in loading, user themes, editor contract), fonts, tabular figures, truncation, bidi |
| `shell.md` | UI-SHL | 77 (001 to 140) | Main window frame and title bar, caption buttons, shield, toolbar buttons, tab strip, filter box, menus (File, View, Settings, Help, Debug), status chip and health items, Settings window pages, theme windows, confirmations, About, Crash Report, Additional Setup frame, Keyboard Shortcuts, Public IP notice, Run and Handle Search frames, toasts, startup and single instance, tray menu, keymap and accessibility |
| `processes.md` | UI-PRC | 32 (001 to 024, 030 to 037) | `TlTreeTable` (header, rows, cells, selection, keyboard, scrolling, column picker, header menu), process row renderer, row tooltip, row context menu, confirmations and toasts for process actions, filter behaviour, Ctrl pause, status text, toolbar buttons, submenus, jump target, empty states, History mode |
| `process-windows.md` | UI-PWN | 36 (001 to 036) | Secondary window frames for processes, the Properties data path, Process Properties (General, Threads, File descriptors, Memory maps, Security, Environment, Strings), App Properties, Set Affinity, Set Priority, Send Signal, Run window, Search File Handles, Low-memory rescue window, dump and VirusTotal toasts |
| `performance.md` | UI-PRF | 43 (001 to 009, 010 to 016, 020 to 036, 040 to 045, 050 to 053) | Chart element, graph palette, Performance tab frame and sidebar, summary mode, device pages (CPU, Memory, Disk, Network, GPU, Fan, Battery), minigraph panel, detached graph windows, mini window, tray graph icons, Additional Setup rows |
| `system-tabs.md` | UI-SYS | 70 (001 to 007, 010 to 024, 030 to 038, 040 to 050, 060 to 070, 080 to 096) | Shared system table frame and query, Services (strip, rows, menu, flows, Properties window with Logs), Startup Apps (rows, menu, flows, Properties), Connections (rows, menu, close flow, Properties, remote host, namespaces), Information tab |

## Shared vocabulary

### Tokens (`foundation.md` UI-FND-001 to UI-FND-003)

- Colour slots (`TlTheme` fields): `window_bg`, `titlebar`, `toolbar`, `input`, `input_focus`,
  `header`, `list_bg`, `row_alt`, `row_hover`, `row_selected`, `divider`, `menu_bg`, `menu_hover`,
  `tab_bg`, `tab_hover`, `scroll_track`, `scroll_thumb`, `text_primary`, `text_secondary`,
  `text_selected_primary`, `text_selected_secondary`, `value_heat`, `accent`; status colours
  `status.success`, `status.warning`, `status.danger`, `status.info`; graph palette `graph.*`
  (`cpu_fill`, `cpu_line`, `kernel_line`, `mem_fill`, `mem_line`, `gpu_fill`, `gpu_line`,
  `disk_fill`, `disk_line`, `net_fill`, `net_line`, `grid`, `chart_bg`); derived tints in
  `TlTheme.derived` (`overlay`, `skeleton`, `heat()`).
- Spacing `sp1`..`sp8` = 2, 4, 6, 8, 12, 16, 24, 32.
- Text `t_xs` 11/400, `t_sm` 12/400, `t_base` 13/400 (line height 17), `t_md` 14/500, `t_lg`
  16/600, `t_xl` 20/600. Numbers use `tnum`.
- Radii `r_sm` 4, `r_md` 6; `appearance.rounded_corners = false` sets both to 0.
- Icons (Lucide, kebab-case names): `i_xs` 12, `i_sm` 14, `i_md` 16, `i_lg` 20.
- Motion `m_fast` 120 ms, `m_base` 200 ms, `m_slow` 300 ms, `ease_out_cubic`; reduced motion and
  `motion.animations = false` turn every animation off.
- Rows: `row_h = 28 × row_height_pct/100 × ui_scale`; every size is multiplied by
  `ui_scale = ui_scale_pct / 100` once (`UiMetrics`).
- Scrollbars: kit `Scrollbar`, width 16, idle thumb 6, active 8, min length 48, radius 0;
  `ScrollbarMode::Always` on tables, `Scrolling` elsewhere.

### Shell constants (`shell.md`, `window/main.rs`)

`TITLEBAR_H` 34, `TAB_STRIP_H` 30, toast stack top 72 (34 + 30 + `sp4`), status chip height 26,
caption buttons 36 wide on the main window (kit 34 on secondary windows), menu item height 26,
process table `header_h` 44 (two lines) or 28 (`HeaderLines::One`), tree indent 18, chevron 16.

### Components

| Component | File, entry | Notes |
|---|---|---|
| `TlTheme`, `ThemeStore`, `theme::map::apply` | foundation UI-FND-001, 002, 040 to 043 | colour slots, kit mapping, switch path |
| `UiMetrics`, `TlFocusRing` | foundation UI-FND-003 | scale, hairline, focus ring |
| `TlButton` (`Primary`, `Secondary`, `Danger`, `Ghost`, `Icon`, `Toggle { on }`, `Segment { on }`; sizes sm 24, md 28) | foundation UI-FND-010 | one type for every button |
| `TlCheckbox`, `TlRadio`, `TlRadioGroup`, `TlSwitch`, `TlSlider`, `TlStepper`, `TlTextInput`, `TlFilterBox`, `TlSelect`, `TlCombo` | foundation UI-FND-011 to 017 | form controls |
| `TlTabs` | foundation UI-FND-018 | main tab strip and window tabs |
| `TlTooltip` (`text` 360 wide, `lines` 560 wide) | foundation UI-FND-019 | 500 ms, 600 ms on Name cells |
| `ToastQueue::push(kind, text, key: Option<ToastKey>, action)`, `ToastKey` | foundation UI-FND-020 | 4 visible, 5 s / 8 s / 15 s, replace in place |
| `TlDialog::confirm` (width 420, optional heading, focus on Cancel when destructive), `TlSecondaryWindow`, `WindowRegistry`, `WindowKey` | foundation UI-FND-021 | every dialog is a window (APP-045) |
| `TlMenu` (`min_w` 180 default, 240 process menus, 220 system table menus; `max_w` 360) | foundation UI-FND-022 | context, dropdown and menu bar menus |
| `TlProgress`, `TlSpinner` | foundation UI-FND-023 | never inside table cells or charts |
| `TlSkeletonRows`, `TlSkeletonBlock` | foundation UI-FND-024 | first load only |
| `TlKeyValueGrid` (`.row_height`, `.label_width`, `.block_indent`, `.block_gap`), `TlValue` | foundation UI-FND-025 | Properties, Information, Performance stats |
| `TlValidityGlyph` | foundation UI-FND-026 | `…`, lock, `n/a`, `-`, `!` |
| `TlBadge` (`Neutral`, `Info`, status kinds) | foundation UI-FND-027 | tags and counts |
| `TlShield` | foundation UI-FND-028 | `StartHelperSession`, `EndHelperSession` |
| `TlEmptyState` | foundation UI-FND-029 | icon, heading, text, one button |
| `TlBanner` (`Info`, `Warning`) | foundation UI-FND-031 | strip inside a pane |
| `TlLink` | foundation UI-FND-032 | inline link text |
| `TlTreeTable<D: TreeTableDelegate>`, `TreeTableDelegate`, `RowKind::{GroupHeader, Unit, Process, Plain}`, `HeaderLines::{One, Two}`, `ColumnSpec`, `CellTextCache`, `ProcessRowCells` | processes UI-PRC-001 to 011 | every table in the app (processes, history, services, startup, connections, Properties tabs, static Performance tables) |
| `ChartView`, `ChartGridView`, `AnimationDriver`, `GraphPalette`, `DeviceCard` | performance UI-PRF-001 to 008, 011 | charts, sidebar cards, minigraphs |
| `MainWindow`, `TabStrip`, `FilterBox`, `StatusChip`, `MenuBar`, `TlSettings` | shell UI-SHL-001 to 050 | window chrome |

### Actions

One GPUI namespace, `tasklens` (R-128). Window-level actions are the shell's (`SelectTab`,
`FocusFilter`, `ClearFilter`, `RefreshCurrentTab`, `OpenProperties`, `OpenColumnPicker { table }`,
`ResizeColumnsToFit { table }`, `ResetColumns { table }`, `SetSetting`, `ToggleSetting`,
`ToggleHideSystem`, `CycleViewMode`, `StartHelperSession`, `EndHelperSession`, `ShowMainWindow`,
`ToggleMainWindow`, `ToggleMiniWindow`, `OpenAdditionalSetup`, `OpenHandleSearch`,
`ShowSystemInformation`, `CopyInformationAll`, `Quit`). Table actions are shared (`CopyCell { col }`,
`CopyDetails`, `CopyRowsAsTable`, `SelectAll`, `CollapseAll`, `ToggleExpand`, `OpenContextMenu`,
`MoveFocus*`). Page actions carry the page word: `Perf*`, `Minigraph*`, `Service*`, `Services*`
(filter strip), `Startup*`, `Connection*`, `CopyInformationSection`, `Props*`, `*HandleSearch*`.
Keys and their contexts are canonical table (b) in `docs/RECONCILIATION.md`.

### Window keys (`WindowRegistry`, UI-FND-021)

| `WindowKey` | Window | Owner |
|---|---|---|
| `Properties(ProcessKey)` | Process Properties (cap 8) | process-windows UI-PWN-003 |
| `AppProperties(GroupKey)` | App Properties (cap 8) | process-windows UI-PWN-019 |
| `Service(UnitRef)` | Service Properties (cap 10) | system-tabs UI-SYS-030 |
| `StartupProperties(StartupRef)` | Startup Properties (cap 10) | system-tabs UI-SYS-050 |
| `ConnectionProperties(u64)` | Connection Properties by socket cookie (cap 10) | system-tabs UI-SYS-068 |
| `Run` | "tasklens Run" | process-windows UI-PWN-028 |
| `HandleSearch` | "tasklens Handle Search" | process-windows UI-PWN-030 |
| `Columns` | "tasklens Columns" | processes UI-PRC-007 |
| `Settings`, `ThemeEditor`, `ShareTheme`, `About`, `CrashReport`, `AdditionalSetup`, `Shortcuts`, `PublicIpNotice`, `CollectorHealth` | shell tool windows | shell UI-SHL-060, 081, 082, 092 to 099 |
| `SetupScript(&'static str)` | "tasklens-setup <subcommand>" script viewer, one per subcommand | shell UI-SHL-094, UI-PRF-052 |
| `Smart(DeviceIdx)`, `DiskInUse(DeviceIdx)` | "tasklens SMART Data", "tasklens Disk is in use" | performance UI-PRF-030 |
| `DetachedGraph(u8)`, `Mini` | detached graph windows, mini window | performance UI-PRF-044, 045 |
| none (modal child of its parent) | confirmations (`TlDialog::confirm`), Set Affinity, Set Priority, Send Signal | foundation UI-FND-021, process-windows UI-PWN-021 to 027 |
| none (own process) | "tasklens - Low memory" | process-windows UI-PWN-032 |

### Snapshot component bits (`ComponentMask`)

`SYSTEM`, `SERIES`, `PROCESSES`, `TABLE`, `CONNECTIONS`, `SERVICES`, `STARTUP`, `INVENTORY`,
`DEVICES`, `HEALTH`, `WINDOWS`, `DETAILS`, `HISTORY` (ARCHITECTURE ADR-003). A view notifies only
on the bits it draws; chart views also check `changed_series`.

## Global ID table

"UI entries" lists every entry that realizes the ID, in file order (foundation, shell, processes,
process-windows, performance, system-tabs). "Verdict" and "Phase" are the merged spec's. An ID
with no UI entry says why: "engine only" (the behaviour has no surface of its own; the entries
named show its effects) or "dropped" (the merged spec's verdict).

### PROC (docs/spec/merged/proc.md)

| ID | Title | Verdict | Phase | UI entries |
|---|---|---|---|---|
| PROC-001 | Process identity and action targeting | BUILD | P1 | UI-PRC-008, UI-PRC-009, UI-PRC-013, UI-PRC-017 |
| PROC-002 | Process sweep and per-process data pipeline | BUILD | P1 | engine only: the sweep and pipeline (ADR-004, ADR-008); its results show through UI-PRC-004 validity glyphs |
| PROC-003 | Value formats, zero values and units | ADAPT | P1 | UI-PRC-004 |
| PROC-004 | Process classification and Task Manager groups | ADAPT | P1 | UI-PRC-002 |
| PROC-005 | App detection from cgroups | BUILD | P1 | engine only: cgroup app detection (ADR-016); the groups it produces are drawn by UI-PRC-002 |
| PROC-006 | Desktop-file index, exe fallback, disambiguation and icons | BUILD | P1 | UI-PRC-002, UI-PRC-003 |
| PROC-007 | Process display naming | ADAPT | P1 | UI-PRC-003 |
| PROC-008 | App row contents | ADAPT | P1 (window titles P2) | UI-PRC-002, UI-PRC-003 |
| PROC-009 | Unit rows and the Kernel threads row | ADAPT | P1 (Description P2) | UI-PRC-002 |
| PROC-010 | Flatpak bwrap unwrap | BUILD | P1 | engine only: bwrap unwrap feeds the PROC-007 name shown by UI-PRC-003 |
| PROC-011 | Launcher split (Steam, Lutris, Heroic) | ADAPT | P4 | engine only (P4): launcher split feeds grouping drawn by UI-PRC-002 |
| PROC-012 | Pseudo-process rows | ADAPT | P2 | UI-PRC-002, UI-PRC-012, UI-PRC-013 |
| PROC-013 | Table widget basis | BUILD | P1 | UI-PRC-001, UI-PRC-010, UI-PRC-023 |
| PROC-014 | View modes and View > Process View | ADAPT | P1 | UI-SHL-035, UI-PRC-002, UI-PRC-019 |
| PROC-015 | Toolbar view-mode cycler | BUILD | P1 | UI-SHL-013, UI-PRC-015, UI-PRC-019 |
| PROC-016 | Processes-tab toolbar buttons | BUILD | P1 | UI-SHL-010, UI-SHL-013, UI-PRC-019, UI-PRC-030 |
| PROC-017 | Tree Connector Lines | BUILD | P1 | UI-SHL-035, UI-PRC-003 |
| PROC-018 | PID cell "PID +N children" | BUILD | P1 | UI-PRC-002, UI-PRC-004 |
| PROC-019 | Subtree totals on parent processes | BUILD | P2 | UI-PRC-002, UI-PRC-004 |
| PROC-020 | Collapse All | BUILD | P1 | UI-SHL-035, UI-PRC-009 |
| PROC-021 | Startup placeholder | ADAPT | P1 | UI-FND-024, UI-FND-026, UI-PRC-002, UI-PRC-024 |
| PROC-022 | Column model, default set and visibility | ADAPT | P1 (Linux extras P2 and later, per column) | UI-PRC-001, UI-PRC-006, UI-PRC-007 |
| PROC-023 | Column minimum and default widths | BUILD | P1 | UI-FND-051, UI-PRC-005 |
| PROC-024 | Column picker | ADAPT | P1 | UI-FND-011, UI-FND-016, UI-FND-022, UI-SHL-034, UI-PRC-007 |
| PROC-025 | Column reorder, resize and persistence | BUILD | P1 | UI-SHL-034, UI-PRC-001, UI-PRC-005, UI-PRC-006 |
| PROC-026 | Header context menu | BUILD | P1 | UI-SHL-034, UI-PRC-006, UI-PRC-015 |
| PROC-027 | Columns: identity, image and package | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-028 | Columns: ownership, session and security | ADAPT | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-029 | Columns: CPU and scheduling | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-030 | Columns: memory | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-031 | Columns: I/O and disk | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-032 | Column: network | ADAPT | P1 (TCP attribution); P4 (`tl-netmon` exact counting) | UI-PRC-004, UI-PRC-005 |
| PROC-033 | Columns: GPU | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-034 | Columns: Power usage and Power usage trend | ADAPT | P2 (the setup window is P3) | UI-PRC-004, UI-PRC-005 |
| PROC-035 | Columns: object counts | see entry | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-036 | Linux-only extra columns | ADAPT | see the table | UI-PRC-004, UI-PRC-005 |
| PROC-037 | TaskSlinger column id map | ADAPT | P1 | no UI: TaskSlinger id map; tasklens uses string column ids |
| PROC-038 | Two-line header with system totals | BUILD | P1 | UI-PRC-005 |
| PROC-039 | Row hover tooltip | ADAPT | P1 (Package line P2) | UI-FND-019, UI-PRC-012 |
| PROC-040 | Heat colouring | BUILD | P1 | UI-FND-001, UI-PRC-004 |
| PROC-041 | Dim zero values | BUILD | P2 | UI-PRC-004 |
| PROC-042 | Column separators | BUILD | P4 | UI-PRC-002, UI-PRC-004 |
| PROC-043 | Highlight new and exited processes | BUILD | P1 | UI-SHL-035, UI-PRC-011, UI-PRC-013 |
| PROC-044 | Filter box and type-to-filter | ADAPT | P1 | UI-SHL-024, UI-SHL-124, UI-PRC-009, UI-PRC-016, UI-PRC-024 |
| PROC-045 | Include children | BUILD | P1 | UI-SHL-025, UI-PRC-016 |
| PROC-046 | Hide system processes | ADAPT | P1 | UI-SHL-012, UI-PRC-019, UI-PRC-031 |
| PROC-047 | Sorting | BUILD | P1 | UI-PRC-005, UI-PRC-006 |
| PROC-048 | Selection and multi-select | BUILD | P1 | UI-PRC-008, UI-PRC-013, UI-PRC-018 |
| PROC-049 | Keyboard in the process table | ADAPT | P1 | UI-PRC-009 |
| PROC-050 | Hold Ctrl to pause | BUILD | P1 | UI-SHL-050, UI-SHL-123, UI-PRC-017, UI-PRC-018 |
| PROC-051 | Processes status bar text | BUILD | P1 | UI-SHL-050, UI-PRC-018 |
| PROC-052 | KWin window bridge | ADAPT | P2 | engine only: KWin bridge worker (ADR-004); its data shows through UI-PRC-003 and UI-PWN-007, its absence through UI-SHL-051 |
| PROC-053 | App window titles and the Name-cell window button | ADAPT | P2 | UI-PRC-003 |
| PROC-054 | Show Window | ADAPT | P2 | UI-PRC-003, UI-PRC-013, UI-PRC-015 |
| PROC-055 | Window picker ("+" button) | ADAPT | P3 | UI-SHL-014, UI-PRC-014, UI-PRC-022 |
| PROC-056 | Pin on top for tasklens windows | ADAPT | P2 | UI-FND-021, UI-SHL-005, UI-SHL-090, UI-PWN-001, UI-PWN-003, UI-PWN-032 |
| PROC-057 | Row context menu | ADAPT | P1 for the menu and the P1 items; other items arrive with their own entries. | UI-FND-022, UI-PRC-013 |
| PROC-058 | Action pipeline, results and toasts | BUILD | P1 (retry button P3) | UI-FND-020, UI-SHL-011, UI-PRC-015, UI-PWN-011, UI-PWN-021, UI-PWN-022, UI-PWN-023, UI-PWN-026, UI-PWN-031 |
| PROC-059 | Protection rules and confirmations | ADAPT | P1 | UI-FND-021, UI-PRC-014, UI-PRC-015, UI-PWN-011, UI-PWN-025, UI-PWN-026, UI-PWN-031 |
| PROC-060 | End Task and End App (SIGTERM) | ADAPT | P1 | UI-PRC-013, UI-PRC-014, UI-PRC-015 |
| PROC-061 | End Process (SIGKILL) | ADAPT | P1 | UI-PRC-013, UI-PRC-014, UI-PRC-015 |
| PROC-062 | End Process Tree | ADAPT | P1 | UI-PRC-013, UI-PRC-014, UI-PRC-015 |
| PROC-063 | Suspend and Resume process | ADAPT | P1 | UI-PRC-013, UI-PRC-014, UI-PRC-015 |
| PROC-064 | Freeze App, Thaw App and Kill App | ADAPT | P2 | UI-PRC-002, UI-PRC-013, UI-PRC-014, UI-PRC-015 |
| PROC-065 | Send Signal submenu | BUILD | P2 | UI-PRC-020, UI-PWN-026, UI-PWN-027 |
| PROC-066 | Set Priority | ADAPT | P1 (High to Low); P2 (Realtime, Idle, Custom) | UI-PRC-014, UI-PRC-020, UI-PRC-021, UI-PWN-011, UI-PWN-023, UI-PWN-024 |
| PROC-067 | Set Affinity | ADAPT | P1 (one target); P2 (multi-select mixed marks) | UI-PRC-021, UI-PWN-007, UI-PWN-011, UI-PWN-021 |
| PROC-068 | Set I/O Priority | ADAPT | P2 | UI-PRC-020 |
| PROC-069 | Multi-select priority, affinity, I/O priority, copy and end | BUILD | P2 (priority, affinity, I/O priority); P1 (copy and end) | UI-PRC-013, UI-PRC-015, UI-PRC-020, UI-PRC-021, UI-PWN-022, UI-PWN-023 |
| PROC-070 | Copy, Copy details, Copy all metrics and Copy rows as table | BUILD | P1 | UI-PRC-013, UI-PRC-015, UI-PRC-037 |
| PROC-071 | Process jump target, Go to parent and Go to service | BUILD | P1 (Go to parent); P2 (the shared entry point for other callers, Go to service) | UI-PRC-010, UI-PRC-015, UI-PRC-022, UI-PWN-003, UI-PWN-007, UI-PWN-020, UI-PWN-031 |
| PROC-072 | Restart Plasma Shell and Restart unit | ADAPT | P2 | UI-PRC-013, UI-PRC-015 |
| PROC-073 | Open file location and File properties | ADAPT | P1 (Open file location); P2 (File properties) | UI-PRC-013, UI-PRC-015, UI-PWN-007, UI-PWN-012, UI-PWN-013, UI-PWN-031 |
| PROC-074 | Open in VirusTotal | BUILD | P2 | UI-PRC-013, UI-PRC-014, UI-PRC-015, UI-PWN-008, UI-PWN-036 |
| PROC-075 | Create dump file | ADAPT | P3 | UI-PRC-013, UI-PRC-014, UI-PRC-015, UI-PWN-034, UI-PWN-035 |
| PROC-076 | Privileged helper for process operations | ADAPT | P3 | UI-PRC-015, UI-PRC-018, UI-PWN-018, UI-PWN-030 |
| PROC-077 | Process Properties window | ADAPT | P2 | UI-FND-016, UI-FND-018, UI-FND-021, UI-FND-029, UI-PRC-009, UI-PRC-015, UI-PWN-001, UI-PWN-002, UI-PWN-003, UI-PWN-004, UI-PWN-005 |
| PROC-078 | General tab | ADAPT | P2 | UI-FND-025, UI-PWN-006, UI-PWN-007, UI-PWN-008, UI-PWN-009 |
| PROC-079 | Threads tab | ADAPT | P2 | UI-PWN-010, UI-PWN-011 |
| PROC-080 | File descriptors tab | ADAPT | P2 | UI-PWN-012, UI-PWN-018 |
| PROC-081 | Memory maps tab | ADAPT | P2 | UI-PWN-013, UI-PWN-018 |
| PROC-082 | Security tab | ADAPT | P2 | UI-PWN-014, UI-PWN-015, UI-PWN-018 |
| PROC-083 | Environment tab | ADAPT | P2 | UI-PWN-016, UI-PWN-018 |
| PROC-084 | Strings tab | ADAPT | P3 | UI-PWN-005, UI-PWN-017, UI-PWN-018 |
| PROC-085 | App Properties window | ADAPT | P2 | UI-PWN-019, UI-PWN-020 |
| PROC-086 | History recording | ADAPT | P3 | UI-SHL-023, UI-SHL-040, UI-SHL-065, UI-PRC-036 |
| PROC-087 | History persistence | ADAPT | P3 | UI-SHL-023, UI-SHL-040, UI-SHL-065, UI-PRC-030, UI-PRC-036 |
| PROC-088 | History view and table | BUILD | P3 | UI-SHL-023, UI-SHL-050, UI-PRC-022, UI-PRC-030, UI-PRC-031, UI-PRC-035, UI-PRC-036, UI-PRC-037 |
| PROC-089 | History timeline scrubber | BUILD | P3 | UI-PRC-032, UI-PRC-033 |
| PROC-090 | History top-3 hover | BUILD | P3 | UI-PRC-034 |
| PROC-091 | History first-snapshot state | BUILD | P3 | UI-PRC-030 |
| PROC-092 | History scrub benchmark | BUILD | P3 | UI-PRC-032 |
| PROC-093 | Process minigraphs from the row menu | BUILD | P3, for the menu items and the process cards (R-101) | UI-PRC-013, UI-PRC-015, UI-PRC-020 |
| PROC-094 | Process tray icon graphs | ADAPT | P3 | UI-PRC-013, UI-PRC-015, UI-PRC-020 |
| PROC-095 | Run dialog | ADAPT | P2 | UI-SHL-032, UI-SHL-097, UI-PWN-028, UI-PWN-029 |
| PROC-096 | Search File Handles | ADAPT | P3 | UI-SHL-032, UI-SHL-098, UI-PWN-030, UI-PWN-031 |
| PROC-097 | Low-memory watcher | ADAPT | P3 | UI-SHL-040, UI-SHL-069, UI-PWN-032 |
| PROC-098 | Low-memory rescue window | ADAPT | P3 | UI-SHL-110, UI-PWN-032, UI-PWN-033 |
| PROC-099 | Settings and state keys for this area | ADAPT | P1 (each key arrives with its feature) | UI-SHL-065, UI-PRC-002, UI-PRC-004, UI-PRC-005, UI-PRC-006, UI-PRC-011, UI-PRC-016, UI-PRC-019, UI-PRC-030, UI-PRC-031, UI-PRC-036 |

### PERF (docs/spec/merged/perf.md)

| ID | Title | Verdict | Phase | UI entries |
|---|---|---|---|---|
| PERF-001 | Device sidebar cards | BUILD | P1 (fan and battery cards arrive with their pages in P2) | UI-PRF-002, UI-PRF-011 |
| PERF-002 | Hide and show page context menu | BUILD | P1 | UI-PRF-012, UI-PRF-013, UI-PRF-014 |
| PERF-003 | Sidebar edit mode | BUILD | P2 | UI-PRF-013 |
| PERF-004 | Per-type default visibility and network groups | ADAPT | P2 (the P1 default behaviour, virtual hidden and loopback never shown, is built with PERF-054) | UI-SHL-066, UI-PRF-012 |
| PERF-005 | Device hotplug and stable keys | BUILD | P1 for disks and network; P2 for the rest. | UI-PRF-011, UI-PRF-012 |
| PERF-006 | Sidebar width splitter | BUILD | P1 | UI-PRF-010 |
| PERF-007 | Performance keys, View submenu and sidebar toggle | BUILD | P1 for F1 to F8, Up and Down, and F9 and Ctrl+T; P2 for the View submenu. | UI-SHL-120, UI-PRF-010, UI-PRF-012, UI-PRF-015 |
| PERF-008 | Device type names, headers and empty states | ADAPT | P1 (aggregate strings with PERF-009 in P4; edit-mode strings with PERF-003 in P2) | UI-PRF-011, UI-PRF-012, UI-PRF-013, UI-PRF-020, UI-PRF-036 |
| PERF-009 | Aggregate pages | BUILD | P4 | UI-PRF-036 |
| PERF-010 | Graph summary mode | ADAPT | P2 | UI-PRF-015, UI-PRF-016 |
| PERF-011 | Copy device info | BUILD | P2 | UI-FND-025, UI-PRF-015, UI-PRF-022 |
| PERF-012 | Page layout | BUILD | P1 | UI-FND-025, UI-FND-026, UI-PRF-010, UI-PRF-020, UI-PRF-021 |
| PERF-013 | Fallback value words | ADAPT | P1 | UI-FND-025, UI-FND-026, UI-SHL-051, UI-PRF-011, UI-PRF-021, UI-SYS-082 |
| PERF-014 | Performance-tab toolbar contents | BUILD | P1 | UI-SHL-010, UI-PRF-010, UI-PRF-020 |
| PERF-015 | Graph element | BUILD | P1 | UI-PRF-001, UI-PRF-003 |
| PERF-016 | Time range | ADAPT | P1 for 30 s and 1 min; P2 for 5 and 10 min. | UI-SHL-066, UI-PRF-001, UI-PRF-003, UI-PRF-015 |
| PERF-017 | Series rings and the 1 Hz tier | BUILD | P1 for the fast tier; P2 for the 1 Hz tier. | UI-PRF-001, UI-PRF-007 |
| PERF-018 | Scaling modes | ADAPT | P1 | UI-PRF-006 |
| PERF-019 | Axis maximum rounding and label | BUILD | P1 | UI-PRF-003, UI-PRF-006 |
| PERF-020 | Smooth (sliding) graphs | ADAPT | P1 | UI-PRF-001, UI-PRF-005 |
| PERF-021 | Curved lines | ADAPT | P4 | UI-SHL-066, UI-PRF-009 |
| PERF-022 | Gaps and stale data | BUILD | P1 | UI-PRF-001, UI-PRF-007 |
| PERF-023 | Hover readout | BUILD | P2 | UI-PRF-004 |
| PERF-024 | Graph colours | BUILD | P1 for the palette; P3 for per-card colour. | UI-PRF-008, UI-PRF-041 |
| PERF-025 | Units and number formats | ADAPT | P1 for formats and menu items; P2 for the settings page. | UI-PRF-003, UI-PRF-031 |
| PERF-026 | Legends and secondary series | BUILD | P1 | UI-PRF-001, UI-PRF-003 |
| PERF-027 | CPU header and graph mode | ADAPT | P1 for Overall and Per core; P2 for the other two. | UI-SHL-066, UI-PRF-023 |
| PERF-028 | Per-core grid | BUILD | P1 | UI-PRF-002, UI-PRF-023 |
| PERF-029 | Kernel time overlay | BUILD | P1 | UI-PRF-023 |
| PERF-030 | Overlaid and stacked modes | BUILD | P2 | UI-PRF-003, UI-PRF-006, UI-PRF-023 |
| PERF-031 | Core type marks | ADAPT | P2 | UI-PRF-023 |
| PERF-032 | CPU secondary graph | BUILD | P2 | UI-SHL-066, UI-PRF-025 |
| PERF-033 | CPU live stats | ADAPT | P1 (Power draw row with PERF-038 in P2) | UI-PRF-023, UI-PRF-024 |
| PERF-034 | Up time format | BUILD | P1 | UI-PRF-024 |
| PERF-035 | CPU static stats | BUILD | P1 for cores, logical processors and caches; P2 for the rest. | UI-PRF-024 |
| PERF-036 | Cache size format | ADAPT | P1 | UI-PRF-024 |
| PERF-037 | Virtualization and security rows | ADAPT | P2 | UI-PRF-024 |
| PERF-038 | CPU power (RAPL) | BUILD | P2 | UI-PRF-024, UI-PRF-025 |
| PERF-039 | CPU temperature | BUILD | P1 | UI-PRF-024, UI-PRF-025 |
| PERF-040 | CPU clock | BUILD | P1 | UI-PRF-024, UI-PRF-025 |
| PERF-041 | Memory graphs | BUILD | P1 | UI-SHL-066, UI-PRF-026 |
| PERF-042 | Memory composition bar | ADAPT | P1 | UI-PRF-027 |
| PERF-043 | Memory stats | ADAPT | P1 for meminfo rows; P2 for DIMM rows. | UI-PRF-028 |
| PERF-044 | zram and zswap details | ADAPT | P2 | UI-PRF-028 |
| PERF-045 | Swap devices list | ADAPT | P2 | UI-PRF-028 |
| PERF-046 | Memory modules on the Memory page | BUILD | P2 | UI-PRF-028 |
| PERF-047 | Disk list and kind | ADAPT | P1 | UI-PRF-029 |
| PERF-048 | Disk graphs | BUILD | P1 | UI-PRF-029 |
| PERF-049 | Disk stats | ADAPT | P1 for rates, active time, capacity and response time; P2 for the rest. | UI-PRF-030 |
| PERF-050 | Partitions table | ADAPT | P2 | UI-PRF-030 |
| PERF-051 | SMART data dialog | BUILD | P2 (the smartctl fallback in P3) | UI-PRF-029, UI-PRF-030 |
| PERF-052 | Eject with blocking processes | BUILD | P2 | UI-PRF-029, UI-PRF-030 |
| PERF-053 | Disk temperature | BUILD | P2 | UI-PRF-011, UI-PRF-030 |
| PERF-054 | Interface list and type | ADAPT | P1 | UI-PRF-011, UI-PRF-012, UI-PRF-031 |
| PERF-055 | Throughput graph and legend | BUILD | P1 | UI-PRF-003, UI-PRF-031 |
| PERF-056 | Network stats | BUILD | P1 for rates and totals; P2 for the rest. | UI-PRF-031 |
| PERF-057 | Display unit row | BUILD | P1 | UI-PRF-031 |
| PERF-058 | Wi-Fi details | ADAPT | P2 | UI-PRF-031 |
| PERF-059 | IP addresses | ADAPT | P2 | UI-PRF-031 |
| PERF-060 | Public IP fetch | BUILD | P2 | UI-SHL-096, UI-PRF-031 |
| PERF-061 | Network settings link | ADAPT | P3 | UI-PRF-031 |
| PERF-062 | GPU enumeration and tl-gpu-probe | ADAPT | P1 | UI-PRF-032 |
| PERF-063 | Engine graphs | ADAPT | P1 for 3D/Compute, Video Encode and Video Decode; P2 for the rest, hiding, and the optional row. | UI-SHL-066, UI-PRF-002, UI-PRF-032 |
| PERF-064 | GPU memory | BUILD | P1 | UI-PRF-032, UI-PRF-033 |
| PERF-065 | Video encode and decode | BUILD | P1 | UI-PRF-032, UI-PRF-033 |
| PERF-066 | GPU clocks | BUILD | P2 | UI-PRF-032, UI-PRF-033 |
| PERF-067 | GPU power | BUILD | P2 | UI-PRF-032, UI-PRF-033 |
| PERF-068 | GPU temperature and fan | BUILD | P1 for temperature; P2 for fan and thresholds. | UI-PRF-032, UI-PRF-033 |
| PERF-069 | GPU temperature source values | ADAPT | P1 for the value; P2 for the Information row. | UI-PRF-033 |
| PERF-070 | PCIe link | BUILD | P2 | UI-PRF-033 |
| PERF-071 | GPU static fields | ADAPT | P2 | UI-PRF-033 |
| PERF-072 | Displays list | ADAPT | P2 (HDR, VRR and WCG in P4) | UI-PRF-033 |
| PERF-073 | Display connector type names and row format | ADAPT | P2 | UI-PRF-033 |
| PERF-074 | AMD GPU backend | ADAPT | P2 | UI-PRF-032, UI-PRF-033 |
| PERF-075 | Intel GPU backend | ADAPT | P3 | UI-PRF-032, UI-PRF-033 |
| PERF-076 | Fan pages | BUILD | P2 | UI-SHL-066, UI-PRF-034 |
| PERF-077 | Fan labels and temperature pairing | ADAPT | P3 | UI-SHL-066, UI-PRF-034 |
| PERF-078 | Battery and power device pages | BUILD | P2 | UI-PRF-035 |
| PERF-079 | Battery charge history | BUILD | P3 | UI-PRF-035 |
| PERF-080 | Minigraph panel and system cards | BUILD | P1 (cards, View > Graphs checkboxes and Reset Minigraph Layout; R-089). "Keep detached graphs open" is P3 (PERF-084). | UI-SHL-039, UI-SHL-067, UI-PRF-040, UI-PRF-041 |
| PERF-081 | Minigraph card context menu | BUILD | P1 for hide and range; P3 for colour, detach and tray. | UI-SHL-085, UI-PRF-042 |
| PERF-082 | Move and reorder cards | BUILD | P2 | UI-PRF-040, UI-PRF-042, UI-PRF-043 |
| PERF-083 | Detached graph windows | ADAPT | P3 (needs the P2 KWin bridge) | UI-PRF-042, UI-PRF-043, UI-PRF-044 |
| PERF-084 | Keep detached graphs open | BUILD | P3 | UI-SHL-039, UI-SHL-067, UI-SHL-113, UI-PRF-044 |
| PERF-085 | Process-source cards and labels | BUILD | P3 | UI-PRF-041 |
| PERF-086 | Minigraph states "No match" and "Ended" | BUILD | P3 (with PERF-085; the addendum's P2 is moved to match the menu that creates these cards, R-101) | UI-PRF-041, UI-PRF-050 |
| PERF-087 | Column labels for process cards | ADAPT | P3 | UI-PRF-041 |
| PERF-088 | Tray icon graph | ADAPT | P3 | UI-SHL-113, UI-PRF-014, UI-PRF-015, UI-PRF-042, UI-PRF-050 |
| PERF-089 | Tray tooltips | BUILD | P3 | UI-PRF-051 |
| PERF-090 | Tray menus and graph colour | BUILD | P3 | UI-SHL-085, UI-SHL-113, UI-PRF-051 |
| PERF-091 | Main tray icon while graphs show | BUILD | P3 | UI-SHL-113, UI-PRF-050, UI-PRF-051 |
| PERF-092 | Tray icon re-registration | ADAPT | P3 | UI-SHL-113, UI-PRF-050, UI-PRF-051 |
| PERF-093 | Performance mini window | ADAPT | P3 | UI-SHL-036, UI-PRF-045 |
| PERF-094 | Information layout, copy and filter | BUILD | P2 | UI-SHL-023, UI-SYS-080, UI-SYS-081, UI-SYS-083, UI-SYS-084, UI-SYS-085, UI-SYS-096 |
| PERF-095 | Operating System section | ADAPT | P2 | UI-SYS-081, UI-SYS-086 |
| PERF-096 | Processor section | BUILD | P2 | UI-SYS-087 |
| PERF-097 | System section | ADAPT | P2 | UI-SYS-088 |
| PERF-098 | Memory section | ADAPT | P2 | UI-SYS-089 |
| PERF-099 | Memory modules section | BUILD | P2 | UI-SYS-090 |
| PERF-100 | Disk drives section | ADAPT | P2 | UI-SYS-091 |
| PERF-101 | BIOS/Firmware section | BUILD | P2 | UI-SYS-092 |
| PERF-102 | Motherboard section | BUILD | P2 | UI-SYS-082, UI-SYS-093 |
| PERF-103 | Graphics section | ADAPT | P2 | UI-SYS-094 |
| PERF-104 | Network adapters section | BUILD | P2 | UI-SYS-095 |
| PERF-105 | Additional setup window | ADAPT | P3 (Fan sensors row P4) | UI-SHL-094, UI-PRF-052 |
| PERF-106 | RAPL udev rule | ADAPT | P3 | UI-PRF-052 |
| PERF-107 | Per-process network row (nethogs capabilities dropped) | DROP | P3 (the row text only) | UI-PRF-052 (the nethogs capability step itself is dropped; the row shows the state) |
| PERF-108 | Fan sensor detection | ADAPT | P4 | UI-PRF-034, UI-PRF-052 |

### SYS (docs/spec/merged/sys-app.md)

| ID | Title | Verdict | Phase | UI entries |
|---|---|---|---|---|
| SYS-001 | Services tab, two systemd managers | ADAPT | P2 | UI-SYS-001, UI-SYS-005, UI-SYS-010, UI-SYS-014, UI-SYS-015, UI-SYS-016 |
| SYS-002 | Unit types shown | ADAPT | P2 | UI-SYS-011, UI-SYS-012 |
| SYS-003 | Services columns | ADAPT | P2 (resource columns P2) | UI-FND-027, UI-SYS-003, UI-SYS-013, UI-SYS-015 |
| SYS-004 | State filters and filter strip | BUILD | P2 | UI-SYS-002, UI-SYS-005, UI-SYS-011 |
| SYS-005 | Live updates and job tracking | ADAPT | P2 | UI-SYS-007, UI-SYS-014, UI-SYS-018 |
| SYS-006 | Start, Stop, Restart, Reload, Reset failed | BUILD | P2 | UI-SYS-007, UI-SYS-016, UI-SYS-017, UI-SYS-018, UI-SYS-023 |
| SYS-007 | Startup type changes (enable, disable, mask) | ADAPT | P2 | UI-SYS-017, UI-SYS-019, UI-SYS-031 |
| SYS-008 | Services context menu | ADAPT | P2 | UI-SYS-017 |
| SYS-009 | Copy formats (services) | BUILD | P2 | UI-SYS-006, UI-SYS-020 |
| SYS-010 | Jump between services and processes | BUILD | P2 | UI-SYS-021, UI-SYS-065 |
| SYS-011 | Service Properties window: frame and General tab | ADAPT | P2 | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-030, UI-SYS-031 |
| SYS-012 | Service Properties: Log On tab | ADAPT | P2 | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-032 |
| SYS-013 | Service Properties: Recovery tab | ADAPT | P2 view, P3 edit | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-033, UI-SYS-034 |
| SYS-014 | Service Properties: Dependencies tab | ADAPT | P2 | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-035 |
| SYS-015 | Service logs (journal) | ADAPT | P2 | UI-SYS-030, UI-SYS-036, UI-SYS-037, UI-SYS-038 |
| SYS-016 | Services status bar and loading state | BUILD | P2 | UI-FND-020, UI-FND-024, UI-SYS-001, UI-SYS-004, UI-SYS-022 |
| SYS-017 | Services sort and column persistence | BUILD | P2 | UI-SYS-013 |
| SYS-018 | OpenRC backend | BUILD | P4 | UI-SYS-024 |
| SYS-019 | Startup sources and precedence | ADAPT | P2 | UI-SYS-040, UI-SYS-042 |
| SYS-020 | Startup columns and values | ADAPT | P2 | UI-FND-027, UI-SYS-003, UI-SYS-041, UI-SYS-042 |
| SYS-021 | Startup impact | ADAPT | P2 | UI-SYS-043 |
| SYS-022 | Enable and disable | ADAPT | P2 | UI-SYS-044, UI-SYS-045 |
| SYS-023 | Run now | ADAPT | P2 | UI-SYS-044, UI-SYS-046 |
| SYS-024 | Delete entry | ADAPT | P2 | UI-SYS-044, UI-SYS-047 |
| SYS-025 | Startup context menu, copy and open | BUILD | P2 | UI-SYS-006, UI-SYS-044, UI-SYS-048 |
| SYS-026 | Startup Properties window | ADAPT | P2 | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-050 |
| SYS-027 | Startup status bar, sort and persistence | BUILD | P2 | UI-SYS-004, UI-SYS-041, UI-SYS-049 |
| SYS-028 | Connection inventory | ADAPT | P2 | UI-SYS-004, UI-SYS-060 |
| SYS-029 | Connection columns, address formats, states | ADAPT | P2 | UI-FND-027, UI-SYS-003, UI-SYS-061, UI-SYS-062 |
| SYS-030 | Owning process attribution | ADAPT | P2 (own processes), P3 (helper for others) | UI-SYS-062 |
| SYS-031 | Close TCP connection | ADAPT | P3 (needs tl-helper) | UI-SYS-063, UI-SYS-064 |
| SYS-032 | Connections context menu and copy formats | BUILD | P2 | UI-SYS-006, UI-SYS-063, UI-SYS-065, UI-SYS-066 |
| SYS-033 | Jump to owning process | BUILD | P2 | UI-SYS-065 |
| SYS-034 | Connection Properties window | ADAPT | P2 | UI-FND-018, UI-FND-021, UI-FND-025, UI-SYS-068 |
| SYS-035 | Remote host name column | ADAPT | P4 | UI-SYS-061, UI-SYS-069 |
| SYS-036 | Connections status bar, filter, sort, refresh | BUILD | P2 | UI-SYS-061, UI-SYS-067 |
| SYS-037 | Other network namespaces | ADAPT | P4 | UI-SYS-061, UI-SYS-070 |

### APP (docs/spec/merged/sys-app.md)

| ID | Title | Verdict | Phase | UI entries |
|---|---|---|---|---|
| APP-001 | Window decorations (system title bar by default, D-035) | BUILD | P1 | UI-SHL-001, UI-SHL-002, UI-SHL-003, UI-SHL-004, UI-SHL-006 |
| APP-002 | Title bar layout and toolbar | ADAPT | P1 | UI-FND-010, UI-SHL-003, UI-SHL-004, UI-SHL-010 |
| APP-003 | Shield: unlock privileged actions | ADAPT | P3 | UI-FND-028, UI-SHL-010, UI-SHL-011 |
| APP-004 | Hide system processes button | ADAPT | P1 | UI-SHL-012 |
| APP-005 | View mode button | BUILD | P1 | UI-SHL-013 |
| APP-006 | Window picker "+" | ADAPT | P3 | UI-SHL-014 |
| APP-007 | Update badge | DROP | P4 (recorded as dropped) | dropped: no updater, so no update badge (UI-SHL-003 part list) |
| APP-008 | Caption buttons | BUILD | P1 (pin P2) | UI-FND-021, UI-SHL-005 |
| APP-009 | Always on top and the pin | ADAPT | P2 | UI-FND-021, UI-SHL-005, UI-SHL-036, UI-SHL-090, UI-PWN-001 |
| APP-010 | Size, minimum size and geometry restore | ADAPT | P1 | UI-SHL-001, UI-SHL-002 |
| APP-011 | Single instance | ADAPT | P1 | UI-SHL-112 |
| APP-012 | File menu | ADAPT | P1 (Exit), later items in their own phase | UI-SHL-030, UI-SHL-032 |
| APP-013 | Run Installer item | DROP | P4 (recorded) | dropped: pacman installs tasklens (UI-SHL-032 File menu has no item) |
| APP-014 | Run dialog (window frame and privilege path) | ADAPT | P2 | UI-SHL-032, UI-SHL-097, UI-PWN-028 |
| APP-015 | Search File Handles window (frame) | ADAPT | P3 | UI-SHL-032, UI-SHL-098, UI-PWN-030 |
| APP-016 | Launch other system monitor | ADAPT | P4 | UI-SHL-032 |
| APP-017 | View menu structure | BUILD | P1 (Columns, Process View, Window, Graphs), P2 (Appearance, Theme) | UI-SHL-033 |
| APP-018 | View > Columns (all tables) | BUILD | P1 | UI-SHL-034 |
| APP-019 | View > Process View | BUILD | P1 | UI-SHL-035 |
| APP-020 | View > Window | ADAPT | P2 (Always On Top), P3 (Performance mini window) | UI-SHL-036 |
| APP-021 | View > Graphs | BUILD | P1 (graph checks and Reset, with the system minigraph cards, PERF-080 and table a), P3 (Keep detached graphs open) | UI-SHL-039 |
| APP-022 | Settings menu | ADAPT | P1 (Refresh, units, Default tab, Close with Esc, motion items), others in their feature's phase | UI-SHL-040, UI-SHL-072, UI-SHL-091 |
| APP-023 | Settings window ("All settings…") | ADAPT | P2 | UI-FND-011, UI-FND-012, UI-FND-013, UI-FND-014, UI-FND-015, UI-FND-016, UI-FND-017, UI-FND-018, UI-FND-021, UI-SHL-060, UI-SHL-061, UI-SHL-062, UI-SHL-063, UI-SHL-064, UI-SHL-065, UI-SHL-066, UI-SHL-067, UI-SHL-068, UI-SHL-069, UI-SHL-070, UI-SHL-071 |
| APP-024 | Help menu | ADAPT | P2 (Additional setup… P3, Documentation P4) | UI-SHL-041 |
| APP-025 | Purchase license, Enter license, early-bird notice | DROP | P4 (recorded) | dropped: no licence system (UI-SHL-041 Help menu) |
| APP-026 | Debug menu | ADAPT | P4 (Show FPS in P1 as a developer-only flag) | UI-SHL-042, UI-SHL-099, UI-SHL-140 |
| APP-027 | Tab bar and tab animation | BUILD | P1 (animation P2) | UI-SHL-020, UI-SHL-021, UI-SHL-022, UI-SHL-023 |
| APP-028 | Filter box | BUILD | P1 | UI-FND-016, UI-SHL-024, UI-SHL-025, UI-SHL-124 |
| APP-029 | Status bar | ADAPT | P1 | UI-SHL-050, UI-SHL-051 |
| APP-030 | Keyboard Shortcuts window | BUILD | P2 | UI-SHL-095 |
| APP-031 | Keyboard shortcuts | ADAPT | P1 (Global, Table and ProcessTable keys), P2 (ServiceTable, Performance extras, RunDialog), P3 (Ctrl+Shift+M, History scrubber, Picker, Shift at launch) | UI-SHL-120, UI-SHL-122, UI-SHL-123, UI-SHL-124 |
| APP-032 | Settings registry (every setting) | ADAPT | P1 (store and the keys of P1 features), each other key with its feature | UI-SHL-060, UI-SHL-061, UI-SHL-062, UI-SHL-063, UI-SHL-064, UI-SHL-065, UI-SHL-066, UI-SHL-067, UI-SHL-068, UI-SHL-069, UI-SHL-070, UI-SHL-071 |
| APP-033 | Settings persistence | ADAPT | P1 | UI-SHL-002 |
| APP-034 | Import TaskSlinger settings and themes | ADAPT | P4 | UI-SHL-071 |
| APP-035 | Built-in themes and colour slots | BUILD | P1 (Default Dark, Default Light), P2 (the other 10) (R-068) | UI-FND-001, UI-FND-002, UI-FND-040, UI-FND-041, UI-SHL-038, UI-SHL-080, UI-SHL-081 |
| APP-036 | Theme menu and follow system light/dark | ADAPT | P2 | UI-FND-022, UI-FND-040, UI-FND-042, UI-SHL-038, UI-SHL-080, UI-SHL-084 |
| APP-037 | Theme editor | BUILD | P3 | UI-FND-043, UI-SHL-081 |
| APP-038 | Theme share and import codes | ADAPT | P3 | UI-FND-042, UI-SHL-082, UI-SHL-083 |
| APP-039 | Graph accent colours | BUILD | P3 | UI-FND-001, UI-SHL-085 |
| APP-040 | Appearance sliders | BUILD | P2 (Ctrl+Plus/Minus/0 with the global keys in P1) | UI-FND-003, UI-FND-011, UI-FND-014, UI-SHL-037, UI-SHL-130 |
| APP-041 | System Information (Mission Center dialog) | ADAPT | P2 | UI-SHL-041, UI-SHL-095, UI-SYS-083, UI-SYS-086, UI-SYS-088, UI-SYS-096 |
| APP-042 | Additional Setup window | ADAPT | P3 | UI-SHL-094 |
| APP-043 | Language and live locale change | ADAPT | P1 (key system and English), P4 (other locales) | UI-SHL-040, UI-SHL-061, UI-SHL-131 |
| APP-044 | Toasts | BUILD | P1 | UI-FND-020, UI-SHL-100, UI-PWN-003, UI-PWN-007, UI-PWN-028, UI-PWN-034, UI-PWN-035, UI-PWN-036 |
| APP-045 | Dialogs as separate pinnable windows | BUILD | P1 (frame), others with their features | UI-FND-021, UI-SHL-060, UI-SHL-081, UI-SHL-082, UI-SHL-090, UI-SHL-091, UI-SHL-092, UI-PWN-001, UI-PWN-025 |
| APP-046 | Rendering GPU choice | ADAPT | P4 | UI-SHL-040, UI-SHL-071 |
| APP-047 | Main tray icon and close/minimize to tray | ADAPT | P3 | UI-SHL-005, UI-SHL-113 |
| APP-048 | Start at login | ADAPT | P2 | UI-SHL-002, UI-SHL-040, UI-SHL-069, UI-SHL-110 |
| APP-049 | Privilege settings (unlock at launch, passwordless) | ADAPT | P3 (unlock at launch), P4 (passwordless) | UI-SHL-011, UI-SHL-040, UI-SHL-070, UI-SHL-091 |
| APP-050 | Updater, installer and related items | DROP | P4 (recorded) | dropped: no updater or installer (UI-SHL-040) |
| APP-051 | Open tasklens with Ctrl+Esc (KDE) | ADAPT | P4 | UI-SHL-069 |
| APP-052 | Command-line arguments | ADAPT | P1 (`--tab`, `--select-pid`, `--version`, `--help`, `--bench`), P2 (`--start-minimized`), P3 (`--low-memory`, `--watch-memory`, `--force-full-memory-mode`), P4 (`--app-id`, `--debug`) | UI-SHL-110, UI-SHL-112 |
| APP-053 | About window | ADAPT | P2 | UI-SHL-092 |
| APP-054 | Logging | ADAPT | P1 | UI-SHL-092, UI-SHL-093 |
| APP-055 | Crash handling and fatal errors | ADAPT | P1 | UI-SHL-093, UI-SHL-114 |
| APP-056 | Public IP fetch notice | BUILD | P2 | UI-SHL-071, UI-SHL-096 |
| APP-057 | NVIDIA G-SYNC profile | DROP | P4 (recorded) | dropped: KWin applies VRR to fullscreen windows only (UI-SHL-040) |
| APP-058 | Accessibility and reduced motion | ADAPT | P2 (scale-from-font default P4) | UI-FND-003, UI-FND-023, UI-FND-024, UI-SHL-121, UI-SHL-130, UI-SHL-132 |
| APP-059 | Loading placeholders | BUILD | P1 (frame rule), P2 (skeletons with the P2 tabs) | UI-FND-024, UI-FND-026, UI-SHL-110, UI-SHL-111, UI-SYS-004, UI-SYS-085 |

### Result

Every ID has a UI entry, an engine-only note or a dropped note. No ID lacks a UI entry that
should have one: the five engine-only PROC entries (002, 005, 010, 011, 052) have no surface of
their own and their effects are drawn by the entries named; PROC-037 has no UI by design; the five
dropped APP entries (007, 013, 025, 050, 057) follow the merged spec's verdicts.
