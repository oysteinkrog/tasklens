# GPU Idle Validation of the Release GUI (tasklens-x12tr.5)

Measured 2026-11-11 on the office PC (RTX 5070 Ti, NVIDIA driver 615.78.08). Build: release
profile of commit 115579a0e, which includes x12tr.1 to x12tr.4.

Caveat: the GUI ran in a nested KWin session at 60 Hz (X11 through Xwayland), not on the real
screen. It ran on the NVIDIA GPU (47 `/dev/nvidia*` fds, listed by `nvidia-smi`). The window was
kept active for the whole run, so x12tr.4's slow tick for inactive windows did not apply.

## Method

- `target/release/tasklens --tab performance` in a hidden smoke session (`harness.sh`, session
  `x12tr5`), pid 552400.
- Phase A: Performance tab, CPU page, 60 s warm-up, then 10 min (04:10:05 to 04:20:07).
- Phase B: F1 (Overview card), 60 s warm-up, then 10 min (04:21:12 to 04:31:12).
- Counts come from `bpftrace` on that pid (`gpu-idle-validation.tlgpu.bt`, printed every 60 s).
  The release GUI has no runtime counter, so the counts are taken from outside:
  - `draw`: uprobe on `WgpuRenderer::draw`. `present`: uprobe on the Vulkan `Queue::present`.
  - `ioctl`: kprobe `nvidia_unlocked_ioctl`. `nvos_alloc`: kprobe `nvos_create_alloc`.
    `mmap`: kprobe `nvidia_mmap_helper`. `allocpages`: kprobe `nv_alloc_pages`.
  - `lockholds` and `max_hold_us`: time between `os_acquire_rwlock_write` returning and
    `os_release_rwlock_write` on the same lock and thread, for this pid only.
- The plan named the RM entry points `Nv04VidHeapControlWithSecInfo`, `Nv04MapMemoryWithSecInfo`
  and `Nv04MapMemoryDmaWithSecInfo`. They are in `/proc/kallsyms` but bpftrace cannot attach
  kprobes to them (not traceable), so the four probes above stand in. The probes fire: the F1
  switch at 04:20:11 gave `ioctl=5 nvos_alloc=1 mmap=1 allocpages=1` in that one minute.
- Load average at start/end: A 5.76 at start (5 min 5.97), 6.79 at end; B 3.70 at start,
  10.87 at end (other jobs on the machine, so the CPU numbers are upper bounds).

## Results

| | Phase A: Performance, CPU page | Phase B: Overview |
|---|---|---|
| (a) `ioctl`, `nvos_alloc`, `mmap`, `allocpages` per minute after warm-up | 0, 0, 0, 0 in all 10 minutes | 0, 0, 0, 0 in all 10 minutes |
| (b) `draw` per minute (min to max, mean) | 851 to 867, mean 862 (14.4/s) | 835 to 848, mean 841 (14.0/s) |
| (b) `present` per minute | equal to `draw` | equal to `draw` |
| (c) GUI CPU (utime + stime, one core = 100%) | 15.1% | 9.9% |
| (c) GPU memory (`nvidia-smi`, start and end) | 126 MiB, 126 MiB | 126 MiB, 126 MiB |
| (d) longest RM write-lock hold by the GUI | 6.9 ms | 4.1 ms |
| (d) write-lock holds per minute | about 1720 | about 1680 |
| (d) holds over 10 ms | 0 | 0 |

The `draw` rate is not zero because the CPU chart and the Overview charts slide continuously
(about 14 frames per second). The x12tr.4 change stops draws for windows with nothing changing,
which was not the case on these two pages.

## Acceptance

- (a) Vulkan and RM allocations after warm-up: 0 in both phases. Met.
- All four numbers recorded: yes (above).
- No tasklens write hold over 10 ms: none in either phase by this bead's own trace. The longest
  was 6.9 ms (6 of the 20 minutes had a longest hold of 3 to 7 ms). The separate 30 minute trace
  by manager-office (`/var/tmp/claude/rmlock`) is recorded in the bead comment when it reports.
