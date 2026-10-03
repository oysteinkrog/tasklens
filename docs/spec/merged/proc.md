# tasklens merged spec: processes, apps and process actions

This is the merged PROC spec (round 3). It replaces `features-proc.md` (v1), `features-proc-r2.md` (r2) and `features-proc-addendum.md` (add) for implementation. It covers the Processes tab, apps and units, process actions, the Process and App Properties windows, History mode, the process parts of minigraphs and tray graphs, the Run dialog, Search File Handles and the low-memory rescue window.

Precedence used: `docs/DECISIONS.md` (D-001 to D-010), then `docs/RECONCILIATION.md` (R-001 to R-074 and canonical tables a to d), then `docs/ARCHITECTURE.md` on engineering structure, then the source specs (r2 over v1 on a shared topic, the addendum where it is later and more specific). "Was:" lines name every old ID an entry absorbs. Old IDs are prefixed `v1`, `r2` or `add`; "add amend PROC-0xx" means the addendum's amendment to that v1 entry.

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
  - Metrics: CPU, Disk, Network and GPU are sums over the app's processes. Memory is cgroup `memory.stat` `anon`, with the tooltip "Anonymous memory charged to this app's cgroup" (R-005, Q-5). When a member's per-process Disk value is Restricted, the row uses the cgroup `io.stat` rate (Δ(rbytes + wbytes)/Δt over all devices) instead, which every user can read (inferred, so system units still show a value).
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
  - "Resize Behavior: Proportional / Last Column", default Proportional (`processes.column_resize = "proportional"`).
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
