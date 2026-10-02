> Note from the leader: this addendum was written against the first PERF draft, so its IDs (PERF-069, PERF-079 to PERF-093) do not match features-perf.md. The reconciliation pass merges it by subject. Its verified facts (NVML JPEG and OFA support, kscreen-doctor JSON keys, the sysfs `enabled` doubt) take precedence over features-perf.md.

I could not see the saved PERF spec from this session; the only number I know is PERF-069. So the amendments below name each entry by its subject. Please match them to the saved IDs.

# PERF addendum: TaskSlinger cross-check items

## What I checked on this machine

**NVML JPEG and OFA: supported.** `/usr/lib/libnvidia-ml.so.1` (driver 615.71.09) exports `nvmlDeviceGetJpgUtilization` and `nvmlDeviceGetOfaUtilization`. `nvidia-smi --query-gpu=utilization.jpeg,utilization.ofa` returns `0 %, 0 %`, not `[N/A]`. `nvidia-smi -q -d UTILIZATION` lists JPEG and OFA rows. These symbols are also present:
- `nvmlDeviceGetTemperatureV`
- `nvmlDeviceGetMemoryInfo_v2`
- `nvmlDeviceGetProcessesUtilizationInfo`
- `nvmlDeviceGetCurrentClocksEventReasons`
- `nvmlDeviceGetGpuMaxPcieLinkGeneration`
- `nvmlDeviceGetPcieThroughput`
- `nvmlDeviceGetFanSpeed_v2`
- `nvmlDeviceGetComputeRunningProcesses_v3` and `nvmlDeviceGetGraphicsRunningProcesses_v3`

**`kscreen-doctor -j` keys.**
- Top level: `features`, `outputs`, `screen`, `tabletModeAvailable`, `tabletModeEngaged`.
- Each output: `brightness`, `clones`, `connected`, `currentModeId`, `enabled`, `followPreferredMode`, `iccProfilePath`, `icon`, `id`, `modes`, `name`, `pos`, `preferredModes`, `priority`, `replicationSource`, `rotation`, `scale`, `size`, `sizeMM`, `type`.
- Each mode: `id`, `name` (`1920x1200@60`), `refreshRate` (float Hz), `size`.
- `type` is the KScreen `Output::Type` enum. All three outputs report 14, which is DisplayPort.
- `rotation` uses 1 = none, 2 = left, 4 = inverted, 8 = right. DP-3 reports 2, with size 1200x1920, so it is in portrait.
- `priority` 1 is the primary output.
- **The JSON has no bit depth, HDR, VRR or WCG keys.** The text output (`kscreen-doctor -o`) does show `Vrr`, `HDR`, `Wide Color Gamut` and `Color resolution` ("unknown" here). That output is coloured and unstable to parse.

**One discrepancy you need to know about.** During the check, both `kscreen-doctor` and `/sys/class/drm/card1-*/enabled` reported only DP-2 as enabled. DP-1 and DP-3 were connected but disabled. That disagrees with the brief's three active monitors. Two readings are possible: the monitors were really off at that moment, or both sources are wrong. I cannot tell which. If the saved spec says sysfs `enabled` is unreliable on NVIDIA, that claim is not supported, because KScreen agrees with sysfs. Please withdraw it, or confirm with Oystein whether all three monitors were on.

## Amendments to existing entries

### PERF-069 Performance mini window (amended trigger)
- Behaviour: The cross-check (B.1) shows that "Toggle view mode" is the Processes-tab button that cycles Tree View, Task Manager Grouping and List View. It does not open the mini window. The process-view agent owns that button. The mini window gets its own triggers:
  - View > Window > **Performance mini window** (checkable);
  - the shortcut **Ctrl+Shift+M** (Ctrl+M stays as summary mode);
  - **Show mini window** in the tray icon menu, from P3.

  Closing the mini window unchecks the menu item. Everything else in PERF-069 is unchanged.
- Verdict: ADAPT (TaskSlinger's trigger is unknown, so tasklens defines one).
- Phase: P3.
- Privilege: none.

### GPU page engine graphs (amend the GPU engines entry)
- Behaviour: Remove "Throughput" from any GPU engine list. It is the title of the Network graph (L28242). The TaskSlinger engines are Utilization, Compute, Video Encode, Video Decode and Video Processing. On NVIDIA, tasklens shows these engine graphs:

  | Graph | NVML call |
  |---|---|
  | 3D / Compute | `nvmlDeviceGetUtilizationRates.gpu` |
  | Memory controller | `.memory` |
  | Video Encode | `nvmlDeviceGetEncoderUtilization` |
  | Video Decode | `nvmlDeviceGetDecoderUtilization` |
  | JPEG | `nvmlDeviceGetJpgUtilization` |
  | Optical flow | `nvmlDeviceGetOfaUtilization` |

  JPEG and Optical flow are verified on this machine (above). The probe still checks each call per device. A call that returns `NVML_ERROR_NOT_SUPPORTED` hides its graph and is never drawn as zero. "Video Processing" has no NVIDIA engine to map to, so it is used only where AMD or Intel report a video-enhance engine.

### GPU displays list (amend the Displays entry)
- Linux data/mechanism: On KDE, run `kscreen-doctor -j` once when the GPU page opens, and again on a KScreen config change. It supplies:
  - name, connector type and enabled state;
  - primary output (`priority == 1`);
  - current mode (`currentModeId` resolved through `modes[].size` and `refreshRate`);
  - orientation (`rotation`) and scale.

  Bit depth comes only from the DRM KMS connector property `max bpc`, read through `/dev/dri/cardN` (the seat ACL allows this). Label it **Max bit depth**. HDR, VRR and wide colour gamut are not in the JSON, so v1 does not show them. They move to P4 through the `kde_output_device_v2` Wayland protocol. tasklens does not parse the text output of `kscreen-doctor -o`. Without KScreen, mode and refresh come from DRM KMS and orientation shows "Not reported".
- Phase: P2. Privilege: none.

### Public IP fetch (amend)
- Behaviour: The "Auto-fetch Public IP on launch" setting moves to **Settings > Window & startup**, following the cross-check B.7 string order. The default stays off.

### Sidebar device cards (amend the card headings)
- Behaviour: The card headings stay as in `performance.png`:
  - **CPU**, **Memory**, **GPU N**, **Disk N**;
  - a network card's heading is the interface name, such as `enp38s0`.

  The four i350 ports on this machine would all read "Ethernet", so the interface name is the only heading that tells them apart. Line 1 is the type ("Ethernet", "Wi-Fi", "Tunnel"). Line 2 is the rate in the accent colour. The NetworkManager connection name goes in the card tooltip and on the page.

### Information tab, System section (amend User Domain)
- Behaviour: TaskSlinger's fallback "WORKGROUP" is not used. "Domain" shows `getdomainname()`, or the DNS domain of the hostname. The row is hidden when that is empty or "(none)".

### CPU page, Security services row (amend)
- Behaviour: The Windows values listed in cross-check A.18 (Credential Guard, HVCI and the rest) have no Linux meaning. The row is labelled **Security features**. It lists the ones that are active, in this order, separated by commas:
  - Secure Boot;
  - Kernel lockdown (integrity or confidentiality);
  - Memory encryption (SME);
  - User shadow stack;
  - IOMMU.

  The tooltip summarises CPU vulnerability status, for example "12 mitigated, 0 vulnerable, 3 not affected".
- Linux data/mechanism:

  | Feature | Source |
  |---|---|
  | Secure Boot | efivarfs `SecureBoot-8be4df61-93ca-11d2-aa0d-e0c98e1ccb4a`, byte 4 |
  | Kernel lockdown | `/sys/kernel/security/lockdown`, the bracketed value |
  | Memory encryption | the `sme` flag in `/proc/cpuinfo`, plus `mem_encrypt` on the kernel command line |
  | User shadow stack | the `user_shstk` flag |
  | IOMMU | a non-empty `/sys/class/iommu` |
  | Vulnerabilities | `/sys/devices/system/cpu/vulnerabilities/*` |

  I did not check the permissions of these files on this machine. A source that cannot be read is left out of the list, and the tooltip names it.
- Verdict: ADAPT. Phase: P2. Privilege: none.

## New entries

### Formats and fallback values

### PERF-079 Fallback value wording
- Source: TS crosscheck A.20 (L28059-28085) | MC "Unknown", "N/A" and "Unsupported" in `cpu.rs:552`, `gpu.rs:565` and `gpu.rs:584` | both
- Behaviour: Any value that is not valid shows a fixed word in secondary text colour, never a zero. A tooltip gives the reason and the source path or call.

  | State (architecture review) | tasklens text | Replaces |
  |---|---|---|
  | warming up | `Collecting…` | MC shimmer placeholder |
  | stale | the last value in secondary colour; tooltip "Stale, last read N s ago" | |
  | restricted | `Restricted` | |
  | needs setup (RAPL not readable) | `Needs setup` (a link to PERF setup) | |
  | unsupported by hardware or driver | `Not supported` | TS "Not supported", MC "Unsupported" |
  | the source answers but has no value (for example NVML memory temperature N/A) | `Not reported` | TS "Not reported", MC "N/A" |
  | the source failed | `Unavailable` | TS "Unavailable" |
  | device missing | the card or page is removed | |
  | enum or status unknown (operstate `unknown`, as on `tailscale0`) | `Unknown` | TS "Status not reported", "<Unknown>" |
  | no CPU model name | `Unknown processor`; on ARM, implementer and part from `/proc/cpuinfo` first | TS "Unknown CPU" |
  | no GPU name | `Unknown GPU` | TS "Unknown Adapter" |
  | no network adapter name | `Unknown adapter` | |
  | primary display | ` (primary)` after the display name | TS "Primary Display" |

  TaskSlinger's "No root" is not adopted. I could not work out what it means from the strings. It seems to belong to process-keyed graphs. Rows that MC hides when a value is missing stay hidden in tasklens: CPU power, CPU temperature, disk WWN, serial and rotation rate, and battery thresholds. Rows that TaskSlinger always shows use the words above.
- Linux data/mechanism: Each metric carries its validity flag from `tl-model` (the review's validity states). The formatter maps flags to these strings in one table in `tl-ui`.
- Verdict: ADAPT (TaskSlinger's strings mapped onto the review's validity states).
- Phase: P1
- Privilege: none

### PERF-080 Up time format
- Source: TS crosscheck A.19 (L29523, L28073-28074; `performance.png` shows "2 days, 2h 38m 15s") | MC DD:HH:MM:SS (`cpu.rs:720`)
- Behaviour: TaskSlinger's format wins.
  - With days: `1 day, 2h 38m 15s` or `2 days, 2h 38m 15s`.
  - Under one day: `2h 38m 15s`.
  - Under one hour: `0h 38m 15s`, because TaskSlinger's format always includes hours.

  The same format is used on the CPU page and for Information > System Uptime. Boot Time uses `YYYY-MM-DD HH:MM:SS` in local time.
- Linux data/mechanism: `clock_gettime(CLOCK_BOOTTIME)` counts time spent in suspend, the same as `uptime`. Boot time = now (`CLOCK_REALTIME`) − `CLOCK_BOOTTIME`. `btime` in `/proc/stat` is the cross-check.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-081 Cache and fixed-size formats
- Source: TS "%ld KB" (L35646; `performance.png` shows "512 KB", "8192 KB", "98304 KB") | MC binary units (`main.rs:303-438`)
- Behaviour: Cache sizes use IEC units, because the architecture review fixes IEC for memory. Up to 1023 KiB the value shows as an integer in KiB ("512 KiB"). From 1 MiB it shows as an integer in MiB when the value is exact ("8 MiB", "96 MiB"), and otherwise with one decimal. A tooltip gives the exact KiB count ("98,304 KiB"). For each level, the label is the total across the CPU, with the instance count in the tooltip ("2 × 32 MiB"). The page shows L1 (data plus instruction combined), L2 and L3. L4, when present, shows in the tooltip only, as MC does.
- Linux data/mechanism: `/sys/devices/system/cpu/cpu*/cache/index*/{level,type,size,shared_cpu_list}`, de-duplicated by `shared_cpu_list`.
- Verdict: ADAPT ("KB" becomes "KiB"/"MiB", because the review's IEC rule is binding)
- Phase: P1
- Privilege: none

### PERF-082 Device type names and empty states
- Source: TS crosscheck A.14 (L28219-28232: "Processor", "Memory", "Disk %d"/"Disk %s", "Graphics adapter", "Adapters", "Aggregate", "No adapter", "Multiple", "Details")
- Behaviour: These TaskSlinger strings are not the card headings (`performance.png` shows "CPU", "GPU 0" and so on). tasklens uses them where they fit:
  - **Type group names** in sidebar edit mode, the right-click **View >** jump submenu and the Settings device toggles: Processor, Memory, Disks, Graphics adapters, Network adapters, Fans, Batteries.
  - **Aggregate pages** (PERF aggregate entry): headings **Aggregate storage** and **Aggregate graphics**.
  - **Empty states**: **No graphics adapter found** and **No network adapter found** on an otherwise empty type group in edit mode.
  - **Multiple**: on an aggregate card or page when member values differ, for example a drive type mixing NVMe and SSD.
  - **Details**: the heading over the stats block under the graphs on every device page.
  - **Disk %s**: not used. Disks are numbered "Disk N" in sidebar order. The page header shows the model and the kernel name (`nvme0n1`).
- Linux data/mechanism: UI strings only.
- Verdict: ADAPT (strings moved to Linux positions)
- Phase: P1 (edit-mode strings P1, aggregate strings with the aggregate pages)
- Privilege: none

### Network page

### PERF-083 Network "Display unit" field
- Source: TS crosscheck A.15 and B.6 (L28290)
- Behaviour: The network page's Details block has a row **Display unit** whose value is the current unit, for example `Mbps` or `MB/s`. Clicking the value opens a two-item menu: **Bits per second** and **Bytes per second**. The choice changes the global network unit setting (Settings > Monitoring > Network transfer unit). All network graphs, cards and process columns switch at once. The tooltip says "Applies to all network values". When "Auto-scale network transfer rates" is on, the row shows the unit family (`bits/s, auto` or `bytes/s, auto`).
- Linux data/mechanism: settings key `units.network = "bits" | "bytes"` in the TOML settings; no system data.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-084 Network graph title and Upload/Download legend
- Source: TS crosscheck A.15 and B.4 (L28242 "Throughput", L28283 "Upload", L28192 "Download") | MC "Send"/"Receive" (`network.rs:258`)
- Behaviour: TaskSlinger's labels win everywhere.
  - Graph title: **Throughput**.
  - Legend: **Download** (filled area in network colour #359CAB) and **Upload** (line only, no fill, same colour at a lighter tint).
  - Details rows: **Download speed**, **Upload speed**, **Total downloaded**, **Total uploaded** (since boot).
  - Sidebar card line 2: `↓ 1.2 Mbps ↑ 0.3 Mbps` when both are non-zero, otherwise the total rate. TaskSlinger shows one rate.

  MC's "S:" and "R:" are not used.
- Linux data/mechanism: `/sys/class/net/<if>/statistics/{rx_bytes,tx_bytes}` (download = rx). Rates come from actual elapsed monotonic time.
- Verdict: BUILD (TaskSlinger wording)
- Phase: P1
- Privilege: none

### Memory page

### PERF-085 Memory module blocks on the Memory page
- Source: TS crosscheck A.16 (L28271 "Memory module"; type list L29453-29482) | MC first-DIMM-only fields (`memory.rs:348-368`)
- Behaviour: Below the Memory page's Details block there is one collapsible block per populated DIMM, headed **Memory module N** (N counts from 1 in SMBIOS order). Each block has these fields:
  - **Slot**: bank locator and locator, for example "P0 CHANNEL A (DIMM 0)";
  - **Capacity** (GiB);
  - **Speed** (MT/s), with **Configured speed** when it differs;
  - **Type** (the SMBIOS string as reported: DDR4, DDR5, LPDDR5, HBM3, MRDIMM and others);
  - **Form factor**;
  - **Manufacturer**;
  - **Part number**;
  - **Rank**;
  - **Voltage** (configured, V).

  Serial numbers are not shown. The source exposes them, but the Copy text and screenshots would leak them. Empty slots are left out. A summary line above the blocks reads "Slots used: 2 of 4". Blocks start collapsed when there are more than 4 modules. Information > Memory uses the same data.
- Linux data/mechanism: udev properties of `/sys/devices/virtual/dmi/id`, through libudev or by parsing `/run/udev/data/+dmi:id`: `MEMORY_ARRAY_NUM_DEVICES`, `MEMORY_DEVICE_<n>_{SIZE,LOCATOR,BANK_LOCATOR,SPEED_MTS,CONFIGURED_SPEED_MTS,TYPE,FORM_FACTOR,MANUFACTURER,PART_NUMBER,RANK,CONFIGURED_VOLTAGE}`. These come from systemd's `dmi_memory_id`. I verified on this machine that they are readable without root (DDR4 3200 Kingston KHX3200C18D4/16G). No `dmidecode` and no root are needed. Fallback when the properties are missing: the block list is replaced by "Module details: Not reported".
- Verdict: BUILD
- Phase: P2
- Privilege: none (this corrects PLAN.md, which says DIMM details need root)

### Minigraphs and tray

### PERF-086 Minigraph card labels
- Source: TS crosscheck A.28 (L28570 "GPU temp", L28571-28572 "Total"/"Kernel", L28579-28581 "Process %lu", " by app", " by name") | TS IMG `processes.png`
- Behaviour: Card labels, shown top left:
  - System cards: **CPU**, **Memory**, **Disk**, **Network**, **GPU**, **VRAM**, **GPU temp**. "GPU temp" is shown only when View > Graphs > GPU Temperature Graph is on (default off).
  - The CPU card draws total CPU as a filled area and kernel CPU as a red line (#C1514E). Its hover tooltip shows a two-line legend, **Total** and **Kernel**. The CPU page's per-core grid uses the same two legend words in the first cell, as in `performance.png`.
  - A card for one process: `<name> (<pid>)`. Use `Process <pid>` only when the name has not been read yet.
  - A card for a process-name filter: `<name> by name`.
  - A card for an app: `<app name> by app`.
  - A column-keyed card appends the column's short label, for example `firefox (4121) · CPU`.
- Linux data/mechanism: UI strings. Process identity is pid plus start time (the review). App name comes from the cgroup scope and its desktop file.
- Verdict: BUILD
- Phase: P1 for system cards, P2 for process cards (with the process context menu)
- Privilege: none

### PERF-087 Minigraph states "No match" and "Ended"
- Source: TS crosscheck A.28 (L28582 "No match", L28583 "Ended")
- Behaviour: The value area at top right shows one of two states:
  - **No match**: a name-filter or app card that currently matches no process. The graph records a gap, not zero. When a process matches again, the graph resumes.
  - **Ended**: a single-process card whose process exited, because its pid plus start time are gone. The graph stops at its last sample and stays visible until the user closes the card. Context menu: **Remove Graph**. The card never re-attaches to a new process that reuses the pid.
- Linux data/mechanism: Process exit is detected when the identity is missing from a sweep, or through pidfd readability when the card holds a pidfd.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-088 Column-graph labels for memory and GPU columns
- Source: TS crosscheck A.28 (L28576-28578 "WS total", "WS shared", "GPU shared")
- Behaviour: TaskSlinger's labels use Windows working-set terms. tasklens labels these graphs:
  - **RSS**, for TaskSlinger's "WS total" and the default Memory column (the review sets RSS as the default);
  - **RSS shared**, for "WS shared" (`RssFile + RssShmem`);
  - **GPU shared**, for GTT or shared GPU memory.

  On NVIDIA, "GPU shared" is not offered, because NVML gives no per-process shared memory. The menu item is greyed out with the tooltip "Not supported on this GPU".
- Linux data/mechanism: `/proc/<pid>/status` (`VmRSS`, `RssFile`, `RssShmem`). For AMD and Intel GTT: the `drm-memory-gtt` or `drm-resident-system` keys in `/proc/<pid>/fdinfo`.
- Verdict: ADAPT (labels renamed for Linux memory terms)
- Phase: P2
- Privilege: none for your own processes. Other users' processes show `Restricted`.

### PERF-089 Tray graph "Graph color" menu item
- Source: TS crosscheck A.29 (L28587)
- Behaviour: Each tray icon graph's menu has **Graph color >**. It lists Accent (theme default) and the 17 named accents (Coral to Slate) with check marks, the same list as the minigraph menu. The choice is saved per tray graph (`tray_graphs[n].color`) and the icon redraws at once. Full tray menu order:
  - Graph color >
  - Close tray icon graph
  - separator
  - Show Main Window
  - Show mini window
  - Exit tasklens
- Linux data/mechanism: the menu is served over `com.canonical.dbusmenu` on the SNI item's object path. The icon pixmap is redrawn and `NewIcon` is emitted.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-090 Tray icon re-registration
- Source: TS crosscheck A.29 (L35761 "TaskbarCreated")
- Behaviour: When the panel restarts (for example after plasmashell crashes), every tray item tasklens owns reappears within 1 s, with the same graphs and colours.
- Linux data/mechanism: Watch `NameOwnerChanged` for `org.kde.StatusNotifierWatcher` on the session bus. When it gains a new owner, call `RegisterStatusNotifierItem` again for each item's bus name.
- Verdict: ADAPT (Explorer's TaskbarCreated becomes the StatusNotifierWatcher name owner)
- Phase: P3
- Privilege: none

### GPU page

### PERF-091 Display connector type names
- Source: TS crosscheck A.17 (L29513-29519, L35237)
- Behaviour: The Displays list on the GPU page shows the connector type as one of: VGA, DVI-I, DVI-D, DVI-A, Composite, S-Video, LVDS, Component, DIN, DisplayPort, HDMI, TV, eDP (Embedded DisplayPort), Virtual, DSI, DPI, Writeback, SPI, USB, or Unknown. TaskSlinger's Windows-only names are left out: D-Connector, UDI (Embedded), SDTV Dongle and Miracast. The row format follows TaskSlinger: `<monitor name> · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%`, and `(primary)` for the primary output.
- Linux data/mechanism:
  - The DRM connector type comes from the sysfs connector name prefix (`card1-DP-2` gives DisplayPort) or from KMS `drmModeGetConnector().connector_type`.
  - The KScreen `type` enum is the KDE cross-check (14 = DisplayPort, verified).
  - The monitor name comes from the EDID descriptor 0xFC in `/sys/class/drm/card1-DP-2/edid`.
  - Orientation comes from KScreen `rotation`: 1 Landscape, 2 Portrait (left), 4 Landscape (flipped), 8 Portrait (right).
- Verdict: ADAPT (DRM connector types replace Windows types)
- Phase: P2
- Privilege: none

### PERF-092 GPU temperature source values
- Source: TS crosscheck A.17 (L29508-29509 "Windows D3DKMT", "Windows D3DKMT: %s"; NVAPI L35305)
- Behaviour: Information > Graphics > **Temperature Source** and the GPU page temperature tooltip show where the reading came from:
  - `NVML (GPU core)`
  - `hwmon amdgpu: edge`, `hwmon amdgpu: junction`, `hwmon amdgpu: mem`
  - `hwmon i915` or `hwmon xe: pkg`
  - `Not reported` when there is no source

  On AMD, the main reading is junction when present, otherwise edge. The tooltip lists all sensors.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetTemperatureV` (exported by this driver), falling back to `nvmlDeviceGetTemperature(NVML_TEMPERATURE_GPU)`.
  - AMD and Intel: `/sys/class/drm/cardN/device/hwmon/hwmon*/temp*_{input,label}`.
- Verdict: ADAPT
- Phase: P1 for the value, P2 for the Information field
- Privilege: none

### Chrome note

### PERF-093 Performance-tab toolbar contents
- Source: TS crosscheck A.8 (`performance.png` against `processes.png`)
- Behaviour: On the Performance tab, the title-bar toolbar shows only the elevation button and the "+" window picker. The "Show system processes" toggle and the process-view cycler appear only on Processes. The chrome agent owns these buttons. This entry only records that the Performance tab adds no buttons of its own: the page toggles ("Overall"/"Per core", "Kernel on"/"Kernel off") sit in the page header, as in `performance.png`.
- Linux data/mechanism: none.
- Verdict: BUILD
- Phase: P1
- Privilege: none

## Summary table (addendum only)

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PERF-069 (amended) | Performance mini window: new trigger | ADAPT | P3 |
| PERF-079 | Fallback value wording | ADAPT | P1 |
| PERF-080 | Up time format | BUILD | P1 |
| PERF-081 | Cache and fixed-size formats | ADAPT | P1 |
| PERF-082 | Device type names and empty states | ADAPT | P1 |
| PERF-083 | Network "Display unit" field | BUILD | P1 |
| PERF-084 | Network graph title and Upload/Download legend | BUILD | P1 |
| PERF-085 | Memory module blocks on the Memory page | BUILD | P2 |
| PERF-086 | Minigraph card labels | BUILD | P1/P2 |
| PERF-087 | Minigraph states "No match" and "Ended" | BUILD | P2 |
| PERF-088 | Column-graph labels for memory and GPU columns | ADAPT | P2 |
| PERF-089 | Tray graph "Graph color" menu item | BUILD | P3 |
| PERF-090 | Tray icon re-registration | ADAPT | P3 |
| PERF-091 | Display connector type names | ADAPT | P2 |
| PERF-092 | GPU temperature source values | ADAPT | P1/P2 |
| PERF-093 | Performance-tab toolbar contents | BUILD | P1 |

## Conflicts with the architecture review

- None new. PERF-079 applies the review's validity states directly.
- PERF-087 depends on the review's process identity (pid plus start time) so that "Ended" cards never pick up a reused pid.
