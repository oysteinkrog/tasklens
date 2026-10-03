# PERF merged spec: Performance tab, graphs, minigraphs, tray, mini window, Information tab, additional setup

This file replaces `spec/features-perf.md` (v2) and `spec/features-perf-addendum.md` for implementation. It applies `docs/DECISIONS.md` (D-001 to D-010), `docs/RECONCILIATION.md` (R-001 to R-074 and canonical tables a to d) and `docs/ARCHITECTURE.md` (ADR-001 to ADR-016), in that order of precedence.

How to read the "Was:" lines:
- `v2 PERF-0xx` is an entry in `spec/features-perf.md`.
- `add PERF-0xx` is a new entry in `spec/features-perf-addendum.md`. The addendum used first-draft numbers, so its IDs clash with v2 IDs; the prefix tells them apart.
- `add amend (<subject>)` is an addendum amendment that had no ID of its own. `add amend PERF-069` is the mini window amendment (first-draft number).
- `table b` is a row of the canonical keyboard table in `docs/RECONCILIATION.md`.

v2 also points at `PERF-101` and `PERF-103`, which do not exist. `PERF-101` meant the RAPL setup (now PERF-106) and `PERF-103` meant sensor detection (now PERF-108).

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
  - F9 and Ctrl+T show or hide the device sidebar. The page then fills the width. The state is not persisted (inferred: no key exists in table a).
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

  The addendum wrote the Secure Boot GUID as `8be4df61-93ca-11d2-aa0d-e0c98e1ccb4a`; the EFI global variable GUID is `8be4df61-93ca-11d2-aa0d-00e098032b8c` (v2 and PERF-101 agree), so that form is used. Readability of lockdown, efivars and iommu without root is unverified (U-27). A source that cannot be read is left out of the list and named in the tooltip.
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
