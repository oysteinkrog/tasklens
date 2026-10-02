# tasklens reconciliation record

This file settles every contradiction found between `docs/DECISIONS.md`, `docs/ARCHITECTURE.md` (ADR-001 to ADR-016) and the feature specs (`spec/features-proc.md` v1, `spec/features-proc-r2.md`, `spec/features-proc-addendum.md`, `spec/features-perf.md`, `spec/features-perf-addendum.md`, `spec/features-sys-app.md`). Where this file speaks, it overrides those documents. Where it is silent, the precedence order is: DECISIONS.md, then ARCHITECTURE.md on engineering structure, then TaskSlinger for look, labels and defaults, then Mission Center for Linux data correctness, then verified machine facts over inferred claims. Among the three PROC files, r2 wins on a topic it covers, except where the addendum's later and more specific text or a decision says otherwise; v1-only topics stay.

Labels: "verified" means read in a document or checked on the machine; "inferred" means decided here from the rules because no document settles it.

PROC IDs below are prefixed `v1`, `r2` or `add` to say which file they come from.

## Resolutions

### R-001 Application ID, D-Bus names and object paths
- Sources in conflict: v1 PROC-003/PROC-030 `org.tasklens.helper.process`, `org.tasklens.Tasklens`, `/KWinBridge`; r2 helper note and PROC-059/064 `org.tasklens.helper`, `org.tasklens.App`, `/WindowBridge`, policy file `org.tasklens.policy`; PERF-094 `io.tasklens.setup`; ADR-001/006/010, APP-001/003/011, SYS-013/032/033 `io.github.oysteinkrog.*`.
- Resolution: app ID and session bus name `io.github.oysteinkrog.Tasklens`. Object path `/io/github/oysteinkrog/Tasklens` serves `org.freedesktop.Application`. The KWin bridge object is `/io/github/oysteinkrog/Tasklens/WindowBridge`, interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s)`. Polkit action prefix `io.github.oysteinkrog.tasklens.` (lower-case `tasklens`, as D-005 writes it). Policy file `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy` (ADR-001 packaging name). Desktop file `io.github.oysteinkrog.Tasklens.desktop`.
- Rule: D-005.
- Edits: v1 PROC-003 helper line and PROC-030; r2 helper note, PROC-059, PROC-064; PERF-094 action name. ADR-001 unchanged.

### R-002 Polkit action set
- Sources in conflict: ADR-006 one action `…tasklens.helper`; SYS-013 `…tasklens.service-dropin`; SYS-032/039 `…tasklens.inspect`; SYS-033 `…tasklens.close-connection`; PERF-094 setup action; APP-049 passwordless rule for the whole prefix.
- Resolution: three actions only. `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`) covers every tl-helper operation, including reads, SOCK_DESTROY and service drop-ins. `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`) covers `/usr/lib/tasklens/tasklens-setup`. `io.github.oysteinkrog.tasklens.netmon` is reserved for `tl-netmon`. Reason: polkit authorises the `pkexec` launch of the helper process once; it cannot authorise individual operations inside a running session, so per-operation action names would not be enforced (inferred from how pkexec works).
- Rule: ADR-006 wins on the privilege boundary.
- Edits: SYS-013, SYS-032, SYS-033, SYS-039, PERF-094, APP-049 (rule text stays prefix-based).

### R-003 Helper transport, flags and idle exit
- Sources in conflict: ADR-006 stdin/stdout framing, 300 s idle, no flag; v1 and r2 stdin/stdout, r2 `--session`, 5 min idle, r2 PROC-064 also `--once` single calls in P2; APP-003 "inherited socketpair", 15 min idle.
- Resolution: transport is the helper's stdin and stdout with `tl-model::wire` framing (max frame 64 KiB). Start command `pkexec /usr/lib/tasklens/tl-helper --session`. Idle exit after 300 s or when stdin reaches EOF. No `--once` mode: privileged actions exist only from the helper phase (P3). Replies larger than one frame are sent as a sequence of frames; the total per request is capped at 4 MiB (v1 PROC-003).
- Rule: ADR-006 wins; `--session` is a harmless addition.
- Edits: APP-003 (socketpair, 15 min); r2 PROC-064 phase note (`--once`); ADR-006 add `--session` and the multi-frame reply rule.

### R-004 Helper operation allowlist
- Sources in conflict: ADR-006 allows Signal, SetNice, SetAffinity, SetIoPriority, Freeze/Thaw/KillGroup, ReadProcessDetail only. PROC (v1 PROC-003/070/078, r2 PROC-064), SYS-013/032/033/039 need reads of io, fd list, fdinfo, environ, maps, smaps_rollup, ns, cwd, exe; strings scan; gcore dump; socket inode resolution; SOCK_DESTROY; recovery drop-in write; netns sock_diag dump.
- Resolution: ADR-006's allowlist is extended with exactly: `ReadProcessFiles { key, files: set of {io, fd_list, fdinfo, environ, maps, smaps_rollup, ns, cwd, exe, status} }`; `ScanStrings { key, opts }` (streams batches); `Dump { key, mode, out_fd }` (runs `/usr/bin/gcore` by fixed path, writes to the passed fd); `ResolveSocketInodes { inodes }`; `SockDestroy { family, proto, four_tuple, cookie }`; `WriteServiceDropIn { manager: System, unit, keys limited to Restart*, StartLimit*, FailureAction, OnFailure }` to the fixed path `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`; `NetnsSockDiagDump { netns_ino }`. Still no path, command or free-form file argument. The helper re-runs `open_verified` on every process target.
- Rule: rule 2, second clause: the ADR as written makes required features impossible, so the ADR changes.
- Edits: ADR-006 "Privileged helper boundary" list.

### R-005 Default Memory column is RssAnon
- Sources in conflict: D-004 RssAnon; ADR-008 "Memory (RSS)" and ADR-011 `memory_rss`; v1 PROC-020 and r2 PROC-020 VmRSS; add PROC-098/102 "Working set total - Memory", "Memory is RSS"; PERF-088 "RSS for the default Memory column".
- Resolution: column id `memory`, header "Memory", value RssAnon from `/proc/<pid>/status`. Header tooltip: "Private memory (RssAnon). Shared and file-backed pages are not counted." Picker alias `Working set private - "Memory"` (TaskSlinger's own alias). Remove the separate `ws_private` column (it duplicates `memory`). Keep `ws_total` (VmRSS, label "Working set total", tooltip "Resident set size (VmRSS)"), `pss`, `uss`, `swap`. App and unit rows: cgroup `memory.stat` `anon`, tooltip "Anonymous memory charged to this app's cgroup". Subtree totals: Σ RssAnon. Heat reference: 10% of MemTotal against RssAnon. Copy details lines: `Memory:` RssAnon, `Private bytes:` VmData+VmStk, `Working set:` VmRSS, all whole MiB. Minigraph label for the Memory column: "Memory"; for `ws_total`: "RSS"; for `ws_shared`: "RSS shared". History (ADR-012 `HistProcess`) stores RssAnon in `rss_kib`; rename the field `mem_anon_kib`.
- Rule: D-004.
- Edits: ADR-003 `ProcessRecord` gains `rss_anon_bytes: u64` and `ProcessField::RssAnon` (PROCESS_FIELDS = 13); ADR-008 memory sentence; ADR-011 column list; ADR-012 field; v1/r2 PROC-020; add PROC-098, PROC-102; PERF-088; r2 PROC-007 (anon only); r2 PROC-015.

### R-006 Default visible columns and Power usage
- Sources in conflict: ADR-011 `name,user,pid,cpu,memory_rss,disk_read,disk_write,net,gpu,description`; ADR-016 power hidden; v1 PROC-015 and add PROC-106 nine columns without Power; r2 PROC-016/024 Power on when RAPL readable; v1 PROC-022 single Disk column.
- Resolution: default visible, in order: `name, user, pid, cpu, memory, disk, network, gpu, description`. `power` and `power_trend` exist and are off by default on every machine. Default widths (logical px at 100%): Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342; Power usage 101 when shown.
- Rule: ADR-016 and the architecture review (no default power column without a per-process source; the column is an estimate). TaskSlinger's single Disk column wins over ADR-011's read/write pair (rule 3).
- Edits: ADR-011 `columns` and `column_widths` example; r2 PROC-016 and PROC-024 default-set sentences.

### R-007 Column identifiers and labels
- Sources in conflict: v1 gives string ids; r2 gives labels only, some renamed.
- Resolution: v1's string ids are canonical (PROC-015 to PROC-027 plus add PROC-103 `gpu_0..gpu_15`). r2's labels win where they differ, except where TaskSlinger has the label: "Handles" stays "Handles" (tooltip "Open file descriptors"), not "File descriptors". Adopt from r2: Type values "App, Background process, System process, Kernel thread, Unit, Pseudo"; Elevated values "Full (root)", "Capabilities", "Setuid" (euid differs from ruid; added, inferred), "Default"; Sandbox values; "Package", "Packager", "Package status", "Build date"; Session blank for 4294967295; Start time "HH:MM:SS" today else "YYYY-MM-DD HH:MM"; Priority class text with the nice value in the tooltip; Disk = Δ(read_bytes + write_bytes − cancelled_write_bytes)/Δt; GPU = max over adapters of max(SM, encoder, decoder).
- Rule: rule 5 and rule 3.
- Edits: v1 PROC-015/017/018/021/022/024/026 label columns; the merge agent carries ids into r2's tables.

### R-008 Command-line read and display limits
- Sources in conflict: v1 PROC-002/015 64 KiB; r2 PROC-001/017 4 KiB in the table, full on demand; ADR-015 32 KiB.
- Resolution: read limit 32 KiB with `…` appended (ADR-015). Table cell and hover tooltip show the first 512 characters; Properties shows the full 32 KiB.
- Rule: ADR-015.
- Edits: v1 PROC-002, PROC-015; r2 PROC-001, PROC-017.

### R-009 Row hover tooltip format
- Sources in conflict: add PROC-032 amendment (TaskSlinger line keys, cmdline first 512 chars); r2 PROC-013 ("path:", "user:", "Unit:", "Command line:" 8 lines, "Package:").
- Resolution: the addendum's block, plus r2's `Package: <name> <version>` line after `cgroup:` when known:
  `<command line, first 512 chars>` / `path: <exe or "restricted">` / `user: <name> (uid N)` / `Unit: <unit> (<Description>)` / `cgroup: <path>` / `Package: …` / `Desktop file: …` (app rows) / `Some values need administrator access.` (restricted rows).
- Rule: addendum later and more specific; r2 line added because it does not contradict.
- Edits: r2 PROC-013.

### R-010 View mode model, menu and cycle order
- Sources in conflict: ADR-011 `processes.mode = grouped|tree|flat`; r2 PROC-002 one enum, three radio items, default Grouping; add PROC-004 amendment (radio pair plus check), add PROC-105 cycle Tree, Grouping, List; APP-005 cycle Grouping, Tree, List; APP-032 and PROC-095 two booleans.
- Resolution: one setting `processes.mode` with values `grouped | tree | flat`, default `grouped`. View > Process View shows three radio items "Tree View", "Task Manager Grouping", "List View". The toolbar button cycles Tree View, then Task Manager Grouping, then List View (TaskSlinger string order). Toast "View mode: <name>". Tooltip "Toggle view mode (current: <name>)".
- Rule: ADR-011 on the data model; TaskSlinger string order on the cycle (rule 3).
- Edits: APP-005, APP-032 (`tree_view`, `grouping` keys), PROC-095 (same keys), add PROC-105 (two-boolean mapping), add PROC-004 amendment (drop).

### R-011 Default sort
- Sources in conflict: ADR-011 `cpu desc`; v1 PROC-037, r2 PROC-035, APP-032 PID ascending.
- Resolution: processes PID ascending; history CPU descending; services Name ascending; startup Status ascending then Name; connections Proto ascending then local port.
- Rule: rule 3 (TaskSlinger INI `Sort.ProcessesKey=3`, verified in the inventory).
- Edits: ADR-011 `sort` default.

### R-012 Ctrl+C and the copy commands
- Sources in conflict: ADR-015 "Copy" = TSV of visible columns; add PROC-098 Ctrl+C = Copy details (fixed block), Ctrl+Shift+C = "Copy rows as table" (TSV); APP-031 Ctrl+C = TSV in all tables; SYS-009 Ctrl+C = TSV.
- Resolution: in every table, Ctrl+C runs "Copy row" (processes: menu label "Copy details"; services, startup, connections: "Copy row"), which copies the fixed block from add PROC-098, SYS-009, SYS-026 or SYS-034. Ctrl+Shift+C runs "Copy rows as table": TSV of the visible columns with a header line. "Copy" copies the clicked cell. "Copy all metrics" copies every column with data as `Label: value` lines, ending with `Command line: …`. On the Performance tab Ctrl+C copies the device info (PERF-010). Command lines appear only in "Copy all metrics" and only for rows the user selected; this is the deliberate export ADR-015 allows.
- Rule: addendum later and more specific; applied to the other tables for consistency (inferred).
- Edits: ADR-015 "Exports and copy"; APP-031 Ctrl+C row; SYS-009; r2 PROC-037 and PROC-053.

### R-013 Number formats
- Sources in conflict: ADR-003 (3 significant digits for memory, no decimal on percent at or above 10, GHz with 2 decimals, durations "1d 4h"); add PROC-104 and r2 PROC-027 (percent "%.1f%%", memory 2 decimals, counts thin-space grouped); PERF-022 Mission Center rule (2 decimals below 10, 1 below 100); PERF-001 "4631 MHz"; PERF-080 uptime "2 days, 2h 38m 15s".
- Resolution: see canonical table (d). In short: percent one decimal everywhere, "0%" at measured zero; memory and storage IEC with 2 decimals, "512 B" below 1 KiB; rates IEC MiB/s with 1 decimal in tables, auto-scaled with 1-2-5 axis rounding on graphs; frequencies integer MHz; uptime in the TaskSlinger form; the short "4h 12m" form only inside tooltips and the Start time column's age text.
- Rule: rule 3 (TaskSlinger density and strings).
- Edits: ADR-003 units list; PERF-022 number-format line; PERF-025 tooltip "4.2 GHz" becomes "4200 MHz".

### R-014 Disk and I/O rate unit labels
- Sources in conflict: r2 PROC-021/027, PROC-029, PROC-077 "MB/s"; add PROC-104, ADR-003, PERF-022 "MiB/s".
- Resolution: "MiB/s" and "KiB/s" everywhere, base 1024. Heat reference 100 MiB/s. Top-3 hover "Disk: 4.2 MiB/s".
- Rule: ADR-003 and the review's IEC rule; the addendum agrees.
- Edits: r2 PROC-021, PROC-027, PROC-029, PROC-077; v1 PROC-033, PROC-084.

### R-015 Network units and auto-scale default
- Sources in conflict: PERF-022 auto-scale on, memory and disk base choices; APP-032 `units.network_auto_scale=false`, `network_base2`, `memory_*`, `disk_*`; ADR-003 `[units] network = bits|bytes` only; TaskSlinger INI `NetworkRateAutoScale=0` (verified).
- Resolution: `[units] network = "bits"` (bits are SI base 1000: Kbps, Mbps, Gbps; bytes are IEC: KiB/s, MiB/s). `[units] network_auto_scale = false`. With auto-scale off, values are fixed Mbps (or MiB/s) with one decimal, "0 Mbps" at zero. Graph axis maxima still use PERF-016 rounding regardless of this setting. No base-10 options for memory or disk; those Mission Center settings are dropped.
- Rule: rule 3 on the default (TaskSlinger 0); ADR-003 on the setting shape.
- Edits: PERF-022; APP-032 units rows; add PROC-104 "with auto-scale on" wording stays as the on-case.

### R-016 Graph time ranges and ring capacity
- Sources in conflict: ADR-003/009/011 ranges 15 s and 60 s, one 602-sample ring; PERF-013 30 s, 1, 5, 10 min with one Performance-wide range; APP-032 `graphs.time_range_s` 30, 60, 120, 300, 600; TaskSlinger minigraph ranges 30 s, 1 min, 5 min, 10 min (inventory, verified).
- Resolution: choices are 30 s, 1 min, 5 min, 10 min; default 1 min. Phase 1 ships 30 s and 1 min; 5 and 10 min arrive in P2 with the 1 Hz tier. Drop 15 s and 2 min. One setting `performance.graph_range_s` for the Performance tab (values 30, 60, 300, 600); each minigraph card keeps its own range. Rings: every series has a fast tier of capacity `ceil(60 s / period) + 2` (602 at 10 Hz) and, from P2, a 1 Hz tier of 602 samples (10 min + 2) whose value is the per-second maximum of the fast tier (same rule ADR-012 uses for history). `Sample` stays 16 bytes; no per-sample min/max pair. Memory about 1.5 MB for 80 series.
- Rule: rule 3 on the user-visible choices; ADR-003 structure kept and extended.
- Edits: ADR-003 ring paragraph; ADR-009 `ChartSpec.range_ns` comment; ADR-011 `graph_range_s`; PERF-013/014 (phases match); APP-032 `graphs.time_range_s` (rename and value list).

### R-017 Motion and animation settings
- Sources in conflict: ADR-011 one key `appearance.animations` covering "Smooth graphs" and motion; APP-032 `motion.animations`, `motion.smooth_graphs`, `motion.smooth_scroll`, `motion.smooth_scroll_amount`, `motion.tab_switch_ms`; TaskSlinger has separate INI keys (verified).
- Resolution: separate keys in a `[motion]` section as APP-032 lists them (table a). "Smooth graphs" alone decides chart sliding; "Animations" decides tab, theme and toast motion; system reduce-motion overrides all of them to off.
- Rule: rule 3 (TaskSlinger has the separate settings).
- Edits: ADR-011 schema; ADR-009 animation driver text ("setting Smooth graphs").

### R-018 Settings files, format details and corrupt-file handling
- Sources in conflict: ADR-011 `settings.toml` plus `window.toml`, user themes `themes/*.json`, `schema_version`, corrupt file renamed `.corrupt-<unix>`, toast text, no `directories` crate; APP-033 `settings.toml`, `state.toml`, `themes.json`, `version`, `.bad-<unix>`, different toast, `directories`, inotify reload, unknown keys kept; PROC-095 everything under `[processes]` in settings.toml including sort, columns and `history_mode`.
- Resolution: two TOML files. `$XDG_CONFIG_HOME/tasklens/settings.toml` holds user choices. `$XDG_STATE_HOME/tasklens/state.toml` holds window size and maximized, sort per table, columns per table, group expand state, last tab, Performance nav width and selected device, `history_mode`, minigraph layout, tray graphs. `window.toml` is dropped (its content moves to `[window]` in `state.toml`). User themes live in `$XDG_CONFIG_HOME/tasklens/themes.json` in TaskSlinger's `{"themes":[…]}` format; built-ins ship as `assets/themes/builtin.json`. Both TOML files start with `schema_version = 1`. Load, migrate, clamp, atomic save and the read-only rule for newer files follow ADR-011. A corrupt file is renamed `<name>.corrupt-<unix seconds>` and the toast reads "Settings file was unreadable and has been reset. The old file was kept as {name}". Unknown keys are ignored and not preserved. No `directories` crate. No inotify on the settings file; external edits apply at the next start.
- Rule: ADR-011 on structure; APP-033's split into settings and state adopted because ADR-011 already separates window state.
- Edits: ADR-011 paths table and ownership text; ADR-009 theme path; APP-033; PROC-095 (state keys move).

### R-019 Settings key names
- Sources in conflict: three different key sets (ADR-011, APP-032, PROC-095) plus PERF keys.
- Resolution: canonical table (a) is the only settings schema. Merge agents replace every key named in a spec with the table's name. Keys that no feature needs are dropped there with a reason.
- Rule: ADR-011 naming style (short section names, snake_case ids).
- Edits: APP-032, PROC-095, PERF-002/003/004/069/073, ADR-011.

### R-020 UI scale and row height
- Sources in conflict: ADR-009/011 `ui_scale = 1.0`, 0.8..=1.5, row height 28 × ui_scale; APP-032/040 `ui_scale_pct` 75-200 step 5, `row_height_pct` 80-150; TaskSlinger INI `UIScale`, `RowHeightPct` in percent (verified).
- Resolution: `appearance.ui_scale_pct = 100` (75..=200, step 5) and `appearance.row_height_pct = 100` (80..=150, step 5). Row height = 28 × row_height_pct/100 × ui_scale_pct/100 logical px. Toast "UI scale changed to %d%%". Ctrl+Plus/Minus/0 step by 5.
- Rule: rule 3.
- Edits: ADR-009 `UiMetrics`; ADR-011 `ui_scale`; APP-058 default-from-font rule stays (P4).

### R-021 Decorations setting
- Sources in conflict: ADR-011 `appearance.decorations = client|server`; APP-001/032 `window.system_titlebar` bool.
- Resolution: key `appearance.decorations = "client"` (`client | server`), settings label "Use system title bar". Change needs a restart (APP-001). Default client (D-006).
- Rule: D-006 and ADR-011.
- Edits: APP-001, APP-032.

### R-022 Window default and minimum size
- Sources in conflict: ADR-010 minimum 800×500, no default given; APP-010 default 1280×800, minimum 760×480.
- Resolution: default 1280×800 logical px; minimum 800×500.
- Rule: ADR-010 on the minimum; APP-010's default fills the gap (TaskSlinger INI, verified).
- Edits: APP-010; ADR-010 add the default.

### R-023 Tab set, order and labels
- Sources in conflict: ADR-009 lists Processes, Performance, App history, Startup apps, Users, Details, Services, Connections, Information; APP-027 six tabs; TaskSlinger has six tabs in order Processes, Performance, Services, Startup Apps, Connections, Information (inventory, verified).
- Resolution: six tabs in TaskSlinger order with these labels: Processes, Performance, Services, Startup Apps, Connections, Information. History is a mode of the Processes tab (the tab label reads "History" while it is on). There are no Users, Details or App history tabs; ADR open question 13 is closed as "not required".
- Rule: rule 3.
- Edits: ADR-009 tab list; ADR "Open questions" item 13.

### R-024 Start tab setting
- Sources in conflict: ADR-011 `general.start_tab` without `last_used`; APP-032 `startup.default_tab` with `last_used`; ADR `state.last_tab`.
- Resolution: `general.start_tab = "processes"` with values `processes | performance | services | startup | connections | information | last_used`. `state.toml` keeps `last_tab`.
- Rule: ADR-011 name; APP value list added.
- Edits: APP-032.

### R-025 History retention, caps, format and persistence
- Sources in conflict: D-003 off by default, 6 h; ADR-011/012 1..=24 h, `max_disk_mb = 256`, 16 MB RAM, segments per boot `seg-<boottime>.tlh`; v1 PROC-080/081 and r2 PROC-074/078 1..=12 h, 64 MiB RAM cap, 10-minute uncompressed window, `YYYYmmdd-HHMM.tlh`, "Save history to disk" setting, r2 shows a 10-minute rolling window when recording is off; APP-022 menu 1, 2, 3, 4, 6, 8, 12 hours.
- Resolution: `history.enabled = false`; `history.retention_h = 6`, range 1..=12; menu offers 1, 2, 3, 4, 6, 8, 12. `history.max_disk_mb = 256` (not shown in menus). `history.persist = true`: when false the recorder deletes its segments at shutdown and loads nothing at start; the on-disk format is unchanged. Storage format, RAM budget (16 MB), directory layout and segment names follow ADR-012. With recording off, History mode shows the "History recording is off" toast with an "Enable" button and no data; the r2 10-minute rolling window is dropped. Load error toasts from r2 PROC-078 stay; load time budget 2 s.
- Rule: D-003; ADR-012 on storage; TaskSlinger on the 1..=12 range (rule 3).
- Edits: ADR-011 `retention_h` range; ADR-012 retention sentence; v1 PROC-080/081; r2 PROC-074/078; APP-032.

### R-026 GPU sample rate
- Sources in conflict: ADR-007 device totals at 1 Hz, never faster; PERF-056 500 ms when Refresh is under 1 s.
- Resolution: GPU totals and GPU process data at 1 Hz. The Refresh setting does not affect the probe.
- Rule: ADR-007.
- Edits: PERF-056; PERF conflicts item 2.

### R-027 Tray implementation, icon sizes and menus
- Sources in conflict: ADR-010 `ksni =0.3.6`, sizes 22 and 44, menu "Show tasklens, graph choice, Quit", data from a 1 Hz GPUI timer in `tl-tray`; APP-047 hand-written SNI in the tl-engine zbus adapter, 22 and 44; PERF-078 ksni if no Tokio else hand-written, sizes 22, 32, 44; PERF-080/089 detailed menus.
- Resolution: `tl-tray` with `ksni =0.3.6` as ADR-010 says; `cargo xtask check-deps` proves no Tokio. If that check fails, the fallback is a hand-written SNI and dbusmenu server inside `tl-tray` on its own zbus connection, never inside tl-engine. Icon sizes 22 and 44 px. Tray graph icon menu (PERF-089 order): Graph color >, Time range >, Close tray icon graph, Tray icon graphs >, separator, Show Main Window, Show mini window (P3), Exit tasklens. Main tray icon menu (APP-047): Show Main Window, Tray icon graphs >, separator, Exit tasklens. Left click toggles the main window. Updates at most 1 Hz and only when pixels changed. Re-register on `StatusNotifierWatcher` owner change (PERF-090).
- Rule: ADR-010 on structure; TaskSlinger menus (rule 3).
- Edits: ADR-010 tray paragraph (menus); APP-047 mechanism; PERF-078 sizes.

### R-028 Additional setup: panel, not first-run dialog
- Sources in conflict: PERF-094 on-demand panel under Settings > Monitoring, no first-run dialog; APP-042 first-run dialog gated by `state.first_run_done`, reopened from Help; APP-024 Help item.
- Resolution: no first-run dialog. One window titled "tasklens Additional Setup", opened from Help > "Additional setup…", from Settings > All settings > Privileges, and from every "Needs setup" link. Rows: "CPU power draw" (RAPL rule, PERF-095), "Journal access" (shows `sudo usermod -aG systemd-journal $USER` with Copy; no automatic change), "Administrator helper" (Installed / Not installed), "Fan sensors" (PERF-097, P4), "Per-process network" (text only, PERF-096). Each row has status text, Enable or Revert, "View script" and "Copy command". Script `/usr/lib/tasklens/tasklens-setup` with subcommands `rapl-enable`, `rapl-revert`, `sensors-detect`, `polkit-rule on|off`. `state.first_run_done` is dropped.
- Rule: rule 3 (TaskSlinger has no first-run dialog); PERF-094 is the more complete design.
- Edits: APP-042 (rewrite to point at PERF-094), APP-032 state row, PERF-094 title and location.

### R-029 RAPL udev rule file
- Sources in conflict: PERF-095 `60-tasklens-powercap.rules`; r2 PROC-024 and APP-042 `99-tasklens-powercap.rules`; existing `/etc/udev/rules.d/99-powercap.rules` from Mission Center (verified present).
- Resolution: `/etc/udev/rules.d/99-tasklens-powercap.rules` with one line: `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`. `rapl-enable` writes it, then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add`. Status "Enabled (existing rule)" when `energy_uj` is already readable and the tasklens file is absent; tasklens never touches `99-powercap.rules`. The warning text from PERF-095 is shown before enabling.
- Rule: two of three documents agree; a 99- prefix sorts after distro rules (inferred).
- Edits: PERF-095 file name.

### R-030 DIMM serial numbers
- Sources in conflict: PERF-041 key list includes `SERIAL_NUMBER`; PERF-088 Information shows "Serial number"; PERF addendum PERF-085 hides serials.
- Resolution: serials are not shown on the Memory page, the Information tab or in any copy text. The udev key is not read. See owner question Q-1 if Oystein wants them.
- Rule: addendum is later and more specific; it also protects copy and screenshot output.
- Edits: PERF-041, PERF-088.

### R-031 Hide system processes: wording, key and scope
- Sources in conflict: v1 PROC-012 and r2 PROC-034 tooltip "Hide system background processes"; add PROC-012 amendment and APP-004 "Hide system processes" / "Show system processes"; keys `processes.hide_system_processes` (APP-032) vs `hide_system` (PROC-095).
- Resolution: tooltip "Hide system processes" / "Show system processes"; toasts "Hiding system processes" / "Showing system processes". Key `processes.hide_system = false`. Scope per r2 PROC-034 plus pseudo rows (add PROC-100). The button exists on the Processes tab only, removed (not greyed) on other tabs; in History mode it stays active.
- Rule: addendum later; key name by ADR style.
- Edits: v1 PROC-012, r2 PROC-034 tooltip text; APP-032 key.

### R-032 Pseudo-process rows
- Sources in conflict: r2 PROC-010 three rows (System Idle Process, System interrupts, Memory compression), toggle "Show pseudo processes" default on, Type "Pseudo"; add PROC-100 two rows ("Interrupts", "Memory compression (zram)"), no idle row, no toggle, Type "Kernel".
- Resolution: add PROC-100 wins: two rows, names "Interrupts" and "Memory compression (zram)", no extra toggle, hidden by Hide system processes, excluded from the process count, menu Copy, Copy details, Copy all metrics, Add Minigraph only. Type column shows "Pseudo" (r2's value list, R-007). Phase P2.
- Rule: addendum later and more specific; it also uses the verified `CONFIG_IRQ_TIME_ACCOUNTING=y` fact.
- Edits: r2 PROC-010; r2 PROC-017 Type list stays.

### R-033 Row context menu order and labels
- Sources in conflict: v1 PROC-049 (with add amendments); r2 PROC-041; app-row labels "Force End App" (v1, add PROC-096) vs "Kill App (all processes)" (r2); "Suspend App/Resume App" (v1) vs "Freeze App/Thaw App" (r2); Set Affinity submenu (v1) vs dialog "Set Affinity…" (r2).
- Resolution: r2 PROC-041 order, with "Copy details" inserted between "Copy" and "Copy all metrics". App and unit rows: "End App", "Freeze App" / "Thaw App", "Kill App (all processes)". "Set Affinity…" opens the r2 PROC-050 dialog. Pseudo rows and exited rows per R-032 and r2.
- Rule: r2 wins where it covers; addendum adds the one item it introduced.
- Edits: v1 PROC-049 (superseded); add PROC-096 app-row sentence ("Force End App" becomes "Kill App (all processes)").

### R-034 Multi-select labels
- Sources in conflict: add PROC-096 "End %d Processes"; r2 PROC-041 "End Selected Processes".
- Resolution: "End Selected Tasks", "End Selected Processes", "Suspend Selected Tasks", "Resume Selected Tasks". Toasts per add PROC-097 ("Selected process priority updated." and the rest).
- Rule: r2 pattern kept for consistency (inferred; TaskSlinger evidences only the toasts, not the labels).
- Edits: add PROC-096.

### R-035 Action toasts and identity-change text
- Sources in conflict: v1 PROC-050/051 toasts; r2 PROC-042/043/044; identity-change text capitalised (v1, TaskSlinger) vs lower-case (r2).
- Resolution: r2 PROC-042 to PROC-044 toasts. Identity mismatch toast is TaskSlinger's exact text: "The selected process changed; wait for the list to update and try again." EPERM toast "Permission denied" with button "Retry as administrator" (hidden until the helper phase ships; before P3 the toast has no button).
- Rule: r2 wins; TaskSlinger text wins on the quoted string.
- Edits: r2 PROC-042 string; v1 PROC-050/051 superseded.

### R-036 Confirmations and protected processes
- Sources in conflict: ADR-006 PROTECTED set and "Force end and End process tree on anything open an AlertDialog"; r2 PROC-052 (confirmations for session-critical, End Process Tree, Kill App, multi-select End of more than 1, any action on another user's process; refusals for tasklens, PID 1, kernel threads; adds sddm, login, systemd-logind to the critical list); r2 PROC-044 no confirmation for single End Process.
- Resolution: PROTECTED set = ADR-006's list plus `sddm`, `systemd-logind` and the session's `login` ancestor. Refused with toast: tasklens itself ("Refusing to end tasklens itself."), PID 1 ("PID 1 (systemd) cannot be ended."), kernel threads ("Kernel threads cannot be signalled."). Confirmation dialog for: any signal, freeze or kill on a PROTECTED target ("Ending <name> will end your desktop session. End it anyway?"); End Process Tree ("End N processes in this tree?", buttons End tree / Cancel); Kill App ("Kill all N processes of <App>?"); End Task or End Process with more than one target; any action on another user's process. No confirmation for a single End Process (SIGKILL) on the user's own non-protected process.
- Rule: rule 3 (TaskSlinger shows a toast, not a dialog, for End Process); ADR-006 protected set kept.
- Edits: ADR-006 "Protected processes" paragraph.

### R-037 End Process Tree order
- Sources in conflict: ADR-006 and v1 PROC-052 SIGTERM deepest first; r2 PROC-045 open all pidfds, SIGSTOP all, SIGKILL all.
- Resolution: r2's order. The engine opens a verified pidfd for every captured target, sends SIGSTOP to all, then SIGKILL to all, then closes the fds; one event per target. `ActionKind` gains `KillTree` so the reducer can order the two passes.
- Rule: r2 wins on topic; the stop-then-kill order prevents fork races, which ADR-006's goal (race-free actions) supports.
- Edits: ADR-005 `ActionKind`; ADR-006 "Tree operations"; v1 PROC-052.

### R-038 Set Priority, scheduling policy and the action types
- Sources in conflict: ADR-005 `SetNice(i8)` only; v1 PROC-057 and r2 PROC-049 Custom dialog with policy and RT priority, SCHED_RR 1 for Realtime, Idle item, confirmation text; phases v1 P2 vs r2 P1 for raising nice.
- Resolution: r2 PROC-049 behaviour and phases (P1 nice classes; P2 Custom, Realtime, Idle). `ActionKind` gains `SetSched { policy: SchedPolicy, rt_prio: u8 }`; tl-ops implements it with `sched_setattr` per TID under the same pidfd check as `SetNice`. Realtime sets SCHED_RR priority 1 with `SCHED_RESET_ON_FORK` after the confirmation "Realtime priority can freeze the desktop if the process spins. Set it anyway?".
- Rule: r2 wins; ADR-005 extended because the feature is required.
- Edits: ADR-005 enum; ADR-006 details list; v1 PROC-057.

### R-039 Set Affinity and Send Signal phases
- Sources in conflict: v1 Set Affinity P2, r2 P1; v1 Send Signal P1, r2 P2; add PROC-097 multi-select affinity P2.
- Resolution: Set Affinity dialog P1 for one target; multi-select mixed marks P2. Send Signal submenu P2 with r2's top-level list (TERM, KILL, HUP, INT, QUIT, STOP, CONT, USR1, USR2), "Other signals >", "Real-time signal…", suffix "(ends process)", toast "Sent SIGHUP to firefox (1234)".
- Rule: r2 wins.
- Edits: v1 PROC-056, PROC-059 phases.

### R-040 Suspend and resume processes vs apps
- Sources in conflict: v1 PROC-053/054 "Suspend App/Resume App" with toasts "App suspended."; r2 PROC-047/048 Freeze App / Thaw App, toasts "App frozen", "App thawed", "App killed".
- Resolution: r2.
- Rule: r2 wins.
- Edits: v1 PROC-054 superseded.

### R-041 New and exited linger time
- Sources in conflict: ADR-003 flags "within the last 2 s", ADR-004 reducer removes exited records 2 s after exit, ADR-009 highlight 2 s; v1 PROC-036 max(1 s, 2 intervals) capped 3 s; r2 PROC-031 2 refresh intervals clamped to 1..3 s.
- Resolution: linger = 2 × `processes.interval_ms`, clamped to 1..=3 s. The reducer keeps exited records for the linger time, then removes them. Fade over the last 300 ms unless reduced motion.
- Rule: r2 wins on behaviour; ADR constants follow.
- Edits: ADR-003 `ProcFlags` comments; ADR-004 "exited records are removed" sentence; ADR-009 highlight bullet.

### R-042 Ctrl-pause focus condition
- Sources in conflict: ADR-009 "while Ctrl is held and the table has focus"; r2 PROC-038 and APP-031 "while the main window has focus".
- Resolution: pause applies while Ctrl is held and the main window is active, whatever has focus inside it. Released on Ctrl-up or window deactivation.
- Rule: rule 3 (TaskSlinger behaviour as both specs describe it).
- Edits: ADR-009 Ctrl-pause bullet.

### R-043 Filter box: fields, Esc and the `filter_cmdline` key
- Sources in conflict: ADR-011 `processes.filter_cmdline = false` (cmdline matched only when set); v1 PROC-041 and r2 PROC-032 always match the command line; APP-028 first Esc clears, second returns focus; r2 one Esc clears and returns focus.
- Resolution: the filter always matches display name, image name, command line, user, description, app name, unit name, group names, and PID by prefix for numeric input. Drop `filter_cmdline`. One Esc clears the filter and returns focus to the table; a further Esc follows the global Esc rule (APP-031).
- Rule: rule 3 on fields; r2 on Esc (it owns PROC-032).
- Edits: ADR-011 key; ADR-009 `TableQuery.filter` comment; APP-028 Esc sentence.

### R-044 CPU percentage mode
- Sources in conflict: ADR-008/011 setting `process_cpu_mode = machine|core`; APP-032 `processes.cpu_one_core_100`; v1 PROC-019 and r2 PROC-019 no setting, two columns ("CPU" and "CPU (one core = 100%)").
- Resolution: no setting. Column `cpu` is percent of the whole machine; column `cpu_relative` is labelled "CPU (one core = 100%)". Drop both keys.
- Rule: rule 3 (TaskSlinger has the two columns; the toggle is Mission Center's).
- Edits: ADR-008 process CPU paragraph; ADR-011; APP-032.

### R-045 Keyboard: Enter, End Process names and the merged table
- Sources in conflict: APP-031 Enter toggles expand, "Force end" names, Ctrl+C TSV; v1 PROC-040 and r2 PROC-037 Enter opens Properties, Space toggles expand; add PROC-096 "End Process"; ADR-009 Ctrl+Tab tab switching.
- Resolution: canonical table (b). Enter and Ctrl+I and Alt+Enter open Properties; Space toggles expand. Shift+Delete and Ctrl+X are "End Process". Ctrl+Tab and Ctrl+Shift+Tab switch tabs in addition to Ctrl+1..6 and Alt+1..6. See owner question Q-3 on Enter.
- Rule: PROC owns the process table keys; ADR-009 keys kept.
- Edits: APP-031 rows for Enter, Shift+Delete, Ctrl+C; add Ctrl+Tab and Ctrl+Shift+C rows.

### R-046 KWin bridge: which crate owns it
- Sources in conflict: APP-009 "the KWin scripting channel lives in tl-ui's KDE integration module"; ADR-001 rule 1 forbids zbus in tl-ui; ADR-004 worker table has no KWin worker; PROC-030 (v1) and r2 PROC-059 describe the bridge without a crate.
- Resolution: the bridge is a tl-engine D-Bus worker `workers/dbus_kwin.rs`, `SourceId::KWin`, phase P2, event-driven. It writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`, loads it as plugin "tasklens-bridge", exports the WindowBridge object (R-001) on the engine's session-bus connection, and publishes `windows: Arc<WindowsSnapshot>` as a new `Snapshot` component (window list keyed by pid: internalId, caption, resourceClass, desktopFileName, minimized, maximized, keepAbove, frameGeometry, windowType). Commands (activate, close, keepAbove, pick) go through `Command::KWin(KWinCommand)` and run as one-shot scripts (r2 PROC-059). Availability: `XDG_CURRENT_DESKTOP` contains KDE and `org.kde.KWin` is on the bus; otherwise the window features are hidden with the toast "Window features need KDE Plasma" (v1 PROC-030). The X11 EWMH fallback in r2 PROC-057/058 is dropped for v1 (X11 is best effort per ADR-010).
- Rule: ADR-001 dependency rule; ADR-004 structure extended.
- Edits: ADR-003 `Snapshot`; ADR-004 worker table; APP-009, APP-006, APP-011 mechanism lines; r2 PROC-057/058 X11 lines.

### R-047 Single instance protocol
- Sources in conflict: ADR-010 `org.freedesktop.Application.Activate` at `/io/github/oysteinkrog/Tasklens`, no bus means no single instance; APP-011 custom `Activate(argv, platform_data)`, flock fallback, KWin activation.
- Resolution: ADR-010's `org.freedesktop.Application` interface. Argument forwarding uses `ActivateAction(name, parameter, platform_data)` with actions `quit`, `tab` (string), `select-pid` (uint32). The owner calls `window.activate_window()` and, when the KWin bridge is up, also activates through it. No flock fallback.
- Rule: ADR-010.
- Edits: APP-011.

### R-048 Logging
- Sources in conflict: ADR-015 tracing with `EnvFilter` from `TASKLENS_LOG`, daily rotation, 7 files, `tasklens.<date>.log`, targets `tasklens::*`; APP-054 5 MB rotation, 5 files, `tasklens.log`, levels incl. PERF, categories, custom line format, repeat collapsing, `RUST_LOG` honoured.
- Resolution: ADR-015 as written. No PERF level (use `info` on target `tasklens::perf`), no custom categories, no repeat collapsing, `RUST_LOG` not read.
- Rule: ADR-015.
- Edits: APP-054 (rewrite to point at ADR-015).

### R-049 Crash handling
- Sources in conflict: ADR-015 `crash-<unix seconds>.txt`, keep 20, toast on next start; APP-055 `crash-<timestamp>.log` with the last 200 log lines, desktop notification, next-start window with buttons, device-loss relaunch.
- Resolution: file `$XDG_STATE_HOME/tasklens/crash/crash-<unix seconds>.txt` with ADR-015's content plus the last 200 log lines (no process metadata), newest 20 kept. On crash: desktop notification "tasklens stopped after an unrecoverable error." Next start: a window "tasklens Crash Report" with the text "tasklens closed unexpectedly last time." and buttons "Open log folder", "Copy report", "Close" (replaces the ADR toast). Native crashes go to systemd-coredump; the window mentions `coredumpctl info tasklens`. GPU device loss: notification "tasklens lost the graphics device and must restart", relaunch once.
- Rule: ADR-015 on file and content; APP-055 on the user-facing part (ADR is not the authority on UI).
- Edits: ADR-015 crash paragraph; APP-055 file name.

### R-050 Validity glyphs and fallback words
- Sources in conflict: ADR-003 glyph table for cells (`…`, lock, `n/a`, `-`, `!`); v1 PROC "?" for failed; r2 lock tooltip "Needs administrator rights"; v1 "Needs administrator. Click the shield to grant access."; PERF-079 word table for the stats grid ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown").
- Resolution: table cells, header totals and sidebar readouts use ADR-003's glyphs. Performance stats grids, Information rows and Properties key/value fields use PERF-079's words. Tooltips use ADR-003's texts, with the Restricted tooltip changed to "Needs administrator access. Click the shield to grant it." (before P3: "Needs administrator access."). "Needs setup" is Validity `Unsupported` with a reason tooltip linking to the setup window. "Not reported" is `Unsupported` where the source answers with no value.
- Rule: ADR-003 structure; TaskSlinger words where TaskSlinger has them (rule 3).
- Edits: ADR-003 tooltip text; v1 PROC conventions ("?"); r2 conventions.

### R-051 Stale and suspend thresholds
- Sources in conflict: ADR-008 stale `max(3 × period, 1.5 s)`, suspend when `Δboot − Δmono > 250 ms`; PERF-019 stale 3 × period, suspend jump over 2 s; r2 "older than 3 sample periods".
- Resolution: ADR-008 values.
- Rule: ADR-008.
- Edits: PERF-019.

### R-052 Which files the collectors read
- Sources in conflict: ADR-016 `/proc/net/dev`, `/proc/diskstats`, device enumeration every 5 s; PERF-050 per-interface `statistics/{rx,tx}_bytes`, PERF-043 `/sys/block/<d>/stat`, PERF-005 netlink uevent plus 5 s rescan.
- Resolution: ADR-016 sources: one `pread` of `/proc/net/dev` and `/proc/diskstats` per tick; enumeration by rescan every 5 s while demanded. No uevent netlink socket in v1 (may be added in P4 as an optimisation bead).
- Rule: ADR-016.
- Edits: PERF-043, PERF-050, PERF-005.

### R-053 Virtual and tunnel interfaces
- Sources in conflict: ADR-016 hides `veth*, docker*, br-*, virbr*, tailscale*` by default with key `show_virtual_interfaces = false`; PERF-004/049 Tunnel and VPN group on by default (so `tailscale0` shows), Virtual off, with keys `performance.show_network_{wired,wireless,vpn,virtual,other}`.
- Resolution: PERF classification and keys. `tailscale0` is a Tunnel and is shown. `docker0`, `veth*`, `br-*`, `virbr*`, `vnet*` are Virtual and hidden by default. `lo` never shows.
- Rule: PERF is more specific and matches TaskSlinger (it shows Tailscale, PERF-049).
- Edits: ADR-016 network row and key name.

### R-054 GPU page graph layout keys
- Sources in conflict: APP-032 `performance.gpu_graph_middle`, `gpu_graph_bottom` (Mission Center choices); PERF-057 fixed rows: engine grid, then Memory and Temperature, then optional Power and Clocks via a checklist.
- Resolution: PERF-057 layout. Keys: `performance.hidden_gpu_engines = []` and `performance.gpu_optional_graphs = []` (values `power`, `clocks`). Drop the middle and bottom keys.
- Rule: rule 3 (TaskSlinger 0.9.2 layout).
- Edits: APP-032.

### R-055 Sidebar card headings and lines
- Sources in conflict: PERF-001 network card title "interface display name, its type, then rate"; PERF addendum (verified against `performance.png`): heading is the interface name, line 1 the type, line 2 the rate.
- Resolution: addendum. Headings "CPU", "Memory", "GPU N", "Disk N", `<ifname>`, "Fan N", battery kind. Network line 2 shows `↓ 1.2 Mbps ↑ 0.3 Mbps` when both are non-zero, else the total (PERF-084).
- Rule: rule 4 (verified screenshot).
- Edits: PERF-001.

### R-056 CPU security row
- Sources in conflict: PERF-032 "CPU security: N mitigated, N not affected, N vulnerable"; PERF addendum "Security features" listing Secure Boot, Kernel lockdown, SME, user shadow stack, IOMMU, with the vulnerability summary in the tooltip.
- Resolution: addendum. Row label "Security features".
- Rule: addendum takes precedence (leader note).
- Edits: PERF-032.

### R-057 Displays list source
- Sources in conflict: PERF-065 says the sysfs connector `enabled` file is wrong on NVIDIA 615; PERF addendum found KScreen agreeing with sysfs and asks to withdraw the claim; PERF-065 orientation from `kscreen-doctor --json` vs addendum `-j` key list.
- Resolution: mode, refresh, connector type and `max bpc` come from DRM KMS read-only ioctls on `/dev/dri/cardN`; enabled state, primary, orientation and scale come from `kscreen-doctor -j` on KDE. The sysfs `enabled` claim is withdrawn and moves to the unverified list. Row format and connector names per PERF-091. HDR, VRR and wide gamut are P4.
- Rule: rule 4.
- Edits: PERF-065 (remove the `enabled` sentence; adopt the addendum's mechanism).

### R-058 Public IP setting location and mini window trigger
- Sources in conflict: PERF-054 setting under Monitoring; PERF addendum and APP-022 under Window & startup. PERF-082 mini window opened by double-click; addendum adds View > Window item and Ctrl+Shift+M.
- Resolution: "Auto-fetch Public IP on launch" under Window & startup, key `network.auto_fetch_public_ip`. Mini window: View > Window > "Performance mini window" (checkable), Ctrl+Shift+M, tray menu "Show mini window" (P3), and double-click on the Performance graph area. Double-click on the mini window returns to the main window.
- Rule: addendum takes precedence; TaskSlinger string order (verified in the inventory).
- Edits: PERF-054, PERF-082.

### R-059 Heat colouring references
- Sources in conflict: v1 PROC-033 Power reference "class index/4"; r2 PROC-029 Power 25 W; disk and I/O "100 MB/s".
- Resolution: r2 references with IEC labels: CPU 400/ncpu % (12.5% here), CPU (one core = 100%) 400%, memory columns 10% of MemTotal, disk and I/O rates 100 MiB/s, network 10 MiB/s (or 80 Mbps), GPU 100%, VRAM 25% of total VRAM, power 25 W. Counts are not tinted. On by default, intensity 50%.
- Rule: r2 wins; R-014 on units.
- Edits: v1 PROC-033; r2 PROC-029 labels.

### R-060 Startup placeholders
- Sources in conflict: v1 PROC-046 no shimmer on the Processes tab; APP-059 skeleton rows on first load for every tab; SYS-016 skeleton rows plus "Loading service data..." toast.
- Resolution: Processes tab shows rows from the first sweep with `…` in rate cells and no skeleton. Services, Startup Apps, Connections and Information show skeleton rows or blocks until their first data, once per session. Services also shows the toast "Loading service data...".
- Rule: rule 3 (TaskSlinger has no shimmer on Processes but does have the services toast).
- Edits: APP-059 (exclude Processes).

### R-061 Run dialog and Search File Handles details
- Sources in conflict: window titles "tasklens Run" (v1, APP-014/045) vs "Run" (r2); administrator run `pkexec /bin/sh -c` (r2) vs `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c` (APP-014); Handle Search result columns (APP-015 four columns) vs r2 PROC-080 six columns and menu.
- Resolution: titles "tasklens Run" and "tasklens Handle Search" (APP-045 naming). Administrator run uses APP-014's `pkexec env … /bin/sh -c <text>` under `org.freedesktop.policykit.exec`, never tl-helper. `kcm_*` names run `systemsettings <name>` (r2). History file and format per r2 PROC-079. Handle Search columns and menu per r2 PROC-080; footer note per r2.
- Rule: APP owns the window frame and privilege path; r2 owns the content.
- Edits: r2 PROC-079 title and pkexec line; APP-015 columns.

### R-062 Low-memory watcher thresholds
- Sources in conflict: v1 PROC-093 PSI `full 150000 1000000`, MemAvailable under 5%; r2 PROC-081 `full 100000 1000000`, under 3%, MemAvailable check every 5 s.
- Resolution: r2 values.
- Rule: r2 wins.
- Edits: v1 PROC-093 superseded.

### R-063 History hover and header formats
- Sources in conflict: v1 PROC-084 "Memory: %.2f MiB/GiB", "Disk: %.2f MB/s"; r2 PROC-077 "Memory: 1.22 / 62.70 GiB", "Disk: 4.20 MB/s", "GPU: 41.0%".
- Resolution: "CPU: 12.3%", "Memory: 1.22 / 62.70 GiB", "Disk: 4.2 MiB/s", "Network: 12.0 Mbps", "GPU: 41.0%".
- Rule: r2 wins; R-013/R-014 formats.
- Edits: r2 PROC-077; v1 PROC-084.

### R-064 Process metadata refresh cadence and time base
- Sources in conflict: ADR-016 cmdline, exe and cgroup refreshed every 10 sweeps and on comm change; r2 PROC-001 every 10 s; r2 "Times use CLOCK_MONOTONIC"; v1 PROC-002 cgroup re-read only on new pid or exec.
- Resolution: ADR-016 (every 10 sweeps and immediately when `comm` changes). All timestamps are CLOCK_BOOTTIME (ADR-003). fd count via `fstatat` `st_size` of `/proc/<pid>/fd` with readdir fallback (r2).
- Rule: ADR-003/016.
- Edits: r2 PROC-001 two bullets.

### R-065 Column picker title, headings and aliases
- Sources in conflict: v1 PROC-048 headings "Identity, File, Security, …"; r2 PROC-016 "Image, Process, CPU, Memory, I/O, Disk, Network, GPU, Objects, Security, Linux" and aliases `Working set private - "Private (anon)"`; add PROC-102 "Columns" title, headings "Image, Identity, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux", aliases; APP-018 example alias `Resident memory - "Memory"`.
- Resolution: add PROC-102 title and headings. Aliases: `Working set private - "Memory"` (R-005) and `GPU memory dedicated - "VRAM"`. The filter field matches full name and alias.
- Rule: addendum later and more specific.
- Edits: r2 PROC-016 headings and aliases; APP-018 example.

### R-066 Minigraph card titles for process sources
- Sources in conflict: v1 PROC-087 "<name> (<pid>) - CPU"; r2 PROC-062 "firefox (Process 1234) CPU", "firefox by name CPU", "Firefox by app CPU"; PERF-077/086 "firefox (Process 1234)", "firefox by name", "Firefox by app", column-keyed cards append " · CPU", and "<name> (<pid>)" with "Process <pid>" only before the name is known.
- Resolution: PERF-086: `<name> (<pid>) · CPU`, `<name> by name · CPU`, `<App> by app · CPU`; `Process <pid>` only until the name is read. States "No match" and "Ended" per PERF-087; r2's extra toasts stay.
- Rule: PERF owns the card (both PROC files say so).
- Edits: r2 PROC-062 titles; v1 PROC-087.

### R-067 Tray tooltip for process graphs
- Sources in conflict: v1 PROC-090 "<name> (PID n): CPU 3.2%"; r2 PROC-063 "firefox: CPU 3.2%"; PERF-079 "CPU Usage: 18.6%" for page graphs, "3 procs: firefox" for name filters.
- Resolution: PERF-079 forms: page graph "<Title>: 18.6%", process graph "<name> (PID n): CPU 3.2%", name filter "3 procs: firefox", app "<App>: CPU 3.2%".
- Rule: PERF owns the tray; v1's PID form kept because it disambiguates (inferred).
- Edits: r2 PROC-063.

### R-068 Built-in themes in phase 1
- Sources in conflict: ADR-009 v1 ships "Default Dark" only; APP-035 P1 Default Dark and Default Light, P2 the other 10.
- Resolution: P1 ships Default Dark and Default Light (the `theme.light` default needs it); the other 10 in P2.
- Rule: rule 3 (TaskSlinger defaults `LightTheme=Default Light`, verified).
- Edits: ADR-009 theme bullet.

### R-069 Theme settings keys
- Sources in conflict: ADR-011 `appearance.theme = "Default Dark"` only; APP-032 `theme.active`, `theme.follow_system`, `theme.light`, `theme.dark`.
- Resolution: `[theme]` section with the four APP keys (table a).
- Rule: rule 3 (TaskSlinger INI keys, verified).
- Edits: ADR-011.

### R-070 Rendering GPU setting vs `ZED_DEVICE_ID`
- Sources in conflict: ADR-010 "tasklens does not set `ZED_DEVICE_ID`"; APP-046 sets it from `rendering.gpu` (P4).
- Resolution: ADR-010 holds for phases 0 to 3. In P4, `rendering.gpu` may set `ZED_DEVICE_ID` or the software-renderer variables before GPUI starts; a user-set `ZED_DEVICE_ID` in the environment still wins.
- Rule: ADR-010 with a dated exception for a later feature.
- Edits: ADR-010 "GPU selection".

### R-071 Dropped settings keys
- Sources in conflict: APP-032 and PROC-095 list keys that no resolved feature uses.
- Resolution: dropped: `tables.remember_sort`, `tables.remember_columns` (always persisted), `processes.column_graphs` (fixed six), `processes.cpu_one_core_100` and ADR `processes.cpu_mode` (R-044), `processes.filter_cmdline` (R-043), `units.network_base2`, `units.memory_*`, `units.disk_*` (R-015), `performance.gpu_graph_middle/bottom` (R-054), `state.first_run_done` (R-028), `window.system_titlebar` (R-021), `copy_rows_tsv_shortcut` (documentation only, add PROC-095), `processes.tree_view`, `processes.grouping` (R-010), `debug.show_health_window` (the Collector health window is opened from the Debug menu or status bar, not a setting).
- Rule: each resolution above.
- Edits: APP-032, PROC-095, ADR-011.

### R-072 Phase numbering
- Sources in conflict: ADR-004 worker table and ADR-016 use phases 1, 2, 3 and "later" (Processes worker is phase 2, Connections and Services phase 3); every spec uses P1..P4 with the process table in P1 and Services in P2.
- Resolution: canonical phases: P0 spike (ADR-014); P1 = ADR phases 1 and 2 (Performance core, process table, P1 actions); P2 = ADR phase 3 (Services, Startup Apps, Connections, Properties windows, KWin bridge, 5 and 10 min ranges, remaining themes); P3 = helper, history, tray, detached graphs, mini window, strings, dumps, low-memory; P4 = everything marked P4 in the specs. ADR crates named "later phase" map: `tl-history`, `tl-tray`, `tl-helper` to P3; `tl-netmon` to P4.
- Rule: specs' numbering is used by every bead; ADR text is relabelled.
- Edits: ADR-004 worker table Phase column (2 becomes P1, 3 becomes P2, later becomes P3 or P4); ADR-016 bracketed phases; ADR-001 crate comments.

### R-073 Process Properties pin and keep-above
- Sources in conflict: v1 PROC-063 pin via one-shot KWin script; r2 PROC-065 pin hidden without the bridge; APP-009 pin on every dialog.
- Resolution: the pin button is shown on every secondary window and on the main window; on KDE it sets `keepAbove` through the bridge (R-046); on X11 it sends `_NET_WM_STATE_ABOVE`; elsewhere it is disabled with tooltip "Not supported by this desktop". Only the main window's state persists (`window.always_on_top`).
- Rule: APP-009 owns the pin; consistent with ADR-010.
- Edits: r2 PROC-065 ("hidden" becomes "disabled").

### R-074 Journal reading
- Sources in conflict: ADR-016 `journalctl -o json -n 50 --no-pager -u <unit>` with 5 s timeout; SYS-015 200 entries, follow mode, "Load older", user units via `--user-unit`.
- Resolution: SYS-015 behaviour: `journalctl -u <id> -b -n 200 -o json --no-pager` (`--user-unit` for user units), `-f` while Follow is on, the child killed when the tab closes; 5 s timeout applies to the non-follow fetch.
- Rule: SYS owns the feature; ADR-016 updated to match (engineering detail, not a structural rule).
- Edits: ADR-016 "Service logs" row.

## Owner questions

Only choices that no rule settles. Beads use the recommended answer until Oystein says otherwise.

### Q-1 Show DIMM serial numbers?
- Options: (a) hidden everywhere (R-030); (b) shown on the Memory page and Information tab but left out of copy text; (c) shown everywhere.
- Recommendation: (a). The data is readable without root, so nothing is lost by adding it later. Copy text and screenshots are the usual way this leaks.

### Q-2 Keep Mission Center's base-10 unit options for memory and disk?
- Options: (a) drop them, IEC only (R-015); (b) keep `units.memory_base` and `units.disk_base` with IEC default.
- Recommendation: (a). TaskSlinger has no such option, the review binds IEC, and every format string in the specs assumes IEC. Keeping it doubles the format test matrix.

### Q-3 What does Enter do on a process row?
- Options: (a) opens Properties (both PROC files, R-045); (b) toggles expand on rows with children (APP-031, Mission Center).
- Recommendation: (a). TaskSlinger's binary evidences no table keys, so there is no reference; Properties is the Windows Task Manager convention and Space already toggles expand.

### Q-4 Confirmation for a single "End Process" (SIGKILL) on the user's own process?
- Options: (a) none, toast only (R-036, TaskSlinger); (b) confirmation dialog as ADR-006 wrote.
- Recommendation: (a). TaskSlinger shows only "End process requested."; End Process Tree, Kill App, multi-select and protected targets still confirm.

### Q-5 App-row Memory: cgroup `anon` or `anon + shmem`?
- Options: (a) `anon` (R-005, matches RssAnon per process); (b) `anon + shmem` (both PROC files, written when Memory was RSS).
- Recommendation: (a). It keeps the app row and its child rows on the same definition, so the app value is never smaller than a child's.

### Q-6 Does History mode show anything while recording is off?
- Options: (a) nothing, toast with an "Enable" button (R-025); (b) a 10-minute rolling window kept in RAM while the Processes tab is open (r2 PROC-074).
- Recommendation: (a). Option (b) needs a second store outside ADR-012 and keeps the process sweep running for a feature the user turned off.

## Canonical tables

### (a) Settings schema

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
dim_zero_values = false
column_separators = false
column_resize = "proportional" # proportional | last_column
dump_type = "mini"             # mini | full

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
source = "cpu"                 # cpu | memory | disk | network | gpu | vram | gpu_temp | process:<pid>:<start_ticks> | name:<image name> | app:<app id>
column = "cpu"                 # for process sources
range_s = 60
color = "accent"               # accent | <one of the 17 names>
slot = 0
detached = false

[[tray_graphs]]                # P3
source = "cpu"
column = "cpu"
color = "accent"
range_s = 60
```

User themes: `$XDG_CONFIG_HOME/tasklens/themes.json`, TaskSlinger format `{"themes":[{"name":..., "SLOT":"#RRGGBB", ..., "accent":"#RRGGBB", "graph":{...}}]}`.

`EngineSettings` (derived, never persisted): `interval_ms`, `network` unit, `show_network_*`, `history.*`, `processes.subtree_totals`, `processes.hide_system`.

### (b) Keyboard shortcuts

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

### (c) Names, paths, polkit actions and udev rules

Identity:
- App ID, Wayland `app_id`, desktop file stem, session bus name: `io.github.oysteinkrog.Tasklens`.
- Desktop file: `/usr/share/applications/io.github.oysteinkrog.Tasklens.desktop`. Autostart entry (Start at login): `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop`.
- D-Bus object `/io/github/oysteinkrog/Tasklens`: interface `org.freedesktop.Application` (`Activate`, `Open` returns an error, `ActivateAction` with `quit`, `tab`, `select-pid`).
- D-Bus object `/io/github/oysteinkrog/Tasklens/WindowBridge`: interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s json)`. P2.
- KWin script: `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`, plugin name `tasklens-bridge`; one-shot scripts `tasklens-cmd-<n>`; picker script `tasklens-pick`.
- Tray items: `org.kde.StatusNotifierItem-<pid>-<n>`, objects `/StatusNotifierItem` and `/MenuBar`. P3.
- Transient scopes: `app-tasklens-run-<rand>.scope` (Run dialog), `app-tasklens-<escaped id>-<rand>.scope` (Run now).
- User units tasklens may install: `tasklens-lowmem.service` (P3). Test unit for integration tests: `tasklens-test.service`.

Polkit (policy file `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy`):
- `io.github.oysteinkrog.tasklens.helper`, `auth_admin_keep`: `pkexec /usr/lib/tasklens/tl-helper --session`. P3.
- `io.github.oysteinkrog.tasklens.setup`, `auth_admin_keep`: `pkexec /usr/lib/tasklens/tasklens-setup <rapl-enable|rapl-revert|sensors-detect|polkit-rule on|off>`. P3.
- `io.github.oysteinkrog.tasklens.netmon`: reserved for `tl-netmon`. P4.
- Third-party actions used as-is: `org.freedesktop.systemd1.manage-units`, `manage-unit-files`, `reload-daemon`; `org.freedesktop.udisks2.ata-smart-update`, `filesystem-unmount-others`, `eject-media`; `org.freedesktop.policykit.exec` (Run as administrator).
- Optional rule (P4, APP-049): `/etc/polkit-1/rules.d/49-tasklens.rules`.

Binaries and scripts:
- `/usr/bin/tasklens`; `/usr/lib/tasklens/tl-gpu-probe`; `/usr/lib/tasklens/tl-helper` (P3); `/usr/lib/tasklens/tl-netmon` (P4); `/usr/lib/tasklens/tasklens-setup` (P3).
- Probe lookup order: `$TASKLENS_GPU_PROBE`, directory of the running binary, `/usr/lib/tasklens/tl-gpu-probe`.

udev and system files:
- RAPL: `/etc/udev/rules.d/99-tasklens-powercap.rules`, one line `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`. Mission Center's `/etc/udev/rules.d/99-powercap.rules` is detected and left alone.
- Fan modules (P4): `/etc/modules-load.d/tasklens-sensors.conf`.
- Service recovery drop-ins (P3): `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf` via the helper; `~/.config/systemd/user/<unit>.d/50-tasklens-recovery.conf` written directly.

XDG paths (ADR-011, directories 0700, files 0600):
- `$XDG_CONFIG_HOME/tasklens/settings.toml`, `themes.json`.
- `$XDG_STATE_HOME/tasklens/state.toml`, `logs/`, `crash/crash-<unix seconds>.txt`, `history/<boot_id 16 hex>/seg-<boottime s, 10 digits>.tlh`, `dumps/<name>_<pid>_<yyyymmdd_hhmmss>.core`, `run-history.json`.
- `$XDG_CACHE_HOME/tasklens/icons.toml`.
- `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`.

Environment variables read: `TASKLENS_LOG`, `TASKLENS_GPU_PROBE`, `TASKLENS_DEBUG`, `ZED_DEVICE_ID` (respected, set by tasklens only in P4 per R-070), `__NV_FORCE_ENABLE_X11_EGL_PLATFORM` (set on X11 with NVIDIA, ADR-010), `TL_UPDATE_GOLDEN` (tests).

Command-line arguments (APP-052): `--start-minimized`, `--low-memory`, `--watch-memory`, `--tab <name>`, `--select-pid <pid>`, `--debug`, `--bench <scene> [--duration --warmup --json]` (ADR-014 scenes `perf-cpu`, `table-2000`, `bridge`, `static`, plus `history-scrub` in P3), `--force-full-memory-mode`, `--app-id <id>`, `--version`, `--help`. APP-052's `--bench [charts|table|history]` and `--bench-out` are replaced by ADR-014's form.

### (d) Number and unit formats

All formatting lives in `tl-model::units`. "Measured zero" means a valid sample equal to zero; missing values use validity glyphs (R-050), never a zero.

| Kind | Format | Measured zero | Examples |
|---|---|---|---|
| Percent (CPU, GPU, memory %, header totals, stats grids) | one decimal, `%.1f%%` | `0%` | `11.9%`, `3.4%`, `100.0%` |
| Memory and storage sizes | IEC, base 1024, units B KiB MiB GiB TiB, two decimals from 1 KiB up; integer bytes below | `0 B` | `512 B`, `16.00 KiB`, `1.22 GiB`, `34.68 GiB / 63.16 GiB` |
| Whole-MiB forms (Copy details, history "Memory: 1.22 / 62.70 GiB" excepted) | integer MiB | `0 MiB` | `812 MiB` |
| Disk and I/O rates in tables | fixed MiB/s, one decimal | `0 MiB/s` | `0.4 MiB/s` |
| Disk rates on Performance graphs and stats | auto IEC unit (KiB/s, MiB/s, GiB/s), one decimal; axis max rounded to a power of two | `0 KiB/s` | `12.3 MiB/s`, axis `4 GiB/s` |
| Network, bits (default) | SI base 1000; fixed Mbps one decimal when auto-scale is off; Kbps/Mbps/Gbps when on; graph axis 1-2-5 × 10ⁿ | `0 Mbps` | `2.3 Mbps`, `0.0 Mbps` is never shown for zero |
| Network, bytes | IEC like disk rates | `0 MiB/s` | `1.5 MiB/s` |
| Counts (threads, handles, faults, ops) | digits grouped with a thin space (U+2009) | `0` | `12 345` |
| CPU time | `h:mm:ss`; past 999:59:59 use `Nd hh:mm` | `0:00:00` | `12:03:44` |
| Uptime (CPU page, Information) | `N day(s), Hh Mm Ss`; hours always shown | | `2 days, 2h 38m 15s`, `0h 38m 15s` |
| Short ages (tooltips, "Last updated … ago") | `1d 4h`, `4h 12m`, `12m 5s`, `5s` | | |
| Start time column | today `HH:MM:SS`, else `YYYY-MM-DD HH:MM`; Properties "Started" and Boot time `YYYY-MM-DD HH:MM:SS` local | | |
| Frequencies | integer MHz | | `4631 MHz`, `1905 / 3105 MHz` |
| Temperatures | integer °C | | `52 °C` |
| Power | one decimal W | | `31.9 / 300 W` |
| Cache sizes | integer KiB up to 1023 KiB, then MiB (one decimal when not exact) | | `512 KiB`, `8 MiB`, `96 MiB` |
| Addresses | `a.b.c.d:port`, `[v6%scope]:port`, wildcards `0.0.0.0:*`, `[::]:*`, `*:*` | | |
| Response time | ms, two decimals | | `0.42 ms` |

Numeric cells use tabular figures and are right-aligned; numeric columns have fixed minimum widths from the template strings in add PROC-101 (`"999999 +99"`, `"100.0%"`, `"xxx Mbps"` or `"xxx MiB/s"`, `"xxx GiB/s"`, `"999 MiB"`, `"999999"`, `"999:59:59"`). Text columns have a 60 px minimum.

## Unverified assumptions

Every claim marked assumed, inferred or to be checked. Phase 0 or the first bead that touches it must confirm or change it.

| # | Claim | Source |
|---|---|---|
| U-1 | Nothing in the gpui-kit 0.7.0 Linux dependency graph pulls in Tokio (only `gpui-pre-reqwest-client` does, and it is not in the graph) | ADR-002 |
| U-2 | `Application::run` returns on Linux after `cx.quit()` | ADR-010, open question 7 |
| U-3 | KWin causes a render when a minimized GPUI window is restored, without a `window.refresh()` | ADR-010, open question 9 |
| U-4 | Headless wgpu screenshots work with gpui-kit's `HeadlessAppContext` on Mesa lavapipe | ADR-013, open question 8 |
| U-5 | GPUI has no screen reader support on Linux | ADR-009 |
| U-6 | Wayland focus-stealing prevention only flashes the taskbar entry on second-instance activation | ADR-010 |
| U-7 | Bundled Noto Sans static faces contain `tnum` and load under zed#61660; Inter is the alternative | ADR-009, open question 5 |
| U-8 | Exact gpui-kit `ThemeColor` token names match ADR-009's mapping | ADR-009, open question 11 |
| U-9 | The zed#64499 patch outline fixes the frame stall | ADR-010, open question 12 |
| U-10 | Collection cost gates: 2% (Performance tab) and 3.5% (Processes tab) are reachable; 15 ms sweep for 2,000 processes | ADR-014, ADR-016 |
| U-11 | History budget: about 200 changed processes per second, 15-byte patches, 3:1 zstd, about 30 MB per 6 h | ADR-012 |
| U-12 | NVML per-process utilization (`nvmlDeviceGetProcessUtilization`) works unprivileged on driver 615 | r2 PROC-023, r2 conflicts 9 |
| U-13 | KWin 6.7.5 scripts cannot receive D-Bus signals, so commands must be one-shot scripts (kdotool pattern) | r2 PROC-059, conflicts 8 |
| U-14 | `org.kde.KWin.queryWindowInfo()` may already return `pid`; the script path is needed only if it does not | APP-006 |
| U-15 | GPUI's `prompt_for_paths` (portal FileChooser) pulls in no Tokio | r2 PROC-079, v1 PROC-091 |
| U-16 | `kglobalaccel` `setForeignShortcutKeys` sticks for a `.desktop` launch component | APP-051 |
| U-17 | TaskSlinger `TS1.` theme share-code encoding (needs one code generated under Wine) | APP-038 |
| U-18 | TaskSlinger's 17 accent hex values and the 11 non-default theme palettes (to be read from `themes.json` after "Create copy" under Wine) | APP-035, APP-039 |
| U-19 | Steam `reaper` argv carries `AppId=<n>`; Lutris and Heroic set `LUTRIS_GAME_UUID` / `HEROIC_APP_NAME` | r2 PROC-009 |
| U-20 | zswap pool size is readable only through debugfs | r2 PROC-010 |
| U-21 | Interrupt time is never also in task `stime` when `CONFIG_IRQ_TIME_ACCOUNTING=y` (verified set here; the double-count claim is inferred) | add PROC-100 |
| U-22 | Process history store estimate (about 100 active processes × 18 B × 21,600 s) | r2 PROC-074 |
| U-23 | socket inode map rebuild under 20 ms on this machine | SYS-032 |
| U-24 | `OOMScoreAdjust=-900` is accepted for a user unit | r2 PROC-081 |
| U-25 | udisks2 2.10 NVMe module is present for NVMe SMART | PERF-046 |
| U-26 | The sysfs DRM connector `enabled` file is wrong on NVIDIA 615 (withdrawn claim; KScreen agreed with sysfs during the check; confirm whether all three monitors were on) | PERF-065, PERF addendum |
| U-27 | Readability of `/sys/kernel/security/lockdown`, efivars SecureBoot and `/sys/class/iommu` without root | PERF addendum, Security features |
| U-28 | `cpuinfo_avg_freq` and `amd_pstate_prefcore_ranking` exist per policy (reported present; recheck in the fixture) | PERF-025, PERF-028, PERF-035 |
| U-29 | `/dev/dri/card1` seat ACL allows the KMS read-only ioctls | PERF-065 |
| U-30 | `ureq` 3 with rustls has no Tokio in its graph | PERF-054, APP-056 |
| U-31 | ksni 0.3.6 with `async-io` and `blocking` features has no Tokio in its graph | ADR-010, PERF-078 |
| U-32 | `x11rb` XRes queries work against XWayland for the "X11 resources" column | r2 PROC-025 |
| U-33 | Wayland `wl_data_device` clipboard writes work from GPUI for all copy actions | v1 PROC-058 |
| U-34 | `fstatat` `st_size` of `/proc/<pid>/fd` gives the fd count on kernel 7.2 | r2 PROC-001 |
| U-35 | Hardware peak reporting: GPU power-limit and fan max values needed for fixed scales are reported by NVML on this card | PERF-015, PERF-061 |
| U-36 | CSD checklist passes on KWin (drag, double-click maximize, restore, resize edges, window menu, tiling); fallback to server decorations if not | ADR-010, D-006 |
| U-37 | Mission Center's `99-powercap.rules` keeps `energy_uj` readable after reboot (so the tasklens rule is only needed on other machines) | PERF-095 |
| U-38 | Columns ids 2, 4, 5, 8-12, 14, 15, 19-32, 34-61, 84-95 of TaskSlinger have no names tasklens lacks (string ids make this moot) | add PROC-028 |

