> **Note.** This file includes the reconciliation in `docs/RECONCILIATION.md` (R-001 to R-138) and the decisions in `docs/DECISIONS.md` (D-001 to D-010). If anything here still disagrees with either of those files, DECISIONS.md wins first and RECONCILIATION.md second. Phases use the canonical P0 to P4 numbering from R-072.

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
| 017 | Device metric history |
| 018 | Engine service, clients and multiple hosts |
| 019 | Configurable widget layout |
| 020 | tl-ui crate layers |

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
    tl-model/             # lib: data contracts, units, formatting, wire framing, EngineClient trait, engine settings (EngineSettings, HistorySettings, NetworkUnit, engine ranges)
    tl-collect/           # lib: synchronous samplers and parsers (procfs, sysfs, netlink)
    tl-ops/               # lib: synchronous OS operations (signals, nice, affinity, ioprio, cgroup writes)
    tl-engine/            # lib: asupersync runtime, workers, reducer, actions, D-Bus adapters, probe client
    tl-gpu-probe/         # bin: the only process that loads NVML
    tl-ui/                # lib: GPUI views, chart element, process table, theme; after the split (ADR-020) the shell: main window, app init
    tl-ui-settings/       # lib: UI settings and widget layout types, moved from tl-model (ADR-020, CS-D1 and CS-D2)
    tl-ui-*/              # lib: base, kit, chart, table, perf-core, overview, perf, procs, sys (ADR-020)
    tl-ui-testkit/        # lib, dev only: FakeEngine and render helpers (ADR-020)
    tl-ui-pagehost/       # lib, dev only: mounts one page for integration tests (ADR-020)
    tl-app/               # bin "tasklens": main, wiring, single instance, settings store, logging
    tl-history/           # lib, P2: device metric segment format (ADR-017); P3: history segment format, store and rolling window (ADR-012)
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
| tl-model | none | serde, postcard, thiserror, async-channel, bitflags, smallvec, rustc-hash (`GroupKey` is an FxHash of `GroupKind`, R-142), rustix (feature `time` only, for `clock_gettime`), toml (`LayoutEntry::Kept` in `layout.rs`, ADR-019; moves to tl-ui-settings with `layout.rs` in CS-D2) |
| tl-collect | tl-model | rustix, memchr, thiserror, tracing, smallvec, rustc-hash, netlink-packet-core, netlink-packet-sock-diag, netlink-sys, sha2, md-5 (file hashes for Properties and VirusTotal), bitflags (`CpuFlags`, R-285) |
| tl-ops | tl-model | rustix, libc, thiserror, tracing |
| tl-engine | tl-model, tl-collect, tl-ops, tl-history (P2 for ADR-017 device history, P3 for ADR-012) | asupersync, arc-swap, async-channel, zbus, futures-lite, rustix, libc (`close_range` in the GPU probe child only, R-290), postcard, serde, serde_json (KWin `Update` payload, `journalctl -o json`, `kscreen-doctor -j`, smartctl JSON; R-142), thiserror, tracing, rustc-hash, smallvec, ureq (public IP fetch only, R-131), bitflags (`DeviceKinds`, E08; R-273) |
| tl-gpu-probe | tl-model | nvml-wrapper, rustix, postcard, serde, ash, khronos-egl (both only for the `--api-versions` mode, ADR-007) |
| tl-ui | tl-model | gpui-kit, tracing, smallvec, rustc-hash, serde, serde_json (theme JSON), fluent-bundle, unic-langid (strings, R-132), intl_pluralrules (plural rules in `pages/performance/details.rs`, in use since o55.37), async-channel (`AppRequest` receiver and tray channels), jiff (local time formatting), unicode-segmentation (R-142), image (optional, `render-tests` only, PNG goldens, R-291) |
| tl-ui-settings | tl-model | serde, toml (`LayoutEntry::Kept`, ADR-019), bitflags (`layout.rs`) |
| tl-ui-base | tl-model, tl-ui-settings | gpui-kit, tracing, smallvec, rustc-hash, serde, serde_json, fluent-bundle, unic-langid, async-channel, unicode-segmentation |
| tl-ui-kit | tl-model, tl-ui-settings, tl-ui-base | gpui-kit, tracing, smallvec, rustc-hash, fluent-bundle |
| tl-ui-chart | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit | gpui-kit, smallvec, jiff |
| tl-ui-table | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit | gpui-kit, tracing, smallvec, rustc-hash, serde, fluent-bundle |
| tl-ui-perf-core | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-chart, tl-ui-table | gpui-kit, tracing, smallvec, rustc-hash, serde, fluent-bundle, unic-langid, intl_pluralrules, unicode-segmentation |
| tl-ui-overview | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-chart, tl-ui-table, tl-ui-perf-core | gpui-kit, tracing, smallvec, rustc-hash, fluent-bundle, jiff |
| tl-ui-perf | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-chart, tl-ui-table, tl-ui-perf-core, tl-ui-overview | gpui-kit, tracing, smallvec, rustc-hash, serde, fluent-bundle, jiff |
| tl-ui-procs | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-chart, tl-ui-table, tl-ui-perf-core | gpui-kit, tracing, smallvec, rustc-hash, serde, serde_json, fluent-bundle, jiff |
| tl-ui-sys | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-chart, tl-ui-table, tl-ui-perf-core, tl-ui-procs | gpui-kit, tracing, smallvec, rustc-hash, serde, fluent-bundle, jiff |
| tl-ui-testkit | tl-model | gpui-kit (feature `test-support` only behind its `render` feature), async-channel, image (optional, `render` only, PNG goldens) |
| tl-ui-pagehost | tl-model, tl-ui-settings, tl-ui-base, tl-ui-kit, tl-ui-testkit | gpui-kit |
| tl-app | tl-model, tl-engine, tl-ui, tl-ops (`write_atomic`, R-130), tl-tray (P3) | zbus, toml, serde, serde_json (`themes.json` and `run-history.json`, R-245), anyhow, tracing, tracing-subscriber, tracing-appender, rustix, clap, async-channel (R-142), wgpu `=29.0.4` (Rendering GPU adapter probe, R-142) |
| tl-watch | none | libc (the low-memory watcher is std plus libc only, R-152) |
| tl-history | tl-model | zstd, crc32fast, postcard, serde, thiserror |
| tl-tray | tl-model | ksni, async-channel (R-278) |
| tl-helper | tl-model, tl-ops, tl-collect (shared strings scanner `proc/strings.rs`, R-157) | rustix, postcard, serde |
| tl-netmon | tl-model | decided in its own ADR when that phase starts |
| xtask | none | serde, serde_json, anyhow |

The tl-ui-* rows are reserved now, like the P3 rows; each crate is created by its extraction bead (ADR-020). Their external lists are what each crate's files used on 2026-11-01, found by grep. An extraction bead that finds another crate from the tl-ui row in use adds it to this table and to `xtask/src/check_deps.rs` in the same commit. Any crate outside the tl-ui row (the tl-model row for tl-ui-settings) needs an ADR change. After the split the tl-ui row covers only the shell; CS-E10 writes its final list. Until CS-D2, the UI settings and layout types stay in tl-model.

**Dependency direction rule.** Edges only point toward `tl-model`:

```text
tl-app -> tl-ui -> tl-model
tl-app -> tl-engine -> {tl-collect, tl-ops, tl-history} -> tl-model
tl-gpu-probe, tl-helper, tl-tray -> tl-model (tl-helper and tl-app also -> tl-ops; tl-helper also -> tl-collect)
```

Hard rules:
1. `tl-ui` never declares a direct dependency on `tl-engine`, `tl-collect`, `tl-ops`, `asupersync`, `arc-swap` or `zbus`. It talks to the engine only through the `EngineClient` trait in `tl-model` (ADR-005). Rule 1 and the allowed-dependency table are about direct dependencies in our own `Cargo.toml` files; what an allowed crate pulls in does not count (`zbus` reaches `tl-ui` through `gpui-pre-linux` and `gpui-base`, `arc-swap` through `gpui-component`; R-259). Rule 1 applies to every `tl-ui-*` crate as well (ADR-020).
2. `asupersync` appears in the normal dependency closure of `tl-engine` and `tl-app` only. No public item of `tl-engine` names an asupersync type. `tl-engine`'s public API is `EngineConfig`, `Engine::start`, `EngineHandle` (implements `EngineClient`) and `ShutdownReport`.
3. `tl-model`, `tl-collect` and `tl-ops` contain no async code and no runtime.
4. `unsafe` is denied workspace-wide (`unsafe_code = "deny"`). It is allowed only in these files, each with `#![allow(unsafe_code)]` at file level and a `// SAFETY:` comment per block: `crates/tl-ops/src/sys.rs`, `crates/tl-engine/src/gpu/spawn.rs`, `crates/tl-app/src/main.rs` (environment setup before any thread starts), `crates/tl-gpu-probe/src/api_versions.rs` (`ash` and `khronos-egl` entry points for the `--api-versions` mode, R-143), `crates/tl-gpu-probe/src/nvml_raw.rs` (only the raw `nvmlDeviceGetJpgUtilization` and `nvmlDeviceGetOfaUtilization` calls from nvml-wrapper-sys 0.10.0, each in a safe function: nvml-wrapper 0.13.0 has no safe API for them, and the probe runs in its own process, so a fault there cannot reach the app; R-292, bead hjn.42). Adding a file to this list needs an ADR change.

**"No Tokio" definition.** None of these crates appear in the resolved normal or build dependency graph of any shipped binary (`tasklens`, `tl-gpu-probe`, `tl-helper`, `tl-netmon`) for target `x86_64-unknown-linux-gnu`: `tokio`, `tokio-util`, `tokio-stream`, `tokio-macros`, `async-std`, `async-global-executor`, `actix-rt`, `reqwest`, `hyper`. GPUI's own executor, smol, async-io, async-executor and blocking are allowed. A banned crate that is only a dev-dependency is allowed.

**CI check: `cargo xtask check-deps`.** It runs and fails on any violation:

1. For each banned crate `C`:
   `cargo tree --workspace -e normal --target x86_64-unknown-linux-gnu -i C`
   Fail if any output line's first word is `C`. No output means the crate is absent; the exit status is not used (R-259).
2. `cargo metadata --format-version 1`. Fail if a member's direct normal or build dependencies (`[dependencies]`, `[build-dependencies]`; the same set as `cargo tree --depth 1 -e normal`) include anything outside its row in the table above, or if `asupersync` is in the normal-dependency closure of any member other than `tl-engine` and `tl-app`. Transitive dependencies of allowed crates are not checked against the rows or rule 1 (R-259). Only normal edges are checked; dev-dependencies are outside the rows.
3. `rg -n "asupersync::" crates/ --glob '!crates/tl-engine/**'` must return nothing.

### Consequences

- The engine runtime can be replaced by changing `tl-engine` only.
- The UI can be tested against a fake engine and replaced by an eframe UI crate without touching the engine.
- `tl-ops` is shared by the unprivileged engine and the P3 root helper, so privileged code paths are the same code.

### Rejected alternatives

- **Five crates as in PLAN.md.** Rejected: NVML must be in a separate process (ADR-007), and process operations must be shareable with the helper.
- **A full magpie-style sampler process.** Rejected per Astra section 3: adds serialization and resync cost without improving UI responsiveness.
- **Relying on the exit status of `cargo tree -i tokio`.** Rejected: check 1 uses the `-i` form but reads its output lines, never its exit status (R-259).
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
rustix = { version = "1.1.5", features = ["fs", "process", "net", "event", "system", "thread", "time", "pipe", "mm", "param"] }  # mm: mlockall (R-152); param: clock_ticks_per_second, page_size (R-142)
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
bitflags = { version = "2", features = ["serde"] }
smallvec = { version = "1.15.2", features = ["union", "const_generics"] }
rustc-hash = "2.1.3"
futures-lite = "2.6.1"
ureq = { version = "3", default-features = false, features = ["rustls"] }   # tl-engine public IP only; exact pin set by the bead that adds it (R-131)
fluent-bundle = "0.15"   # tl-ui strings (R-132); exact pins set by the bead that adds them
unic-langid = "0.9"
clap = { version = "4", default-features = false, features = ["std", "derive", "help", "usage", "error-context"] }  # tl-app arguments (APP-052, R-142)
jiff = { version = "0.2", default-features = false, features = ["std", "tz-system", "tzdb-zoneinfo"] }   # tl-ui local time; no libc (R-142)
unicode-segmentation = "1.12"   # tl-ui grapheme handling (R-142)
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
test-hooks = []                   # render counters, paint record (ADR-013)
bench = ["gpui-kit/profiler", "test-hooks"]     # frame histograms for --bench (ADR-014); the paint record sits behind test-hooks (R-160)
```

`tl-app` has `[features] bench = ["tl-ui/bench"]`. Release packages are built without `bench`; perf gate runs use a separate build with it. The binary package is `tl-app` with `[[bin]] name = "tasklens"`; every crate has `version = "0.1.0"` and `publish = false`; there is no `rustfmt.toml` (R-158).

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

[profile.perf]
inherits = "release"
lto = "thin"
codegen-units = 16
debug = "line-tables-only"
incremental = false

[profile.profiling]
inherits = "perf"
debug = true
```

The `perf` profile is for `cargo xtask bench` and for profiling: it builds much faster than fat LTO. Packaging (PKGBUILD) still builds `--release`. Rule: the two arms of a bench A/B comparison must be built with the same profile. `cargo xtask bench --release` runs the bench on the fat-LTO build when someone wants packaging numbers.

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
- `cargo tree -p tl-engine -e normal,features -i asupersync` shows no `nightly-outcome-try` and no `test-internals` in the non-dev build (`normal` excludes the dev-dependencies, which enable `test-internals` for the `LabRuntime` tests; R-257).
- `cargo tree -p tl-app -i gpui-pre --prefix none` shows exactly one version, `0.3.7`.

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
src/ids.rs        # ProcessKey, DeviceIdx, SeriesId, MetricId, GroupKey, NodeId, ColumnId, GraphTarget, ProcessSeriesId (R-154)
src/validity.rs   # Validity, FieldValidity
src/series.rs     # Sample, SampleFlags, SeriesRing, SeriesView
src/snapshot.rs   # Snapshot and component types
src/process.rs    # ProcessRecord, ProcessMeta, ProcState, ProcFlags, groups, ProcessExtras, ProcessExtrasP2, AppCgroupSample (R-141, R-163), LowMemoryInfo (R-150)
src/table.rs      # TableQuery, ProcessTableView, TableRow, TableTotals, SysTableQuery, SysNodeId, SysColumnId (R-155, R-170)
src/devices.rs    # DeviceCatalog, DeviceIdentity
src/system.rs     # CpuTotals, MemoryTotals, DiskTotals, NetTotals, GpuTotals, SensorReading
src/health.rs     # SourceId, SourceHealth, SourceState, EngineCaps, SetupState (R-141, R-149)
src/actions.rs    # ActionId, ActionRequest, ActionKind, ActionTarget, ActionState, ActionEvent, ActionError
src/command.rs    # Command, DeviceCommand, KWinCommand, SetupStep, HistoryRange, HistoryWhat, CommandError, AppRequest (ADR-005, R-136, R-139)
src/tray.rs       # TrayModel, TrayEvent (P3, R-161)
src/details.rs    # DetailsSnapshot and the Properties payload types
src/demand.rs     # Demand and its parts (ADR-008)
src/settings.rs   # Settings types (ADR-011); no file I/O
src/units.rs      # formatting: IEC bytes, rates, percent, durations
src/wire.rs       # length-prefixed postcard framing (ADR-007), used by probe and helper
src/gpu_proto.rs  # GPU probe messages (ADR-007)
src/helper_proto.rs # helper requests and replies (ADR-006)
src/journal.rs    # JournalQuery, JournalHandle, JournalEvent (ADR-005)
src/history.rs    # HistoryView, RecordSink (ADR-012)
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

How the UI shows each state (exact strings). Table cells, header totals and sidebar readouts use the glyphs in the "Text cell" column. Performance stats grids, Information rows and Properties key/value fields use the words from PERF-079 [now PERF-013] instead ("Collecting…", "Restricted", "Needs setup", "Not supported", "Not reported", "Unavailable", "Unknown"). Every surface uses the tooltips below.

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
    // added for the Performance pages (R-170); 50, 51 and 60 were taken
    pub const CPU_POWER_PKG: Self = Self(5);  pub const CPU_POWER_CORE: Self = Self(6);
    pub const CPU_CLOCK_MAX: Self = Self(7);  pub const MEM_DIRTY: Self = Self(14);
    pub const GPU_MEM_SHARED: Self = Self(47); pub const GPU_MEM_CLOCK: Self = Self(48);
    pub const GPU_PCIE_RX: Self = Self(49);   pub const GPU_PCIE_TX: Self = Self(52);
    pub const GPU_FAN: Self = Self(53);       pub const GPU_ENGINE_MEMCTL: Self = Self(54);
    pub const GPU_ENGINE_JPEG: Self = Self(55); pub const GPU_ENGINE_OFA: Self = Self(56);
    pub const GPU_ENGINE_OTHER: Self = Self(57); // busiest vendor-specific engine, named by GpuDeviceInfo
    pub const FAN_PWM: Self = Self(58);       pub const BATTERY_POWER: Self = Self(61);
    pub const MEM_AVAILABLE: Self = Self(15);
    // per-engine GPU series (R-292); 70 to 79 are kept for GPU engines, 57 stays unused
    pub const GPU_ENGINE_COMPUTE: Self = Self(70); pub const GPU_ENGINE_DMA: Self = Self(71);
    pub const GPU_ENGINE_RENDER: Self = Self(72);  pub const GPU_ENGINE_COPY: Self = Self(73);
    pub const GPU_ENGINE_VIDEO: Self = Self(74);   pub const GPU_ENGINE_VIDEO_ENHANCE: Self = Self(75);
    // system pressure for the status bar and device history (ADR-016 "System pressure", ADR-017);
    // CPU_LOAD1 and PSI_CPU_SOME on the CpuPackage device, the other three on the Memory device
    pub const CPU_LOAD1: Self = Self(8);      pub const PSI_CPU_SOME: Self = Self(9);   // load1; percent
    pub const PSI_MEM_SOME: Self = Self(16);  pub const PSI_MEM_FULL: Self = Self(17);  // percent (avg10)
    pub const SWAP_IN: Self = Self(18);       // pages per second, from the pswpin delta
    // ALL lists 51 metrics. Bit 15 (0x8000) marks the 1 Hz tier view of a metric:
    pub const TIER_1HZ: u16 = 0x8000;
    pub fn with_tier_1hz(self) -> Self; pub fn is_tier_1hz(self) -> bool; pub fn base(self) -> Self;
    // metric numbers stay below 0x8000
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
    Aggregate(AggregateKind),                     // Disks, Nets, Gpus: the card series of the Disk, Network and GPU pages (R-154)
}
pub enum AggregateKind { Disks, Nets, Gpus }
pub struct DeviceEntry {
    pub idx: DeviceIdx, pub identity: DeviceIdentity, pub generation: u32,
    pub display_name: Arc<str>, pub present: bool,
    pub parent: Option<DeviceIdx>,  // partitions and device-mapper children point to their disk
    pub persistent_key: Arc<str>,   // PERF-005 key computed by the worker (R-155): disk:<ID_WWN or ID_SERIAL> with by-path fallback,
                                    // fan:<hwmon name>:<device path>:<n>, bat:<NativePath>, net:<MAC or name>, gpu:<uuid or PCI slot>,
                                    // aggregate:disk, aggregate:net, aggregate:gpu
    pub net_kind: Option<NetKind>,  // network interfaces only
    pub hot_c: Option<f32>,         // P2: Hot threshold (ADR-016): hwmon temp<n>_crit, else temp<n>_max; NVML slowdown threshold for NVIDIA GPUs
}
pub struct DeviceCatalog { pub version: u64, pub entries: Arc<[DeviceEntry]> }
```

A device that disappears keeps its `DeviceIdx` with `present = false`. A device that comes back with the same identity gets the same index and `generation + 1`. Counters are never subtracted across generations. The three `Aggregate` devices exist from session start with `present = true`; the reducer computes their series from the per-device values: `DISK_ACTIVE` is the maximum over present physical disks, `DISK_READ` and `DISK_WRITE` are sums; `NET_RX` and `NET_TX` are sums over the interfaces whose `NetKind` is shown under `EngineSettings.show_network_*`; `GPU_UTIL` is the maximum and `GPU_MEM` the sum (R-154). AMD and Intel GPUs use `Gpu { uuid: [0; 16], pci_bus_id }` (R-166). GPU devices are created on first sight (`devices.ensure` per identity); there is no fixed GPU list in P1 (R-155).

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

The P1 extra-column values live beside the records, index aligned (same length, same order), so `ProcessRecord` stays small (R-141):

```rust
pub struct ProcessExtras {
    pub cpu_kernel_pct: f32, pub cpu_user_pct: f32, pub cpu_relative_pct: f32,
    pub utime_ticks: u64, pub stime_ticks: u64, pub start_ticks: u64,
    pub rss_peak_bytes: u64, pub rss_shared_bytes: u64, pub commit_bytes: u64, pub vsize_bytes: u64, pub vsize_peak_bytes: u64,
    pub page_faults: u64, pub page_faults_delta: u64, pub major_faults: u64,
    pub policy: SchedPolicy, pub rt_prio: u8, pub affinity: CpuSet, pub handles: Option<u32>,
    pub exe_name: Arc<str>, pub exe_path: Arc<str>, pub exe_deleted: bool, pub unit: Option<UnitRef>,
    pub state_letter: u8, pub frozen: bool, pub disk_read_bps: f32, pub disk_write_raw_bps: f32, pub flag_since_ns: TimeNs,
}
// P2 columns, filled only for Demand.detail_keys (R-163): exe mtime and size, ELF class and machine, sessionid,
// attr/current, elevation and sandbox, scheduling policy text, oom_score, oom_score_adj, TTY, cwd, context switches,
// power_w: Option<f32> (reducer estimate, PROC-034)
pub struct ProcessExtrasP2 { /* fields listed in bead E05-031 */ }
pub struct AppCgroupSample { pub group: GroupKey, pub anon_bytes: u64, pub swap_bytes: u64, pub io_read_bps: f32, pub io_write_bps: f32,
                             pub rios: u64, pub wios: u64, pub pids: u32, pub cgroup: CgroupRef, pub validity: FieldValidity }
```

The Priority and State column texts are formatted in the UI from `policy`, `nice`, `rt_prio`, `state_letter` and `frozen`. Start time is `SystemSnapshot.boot_time_s + start_ticks / CLK_TCK`. Columns whose data is in neither the record nor the extras sort `Absent` (R-141).

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

/// One aggregated row of the process table (R-141). The numeric mirrors of ProcessRecord sit beside these.
pub struct GroupRow {
    pub node: NodeId, pub kind: GroupRowKind, pub name: Arc<str>, pub icon: Option<Arc<str>>,
    pub cpu_pct: f32, pub memory_bytes: u64, pub disk_bps: f32, pub net_bps: f32, pub gpu_pct: f32,
    pub pids: u32, pub note: Option<Arc<str>>, pub validity: FieldValidity,
}
pub enum GroupRowKind { App, Unit, KernelThreads, Pseudo }
```

`GroupKind` classifies; `NodeId` (ADR-009) identifies table rows and has `Unit(UnitRef)`, `KernelThreads` and `Pseudo(PseudoKind { Interrupts, MemoryCompression })` beside `Group` and `Process`.

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
    pub details: Arc<DetailsSnapshot>, // P2: Properties payloads, handle search state, affinity masks
    pub history: Arc<HistoryView>,     // P3: answer to the latest HistoryQuery (ADR-012)
    pub process_series: Arc<ProcessSeriesSet>,     // P2: engine-sampled per-process, name and app series (R-154)
    pub device_details: Arc<DeviceDetailsSnapshot>, // P2: static and D-Bus device details, SMART, partitions, eject (R-139)
    pub device_history: Arc<DeviceHistory>,         // P2: device metric history for the Overview ranges and timeline (ADR-017)
}

pub struct SeriesSet {
    pub header: ComponentHeader,
    pub series: Arc<[Arc<SeriesView>]>, // sorted by SeriesId; unchanged series reuse their Arc
}

pub struct ProcessSnapshot {
    pub header: ComponentHeader,
    pub records: Arc<[ProcessRecord]>,  // sorted by ProcessKey
    pub extras: Arc<[ProcessExtras]>,   // index aligned with records (R-141)
    pub extras_p2: Arc<[ProcessExtrasP2]>, // index aligned; filled only for detail_keys (R-163)
    pub apps: Arc<[AppCgroupSample]>,   // one per app group (R-141)
    pub coverage: Coverage,             // which expensive fields cover which keys (ADR-008)
    pub low_memory: Option<Arc<LowMemoryInfo>>, // EngineMode::LowMemory only (R-150)
}

pub struct SeriesSet { /* as above */ pub slow: Arc<[Arc<SeriesView>]> } // 1 Hz tier views with the TIER_1HZ bit set, from P2 (R-141, R-170)

pub struct ProcessSeriesSet { pub header: ComponentHeader, pub series: Arc<[(ProcessSeriesId, Arc<SeriesView>)]>, pub slow: Arc<[(ProcessSeriesId, Arc<SeriesView>)]> } // both sorted by id; slow holds the 1 Hz views (get, get_slow)
pub struct ProcessSeriesId { pub target: GraphTarget, pub column: ColumnId }
pub enum GraphTarget { Process(ProcessKey), Name(Arc<str>), App(Arc<str>) }

pub struct ComponentMask(pub u16); // one bit per Snapshot component, computed by the UI
// bits 0..=12: SYSTEM, SERIES, PROCESSES, TABLE, CONNECTIONS, SERVICES, STARTUP, INVENTORY, DEVICES,
// HEALTH, WINDOWS, DETAILS, HISTORY; PROCESS_SERIES = 1 << 13, DEVICE_DETAILS = 1 << 14,
// DEVICE_HISTORY = 1 << 15 (P2, ADR-017); ALL = 0xFFFF (sixteen bits)
```

**Per-component fields the UI reads** (each type lives in `snapshot.rs` unless named):

- `SystemSnapshot` carries, besides the totals of `system.rs`, `boot_time_s: i64` (`/proc/stat btime`, R-141), `color_scheme: u8` (portal `org.freedesktop.appearance` `color-scheme` 0, 1, 2, read by `opener::watch_color_scheme`, which the `SystemdUser` worker runs on its session connection, and shipped as `SourceUpdate::ColorScheme`, R-170, R-244) and `cpu_topology: Arc<CpuTopology>`:
  `online: CpuSet`, `cores: Arc<[CoreInfo { id, logical: SmallVec<[u16; 2]>, l3_group: u16, kind:
  CoreKind::{Performance, Efficiency, Standard} }]>`, read once at start and again on CPU hotplug
  (Set Affinity dialog, UI-PWN-021; per-core grid marks, PERF-031).
- `DetailsSnapshot` (`details.rs`, bit `DETAILS`): `header`, `targets: FxHashMap<DetailTarget,
  Arc<TargetDetail>>` where `DetailTarget::{Process(ProcessKey), App(GroupKey)}` and
  `TargetDetail` holds one `Option<Arc<TabPayload>>` per `DetailTab` (`General`, `Threads`, `Fds`,
  `Maps`, `Security`, `Environment`, `Strings`, `AppGeneral`, `AppProcesses`) with its own
  `version` and `validity`; `affinity: FxHashMap<ProcessKey, AffinityInfo { mask: CpuSet, cpuset_limit:
  CpuSet }>` for the keys in `Demand.affinity_targets`; `handle_search: Option<Arc<HandleSearchState
  { search: SearchId, phase, progress, chunks: Arc<[HandleHit]>, searched, total, skipped }>>`.
  Unchanged payloads reuse their `Arc`. The `ProcessDetail` worker fills it at
  `processes.interval_ms`, except `smaps` at most once per second and Environment only on open
  and `DetailRefresh` (UI-PWN-002).
- `ServicesSnapshot`: `rows: Arc<[SysRow]>` with `query_generation` (the engine sorts, filters and
  flattens, UI-SYS-002), `units` (each with `cgroup_pids: Arc<[ProcessKey]>`, `has_cgroup_procs`
  and resource fields with `Validity`), `counts { total, running, stopped }`, `managers: [ManagerState
  { state: Connected | Unavailable | Reconnecting }; 2]`, `details: Arc<FxHashMap<UnitRef,
  Arc<UnitDetails>>>` for the units in `Demand.unit_details` (`UnitDetails` holds the General,
  Log On, Recovery (`recovery: RecoverySettings`, `recovery_dropin_exists`) and Dependencies
  (`deps: Arc<[DepEntry]>`) fields of SYS-011 to SYS-014), `backend: Systemd | OpenRc`.
- `StartupSnapshot`: `rows`, `query_generation`, `entries` (SYS-019 fields plus `impact_detail`,
  `can_toggle`, `can_delete`, `can_run`, `overridden_by_user`), `counts { total, enabled, disabled }`.
- `ConnectionsSnapshot`: `rows`, `query_generation`, `records` with engine-formatted address
  strings (`Arc<str>`, formatted once per cookie), `counts { total, tcp, udp, established,
  listening }`, `destroy_supported: Option<bool>` (set after the first `SockDestroy` answer).
- `HistoryView` (ADR-012): the answer to the latest `HistoryQuery`, max 1,024 points per series.
- `HealthSnapshot`: a plain `version: u64` (no `ComponentHeader`, R-155), one `SourceHealth` per `SourceId` with `last_error` capped at 200 characters, `helper: HelperState`, `helper_start: Option<(u64 /* seq */, HelperStartOutcome { Cancelled, NotAuthorized, Failed(Arc<str>) })>` (R-148), `setup: Arc<SetupState { status: Option<SetupStatus>, run: Option<SetupRun> }>` (R-149) and `caps: EngineCaps` (bitflags u16: `FILE_MANAGER1 = 1`, `GDB = 2`, `CONNECTIONS_DESTROY = 4`, `KWIN_BRIDGE = 8`; R-141). `SourceState` is `{ Off, Running, Late, Stale, Failed, Disabled { until: TimeNs }, Unsupported }`; `Unsupported` is terminal and never retried (GPU probe not installed, `LibraryNotFound`, `DriverNotLoaded`, KWin bridge not available; R-155). There is no `SourceId` for the public IP fetch or the low-memory sweep.
- `WindowsSnapshot.pick: Option<PickResult { seq: u64, outcome: PickOutcome { Picked { pid: u32 }, NoWindow, Cancelled } }>` carries the window picker result (R-149).
- `InventorySnapshot` holds one struct per Information section (R-156), `public_ip: PublicIpState { NotFetched, Fetching, Fetched { v4: Option<Arc<str>>, v6: Option<Arc<str>>, fetched_at_s: i64 }, Failed(Arc<str>) }` (R-139) and `ProcessorInfo.bits: InvField<u8>` (64 for `x86_64`, `aarch64`, `riscv64`, `ppc64`, `ppc64le`, `s390x`, `loongarch64`, `mips64`, else 32; R-170). Inventory rows arrive with `device: None`; the reducer matches disk name, PCI slot or interface name against the catalog (R-166). `MemoryInfo.peak_committed_bytes` is the reducer's running maximum of `Committed_AS` (R-166).
- `DeviceDetailsSnapshot { header, cpu: Arc<CpuStaticInfo>, memory: Arc<MemoryDetails>, disks: Arc<[(DeviceIdx, DiskDetails)]>, nets: Arc<[(DeviceIdx, NetDetails)]>, batteries: Arc<[(DeviceIdx, BatteryDetails)]> }` (R-139). `DiskDetails` carries `model`, `wwn`, `serial`, `interface`, `scheduler` as `InvField<Arc<str>>`, `rotation_rpm`, `smart: SmartKind { Ata, Nvme, NeedsFallback, None }`, `smart_data: Option<InvField<Arc<SmartData>>>`, `smart_temp_c`, `ejectable`, `can_power_off`, `eject: Option<(u64, EjectState { Running, Blocked { blockers: Arc<[Blocker]>, others_may_hold: bool }, Done, Failed(Arc<str>) })>`, `partitions: Arc<[PartitionRow]>` (R-140), `formatted_bytes`, `used_bytes`, `free_bytes`, `system_disk`, `swap`. `NetDetails` carries `adapter_name`, `driver`, `mac`, `ifindex`, `link_speed_mbps`, `state: LinkState`, `addresses`, `kind: NetKind`, `wifi: Option<WifiInfo>`. `BatteryDetails` is the E12-023 field list plus `history: Option<Arc<[(i64, f32)]>>`. The reducer merges one record per device: the sysfs source fills every field it knows and a D-Bus source (`Udisks`, `NetworkManager`) overwrites `model`, `wwn`, `serial`, `rotation_rpm`, `smart*`, `ejectable`, `can_power_off`, `eject`, `kind` and `wifi`.
- Totals in `system.rs` (R-139, R-155, R-170): `CpuTotals.throttling` and `GpuTotals.throttling: Metric<bool>` (P2, ADR-016: Intel `thermal_throttle` counts, NVML clocks event reasons; `Unsupported` where no source exists); `DiskTotals.temp_c: Metric<f32>`; `NetTotals.state: LinkState { Up, Down, Dormant, Unknown }` from sysfs `operstate`; `GpuTotals.pcie_rx_bps`, `pcie_tx_bps: Metric<f32>`; `GpuTotals.engines: GpuEngines` (busy percent per `GpuEngine`, `Unsupported` for an engine the device does not report) and `fdinfo_partial: bool` (R-292); `SensorReading.fan_control: Option<FanControlMode { FullSpeed, Manual, Automatic }>` from `pwm<n>_enable` 0, 1, 2 and `fan_connected: bool` (a non-zero RPM seen this session, or a label).

```rust
// tl-model/src/table.rs: one query type for the three system tables (UI-SYS-002)
pub enum SysTable { Services, Startup, Connections }
pub struct SysTableQuery { pub table: SysTable, pub generation: u64, pub sort: (SysColumnId, bool /* ascending */),
                           pub filter: Arc<str>, pub expanded: Arc<[SysNodeId]>,
                           pub services: Option<ServiceFilter> }   // no column list; the Services filter matches every text column (R-167)
/// String ids for the system tables, separate from the process ColumnId (R-170): the 19 Services columns
/// (UI-SYS-003), the 7 Startup ids (name, publisher, status, impact, type, location, command) and the
/// 17 Connections ids (proto, local, foreign, state, pid, process, user, family, local_port, foreign_port,
/// recv_q, send_q, interface, inode, service, remote_host, netns), with as_str and from_str.
pub struct SysColumnId(/* ... */);
pub enum SysNodeId { ServiceGroup(Manager), Unit(UnitRef), UnitProcess(UnitRef, ProcessKey),
                     Startup(StartupRef), Conn(u64) }
pub struct SysRow { pub node: SysNodeId, pub depth: u8, pub expanded: bool,
                    pub has_children: bool, pub record: Option<u32> }
```

`WindowsSnapshot` holds the KWin window list keyed by pid. Each entry has `internal_id`, `caption`, `resource_class`, `desktop_file_name`, `minimized`, `maximized`, `keep_above`, `frame_geometry` and `window_type`. Without the KWin bridge it is empty with validity `Unsupported`.

`ColumnId` numbers 1 to 121 follow PROC-027 to PROC-036 order and are frozen; the dense index is the number minus 1, `ColumnSet([u64; 4])` holds 256, and new columns append after 121 (R-155).

Every component except `series` is replaced as a whole new `Arc` when it changes. A publish that changes only CPU series reuses every other `Arc`.

**Units and formatting (`units.rs`).** Canonical table (d) in `docs/RECONCILIATION.md` is the full list. In short:
- Percent: one decimal everywhere (`%.1f%%`: `3.4%`, `37.0%`); a measured zero is `0%`. The same rule holds for every rate, power and time form: a measured zero prints as its unit's zero, never as a blank (R-155). `percent_2dp` zero is `0%`; `watts_pair` prints the limit as an integer.
- Memory and storage sizes: IEC, base 1024, labels `B KiB MiB GiB TiB`, two decimals from 1 KiB up (`1.22 GiB`, `16.00 KiB`); integer bytes below 1 KiB (`512 B`). Copy details lines use whole MiB.
- Disk and I/O rates: `MiB/s` with one decimal in tables; auto-scaled IEC (`KiB/s`, `MiB/s`, `GiB/s`) with one decimal on Performance graphs and stats.
- Network rates: setting `units.network = "bits" | "bytes"`, default `"bits"`, and `units.network_auto_scale`, default `false`. Bits use SI, base 1000 (`Kbps Mbps Gbps`); bytes use IEC (`KiB/s`, `MiB/s`). With auto-scale off, values are fixed `Mbps` (or `MiB/s`) with one decimal, `0 Mbps` at zero. Graph axes use 1-2-5 × 10ⁿ rounding whatever the setting.
- Counts: digits grouped with a thin space (U+2009), `12 345`.
- Frequencies: integer `MHz` (`4631 MHz`).
- Temperatures: integer `°C`. Power: one decimal `W`.
- Uptime: `2 days, 2h 38m 15s`. Short ages (tooltips and the Start time column's age text only): `1d 4h`, `4h 12m`, `12m 5s`, `5s`.
- Formatting functions write into a caller-provided `String` and never allocate when the buffer is large enough.
- Axis helpers `ceil_pow2(u64) -> u64` and `ceil_125(f64) -> f64` live here too (`ceil_125(1234) = 2000`, `ceil_125(60) = 100`, `ceil_125(0.3) = 0.5`, `ceil_pow2(3 MiB) = 4 MiB`; R-170). `format_local(LocalDateTime)` stays; the local time zone conversion is done in tl-ui with jiff (R-142).

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
- A test serializes every `MetricId` constant and compares against a checked-in list (`crates/tl-model/tests/frozen/metric_ids.txt`, read by `tests/frozen_ids.rs`); changing a number fails the test. New constants are appended to the list (ADR-017 adds `CPU_LOAD1=8`, `PSI_CPU_SOME=9`, `PSI_MEM_SOME=16`, `PSI_MEM_FULL=17`, `SWAP_IN=18`).
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

Never `current_thread()`. Never `RuntimeHandle::spawn_blocking`. Blocking work other than periodic samples (actions, the launcher, the opener, the journal) uses `Cx::spawn_blocking(..)` and is joined with `TaskHandle::join(&mut self, &cx)`.

**Sampling threads** (owner decision 2026-10-08, tasklens-wlf.30). Periodic samples do not use the blocking pool. Each periodic source has its own sampling thread, started at its first sample and asleep between samples. One source does a whole sweep in one sample (never one job per file). A sample that never returns, such as a drive query on an empty card reader (TMOG feedback #1004), stalls only its own source: its health goes `Late`, and every other source keeps its rate. With samples on the shared 4-thread pool, 4 hung samples stopped every graph. When the worker ends, its sampling thread ends; a thread stuck in a sample is never joined, so it never holds up shutdown, and it ends when that sample returns. Under `LabRuntime`, which has no blocking pool, samples run inline.

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
    pub control: watch::Receiver<WorkerControl>, // reset_generation plus the newest-64 QueuedCommand queue (R-166)
    pub shared: SharedHandles,                   // ManagerConnSlot and the other D-Bus handles owned by dbus/mod.rs (R-166)
}
```

Never move an `mpsc::Receiver` into a restartable worker. Never hold a `watch` `Ref` across any other channel call; use `borrow_and_clone()` (bead `c11v6v`).

`EngineConfig { mode, sys_root, state_dir, history_dir: Option<PathBuf>, helper_path: Option<PathBuf> }` is the whole start-up configuration; tl-app sets `history_dir = $XDG_STATE_HOME/tasklens/history` and `helper_path = /usr/lib/tasklens/tl-helper` when that file exists (R-150). `EngineHooks.action_gate: Fn(HookCtx) -> Box<dyn ActionGate>` is the test seam for action admission (R-166). `all_workers()` is the one registry of `WorkerSpec` lines (R-167).

**Engine modes.** `EngineMode::{Full, LowMemory}` is fixed at start. `LowMemory` is used by the
`tasklens --low-memory` rescue process (PROC-098, UI-PWN-032): the `Processes` worker runs with
a stripped sweep (`stat`, `status`, cgroup `memory.current`) every 2 s plus the PSI state, the `KWin`
worker runs only for `KWinCommand::KeepAbove` (the window starts pinned), and the engine publishes
`Snapshot.processes` and `Snapshot.health` only (R-152). The sweep's `LowMemoryInfo` travels in
`ProcessSweep.low_memory` and lands in `ProcessSnapshot.low_memory` (R-150). The actions region and the
targetless launcher run in both modes, so "Open Full tasklens" is an ordinary `Launch`. With
`--force-full-memory-mode` the rescue process runs `EngineMode::Full` instead (D-015). The stripped sweep is its own `Source` in `workers/lowmem.rs`, chosen by one `match` on `EngineMode` in `workers/processes.rs` (R-138). Actions still go through the
ADR-006 pipeline with identity revalidation.

**Worker list** (each is one `ManagedSupervisor`; `SourceId` is the enum in `tl-model::health`):

| SourceId | Module | Phase | Work |
|---|---|---|---|
| `Cpu` | `workers/cpu.rs` | P1 | `/proc/stat`, cpufreq, CPU temperature; from P2 also `/proc/pressure/cpu` and `/proc/loadavg` at 1 Hz (ADR-016 "System pressure") |
| `Memory` | `workers/memory.rs` | P1 | `/proc/meminfo`, zram `mm_stat`; from P2 also `/proc/pressure/memory` and `pswpin` from `/proc/vmstat` (ADR-016 "System pressure"; R-162 still holds for every other `/proc/vmstat` field) |
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
| `MetricHistory` | `workers/metric_history.rs` | P2 | ADR-017 device metric tiers, segment files, event detector and its journal fetches |
| `Journal` | `workers/journal.rs` | P2 | one `journalctl` child per open `JournalHandle` (ADR-005, UI-SYS-036) |

`opener.rs` (E14-020) holds the portal colour-scheme reader (`org.freedesktop.portal.Settings` `Read("org.freedesktop.appearance", "color-scheme")` plus `SettingChanged`); the `SystemdUser` worker runs `opener::watch_color_scheme` as a child task of each session-bus generation in `EngineMode::Full`, and it ships `SourceUpdate::ColorScheme(u8)` (R-170, R-244). The public IP fetch is a job the `Inventory` worker runs on `DeviceCommand::FetchPublicIp`; the helper session and the setup runner report through `SourceUpdate::Helper` and `SourceUpdate::Setup`, which `source_of()` maps to `None` (R-150). Every `SourceUpdate` variant exists from E08 on, so later epics add arms, not variants. `Source::take_extra()` lets a worker send device lists and details before its sample (R-139).

**KWin bridge worker.** `workers/dbus_kwin.rs` runs on the engine's session-bus connection. Availability: `XDG_CURRENT_DESKTOP` contains `KDE` and `org.kde.KWin` is on the bus. Otherwise the window features are hidden and the UI shows the toast "Window features need KDE Plasma". The worker:
- writes `$XDG_RUNTIME_DIR/tasklens/kwin-bridge.js` and loads it as KWin script plugin `tasklens-bridge`;
- exports object `/io/github/oysteinkrog/Tasklens/WindowBridge`, interface `io.github.oysteinkrog.Tasklens.WindowBridge`, method `Update(s json)`, which the script calls with the window list;
- publishes the list as `Snapshot.windows` (ADR-003); the script also reports `minimizedChanged`, maximize changes, `keepAboveChanged` and `interactiveMoveResizeFinished` (R-167);
- writes its connection's unique name into the script as `__TL_BUS_NAME__` and accepts `Update` only from the owner of `org.kde.KWin`; payloads over 1 MiB are rejected and strings cut at 4 KiB (R-167);
- runs commands (activate, close, keep above, pick) from `Command::KWin(KWinCommand)` (ADR-005) as one-shot scripts named `tasklens-cmd-<n>`; the window picker uses `tasklens-pick`, and its result comes back through the `Picked(s uuid, s pid)` method of the `WindowBridge` interface (pid as decimal text, parsed as `u32`; `"0"` or unparsable text is `NoWindow`, R-241) into `WindowsSnapshot.pick` (R-149). Commands have no result event; a failed one-shot script is logged at `warn` (R-170);
- finds tasklens's own secondary windows (detached graph windows "tasklens Graph <n>", the mini window "tasklens Mini Window") in `WindowsSnapshot` by tasklens's pid plus the window caption. Every tasklens window keeps the one app ID `io.github.oysteinkrog.Tasklens`; there are no per-window app IDs.

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
        let done = sampler.start(src)?;                         // whole sweep, on this source's thread
        let (s, update) = wait(&cx, done, p * LATE_COUNT).await?; // Late if still running; never start a second job
        src = s;
        io.updates.send(&cx, EngineMsg::Source(update)).await?;
    }
}
```

- `IDLE_RECHECK = 500 ms`: an off source wakes twice a second to read demand, does no I/O, and costs nothing measurable.
- Demand changes take effect at the next wake (at most 500 ms, or one period).
- A job that runs past its period is not interrupted. The worker skips the missed ticks and the health becomes `Late` after 3 consecutive jobs longer than 0.8 × period, or as soon as one job has run for 3 periods. The next job that ends within 0.8 × period sets it back to `Running`.
- The `Source` trait lives in `tl-engine` and wraps synchronous `tl-collect` samplers:

```rust
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration>; // None = off
    fn sample(self) -> (Self, SourceUpdate) where Self: Sized;   // runs on the source's sampling thread
    fn take_extra(&mut self) -> Option<SourceUpdate> { None }   // device lists and details; drained after each sample and sent first (R-139)
}
```

**D-Bus workers.** Each D-Bus worker generation opens its own `zbus::Connection` with `.internal_executor(false)` and spawns a child task that drives it:

```rust
loop {
    let _ = time::timeout(cx.now(), Duration::from_millis(500), conn.executor().tick()).await;
    if cx.checkpoint().is_err() { break; }
}
// Release, with cancellation masked: keep ticking for the owner's last calls until it drops its
// handle, at most 500 ms, then close the connection and tick until the executor is empty, at
// most 200 ms.
while !owner_dropped() && waited() < Duration::from_millis(500) {
    let _ = time::timeout(cx.now(), Duration::from_millis(20), conn.executor().tick()).await;
}
let _ = time::timeout(cx.now(), Duration::from_millis(200), async {
    let _ = conn.clone().close().await;
    while !conn.executor().is_empty() { conn.executor().tick().await; }
}).await;
```

The tick task closes the connection on every path out of a generation, whether the owner drops its handle first or the region is cancelled first. A dropped match-rule stream makes zbus queue a `RemoveMatch` task on the connection's own executor, and that task holds a clone of the connection. Without the close, nothing runs that task, the socket stays open, and the bus queues every signal for it against the user's quota until dbus-broker disconnects the user's other peers (y6k.33).

Every zbus call and every signal-stream `next()` is wrapped in `time::timeout` (2 s for reads, ADR-006 for actions), followed by `cx.checkpoint()`. Proxies never outlive their generation. On reconnect a worker re-lists everything (full resync) before applying signals.

**The reducer** (`crates/tl-engine/src/reducer.rs`) is the only code that writes the published slot.

```rust
pub enum EngineMsg {
    Source(SourceUpdate),                 // from workers
    Command(Command),                     // from the UI (ADR-005)
    ActionFinished { id: ActionId },      // from action tasks; triggers a targeted refresh
    Tick,                                 // every 1 s: stale checks, linger expiry (R-166)
}
```

Loop: `recv(&cx)` one message, then drain with `try_recv` until empty (at most 256 messages), apply all, then publish once. It never awaits between `latest.store(..)` and `wake_tx.try_send(())`. It owns: all `SeriesRing`s, the process table state, the current `TableQuery`, metadata caches, the device catalog and health. It builds `ProcessTableView` (sort, filter, group, tree flatten) whenever processes or the query change. It also computes the aggregate device series and the per-process series for `Demand.graph_targets` (R-154), the process power estimate (R-163), `TableTotals.handles` from `ProcessSweep.file_nr` and the running maximum of `Committed_AS` (R-166). `Command::Device` is forwarded to the `Udisks`, `Inventory` or `UPower` worker (R-139); `StartHelperSession` and `EndHelperSession` go through `Reducer.helper_tx: Option<mpsc::Sender<HelperCommand { Start, End }>>`, which E29 sets (R-148).

Reducer state is bounded: process metadata cache holds live processes plus at most 256 exited ones. Exited records are kept for the linger time, `2 × processes.interval_ms` clamped to 1..=3 s, then removed.

**Shutdown** (`EngineHandle::shutdown(self) -> ShutdownReport`), called on the main thread after GPUI has stopped (ADR-010):
1. Drop the UI-side senders (done by dropping the `EngineHandle` internals).
2. `rt.drain_root_region(Duration::from_millis(1000))`.
3. Drop every `RuntimeHandle` clone.
4. `rt.shutdown_timeout(Duration::from_millis(500))`.
5. If step 2 returned `TimedOut` or step 4 returned `false`, log a warning and return; `tl-app` then calls `std::process::exit(0)` without running further destructors.

Blocking jobs that walk many items or wait on a child process check their job's cancel flag between items, or at least every 20 ms while they wait, and return early with an incomplete result that is never published. The GPU probe's `Hello`, `Sample` and `--api-versions` waits and the event detector's journal parse do this (tasklens-cr0.30). A single system call that is still running is not interrupted. The warning in step 5 names the session tasks and blocking jobs still running.

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
    /// Open a journal stream for one unit (Service Properties Logs tab, UI-SYS-036). The engine
    /// runs `journalctl` (ADR-016) and batches entries at most every 100 ms. Dropping the handle
    /// kills the child.
    fn journal(&self, query: JournalQuery) -> JournalHandle;
}

pub enum CommandError { Busy, EngineStopped }

/// Sent by tl-app's org.freedesktop.Application handler to the UI over async_channel::bounded(4) (ADR-010, R-136).
pub enum AppRequest { Activate { token: Option<Arc<str>> }, SelectTab(Tab), SelectPid(u32), Quit }

pub struct JournalQuery { pub unit: UnitRef, pub follow: bool }           // last 200 entries of this boot first
pub struct JournalHandle { /* async_channel receiver plus a control sender */ }
impl JournalHandle {
    pub fn events(&self) -> async_channel::Receiver<JournalEvent>;
    pub fn load_older(&self, from: Option<Arc<str>>); // 200 entries before cursor `from` (the UI's oldest line); None: before the oldest the engine sent
    pub fn set_follow(&self, on: bool); // off kills the `-f` child; on resumes with `--after-cursor`
}
pub enum JournalEvent { Batch { entries: Arc<[JournalEntry]>, older: bool, exhausted: bool },
                        Access(JournalAccess), Error(JournalError) }
pub struct JournalEntry { pub realtime_us: u64, pub priority: u8, pub pid: Option<u32>,
                          pub message: Arc<str>, pub cursor: Arc<str> }
pub enum JournalAccess { All, OwnOnly }
pub enum JournalError { NotFound, Timeout, Exit { code: i32, stderr_first_line: Arc<str> } }
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
    SetSysTableQuery(SysTableQuery),   // P2: Services, Startup Apps, Connections (UI-SYS-002)
    Action(ActionRequest),
    RefreshNow(SourceId),         // user pressed refresh, or a page opened
    ResetBaselines,               // debugging aid, also used after resume
    ReprobeSource(SourceId),      // P3: after a setup step succeeded (UI-PRF-053)
    KWin(KWinCommand),            // P2: activate, close, keep above, pick { seq }, cancel pick { seq } (ADR-004 KWin worker, R-149)
    Device(DeviceCommand),        // P2: SMART, eject, public IP, battery history (R-139)
    StartHelperSession, EndHelperSession, // P3: the shield starts or ends the pkexec helper (R-148)
    // P2: Properties windows (UI-PWN-002)
    DetailRefresh { target: DetailTarget, tab: DetailTab },
    ComputeHashes(ProcessKey),    // once per window open; result in the General payload
    VerifyPackage(ProcessKey),
    StringScan { scan: ScanId, key: ProcessKey, opts: StringScanOpts },
    CancelStringScan(ScanId),
    // P2: Search File Handles (UI-PWN-030)
    HandleSearch { search: SearchId, pattern: Arc<str>, include_maps: bool, privileged: bool },
    CancelHandleSearch(SearchId),
    // P3: history (ADR-012, UI-PRC-030)
    HistoryQuery { id: u64, range: HistoryRange, what: HistoryWhat },
    HistoryDelete { keys: Arc<[ProcessKey]> },
    // P3: Additional Setup window (UI-PRF-053)
    SetupStatus,                  // engine refreshes HealthSnapshot.setup.status (R-149)
    RunSetup { step: SetupStep }, // runs tasklens-setup through pkexec; progress and output in HealthSnapshot.setup.run (R-149)
}

/// Keyed by the kernel disk name (`sda`, `nvme0n1`), the same string as `DeviceIdentity::Disk.name`;
/// the UI resolves a DeviceIdx through DeviceCatalog, workers never see one (R-139).
pub enum DeviceCommand {
    SmartRead { disk: Arc<str>, update: bool },   // udisks2 SMART, optionally refresh first
    SmartReadAdmin { disk: Arc<str> },            // pkexec tasklens-smart fallback (R-088, ADR-018)
    Eject { seq: u64, disk: Arc<str> },           // seq is a UI counter; progress in DiskDetails.eject
    FetchPublicIp,                                // Inventory worker job; result in InventorySnapshot.public_ip
    BatteryHistory { native_path: Arc<str> },
}
pub enum SetupStep { RaplEnable, RaplRevert, SensorsDetect, SensorsRevert, PolkitRule { on: bool } } // args(): rapl-enable, rapl-revert, sensors-detect, sensors-revert, polkit-rule on|off
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
    Thread { process: ProcessKey, tid: u32 },  // P2: Threads tab actions (ADR-006)
    Cgroup(CgroupRef),                         // app cgroups under app.slice
    Unit(UnitRef),                             // services; freeze, thaw and kill of units outside app.slice
    Startup(StartupRef),
    Socket { cookie: u64, family: Family, proto: Proto, four_tuple: FourTuple }, // P3: SockDestroy
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
    Dump { mode: DumpMode, name: Arc<str> },   // P2: Create dump file; a file name only, the engine picks the directory (ADR-018)
    // services and startup
    UnitStart, UnitStop, UnitRestart, UnitReload, UnitResetFailed,
    UnitEnable, UnitDisable, UnitMask, UnitUnmask,
    UnitWriteRecovery(RecoveryEdit),           // P3: drop-in with only the changed keys (SYS-013)
    UnitRemoveRecovery,                        // P3: deletes the tasklens drop-in
    StartupEnable, StartupDisable, StartupRunNow, StartupDelete,
    SockDestroy,                               // P3: target ActionTarget::Socket (SYS-031)
    // session-bus openers and the shared launcher (R-134); targets is empty for these three
    OpenUri(Arc<str>),                         // portal OpenURI, else xdg-open
    ShowInFileManager { path: Arc<str>, properties: bool }, // FileManager1 ShowItems / ShowItemProperties
    Launch(LaunchSpec),                        // Run dialog, Startup "Run now"
}

pub struct LaunchSpec { pub what: LaunchWhat, pub as_admin: bool, pub cwd: Option<Arc<str>> }
pub enum LaunchWhat { Command(Arc<str>), DesktopEntry(Arc<str>), Unit(UnitRef),
                      DbusActivate { bus_name: Arc<str>, object_path: Arc<str>, action: Arc<str>, parameter: Option<Arc<str>> } } // org.freedesktop.Application.ActivateAction, 2 s (P3, R-170)

pub enum Signal { Term, Kill, Hup, Int, Quit, Stop, Cont, Usr1, Usr2, Other(u8) }  // Other: any other number, incl. SIGRTMIN..=SIGRTMAX
pub enum SchedPolicy { Other, Batch, Idle, Fifo, Rr }   // rt_prio 1..=99 for Fifo and Rr, else 0
pub struct CpuSet(pub [u64; 16]);      // up to 1024 CPUs
pub enum IoPriority { None, Realtime(u8), BestEffort(u8), Idle }   // levels 0..=7; None = IOPRIO_CLASS_NONE, "Default (follows nice)"

pub enum ActionState {
    Rejected(RejectReason),    // never started, nothing changed
    Started,                   // admitted and running
    Succeeded,
    Failed(ActionError),       // the OS or service reported failure; nothing changed
    OutcomeUnknown(Arc<str>),  // timed out or target vanished after the effect may have happened
    Progress { bytes: u64 },   // not final: a running dump's file size, about every 500 ms
    Saved(Arc<str>),           // succeeded and wrote this full path on the engine's host (Dump)
}

pub enum RejectReason { Busy, TooManyTargets, IdentityChanged, KernelThread, Protected, NotOwner, Unsupported,
                        AlreadyRunning } // StartupRunNow and Launch when the entry already runs: not an error, not a success (R-148)

pub struct ActionEvent {
    pub id: ActionId,
    pub target: Option<u16>,   // index into the request's targets; None for the whole request
    pub state: ActionState,
}
```

`OpenUri`, `ShowInFileManager` and `Launch` carry no targets; admission skips the target checks for them and they run in `tl-engine/src/opener.rs` and `launcher.rs` on the engine's session-bus connection with a 2 s deadline (120 s when `as_admin`), reporting through the same events (R-134). A `pkexec` launch counts as `Succeeded` when the child is still running 2 s after the prompt closed; the prompt counts as closed when the child's `/proc/<pid>/stat` comm is no longer `pkexec` (pkexec execs the program in place) or the child has exited (R-253); Plasma scripts in Run go as `LaunchWhat::Command` with the shell-quoted path (R-148). `Unsupported` is reported when no `FileManager1` owner exists or the command text is invalid.

Event order per request: `Started` (or a single `Rejected`), then for each target any `Progress` events followed by one final state, then one final state with `target: None` summarizing (Succeeded only if every target succeeded, where `Saved` counts as success; otherwise the worst state in the order Failed, OutcomeUnknown, Rejected). `Progress` never decides the summary.

Dumps report progress through action events, on every host (ADR-018): while a dump runs, the engine reads the size of its output file every 500 ms and sends `Progress { bytes }` for that target. A progress event never waits for room in the queue; one that does not fit is dropped. The request carries only the file name; the engine joins it to its own `<state_dir>/dumps/`, refuses a name with a path separator or `..`, and ends the target with `Saved(full path)`, which a one-target dump's summary keeps. A path on a host is always chosen by that host's engine. The UI never reads the file (UI-PWN-034). Service jobs have no client-side timeout (SYS-005); every other action keeps the ADR-006 deadlines.

**UI handling.** `ActionTracker` (entity in `tl-ui/src/action_tracker.rs`, R-129) keeps `FxHashMap<ActionId, PendingAction>` and shows toasts with `Notification::...id1::<ActionToast>(id.0)` so updates replace the same toast. Pending entries older than the action's deadline plus 5 s become "Outcome unknown". Destructive actions are never retried automatically.

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
- `SetSched { policy, rt_prio }` (P2): `sched_setattr` on every TID, same rules. Every FIFO and RR request sets `SCHED_RESET_ON_FORK` (R-165); the Realtime menu item is `SCHED_RR` priority 1, after the confirmation "Realtime priority can freeze the desktop if the process spins. Set it anyway?".
- `SetAffinity(set)`: `sched_setaffinity` on every TID, same rules.
- `SetIoPriority(p)`: `ioprio_set(IOPRIO_WHO_PROCESS, tid, value)` through `libc::syscall(SYS_ioprio_set, ..)` in `tl-ops/src/sys.rs`, on every TID, same rules. `Realtime` needs root and is shown only in the privileged menu.
- TIDs are re-listed right before the loop. A TID that disappears mid-loop is skipped and not counted as failure.
- `ActionTarget::Thread { process, tid }`: `open_verified(process)`, then check that `/proc/<pid>/task/<tid>` exists (else `Rejected(IdentityChanged)`), then run the call on that one TID and poll the pidfd as above. Signals to a thread use `tgkill(pid, tid, sig)`. Allowed kinds: `Signal`, `SetNice`, `SetSched`, `SetAffinity`.
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

**Units outside `app.slice`.** `FreezeGroup`, `ThawGroup` and `KillGroup` on an `ActionTarget::Unit` (user units in `session.slice` or `background.slice`, and system units) go through the owning manager: `FreezeUnit(name)`, `ThawUnit(name)` and `KillUnit(name, "all", SIGKILL)` on `org.freedesktop.systemd1.Manager`, user manager over the session bus, system manager over the system bus. Calls set `ALLOW_INTERACTIVE_AUTHORIZATION`, so polkit `org.freedesktop.systemd1.manage-units` can prompt for system units. Freeze completion is the unit's `FreezerState` property reading `frozen` within 2 s; otherwise `OutcomeUnknown("freeze still in progress")`.

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
| systemd unit start/stop/restart | no client-side timeout for the job (SYS-005); the method call itself 10 s on the user manager, 120 s on the system manager (R-166) |
| systemd enable/disable + `Reload` | 10 s |
| Any call that may show a polkit prompt | 120 s |
| Startup entry write | 2 s |

A timeout reports `OutcomeUnknown`, never `Failed`. Work already running on the blocking pool is not interrupted (ASX section 6).

**systemd units.** Methods `StartUnit(name, "replace")`, `StopUnit(name, "replace")`, `RestartUnit(name, "replace")`, `EnableUnitFiles([name], false, true)` / `DisableUnitFiles([name], false)` then `Reload()`. Calls set the D-Bus `ALLOW_INTERACTIVE_AUTHORIZATION` flag so polkit can ask for a password. Success is the `JobRemoved` signal with result `"done"`; any other result string is `Failed(UnitJob { result, error_name: None })`, and a failed method call is `Failed(UnitJob { result, error_name: Some(<zbus error name>) })` (R-170). systemd `AccessDenied` maps to `Failed(NotAuthorized)` (R-148). `UnitEnable` and `UnitDisable` on a masked unit call `UnmaskUnitFiles` first inside the same action (R-170).

**Privileged helper boundary (P3, reserved now).**
- Binary `/usr/lib/tasklens/tl-helper`, started by the engine as `pkexec /usr/lib/tasklens/tl-helper --session`.
- Polkit has exactly three tasklens actions, in `/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy`: `io.github.oysteinkrog.tasklens.helper` (`auth_admin_keep`) covers every tl-helper operation; `io.github.oysteinkrog.tasklens.setup` (`auth_admin_keep`) covers `/usr/lib/tasklens/tasklens-setup`; `io.github.oysteinkrog.tasklens.netmon` is reserved for `tl-netmon` (P4). Polkit authorises the `pkexec` launch once, so per-operation action names inside a running helper would not be enforced.
- Transport: the helper's stdin and stdout, a Unix socketpair so a file descriptor can travel as `SCM_RIGHTS` ancillary data with the `Dump` frame (R-145), using the same `tl-model::wire` framing as the GPU probe (ADR-007), max frame 64 KiB. Frames are `HelperFrame { seq: u32, request }` and `HelperReply { seq: u32, last: bool, body }` after a `Hello { proto }` handshake both ways, `HELPER_PROTO_VERSION = 1` (R-157). A reply larger than one frame is sent as a sequence of frames; the 4 MiB cap is per reply message for the streamed requests (`ScanStrings`, `NetnsSockDiagDump`) and total per request for the rest. There is no single-call `--once` mode.
- Allowed requests only:
  - `Signal { key, signal }`, `SetNice`, `SetSched`, `SetAffinity`, `SetIoPriority`; `Signal`, `SetNice`, `SetSched` and `SetAffinity` also accept a thread target (`ActionTarget::Thread`);
  - `FreezeGroup`, `ThawGroup`, `KillGroup` for any uid's cgroup under `/sys/fs/cgroup`;
  - `ReadProcessFiles { key, files }`, where `files` is a set of `io`, `fd_list`, `fdinfo`, `environ`, `maps`, `smaps`, `smaps_rollup`, `ns`, `cwd`, `exe`, `status`, `root` (`ProcFiles::ROOT = 1 << 11`, R-157);
  - `ScanStrings { key, opts }`, which streams batches, and `CancelScan { seq }` (17 request variants in all, R-157);
  - `Dump { key, mode }`, which runs `/usr/bin/gdb` by fixed path with the arguments `--nx --nh --batch -p <pid> -ex "gcore /proc/<caller pid>/fd/<n>"`, a cleared environment except `PATH=/usr/bin:/bin` and `LC_ALL=C`, stdio on `/dev/null`, writing to the file descriptor passed with the frame; `dump_available()` requires both `/usr/bin/gdb` and `/usr/bin/gcore` (R-145). The engine's direct path opens `<state_dir>/dumps/<name>.core` with `O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC` mode 0600 and calls the same `tl_ops::dump::dump_to_fd`;
  - `ResolveSocketInodes { inodes }`;
  - `SockDestroy { family, proto, four_tuple, cookie }`;
  - `WriteServiceDropIn { manager: System, unit, keys }` with keys limited to `Restart*`, `StartLimit*`, `FailureAction` and `OnFailure`, written to the fixed path `/etc/systemd/system/<unit>.d/50-tasklens-recovery.conf`; an empty key set deletes that file (`ActionKind::UnitRemoveRecovery`);
  - `NetnsSockDiagDump { netns_ino }`.
- No path, command or free-form file argument. The helper re-runs `open_verified` itself on every process target; it never trusts the engine's check.
- The helper exits after 300 s idle or when stdin reaches EOF. It sets `PR_SET_PDEATHSIG = SIGKILL`.
- The eBPF network collector is a separate binary (`tl-netmon`, P4) with its own polkit action. It never shares a process with the action helper.
- "Run as administrator" in the Run dialog uses `pkexec env … /bin/sh -c <text>` under `org.freedesktop.policykit.exec`, never tl-helper. Unset or empty `DISPLAY`, `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are left out of the `env` line (R-165).
- Inside the engine the helper is reached through `trait PrivilegedOps` in `tl-engine/src/actions/mod.rs` (`call(cx, req, fd, deadline) -> Result<Vec<HelperReply>, HelperCallError { Missing, Cancelled, NotAuthorized, StartFailed(Arc<str>), Timeout, Exited, Protocol(Arc<str>) }>` (one enum, imported by `helper/mod.rs`; `StartFailed` is a `Call` whose implicit session start failed and maps to `Failed(Internal(reason))`, R-246) and `state() -> HelperState`); `ActionCtx.helper` is `None` until the helper lands, and a privileged request is then `Rejected(Unsupported)`, never run unprivileged (R-147). A dismissed prompt is `ActionError::AuthCancelled` (pkexec 126), a denial `ActionError::NotAuthorized` (pkexec 127); a failed session start is reported once through `HealthSnapshot.helper_start` (R-148).
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

**API versions mode.** `tl-gpu-probe --api-versions` is a second, one-shot mode. It loads Vulkan through `ash` and EGL through `khronos-egl` (both loaded at run time), and per GPU reads the Vulkan `apiVersion` and driver properties (matched by `VK_EXT_pci_bus_info`) and the OpenGL `GL_VERSION` from an EGL context on the matching DRM device (`EGL_EXT_platform_device`). It writes one postcard frame (`ApiVersions { devices: Vec<GpuApiVersions> }`, keyed by PCI bus ID) to stdout and exits 0. The engine runs it once at startup on the blocking pool with a 5 s deadline, in its own process, never in the NVML sampler process. A crash, timeout or non-zero exit gives `Unknown` for those fields and is not retried.

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
| `Sample` deadline missed | give the child up (below), return `Err` (supervisor restarts with backoff) | `Stale`, then `Failed` |
| Child exit or crash (EOF, `waitpid` status) | check for its status every 10 ms for up to 100 ms, then give it up; return `Err` | `Failed` |
| `tl-gpu-probe` not installed | health `Unsupported` with `last_error` "GPU probe not installed"; no child is spawned; the worker parks (checkpoint loop every 60 s) | `Unsupported` (terminal, never retried; R-155, R-252) |
| `InitFailed { LibraryNotFound | DriverNotLoaded }` | health `Unsupported` with `last_error` `LibraryNotFound` or `DriverNotLoaded`; the worker shuts the child down and parks (checkpoint loop every 60 s); no retry | `Unsupported` (terminal, never retried; R-155, R-252) |
| `InitFailed { NoPermission | Other }` | return `Err` (normal backoff) | `Failed` |
| Per-metric `Unsupported` | none | that field `Unsupported` |

Restart backoff and the five-per-minute rule come from ADR-004.

**Giving a child up.** A probe child has exactly one owner at a time (tasklens-cr0.30). When the client gives it up (cancel, deadline, protocol error, an exit, or a drop), it moves the `Child` by value into a reaper, a short-lived `tl-gpu-reap` thread, so no other code can signal or wait on that pid again. The reaper sends SIGKILL, then calls `waitpid` until the child is reaped, retrying on `EINTR`; any other `waitpid` error is logged at warn and ends the reaper. No blocking job and no engine thread ever waits for a child to exit. A killed probe can take seconds to exit while the driver tears down its GPU context (3.6 s measured), and a child stuck in uninterruptible kernel sleep cannot be reaped at all; either way only the reaper thread waits. If the reaper thread cannot start, the killed child stays a zombie until the engine exits.

**Rates.** Device totals at 1 Hz while any GPU view, sidebar mini graph or tray graph is demanded; process data at 1 Hz only when a GPU process column is visible. Never faster than 1 Hz; the Refresh setting does not change the probe rate.

**Shutdown.** The worker sends `Shutdown` and checks every 10 ms, for up to 200 ms, whether the child has exited; a child that still runs then is given up. A cancelled worker (engine shutdown) skips the `Shutdown` request and the 200 ms: it gives the child up at once. A request in flight returns `ProbeError::Cancelled` within 20 ms of the cancel (tasklens-cr0.30).

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
    pub details: Arc<[DetailDemand]>,       // P2: open Properties windows, max 16 (UI-PWN-002)
    pub affinity_targets: Arc<[ProcessKey]>,// P2: open Set Affinity dialog, max 256 (UI-PWN-021)
    pub service_cgroup_stats: bool,         // P2: a unit resource column is shown (UI-SYS-013)
    pub unit_details: Arc<[UnitRef]>,       // P2: open Service Properties windows, max 10 (UI-SYS-030)
    pub remote_host_cookies: Arc<[u64]>,    // P4: visible rows of the Remote host column (UI-SYS-069)
    pub connections_all_netns: bool,        // P4: Namespaces "All" (UI-SYS-070)
    pub graph_targets: Arc<[(GraphTarget, ColumnId)]>, // P2: process, name and app graphs, max 16 (GRAPH_TARGETS_MAX, R-154)
    pub tray: TrayDemand,           // P3
    pub recording: bool,            // history recording enabled (P3)
    pub device_history: bool,       // P2: history.device_metrics is on (ADR-017); independent of visibility
    pub device_history_range: Option<DeviceHistoryRequest>, // P2: an Overview range served from ADR-017 (UI-PRF-055, UI-PRF-057)
    pub device_timeline: bool,      // P2: a timeline box is visible (UI-PRF-057)
}

pub struct DeviceHistoryRequest { pub range: DeviceRange, pub end_realtime_ns: Option<i64> } // None = now; Some = paused at that time
pub enum DeviceRange { M1, M5, H1, H24, D7 } // M1 and M5 are requested only while paused

pub struct DetailDemand { pub target: DetailTarget, pub tab: DetailTab }
pub struct TrayDemand { pub icons_shown: bool, pub cpu: bool, pub memory: bool, pub disks: bool, pub nets: bool, pub gpus: bool } // icons_shown counts as a visible surface (R-156)
```

**Schedule.** "Visible" means some visible surface (window page, sidebar mini graph, detached graph, tray icon) displays it.

| Source | Visible rate | Not visible, recording off | Not visible, recording on | Not visible, device history on |
|---|---|---|---|---|
| CPU total and per core | 10 Hz if `cpu_fast`, else 1 Hz | off | 1 Hz | 1 Hz |
| CPU frequency, CPU temperature | 1 Hz | off | 1 Hz | 1 Hz |
| Memory, swap, zram | 1 Hz | off | 1 Hz | 1 Hz (with PSI and swap-in) |
| Disks, network interfaces | 1 Hz | off | 1 Hz | 1 Hz |
| GPU totals (NVML) | 1 Hz | off | 1 Hz | 1 Hz |
| CPU and memory while `process_table` is true | 1 Hz (the table header needs them, R-166) | | |  |
| Sensors (hwmon), fans | 1 Hz | off | off | 1 Hz |
| Process sweep (CPU, memory, I/O, state) | setting `processes.interval_ms`, default 1000, range 250..=10000 | off, unless `graph_targets` is non-empty: then `processes.interval_ms` (PROC-093, R-154) | same as visible rate | as recording off |
| Per-process series (`graph_targets`) | one sample per sweep per target, summed over matching processes for `Name` and `App`, gap when none matches; columns `cpu`, `memory`, `disk`, `network`, `gpu`, `gpu_mem_dedicated` only | same | same | as recording off |
| PSS/USS for `detail_keys` | 1 Hz, only if a PSS or USS column is shown | off | off | off |
| GPU per process | 1 Hz, only if a GPU process column is shown | off | 1 Hz if recording includes GPU | off |
| Connections and socket attribution | every 2 s | off | off | off |
| systemd units | event-driven; full resync on connect and every 60 s while the page is visible | events only, no resync | same | events only, no resync |
| Startup entries | on page open and on `RefreshNow`; while the Startup Apps tab is visible, inotify on the autostart directories and the two Plasma script directories plus the user manager's `UnitFilesChanged` signal trigger a rescan | off | off | off |
| Inventory | on page open and on `RefreshNow` | off | off | off |
| Properties payloads (`details`) | `processes.interval_ms` for the demanded tab; `smaps` at most 1 Hz; Environment on open and `DetailRefresh` only | off | off | off |
| Affinity masks (`affinity_targets`) | 1 Hz while the dialog is open | off | off | off |
| Unit cgroup stats (`service_cgroup_stats`) | 1 Hz while a resource column is shown | off | off | off |
| Unit details (`unit_details`) | `GetAll` once, then `PropertiesChanged` | off | off | off |
| Reverse DNS (`remote_host_cookies`) | on demand, at most 4 lookups at once | off | off | off |
| Logind sleep signal | event-driven, always on | always on | always on | always on |
| Device enumeration (disks, nets, hwmon) | every 5 s while any of those is demanded | off | every 5 s | every 5 s |

`device_history` (ADR-017) is true whenever `history.device_metrics` is on, so with the default settings the "device history on" column applies whenever the engine runs. When several columns apply, each source runs at the fastest rate any of them gives. The PSI, load and swap-in reads of ADR-016's "System pressure" row follow the Memory and CPU rows.

While recording is off, the process sweeps that run because the Processes tab is open also feed the 10-minute rolling window (ADR-012, D-010). The window adds no sweeps of its own.

PSS/USS sort and GPU-per-process sort over partial coverage are disabled: the header for PSS and USS is not clickable and shows the tooltip "Only measured for visible rows". `ProcessSnapshot.coverage` lists which keys have PSS/USS so the UI shows `WarmingUp` for others.

**Hidden detection.** `any_surface_visible = false` when the main window has had no render for 1000 ms while the hub had a pending notify (ADR-010 explains), and no detached graph or tray icon is shown. Hidden with recording off and device history off means every periodic source is off; only logind and the D-Bus executor ticks remain.

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

- Idle cost with the window hidden and device history off is a few timer wakeups per second. With device history on (the default) the hidden engine keeps the 1 Hz CPU, memory, disk, network, GPU and sensor reads of that column.
- Graphs for 1 Hz sources move smoothly because the chart interpolates in time (ADR-009), not because of fast polling.

### Rejected alternatives

- **10 Hz for every system source.** Rejected: CPU cost with no visible benefit (Astra section 3).
- **Lowering the process rate automatically when slow.** Rejected per Astra: report `Late` instead.

### Verification

- LabRuntime tests with fake sources: demand switches change the observed sample times exactly (10 Hz to 1 Hz to off) within one `IDLE_RECHECK`; hidden with recording off and device history off produces zero `sample()` calls over 60 virtual seconds; hidden with device history on gives exactly the 1 Hz sources of that column.
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
app.rs            tl_ui::run(UiLaunch) -> UiExit; TasklensUi::init(cx, client: Arc<dyn EngineClient>, init: UiInit); AppEnv Global (R-153, R-171)
hub.rs            SnapshotHub (Entity, one per app)
demand.rs         DemandAggregator (Entity, one per app)
anim.rs           AnimationDriver (Entity, one per window)
action_tracker.rs ActionTracker (Entity, one per app) (R-129)
actions.rs        window-level GPUI Actions and bind_keys_global; table/actions.rs and pages/<page>/actions.rs hold the rest
i18n.rs           Fluent strings: tr(key), tr_args(key, args), Strings Global, strings_version; catalogue assets/locales/en-US.ftl (R-132)
theme/            TlTheme, built-in themes, mapping onto gpui-kit Theme
window/main.rs    MainWindow (Entity): title bar, tab strip, active page, status bar
pages/performance/{mod.rs, sidebar.rs, cpu.rs, memory.rs, gpu.rs, disk.rs, net.rs, sensors.rs}
pages/processes/{mod.rs, table.rs, header.rs, filter.rs, menu.rs}
pages/{services.rs, startup.rs, connections.rs, information.rs}
chart/{element.rs, view.rs, geometry.rs, grid.rs, scale.rs}
table/{tree_table.rs, columns.rs, columns_window.rs, selection.rs, cells.rs, actions.rs}   # the column picker is table-generic (R-181)
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
pub struct ChartSeries { pub id: SeriesId, pub process: Option<ProcessSeriesId>, pub fill: Option<Hsla>, pub line: Hsla, pub line_width: Pixels } // process: per-process series, read from Snapshot.process_series (R-154)
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
pub enum NodeId { Group(GroupKey), Process(ProcessKey), Unit(UnitRef), KernelThreads, Pseudo(PseudoKind) } // R-141
pub enum PseudoKind { Interrupts, MemoryCompression }
pub struct TableRow { pub node: NodeId, pub depth: u8, pub expanded: bool, pub has_children: bool,
                      pub record: Option<u32>, pub group: Option<u32>,   // indexes into records/groups
                      pub descendants: u32, pub subtree: Option<u32> }   // subtree row count and the subtree-total index (R-141)
pub struct ProcessTableView {
    pub header: ComponentHeader,
    pub query_generation: u64,
    pub rows: Arc<[TableRow]>,
    pub groups: Arc<[GroupRow]>,       // aggregated values, no double counting of descendants
    pub subtree: Arc<[GroupRow]>,      // subtree totals of parent rows; TableRow.subtree indexes here (R-288)
    pub totals: TableTotals,           // two-line header numbers: cpu_pct, memory_pct, swap_pct, disk_pct, net_bps, net_link_bps, gpu_pct, vram_pct, threads, handles, one Total<T> each (R-141)
    pub total: u32, pub shown: u32, pub restricted: u32,   // status text counts (R-141)
}
// TableQuery also carries include_children: bool (R-141).
```

- Group totals sum each process once; in tree mode a parent row shows its own values, and the collapsed parent shows its subtree sum in secondary text.
- Missing parent (PPID not visible or exited): the process is a root. Reparenting moves it at the next sweep.
- Filter: the shell debounces input 100 ms (the one debounce, R-188), then sends `SetTableQuery` with a new generation. The UI shows a view only if `query_generation` equals the latest sent; until then it keeps the previous rows.
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
                               // disk_fill, disk_line, net_fill, net_line, grid, chart_bg (13 fields)
}
```

`chart_bg` is the plot background of every chart and mini graph (measured `#1D2223` in Default Dark, UI-FND-001); the values of every field are in `docs/ui/foundation.md` UI-FND-001.

- Built-in themes ship as `assets/themes/builtin.json` in TaskSlinger's format `{"themes":[{"name":..,"SLOT":"#RRGGBB"}]}` plus `accent` and `graph.*` keys. "Default Dark" (PLAN.md section 9 palette) and "Default Light" ship first from hand-written values, because the `theme.light` default needs it; E17-002 writes those two by hand at P0, and E17-005 adds all ten named palettes once the owner exports the file from TaskSlinger (D-012, R-210). Share codes import the tasklens `TL1` format first and TaskSlinger `TS1` when a sample code exists (D-013). User themes load from `$XDG_CONFIG_HOME/tasklens/themes.json` in the same format.
- Custom tasklens views (tree-table, charts, sidebar, tab strip, title bar content) read `TlTheme` directly (a GPUI `Global`).
- Kit widgets (menus, dialogs, inputs, tooltips, toasts, buttons, scrollbars) read gpui-kit `Theme`. `theme/map.rs::apply(&TlTheme, &mut gpui_kit Theme)` sets: `background ← window_bg`, `foreground ← text_primary`, `muted_foreground ← text_secondary`, `border ← divider`, `title_bar ← titlebar`, `popover ← menu_bg`, `accent ← menu_hover`, `primary ← accent`, `ring ← accent`, `input ← input`, `scrollbar ← scroll_track`, `scrollbar_thumb ← scroll_thumb`, `table ← list_bg`, `table_even ← row_alt`, `table_hover ← row_hover`, `table_active ← row_selected`, `table_head ← header`, `table_row_border ← divider`, `tab_bar ← tab_bg`, `status_bar ← toolbar`, `chart_1..5 ← graph cpu/mem/gpu/disk/net lines`, `chart_grid ← graph.grid`. The bead that writes `map.rs` must check each token name against `ThemeColor` in gpui-component 0.7.0 source; a missing name maps to the nearest existing token and the mapping is recorded in a comment in `map.rs`.
- Theme switch: `Theme::update` plus replacing the `TlTheme` global, bumping `style_version` so every chart rebuilds geometry once.

**Entry point and the tl-app hand-off** (R-153, R-171). tl-app calls `tl_ui::run(UiLaunch) -> UiExit` with `UiLaunch { client, init: UiInit, open: OpenMode { Main, Gallery, LowMemory }, start: StartOptions { tab, select_pid, start_minimized, shift_held, pending_crash_report } }` (no `platform` field, R-153); it returns `UiExit::{Quit, RendererFailed(String), DeviceLost}`. `UiInit` carries `settings`, `state`, `themes_json: Option<Arc<str>>`, the persist callbacks `persist: Box<dyn Fn(&Settings, &UiState)>` and `persist_themes: Box<dyn Fn(PersistTheme)>` with `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}`, `app_requests: async_channel::Receiver<AppRequest>`, `startup_toasts` and `env: AppEnv`. `AppEnv` (a Global installed in `init` step 8) holds everything only tl-app has: `debug`, the config, state and logs directories, `build: BuildInfo` (version, git hash, build date, gpui revision, cargo-about licences JSON), `pending_crash: Option<CrashReport>`, `mark_crash_seen`, `write_settings_now`, `set_autostart(enabled, minimized)` (the `Exec` line gets `--start-minimized` when `minimized` is true, R-236), `global_shortcut: Option<GlobalShortcutApi>` (Ctrl+Esc through kglobalaccel; the settings row shows "Not available" when `None`), `import_taskslinger` (no path argument; tl-app finds the TaskSlinger files), `list_adapters` (the wgpu probe runs in tl-app; `AdapterInfo { name, backend, device_type, pci_bus_id: Option<String> }`, and `rendering.gpu` stores `pci:<bus id>` or else `name:<adapter name>`, R-237) and `run_history: Option<Box<dyn RunHistoryStore>>`. Every closure runs on the main thread and is not `Send`. Tests use `AppEnv::test()`.

**Metrics and scale.** Settings `appearance.ui_scale_pct` (75..=200, step 5, default 100) and `appearance.row_height_pct` (80..=150, step 5, default 100). With `ui_scale = ui_scale_pct / 100`: `UiMetrics { ui_scale, row_height: 28 × row_height_pct/100 × ui_scale, base_text: 13 × ui_scale, radius_small: 4 × ui_scale, radius_card: 6 × ui_scale }`. `radius_small` is for rows, menus and other small elements; `radius_card` is for cards. `appearance.rounded_corners = false` sets both to 0. The theme's `font_size` (rem) is `base_text`. GPUI applies the compositor scale (1.1 here); tasklens multiplies by `ui_scale` once. Ctrl+Plus, Ctrl+Minus and Ctrl+0 step the scale by 5 or reset it to 100, with the toast "UI scale changed to N%". A change in `window.scale_factor()`, `ui_scale_pct` or `row_height_pct` bumps `style_version` and clears `CellTextCache`.

**Fonts.** Bundle in `assets/fonts/` (SIL OFL): `NotoSans-Regular.ttf`, `NotoSans-Medium.ttf`, `NotoSans-SemiBold.ttf` (static weights, not the variable font, because of zed#64783) and `NotoSansMono-Regular.ttf` (family `"Noto Sans Mono"`, used by the Service Properties Logs tab, the Strings tab and Environment values; U-40). Load with `cx.text_system().add_fonts(..)` before the first window. Family name `"Noto Sans"`. Numeric cells and chart labels use `FontFeatures(Arc::new(vec![("tnum".into(), 1)]))`. Fallback for non-Latin process names comes from fontconfig system fonts (cosmic-text). The bead must verify that each bundled face loads (zed#61660: faces without `m` are dropped).

**Icons.** Lucide icons through `gpui_kit_assets::IconName`, plus app SVGs in `assets/icons/` served by an `AssetSource` that tries tasklens icons first and falls back to `gpui_kit::assets::Assets`. Process and app icons are file paths resolved by the engine (ADR-016) and drawn with `img(path)` at 16 × ui_scale px. Material Symbols is not used: it is a variable font with unknown `m`-glyph behaviour under zed#61660.

**Accessibility and input.** Everything works from the keyboard. The full key list is canonical table (b) in `docs/RECONCILIATION.md`. The keys that shape this ADR's design:
- Ctrl+1..Ctrl+6 and Alt+1..Alt+6 select a tab; Ctrl+Tab and Ctrl+Shift+Tab switch to the next or previous tab.
- Ctrl+F focuses the filter.
- A printable key while a table has focus (and no input or dialog has focus) starts filtering.
- Enter, Ctrl+I and Alt+Enter open Properties on a process row; Space toggles expand (D-008).
- Delete and Ctrl+E run End Task (SIGTERM); Shift+Delete and Ctrl+X run End Process (SIGKILL); both follow the confirmation rules in ADR-006.
- Ctrl+C runs "Copy row" (processes: "Copy details"); Ctrl+Shift+C runs "Copy rows as table" (ADR-015).
- Esc closes the topmost dialog or menu; else clears the filter and focuses the table; else closes the window if `window.close_with_esc` is on; else does nothing.
- Key contexts beyond the main window (Properties windows, dialogs, Handle Search, the journal list, the low-memory window) are rows of table (b) too (R-120); the owning UI file names the context on each window.

Motion settings: `motion.smooth_graphs` decides whether charts slide, except while `motion.save_power_on_battery` is on and the system runs on battery (UPower `OnBattery`): then charts do not slide and the merged demand leaves `cpu_fast` off, so CPU updates drop to 1 Hz. `motion.animations` decides tab, theme and toast motion. The system reduce-motion setting (`App::reduce_motion()`) turns all of them off. GPUI has no screen reader support on Linux [inferred]; chart values are also shown as text next to every chart.

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
3. If owned: serve `org.freedesktop.Application` (`Activate`, `Open` returns an error, `ActivateAction(name, parameter, platform_data)` with actions `quit`, `tab` (string) and `select-pid` (uint32)) at that path. Command-line arguments of a second instance are forwarded through `ActivateAction`. `Activate` forwards to the GPUI main thread as `tl_model::command::AppRequest` (R-136) through an `async_channel::bounded(4)`; a GPUI task receives it and calls `window.activate_window()` on the main window, or reopens it if closed to the tray. When the KWin bridge is up (ADR-004), it also activates the window through the bridge.
4. If the session bus is unavailable: log a warning and run without single-instance. There is no lock-file fallback.
Focus stealing prevention on Wayland may only flash the taskbar entry; that is accepted [inferred].

**Close behaviour.** The app uses `QuitMode::Explicit`, which gpui-pre 0.3.7 provides (`Application::with_quit_mode`, `app.rs:227`; R-178); `tl_ui::run` sets it before `run`. `on_window_should_close` on the main window:
- tray enabled and setting `tray.close_to_tray = true` (P3): remove the window, keep the app and engine running (demand drops to tray only), return `true`;
- otherwise: call `cx.quit()`.
After `tl_ui::run` returns `UiExit`, `main` calls `EngineHandle::shutdown(Duration::from_millis(1500))`, then flushes logs, then exits. If `Application::run` does not return on Linux [inferred that it does, U-2], the same steps run in `cx.on_app_quit`. On `UiExit::DeviceLost` tl-app relaunches itself once with `TASKLENS_RELAUNCHED=1` in the environment (R-153). Without a tray, `--start-minimized` opens the window and minimizes it, and a close that would quit minimizes instead when `ToggleMainWindow` arrives while visible (R-219).

**Tray (P3).** Crate `tl-tray` with `ksni =0.3.6`, features `async-io` and `blocking`, using `ksni::blocking::TrayMethods::spawn` (its own thread; no Tokio). `cargo xtask check-deps` proves no Tokio enters the graph. If that check fails, the fallback is a hand-written StatusNotifierItem and dbusmenu server inside `tl-tray` on its own zbus connection, never inside `tl-engine`.
- Tray graphs are drawn on the CPU into ARGB32 buffers (`ksni::Icon { width, height, data }`) at 22×22 and 44×44 px by a small rasterizer in `tl-tray/src/raster.rs` (fill columns plus a 1 px line, no anti-aliasing beyond 2×2 supersampling; `raster::render(max: f32)`). Updates at most 1 Hz, skipped when the pixels did not change. Tray data comes from `EngineClient::latest()` on a 1 Hz GPUI timer and reaches the tray through a `bounded(2)` model channel with `try_send`. The tray types `TrayModel` and `TrayEvent` live in `tl-model/src/tray.rs`; the static icon is the pre-rendered `assets/icons/tray-22.argb` and `tray-44.argb`, embedded with `include_bytes!` (R-161). Tray process graphs use `Demand.graph_targets` (R-154).
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
| Settings (user choices) | `$XDG_CONFIG_HOME/tasklens/settings.toml` (default `~/.config/tasklens/settings.toml`) |
| User themes | `$XDG_CONFIG_HOME/tasklens/themes.json`, TaskSlinger format `{"themes":[…]}` |
| State (remembered UI state) | `$XDG_STATE_HOME/tasklens/state.toml` (default `~/.local/state/tasklens/state.toml`) |
| Logs | `$XDG_STATE_HOME/tasklens/logs/` |
| Crash reports | `$XDG_STATE_HOME/tasklens/crash/` |
| History | `$XDG_STATE_HOME/tasklens/history/` (ADR-012) |
| Run dialog history | `$XDG_STATE_HOME/tasklens/run-history.json` |
| Icon path cache | `$XDG_CACHE_HOME/tasklens/icons.toml` |

Directories are created with mode `0700`; files with `0600`. Built-in themes are not on disk; they ship as `assets/themes/builtin.json` (ADR-009).

**Two files.** `settings.toml` holds user choices. `state.toml` holds what the app remembers: window size and maximized state, sort per table, columns (order, visibility, widths) per table, group expand state, last tab, Performance sidebar width and selected device, `history_mode`, minigraph layout, tray graphs and the `[lowmem] oom_score_adjust` the watcher unit accepted (R-152).

**Schema** (`tl-model/src/settings.rs`; all structs `#[serde(default)]`). Canonical table (a) in `docs/RECONCILIATION.md` is the only settings schema, for both files: every key, default and range is defined there, and a key not listed there does not exist. Both files start with `schema_version = 1`. The sections of `settings.toml` are `[general]`, `[theme]`, `[appearance]`, `[motion]`, `[units]`, `[processes]`, `[tables]`, `[properties]`, `[performance]`, `[fans]`, `[graphs]`, `[history]`, `[window]`, `[tray]`, `[startup]`, `[network]`, `[privilege]`, `[rendering]`, `[services]`, `[lowmem]` and `[debug]`. The keys other ADRs depend on:

```toml
[general]
start_tab = "processes"        # processes | performance | services | startup | connections | information | last_used

[theme]
active = "Default Dark"
follow_system = false
light = "Default Light"
dark = "Default Dark"

[appearance]
ui_scale_pct = 100             # 75..=200, step 5
row_height_pct = 100           # 80..=150, step 5
decorations = "client"         # client | server (label "Use system title bar"; restart)
heat_coloring = true
heat_intensity_pct = 50        # 0..=100

[motion]
animations = true              # tab, theme, toast motion
smooth_graphs = true           # chart sliding
save_power_on_battery = true   # on battery: CPU at 1 Hz, no chart sliding

[units]
network = "bits"               # bits | bytes
network_auto_scale = false

[processes]
interval_ms = 1000             # 250..=10000
mode = "grouped"               # grouped | tree | flat
highlight_new_exited = true
hide_system = false
subtree_totals = false

[performance]
graph_range_s = 60             # 30 | 60 | 300 | 600 (300 and 600 from P2)

[history]                       # P3
enabled = false                # D-003
retention_h = 6                # 1..=12
persist = true
max_disk_mb = 256
```

`state.toml` defaults that other ADRs depend on: `[window] width = 1280, height = 800` (clamped to at least 800 × 500); `[sort.processes] column = "pid", ascending = true` (history: `cpu` descending; services: `name` ascending; startup: `status` ascending, then name; connections: `proto` ascending, then local port); `[columns.processes] visible = ["name", "user", "pid", "cpu", "memory", "disk", "network", "gpu", "description"]` with widths Name 248, User 92, PID 96, CPU 63, Memory 92, Disk 82, Network 92, GPU 72, Description 342 (logical px at 100%), and Power usage 101 when shown. `power` and `power_trend` exist and are off by default on every machine. `memory` is RssAnon (D-004).

There is no CPU percentage mode setting (two columns instead, ADR-008) and no `filter_cmdline` setting (the filter always matches the command line, ADR-009). R-071 lists the other keys that do not exist and why.

Column ids are stable strings defined once in `tl-model::ids::ColumnId` with `as_str()`/`from_str()`; an unknown id in the file is dropped with a log line.

**Load** (same steps for each file).
1. Missing file: defaults; do not write until something changes.
2. Parse to `toml::Table`. Parse error: rename the file to `<name>.corrupt-<unix seconds>`, use defaults, show a warning toast "Settings file was unreadable and has been reset. The old file was kept as {name}".
3. `schema_version` missing: treat as 1. Lower than current: run migrations `fn migrate_vN_to_vN1(t: &mut toml::Table)` in order, then deserialize. Higher than current: deserialize what is understood, set `read_only = true`, never save, show the toast "Settings were written by a newer tasklens; changes will not be saved".
4. Values out of range are clamped and logged.
5. Unknown keys are ignored and not preserved on the next save.

There is no file watching: an external edit applies at the next start.

**Save.** Debounced 500 ms after the last change, plus on exit. Atomic write: write `<name>.tmp-<pid>` in the same directory, `fsync` the file, `rename` over the target, `fsync` the directory. The routine is `tl-ops/src/fs.rs::write_atomic`, re-exported by `tl-app/src/atomic.rs` (R-130); it is used for `settings.toml`, `state.toml`, `themes.json` and, from the engine's `Startup` worker, XDG autostart overrides (ADR-016).

**Ownership.** `tl-ui` holds settings and state in GPUI `Global`s and calls the `UiInit.persist: Box<dyn Fn(&Settings, &UiState)>` callback given by `tl-app` on every change; tl-app debounces the write 500 ms. Themes go through `UiInit.persist_themes: Box<dyn Fn(PersistTheme)>` with `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}`, and `AppEnv.write_settings_now` is the synchronous write Reset settings uses (R-153, R-171). The Run dialog history goes through `AppEnv.run_history` (`RunHistoryStore::{load, save}`), also built on `write_atomic` (R-170). Engine-relevant parts are pushed with `EngineClient::set_settings` as `EngineSettings` (derived, never persisted): `processes.interval_ms`, `units.network`, the `performance.show_network_*` keys, `history.*`, `processes.subtree_totals` and `processes.hide_system`.

### Consequences

- Settings survive column reordering and new columns because they use ids.
- A newer file is never overwritten by an older build.

### Rejected alternatives

- **JSON or RON.** Rejected: TOML is easier to hand-edit and Astra recommended it.
- **`directories` crate.** Rejected: 20 lines of code; one less dependency.

### Verification

- Tests with `tempfile`, for both `settings.toml` and `state.toml`: missing file; corrupt file renamed to `<name>.corrupt-<unix seconds>`; v0 (no version) migrated; future version read-only and never written; crash simulation (kill between write and rename) leaves either the old or new file intact; unknown keys ignored and absent after the next save.
- Test: unknown column id dropped, known ones keep order.

---

## ADR-012: History storage (P3)

### Context

The owner wants six-hour process history (TaskSlinger's history mode with a timeline) and recording while hidden. Astra calculated naive 1 Hz retention at about 11 GB and said history needs its own design. This ADR fixes what the P1 data model must provide and the storage format so P3 work does not change P1 types. History recording is off by default (D-003). While it is off, History mode shows a 10-minute rolling window kept in RAM (D-010).

### Decision

**Constraints on P1 code (binding now):**
1. All timestamps are CLOCK_BOOTTIME `t_ns` (ADR-003), and every snapshot carries `boot_id`.
2. `SeriesId`, `MetricId` and `ColumnId` numbers and strings are frozen.
3. `ProcessRecord` numeric fields are fixed-size; text lives in `ProcessMeta`.
4. The reducer exposes a hook used in P3 by the recorder and the rolling window:

```rust
pub trait RecordSink: Send + 'static {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]);
    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>);
    fn on_device_catalog(&mut self, catalog: &DeviceCatalog);
}
```

In P1 and P2 the reducer holds `Option<Box<dyn RecordSink>>` set to `None`.

**Format** (crate `tl-history`):
- Directory per boot: `history/<boot_id as 16 hex digits>/`.
- Segment files of 10 minutes: `seg-<start boottime seconds, 10 digits>.tlh`.
- Segment header (fixed, 64 bytes): magic `b"TLH\x01"`, `format_version: u16 = 1`, `boot_id: u64`, `realtime_offset_ns: i64` (CLOCK_REALTIME minus CLOCK_BOOTTIME at segment start), `start_t_ns: u64`, reserved zero bytes.
- Body: blocks of 60 s. Each block is `u32 LE compressed length | u32 LE crc32 of compressed bytes | zstd level 3 frame` containing a postcard `Block`:

```rust
pub struct Block {
    pub start_t_ns: u64,
    pub gap_before: bool,                        // R-169
    pub mem_total_kib: u64,
    pub catalog: Option<Vec<HistDevice>>,        // HistDevice { idx, kind, parent, name }, when the catalog changed
    pub new_meta: Vec<(MetaId, HistMeta)>,       // HistMeta { exe_path, user, uid, system } plus comm, display name, app id; no cmdline
    pub keyframe: Vec<HistProcess>,              // every live process at block start
    pub deltas: Vec<SweepDelta>,                 // one per process sweep in the block
    pub system: Vec<(SeriesId, Vec<(u32 /*ms offset*/, f32)>)>,  // 1 Hz system series
}
pub struct HistProcess { pub key: ProcessKey, pub meta: MetaId, pub ppid: u32,
                         pub cpu_permille: u16, pub mem_anon_kib: u32, pub io_r_kibps: u32, pub io_w_kibps: u32,
                         pub net_kibps: u32, pub gpu_pct: u8 }
pub struct SweepDelta { pub dt_ms: u32, pub changed: Vec<(u32 /*keyframe or new index*/, FieldPatch)>,
                        pub started: Vec<HistProcess>, pub exited: Vec<u32> }
```

- `mem_anon_kib` is RssAnon, the default Memory column (D-004).
- `FieldPatch` stores only fields that changed beyond a threshold (CPU by ≥ 1 permille, memory by ≥ 256 KiB, rates by ≥ 1 KiB/s or to/from zero).
- Footer written when a segment closes: block offset index, postcard with the magic `TLHF`. A segment without a footer (crash) is read by scanning blocks until the first bad CRC.
- Format details fixed before any file exists (R-169): header fields are little-endian at offsets 0, 4, 6, 14, 22; the boot directory is lowercase hex; `FieldPatch` is a struct of `Option`s written as a `u8` presence mask plus the present values (R-282); keyframe entries and `started` entries share one index space in that order; one `MetaId` space per segment; a gap is a NaN sample; a value whose validity is not `Valid` is stored as 0; `deleted-keys.bin` (History delete) has mode 0600.
- Command lines are never stored (ADR-015).

**Settings** (canonical table (a)): `history.enabled = false` (D-003); `history.retention_h = 6`, range 1..=12, menu 1, 2, 3, 4, 6, 8, 12 h; `history.max_disk_mb = 256` (not shown in menus); `history.persist = true`. When `persist` is false the recorder deletes its segments at shutdown and loads nothing at start; the on-disk format does not change.

**Retention and budget.**
- The recorder deletes segments older than retention and, when over the disk cap, the oldest segments first. The UI shows the real retained span.
- On start, directories of other boots older than retention are deleted.
- Loading the store at start has a 2 s budget; load errors show the toasts from r2 PROC-078 [now PROC-087].

**Rolling window while recording is off** (D-010).
- While `history.enabled` is false and the Processes tab is open (`Demand.process_table` is true), the engine keeps the last 10 minutes of process history in RAM. History mode shows this window.
- It is a second `RecordSink` in `tl-history` (`RollingWindow`). It uses the same `Block` encoding as segments: 60 s blocks, each zstd-compressed when it closes. It holds the open block plus the newest 10 closed blocks and drops the oldest block as a new one closes. Nothing is written to disk.
- It is fed only by the process sweeps that already run for the Processes tab (ADR-008) and system series that are already demanded. It adds no sweeps and no sources of its own.
- It is dropped as soon as `Demand.process_table` goes false: the user leaves the Processes tab or the main window closes. Hidden-window detection (ADR-008) stops the sweep but keeps `process_table`, so the window survives a minimize with a gap in its data [inferred].
- Turning recording on drops the rolling window and switches History mode to the store below; the store starts at that moment [inferred].
- RAM: worst case (2,000 processes changing every second) is about 18 MB of patches per 10 minutes before compression, about 6 MB compressed at 3:1; it shares the 16 MB history RAM budget below. The recorder and the rolling window never run at the same time.

**Budget math** (estimates, to be measured when the phase starts):
- Changed processes per 1 s sweep on a desktop: about 200. Patch size about 15 bytes. 200 × 15 B × 21,600 s = 64.8 MB raw for 6 h.
- Keyframes: 2,000 processes × 20 B × 360 blocks = 14.4 MB raw.
- System series: about 120 series × 4 B × 21,600 s = 10.4 MB raw (history keeps 1 Hz even when live CPU runs at 10 Hz; 10 Hz is reduced to per-second max).
- Total about 90 MB raw; zstd level 3 at an assumed 3:1 gives about 30 MB on disk for 6 h.
- Worst case, all 2,000 processes changing every second: 2,000 × 15 B × 21,600 = 648 MB raw, about 216 MB compressed, still under the 256 MB cap.
- RAM: the open block (at most 60 s of patches, about 0.2 MB typical, 2 MB worst case) plus segment indexes. Budget 16 MB for history in RAM.

**Reading.** The UI asks for a time range through a new `Command::HistoryQuery { id, range, what }` with `HistoryRange::{Live, At(TimeNs), Span { start_ns, end_ns }}` and `HistoryWhat::{Frame, TopAt { n: u8 }}`; the answer is `HistoryView { query_id, source, store_state, span, gaps, frame_t, range, process_count, rows, totals, series, stack, overview, top, realtime_offset_ns }` with `HistoryProcessRow`, `HistoryTop` and `HistoryLoadProblem` (R-156). The chart stacks the three largest per-process values per bucket plus "other" (D-020); a query without an answer is resent after 1,000 ms (R-169). Then the engine answers from the store when recording is on and from the rolling window when it is off, with a `HistoryView` component in the snapshot (max 1,024 points per series, decimated min/max as in ADR-009). Process history at a time T is rebuilt from the block's keyframe plus deltas up to T. Problems while recording (R-249): a store write error sets `Problem(OpenFailed)`, recording continues in RAM and the store is reopened 60 s later; a read error answers with what could be read and `Problem(EmptyOrTruncated)`; a full worker channel writes `SourceHealth.last_error = "History dropped N events"` at most once per minute with the state unchanged.

### Consequences

- P1 code needs no change when history arrives beyond setting the `RecordSink`.
- Recording while hidden costs the 1 Hz sources in ADR-008's "recording on" column.

### Rejected alternatives

- **SQLite.** Rejected: write amplification for append-only time series, and a C dependency for little gain.
- **Storing every process every second.** Rejected: 11 GB per 6 h (Astra).
- **Keeping all history in RAM.** Rejected: breaks the 150 MB target.

### Verification

- When the phase starts: write 6 h of synthetic data (2,000 processes, 200 changing per second) in an accelerated test; disk usage under 64 MB; random-time reconstruction equals the recorded state at that time; truncated last block is recovered up to the bad CRC.
- Rolling window: with recording off and the Processes tab open, 15 minutes of synthetic sweeps leave exactly the newest 10 minutes in RAM; leaving the tab frees the window; RAM stays under 16 MB with 2,000 processes changing every second.

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
| UI logic tests | one integration test binary per tl-ui crate, `crates/<crate>/tests/<short>/main.rs` (ADR-020), with `#[gpui_kit::test]`; tests that need the shell stay in `crates/tl-ui/tests/ui/` | `cargo test -p <crate>` | yes |
| Headless screenshot tests and FakeEngine UI tests | the `render_*` modules of those binaries (feature `render-tests`) and the tests that need `FakeEngine` from tl-ui-testkit (no feature flag) | `cargo test -p <crate> --features render-tests` | yes, with Mesa lavapipe (`VK_ICD_FILENAMES` set to lavapipe) |
| Real-system integration | `tests/real_*.rs` in each crate, `#[ignore]` | `cargo test --workspace -- --ignored` | no; owner machine |
| Performance gates | `cargo xtask bench` | see ADR-014 | no; owner machine |
| Static checks | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo xtask check-deps` | | yes |

The FakeEngine row holds from CS-B8 on; until then those tests need `--features test-hooks`. The per-crate binaries arrive as each crate is extracted (CS-F1 to CS-F9). Until a crate has its own binary, its tests stay modules of the one `crates/tl-ui/tests/ui/` binary (`cargo test -p tl-ui --test ui`).

`scripts/ci.sh` runs the CI rows in order. A GitHub Actions workflow on the private repo calls it on `ubuntu-latest` with the pinned toolchain.

**Parser fixtures.**
- `cargo xtask capture-fixture <name>` copies the files tasklens reads (`/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/proc/net/dev`, `/proc/<pid>/{stat,statm,status,io,comm,cmdline,cgroup}` for up to 50 pids, the relevant `/sys` files, the `run/udev/data/` block, net and PCI files; R-261) into `crates/tl-collect/tests/fixtures/<name>/` as a fake root.
- It scrubs: `cmdline` becomes `comm` plus `--arg`, user names become `user1..` (every occurrence, case-insensitive substring, longer names first; names shorter than 3 characters are not scrubbed; R-260), hostnames and serials become `REDACTED`, a udev file keeps only its `I:`, `V:`, `G:` and `Q:` lines and the `E:` keys of E04-030's `UDEV_KEYS` list (every `S:` line and every other `E:` key is dropped), with the `ID_SERIAL`, `ID_SERIAL_SHORT`, `ID_WWN` and `ID_FS_LABEL` values replaced by `SCRUBBED` (R-266), MAC addresses become `00:00:00:00:00:00` (R-140, R-162), and the boot ID and machine ID become the fixed fake values `00000000-0000-4000-8000-000000000001` and `00000000000040008000000000000001` (R-267). It never copies `environ`, `maps` or `fd` targets.
- Samplers take a root path (`SysRoot { proc: PathBuf, sys: PathBuf, run: PathBuf }`, `run` default `/run`), so tests point them at a fixture, which may carry a scrubbed `run/udev/data/` directory (R-140).
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
- Render counters (a test-only `Cell<u32>` in each view, behind the test-hook gate of ADR-020): a CPU-only snapshot change does not re-render `ProcessTable`; a series change re-renders only its `ChartView`.
- Animation driver: no frame request when no chart is moving; exactly one pending request per window.

**Headless screenshot tests.**
- Use `HeadlessAppContext::with_platform(Arc::new(CosmicTextSystem::new(..)), assets, || gpui_platform::current_headless_renderer())` and `capture_screenshot(window)`. This path exists in source but gpui-kit does not exercise it [inferred]; U-4 is verified on lavapipe, so there is no `painted_quads()` fallback. Captures are at device scale 2.0; the scales below are `ui_scale` (R-291).
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

- `scripts/ci.sh` green on every commit; each bead names the tests it adds. The script builds `tl-gpu-probe` with `--features fake` before the engine tests (R-166). The CI image uses the lavapipe ICD `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json` (R-158; `scripts/ci.sh` also accepts Arch's `lvp_icd.json`, R-266); the RTL golden is skipped with a printed reason when `Noto Sans Hebrew` is missing (R-215).

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
- `cargo xtask bench` builds with `--profile perf` (thin LTO, ADR-002) and runs `target/perf/tasklens`; `--release` uses the fat-LTO build and `target/release/tasklens`. Compare only summaries made with the same profile. It runs every scene 3 times and writes `target/bench/<date>-<git sha>.json` plus a table.

**Phase-0 fixture.** Window 1600 × 1000 logical px, on a 144 Hz monitor at 110%, focused, smooth graphs on.
- Scene `perf-cpu`: Performance tab, CPU page. Total CPU chart (fill + line + kernel line) and a 32-core grid (8 × 4) with kernel overlays, 60 s range, 10 Hz data. Device sidebar with 9 mini graphs (CPU, memory, GPU, 4 disks, 2 networks) at 1 Hz. A stats block with 12 values updated at 1 Hz and per-core percent labels updated at 10 Hz.
- Scene `table-2000`: Processes tab, 2,000 rows, 10 columns, every value changes at 1 Hz, sorted by CPU descending so order changes every second, grouped mode with 40 groups. Programmatic scroll at 1,200 logical px/s, bouncing between top and bottom, through `UniformListScrollHandle`.
- Scene `bridge`: `perf-cpu` where the total CPU chart is fed by the real `tl-engine` `Cpu` worker at 10 Hz through the real bridge.
- Scene `static`: `perf-cpu` with smooth graphs off.
- Scene `history-scrub` is added in P3 with the history store (ADR-012).
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

- `docs/spike/phase0-results.md` with the JSON outputs and the checklist, committed before any P1 UI bead starts.
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
- No custom log levels or categories: performance logs use `info` on target `tasklens::perf`. No repeat collapsing. `RUST_LOG` is not read.
- The low-memory watcher `tasklens-watch` (crate tl-watch, std plus libc only; `tasklens --watch-memory` execs it) writes `WARN tasklens-watch:` lines to stderr only, with no file appender and no `tracing` (R-152).

**What is never logged** (at any level): command lines, environment, working directories, open file paths, executable paths of other users' processes, window titles, user names other than the current user. Processes are logged as `pid` plus `comm` only, and `comm` only at `debug` and below. Action logs record kind, target PID and result.

**Errors.**
- Libraries use `thiserror` enums: `CollectError` (tl-collect), `OpsError` (tl-ops), `ProbeError` and `WorkerError` (tl-engine), `HistoryError` (tl-history). `anyhow` only in `tl-app::main` and `xtask`.
- `ActionError` (tl-model) has variants `PermissionDenied`, `NoSuchProcess`, `Partial { done: u16, total: u16 }`, `UnitJob { result: Arc<str>, error_name: Option<Arc<str>> }`, `Os(i32)`, `Internal(Arc<str>)`, `NotAuthorized` (pkexec 127, polkit `NotAuthorized`, systemd `AccessDenied`), `AuthCancelled` (pkexec 126, `NotAuthorizedDismissed`) and `Unsupported` (R-148, R-170). `tl-ui/src/action_tracker.rs::message(&ActionError) -> SharedString` turns each into plain, sentence-case text (R-196):

  | `ActionError` | `message()` |
  |---|---|
  | `PermissionDenied` | Permission denied. This process belongs to another user. |
  | `NoSuchProcess` | The process has already exited. |
  | `Partial { done, total }` | Done for {done} of {total} targets. |
  | `UnitJob { result, .. }` | The service job ended with result: {result}. |
  | `Os(code)` | The system call failed (error {code}). |
  | `Internal(text)` | Internal error: {text}. |
  | `NotAuthorized` | Administrator access was refused. |
  | `AuthCancelled` | Authentication was cancelled. |
  | `Unsupported` | Not supported on this system. |

  `short_reason(&err)` is the first sentence lower-cased without its full stop, for partial summaries. Reject toasts and deadlines are in R-196. The last three texts are R-240.
- Collection errors appear as `Validity::Failed` plus `SourceHealth.last_error`; a source error never becomes a toast unless the user started an action.

**Crash handling.**
- `panic = "unwind"` (ADR-002). A panic in a worker is handled by its supervisor; in the reducer by the session loop (ADR-004); in an action task, the action reports `Failed(Internal("internal error"))`.
- A panic hook (installed first in `main`) writes `$XDG_STATE_HOME/tasklens/crash/crash-<unix seconds>.txt` with: tasklens version and git sha, thread name, panic message, location, `std::backtrace::Backtrace::force_capture()`, and the last 200 log lines. No process metadata, no settings contents. Then it calls the default hook. Keep the newest 20 reports.
- A panic on the GPUI main thread ends the process and shows the desktop notification "tasklens stopped after an unrecoverable error." The next start opens a window titled "tasklens Crash Report" with the text "tasklens closed unexpectedly last time." and the buttons "Open log folder", "Copy report" and "Close".
- Native crashes are left to `systemd-coredump`; the crash report window mentions `coredumpctl info tasklens`. NVML native crashes only affect `tl-gpu-probe` (ADR-007).
- GPU device loss: whoever detects it calls `tl_ui::request_exit(cx, UiExit::DeviceLost)`; tl-app shows the desktop notification "tasklens lost the graphics device and must restart", then relaunches once with `TASKLENS_RELAUNCHED=1` set (R-153).

**Untrusted process text** (`tl-collect/src/sanitize.rs`, applied before any string enters `ProcessMeta`):
- Read limits: `comm` 64 bytes, `cmdline` 32 KiB (truncate and append `…`), `exe` link 4 KiB, `status` 8 KiB.
- Bytes are decoded with `String::from_utf8_lossy`.
- C0 and C1 control characters become U+FFFD. Bidirectional controls U+202A to U+202E and U+2066 to U+2069, and U+200B to U+200F, are removed.
- Lines are joined with spaces; NUL separators in `cmdline` become spaces.

**Exports and copy.** In every table:
- Ctrl+C runs "Copy row" (process menu label "Copy details"; services, startup and connections: "Copy row"). It copies a fixed text block per selected row (add PROC-098 [now PROC-070], SYS-009, SYS-026 [now SYS-025], SYS-034 [now SYS-032]).
- Ctrl+Shift+C runs "Copy rows as table": the visible columns of the selected rows as tab-separated text with a header line.
- "Copy" copies the clicked cell.
- "Copy all metrics" copies every column with data as `Label: value` lines, ending with `Command line: …`.

On the Performance tab, Ctrl+C copies the device info. Command lines appear only in "Copy all metrics" and only for rows the user selected; this is the one deliberate export of command lines. There is no automatic export. Any later export feature asks before including command lines.

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

**Module layout of `tl-collect`** (R-135): `sysroot.rs` (`SysRoot`, kept-open files, `pread`), `fdcache.rs`, `counters.rs` (rates, resets, suspend gap), `sanitize.rs`, `hash.rs`; `parse/` with one file per system file (`stat`, `meminfo`, `vmstat` (`pswpin` only, ADR-017), `psi`, `loadavg`, `diskstats`, `netdev`, `cpufreq`, `hwmon`, `zram`, `block`, `netif`, `topology`, `rapl`, `udev` (R-140), `partitions` (R-140)); `sample/{cpu,memory,disk,net,sensors}.rs`; `proc/` (`pid_stat`, `status`, `statm`, `io`, `cgroup`, `meta`, `fdcount`, `sweep`, `smaps`, `detail`, `strings`, `handles`, `extras_p2` (R-163)); `pkg.rs` (package owner index, R-163); `apps/` (`classify`, `cgroup_app`, `desktop`, `icons`, `naming`, `launchers`); `net/` (`sock_diag`, `socket_map`, `attribution`, `connections`, `netns`, `rdns`); `startup/` (`autostart`, `user_units`, `plasma_scripts`, `watch`, `impact`); `inventory/` (`cpuinfo`, `dmi`, `os_release`, `displays`, `modules`); `gpu_drm/` (`amdgpu`, `intel`, `fdinfo`).

**File descriptor policy.**
- Kept open and re-read with `pread` at offset 0: `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/proc/net/dev`, each used `cpufreq` and `hwmon` file, zram `mm_stat`, and from P2 `/proc/pressure/memory`, `/proc/pressure/cpu`, `/proc/loadavg` and `/proc/vmstat`. Reopened after any read error. `/proc/vmstat` is read for `pswpin` only (ADR-017); R-162 still holds for its other fields.
- Per-process files are opened with `openat` relative to an `O_DIRECTORY` fd of `/proc/<pid>` during the sweep, read, and closed in the same sweep.
- An FD cache of at most 128 entries holds `smaps_rollup` fds for `detail_keys` only; entries are dropped when the key leaves demand or the identity check fails.

**Sources** (phase in brackets, R-072 numbering):

| Data | Source | Notes |
|---|---|---|
| CPU [P1] | `/proc/stat` | math in ADR-008 |
| CPU frequency [P1] | `/sys/devices/system/cpu/cpufreq/policyN/cpuinfo_avg_freq`, else `scaling_cur_freq` | amd-pstate-epp here |
| CPU temperature [P1] | hwmon `k10temp` `temp1_input` (`Tctl`), else `coretemp`, `zenpower` | board chips such as `nct6797` at 1 Hz only; `hot_c` as in the Sensors row (`k10temp` on this machine has no `_crit` or `_max` file, verified 2026-10-23, so its `hot_c` is `None`) |
| CPU throttling [P2] | `/sys/devices/system/cpu/cpu<n>/thermal_throttle/{core,package}_throttle_count` (Intel) at 1 Hz | `CpuTotals.throttling` is true when any count increased since the last read; `Unsupported` when the directory is missing (AMD) |
| Memory [P1] | `/proc/meminfo`, `/sys/block/zram*/mm_stat` | `/proc/vmstat` only for `pswpin` (System pressure row); R-162 holds for its other fields |
| System pressure [P2] | `/proc/pressure/memory` (`some` and `full` avg10), `/proc/pressure/cpu` (`some` avg10), `/proc/loadavg` (first field, load1), `/proc/vmstat` `pswpin` (delta per second); uptime is CLOCK_BOOTTIME now, no file | read at 1 Hz: the memory file and `pswpin` by the `Memory` worker while `Demand.memory` or `Demand.device_history` (`PSI_MEM_SOME`, `PSI_MEM_FULL`, `SWAP_IN`), the CPU file and `loadavg` by the `Cpu` worker while `Demand.cpu` is not `Off` or `Demand.device_history` (`PSI_CPU_SOME`, `CPU_LOAD1`), also when the CPU runs at 10 Hz; a kernel without PSI (`ENOENT`) gives `Unsupported` for the PSI series; a `pswpin` decrease is a counter reset (ADR-008) |
| Disks [P1] | `/proc/diskstats`, `/sys/block/*` | active % from `io_ticks` delta; latency = Δ(read_ticks+write_ticks) / Δ(ios); exclude `loop`, `ram`, `zram`, `fd` from the sidebar; partitions and `dm-*` are children |
| udev properties [P1] | `/run/udev/data/b<major>:<minor>` (block), `n<ifindex>` (net), `+pci:<slot>` (PCI) read by `parse/udev.rs`, 64 KiB cap, `E:KEY=value` lines only for the asked keys (`ID_MODEL`, `ID_MODEL_FROM_DATABASE`, `ID_VENDOR_FROM_DATABASE`, `ID_BUS`, `ID_WWN`, `ID_SERIAL`, `ID_SERIAL_SHORT`, `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`, `ID_PART_ENTRY_OFFSET`, `ID_PART_ENTRY_SIZE`, `ID_PATH`; the same 15 keys are the fixture capture allowlist, R-266) | no libudev; values are untrusted text and go through `sanitize`; `ID_BUS=usb` gives the USB kind and `ID_MODEL` the model, else the sysfs rules (R-140) |
| Partitions [P2] | sysfs `/sys/block/<disk>/<part>/{start,size,ro}` plus the udev `ID_PART_*` and `ID_FS_*` keys, `/proc/self/mountinfo` for mounts | `parse/partitions.rs`: rows ordered by `start_bytes`, gaps over 1 MiB as `unallocated`, GPT GUID names and attribute flags per PERF-050, MBR types as `MBR 0x%02X` (R-140) |
| Network [P1] | `/proc/net/dev`; `ifindex` from `/sys/class/net/<if>/ifindex`; netns inode from `/proc/self/ns/net`; `operstate` for `LinkState` | classified Wired, Wireless, VPN and tunnel, Virtual, Other (PERF-049 [now PERF-054]); the virtual name patterns are checked before `bridge/` (R-162); `lo` never shows; `tailscale0` is a tunnel and is shown; `docker0`, `veth*`, `br-*`, `virbr*`, `vnet*` are Virtual and hidden by default (`mpqemubr` joins once seen, U-52); settings `performance.show_network_{wired,wireless,vpn,virtual,other}` (only `virtual` defaults to false); visibility groups: Ethernet and USB to Wired, Wi-Fi and Cellular to Wireless, Tunnel and VPN to VpnTunnel, Virtual and Bridge to Virtual (R-162) |
| NVIDIA GPU [P1] | `tl-gpu-probe` (ADR-007) | P2: `nvmlDeviceGetTemperatureThreshold(NVML_TEMPERATURE_THRESHOLD_SLOWDOWN)` once per device into `hot_c`; `nvmlDeviceGetCurrentClocksEventReasons` at 1 Hz, `GpuTotals.throttling` true when `HW_SLOWDOWN` or `SW_THERMAL` is set |
| AMD/Intel GPU [P2 AMD, P3 Intel] | sysfs `gpu_busy_percent`, `mem_info_vram_*`, hwmon, DRM fdinfo | untested here |
| Sensors and fans [P1] | `/sys/class/hwmon/hwmon*/{temp,fan,pwm}*`, `pwm<n>_enable` for `FanControlMode` | `fan_connected` is kept across samples (R-170); P2: `temp<n>_crit`, else `temp<n>_max`, read at enumeration into the device's `hot_c` (millidegrees to °C), `None` when neither exists |
| Processes [P1] | `/proc/<pid>/stat`, `statm`, `status` (uid, threads, RssAnon), `io` (own processes), `cgroup`; fd count from `fstatat` `st_size` of `/proc/<pid>/fd`, readdir fallback | `io` gives `Restricted` for other users; all timestamps CLOCK_BOOTTIME (ADR-003) |
| Process metadata [P1] | `comm`, `cmdline`, `exe` | refreshed every 10 sweeps per process and immediately when `comm` changes (exec) |
| PSS/USS [P1] | `/proc/<pid>/smaps_rollup` | `detail_keys` only |
| Strings scan [P2] | `rustix::fs::pread` on `/proc/<pid>/mem` over the readable mappings | same ptrace access rule as `process_vm_readv`; scanner in `proc/strings.rs`, shared with tl-helper (R-144) |
| Package owner [P2] | batched `pacman -Qoq <paths…>` for unseen exe paths (debounced 2 s), `/var/lib/pacman/local/<pkg>-<ver>/desc` (`%VERSION%`, `%DESC%`, `%PACKAGER%`, `%BUILDDATE%`, `%URL%`); integrity check `pacman -Qkk <pkg>` | cache keyed by exe path and mtime, invalidated on the `local` directory mtime; `dpkg -S` and `rpm -qf` behind the same trait at P4 (R-163) |
| I/O priority [P2] | `ioprio_get` through `tl_ops::sys::ioprio_get_tid`, re-exported as `tl_ops::process::io_priority_of(key, boot)` | read by the detail worker, never by tl-collect (R-144) |
| App and unit cgroup stats [P1 apps, P2 units] | cgroup v2 `memory.stat` (`anon`), `memory.swap.current`, `cpu.stat` (`usage_usec`), `io.stat` (`rbytes`, `wbytes`, `rios`, `wios`), `pids.current`, `cgroup.procs`, `cgroup.events` (`frozen`); `memory.current` only in `EngineMode::LowMemory` | app scopes every sweep; demanded unit cgroups (`service_cgroup_stats`, `unit_details`) at 1 Hz; every sweep also reads `cgroup.events` alone for each cgroup of a unit row (a unit, by manager and name, with two or more processes outside app units; one row can span several users' cgroups) and marks that cgroup's processes frozen, and a unit row is frozen when all its members are (tasklens-8q0.42); opened with `openat` from a dirfd on `/sys/fs/cgroup` and closed in the same sweep (R-137) |
| App grouping [P1] | cgroup v2 path from `/proc/<pid>/cgroup`; current uid from `getuid()` | app id from scope name `app-<launcher>-<app id>-<n>.scope` (KDE: `app-<desktop file id>@<id>.service` or `.scope`); desktop file lookup through `XDG_DATA_DIRS` |
| Per-process network [P2] | `sock_diag` `inet_diag` dump for TCP v4 and v6 with `INET_DIAG_INFO` (`tcpi_bytes_acked`, `tcpi_bytes_received`) | labelled "Network (TCP, attributed)"; sockets shared by several processes count for each and the cell gets a tooltip; UDP not attributed |
| Socket to pid map [P2] | `/proc/<pid>/fd` symlinks `socket:[inode]` | rebuilt every 2 s while demanded; own processes only without the helper |
| Connections [P2] | `sock_diag` dump TCP, UDP, v4, v6 | |
| Services [P2] | systemd Manager over D-Bus: `ListUnits`, `ListUnitFiles`, `Subscribe`, `UnitNew`, `UnitRemoved`, `JobRemoved`, `PropertiesChanged`; unit `memory.stat` and `cpu.stat` with the E05 cgroup readers | system and user managers are separate identities `UnitRef { manager: System | User, name }`; a missing manager is retried on `RefreshNow` or every 60 s (R-167) |
| Service logs [P2] | `journalctl -u <id> -b -n 200 -o json --no-pager` (`--user-unit <id>` for user units) run with `std::process::Command` inside `spawn_blocking`; 5 s timeout on this fetch, then kill and wait. Older pages use `--reverse --cursor=<oldest> -n 201`. With Follow on, `-f` is added and the child runs until Follow is turned off or the tab closes, then it is killed | missing `PRIORITY` is 6; `Access(OwnOnly)` for system units only (R-167) |
| Startup apps [P2] | XDG autostart: `$XDG_CONFIG_HOME/autostart` overrides `$XDG_CONFIG_DIRS/autostart` (default `/etc/xdg/autostart`) by file name; `Hidden`, `OnlyShowIn`, `NotShowIn` against `XDG_CURRENT_DESKTOP`; systemd user units with `WantedBy=` | disable a system entry by writing a user override with `Hidden=true` and `X-Tasklens-Override=true` (atomic write, mode 0600 for new files, existing mode kept, files over 1 MiB refused); enable by deleting the user file when it carries `X-Tasklens-Override=true`, else set `Hidden=false` and remove `X-GNOME-Autostart-enabled=false` (SYS-022, R-165); units use Enable/DisableUnitFiles; status precedence NotInThisDesktop, then MissingProgram, then Disabled (R-164) |
| Information [P2] | `/proc/cpuinfo`, `/sys/devices/system/cpu`, `/sys/class/dmi/id` (readable fields), `/etc/os-release`, NVML device info | root-only DMI fields shown as `Restricted`; memory module serial numbers are not read or shown anywhere, including copy text (D-007) |
| Disk details [P2] | udisks2 over D-Bus | SMART, partitions, eject |
| Batteries [P2] | UPower over D-Bus | |
| Network details [P2] | NetworkManager over D-Bus | |
| Icons [P1] | desktop entry `Icon=`; theme from `~/.config/kdeglobals` `[Icons] Theme=`, then `breeze`, then `hicolor`; sizes 16, 22, 24, 32, then `scalable` | resolved paths cached in `$XDG_CACHE_HOME/tasklens/icons.toml` |

**Restricted, not zero.** Any `EACCES` or `EPERM` on a field sets that field `Restricted`. Processes owned by other users and capability-raised processes such as `kwin_wayland` show the lock marker for unreadable fields.

**Process power column.** Columns `power` ("Power usage") and `power_trend` exist and are off by default on every machine, because there is no per-process power source; the value is an estimate.

### Consequences

- Parser speed and allocation are under tasklens's control and covered by fixtures.

### Rejected alternatives

- **`procfs` crate.** Rejected: allocates per call; the hot path needs reused buffers.
- **nethogs screen scraping (Mission Center).** Rejected: fragile, needs capabilities on a third-party binary.
- **libsystemd for journal reading.** Rejected for v1: `journalctl` is enough for 200 lines and follow mode, and avoids a C binding.

### Verification

- Fixture tests per source (ADR-013).
- Criterion bench `tl-collect/benches/sweep.rs`: a sweep of the `owner-desktop` fixture's processes scaled to 2,000 entries runs in ≤ 20 ms on this machine with the default columns (R-286).
- Real-system test: a process of another user shows `Restricted` for I/O, not zero.

---

## Open questions and recommended defaults

Settled since these records were first written:
- Application id and D-Bus name: `io.github.oysteinkrog.Tasklens` (D-005, R-001).
- History recording default: off, 6 h retention when turned on (D-003); 10-minute rolling window while off (D-010).
- Decorations default: client-side (D-006), with the fallback rule in ADR-010.
- Users and Details tabs and Mission Center's Apps page: not required. tasklens has six tabs (R-023); grouped mode covers the Apps page.
- Surviving the last window close: gpui-pre 0.3.7 has `QuitMode::Explicit` (R-178). Whether `Application::run` returns on Linux (U-2) stays open until E33-011.
- The open items of the bead round are R-139 to R-235 in `docs/RECONCILIATION.md` and D-012 to D-020.

Still open. Each has a recommended default that beads use unless Oystein decides otherwise. The unverified list in `docs/RECONCILIATION.md` (U-1 to U-38) tracks every assumption that phase 0 or the first bead touching it must confirm; the U-numbers below point there.

1. **Collection-cost gate with the Processes tab visible** (U-10). PLAN.md said 2%; Astra's arithmetic says about 3% for 2,000 processes. Default: gate at 3.5% (ADR-014) and keep the 2% gate for the Performance tab.
2. **Font family** (U-7). Default: Noto Sans static weights. Inter is the alternative if Noto Sans lacks `tnum` in the bundled files (the font bead checks this first).
3. **Icons.** TaskSlinger uses Material Symbols. Default: Lucide through gpui-kit, because the Material Symbols variable font may hit zed#64783 and zed#61660.
4. **Whether `Application::run` returns on Linux** (U-2). Default: shut the engine down after `run` returns, and also from `on_app_quit` if it does not.
5. **Headless wgpu screenshots on Linux** (U-4). Default: try it in the first render-test bead; fall back to `painted_quads()` assertions.
6. **Minimize and restore on KWin** (U-3). Default: the hidden-state timer in ADR-010, plus `window.refresh()` every 1 s while hidden if phase 0 shows it is needed.
7. **GPU power gate.** No baseline exists yet. Default: report only in phase 0; set a gate after the first measurements.
8. **Exact gpui-kit `ThemeColor` token names** (U-8). Default: the mapping in ADR-009, checked against source by the theme bead.
9. **zed#64499 fix details** (U-9). Default: apply the ADR-010 patch only if the soak test fails, and re-run the soak.
10. **CSD checklist on KWin** (U-36). Default: client-side; switch the default to server-side if any check fails (ADR-010).

## ADR-017: Device metric history

Status: accepted (D-030, 2026-10-23). Phase P2. Separate from the process history of ADR-012, which stays off by default.

### Context

The Overview's 1 h, 24 h and 7 day ranges (UI-PRF-055) and the 24-hour timeline (UI-PRF-057) need device metrics for up to 7 days: CPU total and per core, memory rows, GPU engines and VRAM, disk read, write and active time, network down and up, temperatures, power, PSI, swap-in and load. The live series hold at most 10 minutes (ADR-003). `DeviceIdx` is a per-session index and `t_ns` is CLOCK_BOOTTIME, which restarts at every boot, so neither can key data that must be read back after a restart, a reboot or a hotplug. ADR-012 solved the same problem for process history with a device catalog per block and a realtime offset per segment; this ADR uses the same approach.

### Decision

**Worker.** A new `MetricHistory` worker in `tl-engine` (`workers/metric_history.rs`, ADR-004) keeps three tiers per series, with the on-disk format in `tl-history` (`metrics.rs`), so tl-engine's dependency on tl-history starts in P2. It runs while `Demand.device_history` is true, which follows `history.device_metrics` (default `true`) and not visibility; ADR-008's "device history on" column says which sources then keep running while no surface is visible. In P2, closing the main window quits the app, so until the tray ships (P3, `tray.close_to_tray`) the history covers only the time the process runs; a closed app is a gap. Turning `history.device_metrics` off stops the worker and deletes the `metrics/` directory.

**Series identity.** Stored series are keyed by `(device key, MetricId)`, never by `SeriesId`. The device key is the device's `persistent_key` (PERF-005, R-155), which this ADR also defines for the kinds that had none: `cpu` (`CpuPackage`), `cpu:<id>` (`CpuLogical`), `memory`, `sensor:<hwmon name>:<label>`. Each segment's first block, and every block after a catalog change, carries a catalog `Vec<HistSeries { slot: u16, device_key: Arc<str>, device_name: Arc<str>, metric: MetricId }>`; the other blocks refer to series by `slot`. On read, the worker maps each device key to the current catalog's `DeviceIdx`, so a disk unplugged and plugged back, or a reboot that renumbers devices, continues the same series. Series of devices not in the current catalog stay on disk and age out with retention.

**Time base.** Each segment header carries the `boot_id` and `realtime_offset_ns` (CLOCK_REALTIME minus CLOCK_BOOTTIME at segment start), as ADR-012. Points inside a segment use `t_ns`; the reader converts to CLOCK_REALTIME with the segment's offset. Ranges of 1 h and longer and the timeline are drawn on CLOCK_REALTIME in local time. A boot shows as a gap plus a boot event; a suspend shows as a gap plus a suspend event. A new segment starts at every engine start, so one segment never spans two boots.

**Tiers and aggregation.**

| Tier | Point | Kept for | Values per point | Fed from |
|---|---|---|---|---|
| 1 s | 1 s | 1 h | value (f32) | the per-second maximum of a 10 Hz series (the ADR-003 1 Hz tier rule, as ADR-012), the sample of a 1 Hz series; for a core clock the per-second mean, with the per-second maximum as a second value |
| 10 s | 10 s | 24 h | mean, min, max (f32) | the ten 1 s points: mean of the valid values, their min and max |
| 1 min | 60 s | 7 d | mean, min, max (f32) | the six 10 s points: mean of the means weighted by valid counts, min of the mins, max of the maxes |

A point with no valid input is invalid (a gap); validity is one bit per point. A sample whose validity is not `Valid` is not an input. `GAP_BEFORE` on a live sample ends the current point.

**Size** (at about 150 series; raw bytes, in RAM and before compression on disk):
- 1 s tier: 150 × 3,600 × 4 B = 2.2 MB.
- 10 s tier: 150 × 8,640 × 12 B = 15.6 MB.
- 1 min tier: 150 × 10,080 × 12 B = 18.1 MB.
- Validity bitmaps: 150 × 22,320 bits = 0.4 MB.
- Total about 36 MB if every tier were held in RAM; that is too much next to the 150 MB RSS gate of ADR-014 (1.0.3 already runs at about 155 MB RSS, 77 MB anonymous, on the owner machine). So RAM holds only the 1 s tier (the last hour, about 2.2 MB) and the open block of each other tier (D-030 leader note, 2026-10-23). The 10 s and 1 min tiers are read from their segment files when a range or the timeline needs them, into a cache that holds only the requested span (at most 1,024 decimated points per series, under 2 MB) and is dropped 60 s after nothing requests it. Idle history RAM stays under 6 MB. On disk the same 36 MB raw plus up to one extra segment per tier before it is deleted (about 3.6 MB), before zstd; the compressed size is measured when the phase starts. Cap: `history.device_metrics_max_mb = 64` (not in menus); over the cap the oldest segments by end time are deleted first.

**Format** (crate `tl-history`, `metrics.rs`):
- One directory per tier under `$XDG_STATE_HOME/tasklens/metrics/`: `1s/`, `10s/`, `1m/`. Segment files `seg-<CLOCK_REALTIME start seconds, 10 digits>.tlm`. A segment covers 10 min (1 s tier), 1 h (10 s tier) or 24 h (1 min tier), and is cut early at engine shutdown or start.
- Retention per tier: 1 h, 24 h and 7 d. Whole segments are deleted once their end is older than the tier's retention; nothing inside a segment is rewritten.
- Segment header (fixed, 64 bytes, little-endian): magic `b"TLM\x01"` at 0, `format_version: u16 = 1` at 4, `tier: u8` (0 = 1 s, 1 = 10 s, 2 = 1 min) at 6, `boot_id: u64` at 8, `realtime_offset_ns: i64` at 16, `start_t_ns: u64` at 24, reserved zero bytes.
- Body: one block per 60 s, appended once a minute (a crash loses at most one minute). Each block is `u32 LE compressed length | u32 LE crc32 of compressed bytes | zstd level 3 frame` of a postcard `MetricBlock { start_t_ns, gap_before: bool, catalog: Option<Vec<HistSeries>>, columns: Vec<SeriesColumn { slot, valid: Vec<u8> /* bitmap */, mean: Vec<f32>, min: Option<Vec<f32>>, max: Option<Vec<f32>> }>, events: Vec<HistEvent> }`, with 60, 6 or 1 points per column by tier; `min` and `max` are present in the 10 s and 1 min tiers only. An invalid point stores 0.
- Events are written in the 1 min tier's blocks only, so they are kept 7 days: `HistEvent { kind: EventKind { MemoryPressure, SwapStorm, Heat, FullLoad, Suspend, Boot, OomKill, Crash }, start_t_ns: u64, duration_ms: u32, peak: f32, device_key: Option<Arc<str>>, process: Option<Arc<str>> }`. Process names are `comm` only (ADR-015: no command lines).
- A segment without a complete last block (crash) is read up to the first bad CRC, as ADR-012.

**Start.** The worker loads the last hour of the 1 s tier into its ring within the 2 s start budget of ADR-012 (the 10 s and 1 min tiers are not loaded; they are read on request), and appends a `Boot` event when the newest segment's `boot_id` differs from the current one.

**Event detector.** Part of the `MetricHistory` worker. It evaluates the UI-PRF-057 rules once per second on the live series, and fetches the journal event kinds there named (at start, then every 60 s while `Demand.device_timeline` is true). It writes each event into the store when it ends (or at shutdown, with the duration so far).

**Memory group series** (D-036, UI-PRF-060). The reducer publishes the top 20 memory groups, "Other apps" and "Shared and kernel" as `MetricId::MemGroup` series on device keys `memgroup:<group key>` (`memgroup:other`, `memgroup:shared` for the two fixed bands), with the group's display name as `device_name` in the catalog. The worker stores them like device series, so the format does not change. A group enters the block catalog when it first reaches the top 20 and stays there until its segment ends; outside the top 20 it is a gap, and its memory counts in "Other apps". The six parts of "Shared and kernel" (UI-PRF-060 Shared and kernel split) are published as `memgroup:k:shmem`, `memgroup:k:zswap`, `memgroup:k:slab`, `memgroup:k:kernel`, `memgroup:k:driver` and `memgroup:k:filecache`; `memgroup:shared` stays as their sum, so history written by 1.0.8 still reads. 28 series add about 0.4 MB to the 1 s tier in RAM. While `history.device_metrics` is on, the engine runs its own group sweep every 10 s (RssAnon only) whatever the UI demands, so the 10 s and 1 min tiers have no holes; the 1 s tier fills only while a By app view or the Processes tab drives the 1 s sweep (UI-PRF-060 Data binding).

**Per-core clock series** (tasklens-o55.56). The reducer publishes `MetricId::CPU_CORE_CLOCK` once per physical core, on the `cpu:<id>` device of the core's lowest online CPU. The value is the highest clock in MHz among the core's SMT siblings, from the per-CPU clocks the CPU worker already reads, so there are no new file reads. A core whose CPUs have no reading is a gap. While the clock comes from BogoMIPS (`freq_is_bogomips`), no series starts and a running one gets a gap. The 1 s tier keeps two values for these series: each second's mean, and each second's maximum in the block's `max` column, which the other series leave empty in the 1 s tier, so the format does not change. The 10 s and 1 min tiers keep the mean of the per-second means, the lowest per-second mean and the highest per-second maximum. So every range reads the true mean of its samples, and a 100 ms boost shows as the bucket maximum. Each series costs about 256 KB raw over the three tiers, 29 KB of it in the two 1 s rings in RAM. The owner machine has 16 cores: 4.1 MB raw and 0.46 MB in RAM, so the total above grows to about 40 MB raw, under the 64 MB cap. A 64-core machine adds 16.4 MB raw (1.8 MB in RAM), about 52 MB in total, still under the cap. The CPU inventory carries each core's `cpuinfo_max_freq` once (`ProcessorInfo.core_max_mhz`, by `CoreInfo.id`), beside `base_mhz`.

**Per-kind network totals** (tasklens-q4x76.22, leader decision on q4x76.9). The engine counts every network adapter kind, whatever a window shows. `aggregate:net` (`MetricId::NET_RX`, `NET_TX`) is the total of all kinds. Each kind also gets its own total, `AggregateKind::NetsOf(kind)` with the device key `aggregate:net:<kind>`, where `<kind>` is `wired`, `wireless`, `vpn`, `virtual` or `other` (an interface with no kind counts as `other`). The catalog creates all five at session start, and every network sample writes all five beside `aggregate:net`: 0 for a kind with no interface, and valid exactly when `aggregate:net` is valid. So each per-kind series has the same samples as the total, and a bucket's total mean minus a kind's mean is the mean of the other kinds, also when an adapter comes or goes inside the bucket. The worker stores them like any other device series, so the format does not change: 10 more series, about 0.15 MB in the 1 s tier in RAM and 2.4 MB raw over the three tiers. `DeviceTimeline.net_kinds` carries download plus upload of each stored per-kind total on the Network band's times. A window shows `aggregate:net` minus the per-kind totals of the kinds its `performance.show_network_*` settings hide, in the live charts, the range figures, the timeline band, the tray and the minigraphs, so two windows on one engine each see their own total, and history written with no window open has every kind. History written before this change has only `aggregate:net`, which then counted the kinds shown at the time; with no per-kind points there, nothing is taken away and it reads as stored.

**Read path.** The UI does not use `HistoryQuery`. A snapshot component carries the data:

```rust
pub struct DeviceHistory {
    pub header: ComponentHeader,
    pub request: Option<DeviceHistoryRequest>,   // the request this answers (ADR-008 Demand)
    pub realtime_offset_ns: i64,                  // current boot, to place live samples on the same axis
    pub since_realtime_ns: Option<i64>,           // oldest stored point of the served tier ("since 09:14")
    pub series: Arc<[(SeriesId, Arc<[HistPoint]>)]>, // sorted by SeriesId; current catalog only
    pub timeline: Option<Arc<DeviceTimeline>>,    // while Demand.device_timeline
}
pub struct HistPoint { pub t_realtime_ns: i64, pub mean: f32, pub min: f32, pub max: f32 } // mean NaN = gap
pub struct DeviceTimeline { pub bands: [Arc<[HistPoint]>; 4], pub events: Arc<[HistEvent]>, pub boots: Arc<[i64]>, pub journal: JournalAccess }
```

`Snapshot.device_history: Arc<DeviceHistory>` has the component bit `DEVICE_HISTORY = 1 << 15`, so `ComponentMask::ALL` becomes `0xFFFF` (sixteen bits). The worker publishes it once per tier period (1 s, 10 s or 60 s) while `Demand.device_history_range` is `Some`, with the tier fixed by the range (1 s for 1 h, 10 s for 24 h, 1 min for 7 d; while paused, the finest tier that covers the requested end, UI-PRF-057) and at most 1,024 points per series, min/max-decimated as `HistoryView` is (ADR-009, ADR-012). Unchanged series reuse their `Arc`. `DeviceTimeline` carries the 24-hour bands (CPU total, memory in use, busiest disk active time, network total, 1 min means) and the events, republished every 60 s and when an event ends.

**Settings** (canonical table (a)): `history.device_metrics = true`, `history.device_metrics_max_mb = 64`.

### Consequences

- With the default settings the engine keeps reading the 1 Hz device sources while the window is hidden (ADR-008), and holds under 6 MB of history in RAM while no long range is shown.
- Device series survive restarts, reboots and hotplug; process history (ADR-012) is unchanged.

### Rejected alternatives

- **Keying by `SeriesId` and `t_ns`.** Rejected: both are only valid within one engine session and one boot.
- **Means only in the 1 s tier.** Rejected: hides CPU spikes that the live chart and ADR-012 show as per-second maxima.
- **One file for all tiers.** Rejected: retention would need rewrites; per-tier segments are deleted whole.

### Verification

- Accelerated test: 7 days of synthetic data at 150 series stays under the 64 MB cap, keeps exactly 1 h, 24 h and 7 d per tier, reloads the 1 s ring within 2 s, and serves a 7 d request from disk within 300 ms on the owner machine.
- A series written under one `DeviceIdx` and read back after a catalog renumbering (and after a fake `boot_id` change) continues under its device key, with a gap and a `Boot` event between.
- Aggregation: a single 100% 10 Hz sample in a second gives 100 in the 1 s tier and as the max of the 10 s and 1 min points.
- Truncated last block is read up to the bad CRC.

## ADR-018: Engine service, clients and multiple hosts

### Context

The owner works from the office desktop and the home desktop and wants one tasklens window that shows either PC, picked from tabs in the window (owner request, 2026-10-29, D-038). Owner decisions: remote PCs are reached over SSH; the engine becomes an always-on per-user service and every window is a client of it; remote control covers everything the local window can do, root actions included.

ADR-005 already puts the engine behind `EngineClient`, and `tl-ui` uses nothing else. A client that speaks a wire protocol replaces the in-process `EngineHandle`; the pages do not change shape. Astra reviewed the first draft (2026-10-29); this version answers its twelve findings.

### Decision

**Crates and binaries** (amends ADR-001).

| Piece | Kind | Allowed workspace deps | New external deps |
|---|---|---|---|
| `tl-model/src/remote_proto.rs` | protocol types and the envelope codec, pure | (tl-model's own) | serde `rc` feature on (amends ADR-002): message types derive `Serialize`/`Deserialize` with `Arc<str>` and `Arc<[T]>` sent by value |
| `tl-remote` | lib: transports, envelope compression, `RemoteClient`, the service's endpoint checks, server sessions' wire side | tl-model | `zstd`, `arc-swap`, `async-channel`, `rustix` (already allowed in tl-model/tl-ops rows) |
| `tl-server` | bin `tasklens-engine`: the engine service | tl-model, tl-engine, tl-history, tl-remote, tl-ops (`write_atomic` for `engine.toml`, q4x76.9) | `sd-notify`; `tracing` and `tracing-subscriber` for its log, `libc` for the SIGTERM wait (q4x76.5); `toml` for `engine.toml` and the import from `settings.toml` (q4x76.9) |
| `tl-app` | bin `tasklens`: the window, `tasklens bridge` | adds tl-remote | none |

asupersync stays inside `tl-engine` (D-001); its normal dependency closure may now reach `tl-server` as it does `tl-app`. `xtask check-deps` gains rows for `tl-remote` and `tl-server` and checks that `tl-remote` has no asupersync, Tokio or GPUI in its closure. `tasklens-engine` joins the shipped-binary checks (ADR-002).

**The engine service.** `tasklens-engine.service`, a systemd user unit: `Type=notify`, `Restart=on-failure`, `RestartPreventExitStatus=0`, `RestartSec=2`, `TimeoutStopSec=10`, enabled by the package through a user preset. It sends `READY=1` after the socket is bound and history is open (when another engine holds the lock it sends `READY=1` and `STOPPING=1`, then exits 0, so the unit ends inactive and is not restarted; `RestartPreventExitStatus=0` guards any other clean exit); on SIGTERM it stops sampling, flushes history (ADR-017 shutdown) and exits within 5 s. It runs while the user manager runs, so from login to logout; `tasklens-setup linger on` keeps it running with nobody logged in (off by default).

**One engine per user per machine.** Any process that runs an engine first takes `flock(LOCK_EX | LOCK_NB)` on `$XDG_STATE_HOME/tasklens/engine.lock` and holds it for its lifetime; only the holder opens history for writing. The service exits 0 with the log line "engine already running in a window" when the lock is taken. The window's fallback engine (below) also needs the lock; without it the window shows "Can't start the engine" and keeps retrying the socket.

**Local transport.** `$XDG_RUNTIME_DIR` must be an absolute path to a directory owned by the user with mode 0700, else the service and the window refuse with an error naming the path. The socket is `$XDG_RUNTIME_DIR/tasklens/engine.sock`; the `tasklens` directory is created 0700 and checked with `lstat` (directory, owned by the user, not a symlink). An existing socket path is removed only when it is a socket owned by the user and `connect` fails with `ECONNREFUSED` while the service holds the engine lock. On accept the server checks `SO_PEERCRED` uid == its own uid; the client checks the same of the server.

**Remote transport.** The window spawns `ssh` with structured arguments: `-T -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- <target> tasklens bridge`. `<target>` is rejected when it is empty, starts with `-`, or contains whitespace or control characters. Host-key checking is left to the user's ssh config. stdout carries only protocol bytes; stderr is drained continuously into a 64-line ring. Deadlines: process start plus handshake 20 s. Failures are named from the exit status and stderr: 255 with "Permission denied" (key needed), "Host key verification failed", 127 or "not found" (tasklens not installed there), protocol refusal (version). `tasklens bridge` connects its stdin and stdout to the local socket, running `systemctl --user start tasklens-engine.service` first when the socket is missing, and tells the server in `Hello` that it is a bridge, so the server knows the session is remote. Reconnect after a drop with backoff 1, 2, 4, 8, 16, 30 s while the host tab exists.

**Envelope** (`remote_proto.rs`, extends `wire.rs`). Every frame: magic `TLR1` (4 bytes), protocol version `u16`, flags `u8` (bit 0 zstd), encoded length `u32`, decoded length `u32`, payload. Limits: encoded and decoded length at most 64 MiB each, decompression into a buffer of exactly the decoded length, trailing bytes and short payloads are errors that close the connection. Payloads larger than 1 KiB are zstd level 1. `Hello`, `Welcome` and `Refused` are bootstrap frames: the envelope's own `TLR1` is the only magic, and the payload starts with a fixed prefix that never changes between versions: `proto: u16`, `app_version` as a `u16` byte length plus UTF-8 (at most 64 bytes), `flags: u32`, all little endian. After the prefix comes a postcard tail that a peer decodes only when `proto` matches: `Hello` carries the window's pid, app id, the bridge flag and what it wants; `Welcome` carries `host_name`, `machine_id`, `boot_id` and `engine_instance`. `Refused` has no tail; after its prefix come a reason code `u16` and a reason text (`u16` length plus UTF-8), so a version mismatch is always readable. Compatibility is `proto` equality; app versions may differ when `proto` matches. `Refused` names the server's `proto` and app version.

**Sessions.** The server keeps one session per connection, up to 8. Each session has its own `Demand`, its own `TableQuery` and table projection, its own detail and history window requests (`Snapshot.device_history` demand, ADR-017), its own journal streams and its own action namespace. The engine collects for the union of all sessions' demands plus the service's own background demand (below); the reducer builds the table view per session (ADR-004's single `TableQuery` becomes one per session). A session that is not shown in any window asks only for the title bar value.

**Background demand.** With no client connected the service keeps collecting what history needs: the device series of ADR-017 at their 1 s rate, the memory group sweep every 10 s (UI-PRF-060 Data binding) and the event detector's journal sources, as the window does today when hidden to the tray.

**Snapshot deltas.**
- The server numbers the frames it sends to a session (`frame: u64`, from 1 per session). A frame carries `base: u64`, the frame it is a delta against (0 means complete), and every part of `Snapshot` whose content differs from `base`'s.
- The client sends `Ack { frame }` after it has applied a frame and installed the result. The server keeps the `Snapshot` (Arc clones) of the newest acknowledged frame and always builds the next delta against it. Frames in flight never become a base.
- Parts: `table` and `processes` are row deltas keyed by `ProcessKey` (added rows whole, changed rows as changed fields, removed keys); `series` and `process_series` send, per series id, the samples after the base's last sample time, or the whole ring when the base's last sample is no longer in the ring, and list removed series; `device_history` is sent whole when its request generation changes and as appended points otherwise; every other part is sent whole when its `Arc` differs from the base's.
- The client builds a complete new `Snapshot` from its installed one plus the frame, gives it the client's own strictly increasing `sequence` (so ADR-005's hub check holds across engine restarts), stores it in its `ArcSwap`, then `try_send`s the wake token. UI code compares component versions against the installed snapshot, never against a frame's contents.
- A complete frame is sent first, after an engine instance change, and every 60 s.

**Queues and back-pressure.** Per session the server has: a snapshot slot of one (latest wins, built from the newest publish when the writer is free), the ordered action event queue (256; overflow closes the session), per-stream journal queues (64 batches; overflow ends that stream with `JournalError::Overflow`), and replies. One writer thread per session sends in the order actions, replies, journal, snapshot. A session with no `Ack` for 30 s is closed. Collection never waits on a socket. Client side: `send` puts the command on a bounded queue of 64 and returns `Err(Busy)` when full, `Err(EngineStopped)` while disconnected; it never blocks.

**Identity and reconnects.** `Welcome` carries `host_name`, `machine_id`, `boot_id` and `engine_instance` (128 random bits per engine process). The client keys a host by `machine_id`. On a new `engine_instance` or `boot_id` the client drops its installed snapshot and starts from the complete frame. Action ids are per session; the server maps `(session, ActionId)`, so ids from two windows never mix. When a connection drops, every action sent and not yet finished becomes `OutcomeUnknown("connection to <host> lost")` in the UI; commands are never replayed. Process targets keep ADR-006's checks on the server (`boot_id`, start time, pidfd); a target captured before a reconnect is checked against the new state like any other.

**Journal.** `JournalOpen { stream, query }`, `JournalCtl`, `JournalClose`. The server runs `journalctl` as ADR-016 does, batches at most every 100 ms and kills the child on close, on session end and on handle drop (the client sends `JournalClose` from `Drop`).

**History.** ADR-017's read path stays: history reaches the UI through `Snapshot.device_history` on demand, with its gaps, boot markers and time base. The protocol adds no history query.

**Settings** (amends ADR-011). Two owners:
- Host settings, in `$XDG_CONFIG_HOME/tasklens/engine.toml`, written only by the engine service: `processes.interval_ms` and the `[history]` table, plus `remote_admin` (below). A client reads them with `GetHostSettings` (reply `HostSettings { rev, values }`) and changes them with `SetHostSettings { base_rev, changes }` (reply `Ack { rev }` or `Conflict { current }`; the UI reloads on conflict). A client never sends defaults it has not read.
- Window settings, in `settings.toml` as today, written only by the window: everything else, including the network unit, the adapter kinds counted, `subtree_totals` and `hide_system`, which move from `EngineSettings` into the session's `TableQuery` and demand.
- Migration: on first start the service creates `engine.toml` from the host keys in `settings.toml`; the window stops writing them. ADR-011's atomic writes, permissions and future-schema read-only rule apply to both files.

**Remote root actions.** Every action runs on the host that owns the process. Root actions go through that host's privilege paths:
- Process actions through `tl-helper` (polkit action `io.github.oysteinkrog.tasklens.helper`, ADR-006, with its request checks unchanged).
- Service start, stop, restart, enable and disable through systemd's D-Bus API (polkit `org.freedesktop.systemd1.manage-units` and `manage-unit-files`).
- `tasklens-setup` operations (polkit `io.github.oysteinkrog.tasklens.setup`).
- SMART reads through `/usr/lib/tasklens/tasklens-smart <disk>` (polkit `io.github.oysteinkrog.tasklens.smart`).
- Generic administrator launches (`pkexec <program>`) run on the host's desktop session and its polkit agent, so on a remote host someone must answer the prompt there.
With nobody at the remote screen, a polkit prompt cannot be answered. `tasklens remote-admin on`, run as the user on a host, prints the reach sentence below and asks for confirmation. It then runs `tasklens-setup` through pkexec only to install `/etc/polkit-1/rules.d/49-tasklens-remote-admin.rules`, which returns `YES` for exactly the action ids listed under Privilege coverage when `subject.user` is the user who ran it (`PKEXEC_UID`); `manage-units` only when `action.lookup("unit")` is set (single units, no daemon reload). Last, it saves `remote_admin = true` in that host's `engine.toml` through the engine socket, as a control-only client (`Want { snapshots: false }`); `on` and `off` are each one transaction: the command holds an exclusive `flock` on `$XDG_RUNTIME_DIR/tasklens/remote-admin.lock` for the whole run (a second command waits up to 10 s, then fails with a plain message), and before it changes anything it asks `tasklens-setup remote-admin state` whether the rules file is present or absent and keeps the answer itself; the script keeps no backup. The commit point is the successful socket save. Any failure before it, once the script was asked to change the file, of any kind (a failed spawn, any pkexec or script exit code, 126 and 127 included, since pkexec passes the script's own exit code through, a failed wait or output read, or the save failing), runs the restore before anything is printed: the restore is idempotent and sets the rules file to the recorded state through the script (`remote-admin on` for present, `remote-admin off` for absent), whatever the file is now, so the two do not disagree and a closed terminal cannot skip the restore. After the commit point nothing is undone; a failure to print the result only sets a non-zero exit code. The lock file is the same for every `--app-id`, because there is one rules file per host. When the socket does not answer, the command starts the engine unit once, as `tasklens bridge` does, and fails before pkexec when the engine still does not answer. `tasklens remote-admin off` saves `false` first, which closes admission at once, then removes the file; when the save fails nothing changes, and the save is its commit point, so when removing the file fails after it nothing is undone: the setting stays off and the command says so. `tasklens remote-admin status` prints the setting and polkit's answer for the engine process (the check under Privilege coverage), and says whether they agree. The setup script (`tasklens-setup remote-admin on|off|state`) runs only as root and never writes `engine.toml`. This is honest about its reach: with it on, any process running as that user on that host gets those actions without a prompt, whether it came through tasklens or not. `tasklens remote-admin on` prints that sentence before asking for confirmation. Admission (ADR-006) refuses a root action from a remote session while remote admin is off on the host, with `RejectReason::RemoteAdminOff` and the text "Remote admin is off on home. Run tasklens remote-admin on there." A root action is a privileged request (helper), `SockDestroy`, or a unit action on the system manager. A session is local only when it is the window's in-process session or its `Hello` names a window on this host; a bridge session and a session with no `Hello` count as remote, so the check fails closed. The window always asks before a root action on a remote host: "End task as root on home?".

**Commands that act on the host's desktop.** Every command runs on the host it targets, including switch to window, open file location, open a terminal, run new task and the KWin bridge actions: they act on that host's screen. On a remote host the menu item gets the suffix "on home" and the toast after it says so. The service reads the desktop session's environment (`WAYLAND_DISPLAY`, `DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `XDG_ACTIVATION_TOKEN` is not used) from the systemd user manager, which Plasma fills at login, and starts the KWin bridge when the session bus appears and again after it goes away and comes back. Each window sends its own process id and app id in `Hello`, so the KWin bridge can tell tasklens windows apart from the engine's pid. When the host has no desktop session, these commands fail with `Rejected(NoDesktopSession)`: "home has no desktop session".

**Dumps on a remote host.** Dump files are written on the engine host under `$XDG_STATE_HOME/tasklens/dumps/`; progress comes as `ActionEvent`s from the server instead of the UI polling the file (ADR-005 changes for all hosts). On a remote host the result toast says "Saved on home: <path>" with "Copy path"; "Open folder" shows only for the local host.

**Window fallback.** When the local socket cannot be reached within 3 s of start and `systemctl --user start` fails, the window runs an in-process engine as today, if it can take the engine lock, and shows "Engine service not running" in the status bar. It keeps trying the socket every 30 s; when the service comes up (it cannot while the window holds the lock), nothing changes until the window restarts.

**Budgets.** Measured by tests with recorded fixtures: a host with 1500 processes on the Processes tab at most 64 KiB/s on the wire at 1 s refresh; a hidden host at most 2 KiB/s; the service with no clients uses no more CPU than today's engine with the window hidden to the tray, within 10%.

### Consequences

- History and the event detector no longer depend on the window being open.
- The local window always runs the wire protocol, so remote hosts get the same code path.
- `proto` becomes a compatibility surface; both PCs should run the same release, which the owner's release flow gives.
- `remote-admin on` lets any process of that user on that host use the listed root actions without a prompt. It is off by default and per host.

### Implementation contracts (settled after Astra's second review, 2026-10-29)

Each bead below owns its contract and a test for it.

Four rules hold for every connection, thread and queue in this ADR (leader decisions on q4x76.5, .7, .8, .18, .20, 2026-10-31):

- **One deadline per operation.** An operation (a connection attempt, a handshake, a frame write, a child process's shutdown) has one overall deadline on a monotonic clock. Every blocking step inside it (connect, read, write, join, wait) is bounded by the time left, never by its own fresh timeout. Past the deadline the operation fails and frees what it holds: the session slot, the child process, the pipe threads. Ending an operation for any reason (deadline, EOF on either side, error) must wake every step blocked inside it: use poll on non-blocking fds, or shut down or close every fd a thread may block on. A thread that can stay blocked after its operation ended is a bug.
- **Up means handshaken.** A connection counts as up only after its handshake is complete and checked, never when a socket accepts or a process starts.
- **Level-triggered state.** A latest-value request (demand, table query, settings) is kept and re-offered on the receiver's own tick until it is taken; close and end markers never travel through a queue that can drop them.
- **Generations.** Every stream, subscription, client or engine instance carries a generation. Opening, switching or losing a connection bumps it at that moment (not when the next snapshot arrives), and anything from an older generation is dropped.

- **Delta flow.** At most one unacknowledged frame per session. The next frame is built only after the `Ack`, against the acknowledged snapshot, from the newest publish (latest wins). A series whose base sample has left the ring, or whose ring was reset, is sent whole. The table part carries the row order as a key list whenever the order changed. Component versions on the client are the server's, copied as they are; the client resets its hub comparison state on every reconnect and engine instance change. History points that change after they were sent (decimation) make `device_history` go whole.
- **Queues in bytes.** Replies: 16 messages or 4 MiB. Journal: at most 4 streams per session, 1 MiB queued per stream. Snapshots are never starved: one goes out at least every 2 s while others wait. A write blocked for 10 s closes that session only. `JournalClose` and `Ack` travel on the control path, ahead of the command queue, so closing works when the queue is full; follow and cursor rules stay as ADR-005.
- **Per session.** One `SnapshotHub` per host connection in the window. Server limits per session: 8 detail requests, 4 history windows, 4 journal streams.
- **Reconnect.** Until the first complete frame after a reconnect, the host's pages show their last data dimmed with "Reconnecting to home" and every action is disabled. UI action tracking is keyed by host and session.
- **Privilege coverage.** `remote-admin on` covers the four action ids in `io.github.oysteinkrog.Tasklens.policy` (`io.github.oysteinkrog.tasklens.helper`, `.setup`, `.netmon` and `.smart`) and systemd's `manage-units` (only with a `unit` detail) and `manage-unit-files`. SMART reads run `pkexec /usr/lib/tasklens/tasklens-smart <disk>`, the `.smart` action's fixed program: the script checks the kernel disk name (1 to 32 bytes of `[a-z0-9]`, starting with a letter) and always runs `smartctl --json=c -a /dev/<disk>`, so the action grants SMART reads and never smartctl with other arguments. The rule never covers the generic `org.freedesktop.policykit.exec` action, because that would let any process of the user run any program it names, smartctl included, as root with any arguments. The engine cannot read `/etc/polkit-1/rules.d` (root:polkitd 0750), so it checks the grant instead of the file: at start, and each time `remote_admin` turns on, it asks polkit `CheckAuthorization` for `io.github.oysteinkrog.tasklens.helper` with its own process as the subject and no user interaction. Remote admin is on only while `remote_admin` is true and polkit answers authorized. Until polkit answers, and when it says no or cannot be asked, remote admin is off and the reason is logged. This is kept in memory only: the check never rewrites `engine.toml`; `tasklens remote-admin status` reports the disagreement.
- **Envelope details.** Little endian. Bootstrap frames are never compressed. zstd window at most 8 MiB (`windowLogMax 23`), one decode buffer of the declared decoded length, total buffered input per session at most 80 MiB. A snapshot frame that would exceed 64 MiB is sent without `process_series` and `device_history`, which follow in later frames; this is logged.
- **Endpoints.** An existing `tasklens` runtime directory with the wrong owner or mode is an error, not fixed silently. stderr ring: 64 lines, 16 KiB. ssh options never turn off host-key checking. `tasklens bridge` runs `systemctl --user start` and retries once when connecting fails for any reason.
- **History off.** Background demand follows host settings: with `history.device_metrics` off the service collects nothing for history and runs the memory group sweep only on demand.
- **Settings migration.** The release that adds the service stops writing host keys from the window. The service imports them from `settings.toml` once, then owns them; its revision counter is stored in `engine.toml` and survives restarts. `tasklens remote-admin` changes `remote_admin` through the engine socket, never by writing the file. Each machine has one window process (single instance, ADR-010), so `settings.toml` keeps one writer.
- **Lifetime.** "Always on" means from login to logout of the owner's user, plus linger when turned on. When the window's fallback engine holds the lock, the window shows "Switch to engine service" in the status bar; it stops its engine (flushing history), releases the lock, starts the unit and connects.
- **Rescue mode.** `--low-memory` keeps its own stripped engine (ADR-004). It writes no history, so it neither takes nor needs the engine lock.

---

## ADR-019: Configurable widget layout

Status: accepted (D-041, 2026-10-31). Phase P2. This ADR turns `docs/plan/layout-system.md` into build rules. Phase 1 (epic E27, beads `tasklens-lhliq.2` to `.8`) builds it for the Overview GPU box only. Phase 2 (epic E28, `tasklens-6swdr`) moves every other Overview box onto the same types and adds group and fixed widgets (section "Phase 2"). Phase 3 (epic E29, `tasklens-hzire`) makes every device page a container (section "Phase 3"). Phase 4 reuses the same types for named presets and copy layout, in its own epic.

### Context

Each Overview box has 1 to 5 fixed modes (UI-PRF-019), an Arrange choice, one inner splitter and some kind-specific keys in `OverviewGridSetting` (`mode`, `split`, `arrange`, `power`, `power_split`, `power_placement`, `clocks`, `hidden`). The owner wants to pick freely what a box shows: any metric of the device (each GPU engine, VRAM, clocks, power, temperature) as any mix of chart, load bar, current value, stats and name, placed on a snap grid inside the box (D-041). The Overview must keep its render contract (UI-PRF-017, UI-PRF-019): a sample never makes the page or a box notify, and each chart is a cached view. Settings written by 1.1.0 must keep their look, and an older build must still open a file a newer build wrote.

### Decision

**Terms.**

- A **container** is a box or page that holds widgets. Its id is a string: `overview.<kind>` for an Overview box (phase 1: `overview.gpu` only), and `page.<persistent device key>` for a device page (`page.cpu`, `page.gpu:GPU-…`; see "Phase 3"). Overview kinds are unique on the page (UI-PRF-056), so the kind names the box.
- A **widget** shows one metric of one device (its **source**) in one or more **forms**: `chart` (history over the box range), `bar` (load bar), `value` (current value), `stats` (min, average and peak over the range) and `label` (the source's name). Phase 1 has single-metric widgets only. Phase 2 adds group widgets (all cores, disks, adapters or engines) and fixed widgets (the Processes table, the Timeline, Memory by app); see "Phase 2". The Details grid widget and the page parts come in phase 3.
- A **composition** is the fixed arrangement a widget uses for its forms at its size (UI-PRF-061): `Cell`, `Row` or `Tile`.
- A **layout** is a container's list of widgets. It is either a **flow layout**, placed by a built-in **preset** (one of today's modes), or a **grid layout**, where every widget has its own cells on the container's inner snap grid. Defaults and converted old settings are flow layouts. The first applied inner edit turns a flow layout into a grid layout (see "Editing").

**Types** (`tl-model/src/layout.rs`, plain serde data like `OverviewLayoutSetting`; `tl-ui` turns them into its views):

```rust
pub const LAYOUT_VERSION: u32 = 1;
pub const GPU_BOX_CONTAINER: &str = "overview.gpu";
/// The containers this build supports. Phase 1: GPU_BOX_CONTAINER only.
pub const LAYOUT_CONTAINERS: &[&str] = &[GPU_BOX_CONTAINER];
/// The inner grid of the GPU box (see "Inner grid").
pub const GPU_BOX_GRID_COLS: u16 = 6;

/// `[[performance.layouts]]` in file order.
pub struct LayoutsSetting(pub Vec<LayoutEntry>);

pub enum LayoutEntry {
    /// A container this build supports, at a version it reads.
    Known(ContainerLayout),
    /// A container this build does not support, or a newer `version`: kept as read and
    /// written back unchanged. The container keeps using its old settings.
    Kept(toml::Table),
}

pub struct ContainerLayout {
    pub container: String,
    pub version: u32,
    /// `Some`: a flow layout placed by a built-in preset. `None`: a grid layout.
    pub preset: Option<PresetSetting>,
    /// Empty is an empty box, which is not the same as no entry (the default).
    pub widgets: Vec<WidgetSetting>,
    /// The id the next new widget takes. It only grows: removing a widget never lowers it, so
    /// an id is never given to a second widget. On load it is raised to at least the highest
    /// widget id plus 1. Key `next_id`.
    pub next_id: u32,
}

pub struct PresetSetting {
    /// A mode id of the container's kind in OVERVIEW_BOX_MODES: for the GPU box
    /// "engine_charts", "chart_engines" or "chart".
    pub id: String,
    /// An id from OVERVIEW_ARRANGES; None is the preset's default.
    pub arrange: Option<String>,
    /// In OVERVIEW_SPLIT_RANGE; None is the preset's default.
    pub split: Option<f64>,
}

pub struct WidgetSetting {
    /// Unique in its container, taken from `ContainerLayout::next_id`, so an id is never
    /// reused while the entry exists, not even after the widget that had it is removed.
    pub id: u32,
    pub source: SourceSelector,
    pub forms: FormSet,
    /// Required in a grid layout, never written in a flow layout.
    pub cell: Option<WidgetCell>,
    pub options: WidgetOptions,
    /// The device's name when the widget was placed, shown only while the device is gone.
    pub device_name: Option<Arc<str>>,
}

/// A durable source: never a runtime SeriesId or DeviceIdx.
pub struct SourceSelector {
    pub device: DeviceSelector,
    pub metric: MetricKey,
}

pub enum DeviceSelector {
    /// The device's persistent key (PERF-005, R-155, ADR-017), for example
    /// "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9" or "gpu:0000:01:00.0".
    Key(Arc<str>),
    /// The nth device the box shows (0 or 1 for the GPU box), in sidebar order. Flow
    /// layouts only, so a default or converted layout follows the GPUs present.
    Slot(u8),
}

/// A stable metric name, written as text (see "Metric keys"). An unknown key is kept.
pub struct MetricKey(pub Arc<str>);

bitflags::bitflags! {
    /// Never empty in a loaded widget. Written as names in this order.
    pub struct FormSet: u8 { const CHART = 1; const BAR = 2; const VALUE = 4; const STATS = 8; const LABEL = 16; }
}

pub struct WidgetCell { pub col: u16, pub row: u16, pub cols: u16, pub rows: u16 }

/// Chart options. Written only when not the default.
pub struct WidgetOptions { pub fill: bool /* true */, pub dashed: bool /* false */ }
```

`PerformanceSettings` gains `layouts: LayoutsSetting`, written only when it has an entry. A kernel overlay option (CPU) and group sources come with the containers that need them.

**Metric keys** (phase 1, GPU devices). `tl-model/src/layout.rs` maps each key to its `MetricId`:

| Key | MetricId | Name (en-US) |
|---|---|---|
| `engine:<key>`, `<key>` from `GpuEngine::key()` (`gpu`, `memctl`, `enc`, `dec`, `jpeg`, `ofa`, `compute`, `dma`, `render`, `copy`, `video`, `video-enhance`) | `GpuEngine::metric()` | the engine's name on the GPU page for the GPU's vendor |
| `vram` | `GPU_MEM` | VRAM |
| `clock_gfx` | `GPU_CLOCK` | Graphics clock |
| `clock_mem` | `GPU_MEM_CLOCK` | Memory clock |
| `power` | `GPU_POWER` | Power draw |
| `temp` | `GPU_TEMP` | Temperature |

Later phases add keys; a key is never renamed or reused.

**Settings schema** (canonical table (a)):

```toml
[[performance.layouts]]
container = "overview.gpu"
version = 1
widgets = [
  { id = 1, device = "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9", device_name = "NVIDIA GeForce RTX 4090", metric = "engine:gpu", forms = ["chart", "value", "label"], col = 0, row = 0, cols = 3, rows = 4 },
  { id = 2, device = "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9", device_name = "NVIDIA GeForce RTX 4090", metric = "vram", forms = ["bar", "value", "stats", "label"], col = 0, row = 4, cols = 6, rows = 1 },
]
```

A flow layout has `preset = { id = "chart_engines", arrange = "auto", split = 0.5 }` and widgets with `slot = 0` or `slot = 1` instead of `device`, and no cells.

**Load rules** (`Settings::clamp`, one `warn!` line for all layout drops, reported with `LAYOUTS_KEY = "performance.layouts"` as `OVERVIEW_KEY` is):

- An entry without a string `container` is dropped. A second entry for the same container is dropped.
- An entry for a container not in `LAYOUT_CONTAINERS`, or with `version` above `LAYOUT_VERSION`, becomes `LayoutEntry::Kept` and is written back unchanged. A missing `version` reads as 1.
- An unknown preset id reads as the kind's default preset. `arrange` and `split` follow the UI-PRF-019 rules.
- A widget is dropped when its `id` is missing or repeated, or when it has no known form. Unknown form names are ignored.
- A widget is dropped when it has both `device` and `slot`, or neither, or a `slot` above 1, or a `slot` in a grid layout.
- In a grid layout, a widget is dropped when its cells are missing, have a span below 1, end past the grid's columns or overlap an earlier widget.
- An unknown metric key or a device key that no device has is **kept**. Such a widget shows its unavailable state (UI-PRF-061).

**Conversion of the GPU box** (`layout::convert_gpu_box(&OverviewGridSetting) -> ContainerLayout`). It runs in `Settings::clamp` for the GPU box when there is no `overview.gpu` entry and the GPU box's grid entry has `mode`, `split`, `arrange` or `hidden`. The result is a flow layout:

- `preset.id` is the entry's `mode` (or `engine_charts`), with its `arrange` and `split`.
- Widgets, for slot 0 and then slot 1, ids from 1 in this order:
  - `engine_charts`: `engine:<key>` for every key in `GpuEngine::ALL` order with forms `chart`, `value`, `label`; then `vram` with `bar`, `value`, `label`.
  - `chart_engines`: `engine:gpu` with `chart` (the section chart of UI-PRF-017); `engine:<key>` for every key in `GpuEngine::ALL` order except `memctl`, with `chart`, `bar`, `value`, `label`; `vram` with `bar`, `value`, `label`; `temp` with `value`; `power` with `value`.
  - `chart`: `engine:gpu` with `chart`; `vram` with `bar`, `value`, `label`.
- `hidden` names (en-US, D-029) leave out the widgets they name. Each name maps to engine keys the same way for every vendor: "3D / Compute" and "Graphics" to `gpu`, "Video Encode" to `enc`, "Video Decode" to `dec`, "Video Encode/Decode" to `enc` and `dec`, "Memory controller" to `memctl`, "Compute" to `compute`, "DMA" to `dma`, "JPEG" to `jpeg`, "Optical flow" to `ofa`, "Render/3D" to `render`, "Copy" to `copy`, "Video" to `video`, "Video Processing" to `video-enhance`, and "VRAM" to `vram`. An unknown name is ignored.
- The built-in default (no entry and no old key) is `convert_gpu_box` of an entry with no keys. It is used in memory and never written.
- Conversion is deterministic: the same entry always gives the same layout, and it reads no device. It is idempotent: once an `overview.gpu` entry exists, conversion does not run again.
- The old keys of the GPU box's grid entry are kept as loaded and written back unchanged, and a build with an `overview.gpu` entry ignores them. So an older build that drops `[[performance.layouts]]` still shows the old mode. Every other box keeps using its old keys. The one exception is a reset to the default ("Editing", Reset): it removes them.
- Before the first save after a load that converted a container, `tl-app` (`settings_store.rs`) copies the current `settings.toml` to `settings.toml.pre-layouts-v1` in the same directory, mode `0600`, unless that file already exists. `Settings::clamp` reports the conversion with the key `LAYOUTS_CONVERTED_KEY = "performance.layouts.converted"`, which the load step logs with `info!` and does not show as a clamped value.

**Metric metadata** (`tl-ui/src/pages/performance/widget/meta.rs`, `MetricMeta`, one entry per metric key). Each entry gives: the unit; the `ValueFormat`; the chart `YScale`; gauge or counter; the bar's denominator (`BarMax::Fixed(100.)`, `BarMax::Total(fn)` from the device record, or `BarMax::None`); and the stats rule (`StatsRule::MinAvgPeak` or `StatsRule::None`). The forms a metric offers follow from it: `bar` only with a denominator, `stats` only with a stats rule. Phase 1 entries:

| Key | Format | Scale | Kind | Bar max | Stats |
|---|---|---|---|---|---|
| `engine:*` | `Percent` | `Fixed { 0, 100 }` | gauge | 100 | min, avg, peak |
| `vram` | `Bytes`, as "3.1 / 16.0 GiB" (UI-PRF-017 VRAM row) | `FixedTotal(mem_total)` | gauge | `mem_total` | min, avg, peak |
| `clock_gfx` | `Mhz` | `StickyUp { hw_max: clock_gfx_max_mhz }` | gauge | `clock_gfx_max_mhz`, else no bar | min, avg, peak |
| `clock_mem` | `Mhz` | `StickyUp { hw_max: clock_mem_max_mhz }` | gauge | `clock_mem_max_mhz`, else no bar | min, avg, peak |
| `power` | `Watts` | `StickyUp { hw_max: power_limit_w }` | gauge | `power_limit_w`, else `power_max_limit_w`, else no bar | min, avg, peak |
| `temp` | `Celsius` | `StickyTemp` | gauge | `slowdown_temp_c`, else no bar | min, avg, peak |

A counter (later phases, for example energy) offers no bar and no stats; its value is the running total. `MinAvgPeak` covers the box range (`performance.overview_range`) with the figures UI-PRF-055 gives box titles: the valid samples of the live series for 1 min and 5 min, the `DeviceHistory` points (min of the mins, mean of the means, max of the maxes) for 1 h, 24 h and 7 d. With no valid sample in the range the stats text is empty, as in the box titles. The text uses the existing string `perf-overview-figures` ("min {$min}  avg {$avg}  peak {$peak}").

**Entities and data flow** (`tl-ui/src/pages/performance/widget/`):

- `MetricWidget` (`metric_widget.rs`) is one GPUI entity per widget, keyed by its widget id. It owns its `ChartView` when it shows a chart, picks its composition from its forms and size (`compose.rs`), formats its text only when the value, stats or bar bucket changed, and notifies only then.
- `SourceData` (`source.rs`) is one entity per source in use. It resolves the `SourceSelector` to a `SeriesId` from the device catalog (again only when the catalog changes), subscribes to `SnapshotHub`, and on a snapshot whose series version changed it updates the current value and the stats and notifies. Each `MetricWidget` observes its `SourceData`. Widgets on the same source share one `SourceData`, so history is read and stats are computed once per data revision. Everything `SourceData` derives that a widget reads (reading, vendor, device, series, network total, minus list) and every input its text is formatted from (key, followed settings, strings version) lives in one `SourceState` with `PartialEq`, and an update compares the whole state before and after, never single fields, so a new derived field cannot be left out of the notify decision. `SourceStore` (one per Overview page) maps selectors to `SourceData` entities and drops an entry when no widget uses it.
- `SourceData` emits `AvailabilityChanged` when its device goes or comes back, or the device stops or starts reporting the metric. This is the only source event the container subscribes to.
- `WidgetContainer` (`container.rs`) holds the widgets of one container: it places them (flow or grid), keeps a map from widget id to `Entity<MetricWidget>`, and adds the sources it shows to the page's demand part. A layout change compares by widget id: a widget whose id stays keeps its entity and gets its new settings in place; only new ids get new entities. The container re-renders on a layout change, a resize, an edit mode change and `AvailabilityChanged`, never because of a sample.
- **Selectors.** Widget ids are unique only in their container, so every widget-level debug selector and every widget-level GPUI element id that no parent id scopes has the container id, for example `perf-widget-overview.cpu-1`; the selector helpers (`widget_id`, `part_id`, the group `scroll_id`, the edit `frame_id`, `grip_id`, `gear_id`, `outline_id`, `corner_id`) take the container id, never the bare widget id (tasklens-6swdr.10). Set every tl-ui debug selector through `TlSelector::tl_selector` in `tl_ui::selector`, never through GPUI's `debug_selector` (tasklens-6swdr.11).
- Chart geometry stays cached by `ChartView` (ADR-009): rebuilt only when the series version, the scale or the size changes.
- Hidden widgets do no work. A widget that takes no space (a metric the device does not report, an empty slot, a widget cut by a fit rule) has no entity. A `SourceData` with no observer is dropped, and nothing reads a source while the Overview page is not shown (the demand part is withdrawn, as today).
- **Render contract.** UI-PRF-017 and UI-PRF-019 still hold. GPUI marks every ancestor of a notifying view dirty (crw.34), so a widget's notify can make the container and page render once in that frame. A view GPUI re-renders also re-renders its cached children (`window.refreshing`, gpui-pre 0.3.7 `view.rs`), so the other widgets in the container can render once too. Each of these renders does fixed work: it reads no snapshot, formats nothing and builds no geometry; a widget that did not notify draws the text it already holds. The gate is therefore: on a sample, the page and the container never notify, no widget entity is created or dropped, and only the widgets on a changed source notify.

**Flow placement** (`tl-ui/src/pages/performance/overview/gpu_flow.rs`). A flow layout is placed by its preset's rules, which are the GPU box rules of UI-PRF-017, UI-PRF-019 and UI-PRF-059 with the fixed engine lists replaced by the widget list:

- Slot 0 and slot 1 are the first and second GPU the box shows today (the sidebar's visible GPU cards, in order). At most two sections show; the second only when both fit; GPUs left over give the "+n more" row.
- In a section, engine widgets draw in the order of the old mode: the GPU page grid order (`grid_engines`) in `engine_charts`, and the vendor order (`shown_engines`, with one shared "Video Encode/Decode" row when `enc_dec_shared`) in `chart_engines`. Other widgets keep list order after them. An engine widget whose engine the device does not report, or that `performance.hidden_gpu_engines` hides, takes no space. That global setting applies to flow layouts only; a widget in a grid layout shows what the user placed.
- In `engine_charts` the widgets with a chart are cells of `parts::fill_grid` (at least 120 × 48 px) and the rest are rows under the grid. In `chart_engines` the first widget whose only form is `chart` is the section chart, widgets with a bar are spark rows (`spark_row`), and widgets whose only form is `value` share one line, joined by " · " (today's temperature and power line). `arrange`, `split` and the inner splitter work as UI-PRF-019. The UI-PRF-059 fit rules ("+n more engines", rows giving way) are unchanged.
- **One look in every layout** (tasklens-6swdr.12). A widget draws the same in a flow layout, in Edit Widgets and in a grid layout. Its accent comes from the kind of device it reads (the box's colour; a Memory row its row colour), and its chart's series, overlays and legend, its value format and a group's per-item columns come from the metric metadata, the widget's options and the settings every box follows (`performance.cpu_kernel`), never from the placement. The rules are in `widget/look.rs` and `widget/meta.rs`: CPU `total` filled in the CPU colour with the `CPU_KERNEL` line and the Total and Kernel legend while `cpu_kernel` is on; core `load` plotted as 1-second means and shown as a whole percent ("41%", as the CPU, memory pressure and disk active values); Memory rows in their row colours and `composition` as three stacked bands with their legend; disk `io` read filled and write dashed; Network download filled and upload as the lighter line, the hidden kinds taken off the aggregate; a cores group of `load` in Rows drawn as the core rows (name, sparkline, meter, percent and clock column, the core rows' fit and columns). Pinning a flow layout to the grid (what entering Edit Widgets does) keeps each widget's forms and options.
- The box menu keeps "Layout" (preset radio items), "Arrange" and "Engines" for a flow layout. They edit the flow layout entry: a preset change replaces the widget list with that preset's list, leaving out the engines the Engines list hides; an Engines change removes or adds that engine's widgets.

**Inner grid** (`tl-ui/src/pages/performance/widget/grid.rs`, pure functions with `proptest` tests, like `overview/layout.rs`). For grid layouts:

- The GPU box grid has `GPU_BOX_GRID_COLS` = 6 columns over the box body width, a row unit of 24 logical px and a 4 px gutter (`sp2`) in both directions. Column width `cw = (W − 5·gutter) / 6`. A later container sets its own column count and unit.
- **Fill** (D-037, D-040): when the widgets end above the bottom of the body, every row grows by the same amount: the drawn row height is `max(unit, (H − (R − 1)·gutter) / R)`, where `R` is the last used row. A taller layout keeps the unit and the body scrolls vertically with a scrollbar. Columns never scroll sideways.
- **Minimum span**: a widget spans at least `max(1, ceil(min_w / cw))` columns and `max(1, ceil(min_h / unit))` rows, where `min_w × min_h` is the smallest composition of its forms (UI-PRF-061). It is enforced when a move, resize or add ends. At draw time a widget keeps its saved span, and a widget drawn below its minimum drops forms in the UI-PRF-061 order.
- **Insert** (Add Widget): at the new widget's default span (3 × 4 cells when it shows a chart, else 6 × 1), raised to its minimum span, in the first free place scanning rows top to bottom and columns left to right; with no free place, under the lowest widget. After an add, and after a move or resize that ends below or above the visible body, the body scrolls the least that shows the widget fully (its top, when it is taller than the body).
- **Move**: the widget snaps to whole cells. Widgets in the way move down to make room, and widgets above a freed space close up, as the Overview grid does (UI-PRF-056).
- **Resize**: the bottom-right handle changes the span in whole cells, clamped at the minimum span and at the last column. Widgets in the way move down.
- **Shrink** (the box gets smaller): the layout does not change. Spans are kept, the body scrolls vertically, and compositions drop forms.
- **Scale**: the unit, gutter and every minimum are logical px times `ui_scale` (ADR-009). Spans do not change with the scale.
- No two widgets overlap after any action (proptest over random action sequences).
- **Same look as the flow layout** (tasklens-6swdr.12): a widget on the grid draws its accent, series, overlays, legend, value format and group columns by the rule "One look in every layout" under "Flow placement". Moving or resizing a widget changes its composition only (UI-PRF-061), never these.
- **Edit mode**: no edit decoration (grip, gear, border, selected outline, resize corner, placement preview) covers any text of any widget, and the body scrolls exactly as in the applied view: the wheel scrolls it over the widgets, and entering Edit Widgets keeps the scroll offset. UI-PRF-061 "Edit chrome" gives the margins a widget keeps free.

**Editing** (`tl-ui/src/pages/performance/widget/edit.rs` and `menu.rs`). Editing the boxes on the page (Edit Layout, UI-PRF-018 and UI-PRF-056) and editing the widgets in one box (Edit Widgets, UI-PRF-061) are separate modes, and at most one is on. Starting Edit Widgets ends Edit Layout as "Done" does. Edit Layout is disabled while a box is in Edit Widgets.

- Inner editing is a transaction: `LayoutEdit { container, before: Option<ContainerLayout>, working: Option<ContainerLayout>, undo: Vec<Option<ContainerLayout>>, selected: Option<u32> }`. `None` means no entry (the built-in default). Each action pushes the previous working copy onto `undo`, at most 100 steps.
- Entering Edit Widgets on a flow layout pins it into the working copy (`grid::pin`):
  - Each widget that takes space gets the cells nearest its drawn rectangle: rounded to whole cells, span at least 1, overlaps moved down.
  - Each slot selector becomes the device's persistent key, and `device_name` is set.
  - Widgets cut by a fit rule are placed under the lowest widget at their minimum span.
  - Widgets of an empty slot, and widgets whose metric the device does not report, are left out.
  - When the user cancels, the saved flow layout stays as it was.
- **Apply** saves the working copy: an entry for a layout, no entry for `None` (after Reset). With no change from `before`, nothing is saved.
- **Saving the default** (`layout::reset_to_default(&mut PerformanceSettings, container)`). Apply with `None` removes the container's entry and, in the same save, removes the old keys that conversion reads from the box's grid entry (GPU box: `mode`, `split`, `arrange` and `hidden`). The box's other grid keys (`kind`, `width`, cells) stay. The next load then finds no entry and no old key, so conversion does not run and the built-in default shows; an older build shows its default mode too. Without this step the old keys would convert again at the next load and bring back the customization the user reset. "Reset Box" outside Edit Widgets saves the default the same way, with a flow layout or a grid layout. **Cancel** (and Esc) drops the working copy and places `before` again; entities of widgets in `before` were kept during editing, so none is created again. **Undo** restores the previous working copy. **Reset** asks first, then sets the working copy to `None`; Undo can bring it back before Apply.
- Only Apply writes settings. Nothing is saved during a drag or while editing.

**Files the later beads create or change.**

| Bead | Files |
|---|---|
| `tasklens-lhliq.2` | `tl-model/src/layout.rs` (new: the types, metric key map, load rules, `convert_gpu_box`, `reset_to_default`, the hidden name map), `tl-model/src/settings.rs` (`PerformanceSettings.layouts`, conversion in `Settings::clamp`, `LAYOUTS_KEY`, `LAYOUTS_CONVERTED_KEY`), `tl-model/src/lib.rs` (mod line), `tl-app/src/settings_store.rs` (the `.pre-layouts-v1` copy) |
| `tasklens-lhliq.3` | `tl-ui/src/pages/performance/widget/meta.rs` (new: `MetricMeta`, `BarMax`, `StatsRule`, `MetricKind`, the stats function), `widget/mod.rs` (new) |
| `tasklens-lhliq.4` | `widget/metric_widget.rs` (`MetricWidget`), `widget/compose.rs` (`Composition`, minimum sizes, form drop order), `widget/source.rs` (`SourceStore`, `SourceData`, `AvailabilityChanged`) |
| `tasklens-lhliq.5` | `widget/container.rs` (`WidgetContainer`), `widget/grid.rs` (pure inner grid rules and pinning) |
| `tasklens-lhliq.6` | `widget/edit.rs` (`LayoutEdit`, the edit toolbar, keys), `widget/menu.rs` (widget menu, Add Widget, the Reset and preset confirmations), `overview/box_menu.rs` ("Edit Widgets…"), `pages/performance/actions.rs` (the actions of UI-PRF-061), `assets/locales/en-US.ftl` (the UI-PRF-061 strings) |
| `tasklens-lhliq.7` | `overview/gpu_box.rs` (hosts a `WidgetContainer` and draws each flow widget's entity at its 1.1.0 place), `overview/gpu_flow.rs` (new: the GPU presets' flow placement), `overview/mod.rs` (wiring), `widget/metric_widget.rs` (`FlowLook`: a flow widget draws its 1.1.0 part), `widget/container.rs` (a hosted container: the host draws the flow widgets, the values of one line joined by " · ") |
| `tasklens-lhliq.8` | `crates/tl-model/tests/fixtures/layouts/*.toml` (old GPU box settings), `crates/tl-ui/tests/ui/overview_widgets.rs` (new module of the `ui` test binary), the `perf-overview` bench scenes |

Paths under `widget/` are in `crates/tl-ui/src/pages/performance/widget/`.

### Phase 2: every Overview box (epic E28, `tasklens-6swdr`)

Phase 2 moves the CPU, Memory, Disk, Network, Top processes and Timeline boxes onto the widget container. It reuses the phase 1 types, rules and files: `WidgetContainer`, `SourceStore`, `SourceData`, `MetricWidget`, `compose.rs`, `grid.rs`, `edit.rs`, `menu.rs`, `meta.rs`, and the load, conversion and reset code in `tl-model/src/layout.rs`. Nothing is forked per box. Every rule of phase 1 above holds for every container unless this section says otherwise. That includes the render contract, the editor rule of `tasklens-lhliq.12` (no edit decoration covers text, edit-mode scrolling equals the applied view), Apply, Cancel and Undo, and "Saving the default". The GPU box does not change, except that Add Widget offers its engine group.

**Containers and inner grids.** `LAYOUT_CONTAINERS` holds all seven Overview containers: `overview.cpu`, `overview.memory`, `overview.gpu`, `overview.disk`, `overview.net`, `overview.processes` and `overview.timeline` (the kind keys of `OVERVIEW_BOX_MODES`). `OVERVIEW_GRID_COLS` replaces `GPU_BOX_GRID_COLS` in `grid_cols(container)`:

| Container | Columns | Why |
|---|---|---|
| `overview.cpu` | 12 | The box spans 2 of 3 page columns by default (about 750 px of body), so 12 columns are about 60 px each |
| `overview.memory`, `overview.gpu`, `overview.disk`, `overview.net` | 6 | One page column (about 360 px of body); the GPU value is unchanged |
| `overview.processes`, `overview.timeline` | 12 | Full page width |

Every container uses the row unit of 24 logical px and the 4 px gutter of the GPU box, and the Fill, Move, Resize, Shrink and Scale rules of "Inner grid". The **default span** of a new widget generalizes the GPU rule: a widget with a chart spans half the columns (rounded up) and 4 rows; any other single-metric widget spans all columns and 1 row; a group or fixed widget spans all columns and 8 rows. Each is then raised to its minimum span.

**Widget content.** A widget shows one of three kinds of content. `WidgetSetting.source` becomes `content`:

```rust
pub enum WidgetContent {
    /// One metric of one device (phase 1).
    Metric(SourceSelector),
    /// One metric of every member of a group: every core, disk, adapter or GPU engine.
    Group(GroupSelector),
    /// A widget with content of its own that is not a metric.
    Fixed(FixedWidget),
}

pub struct GroupSelector {
    /// The device the members belong to: `Key("cpu")` for cores, `Key("aggregate:disk")` for
    /// disks, `Key("aggregate:net")` for adapters, a GPU key (or a slot, flow layouts only) for
    /// engines.
    pub device: DeviceSelector,
    pub members: GroupKind,
    /// The metric each member shows. For `Engines` it is `engine:*`: each member shows
    /// `engine:<key>` of its engine.
    pub metric: MetricKey,
    pub options: GroupOptions,
}

pub enum GroupKind {
    Cores,
    Disks,
    Adapters,
    Engines,
    /// A name this build does not know: kept, written back, shown as unavailable.
    Other(Arc<str>),
}

pub struct GroupOptions {
    /// Show at most this many members; the others are counted in a "+n more" item.
    /// `None` shows all. Key `limit`, 1..=256.
    pub limit: Option<u16>,
    /// How the members are drawn. Key `items`.
    pub items: GroupItems,
    /// `Cells` only: an id from OVERVIEW_ARRANGES; `None` is `auto`. Key `arrange`.
    pub arrange: Option<String>,
}

pub enum GroupItems {
    /// One Row composition per member, filling the height as D-040 does.
    Rows,
    /// One Cell composition per member (Tile when the forms have no chart) in a grid.
    Cells,
}

pub enum FixedWidget {
    /// The Top processes table (UI-PRF-017), in `overview.processes` only.
    Processes,
    /// The 24-hour timeline (UI-PRF-057), in `overview.timeline` only.
    Timeline,
    /// The Memory by app chart and its legend (UI-PRF-060), in `overview.memory` only.
    MemoryByApp,
    /// A name this build does not know: kept, written back, shown as unavailable.
    Other(Arc<str>),
}
```

`forms`, `options` and `device_name` keep their phase 1 meaning for metric and group widgets. A fixed widget has no forms and no options: `forms` is not written, and on load it reads as empty and is not a drop reason.

**Settings schema** (canonical table (a)). A group widget writes `group` and its options; a fixed widget writes `fixed` and its cells only:

```toml
[[performance.layouts]]
container = "overview.cpu"
version = 2
widgets = [
  { id = 1, device = "cpu", metric = "total", forms = ["chart"], col = 0, row = 0, cols = 5, rows = 12 },
  { id = 2, device = "cpu", group = "cores", metric = "load", forms = ["chart", "bar", "value", "label"], items = "rows", col = 5, row = 0, cols = 7, rows = 12 },
  { id = 3, device = "cpu", metric = "power_draw", forms = ["chart", "value", "label"], col = 0, row = 12, cols = 12, rows = 4 },
]

[[performance.layouts]]
container = "overview.processes"
version = 2
widgets = [
  { id = 1, fixed = "processes", col = 0, row = 0, cols = 8, rows = 10 },
  { id = 2, device = "cpu", metric = "total", forms = ["chart", "value", "label"], col = 8, row = 0, cols = 4, rows = 10 },
]
```

A disk group writes `{ id = 1, device = "aggregate:disk", group = "disks", metric = "io", forms = [...], items = "cells", limit = 8, arrange = "vertical" }`; an engine group writes `{ id = 7, device = "gpu:<key>", group = "engines", metric = "engine:*", forms = [...], items = "cells" }`. `limit` and `arrange` are written only when set.

**Schema version.** `LAYOUT_VERSION` = 2; this build reads versions 1 and 2. It writes version 1 for an `overview.gpu` entry with single-metric widgets only. It writes version 2 for every other entry. So a 1.2.0 build still reads a plain GPU layout, keeps every version 2 entry unchanged as `LayoutEntry::Kept`, and shows that box by its old keys, which conversion left in place. Before the first save after a load that converted a container, `tl-app` copies `settings.toml` to `settings.toml.pre-layouts-v{LAYOUT_VERSION}` (now `settings.toml.pre-layouts-v2`), mode `0600`, unless that file already exists. An existing `settings.toml.pre-layouts-v1` is left as it is.

**Load rules added in phase 2** (same warn line and keys as phase 1):

- A widget with `fixed` is a fixed widget. It is dropped when it also has `group`, `device`, `slot` or `metric`. It is also dropped when its kind is not allowed in the container (see `FixedWidget`), or when an earlier widget there has the same fixed kind. An unknown `fixed` name is kept as `Other` and shows "Needs a newer tasklens".
- A widget with `group` is a group widget. It is dropped when its device does not fit the group: `Cores` needs `device = "cpu"`, `Disks` `device = "aggregate:disk"`, `Adapters` `device = "aggregate:net"`, `Engines` a `gpu:` key or, in a flow layout, a slot. It is also dropped when `limit` is outside 1..=256, or when it has no known form. An unknown `items` reads as the group's default (`rows` for cores, `cells` for the others); an unknown `arrange` reads as `auto`. An unknown `group` name is kept as `Other` and shows "Needs a newer tasklens".
- `slot` stays a GPU box selector: a `slot` in any other container drops the widget.
- In a grid layout, group and fixed widgets need cells like the others.

**Metric keys added in phase 2.** A key is looked up together with its device's kind, so `temp` and `power` can mean a CPU or a GPU metric. The kind comes from the persistent key (PERF-005): `cpu` is the CPU package, `cpu:<n>` a logical CPU (the key the reducer gives `CpuLogical { id: n }`), `memory`, `disk:…`, `net:…`, `aggregate:disk` and `aggregate:net`. `tl-model/src/layout_boxes.rs` maps each key to its series:

| Device | Key | Series | Name (en-US) |
|---|---|---|---|
| `cpu` | `total` | `CPU_TOTAL` | Utilization |
| `cpu` | `kernel` | `CPU_KERNEL` | Kernel time |
| `cpu` | `freq` | `CPU_FREQ` | Speed |
| `cpu` | `temp` | `CPU_TEMP` | Temperature |
| `cpu` | `power` | `CPU_POWER_PKG` | Package power |
| `cpu` | `power_cores` | `CPU_POWER_CORE` | Core power |
| `cpu` | `power_draw` | `CPU_POWER_PKG` filled and `CPU_POWER_CORE` dashed, one chart (the UI-PRF-019 strip) | Power draw |
| `cpu` | `core_clocks` | `CPU_CORE_CLOCK` of every physical core, one line each (`CoreClocks`, tasklens-o55.57) | Core clocks |
| `cpu` | `pressure` | `PSI_CPU_SOME` | CPU pressure |
| `cpu:<n>` | `load` | `CPU_TOTAL` of the logical CPU | C{n} |
| `cpu:<n>` | `freq` | `CPU_FREQ` of the logical CPU | C{n} speed |
| `memory` | `used`, `cached`, `available`, `swap`, `commit` | `MEM_USED`, `MEM_CACHED`, `MEM_AVAILABLE`, `SWAP_USED`, `MEM_COMMIT` | In use, Cached, Available, Swap, Commit (the `perf-overview-mem-*` strings) |
| `memory` | `pressure` | `PSI_MEM_SOME` | Memory pressure |
| `memory` | `composition` | `MEM_USED`, `MEM_CACHED`, `MEM_AVAILABLE` stacked, with the legend of the `chart` mode | Memory composition |
| `disk:…`, `aggregate:disk` | `active` | `DISK_ACTIVE` | Active time |
| `disk:…`, `aggregate:disk` | `read`, `write` | `DISK_READ`, `DISK_WRITE` | Read, Write |
| `disk:…`, `aggregate:disk` | `io` | `DISK_READ` and `DISK_WRITE`, two lines in one chart (the `disk_charts` cell chart) | Read and write |
| `disk:…` | `temp` | no series: the disk record's `temp_c`, else the SMART temperature, as the Disk page's Temperature row | Temperature |
| `net:…`, `aggregate:net` | `rx`, `tx` | `NET_RX`, `NET_TX` | Download, Upload |
| `net:…`, `aggregate:net` | `traffic` | `NET_RX` and `NET_TX`, two lines in one chart (the `combined` chart) | Download and upload |

On `aggregate:net` the series are the Network box's counted total (`NetTotal`: the engine's total of every class minus the per-class totals of the classes `show_network_*` hides), as the box draws today. The `total` chart draws the `CPU_KERNEL` line under it while `performance.cpu_kernel` is on, in flow and grid layouts alike, as the CPU box does today; there is no per-widget kernel option. A key with two or more series (`power_draw`, `core_clocks`, `composition`, `io`, `traffic`) is a **multi-series metric**: one widget, one source, several lines.

The Name form shows the metric name for a CPU, memory or aggregate device ("Utilization", "In use", "Download"). For a disk or adapter it shows the device name (the sidebar card's name) for `io` and `traffic`, and "{device} · {metric}" for the other keys ("nvme0n1 · Read"). In a group, each member's name follows the same rule; cores are "C{n}" and engines keep their GPU page name.

**Metric metadata added in phase 2** (`widget/meta.rs`). `DeviceFact` becomes `fn(&Snapshot, DeviceIdx) -> Option<f32>`, so a denominator or a scale can read the CPU, memory, disk and network records as well as `GpuTotals`. `MetricMeta` gains `lines: Lines` (`One`, the phase 1 case; `Pair { second_dashed: bool }` for two lines in one chart; `Stacked` for `composition`; `PerCore` for `core_clocks`; `NoSeries` for `disk temp`) and `smoothed: bool`. Chart and stats are offered only with a series (`Lines` other than `NoSeries`). A multi-series metric offers chart, value and name, with no bar and no stats. `smoothed` makes the value and the bar use the core smoothing of UI-PRF-018 (`overview_core_smoothing`), and the chart plot 1-second means, as the core rows and their sparklines do today. Only `load` is smoothed.

**Sources of multi-series and derived metrics** (`widget/source.rs`, bead `tasklens-6swdr.2`). `SourceData` keeps one source per selector, also for a multi-series metric. It resolves the selector to a list of series (`series_list()`; `series()` stays the first one) and to the device record it reads. Its revision bumps when the version of any series in the list changes, and the page's demand part asks for every series in the list. The list is resolved again on a catalog change, which matters for `core_clocks` (one series per physical core). On `aggregate:net` the list is the engine total and the per-class totals of the hidden classes (`NetTotal`), resolved again when a `show_network_*` setting changes. Availability follows the first series: the source is available when the device reports it.

| Key | Format | Scale | Kind | Bar max | Stats | Value text |
|---|---|---|---|---|---|---|
| `total`, `kernel`, `pressure` (CPU and memory), `load`, `active` | `Percent` | `Fixed { 0, 100 }` | gauge | 100 | min, avg, peak | "42%"; `load` smoothed |
| `freq` (CPU and core) | `Mhz` | `StickyUp { hw_max: inventory max_mhz }`, as the CPU page's Clock graph | gauge | the inventory's `processor.max_mhz`, else no bar | min, avg, peak | as the CPU box title ("3.40 GHz") |
| `temp` (CPU) | `Celsius` | `StickyTemp` | gauge | the device's `hot_c`, else no bar | min, avg, peak | "52 °C" |
| `power`, `power_cores` | `Watts` | `StickyUp { hw_max: none }` | gauge | no bar | min, avg, peak | "35.2 W" |
| `power_draw` | `Watts` | as the CPU page's Power draw row | gauge | no bar | none | the package watts |
| `core_clocks` | `Mhz` | as the CPU page's Core clocks chart | gauge | no bar | none | none (offers chart and name) |
| `used`, `cached`, `available` | `Bytes` | `FixedTotal(MemTotal)` | gauge | `MemTotal` | min, avg, peak | `OfTotal`: "8.0 / 31.2 GiB" |
| `swap` | `Bytes` | `FixedTotal(SwapTotal)` | gauge | `SwapTotal` | min, avg, peak | `OfTotal`; "No swap" without swap |
| `commit` | `Bytes` | `StickyUp { hw_max: CommitLimit }` | gauge | `CommitLimit` (the bar stops at full; the value can pass it) | min, avg, peak | `OfTotal` with `CommitLimit` |
| `composition` | `Bytes` | `FixedTotal(MemTotal)` | gauge | no bar | none | In use as `OfTotal` |
| `read`, `write` | `RateIec` | `StickyUp { hw_max: none }` | gauge | no bar | min, avg, peak | "1.2 MB/s" |
| `io` | `RateIec` | one `StickyUp` scale for both lines | gauge | no bar | none | "R 1.2 MB/s  W 340 KB/s" |
| `temp` (disk) | `Celsius` | none | gauge | no bar | none | "38 °C" (offers value and name) |
| `rx`, `tx` | `RateNetwork` (`network_unit`) | the Network page's rule (`network_dynamic_scale`; link speed on an adapter) | gauge | an adapter's link speed when known, else no bar; none on `aggregate:net` | min, avg, peak | "1.4 Mbps" |
| `traffic` | `RateNetwork` | one scale for both lines, as `rx` | gauge | no bar | none | "↓ 1.4 Mbps  ↑ 220 kbps" |

`source.rs` reads each current value from the same record the box shows it from today (CPU totals, memory totals, disk and network records, `GpuTotals`), so a widget and the 1.2.0 box show the same text.

**Group widgets** (implementation rule 5).

- **Members** (`widget/members.rs`, a pure function of the snapshot and the settings):
  - `Cores`: every present `CpuLogical` device, by logical id (C0, C1, …). Each member is the source `{ device = "cpu:<n>", metric }`.
  - `Disks`: the whole disks of the sidebar's visible disk cards, in sidebar order (`device_order`), as the Disk box lists them today. Partitions never.
  - `Adapters`: the present adapters the Network box counts (not `lo`, a class `show_network_*` shows), in sidebar order.
  - `Engines`: the engines the GPU reports, in the GPU page's grid order (`grid_engines`). `performance.hidden_gpu_engines` leaves engines out only in a flow layout, as for single widgets.
  - The order is fixed in phase 2; the user cannot reorder members.
- **Limit**: the group shows `shown = min(members, limit)` members, then, when `hidden = members − shown` is above 0, a "+{hidden} more" item (`perf-overview-more`). Without a limit every member shows. Members past the limit have no `SourceData`. In a grid layout only the limit hides members; members that do not fit scroll. In a flow layout the 1.2.0 fit rules also cut members (UI-PRF-059), and those count in the same "+n more" item.
- **Forms**: a group offers the forms its member metric offers, and draws them for every member. The Name form is each member's name. A group has no heading; the box title names the box. A group of a multi-series metric (`io`, `traffic`) draws each member's lines in that member's item.
- **Fill** (`widget/group_fill.rs`, pure functions with `proptest` tests, like `grid.rs`). The group lays its items out inside its own rectangle:
  - `Rows`: each item is a Row composition (UI-PRF-061). The column count is the fewest columns, at most 4, in which every item fits at 18 px with each column at least the Row minimum width of the forms; items flow top to bottom, then into the next column (C0 to C{k − 1} in the first). Then every row takes the same share of the height, at least 18 px; the bar and the sparkline grow and the text stays centred (D-040). When the items do not fit at 18 px in the columns that fit the width, rows stay 18 px and the widget scrolls vertically with a scrollbar. The "+n more" item is the last row.
  - `Cells`: each item is a Cell composition (a Tile when the forms have no chart). `arrange = auto` picks the column count that gives the largest equal cells for the item count, every cell at least the composition's minimum (the `parts::fill_grid` rule of UI-PRF-019); `vertical` is one column and `horizontal` one row, every cell an equal share at least the minimum. When the cells do not fit at their minimum, the widget scrolls in the arrangement's direction (vertically for `auto` and `vertical`) with a scrollbar. The "+n more" item is a cell of its own with the text centred.
  - **Minimum span**: one item of its composition (Row: the Row minimum width × 18 px; Cells: the Cell or Tile minimum). More items scroll.
  - **Edit mode**: the group lays out its items in the widget's rectangle minus the UI-PRF-061 edit margins, with the column count and item kind of the applied view, so nothing jumps; a Row item drops parts by the narrow rule and a Cell draws its chart smaller. No edit decoration covers any item's text, and the group scrolls as in the applied view.
- **Hotplug**: members come and go with the device catalog, and the widget keeps its id, entity and cells. A member that goes away leaves the group; one that comes back takes its place in the order again. When the group's own device is gone (a GPU), the widget shows "Not connected". A group with no members shows "None found" (`perf-widget-group-none`) centred.
- **Entity** (`widget/group_widget.rs`): `GroupWidget`, one entity per group widget id. It takes one `SourceData` per shown member from the page's `SourceStore`, shared with single widgets and other groups on the same member source, and keeps one cached `ChartView` per member item with a chart. It formats a member's text only when that member's source notified, and notifies only when a shown member's value text, stats text or bar bucket changed, or the member list changed. A member list change (catalog version, visible cards, `show_network_*`) re-fits the group inside its own cells: the container does not re-render and keeps every entity. A sample notifies only the group widgets with a changed member, never the container or the page.
- **Extra member sources of a flow look.** Some 1.2.0 parts show a second value per member. Their `GroupFlowLook` takes a second `SourceData` per member from the same `SourceStore` and asks for its series too: the core rows read `freq` for the clock column; the disk sections and `disk_charts` cells read `active` for the header percent and the device details for the model. A change of either source counts as a member change. The busiest-first expansion of the disk sections (UI-PRF-017 "Disk fit") is worked out inside the group widget, which fills the Disk box body, so an activity change re-renders that group only. The Network `rx` and `tx` flow looks work out the 60 s peak from their own series and the total from the network record, as the box does today.

**Fixed widgets.**

- `FixedWidgetView` (`widget/fixed_widget.rs`) is one entity per fixed widget id. It hosts the content view that the box draws today, out of its box frame: the Top processes rows of `procs_box.rs`, the timeline of `timeline.rs`, and `MemoryAppsChart` of `memory_apps.rs` in its Overview place. Each keeps its own render contract, interaction and demand: `process_top` while the Processes widget is placed, `device_timeline` while the Timeline widget is placed, the process sweep while Memory by app is placed.
- Minimum size, logical px: Processes 256 × 192 and Timeline 376 × 92 (the box body at the kind minimum of UI-PRF-056), Memory by app 160 × 100 (the size from which the legend shows, UI-PRF-060).
- Each fixed kind is allowed in its own container only, at most once. Add Widget lists it, after a separator, while the container has none.
- The widget menu of a fixed widget has only "Remove Widget". In flow and grid layouts it draws its content at its rectangle; it has no compositions.

**Presets** (the flow layouts of phase 2). Each box's modes become presets as the GPU modes did. A preset's widget list replaces the box's fixed parts; each widget of a flow layout draws its 1.2.0 part (`FlowLook`; for groups `GroupFlowLook`), at its 1.2.0 place, with the 1.2.0 fit rules: the core rows of D-040 with their clock column, the `core_charts` cells, the disk sections and `disk_charts` cells with "+n more" (UI-PRF-059), the Download and Upload parts with their peak line and total tooltip, the adapter cells. The box draws only what is not a widget: the title line, the range figures row, the time axis, the splitters and the "+n more" rows the fit rules add. Widget ids start at 1 in list order. In the tables, "group" means a group widget on the box's device with the given members.

CPU box (`device = "cpu"`, flow placement `overview/cpu_flow.rs`):

| Preset | Widgets |
|---|---|
| `chart_cores` (default) | `total` [chart]; group cores `load` [chart, bar, value, label], `items = "rows"` |
| `chart` | `total` [chart] |
| `cores` | group cores `load` [chart, bar, value, label], `items = "rows"` |
| `core_charts` | group cores `load` [chart, value, label], `items = "cells"`, `arrange` from the preset |

After these, in every preset: `core_clocks` [chart, label] when the Core clocks chart is on, then `power_draw` [chart, value, label] when the power draw strip is on. `core_clocks` draws under the total chart in `chart_cores` and `chart` and takes no space in `cores` and `core_charts`, as today. `power_draw` is the strip, placed by `power_placement` with the share `power_split`; when RAPL is unreadable or has no zone it takes no space, as today. The box menu's "Show power draw" and "Show core clocks" add or remove these widgets. `PresetSetting` gains `power_split: Option<f64>` (in `OVERVIEW_POWER_SPLIT_RANGE`) and `power_placement: Option<String>` (in `OVERVIEW_POWER_PLACEMENTS`), CPU box only, written only when set. A drag of the strip's splitter saves `power_split` there.

Memory box (`device = "memory"`, `overview/mem_flow.rs`):

| Preset | Widgets |
|---|---|
| `rows` (default) | `used`, `cached`, `available`, `swap`, `commit`, each [chart, bar, value, label] |
| `chart_rows` | `used` [chart]; then `used`, `cached`, `available`, `swap`, `commit`, each [bar, value, label] |
| `chart` | `composition` [chart] |
| `charts` | `used`, `cached`, `available`, `swap`, `commit`, each [chart, value, label] |
| `apps` | fixed `memory_by_app` |

Disk box (`device = "aggregate:disk"`, `overview/disk_flow.rs`):

| Preset | Widgets |
|---|---|
| `disk_charts` (default) | group disks `io` [chart, value, label], `items = "cells"`, `limit = 8`, `arrange` from the preset |
| `rows` | group disks `io` [chart, value, label], `items = "rows"`, `limit = 8` |
| `total_chart` | `io` [chart] on `aggregate:disk` |

Network box (`device = "aggregate:net"`, `overview/net_flow.rs`):

| Preset | Widgets |
|---|---|
| `stacked` (default) | `rx` [chart, value, label]; `tx` [chart, value, label] |
| `columns` | `rx` [chart, value, label]; `tx` [chart, value, label] |
| `combined` | `traffic` [chart, value, label] |
| `adapter_charts` | group adapters `traffic` [chart, value, label], `items = "cells"`, `arrange` from the preset |

Top processes box: preset `list`, widgets: fixed `processes`. Timeline box: preset `bands`, widgets: fixed `timeline`. A flow layout whose only widget is fixed draws it over the whole body (`fixed_flow` in `widget/container.rs`).

Changing the preset in the box menu replaces the widget list with the new preset's list, as for the GPU box. The CPU box keeps its `core_clocks` and `power_draw` widgets across the change, so the two switches survive a mode change as they do today. With a grid layout the box-specific menu items ("Arrange", "Show power draw", "Show core clocks", "Engines") are hidden, as phase 1 hides "Arrange" and "Engines".

**Conversion of the other boxes** (`layout_boxes.rs`: `convert_cpu_box`, `convert_memory_box`, `convert_disk_box`, `convert_net_box`, each `fn(&OverviewGridSetting) -> ContainerLayout`, called through `convert_box(kind, entry)`). It runs in `Settings::clamp` for each supported box that has no entry and whose grid entry has one of its old keys. It gives a flow layout: the entry's `mode` (or the default) as `preset.id` with its `arrange` and `split`, and that preset's widget list from the tables above. The old keys per kind:

| Box | Old keys conversion reads and "Saving the default" removes |
|---|---|
| CPU | `mode`, `split`, `arrange`, `power`, `power_split`, `power_placement`, `clocks` (the old `cpu_split` already loads as `split`) |
| Memory, Disk, Network | `mode`, `split`, `arrange` |
| GPU | `mode`, `split`, `arrange`, `hidden` (phase 1) |
| Top processes, Timeline | none: they never convert; no entry gives the built-in default |

The CPU box adds `core_clocks` when `clocks = true` and `power_draw` when `power = true`, and copies `power_split` and `power_placement` into the preset. The built-in default of a box is its conversion of an entry with no keys, used in memory and never written. Conversion is deterministic (it reads no device: a group stands for whatever members are present), idempotent per container, and leaves the old keys in place, as phase 1. `gpu_box_layout` becomes `box_layout(perf, kind)`, and `reset_to_default` removes the old keys of the container's kind.

**Editing in every box.** "Edit Widgets…" is in every box's menu. Entering Edit Widgets pins a flow layout as phase 1 does. A group widget keeps its `items`, `limit` and `arrange`, and gets the cells nearest its drawn rectangle. Its selector is durable already, except for an engine group with a slot: the slot becomes the GPU's persistent key and `device_name` is set, as for a single widget. An engine group of an empty slot is left out. A fixed widget gets the cells nearest its rectangle. After pinning, each widget draws its UI-PRF-061 composition instead of its flow look, with the accent, series, overlays, legend, value format and group columns of its flow look ("One look in every layout" under "Flow placement"): a cores group of `load` in Rows keeps the core rows with their clock column, which reads each core's `freq` source. Parts that only the box's flow look draws do not carry over: a disk header's active percent and model, and the Network 60 s peak line and its total tooltip. The user can add disk `active` widgets instead. The disk model, the 60 s peak and the total since boot have no widget in phase 2; the `stats` form gives min, average and peak over the box range instead. Reset in a box with an old-key conversion removes that kind's old keys, as for the GPU box.

**Demand.** A container asks for the series of the sources it places, group members included, and the demand of its fixed widgets. The CPU box asks for `cpu_fast` while it places a cores group or a core widget, as it does today. A widget or member that takes no space asks for nothing.

**Files the phase 2 beads create or change.** Paths under `widget/` and `overview/` are in `crates/tl-ui/src/pages/performance/`.

| Bead | Files |
|---|---|
| `tasklens-6swdr.2` | `tl-model/src/layout.rs` (`WidgetContent`, `GroupSelector`, `GroupKind`, `GroupOptions`, `GroupItems`, `FixedWidget`, `LAYOUT_VERSION` 2 and the version write rule, `LAYOUT_CONTAINERS`, `OVERVIEW_GRID_COLS`, `grid_cols`, the phase 2 load rules, `PresetSetting` power fields, `box_layout`, conversion and `reset_to_default` for every box), `tl-model/src/layout_boxes.rs` (new: the phase 2 metric keys and their series, `convert_box` and the four `convert_*_box`, the preset widget lists), `tl-model/src/lib.rs` (mod line), `tl-app/src/settings_store.rs` (the backup name from `LAYOUT_VERSION`), `widget/meta.rs` (the phase 2 entries, `Lines`, `smoothed`, `DeviceFact` on the snapshot), `widget/source.rs` (per-kind readings and names, multi-series and derived sources with `series_list()`, the "Not reported by this device" text), the phase 1 users of `WidgetSetting.source` (`widget/container.rs`, `widget/menu.rs`, `widget/edit.rs`, `overview/gpu_flow.rs`) for the rename to `content`, `crates/tl-model/tests/fixtures/layouts/{cpu,memory,disk,net}_*.toml` |
| `tasklens-6swdr.3` (needs the types of `.2`) | `widget/members.rs` (new), `widget/group_fill.rs` (new, pure, `proptest`), `widget/group_widget.rs` (new: `GroupWidget`), `widget/container.rs` (a group gets a `GroupWidget` entity; the id map holds metric, group and fixed views), `widget/compose.rs` (group minimum span), `widget/menu.rs` (group items in Add Widget and the group menu items of UI-PRF-061), `crates/tl-ui/tests/ui/overview_groups.rs` (new module) |
| `tasklens-6swdr.4` | `overview/cpu_box.rs` (hosts a `WidgetContainer`, draws each flow widget's entity at its 1.2.0 place), `overview/cpu_flow.rs` (new), `overview/box_menu.rs` (CPU switches edit the flow layout; "Edit Widgets…" in every box), `overview/mod.rs` (wiring), `widget/metric_widget.rs` and `widget/group_widget.rs` (the CPU flow looks), `crates/tl-ui/tests/ui/overview_cpu_widgets.rs` |
| `tasklens-6swdr.5` | `overview/mem_box.rs`, `overview/mem_flow.rs` (new), `widget/fixed_widget.rs` (new: `FixedWidgetView`, first used here for Memory by app), `memory_apps.rs` (hosted by a fixed widget), `widget/metric_widget.rs` (the Memory flow looks), `crates/tl-ui/tests/ui/overview_mem_widgets.rs` |
| `tasklens-6swdr.6` | `overview/disk_box.rs`, `overview/disk_flow.rs` (new), `widget/group_widget.rs` (the disk flow looks), `crates/tl-ui/tests/ui/overview_disk_widgets.rs` |
| `tasklens-6swdr.7` | `overview/net_box.rs`, `overview/net_flow.rs` (new), `widget/metric_widget.rs` and `widget/group_widget.rs` (the Network flow looks), `crates/tl-ui/tests/ui/overview_net_widgets.rs` |
| `tasklens-6swdr.8` | `overview/procs_box.rs`, `overview/timeline.rs` (their content in a `FixedWidgetView`), `widget/container.rs` (`fixed_flow`), `crates/tl-ui/tests/ui/overview_fixed_widgets.rs` |
| `tasklens-6swdr.9` | `crates/tl-model/tests/fixtures/layouts/*.toml` (every old setting of every box), the hotplug and counter tests of the boxes' test modules, the `perf-overview` and `perf-overview-static` bench scenes |

Each tl-ui bead adds its UI-PRF-061 strings to `assets/locales/en-US.ftl`.

**Verification (phase 2).**

- tl-model: a fixture per old mode of each box, with `arrange` and `split` set and, for the CPU box, with `power`, `power_split`, `power_placement` and `clocks`, converts to the preset lists above; converting twice changes nothing; an entry with only phase 1 content in `overview.gpu` writes version 1 and any other entry version 2; each new drop rule drops only its widget; unknown `group` and `fixed` names are kept and written back; every phase 2 key has metadata; reset of each box removes its old keys and reloads as the built-in default.
- tl-app: the first save after a phase 2 conversion writes `settings.toml.pre-layouts-v2` once and leaves `-v1` alone.
- tl-ui (`test-hooks`): group fill at 1, 8, 32 and 128 members in rows and cells, with and without a limit, at 1.0, 1.5 and 2.0 scale; 32 cores fill the height as D-040; members added and removed keep the entity and cells; a sample notifies only the group widgets with a changed member; fixed widgets at their minimum; each box's presets place the same rectangles as 1.2.0 at 1200 × 900; Edit Widgets add, remove, move and Apply in each box; the editor rule of `tasklens-lhliq.12` for group and fixed widgets.
- Phase gate (`tasklens-6swdr.9`): migration fixtures for every old setting of every box; a visual check of each box's default and old modes against 1.2.0 screenshots in a hidden session; hotplug tests for disks, adapters and logical CPUs going offline; the counter test (a sample renders no page or container and keeps every widget entity); the `perf-overview` bench budgets within 10% of v1.2.0 under the same conditions.

### Phase 3: resource pages (epic E29, `tasklens-hzire`)

Phase 3 makes every device page one widget container: CPU, Memory, each GPU, each disk, each network adapter, each fan, each battery, Aggregate storage and Aggregate graphics. There is no separate Sensors page in 1.3.0: the fan and battery pages of `sensors.rs` are "the other device pages". Phase 3 reuses the phase 1 and phase 2 types, rules and files: `WidgetContainer`, `WidgetEditor`, `SourceStore`, `SourceData`, `MetricWidget`, `GroupWidget`, `FixedWidgetView`, `compose.rs`, `grid.rs`, `group_fill.rs`, `look.rs`, `meta.rs`, `menu.rs`, and the load and reset code of `tl-model/src/layout.rs`. Nothing is forked per page. Every rule above holds for a page container unless this section says otherwise. That includes the render contract, the editor rule of `tasklens-lhliq.12`, Apply, Cancel and Undo, the selector rule (container id in every widget selector, every selector through `tl_selector`) and "One look in every layout". The leader rules of 2026-11-01 hold too. Everything a source or view derives lives in one state struct that an update compares whole. A widget computes every value it shows from the current snapshot when it is created.

**Containers.** A page container's id is `page.` followed by the page's persistent device key (PERF-005): `page.cpu`, `page.memory`, `page.gpu:GPU-0a1b2c3d-…`, `page.gpu:pci:0000:01:00.0`, `page.disk:…`, `page.net:…`, `page.fan:…`, `page.bat:BAT0`, `page.aggregate:disk` and `page.aggregate:gpu`. The key is kept whole, so `page_key(container)` gets it back by removing `page.`. Each GPU, disk, adapter, fan and battery has its own layout; a new device shows its page's default.

- `tl-model/src/layout_pages.rs` keeps `PAGE_CONTAINER_PREFIX = "page."` and `PAGE_KINDS`, the device kinds whose pages this build supports. Each page bead adds its kind (`tasklens-hzire.4` adds GPU, `.5` CPU and so on). `is_layout_container(id)` is true for an id in `LAYOUT_CONTAINERS` and for `page.<key>` when the key's kind is in `PAGE_KINDS`. Every check that read `LAYOUT_CONTAINERS` directly calls it. An entry for a page kind this build does not support is `LayoutEntry::Kept`, so it survives a save.
- `layout_boxes::DeviceKind` gains `Fan` (key `fan:…`), `Battery` (`bat:…`) and `AggregateGpu` (`aggregate:gpu`). `fan:` keys stop being `Other`.
- The entry of a device that is gone is kept and written back unchanged. When the device comes back, its page shows the saved layout. Phase 3 never prunes entries.

**Schema version.** `LAYOUT_VERSION` stays 2. Page entries are written as version 2. A 1.3.0 build keeps every `page.*` entry as `LayoutEntry::Kept` (an unknown container), so it shows its pages as before and writes the entries back unchanged. Phase 3 changes nothing in the Overview containers.

**Inner grid of a page.** `grid_cols(container)` returns `PAGE_GRID_COLS` = 12 for every page container. The page body is the page width minus `page_pad` on both sides: at least 368 px (`PAGE_MIN_W` 400), about 900 to 1600 px on a desktop. Rules that differ from the boxes:

- **Gutter and row unit**: both 12 logical px, from `grid_gutter(container)` and `grid_unit(container)` in `widget/grid.rs`; the Overview containers keep 4 px and 24 px. The gutter is `sp4`, the gap between the 1.3.0 page parts, so pinning keeps the page's spacing. The row pitch is 24 px, so a part of height `h` pins to `(h + 12) / 24` rows, a half rounded up: the 300 px charts to 13 rows (300 px), the 240 px chart to 11 rows (252 px), the 150 px charts to 7 rows (156 px) and the 120 px chart to 6 rows (132 px). A Row composition (18 px) needs 2 rows.
- **No fill**: a page is a scrolling column (UI-PRF-020), so its rows never grow. A layout shorter than the body leaves space under it, as the 1.3.0 pages do; a taller one scrolls. D-037 and D-040 do not apply to page containers. Without this rule the rows would stretch the moment Edit Widgets pins a short page, and the parts would jump.
- **Scrolling**: the page header row stays at the top of the page and only the body under it scrolls, with one vertical scrollbar. In Edit Widgets the edit toolbar sits between the header and the body, so Apply and Cancel stay in view. The body is the container's scroll area, so edit-mode scrolling equals the applied view as phase 1 requires. This is the one look change of phase 3: in 1.3.0 the header scrolls away with the page. Scroll offset 0 looks the same as 1.3.0.
- **Default span** of a new widget: a widget with a chart 6 columns × 7 rows (the 150 px page chart); any other single-metric widget 12 × 2; a group or fixed widget 12 × 16. Each is then raised to its minimum span.

**Page style.** A widget in a page container draws in the page style of Part C; a widget in an Overview box draws in the box style of phases 1 and 2. The style follows the container, never the placement, so "One look in every layout" holds: a page widget looks the same in its page's flow layout, in Edit Widgets and in a grid layout.

- A Cell in a page container draws its chart as the 1.3.0 page charts do (UI-PRF-001): the Name form is the chart heading at the top left, the Value form is the value at the right of the heading, then the legend, and the range text under the plot ("Utilization over 60 seconds"). The Cell header line of the box style is left out. Stats and bar sit between the heading row and the plot. Minimum height 30 + 20 (heading row) + 18 (range text) + 16 for stats + 14 for the bar; minimum width 120.
- Tile and Row are the same in both styles.
- Value text in a page container uses the page format of Part C for the keys in the table below, in every composition. Other keys use their phase 1 or phase 2 value text.

| Key | Value text in a page container |
|---|---|
| `total`, `kernel`, `active`, `engine:*`, `util` | one decimal, "18.6%" (UI-PRF-024, UI-PRF-029, UI-PRF-033) |
| `swap` | bytes used, "1.20 GiB" (UI-PRF-026) |
| `io` | read + write, "12.3 MiB/s" (UI-PRF-029); write is the dashed line |
| `traffic` | download + upload, "1.5 Mbps" (UI-PRF-031); upload is the dashed, tinted line |
| `power` (GPU) | "31.9 / 300 W" (UI-PRF-032) |
| `freq` (CPU) | "4631 MHz" (UI-PRF-025) |

- The heading tooltips of the 1.3.0 page charts stay on the widget's name in both styles: Commit ("Memory the system has promised to programs…"), GPU Power (limit range and energy), GPU Clocks (P-state and throttle reasons), the AMD "Own processes only" engine note, and the CPU fallback note below.

**Metric keys added in phase 3** (`tl-model/src/layout_pages.rs`, looked up with the device kind as in phase 2). The Name strings are in UI-PRF-061 "Phase 3". "Menu label" is the text of the Source and Add Widget items when it differs from the name, so two keys never share an item text.

| Device | Key | Series | Name | Menu label |
|---|---|---|---|---|
| `cpu` | `cores_overlaid` | `CPU_TOTAL` of every present logical CPU, one line each at alpha max(1/n, 0.08), no fill, no kernel line (UI-PRF-023) | All cores overlaid | |
| `cpu` | `cores_stacked` | the same, as stacked filled bands, `Stacked(n)` scale | All cores stacked | |
| `cpu` | `clock` | `CPU_FREQ` filled and `CPU_CLOCK_MAX` dashed (the UI-PRF-025 Clock speed chart) | Clock speed | |
| `cpu` | `freq_max` | `CPU_CLOCK_MAX` | Fastest core | |
| `memory` | `physical` | `MEM_USED` filled, `MEM_COMMIT` dashed, `MEM_DIRTY` as a 1 px line at 0.6 (UI-PRF-026) | Physical memory | |
| `memory` | `commit_pct` | `MEM_COMMIT` on `YScale::Commit(CommitLimit)` | Commit | Commit (% of limit) |
| `memory` | `dirty` | `MEM_DIRTY` | Modified | |
| `gpu:…` | `memory` | `GPU_MEM` filled and `GPU_MEM_SHARED` dashed (absent on NVIDIA) | Memory | Memory (dedicated and shared) |
| `gpu:…` | `vram_shared` | `GPU_MEM_SHARED` | Shared memory | |
| `gpu:…` | `clocks` | `GPU_CLOCK` filled and `GPU_MEM_CLOCK` dashed (the UI-PRF-032 Clocks chart) | Clocks | |
| `gpu:…` | `fan` | no series: `GpuTotals.fan_pct` and `fan_rpm` | Fan speed | |
| `disk:…` | `response` | no series: the disk record's `response_ms` | Average response time | |
| `disk:…` | `total_read`, `total_written` | no series: the disk record's running totals | Total read, Total written | |
| `net:…` | `total_rx`, `total_tx` | no series: the adapter record's running totals | Total downloaded, Total uploaded | |
| `net:…` | `signal` | no series: the Wi-Fi signal (wired adapters do not report it) | Signal | |
| `fan:…` | `speed` | `FAN_RPM` filled and `FAN_PWM` dashed on its own 0..100 axis (UI-PRF-034) | Fan speed | |
| `fan:…` | `rpm` | `FAN_RPM` | RPM | |
| `fan:…` | `pwm` | `FAN_PWM` | PWM | |
| `fan:…` | `temp` | `SENSOR_TEMP` of the paired sensor (`[fans]` `temp`) | Temperature | |
| `bat:…` | `power` | `BATTERY_POWER` | Power | |
| `bat:…` | `percent` | `BATTERY_PCT` | Percentage | |
| `aggregate:gpu` | `util` | `GPU_UTIL` of the aggregate (the busiest adapter) | Utilization | |
| `aggregate:gpu` | `memory` | `GPU_MEM` of the aggregate (the sum), one line | Memory | |

The phase 2 keys `active`, `read`, `write` and `io` on `aggregate:disk`, and every phase 1 and phase 2 key, also serve the pages. Details rows that have no series and no key here (Processes, Threads, Handles, Up time, Load average, addresses and the like) are shown only by the Details widget.

**Metric metadata added in phase 3** (`widget/meta.rs`). `Lines` gains `PerLogical { stacked: bool }` (`cores_overlaid`, `cores_stacked`), `Physical` (the three lines of `physical`) and `RpmPwm` (`speed`). `MetricMeta` gains `menu_label: Option<&'static str>` (an `en-US.ftl` key) and `page_value: Option<ValueText>` (the page format of the table above).

| Key | Format | Scale | Kind | Bar max | Stats | Value text |
|---|---|---|---|---|---|---|
| `cores_overlaid`, `cores_stacked` | `Percent` | `Fixed { 0, 100 }`; stacked `Stacked(n)` | gauge | no bar | none | none (offers chart and name) |
| `clock`, `freq_max` | `Mhz` | `StickyUp { hw_max: cpuinfo_max_freq }` | gauge | `freq_max`: the inventory's `max_mhz`; `clock`: no bar | `freq_max` min, avg, peak; `clock` none | "4631 MHz" |
| `physical` | `Bytes` | `FixedTotal(MemTotal)` | gauge | no bar | none | used, "34.68 GiB" |
| `commit_pct` | `PercentOf { CommitLimit }` | `Commit(CommitLimit)`, axis text above 100% | gauge | 100% of the limit (the bar stops at full) | min, avg, peak | "69.0%" |
| `dirty` | `Bytes` | `StickyUp { hw_max: none }` | gauge | no bar | min, avg, peak | "120.00 MiB" |
| `memory` (GPU) | `Bytes` | `FixedTotal(mem_total)` | gauge | no bar | none | "2.80 GiB / 15.92 GiB" |
| `vram_shared` | `Bytes` | `FixedTotal(shared_total)` | gauge | `shared_total` | min, avg, peak | `OfTotal` |
| `clocks` | `Mhz` | `StickyUp { hw_max: clock_gfx_max_mhz }` | gauge | no bar | none | as the 1.3.0 Clocks chart, "1905 / 3105 MHz" |
| `fan` (GPU) | `Percent` | none | gauge | 100 | none | "0% (0 RPM)" (offers bar, value and name) |
| `response` | text as the Details row | none | gauge | no bar | none | "0.42 ms" (offers value and name) |
| `total_read`, `total_written`, `total_rx`, `total_tx` | `Bytes` | none | counter | no bar | none | the running total, "812.40 GiB" (offers value and name) |
| `signal` | text as the Details row | none | gauge | 100 (the percent) | none | "-52 dBm (78%)" (offers bar, value and name) |
| `speed` | `Rpm` | `StickyUp { hw_max: fanN_max }`, else sticky 1-2-5 | gauge | `fanN_max`, else no bar | none | "1200 RPM" |
| `rpm` | `Rpm` | as `speed` | gauge | `fanN_max`, else no bar | min, avg, peak | "1200 RPM" |
| `pwm` | `Percent` | `Fixed { 0, 100 }` | gauge | 100 | min, avg, peak | "45%" |
| `temp` (fan) | `Celsius` | `StickyTemp` | gauge | no bar | min, avg, peak | "45 °C"; with no paired sensor "-" and the plot shows "Choose a sensor" centred (UI-PRF-034) |
| `power` (battery) | `Watts` | `Symmetric` (charge above the zero line, discharge below) | gauge | no bar | min, avg, peak | "-12.4 W" |
| `percent` | `Percent` | `Fixed { 0, 100 }` | gauge | 100 | min, avg, peak | "85.0%" |
| `util` | `Percent` | `Fixed { 0, 100 }` | gauge | 100 | min, avg, peak | "10.9%" |
| `memory` (aggregate GPU) | `Bytes` | `FixedTotal(sum of mem_total)` | gauge | the sum | min, avg, peak | `OfTotal` |

**Fixed widgets added in phase 3.** Each page part that is not a single metric becomes a fixed widget kind. `FixedWidget` gains the kinds below. Each is allowed only in the page containers named, at most once, like the phase 2 kinds; `memory_by_app` is now allowed in `page.memory` as well as `overview.memory`. A fixed widget's device is its page's device. Its view keeps the 1.3.0 part's own render contract, interaction, tooltips, menus and demand, and is hosted by `FixedWidgetView` as in phase 2.

| Written as | Page | The 1.3.0 part | Minimum, logical px |
|---|---|---|---|
| `details` | every page | the "Details" heading and the page's Details grid (UI-PRF-021), with its link values, prefixes and suffixes (Needs setup, Display unit, Public IP Fetch, Wi-Fi bars) | 360 × 74 (heading and one grid row) |
| `cpu_core_grid` | `page.cpu` | the per-core `ChartGridView` with kernel lines, core marks and heading tooltips (UI-PRF-023) | 120 × 112 |
| `cpu_core_board` | `page.cpu` | the core board with its heading and sort toggle (`CpuBoard`, crw.104) | 200 × 60 |
| `gpu_engine_grid` | `page.gpu:…` | the engine `ChartGridView` with "Hide this engine graph" (UI-PRF-032) | 120 × 112 |
| `memory_bar` | `page.memory` | the composition bar with its caption (UI-PRF-027) | 160 × 44 |
| `memory_compressed` | `page.memory` | the "Compressed memory" block (UI-PRF-028) | 360 × 74 |
| `swap_devices` | `page.memory` | the "Swap devices" table | 360 × 74 |
| `memory_modules` | `page.memory` | the "Memory modules" block | 360 × 74 |
| `gpu_displays` | `page.gpu:…` | the "Displays" block (UI-PRF-033) | 360 × 48 |
| `disk_partitions` | `page.disk:…` | the partitions heading and table (UI-PRF-030) | 360 × 74 |
| `net_settings` | `page.net:…` | the "Network settings" link (UI-PRF-031) | 160 × 24 |
| `battery_history` | `page.bat:…` | the 24-hour charge chart with its day labels (UI-PRF-035) | 200 × 80 |

- The core grid and the engine grid stay fixed widgets because each is one element with three paths for all its cells (ADR-009 rule 7, the `perf-cpu` scene of ADR-014). A cores group or an engines group would make one `ChartView` per member. In a grid layout each grid lays out its columns by width as today, and its rows share the widget's height equally, at least `grid_cell_h` 112 px each; a taller grid scrolls inside the widget. The engine grid follows `performance.hidden_gpu_engines` in every layout, because "Hide this engine graph" is part of it.
- The core board stays a fixed widget because of its heading and its sort toggle (`performance.cpu_board_sort`). A user who wants the core rows without them adds a cores group, which draws the same rows.
- A block that 1.3.0 hides (no zram and zswap off, no swap device, no module data, a desktop without a network settings tool, a battery with fewer than 2 history points) takes no space in a flow layout. In a grid layout it keeps its cells and shows its heading and the centred `text_secondary` line "Nothing to show on this device" (`perf-widget-nothing-to-show`). Add Widget does not list a block the page cannot show.
- A block taller than its widget scrolls inside the widget. A table keeps its own header row and scrollbar.
- **Details and the render contract.** In 1.3.0 a page re-renders when a Details value changes (1 Hz). In phase 3 the Details grid is its own entity, `DetailsView` (`widget/details_widget.rs`). The page keeps building the `KeyValueModel` on a snapshot, as today, and hands it to `DetailsView`, which compares the whole model and notifies only when a row's text, validity or tooltip changed. The page itself does not notify. The page blocks follow the same rule with their own views. Copy device info (UI-PRF-022) still reads the page's Details model and blocks, whatever the layout holds.

**Group widgets on pages.** The CPU page offers "All cores >" (`load`, `freq`) and each GPU page "All engines" for its own GPU, as the Overview boxes do. The other pages show one device and offer no group.

**Presets of the pages** (the flow layouts of phase 3). Each widget of a page flow layout draws its 1.3.0 page part at its 1.3.0 place and height, in the page style. The flow placement (`pages/performance/page_flow/`) draws only what is not a widget: the gaps (`sp4` between parts, `details_gap` 24 above Details, `sp6` above each block), the Memory page's Usage and By app switch, and the CPU fallback note. All device keys are the page's own key; ids start at 1 in list order. "Details" means fixed `details`; every list ends with it and then the page's blocks, in the order of the 1.3.0 page.

| Page | Preset (from) | Widgets |
|---|---|---|
| CPU | `overall` (`cpu_graph_mode`) | `total` [chart, value, label], 300 px |
| CPU | `per_core` (default) | fixed `cpu_core_grid` |
| CPU | `all_cores_overlaid` | `cores_overlaid` [chart, label], 300 px |
| CPU | `all_cores_stacked` | `cores_stacked` [chart, label], 300 px |
| CPU | `board` | `total` [chart, value, label] at `BOARD_CHART_SHARE` of the width, beside fixed `cpu_core_board`, 300 px |
| Memory | `usage` (default, `memory_view`) | `physical` [chart, value, label]; `commit_pct` [chart, value, label]; `swap` [chart, value, label]; fixed `memory_bar`; Details; `memory_compressed`, `swap_devices`, `memory_modules` |
| Memory | `apps` | fixed `memory_by_app` at 300 px in place of `physical`; the rest as `usage` |
| GPU | `default` | fixed `gpu_engine_grid`; `memory` and `temp` [chart, value, label] side by side; `power` and `clocks` [chart, value, label] side by side; Details; `gpu_displays` |
| Disk | `default` | `active` [chart, value, label]; `io` [chart, value, label]; Details; `disk_partitions` |
| Network | `default` | `traffic` [chart, value, label], 240 px; Details; `net_settings` |
| Fan | `default` | `speed` [chart, value, label]; `temp` [chart, value, label]; Details |
| Battery | `default` | `power` [chart, value, label]; `percent` [chart, value, label]; fixed `battery_history`; Details |
| Aggregate storage | `default` | `active` [chart, value, label]; `io` [chart, value, label]; Details |
| Aggregate graphics | `default` | `util` [chart, value, label]; `memory` [chart, value, label]; Details |

Every chart is 150 px high unless the table says otherwise. Flow rules:

- **CPU secondary graph.** After the mode's widgets, a CPU flow layout lists the widgets of its secondary graph, 120 px. Conversion picks them from `performance.cpu_secondary_graph`; the built-in default is `power`. `power` gives `power_draw` [chart, value, label] and then the two fallbacks `temp` and `clock` [chart, value, label]; `clock` gives `clock`; `clocks` gives `core_clocks` [chart, label]; `temperature` gives `temp`; `none` gives nothing. Of these widgets only the first that `effective_secondary` allows takes space. When a fallback shows, its heading carries the 1.3.0 tooltip "Power draw is not available. Showing Temperature." (or Clock speed).
- **Memory.** A Memory flow layout lists `swap` only when conversion found `memory_swap_graph` on, and `memory_bar` only when it found `memory_composition` on (both on in the built-in default). A listed `swap` takes no space while SwapTotal is 0.
- **GPU.** A GPU flow layout lists `power` and `clocks` only when conversion found them in `gpu_optional_graphs` (neither in the built-in default); one alone takes the full width. With every engine hidden the engine grid takes no space.
- **Battery.** `power` takes space on a system battery only, `percent` on a peripheral only (UI-PRF-035).
- A widget whose metric the device does not report, and a block 1.3.0 hides, take no space (as phase 1).
- 1.3.0 animates some flow changes: the CPU mode cross-fade, the swap chart height and the engine row collapse. Phase 3 drops these three; the change shows at once, as 1.3.0 already does with reduced motion.

**Conversion of the page settings** (`layout_pages.rs`: `convert_cpu_page`, `convert_memory_page` and `convert_gpu_page`, each `fn(&PerformanceSettings, container: &str) -> ContainerLayout`, called through `convert_page(perf, container)`). The 1.3.0 page keys are the old keys of the page kinds, as `mode`, `split` and `arrange` are for the boxes (layout-system.md section 5 and implementation rule 7). Conversion reads them once per page and writes the result as the page's entry: a flow layout with the preset of the table above and its widget list, as the flow rules above give it.

| Page | Old keys conversion reads and "Saving the default" sets back to their defaults |
|---|---|
| CPU | `cpu_graph_mode` (the preset), `cpu_secondary_graph` (the secondary widgets) |
| Memory | `memory_view` (the preset), `memory_swap_graph` (`swap`), `memory_composition` (`memory_bar`) |
| GPU | `gpu_optional_graphs` (`power`, `clocks`) |
| Disk, Network, Fan, Battery, Aggregate storage, Aggregate graphics | none: they never convert; no entry gives the built-in default |

- **When.** `page.cpu` and `page.memory` convert in `Settings::clamp`, as the boxes do: when the kind is in `PAGE_KINDS`, the page has no entry and one of its old keys differs from its default. A GPU page's container id holds the GPU's key, which `Settings::clamp` does not know (`device_order` is empty by default). So a GPU page converts in tl-ui the first time the window builds that page (`DevicePage` for its key): when GPU is in `PAGE_KINDS`, the page has no entry and `gpu_optional_graphs` is not empty, the page writes `convert_gpu_page` as its entry through `TlSettings::update`. Each GPU converts on its own; a GPU first seen after the upgrade converts when its page is first built.
- **Deterministic and idempotent.** Conversion reads the old keys and the container id, never a device: a group and a fixed widget stand for whatever the device has, and the device rules above decide what takes space. Once a page has an entry, conversion does not run for it again, so converting twice changes nothing.
- **The built-in default** of a page is its conversion with every old key at its default. It is used in memory and never written. With every old key at its default nothing converts and the file does not change.
- **Old keys stay.** Conversion leaves the page keys as loaded, and a page with an entry ignores them. So 1.3.0, which keeps every `page.*` entry as `LayoutEntry::Kept`, still shows its pages by these keys. The keys that are not about layout stay live in every layout: `cpu_kernel`, `cpu_board_sort`, `hidden_gpu_engines` (the engine grid's own "Hide this engine graph"), `memory_by_app.*` and `network_dynamic_scale`. The `settings_global.rs` keys of the old page keys still set them; they change only pages without an entry.
- **Report and backup.** `Settings::clamp` reports a page conversion with `LAYOUTS_CONVERTED_KEY`, so the first save copies `settings.toml` to `settings.toml.pre-layouts-v2` as for the boxes. A GPU page conversion first calls `AppEnv.keep_pre_layouts_copy` (new; tl-app wires it to `Store::keep_pre_layouts_copy`), so the same copy is made before that save when it does not exist yet.
- `container_layout(perf, container)` (replacing `box_layout(perf, kind)` for every container) gives a container's entry when it has one, else its built-in default.
- **Saving the default** of a page (`reset_to_default`) removes the page's entry and, in the same save, sets the old keys of its kind back to their defaults, as phase 2 removes a box's old keys. Without this the keys would convert again at the next load (CPU, Memory) or the next time the page is built (GPU) and bring back what the user reset. A GPU page is the exception, because `gpu_optional_graphs` is shared by every GPU page: its reset writes the built-in default as the page's entry (a flow layout of the `default` preset with no `power` and `clocks`) and leaves `gpu_optional_graphs` as it is. The entry stops that page converting again, and the other GPU pages, converted or not, keep what the key gives them. Rule for every page kind: a reset never changes an old key that another container still reads; it writes an explicit default entry instead.

**Editing a page.**

- **Entering.** The page menu (UI-PRF-015) gets "Edit Widgets…" (`PerfWidgetEditStart`, dispatched on the page) and "Reset Page" (`PerfWidgetResetPage`) as the first page-specific items, followed by a separator. "Reset Page" is disabled while the page has no entry. Entering pins a flow layout as phase 1 does: each widget that takes space gets the cells nearest its drawn rectangle (with the page gutter), and widgets that take no space are left out, the secondary fallbacks not shown included.
- **Select Pages and Edit Layout.** "Select Pages" (`PerfEnterEditMode`, UI-PRF-013) edits which pages the sidebar shows and their order. Edit Widgets edits the widgets inside one page or box. They are separate modes, and at most one of Select Pages, Edit Layout and Edit Widgets is on in a window, as phase 1 rules for Edit Layout. Starting Edit Widgets ends Select Pages as "Done" does. While any container of the window is in Edit Widgets, "Select Pages" (the sidebar pencil and the card menu item), "Edit Layout", "Graph Summary View" and "Edit Widgets…" of every other container are disabled.
- **Leaving the page.** Selecting another page, tab or host tab keeps the edit open, as an Overview box does; the page shows it again when the user comes back. When the page's device goes away, the edit ends as Cancel does.
- **Header buttons and page menu outside Edit Widgets.** With a flow layout, every 1.3.0 item stays and edits the page's flow layout, as the phase 2 box menus do; a page without an entry gets one, starting from its built-in default. The page keys are not written. The CPU mode button, the CPU "Graph >" items and the Memory Usage and By app switch set the preset and replace the mode's widgets with the new preset's. The CPU secondary widgets and the Memory `swap`, `memory_bar` and blocks stay, as the CPU box keeps `core_clocks` and `power_draw`. "Secondary graph >" replaces the secondary widgets. "Show swap usage" and "Show memory composition" add or remove `swap` and `memory_bar`, and the GPU "Power" and "Clocks" items add or remove `power` and `clocks`. Each item shows its check from the flow layout. With a grid layout, the items that pick a preset stay and ask "Replace your widgets?" first; on Replace the page's entry becomes a flow layout of the chosen preset, with the widgets the built-in default lists for it (CPU: the `power` secondary graph), so the page shows that preset. These are the CPU mode button and "Graph >" items (`cpu_graph_mode`) and the Memory Usage and By app switch (`memory_view`). The items that add or remove flow widgets are hidden: CPU "Secondary graph >", Memory "Show memory composition" and "Show swap usage", and the "Power" and "Clocks" items of GPU "Select GPU engine graphs >" (its engine items stay while the page has the engine grid widget). Items that are not about layout stay as they are: Kernel, SMART, Eject, Network settings, Rename fan, Temperature sensor and "Groups shown" (on the By app widget's own menu in a grid layout).
- **Reset.** In Edit Widgets, Reset asks first, then sets the working copy to `None`, as in phase 1: the page shows its built-in default, Undo can bring the old working copy back, and only Apply saves the default as above. Cancel leaves the saved entry as it was. "Reset Page" outside Edit Widgets asks as "Reset Box" does with a grid layout, then saves the default at once.
- **Double-click** on a chart widget of a page outside Edit Widgets opens the mini window (`PerfShowMiniWindow`), as the 1.3.0 graph area does.
- **Source lists.** "Source >" and Add Widget list the metrics of the page's own device in the order of the key tables (phase 1, phase 2, then phase 3 for that device kind), by menu label. The CPU page also lists "Core load >" and "Core speed >". Then the groups the page offers, then a separator and the page's fixed widgets it does not hold yet, Details first. A widget may still name another device's key (a hand edit, or copy layout in phase 4); it is kept and shows its unavailable state when that device is missing. The Overview box menus do not change in phase 3.

**Render contract and demand.** A page renders on a header change (`DEVICES`), a header button change, a layout change, an edit mode change and a resize, never because of a sample. Its body is one `WidgetContainer` cached view. The gate is the phase 1 gate with "page" in place of "Overview page": on a sample the page and its container never notify, no widget entity is created or dropped, and only widgets on a changed source (and a `DetailsView` whose rows changed) notify. The page asks for what its placed widgets and fixed widgets ask for, plus its 1.3.0 page bits (`cpu`, `memory`, `gpus`, `disks`, `nets`, `sensors`, `cpu_fast` while the core grid or board shows). With the default flow layout its demand equals 1.3.0's. A page that is not shown asks for nothing, as today. Each page has its own `SourceStore`.

**Files the phase 3 beads create or change.** Paths under `widget/` and `page_flow/` are in `crates/tl-ui/src/pages/performance/`.

| Bead | Files |
|---|---|
| `tasklens-hzire.2` | `tl-model/src/layout_pages.rs` (new: `PAGE_CONTAINER_PREFIX`, `PAGE_KINDS`, `PAGE_GRID_COLS`, `page_container`, `page_key`, the phase 3 metric keys and their series, the page preset lists, `convert_page` and the three `convert_*_page`), `tl-model/src/settings.rs` (CPU and Memory page conversion in `Settings::clamp`), `tl-model/src/layout.rs` (`FixedWidget` kinds and where each is allowed, `is_layout_container`, `grid_cols` for pages, `container_layout`, `reset_to_default` of a page with its old keys), `tl-model/src/layout_boxes.rs` (`DeviceKind::Fan`, `Battery`, `AggregateGpu`), `tl-model/src/lib.rs` (mod line), `widget/meta.rs` (the phase 3 entries, `Lines::PerLogical`, `Physical`, `RpmPwm`, `menu_label`, `page_value`), `widget/source.rs` (readings and names of the phase 3 keys), `crates/tl-model/tests/fixtures/layouts/page_*.toml` |
| `tasklens-hzire.3` | `page.rs` (fixed header, the edit toolbar row, the body as a `WidgetContainer`; `DevicePage` gains the container and the fixed views), `page_flow/mod.rs` (new: the column flow, gaps and side-by-side rows), `widget/details_widget.rs` (new: `DetailsView`), `widget/fixed_widget.rs` (page fixed kinds, "Nothing to show on this device"), `widget/container.rs` and `widget/grid.rs` (`grid_gutter`, no fill for pages), `widget/look.rs` and `widget/compose.rs` (page style Cell), `widget/edit.rs` (page containers, mode exclusion with Select Pages), `widget/menu.rs` (page source lists, page fixed items), `menu.rs` (Edit Widgets… and Reset Page), `sidebar.rs` (Select Pages disabled while editing), `actions.rs` (`PerfWidgetResetPage`), one page wired end to end (the Aggregate storage page: two charts and Details), `crates/tl-ui/tests/ui/page_widgets.rs` (new module) |
| `tasklens-hzire.4` | `gpu.rs` (GPU page conversion on first build), `tl-ui/src/app.rs` (`AppEnv.keep_pre_layouts_copy`), `tl-app/src/main.rs` (wires it to `Store::keep_pre_layouts_copy`), `page_flow/gpu.rs` (new), the `gpu_engine_grid` and `gpu_displays` views, `crates/tl-ui/tests/ui/page_gpu_widgets.rs` |
| `tasklens-hzire.5` | `cpu.rs`, `cpu_grid.rs`, `cpu_board.rs`, `page_flow/cpu.rs` (new), `crates/tl-ui/tests/ui/page_cpu_widgets.rs` |
| `tasklens-hzire.6` | `memory.rs`, `memory_bar.rs`, `memory_apps.rs` (hosted on the page), `page_flow/memory.rs` (new), the block views, `crates/tl-ui/tests/ui/page_mem_widgets.rs` |
| `tasklens-hzire.7` | `disk.rs`, `page_flow/disk.rs` (new), the `disk_partitions` view, `crates/tl-ui/tests/ui/page_disk_widgets.rs` |
| `tasklens-hzire.8` | `net.rs`, `page_flow/net.rs` (new), the `net_settings` view, `crates/tl-ui/tests/ui/page_net_widgets.rs` |
| `tasklens-hzire.9` | `sensors.rs` (Fan and Battery), `aggregate.rs` (Aggregate graphics; Aggregate storage is `.3`), `page_flow/sensors.rs` and `page_flow/aggregate.rs` (new), the `battery_history` view, `crates/tl-ui/tests/ui/page_other_widgets.rs` |
| `tasklens-hzire.10` | the page fixtures of every toggle combination, the hotplug and counter tests of the page modules, the look sweep over every page, the `perf-cpu` and Performance page bench scenes |

Each tl-ui bead adds its UI-PRF-061 strings to `assets/locales/en-US.ftl`.

**Verification (phase 3).**

- tl-model: `convert_page` for every value of `cpu_graph_mode` × `cpu_secondary_graph`, both `memory_view` values with `memory_swap_graph` and `memory_composition` on and off, and every subset of `gpu_optional_graphs` gives the preset lists above; converting twice gives the same layout; a load with a CPU or Memory old key off its default writes that page's entry, reports `LAYOUTS_CONVERTED_KEY` and writes the page keys back unchanged; a load with every old key at its default writes no page entry and reports nothing; a page with an entry does not convert again; an entry for a page kind not in `PAGE_KINDS` and an entry of a device that is gone are written back unchanged; a page fixed kind outside its page is dropped; every phase 3 key has metadata; reset of a CPU or Memory page removes its entry and sets its kind's old keys to their defaults, and a save and a load then convert nothing; with `gpu_optional_graphs = ["power"]` and two GPU pages, a reset of one writes its default entry, leaves the key as it was, and the other GPU still converts to a layout with `power`.
- tl-ui (`test-hooks`): each page's default places the same rectangles as 1.3.0 at 1200 × 900; Edit Widgets add, move, resize, remove, Undo, Apply and Cancel on a page; the Details widget shows the same rows as the 1.3.0 grid; a sample renders no page or container (the counter gate) and a Details change notifies only `DetailsView`; a second GPU and a second disk keep separate layouts; a device gone and back shows its saved layout; the page body scrolls and the header stays; no fill on a short page; the page style Cell at its minimum at 1.0, 1.5 and 2.0 scale; edit decorations cover no text on a page; Select Pages and Edit Widgets exclude each other; a preset item with a grid layout asks before it replaces; a GPU page with `gpu_optional_graphs = ["power"]` writes its entry once when first built and arms the pre-layouts copy, and a second GPU converts on its own; a CPU header button on a page without an entry writes the entry and leaves `cpu_graph_mode` as it was.
- Phase gate (`tasklens-hzire.10`): the fixtures above, a visual check of every page's default and each old toggle against 1.3.0 screenshots in a hidden session, hotplug tests for GPUs, disks and adapters, the counter test, the look sweep (each page's widgets look the same in flow, in Edit Widgets and after Apply), and the `perf-cpu` and Performance page bench budgets within 10% of v1.3.0 under the same conditions.

### Consequences

- The GPU box looks as in 1.1.0 until the user edits it. Old settings stay in the file, so going back to an older build keeps the old mode. After a reset of the GPU box both builds show their default. That build drops the `[[performance.layouts]]` entries at its next save. `settings.toml.pre-layouts-v1` holds the file as it was before the first conversion.
- The GPU box code splits into the flow placement (`gpu_flow.rs`) and the shared widget code. The other boxes do not change in phase 1.
- A layout written by a later build for a box this build does not support survives a save unchanged.
- Each widget is an entity, so a box with 30 widgets has 30 entities. Widgets that take no space have none.
- Phase 3: each device page looks as in 1.3.0 until the user edits it, except that the page header stays at the top while the body scrolls, and three flow animations go. A page whose old keys are off their defaults converts once: CPU and Memory at load, each GPU page when it is first built. The first save after a conversion keeps `settings.toml.pre-layouts-v2`. Conversion leaves the page keys in place, so 1.3.0 still shows its pages by them. Page menu changes made after the upgrade edit only the page's entry, so 1.3.0 does not see them. Each GPU, disk, adapter, fan and battery has its own page layout, kept after the device is gone.

### Rejected alternatives

- **Store `SeriesId` or `DeviceIdx` in settings.** Rejected: both are valid only within one engine session (ADR-003, ADR-017).
- **Convert old modes into grid cells on load.** Rejected: the engines a GPU reports and its vendor order are not known at load, so the result would depend on the machine and would not be deterministic. Flow layouts keep the old placement rules instead.
- **One `schema_version` bump of `settings.toml` with a migration in `tl-app/src/migrations.rs`.** Rejected: an older build would open the whole file read-only (ADR-011 load step 3), and the conversion must run per container and only for containers this build supports.
- **One render of the container per sample, drawing every widget.** Rejected: it breaks the render contract of UI-PRF-017.
- **Build page flow layouts from the page keys in memory every time, never converting them (phase 3).** Rejected: layout-system.md section 5 and implementation rule 7 ask for a one-time conversion per container. It would also leave two sources for one page: the page menus would edit keys shared by every page of a kind while Edit Widgets edits one page.
- **Convert GPU pages at load, for the GPU keys the settings file names.** Rejected: `Settings::clamp` knows no GPUs and `device_order` is empty by default, so most GPUs would miss their conversion. A GPU page converts when it is first built instead, with its own key.
- **One converted entry per page kind (`page.gpu`) used by every GPU page without its own entry.** Rejected: it adds a second lookup level, and a reset of one GPU page would not show the built-in default.
- **The CPU core grid and the GPU engine grid as group widgets.** Rejected: each is one element with three paths for every cell (ADR-009 rule 7, the `perf-cpu` budget of ADR-014); a group makes one `ChartView` per member. A cores or engines group is still offered for users who want one.

### Verification

- tl-model: a fixture per old GPU mode, with and without `hidden` engines, with "VRAM" hidden, and with `arrange` and `split` set, converts to the expected widgets; converting twice gives the same layout; no entry and `widgets = []` load as different states; an unknown metric key and an unknown device key are kept; a newer `version` and an unknown container are written back unchanged; each drop rule drops only its widget; after `reset_to_default` on a converted GPU box, a save and a load give the built-in default with no `overview.gpu` entry, the old keys gone and no conversion reported.
- tl-app: the first save after a conversion writes `settings.toml.pre-layouts-v1` with the old bytes, and a later save does not change it.
- tl-ui (`test-hooks`): every composition at its minimum size and larger; the three unavailable states; two widgets on one source share one `SourceData`; inner grid insert, move, resize and shrink at 1.0, 1.5 and 2.0 scale and at the GPU box minimum of 200 × 220 px; no overlap after random actions (proptest); Apply saves, Cancel restores, Undo steps back; the counter test of the render contract above; widget entity ids survive a move of another widget and a Cancel.
- Phase gate (`tasklens-lhliq.8`): migration fixtures for every old GPU box setting; a visual check of the default and of each old mode against screenshots of 1.1.0 taken in a hidden session; a hotplug test (a GPU gone and back keeps the layout and shows the unavailable state between); the `perf-overview` and `perf-overview-static` bench scenes within their ADR-014 budgets with the GPU box on widgets.

---

## ADR-020: tl-ui crate layers

### Context

tl-ui is one crate of about 222k lines, and its integration tests are one `ui` binary of 142.4k lines (80 files, 366 MB). Every edit compiles tl-ui twice: once for the app build and once with feature `test-hooks` for the tests. In a 40.8 h window, lanes spent 6.14 h compiling test builds, mostly tl-ui and the `ui` binary. The measurements are in `docs/plan/crate-split.md` section 1.

Oystein approved splitting tl-ui into layered crates and moving the UI settings out of tl-model on 2026-11-01 (epic `tasklens-zljr5`, decisions in `docs/plan/crate-split.md` section 9). This ADR is the target layout and the rules every split bead follows. The bead order and the reasons for it are in `docs/plan/crate-split.md` section 4. Where this ADR and the plan differ, this ADR wins.

### Decision

**Crates.** Twelve new crates under `crates/`. The workspace grows from 14 to 26 members.

| Crate | Holds | Lines (about) |
|---|---|---|
| `tl-ui-settings` | The UI half of tl-model `settings.rs` (`Settings` and the UI sections), plus `layout.rs`, `layout_boxes.rs` and `layout_pages.rs` from `crates/tl-model/src`. tl-model keeps `EngineSettings`, `HistorySettings`, `NetworkUnit` and the engine ranges. | 7.5k |
| `tl-ui-base` | Theme, text, strings, focus, animation, hosts, the action tracker, app and shell types | 14k |
| `tl-ui-kit` | Components and the helper flow | 22k |
| `tl-ui-chart` | The chart element and views, network totals | 9k |
| `tl-ui-table` | `TlTreeTable`, the text cache, the shared system table | 10k |
| `tl-ui-perf-core` | Device keys, perf actions, details, core clocks, minigraphs, the tray bridge, the detached graph window | 10k |
| `tl-ui-overview` | The Overview page and the widget container | 40k |
| `tl-ui-perf` | `PerformancePage`, the device pages, the mini window | 31k |
| `tl-ui-procs` | The Processes page and its dialogs and property windows | 43k |
| `tl-ui-sys` | The Services, Connections and Startup pages and their windows | 17k |
| `tl-ui` (the shell) | The main window, app init, the Information page, the run, low-memory and SMART windows, the bench | 32k |
| `tl-ui-testkit` (dev only) | `FakeEngine`; the render helpers behind its feature `render` | 0.8k |
| `tl-ui-pagehost` (dev only) | A host that mounts one page with its globals, without `MainWindow` | 0.5k |

**Where each file goes.** Paths are today's paths under `crates/tl-ui/src`. A file keeps its path relative to `src/` in its new crate, so `pages/processes/menu.rs` becomes `crates/tl-ui-procs/src/pages/processes/menu.rs`.

| Crate | Files |
|---|---|
| `tl-ui-base` | `theme/`, `text/`, `i18n.rs`, `selector.rs`, `util.rs`, `focus.rs`, `anim.rs`, `components/motion.rs` (as the root module `motion`), `hub.rs`, `demand.rs`, `settings_global.rs`, `host.rs`, `host_settings.rs`, `hosts_api.rs`, `hosts_edit.rs`, `action_tracker.rs`, `actions.rs`, `engine_link.rs`, `request_ends.rs`. Also the files the cycle beads create: `template.rs` (`Template`, CS-C5), `app_env.rs` and `registry.rs` (CS-C1, CS-C2), `shell.rs` (shell types and `ShellApi`, CS-C2 to CS-C4). |
| `tl-ui-kit` | `components/` (all but `motion.rs`), `helper_flow.rs` |
| `tl-ui-chart` | `chart/`, `net_totals.rs` (it uses `chart::view::derived_view`) |
| `tl-ui-table` | `table/`, `text_cache.rs`, `pages/sys_table.rs`, `pages/sys_status.rs`, `pages/sys_copy.rs` |
| `tl-ui-perf-core` | `pages/performance/actions.rs`, `pages/performance/details.rs` and `pages/performance/core_clocks.rs`, which CS-C8 moves to `pages/performance/core/` together with `DeviceKey`, `DeviceKind` and the shared constants and helpers the overview and widget code name; `pages/minigraph/`; `tray_bridge.rs`; `windows/detached_graph.rs` |
| `tl-ui-overview` | `pages/performance/overview/`, `pages/performance/widget/`, `pages/performance/memory_apps.rs` |
| `tl-ui-perf` | Every other file under `pages/performance/`: `mod.rs` (`PerformancePage`), `page.rs`, `page_flow/`, `aggregate.rs`, `card.rs`, `sidebar.rs`, `status_bar/`, `menu.rs`, `summary.rs`, `cpu.rs`, `cpu_board.rs`, `cpu_details.rs`, `cpu_grid.rs`, `memory.rs`, `memory_bar.rs`, `disk.rs`, `gpu.rs`, `net.rs`, `sensors.rs`. Also `windows/mini.rs`. |
| `tl-ui-procs` | `pages/processes/`; `windows/props/`, `windows/app_props.rs`, `windows/affinity.rs`, `windows/priority.rs`, `windows/signal.rs`, `windows/handle_search.rs`, `windows/notices.rs`, `windows/actions.rs`, `windows/disk_in_use.rs`, `windows/frame.rs` |
| `tl-ui-sys` | `pages/services/`, `pages/connections/`, `pages/startup/`; `windows/service_properties/`, `windows/connection_properties.rs`, `windows/startup_properties.rs`, `windows/confirm_startup_delete.rs` |
| `tl-ui` (shell) | `lib.rs`, `app.rs`, `window/`, `pages/mod.rs`, `pages/information/`, `windows/mod.rs`, `windows/run.rs`, `windows/run_history.rs`, `windows/low_memory.rs`, `windows/smart.rs`, `bench/` |
| `tl-ui-testkit` | `testing/fake_engine.rs`; `testing/render.rs` behind feature `render` (CS-B1) |

A new file goes to the crate that holds its directory. A new file at the root of `src/`, or in `pages/` or `windows/` outside a listed subdirectory, needs a row in this table, added by the bead that creates it.

The `action_tracker.rs` loop (settings_global uses host, host holds an `ActionTracker`, `ActionTracker` uses `components::dialog` and `components::toast`, components use settings_global and host) is broken by CS-C6. If CS-C6 cannot break it cleanly, tl-ui-base and tl-ui-kit merge into one `tl-ui-kit` of about 36k lines, and every rule below reads "kit" for "base".

**Allowed edges.** Each crate may depend on the tl-ui crates in its row, plus tl-model and gpui-kit. No other workspace crate is allowed.

| Crate | May depend on |
|---|---|
| `tl-ui-settings` | tl-model only, no gpui-kit |
| `tl-ui-base` | settings |
| `tl-ui-kit` | base, settings |
| `tl-ui-chart` | kit, base, settings |
| `tl-ui-table` | kit, base, settings |
| `tl-ui-perf-core` | chart, table, kit, base, settings |
| `tl-ui-overview` | perf-core, chart, table, kit, base, settings |
| `tl-ui-perf` | overview, perf-core, chart, table, kit, base, settings |
| `tl-ui-procs` | perf-core, chart, table, kit, base, settings |
| `tl-ui-sys` | procs, perf-core, chart, table, kit, base, settings |
| `tl-ui` (shell) | every crate above |
| `tl-ui-testkit` | tl-model and gpui-kit only |
| `tl-ui-pagehost` | testkit, kit, base, settings |

Page crates (overview, perf, procs, sys) never depend on each other, with two exceptions: tl-ui-perf on tl-ui-overview, and tl-ui-sys on tl-ui-procs. tl-app depends on tl-ui only; the shell re-exports what tl-app needs. tl-engine, tl-server and tl-remote depend on tl-model and not on tl-ui-settings, so a UI settings edit no longer rebuilds them.

```text
tl-app -> tl-ui (shell) -> tl-ui-sys -> tl-ui-procs --------\
                        -> tl-ui-perf -> tl-ui-overview -----> tl-ui-perf-core -> tl-ui-chart -> tl-ui-kit -> tl-ui-base -> tl-ui-settings -> tl-model
                                                              \-> tl-ui-table --/
```

The ADR-001 rule 1 closure for tl-ui applies to every `tl-ui-*` crate: no tl-engine, tl-collect or tl-ops, and arc-swap and zbus only through gpui-kit. asupersync stays out of every tl-ui closure (rule 2).

**Test hooks.** Test-only code is gated on `cfg(any(debug_assertions, feature = "test-hooks"))`. Items that only unit tests use may add `test` to that list. The `test-hooks` feature stays only for release-mode bench builds (`bench` enables it). Debug builds (`cargo run`, the smoke harness) compile the hooks behind their runtime checks; release builds never do. A hook that changes behaviour must do nothing until a test turns it on. The hook kinds and the list of switches follow; CS-B3 to CS-B7 add the switches of the files they convert. CS-B2 to CS-B7 move the gates one directory at a time, and CS-B8 removes `--features test-hooks` from tl-app's dev-dependency, the tests, the scripts and the docs.

**Test hook kinds.** Every test hook is one of three kinds:

- Recorder: a counter or log (a `Cell`, a `RefCell` log, a `Vec` field) that only records what the code did.
- Reader: a function that returns state.
- Switch: a hook that changes behaviour, such as a fake clock, a forced state or injected data. A switch must do nothing until a test sets it: its state is an `Option`, a flag or a GPUI global that is off by default, or it is a function that no app code calls.

`scripts/split/hooks_gate.py` moves a file's gates to the rule above. Recorders that grow for the app's life are acceptable in debug builds: `ActionTracker::pushed` and `opened_details` (`action_tracker.rs`) and `SysActions::pushed` (`pages/sys_table.rs`) and `MinigraphPanel::toasts` (`pages/minigraph/panel.rs:141`) keep one entry per toast or Details dialog. The recorders a test turns on (`table/cells.rs:698` `record_painted_texts`, `anim.rs:207` `AnimationDriver::install_paint_log`) record nothing until then: their log is an `Option` that starts as `None`.

Switches in the files CS-B2 converted (paths under `crates/tl-ui/src/`). None needed a fix:

| Hook | What it changes | Guard |
|---|---|---|
| `anim.rs:202` `AnimationDriver::set_cpu_clock` | the CPU clock the frame stats read | the field starts as `tl_model::time::thread_cpu_ns`; only tests call the setter |
| `selector.rs:91` `tag_frame`, read at `selector.rs:35` | the debug selector each element gets during one frame | the thread-local `LOG` is `None` until `tag_frame` sets it; `tag_frame` and `untag_frame` set it back to `None` |
| `i18n.rs:95` `set_test_catalogue` | the string catalogue | only tests call it |
| `hub.rs:117` `HostClock::remote_with_local_shift`, `hub.rs:124` `HostClock::shift_local`, `hub.rs:341` `SnapshotHub::new_remote_with_clock` | a remote host clock's offset from this PC's clock | only tests call them; the app makes remote clocks with `HostClock::remote`, shift 0 |
| `components/window_frame.rs:106` `TestClientFrame`, read at `components/window_frame.rs:122` and `:958` | the frame inset, as if the window had client decorations | a GPUI global that the app never sets; `has_global` is false until a test sets it |
| `app.rs:174` `AppEnv::test`, `app.rs:645` `open_first_for_test`, `app.rs:651` `start_for_test` | test app settings and app start | only tests call them |
| `pages/startup/copy.rs:162` `copy_rows_as_table_for_test` | runs the page's "Copy rows as table" | only tests call it |
| `table/mod.rs:10` module `test_delegate` | a fake table delegate | only tests build one |
| `window/main.rs:363` `TestPages`, set by `main.rs:370` `set_test_page`, read at `main.rs:377` | the page a main window tab builds | a GPUI global the app never sets; `try_global` is `None` until a test sets it |
| `window/main.rs:782` `FailMainOpen`, read at `main.rs:790` | makes the first main window open fail | a GPUI global the app never sets |
| `window/titlebar.rs:152` `TitleBarProbe`, read at `titlebar.rs:172`, `:186`, `:209`, `:218` and `main.rs:1022`, `:2806` | window decorations, maximized state and controls; skips `minimize_window`; counts moves, zooms and minimizes | a GPUI global the app never sets; every read is `try_global` or `has_global`, false until a test sets it |
| `window/settings/mod.rs:247` `TestPages` (settings), read at `settings/mod.rs:239` | extra pages after the real settings pages | a GPUI global the app never sets |
| `window/main.rs:2243` `set_picking_for_test`, `main.rs:2431` `apply_close_for_test`, `main.rs:2518` `scale_factor_changed_for_test` | window picking, a window close, a scale factor change | only tests call them |
| `chart/element.rs:165` `PLOTS`, set by `element.rs:174` `take_plots`, written at `element.rs:156` | collects the plot of every chart paint (CS-B3 fix: it was a `Vec` that grew on every paint in a debug app) | the thread-local is `None` until a test's first `take_plots` call; the paint writes only when it is `Some` |
| `chart/view.rs:642` `ChartView::set_series_for_test`, `chart/grid_element.rs:597` `ChartGridView::set_series_for_test` | replaces the series a chart shows | only tests call them |

**Testkit and page host.** tl-ui-testkit depends only on tl-model and gpui-kit (`FakeEngine`, and the render helpers behind its `render` feature, which turns on gpui-kit `test-support`). Every tl-ui crate may use it from unit tests. tl-ui-pagehost depends on tl-ui-base, tl-ui-kit and tl-ui-testkit. It is used only from integration test binaries under `tests/`, never from `#[cfg(test)]` code in base or kit, because that would build two copies of base.

**Move method.** A crate extraction is one commit:

1. `git mv` the files into `crates/<crate>/src/`, keeping their paths relative to `src/`.
2. The new crate's `lib.rs` imports the lower crates' modules at its root (`use tl_ui_base::{theme, i18n, ...};`), so moved code keeps its `crate::theme::...` paths. Its `pages` and `windows` modules do the same for the lower crates' modules under `pages/` and `windows/`.
3. tl-ui's `lib.rs` replaces `pub mod x;` with `pub use tl_ui_x::x;`, and the shell's `pages/mod.rs` and `windows/mod.rs` do the same for their submodules, so upper code, tl-app and the tests keep their `tl_ui::x` paths.
4. `pub(crate)` and `pub(super)` items used across the new boundary become `pub`. A `#[cfg(test)]` helper used across the boundary becomes `#[cfg(any(test, debug_assertions))] pub`, or moves with the tests that use it.
5. The root `Cargo.toml` gains the member, and `xtask/src/check_deps.rs` gains the crate's row from ADR-001.

These root re-exports stay during the split. CS-J1 rewrites them to direct crate paths after CS-E10 and CS-F9. A move does not change behaviour: if a test needs a changed assertion, the move is wrong.

**Actions.** Each crate that declares `actions!` exports `pub fn init(cx: &mut App)` (or a `bind_keys_*` function) that tl-ui's app init calls. That call keeps the crate linked, so its actions resolve from the keymap. A shell test (CS-E10) checks that every keymap action name resolves and that every crate with `actions!` is initialised.

**Tests.** Each crate has one integration test binary, `crates/<crate>/tests/<short>/main.rs`, where `<short>` is the crate name without `tl-ui-` (for example `crates/tl-ui-kit/tests/kit/main.rs`). Tests that need the shell (`MainWindow`, app init, more than one page) stay in tl-ui's `tests/ui`. Page tests use tl-ui-pagehost. A test move takes the golden files only that test uses and reads them through `CARGO_MANIFEST_DIR`. Golden files shared between crates stay where they are, and the tests that read them name them in a comment.

**Lane commands after the split.** `heavy-build cargo test -p tl-ui-<crate>` for the crate a bead touches, plus `cargo clippy -p tl-ui-<crate> --all-targets -- -D warnings`. CS-B9 writes the final commands and the leader-gate rule into AGENTS.md.

### Consequences

- An edit rebuilds its crate and the crates above it. A lane tests one crate and links that crate's test binary instead of the 366 MB `ui` binary.
- tl-ui is compiled once per edit, because the hooks no longer need a feature flip.
- A full `cargo build -p tl-app` after a kit edit still runs rustc for every UI crate above kit; incremental compilation makes most of that cheap.
- The workspace has 26 members. CS-I1 measures a build from an empty cache against the baseline of 873 units and 381 s.
- Debug app builds carry the test hooks. Release builds do not.

### Rejected alternatives

- **Keep the `testing` module inside tl-ui behind `debug_assertions`.** Rejected: `render.rs` needs gpui-kit `test-support`, which app builds do not have (crate-split.md section 3 item 9).
- **Keep the `test-hooks` feature for tests.** Rejected: it compiles tl-ui and the crates above it twice per edit.
- **One test binary per test file.** Rejected: each binary links GPUI, about 300 MB each.
- **Page host used from unit tests in base or kit.** Rejected: the page host depends on base, so base would be built twice.
- **Split crates first and break the cycles after.** Rejected: a lower crate cannot name an upper module. Every cycle is broken inside tl-ui first (CS-C1 to CS-C11).

### Verification

- `cargo xtask check-layers` (CS-A2) passes on every commit. Its allowlist of today's upward edges may only shrink, and CS-C11 empties it.
- `cargo xtask check-deps` passes, and each `tl-ui-*` crate has a row in `xtask/src/check_deps.rs` that matches its ADR-001 row.
- The CS-E10 shell test: every keymap action name resolves, and every crate with `actions!` is initialised.
- CS-I2 runs the full suite and compares the test count with the 7,163 tests of the last full run before the split.
- CS-I1 records build times, units and `target/` size against the baselines in crate-split.md section 1.
