# tasklens merged spec: Services, Startup Apps, Connections and the app shell

This is the merged feature spec for two areas:
- SYS: the Services, Startup Apps and Connections tabs.
- APP: the application shell. It covers the title bar, toolbar, menus, shortcuts, settings, persistence, themes, dialogs, toasts, the main tray icon, the command line, logging, crash handling and single instance.

Source: `docs/spec/features-sys-app.md` (old IDs SYS-001 to SYS-039, APP-001 to APP-059).

Binding inputs, in order of precedence: `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-103 and canonical tables a to d), `docs/ARCHITECTURE.md` (ADR-001 to ADR-016) on engineering structure, then the source spec. Every resolution is already applied below. Where an entry cites R-0xx or D-0xx, that record set the value.

Conventions:
- Phases follow R-072. P0 is the spike. P1 is the Performance core, the process table and P1 actions. P2 adds Services, Startup Apps, Connections, Properties windows, the KWin bridge, the 5 and 10 minute ranges and the remaining themes. P3 adds the helper, history, tray, detached graphs, mini window, strings, dumps and low-memory. P4 is everything later.
- Names, paths, polkit actions and D-Bus names are from canonical table (c). Number formats are from canonical table (d). Settings keys are from canonical table (a). Keys are from canonical table (b).
- Polkit actions owned by tasklens are exactly three (R-002): `io.github.oysteinkrog.tasklens.helper`, `io.github.oysteinkrog.tasklens.setup` and `io.github.oysteinkrog.tasklens.netmon` (reserved). Every tl-helper operation runs under the helper action.
- Table cells, header totals and sidebar readouts use ADR-003's validity glyphs. Properties fields and Information rows use the PERF-013 words ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown") (R-050).
- "Was:" lines name old IDs from `features-sys-app.md`. Every other ID reference names a merged entry: SYS and APP in this file, PROC in `proc.md`, PERF in `perf.md`.

# SYS: Services, Startup Apps, Connections

## Services tab

### SYS-001 Services tab, two systemd managers
- Was: SYS-001
- Source: TS 2.4 | MC services_page/mod.rs:181-190, PL:services/systemd/mod.rs:325-511 | both
- Behaviour:
  - One table lists units from the system manager and from the user manager.
  - Two collapsible group rows, styled like TaskSlinger's process groups: "System services (N)" first, then "User services (N)".
  - Expand state is kept in `state.toml` `[services] group_expanded = { system = true, user = true }`.
  - Expanding a service row shows the processes in its cgroup as child rows, as a process subtree. Child rows reuse the process row renderer and the process context menu.
  - A unit is identified by `(manager, unit id)`, so `foo.service` can exist in both groups.
  - TaskSlinger has a flat list and Mission Center has two sections. The grouped layout wins because Linux has two managers.
- Linux data/mechanism:
  - zbus on the system bus and the session bus, `org.freedesktop.systemd1`, `/org/freedesktop/systemd1`, interface `org.freedesktop.systemd1.Manager`.
  - `ListUnitsByPatterns([], patterns)` for loaded units. `ListUnitFilesByPatterns([], patterns)` adds installed but unloaded units (disabled ones). Per unit, `org.freedesktop.DBus.Properties.GetAll` on `org.freedesktop.systemd1.Unit` and `.Service`.
  - Child processes come from `ControlGroup`, mapped to `/sys/fs/cgroup<ControlGroup>/cgroup.procs`, read recursively.
  - Engine workers (ADR-004): `SourceId::SystemdSystem` and `SourceId::SystemdUser`, both in `workers/dbus_systemd.rs`. Each opens its own zbus connection with `default-features=false`, `async-io` and `.internal_executor(false)`, driven by an asupersync task. Identity type `UnitRef { manager: System | User, name }` (ADR-016).
- Verdict: ADAPT (Windows SCM becomes two systemd managers; group rows added)
- Phase: P2
- Privilege: none to list. If the session bus is missing, the User group shows "User manager not available". If the system bus is missing, the System group shows "System manager not available". Mission Center hides the whole tab when both lists are empty. tasklens never hides it and shows the empty state "systemd not found" instead.

### SYS-002 Unit types shown
- Was: SYS-002
- Source: MC PL:services/systemd/mod.rs:437-474 | TS 2.4
- Behaviour:
  - A "Unit types" dropdown in the Services filter strip (SYS-004) has check items: Services, Sockets, Timers, Mounts, Automounts, Paths, Targets, Swaps, Slices, Scopes.
  - Default on: Services, Sockets, Timers, Mounts. Mission Center shows service, socket and mount. Timers are added because they are the Linux scheduled-task view.
  - Always hidden: `LoadState=not-found` units with no unit file, uninstantiated templates (`foo@.service`; their instances are listed), and `device` units.
- Linux data/mechanism: suffix filter on the unit id, done in the engine projection. Saved in `services.unit_types = ["service", "socket", "timer", "mount"]`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-003 Services columns
- Was: SYS-003
- Source: TS 2.4 (widths 250,300,100,120,80,200,260; Columns.Services=0..6) | MC table_view (resource columns)
- Behaviour:
  - Default visible columns, in TaskSlinger order, with column ids: Name (`name`), Display name (`display_name`), Status (`status`), Startup type (`startup_type`), PID (`pid`), User (`user`), Path (`path`).
  - Optional columns, hidden by default and chosen through View > Columns: Manager (`manager`, System or User), Unit type (`unit_type`), Sub state (`sub_state`), Load state (`load_state`), CPU (`cpu`), Memory (`memory`), Disk (`disk`, read plus write rate), Tasks (`tasks`), Active since (`active_since`), Unit file (`unit_file`), Triggered by (`triggered_by`), Description (`description`).

  | Column | Value |
  |---|---|
  | Name | unit id, for example `sshd.service` |
  | Display name | `Description` |
  | Status | see the status table below |
  | Startup type | `UnitFileState` mapped to: Enabled, Enabled (runtime), Disabled, Static, Indirect, Masked, Generated, Transient, Alias, Linked, or "-" when there is no unit file |
  | PID | `MainPID`, blank when 0 |
  | User | real uid owner of MainPID from `/proc/<pid>/status`; else `User=`; else "root" (system manager) or the current user (user manager) |
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

  - TaskSlinger's Resuming, Pausing and Paused have no systemd state and are not used. Startup type uses systemd words, because Automatic and Manual would lose Static, Indirect and Masked.
  - The resource columns (CPU, Memory, Disk, Tasks) are cgroup totals for the unit, sampled at 1 Hz only while one of them is visible. Units without a cgroup show "-", never 0.
  - Formats per table (d): CPU `%.1f%%` ("0%" at measured zero); Memory IEC with two decimals; Disk fixed MiB/s with one decimal; Tasks a count with thin-space grouping; Active since `YYYY-MM-DD HH:MM:SS` local.
- Linux data/mechanism: Unit properties `Id`, `Description`, `ActiveState`, `SubState`, `LoadState`, `UnitFileState`, `FragmentPath`, `ActiveEnterTimestamp`, `TriggeredBy`, `ControlGroup`. Service properties `MainPID`, `User`, `ExecStart`. Socket property `Listen`. Mount property `What`. Timer property `Unit`. Cgroup files `cpu.stat usage_usec` (delta), `memory.current`, `io.stat` rbytes+wbytes (delta), `pids.current`.
- Verdict: ADAPT
- Phase: P2 (resource columns P2)
- Privilege: none

### SYS-004 State filters and filter strip
- Was: SYS-004
- Source: MC services_page/page.blp:133-211, table_view/mod.rs:641 | TS has none
- Behaviour:
  - One 28 px strip at the top of the Services tab holds: toggles "Running", "Failed", "Stopped", "Disabled"; the Unit types dropdown (SYS-002); toggles "System" and "User", both on by default; a "Remove filters" button.
  - State classification follows Mission Center: running first, then failed, then stopped (enabled and not running), otherwise disabled.
  - Selected states combine with OR. The result combines with the tab's text filter using AND. No state toggle on means all states.
  - Child process rows inherit their service's state.
  - The state toggles are saved in `services.filter_states = []` (subset of `running failed stopped disabled`).
- Linux data/mechanism: engine projection over cached unit state
- Verdict: BUILD (Mission Center behaviour, TaskSlinger styling)
- Phase: P2
- Privilege: none

### SYS-005 Live updates and job tracking
- Was: SYS-005
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
  - Result `done` maps to success. `failed`, `timeout`, `canceled`, `dependency`, `skipped` and `invalid` each map to a failure with that word (`ActionError::UnitJob`).
  - Action states are Rejected, Started, Succeeded, Failed and OutcomeUnknown, delivered on the action result path (ADR-005), never in the snapshot.
  - There is no client-side timeout on the job, because systemd enforces `TimeoutStartSec`.
  - On reconnect, re-subscribe and reload the lists.
- Verdict: ADAPT
- Phase: P2
- Privilege: none for signals

### SYS-006 Start, Stop, Restart, Reload, Reset failed
- Was: SYS-006
- Source: TS 2.4 | MC actions.rs:87-138, PL service.rs:71-141
- Behaviour:
  - Context menu and keys (table b, ServiceTable context, service row): Start (Ctrl+S; enabled when not active), Stop (Ctrl+E; enabled when active), Restart (Ctrl+R; enabled when active). On a child process row Ctrl+E acts on the process.
  - Reload: shown only when `CanReload`.
  - Reset failed state: tasklens addition, shown only when Failed; clears the failed state.
  - No confirmation dialog for any of these (both apps agree).
  - Multi-select runs the action on each selected unit and gives one toast per result.
- Linux data/mechanism: `StartUnit(name,"replace")`, `StopUnit(name,"replace")`, `RestartUnit(name,"replace")`, `ReloadUnit(name,"replace")`, `ResetFailedUnit(name)`. All called with the `ALLOW_INTERACTIVE_AUTHORIZATION` message flag. Mission Center uses "fail" mode for start; tasklens uses "replace" to match `systemctl`.
- Verdict: BUILD
- Phase: P2
- Privilege: the user manager needs none. The system manager uses polkit `org.freedesktop.systemd1.manage-units` (auth_admin_keep), and polkit-kde-authentication-agent shows the prompt. A cancelled prompt gives the toast "Authentication cancelled". Denied gives "Failed to start service: not authorized".

### SYS-007 Startup type changes (enable, disable, mask)
- Was: SYS-007
- Source: TS 3.2 Startup combo, toasts L28651, L28663 | MC Enabled switch (details dialog)
- Behaviour:
  - Context submenu "Startup type >" with Enabled, Disabled, Masked. The current value is checked.
  - Static, Indirect, Generated and Transient units show the submenu disabled, with tooltip "This unit has no install section".
  - The same choice sits in the Properties General combo (SYS-011).
  - Toasts: "Startup type changed" / "Failed to change startup type".
- Linux data/mechanism:
  - Enabled: `EnableUnitFilesWithFlags([name], 0, …)`.
  - Disabled: `DisableUnitFilesWithFlags([name], 0)`.
  - Masked: `MaskUnitFiles([name], false, true)`.
  - Leaving Masked: `UnmaskUnitFiles` first.
  - After each, call `Manager.Reload()`, as systemctl does.
- Verdict: ADAPT
- Phase: P2
- Privilege: the system manager uses polkit `org.freedesktop.systemd1.manage-unit-files` plus `org.freedesktop.systemd1.reload-daemon`. This may prompt twice when the keep window does not cover both actions; that is accepted. The user manager needs none.

### SYS-008 Services context menu
- Was: SYS-008
- Source: TS 2.4 L28202-28203, L29283-29317 | MC table_view.blp:179
- Behaviour: Items in order:
  1. Start, Stop, Restart, Reload
  2. Startup type >
  3. separator
  4. View logs (opens Properties on the Logs tab)
  5. Properties (Enter, Alt+Enter, Ctrl+I)
  6. File properties
  7. Open file location
  8. Open unit file
  9. separator
  10. Copy (clicked cell), Copy row (Ctrl+C)
  11. Jump to process

  Child process rows show the full process menu instead.
- Linux data/mechanism:
  - File properties: `org.freedesktop.FileManager1.ShowItemProperties([file://binary], "")` on the session bus.
  - Open file location: `org.freedesktop.FileManager1.ShowItems([file://binary], "")`.
  - Open unit file: `xdg-open FragmentPath`, opened read-only by the user's editor.
  - Toasts: "Opening service file location." / "Could not open service file location.", "Opened service file properties." / "Could not open service file properties.", "Service image unavailable" when there is no ExecStart.
- Verdict: ADAPT (File properties maps to FileManager1; "Open unit file" added)
- Phase: P2
- Privilege: none

### SYS-009 Copy formats (services)
- Was: SYS-009
- Source: TS L35514-35520 (cross-check A.3), toasts "Service details copied." / "Could not copy service details."
- Behaviour (R-012):
  - "Copy row" (Ctrl+C) puts these lines on the clipboard:
    ```
    Name: <id>
    Display name: <Description>
    Status: <Status>
    Start type: <Startup type>
    PID: <MainPID>
    User: <User>
    Binary: <ExecStart[0].path>
    ```
  - With multi-select, the blocks are separated by a blank line.
  - "Copy rows as table" (Ctrl+Shift+C) copies the selected rows as tab-separated values of the visible columns, with a header line.
  - "Copy" copies the clicked cell.
  - Toasts: "Service details copied." / "Could not copy service details.".
- Linux data/mechanism: Wayland clipboard through GPUI `write_to_clipboard`
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-010 Jump between services and processes
- Was: SYS-010
- Source: TS L28709-28710, L28789-28790, L28830, L28700
- Behaviour:
  - Services "Jump to process" switches to Processes and selects MainPID by identity (pid plus start time) through merged PROC-071. Toast "Switched to service process.". If filtered out or hidden: "Service process not visible in the current process view.". When MainPID is 0: "Service not running".
  - The reverse, Processes "Go to service" (owned by merged `proc.md` PROC-071, the shared "select process by identity" entry point), switches to Services, expands the group and selects the unit. Toasts "Switched to Services tab" / "Switched to service." / "Service not found in current view".
- Linux data/mechanism: process to unit lookup through `/proc/<pid>/cgroup`. The last path element ending in `.service` (or any unit suffix) under `system.slice`, or under `user@<uid>.service`, gives manager and unit. Unit to process uses MainPID.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-011 Service Properties window: frame and General tab
- Was: SYS-011
- Source: TS 3.2 L28932-28958, L30268-30277, "Too many service properties windows are already open" L28714 | MC service_details_dialog
- Behaviour:
  - Frame: a separate top-level window (APP-045) titled "<id> Properties", with a pin button (APP-009, R-073).
  - Up to 10 can be open at once. The 11th gives the toast "Too many service properties windows are already open".
  - Values refresh live through PropertiesChanged.
  - Tabs: General, Log On, Recovery, Dependencies, Logs. Logs comes from Mission Center.
  - Buttons: Start, Stop, Restart, OK, Cancel, Apply. OK and Apply commit the Startup combo through SYS-007. The toast "Changes applied" appears on success.

  General fields:

  | Field | Value |
  |---|---|
  | Name | Id |
  | Display | Description |
  | Desc | Documentation URIs; `man:` and `https:` are clickable |
  | Path | full ExecStart argv joined, shell-quoted |
  | Unit file | FragmentPath |
  | Drop-ins | DropInPaths |
  | Startup | combo, as in SYS-007 |
  | Status | "Running (running) since <ActiveEnterTimestamp>", time as `YYYY-MM-DD HH:MM:SS` local |
  | Main PID | link that jumps to the process |
  | Memory | MemoryCurrent, IEC two decimals |
  | Tasks | TasksCurrent |
  | CPU time | CPUUsageNSec as `h:mm:ss` |
  | Manager | System or User |

  - Missing values use the PERF-013 words (R-050). Mission Center's empty Description bug does not apply.
- Linux data/mechanism: Unit and Service properties as named, through GetAll on open and PropertiesChanged afterwards
- Verdict: ADAPT
- Phase: P2
- Privilege: as SYS-006 and SYS-007; viewing needs none

### SYS-012 Service Properties: Log On tab
- Was: SYS-012
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
  | Session | "graphical session" for user units bound to `graphical-session.target`, else "none" |

  - Missing values show "N/A", as in Mission Center.
  - "Interact with desktop" has no systemd meaning; the Session field takes its place.
- Linux data/mechanism: Service properties `User`, `Group`, `DynamicUser`, `SupplementaryGroups`, `PAMName`; `/proc/<pid>/status` Uid and Gid; `PartOf`/`BindsTo` containing `graphical-session.target`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-013 Service Properties: Recovery tab
- Was: SYS-013
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

  - P2 shows these read-only. P3 makes them editable; Apply writes a drop-in.
- Linux data/mechanism:
  - System units: tl-helper operation `WriteServiceDropIn { manager: System, unit, keys limited to Restart*, StartLimit*, FailureAction, OnFailure }` (R-004) writes the fixed path `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`. tasklens then calls `Manager.Reload()`.
  - User units: `~/.config/systemd/user/<unit>.d/50-tasklens-recovery.conf`, written directly and atomically (`write_atomic`, ADR-011), then `Reload()` on the user manager.
  - `SetUnitProperties` cannot change `Restart=` on non-transient units, so drop-ins are required.
- Verdict: ADAPT
- Phase: P2 view, P3 edit
- Privilege: system-unit edits need tl-helper under polkit `io.github.oysteinkrog.tasklens.helper` (auth_admin_keep) (R-002). Without the helper the fields stay read-only, with tooltip "Needs the tasklens helper". User-unit edits need none.

### SYS-014 Service Properties: Dependencies tab
- Was: SYS-014
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
- Was: SYS-015
- Source: MC service_details_dialog Logs expander (last 5 messages, `UNIT=` or `_PID=`, current boot, copy button) | R-074
- Behaviour: Logs tab in Service Properties.
  - Monospace list of the last 200 entries from the current boot, newest at the bottom.
  - Each line shows time, priority and message. Priority 0-3 uses the theme danger colour; priority 4 uses warning.
  - Controls: filter box, "Follow" toggle (default on) for live tail, "Load older" (200 more per click), "Copy logs to clipboard", "Open in terminal". "Open in terminal" runs `journalctl -u <id> -b` in the default terminal through `xdg-terminal-exec`, falling back to `konsole -e`.
  - Mission Center shows 5 lines, which is too few to be useful.
- Linux data/mechanism (R-074): a `journalctl` child process run on the blocking pool:
  - system unit: `journalctl -u <id> -b -n 200 -o json --no-pager`;
  - user unit: `--user-unit <id>` in place of `-u <id>`;
  - follow mode adds `-f`; the child is killed when the tab closes;
  - the non-follow fetch has a 5 s timeout, then kill and wait.
  The JSON fields used are `__REALTIME_TIMESTAMP`, `PRIORITY`, `MESSAGE` and `_PID`. Using the CLI avoids dlopen and unsafe FFI for a feature off the hot path.
- Verdict: ADAPT
- Phase: P2
- Privilege: reading other units' entries needs membership in `wheel`, `adm` or `systemd-journal` (Arch journal ACLs). Without it, only the user's own entries come back. The tab then shows a banner: "Only your own log messages are visible. Add your user to the systemd-journal group to see all." The Additional Setup window (APP-042) has a "Journal access" row with the command.

### SYS-016 Services status bar and loading state
- Was: SYS-016
- Source: TS L28439, L28829 | MC placeholders | R-060
- Behaviour:
  - Status bar: "%zu Total | %d Running | %d Stopped". Running counts active units; Stopped counts inactive plus failed. Counts follow the current filters.
  - With a selection, it adds " | %d selected".
  - First load in a session shows skeleton rows and the toast "Loading service data..." until both managers answer or fail (R-060).
  - F5 reloads the lists and shows "Services refreshed.".
- Linux data/mechanism: as SYS-001; F5 sends `RefreshNow(SystemdSystem)` and `RefreshNow(SystemdUser)`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-017 Services sort and column persistence
- Was: SYS-017
- Source: TS INI Sort.ServicesKey=0/Ascending=1, Columns.Services, ColumnWidths.Services, ServiceVisibilityMask=7F | MC services-page-sorting-*, -column-order
- Behaviour:
  - Default sort is Name ascending (R-011). Groups stay grouped while sorting.
  - Column order, width and visibility are stored by column id (APP-033).
  - Default widths in logical px at 100%: name 250, display_name 300, status 100, startup_type 120, pid 80, user 200, path 260.
- Linux data/mechanism: `state.toml` `[sort.services] column = "name", ascending = true` and `[columns.services]` (same shape as `[columns.processes]`).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-018 OpenRC backend
- Was: SYS-018
- Source: MC PL:services/openrc (librc, `pkexec rc-service`, `rc-update`)
- Behaviour: On systems without systemd, the Services tab lists OpenRC services with Status from `rc-status`. Start, Stop and Restart call `rc-service`; enable and disable call `rc-update add|del <svc> default`. Properties shows only General. Logs, Recovery and cgroup columns are hidden.
- Linux data/mechanism: `rc-status --all --format ini`; `pkexec rc-service <svc> start|stop|restart`; `pkexec rc-update`
- Verdict: BUILD
- Phase: P4
- Privilege: pkexec (auth_admin) for actions, under `org.freedesktop.policykit.exec`; listing needs none

## Startup Apps tab

### SYS-019 Startup sources and precedence
- Was: SYS-020
- Source: TS 2.5 | PLAN data sources | review "Startup applications" row
- Behaviour: One row per effective entry, from these sources:
  1. XDG autostart, user: `$XDG_CONFIG_HOME/autostart/*.desktop`.
  2. XDG autostart, system: each dir in `$XDG_CONFIG_DIRS` (default `/etc/xdg`) + `/autostart`.
  3. systemd user units enabled into `default.target`, `graphical-session.target` or any `plasma-*.target`, from the user manager's WantedBy closure.
  4. KDE only: Plasma pre-startup scripts in `~/.config/plasma-workspace/env/*.sh`.
  5. KDE only: logout scripts in `~/.config/plasma-workspace/shutdown/*`.

  - Precedence follows the XDG Autostart spec. Same file name: user beats system, and an earlier `XDG_CONFIG_DIRS` entry beats a later one. A shadowed system entry is shown once, with Type "Autostart (system, overridden)".
  - An entry is effectively disabled when any of these hold: `Hidden=true`; `X-GNOME-Autostart-enabled=false`; `OnlyShowIn`/`NotShowIn` excludes `$XDG_CURRENT_DESKTOP`; `TryExec` is not found on PATH.
  - Generated `app-*@autostart.service` units are not listed a second time under source 3; they are linked to their desktop entry.
- Linux data/mechanism:
  - Engine worker `SourceId::Startup`, `workers/startup.rs` (ADR-004). A Desktop Entry spec parser in tl-collect.
  - Read on page open and on `RefreshNow` (ADR-008). While the Startup Apps tab is visible, inotify on all autostart dirs and the two Plasma dirs, plus the user manager's `UnitFilesChanged` signal, trigger a rescan; both stop when the tab is not visible (R-082, ADR-008).
  - Units come from `ListUnitFilesByPatterns` and `ListUnitsByPatterns` on the user manager.
  - Desktop entries map to units by escaping the id with systemd-escape rules: `app-<escaped id>@autostart.service`.
- Verdict: ADAPT (Run keys, Startup folders and packaged tasks become XDG autostart, systemd user units and Plasma scripts)
- Phase: P2
- Privilege: none

### SYS-020 Startup columns and values
- Was: SYS-021
- Source: TS 2.5 (Columns.StartupApps=0,1,2,6,3,4,5; widths 220,180,90,100,190,260,170) L28418-28446
- Behaviour: Columns in display order, with ids and default widths (logical px at 100%): Name (`name`, 220), Publisher (`publisher`, 180), Status (`status`, 90), Startup impact (`impact`, 100), Type (`type`, 190), Location (`location`, 260), Command (`command`, 170).

  | Column | Value |
  |---|---|
  | Name | desktop `Name` (localised) or unit id |
  | Publisher | owning package as "name version"; "User" for files under $HOME not owned by a package; blank if unknown |
  | Status | see list below |
  | Startup impact | SYS-021 |
  | Type | see list below |
  | Location | directory path |
  | Command | `Exec`, `ExecStart` or the script path |

  - Status values: Enabled, Disabled, Masked, Not in this desktop, Missing program. The last two replace TaskSlinger's "(Policy)" variants: entries whose state the user cannot get by toggling.
  - Type values: Autostart (user), Autostart (system), Autostart (system, overridden), systemd user unit, Plasma pre-startup script, Plasma logout script.
- Linux data/mechanism: a package owner index in tl-collect, shared with the process Package column (R-007). It parses `/var/lib/pacman/local/*/files` once into a path-to-package map. The map is built lazily on first use and rebuilt when the mtime of `/var/lib/pacman/local` changes. On Debian-family systems it reads `/var/lib/dpkg/info/*.list` instead. Other distros leave Publisher blank.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-021 Startup impact
- Was: SYS-022
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

### SYS-022 Enable and disable
- Was: SYS-023
- Source: TS context Disable/Enable, toasts L28797-28808 | review "Hidden=true applies to desktop entries, not systemd units"
- Behaviour:
  - The context item reads "Disable" or "Enable" by state, and works on multi-select.
  - Toasts "Startup entry enabled." / "Startup entry disabled." / "Failed to update startup entry".
  - Changes take effect at next login; running programs are not stopped.
  - "Not in this desktop" and "Missing program" entries show the item disabled, with the reason as tooltip.
- Linux data/mechanism (`ActionKind::StartupEnable` / `StartupDisable`, ADR-005; time budget 2 s per ADR-006):
  - XDG system entry, disable: write `~/.config/autostart/<id>.desktop` as a full copy of the system file plus `Hidden=true` and `X-Tasklens-Override=true`. This is the KDE Autostart module's method.
  - XDG system entry, enable: delete that user file if it carries `X-Tasklens-Override=true`; otherwise set `Hidden=false` and remove `X-GNOME-Autostart-enabled=false`.
  - XDG user entry: toggle `Hidden` in place.
  - All writes use `write_atomic` (ADR-011): temp file in the same dir, fsync, rename, fsync the dir. Unknown keys and comments are kept.
  - systemd user unit: `EnableUnitFilesWithFlags` / `DisableUnitFilesWithFlags` on the user manager, then `Reload()`.
  - Plasma scripts: toggle the executable bit (Plasma runs only executable scripts).
- Verdict: ADAPT
- Phase: P2
- Privilege: none, because system entries are overridden per user and never edited

### SYS-023 Run now
- Was: SYS-024
- Source: TS "Run now" (WdcRunTaskAsInteractiveUser, cross-check A.32), toasts "Startup entry launched." / "Failed to launch startup entry"
- Behaviour: Starts the entry now, whatever its Status. Toasts "Startup entry launched." / "Failed to launch startup entry". If it is already running as its autostart unit, the toast reads "Already running".
- Linux data/mechanism:
  - If `app-<id>@autostart.service` exists: `StartUnit(…, "replace")` on the user manager.
  - Otherwise: the shared launcher in tl-engine (also used by APP-014 Run). It parses `Exec` per the Desktop Entry spec: field codes %f %F %u %U removed, %i/%c/%k expanded, quoting rules applied.
  - The launcher runs `posix_spawn` with `setsid`, with stdio set to /dev/null and cwd from `Path=`.
  - It then calls `StartTransientUnit("app-tasklens-<escaped id>-<random hex>.scope", "fail", [("PIDs",[pid]),("Description",Name),("CollectMode","inactive-or-failed")], [])` on the user manager, so app grouping works. A failed scope creation is logged and not fatal.
  - systemd unit: `StartUnit`. Script: spawned the same way through `/bin/sh <path>`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-024 Delete entry
- Was: SYS-025
- Source: TS "Delete entry..." and dialog "Delete startup entry" L29399-29404
- Behaviour:
  - Confirm window (separate pinnable window, modal to the main window, APP-045). Window title "tasklens Delete Startup Entry". Heading "Delete startup entry", text "Permanently remove this startup entry?", a type-specific line, and buttons "Delete" (danger) and "Cancel".
  - Type-specific lines: user XDG entry: "The autostart file will be deleted. The app stays installed."; user systemd unit: "The unit will be disabled and its unit file deleted."; Plasma script: "The script file will be deleted.".
  - Entries that cannot be deleted show the item disabled with the reason, and the toast "This startup entry cannot be deleted" if triggered by key. These are system XDG entries and package-owned units (use Disable instead), because pacman would restore them.
  - Other toasts: "Startup entry deleted." / "Failed to delete startup entry".
- Linux data/mechanism: `unlink`. For a unit: `DisableUnitFiles`, then unlink the file in `~/.config/systemd/user`, then `Reload()`.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-025 Startup context menu, copy and open
- Was: SYS-026
- Source: TS L29293-29297, copy format L35521-35528 (cross-check A.3), toasts "Startup command copied.", "Startup file location unavailable."
- Behaviour:
  - Menu: Disable/Enable, Run now, separator, Copy, Copy row, Copy command, Open file location, Properties, separator, Delete entry….
  - "Copy row" (Ctrl+C, R-012) format:
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
    Resolved path is the absolute path of the binary found on PATH. Multi-select blocks are separated by a blank line.
  - "Copy rows as table" (Ctrl+Shift+C): TSV of the visible columns with a header line. "Copy" copies the clicked cell.
  - "Copy command": toast "Startup command copied.".
  - Open file location: `FileManager1.ShowItems` on the source file. If the file is gone: "Startup file location unavailable.".
- Linux data/mechanism: as above
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-026 Startup Properties window
- Was: SYS-027
- Source: TS L28959-28972, title "TaskSlinger Startup Properties" L29326
- Behaviour: A separate pinnable window titled "Startup properties: <Name>", read-only, with a copy button per line. Opened by Properties (Enter, Alt+Enter, Ctrl+I). Lines:

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

### SYS-027 Startup status bar, sort and persistence
- Was: SYS-028
- Source: TS L28437-28438; Sort.StartupAppsKey=2, StartupAppsStatusDefault=1; StartupVisibilityMask=7F | R-060
- Behaviour:
  - Status bar: "%zu Total | %d Enabled | %d Disabled", plus " | %d selected".
  - Default sort is Status ascending, so Enabled comes first, with Name ascending as tie-break (R-011).
  - Columns are persisted as in APP-033.
  - First load in a session shows skeleton rows until the first data (R-060).
  - F5 rescans and shows "Startup entries refreshed.".
- Linux data/mechanism: `state.toml` `[sort.startup] column = "status", ascending = true` and `[columns.startup]`; F5 sends `RefreshNow(Startup)`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Connections tab

Mission Center's "connections" request lists network interfaces, not sockets, so this tab exists only in TaskSlinger.

### SYS-028 Connection inventory
- Was: SYS-030
- Source: TS 2.6 | PLAN data sources | review sampling table (connections every 2 s while consumed)
- Behaviour:
  - Lists every TCP and UDP socket, IPv4 and IPv6, in tasklens's network namespace.
  - Refreshes every 2 s while the tab is visible or the socket map is in demand (the process Network column shares it). Collection is off otherwise (ADR-008).
  - Rows keep identity across refreshes by socket cookie, so selection is stable.
  - Holding Ctrl pauses the display as everywhere (R-042).
  - First load in a session shows skeleton rows until the first data (R-060).
- Linux data/mechanism:
  - Engine worker `SourceId::Connections`, `workers/connections.rs` (ADR-004).
  - Netlink `NETLINK_SOCK_DIAG`, `SOCK_DIAG_BY_FAMILY`, `inet_diag_req_v2`, for AF_INET and AF_INET6 × IPPROTO_TCP and IPPROTO_UDP.
  - `idiag_states=0xFFF`, with `INET_DIAG_INFO` requested for TCP.
  - Fields used: `id.{src,dst,sport,dport,if,cookie}`, `state`, `rqueue`, `wqueue`, `uid`, `inode`, and `tcp_info`.
  - Crates: `netlink-packet-sock-diag` and `netlink-sys`.
  - Unprivileged dumps work; the `inet_diag`/`udp_diag` modules load on demand.
- Verdict: ADAPT (TCP and UDP tables become sock_diag; IPv6 included)
- Phase: P2
- Privilege: none to list

### SYS-029 Connection columns, address formats, states
- Was: SYS-031
- Source: TS 2.6 L28422-28426, L27922-27939 (widths 90,260,260,120,80,180; Columns.Connections=0..5)
- Behaviour:
  - Default columns, with ids and default widths (logical px at 100%): Proto (`proto`, 90), Address local (`local`, 260), Address foreign (`foreign`, 260), State (`state`, 120), PID (`pid`, 80), Process (`process`, 180).
  - Optional columns: User (`user`), Family (`family`), Local port (`local_port`), Foreign port (`foreign_port`), Recv-Q (`recv_q`), Send-Q (`send_q`), Interface (`interface`), Inode (`inode`), Service (`service`, owning systemd unit), Remote host (`remote_host`, SYS-035).

  | Column | Value |
  |---|---|
  | Proto | "TCP" or "UDP" |
  | Address local / foreign | `a.b.c.d:port` or `[v6addr%scope]:port` (table d) |
  | State | see below |
  | PID | SYS-030 |
  | Process | SYS-030 |

  Address details (table d):
  - Wildcards show as `0.0.0.0:*` and `[::]:*`.
  - The foreign side of a listener or unconnected UDP socket shows `*:*`.
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

  - DELETE-TCB has no Linux state and is not used. UDP shows "UNCONNECTED" or "CONNECTED".
  - Recv-Q and Send-Q are counts with thin-space grouping (table d).
- Linux data/mechanism: as SYS-028; Interface from `if_indextoname(id.if)`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-030 Owning process attribution
- Was: SYS-032
- Source: TS PID/Process columns, "No owning process for this connection" | PLAN (pid map from /proc/*/fd every 2 s) | review "Network attribution" row
- Behaviour:
  - PID shows the owning pid. When several processes share the socket (after fork), it shows "pid +N" like the process table, and the tooltip lists all of them.
  - Process shows the display name and icon.
  - TIME-WAIT and orphaned sockets (inode 0): PID "-", Process "No owner (kernel)".
  - Sockets of other users whose `/proc/<pid>/fd` cannot be read: PID "?", Process shows the restricted glyph (ADR-003). The tooltip reads "Owned by <user from diag uid>. Needs the tasklens helper to identify the process". The value is never blank and never 0.
- Linux data/mechanism:
  - Walk `/proc/<pid>/fd/*` with `readlinkat` and match `socket:[<inode>]`, producing inode to `ProcessKey{pid,start_ticks}`.
  - Rebuilt every 2 s while in demand, on the blocking pool, with one outstanding job at a time.
  - Budget target: under 20 ms per rebuild on this machine, measured in phase 0 (unverified, U-23).
  - The same map feeds the process TCP Network column.
  - From P3, tl-helper operation `ResolveSocketInodes { inodes }` (R-004) resolves inodes for other users' processes. Request: a list of inodes. Reply: a list of pids.
- Verdict: ADAPT
- Phase: P2 (own processes), P3 (helper for others)
- Privilege: none for own processes. Others need tl-helper under polkit `io.github.oysteinkrog.tasklens.helper` (auth_admin_keep) (R-002). The fallback is the restricted glyph.

### SYS-031 Close TCP connection
- Was: SYS-033
- Source: TS "Close TCP connection" (SetTcpEntry, IPv4 only), toasts L28809-28814 | review "Unsupported feature parity" (defer TCP destruction)
- Behaviour:
  - Shown for TCP rows in any state except TIME-WAIT, for IPv4 and IPv6. TaskSlinger's "Only IPv4 TCP connections are supported" never appears.
  - LISTENING rows first ask for confirmation in a separate modal window. Heading "Close listening socket?", text "The program will stop accepting new connections on <addr>.", buttons "Close socket" (danger) / "Cancel".
  - Toasts: "TCP connection close requested", then, replaced in place, "TCP connection closed" or "Failed to close TCP connection: <errno text>".
- Linux data/mechanism: tl-helper operation `SockDestroy { family, proto, four_tuple, cookie }` (R-004) sends the `SOCK_DESTROY` netlink message with `inet_diag_req_v2` filled from the exact 4-tuple, family, protocol and `id.cookie`. The kernel matches the cookie, so a reused 4-tuple cannot be hit. Kernel requirement `CONFIG_INET_DIAG_DESTROY=y` is verified on this machine. If the kernel returns EOPNOTSUPP, the menu item is disabled with tooltip "Kernel lacks CONFIG_INET_DIAG_DESTROY".
- Verdict: ADAPT
- Phase: P3 (needs tl-helper)
- Privilege: CAP_NET_ADMIN, through tl-helper under polkit `io.github.oysteinkrog.tasklens.helper` (auth_admin_keep) (R-002). Before P3 the item is hidden. With the helper not installed, it is disabled with tooltip "Needs the tasklens helper".

### SYS-032 Connections context menu and copy formats
- Was: SYS-034
- Source: TS L29298-29301, copy format L35529-35536 (cross-check A.3), toasts L28657-28688
- Behaviour:
  - Menu: Copy row, Copy local address, Copy foreign address, separator, Close TCP connection, separator, Jump to process, Go to service (when the owner belongs to a systemd unit), Properties.
  - "Copy row" (Ctrl+C, R-012) format:
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
    Multi-select blocks are separated by a blank line.
  - "Copy rows as table" (Ctrl+Shift+C): TSV of the visible columns with a header line.
  - Toasts: "Connection copied", "Local address copied", "Foreign address copied".
  - "Go to service" switches to Services and selects the unit, with the toasts from SYS-010.
- Linux data/mechanism: clipboard; owning unit from the owner's `/proc/<pid>/cgroup` as in SYS-010
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-033 Jump to owning process
- Was: SYS-035
- Source: TS L28711, L28812-28814
- Behaviour: Switches to Processes, expands the group or tree path and selects the owner by ProcessKey. Toast "Switched to owning process.". If filtered out: "Owning process not visible in the current process view.". With no owner: "No owning process for this connection". If the pid exited: "Process not found in current view".
- Linux data/mechanism: identity lookup in the process snapshot, through the shared entry point in merged `proc.md` PROC-071
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-034 Connection Properties window
- Was: SYS-036
- Source: TS L28960, L28979-28983, title "TaskSlinger Connection Properties" L29327
- Behaviour:
  - A separate pinnable window titled "Connection properties: <Proto> <State>", with a copy button per line. Opened by Properties (Enter, Alt+Enter, Ctrl+I). Lines:
    - "Protocol / State / Family"
    - "Local <addr> Port <n>"
    - "Foreign <addr> Port <n>"
    - "Process <name> PID <n>" (link that jumps)
    - "User", "Inode", "Interface", "Recv-Q / Send-Q"
    - TCP only: "RTT <ms> (var <ms>)" (ms with two decimals, table d), "Congestion window", "Bytes sent / received" (`tcpi_bytes_acked`/`tcpi_bytes_received`, IEC two decimals), "Retransmits", "Last data received <ms> ago"
    - "Closable here: Yes/No (reason)"
  - TaskSlinger's "Protocol rank / State rank" are internal sort keys with no meaning to the user. The tcp_info lines take their place.
  - The window refreshes every 2 s while open, by cookie. When the socket closes, it shows "(closed)" and keeps the last values.
- Linux data/mechanism: `INET_DIAG_INFO` (`struct tcp_info`)
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### SYS-035 Remote host name column
- Was: SYS-037
- Source: none (tasklens addition replacing Windows netstat -f)
- Behaviour: Optional column "Remote host", hidden and off by default because lookups leak queries. When enabled, it shows the reverse DNS name, or the address while the lookup is pending.
- Linux data/mechanism: `getnameinfo(NI_NAMEREQD)` on the blocking pool. At most 4 concurrent lookups. Positive answers are cached for 10 minutes, negative ones for 1 minute.
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### SYS-036 Connections status bar, filter, sort, refresh
- Was: SYS-038
- Source: TS L28435, Sort.ConnectionsKey=0 asc, ConnectionVisibilityMask=3F
- Behaviour:
  - Status bar: "%zu Total | %d TCP | %d UDP | %d EST | %d LISTEN". EST and LISTEN count TCP only. With a selection it adds " | %d selected".
  - The text filter matches substrings in Proto, both addresses with ports, State, PID, Process and User.
  - Default sort is Proto ascending, then local port ascending (R-011).
  - F5 refreshes now and shows "Connections refreshed.".
- Linux data/mechanism: engine projection; `state.toml` `[sort.connections] column = "proto", ascending = true` and `[columns.connections]`; F5 sends `RefreshNow(Connections)`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### SYS-037 Other network namespaces
- Was: SYS-039
- Source: none (Linux gap; containers live in their own namespace)
- Behaviour: A "Namespaces" dropdown in the status bar, default "Host only". Choosing "All" adds a Namespace column (`netns`, container or netns name) and lists sockets from every network namespace.
- Linux data/mechanism: tl-helper operation `NetnsSockDiagDump { netns_ino }` (R-004), once per unique `/proc/*/ns/net` target. The helper calls `setns(CLONE_NEWNET)` in a worker thread and dumps sock_diag there. Names come from `/run/netns` and from the cgroup of a member process.
- Verdict: ADAPT
- Phase: P4
- Privilege: root through tl-helper under polkit `io.github.oysteinkrog.tasklens.helper` (R-002). Without it, only "Host only" is offered.

# APP: application shell

## Window chrome

### APP-001 Window decorations (client-drawn title bar by default)
- Was: APP-001
- Source: TS 1.1 | MC Adw header bar | gpui-component title_bar.rs:258, widgets §2 | D-006, R-021
- Behaviour:
  - By default tasklens draws its own 34 px title bar in TaskSlinger's layout (APP-002) (D-006).
  - Setting `appearance.decorations = "client"` (`client | server`). The settings label is "Use system title bar"; checking it sets `server`. A change takes effect after a restart; the toast reads "Restart tasklens to apply the title bar change" (R-083).
  - In `server` mode KWin draws the title bar. The menus and toolbar move into a 28 px row under it, then the tab strip. The caption buttons are omitted except the pin, which stays in that row.
  - On compositors without xdg-decoration, GPUI uses client-side mode whatever the setting says.
  - Predetermined fallback (ADR-010, U-36): if any phase-0 CSD check fails (drag, double-click maximize, restore, resize from all edges and corners, right-click window menu, tiling), the default becomes `server` with no further discussion.
- Linux data/mechanism: `WindowOptions { window_decorations: Some(Client), window_background: Opaque, window_min_size: Some(size(px(800.), px(500.))), app_id: Some("io.github.oysteinkrog.Tasklens"), window_bounds: Some(restored_bounds), ..TitleBar::window_options() }`, or `Server` when the setting says so. Root's WindowBorder supplies resize edges and the shadow. The Wayland `app_id` matches the installed `io.github.oysteinkrog.Tasklens.desktop`, so KDE shows the icon.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-002 Title bar layout and toolbar
- Was: APP-002
- Source: TS 1.1 [IMG processes.png, performance.png], cross-check A.8 | R-031
- Behaviour: Left to right:
  1. app icon (16 px);
  2. menus File, View, Settings, Help (Debug added when enabled, APP-026);
  3. toolbar buttons, which depend on the tab;
  4. drag area;
  5. caption buttons: pin, minimize, maximize, close (APP-008).

  Toolbar buttons by tab:
  - Every tab: Shield (APP-003, from P3 and only when tl-helper is installed) and Window picker "+" (APP-006).
  - Processes tab only: Hide system processes (APP-004) and View mode (APP-005). On other tabs these two are removed, not greyed. In History mode both stay active (R-031).
  - Mission Center's header buttons map elsewhere: Toggle Sidebar is a Performance-tab action (F9, Ctrl+T), and System Information is APP-041.

  Double-click on the drag area maximizes. Right-click opens the KWin window menu.
- Linux data/mechanism: gpui-component TitleBar (`start_window_move`, `zoom_window`, `show_window_menu`); toolbar buttons are ghost Buttons bound to Actions
- Verdict: ADAPT (update badge removed, APP-007)
- Phase: P1
- Privilege: none

### APP-003 Shield: unlock privileged actions
- Was: APP-003
- Source: TS 1.1 `chrome.admin` (relaunch elevated), "Shift held at launch" L30398 | R-002, R-003, R-004
- Behaviour:
  - The icon is shield (locked) or shield-hi (unlocked).
  - Click starts a tl-helper session (one polkit prompt). Privileged actions then run without further prompts while the session lives.
  - The helper exits after 300 s with no request, or when tasklens exits (stdin EOF). The shield then shows locked again; the next privileged action starts a new session.
  - Click on an unlocked shield ends the session (lock).
  - Holding Shift during launch unlocks at start (table b). Setting `privilege.unlock_at_launch` does the same on every launch (APP-049).
  - Tooltip: "Unlock privileged actions" or "Privileged actions unlocked (click to lock)".
  - tasklens never relaunches the GUI as root: a root GUI on Wayland breaks the session bus, portals and theming.
- Linux data/mechanism: `pkexec /usr/lib/tasklens/tl-helper --session` under polkit `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`). Transport is the helper's stdin and stdout with `tl-model::wire` framing, max frame 64 KiB, multi-frame replies capped at 4 MiB per request (R-003). The helper allows only the ADR-006 operations plus the R-004 extensions, never a path, command or free-form file argument, and re-runs `open_verified` on every process target.
- Verdict: ADAPT
- Phase: P3
- Privilege: polkit auth_admin_keep. The button is hidden before P3 and when tl-helper is not installed.

### APP-004 Hide system processes button
- Was: APP-004
- Source: TS 1.1 `chrome.microsoft`, toasts L28738-28739, INI UI.HideMicrosoftProcesses | R-031
- Behaviour:
  - Toggle button, Processes tab only (removed on other tabs; active in History mode).
  - Tooltip "Hide system processes" / "Show system processes". Toasts "Hiding system processes" / "Showing system processes".
  - Which processes count as system (and the pseudo rows, R-032) is defined in the process spec (PROC-046, PROC-012).
- Linux data/mechanism: setting `processes.hide_system = false`; part of `EngineSettings`
- Verdict: ADAPT (Microsoft signer test becomes a Linux system classification)
- Phase: P1
- Privilege: none

### APP-005 View mode button
- Was: APP-005
- Source: TS `chrome.view_mode`, toast "View mode: %ls" L28638, cross-check B.1 | R-010
- Behaviour:
  - Processes tab only.
  - Each click cycles Tree View, then Task Manager Grouping, then List View (TaskSlinger string order), the same three choices as View > Process View.
  - Toast "View mode: <name>", for example "View mode: Tree View".
  - Tooltip "Toggle view mode (current: <name>)".
- Linux data/mechanism: sets `processes.mode` (`tree | grouped | flat`, default `grouped`)
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-006 Window picker "+"
- Was: APP-006
- Source: TS 7 window picker, tooltips L29201-29213, cross-check A.7 | R-046, R-076
- Behaviour:
  - Click "+": KWin asks the user to click a window.
  - Plain click on a window: tasklens selects that window's process in Processes.
  - Ctrl+click on a window: tasklens ends that process with SIGTERM, then gives a toast.
  - Esc cancels (table b, Picker context).
  - Toasts: "No window selected", "No window found".
  - Desktop and panel windows (plasmashell) are ignored, with "No window found".
  - Without the KWin bridge (not KDE, or `org.kde.KWin` not on the bus) the button is hidden (R-046). There is no X11 picker path in v1 (R-076).
- Linux data/mechanism:
  - Through the KWin bridge worker (`workers/dbus_kwin.rs`, `SourceId::KWin`, R-046) as `Command::KWin(KWinCommand::Pick)`.
  - Session D-Bus `org.kde.KWin /KWin org.kde.KWin.queryWindowInfo()` does the interactive selection and returns a map with `uuid`.
  - If the map has no `pid` (U-14), a one-shot KWin script `tasklens-pick`, loaded through `org.kde.kwin.Scripting.loadScript`, looks up `workspace.windowList()` by `internalId == uuid` and returns `pid` through the WindowBridge object.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### APP-007 Update badge
- Was: APP-007
- Source: TS `chrome.update_available` [CL 0.8.15, 0.8.18]
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (pacman and the localbuilds recipe update tasklens, so there is no in-app updater)
- Phase: P4 (recorded as dropped)
- Privilege: none

### APP-008 Caption buttons
- Was: APP-008
- Source: TS 1.1 `chrome.caption.*` [CL 0.8.28]
- Behaviour: Pin (always on top, accent fill when on), Minimize, Maximize/Restore, Close. Minimize and maximize appear when the compositor reports them (`window_controls()`). Close follows the close behaviour in APP-047.
- Linux data/mechanism: TitleBar controls; the pin is a custom button bound to the `ToggleAlwaysOnTop` action (APP-009)
- Verdict: BUILD
- Phase: P1 (pin P2)
- Privilege: none

### APP-009 Always on top and the pin
- Was: APP-009
- Source: TS View > Window > Always On Top L28493, INI Window.AlwaysOnTop | review (defer where unsupported) | R-046, R-073
- Behaviour:
  - The pin button is shown on the main window and on every secondary window (R-073). On the main window it is toggled by the pin, by View > Window > Always On Top, and by nothing else.
  - Only the main window's state persists, in `window.always_on_top = false`.
  - Where no mechanism exists, the pin and the menu item are disabled with tooltip "Not supported by this desktop".
- Linux data/mechanism:
  - KDE: `keepAbove` through the KWin bridge (R-046), `Command::KWin(KWinCommand::KeepAbove)`, run as a one-shot script `tasklens-cmd-<n>`. The window is found by pid and `resourceClass == "io.github.oysteinkrog.Tasklens"`, plus the caption for secondary windows.
  - X11: `_NET_WM_STATE_ABOVE` client message.
  - The bridge lives in tl-engine (`workers/dbus_kwin.rs`), never in tl-ui (ADR-001 rule 1).
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-010 Size, minimum size and geometry restore
- Was: APP-010
- Source: TS INI Window.Width/Height/X/Y/Maximized (1280×800), minimum size [CL 0.9.2] | MC window-width/height/is-maximized | gpui APIs (Wayland position cannot be restored) | R-022
- Behaviour:
  - Default size 1280×800 logical px. Minimum 800×500 logical px (R-022).
  - Width, height and maximized state are saved, debounced 500 ms and on close, and restored. On start the size is clamped to at least 800×500.
  - Position is never saved; the compositor places the window. tasklens does not pick an output at startup (zed#46378).
  - KWin quick tiling and snapping work as for any window, so Windows 11 snap layouts need no work.
- Linux data/mechanism: `window_min_size`, `observe_window_bounds`, `window_bounds()`; saved to `state.toml` `[window] width, height, maximized`
- Verdict: ADAPT (position not restored)
- Phase: P1
- Privilege: none

### APP-011 Single instance
- Was: APP-011
- Source: TS L35762, L35794 [CL 0.8.12] | MC GApplication single instance | review "Window lifecycle" | R-047
- Behaviour:
  - A second launch hands its arguments to the running copy and exits with code 0. `--tab` and `--select-pid` are forwarded; `--start-minimized` is ignored.
  - The running copy shows, raises and focuses its main window, or reopens it if it was closed to the tray.
- Linux data/mechanism (in tl-app before GPUI starts, ADR-010, R-047):
  1. `zbus::blocking::Connection::session()`, then request `io.github.oysteinkrog.Tasklens` with `RequestNameFlags::DoNotQueue`.
  2. If taken: call `org.freedesktop.Application.Activate({"activation-token": $XDG_ACTIVATION_TOKEN if set})` on `/io/github/oysteinkrog/Tasklens`. Forward arguments with `ActivateAction(name, parameter, platform_data)`: `tab` (string), `select-pid` (uint32). Then exit 0.
  3. If owned: serve `org.freedesktop.Application` at that path: `Activate`; `Open` returns an error; `ActivateAction` with `quit`, `tab`, `select-pid`. Calls reach the GPUI main thread through `async_channel::bounded(4)`. The owner calls `window.activate_window()` and, when the KWin bridge is up, also activates through it, because KWin may refuse GPUI's own token.
  4. No session bus: log a warning and run without single instance. There is no flock fallback.
  - Wayland focus-stealing prevention may only flash the taskbar entry; that is accepted (U-6).
- Verdict: ADAPT
- Phase: P1
- Privilege: none

## Menus

### APP-012 File menu
- Was: APP-012
- Source: TS 1.2
- Behaviour: Items:
  - "Run..." (Ctrl+N), see APP-014
  - "Search File Handles...", see APP-015
  - "Open System Monitor", see APP-016
  - separator
  - "Exit tasklens" (Ctrl+Q)

  Menu items appear only once their feature exists, so the menu never has dead items. F10 and Alt+F open this menu (table b).
- Linux data/mechanism: gpui-component PopupMenu from the title bar; keyboard hints come from the Actions
- Verdict: ADAPT
- Phase: P1 (Exit), later items in their own phase
- Privilege: none

### APP-013 Run Installer item
- Was: APP-013
- Source: TS 1.2 L28475
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (TaskSlinger installer front end; pacman installs tasklens)
- Phase: P4 (recorded)
- Privilege: none

### APP-014 Run dialog (window frame and privilege path)
- Was: APP-014
- Source: TS 8 "TaskSlinger Run" L29321-29338, runCommandHistory.json | R-061
- Behaviour:
  - Ownership (R-061): this entry owns the window frame and the administrator path. The content (text, combo, buttons, keys, toasts, history format, URL and `kcm_*` handling) is owned by merged `proc.md` PROC-095. The summary below matches it.
  - A separate pinnable window titled "tasklens Run", opened by File > "Run..." and Ctrl+N.
  - Text "Type the name of a program, folder, document, or Internet resource.", then "Open:" and a combo with history (5 visible rows, prefix matches first).
  - Keys (table b, RunDialog): Up and Down pick a history match, Enter runs, Shift+Enter runs as administrator.
  - Controls per merged PROC-095: "Run as administrator" checkbox, "Browse…" (Programs filter = executable files; All files), "Clear command history", OK (default), Cancel.
  - Toasts: "Command launched", "Command history cleared", "Please enter a command", "Command contains invalid text." (a NUL byte or a control character), "Failed to run command", "Authentication cancelled".
  - The dialog notes "GUI programs may not start as root on Wayland" next to the administrator checkbox.
- Linux data/mechanism:
  - URLs and existing paths go to the portal `org.freedesktop.portal.OpenURI` (or `xdg-open`). `kcm_*` names run `systemsettings <name>`.
  - Anything else runs as `/bin/sh -c <text>` through the shared launcher (SYS-023), in a transient scope `app-tasklens-run-<rand>.scope` on the user manager.
  - Run as administrator (R-061): `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c <text>` under `org.freedesktop.policykit.exec` with standard auth_admin. Plain pkexec, never tl-helper, so the helper never offers an arbitrary shell.
  - History: `$XDG_STATE_HOME/tasklens/run-history.json`, format `{"entries":[{"command","executedAt","elevated"}]}`, up to 50 entries.
  - Browse: GPUI `prompt_for_paths` (portal FileChooser). Phase 0 confirms it pulls in no Tokio (U-15); otherwise call `org.freedesktop.portal.FileChooser.OpenFile` through zbus directly.
- Verdict: ADAPT
- Phase: P2
- Privilege: none, or pkexec for Run as administrator

### APP-015 Search File Handles window (frame)
- Was: APP-015
- Source: TS 8 "Search File Handles" L29339-29345, L30071-30081 | R-061
- Behaviour:
  - Ownership (R-061): this entry owns the menu item and the window frame. Controls, result columns, progress strings, the result menu and the footer note are owned by merged `proc.md` PROC-096. The summary below matches it.
  - A separate pinnable, non-modal window titled "tasklens Handle Search", opened by File > "Search File Handles...".
  - Field "Pattern:" (case-insensitive substring; `*` and `?` globs allowed), checkbox "Include memory-mapped files, working directories and executables" (default on), Search, Clear, Close.
  - Result columns: Process, PID, User, Type (fd, mmap, cwd, exe, root), FD, Path.
  - Progress: "Initializing... please wait.", "Retrieving file handles... 42% (18 234 total handles)", "Searching...", then "Found 12 matches" or "No results found.".
  - Result menu: End Task (SIGTERM), End Process (SIGKILL), Go to process, Copy path, Open file location (R-102, PROC-096).
  - Unprivileged footer: "Searched 412 of 530 processes. 118 need administrator rights." with a "Search all" button.
- Linux data/mechanism: walk `/proc/*/fd` readlinks, `/proc/*/maps` paths, `cwd`, `exe` and `root` on the blocking pool, cancellable between pids. Other users' processes go through tl-helper `ReadProcessFiles` (R-004).
- Verdict: ADAPT
- Phase: P3
- Privilege: own processes need none. Others need tl-helper under polkit `io.github.oysteinkrog.tasklens.helper` (R-002).

### APP-016 Launch other system monitor
- Was: APP-016
- Source: TS "Launch Windows Task Manager" L28474, toast L28639
- Behaviour: The File item "Open System Monitor" starts the desktop's own monitor. It is hidden when none is installed. Failure gives the toast "Failed to launch System Monitor".
- Linux data/mechanism: the first found of `plasma-systemmonitor`, `gnome-system-monitor`, `missioncenter`, run through the shared launcher (SYS-023)
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### APP-017 View menu structure
- Was: APP-017
- Source: TS 1.3 | R-089
- Behaviour: Submenus, in order: "Columns >", "Process View >", "Window >", "Appearance >", "Theme >", "Graphs >". Alt+V opens it (table b).
- Linux data/mechanism: PopupMenu submenus
- Verdict: BUILD
- Phase: P1 (Columns, Process View, Window, Graphs), P2 (Appearance, Theme)
- Privilege: none

### APP-018 View > Columns (all tables)
- Was: APP-018
- Source: TS 1.3, header menu L29313-29314, L28434, L28484-28487, toasts L28648, L28652, cross-check A.10 | R-065, R-075, table (d)
- Behaviour: Applies to the active tab's table (Processes, History, Services, Startup Apps, Connections). The same items open from a right-click on a column header.
  - "Select Columns…": the "Columns" picker, a checklist with a filter field, grouped under headings. Process headings (PROC-024): Image, Identity, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux. Entries show alias text, for example `Working set private - "Memory"` and `GPU memory dedicated - "VRAM"` (R-065). The filter matches full name and alias. Other tables list their columns without headings (inferred).
  - "Hide This Column" (header menu only).
  - "Resize columns to fit content": measures the table (d) template strings `"999999 +99"`, `"100.0%"`, `"xxx Mbps"` or `"xxx MiB/s"`, `"xxx GiB/s"`, `"999 MiB"`, `"999999"`, `"999:59:59"`. Text columns have a 60 px minimum.
  - "Resize Behavior: Proportional / Last Column" (`tables.column_resize`).
  - The display keys `tables.column_resize`, `tables.column_separators` and `tables.dim_zero_values` apply to every table; there are no per-table keys (R-075).
  - "Reset to default".
  - Toasts: "At least one column must stay visible", "Columns reset to default".
- Linux data/mechanism: tl-ui table state, persisted per APP-033 in `state.toml` `[columns.<table>]` (`order`, `visible`, `widths`)
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-019 View > Process View
- Was: APP-019
- Source: TS 1.3 L28488-28492 | R-010
- Behaviour: Three radio items "Tree View", "Task Manager Grouping", "List View" (`processes.mode`), then check items "Tree Connector Lines" (`processes.tree_guide_lines`) and "Highlight New and Exited Processes" (`processes.highlight_new_exited`). Behaviour is defined in the process spec.
- Linux data/mechanism: settings in table (a)
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-020 View > Window
- Was: APP-020
- Source: TS L28493 | R-058
- Behaviour:
  - "Always On Top" (check), see APP-009.
  - "Performance mini window" (check, Ctrl+Shift+M). The mini window itself is owned by the Performance spec (PERF-093).
- Linux data/mechanism: APP-009; the mini window per PERF-093
- Verdict: ADAPT
- Phase: P2 (Always On Top), P3 (Performance mini window)
- Privilege: none

### APP-021 View > Graphs
- Was: APP-021
- Source: TS 1.3 L28470-28471, L28483, L28543-28544 | R-089
- Behaviour:
  - Checks: "CPU Graph", "Memory Graph", "Disk Graph", "Network Graph", "GPU Graph", "VRAM Graph", "GPU Temperature Graph" (`graphs.minigraph_cpu`, `_memory`, `_disk`, `_network`, `_gpu`, `_vram`, `_gpu_temperature`).
  - Then "Keep detached graphs open when closing main window" (`graphs.keep_detached_on_close`) and "Reset Minigraph Layout" (toast "Minigraph layout reset"; resets `[[minigraphs]]` in `state.toml`).
  - The minigraph spec (PERF) owns what these graphs show and how they behave.
- Linux data/mechanism: settings `[graphs]`
- Verdict: BUILD
- Phase: P1 (graph checks and Reset, with the system minigraph cards, PERF-080 and table a), P3 (Keep detached graphs open)
- Privilege: none

### APP-022 Settings menu
- Was: APP-022
- Source: TS 1.4 L28494-28536, cross-check B.7 | R-015, R-025, R-028, R-058, D-003
- Behaviour: Group headings and items, in order. Alt+S opens it.
  - **Monitoring**
    - "Refresh >": 0.25s, 0.5s, 1.0s, 2.0s, 5.0s, 10.0s (`processes.interval_ms` 250, 500, 1000, 2000, 5000, 10000)
    - "Network transfer unit >": Bits per second / Bytes per second (`units.network = bits | bytes`)
    - "Auto-scale network transfer rates" (`units.network_auto_scale`, default off)
    - "Record history" (`history.enabled`, default off, D-003)
    - "History storage >": 1, 2, 3, 4, 6, 8, 12 hours (`history.retention_h`, default 6)
    - "Save history to disk" (`history.persist`, default on)
    - "Rendering GPU >" (`rendering.gpu`, APP-046)
  - **Motion & interaction**
    - "Animations" (`motion.animations`), "Smooth Graphs" (`motion.smooth_graphs`), "Smooth Scroll" (`motion.smooth_scroll`)
    - "Tab Switch" slider (0-500 ms, step 10, label "%d ms") (`motion.tab_switch_ms`)
    - "Close with Esc" (`window.close_with_esc`)
  - **Window & startup**
    - "Close to tray", "Minimize to tray", "Always show tray icon" (`tray.close_to_tray`, `tray.minimize_to_tray`, `tray.always_show`)
    - "Start at login" (`startup.start_at_login`), then an indented "Start minimized" (`startup.start_minimized`, disabled unless Start at login is on)
    - "Default tab >" (`general.start_tab`: Processes, Performance, Services, Startup Apps, Connections, Information, Last used)
    - "Unlock privileged actions at launch" (`privilege.unlock_at_launch`)
    - "Skip password prompts (experimental)" (`privilege.passwordless`)
    - "Auto-fetch Public IP on launch" (`network.auto_fetch_public_ip`, R-058)
    - "Open rescue window on critical memory pressure" (`lowmem.enabled`; owned by the process spec, PROC-097)
  - **Language**: "Language: >" with "System default", then the locales (APP-043) (`general.language`)
  - Bottom: "All settings…" (Ctrl+,), "Reset Settings to Defaults"

  - "Reset Settings to Defaults" opens a modal confirm window: "Reset all settings to their defaults? Themes and history are kept.", buttons "Reset" / "Cancel". Then toast "Settings reset to defaults", or "Failed to reset settings". It resets `settings.toml` only; `state.toml`, `themes.json` and history are kept (inferred from the confirm text).
  - The Additional Setup window is not in this menu. It opens from Help, from All settings > Privileges and from every "Needs setup" link (R-028).
- Linux data/mechanism: PopupMenu; every item writes through the settings store (APP-033)
- Verdict: ADAPT
- Phase: P1 (Refresh, units, Default tab, Close with Esc, motion items), others in their feature's phase
- Privilege: none

### APP-023 Settings window ("All settings…")
- Was: APP-023
- Source: MC Preferences dialog (General, Performance Page, Apps & Services, Units) | TS has menus only
- Behaviour:
  - A separate window titled "tasklens Settings", opened by "All settings…" and Ctrl+,. It uses the gpui-component `Settings` widget (page sidebar plus search).
  - Pages: General, Appearance, Motion, Units, Processes, Performance, Graphs, Services, Startup and window, Privileges, Advanced.
  - Every key of `settings.toml` in table (a) appears exactly once, with the same label as its menu item where one exists. `state.toml` keys do not appear.
  - Privileges page: the two privilege keys and an "Additional setup…" button (R-028).
  - Startup and window page: also "Open with Ctrl+Esc" (APP-051), which reads and writes kglobalaccel and has no settings key (inferred).
  - Advanced page: "Import from TaskSlinger…" (APP-034).
  - Changes apply live, except `appearance.decorations` and `rendering.gpu`, which need a restart.
  - TaskSlinger's quick menus are kept; this window adds the settings that have no menu item, mostly from Mission Center.
- Linux data/mechanism: gpui-component Settings/SettingPage/SettingField bound to the settings Global
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-024 Help menu
- Was: APP-024
- Source: TS 1.5 | MC main menu (Keyboard Shortcuts, Open Wiki, Show First Run Dialog, About) | R-028
- Behaviour: Items (Alt+H opens the menu):
  - "Keyboard Shortcuts" (Ctrl+?), see APP-030
  - "System Information…", see APP-041
  - "Additional setup…", see APP-042
  - "Documentation", which opens `/usr/share/doc/tasklens/README.md` with xdg-open; this replaces Mission Center's "Open Wiki"
  - separator
  - "About tasklens…", see APP-053
- Linux data/mechanism: PopupMenu
- Verdict: ADAPT
- Phase: P2 (Additional setup… P3, Documentation P4)
- Privilege: none

### APP-025 Purchase license, Enter license, early-bird notice
- Was: APP-025
- Source: TS 1.5 L28537-28539, 8 License and Early-bird dialogs, cross-check A.35, INI License.Token, WingetEarlyBirdNoticeShown
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (personal tool with no licence system; the purchase flow is TaskSlinger's commercial model)
- Phase: P4 (recorded)
- Privilege: none

### APP-026 Debug menu
- Was: APP-026
- Source: TS 1.6, hidden INI keys ShowFps/ShowDebugWindow/TripleBuffering | R-071
- Behaviour: Shown only with `--debug` or `TASKLENS_DEBUG=1`. Items:
  - "Show FPS" (`debug.show_fps`): overlay of frame time p50/p99 and fps.
  - "Continuous rendering": requests a frame every vsync.
  - "Enable all columns".
  - "Add random process minigraphs": toasts "Added %ld random process minigraphs." / "No process minigraphs available.".
  - "Collector health": window listing each collector's state, age, last error and restart count. This is the Linux analogue of ShowDebugWindow. It also opens from the status bar health text (APP-029). There is no setting for it (R-071).
  Dropped items:
  - Force D3D feature level: Direct3D only.
  - Triple buffering: wgpu present mode is fixed to Mailbox.
  - Clear text atlas cache: GPUI has no public API for it.
- Linux data/mechanism: GPUI `request_animation_frame` (debug only; ADR-009's AnimationDriver rule applies everywhere else); engine health snapshot
- Verdict: ADAPT
- Phase: P4 (Show FPS in P1 as a developer-only flag)
- Privilege: none

## Tabs, filter, status bar, shortcuts

### APP-027 Tab bar and tab animation
- Was: APP-027
- Source: TS 1.7 L28210-28216, INI MainTabAnimationMs=250, Startup.DefaultTab | MC window-selected-page | R-023, R-024
- Behaviour:
  - Six tabs in TaskSlinger order: Processes, Performance, Services, Startup Apps, Connections, Information (R-023). There are no Users, Details or App history tabs. Mission Center's Apps maps to Processes.
  - Processes carries a history icon button that toggles History mode; in History mode the tab label reads "History" (cross-check B.3). Information carries a copy icon (copy all).
  - The active tab is an accent outline joined to a 2 px accent line along the bar.
  - A switch slides the indicator and crossfades content over `motion.tab_switch_ms` (default 250). It is instant when `motion.animations` is off or the system asks for reduced motion (R-017).
  - Ctrl+1 to Ctrl+6 and Alt+1 to Alt+6 select tabs. Ctrl+Tab and Ctrl+Shift+Tab select the next and previous tab (table b).
  - When narrow, overflowing tabs go into a "»" menu (TabBar `.menu(true)`). There is no Mission Center bottom tab bar.
  - The start tab follows `general.start_tab` (`processes | performance | services | startup | connections | information | last_used`). `last_used` reads `state.toml` `[tabs] last_tab`, which is saved on every switch.
- Linux data/mechanism: gpui-component TabBar with custom outline styling; GPUI `Animation` (respects `reduce_motion()`). Page entities are created on first visit and kept; only the active page renders (ADR-009).
- Verdict: BUILD
- Phase: P1 (animation P2)
- Privilege: none

### APP-028 Filter box
- Was: APP-028
- Source: TS 1.7 L28339, L30133 [CL 0.8.28, 0.8.32] | MC search (typing forwards to search; name/PID; Levenshtein) | review "Sort/filter execution" | R-043
- Behaviour:
  - Sits at the right of the tab bar: "Filter: " label, input and clear button. 160 px wide, growing to 280 px on focus over 150 ms (instant with reduced motion).
  - Typing any printable character while a table has focus, and no other input or dialog has it, starts filtering.
  - Ctrl+F focuses the box. One Esc clears the filter and returns focus to the table; a further Esc follows the global Esc rule (table b) (R-043).
  - Each tab keeps its own filter text for the session.
  - Matching is case-insensitive substring.
    - Processes (R-043): display name, image name, command line, user, description, app name, unit name, group names, and PID by prefix for numeric input. There is no command-line toggle.
    - Services and Startup Apps: all visible columns.
    - Connections: SYS-036.
    - Information: field labels and values.
  - Mission Center's reverse-containment and Levenshtein matching is rejected because it gives false hits.
  - "Include children" checkbox (`processes.filter_include_children`): Processes only, shown while filtering or in Tree View (process spec).
  - The UI debounces input 100 ms and sends the query with a new generation; the engine projection filters, and the UI shows a result only when its generation is the latest sent (ADR-009).
- Linux data/mechanism: gpui-component Input `.cleanable(true)`; normalized name and command-line cache in the engine
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-029 Status bar
- Was: APP-029
- Source: TS 1.8 [CL 0.8.9]
- Behaviour: Compact bar, 22 px at 100% scale.
  - Left: the tab's own text. Processes: "%zu Processes" or "%zu / %zu Processes", plus " | %d processes selected". Services, Startup Apps and Connections: SYS-016, SYS-027, SYS-036. Performance and Information: blank.
  - While Ctrl is held and the main window is active: "[PAUSED - Ctrl]", appended in the accent colour (R-042).
  - Right: collector health. Nothing when all is well; otherwise "GPU data stale", "Restricted: 42 processes" and so on, each clickable to open Collector health (APP-026).
  - Counts use thin-space grouping (table d).
- Linux data/mechanism: gpui-component StatusBar; health from `SourceHealth` in the snapshot
- Verdict: ADAPT (right-side health text added per the review's validity rules)
- Phase: P1
- Privilege: none

### APP-030 Keyboard Shortcuts window
- Was: APP-030
- Source: MC Ctrl+? shortcuts dialog
- Behaviour: A separate pinnable window titled "tasklens Keyboard Shortcuts" (R-083), opened by Help > "Keyboard Shortcuts" and Ctrl+?. It lists APP-031 grouped by context: Global, Tables, Processes, Services, Performance, Dialogs (Run dialog, window picker), Startup. Key caps come from `Kbd::binding_for_action`.
- Linux data/mechanism: generated from the keymap
- Verdict: BUILD
- Phase: P2
- Privilege: none

### APP-031 Keyboard shortcuts
- Was: APP-031
- Source: TS 7 | MC 6 | cross-check C.4 | R-042, R-045, D-008, canonical table (b)
- Behaviour: The table below is canonical table (b), copied exactly. Q-3 is settled by D-008: Enter, Ctrl+I and Alt+Enter open Properties; Space toggles expand.

  Key contexts: Global, Table (every table), ProcessTable, ServiceTable, Performance, RunDialog, Picker. One GPUI Action per row; menu items dispatch the same Action.

  | Keys | Context | Action |
  |---|---|---|
  | Ctrl+1..Ctrl+6, Alt+1..Alt+6 | Global | Select tab 1 to 6 (Processes, Performance, Services, Startup Apps, Connections, Information) |
  | Ctrl+Tab / Ctrl+Shift+Tab | Global | Next / previous tab |
  | Ctrl+N | Global | Run… |
  | Ctrl+Q | Global | Exit tasklens |
  | Ctrl+W | Global | Close window (follows close behaviour) |
  | Ctrl+, | Global | All settings… |
  | Ctrl+? (Ctrl+Shift+/) | Global | Keyboard Shortcuts |
  | F10 | Global | Open File menu |
  | Alt+F / Alt+V / Alt+S / Alt+H | Global | Open File / View / Settings / Help menu |
  | Ctrl+F | Global | Focus filter |
  | F5 | Global | Refresh current tab now |
  | Esc | Global | Close topmost dialog or menu; else clear filter and focus the table; else close window if `window.close_with_esc`; else nothing |
  | Hold Ctrl | Global (main window active) | Pause display, "[PAUSED - Ctrl]" |
  | Ctrl+Plus / Ctrl+Minus / Ctrl+0 | Global | UI scale +5 / −5 / 100 |
  | Ctrl+Shift+M | Global | Performance mini window (P3) |
  | Up / Down / PgUp / PgDn / Home / End | Table | Move row selection (rows, never columns) |
  | Shift+Up/Down, Shift+click, Ctrl+click | Table | Extend or toggle selection |
  | Ctrl+A | Table | Select all visible rows |
  | Right / Left | Table | Expand, or move to first child / collapse, or move to parent |
  | Space | Table | Toggle expand |
  | Enter, Alt+Enter, Ctrl+I | Table | Properties (see Q-3) |
  | Ctrl+C | Table | Copy row (processes: "Copy details") |
  | Ctrl+Shift+C | Table | Copy rows as table (TSV with header) |
  | Ctrl+L | Table | Collapse all |
  | Shift+F10, Menu | Table | Open context menu for the selection |
  | Delete, Ctrl+E | ProcessTable | End Task (SIGTERM) |
  | Shift+Delete, Ctrl+X | ProcessTable | End Process (SIGKILL) |
  | Printable character | ProcessTable, other tables | Start filtering (not when an input or dialog has focus) |
  | Ctrl+S / Ctrl+E / Ctrl+R | ServiceTable (service row) | Start / Stop / Restart (Ctrl+E on a child process row acts on the process) |
  | Ctrl+C | Performance | Copy device info |
  | Ctrl+M | Performance | Graph Summary View |
  | F9, Ctrl+T | Performance | Toggle device sidebar |
  | F1..F8 | Performance | Select visible device 1 to 8 |
  | Up / Down | Performance sidebar focused | Move selection |
  | Left / Right, Shift+Left / Shift+Right | History scrubber | Step 1 s / 1 min (P3) |
  | Up / Down, Enter, Shift+Enter | RunDialog | History pick, Run, Run as administrator |
  | Esc | Picker | Cancel window picking |
  | Shift held at launch | Startup | Unlock privileged actions (P3) |

  Removed from APP-031: Enter as toggle expand; "Force end" naming.

  Notes:
  - "Hold Ctrl" pauses while the main window is active, whatever has focus inside it; it releases on Ctrl-up or window deactivation (R-042).
  - Ctrl+E means End Task on a process table and Stop on a service row.
- Linux data/mechanism: `cx.bind_keys` with key contexts "Global", "Table", "ProcessTable", "ServiceTable", "Performance", "RunDialog", "Picker". One GPUI Action per row; menu items dispatch the same Action. Ctrl-hold is detected with `on_modifiers_changed`, and window deactivation releases the pause.
- Verdict: ADAPT
- Phase: P1 (Global, Table and ProcessTable keys), P2 (ServiceTable, Performance extras, RunDialog), P3 (Ctrl+Shift+M, History scrubber, Picker, Shift at launch)
- Privilege: none

## Settings and persistence

### APP-032 Settings registry (every setting)
- Was: APP-032
- Source: TS 5 (UserSettings.ini) | MC 5 (gschema, 53 keys) | review "Settings persistence" | R-015 to R-021, R-024, R-025, R-044, R-054, R-069, R-071, R-075, R-084, R-085, R-095, R-100, canonical table (a)
- Behaviour: The schema below is canonical table (a), copied exactly. It is the only settings schema; no other key exists.

  Two files, both `#[serde(default)]`, both starting with `schema_version = 1`, both saved atomically with a 500 ms debounce (ADR-011). Ranges are clamped and logged on load. Column ids are the strings from R-007.

  `$XDG_CONFIG_HOME/tasklens/settings.toml`:

  ```toml
  schema_version = 1

  [general]
  start_tab = "processes"        # processes | performance | services | startup | connections | information | last_used
  language = "system"            # system | en-US | de-DE | es-ES | fr-FR | ja-JP | ko-KR | pt-BR | ru-RU | uk-UA | zh-CN | nb-NO

  [theme]
  active = "Default Dark"        # theme name
  follow_system = false
  light = "Default Light"
  dark = "Default Dark"

  [appearance]
  ui_scale_pct = 100             # 75..=200, step 5
  row_height_pct = 100           # 80..=150, step 5
  decorations = "client"         # client | server  (label "Use system title bar"; restart)
  rounded_corners = true
  heat_coloring = true
  heat_intensity_pct = 50        # 0..=100
  shadow_intensity_pct = 100     # 0..=100

  [motion]
  animations = true              # tab, theme, toast motion; ANDed with system reduce-motion
  smooth_graphs = true           # chart sliding
  smooth_scroll = true
  smooth_scroll_amount = 15      # 1..=50
  tab_switch_ms = 250            # 0..=500, step 10

  [units]
  network = "bits"               # bits | bytes
  network_auto_scale = false

  [processes]
  interval_ms = 1000             # 250..=10000; menu 250 500 1000 2000 5000 10000
  mode = "grouped"               # grouped | tree | flat
  tree_guide_lines = false
  highlight_new_exited = true
  filter_include_children = false
  hide_system = false
  subtree_totals = false         # "Show subtree totals on parent processes"
  dump_type = "mini"             # mini | full

  [tables]                        # every table (R-075)
  column_resize = "proportional" # proportional | last_column
  column_separators = false
  dim_zero_values = false

  [properties]
  hide_unnamed_handles = true
  shared_objects_only = true
  env_mask = true

  [performance]
  graph_range_s = 60             # 30 | 60 | 300 | 600 (300 and 600 from P2)
  cpu_graph_mode = "per_core"    # overall | per_core | all_cores_overlaid | all_cores_stacked
  cpu_kernel = true
  cpu_secondary_graph = "power"  # none | power | clock | temperature
  gpu_optional_graphs = []       # subset of ["power", "clocks"]
  hidden_gpu_engines = []        # engine keys
  memory_composition = true
  memory_swap_graph = true
  show_disks = true
  show_gpus = true
  show_fans = true
  show_batteries = true
  show_network_wired = true
  show_network_wireless = true
  show_network_vpn = true        # VPN and tunnel
  show_network_virtual = false
  show_network_other = true
  show_disconnected_fans = false
  device_order = []              # device keys (PERF-005)
  device_overrides = {}          # device key -> "show" | "hide"
  curved_lines = false
  network_dynamic_scale = true   # false = link speed

  [fans]                          # per fan key, P3
  # "<key>" = { label = "...", temp = "<hwmon name>:<path>:tempN" }

  [graphs]
  minigraph_cpu = true
  minigraph_memory = true
  minigraph_disk = true
  minigraph_network = true
  minigraph_gpu = true
  minigraph_vram = true
  minigraph_gpu_temperature = false
  keep_detached_on_close = false

  [history]                       # P3; D-003
  enabled = false
  retention_h = 6                # 1..=12; menu 1 2 3 4 6 8 12
  persist = true                 # false = delete segments at shutdown
  max_disk_mb = 256              # not in menus

  [window]
  close_with_esc = false
  always_on_top = false

  [tray]                          # P3
  close_to_tray = false
  minimize_to_tray = false
  always_show = false

  [startup]
  start_at_login = false
  start_minimized = false

  [network]
  auto_fetch_public_ip = false
  public_ip_notice_accepted = false

  [privilege]                     # P3/P4
  unlock_at_launch = false
  passwordless = false

  [rendering]                     # P4
  gpu = "auto"                   # auto | software | pci:<bus id>

  [services]
  unit_types = ["service", "socket", "timer", "mount"]
  filter_states = []             # subset of running failed stopped disabled

  [lowmem]                        # P3
  enabled = false

  [debug]
  show_fps = false
  ```

  `$XDG_STATE_HOME/tasklens/state.toml`:

  ```toml
  schema_version = 1

  [window]
  width = 1280                   # logical px; clamped to >= 800
  height = 800                   # clamped to >= 500
  maximized = false

  [tabs]
  last_tab = "processes"

  [processes]
  history_mode = false
  group_expanded = { apps = true, background = true, system = true }
  virustotal_notice_accepted = false   # R-095

  [services]
  group_expanded = { system = true, user = true }

  [performance]
  nav_width = 252                # 180..=400
  selected_device = "cpu"

  # one table per view: processes, history, services, startup, connections
  [sort.processes]
  column = "pid"
  ascending = true
  [sort.history]
  column = "cpu"
  ascending = false
  [sort.services]
  column = "name"
  ascending = true
  [sort.startup]
  column = "status"
  ascending = true
  [sort.connections]
  column = "proto"
  ascending = true

  [columns.processes]            # same shape for the other four tables
  order = ["name", "user", "pid", "cpu", "memory", "disk", "network", "gpu", "description"]
  visible = ["name", "user", "pid", "cpu", "memory", "disk", "network", "gpu", "description"]
  widths = { name = 248, user = 92, pid = 96, cpu = 63, memory = 92, disk = 82, network = 92, gpu = 72, description = 342, power = 101 }

  [[minigraphs]]                 # P1 hide and range, P3 colour and detach
  source = "cpu"                 # cpu | memory | disk | network | gpu | vram | gpu_temp | gpu:<device key> | vram:<device key> | gpu_temp:<device key> | process:<boot_id 16 hex>:<pid>:<start_ticks> | name:<image name> | app:<app id>
                                 # bare gpu, vram, gpu_temp = first GPU in sidebar order (R-085); boot ID per R-100
  column = "cpu"                 # for process sources
  range_s = 60
  color = "accent"               # accent | <one of the 17 names, lower case>
  slot = 0
  detached = false
  window = 0                     # detached window id; ignored when detached = false (R-084)

  [[detached_windows]]           # P3 (R-084)
  id = 0
  width = 180                    # logical px
  height = 90
  x = 0                          # x and y written and applied on KDE only, through the KWin bridge
  y = 0

  [[tray_graphs]]                # P3
  source = "cpu"
  column = "cpu"
  color = "accent"
  range_s = 60
  ```

  User themes: `$XDG_CONFIG_HOME/tasklens/themes.json`, TaskSlinger format `{"themes":[{"name":..., "SLOT":"#RRGGBB", ..., "accent":"#RRGGBB", "graph":{...}}]}`.

  `EngineSettings` (derived, never persisted): `interval_ms`, `network` unit, `show_network_*`, `history.*`, `processes.subtree_totals`, `processes.hide_system`.

  Where each key comes from (TS = TaskSlinger INI, MC = Mission Center gschema):

  | Key | Origin |
  |---|---|
  | general.start_tab | TS DefaultTab; MC window-selected-page (as `last_used`) |
  | general.language | TS UI.Language |
  | theme.active | TS UI.Theme, ThemeName; MC window-interface-style light/dark |
  | theme.follow_system | TS ThemeSyncWithWindows; MC window-interface-style=system |
  | theme.light / theme.dark | TS LightTheme/LightThemeName, DarkTheme/DarkThemeName |
  | appearance.ui_scale_pct | TS UIScale |
  | appearance.row_height_pct | TS RowHeightPct |
  | appearance.decorations | new (APP-001, D-006) |
  | appearance.rounded_corners | TS RoundedCorners |
  | appearance.heat_coloring | TS ProcessValueColorCoding |
  | appearance.heat_intensity_pct | TS ProcessValueColorCodingIntensityPct |
  | appearance.shadow_intensity_pct | TS DropShadowIntensityPct |
  | motion.animations | TS AnimationsEnabled |
  | motion.smooth_graphs | TS SmoothGraphs; MC performance-sliding-graphs |
  | motion.smooth_scroll | TS SmoothScrolling |
  | motion.smooth_scroll_amount | TS SmoothScrollAmount (scroll easing = 10 ms × value) |
  | motion.tab_switch_ms | TS MainTabAnimationMs |
  | units.network | TS ProcessNetworkRateUnit=0; MC network-use-bytes=false |
  | units.network_auto_scale | TS NetworkRateAutoScale |
  | processes.interval_ms | TS RefreshIntervalMs; MC app-update-interval-u64 (× 50 ms) |
  | processes.mode | TS TreeView and TaskManagerGrouping (R-010) |
  | processes.tree_guide_lines | TS TreeGuideLines |
  | processes.highlight_new_exited | TS HighlightNewExitedProcesses |
  | processes.filter_include_children | TS FilterShowChildren |
  | processes.hide_system | TS HideMicrosoftProcesses |
  | processes.subtree_totals | MC apps-page-merged-process-stats (inferred mapping of the old `processes.merge_child_stats`) |
  | tables.dim_zero_values | MC apps-page-hide-is-zero (0.3 opacity) |
  | tables.column_separators | MC apps-page-show-column-separators |
  | tables.column_resize | TS ColumnResizeBehavior |
  | processes.dump_type | process spec (dumps) |
  | properties.hide_unnamed_handles | TS HideUnnamedHandles |
  | properties.shared_objects_only | TS ModulesDllOnly |
  | properties.env_mask | process spec |
  | performance.graph_range_s | TS minigraph ranges; MC performance-page-data-points (R-016) |
  | performance.cpu_graph_mode | TS CpuPerCore; MC performance-page-cpu-graph |
  | performance.cpu_kernel | TS CpuKernel; MC performance-page-kernel-times |
  | performance.cpu_secondary_graph | MC performance-page-cpu-graph-bottom |
  | performance.gpu_optional_graphs | PERF-063 (R-054) |
  | performance.hidden_gpu_engines | TS HiddenGpuEngines |
  | performance.memory_composition / memory_swap_graph | MC memory-composition-visible / memory-swap-visible |
  | performance.show_disks / show_gpus / show_fans / show_batteries | MC performance-show-* |
  | performance.show_network_* | MC performance-show-network-* (R-053) |
  | performance.show_disconnected_fans | PERF spec |
  | performance.device_order | TS Columns.Performance; MC performance-sidebar-order |
  | performance.device_overrides | TS Performance.HiddenPages; MC performance-sidebar-device-overrides |
  | performance.curved_lines | MC performance-smooth-graphs (monotone cubic, no overshoot) |
  | performance.network_dynamic_scale | MC performance-page-network-dynamic-scaling |
  | fans.* | PERF spec |
  | graphs.minigraph_cpu / _memory / _disk / _network | TS ShowCpuGraph/ShowRamGraph/ShowDiskGraph/ShowNetworkGraph |
  | graphs.minigraph_gpu / _vram | TS screenshot (inferred) |
  | graphs.minigraph_gpu_temperature | TS ProcessGraphs.ShowGpuTemperatureGraph |
  | graphs.keep_detached_on_close | TS KeepDetachedOpenOnMainClose |
  | history.enabled | D-003 |
  | history.retention_h | TS HistoryStorageDurationHours |
  | history.persist | TS HistoryPersistToDisk |
  | history.max_disk_mb | ADR-011 |
  | window.close_with_esc / always_on_top | TS CloseWithEsc / Window.AlwaysOnTop |
  | tray.close_to_tray / minimize_to_tray / always_show | TS CloseToTray / MinimizeToTray / AlwaysShowTrayIcon |
  | startup.start_at_login / start_minimized | TS StartWithWindows / StartMinimized |
  | network.auto_fetch_public_ip / public_ip_notice_accepted | TS Connections.AutoFetchPublicIP / PublicIPFetchNoticeAccepted |
  | privilege.unlock_at_launch | TS AlwaysLaunchElevated (adapted) |
  | privilege.passwordless | TS UacLessElevation (adapted, APP-049) |
  | rendering.gpu | TS Graphics.GpuSelectionKind/GpuAdapterId/GpuAdapterName/GpuPreference |
  | services.unit_types / filter_states | new (SYS-002, SYS-004) |
  | lowmem.enabled | PROC-097 |
  | debug.show_fps | TS ShowFps |
  | state `[window]` | TS Window.Width/Height/Maximized; MC window-width/window-height/is-maximized |
  | state `[tabs] last_tab` | MC window-selected-page |
  | state `[processes] history_mode` | TS ProcessesHistoryMode |
  | state `[processes] group_expanded` | TS TaskManagerAppsExpanded/BackgroundExpanded/WindowsExpanded |
  | state `[services] group_expanded` | new (SYS-001) |
  | state `[performance] nav_width` / `selected_device` | TS Performance.NavWidth / MC performance-selected-page |
  | state `[sort.*]` | TS Sort.*; MC *-sorting-column-name / -sorting-order |
  | state `[columns.*]` | TS Columns.*, ColumnWidths.*, *VisibilityMask; MC *-column-order |
  | state `[[minigraphs]]` | TS ProcessGraphs.Column%d, DetachedProcessGraphs, ProcessGraphColors, ProcessGraphTimeRanges, ProcessSpecificMiniGraphs.* |
  | state `[[detached_windows]]` | R-084 (TS stores detached graph windows; tasklens adds KDE-only position) |
  | state `[processes] virustotal_notice_accepted` | R-095 |
  | state `[[tray_graphs]]` | TS TrayIconGraphs.* |

  Old key names in the source spec and their canonical names:
  - `monitoring.refresh_interval_ms` is `processes.interval_ms`.
  - `units.network_bits` is `units.network` (`bits | bytes`).
  - `history.retention_hours` is `history.retention_h`.
  - `window.system_titlebar` is `appearance.decorations` (R-021).
  - `startup.default_tab` is `general.start_tab` (R-024).
  - `tables.column_resize`, `tables.column_separators` and `tables.dim_zero_values` keep their names and apply to every table (R-075).
  - `processes.hide_system_processes` is `processes.hide_system` (R-031).
  - `processes.tree_view` and `processes.grouping` are `processes.mode` (R-010).
  - `processes.merge_child_stats` is `processes.subtree_totals` (inferred).
  - `graphs.time_range_s` is `performance.graph_range_s` with values 30, 60, 300, 600 (R-016).
  - `graphs.curved_lines` and `graphs.network_dynamic_scale` are `performance.curved_lines` and `performance.network_dynamic_scale`.
  - `performance.cpu_graph_mode` values `all_threads_overlay`, `all_threads_stacked` are `all_cores_overlaid`, `all_cores_stacked`.
  - `tray.graphs` is `state.toml` `[[tray_graphs]]`.
  - `state.last_tab` is `state.toml` `[tabs] last_tab`.

  Keys not carried over:
  - Dropped by R-071: `tables.remember_sort`, `tables.remember_columns` (always persisted), `processes.column_graphs` (fixed six), `processes.cpu_one_core_100` and ADR `processes.cpu_mode` (R-044), `processes.filter_cmdline` (R-043), `units.network_base2`, `units.memory_*`, `units.disk_*` (R-015), `performance.gpu_graph_middle` and `performance.gpu_graph_bottom` (R-054), `state.first_run_done` (R-028), `window.system_titlebar` (R-021), `copy_rows_tsv_shortcut`, `processes.tree_view` and `processes.grouping` (R-010), `debug.show_health_window`.
  - TS Window.X/Y: Wayland cannot restore position (APP-010).
  - TS Crash.DumpType: systemd-coredump handles native crashes of tasklens itself (APP-055). (`processes.dump_type` is the process-dump setting, not this one.)
  - TS Updates.CheckForUpdatesOnStartup, Install.*, License.Token, WingetEarlyBirdNoticeShown: dropped (APP-025, APP-050).
  - TS TripleBuffering: dropped (APP-026).
  - MC app-update-interval: already deprecated in Mission Center.
  - MC performance-show-cpu / performance-show-memory: never read by Mission Center; CPU and memory stay hideable per device.
  - MC performance-sidebar-hidden-graphs: deprecated in Mission Center.
  - TaskSlinger's ProcessColumnVisibilityMask is not trusted as a visibility mask (cross-check B.2). tasklens stores explicit per-column visibility.
- Linux data/mechanism: APP-033
- Verdict: ADAPT
- Phase: P1 (store and the keys of P1 features), each other key with its feature
- Privilege: none

### APP-033 Settings persistence
- Was: APP-033
- Source: TS UserSettings.ini | MC GSettings | review "Settings persistence" | ADR-011, R-018
- Behaviour:
  - Files (R-018, table c; directories 0700, files 0600):
    - `$XDG_CONFIG_HOME/tasklens/settings.toml`: user choices.
    - `$XDG_STATE_HOME/tasklens/state.toml`: window size and maximized, sort per table, columns per table, group expand state, last tab, Performance nav width and selected device, `history_mode`, minigraph layout, tray graphs. There is no `window.toml`.
    - `$XDG_CONFIG_HOME/tasklens/themes.json`: user themes in TaskSlinger's `{"themes":[…]}` format. Built-ins ship as `assets/themes/builtin.json`.
  - Both TOML files start with `schema_version = 1`. Both structs are `#[serde(default)]`.
  - Load (ADR-011): a missing file means defaults, and nothing is written until the user changes something. A `schema_version` missing is treated as 1. A lower version runs `migrate_vN_to_vN1` functions in order. A higher version loads what is understood, sets read-only, never saves, and shows the toast "Settings were written by a newer tasklens; changes will not be saved".
  - Out-of-range values clamp to the range and are logged.
  - A file that fails to parse is renamed `<name>.corrupt-<unix seconds>`, defaults load, and the toast reads "Settings file was unreadable and has been reset. The old file was kept as {name}".
  - Unknown keys are ignored and not preserved. Unknown column ids are dropped with a log line.
  - Columns are stored by string id, never by display index.
  - Save: debounced 500 ms after the last change, plus on exit. Atomic write through `tl-app/src/atomic.rs::write_atomic`: write `<name>.tmp-<pid>` in the same directory, fsync the file, rename over the target, fsync the directory.
  - No inotify on the settings files; external edits apply at the next start.
- Linux data/mechanism: `toml` + `serde`; paths from `tl-app/src/paths.rs` (an env var that is unset, empty or not absolute falls back to the default; no `directories` crate). tl-ui holds settings in a GPUI Global and calls a persist callback from tl-app; engine-relevant parts go to the engine as `EngineSettings` (derived, never persisted).
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-034 Import TaskSlinger settings and themes
- Was: APP-034
- Source: TS UserSettings.ini, themes.json (owner runs TaskSlinger under Wine)
- Behaviour: Settings window > Advanced > "Import from TaskSlinger…" opens a portal file chooser for `UserSettings.ini` or `themes.json`. It maps the keys with the origin table in APP-032 and imports custom themes into `themes.json`. Toast "Imported N settings and M themes".
- Linux data/mechanism: INI parse, then the APP-032 origin table; theme JSON is TaskSlinger's own format
- Verdict: ADAPT
- Phase: P4
- Privilege: none

## Themes and appearance

### APP-035 Built-in themes and colour slots
- Was: APP-035
- Source: TS 6 (12 themes, 22 slots), PLAN 9 palette | ADR-009, R-068
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

  - Each theme also carries an accent and the graph palette (ADR-009 `GraphPalette`: cpu_fill, cpu_line, kernel_line, mem_fill, mem_line, gpu_fill, gpu_line, disk_fill, disk_line, net_fill, net_line, grid). These sit outside the 22 editable slots.
  - Custom tasklens views read `TlTheme` directly. Kit widgets read the gpui-kit Theme, filled by `theme/map.rs::apply` with ADR-009's mapping (for example LIST_BG to `table`, ROW_ALT to `table_even`, ROW_SELECTED to `table_active`, TITLEBAR to `title_bar`, DIVIDER to `border` and `table_row_border`). Token names are checked against gpui-component 0.7.0 (U-8).
  - Theme changes animate as a 200 ms colour crossfade, instant when `motion.animations` is off or with reduced motion (R-017).
- Linux data/mechanism:
  - Default Dark values are in PLAN 9.
  - The other 11 come from the running TaskSlinger 0.9.2 under Wine: for each built-in, run "Create copy", then read the resulting `themes.json`, which gives exact `#RRGGBB` per slot (U-18).
  - Accent and graph colours come from pixel samples of each theme's screenshot (selected tab line, graphs).
  - The palettes ship as `assets/themes/builtin.json` in TaskSlinger's format plus `accent` and `graph.*` keys (R-018).
- Verdict: BUILD
- Phase: P1 (Default Dark, Default Light), P2 (the other 10) (R-068)
- Privilege: none

### APP-036 Theme menu and follow system light/dark
- Was: APP-036
- Source: TS 1.3 Theme submenu L28480-28482, L28542, L28885-28888 | MC theme selector (system, light, dark) | cross-check A.40 | R-069
- Behaviour:
  - View > Theme > lists the 12 built-ins and the custom themes. Each entry is painted in its own palette, the active one has a check (`theme.active`), and each has a copy icon.
  - Per-theme items: "Create copy", "Edit colors", "Share theme", "Delete theme".
  - Then "Import theme", "Sync theme with system" (`theme.follow_system`), "Preferred Light Theme >" (`theme.light`), "Preferred Dark Theme >" (`theme.dark`).
  - With sync on, the active theme follows the portal colour scheme live: 1 (prefer dark) uses `theme.dark`; 2 (prefer light) uses `theme.light`; 0 (no preference) uses `theme.dark`.
  - Toasts: "Theme copied", "Theme deleted", "Built-in themes cannot be deleted", "Copy a theme before editing it". Delete asks first in a modal window: "Delete this custom theme?".
- Linux data/mechanism: `org.freedesktop.portal.Settings.ReadOne("org.freedesktop.appearance","color-scheme")`, plus the `SettingChanged` signal, through zbus on the session bus (xdg-desktop-portal-kde answers). Custom themes are saved to `themes.json` with `write_atomic`.
- Verdict: ADAPT (Windows AppsUseLightTheme becomes the XDG portal)
- Phase: P2
- Privilege: none

### APP-037 Theme editor
- Was: APP-037
- Source: TS 6 "Theme Editor" L28900-28903, L30008-30025
- Behaviour:
  - A separate pinnable, non-modal window titled "tasklens Theme Editor" (APP-045 naming, R-083).
  - Left: the 22-slot list, plus Accent.
  - Centre: hue strip and colour plane, "R %d G %d B %d" and "H %03d S %03d V %03d" readouts, and a hex input with the hint "Enter a 6-digit hex color".
  - A title input for the theme name.
  - A live preview pane: mini table, tab bar and graph.
  - Buttons: Done, Random (toast "Random theme generated."), Share, Import, Cancel.
  - Edits apply live to the app; Cancel reverts. Built-in themes cannot be edited ("Copy a theme before editing it").
- Linux data/mechanism: custom GPUI elements for the colour plane and hue strip; saved to `themes.json`
- Verdict: BUILD
- Phase: P3
- Privilege: none

### APP-038 Theme share and import codes
- Was: APP-038
- Source: TS 6 L29405, L29532-29533, L29667-29675, toasts L28620-28636
- Behaviour:
  - "Share theme" opens the window "tasklens Share Theme" with the code and a Copy button. Toast "Theme code copied".
  - "Import theme" reads the clipboard and accepts:
    - tasklens codes `TL1.` + base64url(JSON `{"v":1,"name":…,"accent":"#…","SLOT":"#RRGGBB",…}`);
    - TaskSlinger codes `TS1.…`;
    - raw TaskSlinger `themes.json` text.
  - Toasts: "Theme imported", "Clipboard does not contain a theme code".
- Linux data/mechanism: clipboard read through GPUI; serde_json
- Verdict: ADAPT
- Phase: P3
- Privilege: none
- Verify U-17: the TaskSlinger `TS1.` encoding is inferred. The bead generates one code in TaskSlinger under Wine, decodes it and matches it. If it cannot be decoded, tasklens accepts only `TL1.` codes and raw JSON (R-079).

### APP-039 Graph accent colours
- Was: APP-039
- Source: TS 6 L28548-28565
- Behaviour: The Graph Color menu (minigraphs and tray graphs) offers Accent (theme default), then 17 named colours:

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

  These hex values are fallbacks. They are replaced by pixel samples of TaskSlinger's swatches when the theme extraction in APP-035 runs (U-18). The stored value in `state.toml` (`color` in `[[minigraphs]]` and `[[tray_graphs]]`) is `accent` or the lower-case name (R-083).
- Linux data/mechanism: constants in tl-ui
- Verdict: BUILD
- Phase: P3
- Privilege: none

### APP-040 Appearance sliders
- Was: APP-040
- Source: TS 1.3 Appearance L28500-28505, toast "UI scale changed to %d%%" L28643, slider formats L35541-35543 | R-020, R-059
- Behaviour: View > Appearance > holds, with labels "%d%%":
  - "UI Scale" slider: 75-200%, step 5 (`appearance.ui_scale_pct`). Toast "UI scale changed to %d%%". Ctrl+Plus, Ctrl+Minus and Ctrl+0 step by 5 or reset to 100.
  - "Row Height" slider: 80-150%, step 5 (`appearance.row_height_pct`).
  - "Rounded Corners" check (`appearance.rounded_corners`).
  - "Value Color Coding" check (`appearance.heat_coloring`).
  - "Value Color Coding Intensity" slider: 0-100% (`appearance.heat_intensity_pct`, default 50).
  - "Drop Shadow Intensity" slider: 0-100% (`appearance.shadow_intensity_pct`).

  Effects:
  - UI scale multiplies the theme font size and rem. The compositor scale (110% here) is applied once by GPUI and never doubled.
  - Row height = 28 × row_height_pct/100 × ui_scale_pct/100 logical px (R-020).
  - Rounded corners set radius 4 px (rows, menus and other small elements) and 6 px (cards), times ui_scale; off sets 0 (R-077, ADR-009 `radius_small`, `radius_card`).
  - Heat colouring blends VALUE_HEAT into the cell background by value / reference × intensity. The per-column references are R-059's (process spec).
  - Shadow intensity scales the alpha of menu, minigraph and toast shadows.
  - Dragging previews live. The save happens on release.
  - A change of scale bumps the style version and clears the cell text cache (ADR-009).
- Linux data/mechanism: gpui-component Slider (`SliderEvent::Release` saves); `Theme::update` font_size and radius; row-height and text caches invalidated when `rem_size` changes
- Verdict: BUILD
- Phase: P2 (Ctrl+Plus/Minus/0 with the global keys in P1)
- Privilege: none

## Windows, dialogs and system integration

### APP-041 System Information (Mission Center dialog)
- Was: APP-041
- Source: MC about_system_dialog (OS, Kernel, Desktop groups; per-row copy; Copy All; Ctrl+A)
- Behaviour:
  - Not a separate window. Its fields join the Information tab's "Operating System" section, which the Performance spec owns (PERF-095): Package manager, Package manager version, Kernel release, Kernel version; Desktop name, Desktop version, Windowing system, Virtual terminal; the distro logo beside the section title.
  - Missing values hide their rows, as in Mission Center.
  - Help > "System Information…" switches to the Information tab and scrolls to that section.
  - Mission Center's per-row copy is the Information row copy; its Copy All is the Information tab copy icon. Ctrl+A is Select all (table b); System Information has no shortcut.
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

### APP-042 Additional Setup window
- Was: APP-042
- Source: MC first_run_dialog (Enabling Additional Values, setup script, revert, view, copy command) | R-028, R-029
- Behaviour (R-028; the full design is the Performance spec's PERF-105, which owns the rows):
  - There is no first-run dialog.
  - One window titled "tasklens Additional Setup", opened from Help > "Additional setup…", from the Settings window's Privileges page, and from every "Needs setup" link.
  - Rows: "CPU power draw" (PERF-106), "Journal access", "Administrator helper", "Fan sensors" (PERF-108, P4), "Per-process network" (text only, PERF-107).
    - Journal access shows `sudo usermod -aG systemd-journal $USER` with a Copy button. tasklens never changes groups itself.
    - Administrator helper shows "Installed" or "Not installed".
  - Each row has status text, Enable or Revert, "View script" and "Copy command".
  - CPU power draw shows PERF-106's warning before enabling: "Lets every local user read CPU energy counters. This can leak information about other programs (CVE-2020-8694)." Status "Enabled (existing rule)" when `energy_uj` is already readable and the tasklens rule file is absent.
  - Success shows "Done. No restart needed" and re-probes the source at once. Failure shows the output with "Copy output" and "Retry".
  - Mission Center's `nethogs` capabilities are not needed: tasklens uses sock_diag.
- Linux data/mechanism: the packaged script `/usr/lib/tasklens/tasklens-setup` (never written to /tmp), run as `pkexec /usr/lib/tasklens/tasklens-setup <rapl-enable|rapl-revert|sensors-detect|polkit-rule on|off>` under polkit `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`). `rapl-enable` writes `/etc/udev/rules.d/99-tasklens-powercap.rules` with the one line `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`, then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add` (R-029). Mission Center's `/etc/udev/rules.d/99-powercap.rules` is detected and never touched.
- Verdict: ADAPT
- Phase: P3
- Privilege: pkexec under the setup action. Without it, CPU power shows "Needs setup" (R-050), never 0.

### APP-043 Language and live locale change
- Was: APP-043
- Source: TS 1.4 Language (10 locales), INI UI.Language, live locale change (cross-check A.40) | MC gettext (48 po files)
- Behaviour:
  - Every user-visible string goes through a message key from P1. The English catalogue ships in P1.
  - "Language: >" lists "System default" (follows `LC_ALL`/`LC_MESSAGES`/`LANG`) and the ten TaskSlinger locales plus nb-NO (`general.language`: `system | en-US | de-DE | es-ES | fr-FR | ja-JP | ko-KR | pt-BR | ru-RU | uk-UA | zh-CN | nb-NO`).
  - Changing the language in tasklens reapplies all text live, without a restart. A change of the system locale applies at the next start, because the KDE portal does not provide a locale setting.
  - Numbers do not follow the locale in v1: every language uses the table (d) forms, with decimal point "." and thin-space grouping. Dates and times use the table (d) patterns in local time. A language change changes text only (R-081).
  - Missing keys fall back to English.
- Linux data/mechanism: Fluent (`fluent-bundle` + `unic-langid`), `.ftl` files embedded with `include_str!`; locale from the environment. Translations are drafted by Claude agents and reviewed per locale. No Mission Center po text is copied, because of GPL-3.
- Verdict: ADAPT
- Phase: P1 (key system and English), P4 (other locales)
- Privilege: none

### APP-044 Toasts
- Was: APP-044
- Source: TS 8 (top centre) | gpui-component notifications (5 s autohide, placement per toast)
- Behaviour:
  - Toasts appear at the top centre of the window, 8 px under the tab bar, at most 4 stacked, auto-hidden after 5 s. Hover pauses the timer. Errors stay 8 s.
  - Types: Success (green icon), Info, Warning, Error.
  - An in-progress toast is replaced in place by its result through the same id (service jobs, close connection).
  - All toast texts come from the owning area's spec (TaskSlinger strings). General ones: "Copied to clipboard", "Copied: <value>", "Failed to copy to clipboard", "No process selected", "Select a process row first." (and the service, startup and connection variants), "Opening file location", "Failed to open file location", "Could not open properties.", "Authentication cancelled".
  - Toast motion follows `motion.animations` and reduced motion (R-017).
  - Shown only while the main window is visible. While hidden to the tray, Error results go to a desktop notification instead (`NotificationDelivery::System`).
  - A collection error never becomes a toast unless the user started an action (ADR-015).
- Linux data/mechanism: `window.push_notification(Notification::success(..).placement(Anchor::TopCenter).id1::<T>(key))`; system delivery through `org.freedesktop.Notifications`
- Verdict: BUILD
- Phase: P1
- Privilege: none

### APP-045 Dialogs as separate pinnable windows
- Was: APP-045
- Source: TS 8 [CL 0.8.25], cross-check A.38 (`TaskSlingerDetachedModal`, `modal.pin`) | R-061, R-073
- Behaviour:
  - Every dialog and property panel is its own top-level window with a title bar pin button (APP-009).
  - Confirmations are modal to their parent: Esc and Cancel close, Enter triggers the default button. Property, editor and tool windows are not modal.
  - Window titles:

    | Window | Title |
    |---|---|
    | Run | "tasklens Run" (R-061) |
    | Handle Search | "tasklens Handle Search" (R-061) |
    | Public IP notice | "tasklens Public IP Notice" |
    | Delete startup entry | "tasklens Delete Startup Entry" |
    | Theme share code | "tasklens Share Theme" |
    | About | "tasklens About" (R-078) |
    | Additional setup | "tasklens Additional Setup" (R-028) |
    | Crash report | "tasklens Crash Report" (R-049) |
    | Settings | "tasklens Settings" |
    | Keyboard shortcuts | "tasklens Keyboard Shortcuts" (R-083) |
    | Theme editor | "tasklens Theme Editor" (R-083) |
    | Service properties | "<id> Properties" (SYS-011) |
    | Startup properties | "Startup properties: <Name>" (SYS-026) |
    | Connection properties | "Connection properties: <Proto> <State>" (SYS-034) |
    | Process properties | per the process spec |

  - Other confirmations (Reset settings, Delete theme, Close listening socket, Skip password prompts, process action confirmations) are modal windows titled "tasklens" (R-083).
  - Closing the main window closes all child windows, except detached graphs when `graphs.keep_detached_on_close` is on.
- Linux data/mechanism:
  - `cx.open_window` with `WindowKind::Dialog` for modal confirmations (xdg_dialog_v1 set_modal) and `WindowKind::Floating` (xdg parent) for tools.
  - Each window gets a gpui-kit Root, so it has its own toasts and menus.
  - In-window gpui-component Dialog is used only for sub-prompts inside those windows.
- Verdict: BUILD
- Phase: P1 (frame), others with their features
- Privilege: none

### APP-046 Rendering GPU choice
- Was: APP-046
- Source: TS GPU preference L28527-28529, L28540-28541, Graphics.*, dual-path startup (cross-check A.43) | R-070
- Behaviour: Settings > Monitoring > "Rendering GPU >" lists "Auto", one entry per Vulkan adapter (name), and "Software renderer" (`rendering.gpu = "auto" | "software" | "pci:<bus id>"`). Changing it shows the toast "Restart tasklens to use the new GPU".
- Linux data/mechanism:
  - At startup, before GPUI initialises, set `ZED_DEVICE_ID=0x<pci device id>` for an adapter. A `ZED_DEVICE_ID` already set in the environment by the user wins (R-070).
  - For software rendering, set `WGPU_ADAPTER_NAME=llvmpipe` together with `VK_ICD_FILENAMES` set to lvp_icd.
  - Adapters are listed with wgpu `Instance::enumerate_adapters` in a short-lived probe at menu open.
  - TaskSlinger's WARP first frame has no wgpu equivalent and is not copied.
  - Before P4 tasklens never sets `ZED_DEVICE_ID` (ADR-010).
- Verdict: ADAPT
- Phase: P4
- Privilege: none

### APP-047 Main tray icon and close/minimize to tray
- Was: APP-047
- Source: TS INI Window.CloseToTray/MinimizeToTray/AlwaysShowTrayIcon, tray menu L28588, L29307, tooltip L28584-28586, L30144-30157 | review (closing the last window exits in v1; tray is a later explicit mode) | R-027, ADR-010
- Behaviour:
  - When any tray setting is on, a main StatusNotifierItem appears. Tray graph icons are owned by the Performance spec (PERF-088 to PERF-092).
  - Tooltip: "CPU: 12.3% RAM: 45.6% GPU: 3.0% Disk: 0.4% Network: 2.3 Mbps", percent with one decimal (table d), network in the `units.network` unit. Updated at 1 Hz while the tray is shown.
  - Left click toggles the main window.
  - Menu (R-027): "Show Main Window", "Tray icon graphs >", separator, "Exit tasklens".
  - Close to tray (`tray.close_to_tray`): close (button, Ctrl+W, Esc-close) hides the window instead of exiting.
  - Minimize to tray (`tray.minimize_to_tray`): minimize hides it.
  - Always show tray icon (`tray.always_show`): keeps the icon while the window is visible.
  - The main tray icon is hidden while a tray graph icon is shown [CL 0.9.2] (PERF-091).
  - With every tray setting off, closing the main window exits.
  - While hidden: collection stops except tray and graph demand.
- Linux data/mechanism (R-027, ADR-010):
  - Crate `tl-tray` with `ksni =0.3.6`, features `async-io` and `blocking`, on its own thread through `ksni::blocking::TrayMethods::spawn`. `cargo xtask check-deps` proves no Tokio (U-31). If that check fails, the fallback is a hand-written SNI and dbusmenu server inside `tl-tray` on its own zbus connection, never inside tl-engine.
  - Items `org.kde.StatusNotifierItem-<pid>-<n>`, objects `/StatusNotifierItem` and `/MenuBar` (table c). Icons ARGB32 at 22×22 and 44×44 px. Updates at most 1 Hz and only when the pixels changed.
  - Re-register on `StatusNotifierWatcher` owner change (the Linux form of TaskbarCreated).
  - Close handling: the app uses `QuitMode::Explicit`. `on_window_should_close` on the main window removes the window and keeps the app and engine running when the tray is enabled and close-to-tray is on; otherwise it calls `cx.quit()`. After `Application::run` returns, `main` shuts the engine down within 1500 ms, flushes logs and exits.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

### APP-048 Start at login
- Was: APP-048
- Source: TS Startup.StartWithWindows/StartMinimized (`--start-minimized`)
- Behaviour: "Start at login" (`startup.start_at_login`) writes the autostart entry; turning it off deletes it. "Start minimized" (`startup.start_minimized`) adds `--start-minimized`. That flag starts hidden to the tray if the tray is enabled, otherwise minimized. The entry then shows in the Startup Apps tab like any other.
- Linux data/mechanism: `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop` with `Exec=tasklens [--start-minimized]` and `X-KDE-autostart-phase=2`, written with `write_atomic`
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-049 Privilege settings (unlock at launch, passwordless)
- Was: APP-049
- Source: TS AlwaysLaunchElevated, UacLessElevation (IFEO, experimental) L30388-30400 | R-002
- Behaviour:
  - "Unlock privileged actions at launch" (`privilege.unlock_at_launch`) runs APP-003 at startup (one prompt).
  - "Skip password prompts (experimental)" (`privilege.passwordless`) installs a polkit rule so tasklens actions need no password for the active local session of wheel members. Turning it off removes the rule.
  - A modal confirm window comes first: "tasklens actions that change other users' processes, system services and network connections will run without asking for a password. Continue?", buttons "Skip prompts" / "Cancel".
- Linux data/mechanism: `pkexec /usr/lib/tasklens/tasklens-setup polkit-rule on|off` under `io.github.oysteinkrog.tasklens.setup` writes or removes `/etc/polkit-1/rules.d/49-tasklens.rules`. The rule returns `polkit.Result.YES` for `action.id.indexOf("io.github.oysteinkrog.tasklens.") == 0 && subject.local && subject.active && subject.isInGroup("wheel")`. The prefix covers the helper, setup and netmon actions; the systemd and policykit.exec actions are not covered.
- Verdict: ADAPT
- Phase: P3 (unlock at launch), P4 (passwordless)
- Privilege: pkexec once to install

### APP-050 Updater, installer and related items
- Was: APP-050
- Source: TS 8 Updater, TaskSlinger Setup, cross-check A.33-A.36, A.44, C.5 internal flags; INI Updates, Install
- Behaviour: none. Each item below is DROP:
  - In-app updater (check, download, signature check, "Click to update", changelog), the CheckForUpdatesOnStartup setting, and the About "Updates" block: pacman and the localbuilds recipe update tasklens.
  - Installer (install for current user, location, PATH, Start Menu, desktop shortcut, Modify, Use standalone, Uninstall, folder picker, `.ts-migrate`, installed-notification) and the flags `--install-source`, `--internal-install`, `--internal-uninstall-helper`, `--internal-replace-in-place`, `--internal-install-handoff-pid`, `--internal-uninstall-wait-pid`, `--internal-show-installed-notification`: the pacman package installs the binary, `.desktop` file, icon and polkit policy.
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
- Was: APP-051
- Source: TS "Set as default Task Manager" (IFEO), Installer option
- Behaviour: Settings window > Startup and window > "Open with Ctrl+Esc". When on, KDE's Ctrl+Esc opens tasklens instead of Plasma System Monitor. When off, the KDE default comes back. The check state is read from kglobalaccel; there is no settings key.
- Linux data/mechanism: kglobalaccel D-Bus `org.kde.kglobalaccel /kglobalaccel`. Register `io.github.oysteinkrog.Tasklens.desktop` `_launch` = Ctrl+Esc, and set `org.kde.plasma-systemmonitor.desktop` `_launch` to none, using `setForeignShortcutKeys`.
- Verdict: ADAPT
- Phase: P4
- Privilege: none
- Verify U-16: whether `setForeignShortcutKeys` sticks for a `.desktop` launch component that is not running. The bead tests it on Plasma 6. If it does not stick, tasklens writes `~/.config/kglobalshortcutsrc` `[services][io.github.oysteinkrog.Tasklens.desktop] _launch=Ctrl+Esc` and asks the user to log out and back in (R-080).

### APP-052 Command-line arguments
- Was: APP-052
- Source: TS 7 and cross-check C.5 | MC `--app-id`/`-a` | PLAN `--bench` | table (c)
- Behaviour: Arguments (table c):
  - `--start-minimized` (APP-048)
  - `--low-memory`: low-memory rescue window (process spec, PROC-098); TaskSlinger's `/low-memory` and `--internal-low-memory-recovery`
  - `--watch-memory`: the low-memory watcher mode run by `tasklens-lowmem.service`; it does not start GPUI (PROC-097)
  - `--tab <processes|performance|services|startup|connections|information>`: TaskSlinger's page argument, whose syntax is unknown, made explicit
  - `--select-pid <pid>`: opens Processes and selects that pid
  - `--debug`: Debug menu (APP-026)
  - `--bench <scene> [--duration --warmup --json]`: ADR-014 scenes `perf-cpu`, `table-2000`, `bridge`, `static`, plus `history-scrub` in P3. This replaces TaskSlinger's `--history-scrub-bench` and `--history-scrub-bench-out`.
  - `--force-full-memory-mode`: TaskSlinger's `--internal-force-full-memory-mode`, used to test the low-memory path
  - `--app-id <reverse-dns id>`: separate instance and settings dir, for development builds (Mission Center)
  - `--version`, `--help`

  Unknown arguments print usage and exit 2. Forwarding to a running instance follows APP-011.
  Environment variables read (table c): `TASKLENS_LOG`, `TASKLENS_GPU_PROBE`, `TASKLENS_DEBUG`, `ZED_DEVICE_ID`, `__NV_FORCE_ENABLE_X11_EGL_PLATFORM`, `TL_UPDATE_GOLDEN` (tests).
- Linux data/mechanism: `clap` derive
- Verdict: ADAPT
- Phase: P1 (`--tab`, `--select-pid`, `--version`, `--help`, `--bench`), P2 (`--start-minimized`), P3 (`--low-memory`, `--watch-memory`, `--force-full-memory-mode`), P4 (`--app-id`, `--debug`)
- Privilege: none

### APP-053 About window
- Was: APP-053
- Source: TS About L29358-29368, L35581-35590, cross-check A.33 | MC adw AboutDialog
- Behaviour:
  - A separate pinnable window titled "tasklens About" (APP-045, R-078), opened by Help > "About tasklens…".
  - Shows icon, "tasklens", "Version: <semver> (<git short hash>, <build date>)".
  - "Copy version info" copies version, GPUI revision, kernel, NVIDIA driver, compositor, session type and scale for bug reports.
  - "Third-party licenses" opens a scrollable list generated at build time.
  - Links: project repository, and "Open log folder".
  - No TaskSlinger or Mission Center branding (D-005).
  - TaskSlinger's author line, Discord, Twitter, bug email, support text and update block are not copied: they belong to TaskSlinger's product.
- Linux data/mechanism: `cargo-about` output embedded with `include_str!`; `env!("CARGO_PKG_VERSION")` plus a build.rs git hash
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### APP-054 Logging
- Was: APP-054
- Source: TS log levels and format (cross-check A.42) L29425-29437, logs\app.log | review risk 7 | ADR-015, R-048
- Behaviour (ADR-015 as written):
  - `tracing` everywhere; `tracing-subscriber` with `EnvFilter` from `TASKLENS_LOG`, default `info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,zbus=warn`. `RUST_LOG` is not read.
  - Outputs: stderr when stderr is a terminal; always a file in `$XDG_STATE_HOME/tasklens/logs/`, rotated daily as `tasklens.<date>.log`, keeping 7 files, written through `tracing_appender::non_blocking` (its guard is held in `main` and dropped last).
  - Targets: `tasklens::engine`, `tasklens::source::<name>`, `tasklens::action`, `tasklens::ui`, `tasklens::frame`, `tasklens::probe`. Timing lines (first-frame time, collector timings) use `info` on target `tasklens::perf`. There is no PERF level, no custom categories and no repeat collapsing (R-048).
  - Never logged at any level: command lines, environment, working directories, open file paths, executable paths of other users' processes, window titles, user names other than the current user. Processes are logged as pid plus comm, and comm only at debug and below. Action logs record kind, target pid and result.
  - No logging inside per-frame or per-process loops above trace level.
- Linux data/mechanism: `tracing`, `tracing-subscriber`, `tracing-appender` `RollingFileAppender` with `Rotation::DAILY`, prefix `tasklens`, suffix `log`, `max_log_files(7)`
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-055 Crash handling and fatal errors
- Was: APP-055
- Source: TS 8 fatal errors L28904-28920, crash.log, dumps\, Crash.DumpType | review (NVML isolation, device loss) | ADR-015, R-049
- Behaviour (R-049):
  - A panic hook, installed first in `main`, writes `$XDG_STATE_HOME/tasklens/crash/crash-<unix seconds>.txt`. Content: tasklens version and git sha, thread name, panic message, location, `Backtrace::force_capture()`, and the last 200 log lines. No process metadata, no settings contents, no command lines or environments. The newest 20 reports are kept. Then the default hook runs.
  - A panic in an engine worker is handled by its supervisor with backoff and does not crash the app. In an action task, the action reports "internal error".
  - On a crash of the app: desktop notification "tasklens stopped after an unrecoverable error."
  - Next start: a window "tasklens Crash Report" with the text "tasklens closed unexpectedly last time." and buttons "Open log folder", "Copy report", "Close". This replaces the ADR-015 toast.
  - Native crashes (SIGSEGV) are left to systemd-coredump. The report window mentions `coredumpctl info tasklens`. TaskSlinger's Mini/Full dump setting is not needed.
  - GPU init failure ("tasklens could not start the renderer") goes to stderr and a desktop notification, then exit 1.
  - GPU device loss: if GPUI cannot recover, the notification "tasklens lost the graphics device and must restart" appears and tasklens relaunches itself once.
- Linux data/mechanism: `std::panic::set_hook`; `org.freedesktop.Notifications.Notify` through zbus (works without a GPUI window); `panic = "unwind"` (ADR-002)
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### APP-056 Public IP fetch notice
- Was: APP-056
- Source: TS 8 "Confirm Public IP Fetch" L29346-29350, Connections.AutoFetchPublicIP | R-058
- Behaviour:
  - The first Fetch (Network page or Information tab) shows the modal window "tasklens Public IP Notice" with the text:
    - "tasklens will fetch your public IP address from an online service."
    - "Service: https://api.ipify.org"
    - "Continue only if you want this computer to contact that service."
  - A checkbox "Do not ask again for public IP fetches" (`network.public_ip_notice_accepted`), and buttons Continue / Cancel.
  - "Auto-fetch Public IP on launch" (`network.auto_fetch_public_ip`, Settings > Window & startup, R-058) fetches once at startup when the notice has been accepted.
- Linux data/mechanism: HTTPS GET with `ureq` 3 (rustls, blocking, no Tokio, U-30) on the blocking pool, 5 s timeout
- Verdict: BUILD
- Phase: P2
- Privilege: none

### APP-057 NVIDIA G-SYNC profile
- Was: APP-057
- Source: TS Settings "Create NVIDIA G-SYNC profile" L28530-28535 [CL 0.8.28]
- Behaviour: none
- Linux data/mechanism: none
- Verdict: DROP (KWin applies adaptive sync only to fullscreen windows by default, so a windowed tasklens never triggers VRR flicker; no NVIDIA driver profile is needed)
- Phase: P4 (recorded)
- Privilege: none

### APP-058 Accessibility and reduced motion
- Was: APP-058
- Source: TS TextScaleFactor [CL 0.8.29] | review "Accessibility/input" | R-017
- Behaviour:
  - Full keyboard use per APP-031, with visible focus rings.
  - Icon-only buttons have accessible names.
  - Charts show their latest value and unit as text next to the chart. GPUI has no screen reader support on Linux (U-5).
  - Reduced motion: system reduce-motion turns off tab, theme and toast animations, smooth scrolling and graph sliding, whatever `[motion]` says (R-017).
  - The UI scale default follows the system font: when `appearance.ui_scale_pct` has never been set, the default is the KDE general font point size / 10 × 100, rounded to step 5 and clamped to 75..=200.
- Linux data/mechanism: GPUI accesskit adapter if present in gpui-linux (unverified, U-5); `App::reduce_motion()`; `kdeglobals [General] font`
- Verdict: ADAPT
- Phase: P2 (scale-from-font default P4)
- Privilege: none

### APP-059 Loading placeholders
- Was: APP-059
- Source: MC shimmer placeholders | TS "Loading service data..." | widgets rule 17 | R-060
- Behaviour:
  - The Processes tab shows rows from the first sweep with `…` in rate cells and no skeleton.
  - Services, Startup Apps, Connections and Information show skeleton rows or blocks until their first data, once per session. Services also shows the toast "Loading service data..." (SYS-016).
  - Skeletons are never shown when switching between tabs whose data is already loaded.
  - The first frame is drawn without waiting for collectors.
- Linux data/mechanism: gpui-component Skeleton, which stops requesting frames once replaced; never used in table cells or charts (ADR-009)
- Verdict: BUILD
- Phase: P1 (frame rule), P2 (skeletons with the P2 tabs)
- Privilege: none

# Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| SYS-001 | Services tab, two systemd managers | ADAPT | P2 |
| SYS-002 | Unit types shown | ADAPT | P2 |
| SYS-003 | Services columns | ADAPT | P2 (resource columns P2) |
| SYS-004 | State filters and filter strip | BUILD | P2 |
| SYS-005 | Live updates and job tracking | ADAPT | P2 |
| SYS-006 | Start, Stop, Restart, Reload, Reset failed | BUILD | P2 |
| SYS-007 | Startup type changes (enable, disable, mask) | ADAPT | P2 |
| SYS-008 | Services context menu | ADAPT | P2 |
| SYS-009 | Copy formats (services) | BUILD | P2 |
| SYS-010 | Jump between services and processes | BUILD | P2 |
| SYS-011 | Service Properties window: frame and General tab | ADAPT | P2 |
| SYS-012 | Service Properties: Log On tab | ADAPT | P2 |
| SYS-013 | Service Properties: Recovery tab | ADAPT | P2 view, P3 edit |
| SYS-014 | Service Properties: Dependencies tab | ADAPT | P2 |
| SYS-015 | Service logs (journal) | ADAPT | P2 |
| SYS-016 | Services status bar and loading state | BUILD | P2 |
| SYS-017 | Services sort and column persistence | BUILD | P2 |
| SYS-018 | OpenRC backend | BUILD | P4 |
| SYS-019 | Startup sources and precedence | ADAPT | P2 |
| SYS-020 | Startup columns and values | ADAPT | P2 |
| SYS-021 | Startup impact | ADAPT | P2 |
| SYS-022 | Enable and disable | ADAPT | P2 |
| SYS-023 | Run now | ADAPT | P2 |
| SYS-024 | Delete entry | ADAPT | P2 |
| SYS-025 | Startup context menu, copy and open | BUILD | P2 |
| SYS-026 | Startup Properties window | ADAPT | P2 |
| SYS-027 | Startup status bar, sort and persistence | BUILD | P2 |
| SYS-028 | Connection inventory | ADAPT | P2 |
| SYS-029 | Connection columns, address formats, states | ADAPT | P2 |
| SYS-030 | Owning process attribution | ADAPT | P2 (own processes), P3 (helper for others) |
| SYS-031 | Close TCP connection | ADAPT | P3 (needs tl-helper) |
| SYS-032 | Connections context menu and copy formats | BUILD | P2 |
| SYS-033 | Jump to owning process | BUILD | P2 |
| SYS-034 | Connection Properties window | ADAPT | P2 |
| SYS-035 | Remote host name column | ADAPT | P4 |
| SYS-036 | Connections status bar, filter, sort, refresh | BUILD | P2 |
| SYS-037 | Other network namespaces | ADAPT | P4 |
| APP-001 | Window decorations (client-drawn title bar by default) | BUILD | P1 |
| APP-002 | Title bar layout and toolbar | ADAPT | P1 |
| APP-003 | Shield: unlock privileged actions | ADAPT | P3 |
| APP-004 | Hide system processes button | ADAPT | P1 |
| APP-005 | View mode button | BUILD | P1 |
| APP-006 | Window picker "+" | ADAPT | P3 |
| APP-007 | Update badge | DROP | P4 (recorded as dropped) |
| APP-008 | Caption buttons | BUILD | P1 (pin P2) |
| APP-009 | Always on top and the pin | ADAPT | P2 |
| APP-010 | Size, minimum size and geometry restore | ADAPT | P1 |
| APP-011 | Single instance | ADAPT | P1 |
| APP-012 | File menu | ADAPT | P1 (Exit), later items in their own phase |
| APP-013 | Run Installer item | DROP | P4 (recorded) |
| APP-014 | Run dialog (window frame and privilege path) | ADAPT | P2 |
| APP-015 | Search File Handles window (frame) | ADAPT | P3 |
| APP-016 | Launch other system monitor | ADAPT | P4 |
| APP-017 | View menu structure | BUILD | P1 (Columns, Process View, Window, Graphs), P2 (Appearance, Theme) |
| APP-018 | View > Columns (all tables) | BUILD | P1 |
| APP-019 | View > Process View | BUILD | P1 |
| APP-020 | View > Window | ADAPT | P2 (Always On Top), P3 (Performance mini window) |
| APP-021 | View > Graphs | BUILD | P1 (graph checks and Reset, with the system minigraph cards, PERF-080 and table a), P3 (Keep detached graphs open) |
| APP-022 | Settings menu | ADAPT | P1 (Refresh, units, Default tab, Close with Esc, motion items), others in their feature's phase |
| APP-023 | Settings window ("All settings…") | ADAPT | P2 |
| APP-024 | Help menu | ADAPT | P2 (Additional setup… P3, Documentation P4) |
| APP-025 | Purchase license, Enter license, early-bird notice | DROP | P4 (recorded) |
| APP-026 | Debug menu | ADAPT | P4 (Show FPS in P1 as a developer-only flag) |
| APP-027 | Tab bar and tab animation | BUILD | P1 (animation P2) |
| APP-028 | Filter box | BUILD | P1 |
| APP-029 | Status bar | ADAPT | P1 |
| APP-030 | Keyboard Shortcuts window | BUILD | P2 |
| APP-031 | Keyboard shortcuts | ADAPT | P1 (Global, Table and ProcessTable keys), P2 (ServiceTable, Performance extras, RunDialog), P3 (Ctrl+Shift+M, History scrubber, Picker, Shift at launch) |
| APP-032 | Settings registry (every setting) | ADAPT | P1 (store and the keys of P1 features), each other key with its feature |
| APP-033 | Settings persistence | ADAPT | P1 |
| APP-034 | Import TaskSlinger settings and themes | ADAPT | P4 |
| APP-035 | Built-in themes and colour slots | BUILD | P1 (Default Dark, Default Light), P2 (the other 10) (R-068) |
| APP-036 | Theme menu and follow system light/dark | ADAPT | P2 |
| APP-037 | Theme editor | BUILD | P3 |
| APP-038 | Theme share and import codes | ADAPT | P3 |
| APP-039 | Graph accent colours | BUILD | P3 |
| APP-040 | Appearance sliders | BUILD | P2 (Ctrl+Plus/Minus/0 with the global keys in P1) |
| APP-041 | System Information (Mission Center dialog) | ADAPT | P2 |
| APP-042 | Additional Setup window | ADAPT | P3 |
| APP-043 | Language and live locale change | ADAPT | P1 (key system and English), P4 (other locales) |
| APP-044 | Toasts | BUILD | P1 |
| APP-045 | Dialogs as separate pinnable windows | BUILD | P1 (frame), others with their features |
| APP-046 | Rendering GPU choice | ADAPT | P4 |
| APP-047 | Main tray icon and close/minimize to tray | ADAPT | P3 |
| APP-048 | Start at login | ADAPT | P2 |
| APP-049 | Privilege settings (unlock at launch, passwordless) | ADAPT | P3 (unlock at launch), P4 (passwordless) |
| APP-050 | Updater, installer and related items | DROP | P4 (recorded) |
| APP-051 | Open tasklens with Ctrl+Esc (KDE) | ADAPT | P4 |
| APP-052 | Command-line arguments | ADAPT | P1 (`--tab`, `--select-pid`, `--version`, `--help`, `--bench`), P2 (`--start-minimized`), P3 (`--low-memory`, `--watch-memory`, `--force-full-memory-mode`), P4 (`--app-id`, `--debug`) |
| APP-053 | About window | ADAPT | P2 |
| APP-054 | Logging | ADAPT | P1 |
| APP-055 | Crash handling and fatal errors | ADAPT | P1 |
| APP-056 | Public IP fetch notice | BUILD | P2 |
| APP-057 | NVIDIA G-SYNC profile | DROP | P4 (recorded) |
| APP-058 | Accessibility and reduced motion | ADAPT | P2 (scale-from-font default P4) |
| APP-059 | Loading placeholders | BUILD | P1 (frame rule), P2 (skeletons with the P2 tabs) |

# Old ID to new ID map

Every ID in `features-sys-app.md`. There are no old SYS-019 or SYS-029.

| Old ID | New ID |
|---|---|
| APP-001 | APP-001 |
| APP-002 | APP-002 |
| APP-003 | APP-003 |
| APP-004 | APP-004 |
| APP-005 | APP-005 |
| APP-006 | APP-006 |
| APP-007 | APP-007 |
| APP-008 | APP-008 |
| APP-009 | APP-009 |
| APP-010 | APP-010 |
| APP-011 | APP-011 |
| APP-012 | APP-012 |
| APP-013 | APP-013 |
| APP-014 | APP-014 |
| APP-015 | APP-015 |
| APP-016 | APP-016 |
| APP-017 | APP-017 |
| APP-018 | APP-018 |
| APP-019 | APP-019 |
| APP-020 | APP-020 |
| APP-021 | APP-021 |
| APP-022 | APP-022 |
| APP-023 | APP-023 |
| APP-024 | APP-024 |
| APP-025 | APP-025 |
| APP-026 | APP-026 |
| APP-027 | APP-027 |
| APP-028 | APP-028 |
| APP-029 | APP-029 |
| APP-030 | APP-030 |
| APP-031 | APP-031 |
| APP-032 | APP-032 |
| APP-033 | APP-033 |
| APP-034 | APP-034 |
| APP-035 | APP-035 |
| APP-036 | APP-036 |
| APP-037 | APP-037 |
| APP-038 | APP-038 |
| APP-039 | APP-039 |
| APP-040 | APP-040 |
| APP-041 | APP-041 |
| APP-042 | APP-042 |
| APP-043 | APP-043 |
| APP-044 | APP-044 |
| APP-045 | APP-045 |
| APP-046 | APP-046 |
| APP-047 | APP-047 |
| APP-048 | APP-048 |
| APP-049 | APP-049 |
| APP-050 | APP-050 |
| APP-051 | APP-051 |
| APP-052 | APP-052 |
| APP-053 | APP-053 |
| APP-054 | APP-054 |
| APP-055 | APP-055 |
| APP-056 | APP-056 |
| APP-057 | APP-057 |
| APP-058 | APP-058 |
| APP-059 | APP-059 |
| SYS-001 | SYS-001 |
| SYS-002 | SYS-002 |
| SYS-003 | SYS-003 |
| SYS-004 | SYS-004 |
| SYS-005 | SYS-005 |
| SYS-006 | SYS-006 |
| SYS-007 | SYS-007 |
| SYS-008 | SYS-008 |
| SYS-009 | SYS-009 |
| SYS-010 | SYS-010 |
| SYS-011 | SYS-011 |
| SYS-012 | SYS-012 |
| SYS-013 | SYS-013 |
| SYS-014 | SYS-014 |
| SYS-015 | SYS-015 |
| SYS-016 | SYS-016 |
| SYS-017 | SYS-017 |
| SYS-018 | SYS-018 |
| SYS-020 | SYS-019 |
| SYS-021 | SYS-020 |
| SYS-022 | SYS-021 |
| SYS-023 | SYS-022 |
| SYS-024 | SYS-023 |
| SYS-025 | SYS-024 |
| SYS-026 | SYS-025 |
| SYS-027 | SYS-026 |
| SYS-028 | SYS-027 |
| SYS-030 | SYS-028 |
| SYS-031 | SYS-029 |
| SYS-032 | SYS-030 |
| SYS-033 | SYS-031 |
| SYS-034 | SYS-032 |
| SYS-035 | SYS-033 |
| SYS-036 | SYS-034 |
| SYS-037 | SYS-035 |
| SYS-038 | SYS-036 |
| SYS-039 | SYS-037 |

# Still open

Nothing. The open items of this merge were settled in `docs/RECONCILIATION.md`:

| Item | Resolution | Entries |
|---|---|---|
| O-1 Column display keys for all tables | R-075 | APP-018, APP-032 |
| O-2 X11 window picker | R-076 | APP-006 |
| O-3 Corner radius | R-077 | APP-040 |
| O-4 About window title | R-078 | APP-045, APP-053 |
| O-5 `TS1.` share code encoding | R-079 (verify U-17) | APP-038 |
| O-6 kglobalaccel Ctrl+Esc | R-080 (verify U-16) | APP-051 |
| O-7 Number formats and locale | R-081 | APP-043 |
| O-8 Startup Apps live refresh | R-082 | SYS-019 |
| O-9 Strings written in this merge | R-083 | APP-001, APP-030, APP-037, APP-039, APP-045 |
| O-10 Handle Search result menu labels | R-102 | APP-015 |
