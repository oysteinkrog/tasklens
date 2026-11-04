# Phase-0 Results

E01 owns this file. Each section records one measurement run.

## Start-up timing (tasklens-cr0.23, 2026-10-07)

Result: the warm median `first_frame_ms` is 365 ms after this bead, against 385 ms before it.
The ADR-014 warm gate is 200 ms. The gate cannot pass while gpui's GPU adapter probe takes
about 265 ms by itself. tasklens code before the window opens takes about 12 ms in total.

### Setup

- Owner desktop: KDE Plasma Wayland (`wayland-0`), RTX 5070 Ti, Vulkan, 32 logical CPUs.
- Release build of `tl-app`. `XDG_STATE_HOME` was a fresh folder under `/var/tmp/claude/` for
  every run, so each run started with no state and default settings.
- Each run waited for the `first frame` log line, then stopped the process by its PID.
- Other agents were building at the same time. The 1-minute load average was 25 to 30 during
  the before and after runs, and 37 to 53 during the per-step runs. All numbers here are noisy.
  The owner reruns the gate on an idle machine.
- `first_frame_ms` comes from `/proc/uptime`, so it has 10 ms resolution.

### Before and after

Runs alternated between the two binaries, three rounds of three runs each. The first run of
round 1 for each binary was a cold start (binary just copied, not in the page cache), and it
is left out of the warm median.

| Binary | Cold run | Warm runs (ms) | Warm median |
|---|---|---|---|
| Before (`bb18afc`) | 2660 | 540, 580, 390, 380, 360, 380, 440, 370 | 385 |
| After (`9515dab`) | 1060 | 430, 540, 370, 370, 360, 350, 360, 360 | 365 |

### Per-step times, after

From the `tasklens::perf` step lines. Each step's `ms` is the time since the previous step
ended. Medians of the 8 warm runs above.

| Step | What it covers | Median ms |
|---|---|---|
| `exec_to_main` | dynamic loader and static initialisers | 0 to 20 (10 ms resolution) |
| `logging` | paths, panic hook, log file | 2.4 |
| `args_settings` | arguments, NVIDIA rule, settings (defaults, no file I/O yet) | 0.1 |
| `engine_start` | `Engine::start` | 0.7 |
| `platform` | `gpui_kit::application()`: Wayland connection, gpui platform | 39.9 |
| `fonts` | bundled Noto faces added and checked | 2.5 |
| `kit` | `gpui_kit::init` | 5.5 |
| `ui_init` | theme, strings, hub, actions, open hooks | 0.9 |
| `window_open` | window, wgpu instance, adapter probe, renderer | 276 |
| `first_frame` | first layout and paint | 42 |

`window_open` split by gpui's own log lines (`gpui_wgpu::wgpu_context`):

| Part | Median ms |
|---|---|
| Window and wgpu instance (Vulkan and GL), then enumerating 3 adapters | 176 |
| Testing the RTX 5070 Ti with a real device and surface | 89 |
| Renderer set-up after the adapter is chosen | 19 |

### What changed

- `tasklens::perf` step lines in `crates/tl-app/src/main.rs`, `crates/tl-ui/src/app.rs` and
  `crates/tl-ui/src/window/main.rs` (`41b0299`).
- The placeholder logo `assets/icons/tasklens.svg` drew "tl" with an SVG `<text>` element.
  The first SVG with text makes gpui's SVG renderer load every system font on the main thread
  during the first frame: 1134 faces here, 36 to 47 ms warm and 1040 ms cold, measured with
  fontdb 0.23 on its own. It also logged three warnings: `IBMPlexSans-Regular.ttf` and
  `Lilex-Regular.ttf` not found, and no match for `monospace`. The logo now draws the glyphs
  of the bundled Noto Sans Mono Regular as one path (`9515dab`). The warnings are gone, and
  the cold start dropped from 2660 ms to 1060 ms in this run.

### What was not changed, and why

- Starting the engine on another thread while the window opens: `Engine::start` takes 0.7 ms,
  so running it in parallel would save less than the 10 ms resolution of `first_frame_ms`.
- Settings load: tl-app still uses default settings and reads no file before the window
  opens. E33-A2 adds the settings load. It must stay small or move off the window path.
- Registering Noto as gpui's SVG fallback font: gpui asks the asset source for its own IBM
  Plex Sans and Lilex paths and maps generic families to those names. Serving Noto bytes at
  those paths would not satisfy the `monospace` lookup. With no SVG text left, gpui never
  makes the lookup.

### Adapter probe time (finding)

The adapter probe inside gpui takes about 265 ms (176 + 89) on this machine. That alone is
over the 200 ms warm gate. tasklens cannot change it without patching gpui. Two causes in
`gpui-pre-wgpu` 0.3.7 (`wgpu_context.rs`):

- `WgpuContext::instance` always enables the Vulkan and GL backends. GL means an EGL set-up,
  and `enumerate_adapters` also lists llvmpipe. No environment variable changes this.
- `select_adapter_and_device` creates a full device and configures the surface to test each
  adapter before it picks one. This takes 80 to 120 ms even for the first adapter.

Options for the owner: accept a higher warm gate on this hardware, patch gpui to drop the GL
backend when Vulkan works, or cache the chosen adapter between runs.

## rendering.gpu = "software" (tasklens-cr0.25, 2026-10-07)

With `rendering.gpu = "software"`, gpui now picks lavapipe (llvmpipe on Vulkan). Before this
change it picked no software adapter at all.

### Setup

Release build, this machine (NVIDIA RTX 5070 Ti, KWin Wayland, Mesa 26.2.3,
`vulkan-swrast` 26.2.3). Scratch `XDG_CONFIG_HOME` with a `settings.toml` holding
`[rendering] gpu = "software"`, scratch `XDG_STATE_HOME`. The adapter lines come from gpui's
own log (`gpui_wgpu::wgpu_context`) in the tasklens log file.

### Run 1: cr0.19 variables

`env_for` set `WGPU_ADAPTER_NAME=llvmpipe` and
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`.

| Adapter listed | Backend | Type | Result |
|---|---|---|---|
| NVIDIA GeForce RTX 5070 Ti/PCIe/SSE2 | Gl | Other | failed: no compatible surface formats |

The main window did not open ("No GPU adapter found that can configure the display surface").
Two faults:

- The ICD path is wrong. Current `vulkan-swrast` ships `lvp_icd.json`, not
  `lvp_icd.x86_64.json`, so Vulkan listed no adapter at all.
- The GL backend used the NVIDIA EGL driver. On a machine where that GL adapter works, it
  would have been picked, because gpui ranks type `Other` above type `Cpu`.

gpui does not read `WGPU_ADAPTER_NAME` (cr0.19).

### Experiments with gpu = "auto" and variables set by hand

| Variables | Adapters listed | Picked |
|---|---|---|
| none | NVIDIA Vulkan, NVIDIA Gl, llvmpipe Vulkan | NVIDIA Vulkan |
| `VK_ICD_FILENAMES=lvp_icd.json` | NVIDIA Gl, llvmpipe Vulkan | llvmpipe Vulkan (NVIDIA Gl failed) |
| plus `LIBGL_ALWAYS_SOFTWARE=1` | NVIDIA Gl, llvmpipe Vulkan | same |
| missing ICD, `LIBGL_ALWAYS_SOFTWARE=1` | NVIDIA Gl | none, window did not open |
| missing ICD, `LIBGL_ALWAYS_SOFTWARE=1`, `__EGL_VENDOR_LIBRARY_FILENAMES=50_mesa.json` | llvmpipe Gl | llvmpipe Gl |

`LIBGL_ALWAYS_SOFTWARE=1` alone does not move GL off NVIDIA. glvnd loads the NVIDIA EGL vendor
first, and that driver ignores the Mesa variable. Pointing glvnd at the Mesa vendor file
(`/usr/share/glvnd/egl_vendor.d/50_mesa.json`) does. `MESA_LOADER_DRIVER_OVERRIDE` was not
needed.

### Run 2: the new variables

`env_for("software")` now sets:

- `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json:/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`
  (the loader skips a file that is missing)
- `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`
- `LIBGL_ALWAYS_SOFTWARE=1`
- `WGPU_ADAPTER_NAME=llvmpipe` (kept, has no effect on gpui)

| Adapter listed | Backend | Type | Result |
|---|---|---|---|
| llvmpipe (LLVM 22.1.8, 256 bits) | Vulkan | Cpu | selected |
| llvmpipe (LLVM 22.1.8, 256 bits) | Gl | Cpu | not tried |

Every adapter gpui can list is now a software one, so the real GPU cannot be picked.
`window_open` took 164 ms.

### Open points

- When no adapter works, the main window does not open but the process keeps running with
  no window. Run 1 had to be stopped by hand. Not fixed here.
- The Mesa vendor file path is the Arch one (package `mesa`). A system without that file was
  not tested.

## zed#64499 soak (tasklens-wlf.20, 2026-10-07)

Result: PASS. 0 stalls over 250 ms in 600 s across 3 windows, with no input.

### Setup

- Owner desktop: KDE Plasma Wayland (`wayland-0`), RTX 5070 Ti, Vulkan, session unlocked.
- Release build of `tl-app` with `--features bench`, from the tasklens-wlf.17 commits
  (`536e7d1f`, `66855ba2`, `16e7fff5`).
- Command, started from a terminal with no keyboard or mouse input for the whole run:
  `tasklens --app-id dev.tasklens.benchwlf17 --bench soak --duration 600 --warmup 0 --json
  /var/tmp/claude/tasklens-wlf.17-soak.json`, with scratch `XDG_*` folders under
  `/var/tmp/claude/`. PID 434439, 22:00:30 to 22:10:31 local time, exit 0.
- Other agents were building at the same time. The 1-minute load average was 9.6 at the start
  and 10.8 at the end. A pass under this load is stronger evidence than a pass on an idle
  machine.
- Not checked: whether the windows stayed unobscured. The run was started by an agent, which
  cannot see the screen. No window was minimized by the run.

### Stalls

| Window | Frames | Stalls |
|---|---|---|
| 1, perf-cpu composition | 14364 | 0 |
| 2, Soak Graph 1 (CPU total) | 4605 | 0 |
| 3, Soak Graph 2 (Memory) | 16877 | 0 |
| Total | 35846 | 0 |

The log has no `frame stall` line.

### Report

```json
{
  "scene": "soak",
  "frames": 35846,
  "frame_cpu_ms": { "p50": 0.0, "p99": 0.0, "max": 0.0 },
  "missed_frames": 0,
  "long_missed_frames": 0,
  "process_cpu_pct": 23.656354874028942,
  "probe_cpu_pct": null,
  "rss_mb": 325.308416,
  "pss_mb": 263.972864,
  "stalls": 0,
  "warmup_stalls": 0,
  "bridge_latency_ms": null,
  "gpu": null
}
```

`frame_cpu_ms` is 0 because the soak runs without the frame probe (it would force frames).

## Static scene idle CPU (tasklens-wlf.27, 2026-11-04)

Result: FAIL. The static scene used a median 13.27% process CPU of one core over 30 s
(nested KWin, not the 144 Hz screen). The UI-PRF-053 gate is at most 2%. The fix goes to a
new bead with the profile below (owner decision, wlf.27 comment 1005).

The other two acceptance points pass. The bench requested no frames (a static run that
requests frames fails before it writes a report, `scenes.rs:702`), and the report's `frames` field counts window
paints: 300 in 30 s, so 10 Hz, as UI-PRF-053 expects.

### Setup

- Hidden nested KWin (`kwin_wayland --virtual`, 1920x1200, 60 Hz output) started by
  `/var/tmp/claude/tl-smoke/harness.sh start wlf27`. The app ran on the nested Xwayland
  (X11), with its own `XDG_*` folders. Office PC, RTX 5070 Ti, NVIDIA 615.78.08. Session 2
  (the real desktop) was unlocked for every run.
- Build: `cargo build --locked --profile perf -p tl-app --features bench` at `f62a4d71a`
  (the UI and chart crates last changed in `0eb55edda`). Binary `target/perf/tasklens`.
- Command per run: `tasklens --bench static --duration 30 --warmup 5 --json <file>`, no
  perf attached. Scripts and output are in `/var/tmp/claude/wlf27/`.
- The 60 Hz output does not affect this gate: the static scene draws when data arrives
  (10 Hz), and it starts no frame probe.
- The driver started each run only when the 1-minute load average was below 8.

### Runs

| Run | Start (local) | Load at start (1, 5, 15 min) | Frames | Process CPU (nested KWin, not the 144 Hz screen) | RSS |
|---|---|---|---|---|---|
| 1 | 12:30:01 | 7.40, 21.15, 21.98 | 300 | 14.38% | 300 MB |
| 2 | 12:42:08 | 7.44, 11.46, 16.31 | 300 | 12.49% | 301 MB |
| 3 | 12:42:45 | 6.64, 10.74, 15.86 | 300 | 13.27% | 305 MB |
| Median | | | 300 | 13.27% | |

`window_active` was true and `stalls` was 0 in every run. 13.27% of one core over 30 s is
4.0 s of CPU for 300 paints, so about 13 ms of CPU per paint. To pass at 10 Hz, a paint
must cost at most 2 ms.

Earlier readings of the same scene: 21.9% on the owner screen in wlf.1 (2026-10-07,
release build, before the chart fixes), and 18.0% when wlf.33 closed. The fixes in wlf.33,
wlf.34, wlf.35, wlf.36 and wlf.37 removed lyon tessellation and the repaint of unchanged
cards, but the scene is still about 6.6 times over the gate.

### Profile

One more 30 s run under `perf record -e cpu-clock:u -F 499 --call-graph dwarf,16384`
(12:43:21, load 6.38): 12.66% process CPU, 300 frames, 1950 user-space samples. Perf added
little (12.66% against the 13.27% median), since it sampled user space only. 98.8% of the samples are on the UI main
thread (`tasklens`); the bench publisher thread has 0.8%.

Top 10 self symbols:

| Share | Symbol |
|---|---|
| 9.90% | `gpui_wgpu::wgpu_renderer::WgpuRendererCore::render_frame` |
| 5.59% | `gpui::bounds_tree::BoundsTree<ScaledPixels>::insert` |
| 4.77% | `gpui::scene::Scene::insert_primitive::<Path<ScaledPixels>>` |
| 4.77% | `tl_ui_chart::chart::geometry::StrokeMesh::stroke_line` |
| 4.36% | `tl_ui_chart::chart::geometry::StrokeMesh::tri` |
| 3.69% | `Vec<PathVertex<Pixels>>` from iterator (vertex copy) |
| 3.69% | libc `0x1b1b4a` (memcpy family) |
| 3.18% | `Vec<PathVertex<ScaledPixels>>` from iterator (vertex copy) |
| 2.46% | `tl_ui_chart::chart::geometry::FillMesh::tri` |
| 1.64% | libc `0x1b1b29` (memcpy family) |

By phase (script `phases.py` from wlf.28):

| Phase | Share |
|---|---|
| libc (malloc, free, memcpy) | 21.8% |
| GPU submit (gpui_wgpu, NVIDIA driver) | 16.6% |
| Chart geometry (`tl_ui_chart`) | 15.7% |
| Other gpui (element tree, window, app) | 12.0% |
| Scene build (BoundsTree, scene insert) | 10.9% |
| Chart paint copies (`translated`, `Path::scale`, `paint_path`) | 6.9% |
| Layout (taffy) | 6.4% |
| Text (shaping, glyphs) | 3.0% |
| Other tasklens UI code | 2.5% |
| Lyon tessellation | 0.0% |

Reading: the cost now follows the number of path vertices per paint. Each paint rebuilds
stroke and fill meshes (15.7%, chart geometry), copies the vertex buffers twice on their way
into the scene (`Pixels`, then `ScaledPixels`: 6.9%, and likely much of the 21.8% in libc,
which I did not attribute), inserts each
path into the scene's bounds tree (10.4%), and uploads it all to the GPU (16.6%). The call
stacks did not unwind past one frame (children equal self), so which chart owns which
share is inferred from the code, not measured.

### Redraw counters

The ignored test `bench_scenes::redraw_work_static` (test platform, no GPU) counts work
per draw. Before is wlf.27 comment 187 (2026-10-07), after is this run.

| Per draw | Before | After |
|---|---|---|
| Window paints | 3.3 | 2.2 |
| Sidebar renders | 2.3 | 2.2 |
| 9 card charts: renders, geometry builds, path paints | 20.7, 3.9, 48.3 | 2.5, 2.0, 6.5 |
| Overall chart: renders, builds, paths | 2.3, 2.3, 6.9 | 1.2, 1.2, 3.6 |
| CPU grid: renders, builds, paths, text shapes | 2.3, 2.3, 6.9, 61.5 | 1.2, 1.2, 3.6, 35.2 |

## Unverified items settled in phase 0

- U-9 not needed: the soak passed without the patch.
