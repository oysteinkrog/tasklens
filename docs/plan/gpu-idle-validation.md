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

## Coverage of the allocation counts

The four probes in the main run (`nvidia_unlocked_ioctl`, `nvos_create_alloc`,
`nvidia_mmap_helper`, `nv_alloc_pages`) all read 0, but the GUI took about 1540 RM write locks
per minute and 10 600 to 11 300 `ioctl` syscalls per minute (`sc_ioctl`, all fds). So the probes
were not on the path that carries those calls. A second trace (`gpu-idle-validation.rmentry.bt`,
7 minutes, 05:26 to 05:33, with an F1 press at 05:30) printed the kernel stack of every
`os_acquire_rwlock_write` by the GUI each minute. The outermost of 6 frames is the RM API entry:

- `rmapiControlWithSecInfo`: 1566, 1498, 1512, 1502, 1528, 1496, 1492 per minute (every lock).
- `rmapiAllocWithSecInfo`, `rmapiMapWithSecInfo`, `rmapiMapToCpuWithSecInfoV2`: 0 in all 7 minutes.

`rmapiAllocWithSecInfo` is the RM allocation entry that the `Nv04Alloc` and
`Nv04VidHeapControl` wrappers call, so no allocation path took a lock. In the 31 minute main run
the same stack map (not split by minute) had 4 `rmapiAlloc`, 2 `rmapiMap`, 2 `rmapiMapToCpu`
and 1 `rm_ioctl` stacks against 48 876 `rmapiControl` ones. The first minute line shows
`nvos_alloc=1 mmap=1`, so these 9 are most likely the start-up and the tab switch, but the map
has no time axis, so this is inferred, not shown. The lock-taking calls in the steady state are RM
control calls (state queries and queue work such as fences and semaphores), not allocations.
`rmapiAlloc*`, `rmapiMap*` and `rmapiControl*` cannot take kprobes (bpftrace rejects them), which
is why the stack method is used. Why `nvidia_unlocked_ioctl` stays at 0 while the ioctl syscalls
run is not explained: the RM ioctl reaches `rmapiControl` by another entry point.

## Extended run and KWin trace (31 minutes, own trace)

manager-office never answered the request for its 30 minute trace (asked at 04:31, no reply by
05:25), so this lane ran the same kind of trace itself. It is not manager-office's
`kwinwake.bt` output: it is a copy of that script's logic, inside `gpu-idle-validation.tlgpu.bt`.
Run: new GUI on the same release binary, Performance tab, 04:53:50 to 05:25:30 (31 minutes
of one-minute lines), nested KWin, load average 4.9 to 22.

- `ioctl`, `nvos_alloc`, `mmap`, `allocpages`: 0 in all 30 minutes after the first.
- `draw` = `present`: 755 to 805 per minute (mean 771).
- Longest RM write-lock hold by the GUI: 4.88 ms. No hold over 10 ms (the script prints a stack
  for any, none printed). About 1540 holds per minute.
- KWin main thread (pid 3327) off-CPU and then woken: the GUI was the waker 13 times in 31
  minutes. None of those stalls lasted more than 10 ms (so none over 100 ms). KWin had 115 to
  191 stalls over 100 ms per minute (4351 in total) with other wakers, none of them tasklens.
- GPU memory at the end: 126 MiB.

An earlier 41 minute run (Overview, after the two phases above) also showed 0 for all four counts
and a longest hold of 7.4 ms.

## Acceptance

- (a) Vulkan and RM allocations after warm-up: 0 in both phases and in the 31 minute run, and the
  RM entry stacks show no `rmapiAlloc` or `rmapiMap` lock in 7 minutes. Met.
- All four numbers recorded: yes (above).
- No tasklens write hold over 10 ms: none in the two phases, the 41 minute run or the 31 minute
  run (longest 7.4 ms). Met.
- No KWin stall with tasklens as waker: none over 10 ms in 31 minutes (13 wakeups). Met by this
  lane's own trace. manager-office's separate 30 minute trace did not come back, so it is not
  recorded.
