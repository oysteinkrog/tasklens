# PROC area spec: processes and apps

Coverage: TaskSlinger 0.9.2 (TS) and Mission Center 1.2.0 (MC). Everything about processes, apps, process actions, Process Properties, History mode, process graphs, the Run dialog, Search File Handles and the low-memory rescue window.

I checked these facts on the owner's machine, and several entries depend on them:
- Kernel 7.2 with cgroup v2. `cgroup.freeze` and `cgroup.kill` exist in every user unit.
- The user manager has `app.slice`, `session.slice`, `background.slice` and `init.scope`. KDE puts apps in `app-<id>@<rand>.service` and autostart items in `app-<id>@autostart.service`. `plasma-kwin_wayland.service` and `plasma-plasmashell.service` sit in `session.slice`.
- `kernel.yama.ptrace_scope = 1`. Reading `/proc/<pid>/mem`, `process_vm_readv` and `gcore` therefore need CAP_SYS_PTRACE even for the user's own processes, unless the target is a child of tasklens. `environ`, `maps`, `fd` and `io` are allowed for same-uid processes, because Yama only restricts attach mode.
- `/proc/1/io`, `/proc/1/fd` and `/proc/1/environ` give EACCES to uid 1000.
- RAPL `intel-rapl:0/energy_uj` can be read without root because `/etc/udev/rules.d/99-powercap.rules` exists.
- `sched_autogroup_enabled = 1` and `perf_event_paranoid = 2`.
- `/sys/devices/cpu_core` is missing (AMD CPU, so there are no P-cores or E-cores).
- KWin 6.7.5 exposes `org.kde.KWin.queryWindowInfo()`, `getWindowInfo(QString)` and `org.kde.kwin.Scripting.loadScript`.
- `gdb`/`gcore` and `coredumpctl` are installed.

Conventions used in every entry:
- **Validity glyphs** (binding rule from the architecture review): "…" means warming up (no first delta yet). A lock icon means restricted (permission). "n/a" means unsupported on this hardware or kernel. A dimmed value with an age tooltip means stale. "?" means collection failed. A missing value is never drawn as 0.
- **Identity:** `ProcId = (boot_id, pid, starttime_ticks)`. Selections, menu targets, graphs and history all key on ProcId, never on row index.
- **Helper:** the privileged helper is `tl-helper`. It is started once per session with `pkexec /usr/lib/tasklens/tl-helper`, under polkit action `org.tasklens.helper.process` (`auth_admin_keep`). It talks over a stdin/stdout length-prefixed protocol, takes a fixed whitelist of operations, revalidates the ProcId and acts through pidfd. Nothing ever retries through it on its own: on EPERM the toast offers "Retry as administrator".

---

## A. Foundations

### PROC-001 Process identity and action targeting
- Source: TS L28922-28926 ("the selected process changed; wait for the list to update and try again") | MC `apps_page/actions.rs:56` | architecture review §5 and §6.3 (binding)
- Behaviour: every row, selection, open menu, properties window, graph and history entry holds a ProcId. When a menu opens, it captures the ProcIds of the selection at that moment. If the target exited, or the pid was reused, the action does nothing and shows the TS toast "The selected process changed; wait for the list to update and try again."
- Linux data/mechanism:
  - `starttime` is field 22 of `/proc/<pid>/stat`. `boot_id` comes from `/proc/sys/kernel/random/boot_id`.
  - Before every action: `pidfd_open(pid, 0)`, then re-read `/proc/<pid>/stat` and compare starttime. Signals then go through `pidfd_send_signal`.
  - Priority, affinity and ioprio take a pid, not a pidfd. They revalidate right before the syscall and are reported as best-effort, per the review.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-002 Process sweep, rates and demand
- Source: TS [INI UI.RefreshIntervalMs] | MC magpie `processes/` | PLAN §3 | architecture review §3 (binding per-source rates)
- Behaviour: process data refreshes at the Settings > Refresh rate: 0.25, 0.5, 1.0 (default), 2.0, 5.0 or 10.0 s. Rates use the real elapsed time between samples. A rate column shows "…" until its second sample arrives.
- Linux data/mechanism:
  - Every sweep: readdir `/proc`, then read `stat`, `statm`, `status` and `cmdline` (bounded to 64 KiB) per pid. `cgroup` is re-read only when a pid is new or after an exec (exe inode changed).
  - Demand-driven reads, only while a consumer exists (visible column, properties window, graph or history):
    - `io` while any I/O or disk column is visible;
    - `smaps_rollup` for PSS/USS;
    - `/proc/<pid>/fd` count for Handles;
    - `task/*/status` for context switches;
    - NVML through `tl-gpu-probe`;
    - sock_diag for network.
  - Process-directory FDs are opened per sweep and closed straight away, with at most 128 cached (review §5).
  - The sweep is one supervised worker in `tl-engine`. It publishes `ProcessSnapshot` with a version and an observation interval.
  - When no process consumer exists (window hidden, no graph, no recording), the sweep stops.
- Verdict: BUILD
- Phase: P1
- Privilege: none. Other users' `io`, `fd`, `exe`, `environ`, `maps` and `smaps_rollup` show the lock glyph unless the helper is running (PROC-003).

### PROC-003 Privileged read mode for other users' processes
- Source: TS shield "relaunch elevated" L30127 | MC setup script (setcap on nethogs) | PLAN §2 tl-helper | review §6.8
- Behaviour: when the user clicks the shield or "Retry as administrator", tl-helper starts. Restricted cells for root and system processes then fill in: Handles, I/O, Disk, Image path, Memory maps and Environment. The shield icon turns to `#shield-hi` while the helper is alive.
- Linux data/mechanism: the helper whitelist for reads is `read_proc_files(ProcId, [io, fd-list, exe-link, environ, maps, smaps_rollup, fdinfo])`, returning bytes capped at 4 MiB per call. The engine merges these into the same sweep. There is no arbitrary file read: paths are built from the ProcId inside the helper.
- Verdict: ADAPT. Elevation comes from a pkexec helper, not a relaunch of the whole GUI as root.
- Phase: P3
- Privilege: pkexec + polkit `org.tasklens.helper.process` (`auth_admin_keep`). Without it, the lock glyph shows, with the tooltip "Needs administrator. Click the shield to grant access."

---

## B. Processes tab layout and view modes

### PROC-004 Tree View / List View
- Source: TS L28488, L28490, [INI UI.TreeView=1] | MC `table_view/models.rs:313` (ppid tree)
- Behaviour:
  - View > Process View has two radio items: "Tree View" (default) and "List View".
  - Tree View shows the ppid hierarchy with chevrons. A row is a root when its parent is not shown (hidden by filter or by Hide system processes, or exited).
  - List View is flat.
  - Expand state is kept per ProcId for the session.
- Linux data/mechanism: ppid is field 4 of `/proc/<pid>/stat`. Orphans have already been reparented by the kernel to the subreaper (`systemd --user`) or pid 1. The flattened visible-row list is built in the engine projection (review §5 "Tree ordering").
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-005 Task Manager Grouping
- Source: TS L28489, L28448-28454, L35693-35711, [INI UI.TaskManagerGrouping=1, TaskManager*Expanded=1] | MC Apps/Processes sections `apps_page/mod.rs:85-94`
- Behaviour:
  - A toggle, "Task Manager Grouping", on by default. It works on top of either Tree View or List View.
  - Top-level groups, each shown as "Name (N)": "Apps", "Background processes" and "System processes" (renamed from "Windows processes"). Each group's expand state persists (`processes.group_expanded.{apps,background,system}`, default true).
  - Inside **Apps**: one row per app instance (PROC-008), with that app's processes beneath it. Tree View shows them as a ppid subtree; List View shows them flat.
  - Inside **Background**: one row per user systemd unit that has 2 or more processes (the "Service Host" analogue, name = unit description). A unit with a single process shows as that process row.
  - Inside **System**: one row per system unit with its processes, plus one row "Kernel threads (N)" that holds all kernel threads.
  - When grouping is off, MC's two sections are not used. TS behaviour wins.
- Linux data/mechanism: classification comes from the cgroup path in `/proc/<pid>/cgroup` (PROC-011). Unit descriptions come from systemd D-Bus `org.freedesktop.systemd1.Unit.Description`, cached and refreshed on `UnitNew`/`UnitRemoved`.
- Verdict: ADAPT. "Windows processes" becomes "System processes", and Service Host grouping becomes systemd-unit grouping.
- Phase: P1
- Privilege: none

### PROC-006 Tree Connector Lines
- Source: TS L28491, [INI UI.TreeGuideLines=0]
- Behaviour: a toggle under View > Process View, off by default. It draws 1 px guide lines in the DIVIDER colour from parent to child, through the indent column.
- Linux data/mechanism: drawn with `uniform_list` `with_decoration` or in `render_td` (claude-gpui-apis.md §4 Lists).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-007 Table widget basis
- Source: claude-gpui-apis.md §4 DataTable gaps 1-6 | review §5 "Selection/context menus", "Visible-row demand"
- Behaviour: a virtualized tree table with 28 logical px rows at 100% (scaled by Row Height). Tree rows, multi-select, identity-based selection and column show/hide all work.
- Linux data/mechanism: GPUI `uniform_list` with an app-owned delegate. The DataTable's index-based selection and its left/right column mode are not used. Header, resize and drag-reorder are app-owned or reuse the DataTable header only. Visible ProcIds are reported to the engine after every sort, filter or scroll, with one viewport of overscan.
- Verdict: BUILD (custom tree table on `uniform_list`)
- Phase: P1
- Privilege: none

---

## C. App detection and classification

### PROC-008 App detection from cgroups
- Source: MC app-rummage `cgroup.rs:13-190` | PLAN §3 "App grouping" | review §5 "Application grouping"
- Behaviour: every app instance becomes one App row. The Name is the `.desktop` Name, the icon is the desktop icon, and Description is the desktop Comment (else GenericName).
- Linux data/mechanism:
  - Walk `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/app.slice/`. The uid comes from `getuid()` and is never hard-coded.
  - Parse these unit names, with `\x2d` unescaping (ksysguard rules):
    - `app-[<launcher>-]<id>[@<rand>].{scope,service}`
    - `app-<id>-<rand>.scope`
    - `flatpak-<id>-<n>.scope`
    - `dbus-:<n>-<id>@<n>.service`
    - `snap.<a>.<b>-<uuid>.scope` → `a_b`
  - PIDs come from `cgroup.procs`, recursively.
  - Units named `app-*@autostart.service` (for example arch-update-tray) go to Background, unless PROC-030 reports a mapped window for one of their pids.
  - Non-`app-` units in `app.slice` (dconf, frankenterm-mux, win-vm) go to Background.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-009 Desktop-file index and exe fallback
- Source: MC app-rummage `desktop.rs:78-120`, `exec.rs`, `matching.rs:10-24, 109-175`
- Behaviour:
  - Apps not in their own scope (launched from a terminal or by another app) are still recognised.
  - The match order is: cgroup id → `.desktop` id; then `StartupWMClass` against the KWin window resourceClass (PROC-030); then the canonical exe path against the desktop `Exec` (launcher prefixes `flatpak run`, `snap run` and `env` are stripped).
  - An exception table maps helpers to their app: `firefox-bin`, `soffice.bin`, `gnome-terminal-server`, `chrome`, `chromium`, `electron`.
  - When several apps share a binary, argv tokens decide (Chromium PWAs `--app-id=`). True ties stay unmatched.
- Linux data/mechanism:
  - `.desktop` files come from `$XDG_DATA_DIRS/applications` and `~/.local/share/applications`. Entries with `NoDisplay=true`, `Hidden=true` or a `Type` other than Application are skipped.
  - inotify on those directories rescans them with a 500 ms debounce.
  - The index is built off the UI thread at startup.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-010 Process display names, Wine and icons
- Source: MC `table_view/models.rs:134-168`, magpie `apps.rs:134-184` | TS "Name display" L28358
- Behaviour:
  - Process name is the exe basename. If exe is unreadable, it is the first token of argv[0] with a trailing ":" removed. If that is empty, it is `comm`.
  - Wine processes (exe basename `wine64-preloader`, `wine-preloader`, `wine64` or `wine`, or `comm` ending `.exe`) use the last path part of the `.exe` in argv, splitting on both `/` and `\`.
  - Processes inside an app inherit the app icon. Others get a generic executable icon. Kernel threads get a chip icon and the name `[comm]`.
- Linux data/mechanism:
  - The icon theme comes from `~/.config/kdeglobals [Icons] Theme=`, else `hicolor`. Freedesktop icon lookup at 24 px for app rows and 16 px for process rows, preferring SVG. Absolute `Icon=` paths are loaded directly.
  - Rasterised icons are cached per (name, size, scale).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-011 Process classification (Type and group)
- Source: TS L28448-28451, L29017-29019, L35693-35711 | MC section split
- Behaviour: every process gets one Type. The Type decides its group.
  - **Kernel thread:** PF_KTHREAD bit (0x00200000) set in stat field 9, or ppid 2, or pid 2. Goes to System.
  - **App:** member of an App instance (PROC-008/009). Goes to Apps.
  - **Service:** cgroup under `system.slice/<unit>`. Goes to System.
  - **System:** any other uid other than the current user, or cgroup under `init.scope`, `machine.slice` or a user slice other than ours. Goes to System.
  - **User service:** cgroup under `user@<uid>.service/{session,background,app}.slice/<unit>` and not an App. Goes to Background.
  - **Background:** anything else owned by the current uid, including `session-N.scope` processes such as login shells. Goes to Background.
- Linux data/mechanism: `/proc/<pid>/cgroup` (the `0::` line), stat flags, and status `Uid:` (real uid).
- Verdict: ADAPT. Windows kinds map to cgroup and uid classes.
- Phase: P1
- Privilege: none

### PROC-012 Hide system processes toggle
- Source: TS L28330-28331, L28738-28739, L35335-35345, [INI UI.HideMicrosoftProcesses=0], [CL 0.9.2] "Explorer stays visible"
- Behaviour:
  - Toolbar grid button, with tooltip "Show system background processes" or "Hide system background processes". Toasts: "Hiding system processes" and "Showing system processes". Off by default.
  - When on, it hides:
    - kernel threads;
    - all Service and System types;
    - User services in `session.slice` and `background.slice`, except `plasma-plasmashell.service` and `plasma-kwin_wayland.service`, which stay visible as the Explorer analogue;
    - `init.scope` (`systemd --user`).
  - App rows and units in `app.slice` stay visible.
- Linux data/mechanism: the PROC-011 class plus the unit name from the cgroup path. The exception list is a constant: `plasma-plasmashell.service`, `plasma-kwin_wayland.service`, `org.gnome.Shell@wayland.service`.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-013 Flatpak bwrap unwrap
- Source: MC `apps_page/actions.rs:87` | MC inventory §8
- Behaviour: app-level signals skip `bwrap` wrapper processes and target the first descendant that is not bwrap. bwrap rows still show in the tree.
- Linux data/mechanism: exe basename `bwrap`, then walk the children from `/proc/<pid>/task/<pid>/children`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-014 Launcher child apps (Steam, Lutris, Heroic)
- Source: none in either tool. Linux-only gap: Steam games all land in Steam's app scope.
- Behaviour: under the Steam app row, each game shows as its own child App row with the game name.
- Linux data/mechanism:
  - The `reaper` process argv contains `AppId=<n>` (also seen as `SteamLaunch AppId=<n>`). The name comes from `~/.local/share/Steam/steamapps/appmanifest_<n>.acf` `"name"`.
  - Lutris and Heroic are detected by the `LUTRIS_GAME_UUID` / `HEROIC_APP_NAME` env vars, read from `/proc/<pid>/environ` for same-uid processes.
- Verdict: ADAPT (Linux-only extension)
- Phase: P4
- Privilege: none

---

## D. Columns

All columns use stable string IDs (persisted, review §5).
- **Default visible:** `name`, `user`, `pid`, `cpu`, `memory`, `disk`, `network`, `gpu`, `description`. This is TS's default set minus Power usage, because the review forbids a default power column without a measured source.
- Columns are chosen in the column picker (PROC-048).
- App, unit and group rows use the aggregation stated in each table. Process rows show their own values. "Σ" means a sum over member processes, with no double counting: each pid is counted once per row.

### PROC-015 Column group: identity and naming
- Source: TS L28342-28362, L28414, [INI widths ids 0,1,3,33] | MC columns name, pid
- Behaviour, Linux data/mechanism and verdict per column:

| Column (label) | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Name | name | PROC-010 name. App rows also show the most recent window title in dim text after the name (PROC-031) | desktop Name / unit Description | BUILD | P1 |
| Name display | name_display | desktop Name, else `comm` | same | BUILD | P1 |
| Image name | image_name | basename of `readlink /proc/<pid>/exe`. "(deleted)" suffix kept | blank | BUILD | P1 |
| Image path | image_path | `readlink /proc/<pid>/exe`. If restricted, absolute argv[0] in italic with tooltip "From command line" | desktop file path | ADAPT (fallback for restricted) | P1 |
| Command line | cmdline | `/proc/<pid>/cmdline`, NUL→space, bounded 64 KiB, control chars escaped | primary process cmdline | BUILD | P1 |
| Description | description | desktop Comment/GenericName, else owning package `%DESC%` (PROC-016), else blank | desktop Comment | ADAPT | P1 (desktop) / P2 (package) |
| Type | type | PROC-011 value: App, Background, User service, Service, System, Kernel thread | "App" / "Service" | ADAPT | P1 |
| User | user | real uid from status `Uid:` → name via `getpwuid_r`, cached | owner uid | BUILD | P1 |
| PID | pid | pid. Parent rows show "PID +N children" (`%u +%llu`, TS L35614) when they have children in the current view | blank (MC) | BUILD | P1 |
| PID parent | ppid | stat field 4 | blank | BUILD | P1 |
| Session | session | `/proc/<pid>/sessionid` (audit/logind session). 4294967295 shows "n/a" | blank | ADAPT | P2 |
| Start time | start_time | stat starttime / `sysconf(_SC_CLK_TCK)` + `btime` from `/proc/stat`, local `yyyy-MM-dd HH:mm:ss` | earliest member | BUILD | P1 |

- Verdict: see table
- Phase: see table
- Privilege: none. Image path and Image name for other users are restricted without the helper and use the argv fallback.

### PROC-016 Column group: file and package metadata
- Source: TS L28345-28349, L29128-29138, [CL 0.8.19]
- Behaviour: metadata about the executable file, computed lazily for visible rows. It is cached by (dev, inode, mtime) of the exe, so sorting covers only rows already computed. These columns sort only after a "Compute for all rows" pass, which starts on the first click of their header and shows a progress tooltip, per review §6.4.
- Linux data/mechanism:
  - Owning package: a path→package index built from `/var/lib/pacman/local/*/files` at first use, refreshed by inotify on `/var/lib/pacman/local`. Then read `desc` (`%VERSION%`, `%DESC%`, `%PACKAGER%`, `%BUILDDATE%`, `%URL%`) and `mtree` (gzip, sha256 per file).
  - Flatpak: `/proc/<pid>/root/.flatpak-info` → `[Application] name`, plus `flatpak info` metadata from `/var/lib/flatpak/app/<id>/current/active/metadata`.
  - Wine: parse the PE header and version resource of the `.exe` from argv, with the path translated through `WINEPREFIX` (`drive_c`).

| Column | ID | Linux source | Verdict | Phase |
|---|---|---|---|---|
| Version | version | package `%VERSION%`. Flatpak branch/commit. Wine PE FileVersion. AppImage `X-AppImage-Version` | ADAPT | P2 |
| Time stamp | timestamp | package `%BUILDDATE%`. Wine PE `TimeDateStamp`. ELF has no link time | ADAPT | P2 |
| File modified time | file_mtime | `statx` of exe target, `stx_mtime` | BUILD | P2 |
| File size | file_size | `statx` `stx_size` | BUILD | P2 |
| Company | company | owning package name plus `%URL%` host, e.g. "firefox (mozilla.org)". Wine PE CompanyName | ADAPT | P2 |
| Signer | signer | package `%PACKAGER%` name without the email | ADAPT | P2 |
| Signature status | signature | "Packaged (unchanged)" when sha256 matches mtree. "Packaged (modified)". "Not packaged". "Deleted (upgraded while running)" when exe ends in " (deleted)". "Flatpak". "AppImage". "Wine (PE)" | ADAPT | P2 |
| Architecture | architecture | ELF `e_ident[EI_CLASS]` + `e_machine` of exe → "x86-64", "x86 (32-bit)", "AArch64". Wine: PE `Machine` → "Windows x64 (Wine)" / "Windows x86 (Wine)" | ADAPT | P2 |

- Verdict: ADAPT (Authenticode becomes package integrity)
- Phase: P2
- Privilege: none for files readable by the user. Exe of other users: restricted path, so these columns use the argv path when it is absolute.

### PROC-017 Column group: security
- Source: TS L28354-28357, [CL 0.8.19]
- Behaviour: TS's Windows security columns mapped to Linux equivalents.

| Column (TS) | ID | Linux meaning and source | Verdict | Phase |
|---|---|---|---|---|
| Integrity | security_label | LSM label from `/proc/<pid>/attr/current` (AppArmor profile or SELinux context). "none" when no major LSM is active per `/sys/kernel/security/lsm` | ADAPT | P2 |
| Elevation | elevation | status `Uid:` euid 0 → "Yes (root)". euid ≠ ruid → "Setuid". CapEff ≠ 0 for non-root → "Capabilities". Else "No" | ADAPT | P2 |
| Protection | sandbox | "Flatpak" (`/.flatpak-info` exists in `/proc/<pid>/root`), "Snap" (cgroup `snap.`), "Container (<engine>)" (cgroup `docker-`, `libpod-`, `machine.slice`), "Seccomp" (status `Seccomp: 2`), "NoNewPrivs" (status `NoNewPrivs: 1`), else "None". Multiple values joined with ", " | ADAPT | P2 |

- Verdict: ADAPT
- Phase: P2
- Privilege: none. `/proc/<pid>/root` of other users is restricted, so Flatpak detection falls back to the cgroup name.

### PROC-018 Column group: scheduling
- Source: TS L28361-28362, L28455-28460

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Priority | priority | stat field 19 (nice) and field 41 (policy). Display classes: SCHED_FIFO/RR → "Realtime (rt N)". SCHED_IDLE → "Idle". Otherwise by nice: -20..-11 "High", -10..-1 "Above normal", 0 "Normal", 1..9 "Below normal", 10..19 "Low", with the nice value in parentheses, e.g. "High (-10)". SCHED_BATCH adds " batch" | blank | ADAPT | P1 |
| Affinity | affinity | status `Cpus_allowed_list` (leader thread). "All" when it equals the online mask | blank | ADAPT | P1 |

- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-019 Column group: CPU
- Source: TS L28351-28353, L28388-28395, [CL 0.8.21] | MC column cpu, setting `apps-page-core-count-affects-percentages`

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| CPU | cpu | Δ(utime+stime) of stat fields 14+15 / CLK_TCK / Δt / online CPUs × 100. % of whole machine, 1 decimal | Σ | BUILD | P1 |
| CPU (kernel) | cpu_kernel | Δstime only, same scaling | Σ | BUILD | P1 |
| CPU (user) | cpu_user | Δutime only | Σ | BUILD | P1 |
| CPU (relative) | cpu_relative | same Δ but one logical CPU = 100% (max 3200% here) | Σ | BUILD | P1 |
| CPU time | cpu_time | (utime+stime)/CLK_TCK as `h:mm:ss` | Σ | BUILD | P1 |
| CPU time kernel | cpu_time_kernel | stime | Σ | BUILD | P1 |
| CPU time user | cpu_time_user | utime | Σ | BUILD | P1 |
| Cycles | cycles | `perf_event_open(PERF_COUNT_HW_CPU_CYCLES, exclude_kernel=1, inherit=1)`, one counter per thread, only for visible rows of same-uid processes. Cap of 256 counters total; beyond that, "n/a". Restricted for other uids at paranoid 2 | Σ | ADAPT | P4 |
| Cycles delta | cycles_delta | Δ of the above per interval | Σ | ADAPT | P4 |
| Context switches | ctx_switches | Σ over `/proc/<pid>/task/*/status` `voluntary_ctxt_switches + nonvoluntary_ctxt_switches`. Read for all pids while the column is visible, so sorting stays global | Σ | ADAPT | P2 |
| Context switches delta | ctx_switches_delta | Δ per interval | Σ | ADAPT | P2 |

- Behaviour: MC's "Scale CPU Usage to Core Count" setting is not built as a toggle. The two TS columns (CPU and CPU (relative)) cover both views, and the review fixes whole-machine as the default.
- Verdict: see table
- Phase: see table
- Privilege: none, except Cycles for other uids, which is restricted.

### PROC-020 Column group: memory
- Source: TS L28364-28372, L28396, L28411-28412, L28427 | MC columns memory, swap | review §5 "Memory semantics" (binding)

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Memory | memory | VmRSS (status), header tooltip "Resident set size" | cgroup `memory.stat` anon + shmem when the row is one cgroup; else Σ RSS shown with a "Σ" tooltip | ADAPT (RSS default per review) | P1 |
| Working set private | ws_private | RssAnon | cgroup anon | ADAPT | P1 |
| Working set total | ws_total | VmRSS | as memory | BUILD | P1 |
| Working set peak | ws_peak | VmHWM | blank | BUILD | P1 |
| Working set shared | ws_shared | RssFile + RssShmem | blank | BUILD | P1 |
| Commit size | commit | VmData + VmStk | Σ | ADAPT | P1 |
| Commit size peak | commit_peak | tasklens-observed max of commit since first seen (header tooltip "Observed peak") | blank | ADAPT | P2 |
| Commit size delta | commit_delta | Δ commit per interval | Σ | ADAPT | P2 |
| Virtual size | vsize | VmSize | blank | BUILD | P1 |
| Virtual size peak | vsize_peak | VmPeak | blank | BUILD | P1 |
| Paged pool peak | page_tables | becomes "Page tables": VmPTE | Σ | ADAPT (no per-process paged pool; page tables are the per-process kernel memory Linux reports) | P2 |
| Non-paged pool peak | (none) | merged into Page tables | n/a | DROP: Linux charges no per-process non-paged pool. The only per-process kernel memory figure, VmPTE, is already the Page tables column | n/a |
| Page faults | page_faults | stat minflt (field 10) + majflt (field 12) | Σ | BUILD | P1 |
| Page faults delta | page_faults_delta | Δ per interval | Σ | BUILD | P1 |
| Swap (MC) | swap | VmSwap | cgroup `memory.swap.current` | BUILD | P1 |
| PSS (Linux) | pss | `smaps_rollup` Pss. Visible rows only; sorting disabled with tooltip "Measured for visible rows only" | Σ | BUILD | P2 |
| USS (Linux) | uss | `smaps_rollup` Private_Clean + Private_Dirty. Same rule | Σ | BUILD | P2 |

- Verdict: see table
- Phase: see table
- Privilege: none for status fields. `smaps_rollup` for other uids is restricted without the helper.

### PROC-021 Column group: I/O (all read/write syscalls)
- Source: TS L28373-28378, L28397-28398

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| I/O read bytes | io_read_bytes | `/proc/<pid>/io` rchar | Σ | BUILD | P1 |
| I/O write bytes | io_write_bytes | wchar | Σ | BUILD | P1 |
| I/O other bytes | (none) | none | n/a | DROP: Linux has no per-process counter for I/O outside read/write (ioctl and similar) | n/a |
| I/O read bytes delta | io_read_delta | Δrchar | Σ | BUILD | P1 |
| I/O write bytes delta | io_write_delta | Δwchar | Σ | BUILD | P1 |
| I/O read+other rate | io_read_rate | Δrchar/Δt, relabelled "I/O read rate" | Σ | ADAPT | P1 |
| I/O write rate | io_write_rate | Δwchar/Δt | Σ | BUILD | P1 |
| Read ops (Linux) | io_read_ops | syscr | Σ | BUILD | P2 |
| Write ops (Linux) | io_write_ops | syscw | Σ | BUILD | P2 |

- Verdict: see table
- Phase: see table
- Privilege: `io` of other uids is restricted without the helper.

### PROC-022 Column group: disk (storage layer)
- Source: TS L28372, L28399-28410, [INI width id 13 "Disk"] | MC column drive

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Disk | disk | Δ(read_bytes + write_bytes)/Δt from `/proc/<pid>/io`, MB/s | cgroup `io.stat` Σ(rbytes+wbytes) over all devices, as a rate. This works unprivileged even for system units | BUILD (MC "Drive" merges here) | P1 |
| Disk read rate | disk_read_rate | Δread_bytes/Δt | io.stat rbytes | BUILD | P1 |
| Disk write rate | disk_write_rate | Δwrite_bytes/Δt. Tooltip shows cancelled_write_bytes | io.stat wbytes | BUILD | P1 |
| Disk total rate | disk_total_rate | same as disk | same | BUILD | P1 |
| Disk read bytes | disk_read_bytes | read_bytes | io.stat rbytes | BUILD | P1 |
| Disk write bytes | disk_write_bytes | write_bytes | io.stat wbytes | BUILD | P1 |
| Disk total bytes | disk_total_bytes | read_bytes + write_bytes | Σ | BUILD | P1 |
| Disk read bytes delta | disk_read_delta | Δ | Σ | BUILD | P1 |
| Disk write bytes delta | disk_write_delta | Δ | Σ | BUILD | P1 |
| Disk total bytes delta | disk_total_delta | Δ | Σ | BUILD | P1 |
| Disk reads | disk_reads | per process "n/a" (tooltip "Linux counts storage operations per cgroup, not per process"). Process rows that are the only member of their cgroup use io.stat `rios` | io.stat rios | ADAPT | P2 |
| Disk writes | disk_writes | same with `wios` | io.stat wios | ADAPT | P2 |
| Disk reads delta | disk_reads_delta | Δrios | Δ | ADAPT | P2 |
| Disk writes delta | disk_writes_delta | Δwios | Δ | ADAPT | P2 |

- Verdict: see table
- Phase: see table
- Privilege: process rows of other uids are restricted. Unit rows use io.stat, which every user can read.

### PROC-023 Column group: network
- Source: TS [INI width id 16], L35329 | MC column network (nethogs) | review §5 "Network attribution" (binding)
- Behaviour:
  - "Network" shows TCP throughput attributed to the process, in the unit from Settings (bits or bytes, optional auto-scale). The header tooltip says "Attributed TCP traffic. UDP and other users' sockets are not counted without administrator access."
  - A socket shared by several processes counts toward each of them, and the cell gets a "shared" marker.
  - MC auto-hides this column when nethogs is missing. tasklens never auto-hides it: it shows validity glyphs instead.
  - Extra columns: `net_send_rate` "Network sent" and `net_recv_rate` "Network received".
- Linux data/mechanism:
  - sock_diag `inet_diag_req_v2` dump for AF_INET/AF_INET6 TCP with `INET_DIAG_INFO`. Δ`tcpi_bytes_acked` (sent) and Δ`tcpi_bytes_received` per socket inode.
  - inode→pid comes from readlink of `/proc/<pid>/fd/*` (`socket:[ino]`), refreshed every 2 s while the column is visible (shared with the Connections tab collector).
  - The exact path, eBPF `cgroup_skb` or kprobe byte accounting in a separate privileged collector, is the later helper (P3). The review keeps the eBPF collector separate from the action helper.
- Verdict: ADAPT
- Phase: P1 (TCP attribution); P3 (exact eBPF collector)
- Privilege: own processes need none. Other uids' fds are restricted, so those sockets count as unattributed; the header total still includes them, labelled. Exact counting: helper with CAP_BPF + CAP_PERFMON.

### PROC-024 Column group: GPU
- Source: TS L28384-28385, L28428, [CL 0.8.23] per-adapter columns | MC columns gpu, gpu_memory | review §3 (NVML only in `tl-gpu-probe`) and §5 "GPU capability model"

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| GPU | gpu | NVIDIA: `nvmlDeviceGetProcessUtilization(dev, samples, &count, lastSeenTimeStamp)` smUtil, max over this interval's samples. AMD/Intel: Δ`drm-engine-*` ns / Δt from `/proc/<pid>/fdinfo/*`, deduplicated by `drm-client-id`. Summed across GPUs (MC) | Σ | BUILD | P1 |
| GPU memory dedicated ("VRAM") | gpu_mem_dedicated | NVIDIA: `usedGpuMemory` from `nvmlDeviceGetGraphicsRunningProcesses_v3` + `nvmlDeviceGetComputeRunningProcesses_v3`, merged by pid. AMD/Intel: `drm-memory-vram` / `drm-resident-vram0` | Σ | BUILD (MC "GPU Memory" merges here) | P1 |
| GPU memory shared | gpu_mem_shared | AMD/Intel: `drm-memory-gtt` / `drm-total-system`. NVIDIA: "n/a" (unsupported) | Σ | ADAPT | P2 |
| GPU engine | gpu_engine | busiest engine label: "GPU 0 - 3D/Compute", "GPU 0 - Encode" (encUtil), "GPU 0 - Decode" (decUtil) from the same NVML samples; DRM engine name for others | most-used member engine | ADAPT | P2 |
| Per-adapter GPU columns | gpu_<n>_util, gpu_<n>_mem | the same data split per device UUID, grouped under a "GPU" header in the picker | Σ | ADAPT | P2 |

- Behaviour: a process with no NVML sample this interval shows 0% only when NVML reported the device as supporting process utilization. Otherwise it shows "n/a". The probe exposes a capability bit per device UUID.
- Verdict: see table
- Phase: see table
- Privilege: none. NVML per-process data works for all uids on Linux.

### PROC-025 Column group: power
- Source: TS L28378-28383 | review §5 "Default columns" (binding: no default power column without a real source)
- Behaviour:
  - "Power usage": estimated class "Very low", "Low", "Moderate", "High" or "Very high". The exact estimated watts appear in a tooltip. Header label "Power usage (est.)".
  - "Power usage trend": the same classes, computed from a 120 s EWMA of the estimate.
  - Both are off by default.
- Linux data/mechanism:
  - Estimate = package power × (process CPU time share of total busy CPU time) + GPU board power × (process smUtil share).
  - Package power is the RAPL Δ`/sys/class/powercap/intel-rapl:0/energy_uj` with wrap from `max_energy_range_uj`. Board power is NVML `nvmlDeviceGetPowerUsage`.
  - Class thresholds in W: <0.5 Very low, <2 Low, <8 Moderate, <25 High, else Very high.
  - Without RAPL access (no udev rule): "n/a", never a guessed class.
- Verdict: ADAPT
- Phase: P2
- Privilege: RAPL is root-only by default. Fallback: offer the same udev rule MC uses, `/etc/udev/rules.d/99-powercap.rules` making `energy_uj` mode 0444 (installed through a pkexec setup action). It already exists on the owner's machine.

### PROC-026 Column group: object counts
- Source: TS L28390-28391, L28248-28249, L35512

| Column | ID | Linux source | App/unit row | Verdict | Phase |
|---|---|---|---|---|---|
| Handles | handles | entry count of `/proc/<pid>/fd` (getdents64, no readlink). Label "Handles", tooltip "Open file descriptors" | Σ | ADAPT | P1 |
| Threads | threads | stat field 20 (num_threads); readable for all uids | Σ | BUILD | P1 |
| GDI objects | x11_resources | XRes `XResQueryClientResources` total for XWayland clients whose `XResQueryClientIds` pid matches. Native Wayland clients show "n/a" | Σ | ADAPT | P4 |
| USER objects | windows | count of top-level windows of the pid from the KWin bridge (PROC-030). Label "Windows" | Σ | ADAPT | P3 |

- Verdict: see table
- Phase: see table
- Privilege: Handles of other uids is restricted without the helper.

### PROC-027 Column group: Linux-only extras
- Source: MC inventory §8 (Linux features); TS has none of these. MC wins on Linux-only features.

| Column | ID | Linux source | Verdict | Phase |
|---|---|---|---|---|
| State | state | stat field 3: R "Running", S "Sleeping", D "Disk sleep", T "Stopped", t "Traced", Z "Zombie", I "Idle", X "Dead". Plus "Frozen" when `cgroup.events` has `frozen 1` | BUILD | P1 |
| Nice | nice | stat field 19 | BUILD | P2 |
| Scheduling policy | sched_policy | stat field 41 → Other, Batch, Idle, FIFO, RR, Deadline (plus the rt priority, field 40) | BUILD | P2 |
| I/O priority | io_priority | `ioprio_get(IOPRIO_WHO_PROCESS, pid)` → "rt/N", "be/N", "idle", "none (be/N from nice)" | BUILD | P2 |
| Unit | unit | systemd unit name from the cgroup path | BUILD | P1 |
| cgroup | cgroup | full `0::` path | BUILD | P2 |
| App ID | app_id | desktop id or Flatpak id | BUILD | P2 |
| TTY | tty | stat field 7 decoded via `/dev/pts/N` / `ttyN` | BUILD | P2 |
| SID | sid | stat field 6 | BUILD | P2 |
| OOM score | oom_score | `/proc/<pid>/oom_score`. Tooltip shows `oom_score_adj` | BUILD | P2 |

- Verdict: BUILD (extras)
- Phase: see table
- Privilege: none, except ioprio_get on some kernels for other uids. EPERM shows the lock glyph.

### PROC-028 Unmapped TaskSlinger column ids
- Source: TS INI holds 96 ids (0-95). The strings dump plus screenshots name 80 of them (the tables above). Strings under 4 characters are missing from the dump.
- Behaviour: tasklens does not reuse TS numeric ids. Its column registry is the union of PROC-015 to PROC-027. That is 80 TS columns (2 dropped, 1 merged) plus the MC Swap column and the Linux extras.
- Linux data/mechanism: none.
- Verdict: ADAPT. Numeric ids are replaced by string ids.
- Phase: P1
- Privilege: none
- Open question: what the 16 unidentified TS ids are. They are most likely per-adapter GPU columns and a Status column (covered by `gpu_<n>_*` and `state`). Recommended answer: compare against a live TaskSlinger column picker under Wine once, and add any missing column then as a P4 entry.

---

## E. Header, sorting, selection, filter, display

### PROC-029 Two-line header with system totals
- Source: TS [IMG processes.png "CPU / 11.9%"] | MC `table_view/mod.rs:852-948`
- Behaviour:
  - Line 1 is the column name with the sort arrow. Line 2 is the system total, right-aligned.
  - CPU: total CPU % from `/proc/stat`, with kernel = system+irq+softirq per the review.
  - Memory: (MemTotal − MemAvailable)/MemTotal %.
  - Swap: used %.
  - Disk: highest busy % among physical disks (MC).
  - Network: total throughput of physical interfaces as % of the summed link speed, falling back to a rate when link speed is unknown.
  - GPU: highest utilization among GPUs.
  - VRAM: used/total % over all GPUs.
  - Threads: system thread count.
  - Handles: `/proc/sys/fs/file-nr` field 1.
  - Other columns leave line 2 blank.
  - No GPU: "n/a".
- Linux data/mechanism: values come from the system collectors' snapshot at the time of the process sweep.
- Verdict: BUILD (merge: TS layout, MC formulas)
- Phase: P1
- Privilege: none

### PROC-030 KWin window bridge
- Source: TS window features L29074-29080, L29201-29213, [CL 0.8.30] | PLAN §7 "window picker needs a KWin script" | review "Unsupported feature parity" (deferred, not cut)
- Behaviour: gives tasklens a live map of pid → windows (title, resourceClass, desktopFileName, minimized, maximized, geometry, internalId) on KDE Wayland. Window features in PROC-031, PROC-061, PROC-062 and PROC-063 use it. On any other desktop those features are hidden, with the toast "Window features need KDE Plasma".
- Linux data/mechanism:
  - tasklens owns the session-bus name `org.tasklens.Tasklens` (renamed with the project name) and the object `/KWinBridge`.
  - At start it writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js` and calls `org.kde.kwin.Scripting.loadScript(path, "tasklens-bridge")`, then `org.kde.kwin.Script.run` on the returned `/Scripting/ScriptN`.
  - The script sends the full window list once, then a message on `workspace.windowAdded`, `windowRemoved`, `windowActivated` and each window's `captionChanged`, through `callDBus("org.tasklens.Tasklens", "/KWinBridge", "org.tasklens.KWinBridge", "Update", json)`.
  - Commands (activate, close, keepAbove) run as short one-shot scripts loaded the same way, which unload themselves.
  - On exit tasklens calls `unloadScript("tasklens-bridge")`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none. KWin scripting is open to session-bus clients.

### PROC-031 App row window titles and Name window button
- Source: TS [IMG adv_sheet_01 "ShareX - Image editor - 1280x800"], [CL 0.8.30]
- Behaviour:
  - App rows show the title of their most recently active window in dim text after the app name.
  - A small window button in the Name cell, shown on hover and on the selected row, lists the app's or process's windows by title. Clicking one activates it.
  - Without the bridge, there is no title and no button.
- Linux data/mechanism: the PROC-030 map, joined by pid, plus all pids of the app instance.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-032 Row hover tooltip
- Source: TS L35703, L28447, [CL 0.8.27] ("Services:", "Service Host: ")
- Behaviour: hovering the Name cell for 600 ms shows a tooltip with:
  - full command line (first 512 chars);
  - Image path;
  - "Unit: <unit> (<Description>)";
  - "cgroup: <path>";
  - for app rows: "Desktop file: <path>";
  - for restricted rows: "Some values need administrator access."
- Linux data/mechanism: data already in the snapshot.
- Verdict: ADAPT (services become systemd units)
- Phase: P1
- Privilege: none

### PROC-033 Heat colouring
- Source: TS [INI UI.ProcessValueColorCoding=1, ProcessValueColorCodingIntensityPct=50], View > Appearance "Value Color Coding" and its intensity slider | PLAN §9 heat tint #3E2A28
- Behaviour:
  - Numeric resource cells get a background tint: VALUE_HEAT × intensity × clamp(value/reference, 0, 1).
  - References: CPU 4 logical CPUs' worth (12.5% here; 100/ncpu×4); CPU (relative) 400%; Memory/RSS/PSS/USS/commit 10% of MemTotal; Disk rates 100 MB/s; I/O rates 100 MB/s; Network 10 MB/s; GPU 100%; VRAM 25% of total VRAM; Power class index/4.
  - Count columns are not tinted.
  - On by default, intensity 50%.
- Linux data/mechanism: none (UI only).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-034 Dim zero values
- Source: MC `apps-page-hide-is-zero` (0.3 opacity)
- Behaviour: a setting, "Dim zero values", off by default. Exact zeros render at 30% opacity. It never applies to validity glyphs.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-035 Column separators
- Source: MC `apps-page-show-column-separators`
- Behaviour: a setting, "Show column separators", off by default. It draws 1 px DIVIDER lines between columns.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P4
- Privilege: none

### PROC-036 Highlight new and exited processes
- Source: TS L28492, [INI UI.HighlightNewExitedProcesses=1], L30335 "process diff linger", [CL 0.9.0]
- Behaviour:
  - A toggle under View > Process View, on by default.
  - New rows get a green tint (theme success colour at 35%). Exited rows stay in place with a red tint (theme error colour at 35%) and grey text, and cannot be acted on: menu items are disabled with "(exited)".
  - Both last max(1 s, 2 refresh intervals), capped at 3 s. The tint fades out over the last 300 ms, or switches off at once when Animations is off.
- Linux data/mechanism: ProcId diff between consecutive sweeps.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-037 Sorting
- Source: TS [INI Sort.ProcessesKey=3, ProcessesAscending=1], header click | MC `table_view/mod.rs:226`, `settings.rs:55-101` | review §5 "Sort/filter execution", "Sorting expensive metrics"
- Behaviour:
  - Click a header to sort. The first click is descending for numeric columns and ascending for text. Each further click reverses.
  - Default: PID ascending.
  - Groups stay in fixed order (Apps, Background, System). Tree mode sorts siblings and keeps parents.
  - Ties break by ProcId.
  - The sort re-applies on every refresh. It always persists: MC's "Remember sorting" option is dropped, because TS always remembers.
  - Columns measured for visible rows only (PSS, USS, Cycles) cannot be sorted. Their header shows a tooltip explaining why.
- Linux data/mechanism: an engine-side projection tagged with a generation number. Results for an outdated query are discarded.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-038 Column reorder, resize and persistence
- Source: TS [INI Columns.Processes, ColumnWidths, ColumnResizeBehavior=0], L28484-28487 | MC remember-column-order
- Behaviour:
  - Drag a header to reorder it, and drag a header edge to resize.
  - "Resize Behavior: Proportional / Last Column" (default Proportional).
  - "Resize columns to fit content".
  - Order, widths and visibility persist by column ID in `settings.toml` `[processes.columns]`. MC's "Remember column order" option is dropped because it always persists.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-039 Multi-select
- Source: TS L28336, L29280-29285, [CL 0.8.4] | MC single select only (TS wins)
- Behaviour:
  - Click selects one row. Ctrl+click toggles a row. Shift+click selects a range. Shift+Up/Down extends. Ctrl+A selects all visible process rows.
  - Group header rows cannot be selected for actions.
  - Selection is a set of ProcIds and survives re-sorting.
  - Right-click on an unselected row selects only that row. Right-click inside the selection keeps it.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-040 Keyboard navigation
- Source: TS [CL 0.8.4] | MC `table_view/mod.rs:323-470`
- Behaviour:
  - Up/Down move. Right expands, or moves to the first child. Left collapses, or jumps to the parent (MC). Space toggles expand. Home/End go to the first/last row. PageUp/PageDown move.
  - Enter or Ctrl+I opens Properties.
  - Delete = End Task. Shift+Delete = Force End Task. Ctrl+E = End Task (MC). Ctrl+X = Force End Task (MC; only while the table has focus).
  - Ctrl+C copies the selected rows (PROC-058).
  - Ctrl+L collapses all (MC).
  - Ctrl+F focuses the filter.
  - Shift+F10 or the Menu key opens the context menu. MC has no handler for these; tasklens adds one.
- Linux data/mechanism: GPUI actions bound in key context "ProcessTable".
- Verdict: BUILD (merge)
- Phase: P1
- Privilege: none

### PROC-041 Filter box and type-to-filter
- Source: TS L28339, L30133, [CL 0.8.28, 0.8.32] | MC `table_view/mod.rs:604-638` | review §5 (100 ms debounce)
- Behaviour:
  - "Filter: " box at the right of the tab bar, with a clear button. It expands on focus.
  - Typing while the table has focus starts the filter, unless another input or a modal has focus. Esc clears it.
  - Matching is a case-insensitive substring over: name, image name, command line, user, description, app name, unit name, and group names ("Apps" and so on). A purely numeric query also matches PIDs by prefix.
  - Space-separated terms must all match.
  - MC's Levenshtein matching is not used, because it gives false hits.
  - Group headers show only when they contain a match.
- Linux data/mechanism: cached normalized strings per ProcId. Filtering runs in the engine projection with a 100 ms debounce.
- Verdict: BUILD (TS behaviour, wider fields)
- Phase: P1
- Privilege: none

### PROC-042 Include children
- Source: TS L28338, L28831-28832, [INI UI.FilterShowChildren=0]
- Behaviour:
  - An "Include children" checkbox, visible while filtering. Off by default.
  - When on, all descendants of matching rows also show.
  - Toasts: "Including filtered process children." and "Hiding unmatched filtered process children."
- Linux data/mechanism: ppid tree.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-043 Hold Ctrl to pause
- Source: TS L28337, [CL 0.8.10] | MC `window.rs:863-907` | review §5 "Ctrl pause" (binding)
- Behaviour:
  - While Ctrl is held, the displayed rows, order and values freeze, and the status bar shows "[PAUSED - Ctrl]".
  - Collection and history keep running.
  - Releasing Ctrl or losing window focus resumes, applying the latest snapshot.
  - Actions taken while paused use the captured ProcIds and revalidate them.
- Linux data/mechanism: the UI holds one snapshot Arc. The review bounds this to one retained paused snapshot.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-044 Status bar text (Processes tab)
- Source: TS L28335-28337, L30142
- Behaviour, at bottom left:
  - "%zu Processes";
  - while filtering: "%zu / %zu Processes";
  - when 2 or more rows are selected: "%d processes selected";
  - "[PAUSED - Ctrl]" appended while paused;
  - " | %d restricted" appended when some rows lack permission, with tooltip "Click the shield for administrator access".
  - The count includes kernel threads only when they are shown.
- Linux data/mechanism: from the projection.
- Verdict: BUILD (plus the restricted count)
- Phase: P1
- Privilege: none

### PROC-045 Collapse All
- Source: MC Collapse All button / Ctrl+L
- Behaviour: View > Process View > "Collapse all" (Ctrl+L) collapses every expanded row and group except the three group headers.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-046 Startup placeholder
- Source: MC `window.rs:1150-1239` (shimmer)
- Behaviour: until the first two sweeps finish, rate columns show "…" and the table shows rows from the first sweep. There is no shimmer, because TS has none and density wins.
- Linux data/mechanism: validity state "warming up".
- Verdict: ADAPT
- Phase: P1
- Privilege: none

---

## F. Context menus

### PROC-047 Header context menu
- Source: TS L29313-29314, L28434, L28484, L28648, L28652
- Behaviour:
  - Items: "Select Columns...", "Hide This Column", "Resize columns to fit content", "Reset to default".
  - Toasts: "At least one column must stay visible" and "Columns reset to default".
  - Name cannot be hidden.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-048 Column picker
- Source: TS View > Columns, `header_column.filter` L29888
- Behaviour:
  - A popover list of all columns with checkboxes and a filter field at the top.
  - Columns are grouped under headings: Identity, File, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux.
  - Restricted or unsupported columns are still listed, with a note such as "n/a on this system".
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-049 Row context menu
- Source: TS [IMG adv_sheet_01/02], L29277-29306 | MC `table_view.blp:121`
- Behaviour, in this order (separators shown as "|"). Group header rows get only "Expand/Collapse". Exited rows disable everything except Copy.
  1. End Task. Multi-select label: "End Selected Tasks". App rows: "End App".
  2. Force End Task (SIGKILL). App rows: "Force End App".
  3. End Process Tree.
  4. Suspend Process / Resume Process. Multi-select: "Suspend Selected Tasks" / "Resume Selected Tasks". App rows: "Suspend App" / "Resume App" (freeze).
  5. Send Signal >
  |
  6. Set Priority >
  7. Set Affinity >
  8. Set I/O Priority >
  |
  9. Show Window (only when the bridge has a window)
  10. Go to parent process
  11. Go to service (only when the process is in a systemd unit)
  12. Restart Plasma Shell (only on plasmashell) / Restart unit "<unit>" (only on a unit's MainPID)
  |
  13. Copy
  14. Copy all metrics
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
- Linux data/mechanism: the menu captures ProcIds when it opens (PROC-001). Built in the table delegate's `context_menu` hook. Every item dispatches the same GPUI Action as its key binding (gpui-component rule 10).
- Verdict: ADAPT (TS order, plus MC's signal submenu, plus Linux items)
- Phase: P1 (menu shell and P1 actions); later items arrive in their own phases
- Privilege: per item

---

## G. Process actions

### PROC-050 End Task
- Source: TS L29277-29280, L28777-28781 | MC Stop (SIGTERM) | review §5 "Process action semantics" (binding: End = SIGTERM)
- Behaviour:
  - Process row: SIGTERM to that process.
  - App row ("End App"): SIGTERM to each primary pid of the app (pids whose parent is outside the app), with bwrap unwrapped (PROC-013).
  - Multi-select: applied to each target, and one toast reports results, e.g. "Ended 3 of 4 processes. 1 failed: permission denied".
  - Toasts: "End task requested." and "Could not end task."
  - No confirmation for a single target. A confirmation dialog appears for 2 or more targets, or for any session-critical target (PROC-055).
- Linux data/mechanism: `pidfd_open` + identity check + `pidfd_send_signal(fd, SIGTERM, NULL, 0)`. Results come back on the action result channel, never through the snapshot (review §2).
- Verdict: ADAPT (TerminateProcess becomes SIGTERM)
- Phase: P1
- Privilege: own processes need none. EPERM gives the toast "Permission denied" with a "Retry as administrator" button → helper `signal(ProcId, SIGTERM)`.

### PROC-051 Force End Task
- Source: MC Force Stop (SIGKILL), Ctrl+X | TS End Task is forceful on Windows
- Behaviour:
  - Process row: SIGKILL.
  - App row: kills the whole app unit, with toast "App force-ended".
  - Toasts: "Process terminated" and "Could not end task."
- Linux data/mechanism:
  - Process: pidfd SIGKILL.
  - App or unit row: `org.freedesktop.systemd1.Manager.KillUnit(unit, "all", SIGKILL)` on the user manager, with the unit validated to be under our `user@<uid>.service`. If the cgroup is not a unit, write "1" to `cgroup.kill` after checking the directory is owned by our uid.
- Verdict: BUILD
- Phase: P1
- Privilege: as PROC-050. System units go through the system manager `KillUnit`, which triggers polkit `org.freedesktop.systemd1.manage-units`.

### PROC-052 End Process Tree
- Source: TS L29279, L28645-28766 "Process tree terminated", "End process tree requested."
- Behaviour:
  - Always shows a confirmation dialog: "End <name> and its N child processes?" with buttons End tree / Cancel.
  - Targets are captured at confirmation time, deepest first.
  - SIGTERM is sent to each, and the result is reported per target in one toast.
- Linux data/mechanism: walk `/proc/<pid>/task/*/children` to build the captured ProcId list. A pidfd for each target is opened before any signal is sent.
- Verdict: ADAPT (SIGTERM per target)
- Phase: P1
- Privilege: as PROC-050

### PROC-053 Suspend / Resume process
- Source: TS L29281-29285, toasts "Process suspended." / "Process resumed." | MC Suspend (STOP) / Continue (CONT)
- Behaviour:
  - "Suspend Process" sends SIGSTOP. "Resume Process" sends SIGCONT.
  - The item label follows the State column: a row in State T shows Resume.
  - Multi-select variants exist.
- Linux data/mechanism: pidfd SIGSTOP / SIGCONT.
- Verdict: ADAPT
- Phase: P1
- Privilege: as PROC-050

### PROC-054 Suspend / Resume app (cgroup freeze)
- Source: PLAN §3 "freeze and kill without root" | review §5 (validated scope ownership)
- Behaviour: on App and unit rows, "Suspend App" freezes the whole cgroup and "Resume App" thaws it. State shows "Frozen". Toasts: "App suspended." and "App resumed."
- Linux data/mechanism:
  - `org.freedesktop.systemd1.Manager.FreezeUnit(unit)` / `ThawUnit(unit)` on the user manager (systemd 246 or later).
  - Fallback for a cgroup that is not a unit: write "1"/"0" to `cgroup.freeze` after checking our uid owns it. Completion is confirmed by `cgroup.events` `frozen 1`.
- Verdict: ADAPT
- Phase: P2
- Privilege: user units need none. System units go through polkit `org.freedesktop.systemd1.manage-units`.

### PROC-055 Session-critical and self protection
- Source: TS L28779-28781 ("Refusing to terminate the current TaskSlinger process.", "Windows marked the selected process as critical...")
- Behaviour:
  - Refused outright, with a toast:
    - tasklens itself: "Refusing to end tasklens itself."
    - pid 1 and kernel threads: "This process cannot be ended."
  - Confirmation dialog "Ending <name> will end or break your desktop session. End it anyway?" for:
    - `systemd --user` (pid of `init.scope`);
    - the Wayland compositor;
    - the session D-Bus broker;
    - Xwayland;
    - `sddm`/`login` ancestors of the session.
- Linux data/mechanism: the compositor pid comes from `SO_PEERCRED` on a connection to `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`. The bus broker pid comes from `GetConnectionUnixProcessID("org.freedesktop.DBus")`. Xwayland is found by exe basename.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-056 Send Signal submenu (all POSIX signals)
- Source: MC `table_view.blp:121` (8 signals), `PL:processes/manager.rs:29-110`
- Behaviour:
  - Top level: Suspend (STOP), Continue (CONT), Hangup (HUP), Interrupt (INT), Terminate (TERM), Kill (KILL), User 1 (USR1), User 2 (USR2).
  - "Other >" lists QUIT, ABRT, ALRM, PIPE, CHLD, TSTP, TTIN, TTOU, URG, XCPU, XFSZ, VTALRM, PROF, WINCH, IO, PWR, SYS, TRAP, BUS, FPE, ILL, SEGV, STKFLT, each with its number.
  - "Real-time signal..." opens a number field limited to SIGRTMIN..SIGRTMAX (34-64).
  - MC retries automatically with `pkexec kill`. tasklens does not, per the review: the toast offers the retry.
- Linux data/mechanism: `pidfd_send_signal`. App rows send to the primary pids.
- Verdict: BUILD
- Phase: P1
- Privilege: as PROC-050

### PROC-057 Set Priority
- Source: TS L28455-28458, [IMG adv_sheet_02], toast "Priority updated"
- Behaviour:
  - Submenu: Realtime, High, Above normal, Normal, Below normal, Low, then "Custom nice..." (a spin field from -20 to 19). The current class has a check.
  - Value mapping: Realtime = SCHED_RR priority 1; High = nice -10; Above normal = -5; Normal = 0; Below normal = 5; Low = 19.
  - "Scheduling policy >" offers Other, Batch and Idle. FIFO, RR and Deadline are display-only, except Realtime above.
  - App rows apply to every current process in the app. The tooltip says "Applies to the app's current processes".
- Linux data/mechanism:
  - Linux nice is per thread, so `setpriority(PRIO_PROCESS, tid, n)` is called for every tid in `/proc/<pid>/task`.
  - `sched_setscheduler` per tid for policy changes. Revalidate the ProcId first; best-effort semantics (review).
  - With `sched_autogroup_enabled=1`, nice only orders threads within one autogroup. Properties > General shows the autogroup from `/proc/<pid>/autogroup`.
- Verdict: ADAPT
- Phase: P2
- Privilege: raising nice on own processes needs none. Lowering nice or Realtime needs CAP_SYS_NICE: helper `set_priority(ProcId, policy, nice|rtprio)`. The unprivileged fallback disables those items, with the tooltip "Needs administrator" plus the retry button.

### PROC-058 Copy and Copy all metrics
- Source: TS "Copy" (clicked cell), "Copy all metrics" L35504-35513, [CL 0.8.4], toasts "Copied to clipboard", "Copied metrics to clipboard", "Failed to copy to clipboard"
- Behaviour:
  - **Copy** copies the right-clicked cell's displayed text.
  - **Copy all metrics** copies, for each selected row, "Label: value" lines. TS's fixed order comes first: Name, PID, Parent PID, CPU (`%.2f%%`), Memory, Private bytes, Working set (MiB), Threads, Handles, Priority. Then every other column in registry order. Rows are separated by a blank line.
  - **Ctrl+C** copies the selected rows as TSV with a header line of the visible columns.
  - Environment values are never included.
- Linux data/mechanism: GPUI clipboard (`cx.write_to_clipboard`) through Wayland `wl_data_device`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-059 Set Affinity
- Source: TS L28460, L28649, [CL 0.8.31] P/E-core icons
- Behaviour:
  - Submenu: "All processors", then one checkable entry per logical CPU ("CPU 0" to "CPU 31").
  - Entries are grouped under headings per L3 domain on AMD ("CCD 0 (L3 32 MiB)") and per core type on Intel hybrid, with P-core and E-core icons. SMT siblings are listed next to each other.
  - "Custom..." opens a dialog with a CPU grid and a list field (e.g. "0-7,16-23").
  - Toasts: "Affinity updated" and "At least one CPU must stay enabled".
- Linux data/mechanism:
  - `sched_setaffinity(tid, mask)` for every tid.
  - L3 groups from `/sys/devices/system/cpu/cpu*/cache/index3/shared_cpu_list`. Core types from `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus` when present (absent on this AMD CPU). Siblings from `topology/thread_siblings_list`.
  - Best-effort revalidation.
- Verdict: ADAPT
- Phase: P2
- Privilege: own processes need none. Other uids need CAP_SYS_NICE: helper `set_affinity`.

### PROC-060 Set I/O Priority
- Source: none in either tool (Linux-only gap listed in MC inventory §3 "Missing: I/O priority")
- Behaviour: submenu: Realtime (rt/4), High (be/0), Normal (be/4), Low (be/7), Idle, "Default (follows nice)". The current value has a check.
- Linux data/mechanism: `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_PRIO_VALUE(class, level))` for every tid. It only has effect with the BFQ or mq-deadline scheduler; the tooltip shows the active scheduler from `/sys/block/<dev>/queue/scheduler`.
- Verdict: BUILD
- Phase: P2
- Privilege: Realtime class needs CAP_SYS_ADMIN, and other uids need it too: helper `set_ioprio`.

### PROC-061 Show Window
- Source: TS L29289, toasts "Process window shown.", "No window found"
- Behaviour: brings the process's (or app's) most recently active window to the front and un-minimizes it. Hidden when the bridge has no window for those pids.
- Linux data/mechanism: a one-shot KWin script: `w.minimized = false; workspace.activeWindow = w;` (PROC-030).
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-062 Window picker
- Source: TS `chrome.window_picker` L28333, L30131, L29201-29213, [CL 0.8.4]
- Behaviour:
  - Clicking the toolbar "+" shows the toast "Click a window to select its process". KWin shows its crosshair. After a click, tasklens selects that window's process and scrolls to it, clearing the filter if needed. Esc cancels.
  - Ctrl+click on "+" arms terminate mode instead. The toast reads "Click a window to end its process". The picked process gets End Task (SIGTERM), and the result toast has a "Force end" button.
  - TS reads Ctrl at the moment of the click on the target window. Wayland gives tasklens no global key state, so the Ctrl choice moves to the arming click.
- Linux data/mechanism: `org.kde.KWin.queryWindowInfo()` returns the picked window's map including `uuid`. The bridge map turns uuid (internalId) into pid.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-063 Pin windows on top (properties windows)
- Source: TS L30218-30221 pin, [CL 0.8.25]
- Behaviour: the pin button on Process Properties windows and dialogs keeps them above other windows.
- Linux data/mechanism: a one-shot KWin script sets `keepAbove = true` on the window matched by tasklens's pid and title. Wayland has no client protocol for this.
- Verdict: ADAPT
- Phase: P2
- Privilege: none. Without KWin the pin button is hidden.

### PROC-064 Go to parent process
- Source: TS toasts "Switched to parent process", "Parent process not found in current view" L28645-28766 | General tab PPID link
- Behaviour: selects and scrolls to the parent row. If the parent is hidden by the filter or by Hide system processes, the toast "Parent process not found in current view" offers a "Show" button that clears the filter and unhides system processes.
- Linux data/mechanism: ppid → ProcId lookup.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-065 Go to service
- Source: TS toasts "Switched to Services tab", "Switched to service." L28710, L28830
- Behaviour: for processes inside a systemd unit, this switches to the Services tab and selects that unit.
- Linux data/mechanism: the unit name from the cgroup path, plus user or system manager from the path prefix.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-066 Restart Explorer analogue
- Source: TS L29283, L29288, L35562-35572, toasts "Restarting Explorer...", "Explorer restarted", "Explorer restart is already in progress."
- Behaviour:
  - On the plasmashell row: "Restart Plasma Shell". Toasts: "Restarting Plasma Shell...", "Plasma Shell restarted", "Plasma Shell restart is already in progress."
  - On any process that is a unit's MainPID: "Restart unit <name>".
  - No item for kwin_wayland or gnome-shell on Wayland, because restarting them ends the session.
- Linux data/mechanism: `org.freedesktop.systemd1.Manager.RestartUnit("plasma-plasmashell.service", "replace")` on the user manager. Success means the job's `JobRemoved` signal reported result "done". MainPID comes from the unit property `MainPID`.
- Verdict: ADAPT
- Phase: P2
- Privilege: user units need none. System units go through polkit `manage-units`.

### PROC-067 Open file location
- Source: TS L35502, toast "Opening file location"
- Behaviour: opens the file manager with the exe selected. Disabled with the tooltip "File was deleted" when the exe is deleted.
- Linux data/mechanism: `org.freedesktop.FileManager1.ShowItems(["file://<exe>"], "")` on the session bus (Dolphin implements it). If no service owns that name, fall back to `xdg-open <dir>`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none. Restricted exe paths use the argv fallback path.

### PROC-068 File properties
- Source: TS L29316 (shell file properties dialog)
- Behaviour: opens the file manager's properties dialog for the exe.
- Linux data/mechanism: `org.freedesktop.FileManager1.ShowItemProperties(["file://<exe>"], "")`. Hidden if the service lacks that method.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-069 Open in VirusTotal
- Source: TS L35503, toasts "SHA-256 unavailable", "Process image unavailable"
- Behaviour: computes the exe's SHA-256 locally and opens `https://www.virustotal.com/gui/file/<sha256>` in the default browser. Nothing is uploaded. The first use shows a one-time notice: "This opens virustotal.com with the file's hash. The file is not uploaded."
- Linux data/mechanism: read `/proc/<pid>/exe` (this works even when the file is deleted), streamed SHA-256 on the blocking pool. Then `org.freedesktop.portal.OpenURI.OpenURI`, falling back to `xdg-open`.
- Verdict: BUILD
- Phase: P2
- Privilege: other uids' exe is restricted; it uses the path from argv when that file is readable, and otherwise shows the toast "Process image unavailable".

### PROC-070 Create dump file
- Source: TS [IMG adv_sheet_02] `<name>_<pid>_<yyyymmdd_hhmmss>.dmp`, "Dump created: %ls", [INI Crash.DumpType Mini/Full], L34881-34884
- Behaviour:
  - Writes `<name>_<pid>_<yyyymmdd_hhmmss>.core` to `$XDG_STATE_HOME/tasklens/dumps/`.
  - Toast "Dump created: <path>" with an "Open folder" button.
  - Setting "Dump type: Mini / Full", default Mini.
  - The process is paused while the dump is written. The toast warns of this before large dumps (VmRSS over 2 GiB).
- Linux data/mechanism:
  - `gcore -o <path-stem> <pid>`. gdb is an optional dependency; without it the item is disabled with the tooltip "Install gdb".
  - Mini uses the process's own `/proc/<pid>/coredump_filter`. Full sets `coredump_filter=0x1ff` for the duration.
  - Yama scope 1 blocks ptrace of non-children, so it runs through helper `dump(ProcId, mode, out_fd)`. The helper runs gcore as root and writes to a file descriptor tasklens opened, so the file is owned by the user.
  - With ptrace_scope 0 it runs directly.
- Verdict: ADAPT
- Phase: P3
- Privilege: CAP_SYS_PTRACE via pkexec helper. Fallback: the item is disabled with the tooltip "Needs administrator (ptrace is restricted on this system)".

---

## H. Process Properties window

### PROC-071 Process Properties window shell
- Source: TS L28990, L30218-30237, L28713, L29889-29894, [CL 0.8.4]
- Behaviour:
  - A separate top-level window titled "Process Properties - <name> - PID <pid>". It is resizable and pinnable (PROC-063), and has its own title bar with pin and close.
  - Up to 8 can be open. A ninth gives the toast "Too many process properties windows are already open".
  - Opening Properties again for the same ProcId focuses the existing window.
  - Contents refresh at the process refresh rate.
  - Tabs: "General", "Threads [n]", "File descriptors [n]", "Memory maps [n]", "Security", "Environment [n]", "Strings".
  - Each tab has its own filter field with a clear button.
  - When the process exits, the title shows "PID <pid> (not running)", values freeze and actions disable.
- Linux data/mechanism: GPUI `open_window`. The engine is told this window demands the ProcId and the active tab's data.
- Verdict: ADAPT (tab renames listed above)
- Phase: P2
- Privilege: none for the shell

### PROC-072 General tab
- Source: TS L29038-29081, L29097-29103, L29175-29198, L35680-35681, L29094-29096, L29127, L29147
- Behaviour: three key/value columns. Clicking a value copies it (toast "Process property value copied."). The empty filter state reads "No properties match the current filter."
  - **Column 1:**
    - Process, PID, Started;
    - PPID (link → PROC-064);
    - User (shows uid and euid when they differ);
    - Priority ("Nice 0, Other", plus autogroup "autogroup-N nice 0");
    - Hardening (replaces Mitigations): PIE, NX stack, RELRO (full/partial/none), Shadow stack (x86_Thread_features), ASLR (system setting + `ADDR_NO_RANDOMIZE` personality);
    - Architecture;
    - Executable (link → PROC-067);
    - Location category: Temp directory (/tmp, /var/tmp), User Downloads, User data (~/.local), User profile (~), System (/usr), Optional (/opt), Flatpak, AppImage (/tmp/.mount_*), Wine prefix (drive_c), Deleted;
    - Company (Package), Description, Cmd line, Cwd, Root (when it is not "/"), Unit, cgroup, App ID.
  - **Column 2:**
    - SHA-256, MD5 (computed on open, async; errors "Failed to compute one or more hashes", "Executable path unavailable");
    - Signature (Package status), Signer (Packager);
    - Threads, Handles (fds);
    - RSS, PSS, USS, Swap, Commit, Memory %;
    - CPU %, CPU time;
    - I/O read bytes, I/O write bytes, Read ops, Write ops, Disk read bytes.
  - **Column 3:**
    - Disk write bytes, Cancelled write bytes, Disk total rate, Disk read rate, Disk write rate;
    - Net I/O (attributed TCP), GPU %, GPU memory, GPU engine;
    - Window title, Window id (KWin internalId), Window state (Minimized/Maximized/Normal), Window class (resourceClass), Window rect ("L:%ld T:%ld R:%ld B:%ld"), Window size ("%ld x %ld"), "And %llu more" when there are several windows, "No top-level window" when there are none.
- Linux data/mechanism:
  - ELF program headers via `/proc/<pid>/exe`: `PT_GNU_STACK` flags, `PT_GNU_RELRO` plus `DT_FLAGS BIND_NOW`, `e_type ET_DYN` for PIE.
  - status `x86_Thread_features:`; `/proc/sys/kernel/randomize_va_space`; `/proc/<pid>/personality`; `readlink cwd` and `root`.
  - Everything else is the same as the columns.
- Verdict: ADAPT (Windows mitigations become ELF hardening; HWND becomes the KWin id)
- Phase: P2
- Privilege: exe, cwd, root and personality for other uids are restricted without the helper; those fields show the lock glyph.

### PROC-073 Threads tab
- Source: TS L29021-29025, L35489, L35667
- Behaviour:
  - Columns: Thread ID, Name, State, CPU %, User Time (ms), Kernel Time (ms), Priority (nice), Policy, Affinity, Last CPU, Context switches (vol/invol), Wait channel.
  - States: "Loading threads...", "No threads found.", "No threads match the current filter."
  - Row menu: Copy, Set Priority >, Set Affinity >, Send signal to thread > (TERM, KILL, USR1, USR2 via `tgkill`). The tooltip warns that a fatal signal ends the whole process.
  - TS's Start Address and Start Module are replaced by Wait channel and Name.
- Linux data/mechanism:
  - `/proc/<pid>/task/<tid>/stat`: comm (field 2), state, utime, stime, nice, policy, processor (field 39).
  - `task/<tid>/status`: `Cpus_allowed_list` and ctxt switches.
  - `task/<tid>/wchan`.
  - Per-thread `setpriority` / `sched_setaffinity`.
  - A thread start address would need ptrace plus unwinding, so it is not collected.
- Verdict: ADAPT
- Phase: P2
- Privilege: wchan for other uids is restricted. Changing other uids' threads goes through the helper.

### PROC-074 File descriptors tab (Handles)
- Source: TS L29026-29027, L29154-29174, L28988, L35666, [INI UI.HideUnnamedHandles=1]
- Behaviour:
  - Columns: FD, Type, Name, Access, Position.
  - Type values: File, Directory, Socket (TCP/UDP/Unix/Netlink/other), Pipe, FIFO, Device, eventfd, epoll, timerfd, signalfd, inotify, fanotify, memfd, dmabuf, DRM, pidfd, anon_inode:<kind>.
  - Name:
    - path;
    - sockets are resolved through the shared socket inventory to "TCP 127.0.0.1:5000 → 10.0.0.2:443 ESTABLISHED" or the Unix path;
    - pipes become "pipe:[ino] → <peer process> (PID n)" by matching the inode across processes;
    - nameless entries show "(unnamed)".
  - Access is decoded from fdinfo `flags` (octal): access mode (Read/Write/Read-write), status flags (Append, Non-blocking, Sync, Direct, Path, Temp file), descriptor flag Close-on-exec. "Raw flags: 0o..." is shown at the top of the tooltip.
  - Filter checkbox "Hide unnamed handles", on by default: it hides `anon_inode:*` entries.
  - States: "Loading handles...", "No handles reported.", "Handle detail query unavailable."
  - Row menu: Copy, Open file location (files), Go to connection (sockets → Connections tab).
- Linux data/mechanism: readdir and readlink of `/proc/<pid>/fd/*`; `/proc/<pid>/fdinfo/<fd>` (`pos`, `flags`, `mnt_id`, plus `drm-*` and `eventfd-count`).
- Verdict: ADAPT
- Phase: P2
- Privilege: same uid needs none. Other uids: helper `read_proc_files(fd-list, fdinfo)`, otherwise "Handle detail query unavailable. Needs administrator."

### PROC-075 Memory maps tab (Modules)
- Source: TS L29028, L29112, L29215-29216, L29020, [CL 0.8.4], [INI UI.ModulesDllOnly=1]
- Behaviour:
  - A tree of backing files. "(Primary)" is the main executable and comes first. Then shared libraries, other mapped files and the "Outside loaded modules" node, which holds `[heap]`, `[stack]`, `[vdso]`, `[vvar]`, anonymous and memfd mappings. Each file expands to its regions.
  - Columns: Name, Base Address (lowest start), Size (sum of region sizes), RSS (sum from `smaps`, loaded when the tab opens), Permissions (e.g. "r-xp"), Offset, Path, Description (package `%DESC%`), Company (package name).
  - Filter "Shared libraries only", on by default. It shows ELF `.so` files, plus `.dll`, `.exe` and `.drv` files for Wine processes.
- Linux data/mechanism: `/proc/<pid>/maps`; `/proc/<pid>/smaps` for RSS (on open, then every 5 s); the package index from PROC-016.
- Verdict: ADAPT
- Phase: P2
- Privilege: same uid needs none. Other uids go through the helper, otherwise the tab shows "Needs administrator".

### PROC-076 Security tab (Token)
- Source: TS L29006-29013, L29082-29093, L29104-29119, L30247-30257, L29152
- Behaviour:
  - **Summary:**
    - "Risky capabilities": effective caps in {SYS_ADMIN, SYS_PTRACE, SYS_MODULE, SYS_RAWIO, DAC_OVERRIDE, DAC_READ_SEARCH, SETUID, SETGID, NET_ADMIN, BPF, PERFMON, SYS_BOOT}.
    - "Admin groups": membership in wheel, sudo, root, adm, docker, libvirt or disk. docker and disk are marked "root-equivalent".
    - Flags: NoNewPrivs, Seccomp mode and filter count, Speculation_Store_Bypass, SpeculationIndirectBranch.
    - User and UID; real, effective, saved and fs uid/gid.
    - Security label (LSM).
    - Session (audit sessionid, loginuid).
    - Elevated ("No", "Yes (root)", "Yes (capabilities)", "Setuid").
    - Namespaces: user, pid, net, mnt, ipc, uts, cgroup, time; each "Shared with init" or "Isolated (inode)".
  - **"Capabilities [n]" table:** Name, Effective, Permitted, Inheritable, Bounding, Ambient, Description. It replaces TS's Privileges table.
  - **"Groups [n]" table:** Name, GID, Status ("Primary" / "Supplementary").
  - Error: "Security information unavailable (insufficient privileges)".
  - Copy toast: "Could not copy token summary value."
- Linux data/mechanism:
  - `/proc/<pid>/status` `Uid`, `Gid`, `Groups`, `CapInh`/`CapPrm`/`CapEff`/`CapBnd`/`CapAmb` (hex, decoded against the `linux/capability.h` names), `NoNewPrivs`, `Seccomp`, `Seccomp_filters`, `Speculation_*`.
  - `/proc/<pid>/attr/current`, `/proc/<pid>/loginuid`, `/proc/<pid>/sessionid`.
  - readlink `/proc/<pid>/ns/*` compared with `/proc/1/ns/*`. Reading pid 1's namespaces is restricted, so the comparison uses `/proc/self/ns/*` and the label becomes "Same as tasklens".
- Verdict: ADAPT (Windows tokens become credentials, capabilities and namespaces)
- Phase: P2
- Privilege: status is readable by all uids. `ns/*` of other uids is restricted and shows the lock glyph.

### PROC-077 Environment tab
- Source: TS [IMG adv_sheet_01], L29148-29151
- Behaviour:
  - Columns: Name (sortable), Value.
  - A note at the top: "The environment the process started with."
  - Values of names matching `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*PASSWD*`, `*KEY*`, `*CREDENTIAL*` or `*COOKIE*` (case-insensitive) are masked as "••••••". Clicking reveals one value. This mask is a safety decision from review §6.7.
  - A Refresh button reloads the list.
  - Copy works on single values, and only from the context menu.
  - Errors: "Environment unavailable (insufficient privileges)", "(read failed)", "Environment block unavailable" (zombie or kernel thread).
- Linux data/mechanism: `/proc/<pid>/environ` (NUL-separated, bounded to 1 MiB), read on open and on Refresh. Never logged.
- Verdict: BUILD
- Phase: P2
- Privilege: same uid needs none. Other uids go through helper `read_proc_files(environ)`.

### PROC-078 Strings tab
- Source: TS L29005, L29014, L35669-35671, L29846-29850
- Behaviour:
  - A "Scan" button starts the scan. Options: minimum length (4 to 64, default 4); encodings ASCII, UTF-8 and UTF-16LE (all on by default; UTF-16 matters for Wine); "Include file-backed mappings" (off by default).
  - Progress text: "Scanning process memory... %ld%%  %ld strings found".
  - Columns: Address (hex), Encoding, Length, Value.
  - Results stop at 1,000,000 strings or 256 MiB of text, with "String scan stopped: not enough memory."
  - Error: "Failed to read process memory."
  - A Cancel button stops the scan.
- Linux data/mechanism:
  - Regions come from `/proc/<pid>/maps`: readable, not `[vvar]`, not device mappings, and anonymous only unless the option is on.
  - Reads use `process_vm_readv` in 1 MiB chunks.
  - With Yama `ptrace_scope=1`, this needs CAP_SYS_PTRACE even for own processes. The scan therefore runs inside the helper, `scan_strings(ProcId, opts)`, which streams result batches.
  - With ptrace_scope 0 and the same uid, it runs in-process.
- Verdict: ADAPT
- Phase: P3
- Privilege: pkexec helper (CAP_SYS_PTRACE). Fallback: the tab shows "Reading another process's memory needs administrator on this system (ptrace_scope=1)" and a "Grant access" button.

### PROC-079 App Properties window (replaces the MC Details dialog)
- Source: MC `process_details_dialog.rs`, `process_details_dialog.blp:24-163`, Ctrl+I
- Behaviour:
  - Properties (Enter or Ctrl+I) on an App or unit row opens "App Properties - <name>".
  - **General:** icon and name, App ID, Type ("App" or "Unit"), Desktop file, Unit, cgroup, Process count, Command line of the primary process, and live Resource Usage rows: CPU, Memory, Swap, Drives, Network, GPU, GPU Memory.
  - **Processes [n]:** a mini table of member processes. Double-click opens that process's Properties.
  - MC's dialog for a process row is merged into Process Properties > General, which is a superset.
  - MC's Swap binding bug is not copied.
- Linux data/mechanism: the cgroup aggregates from PROC-020 and PROC-022, plus the snapshot.
- Verdict: ADAPT (merge)
- Phase: P2
- Privilege: none

---

## I. History mode

### PROC-080 History recording engine
- Source: TS [INI UI.HistoryStorageDurationHours=6, HistoryPersistToDisk=1], L28515-28516, [CL 0.8.9] | review §3 "History policy" and §6.5 (binding: off by default, separate storage design)
- Behaviour:
  - Settings > Monitoring > "Record process history". **Off by default** (review), unlike TS, where it is on.
  - "History storage (hours)" ranges from 1 to 12, default 6.
  - While recording is on and the window is hidden, the process sweep keeps running at 1 Hz (review hidden policy).
  - Toast "History recording is off" when History mode is opened with recording off. The view then offers an "Enable" button.
- Linux data/mechanism:
  - The engine owns the store.
  - Each 1 s frame stores one record per process that is active in that frame (cpu > 0, or I/O, net or GPU > 0, or memory changed by ≥ 1 MiB). Idle processes carry their last memory value forward and store nothing.
  - Record fields: identity index u32 into a ProcId+name+user+path dictionary; cpu u16 (per-mille of the machine); RSS u32 (4 KiB pages); disk u32 (KiB/s); net u32 (B/s); gpu u16 (per-mille).
  - The last 10 minutes stay uncompressed in RAM. Older data is packed into 60 s chunks, zstd level 3 (`zstd` crate).
  - RAM is capped at 64 MiB. When the cap is reached, the oldest chunks drop first and a gap is shown.
  - Gaps are recorded for suspend/resume and engine restarts.
  - Estimate: about 100 active processes/s × 16 B × 21,600 s ≈ 35 MB raw for 6 h, about 12 MB compressed. This is an estimate, to be measured in P3.
- Verdict: ADAPT (default off; storage redesigned)
- Phase: P3
- Privilege: none

### PROC-081 History persistence
- Source: TS L34862, L30348, L35751-35760, L30356-30362
- Behaviour:
  - Setting "Save history to disk", on by default once recording is enabled.
  - On launch, history loads in the background.
  - Toasts: "Could not open process history store.", "Process history store is empty or truncated.", "Process history store version/layout mismatch.", "No in-range process history in store.", "Process history store is stale.", "Process history load timed out; starting fresh." (5 s budget).
- Linux data/mechanism:
  - Directory `$XDG_STATE_HOME/tasklens/history/`. One file per 10 minutes: `YYYYmmdd-HHMM.tlh`, with header `TLH1` + version u16 + boot_id + layout hash, followed by zstd chunks.
  - Files are written via temp file + `rename`, and files older than the retention period are deleted.
  - A boot_id change marks earlier identities as "previous boot".
- Verdict: ADAPT (XPRESS `store.tshp` becomes zstd chunk files)
- Phase: P3
- Privilege: none

### PROC-082 History view and table
- Source: TS [INI UI.ProcessesHistoryMode, HistoryVisibilityMask=1FF, Sort.HistoryKey=6], `chrome.tab.processes.history`, L35690-35692, L35741-35742, [CL 0.8.27]
- Behaviour:
  - The history icon on the Processes tab switches the tab to History mode (state persists in `processes.history_mode`).
  - Layout: header text, the graph with scrubber, then a table.
  - Header text:
    - "History - Drag scrubber below to view timeline" (live);
    - "History Snapshot: %llu processes - %llu min %llu sec ago" (point selected);
    - "Live".
  - Table columns (9): Name, PID, User, Image Path, CPU, Memory, Disk, Network, GPU. They show values at the scrub point, or averages over a selected range, with the peak in a tooltip.
  - Default sort: CPU descending.
  - Clicking a row selects that process: the graph shows its series and the chip "history.process.clear" clears it. The table shows "Process not running" when the process has exited.
  - "History entry deleted" toast comes from the row menu "Delete from history".
  - The global filter applies to the table and the graph [CL 0.8.27].
- Linux data/mechanism: bounded history views published by the engine (review §2).
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-083 History scrubber
- Source: TS `history.scrubber`, [CL 0.8.18], L35725-35731, `--history-scrub-bench`
- Behaviour:
  - Drag to scrub. Wheel zooms around the cursor. Shift+wheel or middle-drag pans. Shift+drag selects a range.
  - A minimap strip below shows the full retention with a viewport box.
  - Time labels: "-%lld:%02lld:%02lld", "%lldh %02lldm", "%lldm %02llds", "%llds".
  - A "Live" button returns to live.
  - Metric chips (CPU, Memory, Disk, Network, GPU) choose the graphed metric, and legend toggles show or hide series.
- Linux data/mechanism: the app-owned chart element; level-of-detail by min/max per pixel column from the chunk store.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-084 History top-3 hover
- Source: TS [CL 0.8.19], L35743-35749
- Behaviour: hovering the graph shows the time and the top 3 processes for the current metric, formatted "CPU: %.1f%%", "Memory: %.2f MiB/GiB", "Disk: %.2f MB/s", "Network: <rate in the chosen unit>", "GPU: %.1f%%".
- Linux data/mechanism: an engine query `top_n(metric, t, 3)` against the store, with a 50 ms budget.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-085 History first-snapshot state
- Source: TS L28654-28655 "Collecting first history snapshot..."
- Behaviour: shown in the header until the first frame exists.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-086 History scrub benchmark flag
- Source: TS L35427-35428
- Behaviour: `tasklens --bench history-scrub [--out <file>]` replays a synthetic 6 h store and prints p50/p95/p99 scrub frame times. It is part of the PLAN `--bench` family.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P3
- Privilege: none

---

## J. Process graphs and tray (process-specific parts only)

### PROC-087 Add Minigraph (per process and column)
- Source: TS L29304, [INI ProcessSpecificMiniGraphs.*], keys L35545-35557, [INI ProcessGraphColumnMask=720C0]
- Behaviour:
  - Row menu "Add Minigraph >" lists CPU, Memory, Disk, Network, GPU and VRAM. The six graphable columns come from the graph column mask.
  - It adds a minigraph card titled "<name> (<pid>) - CPU", bound to that ProcId.
  - When the process exits, the card keeps its data, shows "Process not running" and stops. It never rebinds automatically; Process Name graphs are the way to follow restarts.
  - Toasts: "Minigraph added", "Minigraph already exists", "No space for more minigraphs", "Unable to add minigraph".
  - Persisted keys: Mode=Process, Root=pid, ImagePath, CreateTime (starttime as wall time), Column, Color, TimeRange. After restart or reboot, a card whose identity is gone shows "Process not running" until it is removed.
  - The card's window, layout, colours and detaching belong to the minigraph area.
- Linux data/mechanism: the engine adds the ProcId to the demand set, so collection continues while the card exists.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-088 Add Process Name Graph
- Source: TS L29304 area, L28719 "Process name minigraph added", [INI NameFilter]
- Behaviour: a graph of the summed metric over all processes whose name matches the row's name exactly. It follows restarts. Title "<name> (all) - CPU".
- Linux data/mechanism: the name index from the sweep.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-089 Add App Name Graph
- Source: TS L29304, "App name minigraph added", [INI AppName]
- Behaviour: a graph of the summed metric over all instances of the app ID (or unit name for unit rows). Title "<App> - CPU".
- Linux data/mechanism: cgroup aggregates (cpu.stat `usage_usec`, memory.stat, io.stat) where the app is a single cgroup; otherwise Σ over members.
- Verdict: ADAPT (app name becomes app ID / unit)
- Phase: P3
- Privilege: none

### PROC-090 Add / Remove tray icon graph (process sources)
- Source: TS L29305-29306, L30441-30446, L28715-28723, L28584-28586, [INI TrayIconGraphs Graph0Mode]
- Behaviour:
  - Row menu "Add tray icon graph >" offers the same six metrics, and "Remove tray icon graph" appears when one exists for this source.
  - Modes: Process, NameFilter and AppName. The tray tooltip for a process graph reads "<name> (PID n): CPU 3.2%".
  - Toasts: "Tray icon graph added", "Tray icon graph removed", "Tray icon graph already exists", "Unable to add tray icon graph", "Unable to remove tray icon graph".
  - The tray rendering itself belongs to the tray area.
- Linux data/mechanism: StatusNotifierItem (`org.kde.StatusNotifierItem`) via zbus. `IconPixmap` is rendered at 1 Hz.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

---

## K. Run dialog, Search File Handles, low-memory rescue

### PROC-091 Run dialog
- Source: TS L28472 (Ctrl+N), L29321, L29331-29338, L28653-28812 toasts, `runCommandHistory.json`
- Behaviour:
  - A modal "tasklens Run" dialog, pinnable. Text: "Type the name of a program, folder, document, or Internet resource." with an "Open:" combo.
  - The combo shows 5 history rows. Up/Down pick a history match, Enter runs, Shift+Enter runs as administrator.
  - Buttons: Browse... (filters "Programs" = executable files, "All files"), "Clear command history", "Run as administrator" checkbox, OK, Cancel.
  - Toasts: "Command launched", "Command history cleared", "Please enter a command", "Command contains invalid text." (NUL or control characters), "Failed to run command".
- Linux data/mechanism:
  - If the input is an existing path or a URL with a scheme, open it via `org.freedesktop.portal.OpenURI`, falling back to `xdg-open`.
  - Otherwise run `/bin/sh -c '<input>'` inside a new transient user scope, `StartTransientUnit("app-tasklens-run-<rand>.scope", "fail", [PIDs], [])` on the user manager, so it shows as an App and is detached from tasklens.
  - Run as administrator uses `pkexec /bin/sh -c '<input>'`. A GUI program started as root under Wayland usually fails, and the dialog says so under the checkbox.
  - History is `$XDG_STATE_HOME/tasklens/run-history.json` in the TS format `{"entries":[{"command","executedAt","elevated"}]}`, capped at 50 entries.
  - Browse uses the GPUI path prompt (portal FileChooser). The P2 build must confirm its dependency tree has no Tokio.
- Verdict: ADAPT
- Phase: P2
- Privilege: none, or pkexec for administrator runs

### PROC-092 Search File Handles
- Source: TS L28473, L29339-29345, L28677-28679, L30071-30081
- Behaviour:
  - A dialog with a "Pattern:" field (case-insensitive substring; `*` and `?` work as globs), a results list, Clear and Close buttons, and the checkbox "Include memory-mapped files, working directories and executables" (on by default; this covers lsof-style "who holds this file").
  - Progress: "Initializing... please wait.", "Retrieving file handles... %d%% (%d total handles)", "Searching...", "Found %d matches", "No results found."
  - Columns: Process, PID, User, Type (fd type, or cwd/root/exe/mmap), FD, Name.
  - Result menu: End Process, Go to process, Copy path, Open file location.
  - Error: "Handle search failed".
  - When processes could not be read: "N processes could not be searched (need administrator)" with a "Grant access" button.
- Linux data/mechanism: readlink of `/proc/*/fd/*`, `/proc/*/cwd`, `/proc/*/root`, `/proc/*/exe`, and parsing of `/proc/*/maps` paths. Runs on the blocking pool with cancellation between pids.
- Verdict: ADAPT
- Phase: P3
- Privilege: own processes need none. Others need helper `read_proc_files(fd-list)` per pid.

### PROC-093 Low-memory watcher
- Source: TS `/low-memory` L35425, [CL 0.8.0] | TS inventory §9 (PSI trigger)
- Behaviour:
  - Setting "Open rescue window on critical memory pressure", off by default.
  - When on, tasklens installs and enables the user unit `tasklens-lowmem.service`, which runs `tasklens --watch-memory`.
  - When memory pressure crosses the threshold, the watcher starts `tasklens --low-memory`, unless a rescue window is already open.
- Linux data/mechanism:
  - The PSI trigger: open `/proc/pressure/memory` and write `full 150000 1000000` (150 ms of full stall per 1 s window), then poll for `POLLPRI`.
  - Before acting it also checks MemAvailable < 5% of MemTotal, so a short stall does not trigger it.
  - The watcher does not initialise GPUI. It `mlockall(MCL_CURRENT|MCL_FUTURE)`, uses under 4 MiB RSS and runs with `OOMScoreAdjust=-900` in the unit.
  - systemd-oomd coexists. The watcher only opens UI and never kills anything itself.
- Verdict: ADAPT
- Phase: P3
- Privilege: none. A user unit can lower its own OOMScoreAdjust only down to its current value; if systemd refuses -900, the unit uses 0 and the setting tooltip notes it.

### PROC-094 Low-memory rescue window
- Source: TS L35830, L28914-28916, L28921, L28926-28929
- Behaviour:
  - A small window, "tasklens - Low memory". Text: "Your device is running on critically low memory." and "We recommend you close some of the processes below."
  - A "Filter..." field and a list of apps and processes sorted by memory (RSS + Swap), each with "Memory %lu%%" of MemTotal and an "End Task" button.
  - Here End Task sends SIGKILL to processes and KillUnit SIGKILL to app rows, because a graceful shutdown under memory pressure makes thrashing worse. The tooltip says "Force ends (SIGKILL)".
  - "Open Full tasklens" button.
  - When PSI `full avg10` stays below 1.0 for 10 s: "Memory pressure has eased. You can open the full tasklens or close this window."
  - Errors: "End Task Failed" with the errno text.
- Linux data/mechanism:
  - The window uses minimal GPUI: no charts, no icons, a list of at most 30 rows.
  - Data: `/proc/<pid>/status` VmRSS/VmSwap and cgroup `memory.current`, refreshed every 2 s.
  - Same identity and pidfd rules as PROC-001.
- Verdict: ADAPT
- Phase: P3
- Privilege: none for own processes. Others get the retry button through the helper.

---

## L. Persistence of process-view settings

### PROC-095 Process view settings keys
- Source: TS [INI UI.*, Sort.*, Columns.*] | MC gschema `apps-page-*` | review §5 "Settings persistence" (versioned TOML)
- Behaviour: these keys live in `$XDG_CONFIG_HOME/tasklens/settings.toml` under `[processes]`, with defaults:

| Key | Default |
|---|---|
| tree_view | true |
| grouping | true |
| group_expanded.apps / .background / .system | true |
| guide_lines | false |
| heat | true |
| heat_intensity_pct | 50 |
| highlight_new_exited | true |
| filter_include_children | false |
| hide_system | false |
| dim_zero | false |
| column_separators | false |
| subtree_totals | false (MC "Merge Process Stats": parent process rows show Σ of their subtree, marked "Σ") |
| resize_behavior | "proportional" |
| columns.order / .visible / .widths | by column ID |
| sort.key | "pid" |
| sort.ascending | true |
| history_mode | false |
| history.enabled | false |
| history.hours | 6 |
| history.persist | true |
| history.sort | "cpu" desc |
| props.hide_unnamed_handles | true |
| props.shared_libs_only | true |
| props.env_mask | true |
| dump_type | "mini" |
| lowmem.enabled | false |

- Linux data/mechanism: atomic write (temp file + rename), with migration on version change.
- Verdict: ADAPT (INI becomes TOML)
- Phase: P1 (the keys arrive together with their features)
- Privilege: none

---

## Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PROC-001 | Process identity and action targeting | BUILD | P1 |
| PROC-002 | Process sweep, rates and demand | BUILD | P1 |
| PROC-003 | Privileged read mode for other users' processes | ADAPT | P3 |
| PROC-004 | Tree View / List View | BUILD | P1 |
| PROC-005 | Task Manager Grouping | ADAPT | P1 |
| PROC-006 | Tree Connector Lines | BUILD | P1 |
| PROC-007 | Table widget basis | BUILD | P1 |
| PROC-008 | App detection from cgroups | BUILD | P1 |
| PROC-009 | Desktop-file index and exe fallback | BUILD | P1 |
| PROC-010 | Display names, Wine and icons | BUILD | P1 |
| PROC-011 | Process classification | ADAPT | P1 |
| PROC-012 | Hide system processes | ADAPT | P1 |
| PROC-013 | Flatpak bwrap unwrap | BUILD | P1 |
| PROC-014 | Launcher child apps (Steam etc.) | ADAPT | P4 |
| PROC-015 | Columns: identity and naming | BUILD/ADAPT | P1-P2 |
| PROC-016 | Columns: file and package metadata | ADAPT | P2 |
| PROC-017 | Columns: security | ADAPT | P2 |
| PROC-018 | Columns: scheduling | ADAPT | P1 |
| PROC-019 | Columns: CPU | BUILD/ADAPT | P1, P2, P4 |
| PROC-020 | Columns: memory | BUILD/ADAPT, 1 DROP | P1-P2 |
| PROC-021 | Columns: I/O | BUILD/ADAPT, 1 DROP | P1-P2 |
| PROC-022 | Columns: disk | BUILD/ADAPT | P1-P2 |
| PROC-023 | Columns: network | ADAPT | P1, P3 |
| PROC-024 | Columns: GPU | BUILD/ADAPT | P1-P2 |
| PROC-025 | Columns: power | ADAPT | P2 |
| PROC-026 | Columns: object counts | BUILD/ADAPT | P1, P3, P4 |
| PROC-027 | Columns: Linux-only extras | BUILD | P1-P2 |
| PROC-028 | Unmapped TS column ids | ADAPT | P1 |
| PROC-029 | Two-line header totals | BUILD | P1 |
| PROC-030 | KWin window bridge | ADAPT | P2 |
| PROC-031 | App window titles and Name window button | ADAPT | P2 |
| PROC-032 | Row hover tooltip | ADAPT | P1 |
| PROC-033 | Heat colouring | BUILD | P1 |
| PROC-034 | Dim zero values | BUILD | P2 |
| PROC-035 | Column separators | BUILD | P4 |
| PROC-036 | Highlight new and exited | BUILD | P1 |
| PROC-037 | Sorting | BUILD | P1 |
| PROC-038 | Column reorder, resize, persistence | BUILD | P1 |
| PROC-039 | Multi-select | BUILD | P1 |
| PROC-040 | Keyboard navigation | BUILD | P1 |
| PROC-041 | Filter and type-to-filter | BUILD | P1 |
| PROC-042 | Include children | BUILD | P1 |
| PROC-043 | Hold Ctrl to pause | BUILD | P1 |
| PROC-044 | Status bar text | BUILD | P1 |
| PROC-045 | Collapse All | BUILD | P1 |
| PROC-046 | Startup placeholder | ADAPT | P1 |
| PROC-047 | Header context menu | BUILD | P1 |
| PROC-048 | Column picker | BUILD | P1 |
| PROC-049 | Row context menu | ADAPT | P1 |
| PROC-050 | End Task | ADAPT | P1 |
| PROC-051 | Force End Task | BUILD | P1 |
| PROC-052 | End Process Tree | ADAPT | P1 |
| PROC-053 | Suspend / Resume process | ADAPT | P1 |
| PROC-054 | Suspend / Resume app (cgroup freeze) | ADAPT | P2 |
| PROC-055 | Session-critical and self protection | ADAPT | P1 |
| PROC-056 | Send Signal submenu | BUILD | P1 |
| PROC-057 | Set Priority | ADAPT | P2 |
| PROC-058 | Copy and Copy all metrics | BUILD | P1 |
| PROC-059 | Set Affinity | ADAPT | P2 |
| PROC-060 | Set I/O Priority | BUILD | P2 |
| PROC-061 | Show Window | ADAPT | P2 |
| PROC-062 | Window picker | ADAPT | P2 |
| PROC-063 | Pin windows on top | ADAPT | P2 |
| PROC-064 | Go to parent process | BUILD | P1 |
| PROC-065 | Go to service | ADAPT | P2 |
| PROC-066 | Restart Explorer analogue | ADAPT | P2 |
| PROC-067 | Open file location | ADAPT | P2 |
| PROC-068 | File properties | ADAPT | P2 |
| PROC-069 | Open in VirusTotal | BUILD | P2 |
| PROC-070 | Create dump file | ADAPT | P3 |
| PROC-071 | Process Properties window shell | ADAPT | P2 |
| PROC-072 | General tab | ADAPT | P2 |
| PROC-073 | Threads tab | ADAPT | P2 |
| PROC-074 | File descriptors tab | ADAPT | P2 |
| PROC-075 | Memory maps tab | ADAPT | P2 |
| PROC-076 | Security tab | ADAPT | P2 |
| PROC-077 | Environment tab | BUILD | P2 |
| PROC-078 | Strings tab | ADAPT | P3 |
| PROC-079 | App Properties (MC Details) | ADAPT | P2 |
| PROC-080 | History recording engine | ADAPT | P3 |
| PROC-081 | History persistence | ADAPT | P3 |
| PROC-082 | History view and table | BUILD | P3 |
| PROC-083 | History scrubber | BUILD | P3 |
| PROC-084 | History top-3 hover | BUILD | P3 |
| PROC-085 | History first-snapshot state | BUILD | P3 |
| PROC-086 | History scrub benchmark flag | BUILD | P3 |
| PROC-087 | Add Minigraph (process) | BUILD | P3 |
| PROC-088 | Add Process Name Graph | BUILD | P3 |
| PROC-089 | Add App Name Graph | ADAPT | P3 |
| PROC-090 | Tray icon graph (process sources) | ADAPT | P3 |
| PROC-091 | Run dialog | ADAPT | P2 |
| PROC-092 | Search File Handles | ADAPT | P3 |
| PROC-093 | Low-memory watcher | ADAPT | P3 |
| PROC-094 | Low-memory rescue window | ADAPT | P3 |
| PROC-095 | Process view settings keys | ADAPT | P1 |

Dropped items (2 columns, both because Linux has no counter for them):
- Non-paged pool peak (folded into Page tables).
- I/O other bytes.

## Conflicts with the architecture review

None of these overrides the review. Each needs the owner's eye because it departs from TaskSlinger or from the PLAN:

1. **History default off.** The review makes background recording off by default and asks for a separate storage design. TS records 6 h to disk by default. PROC-080 follows the review (off by default, chunked zstd store, 64 MiB RAM cap). Turning it on by default would need the owner to override the review.
2. **Memory column is RSS.** The review makes RSS the labelled default. TS's default "Memory" is private working set. PROC-020 follows the review and keeps "Working set private" (RssAnon) as an optional column. App rows use cgroup anon+shmem to avoid the double counting the review warns about.
3. **No Power usage column by default.** TS shows it by default. The review forbids it without a real source. PROC-025 builds it as an opt-in RAPL+NVML estimate, labelled "(est.)", and shows "n/a" without RAPL.
4. **No automatic pkexec retry.** MC retries failed signals through `pkexec kill` automatically. The review bans automatic retry of destructive actions, so PROC-050/056 offer a "Retry as administrator" button instead.
5. **Single Disk column by default.** The review's default list names separate disk read and write rates. PROC-022 keeps TS's single "Disk" total column by default, because TS density wins, and offers read and write rate columns. This is a small departure from the review's suggested default set, flagged for the owner.
6. **Features the review defers are phased, not cut.** The review lists as deferred: KWin window picking, show-window, TCP destruction, Windows token/modules semantics, six-hour history, tray graphs and privileged inspection. Per the brief, nothing is cut, so these are phased: window features P2 via the KWin bridge, privileged inspection and history P3. This matches the review's "deferred" status but puts some window features earlier than it implies.
7. **Priority and affinity act by pid, not pidfd.** The pidfd binding covers signals. PROC-057/059/060 use PID-based syscalls with revalidation and report best-effort results, exactly as the review says. They are listed here because the PLAN's "never hits a reused pid" claim does not extend to them.
8. **Strings scan and dump need the helper even for the user's own processes**, because `ptrace_scope=1` on this machine. The review's helper separation (action helper apart from any eBPF collector) is kept. The action helper does gain memory-read operations, and the owner should approve that widening of its whitelist.
