# tasklens merged spec: Services, Startup Apps, Connections and the app shell

This is the merged feature spec for two areas:
- SYS: the Services, Startup Apps and Connections tabs.
- APP: the application shell. It covers the title bar, toolbar, menus, shortcuts, settings, persistence, themes, dialogs, toasts, the main tray icon, the command line, logging, crash handling and single instance.

Source: `docs/spec/features-sys-app.md` (old IDs SYS-001 to SYS-039, APP-001 to APP-059).

Binding inputs, in order of precedence: `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-074 and canonical tables a to d), `docs/ARCHITECTURE.md` (ADR-001 to ADR-016) on engineering structure, then the source spec. Every resolution is already applied below. Where an entry cites R-0xx or D-0xx, that record set the value.

Conventions:
- Phases follow R-072. P0 is the spike. P1 is the Performance core, the process table and P1 actions. P2 adds Services, Startup Apps, Connections, Properties windows, the KWin bridge, the 5 and 10 minute ranges and the remaining themes. P3 adds the helper, history, tray, detached graphs, mini window, strings, dumps and low-memory. P4 is everything later.
- Names, paths, polkit actions and D-Bus names are from canonical table (c). Number formats are from canonical table (d). Settings keys are from canonical table (a). Keys are from canonical table (b).
- Polkit actions owned by tasklens are exactly three (R-002): `io.github.oysteinkrog.tasklens.helper`, `io.github.oysteinkrog.tasklens.setup` and `io.github.oysteinkrog.tasklens.netmon` (reserved). Every tl-helper operation runs under the helper action.
- Table cells, header totals and sidebar readouts use ADR-003's validity glyphs. Properties fields and Information rows use the PERF-079 words ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown") (R-050).
- "Was:" lines name old IDs from `features-sys-app.md`. Cross-references to other areas use their old IDs (PROC, PERF) where the new merged ID is not known.

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
  - Services "Jump to process" switches to Processes and selects MainPID by identity (pid plus start time). Toast "Switched to service process.". If filtered out or hidden: "Service process not visible in the current process view.". When MainPID is 0: "Service not running".
  - The reverse, Processes "Go to service" (menu item owned by the process spec), switches to Services, expands the group and selects the unit. Toasts "Switched to Services tab" / "Switched to service." / "Service not found in current view".
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

  - Missing values use the PERF-079 words (R-050). Mission Center's empty Description bug does not apply.
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
  - Read on page open and on `RefreshNow` (ADR-008). While the tab is open, inotify on all autostart dirs and the two Plasma dirs triggers a rescan (inferred: compatible with ADR-008, since it only runs while the page is in demand).
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
