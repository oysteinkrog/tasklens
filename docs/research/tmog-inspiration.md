# TMOG (tmog.org) Research for tasklens, 2026-10-07

### 1. What TMOG is, and the main lessons

TMOG ("Task Manager OG") is a closed-source system monitor by Dave Plummer, who wrote the original Windows Task Manager. Version 1.0.0 shipped in 2026 for macOS, Windows and Linux. One C++20 core does the measuring, and the Linux app uses Qt 6. The base edition is free and TMOG Pro is paid. The only public repo is the issue tracker, PlummersSoftwareLLC/tmog-feedback. The old davepl/tmog-feedback name redirects there.

Main lessons:
1. **Users complain most about what TMOG costs to run.** One report: about 30 W of laptop GPU power (#1275). Another: 16 to 20% GPU at idle on the Summary page (#1226). A third: graphs drop to 1 to 4 fps while the mouse moves over them (#1272). tasklens has the same two weak spots in code: charts redraw every vsync, and hover notifies on every mouse move. These are the cheapest and largest wins.
2. **TMOG drops bad entries in a skin file one by one and keeps the rest.** tasklens should use this rule for saved Overview layouts before any saved file exists.
3. **The Summary page has two tiers.** Large boxes with charts sit at the top. Small tiles with no chart sit below them, each with a title, a value, one text line and one meter. A Top CPU processes list sits next to the CPU chart. For tasklens this suggests that box size, not a user setting, should decide how much detail a box draws.
4. **Small charts with no scale cannot be read.** Users keep asking for y-axis values (#1334, #1300, #1245). On the planned Overview, tasklens's MIN_PLOT_H = 100 would hide the scale on most charts.
5. **TMOG has no layout editor to copy.** The skin guide says "A skin paints the layout; it does not rearrange it." The tasklens editor should build on its own sidebar edit mode (UI-PRF-013).

### 2. Ideas worth keeping, ranked

Duplicates across facets are merged into one row.

| # | Idea | What to do in tasklens | Value | Effort | Perf risk | Evidence |
|---|---|---|---|---|---|---|
| 1 | Pace chart scrolling by physical pixel, not by vsync | In anim.rs, chart/element.rs and chart/grid_element.rs, a moving chart reports how often it moves one physical pixel. The driver takes the fastest chart and draws no faster than that. Example: a 60 s chart 1760 px wide needs about 29 fps, and at 10 min about 3 fps. Two ways to build it (see section 6). Snap dx to whole pixels. Stall detection must count from when the frame is armed. Link it to wlf.28. No user setting until it has been compared side by side on a 144 Hz screen. | high | small to medium | Lowers cost. Watch for visible stepping at 0.5 px. | https://tmog.org/rtm/release-notes.html , https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1275 |
| 2 | Hover notifies only when the hovered sample changes | In element.rs register_hover_events (lines 175 to 190) and grid_element.rs (about line 711), snap the pointer time to the nearest sample before comparing, using a new hover::snap_time with no allocation. Tests: hover_move_within_one_sample_does_not_notify and hover_pin_is_a_sample_time. Add a pointer-sweep render test (see section 3). | high | small | Lowers cost. Today one mouse move can dirty every ancestor view (crw.34). | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1272 |
| 3 | Lenient loader for saved Overview layouts | In layout.rs, add `load(raw) -> (OverviewLayout, Vec<LayoutIssue>)` next to the strict `validate`. Drop unknown kinds, repeated kinds (the first one wins), boxes outside the grid and overlapping boxes. Clamp columns, row weight 1..=10 and min_h 120..=600. Fall back to btop_default only when no box is left. Store `format: 1` and put serde(default) on every field. Ignore an unknown optional `style` key. Log each issue with tracing::warn. A Settings list can come later. | high | small | None. It runs once at load. | https://tmog.org/rtm/skins.html |
| 4 | Sort and heat Processes on the shown value | In tl-engine reducer/table.rs (line 110 uses total_cmp on the raw value), compare the displayed step first (0.1% for CPU, the shown unit for memory and rates), then ProcessKey. Quantize heat alpha to 2% steps. Amend PROC-047 and UI-PRC-004. | high | small | Lowers cost: fewer row moves and fewer re-shaped strings at idle. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1278 |
| 5 | Show the scale on small Overview charts | Add `ChartLabels.compact_axis` in labels.rs: the ceiling ("4 MiB/s") as one t_xs line in the plot's top-left corner, for plots at least 32 px × ui_scale tall. Show the range ("60 s") once per box. | high | small | One cached ShapedLine, re-shaped only when the ceiling changes. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1334 |
| 6 | Click a box title to open its device page | A single click on the 24 px title row dispatches PerfSelectDevice. A disk row opens that disk. NET opens the aggregate page, or the first visible adapter. No back stack, because F1 already returns to Overview. Charts get no click handler. | high | small | Static hitboxes only, with no hover notify. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1277 |
| 7 | Disk and GPU boxes fit their row count to the box height | Set MIN_BOX_CHART_H to about 48 px × ui_scale. Disk shows as many rows as fit (up to 8), then "+n more". GPU shows a second section only when both fit. | medium | small | Changes only on resize or hotplug. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1311 |
| 8 | Busiest core and per-CCD rows in the CPU box | Add "max n%" after the total. When CpuStaticInfo has more than one l3_group, add one meter row per CCD (with Tccd when hwmon has it). Data comes from per-core samples the box already reads. | medium | small | Low. Uses the existing 1% bucket rule. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1313 |
| 9 | Size-driven detail levels, with Compact as the smallest | Give each BoxKind a static table of levels with minimum (w, h). Example for Memory: Full, Medium (chart plus one footer line), Compact (title, one line, one Meter, no ChartView). `place()` writes the level into PlacedBox. A box drops or creates its ChartView only when its level changes. Pin the thresholds in tests, as the 560 px rule is pinned. This is a level, not a `form` field the user sets. | high | medium | Low. The choice happens at resize time. Compact is cheaper than Full. | https://tmog.org/rtm/assets/screenshots/summary.webp |
| 10 | One slow source must not stall the others | Add a LabRuntime test: four sources whose sample() blocks forever, and Cpu must keep arriving at 10 Hz. Check whether journal Follow holds a pool thread (ARCHITECTURE line 2353). If the test fails, an owner decision on ADR-013 follows (for example a reserved thread for Cpu and Memory). | medium | small | No render cost. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1004 |
| 11 | Colour each Overview box by channel | Add `box_border[5]` and `box_bg[5]` to theme Derived, computed in rebuild_derived. Add `value_text[5]`, stepped in lightness until it reaches 4.5:1 contrast, for the headline value. Add a 16 px kit icon (cpu, memory-stick, gpu, hard-drive, network) in the title. Extend the contrast_floor tests. Do not add a settings switch. | medium | small | None per frame. Colours are computed on theme switch. | https://tmog.org/rtm/assets/screenshots/summary.webp , https://tmog.org/rtm/assets/screenshots/theme-light.webp |
| 12 | A second layout fixture for the `place()` tests | Add a test-only `summary_fixture()`: 4 rows, one span-3 row with min_h 140, tested at 1200×900, at 700 px stacked, and with a row whose only box is absent. No UI and no menu entry. | medium | small | None. | https://tmog.org/rtm/assets/screenshots/summary.webp |
| 13 | Lower cost on battery | Have the UPower worker publish OnBattery. A setting "Save power on battery" turns off cpu_fast and smooth sliding while on battery, applied only when OnBattery changes. Needs an owner decision on the default. | medium | small | Lowers cost. | https://github.com/PlummersSoftwareLLC/tmog-feedback/issues/1275 |
| 14 | Overview edit mode built on UI-PRF-013 | Add "Edit layout" to the page menu. Each box shows a grip, a hide switch and a span stepper over a flat placeholder, and live box views are not mounted. Drag a box onto another to swap, Alt+Arrow moves the focused box, Reset restores btop_default, Done or Esc saves through `load()`. | high | medium | Zero when off. No chart geometry is built while editing. | https://tmog.org/rtm/skins.html (shows TMOG has no editor) |
| 15 | Opt-in Top processes box | Add `BoxKind::TopProcs`, left out of btop_default. 10 rows from a limited top-N query at 1 Hz, notify only when the hash of the rows changes, click selects the process on the Processes tab. The process source joins Overview demand only while the box is placed. Extends D-022, so it needs an owner decision. | high | medium | Medium: about 34 ms of /proc sweep per tick at 1,000 processes (yms.26) while placed. | https://tmog.org/rtm/assets/screenshots/summary.webp |
| 16 | Segmented meter style (low priority, facets disagree) | Add `MeterStyle { Solid, Segmented }` at page level, defaulting to Solid. Draw two paths per meter (lit and unlit), not one quad per segment. Unlit colour is the line colour blended toward box_bg. It saves no repaints, because the value text next to the meter changes anyway. | low | medium | Low with paths. Must be measured in perf-overview first. | https://tmog.org/rtm/skins.html |
| 17 | "Now" marker at the end of each trace, Overview only (facets disagree) | `head_marker(bool)` on the chart element: a 5 px triangle at the last drawn point (at right_edge_ns, not at the newest value, because of ADR-009 rule 5). Only the Overview charts set it. | low | small | One small path per series. | https://tmog.org/rtm/assets/screenshots/network.webp |

### 3. Items that touch the Overview beads (crw.36 to crw.43) and the future layout editor

**Change a filed bead now:**
- **crw.39 (GPU, Disk and Net boxes).** Add row 7 (fit rows to the box height) to its acceptance criteria. Add the compact axis label from row 5 to the Net, Disk and GPU charts.
- **crw.38 (CPU box).** Add row 8 (max core and CCD rows) if crw.38 is still open, otherwise file it as a follow-up. Add compact_axis to the CPU chart.
- **crw.43 (perf-overview bench).** Add a pointer-sweep test, `overview_pointer_sweep_renders_nothing`: about 200 moves over every box, then zero renders of the page and boxes and no geometry rebuild. Add a static variant held to the 2% gate. Add an fps check (at most the pixel rate plus 10%) once row 1 lands.
- **UI-PRF-017 text.** Add one line to the render contract: "Pointer movement never notifies a box or the page." Add the click rule from row 6. Write the lenient-load rules from row 3 into the "Not yet: saving the layout" bullet, so the save bead starts with them.

**Do not touch crw.37 now.** It is in progress and owns parts.rs. Box tint and icon (row 11), segmented meters (row 16) and the head marker (row 17) go into follow-up beads after crw.37 closes.

**File as new beads now (small, no owner decision needed):**
- Row 2 (hover snap) and row 1 (pixel pacing). Both go under wlf, linked to wlf.28. Row 1 changes ADR-009, so owner sign-off comes first.
- Row 12 (second `place()` fixture), after crw.37's layout.rs exists.
- Row 6 (title click), after crw.38, crw.39 and crw.42.

**Wait:**
- Row 9 (detail levels): file it as an E24 follow-up after crw.37 to crw.40 and before the editor bead. Add a spec note to UI-PRF-017 now.
- Row 3 (lenient loader): becomes part of the save bead when that bead is filed.
- Row 14 (edit mode): wait for the save bead and detail levels. It becomes a new UI-PRF-018.
- Rows 15 and 13 (Top processes box, battery saver): owner decisions first. Both change D-022 or the demand in crw.40.
- Per-box style (chart or meters), segmented meters and layout sharing as JSON: revisit once the editor exists.

### 4. What not to copy from TMOG

- **Drawing at a fixed 60 fps.** Users report 30 W on a laptop GPU (#1275), 16 to 20% GPU at idle on Summary (#1226), and 5 to 15% CPU on laptops (#1049, #1029). The adjustable frame rate came later as a patch.
- **Re-rendering on every mouse move.** Graphs stutter under the pointer (#1272, #962).
- **Charts with no y-axis values.** Network, disk and power graphs cannot be read (#1334, #1300, #1245), and nothing shows how old the left edge is (#1321).
- **One slow provider stalling every metric.** A drive query hung on empty card readers and froze all graphs for about 20 s at a time (#1004).
- **Showing fixed wrong values despite the "no fake values" claim.** Users see constant CPU temperatures of 17, 20 and 28 °C from board or ACPI sensors (#1306, #1292, #1333). tasklens already avoids this with CPU_TEMP_CHIPS.
- **Totals that count inactive NICs.** The Summary network capacity bar includes interfaces that are down (#1301).
- **Squeezing many graphs at high UI scale.** At 300%, 17 GPU engine graphs become unreadable 12 px strips (#1311).
- **Bloom and glow.** Each costs an extra blur pass per frame, and "Visual Bloom" caused a graph bleed bug (#1031).
- **Treating the skin format as a layout system.** It only paints. It is also written for macOS only.
- **Reading "closed" as "fixed."** The maintainer closes most issues with "moved to our internal engineering team… This closure does not mean the work has been completed."

### 5. Ideas dropped

- **Per-box `form: Full | Compact` setting:** replaced by size-driven detail levels (row 9).
- **CPU temperature on a right-hand axis in the CPU chart:** temperature is already in the CPU text line and on the CPU page (UI-PRF-025). Two scales in one plot are easy to misread and add per-frame cost.
- **Heat colours on traces (green to red):** breaks the fixed device colours (UI-PRF-008, ADR-009 rule 6) and doubles path draws in the per-core grid.
- **Device page header meter:** goes against UI-PRF-020, which follows TaskSlinger.
- **Readout of tasklens's own cost:** already covered by the Processes row and the FPS overlay (UI-SHL-140). A live readout would re-render on every tick.
- **Rule against a zoomed memory axis:** tasklens already uses FixedTotal, so there is nothing to guard.
- **Process details strip:** duplicates the Properties window and the hover tooltip.
- **Up and Down in the filter box move the selection:** reopens the settled UI-SHL-024.
- **Process names on graph hover:** large, and depends on P3 process history.
- **Back stack (Alt+Left) and back link:** F1 already returns to Overview.
- **Sticky pause for charts:** the planned hover readout (UI-PRF-004) covers the need.
- **Copy and paste layouts as JSON:** depends on saving and the editor. Revisit later.
- **Phosphor themes:** D-012 sets the theme plan, and the theme editor can build one today.
- **Glow on meters:** costs a blur pass per meter at 10 Hz for decoration only.
- **Graph frame rate setting:** mostly redundant once pixel pacing lands.
- **Separate text readout rate:** reverses UI-PRF-023, and wlf.27 does not show that label renders are the cost.
- **New Overview bench bead:** crw.43 already covers it. Only the extra checks above are new.
- **Decaying network scale:** tasklens already uses stepped ceil_125 scales.
- **Health line on Overview:** duplicates the status chip (UI-SHL-051).
- **History scrub on Overview:** large, and depends on P3 tl-history.
- **One shared scale for download and upload:** would flatten upload, which is the complaint in #1250.
- **Re-find sensors after resume:** already covered by 5 s re-enumeration keyed by chip name and label.

### 6. What is uncertain

- **The facets disagree on how to build pixel pacing (row 1).** Two facets arm a background timer for the next pixel step. ADR-009 and ARCHITECTURE line 1858 reject app timers for redraws. The gaps facet instead keeps compositor callbacks and skips the notify, which still wakes the UI thread at 144 Hz (measure that a skipped callback costs under 50 µs). The owner picks one. "Looks the same as today" is a guess until a side-by-side check on a 144 Hz screen.
- **Hover cost and the missing scale are inferred from code and numbers, not measured.** Hover cost (row 2) comes from element.rs. That Overview plots fall under 100 px (row 5) is inferred, because the box beads are not built yet.
- **"60 fps" comes only from press.** tmog.org says "Selectable visual frame rates."
- **Every screenshot is the macOS build.** Whether skins work on the Linux build is not verified, and no TMOG resize behaviour is known.
- **The free versus Pro split is partly confirmed.** The app sidebar has a PRO group with Power & Freq, Benchmarks, Installed Apps and Disk Space. Connections does not appear, and the screenshots may predate 1.0.
- **What "flashing" in #1278 means is a guess.** The case for row 4 rests on tasklens's own code, not on that issue.
- **Numbers that need owner sign-off:** the clamp ranges in row 3 (one facet says columns 1..=4, another 1..=6) and the 48 px minimum chart height in row 7.
- **Not checked:** whether LabRuntime can model a blocked spawn_blocking job (row 10), whether the Linux path shader draws gradients, and whether the bench can inject pointer events.
- **Tracker size:** the tracker has 33 open issues, but numbers run past #1340.
- **Two URL errors on tmog.org.** The Linux notes show the udev rule as `/rtm/bin/sh`, which looks like a site bug. The /rtm/rtm/skins.html link given by two facets returns 404; the right URL is https://tmog.org/rtm/skins.html.
- **No new fetching for this report.** It only combines the facet results and their verdicts. No tasklens file was edited.