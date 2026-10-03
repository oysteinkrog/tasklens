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
