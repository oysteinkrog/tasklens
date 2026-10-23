> **Leader note.** This addendum was written against `features-proc.md` (the first version), so its PROC-096 to PROC-107 and its amendments use that file's IDs, not those in `features-proc-r2.md`. The reconciliation pass merges all three files by subject.
> - It assumes the Memory column is RSS. D-004 overrides that: Memory is RssAnon. The "Working set total" alias in PROC-102 and the Memory and Working set lines in PROC-098 must follow D-004.
> - Its cycle order (Tree, Grouping, List) and IEC disk labels (MiB/s) are recommendations for the reconciliation pass to settle across PROC, PERF and APP.

# PROC addendum: TaskSlinger cross-check folded in

This addendum adds PROC-096 to PROC-107 and amends 15 existing entries. It covers every item you listed, from sections A to C of `~/work/tasklens-ref/taskslinger/fable-taskslinger-crosscheck.md`. I checked `docs/spec/features-sys-app.md` (APP-002, APP-004, APP-005, SYS-010, SYS-035) so these entries line up with the chrome and services specs.

Two new facts from the owner's machine are used below:
- `CONFIG_IRQ_TIME_ACCOUNTING=y`. IRQ and softirq time is counted only in the `/proc/stat` irq and softirq fields, not in any task's `stime`.
- zram0 is active with about 16 GB `mem_used_total`. The only GPU is the NVIDIA card (`card1`, vendor 0x10de).

---

## M. Additions from the TaskSlinger cross-check

### PROC-096 End Process item
- Source: TS L29278, toasts "End process requested." L28845 and "Could not end process." L28846 | cross-check A.1
- Behaviour:
  - "End Process" sits directly under "End Task" in the row menu. It replaces the label "Force End Task" from PROC-051. The behaviour is unchanged: SIGKILL for a process row.
  - Tooltip: "Ends the process immediately (SIGKILL)".
  - Toasts: "End process requested." and "Could not end process."
  - Multi-select label: "End %d Processes".
  - App rows keep "Force End App" (KillUnit SIGKILL on the whole unit). A single "End Process" makes no sense on a row that holds many processes.
  - Keys stay as they were: Shift+Delete, and Ctrl+X while the table has focus.
- Linux data/mechanism: the same as PROC-051. `pidfd_open`, identity check, then `pidfd_send_signal(SIGKILL)`.
- Verdict: ADAPT. On Windows, End Task means a window close for apps and End Process means TerminateProcess. On Linux these become SIGTERM (End Task) and SIGKILL (End Process).
- Phase: P1
- Privilege: the same as PROC-050. EPERM gives a "Retry as administrator" button, which calls helper `signal(ProcId, SIGKILL)`.

### PROC-097 Multi-select priority, affinity, copy and end
- Source: TS toasts "Selected process priority updated." L28754, "Could not update selected process affinity." L28743, "Selected process values copied." L28750, "Selected process metrics copied." L28752, "End task requested for selected processes." L28744 | cross-check A.2
- Behaviour:
  - **Set Priority:** with 2 or more rows selected, Set Priority applies to every selected process (and to every process of a selected app row).
    - A class has a check mark only when all targets share that class. Otherwise no class is checked.
    - Toasts:
      - "Selected process priority updated." when all succeed.
      - "Could not update selected process priority." when all fail.
      - On partial failure, for example "Priority updated for 3 of 4 processes. 1 failed: permission denied", with a "Retry as administrator" button that retries only the failed targets.
  - **Set Affinity:** applies to all targets.
    - Each CPU entry is checked when every target has that CPU, and shows a mixed mark when only some do. Clicking a mixed entry sets that CPU on all targets.
    - Toasts: "Selected process affinity updated." and "Could not update selected process affinity.", with the same partial form as priority.
  - **Set I/O Priority:** follows the same rules, with toasts "Selected process I/O priority updated." and "Could not update selected process I/O priority."
  - **Copy:** copies the clicked column's value for every selected row, one per line, in view order. Toast "Selected process values copied."
  - **Copy all metrics:** with 2 or more rows, toast "Selected process metrics copied."
  - **End Task:** with 2 or more rows, toast "End task requested for selected processes." The confirmation dialog from PROC-050 still comes first.
- Linux data/mechanism:
  - Targets are captured as ProcIds when the menu opens.
  - Syscalls run once per target thread, as in PROC-057, PROC-059 and PROC-060.
  - One action request carries the whole target list and returns per-target results on the action result channel.
- Verdict: BUILD
- Phase: P2. Priority, affinity and I/O priority are P2. Copy and End in multi-select mode are P1.
- Privilege: the same as PROC-057, PROC-059 and PROC-060, per target.

### PROC-098 Copy process details (fixed format)
- Source: TS "Process details copied." L28761, format strings L35504-35513 | cross-check A.3
- Behaviour:
  - A new row menu item "Copy details" sits between "Copy" and "Copy all metrics". Ctrl+C runs the same action.
  - It copies this fixed block per selected row, with one blank line between rows:
    ```
    Name: firefox
    PID: 4321
    Parent PID: 2010
    CPU: 3.27%
    Memory: 812 MiB
    Private bytes: 1490 MiB
    Working set: 812 MiB
    Threads: 97
    Handles: 412
    Priority: Normal (0)
    ```
  - Field sources:
    - CPU is `%.2f%%` of the whole machine.
    - Memory is RSS.
    - Private bytes is Commit size (VmData + VmStk).
    - Working set is VmRSS. It equals Memory because the tasklens Memory column is RSS (PROC-020). The line stays so the TS format is kept.
    - Memory values are whole MiB (`%llu MiB`).
    - Handles is the fd count.
    - Priority uses the PROC-018 label.
  - A restricted value prints "restricted" and an unsupported one prints "n/a". Neither prints 0.
  - Toast "Process details copied.", or "Failed to copy to clipboard".
  - The visible-column TSV copy moves from Ctrl+C to Ctrl+Shift+C, with menu label "Copy rows as table". Toast "Copied to clipboard".
- Linux data/mechanism: snapshot values and the GPUI clipboard.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-099 Process jump target and its toasts
- Source: TS "Switched to Processes tab" L28709, "Process not found in current view" L28696, "Service not found in current view" L28700, "Could not open properties." L28728 | cross-check A.4 | SYS-010, SYS-035
- Behaviour: the Processes tab offers one "select process by identity" entry point. Services "Jump to process", Connections "Jump to process", Search File Handles "Go to process", the window picker and the history table all use it.
  - On success it does the following:
    1. switches to Processes, with toast "Switched to Processes tab" when the source was another tab;
    2. expands the group or tree path;
    3. selects the row and scrolls it to the centre.
  - The caller's own toast (for example "Switched to owning process.") replaces "Switched to Processes tab" when the caller has one.
  - When the ProcId is no longer in the snapshot (the process exited or the pid was reused), toast "Process not found in current view".
  - When the process exists but the filter or Hide system processes hides it, the caller's "not visible" toast appears with a "Show" button. The button clears the filter and turns Hide system processes off, then selects the row.
  - "Go to service" (PROC-065) for a unit that the Services tab does not list gives toast "Service not found in current view".
  - Properties (PROC-071) gives toast "Could not open properties." when the target exits between the click and the window opening, or when its `/proc/<pid>` directory cannot be opened. The window-cap toast "Too many process properties windows are already open" stays separate.
- Linux data/mechanism: a ProcId lookup in the current projection. A pid match alone is never enough: the start time must match too.
- Verdict: BUILD
- Phase: P2. Go to parent (PROC-064) uses the same entry point in P1.
- Privilege: none

### PROC-100 Pseudo-process rows
- Source: TS pseudo list "idle", "memory compression", "registry", "secure system", "system", "system idle process", "vmmemwsl" L35439-35445; display names L35392, L35420; customize-27 shows System, Secure System, Registry and Memory Compression rows | cross-check A.6
- Behaviour: tasklens shows two pseudo rows, which stand for kernel work that no process owns.
  - **"Interrupts"**
    - CPU is the irq + softirq share of total CPU time.
    - Every other column shows "n/a".
    - It is always present.
  - **"Memory compression (zram)"**
    - Memory is Σ `mem_used_total` over all zram devices. The tooltip shows original size, compressed size and ratio.
    - The Disk column shows Σ zram read and write rates.
    - CPU shows "n/a", because compression runs inside the faulting task or in kworkers, and that time is already counted there.
    - It appears only when a zram device has `disksize` > 0. It is present on the owner's machine.
  - Rules for both rows:
    - PID cell blank; Type "Kernel"; User "root"; no icon expander.
    - They sit in the System group, after "Kernel threads (N)". In Tree View and List View without grouping they are top-level rows, sorted like any other row.
    - They are excluded from the "%zu Processes" count.
    - Hide system processes hides them.
    - The row menu holds only Copy, Copy details, Copy all metrics and Add Minigraph.
  - TS's "System" row has no separate pseudo row on Linux. The existing "Kernel threads (N)" row (PROC-005), with kthreadd and its children, is its analogue.
  - TS's other pseudo processes:
    - "System Idle Process" and "Idle": not shown. TS keeps those names so it can exclude them; the screenshot shows no such row.
    - "Secure System": DROP. There is no secure kernel (VBS) on Linux.
    - "Registry": DROP. Linux has no registry hive.
    - "vmmemwsl": DROP. Linux has no WSL VM; KVM guests are ordinary qemu processes.
- Linux data/mechanism:
  - Interrupts: Δ(irq + softirq) / Δtotal from the `cpu` line of `/proc/stat`. With `CONFIG_IRQ_TIME_ACCOUNTING=y`, this time is not also in any task's `stime`, so nothing is counted twice. Without that option the row shows "n/a" with tooltip "Interrupt time is included in task time on this kernel" (check `/proc/config.gz`, or test whether irq stays 0).
  - zram: `/sys/block/zram*/mm_stat` field 3 for memory; `/sys/block/zram*/stat` sectors for the rate.
- Verdict: ADAPT. System and Memory Compression get Linux analogues; the other four are dropped or not shown.
- Phase: P2
- Privilege: none

### PROC-101 Column minimum widths and default widths
- Source: TS template strings "999999 +99" (PID), "100.0%", "xxx Mbps", "xxx GB/s", "999 %ls", "999999", "999:59:59" L35615-35626 | INI `[ColumnWidths] Processes` | cross-check A.9, C.1
- Behaviour: each column's minimum width is the measured width of a template string, plus 2 × 8 logical px cell padding, plus 12 px for the sort arrow on the active sort column.

  | Template | Columns |
  |---|---|
  | "999999 +99" | PID, PID parent, SID |
  | "100.0%" | CPU (all % kinds), GPU, GPU 0..15, Memory % |
  | "xxx Mbps" | Network, Network sent, Network received (bits mode). Bytes mode uses "xxx MiB/s" |
  | "xxx GiB/s" | Disk and all rate columns. TS writes "GB/s"; tasklens uses IEC units (PROC-104) |
  | "999 MiB" | memory columns ("999 %ls" with the widest unit label) |
  | "999999" | counts: Threads, Handles, Page faults, Context switches, Read ops, Write ops |
  | "999:59:59" | CPU time, CPU time kernel, CPU time user |

  - Text columns have a fixed minimum of 60 px: Name, User, Path, Description, Command line.
  - Measurement uses the table font with tabular numerals, at the current UI scale and row height. It is recomputed when the font or scale changes.
  - A drag below the minimum stops at the minimum.
  - Default widths at 100% scale (logical px), taken from TS's INI: Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342, Power usage 101. Every other column defaults to max(minimum, header text width).
  - "Resize columns to fit content" never sets a width below the template minimum.
- Linux data/mechanism: GPUI text layout (`text_system().layout_line`) of the template string, cached per (font, size, scale).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PROC-102 Column picker headings and alias text
- Source: TS "Columns", "Image", "Objects", "Service", "Connection" L28429-28433; alias entries `Working set private - "Memory"` L28427 and `GPU memory dedicated - "VRAM"` L28428 | cross-check A.10
- Behaviour:
  - The picker title is "Columns".
  - Group headings, in order: Image, Identity, Security, Scheduling, CPU, Memory, I/O, Disk, Network, GPU, Power, Objects, Linux. TS's "Image" heading replaces the earlier "File" heading. "Objects" holds Handles, Threads, GDI objects and Windows. TS's "Service" and "Connection" headings belong to the Services and Connections pickers, which the SYS spec owns.
  - Two entries carry TS-style aliases, giving the full metric name and then, in quotes, the short header label:
    - `Working set total - "Memory"`: the default Memory column (VmRSS). The separate `ws_total` column is removed because it duplicated Memory (see the PROC-020 amendment).
    - `GPU memory dedicated - "VRAM"`.
  - The filter field matches both the full name and the alias.
- Linux data/mechanism: none.
- Verdict: ADAPT. TS headings are used where they exist; the alias for Memory names RSS, because Memory is RSS here.
- Phase: P1
- Privilege: none

### PROC-103 Per-adapter GPU columns "GPU 0" to "GPU 15"
- Source: TS "GPU 0" .. "GPU 15" L35456-35471, [CL 0.8.23] | cross-check A.11, C.1 (16 ids, inferred 62-77)
- Behaviour:
  - There are 16 column slots, labelled "GPU 0" to "GPU 15". Slot N shows this process's utilization on adapter N.
  - Header line 2 shows that adapter's total utilization. The header tooltip gives the adapter name and PCI address.
  - The picker lists only slots for adapters that exist, under the GPU heading. On the owner's machine that is "GPU 0", the RTX 5070 Ti.
  - Adapter numbering is fixed by PCI bus address, ascending, across NVIDIA (from the probe) and DRM render nodes. The same numbering appears on the Performance tab "GPU N" pages.
  - All 16 slots are off by default.
  - App rows show the Σ over members.
  - These columns show utilization only. VRAM per adapter goes in the cell tooltip ("VRAM on GPU 0: 1.22 GiB").
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetProcessUtilization` smUtil per device, inside `tl-gpu-probe`.
  - Other GPUs: `/proc/<pid>/fdinfo` `drm-engine-*` per `drm-pdev`.
  - A device that does not support process utilization shows "n/a" (PROC-024 rule).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PROC-104 Value formats, zero values and units
- Source: TS formats "0 %ls", "0 Mbps", "0 MB/s", "0 KB/s" L35601-35609; [IMG processes.png] "16.00 KiB", "1.22 GiB" | cross-check A.13 | review §5 "Units" (binding: IEC memory units, selectable network bits or bytes)
- Behaviour, for every process-table cell:

  | Kind | Non-zero format | Measured zero |
  |---|---|---|
  | CPU and GPU % | "%.1f%%". A value that rounds to 0.0 counts as zero | "0%" |
  | Memory | auto IEC unit (B, KiB, MiB, GiB, TiB), 2 decimals: "16.00 KiB", "1.22 GiB". Below 1 KiB: "512 B" | "0 B" |
  | Disk and I/O rates | fixed MiB/s with 1 decimal: "0.4 MiB/s" | "0 MiB/s" |
  | Network, bits mode (default) | fixed Mbps with 1 decimal, SI: "2.3 Mbps". With auto-scale on: Kbps, Mbps, Gbps | "0 Mbps" |
  | Network, bytes mode | fixed MiB/s, same as disk. With auto-scale on: KiB/s, MiB/s, GiB/s | "0 MiB/s" |
  | Counts | grouped digits with a thin space, "12 345" | "0" |
  | CPU time | "h:mm:ss" ("999:59:59" at most, then "Nd hh:mm") | "0:00:00" |

  - TS writes "MB/s" and "KB/s". tasklens writes "MiB/s" and "KiB/s", because the review binds IEC units and the values are base 2.
  - Zero formatting applies only to measured zero. Missing values use the validity glyphs, never "0".
  - Dim zero values (PROC-034) applies only to a measured zero.
- Linux data/mechanism: formatting in `tl-model` units. Visible cells are cached per value (review §1 Text).
- Verdict: ADAPT. TS's density is kept with IEC labels.
- Phase: P1
- Privilege: none

### PROC-105 Toolbar "Toggle view mode" cycler
- Source: TS `chrome.view_mode`, "Toggle view mode" L28332, toast "View mode: %ls" L28638, names L28488-28490 | cross-check B.1, C.3 | APP-005 (the button)
- Behaviour:
  - Each click moves to the next of three presets, in TS's string order: Tree View → Task Manager Grouping → List View → Tree View.
    - "Tree View" sets `tree_view = true` and `grouping = false`.
    - "Task Manager Grouping" sets `tree_view = true` and `grouping = true`.
    - "List View" sets `tree_view = false` and `grouping = false`.
  - The current preset is named by the first rule that applies: grouping on gives "Task Manager Grouping"; tree on gives "Tree View"; otherwise "List View". So the combination "grouping with a flat list", which only the View menu can produce, counts as Task Manager Grouping, and the next click goes to List View.
  - Toast: "View mode: Tree View", "View mode: Task Manager Grouping" or "View mode: List View".
  - The button tooltip is "Toggle view mode (current: <name>)".
- Linux data/mechanism: it sets the PROC-095 keys. The engine rebuilds the projection with the selection kept by ProcId.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Open question: APP-005 lists the order as Grouping, Tree, List. Recommended answer: use TS's string order above (Tree, Grouping, List) and change APP-005 to match.

### PROC-106 Column visibility model
- Source: TS INI `ProcessColumnVisibilityMask=FFFFFFFFFFFFFFFF` while only the 10 ids with width > 0 show | cross-check B.2
- Behaviour:
  - TS treats a column as visible when its stored width is above 0. The mask is all ones and does not decide visibility.
  - tasklens does not copy that encoding. It stores visibility and width separately (`columns.visible` and `columns.widths`, by column ID). Hiding a column then showing it again brings back its old width.
  - The default visible set is TS's width > 0 set (Name, User, PID, CPU, Memory, Disk, Network, GPU, Description, Power usage) minus Power usage, as decided in PROC-015 and in architecture-review conflict 3.
  - tasklens never imports TS's INI, so the mask's meaning is not needed.
- Linux data/mechanism: TOML settings (PROC-095).
- Verdict: ADAPT
- Phase: P1
- Privilege: none

### PROC-107 Processes-tab toolbar buttons
- Source: TS [IMG processes.png vs performance.png] | cross-check A.8 | APP-002, APP-004, APP-005 (the buttons belong to the chrome spec)
- Behaviour:
  - The Hide system processes toggle and the Toggle view mode button appear only while the Processes tab is active. Shield and "+" appear on every tab (APP-002).
  - When the tab changes, the two buttons are removed, not just greyed out. The toolbar does not reserve their space.
  - In History mode (PROC-082) on the Processes tab:
    - Hide system processes stays active and also filters the History table.
    - The view-mode button stays visible but disabled, with tooltip "History shows a flat list", because the History table has no tree or groups.
  - Both buttons dispatch the same Actions as View > Process View and the setting toggles (gpui-component rule 10). Hide system processes (PROC-012) and the cycler (PROC-105) define what those Actions do.
- Linux data/mechanism: the title-bar toolbar's content depends on the active tab.
- Verdict: BUILD
- Phase: P1
- Privilege: none

---

## Amendments to existing entries

**PROC-004 Tree View / List View.** Replace the first behaviour bullet with:
- View > Process View holds three items: "Tree View" and "List View", which are a radio pair, and "Task Manager Grouping", which is a check item (PROC-005). The toolbar cycler (PROC-105) moves between three named presets of these two settings.

**PROC-012 Hide system processes toggle.** Replace the tooltip and toast line with the APP-004 wording, so both specs agree:
- Tooltip "Hide system processes" or "Show system processes". Toasts "Hiding system processes" and "Showing system processes".

Add this bullet: pseudo rows (PROC-100) are hidden too.

**PROC-015 Column group: identity and naming.** Add a line under the table: column min widths and default widths are set in PROC-101.

**PROC-020 Column group: memory.**
- Remove the `ws_total` row ("Working set total"). It was the same VmRSS data as `memory`.
- The `memory` row's label in the picker becomes `Working set total - "Memory"` (PROC-102).
- The header still reads "Memory".

**PROC-024 Column group: GPU.** Replace the "Per-adapter GPU columns" row with:
- "GPU 0 .. GPU 15 | gpu_0 .. gpu_15 | see PROC-103 (utilization per adapter, numbered by PCI address) | Σ | BUILD | P2".
- Per-adapter memory columns are dropped. Per-adapter VRAM appears in the cell tooltip instead.

**PROC-028 Unmapped TaskSlinger column ids.** Replace the behaviour and the open question with:
- TS's 96 ids are 80 named columns plus "GPU 0" to "GPU 15" (cross-check C.1). All 96 are now covered: PROC-015 to PROC-026 plus PROC-103.
- Partial id map, kept as a reference only:
  - confirmed: 0 Name, 1 User, 3 PID, 6 CPU, 7 Memory, 13 Disk, 16 Network, 17 GPU, 33 Description, 79 Power usage;
  - inferred: 18 VRAM, 62-77 GPU 0-15, 80 Power usage trend, 81-83 Company, Signer, Signature status.
- tasklens uses string IDs, so the unmapped ids (2, 4, 5, 8-12, 14, 15, 19-32, 34-61, 84-95) need no resolution.
- Remove the open question. Status is not a TS column; `state` stays a Linux extra (PROC-027).

**PROC-032 Row hover tooltip.** Replace the line list with this format, which keeps TS's line keys:
```
<command line, first 512 chars>
path: <exe path, or "restricted">
user: <name> (uid N)
Unit: <unit> (<Description>)
cgroup: <path>
Desktop file: <path>                 (app rows only)
Some values need administrator access.   (restricted rows only)
```
"Unit:" is the systemd analogue of TS's "Services:". Pseudo rows (PROC-100) show only a one-line description of what they count.

**PROC-038 Column reorder, resize and persistence.** Add this bullet: drags stop at the template minimum width (PROC-101), and visibility is stored separately from width (PROC-106).

**PROC-039 Multi-select.** Add this bullet: priority, affinity, I/O priority, copy and end all act on the full selection (PROC-097).

**PROC-040 Keyboard navigation.** Replace the Ctrl+C bullet with:
- Ctrl+C runs Copy details (PROC-098). Ctrl+Shift+C runs Copy rows as table (TSV).

In the Delete bullet, the action bound to Shift+Delete and Ctrl+X is now named "End Process" (PROC-096).

**PROC-048 Column picker.** Replace the headings bullet with the PROC-102 title, headings and alias rules.

**PROC-049 Row context menu.** Replace items 2 and 13-14 with:
- 2. End Process (SIGKILL; PROC-096). App rows: "Force End App".
- 13. Copy. 14. Copy details (PROC-098). 15. Copy all metrics.
- Later items are renumbered.

Pseudo rows (PROC-100) get only Copy, Copy details, Copy all metrics and Add Minigraph.

**PROC-051 Force End Task.** The entry is renamed "End Process" and its toasts change to the PROC-096 toasts. App-row behaviour is unchanged.

**PROC-058 Copy and Copy all metrics.**
- Move the fixed TS field list (Name, PID, Parent PID, CPU, Memory, Private bytes, Working set, Threads, Handles, Priority) out of Copy all metrics and into Copy details (PROC-098).
- Copy all metrics now copies "Label: value" for every column in the registry, in picker order, one block per row. Toast "Copied metrics to clipboard", or "Selected process metrics copied." for several rows.
- Ctrl+C is now Copy details, and TSV moves to Ctrl+Shift+C.

**PROC-064 Go to parent process and PROC-065 Go to service.** Add: both use the PROC-099 entry point and its toasts.

**PROC-072 General tab.** In Column 1, insert after Priority:
- Affinity: "All (0-31)", or the list with a count, e.g. "0-7,16-23 (16 CPUs)". Source: status `Cpus_allowed_list` of the leader thread. When any thread's list differs, append " (threads differ)". Clicking the value opens Set Affinity (PROC-059). Source TS L29043, cross-check A.21.

**PROC-073 Threads tab.** Replace "Priority (nice), Policy" in the column list with two columns:
- **Priority:**
  - "Variable" for SCHED_OTHER, SCHED_BATCH and SCHED_IDLE, because the scheduler adjusts their effective priority at run time. This mirrors TS's "Variable" (L35668).
  - "Realtime N" for SCHED_FIFO and SCHED_RR, with N = rt_priority from stat field 40.
  - "Deadline" for SCHED_DEADLINE.
  - The tooltip names the policy.
- **Base priority:** nice (stat field 19) for normal policies, or rt_priority for realtime threads. This mirrors TS's "Base priority" (L35667).

**PROC-095 Process view settings keys.**
- Add `copy_rows_tsv_shortcut = "ctrl-shift-c"`. This is fixed, not user-editable; it is listed only so the binding is documented.
- Remove nothing.
- `columns.visible` and `columns.widths` stay separate keys (PROC-106).

---

## Summary table (addendum)

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PROC-096 | End Process item | ADAPT | P1 |
| PROC-097 | Multi-select priority, affinity, copy and end | BUILD | P2 (copy and end P1) |
| PROC-098 | Copy process details (fixed format) | BUILD | P1 |
| PROC-099 | Process jump target and its toasts | BUILD | P2 |
| PROC-100 | Pseudo-process rows | ADAPT (Secure System, Registry, vmmemwsl dropped) | P2 |
| PROC-101 | Column minimum widths and default widths | BUILD | P1 |
| PROC-102 | Column picker headings and alias text | ADAPT | P1 |
| PROC-103 | Per-adapter GPU columns "GPU 0" to "GPU 15" | BUILD | P2 |
| PROC-104 | Value formats, zero values and units | ADAPT | P1 |
| PROC-105 | Toolbar "Toggle view mode" cycler | BUILD | P1 |
| PROC-106 | Column visibility model | ADAPT | P1 |
| PROC-107 | Processes-tab toolbar buttons | BUILD | P1 |

## Conflicts (addendum)

None of these new entries conflicts with the architecture review. Two cross-spec items need a decision:

1. **Cycle order.** APP-005 cycles Grouping, Tree, List. PROC-105 recommends TS's string order: Tree, Grouping, List. One of the two must change. I recommend changing APP-005.
2. **Disk-rate unit labels.** TS writes "MB/s". PROC-104 uses "MiB/s", because the review binds IEC units. The perf spec should use the same labels on the Performance tab.
