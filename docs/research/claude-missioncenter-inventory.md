# Mission Center 1.2.0 feature inventory

**Sources**
- `MC/` is `/home/oystein/work/tasklens-ref/mission-center` at tag v1.2.0.
- The magpie submodule is empty in that checkout. I cloned it into my scratchpad at the pinned commit a5272b3; `M:` below means `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/magpie`, and `PL:` means `M:platform-linux/src/`.
- Magpie's own app-detection submodule (app-rummage) is also empty. I cloned it at commit 26b93b8 to `.../scratchpad/app-rummage`.
- The nvtop C sources are downloaded at build time and are not on disk. GPU backend details that depend on them are marked [inferred].
- I did not change the reference checkout.

The release notes in `MC/src/application.rs:528-570` are a good one-page summary of what 1.2.0 added.

---

## 1. Window and navigation

**Window**
- `Adw.ApplicationWindow` with a minimum of 360×400 and a template default of 800×480 (`resources/ui/window.blp:23-30`).
- Size and maximized state are restored from `window-width`, `window-height` and `is-maximized`, and saved on every change (`src/application.rs:116-166`).
- Title is "Mission Center".

**Layout**
- `Adw.OverlaySplitView`:
  - left sidebar: "Devices" list, only used on the Performance tab;
  - content: a header bar, an `Adw.ViewStack` with three pages, and an `Adw.ViewSwitcherBar` at the bottom (`window.blp:34-228`).
- The pages are Performance (`speedometer-symbolic`), Apps (`overlapping-windows-symbolic`) and Services (`system-run-symbolic`).
- The last page is restored from `window-selected-page` (`window.rs:1048`) and saved on every switch (`window.rs:454`).
- The Services tab is hidden if both service lists are still empty after 5 refreshes (`window.rs:463-484`).

**Header bar**
- Start side:
  - Toggle Sidebar (`win.toggle-sidebar`).
  - System Information button (`app.system-about`).
- Title area: a Stack that switches between the ViewSwitcher and a SearchEntry ("Type a name or PID to search", clamped to 400 px).
- End side:
  - "Toggle Device Information" button, visible only on Performance when the info panel is collapsed.
  - Search toggle, visible only on Apps and Services.
  - Main menu (`window.blp:103-183`).

**Main menu** (`window.blp:232-270`)
- Theme selector: system, light or dark, saved to `window-interface-style` (`window.rs:524-571`).
- Preferences.
- Keyboard Shortcuts.
- Open Wiki (`gitlab.com/mission-center-devs/mission-center/-/wikis/home`).
- Show First Run Dialog.
- About Mission Center. This is an `adw::AboutDialog` with release notes and credits, including NVTOP (`application.rs:507-600`).

**Search**
- Typing anywhere is forwarded to the search entry; on Apps and Services that starts search mode (`window.rs:885-943`).
- Closing search clears the text, and switching tab closes search (`window.rs:503-519, 852-861`).

**Responsive behaviour**
- Window breakpoint at `max-width: 1000sp` (`collapse-threshold`). When it applies:
  - the bottom ViewSwitcherBar is revealed;
  - the header switcher is hidden;
  - Apps and Services go into compact mode;
  - the device sidebar collapses (`window.rs:945-991`).
- The left sidebar is hidden when: not on Performance, the user hid it, summary mode is on, or the window is narrow (`window.rs:733-742`).
- The Performance page has its own breakpoint at `max-width: 570sp`, which collapses the right-hand info panel (`performance_page/mod.rs:2770-2804`).

**Pause updates**
- Holding Ctrl pauses UI refresh. The latest readings are stored and applied when Ctrl is released or the window loses focus (`window.rs:863-907, 1240-1275`).
- This copies Windows Task Manager.

**Startup**
- A shimmer placeholder shows until the first readings arrive (`window.rs:1150-1239`; `widgets/placeholder.rs`).
- A first-run dialog appears once, controlled by `first-time-running` (`window.rs:1222-1232`).

**CLI**
- `--app-id` / `-a` sets a custom application ID (`main.rs:463-528`).

**System Information dialog** (Ctrl+A; `resources/ui/about_system_dialog.blp`, `src/about_system_dialog.rs`)
- OS group: name, version, package manager, package manager version, distro logo.
- Kernel group: release, version.
- Desktop group: name, version, windowing system, virtual terminal.
- Each row has a copy button that copies "Name: value"; there is also a Copy All button (`about_system_dialog.rs:142-146, 261-273`).

**First-run dialog** (`resources/ui/first_run_dialog.blp`, `src/first_run_dialog.rs`)
- Text: "Enabling Additional Values. CPU power draw, per-process network usage and fans may require additional setup".
- Buttons: Open Wiki, a "Run Additional Setup" split button, and Close.
- The split button's menu has: Revert Additional Setup, View Script, Copy Script Location, Copy Command, Copy Revert Command.
- Results are shown as a success dialog with an offer to restart, or a failure dialog with Copy Output and Retry (`first_run_dialog.rs:120-275`).
- What the script does is in section 7.

---

## 2. Performance page

### 2.0 Shared behaviour

**Device list**
- Each row (`SummaryGraph`) has an 80×50 mini-graph with no grid, a heading, and two caption lines (`summary_graph.blp`).
- Device IDs: `cpu`, `memory`, `disk-<id>`, `net-<ifname>`, `gpu-<pci>`, `fan-<hwmon>-<n>`, `battery-<n>-<name>` (`mod.rs:1121-1708`).
- Default order: CPU, Memory, Disks, Network, GPUs, Fans, Batteries. Within a type, rows are sorted by ID (`mod.rs:1774-1854`).
- Disks, network interfaces, GPUs, fans and batteries are added and removed at runtime (`mod.rs:2072-2657`).

**What each row shows**

| Device | Heading | Line 1 | Line 2 | Mini-graph |
|---|---|---|---|---|
| CPU | "CPU" | CPU name | "N%" and temperature | total % |
| Memory | "Memory" | used and total (the "/" is missing, a bug) | % | committed (dashed) and used (filled), max = total |
| Disk | "SSD 0 (sda)", or just the type if there is one disk | model | busy % and temperature | busy %, 0 to 100 |
| Network | kind and interface, e.g. "Wired (eth0)" | "S: x/s" | "R: x/s" | tx (dashed) and rx; shows the state if not connected; 0.6 opacity if unavailable |
| GPU | "GPU N" | device name | % and temperature | utilization % |
| Fan | "Fan N" | label | PWM % or RPM, and temperature | RPM |
| Battery | "Battery N" | model | % and temperature | % |

Evidence: `mod.rs:2231-2629`.

**Sidebar edit mode**
- Opened with the edit button in the sidebar header (`window.blp:42-69`).
- In edit mode each row shows a drag handle and an on/off switch, and hidden rows reappear with the switch off (`summary_graph.rs:338-350`).
- Header buttons: Enable All, Disable All, Reset to Default (`mod.rs:2907-2989`).
- Drag-and-drop reordering:
  - the drag image is a snapshot of the row;
  - a 3 px drop line is drawn in the accent colour (`sidebar_drop_hint.rs`).
- Saved state:
  - order: `performance-sidebar-order` as `id;id;…`;
  - per-device switches: `performance-sidebar-device-overrides` as `id:show;id:hide`.
- A per-device setting wins over the type setting (`summary_graph.rs:87-126`).
- The old key `performance-sidebar-hidden-graphs` is migrated on startup (`mod.rs:1934-1947`).

**Which network interfaces count as which type** (used by the show-network settings; `summary_graph.rs:41-66`)
- Wired → wired
- Wireless → wireless
- Vpn → vpn
- Docker, Multipass, Bridge, Virtual → virtual
- Bluetooth, InfiniBand, Wwan, Other → other

**Device detail area**
- One `gtk::Stack` with a crossfade. **Only one device is visible at a time** (`page.blp`; `mod.rs:212`).
- The right-hand info panel is an `OverlaySplitView` on the end side, 240 to 270 px wide.
- The last selected device is saved in `performance-selected-page` (`mod.rs:214-221, 1917-1925`). The restore actually picks the first device of the same type, not necessarily the saved one [inferred].

**Summary mode** (Ctrl+M, or "Graph Summary View" in the context menu)
- Hides the header bar, the bottom bar, the left sidebar and the info panel, leaving only the graphs.
- Blocks tab switching and the sidebar toggle (`window.blp:200`; `window.rs:1207-1209`, `585-622`; `mod.rs:545-566`).

**Context menu on every device page** (right-click, `GestureClick` button 3; e.g. `cpu.rs:460-479`)
- Page-specific items (listed per page below).
- Graph Summary View.
- View submenu: CPU, Memory, Drive, Network, GPU, Fan, Battery. This jumps to the first visible device of that type.
- Copy.

**Copy** (Ctrl+C or the menu)
- Puts a plain-text block on the clipboard (`data_summary()` on each page).
- Format: the device heading, then the model, then aligned "Label: value" lines indented by 4 spaces (e.g. `cpu.rs:798-873`).

**Time window label**
- "Utilization over N seconds/minutes".
- Duration = data points × update interval (`cpu.rs:1305-1317`; `main.rs:201-222`).

**Graph engine** (`widgets/graph_widget.rs`, `graph_widget_utils.rs`)
- Drawn with a GSK snapshot (hardware accelerated, no cairo).
- Rounded clip of radius 7 and a 1 px outline in the device colour.
- Grid lines in the device colour at 20% alpha:
  - vertical count = round(width / 60);
  - horizontal count = round(height / 100) × 2 − 1;
  - the vertical lines **scroll** with the data, driven by a global tick counter.
- History: up to 600 points per series, NaN breaks the line, the newest point is at the right.
- Fill: base colour at alpha 100/255, with vertical edges stroked. Lines are 1 px; secondary series are dashed (5,5).
- Scaling modes: Fixed, ScaleUp/Down/UpDown, StickyUp/Down/UpDown, StickyUpDownEqualMagnitude, Stacking.
- Rounding of the max: Pow2, Pow2Base10, Integer, Pow10.
- Datasets can be linked so they share one max.
- Smooth graphs: cubic Bézier with horizontal tangents.
- Sliding graphs: a 50 ms timer interpolates `progress` between refreshes and shifts a cached render node (`window.rs:1128-1147`).
- No tooltips, hover readouts or log scale.

**Colours** (`mod.rs:87-100`)

| Device | Hex |
|---|---|
| CPU | #1C71D8 |
| Memory | #62A0EA |
| Disk | #26A269 |
| Network | #DC8ADD |
| Fan | #813D9C |
| GPU | #F66151 |
| Battery | #57E389 |

- There is one colour per device type. Two disks share the same colour.
- Legend icons are `resources/line-{solid,dashed}-{disk,gpu,mem,net}.svg`.

**Units** (`main.rs:303-438`)
- K, M, G… with 2 decimals below 10, 1 below 100, else 0.
- Binary units add "i" (KiB).
- Bits are shown as bytes × 8 with "ps".
- Hz, W, V and Wh have their own formatting.

### 2.1 CPU (`resources/ui/performance_page/cpu.blp`, `cpu_details.blp`, `src/performance_page/cpu.rs`)

**Header:** "CPU" and the CPU model, "Utilization over …", "100%".

**Top graph** (`performance-page-cpu-graph`, saved in `cpu.rs:123-167, 896-978`):

| Value | Menu item | What it shows |
|---|---|---|
| 1 | Overall Utilization | one graph: total % (filled) and kernel % (dashed, no fill, if kernel times are on) |
| 2 | Logical Processors | a grid with one graph per logical CPU, each with usage and dashed kernel; columns = a divisor near √n (`compute_column_count`, `cpu.rs:980-995`) |
| 3 | All Threads | all cores overlaid in one graph, opacity 1/n each |
| 4 | All Threads Stacked | cores stacked, max = 100 × n, filled |

**Bottom graph** (`performance-page-cpu-graph-bottom`):
- 0 = None, 1 = Power Draw (StickyUp, integer rounding), 2 = Clock Speed (StickyUp), 3 = Temperature (StickyUpDown, range 35 to 45 °C at start).
- Its label reads "… over N", and the max value is shown (`cpu.rs:997-1073, 1105-1128`).
- An option is greyed out when no data is available, with automatic fallback Power → Temperature → Clock (`cpu.rs:612-630`).

**Show Kernel Times** toggle: `performance-page-kernel-times`.

**Live stats panel**
- Utilization %.
- Speed. With the BogoMIPS fallback it shows "(?)" with a tooltip explaining why (`cpu.rs:499-545`).
- Power Draw, hidden if not available.
- Processes, Threads, Handles.
- Up time as DD:HH:MM:SS.
- Temperature in °C, hidden if not available.

**Static stats panel**
- Base Speed, Sockets, Virtual processors, Virtualization ("Unsupported" when absent), Virtual machine (Yes/No).
- L1, L2, L3 cache. L4 is collected but not shown (`cpu.rs:609-613`).
- Cpufreq driver, Cpufreq governor, Power preference (EPP). Each is hidden when missing.

### 2.2 Memory (`memory.blp`, `memory_details.blp`, `memory_info_tooltip.blp`, `memory.rs`)

**Header:** "Memory" and the total.

**Main graph:** "Memory usage over …", max = total, with three series (`memory.rs:402-406, 691-702`):
- committed (dashed);
- dirty (no fill);
- used (filled), where used = total − available (free if available > total).

**Swap graph:** "Swap usage over …", max = swap total. Toggled by `performance-page-memory-swap-visible`; the toggle is disabled when there is no swap.

**Memory composition bar** (`mem_composition_widget.rs`)
- Segments: In Use, Modified (dirty), Standby (total − used − free), Free.
- Hover tooltip per segment.
- Toggled by `performance-page-memory-composition-visible`.
- A separate tooltip on the panel lists In Use, Modified, Standby, Free.

**Stats panel**
- In use, Available, Committed, Cached, Swap used, Swap available.
- Compressed and Savings (zswap plus zram). Shown only when at least 1 MiB is compressed (`memory.rs:478-512`).
- Speed (MT/s), Slots used ("N of M"), Form factor, Type.
- A toast appears if module info fails to load.

**Context menu:** Show Memory Composition, Show Swap Usage.

### 2.3 Disk, one page per disk (`disk.blp`, `disk_details.blp`, `disk.rs`, SMART and eject widgets)

**Header**
- "Disk N (id)" and the model.
- **Show SMART Data** and **Eject Disk** buttons. Eject only appears for ejectable drives, and this updates live (`disk.rs:238-300, 433-438`).

**Graphs**
- "Active time over …", 0 to 100% (filled).
- "Throughput over …": read solid and write dashed. ScaleUp with Pow2 rounding; the max label follows the units setting (`disk.rs:516-533`).

**Stats panel**
- Read speed, Write speed, Total Read, Total Written.
- Active time %, Avg. response time (ms).
- Capacity, Formatted, System disk (Yes/No).
- Type: HDD, SSD, NVMe, eMMC, SD, iSCSI, Optical, Floppy, Thumb Drive.
- WWN, Serial, Rotation Rate (RPM). Each is hidden when missing.

**Partitions list** (`partition_usage_item.rs`)
- Per partition: device name, filesystem type, used and total, usage bar, %.
- Up to 3 mount points are listed with an icon; the rest go into a tooltip ("Additional mount points:").

**SMART dialog** (`smart_data_dialog.rs`, `disk_smart_data_dialog.blp`)
- Header fields: Last Updated ("… ago"), Powered On, Drive Health, SMART Test Status.
- ATA table columns: ID, Attribute, Value, Normalized, Threshold, Worst, Type (Pre-Fail or Old-Age), Updates (Online or Offline), Assessment (FAILING, Failed in the past, Ok).
- NVMe rows: Percent Used, Available Spare, Spare Threshold, Total Data Read, Total Data Written, Warning Temp Time, Critical Temp Time, Control Busy Time, Warning Temperature, Critical Temperature, Temps, Unsafe Shutdowns, Media Errors, Number Error Log Entries, Power Cycles.
- Failure dialog text: "Failed To Get SMART Data".

**Eject failure dialog** (`eject_failure_dialog.rs`)
- Lists the processes blocking the eject, grouped by app, with PID, open files and a **Kill** button per row.
- Responses: Close, Retry, **Kill All**.

**Copy text:** capacity, formatted, system disk, type, read speed, total read, write speed, total written, active time, average response time.

### 2.4 Network, one page per interface (`network.blp`, `network_details.blp`, `network.rs`)

**Header:** connection type and the adapter name (from the hardware database).

**Graph:** "Throughput over …", send dashed and receive filled, linked to one max.
- Dynamic scaling on (default): ScaleUp, Pow2 rounding (Pow2Base10 for base 10).
- Dynamic scaling off: fixed at the link speed (`network.rs:840-895`).

**Stats panel**
- Receive, Send, Total Received, Total Sent.
- Interface name, Connection type.
- Connection status: Unknown, Failed, Disconnected, Disconnecting, Connected, Connecting, Configuring, Waiting for auth, Unavailable, Loading.
- Wi-Fi only: SSID, Signal strength (icon in 4 levels at ≤25/50/75/100), Maximum Bitrate, Frequency.
- Hardware address, IPv4 address, IPv6 address.

**Context menu:** **Network Settings**. This opens GNOME Settings at the wifi or network panel over D-Bus (`org.gnome.Settings`, `org.freedesktop.Application.ActivateAction("launch-panel")`; `network.rs:160-230`).

### 2.5 GPU, one page per GPU (`gpu.blp`, `gpu_details.blp`, `gpu.rs`)

**Header:** "GPU N" and the device name.

**Top graph:** "Utilization over …", 0 to 100%.

**Middle and bottom graphs** are each chosen from the same list (`performance-page-gpu-graph-middle` default 1, `-bottom` default 2; `gpu.rs:46-60`):

| Value | Option | Notes |
|---|---|---|
| 0 | None | |
| 1 | Encode/Decode | two series, or one if encode and decode share hardware |
| 2 | Memory Usage | VRAM and GTT/shared; "Dedicated and shared memory usage", or total memory when GTT is missing |
| 3 | Power Draw | |
| 4 | Clock Speeds | GPU clock and memory clock |
| 5 | Temperature | |

Options with no data are disabled.

**Stats panel**
- Utilization.
- Clock Speed current/max, Power Draw current/limit, Memory Usage used/total, Shared Memory Usage used/total, Memory Speed current/max.
- Video encode, Video decode (or a combined "Video encode/decode").
- Temperature.
- OpenGL version (with an ES marker), Vulkan version.
- PCI Express speed ("PCIe Gen N xW"), Max PCI Express speed, PCI bus address.

**Copy text:** all of the above.

### 2.6 Fan, one page per hwmon fan (`fan.blp`, `fan_details.blp`, `fan.rs`)

**Header:** fan label or "Fan N", and the temperature sensor name.

**Graphs**
- "Fan speed over …": RPM, plus PWM as a dashed 0 to 1 line. StickyUp, or Fixed at `max_rpm` when known.
- "Temperature over …": StickyUpDown, range 35 to 45 °C at start. Hidden if there is no temperature sensor (`fan.rs:200-290, 484-500`).

**Stats:** Fan Speed (RPM), PWM Percentage, Current Temperature.

### 2.7 Battery (`battery.blp`, `battery_details.blp`, `battery.rs`)

**Header:** battery kind and vendor/model. Kinds are UPower device kinds: Battery, UPS, Mouse, Keyboard, Phone, Headset and others (`battery.rs:1054-1084`).

**Power graph**
- "Power Input" / "Power Output over …". Charging is shown positive and discharging negative, filled to zero, symmetric StickyUpDown, Pow10 rounding.
- For devices that do not supply system power, it shows Percentage instead (`battery.rs:359-380, 535-557`).

**History graph:** "Battery Charge over past week", 7 days from UPower history, with a dashed line connecting the points (`battery.rs:975-1035`).

**Stats panel**
- Percentage, Energy, Voltage, Power.
- State: Charging, Discharging, Empty, Full.
- "Time to Full" or "Time to Empty".
- Charge Cycles, Serial, Type, Powering System, Technology (Li-ion, LiPo, LiFePO4, Lead Acid, NiCd, NiMH), Capacity %.
- Energy Empty, Energy Full, Energy Full (design), Voltage min (design), Voltage max (design).
- Charge Threshold (Yes or Firmware), Charge Start Threshold, Charge End Threshold.

---

## 3. Apps page

**Table**
- One `Gtk.ColumnView` tree with two sections: **Apps** and **Processes** (`apps_page/mod.rs:85-94`; `table_view/table_view.blp`).
- Header text: "N Running Apps", "N Running Processes".
- Collapse All button.
- **Single selection only**; no multi-select.

**Columns**
- All visible by default.
- **No column chooser.** Columns can be reordered by dragging the header.
- Network is hidden automatically when nethogs is unavailable; Swap is hidden when there is no swap (`apps_page/mod.rs:271-281`).
- Each column header has two lines: the name and a system total (CPU %, memory used %, highest disk busy %, swap %, total process network, average GPU %, GPU memory %) (`table_view/mod.rs:852-948`).

| ID | Title | Format |
|---|---|---|
| name | Name | 24 px icon for apps, 16 px for processes; middle ellipsis |
| pid | PID | blank for app and service rows |
| cpu | CPU | integer % |
| memory | Memory | memory units |
| swap | Swap | memory units |
| drive | Drive | disk rate, read + write combined |
| network | Network | network rate |
| gpu | GPU | % |
| gpu_memory | GPU Memory | memory units |

**How apps are found** (app-rummage: `scratchpad/app-rummage/src/`)
1. **Installed apps:** `.desktop` files from `$XDG_DATA_DIRS` and `~/.local/share/applications`.
   - Skipped if `NoDisplay=true`, `Hidden=true` or `Type` is not Application.
   - Uses Name, Exec and Icon. The app ID is the file name without `.desktop`.
   - Re-scanned when inotify reports a change (`lib.rs:98-137`, `desktop.rs:78-120`).
   - `Exec` launcher prefixes `flatpak run`, `snap run` and `env` are stripped (`exec.rs`).
2. **Running apps from cgroups, preferred:**
   - Walks `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/app.slice/*.{scope,service,slice}`.
   - Parses `app-[<launcher>-]<id>-<rand>.scope`, `app-…@….service`, `flatpak-…` and `dbus-…`, with `\x2d` unescaping.
   - Snap units `snap.<a>.<b>-<uuid>.scope` map to ID `a_b`.
   - Collects PIDs from `cgroup.procs` recursively (`cgroup.rs:13-190`).
   - The parsing is a port of ksysguard's.
3. **Fallback by executable:** matches the process's canonical exe path against the desktop file's `Exec`.
   - An exceptions table maps helper binaries to their app: `firefox-bin`, `soffice.bin`, `gnome-terminal-server`, `chrome` and others (`matching.rs:10-24`).
   - When several apps share one binary (Chromium PWAs, custom launchers), argv tokens decide; true ties are dropped (`matching.rs:109-175`).
4. Only the current user's apps are shown.
5. Magpie ignores `guake-prefs` and the foot server/client (`PL:apps.rs:31-35`).
6. Icons are resolved by magpie: freedesktop icon lookup at 64 px forcing SVG, or the raw file for absolute paths (`PL:apps.rs:134-184`).

**Tree**
- The Processes section is the full ppid tree under init.
- Each app row's children are its "primary" processes: app PIDs whose parent is not an app PID. The same row objects are shared, so a full subtree shows under the app (`table_view/models.rs:313-317, 456-487`).
- Display name rules:
  - basename of exe;
  - else the first token of cmd, with a trailing ":" removed;
  - Wine processes use the last path part of `cmd[0]` (`models.rs:134-168`).
- An app's icon is inherited by its processes.
- App rows always show summed (merged) stats. Process rows show their own stats, or subtree stats when `apps-page-merged-process-stats` is on.

**Sorting**
- By any column.
- Sections stay grouped (in descending order the second section moves to the top).
- Re-sorted on every refresh.
- Saved only if `apps-page-remember-sorting` is on; there is no default sort (`table_view/settings.rs:55-101`).

**Filter**
- Matches name or PID only, case-insensitive.
- A row matches on substring either way, or when normalized Levenshtein distance ≤ 0.6.
- Section headers always show.
- Because app rows have PID 0, any query containing "0" matches every app [inferred quirk] (`table_view/mod.rs:604-638`).

**Actions** (`apps_page/actions.rs`; the backend side is in section 7)
- Stop (SIGTERM), Force Stop (SIGKILL).
- Send Signal submenu: Suspend (STOP), Continue (CONT), Hangup (HUP), Interrupt (INT), Terminate, Kill, User 1, User 2.
- Details.
- An app action signals each primary PID. If that process is `bwrap` (Flatpak sandbox), it goes down to the first process that is not bwrap.
- **No confirmation dialogs**, and errors are only logged.
- **Missing:** nice/priority, CPU affinity, I/O priority, end process tree, open file location, copy.
- Context menu: right-click or touch long-press.
- Action bar: Stop and Force Stop (linked), then Details. It shows for app and process rows.

**Keys**
- Right expands a row.
- Left collapses, or jumps to the parent.
- Enter or double-click toggles expand/collapse (`table_view/mod.rs:323-470`).

**Details dialog** (`process_details_dialog.rs`)
- Icon and name.
- PID or App ID, Type, Command Line.
- Resource Usage group: CPU, Memory, Swap, Drives, Network, GPU, GPU Memory, updating live.
- Bug: Swap binds to a property that does not exist, so it does not update [inferred].
- No user, start time, path or thread count; no copy buttons.

**Greying out zeros:** `apps-page-hide-is-zero` sets 0.3 opacity on zero values. There is **no colour heat map**.

---

## 4. Services page

**Sections:** User Services (systemd session bus) and System Services (system bus) (`services_page/mod.rs:181-190`).

**Header**
- "N Total Services" and a filter summary.
- Filter toggles: **Running, Failed, Stopped, Disabled**. They combine with OR, and the result is ANDed with search.
- Remove Filters button (`services_page/page.blp:133-211`; `mod.rs:349-364, 643-698`).

**Rows**
- Same 9 columns.
- Stats are the subtree of the service's main PID.
- One child row: the main process and its tree.
- The icon shows the state: running, failed, stopped (enabled but not running), disabled.
- Unit types listed: `.service`, `.socket` and `.mount`, excluding not-found units (`PL:services/systemd/mod.rs:325-511`).

**Actions**
- Start (when not running); Stop and Restart (when running).
- Details.
- The action bar has Start, Stop (linked), Restart, Details. Start, Stop and Restart are hidden under Snap (`service_action_bar.rs:66-70`).
- Enable and disable exist only as a switch in the details dialog.
- Process signals are available on the child process rows.
- No confirmation dialogs.

**Details dialog** (`service_details_dialog.rs`)
- State group:
  - Name.
  - Description: **always empty, never filled in**.
  - Status: Running, Failed or Stopped, updating live.
  - Enabled switch, which calls enable or disable. Bug: it resyncs on the wrong signal [inferred].
  - Start, Stop and Restart buttons in the group header.
- Process group: Process ID, User, Group, File Location.
- **Logs** expander: the last **5** journal messages for `UNIT=` or `_PID=` in the current boot, monospace, with a copy button.

**Privileges:** systemd calls send `AllowInteractiveAuth`, so polkit can prompt. OpenRC is a fallback, using `pkexec rc-service` and `rc-update`.

---

## 5. Preferences and every GSettings key

**Dialog** (`resources/ui/preferences/*.blp`): four pages.

**General**
- Update Interval: spin box, 0.5 to 10 s, step 0.05.

**Performance Page**
- Data Points slider: 10 to 600.
- Smooth Graphs, Sliding Graphs ("may negatively impact performance"), Scale Network Graphs Dynamically.
- Devices group:
  - Show Disk, GPU, Fan and Battery Information switches.
  - Show Network Information expander, with switches for Wired, Wireless, VPN, Virtual (Docker, Multipass, Bridge) and Other (Bluetooth, InfiniBand, WWAN).

**Units:** six toggle groups for Bytes/Bits and Base 10/Base 2, each for Memory, Drive and Network.

**Apps & Services:** Merge Process Stats, Remember Sorting, Remember Column Order, Scale CPU Usage to Core Count, Show Column Separators, Grey Out Zeroes.

**Keys** (`data/io.missioncenter.MissionCenter.gschema.xml`)

| Key | Type | Default | Effect / where used |
|---|---|---|---|
| window-width / window-height | i | 600 / 400 | saved window size (application.rs:116-149) |
| is-maximized | b | false | application.rs:155-166 |
| window-selected-page | s | "performance-page" | last tab |
| window-interface-style | enum system=0 / light=1 / dark=4 | system | theme |
| app-update-interval | i 10..200 | 20 | deprecated; still read at gpu.rs:1376 |
| app-update-interval-u64 | t 10..200 | 20 | refresh in 50 ms steps (20 = 1 s) |
| performance-page-data-points | i 10..600 | 60 | history points per graph |
| performance-page-memory2-use-bytes / -use-base2 | b | true / true | memory units (also Apps memory, swap, GPU memory) |
| performance-page-drive-use-bytes / -use-base2 | b | true / true | disk units |
| performance-page-network-use-bytes / -use-base2 | b | false / false | network shown in bits, base 10 |
| performance-page-network-dynamic-scaling | b | true | observed max (true) or link speed (false) |
| apps-page-merged-process-stats | b | false | process rows show subtree sums |
| apps-page-remember-sorting | b | false | also applies to Services |
| apps-page-remember-column-order | b | false | also applies to Services |
| apps-page-core-count-affects-percentages | b | false | when false, process CPU % is divided by core count (100% = whole machine) (magpie_client/client.rs:880-889) |
| apps-page-show-column-separators | b | false | |
| apps-page-hide-is-zero | b | false | zero values at 0.3 opacity |
| apps-page-sorting-column-name / -sorting-order / -column-order | s / enum / s | "" / None / "" | built from a page prefix (table_view/mod.rs:87-89) |
| services-page-sorting-column-name / -sorting-order / -column-order | s / enum / s | "" / None / "" | same |
| performance-page-cpu-graph | i 1..4 | 1 | overall, per-CPU grid, threads overlaid, stacked |
| performance-page-cpu-graph-bottom | i 0..3 | 1 | none, power, clock, temperature |
| performance-page-kernel-times | b | false | dashed kernel series |
| performance-selected-page | s | "cpu" | last device |
| performance-smooth-graphs | b | false | Bézier lines |
| performance-sliding-graphs | b | false | animated scroll |
| performance-show-cpu / -memory | b | true | **in the schema but never read** |
| performance-show-disks / -network / -gpus / -fans / -batteries | b | true | device types shown |
| performance-show-network-wired / -wireless / -vpn / -virtual / -other | b | true | network groups shown |
| performance-page-memory-composition-visible | b | true | composition bar |
| performance-page-memory-swap-visible | b | true | swap graph |
| performance-page-gpu-graph-middle / -bottom | i 0..5 | 1 / 2 | GPU graph choice |
| performance-sidebar-order | s | "" | `id;id` |
| performance-sidebar-hidden-graphs | s | "" | deprecated; migrated |
| performance-sidebar-device-overrides | s | "" | `id:show;id:hide` |
| first-time-running | b | true | first-run dialog |

---

## 6. Keyboard shortcuts

Sources: `resources/ui/keyboard_shortcuts.blp`, `window.rs:225-257, 574-663`, `application.rs:98, 450-451`.

**Global**

| Keys | Action |
|---|---|
| F10 | open menu |
| Ctrl+Q / Ctrl+W | quit / close window |
| Ctrl+, | preferences |
| Ctrl+? | shortcuts dialog |
| Alt or Ctrl+1 / 2 / 3 | Performance / Apps / Services (2 and 3 blocked in summary mode) |
| Ctrl+A | System Information |
| F9 or Ctrl+T | toggle sidebar |
| hold Ctrl | pause updates |

**Performance tab**

| Keys | Action |
|---|---|
| Ctrl+C | copy device info |
| Ctrl+M | summary mode |
| F1 to F8 | select sidebar row by raw index, counting hidden rows (bug) |

**Apps tab**

| Keys | Action |
|---|---|
| Ctrl+F | search |
| Ctrl+L | collapse all |
| Ctrl+E | Stop |
| Ctrl+X | Force Stop |
| Ctrl+I | Details |

**Services tab**

| Keys | Action |
|---|---|
| Ctrl+F | search |
| Ctrl+L | collapse all |
| Ctrl+S | start |
| Ctrl+E | stop |
| Ctrl+R | restart |
| Ctrl+I | details |
| Ctrl+X | Force Stop on process rows (also fires service stop) |

**In the table:** Left/Right collapse and expand, Enter toggles. There is no Menu key or Shift+F10 handler for the context menu.

---

## 7. Magpie data layer

### Process model
- Separate binary `missioncenter-magpie --addr <nng url>`. `--test` exits at once and is used as a probe.
- `PR_SET_PDEATHSIG=SIGKILL`, so magpie dies with the UI (`M:src/main.rs:369-422, 385-387`).
- Warm-up: synchronous refresh of CPU, memory, processes, batteries, disks, network and fans, with a 3 s deadline (`main.rs:426-461`).
- Threads inside magpie:
  - main NNG loop, one request at a time;
  - a tokio thread for zbus;
  - a rayon pool of 4 `data-cache-N` threads for refreshes;
  - forked children to probe GL and Vulkan;
  - a nethogs child process on a pseudo-terminal.
- Spawning (`MC/src/magpie_client/client.rs:168-242`):
  - native: from PATH;
  - Flatpak: `missioncenter-spawner` → `flatpak-spawn --watch-bus --host`, choosing between `-glibc` and `-musl` builds by probing, with `MC_MAGPIE_HW_DB` set and `LD_PRELOAD` cleared.
- Supervision: the client polls every 100 ms and respawns on exit with no backoff.
  - Initial connect: 50 tries, about 15 s, then an error dialog and exit.
  - Reconnect: 6 tries 100 ms apart (`client.rs:297-317, 525-605`).
- `MC_DEBUG_MAGPIE_PROCESS_SOCK` attaches to an existing magpie instead of spawning one.

### IPC
- NNG REQ/REP over `ipc://` (Unix socket): `/tmp/magpie_<8 random chars>.ipc`, or `$HOME/.var/app/io.missioncenter.MissionCenter/data/magpie.ipc` under Flatpak.
- One raw proto2 protobuf message per NNG message; protos are in `M:types/src/*.proto`.
- Requests (`ipc.proto:38`):
  - about
  - apps; app icons by ID list
  - cpu
  - disks: list, eject{id}, smart{id}
  - fans
  - gpus
  - memory: Memory or MemoryDevices
  - connections
  - processes: map, plus terminate, kill, interrupt, hangup, continue, suspend, user1, user2 {pids}
  - services: system list, user list, logs{id, pid}, start, stop, restart, enable, disable
  - battery
  - setup script: name, run, run_revert, open
- **Caching:** each reply is the last snapshot, and then an async refresh starts (one at a time per type). So every reply is one poll old, and rates use the real elapsed time (`M:platform/src/cpu.rs:42-91`).
  - Exceptions run synchronously: about, eject, SMART, logs, signals, the setup script.

### Sampling
- The UI client drives everything.
- Each loop requests processes, apps, disks, GPUs, CPU, memory, connections, fans, batteries, user services and system services. Memory devices are fetched only at startup.
- Readings are handed to GTK with `idle_add`, then the client sleeps `ticks × 50 ms` minus the time spent, in 10 slices, handling queued actions between slices (`MC/src/magpie_client/mod.rs:868-1120`).
- Magpie's internal cadences:
  - per-process GPU data on every 3rd GPU refresh;
  - the `/proc/*/maps` walk at most every 5 s;
  - CPU static info and memory devices once;
  - the systemd unit list only on `UnitFilesChanged` or `Reloading`;
  - battery sysfs rescan every 30 s when UPower is missing.

### Sources per metric

**CPU**
- `/proc/stat`:
  - idle = idle + iowait;
  - kernel = system + irq + softirq;
  - used = user + nice + steal;
  - guest columns ignored (`PL:cpu/ticks.rs:21-63`).
- Logical CPU count from the `cpu*` lines.
- Model name from sysinfo.
- Sockets: distinct `topology/physical_package_id`.
- Base speed: `cpufreq/base_frequency`, then `bios_limit`, then `acpi_cppc/nominal_freq`.
- Current speed: max of `policy*/scaling_cur_freq`, then `/proc/cpuinfo` MHz, then BogoMIPS (driver reported as "bogomips").
- Driver, governor and EPP from `cpu0/cpufreq/{scaling_driver, scaling_governor, energy_performance_preference}`.
- Virtualization: `vmx`/`svm` flags, `/dev/kvm`, `/proc/xen/capabilities`.
- Running in a VM: systemd `Manager.Virtualization` over D-Bus.
- Caches: `/sys/devices/system/node/node*/cpu*/cache/index*`, de-duplicated by `shared_cpu_list`.
- Temperature: hwmon `k10temp`, `coretemp` or `zenpower`, reading `temp1_input`.
- Processes: count of `/proc`. Threads: `/proc/*/task`. Handles: `/proc/sys/fs/file-nr` field 1, system-wide only.
- Uptime: `CLOCK_BOOTTIME`.
- Power: RAPL `/sys/class/powercap/*package*/energy_uj` delta, with wrap handled. Usually needs root, hence the udev rule in the setup script.
- Max/min frequency are not collected.

**Memory**
- `/proc/meminfo` (all fields).
- zram: `/sys/block/zram*/mm_stat`.
- ZFS ARC: `/proc/spl/kstat/zfs/arcstats` (size − c_min) is added to free and available.
- Modules: `udevadm info -q property -p /sys/devices/virtual/dmi/id` with `MEMORY_DEVICE_<n>_*` keys and `MEMORY_ARRAY_NUM_DEVICES`. No dmidecode.

**Disks**
- `/sys/block/*`, excluding loop, ram, zram, fd, md, dm and zd devices. Each disk needs a UDisks2 block object.
- From `/sys/block/<id>/stat`:
  - busy = Δio_ticks;
  - response time = Σ Δticks / Σ Δios;
  - rates = Δsectors × 512.
- Type from the name prefix, mmc type, UDisks2 `Drive.Media`, then `RotationRate`. There is no USB type.
- Model from sysfs vendor + model.
- From UDisks2: Block.Size, Ejectable, Serial, WWN, RotationRate.
- Temperature: drive hwmon, then `Ata.SmartTemperature`.
- Partitions: UDisks2 PartitionTable, following LUKS and dm holders, with `statfs` for usage.
- SMART: `Drive.Ata.SmartGetAttributes` and `NVMe.Controller.SmartGetAttributes`.
- Eject: `Filesystem.Unmount` without user interaction. On failure, blockers are found by scanning `/proc/*/cwd` and `/proc/*/fd`. Then `Drive.Eject` (`PL:disks/manager.rs:67-277`).

**Network**
- Interfaces from `if_nameindex`, excluding lo. An interface is listed only if NetworkManager knows it.
- NetworkManager Device: Udi, HwAddress, DeviceType, State.
- Kind falls back to name prefixes. The DeviceType read is probably broken (u64 vs u32), so the name rules are what actually run [inferred].
- Byte counters: `/sys/class/net/<if>/statistics/{rx,tx}_bytes`. Link speed: `/sys/class/net/<if>/speed`.
- IP addresses: NM `IP4Config` / `IP6Config` `AddressData`.
- Wi-Fi: NM `ActiveAccessPoint` → `{Ssid, Frequency, MaxBitrate, Strength}`.
- Adapter name: SQLite `hw.db`, built from systemd's pci/usb hwdb and matched by modalias prefix.
- Driver name is not collected.

**GPU**
- Bundled, patched **nvtop** with backends for Intel (i915/xe), AMDGPU, NVIDIA, V3D, MSM, Panfrost and Panthor.
- Underneath nvtop [inferred]: NVML via dlopen for NVIDIA; libdrm `amdgpu_query` plus hwmon for AMD; DRM ioctls plus hwmon for Intel.
- Values from nvtop: utilization, encoder/decoder, VRAM, clocks and their maximums, power and power limit, temperature, fan, PCIe.
- GTT: `/sys/bus/pci/devices/<pdev>/mem_info_gtt_*` (AMD).
- PCIe fallback: `current_link_speed`/`current_link_width` and `max_link_speed`/`max_link_width`.
- OpenGL version: EGL on GBM in a forked child.
- Vulkan version: ash `apiVersion` in a forked child.

**Fans**
- `/sys/class/hwmon/hwmon*/fan*_input`, with `fanN_label`, `fanN_max`, `pwmN`/255 and `tempN_input`.
- Per-file overrides can be placed in the config dir (`PL:fan/mod.rs`).

**Batteries**
- UPower `EnumerateDevices`, with DeviceAdded/Removed signals, device properties and `GetHistory("charge", 604800)`.
- sysfs `/sys/class/power_supply/*` as a fallback and for `capacity`, `cycle_count` and `charge_types`.

**Processes**
- `/proc/<pid>/stat`: comm, state, ppid, utime, stime.
- CPU % = Δjiffies / Δt, capped at ncpu × 100.
- cmdline and exe.
- Memory = `RssAnon + RssShmem + RssFile / share`, where share is a weighted harmonic mean of how many processes map each file (from `/proc/*/maps`). Falls back to statm.
- Swap = `VmSwap`.
- Disk I/O = Δ(read_bytes + write_bytes) from `/proc/<pid>/io`.
- Threads: count of `/proc/<pid>/task`.
- cgroups and per-process handle counts are not read (`PL:processes/`).

**Per-process GPU**
- nvtop `gpuinfo_refresh_processes`: DRM `/proc/<pid>/fdinfo` (`drm-engine-*`, `drm-memory-*`), de-duplicated with kcmp, or NVML for NVIDIA [inferred].
- Usage and memory are summed across all GPUs into each process.

**Per-process network**
- A long-running `nethogs -v 2 -C` child process on a 250×1000 pseudo-terminal, read through a vt100 parser plus a regex (screen scraping).
- Rate = Δ cumulative bytes / Δt.
- If nethogs is missing or not permitted, a permissions error is reported and the Network column is hidden (`PL:processes/mod.rs:126-268, 401-416`).

**Process actions**
- `kill(2)` with TERM, KILL, INT, USR1, USR2, HUP, CONT or STOP.
- On any failure, the batch is retried with `pkexec --disable-internal-agent kill -N pids` (`PL:processes/manager.rs:29-110`).
- No nice, affinity or ionice.

**Services**
- systemd over D-Bus: `ListUnitFiles` + `ListUnitsByNames` (fallback `ListUnits`), and `Subscribe`.
- Per-unit property streams: `ActiveState`, `MainPID`, `User`, `Group`.
- Actions:
  - `StartUnit(name, "fail")`
  - `StopUnit(name, "replace")`
  - `RestartUnit(name, "fail")`
  - `EnableUnitFiles` / `DisableUnitFiles` followed by `Reload`
- Logs: dlopen `libsystemd`, `sd_journal`, last 5 messages for the unit or PID in the current boot.
- OpenRC fallback via `librc`.

**About**
- `/etc/os-release` and its logo icon.
- `/proc/sys/kernel/*`.
- Package manager version (pacman, apt, dnf, zypper).
- `XDG_*` environment variables plus `<shell> --version`.
- Hostname.

**Setup script** (`M:platform-linux/bin/missioncenter-magpie-setup-linux`)
- Written to `/tmp/missioncenter-magpie-setup` and run with `pkexec`.
- What it does:
  1. Runs `setcap cap_net_admin,cap_net_raw,cap_dac_read_search,cap_sys_ptrace+pe` on nethogs.
  2. Adds the udev rule `/etc/udev/rules.d/99-powercap.rules` that makes RAPL `energy_uj` world-readable.
  3. Runs `sensors-detect --auto`.
- `--revert` undoes the capabilities and the rule.

**Environment variables:** `MC_MAGPIE_HW_DB`, `MC_MAGPIE_CONFIG_DIR`, `MC_MAGPIE_IS_FLATPAK`, `SNAP_CONTEXT`, `RUST_LOG`.

---

## 8. Linux features TaskSlinger (Windows) would not have

- **Packaging awareness:**
  - Flatpak host spawning;
  - unwrapping `bwrap` so app signals reach the real process;
  - Snap path translation and hidden service controls under Snap.
- **App detection from systemd/XDG cgroups** (`app-*.scope` and similar) and `.desktop` files, including Chromium PWA argv disambiguation.
- **Full POSIX signal menu:** STOP, CONT, HUP, INT, USR1, USR2, with a pkexec fallback for processes the user does not own.
- **systemd services:** user and system buses, socket and mount units, enable/disable through polkit, journal logs per unit or PID. OpenRC as a fallback.
- **Kernel CPU metrics:** cpufreq driver, governor and EPP; RAPL package power; kernel-time split from `/proc/stat`; BogoMIPS fallback.
- **Memory:** zram and zswap compression savings, ZFS ARC counted as free, PSS-style shared-memory estimate per process.
- **Disks:**
  - UDisks2 SMART for ATA and NVMe;
  - eject with a list of blocking processes and Kill/Kill All;
  - LUKS and device-mapper partition resolution;
  - mount point lists.
- **Network:**
  - NetworkManager state machine and VPN, Docker, Multipass and bridge grouping;
  - Wi-Fi details;
  - opens GNOME Settings panels.
- **GPU:**
  - DRM fdinfo per-process usage on AMD, Intel and ARM GPUs (Mali, Adreno, V3D);
  - GTT shared memory;
  - EGL and Vulkan version probing.
- **Sensors:** hwmon fans with PWM and paired temperature sensors, plus a user override directory.
- **Batteries:** all UPower device kinds (mice, keyboards, headsets and others), 7-day charge history, charge thresholds.
- **Elevation and setup:** one-click pkexec setup script (nethogs capabilities, RAPL udev rule, sensors-detect).
- **Interaction:** hold Ctrl to pause. TaskSlinger may have this too, since Mission Center copied it from Windows.

---

## UX problems and gaps found

1. **Only one device page is visible at a time.** The sidebar mini-graphs are 80×50 with no grid, axis or hover value, so a full view of everything at once is impossible. This is the user's main complaint; tasklens should show all devices together.
2. The Apps table has no column chooser, no multi-select, no confirmation dialogs, no user-visible errors, no priority or affinity, and no colour heat map.
3. Search matches only name and PID, and its loose matching (reverse containment, Levenshtein ≤ 0.6) gives false hits.
4. Bugs noticed:
   - the memory row is missing its "/" separator;
   - F1 to F8 count hidden rows;
   - devices plugged in later ignore visibility settings outside edit mode and land second-to-last;
   - the service Description is always empty;
   - Swap in the process details dialog never updates;
   - the Enabled switch resyncs on the wrong signal;
   - `performance-show-cpu` and `performance-show-memory` are never read;
   - the restored device is chosen by HashMap order, not the saved device.
5. All data is one poll old (cache-then-refresh), and the REQ/REP connection serializes every request.

**What the companion agents found that I did not re-check line by line**
- The line references for the Apps and Services tables, the sidebar edit mode and graph engine, and the magpie data sources came from three helper agents.
- I checked a sample myself: the window, settings, shortcuts, every device page, the setup script and app-rummage.

**Files that hold most of the evidence**
- `/home/oystein/work/tasklens-ref/mission-center/data/io.missioncenter.MissionCenter.gschema.xml`
- `/home/oystein/work/tasklens-ref/mission-center/src/window.rs`
- `/home/oystein/work/tasklens-ref/mission-center/src/performance_page/` (`mod.rs`, `cpu.rs`, `memory.rs`, `disk.rs`, `network.rs`, `gpu.rs`, `fan.rs`, `battery.rs`, `widgets/`)
- `/home/oystein/work/tasklens-ref/mission-center/src/table_view/`
- `/home/oystein/work/tasklens-ref/mission-center/src/magpie_client/`
- `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/magpie/`
- `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/app-rummage/`
