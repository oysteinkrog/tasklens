# tasklens reconciliation record

This file settles every contradiction found between `docs/DECISIONS.md`, `docs/ARCHITECTURE.md` (ADR-001 to ADR-016) and the feature specs (`spec/features-proc.md` v1, `spec/features-proc-r2.md`, `spec/features-proc-addendum.md`, `spec/features-perf.md`, `spec/features-perf-addendum.md`, `spec/features-sys-app.md`). Where this file speaks, it overrides those documents. Where it is silent, the precedence order is: DECISIONS.md, then ARCHITECTURE.md on engineering structure, then TaskSlinger for look, labels and defaults, then Mission Center for Linux data correctness, then verified machine facts over inferred claims. Among the three PROC files, r2 wins on a topic it covers, except where the addendum's later and more specific text or a decision says otherwise; v1-only topics stay.

Labels: "verified" means read in a document or checked on the machine; "inferred" means decided here from the rules because no document settles it.

PROC IDs below are prefixed `v1`, `r2` or `add` to say which file they come from. In R-001 to R-074, the owner questions, the canonical tables and the unverified list, feature IDs are those of the old specs; `[now X]` after an old ID names the entry in the merged specs (`spec/merged/proc.md`, `spec/merged/perf.md`, `spec/merged/sys-app.md`). From R-075 on, a feature ID without a `v1`, `r2`, `add` or `old` prefix is a merged spec ID.

## Resolutions

### R-001 Application ID, D-Bus names and object paths
- Sources in conflict: v1 PROC-003/PROC-030 [now PROC-076, PROC-052] `org.tasklens.helper.process`, `org.tasklens.Tasklens`, `/KWinBridge`; r2 helper note and PROC-059/064 [now PROC-052, PROC-076] `org.tasklens.helper`, `org.tasklens.App`, `/WindowBridge`, policy file `org.tasklens.policy`; PERF-094 [now PERF-105] `io.tasklens.setup`; ADR-001/006/010, APP-001/003/011, SYS-013/032/033 [now SYS-013, SYS-030, SYS-031] `io.github.oysteinkrog.*`.
- Resolution: app ID and session bus name `io.github.oysteinkrog.Tasklens`. Object path `/io/github/oysteinkrog/Tasklens` serves `org.freedesktop.Application`. The KWin bridge object is `/io/github/oysteinkrog/Tasklens/WindowBridge`, interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s)`. Polkit action prefix `io.github.oysteinkrog.tasklens.` (lower-case `tasklens`, as D-005 writes it). Policy file `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy` (ADR-001 packaging name). Desktop file `io.github.oysteinkrog.Tasklens.desktop`.
- Rule: D-005.
- Edits: v1 PROC-003 [now PROC-076] helper line and PROC-030 [now PROC-052]; r2 helper note, PROC-059 [now PROC-052], PROC-064 [now PROC-076]; PERF-094 [now PERF-105] action name. ADR-001 unchanged.

### R-002 Polkit action set
- Sources in conflict: ADR-006 one action `…tasklens.helper`; SYS-013 `…tasklens.service-dropin`; SYS-032/039 [now SYS-030, SYS-037] `…tasklens.inspect`; SYS-033 [now SYS-031] `…tasklens.close-connection`; PERF-094 [now PERF-105] setup action; APP-049 passwordless rule for the whole prefix.
- Resolution: three actions only. `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`) covers every tl-helper operation, including reads, SOCK_DESTROY and service drop-ins. `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`) covers `/usr/lib/tasklens/tasklens-setup`. `io.github.oysteinkrog.tasklens.netmon` is reserved for `tl-netmon`. Reason: polkit authorises the `pkexec` launch of the helper process once; it cannot authorise individual operations inside a running session, so per-operation action names would not be enforced (inferred from how pkexec works).
- Rule: ADR-006 wins on the privilege boundary.
- Edits: SYS-013, SYS-032 [now SYS-030], SYS-033 [now SYS-031], SYS-039 [now SYS-037], PERF-094 [now PERF-105], APP-049 (rule text stays prefix-based).

### R-003 Helper transport, flags and idle exit
- Sources in conflict: ADR-006 stdin/stdout framing, 300 s idle, no flag; v1 and r2 stdin/stdout, r2 `--session`, 5 min idle, r2 PROC-064 [now PROC-076] also `--once` single calls in P2; APP-003 "inherited socketpair", 15 min idle.
- Resolution: transport is the helper's stdin and stdout (a socketpair, so a file descriptor can travel with the Dump request, R-145) with `tl-model::wire` framing (max frame 64 KiB). Start command `pkexec /usr/lib/tasklens/tl-helper --session`. Idle exit after 300 s or when stdin reaches EOF. No `--once` mode: privileged actions exist only from the helper phase (P3). Replies larger than one frame are sent as a sequence of frames; the total per request is capped at 4 MiB (v1 PROC-003 [now PROC-076]).
- Rule: ADR-006 wins; `--session` is a harmless addition.
- Edits: APP-003 (socketpair, 15 min); r2 PROC-064 [now PROC-076] phase note (`--once`); ADR-006 add `--session` and the multi-frame reply rule.

### R-004 Helper operation allowlist
- Sources in conflict: ADR-006 allows Signal, SetNice, SetAffinity, SetIoPriority, Freeze/Thaw/KillGroup, ReadProcessDetail only. PROC (v1 PROC-003/070/078 [now PROC-076, PROC-075, PROC-084], r2 PROC-064 [now PROC-076]), SYS-013/032/033/039 [now SYS-013, SYS-030, SYS-031, SYS-037] need reads of io, fd list, fdinfo, environ, maps, smaps_rollup, ns, cwd, exe; strings scan; gcore dump; socket inode resolution; SOCK_DESTROY; recovery drop-in write; netns sock_diag dump.
- Resolution: ADR-006's allowlist is extended with exactly: `ReadProcessFiles { key, files: set of {io, fd_list, fdinfo, environ, maps, smaps, smaps_rollup, ns, cwd, exe, status, root} }` (`smaps` per R-098, `root` per R-157); `ScanStrings { key, opts }` (streams batches) and `CancelScan { seq }` (R-157); `Dump { key, mode }` (runs `/usr/bin/gdb` by fixed path with `gcore` as a batch command and writes to the file descriptor passed as `SCM_RIGHTS` data with the frame, R-145); `ResolveSocketInodes { inodes }`; `SockDestroy { family, proto, four_tuple, cookie }`; `WriteServiceDropIn { manager: System, unit, keys limited to Restart*, StartLimit*, FailureAction, OnFailure }` to the fixed path `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`; `NetnsSockDiagDump { netns_ino }`. Still no path, command or free-form file argument. The 4 MiB reply cap is per reply message for streamed requests (`ScanStrings`, `NetnsSockDiagDump`) and total for the rest (R-157). The helper re-runs `open_verified` on every process target.
- Rule: rule 2, second clause: the ADR as written makes required features impossible, so the ADR changes.
- Edits: ADR-006 "Privileged helper boundary" list.

### R-005 Default Memory column is RssAnon
- Sources in conflict: D-004 RssAnon; ADR-008 "Memory (RSS)" and ADR-011 `memory_rss`; v1 PROC-020 [now PROC-030] and r2 PROC-020 [now PROC-030] VmRSS; add PROC-098/102 [now PROC-070, PROC-024] "Working set total - Memory", "Memory is RSS"; PERF-088 [now PERF-087] "RSS for the default Memory column".
- Resolution: column id `memory`, header "Memory", value RssAnon from `/proc/<pid>/status`. Header tooltip: "Private memory (RssAnon). Shared and file-backed pages are not counted." Picker alias `Working set private - "Memory"` (TaskSlinger's own alias). Remove the separate `ws_private` column (it duplicates `memory`). Keep `ws_total` (VmRSS, label "Working set total", tooltip "Resident set size (VmRSS)"), `pss`, `uss`, `swap`. App and unit rows: cgroup `memory.stat` `anon`, tooltip "Anonymous memory charged to this app's cgroup". Subtree totals: Σ RssAnon. Heat reference: 10% of MemTotal against RssAnon. Copy details lines: `Memory:` RssAnon, `Private bytes:` VmData+VmStk, `Working set:` VmRSS, all whole MiB. Minigraph label for the Memory column: "Memory"; for `ws_total`: "RSS"; for `ws_shared`: "RSS shared". History (ADR-012 `HistProcess`) stores RssAnon in `rss_kib`; rename the field `mem_anon_kib`.
- Rule: D-004.
- Edits: ADR-003 `ProcessRecord` gains `rss_anon_bytes: u64` and `ProcessField::RssAnon` (PROCESS_FIELDS = 13); ADR-008 memory sentence; ADR-011 column list; ADR-012 field; v1/r2 PROC-020 [now PROC-030]; add PROC-098 [now PROC-070], PROC-102 [now PROC-024]; PERF-088 [now PERF-087]; r2 PROC-007 [now PROC-008] (anon only); r2 PROC-015 [now PROC-019].

### R-006 Default visible columns and Power usage
- Sources in conflict: ADR-011 `name,user,pid,cpu,memory_rss,disk_read,disk_write,net,gpu,description`; ADR-016 power hidden; v1 PROC-015 [now PROC-018, PROC-027, PROC-028] and add PROC-106 [now PROC-022] nine columns without Power; r2 PROC-016/024 [now PROC-022, PROC-023, PROC-024, PROC-025, PROC-034] Power on when RAPL readable; v1 PROC-022 [now PROC-031] single Disk column.
- Resolution: default visible, in order: `name, user, pid, cpu, memory, disk, network, gpu, description`. `power` and `power_trend` exist and are off by default on every machine. Default widths (logical px at 100%): Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342; Power usage 101 when shown.
- Rule: ADR-016 and the architecture review (no default power column without a per-process source; the column is an estimate). TaskSlinger's single Disk column wins over ADR-011's read/write pair (rule 3).
- Edits: ADR-011 `columns` and `column_widths` example; r2 PROC-016 [now PROC-022, PROC-023, PROC-024, PROC-025] and PROC-024 [now PROC-034] default-set sentences.

### R-007 Column identifiers and labels
- Sources in conflict: v1 gives string ids; r2 gives labels only, some renamed.
- Resolution: v1's string ids are canonical (PROC-015 [now PROC-018, PROC-027, PROC-028] to PROC-027 [now PROC-036] plus add PROC-103 [now PROC-033] `gpu_0..gpu_15`). r2's labels win where they differ, except where TaskSlinger has the label: "Handles" stays "Handles" (tooltip "Open file descriptors"), not "File descriptors". Adopt from r2: Type values "App, Background process, System process, Kernel thread, Unit, Pseudo"; Elevated values "Full (root)", "Capabilities", "Setuid" (euid differs from ruid; added, inferred), "Default"; Sandbox values; "Package", "Packager", "Package status", "Build date"; Session blank for 4294967295; Start time "HH:MM:SS" today else "YYYY-MM-DD HH:MM"; Priority class text with the nice value in the tooltip; Disk = Δ(read_bytes + write_bytes − cancelled_write_bytes)/Δt; GPU = max over adapters of max(SM, encoder, decoder).
- Rule: rule 5 and rule 3.
- Edits: v1 PROC-015/017/018/021/022/024/026 [now PROC-018, PROC-027, PROC-028, PROC-029, PROC-031, PROC-033, PROC-035] label columns; the merge agent carries ids into r2's tables.

### R-008 Command-line read and display limits
- Sources in conflict: v1 PROC-002/015 [now PROC-002, PROC-018, PROC-027, PROC-028] 64 KiB; r2 PROC-001/017 [now PROC-002, PROC-027] 4 KiB in the table, full on demand; ADR-015 32 KiB.
- Resolution: read limit 32 KiB with `…` appended (ADR-015). Table cell and hover tooltip show the first 512 characters; Properties shows the full 32 KiB.
- Rule: ADR-015.
- Edits: v1 PROC-002, PROC-015 [now PROC-018, PROC-027, PROC-028]; r2 PROC-001 [now PROC-002], PROC-017 [now PROC-027].

### R-009 Row hover tooltip format
- Sources in conflict: add PROC-032 amendment [now PROC-039] (TaskSlinger line keys, cmdline first 512 chars); r2 PROC-013 [now PROC-039] ("path:", "user:", "Unit:", "Command line:" 8 lines, "Package:").
- Resolution: the addendum's block, plus r2's `Package: <name> <version>` line after `cgroup:` when known:
  `<command line, first 512 chars>` / `path: <exe or "restricted">` / `user: <name> (uid N)` / `Unit: <unit> (<Description>)` / `cgroup: <path>` / `Package: …` / `Desktop file: …` (app rows) / `Some values need administrator access.` (restricted rows).
- Rule: addendum later and more specific; r2 line added because it does not contradict.
- Edits: r2 PROC-013 [now PROC-039].

### R-010 View mode model, menu and cycle order
- Sources in conflict: ADR-011 `processes.mode = grouped|tree|flat`; r2 PROC-002 [now PROC-014, PROC-015] one enum, three radio items, default Grouping; add PROC-004 amendment [now PROC-014 (Dropped: R-010)] (radio pair plus check), add PROC-105 [now PROC-015] cycle Tree, Grouping, List; APP-005 cycle Grouping, Tree, List; APP-032 and PROC-095 [now PROC-099] two booleans.
- Resolution: one setting `processes.mode` with values `grouped | tree | flat`, default `grouped`. View > Process View shows three radio items "Tree View", "Task Manager Grouping", "List View". The toolbar button cycles Tree View, then Task Manager Grouping, then List View (TaskSlinger string order). Toast "View mode: <name>". Tooltip "Toggle view mode (current: <name>)".
- Rule: ADR-011 on the data model; TaskSlinger string order on the cycle (rule 3).
- Edits: APP-005, APP-032 (`tree_view`, `grouping` keys), PROC-095 [now PROC-099] (same keys), add PROC-105 [now PROC-015] (two-boolean mapping), add PROC-004 amendment [now PROC-014 (Dropped: R-010)] (drop).

### R-011 Default sort
- Sources in conflict: ADR-011 `cpu desc`; v1 PROC-037 [now PROC-047], r2 PROC-035 [now PROC-047], APP-032 PID ascending.
- Resolution: processes PID ascending; history CPU descending; services Name ascending; startup Status ascending then Name; connections Proto ascending then local port.
- Rule: rule 3 (TaskSlinger INI `Sort.ProcessesKey=3`, verified in the inventory).
- Edits: ADR-011 `sort` default.

### R-012 Ctrl+C and the copy commands
- Sources in conflict: ADR-015 "Copy" = TSV of visible columns; add PROC-098 [now PROC-070] Ctrl+C = Copy details (fixed block), Ctrl+Shift+C = "Copy rows as table" (TSV); APP-031 Ctrl+C = TSV in all tables; SYS-009 Ctrl+C = TSV.
- Resolution: in every table, Ctrl+C runs "Copy row" (processes: menu label "Copy details"; services, startup, connections: "Copy row"), which copies the fixed block from add PROC-098 [now PROC-070], SYS-009, SYS-026 [now SYS-025] or SYS-034 [now SYS-032]. Ctrl+Shift+C runs "Copy rows as table": TSV of the visible columns with a header line. "Copy" copies the clicked cell. "Copy all metrics" copies every column with data as `Label: value` lines, ending with `Command line: …`. On the Performance tab Ctrl+C copies the device info (PERF-010 [now PERF-011]). Command lines appear only in "Copy all metrics" and only for rows the user selected; this is the deliberate export ADR-015 allows.
- Rule: addendum later and more specific; applied to the other tables for consistency (inferred).
- Edits: ADR-015 "Exports and copy"; APP-031 Ctrl+C row; SYS-009; r2 PROC-037 [now PROC-049] and PROC-053 [now PROC-070].

### R-013 Number formats
- Sources in conflict: ADR-003 (3 significant digits for memory, no decimal on percent at or above 10, GHz with 2 decimals, durations "1d 4h"); add PROC-104 [now PROC-003] and r2 PROC-027 [now PROC-003] (percent "%.1f%%", memory 2 decimals, counts thin-space grouped); PERF-022 [now PERF-025] Mission Center rule (2 decimals below 10, 1 below 100); PERF-001 "4631 MHz"; PERF-080 [now PERF-034] uptime "2 days, 2h 38m 15s".
- Resolution: see canonical table (d). In short: percent one decimal everywhere, "0%" at measured zero; memory and storage IEC with 2 decimals, "512 B" below 1 KiB; rates IEC MiB/s with 1 decimal in tables, auto-scaled with 1-2-5 axis rounding on graphs; frequencies integer MHz; uptime in the TaskSlinger form; the short "4h 12m" form only inside tooltips and the Start time column's age text.
- Rule: rule 3 (TaskSlinger density and strings).
- Edits: ADR-003 units list; PERF-022 [now PERF-025] number-format line; PERF-025 [now PERF-028] tooltip "4.2 GHz" becomes "4200 MHz".

### R-014 Disk and I/O rate unit labels
- Sources in conflict: r2 PROC-021/027 [now PROC-031, PROC-003], PROC-029 [now PROC-040], PROC-077 [now PROC-090] "MB/s"; add PROC-104 [now PROC-003], ADR-003, PERF-022 [now PERF-025] "MiB/s".
- Resolution: "MiB/s" and "KiB/s" everywhere, base 1024. Heat reference 100 MiB/s. Top-3 hover "Disk: 4.2 MiB/s".
- Rule: ADR-003 and the review's IEC rule; the addendum agrees.
- Edits: r2 PROC-021 [now PROC-031], PROC-027 [now PROC-003], PROC-029 [now PROC-040], PROC-077 [now PROC-090]; v1 PROC-033 [now PROC-040], PROC-084 [now PROC-090].

### R-015 Network units and auto-scale default
- Sources in conflict: PERF-022 [now PERF-025] auto-scale on, memory and disk base choices; APP-032 `units.network_auto_scale=false`, `network_base2`, `memory_*`, `disk_*`; ADR-003 `[units] network = bits|bytes` only; TaskSlinger INI `NetworkRateAutoScale=0` (verified).
- Resolution: `[units] network = "bits"` (bits are SI base 1000: Kbps, Mbps, Gbps; bytes are IEC: KiB/s, MiB/s). `[units] network_auto_scale = false`. With auto-scale off, values are fixed Mbps (or MiB/s) with one decimal, "0 Mbps" at zero. Graph axis maxima still use PERF-016 [now PERF-019] rounding regardless of this setting. No base-10 options for memory or disk; those Mission Center settings are dropped.
- Rule: rule 3 on the default (TaskSlinger 0); ADR-003 on the setting shape.
- Edits: PERF-022 [now PERF-025]; APP-032 units rows; add PROC-104 [now PROC-003] "with auto-scale on" wording stays as the on-case.

### R-016 Graph time ranges and ring capacity
- Sources in conflict: ADR-003/009/011 ranges 15 s and 60 s, one 602-sample ring; PERF-013 [now PERF-016] 30 s, 1, 5, 10 min with one Performance-wide range; APP-032 `graphs.time_range_s` 30, 60, 120, 300, 600; TaskSlinger minigraph ranges 30 s, 1 min, 5 min, 10 min (inventory, verified).
- Resolution: choices are 30 s, 1 min, 5 min, 10 min; default 1 min. Phase 1 ships 30 s and 1 min; 5 and 10 min arrive in P2 with the 1 Hz tier. Drop 15 s and 2 min. One setting `performance.graph_range_s` for the Performance tab (values 30, 60, 300, 600); each minigraph card keeps its own range. Rings: every series has a fast tier of capacity `ceil(60 s / period) + 2` (602 at 10 Hz) and, from P2, a 1 Hz tier of 602 samples (10 min + 2) whose value is the per-second maximum of the fast tier (same rule ADR-012 uses for history). `Sample` stays 16 bytes; no per-sample min/max pair. Memory about 1.5 MB for 80 series.
- Rule: rule 3 on the user-visible choices; ADR-003 structure kept and extended.
- Edits: ADR-003 ring paragraph; ADR-009 `ChartSpec.range_ns` comment; ADR-011 `graph_range_s`; PERF-013/014 [now PERF-016, PERF-017] (phases match); APP-032 `graphs.time_range_s` (rename and value list).

### R-017 Motion and animation settings
- Sources in conflict: ADR-011 one key `appearance.animations` covering "Smooth graphs" and motion; APP-032 `motion.animations`, `motion.smooth_graphs`, `motion.smooth_scroll`, `motion.smooth_scroll_amount`, `motion.tab_switch_ms`; TaskSlinger has separate INI keys (verified).
- Resolution: separate keys in a `[motion]` section as APP-032 lists them (table a). "Smooth graphs" alone decides chart sliding; "Animations" decides tab, theme and toast motion; system reduce-motion overrides all of them to off.
- Rule: rule 3 (TaskSlinger has the separate settings).
- Edits: ADR-011 schema; ADR-009 animation driver text ("setting Smooth graphs").

### R-018 Settings files, format details and corrupt-file handling
- Sources in conflict: ADR-011 `settings.toml` plus `window.toml`, user themes `themes/*.json`, `schema_version`, corrupt file renamed `.corrupt-<unix>`, toast text, no `directories` crate; APP-033 `settings.toml`, `state.toml`, `themes.json`, `version`, `.bad-<unix>`, different toast, `directories`, inotify reload, unknown keys kept; PROC-095 [now PROC-099] everything under `[processes]` in settings.toml including sort, columns and `history_mode`.
- Resolution: two TOML files. `$XDG_CONFIG_HOME/tasklens/settings.toml` holds user choices. `$XDG_STATE_HOME/tasklens/state.toml` holds window size and maximized, sort per table, columns per table, group expand state, last tab, Performance nav width and selected device, `history_mode`, minigraph layout, tray graphs. `window.toml` is dropped (its content moves to `[window]` in `state.toml`). User themes live in `$XDG_CONFIG_HOME/tasklens/themes.json` in TaskSlinger's `{"themes":[…]}` format; built-ins ship as `assets/themes/builtin.json`. Both TOML files start with `schema_version = 1`. Load, migrate, clamp, atomic save and the read-only rule for newer files follow ADR-011. A corrupt file is renamed `<name>.corrupt-<unix seconds>` and the toast reads "Settings file was unreadable and has been reset. The old file was kept as {name}". Unknown keys are ignored and not preserved. No `directories` crate. No inotify on the settings file; external edits apply at the next start.
- Rule: ADR-011 on structure; APP-033's split into settings and state adopted because ADR-011 already separates window state.
- Edits: ADR-011 paths table and ownership text; ADR-009 theme path; APP-033; PROC-095 [now PROC-099] (state keys move).

### R-019 Settings key names
- Sources in conflict: three different key sets (ADR-011, APP-032, PROC-095 [now PROC-099]) plus PERF keys.
- Resolution: canonical table (a) is the only settings schema. Merge agents replace every key named in a spec with the table's name. Keys that no feature needs are dropped there with a reason.
- Rule: ADR-011 naming style (short section names, snake_case ids).
- Edits: APP-032, PROC-095 [now PROC-099], PERF-002/003/004/069/073 [now PERF-002, PERF-003, PERF-004, PERF-077, PERF-081], ADR-011.

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
- Sources in conflict: D-003 off by default, 6 h; ADR-011/012 1..=24 h, `max_disk_mb = 256`, 16 MB RAM, segments per boot `seg-<boottime>.tlh`; v1 PROC-080/081 [now PROC-086, PROC-087] and r2 PROC-074/078 [now PROC-086, PROC-087] 1..=12 h, 64 MiB RAM cap, 10-minute uncompressed window, `YYYYmmdd-HHMM.tlh`, "Save history to disk" setting, r2 shows a 10-minute rolling window when recording is off; APP-022 menu 1, 2, 3, 4, 6, 8, 12 hours.
- Resolution: `history.enabled = false`; `history.retention_h = 6`, range 1..=12; menu offers 1, 2, 3, 4, 6, 8, 12. `history.max_disk_mb = 256` (not shown in menus). `history.persist = true`: when false the recorder deletes its segments at shutdown and loads nothing at start; the on-disk format is unchanged. Storage format, RAM budget (16 MB), directory layout and segment names follow ADR-012. With recording off, History mode shows the "History recording is off" toast with an "Enable" button and no data; the r2 10-minute rolling window is dropped. Load error toasts from r2 PROC-078 [now PROC-087] stay; load time budget 2 s.
- Rule: D-003; ADR-012 on storage; TaskSlinger on the 1..=12 range (rule 3).
- Edits: ADR-011 `retention_h` range; ADR-012 retention sentence; v1 PROC-080/081 [now PROC-086, PROC-087]; r2 PROC-074/078 [now PROC-086, PROC-087]; APP-032.

### R-026 GPU sample rate
- Sources in conflict: ADR-007 device totals at 1 Hz, never faster; PERF-056 [now PERF-062] 500 ms when Refresh is under 1 s.
- Resolution: GPU totals and GPU process data at 1 Hz. The Refresh setting does not affect the probe.
- Rule: ADR-007.
- Edits: PERF-056 [now PERF-062]; PERF conflicts item 2.

### R-027 Tray implementation, icon sizes and menus
- Sources in conflict: ADR-010 `ksni =0.3.6`, sizes 22 and 44, menu "Show tasklens, graph choice, Quit", data from a 1 Hz GPUI timer in `tl-tray`; APP-047 hand-written SNI in the tl-engine zbus adapter, 22 and 44; PERF-078 [now PERF-088] ksni if no Tokio else hand-written, sizes 22, 32, 44; PERF-080/089 [now PERF-090] detailed menus.
- Resolution: `tl-tray` with `ksni =0.3.6` as ADR-010 says; `cargo xtask check-deps` proves no Tokio. If that check fails, the fallback is a hand-written SNI and dbusmenu server inside `tl-tray` on its own zbus connection, never inside tl-engine. Icon sizes 22 and 44 px. Tray graph icon menu (PERF-089 [now PERF-090] order): Graph color >, Time range >, Close tray icon graph, Tray icon graphs >, separator, Show Main Window, Show mini window (P3), Exit tasklens. Main tray icon menu (APP-047): Show Main Window, Tray icon graphs >, separator, Exit tasklens. Left click toggles the main window. Updates at most 1 Hz and only when pixels changed. Re-register on `StatusNotifierWatcher` owner change (PERF-090 [now PERF-092]).
- Rule: ADR-010 on structure; TaskSlinger menus (rule 3).
- Edits: ADR-010 tray paragraph (menus); APP-047 mechanism; PERF-078 [now PERF-088] sizes.

### R-028 Additional setup: panel, not first-run dialog
- Sources in conflict: PERF-094 [now PERF-105] on-demand panel under Settings > Monitoring, no first-run dialog; APP-042 first-run dialog gated by `state.first_run_done`, reopened from Help; APP-024 Help item.
- Resolution: no first-run dialog. One window titled "tasklens Additional Setup", opened from Help > "Additional setup…", from Settings > All settings > Privileges, and from every "Needs setup" link. Rows: "CPU power draw" (RAPL rule, PERF-095 [now PERF-106]), "Journal access" (shows `sudo usermod -aG systemd-journal $USER` with Copy; no automatic change), "Administrator helper" (Installed / Not installed), "Fan sensors" (PERF-097 [now PERF-108], P4), "Per-process network" (text only, PERF-096 [now PERF-107]). Each row has status text, Enable or Revert, "View script" and "Copy command". Script `/usr/lib/tasklens/tasklens-setup` with subcommands `rapl-enable`, `rapl-revert`, `sensors-detect`, `sensors-revert` (P4, R-149), `polkit-rule on|off`. `state.first_run_done` is dropped.
- Rule: rule 3 (TaskSlinger has no first-run dialog); PERF-094 [now PERF-105] is the more complete design.
- Edits: APP-042 (rewrite to point at PERF-094 [now PERF-105]), APP-032 state row, PERF-094 [now PERF-105] title and location.

### R-029 RAPL udev rule file
- Sources in conflict: PERF-095 [now PERF-106] `60-tasklens-powercap.rules`; r2 PROC-024 [now PROC-034] and APP-042 `99-tasklens-powercap.rules`; existing `/etc/udev/rules.d/99-powercap.rules` from Mission Center (verified present).
- Resolution: `/etc/udev/rules.d/99-tasklens-powercap.rules` with one line: `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`. `rapl-enable` writes it, then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add`. Status "Enabled (existing rule)" when `energy_uj` is already readable and the tasklens file is absent; tasklens never touches `99-powercap.rules`. The warning text from PERF-095 [now PERF-106] is shown before enabling.
- Rule: two of three documents agree; a 99- prefix sorts after distro rules (inferred).
- Edits: PERF-095 [now PERF-106] file name.

### R-030 DIMM serial numbers
- Sources in conflict: PERF-041 [now PERF-046] key list includes `SERIAL_NUMBER`; PERF-088 [now PERF-099] Information shows "Serial number"; PERF addendum PERF-085 [now PERF-046] hides serials.
- Resolution: serials are not shown on the Memory page, the Information tab or in any copy text. The udev key is not read. See owner question Q-1 if Oystein wants them.
- Rule: addendum is later and more specific; it also protects copy and screenshot output.
- Edits: PERF-041 [now PERF-046], PERF-088 [now PERF-099].

### R-031 Hide system processes: wording, key and scope
- Sources in conflict: v1 PROC-012 [now PROC-046] and r2 PROC-034 [now PROC-046] tooltip "Hide system background processes"; add PROC-012 amendment [now PROC-046] and APP-004 "Hide system processes" / "Show system processes"; keys `processes.hide_system_processes` (APP-032) vs `hide_system` (PROC-095 [now PROC-099]).
- Resolution: tooltip "Hide system processes" / "Show system processes"; toasts "Hiding system processes" / "Showing system processes". Key `processes.hide_system = false`. Scope per r2 PROC-034 [now PROC-046] plus pseudo rows (add PROC-100 [now PROC-012]). The button exists on the Processes tab only, removed (not greyed) on other tabs; in History mode it stays active.
- Rule: addendum later; key name by ADR style.
- Edits: v1 PROC-012 [now PROC-046], r2 PROC-034 [now PROC-046] tooltip text; APP-032 key.

### R-032 Pseudo-process rows
- Sources in conflict: r2 PROC-010 [now PROC-012] three rows (System Idle Process, System interrupts, Memory compression), toggle "Show pseudo processes" default on, Type "Pseudo"; add PROC-100 [now PROC-012] two rows ("Interrupts", "Memory compression (zram)"), no idle row, no toggle, Type "Kernel".
- Resolution: add PROC-100 [now PROC-012] wins: two rows, names "Interrupts" and "Memory compression (zram)", no extra toggle, hidden by Hide system processes, excluded from the process count, menu Copy, Copy details, Copy all metrics, Add Minigraph only. Type column shows "Pseudo" (r2's value list, R-007). Phase P2.
- Rule: addendum later and more specific; it also uses the verified `CONFIG_IRQ_TIME_ACCOUNTING=y` fact.
- Edits: r2 PROC-010 [now PROC-012]; r2 PROC-017 [now PROC-027] Type list stays.

### R-033 Row context menu order and labels
- Sources in conflict: v1 PROC-049 [now PROC-057] (with add amendments); r2 PROC-041 [now PROC-057]; app-row labels "Force End App" (v1, add PROC-096 [now PROC-061]) vs "Kill App (all processes)" (r2); "Suspend App/Resume App" (v1) vs "Freeze App/Thaw App" (r2); Set Affinity submenu (v1) vs dialog "Set Affinity…" (r2).
- Resolution: r2 PROC-041 [now PROC-057] order, with "Copy details" inserted between "Copy" and "Copy all metrics". App and unit rows: "End App", "Freeze App" / "Thaw App", "Kill App (all processes)". "Set Affinity…" opens the r2 PROC-050 [now PROC-067] dialog. Pseudo rows and exited rows per R-032 and r2.
- Rule: r2 wins where it covers; addendum adds the one item it introduced.
- Edits: v1 PROC-049 [now PROC-057] (superseded); add PROC-096 [now PROC-061] app-row sentence ("Force End App" becomes "Kill App (all processes)").

### R-034 Multi-select labels
- Sources in conflict: add PROC-096 [now PROC-061] "End %d Processes"; r2 PROC-041 [now PROC-057] "End Selected Processes".
- Resolution: "End Selected Tasks", "End Selected Processes", "Suspend Selected Tasks", "Resume Selected Tasks". Toasts per add PROC-097 [now PROC-069] ("Selected process priority updated." and the rest).
- Rule: r2 pattern kept for consistency (inferred; TaskSlinger evidences only the toasts, not the labels).
- Edits: add PROC-096 [now PROC-061].

### R-035 Action toasts and identity-change text
- Sources in conflict: v1 PROC-050/051 [now PROC-058, PROC-060, PROC-061, PROC-064] toasts; r2 PROC-042/043/044 [now PROC-058, PROC-060, PROC-061]; identity-change text capitalised (v1, TaskSlinger) vs lower-case (r2).
- Resolution: r2 PROC-042 [now PROC-058] to PROC-044 [now PROC-061] toasts. Identity mismatch toast is TaskSlinger's exact text: "The selected process changed; wait for the list to update and try again." EPERM toast "Permission denied" with button "Retry as administrator" (hidden until the helper phase ships; before P3 the toast has no button).
- Rule: r2 wins; TaskSlinger text wins on the quoted string.
- Edits: r2 PROC-042 [now PROC-058] string; v1 PROC-050/051 [now PROC-058, PROC-060, PROC-061, PROC-064] superseded.

### R-036 Confirmations and protected processes
- Sources in conflict: ADR-006 PROTECTED set and "Force end and End process tree on anything open an AlertDialog"; r2 PROC-052 [now PROC-059] (confirmations for session-critical, End Process Tree, Kill App, multi-select End of more than 1, any action on another user's process; refusals for tasklens, PID 1, kernel threads; adds sddm, login, systemd-logind to the critical list); r2 PROC-044 [now PROC-061] no confirmation for single End Process.
- Resolution: PROTECTED set = ADR-006's list plus `sddm`, `systemd-logind` and the session's `login` ancestor. Refused with toast: tasklens itself ("Refusing to end tasklens itself."), PID 1 ("PID 1 (systemd) cannot be ended."), kernel threads ("Kernel threads cannot be signalled."). Confirmation dialog for: any signal, freeze or kill on a PROTECTED target ("Ending <name> will end your desktop session. End it anyway?"); End Process Tree ("End N processes in this tree?", buttons End tree / Cancel); Kill App ("Kill all N processes of <App>?"); End Task or End Process with more than one target; any action on another user's process. No confirmation for a single End Process (SIGKILL) on the user's own non-protected process.
- Rule: rule 3 (TaskSlinger shows a toast, not a dialog, for End Process); ADR-006 protected set kept.
- Edits: ADR-006 "Protected processes" paragraph.

### R-037 End Process Tree order
- Sources in conflict: ADR-006 and v1 PROC-052 [now PROC-062] SIGTERM deepest first; r2 PROC-045 [now PROC-062] open all pidfds, SIGSTOP all, SIGKILL all.
- Resolution: r2's order. The engine opens a verified pidfd for every captured target, sends SIGSTOP to all, then SIGKILL to all, then closes the fds; one event per target. `ActionKind` gains `KillTree` so the reducer can order the two passes.
- Rule: r2 wins on topic; the stop-then-kill order prevents fork races, which ADR-006's goal (race-free actions) supports.
- Edits: ADR-005 `ActionKind`; ADR-006 "Tree operations"; v1 PROC-052 [now PROC-062].

### R-038 Set Priority, scheduling policy and the action types
- Sources in conflict: ADR-005 `SetNice(i8)` only; v1 PROC-057 [now PROC-066] and r2 PROC-049 [now PROC-066] Custom dialog with policy and RT priority, SCHED_RR 1 for Realtime, Idle item, confirmation text; phases v1 P2 vs r2 P1 for raising nice.
- Resolution: r2 PROC-049 [now PROC-066] behaviour and phases (P1 nice classes; P2 Custom, Realtime, Idle). `ActionKind` gains `SetSched { policy: SchedPolicy, rt_prio: u8 }`; tl-ops implements it with `sched_setattr` per TID under the same pidfd check as `SetNice`. Realtime sets SCHED_RR priority 1 with `SCHED_RESET_ON_FORK` after the confirmation "Realtime priority can freeze the desktop if the process spins. Set it anyway?".
- Rule: r2 wins; ADR-005 extended because the feature is required.
- Edits: ADR-005 enum; ADR-006 details list; v1 PROC-057 [now PROC-066].

### R-039 Set Affinity and Send Signal phases
- Sources in conflict: v1 Set Affinity P2, r2 P1; v1 Send Signal P1, r2 P2; add PROC-097 [now PROC-069] multi-select affinity P2.
- Resolution: Set Affinity dialog P1 for one target; multi-select mixed marks P2. Send Signal submenu P2 with r2's top-level list (TERM, KILL, HUP, INT, QUIT, STOP, CONT, USR1, USR2), "Other signals >", "Real-time signal…", suffix "(ends process)", toast "Sent SIGHUP to firefox (1234)".
- Rule: r2 wins.
- Edits: v1 PROC-056 [now PROC-065], PROC-059 [now PROC-067] phases.

### R-040 Suspend and resume processes vs apps
- Sources in conflict: v1 PROC-053/054 [now PROC-063, PROC-064] "Suspend App/Resume App" with toasts "App suspended."; r2 PROC-047/048 [now PROC-063, PROC-064] Freeze App / Thaw App, toasts "App frozen", "App thawed", "App killed".
- Resolution: r2.
- Rule: r2 wins.
- Edits: v1 PROC-054 [now PROC-064] superseded.

### R-041 New and exited linger time
- Sources in conflict: ADR-003 flags "within the last 2 s", ADR-004 reducer removes exited records 2 s after exit, ADR-009 highlight 2 s; v1 PROC-036 [now PROC-043] max(1 s, 2 intervals) capped 3 s; r2 PROC-031 [now PROC-043] 2 refresh intervals clamped to 1..3 s.
- Resolution: linger = 2 × `processes.interval_ms`, clamped to 1..=3 s. The reducer keeps exited records for the linger time, then removes them. Fade over the last 300 ms unless reduced motion.
- Rule: r2 wins on behaviour; ADR constants follow.
- Edits: ADR-003 `ProcFlags` comments; ADR-004 "exited records are removed" sentence; ADR-009 highlight bullet.

### R-042 Ctrl-pause focus condition
- Sources in conflict: ADR-009 "while Ctrl is held and the table has focus"; r2 PROC-038 [now PROC-050] and APP-031 "while the main window has focus".
- Resolution: pause applies while Ctrl is held and the main window is active, whatever has focus inside it. Released on Ctrl-up or window deactivation.
- Rule: rule 3 (TaskSlinger behaviour as both specs describe it).
- Edits: ADR-009 Ctrl-pause bullet.

### R-043 Filter box: fields, Esc and the `filter_cmdline` key
- Sources in conflict: ADR-011 `processes.filter_cmdline = false` (cmdline matched only when set); v1 PROC-041 [now PROC-044] and r2 PROC-032 [now PROC-044] always match the command line; APP-028 first Esc clears, second returns focus; r2 one Esc clears and returns focus.
- Resolution: the filter always matches display name, image name, command line, user, description, app name, unit name, group names, and PID by prefix for numeric input. Drop `filter_cmdline`. One Esc clears the filter and returns focus to the table; a further Esc follows the global Esc rule (APP-031).
- Rule: rule 3 on fields; r2 on Esc (it owns PROC-032 [now PROC-044]).
- Edits: ADR-011 key; ADR-009 `TableQuery.filter` comment; APP-028 Esc sentence.

### R-044 CPU percentage mode
- Sources in conflict: ADR-008/011 setting `process_cpu_mode = machine|core`; APP-032 `processes.cpu_one_core_100`; v1 PROC-019 [now PROC-029] and r2 PROC-019 [now PROC-029] no setting, two columns ("CPU" and "CPU (one core = 100%)").
- Resolution: no setting. Column `cpu` is percent of the whole machine; column `cpu_relative` is labelled "CPU (one core = 100%)". Drop both keys.
- Rule: rule 3 (TaskSlinger has the two columns; the toggle is Mission Center's).
- Edits: ADR-008 process CPU paragraph; ADR-011; APP-032.

### R-045 Keyboard: Enter, End Process names and the merged table
- Sources in conflict: APP-031 Enter toggles expand, "Force end" names, Ctrl+C TSV; v1 PROC-040 [now PROC-049] and r2 PROC-037 [now PROC-049] Enter opens Properties, Space toggles expand; add PROC-096 [now PROC-061] "End Process"; ADR-009 Ctrl+Tab tab switching.
- Resolution: canonical table (b). Enter and Ctrl+I and Alt+Enter open Properties; Space toggles expand. Shift+Delete and Ctrl+X are "End Process". Ctrl+Tab and Ctrl+Shift+Tab switch tabs in addition to Ctrl+1..6 and Alt+1..6. See owner question Q-3 on Enter.
- Rule: PROC owns the process table keys; ADR-009 keys kept.
- Edits: APP-031 rows for Enter, Shift+Delete, Ctrl+C; add Ctrl+Tab and Ctrl+Shift+C rows.

### R-046 KWin bridge: which crate owns it
- Sources in conflict: APP-009 "the KWin scripting channel lives in tl-ui's KDE integration module"; ADR-001 rule 1 forbids zbus in tl-ui; ADR-004 worker table has no KWin worker; PROC-030 [now PROC-052] (v1) and r2 PROC-059 [now PROC-052] describe the bridge without a crate.
- Resolution: the bridge is a tl-engine D-Bus worker `workers/dbus_kwin.rs`, `SourceId::KWin`, phase P2, event-driven. It writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`, loads it as plugin "tasklens-bridge", exports the WindowBridge object (R-001) on the engine's session-bus connection, and publishes `windows: Arc<WindowsSnapshot>` as a new `Snapshot` component (window list keyed by pid: internalId, caption, resourceClass, desktopFileName, minimized, maximized, keepAbove, frameGeometry, windowType). Commands (activate, close, keepAbove, pick) go through `Command::KWin(KWinCommand)` and run as one-shot scripts (r2 PROC-059 [now PROC-052]). Availability: `XDG_CURRENT_DESKTOP` contains KDE and `org.kde.KWin` is on the bus; otherwise the window features are hidden with the toast "Window features need KDE Plasma" (v1 PROC-030 [now PROC-052]). The X11 EWMH fallback in r2 PROC-057/058 [now PROC-054, PROC-055] is dropped for v1 (X11 is best effort per ADR-010).
- Rule: ADR-001 dependency rule; ADR-004 structure extended.
- Edits: ADR-003 `Snapshot`; ADR-004 worker table; APP-009, APP-006, APP-011 mechanism lines; r2 PROC-057/058 [now PROC-054, PROC-055] X11 lines.

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
- Sources in conflict: ADR-003 glyph table for cells (`…`, lock, `n/a`, `-`, `!`); v1 PROC "?" for failed; r2 lock tooltip "Needs administrator rights"; v1 "Needs administrator. Click the shield to grant access."; PERF-079 [now PERF-013] word table for the stats grid ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown").
- Resolution: table cells, header totals and sidebar readouts use ADR-003's glyphs. Performance stats grids, Information rows and Properties key/value fields use PERF-079 [now PERF-013]'s words. Tooltips use ADR-003's texts, with the Restricted tooltip changed to "Needs administrator access. Click the shield to grant it." (before P3: "Needs administrator access."). "Needs setup" is Validity `Unsupported` with a reason tooltip linking to the setup window. "Not reported" is `Unsupported` where the source answers with no value.
- Rule: ADR-003 structure; TaskSlinger words where TaskSlinger has them (rule 3).
- Edits: ADR-003 tooltip text; v1 PROC conventions ("?"); r2 conventions.

### R-051 Stale and suspend thresholds
- Sources in conflict: ADR-008 stale `max(3 × period, 1.5 s)`, suspend when `Δboot − Δmono > 250 ms`; PERF-019 [now PERF-022] stale 3 × period, suspend jump over 2 s; r2 "older than 3 sample periods".
- Resolution: ADR-008 values.
- Rule: ADR-008.
- Edits: PERF-019 [now PERF-022].

### R-052 Which files the collectors read
- Sources in conflict: ADR-016 `/proc/net/dev`, `/proc/diskstats`, device enumeration every 5 s; PERF-050 [now PERF-055] per-interface `statistics/{rx,tx}_bytes`, PERF-043 [now PERF-048] `/sys/block/<d>/stat`, PERF-005 netlink uevent plus 5 s rescan.
- Resolution: ADR-016 sources: one `pread` of `/proc/net/dev` and `/proc/diskstats` per tick; enumeration by rescan every 5 s while demanded. No uevent netlink socket in v1 (may be added in P4 as an optimisation bead).
- Rule: ADR-016.
- Edits: PERF-043 [now PERF-048], PERF-050 [now PERF-055], PERF-005.

### R-053 Virtual and tunnel interfaces
- Sources in conflict: ADR-016 hides `veth*, docker*, br-*, virbr*, tailscale*` by default with key `show_virtual_interfaces = false`; PERF-004/049 [now PERF-004, PERF-054] Tunnel and VPN group on by default (so `tailscale0` shows), Virtual off, with keys `performance.show_network_{wired,wireless,vpn,virtual,other}`.
- Resolution: PERF classification and keys. `tailscale0` is a Tunnel and is shown. `docker0`, `veth*`, `br-*`, `virbr*`, `vnet*` are Virtual and hidden by default. `lo` never shows.
- Rule: PERF is more specific and matches TaskSlinger (it shows Tailscale, PERF-049 [now PERF-054]).
- Edits: ADR-016 network row and key name.

### R-054 GPU page graph layout keys
- Sources in conflict: APP-032 `performance.gpu_graph_middle`, `gpu_graph_bottom` (Mission Center choices); PERF-057 [now PERF-063] fixed rows: engine grid, then Memory and Temperature, then optional Power and Clocks via a checklist.
- Resolution: PERF-057 [now PERF-063] layout. Keys: `performance.hidden_gpu_engines = []` and `performance.gpu_optional_graphs = []` (values `power`, `clocks`). Drop the middle and bottom keys.
- Rule: rule 3 (TaskSlinger 0.9.2 layout).
- Edits: APP-032.

### R-055 Sidebar card headings and lines
- Sources in conflict: PERF-001 network card title "interface display name, its type, then rate"; PERF addendum (verified against `performance.png`): heading is the interface name, line 1 the type, line 2 the rate.
- Resolution: addendum. Headings "CPU", "Memory", "GPU N", "Disk N", `<ifname>`, "Fan N", battery kind. Network line 2 shows `↓ 1.2 Mbps ↑ 0.3 Mbps` when both are non-zero, else the total (PERF-084 [now PERF-055, PERF-056]).
- Rule: rule 4 (verified screenshot).
- Edits: PERF-001.

### R-056 CPU security row
- Sources in conflict: PERF-032 [now PERF-037] "CPU security: N mitigated, N not affected, N vulnerable"; PERF addendum "Security features" listing Secure Boot, Kernel lockdown, SME, user shadow stack, IOMMU, with the vulnerability summary in the tooltip.
- Resolution: addendum. Row label "Security features".
- Rule: addendum takes precedence (leader note).
- Edits: PERF-032 [now PERF-037].

### R-057 Displays list source
- Sources in conflict: PERF-065 [now PERF-072] says the sysfs connector `enabled` file is wrong on NVIDIA 615; PERF addendum found KScreen agreeing with sysfs and asks to withdraw the claim; PERF-065 [now PERF-072] orientation from `kscreen-doctor --json` vs addendum `-j` key list.
- Resolution: mode, refresh, connector type and `max bpc` come from DRM KMS read-only ioctls on `/dev/dri/cardN`; enabled state, primary, orientation and scale come from `kscreen-doctor -j` on KDE. The sysfs `enabled` claim is withdrawn and moves to the unverified list. Row format and connector names per PERF-091 [now PERF-073]. HDR, VRR and wide gamut are P4.
- Rule: rule 4.
- Edits: PERF-065 [now PERF-072] (remove the `enabled` sentence; adopt the addendum's mechanism).

### R-058 Public IP setting location and mini window trigger
- Sources in conflict: PERF-054 [now PERF-060] setting under Monitoring; PERF addendum and APP-022 under Window & startup. PERF-082 [now PERF-093] mini window opened by double-click; addendum adds View > Window item and Ctrl+Shift+M.
- Resolution: "Auto-fetch Public IP on launch" under Window & startup, key `network.auto_fetch_public_ip`. Mini window: View > Window > "Performance mini window" (checkable), Ctrl+Shift+M, tray menu "Show mini window" (P3), and double-click on the Performance graph area. Double-click on the mini window returns to the main window.
- Rule: addendum takes precedence; TaskSlinger string order (verified in the inventory).
- Edits: PERF-054 [now PERF-060], PERF-082 [now PERF-093].

### R-059 Heat colouring references
- Sources in conflict: v1 PROC-033 [now PROC-040] Power reference "class index/4"; r2 PROC-029 [now PROC-040] Power 25 W; disk and I/O "100 MB/s".
- Resolution: r2 references with IEC labels: CPU 400/ncpu % (12.5% here), CPU (one core = 100%) 400%, memory columns 10% of MemTotal, disk and I/O rates 100 MiB/s, network 10 MiB/s (or 80 Mbps), GPU 100%, VRAM 25% of total VRAM, power 25 W. Counts are not tinted. On by default, intensity 50%.
- Rule: r2 wins; R-014 on units.
- Edits: v1 PROC-033 [now PROC-040]; r2 PROC-029 [now PROC-040] labels.

### R-060 Startup placeholders
- Sources in conflict: v1 PROC-046 [now PROC-021] no shimmer on the Processes tab; APP-059 skeleton rows on first load for every tab; SYS-016 skeleton rows plus "Loading service data..." toast.
- Resolution: Processes tab shows rows from the first sweep with `…` in rate cells and no skeleton. Services, Startup Apps, Connections and Information show skeleton rows or blocks until their first data, once per session. Services also shows the toast "Loading service data...".
- Rule: rule 3 (TaskSlinger has no shimmer on Processes but does have the services toast).
- Edits: APP-059 (exclude Processes).

### R-061 Run dialog and Search File Handles details
- Sources in conflict: window titles "tasklens Run" (v1, APP-014/045) vs "Run" (r2); administrator run `pkexec /bin/sh -c` (r2) vs `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c` (APP-014); Handle Search result columns (APP-015 four columns) vs r2 PROC-080 [now PROC-096] six columns and menu.
- Resolution: titles "tasklens Run" and "tasklens Handle Search" (APP-045 naming). Administrator run uses APP-014's `pkexec env … /bin/sh -c <text>` under `org.freedesktop.policykit.exec`, never tl-helper. `kcm_*` names run `systemsettings <name>` (r2). History file and format per r2 PROC-079 [now PROC-095]. Handle Search columns and menu per r2 PROC-080 [now PROC-096]; footer note per r2.
- Rule: APP owns the window frame and privilege path; r2 owns the content.
- Edits: r2 PROC-079 [now PROC-095] title and pkexec line; APP-015 columns.

### R-062 Low-memory watcher thresholds
- Sources in conflict: v1 PROC-093 [now PROC-097] PSI `full 150000 1000000`, MemAvailable under 5%; r2 PROC-081 [now PROC-097] `full 100000 1000000`, under 3%, MemAvailable check every 5 s.
- Resolution: r2 values, except the PSI window: the trigger is `full 200000 2000000` (10% stall over a 2 s window, the smallest window an unprivileged trigger may use since Linux 6.5; R-151). `MemAvailable` under 3%, checked every 5 s.
- Rule: r2 wins.
- Edits: v1 PROC-093 [now PROC-097] superseded.

### R-063 History hover and header formats
- Sources in conflict: v1 PROC-084 [now PROC-090] "Memory: %.2f MiB/GiB", "Disk: %.2f MB/s"; r2 PROC-077 [now PROC-090] "Memory: 1.22 / 62.70 GiB", "Disk: 4.20 MB/s", "GPU: 41.0%".
- Resolution: "CPU: 12.3%", "Memory: 1.22 / 62.70 GiB", "Disk: 4.2 MiB/s", "Network: 12.0 Mbps", "GPU: 41.0%".
- Rule: r2 wins; R-013/R-014 formats.
- Edits: r2 PROC-077 [now PROC-090]; v1 PROC-084 [now PROC-090].

### R-064 Process metadata refresh cadence and time base
- Sources in conflict: ADR-016 cmdline, exe and cgroup refreshed every 10 sweeps and on comm change; r2 PROC-001 [now PROC-002] every 10 s; r2 "Times use CLOCK_MONOTONIC"; v1 PROC-002 cgroup re-read only on new pid or exec.
- Resolution: ADR-016 (every 10 sweeps and immediately when `comm` changes). All timestamps are CLOCK_BOOTTIME (ADR-003). fd count via `fstatat` `st_size` of `/proc/<pid>/fd` with readdir fallback (r2).
- Rule: ADR-003/016.
- Edits: r2 PROC-001 [now PROC-002] two bullets.

### R-065 Column picker title, headings and aliases
- Sources in conflict: v1 PROC-048 [now PROC-024] headings "Identity, File, Security, …"; r2 PROC-016 [now PROC-022, PROC-023, PROC-024, PROC-025] "Image, Process, CPU, Memory, I/O, Disk, Network, GPU, Objects, Security, Linux" and aliases `Working set private - "Private (anon)"`; add PROC-102 [now PROC-024] "Columns" title, headings "Image, Identity, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux", aliases; APP-018 example alias `Resident memory - "Memory"`.
- Resolution: add PROC-102 [now PROC-024] title and headings. Aliases: `Working set private - "Memory"` (R-005) and `GPU memory dedicated - "VRAM"`. The filter field matches full name and alias.
- Rule: addendum later and more specific.
- Edits: r2 PROC-016 [now PROC-022, PROC-023, PROC-024, PROC-025] headings and aliases; APP-018 example.

### R-066 Minigraph card titles for process sources
- Sources in conflict: v1 PROC-087 [now PROC-093] "<name> (<pid>) - CPU"; r2 PROC-062 [now PROC-093] "firefox (Process 1234) CPU", "firefox by name CPU", "Firefox by app CPU"; PERF-077/086 [now PERF-085, PERF-080] "firefox (Process 1234)", "firefox by name", "Firefox by app", column-keyed cards append " · CPU", and "<name> (<pid>)" with "Process <pid>" only before the name is known.
- Resolution: PERF-086 [now PERF-080, PERF-085]: `<name> (<pid>) · CPU`, `<name> by name · CPU`, `<App> by app · CPU`; `Process <pid>` only until the name is read. States "No match" and "Ended" per PERF-087 [now PERF-086]; r2's extra toasts stay.
- Rule: PERF owns the card (both PROC files say so).
- Edits: r2 PROC-062 [now PROC-093] titles; v1 PROC-087 [now PROC-093].

### R-067 Tray tooltip for process graphs
- Sources in conflict: v1 PROC-090 [now PROC-094] "<name> (PID n): CPU 3.2%"; r2 PROC-063 [now PROC-094] "firefox: CPU 3.2%"; PERF-079 [now PERF-089] "CPU Usage: 18.6%" for page graphs, "3 procs: firefox" for name filters.
- Resolution: PERF-079 [now PERF-089] forms: page graph "<Title>: 18.6%", process graph "<name> (PID n): CPU 3.2%", name filter "3 procs: firefox", app "<App>: CPU 3.2%".
- Rule: PERF owns the tray; v1's PID form kept because it disambiguates (inferred).
- Edits: r2 PROC-063 [now PROC-094].

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
- Sources in conflict: APP-032 and PROC-095 [now PROC-099] list keys that no resolved feature uses.
- Resolution: dropped: `tables.remember_sort`, `tables.remember_columns` (always persisted), `processes.column_graphs` (fixed six), `processes.cpu_one_core_100` and ADR `processes.cpu_mode` (R-044), `processes.filter_cmdline` (R-043), `units.network_base2`, `units.memory_*`, `units.disk_*` (R-015), `performance.gpu_graph_middle/bottom` (R-054), `state.first_run_done` (R-028), `window.system_titlebar` (R-021), `copy_rows_tsv_shortcut` (documentation only, add PROC-095 [now PROC-099]), `processes.tree_view`, `processes.grouping` (R-010), `debug.show_health_window` (the Collector health window is opened from the Debug menu or status bar, not a setting).
- Rule: each resolution above.
- Edits: APP-032, PROC-095 [now PROC-099], ADR-011.

### R-072 Phase numbering
- Sources in conflict: ADR-004 worker table and ADR-016 use phases 1, 2, 3 and "later" (Processes worker is phase 2, Connections and Services phase 3); every spec uses P1..P4 with the process table in P1 and Services in P2.
- Resolution: canonical phases: P0 spike (ADR-014); P1 = ADR phases 1 and 2 (Performance core, process table, P1 actions); P2 = ADR phase 3 (Services, Startup Apps, Connections, Properties windows, KWin bridge, 5 and 10 min ranges, remaining themes); P3 = helper, history, tray, detached graphs, mini window, strings, dumps, low-memory; P4 = everything marked P4 in the specs. ADR crates named "later phase" map: `tl-history`, `tl-tray`, `tl-helper` to P3; `tl-netmon` to P4.
- Rule: specs' numbering is used by every bead; ADR text is relabelled.
- Edits: ADR-004 worker table Phase column (2 becomes P1, 3 becomes P2, later becomes P3 or P4); ADR-016 bracketed phases; ADR-001 crate comments.

### R-073 Process Properties pin and keep-above
- Sources in conflict: v1 PROC-063 [now PROC-056] pin via one-shot KWin script; r2 PROC-065 [now PROC-077] pin hidden without the bridge; APP-009 pin on every dialog.
- Resolution: the pin button is shown on every secondary window and on the main window; on KDE it sets `keepAbove` through the bridge (R-046); elsewhere, X11 included, it is disabled with tooltip "Not supported by this desktop" (R-177). Only the main window's state persists (`window.always_on_top`); secondary windows keep theirs in memory per key and pass their caption with the action (R-176).
- Rule: APP-009 owns the pin; consistent with ADR-010.
- Edits: r2 PROC-065 [now PROC-077] ("hidden" becomes "disabled").

### R-074 Journal reading
- Sources in conflict: ADR-016 `journalctl -o json -n 50 --no-pager -u <unit>` with 5 s timeout; SYS-015 200 entries, follow mode, "Load older", user units via `--user-unit`.
- Resolution: SYS-015 behaviour: `journalctl -u <id> -b -n 200 -o json --no-pager` (`--user-unit` for user units), `-f` while Follow is on, the child killed when the tab closes; 5 s timeout applies to the non-follow fetch.
- Rule: SYS owns the feature; ADR-016 updated to match (engineering detail, not a structural rule).
- Edits: ADR-016 "Service logs" row.

### R-075 Column display keys apply to every table
- Sources in conflict: table (a) keeps `column_resize`, `column_separators` and `dim_zero_values` under `[processes]` (from v1 PROC-095 [now PROC-099]); APP-018 applies View > Columns to every table, and APP-032 had `tables.*` keys.
- Resolution: one key each, in a new `[tables]` section of `settings.toml`: `tables.column_resize = "proportional"` (`proportional | last_column`), `tables.column_separators = false`, `tables.dim_zero_values = false`. Each key applies to all five tables (Processes, History, Services, Startup Apps, Connections). There are no per-table keys. The three `processes.*` keys are removed.
- Rule: APP-018 owns View > Columns, which acts on every table (rule 3); key naming by ADR-011 style.
- Edits: table (a); ADR-011 section list; merged sys-app APP-018, APP-032; merged proc PROC-025, PROC-041, PROC-042, PROC-099.

### R-076 X11 window picker
- Sources in conflict: R-046 drops the X11 EWMH fallback for the window features; APP-006 kept an X11 picker path (`XGrabPointer` crosshair, then `_NET_WM_PID`).
- Resolution: the X11 picker path is dropped in v1. The "+" button is hidden without the KWin bridge.
- Rule: R-046; ADR-010 (X11 is best effort).
- Edits: merged sys-app APP-006.

### R-077 Corner radius
- Sources in conflict: ADR-009 `UiMetrics.radius = 5 × ui_scale` for everything; APP-040 4 px for rows and menus and 6 px for cards.
- Resolution: `UiMetrics` has two radii: `radius_small = 4 × ui_scale` (rows, menus and other small elements) and `radius_card = 6 × ui_scale` (cards). `appearance.rounded_corners = false` sets both to 0.
- Rule: rule 3 (TaskSlinger look).
- Edits: ADR-009 "Metrics and scale"; merged sys-app APP-040.

### R-078 About window title
- Sources in conflict: APP-045 window naming pattern "tasklens <Name>"; APP-053 "About tasklens".
- Resolution: the window title is "tasklens About". The Help menu item stays "About tasklens…".
- Rule: APP-045 naming, applied to every secondary window.
- Edits: merged sys-app APP-045, APP-053.

### R-079 TaskSlinger theme share codes
- Sources in conflict: none. The `TS1.` code encoding in APP-038 is inferred (U-17).
- Resolution: no new decision. APP-038 stands as written: generate one code under Wine and decode it; if that fails, import only `TL1.` codes and raw JSON. The entry is marked "verify U-17".
- Rule: none needed.
- Edits: merged sys-app APP-038.

### R-080 Ctrl+Esc through kglobalaccel
- Sources in conflict: none. Whether `setForeignShortcutKeys` sticks for a launch component that is not running is unverified (U-16).
- Resolution: no new decision. APP-051 stands as written: test on Plasma 6; the fallback writes `kglobalshortcutsrc` and asks the user to log out and back in. The entry is marked "verify U-16".
- Rule: none needed.
- Edits: merged sys-app APP-051.

### R-081 Number formats do not follow the locale in v1
- Sources in conflict: table (d) gives one fixed form per kind; APP-043 says number and date formats follow the selected locale.
- Resolution: in v1 every number uses table (d) exactly, whatever the language: decimal point ".", digit grouping with a thin space (U+2009), units and patterns unchanged. Changing the language changes text only. Dates and times use the table (d) patterns in local time; no merged entry asks for a locale-specific date form, so none changes with the language.
- Rule: table (d) is canonical, and the fixed column widths (template strings in table d) depend on it.
- Edits: merged sys-app APP-043.

### R-082 Startup Apps live refresh
- Sources in conflict: ADR-008 reads startup entries on page open and on `RefreshNow`; SYS-019 (old SYS-020 [now SYS-019]) adds inotify live refresh.
- Resolution: while the Startup Apps tab is visible, the `Startup` worker also watches the autostart directories and the two Plasma script directories with inotify and listens to the user manager's `UnitFilesChanged` signal; either triggers a rescan. Both stop when the tab is not visible.
- Rule: ADR-008 demand rules (work runs only while a surface shows it).
- Edits: ADR-008 schedule table; merged sys-app SYS-019.

### R-083 Strings written for the merged specs
- Sources in conflict: none. These strings have no TaskSlinger or Mission Center source; the merge wrote them.
- Resolution: accepted as written:
  - decorations restart toast "Restart tasklens to apply the title bar change" (APP-001);
  - window title "tasklens Keyboard Shortcuts" (APP-030);
  - window title "tasklens Theme Editor" (APP-037);
  - graph colour stored in `state.toml` as `accent` or the lower-case colour name (APP-039);
  - generic confirmation window title "tasklens" (APP-045).
- Rule: APP-045 naming pattern; none of them conflicts with a source.
- Edits: merged sys-app APP-001, APP-030, APP-037, APP-039, APP-045 (the "inferred" marks go).

### R-084 Detached graph window state
- Sources in conflict: table (a) has `[[minigraphs]] detached` but no window grouping, size or position; PERF-083 (old PERF-075 [now PERF-083]) persists size, stacked cards and, on KDE, position.
- Resolution: `[[minigraphs]]` gains `window = 0` (the detached window the card sits in; ignored when `detached = false`). `state.toml` gains `[[detached_windows]]` with `id`, `width`, `height`, `x`, `y`. `x` and `y` are written and applied only on KDE through the KWin bridge. ADR-010's rule against restoring the main window's position is unchanged.
- Rule: R-018 (window state lives in `state.toml`); R-046 and R-073 (position only through the bridge).
- Edits: table (a); merged perf PERF-083.

### R-085 Minigraph GPU source
- Sources in conflict: the card menu picks a GPU (old PERF-072 [now PERF-080]); the table (a) `source` values have no device.
- Resolution: `source` accepts `gpu:<device key>`, `vram:<device key>` and `gpu_temp:<device key>` (device keys per PERF-005). The bare values `gpu`, `vram` and `gpu_temp` mean the first GPU in sidebar order.
- Rule: table (a) shape extended; no rule conflicts.
- Edits: table (a); merged perf PERF-080.

### R-086 Matching detached and mini windows through the KWin bridge
- Sources in conflict: old PERF-075 [now PERF-083] used its own `/Scripting loadScript` script and a per-window app ID; R-046 makes the KWin bridge worker the only KWin channel.
- Resolution: the bridge finds a detached graph window or the mini window by tasklens's pid plus the window caption ("tasklens Graph <n>", "tasklens Mini Window") in `WindowsSnapshot`. All tasklens windows keep the one app ID `io.github.oysteinkrog.Tasklens`.
- Rule: R-046; R-001 (one app ID).
- Edits: ADR-004 KWin bridge worker; merged perf PERF-083, PERF-093.

### R-087 GPU API versions probe mode
- Sources in conflict: PERF-071 (old PERF-064 [now PERF-071]) needs Vulkan and OpenGL versions from `tl-gpu-probe --api-versions`; ADR-007 describes only the NVML sampler.
- Resolution: `tl-gpu-probe` has a second, one-shot mode `--api-versions`. It loads Vulkan (ash) and EGL, prints one postcard frame with the versions per GPU to stdout, and exits. The engine runs it once at startup, in its own process, never in the NVML sampler process. A crash or timeout gives `Unknown`.
- Rule: ADR-007 extended because the feature needs it; the crash isolation goal holds.
- Edits: ADR-007; ADR-001 `tl-gpu-probe` row (adds `ash` and an EGL loader); merged perf PERF-071.

### R-088 SMART fallback through pkexec smartctl
- Sources in conflict: PERF-051 (old PERF-046 [now PERF-051]) runs `pkexec /usr/bin/smartctl -a -j /dev/<d>` when UDisks2 or its NVMe module is missing; table (c) lists `org.freedesktop.policykit.exec` only for Run as administrator.
- Resolution: keep the fallback in P3 under `org.freedesktop.policykit.exec`, and list that use in table (c). It is not a tl-helper operation.
- Rule: R-002 (third-party actions used as-is); R-004 (no SMART read in the helper).
- Edits: table (c); merged perf PERF-051.

### R-089 Phase of View > Graphs
- Sources in conflict: APP-017 and APP-021 put the Graphs submenu in P3 with the toggles in P2; table (a) marks `[[minigraphs]]` hide and range as P1, and the minigraph panel is P1.
- Resolution: the Graphs submenu, its seven graph checkboxes and "Reset Minigraph Layout" ship in P1 with the panel. "Keep detached graphs open when closing main window" ships in P3.
- Rule: table (a) phase marks.
- Edits: merged sys-app APP-017, APP-021; merged perf PERF-080.

### R-090 Sidebar toggle state
- Sources in conflict: none. F9 and Ctrl+T hide the Performance device sidebar, and no key stores that.
- Resolution: the sidebar visibility is not persisted. The sidebar shows at every start.
- Rule: table (a) is the only schema.
- Edits: merged perf PERF-007.

### R-091 "Copy rows as table" in the row menu
- Sources in conflict: R-033's order has no "Copy rows as table"; add PROC-098 [now PROC-070] gives it a menu label for Ctrl+Shift+C.
- Resolution: it is row-menu item 14, after "Copy all metrics".
- Rule: addendum is later and more specific; R-012 defines the command.
- Edits: merged proc PROC-057.

### R-092 Freeze, Thaw and Kill for units outside app.slice
- Sources in conflict: ADR-006 validates only `app.slice` cgroups; PROC-064 offers Freeze App, Thaw App and Kill App on every app and unit row, including `session.slice`, `background.slice` and system units.
- Resolution: dispatch by target. `ActionTarget::Cgroup` for cgroups under `app.slice` (ADR-006 cgroup operations). `ActionTarget::Unit` for every other unit: the user or system manager's `FreezeUnit(name)`, `ThawUnit(name)` and `KillUnit(name, "all", SIGKILL)`, with `ALLOW_INTERACTIVE_AUTHORIZATION` so polkit `org.freedesktop.systemd1.manage-units` can prompt for system units.
- Rule: ADR-006 extended because the feature needs it; systemd owns units outside `app.slice`.
- Edits: ADR-006; merged proc PROC-064.

### R-093 I/O priority "Default (follows nice)"
- Sources in conflict: PROC-068 offers "Default (follows nice)"; ADR-005 `IoPriority` has no such value.
- Resolution: add `IoPriority::None` (`IOPRIO_CLASS_NONE`, level 0).
- Rule: ADR-005 extended because the feature needs it.
- Edits: ADR-005; ADR-006 `SetIoPriority` detail; merged proc PROC-068.

### R-094 CPU decimals in Copy details
- Sources in conflict: R-013 and table (d) give one decimal for percent everywhere; TaskSlinger's Copy details block uses `%.2f%%`.
- Resolution: Copy details prints CPU with two decimals (`%.2f%%`, for example `CPU: 3.27%`). This is the only exception to the percent row of table (d).
- Rule: rule 3 (TaskSlinger's fixed block, verified string).
- Edits: table (d); merged proc PROC-070.

### R-095 VirusTotal one-time notice flag
- Sources in conflict: PROC-074 shows a one-time notice; table (a) has no key for it.
- Resolution: `state.toml` `[processes] virustotal_notice_accepted = false`. It is state, not a user choice, so it is not in `settings.toml` and not in the settings window.
- Rule: R-018 (state goes to `state.toml`).
- Edits: table (a); merged proc PROC-074, PROC-099.

### R-096 Hash crates
- Sources in conflict: PROC-074 and PROC-078 need SHA-256 and MD5; ADR-001's `tl-collect` row lists no hash crate.
- Resolution: `tl-collect` may depend on `sha2` and `md-5`.
- Rule: ADR-001 extended because the feature needs it.
- Edits: ADR-001 dependency table; merged proc PROC-078.

### R-097 Thread-level action target
- Sources in conflict: PROC-079 has thread actions (priority, affinity, signal); ADR-005 has no thread target.
- Resolution: `ActionTarget::Thread { process: ProcessKey, tid: u32 }`. The engine opens and verifies the process pidfd (`open_verified`), then checks that `/proc/<pid>/task/<tid>` exists, then acts on the TID; the pidfd is polled after the call as for other PID-based syscalls. The helper accepts this target for `Signal`, `SetNice`, `SetSched` and `SetAffinity`.
- Rule: ADR-005 and ADR-006 extended because the feature needs it; the identity rule is unchanged.
- Edits: ADR-005; ADR-006; merged proc PROC-079.

### R-098 smaps in the helper's file set
- Sources in conflict: PROC-081 needs per-mapping RSS from `/proc/<pid>/smaps`; R-004 and ADR-006 list `smaps_rollup` only.
- Resolution: add `smaps` to the `ReadProcessFiles` set.
- Rule: rule 2 (the allowlist made a required feature impossible).
- Edits: R-004 set; ADR-006; merged proc PROC-081.

### R-099 Environment masking and copying
- Sources in conflict: v1 PROC-077 [now PROC-083] masked `*KEY*` and `*COOKIE*`; r2 PROC-071 [now PROC-083] masked a different list; neither said what copy does with masked values.
- Resolution: mask the union: names matching `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*PASSWD*`, `*KEY*`, `*COOKIE*` or `*CREDENTIAL*` (case-insensitive; `*KEY*` covers `*_KEY` and `*API_KEY*`). "Copy value" and "Copy all" copy a masked value as "••••••" unless its row is revealed.
- Rule: ADR-015 (secrets never leave by accident); union keeps both sources' protection.
- Edits: merged proc PROC-083.

### R-100 Boot ID in saved process minigraph sources
- Sources in conflict: `process:<pid>:<start_ticks>` (table a) can match a new process after a reboot.
- Resolution: the source string is `process:<boot_id 16 hex>:<pid>:<start_ticks>`. A saved card whose boot ID is not the current boot shows "Ended".
- Rule: ADR-003 `ProcessKey` includes the boot ID.
- Edits: table (a); merged proc PROC-093; merged perf PERF-085, PERF-086.

### R-101 Phase of process minigraph cards
- Sources in conflict: add PERF-087 [now PERF-086] put process cards in P2; both PROC sources put the menu items that create them in P3.
- Resolution: P3 for the menu items and the process cards (merged PERF-085, PERF-086, PERF-087).
- Rule: a card cannot ship before the menu that creates it.
- Edits: merged proc PROC-093; merged perf PERF-086.

### R-102 Handle Search result menu labels
- Sources in conflict: R-061 adopts the r2 PROC-080 [now PROC-096] labels "End Process (SIGTERM)" and "Kill Process (SIGKILL)"; R-045 and table (b) name SIGTERM "End Task" and SIGKILL "End Process".
- Resolution: "End Task" (SIGTERM) and "End Process" (SIGKILL). The rest of R-061 stands.
- Rule: R-045 naming, the same everywhere.
- Edits: merged proc PROC-096; merged sys-app APP-015.

### R-103 Inferred mechanisms in the merged PROC spec
- Sources in conflict: none. The merge wrote three mechanisms that no source gives.
- Resolution: accepted as written:
  - app-row Disk falls back to the cgroup `io.stat` rate when a member's per-process value is Restricted (PROC-008);
  - the in-RAM rolling history window keeps the last 10 blocks in the ADR-012 block encoding, writes nothing to disk and is dropped when the Processes tab closes (PROC-086, D-010);
  - "Delete from history" adds the key to a per-boot deleted-keys list beside the segments, which every query filters out; segments are not rewritten (PROC-088).
- Rule: D-010 and ADR-012 for the rolling window; the others conflict with nothing.
- Edits: merged proc PROC-008, PROC-086, PROC-088 (the "inferred" marks go).

### R-104 Confirmation focus and Enter rule
- Sources in conflict: UI-FND-021 and UI-SHL-091 said Enter runs the default (action) button with focus on Cancel for destructive confirmations; UI-PRC-014 said Enter is ignored in danger confirmations; UI-PWN-025 deferred to whichever won.
- Resolution: one rule for every confirmation window. Focus starts on Cancel in a destructive confirmation and on the action button otherwise. Enter activates the focused button, so a stray Enter in a destructive confirmation cancels; the destructive button needs Tab then Enter, or a click. Esc always cancels. The confirmation width is 420 and an optional `t_md` 500 heading line carries the owner's title.
- Rule: owner decision (consistency pass, 2026-10-03); UI-FND-021 is the component authority.
- Edits: UI-FND-021; UI-SHL-091; UI-PRC-014; UI-PWN-025 (open item 6 closed); UI-SYS-047, UI-SYS-064 already matched.

### R-105 Group header row band
- Sources in conflict: D-011 says group header rows stand out by bold text, a background band and the count; UI-PRC-002 drew them on plain `list_bg` with a top divider; UI-SYS-014 reused the renderer with `t_base` text.
- Resolution: group header rows are `row_h` tall, filled with the `header` colour across the full row width, with a 1 px `divider` line at the top edge (except the first row), label `t_md` 600 `text_primary` with the count, chevron 16. The same renderer serves the Services manager groups.
- Rule: D-011; `header` is the band colour the table header and the Information section bands already use.
- Edits: UI-PRC-002; UI-SYS-014.

### R-106 Column picker form and title
- Sources in conflict: PROC-024 titles the picker "Columns"; UI-SHL-034 called it a popover anchored under the header; UI-PRC-007 made it a secondary window "tasklens Columns" 420 × 560.
- Resolution: a `TlSecondaryWindow` tool window titled "tasklens Columns" (APP-045 title pattern), `WindowKey::Columns`, one instance bound to the table that opened it, 420 × 560 default, 320 × 400 minimum, immediate apply. `OpenColumnPicker { table }` opens or focuses it from the header menu, View > Columns and the shell.
- Rule: APP-045 (every dialog is a pinnable window) over PROC-024's bare title; the table spec owns the picker.
- Edits: UI-SHL-034; UI-PRC-007 (open item closed).

### R-107 Status text form: floating chip
- Sources in conflict: APP-029 asks for a 22 px status bar on gpui-kit `StatusBar`; the TaskSlinger reference (`processes.png`, `performance.png`, `information.png`) draws a 26 px floating chip at the bottom-left of the table and nothing on Performance and Information; UI-PRC-018 and UI-SYS-001 drew a bar.
- Resolution: the chip of UI-SHL-050: floating bottom-left, 26 high, `tab_bg`, text `t_sm` `text_primary`, segments separated by " | " (PROC-051), "[PAUSED - Ctrl]" in `accent`, health items after " · ", hidden when empty. Pages provide `status_text(&self, out: &mut String)` and emit `StatusChanged`. Tables add bottom padding equal to the chip height.
- Rule: rule 3 (the reference is the visual authority where the text spec disagrees).
- Edits: UI-SHL-050 (open item 1 closed); UI-PRC-017, UI-PRC-018; UI-SYS-001; merged sys-app APP-029 reads "status chip (UI-SHL-050)" from now on.

### R-108 Toolbar and chrome Tab stops
- Sources in conflict: UI-FND-010 makes every `TlButton` a Tab stop; UI-SHL-010 keeps the title bar, toolbar and caption buttons out of the Tab order; UI-PRC-019 listed a focus ring and "shell toolbar tab order".
- Resolution: the shell rule stands. Title bar, toolbar and caption buttons are not Tab stops; every chrome command has a key or menu path (table b). `TlButton` stays a Tab stop everywhere else.
- Rule: TaskSlinger has no keyboard path into its chrome, and table (b) covers every command.
- Edits: UI-PRC-019; UI-SHL open item 6 closed.

### R-109 Toast enter animation
- Sources in conflict: UI-FND-020 specifies a 200 ms enter; gpui-kit 0.7.0 fixes the notification enter at 400 ms and exit at 200 ms and exposes no setting.
- Resolution: accept the kit's 400 ms enter and 200 ms exit in P1. Reduced motion and `motion.animations = false` still turn both off. Changing it later means vendoring one constant.
- Rule: no behaviour depends on the number; a fork for 200 ms is not worth it.
- Edits: UI-FND-020 (open item 1 closed).

### R-110 Information section titles and spacing
- Sources in conflict: UI-FND-003 listed `t_lg` 600 for Information section titles while UI-FND-025 and UI-SYS-081 use `t_md` 500; UI-FND-025 fixed the card gap at `sp6`, the label column at 180 and the sub-block indent at `sp6`, while `information.png` measures 37, 232 and 11 to 12.
- Resolution: Information section titles are `t_md` 500 (the measured title is 14 px). `TlKeyValueGrid` takes `.label_width`, `.block_indent` and `.block_gap` as parameters (defaults 180, `sp6`, `sp4`); the card gap belongs to the page. The Information page passes 232, `sp5`, `sp5` and a card gap of `sp8` 32. `t_lg` 600 stays for the About window name, the App Properties header and the Performance page header.
- Rule: rule 3 for the measured values; one text token per element type.
- Edits: UI-FND-003, UI-FND-025; UI-SYS-080, UI-SYS-081 (open item O-7 closed).

### R-111 Tooltip width
- Sources in conflict: UI-FND-019 max width 360; UI-PRC-012 row tooltip max width 560 (PROC-039's command-line line needs it).
- Resolution: `TlTooltip::text` wraps at 360; `TlTooltip::lines` (row tooltips) wraps at 560. Both keep the 12-line cut.
- Rule: the multi-line tooltip carries paths and command lines that read badly at 360.
- Edits: UI-FND-019; UI-PRC-012.

### R-112 Menu minimum width
- Sources in conflict: UI-FND-022 min width 180; UI-PRC-006 and UI-PRC-013 min width 240; UI-SYS-017, UI-SYS-044, UI-SYS-063 min width 220.
- Resolution: `TlMenu` defaults to `min_w(180)` and owners may raise it with `TlMenu::min_w(px)`. Process row and header menus use 240, system table row menus 220, every other menu the default. Max width stays 360.
- Rule: the shortcut column in the process menus needs the room; menus without hints do not.
- Edits: UI-FND-022; UI-PRC-006, UI-PRC-013.

### R-113 Toast durations, stack and position
- Sources in conflict: UI-PRC-015 said a 16 px top margin, button toasts closed after 30 s and up to 10 live toasts; UI-FND-020 says top 72, button toasts 15 s, 4 visible plus a 16-entry wait list.
- Resolution: UI-FND-020 wins on every number. Owners choose kinds, texts and keys only.
- Rule: the foundation owns the toast queue.
- Edits: UI-PRC-015.

### R-114 Summary toast for 4 or more action targets
- Sources in conflict: SYS-006 and SYS-022 say one toast per result; APP-044 shows at most 4 toasts, so a 6-service Start would hide results.
- Resolution: 1 to 3 targets get one toast per target; 4 or more targets get one summary toast per request ("Starting 6 services...", then "6 services started" or "4 of 6 services started. 2 failed." with a "Details" button opening a `TlDialog` that lists each failed target and its error). The same pattern covers stop, restart, enable, disable, startup enable, disable and run now. R-203 gives the exact strings for service reload, reset failed and startup type, and for the Startup Apps and Connections families; startup and connection actions push no Progress toast (R-197).
- Rule: APP-044 stack limit; no result may be lost.
- Edits: UI-SYS-007 (open item O-10 closed); merged sys-app SYS-006, SYS-022 gain the pointer.

### R-115 "N/A" in the Log On tab
- Sources in conflict: SYS-012 shows "N/A" for missing Log On values; R-050 sets the PERF-013 words for every Properties field.
- Resolution: "Not reported" (R-050). "N/A" appears nowhere.
- Rule: R-050 has precedence over a merged spec entry.
- Edits: UI-SYS-032 (open item O-11 closed); merged sys-app SYS-012.

### R-116 Namespaces dropdown placement
- Sources in conflict: SYS-037 puts the Namespaces dropdown in the status bar; the status chip of UI-SHL-050 holds text only.
- Resolution: the Connections page draws the control itself as a floating element at the bottom-right of the table body (`sp4` from both edges, `menu_bg` fill, 1 px `divider` border, `r_md`), a `TlButton::Ghost` sm "Namespaces: Host only | All" opening a `TlMenu`. The chip gets no control slot.
- Rule: keeps the chip one component; P4 feature, smallest change.
- Edits: UI-SYS-070 (open item O-6 closed); merged sys-app SYS-037.

### R-117 Telling a dismissed polkit prompt from a denial
- Sources in conflict: SYS-005 and APP-044 want "Authentication cancelled" for a dismissed prompt and "<failure>: not authorized" for a denial; for systemd D-Bus calls both are believed to come back as the same access-denied error.
- Resolution: pkexec paths (helper, setup, Run as administrator) use the exit code: 126 dismissed, 127 not authorized. systemd D-Bus paths show "<failure text>: not authorized" for both until U-39 is verified; if phase 0 finds a distinguishing error name, the dismissed case switches to "Authentication cancelled". `InteractiveAuthorizationRequired` (no agent running) shows "<failure text>: no authentication agent is running".
- Rule: never claim a cause the code cannot tell apart.
- Edits: UI-SYS-007 (open item O-3 closed); U-39.

### R-118 Journal stream API and the Logs tab font
- Sources in conflict: ADR-005 had no path for a streamed journal; ADR-016 runs `journalctl` in the engine; UI-SYS-036 needs follow mode, paging and a monospace font that UI-FND-050 did not bundle.
- Resolution: `EngineClient::journal(JournalQuery) -> JournalHandle` with `load_older` and `set_follow`, events `Batch`, `Access`, `Error`, batches at most every 100 ms, the child killed when the handle drops (ARCHITECTURE ADR-005). `NotoSansMono-Regular.ttf` (SIL OFL, static) is bundled for the Logs tab, the Strings tab and Environment values, family "Noto Sans Mono"; the font bead checks it loads (U-40).
- Rule: the engine owns child processes (ADR-001); a log view needs fixed-width columns.
- Edits: ARCHITECTURE ADR-005, ADR-009; UI-FND-050; UI-SYS-036 (open items O-4 and O-8 closed).

### R-119 Engine additions written by the UI files
- Sources in conflict: the UI files needed Demand fields, commands, action kinds, snapshot components and modes that ADR-003, ADR-005, ADR-006, ADR-008 and ADR-009 did not have.
- Resolution: accepted as written in the UI files and now current-state text in ARCHITECTURE: `Demand` fields `details`, `affinity_targets`, `service_cgroup_stats`, `unit_details`, `remote_host_cookies`, `connections_all_netns`; `Command` variants `SetSysTableQuery`, `DetailRefresh`, `ComputeHashes`, `VerifyPackage`, `StringScan`, `CancelStringScan`, `HandleSearch`, `CancelHandleSearch`, `HistoryQuery`, `HistoryDelete`, `ReprobeSource`, `SetupStatus`, `RunSetup`; `ActionKind` variants `UnitReload`, `UnitResetFailed`, `UnitMask`, `UnitUnmask`, `UnitWriteRecovery`, `UnitRemoveRecovery`, `StartupRunNow`, `StartupDelete`, `SockDestroy`, `Dump`; `ActionTarget::Socket`; snapshot components `details` (bit `DETAILS`, with Properties payloads, handle search state and affinity masks), `history` (bit `HISTORY`), `SystemSnapshot.cpu_topology`, the system table rows, counts, manager health, unit details and `destroy_supported` fields; `EngineMode::LowMemory`; dump progress read by the UI from the output file size every 500 ms; `GraphPalette.chart_bg`; `UnitRemoveRecovery` for system units is `WriteServiceDropIn` with an empty key set, which deletes the tasklens drop-in (R-004's allowlist does not grow).
- Rule: ADR text follows the spec that needs it; the additions conflict with no decision.
- Edits: ARCHITECTURE ADR-003, ADR-004, ADR-005, ADR-006, ADR-008, ADR-009; UI-PWN open items 1 to 3, UI-SYS open items O-1, O-2, O-5 and UI-FND open item 5 closed.

### R-120 Keys and contexts beyond table (b)
- Sources in conflict: the UI files bind keys in contexts table (b) did not list (`PropsWindow`, `KvGrid`, `AffinityDialog`, `PriorityDialog`, `SignalDialog`, `HandleSearch`, `LowMemory`, `StartupTable`, `ConnectionTable`, `JournalList`, `ColumnPicker`, `History`, `Information`, `ServiceProps`).
- Resolution: the contexts and their rows are added to table (b) below. The Keyboard Shortcuts window (APP-030) lists them under the owning window's heading.
- Rule: table (b) is the one key list.
- Edits: table (b); UI-SYS open item O-9 closed.

### R-121 Agent-invented UI strings
- Sources in conflict: none. The UI files wrote strings no source gives, marked "(new)" or "chosen".
- Resolution: accepted as listed in "Accepted UI strings" below. Every later change to one of them is a RECONCILIATION edit, not a bead decision.
- Rule: every user-visible string has one home.
- Edits: none beyond the list.

### R-122 Default Light palette
- Sources in conflict: UI-FND-001 gives Default Light values with no TaskSlinger frame to measure.
- Resolution: ship the UI-FND-001 values. When the APP-035 extraction runs (U-18) compare against TaskSlinger's "Default Light" export and replace values that differ, keeping the 4.5:1 contrast floor.
- Rule: a measured source replaces a derived one when it arrives.
- Edits: UI-FND open item 2 closed.

### R-123 Kit rem scaling and the focus-ring heuristic
- Sources in conflict: UI-FND gives target px sizes while kit sizes are rem-based (13/16 of the documented px at `font_size` 13); `TlFocusRing` relies on a "last input was keyboard" heuristic because GPUI has no focus-visible.
- Resolution: the first component bead measures the kit `Button` small height at rem 13 and picks the kit size (`xsmall` or `small`) that gives 24 px (U-41); the gallery render tests hold every target px. `TlFocusRing` ships with the heuristic; if it misfires the fallback is the kit default of always showing the ring on focus (U-42).
- Rule: verify, then adjust one constant; no spec text depends on the kit's internal size names.
- Edits: UI-FND open items 3 and 4 closed.

### R-124 Shift held at launch on Wayland
- Sources in conflict: table (b) "Shift held at launch" unlocks privileged actions; a Wayland client gets no modifier state before focus.
- Resolution: UI-SHL-110 step 6's 500 ms first-focus rule, verified on KWin in the P3 bead (U-43). If KWin sends no modifiers on first focus the Shift path is dropped on Wayland and `privilege.unlock_at_launch` remains.
- Rule: a feature the platform cannot support is dropped there, not faked.
- Edits: UI-SHL open item 3 closed.

### R-125 Kit TitleBar controls and kit Input global shortcuts
- Sources in conflict: UI-SHL-003 draws its own caption buttons and drag region; whether gpui-kit 0.7.0's `TitleBar` can hide its own buttons is unchecked. UI-SHL-024 assumes the kit input does not swallow Ctrl+N, Ctrl+Q and Ctrl+W.
- Resolution: own drag code and caption buttons unless the P1 bead finds a switch (U-44). The P1 keymap test `keymap_matches_table_b` also runs with the filter focused (U-45).
- Rule: verified by the first bead that touches it.
- Edits: UI-SHL open items 4 and 5 closed.

### R-126 Performance tab open items
- Sources in conflict: PERF-012 wants selectable Details values but GPUI text selection on arbitrary elements is unverified; PERF-080 defines the Disk card as active time while the reference shows a rate; per-core labels at 10 Hz and `cpu_fast` with smooth graphs off were undecided; legend orientation was a one-constant choice.
- Resolution: selectable text ships through the UI-PRF-020 fallback (click selects a value, Ctrl+C copies it, right-click "Copy value") unless the kit gains text selection before the bead runs (U-46). Disk card: graph is aggregate active time, value is the aggregate rate (UI-PRF-041). Per-core value labels update at 10 Hz; if the `perf-cpu` bead's p99 fails the budget they update at 5 Hz while the plot keeps 10 Hz (U-47). `cpu_fast` stays on with smooth graphs off. Legend: vertical under 200 px cell width, horizontal above (UI-PRF-003).
- Rule: ship the measurable default; the alternatives are one constant each.
- Edits: UI-PRF open items 1 to 5 closed.

### R-127 Process-windows window exceptions
- Sources in conflict: UI-FND-021 said tool windows are resizable with a 480 × 320 minimum; the Run window is fixed at 480 × 248; UI-FND-025 named 126 px as the Properties label width while UI-PWN-006 computes 96 to 150 px per column.
- Resolution: UI-FND-021 carries the Run window as its own row (fixed 480 × 248). Properties General computes its label width per column (96..150 px) and passes it through `.label_width(..)`; 126 is the measured TaskSlinger value, not a constant.
- Rule: the component takes parameters; the owner decides the values.
- Edits: UI-FND-021, UI-FND-025; UI-PWN open items 4 and 5 closed.

### R-128 One Action namespace
- Sources in conflict: shell.md and processes.md put every GPUI Action in the `tasklens` namespace with bare names; performance.md used `perf::` and `minigraph::`; system-tabs.md used `services::`, `startup::`, `connections::` and `information::`.
- Resolution: one namespace, `tasklens`. Page actions carry the page word in their name (`PerfSelectDevice`, `MinigraphCloseDetachedWindow`, `ServiceStart`, `ServicesRemoveFilters`, `StartupRunNow`, `ConnectionCloseTcp`, `CopyInformationSection`). Window-level actions keep the shell's names (`ShowMainWindow`, `ToggleMainWindow`, `ToggleMiniWindow`, `OpenAdditionalSetup`, `SetSetting`, `RunSetupStep`, `Quit`). The source module is a build detail.
- Rule: shell.md is the authority for actions; GPUI action names must be unique per namespace.
- Edits: performance.md, system-tabs.md (job 1 of the consistency pass).

### R-129 `ActionTracker` file name
- Sources in conflict: ADR-009 puts `ActionTracker` in `tl-ui/src/actions.rs`; shell.md puts every GPUI Action and `bind_keys` in `tl-ui/src/actions.rs`.
- Resolution: the entity lives in `tl-ui/src/action_tracker.rs` (with `message(&ActionError)`); `tl-ui/src/actions.rs` holds the window-level GPUI Actions and `bind_keys_global`. Shared table actions live in `tl-ui/src/table/actions.rs`; page actions in `pages/<page>/actions.rs`; `app.rs` calls each `bind_keys_*`.
- Rule: one file, one purpose; shell.md names the Action file.
- Edits: ADR-005, ADR-009, ADR-015; `docs/plan/00-epics.md` item 1.

### R-130 Where `write_atomic` lives
- Sources in conflict: ADR-011 names `tl-app/src/atomic.rs::write_atomic` for settings, state, themes and XDG autostart overrides; the autostart overrides are written by the engine's `Startup` worker, and ADR-001 lets no crate depend on `tl-app`.
- Resolution: `write_atomic` lives in `tl-ops/src/fs.rs` together with the autostart override write and remove. `tl-app` gains `tl-ops` in its allowed workspace dependencies and `tl-app/src/atomic.rs` re-exports the function. `tl-engine` already depends on `tl-ops`.
- Rule: ADR-001 dependency direction (edges point toward `tl-model`; `tl-ops` is below both callers).
- Edits: ADR-001 table (tl-app row), ADR-011 "Save".

### R-131 `ureq` for the public IP fetch
- Sources in conflict: PERF-060 fixes `ureq` 3 with rustls on the blocking pool; no ADR-001 row allows it and ADR-002 has no pin.
- Resolution: `ureq = { version = "3", default-features = false, features = ["rustls"] }` is added to `[workspace.dependencies]` and to `tl-engine`'s allowed row. The bead that adds it pins the exact version `cargo add` resolves and runs `xtask check-deps`, which settles U-30. It is used only in `tl-engine/src/workers/public_ip.rs`.
- Rule: a dependency the spec names is added to the ADR tables, not smuggled in.
- Edits: ADR-001 table, ADR-002 pins.

### R-132 Fluent strings and the catalogue layout
- Sources in conflict: APP-043 fixes `fluent-bundle` and `unic-langid` with embedded `.ftl` files; no ADR-001 row allows them and no file names the catalogue layout.
- Resolution: both crates are added to `tl-ui`'s allowed row and the workspace pins. `tl-ui/src/i18n.rs` exposes `tr(key) -> SharedString` and `tr_args(key, args) -> SharedString`, a `Strings` Global and `strings_version` (UI-SHL-131). One English catalogue `assets/locales/en-US.ftl` is embedded with `include_str!`; every epic that adds user-visible strings appends its keys in the bead that uses them (chain edit, `docs/plan/00-epics.md` section 2). Keys are `<area>-<entry>-<slug>` in kebab case. Areas: `fnd` (foundation), `shl` (shell), `prc` (processes), `pwn` (process windows), `prf` (performance), `sys` (system tabs), `app` (tl-app strings passed through). The slug is the kebab-case of the first five words of the English text, letters and digits only, for example `prc-015-process-terminated`; a key that collides inside one entry gets the suffix `-2` (R-200). Other locales are P4 and are separate files in the same directory.
- Rule: APP-043 mechanism; ADR-001 tables follow the spec that needs them.
- Edits: ADR-001 table, ADR-002 pins, ADR-009 entity structure.

### R-133 Process-windows Actions use the `tasklens` namespace
- Sources in conflict: process-windows.md conventions say `actions!(tl_pwn, [...])`; R-128 fixes one namespace, `tasklens`.
- Resolution: `crates/tl-ui/src/windows/actions.rs` declares its Actions in `tasklens` with the prefixes `Props`, `Affinity`, `Priority`, `Signal`, `Run` and `HandleSearch`.
- Rule: R-128.
- Edits: process-windows.md conventions.

### R-134 Opening URLs, file locations and launching programs
- Sources in conflict: PROC-073, PROC-074, PROC-095, SYS-023, PERF-061 and UI-SHL-093 need session-bus calls (`org.freedesktop.FileManager1`, portal `OpenURI`, `StartTransientUnit`) with an `xdg-open` fallback and a result toast; `tl-ui` may not use zbus (ADR-001) and `Command` had no variant for them.
- Resolution: three new `ActionKind` variants in `tl-model::actions`: `OpenUri(Arc<str>)`, `ShowInFileManager { path: Arc<str>, properties: bool }` and `Launch(LaunchSpec)` where `LaunchSpec { what: LaunchWhat::{Command(Arc<str>), DesktopEntry(Arc<str>), Unit(UnitRef)}, as_admin: bool, cwd: Option<Arc<str>> }`. For these three kinds `ActionRequest.targets` is empty and admission skips the target checks. They run in `tl-engine/src/opener.rs` and `tl-engine/src/launcher.rs` on the engine's session-bus connection with the fallbacks the specs name, under the 2 s deadline (120 s for `as_admin`), and report through the normal `ActionEvent` path so the existing toasts apply.
- Rule: one action path, one place for session-bus code (ADR-005, ADR-006).
- Edits: ADR-005 action types.

### R-135 `tl-collect` module layout
- Sources in conflict: none. ADR-016 names only `sanitize.rs` and `benches/sweep.rs`.
- Resolution: the layout in `docs/plan/00-epics.md` section 2 is the current state: `sysroot.rs`, `fdcache.rs`, `counters.rs`, `sanitize.rs`, `hash.rs`, `parse/` (one file per system file), `sample/` (`cpu`, `memory`, `disk`, `net`, `sensors`), `proc/` (per-pid readers, `sweep.rs`, `smaps.rs`, `detail.rs`, `strings.rs`, `handles.rs`), `apps/` (`classify`, `cgroup_app`, `desktop`, `icons`, `naming`, `launchers`), `net/` (`sock_diag`, `socket_map`, `attribution`, `connections`, `netns`, `rdns`), `startup/`, `inventory/`, `gpu_drm/`.
- Rule: ADR-016 is the home of the parser rules; the layout is written there once.
- Edits: ADR-016 "Parsers".

### R-136 `AppRequest` lives in `tl-model`
- Sources in conflict: UI-SHL-112 names an `AppRequest` value carried over `async_channel::bounded(4)` from `tl-app` to `tl-ui` without defining it.
- Resolution: `tl_model::command::AppRequest { Activate { token: Option<Arc<str>> }, SelectTab(Tab), SelectPid(u32), Quit }`. `tl-app` sends it from the `org.freedesktop.Application` handler; `TasklensUi::init` receives it. `Command`, `KWinCommand`, `SetupStep`, `HistoryRange`, `HistoryWhat` and `CommandError` live in the same file `tl-model/src/command.rs`.
- Rule: ADR-001, neither crate depends on the other for a shared type.
- Edits: ADR-003 module list, ADR-005, ADR-010 single instance.

### R-137 cgroup files read by the process sweep
- Sources in conflict: PROC-008, PROC-093, PROC-098 and SYS-003 read cgroup files (`memory.stat` `anon`, `memory.current`, `memory.swap.current`, `cpu.stat` `usage_usec`, `io.stat`, `pids.current`, `cgroup.procs`) that ADR-016's source table does not list.
- Resolution: the table gains one row for them. App scopes are read every sweep; demanded unit cgroups (`Demand.service_cgroup_stats`, `unit_details`) at 1 Hz; `memory.current` only in `EngineMode::LowMemory`. Files are opened with `openat` from a dirfd on `/sys/fs/cgroup` and closed in the same sweep.
- Rule: ADR-016 lists every file the collectors read (R-052).
- Edits: ADR-016 source table.

### R-138 Accepted structural choices from the epic map
- Sources in conflict: none. `docs/plan/00-epics.md` made seven structural choices that no doc fixed.
- Resolution: accepted as written there:
  - the phase-0 spike measures the real chart, table, pages and bridge, so E01 depends on 21 anchors across 13 epics, those anchors are `phase-p0`, and the ADR-014 gate on P1 UI beads is a scheduling rule (P0 first, P1 UI after `E01-A2`), not a dependency edge (item 8);
  - `tl-engine/src/reducer/sys_table.rs` is created by E09-A1 with its signatures and an empty body and filled by E14-A1 (item 9);
  - the low-memory stripped sweep is its own `Source` in `tl-engine/src/workers/lowmem.rs`, selected by one `match` arm on `EngineMode` in `workers/processes.rs` (item 10);
  - `tasklens-setup` is `packaging/tasklens-setup`, POSIX `sh`, installed with mode 0755 (item 11);
  - the KWin bridge scripts are `assets/kwin/{kwin-bridge.js,pick.js,cmd-template.js}`, embedded with `include_str!` and written to `$XDG_RUNTIME_DIR/tasklens/` at run time (item 12);
  - 33 epics stand (item 14);
  - `--gallery` is declared beside `--bench`, both behind the `bench` feature, and table (c)'s argument list gains it (item 15).
- Rule: the epic map is input 6 of the bead brief; its choices bind writers once recorded here.
- Edits: ADR-004 engine modes; table (c) arguments.

### R-139 Device commands, device details and the public IP state (OI-E01)
- Question: which `Command` variants carry SMART, smartctl, eject, public IP and battery history requests; how device details and eject progress reach the UI; where the public IP state lives. Raised by E12-OI-1 (A to F), E25-OI-1, E28 item 1, E03 contracts item 10, B7 item 11, E12-A3 placeholder question, E03 first-half item 9.
- Resolution:
  - One command enum in `tl-model/src/command.rs`: `Command::Device(DeviceCommand)` with `DeviceCommand { SmartRead { disk: Arc<str>, update: bool }, SmartReadAdmin { disk: Arc<str> }, Eject { seq: u64, disk: Arc<str> }, FetchPublicIp, BatteryHistory { native_path: Arc<str> } }`. `disk` is the kernel name (`sda`, `nvme0n1`), the same string as `DeviceIdentity::Disk.name`; the UI resolves a `DeviceIdx` to it through `DeviceCatalog`, workers never see a `DeviceIdx`. `seq` is a UI counter. `command_variants_exhaustive` goes from 18 to 19 variants (21 with R-148).
  - Routing in the reducer: `SmartRead`, `SmartReadAdmin`, `Eject` to `forward(SourceId::Udisks, WorkerCommand::Device(cmd))`; `FetchPublicIp` to `SourceId::Inventory`; `BatteryHistory` to `SourceId::UPower`. `WorkerCommand` gains `Device(DeviceCommand)`. No `SourceId::PublicIp`: the fetch is a job the Inventory worker runs on request.
  - Public IP state stays `InventorySnapshot.public_ip: PublicIpState { NotFetched, Fetching, Fetched { v4: Option<Arc<str>>, v6: Option<Arc<str>>, fetched_at_s: i64 }, Failed(Arc<str>) }`. No `SystemSnapshot.public_ip`. The Information tab and the Network page read `snapshot.inventory.public_ip`; `request_public_ip_fetch` sends `Command::Device(DeviceCommand::FetchPublicIp)` after the notice.
  - Startup auto-fetch is UI-driven: `EngineSettings.auto_fetch_public_ip` is dropped. E20-024 sends the command once at startup when `network.auto_fetch_public_ip` and `network.public_ip_notice_accepted` are both true. E12-019 keeps `ureq` 3, rustls, 5 s, 64-byte body, no redirects, and drops its start-up fetch and `AtomicBool` flag.
  - Device details: `Snapshot.device_details: Arc<DeviceDetailsSnapshot>` with `DeviceDetailsSnapshot { header: ComponentHeader, cpu: Arc<CpuStaticInfo>, memory: Arc<MemoryDetails>, disks: Arc<[(DeviceIdx, DiskDetails)]>, nets: Arc<[(DeviceIdx, NetDetails)]>, batteries: Arc<[(DeviceIdx, BatteryDetails)]> }`, mask bit `ComponentMask::DEVICE_DETAILS = 1 << 14` (`PROCESS_SERIES = 1 << 13`, R-154), `ALL = 0x7FFF`, fifteen bits. Worker records are the same structs keyed by `DeviceIdentity`; the reducer sets the index.
  - `DiskDetails { model, wwn, serial, interface: InvField<Arc<str>>, rotation_rpm: Option<u32>, scheduler: InvField<Arc<str>>, smart: SmartKind { Ata, Nvme, NeedsFallback, None }, smart_data: Option<InvField<Arc<SmartData>>>, smart_temp_c: Option<f32>, ejectable: bool, can_power_off: bool, eject: Option<(u64, EjectState)>, partitions: Arc<[PartitionRow]>, formatted_bytes, used_bytes, free_bytes: InvField<u64>, system_disk: InvField<bool>, swap: InvField<bool> }`. `EjectState { Running, Blocked { blockers: Arc<[Blocker]>, others_may_hold: bool }, Done, Failed(Arc<str>) }`; idle is `eject == None`. `Blocker { key: ProcessKey, name: Arc<str>, app: Option<Arc<str>>, paths: Arc<[Arc<str>]> }`; the UI groups blockers on `app`. `SmartData { updated_s: i64, health: InvField<Arc<str>>, power_on_s: InvField<u64>, selftest: InvField<Arc<str>>, ata: Arc<[SmartAttr]>, nvme: Arc<[(Arc<str>, Arc<str>)]> }`, `SmartAttr { id: u8, name: Arc<str>, value: i64, normalized: i32, threshold: i32, worst: i32, prefail: bool, online: bool, assessment: Arc<str> }`. `NetDetails { adapter_name, driver, mac: InvField<Arc<str>>, ifindex: u32, link_speed_mbps: InvField<u32>, state: LinkState, addresses: Arc<[Arc<str>]>, kind: NetKind, wifi: Option<WifiInfo> }`, `WifiInfo { ssid: Arc<str>, signal_dbm: i32, signal_pct: u8, rx_mbps: u32, tx_mbps: u32, freq_mhz: u32, channel: u16, security: Arc<str> }`. `BatteryDetails` is the E12-023 field list plus `history: Option<Arc<[(i64, f32)]>>`. `CpuStaticInfo` and `MemoryDetails` carry the PERF-036 to PERF-046 static rows as `InvField<T>`.
  - Transport: `SourceUpdate::DeviceDetails(DeviceDetailsUpdate { Cpu(Arc<CpuStaticInfo>), Memory(Arc<MemoryDetails>), Disks { from: SourceId, entries: Vec<(DeviceIdentity, DiskDetails)> }, Nets { from: SourceId, entries: Vec<(DeviceIdentity, NetDetails)> }, Batteries(Vec<(DeviceIdentity, BatteryDetails)>) })`. The reducer keeps one record per device and merges by field: the sysfs source fills every field it knows; a D-Bus source (`Udisks`, `NetworkManager`) overwrites `model`, `wwn`, `serial`, `rotation_rpm`, `smart*`, `ejectable`, `can_power_off`, `eject`, `kind`, `wifi`. A full list from a source replaces that source's contribution; a device missing from the list loses that source's fields. `BATTERY_PCT` is appended from `BatteryDetails.percentage`.
  - `Source` gains `fn take_extra(&mut self) -> Option<SourceUpdate> { None }`; `source::run` drains it after each `sample()` and sends those updates before the sample.
  - Totals: `DiskTotals.temp_c: Metric<f32>`, `NetTotals.state: LinkState { Up, Down, Dormant, Unknown }` (sysfs `operstate`), `SensorReading.fan_control: Option<FanControlMode { FullSpeed, Manual, Automatic }>` (`pwm<n>_enable` 0, 1, 2).
- Basis: ADR-003, ADR-005, PERF-051, PERF-052, PERF-060, PERF-078, PERF-079, APP-056, the E03-056, E03-063 and E12-019 to E12-029 bead texts [verified]; `operstate` and `pwm_enable` value sets [inferred, kernel ABI].
- Edits: ADR-003 (component, mask, totals fields), ADR-004 (`take_extra`), ADR-005 (`Command::Device`); beads as2.29, as2.16, as2.22, as2.30, as2.11, as2.33, 8hz.5, 8hz.6, 8q0.9, hjn.22, hjn.23 to hjn.26, hjn.32, hjn.33, hjn.7, jqk.26, o55.22, 9o0.32; b6-ui-performance.md E25-OI-1 and b8-ui-tables.md E28 item 1 point here.

### R-140 udev data source and the Partitions table (OI-E02)
- Question: how disk model, serial, WWN, bus, filesystem labels and adapter names are read without libudev, and who builds the PERF-050 Partitions table. Raised by E04-OI-1, E04-OI-2, E04-OI-11, E04-OI-13, E12-OI-4.
- Resolution:
  - New file `crates/tl-collect/src/parse/udev.rs`, owner E04, new bead E04-030 (P1): `pub fn read_udev_props(path: &Path, keys: &[&str], out: &mut UdevProps) -> Result<(), CollectError>` reads one `/run/udev/data/<entry>` file (64 KiB cap), keeps only `E:KEY=value` lines whose key is in `keys`, into `UdevProps(SmallVec<[(Arc<str>, Arc<str>); 8]>)`. Entry names: `b<major>:<minor>` (block), `n<ifindex>` (net), `+pci:<slot>` (PCI, GPU names). Keys: `ID_MODEL`, `ID_MODEL_FROM_DATABASE`, `ID_VENDOR_FROM_DATABASE`, `ID_BUS`, `ID_WWN`, `ID_SERIAL`, `ID_SERIAL_SHORT`, `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`, `ID_PART_ENTRY_OFFSET`, `ID_PART_ENTRY_SIZE`. Values go through `sanitize` before display.
  - `SysRoot { proc, sys, run }` with `run` default `/run`, so fixtures carry a scrubbed `run/udev/data/`. The ADR-013 scrub list gains `ID_SERIAL`, `ID_SERIAL_SHORT` and `ID_WWN` values (replaced by `SCRUBBED`) and MAC addresses (R-162).
  - E04-021 uses `ID_BUS=usb` for the USB kind and `ID_MODEL` for the model when present, else its sysfs rules; the disk persistent key is `disk:<ID_WWN or ID_SERIAL>` with the by-path fallback (PERF-005). E12-013, E12-014, E12-018 and E12-026 depend on E04-030.
  - New bead E04-031 (P2) `crates/tl-collect/src/parse/partitions.rs`: `PartitionRow { name: Arc<str>, start_bytes: u64, size_bytes: u64, holder: Option<Arc<str>>, mount: Option<Arc<str>>, extra_mounts: Arc<[Arc<str>]>, label: Option<Arc<str>>, fs_type: Option<Arc<str>>, usage_pct: InvField<f32>, flags: Arc<[Arc<str>]>, type_name: Arc<str>, read_only: bool, unallocated: bool }` and `PartitionTable { kind: Arc<str> /* "GPT" | "MBR" | "none" */, rows: Arc<[PartitionRow]> }`, ordered by `start_bytes`, gaps over 1 MiB as `unallocated` rows, GPT GUID names and attribute flags per PERF-050, MBR types as `MBR 0x%02X`. Rows land in `DiskDetails.partitions` (R-139).
  - New anchor E04-A4 "P2 system details" closes E04-013, E04-014, E04-015, E04-019, E04-023, E04-024, E04-028 and E04-031. The `minimal-vm` fixture bead is E04-032.
- Basis: PERF-005, PERF-047, PERF-049, PERF-050, PERF-056, ADR-001 (no libudev crate), ADR-016 [verified]; udev database file layout [inferred].
- Edits: ADR-016 `SysRoot`; ADR-013 fixture directory and scrub list; 00-epics section 2 (`parse/udev.rs`, `parse/partitions.rs` under E04); beads 2dl.1, 2dl.24, hjn.16, hjn.17, hjn.21, hjn.29 and the four new beads.

### R-141 ProcessRecord extras, group rows and table fields (OI-E03)
- Question: where the P1 extra-column values, the app cgroup values and the table-only fields live in tl-model, and how `GroupKind`, `NodeId` and `GroupRow` relate. Raised by E05 items 1 and 2, B3 items 5 and 6, E22 item 1, E03 first-half items 5 and 6, E03 contracts item 7.
- Resolution:
  - `ProcessRecord` stays as E03-008 writes it. Same file adds `ProcessExtras` and `ProcessSnapshot.extras: Arc<[ProcessExtras]>`, index aligned with `records`. Fields: `cpu_kernel_pct: f32, cpu_user_pct: f32, cpu_relative_pct: f32, utime_ticks: u64, stime_ticks: u64, start_ticks: u64, rss_peak_bytes: u64, rss_shared_bytes: u64, commit_bytes: u64, vsize_bytes: u64, vsize_peak_bytes: u64, page_faults: u64, page_faults_delta: u64, major_faults: u64, policy: SchedPolicy, rt_prio: u8, affinity: CpuSet, handles: Option<u32>, exe_name: Arc<str>, exe_path: Arc<str>, exe_deleted: bool, unit: Option<UnitRef>, state_letter: u8, frozen: bool, disk_read_bps: f32, disk_write_raw_bps: f32, flag_since_ns: TimeNs`. Priority and State texts are formatted in the UI from `policy`, `nice`, `rt_prio`, `state_letter` and `frozen`; `ProcState` stays. Start time is `btime + start_ticks / CLK_TCK`, formatted by the UI from the new `SystemSnapshot.boot_time_s: i64` (`/proc/stat btime`).
  - `ProcessSnapshot.apps: Arc<[AppCgroupSample]>` with `AppCgroupSample { group: GroupKey, anon_bytes: u64, swap_bytes: u64, io_read_bps: f32, io_write_bps: f32, rios: u64, wios: u64, pids: u32, cgroup: CgroupRef, validity: FieldValidity }`. Both types move from tl-collect into tl-model; the sweep fills them and E10 ships them in `ProcessSweep.extras: Vec<ProcessExtras>` and `ProcessSweep.app_stats: Vec<AppCgroupSample>`.
  - `GroupKind` keeps App, Background, System, Ungrouped (PROC-004). `NodeId` gains `Unit(UnitRef)`, `KernelThreads` and `Pseudo(PseudoKind { Interrupts, MemoryCompression })`. `GroupRow { node: NodeId, kind: GroupRowKind { App, Unit, KernelThreads, Pseudo }, name: Arc<str>, icon: Option<Arc<str>>, cpu_pct, memory_bytes, disk_bps, net_bps, gpu_pct, pids: u32, note: Option<Arc<str>>, validity: FieldValidity }` plus the numeric mirrors E03-008 has.
  - `TableRow.descendants: u32`, `TableRow.subtree: Option<u32>`, `TableQuery.include_children: bool`, `TableTotals { cpu_pct, memory_pct, swap_pct, disk_pct, net_bps, net_link_bps: Option<u64>, gpu_pct, vram_pct, threads, handles }` one `Total<T>` each, `ProcessTableView { total: u32, shown: u32, restricted: u32 }`, `SeriesSet.slow: Arc<[Arc<SeriesView>]>` for the 1 Hz tier.
  - `HealthSnapshot.caps: EngineCaps` (bitflags u16): `FILE_MANAGER1 = 1`, `GDB = 2`, `CONNECTIONS_DESTROY = 4`, `KWIN_BRIDGE = 8`. `GDB` from `tl_ops::dump::dump_available()` at start, `FILE_MANAGER1` when the opener finds an owner, `CONNECTIONS_DESTROY` from `SourceUpdate::ConnectionsDestroySupport`. Unit `MainPID` is a field of the services unit record.
  - Columns whose data is not in the record or extras sort `Absent`: the P2 and P4 package, file and security columns and the visible-row-only columns (`pss`, `uss`, `ctx_switches*`, `cycles*`, `windows`, `x11_resources`), matching R-007 and PROC-047.
- Basis: PROC-027 to PROC-038, ADR-003, E05-010 `SweptProc` fields, B3 item 5 and E22 item 1 lists [verified].
- Edits: ADR-003 (`ProcessExtras`, `AppCgroupSample`, `GroupRow`, `TableTotals`, `EngineCaps`), ADR-009 (`NodeId` variants); beads as2.10, as2.16, as2.17, as2.13, yms.1, 8hz.5, 8q0.7, 8q0.11 to 8q0.15; b8-ui-tables.md E22 item 1 points here.

### R-142 Crate pins and allowed-dependency rows (OI-E04)
- Question: which crates and features the ADR-001 rows and ADR-002 pins must gain. Raised by B1 item 2, E33-OI-2, E33-OI-3, E03 first-half items 4, 12 and 14, E03 contracts item 2, E05 items 3, 4, 5 and 12, E06 items 1, 2, 7 and 11, E12-OI-2, E14 item 3, E32 OI-5, E01-OI-1, B5 items 2, 22 and 30, E21 item 9.
- Resolution (checked against the crate manifests on disk):
  - `clap = { version = "4", default-features = false, features = ["std", "derive", "help", "usage", "error-context"] }` pinned; tl-app row.
  - `async-channel` (pin `2.5.0` exists): tl-app and tl-ui rows.
  - `rustc-hash`: tl-model row (`GroupKey` is an FxHash of `GroupKind`). The E03 `std::collections::HashMap` choices for `DetailsSnapshot` and `ServicesSnapshot.details` stay.
  - `bitflags = { version = "2", features = ["serde"] }`.
  - `rustix` features `["fs", "process", "net", "event", "system", "thread", "time", "pipe", "mm", "param"]`; `rustix::mm::mlockall` is safe, so E32 needs no `unsafe`. tl-model keeps `time` only.
  - `ureq` stays pinned `3`, `default-features = false, features = ["rustls"]`, tl-engine row; ureq 3.4.0 has no `tokio` dependency (U-30 answered, inferred from the manifest; `cargo xtask check-deps` confirms). E12-019 adds the `Cargo.toml` line as a chain edit on E08-A1.
  - `serde_json` (pin `1`): tl-engine row; E14-A4 adds the line.
  - PRNG: none. E01-004 writes SplitMix64 plus Box-Muller in `bench/synth.rs`; UI-PRF-053 gains "or an in-tree SplitMix64".
  - `jiff = { version = "0.2", default-features = false, features = ["std", "tz-system", "tzdb-zoneinfo"] }` pinned; tl-ui row. No libc, reads `/etc/localtime` and the system zoneinfo. `chrono` and `time` are out. tl-model's `format_local(LocalDateTime)` stays; tl-ui converts with jiff.
  - `drm`: not in v1; KMS display modes are a P4 item (add `drm` to the tl-collect row then). `x11rb`: P4 only. `libc` in tl-collect: no; user names from `/etc/passwd`, `getnameinfo` goes into `tl-ops/src/sys.rs` at P4. `flate2`: no; the package check runs `pacman -Qkk <pkg>`.
  - tl-helper row gains `tl-collect` (shared strings scanner, R-157).
  - `unicode-segmentation = "1.12"` pinned; tl-ui row. E15-003 is unblocked.
  - `image`: not added. E16-005 goes through gpui-kit's image support; fallback if it cannot: `image = { version = "0.25", default-features = false, features = ["png"] }` in the tl-ui row.
  - `wgpu`: not in any row. tl-app probes adapters through GPUI (`AppEnv::list_adapters`).
  - Dev-dependencies (`insta`, `proptest`, `tempfile`, `criterion`, `toml`, `tracing-subscriber`) are outside the rows; `xtask check-deps` checks `normal` edges only. E05's `[[bench]] sweep`, `criterion` and `tracing-subscriber` lines are allowed chain edits.
- Basis: ADR-001, ADR-002, APP-052, UI-PRF-053 and the manifests on disk [verified].
- Edits: ADR-001 rows (tl-app + `clap`, `async-channel`; tl-ui + `async-channel`, `jiff`, `unicode-segmentation`; tl-model + `rustc-hash`; tl-engine + `serde_json`; tl-helper + `tl-collect`); ADR-002 pins; UI-PRF-053; beads v9m.4, v9m.10, cr0.4, fut.4, y6k.4, hjn.22, yms.26, yms.1; b2-collect-ops.md E06 item 1.

### R-143 `unsafe` in tl-gpu-probe for the API versions mode (OI-E05)
- Question: add `crates/tl-gpu-probe/src/api_versions.rs` to the ADR-001 rule 4 list, or drop the mode. Raised by B3 item 13.
- Resolution: add the file. `ash` entry points are `unsafe fn` and `khronos-egl` needs raw calls for `eglGetProcAddress`. The file runs in the probe process, apart from the engine (ADR-007). Rule 4 lists four files: `crates/tl-ops/src/sys.rs`, `crates/tl-engine/src/gpu/spawn.rs`, `crates/tl-app/src/main.rs`, `crates/tl-gpu-probe/src/api_versions.rs`, each with `#![allow(unsafe_code)]` and a `// SAFETY:` comment per block. E13-006 is unblocked.
- Basis: ADR-001 rule 4, ADR-007, R-087 [verified]; ash and khronos-egl API shape [inferred].
- Edits: ADR-001 rule 4; bead ndy.8.

### R-144 Other `unsafe` or libc needs outside the allowed files (OI-E06)
- Question: how readers that need a syscall without a safe rustix wrapper get it. Raised by E07 note after O6, E05 item 6, E06 items 3 and 4, E14 item 13, E01-OI-9.
- Resolution: `tl_ops::sys::ioprio_get_tid(tid) -> OpsResult<IoPriority>` is `pub`, re-exported as `tl_ops::process::io_priority_of(key, boot)` with the identity check; the I/O priority column and menu check mark read it from the engine, never from tl-collect. The strings scan reads memory with `rustix::fs::pread` on `/proc/<pid>/mem` under the same ptrace access rule; PROC-084's mechanism line changes from `process_vm_readv` to `/proc/<pid>/mem`; the scanner stays in `tl-collect` `proc/strings.rs` and tl-helper reuses it. `getnameinfo` (reverse DNS, SYS-035) becomes `tl_ops::sys::reverse_lookup` when P4 starts; no bead now. `perf_event_open` (PERF-075, Cycles column) is not in v1; fdinfo only. Bench clocks: tl-app passes `rustix::time::clock_gettime(ClockId::ProcessCPUTime)` and `ThreadCPUTime` as function pointers; `tl_model::time::thread_cpu_ns()` is the same call behind tl-model's `rustix` `time` feature.
- Basis: ADR-001 rule 4 and tl-ops row; rustix 1.1.5 has `fs::pread` and `time::clock_gettime` with both clock ids [verified].
- Edits: PROC-084 mechanism line; ADR-016 strings-scan row; bead iuz.8 (`pub` and the `process.rs` wrapper).

### R-145 Core dump tool and the passed file descriptor (OI-E07)
- Question: `gcore -o` cannot write to an fd or to the exact path the UI polls; how does the dump reach the UI's file, directly and through the helper. Raised by E07-O1, E29-OI-2, E03 contracts item 19 (fd part), B3 item 16 (a).
- Resolution: the tool is `/usr/bin/gdb` by fixed path with `--nx --nh --batch -p <pid> -ex "gcore /proc/<caller pid>/fd/<n>"`, environment cleared except `PATH=/usr/bin:/bin` and `LC_ALL=C`, stdio on `/dev/null`. `dump_available()` requires both `/usr/bin/gdb` and `/usr/bin/gcore`. R-004 and ADR-006 change "runs `/usr/bin/gcore` by fixed path" to "runs `/usr/bin/gdb` by fixed path with `gcore` as a batch command". Direct path: the engine opens the UI's path `O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC` mode 0600 and calls `dump_to_fd`; the path must be directly under `<state_dir>/dumps/` with a `*.core` name. Helper path: the engine opens the file the same way and passes the fd as `SCM_RIGHTS` ancillary data with the `Dump` frame over a Unix socketpair that is the helper's stdin and stdout. `HelperRequest::Dump { key, mode }` has no fd field; ADR-006's `Dump { key, mode, out_fd }` becomes `Dump { key, mode }` plus the passed fd. R-003's transport sentence gains "(a socketpair, so a file descriptor can travel with the Dump request)". The helper runs the same `tl_ops::dump::dump_to_fd` as root.
- Basis: R-004, ADR-006, UI-PWN-034, E29-OI-2 and E03-070 text [verified]; gdb `gcore` script internals [inferred].
- Edits: R-003, R-004, ADR-006; beads iuz.15, qfx.9; b2-collect-ops.md E07-O1.

### R-146 `write_atomic` phase and the P1 settings store (OI-E08)
- Question: E07-A3 (`fs.rs`, `launch.rs`, `dump.rs`) is P2, but E33's P1 settings store needs `write_atomic`. Raised by B1 item 7, E33-OI-5, E07-O4.
- Resolution: E07-010 (`fs::write_atomic`) is its own P1 bead. E33-008 (settings save) and E33-014 (autostart writer) depend on it directly instead of on E07-A3, as a named exception to the anchors-only rule. E07-A3 stays P2. 00-epics section 4 records the exception.
- Basis: ADR-011, R-130, bead priorities [verified].
- Edits: 00-epics section 4; edges cr0.11 and cr0.17 to iuz.13, the two edges to iuz.3 removed.

### R-147 Privileged helper hook in E11 (OI-E09)
- Question: the shape of the hook through which E11 actions reach the P3 helper. Raised by B3 item 9, E29-OI-5, E29-OI-6.
- Resolution: E11-002 as written: `trait PrivilegedOps: Send + Sync + 'static { fn call(&self, cx: &Cx, req: HelperRequest, fd: Option<OwnedFd>, deadline: Duration) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>>; fn state(&self) -> HelperState; }` and `HelperCallError { Missing, Cancelled, NotAuthorized, StartFailed(Arc<str>), Timeout, Exited, Protocol(Arc<str>) }` (`StartFailed` added by R-246) in `crates/tl-engine/src/actions/mod.rs`; `ActionCtx.helper: Option<Arc<dyn PrivilegedOps>>`, `None` until E29-014 installs the `HelperHandle` implementation and the `req.privileged` routing arm. Until then a privileged request is `Rejected(Unsupported)` and never falls back to an unprivileged call. The `fd` argument is the `Dump` fd of R-145.
- Basis: ADR-005, ADR-006, i3o.4 text [verified].
- Edits: none to docs; edge qfx.17 on i3o.4.

### R-148 Helper session commands and the action error values (OI-E10)
- Question: how the shield starts and ends a helper session, how a dismissed or denied polkit prompt reaches the UI, and how "Already running" is reported. Raised by E29-OI-3, E14 item 11, B3 items 10 and 17, B8 item 9.
- Resolution:
  - `Command::StartHelperSession` and `Command::EndHelperSession` (variant count 21 with R-139). The session owns `helper: Option<HelperHandle>`; the reducer routes the two commands through `Reducer.helper_tx: Option<mpsc::Sender<HelperCommand>>` with `HelperCommand { Start, End }`, set by E29-020 as a chain edit; `None` logs at `debug` and drops the command.
  - `HealthSnapshot.helper_start: Option<(u64 /* seq */, HelperStartOutcome { Cancelled, NotAuthorized, Failed(Arc<str>) })>`; the UI shows the toast for a new `seq` once.
  - `ActionError` gains `NotAuthorized` (pkexec 127, polkit `NotAuthorized`), `AuthCancelled` (pkexec 126, `NotAuthorizedDismissed`) and `Unsupported` (no `FileManager1` owner, invalid command text). E11-011, E11-018 and E11-020 map systemd `AccessDenied` to `Failed(NotAuthorized)`.
  - "Already running" for `StartupRunNow` and `Launch` is `RejectReason::AlreadyRunning`, reported as `ActionState::Rejected(AlreadyRunning)`; neither an error nor a success. The UI maps it to the Info toast "Already running" (SYS-023, R-196).
  - A `pkexec` launch counts as `Succeeded` when the child is still running 2 s after the prompt closed; Plasma scripts in Run go as `LaunchWhat::Command` with the shell-quoted path.
- Basis: UI-FND-028, R-117, SYS-023, as2.13 and as2.19 text, E29-OI-3, E14 item 11 [verified].
- Edits: ADR-005 (commands, reject reason, error values), ADR-015 `ActionError` list; beads as2.29, as2.19, as2.13, 8q0.3, 8q0.9, qfx.23, i3o.13, i3o.17, i3o.18, i3o.20, the 9o0 "Already running" bead.

### R-149 Setup state and window pick answers (OI-E11)
- Question: where `SetupStatus`, setup run progress and the KWin pick result travel, and the final `SetupStep` list. Raised by E03 contracts items 16 and 17, E29-OI-4, E29-OI-14, E14 item 7, E21 item 11.
- Resolution: `HealthSnapshot.setup: Arc<SetupState>` with `SetupState { status: Option<SetupStatus>, run: Option<SetupRun> }`, `SetupStatus { rapl: RaplStatus { NoZone, Readable { tasklens_rule: bool }, NotReadable }, journal_group: bool, helper_installed: bool, lm_sensors: bool, hwmon_fan: bool, sensors_conf: bool }`, `SetupRun { seq: u64, step: SetupStep, phase: SetupPhase { Running, Succeeded, Cancelled, Failed(Arc<str>) }, output: Arc<str> }` (output capped at 64 KiB, last lines kept). E03-063's `SetupStatus { journal_group, lm_sensors_present }` and `SetupEvent` are replaced; no new channel. `SetupStep { RaplEnable, RaplRevert, SensorsDetect, SensorsRevert, PolkitRule { on: bool } }` with `args()` `["rapl-enable"]`, `["rapl-revert"]`, `["sensors-detect"]`, `["sensors-revert"]`, `["polkit-rule", "on" | "off"]`. Table (c) gains the `sensors-revert` subcommand (removes `/etc/modules-load.d/tasklens-sensors.conf`, PERF-108), P4. Window pick: `WindowsSnapshot.pick: Option<PickResult { seq: u64, outcome: PickOutcome { Picked { pid: u32 }, NoWindow, Cancelled } }>`; `KWinCommand::Pick { seq }` and `CancelPick { seq }`; the `WindowBridge` D-Bus interface gains `Picked(s uuid, s pid)` called by `pick.js` (pid as decimal text, R-241). UI-SHL-014's "as an `ActionEvent` or a `KWinEvent`" becomes "in `WindowsSnapshot.pick`". Transport: `SourceUpdate::Setup(Arc<SetupState>)` and the existing `SourceUpdate::Windows`.
- Basis: table (c), R-028, PERF-108, UI-SHL-014, E29-OI-4 [verified].
- Edits: ADR-003 (`HealthSnapshot.setup`, `WindowsSnapshot.pick`); table (c) `sensors-revert`; UI-SHL-014; beads as2.13, as2.16, as2.29, y6k.21, y6k.23, qfx.22, qfx.25.

### R-150 `SourceUpdate` variants and `EngineConfig` fields added up front (OI-E12)
- Question: which variants and config fields E08 writes up front so E29, E30 and E32 need no edits to `update.rs`, and which chain edits into `session.rs` and `apply.rs` are allowed. Raised by E29-OI-5, E30-OI-4, E32 OI-1, B3 items 2 and 3, E12 resume note.
- Resolution: E08-003 gains `SourceUpdate::Helper(HelperUpdate { state: HelperState, start: Option<(u64, HelperStartOutcome)> })` and `SourceUpdate::Setup(Arc<SetupState>)`; `source_of()` maps both to `None`. Low memory needs no variant: `ProcessSweep.low_memory: Option<Arc<LowMemoryInfo>>` with `LowMemoryInfo { groups: Arc<[LowMemGroup]>, mem_total: u64, mem_available: u64, psi_full_pct: Option<f32>, critical: bool }` and `LowMemGroup { group: GroupKey, name: Arc<str>, process_count: u32, uid: u32, cgroup: CgroupRef, memory_current: u64 }` in tl-model `process.rs`; the reducer copies it into `ProcessSnapshot.low_memory`. `EngineConfig { mode, sys_root, state_dir, history_dir: Option<PathBuf>, helper_path: Option<PathBuf> }`; tl-app sets `history_dir = $XDG_STATE_HOME/tasklens/history` and `helper_path = /usr/lib/tasklens/tl-helper` when the file exists. Chain edits allowed at named insertion points: E29-020 in `session.rs` (helper spawn) and `reducer/apply.rs` (the two helper commands; the `Helper` and `Setup` arms are written up front and only log until then); E30-A2 in `session.rs` (install the `RecordSink`, start the history worker) and `reducer/apply.rs` (`HistoryQuery`, `HistoryDelete` forward to `SourceId::HistoryRecorder`). `ResumeDetected` stays with E08's logind worker only.
- Basis: 00-epics section 2, 8hz.5 variant list, ADR-012, ADR-006 [verified].
- Edits: ADR-004 `EngineConfig` sentence; 00-epics section 2 chain-edit rows; beads 8hz.3, 8hz.5, 8q0.9, 8q0.18, as2.10, as2.16, m1f.8.

### R-151 PSI trigger window for the memory watcher (OI-E13)
- Question: R-062 writes `full 100000 1000000` (1 s window); since Linux 6.5 an unprivileged trigger must use a window that is a multiple of 2 s, and older kernels refuse unprivileged triggers. Raised by B1 item 5, E32 OI-2.
- Resolution: the trigger is `full 200000 2000000` (10% stall over a 2 s window) from the start. Older kernels refuse the write with `EINVAL` or `EPERM`; the watcher logs `warn` once and runs on the 5 s `MemAvailable` check only. No runtime probing of two strings. The 10% threshold and the 3% `MemAvailable` rule stay. New unverified entry U-51 (the engine file called it U-50, which was taken): unprivileged PSI trigger accepted on the owner machine (kernel 7.2), recorded by m1f.1's ignored test.
- Basis: R-062, PROC-097 [verified]; the kernel `psi.rst` rule for unprivileged triggers [inferred; U-51 confirms].
- Edits: R-062, PROC-097; U-51; beads m1f.1, m1f.3.

### R-152 Low-memory mode details owned by the engine and tl-app (OI-E14)
- Question: the remaining E32 choices. Raised by E32 OI-3, OI-4, OI-6 to OI-9, OI-11 to OI-13.
- Resolution: unit file as E32-002 writes it (`Description=tasklens low-memory watcher`, `Type=simple`, `Restart=on-failure`, `RestartSec=5`, `WantedBy=default.target`, `ExecStart=<current_exe> --watch-memory`, path `$XDG_CONFIG_HOME/systemd/user/tasklens-lowmem.service`, installed over the session-bus Manager API, `ActiveState` polled every 100 ms up to 2 s for exit status 214). The watcher logs `warn` and above to stderr only. It relies on the user manager's imported `WAYLAND_DISPLAY` or `DISPLAY`; when neither is set it logs `warn` and does not spawn; "no rescue window open" means the watcher's own child is alive. Texts: tooltip "systemd-oomd may end processes before the rescue window opens."; on install or remove failure revert the setting and send the desktop notification "Could not set up the rescue window"; `state.toml` `[lowmem] oom_score_adjust` with the extra tooltip sentence " The watcher runs with normal OOM priority." when it is 0; at launch reinstall the unit when `lowmem.enabled` is true and the file is missing. In `LowMemory` mode the app name is the app id parsed from the scope name; no desktop index is loaded. `EngineMode::LowMemory` runs the `Processes` worker (stripped sweep) and the `KWin` worker, the latter only for `KWinCommand::KeepAbove`; the window starts pinned. "Open Full tasklens" sends `ActionKind::Launch(LaunchSpec { what: LaunchWhat::Command("tasklens"), as_admin: false, cwd: None })`; E32-009 depends on E14-A3. The rescue process tries `mlockall(MCL_CURRENT)` once after the first frame, logs `warn` on failure, never `MCL_FUTURE`; PROC-098 gains "(current pages only)". `--force-full-memory-mode` is D-015.
- Basis: PROC-097, PROC-098, UI-PWN-032 to UI-PWN-034, APP-052, ADR-004, ADR-015 [verified]; systemd user-manager environment import on Plasma [inferred].
- Edits: ADR-004 `LowMemory` worker list; ADR-015 watcher logging; PROC-098; table (a) `[lowmem] oom_score_adjust`; UI-SHL-069 tooltip texts; beads m1f.4, m1f.6, m1f.8, m1f.11; edge m1f.11 on y6k.3.

### R-153 `UiLaunch` shape and the tl-app to tl-ui boundary (OI-E15)
- Question: the exact entry point tl-app calls, and how persist callbacks and tl-app-only features cross the boundary. Raised by B1 item 1, E33-OI-1, E33-OI-4, E33-OI-8, E33-OI-12, B5 item 38.
- Resolution: E16-012 is the contract: `tl_ui::run(UiLaunch) -> UiExit` with `UiLaunch { client: Arc<dyn EngineClient>, init: UiInit, open: OpenMode { Main, Gallery, LowMemory }, start: StartOptions { tab: Option<Tab>, select_pid: Option<u32>, start_minimized: bool, shift_held: bool, pending_crash_report: Option<PathBuf> } }` and `UiExit { Quit, RendererFailed(String), DeviceLost }`. There is no `platform` field and no `PlatformOps` trait: OI-U01 (R-171) wins over OI-E15 items 2 and 3 (leader decision, 2026-10-04). Settings, state, themes, the persist callbacks, `app_requests` and queued toasts are fields of `UiInit` (R-171); `debug` is `AppEnv.debug`; `on_quit` is the return value. Persist callbacks on `UiInit` (R-171): `persist: Box<dyn Fn(&Settings, &UiState)>` (called on every change, tl-app debounces 500 ms) and `persist_themes: Box<dyn Fn(PersistTheme)>` with `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}`; tl-app writes with `write_atomic`. The tl-app-only features are `AppEnv` members (R-171): Ctrl+Esc is `global_shortcut: Option<GlobalShortcutApi>` (the settings row shows "Not available" when `None`) and the TaskSlinger import is `import_taskslinger: Arc<dyn Fn() -> Result<ImportSummary, String>>` (no path argument; tl-app finds the TaskSlinger files). Every closure runs on the main thread and is not `Send`. Device loss: whoever detects it calls `tl_ui::request_exit(cx, UiExit::DeviceLost)`; tl-app relaunches once with `TASKLENS_RELAUNCHED=1` (table (c)). U-2 stays open and is answered by E33-011.
- Basis: ADR-001 tl-app row, UI-SHL-110 steps 8 to 11, fut.15 and cr0.1 text, ADR-011 [verified].
- Edits: ADR-010 and ADR-011 (`tl_ui::run(UiLaunch) -> UiExit`, the `UiInit` callbacks); table (c) `TASKLENS_RELAUNCHED`; beads fut.15, fut.1, cr0.1, cr0.11, cr0.14, cr0.15, cr0.20, cr0.21; b1-infra.md E33-OI-1; edges cr0.1, cr0.14, cr0.15 on fut.15.

### R-154 Aggregate devices and per-process series (OI-E16)
- Question: which device carries the Disk and Network card series, and how a process, name or app graph gets an engine-sampled series while hidden. Raised by E26-OI-1, E26-OI-2, E31 item 8 (a), PERF-005.
- Resolution: `DeviceIdentity::Aggregate(AggregateKind { Disks, Nets, Gpus })` with persistent keys `aggregate:disk`, `aggregate:net`, `aggregate:gpu`. The reducer computes the series: `Disks` arm `DISK_ACTIVE` = max over present physical disks, `DISK_READ` and `DISK_WRITE` = sums; `Nets` arm `NET_RX` and `NET_TX` = sums over interfaces whose `NetKind` is shown under `EngineSettings.show_network_*`; `Gpu` arm `GPU_UTIL` = max, `GPU_MEM` = sum. The three devices exist from session start with `present = true`. Per-process series: `GraphTarget { Process(ProcessKey), Name(Arc<str>), App(Arc<str>) }` and `ProcessSeriesId { target: GraphTarget, column: ColumnId }` in `ids.rs`; `Demand.graph_targets: Arc<[(GraphTarget, ColumnId)]>` with `GRAPH_TARGETS_MAX = 16` (clamped in `clamp_lists`); `Snapshot.process_series: ProcessSeriesSet { header: ComponentHeader, series: Arc<[(ProcessSeriesId, Arc<SeriesView>)]> }`, mask bit `PROCESS_SERIES = 1 << 13`. Columns allowed as graph sources: `cpu`, `memory`, `disk`, `network`, `gpu`, `gpu_mem_dedicated`. A non-empty `graph_targets` runs the `Processes` worker at `processes.interval_ms` even when `process_table` is false (PROC-093); the reducer appends one sample per sweep per target, sums over matching processes for `Name` and `App`, and marks a gap when none matches. Rings use the 60 s fast tier plus the 1 Hz slow tier. Tray process graphs use the same entries; `TrayDemand` has no process list (R-156).
- Basis: PERF-005, PERF-080, PERF-085, PROC-093, ADR-003, ADR-008, E26-OI-1 and E26-OI-2 [verified].
- Edits: ADR-003 (`Aggregate`, `process_series`, `ProcessSeriesId`), ADR-008 (`graph_targets` row and the hidden-sampling rule); beads as2.12, as2.6, as2.26, as2.16, as2.30, 8q0.9, dwy.4; new bead E09-029 (P2, `reducer/process_series.rs`).

### R-155 tl-model core types review (OI-E17)
- Question: the derived values in E03-A1's beads that B1 asked to have reviewed. Raised by E03 first-half items 1, 2, 3, 7 to 11 and 13, B1 item 6, E12-OI-6, B3 items 11, 12 and 14, E13 health note.
- Resolution: `ColumnId` numbers 1 to 121 in PROC-027 to PROC-036 order are frozen; dense index = number minus 1; `ColumnSet([u64; 4])` holds 256; new columns append after 121 and are never renumbered. The system tables use a separate `SysColumnId` in `table.rs`; `SysNodeId` lives in `table.rs`. `NodeId` gains `Unit`, `KernelThreads`, `Pseudo` (R-141). System totals use E03-009's `Metric<T>` field sets plus R-139's part-B fields and `GpuTotals.pcie_rx_bps`, `pcie_tx_bps: Metric<f32>`. `HealthSnapshot` keeps a plain `version: u64`. `SourceState { Off, Running, Late, Stale, Failed, Disabled { until: TimeNs }, Unsupported }`: `Unsupported` is terminal, never retried, used for "GPU probe not installed", `LibraryNotFound`, `DriverNotLoaded` and "KWin bridge not available". `last_error` is capped at 200 characters; no `SourceId` for the public IP fetch or the low-memory sweep. The 1 Hz tier is as E03-007 writes it; the fast ring holds 600 samples (CPU 10 Hz, 60 s). PROC-003's measured-zero rule applies to every rate, power and time form; `percent_2dp` zero is "0%"; `watts_pair` prints the limit as an integer. `DeviceDesc` gains `persistent_key: Arc<str>`, computed by workers per PERF-005 (`disk:<ID_WWN or ID_SERIAL>` with by-path fallback, `fan:<hwmon name>:<device path>:<n>`, `bat:<NativePath>`, `net:<MAC or name>`, `gpu:<uuid or PCI slot>`); `DeviceEntry` stores `persistent_key` and `net_kind: Option<NetKind>`. No `GPUS` list in P1: the `Gpu` and `GpuInfo` arms call `devices.ensure` per identity; `GpuTotals.device` is a placeholder the reducer overwrites; the `GpuInfo` payload carries `persistent_key`.
- Basis: PROC-003, PROC-027 to PROC-036, PERF-005, PERF-062, ADR-003, ADR-007, ADR-009, ADR-012, as2.7, as2.11, as2.13 [verified].
- Edits: ADR-003 (`DeviceEntry`, `SourceState::Unsupported`, `GpuTotals` pcie fields), ADR-009 (`NodeId`), PROC-003, 00-epics section 2 (`SysNodeId`); beads as2.13, as2.12, as2.11, 8hz.5, 8hz.8, ndy.12, 8q0.5, hjn.5.

### R-156 tl-model contract types: the derived field lists (OI-E18)
- Question: confirm the field lists E03-A2 derived, and settle the history types conflict. Raised by E03 contracts items 3, 5, 6, 8, 9, 11 to 15, 18, 20 to 22, 24, E30-OI-1, E14 item 1, B1 item 6.
- Resolution: accepted as the beads write them: `StartupRef { source: StartupSource, id: Arc<str> }`; `SystemSnapshot` and `WindowEntry`; the services, startup and connections records (`ImpactDetail`, `TcpInfoView`, `ConnRecord.four_tuple`); `InventorySnapshot` per-section structs; the Properties payloads; `FourTuple { local: SocketAddr, remote: SocketAddr }`, `Family { Inet, Inet6 }`, `Proto { Tcp, Udp }`; `RecoveryEdit` with one `Option` per SYS-013 key; `tl_model::command::Tab` with the six R-023 names, and tl-ui's `TabId` is this type; `KWinCommand` variants of E03-063 plus `seq` on `Pick` and `CancelPick`; `GpuApiVersions { pci_bus_id, vulkan_api, vulkan_driver, opengl }` plus `GpuDeviceInfo.cuda_version`, `vbios`, `architecture: Option<String>`; clamp then round to the nearest step; `Snapshot::empty` and `changed_since`; `CpuSet` derives `Serialize, Deserialize`. `TrayDemand { icons_shown: bool, cpu: bool, memory: bool, disks: bool, nets: bool, gpus: bool }`, no `process_keys`; `icons_shown` counts as a visible surface. History types: the E30 block wins over E03-062 and E03-063: `HistoryMetric`, `HistoryRange::{Live, At(TimeNs), Span { start_ns, end_ns }}`, `HistoryWhat::{Frame, TopAt { n: u8 }}`, `HistoryView` with `query_id`, `source`, `store_state`, `span`, `gaps`, `frame_t`, `range`, `process_count`, `rows`, `totals`, `series`, `stack`, `overview`, `top`, `realtime_offset_ns`, plus `HistoryProcessRow`, `HistoryTop`, `HistoryLoadProblem`. `ColumnId` has `image_path`. E14 item 1 is a check: the services unit record carries `cgroup_pids`, `has_cgroup_procs`, the four resource fields with `Validity`, `can_reload`, `main_pid`; `ManagerState`; `UnitDetails` with `RecoverySettings` and `DepEntry`; `JournalHandle` with `JournalControl::{LoadOlder, SetFollow(bool)}` and the `bounded(64)` event channel. The leader checks as2.20, as2.21, as2.27 against E14-001 and E14-004 before E14-A1 starts.
- Basis: UI-PRC-030 to UI-PRC-037, the UI-SYS data bindings, SYS-013, R-023, the E30 block, as2.26, as2.29, as2.36 [verified].
- Edits: ADR-012 history type names; beads as2.26, as2.28, as2.29, as2.35.

### R-157 Helper protocol: frame, replies, handshake and request set (OI-E19)
- Question: two shapes exist, E03-070's and E29-OI-1's; which one holds, and what is added. Raised by E29-OI-1, E29-OI-7, E29-OI-9, E03 contracts item 19, B3 item 16.
- Resolution: E03-070 holds: `HelperFrame { seq: u32, request }`, `HelperReply { seq: u32, last: bool, body: HelperReplyBody }`, `Hello { proto }` both ways, `HELPER_PROTO_VERSION = 1`, `HelperError { IdentityChanged, PermissionDenied, NoSuchProcess, NotAllowed, Partial { done, total }, Os(i32), TooLarge }`, `validate_drop_in`, the 64 KiB frame and the exhaustive-variant tests. E29 beads rename `req` to `seq`, `Done` to `Ok`, `Error` to `Err`, `Files` to `File`, `Inodes` to `SocketOwners`, `SockDiag` to `NetlinkDump`. Added: `HelperRequest::CancelScan { seq: u32 }` (17 variants), `ProcFiles::ROOT = 1 << 11` (R-004 file set gains `root`, for PROC-078 and PROC-096). `ReadProcessDetail` is dropped from ADR-006. The 4 MiB cap is per reply message for streamed requests (`ScanStrings`, `NetnsSockDiagDump`) and total for the rest. tl-helper depends on tl-collect so `ScanStrings` reuses `proc/strings.rs`. The `Dump` fd is R-145.
- Basis: ADR-006, R-003, R-004, R-097, R-098, as2.36, E29-OI-1 [verified].
- Edits: ADR-006 and R-004 (`ReadProcessDetail` out, `CancelScan` and `root` in, the per-message cap); beads as2.36, qfx.4 to qfx.13, qfx.16.

### R-158 E02 workspace choices (OI-E20)
- Question: package name, `bench` lines, banned-API greps, zbus in tl-ui's closure, CI packages, version, rustfmt. Raised by E02 items 1 to 7, B1 item 3.
- Resolution: package `tl-app` with `[[bin]] name = "tasklens"`; ADR-002's command becomes `cargo tree -p tl-app`. E02-005 writes both `bench` feature lines up front, so E01-001 and E01-002 are closed as duplicates and 00-epics section 2 drops the E01 `Cargo.toml` chain edits. `check-deps` matches the literals `RuntimeHandle::spawn(` and `.with_poll_quota(` with the `// check-deps: allow <reason>` escape. If `zbus` shows up in tl-ui's closure through gpui-kit, ADR-001 check 2 narrows to "no direct dependency and no path that avoids gpui-kit". The Ubuntu CI package list of E02-010 and the lavapipe ICD path `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json` hold. `version = "0.1.0"`, `publish = false`. No `rustfmt.toml`.
- Basis: ADR-001, ADR-002, ADR-013, v9m.7 and wlf.3 text [verified].
- Edits: ADR-002 (`-p tl-app`, version); 00-epics section 2; beads wlf.3 and wlf.4 closed as duplicates of v9m.7, wlf.1 loses those edges.

### R-159 E33 binary and packaging choices (OI-E21)
- Question: the remaining tl-app choices. Raised by E33-OI-6, OI-7, OI-9, OI-10, OI-11, OI-13, OI-14, OI-15.
- Resolution: `--app-id <id>` isolates everything (D-016). The clap `about` line "Task manager for Linux", the per-argument help texts of E33-003 and the critical notification urgency are accepted. The desktop file keys of E33-013 hold; the icon installs to `/usr/share/icons/hicolor/scalable/apps/io.github.oysteinkrog.Tasklens.svg`. PKGBUILD: `pkgver=0.1.0`, built from the local checkout, `nvidia-utils` as `optdepends`, `license=('MIT')` (D-017). TaskSlinger INI import: plain booleans and in-range integers only until the owner supplies `UserSettings.ini` (D-018). The tray start in `main.rs` is E31-008, a chain edit depending on cr0.1, which also adds the `tl-tray` dependency line. `crates/tl-app/build.rs` is an E33 owner-only file.
- Basis: APP-052, APP-053, ADR-010, ADR-015, D-005, the cr0.4 to cr0.22 texts [verified]; D-016 to D-018.
- Edits: APP-052 (`--app-id` sentence); 00-epics section 2 (`build.rs`, the E31 `main.rs` chain edit); bead cr0.22.

### R-160 E01 bench harness choices (OI-E22)
- Question: the synthetic rules, report fields, cross-epic hooks and the soak launch. Raised by E01-OI-2 to OI-8, OI-10 to OI-16.
- Resolution: "X +- N" is a fresh uniform draw each second; random walk steps uniform in [-1, 1] for memory and [-0.05, 0.05] for load average; network bursts p = 0.1 per second else 0.01 Mbps; placeholder catalog 16 cores x 2 threads, disks 259:0 to 259:3, ifindex 2 and 3. `long_missed_frames` follows `missed_frames` in the JSON. MB = 1,000,000 bytes. `CpuPage.bench_show_both` behind `#[cfg(feature = "bench")]` in E24-A1. The E19 paint record sits behind `test-hooks`, so tl-ui's `bench = ["test-hooks"]`. The nine R-006 columns plus `power`, 40 app groups of 50 processes. `TlTreeTable::scroll_handle()` from E18-A1 and the table entity from E22-A1. `xtask bench` runs its build through `heavy-build` when `TL_BUILD_WRAP=1`. Gallery title "tasklens Gallery". Scene `soak` added to table (c), run `--duration 600 --warmup 0`. E01 owns `vendor/gpui-pre-linux/` and one root `Cargo.toml` chain edit. Phase-0 reports carry `gpu: null`; E01-023 depends on ndy.2. Collection cost = utime + stime of `tl-engine*` threads plus the probe child over 60 s. E20-008 writes `first_frame_ms`; E01-017 depends on it.
- Basis: UI-PRF-053, ADR-014, ADR-010, R-006, leader notes on yrf.7, yrf.9 and jqk.10 [verified].
- Edits: UI-PRF-053; table (c) `soak`; 00-epics section 2; beads crw.1, rsf.1; the edges in the engine file's Edges section.

### R-161 E31 tray choices (OI-E23)
- Question: the tray type home, the chain edits, the icon pixels, menu plumbing and wording. Raised by E31 items 1 to 11, E33-OI-14.
- Resolution: `crates/tl-model/src/tray.rs` plus one `pub mod tray;` line, after E03-A2; E03's frozen module list gains `tray.rs`. `raster::render(max: f32)` and the model channel `bounded(2)` with `try_send`. E31-008 is the allowed `main.rs` chain edit; E31-009 may chain-edit E20's `window/main.rs` (close decision row 1) and `window/titlebar.rs` (minimize to tray), depending on jqk.1. Two pre-rendered files `assets/icons/tray-22.argb` and `tray-44.argb` under E15's assets, `include_bytes!` in tl-app. `PerfAddTrayGraph { source }` is declared in `tray_bridge.rs`; the row menu dispatches `AddTrayGraph { source, column }` and `RemoveTrayGraph { source, column }` built at menu time; process row `process:<pid>`, app row `app:<id>`, unit row `app:<unit>`; the PROC-094 name-filter mode is reached from the minigraph "Add Process Name Graph" item. Menu text "Accent (theme default)" (UI-SHL-085). UI-FND toast rule 8 holds; UI-PRF-051's "shown on the next render" sentence goes. `TrayDemand` per R-156; `VRAM Usage` and `GPU temp` tooltip forms; busiest disk active time and the first GPU in sidebar order; five tooltip lines joined by `\n` (PERF-089 and UI-PRF-051 win; APP-047 and UI-SHL-113 follow). Assume a watcher is available until the first ksni callback says otherwise.
- Basis: ADR-001, UI-SHL-113, UI-PRF-050, UI-PRF-051, UI-SHL-085, PERF-089, wox.3 [verified].
- Edits: 00-epics section 2; APP-047, UI-SHL-113, UI-PRF-051; beads wox.3, wox.10, wox.11; edge wox.11 on jqk.1.

### R-162 E04 collect-system choices (OI-E24)
- Question: the remaining E04 choices. Raised by E04-OI-3 to OI-10, OI-12.
- Resolution: MAC addresses scrubbed to `00:00:00:00:00:00` in fixtures (ADR-013). `/proc/vmstat` is dropped from v1 (ADR-004 and ADR-016 `Memory` rows; `parse/vmstat.rs` stays a stub). PERF-037 moves to E06 (hx1.18). E12 maps tl-collect readings to the tl-model totals; E04 keeps its own reading types. The ADR-016 latency formula (read and write ticks over read and write counts) stands and PERF-049's response-time line takes that form. Virtual name patterns are checked before `bridge/` (PERF-054). Visibility groups: Ethernet and USB network to Wired; Wi-Fi and Cellular to Wireless; Tunnel and VPN to VpnTunnel; Virtual and Bridge to Virtual; Other to Other; the mapping lives in E12-014. `mpqemubr` joins `VIRTUAL_NAME_PREFIXES` once seen on a Multipass install: new unverified entry U-52 (the engine file called it U-51). Build lines are the leader's pass.
- Basis: ADR-013, ADR-016, PERF-004, PERF-049, PERF-054 [verified].
- Edits: ADR-013, ADR-004, ADR-016, PERF-049, PERF-054, 00-epics section 6, U-52; bead hjn.17.

### R-163 E05 collect-process choices and the P2 column readers (OI-E25)
- Question: the package index, the P2 per-process columns and the smaller choices. Raised by E05 items 5, 7 to 13.
- Resolution: new file `crates/tl-collect/src/pkg.rs`, new bead E05-030 (P2): batched `pacman -Qoq <paths…>` for unseen exe paths debounced 2 s, the `/var/lib/pacman/local/<pkg>-<ver>/desc` parser (`%VERSION%`, `%DESC%`, `%PACKAGER%`, `%BUILDDATE%`, `%URL%`), cache keyed by exe path and mtime, invalidated on the `local` directory mtime; `dpkg -S` and `rpm -qf` behind the same trait at P4. The PROC-078 integrity check is `pacman -Qkk <pkg>` run by the detail worker. SYS-020's Publisher uses the same index. New bead E05-031 (P2) `proc/extras_p2.rs`: `statx` mtime and size of the exe, ELF class and machine, `sessionid`, `attr/current`, elevation and sandbox rules (PROC-028), scheduling policy text, `oom_score`, `oom_score_adj`, TTY decode, `cwd`, context switches over `task/*/status` for `detail_keys`; values in `ProcessSnapshot.extras_p2: Arc<[ProcessExtrasP2]>` filled only for `detail_keys`; `ProcessExtrasP2` is defined in tl-model `process.rs`. PROC-005's inotify sentence is dropped; the app scan runs every sweep. "Not dumpable" when `/proc/<pid>` is owned by uid 0 while the real uid is not 0, else "Not reported" (PROC-082). U-21 belongs to E09 with the rule "`irq` stays 0 across 10 samples"; no `/proc/config.gz` read. E05-016 checks Mission Center's `matching.rs:10-24` table first. The reducer computes the PROC-034 power estimate: new bead E09-030 (P2, `reducer/power.rs`) writes `ProcessExtrasP2.power_w: Option<f32>` for every row and the 2-minute trend.
- Basis: PROC-005, PROC-027, PROC-028, PROC-034, PROC-036, PROC-078, PROC-082, SYS-020, ADR-008 [verified].
- Edits: PROC-005, PROC-078, PROC-082; 00-epics section 2 (`pkg.rs`, `proc/extras_p2.rs`, `reducer/power.rs`); beads as2.10, yms.19; new beads E05-030, E05-031, E09-030.

### R-164 E06 collect-netsys choices (OI-E26)
- Question: the remaining E06 choices. Raised by E06 items 5, 8, 9, 10.
- Resolution: the xe VRAM total sysfs file is unverified: new entry U-53 (the engine file called it U-52); E06-023 leaves VRAM total out for xe until it is named. E06 owns the PERF-037 readers (R-162). E12-027 may depend on hx1.26 directly (named exception). Six small choices hold: IPv6 scope as interface name; autostart precedence NotInThisDesktop, then MissingProgram, then Disabled; 64 KiB read caps; fdinfo busy summed and clamped to 100%; first attribution pass `WarmingUp`; pacman.log timestamp form.
- Basis: PERF-064, PERF-075, SYS-022, PROC-032 [verified].
- Edits: U-53; edge hjn.31 on hx1.26.

### R-165 E07 ops choices (OI-E27)
- Question: the remaining tl-ops choices. Raised by E07-O2, O3, O5, O6.
- Resolution: autostart re-enable follows SYS-022 (delete the user file when it carries `X-Tasklens-Override=true`, else set `Hidden=false` and remove `X-GNOME-Autostart-enabled=false`); the ADR-016 row matches. `SCHED_RESET_ON_FORK` is set for every FIFO and RR request. Unset or empty `DISPLAY`, `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are omitted from the `pkexec env` line (PROC-095). New overrides get mode 0600, existing mode is kept, desktop files over 1 MiB are refused.
- Basis: SYS-022, ADR-016, ADR-006, R-038, PROC-095, ADR-011 [verified].
- Edits: ADR-016 autostart row; ADR-006 reset-on-fork sentence; PROC-095.

### R-166 Engine runtime and reducer choices from B3 (OI-E28)
- Question: the remaining engine-core and E12 choices. Raised by B3 items 1 to 4, 7, 8, 15, 16 (b), E12-OI-3, OI-8 to OI-11.
- Resolution: `WorkerIo.control: watch::Receiver<WorkerControl>` with `reset_generation` and the newest-64 `QueuedCommand` queue. `EngineConfig` per R-150. The E08-003 payloads are the contract for B4. `EngineMsg::Tick` every 1 s. The E05 sweep fills `ProcessSweep.file_nr` from `/proc/sys/fs/file-nr`; the reducer copies it into `TableTotals.handles`. `dbus/mod.rs` owns `ManagerConnSlot` and `SharedHandles`; `WorkerIo.shared`; `EngineHooks.action_gate: Fn(HookCtx) -> Box<dyn ActionGate>`. Service jobs have no client-side timeout; the method call has 10 s (user) or 120 s (system); ADR-006's 30 s row changes. `scripts/ci.sh` gets `cargo build -p tl-gpu-probe --features fake` before the engine tests, written by E02-009. `cpu_period` and `memory_period` return 1 s when `Demand.process_table` is true. The reducer keeps the running maximum of `Committed_AS` since start as `MemoryInfo.peak_committed_bytes`; the inventory worker leaves it `WarmingUp`. "Session type" shows the raw `XDG_SESSION_TYPE`, "Windowing system" the display name (PERF-097). Inventory rows carry `device: None` from the worker; the reducer matches disk name, PCI slot or interface name against the catalog. AMD and Intel GPUs use `DeviceIdentity::Gpu { uuid: [0; 16], pci_bus_id }`; process GPU columns match on the full identity. Battery demand, 24 h history at 288 points and the power-profiles-daemon bus names hold.
- Basis: ADR-004, ADR-005, ADR-006, ADR-008, ADR-013, PERF-078, PERF-097, PERF-098 [verified].
- Edits: ADR-004 (control channel, `Tick`, `SharedHandles`, `HookCtx`); ADR-006 job row; ADR-008 process-table note; PERF-097; PERF-098; bead v9m.11.

### R-167 E14 D-Bus worker choices (OI-E29)
- Question: the remaining E14 choices. Raised by E14 items 2, 4, 5, 6, 8, 9, 10, 12, 14, 15, 16.
- Resolution: one `WorkerSpec` line per worker in `all_workers()`; the journal opener is one allowed line in `bridge.rs`. The systemd worker reads unit `memory.stat` and `cpu.stat` with E05-A2's readers; E05-A1 and E05-A2 join E14's anchor row. The engine writes its connection's unique name into the KWin script as `__TL_BUS_NAME__` and accepts `Update` only from the owner of `org.kde.KWin`. The extra KWin signals `minimizedChanged`, maximize change, `keepAboveChanged` and `interactiveMoveResizeFinished` join PROC-052. Journal paging uses `journalctl ... --reverse --cursor=<oldest> -n 201`. `Access(OwnOnly)` for system units only; `bounded(64)`; missing `PRIORITY` = 6. No column list in `SysTableQuery`; the Services filter matches every text column of a unit; sort orders follow the SYS-003 tables (APP-028). `<rand>` is 8 lowercase hex digits from 4 bytes of `/dev/urandom` (table (c), SYS-023). A missing manager is retried on `RefreshNow` or every 60 s. `Update` payloads over 1 MiB are rejected, strings cut at 4 KiB. OpenRC word mapping and 5 s refresh are P4.
- Basis: ADR-004, ADR-010, PROC-052, SYS-003, SYS-005, SYS-018, SYS-023, APP-028, R-138 [verified].
- Edits: ADR-004 KWin paragraph; PROC-052; APP-028; table (c) and SYS-023; 00-epics section 4 (E14 anchor row).

### R-168 E29 helper and setup choices (OI-E30)
- Question: the remaining E29 choices. Raised by E29-OI-8, OI-10 to OI-13, OI-15 to OI-19.
- Resolution: the helper cgroup validation rule of E29-003 holds and E07 exposes write helpers that take a validated dirfd. `render(&RecoveryEdit) -> String` lives in `crates/tl-ops/src/dropin.rs`, an E29-owned file inside tl-ops, used by E11-020. E16 and E23 build the "Retry as administrator" and "Grant access" buttons from their own specs and E29-029 tests them. The chain edits E29-017 and E29-018 are allowed. `tl-engine/src/helper/setup.rs` is an E29 file; the `TASKLENS_SETUP_ROOT` test mode is honoured only when not root (table (c)). The setup script is embedded with `include_str!` and shown whole. The three polkit prompt texts are the E29-021 texts as written (D-019). E33-015 depends on E29-021 and E29-022. Setup cards follow UI-PRF-052 and the frame UI-SHL-094. E29-030 may chain-edit `window/main.rs` and uses the first-focus modifier rule on X11 too.
- Basis: ADR-006, R-004, SYS-013, PERF-108, table (c), UI-SHL-094, UI-PRF-052 [verified]; D-019.
- Edits: 00-epics section 2 (`tl-ops/src/dropin.rs`, `helper/setup.rs`, the two worker chain edits, the `window/main.rs` E29 row); table (c) `TASKLENS_SETUP_ROOT`; edges cr0.18 on qfx.24 and qfx.34.

### R-169 E30 history choices (OI-E31)
- Question: the on-disk block fields and format details, and the two data shapes. Raised by E30-OI-2, OI-3, OI-5 to OI-8.
- Resolution: `HistMeta { exe_path, user, uid, system }`, `Block { gap_before, mem_total_kib, catalog: Option<Vec<HistDevice>> }`, `HistDevice { idx, kind, parent, name }` hold and ADR-012 records them now. The nine on-disk details hold: little-endian header offsets 0, 4, 6, 14, 22; lowercase hex boot dir; postcard footer with `TLHF` magic; `FieldPatch` of `Option`s; keyframe-then-started index space; `MetaId` per segment; NaN gap marker; non-`Valid` stored as 0; `deleted-keys.bin` mode 0600. The history chart stacks the three largest per-process values per bucket plus "other" as the sum of the rest, drawn in the metric colour (D-020); header totals for a selected range are averages over the range. A query is resent after 1,000 ms without an answer. Copy details on a history row prints Name, PID, Parent PID, CPU, Memory, Disk, Network and GPU; Copy all metrics prints every visible history column; never a command line. E20 declares `ToggleHistoryMode` and selects the Processes tab, E30-A3 flips the mode; E16's aggregator sets `Demand.recording` from `history.enabled`. Axis labels "-M:SS" under an hour; the disk-limit input snaps to multiples of 64.
- Basis: ADR-012, R-136, PROC-089, UI-PRC-030 to UI-PRC-036, UI-SHL-023 [verified]; D-020.
- Edits: ADR-012; PROC-089 and UI-PRC-032 (stack rule); UI-PRC-036 (range totals are averages).

### R-170 Smaller tl-model and engine contract additions asked by the UI batches (OI-E32)
- Question: the engine-side half of UI open items that add a field, constant or command. Raised by E19-OI-1 to OI-3, B5 item 6, E24 item 9, E27 items 3, 5 and 7, E28 items 4 and 11, E22 item 10, E25-OI-6, E23 items 3 and 6, B7 item 4.
- Resolution:
  - `MetricId` constants appended in E03-004: `CPU_POWER_PKG = 5`, `CPU_POWER_CORE = 6`, `CPU_CLOCK_MAX = 7`, `MEM_DIRTY = 14`, `GPU_MEM_SHARED = 47`, `GPU_MEM_CLOCK = 48`, `GPU_PCIE_RX = 49`, `GPU_PCIE_TX = 52`, `GPU_FAN = 53`, `GPU_ENGINE_MEMCTL = 54`, `GPU_ENGINE_JPEG = 55`, `GPU_ENGINE_OFA = 56`, `GPU_ENGINE_OTHER = 57` (busiest vendor-specific engine, named by `GpuDeviceInfo`), `FAN_PWM = 58`, `BATTERY_POWER = 61` (50, 51 and 60 are taken). `ALL` grows to 39 entries.
  - `TIER_1HZ` is bit 15 of `MetricId` (`0x8000`) with `with_tier_1hz()`, `is_tier_1hz()` and `base()`; metric numbers stay below 0x8000; the reducer publishes slow views in `SeriesSet.slow` with the flagged id from P2.
  - `ceil_pow2(u64) -> u64` and `ceil_125(f64) -> f64` in `tl_model::units` with tests `ceil_125(1234) = 2000`, `ceil_125(60) = 100`, `ceil_125(0.3) = 0.5`, `ceil_pow2(3 MiB) = 4 MiB`.
  - `SystemSnapshot.color_scheme: u8` (portal `color-scheme` 0, 1, 2) from `org.freedesktop.portal.Settings` `Read("org.freedesktop.appearance", "color-scheme")` plus `SettingChanged`, read by E14's session-bus opener worker, shipped as `SourceUpdate::ColorScheme(u8)`.
  - `SensorReading.fan_connected: bool`: true when the fan has reported a non-zero RPM in this session or carries a label.
  - `SysColumnId` in `table.rs` with string ids for the 19 Services columns (UI-SYS-003), the 7 Startup ids (`name`, `publisher`, `status`, `impact`, `type`, `location`, `command`) and the 17 Connections ids (`proto`, `local`, `foreign`, `state`, `pid`, `process`, `user`, `family`, `local_port`, `foreign_port`, `recv_q`, `send_q`, `interface`, `inode`, `service`, `remote_host`, `netns`), with `as_str` and `from_str`; `SysTableQuery.sort` and the `[columns.*]` settings use it.
  - `ProcessorInfo.bits: InvField<u8>`: 64 for `x86_64`, `aarch64`, `riscv64`, `ppc64`, `ppc64le`, `s390x`, `loongarch64`, `mips64`; else 32.
  - KWin commands have no result event: "Process window shown." on send, "No window found" when the window is absent from `WindowsSnapshot`; a failed one-shot script is logged at `warn`.
  - `ActionError::UnitJob { result: Arc<str>, error_name: Option<Arc<str>> }`; E11-011 fills `error_name` from the zbus error on a failed method call and `result` from `JobRemoved` otherwise.
  - `UnitEnable` and `UnitDisable` on a masked unit call `UnmaskUnitFiles` first inside the same action; one summary event.
  - `LaunchWhat::DbusActivate { bus_name: Arc<str>, object_path: Arc<str>, action: Arc<str>, parameter: Option<Arc<str>> }`; the launcher calls `org.freedesktop.Application.ActivateAction`, 2 s deadline, P3 with PERF-061.
  - `AppEnv.run_history: Option<Box<dyn RunHistoryStore>>` with `load()` and `save()` built on `write_atomic` in tl-app; `executedAt` is RFC 3339 local time from jiff in tl-ui; no clock closure.
  - Grant access: no flag on `DetailDemand`; the UI sends `Command::DetailRefresh { target, tab }` once when `HealthSnapshot.helper` becomes `Unlocked`; E29-015 routes Restricted targets through the helper.
  - No X11 keep-above command in v1: KDE goes through `KWinCommand::KeepAbove`, other desktops show the pin disabled (R-073); APP-009 and UI-SHL-005 drop the "tl-engine X11 helper" sentence.
- Basis: ADR-003, R-073, UI-PRF-001, UI-PRF-006, UI-FND-042, UI-SYS-003, PERF-061, UI-PWN-018, as2.6, as2.8, as2.14, as2.18 [verified]; portal `Settings` interface names [inferred].
- Edits: ADR-003, ADR-004, ADR-005, APP-009, UI-SHL-005; beads as2.6, as2.14, as2.16, as2.18, as2.19, as2.22, 8hz.5, 8q0.24, hjn.9, hjn.19, i3o.13, i3o.15, y6k.24, qfx.18, cr0.11, fut.1.

### R-171 `UiInit` and `AppEnv`: everything tl-app hands to `TasklensUi::init` (OI-U01)
- Question: ADR-009 gives `TasklensUi::init(cx, client, settings)` and ADR-011 one persist callback; the UI beads need state, the themes text, theme persistence, the `AppRequest` receiver, queued toasts and about ten things only tl-app has. Raised by B5-4, B5-5, B5-29, B7-E21-1, B7-E21-2, B7-E21-5, B7-E21-9, B7-E23-3.
- Resolution: `TasklensUi::init(cx: &mut App, client: Arc<dyn EngineClient>, init: UiInit)` as E16-A1 writes it, plus `env: AppEnv`. In `crates/tl-ui/src/app.rs`: `UiInit { settings: Settings, state: UiState, themes_json: Option<Arc<str>>, persist: Box<dyn Fn(&Settings, &UiState)>, persist_themes: Box<dyn Fn(PersistTheme)>, app_requests: async_channel::Receiver<AppRequest>, startup_toasts: Vec<(ToastKindTag, SharedString)>, env: AppEnv }`; `BuildInfo { version, git_hash, build_date, gpui_rev, licenses_json: &'static str }` (cargo-about output embedded by tl-app); `CrashReport { path: PathBuf, text: Arc<str>, at: SystemTime }`; `AdapterInfo { name, backend, device_type: String, pci_bus_id: Option<String> }` (R-237); `RunHistoryEntry { command: String, executed_at: String /* RFC 3339 */, elevated: bool }`; `ImportSummary { settings: u32, themes: u32 }`; `AppEnv { debug: bool, config_dir, state_dir, logs_dir: PathBuf, build: BuildInfo, pending_crash: Option<CrashReport>, mark_crash_seen: Arc<dyn Fn()>, write_settings_now: Arc<dyn Fn(&Settings) -> Result<(), String>>, set_autostart: Arc<dyn Fn(bool /* enabled */, bool /* minimized */) -> Result<(), String>> (R-236), global_shortcut: Option<GlobalShortcutApi>, import_taskslinger: Arc<dyn Fn() -> Result<ImportSummary, String>>, list_adapters: Arc<dyn Fn() -> Vec<AdapterInfo>>, run_history: Option<Box<dyn RunHistoryStore>> }`; `trait RunHistoryStore { fn load(&self) -> Vec<RunHistoryEntry>; fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String>; }`; `GlobalShortcutApi { get: Arc<dyn Fn() -> Option<String>>, set: Arc<dyn Fn(Option<&str>) -> Result<(), String>> }`; `impl Global for AppEnv {}`. `init` step 8 installs `AppEnv` as a Global next to `BootSettings`. Every closure runs on the main thread; none is `Send`. `PersistTheme` is declared in `app.rs` by E16-A1 and moved to `theme/user_store.rs` with a re-export by E17-008. Reset settings calls `write_settings_now` before replacing `TlSettings` and shows "Failed to reset settings" on `Err` with nothing changed. `executedAt` is RFC 3339 local time formatted with `jiff` in tl-ui (R-142). The adapter probe runs in tl-app. A `FakeEngine` test gets `AppEnv::test()` with empty closures and `debug = false`.
- Basis: ADR-001 tl-ui row, ADR-009, ADR-011, UI-SHL-072, UI-SHL-092, UI-SHL-093, UI-PWN-028, APP-053, PROC-095 [verified]; field types [inferred].
- Edits: ADR-009 "Entry point" and ADR-011 (`UiInit`, `AppEnv`, `persist_themes`); beads fut.1, bj6.3, bj6.8, bj6.14, bj6.17, bj6.42, bj6.44, bj6.45, gma.31.

### R-172 `tl_ui::run(UiLaunch) -> UiExit` (OI-U02)
- Question: tl-app cannot depend on gpui-kit, so who opens the application? Raised by B5-38.
- Resolution: R-153 holds. UI half: E16-012 builds `tl_ui::run(UiLaunch) -> UiExit`; `debug` and `on_quit` are not in `UiLaunch` (`debug` is `AppEnv.debug`); there is no `platform` field (R-153).
- Basis: B5-38, leader notes [verified].
- Edits: edge cr0.1 on fut.15.

### R-173 Crates the UI batches asked for (OI-U03)
- Question: five UI beads need a crate that is not in the tl-ui row of ADR-001. Raised by B5-2, B5-22, B5-30, E19-4, B7-E21-9.
- Resolution: R-142 and R-144 settle the rows. `UiInit.app_requests` stays an `async_channel::Receiver`; E15-003 uses `unicode-segmentation`; E16-005 stops and reports if it needs `image`; tl-ui never needs `wgpu`; `FrameStats` reports 0 thread CPU until `tl_model::time::thread_cpu_ns()` exists.
- Basis: ADR-001 [verified].
- Edits: none here.

### R-174 `RefreshCurrentTab` is the F5 action (OI-U04)
- Question: UI-SHL-120 names F5 `RefreshNow`, which is also `Command::RefreshNow`; E20-002, E27 and E28 use `RefreshCurrentTab`. Raised by B7-E20-1.
- Resolution: `RefreshCurrentTab`, declared in E20-002 `window/actions.rs`, bound to F5 in the Global context.
- Basis: INDEX.md action vocabulary, UI-SYS-085 [verified].
- Edits: UI-SHL-120.

### R-175 `ShowSystemInformation` is the Help action (OI-U05)
- Question: UI-SHL-041 says `OpenSystemInformation`; INDEX.md and UI-SYS-096 say `ShowSystemInformation`. Raised by B7-E21-3, B8-E28-6.
- Resolution: `ShowSystemInformation`, declared in E20-002, dispatched by the Help menu, handled by E28-A3 by switching to the Information tab.
- Basis: INDEX.md, UI-SYS-096 [verified].
- Edits: UI-SHL-041.

### R-176 `WindowKey::Main` and the caption on `ToggleAlwaysOnTop` (OI-U06)
- Question: the pin action carries a `WindowKey`, but there is no key for the main window, and the KWin keep-above command needs the window caption. Raised by B7-E20-2, B7-E20-3, B5-27.
- Resolution: E16-008 adds `Main` to `WindowKey`; `Main` is never stored in `WindowRegistry` and `cap_class()` returns `None` for it. The action becomes `ToggleAlwaysOnTop { window: WindowKey, caption: SharedString }` with `#[action(no_json)]`. The caption is the exact window title at dispatch time; the main window passes `"tasklens"`. E20-022 handles `Main` only. New bead E20-029 (P2, `window/main.rs` handler arm and `components/window_frame.rs` pin button; depends on jqk.24, fut.12, y6k.2) handles secondary windows: it sends `KWinCommand::KeepAbove { caption, on }` and keeps the pin state in memory per key (R-073). Without a KWin bridge the pin is disabled with tooltip "Not supported by this desktop".
- Basis: UI-FND-021, UI-SHL-005, R-073 [verified]; the caption field [inferred from PERF-083].
- Edits: UI-FND-021 and UI-PWN-001 (`caption`); bead fut.12; new bead E20-029.

### R-177 No X11 keep-above in v1 (OI-U07)
- Question: APP-009, UI-SHL-005 and PERF-093 name an X11 `_NET_WM_STATE_ABOVE` path; tl-model has no command for it and gpui-pre 0.3.7 exposes no such call. Raised by B7-E20-4, E26-OI-5.
- Resolution: dropped for v1. On X11 the pin, View > Window > Always On Top and the mini window's `ToggleMiniAlwaysOnTop` are disabled with tooltip "Not supported by this desktop". KDE through the bridge is the only platform with keep-above (R-170 confirms no engine command).
- Basis: ADR-004 and ADR-010 (no X11 EWMH fallback in v1), R-046, R-076 [verified].
- Edits: APP-009, UI-SHL-005, UI-SHL-036, UI-PRF-045, R-073: the X11 clause goes.

### R-178 `QuitMode::Explicit` exists in gpui-pre 0.3.7 (OI-U08)
- Question: does the pinned gpui let the app survive its last window closing? Raised by B7-E20-5.
- Resolution: yes. `gpui-pre-0.3.7/src/app.rs:389` defines `QuitMode { Default, LastWindowClosed, Explicit }`; `Application::with_quit_mode` (line 227) and `App::set_quit_mode` (line 1754) set it. E16-012 calls `.with_quit_mode(QuitMode::Explicit)` before `run`; E20-A1 drops its fallback clause. The UI calls `cx.quit()` itself for `UiExit::Quit`.
- Basis: gpui source on disk, `docs/research/claude-gpui-apis.md:58` [verified].
- Edits: beads jqk.1, fut.15.

### R-179 Actions declared once, outside the page that uses them (OI-U09)
- Question: several actions are named by more than one epic; who declares each? Raised by B5-26, B5-28, B8-E22-15, E26 note.
- Resolution: one declaration each, everyone else imports. `StartHelperSession`, `EndHelperSession`: E16-A3, `components/shield.rs` (E20 `actions.rs` re-exports). `CloseSecondaryWindow`: E16-008, `components/window_frame.rs`, Ctrl+W in secondary windows, independent of E20's `CloseWindow`. `ToggleHideSystem`, `CycleViewMode`, `SetViewMode`, `FocusFilter`, `ClearFilter`, `StartFilter` routing: E20-002, `window/actions.rs`; E22 builds the buttons only. `ResetMinigraphLayout`: E26-002, `pages/minigraph/actions.rs`; E21-003 dispatches it. Table actions (`MoveFocus`, `CopyCell`, `ResetColumns`, `SetColumnResize`, and the rest): E18-A1, `table/actions.rs`. `EnableAllColumns { table: TableId }`: E18 (R-180), `table/actions.rs`. All in the one `tasklens` namespace (R-128).
- Basis: INDEX.md action vocabulary, R-128 [verified].
- Edits: UI-PRC-009 Build line and UI-PRC-019: the shell actions are imported from `window/actions.rs`.

### R-180 `EnableAllColumns` belongs to the table (OI-U10)
- Question: the Debug menu item (UI-SHL-042) changes column visibility, which is E18 state. Raised by B7-E21-8.
- Resolution: E18-A2 declares `EnableAllColumns { table: TableId }` in `table/actions.rs` and handles it in `TlTreeTable` by setting every column of the owner's column set visible and persisting through `[columns.<table>]`. E21's Debug menu dispatches it with the active tab's `TableId`; on a tab without a table the item is disabled with tooltip "This tab has no table".
- Basis: UI-SHL-042 [verified]; placement [inferred from UI-SHL-034].
- Edits: bead rsf.2 (action, handler, test `enable_all_columns_shows_every_column`); the E21 Debug menu bead dispatches only.

### R-181 The column picker file lives in `table/` (OI-U11)
- Question: 00-epics section 2 puts `pages/processes/columns_window.rs` under E18, but that directory is created by E22-A1, which depends on E18-A2. Raised by B5-3.
- Resolution: `crates/tl-ui/src/table/columns_window.rs`, owned by E18-A2. The picker is table-generic.
- Basis: 00-epics section 2, R-106 [verified].
- Edits: 00-epics section 2 (E18 `src/table/...` row); UI-PRC-007 Build line.

### R-182 Scrollbar width is 16 (OI-U12)
- Question: processes.md "Shared geometry" and the UI-PRC-001 diagram say 12; UI-PRC-001 anatomy, UI-PRC-010 and UI-FND-003 say 16. Raised by B5-14.
- Resolution: 16 logical px at 100% scale for every scrollbar. UI-FND-003 is the component authority.
- Basis: UI-FND-003, UI-PRC-010 [verified].
- Edits: processes.md "Shared geometry" and the UI-PRC-001 diagram.

### R-183 Column picker: heading rows 28, filter md 28 (OI-U13)
- Question: UI-PRC-007 gives group headings 24 px and the filter as both "sm" and "md, height 28"; `uniform_list` needs one row height. Raised by B5-32, B5-33.
- Resolution: every picker row is 28 px, group headings included (heading text bottom-aligned); the filter is `TlTextInput` md, 28 px.
- Basis: UI-PRC-007 anatomy, gpui `uniform_list` fixed row height [verified]; heading height [inferred].
- Edits: UI-PRC-007.

### R-184 Delegate trait additions and header-menu actions (OI-U14)
- Question: E18-005 adds `table_version()` and `guide_lines()` to the UI-PRC-001 delegate; the header menu items and `OpenProperties` need a handler. Raised by B5-16, B5-34.
- Resolution: `TreeTableDelegate` gains `fn table_version(&self) -> u64` and `fn guide_lines(&self) -> bool`. `TlTreeTable` handles `ResetColumns { table }` itself when the owner called `set_default_layout`, `SetColumnResize` (writes `tables.column_resize` through `TlSettings`), `HideColumn { col }`, `CopyCell { col }`, `CopyRow`, `CollapseAll` and `ResizeColumnsToFit`, and routes `OpenProperties` to `delegate.on_activate()`. Owner pages implement `on_activate`, `context_menu` and the copy formats only. UI-SHL-034 dispatches with the active tab's `TableId`.
- Basis: UI-PRC-001, UI-PRC-006, UI-SHL-034, B5-16 [verified].
- Edits: UI-PRC-001 (two trait methods); beads pdu.1, hk6.1, 9o0.1.

### R-185 Icons the foundation adds or drops (OI-U15)
- Question: E18-009 needs Lucide `arrow-up` and `arrow-down`, which UI-FND-003 does not list; seven Lucide icons are outside the kit bundle; `shield-locked.svg` is in the E15-A1 list but UI-FND-028 uses Lucide `shield`. Raised by B5-35, B5-37, B5-17.
- Resolution: UI-FND-003's Lucide list gains `arrow-up` and `arrow-down` (sort indicators). E15-005 embeds `x`, `pin`, `pin-off`, `lock`, `shield`, `shield-check`, `rotate-ccw-clock`, `arrow-up`, `arrow-down` with the kit's `icon_assets!` macro. `assets/icons/shield-locked.svg` is not created; the locked shield is Lucide `shield`.
- Basis: UI-FND-003, UI-FND-028, UI-PRC-005 [verified].
- Edits: UI-FND-003; beads ug9.1 (drop `shield-locked.svg`), ug9.8 (add the arrows).

### R-186 `TlTreeTable::scroll_handle` for the bench (OI-U16)
- Question: the E01 `table-2000` scene must scroll the production process table. Raised by B8-E22-19, leader notes.
- Resolution: E18-A1 exposes `pub fn scroll_handle(&self) -> UniformListScrollHandle` (a clone; `Rc`-backed). E22-A1 exposes `ProcessesPage::scroll_handle(&self, cx: &App) -> UniformListScrollHandle` next to `table()`, with test `scroll_handle_drives_table`. E01-008 calls the page method and drives it with `scroll_to_item_with_offset`.
- Basis: gpui `UniformListScrollHandle(pub Rc<RefCell<..>>)`, UI-PRC-010 Build line [verified].
- Edits: bead rsf.1; edge wlf.10 on pdu.1.

### R-187 First visible row across a scale change: one E18 bead (OI-U17)
- Question: UI-SHL-130 step 5 and test `scale_keeps_first_row` belong to `tree_table.rs`, and E18 has no bead for them. Raised by B7-E20-7.
- Resolution: new bead E18-017 (P1, `table/tree_table.rs`, depends on rsf.1): `TlTreeTable` gains `pub fn first_visible_row(&self) -> Option<(RowId, usize)>` and `pub fn restore_first_visible_row(&mut self, anchor, cx)`, which calls `scroll_to_item_with_offset(ix, ScrollStrategy::Top, 0)` after the new metrics are applied. E20-013 calls them around the metrics re-apply, for the active table only. Test `scale_keeps_first_row` lives in the new bead.
- Basis: UI-SHL-130 step 5, UI-PRC-010 anchor [verified].
- Edits: new bead E18-017; edge jqk.15 on it.

### R-188 One filter debounce, in the shell (OI-U18)
- Question: the shell debounces 100 ms before `FilterChanged` (UI-SHL-024) and E22-024 debounces another 100 ms. Raised by B7-E20-6.
- Resolution: E22-024 drops its own debounce and sends `Command::SetTableQuery` on every `FilterChanged`. UI-PRC-016's "100 ms debounce" is the shell's.
- Basis: UI-SHL-024, UI-SYS-002 [verified].
- Edits: UI-PRC-016 Interaction and Build; bead pdu.27 (test renamed `filter_changed_sends_new_generation`).

### R-189 `SHIPPED_PHASE` and End Process on app rows before P2 (OI-U19)
- Question: how code decides "a phase has not shipped", and what Shift+Delete does on an app or unit row in P1. Raised by B8-E22-4, B8-E22-17.
- Resolution: `pub const SHIPPED_PHASE: u8 = 1;` in `pages/processes/delegate.rs`, raised by E22-042 to E22-044 as each phase's columns land; columns and menu items with a higher phase are not built. In P1 `EndProcess` on an app or unit row is a no-op without a toast; E22-035 turns it into Kill App.
- Basis: UI-PRC-013 [verified]; the mechanism [inferred].
- Edits: none.

### R-190 Name cell geometry: gap 6 after the chevron and after the icon (OI-U20)
- Question: UI-PRC-002's app-row and unit-row sketches differ by 4 px and the UI-PRC-003 test formula omits the icon gap. Raised by B8-E22-2.
- Resolution: one rule for every row kind: indent = depth x 20; chevron 16 wide starting at indent + 6; gap 6; icon 16 (unit) or 24 (app); gap 6; text. For an app row at depth 0: chevron 6..22, icon 28..52, text 58 before the row's left padding of 20 (26..42, 48..72, 78 in the sketch). The unit sketch becomes icon 48..64, text 70.
- Basis: UI-PRC-002 app sketch [verified]; the generalisation [inferred].
- Edits: UI-PRC-002 unit sketch, UI-PRC-003 formula.

### R-191 Picker group of each process column (OI-U21)
- Question: PROC-024 gives 13 headings and the Objects members only. Raised by B8-E22-3.
- Resolution: E22-002's mapping: each PROC column section goes under its own heading; Start time, Type and the package and file columns under Image; User, PID, Parent PID and Session under Identity. The mapping is the source for `[columns.processes]` defaults and is recorded in UI-PRC-007.
- Basis: PROC-024 [verified]; the mapping [guess, writer-level].
- Edits: UI-PRC-007 (the mapping table from pdu.6).

### R-192 Subtree for End Process Tree and the bwrap first descendant (OI-U22)
- Question: UI-PRC-013 says the subtree comes from "the engine forest", which is not in the snapshot. Raised by B8-E22-8.
- Resolution: the UI walks `ProcessSnapshot.records` by `ppid`, matching parents by `pid` among non-exited records, breadth first, capped at 256 keys (the `ActionRequest` limit). For a bwrap row the first descendant is the first non-bwrap record in that walk, ties by lowest PID.
- Basis: ADR-005 256 targets [verified]; the walk [inferred].
- Edits: UI-PRC-013 Interaction.

### R-193 Flat `TlTreeTable` instead of `TlStaticTable` (OI-U23)
- Question: UI-PRF-030 names a sortable `TlStaticTable`; foundation.md has no such component. Raised by E25-OI-3.
- Resolution: no new component. The SMART attribute table and any other static table in the Performance tab is a `TlTreeTable` with a flat delegate (`RowKind::Plain`, no expansion, no selection actions), sorted by the delegate on header click.
- Basis: UI-PRF-030, UI-SYS-002 [verified].
- Edits: UI-PRF-030.

### R-194 Toolbar slots: E20 defines the trait, pages draw the buttons (OI-U24)
- Question: UI-SHL-010 draws slots 2 and 3 inside `TitleBarView` from a `ToolbarModel`; 00-epics gives E22 `toolbar.rs` and a `ToolbarContent` registration. Raised by B8-E22-11.
- Resolution: E20-003 defines `ToolbarContent` and `ToolbarModel` and draws only the slot frame; each page epic registers one `ToolbarContent` that draws its own buttons (E22-022 draws the Processes buttons). E20 draws no page button.
- Basis: 00-epics section 2 chain-edit rule, UI-SHL-010 [verified].
- Edits: UI-SHL-010.

### R-195 Toast top is 72 in every window for P1 (OI-U25)
- Question: the kit `NotificationSettings` is app-global, so the main window top 72 and the secondary-window top 42 (UI-FND-020 rule 9) cannot both hold. Raised by B5-8.
- Resolution: 72 x `ui_scale` in every window for P1 (66 with server decorations, app-wide). Rule 9 stays the P2 target: a kit change or `ToastQueue::new(window, cx, top)` taking a per-window top once E16-006's queue no longer goes through the kit global. No bead now.
- Basis: UI-FND-020 rule 9, UI-SHL-100, R-109, R-113 [verified].
- Edits: UI-FND-020 rule 9, UI-SHL-100; bead fut.9 (every caller passes the shell's value in P1).

### R-196 `message(&ActionError)`, reject texts and deadlines (OI-U26)
- Question: the docs give only the `PermissionDenied` text; the tracker also needs reject texts, deadlines and the short reason used in partial-failure summaries. Raised by B5-9, B5-23, B8-E22-6.
- Resolution, in `tl-ui/src/action_tracker.rs`. `message()`: `PermissionDenied` "Permission denied. This process belongs to another user."; `NoSuchProcess` "The process has already exited."; `Partial { done, total }` "Done for {done} of {total} targets."; `UnitJob { result, error_name }` "The service job ended with result: {result}."; `Os(code)` "The system call failed (error {code})."; `Internal(text)` "Internal error: {text}.". Reject toasts (Warning): `Busy` "Another action on this process is still running."; `TooManyTargets` "Too many processes selected. The limit is 256."; `IdentityChanged` "The selected process changed; wait for the list to update and try again."; `KernelThread` "Kernel threads cannot be signalled."; `Protected` "This process is protected. Confirm the warning to end it."; `NotOwner` "This process belongs to another user. Administrator access is needed."; `Unsupported` "This action is not available for this process."; `AlreadyRunning` "Already running" (Info). `deadline_for`: 2 s per target for signal, nice, affinity, ioprio and freeze writes; 120 s for every call that may show a polkit prompt (`SockDestroy`, `UnitWriteRecovery`, `UnitRemoveRecovery` included); `Dump` has no deadline, its progress toast "Writing core… 1.2 GiB" is the liveness signal. `short_reason(&err)` is the first sentence of `message()` lower-cased without its full stop ("permission denied").
- Basis: ADR-006 deadline table, ADR-015, PROC-058 [verified]; the new texts [guess]; `Dump` deadline [inferred from PROC-075].
- Edits: ADR-015 (`message()` table); R-121 accepted strings; beads fut.3, pdu.20.

### R-197 `ActionTracker` contract (OI-U27)
- Question: five batches need the same tracker API: per-request texts, a summary threshold, a Details dialog, a Progress toast without the auto-dismiss cap, result events by request id and a caller for `StatusChip::note_restricted_refusal`. Raised by B5-24, B7-E23-8, B8-E27-3, B8-E27-4, B7-E20-8.
- Resolution: E16-A3 as written with these fixed points. `ActionTracker::track(&mut self, request, queue: WeakEntity<ToastQueue>, texts: ToastTexts, cx) -> Result<ActionId, CommandError>`; `ToastTexts.toast_key: Option<ToastKey>` so grouped requests share one toast that replaces in place. `ToastTexts.summary_min_targets` defaults to 4 (R-114); process actions pass 2. A `ToastTexts` whose `started` is `None` pushes no Progress toast; E27's `SysActions` passes `None` when `pending_one` is empty. Progress toasts never auto-dismiss and do not count against the 4-visible cap. The tracker emits `ActionTrackerEvent::{Started(ActionId), Finished { id, ok, total, failed }}` through `cx.emit`. "Details" on a partial summary opens `TlDialog` titled "tasklens", non-destructive, one line per failed target "<target label>: <message(err)>", one button "Close". On `Failed(PermissionDenied)` when `HealthSnapshot.helper == HelperState::Missing`, the tracker calls `StatusChip::note_restricted_refusal(cx)` through the `ShellState` global if present. The D-Bus error name inside `UnitJob` is engine side (R-170).
- Basis: fut.3 Deliverable, R-114, UI-PRC-015, UI-SYS-007, UI-SYS-045, UI-SYS-046 [verified]; event enum and `toast_key` [inferred].
- Edits: UI-SYS-007 (Details dialog); beads fut.3, jqk.18, hk6.9, gma.5 to gma.7.

### R-198 Confirmation precedence when several rules match (OI-U28)
- Question: UI-PRC-014 gives only "protected wins over tree". Raised by B8-E22-7.
- Resolution: Protected, then End tree, then Kill app, then several targets, then another user's process. The winning row's title and buttons are used; the count N is still shown where the winning text has one.
- Basis: UI-PRC-014 [verified]; the order [inferred, most destructive first].
- Edits: UI-PRC-014.

### R-199 Copy details rounding and clipboard failure (OI-U29)
- Question: rounding and glyph words in the TSV, and whether a clipboard failure can be reported. Raised by B8-E22-9, B8-E27-2.
- Resolution: whole MiB rounded to nearest; TSV writes the words `restricted`, `n/a` and an empty cell for the other glyphs (R-050). `App::write_to_clipboard` returns `()` in gpui-pre 0.3.7 (`app.rs:1549`), so no failure can be detected. One wrapper `tl_ui::util::clipboard_write(cx, text) -> Result<(), ()>` returns `Ok(())` today; every copy path shows the success toast through it; the "Failed to copy to clipboard" and "Could not copy" texts stay in Fluent and unused.
- Basis: gpui source, R-050, APP-044 [verified].
- Edits: UI-PRC-015 and system-tabs.md footnote; beads pdu.2 (hosts the wrapper), hk6.8, hk6.14.

### R-200 Fluent key prefix and slug rule (OI-U30)
- Question: R-132 fixes `<area>-<entry>-<slug>` but not the area prefixes nor the slug form. Raised by B5-31, B8-E22-18.
- Resolution: areas `fnd` (foundation), `shl` (shell), `prc` (processes), `pwn` (process windows), `prf` (performance), `sys` (system tabs), `app` (tl-app strings passed through). The slug is the kebab-case of the first five words of the English text, letters and digits only, for example `prc-015-process-terminated`. A key that collides inside one entry gets a numeric suffix `-2`.
- Basis: R-132 [verified]; the rule [guess, writer-level, already used by every bead].
- Edits: R-132.

### R-201 Small texts no doc gives (OI-U31)
- Question: stepper button tooltips, the shield's starting-state tooltip, the TaskSlinger import failure toast. Raised by B5-20, B5-25, B7-E21-10.
- Resolution: stepper minus "Decrease", plus "Increase". Shield starting state tooltip "Waiting for authentication…". TaskSlinger import failure: Error toast "Could not import TaskSlinger settings"; success stays APP-034's "Imported N settings and M themes".
- Basis: R-121 [verified]; the two new texts [guess, writer-level].
- Edits: UI-FND-014, UI-FND-028, UI-SHL-040; R-121.

### R-202 "Grant access" without the ellipsis (OI-U32)
- Question: R-121 lists "Grant access…"; UI-PWN-018 and the PROC texts use "Grant access". Raised by B7-E23-7.
- Resolution: "Grant access". The button opens no dialog of its own (the polkit prompt is the system's), so the UI-FND-050 ellipsis rule does not apply.
- Basis: UI-PWN-018, R-050 [verified].
- Edits: R-121.

### R-203 Startup and connection action toasts (OI-U33)
- Question: no busy, unknown or summary strings exist for the startup, connection and the Reload, Reset failed and startup-type service actions. Raised by B8-E28-10, B8-E27-6.
- Resolution: R-114's pattern extended. Busy: "A startup action is already in progress.", "A connection action is already in progress.". Unknown: "Outcome unknown" for all. Services: "Reloading { $n } services...", "{ $n } services reloaded", "{ $ok } of { $n } services reloaded. { $failed } failed."; "Resetting { $n } services...", "{ $n } services reset", "{ $ok } of { $n } services reset. { $failed } failed."; "Changing startup type of { $n } services...", "Startup type changed for { $n } services", "Startup type changed for { $ok } of { $n } services. { $failed } failed.". Startup Apps: "{ $n } startup entries disabled", "{ $n } startup entries enabled", "{ $n } startup entries launched", "{ $n } startup entries deleted", and the "{ $ok } of { $n } startup entries disabled. { $failed } failed." forms. Connections: "{ $n } TCP connections closed", "{ $ok } of { $n } TCP connections closed. { $failed } failed.". No Progress toast for startup and connection actions (`ToastTexts.started = None`, R-197).
- Basis: R-114, UI-SYS-007, PROC-058 [verified]; the new strings [inferred from the pattern].
- Edits: R-114.

### R-204 Status text "| N selected" and the copy block for kernel sockets (OI-U34)
- Question: when the selected-count suffix appears on Startup Apps and Connections; what the copy block says for kernel-owned and unresolved sockets. Raised by B8-E28-8, B8-E28-14.
- Resolution: " | N selected" from 2 selected rows, as the Processes text (APP-029) does. Copy block: kernel-owned socket "PID: -" and "Process: No owner (kernel)"; unresolved (restricted) socket "PID: ?" and "Process: Restricted".
- Basis: APP-029, SYS-027, SYS-036, UI-SYS-049, UI-SYS-066 [verified]; the kernel and restricted lines [inferred from R-050].
- Edits: UI-SYS-049, UI-SYS-066, UI-SYS-067.

### R-205 Session type wording (OI-U35)
- Question: UI-SYS-088 shows "user (graphical)" but logind Type is x11, wayland, mir, tty or unspecified. Raised by B8-E28-5.
- Resolution: "<Class> (graphical)" for x11, wayland and mir; "<Class> (text)" for tty; "<Class> (<Type>)" otherwise. Formatted in the UI from the two raw `InventorySnapshot` strings.
- Basis: UI-SYS-088 [verified]; mapping [inferred].
- Edits: UI-SYS-088.

### R-206 Jump target texts for callers without their own (OI-U36)
- Question: `--select-pid`, the D-Bus `select-pid` action and the window picker reach the jump target without a "not visible" text. Raised by B8-E22-12.
- Resolution: `JumpTexts::default()` in E22-A3: not found "Process not found in current view" (PROC-071); not visible "Process not found in current view" with a "Show" button that clears the filter, turns Hide system processes off and selects the row; switched "Switched to Processes tab". Callers with their own texts pass theirs.
- Basis: PROC-071 [verified]; reuse for the hidden case [inferred].
- Edits: UI-PRC-022.

### R-207 Which block device the I/O priority "Scheduler:" line names (OI-U37)
- Question: UI-PRC-020 names "the active block scheduler" without saying which device. Raised by B8-E22-13.
- Resolution: the device holding the root filesystem, read from the snapshot (`DiskDetails.scheduler` of the system disk, R-139). The UI does not read `/sys`.
- Basis: UI-PRC-020 [verified]; choice [inferred].
- Edits: UI-PRC-020.

### R-208 Combo filter at 12 items or more (OI-U38)
- Question: UI-FND-017 says the filter appears "above 12 items" and gives the 12-item language list as an example that has one. Raised by B5-21.
- Resolution: 12 or more items show the filter.
- Basis: UI-FND-017 [verified; the example wins].
- Edits: UI-FND-017.

### R-209 Theme store needs: the animation requester, the E19 edge, new error and event variants (OI-U39)
- Question: the theme crossfade and preview coalescing need `AnimationDriver` to accept a requester that is not a view entity; E17-006 therefore depends on E19-A1; `ThemeError` and `ThemeEvent` gain variants the E21 toast mapping must cover. Raised by B5-18, B5-13, B5-19.
- Resolution: `AnimationDriver` accepts any `EntityId` in `request(id)` and gains `request_callback(id: EntityId, f: impl Fn(&mut Window, &mut App) + 'static)`. The theme store reserves one `EntityId` from a zero-sized entity it creates at init. E17-006 depends on E19-A1. `ThemeError` gains `BuiltinCannotBeDeleted` and `BuiltinCannotBeEdited`; `ThemeEvent` gains `Imported { count: u32 }`. E21 toasts: "Built-in themes cannot be deleted" (Warning), "Built-in themes cannot be edited. Copy it first." (Warning), "Imported { $count } themes" (Success).
- Basis: UI-FND-040 to UI-FND-043 [verified]; texts [guess, writer-level].
- Edits: 00-epics section 4 (E19-A1 in the E17 row); UI-FND-040; beads yrf.7, the E21 theme menu bead; edge lgw.8 on yrf.1.

### R-210 themes.json from TaskSlinger (U-18) blocks E17-005 (OI-U40)
- Question: the named built-in palettes come from TaskSlinger under Wine (U-18); E17-005 cannot start without the file. Raised by B5-10.
- Resolution: D-012. The two default themes (Default Light, Default Dark) already ship at P0 from hand-written values in E17-002 (lgw.4). E17-005 (lgw.7) adds all ten named APP-035 palettes from the TaskSlinger export and stays `blocked-open-item` until the owner commits `assets/themes/builtin.json`. No new bead.
- Basis: U-18 [verified]; D-012.
- Edits: bead lgw.7 (ten palettes, blocked on the export); no new bead.

### R-211 One TS1 share code (U-17) blocks E17-010 (OI-U41)
- Question: the TS1 share-code encoding is unknown until the owner generates one code in TaskSlinger (U-17). Raised by B5-11.
- Resolution: D-013. E17-010 ships import of the tasklens `TL1` format (UI-FND-043) now and TS1 when the owner records a sample code in U-17. The E21 share-code beads depend on lgw.11.
- Basis: U-17 [verified]; D-013.
- Edits: bead lgw.12 (`blocked-open-item` for the TS1 part, TL1-only path noted).

### R-212 `assets/icons/tasklens.svg` artwork (OI-U42)
- Question: no document specifies the application icon. Raised by B5-12.
- Resolution: D-014. E15-022 builds a placeholder: a 256 px rounded square in the accent colour with a white monospace "tl". New bead E15-026 "Replace the placeholder application icon" (P3, `assets/icons/tasklens.svg`, depends on ug9.25) stays `blocked-open-item` until artwork exists.
- Basis: none in the docs [verified absent]; D-014.
- Edits: bead ug9.25 (drop `blocked-open-item`); new bead E15-026.

### R-213 Recording U-nn outcomes (OI-U43)
- Question: where a resolved U-nn item is recorded. Raised by B5-15.
- Resolution: append the outcome to the item's row in the "Unverified assumptions" table, one line "Outcome (date): ...". No other place.
- Basis: the U table [verified].
- Edits: none.

### R-214 Priority of spec-P1 components inside P0 anchors (OI-U44)
- Question: E15-A1 and E15-A2 contain spec-P1 and P2 components; E18-A1 and A2 (P0) need E16-006, 007, 008, A2 and 009. Raised by B5-1.
- Resolution: accepted; those E16 beads carry priority 0. Shield stays in E16-A3 (P1) and the preview contract in E17-A2 (P2).
- Basis: 00-epics phase rules [verified].
- Edits: none.

### R-215 RTL golden needs a Hebrew font on CI (OI-U45)
- Question: E18-013 renders a Hebrew golden; CI may lack the font. Raised by B5-36.
- Resolution: the case is skipped with a printed reason when `font_id("Noto Sans Hebrew")` fails. The E02 CI image may add the font later.
- Basis: UI-FND-050 [verified].
- Edits: none.

### R-216 Double-click on a process row expands, it does not open Properties (OI-U46)
- Question: UI-PRC-002 says double-click runs `OpenProperties`; UI-PWN-003 says double-click is not bound and expands, like TaskSlinger. Raised by B7-E23-1.
- Resolution: double-click toggles expansion on a row with children and does nothing on a leaf. Properties open with Enter, Alt+Enter, Ctrl+I, the menu item, and the delegate's `on_activate` for the system tables (UI-SYS-002 keeps double-click there: those rows have no children).
- Basis: D-008, UI-PWN-003, UI-SYS-002 [verified].
- Edits: UI-PRC-002, UI-PRC-008; bead pdu.1 (gesture binds `ToggleExpand`).

### R-217 Health items: sources per tab and display names (OI-U47)
- Question: UI-SHL-051 says "sources the active tab uses" without a list; UI-SHL-050 names 12 sources but `SourceId` has more. Raised by B7-E20-9, B7-E21-6.
- Resolution: per tab: Processes: `Processes` plus every source a visible minigraph draws; Performance: `Cpu`, `Memory`, `Disks`, `Network`, `Gpu`, `Sensors` plus `Battery` when a battery page exists; Services: both systemd managers and `Journal`; Startup Apps: `Startup`; Connections: `Connections`; Information: `Inventory`. Display names: CPU, Memory, Disks, Network, GPU, Sensors, Processes, Services, Startup, Connections, Inventory, KWin, Logind, UDisks, UPower, NetworkManager, Public IP, History, Journal, Battery; any other variant shows its Rust name.
- Basis: UI-SHL-050, UI-SHL-051 [verified]; the mapping [inferred].
- Edits: UI-SHL-050, UI-SHL-051.

### R-218 `WindowRegistry` key queries and the keep-detached close row (OI-U48)
- Question: E20-028 must know whether any `DetachedGraph(_)` window is open and be told when one closes; E26-018 provides `any_detached_open`; `DetachedGraph(u8)` caps ids at 255. Raised by B7-E20-10, E26-OI-3, E26-OI-7.
- Resolution: E16-A2 adds `pub fn has_key(&self, pred: impl Fn(&WindowKey) -> bool) -> bool` and `pub fn count(&self, pred) -> usize` to `WindowRegistry` and calls `cx.notify()` on every insert and removal. E20-028 uses `has_key(|k| matches!(k, WindowKey::DetachedGraph(_)))` and observes the registry; `any_detached_open` in E26-018 becomes a one-line wrapper and that bead un-ignores `keep_detached_on_close_keeps_app`. `DetachedGraph(u8)` holds: `[[detached_windows]].id` is a `u8`.
- Basis: UI-SHL-113 row 2, PERF-084, APP-045, jqk.30 [verified].
- Edits: beads fut.2, jqk.30, xdh.20; edge xdh.20 on jqk.30.

### R-219 Start hidden and `ToggleMainWindow` before the tray exists (OI-U49)
- Question: `--start-minimized` can hide only when a tray exists; `tray_available` is false until E31. Raised by B7-E20-12, B7-E20-13, E24-8.
- Resolution: `tray_available(cx)` returns `TrayModel.available` and is `false` until E31-009 fills it (E24-023 stubs the same `Some(false)`). Without a tray `--start-minimized` opens the window and minimizes it (APP-048), and a close that would quit minimizes instead when `ToggleMainWindow` arrives while visible.
- Basis: APP-048, UI-SHL-113 [verified].
- Edits: UI-SHL-110 step 6.

### R-220 Keyboard Shortcuts window headings (OI-U50)
- Question: UI-SHL-095 lists seven headings; table (b) has more contexts. Raised by B7-E21-4.
- Resolution: eleven headings in order: Global, Tables, Processes, History, Services, Startup Apps, Connections, Information, Performance, Properties windows (PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog), Dialogs (RunDialog, Picker, ColumnPicker, HandleSearch, JournalList, LowMemory). A heading with no shipped bindings is not drawn.
- Basis: table (b) [verified]; grouping [inferred].
- Edits: UI-SHL-095.

### R-221 Fans group: partial entries and empty state (OI-U51)
- Question: UI-SHL-066 says an empty label plus "None" removes the entry but not what a half-set entry stores or what the group shows with no fans. Raised by B7-E21-7.
- Resolution: `[[sensors.fans]]` entries store `key`, optional `label` (omitted when empty) and optional `temp` (omitted for None); an entry with neither is removed. With no fan in the snapshot the group is hidden. Keys follow PERF-005 (`fan:<hwmon name>:<device path>:<n>`; fallback form in R-229).
- Basis: UI-SHL-066, table (a) [verified]; storage shape [inferred].
- Edits: UI-SHL-066; table (a) `[[sensors.fans]]`.

### R-222 Passwordless setup step (OI-U52)
- Question: the `SetupStep` variant for `polkit-rule on|off` and the result stream belong to E03 and E29. Raised by B7-E21-11.
- Resolution: R-149 (`SetupStep::PolkitRule { on }`, results in `HealthSnapshot.setup`). UI half: E21-042 dispatches `Command::RunSetup { step }` and reads the result from `HealthSnapshot.setup` on the `HEALTH` bit.
- Basis: UI-PRF-052 [verified].
- Edits: none.

### R-223 Set Affinity is P1 inside a P2 anchor (OI-U53)
- Question: raised by B7-E23-2.
- Resolution: accepted. E23-001 to E23-004 are P1 and schedulable before E23-A2 closes; the anchor stays P2.
- Basis: PROC-067, UI-PWN-021 [verified].
- Edits: none.

### R-224 Dump file name sanitising (OI-U54)
- Question: `{name}` in `<name>_<pid>_<yyyymmdd_hhmmss>.core` can contain `/` and control characters. Raised by B7-E23-4.
- Resolution: replace `/`, `\0` and every control character with `_`, cut the name part to 64 bytes on a char boundary, use `process` when the result is empty. The timestamp is local time formatted with `jiff`. The write is engine side (R-145).
- Basis: PROC-075, UI-PWN-034 [verified]; rule [inferred].
- Edits: UI-PWN-034.

### R-225 "Shared libraries only" keeps the Primary and "Outside loaded modules" nodes (OI-U55)
- Question: raised by B7-E23-5.
- Resolution: with the box on, the Primary node (the executable's own mapping) and the "Outside loaded modules" node stay visible; only file-backed library mappings are filtered by the PROC-081 patterns.
- Basis: PROC-081 [verified]; choice [inferred].
- Edits: process-windows.md Memory maps entry.

### R-226 After "Grant access" succeeds (OI-U56)
- Question: UI-PWN-018 says the window re-sends its demand "with the helper flag", which `DetailDemand` lacks. Raised by B7-E23-6.
- Resolution: no flag. When `HealthSnapshot.helper` changes to `Unlocked`, every open Properties window whose General payload is `Restricted` sends `Command::DetailRefresh { target, tab }` once for its active tab. Routing Restricted targets through the helper is R-170.
- Basis: UI-PWN-002, UI-FND-028 [verified].
- Edits: UI-PWN-018.

### R-227 VirusTotal and dump handlers reach the Processes page by chain edit (OI-U57)
- Question: raised by B8-E22-14.
- Resolution: E22-038 and E22-039 add the menu items and dispatch `OpenVirusTotal` and `CreateDump`; E23-032 and E23-034 register the handlers in `pages/processes/mod.rs` under the E23 row of the chain-edit rule. No new bead.
- Basis: 00-epics section 2 `pages/processes/mod.rs` rule [verified].
- Edits: edges gma.35 on pdu.41 and gma.37 on pdu.42.

### R-228 Chart validity and the E19 model constants (OI-U58)
- Question: the chart cannot tell Failed from Restricted from Unsupported by itself; the pages need `MetricId` constants, the `TIER_1HZ` bit, `ceil_pow2`, `ceil_125` and thread CPU time. Raised by E19-1 to E19-5, E25-OI-2.
- Resolution: the owner page calls `ChartView::set_state` from the `SourceHealth` of the source it knows; the chart derives only WarmingUp and Stale. The model items are R-170 and R-144. E24 and E25 beads name series by their UI-PRF names and stop and report when a constant is missing; E19-013 waits for `TIER_1HZ`; E19-004 defines nothing privately; E19-005 reports 0 thread CPU until `thread_cpu_ns` exists.
- Basis: UI-PRF-001, UI-PRF-006, ADR-014 [verified].
- Edits: none here.

### R-229 Performance frame items (OI-U59)
- Question: raised by E24-6, E24-7, E24-9, E24-10.
- Resolution: where `DeviceIdentity` has no by-path or device path, the UI key is `disk:<name>` and `fan:<chip>:<index>`; PERF-005 records those as fallback forms. `DevicePage::set_active(&mut self, active: bool, cx)` is on the trait from E24-006. `CpuPage.bench_show_both` is a public field in crw.11; the paint hook is `AnimationDriver.paint_log` behind `cfg(any(test, feature = "test-hooks"))`, so E01-001 declares `bench = ["test-hooks"]` in `tl-ui/Cargo.toml`. The fan `connected` flag is R-170.
- Basis: PERF-005, UI-PRF-053, leader notes [verified].
- Edits: PERF-005, UI-PRF-011; beads crw.8, wlf.3.

### R-230 Device page items (OI-U60)
- Question: raised by E25-OI-1, OI-4 to OI-8.
- Resolution: `request_public_ip_fetch` lives in E21-017 and shows the notice window first; E25-021 and E28-036 call it (edges on bj6.20). PWM is pre-scaled into the RPM axis through a derived `SeriesView` kept by the page; if E19 gains a per-series `YScale` the derived series goes. The UPower `Type` and `State` word mapping lives in `pages/perf/battery.rs`. `GridStyle.vertical_at: Option<Arc<[f32]>>` and `ChartSeries.gap_style: GapStyle::{Break, Dashed}` are added to E19-001's types by E25-032 as a same-batch chain edit. The device commands are R-139 and `LaunchWhat::DbusActivate` is R-170.
- Basis: UI-PRF-034, PERF-060, UI-SYS-095 [verified].
- Edits: bead yrf.3 (note on the E25-032 extension); edges o55.22 and 9o0.32 on bj6.20.

### R-231 Minigraph items (OI-U61)
- Question: raised by E26-OI-1, OI-2, OI-4, OI-6.
- Resolution: E26-010 calls the shared Graph Color submenu builder (edge xdh.12 on bj6.35). Card reorder is pointer-only; Reset Minigraph Layout is the keyboard path. Aggregate devices and per-process series are R-154.
- Basis: UI-PRF-040 to UI-PRF-044 [verified].
- Edits: edge xdh.12 on bj6.35.

### R-232 Services items (OI-U62)
- Question: raised by E27-1, E27-5, E27-7, E27-8, E27-9.
- Resolution: on `CommandError::Busy`, `SysQuery.last_sent` still becomes the new generation, `pending_resend = true`, and the next snapshot resends once with a fresh generation; on `EngineStopped` nothing is resent. `ToastKey` values at or above `0x8000_0000_0000_0000` are fixed keys reserved by E16-006; the tracker's generated keys (`id << 8 | index`) never reach that range because `ActionId` stays below 2^55; `SERVICES_LOADING_TOAST = ToastKey(0x8000_0000_0000_0101)`; the E28 pages take `..0201` (Startup) and `..0301` (Connections). E23-A1 joins the E27 row of 00-epics section 4. The UI sends `UnitEnable` or `UnitDisable` when leaving Masked; the unmask inside that request is R-170. Services column ids are `SysColumnId` (R-170).
- Basis: UI-SYS-002, UI-SYS-007, UI-FND-020 [verified]; the reserved range [inferred].
- Edits: 00-epics section 4; UI-FND-020 (fixed-key range); bead fut.9.

### R-233 Startup Apps, Connections and Information items (OI-U63)
- Question: raised by E28-1 to E28-4, E28-7, E28-9, E28-11 to E28-13, E28-15.
- Resolution: the Information skeleton follows UI-SYS-085 (every section shows its band and an 8-line skeleton block); shell.md's "4 section blocks 120 tall" is corrected. E27-A2, E29-A2, E29-A3, E25-A1 and E03-A2 join the E28 row of 00-epics section 4. The public IP command, `InventorySnapshot` fields, `committed_peak`, the 64-bit rule, `AlreadyRunning`, system-table column ids, startup entry fields and connection display fields are R-139, R-156, R-166, R-170 and R-148; the E28 beads read "the E03 field of that meaning" and stop and report when it is missing. "Already running" is `RejectReason::AlreadyRunning`, shown as the Info toast (R-196).
- Basis: UI-SYS-085, UI-SHL-111, SYS-023 [verified].
- Edits: UI-SHL-111; 00-epics section 4.

### R-234 E22 epic-row additions and the build-rule note (OI-U64)
- Question: raised by B8-E22-5, B8-E22-16.
- Resolution: E19-A1 and E01-A1 join the E22 row of 00-epics section 4 (edges exist). The build lines were rewritten by the leader on 405 beads.
- Basis: 00-epics section 4 [verified].
- Edits: 00-epics section 4.

### R-235 Topics settled on the engine side (OI-U65)
- Question: the UI raisers whose answer is an engine-side entry, listed so no raiser is lost.
- Resolution: E25-OI-1, B8-E28-1, B7-E20-11: R-139. B8-E22-1: R-141. B5-22, E19-4: R-142, R-144. B5-2, B5-30, B7-E21-9: R-142. B5-38: R-153. B5-6: R-170. B5-7: R-148. B7-E21-5: R-170 and R-171. B7-E23-3: R-146. E19-1 to E19-3, E25-OI-2: R-170. E24-9: R-170. E25-OI-6: R-170. E26-OI-1, E26-OI-2: R-154. B7-E21-11: R-149. B7-E23-6 (engine half): R-170. B8-E22-10: R-149, R-170. B8-E27-3 (engine half), B8-E27-7: R-170. B8-E27-5, B8-E28-11: R-170. B8-E28-2, B8-E28-12, B8-E28-13: R-156. B8-E28-3: R-166. B8-E28-4: R-170. B8-E28-9: R-148. B7-E20-4 (engine half): R-170. Not raised by a UI item: udev R-140, gcore R-145, helper hook R-147, PSI R-151, `unsafe` R-143.
- Basis: `docs/plan/open-items-ui.md` OI-U65 [verified].
- Edits: none.

### R-236 `AppEnv.set_autostart` takes enabled and minimized
- Question: the autostart entry must carry `--start-minimized` when "Start minimized" is on, but `set_autostart(bool)` cannot say so. Leader entry, 2026-10-04.
- Resolution: `AppEnv.set_autostart: Arc<dyn Fn(bool /* enabled */, bool /* minimized */) -> Result<(), String>>`. The autostart desktop file's `Exec` line gets `--start-minimized` when `minimized` is true. Toggling Start minimized while Start at login is on calls `set_autostart(true, minimized)` again so the file is rewritten.
- Basis: APP-048, UI-SHL-040 and UI-SHL-069 rows, R-171 [verified].
- Edits: R-171 field; ADR-009 `AppEnv` paragraph; UI-SHL-040 and UI-SHL-069 Start minimized rows.

### R-237 `AdapterInfo.pci_bus_id` and the `rendering.gpu` value form
- Question: `rendering.gpu` needs a stable value for an adapter. Leader entry, 2026-10-04.
- Resolution: `AdapterInfo { name, backend, device_type: String, pci_bus_id: Option<String> }`. `rendering.gpu` stores `pci:<bus id>` when the adapter reports one, otherwise `name:<adapter name>`; `"auto"` and `"software"` stay as table (a) has them.
- Basis: R-070, APP-046, R-171 [verified].
- Edits: R-171 type; ADR-009 `AppEnv` paragraph; table (a) `rendering.gpu` comment; UI-SHL-040 and UI-SHL-069 Rendering GPU rows.

### R-238 `AddRandomMinigraphs` is an E26 bead
- Question: the Debug menu item "Add random process minigraphs" (UI-SHL-042) dispatches `AddRandomMinigraphs`, which no epic declared or handled. Leader entry, 2026-10-04.
- Resolution: a new E26 bead declares `AddRandomMinigraphs` in `pages/minigraph/actions.rs` and handles it in the minigraph panel (adds process minigraph cards for random visible processes; toasts "Added %ld random process minigraphs." and "No process minigraphs available."). E21's Debug menu only dispatches it (R-179 pattern).
- Basis: UI-SHL-042, UI-PRF-040 to UI-PRF-044, R-179 [verified].
- Edits: UI-SHL-042 row; new E26 bead (plan key and ID in `b6-ui-performance.md`).

### R-239 The window-pick result is an E22 bead
- Question: PROC-055's result handling (select the row, the Picked dialog for Ctrl+click, the toasts "No window found", "No window selected" and "Unknown process", `MainWindow::end_window_pick`) had no bead. Leader entry, 2026-10-04.
- Resolution: a new E22 bead handles `WindowsSnapshot.pick` (R-149): on `Picked { pid }` it selects the row through `ProcessesPage::select_by_key` (UI-PRC-022) or, with Ctrl held, opens the PROC-055 confirmation and ends the process; `NoWindow` and `Cancelled` map to the toasts; it then calls `MainWindow::end_window_pick` (E20, jqk.29) to clear the picking state. UI-SHL-014 keeps the picker button and the start of a pick.
- Basis: PROC-055, UI-SHL-014, UI-PRC-014, UI-PRC-022, R-149 [verified].
- Edits: UI-SHL-014 result sentence; UI-PRC-022 implements line; new E22 bead (plan key and ID in `b8-ui-tables.md`).

### R-240 `message()` texts for `NotAuthorized`, `AuthCancelled` and `Unsupported`
- Question: R-170 added the three `ActionError` variants but gave them no user text; ADR-015 left a note. Leader entry, 2026-10-04.
- Resolution: `NotAuthorized` reads "Administrator access was refused.", `AuthCancelled` reads "Authentication was cancelled." and `Unsupported` reads "Not supported on this system." They follow the R-196 rules (sentence case, full stop, `short_reason` is the first sentence lower-cased without its full stop).
- Basis: R-170, R-196 [verified]; texts set by the leader.
- Edits: ADR-015 `message()` table; R-121 accepted strings (settled open items line).

### R-241 `WindowBridge.Picked` takes the pid as a string
- Question: KWin's `callDBus` picks the D-Bus type of a JavaScript number itself (int32 or double), so `Picked(s uuid, u pid)` may be refused by a strict zbus signature check. Raised by the E14-017 implementer (`tasklens-y6k.21`). Leader entry, 2026-10-04.
- Resolution: the method is `Picked(s uuid, s pid)`. `pick.js` sends `String(w ? w.pid : 0)`. The engine parses the text as `u32`; `"0"` or text that does not parse means `PickOutcome::NoWindow`. This matches the bridge `Update` call, which already sends one JSON string.
- Basis: R-149, ADR-010 [verified]; the `callDBus` number conversion is [inferred] from KWin's QJSValue handling.
- Edits: ADR-010 KWin worker paragraph; R-149 (method signature); `assets/kwin/pick.js`; E14-019 (`tasklens-y6k.23`).

### R-242 Netlink readers in tl-collect are E12 files (E12-OI-5)
- Question: IP addresses (`RTM_GETADDR`) and Wi-Fi (`nl80211`) need tl-collect readers, but 00-epics section 2 lists only E06's `net/` files, so `tasklens-hjn.18`, `hjn.21` and `hjn.28` stayed `blocked-open-item`. Leader entry, 2026-10-04.
- Resolution: `crates/tl-collect/src/net/addrs.rs` (E12-015, `hjn.18`) and `crates/tl-collect/src/net/nl80211.rs` (E12-025, `hjn.28`) are E12-owned files inside tl-collect. Each adds one `pub mod` line to `net/mod.rs` as a chain edit on E06-A1 (`tasklens-hx1.1`). Both use the `netlink-sys` crate that E06-003 already puts in tl-collect's `Cargo.toml`; no new crate. The three beads are unblocked.
- Basis: PERF-058, PERF-059, PERF-104, R-052, ADR-001 tl-collect row [verified]; `docs/plan/b4-engine-workers.md` E12-OI-5 recommendation, accepted.
- Edits: 00-epics section 2 (tl-collect `net/` row); beads hjn.18, hjn.21, hjn.28 (label removed, Notes).

### R-243 Minimal UI init for `--low-memory` (E32 OI-10)
- Question: `TasklensUi::init` loads three font faces, the icon set and every page's key bindings; UI-PWN-032 wants Noto Sans Regular and SemiBold only and no icons. `tasklens-m1f.11` and `m1f.12` carried the E32 OI-10 recommendation (`TasklensUi::init_low_memory` in E16) as open. R-153 has since made `tl_ui::run(UiLaunch)` the entry point, with `OpenMode::LowMemory` and `OpenHooks.low_memory`. Leader entry, 2026-10-04.
- Resolution: `pub fn init_low_memory(cx: &mut App, client: Arc<dyn EngineClient>, init: UiInit)` lives in `crates/tl-ui/src/windows/low_memory.rs` (E32-009, `m1f.11`), not in `app.rs`. It performs the steps of `TasklensUi::init` with these differences: fonts are `NotoSans-Regular.ttf` and `NotoSans-SemiBold.ttf` only (`cx.text_system().add_fonts` with two `include_bytes!` from `assets/fonts/`); no icon loading (the frame's pin and close SVGs are embedded by E32-009); `bind_keys_low_memory(cx)` is the only key-binding call; no page registration; it installs the theme store without user themes (`init.themes_json` is ignored) and sets `OpenHooks.low_memory = Some(Box::new(move |cx| { LowMemoryWindow::open(cx, client.clone()); }))`. `tl_ui::run` (E16-012, `fut.15`) leaves a `// E32-013: low-memory init` insertion point in step 3; the new bead E32-013 replaces the `TasklensUi::init(..)` call there with `if launch.open == OpenMode::LowMemory { windows::low_memory::init_low_memory(cx, launch.client, launch.init) } else { TasklensUi::init(cx, launch.client, launch.init) }`. tl-app's `--low-memory` arm (E32-010, `m1f.12`) sets `UiLaunch.open = OpenMode::LowMemory` and `EngineConfig.mode = EngineMode::LowMemory`; it opens no window itself. `mlockall(MCL_CURRENT)` runs in tl-app right before `tl_ui::run` (tl-ui has no rustix and `AppEnv` gets no new member); this replaces R-152 OI-12's "after the first frame", and PROC-098's "mlocked after start (current pages only)" stays true.
- Basis: UI-PWN-032, UI-SHL-110, R-152, R-153, R-171, fut.15 text [verified]; the init step list is the E16-A1 text.
- Edits: UI-PWN-032 Build line; new bead E32-013 (plan key and ID in `b1-infra.md`); beads m1f.11, m1f.12, fut.15; edges m1f.12 on E32-013, E32-013 on fut.15 and m1f.11.

### R-244 The colour-scheme watcher runs in the `SystemdUser` worker
- Question: R-170 item 4 put the portal `color-scheme` reader "in E14's session-bus opener worker (y6k.24)", but `opener.rs` is a set of functions E11 calls per action, not a worker; nothing owned the loop that subscribes to `SettingChanged` and sends `SourceUpdate::ColorScheme`. Leader entry, 2026-10-04.
- Resolution: `opener.rs` (E14-020, `tasklens-y6k.24`) keeps the two functions `read_color_scheme` and `watch_color_scheme` and their tests, as written. The `SystemdUser` generation of the systemd worker (E14-001, `tasklens-y6k.5`, `workers/dbus_systemd.rs`) runs the watcher: after `ManagerConnSlot::set(conn)` succeeds on the session bus and only in `EngineMode::Full`, it spawns `opener::watch_color_scheme(cx, &conn, &io.tx)` as a child task of the generation (the `tx` is the worker's `EngineMsg` sender from `WorkerIo`). The child ends with the generation; a reconnect (E14-002) starts it again, which resends the current value. A watcher error is logged at `debug` and does not restart the worker. The reducer arm (8q0.9) is unchanged.
- Basis: R-170 item 4, R-166 B3-8, ADR-004 worker table (`SystemdUser` runs over the session bus on every desktop), y6k.24 Notes [verified].
- Edits: ADR-003 `SystemSnapshot` sentence and ADR-004 opener paragraph ("session-bus opener worker" becomes the `SystemdUser` worker); beads y6k.24, y6k.5; edge y6k.5 on y6k.24.

### R-245 `serde_json` in tl-app for `themes.json` and `run-history.json`
- Question: `cr0.11` (E33-008) writes `themes.json` (`PersistTheme::Save`) and `run-history.json` (`RunHistoryFile`), both JSON, but `serde_json` is not in ADR-001's tl-app row, so the bead stops and reports. Leader entry, 2026-10-04.
- Resolution: ADR-001's tl-app row gains `serde_json` (themes JSON and the Run history file). `cr0.11` adds `serde_json = { workspace = true }` to `crates/tl-app/Cargo.toml` (an E33 file; the root `Cargo.toml` already pins `serde_json = "1"`, so no E02 bead is needed). `ThemeDef` and `RunHistoryEntry` derive `Serialize` and `Deserialize` in tl-ui (R-171, fut.1), so tl-app serialises them directly: `themes.json` is the pretty-printed JSON array of `ThemeDef`; `run-history.json` is the pretty-printed JSON array of `RunHistoryEntry`. No alternative format.
- Basis: ADR-001 rows, R-142 item 1, R-153, R-170 item 12, R-171 [verified].
- Edits: ADR-001 tl-app row; bead cr0.11.

### R-246 `HelperCallError` gains `StartFailed(Arc<str>)` and lives in `actions/mod.rs`
- Question: `qfx.2` (E29-A2) declares its own `HelperCallError { Missing, Cancelled, NotAuthorized, StartFailed(Arc<str>), Timeout, Exited, Protocol }` in `helper/mod.rs`, while R-147 fixes `HelperCallError { Missing, Cancelled, NotAuthorized, Timeout, Exited, Protocol(Arc<str>) }` in `actions/mod.rs`; a failed session start had no place. Leader entry, 2026-10-04.
- Resolution: one enum, in `crates/tl-engine/src/actions/mod.rs` (E11-002, `i3o.4`): `HelperCallError { Missing, Cancelled, NotAuthorized, StartFailed(Arc<str>), Timeout, Exited, Protocol(Arc<str>) }`. `helper/mod.rs` imports it (`pub(crate) use crate::actions::HelperCallError;`) and declares no copy. `StartFailed(reason)` is returned by a `Call` whose implicit start ended in `HelperStartOutcome::Failed(reason)` (a `Call` while `Locked` starts the session first). E29-014 (`qfx.17`) maps it to `Failed(ActionError::Internal(reason))`. A start that a `Call` triggered publishes no `helper_start` outcome: the action's own `Failed(AuthCancelled | NotAuthorized | Internal)` toast reports it, so the user sees one toast; only `HelperMsg::Start` (shield, Grant access, unlock at launch) sets `helper_start`.
- Basis: R-147, R-148 items 2 and 3, i3o.4 and qfx.17 text [verified].
- Edits: ADR-006 `PrivilegedOps` sentence; R-147 enum text; beads i3o.4, qfx.2, qfx.17.

### R-247 Setup commands travel on the helper channel
- Question: `qfx.22` (E29-019) says the setup runner is "called from the routing added in E29-020" and `qfx.23` (E29-020) routes `SetupStatus` and `RunSetup` "to the setup handlers of E29-019", but the reducer has only `helper_tx: Option<mpsc::Sender<HelperCommand>>` with `HelperCommand { Start, End }`; no channel reached a setup runner. Leader entry, 2026-10-04.
- Resolution: `HelperCommand` (E09, `reducer/mod.rs`, `8q0.3`) becomes `HelperCommand { Start, End, SetupStatus, RunSetup(SetupStep) }`. E09's `apply.rs` (`8q0.9`) forwards `Command::SetupStatus` and `Command::RunSetup { step }` through `helper_tx` with `try_send`, exactly like `Start` and `End` (`None` or a full queue logs at `debug` and drops; the UI can click again). `HelperMsg` (`qfx.2`) gains `SetupStatus` and `RunSetup(SetupStep)`; the forwarding task E29-020 spawns in `session.rs` maps the four `HelperCommand` variants to `HelperMsg` one to one. `helper_task` owns the `SetupState` and runs `probe_status` and `run_step` (E29-019) inside `cx.spawn_blocking`, one message at a time; a `Call` or `Start` that arrives during a setup run waits in the queue (the UI disables the setup buttons while a run is in progress, and two polkit prompts never overlap). The 100 ms output updates are sent from the blocking job through a clone of the `updates` sender with `try_send`. `qfx.23` no longer edits the `SetupStatus` and `RunSetup` arms; its `apply.rs` chain edit is the `SourceUpdate::Helper` and `SourceUpdate::Setup` arms only.
- Basis: R-148 item 1, R-149 items 1 to 4, R-150 items 1 and 4, 8q0.3 and 8q0.9 text [verified].
- Edits: ADR-005 or ADR-006 helper session sentence (none needed: the reducer field is R-148's); beads 8q0.3, 8q0.9, qfx.2, qfx.22, qfx.23.

### R-248 One drop-in renderer and one file text
- Question: `qfx.12` (E29-009) and `i3o.22` (E11-020) render the recovery drop-in differently: `render(&RecoveryEdit)` with `RestartSec=<n>ms`, `StartLimitIntervalSec=<n>` in seconds, `OnFailure` under `[Unit]`, a blank line between sections and no header, against `render(&[(DropInKey, String)])` with `<n>us` values, a header line, `OnFailure` under `[Service]` (wrong: it is a `[Unit]` key) and no blank line. Leader entry, 2026-10-04.
- Resolution: `pub fn render(keys: &[(DropInKey, String)]) -> String` in `crates/tl-ops/src/dropin.rs` (E29-owned, R-168) is the only renderer; `tl-engine/src/actions/recovery.rs` re-exports it. Values are rendered verbatim; `keys_of` (E11-020) writes time values as `<n>us` for `RestartSec`, `RestartMaxDelaySec` and `StartLimitIntervalSec`, integers in decimal, words as given and `OnFailure` names joined by one space. File text: `HEADER` (`# Written by tasklens. Edit it in the Service Properties Recovery tab or delete this file.\n`), then `[Unit]\n` with `StartLimitIntervalSec`, `StartLimitBurst`, `StartLimitAction`, `FailureAction`, `OnFailure` (only those present, in that order), then `[Service]\n` with `Restart`, `RestartSec`, `RestartSteps`, `RestartMaxDelaySec` (same rule). A section with no keys is omitted; every line is `Key=value\n`; no blank lines; LF only. Example: `HEADER + "[Unit]\nStartLimitIntervalSec=60000000us\nStartLimitBurst=3\n[Service]\nRestart=on-failure\nRestartSec=5000000us\n"`. The helper (E29-009) still parses the keys into typed values for its range checks (`RestartSec` and `RestartMaxDelaySec` 0 to 3 600 000 000 us, `StartLimitIntervalSec` 0 to 86 400 000 000 us, `StartLimitBurst` 0 to 1000, `RestartSteps` 0 to 1000) and then calls `render(keys)` with the validated list.
- Basis: SYS-013, UI-SYS-034, R-004, R-168, systemd unit syntax (`OnFailure`, `StartLimit*` and `FailureAction` are `[Unit]` keys; `Restart*` are `[Service]` keys) [verified].
- Edits: 00-epics section 2 tl-ops exception (signature); beads qfx.12, i3o.22.

### R-249 History worker texts and problem states (E30-OI-4 proposals)
- Question: `8fx.16`, `8fx.17` and `8fx.18` carry three values marked "proposed, E30-OI-4": the health text "History dropped N events", `Problem(OpenFailed)` with a 60 s reopen on a store write error, and `Problem(EmptyOrTruncated)` on a read error. Leader entry, 2026-10-04.
- Resolution: accepted as written. The `HistoryRecorder` worker writes `SourceHealth.last_error = "History dropped N events"` at most once per minute with the state unchanged (`Running`). A store write error sets `HistoryStoreState::Problem(OpenFailed)`, recording continues in RAM, and the worker tries `Store::open` again 60 s later. A store read error answers with the data that could be read and `store_state = Problem(EmptyOrTruncated)`.
- Basis: ADR-012, ADR-015 (`last_error` capped at 200 characters), R-156 item 3, R-169 [verified].
- Edits: ADR-012 "Reading" paragraph (one sentence); beads 8fx.16, 8fx.17, 8fx.18 (the "proposed" tags go).

### R-250 Mission Center's app-matching exceptions table
- Question: `yms.19` (E05-016) waits for the leader to paste Mission Center's `matching.rs:10-24` exceptions table. The 1.2.0 source is not on disk; the installed binary `/usr/bin/missioncenter-magpie` (metainfo release 1.2.0) is. Leader entry, 2026-10-04.
- Resolution: the table read from the binary's string data is five pairs, exe basename to desktop id: `firefox-bin` to `firefox`, `soffice.bin` to `libreoffice`, `resources-processes` to `resources`, `gnome-terminal-server` to `gnome-terminal`, `chrome` to `google-chrome-stable`. `EXCEPTIONS` in E05-016 is exactly these five; the bead's `chromium` and `electron` identity entries go (the exe step finds them anyway), and `gnome-terminal-server` maps to `gnome-terminal`, `chrome` to `google-chrome-stable`. A mapped id missing from the index falls through to the normal steps, so a distribution's different desktop file name costs only the shortcut.
- Basis: `strings /usr/bin/missioncenter-magpie` on the installed 1.2.0 [verified]; the pair direction (basename to id) [inferred] from the names, matching the inventory's description (`docs/research/claude-missioncenter-inventory.md`, `matching.rs:10-24`).
- Edits: bead yms.19.

### R-251 `tray-22.argb` and `tray-44.argb` are an E31 bead
- Question: R-161 item 4 names the two pre-rendered ARGB32 files under `assets/icons/` and `wox.10` embeds them, but no bead creates them. Leader entry, 2026-10-04.
- Resolution: new bead E31-013 renders `assets/icons/tasklens.svg` to `assets/icons/tray-22.argb` (22 x 22 x 4 = 1,936 bytes) and `assets/icons/tray-44.argb` (7,744 bytes), raw ARGB32 in network byte order (one byte each of A, R, G, B per pixel, rows top to bottom), with `rsvg-convert -w N -h N assets/icons/tasklens.svg -o <tmp>.png` then `magick <tmp>.png -depth 8 argb:assets/icons/tray-N.argb`. It is a chain edit to E15's `assets/icons/` and depends on E15-022 (`ug9.25`, the placeholder logo); `wox.10` depends on it. When E15-026 (`ug9.26`) replaces the artwork, the two files are re-rendered with the same commands (sentence added to ug9.26).
- Basis: UI-SHL-113, R-161 item 4, ksni `Icon { width, height, data }` format [verified]; `rsvg-convert` and `magick` are installed on the owner machine.
- Edits: new bead E31-013 (plan key and ID in `b1-infra.md`); beads wox.10, ug9.26; edges wox.10 on E31-013, E31-013 on ug9.25.

### R-252 ADR-007 failure table follows R-155
- Question: ADR-007's failure table still says `InitFailed { LibraryNotFound | DriverNotLoaded }` makes the worker retry `Hello` every 300 s with data state `Missing`, but R-155 item 4 made it terminal and `ndy.12` no longer retries. Leader entry, 2026-10-04.
- Resolution: the row reads: engine action "health `SourceState::Unsupported` with `last_error` `LibraryNotFound` or `DriverNotLoaded`; the worker shuts the child down and parks (checkpoint loop every 60 s); no retry", data state `Unsupported` (terminal, never retried). A new row covers the missing probe: "`tl-gpu-probe` not installed: health `Unsupported` with `last_error` "GPU probe not installed", no spawn, parked". `ndy.12` is already current.
- Basis: R-155 item 4, ndy.12 text [verified].
- Edits: ADR-007 failure table.

### R-253 How the launcher knows the pkexec prompt has closed
- Question: R-148 item 5 counts an `as_admin` launch as `Succeeded` "when the child is still running 2 s after the prompt closed", but the engine cannot see the polkit agent. Raised by the E14-A3 writer (`y6k.3`). Leader entry, 2026-10-04.
- Resolution: `pkexec` runs the authentication itself and, once authorised, replaces its own process image with the program (`execv`), so the child pid stays the same and its `comm` changes from `pkexec` to `env` (and then to `sh`). The launcher polls `/proc/<pid>/stat` every 250 ms on the blocking pool: while the comm field (the text between the parentheses) is `pkexec`, the prompt is open or being checked. The prompt counts as closed on the first poll where the comm is not `pkexec`, or when `waitpid(pid, WNOHANG)` reports an exit. Exit while the comm was still `pkexec`: status 126 gives `AuthCancelled`, 127 gives `NotAuthorized`, anything else `Failed(Internal("pkexec exited with status <n>"))`. After the comm changed: still running 2 s later, or exited with status 0 within those 2 s, gives `Launched`; a non-zero exit within the 2 s gives `Failed(Internal("exited with status <n>"))`. The 120 s deadline E11 applies bounds the whole wait. The child is a direct child of the engine (the tl-ops launch helper uses `posix_spawn` with `setsid`, no double fork), so `waitpid` works.
- Basis: R-148 item 5, ADR-006, E07-A3 spawn helper text [verified]; pkexec's `execv` behaviour [inferred] from polkit's `pkexec.c`.
- Edits: ADR-006 `Launch` sentence; PROC-095 Run as administrator line; bead y6k.3.

### R-254 Missing edges for `serde_json` and `SourceUpdate::ColorScheme`
- Question: `y6k.2` (KWin bridge) and `y6k.22` (KWin commands) build JSON with `serde_json`, whose tl-engine `Cargo.toml` line is added by E14-A4 (`y6k.4`, R-142 item 7); `y6k.24` sends `SourceUpdate::ColorScheme`, declared in `8hz.5`. None of the three edges existed. Leader entry, 2026-10-04.
- Resolution: add `y6k.2` on `y6k.4`, `y6k.22` on `y6k.4` and `y6k.24` on `8hz.5`. No cycle: `y6k.4` depends on `8q0.1`, `8hz.1` and `as2.2`, none of which depend on an E14 bead; `8hz.5` depends on `8hz.4` and `as2.2` only.
- Basis: y6k.2, y6k.22, y6k.4, 8hz.5 text and dependency lists [verified].
- Edits: the three edges.

### R-255 Startup "Run now" already running in E28-013
- Question: `9o0.9` (E28-013) still says "how the engine reports this: open item B8-E28-9". Leader entry, 2026-10-04.
- Resolution: the engine reports `ActionState::Rejected(RejectReason::AlreadyRunning)` (R-148 item 4); the tracker's reject table maps it to the Info toast "Already running" (R-196, OI-U26). E28-013 reads that reject reason and shows no success text for it.
- Basis: R-148 item 4, R-196 [verified].
- Edits: bead 9o0.9.

### R-256 E14-019 matches R-241
- Question: check that `y6k.23` says `Picked(s uuid, s pid)`, parses the text as `u32`, maps `"0"` or unparsable text to `NoWindow`, and no longer says "open item 7". Leader entry, 2026-10-04.
- Resolution: verified in the bead text on 2026-10-04; no edit needed.
- Basis: y6k.23 text, R-241 [verified].
- Edits: none.

### R-257 The asupersync feature check must exclude dev-dependencies
- Question: `v9m.1` (E02-A1) and ADR-002 run `cargo tree -p tl-engine -e features -i asupersync` and expect no `test-internals`, but tl-engine's dev-dependencies enable that feature for tests, so the command as written fails on a correct workspace. Leader entry, 2026-10-04.
- Resolution: the check is `cargo tree -p tl-engine -e normal,features -i asupersync`, which inverts over normal dependencies only. It must show no `nightly-outcome-try` and no `test-internals`.
- Basis: ADR-002 verification list, ADR-013 (`LabRuntime` tests need `test-internals`) [verified]; `cargo tree -e` edge kinds [verified] in cargo's documentation.
- Edits: ADR-002 verification item; bead v9m.1.

### R-258 The `LICENSE` file is an E02 bead
- Question: the PKGBUILD (E33-A3, `cr0.3`) installs the repo's `LICENSE` file for `license=('MIT')` (D-017) and stops when the repo has none; no bead wrote the file. Leader entry, 2026-10-04.
- Resolution: new bead E02-011 (`tasklens-v9m.13`) writes `LICENSE` at the repo root: the OSI MIT text, LF line ends, first body line `Copyright (c) 2026 <HOLDER>`. `cr0.3` depends on it. The copyright holder name is an owner question; E02-011 stays `blocked-open-item` on that one line and nothing else.
- Basis: D-017, R-159 OI-10, Arch packaging rule for MIT (licence file under `/usr/share/licenses/<pkg>/`) [verified].
- Edits: new bead E02-011 (plan key and ID in `b1-infra.md`); bead cr0.3; edge cr0.3 on v9m.13. Owner question Q-7 below.

### R-259 ADR-001 rule 1 and check 2 cover direct dependencies only
- Question: E02-A1's first reading shows `zbus` 5.19.0 in `tl-ui`'s normal closure through `gpui-pre-linux` 0.3.7 (`accesskit_unix`, `ashpd`, `oo7`, `notify-rust`) and `gpui-base` 0.7.0 (`ashpd`), and `arc-swap` 1.9.2 through `gpui-component` 0.7.0 (`rust-i18n`). Rule 1 and check 2 as written fail on a correct workspace. Leader decision, 2026-10-04.
- Resolution: ADR-001 rule 1 (crate layering) and check 2 apply to direct dependencies only, as declared under `[dependencies]` and `[build-dependencies]` in our own `Cargo.toml` files; transitive dependencies of allowed crates do not count. The R-158 "no path that avoids gpui-kit" narrowing is replaced by this rule. Rule 2 (`asupersync` in the closure of `tl-engine` and `tl-app` only) keeps its closure form. The banned-crate rule (`tokio` and the others of the "No Tokio" definition, U-1) still covers the whole normal graph for `x86_64-unknown-linux-gnu`: check 1 runs `cargo tree --workspace -e normal --target x86_64-unknown-linux-gnu -i <crate>` once per banned crate and fails on any output line whose first word is that crate; no output means absent, and the exit status is not used. E02-008 (`v9m.10`) reads each member's `[dependencies]` and `[build-dependencies]` from `cargo metadata` (equivalent to `cargo tree --depth 1 -e normal`) for the row check and drops the `tl-ui` closure violation; E02-007 (`v9m.9`) is in progress, so E02-008 also switches check 1 to the `-i` form.
- Basis: E02-A1 reading on 2026-10-04 [verified]; leader decision.
- Edits: ADR-001 rule 1, check 1, check 2 and the rejected-alternatives line; R-158 item 4 is superseded; beads v9m.10, v9m.1 (reading 2 is now informational); 00-epics unchanged.

### R-260 Fixture user names are scrubbed as substrings
- Question: `capture-fixture` (E04-005, `2dl.8`, commits 4976667 and 319dc22) replaced user names as whole words with `-` counted as a word character, so `oystein-firefox-...` in `proc/self/mountinfo` survived. Raised by the 2dl.8 implementer. Leader decision, 2026-10-04.
- Resolution: every occurrence of a user name is replaced, as a case-insensitive substring, longer names first. Names shorter than 3 characters are skipped so unrelated text is not wrecked. The mapping `user1`, `user2`, ... by uid order stays. E04-005's acceptance line "`alicex` stays" is superseded. Implemented by the new bead E04-033 (`tasklens-2dl.37`), which E04-A1 (`2dl.1`, the `owner-desktop` capture) depends on, so no fixture is committed before the fix.
- Basis: ADR-013 scrub list, ADR-015 [verified]; the leak seen on the owner machine [verified].
- Edits: ADR-013 scrub sentence; new bead E04-033 (plan key and ID in `b2-collect-ops.md`); edges E04-033 on 2dl.8, 2dl.1 on E04-033.

### R-261 Fixtures carry scrubbed `run/udev/data` files
- Question: ADR-013 says a fixture may carry a scrubbed `run/udev/data/` directory with `ID_SERIAL`, `ID_SERIAL_SHORT` and `ID_WWN` replaced by `SCRUBBED`, but E04-005 never captured those files. Leader decision, 2026-10-04.
- Resolution: `capture-fixture` copies the regular files under `run/udev/data/` whose names start with `b`, `n` or `+pci:` (block, net, PCI; the ADR-016 udev row), 64 KiB cap each, and replaces the values of the `E:ID_SERIAL=`, `E:ID_SERIAL_SHORT=` and `E:ID_WWN=` lines with `SCRUBBED` before the ordinary text scrub. A missing directory is not an error. Implemented by E04-033 (`tasklens-2dl.37`), with regression tests.
- Basis: ADR-013, ADR-016 udev row, R-140 [verified].
- Edits: ADR-013 (the capture list names `run/udev/data`); bead E04-033.

### R-262 A later bead adds the field it defines the type for
- Question: Codex's edge scan found 18 pairs where an earlier bead A declares a field, variant, match arm, parameter or re-export whose type a later bead B creates, while B already depends on A (adding A on B would be a cycle). Raised by the leader from `scratchpad/codex/missing-edges.out`, 2026-10-04.
- Resolution, one rule: the earlier bead does not name the later type. It leaves an insertion comment `// <B plan key>: <field>` where the field, arm or parameter will go, and the later bead B adds the field, its initialiser, its `mod` or `use` line and its uses to A's file as a named chain edit (B's Files list names A's file). Only when the type is a small shared contract with no behaviour does it move into the earlier bead instead. Applied:
  - `8fx.16` / `8fx.17`: `Mode { Off, Rolling(RollingWindow) }`; E30-014 adds `Recording(RecordingState)` and the struct (same file).
  - `8hz.6` / `8hz.11`: `WorkerIo` has four fields in E08-004; E08-009 adds `pub shared: crate::dbus::SharedHandles` and its construction to `source.rs`.
  - `8q0.20` / `8q0.21` / `8q0.2`: E09-018's `lib.rs` line re-exports `EngineHandle` and `EngineHooks`; E09-019 appends `Engine`, E09-A2 appends `ShutdownReport` to the same `pub use`.
  - `8q0.3` / `8q0.10`: `Reducer` has no `suspend` field in E09-001; E09-008 adds `pub suspend: suspend::SuspendState` and its init to `reducer/mod.rs`.
  - `8q0.3` / `8q0.5` (the leader added `8q0.3` on `8q0.5`): `InvariantViolation` is the small shared type and moves to `reducer/devices.rs` (E09-003); `reducer/mod.rs` re-exports it (`pub use devices::InvariantViolation;`).
  - `9o0.15` / `9o0.16`: no `menu_targets` field in E28-020; E28-021 adds `menu_targets: Option<ConnMenuTargets>` to `ConnectionsPage`.
  - `bj6.30` / `bj6.31`: no `plane`, `hue`, `hex` fields in E21-027; E21-028 adds them (same file).
  - `crw.3` / `crw.6` and `crw.3` / `crw.17`: `PerformancePage` has neither `sidebar` nor `summary` in E24-001; E24-004 adds `pub(crate) sidebar: Entity<sidebar::DeviceSidebar>` and its construction, E24-015 adds `pub(crate) summary: Option<Entity<summary::SummaryView>>`, both to `pages/performance/mod.rs`.
  - `crw.8` / `crw.11`: `build_page` in E24-006 has only the placeholder arm; E24-009 adds the `Cpu` arm to `page.rs`, as E25 adds the other kinds.
  - `gma.1` / `gma.28`: `access_block(text: &'static str)` in E23-A1 (no button); E23-028 adds the `button: Option<AccessButton>` parameter and updates every caller.
  - `ndy.3` / `ndy.4` / `ndy.7`: `main.rs` in E13-001 has the `Sampler` arm exit 3 with "nvml backend not built" (like `ApiVersions`); E13-002 adds `mod nvml;` and the real `Sampler` arm; E13-005 adds the `Fake` arm.
  - `o55.2` / `o55.3`, `o55.5` / `o55.17`, `o55.7` / `o55.22`: no `bar`, `partitions`, `public_ip` field in the page beads; E25-002, E25-016 and E25-021 add them to `memory.rs`, `disk.rs` and `net.rs`.
  - `yrf.3` / `yrf.6`: `ChartView` has no `scale_state` in E19-001; E19-004 adds `pub(crate) scale_state: scale::ScaleState` and its init to `chart/view.rs`.
  Module lists written up front with empty "filled by" files stay as they are; they create no type dependency.
- Basis: `missing-edges.out` CYCLE_RISKS and the bead texts [verified]; 00-epics section 2 chain-edit rules.
- Edits: 00-epics section 2 (aggregator-file paragraph, one sentence); the 26 beads named above.

### R-263 `GraphColor` is one type, owned by E26-001
- Question: E21-032 (`bj6.35`) defines `pub enum GraphColor` (18 variants, `as_str`, `from_str`, `label`) in `window/menubar.rs`; E26 (`xdh.3`, `xdh.4`, `xdh.5`, `xdh.7`) uses a `GraphColor /* Accent | Named(AccentName) */` of another shape. Leader entry, 2026-10-04.
- Resolution: one type, `pub enum GraphColor { Accent, Coral, Orange, Amber, Yellow, Lime, Green, Mint, Teal, Cyan, Blue, Indigo, Violet, Lavender, Fuchsia, Pink, Rose, Slate }` with `as_str` (`"accent"` or the lower-case name, the stored value of R-083), `from_str` and `label`, defined in `crates/tl-ui/src/pages/minigraph/mod.rs` by E26-001 (`xdh.3`, P1). E21-032 (`bj6.35`, P3) imports it and keeps `graph_color_menu`; it depends on `xdh.3`. E26 texts say `GraphColor::Coral` and so on, not `Named(..)`.
- Basis: UI-SHL-085, R-083, R-231 [verified].
- Edits: beads xdh.3, xdh.5, bj6.35; edge bj6.35 on xdh.3.

### R-264 `HelloInfo` on each side of the probe protocol
- Question: `tl-gpu-probe` (`ndy.3`, `ndy.4`) and `tl-engine/src/gpu/client.rs` (`ndy.11`, `ndy.12`) both name a `HelloInfo`; the owners were unclear. Leader entry, 2026-10-04.
- Resolution: the wire type is `ProbeResponse::Hello { proto, driver: String, nvml: String, devices: Vec<GpuDeviceInfo> }` in tl-model `gpu_proto.rs` (E03-069, `as2.35`). Two crate-local structs sit on its two sides and keep their names: `sampler::HelloInfo { driver: String, nvml: String, devices: Vec<GpuDeviceInfo> }` in `tl-gpu-probe/src/sampler.rs`, owned by E13-001 (`ndy.3`), returned by `Backend::hello` and written into the wire variant by the sampler loop; `client::HelloInfo { proto: u16, driver: Arc<str>, nvml: Arc<str>, devices: Arc<[GpuDeviceInfo]> }` in `tl-engine/src/gpu/client.rs`, owned by E13-009 (`ndy.11`), built from the wire variant. Neither crate imports the other's struct.
- Basis: ADR-007, as2.35, ndy.3 and ndy.11 text [verified].
- Edits: beads ndy.3 (owner note), ndy.4 (uses `sampler::HelloInfo`; its stale "retried every 300 s" goes, R-155).

### R-265 The engine's in-flight record keeps `ActionKind`, not `ActionKindTag`
- Question: E11-A1 (`i3o.1`) declares `InFlight { kind: ActionKindTag, .. }` in tl-engine, and E27 (`hk6.9`) defines `pub enum ActionKindTag` in `tl-ui/src/pages/sys_table.rs`; tl-engine cannot use a tl-ui type and no engine bead defines one. Leader entry, 2026-10-04.
- Resolution: `InFlight.kind` is the request's `ActionKind` (tl-model `actions.rs`, cloned from the request), so the busy-unit and dedup logic matches on `ActionKind` variants. `ActionKindTag` is a tl-ui type owned by E27-008 (`hk6.9`) and used only by the system-table pages.
- Basis: ADR-005, i3o.1 and hk6.9 text [verified].
- Edits: bead i3o.1.

### R-266 Fixture capture of `run/udev/data` keeps an allowlist of lines
- Question: a test capture of `run/udev/data` showed disk serials in `S:disk/by-id/...` symlink lines, the WWN in `ID_WWN_WITH_EXTENSION`, MACs in `ID_NET_NAME_MAC`, and filesystem UUIDs and labels in other `E:` keys, none of which R-261's three-key scrub touched. Leader decision, 2026-10-04.
- Resolution: `capture-fixture` (E04-033, `tasklens-2dl.37`) writes a udev file from an allowlist: it keeps `I:`, `V:`, `G:` and `Q:` lines and the `E:` lines whose key is one of the 15 `UDEV_KEYS` of E04-030 (`tasklens-2dl.33`): `ID_MODEL`, `ID_MODEL_FROM_DATABASE`, `ID_VENDOR_FROM_DATABASE`, `ID_BUS`, `ID_WWN`, `ID_SERIAL`, `ID_SERIAL_SHORT`, `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`, `ID_PART_ENTRY_OFFSET`, `ID_PART_ENTRY_SIZE`, `ID_PATH`. Every `S:` line and every other `E:` key is dropped. The values of `ID_WWN`, `ID_SERIAL`, `ID_SERIAL_SHORT` and `ID_FS_LABEL` become `SCRUBBED` (R-261 plus the label). E04-030 must not read a key outside this list without adding it here and to the capture allowlist in the same change. Also recorded: the CI workflow (`.github/workflows/ci.yml`, E02-010, `v9m.12`) sets `VK_ICD_FILENAMES` to Ubuntu's `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`, and `scripts/ci.sh` tries that file and Arch's `lvp_icd.json` (commit `b2a0f5b`).
- Basis: the test capture on the owner machine [verified]; ADR-013, ADR-016 udev row, R-140, R-261 [verified].
- Edits: ADR-013 scrub sentences and ADR-016 udev row (`ID_PATH`); beads 2dl.37, 2dl.33.







## Owner questions

Answered on 2026-10-03. Q-1, Q-3, Q-4 and Q-6 are decisions D-007 to D-010 in `docs/DECISIONS.md`. Q-6 went against the recommendation: History mode shows a rolling 10-minute window while recording is off, so R-025's sentence about showing no data is replaced by D-010. Q-2 and Q-5 were not put to Oystein; their recommended answers (IEC units only, app-row Memory is cgroup `anon`) stand.

Only choices that no rule settles. Beads use the recommended answer until Oystein says otherwise.

The owner-decision topics of the settled open items (R-139 to R-235) were answered on 2026-10-04 and are D-012 to D-020. Every answer took the recommended option except the PKGBUILD licence, which is MIT (D-017).

### Q-7 Copyright holder for the MIT `LICENSE` file (R-258)
- Answered on 2026-10-04: `Øystein Krog` (D-021). `LICENSE` is committed (`daa15ab`) and E02-011 (`tasklens-v9m.13`) is closed.

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
- Options: (a) nothing, toast with an "Enable" button (R-025); (b) a 10-minute rolling window kept in RAM while the Processes tab is open (r2 PROC-074 [now PROC-086]).
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
# both fields optional: label omitted when empty, temp omitted for None; an entry with neither is removed (R-221)

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
gpu = "auto"                   # auto | software | pci:<bus id> | name:<adapter name> (R-237)

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
[lowmem]                       # P3 (R-152)
oom_score_adjust = -900        # the OOMScoreAdjust the installed watcher unit accepted; 0 when the manager refused -900 (U-24)

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

### (b) Keyboard shortcuts

Key contexts: Global, Table (every table), ProcessTable, ServiceTable, StartupTable, ConnectionTable, History, Information, Performance, RunDialog, Picker, ColumnPicker, PropsWindow (process, app and service Properties windows; nested KvGrid and Table), ServiceProps, AffinityDialog, PriorityDialog, SignalDialog, HandleSearch (nested Table), JournalList, LowMemory. One GPUI Action per row; menu items dispatch the same Action. Secondary windows get the Global rows that make sense in them (Esc, Ctrl+W, Ctrl+F, F5, Ctrl+Plus/Minus/0); the owning UI entry says which.

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
| Delete | StartupTable | Delete entry… (confirmation; toast "This startup entry cannot be deleted" when not allowed) (UI-SYS-044) |
| Ctrl+S / Ctrl+E / Ctrl+R | ServiceTable (group row) | nothing (UI-SYS-023) |
| Enter | ServiceTable (group row) | Toggle expand (UI-SYS-023) |
| Up / Down / PgUp / PgDn / Home / End | Information | Scroll by row, page, to top or bottom (UI-SYS-080) |
| Ctrl+C | Information | Copy selected text, else Copy all (UI-SYS-083) |
| 1..5 | History | Metric chip CPU, Memory, Disk, Network, GPU (UI-PRC-035) |
| Esc | History (scrub point set) | Return to live, before the global Esc rule (UI-PRC-030) |
| Up / Down, Space, Home / End | ColumnPicker list | Move, toggle, jump (UI-PRC-007) |
| Alt+1..Alt+7 | PropsWindow | Select tab 1 to 7 (`PropsSelectTab(n)`) (UI-PWN-004) |
| Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+PgDn / Ctrl+PgUp | PropsWindow, ServiceProps | Next / previous tab (UI-PWN-004, UI-SYS-030) |
| Ctrl+1..Ctrl+5 | ServiceProps | Select tab General, Log On, Recovery, Dependencies, Logs (UI-SYS-030) |
| Ctrl+F | PropsWindow | Focus the active tab's filter (UI-PWN-003) |
| F5 | PropsWindow | Refresh the active tab (`Command::DetailRefresh`) (UI-PWN-003) |
| Up / Down / Left / Right / Home / End, Enter, Ctrl+C | PropsWindow > KvGrid | Move between fields, activate a link, copy the field (UI-PWN-007) |
| Space | PropsWindow (Environment, masked row) | Reveal or hide the value (UI-PWN-016) |
| Tab order, Left / Right / Up / Down / Home / End, Space | AffinityDialog | Move between CPU tiles and toggle (UI-PWN-021) |
| Left / Right, PgUp / PgDn, Home / End, Up / Down, Enter | PriorityDialog | Slider steps 1 and 5, ends, stepper, Set priority (UI-PWN-022) |
| Up / Down, Enter | SignalDialog | Step the signal number, Send signal (UI-PWN-027) |
| Enter, Esc | HandleSearch | Start search; stop a running search, else close when idle (UI-PWN-030) |
| Delete / Ctrl+E, Shift+Delete / Ctrl+X, Enter, Ctrl+C, Ctrl+Shift+C | HandleSearch > Table | End Task, End Process, Go to process, copy paths, copy rows as TSV (UI-PWN-031) |
| Ctrl+A, Ctrl+C, Home / End, click, Shift+click, Ctrl+click | JournalList | Select all shown lines, copy selected lines, top / bottom (End re-sticks), select (UI-SYS-036, UI-SYS-037) |
| Esc, Ctrl+W, printable character | LowMemory | Clear filter then close; close; start filtering (UI-PWN-032) |

Removed from APP-031: Enter as toggle expand; "Force end" naming.

### (c) Names, paths, polkit actions and udev rules

Identity:
- App ID, Wayland `app_id`, desktop file stem, session bus name: `io.github.oysteinkrog.Tasklens`.
- Desktop file: `/usr/share/applications/io.github.oysteinkrog.Tasklens.desktop`. Autostart entry (Start at login): `~/.config/autostart/io.github.oysteinkrog.Tasklens.desktop`.
- D-Bus object `/io/github/oysteinkrog/Tasklens`: interface `org.freedesktop.Application` (`Activate`, `Open` returns an error, `ActivateAction` with `quit`, `tab`, `select-pid`).
- D-Bus object `/io/github/oysteinkrog/Tasklens/WindowBridge`: interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s json)`. P2.
- KWin script: `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js`, plugin name `tasklens-bridge`; one-shot scripts `tasklens-cmd-<n>`; picker script `tasklens-pick`.
- Tray items: `org.kde.StatusNotifierItem-<pid>-<n>`, objects `/StatusNotifierItem` and `/MenuBar`. P3.
- Transient scopes: `app-tasklens-run-<rand>.scope` (Run dialog), `app-tasklens-<escaped id>-<rand>.scope` (Run now). `<rand>` is 8 lowercase hex digits from 4 bytes of `/dev/urandom` (R-167).
- User units tasklens may install: `tasklens-lowmem.service` (P3). Test unit for integration tests: `tasklens-test.service`.

Polkit (policy file `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy`):
- `io.github.oysteinkrog.tasklens.helper`, `auth_admin_keep`: `pkexec /usr/lib/tasklens/tl-helper --session`. P3.
- `io.github.oysteinkrog.tasklens.setup`, `auth_admin_keep`: `pkexec /usr/lib/tasklens/tasklens-setup <rapl-enable|rapl-revert|sensors-detect|sensors-revert|polkit-rule on|off>`. P3 (`sensors-revert` P4, R-149).
- `io.github.oysteinkrog.tasklens.netmon`: reserved for `tl-netmon`. P4.
- Third-party actions used as-is: `org.freedesktop.systemd1.manage-units`, `manage-unit-files`, `reload-daemon`; `org.freedesktop.udisks2.ata-smart-update`, `filesystem-unmount-others`, `eject-media`; `org.freedesktop.policykit.exec` (Run as administrator; the P3 SMART fallback `pkexec /usr/bin/smartctl -a -j /dev/<d>`, R-088).
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

Environment variables read: `TASKLENS_LOG`, `TASKLENS_GPU_PROBE`, `TASKLENS_DEBUG`, `ZED_DEVICE_ID` (respected, set by tasklens only in P4 per R-070), `__NV_FORCE_ENABLE_X11_EGL_PLATFORM` (set on X11 with NVIDIA, ADR-010), `TASKLENS_RELAUNCHED` (set to `1` by tasklens on its one relaunch after GPU device loss, R-153), `TASKLENS_SETUP_ROOT` (setup test mode, honoured only when not root, R-168), `TL_BUILD_WRAP` (`xtask bench` builds through `heavy-build` when `1`, R-160), `TL_UPDATE_GOLDEN` (tests).

Command-line arguments (APP-052): `--start-minimized`, `--low-memory`, `--watch-memory`, `--tab <name>`, `--select-pid <pid>`, `--debug`, `--bench <scene> [--duration --warmup --json]` (ADR-014 scenes `perf-cpu`, `table-2000`, `bridge`, `static`, `soak` with `--duration 600 --warmup 0` (R-160), plus `history-scrub` in P3), `--gallery` (UI-FND-030; both behind the `bench` feature, R-138), `--force-full-memory-mode` (with `--low-memory` it starts the engine in `EngineMode::Full` and keeps the rescue window; alone it does nothing; D-015), `--app-id <id>` (replaces the `tasklens` directory component in the config, state, cache and runtime roots and is the Wayland `app_id` and the single-instance bus name; D-016), `--version`, `--help`. APP-052's `--bench [charts|table|history]` and `--bench-out` are replaced by ADR-014's form.

### (d) Number and unit formats

All formatting lives in `tl-model::units`. These forms are used in every language in v1: decimal point ".", grouping with a thin space, no locale-specific number forms (R-081). "Measured zero" means a valid sample equal to zero; missing values use validity glyphs (R-050), never a zero.

| Kind | Format | Measured zero | Examples |
|---|---|---|---|
| Percent (CPU, GPU, memory %, header totals, stats grids) | one decimal, `%.1f%%`. One exception: Copy details prints CPU as `%.2f%%` (R-094) | `0%` | `11.9%`, `3.4%`, `100.0%` |
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

Numeric cells use tabular figures and are right-aligned; numeric columns have fixed minimum widths from the template strings in add PROC-101 [now PROC-023] (`"999999 +99"`, `"100.0%"`, `"xxx Mbps"` or `"xxx MiB/s"`, `"xxx GiB/s"`, `"999 MiB"`, `"999999"`, `"999:59:59"`). Text columns have a 60 px minimum.

## Accepted UI strings

Strings the UI files wrote that no merged spec gives (R-121). Placeholders in angle or curly brackets are filled at run time. Each file's entries mark the string "(new)", "chosen" or [C] where it is used; this list is the record of acceptance.

- `docs/ui/system-tabs.md` (UI-SYS): "tasklens lists services from systemd. Neither the system manager nor the user manager answered."; "No matches"; "No services match the current filters."; "No startup entries match the filter."; "No connections match the filter."; "Clear filters"; "No startup entries"; "Nothing starts automatically when you log in."; "No connections"; "Could not load services"; "Could not load startup entries"; "Could not load connections"; "<failure text>: no authentication agent is running"; "Starting 6 services..."; "6 services started"; "4 of 6 services started. 2 failed."; "Details"; "Show all units"; "Reset to default"; "At least one unit type must stay selected"; " (reconnecting…)"; "Reloading service..."; "Service reloaded"; "Failed to reload service: <result>"; "Clearing failed state..."; "Failed state cleared"; "Failed to clear failed state: <error>"; "Could not open the unit file"; "This unit has no unit file"; "Changing startup type..."; "OpenRC services (N)"; " (not loaded)"; "Could not open the link"; "Copy value"; "Editing recovery settings comes in a later version."; "Enter unit names separated by spaces, for example failure-notify@%n.service"; "The computer will restart or power off when this happens."; "Saving recovery settings..."; "Recovery settings saved"; "Failed to save recovery settings: <error>"; "Remove tasklens settings"; "Remove tasklens recovery settings?"; "The unit goes back to the settings in its unit file."; "Remove"; "Ordering (N)"; "Filter logs"; "↓ N new lines"; "Could not open a terminal"; "Loading…"; "No older entries"; "Loading logs…"; "No log entries for this unit in the current boot."; "No log entries match the filter."; "journalctl not found"; "Reading the journal took too long."; "Could not read the journal"; "Copy command"; "Updating…"; "For this unit type only the time to start the program is measured."; "Disabled entries do not run at login."; "This entry did not start through systemd in this session."; "This entry is not used in this desktop (OnlyShowIn or NotShowIn)."; "The program for this entry is not installed (TryExec)."; "This unit is masked. Change its startup type in the Services tab."; "System entries cannot be deleted. Use Disable instead."; "This unit belongs to a package. Use Disable instead."; "3 startup entries enabled"; "2 of 3 startup entries enabled. 1 failed."; "4 startup entries launched"; "Delete startup entries"; "Permanently remove these <n> startup entries?"; "and <n> more"; "<n> entries cannot be deleted and are skipped."; "Not found"; " (removed)"; "Too many startup properties windows are already open"; "Sends reverse DNS queries for foreign addresses."; "Closing…"; "Close <n> listening sockets?"; "Too many connection properties windows are already open"; "not supported in this version"; "Namespaces: Host only"; "Namespaces: All"; "No information matches the filter."; "Information refreshed."; "Could not load system information"; "Not fetched"; "Fetching…"; "Modules: Not reported".
- `docs/ui/processes.md` (UI-PRC): "No processes match the filter"; "Clear filter"; "Already added"; "Process exited"; "Some values for other users' processes need administrator rights."; "History - Drag scrubber below to view timeline"; "213 processes at 12:04:31"; "Live"; "Show process history"; "Show live processes"; "autogroup"; "Scheduler: bfq"; "Columns reset to default"; "At least one column must stay visible"; "Name cannot be hidden"; "always on"; "n/a on this system"; "No columns match"; "9 of 96 columns shown"; "Processes table"; "Hide <label>"; "Sorted ascending"; "Sorted descending"; "Windows (N)"; "Untitled window".
- `docs/ui/process-windows.md` (UI-PWN): "This process has exited. Values are frozen at the last update."; "This app has no running processes. Values are frozen at the last update."; "Loading processes..."; "Loading memory maps..."; "Loading environment..."; "No processes."; "No processes match the filter."; "No memory maps."; "Anonymous, heap, stack and other mappings without a file"; "Kernel function the thread is sleeping in"; "Environment at process start"; "Choose at least one encoding"; "Sorting is available after the scan"; "Scan finished. 18 234 strings found."; "Scan cancelled. 18 234 strings found."; "Scan stopped: the process exited. 18 234 strings found."; "Enter a file name or path pattern, then press Search."; "Case-insensitive. * and ? work as wildcards."; "Found 1 match"; "Search stopped. Found 12 matches"; "CPU is offline"; "Not allowed by the process's cpuset"; " - reading current affinity…"; "Child processes start at normal priority."; "Set priority"; "Thread priority updated"; "Thread affinity updated"; "Send signal to N processes?"; "Send SIG{NAME} to N selected processes?"; "Send signal"; "Sent SIGHUP to 4 processes"; "Needs the KDE window bridge."; "Opening file location"; "Could not open the browser."; "Copied to clipboard"; "Writing core…"; "Waiting for authentication…"; "Ran as administrator"; "tasklens Set Affinity"; "tasklens Set Priority"; "tasklens Send Signal"; "Grant access"; "Risky capability".
- `docs/ui/performance.md` (UI-PRF): "Add graph >"; " · Graph Summary"; "No battery found"; "Power draw is not available. Showing Temperature."; "Turned off by the system reduce-motion setting"; "At least one performance page must stay visible"; "No space for more minigraphs"; "Minigraph layout reset"; "GPU monitor restarting"; "GPU probe not installed"; "Own processes only"; "Applies to all network values"; "Not reported"; "Done. No restart needed."; "Setup failed"; "Install lm_sensors to detect fan sensors"; "Readable through /etc/udev/rules.d/99-powercap.rules. tasklens leaves that file alone."; "/usr/lib/tasklens/tl-helper was not found"; "Needs setup"; "Enable CPU power draw?"; "Lets every local user read CPU energy counters. This can leak information about other programs. Continue?"; "3 partitions - GPT"; "No partitions"; "Additional mount points: /a, /b"; "Not supported by this desktop"; "tasklens Graph <n>"; "Fan name".
- `docs/ui/shell.md` (UI-SHL): "More tabs"; "+N more"; "This tab has no table"; "History recording off"; "History shows a flat list"; "Show live processes"; "All columns enabled"; "Copy all"; "Could not open the documentation"; "Detecting adapters…"; "Process <pid> not found"; "Reduced motion is on in your system settings"; "Your system's reduced motion setting turns all of these off."; "Numbers and dates keep the same format in every language."; "Theme saved"; "Theme sync turned off"; "Copy this code and send it to someone who uses tasklens."; "tasklens Collector Health"; "Not installed"; "Last used"; "idle"; "Waiting for authentication…"; "If tasklens crashed without a report, run coredumpctl info tasklens in a terminal."; "144 fps · frame p50 0.62 ms · p99 1.40 ms · stalls 0"; "Skip prompts"; "Reset all settings to their defaults? Themes and history are kept."; "Delete this custom theme?".
- Settled open items (R-139 to R-240): "Administrator access was refused."; "Authentication was cancelled."; "Not supported on this system." (R-240); "Already running"; "Permission denied. This process belongs to another user."; "The process has already exited."; "Done for {done} of {total} targets."; "The service job ended with result: {result}."; "The system call failed (error {code})."; "Internal error: {text}."; "Another action on this process is still running."; "Too many processes selected. The limit is 256."; "The selected process changed; wait for the list to update and try again."; "Kernel threads cannot be signalled."; "This process is protected. Confirm the warning to end it."; "This process belongs to another user. Administrator access is needed."; "This action is not available for this process."; "Writing core… 1.2 GiB"; "Close"; "Decrease"; "Increase"; "Could not import TaskSlinger settings"; "Grant access" (replaces "Grant access…", R-202); "Failed to reset settings"; "Show"; "Switched to Processes tab"; "Built-in themes cannot be deleted"; "Built-in themes cannot be edited. Copy it first."; "Imported { $count } themes"; "A startup action is already in progress."; "A connection action is already in progress."; the service, startup and connection summary forms of R-203; "PID: -"; "Process: No owner (kernel)"; "PID: ?"; "Process: Restricted"; "systemd-oomd may end processes before the rescue window opens."; "Could not set up the rescue window"; " The watcher runs with normal OOM priority."; "Not available" (the Ctrl+Esc settings row while `AppEnv.global_shortcut` is `None`); "Task manager for Linux" (clap about line); "Process window shown."; "No window found".
- `docs/ui/foundation.md` (UI-FND): "Collecting first sample"; "Last updated {age} ago"; "Not available on this device"; "Could not read: {last_error}"; "No items"; "Always on top"; "Always on top (on)"; "Not supported by this desktop"; "Could not open properties."; "Outcome unknown"; "Copied to clipboard (×5)"; "Clear filter"; "No properties match the current filter."; "Copy section"; "Copy row"; "Copy value"; "Could not start the privileged helper"; "Authentication cancelled"; "UI scale changed to N%"; "(custom)"; "(editing)"; "Finish or cancel the theme editor first"; "<name> copy N".

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
| U-12 | NVML per-process utilization (`nvmlDeviceGetProcessUtilization`) works unprivileged on driver 615 | r2 PROC-023 [now PROC-033], r2 conflicts 9 |
| U-13 | KWin 6.7.5 scripts cannot receive D-Bus signals, so commands must be one-shot scripts (kdotool pattern) | r2 PROC-059 [now PROC-052], conflicts 8 |
| U-14 | `org.kde.KWin.queryWindowInfo()` may already return `pid`; the script path is needed only if it does not | APP-006 |
| U-15 | GPUI's `prompt_for_paths` (portal FileChooser) pulls in no Tokio | r2 PROC-079 [now PROC-095], v1 PROC-091 [now PROC-095] |
| U-16 | `kglobalaccel` `setForeignShortcutKeys` sticks for a `.desktop` launch component | APP-051 |
| U-17 | TaskSlinger `TS1.` theme share-code encoding (needs one code generated under Wine). Outcome (2026-10-04): still open; TL1 import ships first and TS1 follows when the code exists (D-013, R-211) | APP-038 |
| U-18 | TaskSlinger's 17 accent hex values and the 11 non-default theme palettes (to be read from `themes.json` after "Create copy" under Wine). Outcome (2026-10-04): still open; the two default themes ship from hand-written values (E17-002) and the ten named palettes wait for `assets/themes/builtin.json` (D-012, R-210) | APP-035, APP-039 |
| U-19 | Steam `reaper` argv carries `AppId=<n>`; Lutris and Heroic set `LUTRIS_GAME_UUID` / `HEROIC_APP_NAME` | r2 PROC-009 [now PROC-011] |
| U-20 | zswap pool size is readable only through debugfs | r2 PROC-010 [now PROC-012] |
| U-21 | Interrupt time is never also in task `stime` when `CONFIG_IRQ_TIME_ACCOUNTING=y` (verified set here; the double-count claim is inferred) | add PROC-100 [now PROC-012] |
| U-22 | Process history store estimate (about 100 active processes × 18 B × 21,600 s) | r2 PROC-074 [now PROC-086] |
| U-23 | socket inode map rebuild under 20 ms on this machine | SYS-032 [now SYS-030] |
| U-24 | `OOMScoreAdjust=-900` is accepted for a user unit | r2 PROC-081 [now PROC-097] |
| U-25 | udisks2 2.10 NVMe module is present for NVMe SMART | PERF-046 [now PERF-051] |
| U-26 | The sysfs DRM connector `enabled` file is wrong on NVIDIA 615 (withdrawn claim; KScreen agreed with sysfs during the check; confirm whether all three monitors were on) | PERF-065 [now PERF-072], PERF addendum |
| U-27 | Readability of `/sys/kernel/security/lockdown`, efivars SecureBoot and `/sys/class/iommu` without root | PERF addendum, Security features |
| U-28 | `cpuinfo_avg_freq` and `amd_pstate_prefcore_ranking` exist per policy (reported present; recheck in the fixture) | PERF-025 [now PERF-028], PERF-028 [now PERF-031], PERF-035 [now PERF-040] |
| U-29 | `/dev/dri/card1` seat ACL allows the KMS read-only ioctls | PERF-065 [now PERF-072] |
| U-30 | `ureq` 3 with rustls has no Tokio in its graph. Outcome (2026-10-04): ureq 3.4.0's manifest has no `tokio` dependency (inferred from the manifest; `cargo xtask check-deps` confirms, R-142) | PERF-054 [now PERF-060], APP-056 |
| U-31 | ksni 0.3.6 with `async-io` and `blocking` features has no Tokio in its graph | ADR-010, PERF-078 [now PERF-088] |
| U-32 | `x11rb` XRes queries work against XWayland for the "X11 resources" column | r2 PROC-025 [now PROC-035] |
| U-33 | Wayland `wl_data_device` clipboard writes work from GPUI for all copy actions | v1 PROC-058 [now PROC-070] |
| U-34 | `fstatat` `st_size` of `/proc/<pid>/fd` gives the fd count on kernel 7.2 | r2 PROC-001 [now PROC-002] |
| U-35 | Hardware peak reporting: GPU power-limit and fan max values needed for fixed scales are reported by NVML on this card | PERF-015 [now PERF-018], PERF-061 [now PERF-067] |
| U-36 | CSD checklist passes on KWin (drag, double-click maximize, restore, resize edges, window menu, tiling); fallback to server decorations if not | ADR-010, D-006 |
| U-37 | Mission Center's `99-powercap.rules` keeps `energy_uj` readable after reboot (so the tasklens rule is only needed on other machines) | PERF-095 [now PERF-106] |
| U-38 | Columns ids 2, 4, 5, 8-12, 14, 15, 19-32, 34-61, 84-95 of TaskSlinger have no names tasklens lacks (string ids make this moot) | add PROC-028 [now PROC-037] |
| U-39 | A dismissed polkit prompt and a denial come back as the same access-denied error on systemd D-Bus calls (`manage-units`); phase 0 records both error names | R-117, UI-SYS-007 |
| U-40 | The static `NotoSansMono-Regular.ttf` loads under cosmic-text (zed#61660: faces without `m` are dropped) and has `tnum` | R-118, UI-FND-050, UI-SYS-036 |
| U-41 | gpui-kit 0.7.0 `Button` at rem 13 reaches 24 px with `xsmall` or `small`; the first component bead measures and picks | R-123, UI-FND-010 |
| U-42 | The `TlFocusRing` "last input was keyboard" heuristic does not show the ring after a mouse click; fallback is the kit's always-on ring | R-123, UI-FND-003 |
| U-43 | KWin on Wayland delivers the Shift modifier state within 500 ms of first focus, so "Shift held at launch" works; else the path is dropped on Wayland | R-124, UI-SHL-110 |
| U-44 | gpui-kit 0.7.0 `TitleBar` cannot hide its own caption buttons (so the shell draws its own); checked in the P1 title-bar bead | R-125, UI-SHL-003 |
| U-45 | The kit `Input` does not bind Ctrl+N, Ctrl+Q or Ctrl+W; `keymap_matches_table_b` runs with the filter focused | R-125, UI-SHL-024 |
| U-46 | gpui-pre 0.3.7 and gpui-kit 0.7.0 give no text selection on arbitrary elements; if `Label` gains it before the bead, the UI-PRF-020 fallback goes | R-126, UI-PRF-020, UI-FND-025 |
| U-47 | Re-shaping 32 per-core value labels at 10 Hz fits the ADR-014 `perf-cpu` budget (p99); else labels go to 5 Hz | R-126, UI-PRF-025 |
| U-48 | The kit `Skeleton` stops requesting frames once removed from the tree (first-load skeletons, UI-SYS-004, UI-FND-024) | UI-SYS-004 |
| U-49 | `journalctl -f --after-cursor` resumes without gaps or duplicates after a stop and restart of the child | R-118, UI-SYS-037 |
| U-50 | `/proc/<pid>/stat` field 22 plus boot ID is stable across the low-memory engine's stripped sweep, so `ProcessKey` revalidation works in `EngineMode::LowMemory` | R-119, UI-PWN-032 |
| U-51 | An unprivileged PSI trigger `full 200000 2000000` on `/proc/pressure/memory` is accepted on the owner machine (kernel 7.2); recorded by E32-A1's ignored test | R-151, PROC-097 |
| U-52 | Multipass creates a bridge named `mpqemubr<n>`; the prefix joins `VIRTUAL_NAME_PREFIXES` once seen on a Multipass install | R-162, PERF-054 |
| U-53 | The sysfs file that reports total VRAM for the Intel xe driver; E06-023 leaves VRAM total out for xe until it is named | R-164, PERF-064 |

