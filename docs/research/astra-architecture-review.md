## 1. Is GPUI + gpui-component the right top pick?

**TL-R1-ASTRA-ARCH — Yes, as the first candidate. The plan is not yet ready to split into implementation beads.** Its main weaknesses are an unproven chart cost model, incomplete cancellation boundaries, and missing data/UI contracts.

This is a source review, not a hardware benchmark. I inspected the supplied trees and downloaded the exact GPUI 0.3.7 snapshot crates referenced by their lockfile; their checksums match. Mission Center’s magpie submodule is unpopulated, so observations about its process architecture come from its client and launcher, not magpie’s collector implementation. The submodule is declared in [mission-center/.gitmodules:1](/home/oystein/work/tasklens-ref/mission-center/.gitmodules:1).

**Recommendation [inferred]: use GPUI Kit’s components for the application shell and table, with an application-owned chart element. Keep egui/eframe as the predetermined fallback. Remove the speculative GPUI fork from the implementation path.**

| Decision | Recommendation and reason | Evidence |
|---|---|---|
| **Exact GPUI stack** | Pin the supplied component revision `3467e647600290343885b500bd7464057e334d18` and its **entire** `gpui-pre = 0.3.7` family. Use the Kit initialization/window entry points appropriate to that revision. “A recent Zed commit” is insufficient. | The source explicitly warns that independently advancing snapshots has broken compilation: [Cargo.toml:62](/home/oystein/work/tasklens-ref/gpui-component/Cargo.toml:62). |
| **Chart throughput** | GPUI is plausible for this workload, but “well under 1 ms” is unsupported. Benchmark realistic history lengths, kernel overlays, labels, clipping and the table together. [inferred] | The estimate is in [PLAN.md:66](/home/oystein/work/tasklens/docs/PLAN.md:66). Component path caching still clones and translates every vertex: [path_cache.rs:44](/home/oystein/work/tasklens-ref/gpui-component/crates/base/src/plot/path_cache.rs:44). |
| **Text** | Keep GPUI’s text system. Cache formatted visible-cell strings and stable labels; use tabular numerals and fixed numeric column widths. Test font fallback, non-ASCII process names, ellipsis and IME at 110%. [inferred] | This Linux snapshot uses CosmicText: [text_system.rs:1](/tmp/tl-r1-astra-arch-deps/gpui-pre-linux-0.3.7/src/linux/text_system.rs:1), including advanced shaping: [cosmic_text_system.rs:741](/tmp/tl-r1-astra-arch-deps/gpui-pre-wgpu-0.3.7/src/cosmic_text_system.rs:741). |
| **Table virtualization** | Use its virtualized table, with application-owned identity, sorting and selection. Two thousand records are modest; rendering all 2,000 rows or repeatedly formatting them is the avoidable expense. [inferred] | Rows use `uniform_list`: [state.rs:2495](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/state.rs:2495); columns also virtualize: [state.rs:2139](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/state.rs:2139). |
| **Fractional scaling** | Native support exists. Treat correctness on this desktop as a release gate, including popups, text, pointer coordinates, resizing, monitor transitions and reconnects. [inferred] | Preferred fractional scale is divided by 120: [Wayland window.rs:1184](/tmp/tl-r1-astra-arch-deps/gpui-pre-linux-0.3.7/src/linux/wayland/window.rs:1184). Drawable resizing and viewport destination are updated together: [window.rs:1471](/tmp/tl-r1-astra-arch-deps/gpui-pre-linux-0.3.7/src/linux/wayland/window.rs:1471). |
| **CSD title bar** | Reuse the component behavior and apply Tasklens styling. Test drag, double-click maximize, restore, resize borders, right-click window menu and compositor decoration negotiation. [inferred] | Linux control actions exist at [title_bar.rs:223](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/title_bar.rs:223); decoration negotiation is accounted for at [title_bar.rs:258](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/title_bar.rs:258); dragging at [title_bar.rs:365](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/title_bar.rs:365). |
| **Idle power** | GPUI can stop rendering, but continuously scrolling visible graphs are continuously active. Separate “visible animation,” “visible static” and “hidden” budgets. Stop animation explicitly when hidden or motion-disabled. [inferred] | `request_animation_frame` explicitly redraws unchanged content: [window.rs:2610](/tmp/tl-r1-astra-arch-deps/gpui-pre-0.3.7/src/window.rs:2610). Wayland supports suspended visibility and a parked frame loop: [window.rs:121](/tmp/tl-r1-astra-arch-deps/gpui-pre-linux-0.3.7/src/linux/wayland/window.rs:121), [window.rs:990](/tmp/tl-r1-astra-arch-deps/gpui-pre-linux-0.3.7/src/linux/wayland/window.rs:990). |

**Do not treat the system-monitor example as performance validation.** It truncates to 200 processes and runs synchronous collection inside a UI entity update. Borrow its component usage, not its engine architecture. Evidence: [main.rs:161](/home/oystein/work/tasklens-ref/gpui-component/examples/system_monitor/src/main.rs:161), [main.rs:300](/home/oystein/work/tasklens-ref/gpui-component/examples/system_monitor/src/main.rs:300), [main.rs:319](/home/oystein/work/tasklens-ref/gpui-component/examples/system_monitor/src/main.rs:319).

The alternative-toolkit assessments below are **[inferred]**: their implementations were not supplied or benchmarked here.

| Alternative | Concrete condition that would make me choose it |
|---|---|
| **egui/eframe** | GPUI fails the chart frame budget or Wayland acceptance gate, and an equivalent eframe prototype passes. Its wgpu callback path is attractive when persistent chart GPU buffers matter more than retained widget behavior. Virtualize with visible-row rendering; benchmark text/layout rebuilding during animation. Fractional DPI and custom chrome still require the same desktop tests. This is my fallback. |
| **Makepad** | A working prototype demonstrates materially lower CPU cost through shader-driven charts while also delivering the required table, text input, menus and Linux window behavior. I would not select it merely because custom shaders are convenient; this application has considerable desktop interaction complexity. |
| **iced** | The dominant problem becomes predictable application state and conventional desktop controls, and a custom chart primitive plus an explicitly virtualized table meets the budget. Choose its executor/features deliberately to preserve the no-Tokio requirement. Canvas caching alone does not establish cheap translated rendering. |
| **Slint** | A supported Slint configuration demonstrates better idle behavior and Linux integration, and its model-driven views cover the process table without substantial custom widget work. Confirm text, fractional scaling, CSD and custom-rendering integration in the selected renderer, plus licensing appropriate to distribution. |
| **Custom wgpu application** | Existing toolkits demonstrably cannot meet the measured requirements, or the product becomes predominantly a visualization surface. For this task manager, taking ownership of text shaping, IME, accessibility, focus, menus, table interaction and Wayland integration is disproportionate. A custom **chart pass inside a toolkit** is much more defensible. |

**Fallback rule [inferred]:** first test GPUI with the chart design in section 4. If it fails after a focused optimization pass, run the same fixture in eframe with a custom chart pass. Do not let an implementation agent choose between upstream GPUI, gpui-ce, a private fork and egui. The current plan leaves exactly that branching decision open at [PLAN.md:28](/home/oystein/work/tasklens/docs/PLAN.md:28).

## 2. Is the ArcSwap + async-channel bridge sound?

**Yes, with a bounded notification channel, immutable snapshots and one publisher. As written, its protocol is incomplete.**

**Recommended contract [inferred]:**

```rust
// Shared within one process:
latest: ArcSwap<Snapshot>
wake: async_channel::bounded::<()>(1)

// Single publisher; no await/checkpoint between these operations:
latest.store(next_snapshot);
match wake_tx.try_send(()) {
    Ok(()) | Err(TrySendError::Full(())) => {}
    Err(TrySendError::Closed(())) => stop_publication(),
}

// One GPUI-owned receiver task:
while wake_rx.recv().await.is_ok() {
    let snapshot = latest.load_full();
    // Ignore duplicate sequence numbers.
    // Install once; notify entities whose component versions changed.
}
```

- **Backpressure — never await a snapshot wake send.** `Full` means an unread wake already exists, so the newer snapshot can replace the old one. An unbounded channel accumulates obsolete notifications; an awaited bounded send contradicts “collection never waits for a frame.” Evidence: [PLAN.md:36](/home/oystein/work/tasklens/docs/PLAN.md:36); `async-channel` distinguishes `try_send` from waiting `send` at [lib.rs:230](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-channel-2.5.0/src/lib.rs:230).

- **Lost wakes — keep the protocol simple.** Store before signaling. Receive before loading. Do not load and then drain notifications: that can consume the notification corresponding to a newer snapshot without loading it. No extra “notification pending” atomic is needed. The bounded token may cause a harmless duplicate read. [inferred]

- **Ownership — collectors must not independently overwrite the slot.** A single engine reducer owns current state and publishes snapshots. Independent read-modify-store publishers can overwrite another collector’s update despite each pointer swap being atomic. This ownership is missing from [PLAN.md:51](/home/oystein/work/tasklens/docs/PLAN.md:51). [inferred]

- **Tearing — load one outer `Arc` for a UI update/frame.** Do not load totals, rows and histories independently. Put source timestamps, source versions and status alongside each component. An immutable snapshot prevents memory-level tearing; it does **not** make a 1 Hz process sweep simultaneous with 10 Hz CPU samples. [inferred] The different rates are prescribed at [PLAN.md:58](/home/oystein/work/tasklens/docs/PLAN.md:58).

- **Change detection — compare component versions against the last consumed snapshot.** A dirty-bit mask describing only the most recent publication is insufficient after coalescing: it can conceal an earlier process-table change. [inferred]

- **Delivery semantics — latest state may be dropped; action results may not.** Use a separate bounded request/result path for actions, with request IDs and explicit completion states. Never put a kill result or error toast solely into the replaceable snapshot slot. The plan currently specifies request/response actions without this separation: [PLAN.md:62](/home/oystein/work/tasklens/docs/PLAN.md:62). [inferred]

**Snapshot representation [inferred]:**

```rust
struct Snapshot {
    sequence: u64,
    published_mono_ns: u64,
    system: Arc<SystemSnapshot>,
    processes: Arc<ProcessSnapshot>,
    histories: Arc<HistorySnapshot>,
    devices: Arc<DeviceCatalog>,
    health: Arc<CollectorHealth>,
}
```

Every component carries its own version and observation interval. A 10 Hz totals publication reuses the existing process `Arc`; it does not clone 2,000 records.

**Size decision [inferred]:** budget approximately 256 bytes of changing scalar data per process: 2,000 processes then require about **512 KB per sweep**, excluding metadata and allocation overhead. Keep names, executable paths and other mostly stable metadata behind shared references. Even a 1–2 MB rebuilt process snapshot at 1 Hz is modest bandwidth; the more consequential risks are allocation churn, formatting and sorting on the UI thread. These are estimates, not measured record sizes.

**History decision [inferred]:** the engine owns history and publishes bounded history views. Otherwise dropped snapshots also drop samples. Keep process records out of the 10 Hz copying path. Bound retained snapshots—including a paused UI snapshot—so an accidental archive of `Arc`s cannot defeat the memory target. Evidence: [PLAN.md:67](/home/oystein/work/tasklens/docs/PLAN.md:67).

**Better options?** A latest-value/watch channel can combine the slot and notification correctly, but only use one with understood executor and cancellation semantics. For this seam, ArcSwap plus a one-token channel is straightforward and sufficient. Avoid a hand-written atomic-waker protocol. Keep `async-channel` at the GPUI boundary; use asupersync-native cancellation-aware primitives inside the engine. [inferred]

## 3. Data engine: process boundary, supervision and scheduling

**Recommendation [inferred]: keep the main engine in-process, on its own asupersync runtime; isolate NVML in one unprivileged child process. Do not introduce a full magpie-style sampler process for v1.**

The reason is specific to this application:

- Ordinary procfs parsing benefits from direct immutable snapshot sharing.
- A separate full sampler adds serialization, protocol/version handling and restart resynchronization, without inherently improving UI responsiveness.
- The strongest fault-isolation argument is NVML’s native driver boundary. Isolate that boundary directly.
- A full sampler process would still be a reasonable choice for a future Flatpak host bridge or independently persistent recorder. Those are not requirements of the native pacman deployment in [PLAN.md:159](/home/oystein/work/tasklens/docs/PLAN.md:159). [inferred]

Mission Center genuinely launches and restarts a separate process: [client.rs:525](/home/oystein/work/tasklens-ref/mission-center/src/magpie_client/client.rs:525). Its launcher also addresses Flatpak host execution: [spawner.rs:44](/home/oystein/work/tasklens-ref/mission-center/src/magpie_client/spawner.rs:44). Those are useful precedents, but do not establish that IPC is necessary for Tasklens.

**Critical correction:** “NVML crashes, only its worker restarts” is false for segmentation faults or process aborts. Asupersync explicitly documents that blocking operations cannot be interrupted mid-execution: [blocking_pool.rs:26](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/src/runtime/blocking_pool.rs:26). The claim at [PLAN.md:56](/home/oystein/work/tasklens/docs/PLAN.md:56) must distinguish returned errors, Rust panics, hung calls and native crashes.

**Supervision layout [inferred]:**

```text
Engine AppSpec
├── state reducer / history / publisher
├── fast CPU sampler
├── memory / disk / interface samplers
├── process sweep and metadata cache
├── visible-process enrichment
├── connection inventory and socket attribution
├── D-Bus adapter: user + system managers
├── GPU child-process supervisor
└── action dispatcher
    └── one owned region per accepted request
```

- Use **one-for-one** restart for independent collectors.
- A reducer invariant failure restarts the engine session, with a new epoch and explicit history gaps.
- Expected permission errors, missing devices and unsupported NVML functions produce health/status updates, not restart loops.
- Back off exceptional restarts: 0.5, 1, 2, 4, 8, 16, then 30 seconds; after five failures in a minute, disable that source for a minute.
- Start with `worker_threads(1)` and a blocking pool bounded to two threads. Thirty-two logical CPUs are not a reason to create 32 sampling workers.
- Never schedule another instance of a sampler while its previous blocking job is still running. A timeout marks it late; it does not magically free the occupied thread.

These are the missing operational details behind [PLAN.md:55](/home/oystein/work/tasklens/docs/PLAN.md:55).

**GPU child contract [inferred]:** add `tl-gpu-probe`, the only process loading NVML. Use an inherited Unix socketpair, a versioned length-prefixed protocol, a 1 MiB message limit and one outstanding request. A 500 ms expired request marks GPU data stale and initiates child termination/reaping before restart. Keep the privileged action helper separate. A driver stuck in uninterruptible kernel sleep remains a limitation; process isolation contains the UI failure but cannot repair the kernel.

**Sampling schedule [inferred]:**

| Source | Visible/default policy | Hidden policy |
|---|---|---|
| CPU totals/per-core | 10 Hz while CPU charts are visible; otherwise 1 Hz when needed | Off without recording; 1 Hz with recording |
| Memory, disk, network totals | 1 Hz | Off without recording; 1 Hz with recording |
| GPU totals | 1 Hz; do not poll NVML at 10 Hz by default | Off without recording; 1 Hz with recording |
| Process CPU/RSS/I/O | User setting, default 1 Hz; only while a process consumer exists | Off without process recording |
| PSS/USS, process GPU details | At most 1 Hz for demanded identities/columns | Off |
| Connections and attribution | Every 2 seconds while consumed | Off |
| Clock/temperature sensors | 1 Hz while displayed | Off |
| Static inventory, icons, services | Initial load plus relevant change events; refresh on demand | No periodic full inventory scan |

The plan unnecessarily couples all system sources to 10 Hz at [PLAN.md:58](/home/oystein/work/tasklens/docs/PLAN.md:58). Smooth graph motion does not require equally fast sensor polling.

**Tick semantics [inferred]:** schedule against absolute monotonic deadlines, `t0 + n × period`; skip missed ticks and never catch up with a burst. Compute rates from actual elapsed sample time. Reset counter baselines after restart, device replacement and suspend/resume. Record collection start/end times rather than pretending a process sweep is instantaneous.

**Visibility semantics [inferred]:** aggregate demand across all windows, detached mini-graphs, selected columns and recording consumers. Demand refers to stable process IDs, not row numbers. Coalesce viewport changes and include one viewport of overscan. A minimized main window does not mean a detached graph is invisible.

**History policy [inferred]:** background recording is **off by default in v1**. Retain a rolling live history while demanded; show gaps after suspension. If six-hour recording is required, design it as a separate storage feature. “Only history recording keeps running” is not a sufficiently bounded hidden-mode policy: [PLAN.md:61](/home/oystein/work/tasklens/docs/PLAN.md:61), [inventory:82](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:82).

**Budget contradiction:** the plan’s 15 ms sweep for 1,000 processes suggests roughly 30 ms for 2,000 if cost scales linearly—already **3% of one core**, before other collectors. That scaling is **[inferred]**, but it directly challenges the 2% target. Preserve the 1 Hz requirement and measure/optimize; do not silently lower sampling frequency to pass. Evidence: [PLAN.md:88](/home/oystein/work/tasklens/docs/PLAN.md:88), [PLAN.md:111](/home/oystein/work/tasklens/docs/PLAN.md:111).

## 4. Chart rendering and exact data structures

**Recommendation [inferred]: use quads for backgrounds and gridlines, cached linear filled paths and strokes for data, and one animation driver per window. Do not use one rectangle per sample as the default area renderer.**

One quad per sample produces a staircase when samples are wider than a physical pixel. It does not reproduce an area under a sloped line. A per-pixel-column implementation changes both the primitive count and the cost model. The reference calls for filled scrolling graphs: [inventory:183](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:183).

**History structures [inferred]:**

```rust
#[repr(C)]
struct Sample {
    mono_ns: u64,
    value: f32,
    flags: u32,       // valid, gap, reset; never encode missing as zero
}

struct SeriesRing {
    samples: Box<[Sample]>, // capacity 602
    next: u16,
    len: u16,
    version: u64,
}

struct SeriesSnapshot {
    id: SeriesId,
    version: u64,
    period_ns: u64,
    samples: Arc<[Sample]>, // chronological; at most 602
}

struct ProcessKey {
    pid: u32,
    start_ticks: u64,
}
// Engine-session metadata also includes boot_id.

struct ChartGeometryKey {
    series_version: u64,
    physical_width: u32,
    physical_height: u32,
    scale_bits: u32,
    y_min_bits: u32,
    y_max_bits: u32,
    style_version: u64,
}

struct ChartCache {
    key: ChartGeometryKey,
    anchor_mono_ns: u64,
    fill: Option<gpui::Path<gpui::Pixels>>,
    stroke: Option<gpui::Path<gpui::Pixels>>,
}
```

Use **60 seconds as the default range**, with 15/60-second choices in v1. Capacity 602 accommodates a 60-second 10 Hz window plus boundary samples. Store raw counters as `u64`; convert only graph values to `f32`.

Forty such 16-byte rings occupy approximately **385 KB**. The plan’s 120-sample estimate represents only 12 seconds at 10 Hz. Both calculations are **[inferred from the proposed representations]**; the original estimate appears at [PLAN.md:71](/home/oystein/work/tasklens/docs/PLAN.md:71).

**Rendering rules [inferred]:**

1. Build geometry only when samples, bounds, scale, y-domain or style change.
2. Use straight segments; avoid splines that invent overshoot in percentages and rates.
3. Separate contiguous valid runs at gaps.
4. If samples exceed the useful horizontal resolution, retain each physical-pixel bucket’s minimum and maximum in time order, plus boundary points. Do not average away spikes.
5. Cache geometry in chart-local coordinates with a stable time anchor.
6. On each animation frame, update the time translation and clip to the chart rectangle.
7. Rebuild text only when its displayed value changes. Do not rerender the entire table merely because its floating graph panel moved.

**Smooth time mapping [inferred]:**

```text
right_edge_time = min(monotonic_now - source_period, newest_sample_time)
x(sample)       = chart_right
                  - (right_edge_time - sample.time) × width / time_range
```

This deliberately renders one sampling interval behind the latest observations, allowing interpolation between known samples rather than forecasting future values. Late data stops advancement at the latest available point; prolonged absence becomes a visible gap/stale state. Keep numeric readouts at the latest measured value.

Use GPUI’s compositor-paced animation callback, never a 144 Hz backend timer. Register only one next-frame request per window while at least one visible chart needs motion. At 144 Hz the frame interval is about 6.94 ms; sampling and frame scheduling remain independent.

**Important limitation:** cached GPUI paths are not persistent GPU geometry.

- The component cache clones and translates vertices: [path_cache.rs:46](/home/oystein/work/tasklens-ref/gpui-component/crates/base/src/plot/path_cache.rs:46).
- `paint_path` scales the path into the next scene: [window.rs:4573](/tmp/tl-r1-astra-arch-deps/gpui-pre-0.3.7/src/window.rs:4573).
- Scaling creates another vertex vector: [scene.rs:821](/tmp/tl-r1-astra-arch-deps/gpui-pre-0.3.7/src/scene.rs:821).
- The wgpu renderer expands/uploads path vertices and renders an intermediate pass: [wgpu_renderer.rs:1836](/tmp/tl-r1-astra-arch-deps/gpui-pre-wgpu-0.3.7/src/wgpu_renderer.rs:1836).

Therefore **“only an x offset per frame” is mathematically true but operationally misleading**. Likewise, batching depends on draw order and primitive type; it does not guarantee the whole chart surface becomes one instanced draw. Evidence: [scene.rs:291](/tmp/tl-r1-astra-arch-deps/gpui-pre-0.3.7/src/scene.rs:291).

Mission Center offers a useful comparison: it caches a render node and translates it between updates, [graph_widget.rs:240](/home/oystein/work/tasklens-ref/mission-center/src/performance_page/widgets/graph_widget.rs:240). However, its animation driver uses a 50 ms timer, so that implementation is not evidence of 144 Hz behavior: [window.rs:1128](/home/oystein/work/tasklens-ref/mission-center/src/window.rs:1128).

**Custom-wgpu contingency [inferred]:** if GPUI fails the frame budget, the eframe chart renderer should use:

- A persistent storage buffer containing packed series rings.
- A descriptor buffer containing chart rectangles, series offsets, ranges and colors.
- Per-frame uniforms containing time and viewport parameters.
- Dirty-range uploads only when samples or descriptors change.
- Instanced segment geometry for fills and anti-aliased strokes.
- One chart render pass per window, with a small bounded number of draws and clipping through chart descriptors.
- Toolkit text and interaction above the chart pass.

Do not implement CPU readback/re-upload between independent GPU devices. A texture-import feature does not by itself prove a usable, synchronized custom-pass API. The plan’s gpui-ce assertion at [PLAN.md:28](/home/oystein/work/tasklens/docs/PLAN.md:28) is not substantiated by the supplied source.

## 5. Decisions PLAN.md leaves open or under-specified

Sections 1–4 resolve stack selection, process boundaries, snapshot publication, supervision, rates and chart representation. The following additional decisions should be written into the plan **before** creating production implementation beads. All recommendations in this section are **[inferred]**.

| Decision | Recommended decision and reason | Evidence of the gap |
|---|---|---|
| **Project identity/publication** | Keep `tasklens`; keep the repository private initially. Public publication remains an owner-controlled action, not an agent choice. | [PLAN.md:176](/home/oystein/work/tasklens/docs/PLAN.md:176) |
| **v1 scope** | Include phases 1–4, with scale, motion, refresh settings and persistence introduced alongside their features. Explicitly call this a scoped replacement, not complete TaskSlinger parity. | [PLAN.md:128](/home/oystein/work/tasklens/docs/PLAN.md:128), [PLAN.md:180](/home/oystein/work/tasklens/docs/PLAN.md:180) |
| **Toolchain/runtime features** | Pin Rust 1.98.1 as the initial toolchain and `asupersync = "=0.5.0"`, disable default features, enable only the required native runtime/macros and test features. Validate the combination in phase 0. Do not accidentally select nightly-only defaults. | Asupersync defaults enable `nightly-outcome-try`: [Cargo.toml:215](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/Cargo.toml:215); it activates unstable language features at [lib.rs:52](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/src/lib.rs:52). |
| **Meaning of “no Tokio”** | Enforce absence from the resolved Linux production dependency graph, including helpers. Exclude unused HTTP/webview/shell features. A Tokio entry in a workspace lockfile alone neither proves nor disproves production use. GPUI’s own executor remains permitted. | [PLAN.md:23](/home/oystein/work/tasklens/docs/PLAN.md:23); component workspace has optional-purpose HTTP dependencies at [Cargo.toml:73](/home/oystein/work/tasklens-ref/gpui-component/Cargo.toml:73). |
| **D-Bus executor ownership** | Put zbus behind one engine adapter. Use `default-features = false`, `async-io`; disable its internal executor thread and drive its executor from an owned asupersync task. Explicitly acknowledge async-io’s transport reactor. Close connections before draining the driver task. | zbus enables an internal executor by default: [builder.rs:387](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/zbus-5.19.0/src/connection/builder.rs:387); async-io brings additional execution machinery: [Cargo.toml:62](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/zbus-5.19.0/Cargo.toml:62). |
| **Crate boundaries** | Add `tl-gpu-probe`; move NVML loading there. Keep synchronous parsers in `tl-collect`, orchestration/actions in `tl-engine`, immutable contracts in `tl-model`, presentation in `tl-ui`. `tl-helper` remains a distinct later privilege boundary. | [PLAN.md:45](/home/oystein/work/tasklens/docs/PLAN.md:45) |
| **Data validity** | Every metric distinguishes valid, warming up, stale, restricted, unsupported, missing device and failed collection. Attach age and provenance; never substitute zero. | Only restricted access is currently specified: [PLAN.md:99](/home/oystein/work/tasklens/docs/PLAN.md:99). |
| **Counter/time semantics** | Use monotonic timestamps, actual deltas, reset detection and `u64` cumulative counters. Never subtract counters across process/device generations. Handle CPU hotplug and suspend explicitly. | Sources are listed without these rules: [PLAN.md:77](/home/oystein/work/tasklens/docs/PLAN.md:77). |
| **CPU percentages** | Default process CPU is percentage of whole-machine capacity; offer a clearly labelled “one core = 100%” display option. Define kernel usage as system+IRQ+softIRQ; keep iowait separate and avoid guest-time double counting. | CPU presentation and kernel toggle: [PLAN.md:130](/home/oystein/work/tasklens/docs/PLAN.md:130). |
| **Memory semantics** | Default process memory is RSS, labelled accordingly; optional PSS/USS are separate. System “used” derives from available memory. Do not sum RSS and describe it as unique application memory; commit may exceed 100%. | [PLAN.md:82](/home/oystein/work/tasklens/docs/PLAN.md:82), [PLAN.md:88](/home/oystein/work/tasklens/docs/PLAN.md:88). |
| **File-descriptor caching** | Keep a small set of global proc/sys files open where verified. Open process directories per sweep and close promptly; cap any process FD cache at 128 entries. Validate identity and reopen after errors. Do not retain every PID’s files indefinitely. | Blanket persistent-FD proposal: [PLAN.md:50](/home/oystein/work/tasklens/docs/PLAN.md:50). |
| **Process metadata** | Cache by process identity, but refresh mutable names/command lines and detect executable changes. Bound command-line reads; fetch large properties on demand. An `exec` does not necessarily change PID/start time. | [PLAN.md:63](/home/oystein/work/tasklens/docs/PLAN.md:63), [inventory:55](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:55). |
| **Application grouping** | Resolve the current UID dynamically. Use cgroup v2 scope identity plus desktop metadata; provide an “ungrouped” fallback. Maintain app grouping separately from the PPID tree. Never hard-code `user@1000.service`. | [PLAN.md:92](/home/oystein/work/tasklens/docs/PLAN.md:92). |
| **Tree ordering/aggregation** | Flatten expanded tree nodes into a virtual row list. Sort siblings, preserve parents, and prevent double counting descendants in group totals. Handle missing parents and reparenting. | [PLAN.md:136](/home/oystein/work/tasklens/docs/PLAN.md:136). |
| **Default columns** | Use Name, User, PID, CPU, RSS, disk read/write rate, TCP-attributed network rate, supported GPU metrics and Description. Omit per-process “Power usage” by default unless an actual measurement source exists. | Inventory requires power but the plan supplies no source: [inventory:51](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:51), [PLAN.md:77](/home/oystein/work/tasklens/docs/PLAN.md:77). |
| **Sort/filter execution** | Engine-owned, generation-tagged table projections; discard results for obsolete queries. Stable tie-break by process identity. Search cached normalized names and command lines; debounce typing by 100 ms. | Delegate leaves sorting to the application: [delegate.rs:28](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/delegate.rs:28). |
| **Sorting expensive metrics** | Disable global sorting by PSS/USS while only visible rows are sampled. Do not present a partial sample as a global ranking. A future full-column scan must be an explicit separate mode. | Visible-only PSS: [PLAN.md:89](/home/oystein/work/tasklens/docs/PLAN.md:89). |
| **Selection/context menus** | Store selections and captured action targets as process identities. Remap row indices after reorder; support application-owned multi-selection. Keep an open menu attached to the captured identity. | Component state stores row indices: [state.rs:259](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/state.rs:259); multi-selection is in [inventory:145](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:145). |
| **Visible-row demand** | Recompute identity demand after sorting/filtering even if the visible index range is unchanged. Handle a viewport containing one row. Do not rely solely on delegate range-change callbacks. | Callback suppresses unchanged ranges and ranges of length ≤1: [state.rs:1333](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/state.rs:1333). |
| **Ctrl pause** | Freeze displayed rows, ordering and values; keep collection/history running. Release on Ctrl-up or focus loss. Actions retain captured identities and revalidate them. | [PLAN.md:137](/home/oystein/work/tasklens/docs/PLAN.md:137), [inventory:37](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:37). |
| **Process action semantics** | “End” sends SIGTERM; “Force end” sends SIGKILL. Open a pidfd and verify identity before signaling. Tree operations capture targets and report per-target outcomes; cgroup actions require validated scope ownership. Never retry destructive requests automatically. | [PLAN.md:62](/home/oystein/work/tasklens/docs/PLAN.md:62), [PLAN.md:138](/home/oystein/work/tasklens/docs/PLAN.md:138). |
| **Priority/affinity safety** | Define these separately from signals: pidfds do not automatically make PID-based scheduling syscalls race-free. Revalidate immediately and report best-effort semantics; do not extend the “never hits a reused PID” guarantee to unsupported operations. | The guarantee covers signals at [PLAN.md:63](/home/oystein/work/tasklens/docs/PLAN.md:63), while scheduling actions appear at [PLAN.md:62](/home/oystein/work/tasklens/docs/PLAN.md:62). |
| **Action deadlines** | Use bounded admission, request IDs and states `Rejected/Started/Succeeded/Failed/OutcomeUnknown`. A timeout after an external effect is not proof of failure or rollback. Preserve results separately from snapshots. | [PLAN.md:62](/home/oystein/work/tasklens/docs/PLAN.md:62). |
| **Services** | Distinguish system/user managers in identity. Subscribe to changes, resynchronize after reconnect, and track systemd job completion rather than treating method acceptance as success. Keep polkit interaction asynchronous. | [PLAN.md:93](/home/oystein/work/tasklens/docs/PLAN.md:93). |
| **Startup applications** | Apply XDG precedence and desktop visibility rules; write user overrides atomically. `Hidden=true` applies to desktop entries, not systemd units. Manage unit enablement separately. | [PLAN.md:94](/home/oystein/work/tasklens/docs/PLAN.md:94). |
| **Network attribution** | Label it “attributed TCP traffic,” scoped to observable sockets/namespaces. Preserve ambiguity for sockets shared by processes. Do not infer exact process totals or UDP bytes from socket inventory. | [PLAN.md:91](/home/oystein/work/tasklens/docs/PLAN.md:91), [PLAN.md:95](/home/oystein/work/tasklens/docs/PLAN.md:95). |
| **GPU capability model** | Probe each metric independently by device UUID. Unsupported process utilization is unavailable, not zero. Never divide total GPU utilization among processes as a substitute. | [PLAN.md:83](/home/oystein/work/tasklens/docs/PLAN.md:83). |
| **Device identity/topology** | CPU logical ID; GPU UUID; block device major/minor plus lifecycle generation; network namespace+ifindex plus generation. Model partitions/device-mapper relationships to avoid aggregate double counting. Make virtual-interface hiding configurable. | [PLAN.md:79](/home/oystein/work/tasklens/docs/PLAN.md:79), [PLAN.md:86](/home/oystein/work/tasklens/docs/PLAN.md:86). |
| **Units and graph ranges** | IEC memory units, selectable network bits/bytes, explicit per-second rates. Percent graphs use fixed bounds; bandwidth scales rise immediately and fall only after 10 seconds below half-scale. Preserve unsupported gaps. | [inventory:134](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:134), [PLAN.md:66](/home/oystein/work/tasklens/docs/PLAN.md:66). |
| **DPI/theme/font** | Layout in logical pixels; compositor scale and user scale multiply exactly once. Use Noto Sans with tabular numeric styling, bundled/licensed fallback assets, 28 logical-pixel rows by default. Rebuild geometry/text caches on scale changes. | [PLAN.md:145](/home/oystein/work/tasklens/docs/PLAN.md:145), [PLAN.md:214](/home/oystein/work/tasklens/docs/PLAN.md:214). |
| **Settings persistence** | Versioned TOML under `$XDG_CONFIG_HOME/tasklens/`; atomic replacement, migration and corrupt-file fallback. Persist column IDs, not display indices, plus sort, theme, motion, units and window size. | [PLAN.md:146](/home/oystein/work/tasklens/docs/PLAN.md:146), [inventory:138](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:138). |
| **Window lifecycle** | Single application instance; subsequent launches activate it. Closing the last window exits in v1; tray persistence is a later explicit mode. Cancel the root region and drain without joining threads on the UI thread. | Tray support is deferred at [PLAN.md:151](/home/oystein/work/tasklens/docs/PLAN.md:151); lifecycle is unspecified. |
| **Accessibility/input** | Require keyboard navigation, focus indication, accessible labels, usable sorting/selection and reduced-motion support. Charts expose their latest values and units as text. Do not capture typing intended for another input/modal. | “Typing starts filtering” and motion settings: [inventory:33](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:33), [inventory:135](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:135). |
| **Unsupported feature parity** | Explicitly defer KWin window picking, show-window, always-on-top where unsupported, TCP destruction, Windows token/modules semantics, six-hour history, tray graphs and privileged inspection. Every inventory item needs implemented/adapted/deferred/unsupported status. | [inventory:61](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:61), [inventory:70](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:70), [inventory:116](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:116). |
| **Phase-0 acceptance** | Test 60-second histories, 32 core charts with kernel overlays, sidebar/mini-graphs, realistic text and 2,000 changing rows. Require p99 UI CPU <2 ms and zero application-attributable missed presentations in repeated 30-second runs after warm-up. Record GPU and presentation timing separately. | Existing fixture/targets: [PLAN.md:108](/home/oystein/work/tasklens/docs/PLAN.md:108), [PLAN.md:122](/home/oystein/work/tasklens/docs/PLAN.md:122). |
| **Startup/memory/power measurement** | Measure cold and warm first presentation separately, without waiting for collectors. Report UI+probe CPU and RSS, plus PSS and GPU memory separately. Test hidden, static-visible and animated-visible states against a desktop baseline. | [PLAN.md:108](/home/oystein/work/tasklens/docs/PLAN.md:108). |
| **Test boundaries** | LabRuntime verifies engine scheduling and state transitions. Fixture tests verify parsing. Real Linux integration tests verify procfs permissions, pidfds, D-Bus, NVML child failure, shutdown and Wayland rendering. Headless tests cannot certify compositor behavior. | [PLAN.md:64](/home/oystein/work/tasklens/docs/PLAN.md:64), [PLAN.md:161](/home/oystein/work/tasklens/docs/PLAN.md:161). |

The architectural choices above can be frozen now. **Hardware feasibility cannot be decided from source alone.** Phase 0 should produce measured pass/fail results against predetermined gates; production beads follow the resulting branch.

## 6. Risks the plan misses

1. **A very low CPU figure can still mean substantial desktop power consumption.**
   **Recommendation [inferred]:** measure GPU clocks/power and compositor work against the same three-monitor baseline, and provide reduced-motion/sample-only rendering. Low UI-thread time does not establish low GPU power. The plan specifies CPU/RSS but no GPU-power measurement: [PLAN.md:106](/home/oystein/work/tasklens/docs/PLAN.md:106).

2. **Persistent blocking work can exhaust the collector pool despite structured concurrency.**
   **Recommendation [inferred]:** bound outstanding jobs, prohibit overlapping retries, isolate NVML, and expose “collector stalled” health. Never describe cancellation budgets as syscall interruption guarantees. Evidence: [blocking_pool.rs:26](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/src/runtime/blocking_pool.rs:26).

3. **A perfectly smooth table can execute an action on the wrong displayed target.**
   **Recommendation [inferred]:** identity-based selection and captured context-menu targets are mandatory before implementing kill/suspend. Reordering and the component’s row-index selection create this risk together. Evidence: [state.rs:259](/home/oystein/work/tasklens-ref/gpui-component/crates/component/src/table/state.rs:259), [PLAN.md:138](/home/oystein/work/tasklens/docs/PLAN.md:138).

4. **Visibility-driven sampling can silently corrupt ranking and aggregation.**
   **Recommendation [inferred]:** expose coverage explicitly; forbid global ranking from visible-only PSS/GPU samples and incomplete socket attribution. A fast but misleading ranking is a product correctness failure. Evidence: [PLAN.md:61](/home/oystein/work/tasklens/docs/PLAN.md:61), [PLAN.md:89](/home/oystein/work/tasklens/docs/PLAN.md:89).

5. **Six-hour process history is a different storage architecture.**
   **Recommendation [inferred]:** require a separate design with chunked/delta storage, retention limits and bounded replay. At 2,000 processes × 256 bytes × 21,600 seconds, naive 1 Hz scalar retention alone is about **11 GB**, before metadata. This conflicts with the 150 MB target. Evidence: [inventory:87](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:87), [PLAN.md:113](/home/oystein/work/tasklens/docs/PLAN.md:113).

6. **Fractional scaling support does not establish multi-monitor correctness or 144 Hz pacing.**
   **Recommendation [inferred]:** test mixed refresh rates, straddling monitors, unplug/replug, lock/unlock, suspend/resume and changed scale. One window has one surface/frame cadence, not an independent clock for each monitor it overlaps. The plan’s monitor-move test is too narrow: [PLAN.md:123](/home/oystein/work/tasklens/docs/PLAN.md:123).

7. **Sensitive and adversarial process metadata reaches rendering, logs and exports.**
   **Recommendation [inferred]:** bound reads, handle invalid UTF-8/control characters, avoid logging command lines/environment by default, and make exports deliberate. This follows directly from the command-line/environment/property scope: [inventory:55](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:55), [inventory:78](~/work/tasklens-ref/taskslinger/taskslinger-inventory.md:78).

8. **The privileged helper combines two unrelated privilege requirements.**
   **Recommendation [inferred]:** separate narrow authorized process/service actions from any future eBPF collector. Define allowed operations, caller authentication and target validation; never expose arbitrary shell execution or arbitrary file access. The plan groups privileged actions and exact network collection into one later helper: [PLAN.md:152](/home/oystein/work/tasklens/docs/PLAN.md:152).

9. **Licensing needs an explicit decision before copying code or publishing.**
   **Recommendation [inferred]:** use Mission Center as architectural evidence unless intentionally adopting its applicable licensing obligations. Also record asupersync’s actual license: the installed 0.5.0 package has an OpenAI/Anthropic rider, not plain MIT. Its implications for this development/distribution arrangement need resolution; do not silently classify it as ordinary MIT. Evidence: [Mission Center COPYING:1](/home/oystein/work/tasklens-ref/mission-center/COPYING:1), [asupersync LICENSE:12](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0/LICENSE:12).

10. **The schedule assumes away the hardest work.**
    **Recommendation [inferred]:** replace the 4–5-week commitment with milestone acceptance. Performance-table identity, cancellation boundaries, accurate attribution, device lifecycle and KDE integration require explicit work items. The current estimate precedes evidence that its central frame and collection budgets are achievable: [PLAN.md:154](/home/oystein/work/tasklens/docs/PLAN.md:154).