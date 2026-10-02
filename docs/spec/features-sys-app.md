The SYS and APP spec is below, ready for you to save. I folded in items from sections A to C of the Fable cross-check. I checked a few facts on this machine:
- `CONFIG_INET_DIAG_DESTROY=y` is set.
- systemd is version 262.
- The user is in `wheel`, so the system journal is readable.
- Plasma 6 runs XDG autostart entries as `app-<id>@autostart.service` units.

Mission Center's "connections" request turned out to list network interfaces, not sockets. So the Connections tab exists only in TaskSlinger.

# SYS: Services, Startup Apps, Connections

## Services tab

### SYS-001 Services tab, two systemd managers
- Source: TS 2.4 | MC services_page/mod.rs:181-190, PL:services/systemd/mod.rs:325-511 | both
- Behaviour: One table lists units from the system manager and from the user manager. Two collapsible group rows, styled like TaskSlinger's process groups: "System services (N)" first, then "User services (N)". Expand state is kept in `state.services.group_expanded.{system,user}`, default true. Expanding a service row shows the processes in its cgroup as child rows, as a process subtree. The child rows reuse the process row renderer and process context menu. A unit is identified by `(manager, unit id)`, so `foo.service` can exist in both groups. TaskSlinger has a flat list and Mission Center has two sections; the grouped layout wins because Linux has two managers.
- Linux data/mechanism: zbus on the system bus and the session bus, `org.freedesktop.systemd1`, `/org/freedesktop/systemd1`, interface `org.freedesktop.systemd1.Manager`. Calls:
  - `ListUnitsByPatterns([], patterns)` for loaded units;
  - `ListUnitFilesByPatterns([], patterns)` to add installed but unloaded units (disabled ones);
  - per-unit `org.freedesktop.DBus.Properties.GetAll` on `org.freedesktop.systemd1.Unit` and `.Service`.
  Child processes come from `ControlGroup` mapped to `/sys/fs/cgroup<ControlGroup>/cgroup.procs`, read recursively. One D-Bus adapter in tl-engine owns both connections (review: zbus `default-features=false`, `async-io`, executor driven by an asupersync task).
- Verdict: ADAPT (Windows SCM becomes two systemd managers; group rows added)
- Phase: P2
- Privilege: none to list. If the session bus is missing, the User group shows "User manager not available". If the system bus is missing, the System group shows "System manager not available". Mission Center hides the whole tab when both lists are empty. tasklens never hides it and shows the empty state "systemd not found" instead.

### SYS-002 Unit types shown
- Source: MC PL:services/systemd/mod.rs:437-474 | TS 2.4
- Behaviour: A "Unit types" dropdown in the Services filter strip (SYS-004) has check items: Services, Sockets, Timers, Mounts, Automounts, Paths, Targets, Swaps, Slices, Scopes. Default on: Services, Sockets, Timers, Mounts. Mission Center shows service, socket and mount; timers are added because they are the Linux scheduled-task view. Hidden by rule: `LoadState=not-found` units with no unit file, uninstantiated templates (`foo@.service`; their instances are listed), and `device` units.
- Linux data/mechanism: suffix filter on the unit id, done in the engine projection. Persisted in `services.unit_types`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-003 Services columns
- Source: TS 2.4 (widths 250,300,100,120,80,200,260; Columns.Services=0..6) | MC table_view (resource columns)
- Behaviour: Default visible columns, in TaskSlinger order: Name, Display name, Status, Startup type, PID, User, Path. Optional columns, hidden by default and chosen through View > Columns: Manager (System/User), Unit type, Sub state, Load state, CPU, Memory, Disk (read+write rate), Tasks, Active since, Unit file, Triggered by, Description.

  | Column | Value |
  |---|---|
  | Name | unit id, e.g. `sshd.service` |
  | Display name | `Description` |
  | Status | see table below |
  | Startup type | `UnitFileState` mapped to: Enabled, Enabled (runtime), Disabled, Static, Indirect, Masked, Generated, Transient, Alias, Linked, or "-" when there is no unit file |
  | PID | `MainPID`, blank when 0 |
  | User | real uid owner of MainPID from `/proc/<pid>/status`; otherwise `User=`; otherwise "root" (system manager) or the current user (user manager) |
  | Path | services: `ExecStart[0].path`; sockets: first `Listen` address; mounts: `What`; timers: the `Unit` they trigger; others blank |

  Status mapping (ActiveState and SubState):

  | systemd state | Status shown |
  |---|---|
  | active + sub running | Running |
  | other active (exited, listening, mounted, waiting, elapsed, plugged) | Active |
  | activating | Starting |
  | deactivating | Stopping |
  | reloading or refreshing | Reloading |
  | inactive | Stopped |
  | failed | Failed |
  | maintenance | Maintenance |

  TaskSlinger's Resuming, Pausing and Paused have no systemd state and are not used. Startup type uses systemd words, because Automatic and Manual would lose Static, Indirect and Masked.
  The resource columns (CPU, Memory, Disk, Tasks) are cgroup totals for the unit, sampled at 1 Hz only while one of them is visible. Units without a cgroup show "-", never 0.
- Linux data/mechanism: Unit properties `Id`, `Description`, `ActiveState`, `SubState`, `LoadState`, `UnitFileState`, `FragmentPath`, `ActiveEnterTimestamp`, `TriggeredBy`, `ControlGroup`. Service properties `MainPID`, `User`, `ExecStart`. Socket property `Listen`. Mount property `What`. Timer property `Unit`. Cgroup files `cpu.stat usage_usec` (delta), `memory.current`, `io.stat` rbytes+wbytes (delta), `pids.current`.
- Verdict: ADAPT
- Phase: P2 (resource columns P2)
- Privilege: none

### SYS-004 State filters and filter strip
- Source: MC services_page/page.blp:133-211, table_view/mod.rs:641 | TS has none
- Behaviour: One 28 px strip at the top of the Services tab holds:
  - toggles "Running", "Failed", "Stopped", "Disabled";
  - the Unit types dropdown (SYS-002);
  - toggles "System" and "User", both on by default;
  - a "Remove filters" button.
  State classification follows Mission Center: running first, then failed, then stopped (enabled and not running), otherwise disabled. Selected states combine with OR. The result combines with the tab's text filter using AND. No state toggles means all states. Child process rows inherit their service's state. The filter state is saved in `services.filter_states`, default empty.
- Linux data/mechanism: engine projection over cached unit state
- Verdict: BUILD (Mission Center behaviour, TaskSlinger styling)
- Phase: P2
- Privilege: none

### SYS-005 Live updates and job tracking
- Source: MC PL:services/systemd/mod.rs (Subscribe, UnitFilesChanged, Reloading) | review "Services" row
- Behaviour: The table updates without polling.
  - "Starting service..." (toast, replaced in place by id) stays until the job ends.
  - Then: "Service started" / "Service stopped" / "Service restarted", or "Failed to start service: <result>" with a "View logs" button.
  - Only one action can run per unit at a time. Another request on the same unit shows "A service action is already in progress."
  - If the bus drops before the job ends, the outcome is OutcomeUnknown: the toast "Service action finished." is followed by a full resync.
- Linux data/mechanism:
  - `Manager.Subscribe()` on both buses.
  - Signals `UnitNew`, `UnitRemoved`, `JobNew`, `JobRemoved(id, job, unit, result)`, `UnitFilesChanged`, `Reloading`, plus `PropertiesChanged` on unit objects.
  - Add the `JobRemoved` match before calling the method, then match the returned job path, so a fast job cannot be missed.
  - Result `done` maps to success. `failed`, `timeout`, `canceled`, `dependency`, `skipped` and `invalid` each map to a failure with that word.
  - Action states are Rejected, Started, Succeeded, Failed and OutcomeUnknown, delivered on the action result path, never in the snapshot.
  - There is no client-side timeout on the job, because systemd enforces `TimeoutStartSec`.
  - On reconnect, re-subscribe and reload the lists.
- Verdict: ADAPT
- Phase: P2
- Privilege: none for signals

### SYS-006 Start, Stop, Restart, Reload, Reset failed
- Source: TS 2.4 | MC actions.rs:87-138, PL service.rs:71-141
- Behaviour:
  - Context menu and keys: Start (Ctrl+S; enabled when not active), Stop (Ctrl+E; when active), Restart (Ctrl+R; when active).
  - Reload: shown only when `CanReload`.
  - Reset failed state: tasklens addition, shown only when Failed; clears the failed state.
  - No confirmation dialog for any of these (both apps agree).
  - Multi-select runs the action on each selected unit and gives one toast per result.
- Linux data/mechanism: `StartUnit(name,"replace")`, `StopUnit(name,"replace")`, `RestartUnit(name,"replace")`, `ReloadUnit(name,"replace")`, `ResetFailedUnit(name)`. All called with the `ALLOW_INTERACTIVE_AUTHORIZATION` message flag. Mission Center uses "fail" mode for start; tasklens uses "replace" to match `systemctl`.
- Verdict: BUILD
- Phase: P2
- Privilege: user manager needs none. The system manager uses polkit `org.freedesktop.systemd1.manage-units` (auth_admin_keep), and polkit-kde-authentication-agent shows the prompt. A cancelled prompt gives the toast "Authentication cancelled". Denied gives "Failed to start service: not authorized".

### SYS-007 Startup type changes (enable, disable, mask)
- Source: TS 3.2 Startup combo, toasts L28651, L28663 | MC Enabled switch (details dialog)
- Behaviour: Context submenu "Startup type >" with Enabled, Disabled, Masked. The current value is checked. Static, Indirect, Generated and Transient units show the submenu disabled, with tooltip "This unit has no install section". The same choice sits in the Properties General combo (SYS-011). Toasts: "Startup type changed" / "Failed to change startup type".
- Linux data/mechanism:
  - Enabled: `EnableUnitFilesWithFlags([name], 0, …)`.
  - Disabled: `DisableUnitFilesWithFlags([name], 0)`.
  - Masked: `MaskUnitFiles([name], false, true)`.
  - Leaving Masked: `UnmaskUnitFiles` first.
  - After each, call `Manager.Reload()`, as systemctl does.
- Verdict: ADAPT
- Phase: P2
- Privilege: system manager uses polkit `org.freedesktop.systemd1.manage-unit-files` plus `org.freedesktop.systemd1.reload-daemon`. This may prompt twice when the keep window does not cover both actions; that is accepted. The user manager needs none.

### SYS-008 Services context menu
- Source: TS 2.4 L28202-28203, L29283-29317 | MC table_view.blp:179
- Behaviour: In order:
  1. Start, Stop, Restart, Reload
  2. Startup type >
  3. separator
  4. View logs (opens Properties on the Logs tab)
  5. Properties (Alt+Enter, Ctrl+I)
  6. File properties
  7. Open file location
  8. Open unit file
  9. separator
  10. Copy (clicked cell), Copy row
  11. Jump to process
  Child process rows show the full process menu instead.
- Linux data/mechanism:
  - File properties: `org.freedesktop.FileManager1.ShowItemProperties([file://binary], "")` on the session bus.
  - Open file location: `org.freedesktop.FileManager1.ShowItems([file://binary], "")`.
  - Open unit file: `xdg-open FragmentPath`, opened read-only by the user's editor.
  - Toasts: "Opening service file location." / "Could not open service file location.", "Opened service file properties." / "Could not open service file properties.", "Service image unavailable" when there is no ExecStart.
- Verdict: ADAPT (File properties maps to the FileManager1 portal; "Open unit file" added)
- Phase: P2
- Privilege: none

### SYS-009 Copy formats (services)
- Source: TS L35514-35520 (cross-check A.3), toasts "Service details copied." / "Could not copy service details."
- Behaviour: Copy row puts these lines on the clipboard:
  ```
  Name: <id>
  Display name: <Description>
  Status: <Status>
  Start type: <Startup type>
  PID: <MainPID>
  User: <User>
  Binary: <ExecStart[0].path>
  ```
  With multi-select, the blocks are separated by a blank line. Ctrl+C on the table copies the selected rows as tab-separated values of the visible columns, with a header line.
- Linux data/mechanism: Wayland clipboard through GPUI `write_to_clipboard`
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-010 Jump between services and processes
- Source: TS L28709-28710, L28789-28790, L28830, L28700
- Behaviour:
  - Services "Jump to process" switches to Processes and selects MainPID by identity (pid plus start time). Toast "Switched to service process.". If filtered out or hidden: "Service process not visible in the current process view.". When MainPID is 0: "Service not running".
  - The reverse, Processes "Go to service" (menu item owned by the process spec), switches to Services, expands the group and selects the unit. Toasts "Switched to Services tab" / "Switched to service." / "Service not found in current view".
- Linux data/mechanism: process to unit lookup through `/proc/<pid>/cgroup`. The last path element ending in `.service` (or any unit suffix) under `system.slice`, or under `user@<uid>.service`, gives manager and unit. Unit to process uses MainPID.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-011 Service Properties window: frame and General tab
- Source: TS 3.2 L28932-28958, L30268-30277, "Too many service properties windows are already open" L28714 | MC service_details_dialog
- Behaviour:
  - Frame: a separate top-level window titled "<id> Properties", with a pin button that sets always on top (APP-009).
  - Up to 10 can be open at once. The 11th gives the toast above.
  - Values refresh live through PropertiesChanged.
  - Tabs: General, Log On, Recovery, Dependencies, Logs. Logs comes from Mission Center.
  - Buttons: Start, Stop, Restart, OK, Cancel, Apply. OK and Apply commit the Startup combo through SYS-007. The toast "Changes applied" appears on success.

  General fields:

  | Field | Value |
  |---|---|
  | Name | Id |
  | Display | Description |
  | Desc | Documentation URIs; man: and https: are clickable |
  | Path | full ExecStart argv joined, shell-quoted |
  | Unit file | FragmentPath |
  | Drop-ins | DropInPaths |
  | Startup | combo, as in SYS-007 |
  | Status | "Running (running) since <ActiveEnterTimestamp>" |
  | Main PID | link that jumps to the process |
  | Memory | MemoryCurrent |
  | Tasks | TasksCurrent |
  | CPU time | CPUUsageNSec |
  | Manager | System or User |

  Mission Center's empty Description bug does not apply.
- Linux data/mechanism: Unit and Service properties as named, through GetAll on open and PropertiesChanged afterwards
- Verdict: ADAPT
- Phase: P2
- Privilege: as SYS-006 and SYS-007; viewing needs none

### SYS-012 Service Properties: Log On tab
- Source: TS L28938-28939 | MC details "Process" group (User, Group, File Location)
- Behaviour: Read-only fields:

  | Field | Value |
  |---|---|
  | Log on | `User=`, or "root"; "dynamic user (<name>)" when `DynamicUser=yes` |
  | Group | `Group=` |
  | Supplementary groups | list |
  | Runs as | real uid and gid of MainPID |
  | PAM service | PAMName |
  | Manager | "System manager" or "User manager (<user>)" |

  Missing values show "N/A", as in Mission Center. "Interact with desktop" has no systemd meaning. Its place is taken by "Session: graphical session" for user units bound to `graphical-session.target`, or "Session: none".
- Linux data/mechanism: Service properties `User`, `Group`, `DynamicUser`, `SupplementaryGroups`, `PAMName`; `/proc/<pid>/status` Uid and Gid; `PartOf`/`BindsTo` containing `graphical-session.target`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-013 Service Properties: Recovery tab
- Source: TS L28947-28955
- Behaviour: Windows' per-failure actions become systemd's restart policy:

  | Field | systemd property |
  |---|---|
  | On failure | `Restart=` (no, on-success, on-failure, on-abnormal, on-watchdog, on-abort, always) |
  | Restart service after: N ms | `RestartUSec` (plus RestartSteps / RestartMaxDelayUSec if set) |
  | Give up after: N starts within N s | `StartLimitBurst` / `StartLimitIntervalUSec`; replaces "Reset fail count after: N days" |
  | When giving up | `StartLimitAction` (none, reboot, reboot-force, poweroff, exit…); replaces "Reboot the Computer" |
  | Failure action | `FailureAction` |
  | Run on failure | `OnFailure=` units, as links; replaces "Run a Program" |
  | Restarts so far | `NRestarts` |

  P2 shows these read-only. P3 makes them editable, with Apply writing a drop-in.
- Linux data/mechanism: P3 writes `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf` for system units, through tl-helper, then calls `Manager.Reload()`. User units get `~/.config/systemd/user/<unit>.d/50-tasklens-recovery.conf`, written directly and atomically. `SetUnitProperties` cannot change `Restart=` on non-transient units, so drop-ins are required.
- Verdict: ADAPT
- Phase: P2 view, P3 edit
- Privilege: system-unit edits need tl-helper with polkit action `io.github.oysteinkrog.tasklens.service-dropin` (auth_admin_keep). Without the helper the fields stay read-only, with tooltip "Needs the tasklens helper".

### SYS-014 Service Properties: Dependencies tab
- Source: TS L28940-28941
- Behaviour: Two lists. Each row shows the unit name, the relation tag and a state dot.
  - "Depends on:" holds Requires, Requisite, Wants, BindsTo, PartOf, Upholds.
  - "Dependents:" holds RequiredBy, RequisiteOf, WantedBy, BoundBy, ConsistsOf, UpheldBy.
  - A collapsed "Ordering" group holds After and Before.
  - Empty lists show "(None)".
  - Double-click opens that unit's Properties window.
- Linux data/mechanism: Unit properties with the same names, all `as`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-015 Service logs (journal)
- Source: MC service_details_dialog Logs expander (last 5 messages, `UNIT=` or `_PID=`, current boot, copy button)
- Behaviour: Logs tab in Service Properties.
  - Monospace list of the last 200 entries from the current boot, newest at the bottom.
  - Each line shows time, priority and message. Priority 0-3 uses the theme danger colour; priority 4 uses warning.
  - Controls: filter box, "Follow" toggle (default on) for live tail, "Load older" (200 more per click), "Copy logs to clipboard", "Open in terminal". The last runs `journalctl -u <id> -b` in the default terminal through `xdg-terminal-exec`, falling back to `konsole -e`.
  - Mission Center shows 5 lines, which is too few to be useful.
- Linux data/mechanism: a `journalctl` subprocess run on the blocking pool, killed when the tab closes:
  - system unit: `journalctl -u <id> -b -n 200 -o json --no-pager`;
  - user unit: `--user-unit <id>`;
  - follow mode adds `-f`.
  The JSON fields used are `__REALTIME_TIMESTAMP`, `PRIORITY`, `MESSAGE` and `_PID`. Using the CLI avoids dlopen and unsafe FFI for a feature off the hot path.
- Verdict: ADAPT
- Phase: P2
- Privilege: reading other units' entries needs membership in `wheel`, `adm` or `systemd-journal` (Arch journal ACLs). Without it, only the user's own entries come back. The tab then shows a banner: "Only your own log messages are visible. Add your user to the systemd-journal group to see all."

### SYS-016 Services status bar and loading state
- Source: TS L28439, L28829 | MC placeholders
- Behaviour:
  - Status bar: "%zu Total | %d Running | %d Stopped". Running counts active units; Stopped counts inactive plus failed. Counts follow the current filters.
  - With a selection, it adds " | %d selected".
  - First load shows skeleton rows and the toast "Loading service data..." until both managers answer or fail.
  - F5 reloads the lists and shows "Services refreshed.".
- Linux data/mechanism: as SYS-001
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-017 Services sort and column persistence
- Source: TS INI Sort.ServicesKey=0/Ascending=1, Columns.Services, ColumnWidths.Services, ServiceVisibilityMask=7F | MC services-page-sorting-*, -column-order
- Behaviour: Default sort is Name ascending. Groups stay grouped while sorting. Column order, width and visibility are stored by column id (APP-033). Default widths in logical px: 250, 300, 100, 120, 80, 200, 260.
- Linux data/mechanism: `state.toml` `[columns.services]`, `[sort.services]`
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-018 OpenRC backend
- Source: MC PL:services/openrc (librc, `pkexec rc-service`, `rc-update`)
- Behaviour: On systems without systemd, the Services tab lists OpenRC services with Status from `rc-status`. Start, Stop and Restart call `rc-service`; enable and disable call `rc-update add|del <svc> default`. Properties shows only General. Logs, Recovery and cgroup columns are hidden.
- Linux data/mechanism: `rc-status --all --format ini`; `pkexec rc-service <svc> start|stop|restart`; `pkexec rc-update`
- Verdict: BUILD
- Phase: P4
- Privilege: pkexec (auth_admin) for actions; listing needs none

## Startup Apps tab

### SYS-020 Startup sources and precedence
- Source: TS 2.5 | PLAN data sources | review "Startup applications" row
- Behaviour: One row per effective entry, from these sources:
  1. XDG autostart, user: `$XDG_CONFIG_HOME/autostart/*.desktop`.
  2. XDG autostart, system: each dir in `$XDG_CONFIG_DIRS` (default `/etc/xdg`) + `/autostart`.
  3. systemd user units enabled into `default.target`, `graphical-session.target` or any `plasma-*.target`, from the user manager's WantedBy closure.
  4. KDE only: Plasma pre-startup scripts in `~/.config/plasma-workspace/env/*.sh`.
  5. KDE only: logout scripts in `~/.config/plasma-workspace/shutdown/*`.

  Precedence follows the XDG Autostart spec. Same file name: user beats system, and an earlier `XDG_CONFIG_DIRS` entry beats a later one. A shadowed system entry is shown once, with Type "Autostart (system, overridden)".
  An entry is effectively disabled when any of these hold:
  - `Hidden=true`;
  - `X-GNOME-Autostart-enabled=false`;
  - `OnlyShowIn`/`NotShowIn` excludes `$XDG_CURRENT_DESKTOP`;
  - `TryExec` is not found on PATH.
  Generated `app-*@autostart.service` units are not listed a second time under source 3; they are linked to their desktop entry.
- Linux data/mechanism: a desktop entry parser in tl-collect, written to the Desktop Entry spec. inotify on all autostart dirs and the two Plasma dirs gives live refresh. Units come from `ListUnitFilesByPatterns` and `ListUnitsByPatterns` on the user manager. Desktop entries map to units by escaping the id with systemd-escape rules: `app-<escaped id>@autostart.service`.
- Verdict: ADAPT (Run keys, Startup folders and packaged tasks become XDG autostart, systemd user units and Plasma scripts)
- Phase: P2
- Privilege: none

### SYS-021 Startup columns and values
- Source: TS 2.5 (Columns.StartupApps=0,1,2,6,3,4,5; widths 220,180,90,100,190,260,170) L28418-28446
- Behaviour: Columns, in display order: Name, Publisher, Status, Startup impact, Type, Location, Command.

  | Column | Value |
  |---|---|
  | Name | desktop `Name` (localised) or unit id |
  | Publisher | owning package as "name version"; "User" for files under $HOME not owned by a package; blank if unknown |
  | Status | see list below |
  | Startup impact | SYS-022 |
  | Type | see list below |
  | Location | directory path |
  | Command | `Exec`, `ExecStart` or the script path |

  Status values: Enabled, Disabled, Masked, Not in this desktop, Missing program. The last two replace TaskSlinger's "(Policy)" variants: entries whose state the user cannot get by toggling.
  Type values: Autostart (user), Autostart (system), Autostart (system, overridden), systemd user unit, Plasma pre-startup script, Plasma logout script.
- Linux data/mechanism: a package owner index in tl-collect, shared with the process Company column. It parses `/var/lib/pacman/local/*/files` once into a path-to-package map. The map is built lazily on first use and rebuilt when the mtime of `/var/lib/pacman/local` changes. On Debian-family systems it reads `/var/lib/dpkg/info/*.list` instead. Other distros leave Publisher blank.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-022 Startup impact
- Source: TS impact values L28976-28978; WDI StartupInfo source (cross-check A.32), which is why TaskSlinger needs admin
- Behaviour:
  - Values: High, Medium, Low, None, Not measured. TaskSlinger's "Requires administrator" never occurs, because systemd timing is readable unprivileged.
  - Thresholds on activation time: High at 1000 ms or more, Medium from 300 to 999 ms, Low under 300 ms.
  - None means the entry is disabled.
  - Not measured means the entry did not run through systemd this session. This happens with Plasma's non-systemd boot, Plasma scripts, or units that are inactive and were never activated since login.
  - The tooltip shows the raw activation time, "Started <s> after login", and CPU time used so far.
  - Documented limit: `Type=exec`/`simple` units count fork to exec, so most entries show Low.
  - TaskSlinger's status-bar suffix " | Startup impact requires administrator" is not used.
- Linux data/mechanism: the same data `systemd-analyze --user blame` uses, read over D-Bus. Per unit: `InactiveExitTimestampMonotonic` and `ActiveEnterTimestampMonotonic`. The start offset is `InactiveExitTimestampMonotonic` minus the Manager's `UserspaceTimestampMonotonic`. CPU time is `CPUUsageNSec`. Read once when the tab opens and again on UnitFilesChanged.
- Verdict: ADAPT (WDI CPU and disk usage become systemd activation time)
- Phase: P2
- Privilege: none

### SYS-023 Enable and disable
- Source: TS context Disable/Enable, toasts L28797-28808 | review "Hidden=true applies to desktop entries, not systemd units"
- Behaviour: The context item reads "Disable" or "Enable" by state, and works on multi-select. Toasts "Startup entry enabled." / "Startup entry disabled." / "Failed to update startup entry". Changes take effect at next login; running programs are not stopped. "Not in this desktop" and "Missing program" entries show the item disabled, with the reason as tooltip.
- Linux data/mechanism:
  - XDG system entry, disable: write `~/.config/autostart/<id>.desktop` as a full copy of the system file plus `Hidden=true` and `X-Tasklens-Override=true`. This is the KDE Autostart module's method.
  - XDG system entry, enable: delete that user file if it carries `X-Tasklens-Override=true`; otherwise set `Hidden=false` and remove `X-GNOME-Autostart-enabled=false`.
  - XDG user entry: toggle `Hidden` in place.
  - All writes are atomic: temp file in the same dir, fsync, rename. Unknown keys and comments are kept.
  - systemd user unit: `EnableUnitFilesWithFlags` / `DisableUnitFilesWithFlags` on the user manager, then `Reload()`.
  - Plasma scripts: toggle the executable bit (Plasma runs only executable scripts).
- Verdict: ADAPT
- Phase: P2
- Privilege: none, because system entries are overridden per user and never edited

### SYS-024 Run now
- Source: TS "Run now" (WdcRunTaskAsInteractiveUser, cross-check A.32), toasts "Startup entry launched." / "Failed to launch startup entry"
- Behaviour: Starts the entry now, whatever its Status. If it is already running as its autostart unit, the toast reads "Already running".
- Linux data/mechanism:
  - If `app-<id>@autostart.service` exists: `StartUnit(…, "replace")` on the user manager.
  - Otherwise: the shared launcher in tl-engine (also used by APP-014 Run). It parses `Exec` per the spec: field codes %f %F %u %U removed, %i/%c/%k expanded, quoting rules applied.
  - The launcher runs `posix_spawn` with `setsid`, with stdio set to /dev/null and cwd from `Path=`.
  - It then calls `StartTransientUnit("app-tasklens-<escaped id>-<random hex>.scope", "fail", [("PIDs",[pid]),("Description",Name),("CollectMode","inactive-or-failed")], [])` on the user manager, so app grouping works. A failed scope creation is logged and not fatal.
  - systemd unit: `StartUnit`. Script: spawned the same way through `/bin/sh <path>`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-025 Delete entry
- Source: TS "Delete entry..." and dialog "Delete startup entry" L29399-29404
- Behaviour: Confirm dialog (separate pinnable window, APP-029). Title "Delete startup entry", text "Permanently remove this startup entry?", a type-specific line, and buttons "Delete" (danger) and "Cancel". Type-specific lines:
  - user XDG entry: "The autostart file will be deleted. The app stays installed.";
  - user systemd unit: "The unit will be disabled and its unit file deleted.";
  - Plasma script: "The script file will be deleted.".
  Entries that cannot be deleted show the item disabled with the reason, and the toast "This startup entry cannot be deleted" if triggered by key. These are system XDG entries and package-owned units (use Disable instead), because pacman would restore them. Other toasts: "Startup entry deleted." / "Failed to delete startup entry".
- Linux data/mechanism: `unlink`. For a unit: `DisableUnitFiles`, then unlink the file in `~/.config/systemd/user`, then `Reload()`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-026 Startup context menu, copy and open
- Source: TS L29293-29297, copy format L35521-35528 (cross-check A.3), toasts "Startup command copied.", "Startup file location unavailable."
- Behaviour:
  - Menu: Disable/Enable, Run now, separator, Copy, Copy row, Copy command, Open file location, Properties, separator, Delete entry….
  - Copy row format:
    ```
    Name: …
    Publisher: …
    Status: …
    Startup impact: …
    Type: …
    Location: …
    Command: …
    Resolved path: …
    ```
    Resolved path is the absolute path of the binary found on PATH.
  - Open file location: FileManager1.ShowItems on the source file.
- Linux data/mechanism: as above
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-027 Startup Properties window
- Source: TS L28959-28972, title "TaskSlinger Startup Properties" L29326
- Behaviour: A separate pinnable window titled "Startup properties: <Name>", read-only, with a copy button per line. Lines:

  | Line | Content |
  |---|---|
  | Name | |
  | Publisher | |
  | Status, Enabled, State code | State code is the generated unit's ActiveState/SubState, or "-" |
  | Startup impact | with ms |
  | Type, Source, Scope | Scope replaces Hive: user or system |
  | Location | |
  | Source path | |
  | Resolved path | |
  | Command | |
  | Can run, Can toggle, Can delete, File exists | Yes/No each |

  Linux additions: OnlyShowIn/NotShowIn, TryExec, X-KDE-autostart-phase, generated unit name, "Overrides: <system path>" when a user file shadows one.
- Linux data/mechanism: parsed entry plus unit properties
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-028 Startup status bar, sort and persistence
- Source: TS L28437-28438; Sort.StartupAppsKey=2, StartupAppsStatusDefault=1; StartupVisibilityMask=7F
- Behaviour:
  - Status bar: "%zu Total | %d Enabled | %d Disabled", plus " | %d selected".
  - Default sort is Status ascending, so Enabled comes first, with Name ascending as tie-break.
  - Columns are persisted as in APP-033.
  - F5 rescans and shows "Startup entries refreshed.".
- Linux data/mechanism: `state.toml`
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Connections tab

### SYS-030 Connection inventory
- Source: TS 2.6 | PLAN data sources | review sampling table (connections every 2 s while consumed)
- Behaviour: Lists every TCP and UDP socket, IPv4 and IPv6, in tasklens's network namespace. Refreshes every 2 s while the tab is visible or the socket map is in demand (the process Network column shares it). Collection is off otherwise. Rows keep identity across refreshes by socket cookie, so selection is stable. Hold Ctrl pauses as everywhere.
- Linux data/mechanism:
  - Netlink `NETLINK_SOCK_DIAG`, `SOCK_DIAG_BY_FAMILY`, `inet_diag_req_v2`, for AF_INET and AF_INET6 × IPPROTO_TCP and IPPROTO_UDP.
  - `idiag_states=0xFFF`, with `INET_DIAG_INFO` requested for TCP.
  - Fields used: `id.{src,dst,sport,dport,if,cookie}`, `state`, `rqueue`, `wqueue`, `uid`, `inode`, and `tcp_info`.
  - Crates: `netlink-packet-sock-diag` and `netlink-sys`.
  - Unprivileged dumps work; the `inet_diag`/`udp_diag` modules load on demand.
- Verdict: ADAPT (TCP and UDP tables become sock_diag; IPv6 included)
- Phase: P2
- Privilege: none to list

### SYS-031 Connection columns, address formats, states
- Source: TS 2.6 L28422-28426, L27922-27939 (widths 90,260,260,120,80,180; Columns.Connections=0..5)
- Behaviour: Default columns: Proto, Address local, Address foreign, State, PID, Process. Optional columns: User, Family, Local port, Foreign port, Recv-Q, Send-Q, Interface, Inode, Service (owning systemd unit), Remote host (SYS-037).

  | Column | Value |
  |---|---|
  | Proto | "TCP" or "UDP" |
  | Address local / foreign | "a.b.c.d:port" or "[v6addr%scope]:port" |
  | State | see below |
  | PID | SYS-032 |
  | Process | SYS-032 |

  Address details:
  - Wildcards show as "0.0.0.0:*" and "[::]:*".
  - The foreign side of a listener or unconnected UDP socket shows "*:*".
  - IPv4-mapped IPv6 addresses show in IPv4 form.

  TCP states use TaskSlinger labels:

  | Kernel state | Label |
  |---|---|
  | LISTEN | LISTENING |
  | SYN_SENT | SYN-SENT |
  | SYN_RECV and NEW_SYN_RECV | SYN-RECEIVED |
  | ESTABLISHED | ESTABLISHED |
  | FIN_WAIT1 / FIN_WAIT2 | FIN-WAIT-1 / FIN-WAIT-2 |
  | CLOSE_WAIT | CLOSE-WAIT |
  | CLOSING | CLOSING |
  | LAST_ACK | LAST-ACK |
  | TIME_WAIT | TIME-WAIT |
  | CLOSE | CLOSED |
  | other | UNKNOWN |

  DELETE-TCB has no Linux state and is not used. UDP shows "UNCONNECTED" or "CONNECTED".
- Linux data/mechanism: as SYS-030; Interface from `if_indextoname(id.if)`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-032 Owning process attribution
- Source: TS PID/Process columns, "No owning process for this connection" | PLAN (pid map from /proc/*/fd every 2 s) | review "Network attribution" row
- Behaviour:
  - PID shows the owning pid. When several processes share the socket (after fork), it shows "pid +N" like the process table, and the tooltip lists all of them.
  - Process shows the display name and icon.
  - TIME-WAIT and orphaned sockets (inode 0): PID "-", Process "No owner (kernel)".
  - Sockets of other users whose `/proc/<pid>/fd` cannot be read: PID "?", Process shows the "restricted" marker. The tooltip reads "Owned by <user from diag uid>. Needs the tasklens helper to identify the process". The value is never blank and never 0.
- Linux data/mechanism:
  - Walk `/proc/<pid>/fd/*` with `readlinkat` and match `socket:[<inode>]`, producing inode to `ProcessKey{pid,start_ticks}`.
  - Rebuilt every 2 s while in demand, on the blocking pool, with one outstanding job at a time.
  - Budget target: under 20 ms per rebuild on this machine, measured in phase 0.
  - The same map feeds the process TCP Network column.
  - From P3, tl-helper resolves inodes for other users' processes: request is a list of inodes, reply is a list of pids, with CAP_SYS_PTRACE-equivalent root reads.
- Verdict: ADAPT
- Phase: P2 (own processes), P3 (helper for others)
- Privilege: none for own processes. Others need tl-helper (polkit `io.github.oysteinkrog.tasklens.inspect`, auth_admin_keep). The fallback is the restricted marker.

### SYS-033 Close TCP connection
- Source: TS "Close TCP connection" (SetTcpEntry, IPv4 only), toasts L28809-28814 | review "Unsupported feature parity" (defer TCP destruction)
- Behaviour:
  - Shown for TCP rows in any state except TIME-WAIT, for IPv4 and IPv6. TaskSlinger's "Only IPv4 TCP connections are supported" never appears.
  - LISTENING rows first ask for confirmation in a separate window. Title "Close listening socket?", text "The program will stop accepting new connections on <addr>.", buttons "Close socket" (danger) / "Cancel".
  - Toasts: "TCP connection close requested", then "TCP connection closed", or "Failed to close TCP connection: <errno text>".
- Linux data/mechanism: tl-helper performs `SOCK_DESTROY` (`SOCK_DESTROY` netlink message with `inet_diag_req_v2` filled from the exact 4-tuple, family, protocol and `id.cookie`). The kernel matches the cookie, so a reused 4-tuple cannot be hit. Kernel requirement `CONFIG_INET_DIAG_DESTROY=y` is verified on this machine. If the kernel returns EOPNOTSUPP, the menu item is disabled with tooltip "Kernel lacks CONFIG_INET_DIAG_DESTROY".
- Verdict: ADAPT
- Phase: P3 (needs tl-helper)
- Privilege: CAP_NET_ADMIN, through tl-helper with polkit action `io.github.oysteinkrog.tasklens.close-connection` (auth_admin_keep). Before the helper exists, the item is hidden. With the helper not installed, it is disabled with tooltip "Needs the tasklens helper".

### SYS-034 Connections context menu and copy formats
- Source: TS L29298-29301, copy format L35529-35536 (cross-check A.3), toasts L28657-28688
- Behaviour:
  - Menu: Copy row, Copy local address, Copy foreign address, separator, Close TCP connection, separator, Jump to process, Go to service (when the owner belongs to a systemd unit), Properties.
  - Copy row format:
    ```
    Protocol: …
    Local address: …
    Local port: …
    Foreign address: …
    Foreign port: …
    State: …
    PID: …
    Process: …
    ```
  - Toasts: "Connection copied", "Local address copied", "Foreign address copied".
- Linux data/mechanism: clipboard
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-035 Jump to owning process
- Source: TS L28711, L28812-28814
- Behaviour: Switches to Processes, expands the group or tree path and selects the owner by ProcessKey. Toast "Switched to owning process.". If filtered out: "Owning process not visible in the current process view.". With no owner: "No owning process for this connection". If the pid exited: "Process not found in current view".
- Linux data/mechanism: identity lookup in the process snapshot
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-036 Connection Properties window
- Source: TS L28960, L28979-28983, title "TaskSlinger Connection Properties" L29327
- Behaviour: A separate pinnable window titled "Connection properties: <Proto> <State>", with a copy button per line. Lines:
  - "Protocol / State / Family"
  - "Local <addr> Port <n>"
  - "Foreign <addr> Port <n>"
  - "Process <name> PID <n>" (link that jumps)
  - "User", "Inode", "Interface", "Recv-Q / Send-Q"
  - TCP only: "RTT <ms> (var <ms>)", "Congestion window", "Bytes sent / received" (`tcpi_bytes_acked`/`tcpi_bytes_received`), "Retransmits", "Last data received <ms> ago"
  - "Closable here: Yes/No (reason)"

  TaskSlinger's "Protocol rank / State rank" are internal sort keys with no meaning to the user. The tcp_info lines take their place. The window refreshes every 2 s while open, by cookie. When the socket closes, it shows "(closed)" and keeps the last values.
- Linux data/mechanism: `INET_DIAG_INFO` (`struct tcp_info`)
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-037 Remote host name column
- Source: none (tasklens addition replacing Windows netstat -f)
- Behaviour: Optional column "Remote host", hidden and off by default because lookups leak queries. When enabled, it shows the reverse DNS name, or the address while pending.
- Linux data/mechanism: `getnameinfo(NI_NAMEREQD)` on the blocking pool. At most 4 concurrent lookups. Positive answers are cached for 10 minutes, negative ones for 1 minute.
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### SYS-038 Connections status bar, filter, sort, refresh
- Source: TS L28435, Sort.ConnectionsKey=0 asc, ConnectionVisibilityMask=3F
- Behaviour:
  - Status bar: "%zu Total | %d TCP | %d UDP | %d EST | %d LISTEN". EST and LISTEN count TCP only.
  - The text filter matches substrings in Proto, both addresses with ports, State, PID, Process and User.
  - Default sort is Proto ascending, then local port ascending.
  - F5 refreshes now and shows "Connections refreshed.".
- Linux data/mechanism: engine projection
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-039 Other network namespaces
- Source: none (Linux gap; containers live in their own namespace)
- Behaviour: A "Namespaces" dropdown in the status bar, default "Host only". Choosing "All" adds a Namespace column (container or netns name) and lists sockets from every network namespace.
- Linux data/mechanism: tl-helper walks the unique `/proc/*/ns/net` targets, calls `setns(CLONE_NEWNET)` in a worker thread and dumps sock_diag there. Names come from `/run/netns` and from the cgroup of a member process.
- Verdict: ADAPT
- Phase: P4
- Privilege: root through tl-helper (polkit `io.github.oysteinkrog.tasklens.inspect`). Without it, only "Host only" is offered.

# APP: application shell

## Window chrome

### APP-001 Window decorations (CSD on KDE)
- Source: TS 1.1 | MC Adw header bar | gpui-component title_bar.rs:258, widgets §2
- Behaviour: By default tasklens draws its own 34 px title bar in TaskSlinger's layout (APP-002). The setting "Use system title bar" (`window.system_titlebar`, default false) switches to KWin server-side decorations, which takes a restart. In that mode, the menus and toolbar move into a 28 px row under the KWin title bar, and the caption buttons other than the pin are omitted. On compositors without xdg-decoration, GPUI uses client-side mode regardless.
- Linux data/mechanism: `WindowOptions{ window_decorations: Some(Client), ..TitleBar::window_options() }`, or `Server` when the setting is on. Root's WindowBorder supplies resize edges and the shadow. Wayland `app_id` is `io.github.oysteinkrog.Tasklens`, matching the installed `.desktop` file name, so KDE shows the icon.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-002 Title bar layout and toolbar
- Source: TS 1.1 [IMG processes.png, performance.png], cross-check A.8
- Behaviour: Left to right:
  1. app icon (16 px);
  2. menus File, View, Settings, Help;
  3. toolbar buttons, which depend on the tab;
  4. drag area;
  5. caption buttons: pin, minimize, maximize, close.

  Toolbar buttons by tab:
  - Every tab: Shield (APP-003, only once the helper exists) and Window picker "+" (APP-006).
  - Processes only: also Hide system processes (APP-004) and View mode (APP-005).
  - Mission Center's header buttons map elsewhere: Toggle Sidebar is a Performance-tab action (F9/Ctrl+T), and System Information is APP-041.

  Double-click on the drag area maximizes. Right-click opens the KWin window menu.
- Linux data/mechanism: gpui-component TitleBar (start_window_move, zoom_window, show_window_menu); toolbar buttons are ghost Buttons bound to Actions
- Verdict: ADAPT (update badge removed, APP-007)
- Phase: P1
- Privilege: none

### APP-003 Shield: unlock privileged actions
- Source: TS 1.1 `chrome.admin` (relaunch elevated), "Shift held at launch" L30398
- Behaviour: The icon is shield (locked) or shield-hi (unlocked). Click starts a tl-helper session (one polkit prompt), after which privileged actions run without further prompts until tasklens exits or 15 minutes pass idle. Click again to lock. Holding Shift during launch unlocks at start. Tooltip: "Unlock privileged actions" or "Privileged actions unlocked (click to lock)". tasklens never relaunches the GUI as root: a root GUI on Wayland breaks the session bus, portals and theming.
- Linux data/mechanism: `pkexec /usr/lib/tasklens/tl-helper --session`, under polkit action `io.github.oysteinkrog.tasklens.helper` (auth_admin_keep). Talks over an inherited socketpair. The helper allows only a whitelisted set of operations, with targets checked (review risk 8).
- Verdict: ADAPT
- Phase: P3
- Privilege: polkit auth_admin. The button is hidden when tl-helper is not installed.

### APP-004 Hide system processes button
- Source: TS 1.1 `chrome.microsoft`, toasts L28738-28739, INI UI.HideMicrosoftProcesses
- Behaviour: Toggle button, Processes tab only. Tooltip "Hide system processes" / "Show system processes". Toasts "Hiding system processes" / "Showing system processes". Which processes count as system is defined in the process spec (kernel threads, system.slice, root-owned daemons).
- Linux data/mechanism: setting `processes.hide_system_processes`
- Verdict: ADAPT (Microsoft signer test becomes a Linux system classification)
- Phase: P1
- Privilege: none

### APP-005 View mode button
- Source: TS `chrome.view_mode`, toast "View mode: %ls" L28638, cross-check B.1
- Behaviour: Processes tab only. Each click cycles Task Manager Grouping, Tree View, List View, the same three choices as View > Process View. Toast "View mode: Tree View" and so on.
- Linux data/mechanism: sets `processes.tree_view` and `processes.grouping`
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-006 Window picker "+"
- Source: TS 7 window picker, tooltips L29201-29213, cross-check A.7
- Behaviour:
  - Click "+": KWin asks the user to click a window.
  - Plain click: tasklens selects that window's process in Processes.
  - Ctrl+click: tasklens ends that process with SIGTERM, then gives a toast.
  - Esc cancels.
  - Toasts: "No window selected", "No window found".
  - Desktop and panel windows (plasmashell) are ignored, with "No window found".
  - Hidden on non-KDE Wayland sessions.
- Linux data/mechanism:
  - KDE: session D-Bus `org.kde.KWin /KWin org.kde.KWin.queryWindowInfo()`, which does interactive selection and returns a map with `uuid`.
  - Then a short KWin script, loaded through `org.kde.kwin.Scripting.loadScript(path, "tasklens-pick")` and run, looks up `workspace.windowList()` by `internalId == uuid` and returns `pid` to tasklens over D-Bus.
  - X11: `XGrabPointer` crosshair, then `_NET_WM_PID`.
- Verdict: ADAPT
- Phase: P3
- Privilege: none
- Open question: whether the `queryWindowInfo` map already contains `pid` on Plasma 6.x. Recommended answer: check in phase 0 and use the script path only if `pid` is absent.

### APP-007 Update badge
- Source: TS `chrome.update_available` [CL 0.8.15, 0.8.18]
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (pacman and the localbuilds recipe update tasklens, so there is no in-app updater)
- Phase: P4 (recorded as dropped)
- Privilege: none

### APP-008 Caption buttons
- Source: TS 1.1 `chrome.caption.*` [CL 0.8.28]
- Behaviour: Pin (always on top, accent fill when on), Minimize, Maximize/Restore, Close. Minimize and maximize appear when the compositor reports them (`window_controls()`). Close follows the close behaviour in APP-047.
- Linux data/mechanism: TitleBar controls; the pin is a custom button bound to the ToggleAlwaysOnTop action
- Verdict: BUILD
- Phase: P1 (pin P2)
- Privilege: none

### APP-009 Always on top
- Source: TS View > Window > Always On Top L28493, INI Window.AlwaysOnTop | review (defer where unsupported)
- Behaviour: Toggled by the pin button, View > Window > Always On Top, and the property windows' pins. The state persists across restarts for the main window only. On compositors with no mechanism, the item is disabled with tooltip "Not supported by this desktop".
- Linux data/mechanism:
  - KWin Wayland: a KWin script loaded through `org.kde.kwin.Scripting.loadScript`. It finds the window by pid and `resourceClass == "io.github.oysteinkrog.Tasklens"`, plus the window caption for secondary windows, and sets `keepAbove`.
  - X11: `_NET_WM_STATE_ABOVE` client message.
  - The KWin scripting channel lives in tl-ui's KDE integration module, shared with APP-006 and APP-012.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-010 Size, minimum size and geometry restore
- Source: TS INI Window.Width/Height/X/Y/Maximized (1280×800), minimum size [CL 0.9.2] | MC window-width/height/is-maximized, min 360×400 | gpui APIs (Wayland position cannot be restored)
- Behaviour: Default size 1280×800 logical px. Minimum 760×480 logical px, so the six tabs and the filter box fit. Width, height and maximized state are saved, debounced 500 ms, and restored. Position is never saved; the compositor places the window. KWin quick tiling and snapping work as they would for any window, so Windows 11 snap layouts need no work.
- Linux data/mechanism: `window_min_size`, `observe_window_bounds`, `window_bounds()`; saved to `state.toml [window]`
- Verdict: ADAPT (position not restored)
- Phase: P1
- Privilege: none

### APP-011 Single instance
- Source: TS L35762, L35794 [CL 0.8.12] | MC GApplication single instance | review "Window lifecycle"
- Behaviour: A second launch hands its arguments (`--tab`, `--select-pid`, `--start-minimized` is ignored) to the running copy and exits with code 0. The running copy shows, raises and focuses its main window.
- Linux data/mechanism:
  - At startup, request the session bus name `io.github.oysteinkrog.Tasklens` with `DO_NOT_QUEUE`.
  - If it is taken, call `org.freedesktop.Application.CommandLine`-style method `io.github.oysteinkrog.Tasklens.Activate(argv: as, platform_data: a{sv})` on the owner. `platform_data` carries `activation-token` from `XDG_ACTIVATION_TOKEN`.
  - The owner calls `window.activate_window()`. On KDE it also activates through the KWin script (`workspace.activeWindow = w`), because GPUI requests its own token, which KWin may refuse.
  - Without a session bus, fall back to an `flock` on `$XDG_RUNTIME_DIR/tasklens.lock` with a Unix socket next to it.
- Verdict: ADAPT
- Phase: P1
- Privilege: none

## Menus

### APP-012 File menu
- Source: TS 1.2
- Behaviour: Items:
  - "Run..." (Ctrl+N), see APP-014
  - "Search File Handles...", see APP-015
  - "Open System Monitor", see APP-016
  - separator
  - "Exit tasklens" (Ctrl+Q)
  Menu items appear only once their feature exists, so the menu never has dead items.
- Linux data/mechanism: gpui-component PopupMenu from the title bar; keyboard hints come from the Actions
- Verdict: ADAPT
- Phase: P1 (Exit), later items in their own phase
- Privilege: none

### APP-013 Run Installer item
- Source: TS 1.2 L28475
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (TaskSlinger installer front end; pacman installs tasklens)
- Phase: P4 (recorded)
- Privilege: none

### APP-014 Run dialog
- Source: TS 8 "TaskSlinger Run" L29321-29338, runCommandHistory.json, toasts "Command launched", "Command history cleared", "Please enter a command", "Command contains invalid text.", "Failed to run command"
- Behaviour:
  - A separate pinnable window titled "tasklens Run".
  - Text: "Type the name of a program, folder, document, or Internet resource.", then "Open:" and a combo whose dropdown shows the last 5 history matches.
  - Keys: Up and Down pick a history match, Enter runs, Shift+Enter runs as administrator.
  - Buttons: "Browse..." (portal file chooser, filter "Programs" = executable files, and "All files"), "Clear command history", "Run as administrator", "Run" (default), "Cancel".
  - Invalid text means a NUL byte or a control character.
- Linux data/mechanism:
  - URLs and existing paths that are not executable go to `xdg-open`.
  - Anything else runs as `/bin/sh -c <text>` through the shared launcher (SYS-024), in a transient `app-tasklens-run-<rand>.scope`.
  - Run as administrator: `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c <text>`. This uses polkit `org.freedesktop.policykit.exec` with standard auth_admin. Plain pkexec, not tl-helper, so the helper never offers arbitrary shell.
  - History goes to `$XDG_STATE_HOME/tasklens/run-history.json` in the format `{"entries":[{"command","executedAt","elevated"}]}`, up to 50 entries.
  - The browse dialog uses `org.freedesktop.portal.FileChooser.OpenFile` through zbus.
- Verdict: ADAPT
- Phase: P2
- Privilege: none, or pkexec for Run as administrator. A cancelled prompt gives "Authentication cancelled".

### APP-015 Search File Handles dialog (menu entry)
- Source: TS 8 "Search File Handles" L29339-29345, L30071-30081
- Behaviour:
  - A separate window titled "tasklens Handle Search", with field "Pattern:" (case-insensitive substring), results list, "Clear" and "Close".
  - Result columns: Process, PID, Type (file, socket, pipe, anon_inode, memfd, mapped), Path.
  - Progress text: "Retrieving file handles... %d%% (%d total handles)", "Found %d matches", "No results found.".
  - Result menu: "End Process", "Go to process".
  - The process spec owns the result-row actions.
- Linux data/mechanism: walk `/proc/*/fd` (readlink) and `/proc/*/maps` on the blocking pool, cancellable
- Verdict: ADAPT
- Phase: P3
- Privilege: own processes need none. Others need tl-helper (`…tasklens.inspect`), and their rows show "(restricted: N processes skipped)" in the footer.

### APP-016 Launch other system monitor
- Source: TS "Launch Windows Task Manager" L28474, toast L28639
- Behaviour: The item "Open System Monitor" starts the desktop's own monitor. It is hidden when none is installed. Failure gives the toast "Failed to launch System Monitor".
- Linux data/mechanism: the first found of `plasma-systemmonitor`, `gnome-system-monitor`, `missioncenter`, run through the shared launcher
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### APP-017 View menu structure
- Source: TS 1.3
- Behaviour: Submenus, in order: "Columns >", "Process View >", "Window >", "Appearance >", "Theme >", "Graphs >".
- Linux data/mechanism: PopupMenu submenus
- Verdict: BUILD
- Phase: P1 (Columns, Process View, Window), P2 (Appearance, Theme), P3 (Graphs)
- Privilege: none

### APP-018 View > Columns (all tables)
- Source: TS 1.3, header menu L29313-29314, L28434, L28484-28487, toasts L28648, L28652, cross-check A.10 (picker group headings and alias text)
- Behaviour: Applies to the active tab's table (Processes, History, Services, Startup Apps, Connections). The same items open from a right-click on a column header.
  - "Select Columns…": checklist with a filter field, grouped under headings. Process groups: Image, Objects, Service, Connection, per the process spec. Entries show alias text, e.g. `Resident memory - "Memory"`.
  - "Hide This Column" (header menu only).
  - "Resize columns to fit content": measures template strings: "999999 +99" (PID), "100.0%", "xxx Mbps", "xxx GB/s", "999:59:59".
  - "Resize Behavior: Proportional / Last Column".
  - "Reset to default".
  - Toasts: "At least one column must stay visible", "Columns reset to default".
- Linux data/mechanism: tl-ui table state, persisted per APP-033
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-019 View > Process View
- Source: TS 1.3 L28488-28492
- Behaviour: Check items: "Tree View", "Task Manager Grouping", "List View", "Tree Connector Lines", "Highlight New and Exited Processes". Behaviour is defined in the process spec.
- Linux data/mechanism: settings in APP-032
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-020 View > Window
- Source: TS L28493
- Behaviour: "Always On Top" (check), see APP-009
- Linux data/mechanism: APP-009
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-021 View > Graphs
- Source: TS 1.3 L28470-28471, L28483, L28543-28544
- Behaviour:
  - Checks: "CPU Graph", "Memory Graph", "Disk Graph", "Network Graph", "GPU Graph", "VRAM Graph", "GPU Temperature Graph".
  - Then "Keep detached graphs open when closing main window" and "Reset Minigraph Layout" (toast "Minigraph layout reset").
  - The mini-graph spec owns what these graphs show and how they behave.
- Linux data/mechanism: settings `graphs.*`
- Verdict: BUILD
- Phase: P3 (CPU to Network toggles P2 together with the mini-graph panel)
- Privilege: none

### APP-022 Settings menu
- Source: TS 1.4 L28494-28536, cross-check B.7
- Behaviour: Group headings and items, in order:
  - **Monitoring**
    - "Refresh >": 0.25s, 0.5s, 1.0s, 2.0s, 5.0s, 10.0s
    - "Network transfer unit >": Bits per second / Bytes per second
    - "Auto-scale network transfer rates"
    - "Record history"
    - "History storage >": 1, 2, 3, 4, 6, 8, 12 hours
    - "Save history to disk"
    - "Rendering GPU >" (APP-046)
  - **Motion & interaction**
    - "Animations", "Smooth Graphs", "Smooth Scroll"
    - "Tab Switch" slider (0-500 ms, label "%d ms")
    - "Close with Esc"
  - **Window & startup**
    - "Close to tray", "Minimize to tray", "Always show tray icon"
    - "Start at login", then an indented "Start minimized" (disabled unless Start at login is on)
    - "Default tab >"
    - "Unlock privileged actions at launch"
    - "Skip password prompts (experimental)"
    - "Auto-fetch Public IP on launch" (moved here per cross-check B.7)
  - **Language**: "Language: >" with System default, then the locales (APP-043)
  - Bottom: "All settings…" (Ctrl+,), "Reset Settings to Defaults"

  "Reset Settings to Defaults" opens a confirm window: "Reset all settings to their defaults? Themes and history are kept.", buttons "Reset" / "Cancel". Then toast "Settings reset to defaults", or "Failed to reset settings".
- Linux data/mechanism: PopupMenu; every item writes through the settings store (APP-032)
- Verdict: ADAPT
- Phase: P1 (Refresh, units, Default tab, Close with Esc), others in their feature's phase
- Privilege: none

### APP-023 Preferences window ("All settings…")
- Source: MC Preferences dialog (General, Performance Page, Apps & Services, Units) | TS has menus only
- Behaviour: A separate window titled "tasklens Settings", using the gpui-component `Settings` widget (page sidebar plus search). Pages: General, Appearance, Motion, Units, Processes, Performance, Graphs, Services, Startup and window, Privileges, Advanced. Every key in APP-032 appears exactly once, with the same label as its menu item where one exists. Changes apply live. TaskSlinger's quick menus are kept; this window adds the settings that have no menu item, mostly from Mission Center.
- Linux data/mechanism: gpui-component Settings/SettingPage/SettingField bound to the settings Global
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-024 Help menu
- Source: TS 1.5 | MC main menu (Keyboard Shortcuts, Open Wiki, Show First Run Dialog, About)
- Behaviour: Items:
  - "Keyboard Shortcuts" (Ctrl+?), see APP-030
  - "System Information…", see APP-041
  - "Additional Setup…", see APP-042
  - "Documentation", which opens `/usr/share/doc/tasklens/README.md` with xdg-open; this replaces Mission Center's "Open Wiki"
  - separator
  - "About tasklens…"
- Linux data/mechanism: PopupMenu
- Verdict: ADAPT
- Phase: P2 (Documentation P4)
- Privilege: none

### APP-025 Purchase license, Enter license, early-bird notice
- Source: TS 1.5 L28537-28539, 8 License and Early-bird dialogs, cross-check A.35, INI License.Token, WingetEarlyBirdNoticeShown
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (personal tool with no licence system; purchase flow is TaskSlinger's commercial model)
- Phase: P4 (recorded)
- Privilege: none

### APP-026 Debug menu
- Source: TS 1.6, hidden INI keys ShowFps/ShowDebugWindow/TripleBuffering
- Behaviour: Shown only with `--debug` or `TASKLENS_DEBUG=1`. Items:
  - "Show FPS": overlay of frame time p50/p99 and fps.
  - "Continuous rendering": requests a frame every vsync.
  - "Enable all columns".
  - "Add random process minigraphs": toasts "Added %ld random process minigraphs." / "No process minigraphs available.".
  - "Collector health": window listing each collector's state, age, last error and restart count. This is the Linux analogue of ShowDebugWindow.
  Dropped items:
  - Force D3D feature level: Direct3D only.
  - Triple buffering: wgpu present mode is fixed to Mailbox.
  - Clear text atlas cache: GPUI has no public API for it.
- Linux data/mechanism: GPUI `request_animation_frame`; engine health snapshot
- Verdict: ADAPT
- Phase: P4 (Show FPS in P1 as a developer-only flag)
- Privilege: none

## Tabs, filter, status bar

### APP-027 Tab bar and tab animation
- Source: TS 1.7 L28210-28216, INI MainTabAnimationMs=250, Startup.DefaultTab | MC window-selected-page
- Behaviour:
  - Tabs in order: Processes, Performance, Services, Startup Apps, Connections, Information. These map to Mission Center as Apps to Processes, plus Performance and Services.
  - Processes carries a history icon button that toggles History mode; in History mode the tab label reads "History" (cross-check B.3). Information carries a copy icon (copy all).
  - The active tab is an accent outline joined to a 2 px accent line along the bar.
  - A switch slides the indicator and crossfades content over `motion.tab_switch_ms` (default 250). It is instant when Animations is off or the system asks for reduced motion.
  - Ctrl+1 to Ctrl+6 and Alt+1 to Alt+6 select tabs.
  - When narrow, overflowing tabs go into a "»" menu (TabBar `.menu(true)`). There is no Mission Center bottom tab bar.
  - The startup tab follows `startup.default_tab`.
- Linux data/mechanism: gpui-component TabBar with custom Outline styling; GPUI `Animation` (respects `reduce_motion()`)
- Verdict: BUILD
- Phase: P1 (animation P2)
- Privilege: none

### APP-028 Filter box
- Source: TS 1.7 L28339, L30133 [CL 0.8.28, 0.8.32] | MC search (typing forwards to search; name/PID; Levenshtein) | review "Sort/filter execution"
- Behaviour:
  - Sits at the right of the tab bar: "Filter: " label, input and clear button. 160 px wide, growing to 280 px on focus over 150 ms.
  - Typing any printable character while a table has focus, and no other input or dialog has it, starts filtering.
  - Ctrl+F focuses the box. The first Esc clears it; the second returns focus to the table.
  - Each tab keeps its own filter text for the session.
  - Matching is case-insensitive substring. Columns searched: Processes per the process spec (name, PID, command line, user, description, group names); Services and Startup over all visible columns; Information over field labels and values.
  - Mission Center's reverse-containment and Levenshtein matching is rejected because it gives false hits.
  - "Include children" checkbox: Processes only, shown while filtering or in Tree View (process spec).
  - Execution is debounced 100 ms in the engine projection, with results tagged by generation.
- Linux data/mechanism: gpui-component Input `.cleanable(true)`; normalized name and command-line cache in the engine
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-029 Status bar
- Source: TS 1.8 [CL 0.8.9]
- Behaviour: Compact bar, 22 px at 100% scale.
  - Left: the tab's own text. Processes: "%zu Processes" or "%zu / %zu Processes", plus " | %d processes selected". Services, Startup and Connections: SYS-016, SYS-028, SYS-038. Performance and Information: blank.
  - While Ctrl is held: "[PAUSED - Ctrl]" (appended in the accent colour).
  - Right: collector health. Nothing when all is well; otherwise "GPU data stale", "Restricted: 42 processes" and so on, each clickable to open Collector health.
- Linux data/mechanism: gpui-component StatusBar
- Verdict: ADAPT (right-side health text added per the review's validity rules)
- Phase: P1
- Privilege: none

### APP-030 Keyboard Shortcuts window
- Source: MC Ctrl+? shortcuts dialog
- Behaviour: A separate window listing APP-031 by context (Global, Tables, Processes, Performance, Services, Dialogs), with key caps from `Kbd::binding_for_action`.
- Linux data/mechanism: generated from the keymap
- Verdict: BUILD
- Phase: P2
- Privilege: none

### APP-031 Keyboard shortcuts (merged)
- Source: TS 7 | MC 6 | cross-check C.4
- Behaviour: Final map. Each command is one Action.

  | Keys | Context | Action | Origin and conflict resolution |
  |---|---|---|---|
  | Ctrl+1 to Ctrl+6, Alt+1 to Alt+6 | Global | Select tab in TaskSlinger order | MC 1/2/3 = Performance/Apps/Services; TS tab order wins |
  | Ctrl+N | Global | Run… | TS |
  | Ctrl+Q | Global | Exit tasklens | MC |
  | Ctrl+W | Global | Close window (follows close behaviour) | MC |
  | Ctrl+, | Global | All settings… | MC |
  | Ctrl+? (Ctrl+Shift+/) | Global | Keyboard Shortcuts | MC |
  | F10 | Global | Open File menu (keyboard menu navigation) | MC "open main menu" |
  | Alt+F / Alt+V / Alt+S / Alt+H | Global | Open File / View / Settings / Help | new; no conflict |
  | Ctrl+F | Global | Focus filter | MC |
  | F5 | Global | Refresh current tab now | new; TS refresh toasts |
  | Esc | Global | Close topmost dialog or menu, else clear filter, else close window if Close with Esc is on, else nothing | TS order made explicit |
  | Hold Ctrl | Global | Pause display ("[PAUSED - Ctrl]"); releases on key up or focus loss | both |
  | Ctrl+Plus / Ctrl+Minus / Ctrl+0 | Global | UI scale +5% / -5% / 100% | new; toast "UI scale changed to %d%%" |
  | Up / Down / PgUp / PgDn / Home / End | Tables | Move row selection | Home and End select rows, not columns (gpui-component quirk overridden) |
  | Shift+arrows, Ctrl+click, Shift+click | Tables | Extend selection | TS |
  | Ctrl+A | Tables | Select all visible rows | MC Ctrl+A = System Information; select all wins, and System Information has no shortcut |
  | Right / Left | Tables | Expand, or collapse and go to parent | MC |
  | Enter | Tables | Toggle expand on rows with children | MC |
  | Alt+Enter, Ctrl+I | Tables | Properties | TS Properties plus MC Ctrl+I |
  | Ctrl+C | Tables | Copy selected rows (tab-separated, with header) | MC Ctrl+C = copy device info; that stays on Performance only |
  | Ctrl+L | Tables | Collapse all | MC |
  | Shift+F10, Menu key | Tables | Open context menu for the selection | new; MC lacks it |
  | Delete, Ctrl+E | Processes | End Task (SIGTERM) | MC Ctrl+E Stop |
  | Shift+Delete, Ctrl+X | Processes | Force end (SIGKILL) | MC Ctrl+X Force Stop |
  | Ctrl+S / Ctrl+E / Ctrl+R | Services (service row) | Start / Stop / Restart | MC; on child process rows Ctrl+E acts on the process |
  | Ctrl+C | Performance | Copy device summary | MC |
  | Ctrl+M | Performance | Graph Summary View | MC |
  | F9, Ctrl+T | Performance | Toggle device sidebar | MC |
  | F1 to F8 | Performance | Select visible device 1 to 8 | MC, with its hidden-row bug fixed |
  | Up/Down, Enter, Shift+Enter | Run dialog | History pick, Run, Run as administrator | TS |
  | Esc | Window picker | Cancel picking | TS |
  | Shift held at launch | Startup | Unlock privileged actions | TS |
- Linux data/mechanism: `cx.bind_keys` with key contexts "Global", "Table", "ProcessTable", "ServiceTable", "Performance", "RunDialog". Ctrl-hold is detected with `on_modifiers_changed`, and window deactivation releases the pause.
- Verdict: ADAPT
- Phase: P1 (global, table and process keys), P2 (rest)
- Privilege: none

## Settings and persistence

### APP-032 Settings registry (every setting)
- Source: TS 5 (UserSettings.ini) | MC 5 (gschema, 53 keys) | review "Settings persistence"
- Behaviour: One tasklens key per setting. Keys marked "state" live in `state.toml`; the rest live in `settings.toml`.

  **Theme**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | theme.active | string | "Default Dark" | theme name | TS UI.Theme, ThemeName; MC window-interface-style light/dark |
  | theme.follow_system | bool | false | | TS ThemeSyncWithWindows; MC window-interface-style=system |
  | theme.light | string | "Default Light" | | TS LightTheme/LightThemeName |
  | theme.dark | string | "Default Dark" | | TS DarkTheme/DarkThemeName |

  **General and appearance**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | general.language | string | "system" | system, en-US, de-DE, es-ES, fr-FR, ja-JP, ko-KR, pt-BR, ru-RU, uk-UA, zh-CN, nb-NO | TS UI.Language |
  | appearance.ui_scale_pct | u16 | 100 | 75-200, step 5 | TS UIScale |
  | appearance.row_height_pct | u16 | 100 | 80-150, step 5 | TS RowHeightPct |
  | appearance.rounded_corners | bool | true | | TS RoundedCorners |
  | appearance.heat_coloring | bool | true | | TS ProcessValueColorCoding |
  | appearance.heat_intensity_pct | u8 | 50 | 0-100 | TS ProcessValueColorCodingIntensityPct |
  | appearance.shadow_intensity_pct | u8 | 100 | 0-100 | TS DropShadowIntensityPct |

  **Monitoring and units**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | monitoring.refresh_interval_ms | u32 | 1000 | 250-10000, step 50; menu offers 250/500/1000/2000/5000/10000 | TS RefreshIntervalMs; MC app-update-interval-u64 (×50 ms) |
  | units.network_bits | bool | true | | TS ProcessNetworkRateUnit=0; MC network-use-bytes=false |
  | units.network_base2 | bool | false | | MC network-use-base2 |
  | units.network_auto_scale | bool | false | off = fixed Mbps or MB/s | TS NetworkRateAutoScale |
  | units.memory_bits | bool | false | | MC memory2-use-bytes |
  | units.memory_base2 | bool | true | | MC memory2-use-base2 |
  | units.disk_bits | bool | false | | MC drive-use-bytes |
  | units.disk_base2 | bool | true | | MC drive-use-base2 |

  **History**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | history.enabled | bool | false | | new (review: recording off by default) |
  | history.retention_hours | u8 | 6 | 1-12 | TS HistoryStorageDurationHours |
  | history.persist | bool | true | applies when enabled | TS HistoryPersistToDisk |

  **Motion**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | motion.animations | bool | true | ANDed with system reduced motion | TS AnimationsEnabled |
  | motion.smooth_graphs | bool | true | sub-sample scrolling | TS SmoothGraphs; MC performance-sliding-graphs |
  | motion.smooth_scroll | bool | true | | TS SmoothScrolling |
  | motion.smooth_scroll_amount | u8 | 15 | 1-50; scroll easing = 10 ms × value | TS SmoothScrollAmount |
  | motion.tab_switch_ms | u16 | 250 | 0-500, step 10 | TS MainTabAnimationMs |

  **Window, tray, startup, network, privileges, rendering**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | window.close_with_esc | bool | false | | TS CloseWithEsc |
  | window.always_on_top | bool | false | | TS Window.AlwaysOnTop |
  | window.system_titlebar | bool | false | | new (APP-001) |
  | tray.close_to_tray | bool | false | | TS CloseToTray |
  | tray.minimize_to_tray | bool | false | | TS MinimizeToTray |
  | tray.always_show | bool | false | | TS AlwaysShowTrayIcon |
  | tray.graphs | list | [] | mode, source, color, range | TS TrayIconGraphs.* |
  | startup.start_at_login | bool | false | | TS StartWithWindows |
  | startup.start_minimized | bool | false | | TS StartMinimized |
  | startup.default_tab | enum | processes | processes, performance, services, startup, connections, information, last_used | TS DefaultTab; MC window-selected-page (as last_used) |
  | network.auto_fetch_public_ip | bool | false | | TS Connections.AutoFetchPublicIP |
  | network.public_ip_notice_accepted | bool | false | | TS PublicIPFetchNoticeAccepted |
  | privilege.unlock_at_launch | bool | false | | TS AlwaysLaunchElevated (ADAPT) |
  | privilege.passwordless | bool | false | | TS UacLessElevation (ADAPT, APP-049) |
  | rendering.gpu | string | "auto" | auto, software, pci:<bus id> | TS Graphics.GpuSelectionKind/GpuAdapterId/GpuAdapterName/GpuPreference |

  **Tables**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | tables.column_resize | enum | proportional | proportional, last_column | TS ColumnResizeBehavior |
  | tables.remember_sort | bool | true | | MC apps-page-remember-sorting (false); TS always remembers, TS wins |
  | tables.remember_columns | bool | true | | MC apps-page-remember-column-order |
  | tables.column_separators | bool | false | | MC apps-page-show-column-separators |
  | tables.dim_zero_values | bool | false | 0.3 opacity | MC apps-page-hide-is-zero |

  **Processes**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | processes.tree_view | bool | true | | TS TreeView |
  | processes.grouping | bool | true | | TS TaskManagerGrouping |
  | processes.tree_guide_lines | bool | false | | TS TreeGuideLines |
  | processes.highlight_new_exited | bool | true | | TS HighlightNewExitedProcesses |
  | processes.filter_include_children | bool | false | | TS FilterShowChildren |
  | processes.hide_system_processes | bool | false | | TS HideMicrosoftProcesses |
  | processes.column_graphs | list | [cpu, memory, disk, network, gpu, vram] | | TS ProcessGraphColumnMask(+Extra) |
  | processes.merge_child_stats | bool | false | | MC apps-page-merged-process-stats |
  | processes.cpu_one_core_100 | bool | false | false = share of whole machine | MC apps-page-core-count-affects-percentages |

  **Process properties**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | properties.hide_unnamed_handles | bool | true | | TS HideUnnamedHandles |
  | properties.shared_objects_only | bool | true | | TS ModulesDllOnly |

  **Graphs**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | graphs.minigraph_cpu / _memory / _disk / _network | bool | true | | TS ShowCpuGraph/ShowRamGraph/ShowDiskGraph/ShowNetworkGraph |
  | graphs.minigraph_gpu / _vram | bool | true | | TS screenshot (inferred) |
  | graphs.minigraph_gpu_temperature | bool | false | | TS ProcessGraphs.ShowGpuTemperatureGraph |
  | graphs.keep_detached_on_close | bool | false | | TS KeepDetachedOpenOnMainClose |
  | graphs.time_range_s | u16 | 60 | 30, 60, 120, 300, 600 | MC performance-page-data-points (× interval) |
  | graphs.curved_lines | bool | false | monotone cubic, no overshoot | MC performance-smooth-graphs |
  | graphs.network_dynamic_scale | bool | true | false = link speed | MC performance-page-network-dynamic-scaling |

  **Performance**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | performance.cpu_graph_mode | enum | per_core | overall, per_core, all_threads_overlay, all_threads_stacked | TS CpuPerCore; MC performance-page-cpu-graph |
  | performance.cpu_kernel | bool | true | | TS CpuKernel; MC performance-page-kernel-times |
  | performance.cpu_secondary_graph | enum | power | none, power, clock, temperature | MC performance-page-cpu-graph-bottom |
  | performance.gpu_graph_middle | enum | encode_decode | none, encode_decode, memory, power, clocks, temperature | MC gpu-graph-middle |
  | performance.gpu_graph_bottom | enum | memory | same values | MC gpu-graph-bottom |
  | performance.memory_composition | bool | true | | MC memory-composition-visible |
  | performance.memory_swap_graph | bool | true | | MC memory-swap-visible |
  | performance.show_disks / _network / _gpus / _fans / _batteries | bool | true | | MC performance-show-* |
  | performance.show_network_wired / _wireless / _vpn / _virtual / _other | bool | true | | MC performance-show-network-* |
  | performance.device_order | list | [] | device ids | TS Columns.Performance; MC performance-sidebar-order |
  | performance.device_overrides | map | {} | id: show or hide | TS Performance.HiddenPages; MC performance-sidebar-device-overrides |
  | performance.hidden_gpu_engines | list | [] | | TS HiddenGpuEngines |

  **Services and debug**

  | tasklens key | Type | Default | Range / values | From |
  |---|---|---|---|---|
  | services.unit_types | list | [service, socket, timer, mount] | | new (SYS-002) |
  | services.filter_states | list | [] | running, failed, stopped, disabled | new (SYS-004) |
  | debug.show_fps | bool | false | | TS ShowFps |
  | debug.show_health_window | bool | false | | TS ShowDebugWindow |

  **State (state.toml)**

  | tasklens key | Type | Default | From |
  |---|---|---|---|
  | state.window | width, height, maximized | 1280, 800, false | TS Window.Width/Height/Maximized; MC window-width/window-height/is-maximized |
  | state.last_tab | enum | processes | MC window-selected-page |
  | state.sort.<table> | column id, ascending | processes pid asc; history cpu desc; services name asc; startup status asc; connections proto asc | TS Sort.*; MC *-sorting-column-name / -sorting-order |
  | state.columns.<table> | ordered list of {id, width, visible} | per spec | TS Columns.*, ColumnWidths.*, *VisibilityMask, ProcessColumnVisibilityMask/ExtraMask; MC *-column-order |
  | state.processes.history_mode | bool | false | TS ProcessesHistoryMode |
  | state.processes.group_expanded.{apps,background,system} | bool | true | TS TaskManagerAppsExpanded/BackgroundExpanded/WindowsExpanded |
  | state.services.group_expanded.{system,user} | bool | true | new |
  | state.performance.nav_width | u16 | 252 | TS Performance.NavWidth |
  | state.performance.selected_device | string | "cpu" | MC performance-selected-page |
  | state.minigraphs | list | [] | TS ProcessGraphs.Column%d, DetachedProcessGraphs, ProcessGraphColors, ProcessGraphTimeRanges; TS ProcessSpecificMiniGraphs.* |
  | state.first_run_done | bool | false | MC first-time-running |

  Keys not carried over:
  - TS Window.X/Y: Wayland cannot restore position (APP-010).
  - TS Crash.DumpType: systemd-coredump handles native dumps (APP-045).
  - TS Updates.CheckForUpdatesOnStartup, Install.*, License.Token, WingetEarlyBirdNoticeShown: DROP (APP-025, APP-050).
  - TS TripleBuffering: DROP (APP-026).
  - MC app-update-interval: already deprecated in Mission Center.
  - MC performance-show-cpu / performance-show-memory: never read by Mission Center; CPU and memory stay hideable per device.
  - MC performance-sidebar-hidden-graphs: deprecated in Mission Center.
  - MC performance-page-data-points: becomes graphs.time_range_s.

  TaskSlinger's ProcessColumnVisibilityMask is not trusted as a visibility mask (cross-check B.2). tasklens stores explicit per-column visibility.
- Linux data/mechanism: APP-033
- Verdict: ADAPT
- Phase: P1 (store and the keys of P1 features), each other key with its feature
- Privilege: none

### APP-033 Settings persistence
- Source: TS UserSettings.ini | MC GSettings | review "Settings persistence"
- Behaviour:
  - Files:
    - `$XDG_CONFIG_HOME/tasklens/settings.toml`: user choices.
    - `$XDG_STATE_HOME/tasklens/state.toml`: window size, sort, columns, expand state, last tab, minigraph layout.
    - `$XDG_CONFIG_HOME/tasklens/themes.json`: custom themes in TaskSlinger's JSON format.
  - Each file has `version = 1` at the top. Writes are debounced 500 ms and atomic (temp file, fsync, rename).
  - Columns are stored by string id, never by display index.
  - Unknown keys are kept. Out-of-range values clamp to the range and are logged.
  - A file that fails to parse is renamed `settings.toml.bad-<unix time>`, defaults load, and the toast "Settings file was damaged; defaults loaded" appears once.
  - Migration: one function per version step.
  - An external edit to settings.toml is picked up through inotify.
- Linux data/mechanism: `toml` + `serde`; `directories` for XDG paths
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-034 Import TaskSlinger settings and themes
- Source: TS UserSettings.ini, themes.json (owner runs TaskSlinger under Wine)
- Behaviour: Settings > All settings > Advanced > "Import from TaskSlinger…" opens a portal file chooser for `UserSettings.ini` or `themes.json`. It maps the keys per APP-032 and imports custom themes. Toast "Imported N settings and M themes".
- Linux data/mechanism: INI parse, then the mapping table in APP-032
- Verdict: ADAPT
- Phase: P4
- Privilege: none

## Themes and appearance

### APP-035 Built-in themes and colour slots
- Source: TS 6 (12 themes, 22 slots), PLAN 9 palette
- Behaviour:
  - The 12 built-ins in TaskSlinger order: Default Dark, Vivid Nightfall, Catppuccin Frappé, Catppuccin Macchiato, Catppuccin Mocha, Dracula, OLED Black, Default Light, Windows 7 Aero, Golden Summer Fields, Soft Pink Delight, Catppuccin Latte.
  - The 22 slots:

    | Slot | Label |
    |---|---|
    | WINDOW_BG | Window background |
    | TITLEBAR | Title bar |
    | TOOLBAR | Toolbar |
    | INPUT | Input |
    | INPUT_FOCUS | Input focus |
    | HEADER | Header |
    | LIST_BG | List background |
    | ROW_ALT | Alternate row |
    | ROW_HOVER | Row hover |
    | ROW_SELECTED | Selected row |
    | DIVIDER | Divider |
    | MENU_BG | Menu background |
    | MENU_HOVER | Menu hover |
    | TAB_BG | Tab background |
    | TAB_HOVER | Tab hover |
    | SCROLL_TRACK | Scroll track |
    | SCROLL_THUMB | Scroll thumb |
    | TEXT_PRIMARY | Primary text |
    | TEXT_SECONDARY | Secondary text |
    | TEXT_SELECTED_PRIMARY | Selected primary text |
    | TEXT_SELECTED_SECONDARY | Selected secondary text |
    | VALUE_HEAT | Value heat |

  - Each theme also carries an accent and the six graph colours (CPU fill/line, kernel line, memory, GPU fill/line, disk, network). These sit outside the 22 editable slots.
  - Every slot maps onto gpui-component ThemeColor tokens. For example: LIST_BG to `table`; ROW_ALT to `table_even`; ROW_SELECTED to `table_active`; TITLEBAR to `title_bar`; DIVIDER to `border` and `table_row_border`.
  - Theme changes animate as a 200 ms colour crossfade, instant with reduced motion.
- Linux data/mechanism:
  - Default Dark values are in PLAN 9.
  - The other 11 come from the running TaskSlinger 0.9.2 under Wine. For each built-in, run "Create copy", then read the resulting `themes.json`, which gives exact `#RRGGBB` per slot.
  - Accent and graph colours come from pixel samples of each theme's screenshot (selected tab line, graphs).
  - The resulting palettes ship as `assets/themes/builtin.json`.
- Verdict: BUILD
- Phase: P1 (Default Dark, Default Light), P2 (other 10)
- Privilege: none

### APP-036 Theme menu and follow system light/dark
- Source: TS 1.3 Theme submenu L28480-28482, L28542, L28885-28888 | MC theme selector (system, light, dark) | cross-check A.40
- Behaviour:
  - View > Theme > lists the 12 built-ins and the custom themes. Each entry is painted in its own palette, the active one has a check, and each has a copy icon.
  - Per-theme items: "Create copy", "Edit colors", "Share theme", "Delete theme".
  - Then "Import theme", "Sync theme with system", "Preferred Light Theme >", "Preferred Dark Theme >".
  - With sync on, the active theme follows the portal colour scheme live:
    - 1 (prefer dark) uses `theme.dark`;
    - 2 (prefer light) uses `theme.light`;
    - 0 (no preference) uses `theme.dark`.
  - Toasts: "Theme copied", "Theme deleted", "Built-in themes cannot be deleted", "Copy a theme before editing it". Delete asks first: "Delete this custom theme?".
- Linux data/mechanism: `org.freedesktop.portal.Settings.ReadOne("org.freedesktop.appearance","color-scheme")`, plus the `SettingChanged` signal, through zbus on the session bus (xdg-desktop-portal-kde answers).
- Verdict: ADAPT (Windows AppsUseLightTheme becomes the XDG portal)
- Phase: P2
- Privilege: none

### APP-037 Theme editor
- Source: TS 6 "Theme Editor" L28900-28903, L30008-30025
- Behaviour:
  - A separate window titled "Theme Editor".
  - Left: the 22-slot list, plus Accent.
  - Centre: hue strip and colour plane, "R %d G %d B %d" and "H %03d S %03d V %03d" readouts, and a hex input with the hint "Enter a 6-digit hex color".
  - A title input for the theme name.
  - A live preview pane: mini table, tab bar and graph.
  - Buttons: Done, Random (toast "Random theme generated."), Share, Import, Cancel.
  - Edits apply live to the app; Cancel reverts. Built-in themes cannot be edited.
- Linux data/mechanism: custom GPUI elements for the colour plane and hue strip; saved to themes.json
- Verdict: BUILD
- Phase: P3
- Privilege: none

### APP-038 Theme share and import codes
- Source: TS 6 L29405, L29532-29533, L29667-29675, toasts L28620-28636
- Behaviour:
  - "Share theme" opens the "Theme Share Code" window with the code and a Copy button. Toast "Theme code copied".
  - "Import theme" reads the clipboard and accepts:
    - tasklens codes `TL1.` + base64url(JSON `{"v":1,"name":…,"accent":"#…","SLOT":"#RRGGBB",…}`);
    - TaskSlinger codes `TS1.…`;
    - raw TaskSlinger themes.json text.
  - Toasts: "Theme imported", "Clipboard does not contain a theme code".
- Linux data/mechanism: clipboard read through GPUI; serde_json
- Verdict: ADAPT
- Phase: P3
- Privilege: none
- Open question: the exact TaskSlinger `TS1.` encoding is inferred. Recommended answer: generate one code in TaskSlinger under Wine, decode it, and match it. If it cannot be decoded, accept tasklens codes and raw JSON only.

### APP-039 Graph accent colours
- Source: TS 6 L28548-28565
- Behaviour: The Graph Color menu (mini-graphs and tray graphs) offers Accent (theme default), then 17 named colours:

  | Name | Hex | Name | Hex |
  |---|---|---|---|
  | Coral | #FF7F50 | Blue | #3B82F6 |
  | Orange | #F28C28 | Indigo | #6366F1 |
  | Amber | #F5B700 | Violet | #8B5CF6 |
  | Yellow | #E8D44D | Lavender | #B4A7F5 |
  | Lime | #9BCB3C | Fuchsia | #D946EF |
  | Green | #4CAF50 | Pink | #EC4899 |
  | Mint | #3EB489 | Rose | #F43F5E |
  | Teal | #14A098 | Slate | #64748B |
  | Cyan | #22B8CF | | |

  These hex values are fallbacks. They are replaced by pixel samples of TaskSlinger's swatches when the theme extraction in APP-035 runs.
- Linux data/mechanism: constants in tl-ui
- Verdict: BUILD
- Phase: P3
- Privilege: none

### APP-040 Appearance sliders
- Source: TS 1.3 Appearance L28500-28505, toast "UI scale changed to %d%%" L28643, slider formats L35541-35543
- Behaviour: View > Appearance > holds, with labels "%d%%":
  - "UI Scale" slider: 75-200%, step 5.
  - "Row Height" slider: 80-150%.
  - "Rounded Corners" check.
  - "Value Color Coding" check.
  - "Value Color Coding Intensity" slider: 0-100%.
  - "Drop Shadow Intensity" slider: 0-100%.

  Effects:
  - UI scale multiplies the theme font size and rem. The compositor scale (110% here) is applied once by GPUI and never doubled.
  - Row height base is 28 logical px × row_height_pct.
  - Rounded corners set radius 4 px (rows, menus) and 6 px (cards); off sets 0.
  - Heat colouring blends VALUE_HEAT into the cell background by value / column max × intensity.
  - Shadow intensity scales the alpha of menu, mini-graph and toast shadows.
  - Dragging previews live. The save happens on Release.
- Linux data/mechanism: gpui-component Slider (`SliderEvent::Release` saves); `Theme::update` font_size and radius; row-height and text caches invalidated when `rem_size` changes
- Verdict: BUILD
- Phase: P2
- Privilege: none

### APP-041 System Information (Mission Center dialog)
- Source: MC about_system_dialog (OS, Kernel, Desktop groups; per-row copy; Copy All; Ctrl+A)
- Behaviour: Not a separate dialog. Its fields join the Information tab's "Operating System" section:
  - Package manager, Package manager version, Kernel release, Kernel version;
  - Desktop name, Desktop version, Windowing system, Virtual terminal;
  - the distro logo beside the section title.
  Missing values hide their rows, as in Mission Center. Help > "System Information…" switches to the Information tab and scrolls to that section. Mission Center's per-row copy is the Information row copy; its Copy All is the Information tab copy icon. Ctrl+A goes to Select all (APP-031).
- Linux data/mechanism:
  - `/etc/os-release` (PRETTY_NAME, VERSION_ID, LOGO);
  - `/proc/sys/kernel/{osrelease,version}`;
  - `pacman --version` / `apt --version` / `dnf --version` / `zypper --version` (first found);
  - `XDG_CURRENT_DESKTOP` and `XDG_SESSION_TYPE`;
  - desktop version from `plasmashell --version` (KDE) or `gnome-shell --version`;
  - `XDG_VTNR`.
- Verdict: ADAPT (merged into the Information tab)
- Phase: P2
- Privilege: none

### APP-042 Additional setup dialog (Mission Center first run)
- Source: MC first_run_dialog (Enabling Additional Values, setup script, revert, view, copy command)
- Behaviour:
  - Shown once after the first readings when `state.first_run_done` is false. Reopened from Help > "Additional Setup…".
  - Title "Enable additional values". Text: "CPU power draw needs read access to RAPL energy counters. Logs of system services need journal access. Privileged actions need the tasklens helper."
  - Each item has a status (OK / Not set up) and a row:
    - (1) "Allow CPU power reading": installs udev rule `/etc/udev/rules.d/99-tasklens-powercap.rules` with `SUBSYSTEM=="powercap", ACTION=="add", RUN+="/bin/chmod 0444 /sys%p/energy_uj"`, then runs `udevadm trigger -s powercap`. It warns that this exposes RAPL timing to all users (CVE-2020-8694).
    - (2) Journal access: shows `sudo usermod -aG systemd-journal $USER` with a Copy button. No automatic group change.
    - (3) Helper: "Installed" or "Not installed".
  - Buttons: "Run setup" (split button menu: "Revert setup", "View script", "Copy command", "Copy revert command"), "Close".
  - Result windows: success, or failure with "Copy output" / "Retry" / "Close".
  - Mission Center's `nethogs` capabilities and `sensors-detect` are not needed: tasklens uses sock_diag, and k10temp loads on its own.
- Linux data/mechanism: script at `/usr/lib/tasklens/tasklens-setup` (shipped by the package, not written to /tmp), run with `pkexec /usr/lib/tasklens/tasklens-setup [--revert]`
- Verdict: ADAPT
- Phase: P3
- Privilege: pkexec auth_admin. Without it, CPU power shows "unsupported (needs setup)", never 0.

### APP-043 Language and live locale change
- Source: TS 1.4 Language (10 locales), INI UI.Language, live locale change (cross-check A.40) | MC gettext (48 po files)
- Behaviour:
  - Every user-visible string goes through a message key from P1. The English catalogue ships in P1.
  - "Language: >" lists "System default" (follows `LC_ALL`/`LC_MESSAGES`/`LANG`) and the ten TaskSlinger locales, plus nb-NO.
  - Changing language reapplies all text live, without a restart.
  - Number and date formats follow the selected locale.
  - Missing keys fall back to English.
- Linux data/mechanism: Fluent (`fluent-bundle` + `unic-langid`), `.ftl` files embedded with `include_str!`; locale from the environment, with live change from `org.freedesktop.portal.Settings` key `org.freedesktop.locale`? Not provided by the KDE portal, so live change applies only to the in-app setting. Translations are drafted by Claude agents and reviewed per locale. No Mission Center po text is copied, because of GPL-3.
- Verdict: ADAPT
- Phase: P1 (key system and English), P4 (other locales)
- Privilege: none

### APP-044 Toasts
- Source: TS 8 (top centre) | gpui-component notifications (5 s autohide, placement per toast)
- Behaviour:
  - Toasts appear at the top centre of the window, 8 px under the tab bar, at most 4 stacked, auto-hidden after 5 s. Hover pauses the timer.
  - Types: Success (green icon), Info, Warning, Error. Errors stay 8 s.
  - An in-progress toast is replaced in place by its result through the same id (service jobs, close connection).
  - All toast texts come from the owning area's spec (TaskSlinger strings). General ones: "Copied to clipboard", "Copied: <value>", "Failed to copy to clipboard", "No process selected", "Select a process row first." (and the service, startup and connection variants), "Opening file location", "Failed to open file location", "Could not open properties.", "Authentication cancelled".
  - Shown only while the main window is visible. While hidden to tray, Error results go to the desktop notification instead (`NotificationDelivery::System`).
- Linux data/mechanism: `window.push_notification(Notification::success(..).placement(Anchor::TopCenter).id1::<T>(key))`; system delivery through org.freedesktop.Notifications
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-045 Dialogs as separate pinnable windows
- Source: TS 8 [CL 0.8.25], cross-check A.38 (`TaskSlingerDetachedModal`, `modal.pin`)
- Behaviour:
  - Every dialog and property panel is its own top-level window with a title bar pin button. This covers Run, Handle Search, delete confirmations, Public IP notice, Theme Share Code, Theme Editor, About, Additional setup, Shortcuts, Settings, and the Process, Service, Startup and Connection Properties.
  - Confirmations are modal to their parent (Esc and Cancel close; Enter triggers the default button). Property, editor and tool windows are not modal.
  - Window titles: "tasklens Run", "tasklens Handle Search", "tasklens Public IP Notice", "tasklens Delete Startup Entry", "tasklens Share Theme", "tasklens About".
  - Closing the main window closes all child windows, except detached graphs when `graphs.keep_detached_on_close` is on.
- Linux data/mechanism:
  - `cx.open_window` with `WindowKind::Dialog` for modal confirmations (xdg_dialog_v1 set_modal) and `WindowKind::Floating` (xdg parent) for tools.
  - Each window gets a gpui-kit Root, so it has its own toasts and menus. The pin uses APP-009.
  - In-window gpui-component Dialog is used only for sub-prompts inside those windows.
- Verdict: BUILD
- Phase: P1 (frame), others with their features
- Privilege: none

### APP-046 Rendering GPU choice
- Source: TS GPU preference L28527-28529, L28540-28541, Graphics.*, dual-path startup (cross-check A.43)
- Behaviour: Settings > Monitoring > "Rendering GPU >" lists "Auto", one entry per Vulkan adapter (name), and "Software renderer". Changing it shows the toast "Restart tasklens to use the new GPU".
- Linux data/mechanism:
  - At startup, before GPUI initialises, set `ZED_DEVICE_ID=0x<pci device id>` for an adapter.
  - For software rendering, set `WGPU_ADAPTER_NAME=llvmpipe` together with `VK_ICD_FILENAMES` set to lvp_icd.
  - Adapters are listed with wgpu `Instance::enumerate_adapters` in a short-lived probe at menu open.
  - TaskSlinger's WARP first frame has no wgpu equivalent and is not copied.
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### APP-047 Tray icon and close/minimize to tray
- Source: TS INI Window.CloseToTray/MinimizeToTray/AlwaysShowTrayIcon, tray menu L28588, L29307, tooltip L28584-28586, L30144-30157 | review (closing the last window exits in v1; tray is a later explicit mode)
- Behaviour:
  - When any tray setting is on, or a tray graph exists, a StatusNotifierItem appears.
  - Tooltip: "CPU: %d% RAM: %d% GPU: %d% Disk: %d% Network: <rate>", updated at 1 Hz while the tray is shown.
  - Left click toggles the main window.
  - Menu: "Show Main Window", "Tray icon graphs >" (graph spec), "Close tray icon graph", separator, "Exit tasklens".
  - Close to tray: close (button, Ctrl+W, Esc-close) hides the window instead of exiting.
  - Minimize to tray: minimize hides it.
  - Always show tray icon: keeps the icon while the window is visible.
  - The main tray icon is hidden while a tray graph icon is shown [CL 0.9.2].
  - With every tray setting off, closing the main window exits.
  - While hidden: collection stops except tray and graph demand (review hidden policy).
- Linux data/mechanism: SNI implemented directly over zbus in the tl-engine D-Bus adapter. No ksni, so no tokio. Objects:
  - `org.kde.StatusNotifierItem` at `/StatusNotifierItem`, with `IconPixmap` an ARGB32 22×22 and 44×44;
  - `com.canonical.dbusmenu` at `/MenuBar`;
  - registered through `org.kde.StatusNotifierWatcher.RegisterStatusNotifierItem`.
  The watcher's NameOwnerChanged triggers re-registration, the Linux form of TaskbarCreated.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### APP-048 Start at login
- Source: TS Startup.StartWithWindows/StartMinimized (`--start-minimized`)
- Behaviour: "Start at login" writes the autostart entry; turning it off deletes it. "Start minimized" adds `--start-minimized`. That flag starts hidden to tray if the tray is enabled, otherwise minimized. The entry then shows in the Startup Apps tab like any other.
- Linux data/mechanism: `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop` with `Exec=tasklens [--start-minimized]`, `X-KDE-autostart-phase=2`, written atomically
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-049 Privilege settings (always elevated, passwordless)
- Source: TS AlwaysLaunchElevated, UacLessElevation (IFEO, experimental) L30388-30400
- Behaviour:
  - "Unlock privileged actions at launch" runs APP-003 at startup (one prompt).
  - "Skip password prompts (experimental)" installs a polkit rule so tasklens helper actions need no password for the active local session of wheel members. Turning it off removes the rule.
  - Confirm window first: "tasklens actions that change other users' processes, system services and network connections will run without asking for a password. Continue?", buttons "Skip prompts" / "Cancel".
- Linux data/mechanism: `pkexec /usr/lib/tasklens/tasklens-setup --polkit-rule [on|off]` writes or removes `/etc/polkit-1/rules.d/49-tasklens.rules`, which returns `polkit.Result.YES` for `action.id.indexOf("io.github.oysteinkrog.tasklens.") == 0 && subject.local && subject.active && subject.isInGroup("wheel")`.
- Verdict: ADAPT
- Phase: P3 (unlock at launch), P4 (passwordless)
- Privilege: pkexec once to install

### APP-050 Updater, installer and related items
- Source: TS 8 Updater, TaskSlinger Setup, cross-check A.33-A.36, A.44, C.5 internal flags; INI Updates, Install
- Behaviour: none. Each item below is DROP:
  - In-app updater (check, download, signature check, "Click to update", changelog), the CheckForUpdatesOnStartup setting, and the About "Updates" block: pacman and the localbuilds recipe update tasklens.
  - Installer (install for current user, location, PATH, Start Menu, desktop shortcut, Modify, Use standalone, Uninstall, folder picker, `.ts-migrate`, installed-notification) and the flags `--install-source` and `--internal-install`, `--internal-uninstall-helper`, `--internal-replace-in-place`, `--internal-install-handoff-pid`, `--internal-uninstall-wait-pid`, `--internal-show-installed-notification`: the pacman package installs the binary, `.desktop` file, icon and polkit policy.
  - AppUserModelID and taskbar pin repair: Wayland uses `app_id` and the `.desktop` file instead (APP-001).
  - Session 0 check: no Linux equivalent.
  - "Set as default Task Manager" (IFEO) and `--internal-set-taskmgr-default` / `--internal-clear-taskmgr-default`: replaced by APP-051.
  - `--internal-elevation-handoff`: tasklens never relaunches elevated (APP-003).
  - `--internal-skip-single-instance`: replaced by `--app-id` (APP-052).
- Linux data/mechanism: none
- Verdict: DROP (reasons per item above)
- Phase: P4 (recorded)
- Privilege: none

### APP-051 Open tasklens with Ctrl+Esc (KDE)
- Source: TS "Set as default Task Manager" (IFEO), Installer option
- Behaviour: Settings > Startup and window > "Open with Ctrl+Esc". When on, KDE's Ctrl+Esc opens tasklens instead of Plasma System Monitor. When off, the KDE default comes back.
- Linux data/mechanism: kglobalaccel D-Bus `org.kde.kglobalaccel /kglobalaccel`. Register `io.github.oysteinkrog.Tasklens.desktop` `_launch` = Ctrl+Esc, and set `org.kde.plasma-systemmonitor.desktop` `_launch` to none, using `setForeignShortcutKeys`.
- Verdict: ADAPT
- Phase: P4
- Privilege: none
- Open question: whether `setForeignShortcutKeys` sticks for a `.desktop` launch component that is not running. Recommended answer: test on Plasma 6. If it does not stick, write `~/.config/kglobalshortcutsrc` `[services][io.github.oysteinkrog.Tasklens.desktop] _launch=Ctrl+Esc` and ask the user to log out and back in.

### APP-052 Command-line arguments
- Source: TS 7 and cross-check C.5 | MC `--app-id`/`-a` | PLAN `--bench`
- Behaviour: Arguments:
  - `--start-minimized`
  - `--low-memory`: low-memory rescue window (low-memory spec); TaskSlinger's `/low-memory` and `--internal-low-memory-recovery`
  - `--tab <processes|performance|services|startup|connections|information>`: TaskSlinger's page argument, whose syntax is unknown, made explicit
  - `--select-pid <pid>`: opens Processes and selects that pid
  - `--debug`: Debug menu
  - `--bench [charts|table|history]`: replaces TaskSlinger's `--history-scrub-bench`
  - `--bench-out <file>`: replaces `--history-scrub-bench-out`
  - `--force-full-memory-mode`: TaskSlinger's `--internal-force-full-memory-mode`, used to test the low-memory path
  - `--app-id <reverse-dns id>`: separate instance and settings dir, for development builds (Mission Center)
  - `--version`, `--help`
  Unknown arguments print usage and exit 2. Forwarding to a running instance follows APP-011.
- Linux data/mechanism: `clap` derive
- Verdict: ADAPT
- Phase: P1 (`--tab`, `--select-pid`, `--version`, `--help`, `--bench`), P2 (`--start-minimized`), P3 (`--low-memory`, `--force-full-memory-mode`), P4 (`--app-id`, `--debug`)
- Privilege: none

### APP-053 About dialog
- Source: TS About L29358-29368, L35581-35590, cross-check A.33 | MC adw AboutDialog
- Behaviour:
  - A separate window titled "About tasklens".
  - Shows icon, "tasklens", "Version: <semver> (<git short hash>, <build date>)".
  - "Copy version info" copies version, GPUI revision, kernel, NVIDIA driver, compositor, session type and scale for bug reports.
  - "Third-party licenses" opens a scrollable list generated at build time.
  - Links: project repository, and "Open log folder".
  - No TaskSlinger or Mission Center branding.
  - TaskSlinger's author line, Discord, Twitter, bug email, support text and update block are not copied: they belong to TaskSlinger's product.
- Linux data/mechanism: `cargo-about` output embedded with `include_str!`; `env!("CARGO_PKG_VERSION")` plus a build.rs git hash
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-054 Logging
- Source: TS log levels and format (cross-check A.42) L29425-29437, logs\app.log | review risk 7
- Behaviour:
  - Log file: `$XDG_STATE_HOME/tasklens/logs/tasklens.log`, rotated at 5 MB, keeping 5 files.
  - Levels: ERROR, WARN, PERF, INFO, DEBUG. Default INFO. Set with `TASKLENS_LOG=<level>[,target=level]`; `RUST_LOG` is honoured too.
  - Categories: STARTUP, RENDER, TEXT, CRASH, ENGINE, DBUS, ACTION.
  - Line format: `2026-10-02 14:03:11.512 [INFO] [STARTUP] [tid=1234] [uptime_ms=87] message`.
  - Identical consecutive lines collapse into "previous message repeated N times: …".
  - Command lines, environments and window titles are never logged by default.
  - PERF lines record first-frame time and collector timings.
- Linux data/mechanism: `tracing` + a custom formatter and rotating file writer; also stderr when attached to a TTY
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-055 Crash handling and fatal errors
- Source: TS 8 fatal errors L28904-28920, crash.log, dumps\, Crash.DumpType | review (NVML isolation, device loss)
- Behaviour:
  - A Rust panic in any thread writes `$XDG_STATE_HOME/tasklens/crash/crash-<timestamp>.log`. It contains message, location, backtrace, version info and the last 200 log lines, with no command lines or environments.
  - Then a desktop notification: "tasklens stopped after an unrecoverable error."
  - The next launch shows a one-time window: "tasklens closed unexpectedly last time", buttons "Open log folder", "Copy report", "Close".
  - Native crashes (SIGSEGV) are left to systemd-coredump. The report window mentions `coredumpctl info tasklens`. TaskSlinger's Mini/Full dump setting is not needed.
  - Engine collector panics restart per the review's backoff and do not crash the app.
  - GPU init failure ("tasklens could not start the renderer") goes to stderr and a desktop notification, then exit 1.
  - GPU device loss: if GPUI cannot recover, show the notification "tasklens lost the graphics device and must restart" and relaunch itself once.
- Linux data/mechanism: `std::panic::set_hook`; `org.freedesktop.Notifications.Notify` through zbus (works without a GPUI window)
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-056 Public IP fetch notice
- Source: TS 8 "Confirm Public IP Fetch" L29346-29350, Connections.AutoFetchPublicIP
- Behaviour:
  - The first Fetch (Network page or Information tab) shows "tasklens Public IP Notice" with text:
    - "tasklens will fetch your public IP address from an online service."
    - "Service: https://api.ipify.org"
    - "Continue only if you want this computer to contact that service."
  - A checkbox "Do not ask again for public IP fetches", and buttons Continue / Cancel.
  - "Auto-fetch Public IP on launch" fetches once at startup when the notice has been accepted.
- Linux data/mechanism: HTTPS GET with `ureq` (rustls, blocking, no tokio) on the blocking pool, 5 s timeout
- Verdict: BUILD
- Phase: P2
- Privilege: none

### APP-057 NVIDIA G-SYNC profile
- Source: TS Settings "Create NVIDIA G-SYNC profile" L28530-28535 [CL 0.8.28]
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (KWin applies adaptive sync only to fullscreen windows by default, so a windowed tasklens never triggers VRR flicker; no NVIDIA driver profile is needed)
- Phase: P4 (recorded)
- Privilege: none

### APP-058 Accessibility and reduced motion
- Source: TS TextScaleFactor [CL 0.8.29] | review "Accessibility/input"
- Behaviour:
  - Full keyboard use per APP-031, with visible focus rings.
  - Icon-only buttons have accessible names.
  - Charts expose their latest value and unit as text.
  - Reduced motion: system reduce-motion turns off tab, theme and toast animations and graph sliding, whatever the settings say.
  - The UI scale default follows the system font: when `appearance.ui_scale_pct` has never been set, the default is the KDE general font point size / 10 × 100, rounded to step 5.
- Linux data/mechanism: GPUI accesskit adapter (present in gpui-linux); `App::reduce_motion()`; `kdeglobals [General] font`
- Verdict: ADAPT
- Phase: P2 (scale-from-font default P4)
- Privilege: none

### APP-059 Loading placeholders
- Source: MC shimmer placeholders | TS "Loading service data..." | widgets rule 17
- Behaviour: Skeleton rows or blocks appear only for a tab's first load before its first data arrives. They are never shown when switching between tabs whose data is already loaded. The first frame is drawn without waiting for collectors.
- Linux data/mechanism: gpui-component Skeleton, which stops requesting frames once replaced
- Verdict: BUILD
- Phase: P1
- Privilege: none

# Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| SYS-001 | Services tab, two systemd managers | ADAPT | P2 |
| SYS-002 | Unit types shown | ADAPT | P2 |
| SYS-003 | Services columns | ADAPT | P2 |
| SYS-004 | State filters and filter strip | BUILD | P2 |
| SYS-005 | Live updates and job tracking | ADAPT | P2 |
| SYS-006 | Start, Stop, Restart, Reload, Reset failed | BUILD | P2 |
| SYS-007 | Startup type changes | ADAPT | P2 |
| SYS-008 | Services context menu | ADAPT | P2 |
| SYS-009 | Copy formats (services) | BUILD | P2 |
| SYS-010 | Jump between services and processes | BUILD | P2 |
| SYS-011 | Service Properties: frame and General | ADAPT | P2 |
| SYS-012 | Service Properties: Log On | ADAPT | P2 |
| SYS-013 | Service Properties: Recovery | ADAPT | P2 view, P3 edit |
| SYS-014 | Service Properties: Dependencies | ADAPT | P2 |
| SYS-015 | Service logs (journal) | ADAPT | P2 |
| SYS-016 | Services status bar and loading | BUILD | P2 |
| SYS-017 | Services sort and column persistence | BUILD | P2 |
| SYS-018 | OpenRC backend | BUILD | P4 |
| SYS-020 | Startup sources and precedence | ADAPT | P2 |
| SYS-021 | Startup columns and values | ADAPT | P2 |
| SYS-022 | Startup impact | ADAPT | P2 |
| SYS-023 | Startup enable and disable | ADAPT | P2 |
| SYS-024 | Run now | ADAPT | P2 |
| SYS-025 | Delete entry | ADAPT | P2 |
| SYS-026 | Startup context menu, copy, open | BUILD | P2 |
| SYS-027 | Startup Properties window | ADAPT | P2 |
| SYS-028 | Startup status bar, sort, persistence | BUILD | P2 |
| SYS-030 | Connection inventory | ADAPT | P2 |
| SYS-031 | Connection columns, formats, states | ADAPT | P2 |
| SYS-032 | Owning process attribution | ADAPT | P2 / P3 |
| SYS-033 | Close TCP connection | ADAPT | P3 |
| SYS-034 | Connections context menu and copy | BUILD | P2 |
| SYS-035 | Jump to owning process | BUILD | P2 |
| SYS-036 | Connection Properties window | ADAPT | P2 |
| SYS-037 | Remote host name column | ADAPT | P4 |
| SYS-038 | Connections status bar, filter, sort | BUILD | P2 |
| SYS-039 | Other network namespaces | ADAPT | P4 |
| APP-001 | Window decorations (CSD on KDE) | BUILD | P1 |
| APP-002 | Title bar layout and toolbar | ADAPT | P1 |
| APP-003 | Shield: unlock privileged actions | ADAPT | P3 |
| APP-004 | Hide system processes button | ADAPT | P1 |
| APP-005 | View mode button | BUILD | P1 |
| APP-006 | Window picker "+" | ADAPT | P3 |
| APP-007 | Update badge | DROP | P4 |
| APP-008 | Caption buttons | BUILD | P1 (pin P2) |
| APP-009 | Always on top | ADAPT | P2 |
| APP-010 | Size, minimum size, geometry restore | ADAPT | P1 |
| APP-011 | Single instance | ADAPT | P1 |
| APP-012 | File menu | ADAPT | P1 |
| APP-013 | Run Installer item | DROP | P4 |
| APP-014 | Run dialog | ADAPT | P2 |
| APP-015 | Search File Handles dialog | ADAPT | P3 |
| APP-016 | Launch other system monitor | ADAPT | P4 |
| APP-017 | View menu structure | BUILD | P1-P3 |
| APP-018 | View > Columns (all tables) | BUILD | P1 |
| APP-019 | View > Process View | BUILD | P1 |
| APP-020 | View > Window | ADAPT | P2 |
| APP-021 | View > Graphs | BUILD | P3 |
| APP-022 | Settings menu | ADAPT | P1 |
| APP-023 | Preferences window | ADAPT | P2 |
| APP-024 | Help menu | ADAPT | P2 |
| APP-025 | License items and early-bird notice | DROP | P4 |
| APP-026 | Debug menu | ADAPT | P4 |
| APP-027 | Tab bar and tab animation | BUILD | P1 |
| APP-028 | Filter box | BUILD | P1 |
| APP-029 | Status bar | ADAPT | P1 |
| APP-030 | Keyboard Shortcuts window | BUILD | P2 |
| APP-031 | Keyboard shortcuts (merged) | ADAPT | P1 |
| APP-032 | Settings registry | ADAPT | P1 |
| APP-033 | Settings persistence | ADAPT | P1 |
| APP-034 | Import TaskSlinger settings and themes | ADAPT | P4 |
| APP-035 | Built-in themes and colour slots | BUILD | P1 / P2 |
| APP-036 | Theme menu and follow system | ADAPT | P2 |
| APP-037 | Theme editor | BUILD | P3 |
| APP-038 | Theme share and import codes | ADAPT | P3 |
| APP-039 | Graph accent colours | BUILD | P3 |
| APP-040 | Appearance sliders | BUILD | P2 |
| APP-041 | System Information (MC dialog) | ADAPT | P2 |
| APP-042 | Additional setup dialog | ADAPT | P3 |
| APP-043 | Language and live locale change | ADAPT | P1 / P4 |
| APP-044 | Toasts | BUILD | P1 |
| APP-045 | Dialogs as separate pinnable windows | BUILD | P1 |
| APP-046 | Rendering GPU choice | ADAPT | P4 |
| APP-047 | Tray icon, close/minimize to tray | ADAPT | P3 |
| APP-048 | Start at login | ADAPT | P2 |
| APP-049 | Privilege settings | ADAPT | P3 / P4 |
| APP-050 | Updater, installer and related items | DROP | P4 |
| APP-051 | Open with Ctrl+Esc (KDE) | ADAPT | P4 |
| APP-052 | Command-line arguments | ADAPT | P1-P4 |
| APP-053 | About dialog | ADAPT | P2 |
| APP-054 | Logging | ADAPT | P1 |
| APP-055 | Crash handling and fatal errors | ADAPT | P1 |
| APP-056 | Public IP fetch notice | BUILD | P2 |
| APP-057 | NVIDIA G-SYNC profile | DROP | P4 |
| APP-058 | Accessibility and reduced motion | ADAPT | P2 |
| APP-059 | Loading placeholders | BUILD | P1 |

# Conflicts with the architecture review

1. **History default.** TaskSlinger records 6 hours by default and saves it to disk. The review says recording is off by default in v1. I followed the review: `history.enabled=false`, and retention stays 6 h when the user turns it on (APP-032).
2. **Graph time ranges.** The review wants 60 s as the default and only 15 s and 60 s choices in v1, with 602-sample rings sized for 10 Hz. TaskSlinger offers 30 s to 10 min and Mission Center up to 600 points. I chose 30 s, 60 s, 2 min, 5 min and 10 min (default 60 s). Ranges over 60 s need a second ring at 1 Hz, decimated with the review's min/max rule. The Performance spec has to confirm this.
3. **Curved lines.** The review bans splines that overshoot. Mission Center has Bézier smoothing. I kept it as an opt-in monotone cubic curve that cannot overshoot, default off (`graphs.curved_lines`).
4. **TCP connection close.** The review defers it. It stays in the plan, phased P3 through tl-helper with a polkit action and cookie-exact `SOCK_DESTROY` (SYS-033). No pkexec shell-outs.
5. **Always on top.** The review defers it where it is unsupported. I build it on KDE through a KWin script and on X11 through `_NET_WM_STATE_ABOVE`. Elsewhere it is disabled (APP-009).
6. **Window lifecycle.** This agrees with the review: closing the last window exits unless a tray setting is on, and tray is P3 (APP-047). The tray's SNI objects live in the engine's zbus adapter, so that adapter must keep running while the window is hidden.
7. **Helper scope.** Run as administrator uses plain pkexec (`org.freedesktop.policykit.exec`), not tl-helper, so the helper never offers arbitrary shell (review risk 8). The opt-in passwordless polkit rule (APP-049) covers only the `io.github.oysteinkrog.tasklens.*` actions.

These are not review conflicts, but the owner should know:
- **Row height and label case.** The gpui-component design rules ask for the Size API row heights (26, 30, 32 or 40 px) and sentence-case labels. TaskSlinger uses 28 px rows and Title Case menu labels ("End Process Tree"). I kept TaskSlinger's because the owner wants its look.
- **App ID.** I chose `io.github.oysteinkrog.Tasklens` as the app ID and D-Bus name. If the project name changes, this ID, the `.desktop` file name and the polkit action prefixes change with it.
