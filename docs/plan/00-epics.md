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

## 2. File ownership

Every planned file, its owner, and the rule for other epics. "Owner only" means no other epic
touches the file. The two sharing rules are:

- **Chain edit**: the owner creates the file; another epic may add a small, named part (one `mod`
  line, one `match` arm, one `on_action` registration, one menu item, one bead's gallery page) in a
  bead of its own that depends on the owner's anchor named in the row. The edit is the only change
  that bead makes to the file.
- **Up front**: the owner writes the whole file in one pass for every phase, because the docs
  already fix every value. Nobody else edits it. Where a later-phase part is still missing, the
  later epic opens a bead that depends on the owner's anchor and asks the leader, it does not edit.

Aggregator files (`lib.rs`, `mod.rs`, root `Cargo.toml` members, `xtask` dispatch, `app.rs` wiring,
`MainWindow` page match) all use the chain-edit rule. Tests live next to the code they test
(`#[cfg(test)]`) unless a `tests/` file is listed; `tests/` files belong to the owner of the code
they cover.

### Repository root

| Path | Owner | Rule |
|---|---|---|
| `Cargo.toml` (workspace, deps, profiles, lints) | E02 | chain edit: a P3 crate adds its `members` line (E29 `tl-helper`, E30 `tl-history`, E31 `tl-tray`), depends on E02-A1; E01 adds the one `[patch.crates-io]` line for `vendor/gpui-pre-linux/` if the soak fails (R-160); every dependency pin from ADR-002 is written up front by E02, including the two `bench` feature lines (E02-005, R-158), so E01 has no `Cargo.toml` chain edit for them |
| `vendor/gpui-pre-linux/` | E01 | owner only (R-160); created only if the zed#64499 soak fails (ADR-010) |
| `Cargo.lock` | E02 | regenerated by any bead that adds a crate or dependency; never hand-edited |
| `rust-toolchain.toml`, `.cargo/config.toml`, `rustfmt.toml`, `clippy.toml` | E02 | owner only |
| `scripts/ci.sh` | E02 | owner only |
| `.github/workflows/ci.yml` | E02 | owner only |
| `xtask/Cargo.toml`, `xtask/src/main.rs` (subcommand dispatch) | E02 | chain edit: one `match` arm per subcommand module, depends on E02-A1 |
| `xtask/src/check_deps.rs` | E02 | owner only |
| `xtask/src/capture_fixture.rs` | E04 | owner only |
| `xtask/src/bench.rs` | E01 | owner only |
| `assets/fonts/NotoSans-{Regular,Medium,SemiBold}.ttf`, `assets/fonts/NotoSansMono-Regular.ttf`, `assets/fonts/LICENSE-OFL.txt` | E15 | owner only |
| `assets/icons/tasklens.svg`, `assets/icons/shield-unlocked.svg`, `assets/icons/shield-locked.svg`, `assets/icons/*.svg` | E15 | chain edit: an epic that needs a new app SVG adds the file (not a shared file) and lists it in the `AssetSource` test, depends on E15-A1 |
| `assets/themes/builtin.json` | E17 | up front in P1 for the two default themes; the P2 bead that adds the ten named themes is E17's own |
| `assets/locales/en-US.ftl` | E16 | chain edit: every epic that introduces user-visible strings appends its keys in the bead that uses them, depends on E16-A1 (see open item 4) |
| `assets/locales/<other>.ftl` (P4) | E16 | owner only |
| `assets/kwin/kwin-bridge.js`, `assets/kwin/pick.js`, `assets/kwin/cmd-template.js` | E14 | owner only (embedded with `include_str!`, written to `$XDG_RUNTIME_DIR/tasklens/` at run time) |
| `packaging/PKGBUILD`, `packaging/io.github.oysteinkrog.Tasklens.desktop` | E33 | owner only |
| `packaging/io.github.oysteinkrog.Tasklens.policy` | E29 | owner only |
| `packaging/tasklens-setup` (POSIX sh), `packaging/99-tasklens-powercap.rules`, `packaging/tasklens-lowmem.service.in` | E29 (setup, rules), E32 (`lowmem.service.in`) | owner only |
| `docs/spike/phase0-results.md` | E01 | owner only |
| `docs/plan/*.md` | the writer batch that owns it | owner only |

### `crates/tl-model` (owner E03, every file up front)

| Path | Contents |
|---|---|
| `Cargo.toml`, `src/lib.rs` | skeleton by E02-A1; E03 fills the module list in E03-A1 and never changes it afterwards |
| `src/time.rs` | `TimeNs`, `now_boot_ns`, `now_mono_ns`, `BootId` |
| `src/ids.rs` | `ProcessKey`, `DeviceIdx`, `MetricId` constants (R-170 numbers and the `TIER_1HZ` bit), `SeriesId`, `GroupKey`, `NodeId` (with `Unit`, `KernelThreads`, `Pseudo`, R-141), `ColumnId` with `as_str`/`from_str` and every column string from R-007 (numbers 1 to 121 frozen, R-155), `GraphTarget`, `ProcessSeriesId` (R-154) |
| `src/validity.rs` | `Validity`, `FieldValidity`, `ProcessField`, `PROCESS_FIELDS` |
| `src/series.rs` | `Sample`, `SampleFlags`, `SeriesRing` (two tiers), `SeriesView` |
| `src/process.rs` | `ProcessMeta`, `ProcState`, `ProcFlags`, `ProcessRecord`, `ProcessExtras`, `ProcessExtrasP2`, `AppCgroupSample` (R-141, R-163), `LowMemoryInfo`, `LowMemGroup` (R-150), `GroupKind`, `GroupRow`, `GroupRowKind`, `Coverage` |
| `src/system.rs` | `CpuTotals`, `MemoryTotals`, `DiskTotals`, `NetTotals`, `GpuTotals`, `SensorReading`, `LinkState`, `FanControlMode` (R-139), `CpuTopology`, `CoreInfo`, `CoreKind`, `CpuSet` |
| `src/devices.rs` | `DeviceIdentity` (with `Aggregate(AggregateKind)`, R-154), `DeviceEntry` (with `persistent_key`, `net_kind`, R-155), `DeviceCatalog`, `NetKind` |
| `src/health.rs` | `SourceId`, `SourceState` (with `Unsupported`, R-155), `SourceHealth`, `HealthSnapshot` (with `caps`, `helper_start`, `setup`), `EngineCaps`, `SetupState`, `SetupStatus`, `SetupRun`, `HelperStartOutcome` (R-141, R-148, R-149), `ManagerState` |
| `src/snapshot.rs` | `ComponentHeader`, `Snapshot`, `SeriesSet`, `ProcessSnapshot`, `SystemSnapshot`, `ConnectionsSnapshot`, `ServicesSnapshot`, `StartupSnapshot`, `InventorySnapshot`, `WindowsSnapshot` (with `pick`, R-149), `DeviceDetailsSnapshot` and its detail structs (R-139), `ProcessSeriesSet` (R-154), `ComponentMask` and bit constants (fifteen bits, R-139) |
| `src/table.rs` | `TableQuery`, `TableMode`, `SortSpec`, `TableRow`, `ProcessTableView`, `TableTotals`, `SysTable`, `SysTableQuery`, `SysNodeId`, `SysColumnId` (R-155, R-170), `ServiceFilter`, `SysRow`, `UnitRef`, `Manager`, `StartupRef` |
| `src/details.rs` | `DetailTarget`, `DetailTab`, `TabPayload` and the per-tab payload structs, `TargetDetail`, `AffinityInfo`, `HandleSearchState`, `HandleHit`, `SearchId`, `ScanId`, `StringScanOpts`, `DetailsSnapshot` |
| `src/demand.rs` | `Demand` (with `graph_targets`, `GRAPH_TARGETS_MAX = 16`, R-154), `CpuDemand`, `ColumnSet`, `DetailDemand`, `TrayDemand` (R-156) |
| `src/actions.rs` | `ActionId`, `ActionRequest`, `ActionTarget`, `ActionKind`, `Signal`, `SchedPolicy`, `IoPriority`, `CgroupRef`, `DumpMode`, `RecoveryEdit`, `Family`, `Proto`, `FourTuple`, `ActionState`, `RejectReason`, `ActionError`, `ActionEvent` |
| `src/command.rs` | `Command` (with `Device`, `StartHelperSession`, `EndHelperSession`), `DeviceCommand` (R-139, R-148), `KWinCommand` (`Pick { seq }`, `CancelPick { seq }`), `SetupStep` (five variants, R-149), `HistoryRange`, `HistoryWhat`, `CommandError`, `AppRequest` |
| `src/settings.rs` | `Settings` with every section of canonical table (a), `UiState` with every `state.toml` table, `EngineSettings`, ranges and clamps, `SCHEMA_VERSION` |
| `src/units.rs` | every format of table (d), writing into a caller `String` |
| `src/wire.rs` | length-prefixed postcard framing, `MAX_PROBE_FRAME`, `MAX_HELPER_FRAME`, multi-frame reply reader |
| `src/gpu_proto.rs` | `ProbeRequest`, `ProbeResponse`, `Reading<T>`, `GpuDeviceInfo`, `GpuDeviceSample`, `GpuProcessSample`, `ApiVersions`, `GPU_PROTO_VERSION` |
| `src/helper_proto.rs` | `HelperRequest` (the ADR-006 allowlist), `HelperReply`, `HELPER_PROTO_VERSION` |
| `src/journal.rs` | `JournalQuery`, `JournalHandle`, `JournalEvent`, `JournalEntry`, `JournalAccess`, `JournalError` |
| `src/history.rs` | the E30 block (R-156): `HistoryMetric`, `HistoryRange`, `HistoryWhat`, `HistoryView`, `HistoryProcessRow`, `HistoryTop`, `HistoryLoadProblem`, `RecordSink` trait |
| `src/tray.rs` | `TrayModel`, `TrayEvent`; written by E31-001 after E03-A2 with its `pub mod tray;` line, the one later addition to the module list (R-161) |
| `src/client.rs` | `EngineClient` trait |
| `tests/sizes.rs`, `tests/frozen_ids.rs`, `tests/units_format.rs`, `tests/wire_framing.rs`, `tests/settings_defaults.rs`, `tests/proto_roundtrip.rs` | E03 |

### `crates/tl-collect`

| Path | Owner | Rule |
|---|---|---|
| `Cargo.toml`, `src/lib.rs` | E04 | chain edit: `mod` lines by E05 and E06, depend on E04-A1 |
| `src/error.rs` (`CollectError`) | E04 | chain edit: E05 and E06 add their variants in their anchor beads, depend on E04-A1 |
| `src/sysroot.rs` (`SysRoot`, kept-open file set, `pread` helpers, reopen rule) | E04 | owner only |
| `src/fdcache.rs` (128-entry `smaps_rollup` cache) | E04 | owner only |
| `src/counters.rs` (rate, reset, `WarmingUp`, suspend gap check) | E04 | owner only |
| `src/parse/mod.rs` | E04 | chain edit for `mod` lines, depends on E04-A1 |
| `src/parse/{stat,meminfo,vmstat,diskstats,netdev,cpufreq,hwmon,zram,block,netif,topology,rapl,udev,partitions}.rs` | E04 | owner only; `udev.rs` is E04-030 and `partitions.rs` E04-031 (R-140); `vmstat.rs` stays a stub in v1 (R-162) |
| `src/sample/mod.rs`, `src/sample/{cpu,memory,disk,net,sensors}.rs` | E04 | owner only |
| `src/sanitize.rs` | E05 | owner only |
| `src/proc/mod.rs` | E05 | owner only |
| `src/proc/{pid_stat,status,statm,io,cgroup,meta,fdcount,sweep,smaps,detail,strings,handles,extras_p2}.rs` | E05 | owner only; `extras_p2.rs` is E05-031 (R-163); `strings.rs` is shared with tl-helper (R-157) |
| `src/pkg.rs` (package owner index) | E05 | owner only; E05-030 (R-163) |
| `src/apps/mod.rs`, `src/apps/{classify,cgroup_app,desktop,icons,naming,launchers}.rs` | E05 | owner only |
| `src/hash.rs` | E05 | owner only |
| `src/net/mod.rs`, `src/net/{sock_diag,socket_map,attribution,connections,netns,rdns}.rs` | E06 | owner only |
| `src/startup/mod.rs`, `src/startup/{autostart,user_units,plasma_scripts,watch,impact}.rs` | E06 | owner only |
| `src/inventory/mod.rs`, `src/inventory/{cpuinfo,dmi,os_release,displays,modules}.rs` | E06 | owner only |
| `src/gpu_drm/mod.rs`, `src/gpu_drm/{amdgpu,intel,fdinfo}.rs` | E06 | owner only |
| `tests/fixtures/<name>/**` | E04 for `owner-desktop`, `minimal-vm` and the system edge cases; E05 for the process edge cases; E06 for socket, autostart and DMI fixtures | each fixture directory has one owner |
| `tests/fixture_{stat,meminfo,diskstats,netdev,cpufreq,hwmon,zram,block,netif}.rs`, `tests/prop_system.rs` | E04 | owner only |
| `tests/fixture_process.rs`, `tests/fixture_apps.rs`, `tests/prop_process.rs`, `tests/sanitize.rs`, `tests/real_restricted.rs` | E05 | owner only |
| `tests/fixture_sockets.rs`, `tests/fixture_startup.rs`, `tests/fixture_inventory.rs`, `tests/prop_netsys.rs` | E06 | owner only |
| `benches/sweep.rs` | E05 | owner only |

### `crates/tl-ops` (owner E07, owner only)

`Cargo.toml`, `src/lib.rs`, `src/error.rs` (`OpsError`), `src/process.rs`, `src/cgroup.rs`,
`src/sys.rs` (the one `unsafe` file; `ioprio_get_tid` is `pub`, R-144), `src/fs.rs` (`write_atomic`, autostart override write and
remove), `src/dump.rs` (gdb `gcore` into a passed fd, R-145), `src/launch.rs`, `tests/ops_real.rs`, `tests/cgroup_validate.rs`,
`tests/launch_exec_parse.rs`. One exception: `src/dropin.rs` (`render(&RecoveryEdit) -> String`) is an E29-owned file inside
tl-ops, written by E29-009 and used by E11-020 (R-168).

### `crates/tl-gpu-probe` (owner E13, owner only)

`Cargo.toml`, `src/main.rs`, `src/nvml.rs`, `src/sampler.rs`, `src/api_versions.rs` (the fourth `unsafe` file, R-143), `src/fake.rs`
(feature `fake`), `tests/fake_scripts.rs`.

### `crates/tl-engine`

| Path | Owner | Rule |
|---|---|---|
| `Cargo.toml`, `src/lib.rs` | E08 | chain edit: `mod` lines by E09 to E14, E29, E30, E32, depend on E08-A1 |
| `src/config.rs` (`EngineConfig`, `EngineMode`) | E08 | owner only |
| `src/runtime.rs`, `src/supervise.rs`, `src/controller.rs` (`SourceController`), `src/source.rs` (`Source`, `WorkerIo`, `run`, `IDLE_RECHECK`), `src/update.rs` (`SourceUpdate`, `EngineMsg`, `WorkerError`) | E08 | owner only; `SourceUpdate` carries every variant up front |
| `src/dbus/mod.rs` (connection builder, executor tick task, timeout wrappers) | E08 | owner only |
| `src/workers/mod.rs` | E08 | chain edit: one `mod` line per worker file, depends on E08-A1 |
| `src/workers/logind.rs` | E08 | owner only |
| `src/session.rs` (`engine_root`, `engine_session`, epochs) | E09 | chain edits at named insertion points: E29-020 spawns the helper, E30-A2 installs the `RecordSink` and starts the history worker (R-150) |
| `src/reducer/mod.rs`, `src/reducer/{apply,series,devices,health,processes,suspend}.rs` | E09 | `apply.rs` handles every `SourceUpdate` variant up front (the `Helper` and `Setup` arms only log until E29); chain edits: E29-020 adds the `StartHelperSession` and `EndHelperSession` routing through `helper_tx`, E30-A2 the `HistoryQuery` and `HistoryDelete` forward (R-148, R-150) |
| `src/reducer/process_series.rs` (per-process series from `Demand.graph_targets`), `src/reducer/power.rs` (PROC-034 estimate) | E09 | owner only; E09-029 and E09-030 (R-154, R-163) |
| `src/reducer/table.rs` (`ProcessTableView` builder) | E09 | owner only |
| `src/reducer/sys_table.rs` (`SysTableQuery` builder) | E09 | E09-A1 creates it with the full signatures and an empty body; E14 fills the body in E14-A1, depends on E09-A1; nobody else edits it |
| `src/bridge.rs` (`EngineHandle`, `Engine::start`, `ShutdownReport`, `EngineClient` impl) | E09 | owner only |
| `src/workers/processes.rs` | E10 | chain edit: E32 adds the `EngineMode::LowMemory` branch that selects the stripped sweep, depends on E10-A1; E29-017 adds the restricted-cell helper reads (R-168) |
| `src/workers/process_detail.rs`, `src/workers/handle_search.rs` | E10 | chain edit: E29 adds the helper call sites for `Restricted` targets, depends on E10-A2 |
| `src/actions/mod.rs` (admission, region, event send), `src/actions/{process,cgroup,unit,startup,dump}.rs` | E11 | chain edit: E29 adds `actions/helper.rs` and the `privileged` routing arm in `mod.rs`, depends on E11-A1 |
| `src/actions/socket.rs`, `src/actions/recovery.rs` | E11 | owner only (P3, calls the helper hook) |
| `src/workers/{cpu,memory,disk,net,sensors,enumerate,inventory,gpu_drm,public_ip}.rs`, `src/workers/{dbus_udisks,dbus_upower,dbus_nm}.rs` | E12 | owner only |
| `src/gpu/mod.rs`, `src/gpu/spawn.rs`, `src/gpu/client.rs`, `src/workers/gpu_nvidia.rs` | E13 | owner only |
| `src/workers/{dbus_systemd,journal,startup,connections,dbus_kwin}.rs`, `src/launcher.rs`, `src/opener.rs` | E14 | owner only, except E29-018 adds `ResolveSocketInodes` to `connections.rs` (R-168); the opener also reads the portal colour scheme (R-170) |
| `src/workers/history.rs`, `src/history_query.rs` | E30 | owner only |
| `src/workers/lowmem.rs` (stripped sweep source) | E32 | owner only |
| `src/helper/mod.rs`, `src/helper/session.rs`, `src/helper/setup.rs` (setup runner, `TASKLENS_SETUP_ROOT` test mode), `src/actions/helper.rs` | E29 | owner only (R-168) |
| `tests/lab_supervise.rs`, `tests/lab_source_loop.rs`, `tests/lab_demand.rs`, `tests/fake_sources.rs` | E08 | owner only |
| `tests/lab_reducer.rs`, `tests/lab_session.rs`, `tests/lab_bridge.rs`, `tests/lab_table.rs`, `tests/lab_suspend.rs`, `tests/real_shutdown.rs` | E09 | owner only |
| `tests/lab_processes.rs`, `tests/lab_details.rs` | E10 | owner only |
| `tests/lab_actions.rs`, `tests/real_actions.rs`, `tests/real_cgroup.rs` | E11 | owner only |
| `tests/lab_system_workers.rs`, `tests/real_inventory.rs` | E12 | owner only |
| `tests/probe_integration.rs`, `tests/real_nvml.rs` | E13 | owner only |
| `tests/lab_dbus_workers.rs`, `tests/real_systemd_unit.rs`, `tests/real_journal.rs` | E14 | owner only |
| `tests/lab_history.rs` | E30 | owner only |
| `tests/lab_helper.rs`, `tests/real_helper.rs` | E29 | owner only |

### `crates/tl-ui`

| Path | Owner | Rule |
|---|---|---|
| `Cargo.toml` (features `bench`, `render-tests`, `test-hooks`) | E16 | the `bench = ["gpui-kit/profiler", "test-hooks"]` line is written up front by E02-005 (R-158, R-160); no E01 chain edit |
| `src/lib.rs` | E16 | chain edit: `mod` lines by every tl-ui epic, depend on E16-A1 |
| `src/app.rs` (`TasklensUi::init`, globals, `AppRequest` receiver) | E16 | chain edit: each epic adds its `bind_keys_<page>` call and its page constructor registration, depends on E16-A1; E20 adds the main-window open call |
| `src/hub.rs`, `src/demand.rs`, `src/action_tracker.rs`, `src/i18n.rs`, `src/testing/mod.rs`, `src/testing/fake_engine.rs`, `src/testing/render.rs` (headless capture helpers) | E16 | owner only |
| `src/components/mod.rs` | E15 | chain edit: `mod` lines by E16 for its six components, depend on E15-A1 |
| `src/components/{focus,icons,button,checkbox,radio,switch,slider,stepper,input,select,tabs,tooltip,progress,skeleton,kv_grid,validity,badge,empty,banner,link}.rs` | E15 | owner only |
| `src/components/gallery.rs` | E15 | chain edit: one gallery page per E16 component, depends on E15-A3 |
| `src/components/{toast,dialog,window_frame,menu,shield}.rs`, `src/window/registry.rs` | E16 | owner only |
| `src/theme/mod.rs`, `src/theme/{builtin,derived,map,metrics,palette,store,user_store,preview,share_code,import_ts}.rs` | E17 | owner only (`metrics.rs` holds `UiMetrics`; E15 depends on E17-A1 for it) |
| `src/text/mod.rs`, `src/text/{fonts,style,ellipsis,untrusted,metrics}.rs` | E15 | owner only |
| `src/text_cache.rs` | E18 | owner only |
| `src/table/mod.rs`, `src/table/{tree_table,header,columns,columns_window,selection,cells,actions}.rs` | E18 | owner only; the column picker is table-generic and lives here, not under `pages/processes/` (R-181); page-specific cell painters live in the page, never in `cells.rs` |
| `src/chart/mod.rs`, `src/chart/{element,view,geometry,grid,scale,labels,hover,grid_element}.rs`, `src/anim.rs` | E19 | owner only |
| `src/actions.rs` (window-level Actions, `bind_keys_global`), `src/focus.rs`, `src/settings_global.rs` | E20 | owner only |
| `src/window/mod.rs` | E16 | chain edit: `mod` lines by E20, E21, E23 to E32, depend on E16-A1 |
| `src/window/main.rs` | E20 | chain edit: one page `match` arm and one `ToolbarContent` registration per page epic (E22, E24, E27, E28), each depends on E20-A1; E31-009 adds the close-to-tray decision (close row 1), E29-030 the Additional Setup first-focus rule, E20-029 the secondary-window keep-above arm (R-161, R-168, R-176) |
| `src/window/{titlebar,toolbar,tabstrip,filter,status}.rs` | E20 | owner only, except E31-009 adds minimize-to-tray to `titlebar.rs` (R-161) |
| `src/window/{menubar,menus_file,menus_view,menus_settings,menus_help,menus_debug}.rs` | E21 | owner only; a menu item that dispatches a page Action depends on that page's anchor at bead level |
| `src/window/settings/mod.rs`, `src/window/settings/{general,appearance,motion,units,processes,performance,graphs,services,startup_window,privileges,advanced}.rs` | E21 | chain edit: E29 adds the privilege buttons' handlers in `privileges.rs`, E30 the history rows' handlers in `processes.rs`, E31 the tray rows in `startup_window.rs`, E32 the lowmem row; each depends on E21-A2 |
| `src/window/{theme_editor,share_theme,about,crash_report,shortcuts,public_ip,collector_health,confirm}.rs` | E21 | owner only |
| `src/window/setup.rs` (Additional Setup frame and rows) | E29 | owner only |
| `src/pages/mod.rs` | E16 | chain edit: `mod` lines per page epic, depend on E16-A1 |
| `src/pages/processes/mod.rs` (`ProcessesPage`) | E22 | chain edit: E26 mounts `MinigraphPanel`, E30 mounts History mode and its toggle handling, E23 registers the Properties and dialog `on_action` handlers; each depends on E22-A1 |
| `src/pages/processes/{table,delegate,name_cell,cells,header_totals,toolbar,filter,confirm,toasts,jump,status,actions,tooltip}.rs` | E22 | owner only |
| `src/pages/processes/menu.rs` | E22 | chain edit: E23 adds the Properties, Set Affinity, Set Priority dialog and Send Signal dialog items; E26 adds the graph items; E27 adds "Go to service"; E31 adds the tray item; each depends on E22-A2 |
| `src/pages/processes/history/mod.rs`, `src/pages/processes/history/{table,scrubber,minimap,legend,hover,actions}.rs` | E30 | owner only |
| `src/windows/mod.rs` | E16 | chain edit: `mod` lines by E23 to E32, depend on E16-A1 |
| `src/windows/frame.rs`, `src/windows/props/mod.rs`, `src/windows/props/{general,threads,fds,maps,security,environ,strings,filter}.rs`, `src/windows/{app_props,affinity,priority,signal,run,run_history,handle_search,notices,actions}.rs` | E23 | owner only |
| `src/windows/{smart,disk_in_use}.rs` | E25 | owner only |
| `src/windows/{detached_graph,mini}.rs` | E26 | owner only |
| `src/windows/service_properties/mod.rs`, `src/windows/service_properties/{general,logon,recovery,dependencies,logs}.rs` | E27 | owner only |
| `src/windows/{startup_properties,connection_properties,confirm_startup_delete}.rs` | E28 | owner only |
| `src/windows/low_memory.rs` | E32 | owner only |
| `src/pages/performance/mod.rs`, `src/pages/performance/{page,sidebar,card,summary,menu,actions,details,cpu,cpu_grid,cpu_details}.rs` | E24 | chain edit: E25 adds one `match` arm per device kind in `page.rs`, depends on E24-A1 |
| `src/pages/performance/{memory,memory_bar,disk,net,gpu,sensors,battery,aggregate}.rs` | E25 | owner only |
| `src/pages/minigraph/mod.rs`, `src/pages/minigraph/{panel,card,menu,actions,sources}.rs` | E26 | chain edit: E31 adds the "Show in tray" item in `menu.rs`, depends on E26-A1 |
| `src/pages/sys_table.rs` (`SysTablePage` frame, `SysDelegate` pattern, skeleton, empty states), `src/pages/sys_copy.rs`, `src/pages/sys_status.rs` (status cells, badges) | E27 | owner only |
| `src/pages/services/mod.rs`, `src/pages/services/{delegate,filter_strip,menu,flows,copy,actions}.rs` | E27 | owner only |
| `src/pages/startup/mod.rs`, `src/pages/startup/{delegate,menu,flows,copy,actions}.rs` | E28 | owner only |
| `src/pages/connections/mod.rs`, `src/pages/connections/{delegate,menu,flows,copy,actions}.rs` | E28 | owner only |
| `src/pages/information/mod.rs`, `src/pages/information/{section,sections,copy,filter,actions}.rs` | E28 | owner only |
| `src/tray_bridge.rs` | E31 | owner only |
| `src/bench/mod.rs`, `src/bench/{scenes,synth,stats,engine}.rs` | E01 | chain edit: E30 adds the `history-scrub` scene file `src/bench/history_scrub.rs` and its `mod` line, depends on E01-A1 |
| `tests/ui_components.rs`, `tests/render_gallery.rs`, `tests/golden/*.png` | E15 | chain edit: E16 adds its component cases and goldens, depends on E15-A3 |
| `tests/ui_hub.rs`, `tests/ui_demand.rs`, `tests/ui_actions.rs`, `tests/ui_toasts.rs`, `tests/ui_dialogs.rs` | E16 | owner only |
| `tests/ui_theme.rs`, `tests/render_theme.rs` | E17 | owner only |
| `tests/ui_tree_table.rs`, `tests/ui_selection.rs`, `tests/ui_columns.rs`, `tests/render_table.rs` | E18 | owner only |
| `tests/ui_chart.rs`, `tests/chart_geometry.rs`, `tests/ui_anim.rs`, `tests/render_chart.rs` | E19 | owner only |
| `tests/ui_shell.rs`, `tests/ui_keymap.rs` (`keymap_matches_table_b`), `tests/ui_focus.rs` | E20 | owner only |
| `tests/ui_menus.rs`, `tests/ui_settings_window.rs`, `tests/ui_shell_windows.rs` | E21 | owner only |
| `tests/ui_processes.rs`, `tests/ui_process_menu.rs`, `tests/ui_process_cells.rs`, `tests/golden/copy_*.txt` | E22 | owner only |
| `tests/ui_props.rs`, `tests/ui_dialogs_process.rs`, `tests/ui_run.rs`, `tests/ui_handle_search.rs` | E23 | owner only |
| `tests/ui_performance.rs`, `tests/ui_sidebar.rs`, `tests/ui_cpu_page.rs` | E24 | owner only |
| `tests/ui_device_pages.rs` | E25 | owner only |
| `tests/ui_minigraphs.rs`, `tests/ui_detached.rs` | E26 | owner only |
| `tests/ui_sys_table.rs`, `tests/ui_services.rs`, `tests/ui_service_props.rs` | E27 | owner only |
| `tests/ui_startup.rs`, `tests/ui_connections.rs`, `tests/ui_information.rs` | E28 | owner only |
| `tests/ui_setup_window.rs`, `tests/ui_shield.rs` | E29 | owner only |
| `tests/ui_history.rs` | E30 | owner only |
| `tests/ui_tray_bridge.rs` | E31 | owner only |
| `tests/ui_low_memory.rs` | E32 | owner only |
| `tests/bench_scenes.rs` | E01 | owner only |

### `crates/tl-app`

| Path | Owner | Rule |
|---|---|---|
| `Cargo.toml`, `src/main.rs` | E33 | chain edit: `--bench` and `--gallery` arms (E01), `--low-memory` and `--watch-memory` arms (E32), the tray start and the `tl-tray` dependency line (E31-008), each depends on E33-A1 (R-159, R-161) |
| `src/build.rs` | E33 | owner only (R-159) |
| `src/args.rs` (clap) | E33 | up front: every APP-052 argument is declared in E33-A1; the arms that handle them are chain edits in `main.rs` |
| `src/paths.rs`, `src/atomic.rs`, `src/settings_store.rs`, `src/migrations.rs`, `src/logging.rs`, `src/crash.rs`, `src/single_instance.rs`, `src/notify.rs`, `src/autostart.rs`, `src/import_ts.rs` (P4), `src/kglobalaccel.rs` (P4), `src/rendering_env.rs` (P4) | E33 | owner only |
| `src/bench_main.rs`, `src/gallery_main.rs` | E01 | owner only |
| `src/watch_memory.rs`, `src/lowmem_unit.rs` | E32 | owner only |
| `tests/settings_store.rs`, `tests/paths.rs`, `tests/crash_report.rs`, `tests/real_single_instance.rs` | E33 | owner only |
| `tests/watch_memory.rs` | E32 | owner only |

### P3 crates

| Path | Owner | Rule |
|---|---|---|
| `crates/tl-helper/Cargo.toml`, `src/main.rs`, `src/dispatch.rs`, `src/ops.rs`, `src/files.rs`, `src/dropin.rs`, `src/sockets.rs`, `tests/helper_proto.rs`, `tests/real_helper_root.rs` | E29 | owner only |
| `crates/tl-history/Cargo.toml`, `src/lib.rs`, `src/format.rs`, `src/block.rs`, `src/segment.rs`, `src/store.rs`, `src/rolling.rs`, `src/recorder.rs`, `src/read.rs`, `src/retention.rs`, `src/error.rs`, `tests/format_roundtrip.rs`, `tests/store_6h.rs`, `tests/rolling_window.rs`, `tests/recovery.rs` | E30 | owner only |
| `crates/tl-tray/Cargo.toml`, `src/lib.rs`, `src/item.rs`, `src/menu.rs`, `src/raster.rs`, `src/model.rs`, `tests/raster.rs` | E31 | owner only |
| `crates/tl-netmon/**` (P4) | not planned in this round; its ADR is written when P4 starts | |

## 3. Anchor beads

An anchor is a bead other epics may depend on. Cross-epic dependencies point at anchors only. An
anchor marked **P0** gets priority 0 because the phase-0 spike (E01) needs it. Within an epic,
later beads depend on earlier ones as the writer sees fit.

| Plan key | Title | Delivers | Phase |
|---|---|---|---|
| E01-A1 | Bench harness and `perf-cpu` scene run end to end | `tl-ui/src/bench/{mod,synth,stats,engine,scenes}.rs`, `BenchEngine`, `FrameStats` JSON writer, `tasklens --bench perf-cpu --duration --warmup --json`, `xtask bench` | P0 |
| E01-A2 | Phase-0 results and decision recorded | `docs/spike/phase0-results.md` with every gate's numbers, the checklist, the U-nn outcomes and the decision (pass, optimise, or fall back). Not a code dependency; the leader schedules P1 UI beads after it closes (see section 4) | P0 |
| E02-A1 | Workspace compiles | root `Cargo.toml` with all pins, profiles and lints; `rust-toolchain.toml`; `.cargo/config.toml`; skeleton crates `tl-model`, `tl-collect`, `tl-ops`, `tl-engine`, `tl-gpu-probe`, `tl-ui`, `tl-app`, `xtask` with empty roots; `cargo build --workspace --locked` passes | P0 |
| E02-A2 | `check-deps` and CI | `xtask check-deps` with its four checks, `scripts/ci.sh`, the GitHub workflow, the recorded failing-branch output | P0 |
| E03-A1 | Core types compile with size tests | `time.rs`, `ids.rs`, `validity.rs`, `series.rs`, `process.rs`, `system.rs`, `devices.rs`, `health.rs`, `units.rs`; `size_of` tests; frozen `MetricId` and `ColumnId` lists | P0 |
| E03-A2 | Contracts compile | `snapshot.rs`, `table.rs`, `details.rs`, `demand.rs`, `actions.rs`, `command.rs`, `settings.rs`, `wire.rs`, `gpu_proto.rs`, `helper_proto.rs`, `journal.rs`, `history.rs`, `client.rs`; defaults match canonical table (a) | P0 |
| E04-A1 | `SysRoot`, counters, fixture infrastructure | `sysroot.rs`, `fdcache.rs`, `counters.rs`, `parse/mod.rs`, the `owner-desktop` and `minimal-vm` fixtures, `xtask capture-fixture`, `insta` wiring, one proptest harness | P0 |
| E04-A2 | CPU sampler | `parse/stat.rs`, `parse/cpufreq.rs`, `parse/topology.rs`, `sample/cpu.rs` producing `CpuTotals`, per-CPU samples and `CpuTopology` with the ADR-008 math | P0 |
| E04-A3 | Memory, disk, net, sensors samplers | the other parsers and samplers with fixtures and proptests | P1 |
| E05-A1 | Process sweep | `sanitize.rs`, `proc/*` readers, `proc/sweep.rs` filling `ProcessRecord` and `FieldValidity`, metadata refresh, flags; fixture and proptests | P1 |
| E05-A2 | App grouping, naming, icons | `apps/*`: cgroup app id, desktop index, matching, naming, bwrap, icon resolution and cache | P1 |
| E05-A3 | Detail readers | `proc/{smaps,detail,strings,handles}.rs`, `hash.rs` | P2 |
| E06-A1 | Sockets | `net/{sock_diag,socket_map,attribution,connections}.rs` with fixtures | P2 |
| E06-A2 | Startup entries | `startup/*` parsers and watches | P2 |
| E06-A3 | Inventory readers | `inventory/*` (no DIMM serials) | P2 |
| E07-A1 | Verified process operations | `process.rs` (`open_verified`, signals, nice, sched, affinity, ioprio, thread targets, kill tree), `sys.rs`, `error.rs`, `tests/ops_real.rs` | P1 |
| E07-A2 | cgroup operations | `cgroup.rs` with `open_validated`, freeze, thaw, kill, the events wait; validation tests | P1 |
| E07-A3 | Atomic files and launch | `fs.rs` (`write_atomic`, autostart override), `launch.rs`, `dump.rs` | P2 |
| E08-A1 | Runtime, supervision, `Source` loop | `config.rs`, `runtime.rs`, `supervise.rs`, `controller.rs`, `source.rs`, `update.rs`, `workers/mod.rs`, `lib.rs`; LabRuntime tests for backoff, budget, cancel, demand rates | P0 |
| E08-A2 | D-Bus helper and logind worker | `dbus/mod.rs`, `workers/logind.rs` | P1 |
| E09-A1 | Engine publishes snapshots | `session.rs`, `reducer/*` with the full `ProcessTableView` builder and the `sys_table.rs` stub, `bridge.rs` with `EngineHandle` implementing `EngineClient`, `Engine::start`; tests: coalescing, sequence, session restart, 10,000-publish bridge test | P0 |
| E09-A2 | Shutdown | `EngineHandle::shutdown` with the five steps and `real_shutdown.rs` | P1 |
| E10-A1 | Processes worker | `workers/processes.rs` publishing `ProcessSnapshot` at `processes.interval_ms` with the hidden policy | P1 |
| E10-A2 | Process detail worker | `workers/process_detail.rs`: PSS and USS for `detail_keys`, Properties payloads, hashes, affinity masks, string scan | P1 (PSS), P2 (rest) |
| E10-A3 | Handle search | `workers/handle_search.rs` with progress and cancel | P3 |
| E11-A1 | Action pipeline for processes | `actions/mod.rs` admission and events, `actions/process.rs` for every process kind, `lab_actions.rs` (17th action is `Busy`, timeout is `OutcomeUnknown`) | P1 |
| E11-A2 | cgroup, unit and startup actions | `actions/{cgroup,unit,startup}.rs` with the deadline table | P1 (cgroup), P2 (unit, startup) |
| E12-A1 | CPU worker | `workers/cpu.rs` at 10 Hz with `cpu_fast`, publishing `CPU_TOTAL`, `CPU_KERNEL`, per-CPU series, `CPU_FREQ`, `CPU_TEMP` | P0 |
| E12-A2 | Memory, disk, net, sensors workers and enumeration | `workers/{memory,disk,net,sensors,enumerate}.rs` | P1 |
| E12-A3 | Inventory and the P2 D-Bus device workers | `workers/{inventory,dbus_udisks,dbus_upower,dbus_nm}.rs` | P2 |
| E13-A1 | Probe binary | `tl-gpu-probe` with `Hello`, `Sample`, `Shutdown`, `--fake`; framing tests | P1 |
| E13-A2 | `GpuNvidia` worker | `gpu/{spawn,client}.rs`, `workers/gpu_nvidia.rs`, failure table, pid mapping input | P1 |
| E14-A1 | systemd workers and system table rows | `workers/dbus_systemd.rs`, the `sys_table.rs` body for all three tables, `ServicesSnapshot` | P2 |
| E14-A2 | KWin bridge | `workers/dbus_kwin.rs`, the scripts, `WindowsSnapshot`, `KWinCommand` handling | P2 |
| E14-A3 | Startup and connections workers, launcher, opener | `workers/{startup,connections}.rs`, `launcher.rs`, `opener.rs` | P2 |
| E14-A4 | Journal | `workers/journal.rs`, `EngineClient::journal` | P2 |
| E15-A1 | Metrics, text, icons and base controls | `theme/metrics.rs` use, `text/*`, `components/{focus,icons,button,checkbox,switch,input,tabs,tooltip}.rs`; U-7, U-41, U-42 recorded | P0 |
| E15-A2 | Display components | `components/{validity,kv_grid,badge,empty,skeleton,progress,banner,link,radio,slider,stepper,select}.rs` | P0 |
| E15-A3 | Gallery and render tests | `components/gallery.rs`, `tests/render_gallery.rs`, golden workflow; U-4 recorded | P1 |
| E16-A1 | App core | `lib.rs`, `app.rs`, `hub.rs`, `demand.rs`, `i18n.rs`, `testing/fake_engine.rs`, `window/mod.rs`, `pages/mod.rs`, `windows/mod.rs`; hub and demand tests | P0 |
| E16-A2 | Windows, dialogs, toasts | `components/{toast,dialog,window_frame}.rs`, `window/registry.rs` with `WindowKey` | P1 |
| E16-A3 | Menus, shield, action tracker | `components/{menu,shield}.rs`, `action_tracker.rs` with `message()` | P1 |
| E17-A1 | Theme model | `theme/{mod,builtin,derived,map,metrics,palette,store}.rs`, `assets/themes/builtin.json` with Default Dark and Default Light, `UiMetrics`; U-8 recorded | P0 |
| E17-A2 | User themes and follow-system | `theme/user_store.rs`, follow-system switching, `theme/preview.rs` | P2 |
| E18-A1 | Generic tree table | `table/{tree_table,columns,selection,cells,actions}.rs`, `text_cache.rs`, one-line header; selection and render-counter tests | P0 |
| E18-A2 | Two-line header, picker and persistence | `table/header.rs` two-line mode, resize, reorder, sort, header menu, `columns_window.rs`, column state by id | P0 |
| E19-A1 | Chart element | `chart/{element,view,geometry,grid,scale}.rs`, `anim.rs`; geometry and driver tests | P0 |
| E19-A2 | Grid element, labels, hover | `chart/{grid_element,labels,hover}.rs` | P0 (grid, labels), P2 (hover) |
| E20-A1 | Main window frame | `window/{main,titlebar,toolbar,tabstrip,status}.rs`, `actions.rs` with `bind_keys_global`, `focus.rs`; U-44 recorded | P0 |
| E20-A2 | Filter box and settings global | `window/filter.rs`, `settings_global.rs`, type-to-filter routing; U-45 recorded | P1 |
| E21-A1 | Menu bar | `window/menubar.rs` and the five menu files with every P1 item | P1 |
| E21-A2 | Settings window | `window/settings/*` with all eleven pages | P2 |
| E22-A1 | Process table renders | `pages/processes/{mod,table,delegate,name_cell,cells,header_totals,actions}.rs` driven by the fake engine | P0 |
| E22-A2 | Row menu, confirmations, toasts | `pages/processes/{menu,confirm,toasts}.rs` with the P1 items | P1 |
| E22-A3 | Jump target | `pages/processes/jump.rs` with `select_by_key(key, source)` | P1 |
| E23-A1 | Properties window and General tab | `windows/frame.rs`, `windows/props/{mod,general,filter}.rs`, `windows/actions.rs` | P2 |
| E23-A2 | Action dialogs | `windows/{affinity,priority,signal}.rs` | P2 |
| E23-A3 | Notices | `windows/notices.rs` (dump and VirusTotal toasts) | P2 |
| E24-A1 | Performance tab with CPU page | `pages/performance/{mod,page,sidebar,card,details,cpu,cpu_grid,cpu_details,actions}.rs` | P0 |
| E24-A2 | Sidebar behaviour | selection, keys, hotplug, visibility, edit mode, menus, splitter, summary mode | P1 |
| E25-A1 | Device pages | `pages/performance/{memory,memory_bar,disk,net,gpu}.rs` | P1 |
| E26-A1 | Minigraph panel and cards | `pages/minigraph/{mod,panel,card,menu,actions,sources}.rs` | P1 |
| E26-A2 | Detached graphs and mini window | `windows/{detached_graph,mini}.rs` | P3 |
| E27-A1 | System table frame | `pages/{sys_table,sys_copy,sys_status}.rs` with the query pattern, skeleton, empty states, copy commands | P2 |
| E27-A2 | Services page | `pages/services/*` | P2 |
| E27-A3 | Service Properties | `windows/service_properties/*` | P2 |
| E28-A1 | Startup Apps page | `pages/startup/*`, `windows/startup_properties.rs`, `windows/confirm_startup_delete.rs` | P2 |
| E28-A2 | Connections page | `pages/connections/*`, `windows/connection_properties.rs` | P2 |
| E28-A3 | Information page | `pages/information/*` | P2 |
| E29-A1 | Helper binary | `tl-helper` with the allowlist and protocol tests | P3 |
| E29-A2 | Helper session in the engine | `helper/*`, `actions/helper.rs`, `privileged` routing, shield state in health | P3 |
| E29-A3 | Additional Setup window | `window/setup.rs`, `tasklens-setup`, the policy file | P3 |
| E30-A1 | History crate | `tl-history` format, store, rolling window | P3 |
| E30-A2 | Recorder and query | `workers/history.rs`, `history_query.rs`, `RecordSink` wiring | P3 |
| E30-A3 | History mode UI | `pages/processes/history/*` | P3 |
| E31-A1 | Tray crate | `tl-tray` with `raster.rs`; U-31 recorded | P3 |
| E31-A2 | Tray bridge | `tray_bridge.rs` with `TrayModel` and `TrayEvent` | P3 |
| E32-A1 | Memory watcher | `watch_memory.rs`, `lowmem_unit.rs` | P3 |
| E32-A2 | Rescue window and low-memory engine | `workers/lowmem.rs`, `windows/low_memory.rs` | P3 |
| E33-A1 | Binary starts | `main.rs` steps 1 to 4 and 7 to 10 (panic hook, logging, args, X11 rule, engine start, `Application::run`, main window), `paths.rs`, `logging.rs`, `crash.rs`, `args.rs` | P0 |
| E33-A2 | Settings store | `settings_store.rs`, `migrations.rs`, `atomic.rs`, persist callback, `single_instance.rs`, `notify.rs` | P1 |
| E33-A3 | Packaging | `PKGBUILD`, desktop file, asset install, `--version` | P1 |

## 4. Epic dependency graph

Each row lists the anchors an epic's beads may depend on. A bead depends only on the anchors it
compiles or tests against, so most beads use a subset of their epic's row. The graph has no cycle:
the order E02, E03, E17, E15, E04, E05, E06, E07, E08, E09, E10, E12, E13, E14, E11, E16, E18,
E19, E20, E21, E22, E23, E24, E25, E26, E27, E28, E29, E31, E33, E32, E01, E30 is a topological
order (every epic appears after everything it depends on; E30 comes after E01 only because of
the `history-scrub` scene file).

Named exceptions to the anchors-only rule, each recorded in RECONCILIATION: E33-008 and E33-014
depend on E07-010 (`write_atomic`, P1; R-146); E33-A1, E33-011 and E33-012 depend on E16-012
(`tl_ui::run`, R-153); E12-027 depends on E06-023 (R-164); E29-014 depends on E11-002 (R-147).

| Epic | Depends on anchors |
|---|---|
| E02 | none |
| E03 | E02-A1 |
| E04 | E02-A1, E03-A1 |
| E05 | E02-A1, E03-A1, E04-A1 |
| E06 | E02-A1, E03-A1, E04-A1 |
| E07 | E02-A1, E03-A1, E03-A2 |
| E08 | E02-A1, E03-A2 |
| E09 | E03-A2, E08-A1, E08-A2 |
| E10 | E05-A1, E05-A2, E05-A3, E08-A1, E09-A1 |
| E11 | E07-A1, E07-A2, E07-A3, E08-A1, E09-A1, E14-A1 (unit actions), E14-A3 (launcher) |
| E12 | E04-A2, E04-A3, E04-A4, E06-A3, E08-A1, E08-A2, E09-A1 |
| E13 | E03-A2, E08-A1, E09-A1 |
| E14 | E05-A1, E05-A2, E06-A1, E06-A2, E07-A3, E08-A1, E08-A2, E09-A1 |
| E15 | E02-A1, E03-A1, E17-A1 |
| E16 | E03-A2, E15-A1, E15-A2, E15-A3, E17-A1 |
| E17 | E02-A1, E03-A1, E03-A2, E19-A1 |
| E18 | E03-A2, E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E17-A1 |
| E19 | E03-A1, E15-A1, E16-A1, E17-A1 |
| E20 | E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E17-A1 |
| E21 | E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E17-A1, E17-A2, E20-A1, E20-A2 |
| E22 | E01-A1, E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E18-A1, E18-A2, E19-A1, E20-A1, E20-A2 |
| E23 | E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E18-A1, E20-A1, E22-A1, E22-A2, E22-A3 |
| E24 | E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E17-A1, E18-A1, E19-A1, E19-A2, E20-A1, E20-A2 |
| E25 | E04-A4, E16-A2, E18-A1, E19-A1, E19-A2, E24-A1, E24-A2 |
| E26 | E16-A2, E16-A3, E19-A1, E20-A1, E22-A1, E22-A2, E24-A1 |
| E27 | E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E18-A1, E18-A2, E20-A1, E20-A2, E22-A1, E22-A2, E22-A3, E23-A1 |
| E28 | E03-A2, E15-A1, E15-A2, E16-A1, E16-A2, E16-A3, E18-A1, E18-A2, E20-A1, E20-A2, E22-A3, E25-A1, E27-A1, E27-A2, E29-A2, E29-A3 |
| E29 | E03-A2, E07-A1, E07-A2, E09-A1, E10-A2, E10-A3, E11-A1, E16-A2, E16-A3, E20-A1, E21-A2, E23-A1 |
| E30 | E01-A1 (scene), E03-A2, E09-A1, E10-A1, E18-A1, E19-A1, E20-A1, E21-A2, E22-A1 |
| E31 | E02-A1, E03-A2, E16-A1, E20-A1, E21-A2, E26-A1 |
| E32 | E05-A1, E07-A1, E08-A1, E09-A1, E10-A1, E11-A1, E16-A2, E18-A1, E21-A2, E33-A1 |
| E33 | E03-A2, E09-A1, E09-A2, E15-A1, E16-A1, E17-A1, E20-A1 |
| E01 | E02-A1, E02-A2, E03-A1, E03-A2, E04-A1, E04-A2, E08-A1, E09-A1, E12-A1, E15-A1, E15-A2, E16-A1, E17-A1, E18-A1, E18-A2, E19-A1, E19-A2, E20-A1, E22-A1, E24-A1, E33-A1 |

Notes on the graph:

- **The phase-0 slice.** The spike measures the real chart element, tree table, pages and bridge
  with synthetic data, so E01 depends on 21 anchors across 13 epics. Those anchors are the P0
  beads. Everything else in those epics is P1 or later.
- **The phase-0 gate is a scheduling rule, not a dependency.** ADR-014 says the results file is
  committed before any P1 UI bead starts. Making every P1 UI bead depend on E01-A2 would create a
  cycle (E01 depends on the P0 anchors of the same epics). Instead, bead priority does it: P0
  beads have priority 0, and the leader starts P1 UI beads only after E01-A2 closes. Writers do
  not add E01-A2 as a dependency.
- **E11 depends on E14-A1 and E14-A3**, because unit actions run on the systemd worker's bus
  connection and startup "Run now" uses the shared launcher. E14 does not depend on E11.
- **E15 depends on E17-A1** (components read `TlTheme` and `UiMetrics`), and E17-A1 depends on
  nothing in the UI. The theme editor and share windows live in E21 for this reason.
- **E16 depends on E15** (window frames use `TlButton`), and E15 never uses E16's components.
  E16's six components add their own gallery pages by chain edit.
- **Page epics depend on E20-A1, never the reverse.** `MainWindow`'s page match and toolbar
  content are chain edits made by the page epics.
- **E23 depends on E22**, never the reverse. The row-menu items that open Properties and the
  three dialogs are E23 beads that chain-edit `pages/processes/menu.rs` and register handlers in
  `pages/processes/mod.rs`.
- **E27 and E28 depend on E22-A3** (jump to process) and E27 on E22-A1 and E22-A2 (service child
  rows reuse the process delegate cells and row menu). "Go to service" is an E27 bead.
- **E26 depends on E22-A1** (the panel mounts in the Processes page) and E24-A1 (`DeviceCard`
  for the mini window). The minigraph row-menu items are E26 beads.
- **E31 depends on E26-A1** (tray graph sources are minigraph sources); "Show in tray" is an E31
  bead. E30 depends on E01-A1 only for the `history-scrub` scene file.
- **E32 depends on E33-A1** (argument arms) and E33 never depends on E32.

## 5. Assignment to bead writers

Eight batches. Each batch is one bead-writing agent. No two batches own the same file; the only
cross-batch touches are the chain edits listed in section 2, each made in a bead that depends on
the owning anchor. Sizes are the section 1 estimates.

| Batch | Plan file | Epics | Size | Why these together |
|---|---|---|---|---|
| B1 `infra` | `docs/plan/b1-infra.md` | E01, E02, E03, E31, E32, E33 | 117 | workspace, contracts, the binary, the spike harness and the two small P3 crates that sit beside the binary (tray, low memory) |
| B2 `collect-ops` | `docs/plan/b2-collect-ops.md` | E04, E05, E06, E07 | 100 | all synchronous system access: parsers, samplers, fixtures, verified operations |
| B3 `engine-core` | `docs/plan/b3-engine-core.md` | E08, E09, E10, E11, E13 | 100 | runtime, reducer, bridge, process workers, actions, the GPU probe |
| B4 `engine-workers` | `docs/plan/b4-engine-workers.md` | E12, E14, E29, E30 | 115 | the remaining workers, the D-Bus family, and the two P3 features that are mostly engine work (helper, history) with their UI parts |
| B5 `ui-foundation` | `docs/plan/b5-ui-foundation.md` | E15, E16, E17, E18 | 110 | tokens, components, app core, theme, tree table |
| B6 `ui-performance` | `docs/plan/b6-ui-performance.md` | E19, E24, E25, E26 | 120 | chart element and every surface that draws charts |
| B7 `ui-shell` | `docs/plan/b7-ui-shell.md` | E20, E21, E23 | 110 | main window, menus, settings, secondary windows and dialogs |
| B8 `ui-tables` | `docs/plan/b8-ui-tables.md` | E22, E27, E28 | 120 | the process table page and the three system table pages with the Information tab |

Rules for writers:

- A writer creates the epic beads for its epics first (`br create -t epic`), then the anchors,
  then the rest, and keeps the plan-key to bead-ID table in its plan file.
- A dependency on another batch's anchor is written as the plan key (for example `E18-A1`); the
  leader resolves plan keys to bead IDs with `br dep add` after every batch has finished, from the
  eight plan files. Dependencies inside a batch are added by the writer directly.
- A chain edit to another batch's file is one bead, named "<epic>: add <part> to <file>", with
  the owning anchor as its dependency and the exact lines to add in its Deliverable.
- A writer who finds a value the docs do not decide stops that bead and lists it under "Open
  items" in its plan file with a recommendation. It does not invent and it does not edit the docs.

## 6. Coverage

Every spec ID, UI ID and ADR section maps to a primary epic and, where other epics carry part of
it, secondary epics in parentheses. Verdicts and phases are the merged specs' (see
`docs/ui/INDEX.md`). Dropped IDs are listed so nothing is missing; they produce no bead.

### PROC

| IDs | Epic (secondary) |
|---|---|
| PROC-001 | E03 (E07, E22) |
| PROC-002 | E05 (E10) |
| PROC-003 | E03 (E22) |
| PROC-004 | E05 (E09) |
| PROC-005, PROC-006, PROC-007 | E05 |
| PROC-008 | E09 (E22, E14 for window titles) |
| PROC-009 | E09 (E22) |
| PROC-010, PROC-011 | E05 |
| PROC-012 | E09 (E22) |
| PROC-013 | E18 |
| PROC-014 | E09 (E21, E22) |
| PROC-015, PROC-016 | E22 (E20 slot) |
| PROC-017 | E22 (E21 menu item) |
| PROC-018, PROC-019 | E09 (E22) |
| PROC-020 | E18 (E21) |
| PROC-021 | E22 |
| PROC-022 | E03 (E18, E22) |
| PROC-023 | E22 (E18) |
| PROC-024, PROC-025, PROC-026 | E18 (E33 state store for 025) |
| PROC-027 to PROC-031 | E05 data (E10, E22 cells) |
| PROC-032 | E06 (E14, E22) |
| PROC-033 | E13 (E22) |
| PROC-034 | E22 (E05 estimate) |
| PROC-035, PROC-036 | E05 (E22) |
| PROC-037 | E03 (no UI) |
| PROC-038 | E18 header (E09 totals, E22 text) |
| PROC-039 | E22 |
| PROC-040 | E22 (E17 `value_heat`) |
| PROC-041 | E22 |
| PROC-042 | E18 |
| PROC-043 | E22 (E09 flags) |
| PROC-044 | E22 (E20 filter box, E09 filter) |
| PROC-045 | E22 (E20, E09) |
| PROC-046 | E22 (E09) |
| PROC-047 | E09 (E18) |
| PROC-048 | E18 |
| PROC-049 | E18 (E22) |
| PROC-050 | E22 (E20 chip and key) |
| PROC-051 | E22 |
| PROC-052 | E14 |
| PROC-053 | E22 (E14) |
| PROC-054 | E22 (E14) |
| PROC-055 | E20 button (E14 picker, E22 select) |
| PROC-056 | E16 (E14, E20) |
| PROC-057 | E22 (E23, E26, E27, E31 items) |
| PROC-058 | E11 (E16, E22) |
| PROC-059 | E22 confirmations (E05 flags, E11 admission) |
| PROC-060 to PROC-063 | E11 (E22) |
| PROC-064 | E11 (E07, E22) |
| PROC-065 | E23 (E11) |
| PROC-066, PROC-067 | E23 dialogs (E22 submenu, E11, E07, E10 masks) |
| PROC-068 | E22 (E11, E07) |
| PROC-069 | E22 (E23) |
| PROC-070 | E22 (E18 rows-as-table) |
| PROC-071 | E22 (E27 Go to service) |
| PROC-072 | E22 (E11) |
| PROC-073 | E22 (E14 opener) |
| PROC-074 | E23 (E05 hash, E10) |
| PROC-075 | E23 toasts (E11, E07) |
| PROC-076 | E29 |
| PROC-077 to PROC-084 | E23 (E10 payloads, E05 readers) |
| PROC-085 | E23 (E10) |
| PROC-086 to PROC-092 | E30 |
| PROC-093 | E26 |
| PROC-094 | E31 |
| PROC-095 | E23 (E07 launch, E14 launcher, E33 history file path) |
| PROC-096 | E23 (E10, E05, E29 helper path) |
| PROC-097, PROC-098 | E32 |
| PROC-099 | E03 schema (every feature epic for its keys) |

### PERF

| IDs | Epic (secondary) |
|---|---|
| PERF-001, PERF-002, PERF-003 | E24 |
| PERF-004 | E04 classification (E12, E24, E21 settings page) |
| PERF-005 | E09 catalog (E03, E24) |
| PERF-006, PERF-007 | E24 (E21 View submenu for 007) |
| PERF-008 | E24 (E25 page strings) |
| PERF-009 | E25 |
| PERF-010, PERF-011, PERF-012 | E24 |
| PERF-013 | E15 (E03) |
| PERF-014 | E24 (E20 slot) |
| PERF-015 | E19 |
| PERF-016 | E19 (E09 tiers, E21) |
| PERF-017 | E09 (E03) |
| PERF-018 to PERF-021 | E19 |
| PERF-022 | E19 (E09) |
| PERF-023 | E19 |
| PERF-024 | E17 (E19, E26 per-card) |
| PERF-025 | E03 formats (E21 page, E24) |
| PERF-026 | E19 |
| PERF-027 to PERF-032 | E24 (E04, E12 data) |
| PERF-033 to PERF-040 | E24 (E04, E12; the PERF-037 readers are E06's, R-162) |
| PERF-041, PERF-042 | E25 (E12) |
| PERF-043 to PERF-045 | E25 (E04) |
| PERF-046 | E25 (E06 inventory) |
| PERF-047 to PERF-050 | E25 (E04, E12) |
| PERF-051, PERF-052 | E25 windows (E12 udisks, E29 smartctl fallback) |
| PERF-053 | E25 (E12) |
| PERF-054 to PERF-057 | E25 (E04, E12) |
| PERF-058, PERF-059 | E25 (E12 NM) |
| PERF-060 | E12 fetch (E25 row, E21 notice) |
| PERF-061 | E25 |
| PERF-062 | E13 (E06 non-NVIDIA) |
| PERF-063 to PERF-071 | E25 (E13) |
| PERF-072, PERF-073 | E25 (E06 displays) |
| PERF-074, PERF-075 | E12 worker (E06 readers, E25) |
| PERF-076, PERF-077 | E25 (E04, E12, E21 fan settings) |
| PERF-078, PERF-079 | E25 (E12 UPower) |
| PERF-080 to PERF-087 | E26 (E21 View > Graphs for 080) |
| PERF-088 to PERF-092 | E31 |
| PERF-093 | E26 |
| PERF-094 to PERF-104 | E28 (E06, E12 data) |
| PERF-105, PERF-106 | E29 |
| PERF-107 | E29 (row text only; the capability step is dropped) |
| PERF-108 | E29 row (E04 detection, E25 page) |

### SYS

| IDs | Epic (secondary) |
|---|---|
| SYS-001 to SYS-004 | E27 (E14) |
| SYS-005 | E14 (E27, E11) |
| SYS-006, SYS-007 | E11 (E27) |
| SYS-008, SYS-009 | E27 |
| SYS-010 | E27 (E22-A3) |
| SYS-011 to SYS-014 | E27 (E14 details, E11 recovery write, E29 drop-in) |
| SYS-015 | E27 Logs tab (E14 journal) |
| SYS-016, SYS-017 | E27 |
| SYS-018 | E14 (E27 variant) |
| SYS-019 | E06 (E14) |
| SYS-020, SYS-021 | E28 (E06) |
| SYS-022 to SYS-024 | E11 actions (E07, E14 launcher, E28 flows) |
| SYS-025, SYS-026, SYS-027 | E28 |
| SYS-028 | E06 (E14) |
| SYS-029 | E28 (E06 formats) |
| SYS-030 | E06 (E14, E29 helper) |
| SYS-031 | E29 (E11, E28 flow) |
| SYS-032, SYS-033, SYS-034 | E28 |
| SYS-035 | E28 (E06 rdns) |
| SYS-036 | E28 |
| SYS-037 | E28 (E06, E14) |

### APP

| IDs | Epic (secondary) |
|---|---|
| APP-001, APP-002 | E20 |
| APP-003 | E29 (E16 `TlShield`) |
| APP-004, APP-005 | E22 (E20 slot) |
| APP-006 | E20 (E14) |
| APP-007 | dropped |
| APP-008 | E20 |
| APP-009 | E16 (E20, E14) |
| APP-010 | E20 (E33 state) |
| APP-011 | E33 (E20 UI side) |
| APP-012 | E21 |
| APP-013 | dropped |
| APP-014, APP-015 | E23 |
| APP-016 to APP-024 | E21 |
| APP-025 | dropped |
| APP-026 | E21 (E20 FPS overlay) |
| APP-027, APP-028, APP-029 | E20 |
| APP-030 | E21 |
| APP-031 | E20 keymap (E18 table keys, every page epic for its context) |
| APP-032 | E03 schema (E21 pages, E33 store) |
| APP-033 | E33 |
| APP-034 | E33 (E17 themes) |
| APP-035 | E17 |
| APP-036 | E17 (E21 menu) |
| APP-037, APP-038 | E21 windows (E17 preview and codec) |
| APP-039 | E17 (E21 submenu) |
| APP-040 | E21 (E15 metrics, E20 runtime change) |
| APP-041 | E28 (E21 Help item) |
| APP-042 | E29 |
| APP-043 | E16 (E21 submenu, E20 live switch) |
| APP-044, APP-045 | E16 |
| APP-046 | E33 |
| APP-047 | E31 (E20 close rows) |
| APP-048 | E33 (E21 setting) |
| APP-049 | E29 |
| APP-050 | dropped |
| APP-051 | E33 |
| APP-052 | E33 (E01, E32 arms) |
| APP-053 | E21 |
| APP-054 | E33 |
| APP-055 | E33 (E21 window) |
| APP-056 | E21 window (E12 fetch) |
| APP-057 | dropped |
| APP-058 | E15 (E20) |
| APP-059 | E15 skeletons (E20 frame rule, E27, E28) |

### UI-FND

| IDs | Epic |
|---|---|
| UI-FND-001, UI-FND-002 | E17 |
| UI-FND-003 | E15 (`UiMetrics` struct in E17-A1) |
| UI-FND-010 to UI-FND-019 | E15 |
| UI-FND-020, UI-FND-021, UI-FND-022 | E16 |
| UI-FND-023 to UI-FND-027 | E15 |
| UI-FND-028 | E16 |
| UI-FND-029, UI-FND-030, UI-FND-031, UI-FND-032 | E15 |
| UI-FND-040 to UI-FND-043 | E17 |
| UI-FND-050 to UI-FND-053 | E15 |

### UI-SHL

| IDs | Epic (secondary) |
|---|---|
| UI-SHL-001 to UI-SHL-007 | E20 |
| UI-SHL-010 | E20 |
| UI-SHL-011 | E29 |
| UI-SHL-012, UI-SHL-013 | E22 |
| UI-SHL-014 | E20 (E14) |
| UI-SHL-020 to UI-SHL-022 | E20 |
| UI-SHL-023 | E20 (E30 toggle behaviour, E28 copy icon) |
| UI-SHL-024, UI-SHL-025 | E20 |
| UI-SHL-030 to UI-SHL-042 | E21 |
| UI-SHL-050, UI-SHL-051 | E20 |
| UI-SHL-060 to UI-SHL-072 | E21 |
| UI-SHL-080 | E17 |
| UI-SHL-081 to UI-SHL-085 | E21 (E17 codec and palette) |
| UI-SHL-090 | E20 (E16 frame) |
| UI-SHL-091, UI-SHL-092, UI-SHL-093 | E21 (E33 report files) |
| UI-SHL-094 | E29 |
| UI-SHL-095, UI-SHL-096 | E21 |
| UI-SHL-097, UI-SHL-098 | E23 |
| UI-SHL-099 | E21 |
| UI-SHL-100 | E16 (E20 placement) |
| UI-SHL-110 | E33 (E20 steps 8 to 11) |
| UI-SHL-111 | E20 |
| UI-SHL-112 | E33 (E20 UI side) |
| UI-SHL-113 | E20 rows 2, 3 and Quit (E31 row 1 and tray) |
| UI-SHL-114 | E33 |
| UI-SHL-120 to UI-SHL-124 | E20 |
| UI-SHL-130 | E20 |
| UI-SHL-131 | E16 (E20, E21) |
| UI-SHL-132 | E20 |
| UI-SHL-140 | E20 |

### UI-PRC

| IDs | Epic (secondary) |
|---|---|
| UI-PRC-001 | E18 |
| UI-PRC-002, UI-PRC-003, UI-PRC-004 | E22 |
| UI-PRC-005 | E18 header (E22 totals) |
| UI-PRC-006 to UI-PRC-010 | E18 |
| UI-PRC-011 to UI-PRC-020 | E22 (E23, E26 items) |
| UI-PRC-021 | E23 |
| UI-PRC-022 | E22 |
| UI-PRC-023 | E22 (E18, E01 scene) |
| UI-PRC-024 | E22 |
| UI-PRC-030 to UI-PRC-037 | E30 |

### UI-PWN

| IDs | Epic |
|---|---|
| UI-PWN-001 to UI-PWN-031 | E23 |
| UI-PWN-032, UI-PWN-033 | E32 |
| UI-PWN-034, UI-PWN-035, UI-PWN-036 | E23 |

### UI-PRF

| IDs | Epic (secondary) |
|---|---|
| UI-PRF-001 to UI-PRF-007 | E19 |
| UI-PRF-008 | E17 (E19) |
| UI-PRF-009 | E19 |
| UI-PRF-010 to UI-PRF-016 | E24 |
| UI-PRF-020 to UI-PRF-025 | E24 |
| UI-PRF-026 to UI-PRF-036 | E25 |
| UI-PRF-040 to UI-PRF-045 | E26 (E31 tray item in 042) |
| UI-PRF-050, UI-PRF-051 | E31 |
| UI-PRF-052 | E29 |
| UI-PRF-053 | E01 |

### UI-SYS

| IDs | Epic |
|---|---|
| UI-SYS-001 to UI-SYS-007 | E27 |
| UI-SYS-010 to UI-SYS-024 | E27 |
| UI-SYS-030 to UI-SYS-038 | E27 |
| UI-SYS-040 to UI-SYS-050 | E28 |
| UI-SYS-060 to UI-SYS-070 | E28 |
| UI-SYS-080 to UI-SYS-096 | E28 |

### ADR sections

| ADR | Part | Epic |
|---|---|---|
| ADR-001 | layout, dependency rules, `check-deps` | E02 |
| ADR-002 | toolchain, pins, profiles, lints | E02 |
| ADR-003 | all types | E03 |
| ADR-004 | runtime, supervisor, worker loop, D-Bus workers | E08 |
| ADR-004 | task tree, session, reducer, shutdown | E09 |
| ADR-004 | worker list | E10, E12, E13, E14, E30 (each its rows) |
| ADR-004 | KWin bridge worker | E14 |
| ADR-004 | engine modes | E08 (`EngineMode`), E32 (`LowMemory` behaviour) |
| ADR-005 | `EngineClient`, action types, `Command` | E03 |
| ADR-005 | bridge, publish, channels | E09 |
| ADR-005 | UI receiver, `ActionTracker` | E16 |
| ADR-005 | journal | E14 |
| ADR-006 | `tl-ops` code | E07 |
| ADR-006 | admission, tasks, deadlines, systemd units, protected rules, confirmations list | E11 (E22 for the dialog texts) |
| ADR-006 | helper boundary | E29 |
| ADR-007 | probe, spawn, request rules, failure table | E13 |
| ADR-007 | framing and messages | E03 |
| ADR-008 | `Demand` | E03 (E16 aggregator) |
| ADR-008 | schedule, tick semantics, counter rules, suspend | E08 (loop), E09 (suspend), E04 (counters), E12 (rates) |
| ADR-008 | CPU and memory math | E04 |
| ADR-009 | entity structure | E16 |
| ADR-009 | chart element, animation driver | E19 |
| ADR-009 | tree-table, text caching | E18 (E22 for row rendering rules) |
| ADR-009 | theme model | E17 |
| ADR-009 | metrics, fonts, icons | E15 (E17 `UiMetrics`) |
| ADR-009 | accessibility and input, motion settings | E20 |
| ADR-010 | backend, decorations, size, close | E20 (E33 env rule) |
| ADR-010 | single instance | E33 |
| ADR-010 | tray | E31 |
| ADR-010 | zed#64499 mitigation, hidden state, checklist | E01 (E19 stall detection, E16 hidden timer) |
| ADR-011 | paths, load, save | E33 |
| ADR-011 | schema | E03 |
| ADR-012 | all, including the `RecordSink` hook | E30 (E09 holds the hook as `None`) |
| ADR-013 | layers, `ci.sh`, static checks | E02 |
| ADR-013 | fixtures | E04 |
| ADR-013 | LabRuntime cases | E08, E09, E11 |
| ADR-013 | UI tests, fake engine | E16 |
| ADR-013 | headless screenshots, goldens | E15 |
| ADR-013 | real-system tests | the owner of each tested code path |
| ADR-014 | all | E01 |
| ADR-015 | logging, crash handling | E33 |
| ADR-015 | errors (`message`) | E16 (E03 for `ActionError`) |
| ADR-015 | untrusted text | E05 (E15 display rules) |
| ADR-015 | exports and copy | E22, E27, E28 (E18 rows-as-table) |
| ADR-016 | parsers, FD policy, sources table | E04, E05, E06 |
| ADR-016 | services, journal, startup, connections rows | E14 |
| ADR-016 | icons | E05 |
| Open questions 1 to 10 | | E01 (1, 4, 5, 6, 9, 10), E15 (2, 3), E17 (8), E13 (7) |

### Unverified list (U-1 to U-53)

| U | Settled by |
|---|---|
| U-1 | E02 |
| U-2, U-3, U-4, U-7, U-9, U-10, U-36, U-39, U-47 | E01 |
| U-5, U-6 | E20 (documented, not testable) |
| U-8 | E17 |
| U-11, U-22 | E30 |
| U-12, U-35 | E13 |
| U-13, U-14, U-49 | E14 |
| U-15 | E23 (Browse) |
| U-16 | E33 |
| U-17, U-18 | E17 |
| U-19, U-21, U-34 | E05 |
| U-20 | E04 |
| U-23, U-26, U-27, U-29, U-32, U-53 | E06 |
| U-24, U-50, U-51 | E32 |
| U-25, U-30, U-37 | E12 |
| U-28, U-52 | E04 |
| U-31 | E31 |
| U-33 | E22 (first copy bead) |
| U-38 | E03 |
| U-40 | E15 |
| U-41, U-42 | E15 |
| U-43 | E29 |
| U-44, U-45 | E20 |
| U-46 | E24 |
| U-48 | E27 |

## 7. Open items (all settled)

The sixteen items below were open when this file was first written. The leader accepted every
recommendation on 2026-10-03. Items 1 to 7, 13 and 16 are resolutions R-129 to R-137 in
`docs/RECONCILIATION.md` and are current-state text in `docs/ARCHITECTURE.md`; items 8 to 12, 14
and 15 are the accepted structural choices in R-138. The text of each item is kept so writers can
see the reasoning; the R entry is the binding form. Nothing here is open.

The open items of the bead round (2026-10-04) are settled in `docs/plan/open-items-engine.md`
(OI-E01 to OI-E32) and `docs/plan/open-items-ui.md` (OI-U01 to OI-U65). Their binding form is
R-139 to R-170 and R-171 to R-235 in `docs/RECONCILIATION.md`; the owner answers are D-012 to
D-020. New beads from that round: E04-030, E04-031, E04-A4, E04-032, E05-030, E05-031, E09-029,
E09-030, E15-026, E18-017, E20-029, plus the E26 `AddRandomMinigraphs` bead (R-238) and the E22 window-pick result bead (R-239). Leader entries R-236 to R-239 followed on the same day.

1. **Settled, R-129. Two files called `tl-ui/src/actions.rs`.** ADR-009 puts `ActionTracker` there; shell.md puts
   every GPUI Action and `bind_keys` there. Recommendation: `src/action_tracker.rs` for the
   entity (E16), `src/actions.rs` for the window-level Actions and `bind_keys_global` (E20).
   Section 2 is written that way. One line in ADR-009 to update.

2. **Settled, R-130. `write_atomic` lives in `tl-app`, but the engine's Startup worker writes autostart
   overrides.** ADR-011 names `tl-app/src/atomic.rs` for both; ADR-001 does not let `tl-engine`
   depend on `tl-app`. Recommendation: the routine lives in `tl-ops/src/fs.rs` (E07-A3);
   `tl-app/src/atomic.rs` re-exports it, which needs `tl-ops` added to `tl-app`'s allowed row in
   ADR-001 (one table cell). Alternative: duplicate the 20 lines. The first is cleaner.

3. **Settled, R-131. `ureq` is not in any allowed-dependency row.** PERF-060 fixes `ureq` 3 with rustls for the
   public IP fetch. Recommendation: add `ureq = { version = "3", default-features = false,
   features = ["rustls"] }` to `[workspace.dependencies]` (E02 writes the pin) and to
   `tl-engine`'s row; the E12 bead that adds it runs `check-deps` to settle U-30. Pin the exact
   version in that bead after `cargo add` resolves it.

4. **Settled, R-132. Fluent is not in `tl-ui`'s allowed row and the catalogue layout is undefined.** APP-043
   fixes `fluent-bundle` and `unic-langid` with embedded `.ftl` files. Recommendation: add both
   to `tl-ui`'s row and the workspace pins; `tl-ui/src/i18n.rs` (E16-A1) exposes `tr(key)` and
   `tr_args(key, args)` returning `SharedString`, with a `Strings` Global and `strings_version`;
   `assets/locales/en-US.ftl` is one file that every epic appends to by chain edit, keys named
   `<area>-<entry>-<slug>` (for example `processes-menu-end-task`). The locale list comes from
   canonical table (a). Other locales are P4 and not beaded in this round.

5. **Settled, R-133. `actions!(tl_pwn, ...)` in process-windows.md.** R-128 fixes one namespace, `tasklens`.
   Recommendation: `windows/actions.rs` declares its Actions in `tasklens` with the `Props`,
   `Affinity`, `Priority`, `Signal`, `Run` and `HandleSearch` prefixes; the one sentence in
   process-windows.md is out of date and writers follow R-128.

6. **Settled, R-134. Opening URLs, file locations and launching programs has no `Command` variant.** PROC-073,
   PROC-074, PROC-095, SYS-023, UI-SHL-093 ("Open log folder") and PERF-061 need session-bus
   calls (`FileManager1`, portal `OpenURI`, `StartTransientUnit`) with an `xdg-open` fallback and
   a result toast. `tl-ui` cannot use zbus. Recommendation: add to `tl-model::actions`
   `ActionKind::OpenUri(Arc<str>)`, `ActionKind::ShowInFileManager { path: Arc<str>, properties:
   bool }` and `ActionKind::Launch(LaunchSpec)` (command text or desktop entry id, `as_admin`,
   `cwd`), allow an empty `targets` vector for these three kinds, run them in `tl-engine/src/opener.rs`
   and `launcher.rs` (E14-A3), and report through the normal `ActionEvent` path so the existing
   toasts work. Alternative: a `Box<dyn Fn>` callback from `tl-app` using its blocking zbus
   connection. The first keeps one action path and one place for session-bus code.

7. **Settled, R-135. `tl-collect` module layout is not in ADR-016.** Section 2 proposes it (`parse/`, `sample/`,
   `proc/`, `apps/`, `net/`, `startup/`, `inventory/`, `gpu_drm/`). Recommendation: accept it;
   the E04-A1 bead adds the module list to ADR-016 as a doc edit in the same commit.

8. **Settled, R-138. The phase-0 slice is wide.** E01 needs 21 anchors from 13 epics (section 4). This follows
   from ADR-014 measuring the real chart, table, pages and bridge with synthetic data.
   Recommendation: accept, label those anchors `phase-p0`, and have the leader start them first
   across batches B1, B2, B3, B5, B6, B7 and B8. The alternative, a throwaway spike UI, would
   measure code that is then rewritten and would not satisfy the decision rule.

9. **Settled, R-138. Where the reducer's `sys_table.rs` body is written.** The builder is reducer code (E09) but
   only the P2 D-Bus epic (E14) knows the rows. Recommendation (used in section 2): E09-A1
   writes the signatures and an empty body; E14-A1 fills it. The same pattern is not used
   anywhere else.

10. **Settled, R-138. `tl-engine/src/workers/processes.rs` is shared with the low-memory mode.** ADR-004 makes
    `LowMemory` a stripped sweep inside the `Processes` worker. Recommendation (used in section
    2): the stripped sweep is its own `Source` in `workers/lowmem.rs` (E32); E32 chain-edits one
    `match` arm in `processes.rs` that picks it by `EngineMode`.

11. **Settled, R-138. The `tasklens-setup` script source location and language.** Table (c) gives the install
    path only. Recommendation: `packaging/tasklens-setup`, POSIX `sh`, installed by the PKGBUILD
    with mode 0755, owned by E29.

12. **Settled, R-138. The KWin bridge scripts.** The docs give the run-time path but not where the JavaScript
    source lives. Recommendation: `assets/kwin/{kwin-bridge.js,pick.js,cmd-template.js}`
    embedded with `include_str!`, owned by E14.

13. **Settled, R-136. The `AppRequest` channel type between `tl-app` and `tl-ui`.** UI-SHL-112 names
    `async_channel::bounded(4)` and `AppRequest` but not the enum. Recommendation:
    `tl_model::command::AppRequest { Activate { token: Option<Arc<str>> }, SelectTab(Tab),
    SelectPid(u32), Quit }` in `tl-model` (E03-A2), so neither crate depends on the other for it.

14. **Settled, R-138. Epic count.** 33 epics against the asked-for 20 to 30. Recommendation: keep 33; the
    section 1 sizes show that merging any two UI epics passes 40 beads.

15. **Settled, R-138. The Run dialog's `--bench` neighbour, `--gallery`, is not in APP-052.** UI-FND-030 adds it.
    Recommendation: `args.rs` (E33-A1) declares `--gallery` beside `--bench`, both behind the
    `bench` feature, and table (c)'s argument list gains it in a doc edit by E01.

16. **Settled, R-137. Unit rows' and app rows' "Memory" from cgroup `memory.stat` `anon`** needs `memory.stat`
    reads in the process sweep that ADR-016's sources table does not list. Recommendation: E05-A2
    reads `memory.stat` and `cpu.stat` for app scopes and the demanded unit cgroups
    (`service_cgroup_stats`) and records this in the ADR-016 table in the same commit.


