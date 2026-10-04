# Engine-side open items, settled

Written 2026-10-04 for the bead round. Covers the open items of batches B1 (E01, E02, E03, E31,
E32, E33), B2 (E04 to E07), B3 (E08 to E11, E13) and B4 (E12, E14, E29, E30), plus the cross-side
topics the leader assigned to the engine side: device commands and records, the udev data source,
ProcessRecord extras, the crate and `unsafe` allow-list, gcore and dump, the helper hook, the PSI
trigger window, `write_atomic`, the `UiLaunch` shape, per-process series and aggregate devices.
The UI-side file (`open-items-ui.md`) refers to the OI-E IDs below for those topics.

Rules used: DECISIONS.md, then RECONCILIATION.md, then ARCHITECTURE.md, then the specs and plan
files. Beads were read with `br show --json`; no bead was changed. Evidence tags: [verified] read
in a doc, bead or crate source on disk; [inferred] follows from a doc or from the crate manifest
without a direct statement; [guess] no source. Bead IDs are `tasklens-<key>`; the prefix is left
out below. Every "Changes" line is for the leader to apply.

Where an item says "accept", the batch writer's recommendation holds as written in the named
beads and only the listed doc edits are needed.

## Topics

### OI-E01 Device commands, device details and the public IP state
- Raised by: E12-OI-1 (A to F), E25-OI-1, E28 item 1 (blocks 9o0.32), E03 contracts item 10,
  B7 item 11 (E20-024), E12-A3 placeholder question, E03 first-half item 9 (no `SourceId` for the
  public IP).
- Question: which `Command` variants carry SMART, smartctl, eject, public IP and battery history
  requests, how device details and the eject progress reach the UI, where the public IP state
  lives, and whether E12-A3 still holds placeholder text.
- Answer:
  1. E12-A3 (hjn.3) is a full bead with context, deliverable, criteria and dependencies, not a
     placeholder [verified]. E25-OI-1's sentence about it is stale.
  2. One command enum, the E12-OI-1 part E shape. In `tl-model/src/command.rs` (E03-063):
     `Command::Device(DeviceCommand)` with
     `DeviceCommand { SmartRead { disk: Arc<str>, update: bool }, SmartReadAdmin { disk: Arc<str> }, Eject { seq: u64, disk: Arc<str> }, FetchPublicIp, BatteryHistory { native_path: Arc<str> } }`.
     `disk` is the kernel name (`sda`, `nvme0n1`), the same string as `DeviceIdentity::Disk.name`.
     The UI resolves a `DeviceIdx` to that name through `DeviceCatalog`; workers never see a
     `DeviceIdx`, which is per session (ADR-003). `seq` is a UI counter so a page can tell its own
     eject from an older one. The E25-OI-1 names map one to one: `SmartUpdate(idx)` is
     `SmartRead { disk, update: true }`, `SmartReadSmartctl(idx)` is `SmartReadAdmin { disk }`,
     `Eject(idx)` is `Eject { seq, disk }`, `FetchPublicIp` is `DeviceCommand::FetchPublicIp`.
     The exhaustive `command_variants_exhaustive` test in E03-063 goes from 18 to 19 variants.
  3. Routing in the reducer (E09 apply.rs, 8q0.9): `SmartRead`, `SmartReadAdmin`, `Eject` go to
     `forward(SourceId::Udisks, WorkerCommand::Device(cmd))`; `FetchPublicIp` to
     `SourceId::Inventory`; `BatteryHistory` to `SourceId::UPower`. `WorkerCommand` (E08-004,
     8hz.6) gains `Device(DeviceCommand)`. `SourceId` already has `Udisks = 16`, `UPower = 17`,
     `Inventory = 14` [verified in as2.13]. No `SourceId::PublicIp`: the fetch is not a supervised
     source, it is a job the Inventory worker runs on request (E03 first-half item 9 holds).
  4. Public IP state stays `InventorySnapshot.public_ip: PublicIpState` exactly as E03-056
     (as2.22) writes it: `NotFetched | Fetching | Fetched { v4: Option<Arc<str>>, v6: Option<Arc<str>>, fetched_at_s: i64 } | Failed(Arc<str>)`.
     E28 item 1's `SystemSnapshot.public_ip` is not added; the Information tab and the Network
     page both read `snapshot.inventory.public_ip`. The UI entry point `request_public_ip_fetch`
     (E25, o55.22) sends `Command::Device(DeviceCommand::FetchPublicIp)` after the notice.
  5. Startup auto-fetch is UI-driven. E12-OI-1 part F (`EngineSettings.auto_fetch_public_ip`) is
     dropped. Reason: the fetch may only run after the notice was accepted, and
     `network.public_ip_notice_accepted` is a UI setting the engine does not see. E20-024
     (jqk.26) sends `Command::Device(DeviceCommand::FetchPublicIp)` once at startup when both
     `network.auto_fetch_public_ip` and `network.public_ip_notice_accepted` are true. E12-019
     (hjn.22) drops its "at worker start" step and the `Arc<AtomicBool>` flag; the rest of that
     bead stands (`ureq` 3, rustls, 5 s, 64-byte body, no redirects).
  6. Device details (part C). In `tl-model/src/snapshot.rs` a new component
     `DeviceDetailsSnapshot { header: ComponentHeader, cpu: Arc<CpuStaticInfo>, memory: Arc<MemoryDetails>, disks: Arc<[(DeviceIdx, DiskDetails)]>, nets: Arc<[(DeviceIdx, NetDetails)]>, batteries: Arc<[(DeviceIdx, BatteryDetails)]> }`,
     field `Snapshot.device_details: Arc<DeviceDetailsSnapshot>`, mask bit
     `ComponentMask::DEVICE_DETAILS = 1 << 14` (bit 13 is taken by OI-E16's `PROCESS_SERIES`),
     `ALL = 0x7FFF`. Worker-side records are the same structs without the `DeviceIdx`
     (`Vec<(DeviceIdentity, DiskDetails)>` and so on); the reducer sets the index.
     `DiskDetails` fields: `model, wwn, serial, interface: InvField<Arc<str>>`,
     `rotation_rpm: Option<u32>`, `scheduler: InvField<Arc<str>>` (the block scheduler name,
     E22 item 1), `smart: SmartKind { Ata, Nvme, NeedsFallback, None }`,
     `smart_data: Option<InvField<Arc<SmartData>>>` (`None` never requested; `WarmingUp` while
     reading), `smart_temp_c: Option<f32>`, `ejectable: bool`, `can_power_off: bool`,
     `eject: Option<(u64, EjectState)>`, `partitions: Arc<[PartitionRow]>` (OI-E02),
     `formatted_bytes, used_bytes, free_bytes: InvField<u64>`, `system_disk: InvField<bool>`,
     `swap: InvField<bool>`.
     `EjectState { Running, Blocked { blockers: Arc<[Blocker]>, others_may_hold: bool }, Done, Failed(Arc<str>) }`;
     E25-OI-1's `Idle` is `eject == None`, and its `BlockerGroup` is built in the UI by grouping
     `Blocker { key: ProcessKey, name: Arc<str>, app: Option<Arc<str>>, paths: Arc<[Arc<str>]> }`
     on `app` (the reducer fills `app` from the process grouping, E12-022).
     `SmartData { updated_s: i64, health: InvField<Arc<str>>, power_on_s: InvField<u64>, selftest: InvField<Arc<str>>, ata: Arc<[SmartAttr]>, nvme: Arc<[(Arc<str>, Arc<str>)]> }`,
     `SmartAttr { id: u8, name: Arc<str>, value: i64, normalized: i32, threshold: i32, worst: i32, prefail: bool, online: bool, assessment: Arc<str> }`
     (values and texts as E12-021, hjn.24).
     `NetDetails { adapter_name, driver, mac: InvField<Arc<str>>, ifindex: u32, link_speed_mbps: InvField<u32>, state: LinkState, addresses: Arc<[Arc<str>]>, kind: NetKind, wifi: Option<WifiInfo> }`,
     `WifiInfo { ssid: Arc<str>, signal_dbm: i32, signal_pct: u8, rx_mbps: u32, tx_mbps: u32, freq_mhz: u32, channel: u16, security: Arc<str> }`.
     `BatteryDetails` exactly the field list of E12-023 (hjn.26) plus
     `history: Option<Arc<[(i64, f32)]>>` (E12-029).
     `CpuStaticInfo` and `MemoryDetails` carry the PERF-036 to PERF-046 static rows as
     `InvField<T>`; the E12-010 and E12-011 beads list them and E03 copies those lists.
  7. Transport (part D). `SourceUpdate::DeviceDetails(DeviceDetailsUpdate)` in E08-003 (8hz.5)
     with `DeviceDetailsUpdate { Cpu(Arc<CpuStaticInfo>), Memory(Arc<MemoryDetails>), Disks { from: SourceId, entries: Vec<(DeviceIdentity, DiskDetails)> }, Nets { from: SourceId, entries: Vec<(DeviceIdentity, NetDetails)> }, Batteries(Vec<(DeviceIdentity, BatteryDetails)>) }`.
     The reducer keeps one record per device and merges by field: the sysfs source
     (`Disk`, `Net`) fills every field it knows; a D-Bus source (`Udisks`, `NetworkManager`)
     overwrites `model`, `wwn`, `serial`, `rotation_rpm`, `smart*`, `ejectable`,
     `can_power_off`, `eject`, `kind`, `wifi`. A full list from a source replaces that source's
     contribution for every device; a device missing from the list loses that source's fields.
     `BATTERY_PCT` is appended from `BatteryDetails.percentage` in the `Batteries` arm.
  8. Device lists next to samples (part A). `Source` gains
     `fn take_extra(&mut self) -> Option<SourceUpdate> { None }`; `source::run` drains it after
     each `sample()` and sends those updates before the sample. E08-004 (8hz.6) edit; E12-004
     (hjn.7) is written to it.
  9. Totals fields (part B) in E03-009 (as2.11): `DiskTotals.temp_c: Metric<f32>`,
     `NetTotals.state: LinkState`, `SensorReading.fan_control: Option<FanControlMode>`;
     `LinkState { Up, Down, Dormant, Unknown }` from sysfs `operstate` [inferred],
     `FanControlMode { FullSpeed, Manual, Automatic }` from `pwm<n>_enable` 0, 1, 2 [inferred].
- Basis: ADR-003 (component list, `DeviceIdx` per session) [verified]; ADR-005 `Command` list
  [verified]; PERF-051, PERF-052, PERF-060, PERF-078, PERF-079, APP-056 [verified]; E03-056 and
  E03-063 bead text [verified]; `SourceId` numbers in as2.13 [verified]; E12-019 to E12-029 bead
  text [verified]. The `operstate` and `pwm_enable` value sets are kernel ABI [inferred].
- Owner decision: no.
- Changes:
  - ADR-003: add `device_details` to the `Snapshot` component list and `DEVICE_DETAILS` to the
    mask; add the `DiskTotals`, `NetTotals`, `SensorReading` fields above.
  - ADR-005 "UI to engine": add `Command::Device(DeviceCommand)` with the five variants.
  - ADR-004 `Source` trait: add `take_extra`.
  - as2.29 (E03-063): add `DeviceCommand` and the variant; test count 19.
  - as2.16 (E03-050): `DEVICE_DETAILS = 1 << 14`, `PROCESS_SERIES = 1 << 13`, `ALL = 0x7FFF`,
    fifteen bits.
  - as2.22 (E03-056): append the `DeviceDetailsSnapshot` block above (same file, same owner).
  - as2.30 (E03-064): the `Snapshot` struct gains `device_details` and `process_series`.
  - as2.11 (E03-009): the three part-B fields and the two enums.
  - as2.33 (E03-067): no `auto_fetch_public_ip` field.
  - 8hz.5 (E08-003): `SourceUpdate::DeviceDetails(..)` and `WorkerCommand::Device(..)`.
  - 8hz.6 (E08-004): `take_extra` on `Source` and the drain in `run`.
  - 8q0.9 (E09 apply): the `DeviceDetails` arm, the `BATTERY_PCT` append and the three
    `forward` targets for `Command::Device`.
  - hjn.22 (E12-019): drop the start-up auto fetch and the `AtomicBool`; keep the command path.
  - hjn.23 to hjn.26, hjn.32, hjn.33, hjn.7: remove the `blocked-open-item` label; the types
    above are the contract.
  - jqk.26 (E20-024, UI side): sends `Command::Device(DeviceCommand::FetchPublicIp)` under the
    two settings; o55.22 and 9o0.32: read `inventory.public_ip`, send the same command.
  - b6-ui-performance.md E25-OI-1 and b8-ui-tables.md E28 item 1: point at this entry.

### OI-E02 udev data source and the Partitions table
- Raised by: E04-OI-2, E04-OI-13, E04-OI-11, E12-OI-4, E04-OI-1 (bead number clash).
- Question: how disk model, serial, WWN, bus, filesystem labels and adapter names are read without
  libudev, and who builds the PERF-050 Partitions table.
- Answer:
  1. New tl-collect file `crates/tl-collect/src/parse/udev.rs`, owner E04, new bead E04-030
     "udev database reader" (P1, so the P1 disk persistent key works):
     `pub fn read_udev_props(path: &Path, keys: &[&str], out: &mut UdevProps) -> Result<(), CollectError>`
     reads one `/run/udev/data/<entry>` file (64 KiB cap), keeps only `E:KEY=value` lines whose
     key is in `keys`, and stores them in `UdevProps(SmallVec<[(Arc<str>, Arc<str>); 8]>)`.
     Entry names: `b<major>:<minor>` for block devices, `n<ifindex>` for net devices,
     `+pci:<slot>` for PCI devices (GPU names) [inferred from the udev database layout].
     Keys read: `ID_MODEL`, `ID_MODEL_FROM_DATABASE`, `ID_VENDOR_FROM_DATABASE`, `ID_BUS`,
     `ID_WWN`, `ID_SERIAL`, `ID_SERIAL_SHORT`, `ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`,
     `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`, `ID_PART_ENTRY_OFFSET`, `ID_PART_ENTRY_SIZE`.
     Values are untrusted text and go through `sanitize` before display.
  2. `SysRoot` gains `run: PathBuf` (default `/run`), so fixtures can carry a scrubbed
     `run/udev/data/` directory. ADR-013 scrub list gains `ID_SERIAL`, `ID_SERIAL_SHORT`,
     `ID_WWN` values (replaced by `SCRUBBED`) and MAC addresses (E04-OI-3).
  3. Consumers: E04-021 (2dl.24) uses `ID_BUS=usb` for the USB kind and `ID_MODEL` for the model
     when present, else the sysfs rules it has now; the disk persistent key is
     `disk:<ID_WWN or ID_SERIAL>` with the by-path fallback (PERF-005). E12-013, E12-014,
     E12-018 and E12-026 depend on the new bead and read through it.
  4. Partitions: new bead E04-031 `crates/tl-collect/src/parse/partitions.rs` (P2):
     `PartitionRow { name: Arc<str>, start_bytes: u64, size_bytes: u64, holder: Option<Arc<str>>, mount: Option<Arc<str>>, extra_mounts: Arc<[Arc<str>]>, label: Option<Arc<str>>, fs_type: Option<Arc<str>>, usage_pct: InvField<f32>, flags: Arc<[Arc<str>]>, type_name: Arc<str>, read_only: bool, unallocated: bool }`
     and `PartitionTable { kind: Arc<str> /* "GPT" | "MBR" | "none" */, rows: Arc<[PartitionRow]> }`,
     ordered by `start_bytes`, gaps over 1 MiB as `unallocated` rows, GPT GUID name table and
     attribute flags exactly as PERF-050, MBR types as `MBR 0x%02X`. Depends on 2dl.24,
     2dl.27 and E04-030. The row lands in `DiskDetails.partitions` (OI-E01).
  5. New anchor E04-A4 "P2 system details" closing E04-013, E04-014, E04-015, E04-019,
     E04-023, E04-024, E04-028 and E04-031 (E04-OI-11), so E12 P2 beads and o55.17 point at an
     anchor. The `minimal-vm` fixture bead (E04-OI-1) is E04-032.
- Basis: PERF-047, PERF-049, PERF-050, PERF-056, PERF-005 [verified]; ADR-016 `SysRoot { proc, sys }`
  [verified]; ADR-001 (no libudev crate) [verified]; udev database file layout [inferred].
- Owner decision: no.
- Changes:
  - ADR-016: `SysRoot { proc, sys, run }`; ADR-013: `run/udev/data` in fixtures and the scrub
    additions; 00-epics section 2: `parse/udev.rs` and `parse/partitions.rs` under E04.
  - New beads: E04-030 (P1, files `parse/udev.rs`, `parse/mod.rs` line, fixture
    `run/udev/data/`; depends on 2dl.1), E04-031 (P2, as above), E04-A4 (anchor), E04-032
    (minimal-vm, depends on 2dl.1).
  - 2dl.1 (E04-A1): `SysRoot.run`. 2dl.24 (E04-021): udev-first kind and model rule.
  - hjn.16, hjn.17, hjn.21, hjn.29: depend on E04-030; hjn.16 also on E04-031.

### OI-E03 ProcessRecord extras, group rows and table fields
- Raised by: E05 item 1, B3 item 5, E22 item 1, E03 first-half items 5 and 6, E03 contracts
  item 7, E05 item 2, B3 item 6.
- Question: where the P1 extra-column values, the app cgroup values and the table-only fields
  live in tl-model, and how GroupKind, NodeId and GroupRow relate.
- Answer:
  1. `ProcessRecord` (E03-008, as2.10) stays as written. Add to the same file
     `pub struct ProcessExtras` and `ProcessSnapshot.extras: Arc<[ProcessExtras]>`, index
     aligned with `records` (same length, same order). Fields:
     `cpu_kernel_pct: f32, cpu_user_pct: f32, cpu_relative_pct: f32, utime_ticks: u64, stime_ticks: u64, start_ticks: u64, rss_peak_bytes: u64, rss_shared_bytes: u64, commit_bytes: u64, vsize_bytes: u64, vsize_peak_bytes: u64, page_faults: u64, page_faults_delta: u64, major_faults: u64, policy: SchedPolicy, rt_prio: u8, affinity: CpuSet, handles: Option<u32>, exe_name: Arc<str>, exe_path: Arc<str>, exe_deleted: bool, unit: Option<UnitRef>, state_letter: u8, frozen: bool, disk_read_bps: f32, disk_write_raw_bps: f32, flag_since_ns: TimeNs`.
     `priority` and the State column text are formatted in the UI from `policy`, `nice`,
     `rt_prio`, `state_letter` and `frozen` (E05 item 2: `ProcState` stays). Start time is
     `btime + start_ticks / CLK_TCK`, formatted by the UI from `SystemSnapshot.boot_time_s`
     (new `SystemSnapshot` field, filled from `/proc/stat btime`).
  2. `ProcessSnapshot.apps: Arc<[AppCgroupSample]>` with
     `AppCgroupSample { group: GroupKey, anon_bytes: u64, swap_bytes: u64, io_read_bps: f32, io_write_bps: f32, rios: u64, wios: u64, pids: u32, cgroup: CgroupRef, validity: FieldValidity }`.
     Both types move from tl-collect into tl-model; tl-collect's sweep fills them
     (E05-A1, yms.1) and E10's worker ships them in `ProcessSweep.extras` and
     `ProcessSweep.app_stats`.
  3. Grouping: `GroupKind` keeps its four classification variants (App, Background, System,
     Ungrouped, PROC-004). Table identity is `NodeId`, which gains `Unit(UnitRef)`,
     `KernelThreads` and `Pseudo(PseudoKind { Interrupts, MemoryCompression })` (E03 first-half
     item 5, ADR-009 edit). `GroupRow` gets `kind: GroupRowKind { App, Unit, KernelThreads, Pseudo }`
     and the fields B3 item 5 lists:
     `{ node: NodeId, kind, name: Arc<str>, icon: Option<Arc<str>>, cpu_pct, memory_bytes, disk_bps, net_bps, gpu_pct, pids: u32, note: Option<Arc<str>>, validity: FieldValidity }`
     plus the numeric mirrors E03-008 already has.
  4. Table: `TableRow.descendants: u32`, `TableRow.subtree: Option<u32>`,
     `TableQuery.include_children: bool`,
     `TableTotals { cpu_pct, memory_pct, swap_pct, disk_pct, net_bps, net_link_bps: Option<u64>, gpu_pct, vram_pct, threads, handles }`
     one `Total<T>` each (E03 contracts item 7 and PROC-038), and
     `ProcessTableView { total: u32, shown: u32, restricted: u32 }` counts.
     `SeriesSet.slow: Arc<[Arc<SeriesView>]>` for the 1 Hz tier.
  5. Capability bits: `HealthSnapshot.caps: EngineCaps` (bitflags u16):
     `FILE_MANAGER1 = 1`, `GDB = 2`, `CONNECTIONS_DESTROY = 4`, `KWIN_BRIDGE = 8`. The engine
     sets `GDB` from `tl_ops::dump::dump_available()` at start, `FILE_MANAGER1` when the opener
     finds an owner, `CONNECTIONS_DESTROY` from `SourceUpdate::ConnectionsDestroySupport`.
     Unit `MainPID` is a field of the services unit record (E14-001).
  6. B3 item 6: columns whose data is not in the record or extras sort `Absent`. With the extras
     above the list shrinks to the P2 and P4 package, file and security columns and the
     visible-row-only columns (`pss`, `uss`, `ctx_switches*`, `cycles*`, `windows`,
     `x11_resources`), which matches R-007 and PROC-047.
- Basis: PROC-027 to PROC-038 column tables [verified]; ADR-003 `ProcessRecord` [verified];
  E05-010 `SweptProc` fields (yms.13) [verified]; B3 item 5 and E22 item 1 lists [verified].
- Owner decision: no.
- Changes:
  - ADR-003: `ProcessExtras`, `AppCgroupSample`, `ProcessSnapshot.extras` and `.apps`,
    `GroupRow` fields, `TableTotals`, `EngineCaps`; ADR-009: the three `NodeId` variants.
  - as2.10 (E03-008): the two new types, `GroupRowKind`, `NodeId` variants.
  - as2.16 (E03-050): `ProcessSnapshot.extras`, `.apps`; `SystemSnapshot.boot_time_s: i64`;
    `SeriesSet.slow`.
  - as2.17 (E03-051): `TableRow.descendants`, `.subtree`, `TableQuery.include_children`,
    `TableTotals` fields, the three counts.
  - as2.13 (E03-011): `HealthSnapshot.caps: EngineCaps`.
  - yms.1 (E05-A1): produce the tl-model types instead of its own.
  - 8hz.5 (E08-003): `ProcessSweep.extras: Vec<ProcessExtras>`; `app_stats` typed as
    `Vec<AppCgroupSample>`.
  - 8q0.7, 8q0.11 to 8q0.15 (E09 table beads): read `extras` and `apps`.
  - b8-ui-tables.md E22 item 1: point here.

### OI-E04 Crate pins and allowed-dependency rows
- Raised by: B1 item 2, E33-OI-2, E33-OI-3, E03 first-half items 4, 12 and 14, E03 contracts
  item 2, E05 items 3, 4 and 5, E06 items 1, 2, 7 and 11, E12-OI-2, E14 item 3, E32 OI-5,
  E01-OI-1, B5 item 22, E05 item 12.
- Question: which crates and features the ADR-001 rows and ADR-002 pins must gain.
- Answer, per crate (checked against `~/.cargo/registry/src/index.crates.io-*`):
  1. `clap`: pin `clap = { version = "4", default-features = false, features = ["std", "derive", "help", "usage", "error-context"] }`
     in ADR-002; add to the tl-app row. E33-001 (cr0.4) records the exact resolved version.
  2. `async-channel`: add to the tl-app row (ADR-010 `bounded(4)` for `AppRequest`) and to the
     tl-ui row (B5 item 22; the tray channels and `AppRequest` receiver). Pin exists
     (`2.5.0`) [verified].
  3. `rustc-hash`: add to the tl-model row. `GroupKey` is an FxHash of `GroupKind`
     (ADR-003), so the hash must be computed in tl-model. The E03 beads that chose
     `std::collections::HashMap` for `DetailsSnapshot` and `ServicesSnapshot.details` keep it
     (maps of at most 256 entries, E03 contracts item 2 accepted).
  4. `bitflags`: pin becomes `bitflags = { version = "2", features = ["serde"] }`. bitflags
     2.13.2 has `serde = ["serde_core"]` [verified].
  5. `rustix`: pin features become
     `["fs", "process", "net", "event", "system", "thread", "time", "pipe", "mm", "param"]`.
     rustix 1.1.5 has both features; `rustix::mm::mlockall(MlockAllFlags)` is a safe function
     (`src/mm/mmap.rs:408`), so E32 needs no `unsafe`; `rustix::param::clock_ticks_per_second()`
     and `page_size()` exist [verified]. tl-model keeps `time` only.
  6. `ureq`: already in the tl-engine row and pinned `3` with `default-features = false,
     features = ["rustls"]`. ureq 3.4.0's manifest has no `tokio` dependency (U-30 answered
     [inferred from the manifest; `cargo xtask check-deps` in hjn.22 confirms]). E12-019 adds the
     `Cargo.toml` line as a chain edit on E08-A1 (E12-OI-2 accepted).
  7. `serde_json`: add to the tl-engine row (KWin `Update` payload, `journalctl -o json`,
     `kscreen-doctor -j`, smartctl JSON). E14-A4 (y6k.4) adds the dependency line as the chain
     edit; E12-028 and E14-009 use it. Pin exists (`1`).
  8. PRNG: none. E01-004 (wlf.6) writes SplitMix64 plus Box-Muller in `bench/synth.rs`
     (E01-OI-1 accepted). UI-PRF-053's "ChaCha8 or fastrand" sentence gets "or an in-tree
     SplitMix64".
  9. Time zone: add `jiff = { version = "0.2", default-features = false, features = ["std", "tz-system", "tzdb-zoneinfo"] }`
     to ADR-002 and to the tl-ui row. jiff 0.2.35 on Linux depends only on `jiff-static`,
     `jiff-tzdb`, `log` and `serde_core`, reads `/etc/localtime` and the system zoneinfo, no
     libc [verified in the manifest]. `chrono` pulls `iana-time-zone` and `time` with
     `local-offset` pulls `libc`, so neither fits. tl-model's `format_local(LocalDateTime)`
     (E03-013) stays; tl-ui converts with jiff.
  10. `drm` (Smithay drm-rs) for KMS display modes (E06 item 1): not in v1. The crate is not
      on this machine, so its API cannot be checked, and PERF-072's current mode and refresh
      come from `kscreen-doctor -j` on KDE (E12 worker) and from sysfs `modes` elsewhere. The
      KMS path is a P4 item: when it starts, add `drm` to the tl-collect row rather than a new
      `unsafe` file.
  11. `x11rb`: P4 only (XRes column, U-32). Decide the row when P4 starts; no change now.
  12. `libc` in tl-collect: no. User names come from parsing `/etc/passwd` (E05 item 4
      accepted; NSS-only users show the numeric uid). `getnameinfo` (E06 item 3, E14 item 13)
      goes into `tl-ops/src/sys.rs` when P4 starts; no bead now.
  13. `flate2` for the pacman mtree check (E05 item 5): no. The "Verify against package" check
      runs `pacman -Qkk <pkg>` and reads its exit status and lines; no gzip crate.
  14. tl-helper row gains `tl-collect` (E29-OI-7), so the strings scanner is shared; see
      OI-E19.
  15. Dev-dependencies (`insta`, `proptest`, `tempfile`, `criterion`, `toml`,
      `tracing-subscriber`) are outside the allowed rows; `xtask check-deps` (E02-008, v9m.10)
      checks `normal` edges only (E03 contracts item 23). E05 item 12's three `Cargo.toml`
      lines (the `[[bench]] sweep` block, `criterion`, `tracing-subscriber` dev-dep) are
      allowed as E05 chain edits on the E04-owned file.
- Basis: ADR-001 rows and ADR-002 pins [verified]; crate manifests on disk [verified];
  APP-052 (clap derive) [verified]; UI-PRF-053 [verified].
- Owner decision: no.
- Changes:
  - ADR-001 rows: tl-app + `clap`, `async-channel`; tl-ui + `async-channel`, `jiff`;
    tl-model + `rustc-hash`; tl-engine + `serde_json`; tl-helper + `tl-collect`.
  - ADR-002 `[workspace.dependencies]`: `clap`, `jiff` lines; `bitflags` and `rustix` feature
    lists as above.
  - v9m.4 (E02-002): the pin edits. v9m.10 (E02-008): the row edits and the dev-dep rule.
  - cr0.4 (E33-001): `clap`, `async-channel` lines. fut.4 (E16-001): `async-channel`, `jiff`
    lines. y6k.4 (E14-A4): `serde_json` line. hjn.22: `ureq` line (as written).
  - yms.26 (E05-023), yms.1: the three chain-edit lines are allowed as written.
  - b2-collect-ops.md E06 item 1: KMS deferred to P4 with `drm`.

### OI-E05 `unsafe` in tl-gpu-probe for the API versions mode
- Raised by: B3 item 13 (blocks E13-006, ndy.8).
- Question: add `crates/tl-gpu-probe/src/api_versions.rs` to the ADR-001 rule 4 list, or drop the
  mode.
- Answer: add the file. `ash` entry points are `unsafe fn` by design and `khronos-egl`'s dynamic
  instance needs raw calls for `eglGetProcAddress`; no safe wrapper exists. The file is in the
  probe process, which already runs apart from the engine so a driver crash costs only two rows
  (ADR-007). Rule 4 list becomes four files: `crates/tl-ops/src/sys.rs`,
  `crates/tl-engine/src/gpu/spawn.rs`, `crates/tl-app/src/main.rs`,
  `crates/tl-gpu-probe/src/api_versions.rs`, each with `#![allow(unsafe_code)]` and a `// SAFETY:`
  comment per block. ndy.8 is written to this answer and is unblocked. Alternative: drop the
  mode and show `Unknown` for Vulkan and OpenGL (loses PERF-071 rows on every machine).
- Basis: ADR-001 rule 4 [verified]; ADR-007 API versions mode and R-087 [verified]; ndy.8 text
  [verified]; ash and khronos-egl API shape [inferred, crates not on disk].
- Owner decision: no.
- Changes: ADR-001 rule 4: the fourth file. ndy.8: remove the blocked note. 00-epics section 2
  lists `api_versions.rs` under E13 already.

### OI-E06 Other `unsafe` or libc needs outside the allowed files
- Raised by: E07 note after O6 (`ioprio_get`), E05 item 6 (`process_vm_readv`), E06 item 3 and
  E14 item 13 (`getnameinfo`), E06 item 4 (`perf_event_open`), E01-OI-9 (process CPU clock).
- Question: how readers that need a syscall without a safe rustix wrapper get it.
- Answer:
  1. `ioprio_get`: E07-005 (iuz.8) makes `tl_ops::sys::ioprio_get_tid(tid) -> OpsResult<IoPriority>`
     `pub` (not `pub(crate)`), re-exported as `tl_ops::process::io_priority_of(key, boot)`
     with the identity check. The I/O priority column (PROC-036, P2) and the check mark in the
     menu read it from the engine (E10 detail worker), never from tl-collect.
  2. Strings scan memory read: `pread` on `/proc/<pid>/mem` through `rustix::fs::pread`
     (E05 item 6 accepted). Same ptrace access rule as `process_vm_readv`; PROC-084's
     mechanism line changes to `/proc/<pid>/mem`. The scanner stays in tl-collect
     (`proc/strings.rs`) and tl-helper reuses it (OI-E04 item 14).
  3. `getnameinfo` (reverse DNS, SYS-035, P4): a `tl_ops::sys::reverse_lookup` wrapper when P4
     starts. No bead now; E14-015 (y6k.19) stays P4 and gets its edge then.
  4. `perf_event_open` (Intel PMU, PERF-075; Cycles column, PROC-029 P4): not in v1; fdinfo
     only (E06 item 4 accepted).
  5. Bench clocks: tl-app passes `rustix::time::clock_gettime(ClockId::ProcessCPUTime)` and
     `ThreadCPUTime` as function pointers (E01-OI-9 accepted); `tl_model::time::thread_cpu_ns()`
     (E19 OI-4) is the same call behind tl-model's `rustix` `time` feature, so tl-ui can use it
     too.
- Basis: ADR-001 rule 4 and tl-ops row [verified]; rustix 1.1.5 has `fs::pread` and
  `time::clock_gettime` with `ProcessCPUTime` and `ThreadCPUTime` [verified].
- Owner decision: no.
- Changes: iuz.8: `pub` visibility and the `process.rs` wrapper (iuz.7 or iuz.8, same anchor).
  PROC-084 mechanism line. ADR-016 "strings scan" row if it names `process_vm_readv`.

### OI-E07 Core dump tool and the passed file descriptor
- Raised by: E07-O1 (blocks E07-012, iuz.15, and so E07-A3), E29-OI-2, E03 contracts item 19
  (Dump fd part), B3 item 16 (a).
- Question: `gcore -o` cannot write to an fd or to the exact path the UI polls; how does the
  dump reach the UI's file, directly and through the helper.
- Answer:
  1. The tool is `/usr/bin/gdb` by fixed path, with the arguments
     `--nx --nh --batch -p <pid> -ex "gcore /proc/<caller pid>/fd/<n>"`, cleared environment
     except `PATH=/usr/bin:/bin` and `LC_ALL=C`, stdio on `/dev/null` (E07-O1 recommendation,
     iuz.15 as written). `dump_available()` requires both `/usr/bin/gdb` and `/usr/bin/gcore`
     to exist (gcore ships with gdb, so this is the "gdb installed" test). R-004 and ADR-006
     change "runs `/usr/bin/gcore` by fixed path" to "runs `/usr/bin/gdb` by fixed path with
     `gcore` as a batch command". The alternative (gcore into a private temp dir, then rename)
     loses the UI's size polling and needs a root-private temp dir in the helper.
  2. Direct path: the engine opens the UI's path `O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC`
     mode 0600 and calls `dump_to_fd`; the path must be directly under `<state_dir>/dumps/` with
     a `*.core` name (B3 item 16 (a) accepted).
  3. Helper path: the engine opens the file the same way and passes the fd as `SCM_RIGHTS`
     ancillary data on the `Dump` frame over a Unix socketpair that is the helper's stdin and
     stdout (E29-OI-2 accepted). `HelperRequest::Dump { key, mode }` has no fd field (E03-070 as
     written); the fd is the one control message that accompanies that frame. R-003's
     "stdin and stdout" sentence gains "(a socketpair, so a file descriptor can travel with the
     Dump request)". ADR-006 `Dump { key, mode, out_fd }` becomes `Dump { key, mode }` plus the
     passed fd.
  4. The helper runs the same `tl_ops::dump::dump_to_fd` as root (E29-006, qfx.9).
- Basis: R-004, ADR-006 helper allowlist [verified]; UI-PWN-034 (path and 500 ms size poll)
  [verified]; gdb `gcore` script internals [inferred]; E29-OI-2 and E03-070 text [verified].
- Owner decision: no.
- Changes: R-004, R-003, ADR-006 wording as above. iuz.15: remove the blocked note. qfx.9:
  the `SCM_RIGHTS` receive. as2.36: no change (already without `out_fd`). b2-collect-ops.md
  E07-O1: settled.

### OI-E08 `write_atomic` phase and the P1 settings store
- Raised by: B1 item 7, E33-OI-5, E07-O4.
- Question: E07-A3 (`fs.rs`, `launch.rs`, `dump.rs`) is P2, but E33's P1 settings store needs
  `write_atomic`.
- Answer: E07-010 (iuz.13, `fs::write_atomic`) is already its own bead at priority 1. The P1
  consumers depend on it directly, as a named exception to the anchors-only rule: cr0.11
  (E33-008 settings save) and cr0.17 (E33-014 autostart writer) depend on iuz.13 instead of
  iuz.3 (E07-A3). E07-A3 stays P2. The same exception is noted in 00-epics section 4.
- Basis: ADR-011 (`write_atomic` in `tl-ops/src/fs.rs`, R-130) [verified]; bead priorities
  [verified].
- Owner decision: no.
- Changes: `br dep add tasklens-cr0.11 tasklens-iuz.13`, `br dep add tasklens-cr0.17 tasklens-iuz.13`,
  then remove the two edges to iuz.3 (leader command). 00-epics section 4: the exception line.

### OI-E09 Privileged helper hook in E11
- Raised by: B3 item 9, E29-OI-6, E29-OI-5 (routing arm part).
- Question: the shape of the hook through which E11 actions reach the P3 helper.
- Answer: accept E11-002 (i3o.4) as written:
  `trait PrivilegedOps: Send + Sync + 'static { fn call(&self, cx: &Cx, req: HelperRequest, fd: Option<OwnedFd>, deadline: Duration) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>>; fn state(&self) -> HelperState; }`
  and `HelperCallError { Missing, Cancelled, NotAuthorized, Timeout, Exited, Protocol(Arc<str>) }`,
  both in `crates/tl-engine/src/actions/mod.rs`; `ActionCtx.helper: Option<Arc<dyn PrivilegedOps>>`,
  `None` until E29-014 (qfx.17) installs the `HelperHandle` implementation and the
  `req.privileged` routing arm. Until then a privileged request is `Rejected(Unsupported)` and
  never falls back to an unprivileged call (the security regression test in i3o.4). The `fd`
  argument is the `Dump` fd of OI-E07. B3 and B4 agree; E29-OI-6 is closed by this.
- Basis: ADR-005, ADR-006 (helper boundary) [verified]; i3o.4 text [verified]; E29-OI-6
  [verified].
- Owner decision: no.
- Changes: none to docs. `br dep add tasklens-qfx.17 tasklens-i3o.4` (the trait must exist
  before the implementation).

### OI-E10 Helper session commands and the action error values
- Raised by: E29-OI-3, E14 item 11, B3 item 10, B3 item 17, B8 item 9 (`AlreadyRunning`).
- Question: how the shield starts and ends a helper session, how a dismissed or denied polkit
  prompt reaches the UI, and how "Already running" is reported.
- Answer:
  1. `Command::StartHelperSession` and `Command::EndHelperSession` (E03-063; variant count
     becomes 21 with OI-E01). The helper is not a supervised source, so `forward()` does not
     fit. The session (E09 `session.rs`, 8q0.18) owns a `helper: Option<HelperHandle>` and the
     reducer routes the two commands through `Reducer.helper_tx: Option<mpsc::Sender<HelperCommand>>`
     with `HelperCommand { Start, End }`, set by E29-020 (qfx.23) as the allowed chain edit;
     `None` logs at `debug` and drops the command.
  2. `HealthSnapshot.helper: HelperState` exists (as2.13) [verified]. Add
     `HealthSnapshot.helper_start: Option<(u64 /* seq */, HelperStartOutcome)>` with
     `HelperStartOutcome { Cancelled, NotAuthorized, Failed(Arc<str>) }`; the UI shows the toast
     for a new `seq` once.
  3. `ActionError` gains `NotAuthorized` (pkexec 127, polkit `NotAuthorized`), `AuthCancelled`
     (pkexec 126, `NotAuthorizedDismissed`) and `Unsupported` (no `FileManager1` owner, invalid
     command text). E11-011, E11-018 and E11-020 map systemd `AccessDenied` to
     `Failed(NotAuthorized)` from the start (B3 item 17 becomes moot).
  4. "Already running" for `StartupRunNow` and `Launch`: a new `RejectReason::AlreadyRunning`,
     reported as `ActionState::Rejected(AlreadyRunning)` on the request. It is not an error and
     not a success, so neither `ActionError::AlreadyRunning` (B8 item 9) nor `Rejected(Busy)`
     (B3 item 10, which means the queue is full) fits. The UI maps it to the toast
     "Already running" (SYS-023).
  5. A `pkexec` launch counts as `Succeeded` when the child is still running 2 s after the
     prompt closed (E14 item 11 accepted); Plasma scripts in Run now go as `LaunchWhat::Command`
     with the shell-quoted path.
- Basis: UI-FND-028 (`HelperState`), R-117, SYS-023 [verified]; as2.13 and as2.19 text
  [verified]; E29-OI-3 and E14 item 11 [verified].
- Owner decision: no.
- Changes:
  - ADR-005: the two commands, `RejectReason::AlreadyRunning`, the three `ActionError` values;
    ADR-015 `ActionError` list likewise.
  - as2.29 (E03-063): two variants. as2.19 (E03-053): the error and reject values.
    as2.13 (E03-011): `helper_start`.
  - 8q0.3 (E09 reducer state): `helper_tx: Option<mpsc::Sender<HelperCommand>>`; 8q0.9: the
    two arms. qfx.23 (E29-020): allowed chain edit to set it.
  - i3o.13, i3o.17, i3o.18, i3o.20 (E11-011, E11-015, E11-016, E11-018): map to the new values;
    drop the "until E29-OI-3 lands" notes.
  - 9o0 "Already running" bead (UI side): match `Rejected(AlreadyRunning)`.

### OI-E11 Engine answers outside ActionEvent: setup state and the window pick
- Raised by: E03 contracts items 16 and 17, E29-OI-4, E29-OI-14, E14 item 7.
- Question: where `SetupStatus`, setup run progress and the KWin pick result travel, and the
  final `SetupStep` list.
- Answer:
  1. Setup state is a versioned part of `HealthSnapshot`:
     `HealthSnapshot.setup: Arc<SetupState>` with
     `SetupState { status: Option<SetupStatus>, run: Option<SetupRun> }`,
     `SetupStatus { rapl: RaplStatus { NoZone, Readable { tasklens_rule: bool }, NotReadable }, journal_group: bool, helper_installed: bool, lm_sensors: bool, hwmon_fan: bool, sensors_conf: bool }`,
     `SetupRun { seq: u64, step: SetupStep, phase: SetupPhase { Running, Succeeded, Cancelled, Failed(Arc<str>) }, output: Arc<str> }`
     (output capped at 64 KiB, last lines kept). E03-063's `SetupStatus { journal_group, lm_sensors_present }`
     and `SetupEvent` are replaced by these; no new channel.
  2. `SetupStep { RaplEnable, RaplRevert, SensorsDetect, SensorsRevert, PolkitRule { on: bool } }`
     with `args()` = `["rapl-enable"]`, `["rapl-revert"]`, `["sensors-detect"]`,
     `["sensors-revert"]`, `["polkit-rule", "on" | "off"]`. Table (c) gains the
     `sensors-revert` subcommand (removes `/etc/modules-load.d/tasklens-sensors.conf`, PERF-108);
     it is P4 like the button.
  3. Window pick: `WindowsSnapshot.pick: Option<PickResult { seq: u64, outcome: PickOutcome { Picked { pid: u32 }, NoWindow, Cancelled } }>`;
     `KWinCommand::Pick { seq }` and `CancelPick { seq }` carry the sequence; the
     `WindowBridge` D-Bus interface gains `Picked(s uuid, u pid)` called by `pick.js`
     (E14 item 7 accepted). UI-SHL-014's "as an `ActionEvent` or a `KWinEvent`" becomes
     "in `WindowsSnapshot.pick`".
  4. Transport: `SourceUpdate::Setup(Arc<SetupState>)` (from the setup runner task) and the
     existing `SourceUpdate::Windows` carrying `pick`. See OI-E12 for the variant list.
- Basis: table (c) subcommands, R-028, PERF-108 [verified]; UI-SHL-014 [verified]; E29-OI-4
  proposal [verified].
- Owner decision: no.
- Changes: ADR-003 `HealthSnapshot` and `WindowsSnapshot` fields; table (c) `sensors-revert`;
  UI-SHL-014 wording. as2.13: `setup`. as2.16: `pick`. as2.29: the `SetupStep`, `SetupState`
  types, `Pick { seq }`. y6k.21, y6k.23 (E14-017, E14-019): `Picked` method and `seq`.
  qfx.22 (E29-019): produce `SetupState`. qfx.25 (E29-023): `sensors-revert`.

### OI-E12 `SourceUpdate` variants and `EngineConfig` fields added up front
- Raised by: E29-OI-5, E30-OI-4, E32 OI-1, B3 items 2 and 3, E12 "resume detection" note.
- Question: which variants and config fields E08 writes up front so E29, E30 and E32 need no
  edits to `update.rs`, and whether E29-020 and E30-A2 may chain-edit `session.rs` and `apply.rs`.
- Answer:
  1. E08-003 (8hz.5) gains `SourceUpdate::Helper(HelperUpdate { state: HelperState, start: Option<(u64, HelperStartOutcome)> })`
     and `SourceUpdate::Setup(Arc<SetupState>)`. `History(Arc<HistoryView>)` is already there
     [verified]. `source_of()` maps `Helper` and `Setup` to `None` (no supervised source).
  2. Low memory: no new variant. `ProcessSweep.low_memory: Option<Arc<LowMemoryInfo>>` with
     `LowMemoryInfo { groups: Arc<[LowMemGroup]>, mem_total: u64, mem_available: u64, psi_full_pct: Option<f32>, critical: bool }`
     and `LowMemGroup { group: GroupKey, name: Arc<str>, process_count: u32, uid: u32, cgroup: CgroupRef, memory_current: u64 }`
     (E32 OI-1 types, moved to tl-model `process.rs`); the reducer copies it into
     `ProcessSnapshot.low_memory: Option<Arc<LowMemoryInfo>>`. The stripped sweep
     (E32-006, m1f.8) sends an ordinary `Processes(ProcessSweep)` with that field set.
  3. `EngineConfig` (E08-001, 8hz.3): `{ mode, sys_root, state_dir, history_dir: Option<PathBuf>, helper_path: Option<PathBuf> }`.
     tl-app sets `history_dir = $XDG_STATE_HOME/tasklens/history` and
     `helper_path = /usr/lib/tasklens/tl-helper` when the file exists (ADR-006 fixed path).
  4. Chain edits allowed, each a named insertion point: E29-020 (qfx.23) in `session.rs`
     (helper spawn) and `reducer/apply.rs` (the two helper commands, `Helper` and `Setup` arms
     are written up front by E09 and only log until then); E30-A2 in `session.rs` (install the
     `RecordSink`, start the history worker) and `reducer/apply.rs` (`HistoryQuery`,
     `HistoryDelete` forward to `SourceId::HistoryRecorder`, already an arm in 8q0.9). 00-epics
     section 2 records both.
  5. `ResumeDetected` stays with E08's logind worker only (E12 note accepted).
- Basis: 00-epics section 2 ("SourceUpdate carries every variant up front") [verified];
  8hz.5 variant list [verified]; ADR-012 (history dir, R-136) [verified]; ADR-006 helper path
  [verified].
- Owner decision: no.
- Changes: ADR-004 `EngineConfig` sentence; 00-epics section 2 chain-edit rows for E29-020 and
  E30-A2. 8hz.3, 8hz.5, 8q0.9, 8q0.18 as above. as2.10: `LowMemoryInfo`, `LowMemGroup`;
  as2.16: `ProcessSnapshot.low_memory`. m1f.8: use the tl-model types.

### OI-E13 PSI trigger window for the memory watcher
- Raised by: B1 item 5, E32 OI-2.
- Question: R-062 writes `full 100000 1000000` (1 s window); since Linux 6.5 an unprivileged
  trigger must use a window that is a multiple of 2 s, and older kernels refuse unprivileged
  triggers outright.
- Answer: the trigger is `full 200000 2000000` (10% stall over a 2 s window) from the start, so
  the same string works on every kernel from 6.5 on. Older kernels refuse the write with
  `EINVAL` or `EPERM`; the watcher then logs `warn` once and runs on the 5 s `MemAvailable`
  check only (`WaitResult::Timeout`), which is already the E32-A1 (m1f.1) fallback. No runtime
  probing of two strings. R-062 and PROC-097 change the window to 2 s; the 10% threshold and
  the 3% `MemAvailable` rule stay. New entry U-50: "unprivileged PSI trigger accepted on the
  owner machine (kernel 7.2)", recorded by m1f.1's ignored test.
- Basis: R-062, PROC-097 [verified]; kernel `Documentation/accounting/psi.rst` rule for
  unprivileged triggers (window a multiple of 2 s, added in 6.5) [inferred from memory of the
  kernel docs; the U-50 run confirms].
- Owner decision: no.
- Changes: R-062, PROC-097: `full 200000 2000000`, "2 s window". m1f.1, m1f.3: `PSI_TRIGGER`
  constant and the fallback sentence. RECONCILIATION U table: U-50.

### OI-E14 Low-memory mode details owned by the engine and tl-app
- Raised by: E32 OI-3, OI-4, OI-6, OI-7, OI-8, OI-9, OI-11, OI-12, OI-13, E32 OI-5 (settled in
  OI-E04 item 5).
- Question: the remaining E32 choices.
- Answer:
  - OI-3 unit file: accept the E32-002 (m1f.4) values (`Description=tasklens low-memory
    watcher`, `Type=simple`, `Restart=on-failure`, `RestartSec=5`, `WantedBy=default.target`,
    `ExecStart=<current_exe> --watch-memory`, path
    `$XDG_CONFIG_HOME/systemd/user/tasklens-lowmem.service`, install over the session-bus
    Manager API, `ActiveState` poll every 100 ms up to 2 s for exit status 214).
  - OI-4: the watcher logs `warn` and above to stderr only, no file appender. ADR-015 gains
    one sentence.
  - OI-6: rely on the user manager's imported `WAYLAND_DISPLAY` or `DISPLAY`; when neither is
    set at trigger time, log `warn` and do not spawn. "No rescue window open" means the
    watcher's own child is alive.
  - OI-7: accept the texts and paths as proposed: tooltip "systemd-oomd may end processes before
    the rescue window opens."; on install or remove failure revert the setting and send the
    desktop notification "Could not set up the rescue window" through E33-010 (cr0.13);
    `state.toml` `[lowmem] oom_score_adjust` with the extra tooltip sentence " The watcher runs
    with normal OOM priority." when it is 0; at launch reinstall the unit when `lowmem.enabled`
    is true and the file is missing. Table (a) gains the state key; UI-SHL-069 gains the
    tooltip text.
  - OI-8: in `LowMemory` mode the app name is the app id parsed from the scope name; no desktop
    index is loaded.
  - OI-9: `EngineMode::LowMemory` runs the `Processes` worker (stripped sweep) and the `KWin`
    worker, the latter only for `KWinCommand::KeepAbove`; the window starts pinned as
    UI-PWN-032 says. ADR-004's `EngineMode` sentence lists both workers.
  - OI-11: "Open Full tasklens" sends `ActionKind::Launch(LaunchSpec { what: LaunchWhat::Command("tasklens"), as_admin: false, cwd: None })`.
    The actions region and the targetless launcher run in both engine modes, so m1f.11
    (E32-009) depends on y6k.3 (E14-A3).
  - OI-12: the rescue process tries `mlockall(MCL_CURRENT)` once after the first frame, logs
    `warn` on failure and never uses `MCL_FUTURE`. PROC-098's "mlocked after start" sentence
    gains "(current pages only)".
  - OI-13 `--force-full-memory-mode`: with `--low-memory` it starts the engine in
    `EngineMode::Full` while keeping the rescue window; alone it does nothing. Not beaded
    until the owner confirms.
- Basis: PROC-097, PROC-098, UI-PWN-032 to UI-PWN-034, APP-052, ADR-004 `EngineMode`,
  ADR-015 [verified]; systemd user-manager environment import on Plasma [inferred].
- Owner decision: yes, for OI-13 only (what a test flag means is the owner's call; recommended
  reading above; alternative: drop the flag from APP-052). The rest: no.
- Changes: ADR-004 `LowMemory` worker list; ADR-015 watcher logging sentence; PROC-098
  mlock sentence; table (a) `[lowmem] oom_score_adjust` (state); UI-SHL-069 tooltip texts.
  m1f.4, m1f.6, m1f.8, m1f.11 as above. `br dep add tasklens-m1f.11 tasklens-y6k.3`.

### OI-E15 `UiLaunch` shape and the tl-app to tl-ui boundary
- Raised by: B1 item 1, E33-OI-1, E33-OI-4, E33-OI-12, B5 report (E33-A1 on fut.15), B5 item 38.
- Question: the exact entry point tl-app calls, and how persist callbacks and tl-app-only
  features cross the boundary.
- Answer:
  1. E16-012 (fut.15) is the contract: `tl_ui::run(UiLaunch) -> UiExit` with
     `UiLaunch { client: Arc<dyn EngineClient>, init: UiInit, open: OpenMode { Main, Gallery, LowMemory }, start: StartOptions { tab: Option<Tab>, select_pid: Option<u32>, start_minimized: bool, shift_held: bool, pending_crash_report: Option<PathBuf> } }`,
     `UiExit { Quit, RendererFailed(String), DeviceLost }`. B1's E33-OI-1 proposal is replaced:
     settings, state, themes, the persist callbacks, `app_requests` and queued toasts are
     fields of `UiInit` (E16-A1, fut.1); `debug` is dropped (tl-app owns logging); `on_quit`
     is the return value.
  2. Persist callbacks (E33-OI-4), on `UiInit`: `persist: Option<Box<dyn Fn(&Settings, &UiState) + Send + Sync>>`
     called on every change (tl-app debounces 500 ms, E33-008) and
     `persist_theme: Option<Box<dyn Fn(PersistTheme) + Send + Sync>>` with
     `PersistTheme::{Save(Vec<u8>), RenameCorrupt}` (theme bytes already serialised by tl-ui,
     tl-app writes them with `write_atomic`).
  3. tl-app-only features (E33-OI-12): `UiLaunch.platform: Option<Arc<dyn PlatformOps>>`
     with `trait PlatformOps: Send + Sync { fn ctrl_esc_state(&self) -> Option<bool>; fn set_ctrl_esc(&self, on: bool); fn import_taskslinger(&self); }`
     declared in tl-ui `app.rs`. `None` until E33-017 and E33-018 (P4) install it; the P4
     settings rows show disabled with "Not available" when it is `None`.
  4. Device loss: whoever detects it (E33-012, cr0.15, through a GPUI hook in tl-ui) calls
     `tl_ui::request_exit(cx, UiExit::DeviceLost)`; tl-app relaunches once with
     `TASKLENS_RELAUNCHED=1` in the environment (E33-OI-8 accepted; table (c) environment list).
  5. U-2 (`Application::run` returns on Linux) stays open and is answered by E33-011 (cr0.14).
- Basis: ADR-001 tl-app row (no gpui-kit) [verified]; UI-SHL-110 steps 8 to 11 [verified];
  fut.15 and cr0.1 text [verified]; ADR-011 persist callback [verified].
- Owner decision: no.
- Changes: ADR-010 or ADR-011: one paragraph naming `tl_ui::run(UiLaunch) -> UiExit` and the
  `UiInit` callbacks; table (c): `TASKLENS_RELAUNCHED`. fut.15: `platform` field and
  `PlatformOps`. fut.1 (E16-A1): the two callbacks on `UiInit`. cr0.1 (E33-A1): build
  `UiLaunch` as in fut.15; cr0.11: debounce and `persist_theme`; cr0.14, cr0.15, cr0.20,
  cr0.21: as above. b1-infra.md E33-OI-1: settled by fut.15. Edges:
  `br dep add tasklens-cr0.1 tasklens-fut.15`, `br dep add tasklens-cr0.14 tasklens-fut.15`,
  `br dep add tasklens-cr0.15 tasklens-fut.15`.

### OI-E16 Aggregate devices and per-process series
- Raised by: E26-OI-1, E26-OI-2, E31-8 (a) (tray process graphs), PERF-005 `aggregate:disk`
  and `aggregate:gpu` keys.
- Question: which device carries the Disk and Network card series, and how a process, name or
  app graph gets an engine-sampled series while hidden.
- Answer:
  1. `DeviceIdentity::Aggregate(AggregateKind { Disks, Nets, Gpus })` in E03-010 (as2.12);
     persistent keys `aggregate:disk`, `aggregate:net`, `aggregate:gpu` (PERF-005 names the
     first and last; `aggregate:net` is new). The reducer, not E12, computes the series: in the
     `Disks` arm `DISK_ACTIVE` = max over present physical disks, `DISK_READ` and `DISK_WRITE`
     = sums; in the `Nets` arm `NET_RX` and `NET_TX` = sums over interfaces whose `NetKind` is
     shown under `EngineSettings.show_network_*`; in the `Gpu` arm `GPU_UTIL` = max and
     `GPU_MEM` = sum. The reducer has every per-device value and the settings in one place;
     E12-OI-1's "E12 publishes" sentence is replaced. The three devices are created at session
     start with `present = true`.
  2. Per-process series: in tl-model `ids.rs`
     `GraphTarget { Process(ProcessKey), Name(Arc<str>), App(Arc<str>) }` and
     `ProcessSeriesId { target: GraphTarget, column: ColumnId }`; in `demand.rs`
     `Demand.graph_targets: Arc<[(GraphTarget, ColumnId)]>` with `GRAPH_TARGETS_MAX = 16`
     (clamped in `clamp_lists`); in `snapshot.rs`
     `ProcessSeriesSet { header: ComponentHeader, series: Arc<[(ProcessSeriesId, Arc<SeriesView>)]> }`
     as `Snapshot.process_series`, mask bit `PROCESS_SERIES = 1 << 13` (OI-E01 item 6).
     Columns allowed as graph sources: `cpu`, `memory`, `disk`, `network`, `gpu`,
     `gpu_mem_dedicated`; others are ignored. A non-empty `graph_targets` makes the
     `Processes` worker run at `processes.interval_ms` even when `process_table` is false
     (PROC-093); the reducer appends one sample per sweep per target, summing over the
     matching processes for `Name` and `App`, and marks a gap when no process matches.
     Rings use the same 60 s fast tier plus the 1 Hz slow tier as device series.
  3. Tray process graphs (E31-8) use the same `graph_targets` entries; `TrayDemand` has no
     process list (see OI-E18 item 5).
- Basis: PERF-080, PERF-085, PERF-005, PROC-093 [verified]; ADR-003 `SeriesId`, ADR-008
  `Demand` [verified]; E26-OI-1 and E26-OI-2 text [verified].
- Owner decision: no.
- Changes: ADR-003: `Aggregate` identity, `process_series` component, `ProcessSeriesId`;
  ADR-008: `graph_targets` row and the hidden-sampling rule. as2.12, as2.6 (`GraphTarget`,
  `ProcessSeriesId`), as2.26 (`graph_targets`, the max), as2.16 and as2.30 (component and
  bit). 8q0.9: aggregate computation in the three arms. New bead E09-029 "per-process series
  from Demand.graph_targets" (P2, files `reducer/process_series.rs`, `reducer/mod.rs` module
  line; depends on 8q0.1, as2.2; xdh's E26-011 and wox.9 depend on it). dwy.4 (E10
  ProcessesSource): period rule for `graph_targets`.

### OI-E17 tl-model core types: ColumnId numbers, NodeId, health and device entries
- Raised by: E03 first-half items 1, 2, 3, 7, 8, 9, 10, 11, 13; B1 item 6; E12-OI-6; B3
  items 11 and 14; E13 health note.
- Question: the derived values in E03-A1's beads that B1 asked to have reviewed.
- Answer:
  1. `ColumnId` numbers 1 to 121 in PROC-027 to PROC-036 order (E03-005, as2.7) are accepted
     and frozen. Dense index = number minus 1; `ColumnSet([u64; 4])` holds 256. New columns
     append after 121 and are never renumbered (ADR-012 constraint 2). The system tables use
     a separate `SysColumnId` in `table.rs` (item 2 accepted); `SysNodeId` lives in `table.rs`
     (item 3 and contracts item 1 accepted; 00-epics section 2 moves it from `ids.rs`).
  2. `NodeId` gains `Unit`, `KernelThreads`, `Pseudo` (OI-E03 item 3; ADR-009 edit).
  3. System totals (item 7): E03-009's field sets with `Metric<T>` are accepted, plus the
     OI-E01 part-B fields and B3 item 12's `GpuTotals.pcie_rx_bps`, `pcie_tx_bps:
     Metric<f32>`.
  4. `HealthSnapshot` keeps a plain `version: u64` instead of `ComponentHeader` (item 8
     accepted; no module cycle). `SourceState` becomes
     `{ Off, Running, Late, Stale, Failed, Disabled { until: TimeNs }, Unsupported }`:
     `Unsupported` is a terminal state the controller never retries, used for "GPU probe not
     installed", `LibraryNotFound`, `DriverNotLoaded` and "KWin bridge not available" (B3 item
     14 alternative chosen, because a string match in the UI on `last_error` would break with
     any text change). `last_error` capped at 200 characters; no `SourceId` for the public IP
     fetch or the low-memory sweep (item 9).
  5. 1 Hz tier (item 10): accepted as E03-007 writes it; the fast ring is sized for the
     fastest period a source can use (CPU 10 Hz, 600 samples for 60 s).
  6. Measured zero (item 11): PROC-003's rule applies to every rate, power and time form;
     `percent_2dp` zero is "0%"; `watts_pair` prints the limit as an integer. PROC-003 gets the
     generalised sentence.
  7. Device entries (item 13, E12-OI-6): `DeviceDesc` (the list entry in E08-003's
     `DeviceList`) gains `persistent_key: Arc<str>`; workers compute it per PERF-005
     (`disk:<ID_WWN or ID_SERIAL>` with by-path fallback through OI-E02,
     `fan:<hwmon name>:<device path>:<n>` from `FanReading.chip_device_path`,
     `bat:<NativePath>`, `net:<MAC or name>`, `gpu:<uuid or PCI slot>`). `DeviceEntry` stores
     `persistent_key` and `net_kind: Option<NetKind>`; `DeviceIdentity` is unchanged.
  8. GPU catalog (B3 item 11): no `GPUS` list in P1; the `Gpu` and `GpuInfo` arms call
     `devices.ensure` per identity; `GpuTotals.device` is a placeholder the reducer overwrites.
     The `GpuInfo` payload carries `persistent_key`.
- Basis: PROC-003, PROC-027 to PROC-036, PERF-005, PERF-062 [verified]; ADR-003, ADR-007,
  ADR-009, ADR-012 [verified]; as2.7, as2.11, as2.13 text [verified].
- Owner decision: no.
- Changes: ADR-003 (`DeviceEntry` fields, `SourceState::Unsupported`, `GpuTotals` pcie
  fields); ADR-009 (`NodeId`); PROC-003 zero rule; 00-epics section 2 (`SysNodeId`).
  as2.13: `Unsupported`. as2.12: `DeviceEntry` fields. as2.11: pcie fields. 8hz.5:
  `DeviceDesc.persistent_key`, `GpuDeviceInfo.persistent_key`. 8hz.8 (controller): treat
  `Unsupported` as never-retry. ndy.12 (GpuNvidia worker): report `Unsupported` for the three
  cases. 8q0.5 (catalog): store the key. hjn.5 (E12-002): compute the keys.

### OI-E18 tl-model contract types: the derived field lists
- Raised by: E03 contracts items 3, 5, 6, 8, 9, 11, 12, 13, 14, 15, 18, 20, 21, 22, 24;
  E30-OI-1; E14 item 1; B1 item 6.
- Question: confirm the field lists E03-A2 derived, and settle the history types conflict.
- Answer:
  1. Accepted as the beads write them: `StartupRef { source: StartupSource, id: Arc<str> }`
     (item 3); `SystemSnapshot` and `WindowEntry` layout (item 6); the services, startup and
     connections records (item 8, `ImpactDetail`, `TcpInfoView`, `ConnRecord.four_tuple`);
     `InventorySnapshot` per-section structs (item 9); the Properties payloads (item 11);
     `FourTuple { local: SocketAddr, remote: SocketAddr }`, `Family { Inet, Inet6 }`,
     `Proto { Tcp, Udp }` (item 12); `RecoveryEdit` with one `Option` per SYS-013 key
     (item 13); `tl_model::command::Tab` with the six R-023 names, and tl-ui's `TabId` is this
     type (item 14); `KWinCommand` variants of E03-063 plus `seq` on `Pick` and `CancelPick`
     (item 15, OI-E11); `GpuApiVersions { pci_bus_id, vulkan_api, vulkan_driver, opengl }`
     plus `GpuDeviceInfo.cuda_version`, `vbios`, `architecture: Option<String>` (item 20, B3
     item 12); clamp then round to the nearest step (item 21); `Snapshot::empty` and
     `changed_since` (item 22); `CpuSet` derives `Serialize, Deserialize` (item 24, as2.11).
  2. `TrayDemand` (item 5, E31-8 a): `{ icons_shown: bool, cpu: bool, memory: bool, disks: bool, nets: bool, gpus: bool }`.
     No `process_keys`: tray process graphs use `Demand.graph_targets` (OI-E16).
     `icons_shown` counts as a visible surface in the aggregator.
  3. History types (item 18, E30-OI-1): the E30 block wins over E03-062 and E03-063's
     sketches, because all 30 E30 beads on both sides are written to it:
     `HistoryMetric`, `HistoryRange::{Live, At(TimeNs), Span { start_ns, end_ns }}`,
     `HistoryWhat::{Frame, TopAt { n: u8 }}` and `HistoryView` with `query_id`, `source`,
     `store_state`, `span`, `gaps`, `frame_t`, `range`, `process_count`, `rows`, `totals`,
     `series`, `stack`, `overview`, `top`, `realtime_offset_ns`, plus `HistoryProcessRow`,
     `HistoryTop`, `HistoryLoadProblem` as the "tl-model history types" block in the E30 beads
     (for example E30-012) spells them. `ColumnId` has `image_path` (number per PROC-027
     order) [verified in the PROC-027 table].
  4. E14 item 1's list is a check, not a change: the services unit record carries
     `cgroup_pids`, `has_cgroup_procs`, the four resource fields with `Validity`, `can_reload`,
     `main_pid`; `ManagerState`; `UnitDetails` with `RecoverySettings` and `DepEntry`;
     `JournalHandle` with `JournalControl::{LoadOlder, SetFollow(bool)}` and the
     `bounded(64)` event channel (E14 item 9). The leader checks as2.20, as2.21, as2.27
     against the E14-001 and E14-004 field names before E14-A1 starts; an E14 bead that finds
     a field missing stops and reports.
- Basis: UI-PRC-030 to UI-PRC-037, UI-SYS data bindings, SYS-013, R-023 [verified]; the E30
  block in the E30 beads [verified in b4 plan text]; as2.26, as2.29, as2.36 text [verified].
- Owner decision: no.
- Changes: ADR-012: the history type names. as2.26: `TrayDemand` fields. as2.28 (E03-062),
  as2.29 (E03-063): replace the history types with the E30 block. as2.35 (E03-069): the
  three `GpuDeviceInfo` fields. Leader check of as2.20, as2.21, as2.27 against E14-001 and
  E14-004.

### OI-E19 Helper protocol: frame, replies, handshake and request set
- Raised by: E29-OI-1, E29-OI-7, E29-OI-9, E03 contracts item 19 (non-fd part), B3 item 16
  (security label).
- Question: two shapes exist, E03-070's and E29-OI-1's; which one holds, and what is added.
- Answer: E03-070 (as2.36) holds: `HelperFrame { seq: u32, request }`,
  `HelperReply { seq: u32, last: bool, body: HelperReplyBody }`, `Hello { proto }` both ways,
  `HELPER_PROTO_VERSION = 1`, `HelperError { IdentityChanged, PermissionDenied, NoSuchProcess, NotAllowed, Partial { done, total }, Os(i32) }`,
  `validate_drop_in`, the 64 KiB frame and the exhaustive-variant tests. It is the type owner
  and carries the security regression tests, and the engine parser for `NetlinkDump` is E06's.
  E29 beads rename their `req` to `seq`, `Done` to `Ok`, `Error` to `Err`, `Files` to `File`,
  `Inodes` to `SocketOwners`, `SockDiag` to `NetlinkDump`. Added from E29-OI-1 and E29-OI-9:
  `HelperRequest::CancelScan { seq: u32 }` (17 variants; the exhaustive test changes to 17),
  `HelperError::TooLarge`, `ProcFiles::ROOT = 1 << 11` (so `/proc/<pid>/root` reads for
  PROC-078 and PROC-096 are in the allowlist; R-004 file set gains `root`).
  `ReadProcessDetail` is dropped from ADR-006 (already absent in as2.36). The 4 MiB cap is per
  reply message for streamed requests (`ScanStrings`, `NetnsSockDiagDump`), total for the
  rest (E29-OI-7). tl-helper may depend on tl-collect (OI-E04 item 14) so `ScanStrings` reuses
  `proc/strings.rs`. The `Dump` fd is OI-E07.
- Basis: ADR-006, R-003, R-004, R-097, R-098 [verified]; as2.36 text [verified];
  E29-OI-1 text [verified].
- Owner decision: no.
- Changes: ADR-006: drop `ReadProcessDetail`, add `CancelScan`, `root`, the per-message cap
  sentence; R-004 likewise. as2.36: `CancelScan`, `TooLarge`, `ROOT`, test counts. qfx.4 to
  qfx.13, qfx.16: the renames; qfx.8 (E29-005): `CancelScan` handling.

### OI-E20 E02 workspace choices
- Raised by: E02 items 1 to 7, B1 item 3.
- Question: package name, `bench` lines, banned-API greps, zbus in tl-ui's closure, CI packages,
  version, rustfmt.
- Answer: accept all seven as E02 wrote them: package `tl-app` with `[[bin]] name = "tasklens"`
  and ADR-002's command becomes `cargo tree -p tl-app` (item 1); E02-005 (v9m.7) writes both
  `bench` feature lines up front, so E01-001 (wlf.3) and E01-002 (wlf.4) are closed as
  duplicates and 00-epics section 2 drops the E01 `Cargo.toml` chain edits (item 2, B1 item 3);
  `check-deps` matches the literals `RuntimeHandle::spawn(` and `.with_poll_quota(` with the
  `// check-deps: allow <reason>` escape (item 3); if `zbus` shows up in tl-ui's closure through
  gpui-kit, the ADR-001 check 2 rule narrows to "no direct dependency and no path that avoids
  gpui-kit", recorded by E02-A1 (item 4); the Ubuntu CI package list of E02-010 and the lavapipe
  ICD path `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json` (item 5, also E33-OI-11);
  `version = "0.1.0"`, `publish = false` (item 6); no `rustfmt.toml` (item 7).
- Basis: ADR-001, ADR-002, ADR-013 [verified]; v9m.7 and wlf.3 text [verified].
- Owner decision: no.
- Changes: ADR-002 `-p tl-app`, version line; 00-epics section 2 (drop the two E01 chain
  edits); close wlf.3 and wlf.4 ("duplicate of v9m.7"); wlf.1 (E01-A1) loses those two
  dependencies.

### OI-E21 E33 binary and packaging choices
- Raised by: E33-OI-6 to E33-OI-11, E33-OI-13 to E33-OI-15 (OI-1 to OI-5, OI-8 and OI-12
  are in OI-E15, OI-E04 and OI-E08).
- Question: the remaining tl-app choices.
- Answer:
  - OI-6 `--app-id <id>`: replaces the `tasklens` directory component in all four XDG roots
    (config, state, cache, runtime), and is the Wayland `app_id` and the single-instance bus
    name. History, logs, crash reports and the icon cache follow. Recommended; owner to
    confirm because APP-052 says only "separate instance and settings dir".
  - OI-7: the clap `about` line "Task manager for Linux", the per-argument help texts of
    E33-003 (cr0.6) and the critical notification urgency (cr0.13) are accepted as written.
  - OI-9: the desktop file keys of E33-013 (cr0.16) are accepted; the icon installs to
    `/usr/share/icons/hicolor/scalable/apps/io.github.oysteinkrog.Tasklens.svg`.
  - OI-10: `pkgver=0.1.0`, build from the local checkout, `nvidia-utils` as `optdepends`.
    `license` is the owner's choice (D-005 keeps the repo private, so no licence is implied).
  - OI-13: TaskSlinger INI section names and value encodings are not in the docs; E33-018
    (cr0.21) imports plain booleans and in-range integers only until the owner supplies
    `UserSettings.ini` or the inventory section.
  - OI-14: the tray start in `main.rs` is E31-008 (wox.10), a chain edit depending on cr0.1,
    which also adds the `tl-tray` dependency line (00-epics section 2 row).
  - OI-15: `crates/tl-app/build.rs` is an E33 owner-only file (section 2 row).
- Basis: APP-052, APP-053, ADR-010, ADR-015, D-005 [verified]; cr0.4 to cr0.22 text [verified
  by title and the B1 summaries].
- Owner decision: yes, for OI-6 (what `--app-id` isolates), OI-10 (licence) and OI-13 (INI
  encodings; the owner holds the file). Recommended answers above; alternative for OI-6: only
  the config directory and the bus name change.
- Changes: APP-052: the `--app-id` sentence; 00-epics section 2: `build.rs`, the E31 main.rs
  chain edit. cr0.22: as above once confirmed.

### OI-E22 E01 bench harness choices
- Raised by: E01-OI-2 to E01-OI-8, E01-OI-10 to E01-OI-16 (OI-1 and OI-9 are in OI-E04 and
  OI-E06).
- Question: the synthetic rules, report fields, cross-epic hooks and the soak launch.
- Answer: accept the E01 recommendations with these fixed values: OI-2 ("X +- N" is a fresh
  uniform draw each second; random walk steps uniform in [-1, 1] for memory and [-0.05, 0.05]
  for load average; network bursts p = 0.1 per second else 0.01 Mbps; the placeholder catalog
  of 16 cores x 2 threads, disks 259:0 to 259:3, ifindex 2 and 3); OI-3 `long_missed_frames`
  after `missed_frames` in the JSON (UI-PRF-053 shape edit); OI-4 MB = 1,000,000 bytes;
  OI-5 `CpuPage.bench_show_both` behind `#[cfg(feature = "bench")]` in E24-A1 (crw.1);
  OI-6 the E19-A1 (yrf.1) paint record behind `bench` (leader notes say yrf.7 and yrf.9
  carry it, feature `test-hooks`, so tl-ui's `bench = ["test-hooks"]`); OI-7 the nine R-006
  columns plus `power`, 40 app groups of 50 processes, the value ranges as written;
  OI-8 `TlTreeTable::scroll_handle()` from E18-A1 (rsf.1) and the table entity from E22-A1
  (pdu.1); OI-10 `xtask bench` runs its build through `heavy-build` when `TL_BUILD_WRAP=1`
  (replaces the old flock line); OI-11 "tasklens Gallery"; OI-12 scene `soak` added to table
  (c), run `--duration 600 --warmup 0`; OI-13 E01 owns `vendor/gpui-pre-linux/` and one root
  `Cargo.toml` chain edit (section 2 rows); OI-14 phase-0 reports carry `gpu: null`, E01-023
  (wlf.25) depends on ndy.2; OI-15 collection cost = utime + stime of `tl-engine*` threads plus
  the probe child over 60 s, in E01-024 (wlf.26); OI-16 E20-008 (jqk.10) writes
  `first_frame_ms`, so E01-017 (wlf.19) depends on it.
- Basis: UI-PRF-053, ADR-014, ADR-010 (soak), R-006 [verified]; leader notes on yrf.7, yrf.9,
  jqk.10 [verified].
- Owner decision: no.
- Changes: UI-PRF-053 (`long_missed_frames`, the value rules); table (c) `soak` scene; 00-epics
  section 2 (`vendor/gpui-pre-linux/`, root `Cargo.toml` line). crw.1: the flag. rsf.1:
  `scroll_handle`. Edges in the Edges section.

### OI-E23 E31 tray choices
- Raised by: E31 items 1 to 11, E33-OI-14.
- Question: the tray type home, the chain edits, the icon pixels, menu plumbing and wording.
- Answer:
  - Item 1: approve E31-001 (wox.3) as written: `crates/tl-model/src/tray.rs` plus one
    `pub mod tray;` line, after E03-A2 (edge exists). E03's frozen module list gains `tray.rs`
    (00-epics section 2). `raster::render(max: f32)` and the model channel `bounded(2)` with
    `try_send` are accepted.
  - Item 2: E31-008 (wox.10) is the allowed `main.rs` chain edit (OI-E21).
  - Item 3: E31-009 (wox.11) may chain-edit E20's `window/main.rs` (close decision row 1) and
    `window/titlebar.rs` (minimize to tray), depending on jqk.1 (E20-A1); 00-epics section 2
    gets the row (the B7 report asked for the same).
  - Item 4: two pre-rendered files `assets/icons/tray-22.argb` and `tray-44.argb` under E15's
    assets, `include_bytes!` in tl-app. Alternative SNI `IconName` loses the icon when the
    hicolor theme is not installed (development runs).
  - Item 5: (a) `PerfAddTrayGraph { source }` is declared in `tray_bridge.rs` and E24's P3
    menu items depend on wox.9 (E31-A2 surface); (b) the row menu dispatches
    `AddTrayGraph { source, column }` / `RemoveTrayGraph { source, column }` built at menu
    time; (c) process row `process:<pid>`, app row `app:<id>`, unit row `app:<unit>`; the
    PROC-094 "name filter" mode is reached from the minigraph "Add Process Name Graph" item,
    not from the row menu (UI-side detail for E26).
  - Item 6: "Accent (theme default)" (UI-SHL-085 wins over the shorter UI-PRF-051 word).
  - Item 7: UI-FND toast rule 8 holds; the UI-PRF-051 "shown on the next render" sentence
    goes.
  - Item 8: (a) `TrayDemand` per OI-E18 item 2; (b) `VRAM Usage` and `GPU temp` tooltip
    forms accepted; (c) busiest disk active time and the first GPU in sidebar order; (d) five
    lines joined by `\n` (PERF-089 and UI-PRF-051 win; APP-047 and UI-SHL-113 get the edit).
  - Item 9: UI side (E26-A1 names), left to the UI file.
  - Item 10: assume a watcher is available until the first ksni callback says otherwise.
- Basis: ADR-001 rows, UI-SHL-113, UI-PRF-050, UI-PRF-051, UI-SHL-085, PERF-089 [verified];
  wox.3 text [verified].
- Owner decision: no.
- Changes: 00-epics section 2 (`tray.rs` in E03's list, the E31 chain-edit rows for
  `main.rs`, `window/main.rs`, `window/titlebar.rs`); APP-047 and UI-SHL-113 tooltip lines;
  UI-PRF-051 toast sentence. wox.3, wox.10, wox.11: drop the pending notes. Edge
  `br dep add tasklens-wox.11 tasklens-jqk.1`.

### OI-E24 E04 collect-system choices
- Raised by: E04-OI-3 to E04-OI-10, E04-OI-12 (OI-1, OI-2, OI-11 and OI-13 are in OI-E02).
- Question: the remaining E04 choices.
- Answer: OI-3 MAC addresses scrubbed to `00:00:00:00:00:00` and added to ADR-013's list;
  OI-4 `/proc/vmstat` dropped from v1 (ADR-004 and ADR-016 `Memory` worker rows lose it;
  `parse/vmstat.rs` stays a stub until a spec row needs it); OI-5 PERF-037 moves to E06
  (hx1.18 builds the readers; 00-epics section 6 edit); OI-6 E12 maps tl-collect readings to
  the tl-model totals (accepted; E04 keeps its own reading types); OI-7 the ADR-016 latency
  formula (read and write ticks over read and write counts) stands, because ADR-016 outranks
  PERF-049; PERF-049's response-time line gets the ADR-016 form; OI-8 Virtual name patterns are
  checked before `bridge/` (PERF-054 order edit); OI-9 visibility groups: Ethernet and USB
  network to Wired, Wi-Fi and Cellular to Wireless, Tunnel and VPN to VpnTunnel, Virtual and
  Bridge to Virtual, Other to Other; this replaces E12-OI-7's Cellular to Other, and the
  mapping lives in E12-014 (hjn.17); OI-10 `mpqemubr` added to `VIRTUAL_NAME_PREFIXES` once
  seen on a Multipass install (U-51); OI-12 build lines are the leader's pass.
- Basis: ADR-013, ADR-016, PERF-004, PERF-049, PERF-054 [verified].
- Owner decision: no.
- Changes: ADR-013 scrub list; ADR-004 and ADR-016 vmstat rows; PERF-049 response-time
  line; PERF-054 order; 00-epics section 6 (PERF-037); U-51. hjn.17: the mapping table.

### OI-E25 E05 collect-process choices and the P2 column readers
- Raised by: E05 items 5, 7 to 13 (items 1 to 4 and 6 are in OI-E03, OI-E04 and OI-E06).
- Question: the package index, the P2 per-process columns and the smaller choices.
- Answer:
  1. Package index (item 5): new file `crates/tl-collect/src/pkg.rs`, owner E05, new bead
     E05-030 "package owner index" (P2): batched `pacman -Qoq <paths…>` for unseen exe paths
     debounced 2 s, the `/var/lib/pacman/local/<pkg>-<ver>/desc` parser (`%VERSION%`, `%DESC%`,
     `%PACKAGER%`, `%BUILDDATE%`, `%URL%`), cache keyed by exe path and mtime, invalidated on
     the `local` directory mtime; `dpkg -S` and `rpm -qf` behind the same trait (P4). The
     PROC-078 integrity check is `pacman -Qkk <pkg>` run by the detail worker (no mtree
     parsing, no gzip crate). SYS-020's Publisher uses the same index (E06 item 6: one E06
     bead depends on E05-030).
  2. P2 column readers (item 5, second half): new bead E05-031 "P2 per-process column
     readers" in `proc/extras_p2.rs`: `statx` mtime and size of the exe, ELF class and
     machine, `sessionid`, `attr/current`, elevation and sandbox rules (PROC-028), scheduling
     policy text, `oom_score`, `oom_score_adj`, TTY decode, `cwd`, context switches over
     `task/*/status` for `detail_keys`. Values go into a second index-aligned slice
     `ProcessSnapshot.extras_p2: Arc<[ProcessExtrasP2]>` filled only for `detail_keys`
     (ADR-008 visible-rows rule). `ProcessExtrasP2` is defined in tl-model `process.rs` with
     those fields; E03-008 gets the list from the bead.
  3. Item 7: PROC-005's inotify sentence is dropped; the app scan runs every sweep.
  4. Item 8: "not dumpable" when `/proc/<pid>` is owned by uid 0 while the real uid is not 0,
     else "Not reported" (PROC-082 edit).
  5. Item 9: U-21 belongs to E09 with the rule "`irq` stays 0 across 10 samples"; no
     `/proc/config.gz` read.
  6. Item 10: E05-016 (yms.19) checks Mission Center's `matching.rs:10-24` table before it
     starts; the leader pastes the table into the bead.
  7. Item 11: P4, decide then.
  8. Item 12: the three `Cargo.toml` lines are allowed (OI-E04 item 15).
  9. Item 13: the reducer computes the PROC-034 power estimate; new bead E09-030 "process
     power estimate" (P2, file `reducer/power.rs`; depends on 8q0.1, hjn.11 (RAPL), ndy.2),
     writing `ProcessExtrasP2.power_w: Option<f32>` for every row (it is cheap: one multiply per
     process per sweep) and the 2-minute trend.
- Basis: PROC-005, PROC-027, PROC-028, PROC-034, PROC-036, PROC-078, PROC-082, SYS-020,
  ADR-008 [verified].
- Owner decision: no.
- Changes: PROC-005 inotify sentence; PROC-082 rule; PROC-078 mechanism (`pacman -Qkk`);
  00-epics section 2 (`pkg.rs`, `proc/extras_p2.rs`, `reducer/power.rs`). New beads E05-030,
  E05-031, E09-030 as above. as2.10: `ProcessExtrasP2`. yms.19: the table.

### OI-E26 E06 collect-netsys choices
- Raised by: E06 items 5, 8, 9, 10 (items 1, 2, 3, 4, 6, 7, 11 are in OI-E04, OI-E06 and
  OI-E25).
- Question: the remaining E06 choices.
- Answer: item 5 (xe VRAM total file): new U-52 "xe VRAM total sysfs file"; E06-023 (hx1.26)
  leaves VRAM total out for xe until it is named; item 8: E06 owns the PERF-037 readers
  (OI-E24); item 9: E12-027 (hjn.31) may depend on hx1.26 directly (named exception); item 10:
  all six small choices accepted (IPv6 scope as interface name, autostart precedence
  NotInThisDesktop then MissingProgram then Disabled, 64 KiB read caps, fdinfo busy summed and
  clamped to 100%, first attribution pass WarmingUp, pacman.log timestamp form).
- Basis: PERF-064, PERF-075, SYS-022, PROC-032 [verified].
- Owner decision: no.
- Changes: U-52. `br dep add tasklens-hjn.31 tasklens-hx1.26`.

### OI-E27 E07 ops choices
- Raised by: E07-O2, E07-O3, E07-O5, E07-O6 (O1 and O4 are OI-E07 and OI-E08).
- Question: the remaining tl-ops choices.
- Answer: O2: SYS-022's rule holds (delete the user file when it carries
  `X-Tasklens-Override=true`, else set `Hidden=false` and remove
  `X-GNOME-Autostart-enabled=false`); the ADR-016 row changes to match. O3:
  `SCHED_RESET_ON_FORK` is set for every FIFO and RR request (accepted; ADR-006 sentence
  generalised). O5: unset or empty `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR` are omitted
  from the `pkexec env` line (PROC-095 sentence). O6: new overrides mode 0600, existing mode
  kept, desktop files over 1 MiB refused (accepted).
- Basis: SYS-022, ADR-016, ADR-006, R-038, PROC-095, ADR-011 [verified].
- Owner decision: no.
- Changes: ADR-016 autostart re-enable row; ADR-006 reset-on-fork sentence; PROC-095 env
  sentence.

### OI-E28 Engine runtime and reducer choices from B3
- Raised by: B3 items 1, 2, 3, 4, 7, 8, 15, 16 (b); E12-OI-3; E12-OI-8; E12-OI-9; E12-OI-10;
  E12-OI-11.
- Question: the remaining engine-core and E12 choices.
- Answer:
  - B3-1: `WorkerIo.control: watch::Receiver<WorkerControl>` with `reset_generation` and the
    newest-64 `QueuedCommand` queue (accepted; ADR-004 sentence).
  - B3-2: `EngineConfig` per OI-E12 item 3.
  - B3-3: the E08-003 payloads are the contract for B4; the leader hands 8hz.5's text to the
    E12 and E14 beads (already the case in the bead texts read).
  - B3-4: `EngineMsg::Tick` every 1 s (accepted; ADR-004 sentence).
  - B3-7: the E05 sweep fills `ProcessSweep.file_nr` from `/proc/sys/fs/file-nr`; the
    reducer copies it into `TableTotals.handles`.
  - B3-8: `dbus/mod.rs` owns `ManagerConnSlot` and `SharedHandles`; `WorkerIo.shared`;
    `EngineHooks.action_gate: Fn(HookCtx) -> Box<dyn ActionGate>`; E14-001 (y6k.5) uses the
    E08 type (accepted; ADR-004 paragraph).
  - B3-15: service jobs have no client-side timeout (ADR-005 wins); the method call has 10 s
    (user) or 120 s (system); ADR-006's 30 s row is edited.
  - B3-16 (b): `scripts/ci.sh` gets the line `cargo build -p tl-gpu-probe --features fake`
    before the engine tests, written up front by E02-009 (v9m.11) since the file is E02's; the
    probe crate exists from E02-004.
  - E12-OI-3: `cpu_period` and `memory_period` return 1 s when `Demand.process_table` is true
    (accepted; ADR-008 schedule note).
  - E12-OI-8: the reducer keeps the running maximum of `Committed_AS` since start and writes
    `MemoryInfo.peak_committed_bytes`; the inventory worker leaves it `WarmingUp`.
  - E12-OI-9: "Session type" shows the raw `XDG_SESSION_TYPE`, "Windowing system" the display
    name (PERF-097 edit).
  - E12-OI-10: inventory rows carry `device: None` from the worker; the reducer matches disk
    name, PCI slot or interface name against the catalog.
  - E12-OI-11: AMD and Intel GPUs use `DeviceIdentity::Gpu { uuid: [0; 16], pci_bus_id }`;
    process GPU columns match on the full identity.
  - Battery demand, 24 h history at 288 points, power-profiles-daemon bus names: accepted.
- Basis: ADR-004, ADR-005, ADR-006, ADR-008, ADR-013, PERF-078, PERF-097, PERF-098 [verified].
- Owner decision: no.
- Changes: ADR-004 (control channel, `Tick`, `SharedHandles`, `HookCtx`); ADR-006 job row;
  ADR-008 process-table note; PERF-097 wording; PERF-098 "peak" mechanism. v9m.11: the
  ci.sh line.

### OI-E29 E14 D-Bus worker choices
- Raised by: E14 items 2, 4, 5, 6, 8, 9, 10, 12, 14, 15, 16 (items 1, 3, 7, 11, 13 are in
  OI-E18, OI-E04, OI-E11, OI-E10, OI-E06).
- Question: the remaining E14 choices.
- Answer: item 2: settled by B3-8 and the `all_workers()` registry (one `WorkerSpec` line per
  worker; the journal opener is one allowed line in `bridge.rs`); item 4: the systemd worker
  reads unit `memory.stat` and `cpu.stat` with E05-A2's readers (R-138 item 16 sentence
  edited); E05-A1 and E05-A2 join E14's anchor row (section 4); item 5: the engine writes its
  connection's unique name into the KWin script as `__TL_BUS_NAME__` and accepts `Update` only
  from the owner of `org.kde.KWin` (accepted; ADR-004 KWin paragraph); item 6: the extra
  KWin signals (`minimizedChanged`, maximize change, `keepAboveChanged`,
  `interactiveMoveResizeFinished`) are accepted (PROC-052 list extended); item 8: the
  `journalctl ... --reverse --cursor=<oldest> -n 201` form, verified by E14-010 (y6k.14);
  item 9: `Access(OwnOnly)` for system units only, `bounded(64)`, missing `PRIORITY` = 6;
  item 10: no column list in `SysTableQuery`; the Services filter matches every text column of
  a unit; sort orders follow the SYS-003 tables (APP-028 sentence edit); item 12: 8 lowercase
  hex digits from 4 bytes of `/dev/urandom` for `<rand>` (table (c) and SYS-023); item 14:
  retry a missing manager on `RefreshNow` or every 60 s; item 15: `Update` payloads over 1 MiB
  rejected, strings cut at 4 KiB; item 16: the OpenRC word mapping and 5 s refresh (P4).
- Basis: ADR-004, ADR-010, PROC-052, SYS-003, SYS-005, SYS-018, SYS-023, APP-028, R-138
  [verified].
- Owner decision: no.
- Changes: R-138 item 16; 00-epics section 4 (E14 anchor row); ADR-004 KWin paragraph;
  PROC-052 signal list; APP-028 filter sentence; table (c) and SYS-023 `<rand>` length.

### OI-E30 E29 helper and setup choices
- Raised by: E29-OI-8, OI-10 to OI-13, OI-15 to OI-19 (OI-1 to OI-7, OI-9, OI-14 are in
  OI-E19, OI-E07, OI-E10, OI-E11, OI-E12, OI-E09, OI-E04).
- Question: the remaining E29 choices.
- Answer: OI-8: the helper cgroup validation rule of E29-003 (qfx.6) is accepted and E07
  exposes the write helpers that take a validated dirfd; OI-10: `render(&RecoveryEdit) -> String`
  lives in a new tl-ops file `crates/tl-ops/src/dropin.rs`, written by E29-009 (qfx.12) as an
  E29-owned file inside tl-ops (section 2 row), and E11-020 (i3o.22) uses it; OI-11: E16 and
  E23 build the "Retry as administrator" and "Grant access" buttons from their own specs and
  E29-029 (qfx.31) tests them (UI side; noted for the UI file); OI-12: the chain edits
  E29-017 (qfx.20) and E29-018 (qfx.21) are allowed; OI-13: `tl-engine/src/helper/setup.rs`
  is an E29 file and the `TASKLENS_SETUP_ROOT` test mode (honoured only when not root) is
  accepted; OI-15: the setup script is embedded with `include_str!` and shown whole; OI-16:
  the three polkit prompt texts proposed by E29-021 are recommended; OI-17: cr0.18 (E33-015)
  depends on qfx.24 (E29-021) and qfx.34 (E29-022) as well as qfx.1; OI-18: UI-PRF-052 for
  the cards, UI-SHL-094 for the frame (UI side); OI-19: E29-030 (qfx.32) may chain-edit
  `window/main.rs` and uses the first-focus modifier rule on X11 too.
- Basis: ADR-006, R-004, SYS-013, PERF-108, table (c), UI-SHL-094, UI-PRF-052 [verified].
- Owner decision: yes, for OI-16 only (polkit prompt wording is user-visible product text;
  recommended: the E29-021 texts; alternative: shorter one-line messages). The rest: no.
- Changes: 00-epics section 2 (`tl-ops/src/dropin.rs`, `helper/setup.rs`, the two worker
  chain edits, `window/main.rs` E29 row); table (c) note on `TASKLENS_SETUP_ROOT`. Edges
  `br dep add tasklens-cr0.18 tasklens-qfx.24`, `br dep add tasklens-cr0.18 tasklens-qfx.34`.

### OI-E31 E30 history choices
- Raised by: E30-OI-2, OI-3, OI-5, OI-6, OI-7, OI-8 (OI-1 and OI-4 are in OI-E18 and
  OI-E12).
- Question: the on-disk block fields and format details, and the two data shapes.
- Answer: OI-2: `HistMeta { exe_path, user, uid, system }`,
  `Block { gap_before, mem_total_kib, catalog: Option<Vec<HistDevice>> }`,
  `HistDevice { idx, kind, parent, name }` are accepted and ADR-012 is edited to match before
  any file exists; OI-3: all nine on-disk details accepted as written (little-endian header
  offsets 0, 4, 6, 14, 22; lowercase hex boot dir; postcard footer with `TLHF` magic;
  `FieldPatch` of `Option`s; keyframe-then-started index space; `MetaId` per segment; NaN gap
  marker; non-`Valid` stored as 0; `deleted-keys.bin` mode 0600); ADR-012 gains these lines;
  OI-5: per bucket the three largest per-process values plus "other" as the sum of the rest,
  drawn in the metric colour; header totals for a selected range are averages over the range;
  OI-6: resend a query after 1,000 ms without an answer; Copy details on a history row prints
  Name, PID, Parent PID, CPU, Memory, Disk, Network and GPU, Copy all metrics every visible
  history column, never a command line; OI-7: E20 declares `ToggleHistoryMode` and selects the
  Processes tab, E30-A3 flips the mode; E16's aggregator sets `Demand.recording` from
  `history.enabled`; OI-8: "-M:SS" axis labels under an hour and the disk-limit input snapping
  to multiples of 64.
- Basis: ADR-012, R-136, PROC-089, UI-PRC-030 to UI-PRC-036, UI-SHL-023 [verified].
- Owner decision: yes, for OI-5 (the stacked "top-3 plus other" chart is a visual design not in
  any spec; recommended as above; alternative: a plain total line with the top-3 only in the
  hover card). The rest: no.
- Changes: ADR-012 (`HistMeta`, `Block`, `HistDevice`, the nine format lines); PROC-089 or
  UI-PRC-032 (the stack rule once confirmed); UI-PRC-036 (range totals are averages).

## New beads

| Plan key | Title | Epic | Phase | Files | Depends on |
|---|---|---|---|---|---|
| E04-030 | udev database reader (`read_udev_props`) | E04 | P1 | `crates/tl-collect/src/parse/udev.rs`, `parse/mod.rs` line, fixture `run/udev/data/` | 2dl.1 |
| E04-031 | Partitions table rows (PERF-050) | E04 | P2 | `crates/tl-collect/src/parse/partitions.rs`, `parse/mod.rs` line | 2dl.24, 2dl.27, E04-030 |
| E04-A4 | Anchor: P2 system details | E04 | P2 | none (closes 2dl.16, 2dl.17, 2dl.18, 2dl.22, 2dl.26, 2dl.27, 2dl.31, E04-031) | those beads |
| E04-032 | `minimal-vm` fixture capture | E04 | P1 | `crates/tl-collect/tests/fixtures/minimal-vm/` | 2dl.1 |
| E05-030 | Package owner index (`pkg.rs`) | E05 | P2 | `crates/tl-collect/src/pkg.rs`, `lib.rs` line | yms.1 |
| E05-031 | P2 per-process column readers | E05 | P2 | `crates/tl-collect/src/proc/extras_p2.rs`, `proc/mod.rs` line | yms.1, E05-030 |
| E09-029 | Per-process series from `Demand.graph_targets` | E09 | P2 | `crates/tl-engine/src/reducer/process_series.rs`, `reducer/mod.rs` line | 8q0.1, as2.2 |
| E09-030 | Process power estimate (PROC-034) | E09 | P2 | `crates/tl-engine/src/reducer/power.rs`, `reducer/mod.rs` line | 8q0.1, hjn.11, ndy.2 |

## Edges

Each line is a `br dep add <bead> <depends-on>` for the leader, with the reason.

```
br dep add tasklens-cr0.1   tasklens-fut.15    # OI-E15: E33-A1 calls tl_ui::run(UiLaunch)
br dep add tasklens-cr0.14  tasklens-fut.15    # OI-E15: shutdown reads UiExit
br dep add tasklens-cr0.15  tasklens-fut.15    # OI-E15: RendererFailed and DeviceLost paths
br dep add tasklens-cr0.11  tasklens-iuz.13    # OI-E08: settings save needs write_atomic at P1
br dep add tasklens-cr0.17  tasklens-iuz.13    # OI-E08: autostart writer needs write_atomic at P1
br dep add tasklens-qfx.17  tasklens-i3o.4     # OI-E09: PrivilegedOps trait before its implementation
br dep add tasklens-m1f.11  tasklens-y6k.3     # OI-E14: "Open Full tasklens" uses the launcher
br dep add tasklens-wox.11  tasklens-jqk.1     # OI-E23: chain edit into E20's window files
br dep add tasklens-hjn.31  tasklens-hx1.26    # OI-E26: Intel GpuDrm worker needs the i915/xe readers
br dep add tasklens-cr0.18  tasklens-qfx.24    # OI-E30: PKGBUILD installs the polkit policy
br dep add tasklens-cr0.18  tasklens-qfx.34    # OI-E30: PKGBUILD installs tasklens-setup
br dep add tasklens-wlf.10  tasklens-pdu.1     # OI-E22: table-2000 drives the production table
br dep add tasklens-wlf.10  tasklens-rsf.1     # OI-E22: TlTreeTable::scroll_handle
br dep add tasklens-wlf.9   tasklens-crw.1     # OI-E22: CpuPage.bench_show_both
br dep add tasklens-wlf.9   tasklens-yrf.1     # OI-E22: paint record behind bench
br dep add tasklens-wlf.25  tasklens-ndy.2     # OI-E22: GPU power baseline through the probe
br dep add tasklens-wlf.19  tasklens-jqk.10    # OI-E22: first_frame_ms log line
br dep add tasklens-jqk.26  tasklens-hjn.22    # OI-E01: startup auto-fetch sends the command the worker handles
br dep add tasklens-o55.22  tasklens-hjn.22    # OI-E01: Fetch button and public_ip state
br dep add tasklens-9o0.32  tasklens-hjn.22    # OI-E01: Information Public IP row
br dep add tasklens-o55.18  tasklens-hjn.23    # OI-E01: SMART and Eject button visibility fields
br dep add tasklens-o55.19  tasklens-hjn.24    # OI-E01: SMART window data
br dep add tasklens-o55.20  tasklens-hjn.25    # OI-E01: Disk is in use window blockers
br dep add tasklens-o55.34  tasklens-hjn.32    # OI-E01: smartctl fallback button
br dep add tasklens-o55.33  tasklens-hjn.33    # OI-E01: battery history chart
br dep add tasklens-o55.17  <E04-A4>           # OI-E02: Partitions table block reads PartitionRow
br dep add tasklens-hjn.16  <E04-030>          # OI-E02: disk details from udev
br dep add tasklens-hjn.16  <E04-031>          # OI-E02: partition rows
br dep add tasklens-hjn.17  <E04-030>          # OI-E02: adapter name from udev
br dep add tasklens-hjn.21  <E04-030>          # OI-E02: inventory disk and adapter rows
br dep add tasklens-hjn.29  <E04-030>          # OI-E02: GPU names from udev
br dep add <E09-029>        tasklens-8q0.1     # OI-E16: reducer anchor
br dep add <E09-029>        tasklens-as2.2     # OI-E16: ProcessSeriesId and the component
br dep add tasklens-wox.9   <E09-029>          # OI-E16: tray process graphs use graph_targets
br dep add <E09-030>        tasklens-8q0.1     # OI-E25: reducer anchor
br dep add <E09-030>        tasklens-hjn.11    # OI-E25: RAPL package power
br dep add <E09-030>        tasklens-ndy.2     # OI-E25: GPU power and per-process SM share
```

Edges to remove (leader command): cr0.11 and cr0.17 on iuz.3 (OI-E08). Beads to close as
duplicates: wlf.3 and wlf.4 (OI-E20). E26-011 (per-process minigraph cards, P3) also depends on
E09-029; its bead ID is in the UI file.

## Summary

| OI | Title | Owner decision | Beads unblocked |
|---|---|---|---|
| OI-E01 | Device commands, device details, public IP state | no | hjn.7, hjn.22 to hjn.26, hjn.32, hjn.33, hjn.2 (E12-A2) and the 21 E12 beads behind E12-OI-1; o55.18, o55.19, o55.20, o55.22, o55.33, o55.34; 9o0.32; jqk.26 |
| OI-E02 | udev data source and the Partitions table | no | hjn.16, hjn.17, hjn.21, hjn.29 (once E04-030 exists); o55.17 |
| OI-E03 | ProcessRecord extras, group rows, table fields | no | yms.1, 8q0.7, 8q0.11 to 8q0.15, the E22 column beads |
| OI-E04 | Crate pins and allowed-dependency rows | no | cr0.4, cr0.6, m1f.1, hjn.22, hjn.32, y6k.4, y6k.13, wlf.6, dwy.4 (CLK_TCK) |
| OI-E05 | `unsafe` in tl-gpu-probe api_versions.rs | no | ndy.8 |
| OI-E06 | Other `unsafe` or libc needs | no | yms (strings scanner), iuz.8 |
| OI-E07 | Core dump tool and the passed fd | no | iuz.15, iuz.3, qfx.9, i3o.20 |
| OI-E08 | `write_atomic` phase | no | cr0.11, cr0.17 (P1 path) |
| OI-E09 | Privileged helper hook | no | qfx.17 |
| OI-E10 | Helper session commands and action error values | no | qfx.16, qfx.23, i3o.13, i3o.17, i3o.18, i3o.20 |
| OI-E11 | Setup state and window pick answers | no | qfx.22, qfx.25, y6k.21, y6k.23 |
| OI-E12 | SourceUpdate variants and EngineConfig up front | no | qfx.23, E30-A2, m1f.8 |
| OI-E13 | PSI trigger window | no | m1f.1, m1f.3 |
| OI-E14 | Low-memory mode details | yes (OI-13 flag meaning) | m1f.4, m1f.6, m1f.11 |
| OI-E15 | UiLaunch shape | no | cr0.1, cr0.2, cr0.14, cr0.15 |
| OI-E16 | Aggregate devices and per-process series | no | E26-003, E26-011, wox.9 |
| OI-E17 | tl-model core types review | no | as2.1 and its 13 beads; ndy.12 |
| OI-E18 | tl-model contract types review | no | as2.2 and its 22 beads; E30 beads; y6k.1 |
| OI-E19 | Helper protocol shape | no | as2.36, qfx.1 to qfx.16 |
| OI-E20 | E02 workspace choices | no | v9m.1 to v9m.12 |
| OI-E21 | E33 binary and packaging choices | yes (OI-6, OI-10, OI-13) | cr0.16, cr0.19, cr0.21, cr0.22, cr0.3 |
| OI-E22 | E01 bench harness choices | no | wlf.6 to wlf.26 |
| OI-E23 | E31 tray choices | no | wox.3, wox.10, wox.11 |
| OI-E24 | E04 collect-system choices | no | 2dl.24, 2dl.29, hjn.17 |
| OI-E25 | E05 choices and P2 column readers | no | yms.19, E05-030, E05-031, E09-030 |
| OI-E26 | E06 choices | no | hjn.31 |
| OI-E27 | E07 choices | no | iuz.8, iuz.14, iuz.17 |
| OI-E28 | Engine runtime and reducer choices | no | 8hz.3, 8hz.6, v9m.11, hjn.12, hjn.19 to hjn.21, hjn.29 |
| OI-E29 | E14 D-Bus worker choices | no | y6k.5, y6k.7, y6k.12 to y6k.14, y6k.18, y6k.20, y6k.22 |
| OI-E30 | E29 helper and setup choices | yes (OI-16 prompt texts) | qfx.6, qfx.12, qfx.20, qfx.21, qfx.22, qfx.24, qfx.28, qfx.32 |
| OI-E31 | E30 history choices | yes (OI-5 chart shape) | E30-002 onward (engine side) |

Owner decisions in one list: OI-E14 (`--force-full-memory-mode` meaning), OI-E21 (`--app-id`
scope, PKGBUILD licence, TaskSlinger INI encodings), OI-E30 (polkit prompt texts), OI-E31
(history stacked chart shape). Every one has a recommended answer above and no bead waits on
it except cr0.22 (`--app-id`).
