# PERF: performance monitoring, graphs, minigraphs, tray graphs and Information tab

This spec has 78 entries. 76 are BUILD or ADAPT and 2 are DROP. Every entry has a phase. The machine checks below were made on this machine on 2026-10-02 and change some earlier assumptions:

- **DIMM details need no root.** `udevadm info -q property -p /sys/devices/virtual/dmi/id` returns `MEMORY_DEVICE_<n>_*` keys as a normal user: size, type, speed, form factor, locator, bank, maker, part number and serial. This corrects PLAN.md section 3, which says DIMM details need root. Only `/sys/class/dmi/id/{board,product,chassis}_serial` and `product_uuid` are root-only (mode 0400).
- **RAPL is readable here, but only because of a leftover Mission Center rule.** `/etc/udev/rules.d/99-powercap.rules` makes `energy_uj` world-readable. The kernel default is 0400.
- **The NVIDIA DRM connector `enabled` attribute is wrong.** Three monitors are lit, but `/sys/class/drm/card1-DP-{1,3}/enabled` reads `disabled`. tasklens must not use it.
- **NVML supports these metrics on driver 615.71.09:** utilization (gpu and memory), encoder, decoder, used/total/reserved memory (380 MiB reserved), GPU temperature, power draw, power limit and max limit, graphics/memory/video clocks and their maximums, PCIe generation and width, fan %, VBIOS, display active and P-state. Memory temperature is N/A. Fan reads 0 % because the card is in zero-RPM mode.
- **Other hardware found:** `k10temp` (Tctl plus CCD sensors), `nct6797` (7 fans, 7 PWM outputs, 13 temperatures), two NVMe hwmon nodes, `zram0` active, zswap off, a Logitech `hidpp_battery_0` through UPower, and UDisks2, NetworkManager and UPower all on the system bus. `nethogs` is not installed.

---

## A. Performance tab structure

### PERF-001 Performance tab layout
- Source: TS 2.3 (sidebar plus page, NavWidth=252) | MC 2.0 (device list, detail area, right info panel)
- Behaviour: Left: a sidebar of device cards, default width 252 logical px. A splitter drags the width between 180 and 400 px, saved as `performance.nav_width`. Right: the selected device page. The page has a header (device name on the left, mode toggles on the right), graphs, then a 3-column stats grid below the graphs, as in the TS screenshot. MC puts stats in a right-hand side panel; tasklens uses the TS layout instead. The last selected page is restored by device identity (MC saves this but restores the wrong device; tasklens fixes that). When the window is under 700 logical px wide, the sidebar collapses to graph-only cards that are 64 px wide.
- Linux data/mechanism: none (UI). The page subscribes to demand for its own device's sources only (architecture review, visibility semantics).
- Verdict: BUILD (TS layout; MC's restore bug fixed)
- Phase: P1
- Privilege: none

### PERF-002 Sidebar device cards
- Source: TS 2.3 sidebar | MC 2.0 "What each row shows"
- Behaviour: Each card has a mini graph (about 116×56 px, no grid, filled area plus top line) with the current value overlaid at its top left, then a title line and two detail lines. The selected card has the accent outline. Per device:
  - CPU: "CPU", model (ellipsized), "4631 MHz · 52 °C".
  - Memory: "Memory", "34.68 GiB / 63.16 GiB", "Commit 69%". The graph shows used as a fill and committed as a dashed line (MC).
  - GPU N: "GPU N", model, "52 °C".
  - Disk N: "Disk N", "<Kind> - <primary mount>" (for example "NVMe - /"), model. The value shown is active %. Temperature is appended when available.
  - Network: interface display name, type ("Ethernet", "Wi-Fi", "VPN", "Virtual"), and the rate in the accent colour ("0.01 Mbps"). A down interface shows its state ("Disconnected") and the card drops to 0.6 opacity (MC).
  - Fan N: label, "1240 RPM" or "PWM 45 %", paired temperature.
  - Battery N: model, "87 % · Charging".
- Linux data/mechanism: values come from the page sources (PERF-010 to PERF-055). Card graphs read the same history rings as the pages, so there is no second sampling.
- Verdict: BUILD (TS look; MC's extra temperature lines added)
- Phase: P1 (CPU, Memory, GPU, Disk, Network); P2 (Fan, Battery)
- Privilege: none

### PERF-003 Device discovery, hotplug and identity
- Source: MC 2.0 (devices added and removed at runtime) | architecture review section 5 (device identity)
- Behaviour: Cards and pages appear and disappear live when devices come and go. If the selected device disappears, selection moves to the next visible card. A device that comes back with the same identity regains its saved order, visibility and colour. MC places late devices second to last and ignores visibility settings; tasklens fixes both.
- Linux data/mechanism: Saved identity keys:
  - GPU: NVML UUID, else PCI slot (`gpu:<uuid>` or `gpu:pci:0000:2d:00.0`).
  - Disk: WWN, else serial, else `/dev/disk/by-id` name, else major:minor plus generation.
  - Network: interface name plus MAC.
  - Fan: hwmon chip name, device path and fan index.
  - Battery: UPower `NativePath`.

  Hotplug is detected by a udev netlink monitor (`NETLINK_KOBJECT_UEVENT`, subsystems `block`, `net`, `hwmon`, `drm`, `power_supply`). Disk events are debounced by 500 ms. NVML devices are enumerated once at probe start; a GPU hotplug restarts the probe.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-004 Page order, hiding and edit mode
- Source: TS 2.3 context menu ("Select Pages", "Hide This Page", "Show This Page"), [INI Columns.Performance, Performance.HiddenPages] | MC 2.0 sidebar edit mode
- Behaviour: Right-click a card for "Select Pages", "Hide This Page" and "Show This Page" (TS labels). "Select Pages" enters MC's edit mode. Each card then shows a drag handle and an on/off switch, hidden cards reappear with the switch off, and the sidebar header shows "Enable All", "Disable All", "Reset to Default" and "Done". Drag-and-drop reorders cards, with a 3 px accent drop line. Hiding the last visible page is refused with the toast "At least one performance page must stay visible" (TS). The default order is CPU, Memory, GPUs, Disks, Network, Fans, Batteries. That is TS order with MC's Fans and Batteries appended.
- Linux data/mechanism: TOML `performance.page_order = [identity keys]` and `performance.page_overrides = { "<id>" = "show" | "hide" }`. A per-device override wins over the type default (PERF-005).
- Verdict: BUILD (TS menu labels plus MC edit mode)
- Phase: P1 (hide and show menu); P2 (edit mode with drag)
- Privilege: none

### PERF-005 Device type visibility defaults
- Source: MC 5 Preferences > Devices, keys `performance-show-*`, network groups
- Behaviour: Settings > Performance > Devices has switches for Disks, GPUs, Fans and Batteries, and a Network expander with Wired, Wireless, VPN/tunnel, Virtual and Other. Defaults: all on except Network > Virtual. Virtual covers `docker*`, `veth*`, `br-*`, `virbr*` and other interfaces with no `device` link. Loopback is never shown. CPU and Memory always exist; they can be hidden only per page.
- Linux data/mechanism: Interface class from sysfs, with no NetworkManager dependency:
  - `wireless/` or `phy80211` → Wi-Fi.
  - `bridge/` → Virtual (bridge).
  - `tun_flags`, or `type` 65534 (e.g. `tailscale0`), or `wireguard` in `uevent` DEVTYPE → VPN/tunnel.
  - `device` link with a USB driver → USB network (Wired group).
  - DEVTYPE `wwan` → Other.
  - `type` 1 with a `device` link → Ethernet.
  - no `device` link → Virtual.
- Verdict: BUILD (MC; TS has no equivalent)
- Phase: P2
- Privilege: none

### PERF-006 Aggregate pages
- Source: TS 2.3 "Aggregate storage" L28224, "Aggregate adapters" L28228
- Behaviour: Optional pages "All disks" and "All GPUs". They exist only when there are 2 or more physical devices of that type, and are hidden by default (enable in edit mode). They graph summed throughput, maximum active time and summed VRAM, with a per-device legend.
- Linux data/mechanism: sums over physical disks only, never partitions or dm/md, to avoid double counting (architecture review, device topology). GPU sums come from per-device NVML or sysfs values.
- Verdict: BUILD
- Phase: P4
- Privilege: none

### PERF-007 Summary mode
- Source: MC 2.0 Summary mode (Ctrl+M, "Graph Summary View") | MC UX problem 1 (only one device visible at a time)
- Behaviour: Ctrl+M, or "Graph Summary View" in the page context menu, hides the title bar menus, tab bar, sidebar and stats. The window then shows a tile grid with the main graph of **every visible device**, each tile labelled and showing its value. Esc or Ctrl+M leaves it. Tab-switch shortcuts are blocked while it is on (MC). Why this changes MC: MC shows only the current page's graphs, and its inventory lists "one device visible at a time" as the main user complaint.
- Linux data/mechanism: none (UI). Demand covers all visible devices.
- Verdict: ADAPT (summary shows all devices tiled, not only the current page)
- Phase: P2
- Privilege: none

### PERF-008 Page context menu and View jump
- Source: MC 2.0 context menu (page items, Graph Summary View, View submenu, Copy) | TS 2.3 GPU engine menu
- Behaviour: Right-click on any page graph area shows:
  1. Page-specific items (listed per page).
  2. "Graph Summary View".
  3. "View >" CPU, Memory, GPU, Disk, Network, Fan, Battery. Each jumps to the first visible device of that type; types with no visible device are greyed out.
  4. "Copy" (PERF-009).
  5. "Add tray icon graph" (PERF-068).
  6. "Hide This Page".
- Linux data/mechanism: none
- Verdict: BUILD (merge)
- Phase: P1 (Copy, Hide); P2 (rest)
- Privilege: none

### PERF-009 Copy device info
- Source: MC 2.0 Copy (Ctrl+C, `data_summary()`) | TS toast "Copied metrics to clipboard"
- Behaviour: Ctrl+C on the Performance tab, or the menu item "Copy", puts plain text on the clipboard. The text is the device heading line, then the model, then every stats-grid field as "Label: value" lines indented by 4 spaces, in on-screen order. tasklens copies **all** visible fields, which fixes MC's per-page omissions (CPU power, disk serial and others). Toast: "Copied metrics to clipboard". On failure: "Failed to copy to clipboard". Stats values are also selectable text (MC).
- Linux data/mechanism: GPUI `cx.write_to_clipboard(ClipboardItem::new_string(..))` (Wayland `wl_data_device`).
- Verdict: BUILD
- Phase: P1
- Privilege: none

---

## B. CPU page

### PERF-010 CPU chart modes
- Source: TS 2.3 ("Overall" / "Per core" toggle, [INI Performance.CpuPerCore=1]) | MC 2.1 top graph (Overall, Logical Processors, All Threads, All Threads Stacked)
- Behaviour: The header button opens a 4-item dropdown: "Overall", "Per core" (default, as TS), "All cores overlaid" and "All cores stacked".
  - Overall: one large chart.
  - Per core: a grid of "CPU N" charts. Columns = ceil(√(2n)), so 16 CPUs give 6 columns (matches the TS screenshot) and 32 CPUs give 8 columns × 4 rows. Minimum cell width is 120 logical px; below that, columns are reduced. The legend ("Total", "Kernel") sits in the first cell.
  - Overlaid: every core in one chart at opacity max(1/n, 0.08).
  - Stacked: cores stacked, max = 100 × n.

  Each cell shows "CPU N" at the top left and the % at the top right. Hovering a cell shows a tooltip with the core's current clock and its SMT sibling.
- Linux data/mechanism: `/proc/stat` `cpuN` lines, read through a persistent fd with `pread`. Used = user+nice+system+irq+softirq+steal. Idle = idle+iowait. Guest columns are ignored (already inside user and nice). Rates come from actual elapsed monotonic time. CPU hotplug is detected from changes in the `cpuN` line set (architecture review). Sampling: 10 Hz while CPU charts are visible, 1 Hz otherwise (architecture review schedule).
- Verdict: BUILD (TS toggle extended with MC's 2 extra modes)
- Phase: P1 (Overall, Per core); P2 (overlaid, stacked)
- Privilege: none

### PERF-011 Kernel time overlay
- Source: TS 2.3 "Kernel on"/"Kernel off", [INI Performance.CpuKernel=1], red kernel line | MC 2.1 Show Kernel Times (dashed)
- Behaviour: A header toggle "Kernel on" / "Kernel off" (TS labels, default on). Kernel time draws as an unfilled line in kernel red (#C1514E) in Overall and Per core modes. The overlaid and stacked modes show no kernel series (MC). TS draws a solid line, so tasklens uses solid.
- Linux data/mechanism: kernel = system+irq+softirq from `/proc/stat`, per architecture review. iowait is not counted.
- Verdict: BUILD (TS)
- Phase: P1
- Privilege: none

### PERF-012 Core type markers
- Source: TS 2.3 "Performance core"/"Efficiency core" icons L28239-28240
- Behaviour: On hybrid CPUs, each per-core cell shows a small P or E icon. On AMD with preferred-core ranking, the tooltip shows "Preferred core rank N". Cores holding the highest rank get a small star. No icon appears on uniform CPUs.
- Linux data/mechanism: Intel hybrid: `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus`. AMD Zen 5/5c hybrids, or any CPU without those files: `/sys/devices/system/cpu/cpuN/cpu_capacity`, where values below the maximum mean E. Rank: `/sys/devices/system/cpu/cpufreq/policyN/amd_pstate_prefcore_ranking` (present on this machine).
- Verdict: ADAPT (adds the AMD preferred-core marker; hybrid detection uses sysfs)
- Phase: P2
- Privilege: none

### PERF-013 CPU bottom graph
- Source: MC 2.1 bottom graph (None, Power Draw, Clock Speed, Temperature)
- Behaviour: Below the chart grid sits a full-width strip of 25 % of the graph height. Its dropdown offers "None", "Power", "Clock speed" and "Temperature".
  - Default: Power if RAPL is readable, else Temperature, else Clock (MC fallback order).
  - Options with no data are greyed out, with the tooltip giving the reason ("Needs additional setup", PERF-075).
  - Label: "<Metric> over <range>", with the current maximum at the right.
  - Scaling (PERF-058): Power is sticky-up with integer rounding. Clock is sticky-up. Temperature is sticky up and down, starting at 35 to 45 °C.
- Linux data/mechanism:
  - Power: delta of `/sys/class/powercap/intel-rapl:0/energy_uj` (package-0; the AMD RAPL interface uses the same `intel-rapl` name, verified here) divided by elapsed time. Wrap is handled with `max_energy_range_uj`. Also read `intel-rapl:0:0` ("core") for the tooltip.
  - Clock: per-policy `cpufreq/policyN/cpuinfo_avg_freq` (APERF/MPERF, exists here), else `scaling_cur_freq`, else `/proc/cpuinfo` "cpu MHz". The value shown is the mean, and the tooltip shows the maximum.
  - Temperature: hwmon `k10temp` `temp1_input` (Tctl), else `coretemp` "Package id 0", else `zenpower`, else `cpu_thermal`. The tooltip lists Tccd sensors (`temp3`/`temp4` labels).
  - Rate: 1 Hz while displayed (architecture review).
- Verdict: BUILD (MC)
- Phase: P2
- Privilege: Power needs `energy_uj` readable (mode 0400 root by default since kernel 5.10). Fallback: "Power: unavailable (needs setup)" and the option greyed out. Setup in PERF-075.

### PERF-014 CPU stats grid
- Source: TS 2.3 stats L28241-28261 | MC 2.1 live and static panels
- Behaviour: 3 columns, label above value (TS style). Rows in order:
  1. Utilization, Kernel time, Speed.
  2. Processes, Threads, Handles.
  3. Up time, Base speed, Sockets.
  4. Cores, Logical processors, Virtualization.
  5. Virtual machine, CPU vulnerabilities, Hypervisor (these replace TS's VBS, Security services and Hypervisor platform).
  6. L1 cache, L2 cache, L3 cache.
  7. Power draw, Temperature, Cpufreq driver.
  8. Governor, Energy preference, Platform profile.
  9. Microcode.

  A row whose value is unsupported shows "Unsupported" in secondary text. MC hides such rows; tasklens keeps TS's fixed grid, so nothing jumps. Up time format is "2 days, 2h 38m 15s" (TS). Speed uses the BogoMIPS fallback with "(?)" and MC's explanatory tooltip.
- Linux data/mechanism:
  - Processes: count of numeric `/proc` entries. Threads: sum of `/proc/<pid>/stat` field 20 from the process sweep. Both come from the shared sweep, not a second scan.
  - Handles: `/proc/sys/fs/file-nr` field 1, labelled "Handles" with the tooltip "Open file handles, system-wide".
  - Up time: `clock_gettime(CLOCK_BOOTTIME)`.
  - Base speed: `cpufreq/base_frequency`, else `acpi_cppc/nominal_freq` (MHz), else `bios_limit`.
  - Sockets: distinct `topology/physical_package_id`. Cores: distinct (package, `core_id`). Logical processors: online count from `/sys/devices/system/cpu/online`.
  - Virtualization: `svm` or `vmx` flag in `/proc/cpuinfo` plus `/dev/kvm`. Values: "Enabled (KVM ready)", "Supported, KVM module not loaded", "Not supported".
  - Virtual machine: systemd `org.freedesktop.systemd1.Manager.Virtualization` property, where empty means "No".
  - CPU vulnerabilities: count of `/sys/devices/system/cpu/vulnerabilities/*` files starting "Mitigation" or "Not affected" versus "Vulnerable", shown as "14 mitigated, 0 vulnerable". The tooltip lists each file.
  - Hypervisor: `hypervisor` cpuinfo flag plus `/sys/hypervisor/type` if present, else "None".
  - Caches: `/sys/devices/system/cpu/cpu*/cache/index*/{level,type,size,shared_cpu_list}`, deduplicated by `shared_cpu_list` and summed per level (L1 = data + instruction).
  - Driver, governor, EPP: `cpufreq/policy0/{scaling_driver,scaling_governor,energy_performance_preference}` (here amd-pstate-epp, powersave, balance_performance).
  - Platform profile: `/sys/firmware/acpi/platform_profile`, else power-profiles-daemon `ActiveProfile` over D-Bus (`org.freedesktop.UPower.PowerProfiles`, present here).
  - Microcode: `/proc/cpuinfo` `microcode`.
- Verdict: ADAPT (Windows VBS, Security services and Hypervisor platform replaced by Linux analogues; MC's cpufreq rows added)
- Phase: P1 (rows 1-4 and 6); P2 (rows 5, 7-9)
- Privilege: Power draw as in PERF-013. Everything else needs none.

---

## C. Memory page

### PERF-015 Memory graphs
- Source: TS 2.3 graphs Physical memory, Commit, Swap L28573-28575 | MC 2.2 main graph (committed dashed, dirty, used filled) and Swap graph
- Behaviour: Three stacked graphs:
  - "Physical memory": used filled, max = total. Modified (Dirty+Writeback) is drawn as an unfilled line.
  - "Commit": Committed_AS filled, CommitLimit as a dashed line. Max = max(CommitLimit, peak Committed_AS in view), because commit can exceed 100 % (architecture review).
  - "Swap": used / total. Hidden, and its toggle disabled, when SwapTotal is 0.

  Context menu: "Show Memory Composition", "Show Commit Graph", "Show Swap Usage".
- Linux data/mechanism: `/proc/meminfo` through a persistent fd at 1 Hz. Used = MemTotal − MemAvailable (if MemAvailable > MemTotal, use MemFree; MC rule). ZFS ARC: if `/proc/spl/kstat/zfs/arcstats` exists, (size − c_min) is added to available (MC).
- Verdict: BUILD (TS graph set; MC dirty line)
- Phase: P1
- Privilege: none

### PERF-016 Memory composition bar
- Source: TS 2.3 "Memory composition" (In use, In use (compressed), Modified, Standby, Free) | MC 2.2 composition widget
- Behaviour: A horizontal bar with segments "In use", "In use (compressed)", "Modified", "Standby" and "Free". Each has a hover tooltip with its amount and a one-line definition. Default visible.
- Linux data/mechanism:
  - compressed = zram `mm_stat` field 3 (mem_used_total) summed over zram devices, plus meminfo `Zswap`.
  - in_use = (MemTotal − MemAvailable) − compressed, clamped at 0.
  - modified = Dirty + Writeback.
  - free = MemFree.
  - standby = MemTotal − in_use − compressed − modified − free, clamped at 0.

  The segments always sum to MemTotal.
- Verdict: ADAPT (TS segments with Linux definitions)
- Phase: P1
- Privilege: none

### PERF-017 Memory stats grid
- Source: TS 2.3 fields L28262-28270 | MC 2.2 stats panel
- Behaviour: Rows:
  1. In use, Available, Committed ("46.2 / 67.2 GiB").
  2. Cached, Slab reclaimable, Slab unreclaimable (these replace Paged pool and Non-paged pool).
  3. Swap used, Swap free, Compressed.
  4. Compression savings, Speed, Slots used ("2 of 4").
  5. Form factor, Type, Hardware reserved.

  Compressed and Savings show "None" when under 1 MiB (MC hides them). Swap free shows actual free; MC has a bug that shows total.
- Linux data/mechanism:
  - meminfo: Cached + Buffers for Cached, SReclaimable, SUnreclaim, SwapTotal, SwapFree, Committed_AS, CommitLimit.
  - Savings = (zram `mm_stat` orig_data_size + meminfo `Zswapped`) − compressed.
  - DIMM fields from the udev database (PERF-020): Speed is the first device's `CONFIGURED_SPEED_MTS`, Slots used is device count of `MEMORY_ARRAY_NUM_DEVICES`, Form factor and Type come from the first device.
  - Hardware reserved = Σ DIMM `SIZE` − MemTotal, shown only if DIMM sizes are known.
- Verdict: ADAPT (pools mapped to slab, as TS inventory section 9 suggests)
- Phase: P1 (meminfo rows); P2 (DIMM rows, reserved)
- Privilege: none (verified)

### PERF-018 zram and zswap section
- Source: MC 2.2 Compressed and Savings | TS inventory section 9 (zram/zswap compressed)
- Behaviour: A collapsible "Compression" block under the stats.
  - Per zram device: Device, Algorithm, Disk size, Original data, Compressed data, Memory used, Ratio ("3.38 : 1"), Used as swap (Yes/No, with priority).
  - zswap: Enabled (Y/N), Compressor, Max pool %, Pool size (`Zswap`), Stored (`Zswapped`).

  The block is hidden when neither zram nor zswap exists.
- Linux data/mechanism: `/sys/block/zram*/mm_stat` (fields orig, compr, mem_used, mem_limit, mem_used_max, same_pages, pages_compacted, huge_pages), `comp_algorithm` (active value in [brackets]), `disksize`. `/sys/module/zswap/parameters/{enabled,compressor,max_pool_percent}`. meminfo `Zswap` and `Zswapped`.
- Verdict: BUILD (Linux-only; MC data, more detail)
- Phase: P2
- Privilege: none

### PERF-019 Swap devices list
- Source: TS 2.3 Swap graph | TS section 9 (swap devices)
- Behaviour: A table under the Swap graph with columns Device, Type (partition, file, zram), Size, Used, Priority.
- Linux data/mechanism: `/proc/swaps`.
- Verdict: ADAPT (page file → swap devices)
- Phase: P2
- Privilege: none

### PERF-020 Memory modules (DIMMs)
- Source: TS 2.7 Memory per module | MC 2.2 Speed, Slots, Form factor, Type
- Behaviour: Shown on the Information tab (PERF-071) and as a tooltip on "Slots used".
- Linux data/mechanism: `udevadm` database properties of `/sys/devices/virtual/dmi/id`: `MEMORY_ARRAY_NUM_DEVICES`, `MEMORY_ARRAY_MAX_CAPACITY` and `MEMORY_DEVICE_<n>_{SIZE,FORM_FACTOR,LOCATOR,BANK_LOCATOR,TYPE,TYPE_DETAIL,SPEED_MTS,CONFIGURED_SPEED_MTS,MANUFACTURER,PART_NUMBER,SERIAL_NUMBER,RANK,CONFIGURED_VOLTAGE}`. Read with libudev (`udev` crate) or by parsing `/run/udev/data/+dmi:id`; no subprocess. Rows with SIZE absent or "No Module Installed" are empty slots. Fallback when the udev keys are missing (no `dmi_memory_id` helper): show "Module details unavailable" and offer nothing that needs root.
- Verdict: BUILD (MC mechanism; verified unprivileged here)
- Phase: P2
- Privilege: none (verified)

---

## D. Disk pages

### PERF-021 Disk list and kind
- Source: TS 2.3 Disk page, drive types L28078-28113 | MC 2.3, 7 Disks
- Behaviour: One page per physical disk: "Disk N (nvme0n1)" plus the model. Kind is one of NVMe, SSD, HDD, eMMC, SD card, USB drive, Optical, Virtual or iSCSI. Kind appears in the card ("NVMe - /") and in the stats.
- Linux data/mechanism: `/sys/block/*`, excluding `loop*`, `ram*`, `zram*` (shown in Memory), `fd*`, `dm-*`, `md*`, `zd*` and `nbd*` with size 0. Kind is decided in this order:
  1. name `nvme` → NVMe;
  2. `mmcblk` with `device/type` MMC → eMMC, SD → SD card;
  3. `sr` → Optical;
  4. udev `ID_BUS=usb` → USB drive;
  5. `virtio`/`xen` → Virtual;
  6. `device/transport` iSCSI;
  7. `queue/rotational` 1 → HDD, else SSD.

  Model: `device/model` (plus `device/vendor` for SCSI), else UDisks2 `Drive.Model`.
- Verdict: ADAPT (TS drive types replaced by MC's Linux kinds)
- Phase: P1
- Privilege: none

### PERF-022 Disk graphs
- Source: TS 2.3 Active time and Disk transfer rate | MC 2.3 Active time, Throughput (read solid, write dashed)
- Behaviour: "Active time" 0-100 % filled. "Transfer rate": read filled, write dashed, both on one shared scale, auto-scaled (PERF-058). The disk colour is #D9B567.
- Linux data/mechanism: `/sys/block/<dev>/stat` through a persistent fd at 1 Hz.
  - active % = Δio_ticks / Δt_ms × 100, clamped to 100.
  - read/write rate = Δsectors × 512 / Δt (the kernel always uses 512-byte units here).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-023 Disk stats grid
- Source: TS 2.3 fields L28272-28282 | MC 2.3 stats
- Behaviour: Rows:
  1. Active time, Average response time ("0.42 ms"), Read speed.
  2. Write speed, Total read, Total written.
  3. Capacity, Formatted, Used space.
  4. Free space, System disk, Swap on disk (replaces Page file).
  5. Type, Hardware name, Interface.
  6. Serial, WWN, Rotation rate.
  7. Temperature, Firmware.
- Linux data/mechanism:
  - Response time = Δ(read_ticks + write_ticks + discard_ticks + flush_ticks) / Δ(reads + writes + discards + flushes), using stat fields 4, 8, 15, 17 and 1, 5, 12, 16.
  - Totals: cumulative sectors × 512. Capacity: `size` × 512.
  - Formatted, Used, Free: `statvfs` summed over mounted filesystems on this disk (following holders, PERF-024).
  - System disk: holds the `/` or `/boot` mount.
  - Swap: any `/proc/swaps` entry on this disk.
  - Interface: udev `ID_BUS` / transport (NVMe, SATA, USB, SCSI, virtio).
  - Serial and WWN: UDisks2 `Drive.Serial` and `Drive.WWN`, else sysfs `device/serial` and `wwid`. Rotation: `Drive.RotationRate`.
  - Temperature: drive hwmon (`nvme` hwmon, present here; SATA needs the `drivetemp` module), else UDisks2 `Drive.Ata.SmartTemperature` (kelvin).
  - Firmware: `device/firmware_rev`.
- Verdict: ADAPT (page file → swap)
- Phase: P1 (rows 1-5); P2 (rows 6-7)
- Privilege: none. If `drivetemp` is not loaded for SATA, Temperature falls back to the UDisks2 SMART value; otherwise "Unavailable".

### PERF-024 Partitions table
- Source: TS 2.3 Partitions ("%zu partitions - %s"; columns Partition, Size, Offset, Drive, Mount path, Volume, File system, Flags, Type) | MC 2.3 partition rows with usage bar, mount points, LUKS/dm following
- Behaviour: The heading is "3 partitions - GPT". A sortable table with columns Partition (`nvme0n1p2`), Size, Offset, Device (dm name if encrypted, for example `luks-…` → `cryptroot`), Mount path (first mount; up to 3 shown, the rest in a tooltip, as MC), Label, File system, Usage (bar plus "%" and "used / size", from MC), Flags and Type. Unallocated gaps of 1 MiB or more appear as "Unallocated" rows. Default sort is offset ascending (TS). MC sorts by size; TS order is used.
- Linux data/mechanism:
  - Partitions: `/sys/block/<dev>/<part>/{partition,start,size,ro}`.
  - udev properties: `ID_PART_TABLE_TYPE` (gpt/dos), `ID_PART_ENTRY_TYPE` (GUID or MBR byte), `ID_PART_ENTRY_FLAGS`, `ID_FS_TYPE`, `ID_FS_LABEL`.
  - Holders: `/sys/block/<dev>/<part>/holders/dm-N` → `/sys/block/dm-N/dm/name`, followed recursively for LVM on LUKS.
  - Mounts: `/proc/self/mountinfo` (major:minor match). Usage: `statvfs`.
  - Type name table, GPT: C12A7328 EFI system, 0FC63DAF Linux filesystem, 0657FD6D Linux swap, E6D6D379 Linux LVM, 4F68BCE3 Linux root x86-64, 933AC7E1 Linux /home, CA7D7CCB LUKS, EBD0A0A2 Basic data, E3C9E316 Microsoft reserved, DE94BBA4 Windows recovery; others "GPT partition". MBR: 0x83 Linux, 0x82 swap, 0x8e LVM, 0x07 NTFS/exFAT, 0x0b/0x0c FAT32, 0xef EFI, 0x05/0x0f Extended; others "MBR 0x%02X".
  - Flags from GPT attributes (bit 0 Required, bit 62 Hidden, bit 63 No automount, bit 60 Read-only) and MBR boot flag 0x80 (Boot); "Read-only" also when sysfs `ro`=1.
  - Rescan on udev `block` change events.
- Verdict: ADAPT (TS columns with Linux types, flags and dm following; MC usage bar added)
- Phase: P2
- Privilege: none

### PERF-025 SMART data
- Source: MC 2.3 Show SMART Data dialog
- Behaviour: A header button "Show SMART Data" (only when the drive supports it) opens a dialog.
  - Header: Last updated ("2 min ago"), Powered on, Drive health ("Healthy", or "Failing" from the assessment; MC leaves this blank), Self-test status.
  - ATA table: ID, Attribute, Value, Normalized, Threshold, Worst, Type (Pre-fail/Old-age), Updates (Online/Offline), Assessment (Failing / Failed in the past / OK).
  - NVMe table: Attribute/Value rows with MC's list, without its duplicate "Control Busy Time" row.
  - Button "Refresh".
  - Failure dialog: "Failed to get SMART data", with the error text and a hint to run `smartctl -a /dev/<dev>`.
- Linux data/mechanism: UDisks2 `org.freedesktop.UDisks2.Drive.Ata`: `SmartGetAttributes(a{sv})`, properties `SmartUpdated`, `SmartPowerOnSeconds`, `SmartFailing`, `SmartSelftestStatus`; `SmartUpdate` on Refresh. NVMe: `org.freedesktop.UDisks2.NVMe.Controller.SmartGetAttributes` / `SmartUpdate` (UDisks2 2.10 or later). Calls go through zbus in the engine's D-Bus adapter.
- Verdict: BUILD (MC)
- Phase: P2
- Privilege: Reading cached attributes needs none. `SmartUpdate` uses polkit `org.freedesktop.udisks2.ata-smart-update` (allowed for the active local session by default). If UDisks2 is absent, the button is hidden. There is no smartctl fallback, because smartctl needs root.

### PERF-026 Eject with blockers
- Source: MC 2.3 Eject Disk and eject failure dialog
- Behaviour: A header button "Eject" appears only for ejectable or removable drives and updates live. It unmounts every filesystem on the drive, then powers off or ejects it. Success toast: "Disk ejected". On a busy failure, a dialog lists the blocking processes grouped by app: icon, name, PID and open paths (up to 5, then "+N more"). Each row has an "End Task" button. The dialog buttons are "Close", "Retry" and "End All". End Task and End All send SIGTERM through the process action path (pidfd, identity check), then retry the eject once after 1 s.
- Linux data/mechanism: UDisks2 `Filesystem.Unmount({})` per mounted filesystem (also through dm holders: `Encrypted.Lock` after unmount), then `Drive.Eject({})` if `Ejectable`, else `Drive.PowerOff({})` if `CanPowerOff`. Blockers come from scanning `/proc/*/fd`, `/proc/*/cwd`, `/proc/*/root` and `/proc/*/maps` for paths under the mount points, run in the blocking pool.
- Verdict: BUILD (MC; TS labels for actions)
- Phase: P2
- Privilege: Unmount and eject use UDisks2 polkit actions (`filesystem-unmount-others`, `eject-media`; allowed for the active session on removable media). Other users' processes cannot be scanned without root, so the dialog adds a line: "Processes of other users may also hold files open". Ending another user's process goes through the privileged helper path owned by the process-actions area.

---

## E. Network pages

### PERF-027 Network throughput graph
- Source: TS 2.3 Throughput (Upload/Download) | MC 2.4 graph (send dashed, receive filled, linked max, dynamic or link speed)
- Behaviour: One graph: "Receive" filled and "Send" dashed, sharing one scale. With the setting "Scale network graphs dynamically" on (default), it auto-scales (PERF-058). With it off, the graph is fixed at the link speed when known, else dynamic. The colour is #359CAB.
- Linux data/mechanism: `/sys/class/net/<if>/statistics/{rx_bytes,tx_bytes}` through persistent fds at 1 Hz. Read `/proc/net/dev` once per tick instead when more than 16 interfaces exist. Rates use actual elapsed time. Counter decrease (interface recreated) resets the baseline.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-028 Network stats grid
- Source: TS 2.3 Network fields L28283-28293, types L28137-28145, states L28147-28152 | MC 2.4 stats
- Behaviour: Rows:
  1. Receive, Send, Adapter name.
  2. Connection type, State, Interface index.
  3. Total received, Total sent, Link speed (rx and tx; one value on Linux, both TS rows merged).
  4. IPv4 address, IPv6 address, MAC address.
  5. Public IP (PERF-030), Driver, MTU.

  States use TS's list, which maps one-to-one to kernel operstate: up "Connected", down "Disconnected", testing "Testing", dormant "Dormant", notpresent "Not present", lowerlayerdown "Lower layer down", unknown "Unknown". When NetworkManager is present, the tooltip adds its state ("Connecting", "Waiting for auth" and so on).
- Linux data/mechanism:
  - `/sys/class/net/<if>/{operstate,ifindex,address,mtu,speed}`; speed -1 or missing → "Unknown".
  - Driver: `device/driver` link basename.
  - Adapter name: udev `ID_MODEL_FROM_DATABASE` / `ID_VENDOR_FROM_DATABASE` (hwdb, unprivileged); MC's own SQLite hwdb is not needed.
  - IPs: rtnetlink `RTM_GETADDR` dump (netlink-packet-route); all addresses listed in a tooltip, the primary global one shown. Link-local IPv6 only when no global address exists.
  - NetworkManager `Device.State` over D-Bus is optional enrichment.
- Verdict: ADAPT (Windows connection states map exactly to Linux operstate; NM optional)
- Phase: P1 (rows 1-4); P2 (row 5)
- Privilege: none

### PERF-029 Wi-Fi details
- Source: MC 2.4 Wi-Fi rows (SSID, Signal strength 4-level icon, Maximum bitrate, Frequency)
- Behaviour: Wi-Fi interfaces add the rows SSID, Signal strength (4-bar icon at ≤25/50/75/100 % plus "−54 dBm"), Bitrate and Frequency/Band ("5.18 GHz (ch 36)").
- Linux data/mechanism: Primary: NetworkManager `Device.Wireless.ActiveAccessPoint` → `AccessPoint.{Ssid,Strength,Frequency,MaxBitrate}`. Without NM: generic netlink nl80211 `NL80211_CMD_GET_INTERFACE` (SSID, frequency) and `NL80211_CMD_GET_STATION` (signal dBm, tx bitrate), both unprivileged.
- Verdict: BUILD (MC, plus an nl80211 fallback)
- Phase: P2
- Privilege: none

### PERF-030 Public IP fetch
- Source: TS 2.3 Public IP ("Fetch", "Not fetched"), Confirm Public IP Fetch dialog, [INI Connections.AutoFetchPublicIP, PublicIPFetchNoticeAccepted] | TS 2.7 Information
- Behaviour: The value shows "Not fetched" plus a "Fetch" link. The first fetch opens TS's dialog: "tasklens will fetch your public IP address from an online service.", "Service: https://api.ipify.org", "Continue only if you want this computer to contact that service.", the checkbox "Do not ask again for public IP fetches.", and Continue / Cancel. Results show "203.0.113.5 (fetched 14:02)". The setting "Auto-fetch public IP on launch" defaults to off. The value is shared by the Network page and the Information tab. The fetch is never repeated automatically after launch.
- Linux data/mechanism: HTTPS GET `https://api.ipify.org` (IPv4) and `https://api6.ipify.org` (IPv6, only if a global IPv6 default route exists). Uses the `ureq` 3 crate (blocking, rustls, no Tokio) in the engine blocking pool, with a 5 s timeout and errors shown inline ("Fetch failed: timed out").
- Verdict: BUILD (TS)
- Phase: P2
- Privilege: none

### PERF-031 Network settings link
- Source: TS 2.3 "Show more information" L28329 | MC 2.4 "Network Settings" context item (GNOME Settings over D-Bus)
- Behaviour: Context menu item and link "Network settings". On KDE it runs `systemsettings kcm_networkmanagement`. On GNOME it calls `org.gnome.Settings` `org.freedesktop.Application.ActivateAction("launch-panel", [("wifi"|"network",[])])` (MC). On other desktops the item is hidden.
- Linux data/mechanism: `XDG_CURRENT_DESKTOP` decides which path to use; the process is spawned detached.
- Verdict: ADAPT (KDE path added)
- Phase: P4
- Privilege: none

---

## F. GPU pages

### PERF-032 GPU backend split
- Source: architecture review section 3 (tl-gpu-probe) | MC 7 GPU (nvtop backends) | PLAN section 3
- Behaviour: Every GPU page and card is fed through one vendor-neutral `GpuSample` with per-metric validity. A metric the backend cannot supply shows "Unsupported" and is never shown as 0.
- Linux data/mechanism:
  - **NVIDIA**: NVML, loaded only in `tl-gpu-probe` (child process, socketpair protocol, 500 ms deadline, restart backoff). Rate 1 Hz while any GPU view or card is visible.
  - **AMD (amdgpu)**: sysfs and hwmon under `/sys/class/drm/cardN/device/`, read in-process.
  - **Intel (i915/xe)**: sysfs, hwmon, DRM fdinfo and i915 PMU, read in-process.

  The vendor comes from `device/vendor`: 0x10de NVIDIA, 0x1002 AMD, 0x8086 Intel. Other DRM drivers (v3d, msm, panfrost, panthor, nouveau) get utilization from fdinfo only. nouveau gets name and PCIe only.
- Verdict: ADAPT (replaces MC's bundled nvtop and TS's D3DKMT)
- Phase: P1 (NVIDIA probe, AMD sysfs); P2 (Intel, fdinfo engines)
- Privilege: none for NVML and amdgpu sysfs. Intel PMU needs privilege; see PERF-034.

### PERF-033 NVML call map (NVIDIA)
- Source: machine check (nvidia-smi field support on 615.71.09) | PLAN section 3 | brief
- Behaviour: Supplies every NVIDIA metric listed in PERF-034 to PERF-039.
- Linux data/mechanism: NVML C functions, called by `tl-gpu-probe` through nvml-wrapper:

  | Metric | NVML call | Poll |
  |---|---|---|
  | Init, count, handles | `nvmlInit_v2`, `nvmlDeviceGetCount_v2`, `nvmlDeviceGetHandleByIndex_v2` | once |
  | Identity | `nvmlDeviceGetUUID`, `nvmlDeviceGetName`, `nvmlDeviceGetPciInfo_v3` (busId), `nvmlDeviceGetVbiosVersion`, `nvmlDeviceGetBoardPartNumber` | once |
  | Driver | `nvmlSystemGetDriverVersion`, `nvmlSystemGetCudaDriverVersion_v2` | once |
  | Utilization (3D/compute) and memory controller % | `nvmlDeviceGetUtilizationRates` (.gpu, .memory) | 1 Hz |
  | Video Encode % | `nvmlDeviceGetEncoderUtilization` | 1 Hz |
  | Video Decode % | `nvmlDeviceGetDecoderUtilization` | 1 Hz |
  | JPEG and optical flow % | `nvmlDeviceGetJpgUtilization`, `nvmlDeviceGetOfaUtilization` (NOT_SUPPORTED → engine hidden) | 1 Hz |
  | VRAM used/total/reserved | `nvmlDeviceGetMemoryInfo_v2` (total, reserved, used, free) | 1 Hz |
  | BAR1 | `nvmlDeviceGetBAR1MemoryInfo` | on page open |
  | Temperature | `nvmlDeviceGetTemperatureV` (sensor GPU), fallback `nvmlDeviceGetTemperature(NVML_TEMPERATURE_GPU)`; slowdown and max from `nvmlDeviceGetTemperatureThreshold` | 1 Hz / once |
  | Power draw | `nvmlDeviceGetPowerUsage` (mW) | 1 Hz |
  | Power limit, max limit | `nvmlDeviceGetEnforcedPowerLimit`, `nvmlDeviceGetPowerManagementLimitConstraints` | 10 s |
  | Energy | `nvmlDeviceGetTotalEnergyConsumption` (mJ) | 1 Hz, optional, used for the average |
  | Clocks | `nvmlDeviceGetClockInfo` (GRAPHICS, MEM, VIDEO, SM) | 1 Hz |
  | Max clocks | `nvmlDeviceGetMaxClockInfo` (GRAPHICS, MEM) | once |
  | PCIe current/max | `nvmlDeviceGetCurrPcieLinkGeneration`, `nvmlDeviceGetCurrPcieLinkWidth`, `nvmlDeviceGetGpuMaxPcieLinkGeneration`, `nvmlDeviceGetMaxPcieLinkWidth` | 5 s |
  | PCIe throughput | `nvmlDeviceGetPcieThroughput` (TX_BYTES, RX_BYTES; KB/s) | 1 Hz while the page is open |
  | Fan | `nvmlDeviceGetNumFans`, `nvmlDeviceGetFanSpeed_v2` | 1 Hz |
  | P-state | `nvmlDeviceGetPerformanceState` | 1 Hz |
  | Throttle reasons | `nvmlDeviceGetCurrentClocksEventReasons` | 1 Hz while the page is open |
  | Display attached/active | `nvmlDeviceGetDisplayMode`, `nvmlDeviceGetDisplayActive` | once |
  | Per-process (process area owns these columns) | `nvmlDeviceGetComputeRunningProcesses_v3`, `nvmlDeviceGetGraphicsRunningProcesses_v3`, `nvmlDeviceGetProcessUtilization(lastSeenTimeStamp)` | 1 Hz while demanded |

  Every call is probed once per UUID. `NVML_ERROR_NOT_SUPPORTED` marks the metric Unsupported for the session. `GPU_IS_LOST` marks the device "Lost" and restarts the probe.
- Verdict: BUILD
- Phase: P1 (utilization, memory, temperature, power, clocks, encode/decode, identity); P2 (rest)
- Privilege: none (NVML device queries run as a normal user)

### PERF-034 GPU engine graphs
- Source: TS 2.3 engine graphs (Utilization (3D), Compute, Video Encode, Video Decode, Video Processing, "Engine %u"), context "Select GPU Engine Graphs" / "Hide This Engine Graph", [INI Performance.HiddenGpuEngines] | MC 2.5 top graph and Encode/Decode
- Behaviour: A grid of engine charts in the CPU grid style (TS), one per engine the backend reports. The context menu offers "Select GPU Engine Graphs" (checklist) and "Hide This Engine Graph". Engine labels:
  - NVIDIA: "3D / Compute" (NVML does not split them), "Memory controller", "Video Encode", "Video Decode", plus "JPEG" and "Optical flow" when supported.
  - AMD: "3D" (gfx), "Compute", "Copy" (dma), "Video Decode", "Video Encode", "JPEG".
  - Intel: "Render/3D", "Copy", "Video", "Video Enhance", "Compute".

  A shared encoder/decoder (MC `encode_decode_shared`) shows one "Video Encode/Decode" chart.
- Linux data/mechanism:
  - NVIDIA: PERF-033.
  - AMD: total = `device/gpu_busy_percent`. Engines come from `device/gpu_metrics` (binary `gpu_metrics_v1_x`/`v2_x`/`v3_x`: `average_gfx_activity`, `average_umc_activity`, `average_mm_activity` or `vcn_activity[]`, `jpeg_activity[]`). Fallback: the sum over readable `/proc/*/fdinfo/*` of `drm-engine-{gfx,compute,dma,dec,enc,jpeg}` ns deltas divided by elapsed time, with clients deduplicated by `drm-client-id`.
  - Intel i915: i915 PMU through `perf_event_open` (type from `/sys/bus/event_source/devices/i915/type`, events `rcs0-busy`, `bcs0-busy`, `vcs0-busy`, `vecs0-busy`, `ccs0-busy`), else fdinfo `drm-engine-{render,copy,video,video-enhance,compute}`.
  - Intel xe: fdinfo `drm-cycles-<class>` / `drm-total-cycles-<class>`.

  fdinfo fallback charts carry the coverage marker "Own processes only" in their label tooltip (architecture review, coverage).
- Verdict: ADAPT (TS grid; engine sets per Linux driver)
- Phase: P1 (NVIDIA and AMD total); P2 (all engines)
- Privilege: Intel PMU needs `CAP_PERFMON` or `kernel.perf_event_paranoid` ≤ 0. tasklens does not request that; it uses the fdinfo fallback with the coverage marker. The fdinfo fallback sees only the user's own processes.

### PERF-035 GPU memory, temperature, power and clock graphs
- Source: TS 2.3 ("VRAM and temperature on a separate row" [CL 0.9.2]) | MC 2.5 middle and bottom graphs (Encode/Decode, Memory Usage, Power Draw, Clock Speeds, Temperature)
- Behaviour: Below the engine grid there is a second row of charts:
  - "Dedicated memory" (VRAM used, max total) with "Shared memory" dashed when the backend has GTT;
  - "Temperature".

  An optional third row is toggled in the context menu "Select GPU Engine Graphs": "Power" (draw filled, limit as a dashed line) and "Clocks" (graphics solid, memory dashed, linked scale). The third row is off by default. Options with no data are disabled (MC).
- Linux data/mechanism:
  - NVIDIA: PERF-033 (no GTT on NVIDIA, so no shared series).
  - AMD:
    - memory: `device/mem_info_vram_{used,total}`, `mem_info_gtt_{used,total}`;
    - temperature: hwmon `temp1_input` (edge), `temp2` (junction, shown in the tooltip), `temp3` (mem);
    - power: hwmon `power1_average`, else `power1_input`; limit `power1_cap`;
    - clocks: hwmon `freq1_input` (sclk) and `freq2_input` (mclk), maximums from the last line of `pp_dpm_sclk` / `pp_dpm_mclk`.
  - Intel:
    - memory: dGPU VRAM from the sum of fdinfo `drm-total-local0`/`drm-resident-vram0` (own processes; iGPU has no dedicated memory);
    - temperature: hwmon `temp1_input` (dGPU only);
    - power: hwmon `energy1_input` delta (dGPU), else RAPL `intel-rapl:0:1` ("uncore") for the iGPU;
    - clocks: i915 `cardN/gt_act_freq_mhz`, max `gt_max_freq_mhz`; xe `device/tile0/gt0/freq0/{act_freq,max_freq}`.
- Verdict: BUILD (TS rows; MC power and clock graphs as an optional row)
- Phase: P1 (memory, temperature); P2 (power, clocks)
- Privilege: none, except that Intel iGPU power needs RAPL readable (PERF-075).

### PERF-036 GPU stats grid
- Source: TS 2.3 GPU fields L28294-28303 | MC 2.5 stats
- Behaviour: Rows:
  1. Utilization, Dedicated memory ("2.8 / 15.9 GiB"), Shared memory.
  2. Temperature, Power ("31.9 / 300 W"), Fan.
  3. GPU clock ("1905 / 3105 MHz"), Memory clock, P-state.
  4. Video encode, Video decode, Throttle reasons.
  5. Driver version, Kernel module, VBIOS.
  6. Vulkan version, OpenGL version, PCIe ("Gen 4 x16 (max Gen 4 x16)").
  7. Physical location ("PCI bus 45, device 0, function 0"; TS format), Hardware reserved memory, UUID.

  "Unsupported" shows where the backend has no value.
- Linux data/mechanism:
  - NVIDIA: PERF-033. Kernel module = `/sys/module/nvidia/version`. Hardware reserved = `MemoryInfo_v2.reserved` (380 MiB here). P-state, throttle and fan are NVIDIA only.
  - AMD: `/sys/module/amdgpu/version` is often absent, so it shows the kernel version. VBIOS = `device/vbios_version`. Fan = hwmon `fan1_input` / `pwm1`. P-state uses `power_dpm_force_performance_level`.
  - All vendors: PCIe from `device/{current_link_speed,current_link_width,max_link_speed,max_link_width}`, with GT/s mapped to generation: 2.5→1, 5→2, 8→3, 16→4, 32→5, 64→6. Location from the PCI slot name.
- Verdict: ADAPT (DirectX version → Vulkan/OpenGL; Driver date → Kernel module version, since Linux drivers carry no date)
- Phase: P1 (rows 1-3, 5); P2 (rows 4, 6, 7)
- Privilege: none

### PERF-037 Vulkan and OpenGL version probe
- Source: MC 7 GPU (EGL on GBM and ash, each in a forked child) | TS DirectX version
- Behaviour: Fills "Vulkan version" ("1.4.312, NVIDIA 615.71.09") and "OpenGL version" ("4.6.0 NVIDIA 615.71.09", ES marked "OpenGL ES"). Shows "Probing…" until done and "Unsupported" on failure.
- Linux data/mechanism: Run once at startup, and again after a driver update detected by a change in `/sys/module/*/version`. Implementation: a short-lived `tl-gpu-probe --api-versions` child. This is a separate invocation from the long-running NVML probe, so a crash in a GL or Vulkan driver cannot hit the UI or NVML sampling.
  - Vulkan: `ash` → `vkEnumeratePhysicalDevices`, matched by PCI bus through `VK_EXT_pci_bus_info`; `apiVersion` plus `VkPhysicalDeviceDriverProperties.driverInfo`.
  - OpenGL: `eglGetPlatformDisplay(EGL_PLATFORM_DEVICE_EXT)`, with the device matched through `EGL_DRM_DEVICE_FILE_EXT`; a surfaceless context, then `glGetString(GL_VERSION)`.
  - Deadline 5 s.
- Verdict: ADAPT
- Phase: P2
- Privilege: none

### PERF-038 Displays list
- Source: TS 2.3 Displays (connection type, "%s @ %d Hz", "%d-bit, %s", orientation) L29511-29519
- Behaviour: A "Displays" table on each GPU page with columns Monitor (EDID name, e.g. "DELL U2723QE"), Connector (DP-2), Type (DisplayPort, HDMI, eDP/Internal, DVI, VGA, USB-C/DP-alt, Virtual), Mode ("3840×2160 @ 144 Hz"), Bit depth ("max 10 bpc"), Orientation (Landscape, Portrait, Landscape (flipped), Portrait (flipped)), Scale ("110 %"). Connected but disabled outputs show Mode "Off".
- Linux data/mechanism:
  - Connectors per GPU: `/sys/class/drm/cardN-*/status` ("connected") and `edid` (parsed for the monitor name descriptor 0xFC). The sysfs `enabled` attribute is not used, because it is wrong on nvidia-drm (verified).
  - Mode, refresh and max bpc: DRM KMS read-only ioctls on `/dev/dri/cardN` (no DRM master needed; the seat user has an ACL): `DRM_IOCTL_MODE_GETRESOURCES`, `GETCONNECTOR`, `GETENCODER`, `GETCRTC` (current mode, refresh = clock×1000/(htotal×vtotal)), connector property "max bpc".
  - Orientation and scale on KDE: `kscreen-doctor -j` run once when the page opens and when KMS connector status changes (JSON fields `rotation`, `scale`, `enabled`, `currentModeId`). On other desktops these columns show "Unknown".
- Verdict: ADAPT (TS fields from DRM KMS plus KScreen; actual output bit depth is not exposed by Linux, so "max bpc" is shown and labelled as a maximum)
- Phase: P2
- Privilege: none (verified: `card1` has the seat ACL)

### PERF-039 GPU card
- Source: TS 2.3 GPU card | MC 2.0 GPU row
- Behaviour: "GPU N", model, then "52 °C" plus "· 32 W" when power is known. The graph shows utilization %.
- Linux data/mechanism: PERF-033 and PERF-035.
- Verdict: BUILD
- Phase: P1
- Privilege: none

---

## G. Fans and batteries

### PERF-040 Fan pages
- Source: MC 2.6 (per hwmon fan; RPM graph with dashed PWM; temperature graph; stats Fan Speed, PWM Percentage, Current Temperature)
- Behaviour: One page per fan: "Fan N" or its label, with the paired sensor name as subtitle.
  - Graphs: "Fan speed" (RPM filled, PWM % dashed on a 0-100 secondary scale; fixed at `fanN_max` if present, else sticky-up) and "Temperature" (paired sensor; hidden if none).
  - Stats: Fan speed (RPM), PWM ("45 %", or "Auto"/"Manual" mode from `pwmN_enable`), Temperature, Chip (`nct6797`), Sensor.
  - Context menu: "Temperature sensor >" (every temp input of the same chip plus CPU and GPU temperatures) and "Rename…". Both are saved per fan identity.
  - Default pairing: the same chip and index. MC uses the same-index temp without a choice; the menu lets the user fix wrong pairs.
  - Fans whose RPM has read 0 since start and that have no label are hidden by default as "probably not connected". The setting "Show fans reading 0 RPM" turns them on, and they reappear as soon as they spin.
  - The GPU fan stays on the GPU page, not here.
- Linux data/mechanism: `/sys/class/hwmon/hwmon*/fan*_input`, `fanN_label`, `fanN_max`, `fanN_min`, `pwmN` (/255), `pwmN_enable`, `tempN_input`, `tempN_label`, `name`. Polled at 1 Hz while displayed. Board chips (`nct6797` here) can take tens of ms per read, so they run in the blocking pool and never at 10 Hz (PLAN section 3).
- Verdict: ADAPT (MC page; MC's config-dir override files become an in-app rename and sensor choice)
- Phase: P2
- Privilege: none. Missing Super I/O drivers mean no fans; PERF-077 covers sensors-detect.

### PERF-041 Battery and UPower device pages
- Source: MC 2.7 and Astra MC Battery (power graph, 7-day history, stats, all UPower kinds)
- Behaviour: One page per UPower device except DisplayDevice and line power. The heading is the kind and index ("Mouse", "Battery 1"), with vendor and model as subtitle.
  - System batteries: "Power" graph (charging positive, discharging negative, filled to zero, symmetric scale with Pow10 rounding).
  - Peripherals and unknown state: "Charge" 0-100 % graph.
  - Stats (MC list, with MC's "Swap available"-style bugs fixed): Percentage, State, Time to full/empty, Energy, Energy rate, Voltage, Charge cycles, Capacity (health %), Technology, Energy empty, Energy full, Energy full (design), Voltage min (design), Voltage max (design), Powering system, Serial, Charge threshold (shows "Disabled" when supported but off; MC shows "Yes" there), Charge start and end threshold, Temperature.
  - Rows with no value are hidden here (MC). UPower values differ widely per device kind, and a fixed grid would be mostly empty for a mouse.
- Linux data/mechanism: zbus on the system bus.
  - `org.freedesktop.UPower.EnumerateDevices`, signals `DeviceAdded` and `DeviceRemoved`.
  - Per device `org.freedesktop.UPower.Device` properties: `Type`, `PowerSupply`, `Percentage`, `State`, `TimeToEmpty`, `TimeToFull`, `Energy`, `EnergyRate`, `EnergyEmpty`, `EnergyFull`, `EnergyFullDesign`, `Voltage`, `VoltageMinDesign`, `VoltageMaxDesign`, `ChargeCycles`, `Capacity`, `Technology`, `Temperature`, `Serial`, `Model`, `Vendor`, `NativePath`, `ChargeThresholdSupported`, `ChargeThresholdEnabled`, `ChargeStartThreshold`, `ChargeEndThreshold`.
  - Updates come from `PropertiesChanged`, with no polling.
  - Fallback without UPower: `/sys/class/power_supply/*` (`capacity`, `status`, `energy_now`, `power_now`, `voltage_now`, `cycle_count`, `scope`), rescanned every 30 s.
- Verdict: BUILD (MC)
- Phase: P2
- Privilege: none

### PERF-042 Battery charge history
- Source: MC 2.7 "Battery Charge over past week"
- Behaviour: For system batteries with 2 or more history points, a graph "Battery charge over past week" spans 7 days with 7 day gridlines. Gaps are drawn as faint dashed lines. The graph refreshes on the UPower history change.
- Linux data/mechanism: `org.freedesktop.UPower.Device.GetHistory("charge", 604800, 1000)` → array of (time, value, state). Re-fetched on page open and every 10 min while open.
- Verdict: BUILD
- Phase: P2
- Privilege: none

---

## H. Graph engine behaviour (all graphs)

### PERF-055 Graph rendering style
- Source: TS 2.3 (filled area, brighter top line, faint vertical grid, right-to-left scroll) | MC 2.0 graph engine (rounded clip radius 7, 1 px outline, scrolling grid, dashed secondary series) | architecture review section 4
- Behaviour:
  - Filled area at the fill colour, with a 1.5 logical px top line in the line colour.
  - Secondary series are dashed (5,5) and unfilled.
  - Vertical gridlines in the line colour at 12 % alpha, spaced every 1/6 of the time range; they scroll with the data (TS and MC).
  - 3 faint horizontal gridlines at 25/50/75 % on percent graphs.
  - Corner radius follows the theme's Rounded Corners setting (4-6 px).
  - 1 px border in the divider colour.
  - The label sits at the top left and the current value at the top right.
  - On graphs over 200 px tall: "<Metric> over 60 seconds" at the bottom left and the scale maximum at the top right.
- Linux data/mechanism: the app-owned chart element. Quads for background and grid. Cached linear fill and stroke paths, rebuilt only when samples, size, scale or y-range change. Min/max per physical-pixel bucket when samples exceed width. A time translation per frame (architecture review rendering rules 1-7).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-056 Time ranges and history tiers
- Source: TS 4.2 minigraph Time Range (30 seconds, 1 minute, 5 minutes, 10 minutes) | MC Data Points 10-600 × update interval | architecture review (60 s default, ring capacity 602)
- Behaviour: Each graph group has a time range of 30 s, 1 min (default), 5 min or 10 min.
  - Performance pages: one range for the whole tab, set from the page context menu "Time range >" and saved as `performance.time_range`.
  - Minigraphs and tray graphs: their own range, per card.

  MC's Data Points slider is replaced by this choice.
- Linux data/mechanism: two ring tiers per series in the engine.
  - Fast tier: capacity 602 at the source rate (10 Hz for CPU while visible), so 60 s.
  - Slow tier: capacity 602 at 1 Hz, so 10 min, holding the min, max and last value of each second.

  Ranges of 60 s or less draw from the fast tier when it exists. Longer ranges draw from the slow tier using its min and max. History exists only for series that have been demanded since start, or since the device appeared. Older time shows a gap, not zero. Memory estimate: about 120 series × 2 tiers × 602 × 16 B ≈ 2.3 MB (inferred from the review's sample layout).
- Verdict: ADAPT (TS ranges; MC data points dropped in favour of ranges)
- Phase: P1 (30 s, 1 min); P2 (5 min, 10 min)
- Privilege: none

### PERF-057 Smooth scrolling
- Source: TS [INI UI.SmoothGraphs=1] "Interpolated graph scrolling" | MC `performance-sliding-graphs` (50 ms timer) | architecture review smooth time mapping
- Behaviour: Setting "Smooth graphs" (Settings > Motion, default on). When on, graphs scroll continuously at the monitor refresh rate (144 Hz here), drawing one sample interval behind real time. When off, or with reduced motion, or when "Animations" is off, graphs step once per sample and request no animation frames. Numeric readouts always show the latest measured value.
- Linux data/mechanism: one GPUI `request_animation_frame` per window, only while a visible chart is moving. x = right − (right_edge_time − t) × width / range (architecture review).
- Verdict: BUILD (TS meaning; MC's 50 ms timer method rejected)
- Phase: P1
- Privilege: none

### PERF-058 Scaling modes
- Source: MC graph engine (Fixed, ScaleUp/Down, Sticky*, Stacking; rounding Pow2, Pow2Base10, Integer, Pow10; linked datasets) | architecture review "Units and graph ranges" | TS "Auto-scale network transfer rates"
- Behaviour: Each graph type has a fixed rule:
  - Percent (CPU, GPU engines, disk active, battery %): fixed 0-100.
  - Memory, VRAM, swap: fixed 0 to total.
  - Commit: 0 to max(CommitLimit, visible peak).
  - Rates (disk, network, PCIe): the scale rises at once when a value exceeds it. It falls only after the visible maximum has stayed below half the scale for 10 s (architecture review). Paired series share one scale (read/write, send/receive).
  - Power and clock: sticky up, never shrinking during the session.
  - Temperature: sticky up and down, starting at 35-45 °C, with a 10 °C minimum span.
  - Stacked CPU: 0 to 100 × n.

  Scale maxima round to 1, 2 or 5 × 10^n for decimal units and to powers of 2 for binary units.
- Linux data/mechanism: none
- Verdict: BUILD (MC modes reduced to fixed rules per type; review rule for rates)
- Phase: P1
- Privilege: none

### PERF-059 Network scale to link speed
- Source: MC `performance-page-network-dynamic-scaling` (true)
- Behaviour: Setting "Scale network graphs dynamically", default on. When off, network graphs use a fixed maximum equal to the link speed, with dynamic scaling as fallback when the speed is unknown.
- Linux data/mechanism: `/sys/class/net/<if>/speed` (Mb/s), or Wi-Fi bitrate from PERF-029.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-060 Units
- Source: TS Network transfer unit (Bits per second / Bytes per second), [INI UI.ProcessNetworkRateUnit=0] bits, NetworkRateAutoScale | MC Units page (Bytes/Bits × Base 10/Base 2 for Memory, Drive, Network) | architecture review (IEC memory)
- Behaviour: Settings > Units has three rows, each with a Bytes/Bits toggle and a Base 2/Base 10 toggle:
  - Memory: Bytes, Base 2 (GiB). Bits is disabled for memory, because a bit count of RAM has no use.
  - Disk: Bytes, Base 2.
  - Network: Bits, Base 10 (Mbps; TS default).

  "Auto-scale transfer rate units" (TS, default on) picks the unit prefix per value. When off, every rate uses one fixed prefix (the setting "Fixed rate unit": K, M or G). Number format: 2 decimals below 10, 1 below 100, else 0 (MC). Rates end in "/s" for bytes and "bps" for bits. The settings agent owns the settings page; these units apply to every PERF surface.
- Linux data/mechanism: none (tl-model formatting)
- Verdict: BUILD (merge: MC's per-class units, TS's network default and auto-scale)
- Phase: P1 (defaults); P2 (all toggles)
- Privilege: none

### PERF-061 Graph colours
- Source: TS palette (PLAN section 9) and 18 graph accents L28548-28565 | MC per-type colours
- Behaviour: Colours per device type from the theme: CPU #0D546D fill / #0A7096 line, Kernel #C1514E, Memory #7E6A9A, GPU #508A59 / #71B768, Disk #D9B567, Network #359CAB. Fans and batteries have no TS colour; they take the theme accents "Violet" and "Lime". Devices of one type share a colour (TS and MC). Minigraph and tray cards can override with "Accent" or any of the 17 named accents. Performance pages use theme colours only, because TS offers no per-page colour.
- Linux data/mechanism: theme slots (theme area owns the slot list).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-062 Gaps, stale and unavailable states
- Source: architecture review (validity states never shown as zero) | MC NaN breaks the line
- Behaviour:
  - Missing samples break the line and leave the area empty, with no ramp down to zero.
  - A source that has missed 3 or more intervals greys the chart and shows "Stale (8 s)" in the value spot.
  - Unsupported metrics show "Unsupported". Permission problems show "Needs setup" or "Restricted" with a tooltip giving the reason and the fix (PERF-075).
  - After suspend and resume, a gap marks the sleep time.
  - Warm-up, before the second sample of a rate: "…".
- Linux data/mechanism: per-sample `flags` (valid, gap, reset) in the history ring. Suspend is detected by a CLOCK_BOOTTIME vs CLOCK_MONOTONIC jump, or by logind `PrepareForSleep`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-063 Hover readout
- Source: TS 4.2 (hover marker following the curve [CL 0.8.28], " avg %.1f%s ", " peak %.1f%s ", " %u samples") | MC (none)
- Behaviour: Hovering any graph shows a vertical marker and a dot on each series. A tooltip shows the time offset ("−12 s"), each series value, and for the visible range "avg 12.3 %", "peak 48.1 %" and "601 samples" (TS strings).
- Linux data/mechanism: reads the history slice already used for drawing.
- Verdict: BUILD (TS)
- Phase: P2
- Privilege: none

### PERF-064 Curved lines option
- Source: MC `performance-smooth-graphs` (cubic Bézier)
- Behaviour: Setting "Curved graph lines", default off. When on, lines use monotone cubic interpolation (Fritsch-Carlson). It never overshoots a sample, so percentages cannot exceed 0-100, and spikes stay at their real height. MC's horizontal-tangent Bézier is not used.
- Linux data/mechanism: tessellated with lyon (GPUI `PathBuilder::cubic_bezier_to`) in the same cached path.
- Verdict: ADAPT (monotone curve instead of MC's Bézier, to meet the architecture review's no-overshoot rule)
- Phase: P4
- Privilege: none

---

## I. System minigraph panel, tray graphs and mini window

### PERF-065 System minigraph panel
- Source: TS 4.2 (floating cards over the bottom right of Processes: VRAM, GPU, Network, Disk, Memory, CPU; label top left, value top right, drop shadow), [INI UI.ShowCpuGraph, ShowRamGraph, ShowDiskGraph, ShowNetworkGraph, ProcessGraphs.ShowGpuTemperatureGraph]
- Behaviour: A grid of cards (about 200×80 logical px) floats over the bottom right of the Processes tab, with the theme drop shadow at DropShadowIntensityPct.
  - Default cards: CPU, Memory, Disk, Network, GPU, VRAM, all on. "GPU Temperature" is off.
  - View > Graphs has toggles: CPU Graph, Memory Graph, Disk Graph, Network Graph, GPU Graph, VRAM Graph, GPU Temperature Graph.
  - Sources: Disk = sum over physical disks of read+write. Network = sum over non-virtual interfaces of rx+tx. GPU and VRAM = GPU 0, or the GPU picked with the card menu "Device >".
  - Process-specific cards ("Add Minigraph", name graph, app graph) are added by the process area through the same card model. This panel renders and stores them.
- Linux data/mechanism: reads the same history rings as the Performance tab. While the Processes tab is visible, the panel adds demand for those system sources at 1 Hz.
- Verdict: BUILD (TS)
- Phase: P1
- Privilege: none

### PERF-066 Minigraph card menu, time range and colour
- Source: TS 4.2 context menu (Hide Graph, Time Range > 30 seconds/1 minute/5 minutes/10 minutes, Graph Color > Accent plus 17 accents, Add tray icon graph), toasts
- Behaviour: Right-click a card for:
  - "Hide Graph";
  - "Time Range >" with 30 seconds, 1 minute (default), 5 minutes, 10 minutes;
  - "Graph Color >" with Accent and the 17 named colours, each with a swatch;
  - "Device >" (GPU and disk cards when more than one device exists);
  - "Detach" or "Attach";
  - "Add tray icon graph";
  - "Reset Minigraph Layout".

  Toasts (TS): "Minigraph added", "Minigraph already exists", "No space for more minigraphs", "Unable to add minigraph", "Minigraph layout reset". Hover readout as PERF-063.
- Linux data/mechanism: TOML `[[minigraphs]]` entries: source, device identity, colour, range, slot, detached. These replace the INI keys `Column%d`, `ProcessGraphColors` and `ProcessGraphTimeRanges`.
- Verdict: BUILD
- Phase: P1 (hide, range, colour); P3 (detach, tray)
- Privilege: none

### PERF-067 Move and detach minigraphs
- Source: TS 4.2 (cards can be moved and detached, window class `TaskSlingerDetachedGraph`, always on top, separate context menu; detached menu Show Main Window, Close Graph, Exit, Add Graph, Remove Graph), "Keep detached graphs open when closing main window" L28543, [INI ProcessGraphs.KeepDetachedOpenOnMainClose=0]
- Behaviour:
  - Moving: drag a card within the panel to reorder it. Cards snap to grid slots, and other cards shift.
  - Detaching: choose "Detach", or drag a card past the window edge. The card becomes its own small window with no decorations. Dragging its body moves it.
  - Detached window menu: "Show Main Window", "Close Graph", "Attach", "Time Range >", "Graph Color >", "Always on Top" (checked by default), "Exit tasklens".
  - Several detached cards can be grouped by dropping one onto another, which shares one window with a group outline (TS).
  - "Keep detached graphs open when closing main window" (View > Graphs, default off). When on, closing the main window leaves detached graphs open, and the app exits when the last one closes.
  - Detached windows are restored on start. On KDE they return to their saved position; elsewhere the compositor picks the position.
- Linux data/mechanism:
  - Windows: GPUI `open_window` with `WindowKind::Normal`, `WindowDecorations::Client`, `app_id` `tasklens-graph`, and `start_window_move()` on drag. Wayland gives clients no position and no keep-above.
  - On KDE: tasklens loads a KWin script through D-Bus `org.kde.KWin /Scripting loadScript(path, "tasklens-graphs")` then `start()`. The script finds windows with resourceClass `tasklens-graph` and sets `keepAbove = true`. It moves each window to its saved geometry, which tasklens passes through a per-window title tag. It reports geometry changes back by calling a tasklens D-Bus method (`io.tasklens.Graphs.GeometryChanged`).
  - On other desktops: no keep-above. The "Always on Top" item shows "Not available on this desktop".
- Verdict: ADAPT (Wayland has no client keep-above or positioning; KWin scripting supplies both on KDE)
- Phase: P3
- Privilege: none

### PERF-068 Tray icon graphs
- Source: TS 4.3 (live graph in the tray icon; sources: performance page, column, process, name filter, app filter; main tray icon hidden while a tray graph is shown; tooltip; tray menu; toasts; remembered), [INI TrayIconGraphs.*]
- Behaviour:
  - Any performance page, minigraph card, or process-area source can be added as a tray graph: "Add tray icon graph" in the page, card or row menu.
  - Each tray graph is its own tray icon, drawn as a filled graph in the source colour on a transparent background over the last 30 s.
  - While at least one tray graph exists, the plain tasklens tray icon is hidden (TS 0.9.2).
  - Tooltip: "CPU Usage: 12 %", plus a summary block "CPU: 12 %, RAM: 55 %, GPU: 11 %, Disk: 0.1 %, Network: 0.01 Mbps".
  - Left click shows or raises the main window.
  - Menu: "Show Main Window", "Tray icon graphs >" (list with checks), "Close tray icon graph", "Exit tasklens".
  - Toasts: "Tray icon graph added", "Tray icon graph removed", "Tray icon graph already exists", "Unable to add tray icon graph".
  - Tray graphs persist across restarts.
- Linux data/mechanism: StatusNotifierItem on the session bus via zbus.
  - Each graph registers its own name `org.kde.StatusNotifierItem-<pid>-<n>` with `org.kde.StatusNotifierWatcher.RegisterStatusNotifierItem`.
  - The icon is supplied through the `IconPixmap` property: ARGB32, network byte order, sizes 22, 32 and 48 px for 110 % scale. A `NewIcon` signal is emitted at 1 Hz.
  - `ToolTip` property (icon, title, text). The menu uses `com.canonical.dbusmenu`.
  - Library rule: use the `ksni` crate with default features off if `cargo tree -e normal` then shows no `tokio`. Otherwise write SNI plus dbusmenu by hand on the engine's zbus connection (async-io).
  - Rendering: a CPU raster into a 48×48 buffer with straight lines, no GPU.
- Verdict: ADAPT (Shell_NotifyIcon → SNI)
- Phase: P3
- Privilege: none. If no StatusNotifierWatcher exists (GNOME without the AppIndicator extension), the add item is disabled with the tooltip "No system tray on this desktop".

### PERF-069 Performance mini window
- Source: TS 4.4 (compact, movable, always-on-top performance window [CL 0.8.4]; probably opened by "Toggle view mode", toast "View mode: %ls")
- Behaviour: The toolbar "Toggle view mode" button cycles "Full" and "Mini", with the toast "View mode: Mini". View > Window > "Performance mini window" does the same. Mini hides the main window and opens a borderless window (about 260 px wide) holding only the sidebar device cards in the user's page order and visibility. Card graphs keep their Performance time range. The window has "Always on Top" on by default (KDE only, same KWin script as PERF-067) and is moved by dragging. Double-click a card, or press Esc, to return to Full with that page selected. Size is saved; position is saved on KDE only.
- Linux data/mechanism: GPUI second window, `app_id` `tasklens-mini`, KWin script as in PERF-067.
- Verdict: ADAPT (always-on-top only on KDE; how TS opens this window is inferred, so tasklens defines it as above)
- Phase: P3
- Privilege: none
- Open question: TS's trigger is unconfirmed (TS inventory "Gaps"). Recommended answer: the toolbar view-mode button plus the menu item, as specified.

---

## J. Information tab

### PERF-070 Information tab layout and copy
- Source: TS 2.7 (two-column layout; section headers with an accent bar and copy button `information.copy.%d`; tab-level copy icon copies everything; the filter applies to this tab)
- Behaviour:
  - Two columns of sections. Each header has a 3 px accent bar and a copy icon that copies "Section" then "  Label: value" lines.
  - The copy icon on the tab itself copies all sections. Toast: "Copied to clipboard".
  - The tab-bar filter hides non-matching rows, and hides sections with no matches.
  - Values are selectable text.
  - Static data loads once at tab open, with "Refresh" in the context menu. Live rows (uptime, counts, available memory) update at 1 Hz while visible.
  - TS's bug of Process Count 0 and Thread Count 0 is not reproduced.
- Linux data/mechanism: per PERF-071 to PERF-074. MC's System Information dialog (OS, Kernel, Desktop) is folded into these sections.
- Verdict: BUILD (TS; MC's dialog merged in)
- Phase: P2
- Privilege: none

### PERF-071 Operating System, Processor and System sections
- Source: TS 2.7 Operating System, Processor, System | MC System Information dialog (OS, Kernel, Desktop groups)
- Behaviour and data:
  - **Operating System**:
    - OS Name (`/etc/os-release` PRETTY_NAME) and Version (VERSION_ID, else BUILD_ID, else "Rolling").
    - Kernel (uname release, replacing Build) and Kernel build (uname version).
    - Architecture ("x86_64 (64-bit)").
    - Install Date: `statx(STATX_BTIME)` of `/`, else the first timestamp in `/var/log/pacman.log`, else apt or dnf logs.
    - Package Manager and Package Manager Version: `pacman --version` first line, run once and cached; same for apt and dnf.
    - Desktop (`XDG_CURRENT_DESKTOP` plus `plasmashell --version`), Windowing System (Wayland or X11 from `XDG_SESSION_TYPE`), Virtual Terminal (`XDG_VTNR`).
    - Machine ID (`/etc/machine-id`, replacing Product ID), plus the distro logo (os-release LOGO from the icon theme).
  - **Processor**: Name, Vendor (`vendor_id`), Architecture, Identifier ("Family 25 Model 97 Stepping 2" from cpuinfo family, model and stepping), Logical Processors, Cores, Base Speed (PERF-014), Max Speed (`cpuinfo_max_freq`), Microcode.
  - **System**: Computer Name (hostnamed `PrettyHostname`, else `gethostname`), User Name, Domain (`getdomainname`, hidden when "(none)"; replaces User Domain), System Uptime, Boot Time (now − CLOCK_BOOTTIME, local time), Process Count, Thread Count, Handle Count (file-nr).
- Verdict: ADAPT (Build → Kernel; Product ID → Machine ID; User Domain → NIS/DNS domain; Registered Owner DROP, see PERF-078)
- Phase: P2
- Privilege: none

### PERF-072 Memory section and modules
- Source: TS 2.7 Memory, per module
- Behaviour and data:
  - Total Physical, Available Physical, Memory Usage %, Committed, Commit Limit.
  - Peak Committed: tasklens records the highest Committed_AS seen since it started, labelled "Peak Committed (since tasklens started)", because Linux keeps no system peak.
  - Page Size (`sysconf(_SC_PAGESIZE)`), Huge Page Size (meminfo `Hugepagesize`).
  - Per module (PERF-020): Module N, Slot ("P0 CHANNEL A (DIMM 0)" from BANK_LOCATOR plus LOCATOR), Capacity, Speed ("3200 MT/s" configured; rated in the tooltip), Type ("DDR4"), Form Factor, Manufacturer, Part Number, Serial, Rank. Empty slots are listed as "Empty".
- Verdict: ADAPT (Peak Committed redefined)
- Phase: P2
- Privilege: none (verified)

### PERF-073 Disk, BIOS/Firmware and Motherboard sections
- Source: TS 2.7 Disk Drives, BIOS/Firmware, Motherboard
- Behaviour and data:
  - **Disk Drives**: per physical disk, Model, Space ("412 GiB free of 1.82 TiB"), Volumes (mount points), Type (PERF-021 kind), Interface (NVMe, SATA, USB, SCSI, virtio).
  - **BIOS/Firmware**: Firmware Type ("UEFI" if `/sys/firmware/efi` exists, else "Legacy BIOS"), Secure Boot (efivar `SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c` byte 4, readable as user), BIOS Vendor, BIOS Version, BIOS Date (`/sys/class/dmi/id/bios_{vendor,version,date}`).
  - **Motherboard**: Manufacturer, Product, Version (`board_{vendor,name,version}`), System (`sys_vendor` plus `product_name`).
  - Serials (`board_serial`, `product_serial`, `product_uuid`) are mode 0400 and shown as "Restricted (needs root)".
- Verdict: ADAPT (drive letters → mount points)
- Phase: P2
- Privilege: none. Serials need root and are not fetched; they show as restricted.

### PERF-074 Graphics and Network Adapters sections
- Source: TS 2.7 Graphics, Network Adapters (Public IP with fetch)
- Behaviour and data:
  - **Graphics**: per GPU, Name, Driver Version, Kernel Module (replaces Driver Date), Vulkan and OpenGL (replace DirectX Version), Temperature, Temperature Source ("NVML GPU core", "hwmon amdgpu edge"), Dedicated Memory, Shared Memory ("None" on NVIDIA), Resolution (each active display "3840×2160 @ 144 Hz", PERF-038).
  - **Network Adapters**: per non-virtual interface, Connection (name plus type), Details (adapter model, driver, link speed), IP Address, MAC Address. Then one Public IP row with "Fetch" (PERF-030).
- Verdict: ADAPT
- Phase: P2
- Privilege: none

---

## K. Additional setup (replaces Mission Center's first-run flow)

### PERF-075 Additional data panel
- Source: MC first-run dialog ("Enabling Additional Values", Run Additional Setup split button, Revert, View Script, Copy Script Location, Copy Command, Copy Revert Command; success and failure dialogs) | MC setup script (nethogs setcap, RAPL udev rule, sensors-detect)
- Behaviour: tasklens shows no first-run dialog. Instead:
  - Every "Needs setup" value has a link "Set up…" that opens Settings > Monitoring > "Additional data".
  - That panel has one row per item (PERF-076, PERF-077) with status "Enabled", "Not enabled" or "Not needed", and an "Enable" or "Revert" button.
  - "View script" shows the exact commands. "Copy command" copies the `pkexec …` line.
  - Results show inline: on success, "Enabled. Values appear within 2 seconds"; no restart needed, because the collectors retry. On failure, the output plus "Copy output" and "Retry".

  Why: MC's dialog explains three things at once on first launch, two of which tasklens does not need.
- Linux data/mechanism: a fixed script shipped at `/usr/lib/tasklens/tasklens-setup` (root-owned, 0755, in the package), run by `pkexec /usr/lib/tasklens/tasklens-setup <enable-rapl|revert-rapl|sensors-detect>`. A polkit action `io.tasklens.setup` with `auth_admin_keep` and `exec.path` annotation is installed at `/usr/share/polkit-1/actions/io.tasklens.setup.policy`. Nothing is written to `/tmp` (MC writes its script there).
- Verdict: ADAPT (inline per-value setup instead of a first-run dialog; no temp-file script)
- Phase: P3
- Privilege: polkit `auth_admin_keep` through pkexec. Without setup, values show "Needs setup".

### PERF-076 RAPL power access
- Source: MC setup script step 2 (`/etc/udev/rules.d/99-powercap.rules`)
- Behaviour: Row "CPU power draw (RAPL)". "Enable" installs the rule and applies it at once. The status reads "Enabled (rule from another app)" when any rule already makes `energy_uj` readable, as on this machine with MC's `99-powercap.rules`. In that case Revert is offered only for tasklens's own rule. The row text includes the caveat: "Lets any local user read CPU energy counters. On some CPUs this leaks timing information (CVE-2020-8694)."
- Linux data/mechanism: Writes `/etc/udev/rules.d/60-tasklens-powercap.rules` with `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`, then `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add`. Revert deletes the file and runs `chmod 0400` on the nodes. Detection: `access("/sys/class/powercap/intel-rapl:0/energy_uj", R_OK)`.
- Verdict: BUILD
- Phase: P3
- Privilege: pkexec as PERF-075. Fallback: Power shows "Needs setup", and the CPU bottom graph falls back to Temperature.

### PERF-077 Fan sensor detection
- Source: MC setup script step 3 (`sensors-detect --auto`)
- Behaviour: Row "Fan and board sensors". It shows "Not needed" when any hwmon `fan*_input` exists (here `nct6797` already gives 7 fans). Otherwise it offers "Detect sensors". That button runs sensors-detect, shows its output, and lists the modules it suggests. "Load now" runs `modprobe` for each; "Load at boot" writes `/etc/modules-load.d/tasklens-sensors.conf`. The row is hidden when lm_sensors is not installed, with the hint "Install lm_sensors to detect board sensors".
- Linux data/mechanism: `pkexec /usr/lib/tasklens/tasklens-setup sensors-detect`, which runs `sensors-detect --auto` and parses the "MODULES=" or `#----cut here----` block.
- Verdict: ADAPT (MC runs it blindly; tasklens runs it only when no fans are found and shows the result)
- Phase: P4
- Privilege: pkexec. Fallback: the Fans pages are absent.

### PERF-078 Dropped items
- Source: MC setup script step 1 (nethogs `setcap cap_net_admin,cap_net_raw,cap_dac_read_search,cap_sys_ptrace+pe`) | TS 2.7 Registered Owner
- Behaviour: Not built.
- Linux data/mechanism: n/a
- Verdict: DROP. nethogs: tasklens does not use nethogs. Per-process network comes from sock_diag (unprivileged), with exact numbers later from an eBPF collector owned by the process and connections areas. Granting ptrace and DAC-bypass capabilities to a third-party binary is not needed. Registered Owner: Linux has no registered-owner record.
- Phase: n/a
- Privilege: n/a

---

## Entry table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PERF-001 | Performance tab layout | BUILD | P1 |
| PERF-002 | Sidebar device cards | BUILD | P1/P2 |
| PERF-003 | Device discovery, hotplug, identity | BUILD | P1 |
| PERF-004 | Page order, hiding, edit mode | BUILD | P1/P2 |
| PERF-005 | Device type visibility defaults | BUILD | P2 |
| PERF-006 | Aggregate pages | BUILD | P4 |
| PERF-007 | Summary mode | ADAPT | P2 |
| PERF-008 | Page context menu and View jump | BUILD | P1/P2 |
| PERF-009 | Copy device info | BUILD | P1 |
| PERF-010 | CPU chart modes | BUILD | P1/P2 |
| PERF-011 | Kernel time overlay | BUILD | P1 |
| PERF-012 | Core type markers | ADAPT | P2 |
| PERF-013 | CPU bottom graph | BUILD | P2 |
| PERF-014 | CPU stats grid | ADAPT | P1/P2 |
| PERF-015 | Memory graphs | BUILD | P1 |
| PERF-016 | Memory composition bar | ADAPT | P1 |
| PERF-017 | Memory stats grid | ADAPT | P1/P2 |
| PERF-018 | zram and zswap section | BUILD | P2 |
| PERF-019 | Swap devices list | ADAPT | P2 |
| PERF-020 | Memory modules | BUILD | P2 |
| PERF-021 | Disk list and kind | ADAPT | P1 |
| PERF-022 | Disk graphs | BUILD | P1 |
| PERF-023 | Disk stats grid | ADAPT | P1/P2 |
| PERF-024 | Partitions table | ADAPT | P2 |
| PERF-025 | SMART data | BUILD | P2 |
| PERF-026 | Eject with blockers | BUILD | P2 |
| PERF-027 | Network throughput graph | BUILD | P1 |
| PERF-028 | Network stats grid | ADAPT | P1/P2 |
| PERF-029 | Wi-Fi details | BUILD | P2 |
| PERF-030 | Public IP fetch | BUILD | P2 |
| PERF-031 | Network settings link | ADAPT | P4 |
| PERF-032 | GPU backend split | ADAPT | P1/P2 |
| PERF-033 | NVML call map | BUILD | P1/P2 |
| PERF-034 | GPU engine graphs | ADAPT | P1/P2 |
| PERF-035 | GPU memory, temperature, power, clock graphs | BUILD | P1/P2 |
| PERF-036 | GPU stats grid | ADAPT | P1/P2 |
| PERF-037 | Vulkan and OpenGL version probe | ADAPT | P2 |
| PERF-038 | Displays list | ADAPT | P2 |
| PERF-039 | GPU card | BUILD | P1 |
| PERF-040 | Fan pages | ADAPT | P2 |
| PERF-041 | Battery and UPower pages | BUILD | P2 |
| PERF-042 | Battery charge history | BUILD | P2 |
| PERF-055 | Graph rendering style | BUILD | P1 |
| PERF-056 | Time ranges and history tiers | ADAPT | P1/P2 |
| PERF-057 | Smooth scrolling | BUILD | P1 |
| PERF-058 | Scaling modes | BUILD | P1 |
| PERF-059 | Network scale to link speed | BUILD | P2 |
| PERF-060 | Units | BUILD | P1/P2 |
| PERF-061 | Graph colours | BUILD | P1 |
| PERF-062 | Gaps, stale, unavailable states | BUILD | P1 |
| PERF-063 | Hover readout | BUILD | P2 |
| PERF-064 | Curved lines option | ADAPT | P4 |
| PERF-065 | System minigraph panel | BUILD | P1 |
| PERF-066 | Minigraph menu, range, colour | BUILD | P1/P3 |
| PERF-067 | Move and detach minigraphs | ADAPT | P3 |
| PERF-068 | Tray icon graphs | ADAPT | P3 |
| PERF-069 | Performance mini window | ADAPT | P3 |
| PERF-070 | Information layout and copy | BUILD | P2 |
| PERF-071 | OS, Processor, System sections | ADAPT | P2 |
| PERF-072 | Memory section and modules | ADAPT | P2 |
| PERF-073 | Disk, BIOS, Motherboard sections | ADAPT | P2 |
| PERF-074 | Graphics, Network Adapters sections | ADAPT | P2 |
| PERF-075 | Additional data panel | ADAPT | P3 |
| PERF-076 | RAPL power access | BUILD | P3 |
| PERF-077 | Fan sensor detection | ADAPT | P4 |
| PERF-078 | Dropped items (nethogs caps, Registered Owner) | DROP | n/a |

IDs 043 to 054 are unused (numbering gap only; no entries are missing).

## Conflicts with the architecture review

1. **Time ranges.** The review sets v1 at 15 s and 60 s with one 602-sample ring. PERF-056 adds TS's 30 s, 5 min and 10 min, which needs a second 1 Hz ring tier (602 samples) per series. The extra memory is about 1 MB. The review's no-catch-up and gap rules still apply.
2. **Last-window exit.** The review says closing the last window exits in v1. PERF-067 ("Keep detached graphs open", default off) and PERF-068 (tray) keep the app alive without the main window. Both are P3. If P3 is inside v1, the review's lifecycle rule needs an exception for these two modes.
3. **KWin scripting.** The review lists KWin-based features and always-on-top as deferred or unsupported. PERF-067 and PERF-069 use a KWin script for keep-above and positioning on KDE in P3, and degrade on other desktops.
4. **Splines.** The review bans splines. PERF-064 keeps MC's smooth-line option, off by default, using monotone cubic interpolation that cannot overshoot. That meets the review's stated reason.
5. **Extra child-process use.** PERF-037 runs Vulkan and EGL probing as a separate short-lived `tl-gpu-probe --api-versions` invocation. This extends the rule that NVML lives only in tl-gpu-probe. It is not a conflict, but the probe's protocol needs a second mode.
6. **Hidden-mode demand.** Tray graphs and detached cards are demand sources while the main window is hidden. The review's hidden policy ("off without recording") must count them, as its demand-aggregation rule already says for detached graphs.
7. **PLAN.md, not the review:** PLAN section 3 says DIMM details need root. That is wrong on this machine, because the udev database exposes them unprivileged (PERF-020). Only DMI board, product and chassis serials need root.

Notes for the leader:

- I looked at `performance.png` and `information.png` as layout references. I skipped the Codex second opinion on them, because they are reference screenshots read inside a research loop and nothing in this spec depends on a disputed reading of them.
- **Unchecked: NVML calls.** I did not call NVML directly. The NVML support claims rest on `nvidia-smi --query-gpu` fields on this machine. JPEG and optical-flow utilization support on the RTX 5070 Ti is not confirmed; PERF-033 hides those engines when the call returns NOT_SUPPORTED.
- **Unchecked: `kscreen-doctor -j` keys.** The JSON field names in PERF-038 (`rotation`, `scale`, `currentModeId`) are from memory. Check them before building that bead.
