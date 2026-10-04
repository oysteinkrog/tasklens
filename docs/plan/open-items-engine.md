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
