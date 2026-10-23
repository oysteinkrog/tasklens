# tasklens Implementation Plan: a native Linux task manager

Status: draft for review, 2026-10-02. `tasklens` is a working name.

## Goal

A native Linux task manager in the style of the Windows task managers, built for this desktop first:
CachyOS, KDE Plasma 6 on Wayland, an RTX 5070 Ti, 32 logical CPUs, three monitors at 110%
scale. UI and rendering speed come first:

- every graph scrolls smoothly at the monitor's refresh rate;
- the process table stays responsive with 2,000 live rows;
- the app costs almost nothing when it is idle or hidden.

TaskSlinger 0.9.2 under Wine was the main design reference. Its screenshots, demo videos and the
feature inventory are kept outside the repo, in `~/work/tasklens-ref/taskslinger/`. Section 9
lists the values the plan uses.

## 1. Stack

| Layer | Choice | Why |
|---|---|---|
| UI and rendering | **GPUI** (Zed's framework) with **gpui-component** | GPU-rendered (quads, sprites, paths on wgpu/Vulkan). Native Wayland with fractional scaling and client-side decorations. Redraws only on change. gpui-component has a virtualized data table and charts, and ships a live system monitor example. Used in production by Zed and Longbridge Pro. |
| Async runtime | **asupersync** 0.5 | Structured concurrency, cancellation and supervision for the collectors and actions. Deterministic lab runtime for tests. No tokio anywhere in the app. |
| System data | procfs, nvml-wrapper, netlink-packet-sock-diag + netlink-sys, zbus (async-io backend), rustix | All synchronous or runtime-agnostic. None pulls in tokio. |
| Fallback UI | egui/eframe with `egui_wgpu` paint callbacks | Used only if the phase 0 spike fails. |

**Known GPUI risks:**
1. **No custom shader hook upstream.** Phase 1 charts use GPUI's own quads and paths, which should be fast enough (estimate below). If profiling shows otherwise, switch to a GPUI fork that exposes wgpu textures (gpui-ce adds this) and draw all charts in one custom pass.
2. **No stable release.** Pin a git commit and upgrade on purpose. gpui-component ships its own GPUI snapshot crates; use those if they track recent Zed closely enough.
3. **Thin documentation.** Zed's source and the gpui-component examples serve as the docs.
4. **Untested here.** NVIDIA Blackwell, Wayland, wgpu Vulkan and three monitors together are unproven on this machine. Phase 0 tests exactly that.

**How asupersync and GPUI fit:** GPUI owns the main thread and its own executor. asupersync runs
on its own worker threads and owns all collection, history and actions. They meet at one bridge:

- asupersync publishes each new snapshot into a shared slot (`ArcSwap<Snapshot>`).
- It then sends a wake-up through a runtime-agnostic channel (`async-channel`).
- A GPUI task awaits that channel, reads the slot and calls `cx.notify()` on the views that changed.

The UI never blocks on collection, and collection never waits for a frame. Phase 0 must check
that an asupersync task can feed a channel that a GPUI task awaits.

## 2. Architecture

A Cargo workspace with five crates:

| Crate | What it does | Depends on |
|---|---|---|
| `tl-model` | Plain data types: snapshots, process records, fixed-size ring buffers for graph history, units and formatting | nothing heavy |
| `tl-collect` | Synchronous samplers, one per source: `/proc/stat`, `/proc/meminfo`, `/proc/<pid>/*`, `/proc/diskstats`, `/proc/net/dev`, NVML, sock_diag, cgroups, hwmon, cpufreq. Keeps file descriptors open and uses `pread` | tl-model |
| `tl-engine` | The asupersync side: a supervised collector per source with its own tick rate and budget, history buffers, the snapshot publisher, D-Bus (systemd units) and process actions | tl-model, tl-collect, asupersync, zbus |
| `tl-ui` | The GPUI app: window, title bar, tabs, views, chart element, theme system | tl-model, gpui, gpui-component |
| `tl-helper` | Later: a small privileged helper started through pkexec, for actions on other users' processes and system services | rustix |

**Engine shape (asupersync):**
- **App structure:** one `AppSpec` with a supervisor tree. Each collector is a supervised worker in its own region. If one crashes (for example NVML after a driver update), only that one restarts, and the UI shows the device as unavailable.
- **Ticks:** each collector runs a loop that wakes on asupersync timers and calls `cx.checkpoint()`. Blocking reads go through the runtime's blocking pool. Rates:
  - system totals: 10 Hz (CPU, memory, disk, network, GPU);
  - per-process data: the refresh rate set in settings (default 1 s);
  - connections and the socket-to-pid map: every 2 s.
- **Visibility:** the UI tells the engine which tab and rows are visible. Expensive data (PSS/USS memory, per-process GPU, connections) is collected only while it is on screen. When the window is hidden, only history recording keeps running.
- **Actions:** kill, suspend, priority, affinity and service start/stop are request/response calls with a deadline. Each one reports success or the exact error back to the UI, which shows it as a toast.
- **Process identity:** pid plus start time. All signals go through pidfds, so a reused pid can never be hit.
- **Tests:** engine scheduling, cancellation and restart behaviour run in asupersync's `LabRuntime`, which is deterministic. Samplers are tested against recorded `/proc` and `/sys` fixture trees.

**Chart element (GPUI):**
- Each series is a ring buffer of `f32` samples owned by the engine and copied into the snapshot.
- The area fill is one quad per sample column, which GPUI batches into a single instanced draw.
- The top line is a GPUI path built with lyon, rebuilt only when a sample arrives (at most 10 Hz).
- Smooth scrolling between samples is a sub-sample x offset per frame. Animation frames are requested only while a chart is visible and moving.
- Budget: 40 charts × 120 samples ≈ 5,000 quads plus 40 paths per frame. That should take well under 1 ms of CPU. Phase 0 measures it.

## 3. Data sources

Measured or checked on this machine where noted.

| Data | Source | Notes |
|---|---|---|
| CPU total, per core, kernel/user | `/proc/stat` | about 30 µs per read |
| CPU clock per core | `cpufreq/policyN/cpuinfo_avg_freq` (falls back to `scaling_cur_freq`) | amd-pstate-epp here |
| CPU temperature | hwmon `k10temp` | board sensors (`nct6797`) only at 1 Hz, they are slow |
| Memory, commit, swap, zram | `/proc/meminfo`, `/sys/block/zram0/mm_stat` | commit = `Committed_AS` / `CommitLimit` |
| NVIDIA totals | NVML | utilization, VRAM, temperature, power, clocks, encoder and decoder |
| NVIDIA per process | NVML process utilization and running processes | DRM fdinfo has no data on this driver (checked) |
| AMD and Intel GPUs | sysfs and DRM fdinfo | for portability; not tested here |
| Disks | `/proc/diskstats` | active time from `io_ticks` |
| Network per interface | `/proc/net/dev` | hide or group `veth*`, `docker0`, `tailscale0` |
| Per-process CPU and memory | `/proc/<pid>/stat`, `statm` | about 15 ms per sweep of 1,000 pids |
| Per-process PSS/USS | `/proc/<pid>/smaps_rollup` | expensive; visible rows only |
| Per-process disk | `/proc/<pid>/io` | own processes only; per-app totals from cgroup `io.stat` |
| Per-process network | sock_diag TCP info, mapped to pids | TCP only, approximate; exact numbers need eBPF and a privileged helper (later) |
| App grouping | cgroup scopes (`app-*.scope` under `user@1000.service`) | KDE gives each launched app its own scope (checked). Also enables freeze and kill without root |
| Services | systemd over D-Bus (zbus): system and user managers | system unit start/stop goes through polkit |
| Startup apps | XDG autostart folders and systemd user units | disable with a `Hidden=true` override |
| Connections | sock_diag dump, TCP and UDP, v4 and v6 | pid map from `/proc/*/fd`, rebuilt every 2 s |
| Information | `/proc/cpuinfo`, `/sys/devices/system/cpu`, `/sys/class/dmi/id`, NVML, udisks2 | serial numbers and DIMM details need root; show them as unavailable |
| Icons | desktop files and the KDE icon theme | cached |

Processes the app cannot inspect (root-owned, or kwin with `cap_sys_nice`) show a "restricted"
marker, not zeros.

## 4. Performance targets

These targets are checked in CI where possible and by hand on this machine otherwise:

| Measure | Target |
|---|---|
| First frame after launch | under 200 ms |
| CPU time per frame, Performance tab with 32 core graphs, 144 Hz | under 2 ms on the UI thread |
| Dropped frames while scrolling the 2,000-row table | none at 144 Hz |
| Collection cost at default rates | under 2% of one core |
| CPU use with the window hidden | under 0.3% of one core |
| Memory | under 150 MB resident |

A `--bench` mode renders fixed synthetic data for 30 s and prints frame-time percentiles, so
regressions show up as numbers.

## 5. Phases

Each phase ends with a working build installed through localbuilds and a short check against the targets.

**Phase 0: stack spike, about 2-3 days. Go/no-go gate for GPUI.**
- A GPUI window with a custom title bar, maximize, move between all three monitors at 110%.
- 40 live charts with synthetic data at 144 Hz, plus a 2,000-row gpui-component table updating at 1 Hz.
- The asupersync-to-GPUI bridge with a real `/proc/stat` collector.
- Measure frame times, idle CPU and startup. If GPUI fails a target or breaks on this hardware, switch to the egui fallback before writing more UI.

**Phase 1: Performance tab, about 1 week.**
- Device sidebar with mini graphs: CPU, memory, each GPU, each disk, each network interface.
- CPU page with the per-core grid, Overall/Per core and Kernel toggles, and the stats table.
- Memory, GPU (NVML), disk and network pages.
- Default Dark theme and the graph colours from section 9.

**Phase 2: Processes tab, about 1-2 weeks.**
- The virtualized table with the default columns, two-line headers showing system totals, sorting and resizable columns.
- Grouping into Apps, Background processes and system processes by cgroup, plus a tree view.
- Heat colouring, filter box that starts on typing, status bar counts, hold Ctrl to pause.
- Context menu actions: end task, end tree, suspend and resume (cgroup freeze), priority, affinity, open file location, copy.
- The floating mini graph panel.

**Phase 3: Services, Startup Apps, Connections, Information, about 1 week.**

**Phase 4: customization, about 1 week.**
- Theme system using TaskSlinger's colour slots, with the dark themes first.
- UI scale, row height, corner rounding, refresh rate, motion settings.
- Column choice per tab, saved settings.

**Phase 5: later, by priority.**
- History mode with a timeline.
- Process properties window (threads, open files, modules, environment).
- Tray graphs.
- The privileged helper for other users' processes and exact per-process network numbers.

Phases 0 to 3 give a daily-usable replacement in about 4-5 weeks of agent work. That is an
estimate, and phase 0 will sharpen it.

## 6. Repository and build

- New repo, Cargo workspace. Pinned Rust toolchain and pinned GPUI commit.
- Built and installed as a pacman package through a localbuilds recipe (`KIND=pkg`), like FrankenTerm and cass.
- Tests: unit tests in `tl-collect` against fixture trees, `LabRuntime` tests in `tl-engine`, a headless render test of the chart element, and the `--bench` frame-time run.

## 7. Risks

| Risk | What we do about it |
|---|---|
| GPUI breaks on this GPU, Wayland or multi-monitor setup | Phase 0 tests exactly this; egui is the fallback |
| GPUI quads and paths are too slow for 40 charts at 144 Hz | Measure in phase 0; then a GPUI fork with a custom wgpu pass |
| GPUI API churn | Pin a commit; upgrade only on purpose |
| asupersync and GPUI executors do not cooperate through the bridge | Phase 0 proves the bridge; worst case, a plain thread plus channel for the engine |
| asupersync is young (0.5) | Use the core runtime, timers, channels and supervision only; avoid its newer areas |
| Per-process network numbers are rough without eBPF | Label them as TCP-only; exact numbers come with the helper |
| Wayland gives no window-to-pid mapping to normal apps | Group by cgroup scope; the window picker needs a KWin script and comes later |
| NVML breaks after a driver update until reboot | The GPU collector restarts on its own and shows the GPU as unavailable |

## 8. Decisions needed

1. Project name.
2. Repo: public (oysteinkrog) or private. TaskSlinger is a commercial product, so a public repo should not use its name, logo or icons. Copying its layout and colours for a personal tool is a different matter from publishing it.
3. Whether v1 stops after phase 3 or includes phase 4.

## 9. Design reference

The full feature inventory, with what was seen in screenshots versus reported in reviews, is kept
outside the repo in `~/work/tasklens-ref/taskslinger/`. The parts the plan depends on:

**Default Dark palette:**

| Element | Colour |
|---|---|
| Window and list background | #191B1D |
| Alternate row | #1D1F20 |
| Title bar | #27292B |
| Tab bar and toolbar | #202224 |
| Column header band | #2E3235 |
| Dividers | #2B2F33 |
| Primary text | #D6D8D9 |
| Secondary text | #6C7072 |
| Accent | #0277A6 |
| Selected row | #075B84 |
| Heat tint at maximum | #3E2A28 |

**Graph colours:**

| Graph | Colour |
|---|---|
| CPU fill / line | #0D546D / #0A7096 |
| Kernel line | #C1514E |
| Memory fill | #7E6A9A |
| GPU fill / line | #508A59 / #71B768 |
| Disk | #D9B567 |
| Network | #359CAB |

**Layout:**
- About 28 px rows at 100% scale and 4-6 px corner radius.
- Material Symbols icons. Inter or Noto Sans in place of Segoe UI.
- The active tab is an accent outline joined to a 2 px accent line.
- Graphs are a filled area with a brighter top line and a faint vertical grid, scrolling right to left.
