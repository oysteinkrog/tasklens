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
