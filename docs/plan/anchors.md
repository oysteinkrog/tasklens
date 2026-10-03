# Epic and anchor bead IDs

Created by the leader before the bead writers start. Writers replace the anchor descriptions with the full template (`br update <id> -d ...`) and use these IDs for cross-epic dependencies.

| Plan key | Bead ID | Title |
|---|---|---|
| E01 | tasklens-wlf | Phase 0 spike: bench harness, scenes, gates, checklist |
| E02 | tasklens-v9m | Workspace, toolchain, lints, check-deps, CI |
| E03 | tasklens-as2 | tl-model: every data contract |
| E04 | tasklens-2dl | tl-collect: system samplers, fixtures, counters |
| E05 | tasklens-yms | tl-collect: process sweep, apps, icons, detail readers |
| E06 | tasklens-hx1 | tl-collect: sockets, startup entries, inventory, DRM GPUs |
| E07 | tasklens-iuz | tl-ops: verified process ops, cgroup ops, atomic files, launch |
| E08 | tasklens-8hz | tl-engine: runtime, supervision, Source loop, D-Bus helper |
| E09 | tasklens-8q0 | tl-engine: session, reducer, table builders, EngineHandle |
| E10 | tasklens-dwy | tl-engine: process sweep, detail and handle-search workers |
| E11 | tasklens-i3o | tl-engine: action admission, tasks, deadlines, events |
| E12 | tasklens-hjn | tl-engine: CPU, memory, disk, net, sensors, inventory, udisks, UPower, NM, DRM GPU, public IP |
| E13 | tasklens-ndy | tl-gpu-probe binary and the engine GpuNvidia worker |
| E14 | tasklens-y6k | tl-engine: systemd, journal, startup, connections, KWin bridge, launcher |
| E15 | tasklens-ug9 | tl-ui: metrics, text, fonts, icons, form controls, grids, glyphs, gallery |
| E16 | tasklens-fut | tl-ui: TasklensUi::init, hub, demand, action tracker, toasts, dialogs, windows, menus, shield, i18n, fake engine |
| E17 | tasklens-lgw | tl-ui: TlTheme, built-in and user themes, kit mapping, palette, share codes |
| E18 | tasklens-rsf | tl-ui: TlTreeTable, header, selection, cells, text cache, column picker |
| E19 | tasklens-yrf | tl-ui: chart element, grid element, geometry, scale, labels, hover, animation driver |
| E20 | tasklens-jqk | tl-ui: main window, title bar, tab strip, toolbar slot, filter, status chip, keymap, focus |
| E21 | tasklens-bj6 | tl-ui: menu bar and menus, Settings window, About, Crash Report, Shortcuts, theme windows, notices |
| E22 | tasklens-pdu | tl-ui: process table page, cells, header totals, menu, confirmations, toasts, jump |
| E23 | tasklens-gma | tl-ui: Properties windows, action dialogs, Run, Handle Search, dump and VirusTotal toasts |
| E24 | tasklens-crw | tl-ui: Performance tab, sidebar, device cards, page frame, details grid, CPU page |
| E25 | tasklens-o55 | tl-ui: Memory, Disk, Network, GPU, Fan, Battery and aggregate pages, SMART and eject windows |
| E26 | tasklens-xdh | tl-ui: minigraph panel and cards, detached graph windows, mini window |
| E27 | tasklens-hk6 | tl-ui: shared system-table frame, Services page, Service Properties with Logs |
| E28 | tasklens-9o0 | tl-ui: Startup Apps, Connections, their Properties windows, Information tab |
| E29 | tasklens-qfx | tl-helper, helper session, shield flows, polkit policy, Additional Setup, tasklens-setup |
| E30 | tasklens-8fx | tl-history, recorder and rolling window, HistoryQuery, History mode UI, scrub bench |
| E31 | tasklens-wox | tl-tray, rasterizer, tray bridge, tray menus and tooltips |
| E32 | tasklens-m1f | --watch-memory watcher, EngineMode::LowMemory, rescue window, lowmem unit |
| E33 | tasklens-cr0 | tl-app binary: args, paths, settings store, logging, crash, single instance, notifications; PKGBUILD, desktop file, assets |
| E01-A1 | tasklens-wlf.1 | Bench harness and perf-cpu scene run end to end |
| E01-A2 | tasklens-wlf.2 | Phase-0 results and decision recorded |
| E02-A1 | tasklens-v9m.1 | Workspace compiles |
| E02-A2 | tasklens-v9m.2 | check-deps and CI |
| E03-A1 | tasklens-as2.1 | Core types compile with size tests |
| E03-A2 | tasklens-as2.2 | Contracts compile |
| E04-A1 | tasklens-2dl.1 | SysRoot, counters, fixture infrastructure |
| E04-A2 | tasklens-2dl.2 | CPU sampler |
| E04-A3 | tasklens-2dl.3 | Memory, disk, net, sensors samplers |
| E05-A1 | tasklens-yms.1 | Process sweep |
| E05-A2 | tasklens-yms.2 | App grouping, naming, icons |
| E05-A3 | tasklens-yms.3 | Detail readers |
| E06-A1 | tasklens-hx1.1 | Sockets |
| E06-A2 | tasklens-hx1.2 | Startup entries |
| E06-A3 | tasklens-hx1.3 | Inventory readers |
| E07-A1 | tasklens-iuz.1 | Verified process operations |
| E07-A2 | tasklens-iuz.2 | cgroup operations |
| E07-A3 | tasklens-iuz.3 | Atomic files and launch |
| E08-A1 | tasklens-8hz.1 | Runtime, supervision, Source loop |
| E08-A2 | tasklens-8hz.2 | D-Bus helper and logind worker |
| E09-A1 | tasklens-8q0.1 | Engine publishes snapshots |
| E09-A2 | tasklens-8q0.2 | Shutdown |
| E10-A1 | tasklens-dwy.1 | Processes worker |
| E10-A2 | tasklens-dwy.2 | Process detail worker |
| E10-A3 | tasklens-dwy.3 | Handle search |
| E11-A1 | tasklens-i3o.1 | Action pipeline for processes |
| E11-A2 | tasklens-i3o.2 | cgroup, unit and startup actions |
| E12-A1 | tasklens-hjn.1 | CPU worker |
| E12-A2 | tasklens-hjn.2 | Memory, disk, net, sensors workers and enumeration |
| E12-A3 | tasklens-hjn.3 | Inventory and the P2 D-Bus device workers |
| E13-A1 | tasklens-ndy.1 | Probe binary |
| E13-A2 | tasklens-ndy.2 | GpuNvidia worker |
| E14-A1 | tasklens-y6k.1 | systemd workers and system table rows |
| E14-A2 | tasklens-y6k.2 | KWin bridge |
| E14-A3 | tasklens-y6k.3 | Startup and connections workers, launcher, opener |
| E14-A4 | tasklens-y6k.4 | Journal |
| E15-A1 | tasklens-ug9.1 | Metrics, text, icons and base controls |
| E15-A2 | tasklens-ug9.2 | Display components |
| E15-A3 | tasklens-ug9.3 | Gallery and render tests |
| E16-A1 | tasklens-fut.1 | App core |
| E16-A2 | tasklens-fut.2 | Windows, dialogs, toasts |
| E16-A3 | tasklens-fut.3 | Menus, shield, action tracker |
| E17-A1 | tasklens-lgw.1 | Theme model |
| E17-A2 | tasklens-lgw.2 | User themes and follow-system |
| E18-A1 | tasklens-rsf.1 | Generic tree table |
| E18-A2 | tasklens-rsf.2 | Two-line header, picker and persistence |
| E19-A1 | tasklens-yrf.1 | Chart element |
| E19-A2 | tasklens-yrf.2 | Grid element, labels, hover |
| E20-A1 | tasklens-jqk.1 | Main window frame |
| E20-A2 | tasklens-jqk.2 | Filter box and settings global |
| E21-A1 | tasklens-bj6.1 | Menu bar |
| E21-A2 | tasklens-bj6.2 | Settings window |
| E22-A1 | tasklens-pdu.1 | Process table renders |
| E22-A2 | tasklens-pdu.2 | Row menu, confirmations, toasts |
| E22-A3 | tasklens-pdu.3 | Jump target |
| E23-A1 | tasklens-gma.1 | Properties window and General tab |
| E23-A2 | tasklens-gma.2 | Action dialogs |
| E23-A3 | tasklens-gma.3 | Notices |
| E24-A1 | tasklens-crw.1 | Performance tab with CPU page |
| E24-A2 | tasklens-crw.2 | Sidebar behaviour |
| E25-A1 | tasklens-o55.1 | Device pages |
| E26-A1 | tasklens-xdh.1 | Minigraph panel and cards |
| E26-A2 | tasklens-xdh.2 | Detached graphs and mini window |
| E27-A1 | tasklens-hk6.1 | System table frame |
| E27-A2 | tasklens-hk6.2 | Services page |
| E27-A3 | tasklens-hk6.3 | Service Properties |
| E28-A1 | tasklens-9o0.1 | Startup Apps page |
| E28-A2 | tasklens-9o0.2 | Connections page |
| E28-A3 | tasklens-9o0.3 | Information page |
| E29-A1 | tasklens-qfx.1 | Helper binary |
| E29-A2 | tasklens-qfx.2 | Helper session in the engine |
| E29-A3 | tasklens-qfx.3 | Additional Setup window |
| E30-A1 | tasklens-8fx.1 | History crate |
| E30-A2 | tasklens-8fx.2 | Recorder and query |
| E30-A3 | tasklens-8fx.3 | History mode UI |
| E31-A1 | tasklens-wox.1 | Tray crate |
| E31-A2 | tasklens-wox.2 | Tray bridge |
| E32-A1 | tasklens-m1f.1 | Memory watcher |
| E32-A2 | tasklens-m1f.2 | Rescue window and low-memory engine |
| E33-A1 | tasklens-cr0.1 | Binary starts |
| E33-A2 | tasklens-cr0.2 | Settings store |
| E33-A3 | tasklens-cr0.3 | Packaging |
