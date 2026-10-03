# B1 infra beads: plan keys and bead IDs

Batch B1 covers epics E01, E02, E03, E31, E32 and E33 (`docs/plan/00-epics.md` section 5). The
leader created the epic and anchor beads (`docs/plan/anchors.md`); B1 replaced each anchor's
placeholder with the full template and created every other bead under its epic. Cross-epic
dependencies point at anchor beads only.

Bead titles start with the plan key. Open items list values the docs do not decide. Each bead
that uses one carries the recommendation in its text and says it is pending the leader.

## E02 Workspace, toolchain, lints, check-deps, CI

Epic bead: `tasklens-v9m`. 12 beads, all P0 (priority 0), labels `phase-p0`, `area-workspace-ci` plus a `crate-*` label. No cross-epic dependencies (E02's row in 00-epics section 4 is "none").

| Plan key | Bead ID | Title | P | Depends on (plan keys or anchor IDs) |
|---|---|---|---|---|
| E02-001 | tasklens-v9m.3 | Toolchain pin, cargo alias, clippy config | 0 | none |
| E02-002 | tasklens-v9m.4 | Root Cargo.toml with pins, profiles and lints | 0 | E02-001 |
| E02-003 | tasklens-v9m.5 | Skeletons for tl-model, tl-collect and tl-ops | 0 | E02-002 |
| E02-004 | tasklens-v9m.6 | Skeletons for tl-engine and tl-gpu-probe | 0 | E02-003 |
| E02-005 | tasklens-v9m.7 | Skeletons for tl-ui and tl-app | 0 | E02-004 |
| E02-006 | tasklens-v9m.8 | xtask package with subcommand dispatch | 0 | E02-002 |
| E02-A1 | tasklens-v9m.1 | Workspace compiles (Cargo.lock, `build --workspace --locked`, ADR-002 verification, first U-1 reading) | 0 | E02-001, E02-003, E02-004, E02-005, E02-006 |
| E02-007 | tasklens-v9m.9 | check-deps: banned crates and asupersync greps (checks 1, 3, 4) | 0 | E02-A1 |
| E02-008 | tasklens-v9m.10 | check-deps: allowed rows and closure rules (check 2) | 0 | E02-007 |
| E02-009 | tasklens-v9m.11 | scripts/ci.sh running the ADR-013 CI rows | 0 | E02-008 |
| E02-010 | tasklens-v9m.12 | GitHub Actions workflow with lavapipe | 0 | E02-009 |
| E02-A2 | tasklens-v9m.2 | check-deps and CI (failing-branch run recorded, U-1 settled) | 0 | E02-008, E02-009, E02-010 |

Structural choices made by this batch (no doc gives them; each is the smallest option):
- Skeletons carry the internal workspace edges plus `gpui-kit` in tl-ui and `asupersync` in tl-engine, so U-1 and the ADR-002 verification commands can run in E02. Owners add every other external crate when they first use it.
- `[workspace.package]` holds `edition = "2024"`; crates use `edition.workspace = true`.
- `.gitignore` with `/target`; `clippy.toml` with `allow-unwrap-in-tests = true` (implements ADR-002 "unwrap_used ... allowed in tests").

### Open items

1. **Package name of the `tasklens` binary.** ADR-001 calls the crate `tl-app` (bin "tasklens"), but ADR-002's verification writes `cargo tree -p tasklens`, which needs the package to be named `tasklens`. Recommendation: package `tl-app` with `[[bin]] name = "tasklens"` (matches the crate directory, the ADR-001 table and the `crate-tl-app` label) and change ADR-002's command to `-p tl-app`. Beads E02-005, E02-A1 and E02-007 are written this way and say how to switch.
2. **`bench` feature lines conflict with the E01 chain edit.** 00-epics section 2 says E01 adds the `bench` feature to `tl-ui/Cargo.toml` by chain edit, but ADR-002 fixes the exact text, and E33-A1 (P0, `args.rs` declares `--bench` behind `bench`) needs `tl-app`'s `bench = ["tl-ui/bench"]`, which cannot compile without tl-ui's `bench`. E01 depends on E33-A1, so waiting for E01 would block E33-A1. Recommendation: E02-005 writes both `bench` lines up front (done); drop or empty the E01 chain-edit bead for the tl-ui `bench` feature.
3. **Two banned asupersync APIs cannot be found by a name grep.** ADR-004 bans `JoinHandle` from `RuntimeHandle::spawn` and `Budget::with_poll_quota` on long-lived tasks only. Recommendation (written into E02-007): match the literals `RuntimeHandle::spawn(` and `.with_poll_quota(`, accepted only with a same-line `// check-deps: allow <reason>` comment.
4. **gpui-kit may put `zbus` in tl-ui's closure.** GPUI's Linux platform uses the XDG portals, which usually go through `ashpd` and `zbus`. ADR-001 check 2 fails if `zbus` is in tl-ui's normal closure. Not verified. E02-A1 records the answer from `cargo tree -p tl-ui`. Recommendation if it appears: narrow the rule to "tl-ui has no direct dependency on zbus or arc-swap and no path to them that does not go through gpui-kit", recorded as an ADR-001 change.
5. **CI system packages on Ubuntu.** ADR-002 lists Arch packages only. Recommendation (written into E02-010): `clang pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libfontconfig1-dev libvulkan1 mesa-vulkan-drivers`, lavapipe ICD at `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`.
6. **Workspace `version` and `publish`.** Not set by any doc; `--version` (E33) prints it. Recommendation: `version = "0.1.0"`, `publish = false` in `[workspace.package]` (written into E02-002 as pending).
7. **`rustfmt.toml`.** 00-epics lists it as an E02 file, but no doc gives options. Recommendation: do not create it; default rustfmt style (E02-001 is written that way).

## E03 tl-model: every data contract

Epic bead `tasklens-as2`. 37 beads: E03-001 writes `lib.rs` with the full module list and one placeholder file per module, so `lib.rs` never changes again. The other beads fill one to three files each. E03-A1 caps the core types, E03-A2 caps the contracts.

### E03 first half: core types (E03-A1, E03-001 to E03-013)

| Plan key | Bead ID | Title | P | Depends on |
|---|---|---|---|---|
| E03-001 | tasklens-as2.3 | tl-model dependencies, full module list and placeholder files | 0 | E02-A1 (tasklens-v9m.1) |
| E03-002 | tasklens-as2.4 | time.rs: CLOCK_BOOTTIME timestamps and BootId | 0 | E03-001 |
| E03-003 | tasklens-as2.5 | validity.rs: Validity, ProcessField, FieldValidity | 0 | E03-001 |
| E03-004 | tasklens-as2.6 | ids.rs: ProcessKey, DeviceIdx, MetricId, SeriesId, GroupKey, NodeId | 0 | E03-002 |
| E03-005 | tasklens-as2.7 | ids.rs: ColumnId with frozen numbers and string ids | 0 | E03-004 |
| E03-006 | tasklens-as2.8 | series.rs: Sample, SampleFlags, SeriesRing, SeriesView | 0 | E03-002, E03-004 |
| E03-007 | tasklens-as2.9 | series.rs: TieredRing with the 1 Hz per-second-max tier | 0 | E03-006 |
| E03-008 | tasklens-as2.10 | process.rs: ProcessMeta, ProcessRecord, flags, groups, coverage | 0 | E03-003, E03-004 |
| E03-009 | tasklens-as2.11 | system.rs: CPU, memory, disk, net, GPU and sensor totals, CpuSet, topology | 0 | E03-003, E03-004 |
| E03-010 | tasklens-as2.12 | devices.rs: DeviceIdentity, DeviceEntry, DeviceCatalog, NetKind | 0 | E03-004 |
| E03-011 | tasklens-as2.13 | health.rs: SourceId, SourceState, SourceHealth, HealthSnapshot | 0 | E03-002 |
| E03-012 | tasklens-as2.14 | units.rs: percent, sizes and rates | 0 | E03-001 |
| E03-013 | tasklens-as2.15 | units.rs: counts, times, frequencies, power, caches, addresses | 0 | E03-012 |
| E03-A1 | tasklens-as2.1 | Core types compile with size tests (sizes.rs, frozen_ids.rs, frozen lists) | 0 | E03-001 to E03-013 |

### Open items (E03 first half)

Each bead is written in full with the recommendation below; the leader confirms or changes it before the bead starts.

1. **ColumnId numbers.** ADR-012 says `ColumnId` numbers are frozen, but no doc assigns them. E03-005 proposes 1 to 121 in the order of PROC-027 to PROC-036 (`name` = 1, `container` = 121). Bit index = number - 1, so a 128-bit `ColumnSet` fits.
2. **ColumnId scope.** Only process-table columns are `ColumnId`s. Services (19 ids), Startup Apps (7), Connections (17) and Properties-tab columns share some strings (`name`, `pid`, `cpu`) with other meanings. Recommendation: page-local column enums, or a `SysColumnId` in `table.rs` (E03-A2 half).
3. **SysNodeId location.** 00-epics puts `SysNodeId` in `ids.rs`, but it refers to `Manager`, `UnitRef`, `StartupRef` from `table.rs` and ADR-003 places it in `table.rs`. Recommendation: `table.rs` (E03-A2 half). E03-004 leaves it out.
4. **GroupKey hash.** `GroupKey` is "FxHash of GroupKind", but `rustc-hash` is not in tl-model's ADR-001 row (DetailsSnapshot's `FxHashMap` has the same problem). Recommendation: add `rustc-hash` to tl-model's row; until then E03-004 defines only the newtype and the hash lives with the classifier (E05).
5. **NodeId and GroupKind variants.** ADR-009 `NodeId` has only `Group` and `Process`; ADR-003 `GroupKind` only App, Background, System, Ungrouped. Unit rows, the "Kernel threads (N)" row and pseudo rows (PROC-012) have no identity. Recommendation: add `NodeId::Unit(UnitRef)`, `NodeId::KernelThreads`, `NodeId::Pseudo(PseudoKind)` (Interrupts, MemoryCompression); needs an ADR-009 edit.
6. **GroupRow fields.** ADR-003 says only "aggregated values". E03-008 mirrors `ProcessRecord`'s numeric fields plus name, icon, member count and `FieldValidity`.
7. **System totals fields.** ADR-003 names `CpuTotals`, `MemoryTotals`, `DiskTotals`, `NetTotals`, `GpuTotals`, `SensorReading` without fields. E03-009 derives each field from a PERF row and adds a `Metric<T> { value, validity }` helper.
8. **HealthSnapshot header.** `ComponentHeader` lives in `snapshot.rs` (E03-A2 half, which depends on E03-A1), so `HealthSnapshot` uses a plain `version: u64` to avoid a cycle. The alternative is moving `ComponentHeader` into `validity.rs`.
9. **SourceState variants and missing SourceIds.** E03-011 uses Off, Running, Late, Stale, Failed, `Disabled { until }`, and caps `last_error` at 200 characters. ADR-004 has no `SourceId` for the public IP fetch or the low-memory sweep; recommendation: low-memory reports as `Processes`, public IP is not a supervised source.
10. **1 Hz tier details.** "Per-second maximum" is fixed; E03-007 adds: slow sample time is the start of the second, flags are ORed, only valid values enter the max, skipped seconds produce no sample. Also the fast ring's capacity when a source changes rate (CPU 10 Hz to 1 Hz, "keeps its ring") is not decided; recommendation: size the ring for the fastest period the source can use.
11. **Measured-zero rule for every form.** PROC-003 states "rounds to 0.0 counts as zero" for percent only; E03-012 applies it to all rate, power and time forms so "0.0 Mbps" is never shown. Related: `percent_2dp` zero form "0%", `watts_pair` prints the limit as an integer (table (d) example "31.9 / 300 W").
12. **Local time conversion.** Start time, Started and Boot time need local broken-down time; no allowed tl-model or tl-ui dependency does time zones. E03-013 formats a caller-supplied `LocalDateTime`. Recommendation: one time-zone crate pin in tl-ui's row.
13. **Device persistent key and NetKind carrier.** PERF-005 keys need data `DeviceIdentity` lacks (fan device path, disk by-path name), and `DeviceEntry` has no `NetKind` field. Recommendation: add `persistent_key: Arc<str>` and `net_kind: Option<NetKind>` to `DeviceEntry` (ADR-003 edit).
14. **bitflags serde feature.** `SampleFlags` and `ProcFlags` derive serde; ADR-002 pins `bitflags = "2"` without the `serde` feature. Recommendation: E02 adds `features = ["serde"]` to the workspace pin.

### E03 contracts half (E03-A2, E03-050 to E03-071)

Every bead is priority 0 and labelled `phase-p0`, `crate-tl-model`, `area-model` (068 and 070 also `security`), because they feed the P0 anchor E03-A2. Every bead also depends on E03-A1 (`tasklens-as2.1`). Beads that edit the same file are chained: `snapshot.rs` 050, 054, 055, 056, 064; `table.rs` 051, 052; `details.rs` 057, 058, 059, 070; `settings.rs` 065, 066, 067.

| Plan key | Bead ID | Title | P | Depends on |
|---|---|---|---|---|
| E03-050 | tasklens-as2.16 | snapshot.rs: component header, mask, series, process, system and windows components | 0 | E03-A1 |
| E03-051 | tasklens-as2.17 | table.rs: process TableQuery, TableRow, ProcessTableView and header totals | 0 | E03-050 |
| E03-052 | tasklens-as2.18 | table.rs: system-table query, rows, UnitRef, StartupRef and ServiceFilter | 0 | E03-051 |
| E03-053 | tasklens-as2.19 | actions.rs: action requests, targets, kinds, states, events and ActionError | 0 | E03-052 |
| E03-054 | tasklens-as2.20 | snapshot.rs: Services component and UnitDetails | 0 | E03-050, E03-051, E03-052 |
| E03-055 | tasklens-as2.21 | snapshot.rs: Startup Apps and Connections components | 0 | E03-054, E03-052, E03-053 |
| E03-056 | tasklens-as2.22 | snapshot.rs: Inventory component and public IP state | 0 | E03-055, E03-051 |
| E03-057 | tasklens-as2.23 | details.rs: Properties frame, General and Threads payloads | 0 | E03-051, E03-052, E03-053 |
| E03-058 | tasklens-as2.24 | details.rs: file descriptor, memory map, security and environment payloads | 0 | E03-057 |
| E03-059 | tasklens-as2.25 | details.rs: Strings, App, handle search, affinity and DetailsSnapshot | 0 | E03-058, E03-050 |
| E03-060 | tasklens-as2.26 | demand.rs: Demand, ColumnSet, DetailDemand and TrayDemand | 0 | E03-057, E03-052 |
| E03-061 | tasklens-as2.27 | journal.rs: journal query, events and JournalHandle | 0 | E03-052 |
| E03-062 | tasklens-as2.28 | history.rs: RecordSink and HistoryView types | 0 | E03-050 |
| E03-063 | tasklens-as2.29 | command.rs: Command, KWinCommand, SetupStep, history query, AppRequest and Tab | 0 | E03-050, E03-051, E03-052, E03-053, E03-059, E03-062 |
| E03-064 | tasklens-as2.30 | snapshot.rs: Snapshot struct, empty snapshot and change mask | 0 | E03-056, E03-059, E03-062 |
| E03-065 | tasklens-as2.31 | settings.rs: Settings root and sections general to properties | 0 | E03-051, E03-053 |
| E03-066 | tasklens-as2.32 | settings.rs: settings sections performance to debug | 0 | E03-065, E03-052 |
| E03-067 | tasklens-as2.33 | settings.rs: UiState, EngineSettings and clamping | 0 | E03-066, E03-063 |
| E03-068 | tasklens-as2.34 | wire.rs: length-prefixed postcard framing with limits | 0 | E03-A1 |
| E03-069 | tasklens-as2.35 | gpu_proto.rs: GPU probe messages | 0 | E03-068 |
| E03-070 | tasklens-as2.36 | helper_proto.rs: helper allowlist, replies and protocol round trips | 0 | E03-053, E03-059, E03-068, E03-069 |
| E03-071 | tasklens-as2.37 | client.rs: EngineClient trait | 0 | E03-064, E03-060, E03-061, E03-063, E03-067 |
| E03-A2 | tasklens-as2.2 | E03-A2 Contracts compile (anchor; `tests/settings_defaults.rs`, table (a) defaults) | 0 | E03-A1, E03-050 to E03-071 |

### Open items (E03 contracts half)

Each bead names the open item it touches in its Notes. Where a bead had to carry a value, it carries the recommendation below and says so.

1. **SysNodeId location.** ADR-003 puts `SysNodeId` in `table.rs`; 00-epics section 2 lists it under `ids.rs`. It needs `UnitRef` and `StartupRef` from `table.rs`. Recommendation: `table.rs` (E03-052 does this); the E03-A1 half must not define it in `ids.rs`.
2. **FxHashMap in tl-model.** ADR-003 uses `FxHashMap` for `DetailsSnapshot.targets`, `.affinity` and `ServicesSnapshot.details`, but `rustc-hash` is not in tl-model's ADR-001 row. Recommendation: `std::collections::HashMap` (maps hold at most 256 entries); beads use it. Alternative: add `rustc-hash` to the row.
3. **StartupRef identity.** No doc gives its fields. Recommendation: `StartupRef { source: StartupSource, id: Arc<str> }` (id = desktop file name, unit id or script file name).
4. **ColumnId dense index for ColumnSet.** ADR-008 says "bitset of ColumnId" with no size. E03-060 needs a dense index below 256 from E03-A1's `ColumnId`. Recommendation: E03-A1 gives `ColumnId` a stable dense index (`index() -> u16`, count under 256).
5. **TrayDemand fields** (P3). Undefined. Recommendation: `{ icons_shown, cpu, memory, disks, nets, gpus, process_keys: Arc<[ProcessKey]> }`.
6. **SystemSnapshot and WindowEntry layout.** ADR-003 says only "the totals of system.rs plus cpu_topology". Recommendation: one field per totals type, per-device totals as `Arc<[(DeviceIdx, T)]>`; `WindowEntry` with `FrameGeometry` in i32 logical px and `window_type: i32` as KWin reports it.
7. **TableTotals field layout.** PROC-038 lists the totals; recommendation: one `Total<T>` per total, with network as rate plus summed link speed so the UI picks percent or rate.
8. **Service record types; Startup and connection record types.** Field names come from UI-SYS data bindings; types are derived (E03-054, E03-055), including `ImpactDetail`, `TcpInfoView` and `ConnRecord.four_tuple`.
9. **InventorySnapshot fields.** No doc defines them. Recommendation: per-section structs mapped from PERF-095 to PERF-104 static rows (E03-056).
10. **Public IP state and trigger.** PERF-060 has no snapshot home and no command. Recommendation: `InventorySnapshot.public_ip: PublicIpState` and `Command::RefreshNow(SourceId::PublicIp)` (needs that `SourceId` variant in E03-A1).
11. **Properties payload fields.** ADR-003 and UI-PWN name the payload types only. Recommendation: the one-to-one mapping from the UI-PWN-006 to UI-PWN-031 field lists in E03-057 to E03-059, including handle-search hits as an append-only chunk list.
12. **FourTuple, Family, Proto fields.** Recommendation: `FourTuple { local: SocketAddr, remote: SocketAddr }`, `Family { Inet, Inet6 }`, `Proto { Tcp, Udp }`.
13. **RecoveryEdit fields.** Recommendation: one `Option` per SYS-013 key (E03-053).
14. **Tab type.** `AppRequest::SelectTab(Tab)` uses a `Tab` no doc defines. Recommendation: `tl_model::command::Tab` with the six R-023 tabs and lowercase names; tl-ui's `TabId` should be this type.
15. **KWinCommand fields.** Docs name activate, close, keep above `{ pid, caption, on }`, pick, `CancelPick` and frame geometry. Recommendation: E03-063's variants (`Activate { internal_id }`, `Close { internal_id }`, `SetFrameGeometry { pid, caption, geometry }`).
16. **SetupStep and setup events.** Recommendation: `RaplEnable`, `RaplRevert`, `SensorsDetect`, `PolkitRule { on }` from table (c). The Fan sensors row's "Revert" button has no `tasklens-setup` subcommand (P4; needs a decision).
17. **Engine answers outside ActionEvent.** `Command::SetupStatus` and `RunSetup` answers (`SetupStatus`, `SetupEvent`) and the KWin pick reply ("an ActionEvent or a KWinEvent", UI-SHL-014) have no delivery channel. Recommendation: put them in `HealthSnapshot` (setup state) and `WindowsSnapshot` (`pick: Option<PickResult>`), both versioned components; needs a leader decision and an E03-A1 edit for `HealthSnapshot`.
18. **History query and view types** (P3). `HistoryRange`, `HistoryWhat`, `HistoryView` fields are derived from UI-PRC-030 to UI-PRC-037 (E03-062, E03-063).
19. **Helper protocol replies, handshake and Dump fd** (P3). ADR-006 and R-004 fix the request set only. Undecided: `HelperReply` shape, `HELPER_PROTO_VERSION` and `Hello`, `DropInKey` names, ADR-006's `ReadProcessDetail { key, what }` (no `what` defined; R-004's `ReadProcessFiles` appears to replace it), and how `Dump { out_fd }` passes a file descriptor over a stdin pipe (it cannot; options: SCM_RIGHTS on a socketpair instead of stdio, or the helper streams the core back in `File` replies). E03-070 carries a recommendation and leaves `out_fd` out.
20. **GpuApiVersions fields.** Recommendation: `{ pci_bus_id, vulkan_api, vulkan_driver, opengl }` (E03-069).
21. **Settings clamp step rounding.** Table (a) says "step 5" and "step 10"; recommendation: round to the nearest multiple after clamping (E03-067).
22. **Snapshot::empty and changed_since.** Not named in docs; added in E03-064 so the engine has a pre-publish value and the hub's mask rule is tested in one place.
23. **Test-only dev-dependencies of tl-model.** `proptest` (E03-068) and `toml` (E03-A2) are dev-dependencies. Confirm `xtask check-deps` (E02-A2) treats dev-dependencies separately from the ADR-001 allowed row.
24. **CpuSet serde.** E03-070 needs `CpuSet` (E03-A1, `system.rs`) to derive `Serialize, Deserialize`.

## E33 tl-app binary and packaging

Epic bead: `tasklens-cr0`. 22 beads (3 anchors, 19 others). Anchors are the wiring capstones: E33-A1 for the P0 binary, E33-A2 for the P1 settings and single-instance wiring, E33-A3 for the P1 package.

| Plan key | Bead ID | Title | P | Depends on |
|---|---|---|---|---|
| E33-001 | tasklens-cr0.4 | tl-app Cargo.toml dependencies, bench feature and build.rs git hash | 0 | E02-A1 (tasklens-v9m.1) |
| E33-002 | tasklens-cr0.5 | paths.rs: XDG path resolution with fallbacks and 0700/0600 modes | 0 | E33-001 |
| E33-003 | tasklens-cr0.6 | args.rs: clap declaration of every command-line argument | 0 | E33-001 |
| E33-004 | tasklens-cr0.7 | logging.rs: tracing subscriber, daily log file, 200-line ring | 0 | E33-002 |
| E33-005 | tasklens-cr0.8 | crash.rs: panic hook writes crash-<unix seconds>.txt, keeps newest 20 | 0 | E33-002, E33-004 |
| E33-A1 | tasklens-cr0.1 | Binary starts | 0 | E33-002, E33-003, E33-004, E33-005, E03-A2 (tasklens-as2.2), E08-A1 (tasklens-8hz.1), E09-A1 (tasklens-8q0.1), E15-A1 (tasklens-ug9.1), E16-A1 (tasklens-fut.1), E17-A1 (tasklens-lgw.1), E20-A1 (tasklens-jqk.1) |
| E33-007 | tasklens-cr0.10 | migrations.rs: schema migration chain for both TOML files | 1 | E33-A1 |
| E33-006 | tasklens-cr0.9 | settings_store.rs: load settings.toml and state.toml (corrupt, newer, clamp) | 1 | E33-A1, E33-007 |
| E33-008 | tasklens-cr0.11 | settings_store.rs save half, atomic.rs re-export and the persist callback | 1 | E33-006, E07-A3 (tasklens-iuz.3) |
| E33-009 | tasklens-cr0.12 | single_instance.rs: org.freedesktop.Application on the session bus | 1 | E33-A1, E03-A2 |
| E33-010 | tasklens-cr0.13 | notify.rs: desktop notifications through org.freedesktop.Notifications | 1 | E33-A1 |
| E33-A2 | tasklens-cr0.2 | Settings store | 1 | E33-006, E33-007, E33-008, E33-009, E33-010, E16-A1 |
| E33-011 | tasklens-cr0.14 | main.rs: engine shutdown in 1500 ms after the UI stops (U-2) | 1 | E33-A2, E09-A2 (tasklens-8q0.2) |
| E33-012 | tasklens-cr0.15 | main.rs: renderer start failure notice and exit 1; device-loss relaunch once | 1 | E33-A2 |
| E33-013 | tasklens-cr0.16 | Desktop file io.github.oysteinkrog.Tasklens.desktop | 1 | E02-A1 |
| E33-A3 | tasklens-cr0.3 | Packaging | 1 | E33-013, E33-A1, E15-A1 |
| E33-014 | tasklens-cr0.17 | autostart.rs: Start at login entry written or removed from settings | 2 | E33-A2, E07-A3 |
| E33-015 | tasklens-cr0.18 | PKGBUILD: install tl-helper, tasklens-setup and the polkit policy | 3 | E33-A3, E29-A1 (tasklens-qfx.1), E29-A3 (tasklens-qfx.3) |
| E33-016 | tasklens-cr0.19 | rendering_env.rs: rendering.gpu sets ZED_DEVICE_ID or software rendering | 4 | E33-A2 |
| E33-017 | tasklens-cr0.20 | kglobalaccel.rs: Open with Ctrl+Esc through KDE kglobalaccel (verify U-16) | 4 | E33-A2 |
| E33-018 | tasklens-cr0.21 | import_ts.rs: import TaskSlinger UserSettings.ini into settings | 4 | E33-A2 |
| E33-019 | tasklens-cr0.22 | --app-id: separate instance and settings directory for development builds | 4 | E33-A2 |

Steps of UI-SHL-110 by bead: 1 to 3 and 7 to 10 in E33-A1, 4 and 5 in E33-A2, exit in E33-011. The E01 (`--bench`, `--gallery`) and E32 (`--low-memory`, `--watch-memory`) arms are chain edits owned by those epics; E33-A1 leaves marked insertion points.

### Open items

1. **E33-OI-1. tl-app cannot call `Application::run`.** ADR-001 gives tl-app no gpui-kit dependency, yet UI-SHL-110 step 8 and ADR-010 have `main` run GPUI. Recommendation: E16-A1 exposes `tl_ui::run(UiLaunch) -> UiExit` that wraps `Application::new().run(..)` and `TasklensUi::init`, with `UiLaunch { client: Arc<dyn EngineClient>, settings, state, themes_json: Option<Vec<u8>>, persist: Option<Box<dyn Fn(&Settings, &UiState) + Send + Sync>>, app_requests: Option<async_channel::Receiver<AppRequest>>, queued_toasts: Vec<String>, start_tab: Option<Tab>, select_pid: Option<u32>, debug: bool, start_minimized: bool, pending_crash_report: Option<PathBuf>, on_quit: Box<dyn FnOnce() + Send> }` and `enum UiExit { Quit, RendererFailed(String), DeviceLost }`. Blocks E33-A1, E33-A2, E33-011, E33-012. The B5 writer (E16) must agree.
2. **E33-OI-2. No argument parser is pinned or allowed.** APP-052 says `clap` derive and UI-SHL-110 says "parse arguments with clap", but `clap` is neither in ADR-002 `[workspace.dependencies]` nor in the tl-app row of ADR-001. Recommendation: add `clap = { version = "4", default-features = false, features = ["std", "derive", "help", "usage", "error-context"] }` to the workspace (exact pin set by E33-001 after `cargo add`) and to the tl-app row; E02 writes the pin.
3. **E33-OI-3. `async-channel` is not in the tl-app row.** ADR-010 has tl-app create `async_channel::bounded(4)` for `AppRequest`. Recommendation: add `async-channel` to the tl-app row of ADR-001 (it is already a workspace pin).
4. **E33-OI-4. The persist callback type is not fixed.** ADR-011 says `Box<dyn Fn(&Settings, &State)>`; UI-FND-042 sends theme writes "through the tl-app persist callback" as `PersistTheme::{Save(Vec<ThemeDef>), RenameCorrupt}`; shell.md does not say whether tl-ui debounces before calling. Recommendation: two callbacks in `UiLaunch`: `persist(&Settings, &UiState)` called on every change (tl-app debounces 500 ms, E33-008), and `persist_theme(PersistTheme)` handled by tl-app with `write_atomic` at once. tl-ui passes theme bytes, so `PersistTheme` lives in tl-ui (tl-app depends on tl-ui, allowed).
5. **E33-OI-5. `write_atomic` arrives in a P2 anchor.** E07-A3 (`fs.rs`, `launch.rs`, `dump.rs`) is P2, but the P1 settings store (E33-008) needs `write_atomic`. Recommendation: the B2 writer splits `fs.rs` into its own P1 bead, or E07-A3 becomes P1.
6. **E33-OI-6. What `--app-id` changes is undecided.** APP-052 says "separate instance and settings dir". Not decided: the directory name (`$XDG_CONFIG_HOME/<id>/`?), whether state, logs, crash, history and icon cache follow, and the Wayland `app_id`. Recommendation: replace the `tasklens` directory component by the id in all four XDG roots and use the id as Wayland `app_id` and bus name. Blocks E33-019.
7. **E33-OI-7. Small strings not in the docs.** The clap `about` line ("Task manager for Linux") and per-argument help texts in E33-003, and the notification urgency (critical) in E33-010, are the writer's suggestions. Recommendation: accept as written, or supply wording.
8. **E33-OI-8. Device-loss relaunch marker.** ADR-015 says "one automatic relaunch" but not how the second process knows it is a relaunch. Recommendation: environment variable `TASKLENS_RELAUNCHED=1`, added to table (c)'s environment list. Also undecided: how GPUI device loss reaches tl-app (part of E33-OI-1).
9. **E33-OI-9. Desktop file keys and icon install path.** Docs fix only the file name, `Exec=tasklens` and the app ID. Recommendation in E33-013: `Name=tasklens`, `GenericName=Task Manager`, `Comment=See and manage processes, performance and services`, `Icon=io.github.oysteinkrog.Tasklens`, `Categories=System;Monitor;`, `Keywords=process;task;performance;monitor;`, `StartupNotify=true`, `StartupWMClass=io.github.oysteinkrog.Tasklens`, `Terminal=false`; icon installed from `assets/icons/tasklens.svg` to `/usr/share/icons/hicolor/scalable/apps/io.github.oysteinkrog.Tasklens.svg`. The autostart entry (E33-014) reuses `Type`, `Name`, `Icon`.
10. **E33-OI-10. PKGBUILD metadata.** `pkgver`, `pkgdesc`, `url`, `license`, `source` are not decided, nor whether `nvidia-utils` (listed as a runtime need in ADR-002) is a hard `depends` or an `optdepends`. Recommendation: `pkgver=0.1.0`, build from the local checkout, `nvidia-utils` as `optdepends` because the probe handles a missing NVML; licence is the owner's choice.
11. **E33-OI-11. Lavapipe ICD path.** APP-046 says "`VK_ICD_FILENAMES` set to lvp_icd" without a path. Recommendation: `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json` (Arch `vulkan-swrast`). Blocks E33-016 software mode only.
12. **E33-OI-12. How tl-ui reaches tl-app-only features.** The Settings window (tl-ui, E21) triggers Ctrl+Esc (E33-017, kglobalaccel over zbus) and "Import from TaskSlinger…" (E33-018, portal file chooser), but tl-ui cannot use zbus and has no callback for these. Recommendation: add a third `UiLaunch` field `platform: Box<dyn PlatformOps>` with `ctrl_esc_state()`, `set_ctrl_esc(bool)`, `import_taskslinger()`; or route them through `ActionKind` like R-134 did for opener actions.
13. **E33-OI-13. TaskSlinger INI encodings.** APP-032 gives key names but not the INI section names or value encodings (for example `DefaultTab`, `UIScale`, `TreeView` plus `TaskManagerGrouping`). E33-018 imports only plain booleans and in-range integers until these are supplied from `docs/taskslinger-inventory.md` or the owner's own `UserSettings.ini`.
14. **E33-OI-14. Tray wiring in `main.rs` is unassigned.** `tl-tray` must be spawned by tl-app (tl-ui cannot depend on it) and connected to `tray_bridge.rs` channels. `main.rs` chain edits are listed only for E01 and E32. Recommendation: add an E31 chain-edit bead "E31: add tray spawn to tl-app/src/main.rs" depending on E33-A1, and allow `tl-tray` in tl-app's `Cargo.toml` by the same bead (ADR-001 already lists `tl-tray (P3)` in the tl-app row).
15. **E33-OI-15. `build.rs` is not in the section 2 file list.** E33-001 creates `crates/tl-app/build.rs` for the git hash (APP-053, ADR-015). Recommendation: record it as an E33 owner-only file.

## E01 Phase 0 spike

26 beads (2 anchors updated in place, 24 created under epic tasklens-wlf). Priority 0 except E01-013 and E01-014 (gallery entry, UI-FND-030 is P1) and E01-023 and E01-024 (they need the P1 anchors E13-A2, E10-A1 and E12-A2; see E01-OI-14 and E01-OI-15). E01-019 and E01-020 are conditional on the soak failing and are closed as not needed if it passes. No bead was made for the table (c) `--gallery` doc edit (R-138 item 15): `docs/RECONCILIATION.md` table (c) already lists `--gallery`.

| Plan key | Bead ID | Title | P | Depends on |
|---|---|---|---|---|
| E01-001 | tasklens-wlf.3 | E01: add bench feature to tl-ui/Cargo.toml | 0 | E16-A1 (tasklens-fut.1) |
| E01-002 | tasklens-wlf.4 | E01: add bench feature to tl-app/Cargo.toml | 0 | E33-A1 (tasklens-cr0.1), E01-001 |
| E01-003 | tasklens-wlf.5 | E01: add mod bench to tl-ui/src/lib.rs | 0 | E16-A1 (tasklens-fut.1), E01-001 |
| E01-004 | tasklens-wlf.6 | Bench synthetic CPU, device and stats sources | 0 | E01-003, E03-A1 (tasklens-as2.1) |
| E01-005 | tasklens-wlf.7 | BenchEngine: scripted EngineClient for the bench scenes | 0 | E01-004, E03-A2 (tasklens-as2.2) |
| E01-006 | tasklens-wlf.8 | Bench frame statistics and JSON report | 0 | E01-003, E03-A1 (tasklens-as2.1), E19-A1 (tasklens-yrf.1) |
| E01-007 | tasklens-wlf.9 | Bench scenes perf-cpu, static and bridge with the run loop | 0 | E01-005, E01-006, E15-A1 (tasklens-ug9.1), E15-A2 (tasklens-ug9.2), E16-A1 (tasklens-fut.1), E17-A1 (tasklens-lgw.1), E19-A1 (tasklens-yrf.1), E19-A2 (tasklens-yrf.2), E24-A1 (tasklens-crw.1) |
| E01-008 | tasklens-wlf.10 | Bench scene table-2000 with synthetic process table | 0 | E01-007, E18-A1 (tasklens-rsf.1), E18-A2 (tasklens-rsf.2), E22-A1 (tasklens-pdu.1) |
| E01-009 | tasklens-wlf.11 | tl-app bench entry: bench_main.rs | 0 | E01-002, E01-007, E33-A1 (tasklens-cr0.1), E08-A1 (tasklens-8hz.1), E09-A1 (tasklens-8q0.1), E12-A1 (tasklens-hjn.1) |
| E01-010 | tasklens-wlf.12 | E01: add --bench arm to tl-app/src/main.rs | 0 | E33-A1 (tasklens-cr0.1), E01-009 |
| E01-011 | tasklens-wlf.13 | cargo xtask bench: three runs per scene, median and table | 0 | E02-A1 (tasklens-v9m.1) |
| E01-012 | tasklens-wlf.14 | E01: add bench arm to xtask/src/main.rs | 0 | E02-A1 (tasklens-v9m.1), E01-011 |
| E01-A1 | tasklens-wlf.1 | Bench harness and perf-cpu scene run end to end | 0 | E01-008, E01-010, E01-012 |
| E01-013 | tasklens-wlf.15 | tl-app gallery entry: gallery_main.rs | 1 | E01-002, E33-A1 (tasklens-cr0.1), E15-A3 (tasklens-ug9.3), E16-A1 (tasklens-fut.1), E17-A1 (tasklens-lgw.1) |
| E01-014 | tasklens-wlf.16 | E01: add --gallery arm to tl-app/src/main.rs | 1 | E01-013, E01-010, E33-A1 (tasklens-cr0.1) |
| E01-015 | tasklens-wlf.17 | Bench soak scene for zed#64499: three windows, 10 minutes | 0 | E01-A1 |
| E01-016 | tasklens-wlf.18 | Phase-0 results file and gate runs of the four scenes | 0 | E01-A1 |
| E01-017 | tasklens-wlf.19 | First-frame, minimized-CPU and RSS gate measurements | 0 | E01-016, E20-A1 (tasklens-jqk.1), E33-A1 (tasklens-cr0.1) |
| E01-018 | tasklens-wlf.20 | Run the zed#64499 soak and record U-9 | 0 | E01-015, E01-017 |
| E01-019 | tasklens-wlf.21 | Conditional: vendor gpui-pre-linux 0.3.7 with the zed#64499 fix | 0 | E01-018 |
| E01-020 | tasklens-wlf.22 | Conditional: E01: add gpui-pre-linux patch to root Cargo.toml and re-run the soak | 0 | E01-019, E02-A1 (tasklens-v9m.1) |
| E01-021 | tasklens-wlf.23 | Phase-0 manual checklist: CSD, monitors, lock, suspend, restore, sharpness, run return | 0 | E01-020, E20-A1 (tasklens-jqk.1), E33-A1 (tasklens-cr0.1) |
| E01-022 | tasklens-wlf.24 | Record the polkit dismissed and denied error names (U-39) | 0 | E01-021 |
| E01-A2 | tasklens-wlf.2 | Phase-0 results and decision recorded | 0 | E01-016, E01-017, E01-018, E01-019, E01-020, E01-021, E01-022, E15-A1 (tasklens-ug9.1) |
| E01-023 | tasklens-wlf.25 | GPU power baseline through the probe (report only) | 1 | E01-A2, E13-A2 (tasklens-ndy.2) |
| E01-024 | tasklens-wlf.26 | Collection-cost gates with the real engine (U-10) | 1 | E01-A2, E10-A1 (tasklens-dwy.1), E12-A2 (tasklens-hjn.2) |

### Open items

- **E01-OI-1 PRNG for the synthetic data.** UI-PRF-053 says "ChaCha8 or `fastrand` seeded"; neither crate is in `tl-ui`'s allowed row (ADR-001). Recommendation: SplitMix64 plus Box-Muller written in `bench/synth.rs`, no new dependency. Blocks E01-004.
- **E01-OI-2 Reading of the synthetic value rules.** UI-PRF-053 gives "55% +- 1 random walk", "10% +- 8", "295 +- 3", "Load average random walk" and network "0.01..5 Mbps bursts" without step sizes or burst probability. Recommendation: "X +- N" is a fresh uniform draw in [X - N, X + N] each second; random walk steps are uniform in [-1, 1] (memory) and [-0.05, 0.05] (load average, start 1.24 0.98 0.87); network bursts use the disk rule (p = 0.1 per second, else 0.01 Mbps). The bench device catalog also uses placeholder identities (16 cores x 2 threads, disk major:minor 259:0..3, ifindex 2 and 3). Blocks E01-004 and E01-005.
- **E01-OI-3 JSON shape lacks the "none above 3 x period" count.** The UI-PRF-053 shape has `missed_frames` only. Recommendation: add `long_missed_frames` (intervals above 3 x period) after `missed_frames`. Affects E01-006.
- **E01-OI-4 MB unit in `rss_mb` and `pss_mb`.** Not stated. Recommendation: 1,000,000 bytes, so the RSS gate "150 MB" compares directly.
- **E01-OI-5 `CpuPage.bench_show_both` lives in E24's `cpu.rs`.** UI-PRF-053 names the bench-only flag, but `cpu.rs` is E24 owner-only and E01 has no chain-edit right. Recommendation: E24-A1 implements the flag behind `#[cfg(feature = "bench")]` (B6 writer to add it to the E24-A1 bead). E01-007 assumes it.
- **E01-OI-6 Paint hook for bridge latency.** UI-PRF-053 measures to "the first `ChartElement::paint` that includes that sample". `ChartElement` is E19's. Recommendation: E19-A1 exposes, behind `bench`, a per-window record of (newest painted sample `t_ns`, `now_boot_ns()` at paint) that `bench/stats.rs` reads. E01-007 assumes it.
- **E01-OI-7 `table-2000` columns and synthetic rows.** ADR-014 says 10 columns (written against ADR-011's old 10-column default); R-006 now has 9 defaults. Names, group sizes and value ranges are not given. Recommendation: R-006's nine columns plus `power` (101 px); 40 App groups "App 01" to "App 40" of 50 processes each named `proc-0001` to `proc-2000`; CPU uniform 0..5%, memory 1..500 MiB, disk 0..2 MiB/s, network 0..1 Mbps, GPU 0..10%, all redrawn each second. Blocks E01-008.
- **E01-OI-8 Access to the table's scroll handle.** The scroll must drive the production table's `UniformListScrollHandle`. Recommendation: E18-A1 exposes `TlTreeTable::scroll_handle(&self) -> &UniformListScrollHandle` and E22-A1 exposes the table entity from `ProcessesPage` (B5 and B8 writers). E01-008 assumes it.
- **E01-OI-9 Process CPU clock.** ADR-014 names `/proc/self/stat` utime + stime; `tl-ui` cannot read clocks itself and `tl-app` has no `libc` for `CLK_TCK`. Recommendation: `tl-app` supplies `clock_gettime(ProcessCPUTime)` and `ThreadCPUTime` through rustix (same quantity, ns resolution) as `BenchClocks` function pointers. No allowed-row change needed. E01-006 and E01-009 are written this way.
- **E01-OI-10 Build wrapper inside `xtask bench`.** Recommendation: `xtask bench` builds through the build-rule flock and `systemd-run` command when `TL_BUILD_WRAP=1`, plainly otherwise.
- **E01-OI-11 Gallery window title.** UI-FND-030 gives none. Recommendation: "tasklens Gallery".
- **E01-OI-12 How the zed#64499 soak is launched.** ADR-010 fixes the soak (three windows, 1 Hz, 10 minutes, no input) but no command, and table (c) has no `soak` scene. Recommendation: bench scene `soak` (E01-015) with the main `perf-cpu` composition at 1 Hz plus two bench-only 600 x 300 graph windows, run with `--duration 600 --warmup 0`; add `soak` to table (c)'s scene list. Blocks E01-015.
- **E01-OI-13 Files for the conditional zed#64499 patch.** `vendor/gpui-pre-linux/` has no owner row and the root `Cargo.toml` chain-edit rule names only P3 `members` lines, but ADR-010 step 3 fixes both the directory and the `[patch.crates-io]` line. Recommendation: allow E01-019 and E01-020 as written (E01 owns `vendor/gpui-pre-linux/`, one chain edit of the root `Cargo.toml`).
- **E01-OI-14 GPU power baseline placement.** ADR-014 and open question 7 want a report-only GPU baseline in phase 0, but the probe and client are P1 (E13). Recommendation: phase-0 reports carry `gpu: null`; E01-023 (priority 1, after E01-A2, depends on E13-A2) adds the measurement and the new test file `tl-app/tests/real_gpu_baseline.rs`. Alternative: make E13-A1 and E13-A2 P0.
- **E01-OI-15 Collection-cost measurement method and timing.** ADR-014 gives the gates but no method, and they need P1 workers. Recommendation: sum utime + stime of threads named `tl-engine*` plus the probe child over 60 s; run in E01-024 (priority 1, after E01-A2), allowed by decision rule step 4.
- **E01-OI-16 First-frame log line in P0.** E01-017 reads `first_frame_ms` (UI-SHL-110 step 11). Recommendation: confirm that E20-A1 (P0) writes that log line; otherwise the first-frame gate is recorded at P1.

## E31 Tray

Epic bead `tasklens-wox`. All beads are P3 (priority 3).

| Plan key | Bead ID | Title | P | Depends on |
|---|---|---|---|---|
| E31-001 | tasklens-wox.3 | Tray contract types in tl-model (TrayModel, TrayEvent, channels) | 3 | E03-A2 (tasklens-as2.2) |
| E31-002 | tasklens-wox.4 | E31: add tl-tray to root Cargo.toml members with the crate skeleton | 3 | E02-A1 (tasklens-v9m.1), E31-001 |
| E31-003 | tasklens-wox.5 | Tray graph icon rasterizer (raster.rs, 22 and 44 px ARGB32) | 3 | E31-002 |
| E31-004 | tasklens-wox.6 | Tray dbusmenu contents for graph and main icons (menu.rs) | 3 | E31-002 |
| E31-005 | tasklens-wox.7 | ksni Tray item for graph and main icons (item.rs) | 3 | E31-003, E31-004 |
| E31-A1 | tasklens-wox.1 | Tray crate (lib.rs, model.rs: service thread, update rule, main icon rule; U-31) | 3 | E31-005 |
| E31-006 | tasklens-wox.8 | E31: add tray_bridge module to tl-ui lib.rs | 3 | E16-A1 (tasklens-fut.1) |
| E31-007 | tasklens-wox.9 | Tray bridge: graph list, 1 Hz model, tooltips, main icon rule, demand | 3 | E31-006, E31-001, E16-A1, E17-A1 (tasklens-lgw.1), E19-A1 (tasklens-yrf.1), E20-A2 (tasklens-jqk.2), E26-A1 (tasklens-xdh.1) |
| E31-A2 | tasklens-wox.2 | Tray bridge (events, add and remove actions, toasts, window actions) | 3 | E31-007, E16-A2 (tasklens-fut.2), E20-A1 (tasklens-jqk.1) |
| E31-008 | tasklens-wox.10 | E31: start the tl-tray thread from tl-app main.rs | 3 | E33-A1 (tasklens-cr0.1), E31-A1, E31-A2 |
| E31-009 | tasklens-wox.11 | E31: add close-to-tray and minimize-to-tray rows to the close decision | 3 | E20-A1 (tasklens-jqk.1), E31-A2 |
| E31-010 | tasklens-wox.12 | E31: add the Tray rows to window/settings/startup_window.rs | 3 | E21-A2 (tasklens-bj6.2), E31-A2 |
| E31-011 | tasklens-wox.13 | E31: add the tray item to pages/processes/menu.rs | 3 | E22-A2 (tasklens-pdu.2), E31-A2 |
| E31-012 | tasklens-wox.14 | E31: add Add tray icon graph to pages/minigraph/menu.rs | 3 | E26-A1 (tasklens-xdh.1), E31-A2 |

Note: E31-007 depends on E17-A1, E19-A1 and E20-A2, which are anchors outside the E31 row of 00-epics section 4 (palette constants, chart scale, `TlSettings`). All three are anchors, so no cycle; `br dep cycles` is clean.

### Open items

1. **E31-1, shared tray types have no legal home.** tl-ui (bridge) and tl-tray (service) cannot depend on each other (ADR-001; tl-tray pulls zbus through ksni), so `TrayModel`, `TrayEvent` and the channels must live in `tl-model`, which E03 owns up front with a frozen module list. Recommendation: approve E31-001 as written (new `tl-model/src/tray.rs` plus one `pub mod tray;` line), or move it into an E03 bead. Related: `raster::render` takes a `max: f32` instead of the `&mut ScaleState` in UI-PRF-050, because the scale lives in tl-ui; model channel `bounded(2)` with `try_send` (event channel `bounded(16)` is from UI-PRF-051). E31-001 is marked pending.
2. **E31-2, tl-app chain edit not listed.** Starting the tray thread needs a `tl-tray` dependency and a few lines in `tl-app/src/main.rs` (E33). Section 2 lists main.rs chain edits only for E01 and E32. Recommendation: add E31 to that row (depends on E33-A1). E31-008 is marked pending.
3. **E31-3, close decision row 1 and minimize to tray.** They belong to E31 (coverage: UI-SHL-113 "E31 row 1 and tray") but the close decision is in E20's `window/main.rs` and the Minimize handler in `window/titlebar.rs`, both owner-only. Recommendation: add a chain-edit rule for E31 there (depends on E20-A1), or move E31-009's deliverable into an E20 bead that depends on E31-A2. E31-009 is marked pending.
4. **E31-4, main tray icon pixels.** UI-SHL-113 says "the app icon at 22 x 22 and 44 x 44 ARGB32", but the app icon is an SVG and neither tl-tray nor tl-app can rasterize SVG. Recommendation: commit two pre-rendered raw ARGB32 files `assets/icons/tray-22.argb` and `tray-44.argb` (rendered once from `assets/icons/tasklens.svg`, chain edit to E15's assets folder) and `include_bytes!` them in tl-app. Alternative: SNI `IconName` with the installed hicolor icon name. E31-A1 takes the pixels as a parameter, so only E31-008 waits on this.
5. **E31-5, add-tray-graph action plumbing.** (a) `PerfAddTrayGraph { source }` appears in E24's card and page menus and E26's card menu but has no declared owner; recommendation: declare it in `tray_bridge.rs` (E31-A2, done that way) and have E24's P3 menu items (UI-PRF-014 item 5, UI-PRF-015 item 8) depend on E31-A2; E24 must confirm it builds those items. (b) processes.md names `AddTrayGraph { column }` / `RemoveTrayGraph` read from `MenuTargets`, which needs a handler in `pages/processes/mod.rs` where E31 has no chain edit; recommendation (used in E31-011): build the source when the menu is built and dispatch `AddTrayGraph { source, column }` / `RemoveTrayGraph { source, column }`. (c) Row kind to source: process row `process:`, app row `app:<id>`, unit row `app:<unit name>`; the "name filter" tray mode (PROC-094) has no entry point in the row menu. Please confirm.
6. **E31-6, accent label in the tray Graph color menu.** UI-PRF-051 says "Accent"; UI-SHL-085 says the tray variant shows "the same names" as the in-app menu, whose first row is "Accent (theme default)". E31-004 uses "Accent (theme default)"; one string to change if the leader picks "Accent".
7. **E31-7, tray toasts while the main window is hidden.** UI-PRF-051 says they show on the next render; UI-FND toast rule 8 says Errors go to a desktop notification and the rest are dropped, nothing replayed. E31-A2 pushes through `ToastQueue` and lets rule 8 decide. Recommendation: keep rule 8 and drop the UI-PRF-051 sentence.
8. **E31-8, tray demand and tooltip gaps.** (a) `Demand.tray: TrayDemand` (ADR-008) has no fields defined anywhere; E03 must define it. Recommendation: `TrayDemand { visible: bool }` (the tray counts as a visible surface) with the tray's sources expressed through the normal `Demand` fields and the same process-identity mechanism the minigraph process cards use. (b) Tooltip forms for `vram` and `gpu_temp` page graphs are not in the UI-PRF-051 table; E31-007 uses `VRAM Usage` with the size value and `GPU temp` with `52 °C`. (c) Main tooltip "Disk" and "GPU" metrics are not defined; E31-007 uses busiest disk active time and the first GPU in sidebar order (R-085). (d) APP-047 and UI-SHL-113 show the main tooltip on one line; PERF-089 and UI-PRF-051 say five lines; E31-007 uses five lines joined by `\n`.
9. **E31-9, minigraph source resolver API.** E31-007 resolves tray graph sources with the resolver in `pages/minigraph/sources.rs` (E26-A1). Its function names are not fixed in the UI docs. The B6 writer should name them in E26-A1 so E31-007 can cite them; E31-007 then needs a one-line update.
10. **E31-10, tray availability before any item exists.** "No system tray available" needs to know whether a `StatusNotifierWatcher` is on the bus even when no tray item is registered, but tl-tray learns that only through ksni callbacks of a registered item, and may not use zbus directly. E31-007 assumes "available" until the first watcher event (a failed registration then shows "Unable to add tray icon graph"). Recommendation: accept, or allow `zbus` in tl-tray's ADR-001 row for a `NameHasOwner` and `NameOwnerChanged` watch.
11. **Not mine, noted.** The Settings menu's "Window & startup" tray check items (shell.md menu table) are E21's own P3 items in `menus_settings.rs`; E31 does not bead them.

