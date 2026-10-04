# Batch B8 ui-tables: bead plan for E22, E27, E28

This file lists every bead of batch B8: the process table page (E22), the shared system-table
frame with the Services page and Service Properties (E27), and the Startup Apps, Connections and
Information pages (E28). The rules are in `docs/research/brief-beads.md` and
`docs/plan/00-epics.md`. Epic and anchor bead IDs come from `docs/plan/anchors.md`.

Conventions used in every bead of this batch:

- Bead priority equals the phase number (P1 = 1 and so on). The E22-A1 anchor is P0.
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, plus `ui`, `security` or `perf-gate`
  where they apply.
- Cross-batch dependencies point only at anchor beads. Dependencies inside the batch may point at
  any bead of the batch.
- Each epic section has the plan-key to bead-ID table, then the open items for the leader.

## E22 ui-processes-tab (tasklens-pdu)

47 beads: the three anchors and 44 others. The P0 beads build the page, delegate, cells, Name cell, header totals and query state that the phase-0 `table-2000` scene measures. P1 adds the row highlight, tooltips, confirmations, toasts, action handlers, copy commands, row menu, jump target, filter binding, toolbar, Ctrl-pause, status text, keys and the performance gate. P2 to P4 add pseudo rows, window features, app actions, the extra priority items, two menu items whose handlers E23 owns, and the later columns. Chain edits into other batches' files: E22-010 (`pages/mod.rs`), E22-011 and E22-023 (`window/main.rs`), E22-027 (`app.rs`). Each bead appends its strings to `assets/locales/en-US.ftl` itself. Beads that edit the same E22 file are chained by dependencies so two agents never edit one file at once.

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E22-001 | tasklens-pdu.5 | Processes page Actions and captured target types | P0 | E22-010, E16-A1 (tasklens-fut.1), E18-A1 (tasklens-rsf.1) |
| E22-002 | tasklens-pdu.6 | Process column registry: ids, labels, groups, kinds, widths | P0 | E22-010, E18-A1 (tasklens-rsf.1) |
| E22-003 | tasklens-pdu.7 | Process cell values and table (d) formatting | P0 | E22-002 |
| E22-004 | tasklens-pdu.8 | Process cell painting: heat, validity glyphs, stale, selection text | P0 | E22-003, E15-A1 (tasklens-ug9.1), E15-A2 (tasklens-ug9.2) |
| E22-005 | tasklens-pdu.9 | Process Name cell: indent, chevron, icon, text, state glyphs | P0 | E22-010, E15-A1 (tasklens-ug9.1), E18-A1 (tasklens-rsf.1) |
| E22-006 | tasklens-pdu.14 | Tree connector lines in the Name cell | P1 | E22-005, E22-009 |
| E22-007 | tasklens-pdu.10 | Process header line 2 system totals | P0 | E22-003, E18-A2 (tasklens-rsf.2) |
| E22-008 | tasklens-pdu.11 | Process table query state, generations and expansion | P0 | E22-010, E16-A1 (tasklens-fut.1), E18-A1 (tasklens-rsf.1), E22-002 |
| E22-009 | tasklens-pdu.12 | ProcessDelegate: TreeTableDelegate for the process table | P0 | E22-004, E22-005, E22-007, E22-008, E22-001, E18-A1 (tasklens-rsf.1), E18-A2 (tasklens-rsf.2) |
| E22-A1 | tasklens-pdu.1 | Process table renders: ProcessesPage with the process delegate | P0 | E22-009, E22-001, E16-A1 (tasklens-fut.1) |
| E22-010 | tasklens-pdu.4 | E22: add processes module to pages/mod.rs | P0 | E16-A1 (tasklens-fut.1) |
| E22-011 | tasklens-pdu.13 | E22: add Processes page arm to window/main.rs | P0 | E22-A1, E20-A1 (tasklens-jqk.1) |
| E22-012 | tasklens-pdu.15 | New and exited row highlight with linger and fade | P1 | E22-006, E22-004, E19-A1 (tasklens-yrf.1), E16-A2 (tasklens-fut.2) |
| E22-013 | tasklens-pdu.16 | Process row hover tooltip | P1 | E22-012, E15-A1 (tasklens-ug9.1) |
| E22-014 | tasklens-pdu.17 | Process cell tooltips | P1 | E22-013 |
| E22-015 | tasklens-pdu.18 | Process action confirmation dialogs | P1 | E22-010, E16-A2 (tasklens-fut.2) |
| E22-016 | tasklens-pdu.19 | Process action toast texts and request submission | P1 | E22-010, E22-001, E16-A2 (tasklens-fut.2), E16-A3 (tasklens-fut.3) |
| E22-017 | tasklens-pdu.20 | End Task, End Process and End Process Tree handlers | P1 | E22-A1, E22-015, E22-016 |
| E22-018 | tasklens-pdu.21 | Suspend, Resume and Set Priority nice-class handlers | P1 | E22-017 |
| E22-019 | tasklens-pdu.22 | Process copy commands: Copy, Copy details, Copy all metrics, Copy rows as table | P1 | E22-018, E22-014 |
| E22-A2 | tasklens-pdu.2 | Process row context menu with the P1 items | P1 | E22-019, E22-014, E16-A3 (tasklens-fut.3), E22-001 |
| E22-A3 | tasklens-pdu.3 | Jump target: ProcessesPage::select_by_key | P1 | E22-A1, E22-019, E16-A2 (tasklens-fut.2), E20-A1 (tasklens-jqk.1) |
| E22-020 | tasklens-pdu.23 | Go to parent process and Open file location | P1 | E22-A3, E22-A2 |
| E22-021 | tasklens-pdu.24 | Process filter binding, Include children and no-match state | P1 | E22-020, E20-A2 (tasklens-jqk.2), E15-A2 (tasklens-ug9.2) |
| E22-022 | tasklens-pdu.25 | Processes toolbar buttons: Hide system processes and view-mode cycler | P1 | E22-A1, E20-A1 (tasklens-jqk.1), E15-A1 (tasklens-ug9.1) |
| E22-023 | tasklens-pdu.26 | E22: add Processes toolbar registration to window/main.rs | P1 | E22-022, E22-011 |
| E22-024 | tasklens-pdu.27 | Hold Ctrl to pause the process table | P1 | E22-021 |
| E22-025 | tasklens-pdu.28 | Processes status chip text | P1 | E22-024, E20-A1 (tasklens-jqk.1) |
| E22-026 | tasklens-pdu.29 | ProcessTable key bindings for End Task and End Process | P1 | E22-025, E22-017 |
| E22-027 | tasklens-pdu.30 | E22: add bind_keys_processes call to app.rs | P1 | E22-026, E16-A1 (tasklens-fut.1) |
| E22-028 | tasklens-pdu.31 | Process table meets the table-2000 performance gate | P1 | E22-A1, E22-012, E22-013, E22-024, E01-A1 (tasklens-wlf.1) |
| E22-029 | tasklens-pdu.32 | Dim zero values in process cells | P2 | E22-019 |
| E22-030 | tasklens-pdu.33 | Subtree totals prefix on parent process rows | P2 | E22-029 |
| E22-031 | tasklens-pdu.34 | Pseudo rows: Interrupts and Memory compression (zram) | P2 | E22-030, E22-024, E22-006 |
| E22-032 | tasklens-pdu.35 | Unit row Description and Frozen state | P2 | E22-031 |
| E22-033 | tasklens-pdu.36 | App row window title as secondary text | P2 | E22-032 |
| E22-034 | tasklens-pdu.37 | Name-cell window button and window popup | P2 | E22-033, E22-026 |
| E22-035 | tasklens-pdu.38 | Freeze App, Thaw App and Kill App | P2 | E22-034 |
| E22-036 | tasklens-pdu.39 | Set Priority Realtime and Idle, Set I/O Priority submenu | P2 | E22-035 |
| E22-037 | tasklens-pdu.40 | Show Window, Restart Plasma Shell or unit, File properties | P2 | E22-036, E22-020 |
| E22-038 | tasklens-pdu.41 | Open in VirusTotal row-menu item | P2 | E22-037 |
| E22-039 | tasklens-pdu.42 | Create dump file row-menu item | P3 | E22-038 |
| E22-040 | tasklens-pdu.43 | Retry as administrator on permission-denied toasts | P3 | E22-039, E22-016 |
| E22-041 | tasklens-pdu.44 | Clickable restricted count and Administrator mode badge | P3 | E22-040, E22-025 |
| E22-042 | tasklens-pdu.45 | Offer phase-2 process columns | P2 | E22-033 |
| E22-043 | tasklens-pdu.46 | Offer phase-3 process columns | P3 | E22-042 |
| E22-044 | tasklens-pdu.47 | Offer phase-4 process columns | P4 | E22-043 |
| E22-045 | tasklens-pdu.48 | Window-pick result on the Processes page: select, Picked dialog, toasts (R-239) | P3 | E20-027 (tasklens-jqk.29), E22-A1, E22-A3, E22-015, E22-017, as2.16 |

### Open items

1. **Model fields E22 reads that ADR-003 and ADR-009 do not define.** `TableRow` has no descendant count for "PID +N" (PROC-018); `ProcessRecord` has no time stamp for when NEW or EXITED was set (UI-PRC-011 `flag_since_ns`); `ProcessRecord` lacks fields for several P1 columns (start_time, cpu_time and its kernel and user parts, cpu_kernel, cpu_user, cpu_relative, ws_peak, ws_shared, commit, vsize, vsize_peak, page_faults, page_faults_delta, priority policy, affinity, handles, unit) and for Copy details "Private bytes"; `GroupRow`, `TableTotals` and the `ProcessTableView` counts (total, shown, restricted) have no field list; the snapshot has no capability bits for FileManager1, gdb, helper installed, the block scheduler name and unit MainPID. Recommendation: the E03 writer adds these to `tl-model` in E03-A1 and E03-A2. The E22 beads read "the E03 field of that meaning" and stop and report when it is missing.
2. **Name-cell geometry.** The UI-PRC-002 app-row sketch (chevron 26..42, icon 48..72, text 78) and the unit-row sketch (icon 52..68, text 74) differ by 4 px, and the UI-PRC-003 test formula omits the gap after the icon. Recommendation: gap 6 after the chevron and gap 6 after the icon for every row kind (E22-005).
3. **Picker group of each column.** PROC-024 gives the 13 headings and the Objects members only. Recommendation: the mapping in E22-002 (each PROC section to its heading; Start time, Type and the package and file columns under Image; User, PID, Parent PID, Session under Identity).
4. **How "a phase has not shipped" is decided in code.** Recommendation: a `SHIPPED_PHASE` constant in `pages/processes/delegate.rs` for columns, raised by E22-042 to E22-044; menu items appear when the bead that adds them lands.
5. **Two dependencies outside the E22 row of 00-epics.md section 4.** E22-012 needs E19-A1 (`AnimationDriver` for the fade) and E22-028 needs E01-A1 (bench harness). Neither creates a cycle (E19 does not depend on E22; E01 depends on E22-A1, not on E22-028). Recommendation: add both to the E22 row. Both edges are already added.
6. **Reason text in partial-failure toasts for errors other than permission denied.** Only "permission denied" is given. Recommendation: the first sentence of `ActionTracker::message(&err)` in lower case without the full stop.
7. **Which confirmation wins when several rules match.** Only "protected wins over tree" is given. Recommendation: Protected, then End tree, then Kill app, then several targets, then another user's process (E22-017).
8. **Source of the End Process Tree subtree and the bwrap "first descendant".** The "engine forest" is not in the snapshot. Recommendation: the UI walks `ProcessSnapshot.records` by ppid, matching parents by pid among non-exited records; for bwrap, the first non-bwrap descendant in breadth-first order by lowest PID.
9. **Copy details rounding, TSV glyph words, clipboard failure.** Recommendation: whole MiB rounded to nearest; TSV writes "restricted", "n/a" and empty for other glyphs; the "Failed to copy to clipboard" toast stays unused because GPUI `write_to_clipboard` cannot report failure.
10. **KWin activation has no result.** ADR-005 gives `KWinCommand` no result event, so "Process window shown." is shown on send and "No window found" only when the window has left `WindowsSnapshot`. Recommendation: accept, or add a KWin command result event in E03 and E14.
11. **Toolbar slot ownership.** UI-SHL-010 draws slots 2 and 3 inside `TitleBarView` from a `ToolbarModel`; 00-epics.md gives E22 `toolbar.rs` and a `ToolbarContent` registration. Recommendation: E20-A1 defines the `ToolbarContent` trait and E22 draws the two buttons (E22-022); E20 must not draw them too.
12. **Jump callers without their own "not visible" text** (`--select-pid`, D-Bus, picker). Recommendation: reuse "Process not found in current view" with the "Show" button (E22-A3 `JumpTexts` default).
13. **Which block device the I/O priority "Scheduler:" line names.** Recommendation: the device holding the root filesystem.
14. **Handlers for "Open in VirusTotal" and "Create dump file".** E22 adds the menu items (E22-038, E22-039); E23-A3's `notices.rs` holds `open_virustotal` and `start_dump`. Recommendation: the E23 writer adds one chain-edit bead registering both handlers in `pages/processes/mod.rs` (covered by the E23 row of the `mod.rs` sharing rule).
15. **Actions declared elsewhere.** `ToggleHideSystem`, `CycleViewMode`, `SetViewMode`, `FocusFilter`, `ClearFilter` and the type-to-filter routing are the shell's (INDEX.md, E20) even though UI-PRC-009 and UI-PRC-019 place them in `pages/processes/actions.rs`. E22 declares none of them and builds only the buttons. Recommendation: accept.
16. **Build rule wording.** The brief's `flock ... systemd-run ...` line is replaced in every bead by `heavy-build cargo ...`, because the owner's CLAUDE.md now says prompts must use `heavy-build` and not copy the long line.
17. **End Process on app and unit rows in P1.** The P1 menu hides it, and Kill App arrives in P2. Recommendation: Shift+Delete on an app or unit row does nothing in P1 (E22-017), then opens Kill App from E22-035.
18. **Fluent key slugs.** R-132 fixes `<area>-<entry>-<slug>` but not how the slug is formed. Recommendation: kebab-case of the first five words of the English text, which every bead uses.
19. **Scroll handle for the E01 `table-2000` scene (E01-OI-8).** E01-008 must scroll the production process table. E22-A1 (tasklens-pdu.1) exposes `ProcessesPage::scroll_handle(&self, cx: &App) -> UniformListScrollHandle`, a clone of E18-A1's `TlTreeTable::scroll_handle()`, next to `table()`, with the test `ui_processes::scroll_handle_drives_table`. E01-008 should call that method. Recommendation: accept; E18-A1 must expose `TlTreeTable::scroll_handle`.

## E27 ui-services (tasklens-hk6)

35 beads: the three anchors and 32 others. E27-A1 is the shared system-table frame (`sys_table.rs`, `sys_status.rs`, `sys_copy.rs` and the `SysActions` toasts) that the Services, Startup Apps and Connections tables sit on; E28 depends on it. E27-A2 is the finished Services page and E27-A3 is the Service Properties window frame with its open API (`ServicePropertiesWindow::open(unit, tab, cx)`). The tab contents (General, Log On, Recovery, Dependencies, Logs) are later beads that depend on E27-A3, so the anchor is not held up by them. The P3 beads are the Recovery drop-in editor and the Logs banner link; the P4 bead is the OpenRC variant. Chain edits into other batches' files: E27-001 (`pages/mod.rs`), E27-030 (`windows/mod.rs`), E27-022 (`app.rs`), E27-023 and E27-025 (`window/main.rs`), E27-024 (`pages/processes/menu.rs`, an E22 file). Each bead appends its strings to `assets/locales/en-US.ftl` itself. The Service Properties tab beads all touch `windows/service_properties/mod.rs` and the Logs beads all touch `logs.rs`, so they form one chain (E27-032 to E27-038, then E27-039 to E27-041) and two agents never edit one file at once.

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E27-001 | tasklens-hk6.4 | E27: add sys_table, sys_copy, sys_status and services mod lines to pages/mod.rs | P2 | E16-A1 (tasklens-fut.1) |
| E27-002 | tasklens-hk6.5 | System tables: status cell looks and painter (sys_status.rs) | P2 | E15-A1 (tasklens-ug9.1), E27-001 |
| E27-003 | tasklens-hk6.6 | System tables: SysQuery and SysPageState (query generation, demand, first visit) | P2 | E18-A1 (tasklens-rsf.1), E16-A1 (tasklens-fut.1), E20-A2 (tasklens-jqk.2), E27-001 |
| E27-004 | tasklens-hk6.7 | System tables: skeleton and empty or error body states | P2 | E20-A1 (tasklens-jqk.1), E15-A2 (tasklens-ug9.2), E27-003 |
| E27-005 | tasklens-hk6.8 | System tables: shared copy commands (sys_copy.rs) | P2 | E18-A1 (tasklens-rsf.1), E16-A2 (tasklens-fut.2), E27-001 |
| E27-006 | tasklens-hk6.9 | System tables: action toasts, pending map and summary toast (SysActions) | P2 | E16-A2 (tasklens-fut.2), E16-A3 (tasklens-fut.3), E27-003 |
| E27-A1 | tasklens-hk6.1 | E27-A1 System table frame | P2 | E20-A1 (tasklens-jqk.1), E18-A2 (tasklens-rsf.2), E27-006, E27-005, E27-004, E27-002 |
| E27-010 | tasklens-hk6.34 | Services: actions and bind_keys_services (ServiceTable, ServiceProps, JournalList) | P2 | E27-A1 |
| E27-011 | tasklens-hk6.10 | Services: ServicesDelegate skeleton, row kinds, columns, sort and column persistence | P2 | E18-A2 (tasklens-rsf.2), E20-A2 (tasklens-jqk.2), E27-010, E27-A1 |
| E27-012 | tasklens-hk6.11 | Services: group and unit row cells, icons, status overlay, tooltips | P2 | E22-A1 (tasklens-pdu.1), E27-011 |
| E27-013 | tasklens-hk6.12 | Services: child process rows and process demand | P2 | E22-A1 (tasklens-pdu.1), E27-012 |
| E27-014 | tasklens-hk6.13 | Services: filter strip and Unit types dropdown | P2 | E20-A2 (tasklens-jqk.2), E16-A3 (tasklens-fut.3), E27-010, E27-A1 |
| E27-016 | tasklens-hk6.14 | Services: Copy row block and copy source | P2 | E22-A2 (tasklens-pdu.2), E27-011 |
| E27-017 | tasklens-hk6.15 | Services: Start, Stop, Restart, Reload and Reset failed flows | P2 | E27-010, E27-A3, E27-A1 |
| E27-018 | tasklens-hk6.16 | Services: startup type flow, file openers and Jump to process | P2 | E22-A3 (tasklens-pdu.3), E27-017 |
| E27-019 | tasklens-hk6.17 | Services: unit row context menu and Startup type submenu | P2 | E16-A3 (tasklens-fut.3), E27-010, E27-011 |
| E27-020 | tasklens-hk6.18 | Services: inbound Go to service (ServicesPage::select_unit) | P2 | E27-A2 |
| E27-022 | tasklens-hk6.19 | E27: add bind_keys_services call to app.rs | P2 | E16-A1 (tasklens-fut.1), E27-010 |
| E27-023 | tasklens-hk6.20 | E27: add Services page arm and toolbar registration to window/main.rs | P2 | E20-A1 (tasklens-jqk.1), E27-A2 |
| E27-024 | tasklens-hk6.21 | E27: add Go to service item to pages/processes/menu.rs | P2 | E22-A2 (tasklens-pdu.2), E27-010 |
| E27-025 | tasklens-hk6.22 | E27: route GoToService in window/main.rs | P2 | E20-A1 (tasklens-jqk.1), E27-020 |
| E27-A2 | tasklens-hk6.2 | E27-A2 Services page | P2 | E22-A2 (tasklens-pdu.2), E20-A2 (tasklens-jqk.2), E27-019, E27-018, E27-016, E27-014, E27-013, E27-012, E27-A3, E27-A1 |
| E27-026 | tasklens-hk6.23 | Services: child rows open Process Properties | P2 | E27-013, E27-010, E27-A2, E23-A1 (tasklens-gma.1) |
| E27-030 | tasklens-hk6.24 | E27: add service_properties mod line to windows/mod.rs | P2 | E16-A1 (tasklens-fut.1) |
| E27-A3 | tasklens-hk6.3 | E27-A3 Service Properties | P2 | E16-A2 (tasklens-fut.2), E15-A1 (tasklens-ug9.1), E27-010, E27-030, E27-A1 |
| E27-032 | tasklens-hk6.25 | Service Properties: General tab | P2 | E27-A3, E27-018, E15-A2 (tasklens-ug9.2) |
| E27-033 | tasklens-hk6.35 | Service Properties: Log On tab | P2 | E27-032, E27-A3, E15-A2 (tasklens-ug9.2) |
| E27-034 | tasklens-hk6.26 | Service Properties: Recovery tab (P2 view) | P2 | E27-033, E27-A3, E15-A2 (tasklens-ug9.2) |
| E27-035 | tasklens-hk6.27 | Service Properties: Dependencies tab | P2 | E27-034, E27-A3, E15-A2 (tasklens-ug9.2) |
| E27-036 | tasklens-hk6.28 | Service Properties: Logs tab list, toolbar, filter, copy, Open in terminal | P2 | E27-035, E27-A3, E15-A2 (tasklens-ug9.2), E16-A1 (tasklens-fut.1) |
| E27-037 | tasklens-hk6.29 | Service Properties: Logs follow mode, new-lines pill, Load older, 10,000-line cap | P2 | E27-036 |
| E27-038 | tasklens-hk6.30 | Service Properties: Logs loading, empty, error and access states | P2 | E27-037, E15-A2 (tasklens-ug9.2) |
| E27-039 | tasklens-hk6.31 | Service Properties: Recovery drop-in editor (P3) | P3 | E27-038, E27-034, E16-A2 (tasklens-fut.2), E20-A1 (tasklens-jqk.1) |
| E27-040 | tasklens-hk6.32 | Service Properties: Logs banner Additional setup link (P3) | P3 | E27-039, E20-A1 (tasklens-jqk.1) |
| E27-041 | tasklens-hk6.33 | Services: OpenRC variant of the page, menu and columns (P4) | P4 | E27-040, E27-A2, E27-A3 |

Notes on the graph:

- E27-026 hangs off E27-A2 because it edits `pages/services/mod.rs`, which every Services bead before it also edits.
- E27-041 (P4) comes last in the chain because it edits the Services delegate, menu and page files.
- One dependency is outside the E27 row of 00-epics section 4: E27-026 on E23-A1 (tasklens-gma.1), see open item 9.

### Open items

1. **Resend rule when the engine is busy (E27-003).** No doc says what `SysQuery` does on `CommandError::Busy`. Recommendation: `last_sent` still becomes the new generation so older views are refused, `pending_resend = true`, and the next snapshot resends once with a fresh generation; on `CommandError::EngineStopped` nothing is resent.
2. **Clipboard failure detection (E27-005, E27-016).** GPUI `App::write_to_clipboard` returns `()`, so the "Could not copy" toasts can never fire. Recommendation: keep the texts behind a `clipboard_write(text) -> Result<(), ()>` wrapper that returns `Ok(())` today, and add no read-back check.
3. **Hooks needed from E16 and E11 (E27-006).** `SysActions` needs `ActionTracker` events by request id and a Progress toast without the auto-dismiss cap; the D-Bus error mapping assumes E11 reports the error name inside `ActionError::UnitJob`. Recommendation: E16-A3 and E16-A2 provide both hooks; E11 puts the D-Bus error name in `UnitJob`. E27 beads stop and report rather than edit E16 files.
4. **"Details" for a partial summary toast (E27-006).** UI-SYS-007 names the button but not what it opens. Recommendation: a `TlDialog` titled "tasklens" with one line per failed target, "<target label>: <failure text>", and one "Close" button.
5. **`ColumnId` constants for the Services columns (E27-011).** E03 lists the R-007 process columns only. Recommendation: E03-A2 adds the Services column ids; the E27 beads stop and report when they are missing.
6. **Summary texts for Reload, Reset failed and startup type changes (E27-017, E27-018).** R-114 gives the pattern for start, stop, restart, enable and disable only. Recommendation: extend the same pattern, as written in those beads.
7. **Leaving Masked (E27-018).** Recommendation: changing Masked to Enabled or Disabled sends `UnitEnable` or `UnitDisable`, and the engine unmasks first inside that request. The E11 writer confirms.
8. **Fixed `ToastKey` values (E27-A2).** No doc assigns fixed keys. Recommendation: E16-A2 reserves the range from `0x8000_0000_0000_0000` for fixed keys; `SERVICES_LOADING_TOAST = ToastKey(0x8000_0000_0000_0101)`.
9. **E27-026 depends on E23-A1.** Opening Process Properties from a Services child row needs E23's window. E23 does not depend on E27, so the edge makes no cycle. Recommendation: accept and add E23-A1 to the E27 row of 00-epics section 4. The edge is added.

## E28 ui-startup-connections-info (tasklens-9o0)

E28 builds the Startup Apps, Connections and Information tabs with their Properties windows and the startup delete confirmation. Startup Apps and Connections sit on E27's `SysTableFrame` (E27-A1); Information is its own two-column page of `InfoSection` entities. Each anchor is the bead that finishes its page and mounts it in `MainWindow`, so a dependency on an anchor means "the whole P2 page works". The chain edits into other batches' files (`pages/mod.rs`, `windows/mod.rs`, `app.rs`, `window/main.rs`) are separate beads or the anchors' named part; Fluent keys are appended to `assets/locales/en-US.ftl` by the bead that uses them (R-132). Every bead tells the implementer to run cargo through `heavy-build` (the current CLAUDE.md rule, which replaced the long `flock` line).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E28-001 | tasklens-9o0.4 | E28: add startup, connections and information mod lines to pages/mod.rs | P2 | E16-A1 |
| E28-002 | tasklens-9o0.5 | E28: add startup and connection window mod lines to windows/mod.rs | P2 | E16-A1 |
| E28-010 | tasklens-9o0.6 | Startup Apps: StartupDelegate columns, cells, impact and tooltips | P2 | E28-001, E18-A1, E18-A2, E27-A1, E15-A1, E15-A2, E16-A1 |
| E28-011 | tasklens-9o0.7 | Startup Apps: StartupPage entity, status text, refresh and actions | P2 | E28-010, E20-A1, E20-A2, E16-A2 |
| E28-012 | tasklens-9o0.8 | Startup Apps: row context menu and Delete key | P2 | E28-011, E16-A3 |
| E28-013 | tasklens-9o0.9 | Startup Apps: Enable, Disable and Run now flows | P2 | E28-012, E16-A3 |
| E28-014 | tasklens-9o0.10 | Startup Apps: delete confirmation window and delete flow | P2 | E28-013, E28-002, E16-A2 |
| E28-015 | tasklens-9o0.11 | Startup Apps: copy commands and Open file location | P2 | E28-014 |
| E28-016 | tasklens-9o0.12 | Startup Apps: Startup Properties window | P2 | E28-015, E28-002, E16-A2, E15-A2 |
| E28-003 | tasklens-9o0.13 | E28: add Startup Apps bind_keys call and page registration to app.rs | P2 | E16-A1, E28-011 |
| E28-A1 | tasklens-9o0.1 | E28-A1 Startup Apps page | P2 | E28-016, E28-003, E20-A1 |
| E28-020 | tasklens-9o0.14 | Connections: ConnectionsDelegate columns, cells and tooltips | P2 | E28-001, E18-A1, E18-A2, E27-A1, E15-A1, E15-A2, E16-A1 |
| E28-021 | tasklens-9o0.15 | Connections: ConnectionsPage entity, status text, refresh and actions | P2 | E28-020, E20-A1, E20-A2, E16-A2 |
| E28-022 | tasklens-9o0.16 | Connections: row context menu (P2 items) | P2 | E28-021, E16-A3 |
| E28-023 | tasklens-9o0.17 | Connections: copy commands | P2 | E28-022 |
| E28-024 | tasklens-9o0.18 | Connections: Jump to process and Go to service | P2 | E28-023, E22-A3, E27-A2 |
| E28-025 | tasklens-9o0.19 | Connections: Connection Properties window | P2 | E28-024, E28-002, E16-A2, E15-A2 |
| E28-004 | tasklens-9o0.20 | E28: add Connections bind_keys call and page registration to app.rs | P2 | E16-A1, E28-021 |
| E28-A2 | tasklens-9o0.2 | E28-A2 Connections page | P2 | E28-025, E28-004, E20-A1, E28-A1 |
| E28-026 | tasklens-9o0.28 | Connections: Close TCP connection flow (P3) | P3 | E28-A2, E29-A2, E16-A2 |
| E28-027 | tasklens-9o0.29 | Connections: Closable here reasons in Connection Properties (P3) | P3 | E28-026 |
| E28-028 | tasklens-9o0.30 | Connections: Remote host column (P4) | P4 | E28-027 |
| E28-029 | tasklens-9o0.31 | Connections: Namespaces control and Namespace column (P4) | P4 | E28-028, E29-A2 |
| E28-030 | tasklens-9o0.21 | Information: InfoSection entity, row types and page actions | P2 | E28-001, E15-A1, E15-A2, E16-A1, E16-A3 |
| E28-031 | tasklens-9o0.22 | Information: row builders for OS, Processor, System, BIOS and Motherboard | P2 | E28-030, E03-A2 |
| E28-032 | tasklens-9o0.23 | Information: row builders for Memory, modules, disks, graphics and network | P2 | E28-031 |
| E28-033 | tasklens-9o0.24 | Information: InformationPage layout, loading, live refresh and keys | P2 | E28-032, E20-A1, E16-A2, E15-A2 |
| E28-034 | tasklens-9o0.25 | Information: filter | P2 | E28-033, E20-A2 |
| E28-035 | tasklens-9o0.26 | Information: copy section, copy all and Ctrl+C | P2 | E28-034 |
| E28-005 | tasklens-9o0.27 | E28: add Information bind_keys call and page registration to app.rs | P2 | E16-A1, E28-035 |
| E28-A3 | tasklens-9o0.3 | E28-A3 Information page | P2 | E28-035, E28-005, E20-A1, E28-A2 |
| E28-036 | tasklens-9o0.32 | Information: Public IP row (blocked on open item B8-E28-1) | P2 | E28-A3, E25-A1 |
| E28-037 | tasklens-9o0.33 | Information: Needs setup links and shield tooltips (P3) | P3 | E28-036, E29-A2, E29-A3 |

Notes on the graph:

- E28-A2 depends on E28-A1 and E28-A3 on E28-A2 only to serialize the three chain edits of `window/main.rs`.
- E28-036 hangs off E28-A3 so the anchor is not held up by open item 1.
- Dependencies outside the E28 row of 00-epics section 4 (all acyclic, because none of these epics depends on E28): E27-A2 (Go to service entry, E28-024), E29-A2 (helper state and session, E28-026, E28-029, E28-037), E29-A3 (Additional Setup window, E28-037), E25-A1 (shared Public IP result, E28-036), E03-A2 (inventory types, E28-031).

### Open items

1. **Public IP fetch entry point and result (blocks E28-036).** PERF-060 and UI-SYS-095 say the Information row shares the Network page's single fetch, but no doc names the `Command`, `SourceId` or snapshot field, and `SourceId` has no `PublicIp`. Recommendation: `Command::FetchPublicIp` handled by E12's `workers/public_ip.rs`; result in `SystemSnapshot.public_ip: PublicIpState { NotFetched, Fetching, Fetched { v4, v6, at }, Failed(Arc<str>) }`; one UI helper in E25's network page (`request_public_ip_fetch(cx)`) that shows the notice window first; E28-036 calls it.
2. **`InventorySnapshot` fields are not written anywhere.** ADR-003 names the component only. E28-031 and E28-032 need one field per Information row. Recommendation: E03-A2 defines per-section structs (os, cpu, system, memory, modules, bios, board, gpus, displays) where each field is a value with its `Validity` and optional reason, covering every row of UI-SYS-086 to UI-SYS-095 (OS logo path, package manager, desktop, logind class and type, `XDG_VTNR`, firmware type, Secure Boot, DMI fields with the chassis number, modules without any serial field).
3. **Owner of "Peak committed (since tasklens started)".** PERF-098 says tasklens tracks the maximum it saw; the UI only sees memory while something demands it. Recommendation: the engine reducer keeps the maximum `Committed_AS` over every memory sample since engine start and publishes it as `MemoryTotals.committed_peak`.
4. **"(64-bit)" in the Architecture row.** No rule maps the machine string to the bit width. Recommendation: 64 for x86_64, aarch64, riscv64, ppc64, ppc64le, s390x, loongarch64, mips64; else 32; computed by the engine.
5. **Session type wording.** UI-SYS-088 gives "user (graphical)" from logind Class and Type, but Type values are x11, wayland, mir, tty, unspecified. Recommendation: "<Class> (graphical)" for x11, wayland and mir, "<Class> (text)" for tty, the raw Type otherwise.
6. **Help action name.** shell.md UI-SHL-041 writes `OpenSystemInformation`; docs/ui/INDEX.md and UI-SYS-096 write `ShowSystemInformation`. Recommendation: `ShowSystemInformation` (INDEX vocabulary); correct the shell.md line. E28-A3 uses whatever E20-A1 declares.
7. **Information skeleton shape.** shell.md's startup text says "4 section blocks 120 tall"; UI-SYS-085 says every section shows its band and an 8-line skeleton block. Recommendation: UI-SYS-085 (the owning entry); correct shell.md. E28-033 follows UI-SYS-085.
8. **When " | N selected" appears** in the Startup Apps and Connections status text. SYS-027 and SYS-036 do not say; the Processes text uses 2 or more. Recommendation: 2 or more, as in Processes. E28-011 and E28-021 use that.
9. **Run now "Already running".** SYS-023 wants an Info toast "Already running", but no `ActionState` or `ActionError` can carry it. Recommendation: add `ActionError::AlreadyRunning` (E03-A2, set by E11's startup action when the autostart unit is already active); the UI maps it to the Info toast.
10. **Startup and connection action texts, and no-Progress toasts.** The docs give no busy, unknown or several summary strings, and UI-SYS-045 and UI-SYS-046 want no pending toast while E27's `SysActions` always pushes one. Recommendation: busy "A startup action is already in progress." and "A connection action is already in progress."; unknown "Outcome unknown" (PROC-058); summaries following UI-SYS-007 ("{ $n } startup entries disabled", "{ $n } startup entries launched", "{ $n } startup entries deleted", "{ $n } TCP connections closed" and the matching "{ $ok } of { $n } … { $failed } failed." forms); and in `SysActions` an empty `pending_one` means no Progress toast (needs a one-line change in an E27 frame bead).
11. **`ColumnId` for system-table columns.** E03 writes "every column string from R-007", which covers process columns. The startup ids (`name`, `publisher`, `status`, `impact`, `type`, `location`, `command`) and connection ids (`proto`, `local`, `foreign`, `state`, `pid`, `process`, `user`, `family`, `local_port`, `foreign_port`, `recv_q`, `send_q`, `interface`, `inode`, `service`, `remote_host`, `netns`) overlap process names. Recommendation: E03-A2 adds these ids to `ColumnId`; `[columns.startup]` and `[columns.connections]` keep per-table lists, so a shared id like `name` is fine.
12. **Startup entry fields for Properties and copy.** UI-SYS-042's field list lacks what UI-SYS-048 and UI-SYS-050 show. Recommendation: E03-A2 adds `resolved_path`, `file_exists`, `state_code` (generated unit ActiveState and SubState), `source` word, `scope`, `only_show_in`, `not_show_in`, `try_exec`, `autostart_phase`, `generated_unit`, `overrides` to the startup entry, filled by E06 and E14.
13. **Connection record display fields.** UI-SYS-062's record lacks the user name (from uid), interface name, address text without port, port numbers, the `TcpInfoView` fields (rtt, rttvar, congestion window, bytes acked and received, retransmits, last data received), the P4 remote host name with pending state, and the P4 namespace name. Recommendation: E03-A2 adds them, formatted once per cookie by the engine like the address strings.
14. **Copy block for kernel-owned and unresolved sockets.** UI-SYS-066 does not say. Recommendation: "PID: -" and "Process: No owner (kernel)"; "PID: ?" and "Process: Restricted". E28-023 uses this.
15. **Extra cross-epic dependencies** listed above (E27-A2, E29-A2, E29-A3, E25-A1, E03-A2). Recommendation: accept and add them to the E28 row of 00-epics section 4.
