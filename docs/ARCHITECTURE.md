> **Note.** This file includes the reconciliation in `docs/RECONCILIATION.md` (R-001 to R-074) and the decisions in `docs/DECISIONS.md` (D-001 to D-010). If anything here still disagrees with either of those files, DECISIONS.md wins first and RECONCILIATION.md second. Phases use the canonical P0 to P4 numbering from R-072.

# tasklens Architecture Decision Records

Status: proposed, 2026-10-02; reconciled 2026-10-03. Supersedes `docs/PLAN.md` wherever they differ. `docs/DECISIONS.md` (D-001 to D-010) stays binding and these ADRs do not reopen it.

Implementation agents follow these records exactly. When a bead needs a choice these records do not make, the agent stops and asks; it does not pick.

Evidence tags: **[verified]** means read in source or run; **[inferred]** means reasoned from source but not run. Sources: `astra-architecture-review.md` (Astra), `claude-asupersync-apis.md` (ASX), `claude-gpui-apis.md` (GPUI), `claude-gpui-component-widgets.md` (KIT), `claude-missioncenter-inventory.md` section 7 (MAGPIE), `claude-taskslinger-inventory.md` (TS).

Where these records deviate from Astra, the ADR says so under "Deviation from Astra".

## Index

| ADR | Topic |
|---|---|
| 001 | Workspace layout and dependency rules |
| 002 | Toolchain, profiles and dependency pins |
| 003 | Data model contracts in tl-model |
| 004 | Engine runtime, supervision and the reducer |
| 005 | Engine and UI bridge, commands and action results |
| 006 | Process actions and privilege boundary |
| 007 | GPU probe child process protocol |
| 008 | Sampling schedule, counters, clocks and suspend |
| 009 | UI architecture: views, table, charts, caching, theme, fonts, icons |
| 010 | Window and platform integration |
| 011 | Settings and persistence |
| 012 | History storage (P3) |
| 013 | Testing strategy |
| 014 | Performance budgets, phase-0 spike and fallback |
| 015 | Logging, errors, crash handling and privacy |
| 016 | Data sources and parsing rules |

---

## ADR-001: Workspace layout and dependency rules

### Context

PLAN.md proposed five crates. Astra added a GPU child process (`tl-gpu-probe`) and asked for a hard rule on what "no Tokio" means. D-001 requires asupersync types to stay inside `tl-engine` so the runtime can be swapped. The owner wants a privileged helper, six-hour history and tray graphs later, so their crates must have a place now. The phase-0 fallback (ADR-014) replaces only the UI crate, so the UI must not depend on the engine crate.

### Decision

**Repository layout** (Cargo workspace, `resolver = "3"`, edition 2024 for every crate):

```text
tasklens/
  Cargo.toml              # [workspace], [workspace.dependencies], [workspace.lints], profiles
  Cargo.lock              # committed
  rust-toolchain.toml
  .cargo/config.toml      # [alias] xtask = "run -p xtask --"
  crates/
    tl-model/             # lib: data contracts, units, formatting, wire framing, EngineClient trait
    tl-collect/           # lib: synchronous samplers and parsers (procfs, sysfs, netlink)
    tl-ops/               # lib: synchronous OS operations (signals, nice, affinity, ioprio, cgroup writes)
    tl-engine/            # lib: asupersync runtime, workers, reducer, actions, D-Bus adapters, probe client
    tl-gpu-probe/         # bin: the only process that loads NVML
    tl-ui/                # lib: GPUI views, chart element, process table, theme
    tl-app/               # bin "tasklens": main, wiring, single instance, settings store, logging
    tl-history/           # lib, P3: history segment format, store and rolling window (ADR-012)
    tl-tray/              # lib, P3: StatusNotifierItem tray and tray graph rasterizer
    tl-helper/            # bin, P3: pkexec helper for privileged process actions
    tl-netmon/            # bin, P4: privileged eBPF per-process network collector
  xtask/                  # bin, not shipped: check-deps, capture-fixture, bench runner
  assets/
    fonts/                # bundled TTFs (ADR-009)
    icons/                # app SVG icons not in Lucide
    themes/               # built-in theme JSON (ADR-009)
  packaging/
    PKGBUILD
    io.github.oysteinkrog.Tasklens.desktop
    io.github.oysteinkrog.Tasklens.policy     # polkit, P3
  docs/
```

Crates for P3 and P4 are not created until their phase starts. Their names and dependency rows below are reserved now.

**Allowed dependencies.** A crate may depend only on what its row lists (plus `[dev-dependencies]`, which are unrestricted except Tokio rules below).

| Crate | Workspace deps allowed | External deps allowed |
|---|---|---|
| tl-model | none | serde, postcard, thiserror, async-channel, bitflags, smallvec, rustix (feature `time` only, for `clock_gettime`) |
| tl-collect | tl-model | rustix, memchr, thiserror, tracing, smallvec, rustc-hash, netlink-packet-core, netlink-packet-sock-diag, netlink-sys |
| tl-ops | tl-model | rustix, libc, thiserror, tracing |
| tl-engine | tl-model, tl-collect, tl-ops, tl-history (P3) | asupersync, arc-swap, async-channel, zbus, futures-lite, rustix, postcard, serde, thiserror, tracing, rustc-hash, smallvec |
| tl-gpu-probe | tl-model | nvml-wrapper, rustix, postcard, serde |
| tl-ui | tl-model | gpui-kit, tracing, smallvec, rustc-hash, serde, serde_json (theme JSON) |
| tl-app | tl-model, tl-engine, tl-ui, tl-tray (P3) | zbus, toml, serde, anyhow, tracing, tracing-subscriber, tracing-appender, rustix |
| tl-history | tl-model | zstd, crc32fast, postcard, serde, thiserror |
| tl-tray | tl-model | ksni |
| tl-helper | tl-model, tl-ops | rustix, postcard, serde |
| tl-netmon | tl-model | decided in its own ADR when that phase starts |
| xtask | none | serde, serde_json, anyhow |

**Dependency direction rule.** Edges only point toward `tl-model`:

```text
tl-app -> tl-ui -> tl-model
tl-app -> tl-engine -> {tl-collect, tl-ops, tl-history} -> tl-model
tl-gpu-probe, tl-helper, tl-tray -> tl-model (tl-helper also -> tl-ops)
```

Hard rules:
1. `tl-ui` never depends on `tl-engine`, `tl-collect`, `tl-ops`, `asupersync`, `arc-swap` or `zbus`. It talks to the engine only through the `EngineClient` trait in `tl-model` (ADR-005).
2. `asupersync` appears in the normal dependency closure of `tl-engine` and `tl-app` only. No public item of `tl-engine` names an asupersync type. `tl-engine`'s public API is `EngineConfig`, `Engine::start`, `EngineHandle` (implements `EngineClient`) and `ShutdownReport`.
3. `tl-model`, `tl-collect` and `tl-ops` contain no async code and no runtime.
4. `unsafe` is denied workspace-wide (`unsafe_code = "deny"`). It is allowed only in these files, each with `#![allow(unsafe_code)]` at file level and a `// SAFETY:` comment per block: `crates/tl-ops/src/sys.rs`, `crates/tl-engine/src/gpu/spawn.rs`, `crates/tl-app/src/main.rs` (environment setup before any thread starts). Adding a file to this list needs an ADR change.

**"No Tokio" definition.** None of these crates appear in the resolved normal or build dependency graph of any shipped binary (`tasklens`, `tl-gpu-probe`, `tl-helper`, `tl-netmon`) for target `x86_64-unknown-linux-gnu`: `tokio`, `tokio-util`, `tokio-stream`, `tokio-macros`, `async-std`, `async-global-executor`, `actix-rt`, `reqwest`, `hyper`. GPUI's own executor, smol, async-io, async-executor and blocking are allowed. A banned crate that is only a dev-dependency is allowed.

**CI check: `cargo xtask check-deps`.** It runs and fails on any violation:

1. For each shipped binary package `P`:
   `cargo tree -p P -e normal,build --target x86_64-unknown-linux-gnu --prefix none --format "{p}"`
   Fail if any line's first word is in the banned list. (This avoids depending on how `cargo tree -i` reports a package that is absent.)
2. `cargo metadata --format-version 1` and compute each workspace member's normal-dependency closure. Fail if a member depends on anything outside its row in the table above (direct dependencies), if `asupersync` is in the closure of any member other than `tl-engine` and `tl-app`, or if `tl-ui`'s closure contains `tl-engine`, `tl-collect`, `tl-ops`, `arc-swap` or `zbus`.
3. `rg -n "asupersync::" crates/ --glob '!crates/tl-engine/**'` must return nothing.

### Consequences

- The engine runtime can be replaced by changing `tl-engine` only.
- The UI can be tested against a fake engine and replaced by an eframe UI crate without touching the engine.
- `tl-ops` is shared by the unprivileged engine and the P3 root helper, so privileged code paths are the same code.

### Rejected alternatives

- **Five crates as in PLAN.md.** Rejected: NVML must be in a separate process (ADR-007), and process operations must be shareable with the helper.
- **A full magpie-style sampler process.** Rejected per Astra section 3: adds serialization and resync cost without improving UI responsiveness.
- **`cargo tree -i tokio` alone.** Rejected: its exit status when the package is absent from the graph is not something we want to depend on.
- **`cargo deny`.** Not needed for one rule; may be added later for license checks.

### Verification

- `cargo xtask check-deps` passes in CI on every commit.
- A deliberate test branch that adds `tokio` to `tl-ui` makes `check-deps` fail (run once when the check lands; record the output in the bead).

---

## ADR-002: Toolchain, profiles and dependency pins

### Context

ASX verified asupersync 0.5.0 on stable rustc 1.98.1 with `default-features = false, features = ["proc-macros"]`. Its default features enable `nightly-outcome-try`, which needs nightly. GPUI notes verified that `gpui-kit = "=0.7.0"` pins the whole `gpui-pre =0.3.7` family (snapshot of zed@1a28cff) and enables `wayland`, `x11`, `font-kit`, `runtime_shaders` on `gpui-pre-platform`. Mixing gpui-pre versions breaks the build (gpui-kit #2532). I checked the gpui-pre 0.3.7 manifests: only `gpui-pre-reqwest-client` depends on Tokio, and nothing in the gpui-kit Linux graph appears to depend on it [verified for manifests, inferred for the resolved graph; ADR-001's check proves it].

Astra's review names gpui-component revision `3467e64` with gpui-pre 0.3.7. That is the same GPUI snapshot that crates.io `gpui-kit 0.7.0` uses, so the crates.io release is used, not a git pin.

### Decision

**`rust-toolchain.toml`:**

```toml
[toolchain]
channel = "1.98.1"
components = ["rustfmt", "clippy"]
targets = ["x86_64-unknown-linux-gnu"]
profile = "minimal"
```

Toolchain upgrades are separate beads with a full test and perf run.

**Workspace dependencies** (in root `Cargo.toml` `[workspace.dependencies]`; crates use `dep = { workspace = true }`). `=` means an exact pin; others are caret requirements with `Cargo.lock` committed.

```toml
[workspace.dependencies]
# Runtime and UI (exact pins; upgrades only through an ADR change)
asupersync = { version = "=0.5.0", default-features = false, features = ["proc-macros"] }
gpui-kit   = { version = "=0.7.0" }                       # default features: component, assets
nvml-wrapper = { version = "=0.13.0", default-features = false }
ksni       = { version = "=0.3.6", default-features = false, features = ["async-io", "blocking"] }  # P3

# Bridge
arc-swap      = "1.9.2"
async-channel = "2.5.0"

# D-Bus: async-io backend; blocking API used by tl-app only
zbus = { version = "5.19.0", default-features = false, features = ["async-io", "blocking-api"] }

# System access
rustix = { version = "1.1.5", features = ["fs", "process", "net", "event", "system", "thread", "time", "pipe"] }
libc   = "0.2.189"
memchr = "2.8.3"
netlink-packet-core      = "0.9.0"
netlink-packet-sock-diag = "0.5.0"
netlink-sys = { version = "0.9.0", default-features = false }

# Data and encoding
serde    = { version = "1.0.228", features = ["derive"] }
serde_json = "1"
postcard = { version = "1.1.3", default-features = false, features = ["use-std"] }
toml     = "1.1.6"
bitflags = "2"
smallvec = { version = "1.15.2", features = ["union", "const_generics"] }
rustc-hash = "2.1.3"
futures-lite = "2.6.1"
zstd      = "0.13.3"     # P3
crc32fast = "1"          # P3

# Errors and logging
thiserror = "2.0.21"
anyhow    = "1.0.104"
tracing   = "0.1.44"
tracing-subscriber = { version = "0.3.23", default-features = false, features = ["fmt", "env-filter", "ansi", "std"] }
tracing-appender   = "0.2.5"

# Test only
insta     = "1.48.0"
proptest  = "1.11.0"
tempfile  = "3.27.0"
criterion = "0.8.2"
```

Dev-dependency forms:

```toml
# crates/tl-engine/Cargo.toml
[dev-dependencies]
asupersync = { workspace = true, features = ["proc-macros", "test-internals"] }

# crates/tl-ui/Cargo.toml
[dev-dependencies]
gpui-kit = { workspace = true, features = ["test-support"] }

[features]
bench = ["gpui-kit/profiler"]     # frame histograms for --bench (ADR-014)
```

`tl-app` has `[features] bench = ["tl-ui/bench"]`. Release packages are built without `bench`; perf gate runs use a separate build with it.

Not used: `lyon` (GPUI's `PathBuilder` tessellates with lyon internally), `procfs` (allocation-heavy; parsers are hand-written, ADR-016), `sysinfo`, `directories` (XDG paths are resolved by hand, ADR-011), `tokio` in any form, `smol` directly (it comes in through gpui-kit only).

**NVML access: `nvml-wrapper`, not raw dlopen.** It already loads `libnvidia-ml.so.1` at runtime through `libloading` and maps every error code. It is used only in `tl-gpu-probe`.

**zbus executor rule.** Engine connections are built with `.internal_executor(false)` and driven by an asupersync task (ADR-004). The `tl-app` single-instance connection uses the blocking API with zbus's internal executor thread. `ksni` (P3) runs its own async-executor thread through its `blocking` API.

**Cargo profiles** (root `Cargo.toml`):

```toml
[profile.dev]
opt-level = 1
[profile.dev.package."*"]
opt-level = 2                 # GPUI and wgpu are too slow at opt-level 0 for UI work

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "unwind"              # required: supervisors and the engine session catch panics
debug = "line-tables-only"    # readable crash backtraces
strip = "none"

[profile.profiling]
inherits = "release"
debug = true
```

**Workspace lints:**

```toml
[workspace.lints.rust]
unsafe_code = "deny"
[workspace.lints.clippy]
dbg_macro = "deny"
print_stdout = "deny"         # xtask and the bench output module allow it locally
unwrap_used = "warn"          # allowed in tests
```

**Build and run packages (Arch):** build needs `base-devel clang pkgconf`; runtime needs `wayland libxkbcommon libxkbcommon-x11 libxcb fontconfig vulkan-icd-loader nvidia-utils` (all already installed, per GPUI section 1).

### Consequences

- Stable toolchain only. `Outcome` from asupersync is matched explicitly; `?` does not work on it.
- Any gpui-kit upgrade is a planned bead that re-runs the phase-0 fixture.

### Rejected alternatives

- **Nightly toolchain** for asupersync's default features: rejected, unnecessary.
- **Git pin of zed's `gpui`**: rejected; gpui-kit's crates.io snapshot is coherent and verified.
- **gpui-ce or a private GPUI fork for custom shaders**: rejected per Astra section 1. The only allowed GPUI patch is the zed#64499 frame-loop fix (ADR-010), applied only if the soak test fails.

### Verification

- `cargo +1.98.1 build --workspace --locked` succeeds.
- `cargo tree -p tl-engine -e features -i asupersync` shows no `nightly-outcome-try` and no `test-internals` in the non-dev build.
- `cargo tree -p tasklens -i gpui-pre --prefix none` shows exactly one version, `0.3.7`.

---

## ADR-003: Data model contracts in tl-model

### Context

Astra requires immutable snapshots with per-component versions, process identity that survives PID reuse, explicit validity states, monotonic timestamps, and never encoding missing as zero. History (ADR-012) needs stable series identifiers and timestamps that survive suspend.

**Deviation from Astra:** Astra's `Sample` uses `mono_ns`. These records use CLOCK_BOOTTIME (`t_ns`) instead, because it keeps counting during suspend. A suspend then shows as a real gap on the time axis, and history timestamps stay comparable across suspend. CLOCK_MONOTONIC is still read beside it to detect suspend (ADR-008).

### Decision

**Module layout of `tl-model`:**

```text
src/lib.rs        # re-exports
src/time.rs       # now_boot_ns(), now_mono_ns(), BootId
src/ids.rs        # ProcessKey, DeviceIdx, SeriesId, MetricId, GroupKey, NodeId, ColumnId
src/validity.rs   # Validity, FieldValidity
src/series.rs     # Sample, SampleFlags, SeriesRing, SeriesView
src/snapshot.rs   # Snapshot and component types
src/process.rs    # ProcessRecord, ProcessMeta, ProcState, ProcFlags, groups
src/table.rs      # TableQuery, ProcessTableView, TableRow
src/devices.rs    # DeviceCatalog, DeviceIdentity
src/system.rs     # CpuTotals, MemoryTotals, DiskTotals, NetTotals, GpuTotals, SensorReading
src/health.rs     # SourceId, SourceHealth, SourceState
src/actions.rs    # ActionId, ActionRequest, ActionKind, ActionTarget, ActionState, ActionEvent, ActionError
src/demand.rs     # Demand and its parts (ADR-008)
src/settings.rs   # Settings types (ADR-011); no file I/O
src/units.rs      # formatting: IEC bytes, rates, percent, durations
src/wire.rs       # length-prefixed postcard framing (ADR-007), used by probe and helper
src/gpu_proto.rs  # GPU probe messages (ADR-007)
src/client.rs     # EngineClient trait (ADR-005)
```

**Time.**

```rust
/// Nanoseconds of CLOCK_BOOTTIME. Every sample and snapshot timestamp uses this clock.
pub type TimeNs = u64;
pub fn now_boot_ns() -> TimeNs;   // rustix::time::clock_gettime(ClockId::Boottime)
pub fn now_mono_ns() -> u64;      // ClockId::Monotonic, used only for suspend detection

/// Folded from /proc/sys/kernel/random/boot_id: first 8 bytes XOR last 8 bytes of the UUID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct BootId(pub u64);
```

**Process identity.**

```rust
/// Identity of one process for one boot. PID alone is never used as identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessKey {
    pub start_ticks: u64, // /proc/<pid>/stat field 22, clock ticks since boot
    pub boot_id: BootId,
    pub pid: u32,
}
```

Ordering is by `(start_ticks, boot_id, pid)`; the table uses it as the stable sort tie-break.

**Validity.** Every displayed value carries one of these states. Missing data is never zero.

```rust
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Validity {
    Valid = 0,
    #[default]
    WarmingUp = 1,  // source running, no rate yet (needs two samples)
    Stale = 2,      // last value older than the stale threshold (ADR-008)
    Restricted = 3, // permission denied (other user's process, root-only file)
    Unsupported = 4,// hardware or driver does not provide this metric
    Missing = 5,    // device or process is gone
    Failed = 6,     // collection error; see SourceHealth.last_error
}
```

How the UI shows each state (exact strings). Table cells, header totals and sidebar readouts use the glyphs in the "Text cell" column. Performance stats grids, Information rows and Properties key/value fields use the words from PERF-079 instead ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown"). Every surface uses the tooltips below.

| State | Text cell | Chart | Tooltip |
|---|---|---|---|
| Valid | formatted value | drawn | none |
| WarmingUp | `…` (U+2026) in secondary text color | nothing drawn yet | "Collecting first sample" |
| Stale | last value in secondary text color | line and fill drawn at 50% alpha after the last fresh sample, then a gap | "Last updated {age} ago" |
| Restricted | Lucide `lock` icon, 12 logical px, secondary color | gap | "Needs administrator access. Click the shield to grant it." (before P3: "Needs administrator access.") |
| Unsupported | `n/a` in secondary color | gap | "Not available on this device" |
| Missing | `-` in secondary color | gap | "No longer present" |
| Failed | `!` in the theme's warning color | gap | "Could not read: {last_error}" |

"Needs setup" is `Unsupported` with a reason tooltip that links to the Additional Setup window. "Not reported" is `Unsupported` where the source answers with no value.

**Field validity in records.** `FieldValidity` holds one `Validity` per numeric field: `pub struct FieldValidity(pub [Validity; PROCESS_FIELDS]);` with `PROCESS_FIELDS = 13`, indexed by `ProcessField` (`Cpu, RssAnon, Rss, Pss, Uss, Swap, IoRead, IoWrite, NetRx, NetTx, GpuUtil, GpuMem, Threads`).

**Series.**

```rust
bitflags! { #[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SampleFlags: u32 {
    const VALID      = 1 << 0; // value is meaningful; if clear, the sample marks a gap
    const GAP_BEFORE = 1 << 1; // do not connect to the previous sample
    const RESET      = 1 << 2; // counter baseline was reset at this sample
    const STALE      = 1 << 3; // published while the source was stale
}}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample { pub t_ns: TimeNs, pub value: f32, pub flags: SampleFlags } // 16 bytes

/// Engine-owned ring. Not shared with the UI.
pub struct SeriesRing { samples: Box<[Sample]>, next: u32, len: u32, version: u64 }

/// Published, immutable, chronological copy of a ring.
#[derive(Clone, Debug)]
pub struct SeriesView {
    pub id: SeriesId,
    pub version: u64,       // bumps on every append
    pub period_ns: u64,     // current nominal sampling period
    pub samples: Arc<[Sample]>,
}
```

Graph range choices are 30 s, 1 min, 5 min and 10 min (default 1 min). P1 ships 30 s and 1 min; 5 and 10 min arrive in P2. Every series has two ring tiers:
- Fast tier, capacity `ceil(60 s / period_ns) + 2`: 602 samples at 10 Hz, 62 at 1 Hz. It serves the 30 s and 1 min ranges.
- 1 Hz tier (from P2), capacity 602 samples (10 min + 2). Each value is the per-second maximum of the fast tier, the same rule ADR-012 uses for history. It serves the 5 and 10 min ranges.

`Sample` stays 16 bytes; there is no per-sample min/max pair. Memory is about 1.5 MB for 80 series. A source that changes rate keeps its ring and marks `GAP_BEFORE` on the first sample at the new rate. Raw counters stay `u64` inside `tl-collect` and `tl-engine`; only graph values become `f32`.

**Identifiers.** These numeric values are stored in history files (ADR-012) and must never be renumbered.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct DeviceIdx(pub u16);  // index into DeviceCatalog for this engine session

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct MetricId(pub u16);
impl MetricId {
    pub const CPU_TOTAL: Self = Self(1);   pub const CPU_KERNEL: Self = Self(2);
    pub const CPU_FREQ: Self = Self(3);    pub const CPU_TEMP: Self = Self(4);
    pub const MEM_USED: Self = Self(10);   pub const MEM_COMMIT: Self = Self(11);
    pub const SWAP_USED: Self = Self(12);  pub const MEM_CACHED: Self = Self(13);
    pub const DISK_ACTIVE: Self = Self(20);pub const DISK_READ: Self = Self(21);
    pub const DISK_WRITE: Self = Self(22); pub const DISK_LATENCY: Self = Self(23);
    pub const NET_RX: Self = Self(30);     pub const NET_TX: Self = Self(31);
    pub const GPU_UTIL: Self = Self(40);   pub const GPU_MEM: Self = Self(41);
    pub const GPU_ENC: Self = Self(42);    pub const GPU_DEC: Self = Self(43);
    pub const GPU_TEMP: Self = Self(44);   pub const GPU_POWER: Self = Self(45);
    pub const GPU_CLOCK: Self = Self(46);
    pub const FAN_RPM: Self = Self(50);    pub const SENSOR_TEMP: Self = Self(51);
    pub const BATTERY_PCT: Self = Self(60);
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct SeriesId { pub device: DeviceIdx, pub metric: MetricId }
```

The CPU device with `DeviceIdx` for "all CPUs" carries total series; each logical CPU is its own device.

**Device identity.**

```rust
pub enum DeviceIdentity {
    CpuPackage,                                   // the whole machine's CPU
    CpuLogical { id: u16 },
    Memory,
    Gpu { uuid: [u8; 16], pci_bus_id: Arc<str> }, // NVML UUID, or DRM PCI slot for non-NVIDIA
    Disk { major: u32, minor: u32, name: Arc<str>, wwn_or_serial: Option<Arc<str>> },
    Net { netns_ino: u64, ifindex: u32, name: Arc<str> },
    Sensor { chip: Arc<str>, label: Arc<str> },   // hwmon name plus label
    Fan { chip: Arc<str>, index: u16 },
    Battery { native_path: Arc<str> },
}
pub struct DeviceEntry {
    pub idx: DeviceIdx, pub identity: DeviceIdentity, pub generation: u32,
    pub display_name: Arc<str>, pub present: bool,
    pub parent: Option<DeviceIdx>,  // partitions and device-mapper children point to their disk
}
pub struct DeviceCatalog { pub version: u64, pub entries: Arc<[DeviceEntry]> }
```

A device that disappears keeps its `DeviceIdx` with `present = false`. A device that comes back with the same identity gets the same index and `generation + 1`. Counters are never subtracted across generations.

**Process record and metadata.**

```rust
pub struct ProcessMeta {
    pub comm: Arc<str>,                // /proc/<pid>/comm, sanitized
    pub name: Arc<str>,                // display name: desktop entry Name, else exe basename, else comm
    pub exe: Option<Arc<str>>,         // readlink /proc/<pid>/exe; None if restricted
    pub cmdline: Option<Arc<str>>,     // bounded to 32 KiB, NULs to spaces, sanitized (ADR-015)
    pub user: Arc<str>,                // user name for uid
    pub cgroup: Option<Arc<str>>,      // cgroup v2 path relative to /sys/fs/cgroup
    pub app_id: Option<Arc<str>>,      // from the app scope or desktop entry
    pub icon: Option<Arc<str>>,        // resolved icon file path
    pub description: Option<Arc<str>>, // desktop entry Comment or GenericName
    pub search_lower: Arc<str>,        // lowercase filter text: display name, image name, cmdline, user,
                                       // description, app name, unit name (R-043)
}

#[repr(u8)] pub enum ProcState { Running, Sleeping, DiskSleep, Stopped, Zombie, Idle, Other }

bitflags! { pub struct ProcFlags: u16 {
    const NEW = 1;           // appeared within the linger time: 2 × processes.interval_ms, clamped to 1..=3 s
    const EXITED = 2;        // exited within the linger time; kept for highlight, then removed
    const KERNEL_THREAD = 4;
    const OTHER_USER = 8;
    const IS_SELF = 16;      // tasklens or its probe
    const PROTECTED = 32;    // see ADR-006
}}

#[derive(Clone)]
pub struct ProcessRecord {
    pub key: ProcessKey,
    pub ppid: u32,
    pub uid: u32,
    pub meta: Arc<ProcessMeta>,
    pub group: GroupKey,
    pub state: ProcState,
    pub flags: ProcFlags,
    pub nice: i8,
    pub threads: u32,
    pub cpu_pct: f32,          // percent of whole machine capacity, 0..=100
    pub rss_anon_bytes: u64,   // RssAnon from /proc/<pid>/status; the default Memory column (D-004)
    pub rss_bytes: u64,        // VmRSS; the "Working set total" column
    pub pss_bytes: u64,        // only for demanded keys (ADR-008)
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    pub net_rx_bps: f32,       // attributed TCP only (ADR-016)
    pub net_tx_bps: f32,
    pub gpu_util_pct: f32,
    pub gpu_mem_bytes: u64,
    pub validity: FieldValidity,
}
```

Size target: 160 bytes or less per record excluding the shared `ProcessMeta` (check with `size_of` in a test).

**Groups.**

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct GroupKey(pub u64); // FxHash of GroupKind
pub enum GroupKind {
    App { app_id: Arc<str> },   // cgroup app-*.scope or app-*.service under user@<uid>.service/app.slice
    Background,                 // processes of the current uid not in an app scope
    System,                     // other uids, system.slice, init.scope, kernel threads
    Ungrouped,                  // cgroup unreadable
}
```

**Snapshot.**

```rust
pub struct ComponentHeader {
    pub version: u64,       // bumps when the component content changes
    pub observed_start: TimeNs,
    pub observed_end: TimeNs,
    pub validity: Validity,
}

pub struct Snapshot {
    pub sequence: u64,           // strictly increasing per engine process
    pub epoch: u32,              // engine session number (ADR-004)
    pub boot_id: BootId,
    pub published_ns: TimeNs,
    pub system: Arc<SystemSnapshot>,
    pub series: Arc<SeriesSet>,
    pub processes: Arc<ProcessSnapshot>,
    pub table: Arc<ProcessTableView>,
    pub connections: Arc<ConnectionsSnapshot>,
    pub services: Arc<ServicesSnapshot>,
    pub startup: Arc<StartupSnapshot>,
    pub inventory: Arc<InventorySnapshot>,
    pub devices: Arc<DeviceCatalog>,
    pub health: Arc<HealthSnapshot>,
    pub windows: Arc<WindowsSnapshot>, // P2: KWin window list keyed by pid (ADR-004, KWin worker)
}

pub struct SeriesSet {
    pub header: ComponentHeader,
    pub series: Arc<[Arc<SeriesView>]>, // sorted by SeriesId; unchanged series reuse their Arc
}

pub struct ProcessSnapshot {
    pub header: ComponentHeader,
    pub records: Arc<[ProcessRecord]>,  // sorted by ProcessKey
    pub coverage: Coverage,             // which expensive fields cover which keys (ADR-008)
}

pub struct ComponentMask(pub u16); // one bit per Snapshot component, computed by the UI
```

`WindowsSnapshot` holds the KWin window list keyed by pid. Each entry has `internal_id`, `caption`, `resource_class`, `desktop_file_name`, `minimized`, `maximized`, `keep_above`, `frame_geometry` and `window_type`. Without the KWin bridge it is empty with validity `Unsupported`.

Every component except `series` is replaced as a whole new `Arc` when it changes. A publish that changes only CPU series reuses every other `Arc`.

**Units and formatting (`units.rs`).** Canonical table (d) in `docs/RECONCILIATION.md` is the full list. In short:
- Percent: one decimal everywhere (`%.1f%%`: `3.4%`, `37.0%`); a measured zero is `0%`.
- Memory and storage sizes: IEC, base 1024, labels `B KiB MiB GiB TiB`, two decimals from 1 KiB up (`1.22 GiB`, `16.00 KiB`); integer bytes below 1 KiB (`512 B`). Copy details lines use whole MiB.
- Disk and I/O rates: `MiB/s` with one decimal in tables; auto-scaled IEC (`KiB/s`, `MiB/s`, `GiB/s`) with one decimal on Performance graphs and stats.
- Network rates: setting `units.network = "bits" | "bytes"`, default `"bits"`, and `units.network_auto_scale`, default `false`. Bits use SI, base 1000 (`Kbps Mbps Gbps`); bytes use IEC (`KiB/s`, `MiB/s`). With auto-scale off, values are fixed `Mbps` (or `MiB/s`) with one decimal, `0 Mbps` at zero. Graph axes use 1-2-5 × 10ⁿ rounding whatever the setting.
- Counts: digits grouped with a thin space (U+2009), `12 345`.
- Frequencies: integer `MHz` (`4631 MHz`).
- Temperatures: integer `°C`. Power: one decimal `W`.
- Uptime: `2 days, 2h 38m 15s`. Short ages (tooltips and the Start time column's age text only): `1d 4h`, `4h 12m`, `12m 5s`, `5s`.
- Formatting functions write into a caller-provided `String` and never allocate when the buffer is large enough.

### Consequences

- The UI can render any value without guessing what zero means.
- Process records are shared between snapshots by `Arc`, so a 10 Hz CPU publish costs no process copying.
- Device and metric identifiers are part of the history file format and are frozen.

### Rejected alternatives

- **PID as identity.** Rejected: PID reuse makes actions hit the wrong process.
- **`Option<f32>` for missing values.** Rejected: loses the reason, which the UI must show.
- **CLOCK_MONOTONIC for sample time.** Rejected: hides suspend in the time axis and breaks history.

### Verification

- Unit tests: `size_of::<Sample>() == 16`, `size_of::<ProcessKey>() == 24`, `size_of::<ProcessRecord>() <= 160`.
- A test serializes every `MetricId` constant and compares against a checked-in list; changing a number fails the test.
- Formatting tests cover every unit with boundary values (0, 1023, 1024, 999.95, u64::MAX).

---

## ADR-004: Engine runtime, supervision and the reducer

### Context

ASX verified that `ManagedSupervisor` is the only supervisor that restarts children in a production runtime; `AppSpec`, `GenServer` and `actor` need `&mut RuntimeState`, which only `LabRuntime` exposes. The restart budget is shared across a supervisor's children, so each worker needs its own supervisor. The blocking pool is off by default and must be configured. A restartable worker must not own a single-owner `mpsc::Receiver`. Astra requires a single publisher (the reducer), one-for-one restarts, the backoff sequence 0.5, 1, 2, 4, 8, 16, 30 s, and "disable for one minute after five failures in a minute".

**Deviation from Astra:** Astra suggested a blocking pool of 2 threads. This ADR uses 1 to 4, because the GPU probe client can hold a blocking thread for up to 500 ms per request and PSS reads and sock_diag dumps run in parallel with the process sweep. Astra's `AppSpec` tree is replaced by one `ManagedSupervisor` per worker, as ASX requires.

### Decision

**Runtime construction** (`crates/tl-engine/src/runtime.rs`):

```rust
let rt = RuntimeBuilder::new()
    .worker_threads(1)
    .blocking_threads(1, 4)
    .thread_name_prefix("tl-engine")
    .global_queue_limit(256)
    .build()?;
```

Never `current_thread()`. Never `RuntimeHandle::spawn_blocking`. All blocking work uses `Cx::spawn_blocking(..)` and is joined with `TaskHandle::join(&mut self, &cx)`. One worker does a whole sweep inside one `spawn_blocking` closure (never one pool job per file).

**Banned asupersync APIs in tl-engine production code:** `AppSpec`, `GenServer`, `actor`, `race!`, `select!`, `race_drained`, `Cx::race_timeout`, `asupersync::signal`, `ctrl_c`, `asupersync::process`, `asupersync::fs`, `Cx::for_request`, `Cx::for_testing`, `ChildSpec::depends_on`, `JoinHandle` from `RuntimeHandle::spawn` (use `spawn_checked`), `Budget::with_poll_quota` on long-lived tasks. `xtask check-deps` greps `crates/tl-engine/src` for these names (excluding `tests/` and `#[cfg(test)]` modules) and fails on a match.

**Task tree:**

```text
engine_root (spawned with RuntimeHandle::try_spawn_with_cx)
  owns: UI-facing endpoints (EngineMsg receiver, demand/settings watch senders' receivers)
  loop over sessions (epoch = 1, 2, ...):
    engine_session(cx, &mut endpoints, epoch)    # awaited inline, wrapped in catch_unwind
      reducer loop (inline in the session task)  # single owner of state and the only publisher
      Vec<SourceController>                      # one per source, each spawned with cx.spawn
        SourceController loop:
          ManagedSupervisor (one child: the worker), spawned, joined
          on budget exhausted: health = Disabled{until}, sleep 60 s, new supervisor
      actions region                             # one task per accepted action (ADR-005, 006)
```

- `engine_session` runs inline in `engine_root` via `futures_lite::FutureExt::catch_unwind(AssertUnwindSafe(..))`, so the receiver it borrows survives a session panic. All `ManagedSupervisorHandle`s and action `TaskHandle`s live in a `Vec` owned by the session; dropping them on unwind aborts every child.
- On a session panic or a reducer `Err(InvariantViolation)`: log at error level, wait 1 s, increment `epoch`, start a new session. The new session starts with empty rings and every series gets `GAP_BEFORE` on its first sample. More than 3 session restarts in 60 s: stop restarting, publish a snapshot whose `health` marks every source `Failed`, and keep the reducer-free root alive so the UI shows the error instead of crashing.

**Per-worker supervisor** (`crates/tl-engine/src/supervise.rs`):

```rust
let compiled = SupervisorBuilder::new(name)
    .with_restart_policy(RestartPolicy::OneForOne)
    .child(ChildSpec::new(name, legacy_unreachable))   // legacy start stub; never called
    .compile()?;
let config = SupervisionConfig::new(5, Duration::from_secs(60))
    .with_restart_policy(RestartPolicy::OneForOne)
    .with_backoff(BackoffStrategy::Exponential {
        initial: Duration::from_millis(500),
        max: Duration::from_secs(30),
        multiplier: 2.0,
    })
    .with_escalation(EscalationPolicy::Stop);
let sup = compiled.bind_managed(
    vec![ManagedChildBinding::new(name, ManagedRestartMode::Transient, factory)],
    config,
)?;
let mut handle = sup.spawn(&cx)?;
let report = handle.join(&cx).await;
```

- `legacy_unreachable` is the stub from `tests/supervision_regression.rs:197-204` in asupersync.
- `Transient`: a worker returns `Outcome::Ok(())` only when cancelled; `Err` or panic restarts it.
- When the report shows the restart budget is spent (5 restarts inside 60 s), the controller publishes `SourceState::Disabled { until }`, sleeps 60 s with `time::sleep`, then binds a fresh supervisor. This gives "five failures in a minute, then off for a minute".
- Expected conditions are **not** failures: permission denied, missing device, unsupported NVML function and absent files produce a `SourceUpdate` with the right `Validity` and the worker keeps running.

**Worker factory contract.** Each factory captures only cloneable endpoints and creates fresh ones per generation:

```rust
pub struct WorkerIo {
    pub updates: mpsc::Sender<EngineMsg>,        // clone per generation
    pub demand: watch::Receiver<Demand>,         // clone per generation
    pub settings: watch::Receiver<EngineSettings>,
}
```

Never move an `mpsc::Receiver` into a restartable worker. Never hold a `watch` `Ref` across any other channel call; use `borrow_and_clone()` (bead `c11v6v`).

**Worker list** (each is one `ManagedSupervisor`; `SourceId` is the enum in `tl-model::health`):

| SourceId | Module | Phase | Work |
|---|---|---|---|
| `Cpu` | `workers/cpu.rs` | P1 | `/proc/stat`, cpufreq, CPU temperature |
| `Memory` | `workers/memory.rs` | P1 | `/proc/meminfo`, zram `mm_stat`, `/proc/vmstat` |
| `Disk` | `workers/disk.rs` | P1 | `/proc/diskstats`, `/sys/block` enumeration |
| `Net` | `workers/net.rs` | P1 | `/proc/net/dev`, `/sys/class/net` enumeration |
| `GpuNvidia` | `workers/gpu_nvidia.rs` | P1 | client of `tl-gpu-probe` (ADR-007) |
| `GpuDrm` | `workers/gpu_drm.rs` | P2 (AMD), P3 (Intel) | AMD and Intel via sysfs and DRM fdinfo |
| `Sensors` | `workers/sensors.rs` | P1 | hwmon temperatures and fans |
| `Processes` | `workers/processes.rs` | P1 | full process sweep |
| `ProcessDetail` | `workers/process_detail.rs` | P1 | PSS/USS and other per-key enrichment |
| `Connections` | `workers/connections.rs` | P2 | sock_diag dump and socket-to-pid map |
| `SystemdSystem` | `workers/dbus_systemd.rs` | P2 | system manager over the system bus |
| `SystemdUser` | `workers/dbus_systemd.rs` | P2 | user manager over the session bus |
| `Logind` | `workers/dbus_logind.rs` | P1 | `PrepareForSleep` signal (ADR-008) |
| `Startup` | `workers/startup.rs` | P2 | XDG autostart and systemd user unit files |
| `Inventory` | `workers/inventory.rs` | P2 | static hardware and OS information |
| `KWin` | `workers/dbus_kwin.rs` | P2 | KWin window bridge, event-driven (below) |
| `Udisks` | `workers/dbus_udisks.rs` | P2 | disk model, SMART, partitions, eject |
| `UPower` | `workers/dbus_upower.rs` | P2 | batteries |
| `NetworkManager` | `workers/dbus_nm.rs` | P2 | interface kind, addresses, Wi-Fi |
| `HistoryRecorder` | `workers/history.rs` | P3 | ADR-012 store and rolling window |

**KWin bridge worker.** `workers/dbus_kwin.rs` runs on the engine's session-bus connection. Availability: `XDG_CURRENT_DESKTOP` contains `KDE` and `org.kde.KWin` is on the bus. Otherwise the window features are hidden and the UI shows the toast "Window features need KDE Plasma". The worker:
- writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js` and loads it as KWin script plugin `tasklens-bridge`;
- exports object `/io/github/oysteinkrog/Tasklens/WindowBridge`, interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s json)`, which the script calls with the window list;
- publishes the list as `Snapshot.windows` (ADR-003);
- runs commands (activate, close, keep above, pick) from `Command::KWin(KWinCommand)` (ADR-005) as one-shot scripts named `tasklens-cmd-<n>`; the window picker uses `tasklens-pick`.

There is no X11 EWMH fallback for the window features in v1 (X11 is best effort, ADR-010).

**Sampling worker loop** (every periodic worker follows this shape):

```rust
async fn run(cx: Cx, io: WorkerIo, mut src: impl Source) -> Outcome<(), WorkerError> {
    let mut period = src.period(&io.demand.borrow_and_clone(), &io.settings.borrow_and_clone());
    let mut iv = interval(cx.now(), period.unwrap_or(IDLE_RECHECK));
    iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        sleep_until(iv.deadline()).await;                       // new Sleep each turn
        if cx.checkpoint().is_err() { return Outcome::Ok(()); }
        iv.tick(cx.now());
        // re-read demand; rebuild iv if the period changed
        let Some(p) = period else { continue };                 // source off: recheck every IDLE_RECHECK
        let job = cx.spawn_blocking(move |_| src.sample())?;   // whole sweep in one job
        let (s, update) = job.join(&cx).await?;                  // await; never start a second job
        src = s;
        io.updates.send(&cx, EngineMsg::Source(update)).await?;
    }
}
```

- `IDLE_RECHECK = 500 ms`: an off source wakes twice a second to read demand, does no I/O, and costs nothing measurable.
- Demand changes take effect at the next wake (at most 500 ms, or one period).
- A job that runs past its period is not interrupted. The worker skips the missed ticks and the health becomes `Late` after 3 consecutive jobs longer than 0.8 × period.
- The `Source` trait lives in `tl-engine` and wraps synchronous `tl-collect` samplers:

```rust
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration>; // None = off
    fn sample(self) -> (Self, SourceUpdate) where Self: Sized;   // runs on the blocking pool
}
```

**D-Bus workers.** Each D-Bus worker generation opens its own `zbus::Connection` with `.internal_executor(false)` and spawns a child task that drives it:

```rust
loop {
    let _ = time::timeout(cx.now(), Duration::from_millis(500), conn.executor().tick()).await;
    if cx.checkpoint().is_err() { break; }
}
```

Every zbus call and every signal-stream `next()` is wrapped in `time::timeout` (2 s for reads, ADR-006 for actions), followed by `cx.checkpoint()`. Proxies never outlive their generation. On reconnect a worker re-lists everything (full resync) before applying signals.

**The reducer** (`crates/tl-engine/src/reducer.rs`) is the only code that writes the published slot.

```rust
pub enum EngineMsg {
    Source(SourceUpdate),                 // from workers
    Command(Command),                     // from the UI (ADR-005)
    ActionFinished { id: ActionId },      // from action tasks; triggers a targeted refresh
}
```

Loop: `recv(&cx)` one message, then drain with `try_recv` until empty (at most 256 messages), apply all, then publish once. It never awaits between `latest.store(..)` and `wake_tx.try_send(())`. It owns: all `SeriesRing`s, the process table state, the current `TableQuery`, metadata caches, the device catalog and health. It builds `ProcessTableView` (sort, filter, group, tree flatten) whenever processes or the query change.

Reducer state is bounded: process metadata cache holds live processes plus at most 256 exited ones. Exited records are kept for the linger time, `2 × processes.interval_ms` clamped to 1..=3 s, then removed.

**Shutdown** (`EngineHandle::shutdown(self, total: Duration) -> ShutdownReport`), called on the main thread after GPUI has stopped (ADR-010):
1. Drop the UI-side senders (done by dropping the `EngineHandle` internals).
2. `rt.drain_root_region(Duration::from_millis(1000))`.
3. Drop every `RuntimeHandle` clone.
4. `rt.shutdown_timeout(Duration::from_millis(500))`.
5. If step 2 returned `TimedOut` or step 4 returned `false`, log a warning and return; `tl-app` then calls `std::process::exit(0)` without running further destructors.

### Consequences

- A crash-looping source affects only its own restart budget.
- At most 4 blocking jobs run at once; with one job per worker, a stalled source holds at most one thread.
- The reducer is a single point of failure by design; a panic there restarts the whole session with visible gaps.

### Rejected alternatives

- **One supervisor for all workers.** Rejected: shared restart budget (ASX section 3).
- **Workers writing the ArcSwap directly.** Rejected: lost updates between read-modify-store publishers (Astra section 2).
- **A watch channel per source into the reducer.** Rejected: the reducer would need `select!`, which is banned in 0.5.0.
- **`worker_threads` above 1.** Rejected: the async side only routes messages; parallel work happens on the blocking pool.

### Verification

- LabRuntime tests (ADR-013): restart timings 0.5, 1, 2, 4 s in virtual time; budget exhausted after 5 failures; source disabled for 60 s then restarted; no generation starts after cancellation; `run_until_quiescent_with_report().lab_test_passed()`.
- A test injects a reducer panic and checks: new epoch, the same UI receiver still works, every series has `GAP_BEFORE` on its first new sample.
- `xtask check-deps` banned-API grep passes.
- Real-system test: `shutdown` returns within 1.5 s with all sources running.

---

## ADR-005: Engine and UI bridge, commands and action results

### Context

PLAN.md and Astra agree on `ArcSwap<Snapshot>` plus a one-token wake channel. Astra adds: store before signal, receive before load, never await the wake send, compare component versions against the last consumed snapshot, and keep action results out of the replaceable snapshot. ASX shows asupersync senders work without a `Cx` from any thread. D-001 forbids asupersync types outside `tl-engine`, so the UI needs a runtime-neutral interface.

### Decision

**The client trait** (`tl-model/src/client.rs`). `tl-ui` uses only this.

```rust
pub trait EngineClient: Send + Sync + 'static {
    /// Latest published snapshot. Cheap: one ArcSwap load_full.
    fn latest(&self) -> Arc<Snapshot>;
    /// One-token wake channel. Exactly one UI task calls recv on it.
    fn wake(&self) -> async_channel::Receiver<()>;
    /// Ordered action events. Exactly one UI task calls recv on it.
    fn action_events(&self) -> async_channel::Receiver<ActionEvent>;
    /// Replace demand. Never blocks. Last value wins.
    fn set_demand(&self, demand: Demand);
    /// Replace engine settings. Never blocks. Last value wins.
    fn set_settings(&self, settings: EngineSettings);
    /// Queue a command. Never blocks. Err(Busy) when the queue is full.
    fn send(&self, cmd: Command) -> Result<(), CommandError>;
}

pub enum CommandError { Busy, EngineStopped }
```

`tl-engine::EngineHandle` implements it. `tl-ui` tests use `tl-ui/src/testing/fake_engine.rs`.

**Engine to UI, latest state.**

```rust
// inside tl-engine
latest: Arc<ArcSwap<Snapshot>>,
wake_tx: async_channel::Sender<()>,      // async_channel::bounded(1)

// publish, in the reducer, with no await between the two lines:
latest.store(Arc::new(next));
match wake_tx.try_send(()) {
    Ok(()) | Err(TrySendError::Full(())) => {}
    Err(TrySendError::Closed(())) => stop_publishing = true,
}
```

**UI receiver** (`tl-ui/src/hub.rs`, entity `SnapshotHub`, one per app):

```rust
cx.spawn(async move |this, cx| {
    while wake.recv().await.is_ok() {
        let snap = client.latest();                    // load after receive
        let alive = this.update(cx, |hub, cx| hub.install(snap, cx)).is_ok();
        if !alive { break; }
    }
}).detach();
```

`install`: ignore if `snap.sequence <= current.sequence`; otherwise compute `ComponentMask` by comparing every component's `header.version` (and `SeriesSet` per-series versions) with the **currently installed** snapshot, replace `current`, then `cx.emit(SnapshotChanged { mask, changed_series: SmallVec<[SeriesId; 64]> })`. Views subscribe and call `cx.notify()` on themselves only if a component they draw changed. The hub never calls `notify` on views directly.

Retention: the UI holds at most two snapshots: `SnapshotHub.current` and one paused snapshot for Ctrl-pause (ADR-009). No other code stores `Arc<Snapshot>` beyond one render.

**UI to engine.**

| Path | Channel inside tl-engine | UI call | Semantics |
|---|---|---|---|
| Demand | `asupersync::channel::watch::channel(Demand::default())` | `set_demand` uses `watch::Sender::send` | latest wins; workers read per wake |
| Engine settings | `watch::channel(EngineSettings::default())` | `set_settings` | latest wins |
| Commands | `mpsc::channel::<EngineMsg>(256)` shared with workers | `send` uses `mpsc::Sender::try_send` | `Full` maps to `CommandError::Busy` |

The UI sends demand at most every 100 ms and only when it differs from the last value sent (ADR-008).

```rust
pub enum Command {
    SetTableQuery(TableQuery),
    Action(ActionRequest),
    RefreshNow(SourceId),         // user pressed refresh, or a page opened
    ResetBaselines,               // debugging aid, also used after resume
    KWin(KWinCommand),            // P2: activate, close, keep above, pick (ADR-004 KWin worker)
}
```

**Engine to UI, action events.** `async_channel::bounded::<ActionEvent>(256)`. Action tasks call `events_tx.send(ev).await` wrapped in `time::timeout(.., 1 s)`; a timeout there is logged and the event is dropped (the UI is not draining, which only happens during shutdown). Admission limits in-flight actions to 16, and each action emits at most 2 + number of targets events, with targets capped at 256 per request, so the channel only fills if the UI stops reading.

**Action request and result types** (`tl-model/src/actions.rs`):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ActionId(pub u64);  // allocated by the UI: AtomicU64 starting at 1

pub struct ActionRequest {
    pub id: ActionId,
    pub kind: ActionKind,
    pub targets: SmallVec<[ActionTarget; 1]>,  // max 256, captured at menu-open time
    pub privileged: bool,                       // user chose "Retry as administrator" (P3)
}

pub enum ActionTarget {
    Process(ProcessKey),
    Cgroup(CgroupRef),
    Unit(UnitRef),
    Startup(StartupRef),
}

pub enum ActionKind {
    // ADR-006
    Signal(Signal),            // End Task = Signal(Term), End Process = Signal(Kill)
    KillTree,                  // End Process Tree: SIGSTOP every target, then SIGKILL every target
    Suspend, Resume,           // SIGSTOP / SIGCONT on one process
    SetNice(i8),               // -20..=19
    SetSched { policy: SchedPolicy, rt_prio: u8 },  // P2: Custom, Realtime and Idle priority
    SetAffinity(CpuSet),
    SetIoPriority(IoPriority),
    FreezeGroup, ThawGroup, KillGroup,
    // services and startup
    UnitStart, UnitStop, UnitRestart, UnitEnable, UnitDisable,
    StartupEnable, StartupDisable,
}

pub enum Signal { Term, Kill, Hup, Int, Quit, Stop, Cont, Usr1, Usr2, Other(u8) }  // Other: any other number, incl. SIGRTMIN..=SIGRTMAX
pub enum SchedPolicy { Other, Batch, Idle, Fifo, Rr }   // rt_prio 1..=99 for Fifo and Rr, else 0
pub struct CpuSet(pub [u64; 16]);      // up to 1024 CPUs
pub enum IoPriority { Realtime(u8), BestEffort(u8), Idle }   // levels 0..=7

pub enum ActionState {
    Rejected(RejectReason),    // never started, nothing changed
    Started,                   // admitted and running
    Succeeded,
    Failed(ActionError),       // the OS or service reported failure; nothing changed
    OutcomeUnknown(Arc<str>),  // timed out or target vanished after the effect may have happened
}

pub enum RejectReason { Busy, TooManyTargets, IdentityChanged, KernelThread, Protected, NotOwner, Unsupported }

pub struct ActionEvent {
    pub id: ActionId,
    pub target: Option<u16>,   // index into the request's targets; None for the whole request
    pub state: ActionState,
}
```

Event order per request: `Started` (or a single `Rejected`), then one final state per target, then one final state with `target: None` summarizing (Succeeded only if every target succeeded; otherwise the worst state in the order Failed, OutcomeUnknown, Rejected).

**UI handling.** `ActionTracker` (entity in `tl-ui/src/actions.rs`) keeps `FxHashMap<ActionId, PendingAction>` and shows toasts with `Notification::...id1::<ActionToast>(id.0)` so updates replace the same toast. Pending entries older than the action's deadline plus 5 s become "Outcome unknown". Destructive actions are never retried automatically.

### Consequences

- A slow UI never slows collection; dropped snapshots lose nothing because history lives in engine rings.
- Action results cannot be lost by snapshot coalescing.
- The UI crate compiles and tests without the engine.

### Rejected alternatives

- **Unbounded wake channel.** Rejected: piles up obsolete wakes.
- **Awaited bounded send for wakes.** Rejected: collection would wait for frames.
- **A dirty bitmask in the snapshot.** Rejected: coalescing hides earlier changes (Astra section 2).
- **asupersync `watch` for engine to UI.** Rejected for now: the UI would need asupersync types, against D-001.

### Verification

- Unit test with a fake publisher thread: 10,000 publishes, the UI-side loop (run on a plain executor) ends on the final sequence and never sees a sequence go backwards.
- LabRuntime test: reducer publishes while the wake receiver is never read; publishes never block; memory stays bounded (one snapshot in the slot).
- `tl-ui` test: `SnapshotChanged` with only CPU series changed does not notify `ProcessTable` (render counter unchanged).
- Action tests in ADR-006.

---

## ADR-006: Process actions and privilege boundary

### Context

PLAN.md claimed pidfds prevent ever hitting a reused PID. Astra notes this holds for signals but not automatically for nice, affinity or ioprio, which take a PID. Astra also requires captured identities, per-target outcomes, validated cgroup ownership and no automatic retries. Mission Center retries failed signals with `pkexec kill`, which tasklens does not copy.

### Decision

**Where code lives.** All OS calls are synchronous functions in `tl-ops` (`src/process.rs`, `src/cgroup.rs`, `src/sys.rs` for raw syscalls). `tl-engine/src/actions/` admits requests, runs each in its own task, calls `tl-ops` inside `cx.spawn_blocking`, and applies `time::timeout`. The P3 `tl-helper` calls the same `tl-ops` functions as root.

**Admission** (in the reducer, on `Command::Action`):
1. More than 16 actions in flight: `Rejected(Busy)`.
2. More than 256 targets: `Rejected(TooManyTargets)`.
3. For each process target: `boot_id` differs from the current boot, or the key is not in the current process table and not exited within the linger time (ADR-004): per-target `Rejected(IdentityChanged)`.
4. `KERNEL_THREAD` flag: `Rejected(KernelThread)`.
5. Otherwise spawn the action task in the session's actions region.

**Identity revalidation, every process operation:**

```rust
// tl-ops::process
pub fn open_verified(key: ProcessKey, boot: BootId) -> Result<OwnedFd, OpsError> {
    if key.boot_id != boot { return Err(OpsError::IdentityChanged); }
    let pidfd = rustix::process::pidfd_open(Pid::from_raw(key.pid)?, PidfdFlags::empty())?;
    // The pidfd now pins this process. Re-read start time AFTER opening.
    let start = read_start_ticks(key.pid)?;          // /proc/<pid>/stat field 22
    if start != key.start_ticks { return Err(OpsError::IdentityChanged); }
    Ok(pidfd)
}
```

**Signals.** `rustix::process::pidfd_send_signal(&pidfd, sig)` on the verified pidfd. This cannot hit a reused PID.

**PID-based syscalls (nice, affinity, ioprio).** Rule: open and verify the pidfd, make the syscall by PID, then poll the pidfd with timeout 0.
- pidfd not readable (process still alive after the call): the call hit the target, because a PID cannot be reused while its process is alive. Report the syscall's result.
- pidfd readable (process exited during the call): report `OutcomeUnknown("process exited during the change")`.

Details:
- `SetNice(n)`: `setpriority(PRIO_PROCESS, pid, n)` applies to the thread group leader only on Linux; to change the whole process, apply to every TID from `/proc/<pid>/task/` with the same before/after pidfd check on the main PID. Report `Succeeded` only if every TID succeeded; partial is `Failed(Partial { done, total })`.
- `SetSched { policy, rt_prio }` (P2): `sched_setattr` on every TID, same rules. Realtime sets `SCHED_RR` priority 1 with `SCHED_RESET_ON_FORK`, after the confirmation "Realtime priority can freeze the desktop if the process spins. Set it anyway?".
- `SetAffinity(set)`: `sched_setaffinity` on every TID, same rules.
- `SetIoPriority(p)`: `ioprio_set(IOPRIO_WHO_PROCESS, tid, value)` through `libc::syscall(SYS_ioprio_set, ..)` in `tl-ops/src/sys.rs`, on every TID, same rules. `Realtime` needs root and is shown only in the privileged menu.
- TIDs are re-listed right before the loop. A TID that disappears mid-loop is skipped and not counted as failure.
- `EPERM` maps to `ActionError::PermissionDenied`. The UI shows the toast "Permission denied" with the button "Retry as administrator" from P3; before P3 the toast has no button. Raising priority (lower nice) is expected to fail without `CAP_SYS_NICE`.

**Suspend and resume one process:** `Signal(Stop)` and `Signal(Cont)` through the pidfd.

**Tree operations ("End Process Tree", `ActionKind::KillTree`).** The UI captures the subtree's keys when the menu opens (max 256). The engine opens a verified pidfd for every captured target, sends SIGSTOP to all of them, then SIGKILL to all of them, then closes the fds. Stopping first means no process in the tree can fork a new child between the two passes. One per-target event each. A target that fails `open_verified` is skipped with `Rejected(IdentityChanged)`.

**cgroup operations** (`FreezeGroup`, `ThawGroup`, `KillGroup` on an app group):

```rust
pub struct CgroupRef {
    pub rel_path: Arc<str>,  // relative to /sys/fs/cgroup
    pub cgroup_id: u64,      // inode of the cgroup directory when captured
}
```

Validation in `tl-ops::cgroup::open_validated(cg, uid)`:
1. `rel_path` starts with `user.slice/user-{uid}.slice/user@{uid}.service/app.slice/` and the last component ends in `.scope` or `.service`. `uid` comes from `getuid()` at runtime; `1000` is never hard-coded.
2. Open the directory with `O_DIRECTORY | O_CLOEXEC` from a dirfd on `/sys/fs/cgroup`, `fstat` it: inode equals `cgroup_id` and owner uid equals `uid`.
3. The tasklens process's own cgroup is not inside it (`Rejected(Protected)`).
4. Write with `openat(dirfd, "cgroup.freeze" | "cgroup.kill")`: `"1"` to freeze, `"0"` to thaw, `"1"` to kill.
5. Freeze completion: poll `cgroup.events` for `frozen 1` every 50 ms for up to 2 s; not frozen by then is `OutcomeUnknown("freeze still in progress")`.

**Protected processes.** `PROTECTED` is set on: PID 1, the current session's `kwin_wayland`, `plasmashell`, `ksmserver`, `Xwayland`, `sddm`, `systemd-logind`, the session's `login` ancestor, the user's `systemd --user`, the user's `dbus-broker` or `dbus-daemon`, and tasklens itself.

Refused with a toast and no dialog (`Rejected(Protected)` or `Rejected(KernelThread)`):
- tasklens itself: "Refusing to end tasklens itself."
- PID 1: "PID 1 (systemd) cannot be ended."
- kernel threads: "Kernel threads cannot be signalled."

A confirmation `AlertDialog` opens for:
- any signal, freeze or kill on a `PROTECTED` target: "Ending <name> will end your desktop session. End it anyway?";
- End Process Tree: "End N processes in this tree?", buttons "End tree" and "Cancel";
- Kill App: "Kill all N processes of <App>?";
- End Task or End Process with more than one target;
- any action on another user's process.

A single End Process (SIGKILL) on one of the user's own, unprotected processes does not ask; it shows only the toast "End process requested." (D-009).

**Deadlines** (`time::timeout` around each target's blocking job):

| Action | Deadline |
|---|---|
| Signal, nice, affinity, ioprio, freeze write | 2 s per target |
| Freeze completion wait | 2 s (above) |
| systemd unit start/stop/restart | 30 s until `JobRemoved` for the returned job path |
| systemd enable/disable + `Reload` | 10 s |
| Any call that may show a polkit prompt | 120 s |
| Startup entry write | 2 s |

A timeout reports `OutcomeUnknown`, never `Failed`. Work already running on the blocking pool is not interrupted (ASX section 6).

**systemd units.** Methods `StartUnit(name, "replace")`, `StopUnit(name, "replace")`, `RestartUnit(name, "replace")`, `EnableUnitFiles([name], false, true)` / `DisableUnitFiles([name], false)` then `Reload()`. Calls set the D-Bus `ALLOW_INTERACTIVE_AUTHORIZATION` flag so polkit can ask for a password. Success is the `JobRemoved` signal with result `"done"`; any other result string is `Failed(UnitJob(result))`.

**Privileged helper boundary (P3, reserved now).**
- Binary `/usr/lib/tasklens/tl-helper`, started by the engine as `pkexec /usr/lib/tasklens/tl-helper --session`.
- Polkit has exactly three tasklens actions, in `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy`: `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`) covers every tl-helper operation; `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`) covers `/usr/lib/tasklens/tasklens-setup`; `io.github.oysteinkrog.tasklens.netmon` is reserved for `tl-netmon` (P4). Polkit authorises the `pkexec` launch once, so per-operation action names inside a running helper would not be enforced.
- Transport: the helper's stdin and stdout, using the same `tl-model::wire` framing as the GPU probe (ADR-007), max frame 64 KiB. A reply larger than one frame is sent as a sequence of frames; the total per request is capped at 4 MiB. There is no single-call `--once` mode.
- Allowed requests only:
  - `Signal { key, signal }`, `SetNice`, `SetSched`, `SetAffinity`, `SetIoPriority`;
  - `FreezeGroup`, `ThawGroup`, `KillGroup` for any uid's cgroup under `/sys/fs/cgroup`;
  - `ReadProcessDetail { key, what }` for root-only `/proc` fields;
  - `ReadProcessFiles { key, files }`, where `files` is a set of `io`, `fd_list`, `fdinfo`, `environ`, `maps`, `smaps_rollup`, `ns`, `cwd`, `exe`, `status`;
  - `ScanStrings { key, opts }`, which streams batches;
  - `Dump { key, mode, out_fd }`, which runs `/usr/bin/gcore` by fixed path and writes to the passed fd;
  - `ResolveSocketInodes { inodes }`;
  - `SockDestroy { family, proto, four_tuple, cookie }`;
  - `WriteServiceDropIn { manager: System, unit, keys }` with keys limited to `Restart*`, `StartLimit*`, `FailureAction` and `OnFailure`, written to the fixed path `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`;
  - `NetnsSockDiagDump { netns_ino }`.
- No path, command or free-form file argument. The helper re-runs `open_verified` itself on every process target; it never trusts the engine's check.
- The helper exits after 300 s idle or when stdin reaches EOF. It sets `PR_SET_PDEATHSIG = SIGKILL`.
- The eBPF network collector is a separate binary (`tl-netmon`, P4) with its own polkit action. It never shares a process with the action helper.
- "Run as administrator" in the Run dialog uses `pkexec env … /bin/sh -c <text>` under `org.freedesktop.policykit.exec`, never tl-helper.
- Without the helper, privileged menu items are shown disabled with the tooltip "Needs administrator helper (not installed)".

### Consequences

- Signals are race-free. PID-based changes are race-free when they report Succeeded or Failed, and report OutcomeUnknown otherwise.
- No automatic privilege escalation ever happens.

### Rejected alternatives

- **`kill(pid)` without pidfd.** Rejected: PID reuse.
- **Mission Center's automatic `pkexec kill` retry.** Rejected: silent escalation and retrying destructive actions.
- **One helper for actions and eBPF.** Rejected per Astra risk 8.

### Verification

- Integration test (real system, `tests/ops_real.rs` in `tl-ops`): spawn `sleep 60` and capture its key. Waiting for real PID reuse is impractical, so change the captured `start_ticks` by one and check that `open_verified` returns `IdentityChanged`. The unchanged key must open and signal the process.
- Integration test: spawn a child, `SetNice(5)` succeeds and `/proc/<pid>/stat` shows nice 5 for every TID; spawn a child that exits immediately and check the result is `Rejected(IdentityChanged)` or `OutcomeUnknown`, never `Succeeded`.
- Integration test: start `systemd-run --user --scope sleep 60`, freeze, read `cgroup.events` = `frozen 1`, thaw, kill.
- Unit test: `open_validated` rejects `session-2.scope`, a path with `..`, another uid's slice and a mismatched inode.
- UI test: right-click row A, the table re-sorts so row A moves, choose End: the request targets A's key.
- Integration test: spawn a parent with two children, run `KillTree`: every target receives SIGSTOP before any receives SIGKILL, and no process of the tree survives.

---

## ADR-007: GPU probe child process protocol

### Context

Astra's critical correction: a segfault or abort inside NVML kills the whole process, and a hung NVML call cannot be cancelled. NVML therefore runs only in `tl-gpu-probe`. DRM fdinfo is empty on this machine with driver 615, so NVML is the only per-process GPU source for NVIDIA here.

### Decision

**Process.** Binary `tl-gpu-probe`, synchronous, no async runtime, depends on `nvml-wrapper =0.13.0`. Located by, in order: `$TASKLENS_GPU_PROBE`, the directory of `std::env::current_exe()`, `/usr/lib/tasklens/tl-gpu-probe`. Not found: `GpuNvidia` health is `Unsupported` with message "GPU probe not installed".

**Spawn** (`tl-engine/src/gpu/spawn.rs`, inside `spawn_blocking`):
1. `rustix::net::socketpair(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::CLOEXEC, None)`.
2. `std::process::Command::new(probe).arg("--fd").arg("3")`, `stdin/stdout = Stdio::null()`, `stderr = Stdio::inherit()`, `env_clear()` then only `PATH`, `LANG`, `RUST_LOG` from `TASKLENS_LOG`.
3. `pre_exec` (unsafe, this file only): `dup2(child_end, 3)` (clears CLOEXEC on fd 3) and `rustix::process::set_parent_process_death_signal(Some(Signal::Kill))`.
4. Keep the `std::process::Child`. Never use `asupersync::process` (zombie bug `bi2462.114`).

**Framing** (`tl-model/src/wire.rs`, shared with `tl-helper`):

```text
frame = u32 little-endian payload length | payload (postcard)
max payload = 1_048_576 bytes (1 MiB) for the probe; 65_536 for the helper
```

A frame longer than the max, a short read or a postcard error is a protocol failure. Helper replies may span several frames, up to 4 MiB per request (ADR-006).

**Messages** (`tl-model/src/gpu_proto.rs`):

```rust
pub const GPU_PROTO_VERSION: u16 = 1;

pub enum ProbeRequest {
    Hello { proto: u16 },
    Sample { seq: u32, processes: bool },
    Shutdown,
}

pub enum ProbeResponse {
    Hello { proto: u16, driver: String, nvml: String, devices: Vec<GpuDeviceInfo> },
    InitFailed { kind: InitFailure, message: String },
    Sample { seq: u32, t_ns: u64, devices: Vec<GpuDeviceSample>, processes: Option<Vec<GpuProcessSample>> },
}

pub enum InitFailure { LibraryNotFound, DriverNotLoaded, NoPermission, Other }

pub struct GpuDeviceInfo { pub uuid: [u8; 16], pub name: String, pub pci_bus_id: String,
                           pub mem_total: u64, pub power_limit_mw: Option<u32> }

pub enum Reading<T> { Ok(T), Unsupported, Failed }   // never encode missing as zero

pub struct GpuDeviceSample {
    pub uuid: [u8; 16],
    pub util_pct: Reading<u32>, pub mem_util_pct: Reading<u32>,
    pub mem_used: Reading<u64>,
    pub enc_pct: Reading<u32>, pub dec_pct: Reading<u32>,
    pub temp_c: Reading<u32>, pub power_mw: Reading<u32>,
    pub clock_gfx_mhz: Reading<u32>, pub clock_mem_mhz: Reading<u32>,
    pub fan_pct: Reading<u32>,
    pub pcie_rx_kbps: Reading<u32>, pub pcie_tx_kbps: Reading<u32>,
}

pub struct GpuProcessSample {
    pub uuid: [u8; 16], pub pid: u32,
    pub sm_pct: Reading<u32>, pub mem_pct: Reading<u32>,
    pub enc_pct: Reading<u32>, pub dec_pct: Reading<u32>,
    pub mem_used: Reading<u64>,
}
```

The probe gets per-process data from `process_utilization_stats(last_seen_timestamp)` plus `running_compute_processes()` and `running_graphics_processes()` for memory. The engine maps `pid` to `ProcessKey` using the current process table; a pid not in the table is dropped. GPU utilization is never divided among processes as a substitute.

**Request rules.**
- Exactly one outstanding request. The engine writes a request, then waits with `rustix::event::poll` on the socket for readability, then reads one frame.
- Deadlines: `Hello` 3000 ms (NVML init); `Sample` 500 ms.
- The probe exits 0 on `Shutdown`, on EOF on fd 3, or on any protocol error.
- Protocol version mismatch in `Hello`: health `Failed("probe version mismatch")`, no restart until the app restarts.

**Failure and restart.**

| Event | Engine action | GPU data state |
|---|---|---|
| `Sample` deadline missed | send SIGKILL to the child, `wait()` it in the blocking job, return `Err` (supervisor restarts with backoff) | `Stale`, then `Failed` |
| Child exit or crash (EOF, `waitpid` status) | reap, return `Err` | `Failed` |
| `InitFailed { LibraryNotFound | DriverNotLoaded }` | worker stays up, retries `Hello` with a new child every 300 s | `Missing` |
| `InitFailed { NoPermission | Other }` | return `Err` (normal backoff) | `Failed` |
| Per-metric `Unsupported` | none | that field `Unsupported` |

Restart backoff and the five-per-minute rule come from ADR-004. A child stuck in uninterruptible kernel sleep cannot be reaped; the blocking job then waits in `wait()`, the worker's health shows `Late`, and the rest of the app is unaffected. This is a known limitation.

**Rates.** Device totals at 1 Hz while any GPU view, sidebar mini graph or tray graph is demanded; process data at 1 Hz only when a GPU process column is visible. Never faster than 1 Hz; the Refresh setting does not change the probe rate.

**Shutdown.** The worker sends `Shutdown`, waits 200 ms for exit, then SIGKILL and `wait()`.

### Consequences

- An NVML crash or hang after a driver update cannot take down the UI.
- Two context switches and about 2 KB of messages per second.

### Rejected alternatives

- **NVML in-process.** Rejected: native crashes and uninterruptible calls.
- **SOCK_SEQPACKET instead of framing.** Not chosen: the explicit framing is shared with the helper over stdin/stdout, which are streams.
- **Raw dlopen of libnvidia-ml.** Rejected: `nvml-wrapper` already does it with typed errors.

### Verification

- Unit tests for framing: max size, oversize rejected, truncated frame rejected, postcard round-trip of every message.
- Fake probe binary `tl-gpu-probe --fake <script>` (test feature `fake` in the probe crate) that replies normally, sleeps 2 s, crashes with SIGSEGV, or sends an oversize frame. Engine integration tests check: stale then restart; backoff 0.5 s then 1 s; no zombie remains (`/proc/<child>/stat` absent after reaping).
- Real-system test on the owner machine: `Hello` returns one device whose name contains "5070".

---

## ADR-008: Sampling schedule, counters, clocks and suspend

### Context

PLAN.md polled all system sources at 10 Hz. Astra showed smooth graphs do not need fast polling, and gave a per-source table with visible and hidden policies. Demand must be aggregated across windows, detached graphs, columns and recording, and must use process identities, not row numbers.

### Decision

**Demand type** (`tl-model/src/demand.rs`), built by the UI's `DemandAggregator` (ADR-009):

```rust
#[derive(Clone, PartialEq, Default)]
pub struct Demand {
    pub any_surface_visible: bool,
    pub cpu: CpuDemand,             // Off | Total | PerCore  (PerCore implies Total)
    pub cpu_fast: bool,             // a visible CPU chart is animating: use 10 Hz
    pub memory: bool, pub disks: bool, pub nets: bool, pub gpus: bool, pub sensors: bool,
    pub process_table: bool,
    pub process_columns: ColumnSet, // bitset of ColumnId
    pub detail_keys: Arc<[ProcessKey]>, // visible rows plus one viewport of overscan, max 512
    pub gpu_process: bool,
    pub connections: bool, pub services: bool, pub startup: bool, pub inventory: bool,
    pub tray: TrayDemand,           // P3
    pub recording: bool,            // history recording enabled (P3)
}
```

**Schedule.** "Visible" means some visible surface (window page, sidebar mini graph, detached graph, tray icon) displays it.

| Source | Visible rate | Not visible, recording off | Not visible, recording on |
|---|---|---|---|
| CPU total and per core | 10 Hz if `cpu_fast`, else 1 Hz | off | 1 Hz |
| CPU frequency, CPU temperature | 1 Hz | off | 1 Hz |
| Memory, swap, zram | 1 Hz | off | 1 Hz |
| Disks, network interfaces | 1 Hz | off | 1 Hz |
| GPU totals (NVML) | 1 Hz | off | 1 Hz |
| Sensors (hwmon), fans | 1 Hz | off | off |
| Process sweep (CPU, memory, I/O, state) | setting `processes.interval_ms`, default 1000, range 250..=10000 | off | same as visible rate |
| PSS/USS for `detail_keys` | 1 Hz, only if a PSS or USS column is shown | off | off |
| GPU per process | 1 Hz, only if a GPU process column is shown | off | 1 Hz if recording includes GPU |
| Connections and socket attribution | every 2 s | off | off |
| systemd units | event-driven; full resync on connect and every 60 s while the page is visible | events only, no resync | same |
| Startup entries, inventory | on page open and on `RefreshNow` | off | off |
| Logind sleep signal | event-driven, always on | always on | always on |
| Device enumeration (disks, nets, hwmon) | every 5 s while any of those is demanded | off | every 5 s |

While recording is off, the process sweeps that run because the Processes tab is open also feed the 10-minute rolling window (ADR-012, D-010). The window adds no sweeps of its own.

PSS/USS sort and GPU-per-process sort over partial coverage are disabled: the header for PSS and USS is not clickable and shows the tooltip "Only measured for visible rows". `ProcessSnapshot.coverage` lists which keys have PSS/USS so the UI shows `WarmingUp` for others.

**Hidden detection.** `any_surface_visible = false` when the main window has had no render for 1000 ms while the hub had a pending notify (ADR-010 explains), and no detached graph or tray icon is shown. Hidden with recording off means every periodic source is off; only logind and the D-Bus executor ticks remain.

**Tick semantics.**
- Deadlines are absolute: `t0 + n × period` from asupersync `interval`, `MissedTickBehavior::Skip`. No catch-up bursts.
- Every sample records `t_ns` (CLOCK_BOOTTIME) read immediately before the read syscalls, and the process sweep records `observed_start` and `observed_end`.
- Rates use actual elapsed time: `rate = (cur - prev) / ((t_cur - t_prev) as f64 / 1e9)`.

**Counter rules.**
- Cumulative counters are `u64`. If `cur < prev`, treat it as a reset: no rate for this interval (`WarmingUp`), set `RESET`, new baseline.
- Never subtract across a process identity change, a device generation change, an engine epoch or a suspend.
- The first sample after start, restart, rate change or reset gives `WarmingUp`.

**CPU math** (`/proc/stat` fields user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice):
- `busy = user + nice + system + irq + softirq + steal`; `total = busy + idle + iowait`.
- `cpu_pct = 100 × Δbusy / Δtotal`; `kernel_pct = 100 × Δ(system + irq + softirq) / Δtotal`.
- `guest` and `guest_nice` are ignored (already inside `user` and `nice`). `iowait` counts as idle and is reported separately in the CPU details.
- Process CPU: column `cpu` is `100 × Δ(utime + stime) / ticks_per_sec / Δt_s / online_cpus` (whole machine). Column `cpu_relative`, labelled "CPU (one core = 100%)", drops the `/ online_cpus`. Both columns are computed from the same sweep; there is no setting.
- CPU hotplug: a change in the set of `cpuN` lines re-enumerates CPU devices; an offline CPU shows `Missing`.

**Memory math.** `used = MemTotal - MemAvailable`. Commit percent = `Committed_AS / CommitLimit` and may exceed 100%. The default process Memory column (id `memory`, header "Memory") is RssAnon from `/proc/<pid>/status` (D-004), with the header tooltip "Private memory (RssAnon). Shared and file-backed pages are not counted." App and unit rows use cgroup `memory.stat` `anon`. RSS (`ws_total`, VmRSS), PSS, USS and swap are extra columns.

**Stale threshold.** A value is `Stale` when `now - t_last > max(3 × period, 1.5 s)`.

**Suspend and resume.** Two signals, whichever comes first:
1. logind `PrepareForSleep(true)`: the reducer pauses publication of rates; `PrepareForSleep(false)`: the reducer sends `ResetBaselines` to every worker, re-enumerates devices, and marks `GAP_BEFORE` on every series.
2. Clock check in every worker: if `(Δboot_ns - Δmono_ns) > 250 ms` between two samples, the source treats it as a resume: drop the rate, reset baselines, set `GAP_BEFORE`.

After resume the GPU probe is not restarted unless it fails a request.

### Consequences

- Idle cost with the window hidden is a few timer wakeups per second.
- Graphs for 1 Hz sources move smoothly because the chart interpolates in time (ADR-009), not because of fast polling.

### Rejected alternatives

- **10 Hz for every system source.** Rejected: CPU cost with no visible benefit (Astra section 3).
- **Lowering the process rate automatically when slow.** Rejected per Astra: report `Late` instead.

### Verification

- LabRuntime tests with fake sources: demand switches change the observed sample times exactly (10 Hz to 1 Hz to off) within one `IDLE_RECHECK`; hidden with recording off produces zero `sample()` calls over 60 virtual seconds.
- Parser tests: counter going backwards yields `WarmingUp` and `RESET`.
- Unit test: a sample pair with a 5 s boottime gap and 10 ms monotonic gap gives a gap and no rate.
- Real-system test: `systemctl suspend` (manual, owner machine) leaves a visible gap and no spike after resume.

---

## ADR-009: UI architecture

### Context

GPUI and KIT notes: `DataTable` has single, index-based selection, no tree rows, keyboard quirks, row heights fixed at 26/30/32/40 px, and rebuilds every visible cell every frame. Kit charts clone data and re-shape labels every frame. Cached GPUI paths are cloned and translated each paint; each path batch draws into a 4x MSAA intermediate texture. `request_animation_frame` re-renders the current view. `Entity::cached` replays prepaint and paint until notified. Variable fonts render at regular weight and fonts without an `m` glyph are dropped.

### Decision

**Entity structure** (`crates/tl-ui/src/`):

```text
app.rs            TasklensUi::init(cx, client: Arc<dyn EngineClient>, settings) and window opening
hub.rs            SnapshotHub (Entity, one per app)
demand.rs         DemandAggregator (Entity, one per app)
anim.rs           AnimationDriver (Entity, one per window)
actions.rs        ActionTracker (Entity, one per app)
theme/            TlTheme, built-in themes, mapping onto gpui-kit Theme
window/main.rs    MainWindow (Entity): title bar, tab strip, active page, status bar
pages/performance/{mod.rs, sidebar.rs, cpu.rs, memory.rs, gpu.rs, disk.rs, net.rs, sensors.rs}
pages/processes/{mod.rs, table.rs, header.rs, filter.rs, menu.rs}
pages/{services.rs, startup.rs, connections.rs, information.rs}
chart/{element.rs, view.rs, geometry.rs, grid.rs, scale.rs}
table/{tree_table.rs, columns.rs, selection.rs, cells.rs}
text_cache.rs
testing/fake_engine.rs
```

- Six tabs, in TaskSlinger's order and with these labels: Processes, Performance, Services, Startup Apps, Connections, Information. History is a mode of the Processes tab, not a tab; the tab label reads "History" while it is on. There are no Users, Details or App history tabs. A tab that is not built yet is not shown.
- Page entities are created on first visit and kept. Only the active page renders (`match` in `MainWindow::render`).
- Every page, sidebar, detail pane, header and status bar is embedded with `.cached(StyleRefinement::default().size_full())` (or its fixed size). The tree-table body is not cached (it owns scrolling).
- Views subscribe to `SnapshotHub` and notify themselves only for components they draw. Nothing calls `notify` inside `render`. Entities, subscriptions and focus handles are created in constructors only.

**Chart element.**

```rust
pub struct ChartSpec {
    pub series: SmallVec<[ChartSeries; 2]>,
    pub y: YScale,
    pub range_ns: u64,           // 30 s, 1 min (default), 5 min, 10 min; 5 and 10 min from P2 read the 1 Hz tier (ADR-003)
    pub grid: GridStyle,
    pub smooth: bool,            // setting motion.smooth_graphs ("Smooth graphs")
}
pub struct ChartSeries { pub id: SeriesId, pub fill: Option<Hsla>, pub line: Hsla, pub line_width: Pixels }
pub enum YScale { Fixed { min: f32, max: f32 }, AutoRate { floor: f32 } }

pub struct ChartGeometryKey {
    pub series_versions: SmallVec<[u64; 2]>,
    pub physical_width: u32, pub physical_height: u32,
    pub scale_bits: u32, pub y_min_bits: u32, pub y_max_bits: u32,
    pub style_version: u64,
}
pub struct ChartCache {
    pub key: ChartGeometryKey,
    pub anchor_ns: u64,                 // newest sample time when built
    pub fill: Option<gpui::Path<Pixels>>,
    pub stroke: Option<gpui::Path<Pixels>>,
}
```

Rules:
1. `ChartView` (Entity) owns `Rc<RefCell<ChartCache>>` and renders a `ChartElement` that implements GPUI `Element` directly (no hitbox, no id, unless a tooltip is enabled).
2. Geometry is rebuilt only when `ChartGeometryKey` changes. Points are straight segments (no splines). Each contiguous run of `VALID` samples is its own subpath; `GAP_BEFORE` or a non-valid sample breaks a run.
3. Decimation: when a run has more than one sample per physical pixel, keep min and max per bucket in time order plus the run's first and last sample. Buckets are aligned to absolute time (`bucket = t_ns / bucket_ns`, `bucket_ns = range_ns / physical_width`), so they do not shimmer as the chart scrolls.
4. Geometry is built in chart-local coordinates with `x = (t_ns - anchor_ns) × px_per_ns`. Each paint clones the path, translates by `dx = chart_right - (right_edge_ns - anchor_ns) × px_per_ns`, and clips with `window.with_content_mask(Some(ContentMask { bounds }))`. Copy the translate approach from gpui-component `crates/base/src/plot/path_cache.rs`.
5. Time mapping: `right_edge_ns = min(now_boot_ns() - period_ns, newest_sample_ns)` when `smooth`; `right_edge_ns = newest_sample_ns` when not. `x(t) = right - (right_edge_ns - t) × width / range_ns`. Numeric readouts always show the newest sample.
6. Fill is a closed polygon down to the baseline per run, drawn with `paint_path` before the stroke. Stroke uses `PathBuilder::stroke(px(1.0))`.
7. **Merging:** charts drawn by one element that share colors merge their fills into one `Path` and their strokes into another. The CPU per-core grid is one `ChartGridElement` drawing all 32 cores: one fill path, one total-line path, one kernel-line path. Sidebar mini graphs are one element per device kind.
8. Grid: hairline quads (`paint_quad(fill(..))`), 1 physical px. Vertical lines every `range_ns / 10`, positioned from absolute time so they scroll with the data; horizontal lines at 25%, 50%, 75%.
9. Y scale: `Fixed` for percentages (0..100). `AutoRate` picks the smallest ceiling above the visible max (a power of two for disk rates, 1-2-5 × 10^n for network rates, per canonical table (d)), rises at once, and falls only after the visible max stays below half the ceiling for 10 s.
10. Labels inside charts are `ShapedLine`s cached in the `ChartView`, re-shaped only when the text changes.

**Animation driver** (one per window, `anim.rs`):

```rust
pub struct AnimationDriver {
    requests: FxHashSet<EntityId>,  // charts that want the next frame
    armed: bool,
    last_frame: Option<Instant>,
    stats: FrameStats,              // frame intervals and UI thread CPU time (ADR-014)
}
```

- A `ChartView` calls `driver.request(entity_id)` during its render only if `smooth` is on, the window is visible, reduce-motion is off, and its newest sample is younger than `2 × period + range/width` (it is moving).
- If not `armed`, the driver calls `window.on_next_frame(..)` once and sets `armed`. In the callback it takes the set, clears `armed`, and calls `cx.notify(id)` for each id. Charts re-request in their render, which re-arms.
- No other code uses `request_animation_frame`, timers or `Animation` for continuous redraws. Kit `Spinner`, `Skeleton` and `Progress` are not used in table cells or charts.
- When nothing requests, no frames are drawn. Static views redraw only on snapshot changes and input.

**Process tree-table** (custom, on GPUI `uniform_list`):

- Why not `DataTable`: index-based single selection, no tree rows, arrow keys switch to column mode, Home/End select columns, right-click does not select, row heights cannot be 28 px, and every visible cell (with a `format!` id) is rebuilt every frame (GPUI section 4).
- The engine builds the rows (`ProcessTableView`); the UI never sorts or filters.

```rust
pub struct TableQuery {
    pub generation: u64,
    pub mode: TableMode,               // Grouped | Tree | Flat
    pub sort: SortSpec,                // column + Asc/Desc; tie-break ProcessKey
    pub filter: Arc<str>,              // lowercase; matches display name, image name, cmdline, user, description,
                                       // app name, unit name, group names; numeric input also matches PID by prefix
    pub expanded: Arc<[NodeId]>,       // UI owns expansion state
}
pub enum NodeId { Group(GroupKey), Process(ProcessKey) }
pub struct TableRow { pub node: NodeId, pub depth: u8, pub expanded: bool, pub has_children: bool,
                      pub record: Option<u32>, pub group: Option<u32> }  // indexes into records/groups
pub struct ProcessTableView {
    pub header: ComponentHeader,
    pub query_generation: u64,
    pub rows: Arc<[TableRow]>,
    pub groups: Arc<[GroupRow]>,       // aggregated values, no double counting of descendants
    pub totals: TableTotals,           // two-line header numbers
}
```

- Group totals sum each process once; in tree mode a parent row shows its own values, and the collapsed parent shows its subtree sum in secondary text.
- Missing parent (PPID not visible or exited): the process is a root. Reparenting moves it at the next sweep.
- Filter: the UI debounces input 100 ms, then sends `SetTableQuery` with a new generation. The UI shows a view only if `query_generation` equals the latest sent; until then it keeps the previous rows.
- Row height: `28 × row_height_pct/100 × ui_scale_pct/100` logical px. Column positions are a precomputed `Arc<[Pixels]>` of x offsets; cells are fixed-width `div`s in one row `div`. Horizontal scroll moves the whole table.
- Header: custom two-line header (total value over column name), resizable and reorderable columns, sort by clicking anywhere on the header cell (Desc first, then Asc), right-click for column chooser.
- Selection (`table/selection.rs`): `FxHashSet<NodeId>` plus `anchor` and `focus` `Option<NodeId>`. Click, Ctrl-click, Shift-click, Ctrl-A, arrow keys, Page Up/Down, Home/End move rows. A `FxHashMap<NodeId, u32>` index is rebuilt when `table.header.version` changes.
- Right-click selects the row if it is not selected, then opens a `PopupMenu` whose actions carry the captured `SmallVec<[ProcessKey; 1]>` (and subtree for tree actions). The menu stays bound to those keys even if rows move.
- Visible demand: the `uniform_list` range callback records the visible range; after each render the table computes the keys of the range plus one viewport above and below (max 512) and, if the set changed, gives it to `DemandAggregator`. This runs after sorting and filtering even if the range did not change.
- Ctrl-pause: while Ctrl is held and the main window is active, whatever has focus inside it, the table renders from `paused: Option<Arc<Snapshot>>` instead of the hub's current snapshot and the status bar shows "[PAUSED - Ctrl]". Release on Ctrl-up or window deactivation. Actions still revalidate identities.
- New and exited highlight: rows with `NEW` get the theme's success tint, `EXITED` the danger tint, for the linger time (`2 × processes.interval_ms`, clamped to 1..=3 s), fading over the last 300 ms unless reduced motion is on. Setting `processes.highlight_new_exited`, label "Highlight new and exited processes", default on.
- Heat colouring: a cell's background blends `ROW` color toward `VALUE_HEAT` by `value / reference`, with a fixed reference per column type: CPU `400 / ncpu` % (12.5% on this machine), CPU (one core = 100%) 400%, memory columns 10% of MemTotal, disk and I/O rates 100 MiB/s, network 10 MiB/s (80 Mbps), GPU 100%, VRAM 25% of total VRAM, power 25 W. Counts are not tinted. Settings `appearance.heat_coloring = true` and `appearance.heat_intensity_pct = 50`.

**Text caching** (`text_cache.rs`):
- `CellTextCache: FxHashMap<(NodeId, ColumnId), (u64 /*value bits*/, Validity, SharedString)>`, max 8192 entries, cleared of keys not seen in the last 3 table versions.
- A cell re-formats only when its value bits or validity change. Names use `SharedString::from(Arc<str>)` without copying.
- Numbers use the tabular-figures font feature and right alignment; numeric columns have fixed widths so changing digits never shift layout.

**Theme model** (`theme/`):

```rust
pub struct TlTheme {
    pub name: Arc<str>, pub mode: ThemeMode, pub version: u64,
    // TaskSlinger's 22 slots
    pub window_bg: Hsla, pub titlebar: Hsla, pub toolbar: Hsla, pub input: Hsla, pub input_focus: Hsla,
    pub header: Hsla, pub list_bg: Hsla, pub row_alt: Hsla, pub row_hover: Hsla, pub row_selected: Hsla,
    pub divider: Hsla, pub menu_bg: Hsla, pub menu_hover: Hsla, pub tab_bg: Hsla, pub tab_hover: Hsla,
    pub scroll_track: Hsla, pub scroll_thumb: Hsla, pub text_primary: Hsla, pub text_secondary: Hsla,
    pub text_selected_primary: Hsla, pub text_selected_secondary: Hsla, pub value_heat: Hsla,
    // not slots in TaskSlinger; part of each theme definition
    pub accent: Hsla,
    pub graph: GraphPalette,   // cpu_fill, cpu_line, kernel_line, mem_fill, mem_line, gpu_fill, gpu_line,
                               // disk_fill, disk_line, net_fill, net_line, grid
}
```

- Built-in themes ship as `assets/themes/builtin.json` in TaskSlinger's format `{"themes":[{"name":..,"SLOT":"#RRGGBB"}]}` plus `accent` and `graph.*` keys. P1 ships "Default Dark" (PLAN.md section 9 palette) and "Default Light", because the `theme.light` default needs it; the other 10 named themes arrive in P2. User themes load from `$XDG_CONFIG_HOME/tasklens/themes.json` in the same format.
- Custom tasklens views (tree-table, charts, sidebar, tab strip, title bar content) read `TlTheme` directly (a GPUI `Global`).
- Kit widgets (menus, dialogs, inputs, tooltips, toasts, buttons, scrollbars) read gpui-kit `Theme`. `theme/map.rs::apply(&TlTheme, &mut gpui_kit Theme)` sets: `background ← window_bg`, `foreground ← text_primary`, `muted_foreground ← text_secondary`, `border ← divider`, `title_bar ← titlebar`, `popover ← menu_bg`, `accent ← menu_hover`, `primary ← accent`, `ring ← accent`, `input ← input`, `scrollbar ← scroll_track`, `scrollbar_thumb ← scroll_thumb`, `table ← list_bg`, `table_even ← row_alt`, `table_hover ← row_hover`, `table_active ← row_selected`, `table_head ← header`, `table_row_border ← divider`, `tab_bar ← tab_bg`, `status_bar ← toolbar`, `chart_1..5 ← graph cpu/mem/gpu/disk/net lines`, `chart_grid ← graph.grid`. The bead that writes `map.rs` must check each token name against `ThemeColor` in gpui-component 0.7.0 source; a missing name maps to the nearest existing token and the mapping is recorded in a comment in `map.rs`.
- Theme switch: `Theme::update` plus replacing the `TlTheme` global, bumping `style_version` so every chart rebuilds geometry once.

**Metrics and scale.** Settings `appearance.ui_scale_pct` (75..=200, step 5, default 100) and `appearance.row_height_pct` (80..=150, step 5, default 100). With `ui_scale = ui_scale_pct / 100`: `UiMetrics { ui_scale, row_height: 28 × row_height_pct/100 × ui_scale, base_text: 13 × ui_scale, radius: 5 × ui_scale }`. The theme's `font_size` (rem) is `base_text`. GPUI applies the compositor scale (1.1 here); tasklens multiplies by `ui_scale` once. Ctrl+Plus, Ctrl+Minus and Ctrl+0 step the scale by 5 or reset it to 100, with the toast "UI scale changed to N%". A change in `window.scale_factor()`, `ui_scale_pct` or `row_height_pct` bumps `style_version` and clears `CellTextCache`.

**Fonts.** Bundle in `assets/fonts/` (SIL OFL): `NotoSans-Regular.ttf`, `NotoSans-Medium.ttf`, `NotoSans-SemiBold.ttf` (static weights, not the variable font, because of zed#64783). Load with `cx.text_system().add_fonts(..)` before the first window. Family name `"Noto Sans"`. Numeric cells and chart labels use `FontFeatures(Arc::new(vec![("tnum".into(), 1)]))`. Fallback for non-Latin process names comes from fontconfig system fonts (cosmic-text). The bead must verify that each bundled face loads (zed#61660: faces without `m` are dropped).

**Icons.** Lucide icons through `gpui_kit_assets::IconName`, plus app SVGs in `assets/icons/` served by an `AssetSource` that tries tasklens icons first and falls back to `gpui_kit::assets::Assets`. Process and app icons are file paths resolved by the engine (ADR-016) and drawn with `img(path)` at 16 × ui_scale px. Material Symbols is not used: it is a variable font with unknown `m`-glyph behaviour under zed#61660.

**Accessibility and input.** Everything works from the keyboard. The full key list is canonical table (b) in `docs/RECONCILIATION.md`. The keys that shape this ADR's design:
- Ctrl+1..Ctrl+6 and Alt+1..Alt+6 select a tab; Ctrl+Tab and Ctrl+Shift+Tab switch to the next or previous tab.
- Ctrl+F focuses the filter.
- A printable key while a table has focus (and no input or dialog has focus) starts filtering.
- Enter, Ctrl+I and Alt+Enter open Properties on a process row; Space toggles expand (D-008).
- Delete and Ctrl+E run End Task (SIGTERM); Shift+Delete and Ctrl+X run End Process (SIGKILL); both follow the confirmation rules in ADR-006.
- Ctrl+C runs "Copy row" (processes: "Copy details"); Ctrl+Shift+C runs "Copy rows as table" (ADR-015).
- Esc closes the topmost dialog or menu; else clears the filter and focuses the table; else closes the window if `window.close_with_esc` is on; else does nothing.

Motion settings: `motion.smooth_graphs` alone decides whether charts slide. `motion.animations` decides tab, theme and toast motion. The system reduce-motion setting (`App::reduce_motion()`) turns all of them off. GPUI has no screen reader support on Linux [inferred]; chart values are also shown as text next to every chart.

### Consequences

- The UI does no sorting, filtering or bulk formatting; per-frame cost is visible cells plus chart path copies.
- Several custom widgets (tree-table, header, chart) are tasklens's to maintain.

### Rejected alternatives

- **Kit `DataTable`.** Rejected, reasons above.
- **Kit `AreaChart`.** Rejected: per-frame data cloning, scale rebuilding and label shaping (GPUI section 4).
- **One quad per sample for fills.** Rejected per Astra: stair-steps and wrong cost model.
- **Rebuilding chart paths every frame.** Rejected: lyon tessellation per frame for 40 charts would exceed the budget.
- **Kit `Dock`.** Rejected: more than needed.

### Verification

- `TestAppContext` tests (ADR-013): selection survives re-sort; right-click target captured; `CellTextCache` hit rate above 95% across 10 identical table versions; a chart whose series did not change is not re-rendered by a snapshot install (render counter); the driver requests no frame when no chart is moving.
- Geometry unit tests: gaps split runs; decimation keeps the max spike of a 1-sample burst; bucket alignment is identical for two builds 100 ms apart.
- Phase-0 perf fixture (ADR-014).

---

## ADR-010: Window and platform integration

### Context

The owner runs KDE Plasma 6 on Wayland, three monitors at 110% and 144 Hz, NVIDIA driver 615. GPUI notes: GPUI uses Wayland when `WAYLAND_DISPLAY` is set; frames are paced by `wl_surface.frame`; position cannot be saved on Wayland; zed#64499 (open) can stop redraws until input on KWin; zed#52944 can panic on X11 with NVIDIA's GL path; `TitleBar::window_options()` does not request CSD.

### Decision

**Backend.** Native Wayland is the supported path and the only one in release gates. X11 (XWayland or a real X session) is best effort:
- In `main`, before any thread starts: if `WAYLAND_DISPLAY` is empty and `/proc/driver/nvidia/version` exists and `__NV_FORCE_ENABLE_X11_EGL_PLATFORM` is unset, set it to `1` (`unsafe { std::env::set_var }`, allowed in `tl-app/src/main.rs` only).
- On X11 there is one scale and one refresh rate per process; tasklens does not try to fix this.

**Decorations.** Default: client-side decorations with the TaskSlinger-style title bar: menus and toolbar beside the caption buttons (D-006).

```rust
WindowOptions {
    window_decorations: Some(WindowDecorations::Client),
    window_background: WindowBackgroundAppearance::Opaque,
    window_min_size: Some(size(px(800.), px(500.))),
    app_id: Some("io.github.oysteinkrog.Tasklens".into()),
    window_bounds: Some(restored_bounds),
    ..TitleBar::window_options()
}
```

- Setting `appearance.decorations = "client" | "server"`, default `"client"`, settings label "Use system title bar". A change applies after a restart. With `"server"` KWin draws its own title bar and tasklens draws its tab strip as the first row.
- Predetermined fallback: if any CSD check in the phase-0 checklist fails (drag, double-click maximize, restore, resize from all edges and corners, right-click window menu, tiling), the default becomes `"server"` with no further discussion.

**Window size persistence.** Save only width, height (logical px) and maximized state, in the `[window]` table of `$XDG_STATE_HOME/tasklens/state.toml` (ADR-011), on `observe_window_bounds` changes (debounced 500 ms) and on close. Never save or restore position. Default size `1280 × 800` logical px; minimum `800 × 500`, and on start the saved size is clamped to at least that. Do not pick an output at startup (zed#46378).

**Single instance.** In `tl-app` before GPUI starts:
1. `zbus::blocking::Connection::session()`, then request the name `io.github.oysteinkrog.Tasklens` with `RequestNameFlags::DoNotQueue`.
2. If the name is taken (`RequestNameReply::Exists` or a `NameTaken` error): call `org.freedesktop.Application.Activate({"activation-token": $XDG_ACTIVATION_TOKEN if set})` on `/io/github/oysteinkrog/Tasklens` and exit 0.
3. If owned: serve `org.freedesktop.Application` (`Activate`, `Open` returns an error, `ActivateAction(name, parameter, platform_data)` with actions `quit`, `tab` (string) and `select-pid` (uint32)) at that path. Command-line arguments of a second instance are forwarded through `ActivateAction`. `Activate` forwards to the GPUI main thread through an `async_channel::bounded(4)`; a GPUI task receives it and calls `window.activate_window()` on the main window, or reopens it if closed to the tray. When the KWin bridge is up (ADR-004), it also activates the window through the bridge.
4. If the session bus is unavailable: log a warning and run without single-instance. There is no lock-file fallback.
Focus stealing prevention on Wayland may only flash the taskbar entry; that is accepted [inferred].

**Close behaviour.** The app uses `QuitMode::Explicit`. `on_window_should_close` on the main window:
- tray enabled and setting `tray.close_to_tray = true` (P3): remove the window, keep the app and engine running (demand drops to tray only), return `true`;
- otherwise: call `cx.quit()`.
After `Application::run` returns, `main` calls `EngineHandle::shutdown(Duration::from_millis(1500))`, then flushes logs, then exits. If `run` does not return on Linux [inferred that it does], the same steps run in `cx.on_app_quit`.

**Tray (P3).** Crate `tl-tray` with `ksni =0.3.6`, features `async-io` and `blocking`, using `ksni::blocking::TrayMethods::spawn` (its own thread; no Tokio). `cargo xtask check-deps` proves no Tokio enters the graph. If that check fails, the fallback is a hand-written StatusNotifierItem and dbusmenu server inside `tl-tray` on its own zbus connection, never inside `tl-engine`.
- Tray graphs are drawn on the CPU into ARGB32 buffers (`ksni::Icon { width, height, data }`) at 22×22 and 44×44 px by a small rasterizer in `tl-tray/src/raster.rs` (fill columns plus a 1 px line, no anti-aliasing beyond 2×2 supersampling). Updates at most 1 Hz, skipped when the pixels did not change. Tray data comes from `EngineClient::latest()` on a 1 Hz GPUI timer.
- Main tray icon menu: Show Main Window, Tray icon graphs >, separator, Exit tasklens.
- Tray graph icon menu: Graph color >, Time range >, Close tray icon graph, Tray icon graphs >, separator, Show Main Window, Show mini window, Exit tasklens.
- Left click toggles the main window.
- Items re-register when the `StatusNotifierWatcher` name changes owner.

**zed#64499 mitigation.**
1. Detection, always on: the `AnimationDriver` records the time of every frame callback. If a frame was requested and no callback arrives for 250 ms while the window is believed visible, it logs `warn!(target: "tasklens::frame", "frame stall")` with the duration, and increments `FrameStats.stalls`.
2. Phase-0 soak test: three tasklens windows (main window plus two detached graph windows of the spike), 1 Hz updates, smooth charts on, 10 minutes with no input. Pass: zero stalls longer than 250 ms.
3. If the soak fails: vendor `gpui-pre-linux 0.3.7` into `vendor/gpui-pre-linux/` (crates.io source as the first commit, the fix as the second) and add `[patch.crates-io] gpui-pre-linux = { path = "vendor/gpui-pre-linux" }`. The fix: in `schedule_frame()`, if the state is `AwaitingCallback` (or `Scheduled`/`RetryScheduled`) and the state has not advanced for more than 2 frame intervals, force a new frame request as from `Parked`. Re-run the soak; it must pass.
4. If the soak still fails after the patch, it counts as a GPUI failure in ADR-014.

**Hidden state.** The Wayland suspended state parks GPUI's frame loop. The hub records `pending_notify_since` when it emits a change and `last_render` from `MainWindow::render`. A 500 ms foreground timer sets the window hidden if a notify has been pending for more than 1000 ms without a render, and visible again on the next render. While hidden, the hub keeps installing snapshots (no render happens). Phase 0 must confirm KWin causes a render when a minimized window is restored; if not, the timer calls `window.refresh()` every 1 s while hidden.

**Multi-monitor and refresh.** One window has one frame cadence (the compositor's for its main output). The phase-0 checklist covers: moving across all three monitors, straddling two, maximize on each, unplug and replug one monitor, lock and unlock, and one suspend and resume (zed#58382 atlas corruption check).

**GPU selection.** Default wgpu adapter. Environment `ZED_DEVICE_ID` is respected if the user sets it. In P0 to P3 tasklens never sets it. In P4, the setting `rendering.gpu` may set `ZED_DEVICE_ID` or the software-renderer variables before GPUI starts; a `ZED_DEVICE_ID` the user already set in the environment still wins.

### Consequences

- The TaskSlinger look is the default, with a fixed rule to fall back to server-side decorations.
- Window position is never restored on Wayland, by protocol.

### Rejected alternatives

- **Server-side decorations by default.** Not chosen because it loses the reference layout; it is the predetermined fallback.
- **A 144 Hz app timer for redraws.** Rejected: frames come from compositor callbacks only.
- **libappindicator or a GTK tray.** Rejected: C dependencies; ksni speaks StatusNotifierItem over zbus without Tokio.

### Verification

- Phase-0 checklist results recorded in `docs/spike/phase0-results.md` (ADR-014).
- `tl-app` integration test: start two instances; the second exits 0 within 1 s and the first receives `Activate`.
- Manual: close the window; `tasklens` process exits within 2 s and `tl-gpu-probe` is gone.

---

## ADR-011: Settings and persistence

### Context

Astra: versioned TOML under `$XDG_CONFIG_HOME/tasklens/`, atomic replacement, migration, corrupt-file fallback, and column IDs rather than indexes. TaskSlinger persists themes, columns, sort, motion, units and window size.

### Decision

**Paths** (resolved by `tl-app/src/paths.rs`; an env var that is unset, empty or not absolute falls back to the default):

| Data | Path |
|---|---|
| Settings | `$XDG_CONFIG_HOME/tasklens/settings.toml` (default `~/.config/tasklens/settings.toml`) |
| User themes | `$XDG_CONFIG_HOME/tasklens/themes/*.json` |
| Window state | `$XDG_STATE_HOME/tasklens/window.toml` (default `~/.local/state/...`) |
| Logs | `$XDG_STATE_HOME/tasklens/logs/` |
| Crash reports | `$XDG_STATE_HOME/tasklens/crash/` |
| History | `$XDG_STATE_HOME/tasklens/history/` (ADR-012) |
| Icon path cache | `$XDG_CACHE_HOME/tasklens/icons.toml` |

Directories are created with mode `0700`; files with `0600`.

**Schema** (`tl-model/src/settings.rs`; all structs `#[serde(default, deny_unknown_fields = false)]`):

```toml
schema_version = 1

[general]
start_tab = "processes"           # processes | performance | startup | services | connections | information
close_to_tray = false

[appearance]
theme = "Default Dark"
ui_scale = 1.0                    # 0.8..=1.5
decorations = "client"            # client | server
animations = true                 # "Smooth graphs" and motion
highlight_new_exited = true

[performance]
graph_range_s = 60                # 15 | 60 in v1
cpu_view = "per_core"             # overall | per_core
show_kernel = true

[processes]
interval_ms = 1000                # 250..=10000
mode = "grouped"                  # grouped | tree | flat
sort = { column = "cpu", dir = "desc" }
columns = ["name", "user", "pid", "cpu", "memory_rss", "disk_read", "disk_write", "net", "gpu", "description"]
column_widths = { name = 260, cpu = 70 }   # logical px at ui_scale 1.0, keyed by column id
cpu_mode = "machine"              # machine | core
filter_cmdline = false

[units]
network = "bits"                  # bits | bytes

[history]                          # later phase
enabled = false
retention_h = 6                   # 1..=24
max_disk_mb = 256
```

Column ids are stable strings defined once in `tl-model::ids::ColumnId` with `as_str()`/`from_str()`; an unknown id in the file is dropped with a log line.

**Load.**
1. Missing file: defaults; do not write until the user changes something.
2. Parse to `toml::Table`. Parse error: rename the file to `settings.toml.corrupt-<unix seconds>`, use defaults, show a warning toast "Settings file was unreadable and has been reset. The old file was kept as {name}".
3. `schema_version` missing: treat as 1. Lower than current: run migrations `fn migrate_vN_to_vN1(t: &mut toml::Table)` in order, then deserialize. Higher than current: deserialize what is understood, set `read_only = true`, never save, show the toast "Settings were written by a newer tasklens; changes will not be saved".
4. Values out of range are clamped and logged.

**Save.** Debounced 500 ms after the last change, plus on exit. Atomic write: write `settings.toml.tmp-<pid>` in the same directory, `fsync` the file, `rename` over `settings.toml`, `fsync` the directory. The same routine (`tl-app/src/atomic.rs::write_atomic`) is used for `window.toml`, user themes and XDG autostart overrides (ADR-016).

**Ownership.** `tl-ui` holds settings in a GPUI `Global` and calls a `Box<dyn Fn(&Settings)>` persist callback given by `tl-app`. Engine-relevant parts are pushed with `EngineClient::set_settings` as `EngineSettings` (derived; never persisted separately).

### Consequences

- Settings survive column reordering and new columns because they use ids.
- A newer file is never overwritten by an older build.

### Rejected alternatives

- **JSON or RON.** Rejected: TOML is easier to hand-edit and Astra recommended it.
- **`directories` crate.** Rejected: 20 lines of code; one less dependency.

### Verification

- Tests with `tempfile`: missing file; corrupt file renamed; v0 (no version) migrated; future version read-only and never written; crash simulation (kill between write and rename) leaves either the old or new file intact.
- Test: unknown column id dropped, known ones keep order.

---

## ADR-012: History storage (later phase)

### Context

The owner wants six-hour process history (TaskSlinger's history mode with a timeline) and recording while hidden. Astra calculated naive 1 Hz retention at about 11 GB and said history needs its own design. This ADR fixes what the phase-1 data model must provide and the storage format so later work does not change phase-1 types.

### Decision

**Constraints on phase-1 code (binding now):**
1. All timestamps are CLOCK_BOOTTIME `t_ns` (ADR-003), and every snapshot carries `boot_id`.
2. `SeriesId`, `MetricId` and `ColumnId` numbers and strings are frozen.
3. `ProcessRecord` numeric fields are fixed-size; text lives in `ProcessMeta`.
4. The reducer exposes a hook used later by the recorder:

```rust
pub trait RecordSink: Send + 'static {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]);
    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>);
    fn on_device_catalog(&mut self, catalog: &DeviceCatalog);
}
```

In phase 1 the reducer holds `Option<Box<dyn RecordSink>>` set to `None`.

**Format** (crate `tl-history`):
- Directory per boot: `history/<boot_id as 16 hex digits>/`.
- Segment files of 10 minutes: `seg-<start boottime seconds, 10 digits>.tlh`.
- Segment header (fixed, 64 bytes): magic `b"TLH\x01"`, `format_version: u16 = 1`, `boot_id: u64`, `realtime_offset_ns: i64` (CLOCK_REALTIME minus CLOCK_BOOTTIME at segment start), `start_t_ns: u64`, reserved zero bytes.
- Body: blocks of 60 s. Each block is `u32 LE compressed length | u32 LE crc32 of compressed bytes | zstd level 3 frame` containing a postcard `Block`:

```rust
pub struct Block {
    pub start_t_ns: u64,
    pub new_meta: Vec<(MetaId, HistMeta)>,       // comm, display name, exe basename, app id; no cmdline
    pub keyframe: Vec<HistProcess>,              // every live process at block start
    pub deltas: Vec<SweepDelta>,                 // one per process sweep in the block
    pub system: Vec<(SeriesId, Vec<(u32 /*ms offset*/, f32)>)>,  // 1 Hz system series
}
pub struct HistProcess { pub key: ProcessKey, pub meta: MetaId, pub ppid: u32,
                         pub cpu_permille: u16, pub rss_kib: u32, pub io_r_kibps: u32, pub io_w_kibps: u32,
                         pub net_kibps: u32, pub gpu_pct: u8 }
pub struct SweepDelta { pub dt_ms: u32, pub changed: Vec<(u32 /*keyframe or new index*/, FieldPatch)>,
                        pub started: Vec<HistProcess>, pub exited: Vec<u32> }
```

- `FieldPatch` stores only fields that changed beyond a threshold (CPU by ≥ 1 permille, RSS by ≥ 256 KiB, rates by ≥ 1 KiB/s or to/from zero).
- Footer written when a segment closes: block offset index. A segment without a footer (crash) is read by scanning blocks until the first bad CRC.
- Command lines are never stored (ADR-015).

**Retention and budget.**
- Default retention 6 h; settings 1 to 24 h; disk cap `max_disk_mb = 256`.
- The recorder deletes segments older than retention and, when over the cap, the oldest segments first. The UI shows the real retained span.
- On start, directories of other boots older than retention are deleted.

**Budget math** (estimates, to be measured when the phase starts):
- Changed processes per 1 s sweep on a desktop: about 200. Patch size about 15 bytes. 200 × 15 B × 21,600 s = 64.8 MB raw for 6 h.
- Keyframes: 2,000 processes × 20 B × 360 blocks = 14.4 MB raw.
- System series: about 120 series × 4 B × 21,600 s = 10.4 MB raw (history keeps 1 Hz even when live CPU runs at 10 Hz; 10 Hz is reduced to per-second max).
- Total about 90 MB raw; zstd level 3 at an assumed 3:1 gives about 30 MB on disk for 6 h.
- Worst case, all 2,000 processes changing every second: 2,000 × 15 B × 21,600 = 648 MB raw, about 216 MB compressed, still under the 256 MB cap.
- RAM: the open block (at most 60 s of patches, about 0.2 MB typical, 2 MB worst case) plus segment indexes. Budget 16 MB for history in RAM.

**Reading.** The UI asks for a time range through a new `Command::HistoryQuery { id, range, what }`; the engine replies with a `HistoryView` component in the snapshot (max 1,024 points per series, decimated min/max as in ADR-009). Process history at a time T is rebuilt from the block's keyframe plus deltas up to T.

### Consequences

- Phase-1 code needs no change when history arrives beyond setting the `RecordSink`.
- Recording while hidden costs the 1 Hz sources in ADR-008's "recording on" column.

### Rejected alternatives

- **SQLite.** Rejected: write amplification for append-only time series, and a C dependency for little gain.
- **Storing every process every second.** Rejected: 11 GB per 6 h (Astra).
- **Keeping all history in RAM.** Rejected: breaks the 150 MB target.

### Verification

- When the phase starts: write 6 h of synthetic data (2,000 processes, 200 changing per second) in an accelerated test; disk usage under 64 MB; random-time reconstruction equals the recorded state at that time; truncated last block is recovered up to the bad CRC.

---

## ADR-013: Testing strategy

### Context

Astra: LabRuntime for engine scheduling and state transitions, fixtures for parsing, real-system tests for permissions, pidfds, D-Bus, NVML failure, shutdown and Wayland; headless tests cannot certify compositor behaviour. GPUI offers `TestAppContext`, kit test helpers and a Linux headless wgpu renderer [inferred to work].

### Decision

**Layers and commands:**

| Layer | Location | Command | Runs in CI |
|---|---|---|---|
| Unit and parser fixture tests | `crates/tl-collect/tests/`, `tl-model`, `tl-ops` unit tests | `cargo test --workspace` | yes |
| Parser property tests | `tl-collect/tests/prop_*.rs` (proptest: arbitrary bytes never panic) | same | yes |
| Engine LabRuntime tests | `crates/tl-engine/tests/lab_*.rs` | same | yes |
| UI logic tests | `crates/tl-ui/tests/ui_*.rs` with `#[gpui_kit::test]` | same | yes |
| Headless screenshot tests | `crates/tl-ui/tests/render_*.rs`, feature `render-tests` | `cargo test -p tl-ui --features render-tests` | yes, with Mesa lavapipe (`VK_ICD_FILENAMES` set to lavapipe) |
| Real-system integration | `tests/real_*.rs` in each crate, `#[ignore]` | `cargo test --workspace -- --ignored` | no; owner machine |
| Performance gates | `cargo xtask bench` | see ADR-014 | no; owner machine |
| Static checks | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo xtask check-deps` | | yes |

`scripts/ci.sh` runs the CI rows in order. A GitHub Actions workflow on the private repo calls it on `ubuntu-latest` with the pinned toolchain.

**Parser fixtures.**
- `cargo xtask capture-fixture <name>` copies the files tasklens reads (`/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/proc/net/dev`, `/proc/<pid>/{stat,statm,status,io,comm,cmdline,cgroup}` for up to 50 pids, the relevant `/sys` files) into `crates/tl-collect/tests/fixtures/<name>/` as a fake root.
- It scrubs: `cmdline` becomes `comm` plus `--arg`, user names become `user1..`, hostnames and serials become `REDACTED`. It never copies `environ`, `maps` or `fd` targets.
- Samplers take a root path (`SysRoot { proc: PathBuf, sys: PathBuf }`), so tests point them at a fixture.
- Expected output is checked with `insta` snapshots (`.snap` files committed).
- Required fixtures: `owner-desktop` (this machine), `minimal-vm`, plus hand-made edge cases: CPU hotplug, counter reset, process name with spaces and parentheses, invalid UTF-8 in `comm`, cmdline longer than 32 KiB, missing `cpufreq`, a zram device, a disk with partitions and a dm device.

**Engine LabRuntime tests.**
- Engine code is `async fn` taking `Cx` and channel endpoints, so tests run it with `LabRuntime::new(LabConfig::new(seed).max_steps(1_000_000))`, `lab.state.create_task(..)`, and `run_with_auto_advance()`.
- Fake sources implement `Source` with scripted results (ok, err, panic, slow).
- Required cases:
  - restart and backoff timings;
  - budget exhausted, then 60 s disable, then restart;
  - no restart after cancel;
  - demand switches rates;
  - hidden policy makes zero samples;
  - the reducer coalesces 100 messages into one publish;
  - sequence strictly increases;
  - session restart after a reducer panic;
  - admission limit (the 17th action is `Busy`);
  - action timeout gives `OutcomeUnknown`;
  - shutdown drain is `Quiescent`.
- Every test ends with `run_until_quiescent_with_report().lab_test_passed()`.
- `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]` on the restart and action tests.

**UI tests** with `FakeEngine` (implements `EngineClient`, publishes scripted snapshots, records commands):
- Selection by identity survives re-sort and filter.
- Right-click captures targets; a re-sort before the click does not change them.
- Filter debounce: 5 keystrokes within 100 ms send one `SetTableQuery`.
- Demand: switching tabs changes `Demand`; visible-row keys update after a re-sort with an unchanged range; demand is not resent when equal.
- Render counters (a test-only `Cell<u32>` in each view, behind `cfg(test)` or feature `test-hooks`): a CPU-only snapshot change does not re-render `ProcessTable`; a series change re-renders only its `ChartView`.
- Animation driver: no frame request when no chart is moving; exactly one pending request per window.

**Headless screenshot tests.**
- Use `HeadlessAppContext::with_platform(Arc::new(CosmicTextSystem::new(..)), assets, || gpui_platform::current_headless_renderer())` and `capture_screenshot(window)`. This path exists in source but gpui-kit does not exercise it [inferred]; the first bead that writes these tests confirms it works and, if not, replaces them with `painted_quads()` assertions.
- Cases: one chart with a gap and a spike; the CPU grid; 30 table rows with each `Validity` state; Default Dark theme. Rendered at scale 1.0 and 1.1.
- Golden PNGs in `crates/tl-ui/tests/golden/`. Pass: at most 0.1% of pixels differ by more than 2/255 in any channel. Update goldens only with `TL_UPDATE_GOLDEN=1`.

**Real-system integration tests** (owner machine, `--ignored`):
- procfs fields of another user's process give `Restricted`;
- pidfd identity checks;
- nice and affinity on a child process;
- cgroup freeze on a `systemd-run --user --scope`;
- start and stop of a test user unit `tasklens-test.service`;
- NVML `Hello` and samples;
- GPU probe kill and restart;
- shutdown under load within 1.5 s;
- second-instance activation.

### Consequences

- Most logic is tested without a display or root.
- Compositor behaviour is tested only by the phase-0 checklist and manual release checks.

### Rejected alternatives

- **Tokio-based test harnesses.** Rejected: LabRuntime is deterministic and keeps the no-Tokio rule simple.
- **Pixel-exact goldens.** Rejected: GPU and driver differences.

### Verification

- `scripts/ci.sh` green on every commit; each bead names the tests it adds.

---

## ADR-014: Performance budgets, phase-0 spike and fallback

### Context

UI and rendering speed are the owner's top priority. Astra found the chart cost estimate unsupported, the process sweep likely above the 2% collection target (30 ms per 2,000 processes is 3% of a core at 1 Hz), and asked for predetermined gates and a predetermined fallback. GPUI notes: about 6.94 ms per frame at 144 Hz; unfocused windows run near 30 fps.

### Decision

**Measurement tools** (built with `--features bench`):
- `tasklens --bench <scene> --duration 30 --warmup 5 --json <out>` opens the scene with synthetic data from `tl-ui/src/bench/` (deterministic, PRNG seed 42) and, for the bridge scene, the real engine.
- UI thread CPU per frame: `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` on the main thread, read in each `on_next_frame` callback; delta between callbacks.
- Frame interval: `Instant::now()` in each frame callback. A **missed frame** is an interval above 1.5 × the output period (10.42 ms at 144 Hz).
- Process CPU: `/proc/self/stat` utime+stime over the run, divided by wall time, as percent of one core. Child `tl-gpu-probe` measured the same way and reported separately.
- Memory: `VmRSS` from `/proc/self/status` at the end; PSS from `smaps_rollup` reported only.
- GPU: NVML power and utilization sampled by the bench through the probe at 1 Hz, reported against a 30 s baseline with tasklens closed. Report only, no gate.
- `cargo xtask bench` runs every scene 3 times and writes `target/bench/<date>-<git sha>.json` plus a table.

**Phase-0 fixture.** Window 1600 × 1000 logical px, on a 144 Hz monitor at 110%, focused, smooth graphs on.
- Scene `perf-cpu`: Performance tab, CPU page. Total CPU chart (fill + line + kernel line) and a 32-core grid (8 × 4) with kernel overlays, 60 s range, 10 Hz data. Device sidebar with 9 mini graphs (CPU, memory, GPU, 4 disks, 2 networks) at 1 Hz. A stats block with 12 values updated at 1 Hz and per-core percent labels updated at 10 Hz.
- Scene `table-2000`: Processes tab, 2,000 rows, 10 columns, every value changes at 1 Hz, sorted by CPU descending so order changes every second, grouped mode with 40 groups. Programmatic scroll at 1,200 logical px/s, bouncing between top and bottom, through `UniformListScrollHandle`.
- Scene `bridge`: `perf-cpu` where the total CPU chart is fed by the real `tl-engine` `Cpu` worker at 10 Hz through the real bridge.
- Scene `static`: `perf-cpu` with smooth graphs off.
- Checklist (manual, recorded in `docs/spike/phase0-results.md`): CSD checks from ADR-010, three-monitor moves, straddling, maximize per monitor, unplug and replug, lock and unlock, suspend and resume, 10-minute zed#64499 soak, minimize and restore re-renders, text sharpness at 110% (screenshot at 2× zoom shows no blur).

**Gates.** Each gate must pass in the median of 3 runs unless stated.

| Gate | Pass |
|---|---|
| UI thread CPU per frame, `perf-cpu` | p50 ≤ 1.0 ms, p99 ≤ 2.0 ms |
| UI thread CPU per frame, `table-2000` while scrolling | p99 ≤ 2.0 ms |
| Missed frames, `perf-cpu` and `table-2000` | 0 in the median run; at most 2 in every run; none above 3 × period |
| Process CPU, `perf-cpu` | ≤ 30% of one core |
| Process CPU, `static` | ≤ 2% of one core |
| Process CPU, window minimized, recording off | ≤ 0.3% of one core (measured over 60 s) |
| Collection cost, real engine, Performance tab visible | ≤ 2% of one core |
| Collection cost, real engine, Processes tab visible with this machine's processes | ≤ 3.5% of one core |
| First frame after launch (warm cache) | ≤ 200 ms median of 5 launches, measured from process start time (`/proc/self/stat` field 22) to the first frame callback |
| First frame, cold (after `echo 3 > drop_caches`, run by the owner) | ≤ 500 ms |
| RSS after `table-2000` | ≤ 150 MB |
| zed#64499 soak | 0 stalls over 250 ms in 10 minutes |
| Bridge | 10 Hz samples arrive; wake-to-render latency p99 ≤ 20 ms |

The collection-cost gates replace PLAN.md's single 2% target because of Astra's arithmetic; the process rate is not lowered to pass.

**Decision rule.**
1. All gates pass: GPUI is confirmed. Production beads proceed.
2. Any rendering or Wayland gate fails: one focused optimization pass of at most 2 working days, limited to: path merging (ADR-009 rule 7), decimation tuning, fewer grid quads, caching more views, text cache fixes, and the vendored zed#64499 patch. Then re-run every gate.
3. Still failing: switch to the fallback below. No other GPUI forks, gpui-ce or private renderer work.
4. Collection-cost gates are engine work and never trigger the UI fallback.

**Predetermined fallback: eframe with a custom wgpu chart pass.**
- New crate `tl-ui-egui` replaces `tl-ui`; `tl-model`, `tl-collect`, `tl-ops`, `tl-engine` and `tl-gpu-probe` are unchanged. It implements the same views against `EngineClient`.
- `eframe = { version = "=0.36.2", default-features = false, features = ["wgpu", "wayland", "x11", "accesskit", "default_fonts"] }`, `egui-wgpu = "=0.36.2"`.
- Charts: one `egui_wgpu::CallbackTrait` paint callback per window drawing every chart in one render pass. A persistent storage buffer holds all series samples (packed rings); a descriptor buffer holds per-chart rectangle, series offsets, y range and colors; a per-frame uniform holds `now`, range and viewport. Upload only changed ranges when samples or descriptors change. Fills and strokes are instanced segment quads with analytic anti-aliasing in WGSL. Text and interaction come from egui above the pass.
- Table: egui `ScrollArea::show_rows` with 28 px rows and the same engine-built `ProcessTableView`.
- The same fixture and gates apply. If eframe also fails, the owner decides.

### Consequences

- The toolkit choice is settled by numbers measured on this machine.
- The fallback costs only the UI crate.

### Rejected alternatives

- **"Well under 1 ms" without measurement.** Rejected per Astra.
- **gpui-ce or a GPUI fork with custom shaders.** Rejected: unproven API (Astra section 4).
- **Makepad, iced, Slint, custom wgpu app.** Rejected per Astra section 1 conditions; none are met.

### Verification

- `docs/spike/phase0-results.md` with the JSON outputs and the checklist, committed before any phase-1 UI bead starts.
- Each later phase re-runs `cargo xtask bench`; a regression of more than 20% on any gated metric blocks the phase.

---

## ADR-015: Logging, errors, crash handling and privacy

### Context

Astra risk 7: process metadata is sensitive and can be hostile (control characters, invalid UTF-8, huge command lines) and reaches rendering, logs and exports.

### Decision

**Logging.**
- `tracing` everywhere; `tracing-subscriber` with `EnvFilter` from `TASKLENS_LOG`, default `info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,zbus=warn`.
- Outputs: stderr when stderr is a terminal; always a file through `tracing_appender::rolling::RollingFileAppender::builder().rotation(Rotation::DAILY).filename_prefix("tasklens").filename_suffix("log").max_log_files(7)` in `$XDG_STATE_HOME/tasklens/logs/`, wrapped in `tracing_appender::non_blocking` (its guard is held in `main` and dropped last).
- Targets: `tasklens::engine`, `tasklens::source::<name>`, `tasklens::action`, `tasklens::ui`, `tasklens::frame`, `tasklens::probe`.
- No logging inside per-frame or per-process loops above `trace` level.

**What is never logged** (at any level): command lines, environment, working directories, open file paths, executable paths of other users' processes, window titles, user names other than the current user. Processes are logged as `pid` plus `comm` only, and `comm` only at `debug` and below. Action logs record kind, target PID and result.

**Errors.**
- Libraries use `thiserror` enums: `CollectError` (tl-collect), `OpsError` (tl-ops), `ProbeError` and `WorkerError` (tl-engine), `HistoryError` (tl-history). `anyhow` only in `tl-app::main` and `xtask`.
- `ActionError` (tl-model) has variants `PermissionDenied`, `NoSuchProcess`, `Partial { done: u16, total: u16 }`, `UnitJob(Arc<str>)`, `Os(i32)`, `Internal(Arc<str>)`. `tl-ui/src/actions.rs::message(&ActionError) -> SharedString` turns each into plain, sentence-case text, for example "Permission denied. This process belongs to another user."
- Collection errors appear as `Validity::Failed` plus `SourceHealth.last_error`; a source error never becomes a toast unless the user started an action.

**Crash handling.**
- `panic = "unwind"` (ADR-002). A panic in a worker is handled by its supervisor; in the reducer by the session loop (ADR-004); in an action task, the action reports `Failed(Internal("internal error"))`.
- A panic hook (installed first in `main`) writes `$XDG_STATE_HOME/tasklens/crash/crash-<unix seconds>.txt` with: tasklens version and git sha, thread name, panic message, location, `std::backtrace::Backtrace::force_capture()`. No process metadata, no settings contents. Then it calls the default hook. Keep the newest 20 reports.
- A panic on the GPUI main thread ends the process; the next start shows a toast "tasklens closed unexpectedly last time. A crash report was saved to {path}".
- Native crashes are left to `systemd-coredump`. NVML native crashes only affect `tl-gpu-probe` (ADR-007).

**Untrusted process text** (`tl-collect/src/sanitize.rs`, applied before any string enters `ProcessMeta`):
- Read limits: `comm` 64 bytes, `cmdline` 32 KiB (truncate and append `…`), `exe` link 4 KiB, `status` 8 KiB.
- Bytes are decoded with `String::from_utf8_lossy`.
- C0 and C1 control characters become U+FFFD. Bidirectional controls U+202A to U+202E and U+2066 to U+2069, and U+200B to U+200F, are removed.
- Lines are joined with spaces; NUL separators in `cmdline` become spaces.

**Exports and copy.** "Copy" in the process menu copies only the visible columns of the selected rows, as tab-separated text. There is no automatic export. Any later export feature asks before including command lines.

### Consequences

- Log files are safe to attach to bug reports without editing.

### Rejected alternatives

- **`log` and `env_logger`.** Rejected: GPUI and zbus already use `tracing`, and spans help with source timing.
- **`panic = "abort"`.** Rejected: supervision of panics needs unwinding.

### Verification

- Unit tests for `sanitize`: control characters, bidi overrides, invalid UTF-8, 1 MiB cmdline truncated to 32 KiB.
- Test: a fake process with cmdline `secret-token-123` runs through a full sweep and an action at `trace` level; the log file does not contain `secret-token-123`.

---

## ADR-016: Data sources and parsing rules

### Context

PLAN.md section 3 lists sources measured on this machine. MAGPIE section 7 records how Mission Center reads each metric. Astra adds FD caching limits, metadata refresh, cgroup grouping without hard-coded uids, and labelling of network attribution.

### Decision

**Parsers.** Hand-written in `tl-collect`, no `procfs` crate. Each parser takes `&[u8]` and a reusable output struct, uses `memchr`, and allocates only for new strings. `/proc/<pid>/stat` is parsed from the last `)` to handle names with spaces and parentheses.

**File descriptor policy.**
- Kept open and re-read with `pread` at offset 0: `/proc/stat`, `/proc/meminfo`, `/proc/vmstat`, `/proc/diskstats`, `/proc/net/dev`, each used `cpufreq` and `hwmon` file, zram `mm_stat`. Reopened after any read error.
- Per-process files are opened with `openat` relative to an `O_DIRECTORY` fd of `/proc/<pid>` during the sweep, read, and closed in the same sweep.
- An FD cache of at most 128 entries holds `smaps_rollup` fds for `detail_keys` only; entries are dropped when the key leaves demand or the identity check fails.

**Sources** (phase in brackets):

| Data | Source | Notes |
|---|---|---|
| CPU [1] | `/proc/stat` | math in ADR-008 |
| CPU frequency [1] | `/sys/devices/system/cpu/cpufreq/policyN/cpuinfo_avg_freq`, else `scaling_cur_freq` | amd-pstate-epp here |
| CPU temperature [1] | hwmon `k10temp` `temp1_input` (`Tctl`), else `coretemp`, `zenpower` | board chips such as `nct6797` at 1 Hz only |
| Memory [1] | `/proc/meminfo`, `/sys/block/zram*/mm_stat`, `/proc/vmstat` | |
| Disks [1] | `/proc/diskstats`, `/sys/block/*` | active % from `io_ticks` delta; latency = Δ(read_ticks+write_ticks) / Δ(ios); exclude `loop`, `ram`, `zram`, `fd` from the sidebar; partitions and `dm-*` are children |
| Network [1] | `/proc/net/dev`; `ifindex` from `/sys/class/net/<if>/ifindex`; netns inode from `/proc/self/ns/net` | `lo` hidden; `veth*`, `docker*`, `br-*`, `virbr*`, `tailscale*` hidden by default, setting `show_virtual_interfaces = false` |
| NVIDIA GPU [1] | `tl-gpu-probe` (ADR-007) | |
| AMD/Intel GPU [later] | sysfs `gpu_busy_percent`, `mem_info_vram_*`, hwmon, DRM fdinfo | untested here |
| Sensors and fans [1] | `/sys/class/hwmon/hwmon*/{temp,fan,pwm}*` | |
| Processes [2] | `/proc/<pid>/stat`, `statm`, `status` (uid, threads), `io` (own processes), `cgroup` | `io` gives `Restricted` for other users |
| Process metadata [2] | `comm`, `cmdline`, `exe` | refreshed every 10 sweeps per process and immediately when `comm` changes (exec) |
| PSS/USS [2] | `/proc/<pid>/smaps_rollup` | `detail_keys` only |
| App grouping [2] | cgroup v2 path from `/proc/<pid>/cgroup`; current uid from `getuid()` | app id from scope name `app-<launcher>-<app id>-<n>.scope` (KDE: `app-<desktop file id>@<id>.service` or `.scope`); desktop file lookup through `XDG_DATA_DIRS` |
| Per-process network [3] | `sock_diag` `inet_diag` dump for TCP v4 and v6 with `INET_DIAG_INFO` (`tcpi_bytes_acked`, `tcpi_bytes_received`) | labelled "Network (TCP, attributed)"; sockets shared by several processes count for each and the cell gets a tooltip; UDP not attributed |
| Socket to pid map [3] | `/proc/<pid>/fd` symlinks `socket:[inode]` | rebuilt every 2 s while demanded; own processes only without the helper |
| Connections [3] | `sock_diag` dump TCP, UDP, v4, v6 | |
| Services [3] | systemd Manager over D-Bus: `ListUnits`, `ListUnitFiles`, `Subscribe`, `UnitNew`, `UnitRemoved`, `JobRemoved`, `PropertiesChanged` | system and user managers are separate identities `UnitRef { manager: System | User, name }` |
| Service logs [3] | `journalctl -o json -n 50 --no-pager -u <unit>` run with `std::process::Command` inside `spawn_blocking`, 5 s timeout, then kill and wait | |
| Startup apps [3] | XDG autostart: `$XDG_CONFIG_HOME/autostart` overrides `$XDG_CONFIG_DIRS/autostart` (default `/etc/xdg/autostart`) by file name; `Hidden`, `OnlyShowIn`, `NotShowIn` against `XDG_CURRENT_DESKTOP`; systemd user units with `WantedBy=` | disable a system entry by writing a user override with `Hidden=true` (atomic write); enable by removing an override that differs only in `Hidden`, else set `Hidden=false`; units use Enable/DisableUnitFiles |
| Information [3] | `/proc/cpuinfo`, `/sys/devices/system/cpu`, `/sys/class/dmi/id` (readable fields), `/etc/os-release`, NVML device info | root-only fields (serials, DIMMs) shown as `Restricted` |
| Disk details [later] | udisks2 over D-Bus | SMART, partitions, eject |
| Batteries [later] | UPower over D-Bus | |
| Network details [later] | NetworkManager over D-Bus | |
| Icons [2] | desktop entry `Icon=`; theme from `~/.config/kdeglobals` `[Icons] Theme=`, then `breeze`, then `hicolor`; sizes 16, 22, 24, 32, then `scalable` | resolved paths cached in `$XDG_CACHE_HOME/tasklens/icons.toml` |

**Restricted, not zero.** Any `EACCES` or `EPERM` on a field sets that field `Restricted`. Processes owned by other users and capability-raised processes such as `kwin_wayland` show the lock marker for unreadable fields.

**Process power column.** Not shown by default: there is no per-process power source on this machine. The column id `power` is reserved.

### Consequences

- Parser speed and allocation are under tasklens's control and covered by fixtures.

### Rejected alternatives

- **`procfs` crate.** Rejected: allocates per call; the hot path needs reused buffers.
- **nethogs screen scraping (Mission Center).** Rejected: fragile, needs capabilities on a third-party binary.
- **libsystemd for journal reading.** Rejected for v1: `journalctl` is enough for 50 lines and avoids a C binding.

### Verification

- Fixture tests per source (ADR-013).
- Criterion bench `tl-collect/benches/sweep.rs`: a sweep of the `owner-desktop` fixture's processes scaled to 2,000 entries runs in ≤ 15 ms on this machine.
- Real-system test: a process of another user shows `Restricted` for I/O, not zero.

---

## Open questions and recommended defaults

These could not be settled from the evidence. Each has a recommended default that beads use unless Oystein decides otherwise.

1. **Reverse-DNS application id and D-Bus name.** The repo is private and has no domain. Default: `io.github.oysteinkrog.Tasklens` for `app_id`, the desktop file name and the bus name.
2. **Collection-cost gate with the Processes tab visible.** PLAN.md said 2%; Astra's arithmetic says about 3% for 2,000 processes. Default: gate at 3.5% (ADR-014) and keep the 2% gate for the Performance tab.
3. **History recording default once `tl-history` lands.** Astra says off by default; the owner wants six-hour history. Default: off until the history phase ships, then on with 6 h retention and a 256 MB cap.
4. **Decorations default.** Client-side matches TaskSlinger; server-side is lower risk on KDE. Default: client-side, with the fixed fallback rule in ADR-010.
5. **Font family.** Astra recommends Noto Sans; PLAN.md allowed Inter. Default: Noto Sans static weights. Inter is the alternative if Noto Sans lacks `tnum` in the bundled files (the font bead checks this first).
6. **Icons.** TaskSlinger uses Material Symbols. Default: Lucide through gpui-kit, because the Material Symbols variable font may hit zed#64783 and zed#61660.
7. **Whether `Application::run` returns on Linux.** Not verified. Default: shut the engine down after `run` returns, and also from `on_app_quit` if it does not.
8. **Headless wgpu screenshots on Linux.** The code path exists but gpui-kit does not test it. Default: try it in the first render-test bead; fall back to `painted_quads()` assertions.
9. **Minimize and restore on KWin.** Not verified that GPUI renders on restore without a notify. Default: the hidden-state timer in ADR-010, plus `window.refresh()` every 1 s while hidden if phase 0 shows it is needed.
10. **GPU power gate.** No baseline exists yet. Default: report only in phase 0; set a gate after the first measurements.
11. **Exact gpui-kit `ThemeColor` token names.** Only some were listed in the research. Default: the mapping in ADR-009, checked against source by the theme bead.
12. **zed#64499 fix details.** The patch outline in ADR-010 is based on the issue text, not a tested fix. Default: apply it only if the soak test fails, and re-run the soak.
13. **Users and Details tabs (TaskSlinger) and Mission Center's Apps page.** Not specified in detail by these ADRs. Default: Users and Details come after phase 3 and get their own small ADRs; Mission Center's Apps page is covered by the Processes tab's grouped mode.
