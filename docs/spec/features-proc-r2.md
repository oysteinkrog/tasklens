> **Leader note.** This is a second full rewrite of the processes spec, written after the TaskSlinger cross-check. It renumbers the entries, so its IDs do not match `features-proc.md`. The reconciliation pass merges the two by subject:
> - Where both files cover a topic, this file wins, except where it breaks a decision in `docs/DECISIONS.md`.
> - It breaks two decisions. D-004: the default Memory column is RssAnon, not RSS (see its PROC-020). D-005: polkit actions and D-Bus names use the `io.github.oysteinkrog.tasklens.` prefix, not `org.tasklens.` (see its PROC-059 and PROC-064).
> - Topics only in `features-proc.md` stay in force: process identity, the table widget basis, Flatpak bwrap unwrap, unmapped TaskSlinger column ids, column separators, Collapse All, the startup placeholder, pinning properties windows, the history first-snapshot state, the history scrub benchmark flag, and the settings keys.

# PROC: Processes, apps and process actions (feature spec, round 2)

Conventions used in every entry below:
- **Identity** means `ProcessId { pid, start_time_ticks }`, where `start_time_ticks` is field 22 of `/proc/<pid>/stat`. App and unit rows use `UnitId { manager: User|System, unit_name, cgroup_inode }`.
- **Validity glyphs** follow the binding architecture rule. Values are never shown as zero:
  - `…` means warming up (the first sample of a rate).
  - A lock icon means restricted. Its tooltip says "Needs administrator rights".
  - `n/a` means unsupported. Its tooltip gives the reason.
  - A dimmed value is stale (older than 3 sample periods).
  - A red `!` means the collection failed.
- **Helper** means `tl-helper`. tasklens starts it once per session with `pkexec /usr/lib/tasklens/tl-helper --session` under the polkit action `org.tasklens.helper` (`auth_admin_keep`, file `/usr/share/polkit-1/actions/org.tasklens.policy`). It talks over inherited stdin/stdout pipes with a versioned, length-prefixed protocol. It accepts only operations from a fixed allowlist, and each operation carries a target identity that the helper checks again. It exits when stdin reaches EOF or after 5 minutes idle. It never runs shell commands and never reads arbitrary files.
- **KWin bridge** is PROC-059.
- This machine has `kernel.yama.ptrace_scope=1` and `sched_autogroup_enabled=1`. RAPL `energy_uj` is world-readable because of `/etc/udev/rules.d/99-powercap.rules`. cgroup v2 is delegated, with `cgroup.freeze` and `cgroup.kill` present under `user@1000.service`. `gdb`/`gcore`, `coredumpctl` and `qdbus6` are installed. KWin 6.7.5 exposes `org.kde.KWin.queryWindowInfo`, `getWindowInfo` and `/Scripting`. All of these were checked on the machine.

---

## A. Process data and table structure

### PROC-001 Process sweep and per-process data pipeline
- Source: TS 2.1 | MC 7 "Processes" | astra-arch 3, 5
- Behaviour: there is one process table model. It refreshes at Settings > Refresh: 0.25, 0.5, 1, 2, 5 or 10 s, default 1 s. Every metric cell carries a validity state. Rate columns show `…` until there are two samples.
- Linux data/mechanism:
  - Every tick, per pid, read:
    - `/proc/<pid>/stat`: comm, state, ppid, pgrp, session, tty_nr, flags (`PF_KTHREAD` 0x00200000), minflt, majflt, utime, stime, priority, nice, num_threads, starttime, processor, policy.
    - `/proc/<pid>/statm`.
    - `/proc/<pid>/status`: Uid/Gid lines, RssAnon/RssFile/RssShmem, VmSwap, VmHWM, VmPeak, VmData, VmStk, VmPTE.
    - `/proc/<pid>/io` (own UID only; EACCES gives restricted).
  - Cached per identity and refreshed every 10 s or on comm change: `/proc/<pid>/cgroup`, `exe` readlink, and `cmdline` (bounded to 4 KiB; the full text loads on demand).
  - fd count comes from `fstatat` `st_size` of `/proc/<pid>/fd` (kernel 6.2 and later).
  - Times use `CLOCK_MONOTONIC`. Rates divide by the real elapsed time. Counter baselines reset on identity change or suspend.
  - The process directory is opened per sweep and closed promptly. The FD cache is capped at 128 (arch rule).
  - The sweep runs in `tl-engine` on the blocking pool and publishes `ProcessSnapshot` behind an `Arc`. Visible-only enrichment (PSS/USS, per-thread sums, Cycles) is driven by identity demand from the UI.
- Verdict: BUILD
- Phase: P1
- Privilege: none. Other users' `io`, `fd` and `environ` are restricted. The helper adds them (PROC-064).

### PROC-002 View modes: Tree View, Task Manager Grouping, List View, and the cycle button
- Source: TS 1.3 "Process View", 1.1 `chrome.view_mode`, cross-check B.1/C.3 | MC 3 "Tree"
- Behaviour:
  - View > Process View has three radio items: Tree View, Task Manager Grouping, List View.
  - The toolbar "stacked bars" button appears only on the Processes tab. It cycles Tree View, then Task Manager Grouping, then List View, then back to Tree View, with the toast "View mode: Tree View" (or the other name).
  - Default: Task Manager Grouping.
  - Tree View is the ppid forest. Roots are processes whose parent is not shown.
  - Task Manager Grouping shows the three groups (PROC-004). Each app or unit row expands into its own ppid subtree.
  - List View is flat.
  - Expanded state is remembered per identity for the session and per group across restarts.
- Linux data/mechanism: tl-ui keeps a flattened row list made of the expanded nodes. Siblings are sorted with stable identity tie-breaks. Missing parents and reparenting are handled by re-rooting. On Linux, orphans reparent to `systemd --user` or PID 1, so the tree stays connected.
- Verdict: ADAPT. TS stores two booleans (TreeView=1 and TaskManagerGrouping=1 in the shipped INI). tasklens stores one enum `view_mode` and treats grouping as "grouped with a tree inside", so the INI combination maps to Grouping.
- Phase: P1
- Privilege: none

### PROC-003 Tree Connector Lines
- Source: TS 1.3, INI `TreeGuideLines`
- Behaviour: a check item under View > Process View, default off. It draws 1 px guide lines in the DIVIDER colour from each parent's chevron down to its children, in Tree View and inside groups.
- Linux data/mechanism: drawn in `render_td` of the Name column from the depth and "last sibling" bits stored in the flattened row.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-004 Task Manager groups: Apps, Background processes, System processes
- Source: TS 2.1 "Groups", cross-check A.6/A.7 | MC 3 (Apps and Processes sections)
- Behaviour:
  - Three bold group header rows read "Apps (N)", "Background processes (N)" and "System processes (N)". "Windows processes" is renamed "System processes".
  - Group order is fixed, whatever the sort.
  - Expanded state persists separately for each group (`apps_expanded`, `background_expanded`, `system_expanded`, all default true).
  - Classification rules, first match wins:
    1. Kernel threads (`PF_KTHREAD`), pseudo-rows (PROC-010), processes with a UID other than the current user, and processes whose cgroup is outside `/user.slice/user-<uid>.slice/` go to System.
    2. Processes inside `user@<uid>.service/app.slice/` units that resolve to a desktop app (PROC-005/006), or that own a KWin window (PROC-059), go to Apps.
    3. Everything else owned by the current UID goes to Background. That covers `session.slice`, `background.slice`, `init.scope`, unmatched `app.slice` units and `session-N.scope`.
  - The current UID is read with `getuid()`. Nothing is hard-coded.
- Linux data/mechanism: `/proc/<pid>/cgroup` (the v2 `0::` line), parsed into slice, unit and scope.
- Verdict: ADAPT. The Windows signer and path rule is replaced by cgroup and UID rules.
- Phase: P1
- Privilege: none

### PROC-005 App detection from cgroups
- Source: MC 3 "How apps are found" step 2 (app-rummage `cgroup.rs:13-190`) | TS 2.1 App group
- Behaviour: each running app instance is one app row.
- Linux data/mechanism:
  - Walk `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/app.slice/*.{scope,service,slice}`.
  - Parse `app-[<launcher>-]<id>[@<instance>]-<rand>.scope` and `app-<id>@<rand>.service` with `\x2d` unescaping, plus `flatpak-<id>-<n>.scope`, `dbus-:<...>-<id>@<n>.service` and snap `snap.<a>.<b>-<uuid>.scope`, which maps to app ID `a_b`.
  - Read PIDs from `cgroup.procs`, recursively.
  - Rescan when the parent's inotify sees IN_CREATE/IN_DELETE on `app.slice`, and on every sweep as a fallback.
  - Example on this machine: `app-chrome\x2dwork@00bca6....service` gives app ID `chrome-work`.
  - Units that are clearly not apps (`dconf.service`, `obex.service`, `dbus.socket`, `run-*.scope` without a desktop match) stay in Background as unit rows (PROC-008).
- Verdict: BUILD (MC behaviour)
- Phase: P1
- Privilege: none

### PROC-006 App detection from .desktop files, the exe fallback and disambiguation
- Source: MC 3 steps 1, 3, 5, 6 (`desktop.rs`, `exec.rs`, `matching.rs`, `PL:apps.rs`) | TS cross-check A.7
- Behaviour: app rows get a name, an icon and a description. A process outside any app scope still joins an app when its executable matches.
- Linux data/mechanism:
  - Index `.desktop` files in `$XDG_DATA_DIRS/applications` and `~/.local/share/applications`. Skip entries with `NoDisplay=true`, `Hidden=true` or a Type other than Application. Keep Name, GenericName, Comment, Exec, Icon, StartupWMClass and X-Flatpak. Watch with inotify.
  - Strip `flatpak run`, `snap run` and `env VAR=...` from Exec.
  - Exe fallback: compare the canonical `/proc/<pid>/exe` with the resolved Exec binary.
  - An exceptions table maps helper binaries to their app. It includes MC's entries (`firefox-bin`, `soffice.bin`, `gnome-terminal-server`, `chrome`) plus `xdg-desktop-portal-kde`, which stays Background, and `Xwayland`, which is System-like but user-owned, so Background.
  - When several apps share one binary (Chromium PWAs, custom launchers such as `chrome-work`), argv tokens decide (`--app-id=`, `--profile-directory=`, `--class=`). True ties stay unmatched.
  - Ignore list: `guake-prefs`, the foot server.
  - Wine: a process whose cmdline[0] ends in `.exe` matches desktop files whose Exec contains `wine` and that `.exe` name, or whose StartupWMClass matches the `.exe` name.
  - Icons: freedesktop icon lookup in the theme from `~/.config/kdeglobals [Icons] Theme`, falling back to hicolor. Use SVG first, rasterized at 24 and 16 logical px × scale, and cache them.
- Verdict: BUILD (MC algorithm, with Wine and KDE additions)
- Phase: P1
- Privilege: none

### PROC-007 App row contents
- Source: TS 2.1 "Row details", cross-check A.12 | MC 3 "Tree"
- Behaviour:
  - Name cell: 24 px icon, the desktop Name, and " (N)" where N is the process count, using the format `%s (%s)`.
  - Then a dimmed secondary text: the title of the app's most recently active window, when the KWin bridge is available. Example: "Firefox (14)  Mozilla Firefox - Inbox".
  - Qualifiers are appended in the `%s (%s) (%s)` form: "(Flatpak)", "(Snap)", "(Wine)".
  - Children: the app's primary processes (app PIDs whose parent is not an app PID), each with its full ppid subtree. Child rows inherit the app icon at 16 px.
  - Metrics on app rows: CPU, disk, network and GPU are sums over the scope's processes. Memory comes from the cgroup (`memory.stat` `anon + shmem`), with the tooltip "Memory charged to this app, excluding file cache". PID is blank.
- Linux data/mechanism: cgroup `memory.stat` and `cgroup.procs`, the KWin bridge for titles, and the `.desktop` index.
- Verdict: ADAPT. Memory is cgroup-based so nothing is counted twice (arch rule).
- Phase: P1 (titles P2 with the bridge)
- Privilege: none

### PROC-008 Unit rows in Background and System (the "Service Host" analogue)
- Source: TS 2.1 kinds "Service Host", tooltip "Services:" | MC 4 (services link)
- Behaviour:
  - In Grouping mode, a systemd unit with more than one process becomes one expandable row. The name is the unit's Description (for example "PipeWire Multimedia Service"), with a 16 px `#service` icon and " (N)".
  - A single-process unit shows the process row directly, with the unit in the tooltip.
  - Kernel threads fold under one System row, "Kernel threads (N)", collapsed by default.
  - System units show the same way under System processes.
- Linux data/mechanism: the unit name from the cgroup path. The Description comes from systemd D-Bus `org.freedesktop.systemd1.Unit.Description` (cached from the Services collector, shared with the services area). Metrics use the same sums and the cgroup memory rule as PROC-007.
- Verdict: ADAPT (svchost grouping becomes systemd unit grouping)
- Phase: P1 (unit Description P2 when Services lands; until then the unit name is shown)
- Privilege: none. System cgroup files are world-readable.

### PROC-009 Launcher split (Steam, Lutris, Heroic)
- Source: TS 2.1 (each app is its own row) | MC 3 (scope-based grouping puts every game under Steam)
- Behaviour: inside the Steam app row, each game is a child app row named after the game, with the processes that `reaper SteamLaunch AppId=<n>` started under it.
- Linux data/mechanism:
  - The argv of `reaper` contains `AppId=<n>`. The name comes from `~/.local/share/Steam/steamapps/appmanifest_<n>.acf`, key `"name"`, and the icon from `steam_icon_<n>`.
  - Lutris and Heroic use the `LUTRIS_GAME_UUID` or `HEROIC_APP_NAME` environment variables when environ is readable (same UID).
- Verdict: ADAPT (new Linux disambiguation, so games show as apps the way they do on Windows)
- Phase: P4
- Privilege: none

### PROC-010 Pseudo-process rows
- Source: TS cross-check A.6 (System Idle Process, Memory Compression, System, Secure System, Registry)
- Behaviour: three non-actionable rows sit in System processes, and in Tree and List views at the top. PID is blank, they have no context-menu actions except Copy and Add Minigraph, and they are excluded from the process count.
  - "System Idle Process": CPU = idle share of all CPUs.
  - "System interrupts": CPU = irq + softirq share.
  - "Memory compression": Memory = zram `mm_stat` mem_used_total plus zswap pool (`/sys/kernel/debug/zswap/pool_total_size` when readable, else `n/a`); CPU = sum of `kswapd*` and `zram*` kernel threads.
  - Visibility toggle: View > Process View > "Show pseudo processes", default on.
- Linux data/mechanism: `/proc/stat` cpu line (idle+iowait; irq+softirq) and `/sys/block/zram*/mm_stat`.
- Verdict: ADAPT. "Secure System" and "Registry" are DROPPED because they have no Linux meaning (VBS enclave, registry hive). "System" maps to the real `kthreadd` (PID 2) row.
- Phase: P2
- Privilege: zswap debugfs needs root, otherwise `n/a`.

### PROC-011 Process display naming
- Source: TS columns Name / Image name / Name display | MC `models.rs:134-168`
- Behaviour:
  - Name display: the app Name for app-matched primary processes.
  - Otherwise the basename of exe. If exe is unreadable, the first cmdline token with a trailing ":" removed.
  - Wine processes use the last segment of the Windows path in cmdline[0] (`C:\...\game.exe` gives `game.exe`).
  - Kernel threads show `[comm]` in brackets, as ps does.
  - If comm differs from the exe basename (renamed threads, for example `Web Content`), it is shown as the name qualifier "firefox (Web Content)".
- Linux data/mechanism: `/proc/<pid>/exe`, `cmdline` and `comm`. Invalid UTF-8 and control characters are replaced with U+FFFD and `\x..` escapes (arch risk 7).
- Verdict: ADAPT (MC rules plus the comm qualifier)
- Phase: P1
- Privilege: none

### PROC-012 PID cell "PID +N children"
- Source: TS 2.1 `%u +%llu` L35614
- Behaviour: in Tree View and grouped subtrees, a collapsed parent's PID cell shows "11528 +18", where N is the total number of descendants. Expanded parents show the PID only. App and unit rows show a blank PID.
- Linux data/mechanism: computed from the forest.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-013 Row hover tooltip
- Source: TS 2.1 tooltip, cross-check A.5 ("path:", "user:", "Services:")
- Behaviour: after 600 ms of hover over the Name cell, a tooltip shows these lines:
  - "path: /usr/lib/firefox/firefox"
  - "user: oystein (1000)"
  - "Unit: app-firefox@ab12.scope" (replaces "Services:")
  - "Command line: …", wrapped and limited to 8 lines
  - "Package: firefox 152.0-1" when it is known
- Linux data/mechanism: cached metadata, the package lookup (PROC-017) and the unit from the cgroup.
- Verdict: ADAPT ("Services:" becomes the systemd unit)
- Phase: P1 (package line P2)
- Privilege: none

### PROC-014 Window button in the Name cell
- Source: TS 2.1 [CL 0.8.30]
- Behaviour: rows that own one or more top-level windows show a small window icon at the right edge of the Name cell. Clicking it opens a popup listing each window title ("Untitled window" when the caption is empty). Clicking an entry activates that window. The button is hidden when the KWin bridge is unavailable.
- Linux data/mechanism: the window list (pid, internalId, caption) from the KWin bridge (PROC-059).
- Verdict: ADAPT (HWND enumeration becomes a KWin script)
- Phase: P2
- Privilege: none

### PROC-015 Subtree totals on parent processes ("Merge Process Stats")
- Source: MC `apps-page-merged-process-stats`
- Behaviour:
  - Settings > Processes > "Show subtree totals on parent processes", default off.
  - When on, process rows that have children show the sum over the subtree for CPU, disk, network, GPU, power, threads and fds, with a "Σ" prefix.
  - Memory shows the RSS sum, marked "Σ RSS" with the tooltip "Shared pages are counted once per process".
  - App and unit rows always aggregate (PROC-007).
- Linux data/mechanism: tree sums in the engine projection.
- Verdict: BUILD (MC feature)
- Phase: P2
- Privilege: none

---

## B. Columns

### PROC-016 Column model, picker, widths and persistence
- Source: TS 2.1 columns, 1.3 Columns >, INI `[Columns]`/`[ColumnWidths]`, cross-check A.9/A.10/B.2/C.1 | MC (no picker, reorder only)
- Behaviour:
  - Columns have stable string IDs. The TS numeric id is kept as metadata for import. Visible state, order and width persist by ID in `$XDG_CONFIG_HOME/tasklens/settings.toml` (arch rule).
  - Default visible columns, in TS order: Name, User, PID, CPU, Memory, Disk, Network, GPU, Description, Power usage. Power usage is in the default set only when RAPL `energy_uj` is readable (PROC-024).
  - Default widths come from the INI: Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342, Power 101 logical px.
  - Min widths are measured from template strings at the current font: "999999 +99" (PID), "100.0%", "xxx Mbps", "xxx GB/s", "999 GiB", "999999", "999:59:59".
  - View > Columns > opens the column picker. It has a filter field and group headings: Image, Process, CPU, Memory, I/O, Disk, Network, GPU, Objects, Security, Linux.
  - Picker entries show alias text, for example `Working set private - "Private (anon)"` and `GPU memory dedicated - "VRAM"`.
  - Header drag reorders a column and edge drag resizes it.
  - "Resize Behavior:" is either Proportional (default) or Last Column.
  - "Resize columns to fit content" and "Reset to default" are available.
  - At least one column stays visible, with the toast "At least one column must stay visible".
  - GPU 0..15 columns appear in the picker only for adapters that exist.
- Linux data/mechanism: tl-ui settings. The gpui-component `Column` supports `.resizable` and `.movable`. Visibility is done by rebuilding `columns_count` and `column(ix)` (gpui-apis gap 6).
- Verdict: BUILD. Visibility is an explicit ID set, not TS's mask-or-width ambiguity.
- Phase: P1 (picker P1; Linux extras P2)
- Privilege: none

### PROC-017 Columns: identity, image and package

| TS id / column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 0 Name | Name | PROC-011 display name plus icon | BUILD | P1 |
| Image name | Image name | basename of `/proc/<pid>/exe`; for Wine the `.exe` name | ADAPT | P1 |
| Name display | Name display | `.desktop` Name for app primaries, otherwise comm | ADAPT | P1 |
| Image path | Image path | `readlink /proc/<pid>/exe`; " (deleted)" suffix kept and shown in red | BUILD | P1 |
| Version | Version | owning package version. Arch: batched `pacman -Qoq <paths…>` in the background for unseen exe paths (debounced 2 s), then `/var/lib/pacman/local/<pkg>-<ver>/desc` %VERSION%. Cache keyed by (path, mtime), invalidated when `/var/lib/pacman/local` mtime changes. Debian `dpkg -S`, Fedora `rpm -qf`. Flatpak: `flatpak info` from `/.flatpak-info` app ref. Wine: PE VS_VERSIONINFO of the `.exe` (path resolved via `WINEPREFIX` from environ or `~/.wine`) | ADAPT | P2 |
| Time stamp | Build date | package %BUILDDATE%; Wine: PE COFF TimeDateStamp; else `n/a` (ELF has no link time) | ADAPT | P2 |
| File modified time | File modified time | `statx` mtime of the exe | BUILD | P2 |
| File size | File size | `statx` size of the exe | BUILD | P2 |
| 81 Company | Package | owning package name, for example `firefox`; Flatpak: app ID; `n/a` when unpackaged | ADAPT | P2 |
| 82 Signer | Packager | package %PACKAGER% name without the email; Flatpak: origin remote | ADAPT | P2 |
| 83 Signature status | Package status | "Packaged", "Packaged (modified)" (only after a Properties integrity check against the mtree sha256), "Not packaged", "Deleted (upgraded)" when the exe link ends " (deleted)", "Flatpak", "Snap", "AppImage" (exe under `/tmp/.mount_*`), "Wine" | ADAPT | P2 |
| 33 Description | Description | `.desktop` Comment, then GenericName, then package %DESC%; else blank | ADAPT | P1 (package part P2) |
| Command line | Command line | `/proc/<pid>/cmdline`, NUL separators shown as spaces; 4 KiB in the table, full in Properties | BUILD | P1 |
| Type | Type | "App", "Background process", "System process", "Kernel thread", "Unit", "Pseudo" | ADAPT | P1 |
| Architecture | Architecture | ELF header of the exe: EI_CLASS plus e_machine, giving "x86-64 (64-bit)", "i386 (32-bit)", "AArch64 (64-bit)"; Wine: PE Machine, giving "x64 (64-bit, Wine)" or "x86 (32-bit, Wine)". Read once per (dev, inode) | ADAPT | P2 |

- Source: TS 2.1 table, cross-check C.1
- Behaviour: text columns sort case-insensitively. Lookups that have not finished show `…`.
- Linux data/mechanism: as in the table. Package lookups never run on the UI thread.
- Verdict: see the table
- Phase: see the table
- Privilege: none. Exe paths of other users' processes are restricted when `/proc/<pid>/exe` gives EACCES. The helper can resolve them.

### PROC-018 Columns: ownership, session and security

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 1 User | User | real UID name from Uid line field 1 via a cached `getpwuid_r`; "oystein" or "root". Tooltip shows the effective UID if it differs | BUILD | P1 |
| 3 PID | PID | pid (PROC-012 format) | BUILD | P1 |
| PID parent | Parent PID | stat field 4 | BUILD | P1 |
| Session | Session | `/proc/<pid>/sessionid` (audit session, world-readable), mapped to the logind session ID; 4294967295 gives blank | ADAPT | P2 |
| Integrity | Security label | `/proc/<pid>/attr/current` (AppArmor "firefox (enforce)", SELinux context); "unconfined", or `n/a` without an LSM | ADAPT | P2 |
| Elevation | Elevated | "Full (root)" if the effective UID is 0; "Capabilities" if CapEff is not 0 for a non-root process; "Default" otherwise (TS values Default/Full/Limited, where Limited is unused) | ADAPT | P2 |
| Protection | Sandbox | first match: "Flatpak" (flatpak- scope or `/.flatpak-info`), "Snap", "bwrap" (user namespace differs from init's, `/proc/<pid>/ns/user` inode), "Firejail", "seccomp + NoNewPrivs", "seccomp", "None"; from status `Seccomp`, `NoNewPrivs` | ADAPT | P2 |

- Source: TS 2.1 table and 9, cross-check A.22
- Behaviour: as in the table.
- Linux data/mechanism: as in the table.
- Verdict: see the table
- Phase: see the table
- Privilege: `ns/user` and `/.flatpak-info` in `/proc/<pid>/root` need ptrace read access, so they are restricted for other users (helper fallback). `attr/current` and `sessionid` are world-readable.

### PROC-019 Columns: CPU and scheduling

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 6 CPU | CPU | Δ(utime+stime) / (Δt × ncpu): % of the whole machine (arch default); 1 decimal; "0%" at zero | BUILD | P1 |
| CPU (kernel) | CPU (kernel) | Δstime share | BUILD | P1 |
| CPU (user) | CPU (user) | Δutime share | BUILD | P1 |
| CPU (relative) | CPU (one core = 100%) | Δ(utime+stime) / Δt; replaces MC's "Scale CPU to core count" toggle | BUILD | P1 |
| CPU time | CPU time | (utime+stime) / CLK_TCK, as "h:mm:ss" | BUILD | P1 |
| CPU time kernel / CPU time user | same | stime / utime | BUILD | P1 |
| Start time | Start time | btime + starttime/CLK_TCK, local time; today shows "HH:MM:SS", otherwise "YYYY-MM-DD HH:MM" | BUILD | P1 |
| Priority | Priority | class from policy and nice: SCHED_FIFO/RR "Realtime (rt N)"; nice -20..-11 "High"; -10..-1 "Above normal"; 0 "Normal"; 1..9 "Below normal"; 10..19 "Low"; SCHED_IDLE "Idle"; SCHED_BATCH appends " (batch)". The tooltip shows "nice N" | ADAPT | P1 |
| Affinity | Affinity | `Cpus_allowed_list` from status, for example "0-31" or "0-7,16-23"; "All" when it equals the online set | BUILD | P1 |
| Cycles / Cycles delta | Cycles / Cycles delta | `perf_event_open(PERF_COUNT_HW_CPU_CYCLES, exclude_kernel=1, inherit=1)` per thread, only for visible rows of own processes; global cap of 256 counters; past the cap or with `perf_event_paranoid` above 2, `n/a`. Sorting is disabled (arch: no global ranking from visible-only samples) | ADAPT | P4 |
| Context switches / delta | Context switches / delta | sum over `/proc/<pid>/task/*/status` voluntary + nonvoluntary; visible rows only; sorting disabled | ADAPT | P2 |

- Source: TS 2.1 table, CL 0.8.21 | MC `client.rs:880-889` | arch 5 "CPU percentages"
- Behaviour: as in the table.
- Linux data/mechanism: as in the table.
- Verdict: see the table
- Phase: see the table
- Privilege: none, except Cycles under `perf_event_paranoid` (2 here, so user-space counting of own processes works).

### PROC-020 Columns: memory

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 7 Memory (alias of Working set private) | Memory | VmRSS (arch binding: default memory is RSS, labelled). Header tooltip: "Resident set size. Shared pages count in every process." | ADAPT | P1 |
| Working set private | Private (anon) | RssAnon | ADAPT | P1 |
| Working set total | Working set total | VmRSS | BUILD | P1 |
| Working set peak | Working set peak | VmHWM | BUILD | P1 |
| Working set shared | Working set shared | RssFile + RssShmem | BUILD | P1 |
| Commit size | Commit size | VmData + VmStk | ADAPT | P1 |
| Commit size peak | Commit size peak | highest VmData+VmStk seen by tasklens since the process started being observed; tooltip "peak seen by tasklens" | ADAPT | P2 |
| Commit size delta | Commit size delta | change since the previous sample | BUILD | P2 |
| Virtual size / peak | Virtual size / peak | VmSize / VmPeak | BUILD | P1 |
| Paged pool peak | Page tables | VmPTE (kernel memory charged to the process for page tables) | ADAPT | P2 |
| Non-paged pool peak | (none) | no per-process non-paged kernel pool on Linux; folded into Page tables | DROP: no per-process kernel-pool accounting exists | n/a |
| Page faults / delta | Page faults / delta | minflt + majflt (stat fields 10 and 12) | BUILD | P1 |
| (Linux extra) | Major faults / delta | majflt | ADAPT (new) | P2 |
| (MC) Swap | Swap | VmSwap. Hidden from the default set; MC's auto-hide when there is no swap is kept | BUILD | P1 |
| (Linux extra) | PSS | `/proc/<pid>/smaps_rollup` Pss; visible rows only, at most 1 Hz; sorting disabled | ADAPT (new; replaces MC's RssFile/share estimate) | P2 |
| (Linux extra) | USS | smaps_rollup Private_Clean + Private_Dirty; same rules as PSS | ADAPT (new) | P2 |

- Source: TS 2.1 table | MC 7 Processes memory formula | arch 5 "Memory semantics"
- Behaviour: IEC units with 2 decimals ("16.00 KiB", "1.22 GiB"). Sorting is numeric.
- Linux data/mechanism: as in the table.
- Verdict: see the table. MC's `RssAnon + RssShmem + RssFile/share` formula is DROPPED in favour of the exact PSS column.
- Phase: see the table
- Privilege: `status` is world-readable. `smaps_rollup` needs ptrace read access, so it is restricted for other users (helper).

### PROC-021 Columns: I/O and disk

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 13 Disk | Disk | Δ(read_bytes + write_bytes − cancelled_write_bytes)/Δt from `/proc/<pid>/io`, in MB/s | ADAPT | P1 |
| Disk read rate / write rate / total rate | same | Δread_bytes, Δwrite_bytes, sum | BUILD | P1 |
| Disk read bytes / write bytes / total bytes and their deltas | same | cumulative read_bytes, write_bytes, and per-sample deltas | BUILD | P2 |
| Disk reads / writes and deltas | Disk reads / writes | there is no per-process block-op count in procfs. Process rows show `n/a`; app and unit rows use cgroup `io.stat` rios/wios summed over devices. Per-process values arrive later via a helper eBPF tracepoint (`block:block_rq_issue`) | ADAPT | P2 (cgroup) / P4 (eBPF) |
| I/O read bytes / write bytes and deltas | same | rchar / wchar (all read and write syscalls, including cache hits, pipes and sockets) | BUILD | P2 |
| I/O read+other rate | I/O read rate | Δrchar/Δt | ADAPT | P2 |
| I/O write rate | I/O write rate | Δwchar/Δt | BUILD | P2 |
| I/O other bytes | (none) | Linux has no "other" I/O accounting (Windows DeviceIoControl class) | DROP: no Linux meaning | n/a |
| (Linux extra) | I/O read ops / write ops | syscr / syscw deltas | ADAPT (new) | P2 |

- Source: TS 2.1 table | MC 7 (disk = Δ(read+write))
- Behaviour: as in the table.
- Linux data/mechanism: as in the table.
- Verdict: see the table
- Phase: see the table
- Privilege: `/proc/<pid>/io` is restricted for other UIDs (helper reads it). `io.stat` is world-readable.

### PROC-022 Column: network
- Source: TS 2.1 id 16 Network | MC 7 per-process network (nethogs scraping) | arch 5 "Network attribution"
- Behaviour:
  - The "Network" column shows the per-process rate in the chosen unit (Settings > Network transfer unit, default bits/s, auto-scale optional).
  - The header tooltip reads "Attributed TCP traffic. UDP and other users' sockets are not counted." Processes with no attributable sockets show "0 Mbps". Processes whose fds cannot be read show the restricted lock.
  - Linux extra columns "Network send" and "Network receive" are added.
  - The MC behaviour of hiding the column on error is replaced by validity states.
- Linux data/mechanism:
  - NETLINK_SOCK_DIAG `inet_diag_req_v2` dump for TCP v4 and v6 with `INET_DIAG_INFO`, using `tcp_info.tcpi_bytes_acked` (sent) and `tcpi_bytes_received`. Values are taken as deltas per socket inode.
  - The socket inode to pid map comes from walking `/proc/<pid>/fd` readlinks (`socket:[ino]`). The map is shared with the Connections area and refreshed every 2 s.
  - A socket shared by several processes is attributed to all of them and marked "shared" in the tooltip (arch: keep the ambiguity).
  - Exact all-protocol numbers come later from the helper's eBPF cgroup_skb or kprobe collector.
  - MC's nethogs approach is DROPPED (screen scraping, needs setcap).
- Verdict: ADAPT
- Phase: P1 (TCP attribution) / P4 (eBPF exact)
- Privilege: other users' fds need ptrace read, so they are restricted unless the helper is running. eBPF needs CAP_BPF and CAP_PERFMON through the helper.

### PROC-023 Columns: GPU

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| 17 GPU | GPU | max over adapters of the process's max(SM, encoder, decoder) utilization. NVIDIA: `nvmlDeviceGetProcessUtilization(dev, samples, &count, lastSeenTimeStamp)` in `tl-gpu-probe`; AMD and Intel: DRM `/proc/<pid>/fdinfo/*` `drm-engine-*` ns deltas, de-duplicated by `drm-client-id`. Unsupported gives `n/a`, never a share of the total (arch) | ADAPT | P1 |
| 18 GPU memory dedicated ("VRAM") | VRAM | NVIDIA: `usedGpuMemory` from `nvmlDeviceGetGraphicsRunningProcesses_v3` plus `nvmlDeviceGetComputeRunningProcesses_v3`, summed per pid across devices; AMD and Intel: fdinfo `drm-memory-vram` / `drm-resident-*` | ADAPT | P1 |
| GPU memory shared | GPU memory shared | AMD: fdinfo `drm-memory-gtt`; Intel: `drm-resident-system`; NVIDIA: `n/a` (no per-process GTT in NVML) | ADAPT | P2 |
| 62-77 GPU 0 .. GPU 15 | GPU 0 .. GPU 15 | per-adapter value of the GPU column, in NVML index order then DRM card order. The header shows "GPU 0" with the adapter name in the tooltip. Columns exist only for adapters that are present | BUILD | P2 |
| (MC) GPU Memory | (merged into VRAM) | same data | BUILD (merge) | P1 |
| (Linux extra) | GPU encode / GPU decode | NVML encUtil / decUtil; fdinfo `drm-engine-video*` | ADAPT (new) | P2 |

- Source: TS 2.1, CL 0.8.23, cross-check A.11/C.1 | MC 7 per-process GPU
- Behaviour: as in the table. "0%" at zero.
- Linux data/mechanism:
  - NVML runs only in the `tl-gpu-probe` child (binding).
  - Process GPU data is polled at 1 Hz at most, and only while a GPU column or graph needs it.
  - NVML pids are host pids. They are matched to identities by pid plus a start-time check.
  - fdinfo is empty on this NVIDIA driver (brief).
- Verdict: see the table
- Phase: see the table
- Privilege: NVML process queries work unprivileged on this driver (assumption: verify in phase 0). fdinfo of other users' processes needs ptrace read, so it is restricted (helper).

### PROC-024 Columns: Power usage and Power usage trend
- Source: TS 2.1 ids 79 and 80 (Very low / Low / Moderate / High / Very high) | arch 5 "Default columns"
- Behaviour:
  - "Power usage" shows a class label, with estimated watts in the tooltip ("≈ 3.4 W (estimate)").
  - Thresholds: below 0.5 W Very low; below 2 W Low; below 8 W Moderate; below 25 W High; 25 W and up Very high.
  - "Power usage trend" uses the same classes over a 2-minute moving average.
  - Without a power source, both show `n/a` ("No power sensor available").
  - The column is in the default visible set only when RAPL is readable.
- Linux data/mechanism:
  - Estimated per-process power = P_pkg × (process Δcpu_time / total busy Δcpu_time) + Σ_gpu P_gpu × (process SM util / Σ SM util on that GPU).
  - P_pkg comes from `/sys/class/powercap/intel-rapl:0/energy_uj` deltas, with wrap handled through `max_energy_range_uj` (AMD uses the same driver name).
  - P_gpu comes from `nvmlDeviceGetPowerUsage` (mW) via the probe, or hwmon `power1_average` for amdgpu.
  - App rows sum their processes.
- Verdict: ADAPT (a measured-power share model in place of Windows' energy estimator)
- Phase: P2
- Privilege: `energy_uj` has been root-only since kernel 5.10. It is readable here because of the udev rule. Otherwise the column shows `n/a`. Settings offers "Enable power readings…", which installs `99-tasklens-powercap.rules` (`SUBSYSTEM=="powercap", ACTION=="add", RUN+="/bin/chmod 0444 /sys%p/energy_uj"`) through the helper.

### PROC-025 Columns: object counts

| TS column | Linux label | Linux source | Verdict | Phase |
|---|---|---|---|---|
| Handles | File descriptors | `st_size` of `/proc/<pid>/fd` (kernel 6.2 and later), falling back to a readdir count | ADAPT | P1 |
| Threads | Threads | stat num_threads | BUILD | P1 |
| USER objects | Windows | number of top-level windows owned by the pid, from the KWin bridge; `n/a` without the bridge | ADAPT | P3 |
| GDI objects | X11 resources | XRes `XResQueryClientIds` (PID mask) plus `XResQueryClientResources` total on XWayland for X11 clients; "n/a (Wayland client)" for others | ADAPT | P4 |

- Source: TS 2.1
- Behaviour: integers, right-aligned.
- Linux data/mechanism: as in the table. XRes runs over x11rb on `$DISPLAY` (XWayland) only when the column is visible.
- Verdict: see the table
- Phase: see the table
- Privilege: the fd count for other UIDs is restricted (helper).

### PROC-026 Linux-only extra columns

| Column | Source | Phase |
|---|---|---|
| State | stat field 3: Running (R), Sleeping (S), Disk sleep (D), Stopped (T), Tracing stop (t), Zombie (Z), Idle (I); "Frozen" when `cgroup.events` frozen 1 | P1 |
| Nice | stat field 19 | P1 |
| Scheduling policy | stat field 41: OTHER, BATCH, IDLE, FIFO, RR, DEADLINE (EEVDF is the kernel default for OTHER) | P2 |
| I/O priority | `ioprio_get(IOPRIO_WHO_PROCESS, pid)`: "best-effort 4", "idle", "realtime 0", "none (from nice)" | P2 |
| Unit | systemd unit from the cgroup path | P1 |
| Cgroup | full v2 cgroup path | P2 |
| App ID | app ID from PROC-005/006 | P2 |
| OOM score / OOM adj | `/proc/<pid>/oom_score`, `oom_score_adj` | P2 |
| TTY | stat tty_nr decoded to `pts/3` | P2 |
| Process group / Session leader (SID) | stat fields 5 and 6 | P4 |
| Last CPU | stat field 39 | P4 |
| Working directory | `readlink /proc/<pid>/cwd` | P2 |
| Container | pid namespace differs from init's, giving "container (podman/docker/systemd-nspawn)" from the cgroup path (`machine.slice`, `docker-<id>.scope`, `libpod-<id>.scope`) | P4 |

- Source: MC 8 (Linux features) | arch 5
- Behaviour: available in the picker under "Linux". None are visible by default.
- Linux data/mechanism: as in the table.
- Verdict: ADAPT (new, because MC wins on Linux-only features)
- Phase: see the table
- Privilege: cwd, ns and ioprio of other UIDs are restricted (the helper reads them). Everything else is world-readable.

### PROC-027 Value formatting and zero formats
- Source: TS cross-check A.13, PLAN 9 | MC units preferences | arch 5 "Units"
- Behaviour:
  - Percent shows 1 decimal ("11.9%"). Zero shows "0%".
  - Memory uses IEC KiB, MiB or GiB with 2 decimals.
  - Disk uses MB/s with 1 decimal ("0 MB/s" at zero). The Settings Units selection (bytes/bits, base 10/2, from MC) applies.
  - Network follows Settings: Mbps by default, "0 Mbps" at zero, and auto-scale through Kbps, Mbps, Gbps and Tbps when that setting is on.
  - Counts use thin-space thousands grouping.
  - Numerals are tabular and numeric columns are right-aligned.
- Linux data/mechanism: formatting in `tl-model`. Strings for visible cells are cached per sample (arch).
- Verdict: BUILD
- Phase: P1
- Privilege: none

---

## C. Header, highlighting, filter, sort and selection

### PROC-028 Two-line header with system totals
- Source: TS 2.1 Header, cross-check B.10 | MC `table_view/mod.rs:852-948`
- Behaviour:
  - Line 1 is the column name plus the sort arrow ("PID ↑"). Line 2 is the system total in the accent-secondary text colour.
  - Totals:
    - CPU: total utilization %.
    - Memory: (MemTotal − MemAvailable)/MemTotal %.
    - Disk: highest busy % among physical disks (MC).
    - Network: total physical-interface throughput as % of the summed link speed, falling back to a rate when the speed is unknown.
    - GPU: highest utilization across GPUs.
    - VRAM: used/total %.
    - Swap: used %.
    - Threads: system total.
    - File descriptors: `/proc/sys/fs/file-nr` field 1.
  - Name, User, PID, Power usage, Description and text columns have no second line.
- Linux data/mechanism: the system snapshot (the Performance collectors).
- Verdict: BUILD (TS look, MC formulas)
- Phase: P1
- Privilege: none

### PROC-029 Heat colouring
- Source: TS 2.1, INI `ProcessValueColorCoding`=1, `...IntensityPct`=50 | MC (none)
- Behaviour:
  - Value cells of resource columns get a background lerped from transparent to VALUE_HEAT (#3E2A28 in Default Dark) at `alpha = intensity × clamp(value/reference)`.
  - References: CPU, 4 cores' worth (400/ncpu %, 12.5% here); Memory, 10% of MemTotal; Disk, 100 MB/s; Network, 10 MB/s; GPU, 100%; VRAM, 25% of total VRAM; Power, 25 W.
  - Toggle: View > Appearance > Value Color Coding. Intensity slider 0-100%.
- Linux data/mechanism: UI only.
- Verdict: BUILD (references decided here because TS's are not evidenced)
- Phase: P1
- Privilege: none

### PROC-030 Dim zero values
- Source: MC `apps-page-hide-is-zero`
- Behaviour: Settings > Processes > "Dim zero values", default off. Exact-zero values draw at 0.3 opacity.
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-031 Highlight new and exited processes
- Source: TS 1.3, INI `HighlightNewExitedProcesses`=1, "process diff linger"
- Behaviour:
  - New rows get a success-coloured tint (theme green at 35% alpha).
  - Exited rows stay in place for the linger time with an error tint and greyed text. They are not actionable: menu items are disabled and the tooltip says "Process exited".
  - Linger time is 2 refresh intervals, clamped to 1-3 s.
  - The tint fades out unless reduced motion is on, in which case it switches off at the end.
  - Toggle: View > Process View > Highlight New and Exited Processes.
- Linux data/mechanism: an identity diff between sweeps in the engine projection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-032 Filter box
- Source: TS 1.7, cross-check | MC 3 Filter (Levenshtein matching) | arch 5 "Sort/filter execution"
- Behaviour:
  - "Filter: " field in the tab bar with a clear button. It expands on focus.
  - Typing printable characters while the process table has focus starts the filter.
  - Esc clears the filter and then returns focus to the table.
  - Matching is case-insensitive substring over: display name, image name, command line, user, description, app name, unit name and group names. A numeric query also matches a PID prefix. Space-separated terms are ANDed.
  - Debounce is 100 ms.
  - Group headers stay visible with their filtered counts.
- Linux data/mechanism: the engine-owned projection is tagged with a generation, and stale results are discarded. Normalized lowercase strings are cached per identity.
- Verdict: ADAPT. MC's reverse-containment and Levenshtein ≤0.6 matching is DROPPED because it gives false hits.
- Phase: P1
- Privilege: none

### PROC-033 Include children
- Source: TS 1.7, INI `FilterShowChildren`=0, toasts L28831-28832
- Behaviour:
  - The "Include children" checkbox shows while filtering, and in Tree View. Default off.
  - When on, all descendants of each match are shown too, with toast "Including filtered process children.".
  - When turned off, toast "Hiding unmatched filtered process children.".
- Linux data/mechanism: tree projection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-034 Hide system processes toggle
- Source: TS 1.1 grid button, INI `HideMicrosoftProcesses`=0, CL 0.9.2 (Explorer stays visible)
- Behaviour:
  - A Processes-only toolbar button with the tooltip "Hide system background processes" or "Show system background processes". Toasts: "Hiding system processes" and "Showing system processes".
  - When on, these are hidden: the System processes group, kernel threads and pseudo-rows, and Background rows of desktop infrastructure.
  - Desktop infrastructure means user units in `session.slice` and `background.slice` (pipewire, wireplumber, dbus-broker, xdg-desktop-portal*, at-spi, kded6, ksmserver, powerdevil and similar).
  - These stay visible: `plasma-plasmashell.service` and `plasma-kwin_wayland.service` (the Explorer analogue), and everything in `app.slice`.
- Linux data/mechanism: cgroup path rules from PROC-004.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-035 Sorting
- Source: TS 2.1 Header, INI `Sort.ProcessesKey`=3 (PID) Ascending=1 | MC sorting | arch 5
- Behaviour:
  - Clicking a header sorts. The first click is descending for numeric columns and ascending for text. The second click reverses.
  - Default sort: PID ascending.
  - Sort always persists (TS). MC's "Remember sorting" option is DROPPED because it is always on.
  - Groups keep their fixed order. In tree views, siblings are sorted under their parent.
  - Ties break by identity.
  - The table re-sorts every refresh unless paused.
  - Visible-only columns (PSS, USS, Cycles, Context switches) cannot be sorted. Their header shows "Sorting needs full collection" on hover.
- Linux data/mechanism: the engine projection is computed off the UI thread.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-036 Selection and multi-select
- Source: TS CL 0.8.4, cross-check A.2 | MC (single selection) | arch "identity-based selection"
- Behaviour:
  - Click selects a row. Ctrl+click toggles. Shift+click selects a range. Shift+Up/Down extends. Ctrl+A selects all visible rows.
  - Right-click on an unselected row selects it first. Right-click inside the selection keeps the selection.
  - Selection is stored as identities and survives re-sorts. Exited identities drop out after the linger time.
  - Multi-select actions: End Selected Tasks, End Selected Processes, Suspend Selected Tasks, Resume Selected Tasks, Set Priority, Set Affinity, Set I/O priority, Send Signal, Copy, Copy all metrics. Properties opens one window per selected process, up to the cap.
- Linux data/mechanism: tl-ui keeps its own selection set. The gpui-component DataTable row-index selection is not used.
- Verdict: BUILD (TS multi-select; MC is single only)
- Phase: P1
- Privilege: none

### PROC-037 Keyboard in the process table
- Source: TS 7, cross-check C.4 | MC 6 Apps keys
- Behaviour:
  - Up/Down move the selection. PageUp/PageDown, Home and End move rows (not columns).
  - Right expands, or moves to the first child. Left collapses, or moves to the parent (MC).
  - Space toggles expand.
  - Enter or Ctrl+I opens Properties.
  - Delete is End Task. Shift+Delete is End Process.
  - Ctrl+E is End Task and Ctrl+X is End Process, kept as MC aliases while the table has focus.
  - Ctrl+C copies the selected rows as TSV of the visible columns (PROC-053).
  - Ctrl+F focuses the filter. Ctrl+L collapses all.
  - Menu key or Shift+F10 opens the context menu for the focused row.
  - Typing starts the filter.
- Linux data/mechanism: GPUI actions bound in the `ProcessTable` key context, one Action per command (widget rule 10).
- Verdict: ADAPT (TS evidences no table keys beyond typing and multi-select, so this set is decided here)
- Phase: P1
- Privilege: none

### PROC-038 Hold Ctrl to pause
- Source: TS L28337 | MC `window.rs:863-907` | arch 5 "Ctrl pause"
- Behaviour:
  - While Ctrl is held and the main window has focus, the displayed rows, order and values freeze. The status bar appends "[PAUSED - Ctrl]".
  - Collection and history keep running.
  - Releasing Ctrl or losing focus applies the latest snapshot.
  - Actions taken while paused use captured identities and revalidate them.
- Linux data/mechanism: the UI holds one snapshot `Arc`, which is bounded (arch).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-039 Processes status bar text
- Source: TS 1.8
- Behaviour, bottom left:
  - "296 Processes", or "41 / 296 Processes" while filtered.
  - "3 processes selected" when more than one is selected.
  - "[PAUSED - Ctrl]".
  - Linux addition: " | 42 restricted" when restricted rows exist, with the tooltip "Some values for other users' processes need administrator rights. Click to enable." Clicking it starts the helper.
  - The count excludes pseudo-rows and includes kernel threads only when they are shown.
- Linux data/mechanism: projection counts.
- Verdict: BUILD (plus the restricted count)
- Phase: P1
- Privilege: none

### PROC-040 Header context menu
- Source: TS 2.1 "Header context menu" L29313-29314, toasts L28648/L28652
- Behaviour:
  - Items: Select Columns… (opens the picker), Hide This Column, Resize columns to fit content, Resize Behavior > (Proportional, Last Column), Reset to default.
  - Toasts: "At least one column must stay visible" and "Columns reset to default".
- Linux data/mechanism: UI only.
- Verdict: BUILD
- Phase: P1
- Privilege: none

---

## D. Row context menu and process actions

### PROC-041 Row context menu structure
- Source: TS 2.1 menu table, cross-check A.1/A.2/B.9 | MC Send Signal menu
- Behaviour. Order for a process row:
  1. End Task
  2. End Process
  3. End Process Tree
  4. separator
  5. Suspend Process / Resume Process
  6. Send Signal >
  7. Set Priority >
  8. Set Affinity…
  9. Set I/O Priority >
  10. separator
  11. Show Window
  12. Restart Plasma Shell / Restart unit (only where it applies)
  13. separator
  14. Copy, Copy all metrics
  15. separator
  16. Add Minigraph >, Add Process Name Graph >, Add App Name Graph >, Add tray icon graph > / Remove tray icon graph
  17. separator
  18. Properties, File properties, Open file location, Open in VirusTotal, Create dump file
  19. separator
  20. Go to parent process, Go to service (when it is a unit's MainPID)

  Other row types:
  - App rows: "End Task" reads "End App". "Freeze App" / "Thaw App" and "Kill App (all processes)" are added. Priority, affinity and I/O priority apply to all processes in the app.
  - Unit rows: as app rows, plus "Go to service".
  - Multi-select: labels become "End Selected Tasks", "End Selected Processes", "Suspend Selected Tasks", "Resume Selected Tasks".
  - Pseudo-rows: Copy and Add Minigraph only.
  - Exited rows: all items disabled except Copy.
  - The menu is bound to the captured identities (arch).
- Linux data/mechanism: gpui-component `PopupMenu` via `TableDelegate::context_menu`. Every item dispatches an Action.
- Verdict: ADAPT (TS order, MC signals, Linux-only items)
- Phase: P1 for End/Suspend/Priority/Affinity/Copy/Open location; P2 for the rest, as in each entry.
- Privilege: per item

### PROC-042 Action pipeline, results and toasts
- Source: TS 2.1 process toasts L28645-28926 | arch 2 "Delivery semantics", 5 "Process action semantics", "Action deadlines"
- Behaviour:
  - Every action captures its target identities when the menu opens. It revalidates each one just before acting by checking that the start time still matches, then reports back.
  - Short toasts at the top centre: "Process terminated", "Process tree terminated", "Process suspended.", "Process resumed.", "Priority updated", "Affinity updated", "Selected process priority updated.", "End task requested for selected processes.".
  - Long-form errors: "Could not end task.", "Could not end process.", "Could not update selected process affinity.", and "the selected process changed; wait for the list to update and try again" when identities do not match.
  - On EPERM the toast reads "Permission denied" with a "Retry as administrator" button, which routes to the helper. There is no automatic retry (arch; MC's automatic `pkexec kill` retry is DROPPED).
  - Multi-target actions report "Ended 4 of 5 processes (1 permission denied)".
- Linux data/mechanism:
  - The `tl-engine` action dispatcher has its own request/result channel (not the snapshot slot). Results are Rejected, Started, Succeeded, Failed or OutcomeUnknown.
  - Signals go through `pidfd_open(pid, 0)`, a start-time check against `/proc/<pid>/stat` read after the open, then `pidfd_send_signal(pidfd, sig, NULL, 0)`.
  - Scheduling syscalls are revalidated just before each call and are documented as best-effort (arch).
- Verdict: BUILD
- Phase: P1
- Privilege: per action (PROC-064)

### PROC-043 End Task / End App (SIGTERM)
- Source: TS L29277-29280 | MC Stop (SIGTERM, bwrap unwrap)
- Behaviour:
  - Process rows: send SIGTERM, then toast "End task requested.". When the process has gone, the toast changes to "Process terminated".
  - App and unit rows ("End App"): SIGTERM to each primary process. If a primary is `bwrap`, it descends to the first non-bwrap child (MC).
  - No confirmation for a single own process (TS). Confirmations follow PROC-052.
- Linux data/mechanism: pidfd signals.
- Verdict: ADAPT. On Windows this is a window close or TerminateProcess; on Linux it is a graceful SIGTERM.
- Phase: P1
- Privilege: own processes need nothing. Others go through the helper's `signal` operation via the retry button.

### PROC-044 End Process (SIGKILL)
- Source: TS cross-check A.1 (L29278, toasts L28845-28846) | MC Force Stop
- Behaviour: sends SIGKILL. Toasts: "End process requested." and then "Process terminated", or "Could not end process.". On app rows this item becomes "Kill App (all processes)" (PROC-048).
- Linux data/mechanism: `pidfd_send_signal(SIGKILL)`.
- Verdict: ADAPT (TerminateProcess becomes SIGKILL)
- Phase: P1
- Privilege: as PROC-043

### PROC-045 End Process Tree
- Source: TS L29279, toast "Process tree terminated"
- Behaviour:
  - Ends the process and all its descendants.
  - A confirmation names the number of processes: "End 18 processes in this tree?". The buttons are End tree and Cancel.
  - Order: open pidfds for the whole captured tree, SIGSTOP all of them (so none can fork), SIGKILL all of them, then close the fds. Results are reported per target.
- Linux data/mechanism: the captured descendant set from the forest at capture time. Each target gets a pidfd and an identity check.
- Verdict: ADAPT (the freeze-then-kill order prevents fork races)
- Phase: P1
- Privilege: as PROC-043. Mixed ownership asks once for the helper.

### PROC-046 Send Signal submenu (all POSIX signals)
- Source: MC Send Signal (STOP, CONT, HUP, INT, TERM, KILL, USR1, USR2) | brief "all POSIX signals"
- Behaviour:
  - The top level lists Terminate (TERM), Kill (KILL), Hangup (HUP), Interrupt (INT), Quit (QUIT), Stop (STOP), Continue (CONT), User 1 (USR1) and User 2 (USR2).
  - An "Other signals >" submenu lists every remaining standard signal by name and number: ABRT, ALRM, BUS, CHLD, FPE, ILL, IO/POLL, PIPE, PROF, PWR, SEGV, STKFLT, SYS, TRAP, TSTP, TTIN, TTOU, URG, VTALRM, WINCH, XCPU, XFSZ.
  - "Real-time signal…" opens a spin box for SIGRTMIN+0 .. SIGRTMAX.
  - Signals that terminate by default show a trailing "(ends process)".
  - Toast: "Sent SIGHUP to firefox (1234)".
- Linux data/mechanism: `pidfd_send_signal`. Names come from `libc` constants.
- Verdict: BUILD (MC, extended)
- Phase: P2
- Privilege: as PROC-043

### PROC-047 Suspend and Resume
- Source: TS L29281-29285 | MC Suspend (STOP) / Continue (CONT)
- Behaviour:
  - Process rows: "Suspend Process" sends SIGSTOP and "Resume Process" sends SIGCONT. Only the item that applies is shown, chosen from State T.
  - Suspended rows show State "Stopped" and a pause glyph in the Name cell.
  - App and unit rows: "Suspend" is Freeze App (PROC-048), because a cgroup freeze cannot be seen by the app and also covers its children.
  - Toasts: "Process suspended.", "Process resumed.".
- Linux data/mechanism: pidfd signals.
- Verdict: ADAPT (NtSuspendProcess becomes SIGSTOP; app rows use the cgroup freezer)
- Phase: P1
- Privilege: as PROC-043

### PROC-048 cgroup actions: Freeze App, Thaw App, Kill App
- Source: PLAN 3 "App grouping" (freeze and kill without root) | arch "cgroup actions require validated scope ownership"
- Behaviour:
  - On app and unit rows: "Freeze App" / "Thaw App" (the row shows "Frozen"), and "Kill App (all processes)", which asks for confirmation ("Kill all 14 processes of Firefox?").
  - Toasts: "App frozen", "App thawed", "App killed".
- Linux data/mechanism:
  - Preferred: systemd user manager D-Bus `org.freedesktop.systemd1.Manager.FreezeUnit(name)` / `ThawUnit(name)` and `KillUnit(name, "all", SIGKILL)`.
  - Fallback for non-unit cgroups: write "1" or "0" to `cgroup.freeze` and wait for `cgroup.events` to show `frozen 1`; write "1" to `cgroup.kill`.
  - Before writing, check that the cgroup directory is under `user@<uid>.service`, is owned by the UID, and still has the captured `cgroup_inode`.
  - System units: the system manager's `FreezeUnit` / `KillUnit` through polkit (`org.freedesktop.systemd1.manage-units`).
- Verdict: ADAPT (new Linux capability)
- Phase: P2
- Privilege: user units need nothing. System units go through the polkit prompt from systemd. The fallback when denied is a toast.

### PROC-049 Set Priority
- Source: TS menu (Realtime, High, Above normal, Normal, Below normal, Low), toasts | MC (missing)
- Behaviour:
  - Submenu: Realtime, High (nice -10), Above normal (nice -5), Normal (nice 0), Below normal (nice 5), Low (nice 19), Idle (SCHED_IDLE), and "Custom…", which opens a dialog with a nice slider (-20..19) and a policy combo (OTHER, BATCH, IDLE, FIFO, RR) with an RT priority of 1-99.
  - The current class has a check mark.
  - "Realtime" asks for confirmation: "Realtime priority can freeze the desktop if the process spins. Set it anyway?". It sets SCHED_RR with priority 1 and SCHED_RESET_ON_FORK.
  - Toasts: "Priority updated", "Selected process priority updated.".
  - Tooltip note: "With autogroup on, nice only orders processes within the same session."
- Linux data/mechanism:
  - nice is per thread on Linux, so `setpriority(PRIO_PROCESS, tid, n)` runs for every tid in `/proc/<pid>/task` (re-listed once more to catch threads created during the change).
  - Realtime and policies use `sched_setattr(tid, …)` for each tid.
  - App rows apply to every process in the scope.
- Verdict: ADAPT (Windows priority classes become nice and policy)
- Phase: P1 (raising nice); P2 (Custom, Realtime)
- Privilege: raising nice on own processes needs nothing. Lowering nice below the current value, Realtime, and other UIDs need CAP_SYS_NICE (RLIMIT_NICE is 0 by default) and go through the helper's `setpriority` / `setsched` operation. Without it: the "Permission denied, Retry as administrator" toast.

### PROC-050 Set Affinity
- Source: TS menu "All processors" plus one entry per CPU with P/E icons, toast "At least one CPU must stay enabled", cross-check A.2 | MC (missing)
- Behaviour:
  - "Set Affinity…" opens a dialog: an "All processors" check and a grid of checkboxes "CPU 0" … "CPU 31".
  - CPUs are grouped by L3 domain ("CCD 0: CPU 0-7, 16-23") with SMT siblings paired. Intel hybrid shows P-core and E-core icons.
  - The OK button is disabled with the text "At least one CPU must stay enabled" when nothing is checked.
  - It applies to all selected processes.
  - Toasts: "Affinity updated", "Could not update selected process affinity.".
- Linux data/mechanism:
  - `sched_setaffinity(tid, mask)` for every tid. The current mask comes from `sched_getaffinity`.
  - Topology: `/sys/devices/system/cpu/cpu*/cache/index3/shared_cpu_list` and `topology/thread_siblings_list`. Hybrid: `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus` (absent on this AMD).
  - Masks are limited to the cgroup's `cpuset.cpus.effective`.
- Verdict: ADAPT (dialog rather than a 32-item submenu; the CCD grouping replaces P/E on AMD)
- Phase: P1
- Privilege: own processes need nothing. Others go through the helper's `setaffinity` operation.

### PROC-051 Set I/O Priority
- Source: brief (I/O priority) | MC (missing) | TS (none)
- Behaviour: submenu: Realtime, High (best-effort 0), Normal (best-effort 4), Low (best-effort 7), Idle. A check mark shows the current value. Toast: "I/O priority updated".
- Linux data/mechanism: `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_PRIO_VALUE(class, level))` for every tid. It only takes effect with the BFQ or mq-deadline schedulers. The tooltip names the active scheduler from `/sys/block/<dev>/queue/scheduler`.
- Verdict: ADAPT (new)
- Phase: P2
- Privilege: Realtime class needs CAP_SYS_ADMIN, and other UIDs need the helper. Best-effort and idle on own processes need nothing.

### PROC-052 Protection rules and confirmations
- Source: TS toasts "Refusing to terminate the current TaskSlinger process.", "Windows marked the selected process as critical…"
- Behaviour:
  - Refused with a toast:
    - tasklens itself: "Refusing to end tasklens itself.".
    - PID 1: "PID 1 (systemd) cannot be ended.".
    - Kernel threads: "Kernel threads cannot be signalled.".
  - These ask for confirmation with an explicit button:
    - Session-critical processes: "Ending kwin_wayland will end your desktop session. End it anyway?". Session-critical means the user's `systemd --user` (init.scope of `user@<uid>.service`), the compositor, the session bus daemon, `plasmashell`, `Xwayland`, `sddm` and `login`/`systemd-logind`.
    - End Process Tree (PROC-045).
    - Kill App (PROC-048).
    - Multi-selection of more than 1 for End.
    - Any action on another user's process.
- Linux data/mechanism:
  - The compositor pid comes from `SO_PEERCRED` on a connect to `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`.
  - The bus daemon pid comes from `org.freedesktop.DBus.GetConnectionUnixProcessID("org.freedesktop.DBus")`.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-053 Copy, Copy all metrics and copy-row formats
- Source: TS menu Copy, Copy all metrics (L35504-35513), cross-check A.2/A.3 ("Process details copied.", "Selected process values copied.", "Selected process metrics copied.")
- Behaviour:
  - "Copy" copies the right-clicked cell's displayed text. With several rows selected, it copies that column's value for each selected row, one per line, with toast "Selected process values copied.". A single row gives "Copied to clipboard".
  - "Copy all metrics" copies per row these lines, in TS's fixed order: "Name: …", "PID: …", "Parent PID: …", "CPU: 3.25%", "Memory: … MiB", "Private bytes: … MiB" (Commit size), "Working set: … MiB" (RSS), "Threads: …", "Handles: …" (fds), "Priority: …". Then one "Label: value" line for every other column with data, then "Command line: …". Rows are separated by a blank line. Toast: "Copied metrics to clipboard", or "Selected process metrics copied." for several rows.
  - Ctrl+C ("copy row", "Process details copied."): TSV of the visible columns with a header line.
  - Clipboard failure gives "Failed to copy to clipboard".
- Linux data/mechanism: GPUI `cx.write_to_clipboard` (Wayland `wl_data_device`). Command lines are never logged (arch risk 7).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-054 Open file location and File properties
- Source: TS L35502, L29316
- Behaviour:
  - "Open file location" opens Dolphin with the executable selected. Toast: "Opening file location".
  - "File properties" opens the file manager's properties dialog for the executable.
  - Both are disabled with the tooltip "Executable deleted" when the exe link ends " (deleted)", and show "Process image unavailable" when the exe is restricted.
- Linux data/mechanism:
  - Session D-Bus `org.freedesktop.FileManager1.ShowItems(["file:///usr/lib/firefox/firefox"], "")` and `ShowItemProperties([...], "")` (Dolphin implements both).
  - Fallback: `xdg-open` on the parent directory. For Properties there is no fallback; the item is hidden when FileManager1 is missing.
- Verdict: ADAPT
- Phase: P1 (location) / P2 (properties)
- Privilege: none

### PROC-055 Open in VirusTotal
- Source: TS L35503 (`https://www.virustotal.com/gui/file/<sha256>`)
- Behaviour: computes SHA-256 of the executable in the background (toast "Computing SHA-256…" when it takes longer than 500 ms), then opens the URL in the default browser. Nothing is uploaded. Errors: "SHA-256 unavailable", "Process image unavailable".
- Linux data/mechanism: reads `/proc/<pid>/exe` (works even for deleted files) with the `sha2` crate in the blocking pool. The hash is cached by (dev, inode, mtime). Opened with `xdg-open` or the portal `org.freedesktop.portal.OpenURI`.
- Verdict: BUILD
- Phase: P2
- Privilege: other users' exe gives EACCES. The fallback hashes the path from the cmdline if that file is readable; otherwise "Process image unavailable".

### PROC-056 Create dump file
- Source: TS menu, `<name>_<pid>_<yyyymmdd_hhmmss>.dmp`, INI `Crash.DumpType` (Mini/Full)
- Behaviour:
  - Writes a core file named `<name>_<pid>_<yyyymmdd_hhmmss>.core` to `$XDG_STATE_HOME/tasklens/dumps/`.
  - Toast "Dump created: <path>" with an "Open folder" button.
  - The process pauses while the core is written. A progress toast "Writing core… 1.2 GiB" shows during the write.
  - Settings > Processes > Dump type: Mini (default) or Full.
  - Hidden when `gcore` is not installed, with the tooltip "Install gdb to create dumps".
- Linux data/mechanism:
  - `gcore -o <path-without-pid> <pid>` (gdb).
  - Mini uses the process's own `/proc/<pid>/coredump_filter` (anonymous memory and ELF headers). Full temporarily writes `0x1ff` to `coredump_filter` and restores it afterwards.
- Verdict: ADAPT (MiniDump becomes a gcore core file)
- Phase: P3
- Privilege: ptrace attach is needed. With `ptrace_scope=1` (this machine), a non-descendant needs CAP_SYS_PTRACE, so the helper's `dump` operation runs gcore as root and chowns the file to the user. With `ptrace_scope=0` and the same UID, tasklens runs gcore directly. Fallback: toast "Creating a dump needs administrator rights".

### PROC-057 Show Window
- Source: TS L29289, toasts "No window found", "Process window shown."
- Behaviour: enabled when the process (or, for app rows, any process in the app) owns a window. It activates and unminimizes the most recently used window. Toasts: "Process window shown.", "No window found".
- Linux data/mechanism: KWin bridge script `workspace.activeWindow = w; w.minimized = false` for the window whose `pid` matches. On X11 or other window managers without KWin: `_NET_ACTIVE_WINDOW` client message by `_NET_WM_PID`. On GNOME Wayland: hidden.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PROC-058 Window picker ("+" button)
- Source: TS 1.1, 7 (tooltips L29201-29213, ignore shell windows, "Untitled window"/"Unknown process")
- Behaviour:
  - A toolbar "+" button, "Select process by window", on the Processes and Performance tabs.
  - Clicking it arms the picker. A toast reads "Click a window to select its process. Hold CTRL while clicking to terminate process. Esc cancels."
  - The next click on any window selects its process (switching to the Processes tab, clearing any filter that hides it, and scrolling to it).
  - Ctrl+click sends SIGTERM after a confirmation.
  - Desktop, panel, notification and OSD windows are ignored.
  - Picking tasklens itself selects tasklens.
  - Unknown pids give "Unknown process".
- Linux data/mechanism:
  - KWin D-Bus `org.kde.KWin.queryWindowInfo()` (verified present on 6.7.5) returns the clicked window's map, including `uuid`.
  - The uuid maps to a pid through the bridge script (`workspace.windowList()` with `internalId` and `pid`).
  - Ctrl state at the click comes from GPUI modifiers when tasklens has focus. Otherwise the picker offers "Terminate" in the confirmation dialog.
  - Shell windows are filtered by `windowType` (desktop, dock, notification, onScreenDisplay).
  - X11 fallback: grab the pointer and read `_NET_WM_PID`.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### PROC-059 KWin window bridge (infrastructure)
- Source: PLAN 7 risk "window picker needs a KWin script" | TS window features (title, HWND, class, rect)
- Behaviour: gives window lists, titles, state, class, geometry, activation and keep-above to PROC-007/014/057/058/066, the Windows column, and the pin buttons of tasklens dialogs.
- Linux data/mechanism:
  - tasklens owns the session D-Bus name `org.tasklens.App` and exports the `/WindowBridge` object.
  - At startup it writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js` and loads it via `org.kde.kwin.Scripting.loadScript(path, "tasklens-bridge")`, then `start()`.
  - The script pushes `{internalId, pid, caption, resourceClass, desktopFileName, minimized, maximized, keepAbove, frameGeometry, windowType}` with `callDBus("org.tasklens.App", "/WindowBridge", …)` on `windowAdded`, `windowRemoved`, `captionChanged` and `windowActivated`.
  - Commands go to the script through a D-Bus signal it subscribes to? No: KWin scripts cannot receive signals. Commands are therefore applied by reloading a tiny one-shot script with the parameters embedded (the kdotool pattern).
  - On exit, tasklens calls `unloadScript("tasklens-bridge")`.
  - Feature availability: a KWin session (`XDG_CURRENT_DESKTOP` contains KDE and `org.kde.KWin` is on the bus). Otherwise the X11 EWMH fallback or hidden features.
- Verdict: ADAPT (replaces Win32 window enumeration)
- Phase: P2
- Privilege: none

### PROC-060 Restart Plasma Shell and Restart unit (Restart Explorer analogue)
- Source: TS L29283, L29288, L35562-35572 (Explorer restart toasts)
- Behaviour:
  - On the `plasmashell` row: "Restart Plasma Shell". Toasts: "Restarting Plasma Shell...", then "Plasma Shell restarted". "Plasma Shell restart is already in progress." if pressed twice.
  - On any row that is the MainPID of a systemd unit: "Restart unit <name>". System units go through polkit.
  - GNOME Wayland: hidden (no shell restart).
- Linux data/mechanism: user manager `RestartUnit("plasma-plasmashell.service", "replace")`. The job is tracked to completion through the `JobRemoved` signal (arch: track job completion). The MainPID comes from unit property `MainPID`.
- Verdict: ADAPT
- Phase: P2
- Privilege: user units need nothing. System units use the polkit `org.freedesktop.systemd1.manage-units` prompt.

### PROC-061 Go to process, go to parent, go to service, and jump toasts
- Source: TS L29292, L29320, toasts "Switched to parent process", "Parent process not found in current view", cross-check A.4 ("Switched to Processes tab", "Process not found in current view", "Could not open properties.")
- Behaviour:
  - "Go to parent process" selects the parent row and scrolls to it. Toast: "Switched to parent process".
  - The PPID link in Properties does the same.
  - "Go to service" switches to the Services tab and selects the unit (handled by the services area).
  - Incoming "go to process" from Services, Connections and Search File Handles switches to the Processes tab ("Switched to Processes tab") and selects by identity.
  - If a filter or "Hide system processes" hides the target, the toast "Process not found in current view" carries a "Show" button that clears the filter and unhides.
  - If the process has exited: "Process not running".
- Linux data/mechanism: the identity lookup in the projection.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-062 Process minigraphs from the context menu
- Source: TS 4.2, menu "Add Minigraph >", "Add Process Name Graph >", "Add App Name Graph >", cross-check A.28 (titles "Process %lu", " by name", " by app"; states "No match", "Ended")
- Behaviour:
  - Each submenu lists CPU, Memory, Disk, Network, GPU and VRAM.
  - "Add Minigraph" binds a card to the identity and stores (pid, start time, image path). Title: "firefox (Process 1234) CPU".
  - "Add Process Name Graph" sums all processes with that image name. Title: "firefox by name CPU".
  - "Add App Name Graph" sums the app's cgroup. Title: "Firefox by app CPU".
  - An identity graph shows "Ended" after its process exits. It stays until removed and is never rebound automatically.
  - A name or app graph with no live match shows "No match".
  - Toasts: "Minigraph added", "Minigraph already exists", "No space for more minigraphs", "Process name minigraph added", "App name minigraph added", "Unable to add minigraph".
  - After a restart, identity graphs whose (image path, start time) are gone show "Ended".
- Linux data/mechanism: engine demand registration by key. The series comes from the per-process sweep, so these graphs keep the process sweep alive while the window is hidden. The card rendering itself belongs to the minigraph area.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-063 Process tray icon graphs
- Source: TS 4.3 (modes Process / NameFilter / AppName / Column), menu "Add tray icon graph >" / "Remove tray icon graph", toasts
- Behaviour:
  - Same submenu and keys as PROC-062. It adds a StatusNotifierItem tray graph.
  - "Remove tray icon graph" appears when one exists for this key.
  - Tooltip: "firefox: CPU 3.2%".
  - Toasts: "Tray icon graph added/removed/already exists", "Unable to add/remove tray icon graph".
- Linux data/mechanism: process series as in PROC-062. The SNI rendering belongs to the tray area.
- Verdict: ADAPT (Shell_NotifyIcon becomes SNI)
- Phase: P3
- Privilege: none

### PROC-064 Privileged process operations (helper)
- Source: MC `PL:processes/manager.rs:29-110` (pkexec kill) | arch risk 8 (narrow helper)
- Behaviour:
  - The first privileged request shows the polkit prompt. After that, the status bar shows a shield badge "Administrator mode", which clicking turns off.
  - The restricted lock icons fill in.
  - Allowed operations, each carrying the identity checked against `/proc/<pid>/stat` start time:
    - signal, setpriority, setsched, setaffinity, ioprio
    - read_io, read_fds (with fdinfo), read_environ, read_maps, read_smaps_rollup, read_status_ns
    - scan_strings, dump (gcore)
    - Fd and network attribution for root processes.
  - No shell, no arbitrary path. The helper never touches files other than those under `/proc/<pid>/` and the dump output directory, which is passed as an fd.
- Linux data/mechanism: polkit action `org.tasklens.helper`, `auth_admin_keep`, `pkexec`. Helper sweeps of restricted fields run at 1 Hz only while those columns are visible.
- Verdict: ADAPT
- Phase: P3 (signals and scheduling may land in P2 as single pkexec calls: `pkexec /usr/lib/tasklens/tl-helper --once signal <pid> <starttime> <sig>`)
- Privilege: root through pkexec. The fallback is restricted states and "Retry as administrator" buttons.

---

## E. Process Properties window

### PROC-065 Process Properties window shell
- Source: TS 3.1 (floating, pinnable, resizable; title; cap toast; live refresh; per-tab filter), cross-check A.38
- Behaviour:
  - A separate top-level window titled "Process Properties - firefox - PID 1234", default 900×640 logical px.
  - Tabs: General, Threads [n], File descriptors [n], Memory maps [n], Security, Environment [n], Strings.
  - Each tab has its own filter field with a clear button and the empty state "No properties match the current filter.".
  - Contents refresh at the main refresh rate.
  - Several windows can be open, capped at 8: "Too many process properties windows are already open".
  - The pin button keeps the window above others through the KWin bridge (`keepAbove`), and is hidden without the bridge.
  - When the process exits, the title appends "(not running)" and values freeze.
  - Esc closes the window.
  - "Could not open properties." on failure.
- Linux data/mechanism: GPUI `open_window` with kind Normal. Engine demand is registered for the identity.
- Verdict: ADAPT (pinning via KWin)
- Phase: P2
- Privilege: per tab

### PROC-066 General tab
- Source: TS 3.1 General (three columns), cross-check A.21/A.22/A.26 | MC details dialog fields
- Behaviour: three key/value columns.
  - **Column 1:**
    - Process, PID, Started (date-time), PPID (a link to the parent), User (name, uid, euid, suid when they differ).
    - Priority ("Nice 0, SCHED_OTHER, kernel prio 120"), Affinity ("0-31").
    - Hardening: PIE, NX stack, RELRO full/partial/none, Shadow stack (from status `x86_Thread_features`), NoNewPrivs, Seccomp.
    - Architecture, Executable (a link that opens the file location), Location category, Package (Company), Description, Cmd line (wrapped; copy button), Working dir, Root (shown when it is not "/"), Cgroup, Unit (a link to Services), App ID.
    - Location category values: Temp directory (`/tmp`, `/var/tmp`, `$XDG_RUNTIME_DIR`), User Downloads (`xdg-user-dir DOWNLOAD`), User data (`~/.local`, `~/.cache`, `~/.config`), User profile (`~`), System (`/usr`), Optional software (`/opt`), Flatpak (`/var/lib/flatpak`, `~/.local/share/flatpak`), AppImage (`/tmp/.mount_*`), Wine prefix (`*/drive_c/*`), Deleted.
  - **Column 2:**
    - SHA-256, MD5 (computed on open; "Failed to compute one or more hashes").
    - Package status (Signature) with a "Verify against package" button that compares the mtree sha256. Packager (Signer).
    - Threads, File descriptors.
    - RSS (WS total), Private anon (WS private), Shared (WS shared), PSS, USS, Swap, Commit (VmData+VmStk), Memory % of total.
    - CPU %, CPU time.
    - I/O read bytes (rchar), I/O write bytes (wchar), I/O read ops, I/O write ops.
  - **Column 3:**
    - Disk read bytes, Disk write bytes, Disk read rate, Disk write rate, Disk total rate.
    - Net I/O (attributed TCP send and receive).
    - GPU %, VRAM, GPU memory shared.
    - Window block: Win title, Window ID (KWin internalId, replacing HWND), Win state (Minimized/Maximized/Normal), Win class (resourceClass), Win rect ("L:%d T:%d R:%d B:%d"), Win size ("%d x %d"), "And N more" when there are several windows.
  - Values that cannot be read show the restricted lock. Exited processes show "PID 1234 (not running)".
  - "No top-level window" when there is none.
  - Clicking a value copies it: "Process property value copied.".
- Linux data/mechanism:
  - Hardening comes from the ELF of the exe: e_type ET_DYN plus PT_INTERP means PIE; PT_GNU_STACK flags; PT_GNU_RELRO plus DT_FLAGS BIND_NOW.
  - Everything else uses the sources in PROC-017 to PROC-026, plus `/proc/<pid>/cwd` and `root`.
- Verdict: ADAPT. Windows mitigations (DEP, ASLR, CFG and so on) become ELF hardening flags. DEP and ASLR map to NX and PIE (system ASLR from `/proc/sys/kernel/randomize_va_space`). CFG, Dyn code and Strict handle are DROPPED because they have no Linux equivalent.
- Phase: P2
- Privilege: smaps_rollup, cwd, root and io for other UIDs need the helper. Otherwise the lock icon.

### PROC-067 Threads tab
- Source: TS 3.1 Threads (Thread ID, Start Address, Start Module, User Time, Kernel Time, Priority / Base priority; "Variable" priority, cross-check A.24)
- Behaviour:
  - Columns, all sortable:
    - Thread ID, Name (thread comm), State, CPU %, User time (ms), Kernel time (ms).
    - Priority (kernel prio from stat field 18), Base priority (nice), Policy, Affinity, Last CPU.
    - Ctx switches (voluntary/involuntary), Wait channel.
  - States: "Loading threads...", "No threads found.", "No threads match the current filter.".
  - Context menu: Copy, Set thread priority > (as PROC-049 for one tid), Set thread affinity…, Send signal to thread > (tgkill; STOP is not offered because it stops the whole group).
- Linux data/mechanism: `/proc/<pid>/task/<tid>/stat`, `status`, `wchan` and `comm`. Thread actions use `setpriority`, `sched_setaffinity` and `tgkill(pid, tid, sig)` after a check of the pid's start time.
- Verdict: ADAPT. Start Address and Start Module are DROPPED because they need ptrace and symbol resolution with no stable Linux analogue; Name and Wait channel take their place.
- Phase: P2
- Privilege: wchan for other UIDs is restricted. Thread actions on other UIDs need the helper.

### PROC-068 File descriptors tab (Handles)
- Source: TS 3.1 Handles (Handle, Type, Name, Access Rights; decoded access; "Hide unnamed handles" default on; cross-check A.25 short forms)
- Behaviour:
  - Columns:
    - FD.
    - Type: file, directory, TCP, UDP, unix, netlink, pipe, FIFO, eventfd, eventpoll, timerfd, signalfd, inotify, fanotify, memfd, dmabuf, DRM, pidfd, io_uring, anon_inode (other), device.
    - Name: the link target. Sockets are resolved to "TCP 127.0.0.1:5000 → 10.0.0.2:443 ESTABLISHED", pipes to "pipe:[ino] ↔ cat (5678)" from the matching pipe inode, and DRM fds to "card1 (NVIDIA)".
    - Access: short form "R", "W", "RW", with "+A" for append, "+NB" for nonblock and "CLOEXEC".
    - Position: fdinfo pos.
  - Details pane for the selected fd: "Raw flags: 0o2104002", decoded into access mode, status flags and descriptor flags, plus the mount id and inode.
  - The filter checkbox "Hide anonymous descriptors" (default on) hides `anon_inode:*`.
  - States: "Loading handles...", "No handles reported.", "Handle detail query unavailable.".
  - Context menu: Copy, Open file location (files), Go to connection (sockets; switches to Connections).
- Linux data/mechanism: `readlinkat` on `/proc/<pid>/fd/*` and `/proc/<pid>/fdinfo/*` (flags, pos, mnt_id, eventfd-count and similar), plus the sock_diag inode map.
- Verdict: ADAPT. Windows handles become fds. Closing a handle is not offered: remote fd close needs ptrace code injection.
- Phase: P2
- Privilege: ptrace read access is needed. Same UID works, because Yama does not restrict read mode. Other UIDs go through the helper's `read_fds`, or show "Handles unavailable (insufficient privileges)".

### PROC-069 Memory maps tab (Modules)
- Source: TS 3.1 Modules (tree with mapped files, "(Primary)", "Outside loaded modules", "DLL only" default on)
- Behaviour:
  - Columns: Name, Base address, Size (VSZ sum), RSS, Perms, Path, Description (package %DESC%), Package.
  - The tree has one node per backing file, with its regions as children (start-end, perms, offset, RSS).
  - The main executable is marked "(Primary)".
  - Anonymous regions ([heap], [stack], [vdso], [vvar], anon, memfd) group under "Outside loaded modules".
  - The checkbox "Shared libraries only" (default on) keeps ELF shared objects (`*.so`, `*.so.*`) and, for Wine processes, `*.dll`, `*.exe`, `*.drv` and `*.sys`.
- Linux data/mechanism: `/proc/<pid>/maps`, and `smaps` per-mapping Rss loaded on demand when the tab is open (1 Hz at most).
- Verdict: ADAPT ("DLL only" becomes "Shared libraries only"; Wine DLLs work literally)
- Phase: P2
- Privilege: ptrace read, as PROC-068.

### PROC-070 Security tab (Token)
- Source: TS 3.1 Token (summary, Privileges table, Groups table), cross-check A.23 | inventory 9 mapping
- Behaviour:
  - Summary rows:
    - User: real/effective/saved/fs UID with names.
    - Groups: count.
    - Risky capabilities: CAP_SYS_ADMIN, CAP_SYS_PTRACE, CAP_DAC_OVERRIDE, CAP_DAC_READ_SEARCH, CAP_SYS_MODULE, CAP_SYS_RAWIO, CAP_SETUID, CAP_SETGID, CAP_NET_ADMIN, CAP_BPF, CAP_PERFMON, CAP_SYS_BOOT, CAP_CHOWN, CAP_FOWNER, when effective or ambient.
    - Admin groups: membership in wheel, sudo, root, adm, docker, libvirt, lxd or disk, marked root-equivalent for docker, libvirt, lxd and disk.
    - Flags: NoNewPrivs, Seccomp (disabled/strict/filter plus Seccomp_filters count), Dumpable.
    - Security label: the LSM.
    - Session: logind session, audit sessionid, loginuid.
    - Elevated: "Yes (root)", "Yes (capabilities)", "No".
    - Namespaces: "Isolated: user, pid, net, mnt" compared with PID 1's namespace inodes.
    - Speculation: Speculation_Store_Bypass, SpeculationIndirectBranch.
  - Table "Capabilities [n]": Name, Effective, Permitted, Inheritable, Bounding, Ambient (check marks), Description.
  - Table "Groups [n]": Name, GID, Kind (Primary/Supplementary).
  - "None reported" when empty.
  - Error: "Security information unavailable (insufficient privileges)".
- Linux data/mechanism: `/proc/<pid>/status` (Uid, Gid, Groups, Cap*, NoNewPrivs, Seccomp, Seccomp_filters, Speculation*), `/proc/<pid>/loginuid`, `sessionid`, `attr/current` and `ns/*`. Capability names and descriptions come from a static table.
- Verdict: ADAPT (Windows token becomes credentials, capabilities and namespaces). Integrity level, impersonation level, LUIDs and UI access are DROPPED because they have no Linux meaning.
- Phase: P2
- Privilege: status is world-readable. `ns/*` of other UIDs needs the helper.

### PROC-071 Environment tab
- Source: TS 3.1 Environment (Name sortable, Value; errors)
- Behaviour:
  - Columns: Name, Value. Sortable.
  - A header note reads "Environment at process start" with a Refresh button.
  - Values of names matching `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*PASSWD*`, `*_KEY`, `*API_KEY*` or `*CREDENTIAL*` are masked as "••••••". A "Reveal" toggle shows them per row.
  - Copy name, copy value, copy all as `NAME=value` lines.
  - Errors: "Environment unavailable (insufficient privileges)", "(read failed)", "Environment block unavailable".
- Linux data/mechanism: `/proc/<pid>/environ`, NUL-separated, capped at 1 MiB. It is never logged (arch risk 7).
- Verdict: ADAPT (Linux shows only the initial environment, as stated in the UI; masking added)
- Phase: P2
- Privilege: PTRACE_MODE_READ, so same UID works. Other UIDs need the helper's `read_environ`.

### PROC-072 Strings tab
- Source: TS 3.1 Strings (Address, Encoding ASCII/UTF-16, Length, Value; progress; errors), cross-check A.27
- Behaviour:
  - A "Scan" button, so the scan never starts by itself.
  - Options: minimum length (default 4, range 4-64), encodings (ASCII, UTF-8, UTF-16LE; UTF-16LE matters for Wine), and "Include file-backed mappings" (default off).
  - Progress: "Scanning process memory... 42%  18234 strings found".
  - Columns: Address, Encoding, Length, Value. The filter applies to Value.
  - The scan stops at 1,000,000 strings or 256 MiB of results: "String scan stopped: not enough memory.".
  - Errors: "Failed to read process memory.", "Strings unavailable for this process." (kernel threads).
- Linux data/mechanism: walk readable regions from `/proc/<pid>/maps` (private anonymous plus heap and stack, and optionally file-backed), skip `[vvar]`, `[vsyscall]` and device mappings, and read in 1 MiB chunks with `process_vm_readv`.
- Verdict: ADAPT
- Phase: P3
- Privilege: PTRACE_MODE_ATTACH. With `ptrace_scope=1` this needs CAP_SYS_PTRACE even for own non-child processes, so the helper's `scan_strings` streams results. With `ptrace_scope=0` and the same UID, tasklens scans directly. Fallback: "Strings unavailable: needs administrator rights" with an "Enable" button.

### PROC-073 App properties and the Mission Center details dialog
- Source: MC `process_details_dialog.rs` (icon, name, PID/App ID, Type, Command Line, live CPU, Memory, Swap, Drives, Network, GPU, GPU Memory; Ctrl+I)
- Behaviour:
  - Properties (Enter or Ctrl+I) on an app or unit row opens "App Properties - Firefox".
  - Its General tab shows: 48 px icon and name, App ID, Type (App/Unit), Desktop file path, Unit, Cgroup, Process count, Primary command line, and live usage for CPU, Memory (cgroup), Swap (`memory.swap.current`), Disk, Network, GPU and VRAM, with MC's resource rows. Swap updates correctly (MC bug fixed).
  - Its Processes tab is a mini table of member processes with "Open properties" per row.
  - On process rows, the MC dialog's fields are covered by PROC-066.
- Linux data/mechanism: cgroup files and the `.desktop` index.
- Verdict: ADAPT (the MC dialog merges into the TS Properties window)
- Phase: P2
- Privilege: none

---

## F. History mode

### PROC-074 History recording engine
- Source: TS 4.1 (retention up to 12 h, default 6), INI `HistoryStorageDurationHours`=6 | arch 3 "History policy", risk 5
- Behaviour:
  - Settings > Monitoring > "Record process history", default **off** (arch binding).
  - "History storage: N hours", 1-12, default 6.
  - With recording off, the History view shows the in-memory rolling window of the last 10 minutes, kept only while the Processes tab or History is open.
  - Toasts: "Collecting first history snapshot...", "History recording is off" (with an "Enable" button).
- Linux data/mechanism:
  - Each 1 s frame stores, for every process whose values changed beyond a threshold or that is non-idle: identity index u32, CPU permille u16, RSS in pages u32, disk B/s u32, network B/s u32, GPU permille u16.
  - Idle unchanged processes are carried forward implicitly.
  - Process metadata is stored once per identity in a dictionary.
  - The last 10 minutes stay uncompressed. Older data goes into compressed 60 s chunks (zstd level 3) in RAM, capped at 64 MiB. Beyond the cap, the oldest chunks are evicted, or moved to disk when persistence is on.
  - Gaps are recorded for suspend and engine restarts (epoch).
  - Estimate: about 100 active processes × 18 B × 21,600 s is about 39 MB raw, about 12 MB compressed (inferred, to be measured).
  - Hidden-window policy: the 1 Hz process sweep continues only while recording is on.
- Verdict: ADAPT (TS default on, tasklens default off)
- Phase: P3
- Privilege: none

### PROC-075 History view and table
- Source: TS 4.1 (9 columns, cross-check C.2: Name, CPU, Memory, Disk, Network, GPU, PID, User, Image path; default sort CPU descending), header strings
- Behaviour:
  - The history icon on the Processes tab toggles History mode, and the tab label reads "History".
  - Header strings:
    - "History - Drag scrubber below to view timeline".
    - At a scrub point: "History Snapshot: 213 processes - 4 min 12 sec ago".
    - "Live" at the right edge.
    - "Process not running" for a selected identity that is absent at that time.
  - The table has 9 columns: Name, PID, User, Image path, CPU, Memory, Disk, Network, GPU. Default sort: CPU descending.
    - At a scrub point, the table shows that second's values.
    - With a range selected, it shows the averages over the range, with the peak in the tooltip.
  - Selecting a row highlights that process's line in the graph. A clear button is shown (`history.process.clear`).
  - Metric legend toggles: CPU, Memory, Disk, Network, GPU.
  - The graph follows the History filter.
  - "History entry deleted" via the context menu item "Delete from history".
- Linux data/mechanism: queries over the history store run in the engine, which publishes bounded views.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-076 Timeline scrubber
- Source: TS 4.1 (zoom, pan, range selection, minimap; time label formats)
- Behaviour:
  - A timeline strip with a stacked metric graph and a minimap of the full retention below it.
  - Drag the handle to scrub. Mouse wheel zooms around the cursor. Shift+wheel or a middle-drag pans. Shift+drag selects a range. Double-click returns to Live.
  - Labels: "-1:02:03", "1h 02m", "2m 05s", "45s".
  - Keyboard: Left/Right step 1 s, Shift ±1 min.
- Linux data/mechanism: an application chart element (arch 4), drawn from the downsampled history view.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-077 Top-3 hover
- Source: TS 4.1 [CL 0.8.19], formats L35743-35749, cross-check A.30
- Behaviour: hovering the timeline lists the top 3 processes for the active metric at that second, using these formats: "CPU: 12.3%", "Memory: 1.22 / 62.70 GiB", "Disk: 4.20 MB/s", "Network: 12 Mbps", "GPU: 41.0%".
- Linux data/mechanism: a per-second top-K index built while recording (K = 3 per metric), stored with the frame.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PROC-078 History persistence
- Source: TS INI `HistoryPersistToDisk`=1, `store.tshp` (XPRESS), load errors L35751-35760, cross-check A.30 (".pending" then rename)
- Behaviour:
  - Settings > Monitoring > "Save history to disk". It is available only when recording is on, and defaults on in that case.
  - On launch, history inside the retention window is restored.
  - Errors shown once as toasts:
    - "Could not open process history store."
    - "Process history store is empty or truncated."
    - "Process history store version/layout mismatch; starting fresh."
    - "No in-range process history in store."
    - "Process history store is stale."
    - "Process history load timed out; starting fresh." (2 s limit; loading is async)
- Linux data/mechanism:
  - Directory `$XDG_STATE_HOME/tasklens/history/`. One file per 10 minutes (`YYYYMMDD-HHMM.tlh`).
  - Each file has a header with magic, version, boot_id (`/proc/sys/kernel/random/boot_id`) and epoch, then zstd frames.
  - Files are written to `<name>.pending`, fsynced and renamed. Older files are pruned on retention change.
  - Identities from a previous boot are marked as such.
- Verdict: ADAPT (XPRESS becomes zstd)
- Phase: P3
- Privilege: none

---

## G. Dialogs and special windows

### PROC-079 Run dialog
- Source: TS 8 "TaskSlinger Run" (text, Open: combo with history of 5 rows, Browse, Clear command history, Run as administrator, Up/Down/Enter/Shift+Enter, `runCommandHistory.json`, toasts), cross-check A.37
- Behaviour:
  - File > Run... (Ctrl+N) opens a separate window "Run", pinnable.
  - Text: "Type the name of a program, folder, document, or Internet resource."
  - "Open:" is a combo with history (5 visible rows, matching prefix first). Up/Down pick a match. Enter runs. Shift+Enter runs as administrator.
  - Buttons: Run as administrator (checkbox), Browse… (Programs filter = executable files; All files), Clear command history, OK, Cancel.
  - Toasts: "Command launched", "Command history cleared", "Please enter a command", "Command contains invalid text." (NUL or control characters), "Failed to run command".
- Linux data/mechanism:
  - An input that is a URL or an existing path is opened through the portal `org.freedesktop.portal.OpenURI` (or `xdg-open`).
  - `kcm_*` names (the `.cpl` analogue) run `systemsettings <name>`.
  - Anything else runs `/bin/sh -c '<cmd>'` inside a new transient user scope via `StartTransientUnit("app-tasklens-run-<rand>.scope", "fail", PIDs=[child])` on the user manager, so it groups as an app.
  - Run as administrator: `pkexec /bin/sh -c '<cmd>'`. The dialog notes "GUI programs may not start as root on Wayland".
  - History is stored in `$XDG_STATE_HOME/tasklens/run-history.json`, format `{"entries":[{"command","executedAt","elevated"}]}`, up to 50 entries.
  - Browse uses GPUI `prompt_for_paths` (portal FileChooser). Phase 0 must confirm it pulls in no Tokio; otherwise call ashpd through zbus directly.
- Verdict: ADAPT
- Phase: P2
- Privilege: pkexec for administrator runs.

### PROC-080 Search File Handles
- Source: TS 8 dialog (Pattern:, results, Clear, Close, progress strings, End Process, Go to process, "Handle search failed")
- Behaviour:
  - File > Search File Handles... opens a separate pinnable window.
  - Controls: the "Pattern:" field (case-insensitive substring; `*` and `?` globs allowed), the checkbox "Include memory-mapped files, working directories and executables" (default on), Search, Clear, Close.
  - Progress: "Initializing... please wait.", "Retrieving file handles... 42% (18234 total handles)", "Searching...", then "Found 12 matches" or "No results found.".
  - Result columns: Process, PID, User, Type (fd, mmap, cwd, exe, root), FD, Path.
  - Result context menu: End Process (SIGTERM), Kill Process (SIGKILL), Go to process, Copy path, Open file location.
  - When unprivileged, a note reads "Searched 412 of 530 processes. 118 need administrator rights." with a "Search all" button.
- Linux data/mechanism: walk `/proc/*/fd` readlinks, `/proc/*/maps` paths, `cwd`, `exe` and `root` on the blocking pool, cancellable between pids.
- Verdict: ADAPT (NT handle table becomes a procfs walk, like lsof)
- Phase: P3
- Privilege: other UIDs need ptrace read, so the helper's `read_fds` and `read_maps` handle them. Fallback: own processes only, with the count note.

### PROC-081 Low-memory watcher
- Source: TS 4.5 (`/low-memory` launch) | inventory 9 (PSI trigger)
- Behaviour:
  - Settings > Window & startup > "Open rescue window on critical memory pressure", default off.
  - When on, tasklens installs and enables the user unit `tasklens-lowmem.service`, which runs `tasklens --watch-memory`.
  - When memory pressure crosses the threshold and no tasklens main window is open, the watcher starts `tasklens --low-memory`.
  - Threshold: PSI `full` above 10% averaged over 10 s, or MemAvailable below 3% of MemTotal.
- Linux data/mechanism:
  - Write `full 100000 1000000` to an open `/proc/pressure/memory` fd and `poll(POLLPRI)`. Also check MemAvailable every 5 s.
  - The watcher binary mode does not initialize GPUI. Its target is below 4 MB RSS, and it calls `mlockall(MCL_CURRENT|MCL_FUTURE)` (within the systemd default RLIMIT_MEMLOCK of 8 MiB).
  - The unit sets `OOMScoreAdjust=-900` so the watcher survives the event.
  - It coexists with systemd-oomd: the tooltip notes that oomd may act first.
- Verdict: ADAPT
- Phase: P3
- Privilege: none for the user unit. Negative `OOMScoreAdjust` in a user unit is allowed only down to the user manager's own value. If it is rejected, use 0 and note it.

### PROC-082 Low-memory rescue window
- Source: TS 4.5 (window class, texts, Filter..., list with "Memory %", End Task, Open Full TaskSlinger, errors)
- Behaviour:
  - A small always-on-top window (keep-above via the KWin bridge when available) titled "tasklens - Low memory".
  - Text: "Your device is running on critically low memory." and "We recommend you close some of the processes below."
  - A "Filter..." field and a list of apps and processes sorted by memory (RSS + swap for processes, cgroup memory for apps), with "Memory 12%" and an "End Task" button per row.
  - End Task sends SIGKILL to processes, or `KillUnit` SIGKILL to apps. The tooltip says "Ends immediately, unsaved work is lost". There is no confirmation, because speed matters here.
  - "Open Full tasklens" button.
  - When pressure stays below the threshold for 10 s: "Memory pressure has eased. You can open the full tasklens or close this window."
  - Errors: "End Task Failed", "kill failed: <errno text>".
- Linux data/mechanism:
  - A minimal GPUI window with a small font set, no charts and no history. The process list refreshes every 2 s from a stripped sweep (stat, status).
  - The process is mlocked after startup.
- Verdict: ADAPT
- Phase: P3
- Privilege: none for own processes. Others are listed but disabled, with the tooltip "Needs administrator rights".

---

## Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PROC-001 | Process sweep and data pipeline | BUILD | P1 |
| PROC-002 | View modes and cycle button | ADAPT | P1 |
| PROC-003 | Tree Connector Lines | BUILD | P1 |
| PROC-004 | Apps / Background / System groups | ADAPT | P1 |
| PROC-005 | App detection from cgroups | BUILD | P1 |
| PROC-006 | App detection from .desktop, exe fallback, disambiguation | BUILD | P1 |
| PROC-007 | App row contents | ADAPT | P1 |
| PROC-008 | Unit rows (Service Host analogue) | ADAPT | P1 |
| PROC-009 | Launcher split (Steam, Lutris, Heroic) | ADAPT | P4 |
| PROC-010 | Pseudo-process rows | ADAPT (2 dropped) | P2 |
| PROC-011 | Display naming | ADAPT | P1 |
| PROC-012 | PID "+N children" | BUILD | P1 |
| PROC-013 | Row hover tooltip | ADAPT | P1 |
| PROC-014 | Window button in Name cell | ADAPT | P2 |
| PROC-015 | Subtree totals option | BUILD | P2 |
| PROC-016 | Column model, picker, widths | BUILD | P1 |
| PROC-017 | Identity, image and package columns | ADAPT | P1-P2 |
| PROC-018 | Ownership, session and security columns | ADAPT | P1-P2 |
| PROC-019 | CPU and scheduling columns | ADAPT | P1-P4 |
| PROC-020 | Memory columns | ADAPT (1 dropped) | P1-P2 |
| PROC-021 | I/O and disk columns | ADAPT (1 dropped) | P1-P4 |
| PROC-022 | Network column | ADAPT | P1/P4 |
| PROC-023 | GPU columns incl. GPU 0..15 | ADAPT | P1-P2 |
| PROC-024 | Power usage and trend | ADAPT | P2 |
| PROC-025 | Object count columns | ADAPT | P1-P4 |
| PROC-026 | Linux-only extra columns | ADAPT | P1-P4 |
| PROC-027 | Value formatting | BUILD | P1 |
| PROC-028 | Two-line header totals | BUILD | P1 |
| PROC-029 | Heat colouring | BUILD | P1 |
| PROC-030 | Dim zero values | BUILD | P2 |
| PROC-031 | New and exited highlight | BUILD | P1 |
| PROC-032 | Filter box | ADAPT | P1 |
| PROC-033 | Include children | BUILD | P1 |
| PROC-034 | Hide system processes | ADAPT | P1 |
| PROC-035 | Sorting | BUILD | P1 |
| PROC-036 | Selection and multi-select | BUILD | P1 |
| PROC-037 | Table keyboard | ADAPT | P1 |
| PROC-038 | Hold Ctrl to pause | BUILD | P1 |
| PROC-039 | Status bar text | BUILD | P1 |
| PROC-040 | Header context menu | BUILD | P1 |
| PROC-041 | Row context menu structure | ADAPT | P1-P2 |
| PROC-042 | Action pipeline and toasts | BUILD | P1 |
| PROC-043 | End Task / End App (SIGTERM) | ADAPT | P1 |
| PROC-044 | End Process (SIGKILL) | ADAPT | P1 |
| PROC-045 | End Process Tree | ADAPT | P1 |
| PROC-046 | Send Signal submenu | BUILD | P2 |
| PROC-047 | Suspend and Resume | ADAPT | P1 |
| PROC-048 | Freeze, Thaw, Kill App | ADAPT | P2 |
| PROC-049 | Set Priority | ADAPT | P1/P2 |
| PROC-050 | Set Affinity | ADAPT | P1 |
| PROC-051 | Set I/O Priority | ADAPT | P2 |
| PROC-052 | Protection rules and confirmations | ADAPT | P1 |
| PROC-053 | Copy and copy formats | BUILD | P1 |
| PROC-054 | Open file location, File properties | ADAPT | P1/P2 |
| PROC-055 | Open in VirusTotal | BUILD | P2 |
| PROC-056 | Create dump file | ADAPT | P3 |
| PROC-057 | Show Window | ADAPT | P2 |
| PROC-058 | Window picker | ADAPT | P3 |
| PROC-059 | KWin window bridge | ADAPT | P2 |
| PROC-060 | Restart Plasma Shell / unit | ADAPT | P2 |
| PROC-061 | Go to process, parent, service | BUILD | P2 |
| PROC-062 | Process minigraphs | BUILD | P3 |
| PROC-063 | Process tray graphs | ADAPT | P3 |
| PROC-064 | Privileged helper operations | ADAPT | P3 (P2 one-shot) |
| PROC-065 | Properties window shell | ADAPT | P2 |
| PROC-066 | General tab | ADAPT | P2 |
| PROC-067 | Threads tab | ADAPT | P2 |
| PROC-068 | File descriptors tab | ADAPT | P2 |
| PROC-069 | Memory maps tab | ADAPT | P2 |
| PROC-070 | Security tab | ADAPT | P2 |
| PROC-071 | Environment tab | ADAPT | P2 |
| PROC-072 | Strings tab | ADAPT | P3 |
| PROC-073 | App properties / MC details | ADAPT | P2 |
| PROC-074 | History recording engine | ADAPT | P3 |
| PROC-075 | History view and table | BUILD | P3 |
| PROC-076 | Timeline scrubber | BUILD | P3 |
| PROC-077 | Top-3 hover | BUILD | P3 |
| PROC-078 | History persistence | ADAPT | P3 |
| PROC-079 | Run dialog | ADAPT | P2 |
| PROC-080 | Search File Handles | ADAPT | P3 |
| PROC-081 | Low-memory watcher | ADAPT | P3 |
| PROC-082 | Low-memory rescue window | ADAPT | P3 |

Dropped sub-items, each with its reason in the entry:
- Non-paged pool peak (PROC-020) and I/O other bytes (PROC-021).
- Secure System and Registry pseudo-rows (PROC-010).
- Start Address and Start Module (PROC-067).
- CFG, Dyn code and Strict handle mitigations (PROC-066).
- Integrity level, impersonation level, LUIDs and UI access (PROC-070).
- Closing a remote handle (PROC-068).
- MC's nethogs scraping (PROC-022), RssFile/share estimate (PROC-020), Levenshtein filter (PROC-032), automatic pkexec retry (PROC-042), and the "Remember sorting" option (PROC-035).

## Conflicts with the architecture review and other sources

1. **History recording default.** TS records 6 h to disk by default. The arch review says background recording is off by default in v1, and six-hour history needs its own storage design. PROC-074 follows the review: off by default, with a chunked zstd store capped at 64 MiB in RAM. The owner may want TS's default on. That is a one-line settings change, but it costs about 2% of a core in the background.
2. **Default Memory column.** TS's default "Memory" is the private working set. The arch review binds the default to RSS, labelled as such. PROC-020 follows the review and keeps "Private (anon)" (RssAnon) as a separate column. The owner may prefer TS semantics, where Memory = RssAnon (closer to "private").
3. **Power usage in the default columns.** TS shows it by default. The review says to omit it unless a real measurement source exists. PROC-024 makes it default-visible only when RAPL `energy_uj` is readable, which is true on this machine because of the udev rule. That satisfies the review here and differs from TS elsewhere.
4. **Expensive visible-only columns** (Cycles, Context switches, PSS, USS) cannot be sorted, per the review's ranking rule. TS presumably sorts them. This is a deliberate parity gap.
5. **Review "unsupported feature parity" list.** The review would defer KWin window picking, show-window, TCP destruction, Windows token and modules semantics, six-hour history, tray graphs and privileged inspection. This spec does not cut them. It assigns them phases: P2 bridge and show-window, P3 picker, helper and strings, all as ADAPT. That is consistent with the brief ("nothing is cut") but later than the review's v1 scope.
6. **Priority and affinity identity guarantee.** The review says pidfd safety does not cover scheduling syscalls. PROC-042, 049, 050 and 051 revalidate the start time immediately before each per-thread call and call this best-effort. There is no conflict, but implementation agents must not claim the pidfd guarantee for these actions.
7. **Helper scope.** The review wants process and service actions kept separate from a future eBPF collector. PROC-064 lists only process operations. The eBPF items (PROC-021 disk ops, PROC-022 exact network) are marked P4 and must live in a separate helper binary or mode. That separation is assumed here but not yet in PLAN.md.
8. **KWin bridge command path.** KWin scripts can call out over D-Bus but cannot receive D-Bus signals. Commands (activate, keepAbove) therefore reload a one-shot script with the parameters embedded. This is inferred from KWin scripting limits and was not checked against KWin 6.7.5 source. Phase 0 or P2 should confirm it; `kdotool` uses the same pattern.
9. **NVML per-process access unprivileged** on driver 615 is assumed (PROC-023) and must be checked in phase 0 inside `tl-gpu-probe`.
10. **GPUI file chooser and Tokio.** PROC-079 Browse depends on GPUI's portal file chooser not pulling in Tokio. This is unverified and is a phase 0 check against the no-Tokio rule.
