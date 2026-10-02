I have enough evidence. Here is the spec.

# PERF feature spec: system and device monitoring

Scope: the Performance tab, graphs, the system minigraph panel, tray graphs, the performance mini window, the Information tab, and the additional-setup flow.

Sources and what I checked:
- **Read:** PLAN.md, DECISIONS.md, both TaskSlinger inventories (including fable-taskslinger-crosscheck.md sections A to C), both Mission Center inventories, the architecture review and the GPUI notes. I also looked at `performance.png` and `information.png` myself. I did not run a Codex second opinion on these two reference images, because this is a subagent run.
- **Checked live on this machine (2026-10-02):**
  - `/sys/class/powercap/intel-rapl:0/energy_uj` can be read by any user. The reason is that `/etc/udev/rules.d/99-powercap.rules` (Mission Center's rule) is already installed.
  - The udev database gives DIMM details without root (`MEMORY_DEVICE_<n>_*`, serial numbers included).
  - `board_serial`, `product_serial` and `product_uuid` are mode 0400.
  - hwmon on this machine: `k10temp`, `nct6797` (fans 1 to 7, pwm 1 to 7), two `nvme`, `iwlwifi_1`, `r8169`, `i350bb` and `hidpp_battery_0`.
  - zram0 is active and zswap is off.
  - Network interfaces: `enp36s0f0` to `f3`, `enp38s0`, `wlan0` (down), `docker0`, `tailscale0`.
  - D-Bus services present: UPower (it lists a Logitech HID++ battery), UDisks2 and NetworkManager.
  - NVIDIA `card1` sysfs shows PCIe at 16 GT/s x16 now, with a 32 GT/s maximum. The connector `enabled` file reads "disabled" for active outputs, so it cannot be trusted on this driver.
  - nvidia-smi reports every NVML metric listed below as supported, except memory temperature, which reads N/A.
- **Not run:** I did not run NVML calls directly. Function support comes from the nvidia-smi fields, which use the same NVML calls.

Shared rules for every entry:
- **No zeros for missing data.** A value that is not valid shows a validity state (valid, warming up, stale, restricted, unsupported, missing device, failed). It is shown with TaskSlinger's fallback strings: "Unavailable", "Not reported", "Not supported", "Status not reported", "<Unknown>", "Unknown CPU", "Unknown Adapter". A tooltip gives the reason, for example "Needs additional setup" or "Restricted: needs root".
- **TaskSlinger's layout wins.** Graphs sit on top and a three-column stats grid sits below them, as in `performance.png`. Mission Center's right-hand info panel is not built.
- **Settings** go in versioned TOML under `$XDG_CONFIG_HOME/tasklens/`.

## Sidebar and navigation

### PERF-001 Device sidebar cards
- Source: TS 2.3 "Left sidebar", crosscheck A.14 | MC 2.0 "What each row shows"
- Behaviour: One card per device: a mini graph (about 116×56 logical px, filled area plus top line, faint grid), the current % at top left over the graph, a title and two detail lines. Clicking a card selects its page. The selected card has an accent outline.
  - CPU: "CPU", the model, then "4631 MHz · 52 °C".
  - Memory: "Memory", "34.68 GiB / 63.16 GiB", "Commit 69%".
  - GPU: "GPU N", the name, "52 °C".
  - Disk: "Disk N", "NVMe - /" (kind and primary mount), the model.
  - Network: the interface display name, its type, then the combined rate in accent colour ("0.01 Mbps"). A disconnected interface shows its state and the card is drawn at 0.6 opacity.
  - Fan: "Fan N", the label, then "1200 RPM · 45 °C".
  - Battery: "Battery N" or the UPower kind, the model, then "% · state".
  - Titles keep TaskSlinger's strings ("Disk %d", "GPU %d"). The page header uses the long form ("Processor", "Graphics adapter").
  - Default order: CPU, Memory, GPU, Disk, Network, Fans, Batteries. This follows TaskSlinger's `Columns.Performance=0,1,4,2,3` order, with Mission Center's fan and battery types added at the end.
- Linux data/mechanism: Each card reads the newest sample of its device's main series from the snapshot. No extra collection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-002 Hide/show page context menu
- Source: TS 2.3 (L28324-28326, L28650)
- Behaviour: Right-clicking a card shows "Select Pages" (opens edit mode, PERF-003), "Hide This Page" and "Show This Page". "Show This Page" appears only in edit mode, on hidden cards. Hiding the last visible page is refused with the toast "At least one performance page must stay visible". Hiding the selected page selects the next visible card.
- Linux data/mechanism: Hidden pages are stored in `performance.hidden_pages` by stable device key (PERF-005).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-003 Sidebar edit mode
- Source: MC 2.0 "Sidebar edit mode" (`summary_graph.rs:338-350`, `mod.rs:2907-2989`)
- Behaviour: Edit mode opens from "Select Pages" or from a pencil button at the top of the sidebar.
  - Each card gets a drag handle and an on/off switch. Hidden cards reappear with the switch off.
  - Header buttons: "Enable All", "Disable All", "Reset to Default".
  - Drag-and-drop reorders cards. A 3 px accent drop line shows the target.
  - "Done" leaves edit mode.
  - Reset clears the custom order and per-device overrides.
  - The at-least-one-visible rule from PERF-002 applies here too.
- Linux data/mechanism: Saves `performance.page_order = [keys]` and `performance.device_overrides = {key: show|hide}`. A per-device override wins over the type default (PERF-004).
- Verdict: BUILD. Mission Center's edit mode is merged into TaskSlinger's "Select Pages" item, because TaskSlinger has no reorder interface.
- Phase: P2
- Privilege: none

### PERF-004 Per-type default visibility and network groups
- Source: MC 5 "Devices" group, `summary_graph.rs:41-66`
- Behaviour: Under Settings > Performance > Devices there are switches for Disks, GPUs, Fans and Batteries. Network has sub-switches: Wired, Wireless, VPN/Tunnel, Virtual and Other.
  - Defaults are on for all except Virtual, which is off. Virtual covers `docker*`, `veth*`, `br-*`, `virbr*` and Multipass.
  - Loopback is never shown.
  - CPU and Memory always exist and can only be hidden per page.
  - A device plugged in later follows these defaults. This fixes Mission Center's bug where late devices ignore visibility outside edit mode.
- Linux data/mechanism: Type classification is in PERF-052.
- Verdict: ADAPT. Virtual interfaces are hidden by default (PLAN section 3). Mission Center shows everything.
- Phase: P2
- Privilege: none

### PERF-005 Device hotplug and stable keys
- Source: MC 2.0 (`mod.rs:2072-2657`) | review section 5 "Device identity/topology"
- Behaviour: Disks, interfaces, GPUs, fans and batteries appear and disappear while tasklens runs. A new device takes its saved place in the order, or else goes at the end of its type group. When the selected device vanishes, the next card is selected. History never joins across device generations. A re-plugged device starts a new series with a visible gap.
- Linux data/mechanism:
  - **Persistent keys** (saved in settings):
    - `cpu` and `memory`;
    - `gpu:<NVML UUID>` for NVIDIA, else `gpu:pci:<domain:bus:dev.fn>`;
    - `disk:<udev ID_WWN or ID_SERIAL>`, falling back to the by-path name;
    - `net:<ifname>`;
    - `fan:<hwmon name>:<device path>:<n>`;
    - `bat:<UPower NativePath>`.
  - **Runtime identity** adds a generation: major:minor plus a lifecycle counter for block devices, and netns plus ifindex plus a counter for interfaces.
  - **Events:** netlink `NETLINK_KOBJECT_UEVENT` (subsystems `block`, `net`, `hwmon`, `drm`, `power_supply`) and UPower `DeviceAdded`/`DeviceRemoved`. Rescan fallback every 5 s.
- Verdict: BUILD
- Phase: P1 for disks and network, P2 for the rest.
- Privilege: none (reading uevents over netlink needs no capability).

### PERF-006 Sidebar width splitter
- Source: TS 2.3 `Performance.NavWidth=252`
- Behaviour: A draggable splitter between the sidebar and the page. The default is 252 logical px, the range 180 to 400. The width persists.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-007 Jump keys and View submenu
- Source: MC 2.0 context menu "View" submenu, MC 6 "F1 to F8"
- Behaviour:
  - On the Performance tab, F1 to F8 select the 1st to 8th **visible** card. This fixes Mission Center's bug of counting hidden rows.
  - Every page's context menu has "View >" with CPU, Memory, Disk, Network, GPU, Fan and Battery. Each jumps to the first visible device of that type. Types with no visible device are disabled.
  - Up and Down move the selection when the sidebar has focus.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-008 Aggregate pages
- Source: TS 2.3 "Aggregate storage" (L28224), "Aggregate adapters" (L28228), card titles "Aggregate"/"Multiple" (A.14)
- Behaviour:
  - "Aggregate storage" sums active time (the busiest disk's %) and transfer rates over all physical disks.
  - "Aggregate adapters" shows GPU utilization (the highest engine across GPUs) and the VRAM sum.
  - Each page exists only when there are two or more devices of that type. Both are hidden by default and can be turned on in edit mode.
- Linux data/mechanism: Sums are taken over whole disks only, never partitions or dm/md devices, so nothing is counted twice (review section 5). GPU values come from PERF-059.
- Verdict: BUILD
- Phase: P4
- Privilege: none

### PERF-009 Graph summary mode
- Source: MC 2.0 "Summary mode" (Ctrl+M, `window.rs:585-622`)
- Behaviour: Ctrl+M, or "Graph Summary View" in any page's context menu, hides the title bar menus, tab bar, sidebar and stats. The window then shows a tiled grid of the **main graph of every visible device**, each with its title and current value. Esc or Ctrl+M leaves the mode. Tab switching is blocked while it is on.
- Linux data/mechanism: None.
- Verdict: ADAPT. Mission Center shows only the one selected page. tasklens tiles all devices, because "only one device visible at a time" is the main complaint about Mission Center (MC inventory, UX problem 1).
- Phase: P2
- Privilege: none

### PERF-010 Copy device info
- Source: MC 2.0 "Copy" (`data_summary()`, e.g. `cpu.rs:798-873`) | TS 8 toast "Copied metrics to clipboard"
- Behaviour: Ctrl+C on a Performance page, or "Copy" in its context menu, copies plain text. The first line is the page heading, the second the model, then aligned `    Label: value` lines (4-space indent). Every stats-grid field of that page is included; nothing is left out the way Mission Center leaves fields out. Missing values print their fallback string. Toast: "Copied metrics to clipboard".
- Linux data/mechanism: GPUI `write_to_clipboard` (Wayland data-device).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-011 Page layout and fallback strings
- Source: TS `performance.png`, crosscheck A.19, A.20
- Behaviour: Each page has:
  - a header with the device name at left and page toggles at right;
  - the graph area;
  - a stats grid of label (secondary text) over value (primary text), three columns, rows in TaskSlinger order.
  - Mission Center fields that TaskSlinger lacks are appended after TaskSlinger's fields.
  - Formats: up time "2 days, 2h 38m 15s" (" day, " when singular) and caches "%ld KB".
  - Rows a device cannot supply show "Not supported". Rows that need privilege show "Unavailable" with a reason tooltip. Mission Center hides missing rows; tasklens shows them, because TaskSlinger keeps a stable grid.
  - Values are selectable text.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1
- Privilege: none

## Graph behaviour

### PERF-012 Graph element
- Source: TS 9 "Layout" (PLAN section 9) | MC 2.0 "Graph engine" | review section 4
- Behaviour:
  - The newest sample is at the right edge.
  - The area is filled in the series fill colour, with a 1.5 px brighter top line.
  - The faint vertical grid scrolls with the data. One line every 60 logical px; horizontal lines at 25/50/75%.
  - 1 px outline in the series colour at 40% alpha, 4 px corner radius (matches the "Rounded Corners" setting).
  - Title at top left and current value at top right in the panel and sidebar sizes.
- Linux data/mechanism: An app-owned GPUI element: background and grid as quads; fill and stroke as cached linear paths, rebuilt only when samples, size, scale, y-range or style change. Contiguous valid runs are drawn separately. When there are more samples than pixels, each pixel bucket keeps its min and max.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-013 Time range
- Source: TS 4.2 Time Range (30 s, 1 min, 5 min, 10 min) | MC "Data Points" 10 to 600 and the "Utilization over N" label
- Behaviour:
  - Each graph family has a "Time Range >" choice of 30 seconds, 1 minute (default), 5 minutes or 10 minutes.
  - The Performance tab has one range for all its pages: Settings > Performance > "Graph time range". Each minigraph card has its own range.
  - The graph's lower-left label reads "Utilization over 60 seconds" (Mission Center wording, words per metric: "Throughput over", "Memory usage over").
  - Mission Center's data-point slider is replaced by this choice.
- Linux data/mechanism: Ranges up to 60 s read the fast tier and longer ranges read the 1 Hz tier (PERF-014).
- Verdict: ADAPT. Data points become TaskSlinger's fixed ranges, so the duration no longer depends on the refresh interval.
- Phase: P1 for 30 s and 60 s; P2 for 5 and 10 min.
- Privilege: none

### PERF-014 History tiers
- Source: review section 4 "History structures"
- Behaviour: The user sees no difference. Graphs show up to 10 minutes.
- Linux data/mechanism: Every series has two `SeriesRing`s of capacity 602:
  - the fast tier, at the source rate (10 Hz CPU while visible, else the source's rate);
  - a 1 Hz tier holding the per-second mean, plus min and max kept for drawing.
  - The engine owns both and publishes views. About 80 series × 2 × 602 × 16 B ≈ 1.5 MB.
- Verdict: BUILD
- Phase: P1 for the fast tier, P2 for the 1 Hz tier.
- Privilege: none

### PERF-015 Scaling modes
- Source: MC 2.0 scaling modes | review "Units and graph ranges"
- Behaviour:
  - **Percentages:** fixed 0 to 100.
  - **Memory, VRAM:** fixed 0 to total.
  - **Commit:** 0 to max(CommitLimit, peak in view).
  - **Rates (disk, network, GPU PCIe):** the scale rises at once and falls only after 10 s below half scale.
  - **Network fixed mode:** when "Scale network graphs dynamically" is off, the scale is the link speed. If the link speed is unknown, the graph falls back to dynamic.
  - **Temperature:** sticky up/down, starting at 35 to 45 °C.
  - **Power and clocks:** sticky up from 0. When a hardware maximum is known (GPU max clock, power limit, fan max RPM), the scale is fixed at that maximum.
  - **Stacked CPU:** 0 to 100 × n.
  - **Battery power:** symmetric around 0.
- Linux data/mechanism: Computed in the UI from the visible samples.
- Verdict: ADAPT. Mission Center's nine modes collapse to these rules. The fall rule follows the review.
- Phase: P1
- Privilege: none

### PERF-016 Axis maximum rounding and label
- Source: MC Pow2/Pow10 rounding, `disk.rs:516-533`
- Behaviour: Non-fixed scales round up to 1, 2 or 5 × 10ⁿ for decimal units, or to a power of two for binary units. The maximum shows at top right of large graphs, for example "100 Mbps" or "4 GiB/s".
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-017 Smooth (sliding) graphs
- Source: TS `UI.SmoothGraphs=1` | MC "Sliding Graphs" (50 ms timer) | review section 4 "Smooth time mapping"
- Behaviour: "Smooth graphs" defaults to **on**. When on, graphs scroll continuously at the monitor's refresh rate (144 Hz here), drawn one sample interval behind the newest data. When off, graphs jump one step per sample. When the window is hidden, or reduced motion is set, sliding stops.
- Linux data/mechanism: One `request_animation_frame` chain per window, active only while a visible graph is moving. The cached path is translated each frame.
- Verdict: ADAPT. TaskSlinger's meaning and default are kept. Mission Center's 50 ms timer is replaced by compositor-paced frames.
- Phase: P1
- Privilege: none

### PERF-018 Curved lines
- Source: MC "Smooth Graphs" (cubic Bézier)
- Behaviour: "Curved lines" in Settings > Performance, off by default.
- Linux data/mechanism: Monotone cubic (Fritsch-Carlson) curves through the samples. This never overshoots, which meets the review's no-overshoot rule.
- Verdict: ADAPT. Monotone curves replace Mission Center's Bézier, which can overshoot.
- Phase: P4
- Privilege: none

### PERF-019 Gaps and stale data
- Source: review "Data validity", `Sample.flags` | MC NaN line breaks
- Behaviour:
  - Invalid samples (suspend, collector restart, device re-plug, unsupported) break the line. The gap shows as empty space with a dotted baseline.
  - When the newest data is older than 3 × the source period, the value shows in secondary colour with "(stale)" in its tooltip.
  - After suspend/resume, a gap is inserted and counter baselines are reset.
- Linux data/mechanism: Suspend is detected when the `CLOCK_BOOTTIME` minus `CLOCK_MONOTONIC` jump exceeds 2 s, or from logind `PrepareForSleep`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-020 Hover readout
- Source: TS 4.2 "Hover shows a value marker" (CL 0.8.28), " avg %.1f%s ", " peak %.1f%s ", " %u samples" | MC has none
- Behaviour: On every large graph and minigraph, hovering shows a vertical marker and a dot on each series. A tooltip shows the time offset ("-12s"), each series value, and the "avg", "peak" and "samples" figures for the visible window.
- Linux data/mechanism: Hit test only on hover.
- Verdict: BUILD. TaskSlinger has it and Mission Center lacks it.
- Phase: P2
- Privilege: none

### PERF-021 Graph colours
- Source: TS PLAN section 9 graph colours, TS 6 "Graph accents" | MC per-type colours
- Behaviour:
  - Per type: CPU #0D546D fill / #0A7096 line; kernel line #C1514E; Memory #7E6A9A; GPU #508A59 / #71B768; Disk #D9B567; Network #359CAB.
  - Fan uses the "Violet" accent and Battery the "Mint" accent. TaskSlinger has no colour for these; tasklens picks them from its own accent list.
  - Two devices of the same type share a colour (both apps do this).
  - Minigraph and tray cards can be recoloured with "Graph Color >": "Accent" plus 17 named accents.
  - Themes may override the graph colours.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1 for the palette; P3 for per-card colour.
- Privilege: none

### PERF-022 Units
- Source: TS "Network transfer unit", "Auto-scale network transfer rates" | MC 5 "Units" (bytes/bits and base 10/base 2 per class)
- Behaviour: Settings > Units has three groups.
  - **Memory:** Base 2 (KiB, MiB, GiB; default) or Base 10.
  - **Disk:** bytes, Base 2 default (MiB/s).
  - **Network:** "Bits per second" (default) or "Bytes per second", plus Base 10 (default) or Base 2.
  - "Auto-scale network transfer rates" defaults to on. When on, the unit changes from Kbps to Mbps to Gbps by size. When off, the unit is fixed at Mbps or MB/s.
  - Number format (Mission Center rule): 2 decimals below 10, 1 below 100, else 0.
  - Rates always carry "/s" or "ps".
- Linux data/mechanism: Formatting lives in `tl-model`.
- Verdict: ADAPT. Mission Center's groups are merged with TaskSlinger's auto-scale. TaskSlinger's `NetworkRateAutoScale` defaults to 0, but tasklens defaults auto-scale to on, because a fixed Mbps unit shows "0.00 Mbps" for idle links.
- Phase: P1 for the defaults; P2 for the settings page.
- Privilege: none

### PERF-023 Legends and secondary series
- Source: TS legend in the first cell ("Total", "Kernel", crosscheck A.28), network "Upload" legend (A.15) | MC dashed secondary series
- Behaviour: Legends are drawn inside the first graph of a page, top left, as coloured line swatches with labels:
  - "Total"/"Kernel" (CPU);
  - "Download"/"Upload" (network; download filled, upload a dashed line);
  - "Read"/"Write" (disk; write dashed);
  - "Dedicated"/"Shared" (GPU memory);
  - "Encode"/"Decode";
  - "GPU clock"/"Memory clock";
  - "RPM"/"PWM";
  - "Used"/"Committed" (memory; committed dashed, which also covers Mission Center's dirty series as a thin unfilled line).
- Linux data/mechanism: Dash pattern 5/5 px through `PathBuilder::dash_array`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

## CPU page

### PERF-024 CPU header and graph mode
- Source: TS "Overall"/"Per core" toggle (L28233-28236) | MC `performance-page-cpu-graph` 1 to 4
- Behaviour: The header shows the CPU name. At right is a mode button with the choices "Overall", "Per core" (default), "All cores overlaid" and "All cores stacked". A "Kernel on"/"Kernel off" toggle sits next to it. Both persist (`performance.cpu_mode`, `performance.cpu_kernel`, default on).
- Linux data/mechanism: `/proc/stat` (PERF-030).
- Verdict: ADAPT. TaskSlinger's two-state toggle becomes a four-choice button so Mission Center's two extra modes fit.
- Phase: P1 for Overall and Per core; P2 for the other two.
- Privilege: none

### PERF-025 Per-core grid
- Source: TS grid of "CPU %d" charts | MC `compute_column_count`
- Behaviour:
  - One chart per logical CPU, titled "CPU N", with the % at top right.
  - The column count is ceil(√(2n)), so 16 CPUs give 6 columns (matches TaskSlinger) and 32 give 8 columns in 4 rows. Cells are at least 120 logical px wide; the count is reduced when needed.
  - Hovering a cell's title shows a tooltip: "Core C, thread T, 4.2 GHz".
  - This is the 32-chart frame-budget case for phase 0.
- Linux data/mechanism:
  - `/proc/stat` `cpuN` lines. Per CPU, at 10 Hz while visible: used = user + nice + steal, kernel = system + irq + softirq, idle = idle + iowait, guest excluded.
  - Core and thread come from `/sys/devices/system/cpu/cpuN/topology/{core_id,thread_siblings_list}`.
  - Clock comes from `cpufreq/policyN/cpuinfo_avg_freq`, falling back to `scaling_cur_freq`.
  - CPU hotplug: offline CPUs show "Offline".
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-026 Kernel time overlay
- Source: TS "Kernel on" red line | MC "Show Kernel Times" dashed
- Behaviour: When on, each per-core and overall chart adds the kernel % as a #C1514E line with no fill (TaskSlinger style, solid). Overlaid and stacked modes show no kernel line.
- Linux data/mechanism: system + irq + softirq deltas.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-027 Overlaid and stacked modes
- Source: MC 2.1 values 3 and 4
- Behaviour: "All cores overlaid" draws one large chart with every core's line at opacity 1/n, minimum 0.08. "All cores stacked" draws stacked filled bands with a maximum of 100 × n%. The y-label then reads "3200%".
- Linux data/mechanism: Same per-CPU series.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-028 Core type marks
- Source: TS "Performance core"/"Efficiency core" icons (L28239-28240)
- Behaviour:
  - On hybrid CPUs, each cell shows a small P or E icon.
  - On AMD with preferred-core ranking, the highest-ranked cores show a small star, and the tooltip reads "Preferred core (rank 236)".
  - Otherwise there is no icon. This machine shows stars only if the ranking differs between cores.
- Linux data/mechanism:
  - Intel: `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus`.
  - AMD Zen 5c hybrids: distinct `cpu_capacity` values.
  - Preferred cores: `cpufreq/policyN/amd_pstate_prefcore_ranking` (present here).
- Verdict: ADAPT. The marks come from Linux topology files. The AMD preferred-core star is new.
- Phase: P2
- Privilege: none

### PERF-029 CPU bottom graph
- Source: MC `performance-page-cpu-graph-bottom` (power, clock, temperature)
- Behaviour: Under the chart area sits one wide graph with a choice: "None", "Power draw" (W, default), "Clock speed" (GHz, average plus a dashed maximum-core line) or "Temperature" (°C). Choices without data are disabled. When power is missing, the default falls back to Temperature, then Clock. Label: "Power draw over 60 seconds", with the max value at top right.
- Linux data/mechanism: PERF-033, PERF-034, PERF-035, at 1 Hz.
- Verdict: BUILD. TaskSlinger has no such graph.
- Phase: P2
- Privilege: power needs RAPL readability (PERF-101). The fallback is the disabled choice with the tooltip "Needs additional setup".

### PERF-030 CPU live stats
- Source: TS stats rows (L28241-28261) | MC "Live stats panel"
- Behaviour: Grid rows, in order:
  - Utilization, Kernel time, Speed;
  - Processes, Threads, Handles;
  - Up time, Base speed, Sockets.
  - Then Mission Center additions: Power draw, Temperature, Load average (1/5/15).
- Linux data/mechanism:
  - **Utilization:** `/proc/stat` `cpu` line.
  - **Speed:** mean of `cpuinfo_avg_freq` over all policies, falling back to `scaling_cur_freq`, then `/proc/cpuinfo` MHz. The tooltip gives the maximum core.
  - **BogoMIPS fallback:** shows "(?)" with Mission Center's tooltip.
  - **Processes:** count of numeric `/proc` entries. **Threads:** the sum from the process sweep, or the `procs_running` and `processes` fields when no sweep runs.
  - **Handles** is relabelled "Handles (open files)": field 1 of `/proc/sys/fs/file-nr`.
  - **Up time:** `CLOCK_BOOTTIME`.
  - **Base speed:** `cpufreq/base_frequency`, then `acpi_cppc/nominal_freq` (amd-pstate), then `bios_limit`.
  - **Sockets:** distinct `physical_package_id`.
  - **Load average:** `/proc/loadavg`.
- Verdict: ADAPT. Handles become open files. Load average is added.
- Phase: P1
- Privilege: none

### PERF-031 CPU static stats
- Source: TS Cores, Logical processors, L1/L2/L3 | MC Cpufreq driver, governor, EPP
- Behaviour: Rows: Cores, Logical processors, Virtualization, Running in VM, CPU security, Hypervisor, L1 cache, L2 cache, L3 cache, Cpufreq driver, Governor, Power preference (EPP), Platform profile, Microcode.
- Linux data/mechanism:
  - Cores: distinct (package, core_id).
  - Caches: `/sys/devices/system/cpu/cpu*/cache/index*/{level,type,size,shared_cpu_list}`, de-duplicated by `shared_cpu_list`. L1 is data plus instruction. Shown as "512 KB".
  - `cpu0/cpufreq/{scaling_driver,scaling_governor,energy_performance_preference}`: here "amd-pstate-epp", "powersave", "balance_performance".
  - `/sys/firmware/acpi/platform_profile`, falling back to power-profiles-daemon `ActiveProfile` over D-Bus.
  - Microcode from `/proc/cpuinfo`.
- Verdict: BUILD
- Phase: P1 for cores, logical processors and caches; P2 for the rest.
- Privilege: none

### PERF-032 Virtualization and security rows
- Source: TS Virtualization, Virtualization-based security, Security services, Hypervisor platform (L28256-28258, crosscheck A.18) | MC Virtualization, Virtual machine
- Behaviour:
  - **"Virtualization":** "Enabled (KVM available)", "Supported, KVM module not loaded" or "Not supported".
  - **"Running in VM":** "No", or the hypervisor name. This replaces "Virtualization-based security".
  - **"CPU security":** "N mitigated, N not affected, N vulnerable". The tooltip lists each entry. This replaces "Security services" (Credential Guard and the other Windows values).
  - **"Hypervisor":** "None", or a vendor.
- Linux data/mechanism:
  - `svm`/`vmx` in `/proc/cpuinfo` flags plus `/dev/kvm` (present here).
  - systemd `org.freedesktop.systemd1.Manager.Virtualization`.
  - `/sys/devices/system/cpu/vulnerabilities/*`.
  - The `hypervisor` cpuinfo flag plus `/sys/hypervisor/type`.
- Verdict: ADAPT. VBS and Windows security services have no Linux meaning, so their rows carry the nearest Linux facts.
- Phase: P2
- Privilege: none

### PERF-033 CPU power (RAPL)
- Source: MC "Power: RAPL"
- Behaviour: "Power draw" in watts on the stats grid and in the bottom graph. The tooltip splits Package and Core.
- Linux data/mechanism:
  - `/sys/class/powercap/intel-rapl:0/energy_uj` (package-0) and `intel-rapl:0:0` (core). The AMD driver also uses the `intel-rapl` names.
  - Rate = Δenergy_uj / Δt. Counter wrap uses `max_energy_range_uj`.
  - Polled at 1 Hz. The file is kept open and read with `pread`.
- Verdict: BUILD
- Phase: P2
- Privilege: The kernel default is mode 0400, root only (since the PLATYPUS fix, CVE-2020-8694). It is readable here only because of the existing udev rule. Fallback: "Unavailable" with a "Set up…" link to PERF-101.

### PERF-034 CPU temperature
- Source: MC "Temperature: hwmon k10temp, coretemp or zenpower"
- Behaviour: "Temperature" in °C on the stats grid, the CPU card and the bottom graph. The tooltip lists every sensor of that chip (Tctl, Tccd1, Tccd2).
- Linux data/mechanism: Find hwmon by `name` in this order: `k10temp`, `zenpower`, `coretemp`, `cpu_thermal`. The main reading is the `tempN_input` whose label is "Tctl" or "Package id 0", else temp1. 1 Hz.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-035 CPU clock
- Source: TS "4631 MHz" | MC Speed
- Behaviour: Average MHz on the card and the stats grid, per-core MHz in cell tooltips, and the clock bottom graph.
- Linux data/mechanism: `cpuinfo_avg_freq` (APERF/MPERF-based, present here) per policy, at 1 Hz while shown. Falls back to `scaling_cur_freq`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

## Memory page

### PERF-036 Memory graphs
- Source: TS "Physical memory", "Commit", "Swap" (L28573-28575) | MC memory and swap graphs
- Behaviour: Three graphs in a column:
  - "Physical memory": used filled, committed dashed and dirty as a thin line. Max = total.
  - "Commit": Committed_AS against CommitLimit. It can go above 100%, and the label says so.
  - "Swap": used. Hidden and disabled when there is no swap.
  - The context menu has "Show memory composition" and "Show swap usage" (Mission Center), both persisted and default on.
- Linux data/mechanism: `/proc/meminfo` at 1 Hz. Used = MemTotal − MemAvailable (MemFree when Available > Total). Swap used = SwapTotal − SwapFree.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-037 Memory composition bar
- Source: TS "Memory composition" (In use, In use (compressed), Modified, Standby, Free) | MC composition widget
- Behaviour: A horizontal bar with five segments and hover tooltips. Each tooltip gives the amount and a one-line meaning, for example "Standby: cache the kernel can drop".
- Linux data/mechanism:
  - Free = MemFree.
  - Modified = Dirty + Writeback.
  - Compressed = zram `mm_stat` field 3 (mem_used_total) + meminfo `Zswap`, clamped to In use.
  - In use = Total − Available − Compressed.
  - Standby = Total − In use − Compressed − Modified − Free, clamped at 0.
- Verdict: ADAPT. Windows page lists are mapped to Linux meminfo fields, with zram and zswap as the compressed segment.
- Phase: P1
- Privilege: none

### PERF-038 Memory stats
- Source: TS In use, Committed, Cached, Paged pool, Non-paged pool, Slots used, Form factor, Hardware reserved, Speed | MC Available, Swap used, Swap available, Compressed, Savings, Type
- Behaviour: Rows, in order:
  - In use, Available, Committed ("46.2 / 67.2 GiB"), Cached;
  - "Kernel slab (reclaimable)" (replaces Paged pool), "Kernel slab (unreclaimable)" (replaces Non-paged pool);
  - Speed, Slots used ("2 of 4"), Form factor, Type, Hardware reserved;
  - Swap used, Swap available;
  - Compressed, Savings (shown only when at least 1 MiB is compressed);
  - Huge pages (shown only when non-zero).
- Linux data/mechanism:
  - meminfo `Cached`, `SReclaimable`, `SUnreclaim`, `Committed_AS`/`CommitLimit` and `HugePages_*`.
  - Swap available = SwapFree. This fixes Mission Center's bug, which shows SwapTotal.
  - Savings = (zram orig_data_size + Zswapped) − (zram compr_data_size + Zswap).
  - Hardware reserved = sum of DIMM sizes − MemTotal.
  - DIMM fields from PERF-041. Speed is the configured speed in MT/s.
- Verdict: ADAPT. Pools become slab.
- Phase: P1 for meminfo rows; P2 for DIMM rows.
- Privilege: none

### PERF-039 zram and zswap details
- Source: MC Compressed/Savings | TS "In use (compressed)"
- Behaviour: A "Compressed memory" block under the stats.
  - Per zram device: Device, Size (disksize), Stored (orig), Compressed, Memory used, Ratio "3.4 : 1", Algorithm.
  - zswap: "Enabled: No" here; when enabled, Pool size and Stored.
- Linux data/mechanism:
  - `/sys/block/zram*/{disksize,mm_stat,comp_algorithm}`. In `comp_algorithm` the active entry is the one in brackets.
  - `/sys/module/zswap/parameters/{enabled,compressor,max_pool_percent}` and meminfo `Zswap`/`Zswapped`.
- Verdict: ADAPT. The block is new and gives detail Mission Center only sums.
- Phase: P2
- Privilege: none

### PERF-040 Swap devices list
- Source: TS "Page file" concept | MC swap graph
- Behaviour: A table under the compressed block with the columns Name, Type (partition, file, zram), Size, Used and Priority.
- Linux data/mechanism: `/proc/swaps`.
- Verdict: ADAPT. Replaces Windows page files.
- Phase: P2
- Privilege: none

### PERF-041 Memory modules
- Source: TS "Memory module" heading (crosscheck A.16, B.5), Information modules | MC slots, speed, form factor, type
- Behaviour: Under "Memory module" there is one block per DIMM: Slot ("P0 CHANNEL A / DIMM 0"), Capacity, Speed (configured / rated MT/s), Type (DDR4 and TaskSlinger's type list), Form factor, Manufacturer, Part number, Rank, Voltage. The same data feeds the Information tab (PERF-093).
- Linux data/mechanism: Read the udev database for `/sys/devices/virtual/dmi/id` through libudev or by parsing `/run/udev/data/+dmi:id`. Keys: `MEMORY_ARRAY_NUM_DEVICES`, `MEMORY_DEVICE_<n>_{SIZE,LOCATOR,BANK_LOCATOR,TYPE,FORM_FACTOR,SPEED_MTS,CONFIGURED_SPEED_MTS,MANUFACTURER,PART_NUMBER,SERIAL_NUMBER,RANK,CONFIGURED_VOLTAGE}`. Read once at startup.
- Verdict: BUILD
- Phase: P2
- Privilege: none. Checked on this machine. This corrects PLAN section 3, which says DIMM details need root. If udev lacks the keys (systemd-udevd before v252), the fallback is "Not reported" and no dmidecode is run.

## Disk pages

### PERF-042 Disk list and kind
- Source: TS drive types | MC disk kinds and exclusions
- Behaviour: One page per physical disk. Kind is one of NVMe, SSD, HDD, eMMC, SD, USB, Optical or Virtual. Here that gives three pages: nvme0n1, nvme1n1 and sda.
- Linux data/mechanism:
  - `/sys/block/*`, excluding `loop`, `ram`, `zram`, `fd`, `dm-`, `md` and `zd`. Those appear as holders and partitions.
  - Kind rules, in order:
    - an `nvme` prefix is NVMe;
    - `mmcblk` plus `device/type` (MMC/SD) gives eMMC or SD;
    - a `usb` transport (udev `ID_BUS=usb`) gives USB;
    - `sr*` is Optical;
    - `virtio`/`xen` is Virtual;
    - otherwise `queue/rotational` decides HDD (1) or SSD (0).
  - Model from udev `ID_MODEL`, falling back to sysfs `device/model`.
- Verdict: ADAPT. Windows drive types become Linux kinds.
- Phase: P1
- Privilege: none

### PERF-043 Disk graphs
- Source: TS "Active time", "Disk transfer rate" | MC throughput with write dashed
- Behaviour: "Active time" (0 to 100%, filled) and "Transfer rate" (Read filled, Write dashed, dynamic scale).
- Linux data/mechanism: `/sys/block/<d>/stat` at 1 Hz.
  - Active = Δio_ticks / Δt_ms.
  - Read rate = Δsectors_read × 512 / Δt; write likewise.
  - Counters are u64, with reset detection.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-044 Disk stats
- Source: TS Average response time, Read speed, Write speed, Capacity, Formatted, System disk, Page file, Hardware name, Used space, Free space | MC Total read/written, Type, WWN, Serial, Rotation rate
- Behaviour: Rows: Active time, Average response time (ms, 2 decimals), Read speed, Write speed, Total read, Total written, Capacity, Formatted, Used space, Free space, System disk, "Swap" (replaces Page file: Yes/No), Hardware name, Type, Interface, WWN, Serial, Rotation rate, Temperature.
- Linux data/mechanism:
  - Response = Δ(read_ticks + write_ticks + discard_ticks + flush_ticks) / Δ(reads + writes + discards + flushes) from `stat` fields 4, 8, 15 and 17 (the read and write fields per the kernel layout).
  - Totals from the sectors counters since boot.
  - Capacity = `size` × 512.
  - Formatted, used and free = the sum of `statvfs` over mounted filesystems on this disk.
  - System disk = holds `/` or `/boot`.
  - Swap = any `/proc/swaps` entry on this disk or its holders.
  - Interface from the udev `ID_BUS` or transport (nvme, ata, usb, scsi, virtio).
  - WWN and Serial from udev `ID_WWN` and `ID_SERIAL_SHORT`, or UDisks2 `Drive.WWN`/`Serial`.
  - Rotation from UDisks2 `Drive.RotationRate`.
- Verdict: ADAPT. Page file becomes Swap.
- Phase: P1 for rates, active time, capacity and response; P2 for the rest.
- Privilege: none. Serials come from udev and are readable without root.

### PERF-045 Partitions table
- Source: TS "Partitions" table ("%zu partitions - %s"; Partition, Size, Offset, Drive, Mount path, Volume, File system, Flags, Type) | MC partition list with usage bar
- Behaviour: The heading reads "3 partitions - GPT". Columns:
  - Partition (`nvme0n1p2`);
  - Size;
  - Offset;
  - Device: a dm/LUKS holder such as `luks-…` → `dm-0`, replacing Drive;
  - Mount path: the first mount, with the rest in a tooltip ("Additional mount points:");
  - Label: replaces Volume;
  - File system;
  - Usage: Mission Center's bar plus %;
  - Flags;
  - Type.
  - Unallocated gaps over 1 MiB show as rows with Type "Unallocated".
  - Rows are ordered by offset. TaskSlinger orders them this way; Mission Center sorts by size.
- Linux data/mechanism:
  - `/sys/block/<d>/<p>/{start,size,partition,ro}` and holders in `/sys/block/<d>/<p>/holders/`.
  - udev `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`.
  - Mounts from `/proc/self/mountinfo`. Usage from `statvfs`.
  - Type names from a GPT GUID table: EFI system, Linux filesystem, Linux root (x86-64), Linux home, Linux swap, Linux LVM, Linux RAID, LUKS, Microsoft basic data, Microsoft reserved, Windows recovery, BIOS boot. Anything else shows "GPT partition {guid}". MBR types show "MBR 0x%02X".
  - Flags from GPT attributes: bit 0 "Required", bit 2 "Legacy BIOS bootable" (mapped to "Boot"), bit 60 "Read-only", bit 62 "Hidden", bit 63 "No automount". The sysfs `ro` value also gives "Read-only".
- Verdict: ADAPT. Drive letters become device and mount path.
- Phase: P2
- Privilege: none

### PERF-046 SMART data dialog
- Source: MC SMART dialog
- Behaviour: A "Show SMART data" button sits in the disk page header when SMART is available. The dialog has these fields: Last updated ("2 min ago"), Powered on, Drive health (the overall assessment; tasklens fills the value Mission Center leaves empty), Self-test status. Then a sortable table:
  - ATA columns: ID, Attribute, Value, Normalized, Threshold, Worst, Type (Pre-fail or Old-age), Updates (Online or Offline), Assessment (FAILING, Failed in the past, OK).
  - NVMe rows: Percent used, Available spare, Spare threshold, Total data read, Total data written, Warning temp time, Critical temp time, Controller busy time, Warning temperature, Critical temperature, Temperature sensors, Unsafe shutdowns, Media errors, Error log entries, Power cycles. Mission Center's duplicate busy-time row is dropped.
  - A "Refresh" button.
  - On failure the dialog reads "Failed to get SMART data" and names the reason.
- Linux data/mechanism:
  - ATA: UDisks2 `org.freedesktop.UDisks2.Drive.Ata.SmartGetAttributes({})`, `SmartUpdated`, `SmartPowerOnSeconds`, `SmartFailing`, `SmartSelftestStatus`. "Refresh" calls `SmartUpdate({})`.
  - NVMe: `org.freedesktop.UDisks2.NVMe.Controller.SmartGetAttributes({})` and `SmartUpdate`. This needs udisks2 2.10 or newer with the nvme module.
- Verdict: BUILD
- Phase: P2
- Privilege: Reading cached attributes is allowed for the active session. `SmartUpdate` uses the polkit action `org.freedesktop.udisks2.ata-smart-update` (allowed for the active local session by default). Fallback when UDisks2 or NVMe support is missing: a "Read with smartctl (admin)" button runs `pkexec /usr/bin/smartctl -a -j /dev/<d>` and parses the JSON.

### PERF-047 Eject with blocking processes
- Source: MC eject button and failure dialog
- Behaviour: An "Eject" button appears only for ejectable or removable drives, and its visibility updates live. A failed unmount opens "Disk is in use".
  - The dialog lists blocking processes grouped by app: name, PID, open paths, and an "End Task" button per row.
  - Buttons: "Close", "Retry", "End All".
  - End and End All send SIGTERM; holding Shift sends SIGKILL ("Force end").
  - A retry follows automatically after ending.
  - Success shows the toast "Disk ejected".
- Linux data/mechanism:
  - UDisks2 `Filesystem.Unmount({})` for each mounted filesystem, `Encrypted.Lock` for LUKS holders, then `Drive.Eject({})`, or `Drive.PowerOff` for USB.
  - Blockers: scan `/proc/*/{cwd,root,exe}`, `/proc/*/fd/*` and `/proc/*/maps` for paths under the mount points.
  - Signals use pidfd with identity check (process actions owner).
- Verdict: BUILD
- Phase: P2
- Privilege: Unmount and eject use the udisks2 polkit actions (`filesystem-unmount-others`, `eject-media`; allowed for the active session on removable media). The blocker scan sees only the user's own processes. The dialog adds the line "Processes of other users may also hold this disk" when any `/proc/*/fd` read failed with EACCES.

### PERF-048 Disk temperature
- Source: MC sidebar disk temperature
- Behaviour: Temperature on the disk card line ("12% · 41 °C") and as a stats row.
- Linux data/mechanism: The drive's hwmon (`nvme` hwmon "Composite", temp1, present here; `drivetemp` for SATA when that module is loaded). Fallback: UDisks2 `Drive.Ata.SmartTemperature` (Kelvin). 1 Hz.
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Network pages

### PERF-049 Interface list and type
- Source: TS interface types and states (L28137-28152) | MC NetworkManager kinds
- Behaviour:
  - One page per interface, except loopback.
  - Type: Ethernet, Wi-Fi, Virtual, USB network, Bridge, Cellular, Tunnel, VPN or Other.
  - State uses TaskSlinger's labels, which map one-to-one to the kernel operstate: Connected (up), Disconnected (down), Testing, Dormant, Not present, Lower layer down. "unknown" plus carrier shows Connected.
  - Here `tailscale0` is a Tunnel, so it shows (TaskSlinger shows Tailscale too). `docker0` is Virtual and hidden by default.
- Linux data/mechanism:
  - `/sys/class/net/<if>/`:
    - a `wireless/` or `phy80211` entry is Wi-Fi;
    - `bridge/` is Bridge;
    - `tun_flags` or `type`=65534 is Tunnel;
    - `device/driver` under a usb parent is USB network;
    - `wwan*` or `type`=519 is Cellular;
    - a name matching `docker*|veth*|br-*|virbr*|vnet*` is Virtual;
    - `type`=1 with a `device` link is Ethernet.
  - NetworkManager `Device.DeviceType`, read as `u` (u32), refines VPN and WireGuard when NM is present.
  - State from `operstate` and `carrier`, with rtnetlink `RTM_NEWLINK` events.
- Verdict: ADAPT. Classification uses sysfs first, so it works without NetworkManager. NetworkManager only adds detail.
- Phase: P1
- Privilege: none

### PERF-050 Throughput graph
- Source: TS "Throughput" (Download/Upload) | MC send dashed, receive filled, linked scale
- Behaviour: "Throughput" graph: Download filled, Upload dashed, one shared scale (PERF-015), "Throughput over 60 seconds".
- Linux data/mechanism: `/sys/class/net/<if>/statistics/{rx_bytes,tx_bytes}` at 1 Hz, kept open and read with `pread`. `/proc/net/dev` serves as a bulk fallback.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-051 Network stats
- Source: TS Adapter name, Connection type, IPv4, IPv6, MAC, Public IP, Receive/Transmit link speed, Interface index, Display unit (crosscheck A.15) | MC Receive, Send, Total received, Total sent, Connection status
- Behaviour: Rows:
  - Download, Upload, Total received, Total sent;
  - Adapter name, Interface name, Connection type, Connection status;
  - Link speed. The kernel reports one speed for Ethernet, so receive and transmit are the same. Wi-Fi shows its RX and TX bitrates.
  - IPv4 address, IPv6 address, MAC address, Interface index, Driver;
  - "Display unit" (the active unit, for example "Megabits per second");
  - Public IP (PERF-054).
- Linux data/mechanism:
  - Adapter name from the systemd hwdb (`ID_MODEL_FROM_DATABASE`/`ID_VENDOR_FROM_DATABASE` in the udev properties of the net device). No bundled SQLite database like Mission Center's.
  - `speed` (Mb/s; -1 shows "Not reported"), `address`, `ifindex`, `device/driver` basename.
- Verdict: BUILD
- Phase: P1 for rates and totals; P2 for the rest.
- Privilege: none

### PERF-052 Wi-Fi details
- Source: MC SSID, Signal strength, Maximum bitrate, Frequency
- Behaviour: Wi-Fi rows: SSID, Signal (4-level icon plus "-52 dBm (78%)"), RX/TX bitrate, Frequency ("5.18 GHz", channel), Security.
- Linux data/mechanism:
  - NetworkManager `Device.Wireless.ActiveAccessPoint`, then `AccessPoint.{Ssid,Strength,Frequency,MaxBitrate,RsnFlags}`.
  - Without NetworkManager: nl80211 generic netlink `NL80211_CMD_GET_INTERFACE` (SSID, frequency) and `NL80211_CMD_GET_STATION` (signal, rx/tx bitrate).
  - 1 Hz while the page is shown.
- Verdict: ADAPT. nl80211 is added as a fallback.
- Phase: P2
- Privilege: none

### PERF-053 IP addresses
- Source: TS IPv4/IPv6 | MC NM AddressData
- Behaviour: Shows every address with its prefix ("192.168.1.10/24"). IPv6 shows global addresses first and link-local addresses in the tooltip.
- Linux data/mechanism: rtnetlink `RTM_GETADDR` dump plus `RTM_NEWADDR`/`RTM_DELADDR` events. This avoids depending on NetworkManager.
- Verdict: ADAPT. rtnetlink replaces NetworkManager IP config.
- Phase: P2
- Privilege: none

### PERF-054 Public IP fetch
- Source: TS Public IP "Fetch" and "Not fetched", the Confirm Public IP Fetch dialog, `AutoFetchPublicIP` (in Window & startup per crosscheck B.7)
- Behaviour:
  - Shows "Not fetched" with a "Fetch" button.
  - The first click opens the confirm dialog with TaskSlinger's text, "Service: https://api.ipify.org", a "Do not ask again for public IP fetches." checkbox, and "Continue"/"Cancel".
  - Result: "203.0.113.4 (fetched 14:02)".
  - The setting "Auto-fetch public IP on launch" defaults to off.
  - The same value is shown on the Information tab.
- Linux data/mechanism:
  - HTTPS GET to `https://api.ipify.org` (IPv4). `https://api6.ipify.org` is also asked when a global IPv6 route exists.
  - Uses `ureq` 3 (sync, rustls, no Tokio) in the blocking pool with a 5 s timeout.
  - The result is never saved to disk.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-055 Network settings link
- Source: TS "Show more information" (L28329) | MC "Network Settings" (GNOME launch-panel)
- Behaviour: A "Network settings" link under the stats and in the context menu.
- Linux data/mechanism:
  - KDE (this machine): run `kcmshell6 kcm_networkmanagement`.
  - GNOME: session D-Bus `org.gnome.Settings` `/org/gnome/Settings` `org.freedesktop.Application.ActivateAction("launch-panel", [<"wifi"|"network">])`.
  - Hidden on other desktops.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

## GPU pages

### PERF-056 GPU enumeration and tl-gpu-probe
- Source: TS GPU N cards | MC nvtop | review section 3 "GPU child contract"
- Behaviour: One page per GPU, keyed by UUID (NVIDIA) or PCI address. A hung or crashed probe shows "Unavailable (GPU monitor restarting)" on every GPU metric, never zero.
- Linux data/mechanism:
  - **Discovery:** DRM devices come from `/sys/class/drm/card*/device` (vendor `0x10de`, `0x1002`, `0x8086`), plus NVIDIA devices from NVML.
  - **NVIDIA (only in `tl-gpu-probe`):**
    - Setup: `nvmlInit_v2`, `nvmlDeviceGetCount_v2`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetUUID`, `nvmlDeviceGetName`, `nvmlDeviceGetPciInfo_v3` (matched to the DRM card by bus id; here `00000000:2D:00.0` matches card1), `nvmlDeviceGetMinorNumber`.
    - Probe sends one batched sample reply per request at 1 Hz. The minimum period is 500 ms, even when Refresh is 0.25 s.
    - It answers over a socketpair with the review's protocol and 500 ms deadline.
  - **AMD and Intel:** read in-process from sysfs, hwmon and fdinfo (PERF-066, PERF-067). No child is needed, because no native library is loaded.
- Verdict: ADAPT. nvtop is replaced by direct per-vendor sources.
- Phase: P1
- Privilege: none

### PERF-057 Engine graphs
- Source: TS engine graphs (Utilization/3D, Compute, Video Encode, Video Decode, Video Processing, "Engine %u"), "Select GPU Engine Graphs", "Hide This Engine Graph", `HiddenGpuEngines`, crosscheck B.4
- Behaviour:
  - Row 1 is a grid of engine graphs in the CPU-grid style. Row 2 holds "Memory" (PERF-058) and "Temperature" graphs (TaskSlinger 0.9.2 layout). Row 3 is optional: "Power" and "Clocks" (Mission Center's middle and bottom choices).
  - The context menu has "Select GPU engine graphs" (a checklist of engines and of the Power and Clocks graphs) and "Hide this engine graph".
  - **NVIDIA engines:** "Graphics/Compute", "Memory controller", "Video encode", "Video decode", "JPEG", "Optical flow".
  - **AMD engines:** "Graphics", "Compute", "Memory controller", "Video encode", "Video decode", "JPEG", "DMA".
  - **Intel engines:** "Render/3D", "Compute", "Copy", "Video", "Video enhance".
  - Engines the device does not report are not listed.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetUtilizationRates` (`.gpu` gives Graphics/Compute, `.memory` gives Memory controller), `nvmlDeviceGetEncoderUtilization`, `nvmlDeviceGetDecoderUtilization`, `nvmlDeviceGetJpgUtilization`, `nvmlDeviceGetOfaUtilization`. A `NOT_SUPPORTED` result hides that engine.
  - AMD and Intel: see PERF-066 and PERF-067.
- Verdict: ADAPT. NVML has no 3D/Compute split and no "Video Processing" engine, so tasklens shows a merged Graphics/Compute engine plus JPEG and optical flow.
- Phase: P1 for Graphics, Encode and Decode; P2 for the rest and for hiding.
- Privilege: none

### PERF-058 GPU memory
- Source: TS GPU memory (dedicated), (shared), Hardware reserved memory | MC memory graph with dedicated solid and shared dashed
- Behaviour: The "Memory" graph shows dedicated (filled) against its total, and shared (dashed) against its total where the device has shared memory. Rows: "Dedicated memory 2.8 / 15.9 GiB", "Shared memory", "Hardware reserved memory 380 MiB".
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetMemoryInfo_v2` (`used`, `total`, `reserved`). NVIDIA has no shared (GTT) figure, so that row shows "Not supported".
  - AMD: `device/mem_info_vram_{used,total}` and `mem_info_gtt_{used,total}`.
  - Intel dGPU (xe/i915): the sum of `drm-resident-local0`/`drm-total-local0` over readable fdinfo, plus `device/tile0/vram0` sizes. Intel iGPU: "Shared" from the summed `drm-resident-system0`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-059 Video encode and decode
- Source: TS Video Encode/Decode | MC Encode/Decode graph and rows
- Behaviour: Rows "Video encode" and "Video decode" in %. When the hardware shares one engine, a single "Video encode/decode" row is shown (Mission Center). Encode and decode are also engine graphs (PERF-057).
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetEncoderUtilization(&util, &periodUs)` and `nvmlDeviceGetDecoderUtilization`. The active session count comes from `nvmlDeviceGetEncoderSessions` (tooltip).
  - AMD: `gpu_metrics` `vcn_activity[]`, or else fdinfo `drm-engine-enc`/`dec`.
  - Intel: the video engines.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-060 GPU clocks
- Source: MC Clock speed, Memory speed, Clock Speeds graph
- Behaviour: Rows "Clock speed 1905 / 3105 MHz", "Memory speed 7001 / 14001 MHz", "Video clock". "Clocks" graph: GPU clock solid, memory clock dashed, scaled to the maximum. The tooltip shows the P-state ("P3") and the throttle reasons.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetClockInfo(NVML_CLOCK_GRAPHICS | NVML_CLOCK_MEM | NVML_CLOCK_VIDEO)`, `nvmlDeviceGetMaxClockInfo`, `nvmlDeviceGetPerformanceState`, `nvmlDeviceGetCurrentClocksEventReasons`.
  - AMD: hwmon `freq1_input` (sclk) and `freq2_input` (mclk). The maximum is the last line of `pp_dpm_sclk`/`pp_dpm_mclk`.
  - Intel: `gt_act_freq_mhz`/`gt_max_freq_mhz` (i915) or `device/tile0/gt0/freq0/{act_freq,max_freq}` (xe).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-061 GPU power
- Source: MC Power draw current / limit, Power graph
- Behaviour: Row "Power draw 31.9 / 300 W". The tooltip gives the limit range "max 350 W" and energy since boot. "Power" graph scaled to the limit.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetPowerUsage` (mW), `nvmlDeviceGetEnforcedPowerLimit`, `nvmlDeviceGetPowerManagementLimitConstraints`, `nvmlDeviceGetTotalEnergyConsumption` (mJ).
  - AMD: hwmon `power1_input`, falling back to `power1_average` (µW), and `power1_cap`.
  - Intel dGPU: hwmon `energy1_input` delta and `power1_max`. Intel iGPU: RAPL `intel-rapl:0:1` (uncore), labelled "Package uncore (estimate)".
- Verdict: BUILD
- Phase: P2
- Privilege: none, except the Intel iGPU RAPL source (needs PERF-101).

### PERF-062 GPU temperature and fan
- Source: TS Temperature, Information "Temperature Source" (crosscheck A.17) | MC Temperature
- Behaviour: Temperature in °C on the card, the stats grid and the "Temperature" graph. A "Temperature source" row reads "NVML (GPU core)" or "hwmon amdgpu edge". Rows for slowdown and shutdown thresholds. "Fan speed 0 % (0 RPM)" belongs to the GPU page and does not appear on the Fans pages.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetTemperatureV` with `NVML_TEMPERATURE_GPU`, falling back to `nvmlDeviceGetTemperature`. Also `nvmlDeviceGetTemperatureThreshold(SLOWDOWN, SHUTDOWN)`, `nvmlDeviceGetNumFans`, `nvmlDeviceGetFanSpeed_v2`, and `nvmlDeviceGetFanSpeedRPM` where available. Memory temperature uses `nvmlDeviceGetFieldValues(NVML_FI_DEV_MEMORY_TEMP)` and shows "Not supported" on this card.
  - AMD: hwmon `temp1` edge, `temp2` junction, `temp3` mem, `fan1_input`, `pwm1`.
  - Intel dGPU: hwmon `temp1`. The iGPU has no sensor of its own and shows "Not supported".
- Verdict: BUILD
- Phase: P1 for temperature; P2 for fan and thresholds.
- Privilege: none

### PERF-063 PCIe link
- Source: MC PCI Express speed, Max PCI Express speed
- Behaviour: Rows "PCI Express: Gen 4 x16", "Max PCI Express: Gen 5 x16" (hidden when equal), "PCIe throughput RX/TX".
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetCurrPcieLinkGeneration`, `nvmlDeviceGetCurrPcieLinkWidth`, `nvmlDeviceGetGpuMaxPcieLinkGeneration`, `nvmlDeviceGetMaxPcieLinkWidth`, `nvmlDeviceGetPcieThroughput(TX_BYTES | RX_BYTES)`.
  - All vendors, and the fallback: `device/{current_link_speed,current_link_width,max_link_speed,max_link_width}`, mapping GT/s to Gen (2.5→1, 5→2, 8→3, 16→4, 32→5, 64→6).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-064 GPU static fields
- Source: TS Driver version, Driver date, DirectX version, Physical location | MC OpenGL, Vulkan, PCI bus address
- Behaviour: Rows: Driver version (615.71.09), "Kernel module" (replaces Driver date: "nvidia 615.71.09"), CUDA version, Vulkan version, OpenGL version (replaces DirectX version), Physical location ("PCI bus 45, device 0, function 0"), VBIOS version, UUID, Architecture.
- Linux data/mechanism:
  - NVIDIA: `nvmlSystemGetDriverVersion`, `nvmlSystemGetCudaDriverVersion_v2`, `nvmlDeviceGetVbiosVersion`, `nvmlDeviceGetArchitecture`.
  - Kernel module from `/sys/module/<driver>/version`.
  - Vulkan and OpenGL: a separate short-lived run of `tl-gpu-probe --api-versions`:
    - Vulkan: ash calls `vkEnumeratePhysicalDevices` and matches the GPU through `VK_EXT_pci_bus_info`. It reads `apiVersion` and `VkPhysicalDeviceDriverProperties`.
    - OpenGL: EGL `EGL_EXT_platform_device` on the matching DRM device, then a context, then `glGetString(GL_VERSION)`.
    - It runs once at startup. A crash only gives "Unknown".
- Verdict: ADAPT. DirectX becomes Vulkan and OpenGL, and the driver date becomes the module version.
- Phase: P2
- Privilege: none

### PERF-065 Displays list
- Source: TS Displays (connection type, "%s @ %d Hz", "%d-bit, %s", orientation; connectors per crosscheck A.17)
- Behaviour: Under the GPU stats, one row per display on that GPU: "DELL U2723QE: DisplayPort, 3840×2160 @ 144 Hz, max 10-bit, Landscape, scale 110%". Disconnected connectors are not listed.
- Linux data/mechanism:
  - DRM KMS read-only ioctls on `/dev/dri/cardN` (no DRM master needed): `drmModeGetResources`, `drmModeGetConnector` (type: DisplayPort, HDMI-A, eDP shown as "Internal", DVI, VGA, ...), `drmModeGetEncoder`/`drmModeGetCrtc` (current mode and refresh), the connector property "max bpc", and the EDID blob for the monitor name.
  - On KDE: orientation and scale from `kscreen-doctor --json`, run on demand and re-run on wl_output changes. Elsewhere orientation shows "Not reported".
  - The sysfs `enabled` file is not used (wrong on this NVIDIA driver).
- Verdict: ADAPT. Linux has no actual-bit-depth API, so the row shows "max N-bit" from the property.
- Phase: P2
- Privilege: none. `/dev/dri/card1` has a seat ACL for the logged-in user. If open fails, the rows fall back to EDID from `/sys/class/drm/card*-*/edid` plus `modes`.

### PERF-066 AMD GPU backend
- Source: MC nvtop amdgpu backend, PLAN section 3 "AMD and Intel GPUs"
- Behaviour: Same pages as NVIDIA. Not tested on this machine.
- Linux data/mechanism:
  - `device/gpu_busy_percent` (Graphics), `device/mem_busy_percent` (Memory controller).
  - `device/gpu_metrics`, a binary struct parsed by its header version: `average_gfx_activity`, `average_umc_activity`, `vcn_activity`, `temperature_*`.
  - Compute, DMA and JPEG engines come from the sum of `/proc/<pid>/fdinfo/*` `drm-engine-{compute,dma,jpeg}` ns deltas over all readable DRM clients, de-duplicated by `drm-client-id`. Coverage is marked "own processes only" when any fdinfo read fails.
  - Memory, clocks, power and temperature as in PERF-058 to PERF-062.
- Verdict: ADAPT
- Phase: P2
- Privilege: none. Engine graphs from fdinfo cover only processes tasklens can read, and say so.

### PERF-067 Intel GPU backend
- Source: PLAN section 3 | MC nvtop i915/xe
- Behaviour: Same pages as NVIDIA. Not tested on this machine.
- Linux data/mechanism:
  - Engines, preferred source: the i915 PMU through `perf_event_open`. The type comes from `/sys/bus/event_source/devices/i915/type`, with events `rcs0-busy`, `bcs0-busy`, `vcs0-busy`, `vecs0-busy`, `ccs0-busy`.
  - Fallback: summed fdinfo `drm-engine-{render,copy,video,video-enhance,compute}` (i915), or `drm-cycles-*`/`drm-total-cycles-*` (xe).
  - Clocks, power and memory as in PERF-058, PERF-060 and PERF-061.
- Verdict: ADAPT
- Phase: P3
- Privilege: The PMU needs `CAP_PERFMON` or `kernel.perf_event_paranoid` ≤ 0. tasklens does not ask for either. The fdinfo fallback covers only processes tasklens can read.

## Fans and batteries

### PERF-068 Fan pages
- Source: MC 2.6 Fan
- Behaviour:
  - One page per fan: "Fan N" or its label, with the chip name ("nct6797").
  - Graphs: "Fan speed" (RPM, filled; PWM % dashed on a 0 to 100 axis) and "Temperature" (paired sensor, PERF-069).
  - Rows: Fan speed, PWM %, Control mode (Manual, Automatic, Full speed, from `pwmN_enable`), Temperature, Chip.
  - Fans that read 0 RPM for the whole session and have no label count as "not connected". They are hidden by default; the setting "Show disconnected fans" shows them.
  - On this machine nct6797 has fans 1 to 7.
- Linux data/mechanism: `/sys/class/hwmon/hwmon*/{name, fanN_input, fanN_label, fanN_max, pwmN, pwmN_enable}` at 1 Hz while shown or in the sidebar. GPU fans are excluded (they are on the GPU page).
- Verdict: BUILD. Display only, no fan control (same as Mission Center).
- Phase: P2
- Privilege: none. Missing Super I/O drivers mean no fans; PERF-103 covers that.

### PERF-069 Fan labels and temperature pairing
- Source: MC per-file override directory | MC same-index temperature pairing
- Behaviour:
  - The context menu has "Rename fan…" and "Temperature sensor >", which lists every hwmon temp with its label and current value. The choice persists per fan key.
  - Default pairing: the same-index `tempN` on the same chip, but only when the chip has exactly one fan. Otherwise no sensor is chosen, and the Temperature graph reads "Choose a sensor".
- Linux data/mechanism: Settings TOML `fans.<key>.label` and `fans.<key>.temp = "<hwmon name>:<path>:tempN"`.
- Verdict: ADAPT. Mission Center's config-file overrides become in-app settings. Mission Center's index pairing is often wrong on Super I/O chips, so tasklens does not pair by default there.
- Phase: P3
- Privilege: none

### PERF-070 Battery and power device pages
- Source: MC 2.7 Battery (all UPower kinds)
- Behaviour:
  - One page per UPower device except `DisplayDevice` and line power. Here that is the Logitech mouse via HID++.
  - Title: the kind ("Mouse", "Battery", "UPS"), then vendor and model.
  - Graph: "Power input"/"Power output" (W, charge positive, discharge negative) for system batteries; "Percentage" for peripherals.
  - Rows (Mission Center's list): Percentage, Energy, Voltage, Power, State, Time to full/empty, Charge cycles, Serial, Type, Powering system, Technology, Capacity, Energy empty, Energy full, Energy full (design), Voltage min/max (design), Charge threshold, Charge start/end threshold, Temperature.
  - Charge threshold shows "Enabled" or "Disabled" correctly (Mission Center always shows "Yes").
- Linux data/mechanism:
  - UPower `org.freedesktop.UPower.EnumerateDevices`, `DeviceAdded`/`DeviceRemoved`, `PropertiesChanged`.
  - Properties: `Percentage`, `Energy`, `EnergyRate`, `Voltage`, `State`, `TimeToFull`, `TimeToEmpty`, `ChargeCycles`, `Serial`, `Type`, `PowerSupply`, `Technology`, `Capacity`, `EnergyEmpty`, `EnergyFull`, `EnergyFullDesign`, `VoltageMinDesign`, `VoltageMaxDesign`, `ChargeThresholdSupported`, `ChargeThresholdEnabled`, `ChargeStartThreshold`, `ChargeEndThreshold`, `Temperature`, `Vendor`, `Model`.
  - Fallback: `/sys/class/power_supply/*` (`capacity`, `status`, `energy_now`, `power_now`, `cycle_count`), rescanned every 30 s.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-071 Battery charge history
- Source: MC "Battery Charge over past week" (`GetHistory("charge", 604800)`)
- Behaviour: A 7-day charge graph with day gridlines and dashed lines across gaps. Shown for system batteries with at least 2 history points.
- Linux data/mechanism: UPower `Device.GetHistory("charge", 604800, 500)` and `GetHistory("rate", …)`. Refreshed every 10 minutes and when `UpdateTime` changes.
- Verdict: BUILD
- Phase: P3
- Privilege: none

## System minigraph panel

### PERF-072 Minigraph panel and cards
- Source: TS 4.2 (cards VRAM, GPU, Network, Disk, Memory, CPU), View > Graphs toggles, labels "GPU temp", "Total"/"Kernel" (crosscheck A.28)
- Behaviour:
  - Floating cards in a grid over the bottom right of the Processes tab, each with a drop shadow.
  - Each card shows the label at top left (for example "CPU" or "GPU temp"), the value at top right, and a graph.
  - System cards: CPU (Total plus a Kernel line), Memory, Disk, Network, GPU, VRAM, GPU temp (off by default).
  - View > Graphs has a checkbox per card (`ShowCpuGraph`, `ShowRamGraph`, `ShowDiskGraph`, `ShowNetworkGraph`, GPU, VRAM, `ShowGpuTemperatureGraph`).
  - Card size is 180×90 logical px. Cards scale with UI scale.
- Linux data/mechanism: Reuses the system series. Disk = aggregate active time; Network = the sum over visible physical interfaces; GPU = the highest engine on GPU 0, or a GPU chosen in the card menu.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-073 Card context menu
- Source: TS 4.2 menu (Hide Graph, Time Range, Graph Color, Add tray icon graph)
- Behaviour: Right-click shows "Hide graph", "Time range >" (30 seconds, 1 minute, 5 minutes, 10 minutes), "Graph color >" (Accent plus 17), "Detach" or "Attach to main window", "Add tray icon graph", and "Reset minigraph layout". Per-card range and colour persist.
- Linux data/mechanism: Settings `minigraphs.cards[]` with `{source, range, color, slot, detached}`.
- Verdict: BUILD
- Phase: P1 for hide and range; P3 for colour, detach and tray.
- Privilege: none

### PERF-074 Move and reorder cards
- Source: TS "Drag to move", "Reset Minigraph Layout", toasts "No space for more minigraphs", "Minigraph layout reset"
- Behaviour: Dragging a card moves it to another grid slot, and other cards shift. The grid fills from the bottom right, at most 3 rows by 4 columns. Adding a card to a full grid shows the toast "No space for more minigraphs". "Reset minigraph layout" restores the defaults and shows "Minigraph layout reset".
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-075 Detached graph windows
- Source: TS `TaskSlingerDetachedGraph`, always on top, detached menu (Show Main Window, Close Graph, Exit, Add Graph, Remove Graph)
- Behaviour:
  - "Detach", or dragging a card past the window edge, opens it as its own small borderless window. Dragging the window body moves it.
  - Its menu: "Show main window", "Close graph", "Attach to main window", "Add graph >" (adds another card to the same window), "Remove graph", "Exit tasklens".
  - Windows are always on top where the desktop allows it. Size and stacked cards persist. Position is restored only on KDE.
- Linux data/mechanism:
  - GPUI `open_window` with `WindowKind::Floating`, client decorations and no title bar. Moving uses `start_window_move()`.
  - On KDE, keep-above and position come from a KWin script loaded through `org.kde.KWin /Scripting loadScript`, matched by app_id `tasklens-graph-<n>`. It sets `keepAbove` and `frameGeometry` from saved values and reports moves back over D-Bus.
  - Elsewhere the compositor places the window and keep-above is not offered.
- Verdict: ADAPT. Wayland clients cannot set position or keep-above, so a KWin script does it on KDE.
- Phase: P3
- Privilege: none

### PERF-076 Keep detached graphs open
- Source: TS "Keep detached graphs open when closing main window" (`KeepDetachedOpenOnMainClose`, default 0)
- Behaviour: When on, closing the main window hides it, and the app keeps running while any detached graph is open. "Show main window" brings it back. When off, closing the main window closes everything.
- Linux data/mechanism: App lifecycle checks the open windows.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-077 Process-source cards
- Source: TS process-specific minigraphs (`ProcessSpecificMiniGraphs`), labels "Process %lu", " by app", " by name", "No match", "Ended" (crosscheck A.28)
- Behaviour:
  - Cards whose source is one process (captured identity), a process-name filter or an app.
  - Titles: "firefox (Process 1234)", "firefox by name", "Firefox by app".
  - A name or app card with no running match shows "No match". A one-process card whose process exited shows "Ended", and its graph freezes.
  - Created from the Processes context menu, which the process-table spec owns. This spec owns the card.
- Linux data/mechanism: The engine adds demand for these identities, so they are sampled even when off screen.
- Verdict: BUILD
- Phase: P3
- Privilege: none, for the user's own processes. Other users' processes show "Restricted" for fields they hide.

## Tray graphs

### PERF-078 Tray icon graph
- Source: TS 4.3 (sources: page, column, process, name filter, app filter), CL 0.8.6, 0.8.27
- Behaviour:
  - Each tray graph is its own tray icon, drawing a live mini graph in the graph colour on a transparent background, with the current value as a filled bar height.
  - Added from a Performance card menu ("Add tray icon graph"), a minigraph card, or the process menu.
  - Toasts: "Tray icon graph added", "removed" and "already exists", and "Unable to add tray icon graph".
  - Choices persist.
- Linux data/mechanism:
  - A StatusNotifierItem on the session bus, one per graph: name `org.kde.StatusNotifierItem-<pid>-<n>`, registered with `org.kde.StatusNotifierWatcher.RegisterStatusNotifierItem`.
  - `IconPixmap` holds ARGB32 big-endian images at 22, 32 and 44 px, re-rendered on the CPU from the 1 Hz series.
  - The `NewIcon` signal fires at 1 Hz, and only when the pixels changed.
  - The menu is served over `com.canonical.dbusmenu`.
  - Implementation: `ksni` with default features off, if `cargo tree -e normal` shows no Tokio. Otherwise a hand-written SNI and dbusmenu server on the engine's zbus connection.
- Verdict: ADAPT. Windows `Shell_NotifyIcon` becomes SNI.
- Phase: P3
- Privilege: none. If no `StatusNotifierWatcher` is on the bus, the menu items are disabled with the tooltip "No system tray available".

### PERF-079 Tray tooltip
- Source: TS tooltip strings ("CPU Usage: ", "%d procs: %s", "CPU:", "RAM:", "GPU:", "Disk:", "Network:")
- Behaviour: A graph icon's tooltip shows its title and value ("CPU Usage: 18.6%"). The main tray icon's tooltip shows five lines: CPU, RAM, GPU, Disk, Network. A process-filter icon shows "3 procs: firefox".
- Linux data/mechanism: SNI `ToolTip` property `(icon, title, description)` and the `NewToolTip` signal at 1 Hz.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-080 Tray menu and graph colour
- Source: TS tray menu ("Tray icon graphs", "Close tray icon graph", Show Main Window, Exit), "Graph color" item (crosscheck A.29)
- Behaviour: The menu has "Show main window", "Graph color >", "Time range >", "Close tray icon graph", "Tray icon graphs >" (a checklist to add or remove the others) and "Exit tasklens". Left-click shows or hides the main window.
- Linux data/mechanism: dbusmenu layout. `Activate(x, y)` toggles the window.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-081 Main tray icon while graphs show
- Source: TS CL 0.9.2 "main tray icon is hidden while a tray graph is shown"
- Behaviour: When at least one tray graph exists, the plain app icon is not registered. The first graph icon's menu carries the main entries. With no graphs, the plain icon shows when a tray setting needs it. Tray settings belong to the chrome and settings spec.
- Linux data/mechanism: Register or unregister the SNI.
- Verdict: BUILD
- Phase: P3
- Privilege: none

## Performance mini window

### PERF-082 Performance mini window
- Source: TS 4.4 (CL 0.8.4: compact, movable, always-on-top), crosscheck B.1 (the view-mode button is a process view cycler, so the opening path is unknown)
- Behaviour:
  - A compact, borderless window listing the visible sidebar cards (PERF-001) in one column.
  - Open it with View > Window > "Performance mini window", or by double-clicking the Performance graph area (Windows Task Manager's summary-view gesture).
  - Double-clicking the mini window, or "Show main window" in its menu, returns to the main window.
  - Its menu: "Always on top" (KDE only), "Show main window", "Close".
  - Clicking a card opens the main window on that page.
  - Width is 260 logical px; height fits the cards.
- Linux data/mechanism: GPUI `Floating` window. Keep-above and position use the KWin script from PERF-075.
- Verdict: ADAPT. The opening path is decided here because TaskSlinger's is unknown. Always on top works on KDE only.
- Phase: P3
- Privilege: none
- Open question: TaskSlinger's real trigger is unknown. Recommendation: use the View menu item plus double-click, as above.

## Information tab

### PERF-083 Information layout, copy and filter
- Source: TS 2.7 (two columns, section header with accent bar and copy button, tab-level copy icon, filter applies) | MC System Information "Copy All"
- Behaviour:
  - Two scrolling columns of sections. Each header has a 3 px accent bar at left and a copy button at right.
  - The copy icon on the tab copies all sections. Toast: "Copied to clipboard".
  - Format: a section name line, then `Label: value` lines, with a blank line between sections.
  - The tab's filter box hides rows whose label or value does not match, and hides empty sections.
  - Values are selectable.
  - Data loads once at tab open. Live values (uptime, available memory, counts) refresh at 1 Hz while the tab is visible.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-084 Operating System section
- Source: TS OS Name, Version, Build, Architecture, Install Date, Registered Owner, Product ID | MC OS name, version, package manager and its version, kernel release and version, distro logo
- Behaviour: Rows: OS name ("CachyOS Linux", with the distro logo), Version, Build, Kernel, Kernel version, Architecture ("x86_64 (64-bit)"), Install date, Package manager ("pacman 7.0.0"), Machine ID.
  - Version is `VERSION_ID`, or "Rolling" when that is missing.
  - Build is `BUILD_ID`, or "Not reported".
  - Machine ID replaces Product ID.
  - Registered Owner is dropped. Linux has no registered-owner record, and the user name is already under System.
- Linux data/mechanism:
  - `/etc/os-release` (`PRETTY_NAME`, `VERSION_ID`, `BUILD_ID`, `LOGO`, which is looked up in the icon theme).
  - `uname(2)` (release, version, machine).
  - Install date: `statx` btime of `/`, falling back to the first timestamp in `/var/log/pacman.log`.
  - Package manager version: `pacman --version`, `dpkg-query --version` or `rpm --version`, run once.
  - `/etc/machine-id`.
- Verdict: ADAPT. Product ID becomes Machine ID. Registered Owner is dropped, with the reason above.
- Phase: P2
- Privilege: none

### PERF-085 Processor section
- Source: TS Name, Vendor, Architecture, Identifier, Logical Processors, Speed
- Behaviour: Rows: Name, Vendor ("AuthenticAMD"), Architecture, Identifier ("Family 25 Model 97 Stepping 2"), Cores, Logical processors, Base speed, Max speed, Microcode, Flags summary (virtualization, AVX-512 yes/no).
- Linux data/mechanism: `/proc/cpuinfo` (`vendor_id`, `cpu family`, `model`, `stepping`, `model name`, `microcode`, `flags`), topology files, `cpuinfo_max_freq`, and base speed from PERF-030.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-086 System section
- Source: TS Computer Name, User Name, User Domain, System Uptime, Boot Time, Process Count, Thread Count, Handle Count | MC Desktop group
- Behaviour: Rows:
  - Computer name: the pretty hostname plus the static hostname.
  - User name.
  - Domain: replaces User Domain, shows "None" instead of "WORKGROUP".
  - System uptime ("2 days, 2h 38m 28s") and Boot time.
  - Process count, Thread count, Open files: real values, fixing TaskSlinger's zero bug.
  - Desktop ("KDE Plasma 6.x"), Windowing system ("Wayland"), Session type, Virtual terminal.
- Linux data/mechanism:
  - systemd-hostnamed `org.freedesktop.hostname1` (`PrettyHostname`, `StaticHostname`).
  - `getpwuid(getuid())`; `getdomainname(2)`, or the DNS domain from the FQDN.
  - Boot time = now − `CLOCK_BOOTTIME`.
  - Counts as in PERF-030.
  - Desktop from `XDG_CURRENT_DESKTOP` plus `plasmashell --version` (once), `XDG_SESSION_TYPE`, `XDG_VTNR`, and logind `Session` properties.
- Verdict: ADAPT. The Windows domain becomes the DNS or NIS domain. Mission Center's Desktop group is merged in here.
- Phase: P2
- Privilege: none

### PERF-087 Memory section
- Source: TS Total Physical, Available Physical, Memory Usage, Committed, Commit Limit, Peak Committed, Page Size
- Behaviour: Rows: Total physical, Available physical, Memory usage (%), Committed, Commit limit, "Peak committed (since tasklens started)", Page size ("4096 bytes"), Huge page size, Swap total, zram size.
- Linux data/mechanism: `/proc/meminfo`, `sysconf(_SC_PAGESIZE)`, and meminfo `Hugepagesize`. The kernel keeps no peak for Committed_AS, so tasklens tracks the maximum it has seen while running.
- Verdict: ADAPT. The peak covers the tasklens run, and the label says so.
- Phase: P2
- Privilege: none

### PERF-088 Memory modules section
- Source: TS per-module block (Module, Slot, Capacity, Speed, Type, Form Factor, Manufacturer, Part Number)
- Behaviour: One block per DIMM, as in `information.png`: "Module 1", Slot ("P0 CHANNEL A (DIMM 0)"), Capacity, Speed ("3200 MT/s"), Type, Form factor, Manufacturer, Part number, Serial number.
- Linux data/mechanism: Same udev keys as PERF-041.
- Verdict: BUILD
- Phase: P2
- Privilege: none (verified on this machine).

### PERF-089 Disk drives section
- Source: TS Disk Drives (Space "%s free of %s", Volume, type, interface)
- Behaviour: One block per physical disk: Model, Type, Interface (NVMe, SATA, USB, SCSI, virtio), Capacity, Space ("812 GB free of 1.82 TB"), Volumes (mount paths), Serial.
- Linux data/mechanism: Same as PERF-042 to PERF-045.
- Verdict: ADAPT. Volume letters become mount paths.
- Phase: P2
- Privilege: none

### PERF-090 BIOS/Firmware section
- Source: TS Firmware Type, BIOS Vendor, BIOS Version, BIOS Date
- Behaviour: Rows: Firmware type ("UEFI" or "Legacy BIOS"), BIOS vendor, BIOS version, BIOS date, BIOS release, Secure Boot ("Enabled", "Disabled" or "Not reported").
- Linux data/mechanism:
  - `/sys/firmware/efi` presence gives the firmware type.
  - `/sys/class/dmi/id/{bios_vendor,bios_version,bios_date,bios_release}`, all world-readable.
  - Secure Boot from byte 4 of `/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-091 Motherboard section
- Source: TS Manufacturer, Product, Version
- Behaviour: Rows: Manufacturer, Product, Version, System vendor, System product, Chassis type, Serial number.
- Linux data/mechanism: `/sys/class/dmi/id/{board_vendor,board_name,board_version,sys_vendor,product_name,chassis_type}`. Serial from `board_serial`.
- Verdict: BUILD
- Phase: P2
- Privilege: `board_serial` is mode 0400 (checked). Unprivileged it shows "Unavailable", with the tooltip "Needs root". There is no pkexec prompt for this.

### PERF-092 Graphics section
- Source: TS Name, Driver Version, Driver Date, DirectX Version, Temperature, Temperature Source, Dedicated Memory, Shared Memory, Resolution
- Behaviour: One block per GPU: Name, Driver version, Kernel module (replaces Driver Date), Vulkan version and OpenGL version (replace DirectX Version), Temperature, Temperature source, Dedicated memory, Shared memory, Resolution (each display "3840×2160 @ 144 Hz").
- Linux data/mechanism: Same as PERF-056 to PERF-065.
- Verdict: ADAPT. Same mapping as PERF-064.
- Phase: P2
- Privilege: none

### PERF-093 Network adapters section
- Source: TS Connection, Details, IP Address, MAC Address, Public IP with fetch
- Behaviour: One block per visible interface: Connection (name and type), Details (adapter name, driver, link speed), IP address, MAC address. A final "Public IP" row has the Fetch button from PERF-054.
- Linux data/mechanism: Same as PERF-049 to PERF-054.
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Additional setup (replaces Mission Center's first-run dialog)

### PERF-094 Additional data panel
- Source: MC first-run dialog ("Enabling Additional Values", Run Additional Setup, Revert, View Script, Copy Script Location, Copy Command, Copy Revert Command; success and failure dialogs)
- Behaviour:
  - No dialog on first launch. Settings > Monitoring > "Additional data…" opens a panel. Each item has a status ("Enabled", "Not enabled", "Not needed") and an "Enable"/"Revert" button.
    - "CPU power draw" (PERF-095).
    - "Per-process network". This item explains that tasklens uses TCP attribution with no setup. Exact numbers need the later network helper (PERF-096).
    - "Fan sensors" (PERF-097).
  - Each item has "View script" and "Copy command".
  - Every "Unavailable: needs setup" tooltip in the app links to this panel.
  - Success shows "Done. No restart needed". tasklens re-probes the source at once.
  - Failure shows the output with "Copy output" and "Retry".
- Linux data/mechanism: A fixed script shipped at `/usr/lib/tasklens/tasklens-setup` (root-owned, in the package) with the subcommands `rapl-enable`, `rapl-revert` and `sensors-detect`. It runs through `pkexec /usr/lib/tasklens/tasklens-setup <cmd>` under the polkit action `io.tasklens.setup` (`auth_admin_keep`). No script is written to `/tmp`, unlike Mission Center.
- Verdict: ADAPT. The first-run dialog becomes an on-demand panel, and the temporary script becomes a packaged one.
- Phase: P3
- Privilege: pkexec with polkit `io.tasklens.setup`. If cancelled, the item stays "Not enabled".

### PERF-095 RAPL udev rule
- Source: MC setup script step 2 (`/etc/udev/rules.d/99-powercap.rules`)
- Behaviour:
  - Status is "Enabled" when `energy_uj` can be read. That is the case on this machine through Mission Center's existing rule, which tasklens detects and leaves alone.
  - Enabling shows a warning first: "Lets every local user read CPU energy counters. This can leak information about other programs (CVE-2020-8694)."
- Linux data/mechanism:
  - `rapl-enable` writes `/etc/udev/rules.d/60-tasklens-powercap.rules` containing `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`.
  - It then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add`.
  - `rapl-revert` removes the file and re-triggers. The kernel then resets the mode to 0400 on the next trigger or reboot. The revert step says that a reboot may be needed.
- Verdict: ADAPT
- Phase: P3
- Privilege: root through pkexec. Fallback: "Unavailable" for power.

### PERF-096 nethogs capabilities
- Source: MC setup script step 1 (`setcap cap_net_admin,cap_net_raw,cap_dac_read_search,cap_sys_ptrace+pe` on nethogs)
- Behaviour: Not offered. The panel row explains the sock_diag TCP attribution and points to the later network helper.
- Linux data/mechanism: Per-process network uses sock_diag. The process and connections specs own that. A later eBPF collector will get its own narrow privilege (review risk 8).
- Verdict: DROP. tasklens does not run nethogs, so there is nothing to give capabilities to. Scraping nethogs output from a pseudo-terminal is also fragile, and putting capabilities on a general tool is too broad. The feature itself (per-process network) is kept through other means.
- Phase: P3 (the panel text only)
- Privilege: none

### PERF-097 sensors-detect
- Source: MC setup script step 3 (`sensors-detect --auto`)
- Behaviour:
  - Shows "Not needed" when any hwmon fan exists. That is the case here: nct6797 is loaded.
  - Otherwise "Run sensor detection" runs it and shows the output. The modules it finds are written to `/etc/modules-load.d/tasklens-sensors.conf` and loaded with `modprobe`.
  - Revert removes that file.
  - When lm_sensors is not installed, the row reads "Install lm_sensors to detect fan sensors" and has no button.
- Linux data/mechanism: `tasklens-setup sensors-detect` runs `sensors-detect --auto`, parses its "MODULE_n" output, then writes the file and runs `modprobe`.
- Verdict: ADAPT. Mission Center's result is not saved across reboots unless sensors-detect writes a config; tasklens always writes the module file.
- Phase: P4
- Privilege: root through pkexec. Fallback: no fan pages.

## Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PERF-001 | Device sidebar cards | BUILD | P1 |
| PERF-002 | Hide/show page context menu | BUILD | P1 |
| PERF-003 | Sidebar edit mode | BUILD | P2 |
| PERF-004 | Per-type default visibility and network groups | ADAPT | P2 |
| PERF-005 | Device hotplug and stable keys | BUILD | P1/P2 |
| PERF-006 | Sidebar width splitter | BUILD | P1 |
| PERF-007 | Jump keys and View submenu | BUILD | P2 |
| PERF-008 | Aggregate pages | BUILD | P4 |
| PERF-009 | Graph summary mode | ADAPT | P2 |
| PERF-010 | Copy device info | BUILD | P2 |
| PERF-011 | Page layout and fallback strings | BUILD | P1 |
| PERF-012 | Graph element | BUILD | P1 |
| PERF-013 | Time range | ADAPT | P1/P2 |
| PERF-014 | History tiers | BUILD | P1/P2 |
| PERF-015 | Scaling modes | ADAPT | P1 |
| PERF-016 | Axis maximum rounding and label | BUILD | P1 |
| PERF-017 | Smooth (sliding) graphs | ADAPT | P1 |
| PERF-018 | Curved lines | ADAPT | P4 |
| PERF-019 | Gaps and stale data | BUILD | P1 |
| PERF-020 | Hover readout | BUILD | P2 |
| PERF-021 | Graph colours | BUILD | P1/P3 |
| PERF-022 | Units | ADAPT | P1/P2 |
| PERF-023 | Legends and secondary series | BUILD | P1 |
| PERF-024 | CPU header and graph mode | ADAPT | P1/P2 |
| PERF-025 | Per-core grid | BUILD | P1 |
| PERF-026 | Kernel time overlay | BUILD | P1 |
| PERF-027 | Overlaid and stacked modes | BUILD | P2 |
| PERF-028 | Core type marks | ADAPT | P2 |
| PERF-029 | CPU bottom graph | BUILD | P2 |
| PERF-030 | CPU live stats | ADAPT | P1 |
| PERF-031 | CPU static stats | BUILD | P1/P2 |
| PERF-032 | Virtualization and security rows | ADAPT | P2 |
| PERF-033 | CPU power (RAPL) | BUILD | P2 |
| PERF-034 | CPU temperature | BUILD | P1 |
| PERF-035 | CPU clock | BUILD | P1 |
| PERF-036 | Memory graphs | BUILD | P1 |
| PERF-037 | Memory composition bar | ADAPT | P1 |
| PERF-038 | Memory stats | ADAPT | P1/P2 |
| PERF-039 | zram and zswap details | ADAPT | P2 |
| PERF-040 | Swap devices list | ADAPT | P2 |
| PERF-041 | Memory modules | BUILD | P2 |
| PERF-042 | Disk list and kind | ADAPT | P1 |
| PERF-043 | Disk graphs | BUILD | P1 |
| PERF-044 | Disk stats | ADAPT | P1/P2 |
| PERF-045 | Partitions table | ADAPT | P2 |
| PERF-046 | SMART data dialog | BUILD | P2 |
| PERF-047 | Eject with blocking processes | BUILD | P2 |
| PERF-048 | Disk temperature | BUILD | P2 |
| PERF-049 | Interface list and type | ADAPT | P1 |
| PERF-050 | Throughput graph | BUILD | P1 |
| PERF-051 | Network stats | BUILD | P1/P2 |
| PERF-052 | Wi-Fi details | ADAPT | P2 |
| PERF-053 | IP addresses | ADAPT | P2 |
| PERF-054 | Public IP fetch | BUILD | P2 |
| PERF-055 | Network settings link | ADAPT | P3 |
| PERF-056 | GPU enumeration and tl-gpu-probe | ADAPT | P1 |
| PERF-057 | Engine graphs | ADAPT | P1/P2 |
| PERF-058 | GPU memory | BUILD | P1 |
| PERF-059 | Video encode and decode | BUILD | P1 |
| PERF-060 | GPU clocks | BUILD | P2 |
| PERF-061 | GPU power | BUILD | P2 |
| PERF-062 | GPU temperature and fan | BUILD | P1/P2 |
| PERF-063 | PCIe link | BUILD | P2 |
| PERF-064 | GPU static fields | ADAPT | P2 |
| PERF-065 | Displays list | ADAPT | P2 |
| PERF-066 | AMD GPU backend | ADAPT | P2 |
| PERF-067 | Intel GPU backend | ADAPT | P3 |
| PERF-068 | Fan pages | BUILD | P2 |
| PERF-069 | Fan labels and temperature pairing | ADAPT | P3 |
| PERF-070 | Battery and power device pages | BUILD | P2 |
| PERF-071 | Battery charge history | BUILD | P3 |
| PERF-072 | Minigraph panel and cards | BUILD | P1 |
| PERF-073 | Card context menu | BUILD | P1/P3 |
| PERF-074 | Move and reorder cards | BUILD | P2 |
| PERF-075 | Detached graph windows | ADAPT | P3 |
| PERF-076 | Keep detached graphs open | BUILD | P3 |
| PERF-077 | Process-source cards | BUILD | P3 |
| PERF-078 | Tray icon graph | ADAPT | P3 |
| PERF-079 | Tray tooltip | BUILD | P3 |
| PERF-080 | Tray menu and graph colour | BUILD | P3 |
| PERF-081 | Main tray icon while graphs show | BUILD | P3 |
| PERF-082 | Performance mini window | ADAPT | P3 |
| PERF-083 | Information layout, copy and filter | BUILD | P2 |
| PERF-084 | Operating System section | ADAPT | P2 |
| PERF-085 | Processor section | BUILD | P2 |
| PERF-086 | System section | ADAPT | P2 |
| PERF-087 | Memory section | ADAPT | P2 |
| PERF-088 | Memory modules section | BUILD | P2 |
| PERF-089 | Disk drives section | ADAPT | P2 |
| PERF-090 | BIOS/Firmware section | BUILD | P2 |
| PERF-091 | Motherboard section | BUILD | P2 |
| PERF-092 | Graphics section | ADAPT | P2 |
| PERF-093 | Network adapters section | BUILD | P2 |
| PERF-094 | Additional data panel | ADAPT | P3 |
| PERF-095 | RAPL udev rule | ADAPT | P3 |
| PERF-096 | nethogs capabilities | DROP | P3 |
| PERF-097 | sensors-detect | ADAPT | P4 |

## Conflicts with the architecture review

1. **Time ranges (PERF-013, PERF-014).** The review proposes 15 s and 60 s ranges in v1, with one 602-sample ring. This spec keeps TaskSlinger's 30 s, 1 min, 5 min and 10 min, and adds a 1 Hz tier ring of 602 samples per series. Extra memory is about 0.8 MB.
2. **GPU sample rate (PERF-056).** The review sets GPU totals at 1 Hz. This spec keeps 1 Hz as the default, but allows 500 ms when Refresh is below 1 s. It never goes faster than 500 ms, so the 500 ms probe deadline still holds.
3. **Window lifecycle (PERF-076, PERF-078, PERF-082).** The review says closing the last window exits the app in v1, with tray persistence later. Keep-detached-open, tray graphs and the mini window keep the app alive after the main window closes. All of these are P3. If v1 includes P3, the review's exit rule needs an exception for these windows.
4. **Always-on-top and KWin scripting (PERF-075, PERF-082).** The review defers always-on-top where unsupported, and KWin window picking. This spec uses a KWin script for keep-above and position on KDE in P3. It is not offered elsewhere.
5. **Splines (PERF-018).** The review says use straight segments. This spec adds monotone curves, off by default. They never overshoot, which keeps the review's correctness concern.
6. **Second use of `tl-gpu-probe` (PERF-064).** The review makes `tl-gpu-probe` the only process that loads NVML. This spec also uses a separate one-shot run of the same binary (`--api-versions`) to load Vulkan and EGL. It is a separate process from the NVML sampler. I read this as an extension, not a break.
7. **PLAN section 3 facts (not review rules) are wrong in two places on this machine:**
   - DIMM details, including serials, come from the udev database without root (PERF-041).
   - The DRM connector `enabled` file is wrong on NVIDIA 615, so display data must come from KMS ioctls (PERF-065).
8. **Hidden-mode policy.** The review says sources go off when the window is hidden and nothing is recording. Tray graphs, detached graphs, process-source cards and the mini window add demand, so their sources keep running while the main window is hidden. The review's rule for aggregating demand already allows this. I am noting it so the hidden-CPU budget (0.3% of one core) is measured with one tray graph active.
