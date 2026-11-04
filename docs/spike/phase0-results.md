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

### Static scene idle CPU (tasklens-wlf.27, 2026-11-04)

Result: FAIL. The static scene used a median 13.27% process CPU of one core over 30 s
(nested KWin, not the 144 Hz screen). The UI-PRF-053 gate is at most 2%. The fix goes to a
new bead with the profile below (owner decision, wlf.27 comment 1005).

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
  leader decision (wlf.28 comment 1004) replaces it with the nested session. So the wrapper
  `tlwrap.sh` points `DBUS_SYSTEM_BUS_ADDRESS` at a missing socket: the lock check cannot
  ask logind and the run goes ahead with a warning. The nested KWin has no lock screen
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

Top 10 self symbols:

| Share | Symbol |
|---|---|
| 38.0% | libc `__memcpy_avx_unaligned_erms`, its 128-byte copy loop (0x1b1b24 to 0x1b1bca, summed) |
| 9.02% | `gpui_wgpu::wgpu_renderer::WgpuRendererCore::render_frame` |
| 4.89% | `gpui::bounds_tree::BoundsTree<ScaledPixels>::insert` |
| 3.98% | `Vec<PathVertex<Pixels>>` from iterator (`tl_ui_chart` `translated` copy) |
| 3.48% | `Vec<PathVertex<ScaledPixels>>` from iterator (GPUI `Path::scale` copy) |
| 3.26% | `gpui::scene::Scene::insert_primitive::<Path<ScaledPixels>>` |
| 2.47% | `taffy::compute::flexbox::compute_flexbox_layout` |
| 0.80% | `tl_ui_chart::chart::geometry::StrokeMesh::stroke_line` |
| 0.67% | `tl_ui_chart::chart::geometry::StrokeMesh::tri` |
| 0.66% | `taffy::compute::block::compute_inner` |

The libc address was named with `objdump -d /usr/lib/libc.so.6`: the hot addresses sit in
`__memcpy_avx_unaligned_erms`, in its loop of four 32-byte loads and stores.

#### Frame time by phase

Shares are self time from the profile (script `split.py`). The ms column applies each share
to the median run's 12.97 ms of CPU per frame (process CPU 71.96%, times 30 s, divided by
1664 frames). Shares are rounded, so they add up to 101.5%. Wait time comes from the runs,
not the profile.

| Phase | Share | ms per frame (nested KWin, not the 144 Hz screen) |
|---|---|---|
| Large copies (libc memcpy) | 38.0% | 4.93 |
| GPU submit (`render_frame`, wgpu, NVIDIA driver) | 14.6% | 1.89 |
| Other GPUI work (element tree, styles, `ElementId` hashing, entities) | 12.1% | 1.58 |
| Scene build (`BoundsTree::insert`, `Scene::insert_primitive`) | 9.0% | 1.16 |
| Our chart paint (`tl_ui_chart`: geometry, `translated` copy, paint) | 7.6% | 0.99 |
| GPUI layout (taffy) | 7.1% | 0.92 |
| malloc, free and other libc | 4.0% | 0.52 |
| GPUI `Path::scale` copy | 3.5% | 0.45 |
| Text (shaping, glyphs, atlas) | 2.8% | 0.37 |
| Our other UI code (`tl_ui*`, `tl_model`) | 1.4% | 0.19 |
| Other | 1.3% | 0.17 |
| Waiting for vsync (mean interval 18.03 ms minus 12.97 ms CPU) | | 5.06 |

Our own code is 9.0% of the CPU, about 1.2 ms per frame. GPUI and the driver are the other
91%. Their cost follows what we hand them: path vertices and views that render.

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

The layout and element share (taffy 7.1%, other GPUI 12.1%, text 2.8%) has a second cause.
The driver notifies the moving chart views each frame, and GPUI marks every ancestor view
dirty (`window.rs:2148`, `mark_view_dirty`). So the CPU page, the Performance page and the
window root render again each frame, with all their children that are not cached views.
This is inferred from the code; tasklens-wlf.40 starts with a counter test to measure it.

#### Fix beads and the GPUI question

- tasklens-wlf.39 (open, from wlf.27): fewer path vertices per paint. It cuts our chart
  paint and, in proportion, the GPUI copies, scene build and GPU submit. The perf-cpu
  numbers above are in a comment there, not in a second bead.
- tasklens-wlf.40 (new): keep the views above the moving charts cheap, so a frame renders
  only the charts. Target: the 2.9 ms of layout, element and text work.
- tasklens-wlf.31 (open): pixel-paced chart scrolling. It lowers the frame count, which
  helps the 30% process CPU gate, but not the per-frame CPU gate.

The gate needs 1.0 ms per frame; today it is about 13 ms. Our own code is about 1.2 ms of
that. The rest is GPUI work that grows with what we submit, so wlf.39 and wlf.40 are the
first steps and need no GPUI change. I do not know whether they can reach 1.0 ms on their
own. The per-vertex copies listed in Reading would need a GPUI change, which is the owner's
decision (bead AC, ADR-014 rule 3). Re-measure after wlf.39 and wlf.40 before deciding.

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

Not run yet.

## Decision

Not run yet.
