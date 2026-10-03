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

