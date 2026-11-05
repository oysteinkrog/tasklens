# Phase-0 spike results

E01 owns this file. Each section records one measurement run.

## Machine and build

Run record: tasklens-wlf.18, 2026-11-04. Numbers in this file from this bead come from a hidden
nested KWin session, never from the real screen (leader decision, 2026-11-04).

- Kernel: 7.3.0-4-cachyos (`uname -r`).
- KWin: 6.8.2 (`kwin_wayland --version`). `plasmashell --version` printed nothing in this lane.
- NVIDIA driver: NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08.
- GPU: NVIDIA GeForce RTX 5070 Ti.
- Monitors on the owner desktop: three DisplayPort outputs, all with a preferred mode of
  1920x1200 (`/sys/class/drm`). This lane cannot read the real session, so the 144 Hz output and
  the 110% scale of the fixture are not measured here. The 144 Hz numbers on the owner's screen
  stay open for tasklens-wlf.2.
- Nested session: `kwin_wayland --virtual --xwayland --no-lockscreen --width 1920 --height 1200
  --scale 1.1`, one 60 Hz virtual output (KWin 6.8.2 has no refresh option). The harness is a
  copy of `/var/tmp/claude/tl-smoke/harness.sh` with `--scale 1.1` added
  (`/var/tmp/claude/wlf18/harness.sh`). Window size, theme and settings are the bench defaults.
- Build: git sha b767ed29845c6cad27a4f52d1079e2b67c58702c (HEAD). The summary file says
  `-dirty` because `.beads/issues.jsonl` and untracked folders were not committed. No Rust file
  was changed in this bead.
- Build commands: `heavy-build cargo build --locked --profile perf -p tl-app --features bench`
  and `heavy-build cargo build --locked -p xtask`.
- Run command (`/var/tmp/claude/wlf18/run.sh`), after the 1-minute load average fell below 8:
  `TL_BUILD_WRAP=1 TL_BENCH_REFRESH_HZ=60 WAYLAND_DISPLAY=<nested socket, absolute path>
  XDG_SESSION_TYPE=wayland DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent-wlf18 XDG_*_HOME=<private>
  cargo xtask bench`, under the bench slot lock. The app ran on the nested Wayland socket, not on
  `wayland-0`. The nonexistent system bus address skips the "session is locked" check, as in
  wlf.28.

Differences from the fixture: 60 Hz output instead of 144 Hz, window and scale as above, a
virtual output with software compositing in KWin, and other jobs on the machine (load below).
Missed frames are judged against this session's own 60 Hz period (limit 25.00 ms, long limit
50.00 ms), as set by tasklens-wlf.38.

### rendering.gpu = "software" (tasklens-cr0.25, 2026-10-07)

With `rendering.gpu = "software"`, gpui now picks lavapipe (llvmpipe on Vulkan). Before this
change it picked no software adapter at all.

#### Setup

Release build, this machine (NVIDIA RTX 5070 Ti, KWin Wayland, Mesa 26.2.3,
`vulkan-swrast` 26.2.3). Scratch `XDG_CONFIG_HOME` with a `settings.toml` holding
`[rendering] gpu = "software"`, scratch `XDG_STATE_HOME`. The adapter lines come from gpui's
own log (`gpui_wgpu::wgpu_context`) in the tasklens log file.

#### Run 1: cr0.19 variables

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

#### Experiments with gpu = "auto" and variables set by hand

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

#### Run 2: the new variables

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

#### Open points

- When no adapter works, the main window does not open but the process keeps running with
  no window. Run 1 had to be stopped by hand. Not fixed here.
- The Mesa vendor file path is the Arch one (package `mesa`). A system without that file was
  not tested.

## Gate runs (cargo xtask bench)

Run record: tasklens-wlf.18, 2026-11-04. Every number here is (hidden nested KWin, 60 Hz), not the 144 Hz screen.
All 12 reports have `refresh_hz` 60, `window_active` true and 0 stalls.

Result: FAIL on 7 of the 8 gates. Only the missed-frame rule for `table-2000` and the bridge
sample arrival passed. Fix beads: tasklens-wlf.39 and tasklens-wlf.40 (static and perf-cpu, from
wlf.27 and wlf.28), tasklens-wlf.41 (table-2000 frame CPU and RSS), tasklens-wlf.42 (bridge
latency).

### Load and reruns

`load.log` has the 1-minute load average every 5 s. Attempt 1 (all four scenes, 3 runs each)
started with `perf-cpu` run 1 at load 10.22, which is above 8, so `perf-cpu` was run again as
attempt 2 (3 runs, started at load 7.29, 5.96 and 4.84). Both attempts are recorded. The gate row
for `perf-cpu` uses attempt 2. All other runs started at a 1-minute load of 7.09 or lower.

| Run (attempt 1) | Start | Load 1 min |
|---|---|---|
| perf-cpu 1, 2, 3 | 20:05:50, 20:06:25, 20:07:01 | 10.22, 7.77, 5.57 |
| table-2000 1, 2, 3 | 20:07:37, 20:08:12, 20:08:48 | 5.66, 5.86, 7.09 |
| bridge 1, 2, 3 | 20:09:23, 20:09:59, 20:10:35 | 6.03, 6.77, 6.94 |
| static 1, 2, 3 | 20:11:11, 20:11:46, 20:12:22 | 6.31, 4.64, 3.64 |
| perf-cpu 1, 2, 3 (attempt 2) | 20:19:54, 20:20:29, 20:21:05 | 7.29, 5.96, 4.84 |

### Stdout table, attempt 1

| Scene | frame CPU p50 ms | p99 ms | missed | process CPU % | RSS MB | stalls | bridge p99 ms |
|---|---|---|---|---|---|---|---|
| bridge | 15.73 | 20.63 | 616.00 | 65.08 | 344.89 | 0.00 | 30.20 |
| perf-cpu | 15.52 | 20.76 | 310.00 | 71.29 | 343.18 | 0.00 | - |
| static | 0.00 | 0.00 | 0.00 | 17.57 | 288.83 | 0.00 | - |
| table-2000 | 4.03 | 6.25 | 0.00 | 25.56 | 238.04 | 0.00 | - |
Summary written to /home/oystein/projects/tasklens/target/bench/2026-11-04-b767ed29845c-dirty.json

The table shows the median run chosen by xtask (by `frame_cpu_ms.p99`, and for `static` by
process CPU), not the middle value of each metric. The gate rows below use the middle of the
three run values.

### Stdout table, attempt 2 (perf-cpu only)

| Scene | frame CPU p50 ms | p99 ms | missed | process CPU % | RSS MB | stalls | bridge p99 ms |
|---|---|---|---|---|---|---|---|
| perf-cpu | 15.75 | 21.94 | 609.00 | 62.91 | 294.52 | 0.00 | - |
Summary written to /home/oystein/projects/tasklens/target/bench/2026-11-04-b767ed29845c-dirty.json

### Summary JSON

Attempt 1, `target/bench/2026-11-04-b767ed29845c-dirty.json` (the file in `target/` was later
overwritten by attempt 2, so the copy is kept in `/var/tmp/claude/wlf18/attempt1/`):

```json
{
  "date": "2026-11-04",
  "git_sha": "b767ed29845c-dirty",
  "scenes": {
    "bridge": {
      "median": {
        "bridge_latency_ms": {
          "p50": 13.714123,
          "p99": 30.20326
        },
        "frame_cpu_ms": {
          "max": 25.289235,
          "p50": 15.728709,
          "p99": 20.630314
        },
        "frames": 1180,
        "gpu": null,
        "long_missed_frames": 2,
        "missed_frames": 616,
        "probe_cpu_pct": null,
        "process_cpu_pct": 65.08488966252996,
        "pss_mb": 271.73376,
        "refresh_hz": 60,
        "rss_mb": 344.891392,
        "scene": "bridge",
        "stalls": 0,
        "warmup_stalls": 0,
        "window_active": true
      },
      "runs": [
        {
          "bridge_latency_ms": {
            "p50": 13.456159,
            "p99": 30.929374
          },
          "frame_cpu_ms": {
            "max": 38.094313,
            "p50": 16.71029,
            "p99": 22.540183
          },
          "frames": 1113,
          "gpu": null,
          "long_missed_frames": 3,
          "missed_frames": 675,
          "probe_cpu_pct": null,
          "process_cpu_pct": 65.61051402589273,
          "pss_mb": 264.293376,
          "refresh_hz": 60,
          "rss_mb": 337.321984,
          "scene": "bridge",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": {
            "p50": 5.944324,
            "p99": 23.590013
          },
          "frame_cpu_ms": {
            "max": 22.289953,
            "p50": 15.002069,
            "p99": 19.782161
          },
          "frames": 1284,
          "gpu": null,
          "long_missed_frames": 2,
          "missed_frames": 509,
          "probe_cpu_pct": null,
          "process_cpu_pct": 65.70519309694848,
          "pss_mb": 270.83776,
          "refresh_hz": 60,
          "rss_mb": 343.851008,
          "scene": "bridge",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": {
            "p50": 13.714123,
            "p99": 30.20326
          },
          "frame_cpu_ms": {
            "max": 25.289235,
            "p50": 15.728709,
            "p99": 20.630314
          },
          "frames": 1180,
          "gpu": null,
          "long_missed_frames": 2,
          "missed_frames": 616,
          "probe_cpu_pct": null,
          "process_cpu_pct": 65.08488966252996,
          "pss_mb": 271.73376,
          "refresh_hz": 60,
          "rss_mb": 344.891392,
          "scene": "bridge",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        }
      ]
    },
    "perf-cpu": {
      "median": {
        "bridge_latency_ms": null,
        "frame_cpu_ms": {
          "max": 27.211316,
          "p50": 15.52385,
          "p99": 20.760073
        },
        "frames": 1350,
        "gpu": null,
        "long_missed_frames": 0,
        "missed_frames": 310,
        "probe_cpu_pct": null,
        "process_cpu_pct": 71.28550231270113,
        "pss_mb": 265.136128,
        "refresh_hz": 60,
        "rss_mb": 343.175168,
        "scene": "perf-cpu",
        "stalls": 0,
        "warmup_stalls": 0,
        "window_active": true
      },
      "runs": [
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 27.211316,
            "p50": 15.52385,
            "p99": 20.760073
          },
          "frames": 1350,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 310,
          "probe_cpu_pct": null,
          "process_cpu_pct": 71.28550231270113,
          "pss_mb": 265.136128,
          "refresh_hz": 60,
          "rss_mb": 343.175168,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 27.599216,
            "p50": 15.150699,
            "p99": 20.448022
          },
          "frames": 1253,
          "gpu": null,
          "long_missed_frames": 2,
          "missed_frames": 542,
          "probe_cpu_pct": null,
          "process_cpu_pct": 64.20605882639266,
          "pss_mb": 221.935616,
          "refresh_hz": 60,
          "rss_mb": 294.699008,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 23.790795,
            "p50": 15.569519,
            "p99": 21.138663
          },
          "frames": 1205,
          "gpu": null,
          "long_missed_frames": 2,
          "missed_frames": 586,
          "probe_cpu_pct": null,
          "process_cpu_pct": 62.839896731254335,
          "pss_mb": 215.593984,
          "refresh_hz": 60,
          "rss_mb": 288.37888,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        }
      ]
    },
    "static": {
      "median": {
        "bridge_latency_ms": null,
        "frame_cpu_ms": {
          "max": 0.0,
          "p50": 0.0,
          "p99": 0.0
        },
        "frames": 300,
        "gpu": null,
        "long_missed_frames": 0,
        "missed_frames": 0,
        "probe_cpu_pct": null,
        "process_cpu_pct": 17.568869079602752,
        "pss_mb": 215.81824,
        "refresh_hz": 60,
        "rss_mb": 288.833536,
        "scene": "static",
        "stalls": 0,
        "warmup_stalls": 0,
        "window_active": true
      },
      "runs": [
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 0.0,
            "p50": 0.0,
            "p99": 0.0
          },
          "frames": 300,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 17.186752324187086,
          "pss_mb": 207.024128,
          "refresh_hz": 60,
          "rss_mb": 280.02304,
          "scene": "static",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 0.0,
            "p50": 0.0,
            "p99": 0.0
          },
          "frames": 300,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 17.568869079602752,
          "pss_mb": 215.81824,
          "refresh_hz": 60,
          "rss_mb": 288.833536,
          "scene": "static",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 0.0,
            "p50": 0.0,
            "p99": 0.0
          },
          "frames": 300,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 20.22836364007175,
          "pss_mb": 218.324992,
          "refresh_hz": 60,
          "rss_mb": 291.328,
          "scene": "static",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        }
      ]
    },
    "table-2000": {
      "median": {
        "bridge_latency_ms": null,
        "frame_cpu_ms": {
          "max": 8.453475,
          "p50": 4.028423,
          "p99": 6.250323
        },
        "frames": 1800,
        "gpu": null,
        "long_missed_frames": 0,
        "missed_frames": 0,
        "probe_cpu_pct": null,
        "process_cpu_pct": 25.560297875915538,
        "pss_mb": 165.528576,
        "refresh_hz": 60,
        "rss_mb": 238.043136,
        "scene": "table-2000",
        "stalls": 0,
        "warmup_stalls": 0,
        "window_active": true
      },
      "runs": [
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 8.244924,
            "p50": 4.301693,
            "p99": 6.047573
          },
          "frames": 1800,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 26.383243130071502,
          "pss_mb": 166.0416,
          "refresh_hz": 60,
          "rss_mb": 238.333952,
          "scene": "table-2000",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 8.246865,
            "p50": 4.330272,
            "p99": 6.861234
          },
          "frames": 1799,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 27.122352228794288,
          "pss_mb": 166.709248,
          "refresh_hz": 60,
          "rss_mb": 239.009792,
          "scene": "table-2000",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 8.453475,
            "p50": 4.028423,
            "p99": 6.250323
          },
          "frames": 1800,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 0,
          "probe_cpu_pct": null,
          "process_cpu_pct": 25.560297875915538,
          "pss_mb": 165.528576,
          "refresh_hz": 60,
          "rss_mb": 238.043136,
          "scene": "table-2000",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        }
      ]
    }
  }
}
```

Attempt 2 (perf-cpu only), same path, copy in `/var/tmp/claude/wlf18/summary-attempt2.json`:

```json
{
  "date": "2026-11-04",
  "git_sha": "b767ed29845c-dirty",
  "scenes": {
    "perf-cpu": {
      "median": {
        "bridge_latency_ms": null,
        "frame_cpu_ms": {
          "max": 26.003386,
          "p50": 15.752059,
          "p99": 21.939523
        },
        "frames": 1184,
        "gpu": null,
        "long_missed_frames": 4,
        "missed_frames": 609,
        "probe_cpu_pct": null,
        "process_cpu_pct": 62.91111015789047,
        "pss_mb": 221.5424,
        "refresh_hz": 60,
        "rss_mb": 294.52288,
        "scene": "perf-cpu",
        "stalls": 0,
        "warmup_stalls": 0,
        "window_active": true
      },
      "runs": [
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 26.003386,
            "p50": 15.752059,
            "p99": 21.939523
          },
          "frames": 1184,
          "gpu": null,
          "long_missed_frames": 4,
          "missed_frames": 609,
          "probe_cpu_pct": null,
          "process_cpu_pct": 62.91111015789047,
          "pss_mb": 221.5424,
          "refresh_hz": 60,
          "rss_mb": 294.52288,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 23.985354,
            "p50": 15.266069,
            "p99": 20.079542
          },
          "frames": 1274,
          "gpu": null,
          "long_missed_frames": 0,
          "missed_frames": 513,
          "probe_cpu_pct": null,
          "process_cpu_pct": 65.064164600606,
          "pss_mb": 216.446976,
          "refresh_hz": 60,
          "rss_mb": 289.374208,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        },
        {
          "bridge_latency_ms": null,
          "frame_cpu_ms": {
            "max": 57.285863,
            "p50": 17.16062,
            "p99": 43.674346
          },
          "frames": 928,
          "gpu": null,
          "long_missed_frames": 110,
          "missed_frames": 586,
          "probe_cpu_pct": null,
          "process_cpu_pct": 63.851650385736455,
          "pss_mb": 207.6416,
          "refresh_hz": 60,
          "rss_mb": 290.28352,
          "scene": "perf-cpu",
          "stalls": 0,
          "warmup_stalls": 0,
          "window_active": true
        }
      ]
    }
  }
}
```

### Gates (hidden nested KWin, 60 Hz)

Median is the middle of the three run values. Two decimals.

| Gate | Pass rule | Median value | Every run | Result |
|---|---|---|---|---|
| UI thread CPU per frame, `perf-cpu`, p50 | <= 1.0 ms | 15.75 ms (hidden nested KWin, 60 Hz) | 15.75, 15.27, 17.16 | FAIL |
| UI thread CPU per frame, `perf-cpu`, p99 | <= 2.0 ms | 21.94 ms (hidden nested KWin, 60 Hz) | 21.94, 20.08, 43.67 | FAIL |
| UI thread CPU per frame, `table-2000`, p99 | <= 2.0 ms | 6.25 ms (hidden nested KWin, 60 Hz) | 6.05, 6.86, 6.25 | FAIL |
| Missed frames, `perf-cpu` | median run 0, every run <= 2, long misses 0 | 586 missed, 4 long (hidden nested KWin, 60 Hz) | missed 609, 513, 586; long 4, 0, 110 | FAIL |
| Missed frames, `table-2000` | same | 0 missed, 0 long (hidden nested KWin, 60 Hz) | missed 0, 0, 0; long 0, 0, 0 | PASS |
| Process CPU, `perf-cpu` | <= 30% | 63.85% (hidden nested KWin, 60 Hz) | 62.91, 65.06, 63.85 | FAIL |
| Process CPU, `static` | <= 2% | 17.57% (hidden nested KWin, 60 Hz) | 17.19, 17.57, 20.23 | FAIL |
| RSS after `table-2000` | <= 150 MB | 238.33 MB (hidden nested KWin, 60 Hz) | 238.33, 239.01, 238.04 | FAIL |
| Bridge samples arrive | `bridge_latency_ms` set in every run | set in 3 of 3 (hidden nested KWin, 60 Hz) | 3 of 3 | PASS |
| Bridge wake-to-render p99 | <= 20 ms | 30.20 ms (hidden nested KWin, 60 Hz) | 30.93, 23.59, 30.20 | FAIL |

Attempt 1 `perf-cpu` for comparison (run 1 started above load 8): p50 15.52, 15.15, 15.57 ms;
p99 20.76, 20.45, 21.14 ms; missed 310, 542, 586; long missed 0, 2, 2; process CPU 71.29, 64.21,
62.84%. The same gates fail in both attempts. Attempt 2 run 3 shows a 43.67 ms p99 and 110 long
misses; its load at start was 4.84, so the cause is not known.

The missed-frame row for `perf-cpu` also fails for its "at most 2 per run" part (513 to 609).
`bridge` runs at about the same frame cost and missed 509 to 675 frames; the gate list has no
missed-frame rule for `bridge`.

### Profile of table-2000

One perf run (`perf record -e cpu-clock:u -F 499 --call-graph dwarf`, nested session, 60 Hz,
Wayland) gave frame CPU p50 4.02 ms, p99 5.43 ms and process CPU 25.18%. Self time:
`BoundsTree::insert` 9.21%, taffy block and flexbox layout 5.8%, unsymbolized libc (copies)
about 26% of samples, `WindowTextSystem::shape_text` closure 1.51%, the `tasklens` binary 68.86%
and libc 26.08% of all samples. Data: `/var/tmp/claude/wlf18/prof.data` and `table-2000-prof.json`.
The `perf-cpu` and `static` profiles are in the two sections below.

### table-2000 cost and memory (tasklens-wlf.41, 2026-11-04)

Result: the gate still fails, and this section only measures. Every number is from a hidden
nested KWin session at 60 Hz, not the 144 Hz screen.

- CPU per frame: no one bucket dominates. Row layout is the largest: 1.27 ms of the 4.30 ms
  p50 (29.5%), most of it taffy, and it includes text measuring. Then GPUI element work, GPU
  submit (0.66 ms), row paint (0.63 ms) and the row build callback (0.36 ms). Work outside the
  table takes 1.74 ms per frame on its own (measured share, hidden nested KWin, 60 Hz).
- RSS: the 2,000-row table adds about 0.7 MB. `table-2000` and `history-scrub` both hold
  237 to 238 MB (hidden nested KWin, 60 Hz). Most of it is library code pages: the NVIDIA
  driver 71.7 MB, the tasklens binary 49.7 MB, and 34.9 MB of Mesa that GPUI's GL backend
  loads. Fonts are 6.8 MB. UI-PRC-023 rule 8 does not hold on this machine.
- GPUI: GPU submit, scene build, element bookkeeping and the GL backend are GPUI code, which
  ADR-014 rule 3 forbids patching.

#### Setup

- Session: a copy of the wlf.18 harness (`kwin_wayland --virtual`, 1920 x 1200, `--scale 1.1`,
  60 Hz), app on the nested Wayland socket with `TL_BENCH_REFRESH_HZ=60` and private `XDG_*`
  folders, as in the wlf.18 gate runs. Office PC, Ryzen 9 5950X, RTX 5070 Ti, NVIDIA
  615.78.08. Each run started only when the 1-minute load was below 8, under the bench slot
  lock. HEAD `c827d807f`; no crate has changed since `e57374626`.
- Profile build: `cargo build --locked --profile profiling -p tl-app --features bench`, then
  only the binary relinked with
  `cargo rustc --locked --profile profiling -p tl-app --bin tasklens --features bench -- -C link-arg=-Wl,-z,separate-loadable-segments`.
- Why the relink: with the normal link, perf's libdw unwinder failed on 2,934 of 4,412
  samples (67%) with "address range overlaps an existing module". The mold-linked binary's
  code segment sits 0x1000 above its file offset (offset 0x175a578, address 0x175b578). perf
  computes a module base from each mapping as start minus file offset, so the code mapping
  gives a base 0x1000 off from the first mapping, and libdw refuses the second module. With
  page-aligned segments, offset equals address and no sample failed (measured). The same
  linker and perf most likely explain why the wlf.27 and wlf.28 stacks did not unwind
  (inferred, not re-tested).
- Profile: `perf record -e cpu-clock:u -F 499 --call-graph dwarf,65528`, the largest user
  stack copy perf allows. 46% of stacks reach `main`. A first profile with a 16 KB copy
  (22% reach `main`) gave the same shares by kind within 1.3 points.
- RSS runs: `target/perf/tasklens` (built 20:05 from the same code), no perf. The script
  copied `/proc/<pid>/smaps` and `/proc/<pid>/status` 10 s and 30 s after launch; the two
  copies differ by less than 0.1 MB in every class.
- Scripts and data: `/var/tmp/claude/wlf41/` (`prof.data`, `script.txt`, `split2.py`,
  `smaps.py`, the `*.smaps` and `*.json` files).

#### Runs

All values are hidden nested KWin, 60 Hz.

| Run | Start | Load at start (1, 5, 15 min) | Frame CPU p50 | p99 | Process CPU | RSS |
|---|---|---|---|---|---|---|
| Profile, normal link (stacks failed) | 21:15:14 | 7.52, 14.39, 20.67 | 3.97 ms | 6.62 ms | 25.22% | 301.54 MB |
| Profile, 16 KB stacks | 21:36:51 | 6.48, 15.24, 19.44 | 4.10 ms | 5.95 ms | 25.20% | 236.84 MB |
| Profile, 64 KB stacks (used below) | 21:38:37 | 4.28, 11.77, 17.73 | 4.74 ms | 9.10 ms | 31.16% | 237.96 MB |
| `table-2000`, first launch of this binary | 21:15:54 | 7.34, 13.45, 20.09 | 5.00 ms | 9.13 ms | 32.05% | 302.48 MB |
| `static` | 21:36:15 | 7.02, 16.39, 19.95 | | | 15.80% | 279.14 MB |
| `history-scrub` | 21:37:37 | 4.91, 13.56, 18.67 | 4.78 ms | 6.63 ms | 29.69% | 237.35 MB |
| `table-2000`, warm shader cache | 21:39:15 | 6.37, 11.50, 17.42 | 4.27 ms | 5.87 ms | 26.23% | 238.00 MB |
| `table-2000`, NVIDIA Vulkan driver only | 21:40:33 | 5.18, 9.98, 16.40 | 3.70 ms | 5.59 ms | 23.36% | 233.88 MB |
| `table-2000`, NVIDIA Vulkan and EGL only | 21:41:44 | 5.68, 9.07, 15.61 | 4.10 ms | 7.18 ms | 25.99% | 199.19 MB |

Missed frames were 0 in every `table-2000` run. The 64 KB profile costs CPU itself (p50
4.74 ms against the 4.30 ms gate median), so the ms columns below apply its sample shares
to the gate median p50 of 4.30 ms, not to its own p50.

#### Frame time by region

Region is where on the stack the sample sits. Shares are of 5,302 main-thread samples in
the 64 KB profile; ms is the share times 4.30 ms (hidden nested KWin, 60 Hz). The budget
column is UI-PRC-023 (`docs/ui/processes.md`, 144 Hz, 2.0 ms target).

| Region | Share | ms per frame | UI-PRC-023 budget |
|---|---|---|---|
| Row build: the range callback (`build_rows`, `build_row`, cell builders) | 8.4% | 0.36 | 0.6 ms |
| Row layout: `request_layout` and taffy for each row, text measuring included | 29.5% | 1.27 | 0.5 ms |
| Row prepaint (hitboxes) and dropping last frame's elements | 6.2% | 0.27 | none |
| Row paint | 14.7% | 0.63 | 0.4 ms |
| Header | 0.7% | 0.03 | 0.1 ms |
| Selection, hover, demand hashing | not separated | | 0.1 ms |
| GPU submit (`render_frame`, wgpu, NVIDIA driver) | 15.3% | 0.66 | none |
| Minigraph panel (cached cards replayed) | 6.1% | 0.26 | none |
| Scene finish (primitive sort) | 2.2% | 0.09 | none |
| Rest of the window, event loop and bench driver | 17.0% | 0.73 | none |

Inside row layout: taffy `compute_layout` with its measure callbacks is 24.8% (1.07 ms) and
element `request_layout` is 4.6% (0.20 ms). Inside row paint: glyph paint (raster bounds,
atlas lookup, sprites) 5.7% (0.25 ms), `BoundsTree::insert` 4.5% (0.19 ms), div background
quads 3.5% (0.15 ms), `Scene::insert_primitive` 0.8%, our `CellText::paint` 0.2%.

The 16 KB profile put 6.6 points more in "rest of the window" and fewer in the row regions,
because its stacks stopped before the `UniformList` frame more often. Some row work may
still sit in the 17.0% (inferred), so 0.73 ms is an upper bound for that region.

#### Frame time by kind

Kind is what the innermost known frame does; a libc leaf takes its caller's kind.

| Kind | Share, 64 KB | Share, 16 KB | ms per frame (64 KB share) |
|---|---|---|---|
| GPUI elements (div, style, hitbox, arena, element state) | 25.9% | 24.6% | 1.11 |
| taffy layout | 23.1% | 23.2% | 0.99 |
| Text (shaping, line layout, glyph paint, atlas) | 15.9% | 15.2% | 0.68 |
| GPU submit | 15.2% | 16.4% | 0.65 |
| Scene build (`BoundsTree`, `insert_primitive`, `replay`, finish) | 12.2% | 13.0% | 0.52 |
| Our code (`tl_ui*`, `CellText`) | 7.8% | 7.4% | 0.34 |

25% of samples end in libc. Their largest callers are `wgpu_core` `Queue::write_buffer`
(6.2% of all samples, the GPU upload) and taffy tree `clear` and `new_with_children`
(2.6%). The single large memcpy from the `perf-cpu` profile does not appear here.

#### Reading

- Text is measured in layout every frame. `CellText` (`crates/tl-ui-table/src/table/cells.rs:591`)
  wraps GPUI's `StyledText`, whose `request_layout` adds a measured taffy leaf, so taffy calls
  back into text layout for each visible cell in each frame (8.8% of samples, 0.38 ms).
  UI-PRC-023 assumes "cells use fixed w and h so taffy does no measuring"; that does not hold
  for the current cell element (measured from the stacks, cause read from the code).
- The minigraph panel's cards are cached views, but each frame replays their primitives into
  the new scene (`Scene::replay`, then `BoundsTree::insert`): 4.9% of samples, 0.21 ms.
- The shares are averages over all frames. Which work makes the slowest 1% of frames (p99
  6.25 ms in the gate) is not separated here; a 1 Hz re-sort frame is a likely candidate
  (not measured).

#### Levers for the CPU gate

- The largest bucket is row layout (1.27 ms). UI-PRC-023's third fallback step, a `canvas`
  row painter with `ShapedLine::paint` per cell and no per-cell `div`, removes taffy and the
  text measure from the row path. It would also cut the per-cell element build and many
  `BoundsTree` inserts (inferred). It uses GPUI's public `canvas`, so it needs no GPUI change.
  ADR-014 rule 2 also allows "text cache fixes", which aim at the 0.68 ms of text.
- Zebra: the zebra is already the row fill, one quad per row (`row_fill`,
  `crates/tl-ui-table/src/table/tree_core.rs:508`). All div background quads in the rows
  cost 0.15 ms, so dropping zebra quads or merging heat quads (one bg per cell,
  `tree_core.rs:759-762`) can save at most part of 0.15 ms.
- Column culling (UI-PRC-023 rule 7) is not built: `build_row` loops over every visible column
  (`tree_core.rs:692`). With 10 columns in a 1600 px window all of them are on screen, so it
  saves nothing in this scene (inferred).
- The fallback order conflicts. tasklens-pdu.31 says: column culling, heat merge, zebra.
  UI-PRC-023 (`docs/ui/processes.md:1336-1339`) says: zebra, heat merge, canvas row painter.
  This section does not pick one. By measured size, only the canvas row painter aims at more
  than 0.15 ms.
- GPUI code holds GPU submit (0.66 ms), scene build (0.52 ms), the element bookkeeping
  (1.11 ms) and taffy itself. ADR-014 rule 3 forbids patching them; they shrink only when the
  table hands GPUI fewer elements and primitives.
- The work outside the table (GPU submit 0.66, minigraph panel 0.26, scene finish 0.09, rest of
  the window 0.73) adds up to 1.74 ms at p50. If a canvas painter removed all of row layout and
  row build (1.63 ms), p50 would be about 2.7 ms, against a p99 gate of 2.0 ms (inferred,
  simple subtraction). So the in-GPUI fallback alone most likely does not pass this gate, and
  the ADR-014 rule 3 decision comes up for it.

#### RSS by mapping

MB as the bench counts them (kB x 1024 / 10^6), from the 30 s copy of `smaps` and `status`
(hidden nested KWin, 60 Hz).

| Class | `table-2000` warm | `history-scrub` | `static` | `table-2000` first launch | `table-2000`, NVIDIA Vulkan and EGL only |
|---|---|---|---|---|---|
| NVIDIA shader compiler (`libnvidia-gpucomp`, `-rtcore`, `-glvkspirv`) | 32.1 | 32.1 | 32.1 | 82.8 | 32.1 |
| Other NVIDIA libraries (`glcore`, `eglcore`, GLX, EGL) | 26.2 | 26.2 | 26.2 | 25.9 | 26.2 |
| `/dev/nvidia*` mappings | 13.4 | 13.4 | 13.9 | 13.4 | 13.4 |
| The tasklens binary | 49.7 | 50.0 | 51.8 | 48.9 | 49.1 |
| Mesa and what it loads (`libLLVM` 27.1, `libgallium` 3.1, `libvulkan_lvp` 2.8, others) | 34.9 | 34.9 | 34.8 | 34.9 | 0.2 |
| Other shared libraries | 10.0 | 10.0 | 10.0 | 9.9 | 7.5 |
| `[heap]` | 34.6 | 34.8 | 81.3 | 36.4 | 34.5 |
| Other anonymous memory | 16.2 | 14.7 | 15.4 | 29.2 | 15.4 |
| Font files | 6.8 | 7.0 | 6.5 | 7.0 | 6.8 |
| Stacks and the rest | 0.4 | 0.4 | 0.5 | 0.4 | 0.4 |
| Shared memory in VmRSS but in no `smaps` line | 13.7 | 13.7 | 14.2 | 13.7 | 13.7 |
| VmRSS at 30 s | 238.00 | 237.28 | 286.67 | 302.52 | 199.19 |
| Bench `rss_mb` at the end | 238.00 | 237.35 | 279.14 | 302.48 | 199.19 |

- The table: `table-2000` and `history-scrub` differ by 0.72 MB at 30 s. The heap is the same
  (34.6 against 34.8 MB), so 2,000 row models and the text cache are small, as rule 8 expects.
  The scenes differ in more than the table, so 0.7 MB is an estimate (inferred).
- Rule 8 says the RSS gate "is dominated by GPU buffers and fonts". Measured: fonts are
  6.8 MB. GPU buffers live in video memory on this card; RSS sees only the driver's mappings
  (13.4 MB, plus probably the 13.7 MB of shared memory, inferred). 158.6 MB of the 238 MB is
  file-backed library and binary code (`RssFile`).
- Mesa: GPUI creates its wgpu instance with `Backends::VULKAN | Backends::GL`
  (`gpui-pre-wgpu-0.3.7/src/wgpu_context.rs:445`). The GL backend starts EGL, and glvnd loads
  every EGL vendor, Mesa's too, which loads `libgallium` and `libLLVM`. With only the NVIDIA
  Vulkan driver (`VK_LOADER_DRIVERS_SELECT=*nvidia*`), RSS was 233.88 MB and `libLLVM` was still
  mapped. With the NVIDIA EGL vendor as well
  (`__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json`), RSS was
  199.19 MB, 38.8 MB less (measured). Turning the GL backend off is a GPUI change. Setting the
  two variables in the launcher is a packaging change. Both are owner decisions, and 199 MB
  still misses the 150 MB gate.
- First launch: the first run of a binary with an empty NVIDIA shader cache read 302.5 MB, 64.5
  MB more than the warm run. The driver compiled the shaders and paged in its compiler
  (`libnvidia-gpucomp` 64 MB instead of 29 MB). The cache lives in
  `$XDG_CACHE_HOME/nvidia/GLCache` with one entry per executable: two entries, written at
  21:15:15 and 21:15:55, the first launch of each binary. This explains the 301.54 MB of the
  first profiled run (inferred). A gate run right after a new build in a fresh profile can read
  about 300 MB for the same reason.
- `static` holds 46.7 MB more heap than `table-2000` (81.3 against 34.6 MB). That scene is not
  gated on RSS, and this bead did not look into it.
- Earlier readings: 203.45 MB on 2026-10-07 (release build, owner screen). The wlf.18
  `history-scrub` runs read 173.93, 113.50 and 97.28 MB; that spread is not explained here.
- None of the ADR-014 rule 2 levers shrinks library code pages, so they do not reach the RSS
  gate. What remains is an owner decision: a GPUI change, a launcher variable, or a different
  gate measure. PSS was 156.08 MB in the warm run, against 238.00 MB RSS.

### table-2000 gate after the row canvas (tasklens-pdu.69, 2026-11-05)

Result: FAIL. UI thread p99 is 3.84 ms against the ADR-014 limit of 2.0 ms (hidden nested KWin, 60 Hz). Missed frames are 0 in the median run (pass). RSS is 233.34 MB against 150 MB (fail). The numbers are close to, not exactly, the 144 Hz screen.

Setup: hidden nested KWin session `pdu69`, 60 Hz (`TL_BENCH_REFRESH_HZ=60`), scale 1.1, 1920 x 1200, HEAD e8415070e (`-dirty`: other lanes' uncommitted files, no code of this bead), `perf` profile built first with `cargo build --locked --profile perf -p tl-app --features bench`. Command: `cargo xtask bench --scene table-2000 --runs 3` under the bench slot lock, with `TL_BUILD_WRAP=1`. Each run is 30 s plus 5 s warmup. Raw files are in `/var/tmp/claude/pdu69/` (`a1/`, `a2/`).

Attempt 1 did not count. xtask rebuilt inside the timed window (2 min 41 s), so run 1 started at load 11.41 and the machine was above load 8 until about 03:27:17. Runs 2 and 3 of attempt 1 met the load rule, but the plan reruns the whole script when any run fails. Attempt 2 had no rebuild. Every run stays in the table.

Runs, frame CPU ms (hidden nested KWin, 60 Hz), local time 2026-11-05. Loads are 1 and 5 minute, read from `load.log` (run start is the report time minus 35 s):

| Run | Start | Load at start | Load at end | Counts | p50 | p99 | max | Missed | Long missed | RSS MB | PSS MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| a1 run 1 | 03:26:42 | 11.41, 7.68 | 6.99 | no (1-min 11.41 at start) | 2.68 | 3.89 | 5.60 | 0 | 0 | 283.9 | 204.2 |
| a1 run 2 | 03:27:17 | 6.99, 6.99 | 5.45 | yes, but attempt 1 is void | 2.68 | 3.91 | 5.33 | 0 | 0 | 228.6 | 155.5 |
| a1 run 3 | 03:27:52 | 5.17, 6.55 | 3.41 | yes, but attempt 1 is void | 2.68 | 3.85 | 4.53 | 0 | 0 | 233.2 | 160.0 |
| a2 run 1 | 03:28:48 | 3.08, 5.72 | 2.20 | yes | 2.69 | 3.80 | 5.89 | 0 | 0 | 232.5 | 159.3 |
| a2 run 2 | 03:29:24 | 2.20, 5.16 | 1.91 | yes | 2.59 | 3.90 | 5.06 | 0 | 0 | 232.7 | 159.5 |
| a2 run 3 | 03:29:59 | 1.91, 4.76 | 1.55 | yes | 2.67 | 3.84 | 4.98 | 0 | 0 | 233.3 | 160.1 |

Load values at a start or end are the nearest 5 s sample. Values come from the run JSON files, not the xtask table.

### Gates (hidden nested KWin, 60 Hz)

Median is the middle of the three counted runs (attempt 2).

| Gate | Pass rule | Median value | Every run | Result |
|---|---|---|---|---|
| UI thread CPU per frame, `table-2000`, p99 | <= 2.0 ms | 3.84 ms (hidden nested KWin, 60 Hz) | 3.80, 3.90, 3.84 | FAIL |
| Missed frames, `table-2000` | median run 0, every run <= 2, long misses 0 | 0 missed, 0 long (hidden nested KWin, 60 Hz) | missed 0, 0, 0; long 0, 0, 0 | PASS |
| RSS after `table-2000` | <= 150 MB | 233.34 MB (hidden nested KWin, 60 Hz) | 232.54, 232.74, 233.34 | FAIL |

Comparison (hidden nested KWin, 60 Hz): wlf.18 had p99 6.25 ms and RSS 238.33 MB. The rsf.32 working runs (counted medians, profiling binaries outside xtask) had p99 4.09 ms and RSS 234.2 MB. This gate run has p99 3.84 ms and RSS 233.34 MB, so p99 is 39% lower than wlf.18 and still 1.84 ms over the limit.

Profile: not retaken. The rsf.32 profile is the same row-canvas code (e8415070e changed only a test). One run, `perf record -F 499`, main thread only, after the row canvas: p50 2.75 ms, p99 3.88 ms, 2,930 main-thread samples. Data: `/var/tmp/claude/rsf32/after-prof.data` and `after-top.txt`. Top self time:

 - 9.93% `BoundsTree::insert`
 - 10.09% four libc addresses 0x1b1b24..0x1b1b33 (unresolved)
 - 2.07% `Scene::finish` SubpixelSprite sort
 - 1.93% taffy block `compute_inner`
 - 1.83% atlas HashMap `rustc_entry`
 - 1.66% taffy `compute_flexbox_layout`
 - 1.39% `Window::paint_glyph`
 - 1.36% `WindowTextSystem::shape_text` closure
 - 1.32% `Scene::insert_primitive` SubpixelSprite

Self-time samples by symbol family (after): taffy 322, element and style bookkeeping 300, text shaping and glyphs 263, scene build 509, GPU submit 208, table code 107, malloc and free 60.

The rest of the cost is in GPUI, taffy and libc. Under leader rule 1 (leader-notes.md, 2026-11-05) the gate is recorded FAIL and goes to the owner in tasklens-wlf.2. No new fix bead.

RSS: the RSS gate is not fixed here. It goes to the owner in tasklens-wlf.2 (see the wlf.41 section above).

### Static scene idle CPU (tasklens-wlf.27, 2026-11-04)

Result: FAIL. The rerun on 2026-11-05 at `e44eb654a`, after the wlf.39 fix, used a median
16.76% process CPU of one core over 30 s (nested KWin, not the 144 Hz screen). The first
run on 2026-11-04 at `f62a4d71a` used a median 13.27% (nested KWin, not the 144 Hz
screen). The UI-PRF-053 gate is at most 2%. The 2026-11-05 numbers are higher because of
the machine state, not the code (see "Rerun after wlf.39" below). The first failure went to
the fix bead wlf.39 (owner decision, wlf.27 comment 1005).

The other two acceptance points pass. The bench requested no frames (a static run that
requests frames fails before it writes a report, `scenes.rs:702`), and the report's `frames` field counts window
paints: 300 in 30 s, so 10 Hz, as UI-PRF-053 expects.

#### Setup

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

#### Runs

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

#### Profile

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

#### Redraw counters

The ignored test `bench_scenes::redraw_work_static` (test platform, no GPU) counts work
per draw. Before is wlf.27 comment 187 (2026-10-07), after is this run.

| Per draw | Before | After |
|---|---|---|
| Window paints | 3.3 | 2.2 |
| Sidebar renders | 2.3 | 2.2 |
| 9 card charts: renders, geometry builds, path paints | 20.7, 3.9, 48.3 | 2.5, 2.0, 6.5 |
| Overall chart: renders, builds, paths | 2.3, 2.3, 6.9 | 1.2, 1.2, 3.6 |
| CPU grid: renders, builds, paths, text shapes | 2.3, 2.3, 6.9, 61.5 | 1.2, 1.2, 3.6, 35.2 |

#### Rerun after wlf.39 (2026-11-05)

Result: FAIL, median 16.76% of one core (nested KWin, not the 144 Hz screen) against the 2%
gate. That is about 16.8 ms of CPU per paint at 10 Hz. The bench requested no frames, and
`frames` counted 300 paints in 30 s in every run.

Setup, where it differs from the first run:

- Build: `cargo build --locked --profile perf -p tl-app --features bench` at `e44eb654a`.
  This has the wlf.39 grid mesh splice (`4367efe09`, `87fd35661`) and the wlf.43 lock check
  skip. The binary was copied to `/var/tmp/claude/wlf27/g3/tasklens-perf`.
- Same nested KWin (`harness.sh start wlf27`, 1920x1200, 60 Hz output, app on the nested
  Xwayland). `harness.sh app` starts the app with `env -i`, so a wrapper
  (`g3/nested.sh`) sets `TL_BENCH_NESTED=1`.
- The real desktop (session 2) was locked for every run. With `TL_BENCH_NESTED=1` the bench
  skips the lock check (wlf.43).
- Quiet-machine rule: a run counts when the 1-minute and 5-minute load are both at most 8
  at the start, and the 1-minute load is at most 8 at the end. All 3 runs counted.
- Scripts, reports and the profile: `/var/tmp/claude/wlf27/g3/`.

| Run | Start (local) | Load at start (1, 5, 15 min) | Load at end (1 min) | Frames | Process CPU (nested KWin, not the 144 Hz screen) | RSS |
|---|---|---|---|---|---|---|
| 1 | 07:40:31 | 5.68, 4.57, 4.84 | 3.82 | 300 | 16.99% | 370 MB |
| 2 | 07:41:08 | 3.82, 4.24, 4.72 | 2.99 | 300 | 16.56% | 299 MB |
| 3 | 07:41:44 | 2.99, 4.00, 4.63 | 2.07 | 300 | 16.76% | 304 MB |
| Median | | | | 300 | 16.76% | |

`window_active` was true and `stalls` was 0 in every run.

Why the median is higher than the 13.27% of the first run: the wlf.39 lane ran the code of
the first run (a binary built at `2b6adbf87`, which is `f62a4d71a` plus docs) in the same
session as `e44eb654a`, interleaved. The old code measured 17.22% and 17.52%, and
`e44eb654a` measured 16.84% to 17.15% (nested KWin, not the 144 Hz screen). So about 4 ms
per paint of today's number comes from the machine state, and the wlf.39 splice saves about
0.9 ms per paint. The cause of the machine-state difference is inferred, not proven: with
the desktop locked and its display off, the GPU stays in a low power state (P5 to P8, PCIe
link at Gen 1 or Gen 2, about 250 MHz), and the per-paint uploads and driver work may cost
more CPU there. A run with the display on would settle it.

Profile: one more 30 s run under `perf record -e cpu-clock:u -F 499 --call-graph
dwarf,16384 --user-callchains` (07:42:20, load 2.07): 16.77% process CPU (nested KWin, not
the 144 Hz screen), 300 frames, 2612 user-space samples. 98.8% of the samples (2580) are on
the UI main thread.

Top 10 self symbols, UI main thread. The libc rows at `0x1b18f0` to `0x1b1d00` are
`__memcpy_avx_unaligned_erms`, summed into one row:

| Share | Symbol |
|---|---|
| 48.37% | libc memcpy |
| 7.04% | `gpui_wgpu::wgpu_renderer::WgpuRendererCore::render_frame` |
| 4.79% | `gpui::bounds_tree::BoundsTree<ScaledPixels>::insert` |
| 2.99% | `tl_ui_chart::chart::geometry::piece_path` (the one paint copy) |
| 2.53% | `Vec<PathVertex<ScaledPixels>>` from iterator (`Path::scale`) |
| 2.30% | `gpui::scene::Scene::insert_primitive::<Path<ScaledPixels>>` |
| 1.23% | `taffy::compute::flexbox::compute_flexbox_layout` |
| 1.00% | `tl_ui_chart::chart::geometry::build_cell` |
| 1.00% | `tl_ui_chart::chart::geometry::decimate` |
| 0.80% | `gpui::window::ElementId` hash |

The stroke mesh rows of the first profile are gone from the top 10 (`StrokeMesh::stroke_line`
4.77%, `StrokeMesh::tri` 4.36%, `FillMesh::tri` 2.46% on 2026-11-04).

By phase (script `split2.py` from wlf.28, main-thread samples). Milliseconds per paint are
the share times 16.77% process CPU times the 98.8% main-thread share, so the total is
16.56 ms (nested KWin, not the 144 Hz screen):

| Phase | Share | ms per paint |
|---|---|---|
| memcpy (libc) | 48.4% | 8.01 |
| GPU submit (`render_frame`, wgpu, NVIDIA driver) | 11.1% | 1.84 |
| Other GPUI (element tree, styles, hashing, entities) | 8.3% | 1.37 |
| Scene build (BoundsTree, scene insert) | 8.3% | 1.37 |
| Chart paint (`tl_ui_chart`: geometry, copy, paint) | 7.5% | 1.24 |
| Layout (taffy) | 4.5% | 0.74 |
| malloc, free, other libc | 3.7% | 0.62 |
| Text (shaping, glyphs, atlas) | 3.0% | 0.49 |
| GPUI `Path::scale` copy | 2.6% | 0.42 |
| Rest | 1.6% | 0.26 |
| Other tasklens UI code | 1.2% | 0.20 |

Reading: tasklens chart code is now 1.24 ms per paint. About 15.3 ms per paint is outside
it, and memcpy alone is 8.0 ms. The stacks do not unwind out of libc, so the memcpy callers
are not measured. wlf.28 inferred them from the GPUI code: the scene clone, `Scene::replay`
of cached views, the raster vertex `Vec` and `write_buffer`. Those levers are in GPUI, and
the leader sent them to the owner in wlf.2 (options B, C and D).

Redraw counters after the rerun (`bench_scenes::redraw_work_static`, test platform): paints
2.2, sidebar renders 2.2, 9 cards renders 2.5, builds 2.0, paths 6.5, overall chart renders
1.2, builds 1.2, paths 3.6, CPU grid renders 1.2, builds 1.2, paths 3.6, text shapes 34.8.
These match the first run; wlf.39 made each grid build cheaper, not less frequent.

### Perf-cpu frame cost and pacing (tasklens-wlf.28, 2026-11-04)

Result: FAIL. perf-cpu used a median 12.21 ms of main-thread CPU per frame at p50 and
19.87 ms at p99, and 71.96% process CPU (nested KWin, not the 144 Hz screen). The
UI-PRF-053 gates are p50 at most 1.0 ms, p99 at most 2.0 ms and at most 30% process CPU.
The fixes go to tasklens-wlf.39 (path vertices per paint, perf-cpu numbers added there as a
comment) and the new tasklens-wlf.40 (views above the charts render again every frame).

The 24 to 31 fps pacing from wlf.1 had one cause: GPUI's 30 fps cap for a window that is
not active (comments 202 and 204 on wlf.28). The bench windows turn the cap off since
`14c20a556` (`inactive_frame_interval: None`). In the nested session the window is active,
and perf-cpu now draws 53 to 56 fps against a 60 Hz output. The rest of the gap is CPU:
about 1 frame in 9 takes more than one vsync.

#### Setup

- Hidden nested KWin (`kwin_wayland --virtual`, 1920x1200, 60 Hz output) started by
  `/var/tmp/claude/tl-smoke/harness.sh start wlf28`. The app ran on the nested Xwayland
  (X11), with its own `XDG_*` folders. Office PC, AMD Ryzen 9 5950X, RTX 5070 Ti, NVIDIA
  615.78.08.
- Build: `cargo build --locked --profile perf -p tl-app --features bench`, made at 12:59
  with HEAD at `2b6adbf87`. No crate or Cargo file changed from `f62a4d71a` to `157c22523`.
- Command per run: `tasklens --bench perf-cpu --duration 30 --warmup 5 --json <file>`, no
  perf attached, under the bench slot lock. Scripts and output are in
  `/var/tmp/claude/wlf28/`. The driver started each run only when the 1-minute load average
  was below 8.
- The real desktop (session 2) was locked during these runs. `bench_main.rs` refuses to
  start while any graphical session is locked. That rule is for the real screen, and the
  leader decision (wlf.28 comment 1004) replaces it with the nested session. As a
  workaround for these runs only, the wrapper `tlwrap.sh` pointed `DBUS_SYSTEM_BUS_ADDRESS`
  at a missing socket: the lock check could not ask logind and the run went ahead with a
  warning. tasklens-wlf.43 replaces this workaround. The nested KWin has no lock screen
  (`--no-lockscreen`), and the frame counts below show frames arrived the whole time.
- `frame_cpu_ms` is the main thread's CPU between two frame callbacks (`stats.rs`). It is
  close to the cost of one frame here, because the main thread does little else: 99.35% of
  the profile samples are on it.
- `missed_frames` is not valid at 60 Hz: until wlf.38 lands, `stats.rs` judges misses
  against 144 Hz, so every 16.7 ms interval counts as missed. `long_missed_frames` (above
  20.8 ms) is still useful: at 60 Hz it counts the intervals that took 2 vsyncs or more.

#### Runs

All values are nested KWin, not the 144 Hz screen.

| Run | Start (local) | Load at start (1, 5, 15 min) | Frames (fps) | Frame CPU p50 | Frame CPU p99 | Process CPU | Intervals above 20.8 ms | RSS |
|---|---|---|---|---|---|---|---|---|
| 1 | 18:55:57 | 7.01, 7.31, 10.95 | 1599 (53.3) | 12.90 ms | 19.87 ms | 72.05% | 196 | 373 MB |
| 2 | 18:56:34 | 5.72, 6.96, 10.68 | 1664 (55.5) | 12.21 ms | 19.23 ms | 71.96% | 131 | 306 MB |
| 3 | 18:57:10 | 4.68, 6.56, 10.40 | 1651 (55.0) | 12.18 ms | 19.93 ms | 70.60% | 138 | 309 MB |
| Median | | | 1651 (55.0) | 12.21 ms | 19.87 ms | 71.96% | 138 | |
| Gate | | | | 1.0 ms | 2.0 ms | 30% | | |

`window_active` was true and `stalls` was 0 in every run.

#### Frame pacing

A 60 Hz output gives 1800 vsyncs in 30 s. If every interval is either 1 or 2 vsyncs, the
number of 2-vsync intervals is 1800 minus the frame count: 201, 136 and 149 for the three
runs. The bench counted 196, 131 and 138 intervals above 20.8 ms. The two agree within 11,
so frames arrive on vsync and about 8% to 12% of them slip one vsync.

The slips come from CPU, not from the compositor or our driver:

- The mean interval is 18.0 to 18.8 ms. The mean main-thread CPU per frame (process CPU
  times 30 s, divided by frames) is 12.8 to 13.5 ms. That leaves about 5 ms per frame of
  waiting for the next vsync.
- Frame CPU p99 (19.2 to 19.9 ms) is above the 16.67 ms period, so the slowest frames miss
  their vsync.
- `window_active` was true, so the 30 fps cap does not apply.

On the owner's 144 Hz screen the period is 6.94 ms. At about 13 ms per frame each frame
would need 2 vsyncs, so the best case there is about 72 fps. That is inferred from these
numbers, not measured.

#### Profile

One more 30 s run under `perf record -e cpu-clock:u -F 499 --call-graph dwarf,16384`
(18:57:47, load 4.46): 1453 frames (48.4 fps), frame CPU p50 14.28 ms, 70.98% process CPU,
11,221 user-space samples. 99.35% of the samples are on the UI main thread; the bench
publisher thread has 0.25% and NVIDIA's present thread 0.24%.

Top 10 self symbols, as a share of the 11,148 main-thread samples:

| Share | Symbol |
|---|---|
| 37.06% | libc `__memcpy_avx_unaligned_erms`, its 128-byte copy loop (0x1b1b24 to 0x1b1bca, summed) |
| 9.08% | `gpui_wgpu::wgpu_renderer::WgpuRendererCore::render_frame` |
| 4.92% | `gpui::bounds_tree::BoundsTree<ScaledPixels>::insert` |
| 4.01% | `Vec<PathVertex<Pixels>>` from iterator (`tl_ui_chart` `translated` copy) |
| 3.50% | `Vec<PathVertex<ScaledPixels>>` from iterator (GPUI `Path::scale` copy) |
| 3.28% | `gpui::scene::Scene::insert_primitive::<Path<ScaledPixels>>` |
| 2.48% | `taffy::compute::flexbox::compute_flexbox_layout` |
| 0.81% | `tl_ui_chart::chart::geometry::StrokeMesh::stroke_line` |
| 0.67% | `tl_ui_chart::chart::geometry::StrokeMesh::tri` |
| 0.66% | `taffy::compute::block::compute_inner` |

The libc address was named with `objdump -d /usr/lib/libc.so.6`: the hot addresses sit in
`__memcpy_avx_unaligned_erms`, in its loop of four 32-byte loads and stores.

#### Frame time by phase

Shares are self time of the UI main thread: sample counts from
`perf report --no-children -g none -n --tid 3845830 --sort dso,sym` (11,148 samples, 1,188
rows), split by `split2.py`. Each row of the report goes to the first phase that matches it,
so the phases do not overlap and their counts add up to all 11,148 samples (100.0%). The
earlier version of this table summed the rounded percentages that perf prints; perf shows
731 one-sample rows (0.0089% each) as 0.01%, so that sum came to 101.5%.

The ms column applies each share to the median run's main-thread CPU per frame: 12.97 ms of
process CPU per frame (process CPU 71.96%, times 30 s, divided by 1664 frames), times the
main thread's 99.35% of all samples, gives 12.89 ms. Wait time comes from the runs, not the
profile.

| Phase | Samples | Share | ms per frame (nested KWin, not the 144 Hz screen) |
|---|---|---|---|
| Large copies (libc `__memcpy_avx_unaligned_erms`) | 4,263 | 38.24% | 4.93 |
| GPU submit (`render_frame`, wgpu, NVIDIA driver) | 1,568 | 14.07% | 1.81 |
| Other GPUI work (element tree, styles, `ElementId` hashing, entities) | 1,317 | 11.81% | 1.52 |
| Scene build (`BoundsTree::insert`, `Scene::insert_primitive`) | 1,006 | 9.02% | 1.16 |
| Our chart paint (`tl_ui_chart`: geometry, `translated` copy, paint) | 850 | 7.62% | 0.98 |
| GPUI layout (taffy) | 792 | 7.10% | 0.92 |
| malloc, free and other libc | 394 | 3.53% | 0.46 |
| GPUI `Path::scale` copy | 392 | 3.52% | 0.45 |
| Text (shaping, glyphs, atlas) | 307 | 2.75% | 0.35 |
| Other (not matched above) | 134 | 1.20% | 0.15 |
| Our other UI code (`tl_ui*`, `tl_model`) | 125 | 1.12% | 0.14 |
| Total main-thread CPU | 11,148 | 100.00% | 12.89 |
| Waiting for vsync (mean interval 18.03 ms minus 12.89 ms CPU) | | | 5.14 |

Our own code is 975 samples (8.75% of the main-thread CPU), about 1.1 ms per frame. GPUI,
libc and the driver are 10,039 samples (90.05%), and 134 samples (1.20%) match no phase. GPUI's cost follows what we hand it:
path vertices and views that render.

#### Reading

The call stacks did not unwind (10,763 of 11,221 samples have no call stack at all), so the
callers of the copy loop are inferred from the GPUI 0.3.7 code, not measured. Each path
vertex is copied several times per frame inside GPUI:

- `Scene::insert_primitive` clones every path it stores (`scene.rs:114`).
- `Scene::replay` clones every path of a cached view into the new scene
  (`scene.rs:144`), so the 9 unchanged sidebar cards still cost a copy each frame.
- `render_frame` builds a 104-byte `PathRasterizationVertex` for each 32-byte path vertex
  (it carries a full 72-byte `Background`), into a `Vec` that starts empty each frame
  (`wgpu_renderer.rs:1842`).
- `queue.write_buffer` copies that `Vec` into the upload buffer.

This explains why the copy share (38%) is higher here than in the static scene (about
22%): perf-cpu paints every frame, and every frame re-copies all paths on screen.

The layout and element share (taffy 7.1%, other GPUI 11.8%, text 2.8%) has a second cause.
The driver notifies the moving chart views each frame, and GPUI marks every ancestor view
dirty (`window.rs:2148`, `mark_view_dirty`). So the CPU page, the Performance page and the
window root render again each frame, with all their children that are not cached views.
This is inferred from the code; tasklens-wlf.40 starts with a counter test to measure it.

#### Fix beads and the GPUI question

- tasklens-wlf.39 (open, from wlf.27): fewer path vertices per paint. It cuts our chart
  paint and, in proportion, the GPUI copies, scene build and GPU submit. The perf-cpu
  numbers above are in a comment there, not in a second bead.
- tasklens-wlf.40 (new): keep the views above the moving charts cheap, so a frame renders
  only the charts. Target: the 2.8 ms of layout, element and text work.
- tasklens-wlf.31 (open): pixel-paced chart scrolling. It lowers the frame count, which
  helps the 30% process CPU gate, but not the per-frame CPU gate.

The gate needs 1.0 ms per frame; today it is about 13 ms. Our own code is about 1.1 ms of
that. The rest is GPUI work that grows with what we submit, so wlf.39 and wlf.40 are the
first steps and need no GPUI change. I do not know whether they can reach 1.0 ms on their
own. The per-vertex copies listed in Reading would need a GPUI change, which is the owner's
decision (bead AC, ADR-014 rule 3). Re-measure after wlf.39 and wlf.40 before deciding.

### History scrub gate (tasklens-8fx.41, 2026-11-05)

Result: FAIL. Median `scrub_frame_ms.p99` is 6.59 ms (hidden nested KWin, 60 Hz), gate 4.0 ms.
Fix bead: tasklens-8fx.47. All numbers here are close to, not exactly, the 144 Hz screen.

#### Setup

- Same session and run script as the wlf.18 gate (`/var/tmp/claude/8fx41/harness.sh`, `run.sh`):
  `kwin_wayland --virtual`, 1920 x 1200, `--scale 1.1`, one 60 Hz output, `TL_BENCH_REFRESH_HZ=60`,
  private `XDG_*` folders, bench slot lock, nonexistent system bus to skip the lock check.
  Office PC, Ryzen 9 5950X, RTX 5070 Ti.
- Command: `cargo xtask bench --scene history-scrub --runs 3`. HEAD 48de50342 (summary says
  `-dirty`: `.beads/issues.jsonl` and untracked folders). No Rust file was changed.
- The gate metric is `scrub_frame_ms.p99`. xtask's own table shows `frame_cpu_ms` and picks its
  median run by that, so the values below are read from each run JSON
  (`/var/tmp/claude/8fx41/history-scrub-{1,2,3}.json`).

#### Runs

All values are hidden nested KWin, 60 Hz. Load is the 1-minute load average from `load.log` at
the run start (report mtime minus 35 s).

Attempt 1 is void: xtask rebuilt `tl-app` inside the run, so the load was 27.5 at run 1 and
11 to 17 at runs 2 and 3. Its p99 values were 7.10, 8.89 and 13.08 ms (median 8.89 ms). Files
are in `/var/tmp/claude/8fx41/attempt1/`.

Attempt 2 (counts as the gate):

| Run | Start | Load 1 min | scrub p50 | p95 | p99 | max | scrub_frames | missed_frames |
|---|---|---|---|---|---|---|---|---|
| 1 | 01:44:16 | 7.6 | 5.23 ms | 8.04 ms | 9.26 ms | 13.06 ms | 1692 | 43 |
| 2 | 01:44:51 | 9.0 (run 1 just ended) | 4.48 ms | 5.42 ms | 6.41 ms | 9.92 ms | 1799 | 0 |
| 3 | 01:45:27 | 6.6 | 4.42 ms | 5.72 ms | 6.59 ms | 9.25 ms | 1799 | 0 |

Run 2 started with load slightly above 8 because of run 1's own load. Its p99 is the lowest of
the three, so the median is not inflated by it. Missed frames use the 60 Hz limits (25.0 ms,
50.0 ms); there is no missed-frame gate for this scene.

#### Gate

| Metric | Rule | Median of 3 | Result |
|---|---|---|---|
| `scrub_frame_ms.p99` (UI-PRC-032) | <= 4.0 ms | 6.59 ms (hidden nested KWin, 60 Hz) | FAIL |

Even p50 (4.42 to 5.23 ms) is above 4.0 ms.

#### Profile

One profiled run (profiling build, relinked as in the wlf.41 section, `perf record -e cpu-clock:u
-F 499 --call-graph dwarf,65528`): p99 6.58 ms, load 7.35 at start. 4,906 main-thread samples,
2,960 (60%) reach `main`, so shares are lower bounds. Files: `/var/tmp/claude/8fx41/`
(`prof.data`, `script.txt`).

Inclusive share of main-thread samples whose stack holds the frame (hidden nested KWin, 60 Hz):

| Frame | Share |
|---|---|
| `HistoryTableDelegate` (any method) | 40.7% |
| taffy | 28.1% |
| `lyon` (path tessellation) | 17.0% |
| `render_frame` (GPU submit) | 12.7% |
| `StyledText` | 8.7% |
| `sort` | 7.0% |
| `BoundsTree` | 5.4% |
| `HistoryTableDelegate::rebuild` | about 4% (any frame matching `rebuild`: 4.1%) |

By kind (exclusive): GPUI elements 44.6%, GPU submit 17.5%, taffy 15.3%, text 10.4%, our code
8.3%, scene build 3.7%. These match the 2026-10-15 suspects: lyon fill tessellation of the chart
and scrubber paths, taffy layout, `BoundsTree::insert`, and row re-sorting. The fix belongs in
`crates/tl-ui-procs/src/pages/processes/history/`, not in the bench scene (tasklens-8fx.47).

### Bridge wake-to-render latency, nested rerun (tasklens-wlf.42, 2026-11-05)

Information only (leader 2026-11-05, option C), not a gate result. The 20 ms bridge gate is judged on the 144 Hz screen in wlf.2.

Setup: HEAD 4489481b1 (dirty: `.beads/issues.jsonl` and untracked files of other lanes), perf profile with the `bench` feature, `cargo xtask bench --scene bridge --runs 3`, nested KWin `wlf42` (1920x1200, 60 Hz, Wayland, scale 1.1, hidden), `TL_BENCH_REFRESH_HZ=60`, `TL_BENCH_NESTED=1`. No code changed since the wlf.18 run apart from wlf.39 and wlf.40. Data: `/var/tmp/claude/wlf42/`.

Attempt 1 run 1 did not count. xtask compiled tl-app just before it, so it started at 1-minute load 9.97, above the limit of 8. Per the quiet-machine rule the whole script was rerun. Attempt 2 is the counted set. Both are listed.

| Attempt, run | Start | Load at start (1 min, 5 min) | Load at end (1 min) | Frames | frame_cpu p50 / p99 ms | Missed frames | Process CPU % | bridge p50 / p99 ms | Counts |
|---|---|---|---|---|---|---|---|---|---|
| 1, run 1 | 07:53:58 | 9.97, 6.79 | 6.55 | 1420 | 14.32 / 18.24 | 340 | 69.4 | 12.69 / 27.83 | no |
| 1, run 2 | 07:54:34 | 6.55, 6.31 | 5.05 | 1219 | 14.69 / 20.86 | 577 | 60.6 | 18.23 / 35.24 | yes |
| 1, run 3 | 07:55:09 | 5.05, 5.97 | 5.07 | 1267 | 14.41 / 19.41 | 528 | 62.8 | 9.94 / 26.30 | yes |
| 2, run 1 | 07:56:01 | 4.6, 5.8 | 4.35 | 1239 | 14.90 / 19.46 | 554 | 62.9 | 8.40 / 25.08 | yes |
| 2, run 2 | 07:56:37 | 4.3, 5.6 | 3.6 | 1498 | 11.91 / 18.92 | 299 | 66.2 | 9.77 / 27.16 | yes |
| 2, run 3 | 07:57:12 | 3.6, 5.2 | 3.7 | 1321 | 14.40 / 19.43 | 468 | 63.6 | 10.27 / 27.46 | yes |

All numbers are (hidden nested KWin, 60 Hz). Start times are file mtime minus 35 s.

Counted attempt 2: median bridge p99 is 27.16 ms (hidden nested KWin, 60 Hz). Reference values:

- 20 ms: the 144 Hz gate, judged in wlf.2. The nested value is above it.
- 29.7 ms: the gate scaled by one 60 Hz frame (13.06 + 16.67). The nested value is below it.

wlf.18 had median p99 30.20 ms and p50 13.46, 5.94 and 13.71 ms. Median p99 is now 3.04 ms lower, and p50 is now 8.40, 9.77 and 10.27 ms. Frame CPU p50 is still 11.9 to 14.9 ms, so frames still take most of a 16.7 ms interval and the bridge latency still includes waiting for the next frame callback.

## First frame, minimized CPU and RSS

### Gate runs (tasklens-wlf.19, 2026-11-04)

Result: all four gates FAIL. Warm first frame median 660 ms (gate 200 ms), cold first frame
1240 ms (gate 500 ms), minimized CPU 1.13% (gate 0.3%), RSS after `table-2000` 238.33 MB
(gate 150 MB). Every number comes from a hidden nested KWin session, not the 144 Hz screen.

#### Setup

- Binary: `/usr/bin/tasklens`, `tasklens 1.5.2`, the release build without bench code.
- Session: hidden nested KWin (`harness.sh start wlf19`), app on the nested Xwayland (X11),
  private XDG folders. `history.enabled` is false by default, so recording is off.
- Load rule: a run starts only when the 1-minute load average is below 8. One untimed launch
  ran first so the binary was in the page cache.
- `first_frame_ms` comes from `/proc/uptime`, so it has 10 ms resolution.
- Each launch waited for a new `first_frame_ms` line in the newest `tasklens.*.log`, then
  stopped the app by its PID.

#### First frame, warm

| Run | `first_frame_ms` | Load at start (1, 5, 15 min) |
|---|---|---|
| 1 | 710 | 5.05, 4.56, 7.00 |
| 2 | 660 | 5.77, 4.72, 7.03 |
| 3 | 800 | 5.77, 4.72, 7.03 |
| 4 | 590 | 7.50, 7.64, 7.92 |
| 5 | 490 | 7.50, 7.64, 7.92 |
| Median | 660 | |

Gate: median <= 200 ms. FAIL (hidden nested KWin, not the 144 Hz screen). Runs 2 and 3, and
runs 4 and 5, share one load reading because they started within the same load sample.

#### First frame, cold

1240 ms. Gate <= 500 ms. FAIL (hidden nested KWin, not the 144 Hz screen). Run by the agent
with `sudo sysctl vm.drop_caches=3` after `sync`, in place of the owner (owner decision,
bead comment 1007). Load at start 6.66, 7.44, 7.86.

#### Minimized CPU

| Item | Value |
|---|---|
| Window state | minimized with `xdotool windowminimize`, `WM_STATE` Iconic |
| `CLK_TCK` | 100 |
| `utime + stime` at t = 0 (10 s after minimizing) | 187 ticks |
| `utime + stime` at t = 60 s | 255 ticks |
| Delta | 68 ticks |
| CPU | 100 x 68 / 100 / 60 = 1.13% of one core |
| Load at start | 6.25, 7.28, 7.79 |

Gate: <= 0.3%. FAIL (hidden nested KWin, not the 144 Hz screen).

#### RSS

Copied from the gate-run table of the scene section above.

| Gate | Limit | Result | Runs | Verdict |
|---|---|---|---|---|
| RSS after `table-2000` | <= 150 MB | 238.33 MB (hidden nested KWin, 60 Hz) | 238.33, 239.01, 238.04 | FAIL |

#### Profile for the fix beads

Start-up steps, median of the 5 warm runs, from the `tasklens::perf` step lines (ms since
the previous step):

| Step | Median ms |
|---|---|
| `logging` | 2.1 |
| `single_instance` | 1.6 |
| `args_settings` | 0.2 |
| `engine_start` | 21.2 |
| `platform` | 45.2 |
| `fonts` | 2.3 |
| `kit` | 3.4 |
| `ui_init` | 4.8 |
| `window_open` | 570.4 |
| `first_frame` | 0.5 |

`window_open` is 570 ms here, against 276 ms in the cr0.23 run on the real screen. It holds
gpui's adapter probe, which tasklens cannot change without a gpui patch (see the cr0.23
section below). I did not measure why it is slower in the nested session.
The cold run is a single run and also pays for reading the binary and libraries from disk.

Minimized CPU: this was an extra run, not a gate run (`perf` is blocked here,
`perf_event_paranoid` is 2, so per-thread ticks from `/proc/<pid>/task/*/stat` were used).
Over 30 s after a 10 s settle, the whole process used 31 ticks (1.03%). By thread:

| Thread | Ticks in 30 s |
|---|---|
| `tl-src-Processes` | 9 |
| `tl-engine-0` | 8 |
| `tl-src-Cpu` | 4 |
| `tasklens` (two threads) | 3 + 3 |
| `tl-src-Net` | 2 |
| `tl-src-Sensors` | 1 |
| `tl-src-Disk` | 1 |

The engine sources keep sampling while the window is minimized, with recording off. The
fix is to turn demand off for every source when the main window is hidden or minimized.

### Start-up timing (tasklens-cr0.23, 2026-10-07)

Result: the warm median `first_frame_ms` is 365 ms after this bead, against 385 ms before it.
The ADR-014 warm gate is 200 ms. The gate cannot pass while gpui's GPU adapter probe takes
about 265 ms by itself. tasklens code before the window opens takes about 12 ms in total.

#### Setup

- Owner desktop: KDE Plasma Wayland (`wayland-0`), RTX 5070 Ti, Vulkan, 32 logical CPUs.
- Release build of `tl-app`. `XDG_STATE_HOME` was a fresh folder under `/var/tmp/claude/` for
  every run, so each run started with no state and default settings.
- Each run waited for the `first frame` log line, then stopped the process by its PID.
- Other agents were building at the same time. The 1-minute load average was 25 to 30 during
  the before and after runs, and 37 to 53 during the per-step runs. All numbers here are noisy.
  The owner reruns the gate on an idle machine.
- `first_frame_ms` comes from `/proc/uptime`, so it has 10 ms resolution.

#### Before and after

Runs alternated between the two binaries, three rounds of three runs each. The first run of
round 1 for each binary was a cold start (binary just copied, not in the page cache), and it
is left out of the warm median.

| Binary | Cold run | Warm runs (ms) | Warm median |
|---|---|---|---|
| Before (`bb18afc`) | 2660 | 540, 580, 390, 380, 360, 380, 440, 370 | 385 |
| After (`9515dab`) | 1060 | 430, 540, 370, 370, 360, 350, 360, 360 | 365 |

#### Per-step times, after

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

#### What changed

- `tasklens::perf` step lines in `crates/tl-app/src/main.rs`, `crates/tl-ui/src/app.rs` and
  `crates/tl-ui/src/window/main.rs` (`41b0299`).
- The placeholder logo `assets/icons/tasklens.svg` drew "tl" with an SVG `<text>` element.
  The first SVG with text makes gpui's SVG renderer load every system font on the main thread
  during the first frame: 1134 faces here, 36 to 47 ms warm and 1040 ms cold, measured with
  fontdb 0.23 on its own. It also logged three warnings: `IBMPlexSans-Regular.ttf` and
  `Lilex-Regular.ttf` not found, and no match for `monospace`. The logo now draws the glyphs
  of the bundled Noto Sans Mono Regular as one path (`9515dab`). The warnings are gone, and
  the cold start dropped from 2660 ms to 1060 ms in this run.

#### What was not changed, and why

- Starting the engine on another thread while the window opens: `Engine::start` takes 0.7 ms,
  so running it in parallel would save less than the 10 ms resolution of `first_frame_ms`.
- Settings load: tl-app still uses default settings and reads no file before the window
  opens. E33-A2 adds the settings load. It must stay small or move off the window path.
- Registering Noto as gpui's SVG fallback font: gpui asks the asset source for its own IBM
  Plex Sans and Lilex paths and maps generic families to those names. Serving Noto bytes at
  those paths would not satisfy the `monospace` lookup. With no SVG text left, gpui never
  makes the lookup.

#### Adapter probe time (finding)

The adapter probe inside gpui takes about 265 ms (176 + 89) on this machine. That alone is
over the 200 ms warm gate. tasklens cannot change it without patching gpui. Two causes in
`gpui-pre-wgpu` 0.3.7 (`wgpu_context.rs`):

- `WgpuContext::instance` always enables the Vulkan and GL backends. GL means an EGL set-up,
  and `enumerate_adapters` also lists llvmpipe. No environment variable changes this.
- `select_adapter_and_device` creates a full device and configures the surface to test each
  adapter before it picks one. This takes 80 to 120 ms even for the first adapter.

Options for the owner: accept a higher warm gate on this hardware, patch gpui to drop the GL
backend when Vulkan works, or cache the chosen adapter between runs.

## zed#64499 soak

Run record: tasklens-wlf.20, 2026-10-07.

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

## zed#64499 patch (only if the soak failed)

Not run yet.

## Manual checklist

Not run yet.

## polkit error names (U-39)

Not run yet.

## Unverified items settled in phase 0

- U-47 not settled: `perf-cpu` p99 is 21.94 ms (hidden nested KWin, 60 Hz), against a gate of
  2.0 ms. FAIL, not isolated: the CPU page has no switch that holds the per-core labels static.
  The wlf.28 profile puts the cost in chart paths and GPUI copies (wlf.39, wlf.40). The labels
  stay at 10 Hz until this is measured again after those beads. R-126 moves them to 5 Hz only
  when the labels are shown to cause the failure, and this run does not show that.
- U-9 not needed: the soak passed without the patch.


## GPU power baseline

Not run yet.

## Collection cost

The live collection cost measurement is not run yet. The fixture sweep gate is below.

### Sweep of 2,000 processes on the fixture root (tasklens-yms.26, 2026-11-04)

Result: PASS. The median steady sweep of the 2,000-process fixture root was 19.19, 19.36 and
19.27 ms in three runs. The R-286 gate is at most 20 ms (it replaced the 15 ms of the bead
text). Each run is 50 sweeps. All numbers are from a hidden nested KWin session and a fixture
root on btrfs, so they are close to, but not equal to, a sweep of the live `/proc`.

#### Setup

- Office PC (`oystein-office-primary`), AMD Ryzen 9 5950X, 32 threads. The fixture root and
  `target/` are on btrfs.
- Hidden nested KWin started by `/var/tmp/claude/tl-smoke/harness.sh start yms26`. Each
  command ran with that session's env and its own `XDG_*` folders. The sweep opens no window.
- Build: release (the `bench` profile for Criterion) at `9fbf803aa`. Since the 2026-10-05
  runs, the sweep gained the `/proc/stat` CPU window, per-cgroup `cpu.stat` reads and Wine
  prefix reads. `9fbf803aa` copies the fixture `proc/stat` into the scaled root, so the bench
  now pays the `cpu` line read that a real sweep pays.
- Scaled root: the `owner-desktop` pid directories plus replicas, exactly 2,000. The fixture
  has no `sys/fs/cgroup` tree, so the root has an empty cgroup folder and no `cpu.stat`
  reads. This understates the live cost.
- Gate: `cargo test -p tl-collect --release --test fixture_process sweep_2000_under_gate --
  --ignored`. The median, min and max came from a temporary copy of the same 50-sweep loop
  that prints them, run right after each gate run (not committed).
- Each run started only when the 1-minute load average was at most 8.

#### Runs

| Run | Start (local) | Load at start (1, 5, 15 min) | Gate test | Median (nested KWin, fixture root on btrfs) | Min | Max |
|---|---|---|---|---|---|---|
| 1 | 22:33:26 | 5.03, 6.54, 8.15 | ok | 19.19 ms | 18.80 ms | 20.48 ms |
| 2 | 22:33:31 | 4.87, 6.48, 8.12 | ok | 19.36 ms | 18.97 ms | 20.56 ms |
| 3 | 22:33:37 | 4.88, 6.45, 8.10 | ok | 19.27 ms | 19.00 ms | 22.26 ms |

#### Criterion

`cargo bench -p tl-collect --bench sweep`, started 22:33:46 at load 4.50, 6.32, 8.04.

| Bench | Mean [95% interval] (nested KWin, fixture root on btrfs) | Gated |
|---|---|---|
| `sweep/2000_steady` | 18.78 ms [18.68, 18.88] | no, reported |
| `sweep/2000_with_fd_count` | 25.04 ms [24.95, 25.14] | no, reported |

#### Reading

- The margin is small: about 0.7 ms under the gate. On 2026-10-05, at load 8 to 10, the
  medians were 23.9 ms, under 20 ms and 22.4 ms, and Criterion read 20.33 ms. Load is the
  likely cause of most of that gap (inferred, not measured).
- The gate holds on the fixture root only. Live `/proc` costs more per process (2026-10-05:
  about 42 to 47 ms at 2,000 processes, scaled from 997). U-10 is settled by R-286, and the
  binding product gate stays ADR-014's 3.5% of one core.

### Sweep gate after the pid reuse fix (tasklens-yms.26, 2026-11-05)

Result: PASS. With the fix at `bea6514e0`, the median steady sweep was 18.68, 18.68 and
18.81 ms in the three runs that count. The gate is at most 20 ms (R-286). A side-by-side
comparison found no measurable cost from the fix. All numbers are from the hidden nested KWin
session `yms26b` and a fixture root on btrfs, so they are close to, but not equal to, a sweep
of the live `/proc`.

#### What changed

A review found that the sweep opens each file as `<pid>/<file>`, so a pid freed and reused
between two opens could put another process's memory, status or metadata under the old key.
`c81fd8702` and `bea6514e0` fix it in `crates/tl-collect/src/proc/sweep.rs`:

- The sweep opens an anchor file before `stat` and reads it after every other file of the
  process. The anchor is `statm`, or `status` in memory-only mode. A `/proc` fd stays bound to
  its process and its read fails once that process is gone, so a good anchor read proves that
  one process held the pid for every open in between. Both files were already read every
  sweep, so this adds no read.
- The `/proc/<pid>` directory for a metadata read or an fd count is opened inside the same
  window (`PerPid::wants_dir`, `MetaCache::needs_read`).
- The P2 extras of detail keys are read by pid after the window. For those, the sampler reads
  the kept `stat` fd again and drops the process when its start time no longer matches.

The first version (`c81fd8702`) re-read `stat` after each metadata refresh, about 200 reads
per sweep. In 5 pairs it read a median 0.41 ms slower than the code before the fix, always
running second, so part of that is the order effect described below. `bea6514e0` removed that
read.

#### Setup

- Office PC, AMD Ryzen 9 5950X, 32 threads, btrfs.
- Hidden nested KWin from `/var/tmp/claude/tl-smoke/harness.sh start yms26b`, with its env
  and its own `XDG_CACHE_HOME`. The sweep opens no window.
- Release build of `fixture_process` at `bea6514e0`. Gate runs called the built test binary
  directly in `builds.slice` (CPUQuota 200%), with `sweep_2000_under_gate --ignored`, because
  heavy-build refuses a third run of a command that failed twice. Criterion ran through
  `heavy-build cargo bench -p tl-collect --bench sweep`.
- A run counts only when the 1-minute and 5-minute load averages are both at most 8 at its
  start and the 1-minute load is still at most 8 at its end.

#### Gate runs

| Run | Start (local) | Load at start (1, 5, 15 min) | Gate test | Median (nested KWin, fixture root on btrfs) | Min | Max | Counts |
|---|---|---|---|---|---|---|---|
| 1 | 00:47:19 | 6.06, 7.95, 12.13 | ok | 18.68 ms | 17.53 ms | 20.56 ms | yes |
| 2 | 00:47:41 | 6.37, 7.89, 12.02 | failed | 20.12 ms | 18.90 ms | 29.46 ms | no: 1-minute load 10.98 at the end |
| 3 | 01:06:03 | 3.40, 7.82, 12.33 | ok | 18.68 ms | 18.14 ms | 19.81 ms | yes |
| 4 | 01:07:07 | 4.90, 7.46, 11.92 | ok | 18.81 ms | 16.59 ms | 21.67 ms | yes |

Earlier the same night, at 1-minute load 6 to 7 but 5-minute load 10 to 14, three runs read
19.74, 20.35 and 20.76 ms (1 of 3 under the gate). The code from before the fix read 19.6 to
20.9 ms under similar load, so that night's load explains the failures, not the fix.

#### Criterion

Started 01:06:25 at load 3.88, 7.64, 12.18.

| Bench | Mean [95% interval] (nested KWin, fixture root on btrfs) | Gated |
|---|---|---|
| `sweep/2000_steady` | 19.18 ms [18.94, 19.42] | no, reported |
| `sweep/2000_with_fd_count` | 25.26 ms [25.12, 25.41] | no, reported |

On 2026-11-04 at load 4.5 the same benches read 18.78 ms and 25.04 ms.

#### Comparison with the code before the fix

The binary from `9f251635b` and the binary from `bea6514e0` were both built in one worktree,
so both build their scaled root in the same folder. Medians in ms, 1-minute load 2.6 to 3.7:

| Pair | Order | Before the fix | With the fix |
|---|---|---|---|
| 1 | with the fix first | 18.11 | 17.25 |
| 2 | before the fix first | 15.63 | 18.38 |
| 3 | with the fix first | 17.71 | 16.51 |

In each pair the binary that ran second read slower, so run order moves the median by 1 to
3 ms. Neither binary is faster across the pairs. Five earlier pairs that always ran the fixed
binary second read it 0.15 to 2.06 ms slower, which fits that order effect (a sixth pair hit a
load spike and is left out). By the code, the
steady sweep makes the same syscalls as before plus one hash lookup per pid in
`MetaCache::needs_read`.

#### Reading

- The margin is about 1.2 ms under the gate at quiet load. A 5-minute load above 8 is enough
  to push the median over 20 ms with or without the fix.
- The fixture still has no cgroup tree, so these runs make no `cpu.stat` reads. This
  understates the live cost.

## Decision

Not run yet.
