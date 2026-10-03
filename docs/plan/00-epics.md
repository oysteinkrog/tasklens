# tasklens epic map

This file is the map for the bead-writing round. It splits the work into epics, gives every
planned source file one owner, names the anchor beads other epics may depend on, and shows that
the epic graph has no cycles. Bead writers follow it; they do not reassign files or invent anchors.

Inputs, in precedence order: `docs/DECISIONS.md`, `docs/RECONCILIATION.md`, `docs/ARCHITECTURE.md`,
`docs/spec/merged/{proc,perf,sys-app}.md`, `docs/ui/*.md` with `docs/ui/INDEX.md`. The bead rules
are in `docs/research/brief-beads.md`.

Conventions:

- Epic keys are `E01` to `E33`. Plan keys for beads are `Enn-Ann` for anchors and `Enn-nnn` for the
  rest. Bead IDs come from `br` later; each writer keeps the plan-key to bead-ID table in their own
  plan file `docs/plan/<batch>.md`.
- Phases are P0 to P4 (R-072). A bead's priority is its phase number. Anchors marked **P0** below
  are needed by the phase-0 spike and get priority 0 even when their epic is P1.
- "Owner" of a file means the only epic whose beads may create or edit that file, except where a
  file's row names a sharing rule.
- Sizes are estimates of bead counts, used only to balance the writer batches.

## 1. Epic list

33 epics. That is above the 20 to 30 asked for, because the six UI files alone hold 292 entries
and the merged specs 303; merging further would push several epics past 40 beads.

| Key | Slug | Title | Phases | Size |
|---|---|---|---|---|
| E01 | `phase0-spike` | Phase 0 spike: bench harness, scenes, gates, checklist | P0 | 15 |
| E02 | `workspace-ci` | Workspace, toolchain, lints, `check-deps`, CI | P0 | 12 |
| E03 | `model` | `tl-model`: every data contract | P1 (P0 anchors) | 30 |
| E04 | `collect-system` | `tl-collect`: system samplers, fixtures, counters | P1, P2 | 25 |
| E05 | `collect-process` | `tl-collect`: process sweep, apps, icons, detail readers | P1 to P4 | 35 |
| E06 | `collect-netsys` | `tl-collect`: sockets, startup entries, inventory, DRM GPUs | P2 to P4 | 25 |
| E07 | `ops` | `tl-ops`: verified process ops, cgroup ops, atomic files, launch | P1 to P3 | 15 |
| E08 | `engine-runtime` | `tl-engine`: runtime, supervision, `Source` loop, D-Bus helper | P1 (P0 anchors) | 15 |
| E09 | `engine-reducer-bridge` | `tl-engine`: session, reducer, table builders, `EngineHandle` | P1 (P0 anchors) | 30 |
| E10 | `engine-processes` | `tl-engine`: process sweep, detail and handle-search workers | P1 to P3 | 15 |
| E11 | `engine-actions` | `tl-engine`: action admission, tasks, deadlines, events | P1 to P3 | 25 |
| E12 | `engine-system-workers` | `tl-engine`: CPU, memory, disk, net, sensors, inventory, udisks, UPower, NM, DRM GPU, public IP | P1 to P3 | 25 |
| E13 | `gpu-probe` | `tl-gpu-probe` binary and the engine `GpuNvidia` worker | P1, P2 | 15 |
| E14 | `engine-dbus-workers` | `tl-engine`: systemd, journal, startup, connections, KWin bridge, launcher | P2 to P4 | 30 |
| E15 | `ui-foundation` | `tl-ui`: metrics, text, fonts, icons, form controls, grids, glyphs, gallery | P1 (P0 anchors) | 35 |
| E16 | `ui-app-core` | `tl-ui`: `TasklensUi::init`, hub, demand, action tracker, toasts, dialogs, windows, menus, shield, i18n, fake engine | P1 (P0 anchors) | 25 |
| E17 | `ui-theme` | `tl-ui`: `TlTheme`, built-in and user themes, kit mapping, palette, share codes | P1 to P3 | 20 |
| E18 | `ui-tree-table` | `tl-ui`: `TlTreeTable`, header, selection, cells, text cache, column picker | P1 (P0 anchors) | 30 |
| E19 | `ui-chart` | `tl-ui`: chart element, grid element, geometry, scale, labels, hover, animation driver | P1, P2 | 25 |
| E20 | `ui-shell-frame` | `tl-ui`: main window, title bar, tab strip, toolbar slot, filter, status chip, keymap, focus | P1 to P3 | 30 |
| E21 | `ui-shell-menus-settings` | `tl-ui`: menu bar and menus, Settings window, About, Crash Report, Shortcuts, theme windows, notices | P1 to P4 | 40 |
| E22 | `ui-processes-tab` | `tl-ui`: process table page, cells, header totals, menu, confirmations, toasts, jump | P1 to P4 | 40 |
| E23 | `ui-process-windows` | `tl-ui`: Properties windows, action dialogs, Run, Handle Search, dump and VirusTotal toasts | P2, P3 | 40 |
| E24 | `ui-performance-frame` | `tl-ui`: Performance tab, sidebar, device cards, page frame, details grid, CPU page | P1, P2 (P0 anchors) | 35 |
| E25 | `ui-performance-devices` | `tl-ui`: Memory, Disk, Network, GPU, Fan, Battery and aggregate pages, SMART and eject windows | P1 to P4 | 35 |
| E26 | `ui-minigraphs` | `tl-ui`: minigraph panel and cards, detached graph windows, mini window | P1 to P3 | 25 |
| E27 | `ui-services` | `tl-ui`: shared system-table frame, Services page, Service Properties with Logs | P2 to P4 | 40 |
| E28 | `ui-startup-connections-info` | `tl-ui`: Startup Apps, Connections, their Properties windows, Information tab | P2 to P4 | 40 |
| E29 | `helper-privilege-setup` | `tl-helper`, helper session, shield flows, polkit policy, Additional Setup, `tasklens-setup` | P3, P4 | 30 |
| E30 | `history` | `tl-history`, recorder and rolling window, `HistoryQuery`, History mode UI, scrub bench | P3 | 30 |
| E31 | `tray` | `tl-tray`, rasterizer, tray bridge, tray menus and tooltips | P3 | 15 |
| E32 | `low-memory` | `--watch-memory` watcher, `EngineMode::LowMemory`, rescue window, lowmem unit | P3 | 15 |
| E33 | `app-packaging` | `tl-app` binary: args, paths, settings store, logging, crash, single instance, notifications; PKGBUILD, desktop file, assets | P1 to P4 (P0 anchor) | 30 |

### Scopes and sources

**E01 `phase0-spike` (P0).** Builds the measurement side of ADR-014: `tl-ui/src/bench/` (synthetic
sources with seed 42, `BenchEngine`, frame statistics, JSON output), the scenes `perf-cpu`,
`table-2000`, `bridge` and `static`, `cargo xtask bench`, the `--bench` and `--gallery` argument
routing in `tl-app`, the 10-minute zed#64499 soak test, the manual checklist (CSD checks, three
monitors, lock and unlock, suspend and resume, minimize and restore, text sharpness), the GPU power
baseline, `docs/spike/phase0-results.md`, and the decision-rule bead that records pass, optimise or
fall back. It settles U-2, U-3, U-4, U-7, U-9, U-10, U-36, U-39 and U-47, and records the result of
each in the results file. The vendored zed#64499 patch (ADR-010 step 3) is a conditional bead in
this epic. Sources: ADR-014, ADR-010 (soak, checklist, hidden state), ADR-013 (bench row),
UI-PRF-053, UI-PRC-023, UI-FND-030 (gallery as golden host; the gallery itself is E15),
APP-052 (`--bench` form), canonical table (c).

**E02 `workspace-ci` (P0).** The Cargo workspace exactly as ADR-001 and ADR-002 write it: root
`Cargo.toml` with every `[workspace.dependencies]` pin (P3 pins included), profiles, lints,
`rust-toolchain.toml`, `.cargo/config.toml`, `Cargo.lock`, one skeleton (`Cargo.toml` plus an empty
`lib.rs` or `main.rs` with the crate's `[lints]` and feature flags) for every P1 crate and `xtask`,
`xtask check-deps` (banned crates, per-crate allowed rows, asupersync closure, banned asupersync
API grep), the deliberate failing branch test, `scripts/ci.sh`, and the GitHub Actions workflow
with lavapipe for render tests. Settles U-1. Sources: ADR-001, ADR-002, ADR-013 (layers and
commands table), ADR-004 (banned API list).

**E03 `model` (P1, P0 anchors).** All of `tl-model`, written up front for every phase because it
is the frozen contract: time and `BootId`, ids (`ProcessKey`, `DeviceIdx`, `MetricId`, `SeriesId`,
`GroupKey`, `NodeId`, `ColumnId` with every column string from R-007), `Validity` and
`FieldValidity`, `Sample`, `SeriesRing`, `SeriesView`, `Snapshot` and every component type (system,
series, processes, table, connections, services, startup, inventory, devices, health, windows,
details, history), `TableQuery`, `SysTableQuery`, `Demand`, `ActionRequest` and every `ActionKind`,
`ActionEvent`, `Settings` and `UiState` from canonical table (a) with `EngineSettings`, `units.rs`
from table (d), `wire.rs`, `gpu_proto.rs`, `helper_proto.rs` (ADR-006 request set),
`JournalQuery` and `JournalEvent`, and the `EngineClient` trait. Size tests, frozen-id tests,
format boundary tests. Sources: ADR-003, ADR-005 (client, actions), ADR-006 (action and target
types, helper request set), ADR-007 (wire, gpu_proto), ADR-008 (Demand), ADR-011 (schema), ADR-012
(history types and `RecordSink`), canonical tables (a), (d), R-005, R-006, R-007, R-013 to R-016,
R-019, R-093, R-097, R-119, PROC-001, PROC-003, PROC-022, PROC-037, PROC-099, PERF-013, PERF-017,
APP-032, D-004.

**E04 `collect-system` (P1, P2).** `tl-collect` for the machine-wide sources: `SysRoot`, the kept-open
`pread` file set and reopen rule, the FD cache, the counter helpers (rate from elapsed time, reset
detection, `WarmingUp`, the 250 ms boottime versus monotonic suspend check), parsers for
`/proc/stat`, `/proc/meminfo`, `/proc/vmstat`, `/proc/diskstats`, `/proc/net/dev`, cpufreq,
hwmon, zram `mm_stat`, `/sys/block` and `/sys/class/net` enumeration with interface
classification, CPU topology and core kinds, RAPL `energy_uj` (P2), the five samplers `cpu`,
`memory`, `disk`, `net`, `sensors`, the fixture layout with `insta` snapshots and proptests, and
`cargo xtask capture-fixture` with its scrubbing rules. Sources: ADR-016 (sources table, FD
policy), ADR-008 (CPU and memory math, counter rules, stale threshold), ADR-013 (fixtures),
PERF-004, PERF-005, PERF-027 to PERF-029, PERF-031, PERF-033 to PERF-040, PERF-041 to PERF-045,
PERF-047 to PERF-049, PERF-053 to PERF-057, PERF-076, PERF-077, PERF-106, PERF-108, R-052, R-053,
U-28.

**E05 `collect-process` (P1 to P4).** `tl-collect` for processes: `sanitize.rs` (ADR-015 limits
and control-character rules), the per-pid readers (`stat` parsed from the last `)`, `statm`,
`status`, `io`, `cgroup`, `comm`, `cmdline`, `exe`, fd count by `fstatat`), metadata refresh every
10 sweeps and on `comm` change, the whole-sweep sampler that fills `ProcessRecord` with
`FieldValidity`, `smaps_rollup` for demanded keys, classification into App, Background, System and
Ungrouped from the cgroup path with the current uid from `getuid()`, the `PROTECTED`, `IS_SELF`,
`KERNEL_THREAD` and `OTHER_USER` flags, app-id parsing from scope names, the desktop-file index
with inotify rescans, exe and `StartupWMClass` matching, the exceptions and ignore lists, Wine
matching, display naming and the bwrap unwrap, icon resolution with the `icons.toml` cache,
launcher split (P4), streamed SHA-256 and MD5 with the (dev, inode, mtime) cache, the Properties
readers (threads, fd list with `fdinfo`, maps, security fields, environment, namespaces, cwd),
the strings scanner and the handle-search walk. `benches/sweep.rs` with the 15 ms gate.
Sources: ADR-016 (processes, metadata, PSS, app grouping, icons), ADR-015 (sanitize, what is never
logged), ADR-003 (`ProcessMeta`, flags), ADR-006 (protected list), PROC-002, PROC-004 to PROC-012,
PROC-027 to PROC-031, PROC-033 to PROC-036, PROC-059 (flag side), PROC-074 (hash), PROC-078 to
PROC-084 (readers), PROC-096 (walk), R-008, R-043, R-064, R-096, R-098, R-099, R-103, U-19, U-21,
U-34.

**E06 `collect-netsys` (P2 to P4).** `tl-collect` for the P2 tabs: the `sock_diag` dump for TCP and
UDP over v4 and v6 with `INET_DIAG_INFO`, the socket-inode to pid map from `/proc/<pid>/fd`,
TCP attribution with the shared-socket rule, the connection record with engine-formatted address
strings, other network namespaces (P4) and reverse DNS (P4), XDG autostart parsing with the
override precedence and `OnlyShowIn` rules, systemd user units with `WantedBy=`, the Plasma script
directories, inotify watches for the Startup tab, startup impact, inventory readers (`cpuinfo`,
`/sys/devices/system/cpu`, DMI with root-only fields as `Restricted` and no DIMM serials,
`os-release`, displays through KMS read-only ioctls), and the AMD (P2) and Intel (P3) sysfs and
DRM fdinfo readers. Sources: ADR-016 (per-process network, socket map, connections, startup,
information, AMD and Intel GPU), PROC-032, SYS-019 to SYS-021, SYS-028 to SYS-030, SYS-035,
SYS-037, PERF-046, PERF-062 (non-NVIDIA part), PERF-072 to PERF-075, PERF-094 to PERF-104,
R-057, R-082, D-007, U-23, U-26, U-27, U-29, U-32.

**E07 `ops` (P1 to P3).** `tl-ops`: `open_verified`, `pidfd_send_signal`, the PID-based calls
(`setpriority` on every TID, `sched_setattr`, `sched_setaffinity`, `ioprio_set` through the one
`unsafe` file `sys.rs`) with the before and after pidfd poll, `tgkill` for thread targets, the
tree operation (SIGSTOP all, then SIGKILL all), `cgroup::open_validated` and the freeze, thaw and
kill writes with the `cgroup.events` wait, `fs::write_atomic` and the autostart override writer,
`dump.rs` (`/usr/bin/gcore` by fixed path to a passed fd), `launch.rs` (`posix_spawn` with
`setsid`, Desktop Entry `Exec` parsing, the `pkexec env …` administrator form), and the
real-system tests. Sources: ADR-006 (identity, PID-based rules, cgroup validation, deadlines are
the engine's), ADR-001 (`unsafe` file list), ADR-011 (atomic write), ADR-016 (autostart writes),
PROC-001, PROC-060 to PROC-068, PROC-075, PROC-095, SYS-022 to SYS-024, R-037, R-038, R-093,
R-097.

**E08 `engine-runtime` (P1, P0 anchors).** `tl-engine` below the reducer: `RuntimeBuilder` setup,
`EngineConfig` and `EngineMode`, the per-worker `ManagedSupervisor` with the backoff and the
five-per-minute rule, `SourceController` with the 60 s disable, the `Source` trait, `WorkerIo`,
the sampling loop with absolute deadlines and `IDLE_RECHECK`, the `Late` rule, `SourceUpdate` and
`EngineMsg`, the zbus connection helper with the executor tick task and the 2 s read timeouts,
the `Logind` sleep worker, and the LabRuntime tests for restart timing, budget, cancel and
demand-driven rates. Sources: ADR-004 (runtime, banned APIs, task tree, supervisor, factory
contract, worker loop, D-Bus workers), ADR-008 (schedule, tick semantics, suspend signal 1),
ADR-013 (LabRuntime cases), D-001.

**E09 `engine-reducer-bridge` (P1, P0 anchors).** `engine_root` and `engine_session` with
`catch_unwind`, epochs and the three-restarts rule, the reducer loop (receive, drain up to 256,
apply, publish once), the series rings with the fast and 1 Hz tiers, device catalog with
generations, health, the process metadata cache with linger and `NEW` and `EXITED`, pid to
`ProcessKey` mapping for GPU process data, the suspend handling (pause rates, `ResetBaselines`,
`GAP_BEFORE`), the `ProcessTableView` builder (grouped, tree, flat; sort with the key tie-break;
filter over `search_lower` and PID prefix; group totals without double counting; subtree sums;
pseudo rows), the `SysTableQuery` builder (stub in P1, body filled by E14), the `RecordSink` hook
held as `None`, `EngineHandle` with `ArcSwap`, the one-token wake, the action-event channel, the
demand and settings `watch` channels, the command queue, `Engine::start`, and `shutdown` with its
five steps. Sources: ADR-004 (session, reducer, shutdown), ADR-005 (bridge, commands), ADR-008
(suspend, coverage), ADR-009 (table query semantics), ADR-012 (hook, 1 Hz tier rule), ADR-003
(series tiers), PROC-004, PROC-008, PROC-009, PROC-012, PROC-014, PROC-018, PROC-019, PROC-038,
PROC-043 to PROC-047, PERF-005, PERF-016, PERF-017, PERF-022, R-010, R-011, R-041, R-044, R-051,
U-50.

**E10 `engine-processes` (P1 to P3).** The `Processes` worker (full sweep on the blocking pool at
`processes.interval_ms`, `LowMemory` stripped sweep hook used by E32), the `ProcessDetail` worker
(PSS and USS for `detail_keys`, Properties payloads per demanded tab at the ADR-008 rates,
`ComputeHashes`, `VerifyPackage`, affinity masks, `StringScan` with cancel), and the handle-search
task (`HandleSearch`, progress, cancel, the helper path hook used by E29). Sources: ADR-004 (worker
rows), ADR-008 (rates, `detail_keys`, `details`, `affinity_targets`), ADR-003 (`DetailsSnapshot`),
ADR-005 (commands), ADR-016 (FD cache), PROC-002, PROC-019, PROC-074, PROC-077 to PROC-085,
PROC-096, UI-PWN-002, R-098, R-119.

**E11 `engine-actions` (P1 to P3).** `tl-engine/src/actions/`: admission in the reducer (16 in
flight, 256 targets, identity, kernel thread, protected), one task per action in the session's
region, `spawn_blocking` calls into `tl-ops` with the deadline table, the event order (`Started`,
per target, summary), `ActionFinished` refresh, process kinds (signal, kill tree, suspend, resume,
nice, sched, affinity, ioprio, thread targets), cgroup kinds, unit kinds through the systemd
worker's connection (`JobRemoved` wait, `ALLOW_INTERACTIVE_AUTHORIZATION`, the dismissed-prompt
error names from U-39), startup kinds, `Dump`, `SockDestroy` (P3, via helper), `UnitWriteRecovery`
and `UnitRemoveRecovery` (P3), and the `privileged` routing hook that E29 fills. Sources: ADR-005
(action events, admission limits), ADR-006 (everything except the helper binary), ADR-015
(`ActionError`), PROC-058 to PROC-069, PROC-072, PROC-075, SYS-005 to SYS-007, SYS-013, SYS-022 to
SYS-024, SYS-031, R-035 to R-040, R-092, R-117, D-009.

**E12 `engine-system-workers` (P1 to P3).** The sampling workers `Cpu` (P0 anchor, 10 Hz with
`cpu_fast`), `Memory`, `Disk`, `Net`, `Sensors`, device enumeration every 5 s, `Inventory` (P2),
the D-Bus workers `Udisks` (model, SMART, partitions, eject with blockers, the `pkexec smartctl`
fallback in P3), `UPower`, `NetworkManager` (kind, addresses, Wi-Fi), `GpuDrm` (AMD P2, Intel P3),
and the public IP fetch on the blocking pool with `ureq` (see open item 3). Sources: ADR-004
(worker rows), ADR-008 (rates and hidden policy), ADR-016 (source rows), PERF-004, PERF-005,
PERF-033 to PERF-040, PERF-043 to PERF-060, PERF-062, PERF-074 to PERF-079, PERF-094 to PERF-104,
PERF-106, APP-056, R-088, U-25, U-30, U-37.

**E13 `gpu-probe` (P1, P2).** The `tl-gpu-probe` binary (`--fd 3` sampler mode with `Hello`,
`Sample`, `Shutdown`, `Reading<T>`, per-process utilisation, the `--api-versions` one-shot mode
with `ash` and `khronos-egl`, the `--fake <script>` test mode), the engine side `gpu/spawn.rs`
(socketpair, `pre_exec`, death signal), `gpu/client.rs` (one outstanding request, poll, deadlines,
kill and reap), the `GpuNvidia` worker with the failure table and the 300 s retry, and the probe
lookup order. Sources: ADR-007, ADR-001 (`unsafe` file), ADR-003 (`DeviceIdentity::Gpu`), PERF-062
to PERF-071, PROC-033, R-026, R-087, U-12, U-35.

**E14 `engine-dbus-workers` (P2 to P4).** `SystemdSystem` and `SystemdUser` (list, subscribe, signals,
60 s resync, `UnitDetails` on demand, cgroup pids and resource stats, OpenRC backend P4), the
`Journal` worker (`journalctl` child per `JournalHandle`, 100 ms batches, follow and `load_older`),
the `Startup` worker (scan on open and `RefreshNow`, inotify and `UnitFilesChanged` rescans), the
`Connections` worker (2 s dump, socket map, `destroy_supported`), the KWin bridge worker (script
plugin, `WindowBridge` object, `WindowsSnapshot`, one-shot command scripts, picker, matching
tasklens's own windows by pid and caption), the shared launcher (`StartTransientUnit` scopes,
`StartUnit` for autostart units), the session-bus openers (`OpenURI` portal, `FileManager1`,
`xdg-open` fallback; see open item 6), and the `SysTableQuery` builder body. Sources: ADR-004
(worker rows, KWin bridge), ADR-005 (`journal`, `KWinCommand`, `SetSysTableQuery`), ADR-016
(services, service logs, startup, connections), PROC-052 to PROC-056, PROC-073, PROC-095, SYS-001
to SYS-005, SYS-010, SYS-015, SYS-018, SYS-019, SYS-023, SYS-028, SYS-030, SYS-033, SYS-036,
UI-SYS-002, R-046, R-074, R-082, R-086, R-118, U-13, U-14, U-49.

**E15 `ui-foundation` (P1, P0 anchors).** `tl-ui` tokens and components: `UiMetrics` with the
scale rules, `TlFocusRing` (U-42), the text layer (`fonts.rs` with the four Noto faces and the
`m`-glyph check, U-7 and U-40; `style.rs` with `TNUM`; ellipsis rules; untrusted-text display;
text metrics), icons (`AssetSource` with tasklens SVGs first), `TlButton` (U-41), `TlCheckbox`,
`TlRadio`, `TlRadioGroup`, `TlSwitch`, `TlSlider`, `TlStepper`, `TlTextInput`, `TlFilterBox`,
`TlSelect`, `TlCombo`, `TlTabs`, `TlTooltip`, `TlProgress`, `TlSpinner`, `TlSkeletonRows`,
`TlSkeletonBlock`, `TlKeyValueGrid`, `TlValue`, `TlValidityGlyph`, `TlBadge`, `TlEmptyState`,
`TlBanner`, `TlLink`, and the gallery with the headless render tests (U-4). Sources: ADR-009
(metrics, fonts, icons, accessibility), ADR-013 (render tests), ADR-003 (validity glyph table),
UI-FND-003, UI-FND-010 to UI-FND-019, UI-FND-023 to UI-FND-027, UI-FND-029 to UI-FND-032,
UI-FND-050 to UI-FND-053, PERF-013, APP-040 (metrics side), APP-058, APP-059 (skeletons), R-020,
R-050, R-077, R-111, R-123.

**E16 `ui-app-core` (P1, P0 anchors).** The glue every page uses: `lib.rs` module list,
`TasklensUi::init` (fonts, kit init, assets, theme store, globals, key binding calls, the
`AppRequest` receiver), `SnapshotHub` with `ComponentMask` and `changed_series`,
`DemandAggregator` with the 100 ms coalescing and the hidden rule, `ActionTracker` with
`message(&ActionError)` and the "Outcome unknown" timeout, `ToastQueue` and `ToastKey`,
`TlDialog::confirm` with the focus-on-Cancel rule, `TlSecondaryWindow`, `WindowRegistry` and
`WindowKey`, `TlMenu` with submenus, `TlShield`, the Fluent string layer with the English
catalogue (see open item 4), and `testing/fake_engine.rs`. Sources: ADR-005 (hub, retention,
`ActionTracker`), ADR-008 (demand aggregation), ADR-009 (entity structure, nothing notifies in
render), ADR-013 (UI tests, fake engine), ADR-015 (`message`), UI-FND-020 to UI-FND-022,
UI-FND-028, UI-SHL-090, UI-SHL-100, UI-SHL-112 (UI half), UI-SHL-131, APP-043 (key system),
APP-044, APP-045, R-104, R-109, R-112 to R-114, R-128.

**E17 `ui-theme` (P1 to P3).** `TlTheme` with the 22 slots, `accent` and the 13-field
`GraphPalette`, derived tints, `assets/themes/builtin.json` with Default Dark and Default Light in
P1 and the ten other named themes in P2 (U-18), `theme/map.rs` onto gpui-kit `Theme` with the
token check against the kit source (U-8), `ThemeStore` with the switch path and `style_version`
bump, follow-system light and dark, the user `themes.json` store, the editor live-preview
contract, the `TS1.` share-code codec (U-17), the accent colour list, and import of TaskSlinger
themes (P4). Sources: ADR-009 (theme model), ADR-011 (themes path), UI-FND-001, UI-FND-002,
UI-FND-040 to UI-FND-043, UI-SHL-080, UI-PRF-008 (palette fields), PERF-024, APP-034 (themes
half), APP-035 to APP-039 (model side), R-068, R-069, R-079, R-122.

**E18 `ui-tree-table` (P1, P0 anchors).** `TlTreeTable<D: TreeTableDelegate>` on `uniform_list`:
the delegate trait, `RowKind`, `HeaderLines`, `ColumnSpec`, `CellOut`, selection by identity with
anchor and focus, click and keyboard gestures, scrolling with anchoring and the visible-range
callback, the one-line and two-line headers with resize, reorder and sort arrows, the header
context menu, the column picker window "tasklens Columns" for every table, column persistence by
id, shared table actions and their keymap, `CellTextCache`, column separators (P4), and the
`TestAppContext` tests. Sources: ADR-009 (tree-table, selection, text caching, header), ADR-011
(columns by id), UI-PRC-001, UI-PRC-005 (header mechanics), UI-PRC-006 to UI-PRC-010, UI-FND-051
(numeric cells), PROC-013, PROC-020, PROC-022 to PROC-026, PROC-042, PROC-047 to PROC-049,
APP-018, R-065, R-075, R-105, R-106, D-011.

**E19 `ui-chart` (P1, P2).** `ChartSpec`, `ChartView`, `ChartElement` with the geometry key and
cache, time mapping, decimation with absolute buckets, gap runs, fill and stroke paths, the grid,
`YScale::{Fixed, AutoRate}` with the 10 s fall rule and the 1-2-5 and power-of-two ceilings,
cached `ShapedLine` labels and legends, `ChartGridElement` with path merging, the hover readout,
curved lines hook (P4), and `AnimationDriver` with `FrameStats` and stall detection. Sources:
ADR-009 (chart element, animation driver), ADR-010 (stall detection), ADR-014 (frame stats),
UI-PRF-001 to UI-PRF-007, UI-PRF-009, PERF-015, PERF-016, PERF-018 to PERF-023, PERF-026, R-016,
R-017.

**E20 `ui-shell-frame` (P1 to P3).** `MainWindow` with the page `match`, window options and the
decorations setting, size persistence hooks, `TitleBarView` with drag region, caption buttons and
the pin, the server-decorations variant, active and inactive looks, the `Toolbar` slot with
per-tab content trait, the window picker button, `TabStrip` with the accent outline, animation,
overflow menu, the History toggle and Information copy icon, `FilterBox` and the Include-children
slot, `StatusChip` with health items, `actions.rs` with the window-level actions and `bind_keys`
for the Global context, `focus.rs` with Tab order and the Esc chain, Ctrl-pause plumbing,
type-to-filter routing, runtime scale changes, reduced motion, the FPS overlay, the close
decision (rows 1 to 3), the UI side of single-instance activation and the startup sequence
steps 8 to 11, and `settings_global.rs`. Settles U-43, U-44, U-45. Sources: ADR-009
(accessibility keys), ADR-010 (decorations, size, close), UI-SHL-001 to UI-SHL-007, UI-SHL-010,
UI-SHL-014, UI-SHL-020 to UI-SHL-025, UI-SHL-050, UI-SHL-051, UI-SHL-100 (placement), UI-SHL-110
(UI steps), UI-SHL-111, UI-SHL-112 (UI side), UI-SHL-113 (rows 2, 3, Quit), UI-SHL-120 to
UI-SHL-124, UI-SHL-130, UI-SHL-132, UI-SHL-140, APP-001, APP-002, APP-006, APP-008 to APP-011,
APP-027 to APP-029, APP-031, APP-047 (close rules), APP-058, APP-059 (frame rule), canonical
table (b), R-021, R-022, R-023, R-042, R-045, R-090, R-107, R-108, R-120, R-124, R-125, D-006,
D-008.

**E21 `ui-shell-menus-settings` (P1 to P4).** `MenuBar` and every menu (File, View with Columns,
Process View, Window, Appearance, Theme, Graphs; Settings; Help; Debug), the Settings window with
its eleven pages and the reset confirmation, the Theme Editor, Share Theme, import-from-clipboard
and delete-theme windows, the Graph Color submenu, About, Crash Report, Keyboard Shortcuts (the
full table (b) as data), Public IP Notice, Collector Health, the shell confirmations, Help >
System Information target, and the language submenu. Sources: UI-SHL-030 to UI-SHL-042,
UI-SHL-060 to UI-SHL-072, UI-SHL-081 to UI-SHL-085, UI-SHL-091 to UI-SHL-093, UI-SHL-095,
UI-SHL-096, UI-SHL-099, APP-012, APP-016 to APP-026, APP-030, APP-032 (pages), APP-036 to APP-041
(window side), APP-046, APP-048 (setting), APP-053, APP-055 (window), APP-056 (window), PERF-007
(View submenu), PERF-025 (settings page), PROC-014, PROC-017, PROC-020 (menu items), R-024, R-071,
R-078, R-085, R-089.

**E22 `ui-processes-tab` (P1 to P4).** `ProcessesPage`, `ProcessDelegate`, row anatomy for
group, unit, process and pseudo rows, the Name cell (indent, chevron, connector lines, icon,
window button), cell rendering per column type with heat colouring and dim zeros, the two-line
header totals text, the row tooltip, the row context menu with the P1 and P2 items and the
multi-select labels, the Send Signal, Set Priority, Set I/O Priority and graph submenus (menu
shells; the dialogs are E23), confirmations and toasts for process actions, filter interaction
and Include children, hide-system and view-mode toolbar buttons, Ctrl-pause rendering, status
text, new and exited highlight, empty and first-sweep states, the 2,000-row performance plan,
`select_by_key` and Go to parent, copy texts, Open file location, Restart Plasma Shell and
Restart unit items, and the processes keymap. Sources: ADR-009 (row height, heat, highlight,
Ctrl-pause, visible demand), ADR-015 (copy commands), UI-PRC-002 to UI-PRC-004, UI-PRC-005
(totals), UI-PRC-011 to UI-PRC-020, UI-PRC-022 to UI-PRC-024, UI-SHL-012, UI-SHL-013, PROC-003,
PROC-007 to PROC-009, PROC-012, PROC-014 to PROC-019, PROC-021, PROC-023, PROC-027 to PROC-036
(cells), PROC-038 to PROC-041, PROC-043 to PROC-046, PROC-049 to PROC-051, PROC-053, PROC-054,
PROC-057 to PROC-064, PROC-068 to PROC-073, APP-004, APP-005, R-009, R-012, R-031 to R-036,
R-041, R-042, R-059, R-091, R-094, D-008, D-009, D-011.

**E23 `ui-process-windows` (P2, P3).** `windows/frame.rs`, the Properties data path, the
Process Properties window with its tab strip and counts, per-tab filter bar and refresh, the
General, Threads (with the thread row menu), File descriptors, Memory maps, Security, Environment
and Strings tabs, tab access states, App Properties with its Processes tab, Set Affinity (single
and mixed), Custom priority, Realtime confirmation, Send Signal submenu and real-time signal
dialog, the confirmations this file owns, the Run window with its combo and history, Search File
Handles with results and menu, and the dump and VirusTotal toasts. It also owns the beads that add
the Properties, affinity, priority and signal items to the process row menu. Sources: UI-PWN-001
to UI-PWN-031, UI-PWN-034 to UI-PWN-036, UI-SHL-097, UI-SHL-098, PROC-065 to PROC-067, PROC-069
(dialog side), PROC-074, PROC-075, PROC-077 to PROC-085, PROC-095, PROC-096, APP-014, APP-015,
R-039, R-061, R-073, R-095, R-099, R-102, R-127.

**E24 `ui-performance-frame` (P1, P2, P0 anchors).** `PerformancePage`, the device sidebar with
`DeviceCard` mini graphs, selection, keys, hotplug and visibility, edit mode, the card and page
context menus, the splitter, the summary mode, the device page frame, the details grid with the
copy fallback (U-46), copy device info, the per-tab toolbar content, the CPU page (header, modes,
overall chart, per-core grid with core type marks, kernel overlay, overlaid and stacked modes,
the secondary graph, live and static stats, per-core labels at 10 Hz with the 5 Hz fallback
U-47), and the performance keymap. Sources: UI-PRF-010 to UI-PRF-016, UI-PRF-020 to UI-PRF-025,
PERF-001 to PERF-003, PERF-005 to PERF-008, PERF-010 to PERF-012, PERF-014, PERF-027 to PERF-040,
APP-021 (none), R-054 to R-056, R-090, R-126.

**E25 `ui-performance-devices` (P1 to P4).** The Memory page (graphs, composition bar, details,
zram and zswap, swap devices, modules), Disk pages (graphs, header buttons, details, partitions
table, "tasklens SMART Data" and "tasklens Disk is in use" windows, temperature), Network pages
(throughput graph, legend, stats, display unit row, Wi-Fi, addresses, public IP row, settings
link), GPU pages (engine graph rows, memory, encode and decode, clocks, power, temperature and
fan, PCIe, static fields, displays list), Fan and Battery pages, and the aggregate pages (P4).
Sources: UI-PRF-026 to UI-PRF-036, PERF-004 (page side), PERF-009, PERF-041 to PERF-061,
PERF-063 to PERF-079, PERF-108 (page row), R-057, R-058.

**E26 `ui-minigraphs` (P1 to P3).** `MinigraphPanel` owned by the Processes page, `MinigraphCard`
with system and process sources, the card context menu (hide and range in P1; colour, detach and
tray in P3), move and reorder, the "No match" and "Ended" states, column labels for process cards,
the row-menu items that create process cards, the detached graph windows "tasklens Graph <n>",
"Keep detached graphs open", and the "tasklens Mini Window". Sources: UI-PRF-040 to UI-PRF-045,
PERF-080 to PERF-087, PERF-093, PROC-093, APP-020 (mini window item), APP-021 (checks), R-066,
R-084 to R-086, R-089, R-100, R-101.

**E27 `ui-services` (P2 to P4).** The shared system-table page frame, the query and delegate
pattern, status rendering, the first-load skeleton, empty and error states, copy commands, action
tracking with the polkit prompt states, then the Services page (layout, filter strip, unit types
dropdown, columns, manager group rows, unit rows, child process rows, context menu, start, stop,
restart, reload and reset-failed flows, startup type submenu, copy formats, jump between
Services and Processes, status text, keyboard, OpenRC variant) and the Service Properties window
(General, Log On, Recovery view and drop-in editor, Dependencies, Logs with follow and Load
older, access states). Sources: UI-SYS-001 to UI-SYS-007, UI-SYS-010 to UI-SYS-024, UI-SYS-030 to
UI-SYS-038, SYS-001 to SYS-018, PROC-071 (Go to service item), PROC-072 (unit side), APP-059
(skeletons), R-012, R-060, R-092, R-115, R-117, R-118, U-48.

**E28 `ui-startup-connections-info` (P2 to P4).** The Startup Apps page (layout, columns, row
anatomy, impact, context menu, enable and disable, Run now, delete confirmation, copy and Open
file location, status text, Startup Properties window), the Connections page (layout, columns,
row anatomy, context menu, Close TCP connection flow, jump to owning process, copy format, status
text, Connection Properties window, Remote host column, Namespaces dropdown), and the Information
page (layout, section anatomy, row values and validity, copy section and all, filter, loading and
refresh, the eleven sections, the Help > System Information target). Sources: UI-SYS-040 to
UI-SYS-050, UI-SYS-060 to UI-SYS-070, UI-SYS-080 to UI-SYS-096, SYS-019 to SYS-037, PERF-094 to
PERF-104, APP-041, R-110, R-116, D-007.

**E29 `helper-privilege-setup` (P3, P4).** The `tl-helper` binary (framed stdin and stdout,
allowlist, own `open_verified`, 300 s idle exit, death signal), the engine helper session
(`pkexec` launch, request routing for `privileged` actions, `ReadProcessFiles` for the handle
search and Properties tabs, `ResolveSocketInodes`, `SockDestroy`, `WriteServiceDropIn`,
`NetnsSockDiagDump`), the shield button states and the `StartHelperSession` and
`EndHelperSession` flows, "Retry as administrator", "Grant access" tab states, the polkit policy
file, `tasklens-setup` with its four subcommands, the Additional Setup window (frame and rows),
the RAPL udev rule, the privilege settings (unlock at launch, passwordless rule P4), and the
Shift-at-launch path. Sources: ADR-006 (helper boundary), ADR-007 (framing reuse), UI-SHL-011,
UI-SHL-070, UI-SHL-094, UI-PRF-052, UI-PWN-018 (Grant access), PROC-076, PERF-105 to PERF-108
(window rows), APP-003, APP-042, APP-049, SYS-030 (helper part), SYS-031, canonical table (c),
R-002 to R-004, R-028, R-029, R-098, R-124, U-24, U-37, U-43.

**E30 `history` (P3).** The `tl-history` crate (segment header, 60 s blocks with zstd and CRC,
keyframes and deltas, `FieldPatch` thresholds, footer index, recovery by scan, retention and
disk cap, `RollingWindow` with the newest 10 closed blocks), the `HistoryRecorder` worker and the
`RecordSink` wiring, `HistoryQuery` and `HistoryDelete` answers with decimation, the History mode
UI (layout and header text, history table, timeline scrubber element, minimap, top-3 hover card,
legend chips, Live button and recording-off state, row menu with Delete from history), the
history settings page items, the `history-scrub` bench scene, and the recording toasts. Sources:
ADR-012, ADR-014 (scene), UI-PRC-030 to UI-PRC-037, UI-SHL-023 (toggle), UI-SHL-065 (history
keys), PROC-086 to PROC-092, R-025, R-063, D-003, D-010, U-11, U-22.

**E31 `tray` (P3).** The `tl-tray` crate on `ksni` (item, dbusmenu, re-registration on watcher
owner change), `raster.rs` with the 22 and 44 px layouts and the byte-compare update rule,
`tray_bridge.rs` in `tl-ui` with `TrayModel` and `TrayEvent`, the main tray icon with its tooltip
and menu, tray graph icons with their tooltips and menus, close and minimize to tray rows, and the
`[tray]` and `[[tray_graphs]]` keys. Settles U-31. Sources: ADR-010 (tray), UI-SHL-113 (rows 1 and
tray), UI-PRF-050, UI-PRF-051, UI-PRF-042 (tray item), PERF-088 to PERF-092, PROC-094, APP-047,
R-027, R-067.

**E32 `low-memory` (P3).** `tasklens --watch-memory` (PSI trigger, `MemAvailable` check, `mlockall`,
under 4 MiB, starts `tasklens --low-memory`), the `tasklens-lowmem.service` user unit install and
removal with `OOMScoreAdjust`, `EngineMode::LowMemory` (the stripped sweep source, `processes` and
`health` only), and the "tasklens - Low memory" window with its list of at most 30 rows, End Task
without confirmation, and the eased-pressure text. Sources: ADR-004 (engine modes), UI-PWN-032,
UI-PWN-033, UI-SHL-069 (setting), PROC-097, PROC-098, APP-052 (`--low-memory`, `--watch-memory`,
`--force-full-memory-mode`), R-062, U-24, U-50.

**E33 `app-packaging` (P1 to P4, P0 anchor).** `tl-app`: `main.rs` in the UI-SHL-110 order
(panic hook, logging, clap arguments, NVIDIA X11 rule, single instance over blocking zbus, settings
and state and themes load, engine start, `Application::run`, shutdown and log flush), `paths.rs`,
the settings and state store (load steps, migrations, corrupt-file rename, read-only newer files,
debounced atomic save, persist callback), the icon-cache path, logging with the daily file and
the never-logged list, crash reports with the newest 20 and the `.seen` marker, desktop
notifications, the `AppRequest` channel, start at login (autostart entry), `rendering.gpu` (P4),
Ctrl+Esc through kglobalaccel (P4), TaskSlinger settings import (P4); and packaging: `PKGBUILD`,
the desktop file, the polkit policy install, `assets/icons/`, install paths from table (c),
`--version`. Sources: ADR-010 (backend, single instance, close, GPU selection), ADR-011, ADR-015
(logging, crash), UI-SHL-002 (restore), UI-SHL-093 (report files), UI-SHL-110, UI-SHL-112
(D-Bus side), UI-SHL-114, APP-010, APP-011, APP-033, APP-034 (settings half), APP-046, APP-048,
APP-051, APP-052, APP-054, APP-055, canonical tables (a), (c), R-001, R-018, R-047 to R-049,
R-070, R-080, U-2, U-6, U-16.

<!-- next -->
