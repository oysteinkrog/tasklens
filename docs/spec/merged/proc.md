# tasklens merged spec: processes, apps and process actions

This is the merged PROC spec (round 3). It replaces `features-proc.md` (v1), `features-proc-r2.md` (r2) and `features-proc-addendum.md` (add) for implementation. It covers the Processes tab, apps and units, process actions, the Process and App Properties windows, History mode, the process parts of minigraphs and tray graphs, the Run dialog, Search File Handles and the low-memory rescue window.

Precedence used: `docs/DECISIONS.md` (D-001 to D-010), then `docs/RECONCILIATION.md` (R-001 to R-103 and canonical tables a to d), then `docs/ARCHITECTURE.md` on engineering structure, then the source specs (r2 over v1 on a shared topic, the addendum where it is later and more specific). "Was:" lines name every old ID an entry absorbs. Old IDs are prefixed `v1`, `r2` or `add`; "add amend PROC-0xx" means the addendum's amendment to that v1 entry.

## Conventions used in every entry

- **Identity.** A process is a `ProcessKey { start_ticks, boot_id, pid }` (ADR-003). `start_ticks` is field 22 of `/proc/<pid>/stat`; `boot_id` is folded from `/proc/sys/kernel/random/boot_id`. Rows, selections, menu targets, Properties windows, graphs and history all key on it, never on a row index or a bare PID. App and unit rows key on `CgroupRef { rel_path, cgroup_id }` or `UnitRef { manager, name }` (ADR-005, ADR-006).
- **Validity.** Missing data is never shown as zero. Table cells, header totals and sidebar readouts use the ADR-003 glyphs: `…` warming up, lock icon restricted, `n/a` unsupported, `-` missing, `!` failed (theme warning colour), dimmed value stale. Properties key/value fields and stats grids use the PERF-079 words: "Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown" (R-050). The restricted tooltip is "Needs administrator access. Click the shield to grant it." from P3, and "Needs administrator access." before P3. Stale means older than `max(3 × period, 1.5 s)` (R-051, ADR-008).
- **Time.** Every timestamp is CLOCK_BOOTTIME; CLOCK_MONOTONIC is read only to detect suspend (ADR-003, R-064).
- **Helper.** `tl-helper` is started as `pkexec /usr/lib/tasklens/tl-helper --session` under polkit action `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`), talks over its stdin and stdout with `tl-model::wire` framing (64 KiB frames, 4 MiB per request), accepts only the ADR-006 allowlist, and exits after 300 s idle or on stdin EOF (R-001 to R-004). Nothing retries through it on its own.
- **Phases** (R-072). P0 spike; P1 Performance core, process table and P1 actions; P2 Services, Startup Apps, Connections, Properties windows, KWin bridge, 5 and 10 min ranges, remaining themes; P3 helper, history, tray, detached graphs, mini window, strings, dumps, low-memory; P4 everything marked P4.
- **Machine facts** used below (verified on the owner's machine by the source specs): kernel 7.2, cgroup v2 with `cgroup.freeze` and `cgroup.kill` in user units; KDE puts apps in `app-<id>@<rand>.service` under `app.slice`; `kernel.yama.ptrace_scope = 1`; `sched_autogroup_enabled = 1`; `perf_event_paranoid = 2`; `/proc/1/io`, `/proc/1/fd` and `/proc/1/environ` give EACCES to uid 1000; RAPL `energy_uj` is readable because Mission Center's `99-powercap.rules` exists; no `/sys/devices/cpu_core` (AMD); KWin 6.7.5 exposes `queryWindowInfo`, `getWindowInfo` and `/Scripting`; `gdb`, `gcore`, `coredumpctl` and `qdbus6` are installed; `CONFIG_IRQ_TIME_ACCOUNTING=y`; zram0 is active; the only GPU is the NVIDIA card `card1`.

---

## A. Foundations

### PROC-001 Process identity and action targeting
- Was: v1 PROC-001; r2 conventions (Identity)
- Source: TS L28922-28926 | MC `apps_page/actions.rs:56` | ADR-003, ADR-005, ADR-006
- Behaviour:
  - Every row, selection, open menu, Properties window, graph and history entry holds a `ProcessKey`.
  - When a menu opens, it captures the keys of the selection at that moment (max 256 targets). The menu stays bound to those keys even if rows move.
  - If a target exited or its PID was reused, the action does nothing for that target and shows the toast "The selected process changed; wait for the list to update and try again." (R-035).
- Linux data/mechanism:
  - Before every process operation, `tl-ops::process::open_verified` opens a pidfd, then re-reads field 22 of `/proc/<pid>/stat` and compares it with `start_ticks` (ADR-006).
  - Signals go through `pidfd_send_signal` on that pidfd and cannot hit a reused PID.
  - Nice, scheduling, affinity and I/O priority take a PID. They follow the ADR-006 rule: verify the pidfd, call by PID, then poll the pidfd with timeout 0; a readable pidfd gives `OutcomeUnknown("process exited during the change")`. Specs and UI text never claim the pidfd guarantee for these.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-002 Process sweep and per-process data pipeline
- Was: v1 PROC-002; r2 PROC-001
- Source: TS 2.1, INI `UI.RefreshIntervalMs` | MC magpie `processes/` | ADR-008, ADR-016
- Behaviour:
  - One process table model. It refreshes at Settings > Refresh: 0.25, 0.5, 1 (default), 2, 5 or 10 s (`processes.interval_ms`, 250..=10000, menu 250 500 1000 2000 5000 10000).
  - Rates divide by the real elapsed time between samples. A rate cell shows `…` until it has two samples.
  - Every metric cell carries a validity state.
- Linux data/mechanism:
  - Every sweep, per PID, read with `openat` relative to an `O_DIRECTORY` fd of `/proc/<pid>` that is closed in the same sweep (ADR-016):
    - `stat`: comm, state, ppid, pgrp, session, tty_nr, flags (`PF_KTHREAD` 0x00200000), minflt, majflt, utime, stime, priority, nice, num_threads, starttime, processor, policy. Parsed from the last `)`.
    - `statm`.
    - `status`: Uid and Gid lines, RssAnon, RssFile, RssShmem, VmRSS, VmSwap, VmHWM, VmPeak, VmSize, VmData, VmStk, VmPTE, `Cpus_allowed_list`.
    - `io` (own UID only; EACCES gives Restricted).
  - Metadata (`cgroup`, `exe` readlink, `cmdline`) is cached per key and refreshed every 10 sweeps, and at once when `comm` changes (R-064).
  - `cmdline` is read up to 32 KiB with `…` appended past that (R-008, ADR-015). Text is sanitized before it enters `ProcessMeta` (ADR-015).
  - Demand-driven reads, only while a consumer exists (a visible column, a Properties window, a graph or recording):
    - fd count, from `fstatat` `st_size` of `/proc/<pid>/fd` with a readdir count as fallback (R-064, U-34);
    - `smaps_rollup` for PSS and USS, for `detail_keys` only, through the FD cache of at most 128 entries (ADR-016);
    - `task/*/status` for context switches;
    - GPU per-process data through `tl-gpu-probe` at 1 Hz (ADR-007, R-026);
    - sock_diag for the Network column (PROC-032).
  - The sweep is the `Processes` worker in `tl-engine`, one whole sweep per `spawn_blocking` job. It publishes `ProcessSnapshot` behind an `Arc` with a component version and an observation interval. `ProcessRecord` carries `rss_anon_bytes` (R-005).
  - Counter baselines reset on identity change, epoch change or suspend (ADR-008).
  - Schedule per ADR-008: the sweep runs at `interval_ms` while the table or any process consumer is visible, and stops when nothing visible needs it and recording is off. Process minigraph cards (PROC-093) and the History rolling window (PROC-086) count as consumers.
- Verdict: BUILD
- Phase: P1
- Privilege: none. Other users' `io`, `fd`, `exe`, `environ`, `maps` and `smaps_rollup` are Restricted until the helper reads them (PROC-076).

### PROC-003 Value formats, zero values and units
- Was: r2 PROC-027; add PROC-104; v1 conventions (validity glyphs)
- Source: TS formats "0 %ls", "0 Mbps", "0 MB/s" L35601-35609, [IMG processes.png] | RECONCILIATION table (d), R-013, R-014, R-015
- Behaviour, for every process-table cell (canonical table d):

  | Kind | Non-zero format | Measured zero |
  |---|---|---|
  | CPU, GPU and other percent | `%.1f%%`; a value that rounds to 0.0 counts as zero | `0%` |
  | Memory | IEC, base 1024 (B, KiB, MiB, GiB, TiB), 2 decimals from 1 KiB: `16.00 KiB`, `1.22 GiB`; below 1 KiB `512 B` | `0 B` |
  | Disk and I/O rates | fixed MiB/s, 1 decimal: `0.4 MiB/s` | `0 MiB/s` |
  | Network, bits (default) | SI base 1000; fixed Mbps with 1 decimal when auto-scale is off (default): `2.3 Mbps`; Kbps, Mbps, Gbps when `units.network_auto_scale` is on | `0 Mbps` |
  | Network, bytes | IEC, fixed MiB/s like disk; KiB/s, MiB/s, GiB/s with auto-scale on | `0 MiB/s` |
  | Counts | digits grouped with a thin space (U+2009): `12 345` | `0` |
  | CPU time | `h:mm:ss`; past 999:59:59 `Nd hh:mm` | `0:00:00` |
  | Start time | today `HH:MM:SS`, else `YYYY-MM-DD HH:MM` | |
  | Short ages (tooltips) | `1d 4h`, `4h 12m`, `12m 5s`, `5s` | |

  - "MiB/s" and "KiB/s" are used everywhere, never TaskSlinger's "MB/s".
  - Zero formats apply only to a measured zero. Missing values use the validity glyphs.
  - Numerals are tabular; numeric columns are right-aligned.
- Linux data/mechanism: all formatting lives in `tl-model::units`, writing into a caller buffer. The UI caches formatted strings per (node, column) and re-formats only when the value bits or validity change (ADR-009 text cache).
- Verdict: ADAPT (TaskSlinger density with IEC labels)
- Phase: P1
- Privilege: none

---

## B. Classification, apps and special rows

### PROC-004 Process classification and Task Manager groups
- Was: v1 PROC-011; v1 PROC-005 (group structure); r2 PROC-004
- Source: TS 2.1 "Groups", L28448-28454, L35693-35711, cross-check A.6/A.7 | MC `apps_page/mod.rs:85-94`
- Behaviour:
  - In Task Manager Grouping mode (PROC-014), three bold group header rows read "Apps (N)", "Background processes (N)" and "System processes (N)". TaskSlinger's "Windows processes" becomes "System processes".
  - Group order is fixed whatever the sort: Apps, Background processes, System processes.
  - Each group's expand state persists in `state.toml` `[processes] group_expanded = { apps, background, system }`, all default true.
  - Group header rows cannot be selected for actions; their only menu item is Expand/Collapse.
  - Classification, first match wins:
    1. **System:** kernel threads (`PF_KTHREAD` in stat field 9, or ppid 2, or pid 2), pseudo rows (PROC-012), processes with a real UID other than the current user's, and processes whose cgroup is outside `/user.slice/user-<uid>.slice/` (`system.slice`, `init.scope`, `machine.slice`, other users' slices).
    2. **Apps:** processes inside `user@<uid>.service/app.slice/` units that resolve to a desktop app (PROC-005, PROC-006), or that own a KWin window (PROC-052).
    3. **Background:** everything else owned by the current UID: `session.slice`, `background.slice`, `init.scope` of the user manager, unmatched `app.slice` units, `session-N.scope` processes such as login shells, and `Xwayland`.
  - The Type column (PROC-027) shows the class: "App", "Background process", "System process", "Kernel thread", "Unit", "Pseudo".
- Linux data/mechanism: the cgroup v2 `0::` line of `/proc/<pid>/cgroup`, parsed into slice, unit and scope; stat flags; status `Uid:` real UID. The current UID comes from `getuid()` and is never hard-coded.
- Verdict: ADAPT (the Windows signer and path rule becomes cgroup and UID rules)
- Phase: P1
- Privilege: none

### PROC-005 App detection from cgroups
- Was: v1 PROC-008; r2 PROC-005
- Source: MC app-rummage `cgroup.rs:13-190` | ADR-016 "App grouping"
- Behaviour: every running app instance becomes one App row.
- Linux data/mechanism:
  - Walk `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/app.slice/*.{scope,service,slice}`.
  - Parse these unit names with `\x2d` unescaping:
    - `app-[<launcher>-]<id>[@<instance>]-<rand>.scope`
    - `app-<id>@<rand>.service` (KDE; example on this machine: `app-chrome\x2dwork@00bca6....service` gives app ID `chrome-work`)
    - `flatpak-<id>-<n>.scope`
    - `dbus-:<n>-<id>@<n>.service`
    - `snap.<a>.<b>-<uuid>.scope`, which gives app ID `a_b`.
  - PIDs come from `cgroup.procs`, recursively.
  - Rescan when inotify on `app.slice` sees `IN_CREATE` or `IN_DELETE`, and on every sweep as a fallback.
  - `app-*@autostart.service` units go to Background unless PROC-052 reports a mapped window for one of their PIDs.
  - Units that are clearly not apps (`dconf.service`, `obex.service`, `dbus.socket`, `run-*.scope` without a desktop match, and other non-`app-` units such as `frankenterm-mux`) stay in Background as unit rows (PROC-009).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-006 Desktop-file index, exe fallback, disambiguation and icons
- Was: v1 PROC-009; v1 PROC-010 (icons); r2 PROC-006
- Source: MC `desktop.rs:78-120`, `exec.rs`, `matching.rs:10-24, 109-175`, `PL:apps.rs` | TS cross-check A.7 | ADR-016 "Icons"
- Behaviour: app rows get a name, an icon and a description. A process outside any app scope (started from a terminal or by another app) still joins an app when its executable matches.
- Linux data/mechanism:
  - Index `.desktop` files in `$XDG_DATA_DIRS/applications` and `~/.local/share/applications`, built off the UI thread at startup. Skip `NoDisplay=true`, `Hidden=true` and any Type other than Application. Keep Name, GenericName, Comment, Exec, Icon, StartupWMClass and X-Flatpak. inotify on those directories rescans with a 500 ms debounce.
  - Match order: cgroup app ID to `.desktop` ID; then `StartupWMClass` against the KWin window `resourceClass` (PROC-052); then the canonical `/proc/<pid>/exe` against the resolved Exec binary, with `flatpak run`, `snap run` and `env VAR=...` stripped.
  - Exceptions table: `firefox-bin`, `soffice.bin`, `gnome-terminal-server`, `chrome`, `chromium`, `electron` map to their app; `xdg-desktop-portal-kde` stays Background; `Xwayland` stays Background.
  - When several apps share a binary (Chromium PWAs, launchers such as `chrome-work`), argv tokens decide: `--app-id=`, `--profile-directory=`, `--class=`. True ties stay unmatched.
  - Ignore list: `guake-prefs`, the foot server.
  - Wine: a process whose cmdline[0] ends in `.exe` matches desktop files whose Exec contains `wine` and that `.exe` name, or whose StartupWMClass matches it.
  - Icons: theme from `~/.config/kdeglobals` `[Icons] Theme=`, then `breeze`, then `hicolor`; sizes 16, 22, 24, 32, then `scalable`, preferring SVG; absolute `Icon=` paths are loaded directly. App rows draw at 24 px and process rows at 16 px, times the UI scale. Resolved paths are cached in `$XDG_CACHE_HOME/tasklens/icons.toml` (ADR-016).
  - Processes inside an app inherit the app icon. Other processes get a generic executable icon. Kernel threads get a chip icon.
- Verdict: BUILD (MC algorithm with Wine and KDE additions)
- Phase: P1
- Privilege: none

### PROC-007 Process display naming
- Was: v1 PROC-010 (names); r2 PROC-011
- Source: MC `table_view/models.rs:134-168`, magpie `apps.rs:134-184` | TS "Name display" L28358
- Behaviour:
  - App-matched primary processes show the app Name.
  - Otherwise the name is the exe basename. If exe is unreadable, it is the first cmdline token with a trailing ":" removed. If that is empty, it is `comm`.
  - Wine processes (exe basename `wine64-preloader`, `wine-preloader`, `wine64` or `wine`, or `comm` ending `.exe`) use the last segment of the `.exe` path in argv, splitting on both `/` and `\` (`C:\...\game.exe` gives `game.exe`).
  - Kernel threads show `[comm]`.
  - When `comm` differs from the exe basename (renamed threads such as `Web Content`), it is a qualifier: "firefox (Web Content)".
- Linux data/mechanism: `/proc/<pid>/exe`, `cmdline` and `comm`, sanitized per ADR-015 (control characters become U+FFFD, bidi controls removed). `ProcessMeta.name` holds the result.
- Verdict: ADAPT (MC rules plus the comm qualifier)
- Phase: P1
- Privilege: none

### PROC-008 App row contents
- Was: r2 PROC-007; v1 PROC-005 (Apps group contents)
- Source: TS 2.1 "Row details", cross-check A.12 | MC 3 "Tree"
- Behaviour:
  - Name cell: 24 px icon, the desktop Name and " (N)" with the process count, format `%s (%s)`.
  - Qualifiers follow in the `%s (%s) (%s)` form: "(Flatpak)", "(Snap)", "(Wine)".
  - From P2, a dimmed secondary text shows the title of the app's most recently active window when the KWin bridge is up (PROC-053). Example: "Firefox (14)  Mozilla Firefox - Inbox".
  - Children: the app's primary processes (app PIDs whose parent is not an app PID), each with its full ppid subtree in Tree mode, or flat in List mode. Child rows inherit the app icon at 16 px.
  - PID is blank. Type is "App".
  - Metrics: CPU, Disk, Network and GPU are sums over the app's processes. Memory is cgroup `memory.stat` `anon`, with the tooltip "Anonymous memory charged to this app's cgroup" (R-005, Q-5). When a member's per-process Disk value is Restricted, the row uses the cgroup `io.stat` rate (Δ(rbytes + wbytes)/Δt over all devices) instead, which every user can read, so system units still show a value (R-103).
- Linux data/mechanism: cgroup `memory.stat`, `io.stat` and `cgroup.procs`; the KWin bridge for titles; the `.desktop` index.
- Verdict: ADAPT (memory from the cgroup so nothing is counted twice)
- Phase: P1 (window titles P2)
- Privilege: none

### PROC-009 Unit rows and the Kernel threads row
- Was: r2 PROC-008; v1 PROC-005 (Background and System contents)
- Source: TS 2.1 kinds "Service Host", tooltip "Services:" | MC 4
- Behaviour:
  - In Grouping mode, a systemd unit with more than one process becomes one expandable row in Background or System. The name is the unit's Description (for example "PipeWire Multimedia Service") with a 16 px service icon and " (N)". Until the Services collector lands (P2) the unit name is shown instead.
  - A unit with a single process shows that process row directly, with the unit in the tooltip.
  - Type is "Unit". Metrics follow the PROC-008 rules.
  - Kernel threads fold under one System row "Kernel threads (N)", collapsed by default. It is the analogue of TaskSlinger's "System" row and holds kthreadd and its children.
- Linux data/mechanism: the unit name from the cgroup path. The Description comes from `org.freedesktop.systemd1.Unit.Description`, cached by the Services collector and refreshed on `UnitNew` and `UnitRemoved`.
- Verdict: ADAPT (svchost grouping becomes systemd unit grouping)
- Phase: P1 (Description P2)
- Privilege: none. System cgroup files are world-readable.

### PROC-010 Flatpak bwrap unwrap
- Was: v1 PROC-013
- Source: MC `apps_page/actions.rs:87` | MC inventory section 8
- Behaviour: app-level signals (End App, PROC-060) skip `bwrap` wrapper processes and target the first descendant that is not bwrap. bwrap rows still show in the tree.
- Linux data/mechanism: exe basename `bwrap`, then walk children from `/proc/<pid>/task/<pid>/children`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-011 Launcher split (Steam, Lutris, Heroic)
- Was: v1 PROC-014; r2 PROC-009
- Source: TS 2.1 (each app is its own row) | MC 3 (scope grouping puts every game under Steam)
- Behaviour: under the Steam app row, each game is its own child app row named after the game, holding the processes that `reaper SteamLaunch AppId=<n>` started.
- Linux data/mechanism:
  - The `reaper` argv contains `AppId=<n>`. The name comes from `~/.local/share/Steam/steamapps/appmanifest_<n>.acf` key `"name"`, the icon from `steam_icon_<n>`.
  - Lutris and Heroic use `LUTRIS_GAME_UUID` or `HEROIC_APP_NAME` from `/proc/<pid>/environ` when it is readable (same UID).
  - Unverified: U-19.
- Verdict: ADAPT (Linux-only extension)
- Phase: P4
- Privilege: none

### PROC-012 Pseudo-process rows
- Was: r2 PROC-010; add PROC-100
- Source: TS pseudo list L35439-35445, display names L35392, L35420, customize-27 | cross-check A.6 | R-032
- Behaviour: two rows stand for kernel work that no process owns.
  - **"Interrupts"**: CPU is the irq + softirq share of total CPU time. Every other column shows `n/a`. Always present.
  - **"Memory compression (zram)"**: Memory is Σ `mem_used_total` over all zram devices; the tooltip shows original size, compressed size and ratio. Disk is Σ zram read and write rates. CPU shows `n/a`, because compression time is already counted in the faulting task or kworkers. Present only when a zram device has `disksize` > 0.
  - Both rows: PID blank, Type "Pseudo", User "root", no expander. They sit in System processes after "Kernel threads (N)". In Tree and List views without grouping they are top-level rows sorted like any other.
  - They are excluded from the "N Processes" count. Hide system processes hides them (PROC-046). There is no separate toggle.
  - Row menu: Copy, Copy details, Copy all metrics, Add Minigraph only.
  - Row tooltip: one line describing what the row counts.
  - Dropped: "System Idle Process" and "Idle" (not shown), "Secure System" (no VBS on Linux), "Registry" (no registry hive), "vmmemwsl" (no WSL VM). r2's "System interrupts" and "Memory compression" rows and the "Show pseudo processes" toggle: Dropped: R-032.
- Linux data/mechanism:
  - Interrupts: Δ(irq + softirq)/Δtotal from the `cpu` line of `/proc/stat`. With `CONFIG_IRQ_TIME_ACCOUNTING=y` this time is not also in any task's `stime` (U-21). Without that option the row shows `n/a` with tooltip "Interrupt time is included in task time on this kernel" (check `/proc/config.gz`, or whether irq stays 0).
  - zram: `/sys/block/zram*/mm_stat` field 3 for memory; `/sys/block/zram*/stat` sectors for the rate.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

---

## C. Table structure and view modes

### PROC-013 Table widget basis
- Was: v1 PROC-007
- Source: claude-gpui-apis.md section 4 DataTable gaps 1-6 | ADR-009 "Process tree-table"
- Behaviour: a virtualized tree table with tree rows, multi-select, identity-based selection and column show, hide, reorder and resize. Row height is `28 × row_height_pct/100 × ui_scale_pct/100` logical px (R-020).
- Linux data/mechanism:
  - Custom table on GPUI `uniform_list` (ADR-009). The kit `DataTable` is not used (index selection, no tree rows, column mode on arrows, fixed row heights, per-frame cell rebuild).
  - The engine builds `ProcessTableView` (sort, filter, group, tree flatten); the UI never sorts or filters.
  - Header, resize and drag-reorder are app-owned. Column x offsets are a precomputed `Arc<[Pixels]>`.
  - After each render the table reports the keys of the visible range plus one viewport above and below (max 512) to `DemandAggregator`, also after a re-sort with an unchanged range.
  - Right-click opens a gpui-kit `PopupMenu` whose actions carry the captured keys. Every menu item dispatches the same GPUI Action as its key binding.
- Verdict: BUILD (custom tree table)
- Phase: P1
- Privilege: none

### PROC-014 View modes and View > Process View
- Was: v1 PROC-004; v1 PROC-005 (grouping toggle); r2 PROC-002; add amend PROC-004 (Dropped: R-010)
- Source: TS 1.3 "Process View" L28488-28490, INI `UI.TreeView`, `UI.TaskManagerGrouping` | MC `table_view/models.rs:313` | R-010
- Behaviour:
  - One setting `processes.mode` with values `grouped | tree | flat`, default `grouped`.
  - View > Process View shows three radio items: "Tree View", "Task Manager Grouping", "List View". The submenu also holds "Tree Connector Lines" (PROC-017), "Highlight New and Exited Processes" (PROC-043) and "Collapse all" (PROC-020).
  - **Tree View** (`tree`): the ppid forest. A row is a root when its parent is not shown (filtered, hidden by Hide system processes, or exited).
  - **Task Manager Grouping** (`grouped`): the three groups of PROC-004. Each app or unit row expands into its own ppid subtree.
  - **List View** (`flat`): one flat list, no groups.
  - Expand state is kept per identity for the session; group expand state persists (PROC-004).
  - The addendum's "radio pair plus check item" layout and its two-boolean mapping are Dropped: R-010.
- Linux data/mechanism: ppid is stat field 4. Orphans are already reparented by the kernel to `systemd --user` or PID 1, so the tree stays connected. A missing parent makes the process a root; reparenting moves it at the next sweep. The engine projection builds the flattened rows with stable identity tie-breaks (ADR-009 `TableQuery.mode`).
- Verdict: ADAPT (TaskSlinger's two booleans become one enum)
- Phase: P1
- Privilege: none

### PROC-015 Toolbar view-mode cycler
- Was: add PROC-105; r2 PROC-002 (cycle button)
- Source: TS `chrome.view_mode`, "Toggle view mode" L28332, toast "View mode: %ls" L28638 | APP-005 | R-010
- Behaviour:
  - The toolbar "stacked bars" button cycles Tree View, then Task Manager Grouping, then List View, then back to Tree View (TaskSlinger string order).
  - Toast: "View mode: Tree View", "View mode: Task Manager Grouping" or "View mode: List View".
  - Tooltip: "Toggle view mode (current: <name>)".
  - The selection is kept by identity across the change.
- Linux data/mechanism: sets `processes.mode`; the engine rebuilds the projection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-016 Processes-tab toolbar buttons
- Was: add PROC-107
- Source: TS [IMG processes.png vs performance.png] | cross-check A.8 | APP-002, APP-004, APP-005 | R-031
- Behaviour:
  - The Hide system processes toggle (PROC-046) and the view-mode cycler (PROC-015) appear only while the Processes tab is active. When the tab changes they are removed, not greyed out, and the toolbar does not reserve their space.
  - The shield (APP-003) and the "+" window picker (PROC-055) appear on every tab (APP-002).
  - In History mode on the Processes tab: Hide system processes stays active and also filters the History table. The view-mode button stays visible but disabled, with tooltip "History shows a flat list".
  - Both buttons dispatch the same Actions as View > Process View and the settings.
- Linux data/mechanism: the title-bar toolbar content depends on the active tab.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-017 Tree Connector Lines
- Was: v1 PROC-006; r2 PROC-003
- Source: TS 1.3, INI `UI.TreeGuideLines=0`
- Behaviour: a check item under View > Process View, default off (`processes.tree_guide_lines = false`). It draws 1 px guide lines in the DIVIDER colour from each parent's chevron down to its children, in Tree View and inside groups.
- Linux data/mechanism: drawn in the Name cell from the depth and "last sibling" bits stored in each flattened row.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-018 PID cell "PID +N children"
- Was: r2 PROC-012; v1 PROC-015 (PID column format)
- Source: TS 2.1 `%u +%llu` L35614
- Behaviour: in Tree View and grouped subtrees, a collapsed parent's PID cell shows "11528 +18", where N is the total number of descendants. Expanded parents show the PID only. App, unit, group and pseudo rows show a blank PID.
- Linux data/mechanism: computed from the forest in the engine projection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-019 Subtree totals on parent processes
- Was: r2 PROC-015; v1 PROC-095 (`subtree_totals` row)
- Source: MC `apps-page-merged-process-stats`
- Behaviour:
  - Settings > Processes > "Show subtree totals on parent processes", default off (`processes.subtree_totals = false`).
  - When on, process rows that have children show the sum over their subtree for CPU, Disk, Network, GPU, Power usage, Threads and Handles, with a "Σ" prefix.
  - Memory shows Σ RssAnon over the subtree, also with the "Σ" prefix (R-005).
  - App and unit rows always aggregate (PROC-008).
- Linux data/mechanism: tree sums in the engine projection; each process counts once. `subtree_totals` is part of `EngineSettings`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-020 Collapse All
- Was: v1 PROC-045
- Source: MC Collapse All button, Ctrl+L
- Behaviour: View > Process View > "Collapse all" (Ctrl+L) collapses every expanded row and group except the three group headers.
- Linux data/mechanism: UI expansion state (`TableQuery.expanded`).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-021 Startup placeholder
- Was: v1 PROC-046
- Source: MC `window.rs:1150-1239` (shimmer) | R-060
- Behaviour: the Processes tab shows rows from the first sweep straight away, with `…` in rate cells until the second sweep. There is no skeleton or shimmer on this tab.
- Linux data/mechanism: Validity `WarmingUp`.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

---

## D. Columns

Column IDs are the stable strings below (v1 IDs are canonical per R-007; IDs marked "new" are minted here for columns only r2 had). Labels follow r2 where it differs from v1, except where TaskSlinger has the label. App, unit and group rows use the aggregation in each table; process rows show their own values. "Σ" means a sum over member processes with each process counted once. "Restricted" in a privilege line means the lock glyph until the helper reads it (PROC-076).

### PROC-022 Column model, default set and visibility
- Was: v1 section D intro; r2 PROC-016 (model, default set, GPU slots); add PROC-106
- Source: TS 2.1 columns, INI `[Columns]`, `[ColumnWidths]`, `ProcessColumnVisibilityMask` | cross-check A.9, B.2 | R-006, R-007
- Behaviour:
  - Every column has a stable string ID. Visibility, order and width persist by ID.
  - Default visible columns, in order: `name, user, pid, cpu, memory, disk, network, gpu, description` (R-006). `power` and `power_trend` exist and are off by default on every machine, whether or not RAPL is readable.
  - Visibility and width are stored separately (`columns.processes.visible` and `.widths`). Hiding a column and showing it again restores its old width. TaskSlinger's "width > 0 means visible" encoding is not copied; tasklens never imports its INI.
  - At least one column stays visible; Name cannot be hidden.
  - GPU 0..GPU 15 columns are offered only for adapters that exist (PROC-033).
  - The column registry is the union of PROC-027 to PROC-036.
- Linux data/mechanism: `tl-model::ids::ColumnId` with `as_str()`/`from_str()`; an unknown ID in a file is dropped with a log line (ADR-011).
- Verdict: ADAPT
- Phase: P1 (Linux extras P2 and later, per column)
- Privilege: none

### PROC-023 Column minimum and default widths
- Was: add PROC-101; r2 PROC-016 (widths, templates); add amend PROC-015
- Source: TS template strings L35615-35626 | INI `[ColumnWidths] Processes` | cross-check A.9, C.1 | R-006
- Behaviour:
  - Minimum width is the measured width of a template string, plus 2 × 8 logical px cell padding, plus 12 px for the sort arrow on the active sort column:

    | Template | Columns |
    |---|---|
    | "999999 +99" | PID, Parent PID, SID |
    | "100.0%" | CPU (all percent kinds), GPU, GPU 0..15, Memory % |
    | "xxx Mbps" | Network, Network send, Network receive in bits mode; "xxx MiB/s" in bytes mode |
    | "xxx GiB/s" | Disk and every other rate column |
    | "999 MiB" | memory columns |
    | "999999" | counts: Threads, Handles, Page faults, Context switches, Read ops, Write ops |
    | "999:59:59" | CPU time, CPU time kernel, CPU time user |

  - Text columns (Name, User, Image path, Description, Command line and the rest) have a 60 px minimum.
  - A drag below the minimum stops at the minimum. "Resize columns to fit content" never goes below it.
  - Default widths at 100% (logical px): Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342, Power usage 101. Every other column defaults to max(minimum, header text width).
- Linux data/mechanism: GPUI `text_system().layout_line` of each template with the table font and tabular numerals, cached per (font, size, scale) and recomputed when the font, UI scale or row height changes.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-024 Column picker
- Was: v1 PROC-048; r2 PROC-016 (picker); add PROC-102; add amend PROC-048
- Source: TS View > Columns, `header_column.filter` L29888, headings L28429-28433, aliases L28427-28428 | cross-check A.10 | R-065 | APP-018
- Behaviour:
  - Opened from View > Columns > "Select Columns…" or the header menu. Title "Columns".
  - A list of all columns with checkboxes and a filter field at the top. The filter matches both the full name and the alias.
  - Group headings, in order: Image, Identity, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux. "Objects" holds Handles, Threads, X11 resources and Windows.
  - Two entries carry TaskSlinger-style alias text (full metric name, then the short header label in quotes): `Working set private - "Memory"` (the default Memory column, RssAnon) and `GPU memory dedicated - "VRAM"`.
  - Restricted or unsupported columns are still listed, with a note such as "n/a on this system".
- Linux data/mechanism: UI only.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-025 Column reorder, resize and persistence
- Was: v1 PROC-038; r2 PROC-016 (drag, resize); add amend PROC-038
- Source: TS INI `Columns.Processes`, `ColumnWidths`, `ColumnResizeBehavior=0`, L28484-28487 | MC remember-column-order | R-018
- Behaviour:
  - Drag a header to reorder; drag a header edge to resize (stops at the PROC-023 minimum).
  - "Resize Behavior: Proportional / Last Column", default Proportional (`tables.column_resize = "proportional"`, one key for every table, R-075).
  - "Resize columns to fit content" and "Reset to default".
  - Order, visibility and widths always persist in `$XDG_STATE_HOME/tasklens/state.toml` under `[columns.processes]` (`order`, `visible`, `widths`). MC's "Remember column order" option is dropped because tasklens always persists.
- Linux data/mechanism: state file saved atomically with a 500 ms debounce (ADR-011).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-026 Header context menu
- Was: v1 PROC-047; r2 PROC-040
- Source: TS L29313-29314, L28434, L28484, toasts L28648, L28652
- Behaviour:
  - Items: "Select Columns…", "Hide This Column", "Resize columns to fit content", "Resize Behavior >" (Proportional, Last Column), "Reset to default".
  - Toasts: "At least one column must stay visible" and "Columns reset to default".
  - "Hide This Column" is disabled on Name.
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-027 Columns: identity, image and package
- Was: v1 PROC-015; v1 PROC-016; r2 PROC-017
- Source: TS 2.1 table, L28342-28362, L28345-28349, L29128-29138, cross-check C.1, [CL 0.8.19] | MC columns name, pid | R-007, R-008

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Name | name | PROC-007 display name plus icon; app rows add the window title (PROC-053) | desktop Name / unit Description | BUILD | P1 |
| Name display | name_display | desktop Name for app primaries, else `comm` | same | ADAPT | P1 |
| Image name | image_name | basename of `readlink /proc/<pid>/exe`; Wine: the `.exe` name; " (deleted)" suffix kept | blank | ADAPT | P1 |
| Image path | image_path | `readlink /proc/<pid>/exe`; " (deleted)" suffix kept and shown in the danger colour | desktop file path | BUILD | P1 |
| Command line | cmdline | `/proc/<pid>/cmdline`, NUL to space, read limit 32 KiB; the cell shows the first 512 characters, Properties the full text (R-008) | primary process command line | BUILD | P1 |
| Description | description | desktop Comment, then GenericName, then package `%DESC%`; else blank | desktop Comment | ADAPT | P1 (package part P2) |
| Type | type | PROC-004 class: "App", "Background process", "System process", "Kernel thread", "Unit", "Pseudo" | "App" / "Unit" | ADAPT | P1 |
| Start time | start_time | `btime` from `/proc/stat` + starttime / `CLK_TCK`, local time; today `HH:MM:SS`, else `YYYY-MM-DD HH:MM` | earliest member | BUILD | P1 |
| Version | version | owning package version (below); Flatpak branch or commit; Wine PE `VS_VERSIONINFO` FileVersion; AppImage `X-AppImage-Version` | blank | ADAPT | P2 |
| Build date | timestamp | package `%BUILDDATE%`; Wine PE COFF `TimeDateStamp`; else `n/a` (ELF has no link time) | blank | ADAPT | P2 |
| File modified time | file_mtime | `statx` `stx_mtime` of the exe target | blank | BUILD | P2 |
| File size | file_size | `statx` `stx_size` | blank | BUILD | P2 |
| Package | company | owning package name, for example `firefox`; Flatpak: app ID; Wine PE CompanyName; `n/a` when unpackaged | blank | ADAPT | P2 |
| Packager | signer | package `%PACKAGER%` name without the email; Flatpak: origin remote | blank | ADAPT | P2 |
| Package status | signature | "Packaged"; "Packaged (modified)" only after a Properties integrity check against the mtree sha256 (PROC-078); "Not packaged"; "Deleted (upgraded)" when the exe link ends " (deleted)"; "Flatpak"; "Snap"; "AppImage" (exe under `/tmp/.mount_*`); "Wine" | blank | ADAPT | P2 |
| Architecture | architecture | ELF `EI_CLASS` + `e_machine`: "x86-64 (64-bit)", "i386 (32-bit)", "AArch64 (64-bit)"; Wine PE Machine: "x64 (64-bit, Wine)", "x86 (32-bit, Wine)". Read once per (dev, inode) | blank | ADAPT | P2 |

- Behaviour:
  - Text columns sort case-insensitively.
  - Package and file columns are computed in the background and cached by (path, mtime) or (dev, inode). Cells whose lookup has not finished show `…`. The first click on one of these headers starts a lookup for all rows with a progress tooltip; the sort applies when it finishes.
- Linux data/mechanism:
  - Arch: batched `pacman -Qoq <paths…>` for unseen exe paths, debounced 2 s, then `/var/lib/pacman/local/<pkg>-<ver>/desc` (`%VERSION%`, `%DESC%`, `%PACKAGER%`, `%BUILDDATE%`, `%URL%`). The cache is invalidated when the mtime of `/var/lib/pacman/local` changes. Debian uses `dpkg -S`, Fedora `rpm -qf`.
  - Flatpak: `/proc/<pid>/root/.flatpak-info` `[Application] name`, plus `/var/lib/flatpak/app/<id>/current/active/metadata`.
  - Wine: the PE header and version resource of the `.exe` from argv, with the path resolved through `WINEPREFIX` from environ, else `~/.wine` (`drive_c`).
  - Lookups never run on the UI thread.
- Verdict: see the table (Authenticode becomes package integrity)
- Phase: see the table
- Privilege: none for readable files. Other users' exe links give EACCES, so Image path and Image name are Restricted until the helper resolves them (`ReadProcessFiles { exe }`); the file and package columns then use the argv path when it is absolute and readable.

### PROC-028 Columns: ownership, session and security
- Was: v1 PROC-015 (User, PID, PID parent, Session); v1 PROC-017; r2 PROC-018
- Source: TS 2.1 table, section 9, L28354-28357, cross-check A.22, [CL 0.8.19] | R-007

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| User | user | real UID from status `Uid:` field 1, name via a cached `getpwuid_r`. Tooltip shows the effective UID when it differs | owner UID | BUILD | P1 |
| PID | pid | pid, format per PROC-018 | blank | BUILD | P1 |
| Parent PID | ppid | stat field 4 | blank | BUILD | P1 |
| Session | session | `/proc/<pid>/sessionid` (audit session, world-readable), mapped to the logind session ID; 4294967295 shows blank | blank | ADAPT | P2 |
| Security label | security_label | `/proc/<pid>/attr/current`: AppArmor "firefox (enforce)", SELinux context, "unconfined"; `n/a` when no major LSM is active per `/sys/kernel/security/lsm` | blank | ADAPT | P2 |
| Elevated | elevation | first match: "Full (root)" when euid is 0; "Setuid" when euid differs from ruid; "Capabilities" when CapEff is non-zero for a non-root process; else "Default" | blank | ADAPT | P2 |
| Sandbox | sandbox | first match: "Flatpak" (flatpak scope or `/.flatpak-info` in `/proc/<pid>/root`), "Snap" (cgroup `snap.`), "bwrap" (user namespace inode differs from init's), "Firejail", "seccomp + NoNewPrivs", "seccomp", "None"; from status `Seccomp` and `NoNewPrivs` | blank | ADAPT | P2 |

- Behaviour: as in the table.
- Linux data/mechanism: as in the table.
- Verdict: ADAPT (TaskSlinger's Integrity, Elevation and Protection mapped to Linux)
- Phase: see the table
- Privilege: `attr/current`, `sessionid` and `status` are world-readable. `ns/user` and `/proc/<pid>/root` need ptrace read access, so other users' rows fall back to the cgroup name for Flatpak and Snap and show Restricted for bwrap until the helper reads `ns`.

### PROC-029 Columns: CPU and scheduling
- Was: v1 PROC-018; v1 PROC-019; r2 PROC-019
- Source: TS 2.1 table, L28351-28353, L28361-28362, L28388-28395, L28455-28460, [CL 0.8.21] | MC `client.rs:880-889` | R-007, R-044

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| CPU | cpu | 100 × Δ(utime + stime) / `CLK_TCK` / Δt / online CPUs: percent of the whole machine | Σ | BUILD | P1 |
| CPU (kernel) | cpu_kernel | Δstime only, same scaling | Σ | BUILD | P1 |
| CPU (user) | cpu_user | Δutime only, same scaling | Σ | BUILD | P1 |
| CPU (one core = 100%) | cpu_relative | same Δ without the division by online CPUs (up to 3200% here) | Σ | BUILD | P1 |
| CPU time | cpu_time | (utime + stime) / `CLK_TCK` as `h:mm:ss` | Σ | BUILD | P1 |
| CPU time kernel | cpu_time_kernel | stime | Σ | BUILD | P1 |
| CPU time user | cpu_time_user | utime | Σ | BUILD | P1 |
| Priority | priority | class from policy (stat field 41) and nice (field 19): SCHED_FIFO or SCHED_RR "Realtime (rt N)"; SCHED_IDLE "Idle"; nice -20..-11 "High", -10..-1 "Above normal", 0 "Normal", 1..9 "Below normal", 10..19 "Low"; SCHED_BATCH appends " (batch)". Tooltip "nice N" | blank | ADAPT | P1 |
| Affinity | affinity | status `Cpus_allowed_list` of the leader thread, for example "0-7,16-23"; "All" when it equals the online set | blank | ADAPT | P1 |
| Cycles | cycles | `perf_event_open(PERF_COUNT_HW_CPU_CYCLES, exclude_kernel=1, inherit=1)` per thread, visible rows of own processes only, global cap 256 counters; past the cap, for other UIDs, or with `perf_event_paranoid` above 2: `n/a` | Σ | ADAPT | P4 |
| Cycles delta | cycles_delta | Δ of Cycles per interval | Σ | ADAPT | P4 |
| Context switches | ctx_switches | Σ over `/proc/<pid>/task/*/status` `voluntary_ctxt_switches + nonvoluntary_ctxt_switches`, visible rows only | Σ | ADAPT | P2 |
| Context switches delta | ctx_switches_delta | Δ per interval, visible rows only | Σ | ADAPT | P2 |

- Behaviour:
  - There is no "scale CPU to core count" setting. The two columns `cpu` and `cpu_relative` cover both views (R-044). MC's toggle is not built.
  - Cycles, Cycles delta, Context switches and Context switches delta cannot be sorted (PROC-047).
- Linux data/mechanism: as in the table; CPU math per ADR-008.
- Verdict: see the table
- Phase: see the table
- Privilege: none, except Cycles for other UIDs (`n/a`).

### PROC-030 Columns: memory
- Was: v1 PROC-020; r2 PROC-020; add amend PROC-020
- Source: TS 2.1 table, L28364-28372, L28396, L28411-28412, L28427 | MC columns memory, swap | D-004, R-005

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Memory | memory | RssAnon from status. Header tooltip "Private memory (RssAnon). Shared and file-backed pages are not counted." Picker alias `Working set private - "Memory"` | cgroup `memory.stat` `anon`, tooltip "Anonymous memory charged to this app's cgroup" | ADAPT | P1 |
| Working set total | ws_total | VmRSS. Tooltip "Resident set size (VmRSS)" | Σ | BUILD | P1 |
| Working set peak | ws_peak | VmHWM | blank | BUILD | P1 |
| Working set shared | ws_shared | RssFile + RssShmem | blank | BUILD | P1 |
| Commit size | commit | VmData + VmStk | Σ | ADAPT | P1 |
| Commit size peak | commit_peak | highest VmData + VmStk seen by tasklens since it first saw the process; tooltip "Peak seen by tasklens" | blank | ADAPT | P2 |
| Commit size delta | commit_delta | change since the previous sample | Σ | BUILD | P2 |
| Virtual size | vsize | VmSize | blank | BUILD | P1 |
| Virtual size peak | vsize_peak | VmPeak | blank | BUILD | P1 |
| Page tables | page_tables | VmPTE (TaskSlinger "Paged pool peak") | Σ | ADAPT | P2 |
| Non-paged pool peak | (none) | none | n/a | DROP: Linux has no per-process non-paged kernel pool; VmPTE is already Page tables | n/a |
| Page faults | page_faults | minflt + majflt (stat fields 10 and 12) | Σ | BUILD | P1 |
| Page faults delta | page_faults_delta | Δ per interval | Σ | BUILD | P1 |
| Major faults | major_faults (new) | majflt | Σ | ADAPT | P2 |
| Major faults delta | major_faults_delta (new) | Δ majflt per interval | Σ | ADAPT | P2 |
| Swap | swap | VmSwap. Not in the default set; MC's auto-hide when the machine has no swap is kept | cgroup `memory.swap.current` | BUILD | P1 |
| PSS | pss | `smaps_rollup` Pss, visible rows only (`detail_keys`), at most 1 Hz | Σ | ADAPT | P2 |
| USS | uss | `smaps_rollup` Private_Clean + Private_Dirty, same rules as PSS | Σ | ADAPT | P2 |
| Working set private | ws_private | Dropped: R-005 (duplicated `memory`) | | | |

- Behaviour:
  - Values use IEC units with 2 decimals (PROC-003). Sorting is numeric.
  - PSS and USS cannot be sorted; cells outside `ProcessSnapshot.coverage` show `…` (ADR-008).
  - MC's `RssAnon + RssShmem + RssFile/share` estimate is not built; the PSS column replaces it.
  - Column-graph labels: "Memory" for `memory`, "RSS" for `ws_total`, "RSS shared" for `ws_shared` (R-005, PERF-088).
- Linux data/mechanism: as in the table.
- Verdict: see the table
- Phase: see the table
- Privilege: `status` is world-readable. `smaps_rollup` needs ptrace read access, so it is Restricted for other users until the helper reads it.

### PROC-031 Columns: I/O and disk
- Was: v1 PROC-021; v1 PROC-022; r2 PROC-021
- Source: TS 2.1 table, L28372-28378, L28397-28410, INI width id 13 "Disk" | MC column drive (merged into Disk) | R-007, R-014

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Disk | disk | Δ(read_bytes + write_bytes − cancelled_write_bytes)/Δt from `/proc/<pid>/io`, MiB/s | PROC-008 rule (Σ, or cgroup `io.stat` when a member is Restricted) | ADAPT | P1 |
| Disk read rate | disk_read_rate | Δread_bytes/Δt | Σ / `io.stat` rbytes | BUILD | P1 |
| Disk write rate | disk_write_rate | Δwrite_bytes/Δt; tooltip shows cancelled_write_bytes | Σ / `io.stat` wbytes | BUILD | P1 |
| Disk total rate | disk_total_rate | Δ(read_bytes + write_bytes)/Δt | Σ | BUILD | P1 |
| Disk read bytes | disk_read_bytes | cumulative read_bytes | Σ / `io.stat` rbytes | BUILD | P2 |
| Disk write bytes | disk_write_bytes | cumulative write_bytes | Σ / `io.stat` wbytes | BUILD | P2 |
| Disk total bytes | disk_total_bytes | read_bytes + write_bytes | Σ | BUILD | P2 |
| Disk read bytes delta | disk_read_delta | Δ per interval | Σ | BUILD | P2 |
| Disk write bytes delta | disk_write_delta | Δ per interval | Σ | BUILD | P2 |
| Disk total bytes delta | disk_total_delta | Δ per interval | Σ | BUILD | P2 |
| Disk reads | disk_reads | process rows `n/a` with tooltip "Linux counts storage operations per cgroup, not per process" | `io.stat` rios summed over devices | ADAPT | P2 (cgroup), P4 (per process) |
| Disk writes | disk_writes | same with wios | `io.stat` wios | ADAPT | P2, P4 |
| Disk reads delta | disk_reads_delta | Δ rios | Δ | ADAPT | P2, P4 |
| Disk writes delta | disk_writes_delta | Δ wios | Δ | ADAPT | P2, P4 |
| I/O read bytes | io_read_bytes | rchar (all read syscalls, including cache hits, pipes and sockets) | Σ | BUILD | P2 |
| I/O write bytes | io_write_bytes | wchar | Σ | BUILD | P2 |
| I/O read bytes delta | io_read_delta | Δrchar | Σ | BUILD | P2 |
| I/O write bytes delta | io_write_delta | Δwchar | Σ | BUILD | P2 |
| I/O read rate | io_read_rate | Δrchar/Δt (TaskSlinger "I/O read+other rate") | Σ | ADAPT | P2 |
| I/O write rate | io_write_rate | Δwchar/Δt | Σ | BUILD | P2 |
| I/O read ops | io_read_ops | syscr | Σ | ADAPT | P2 |
| I/O write ops | io_write_ops | syscw | Σ | ADAPT | P2 |
| I/O other bytes | (none) | none | n/a | DROP: Linux has no per-process counter for I/O outside read and write (ioctl and similar) | n/a |

- Behaviour: as in the table. Rates use fixed MiB/s with 1 decimal (PROC-003).
- Linux data/mechanism:
  - As in the table. Per-process storage operations (P4) come from an eBPF `block:block_rq_issue` collector. It is a separate privileged binary, never `tl-helper` (ADR-006); its ADR is written with `tl-netmon` in P4.
- Verdict: see the table
- Phase: see the table
- Privilege: `/proc/<pid>/io` is Restricted for other UIDs until the helper reads it (`ReadProcessFiles { io }`). `io.stat` is world-readable.

### PROC-032 Column: network
- Was: v1 PROC-023; r2 PROC-022
- Source: TS INI width id 16, L35329 | MC column network (nethogs) | ADR-016 "Per-process network"
- Behaviour:
  - "Network" shows the attributed per-process TCP rate in the chosen unit (`units.network`, default bits; `units.network_auto_scale`, default off).
  - Header tooltip: "Attributed TCP traffic. UDP and other users' sockets are not counted."
  - A process with no attributable socket shows "0 Mbps". A process whose fds cannot be read shows the Restricted glyph.
  - A socket shared by several processes counts toward each of them, and the cell tooltip says "shared".
  - Extra columns: `net_send_rate` "Network send" and `net_recv_rate` "Network receive".
  - The column is never hidden on error; validity glyphs show instead. MC's nethogs scraping is not built.
- Linux data/mechanism:
  - `NETLINK_SOCK_DIAG` `inet_diag_req_v2` dump for TCP v4 and v6 with `INET_DIAG_INFO`; Δ`tcpi_bytes_acked` (send) and Δ`tcpi_bytes_received` (receive) per socket inode.
  - The socket inode to PID map comes from `/proc/<pid>/fd` readlinks (`socket:[ino]`), rebuilt every 2 s while demanded, shared with the Connections tab.
  - The header total includes unattributed traffic and says so.
  - Exact all-protocol counting comes later from `tl-netmon` (eBPF `cgroup_skb` or kprobes) under its own polkit action `io.github.oysteinkrog.tasklens.netmon`.
- Verdict: ADAPT
- Phase: P1 (TCP attribution); P4 (`tl-netmon` exact counting)
- Privilege: own processes need none. Other users' fds are Restricted until the helper resolves them (`ResolveSocketInodes`). `tl-netmon` needs CAP_BPF and CAP_PERFMON.

### PROC-033 Columns: GPU
- Was: v1 PROC-024; r2 PROC-023; add PROC-103; add amend PROC-024
- Source: TS 2.1, L28384-28385, L28428, L35456-35471, [CL 0.8.23], cross-check A.11, C.1 | MC columns gpu, gpu_memory | ADR-007 | R-007, R-026

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| GPU | gpu | max over adapters of the process's max(SM, encoder, decoder) utilization. NVIDIA: `nvmlDeviceGetProcessUtilization` in `tl-gpu-probe`; AMD and Intel: Δ`drm-engine-*` ns / Δt from `/proc/<pid>/fdinfo/*`, de-duplicated by `drm-client-id` | Σ | ADAPT | P1 |
| VRAM | gpu_mem_dedicated | NVIDIA: `usedGpuMemory` from graphics plus compute running processes (`_v3`), summed per PID across devices; AMD and Intel: fdinfo `drm-memory-vram` / `drm-resident-vram0`. Picker alias `GPU memory dedicated - "VRAM"` | Σ | ADAPT | P1 |
| GPU memory shared | gpu_mem_shared | AMD fdinfo `drm-memory-gtt`; Intel `drm-resident-system`; NVIDIA `n/a` (no per-process GTT in NVML) | Σ | ADAPT | P2 |
| GPU engine | gpu_engine | busiest engine label: "GPU 0 - 3D/Compute", "GPU 0 - Encode", "GPU 0 - Decode" from the same NVML samples; DRM engine name for others | most-used member engine | ADAPT | P2 |
| GPU encode | gpu_encode (new) | NVML encUtil; fdinfo `drm-engine-video*` | Σ | ADAPT | P2 |
| GPU decode | gpu_decode (new) | NVML decUtil; fdinfo `drm-engine-video*` | Σ | ADAPT | P2 |
| GPU 0 .. GPU 15 | gpu_0 .. gpu_15 | this process's utilization on adapter N | Σ | BUILD | P2 |
| (MC) GPU Memory | (merged into VRAM) | same data | | BUILD (merge) | P1 |

- Behaviour:
  - A process with no NVML sample this interval shows "0%" only when the probe reports that the device supports process utilization (a capability bit per device UUID). Otherwise `n/a`. GPU utilization is never divided among processes as a substitute (ADR-007).
  - GPU N slots: 16 slots, all off by default, offered in the picker only for adapters that exist (on the owner's machine "GPU 0", the RTX 5070 Ti). Adapter numbering is by PCI bus address ascending across NVIDIA and DRM render nodes, the same numbering as the Performance "GPU N" pages. Header line 2 shows that adapter's total utilization; the header tooltip gives the adapter name and PCI address. The cell tooltip shows that adapter's VRAM ("VRAM on GPU 0: 1.22 GiB"). Per-adapter memory columns are not built.
- Linux data/mechanism:
  - NVML runs only in `tl-gpu-probe` (ADR-007). Process data is requested at 1 Hz, only while a GPU column or graph needs it, whatever the Refresh setting.
  - NVML PIDs are mapped to `ProcessKey` through the current process table; a PID not in the table is dropped.
  - fdinfo is empty on this NVIDIA driver, so NVML is the only per-process source here.
  - Unverified: U-12 (NVML per-process utilization unprivileged on driver 615).
- Verdict: see the table
- Phase: see the table
- Privilege: none for NVML. fdinfo of other users' processes is Restricted until the helper reads it (`ReadProcessFiles { fdinfo }`).

### PROC-034 Columns: Power usage and Power usage trend
- Was: v1 PROC-025; r2 PROC-024
- Source: TS 2.1 ids 79 and 80, L28378-28383 | ADR-016 | R-006, R-028, R-029, R-059
- Behaviour:
  - "Power usage" (`power`) shows a class: "Very low" below 0.5 W, "Low" below 2 W, "Moderate" below 8 W, "High" below 25 W, "Very high" from 25 W. The tooltip gives the estimate, for example "≈ 3.4 W (estimate)".
  - "Power usage trend" (`power_trend`) uses the same classes over a 2-minute moving average.
  - Both are off by default on every machine (R-006).
  - When RAPL exists but `energy_uj` is not readable: `n/a` with the tooltip "Needs setup", which links to the "tasklens Additional Setup" window row "CPU power draw" (R-028). When no power source exists at all: `n/a` with the tooltip "No power sensor available". A guessed class is never shown.
  - App rows sum their processes.
- Linux data/mechanism:
  - Estimate = P_pkg × (process Δcpu_time / total busy Δcpu_time) + Σ over GPUs of P_gpu × (process SM util / Σ SM util on that GPU).
  - P_pkg from Δ`/sys/class/powercap/intel-rapl:0/energy_uj`, with wrap from `max_energy_range_uj` (AMD uses the same driver name).
  - P_gpu from NVML `nvmlDeviceGetPowerUsage` through the probe, or hwmon `power1_average` for amdgpu.
  - Enabling RAPL access is `tasklens-setup rapl-enable` under polkit `io.github.oysteinkrog.tasklens.setup`. It writes `/etc/udev/rules.d/99-tasklens-powercap.rules` with the line `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`, then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add` (R-029). Mission Center's `99-powercap.rules` is detected and left alone.
- Verdict: ADAPT (measured-power share model in place of Windows' energy estimator)
- Phase: P2 (the setup window is P3)
- Privilege: `energy_uj` has been root-only since kernel 5.10; readable here because of Mission Center's rule.

### PROC-035 Columns: object counts
- Was: v1 PROC-026; r2 PROC-025
- Source: TS 2.1, L28390-28391, L28248-28249, L35512 | R-007

| Label | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Handles | handles | fd count: `fstatat` `st_size` of `/proc/<pid>/fd`, readdir count as fallback. Tooltip "Open file descriptors" | Σ | ADAPT | P1 |
| Threads | threads | stat field 20 `num_threads`, readable for all UIDs | Σ | BUILD | P1 |
| Windows | windows | top-level windows owned by the PID, from the KWin bridge (PROC-052); `n/a` without the bridge (TaskSlinger "USER objects") | Σ | ADAPT | P3 |
| X11 resources | x11_resources | XRes `XResQueryClientIds` (PID mask) plus `XResQueryClientResources` total on XWayland; "n/a (Wayland client)" for others (TaskSlinger "GDI objects") | Σ | ADAPT | P4 |

- Behaviour: integers, right-aligned, thin-space grouped.
- Linux data/mechanism: as in the table. XRes runs over `x11rb` on `$DISPLAY` only while the column is visible (U-32).
- Verdict: see the table
- Phase: see the table
- Privilege: the fd count of other UIDs is Restricted until the helper reads it (`ReadProcessFiles { fd_list }`).

### PROC-036 Linux-only extra columns
- Was: v1 PROC-027; r2 PROC-026
- Source: MC inventory section 8 | ADR-016

| Label | ID | Linux source | Verdict | Phase |
|---|---|---|---|---|
| State | state | stat field 3: "Running" (R), "Sleeping" (S), "Disk sleep" (D), "Stopped" (T), "Tracing stop" (t), "Zombie" (Z), "Idle" (I), "Dead" (X); "Frozen" when the cgroup's `cgroup.events` has `frozen 1` | BUILD | P1 |
| Nice | nice | stat field 19 | BUILD | P1 |
| Scheduling policy | sched_policy | stat field 41: OTHER, BATCH, IDLE, FIFO, RR, DEADLINE, plus the rt priority (field 40) for FIFO and RR | BUILD | P2 |
| I/O priority | io_priority | `ioprio_get(IOPRIO_WHO_PROCESS, pid)`: "best-effort 4", "idle", "realtime 0", "none (from nice)" | BUILD | P2 |
| Unit | unit | systemd unit name from the cgroup path | BUILD | P1 |
| Cgroup | cgroup | full v2 path (`0::` line) | BUILD | P2 |
| App ID | app_id | app ID from PROC-005 and PROC-006 (desktop ID or Flatpak ID) | BUILD | P2 |
| OOM score | oom_score | `/proc/<pid>/oom_score`; tooltip shows `oom_score_adj` | BUILD | P2 |
| OOM adj | oom_score_adj (new) | `/proc/<pid>/oom_score_adj` | BUILD | P2 |
| TTY | tty | stat field 7 decoded to `pts/3` or `ttyN` | BUILD | P2 |
| Working directory | cwd (new) | `readlink /proc/<pid>/cwd` | BUILD | P2 |
| Process group | pgrp (new) | stat field 5 | BUILD | P4 |
| SID | sid | stat field 6 | BUILD | P4 |
| Last CPU | last_cpu (new) | stat field 39 | BUILD | P4 |
| Container | container (new) | PID namespace differs from init's: "container (podman)", "container (docker)", "container (systemd-nspawn)" from the cgroup path (`machine.slice`, `docker-<id>.scope`, `libpod-<id>.scope`) | ADAPT | P4 |

- Behaviour: listed under the "Linux" picker heading. None are visible by default.
- Linux data/mechanism: as in the table.
- Verdict: ADAPT (MC wins on Linux-only features)
- Phase: see the table
- Privilege: `cwd`, `ns` and `ioprio_get` of other UIDs are Restricted until the helper reads them. Everything else is world-readable.

### PROC-037 TaskSlinger column id map
- Was: v1 PROC-028; add amend PROC-028
- Source: TS INI 96 ids (0-95) | cross-check C.1
- Behaviour:
  - TaskSlinger's 96 ids are 80 named columns plus "GPU 0" to "GPU 15". All are covered by PROC-027 to PROC-035 (two dropped, one merged).
  - Reference map only: confirmed 0 Name, 1 User, 3 PID, 6 CPU, 7 Memory, 13 Disk, 16 Network, 17 GPU, 33 Description, 79 Power usage; inferred 18 VRAM, 62-77 GPU 0-15, 80 Power usage trend, 81-83 Package, Packager, Package status.
  - tasklens uses string IDs, so the unmapped ids (2, 4, 5, 8-12, 14, 15, 19-32, 34-61, 84-95) need no resolution (U-38). `state` is a Linux extra, not a TaskSlinger column.
- Linux data/mechanism: none.
- Verdict: ADAPT (numeric ids become string IDs)
- Phase: P1
- Privilege: none

---

## E. Header, display, filter, sort and selection

### PROC-038 Two-line header with system totals
- Was: v1 PROC-029; r2 PROC-028
- Source: TS 2.1 Header, [IMG processes.png "CPU / 11.9%"], cross-check B.10 | MC `table_view/mod.rs:852-948`
- Behaviour:
  - Line 1 is the column name with the sort arrow ("PID ↑"). Line 2 is the system total, right-aligned, in secondary text colour.
  - Totals:
    - CPU: total utilization from `/proc/stat` (ADR-008 math; kernel = system + irq + softirq).
    - Memory: (MemTotal − MemAvailable) / MemTotal %.
    - Swap: used %.
    - Disk: highest busy % among physical disks (MC).
    - Network: total physical-interface throughput as % of the summed link speed; a rate when the link speed is unknown.
    - GPU: highest utilization among GPUs. GPU N: that adapter's utilization.
    - VRAM: used / total % over all GPUs.
    - Threads: system thread count.
    - Handles: `/proc/sys/fs/file-nr` field 1.
  - Name, User, PID, Power usage, Description and other text columns have no line 2. With no GPU, the GPU totals show `n/a`.
  - Percent totals use one decimal (PROC-003).
- Linux data/mechanism: the system collectors' snapshot (Performance workers) at the time of the process sweep, from `ProcessTableView.totals`.
- Verdict: BUILD (TaskSlinger layout, MC formulas)
- Phase: P1
- Privilege: none

### PROC-039 Row hover tooltip
- Was: v1 PROC-032; r2 PROC-013; add amend PROC-032
- Source: TS 2.1 tooltip L35703, L28447, [CL 0.8.27], cross-check A.5 | R-009
- Behaviour: after 600 ms of hover over the Name cell, a tooltip shows these lines in order, each only when it applies:
  ```
  <command line, first 512 characters>
  path: <exe path, or "restricted">
  user: <name> (uid N)
  Unit: <unit> (<Description>)
  cgroup: <path>
  Package: <name> <version>              (when known, P2)
  Desktop file: <path>                   (app rows only)
  Some values need administrator access. (restricted rows only)
  ```
  - "Unit:" is the systemd analogue of TaskSlinger's "Services:".
  - Pseudo rows show only a one-line description of what they count.
- Linux data/mechanism: cached metadata in the snapshot; the package lookup from PROC-027.
- Verdict: ADAPT
- Phase: P1 (Package line P2)
- Privilege: none

### PROC-040 Heat colouring
- Was: v1 PROC-033; r2 PROC-029
- Source: TS INI `UI.ProcessValueColorCoding=1`, `ProcessValueColorCodingIntensityPct=50`, View > Appearance "Value Color Coding" | R-059
- Behaviour:
  - Numeric resource cells get a background blended from transparent toward VALUE_HEAT (#3E2A28 in Default Dark) with `alpha = intensity × clamp(value / reference, 0, 1)`.
  - References: CPU 400/ncpu % (12.5% here); CPU (one core = 100%) 400%; memory columns 10% of MemTotal; disk and I/O rates 100 MiB/s; network 10 MiB/s, or 80 Mbps in bits mode; GPU 100%; VRAM 25% of total VRAM; Power usage 25 W.
  - Count columns are not tinted.
  - View > Appearance > "Value Color Coding" (`appearance.heat_coloring = true`) and an intensity slider 0-100% (`appearance.heat_intensity_pct = 50`).
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-041 Dim zero values
- Was: v1 PROC-034; r2 PROC-030
- Source: MC `apps-page-hide-is-zero`
- Behaviour: Settings > Processes > "Dim zero values", default off (`tables.dim_zero_values = false`, one key for every table, R-075). A measured zero draws at 30% opacity. It never applies to validity glyphs.
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-042 Column separators
- Was: v1 PROC-035
- Source: MC `apps-page-show-column-separators`
- Behaviour: Settings > Processes > "Show column separators", default off (`tables.column_separators = false`, one key for every table, R-075). It draws 1 px DIVIDER lines between columns.
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P4
- Privilege: none

### PROC-043 Highlight new and exited processes
- Was: v1 PROC-036; r2 PROC-031
- Source: TS 1.3, INI `UI.HighlightNewExitedProcesses=1`, L30335 "process diff linger", [CL 0.9.0] | R-041
- Behaviour:
  - View > Process View > "Highlight New and Exited Processes", default on (`processes.highlight_new_exited = true`).
  - New rows get the theme success colour at 35% alpha.
  - Exited rows stay in place with the theme error colour at 35% alpha and greyed text. They are not actionable: every menu item except Copy is disabled, with tooltip "Process exited".
  - Linger time is 2 × `processes.interval_ms`, clamped to 1..=3 s. The tint fades out over the last 300 ms, or switches off at the end when reduced motion or Animations is off.
- Linux data/mechanism: identity diff between sweeps in the engine. The reducer keeps exited records for the linger time, then removes them (ADR-004).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-044 Filter box and type-to-filter
- Was: v1 PROC-041; r2 PROC-032
- Source: TS 1.7, L28339, L30133, [CL 0.8.28, 0.8.32] | MC `table_view/mod.rs:604-638` | R-043 | APP-028
- Behaviour:
  - "Filter: " field in the tab bar with a clear button; it expands on focus.
  - Typing a printable character while the process table has focus starts the filter, unless an input or dialog has focus.
  - One Esc clears the filter and returns focus to the table. A further Esc follows the global Esc rule.
  - Matching is a case-insensitive substring over display name, image name, command line, user, description, app name, unit name and group names. A numeric query also matches PIDs by prefix. Space-separated terms must all match.
  - MC's reverse containment and Levenshtein matching are not used (false hits).
  - Group headers stay visible with their filtered counts.
  - The filter also applies to the History table and graph (PROC-088).
- Linux data/mechanism: normalized lowercase strings cached per identity (`ProcessMeta.search_lower`). The UI debounces input 100 ms, then sends `SetTableQuery` with a new generation; results for an older generation are discarded (ADR-009). There is no `filter_cmdline` setting (R-043).
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-045 Include children
- Was: v1 PROC-042; r2 PROC-033
- Source: TS 1.7, L28338, L28831-28832, INI `UI.FilterShowChildren=0`
- Behaviour:
  - An "Include children" checkbox shows while filtering and in Tree View. Default off (`processes.filter_include_children = false`).
  - When on, every descendant of each match also shows.
  - Toasts: "Including filtered process children." when turned on, "Hiding unmatched filtered process children." when turned off.
- Linux data/mechanism: tree projection in the engine.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-046 Hide system processes
- Was: v1 PROC-012; r2 PROC-034; add amend PROC-012
- Source: TS 1.1 grid button, L28330-28331, L28738-28739, L35335-35345, INI `UI.HideMicrosoftProcesses=0`, [CL 0.9.2] | APP-004 | R-031
- Behaviour:
  - A toolbar toggle on the Processes tab only (PROC-016). Tooltip "Hide system processes" or "Show system processes". Toasts "Hiding system processes" and "Showing system processes". Default off (`processes.hide_system = false`).
  - When on, these are hidden: the whole System processes group (kernel threads, the "Kernel threads (N)" row, other users' processes, system units), pseudo rows (PROC-012), and Background rows of desktop infrastructure.
  - Desktop infrastructure means user units in `session.slice` and `background.slice`, for example pipewire, wireplumber, dbus-broker, xdg-desktop-portal*, at-spi, kded6, ksmserver, powerdevil, plus `init.scope` (`systemd --user`).
  - These stay visible: `plasma-plasmashell.service` and `plasma-kwin_wayland.service` (the Explorer analogue), `org.gnome.Shell@wayland.service`, and everything in `app.slice`.
  - In History mode the toggle stays active and also filters the History table.
- Linux data/mechanism: the PROC-004 cgroup classes plus the unit name; the exception list is a constant. `hide_system` is part of `EngineSettings`.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-047 Sorting
- Was: v1 PROC-037; r2 PROC-035
- Source: TS 2.1 Header, INI `Sort.ProcessesKey=3`, `ProcessesAscending=1` | MC `table_view/mod.rs:226`, `settings.rs:55-101` | R-011 | ADR-008, ADR-009
- Behaviour:
  - Click anywhere on a header cell to sort. The first click is descending for numeric columns and ascending for text; each further click reverses.
  - Default: PID ascending. The History table defaults to CPU descending (PROC-088).
  - Groups keep their fixed order. In Tree View and inside groups, siblings sort under their parent.
  - Ties break by `ProcessKey`.
  - The table re-sorts at every refresh unless paused (PROC-050).
  - Sort always persists in `state.toml` `[sort.processes]` (`column`, `ascending`). MC's "Remember sorting" option is not built.
  - Columns measured for visible rows only (PSS, USS, Cycles, Cycles delta, Context switches, Context switches delta) cannot be sorted. Their header is not clickable and shows the tooltip "Only measured for visible rows".
- Linux data/mechanism: engine-side projection tagged with a generation (ADR-009).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-048 Selection and multi-select
- Was: v1 PROC-039; r2 PROC-036; add amend PROC-039
- Source: TS [CL 0.8.4], L28336, L29280-29285, cross-check A.2 | MC single selection only
- Behaviour:
  - Click selects one row. Ctrl+click toggles a row. Shift+click selects a range. Shift+Up/Down extends. Ctrl+A selects all visible process rows.
  - Right-click on an unselected row selects only that row first. Right-click inside the selection keeps it.
  - Group header rows cannot be selected for actions.
  - Selection is a set of identities (`FxHashSet<NodeId>` with anchor and focus) and survives re-sorting and filtering. Exited identities drop out after the linger time.
  - Multi-select actions act on the whole selection: End Selected Tasks, End Selected Processes, Suspend Selected Tasks, Resume Selected Tasks, Send Signal, Set Priority, Set Affinity, Set I/O Priority, Copy, Copy details, Copy all metrics, Copy rows as table (PROC-069, PROC-070).
  - Properties with several rows selected opens one window per selected process, up to the cap of 8 (PROC-077).
- Linux data/mechanism: `tl-ui/src/table/selection.rs` (ADR-009).
- Verdict: BUILD (TaskSlinger multi-select)
- Phase: P1
- Privilege: none

### PROC-049 Keyboard in the process table
- Was: v1 PROC-040; r2 PROC-037; add amend PROC-040
- Source: TS 7, [CL 0.8.4], cross-check C.4 | MC `table_view/mod.rs:323-470` | RECONCILIATION table (b), R-045, D-008
- Behaviour (key context ProcessTable inside Table; one GPUI Action per row, and menu items dispatch the same Action):

  | Keys | Action |
  |---|---|
  | Up / Down / PgUp / PgDn / Home / End | move the row selection (rows, never columns) |
  | Shift+Up/Down, Shift+click, Ctrl+click | extend or toggle the selection |
  | Ctrl+A | select all visible rows |
  | Right / Left | expand, or move to the first child / collapse, or move to the parent |
  | Space | toggle expand |
  | Enter, Alt+Enter, Ctrl+I | Properties (D-008) |
  | Delete, Ctrl+E | End Task (SIGTERM) |
  | Shift+Delete, Ctrl+X | End Process (SIGKILL) |
  | Ctrl+C | Copy details (PROC-070) |
  | Ctrl+Shift+C | Copy rows as table (TSV with a header line) |
  | Ctrl+L | Collapse all |
  | Ctrl+F | focus the filter (global) |
  | Shift+F10, Menu key | open the context menu for the selection |
  | printable character | start filtering (not when an input or dialog has focus) |
  | Hold Ctrl | pause the display (PROC-050) |

- Linux data/mechanism: GPUI actions bound in the `ProcessTable` key context.
- Verdict: ADAPT (TaskSlinger evidences no table keys beyond typing and multi-select; the set is fixed by table b)
- Phase: P1
- Privilege: none

### PROC-050 Hold Ctrl to pause
- Was: v1 PROC-043; r2 PROC-038
- Source: TS L28337, [CL 0.8.10] | MC `window.rs:863-907` | R-042 | ADR-009
- Behaviour:
  - While Ctrl is held and the main window is active (whatever has focus inside it), the displayed rows, order and values freeze, and the status bar appends "[PAUSED - Ctrl]".
  - Collection and history keep running.
  - Releasing Ctrl or deactivating the window resumes and applies the latest snapshot.
  - Actions taken while paused use the captured identities and revalidate them.
- Linux data/mechanism: the table renders from one retained `paused: Option<Arc<Snapshot>>` (ADR-005 allows at most this one extra snapshot).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-051 Processes status bar text
- Was: v1 PROC-044; r2 PROC-039
- Source: TS 1.8, L28335-28337, L30142
- Behaviour, bottom left:
  - "296 Processes", or "41 / 296 Processes" while filtering.
  - "3 processes selected" when 2 or more rows are selected.
  - "[PAUSED - Ctrl]" appended while paused.
  - " | 42 restricted" appended when restricted rows exist, with the tooltip "Some values for other users' processes need administrator rights. Click to enable." From P3, clicking it starts the helper (PROC-076); before P3 it is not clickable.
  - The count excludes pseudo rows and includes kernel threads only when they are shown.
- Linux data/mechanism: projection counts.
- Verdict: BUILD (plus the restricted count)
- Phase: P1
- Privilege: none

---

## F. KWin bridge and window features

### PROC-052 KWin window bridge
- Was: v1 PROC-030; r2 PROC-059
- Source: TS window features L29074-29080, L29201-29213, [CL 0.8.30] | PLAN section 7 | R-001, R-046 | ADR-003, ADR-004, ADR-005
- Behaviour: gives tasklens a live map of PID to windows (title, class, desktop file, state, geometry, internal ID) on KDE Plasma. App titles, the Name window button, Show Window, the window picker, the Windows column, the Properties window block and the pin buttons use it. On any other desktop these features are hidden and the toast "Window features need KDE Plasma" is shown once when one is first requested.
- Linux data/mechanism:
  - A `tl-engine` D-Bus worker `workers/dbus_kwin.rs`, `SourceId::KWin`, event-driven, on the engine's session-bus connection.
  - Availability: `XDG_CURRENT_DESKTOP` contains `KDE` and `org.kde.KWin` is on the bus.
  - At start it writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`, loads it with `org.kde.kwin.Scripting.loadScript(path, "tasklens-bridge")` and starts it.
  - tasklens exports `/io/github/oysteinkrog/Tasklens/WindowBridge`, interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s json)`, on its bus name `io.github.oysteinkrog.Tasklens`.
  - The script sends the full window list once, then an update on `workspace.windowAdded`, `windowRemoved`, `windowActivated` and each window's `captionChanged`, through `callDBus(...)` to that method. Each entry carries `internalId`, `pid`, `caption`, `resourceClass`, `desktopFileName`, `minimized`, `maximized`, `keepAbove`, `frameGeometry` and `windowType`.
  - The engine publishes it as the `Snapshot.windows: Arc<WindowsSnapshot>` component keyed by PID; without the bridge it is empty with validity `Unsupported`.
  - Commands (activate, close, keep above, pick) arrive as `Command::KWin(KWinCommand)` and run as one-shot scripts `tasklens-cmd-<n>` with the parameters embedded, which unload themselves. KWin scripts cannot receive D-Bus signals (U-13). The picker uses the script `tasklens-pick`.
  - On exit tasklens calls `unloadScript("tasklens-bridge")`.
  - The X11 EWMH fallback from r2 is Dropped: R-046.
- Verdict: ADAPT (replaces Win32 window enumeration)
- Phase: P2
- Privilege: none. KWin scripting is open to session-bus clients.

### PROC-053 App window titles and the Name-cell window button
- Was: v1 PROC-031; r2 PROC-014
- Source: TS [IMG adv_sheet_01 "ShareX - Image editor - 1280x800"], 2.1 [CL 0.8.30]
- Behaviour:
  - App rows show the title of their most recently active window as dimmed text after the name (PROC-008).
  - Rows that own one or more top-level windows show a small window icon at the right edge of the Name cell, on hover and on the selected row. Clicking it opens a popup listing each window by title ("Untitled window" when the caption is empty). Clicking an entry activates that window.
  - For app rows the list covers all PIDs of the app.
  - Without the bridge there is no title and no button.
- Linux data/mechanism: `WindowsSnapshot` joined by PID; activation through `KWinCommand` activate.
- Verdict: ADAPT (HWND enumeration becomes the KWin bridge)
- Phase: P2
- Privilege: none

### PROC-054 Show Window
- Was: v1 PROC-061; r2 PROC-057
- Source: TS L29289, toasts "Process window shown.", "No window found"
- Behaviour: "Show Window" brings the most recently active window of the process (or, for app rows, of any process in the app) to the front and un-minimizes it. The item is enabled only when such a window exists, and hidden when the bridge is unavailable. Toasts: "Process window shown." and "No window found".
- Linux data/mechanism: one-shot KWin script `w.minimized = false; workspace.activeWindow = w;` for the window whose `pid` matches. The X11 `_NET_ACTIVE_WINDOW` fallback is Dropped: R-046.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-055 Window picker ("+" button)
- Was: v1 PROC-062; r2 PROC-058
- Source: TS 1.1, 7, `chrome.window_picker` L28333, L30131, tooltips L29201-29213, [CL 0.8.4] | APP-002, APP-006
- Behaviour:
  - A toolbar "+" button with tooltip "Select process by window", on every tab (APP-002).
  - Clicking it arms the picker with the toast "Click a window to select its process. Hold CTRL while clicking to terminate process. Esc cancels." KWin shows its crosshair.
  - A plain click on a window selects its process: tasklens switches to the Processes tab, clears a filter that hides it, and scrolls to it through the jump target (PROC-071).
  - Ctrl+click ends that process with SIGTERM after a confirmation. tasklens reads Ctrl from GPUI modifiers when it has focus; when it cannot see the modifier, the confirmation dialog offers "Terminate" next to "Select".
  - Desktop, panel, notification and OSD windows are ignored, with the toast "No window found". Esc gives "No window selected".
  - Picking a tasklens window selects tasklens. A PID that is not in the table gives "Unknown process".
  - Hidden on non-KDE sessions.
- Linux data/mechanism:
  - `org.kde.KWin.queryWindowInfo()` on `/KWin` does the interactive pick and returns the window's map with `uuid`.
  - If the map has no `pid` (U-14), the `tasklens-pick` script looks up `workspace.windowList()` by `internalId == uuid` and returns the PID over the bridge.
  - Shell windows are filtered by `windowType` (desktop, dock, notification, onScreenDisplay).
  - The X11 pointer-grab fallback is Dropped: R-046.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### PROC-056 Pin on top for tasklens windows
- Was: v1 PROC-063
- Source: TS L30218-30221 pin, [CL 0.8.25] | APP-009 | R-073
- Behaviour: the pin button is shown on every secondary window (Process Properties, App Properties, Run, Handle Search, the rescue window) and on the main window. When pinned, the window stays above other windows. Only the main window's state persists (`window.always_on_top`).
- Linux data/mechanism: on KDE it sets `keepAbove` through the bridge (one-shot script matched by tasklens's PID and window caption); on X11 it sends `_NET_WM_STATE_ABOVE`; elsewhere the button is disabled with the tooltip "Not supported by this desktop". Wayland has no client protocol for this.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

---

## G. Row context menu and process actions

### PROC-057 Row context menu
- Was: v1 PROC-049; r2 PROC-041; add amend PROC-049
- Source: TS 2.1 menu table, [IMG adv_sheet_01/02], L29277-29306, cross-check A.1, A.2, B.9 | MC `table_view.blp:121` | R-033, R-034
- Behaviour:
  - Process row, in this order ("|" is a separator):
    1. End Task
    2. End Process
    3. End Process Tree
    |
    4. Suspend Process / Resume Process (only the one that applies)
    5. Send Signal >
    6. Set Priority >
    7. Set Affinity…
    8. Set I/O Priority >
    |
    9. Show Window
    10. Restart Plasma Shell / Restart unit "<unit>" (only where it applies)
    |
    11. Copy
    12. Copy details
    13. Copy all metrics
    14. Copy rows as table
    |
    15. Add Minigraph >
    16. Add Process Name Graph >
    17. Add App Name Graph >
    18. Add tray icon graph > / Remove tray icon graph
    |
    19. Properties
    20. File properties
    21. Open file location
    22. Open in VirusTotal
    23. Create dump file
    |
    24. Go to parent process
    25. Go to service (when the process is a unit's MainPID)
  - App rows: item 1 reads "End App"; item 2 becomes "Kill App (all processes)"; item 3 is not shown; item 4 becomes "Freeze App" / "Thaw App". Priority, affinity and I/O priority apply to all processes of the app.
  - Unit rows: as app rows, plus "Go to service".
  - Multi-select: items 1, 2 and 4 read "End Selected Tasks", "End Selected Processes", "Suspend Selected Tasks" / "Resume Selected Tasks".
  - Pseudo rows: Copy, Copy details, Copy all metrics and Add Minigraph only.
  - Exited rows: every item disabled except Copy.
  - Group header rows: Expand / Collapse only.
  - An item whose phase has not shipped is not shown. Window items are hidden without the KWin bridge.
  - The menu captures the target keys when it opens (PROC-001).
- Linux data/mechanism: gpui-kit `PopupMenu` from the custom table (PROC-013). Every item dispatches the same GPUI Action as its key binding.
- Verdict: ADAPT (TaskSlinger order, MC signal submenu, Linux items)
- Phase: P1 for the menu and the P1 items; other items arrive with their own entries.
- Privilege: per item
- Item 14 "Copy rows as table" is the menu label for Ctrl+Shift+C (PROC-070), placed after Copy all metrics (R-091).

### PROC-058 Action pipeline, results and toasts
- Was: r2 PROC-042; v1 PROC-050 (result channel, multi-target toast); v1 PROC-001 (revalidation)
- Source: TS process toasts L28645-28926 | ADR-005, ADR-006 | R-035
- Behaviour:
  - Every action captures its targets when the menu opens and revalidates each one just before acting.
  - Toasts appear at the top centre. Updates for one action replace the same toast.
  - Short toasts: "Process terminated", "Process tree terminated", "Process suspended.", "Process resumed.", "Priority updated", "Affinity updated", "I/O priority updated", "Selected process priority updated.", "End task requested for selected processes.".
  - Long-form errors: "Could not end task.", "Could not end process.", "Could not update selected process affinity.".
  - Identity mismatch: "The selected process changed; wait for the list to update and try again."
  - EPERM: "Permission denied" with a "Retry as administrator" button that sends the same request with `privileged = true` to the helper (PROC-076). Before P3 the toast has no button. There is no automatic retry; MC's automatic `pkexec kill` is not built.
  - Multi-target End: "Ended 4 of 5 processes (1 permission denied)".
  - A pending action older than its deadline plus 5 s shows "Outcome unknown". Destructive actions are never retried automatically.
- Linux data/mechanism:
  - `Command::Action(ActionRequest)` with up to 256 targets; admission limits (16 in flight) and event order per ADR-005.
  - Results arrive on the `action_events` channel, never through the snapshot. States: Rejected, Started, Succeeded, Failed, OutcomeUnknown.
  - Deadlines per ADR-006: 2 s per target for signal, nice, affinity, ioprio and freeze writes; 120 s for any call that may show a polkit prompt; a timeout reports OutcomeUnknown.
  - `ActionError` text comes from `tl-ui/src/actions.rs::message`, for example "Permission denied. This process belongs to another user."
  - Action logs record kind, target PID and result only (ADR-015).
- Verdict: BUILD
- Phase: P1 (retry button P3)
- Privilege: per action

### PROC-059 Protection rules and confirmations
- Was: v1 PROC-055; r2 PROC-052
- Source: TS L28779-28781 ("Refusing to terminate the current TaskSlinger process.", "Windows marked the selected process as critical...") | ADR-006 | R-036, D-009
- Behaviour:
  - Refused with a toast and no dialog:
    - tasklens itself: "Refusing to end tasklens itself."
    - PID 1: "PID 1 (systemd) cannot be ended."
    - kernel threads: "Kernel threads cannot be signalled."
  - A confirmation dialog opens for:
    - any signal, freeze or kill on a protected target: "Ending <name> will end your desktop session. End it anyway?";
    - End Process Tree: "End N processes in this tree?", buttons "End tree" and "Cancel";
    - Kill App: "Kill all N processes of <App>?";
    - End Task or End Process with more than one target;
    - any action on another user's process.
  - No confirmation for a single End Task or End Process on one of the user's own unprotected processes (D-009). End Process then shows only the toast "End process requested.".
  - Protected set (`ProcFlags::PROTECTED`): PID 1, the session's `kwin_wayland`, `plasmashell`, `ksmserver`, `Xwayland`, `sddm`, `systemd-logind`, the session's `login` ancestor, the user's `systemd --user`, the user's `dbus-broker` or `dbus-daemon`, and tasklens itself.
- Linux data/mechanism:
  - The compositor PID comes from `SO_PEERCRED` on a connection to `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`.
  - The session bus daemon PID comes from `org.freedesktop.DBus.GetConnectionUnixProcessID("org.freedesktop.DBus")`.
  - `Xwayland`, `plasmashell` and `ksmserver` are found by exe basename in the session; `systemd --user` is the `init.scope` of `user@<uid>.service`; `sddm`, `login` and `systemd-logind` by walking the session leader's ancestors and by unit name.
  - Refusals map to `Rejected(Protected)` and `Rejected(KernelThread)`.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-060 End Task and End App (SIGTERM)
- Was: v1 PROC-050; r2 PROC-043
- Source: TS L29277-29280, L28777-28781 | MC Stop (SIGTERM, bwrap unwrap) | R-035
- Behaviour:
  - Process rows: send SIGTERM; toast "End task requested.", changing to "Process terminated" when the process is gone, or "Could not end task.".
  - App and unit rows ("End App"): SIGTERM to each primary process. A `bwrap` primary is replaced by its first non-bwrap descendant (PROC-010).
  - Multi-select ("End Selected Tasks"): confirmation first (PROC-059), then toast "End task requested for selected processes." and the multi-target result toast.
- Linux data/mechanism: `ActionKind::Signal(Term)` through the verified pidfd.
- Verdict: ADAPT (window close or TerminateProcess becomes a graceful SIGTERM)
- Phase: P1
- Privilege: own processes need none. EPERM offers "Retry as administrator" (helper `Signal`).

### PROC-061 End Process (SIGKILL)
- Was: v1 PROC-051; r2 PROC-044; add PROC-096; add amend PROC-051
- Source: TS L29278, toasts L28845-28846, cross-check A.1 | MC Force Stop | R-033, R-034, D-009
- Behaviour:
  - "End Process" sits directly under "End Task" and sends SIGKILL. Tooltip "Ends the process immediately (SIGKILL)".
  - Toasts: "End process requested." then "Process terminated", or "Could not end process.".
  - Multi-select label "End Selected Processes", with confirmation (PROC-059).
  - On app and unit rows the item is "Kill App (all processes)" (PROC-064).
  - Keys: Shift+Delete and Ctrl+X (PROC-049). TaskSlinger's "Force End Task" label is not used.
- Linux data/mechanism: `ActionKind::Signal(Kill)` through the verified pidfd.
- Verdict: ADAPT (TerminateProcess becomes SIGKILL)
- Phase: P1
- Privilege: as PROC-060

### PROC-062 End Process Tree
- Was: v1 PROC-052; r2 PROC-045
- Source: TS L29279, L28645-28766 "Process tree terminated", "End process tree requested." | R-037
- Behaviour:
  - Ends the process and all its descendants.
  - Always confirms: "End N processes in this tree?", buttons "End tree" and "Cancel".
  - Toast "End process tree requested.", then "Process tree terminated", with per-target results in one toast.
- Linux data/mechanism:
  - The subtree's keys are captured from the engine forest when the menu opens (max 256).
  - `ActionKind::KillTree`: the engine opens a verified pidfd for every target, sends SIGSTOP to all of them, then SIGKILL to all of them, then closes the fds. Stopping first means no process can fork between the passes. A target that fails `open_verified` is `Rejected(IdentityChanged)`.
- Verdict: ADAPT
- Phase: P1
- Privilege: as PROC-060. With mixed ownership the retry asks once for the helper.

### PROC-063 Suspend and Resume process
- Was: v1 PROC-053; r2 PROC-047
- Source: TS L29281-29285 | MC Suspend (STOP), Continue (CONT)
- Behaviour:
  - "Suspend Process" sends SIGSTOP; "Resume Process" sends SIGCONT. Only the item that applies is shown, chosen from State T.
  - A suspended row shows State "Stopped" and a pause glyph in the Name cell.
  - Multi-select: "Suspend Selected Tasks" / "Resume Selected Tasks".
  - App and unit rows use Freeze App (PROC-064) instead.
  - Toasts: "Process suspended.", "Process resumed.".
- Linux data/mechanism: `ActionKind::Suspend` / `Resume` (SIGSTOP and SIGCONT through the pidfd).
- Verdict: ADAPT (NtSuspendProcess becomes SIGSTOP)
- Phase: P1
- Privilege: as PROC-060

### PROC-064 Freeze App, Thaw App and Kill App
- Was: v1 PROC-054; v1 PROC-051 (app rows); r2 PROC-048
- Source: PLAN section 3 | ADR-006 "cgroup operations" | R-033, R-040
- Behaviour:
  - On app and unit rows: "Freeze App" / "Thaw App" (the row's State shows "Frozen"), and "Kill App (all processes)" with the confirmation "Kill all N processes of <App>?".
  - Toasts: "App frozen", "App thawed", "App killed".
- Linux data/mechanism:
  - Cgroups under `user@<uid>.service/app.slice/`: `ActionKind::FreezeGroup`, `ThawGroup`, `KillGroup` on a `CgroupRef`, validated by `tl-ops::cgroup::open_validated` (path prefix, `.scope` or `.service` suffix, inode equals the captured `cgroup_id`, owner is our UID, not tasklens's own cgroup). Freeze writes "1" to `cgroup.freeze` and polls `cgroup.events` for `frozen 1` every 50 ms for up to 2 s; thaw writes "0"; kill writes "1" to `cgroup.kill`.
  - Other user units (`session.slice`, `background.slice`): the user manager's `org.freedesktop.systemd1.Manager.FreezeUnit(name)`, `ThawUnit(name)` and `KillUnit(name, "all", SIGKILL)`.
  - System units: the same methods on the system manager, with `ALLOW_INTERACTIVE_AUTHORIZATION` so polkit `org.freedesktop.systemd1.manage-units` can prompt.
  - TaskSlinger's "Suspend App" / "Resume App" and "Force End App" labels are replaced (R-033, R-040).
- Verdict: ADAPT (new Linux capability)
- Phase: P2
- Privilege: own app cgroups need none. System units use the systemd polkit prompt. A denied prompt gives a toast.
- Dispatch by target type (R-092, ADR-006 "Units outside `app.slice`"): `ActionTarget::Cgroup` for cgroups under `app.slice`, `ActionTarget::Unit` for every other unit.

### PROC-065 Send Signal submenu
- Was: v1 PROC-056; r2 PROC-046
- Source: MC `table_view.blp:121` (8 signals), `PL:processes/manager.rs:29-110` | R-039
- Behaviour:
  - Top level: Terminate (TERM), Kill (KILL), Hangup (HUP), Interrupt (INT), Quit (QUIT), Stop (STOP), Continue (CONT), User 1 (USR1), User 2 (USR2).
  - "Other signals >" lists every other standard signal by name and number: ABRT, ALRM, BUS, CHLD, FPE, ILL, IO/POLL, PIPE, PROF, PWR, SEGV, STKFLT, SYS, TRAP, TSTP, TTIN, TTOU, URG, VTALRM, WINCH, XCPU, XFSZ.
  - "Real-time signal…" opens a spin box for SIGRTMIN+0 to SIGRTMAX (34 to 64).
  - Signals that terminate by default carry a trailing "(ends process)".
  - Toast: "Sent SIGHUP to firefox (1234)".
  - App rows send to the primary processes.
- Linux data/mechanism: `ActionKind::Signal(Signal)` with `Signal::Other(u8)` for any number outside the named nine; `pidfd_send_signal`. Names come from `libc` constants.
- Verdict: BUILD (MC, extended)
- Phase: P2
- Privilege: as PROC-060

### PROC-066 Set Priority
- Was: v1 PROC-057; r2 PROC-049
- Source: TS menu (Realtime, High, Above normal, Normal, Below normal, Low), L28455-28458, [IMG adv_sheet_02] | R-038
- Behaviour:
  - Submenu: Realtime, High (nice -10), Above normal (nice -5), Normal (nice 0), Below normal (nice 5), Low (nice 19), Idle (SCHED_IDLE), "Custom…". The current class has a check mark.
  - "Custom…" opens a dialog with a nice slider (-20..19) and a policy combo (OTHER, BATCH, IDLE, FIFO, RR) with an RT priority of 1-99 for FIFO and RR.
  - "Realtime" asks first: "Realtime priority can freeze the desktop if the process spins. Set it anyway?". It sets SCHED_RR priority 1 with `SCHED_RESET_ON_FORK`.
  - Toast "Priority updated".
  - Tooltip: "With autogroup on, nice only orders processes within the same session."
  - App rows apply to every current process of the app; tooltip "Applies to the app's current processes".
- Linux data/mechanism:
  - Nice classes: `ActionKind::SetNice(n)`, `setpriority(PRIO_PROCESS, tid, n)` for every TID in `/proc/<pid>/task`, re-listed right before the loop (ADR-006).
  - Realtime, Idle and Custom: `ActionKind::SetSched { policy, rt_prio }`, `sched_setattr` per TID.
  - Partial success is `Failed(Partial { done, total })`.
- Verdict: ADAPT (Windows priority classes become nice and policy)
- Phase: P1 (High to Low); P2 (Realtime, Idle, Custom)
- Privilege: raising the nice value of own processes needs none. Lowering it, Realtime, and other UIDs need CAP_SYS_NICE (RLIMIT_NICE is 0 by default): EPERM gives the "Permission denied" toast, with "Retry as administrator" from P3 (helper `SetNice` / `SetSched`).

### PROC-067 Set Affinity
- Was: v1 PROC-059; r2 PROC-050
- Source: TS menu "All processors" plus one entry per CPU with P/E icons, L28460, L28649, [CL 0.8.31], cross-check A.2 | R-039
- Behaviour:
  - "Set Affinity…" opens a dialog with an "All processors" check and a grid of checkboxes "CPU 0" to "CPU 31".
  - CPUs are grouped by L3 domain ("CCD 0: CPU 0-7, 16-23") with SMT siblings paired. Intel hybrid CPUs show P-core and E-core icons.
  - A list field accepts a CPU list such as "0-7,16-23" and stays in sync with the grid.
  - When nothing is checked, OK is disabled and the dialog shows "At least one CPU must stay enabled".
  - Toasts: "Affinity updated", "Could not update selected process affinity.".
  - Multi-target use with mixed marks is PROC-069.
- Linux data/mechanism:
  - `ActionKind::SetAffinity(CpuSet)`: `sched_setaffinity(tid, mask)` for every TID; the current mask from `sched_getaffinity`.
  - Topology from `/sys/devices/system/cpu/cpu*/cache/index3/shared_cpu_list` and `topology/thread_siblings_list`; hybrid from `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus` (absent on this AMD CPU).
  - Masks are limited to the cgroup's `cpuset.cpus.effective`.
- Verdict: ADAPT (a dialog instead of a 32-item submenu; CCD grouping in place of P/E on AMD)
- Phase: P1 (one target); P2 (multi-select mixed marks)
- Privilege: own processes need none. Other UIDs need CAP_SYS_NICE (helper `SetAffinity`).

### PROC-068 Set I/O Priority
- Was: v1 PROC-060; r2 PROC-051
- Source: MC inventory section 3 "Missing: I/O priority" | ADR-006
- Behaviour:
  - Submenu: Realtime, High (best-effort 0), Normal (best-effort 4), Low (best-effort 7), Idle, "Default (follows nice)". A check mark shows the current value.
  - Toast "I/O priority updated".
  - The tooltip names the active block scheduler, because the setting only takes effect with BFQ or mq-deadline.
  - Realtime is enabled only while a helper session is active; otherwise it is disabled with the tooltip "Needs administrator access." (or "Needs administrator helper (not installed)" when the helper is missing).
- Linux data/mechanism: `ActionKind::SetIoPriority(IoPriority)`: `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_PRIO_VALUE(class, level))` for every TID ("Default (follows nice)" is `IoPriority::None`, `IOPRIO_CLASS_NONE`, R-093) through `libc::syscall` in `tl-ops/src/sys.rs`. The scheduler comes from `/sys/block/<dev>/queue/scheduler`.
- Verdict: ADAPT (Linux-only)
- Phase: P2
- Privilege: best-effort and idle on own processes need none. Realtime needs CAP_SYS_ADMIN; other UIDs need the helper.

### PROC-069 Multi-select priority, affinity, I/O priority, copy and end
- Was: add PROC-097
- Source: TS toasts L28743-28754, cross-check A.2
- Behaviour:
  - **Set Priority** with 2 or more rows applies to every selected process (and every process of a selected app row). A class is checked only when all targets share it. Toasts: "Selected process priority updated." when all succeed; "Could not update selected process priority." when all fail; on partial failure, for example "Priority updated for 3 of 4 processes. 1 failed: permission denied", with "Retry as administrator" (P3) retrying only the failed targets.
  - **Set Affinity** applies to all targets. Each CPU is checked when every target has it and shows a mixed mark when only some do; clicking a mixed CPU sets it on all targets. Toasts "Selected process affinity updated." and "Could not update selected process affinity.", with the same partial form.
  - **Set I/O Priority** follows the same rules, with toasts "Selected process I/O priority updated." and "Could not update selected process I/O priority.".
  - **Copy**, **Copy details** and **Copy all metrics** with several rows: PROC-070.
  - **End Task** with several rows: confirmation first, then "End task requested for selected processes.".
- Linux data/mechanism: one `ActionRequest` carries the whole target list and returns per-target results. Syscalls run per target thread as in PROC-066 to PROC-068.
- Verdict: BUILD
- Phase: P2 (priority, affinity, I/O priority); P1 (copy and end)
- Privilege: as PROC-066 to PROC-068, per target

### PROC-070 Copy, Copy details, Copy all metrics and Copy rows as table
- Was: v1 PROC-058; r2 PROC-053; add PROC-098; add amend PROC-058
- Source: TS "Copy", "Copy all metrics" L35504-35513, "Process details copied." L28761, cross-check A.2, A.3 | R-012, R-005
- Behaviour:
  - **Copy** copies the right-clicked cell's displayed text. With several rows selected, it copies that column's value for each selected row, one per line in view order, with toast "Selected process values copied."; one row gives "Copied to clipboard".
  - **Copy details** (Ctrl+C) copies this fixed block per selected row, with a blank line between rows:
    ```
    Name: firefox
    PID: 4321
    Parent PID: 2010
    CPU: 3.27%
    Memory: 812 MiB
    Private bytes: 1490 MiB
    Working set: 1104 MiB
    Threads: 97
    Handles: 412
    Priority: Normal (0)
    ```
    - CPU is `%.2f%%` of the whole machine, the one exception to the one-decimal percent rule of table (d) (R-094). Memory is RssAnon, Private bytes is VmData + VmStk, Working set is VmRSS, all in whole MiB. Handles is the fd count. Priority is the PROC-029 class with the nice value in parentheses.
    - A Restricted value prints "restricted" and an unsupported one prints "n/a"; neither prints 0.
    - Toast "Process details copied.", or "Failed to copy to clipboard".
  - **Copy all metrics** copies, per selected row, one `Label: value` line for every column that has data, in picker order, ending with `Command line: …`. Rows are separated by a blank line. Toast "Copied metrics to clipboard", or "Selected process metrics copied." for several rows.
  - **Copy rows as table** (Ctrl+Shift+C) copies the selected rows as TSV of the visible columns with a header line. Toast "Copied to clipboard".
  - Command lines appear only in Copy all metrics (and Copy on the Command line cell), and only for rows the user selected (ADR-015 deliberate export). Environment values are never included.
- Linux data/mechanism: snapshot values; GPUI `cx.write_to_clipboard` through Wayland `wl_data_device` (U-33). Command lines are never logged.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-071 Process jump target, Go to parent and Go to service
- Was: add PROC-099; r2 PROC-061; v1 PROC-064; v1 PROC-065; add amend PROC-064; add amend PROC-065
- Source: TS L29292, L29320, toasts L28645-28766, L28696, L28700, L28709-28710, L28728, L28830, cross-check A.4 | SYS-010, SYS-033 (merged sys-app IDs) | R-047
- Behaviour:
  - The Processes tab has one "select process by identity" entry point. Go to parent process, the Properties PPID link, Services "Jump to process", Connections "Jump to process", Search File Handles "Go to process", the window picker, the History table, `--select-pid <pid>` and the D-Bus `ActivateAction("select-pid", <uint32>)` all use it.
  - On success: switch to Processes (toast "Switched to Processes tab" when the source was another tab, unless the caller has its own toast such as "Switched to owning process." or "Switched to service process."), expand the group or tree path, select the row and scroll it to the centre.
  - When the key is no longer in the snapshot (exited or PID reused): "Process not found in current view".
  - When the process exists but the filter or Hide system processes hides it: the caller's "not visible" toast with a "Show" button that clears the filter, turns Hide system processes off and then selects the row.
  - **Go to parent process**: selects the parent; toast "Switched to parent process"; when hidden, "Parent process not found in current view" with "Show".
  - **Go to service**: switches to the Services tab and selects the unit; toasts "Switched to Services tab" and "Switched to service."; "Service not found in current view" when the Services tab does not list it (SYS-010).
  - **Properties** failure (target exited between click and open, or `/proc/<pid>` cannot be opened): "Could not open properties.". The window-cap toast stays separate (PROC-077).
- Linux data/mechanism: a `ProcessKey` lookup in the current projection; a PID match alone is never enough. The unit and its manager come from the cgroup path (`system.slice` or `user@<uid>.service`).
- Verdict: BUILD
- Phase: P1 (Go to parent); P2 (the shared entry point for other callers, Go to service)
- Privilege: none

### PROC-072 Restart Plasma Shell and Restart unit
- Was: v1 PROC-066; r2 PROC-060
- Source: TS L29283, L29288, L35562-35572 (Explorer restart toasts)
- Behaviour:
  - On the `plasmashell` row: "Restart Plasma Shell". Toasts "Restarting Plasma Shell...", then "Plasma Shell restarted"; "Plasma Shell restart is already in progress." when pressed again before it finishes.
  - On any row that is a unit's MainPID: "Restart unit <name>".
  - No item for `kwin_wayland` or `gnome-shell` on Wayland, because restarting them ends the session. Hidden on GNOME Wayland.
- Linux data/mechanism: `ActionKind::UnitRestart` on `UnitRef`: `RestartUnit("plasma-plasmashell.service", "replace")` on the user manager, tracked to `JobRemoved` with result "done" (30 s deadline, ADR-006). MainPID comes from the unit property `MainPID`.
- Verdict: ADAPT
- Phase: P2
- Privilege: user units need none. System units use the polkit `org.freedesktop.systemd1.manage-units` prompt.

### PROC-073 Open file location and File properties
- Was: v1 PROC-067; v1 PROC-068; r2 PROC-054
- Source: TS L35502, L29316, toast "Opening file location"
- Behaviour:
  - "Open file location" opens the file manager with the executable selected. Toast "Opening file location".
  - "File properties" opens the file manager's properties dialog for the executable.
  - Both are disabled with the tooltip "Executable deleted" when the exe link ends " (deleted)". When the exe is Restricted they show the toast "Process image unavailable".
- Linux data/mechanism: session D-Bus `org.freedesktop.FileManager1.ShowItems(["file://<exe>"], "")` and `ShowItemProperties([...], "")` (Dolphin implements both). If no service owns `org.freedesktop.FileManager1`, Open file location falls back to `xdg-open` on the parent directory, and File properties is hidden.
- Verdict: ADAPT
- Phase: P1 (Open file location); P2 (File properties)
- Privilege: none

### PROC-074 Open in VirusTotal
- Was: v1 PROC-069; r2 PROC-055
- Source: TS L35503 (`https://www.virustotal.com/gui/file/<sha256>`), toasts "SHA-256 unavailable", "Process image unavailable"
- Behaviour:
  - Computes the executable's SHA-256 in the background (toast "Computing SHA-256…" when it takes longer than 500 ms), then opens `https://www.virustotal.com/gui/file/<sha256>` in the default browser. Nothing is uploaded.
  - The first use shows a one-time notice: "This opens virustotal.com with the file's hash. The file is not uploaded." Accepting it sets `state.toml` `[processes] virustotal_notice_accepted = true` (R-095).
  - Errors: "SHA-256 unavailable", "Process image unavailable".
- Linux data/mechanism: streamed hash of `/proc/<pid>/exe` (works for deleted files) on the blocking pool, cached by (dev, inode, mtime). Opened with the portal `org.freedesktop.portal.OpenURI.OpenURI`, falling back to `xdg-open`.
- Verdict: BUILD
- Phase: P2
- Privilege: other users' exe gives EACCES; the fallback hashes the absolute argv path when that file is readable, else "Process image unavailable".

### PROC-075 Create dump file
- Was: v1 PROC-070; r2 PROC-056
- Source: TS menu, [IMG adv_sheet_02] `<name>_<pid>_<yyyymmdd_hhmmss>.dmp`, "Dump created: %ls", INI `Crash.DumpType` Mini/Full, L34881-34884 | R-004
- Behaviour:
  - Writes `$XDG_STATE_HOME/tasklens/dumps/<name>_<pid>_<yyyymmdd_hhmmss>.core`.
  - The process pauses while the core is written. Before a dump of a process with VmRSS over 2 GiB, the confirmation says so. A progress toast "Writing core… 1.2 GiB" shows during the write.
  - Toast "Dump created: <path>" with an "Open folder" button.
  - Settings > Processes > "Dump type": Mini (default) or Full (`processes.dump_type = "mini"`).
  - Without `gdb` installed the item is disabled with the tooltip "Install gdb to create dumps".
  - Without administrator access: toast "Creating a dump needs administrator rights".
- Linux data/mechanism:
  - `gcore -o <path-stem> <pid>` from `/usr/bin/gcore`.
  - Mini uses the process's own `/proc/<pid>/coredump_filter`. Full writes `0x1ff` to it for the duration and restores it afterwards.
  - With `ptrace_scope = 1` (this machine) a non-descendant needs CAP_SYS_PTRACE, so the helper runs `Dump { key, mode, out_fd }`: gcore by fixed path, writing to a file descriptor that tasklens opened, so the file belongs to the user.
  - With `ptrace_scope = 0` and the same UID, tasklens runs gcore directly.
- Verdict: ADAPT (MiniDump becomes a gcore core file)
- Phase: P3
- Privilege: CAP_SYS_PTRACE through the helper.

### PROC-076 Privileged helper for process operations
- Was: v1 PROC-003; r2 PROC-064
- Source: TS shield "relaunch elevated" L30127 | MC `PL:processes/manager.rs:29-110` (pkexec kill) | APP-003 | ADR-006 | R-001 to R-004
- Behaviour:
  - The first privileged request (the shield, "Retry as administrator", "Grant access", "Search all" or the status-bar restricted count) shows one polkit prompt. tasklens never relaunches the GUI as root.
  - While a helper session is active, the shield shows `shield-hi` (APP-003) and the status bar shows a badge "Administrator mode"; clicking either ends the session.
  - Restricted cells fill in: Handles, I/O, Disk, Image path, Network attribution, PSS, USS, Working directory, Memory maps and Environment. Helper reads of restricted fields run at 1 Hz, only while those columns or windows are shown.
  - Without the helper installed, privileged items are disabled with the tooltip "Needs administrator helper (not installed)".
- Linux data/mechanism:
  - `pkexec /usr/lib/tasklens/tl-helper --session`, polkit `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`), policy file `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy`.
  - Transport: stdin and stdout with `tl-model::wire` framing, 64 KiB frames, multi-frame replies capped at 4 MiB per request. Idle exit after 300 s or on stdin EOF. `PR_SET_PDEATHSIG = SIGKILL`. No `--once` mode.
  - Process operations used by this area: `Signal`, `SetNice`, `SetSched`, `SetAffinity`, `SetIoPriority`, `FreezeGroup`, `ThawGroup`, `KillGroup`, `ReadProcessDetail`, `ReadProcessFiles { key, files ⊆ {io, fd_list, fdinfo, environ, maps, smaps_rollup, ns, cwd, exe, status} }`, `ScanStrings { key, opts }`, `Dump { key, mode, out_fd }`, `ResolveSocketInodes { inodes }`.
  - No path, command or free-form file argument. The helper re-runs `open_verified` on every process target.
  - The eBPF collector `tl-netmon` is a separate binary with its own polkit action (P4).
- Verdict: ADAPT (a pkexec helper instead of an elevated GUI)
- Phase: P3
- Privilege: root through pkexec. Fallback: Restricted states and "Retry as administrator" buttons.

---

## H. Process Properties and App Properties windows

### PROC-077 Process Properties window
- Was: v1 PROC-071; r2 PROC-065
- Source: TS 3.1, L28990, L30218-30237, L28713, L29889-29894, [CL 0.8.4], cross-check A.38 | APP-045 | R-050, R-073
- Behaviour:
  - A separate, non-modal top-level window titled "Process Properties - firefox - PID 1234", default 900 × 640 logical px, resizable, with its own title bar, pin (PROC-056) and close.
  - Tabs: "General", "Threads [n]", "File descriptors [n]", "Memory maps [n]", "Security", "Environment [n]", "Strings".
  - Each tab has its own filter field with a clear button. Empty state: "No properties match the current filter.".
  - Contents refresh at `processes.interval_ms`.
  - Up to 8 windows can be open. A ninth gives "Too many process properties windows are already open". Opening Properties again for the same key focuses the existing window.
  - When the process exits, the title appends " (not running)", values freeze and actions are disabled.
  - Esc closes the window.
  - Failure to open: "Could not open properties." (PROC-071).
  - Key/value fields use the PERF-079 words for missing values ("Restricted", "Not supported", "Unavailable" and the rest), never a zero (R-050).
- Linux data/mechanism: GPUI `cx.open_window` (`WindowKind::Floating`, APP-045) with its own gpui-kit Root. The window registers demand for its `ProcessKey` and the active tab's data.
- Verdict: ADAPT
- Phase: P2
- Privilege: per tab

### PROC-078 General tab
- Was: v1 PROC-072; r2 PROC-066; add amend PROC-072
- Source: TS 3.1 General, L29038-29081, L29094-29103, L29127, L29147, L29175-29198, L35680-35681, cross-check A.21, A.22, A.26 | MC details dialog fields
- Behaviour: three key/value columns. Clicking a value copies it with toast "Process property value copied.".
  - **Column 1:**
    - Process, PID, Started (`YYYY-MM-DD HH:MM:SS` local), PPID (a link to the parent, PROC-071), User (name and UID, plus euid and suid when they differ).
    - Priority: "Nice 0, SCHED_OTHER, kernel prio 120", plus the autogroup "autogroup-N nice 0" from `/proc/<pid>/autogroup`.
    - Affinity: "All (0-31)", or the list with a count such as "0-7,16-23 (16 CPUs)", from the leader thread's `Cpus_allowed_list`; " (threads differ)" is appended when any thread differs. Clicking it opens Set Affinity (PROC-067).
    - Hardening: PIE, NX stack, RELRO (full, partial or none), Shadow stack (status `x86_Thread_features`), ASLR (system setting plus the `ADDR_NO_RANDOMIZE` personality), NoNewPrivs, Seccomp.
    - Architecture; Executable (a link that opens the file location, PROC-073); Location category; Package; Description; Cmd line (wrapped, full 32 KiB, with a copy button); Working dir; Root (only when it is not "/"); Cgroup; Unit (a link to the Services tab); App ID.
    - Location category values: Temp directory (`/tmp`, `/var/tmp`, `$XDG_RUNTIME_DIR`), User Downloads (`xdg-user-dir DOWNLOAD`), User data (`~/.local`, `~/.cache`, `~/.config`), User profile (`~`), System (`/usr`), Optional software (`/opt`), Flatpak (`/var/lib/flatpak`, `~/.local/share/flatpak`), AppImage (`/tmp/.mount_*`), Wine prefix (`*/drive_c/*`), Deleted.
  - **Column 2:**
    - SHA-256 and MD5, computed in the background when the window opens. Errors "Failed to compute one or more hashes", "Executable path unavailable".
    - Package status with a "Verify against package" button that compares the file's sha256 with the package mtree; Packager.
    - Threads, Handles.
    - Memory (RssAnon), Working set total (VmRSS), Working set shared, PSS, USS, Swap, Commit (VmData + VmStk), Memory % of MemTotal.
    - CPU %, CPU time.
    - I/O read bytes (rchar), I/O write bytes (wchar), I/O read ops, I/O write ops.
  - **Column 3:**
    - Disk read bytes, Disk write bytes, Cancelled write bytes, Disk read rate, Disk write rate, Disk total rate.
    - Net I/O (attributed TCP send and receive).
    - GPU %, VRAM, GPU memory shared, GPU engine.
    - Window block (P2 with the bridge): Window title, Window ID (KWin `internalId`, replacing HWND), Window state (Minimized, Maximized or Normal), Window class (`resourceClass`), Window rect ("L:%d T:%d R:%d B:%d"), Window size ("%d x %d"), "And N more" when there are several windows, "No top-level window" when there is none.
- Linux data/mechanism:
  - Hardening from the ELF of `/proc/<pid>/exe`: `e_type ET_DYN` plus `PT_INTERP` for PIE; `PT_GNU_STACK` flags; `PT_GNU_RELRO` plus `DT_FLAGS BIND_NOW`. System ASLR from `/proc/sys/kernel/randomize_va_space`, personality from `/proc/<pid>/personality`.
  - `readlink` of `cwd` and `root`. Everything else uses the PROC-027 to PROC-036 sources.
  - Windows CFG, Dyn code and Strict handle mitigations have no Linux equivalent and are not shown; DEP and ASLR map to NX stack and PIE.
- Verdict: ADAPT (Windows mitigations become ELF hardening; HWND becomes the KWin ID)
- Phase: P2
- Privilege: exe, cwd, root, personality, io and smaps_rollup of other UIDs show "Restricted" until the helper reads them.
- SHA-256 and MD5 (here and in PROC-074) use the `sha2` and `md-5` crates in `tl-collect` (R-096, ADR-001).

### PROC-079 Threads tab
- Was: v1 PROC-073; r2 PROC-067; add amend PROC-073
- Source: TS 3.1 Threads, L29021-29025, L35489, L35667-35668, cross-check A.24
- Behaviour:
  - Columns, all sortable: Thread ID, Name (thread `comm`), State, CPU %, User time (ms), Kernel time (ms), Priority, Base priority, Affinity, Last CPU, Ctx switches (voluntary / involuntary), Wait channel.
    - Priority: "Variable" for SCHED_OTHER, SCHED_BATCH and SCHED_IDLE (the scheduler adjusts them at run time, like TaskSlinger's "Variable"); "Realtime N" for SCHED_FIFO and SCHED_RR with N = rt_priority (stat field 40); "Deadline" for SCHED_DEADLINE. The tooltip names the policy.
    - Base priority: nice (stat field 19) for normal policies, rt_priority for realtime threads.
  - States: "Loading threads...", "No threads found.", "No threads match the current filter.".
  - Row menu: Copy, "Set thread priority >" (as PROC-066 for one TID), "Set thread affinity…" (as PROC-067 for one TID), "Send signal to thread >" (TERM, KILL, USR1, USR2 through `tgkill`). STOP is not offered because it stops the whole thread group. The tooltip warns that a fatal signal ends the whole process.
  - TaskSlinger's Start Address and Start Module are not collected (they need ptrace plus symbol resolution); Name and Wait channel take their place.
- Linux data/mechanism: `/proc/<pid>/task/<tid>/stat` (comm, state, utime, stime, nice, policy, processor field 39), `status` (`Cpus_allowed_list`, context switches), `wchan` and `comm`. Thread actions use `ActionTarget::Thread { process: ProcessKey, tid: u32 }` (R-097, ADR-005): `setpriority`, `sched_setaffinity` and `tgkill(pid, tid, sig)` after the pidfd identity check on the process and a check that `/proc/<pid>/task/<tid>` exists.
- Verdict: ADAPT
- Phase: P2
- Privilege: `wchan` of other UIDs shows "Restricted". Thread actions on other UIDs need the helper, which accepts the thread target for `Signal`, `SetNice`, `SetSched` and `SetAffinity` (R-097).

### PROC-080 File descriptors tab
- Was: v1 PROC-074; r2 PROC-068
- Source: TS 3.1 Handles, L29026-29027, L29154-29174, L28988, L35666, INI `UI.HideUnnamedHandles=1`, cross-check A.25
- Behaviour:
  - Columns:
    - FD.
    - Type: file, directory, TCP, UDP, unix, netlink, pipe, FIFO, eventfd, eventpoll, timerfd, signalfd, inotify, fanotify, memfd, dmabuf, DRM, pidfd, io_uring, anon_inode (other), device.
    - Name: the link target. Sockets resolve to "TCP 127.0.0.1:5000 → 10.0.0.2:443 ESTABLISHED" or the Unix path; pipes to "pipe:[ino] ↔ cat (5678)" by matching the pipe inode across processes; DRM fds to "card1 (NVIDIA)"; nameless entries show "(unnamed)".
    - Access: "R", "W" or "RW", with "+A" for append, "+NB" for non-blocking and "CLOEXEC".
    - Position: fdinfo `pos`.
  - A details pane for the selected fd shows "Raw flags: 0o2104002", decoded into access mode, status flags (Append, Non-blocking, Sync, Direct, Path, Temp file) and descriptor flags (Close-on-exec), plus the mount ID and inode.
  - Checkbox "Hide unnamed handles", default on (`properties.hide_unnamed_handles = true`), hides `anon_inode:*` entries.
  - States: "Loading handles...", "No handles reported.", "Handle detail query unavailable.". For other UIDs without the helper: "Handles unavailable (insufficient privileges)" with a "Grant access" button.
  - Row menu: Copy, Open file location (files), Go to connection (sockets; switches to the Connections tab).
  - Closing another process's fd is not offered (it needs ptrace code injection).
- Linux data/mechanism: `readlinkat` on `/proc/<pid>/fd/*`; `/proc/<pid>/fdinfo/<fd>` (`pos`, `flags`, `mnt_id`, `eventfd-count`, `drm-*`); the shared sock_diag inode map.
- Verdict: ADAPT (Windows handles become fds)
- Phase: P2
- Privilege: ptrace read access, which Yama allows for the same UID. Other UIDs need the helper (`ReadProcessFiles { fd_list, fdinfo }`).

### PROC-081 Memory maps tab
- Was: v1 PROC-075; r2 PROC-069
- Source: TS 3.1 Modules, L29028, L29112, L29215-29216, L29020, [CL 0.8.4], INI `UI.ModulesDllOnly=1`
- Behaviour:
  - A tree with one node per backing file and its regions as children (start-end, perms, offset, RSS).
  - "(Primary)" marks the main executable, which comes first. Anonymous regions (`[heap]`, `[stack]`, `[vdso]`, `[vvar]`, anonymous, memfd) group under "Outside loaded modules".
  - Columns: Name, Base address (lowest start), Size (sum of region sizes), RSS (sum from `smaps`), Perms (for example "r-xp"), Offset, Path, Description (package `%DESC%`), Package.
  - Checkbox "Shared libraries only", default on (`properties.shared_objects_only = true`), keeps ELF shared objects (`*.so`, `*.so.*`) and, for Wine processes, `*.dll`, `*.exe`, `*.drv` and `*.sys`.
- Linux data/mechanism: `/proc/<pid>/maps`; per-mapping RSS from `/proc/<pid>/smaps`, loaded while the tab is open, at most 1 Hz; the package index from PROC-027.
- Verdict: ADAPT ("DLL only" becomes "Shared libraries only"; Wine DLLs work literally)
- Phase: P2
- Privilege: ptrace read, as PROC-080. Other UIDs need the helper (`ReadProcessFiles { maps, smaps }`, R-098); otherwise the tab shows "Needs administrator access.".

### PROC-082 Security tab
- Was: v1 PROC-076; r2 PROC-070
- Source: TS 3.1 Token, L29006-29013, L29082-29093, L29104-29119, L30247-30257, L29152, cross-check A.23
- Behaviour:
  - Summary rows:
    - User: real, effective, saved and fs UID with names; GIDs the same way.
    - Groups: count.
    - Risky capabilities: CAP_SYS_ADMIN, CAP_SYS_PTRACE, CAP_DAC_OVERRIDE, CAP_DAC_READ_SEARCH, CAP_SYS_MODULE, CAP_SYS_RAWIO, CAP_SETUID, CAP_SETGID, CAP_NET_ADMIN, CAP_BPF, CAP_PERFMON, CAP_SYS_BOOT, CAP_CHOWN, CAP_FOWNER, when effective or ambient.
    - Admin groups: wheel, sudo, root, adm, docker, libvirt, lxd, disk; docker, libvirt, lxd and disk are marked "root-equivalent".
    - Flags: NoNewPrivs; Seccomp (disabled, strict or filter, plus the `Seccomp_filters` count); Dumpable.
    - Security label (LSM).
    - Session: logind session, audit `sessionid`, `loginuid`.
    - Elevated: "Yes (root)", "Yes (capabilities)", "Setuid", "No".
    - Namespaces: user, pid, net, mnt, ipc, uts, cgroup, time; each "Shared with init" or "Isolated (inode)". When PID 1's namespaces cannot be read (no helper), the comparison uses `/proc/self/ns/*` and the label reads "Same as tasklens".
    - Speculation: Speculation_Store_Bypass, SpeculationIndirectBranch.
  - Table "Capabilities [n]": Name, Effective, Permitted, Inheritable, Bounding, Ambient (check marks), Description.
  - Table "Groups [n]": Name, GID, Kind ("Primary" or "Supplementary").
  - "None reported" when a table is empty. Error "Security information unavailable (insufficient privileges)". Copy failure toast "Could not copy token summary value.".
  - Windows integrity level, impersonation level, LUIDs and UI access have no Linux meaning and are not shown.
- Linux data/mechanism: `/proc/<pid>/status` (Uid, Gid, Groups, CapInh, CapPrm, CapEff, CapBnd, CapAmb, NoNewPrivs, Seccomp, Seccomp_filters, Speculation*), `loginuid`, `sessionid`, `attr/current`, `readlink ns/*`. Capability names and descriptions come from a static table of `linux/capability.h`.
- Verdict: ADAPT (Windows token becomes credentials, capabilities and namespaces)
- Phase: P2
- Privilege: `status` is world-readable. `ns/*` of other UIDs shows "Restricted" until the helper reads it (`ReadProcessFiles { ns }`).

### PROC-083 Environment tab
- Was: v1 PROC-077; r2 PROC-071
- Source: TS 3.1 Environment, [IMG adv_sheet_01], L29148-29151 | ADR-015
- Behaviour:
  - Columns: Name, Value. Both sortable.
  - A header note "Environment at process start" with a Refresh button.
  - Values whose names match `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*PASSWD*`, `*KEY*`, `*COOKIE*` or `*CREDENTIAL*` (case-insensitive) are masked as "••••••" while `properties.env_mask = true` (default) (R-099). A "Reveal" toggle shows one row's value.
  - Copy name, copy value, and copy all as `NAME=value` lines. A masked value is copied as "••••••" unless its row is revealed (R-099).
  - Errors: "Environment unavailable (insufficient privileges)", "(read failed)", "Environment block unavailable" (zombie or kernel thread).
- Linux data/mechanism: `/proc/<pid>/environ`, NUL-separated, capped at 1 MiB, read when the tab opens and on Refresh. Never logged.
- Verdict: ADAPT (Linux shows only the initial environment, as the note says; masking added)
- Phase: P2
- Privilege: PTRACE_MODE_READ, so the same UID works. Other UIDs need the helper (`ReadProcessFiles { environ }`).

### PROC-084 Strings tab
- Was: v1 PROC-078; r2 PROC-072
- Source: TS 3.1 Strings, L29005, L29014, L35669-35671, L29846-29850, cross-check A.27 | R-004
- Behaviour:
  - A "Scan" button starts the scan; it never starts by itself. A "Cancel" button stops it.
  - Options: minimum length (4 to 64, default 4); encodings ASCII, UTF-8 and UTF-16LE (all on; UTF-16LE matters for Wine); "Include file-backed mappings" (off by default).
  - Progress: "Scanning process memory... 42%  18234 strings found".
  - Columns: Address (hex), Encoding, Length, Value. The tab filter applies to Value.
  - The scan stops at 1,000,000 strings or 256 MiB of text with "String scan stopped: not enough memory.".
  - Errors: "Failed to read process memory.", "Strings unavailable for this process." (kernel threads).
  - Without access: "Strings unavailable: needs administrator rights" with an "Enable" button that starts the helper.
- Linux data/mechanism:
  - Regions from `/proc/<pid>/maps`: readable, private anonymous plus heap and stack (file-backed only with the option), skipping `[vvar]`, `[vsyscall]` and device mappings.
  - Reads with `process_vm_readv` in 1 MiB chunks.
  - With `ptrace_scope = 1` this needs CAP_SYS_PTRACE even for own non-child processes, so the helper's `ScanStrings { key, opts }` streams result batches. With `ptrace_scope = 0` and the same UID, tasklens scans in-process.
- Verdict: ADAPT
- Phase: P3
- Privilege: CAP_SYS_PTRACE through the helper.

### PROC-085 App Properties window
- Was: v1 PROC-079; r2 PROC-073
- Source: MC `process_details_dialog.rs`, `process_details_dialog.blp:24-163`, Ctrl+I
- Behaviour:
  - Properties (Enter, Alt+Enter or Ctrl+I) on an app or unit row opens "App Properties - Firefox", a window like PROC-077.
  - **General** tab: 48 px icon and name, App ID, Type ("App" or "Unit"), Desktop file path, Unit, Cgroup, Process count, the primary process's command line, and live usage rows for CPU, Memory (cgroup `anon`), Swap (`memory.swap.current`), Disk, Network, GPU and VRAM. Swap updates live (MC's binding bug is not copied).
  - **Processes [n]** tab: a mini table of member processes. Double-click or "Open properties" on a row opens that process's Properties.
  - On process rows, MC's details dialog fields are covered by PROC-078.
- Linux data/mechanism: cgroup files (`memory.stat`, `memory.swap.current`, `io.stat`, `cgroup.procs`), the `.desktop` index and the snapshot.
- Verdict: ADAPT (the MC dialog merges into the TaskSlinger Properties window)
- Phase: P2
- Privilege: none

---

## I. History mode

### PROC-086 History recording
- Was: v1 PROC-080; r2 PROC-074
- Source: TS 4.1, INI `UI.HistoryStorageDurationHours=6`, L28515-28516, [CL 0.8.9] | ADR-008, ADR-012 | D-003, D-010, R-005, R-025
- Behaviour:
  - Settings > Monitoring > "Record process history": one switch, default off (`history.enabled = false`, D-003).
  - "History storage: N hours", menu 1, 2, 3, 4, 6, 8, 12; default 6; range 1..=12 (`history.retention_h`). The disk cap `history.max_disk_mb = 256` is not shown in menus.
  - While recording is on, collection continues with the window hidden, at the ADR-008 "recording on" rates (process sweep at `processes.interval_ms`, system series at 1 Hz).
  - While recording is off, tasklens keeps a rolling 10-minute window of process history in RAM, only while the Processes tab is open, and History mode shows that window (D-010). Opening History mode with recording off also shows the toast "History recording is off" with an "Enable" button. Turning recording on switches to the ADR-012 store.
  - The UI always shows the real retained time span.
- Linux data/mechanism:
  - Crate `tl-history`; the reducer's `RecordSink` receives every process sweep, system sample and device catalog (ADR-012).
  - Storage format per ADR-012: 60 s blocks holding new metadata (comm, display name, exe basename, app ID; never command lines), a keyframe of `HistProcess { key, meta, ppid, cpu_permille, mem_anon_kib, io_r_kibps, io_w_kibps, net_kibps, gpu_pct }` and per-sweep deltas. `mem_anon_kib` holds RssAnon (R-005). `FieldPatch` stores only changes beyond a threshold (CPU ≥ 1 permille, memory ≥ 256 KiB, rates ≥ 1 KiB/s or to or from zero). System series are stored at 1 Hz (10 Hz series reduced to the per-second maximum).
  - Gaps are recorded for suspend and resume and for engine epoch changes.
  - RAM budget 16 MB (open block plus segment indexes).
  - The rolling window (recording off) uses the same block encoding held in RAM, keeps the last 10 blocks, writes nothing to disk, and is dropped when the Processes tab closes (D-010, R-103).
  - Budget estimates are unverified: U-11, U-22.
  - The v1 and r2 storage designs (64 MiB RAM cap, uncompressed 10-minute tail, per-record layout) are Dropped: R-025.
- Verdict: ADAPT (TaskSlinger records by default; tasklens does not)
- Phase: P3
- Privilege: none

### PROC-087 History persistence
- Was: v1 PROC-081; r2 PROC-078
- Source: TS INI `HistoryPersistToDisk=1`, `store.tshp` (XPRESS), L34862, L30348, L35751-35760, L30356-30362, cross-check A.30 | ADR-012 | R-025
- Behaviour:
  - Settings > Monitoring > "Save history to disk" (`history.persist = true`), available only while recording is on.
  - When off, the recorder deletes its segments at shutdown and loads nothing at start. The on-disk format does not change.
  - On launch, history inside the retention window loads in the background, with a 2 s budget.
  - Load problems show once as toasts: "Could not open process history store.", "Process history store is empty or truncated.", "Process history store version/layout mismatch; starting fresh.", "No in-range process history in store.", "Process history store is stale.", "Process history load timed out; starting fresh.".
  - Identities from a previous boot are marked as such.
- Linux data/mechanism:
  - `$XDG_STATE_HOME/tasklens/history/<boot_id as 16 hex digits>/seg-<boottime seconds, 10 digits>.tlh`, one segment per 10 minutes (ADR-012).
  - Segment header (64 bytes): magic `TLH\x01`, `format_version`, `boot_id`, `realtime_offset_ns`, `start_t_ns`. Blocks are `u32 length | u32 crc32 | zstd level 3 frame`. A footer with the block index is written when a segment closes; a segment without a footer (crash) is read up to the first bad CRC.
  - The recorder deletes segments older than retention and, when over `max_disk_mb`, the oldest first. At start, directories of other boots older than retention are deleted.
  - Directories are mode 0700, files 0600.
  - r2's `YYYYMMDD-HHMM.tlh` names and `.pending` rename are Dropped: R-025.
- Verdict: ADAPT (XPRESS becomes zstd)
- Phase: P3
- Privilege: none

### PROC-088 History view and table
- Was: v1 PROC-082; r2 PROC-075
- Source: TS 4.1, INI `UI.ProcessesHistoryMode`, `HistoryVisibilityMask=1FF`, `Sort.HistoryKey=6`, `chrome.tab.processes.history`, L35690-35692, L35741-35742, [CL 0.8.27], cross-check C.2 | R-011, R-023
- Behaviour:
  - The history icon on the Processes tab toggles History mode. While it is on, the tab label reads "History". The state persists in `state.toml` `[processes] history_mode`.
  - Layout: header text, then the graph with the scrubber (PROC-089), then the table.
  - Header text: "History - Drag scrubber below to view timeline"; at a scrub point "History Snapshot: 213 processes - 4 min 12 sec ago"; "Live" at the right edge.
  - Table columns (9): Name, PID, User, Image path, CPU, Memory, Disk, Network, GPU. Memory is RssAnon. At a scrub point it shows that second's values; with a range selected it shows averages over the range with the peak in the tooltip.
  - Default sort CPU descending, persisted in `[sort.history]`; columns persist in `[columns.history]`.
  - Selecting a row draws that process's series in the graph; a clear chip (`history.process.clear`) removes it. A selected identity that is absent at the scrub time shows "Process not running".
  - Row menu "Delete from history" gives the toast "History entry deleted".
  - The global filter and Hide system processes apply to the table and the graph. The view-mode button is disabled (PROC-016).
  - Metric chips or legend toggles choose CPU, Memory, Disk, Network or GPU.
- Linux data/mechanism:
  - The UI sends `Command::HistoryQuery { id, range, what }`; the engine replies with a `HistoryView` snapshot component (max 1,024 points per series, min/max decimation as in ADR-009). Process state at time T is rebuilt from the block keyframe plus deltas up to T.
  - "Delete from history" adds the key to a per-boot list of deleted keys stored beside the segments, which every query filters out; segments are not rewritten (R-103).
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-089 History timeline scrubber
- Was: v1 PROC-083; r2 PROC-076
- Source: TS 4.1, `history.scrubber`, [CL 0.8.18], L35725-35731 | RECONCILIATION table (b)
- Behaviour:
  - A timeline strip with a stacked metric graph, and a minimap of the full retention below it with a viewport box.
  - Drag the handle to scrub. The mouse wheel zooms around the cursor. Shift+wheel or a middle-drag pans. Shift+drag selects a range. Double-click or the "Live" button returns to live.
  - Time labels: "-1:02:03", "1h 02m", "2m 05s", "45s".
  - Keyboard: Left and Right step 1 s; Shift+Left and Shift+Right step 1 min.
- Linux data/mechanism: the app chart element (ADR-009) drawn from the decimated `HistoryView`; level of detail is min/max per pixel column.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-090 History top-3 hover
- Was: v1 PROC-084; r2 PROC-077
- Source: TS 4.1 [CL 0.8.19], L35743-35749, cross-check A.30 | R-063
- Behaviour: hovering the timeline shows the time and the top 3 processes for the active metric at that second, formatted "CPU: 12.3%", "Memory: 1.22 / 62.70 GiB", "Disk: 4.2 MiB/s", "Network: 12.0 Mbps" (in the chosen network unit), "GPU: 41.0%".
- Linux data/mechanism: an engine query that rebuilds the sweep at that second from the ADR-012 store and ranks it, with a 50 ms budget. The block format has no stored top-K index.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-091 History first-snapshot state
- Was: v1 PROC-085
- Source: TS L28654-28655 "Collecting first history snapshot..."
- Behaviour: the header shows "Collecting first history snapshot..." until the first frame exists, in the recording store or the rolling window.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-092 History scrub benchmark
- Was: v1 PROC-086
- Source: TS L35427-35428 `--history-scrub-bench` | ADR-014 | RECONCILIATION table (c)
- Behaviour: `tasklens --bench history-scrub --duration <s> --warmup <s> --json <out>` replays a synthetic 6 h store and reports p50, p95 and p99 scrub frame times. It is one scene of the ADR-014 `--bench` family and exists only in builds with the `bench` feature.
- Linux data/mechanism: synthetic data from `tl-ui/src/bench/` with PRNG seed 42.
- Verdict: BUILD
- Phase: P3
- Privilege: none

---

## J. Process graphs and tray graphs

### PROC-093 Process minigraphs from the row menu
- Was: v1 PROC-087; v1 PROC-088; v1 PROC-089; r2 PROC-062
- Source: TS 4.2, L29304, L28719, INI `ProcessSpecificMiniGraphs.*`, `NameFilter`, `AppName`, `ProcessGraphColumnMask=720C0`, keys L35545-35557, cross-check A.28 | PERF-077, PERF-086, PERF-087 | R-066
- Behaviour:
  - "Add Minigraph >", "Add Process Name Graph >" and "Add App Name Graph >" each list CPU, Memory, Disk, Network, GPU and VRAM (the fixed six graphable columns).
  - **Add Minigraph** binds a card to the process key. Title `<name> (<pid>) · CPU`; `Process <pid>` only until the name is read.
  - **Add Process Name Graph** sums all processes whose image name matches exactly and follows restarts. Title `<name> by name · CPU`.
  - **Add App Name Graph** sums all instances of the app ID (or unit name for unit rows). Title `<App> by app · CPU`.
  - A process card whose process exited shows "Ended", keeps its data and stops. It is never rebound to a new process, also not after a tasklens restart or reboot: a saved card from another boot shows "Ended" (R-100). A name or app card with no live match shows "No match" and records a gap.
  - Toasts: "Minigraph added", "Minigraph already exists", "No space for more minigraphs", "Process name minigraph added", "App name minigraph added", "Unable to add minigraph".
  - The Memory graph is labelled "Memory" and shows RssAnon (R-005).
  - The card's layout, colours, range and detaching belong to the minigraph area (PERF).
- Linux data/mechanism:
  - Cards persist in `state.toml` `[[minigraphs]]` with `source = "process:<boot_id 16 hex>:<pid>:<start_ticks>" | "name:<image name>" | "app:<app id>"` and `column` (R-100).
  - The engine adds the key, name or app to demand, so the process sweep keeps running for it while the window is hidden.
  - App graphs use cgroup aggregates (`cpu.stat` `usage_usec`, `memory.stat` `anon`, `io.stat`) when the app is one cgroup, else Σ over members.
- Verdict: BUILD (app name becomes app ID or unit)
- Phase: P3, for the menu items and the process cards (R-101)
- Privilege: none

### PROC-094 Process tray icon graphs
- Was: v1 PROC-090; r2 PROC-063
- Source: TS 4.3, L29305-29306, L30441-30446, L28715-28723, L28584-28586, INI `TrayIconGraphs Graph0Mode` | PERF-078, PERF-079 | R-027, R-067
- Behaviour:
  - Row menu "Add tray icon graph >" offers the same six metrics. "Remove tray icon graph" appears instead when one exists for this source.
  - Modes: process, name filter, app.
  - Tray tooltips: process graph "<name> (PID n): CPU 3.2%"; name filter "3 procs: firefox"; app "<App>: CPU 3.2%".
  - Toasts: "Tray icon graph added", "Tray icon graph removed", "Tray icon graph already exists", "Unable to add tray icon graph", "Unable to remove tray icon graph".
  - The tray icon, its rendering and its menu belong to the tray area (PERF-078, R-027).
- Linux data/mechanism: the same process series as PROC-093; `tl-tray` with `ksni =0.3.6` draws a StatusNotifierItem at 22 and 44 px, updated at most 1 Hz and only when the pixels change. Persisted in `state.toml` `[[tray_graphs]]`.
- Verdict: ADAPT (Shell_NotifyIcon becomes SNI)
- Phase: P3
- Privilege: none

---

## K. Run dialog, Search File Handles and low-memory rescue

### PROC-095 Run dialog
- Was: v1 PROC-091; r2 PROC-079
- Source: TS 8 "TaskSlinger Run", L28472 (Ctrl+N), L29321, L29331-29338, L28653-28812, `runCommandHistory.json`, cross-check A.37 | APP-014, APP-045 | R-061
- Behaviour:
  - File > "Run..." (Ctrl+N) opens a separate pinnable window titled "tasklens Run".
  - Text: "Type the name of a program, folder, document, or Internet resource."
  - "Open:" is a combo with history: 5 visible rows, prefix matches first. Up and Down pick a match, Enter runs, Shift+Enter runs as administrator.
  - Controls: "Run as administrator" checkbox, "Browse…" (filters "Programs" = executable files and "All files"), "Clear command history", OK, Cancel.
  - Under the checkbox: "GUI programs may not start as root on Wayland".
  - Toasts: "Command launched", "Command history cleared", "Please enter a command", "Command contains invalid text." (a NUL byte or a control character), "Failed to run command", "Authentication cancelled" (cancelled polkit prompt).
- Linux data/mechanism:
  - A URL with a scheme, or an existing path that is not an executable, opens through the portal `org.freedesktop.portal.OpenURI`, falling back to `xdg-open`.
  - `kcm_*` names (the `.cpl` analogue) run `systemsettings <name>`.
  - Anything else runs `/bin/sh -c <text>` inside a new transient user scope, `StartTransientUnit("app-tasklens-run-<rand>.scope", "fail", PIDs=[child])` on the user manager, through the shared launcher (SYS-023, merged sys-app ID). It then groups as an app and is detached from tasklens.
  - Run as administrator: `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c <text>` under `org.freedesktop.policykit.exec`, never `tl-helper`.
  - History: `$XDG_STATE_HOME/tasklens/run-history.json`, format `{"entries":[{"command","executedAt","elevated"}]}`, up to 50 entries.
  - Browse uses GPUI `prompt_for_paths` (portal FileChooser). If that pulls in Tokio (U-15), call `org.freedesktop.portal.FileChooser.OpenFile` through zbus directly.
- Verdict: ADAPT
- Phase: P2
- Privilege: none, or polkit `org.freedesktop.policykit.exec` for administrator runs

### PROC-096 Search File Handles
- Was: v1 PROC-092; r2 PROC-080
- Source: TS 8 dialog, L28473, L29339-29345, L28677-28679, L30071-30081 | APP-015, APP-045 | R-061
- Behaviour:
  - File > "Search File Handles..." opens a separate pinnable window titled "tasklens Handle Search".
  - Controls: "Pattern:" field (case-insensitive substring; `*` and `?` work as globs), the checkbox "Include memory-mapped files, working directories and executables" (default on), Search, Clear, Close.
  - Progress: "Initializing... please wait.", "Retrieving file handles... 42% (18 234 total handles)", "Searching...", then "Found 12 matches" or "No results found.".
  - Result columns: Process, PID, User, Type (fd, mmap, cwd, exe, root), FD, Path.
  - Result menu: End Task (SIGTERM), End Process (SIGKILL), Go to process (PROC-071), Copy path, Open file location. The labels follow R-045 naming (R-102).
  - Without the helper, a footer note reads "Searched 412 of 530 processes. 118 need administrator rights." with a "Search all" button that starts the helper and searches again.
  - Error: "Handle search failed".
- Linux data/mechanism: walk `/proc/*/fd` readlinks, `/proc/*/maps` paths, `cwd`, `exe` and `root` on the blocking pool, cancellable between PIDs. Other UIDs go through the helper (`ReadProcessFiles { fd_list, maps, cwd, exe }`).
- Verdict: ADAPT (the NT handle table becomes a procfs walk, like lsof)
- Phase: P3
- Privilege: own processes need none; other UIDs need the helper.

### PROC-097 Low-memory watcher
- Was: v1 PROC-093; r2 PROC-081
- Source: TS `/low-memory` L35425, [CL 0.8.0], 4.5 | TS inventory section 9 (PSI trigger) | R-062
- Behaviour:
  - Settings > Window & startup > "Open rescue window on critical memory pressure", default off (`lowmem.enabled = false`).
  - When turned on, tasklens installs and enables the user unit `tasklens-lowmem.service`, which runs `tasklens --watch-memory`. Turning it off disables and removes the unit.
  - When memory pressure crosses the threshold and no rescue window is open, the watcher starts `tasklens --low-memory`.
  - Threshold: PSI `full` above 10% over a 1 s window, or MemAvailable below 3% of MemTotal.
- Linux data/mechanism:
  - Write `full 100000 1000000` to an open `/proc/pressure/memory` fd and `poll(POLLPRI)`. Also check MemAvailable every 5 s.
  - The watcher mode does not initialise GPUI. Target below 4 MiB RSS. It calls `mlockall(MCL_CURRENT|MCL_FUTURE)` (within the systemd default RLIMIT_MEMLOCK of 8 MiB).
  - The unit sets `OOMScoreAdjust=-900` (U-24). A user unit may lower it only to the user manager's own value; if systemd refuses, the unit uses 0 and the setting tooltip says so.
  - It coexists with systemd-oomd; the setting tooltip notes that oomd may act first. The watcher only opens UI and never kills anything itself.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### PROC-098 Low-memory rescue window
- Was: v1 PROC-094; r2 PROC-082
- Source: TS 4.5, L35830, L28914-28916, L28921, L28926-28929 | R-062, R-073
- Behaviour:
  - A small window titled "tasklens - Low memory", kept above other windows (PROC-056).
  - Text: "Your device is running on critically low memory." and "We recommend you close some of the processes below."
  - A "Filter..." field and a list of at most 30 apps and processes sorted by memory (RssAnon + swap for processes, cgroup `memory.current` for apps), each with "Memory 12.0%" of MemTotal and an "End Task" button.
  - Here "End Task" sends SIGKILL to processes and `KillUnit(name, "all", SIGKILL)` to apps, with no confirmation, because a graceful stop under memory pressure makes thrashing worse. Tooltip "Ends immediately, unsaved work is lost".
  - Other users' rows are listed but disabled, with the tooltip "Needs administrator rights".
  - "Open Full tasklens" opens the main window.
  - When pressure stays below the threshold for 10 s: "Memory pressure has eased. You can open the full tasklens or close this window."
  - Errors: "End Task Failed", "kill failed: <errno text>".
- Linux data/mechanism:
  - A minimal GPUI window: small font set, no charts, no icons, no history.
  - A stripped sweep (`stat`, `status`, cgroup `memory.current`) every 2 s.
  - The process is mlocked after start. The same identity and pidfd rules as PROC-001 apply.
- Verdict: ADAPT
- Phase: P3
- Privilege: none for own processes.

---

## L. Settings

### PROC-099 Settings and state keys for this area
- Was: v1 PROC-095; add amend PROC-095
- Source: TS INI `UI.*`, `Sort.*`, `Columns.*` | MC gschema `apps-page-*` | ADR-011 | R-018, R-019, R-071, R-075, R-095, RECONCILIATION table (a)
- Behaviour: this area uses only these canonical keys (table a). Both files start with `schema_version = 1`, load with clamping, and save atomically with a 500 ms debounce.
  - `$XDG_CONFIG_HOME/tasklens/settings.toml`:

    | Key | Default | Entry |
    |---|---|---|
    | `processes.interval_ms` | 1000 (250..=10000) | PROC-002 |
    | `processes.mode` | "grouped" | PROC-014 |
    | `processes.tree_guide_lines` | false | PROC-017 |
    | `processes.highlight_new_exited` | true | PROC-043 |
    | `processes.filter_include_children` | false | PROC-045 |
    | `processes.hide_system` | false | PROC-046 |
    | `processes.subtree_totals` | false | PROC-019 |
    | `tables.dim_zero_values` | false | PROC-041 |
    | `tables.column_separators` | false | PROC-042 |
    | `tables.column_resize` | "proportional" | PROC-025 |
    | `processes.dump_type` | "mini" | PROC-075 |
    | `appearance.heat_coloring` | true | PROC-040 |
    | `appearance.heat_intensity_pct` | 50 | PROC-040 |
    | `properties.hide_unnamed_handles` | true | PROC-080 |
    | `properties.shared_objects_only` | true | PROC-081 |
    | `properties.env_mask` | true | PROC-083 |
    | `history.enabled` | false | PROC-086 |
    | `history.retention_h` | 6 (1..=12) | PROC-086 |
    | `history.persist` | true | PROC-087 |
    | `history.max_disk_mb` | 256 | PROC-087 |
    | `lowmem.enabled` | false | PROC-097 |
    | `units.network`, `units.network_auto_scale` | "bits", false | PROC-003, PROC-032 |
    | `window.always_on_top` | false | PROC-056 |

  - `$XDG_STATE_HOME/tasklens/state.toml`: `[processes] history_mode = false`, `group_expanded = { apps = true, background = true, system = true }`, `virustotal_notice_accepted = false` (PROC-074, R-095); `[sort.processes]` (`pid`, ascending); `[sort.history]` (`cpu`, descending); `[columns.processes]` and `[columns.history]` (`order`, `visible`, `widths`); `[[minigraphs]]`; `[[tray_graphs]]`.
  - `EngineSettings` (derived, never persisted) carries `interval_ms`, `subtree_totals`, `hide_system`, the network unit and `history.*`.
  - Old keys from the v1 settings list (absorbed into this entry): `tree_view` and `grouping` Dropped: R-010; `guide_lines`, `heat`, `heat_intensity_pct`, `dim_zero`, `resize_behavior`, `history.hours` and `props.*` are renamed to the keys above (R-019); `sort.*`, `columns.*`, `history.sort` and `history_mode` move to `state.toml` (R-018); `copy_rows_tsv_shortcut` Dropped: R-071 (the binding is fixed in table b).
  - Never added: `processes.filter_cmdline` (R-043), `processes.cpu_mode` and `processes.cpu_one_core_100` (R-044), `processes.column_graphs` (R-071).
- Linux data/mechanism: atomic write (temp file, fsync, rename, fsync directory) with migrations on version change (ADR-011). No inotify; external edits apply at the next start.
- Verdict: ADAPT (INI becomes TOML)
- Phase: P1 (each key arrives with its feature)
- Privilege: none

---

## Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PROC-001 | Process identity and action targeting | BUILD | P1 |
| PROC-002 | Process sweep and per-process data pipeline | BUILD | P1 |
| PROC-003 | Value formats, zero values and units | ADAPT | P1 |
| PROC-004 | Process classification and Task Manager groups | ADAPT | P1 |
| PROC-005 | App detection from cgroups | BUILD | P1 |
| PROC-006 | Desktop-file index, exe fallback, disambiguation and icons | BUILD | P1 |
| PROC-007 | Process display naming | ADAPT | P1 |
| PROC-008 | App row contents | ADAPT | P1 |
| PROC-009 | Unit rows and the Kernel threads row | ADAPT | P1 |
| PROC-010 | Flatpak bwrap unwrap | BUILD | P1 |
| PROC-011 | Launcher split (Steam, Lutris, Heroic) | ADAPT | P4 |
| PROC-012 | Pseudo-process rows | ADAPT | P2 |
| PROC-013 | Table widget basis | BUILD | P1 |
| PROC-014 | View modes and View > Process View | ADAPT | P1 |
| PROC-015 | Toolbar view-mode cycler | BUILD | P1 |
| PROC-016 | Processes-tab toolbar buttons | BUILD | P1 |
| PROC-017 | Tree Connector Lines | BUILD | P1 |
| PROC-018 | PID cell "PID +N children" | BUILD | P1 |
| PROC-019 | Subtree totals on parent processes | BUILD | P2 |
| PROC-020 | Collapse All | BUILD | P1 |
| PROC-021 | Startup placeholder | ADAPT | P1 |
| PROC-022 | Column model, default set and visibility | ADAPT | P1 |
| PROC-023 | Column minimum and default widths | BUILD | P1 |
| PROC-024 | Column picker | ADAPT | P1 |
| PROC-025 | Column reorder, resize and persistence | BUILD | P1 |
| PROC-026 | Header context menu | BUILD | P1 |
| PROC-027 | Columns: identity, image and package | BUILD/ADAPT | P1-P2 |
| PROC-028 | Columns: ownership, session and security | BUILD/ADAPT | P1-P2 |
| PROC-029 | Columns: CPU and scheduling | BUILD/ADAPT | P1, P2, P4 |
| PROC-030 | Columns: memory | BUILD/ADAPT, 1 DROP | P1-P2 |
| PROC-031 | Columns: I/O and disk | BUILD/ADAPT, 1 DROP | P1, P2, P4 |
| PROC-032 | Column: network | ADAPT | P1, P4 |
| PROC-033 | Columns: GPU | BUILD/ADAPT | P1-P2 |
| PROC-034 | Columns: Power usage and Power usage trend | ADAPT | P2 |
| PROC-035 | Columns: object counts | BUILD/ADAPT | P1, P3, P4 |
| PROC-036 | Linux-only extra columns | ADAPT | P1, P2, P4 |
| PROC-037 | TaskSlinger column id map | ADAPT | P1 |
| PROC-038 | Two-line header with system totals | BUILD | P1 |
| PROC-039 | Row hover tooltip | ADAPT | P1 |
| PROC-040 | Heat colouring | BUILD | P1 |
| PROC-041 | Dim zero values | BUILD | P2 |
| PROC-042 | Column separators | BUILD | P4 |
| PROC-043 | Highlight new and exited processes | BUILD | P1 |
| PROC-044 | Filter box and type-to-filter | ADAPT | P1 |
| PROC-045 | Include children | BUILD | P1 |
| PROC-046 | Hide system processes | ADAPT | P1 |
| PROC-047 | Sorting | BUILD | P1 |
| PROC-048 | Selection and multi-select | BUILD | P1 |
| PROC-049 | Keyboard in the process table | ADAPT | P1 |
| PROC-050 | Hold Ctrl to pause | BUILD | P1 |
| PROC-051 | Processes status bar text | BUILD | P1 |
| PROC-052 | KWin window bridge | ADAPT | P2 |
| PROC-053 | App window titles and the Name-cell window button | ADAPT | P2 |
| PROC-054 | Show Window | ADAPT | P2 |
| PROC-055 | Window picker ("+" button) | ADAPT | P3 |
| PROC-056 | Pin on top for tasklens windows | ADAPT | P2 |
| PROC-057 | Row context menu | ADAPT | P1 (items per entry) |
| PROC-058 | Action pipeline, results and toasts | BUILD | P1 |
| PROC-059 | Protection rules and confirmations | ADAPT | P1 |
| PROC-060 | End Task and End App (SIGTERM) | ADAPT | P1 |
| PROC-061 | End Process (SIGKILL) | ADAPT | P1 |
| PROC-062 | End Process Tree | ADAPT | P1 |
| PROC-063 | Suspend and Resume process | ADAPT | P1 |
| PROC-064 | Freeze App, Thaw App and Kill App | ADAPT | P2 |
| PROC-065 | Send Signal submenu | BUILD | P2 |
| PROC-066 | Set Priority | ADAPT | P1, P2 |
| PROC-067 | Set Affinity | ADAPT | P1, P2 |
| PROC-068 | Set I/O Priority | ADAPT | P2 |
| PROC-069 | Multi-select priority, affinity, I/O priority, copy and end | BUILD | P1, P2 |
| PROC-070 | Copy, Copy details, Copy all metrics and Copy rows as table | BUILD | P1 |
| PROC-071 | Process jump target, Go to parent and Go to service | BUILD | P1, P2 |
| PROC-072 | Restart Plasma Shell and Restart unit | ADAPT | P2 |
| PROC-073 | Open file location and File properties | ADAPT | P1, P2 |
| PROC-074 | Open in VirusTotal | BUILD | P2 |
| PROC-075 | Create dump file | ADAPT | P3 |
| PROC-076 | Privileged helper for process operations | ADAPT | P3 |
| PROC-077 | Process Properties window | ADAPT | P2 |
| PROC-078 | General tab | ADAPT | P2 |
| PROC-079 | Threads tab | ADAPT | P2 |
| PROC-080 | File descriptors tab | ADAPT | P2 |
| PROC-081 | Memory maps tab | ADAPT | P2 |
| PROC-082 | Security tab | ADAPT | P2 |
| PROC-083 | Environment tab | ADAPT | P2 |
| PROC-084 | Strings tab | ADAPT | P3 |
| PROC-085 | App Properties window | ADAPT | P2 |
| PROC-086 | History recording | ADAPT | P3 |
| PROC-087 | History persistence | ADAPT | P3 |
| PROC-088 | History view and table | BUILD | P3 |
| PROC-089 | History timeline scrubber | BUILD | P3 |
| PROC-090 | History top-3 hover | BUILD | P3 |
| PROC-091 | History first-snapshot state | BUILD | P3 |
| PROC-092 | History scrub benchmark | BUILD | P3 |
| PROC-093 | Process minigraphs from the row menu | BUILD | P3 |
| PROC-094 | Process tray icon graphs | ADAPT | P3 |
| PROC-095 | Run dialog | ADAPT | P2 |
| PROC-096 | Search File Handles | ADAPT | P3 |
| PROC-097 | Low-memory watcher | ADAPT | P3 |
| PROC-098 | Low-memory rescue window | ADAPT | P3 |
| PROC-099 | Settings and state keys for this area | ADAPT | P1 |

## Old ID to new ID map

Every old ID in the three sources maps to at least one entry. Where an old entry was split, every target is listed.

### v1 (features-proc.md)

| Old ID | New ID |
|---|---|
| v1 conventions | PROC-003 |
| v1 section D intro | PROC-022 |
| v1 PROC-001 | PROC-001, PROC-058 |
| v1 PROC-002 | PROC-002 |
| v1 PROC-003 | PROC-076 |
| v1 PROC-004 | PROC-014 |
| v1 PROC-005 | PROC-004, PROC-008, PROC-009, PROC-014 |
| v1 PROC-006 | PROC-017 |
| v1 PROC-007 | PROC-013 |
| v1 PROC-008 | PROC-005 |
| v1 PROC-009 | PROC-006 |
| v1 PROC-010 | PROC-006, PROC-007 |
| v1 PROC-011 | PROC-004 |
| v1 PROC-012 | PROC-046 |
| v1 PROC-013 | PROC-010 |
| v1 PROC-014 | PROC-011 |
| v1 PROC-015 | PROC-018, PROC-027, PROC-028 |
| v1 PROC-016 | PROC-027 |
| v1 PROC-017 | PROC-028 |
| v1 PROC-018 | PROC-029 |
| v1 PROC-019 | PROC-029 |
| v1 PROC-020 | PROC-030 |
| v1 PROC-021 | PROC-031 |
| v1 PROC-022 | PROC-031 |
| v1 PROC-023 | PROC-032 |
| v1 PROC-024 | PROC-033 |
| v1 PROC-025 | PROC-034 |
| v1 PROC-026 | PROC-035 |
| v1 PROC-027 | PROC-036 |
| v1 PROC-028 | PROC-037 |
| v1 PROC-029 | PROC-038 |
| v1 PROC-030 | PROC-052 |
| v1 PROC-031 | PROC-053 |
| v1 PROC-032 | PROC-039 |
| v1 PROC-033 | PROC-040 |
| v1 PROC-034 | PROC-041 |
| v1 PROC-035 | PROC-042 |
| v1 PROC-036 | PROC-043 |
| v1 PROC-037 | PROC-047 |
| v1 PROC-038 | PROC-025 |
| v1 PROC-039 | PROC-048 |
| v1 PROC-040 | PROC-049 |
| v1 PROC-041 | PROC-044 |
| v1 PROC-042 | PROC-045 |
| v1 PROC-043 | PROC-050 |
| v1 PROC-044 | PROC-051 |
| v1 PROC-045 | PROC-020 |
| v1 PROC-046 | PROC-021 |
| v1 PROC-047 | PROC-026 |
| v1 PROC-048 | PROC-024 |
| v1 PROC-049 | PROC-057 |
| v1 PROC-050 | PROC-058, PROC-060 |
| v1 PROC-051 | PROC-061, PROC-064 |
| v1 PROC-052 | PROC-062 |
| v1 PROC-053 | PROC-063 |
| v1 PROC-054 | PROC-064 |
| v1 PROC-055 | PROC-059 |
| v1 PROC-056 | PROC-065 |
| v1 PROC-057 | PROC-066 |
| v1 PROC-058 | PROC-070 |
| v1 PROC-059 | PROC-067 |
| v1 PROC-060 | PROC-068 |
| v1 PROC-061 | PROC-054 |
| v1 PROC-062 | PROC-055 |
| v1 PROC-063 | PROC-056 |
| v1 PROC-064 | PROC-071 |
| v1 PROC-065 | PROC-071 |
| v1 PROC-066 | PROC-072 |
| v1 PROC-067 | PROC-073 |
| v1 PROC-068 | PROC-073 |
| v1 PROC-069 | PROC-074 |
| v1 PROC-070 | PROC-075 |
| v1 PROC-071 | PROC-077 |
| v1 PROC-072 | PROC-078 |
| v1 PROC-073 | PROC-079 |
| v1 PROC-074 | PROC-080 |
| v1 PROC-075 | PROC-081 |
| v1 PROC-076 | PROC-082 |
| v1 PROC-077 | PROC-083 |
| v1 PROC-078 | PROC-084 |
| v1 PROC-079 | PROC-085 |
| v1 PROC-080 | PROC-086 |
| v1 PROC-081 | PROC-087 |
| v1 PROC-082 | PROC-088 |
| v1 PROC-083 | PROC-089 |
| v1 PROC-084 | PROC-090 |
| v1 PROC-085 | PROC-091 |
| v1 PROC-086 | PROC-092 |
| v1 PROC-087 | PROC-093 |
| v1 PROC-088 | PROC-093 |
| v1 PROC-089 | PROC-093 |
| v1 PROC-090 | PROC-094 |
| v1 PROC-091 | PROC-095 |
| v1 PROC-092 | PROC-096 |
| v1 PROC-093 | PROC-097 |
| v1 PROC-094 | PROC-098 |
| v1 PROC-095 | PROC-019, PROC-099 |

### r2 (features-proc-r2.md)

| Old ID | New ID |
|---|---|
| r2 conventions | PROC-001 |
| r2 PROC-001 | PROC-002 |
| r2 PROC-002 | PROC-014, PROC-015 |
| r2 PROC-003 | PROC-017 |
| r2 PROC-004 | PROC-004 |
| r2 PROC-005 | PROC-005 |
| r2 PROC-006 | PROC-006 |
| r2 PROC-007 | PROC-008 |
| r2 PROC-008 | PROC-009 |
| r2 PROC-009 | PROC-011 |
| r2 PROC-010 | PROC-012 |
| r2 PROC-011 | PROC-007 |
| r2 PROC-012 | PROC-018 |
| r2 PROC-013 | PROC-039 |
| r2 PROC-014 | PROC-053 |
| r2 PROC-015 | PROC-019 |
| r2 PROC-016 | PROC-022, PROC-023, PROC-024, PROC-025 |
| r2 PROC-017 | PROC-027 |
| r2 PROC-018 | PROC-028 |
| r2 PROC-019 | PROC-029 |
| r2 PROC-020 | PROC-030 |
| r2 PROC-021 | PROC-031 |
| r2 PROC-022 | PROC-032 |
| r2 PROC-023 | PROC-033 |
| r2 PROC-024 | PROC-034 |
| r2 PROC-025 | PROC-035 |
| r2 PROC-026 | PROC-036 |
| r2 PROC-027 | PROC-003 |
| r2 PROC-028 | PROC-038 |
| r2 PROC-029 | PROC-040 |
| r2 PROC-030 | PROC-041 |
| r2 PROC-031 | PROC-043 |
| r2 PROC-032 | PROC-044 |
| r2 PROC-033 | PROC-045 |
| r2 PROC-034 | PROC-046 |
| r2 PROC-035 | PROC-047 |
| r2 PROC-036 | PROC-048 |
| r2 PROC-037 | PROC-049 |
| r2 PROC-038 | PROC-050 |
| r2 PROC-039 | PROC-051 |
| r2 PROC-040 | PROC-026 |
| r2 PROC-041 | PROC-057 |
| r2 PROC-042 | PROC-058 |
| r2 PROC-043 | PROC-060 |
| r2 PROC-044 | PROC-061 |
| r2 PROC-045 | PROC-062 |
| r2 PROC-046 | PROC-065 |
| r2 PROC-047 | PROC-063 |
| r2 PROC-048 | PROC-064 |
| r2 PROC-049 | PROC-066 |
| r2 PROC-050 | PROC-067 |
| r2 PROC-051 | PROC-068 |
| r2 PROC-052 | PROC-059 |
| r2 PROC-053 | PROC-070 |
| r2 PROC-054 | PROC-073 |
| r2 PROC-055 | PROC-074 |
| r2 PROC-056 | PROC-075 |
| r2 PROC-057 | PROC-054 |
| r2 PROC-058 | PROC-055 |
| r2 PROC-059 | PROC-052 |
| r2 PROC-060 | PROC-072 |
| r2 PROC-061 | PROC-071 |
| r2 PROC-062 | PROC-093 |
| r2 PROC-063 | PROC-094 |
| r2 PROC-064 | PROC-076 |
| r2 PROC-065 | PROC-077 |
| r2 PROC-066 | PROC-078 |
| r2 PROC-067 | PROC-079 |
| r2 PROC-068 | PROC-080 |
| r2 PROC-069 | PROC-081 |
| r2 PROC-070 | PROC-082 |
| r2 PROC-071 | PROC-083 |
| r2 PROC-072 | PROC-084 |
| r2 PROC-073 | PROC-085 |
| r2 PROC-074 | PROC-086 |
| r2 PROC-075 | PROC-088 |
| r2 PROC-076 | PROC-089 |
| r2 PROC-077 | PROC-090 |
| r2 PROC-078 | PROC-087 |
| r2 PROC-079 | PROC-095 |
| r2 PROC-080 | PROC-096 |
| r2 PROC-081 | PROC-097 |
| r2 PROC-082 | PROC-098 |

### add (features-proc-addendum.md)

| Old ID | New ID |
|---|---|
| add PROC-096 | PROC-061 |
| add PROC-097 | PROC-069 |
| add PROC-098 | PROC-070 |
| add PROC-099 | PROC-071 |
| add PROC-100 | PROC-012 |
| add PROC-101 | PROC-023 |
| add PROC-102 | PROC-024 |
| add PROC-103 | PROC-033 |
| add PROC-104 | PROC-003 |
| add PROC-105 | PROC-015 |
| add PROC-106 | PROC-022 |
| add PROC-107 | PROC-016 |

### add amendments to v1 entries

| Amendment | New ID |
|---|---|
| add amend PROC-004 | PROC-014 (Dropped: R-010) |
| add amend PROC-012 | PROC-046 |
| add amend PROC-015 | PROC-023 |
| add amend PROC-020 | PROC-030 |
| add amend PROC-024 | PROC-033 |
| add amend PROC-028 | PROC-037 |
| add amend PROC-032 | PROC-039 |
| add amend PROC-038 | PROC-025 |
| add amend PROC-039 | PROC-048 |
| add amend PROC-040 | PROC-049 |
| add amend PROC-048 | PROC-024 |
| add amend PROC-049 | PROC-057 |
| add amend PROC-051 | PROC-061 |
| add amend PROC-058 | PROC-070 |
| add amend PROC-064 | PROC-071 |
| add amend PROC-065 | PROC-071 |
| add amend PROC-072 | PROC-078 |
| add amend PROC-073 | PROC-079 |
| add amend PROC-095 | PROC-099 |
## Still open

Nothing. The open items of this merge were settled in `docs/RECONCILIATION.md`:

| Item | Resolution | Entries |
|---|---|---|
| 1. "Copy rows as table" in the row menu | R-091 | PROC-057 |
| 2. Freeze, Thaw and Kill for units outside `app.slice` | R-092 | PROC-064 |
| 3. `IoPriority::None` | R-093 | PROC-068 |
| 4. Copy details CPU decimals | R-094 | PROC-070 |
| 5. VirusTotal one-time notice flag | R-095 | PROC-074, PROC-099 |
| 6. Hash crates | R-096 | PROC-074, PROC-078 |
| 7. Thread-level action target | R-097 | PROC-079 |
| 8. `smaps` for other users' memory maps | R-098 | PROC-081 |
| 9 and 10. Environment mask list and copying masked values | R-099 | PROC-083 |
| 11. Boot ID in saved process minigraphs | R-100 | PROC-093 |
| 12. Phase of process minigraph cards | R-101 | PROC-093 |
| 13. Handle Search result menu labels | R-102 | PROC-096 |
| Inferred mechanisms (app-row `io.stat` fallback, in-RAM rolling history window, deleted-keys list) | R-103 | PROC-008, PROC-086, PROC-088 |

Unverified assumptions this spec depends on (RECONCILIATION list): U-11, U-12, U-13, U-14, U-15, U-19, U-21, U-22, U-24, U-32, U-33, U-34, U-38.
