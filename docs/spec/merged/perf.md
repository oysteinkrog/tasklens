# PERF merged spec: Performance tab, graphs, minigraphs, tray, mini window, Information tab, additional setup

This file replaces `spec/features-perf.md` (v2) and `spec/features-perf-addendum.md` for implementation. It applies `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-103 and canonical tables a to d) and `docs/ARCHITECTURE.md` (ADR-001 to ADR-016), in that order of precedence.

How to read the "Was:" lines:
- `v2 PERF-0xx` is an entry in `spec/features-perf.md`.
- `add PERF-0xx` is a new entry in `spec/features-perf-addendum.md`. The addendum used first-draft numbers, so its IDs clash with v2 IDs; the prefix tells them apart.
- `add amend (<subject>)` is an addendum amendment that had no ID of its own. `add amend PERF-069` is the mini window amendment (first-draft number).
- `table b` is a row of the canonical keyboard table in `docs/RECONCILIATION.md`.

v2 also points at two IDs that do not exist in v2: its RAPL setup reference is now PERF-106 and its sensor detection reference is now PERF-108. Every ID outside "Was:" lines and the ID map names a merged entry: PERF in this file, PROC in `proc.md`, SYS and APP in `sys-app.md`.

Labels: "verified" means read in a document or checked on the machine. "Inferred" means this merge decided it from the precedence rules because no document settles it.

## Shared rules for every entry

- **Missing data is never zero.** Every value carries an ADR-003 `Validity`. Table cells, header totals and sidebar card readouts use the ADR-003 glyphs (`…`, lock icon, `n/a`, `-`, `!`). Performance stats grids, Information rows and other key/value fields use the words in PERF-013. Tooltips use the ADR-003 texts; the Restricted tooltip reads "Needs administrator access. Click the shield to grant it." from P3 and "Needs administrator access." before P3 (R-050).
- **TaskSlinger's layout wins.** Graphs sit on top and a three-column stats grid sits below them, as in `performance.png`. Mission Center's right-hand info panel is not built.
- **Number formats** follow canonical table (d) and live in `tl-model::units`. Percent has one decimal (`11.9%`, `0%` at measured zero). Sizes are IEC with two decimals (`34.68 GiB`). Frequencies are integer MHz. Temperatures are integer °C. Power has one decimal W. Network rates default to fixed Mbps with one decimal (R-013, R-015).
- **Settings and state keys** are the ones in canonical table (a). User choices go in `$XDG_CONFIG_HOME/tasklens/settings.toml`; layout and selection go in `$XDG_STATE_HOME/tasklens/state.toml` (R-018, R-019).
- **Sampling** follows ADR-008. A source runs only while some visible surface (page, sidebar card, minigraph, detached graph, tray icon, mini window) demands it. The stale threshold is `max(3 × period, 1.5 s)`. Suspend is detected when `Δboot − Δmono > 250 ms` or on logind `PrepareForSleep` (R-051).
- **Phases** are P0 to P4 per R-072. P1 is the Performance core and process table. P2 adds Services, Startup Apps, Connections, the KWin bridge, the 5 and 10 minute ranges and the Information tab. P3 adds the helper, history, tray, detached graphs, the mini window and the setup window.

## Sidebar and navigation

### PERF-001 Device sidebar cards
- Was: v2 PERF-001, add amend (sidebar device cards)
- Source: TS 2.3 "Left sidebar", crosscheck A.14, `performance.png` (verified) | MC 2.0 "What each row shows"
- Behaviour: One card per device. Each card has a mini graph (about 116×56 logical px, filled area plus top line, faint grid), the current value at top left over the graph, a heading and two detail lines. Clicking a card selects its page. The selected card has an accent outline.
  - CPU: heading "CPU"; line 1 the model; line 2 "4631 MHz · 52 °C".
  - Memory: heading "Memory"; line 1 "34.68 GiB / 63.16 GiB"; line 2 "Commit 69.0%".
  - GPU: heading "GPU N"; line 1 the name; line 2 "52 °C".
  - Disk: heading "Disk N"; line 1 "NVMe - /" (kind and primary mount); line 2 the model; temperature joins line 2 from P2 (PERF-053).
  - Network: heading is the interface name (`enp38s0`); line 1 the type ("Ethernet", "Wi-Fi", "Tunnel"); line 2 the rate in the accent colour: `↓ 1.2 Mbps ↑ 0.3 Mbps` when both directions are non-zero, else the total rate. The NetworkManager connection name goes in the card tooltip and on the page. A disconnected interface shows its state on line 2 and the card is drawn at 0.6 opacity. The interface name is the heading because the four i350 ports on this machine would all read "Ethernet".
  - Fan: heading "Fan N"; line 1 the label; line 2 "1200 RPM · 45 °C".
  - Battery: heading is the UPower kind ("Battery", "Mouse", "UPS"); line 1 the model; line 2 "85.0% · Discharging".
  - The page header uses the long form and model (PERF-008).
  - Default order: CPU, Memory, GPU, Disk, Network, Fans, Batteries. This follows TaskSlinger's `Columns.Performance=0,1,4,2,3`, with fans and batteries added at the end.
- Linux data/mechanism: Each card reads the newest sample of its device's main series from the snapshot. No extra collection. Missing values use the ADR-003 glyphs (R-050).
- Verdict: BUILD
- Phase: P1 (fan and battery cards arrive with their pages in P2)
- Privilege: none
- Applied: R-055, R-013, R-050

### PERF-002 Hide and show page context menu
- Was: v2 PERF-002
- Source: TS 2.3 (L28324-28326, L28650)
- Behaviour: Right-clicking a card shows "Select Pages" (opens edit mode, PERF-003), "Hide This Page" and "Show This Page". "Show This Page" appears only in edit mode, on hidden cards. Hiding the last visible page is refused with the toast "At least one performance page must stay visible". Hiding the selected page selects the next visible card.
- Linux data/mechanism: A hidden page is stored as `performance.device_overrides = { "<device key>" = "hide" }` (canonical table a; v2's `performance.hidden_pages` is not a key). Showing it again writes `"show"` when the type default is off, and removes the entry when the type default is on.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-019

### PERF-003 Sidebar edit mode
- Was: v2 PERF-003
- Source: MC 2.0 "Sidebar edit mode" (`summary_graph.rs:338-350`, `mod.rs:2907-2989`)
- Behaviour: Edit mode opens from "Select Pages" or from a pencil button at the top of the sidebar.
  - Each card gets a drag handle and an on/off switch. Hidden cards reappear with the switch off.
  - Cards are grouped under the type group names from PERF-008. An empty group shows its empty-state text.
  - Header buttons: "Enable All", "Disable All", "Reset to Default".
  - Drag-and-drop reorders cards inside the sidebar. A 3 px accent drop line shows the target.
  - "Done" leaves edit mode.
  - "Reset to Default" clears the custom order and every per-device override.
  - The at-least-one-visible rule from PERF-002 applies here too.
- Linux data/mechanism: Saves `performance.device_order = [device keys]` and `performance.device_overrides = { key = "show" | "hide" }` (canonical table a; v2's `page_order` is renamed). A per-device override wins over the type default (PERF-004).
- Verdict: BUILD. Mission Center's edit mode is merged into TaskSlinger's "Select Pages" item, because TaskSlinger has no reorder interface.
- Phase: P2
- Privilege: none
- Applied: R-019

### PERF-004 Per-type default visibility and network groups
- Was: v2 PERF-004
- Source: MC 5 "Devices" group, `summary_graph.rs:41-66`
- Behaviour: Settings > All settings > Performance > Devices has switches for Disks, GPUs, Fans and Batteries, and network sub-switches Wired, Wireless, VPN and tunnel, Virtual and Other.
  - Defaults: all on except Virtual.
  - Virtual covers `docker*`, `veth*`, `br-*`, `virbr*`, `vnet*` and Multipass bridges. `tailscale0` is a Tunnel and is shown.
  - Loopback (`lo`) is never shown.
  - CPU and Memory always exist and can only be hidden per page.
  - A device plugged in later follows these defaults unless it has an override. This fixes Mission Center's bug where late devices ignore visibility outside edit mode.
- Linux data/mechanism: Keys `performance.show_disks`, `show_gpus`, `show_fans`, `show_batteries`, `show_network_wired`, `show_network_wireless`, `show_network_vpn`, `show_network_virtual` (default false), `show_network_other`. Type classification is in PERF-054. ADR-016's `show_virtual_interfaces` key is replaced by these.
- Verdict: ADAPT. Virtual interfaces are hidden by default. Mission Center shows everything.
- Phase: P2 (the P1 default behaviour, virtual hidden and loopback never shown, is built with PERF-054)
- Privilege: none
- Applied: R-053

### PERF-005 Device hotplug and stable keys
- Was: v2 PERF-005
- Source: MC 2.0 (`mod.rs:2072-2657`) | architecture review section 5 "Device identity/topology"
- Behaviour: Disks, interfaces, GPUs, fans and batteries appear and disappear while tasklens runs. A new device takes its saved place in `device_order`, or else goes at the end of its type group. When the selected device vanishes, the next visible card is selected. Graph history never joins across device generations: a re-plugged device starts a new series after a visible gap.
- Linux data/mechanism:
  - **Persistent keys** (stored in settings and state):
    - `cpu` and `memory`;
    - `gpu:<NVML UUID>` for NVIDIA, else `gpu:pci:<domain:bus:dev.fn>`;
    - `disk:<udev ID_WWN or ID_SERIAL>`, falling back to the by-path name;
    - `net:<ifname>`;
    - `fan:<hwmon name>:<device path>:<n>`;
    - `bat:<UPower NativePath>`;
    - `aggregate:disk` and `aggregate:gpu` for the aggregate pages (PERF-009; inferred).
  - **Runtime identity** adds a generation: major:minor plus a lifecycle counter for block devices; netns plus ifindex plus a counter for interfaces.
  - **Discovery:** rescan every 5 s while any of disks, network or sensors is demanded (ADR-016). There is no `NETLINK_KOBJECT_UEVENT` socket in v1; it may come back in P4 as an optimisation bead. Batteries use UPower `DeviceAdded` and `DeviceRemoved` signals (PERF-078).
  - `performance.selected_device` in `state.toml` keeps the selected key across restarts.
- Verdict: BUILD
- Phase: P1 for disks and network; P2 for the rest.
- Privilege: none
- Applied: R-052

### PERF-006 Sidebar width splitter
- Was: v2 PERF-006
- Source: TS 2.3 `Performance.NavWidth=252`
- Behaviour: A draggable splitter between the sidebar and the page. Default 252 logical px, range 180 to 400. The width persists.
- Linux data/mechanism: `state.toml` `[performance] nav_width = 252` (180..=400).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-007 Performance keys, View submenu and sidebar toggle
- Was: v2 PERF-007, table b rows "F1..F8", "Up / Down (Performance sidebar focused)", "F9, Ctrl+T", "Ctrl+M", "Ctrl+C (Performance)"
- Source: MC 2.0 context menu "View" submenu, MC 6 "F1 to F8" | canonical table b
- Behaviour:
  - F1 to F8 select the 1st to 8th **visible** card. This fixes Mission Center's bug of counting hidden cards.
  - Up and Down move the selection when the sidebar has focus.
  - F9 and Ctrl+T show or hide the device sidebar. The page then fills the width. The state is not persisted; the sidebar shows at every start (R-090).
  - Ctrl+M toggles Graph Summary View (PERF-010). Ctrl+C copies device info (PERF-011).
  - Every page's context menu has "View >" with the type group names from PERF-008 (Processor, Memory, Disks, Graphics adapters, Network adapters, Fans, Batteries). Each jumps to the first visible device of that type. Types with no visible device are disabled.
- Linux data/mechanism: GPUI actions in the `Performance` key context.
- Verdict: BUILD
- Phase: P1 for F1 to F8, Up and Down, and F9 and Ctrl+T; P2 for the View submenu.
- Privilege: none

### PERF-008 Device type names, headers and empty states
- Was: add PERF-082
- Source: TS crosscheck A.14 (L28219-28232: "Processor", "Memory", "Disk %d"/"Disk %s", "Graphics adapter", "Adapters", "Aggregate", "No adapter", "Multiple", "Details")
- Behaviour: These TaskSlinger strings are not the card headings (PERF-001). tasklens uses them here:
  - **Type group names** in sidebar edit mode, the "View >" jump submenu and the Settings device switches: Processor, Memory, Disks, Graphics adapters, Network adapters, Fans, Batteries.
  - **Page headers**: "Processor" plus the CPU model; "Memory"; "Disk N" plus the model and the kernel name (`nvme0n1`); "Graphics adapter" plus the GPU name; the interface name plus the adapter name; "Fan N" plus the chip; the battery kind plus vendor and model.
  - **Aggregate pages**: headings "Aggregate storage" and "Aggregate graphics".
  - **Empty states**: "No graphics adapter found" and "No network adapter found" on an empty type group in edit mode.
  - **Multiple**: on an aggregate card or page when member values differ, for example a drive type mixing NVMe and SSD.
  - **Details**: the heading over the stats grid on every device page.
  - **Disk %s** is not used. Disks are numbered "Disk N" in sidebar order.
- Linux data/mechanism: UI strings only.
- Verdict: ADAPT (strings moved to Linux positions)
- Phase: P1 (aggregate strings with PERF-009 in P4; edit-mode strings with PERF-003 in P2)
- Privilege: none

### PERF-009 Aggregate pages
- Was: v2 PERF-008
- Source: TS 2.3 "Aggregate storage" (L28224), "Aggregate adapters" (L28228), card titles "Aggregate"/"Multiple" (A.14)
- Behaviour:
  - "Aggregate storage" shows active time (the busiest disk's %) and the sum of transfer rates over all physical disks.
  - "Aggregate graphics" shows GPU utilization (the highest engine across GPUs) and the VRAM sum.
  - Each page exists only when there are two or more devices of that type. Both are hidden by default and can be turned on in edit mode.
- Linux data/mechanism: Sums use whole disks only, never partitions or dm/md devices, so nothing is counted twice. GPU values come from PERF-063 and PERF-064. Visibility uses `device_overrides` with keys `aggregate:disk` and `aggregate:gpu` (PERF-005).
- Verdict: BUILD
- Phase: P4
- Privilege: none

### PERF-010 Graph summary mode
- Was: v2 PERF-009
- Source: MC 2.0 "Summary mode" (Ctrl+M, `window.rs:585-622`)
- Behaviour: Ctrl+M, or "Graph Summary View" in any page's context menu, hides the title bar menus, tab bar, sidebar and stats. The window then shows a tiled grid of the **main graph of every visible device**, each with its heading and current value. Esc or Ctrl+M leaves the mode. Tab switching is blocked while it is on.
- Linux data/mechanism: None beyond the existing series.
- Verdict: ADAPT. Mission Center shows only the selected page. tasklens tiles all devices, because "only one device visible at a time" is the main complaint about Mission Center (MC inventory, UX problem 1).
- Phase: P2
- Privilege: none

### PERF-011 Copy device info
- Was: v2 PERF-010
- Source: MC 2.0 "Copy" (`data_summary()`, for example `cpu.rs:798-873`) | TS 8 toast "Copied metrics to clipboard"
- Behaviour: Ctrl+C on the Performance tab, or "Copy" in a page's context menu, copies plain text. Line 1 is the page header, line 2 the model, then aligned `    Label: value` lines (4-space indent). Every stats-grid field of the page is included. Missing values print their PERF-013 word. Memory module serial numbers never appear (D-007). Toast: "Copied metrics to clipboard".
- Linux data/mechanism: GPUI `write_to_clipboard` (Wayland data device; U-33).
- Verdict: BUILD
- Phase: P2
- Privilege: none
- Applied: R-012, D-007

### PERF-012 Page layout
- Was: v2 PERF-011
- Source: TS `performance.png`, crosscheck A.19, A.20
- Behaviour: Each page has:
  - a header with the page header text (PERF-008) at left and page toggles at right;
  - the graph area;
  - a "Details" stats grid: label (secondary text) over value (primary text), three columns, rows in TaskSlinger order, with Mission Center fields TaskSlinger lacks appended after TaskSlinger's fields.
  - Rows a device cannot supply show the PERF-013 word, so the grid keeps a stable shape. The exception is the short list in PERF-013 of rows that stay hidden when missing.
  - Values are selectable text.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-050

### PERF-013 Fallback value words
- Was: add PERF-079
- Source: TS crosscheck A.20 (L28059-28085) | MC "Unknown", "N/A", "Unsupported" (`cpu.rs:552`, `gpu.rs:565`, `gpu.rs:584`)
- Behaviour: In stats grids, Information rows and other key/value fields, a value that is not valid shows a fixed word in secondary text colour, never a zero. A tooltip gives the reason and the source path or call.

  | Validity (ADR-003) | Text | Replaces |
  |---|---|---|
  | WarmingUp | `Collecting…` | MC shimmer placeholder |
  | Stale | the last value in secondary colour; tooltip "Last updated {age} ago" | |
  | Restricted | `Restricted`; tooltip per the shared rules | |
  | Unsupported, because setup is needed (for example RAPL not readable) | `Needs setup`; tooltip links to the setup window (PERF-105) | |
  | Unsupported by hardware or driver | `Not supported` | TS "Not supported", MC "Unsupported" |
  | Unsupported, source answers with no value (for example NVML memory temperature N/A) | `Not reported` | TS "Not reported", MC "N/A" |
  | Failed | `Unavailable` | TS "Unavailable" |
  | Missing (device gone) | the card or page is removed | |
  | enum or status unknown (operstate `unknown`, as on `tailscale0`) | `Unknown` | TS "Status not reported", "<Unknown>" |
  | no CPU model name | `Unknown processor`; on ARM, implementer and part from `/proc/cpuinfo` first | TS "Unknown CPU" |
  | no GPU name | `Unknown GPU` | TS "Unknown Adapter" |
  | no network adapter name | `Unknown adapter` | |
  | primary display | ` (primary)` after the display name | TS "Primary Display" |

  Rows that Mission Center hides when missing stay hidden in tasklens: CPU power when the machine has no RAPL zone at all, CPU temperature when no CPU hwmon chip exists, disk WWN, serial and rotation rate, and battery charge thresholds. When RAPL exists but is not readable, the CPU power row shows `Needs setup` (inferred: this keeps the setup path visible). TaskSlinger's "No root" string is not used.
- Linux data/mechanism: Each metric carries its ADR-003 validity. One formatter table in `tl-ui` maps validity to these words; table cells use the ADR-003 glyphs instead.
- Verdict: ADAPT (TaskSlinger strings mapped onto ADR-003 validity states)
- Phase: P1
- Privilege: none
- Applied: R-050

### PERF-014 Performance-tab toolbar contents
- Was: add PERF-093
- Source: TS crosscheck A.8 (`performance.png` against `processes.png`)
- Behaviour: On the Performance tab the title-bar toolbar shows only the elevation shield and the "+" window picker. The "Hide system processes" button and the view-mode cycler are removed (not greyed) on this tab. The Performance tab adds no toolbar buttons. Its page toggles (CPU mode button, "Kernel on"/"Kernel off") sit in the page header, as in `performance.png`. The chrome spec owns the toolbar buttons.
- Linux data/mechanism: None.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-031

## Graph behaviour

### PERF-015 Graph element
- Was: v2 PERF-012
- Source: TS 9 "Layout" (PLAN section 9) | MC 2.0 "Graph engine" | ADR-009 chart element
- Behaviour:
  - The newest sample is at the right edge.
  - The area is filled in the series fill colour, with a brighter top line.
  - A faint vertical grid scrolls with the data: one line every `range / 10`, placed from absolute time. Horizontal lines at 25, 50 and 75%.
  - A 1 px outline in the series colour at 40% alpha, with a 4 px corner radius when "Rounded Corners" is on (0 when off).
  - Heading at top left and current value at top right on page graphs and sidebar cards.
- Linux data/mechanism: ADR-009 `ChartView` and `ChartElement`, an app-owned GPUI element. Background and grid are quads. Fill and stroke are cached paths, rebuilt only when `ChartGeometryKey` changes (series versions, size, scale, y-range, style) and translated each paint. Points are straight segments. Each contiguous run of valid samples is its own subpath. When there are more samples than pixels, each pixel bucket keeps its min and max, with buckets aligned to absolute time. Stroke width is 1 px (ADR-009 rule 6; v2's 1.5 px is replaced). The CPU per-core grid is one `ChartGridElement` (PERF-028).
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-016 Time range
- Was: v2 PERF-013
- Source: TS 4.2 Time Range (30 s, 1 min, 5 min, 10 min) | MC "Data Points" 10 to 600 and the "Utilization over N" label
- Behaviour:
  - Choices: 30 seconds, 1 minute (default), 5 minutes, 10 minutes. There is no 15 s or 2 min choice.
  - The Performance tab has one range for all its pages: Settings > All settings > Performance > "Graph time range", also offered as "Time range >" in every page's context menu.
  - Each minigraph card and each tray graph has its own range (PERF-081, PERF-090).
  - The lower-left label reads "Utilization over 60 seconds". The words change per metric: "Throughput over", "Memory usage over", "Power draw over".
  - Mission Center's data-point slider is not built.
- Linux data/mechanism: `performance.graph_range_s = 60` (values 30, 60, 300, 600). Ranges up to 60 s read the fast tier; 5 and 10 minutes read the 1 Hz tier (PERF-017).
- Verdict: ADAPT. Fixed ranges replace data points, so the duration no longer depends on the refresh interval.
- Phase: P1 for 30 s and 1 min; P2 for 5 and 10 min.
- Privilege: none
- Applied: R-016

### PERF-017 Series rings and the 1 Hz tier
- Was: v2 PERF-014
- Source: architecture review section 4 "History structures" | ADR-003 ring rule
- Behaviour: The user sees no difference. Graphs show up to 10 minutes.
- Linux data/mechanism:
  - Fast tier: one `SeriesRing` per series at the source rate, capacity `ceil(60 s / period) + 2` (602 at 10 Hz, 62 at 1 Hz).
  - 1 Hz tier (from P2): a second ring of 602 samples (10 min + 2) per series. Each sample is the **per-second maximum** of the fast tier, the same rule ADR-012 uses for history. There is no per-sample min/max pair; `Sample` stays 16 bytes.
  - The engine owns both rings and publishes `SeriesView`s. Memory is about 1.5 MB for 80 series.
- Verdict: BUILD
- Phase: P1 for the fast tier; P2 for the 1 Hz tier.
- Privilege: none
- Applied: R-016

### PERF-018 Scaling modes
- Was: v2 PERF-015
- Source: MC 2.0 scaling modes | review "Units and graph ranges" | ADR-009 rule 9
- Behaviour:
  - **Percentages:** fixed 0 to 100.
  - **Memory, VRAM:** fixed 0 to total.
  - **Commit:** 0 to max(CommitLimit, peak in view).
  - **Rates (disk, network, GPU PCIe):** the scale rises at once and falls only after the visible maximum stays below half the scale for 10 s.
  - **Network fixed mode:** when "Scale network graphs dynamically" is off, the scale is the link speed. If the link speed is unknown, the graph falls back to dynamic.
  - **Temperature:** sticky up and down, starting at 35 to 45 °C.
  - **Power and clocks:** sticky up from 0. When a hardware maximum is known (GPU max clock, power limit, fan max RPM), the scale is fixed at that maximum (U-35).
  - **Stacked CPU:** 0 to 100 × n.
  - **Battery power:** symmetric around 0.
- Linux data/mechanism: Computed in the UI from the visible samples. Key `performance.network_dynamic_scale = true` (false means link speed).
- Verdict: ADAPT. Mission Center's nine modes collapse to these rules.
- Phase: P1
- Privilege: none

### PERF-019 Axis maximum rounding and label
- Was: v2 PERF-016
- Source: MC Pow2/Pow10 rounding, `disk.rs:516-533`
- Behaviour: Scales that are not fixed round up to 1, 2 or 5 × 10ⁿ for decimal units (network in bits), or to a power of two for IEC units (disk rates, network in bytes). The maximum shows at top right of large graphs, for example "100 Mbps" or "4 GiB/s". This rounding applies whatever the network auto-scale setting is.
- Linux data/mechanism: `tl-model::units`.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-015, table d

### PERF-020 Smooth (sliding) graphs
- Was: v2 PERF-017
- Source: TS `UI.SmoothGraphs=1` | MC "Sliding Graphs" (50 ms timer) | ADR-009 animation driver
- Behaviour: "Smooth Graphs" defaults to on. When on, graphs scroll continuously at the monitor's refresh rate (144 Hz here), drawn one sample period behind the newest data. When off, graphs jump one step per sample. Sliding stops while the window is hidden, and when the system reduce-motion preference is on. Numeric readouts always show the newest sample.
- Linux data/mechanism: Key `motion.smooth_graphs = true`. This key alone decides sliding; `motion.animations` does not. ADR-009 `AnimationDriver`, one per window, requests frames only while a visible chart is moving. Time mapping: `right_edge_ns = min(now_boot_ns() - period_ns, newest_sample_ns)`.
- Verdict: ADAPT. TaskSlinger's meaning and default are kept. Compositor-paced frames replace Mission Center's 50 ms timer.
- Phase: P1
- Privilege: none
- Applied: R-017

### PERF-021 Curved lines
- Was: v2 PERF-018
- Source: MC "Smooth Graphs" (cubic Bézier)
- Behaviour: "Curved lines" in Settings > All settings > Performance, off by default.
- Linux data/mechanism: Key `performance.curved_lines = false`. Monotone cubic (Fritsch-Carlson) curves through the samples. They never overshoot. When off, ADR-009's straight segments apply.
- Verdict: ADAPT. Monotone curves replace Mission Center's Bézier, which can overshoot.
- Phase: P4
- Privilege: none

### PERF-022 Gaps and stale data
- Was: v2 PERF-019
- Source: review "Data validity", `Sample.flags` | MC NaN line breaks | ADR-003 chart column
- Behaviour:
  - Invalid samples (suspend, collector restart, device re-plug, unsupported) break the line. The gap shows as empty space with a dotted baseline.
  - A stale series draws its line and fill at 50% alpha after the last fresh sample, then a gap. Its readout shows the last value in secondary colour with the tooltip "Last updated {age} ago".
  - After suspend and resume, a gap is inserted and counter baselines are reset.
- Linux data/mechanism: Stale when `now - t_last > max(3 × period, 1.5 s)`. Suspend when `(Δboot_ns - Δmono_ns) > 250 ms` between two samples, or on logind `PrepareForSleep`. The engine sets `GAP_BEFORE` on the next sample (ADR-008).
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-051

### PERF-023 Hover readout
- Was: v2 PERF-020
- Source: TS 4.2 "Hover shows a value marker" (CL 0.8.28), " avg %.1f%s ", " peak %.1f%s ", " %u samples" | MC has none
- Behaviour: On every large graph and minigraph, hovering shows a vertical marker and a dot on each series. A tooltip shows the time offset ("-12s"), each series value, and "avg", "peak" and "samples" for the visible window.
- Linux data/mechanism: The chart element gets a hitbox only when this tooltip is enabled (ADR-009 rule 1). Hit test runs only on hover.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-024 Graph colours
- Was: v2 PERF-021
- Source: TS PLAN section 9 graph colours, TS 6 "Graph accents" | MC per-type colours | APP-039
- Behaviour:
  - Per type: CPU #0D546D fill / #0A7096 line; kernel line #C1514E; Memory #7E6A9A; GPU #508A59 / #71B768; Disk #D9B567; Network #359CAB.
  - Fans use the "Violet" accent and batteries the "Mint" accent (tasklens's choice; TaskSlinger has no colour for them).
  - Two devices of the same type share a colour.
  - Minigraph and tray graph cards can be recoloured with "Graph Color >": "Accent" plus the 17 named accents of APP-039 (Coral to Slate).
  - Themes override the graph colours through `TlTheme.graph`.
- Linux data/mechanism: Theme `GraphPalette` (ADR-009). Per-card colour in `state.toml` `[[minigraphs]] color` and `[[tray_graphs]] color` (`accent` or one of the 17 names).
- Verdict: BUILD
- Phase: P1 for the palette; P3 for per-card colour.
- Privilege: none

### PERF-025 Units and number formats
- Was: v2 PERF-022
- Source: TS "Network transfer unit", "Auto-scale network transfer rates" (INI `NetworkRateAutoScale=0`, verified) | MC 5 "Units"
- Behaviour:
  - Network unit: "Bits per second" (default) or "Bytes per second", in Settings > Network transfer unit and on the network page's Display unit row (PERF-057). Bits are SI, base 1000 (Kbps, Mbps, Gbps). Bytes are IEC (KiB/s, MiB/s).
  - "Auto-scale network transfer rates" defaults to **off**. Off: values are fixed Mbps (or MiB/s) with one decimal, and "0 Mbps" at measured zero. On: the unit steps through Kbps, Mbps and Gbps by size.
  - Memory and storage are always IEC with two decimals. There are no base-10 options for memory or disk.
  - Disk rates on graphs and stats use an automatic IEC unit with one decimal.
  - All other formats follow canonical table (d).
  - Rates always carry "/s" or "bps".
- Linux data/mechanism: `[units] network = "bits"`, `network_auto_scale = false`. Formatting lives in `tl-model::units`.
- Verdict: ADAPT. Mission Center's per-class base options are dropped; TaskSlinger's default wins.
- Phase: P1 for formats and menu items; P2 for the settings page.
- Privilege: none
- Applied: R-013, R-014, R-015, table d

### PERF-026 Legends and secondary series
- Was: v2 PERF-023
- Source: TS legend in the first cell ("Total", "Kernel", crosscheck A.28), network "Upload" legend (A.15) | MC dashed secondary series
- Behaviour: Legends are drawn inside the first graph of a page, top left, as coloured line swatches with labels:
  - "Total" and "Kernel" (CPU);
  - "Download" and "Upload" (network; PERF-055);
  - "Read" and "Write" (disk; write dashed);
  - "Dedicated" and "Shared" (GPU memory; shared dashed);
  - "Encode" and "Decode";
  - "GPU clock" and "Memory clock";
  - "RPM" and "PWM";
  - "Used" and "Committed" (memory; committed dashed; the dirty series is a thin line without fill and no legend).
- Linux data/mechanism: Dash pattern 5/5 px through `PathBuilder::dash_array`.
- Verdict: BUILD
- Phase: P1
- Privilege: none

## CPU page

### PERF-027 CPU header and graph mode
- Was: v2 PERF-024
- Source: TS "Overall"/"Per core" toggle (L28233-28236) | MC `performance-page-cpu-graph` 1 to 4
- Behaviour: The header shows "Processor" and the CPU name. At right is a mode button with the choices "Overall", "Per core" (default), "All cores overlaid" and "All cores stacked". A "Kernel on"/"Kernel off" toggle sits next to it (default on). Both persist.
- Linux data/mechanism: Keys `performance.cpu_graph_mode = "per_core"` (`overall | per_core | all_cores_overlaid | all_cores_stacked`) and `performance.cpu_kernel = true`. Data from `/proc/stat` (PERF-033).
- Verdict: ADAPT. TaskSlinger's two-state toggle becomes a four-choice button so Mission Center's two extra modes fit.
- Phase: P1 for Overall and Per core; P2 for the other two.
- Privilege: none
- Applied: R-019

### PERF-028 Per-core grid
- Was: v2 PERF-025
- Source: TS grid of "CPU %d" charts | MC `compute_column_count`
- Behaviour:
  - One chart per logical CPU, titled "CPU N", with the percent at top right. The first cell carries the "Total"/"Kernel" legend.
  - Column count is ceil(√(2n)): 16 CPUs give 6 columns (matches TaskSlinger) and 32 give 8 columns in 4 rows. Cells are at least 120 logical px wide; the count drops when needed.
  - Hovering a cell's title shows "Core C, thread T, 4200 MHz".
  - Offline CPUs show "Offline" in place of the chart.
  - This is the 32-chart frame-budget case for phase 0 (ADR-014).
- Linux data/mechanism:
  - `/proc/stat` `cpuN` lines, at 10 Hz while a CPU chart is visible and moving, else 1 Hz. Math per ADR-008: `busy = user + nice + system + irq + softirq + steal`, `total = busy + idle + iowait`, `kernel = system + irq + softirq`; guest fields ignored.
  - Core and thread from `/sys/devices/system/cpu/cpuN/topology/{core_id,thread_siblings_list}`.
  - Clock from `cpufreq/policyN/cpuinfo_avg_freq`, else `scaling_cur_freq` (U-28).
  - All cells are drawn by one `ChartGridElement`: one fill path, one total-line path, one kernel-line path (ADR-009 rule 7).
  - A change in the set of `cpuN` lines re-enumerates CPUs.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-013

### PERF-029 Kernel time overlay
- Was: v2 PERF-026
- Source: TS "Kernel on" red line | MC "Show Kernel Times" dashed
- Behaviour: When on, each per-core and overall chart adds the kernel percent as a solid #C1514E line without fill (TaskSlinger style). Overlaid and stacked modes show no kernel line.
- Linux data/mechanism: Δ(system + irq + softirq) / Δtotal.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-030 Overlaid and stacked modes
- Was: v2 PERF-027
- Source: MC 2.1 values 3 and 4
- Behaviour: "All cores overlaid" draws one large chart with each core's line at opacity 1/n, minimum 0.08. "All cores stacked" draws stacked filled bands with a maximum of 100 × n%; the axis label then reads, for example, "3200%".
- Linux data/mechanism: Same per-CPU series.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-031 Core type marks
- Was: v2 PERF-028
- Source: TS "Performance core"/"Efficiency core" icons (L28239-28240)
- Behaviour:
  - On hybrid CPUs, each cell shows a small P or E icon.
  - On AMD with preferred-core ranking, the highest-ranked cores show a small star; the tooltip reads "Preferred core (rank 236)". Stars show only when the ranking differs between cores.
  - Otherwise there is no icon.
- Linux data/mechanism:
  - Intel: `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus`.
  - AMD Zen 5c hybrids: distinct `cpu_capacity` values.
  - Preferred cores: `cpufreq/policyN/amd_pstate_prefcore_ranking` (reported present here; U-28).
- Verdict: ADAPT. The marks come from Linux topology files. The preferred-core star is new.
- Phase: P2
- Privilege: none

### PERF-032 CPU secondary graph
- Was: v2 PERF-029
- Source: MC `performance-page-cpu-graph-bottom` (power, clock, temperature)
- Behaviour: Under the chart area sits one wide graph with a choice in its context menu: "None", "Power draw" (W, default), "Clock speed" (MHz, average plus a dashed maximum-core line) or "Temperature" (°C). Choices without data are disabled. When power is not available, the graph shows Temperature, then Clock speed, without changing the saved choice. Label: "Power draw over 60 seconds", with the axis maximum at top right.
- Linux data/mechanism: Key `performance.cpu_secondary_graph = "power"` (`none | power | clock | temperature`). Data from PERF-038, PERF-039 and PERF-040 at 1 Hz.
- Verdict: BUILD. TaskSlinger has no such graph.
- Phase: P2
- Privilege: Power needs RAPL to be readable (PERF-106). Until then the "Power draw" choice is disabled with the tooltip "Needs setup", which links to the setup window.
- Applied: R-013, R-019

### PERF-033 CPU live stats
- Was: v2 PERF-030
- Source: TS stats rows (L28241-28261) | MC "Live stats panel"
- Behaviour: Grid rows, in order:
  - Utilization, Kernel time, Speed;
  - Processes, Threads, Handles;
  - Up time, Base speed, Sockets;
  - then Mission Center additions: Power draw, Temperature, Load average (1/5/15).
- Linux data/mechanism:
  - **Utilization:** `/proc/stat` `cpu` line (ADR-008 math).
  - **Speed:** mean of `cpuinfo_avg_freq` over all policies, else `scaling_cur_freq`, else `/proc/cpuinfo` MHz. The tooltip gives the fastest core. When only the BogoMIPS value exists, the value shows "(?)" with Mission Center's tooltip.
  - **Processes:** count of numeric `/proc` entries. **Threads:** the sum from the process sweep, or the `procs_running` and `processes` fields of `/proc/stat` when no sweep runs.
  - **Handles:** field 1 of `/proc/sys/fs/file-nr` (open files system-wide). Label stays "Handles"; tooltip "Open file descriptors, system-wide" (inferred from R-007's rule for the process column).
  - **Up time:** PERF-034.
  - **Base speed:** `cpufreq/base_frequency`, then `acpi_cppc/nominal_freq` (amd-pstate), then `bios_limit`.
  - **Sockets:** distinct `physical_package_id`.
  - **Load average:** `/proc/loadavg`.
- Verdict: ADAPT. Handles become open files. Load average is added.
- Phase: P1 (Power draw row with PERF-038 in P2)
- Privilege: none
- Applied: R-007 (label rule), R-013

### PERF-034 Up time format
- Was: add PERF-080
- Source: TS crosscheck A.19 (L29523, L28073-28074; `performance.png` shows "2 days, 2h 38m 15s") | MC DD:HH:MM:SS (`cpu.rs:720`)
- Behaviour: TaskSlinger's format wins.
  - With days: `1 day, 2h 38m 15s` or `2 days, 2h 38m 15s`.
  - Under one day: `2h 38m 15s`.
  - Under one hour: `0h 38m 15s` (hours are always shown).

  The same format is used on the CPU page and for Information > System > System uptime. Boot time uses `YYYY-MM-DD HH:MM:SS` in local time.
- Linux data/mechanism: `clock_gettime(CLOCK_BOOTTIME)`, which counts time in suspend, like `uptime`. Boot time = now (`CLOCK_REALTIME`) − `CLOCK_BOOTTIME`. `btime` in `/proc/stat` is the cross-check.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: table d

### PERF-035 CPU static stats
- Was: v2 PERF-031
- Source: TS Cores, Logical processors, L1/L2/L3 | MC Cpufreq driver, governor, EPP
- Behaviour: Rows: Cores, Logical processors, Virtualization, Running in VM, Security features, Hypervisor, L1 cache, L2 cache, L3 cache, Cpufreq driver, Governor, Power preference (EPP), Platform profile, Microcode.
- Linux data/mechanism:
  - Cores: distinct (package, core_id).
  - Caches: PERF-036.
  - `cpu0/cpufreq/{scaling_driver,scaling_governor,energy_performance_preference}`: here "amd-pstate-epp", "powersave", "balance_performance".
  - `/sys/firmware/acpi/platform_profile`, else power-profiles-daemon `ActiveProfile` over D-Bus.
  - Microcode from `/proc/cpuinfo`.
  - Virtualization, Running in VM, Security features and Hypervisor: PERF-037.
- Verdict: BUILD
- Phase: P1 for cores, logical processors and caches; P2 for the rest.
- Privilege: none

### PERF-036 Cache size format
- Was: add PERF-081
- Source: TS "%ld KB" (L35646; `performance.png` shows "512 KB", "8192 KB", "98304 KB") | MC binary units (`main.rs:303-438`)
- Behaviour: Cache sizes use IEC units. Up to 1023 KiB the value is an integer in KiB ("512 KiB"). From 1 MiB it is an integer in MiB when exact ("8 MiB", "96 MiB"), else one decimal. The tooltip gives the exact KiB count ("98 304 KiB", thin-space grouped). Each level shows the total across the CPU, with the instance count in the tooltip ("2 × 32 MiB"). The page shows L1 (data plus instruction), L2 and L3. L4, when present, shows only in the L3 tooltip.
- Linux data/mechanism: `/sys/devices/system/cpu/cpu*/cache/index*/{level,type,size,shared_cpu_list}`, de-duplicated by `shared_cpu_list`.
- Verdict: ADAPT ("KB" becomes "KiB"/"MiB")
- Phase: P1
- Privilege: none
- Applied: table d

### PERF-037 Virtualization and security rows
- Was: v2 PERF-032, add amend (CPU page, Security services row)
- Source: TS Virtualization, Virtualization-based security, Security services, Hypervisor platform (L28256-28258, crosscheck A.18) | MC Virtualization, Virtual machine
- Behaviour:
  - **Virtualization:** "Enabled (KVM available)", "Supported, KVM module not loaded" or "Not supported".
  - **Running in VM:** "No", or the hypervisor name. Replaces "Virtualization-based security".
  - **Security features:** a comma-separated list of the active features, in this order: Secure Boot, Kernel lockdown (integrity or confidentiality), Memory encryption (SME), User shadow stack, IOMMU. "None" when none is active. The tooltip summarises CPU vulnerability status, for example "12 mitigated, 0 vulnerable, 3 not affected", and names any source it could not read. Replaces TaskSlinger's "Security services" (Credential Guard, HVCI and the rest have no Linux meaning).
  - **Hypervisor:** "None", or a vendor.
- Linux data/mechanism:

  | Field | Source |
  |---|---|
  | Virtualization | `svm`/`vmx` in `/proc/cpuinfo` flags plus `/dev/kvm` |
  | Running in VM | systemd `org.freedesktop.systemd1.Manager.Virtualization` |
  | Hypervisor | the `hypervisor` cpuinfo flag plus `/sys/hypervisor/type` |
  | Secure Boot | efivarfs `SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c`, byte 4 |
  | Kernel lockdown | `/sys/kernel/security/lockdown`, the bracketed value |
  | Memory encryption | the `sme` flag in `/proc/cpuinfo`, plus `mem_encrypt` on the kernel command line |
  | User shadow stack | the `user_shstk` flag |
  | IOMMU | a non-empty `/sys/class/iommu` |
  | Vulnerabilities | `/sys/devices/system/cpu/vulnerabilities/*` |

  The addendum wrote the Secure Boot GUID as `8be4df61-93ca-11d2-aa0d-e0c98e1ccb4a`; the EFI global variable GUID is `8be4df61-93ca-11d2-aa0d-00e098032b8c` (v2 PERF-032 and v2 PERF-090 use it), so that form is used. Readability of lockdown, efivars and iommu without root is unverified (U-27). A source that cannot be read is left out of the list and named in the tooltip.
- Verdict: ADAPT. Windows security rows carry the nearest Linux facts.
- Phase: P2
- Privilege: none
- Applied: R-056

### PERF-038 CPU power (RAPL)
- Was: v2 PERF-033
- Source: MC "Power: RAPL"
- Behaviour: "Power draw" in watts (one decimal) on the stats grid and in the secondary graph. The tooltip splits Package and Core.
- Linux data/mechanism:
  - `/sys/class/powercap/intel-rapl:0/energy_uj` (package-0) and `intel-rapl:0:0` (core). The AMD driver also uses the `intel-rapl` names.
  - Rate = Δenergy_uj / Δt. Counter wrap uses `max_energy_range_uj`.
  - Polled at 1 Hz while demanded. The file is kept open and read with `pread`.
- Verdict: BUILD
- Phase: P2
- Privilege: The kernel default is mode 0400 (since the PLATYPUS fix, CVE-2020-8694). It is readable here only because Mission Center's `/etc/udev/rules.d/99-powercap.rules` is installed (verified). When not readable the row shows `Needs setup`, linking to PERF-105. When no RAPL zone exists the row is hidden (PERF-013).
- Applied: R-050, R-029

### PERF-039 CPU temperature
- Was: v2 PERF-034
- Source: MC "Temperature: hwmon k10temp, coretemp or zenpower"
- Behaviour: "Temperature" in integer °C on the stats grid, the CPU card and the secondary graph. The tooltip lists every sensor of that chip (Tctl, Tccd1, Tccd2).
- Linux data/mechanism: Find hwmon by `name` in this order: `k10temp`, `zenpower`, `coretemp`, `cpu_thermal`. The main reading is the `tempN_input` whose label is "Tctl" or "Package id 0", else temp1. 1 Hz.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-040 CPU clock
- Was: v2 PERF-035
- Source: TS "4631 MHz" | MC Speed
- Behaviour: Average clock in integer MHz on the card and stats grid, per-core MHz in cell tooltips, and the clock option of the secondary graph.
- Linux data/mechanism: `cpuinfo_avg_freq` (APERF/MPERF based) per policy, at 1 Hz while shown, else `scaling_cur_freq` (U-28).
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-013

## Memory page

### PERF-041 Memory graphs
- Was: v2 PERF-036
- Source: TS "Physical memory", "Commit", "Swap" (L28573-28575) | MC memory and swap graphs
- Behaviour: Three graphs in a column:
  - "Physical memory": used filled, committed dashed, dirty as a thin line. Max = total.
  - "Commit": Committed_AS against CommitLimit. It can go above 100%, and the label says so.
  - "Swap": used. Hidden when there is no swap.
  - The page context menu has "Show memory composition" and "Show swap usage", both persisted and default on.
- Linux data/mechanism: `/proc/meminfo` at 1 Hz. Used = MemTotal − MemAvailable (MemFree when Available > Total). Swap used = SwapTotal − SwapFree. Keys `performance.memory_composition = true` and `performance.memory_swap_graph = true`.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-019

### PERF-042 Memory composition bar
- Was: v2 PERF-037
- Source: TS "Memory composition" (In use, In use (compressed), Modified, Standby, Free) | MC composition widget
- Behaviour: A horizontal bar with five segments and hover tooltips. Each tooltip gives the amount and a one-line meaning, for example "Standby: cache the kernel can drop".
- Linux data/mechanism:
  - Free = MemFree.
  - Modified = Dirty + Writeback.
  - Compressed = zram `mm_stat` field 3 (mem_used_total) + meminfo `Zswap`, clamped to In use.
  - In use = Total − Available − Compressed.
  - Standby = Total − In use − Compressed − Modified − Free, clamped at 0.
- Verdict: ADAPT. Windows page lists map to Linux meminfo fields, with zram and zswap as the compressed segment.
- Phase: P1
- Privilege: none

### PERF-043 Memory stats
- Was: v2 PERF-038
- Source: TS In use, Committed, Cached, Paged pool, Non-paged pool, Slots used, Form factor, Hardware reserved, Speed | MC Available, Swap used, Swap available, Compressed, Savings, Type
- Behaviour: Rows, in order:
  - In use, Available, Committed ("46.20 GiB / 67.20 GiB"), Cached;
  - "Kernel slab (reclaimable)" (replaces Paged pool), "Kernel slab (unreclaimable)" (replaces Non-paged pool);
  - Speed, Slots used ("2 of 4"), Form factor, Type, Hardware reserved;
  - Swap used, Swap available;
  - Compressed, Savings (shown only when at least 1 MiB is compressed);
  - Huge pages (shown only when non-zero).
- Linux data/mechanism:
  - meminfo `Cached`, `SReclaimable`, `SUnreclaim`, `Committed_AS`/`CommitLimit`, `HugePages_*`.
  - Swap available = SwapFree (Mission Center wrongly shows SwapTotal).
  - Savings = (zram orig_data_size + Zswapped) − (zram compr_data_size + Zswap).
  - Hardware reserved = sum of DIMM sizes − MemTotal.
  - DIMM rows from PERF-046. Speed is the configured speed in MT/s.
- Verdict: ADAPT. Pools become slab.
- Phase: P1 for meminfo rows; P2 for DIMM rows.
- Privilege: none
- Applied: table d

### PERF-044 zram and zswap details
- Was: v2 PERF-039
- Source: MC Compressed/Savings | TS "In use (compressed)"
- Behaviour: A "Compressed memory" block under the stats.
  - Per zram device: Device, Size (disksize), Stored (orig), Compressed, Memory used, Ratio ("3.4 : 1"), Algorithm.
  - zswap: "Enabled: No" here; when enabled, Pool size and Stored.
- Linux data/mechanism:
  - `/sys/block/zram*/{disksize,mm_stat,comp_algorithm}`. The active algorithm is the one in brackets.
  - `/sys/module/zswap/parameters/{enabled,compressor,max_pool_percent}` and meminfo `Zswap`/`Zswapped`.
- Verdict: ADAPT. The block is new and gives detail Mission Center only sums.
- Phase: P2
- Privilege: none

### PERF-045 Swap devices list
- Was: v2 PERF-040
- Source: TS "Page file" concept | MC swap graph
- Behaviour: A table under the compressed block with the columns Name, Type (partition, file, zram), Size, Used and Priority.
- Linux data/mechanism: `/proc/swaps`.
- Verdict: ADAPT. Replaces Windows page files.
- Phase: P2
- Privilege: none

### PERF-046 Memory modules on the Memory page
- Was: v2 PERF-041, add PERF-085
- Source: TS "Memory module" heading (crosscheck A.16, B.5; type list L29453-29482) | MC first-DIMM-only fields (`memory.rs:348-368`)
- Behaviour: Below the Details grid, a summary line reads "Slots used: 2 of 4". Under it is one collapsible block per populated DIMM, headed "Memory module N" (N from 1 in SMBIOS order). Fields:
  - **Slot**: bank locator and locator, for example "P0 CHANNEL A (DIMM 0)";
  - **Capacity** (GiB);
  - **Speed** (MT/s), plus **Configured speed** when it differs;
  - **Type** (the SMBIOS string as reported: DDR4, DDR5, LPDDR5, HBM3, MRDIMM and others);
  - **Form factor**;
  - **Manufacturer**;
  - **Part number**;
  - **Rank**;
  - **Voltage** (configured, V).

  Serial numbers are never shown, here, on the Information tab or in copy text. Empty slots are left out. Blocks start collapsed when there are more than 4 modules. The Information tab uses the same data (PERF-099).
- Linux data/mechanism: udev properties of `/sys/devices/virtual/dmi/id`, through libudev or by parsing `/run/udev/data/+dmi:id`: `MEMORY_ARRAY_NUM_DEVICES`, `MEMORY_DEVICE_<n>_{SIZE,LOCATOR,BANK_LOCATOR,SPEED_MTS,CONFIGURED_SPEED_MTS,TYPE,FORM_FACTOR,MANUFACTURER,PART_NUMBER,RANK,CONFIGURED_VOLTAGE}`, from systemd's `dmi_memory_id`. The `SERIAL_NUMBER` key is not read. Read once at startup. Verified readable without root on this machine (DDR4 3200 Kingston KHX3200C18D4/16G). No `dmidecode`. When the keys are missing (systemd-udevd before v252), the block list is replaced by "Module details: Not reported".
- Verdict: BUILD
- Phase: P2
- Privilege: none (this corrects PLAN.md and ADR-016, which say DIMM details need root)
- Applied: D-007, R-030

## Disk pages

### PERF-047 Disk list and kind
- Was: v2 PERF-042
- Source: TS drive types | MC disk kinds and exclusions
- Behaviour: One page per physical disk. Kind is one of NVMe, SSD, HDD, eMMC, SD, USB, Optical or Virtual. This machine has three: nvme0n1, nvme1n1 and sda.
- Linux data/mechanism:
  - `/sys/block/*`, excluding `loop`, `ram`, `zram`, `fd`, `dm-`, `md` and `zd`. Partitions and `dm-*` devices appear as children (PERF-050).
  - Kind rules, in order: an `nvme` prefix is NVMe; `mmcblk` plus `device/type` (MMC/SD) gives eMMC or SD; udev `ID_BUS=usb` gives USB; `sr*` is Optical; `virtio`/`xen` is Virtual; otherwise `queue/rotational` decides HDD (1) or SSD (0).
  - Model from udev `ID_MODEL`, else sysfs `device/model`.
  - Enumeration by rescan every 5 s while demanded (PERF-005).
- Verdict: ADAPT. Windows drive types become Linux kinds.
- Phase: P1
- Privilege: none
- Applied: R-052

### PERF-048 Disk graphs
- Was: v2 PERF-043
- Source: TS "Active time", "Disk transfer rate" | MC throughput with write dashed
- Behaviour: "Active time" (0 to 100%, filled) and "Transfer rate" (Read filled, Write dashed, dynamic scale with a power-of-two axis, "Throughput over 60 seconds").
- Linux data/mechanism: one `pread` of `/proc/diskstats` per 1 Hz tick for all disks (ADR-016; v2's per-device `/sys/block/<d>/stat` is replaced).
  - Active = Δio_ticks / Δt_ms.
  - Read rate = Δsectors_read × 512 / Δt; write likewise.
  - Counters are u64 with reset detection (ADR-008).
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-052, table d

### PERF-049 Disk stats
- Was: v2 PERF-044
- Source: TS Average response time, Read speed, Write speed, Capacity, Formatted, System disk, Page file, Hardware name, Used space, Free space | MC Total read/written, Type, WWN, Serial, Rotation rate
- Behaviour: Rows: Active time, Average response time (ms, 2 decimals), Read speed, Write speed, Total read, Total written, Capacity, Formatted, Used space, Free space, System disk, Swap (replaces Page file: Yes/No), Hardware name, Type, Interface, WWN, Serial, Rotation rate, Temperature. WWN, Serial and Rotation rate are hidden when missing (PERF-013).
- Linux data/mechanism:
  - Response time = Δ(read_ticks + write_ticks + discard_ticks + flush_ticks) / Δ(reads + writes + discards + flushes), from the matching `/proc/diskstats` fields.
  - Totals from the sector counters since boot.
  - Capacity = `/sys/block/<d>/size` × 512.
  - Formatted, used and free = sums of `statvfs` over mounted filesystems on this disk.
  - System disk = holds `/` or `/boot`.
  - Swap = any `/proc/swaps` entry on this disk or its holders.
  - Interface from udev `ID_BUS` or the transport (nvme, ata, usb, scsi, virtio).
  - WWN and Serial from udev `ID_WWN` and `ID_SERIAL_SHORT`, or UDisks2 `Drive.WWN`/`Serial`.
  - Rotation rate from UDisks2 `Drive.RotationRate`.
- Verdict: ADAPT. Page file becomes Swap.
- Phase: P1 for rates, active time, capacity and response time; P2 for the rest.
- Privilege: none. Disk serials come from udev without root. D-007 covers memory modules only.

### PERF-050 Partitions table
- Was: v2 PERF-045
- Source: TS "Partitions" table ("%zu partitions - %s"; Partition, Size, Offset, Drive, Mount path, Volume, File system, Flags, Type) | MC partition list with usage bar
- Behaviour: The heading reads "3 partitions - GPT". Columns:
  - Partition (`nvme0n1p2`);
  - Size;
  - Offset;
  - Device: a dm or LUKS holder such as `luks-…` → `dm-0` (replaces Drive);
  - Mount path: the first mount, the rest in a tooltip ("Additional mount points:");
  - Label (replaces Volume);
  - File system;
  - Usage: a bar plus percent;
  - Flags;
  - Type.

  Unallocated gaps over 1 MiB show as rows with Type "Unallocated". Rows are ordered by offset, as in TaskSlinger.
- Linux data/mechanism:
  - `/sys/block/<d>/<p>/{start,size,partition,ro}` and `/sys/block/<d>/<p>/holders/`.
  - udev `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`.
  - Mounts from `/proc/self/mountinfo`. Usage from `statvfs`.
  - Type names from a GPT GUID table: EFI system, Linux filesystem, Linux root (x86-64), Linux home, Linux swap, Linux LVM, Linux RAID, LUKS, Microsoft basic data, Microsoft reserved, Windows recovery, BIOS boot. Anything else shows "GPT partition {guid}". MBR types show "MBR 0x%02X".
  - Flags from GPT attributes: bit 0 "Required", bit 2 "Boot" (legacy BIOS bootable), bit 60 "Read-only", bit 62 "Hidden", bit 63 "No automount". The sysfs `ro` value also gives "Read-only".
- Verdict: ADAPT. Drive letters become device and mount path.
- Phase: P2
- Privilege: none

### PERF-051 SMART data dialog
- Was: v2 PERF-046
- Source: MC SMART dialog
- Behaviour: A "Show SMART data" button sits in the disk page header when SMART is available. The dialog opens as its own window ("tasklens SMART Data", APP-045 naming) with: Last updated ("2m 5s ago"), Powered on, Drive health (the overall assessment, which Mission Center leaves empty), Self-test status. Then a sortable table:
  - ATA columns: ID, Attribute, Value, Normalized, Threshold, Worst, Type (Pre-fail or Old-age), Updates (Online or Offline), Assessment (FAILING, Failed in the past, OK).
  - NVMe rows: Percent used, Available spare, Spare threshold, Total data read, Total data written, Warning temp time, Critical temp time, Controller busy time, Warning temperature, Critical temperature, Temperature sensors, Unsafe shutdowns, Media errors, Error log entries, Power cycles.
  - A "Refresh" button.
  - On failure the dialog reads "Failed to get SMART data" and names the reason.
- Linux data/mechanism:
  - ATA: UDisks2 `org.freedesktop.UDisks2.Drive.Ata.SmartGetAttributes({})`, `SmartUpdated`, `SmartPowerOnSeconds`, `SmartFailing`, `SmartSelftestStatus`. "Refresh" calls `SmartUpdate({})`.
  - NVMe: `org.freedesktop.UDisks2.NVMe.Controller.SmartGetAttributes({})` and `SmartUpdate`. Needs udisks2 2.10 or newer with the NVMe module (U-25).
- Verdict: BUILD
- Phase: P2 (the smartctl fallback in P3)
- Privilege: Reading cached attributes is allowed for the active session. `SmartUpdate` uses `org.freedesktop.udisks2.ata-smart-update` (allowed for the active local session by default). When UDisks2 or its NVMe module is missing, a "Read with smartctl (admin)" button runs `pkexec /usr/bin/smartctl -a -j /dev/<d>` under `org.freedesktop.policykit.exec` and parses the JSON. This is not a tl-helper operation (R-004 has no SMART read). The fallback ships in P3 and is listed in table (c) (R-088).

### PERF-052 Eject with blocking processes
- Was: v2 PERF-047
- Source: MC eject button and failure dialog
- Behaviour: An "Eject" button appears only for ejectable or removable drives, and its visibility updates live. A failed unmount opens the window "Disk is in use".
  - It lists blocking processes grouped by app: name, PID, open paths, and an "End Task" button per row.
  - Buttons: "Close", "Retry", "End All".
  - "End Task" sends SIGTERM. Holding Shift changes the button to "End Process" and sends SIGKILL.
  - "End All" with more than one process, and any action on another user's process or a protected process, asks first with the R-036 dialogs. A single End Task on the user's own process does not ask.
  - A retry follows automatically after ending.
  - Success shows the toast "Disk ejected".
- Linux data/mechanism:
  - UDisks2 `Filesystem.Unmount({})` for each mounted filesystem, `Encrypted.Lock` for LUKS holders, then `Drive.Eject({})`, or `Drive.PowerOff` for USB.
  - Blockers: scan `/proc/*/{cwd,root,exe}`, `/proc/*/fd/*` and `/proc/*/maps` for paths under the mount points.
  - Signals go through the process actions path (pidfd with identity check, ADR-006).
- Verdict: BUILD
- Phase: P2
- Privilege: Unmount and eject use `org.freedesktop.udisks2.filesystem-unmount-others` and `eject-media` (allowed for the active session on removable media). The blocker scan sees only the user's own processes. The window adds "Processes of other users may also hold this disk" when any `/proc/*/fd` read failed with EACCES.
- Applied: R-036, R-045 (no "Force end" naming), D-009

### PERF-053 Disk temperature
- Was: v2 PERF-048
- Source: MC sidebar disk temperature
- Behaviour: Temperature on the disk card line 2 ("12.0% · 41 °C") and as a stats row.
- Linux data/mechanism: The drive's hwmon (`nvme` hwmon "Composite", temp1, present here; `drivetemp` for SATA when that module is loaded). Fallback: UDisks2 `Drive.Ata.SmartTemperature` (Kelvin). 1 Hz.
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Network pages

### PERF-054 Interface list and type
- Was: v2 PERF-049
- Source: TS interface types and states (L28137-28152) | MC NetworkManager kinds
- Behaviour:
  - One page per interface, except loopback.
  - Type: Ethernet, Wi-Fi, Virtual, USB network, Bridge, Cellular, Tunnel, VPN or Other.
  - State uses TaskSlinger's labels, one-to-one with the kernel operstate: Connected (up), Disconnected (down), Testing, Dormant, Not present, Lower layer down. "unknown" with carrier shows Connected; "unknown" without carrier shows `Unknown`.
  - Here `tailscale0` is a Tunnel and shows. `docker0` is Virtual and hidden by default (PERF-004).
- Linux data/mechanism:
  - `/sys/class/net/<if>/`, in order:
    - a `wireless/` or `phy80211` entry is Wi-Fi;
    - `bridge/` is Bridge;
    - `tun_flags` or `type`=65534 is Tunnel;
    - `device/driver` under a usb parent is USB network;
    - `wwan*` or `type`=519 is Cellular;
    - a name matching `docker*|veth*|br-*|virbr*|vnet*` is Virtual;
    - `type`=1 with a `device` link is Ethernet;
    - anything else is Other.
  - NetworkManager `Device.DeviceType`, read as `u` (u32), refines VPN and WireGuard when NM is present.
  - `operstate` and `carrier` are read from sysfs on each 1 Hz tick while network is demanded. The interface list is rescanned every 5 s. There is no RTM_NEWLINK subscription in v1 (inferred from R-052).
- Verdict: ADAPT. Classification uses sysfs first, so it works without NetworkManager.
- Phase: P1
- Privilege: none
- Applied: R-052, R-053

### PERF-055 Throughput graph and legend
- Was: v2 PERF-050, add PERF-084 (graph and legend part)
- Source: TS "Throughput" (L28242), "Upload" (L28283), "Download" (L28192) | MC send dashed, receive filled, linked scale
- Behaviour: Graph title "Throughput". Legend "Download" (filled area in the network colour #359CAB) and "Upload" (dashed line, no fill, same colour at a lighter tint). One shared scale (PERF-018). Label "Throughput over 60 seconds". Mission Center's "S:" and "R:" are not used.
- Linux data/mechanism: one `pread` of `/proc/net/dev` per 1 Hz tick for all interfaces (ADR-016; v2's per-interface `statistics/{rx,tx}_bytes` files are replaced). Download = rx. Rates use actual elapsed time (ADR-008).
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-052

### PERF-056 Network stats
- Was: v2 PERF-051, add PERF-084 (details rows part)
- Source: TS Adapter name, Connection type, IPv4, IPv6, MAC, Public IP, Receive/Transmit link speed, Interface index, Display unit (crosscheck A.15) | MC Receive, Send, Total received, Total sent, Connection status
- Behaviour: Rows:
  - Download speed, Upload speed, Total downloaded, Total uploaded (since boot);
  - Adapter name, Interface name, Connection type, Connection status;
  - Link speed. Ethernet reports one speed for both directions. Wi-Fi shows its RX and TX bitrates;
  - IPv4 address, IPv6 address (PERF-059), MAC address, Interface index, Driver;
  - Display unit (PERF-057);
  - Public IP (PERF-060).
- Linux data/mechanism:
  - Adapter name from the systemd hwdb (`ID_MODEL_FROM_DATABASE` and `ID_VENDOR_FROM_DATABASE` in the net device's udev properties). No bundled database like Mission Center's. Missing: `Unknown adapter`.
  - sysfs `speed` (Mb/s; -1 shows `Not reported`), `address`, `ifindex`, and the `device/driver` basename.
- Verdict: BUILD
- Phase: P1 for rates and totals; P2 for the rest.
- Privilege: none

### PERF-057 Display unit row
- Was: add PERF-083
- Source: TS crosscheck A.15 and B.6 (L28290)
- Behaviour: The network page's Details grid has a row "Display unit". Its value is the current unit: `Mbps` or `MiB/s` with auto-scale off, `bits/s, auto` or `bytes/s, auto` with it on. Clicking the value opens a two-item menu: "Bits per second" and "Bytes per second". The choice changes the global setting (Settings > Network transfer unit). All network graphs, cards and process columns switch at once. Tooltip: "Applies to all network values".
- Linux data/mechanism: `units.network = "bits" | "bytes"`.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: R-014, R-015

### PERF-058 Wi-Fi details
- Was: v2 PERF-052
- Source: MC SSID, Signal strength, Maximum bitrate, Frequency
- Behaviour: Wi-Fi rows: SSID, Signal (4-level icon plus "-52 dBm (78%)"), RX and TX bitrate, Frequency ("5180 MHz, channel 36"), Security.
- Linux data/mechanism:
  - NetworkManager `Device.Wireless.ActiveAccessPoint`, then `AccessPoint.{Ssid,Strength,Frequency,MaxBitrate,RsnFlags}`.
  - Without NetworkManager: nl80211 generic netlink `NL80211_CMD_GET_INTERFACE` (SSID, frequency) and `NL80211_CMD_GET_STATION` (signal, RX and TX bitrate).
  - 1 Hz while the page is shown.
- Verdict: ADAPT. nl80211 is added as a fallback.
- Phase: P2
- Privilege: none
- Applied: table d (integer MHz)

### PERF-059 IP addresses
- Was: v2 PERF-053
- Source: TS IPv4/IPv6 | MC NM AddressData
- Behaviour: Shows every address with its prefix ("192.168.1.10/24"). IPv6 shows global addresses first and link-local addresses in the tooltip.
- Linux data/mechanism: rtnetlink `RTM_GETADDR` dump when the page opens and on each 5 s rescan while the page is shown. This avoids depending on NetworkManager. (v2's `RTM_NEWADDR`/`RTM_DELADDR` subscription is replaced by the rescan, inferred from R-052.)
- Verdict: ADAPT. rtnetlink replaces NetworkManager IP config.
- Phase: P2
- Privilege: none

### PERF-060 Public IP fetch
- Was: v2 PERF-054, add amend (Public IP fetch)
- Source: TS Public IP "Fetch" and "Not fetched", Confirm Public IP Fetch dialog, `AutoFetchPublicIP` (Window & startup per crosscheck B.7) | APP-056
- Behaviour:
  - The row shows "Not fetched" with a "Fetch" button.
  - The first fetch opens the "tasklens Public IP Notice" window from APP-056 (text, "Service: https://api.ipify.org", the checkbox "Do not ask again for public IP fetches", buttons "Continue" and "Cancel").
  - Result: "203.0.113.4 (fetched 14:02)".
  - "Auto-fetch Public IP on launch" sits under Settings > Window & startup, default off. It fetches once at startup, and only after the notice has been accepted.
  - The same value shows on the Information tab (PERF-104).
- Linux data/mechanism:
  - HTTPS GET to `https://api.ipify.org` (IPv4); `https://api6.ipify.org` too when a global IPv6 route exists.
  - `ureq` 3 (sync, rustls, no Tokio; U-30) in the blocking pool, 5 s timeout.
  - The result is never saved to disk.
  - Keys `network.auto_fetch_public_ip = false`, `network.public_ip_notice_accepted = false`.
- Verdict: BUILD
- Phase: P2
- Privilege: none
- Applied: R-058

### PERF-061 Network settings link
- Was: v2 PERF-055
- Source: TS "Show more information" (L28329) | MC "Network Settings" (GNOME launch-panel)
- Behaviour: A "Network settings" link under the stats and in the page context menu.
- Linux data/mechanism:
  - KDE (this machine): run `kcmshell6 kcm_networkmanagement`.
  - GNOME: session D-Bus `org.gnome.Settings` `/org/gnome/Settings` `org.freedesktop.Application.ActivateAction("launch-panel", [<"wifi"|"network">])`.
  - Hidden on other desktops.
- Verdict: ADAPT
- Phase: P3
- Privilege: none

## GPU pages

### PERF-062 GPU enumeration and tl-gpu-probe
- Was: v2 PERF-056
- Source: TS GPU N cards | MC nvtop | review section 3 "GPU child contract" | ADR-007
- Behaviour: One page per GPU, keyed by NVML UUID (NVIDIA) or PCI address (PERF-005). A hung or crashed probe shows `Unavailable` on every GPU metric with the tooltip "GPU monitor restarting", never zero. When the probe binary is missing, GPU metrics show `Not supported` with the tooltip "GPU probe not installed".
- Linux data/mechanism:
  - **Discovery:** DRM devices from `/sys/class/drm/card*/device` (vendor `0x10de`, `0x1002`, `0x8086`), plus NVIDIA devices from NVML.
  - **NVIDIA, only inside `tl-gpu-probe`** (ADR-007): `nvmlInit_v2`, `nvmlDeviceGetCount_v2`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetUUID`, `nvmlDeviceGetName`, `nvmlDeviceGetPciInfo_v3` (matched to the DRM card by bus id; here `00000000:2D:00.0` matches card1), `nvmlDeviceGetMinorNumber`. Probe lookup: `$TASKLENS_GPU_PROBE`, the running binary's directory, `/usr/lib/tasklens/tl-gpu-probe`.
  - Socketpair, postcard framing, `Sample` deadline 500 ms, restart rules per ADR-007.
  - **Rate:** device totals and GPU process data at 1 Hz, never faster. The Refresh setting does not change it (v2's 500 ms option is dropped).
  - **AMD and Intel:** read in-process from sysfs, hwmon and fdinfo (PERF-074, PERF-075). No child process is needed, because no native library is loaded.
- Verdict: ADAPT. nvtop is replaced by direct per-vendor sources.
- Phase: P1
- Privilege: none
- Applied: R-026

### PERF-063 Engine graphs
- Was: v2 PERF-057, add amend (GPU page engine graphs)
- Source: TS engine graphs (Utilization/3D, Compute, Video Encode, Video Decode, Video Processing, "Engine %u"), "Select GPU Engine Graphs", "Hide This Engine Graph", `HiddenGpuEngines`, crosscheck B.4
- Behaviour:
  - Row 1 is a grid of engine graphs in the CPU-grid style. Row 2 holds "Memory" (PERF-064) and "Temperature" (PERF-068), as in TaskSlinger 0.9.2. Row 3 is optional: "Power" (PERF-067) and "Clocks" (PERF-066).
  - The page context menu has "Select GPU engine graphs" (a checklist of the engines plus the Power and Clocks graphs) and, on an engine graph, "Hide this engine graph".
  - **NVIDIA engines** (verified on driver 615.71.09, JPEG and OFA included):

    | Graph | NVML call |
    |---|---|
    | 3D / Compute | `nvmlDeviceGetUtilizationRates` `.gpu` |
    | Memory controller | `nvmlDeviceGetUtilizationRates` `.memory` |
    | Video Encode | `nvmlDeviceGetEncoderUtilization` |
    | Video Decode | `nvmlDeviceGetDecoderUtilization` |
    | JPEG | `nvmlDeviceGetJpgUtilization` |
    | Optical flow | `nvmlDeviceGetOfaUtilization` |

  - **AMD engines:** "Graphics", "Compute", "Memory controller", "Video Encode", "Video Decode", "JPEG", "DMA".
  - **Intel engines:** "Render/3D", "Compute", "Copy", "Video", "Video Processing" (the video-enhance engine).
  - "Video Processing" is used only where AMD or Intel report a video-enhance engine. "Throughput" is never an engine name (it is the network graph title).
  - The probe checks each call per device. A call returning `NVML_ERROR_NOT_SUPPORTED` hides that engine; it is never drawn as zero. Engines the device does not report are not listed.
- Linux data/mechanism: NVML as above (probe). AMD and Intel per PERF-074 and PERF-075. Keys `performance.hidden_gpu_engines = []` (engine keys) and `performance.gpu_optional_graphs = []` (subset of `power`, `clocks`). Mission Center's middle and bottom graph choices are not built.
- Verdict: ADAPT. NVML has no 3D/Compute split and no video-processing engine, so tasklens shows one 3D/Compute engine plus JPEG and optical flow.
- Phase: P1 for 3D/Compute, Video Encode and Video Decode; P2 for the rest, hiding, and the optional row.
- Privilege: none
- Applied: R-054

### PERF-064 GPU memory
- Was: v2 PERF-058
- Source: TS GPU memory (dedicated), (shared), Hardware reserved memory | MC memory graph with dedicated solid and shared dashed
- Behaviour: The "Memory" graph shows dedicated (filled) against its total, and shared (dashed) against its total where the device has shared memory. Rows: "Dedicated memory" ("2.80 GiB / 15.92 GiB"), "Shared memory", "Hardware reserved memory" ("380.00 MiB").
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetMemoryInfo_v2` (`used`, `total`, `reserved`). NVML has no shared (GTT) figure, so that row shows `Not supported`.
  - AMD: `device/mem_info_vram_{used,total}` and `mem_info_gtt_{used,total}`.
  - Intel dGPU (xe or i915): the sum of `drm-resident-local0`/`drm-total-local0` over readable fdinfo, plus `device/tile0/vram0` sizes. Intel iGPU: "Shared" from the summed `drm-resident-system0`.
- Verdict: BUILD
- Phase: P1
- Privilege: none
- Applied: table d

### PERF-065 Video encode and decode
- Was: v2 PERF-059
- Source: TS Video Encode/Decode | MC Encode/Decode graph and rows
- Behaviour: Rows "Video encode" and "Video decode" in percent. When the hardware shares one engine, a single "Video encode/decode" row is shown. Encode and decode are also engine graphs (PERF-063).
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetEncoderUtilization(&util, &periodUs)` and `nvmlDeviceGetDecoderUtilization`. The active session count comes from `nvmlDeviceGetEncoderSessions` (tooltip).
  - AMD: `gpu_metrics` `vcn_activity[]`, else fdinfo `drm-engine-enc`/`dec`.
  - Intel: the video engines.
- Verdict: BUILD
- Phase: P1
- Privilege: none

### PERF-066 GPU clocks
- Was: v2 PERF-060
- Source: MC Clock speed, Memory speed, Clock Speeds graph
- Behaviour: Rows "Clock speed" ("1905 / 3105 MHz"), "Memory speed" ("7001 / 14001 MHz"), "Video clock". "Clocks" graph (optional row): GPU clock solid, memory clock dashed, scaled to the maximum. The tooltip shows the P-state ("P3") and the throttle reasons.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetClockInfo(NVML_CLOCK_GRAPHICS | NVML_CLOCK_MEM | NVML_CLOCK_VIDEO)`, `nvmlDeviceGetMaxClockInfo`, `nvmlDeviceGetPerformanceState`, `nvmlDeviceGetCurrentClocksEventReasons`.
  - AMD: hwmon `freq1_input` (sclk) and `freq2_input` (mclk). The maximum is the last line of `pp_dpm_sclk`/`pp_dpm_mclk`.
  - Intel: `gt_act_freq_mhz`/`gt_max_freq_mhz` (i915) or `device/tile0/gt0/freq0/{act_freq,max_freq}` (xe).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-067 GPU power
- Was: v2 PERF-061
- Source: MC Power draw current / limit, Power graph
- Behaviour: Row "Power draw" ("31.9 / 300 W"). The tooltip gives the limit range ("max 350 W") and energy since boot. "Power" graph (optional row) scaled to the limit.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetPowerUsage` (mW), `nvmlDeviceGetEnforcedPowerLimit`, `nvmlDeviceGetPowerManagementLimitConstraints`, `nvmlDeviceGetTotalEnergyConsumption` (mJ).
  - AMD: hwmon `power1_input`, else `power1_average` (µW), and `power1_cap`.
  - Intel dGPU: hwmon `energy1_input` delta and `power1_max`. Intel iGPU: RAPL `intel-rapl:0:1` (uncore), labelled "Package uncore (estimate)".
- Verdict: BUILD
- Phase: P2
- Privilege: none, except the Intel iGPU RAPL source, which needs PERF-106.

### PERF-068 GPU temperature and fan
- Was: v2 PERF-062
- Source: TS Temperature | MC Temperature
- Behaviour: Temperature in integer °C on the card, the stats grid and the "Temperature" graph. A "Temperature source" row (PERF-069). Rows for the slowdown and shutdown thresholds. "Fan speed" ("0% (0 RPM)") belongs to the GPU page and does not appear on the Fans pages. Memory temperature shows `Not reported` on this card.
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetTemperatureV` with `NVML_TEMPERATURE_GPU`, else `nvmlDeviceGetTemperature`. Also `nvmlDeviceGetTemperatureThreshold(SLOWDOWN, SHUTDOWN)`, `nvmlDeviceGetNumFans`, `nvmlDeviceGetFanSpeed_v2`, and `nvmlDeviceGetFanSpeedRPM` where available. Memory temperature uses `nvmlDeviceGetFieldValues(NVML_FI_DEV_MEMORY_TEMP)`.
  - AMD: hwmon `temp1` edge, `temp2` junction, `temp3` mem, `fan1_input`, `pwm1`. The main reading is junction when present, else edge.
  - Intel dGPU: hwmon `temp1`. The iGPU has no sensor of its own and shows `Not supported`.
- Verdict: BUILD
- Phase: P1 for temperature; P2 for fan and thresholds.
- Privilege: none

### PERF-069 GPU temperature source values
- Was: add PERF-092
- Source: TS crosscheck A.17 (L29508-29509 "Windows D3DKMT", "Windows D3DKMT: %s"; NVAPI L35305)
- Behaviour: The GPU page "Temperature source" row, the temperature tooltip and Information > Graphics > "Temperature source" show where the reading came from:
  - `NVML (GPU core)`
  - `hwmon amdgpu: edge`, `hwmon amdgpu: junction`, `hwmon amdgpu: mem`
  - `hwmon i915` or `hwmon xe: pkg`
  - `Not reported` when there is no source

  On AMD the tooltip lists all sensors.
- Linux data/mechanism: NVIDIA per PERF-068. AMD and Intel: `/sys/class/drm/cardN/device/hwmon/hwmon*/temp*_{input,label}`.
- Verdict: ADAPT
- Phase: P1 for the value; P2 for the Information row.
- Privilege: none

### PERF-070 PCIe link
- Was: v2 PERF-063
- Source: MC PCI Express speed, Max PCI Express speed
- Behaviour: Rows "PCI Express" ("Gen 4 x16"), "Max PCI Express" ("Gen 5 x16"; hidden when equal), "PCIe throughput" (RX and TX). Here the card runs at 16 GT/s x16 with a 32 GT/s maximum (verified).
- Linux data/mechanism:
  - NVIDIA: `nvmlDeviceGetCurrPcieLinkGeneration`, `nvmlDeviceGetCurrPcieLinkWidth`, `nvmlDeviceGetGpuMaxPcieLinkGeneration`, `nvmlDeviceGetMaxPcieLinkWidth`, `nvmlDeviceGetPcieThroughput(TX_BYTES | RX_BYTES)`.
  - All vendors, and the fallback: `device/{current_link_speed,current_link_width,max_link_speed,max_link_width}`, mapping GT/s to Gen (2.5→1, 5→2, 8→3, 16→4, 32→5, 64→6).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-071 GPU static fields
- Was: v2 PERF-064
- Source: TS Driver version, Driver date, DirectX version, Physical location | MC OpenGL, Vulkan, PCI bus address
- Behaviour: Rows: Driver version (615.71.09), Kernel module (replaces Driver date: "nvidia 615.71.09"), CUDA version, Vulkan version, OpenGL version (replaces DirectX version), Physical location ("PCI bus 45, device 0, function 0"), VBIOS version, UUID, Architecture.
- Linux data/mechanism:
  - NVIDIA: `nvmlSystemGetDriverVersion`, `nvmlSystemGetCudaDriverVersion_v2`, `nvmlDeviceGetVbiosVersion`, `nvmlDeviceGetArchitecture`.
  - Kernel module from `/sys/module/<driver>/version`.
  - Vulkan and OpenGL: a separate short-lived run of `tl-gpu-probe --api-versions`, once at startup, apart from the NVML sampler process:
    - Vulkan: ash calls `vkEnumeratePhysicalDevices` and matches the GPU through `VK_EXT_pci_bus_info`; it reads `apiVersion` and `VkPhysicalDeviceDriverProperties`.
    - OpenGL: EGL `EGL_EXT_platform_device` on the matching DRM device, then a context, then `glGetString(GL_VERSION)`.
    - A crash only gives `Unknown`.
  - The mode, its output frame and its 5 s deadline are defined in ADR-007 "API versions mode" (R-087).
- Verdict: ADAPT. DirectX becomes Vulkan and OpenGL; the driver date becomes the module version.
- Phase: P2
- Privilege: none

### PERF-072 Displays list
- Was: v2 PERF-065, add amend (GPU displays list)
- Source: TS Displays (connection type, "%s @ %d Hz", "%d-bit, %s", orientation; crosscheck A.17)
- Behaviour: Under the GPU stats, one row per enabled display on that GPU, in the PERF-073 format. Disconnected and disabled connectors are not listed. HDR, VRR and wide colour gamut are not shown before P4.
- Linux data/mechanism:
  - **DRM KMS read-only ioctls** on `/dev/dri/cardN` (no DRM master needed; seat ACL, U-29): `drmModeGetResources`, `drmModeGetConnector` (connector type), `drmModeGetEncoder`/`drmModeGetCrtc` (current mode and refresh), the connector property `max bpc`, and the EDID blob for the monitor name. Bit depth comes only from `max bpc` and is labelled "Max bit depth".
  - **KScreen on KDE:** run `kscreen-doctor -j` when the GPU page opens and again on a KScreen config change. It supplies enabled state (`enabled`), primary (`priority == 1`), orientation (`rotation`) and scale (`scale`); the current mode is cross-checked through `currentModeId`, `modes[].size` and `refreshRate`. tasklens never parses `kscreen-doctor -o` text output.
  - Without KScreen: mode and refresh from KMS, enabled state from KMS (a connector with an active CRTC), orientation `Not reported`.
  - If opening `/dev/dri/cardN` fails: monitor name from `/sys/class/drm/card*-*/edid` and modes from `modes`; Max bit depth `Not reported`.
  - v2's claim that the sysfs `enabled` file is wrong on NVIDIA 615 is withdrawn: KScreen agreed with sysfs during the check (U-26).
  - P4: HDR, VRR and wide colour gamut through the `kde_output_device_v2` Wayland protocol.
- Verdict: ADAPT. Linux has no actual-bit-depth API, so the row shows the maximum.
- Phase: P2 (HDR, VRR and WCG in P4)
- Privilege: none
- Applied: R-057

### PERF-073 Display connector type names and row format
- Was: add PERF-091
- Source: TS crosscheck A.17 (L29513-29519, L35237)
- Behaviour: Connector type is one of: VGA, DVI-I, DVI-D, DVI-A, Composite, S-Video, LVDS, Component, DIN, DisplayPort, HDMI, TV, eDP (Embedded DisplayPort), Virtual, DSI, DPI, Writeback, SPI, USB, Unknown. TaskSlinger's Windows-only names (D-Connector, UDI (Embedded), SDTV Dongle, Miracast) are left out. Row format: `<monitor name> · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%`, with ` (primary)` after the monitor name for the primary output.
- Linux data/mechanism:
  - Connector type from KMS `drmModeGetConnector().connector_type`, or from the sysfs connector name prefix (`card1-DP-2` gives DisplayPort).
  - KScreen `type` enum as the KDE cross-check (14 = DisplayPort, verified).
  - Monitor name from EDID descriptor 0xFC.
  - Orientation from KScreen `rotation`: 1 Landscape, 2 Portrait (left), 4 Landscape (flipped), 8 Portrait (right). DP-3 here reports 2 (verified).
- Verdict: ADAPT (DRM connector types replace Windows types)
- Phase: P2
- Privilege: none
- Applied: R-057

### PERF-074 AMD GPU backend
- Was: v2 PERF-066
- Source: MC nvtop amdgpu backend, PLAN section 3 "AMD and Intel GPUs"
- Behaviour: Same pages as NVIDIA. Not tested on this machine.
- Linux data/mechanism:
  - `device/gpu_busy_percent` (Graphics), `device/mem_busy_percent` (Memory controller).
  - `device/gpu_metrics`, a binary struct parsed by its header version: `average_gfx_activity`, `average_umc_activity`, `vcn_activity`, `temperature_*`.
  - Compute, DMA and JPEG engines from the sum of `/proc/<pid>/fdinfo/*` `drm-engine-{compute,dma,jpeg}` ns deltas over all readable DRM clients, de-duplicated by `drm-client-id`. When any fdinfo read fails, the engine graph tooltip says "Own processes only".
  - Memory, clocks, power and temperature per PERF-064 to PERF-068.
- Verdict: ADAPT
- Phase: P2
- Privilege: none. fdinfo engines cover only processes tasklens can read, and say so.

### PERF-075 Intel GPU backend
- Was: v2 PERF-067
- Source: PLAN section 3 | MC nvtop i915/xe
- Behaviour: Same pages as NVIDIA. Not tested on this machine.
- Linux data/mechanism:
  - Engines, preferred source: the i915 PMU through `perf_event_open`, type from `/sys/bus/event_source/devices/i915/type`, events `rcs0-busy`, `bcs0-busy`, `vcs0-busy`, `vecs0-busy`, `ccs0-busy`.
  - Fallback: summed fdinfo `drm-engine-{render,copy,video,video-enhance,compute}` (i915), or `drm-cycles-*`/`drm-total-cycles-*` (xe).
  - Clocks, power and memory per PERF-064, PERF-066 and PERF-067.
- Verdict: ADAPT
- Phase: P3
- Privilege: The PMU needs `CAP_PERFMON` or `kernel.perf_event_paranoid` ≤ 0. tasklens asks for neither. The fdinfo fallback covers only processes tasklens can read.

## Fans and batteries

### PERF-076 Fan pages
- Was: v2 PERF-068
- Source: MC 2.6 Fan
- Behaviour:
  - One page per fan: "Fan N" or its label, with the chip name ("nct6797").
  - Graphs: "Fan speed" (RPM filled; PWM % dashed on a 0 to 100 axis) and "Temperature" (paired sensor, PERF-077).
  - Rows: Fan speed, PWM %, Control mode (Manual, Automatic, Full speed, from `pwmN_enable`), Temperature, Chip.
  - A fan that reads 0 RPM for the whole session and has no label counts as not connected. Such fans are hidden by default; "Show disconnected fans" shows them.
  - On this machine nct6797 has fans 1 to 7 (verified).
- Linux data/mechanism: `/sys/class/hwmon/hwmon*/{name, fanN_input, fanN_label, fanN_max, pwmN, pwmN_enable}` at 1 Hz while shown or in the sidebar. GPU fans are excluded (PERF-068). Key `performance.show_disconnected_fans = false`.
- Verdict: BUILD. Display only, no fan control.
- Phase: P2
- Privilege: none. Missing Super I/O drivers mean no fans; PERF-108 covers that.

### PERF-077 Fan labels and temperature pairing
- Was: v2 PERF-069
- Source: MC per-file override directory | MC same-index temperature pairing
- Behaviour:
  - The fan page context menu has "Rename fan…" and "Temperature sensor >", which lists every hwmon temperature with its label and current value. The choice persists per fan key.
  - Default pairing: the same-index `tempN` on the same chip, but only when the chip has exactly one fan. Otherwise no sensor is chosen and the Temperature graph reads "Choose a sensor".
- Linux data/mechanism: `settings.toml` `[fans]` table: `"<fan key>" = { label = "...", temp = "<hwmon name>:<path>:tempN" }`.
- Verdict: ADAPT. Mission Center's config-file overrides become in-app settings. Index pairing is often wrong on Super I/O chips, so tasklens does not pair by default there.
- Phase: P3
- Privilege: none

### PERF-078 Battery and power device pages
- Was: v2 PERF-070
- Source: MC 2.7 Battery (all UPower kinds)
- Behaviour:
  - One page per UPower device except `DisplayDevice` and line power. Here that is the Logitech mouse via HID++ (verified).
  - Header: the kind ("Mouse", "Battery", "UPS"), then vendor and model.
  - Graph: "Power input"/"Power output" (W, charge positive, discharge negative) for system batteries; "Percentage" for peripherals.
  - Rows: Percentage, Energy, Voltage, Power, State, Time to full or Time to empty, Charge cycles, Serial, Type, Powering system, Technology, Capacity, Energy empty, Energy full, Energy full (design), Voltage min and max (design), Charge threshold, Charge start threshold, Charge end threshold, Temperature.
  - Charge threshold shows "Enabled" or "Disabled" (Mission Center always shows "Yes"). Threshold rows are hidden when unsupported (PERF-013).
- Linux data/mechanism:
  - UPower `org.freedesktop.UPower.EnumerateDevices`, `DeviceAdded`/`DeviceRemoved`, `PropertiesChanged`.
  - Properties: `Percentage`, `Energy`, `EnergyRate`, `Voltage`, `State`, `TimeToFull`, `TimeToEmpty`, `ChargeCycles`, `Serial`, `Type`, `PowerSupply`, `Technology`, `Capacity`, `EnergyEmpty`, `EnergyFull`, `EnergyFullDesign`, `VoltageMinDesign`, `VoltageMaxDesign`, `ChargeThresholdSupported`, `ChargeThresholdEnabled`, `ChargeStartThreshold`, `ChargeEndThreshold`, `Temperature`, `Vendor`, `Model`.
  - Without UPower: `/sys/class/power_supply/*` (`capacity`, `status`, `energy_now`, `power_now`, `cycle_count`), rescanned every 30 s.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-079 Battery charge history
- Was: v2 PERF-071
- Source: MC "Battery Charge over past week" (`GetHistory("charge", 604800)`)
- Behaviour: A 7-day charge graph with day gridlines and dashed lines across gaps. Shown for system batteries with at least 2 history points.
- Linux data/mechanism: UPower `Device.GetHistory("charge", 604800, 500)` and `GetHistory("rate", …)`. Refreshed every 10 minutes and when `UpdateTime` changes.
- Verdict: BUILD
- Phase: P3
- Privilege: none

## Minigraphs and detached graphs

### PERF-080 Minigraph panel and system cards
- Was: v2 PERF-072, add PERF-086 (system card labels)
- Source: TS 4.2 (cards VRAM, GPU, Network, Disk, Memory, CPU), View > Graphs toggles, labels "GPU temp", "Total"/"Kernel" (crosscheck A.28)
- Behaviour:
  - Floating cards in a grid over the bottom right of the Processes tab, each with a drop shadow (scaled by "Drop Shadow Intensity").
  - Each card shows its label at top left, the value at top right, and a graph.
  - System cards and labels: "CPU", "Memory", "Disk", "Network", "GPU", "VRAM", "GPU temp". "GPU temp" is off by default.
  - The CPU card draws total CPU as a filled area and kernel CPU as a #C1514E line. Its hover tooltip shows a two-line legend, "Total" and "Kernel".
  - View > Graphs has one checkbox per card: "CPU Graph", "Memory Graph", "Disk Graph", "Network Graph", "GPU Graph", "VRAM Graph", "GPU Temperature Graph" (APP-021).
  - Card size 180×90 logical px, scaled with UI scale.
- Linux data/mechanism: Reuses the system series. Disk = aggregate active time (busiest disk). Network = the sum over visible physical interfaces. GPU, VRAM and GPU temp follow the GPU in the card's `source`: `gpu:<device key>`, `vram:<device key>` or `gpu_temp:<device key>`; the bare values `gpu`, `vram` and `gpu_temp` mean the first GPU in sidebar order (R-085). Keys `graphs.minigraph_cpu`, `minigraph_memory`, `minigraph_disk`, `minigraph_network`, `minigraph_gpu`, `minigraph_vram` (all true) and `minigraph_gpu_temperature = false`. Card layout in `state.toml` `[[minigraphs]]` (`source`, `range_s`, `color`, `slot`, `detached`, `window`).
- Verdict: BUILD
- Phase: P1 (cards, View > Graphs checkboxes and Reset Minigraph Layout; R-089). "Keep detached graphs open" is P3 (PERF-084).
- Privilege: none

### PERF-081 Minigraph card context menu
- Was: v2 PERF-073
- Source: TS 4.2 menu (Hide Graph, Time Range, Graph Color, Add tray icon graph)
- Behaviour: Right-click shows "Hide graph", "Time range >" (30 seconds, 1 minute, 5 minutes, 10 minutes), "Graph color >" (Accent plus 17), "Detach" or "Attach to main window", "Add tray icon graph", and "Reset minigraph layout". A process-source card also has "Remove graph". Per-card range and colour persist.
- Linux data/mechanism: `state.toml` `[[minigraphs]]` `range_s` (30, 60, 300, 600; 300 and 600 from P2) and `color`.
- Verdict: BUILD
- Phase: P1 for hide and range; P3 for colour, detach and tray.
- Privilege: none
- Applied: R-016

### PERF-082 Move and reorder cards
- Was: v2 PERF-074
- Source: TS "Drag to move", "Reset Minigraph Layout", toasts "No space for more minigraphs", "Minigraph layout reset"
- Behaviour: Dragging a card moves it to another grid slot; other cards shift. The grid fills from the bottom right, at most 3 rows by 4 columns. Adding a card to a full grid shows the toast "No space for more minigraphs". "Reset minigraph layout" (card menu and View > Graphs) restores the default cards and slots and shows "Minigraph layout reset".
- Linux data/mechanism: `[[minigraphs]] slot`.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-083 Detached graph windows
- Was: v2 PERF-075
- Source: TS `TaskSlingerDetachedGraph`, always on top, detached menu (Show Main Window, Close Graph, Exit, Add Graph, Remove Graph)
- Behaviour:
  - "Detach", or dragging a card past the window edge, opens it as its own small borderless window. Dragging the window body moves it.
  - Its context menu: "Show Main Window", "Close graph", "Attach to main window", "Add graph >" (adds another card to the same window), "Remove graph", "Exit tasklens".
  - On KDE the window is kept above other windows and its position is restored. Elsewhere the compositor places it and it is not kept above. Size and stacked cards persist on every desktop.
- Linux data/mechanism:
  - GPUI `open_window` with `WindowKind::Floating`, client decorations, no title bar, `app_id` `io.github.oysteinkrog.Tasklens` (the same app ID, so the desktop file and icon match). Moving uses `start_window_move()`.
  - The window caption is "tasklens Graph <n>". On KDE the KWin bridge worker (`workers/dbus_kwin.rs`, R-046) finds the window by tasklens's pid and that caption in its `WindowsSnapshot`, then sends one-shot `KWinCommand`s to set `keepAbove` and `frameGeometry`, and reads moves back from the snapshot. v2's separate `/Scripting loadScript` script and per-window app ID are replaced by the bridge (R-086, ADR-004).
  - Persisted state (R-084): each card's `[[minigraphs]]` entry has `detached = true` and `window = <n>`; `state.toml` `[[detached_windows]]` holds `id`, `width`, `height`, `x`, `y`. `x` and `y` are written and applied only on KDE through the bridge. ADR-010's rule against restoring the main window's position is unchanged.
- Verdict: ADAPT. Wayland clients cannot set position or keep-above, so the KWin bridge does it on KDE.
- Phase: P3 (needs the P2 KWin bridge)
- Privilege: none
- Applied: R-046, R-073, R-084, R-086

### PERF-084 Keep detached graphs open
- Was: v2 PERF-076
- Source: TS "Keep detached graphs open when closing main window" (`KeepDetachedOpenOnMainClose`, default 0)
- Behaviour: View > Graphs > "Keep detached graphs open when closing main window". When on, closing the main window hides it, and the app keeps running while any detached graph is open; "Show Main Window" brings it back. When off, closing the main window closes everything (unless a tray setting keeps the app, APP-047).
- Linux data/mechanism: Key `graphs.keep_detached_on_close = false`. `on_window_should_close` (ADR-010) checks for open detached windows before calling `cx.quit()`; this is a second exception next to close-to-tray. While hidden, demand comes only from the detached graphs (ADR-008).
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-085 Process-source cards and labels
- Was: v2 PERF-077, add PERF-086 (process card labels)
- Source: TS process-specific minigraphs (`ProcessSpecificMiniGraphs`), labels "Process %lu", " by app", " by name" (crosscheck A.28)
- Behaviour:
  - Cards whose source is one process (captured identity), a process image name, or an app. They are created from the Processes context menu ("Add Minigraph >", "Add Process Name Graph >", "Add App Name Graph >"), which the process spec owns; this spec owns the card.
  - Labels: `<name> (<pid>) · CPU`, `<name> by name · CPU`, `<App> by app · CPU`. The part after `·` is the column label (PERF-087). `Process <pid>` is used only until the name has been read.
  - A name card sums all processes with that image name. An app card sums the app's cgroup.
- Linux data/mechanism: The engine adds demand for these identities, so they are sampled even when off screen and while the window is hidden. `[[minigraphs]] source = "process:<boot_id 16 hex>:<pid>:<start_ticks>" | "name:<image name>" | "app:<app id>"` plus `column` (R-100). Process identity is the ADR-003 `ProcessKey`. App name comes from the cgroup scope and its desktop file.
- Verdict: BUILD
- Phase: P3
- Privilege: none for the user's own processes. Fields hidden for other users' processes show `Restricted`.
- Applied: R-066, R-100, R-101

### PERF-086 Minigraph states "No match" and "Ended"
- Was: add PERF-087
- Source: TS crosscheck A.28 (L28582 "No match", L28583 "Ended")
- Behaviour: The value area at top right shows one of two states:
  - **No match:** a name or app card that matches no process right now. The graph records a gap, not zero. When a process matches again, the graph resumes.
  - **Ended:** a single-process card whose process exited (its pid plus start time are gone). The graph stops at its last sample and stays until the user removes the card ("Remove graph"). The card never re-attaches to a new process that reuses the pid. After a restart, a card whose identity is gone shows "Ended"; so does a card saved in another boot (its source boot ID differs from the current one, R-100).
- Linux data/mechanism: Exit is detected when the identity is missing from a sweep, or through pidfd readability when the card holds a pidfd.
- Verdict: BUILD
- Phase: P3 (with PERF-085; the addendum's P2 is moved to match the menu that creates these cards, R-101)
- Privilege: none
- Applied: R-066

### PERF-087 Column labels for process cards
- Was: add PERF-088
- Source: TS crosscheck A.28 (L28576-28578 "WS total", "WS shared", "GPU shared")
- Behaviour: The column part of a process card label uses Linux memory terms:
  - `CPU`, `Disk`, `Network`, `GPU`, `VRAM` as in TaskSlinger;
  - `Memory` for the default Memory column (RssAnon, private memory);
  - `RSS` for the "Working set total" column (VmRSS; TaskSlinger "WS total");
  - `RSS shared` for the shared column (`RssFile + RssShmem`; TaskSlinger "WS shared");
  - `GPU shared` for GTT or shared GPU memory. On NVIDIA it is not offered, because NVML gives no per-process shared memory; the menu item is greyed out with the tooltip "Not supported on this GPU".
- Linux data/mechanism: `/proc/<pid>/status` (`RssAnon`, `VmRSS`, `RssFile`, `RssShmem`). AMD and Intel GTT: the `drm-memory-gtt` or `drm-resident-system` keys in `/proc/<pid>/fdinfo`.
- Verdict: ADAPT (labels renamed for Linux memory terms)
- Phase: P3
- Privilege: none for the user's own processes; other users' processes show `Restricted`.
- Applied: R-005, D-004

## Tray graphs

### PERF-088 Tray icon graph
- Was: v2 PERF-078
- Source: TS 4.3 (sources: page, column, process, name filter, app filter), CL 0.8.6, 0.8.27
- Behaviour:
  - Each tray graph is its own tray icon. It draws a live mini graph in its graph colour on a transparent background, with the current value as a filled bar height.
  - Added from a Performance page context menu ("Add tray icon graph"), a minigraph card menu, or the process context menu.
  - Toasts: "Tray icon graph added", "Tray icon graph removed", "Tray icon graph already exists", "Unable to add tray icon graph".
  - Tray graphs persist.
- Linux data/mechanism:
  - Crate `tl-tray` with `ksni =0.3.6`, features `async-io` and `blocking`, `ksni::blocking::TrayMethods::spawn` on its own thread. `cargo xtask check-deps` proves there is no Tokio (U-31). If that check fails, the fallback is a hand-written SNI and dbusmenu server inside `tl-tray` on its own zbus connection, never inside tl-engine.
  - One StatusNotifierItem per graph, bus name `org.kde.StatusNotifierItem-<pid>-<n>`, objects `/StatusNotifierItem` and `/MenuBar`, registered with `org.kde.StatusNotifierWatcher.RegisterStatusNotifierItem`.
  - Icons are ARGB32 buffers at 22×22 and 44×44 px, drawn on the CPU by `tl-tray/src/raster.rs` (fill columns plus a 1 px line, 2×2 supersampling).
  - Data comes from `EngineClient::latest()` on a 1 Hz GPUI timer. The icon is updated at most once per second, and only when its pixels changed.
  - `state.toml` `[[tray_graphs]]` with `source`, `column`, `color`, `range_s`.
- Verdict: ADAPT. Windows `Shell_NotifyIcon` becomes SNI.
- Phase: P3
- Privilege: none. With no `StatusNotifierWatcher` on the bus, the "Add tray icon graph" items are disabled with the tooltip "No system tray available".
- Applied: R-027

### PERF-089 Tray tooltips
- Was: v2 PERF-079
- Source: TS tooltip strings ("CPU Usage: ", "%d procs: %s", "CPU:", "RAM:", "GPU:", "Disk:", "Network:")
- Behaviour:
  - Page graph icon: `<Title>: 18.6%`, for example "CPU Usage: 18.6%".
  - Process graph icon: `<name> (PID n): CPU 3.2%`.
  - Name filter icon: `3 procs: firefox`.
  - App icon: `<App>: CPU 3.2%`.
  - Main tray icon (APP-047): five lines, "CPU: 11.9%", "RAM: 54.9%", "GPU: 3.0%", "Disk: 1.2%", "Network: 0.3 Mbps".
- Linux data/mechanism: SNI `ToolTip` property `(icon, title, description)` and the `NewToolTip` signal, at most 1 Hz.
- Verdict: BUILD
- Phase: P3
- Privilege: none
- Applied: R-067, table d

### PERF-090 Tray menus and graph colour
- Was: v2 PERF-080, add PERF-089
- Source: TS tray menu ("Tray icon graphs", "Close tray icon graph", Show Main Window, Exit), "Graph color" item (crosscheck A.29, L28587)
- Behaviour:
  - Tray graph icon menu, in order: "Graph color >", "Time range >", "Close tray icon graph", "Tray icon graphs >" (a checklist to add or remove the other graphs), separator, "Show Main Window", "Show mini window", "Exit tasklens".
  - "Graph color >" lists Accent and the 17 named accents with check marks, the same list as the minigraph menu. The choice saves per tray graph and the icon redraws at once.
  - Main tray icon menu (APP-047): "Show Main Window", "Tray icon graphs >", separator, "Exit tasklens".
  - Left click on any tasklens tray icon shows or hides the main window.
- Linux data/mechanism: `com.canonical.dbusmenu` on `/MenuBar`. `Activate(x, y)` toggles the window. Colour in `[[tray_graphs]] color`, range in `range_s`.
- Verdict: BUILD
- Phase: P3
- Privilege: none
- Applied: R-027

### PERF-091 Main tray icon while graphs show
- Was: v2 PERF-081
- Source: TS CL 0.9.2 "main tray icon is hidden while a tray graph is shown"
- Behaviour: While at least one tray graph exists, the plain app icon is not registered, and every graph icon's menu carries the main entries (PERF-090). With no graphs, the plain icon shows when a tray setting needs it (APP-047 owns those settings).
- Linux data/mechanism: Register or unregister the plain SNI item.
- Verdict: BUILD
- Phase: P3
- Privilege: none

### PERF-092 Tray icon re-registration
- Was: add PERF-090
- Source: TS crosscheck A.29 (L35761 "TaskbarCreated")
- Behaviour: When the panel restarts (for example after plasmashell crashes), every tasklens tray item reappears within 1 s with the same graphs and colours.
- Linux data/mechanism: Watch `NameOwnerChanged` for `org.kde.StatusNotifierWatcher`. When it gains a new owner, call `RegisterStatusNotifierItem` again for each item.
- Verdict: ADAPT (TaskbarCreated becomes the watcher name owner)
- Phase: P3
- Privilege: none

## Performance mini window

### PERF-093 Performance mini window
- Was: v2 PERF-082, add amend PERF-069 (mini window trigger)
- Source: TS 4.4 (CL 0.8.4: compact, movable, always on top), crosscheck B.1 (the view-mode button cycles process views and does not open the mini window)
- Behaviour:
  - A compact borderless window listing the visible sidebar cards (PERF-001) in one column. Width 260 logical px; height fits the cards.
  - Open it with View > Window > "Performance mini window" (checkable), Ctrl+Shift+M, the tray menu item "Show mini window", or a double-click on the Performance graph area. Closing the mini window unchecks the menu item. Ctrl+M stays Graph Summary View.
  - Double-click on the mini window, or "Show Main Window" in its menu, returns to the main window. Clicking a card opens the main window on that page.
  - Its context menu: "Always on top", "Show Main Window", "Close".
  - "Always on top" is on by default when the window opens and is not persisted (only the main window's pin persists). On KDE it sets `keepAbove` through the KWin bridge. On X11 it sends `_NET_WM_STATE_ABOVE`. Elsewhere it is disabled with the tooltip "Not supported by this desktop".
- Linux data/mechanism: GPUI `Floating` window, caption "tasklens Mini Window", matched by the KWin bridge by pid plus caption as in PERF-083 (R-086). While the main window is hidden, demand comes from the cards shown in the mini window.
- Verdict: ADAPT. TaskSlinger's trigger is unknown, so tasklens defines these.
- Phase: P3
- Privilege: none
- Applied: R-058, R-073, table b (Ctrl+Shift+M)

## Information tab

### PERF-094 Information layout, copy and filter
- Was: v2 PERF-083
- Source: TS 2.7 (two columns, section header with accent bar and copy button, tab-level copy icon, filter applies) | MC System Information "Copy All"
- Behaviour:
  - Information is tab 6 (Ctrl+6, Alt+6).
  - Two scrolling columns of sections. Each section header has a 3 px accent bar at left and a copy button at right.
  - Sections: Operating System, Processor, System, Memory, Memory modules, Disk drives, BIOS/Firmware, Motherboard, Graphics, Network adapters (PERF-095 to PERF-104).
  - The copy icon on the tab copies all sections. Toast: "Copied to clipboard". Format: a section name line, then `Label: value` lines, a blank line between sections. Memory module serials never appear (D-007).
  - The tab's filter box hides rows whose label and value do not match, and hides empty sections.
  - Values are selectable text. Missing values use the PERF-013 words.
  - Until the first data arrives, each section shows skeleton blocks, once per session.
  - Help > "System Information…" switches to this tab and scrolls to Operating System (APP-041).
  - Data loads when the tab first opens and on F5. Live values (uptime, available memory, counts) refresh at 1 Hz while the tab is visible.
- Linux data/mechanism: Engine `inventory` component, demanded by the tab (ADR-008 "Startup entries, inventory" row).
- Verdict: BUILD
- Phase: P2
- Privilege: none
- Applied: R-023, R-050, R-060

### PERF-095 Operating System section
- Was: v2 PERF-084
- Source: TS OS Name, Version, Build, Architecture, Install Date, Registered Owner, Product ID | MC OS name, version, package manager and its version, kernel release and version, distro logo (APP-041)
- Behaviour: Rows: OS name ("CachyOS Linux", with the distro logo beside the section title), Version, Build, Kernel, Kernel version, Architecture ("x86_64 (64-bit)"), Install date, Package manager ("pacman 7.0.0"), Machine ID.
  - Version is `VERSION_ID`, or "Rolling" when missing.
  - Build is `BUILD_ID`, or `Not reported`.
  - Machine ID replaces Product ID.
  - Registered Owner is dropped: Linux has no such record, and the user name is under System.
- Linux data/mechanism:
  - `/etc/os-release` (`PRETTY_NAME`, `VERSION_ID`, `BUILD_ID`, `LOGO`, looked up in the icon theme).
  - `uname(2)` (release, version, machine).
  - Install date: `statx` btime of `/`, else the first timestamp in `/var/log/pacman.log`.
  - Package manager version: the first of `pacman --version`, `dpkg-query --version`, `rpm --version` that exists, run once.
  - `/etc/machine-id`.
- Verdict: ADAPT. Product ID becomes Machine ID. Registered Owner is dropped.
- Phase: P2
- Privilege: none

### PERF-096 Processor section
- Was: v2 PERF-085
- Source: TS Name, Vendor, Architecture, Identifier, Logical Processors, Speed
- Behaviour: Rows: Name, Vendor ("AuthenticAMD"), Architecture, Identifier ("Family 25 Model 97 Stepping 2"), Cores, Logical processors, Base speed, Max speed, Microcode, Flags summary (virtualization, AVX-512 yes or no).
- Linux data/mechanism: `/proc/cpuinfo` (`vendor_id`, `cpu family`, `model`, `stepping`, `model name`, `microcode`, `flags`), topology files, `cpuinfo_max_freq`, base speed per PERF-033. Speeds in integer MHz.
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-097 System section
- Was: v2 PERF-086, add amend (Information tab, System section, User Domain)
- Source: TS Computer Name, User Name, User Domain, System Uptime, Boot Time, Process Count, Thread Count, Handle Count | MC Desktop group (APP-041)
- Behaviour: Rows:
  - Computer name: the pretty hostname plus the static hostname.
  - User name.
  - Domain: `getdomainname()`, or the DNS domain of the hostname. The row is hidden when that is empty or "(none)". TaskSlinger's "WORKGROUP" fallback is not used.
  - System uptime (PERF-034 format) and Boot time (`YYYY-MM-DD HH:MM:SS` local).
  - Process count, Thread count, Handle count: real values (fixing TaskSlinger's zero bug), computed as in PERF-033. "Handle count" has the tooltip "Open file descriptors, system-wide".
  - Desktop ("KDE Plasma 6.x"), Windowing system ("Wayland"), Session type, Virtual terminal.
- Linux data/mechanism:
  - systemd-hostnamed `org.freedesktop.hostname1` (`PrettyHostname`, `StaticHostname`).
  - `getpwuid(getuid())`; `getdomainname(2)`.
  - Boot time = now − `CLOCK_BOOTTIME`.
  - Desktop from `XDG_CURRENT_DESKTOP` plus `plasmashell --version` or `gnome-shell --version` (once), `XDG_SESSION_TYPE`, `XDG_VTNR`, and logind `Session` properties.
- Verdict: ADAPT. The Windows domain becomes the DNS or NIS domain. Mission Center's Desktop group is merged in.
- Phase: P2
- Privilege: none

### PERF-098 Memory section
- Was: v2 PERF-087
- Source: TS Total Physical, Available Physical, Memory Usage, Committed, Commit Limit, Peak Committed, Page Size
- Behaviour: Rows: Total physical, Available physical, Memory usage (percent), Committed, Commit limit, "Peak committed (since tasklens started)", Page size ("4096 bytes"), Huge page size, Swap total, zram size.
- Linux data/mechanism: `/proc/meminfo`, `sysconf(_SC_PAGESIZE)`, meminfo `Hugepagesize`. The kernel keeps no peak for Committed_AS, so tasklens tracks the maximum it has seen while running.
- Verdict: ADAPT. The peak covers the tasklens run, and the label says so.
- Phase: P2
- Privilege: none

### PERF-099 Memory modules section
- Was: v2 PERF-088
- Source: TS per-module block (Module, Slot, Capacity, Speed, Type, Form Factor, Manufacturer, Part Number), `information.png`
- Behaviour: One block per populated DIMM: "Module 1", Slot ("P0 CHANNEL A (DIMM 0)"), Capacity, Speed ("3200 MT/s"), Type, Form factor, Manufacturer, Part number. There is no Serial number row.
- Linux data/mechanism: Same udev keys as PERF-046. `SERIAL_NUMBER` is not read.
- Verdict: BUILD
- Phase: P2
- Privilege: none (verified on this machine)
- Applied: D-007, R-030

### PERF-100 Disk drives section
- Was: v2 PERF-089
- Source: TS Disk Drives (Space "%s free of %s", Volume, type, interface)
- Behaviour: One block per physical disk: Model, Type, Interface (NVMe, SATA, USB, SCSI, virtio), Capacity, Space ("812.40 GiB free of 1.82 TiB"), Volumes (mount paths), Serial.
- Linux data/mechanism: Same as PERF-047 to PERF-050.
- Verdict: ADAPT. Volume letters become mount paths.
- Phase: P2
- Privilege: none
- Applied: table d

### PERF-101 BIOS/Firmware section
- Was: v2 PERF-090
- Source: TS Firmware Type, BIOS Vendor, BIOS Version, BIOS Date
- Behaviour: Rows: Firmware type ("UEFI" or "Legacy BIOS"), BIOS vendor, BIOS version, BIOS date, BIOS release, Secure Boot ("Enabled", "Disabled" or `Not reported`).
- Linux data/mechanism:
  - `/sys/firmware/efi` presence gives the firmware type.
  - `/sys/class/dmi/id/{bios_vendor,bios_version,bios_date,bios_release}`, all world-readable.
  - Secure Boot from byte 4 of `/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c` (U-27).
- Verdict: BUILD
- Phase: P2
- Privilege: none

### PERF-102 Motherboard section
- Was: v2 PERF-091
- Source: TS Manufacturer, Product, Version
- Behaviour: Rows: Manufacturer, Product, Version, System vendor, System product, Chassis type, Serial number.
- Linux data/mechanism: `/sys/class/dmi/id/{board_vendor,board_name,board_version,sys_vendor,product_name,chassis_type}`. Serial from `board_serial`.
- Verdict: BUILD
- Phase: P2
- Privilege: `board_serial` is mode 0400 (verified). Unprivileged it shows `Restricted` with the tooltip "Needs administrator access." The shield does not change this, because the tl-helper allowlist (R-004) has no DMI read, and there is no pkexec prompt for it (inferred).
- Applied: R-050

### PERF-103 Graphics section
- Was: v2 PERF-092
- Source: TS Name, Driver Version, Driver Date, DirectX Version, Temperature, Temperature Source, Dedicated Memory, Shared Memory, Resolution
- Behaviour: One block per GPU: Name, Driver version, Kernel module (replaces Driver Date), Vulkan version and OpenGL version (replace DirectX Version), Temperature, Temperature source (PERF-069), Dedicated memory, Shared memory, Resolution (each display "3840x2160 @ 144 Hz", with ` (primary)` on the primary).
- Linux data/mechanism: Same as PERF-062 to PERF-073.
- Verdict: ADAPT. Same mapping as PERF-071.
- Phase: P2
- Privilege: none

### PERF-104 Network adapters section
- Was: v2 PERF-093
- Source: TS Connection, Details, IP Address, MAC Address, Public IP with fetch
- Behaviour: One block per visible interface: Connection (name and type), Details (adapter name, driver, link speed), IP address, MAC address. A final "Public IP" row has the Fetch button from PERF-060.
- Linux data/mechanism: Same as PERF-054 to PERF-060.
- Verdict: BUILD
- Phase: P2
- Privilege: none

## Additional setup

### PERF-105 Additional setup window
- Was: v2 PERF-094; content of sys-app APP-042 (R-028 rewrites APP-042 to point here)
- Source: MC first-run dialog ("Enabling Additional Values", Run Additional Setup, Revert, View Script, Copy Script Location, Copy Command, Copy Revert Command; success and failure dialogs)
- Behaviour:
  - There is no first-run dialog. One window titled "tasklens Additional Setup" opens from Help > "Additional setup…", from Settings > All settings > Privileges, and from every `Needs setup` link in the app.
  - Rows, each with status text, an "Enable" or "Revert" button where one applies, "View script" and "Copy command":
    - "CPU power draw" (PERF-106).
    - "Journal access": shows `sudo usermod -aG systemd-journal $USER` with a Copy button. tasklens never changes groups itself.
    - "Administrator helper": "Installed" or "Not installed" (`/usr/lib/tasklens/tl-helper`).
    - "Fan sensors" (PERF-108, P4).
    - "Per-process network": text only (PERF-107).
  - Statuses: "Enabled", "Enabled (existing rule)", "Not enabled", "Not needed".
  - Success shows "Done. No restart needed." and tasklens re-probes the source at once.
  - Failure shows the script output with "Copy output" and "Retry".
- Linux data/mechanism: A fixed root-owned script shipped at `/usr/lib/tasklens/tasklens-setup` with subcommands `rapl-enable`, `rapl-revert`, `sensors-detect` and `polkit-rule on|off` (the last is used by the P4 privilege settings, APP-049). It runs as `pkexec /usr/lib/tasklens/tasklens-setup <subcommand>` under the polkit action `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`). No script is written to `/tmp`. `state.first_run_done` does not exist.
- Verdict: ADAPT. The first-run dialog becomes an on-demand window, and the temporary script becomes a packaged one.
- Phase: P3 (Fan sensors row P4)
- Privilege: pkexec with `io.github.oysteinkrog.tasklens.setup`. If cancelled, the row keeps its old status.
- Applied: R-001, R-002, R-028

### PERF-106 RAPL udev rule
- Was: v2 PERF-095
- Source: MC setup script step 2 (`/etc/udev/rules.d/99-powercap.rules`)
- Behaviour:
  - Status "Enabled" when `/etc/udev/rules.d/99-tasklens-powercap.rules` exists and `energy_uj` is readable. Status "Enabled (existing rule)" when `energy_uj` is readable and the tasklens file is absent, as on this machine through Mission Center's rule. tasklens never touches `99-powercap.rules`.
  - "Enable" first shows: "Lets every local user read CPU energy counters. This can leak information about other programs (CVE-2020-8694)." with "Enable" and "Cancel".
  - "Revert" removes the tasklens rule and re-triggers. It says that a reboot may be needed before the files return to mode 0400.
- Linux data/mechanism:
  - `rapl-enable` writes `/etc/udev/rules.d/99-tasklens-powercap.rules` with one line: `SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", ACTION=="add", RUN+="/usr/bin/chmod 0444 /sys%p/energy_uj"`, then runs `udevadm control --reload` and `udevadm trigger --subsystem-match=powercap --action=add`.
  - `rapl-revert` removes the file and runs the same two commands.
- Verdict: ADAPT
- Phase: P3
- Privilege: root through pkexec. Without it, CPU power shows `Needs setup` (U-37).
- Applied: R-029

### PERF-107 Per-process network row (nethogs capabilities dropped)
- Was: v2 PERF-096
- Source: MC setup script step 1 (`setcap cap_net_admin,cap_net_raw,cap_dac_read_search,cap_sys_ptrace+pe` on nethogs)
- Behaviour: Mission Center's nethogs step is not offered. The "Per-process network" row only explains that tasklens attributes TCP traffic per process through sock_diag with no setup, and that exact numbers for all traffic need the later network helper (`tl-netmon`, P4).
- Linux data/mechanism: Per-process network uses sock_diag; the process and connections specs own it. `tl-netmon` will use the reserved polkit action `io.github.oysteinkrog.tasklens.netmon`.
- Verdict: DROP (the nethogs capability step). tasklens does not run nethogs, scraping its output is fragile, and capabilities on a general tool are too broad. The per-process network feature itself stays.
- Phase: P3 (the row text only)
- Privilege: none

### PERF-108 Fan sensor detection
- Was: v2 PERF-097
- Source: MC setup script step 3 (`sensors-detect --auto`)
- Behaviour:
  - Status "Not needed" when any hwmon fan exists (as here: nct6797 is loaded).
  - Otherwise "Run sensor detection" runs it and shows the output. The modules it finds are written to `/etc/modules-load.d/tasklens-sensors.conf` and loaded with `modprobe`. "Revert" removes that file.
  - When lm_sensors is not installed, the row reads "Install lm_sensors to detect fan sensors" and has no button.
- Linux data/mechanism: `tasklens-setup sensors-detect` runs `sensors-detect --auto`, parses its `MODULE_n` output, writes the file and runs `modprobe`.
- Verdict: ADAPT. tasklens always writes the module file, so the result survives a reboot.
- Phase: P4
- Privilege: root through pkexec. Without it: no fan pages.

## Summary table

| ID | Name | Verdict | Phase |
|---|---|---|---|
| PERF-001 | Device sidebar cards | BUILD | P1/P2 |
| PERF-002 | Hide and show page context menu | BUILD | P1 |
| PERF-003 | Sidebar edit mode | BUILD | P2 |
| PERF-004 | Per-type default visibility and network groups | ADAPT | P1/P2 |
| PERF-005 | Device hotplug and stable keys | BUILD | P1/P2 |
| PERF-006 | Sidebar width splitter | BUILD | P1 |
| PERF-007 | Performance keys, View submenu and sidebar toggle | BUILD | P1/P2 |
| PERF-008 | Device type names, headers and empty states | ADAPT | P1/P2/P4 |
| PERF-009 | Aggregate pages | BUILD | P4 |
| PERF-010 | Graph summary mode | ADAPT | P2 |
| PERF-011 | Copy device info | BUILD | P2 |
| PERF-012 | Page layout | BUILD | P1 |
| PERF-013 | Fallback value words | ADAPT | P1 |
| PERF-014 | Performance-tab toolbar contents | BUILD | P1 |
| PERF-015 | Graph element | BUILD | P1 |
| PERF-016 | Time range | ADAPT | P1/P2 |
| PERF-017 | Series rings and the 1 Hz tier | BUILD | P1/P2 |
| PERF-018 | Scaling modes | ADAPT | P1 |
| PERF-019 | Axis maximum rounding and label | BUILD | P1 |
| PERF-020 | Smooth (sliding) graphs | ADAPT | P1 |
| PERF-021 | Curved lines | ADAPT | P4 |
| PERF-022 | Gaps and stale data | BUILD | P1 |
| PERF-023 | Hover readout | BUILD | P2 |
| PERF-024 | Graph colours | BUILD | P1/P3 |
| PERF-025 | Units and number formats | ADAPT | P1/P2 |
| PERF-026 | Legends and secondary series | BUILD | P1 |
| PERF-027 | CPU header and graph mode | ADAPT | P1/P2 |
| PERF-028 | Per-core grid | BUILD | P1 |
| PERF-029 | Kernel time overlay | BUILD | P1 |
| PERF-030 | Overlaid and stacked modes | BUILD | P2 |
| PERF-031 | Core type marks | ADAPT | P2 |
| PERF-032 | CPU secondary graph | BUILD | P2 |
| PERF-033 | CPU live stats | ADAPT | P1/P2 |
| PERF-034 | Up time format | BUILD | P1 |
| PERF-035 | CPU static stats | BUILD | P1/P2 |
| PERF-036 | Cache size format | ADAPT | P1 |
| PERF-037 | Virtualization and security rows | ADAPT | P2 |
| PERF-038 | CPU power (RAPL) | BUILD | P2 |
| PERF-039 | CPU temperature | BUILD | P1 |
| PERF-040 | CPU clock | BUILD | P1 |
| PERF-041 | Memory graphs | BUILD | P1 |
| PERF-042 | Memory composition bar | ADAPT | P1 |
| PERF-043 | Memory stats | ADAPT | P1/P2 |
| PERF-044 | zram and zswap details | ADAPT | P2 |
| PERF-045 | Swap devices list | ADAPT | P2 |
| PERF-046 | Memory modules on the Memory page | BUILD | P2 |
| PERF-047 | Disk list and kind | ADAPT | P1 |
| PERF-048 | Disk graphs | BUILD | P1 |
| PERF-049 | Disk stats | ADAPT | P1/P2 |
| PERF-050 | Partitions table | ADAPT | P2 |
| PERF-051 | SMART data dialog | BUILD | P2/P3 |
| PERF-052 | Eject with blocking processes | BUILD | P2 |
| PERF-053 | Disk temperature | BUILD | P2 |
| PERF-054 | Interface list and type | ADAPT | P1 |
| PERF-055 | Throughput graph and legend | BUILD | P1 |
| PERF-056 | Network stats | BUILD | P1/P2 |
| PERF-057 | Display unit row | BUILD | P1 |
| PERF-058 | Wi-Fi details | ADAPT | P2 |
| PERF-059 | IP addresses | ADAPT | P2 |
| PERF-060 | Public IP fetch | BUILD | P2 |
| PERF-061 | Network settings link | ADAPT | P3 |
| PERF-062 | GPU enumeration and tl-gpu-probe | ADAPT | P1 |
| PERF-063 | Engine graphs | ADAPT | P1/P2 |
| PERF-064 | GPU memory | BUILD | P1 |
| PERF-065 | Video encode and decode | BUILD | P1 |
| PERF-066 | GPU clocks | BUILD | P2 |
| PERF-067 | GPU power | BUILD | P2 |
| PERF-068 | GPU temperature and fan | BUILD | P1/P2 |
| PERF-069 | GPU temperature source values | ADAPT | P1/P2 |
| PERF-070 | PCIe link | BUILD | P2 |
| PERF-071 | GPU static fields | ADAPT | P2 |
| PERF-072 | Displays list | ADAPT | P2/P4 |
| PERF-073 | Display connector type names and row format | ADAPT | P2 |
| PERF-074 | AMD GPU backend | ADAPT | P2 |
| PERF-075 | Intel GPU backend | ADAPT | P3 |
| PERF-076 | Fan pages | BUILD | P2 |
| PERF-077 | Fan labels and temperature pairing | ADAPT | P3 |
| PERF-078 | Battery and power device pages | BUILD | P2 |
| PERF-079 | Battery charge history | BUILD | P3 |
| PERF-080 | Minigraph panel and system cards | BUILD | P1 |
| PERF-081 | Minigraph card context menu | BUILD | P1/P3 |
| PERF-082 | Move and reorder cards | BUILD | P2 |
| PERF-083 | Detached graph windows | ADAPT | P3 |
| PERF-084 | Keep detached graphs open | BUILD | P3 |
| PERF-085 | Process-source cards and labels | BUILD | P3 |
| PERF-086 | Minigraph states "No match" and "Ended" | BUILD | P3 |
| PERF-087 | Column labels for process cards | ADAPT | P3 |
| PERF-088 | Tray icon graph | ADAPT | P3 |
| PERF-089 | Tray tooltips | BUILD | P3 |
| PERF-090 | Tray menus and graph colour | BUILD | P3 |
| PERF-091 | Main tray icon while graphs show | BUILD | P3 |
| PERF-092 | Tray icon re-registration | ADAPT | P3 |
| PERF-093 | Performance mini window | ADAPT | P3 |
| PERF-094 | Information layout, copy and filter | BUILD | P2 |
| PERF-095 | Operating System section | ADAPT | P2 |
| PERF-096 | Processor section | BUILD | P2 |
| PERF-097 | System section | ADAPT | P2 |
| PERF-098 | Memory section | ADAPT | P2 |
| PERF-099 | Memory modules section | BUILD | P2 |
| PERF-100 | Disk drives section | ADAPT | P2 |
| PERF-101 | BIOS/Firmware section | BUILD | P2 |
| PERF-102 | Motherboard section | BUILD | P2 |
| PERF-103 | Graphics section | ADAPT | P2 |
| PERF-104 | Network adapters section | BUILD | P2 |
| PERF-105 | Additional setup window | ADAPT | P3/P4 |
| PERF-106 | RAPL udev rule | ADAPT | P3 |
| PERF-107 | Per-process network row (nethogs capabilities dropped) | DROP | P3 |
| PERF-108 | Fan sensor detection | ADAPT | P4 |

## Old ID to new ID map

Every ID in `spec/features-perf.md` (v2) and `spec/features-perf-addendum.md` (add), and the unnumbered addendum amendments.

| Old ID | New ID | Old ID | New ID |
|---|---|---|---|
| v2 PERF-001 | PERF-001 | v2 PERF-050 | PERF-055 |
| v2 PERF-002 | PERF-002 | v2 PERF-051 | PERF-056 |
| v2 PERF-003 | PERF-003 | v2 PERF-052 | PERF-058 |
| v2 PERF-004 | PERF-004 | v2 PERF-053 | PERF-059 |
| v2 PERF-005 | PERF-005 | v2 PERF-054 | PERF-060 |
| v2 PERF-006 | PERF-006 | v2 PERF-055 | PERF-061 |
| v2 PERF-007 | PERF-007 | v2 PERF-056 | PERF-062 |
| v2 PERF-008 | PERF-009 | v2 PERF-057 | PERF-063 |
| v2 PERF-009 | PERF-010 | v2 PERF-058 | PERF-064 |
| v2 PERF-010 | PERF-011 | v2 PERF-059 | PERF-065 |
| v2 PERF-011 | PERF-012 | v2 PERF-060 | PERF-066 |
| v2 PERF-012 | PERF-015 | v2 PERF-061 | PERF-067 |
| v2 PERF-013 | PERF-016 | v2 PERF-062 | PERF-068 |
| v2 PERF-014 | PERF-017 | v2 PERF-063 | PERF-070 |
| v2 PERF-015 | PERF-018 | v2 PERF-064 | PERF-071 |
| v2 PERF-016 | PERF-019 | v2 PERF-065 | PERF-072 |
| v2 PERF-017 | PERF-020 | v2 PERF-066 | PERF-074 |
| v2 PERF-018 | PERF-021 | v2 PERF-067 | PERF-075 |
| v2 PERF-019 | PERF-022 | v2 PERF-068 | PERF-076 |
| v2 PERF-020 | PERF-023 | v2 PERF-069 | PERF-077 |
| v2 PERF-021 | PERF-024 | v2 PERF-070 | PERF-078 |
| v2 PERF-022 | PERF-025 | v2 PERF-071 | PERF-079 |
| v2 PERF-023 | PERF-026 | v2 PERF-072 | PERF-080 |
| v2 PERF-024 | PERF-027 | v2 PERF-073 | PERF-081 |
| v2 PERF-025 | PERF-028 | v2 PERF-074 | PERF-082 |
| v2 PERF-026 | PERF-029 | v2 PERF-075 | PERF-083 |
| v2 PERF-027 | PERF-030 | v2 PERF-076 | PERF-084 |
| v2 PERF-028 | PERF-031 | v2 PERF-077 | PERF-085 |
| v2 PERF-029 | PERF-032 | v2 PERF-078 | PERF-088 |
| v2 PERF-030 | PERF-033 | v2 PERF-079 | PERF-089 |
| v2 PERF-031 | PERF-035 | v2 PERF-080 | PERF-090 |
| v2 PERF-032 | PERF-037 | v2 PERF-081 | PERF-091 |
| v2 PERF-033 | PERF-038 | v2 PERF-082 | PERF-093 |
| v2 PERF-034 | PERF-039 | v2 PERF-083 | PERF-094 |
| v2 PERF-035 | PERF-040 | v2 PERF-084 | PERF-095 |
| v2 PERF-036 | PERF-041 | v2 PERF-085 | PERF-096 |
| v2 PERF-037 | PERF-042 | v2 PERF-086 | PERF-097 |
| v2 PERF-038 | PERF-043 | v2 PERF-087 | PERF-098 |
| v2 PERF-039 | PERF-044 | v2 PERF-088 | PERF-099 |
| v2 PERF-040 | PERF-045 | v2 PERF-089 | PERF-100 |
| v2 PERF-041 | PERF-046 | v2 PERF-090 | PERF-101 |
| v2 PERF-042 | PERF-047 | v2 PERF-091 | PERF-102 |
| v2 PERF-043 | PERF-048 | v2 PERF-092 | PERF-103 |
| v2 PERF-044 | PERF-049 | v2 PERF-093 | PERF-104 |
| v2 PERF-045 | PERF-050 | v2 PERF-094 | PERF-105 |
| v2 PERF-046 | PERF-051 | v2 PERF-095 | PERF-106 |
| v2 PERF-047 | PERF-052 | v2 PERF-096 | PERF-107 |
| v2 PERF-048 | PERF-053 | v2 PERF-097 | PERF-108 |
| v2 PERF-049 | PERF-054 | | |

| Old ID | New ID |
|---|---|
| add amend PERF-069 (mini window trigger) | PERF-093 |
| add amend (GPU page engine graphs) | PERF-063 |
| add amend (GPU displays list) | PERF-072 |
| add amend (Public IP fetch) | PERF-060 |
| add amend (sidebar device cards) | PERF-001 |
| add amend (Information tab, System section, User Domain) | PERF-097 |
| add amend (CPU page, Security services row) | PERF-037 |
| add PERF-079 Fallback value wording | PERF-013 |
| add PERF-080 Up time format | PERF-034 |
| add PERF-081 Cache and fixed-size formats | PERF-036 |
| add PERF-082 Device type names and empty states | PERF-008 |
| add PERF-083 Network "Display unit" field | PERF-057 |
| add PERF-084 Network graph title and Upload/Download legend | PERF-055, PERF-056 |
| add PERF-085 Memory module blocks on the Memory page | PERF-046 |
| add PERF-086 Minigraph card labels | PERF-080, PERF-085 |
| add PERF-087 Minigraph states "No match" and "Ended" | PERF-086 |
| add PERF-088 Column-graph labels for memory and GPU columns | PERF-087 |
| add PERF-089 Tray graph "Graph color" menu item | PERF-090 |
| add PERF-090 Tray icon re-registration | PERF-092 |
| add PERF-091 Display connector type names | PERF-073 |
| add PERF-092 GPU temperature source values | PERF-069 |
| add PERF-093 Performance-tab toolbar contents | PERF-014 |
| v2 dangling reference PERF-101 (RAPL setup) | PERF-106 |
| v2 dangling reference PERF-103 (fan sensor setup) | PERF-108 |
| table b rows F1..F8, F9/Ctrl+T, Ctrl+M, Ctrl+C (Performance), Up/Down (sidebar) | PERF-007 |
| sys-app APP-042 content (R-028) | PERF-105 |

Nothing from either source was dropped. The only DROP verdict is PERF-107 (the nethogs capability step), which v2 had already dropped; its panel text stays.

v2's "Conflicts with the architecture review" list is settled as follows: item 1 by R-016 (PERF-016, PERF-017); item 2 by R-026 (PERF-062); item 3 by PERF-084 and ADR-010's close rule; item 4 by R-046 and R-073 (PERF-083, PERF-093); item 5 by the `curved_lines` key in table (a) (PERF-021); item 6 by R-087 (PERF-071); item 7 by R-030, R-057 and U-26 (PERF-046, PERF-072); item 8 by ADR-008's demand rules, with the hidden-CPU budget measured with one tray graph active.

## Open items

Nothing. The open items of this merge were settled in `docs/RECONCILIATION.md`:

| Item | Resolution | Entries |
|---|---|---|
| 1. State keys for detached graph windows | R-084 | PERF-083 |
| 2. Choosing which GPU a minigraph card follows | R-085 | PERF-080 |
| 3. Detached and mini window matching through the KWin bridge | R-086 | PERF-083, PERF-093 |
| 4. `tl-gpu-probe --api-versions` | R-087 | PERF-071 |
| 5. SMART fallback through `pkexec smartctl` | R-088 | PERF-051 |
| 6. Phase of View > Graphs | R-089 | PERF-080 |
| 7. Sidebar toggle state | R-090 | PERF-007 |

Unverified facts this area depends on: U-25 (udisks2 NVMe module), U-26 (sysfs `enabled`), U-27 (lockdown, efivars and iommu readable without root), U-28 (`cpuinfo_avg_freq`, `amd_pstate_prefcore_ranking`), U-29 (seat ACL for KMS ioctls), U-30 (`ureq` without Tokio), U-31 (ksni without Tokio), U-33 (Wayland clipboard), U-35 (NVML maxima for fixed scales), U-37 (Mission Center's rule survives reboot). Phase 0 or the first bead that touches each one confirms it.
