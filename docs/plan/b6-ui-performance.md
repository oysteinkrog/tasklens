# Batch B6 ui-performance: bead plan for E19, E24, E25, E26

This file lists every bead of batch B6: the chart element (E19), the Performance tab frame with
the sidebar and the CPU page (E24), the device pages with the SMART and eject windows (E25), and
the minigraph panel, detached graph windows and mini window (E26). The rules are in
`docs/research/brief-beads.md` and `docs/plan/00-epics.md`. Epic and anchor bead IDs come from
`docs/plan/anchors.md`.

Conventions used in every bead of this batch:

- Bead priority equals the phase number. The P0 anchors (E19-A1, E19-A2, E24-A1) and the beads
  they depend on are priority 0.
- Labels: `phase-pN`, `crate-tl-ui`, `area-<epic slug>`, plus `ui` when it renders and
  `perf-gate` when it carries an ADR-014 gate. No bead in this batch touches privileges.
- Cross-batch dependencies point only at anchor beads in `anchors.md`. Dependencies inside the
  batch may point at any bead of the batch.
- Every bead that introduces user-visible strings appends its Fluent keys to
  `assets/locales/en-US.ftl` (chain edit, R-132) in the same commit; the bead's Files section says
  so where it applies.
- Every bead's Notes carry the build rule from the brief.

## E19 ui-chart (epic `tasklens-yrf`)

Files owned: `crates/tl-ui/src/chart/{mod,element,view,geometry,grid,scale,labels,hover,grid_element}.rs`,
`crates/tl-ui/src/anim.rs`, `crates/tl-ui/tests/{ui_chart,chart_geometry,ui_anim,render_chart}.rs`.
Chain edits: two `pub mod` lines in `crates/tl-ui/src/lib.rs` (E16-A1), four golden PNGs under
`crates/tl-ui/tests/golden/` (E15-A3).

`chart/grid.rs` holds the grid quads and `chart/grid_element.rs` the `ChartGridElement`, following
the file list in `00-epics.md` section 2 (UI-PRF-002 names `grid.rs` for both).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E19-001 | tasklens-yrf.3 | Chart data types, `ChartSpec`, `ChartCache`, `ChartView` skeleton | P0 | E03-A1, E16-A1, E17-A1 |
| E19-002 | tasklens-yrf.4 | Geometry 1: time mapping, runs, decimation, stale rule | P0 | E19-001 |
| E19-003 | tasklens-yrf.5 | Geometry 2: fill and stroke paths, stale sets, gaps, merge | P0 | E19-002 |
| E19-004 | tasklens-yrf.6 | Y scale rules and axis rounding | P0 | E19-001, E03-A1 |
| E19-005 | tasklens-yrf.7 | `AnimationDriver`, `FrameStats`, stall detection | P0 | E16-A1 |
| E19-006 | tasklens-yrf.8 | Grid quads, baseline, outline, dotted gaps | P0 | E19-002, E17-A1, E15-A1 |
| E19-007 | tasklens-yrf.9 | `ChartElement` paint plan | P0 | E19-003, E19-004, E19-005, E19-006 |
| E19-008 | tasklens-yrf.10 | `ChartView` hub binding, series lookup, derived state | P0 | E19-007, E16-A1 |
| E19-A1 | tasklens-yrf.1 | Anchor: chart element complete with tests | P0 | E19-008, E19-004, E19-005 |
| E19-009 | tasklens-yrf.11 | Chart labels as cached `ShapedLine`s | P0 | E19-A1, E15-A1 |
| E19-010 | tasklens-yrf.12 | `ChartGridElement` and `ChartGridView` | P0 | E19-009 |
| E19-A2 | tasklens-yrf.2 | Anchor: grid element complete, heading tooltips, exports | P0 | E19-010 |
| E19-011 | tasklens-yrf.13 | Headless render goldens | P1 | E19-A2, E15-A3 |
| E19-012 | tasklens-yrf.14 | Overlaid and stacked multi-series charts | P2 | E19-A1 |
| E19-013 | tasklens-yrf.15 | 5 and 10 minute ranges read the 1 Hz tier | P2 | E19-A1 |
| E19-014 | tasklens-yrf.16 | Hover readout | P2 | E19-A2, E15-A1 |
| E19-015 | tasklens-yrf.17 | Hover readout on grid cells | P2 | E19-014 |
| E19-016 | tasklens-yrf.18 | Curved lines hook | P4 | E19-A1 |

18 beads. The anchor title "E19-A2 Grid element, labels, hover" covers the P0 grid and labels;
the P2 hover readout is E19-014 and E19-015, which depend on it.

### E19 open items

1. **`MetricId` constants the pages need are not in ADR-003.** The chart draws series the UI
   files name but ADR-003's frozen list does not define: CPU power package and core, CPU maximum
   core clock, memory dirty bytes, GPU shared memory, GPU memory clock, GPU engines beyond
   `GPU_UTIL`/`GPU_ENC`/`GPU_DEC` (memory controller, JPEG, optical flow, AMD and Intel engines),
   GPU PCIe RX and TX, GPU fan, fan PWM, battery power. Recommendation: E03-A1 adds the constants
   (one number each, appended after the existing ids so nothing is renumbered) and the E12 and E13
   workers publish them. The E24 and E25 beads name the series by their UI-PRF names and say
   "constant from E03-A1"; a bead whose constant is missing when it starts stops and reports.
2. **The `TIER_1HZ` flag bit of `MetricId` has no value.** UI-PRF-001 decides that the 1 Hz tier
   is a second `SeriesView` whose `MetricId` differs by a flag bit, but no document gives the bit.
   Recommendation: bit 15 (`0x8000`) with `MetricId::with_tier_1hz()` and `is_tier_1hz()` in
   `tl-model` (E03-A1) and the reducer publishing both views from P2 (E09). E19-013 is blocked on
   this.
3. **`ceil_pow2` and `ceil_125` live in `tl-model::units`** per UI-PRF-006, but E03's scope text
   lists only the table (d) formats. Recommendation: confirm E03-A1 includes both functions
   (`ceil_125(1234) = 2000`, `ceil_125(60) = 100`, `ceil_125(0.3) = 0.5`, `ceil_pow2(3 MiB) =
   4 MiB`). If not, E19-004 defines them privately in `chart/scale.rs`; the bead says to stop and
   report rather than decide alone.
4. **Thread CPU time for `FrameStats`.** ADR-014 measures UI thread CPU per frame with
   `clock_gettime(CLOCK_THREAD_CPUTIME_ID)`, but `tl-ui`'s allowed dependencies (ADR-001) have no
   `rustix` or `libc`. Recommendation: `tl-model::time::thread_cpu_ns()` (tl-model already has
   `rustix` with the `time` feature), added by E03-A1. Until it exists E19-005 reports 0.
5. **Per-series validity signal.** `SeriesView` has no validity and `SourceHealth` is keyed by
   `SourceId`, so the chart cannot tell Failed from Restricted from Unsupported by itself.
   Resolution used in the beads: the owner page calls `ChartView::set_state` from the health of the
   source it knows; the chart derives only WarmingUp and Stale. No doc change needed; recorded so
   the E24 and E25 beads are consistent.

<!-- B6-GEN-START: E24, E25, E26 sections are generated by b6/genplan.py -->

## E24 ui-performance-frame (epic `tasklens-crw`)

Files owned: `crates/tl-ui/src/pages/performance/{mod,page,sidebar,card,summary,menu,actions,details,cpu,cpu_grid,cpu_details}.rs`, `crates/tl-ui/tests/{ui_performance,ui_sidebar,ui_cpu_page}.rs`. Chain edits: `pub mod performance;` in `pages/mod.rs` and `bind_keys_performance` in `app.rs` (E16-A1), the `TabId::Performance` arm in `window/main.rs` (E20-A1).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E24-001 | tasklens-crw.3 | DeviceKey, DeviceKind, sidebar order and visibility rules, PerformancePage skeleton (pages/performance/mod.rs) | P0 | E16-A1,E03-A1,E03-A2 |
| E24-002 | tasklens-crw.4 | every Perf* action, bind_keys_performance and the app.rs registration (pages/performance/actions.rs) | P0 | E24-001,E20-A1,E16-A1 |
| E24-003 | tasklens-crw.5 | DeviceCard: layout, per-kind heading and lines, states (pages/performance/card.rs) | P0 | E24-001,E19-A1,E15-A1,E15-A2,E17-A1 |
| E24-004 | tasklens-crw.6 | DeviceSidebar entity: cards, charts, selection, keys, scroll, demand (pages/performance/sidebar.rs) | P0 | E24-003,E24-002,E16-A1 |
| E24-005 | tasklens-crw.7 | Details grid model: KvRow, fallback words, tooltips, change-only formatting (pages/performance/details.rs) | P0 | E24-001,E15-A2,E03-A1 |
| E24-006 | tasklens-crw.8 | DevicePage trait and DevicePageFrame: header, toggles, graph area, Details, blocks, page builder (pages/performance/page.rs) | P0 | E24-005,E15-A1,E17-A1 |
| E24-007 | tasklens-crw.9 | CPU per-core grid wrapper: cell specs from topology, headings, tooltips, offline cells (pages/performance/cpu_grid.rs) | P0 | E24-006,E19-A2 |
| E24-008 | tasklens-crw.10 | CPU Details grid, P1 rows 1 to 9 and 11 to 14 (pages/performance/cpu_details.rs) | P0 | E24-005,E03-A1,E03-A2 |
| E24-009 | tasklens-crw.11 | CpuPage entity: header toggles, Overall and Per core modes, kernel toggle, demand, bench layout (pages/performance/cpu.rs) | P0 | E24-007,E24-008,E19-A1 |
| E24-A1 | tasklens-crw.1 | Performance tab with CPU page | P0 | E24-009,E24-004,E24-006,E20-A1,E20-A2 |
| E24-010 | tasklens-crw.12 | sidebar hotplug: insert and remove animations, re-plug gap, saved slot | P1 | E24-A1 |
| E24-011 | tasklens-crw.13 | card context menu (Select Pages, Hide This Page, Show This Page, tray item slot) (pages/performance/menu.rs) | P1 | E24-A1,E16-A3,E16-A2 |
| E24-012 | tasklens-crw.14 | page context menu: Copy, Graph Summary View, Time range, page items slot (pages/performance/menu.rs) | P1 | E24-011 |
| E24-A2 | tasklens-crw.2 | Sidebar behaviour | P1 | E24-010,E24-011,E24-012 |
| E24-013 | tasklens-crw.15 | sidebar edit mode: header, groups, empty states, switches, Enable All, Disable All, Reset to Default | P2 | E24-A2,E15-A1,E16-A2 |
| E24-014 | tasklens-crw.16 | edit mode drag reorder within a group, drop indicator, Alt+Up and Alt+Down, device_order persistence | P2 | E24-013 |
| E24-015 | tasklens-crw.17 | Graph summary mode: SummaryView tiles, key context, Esc and Ctrl+M, title suffix (pages/performance/summary.rs) | P2 | E24-A2,E19-A1,E20-A1 |
| E24-016 | tasklens-crw.18 | Copy device info (Ctrl+C, menu Copy): text format, CPU page copy_text, toast | P2 | E24-A2,E16-A2 |
| E24-017 | tasklens-crw.19 | "View >" jump submenu and 5 and 10 minute ranges in the page menu | P2 | E24-012,E19-013 |
| E24-018 | tasklens-crw.20 | CPU "All cores overlaid" and "All cores stacked" modes with cross-fade and the kernel button rule | P2 | E24-A1,E19-012 |
| E24-019 | tasklens-crw.21 | per-core core type marks (P and E icons, preferred-core star) with tooltips | P2 | E24-A1 |
| E24-020 | tasklens-crw.22 | CPU Details P2 rows: Power draw, Virtualization, Running in VM, Security features, Hypervisor, cpufreq rows, Microcode | P2 | E24-A1 |
| E24-021 | tasklens-crw.23 | CPU secondary graph: Power draw, Clock speed, Temperature, fallback rule and the menu | P2 | E24-A1,E19-A1 |
| E24-022 | tasklens-crw.24 | Details value selection fallback: click selects, Ctrl+C copies the value, Esc clears, "Copy value" context item (U-46) | P1 | E24-A1,E16-A3 |
| E24-023 | tasklens-crw.25 | "Add tray icon graph" items on the card and page menus (P3 slot) | P3 | E24-A2 |
| E24-024 | tasklens-crw.26 | aggregate sidebar cards and group (P4) | P4 | E24-A2 |
| E24-025 | tasklens-crw.27 | disconnected-fan visibility rule in the sidebar (0 RPM all session and no label, unless show_disconnected_fans) | P2 | E24-A2 |

27 beads.

### E24 notes and open items

The anchor E24-A1 is P0 (the `perf-cpu` scene renders this tab); its nine P0 beads carry priority 0. E24-025
was added on resume: the disconnected-fan rule (PERF-076) belongs to `is_visible`, which E24 owns.

6. **Device key forms the identity cannot give.** `DeviceIdentity::Disk` has no by-path fallback and
   `Fan` has no device path, so `disk:<name>` and `fan:<chip>:<index>` are used where PERF-005 asked for
   a path. Recommendation: accept; the keys stay stable across reboots on this machine.
7. **`set_active` on the `DevicePage` trait.** E24-006 defines the trait without it; E24-A1 calls
   `set_active` on pages. E25-010 adds the method to the trait if E24-006 left it off (same-batch
   chain edit). No leader decision needed; recorded so both beads agree.
8. **Tray availability flag.** E24-023 reads `TrayModel.available` from E31-A2 and stubs `Some(false)`
   until it exists.
9. **Fan `connected` flag.** E24-025 needs a per-fan "spun this session or has a label" flag on the
   fan record. Recommendation: E12 (sensors worker) keeps it on the record in `Snapshot.system`; the
   UI does not track RPM history itself.
10. **Bench hooks for E01 (E01-OI-5, E01-OI-6).** `CpuPage.bench_show_both` is a plain public field
    in E24-009 (`tasklens-crw.11`); the paint hook is `AnimationDriver.paint_log` plus
    `ChartElement::paint` step 11 in E19-005 (`tasklens-yrf.7`) and E19-007 (`tasklens-yrf.9`),
    exported by E19-A1 (`tasklens-yrf.1`). Both are behind `cfg(any(test, feature = "test-hooks"))`,
    so E01-001 (`tasklens-wlf.3`) must declare `bench = ["test-hooks"]` in `tl-ui/Cargo.toml`.

## E25 ui-performance-devices (epic `tasklens-o55`)

Files owned: `crates/tl-ui/src/pages/performance/{memory,memory_bar,disk,net,gpu,sensors,aggregate}.rs`, `crates/tl-ui/src/windows/{smart,disk_in_use}.rs`, `crates/tl-ui/tests/ui_device_pages.rs`. Chain edits: one `build_page` arm per device kind in `pages/performance/page.rs` (E24-A1), two `pub mod` lines in `windows/mod.rs` (E16-A2). `pages/performance/battery.rs` from the 00-epics file list is folded into `sensors.rs` (UI-PRF-035 names `sensors.rs`).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E25-001 | tasklens-o55.2 | MemoryPage: Physical memory, Commit and Swap charts, page menu checks, demand (pages/performance/memory.rs) | P1 | E24-A1,E19-A1 |
| E25-002 | tasklens-o55.3 | memory composition bar element: five segments, hatching, min width, labels, tooltips, hover (pages/performance/memory_bar.rs) | P1 | E25-001,E15-A1 |
| E25-003 | tasklens-o55.4 | Memory Details grid P1 rows (In use to Kernel slab, Swap, Compressed, Savings, Huge pages) with hidden-row rules | P1 | E25-001 |
| E25-004 | tasklens-o55.5 | DiskPage: Active time and Transfer rate charts, header, demand, states (pages/performance/disk.rs) | P1 | E24-A1,E19-A1 |
| E25-005 | tasklens-o55.6 | Disk Details grid P1 rows: Active time, Average response time, Read speed, Write speed, Capacity | P1 | E25-004 |
| E25-006 | tasklens-o55.7 | NetPage: Throughput chart, AutoRate or LinkSpeed scale, disconnected state, demand (pages/performance/net.rs) | P1 | E24-A1,E19-A1 |
| E25-007 | tasklens-o55.8 | Network Details P1 rows (rates, totals) and the Display unit row with its menu | P1 | E25-006,E16-A3 |
| E25-008 | tasklens-o55.9 | GpuPage: engine grid (3D/Compute, Video Encode, Video Decode), Memory and Temperature charts, probe states (pages/performance/gpu.rs) | P1 | E24-A1,E19-A2 |
| E25-009 | tasklens-o55.10 | GPU Details grid P1 rows 1 to 8 (Utilization to Temperature source) | P1 | E25-008 |
| E25-010 | tasklens-o55.11 | build_page arms for Memory, Disk, Net and Gpu (chain edit page.rs) and page activation wiring | P1 | E25-001,E25-004,E25-006,E25-008 |
| E25-A1 | tasklens-o55.1 | Device pages | P1 | E25-003,E25-005,E25-007,E25-009,E25-010,E24-A2,E16-A2,E18-A1,E19-A2 |
| E25-011 | tasklens-o55.12 | Memory Details P2 rows from the DIMM inventory: Speed, Slots used, Form factor, Type, Hardware reserved | P2 | E25-A1 |
| E25-012 | tasklens-o55.13 | "Compressed memory" block: per-zram grids and the zswap grid | P2 | E25-A1 |
| E25-013 | tasklens-o55.14 | "Swap devices" table block (flat TlTreeTable: Name, Type, Size, Used, Priority) | P2 | E25-A1,E18-A1 |
| E25-014 | tasklens-o55.15 | "Memory modules" block: Slots used line and collapsible per-DIMM grids without serials | P2 | E25-A1,E15-A1 |
| E25-015 | tasklens-o55.16 | Disk Details P2 rows: totals, Formatted, Used, Free, System disk, Swap, Hardware name, Type, Interface, WWN, Serial, Rotation rate, Temperature with hidden rules | P2 | E25-A1 |
| E25-016 | tasklens-o55.17 | Partitions table block: heading, columns, usage bar, Unallocated rows | P2 | E25-A1,E18-A1 |
| E25-017 | tasklens-o55.18 | Disk header buttons "Show SMART data" and "Eject" with live visibility, and the page menu items | P2 | E25-A1 |
| E25-018 | tasklens-o55.19 | "tasklens SMART Data" window: header grid, Refresh, ATA table, NVMe list, failure state (windows/smart.rs) | P2 | E25-017,E16-A2,E18-A2 |
| E25-019 | tasklens-o55.20 | "tasklens Disk is in use" window: grouped blocker list, End Task and End Process, footer, retry, toasts (windows/disk_in_use.rs) | P2 | E25-017,E16-A2,E16-A3,E22-A2 |
| E25-020 | tasklens-o55.21 | Network Details P2 rows: adapter, type, status, link speed, addresses, MAC, index, driver, and the Wi-Fi rows with the signal icon | P2 | E25-A1 |
| E25-021 | tasklens-o55.22 | Public IP row: Fetch button, notice entry point, Fetching, result and failure states | P2 | E25-A1,E16-A2 |
| E25-022 | tasklens-o55.23 | GPU engine grid P2: all vendor engines, hidden engines, "Select GPU engine graphs" and "Hide this engine graph" menu items, re-tile and collapse | P2 | E25-A1 |
| E25-023 | tasklens-o55.24 | GPU optional row: Power and Clocks charts, menu checks, full-width rule, heading tooltips | P2 | E25-022 |
| E25-024 | tasklens-o55.25 | GPU Details P2 rows 9 to 28 with the hidden and vendor rules | P2 | E25-A1 |
| E25-025 | tasklens-o55.26 | GPU "Displays" block: one row per enabled display in the PERF-073 format | P2 | E25-A1 |
| E25-026 | tasklens-o55.27 | FanPage: Fan speed chart with the PWM second axis, Temperature chart with "Choose a sensor", Details rows (pages/performance/sensors.rs) | P2 | E25-A1 |
| E25-027 | tasklens-o55.28 | BatteryPage: Power or Percentage chart by device kind, Details rows, time label follows state (pages/performance/sensors.rs) | P2 | E25-026 |
| E25-028 | tasklens-o55.29 | build_page arms for Fan and Battery (chain edit page.rs) and the P2 device-page smoke tests | P2 | E25-026,E25-027 |
| E25-029 | tasklens-o55.30 | copy_text for every device page: header, Details rows, blocks and tables in the PERF-011 format | P2 | E25-A1,E24-016 |
| E25-030 | tasklens-o55.31 | Network settings link row and page menu item (KDE and GNOME only) | P3 | E25-A1 |
| E25-031 | tasklens-o55.32 | Fan page menu: "Rename fan…" dialog and "Temperature sensor >" submenu with [fans] persistence | P3 | E25-026,E16-A2,E16-A3 |
| E25-032 | tasklens-o55.33 | Battery charge history chart over the past week | P3 | E25-027 |
| E25-033 | tasklens-o55.34 | SMART window smartctl fallback button "Read with smartctl (admin)" | P3 | E25-018 |
| E25-034 | tasklens-o55.35 | Aggregate storage and Aggregate graphics pages (pages/performance/aggregate.rs) and their build_page arms | P4 | E25-A1,E24-024 |

35 beads.

### E25 open items

The device pages compile against E24-A1 (`DevicePage`, `DevicePageFrame`, `KeyValueModel`, `build_page`)
and E19-A1/A2. Every P2 bead depends on E25-A1 so the leader can schedule the P1 slice first.

- **E25-OI-1. Engine commands and records for SMART, eject, public IP and blockers are not in
  ADR-005.** `Command` has no variant for a SMART update, a smartctl read, an eject, a public IP
  fetch, and `Snapshot.system` has no named eject state or blocker list. E12-A3 (`tasklens-hjn.3`)
  is still a placeholder. Recommendation: `Command::SmartUpdate(DeviceIdx)`,
  `Command::SmartReadSmartctl(DeviceIdx)`, `Command::Eject(DeviceIdx)`, `Command::FetchPublicIp`;
  the disk record gains `smart: Option<Arc<SmartData>>`, `smart_available: bool`, `removable: bool`
  and `eject: EjectState::{Idle, Running, Blocked(Arc<[BlockerGroup]>), Done, Failed(Arc<str>)}`;
  `PublicIpState` already exists (E03-056). Beads E25-017, E25-018, E25-019, E25-021 and E25-033
  stop and report if these are missing when they start.
- **E25-OI-2. Series constants.** Memory dirty bytes, GPU shared memory, GPU memory clock, the GPU
  engines beyond util/enc/dec, fan PWM, battery power: E19 open item 1 (E03-A1 adds them).
- **E25-OI-3. No sortable `TlStaticTable` in the foundation.** UI-PRF-030 names one; `foundation.md`
  has no such entry. E25-018 uses a flat `TlTreeTable` with E18-A2 header sorting instead.
- **E25-OI-4. Cross-batch non-anchor edge.** E25-021 calls `request_public_ip_fetch` from E21-017
  (`tasklens-bj6.20`). The brief allows anchor edges only; the leader adds
  `br dep add <E25-021 id> tasklens-bj6.20` or confirms E21-A2 covers it.
- **E25-OI-5. PWM on a second axis.** UI-PRF-034 draws PWM percent on a right-hand 0..100 axis.
  `ChartSpec` has one `YScale`. E25-026 pre-scales the PWM series into the RPM axis through a
  derived `SeriesView` kept by the page. If E19 gains a per-series scale, the derived series goes.
- **E25-OI-6. GNOME network settings launch.** PERF-061 wants a session D-Bus
  `org.freedesktop.Application.ActivateAction("launch-panel", ...)` call; `ActionKind` has `Launch`
  (a command line) but no D-Bus activation. Recommendation: `LaunchWhat::DbusActivate { bus_name,
  object_path, action, parameter }` in E03-A2 and the launcher (E14-A3); until then E25-030 builds
  the KDE path only and hides the link on GNOME.
- **E25-OI-7. Battery kinds and state words.** UPower `Type` and `State` enums map to the words in
  E25-027; the mapping lives in the UI (table (d) has none). Recommendation: accept.
- **E25-OI-8. `GridStyle.vertical_at` and `ChartSeries.gap_style`** (E25-032, the week chart) are
  small additions to E19-001's types made by the E25 bead as a same-batch chain edit. If E19 beads
  are already closed, the leader may prefer a tiny E19 follow-up bead.

## E26 ui-minigraphs (epic `tasklens-xdh`)

Files owned: `crates/tl-ui/src/pages/minigraph/{mod,panel,card,menu,actions,sources}.rs`, `crates/tl-ui/src/windows/{detached_graph,mini}.rs`, `crates/tl-ui/tests/{ui_minigraphs,ui_detached}.rs`. Chain edits: `pub mod minigraph;` in `pages/mod.rs` (E16-A1), the panel mount in `pages/processes/mod.rs` (E22-A1), the three graph submenus in `pages/processes/menu.rs` and their handlers in `pages/processes/mod.rs` (E22-A2), two `pub mod` lines in `windows/mod.rs` (E16-A2), one `register` line in `app.rs` (E16-A1).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E26-001 | tasklens-xdh.3 | minigraph model: MinigraphId, MinigraphSource parse and format, slot geometry, default layout, state mapping (pages/minigraph/{mod,sources}.rs) | P1 | E16-A1,E03-A2,E24-A1 |
| E26-002 | tasklens-xdh.4 | minigraph actions: Minigraph*, ResetMinigraphLayout, AddMinigraph, AddNameGraph, AddAppGraph (pages/minigraph/actions.rs) | P1 | E26-001,E20-A1 |
| E26-003 | tasklens-xdh.5 | MinigraphCard entity: chart per system source, label and value lines, shadow, hover border, CPU tooltip, colour resolution (pages/minigraph/card.rs) | P1 | E26-001,E19-A1,E17-A1,E15-A1 |
| E26-004 | tasklens-xdh.6 | MinigraphPanel entity: overlay layout from the bottom right, settings-driven system cards, demand, full-grid toast, reset, state persistence (pages/minigraph/panel.rs) | P1 | E26-003,E26-002,E16-A2 |
| E26-005 | tasklens-xdh.7 | minigraph card context menu P1 items: Hide graph, Time range, Reset minigraph layout (pages/minigraph/menu.rs) | P1 | E26-004,E16-A3 |
| E26-006 | tasklens-xdh.8 | mount MinigraphPanel in ProcessesPage (chain edit), visible only on the Processes tab and in History mode | P1 | E26-004,E22-A1 |
| E26-A1 | tasklens-xdh.1 | Minigraph panel and cards | P1 | E26-005,E26-006,E16-A3,E19-A1,E20-A1,E22-A1 |
| E26-007 | tasklens-xdh.9 | hover readout on minigraph cards (P2) | P2 | E26-A1,E19-014 |
| E26-008 | tasklens-xdh.10 | drag to move and reorder cards: 4 px threshold, ghost, home outline, shift preview, drop to slot, no holes | P2 | E26-A1 |
| E26-009 | tasklens-xdh.11 | 5 and 10 minute ranges on minigraph cards (1 Hz tier) | P2 | E26-A1,E19-013 |
| E26-010 | tasklens-xdh.12 | "Graph color >" submenu with colour dots and MinigraphSetColor persistence | P3 | E26-A1 |
| E26-011 | tasklens-xdh.13 | process-source cards: process, name and app sources, labels, Process <pid>, column labels, series binding, demand identities | P3 | E26-A1,E22-A1 |
| E26-012 | tasklens-xdh.14 | minigraph states "No match", "Ended", Restricted and the boot-ID mismatch rule | P3 | E26-011 |
| E26-013 | tasklens-xdh.15 | process row-menu items "Add Minigraph >", "Add Process Name Graph >", "Add App Name Graph >" (chain edit pages/processes/menu.rs), handlers and toasts | P3 | E26-012,E22-A2 |
| E26-014 | tasklens-xdh.16 | DetachedGraphWindow entity: window options, size from the card stack, body drag move, own AnimationDriver, caption (windows/detached_graph.rs) | P3 | E26-A1,E16-A2 |
| E26-015 | tasklens-xdh.17 | Detach and Attach: menu items, drag past the window edge, window ids, [[minigraphs]] detached flags and [[detached_windows]] round trip | P3 | E26-014,E26-008 |
| E26-016 | tasklens-xdh.18 | detached window context menu: Show Main Window, Close graph, Attach, Time range, Graph color, Add graph >, Remove graph, Exit tasklens | P3 | E26-015,E26-010 |
| E26-017 | tasklens-xdh.19 | KWin bridge for detached graph windows and the mini window: keepAbove, geometry restore, move read-back debounced 500 ms | P3 | E26-015,E14-A2 |
| E26-018 | tasklens-xdh.20 | "Keep detached graphs open": any_open query, registry keep flag, demand while the main window is hidden, Show Main Window | P3 | E26-015,E20-A1 |
| E26-019 | tasklens-xdh.21 | MiniWindow entity: borderless window listing the visible sidebar cards, height cap and scroll, card variant, Esc closes (windows/mini.rs) | P3 | E26-A1,E24-A1,E16-A2 |
| E26-020 | tasklens-xdh.22 | mini window interactions: ToggleMiniWindow and PerfShowMiniWindow handlers, card click opens the main window on that page, double-click, context menu, Always on top, View item check state | P3 | E26-019,E26-017 |
| E26-021 | tasklens-xdh.23 | AddRandomMinigraphs: declare and handle the Debug menu action (R-238) | P4 | E26-013,E26-002,E26-011 |
| E26-A2 | tasklens-xdh.2 | Detached graphs and mini window | P3 | E26-016,E26-017,E26-018,E26-020,E26-013,E16-A2,E16-A3,E24-A1 |

22 beads.

### E26 open items

`ResetMinigraphLayout` is the action name E21-003 (`tasklens-bj6.6`) dispatches; E26-002 declares it.
E31-012 (`tasklens-wox.14`) adds the tray item to `pages/minigraph/menu.rs`; E26-005 leaves slot 7
free. The View > Graphs check items flip the `graphs.minigraph_*` settings (E21); the panel observes
the settings, so no toggle action is needed.

- **E26-OI-1. Aggregate disk and network series.** The Disk card draws the busiest disk's active
  time and shows the summed transfer rate; the Network card sums the visible physical interfaces.
  ADR-003 has no aggregate device. Recommendation: E03-A1 adds
  `DeviceIdentity::Aggregate(AggregateKind::{Disks, Nets, Gpus})` and E12 publishes `DISK_ACTIVE`
  (max), `DISK_READ`, `DISK_WRITE` (sums) on the disks aggregate and `NET_RX`, `NET_TX` on the nets
  aggregate (filtered by the `EngineSettings.show_network_*` keys). E26-003 stops and reports if
  absent; E24-024 and E25-034 (P4 aggregates) use the same devices.
- **E26-OI-2. Per-process series.** PERF-085 says process cards draw "the process's column series
  (engine-sampled)", but `SeriesId` is `{ DeviceIdx, MetricId }` and `Demand` has no field for
  card identities. Recommendation: `tl-model` gains `ProcessSeriesId { target: GraphTarget::{Process(ProcessKey), Name(Arc<str>), App(Arc<str>)}, column: ColumnId }`,
  a `Snapshot.process_series: Arc<[Arc<SeriesView>]>` component (bit `PROCESS_SERIES`), and
  `Demand.graph_targets: Arc<[GraphTarget]>` (max 16); the engine samples them at
  `processes.interval_ms` even while hidden (PROC-093). E26-011 (P3) stops and reports if undecided;
  E31 tray process graphs need the same answer.
- **E26-OI-3. Main-window close path.** PERF-084 and ADR-010 want `on_window_should_close` to keep
  the app alive when `graphs.keep_detached_on_close` is on and a detached window exists. That code
  is E20's `window/main.rs`. E26-018 provides `any_detached_open(cx)`; the leader adds the one-line
  check to E20's close bead (UI-SHL-113) or a follow-up bead, and un-ignores
  `keep_detached_on_close_keeps_app`.
- **E26-OI-4. Graph Color submenu builder.** E21-032 (`tasklens-bj6.35`) builds a shared Graph
  Color submenu; E26-010 should call it (cross-batch non-anchor edge for the leader:
  `br dep add <E26-010 id> tasklens-bj6.35`), else it builds the 18 items itself.
- **E26-OI-5. X11 always-on-top for the mini window.** PERF-093 wants `_NET_WM_STATE_ABOVE` on X11;
  gpui-pre 0.3.7 exposes no such call. Recommendation: treat X11 like "elsewhere" (item disabled,
  tooltip `Not supported by this desktop`) in v1; X11 is best effort (ADR-010).
- **E26-OI-6. Keyboard reorder.** Table (b) has no keys for moving minigraph cards; E26-008 is
  pointer-only. Recommendation: accept (Reset is the keyboard path).
- **E26-OI-7. `WindowKey::DetachedGraph(u8)`** caps detached window ids at 255 (E16-A2). Accept.
