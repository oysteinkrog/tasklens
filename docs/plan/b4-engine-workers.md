# B4 engine-workers beads: plan keys and bead IDs

Batch B4 covers epics E12, E14, E29 and E30 (`docs/plan/00-epics.md` section 5): the system and device workers, the D-Bus workers, the privileged helper and its setup, and the history store. The leader created the epic and anchor beads (`docs/plan/anchors.md`). B4 replaced each anchor's placeholder with the full template and created every other bead under its epic.

Totals: E12 33 beads, E14 25, E29 34, E30 31 (123 task beads, anchors included). Bead titles start with the plan key. Every bead carries the build rule in its Notes. E12, E30 and most E29 beads already use `heavy-build`; the leader's end pass rewrites the rest.

Conventions used by all four epics:

- **Dependency rule.** Edges inside B4 point at any bead. Edges to other batches point at anchors, except where a needed bead sits outside every anchor; those direct edges are named in the epic's section. The "Depends on" column lists plan keys; anchor IDs are in `docs/plan/anchors.md`.
- **Open items block, they do not guess.** A bead that needs an undecided contract change carries the `blocked-open-item` label. Each epic's open items list a recommendation.

## E12 engine-system-workers (tasklens-hjn)

33 beads: 3 anchors and 30 others. Code is in `crates/tl-engine/src/workers/`, plus two new `tl-collect` files that E12 owns (`net/addrs.rs` and `net/nl80211.rs`, E12-OI-5). The udev database reader several beads need has no owner yet (E12-OI-4).

- Sampling sources (the E08 `Source` trait, run by `source::run`): `cpu.rs` (E12-001, E12-007 to E12-010), `memory.rs` (E12-003, E12-011), `disk.rs` (E12-004, E12-012, E12-013), `net.rs` (E12-005, E12-014, E12-015), `sensors.rs` (E12-006, E12-030), `gpu_drm.rs` (E12-026 AMD, E12-027 Intel). `enumerate.rs` (E12-002) holds the shared 5 s device rescan and the identity rules.
- Event-driven workers with their own loop: `inventory.rs` (E12-016 to E12-019, with `public_ip.rs`), `dbus_udisks.rs` (E12-020 to E12-022, with `eject.rs` and the P3 `smartctl.rs` of E12-028), `dbus_upower.rs` (E12-023, E12-029), `dbus_nm.rs` (E12-024, E12-025).
- Anchors: E12-A1 registers the CPU worker and proves the 10 Hz, 1 Hz and off schedule in LabRuntime. E12-A2 closes the P1 sources and enumeration (hidden with recording off: zero samples). E12-A3 closes every P2 worker and runs them once against the owner machine.
- Each worker bead adds its own `pub mod` line and one `all_workers()` entry in `workers/mod.rs`. Those are one-line edits; beads that can run in parallel touch different lines of that file.
- Tests: LabRuntime and fixture tests in `crates/tl-engine/tests/lab_system_workers.rs` (created by E12-001) and in `#[cfg(test)]` modules; owner-machine tests are `#[ignore]` in `tests/real_inventory.rs` (created by E12-016). Fixtures come from `crates/tl-collect/tests/fixtures`, copied under `CARGO_TARGET_TMPDIR` by the E12-001 helpers, so no `tempfile` dev-dependency is needed.
- Cross-epic edges point at anchors, except seven P2 E04 beads and one P3 E06 bead that sit outside every anchor: E04-013 (tasklens-2dl.16), E04-014 (tasklens-2dl.17), E04-015 (tasklens-2dl.18), E04-019 (tasklens-2dl.22), E04-023 (tasklens-2dl.26), E04-024 (tasklens-2dl.27), E04-028 (tasklens-2dl.31) and E06-023 (tasklens-hx1.26). The beads that need them carry a direct edge.
- 21 beads carry the `blocked-open-item` label. Almost all wait on E12-OI-1, the contract additions below. That includes E12-004 to E12-006, so E12-A2 (P1) is blocked until E12-OI-1 A is settled.

| Plan key | Bead ID | Title | Priority | Depends on |
|---|---|---|---|---|
| E12-A1 | tasklens-hjn.1 | CPU worker | 0 | E12-001, E08-A1, E09-A1 |
| E12-A2 | tasklens-hjn.2 | Memory, disk, net, sensors workers and enumeration | 1 | E12-002, E12-003, E12-004, E12-005, E12-006, E12-A1, E08-A1, E09-A1 |
| E12-A3 | tasklens-hjn.3 | Inventory and the P2 D-Bus device workers | 2 | E12-008, E12-009, E12-010, E12-011, E12-012, E12-013, E12-014, E12-015, E12-016, E12-017, E12-018, E12-019, E12-020, E12-021, E12-022, E12-023, E12-024, E12-025, E12-026, E12-030, E12-A2 |
| E12-001 | tasklens-hjn.4 | CPU source: workers/cpu.rs with demand-driven 10 Hz, 1 Hz and off rates | 0 | E04-A2, E08-A1, E03-A2 |
| E12-002 | tasklens-hjn.5 | Device enumeration: workers/enumerate.rs with 5 s rescans and stable identities | 1 | E04-A3, E08-A1, E12-001 |
| E12-003 | tasklens-hjn.6 | Memory source: workers/memory.rs | 1 | E04-A3, E08-A1, E12-002, E12-001 |
| E12-004 | tasklens-hjn.7 | Disk source: workers/disk.rs with device list and rates | 1 | E04-A3, E08-A1, E12-002 |
| E12-005 | tasklens-hjn.8 | Net source: workers/net.rs with interface list and rates | 1 | E04-A3, E08-A1, E12-002 |
| E12-006 | tasklens-hjn.9 | Sensors source: workers/sensors.rs for hwmon temperatures and fans | 1 | E04-A3, E08-A1, E12-002 |
| E12-007 | tasklens-hjn.10 | CPU live stats: processes, threads, handles, load average | 1 | E12-A1, E04-A3 |
| E12-008 | tasklens-hjn.11 | CPU package and core power from RAPL | 2 | E12-007, E04-015 |
| E12-009 | tasklens-hjn.12 | CPU and memory at 1 Hz while the process table is shown | 2 | E12-008, E12-A2 |
| E12-010 | tasklens-hjn.13 | CPU static info: caches, virtualization, security, power profile | 2 | E12-009, E06-A3, E08-A2, E04-013, E04-014 |
| E12-011 | tasklens-hjn.14 | Memory details: speed, slots, form factor, swap and zram rows | 2 | E12-009, E04-019 |
| E12-012 | tasklens-hjn.15 | Disk temperatures and filesystem usage | 2 | E12-A2, E04-023, E04-024 |
| E12-013 | tasklens-hjn.16 | Disk details from udev and the partition table | 2 | E12-012 |
| E12-014 | tasklens-hjn.17 | Net details: link speed, state, MAC, adapter name, kind | 2 | E12-A2, E04-028 |
| E12-015 | tasklens-hjn.18 | IP addresses from an rtnetlink RTM_GETADDR dump | 2 | E12-014, E06-A1 |
| E12-016 | tasklens-hjn.19 | Inventory worker: scan loop, OS, processor, memory, modules, firmware, board | 2 | E06-A3, E04-A2, E04-A3, E08-A1, E09-A1 |
| E12-017 | tasklens-hjn.20 | Inventory System section from hostnamed, logind and the environment | 2 | E12-016, E08-A2, E06-A3, E05-A1 |
| E12-018 | tasklens-hjn.21 | Inventory Disk drives, Graphics and Network adapters sections | 2 | E12-017, E12-013, E12-014, E12-015, E06-A3, E04-024 |
| E12-019 | tasklens-hjn.22 | Public IP fetch with ureq on request or at startup | 2 | E12-016, E02-A1, E08-A1, E12-018 |
| E12-020 | tasklens-hjn.23 | UDisks2 worker: drive properties, SMART kind, eject flags, SMART temperature | 2 | E12-A2, E08-A2, E12-016 |
| E12-021 | tasklens-hjn.24 | UDisks2 SMART read and refresh for ATA and NVMe | 2 | E12-020, E03-A2 |
| E12-022 | tasklens-hjn.25 | Disk eject through UDisks2 with busy-process report | 2 | E12-020, E03-A1, E12-021 |
| E12-023 | tasklens-hjn.26 | UPower worker for batteries with a sysfs fallback | 2 | E12-A2, E08-A2, E03-A1, E12-016 |
| E12-024 | tasklens-hjn.27 | NetworkManager worker: device kind, connection name, Wi-Fi details | 2 | E12-014, E08-A2, E03-A1, E12-016 |
| E12-025 | tasklens-hjn.28 | Wi-Fi signal and bit rates from nl80211 | 2 | E12-024, E12-015, E06-A1 |
| E12-026 | tasklens-hjn.29 | GpuDrm worker for AMD cards | 2 | E12-A2, E06-A3, E08-A2, E03-A1 |
| E12-027 | tasklens-hjn.31 | GpuDrm worker for Intel i915 and xe cards | 3 | E12-026, E06-023 |
| E12-028 | tasklens-hjn.32 | SMART read with smartctl through pkexec | 3 | E12-021, E06-A2, E12-022 |
| E12-029 | tasklens-hjn.33 | Battery charge history from UPower | 3 | E12-023 |
| E12-030 | tasklens-hjn.30 | Fan control mode in sensor readings | 2 | E12-006, E04-A3 |

### Open items

Each item has a recommendation. Beads that wait on one carry the `blocked-open-item` label and say "stop if missing".

- **E12-OI-1 Contract additions for device details (blocks 21 beads, including the P1 anchor E12-A2).** The E03 and E08 contracts have no way to send a device list next to a sample, no field for disk temperature, link state or fan control mode, no device details component, no battery channel, and no commands for SMART, eject, public IP or battery history. Recommendation, in six parts (the full type sketch is in the B4 scratch notes and is repeated in each blocked bead):
  - A. E08 `source.rs`: `fn take_extra(&mut self) -> Option<SourceUpdate> { None }` on `Source`. `source::run` calls it until `None` after each `sample()` and sends those updates before the sample's own. This is how `Devices` and `GpuInfo` lists travel (E12-004, E12-005, E12-006, E12-026).
  - B. E03: `LinkState`, `FanControlMode { FullSpeed, Manual, Automatic }`, `DiskTotals.temp_c`, `NetTotals.state`, `SensorReading.fan_control` (E12-012, E12-014, E12-030).
  - C. E03 `snapshot.rs`: `DeviceDetailsSnapshot` with `CpuStaticInfo`, `MemoryDetails`, `DiskDetails` (partitions, `SmartData`, `EjectState`, `smart_temp_c`), `NetDetails` (`WifiInfo`), `BatteryDetails`, and `ComponentMask::DEVICE_DETAILS = 1 << 13`.
  - D. E08 `update.rs`: `SourceUpdate::DeviceDetails(DeviceDetailsUpdate { Cpu, Memory, Disks { from, entries }, Nets { from, entries }, Batteries })`. The reducer (E09) merges the sysfs and D-Bus parts per device (D-Bus wins for model, serial, SMART, kind refinement, Wi-Fi) and appends `BATTERY_PCT` from `BatteryDetails.percentage`.
  - E. E03 `command.rs` and E08 `WorkerCommand`: `Command::Device(DeviceCommand { SmartRead { disk, update }, SmartReadAdmin { disk }, Eject { seq, disk }, FetchPublicIp, BatteryHistory { native_path } })`, routed to `Udisks`, `Inventory` and `UPower`.
  - F. E03 `EngineSettings.auto_fetch_public_ip: bool` (default false).
  Owner: the E03 and E08 writers (B1 and B3), as one chain edit each.
- **E12-OI-2 `ureq` in tl-engine.** R-131 puts the public IP code in tl-engine, but tl-engine's `Cargo.toml` has no `ureq` line. Recommendation: E12-019 adds `ureq = { workspace = true }` as a chain edit on E08-A1, and runs `cargo xtask check-deps` to settle U-30 (no Tokio). E12-028 needs `serde_json` the same way (same question as E14's open item).
- **E12-OI-3 Demand while only the process table is shown.** The CPU and Memory pseudo-rows of the process table need 1 Hz CPU and memory totals when no performance page is visible. Recommendation: `cpu_period` and `memory_period` also return 1 s when `Demand.process_table` is true (E12-009). No contract change.
- **E12-OI-4 udev database reader.** Disk model, serial, partition labels and GPU and adapter names come from `/run/udev/data/*`, and no tl-collect bead reads it. Recommendation: one new tl-collect bead in B2 (E04) for `crates/tl-collect/src/parse/udev.rs`: `read_udev_props(path, keys, out)` copying the listed `E:KEY=value` lines, 64 KiB cap. E12-013, E12-014 and E12-018 are blocked until it exists; E12-026 uses it when present and otherwise shows a generic GPU name. The leader decides whether B2 adds that bead or E12-013 takes the file.
- **E12-OI-5 Netlink readers in tl-collect.** IP addresses (`RTM_GETADDR`) and Wi-Fi (`nl80211`) have no tl-collect file. Recommendation: `crates/tl-collect/src/net/addrs.rs` (E12-015) and `crates/tl-collect/src/net/nl80211.rs` (E12-025), owned by E12, with one `pub mod` line each in `net/mod.rs` as chain edits on E06-A1.
- **E12-OI-6 Fan persistent key.** PERF-005's key `fan:<hwmon name>:<device path>:<n>` needs the chip's device path, which `DeviceIdentity::Fan` does not hold. Recommendation: `FanReading.chip_device_path` already exists in E04; E03 adds it to the identity or the catalog entry. Same question as B1's persistent key open item.
- **E12-OI-7 Interface kind mapping.** tl-collect's `IfaceKind` has Bridge, USB network and Cellular, which `NetKind` lacks. Recommendation: Bridge → `Virtual`, USB network → `Wired`, Cellular → `Other` (E12-014, E12-024). Same as B2's E04-OI-9.
- **E12-OI-8 Peak committed memory.** The Information tab's memory section shows a peak committed value, and nothing in the engine tracks it. Recommendation: the reducer keeps the running maximum of `committed` since start (E09), and the inventory snapshot leaves it `WarmingUp` (E12-016).
- **E12-OI-9 Session type and windowing system.** PERF-097 names `XDG_SESSION_TYPE` for both rows. Recommendation: "Session type" shows the raw value (`wayland`) and "Windowing system" the display name ("Wayland", "X11", "None (text console)") (E12-017).
- **E12-OI-10 Device index in inventory rows.** The inventory's disk, GPU and adapter rows have `device: Option<DeviceIdx>`, which a worker cannot know. Recommendation: workers send `None` and the reducer fills it by matching the disk name, PCI slot or interface name against the catalog (E12-018).
- **E12-OI-11 Per-process GPU use for AMD and Intel.** `GpuProcessSample` is keyed by a 16-byte `uuid`, which AMD and Intel cards lack. Recommendation: identify those cards by `pci_bus_id` with `uuid = [0; 16]` (E12-026) and have the process GPU columns match on the full identity. Per-process DRM use is not in any E12 bead.
- **Battery demand.** `Demand` has no battery field. The `UPower` worker runs while any surface is visible or recording is on (E12-023), which costs almost nothing because UPower is event-driven. If the UI later wants it tighter, E03 adds `Demand.batteries`.
- **Battery history span.** PERF-078 names the history chart but not its span. E12-029 asks UPower for 24 h at 288 points.
- **power-profiles-daemon bus names.** E12-010 reads `ActiveProfile` from `net.hadess.PowerProfiles` and falls back to `org.freedesktop.UPower.PowerProfiles`. The names are inferred from the daemon's docs, not from the spec.
- **Resume detection.** Sampling sources cannot send `ResumeDetected`; that stays with E08's logind worker.

## E14 engine-dbus-workers

25 beads: 4 anchors and 21 others. All are in `crates/tl-engine` (plus the three KWin scripts in `assets/kwin/`).

- systemd workers (`workers/dbus_systemd.rs`): E14-001 listing, E14-002 signals and resync, E14-003 cgroup pids and resource stats, E14-004 unit details, E14-008 OpenRC (P4). E14-A1 fills the Services body of `reducer/sys_table.rs` and closes the chain; E14-005 and E14-006 add the Startup and Connections projections. E14-007 is the real-system test with `tasklens-test.service`.
- Journal (`workers/journal.rs`): E14-A4 fetch, parsing, access and errors; E14-009 follow and Load older; E14-010 real-system test (U-49).
- Startup worker (`workers/startup.rs`): E14-011 scan, E14-012 live rescans, E14-013 unit state and impact.
- Connections worker (`workers/connections.rs`): E14-014 dump and attribution, E14-015 reverse DNS (P4).
- KWin bridge (`workers/dbus_kwin.rs`, `assets/kwin/*.js`): E14-016 bridge script, E14-017 one-shot templates, E14-A2 worker and `WindowsSnapshot`, E14-018 commands, E14-019 picker (P3).
- Opener and launcher: E14-020 `opener.rs`, E14-A3 `launcher.rs`, E14-021 real-system launcher tests.

Tests live in `#[cfg(test)]` modules next to the code, because `tl-engine`'s public API is only `EngineConfig`, `Engine::start`, `EngineHandle` and `ShutdownReport` (ADR-001 rule 2). The `tests/real_*.rs` files use only that public API. `tests/lab_dbus_workers.rs` is not used.

| Plan key | Bead ID | Title | Priority | Depends on |
|---|---|---|---|---|
| E14-A1 | tasklens-y6k.1 | systemd workers and system table rows (Services projection) | 2 | E09-A1, E03-A2, E14-004 |
| E14-A2 | tasklens-y6k.2 | KWin bridge | 2 | E14-016, E07-A3, E08-A1, E08-A2, E03-A2, E05-A1 |
| E14-A3 | tasklens-y6k.3 | Startup and connections workers, launcher, opener (launcher.rs) | 2 | E14-020, E07-A3, E06-A2, E08-A2 |
| E14-A4 | tasklens-y6k.4 | Journal | 2 | E08-A1, E03-A2, E09-A1 |
| E14-001 | tasklens-y6k.5 | systemd worker: connect, list units, unit records | 2 | E08-A1, E08-A2, E03-A2, E05-A1 |
| E14-002 | tasklens-y6k.6 | systemd worker: signals, resync schedule, reconnect | 2 | E14-001 |
| E14-003 | tasklens-y6k.7 | systemd worker: unit cgroup pids and resource stats | 2 | E14-002, E05-A1, E05-A2 |
| E14-004 | tasklens-y6k.8 | systemd worker: UnitDetails for Service Properties | 2 | E14-003 |
| E14-005 | tasklens-y6k.9 | sys_table.rs: Startup Apps projection | 2 | E14-A1 |
| E14-006 | tasklens-y6k.10 | sys_table.rs: Connections projection | 2 | E14-005 |
| E14-007 | tasklens-y6k.11 | Real-system test: tasklens-test.service through the engine | 2 | E14-A1 |
| E14-008 | tasklens-y6k.12 | OpenRC backend for the Services list | 4 | E14-004 |
| E14-009 | tasklens-y6k.13 | Journal worker: follow mode and Load older | 2 | E14-A4 |
| E14-010 | tasklens-y6k.14 | Real-system test: journal follow resume and paging (U-49) | 2 | E14-009 |
| E14-011 | tasklens-y6k.15 | Startup worker: scan on open and RefreshNow | 2 | E06-A2, E08-A1 |
| E14-012 | tasklens-y6k.16 | Startup worker: inotify and UnitFilesChanged rescans | 2 | E14-011, E06-A2, E08-A2 |
| E14-013 | tasklens-y6k.17 | Startup worker: generated units, unit state and startup impact | 2 | E14-012 |
| E14-014 | tasklens-y6k.18 | Connections worker: 2 s dump, socket map, TCP attribution | 2 | E06-A1, E08-A1 |
| E14-015 | tasklens-y6k.19 | Connections worker: reverse DNS for Remote host | 4 | E14-014, E06-A1 |
| E14-016 | tasklens-y6k.20 | KWin bridge script kwin-bridge.js | 2 | none |
| E14-017 | tasklens-y6k.21 | KWin one-shot scripts cmd-template.js and pick.js | 2 | none |
| E14-018 | tasklens-y6k.22 | KWin worker: window commands as one-shot scripts | 2 | E14-A2, E14-017 |
| E14-019 | tasklens-y6k.23 | KWin worker: window picker | 3 | E14-018, E14-017 |
| E14-020 | tasklens-y6k.24 | opener.rs: OpenURI portal, FileManager1, xdg-open fallback | 2 | E07-A3, E08-A2, E03-A2 |
| E14-021 | tasklens-y6k.25 | Real-system tests: launcher scopes and Run now | 2 | E14-A3 |

Anchor titles in `br` keep the leader's wording; the table adds what each anchor's own deliverable is. E14-A3's own work is `launcher.rs`; the Startup and Connections workers are E14-011 to E14-015, and no other epic needs them as a dependency.

### Open items

1. **tl-model and E08 contracts E14 relies on.** E14 beads name these types without their final Rust spelling, because E03 and E08 are not written yet: the services unit record fields (E14-001 list, plus `cgroup_pids`, `has_cgroup_procs`, the four resource fields with `Validity`, `can_reload`), `ManagerState`, `UnitDetails` with `RecoverySettings` and `DepEntry` (E14-004), the startup entry and connection record fields, `WindowsSnapshot` entry fields, every `KWinCommand` variant (activate, close, keep above, set geometry, pick, cancel pick), a `JournalHandle` constructor plus its control-message type (`LoadOlder`, `SetFollow(bool)`), and the `SourceUpdate` variants for services, startup, connections with attribution, and windows. Recommendation: B3 and B1 check their E03 and E08 beads against this list; E14 beads stop and report if a field is missing.
2. **Worker registration and shared handles.** Section 2 lets E14 add only `mod` lines to `workers/mod.rs` and `lib.rs`. Nothing says how a new worker is added to the session's source list (E09 `session.rs`), how `RefreshNow(<SourceId>)` and `KWin(..)` commands reach a worker, how the `SystemdUser`/`SystemdSystem` `ManagerConnSlot` reaches E11's unit actions and the opener and launcher, or how `EngineClient::journal` in E09's `bridge.rs` reaches `workers::journal::open`. Recommendation: E08-A1 gives `WorkerIo` a per-worker command receiver (cloned per generation) and E09-A1 builds the worker list from a `workers::all(mode, &shared) -> Vec<WorkerSpec>` function, with one chain-edit line per worker allowed in that list, plus one allowed line in `bridge.rs` for the journal.
3. **`serde_json` in tl-engine.** The KWin `Update(s json)` payload and `journalctl -o json` both need JSON parsing, but `serde_json` is not in tl-engine's ADR-001 row and only E08 may edit `crates/tl-engine/Cargo.toml`. Recommendation: add `serde_json` to the tl-engine row (it is already a workspace pin) and let E14-A4 add the dependency line as a chain edit. Affects E14-A4, E14-009, E14-A2, E14-018.
4. **Anchors outside E14's row, and who reads unit cgroup stats.** E14-001 and E14-A2 use tl-collect `sanitize` and the uid-to-name lookup (E05-A1); E14-003 uses `proc/pid_stat.rs` (E05-A1) and the cgroup stat readers (E05-A2). Section 4 lists neither for E14. Also, the E14 scope gives the systemd worker "cgroup pids and resource stats", while R-138 item 16 says E05-A2 reads unit `memory.stat` and `cpu.stat` in the process sweep. Recommendation: the systemd worker reads them (it knows each unit's `ControlGroup`) using E05-A2's reader functions; add E05-A1 and E05-A2 to E14's row. The dependency edges are already added.
5. **KWin cannot call the engine by the well-known name.** `tl-app` owns `io.github.oysteinkrog.Tasklens` on its own blocking connection (ADR-010 single instance), so the script's `callDBus` to that name would reach `tl-app`, not the engine's `WindowBridge` object. Recommendation: the engine writes its connection's unique name into the script (`__TL_BUS_NAME__`) and checks that `Update` comes from the owner of `org.kde.KWin` (E14-A2). Alternative: tl-app serves `WindowBridge` and forwards, which puts engine data through tl-app.
6. **KWin signals beyond PROC-052.** PROC-052 lists `windowAdded`, `windowRemoved`, `windowActivated` and `captionChanged`, but the snapshot also carries `minimized`, `maximized`, `keepAbove` and `frameGeometry`, and PERF-083 reads window moves back from it. E14-016 also sends on `minimizedChanged`, the maximize change signal, `keepAboveChanged` and `interactiveMoveResizeFinished` (not per pixel). Recommendation: accept.
7. **Window picker result path.** UI-SHL-014 says the pick reply arrives "as an `ActionEvent` or a `KWinEvent`" and uses `KWinCommand::CancelPick`; neither path nor variant is decided, nor the WindowBridge method `pick.js` calls back with the pid. Recommendation: add `pick: Option<PickResult { seq: u64, outcome: Picked { pid } | NoWindow | Cancelled }>` to `WindowsSnapshot` (E03), add `KWinCommand::CancelPick`, and add method `Picked(s uuid, u pid)` to the `WindowBridge` interface. E14-019 (P3) and `pick.js` wait for this.
8. **Load older command form.** The docs say "200 entries before the oldest loaded one" but give no `journalctl` arguments. Recommendation: `journalctl <unit args> -b -o json --no-pager --reverse --cursor=<oldest> -n 201`, drop the cursor's own entry, reverse; E14-010 verifies it on the owner machine.
9. **Journal details not in the docs.** (a) `Access(OwnOnly)` is sent only for system units, since user-unit entries are the user's own; (b) event channel `async_channel::bounded(64)`; (c) a missing `PRIORITY` field counts as 6 (info). Recommendation: accept all three.
10. **System table filter and sort gaps.** APP-028 says the Services and Startup filters match "all visible columns", but `SysTableQuery` carries no column list. E14-A1 matches all text columns of a unit; Startup has no optional columns, so it is exact there. The Services `status` and `startup_type` sort orders are not decided; E14-A1 uses the order of the SYS-003 mapping tables. Recommendation: accept, or add `visible: ColumnSet` to `SysTableQuery` (E03).
11. **Launcher and opener results.** `ActionError` has `PermissionDenied`, `NoSuchProcess`, `Partial`, `UnitJob`, `Os`, `Internal` only. Not decided: how to report "Authentication cancelled" (pkexec 126) and "not authorized" (127, R-117), "Already running" (SYS-023), "File properties unsupported" when no `FileManager1` owner, and invalid command text. Also when a `pkexec` launch counts as done: `pkexec` only returns when the root program exits. Recommendation: add `ActionError::AuthCancelled`, `ActionError::NotAuthorized` and `ActionError::Unsupported`; report "Already running" as `Succeeded` with a toast chosen by the UI from `LaunchOutcome` (needs an event field, or use a distinct `ActionState::Succeeded` detail); count a `pkexec` launch as `Succeeded` when it is still running 2 s after the prompt closes. Plasma scripts in Run now go as `LaunchWhat::Command` with the shell-quoted path. E11 owns the mapping into events.
12. **`<rand>` in scope names.** Canonical table (c) writes `<rand>` and SYS-023 `<random hex>` without a length. Recommendation: 8 lowercase hex digits from 4 bytes of `/dev/urandom`.
13. **Reverse DNS function.** E14-015 needs E06's `net/rdns.rs`, which is not an anchor, and `getnameinfo` needs libc, which is in neither tl-collect's nor tl-engine's row. Recommendation: the leader adds the edge from E14-015 to that E06 bead and decides where the libc call lives (tl-ops `sys.rs` is the only allowed `unsafe` file besides two others).
14. **Retry when a systemd manager is unavailable.** Not decided how often the worker retries a missing bus. E14-001 uses: on `RefreshNow` or every 60 s. Recommendation: accept.
15. **KWin hardening values.** E14-A2 rejects `Update` payloads over 1 MiB and cuts caption, class and desktop file name at 4 KiB; the docs give no limits. Recommendation: accept.
16. **OpenRC (P4) details.** SYS-018 says "Status from `rc-status`" without a word mapping or refresh rule. Recommendation: `started` Running, `stopped` Stopped, `crashed` Failed, `starting` Starting, `stopping` Stopping, `inactive` and `hotplugged` Stopped, any other word Active; refresh every 5 s while `Demand.services` is true. E14-008 waits for this.

## E29 helper-privilege-setup

34 beads: 3 anchors filled in place, 31 new beads under epic `tasklens-qfx`. P3 unless marked P4.

- **tl-helper crate** (root binary): crate skeleton (001), process operations (002), cgroup operations for any uid (003), `ReadProcessFiles` (004), `ScanStrings` (005), `Dump` with a passed fd (006), `ResolveSocketInodes` (007), `SockDestroy` (008), `WriteServiceDropIn` (009), `NetnsSockDiagDump` (010, P4), session loop and dispatch (A1), root real-system tests (012).
- **Engine**: helper link with pkexec, handshake and calls (013), helper session task, `HelperHandle` and shield state (A2), privileged action routing (014), helper call sites in `process_detail.rs` (015), `handle_search.rs` (016), `processes.rs` (017) and `connections.rs` (018), setup probe and `tasklens-setup` runner (019), command and health routing (020), real pkexec tests (011).
- **Packaging**: polkit policy (021), `tasklens-setup` with RAPL (022), `sensors-detect` (023, P4), `polkit-rule` (024, P4).
- **UI**: Additional Setup window (A3), Enable and Revert flows (025), View script and Copy command (026), Privileges page handlers (027), Skip password prompts (028, P4), shield flow tests (029), Shift at launch and U-43 (030), Fan sensors row (031, P4).

Beads labelled `blocked-open-item` name the open item they wait for in their Notes.

| Plan key | Bead ID | Title | Priority | Depends on |
|---|---|---|---|---|
| E29-A1 | tasklens-qfx.1 | Helper binary | 3 | E29-002, 003, 004, 005, 006, 007, 008, 009 |
| E29-A2 | tasklens-qfx.2 | Helper session in the engine | 3 | E29-013, E09-A1, E08-A1 |
| E29-A3 | tasklens-qfx.3 | Additional Setup window | 3 | E29-021, 022, E16-A1, E16-A2, E15-A2, E03-A2 |
| E29-001 | tasklens-qfx.4 | tl-helper crate skeleton and workspace member | 3 | E02-A1, E03-A2 |
| E29-002 | tasklens-qfx.5 | Helper process operations with own identity check | 3 | E29-001, E07-A1 |
| E29-003 | tasklens-qfx.6 | Helper cgroup freeze, thaw and kill for any user | 3 | E29-002, E07-A2 |
| E29-004 | tasklens-qfx.7 | Helper ReadProcessFiles | 3 | E29-001, E07-A1 |
| E29-005 | tasklens-qfx.8 | Helper ScanStrings streaming | 3 | E29-004, E05-A3 |
| E29-006 | tasklens-qfx.9 | Helper Dump into a passed fd | 3 | E29-002, E07-A3 |
| E29-007 | tasklens-qfx.10 | Helper ResolveSocketInodes | 3 | E29-001 |
| E29-008 | tasklens-qfx.11 | Helper SockDestroy over raw netlink | 3 | E29-007 |
| E29-009 | tasklens-qfx.12 | Helper WriteServiceDropIn | 3 | E29-001, E07-A3 |
| E29-010 | tasklens-qfx.13 | Helper NetnsSockDiagDump | 4 | E29-008, E29-A1 |
| E29-011 | tasklens-qfx.14 | Engine real-system helper tests through pkexec | 3 | E29-020, 014, A1 |
| E29-012 | tasklens-qfx.15 | Helper real-system tests as root | 3 | E29-A1 |
| E29-013 | tasklens-qfx.16 | Engine helper link: pkexec spawn, handshake, calls | 3 | E08-A1, E03-A2 |
| E29-014 | tasklens-qfx.17 | E29: add privileged routing arm and actions/helper.rs to actions/mod.rs | 3 | E29-020, E11-A1 |
| E29-015 | tasklens-qfx.18 | E29: add helper reads to workers/process_detail.rs | 3 | E29-020, E10-A2 |
| E29-016 | tasklens-qfx.19 | E29: add helper path to workers/handle_search.rs | 3 | E29-020, E10-A3 |
| E29-017 | tasklens-qfx.20 | E29: add restricted-cell helper reads to workers/processes.rs | 3 | E29-020, E10-A1 |
| E29-018 | tasklens-qfx.21 | E29: add ResolveSocketInodes to workers/connections.rs | 3 | E29-020, E14-A3 |
| E29-019 | tasklens-qfx.22 | Setup status probe and tasklens-setup runner in the engine | 3 | E29-A2, E04-A1 |
| E29-020 | tasklens-qfx.23 | E29: route helper and setup commands and health in session.rs and the reducer | 3 | E29-A2, 019, E09-A1 |
| E29-021 | tasklens-qfx.24 | Polkit policy file with three actions | 3 | E29-A1 |
| E29-022 | tasklens-qfx.34 | tasklens-setup script with rapl-enable and rapl-revert | 3 | E29-019 |
| E29-023 | tasklens-qfx.25 | tasklens-setup sensors-detect and sensors-revert | 4 | E29-022 |
| E29-024 | tasklens-qfx.26 | tasklens-setup polkit-rule on and off | 4 | E29-022 |
| E29-025 | tasklens-qfx.27 | Additional Setup Enable and Revert flows | 3 | E29-A3, 019 |
| E29-026 | tasklens-qfx.28 | Additional Setup View script and Copy command | 3 | E29-A3, 022 |
| E29-027 | tasklens-qfx.29 | E29: add privilege handlers to window/settings/privileges.rs | 3 | E21-A2, E20-A2, E29-A3, 029 |
| E29-028 | tasklens-qfx.30 | E29: add Skip password prompts to window/settings/privileges.rs | 4 | E29-027, 024, 025 |
| E29-029 | tasklens-qfx.31 | Shield flow UI tests | 3 | E16-A3, E20-A1, E22-A1, E23-A1, E29-A3 |
| E29-030 | tasklens-qfx.32 | E29: add Shift-at-launch detection to window/main.rs (U-43) | 3 | E29-029, E20-A1 |
| E29-031 | tasklens-qfx.33 | Additional Setup Fan sensors row | 4 | E29-025, 026, 023 |

Cross-batch anchors used beyond E29's row in 00-epics.md section 4: E04-A1 (`SysRoot` for the setup probe), E05-A3 (strings scanner), E07-A3 (`write_atomic`, `dump.rs`), E10-A1 and E14-A3 (the two flagged chain edits), E15-A2, E22-A1.

### Open items

1. **E29-OI-1. `helper_proto.rs` shape (E03).** The docs list the request set but not the frame wrapper, the handshake or the replies. Recommendation: `HelperFrame { req: u32, body }` with the reply echoing `req`; `Hello { proto }` / `Hello { proto, pid }` as the mandatory first exchange (uses `HELPER_PROTO_VERSION`); `CancelScan`; `HelperTarget::{Process, Thread}`; replies `Done`, `Error(HelperError)`, `Files`, `Strings { .., last }`, `Inodes(Vec<(u64, ProcessKey)>)`, `SockDiag { bytes, last }` (raw netlink bytes for `NetnsSockDiagDump`, parsed by the engine with the E06 parser); `HelperError::{IdentityChanged, NoSuchProcess, PermissionDenied, Partial, Os(i32), NotAllowed(Arc<str>), TooLarge}`. The full list is in every helper bead's Notes. E03-A2 should adopt it.
2. **E29-OI-2. fd passing for `Dump { out_fd }`.** An fd cannot cross a pipe. Recommendation: the engine passes one end of a Unix `socketpair` as both stdin and stdout of `pkexec`; the fd travels as `SCM_RIGHTS` on the `Dump` frame. Still "stdin and stdout" in R-003's sense.
3. **E29-OI-3. Shield commands and results.** No `Command` variant exists for `StartHelperSession` / `EndHelperSession`, and nothing carries a failed start to the UI for the toasts "Authentication cancelled" and "Could not start the privileged helper". `HelperState` (UI-FND-028) is not in the `health.rs` list of 00-epics.md. Also `ActionError` has no values for a dismissed or denied prompt (R-117). Recommendation: `Command::StartHelperSession`, `Command::EndHelperSession`; `HealthSnapshot.helper: HelperState { Missing, Locked, Starting, Unlocked }` and `HealthSnapshot.helper_start: Option<(u32, HelperStartOutcome::{Cancelled, NotAuthorized, Failed(Arc<str>)})>`; `ActionError::AuthCancelled` and `ActionError::NotAuthorized`. All E03 changes.
4. **E29-OI-4. Setup answers to the UI.** `Command::SetupStatus` and `RunSetup` exist, but `SetupStatus`, `SetupEvent` and the path back are undefined. Recommendation: `HealthSnapshot.setup: Arc<SetupState { status: Option<SetupStatus>, run: Option<SetupRun { seq, step, phase: Running | Succeeded | Cancelled | Failed(Arc<str>), output: Arc<str> }> }>`, with `SetupStatus { rapl: RaplStatus::{NoZone, Readable { tasklens_rule: bool }, NotReadable}, journal_group, helper_installed, lm_sensors, hwmon_fan, sensors_conf }` and `SetupStep::{RaplEnable, RaplRevert, SensorsDetect, SensorsRevert, PolkitRuleOn, PolkitRuleOff}`. No new channel.
5. **E29-OI-5. Engine wiring outside E29's files.** Spawning the helper task, routing the four commands, and storing helper and setup state in `HealthSnapshot` need edits in `session.rs`, `reducer/apply.rs` and `reducer/health.rs` (E09, owner only) and a `SourceUpdate::Helper` / `SourceUpdate::Setup` variant in `update.rs` (E08, up front). Recommendation: E08 and E09 write the variants and arms up front against the OI-3 and OI-4 types; E29-020 is then reduced to filling the session spawn. If not, approve E29-020 as a chain edit.
6. **E29-OI-6. The E11 helper hook.** E11 owns the "privileged routing hook that E29 fills" but its shape is not written. Recommendation: E11-A1 defines `trait PrivilegedOps { async fn call(&self, cx, req: HelperRequest, fd: Option<OwnedFd>, deadline) -> Result<Vec<HelperReply>, HelperCallError> }` and holds `Option<Arc<dyn PrivilegedOps>>` in its action context; `socket.rs`, `recovery.rs` and `dump.rs` call it. Batches B3 and B4 must agree.
7. **E29-OI-7. Strings scan in the helper.** ADR-001 does not let `tl-helper` use `tl-collect`, where the scanner lives, and has no `libc` for `process_vm_readv`. PROC-084's 256 MiB limit also exceeds the 4 MiB per-request cap. Recommendation: add `tl-collect` to tl-helper's ADR-001 row, and state that for streamed requests (`ScanStrings`, `NetnsSockDiagDump`) the 4 MiB cap applies per reply message.
8. **E29-OI-8. cgroup validation for other uids.** `tl_ops::cgroup::open_validated` accepts only the user's own `app.slice`; the helper allows "any uid's cgroup". Recommendation (written into E29-003): relative path, no `.`/`..`, first component `user.slice` or `system.slice`, leaf `.scope` or `.service`, per-component `openat` with `O_NOFOLLOW`, inode check, refuse `init.scope` and any cgroup holding tasklens or the helper; E07 exposes write helpers that take a validated dirfd.
9. **E29-OI-9. `ReadProcessDetail` and `root`.** `ReadProcessDetail { key, what }` has no defined `what`, and it overlaps `ReadProcessFiles`. `root` (`/proc/<pid>/root`) is read by PROC-078 and PROC-096 but is not in the R-004 file set. Recommendation: drop `ReadProcessDetail` (A1 answers "unsupported" until decided) and add `root` to the `ReadProcessFiles` set.
10. **E29-OI-10. One drop-in renderer.** The user-unit recovery drop-in (E11, engine) and the system one (E29-009, helper) should produce the same text. Recommendation: move `render(&RecoveryEdit) -> String` into `tl-ops` (both crates may use it); until then it lives in `tl-helper/src/dropin.rs`.
11. **E29-OI-11. Retry and Grant access buttons.** "Retry as administrator" lives in E16's `action_tracker.rs` and "Grant access" in E23's `props/mod.rs`, both owner only, while 00-epics.md puts these flows in E29's scope. Recommendation: E16 and E23 build the P3 buttons from their own specs (they dispatch `StartHelperSession` or resend with `privileged = true`, disabled with "Needs administrator helper (not installed)" when `Missing`); E29-029 tests them end to end.
12. **E29-OI-12. Restricted table cells and socket owners.** PROC-076 says Handles, I/O, Disk and Image path cells fill in through the helper, and SYS-030 says socket owners do. Those reads belong in `workers/processes.rs` (E10) and `workers/connections.rs` (E14), where section 2 gives E29 no chain edit. Recommendation: allow the two chain edits E29-017 and E29-018.
13. **E29-OI-13. New file and test mode.** `tl-engine/src/helper/setup.rs` is not in section 2; recommend it as an E29 file. `tasklens-setup` gets a test mode (`TASKLENS_SETUP_ROOT`, honoured only when not root) so it can be tested without root; recommend accepting it.
14. **E29-OI-14. Fan sensors Revert.** PERF-108 has a "Revert" that removes `/etc/modules-load.d/tasklens-sensors.conf`, but table (c) lists no revert subcommand. Recommendation: add `sensors-revert` to table (c) and `SetupStep::SensorsRevert` to E03 (P4).
15. **E29-OI-15. "View script" source.** The UI must not read system files. Recommendation: embed `packaging/tasklens-setup` with `include_str!` and show the whole script (E29-026).
16. **E29-OI-16. Polkit prompt texts.** The docs give no `<description>` or `<message>` for the three actions. E29-021 proposes: "Run tasklens administrator actions" / "Authentication is required to let tasklens act on other users' processes, system services and network connections."; "Change tasklens system setup" / "Authentication is required to change system settings for tasklens."; "Run the tasklens network monitor" / "Authentication is required to run the tasklens network monitor." Needs approval.
17. **E29-OI-17. PKGBUILD lines.** Installing `tl-helper`, `tasklens-setup` (0755) and the policy is E33's. Bead `tasklens-cr0.18` (E33-015) already depends on E29-A1 for this; confirm it also depends on E29-021 and E29-022, which are not anchors.
18. **E29-OI-18. Card styling conflict.** UI-SHL-094 gives row cards background `header`, padding 12, plain status text and the busy text "Waiting for authentication…"; UI-PRF-052 gives `list_bg`, a 1 px divider, padding `sp5`, `TlBadge` statuses and the busy text `Running…` with a spinner. UI-SHL-094 says the row content is owned by the PERF entries. Recommendation: UI-PRF-052 for the cards, UI-SHL-094 for the frame (title, 640 × 520, minimum 560 × 400). Beads are written that way.
19. **E29-OI-19. Shift at launch.** The Wayland detection belongs in E20's `window/main.rs`, and UI-SHL-110's X11 path (`XkbGetState`) needs an X11 library outside tl-app's ADR-001 row. Recommendation: allow the one E29 chain edit in `window/main.rs` (E29-030) and use the same first-focus modifier rule on X11.

Note for the leader: `~/.claude/CLAUDE.md` changed during this round and now says build commands in agent prompts should say "use `heavy-build`" instead of the long `flock … systemd-run …` line. The E29 beads still carry the line from `brief-beads.md`; a batch-wide replace is the leader's call.

## E30 history

31 beads: 28 new plus the three anchors filled in. All are phase P3, priority 3. Labels: `phase-p3`, `area-history`, and `crate-tl-history`, `crate-tl-engine` or `crate-tl-ui` (`ui` on every tl-ui bead). E30-003 and E30-007 also carry `security`, because they parse files from disk and need a regression test for corrupt input. E30-028 carries `perf-gate`.

- **tl-history crate (E30-001 to E30-011, anchor E30-A1).** Skeleton and members line, format types and header codec, block codec, `BlockBuilder`, the read side (state at a second, range stats, decimation, top-K), segments with crash recovery, the store, retention, `RollingWindow` and `Recorder`. E30-A1 is the 6-hour acceptance test from the ADR-012 verification.
- **Engine (E30-012 to E30-015, anchor E30-A2).** The `HistorySink` forwarder, the `HistoryRecorder` worker (rolling-window mode and store mode) and `history_query.rs`. E30-A2 wires the sink, the commands and `SourceUpdate::History` into E09's session and reducer. It is blocked on E30-OI-4.
- **UI (E30-016 to E30-028, anchor E30-A3).** Page layout and header text, the query controller, actions and keys, the table, the table menu, the scrubber (paint and interaction), the top-3 card, the minimap, the legend, the Live button with the recording-off banner and toasts, the Settings rows (chain edit) and the `history-scrub` bench scene (chain edit). E30-A3 mounts History mode in the Processes page (chain edit) and holds the end-to-end tests.

Chain edits, each a single named part in another batch's file: root `Cargo.toml` members line (E30-001), tl-engine `Cargo.toml` dependency line and `workers/mod.rs` mod line (E30-012), tl-engine `lib.rs` mod line (E30-015), `session.rs` and `reducer/apply.rs` (E30-A2, needs E30-OI-4), `pages/processes/mod.rs` mod line (E30-016) and mount (E30-A3), `app.rs` `bind_keys_history` call (E30-018), `window/settings/processes.rs` history rows (E30-027), `bench/mod.rs` mod line and dispatch arm (E30-028), and `assets/locales/en-US.ftl` keys in each UI bead that adds strings.

Cross-batch anchors used: E01-A1, E02-A1, E03-A1, E03-A2, E04-A3, E08-A1, E09-A1, E10-A1, E15-A1, E16-A1, E16-A2, E16-A3, E17-A1, E18-A1, E18-A2, E19-A1, E20-A1, E21-A2, E22-A1, E22-A3. Two of these are outside E30's row in 00-epics.md section 4. E04-A3 supplies the meminfo parser for `mem_total_kib`. E22-A3 supplies `select_by_key` for "Show in live table". Neither adds a cycle.

| Plan key | Bead ID | Title | Priority | Depends on |
|---|---|---|---|---|
| E30-001 | tasklens-8fx.4 | tl-history crate skeleton, HistoryError and workspace members line | 3 | E02-A1, E03-A1 |
| E30-002 | tasklens-8fx.5 | History format types and segment header codec | 3 | E30-001, E03-A1 |
| E30-003 | tasklens-8fx.6 | Block frame codec with CRC and zstd | 3 | E30-002 |
| E30-004 | tasklens-8fx.7 | BlockBuilder: keyframes, deltas, 1 Hz system series | 3 | E30-003 |
| E30-005 | tasklens-8fx.8 | History read side: state at a second, range averages | 3 | E30-004 |
| E30-006 | tasklens-8fx.9 | History read side: decimation, top-K, system totals, deleted keys | 3 | E30-005 |
| E30-007 | tasklens-8fx.10 | Segment writer and reader with footer and crash recovery | 3 | E30-003 |
| E30-008 | tasklens-8fx.11 | History store: boot dirs, rotation, 2 s load, deleted keys | 3 | E30-006, E30-007 |
| E30-009 | tasklens-8fx.12 | History retention, disk cap and persist-off purge | 3 | E30-008 |
| E30-010 | tasklens-8fx.13 | RollingWindow: 10-minute in-RAM history | 3 | E30-004, E30-006, E03-A2 |
| E30-011 | tasklens-8fx.14 | Recorder RecordSink for the store | 3 | E30-004, E03-A2 |
| E30-A1 | tasklens-8fx.1 | History crate | 3 | E30-008, E30-009, E30-010, E30-011 |
| E30-012 | tasklens-8fx.15 | HistorySink and history worker messages in tl-engine | 3 | E30-A1, E08-A1, E03-A2 |
| E30-013 | tasklens-8fx.16 | HistoryRecorder worker loop and rolling-window mode | 3 | E30-012, E04-A3, E08-A1 |
| E30-014 | tasklens-8fx.17 | HistoryRecorder store mode: load, write, retention, shutdown | 3 | E30-013 |
| E30-015 | tasklens-8fx.18 | HistoryQuery and HistoryDelete answers in history_query.rs | 3 | E30-013 |
| E30-A2 | tasklens-8fx.2 | Recorder and query | 3 | E30-014, E30-015, E09-A1, E10-A1 |
| E30-016 | tasklens-8fx.19 | History mode page: layout, header text, state | 3 | E16-A1, E22-A1, E15-A1, E17-A1, E03-A2 |
| E30-017 | tasklens-8fx.20 | History query controller with one query in flight | 3 | E30-016 |
| E30-018 | tasklens-8fx.21 | History actions and History key context bindings | 3 | E30-016, E20-A1 |
| E30-019 | tasklens-8fx.22 | History table delegate | 3 | E30-016, E18-A1, E18-A2 |
| E30-020 | tasklens-8fx.23 | History table menu, jump and Delete from history | 3 | E30-019, E30-018, E16-A2, E16-A3, E22-A3 |
| E30-021 | tasklens-8fx.24 | Timeline scrubber element: paint and geometry cache | 3 | E30-016, E19-A1 |
| E30-022 | tasklens-8fx.25 | Timeline scrubber interaction and focus | 3 | E30-021, E30-018, E30-017 |
| E30-023 | tasklens-8fx.26 | History top-3 hover card | 3 | E30-021, E30-017, E16-A2 |
| E30-024 | tasklens-8fx.27 | History minimap | 3 | E30-022 |
| E30-025 | tasklens-8fx.28 | History legend chips and process series | 3 | E30-018, E30-021 |
| E30-026 | tasklens-8fx.29 | Live button, recording-off banner and toast, load-problem toasts | 3 | E30-017, E30-018, E16-A2 |
| E30-027 | tasklens-8fx.30 | E30: add history row handlers to window/settings/processes.rs | 3 | E21-A2, E16-A1 |
| E30-028 | tasklens-8fx.31 | E30: add history-scrub bench scene to tl-ui/src/bench | 3 | E01-A1, E30-A3 |
| E30-A3 | tasklens-8fx.3 | History mode UI | 3 | E30-017, E30-019, E30-020, E30-022, E30-023, E30-024, E30-025, E30-026, E20-A1, E22-A1 |

### Open items

Several beads carry the proposals below as their working text and say so in their Notes. Each item needs a leader decision before the beads named in it start.

1. **E30-OI-1. The tl-model history types have no fields.** ADR-012 and R-136 name `HistoryRange`, `HistoryWhat` and `HistoryView` but define nothing inside them. UI-PRC-030 to UI-PRC-036 name `HistoryView.rows`, `.series`, `.span`, `.overview`, `.source` and `TopAt { t, metric, n }`. E03 owns these files, so this is E03's to write. Recommendation: E03-A2 writes the definitions in the "tl-model history types" block that every engine and UI bead of E30 carries. Those are `HistoryMetric`, `HistoryRange::{Live, At, Span}` and `HistoryWhat::{Frame, TopAt}`, plus `HistoryView` with `query_id`, `source`, `store_state`, `span`, `gaps`, `frame_t`, `range`, `process_count`, `rows`, `totals`, `series`, `stack`, `overview`, `top` and `realtime_offset_ns`. The block also has `HistoryProcessRow`, `HistoryTop`, `HistoryLoadProblem` and the others. Also check that `ColumnId` has an `image_path` id for the history table's Image path column. Blocks: E30-012 onward.
2. **E30-OI-2. The ADR-012 `Block` lacks fields the UI needs.** History table columns User and Image path need the user name and the full exe path, but `HistMeta` holds only comm, display name, exe basename and app id. Hide system processes in History mode (PROC-046) needs a system marker. `SeriesId` uses `DeviceIdx`, which ADR-003 makes per engine session, so system totals need the device catalog. The top-3 text "Memory: 1.22 / 62.70 GiB" needs MemTotal. Gaps need a marker. Recommendation: add `HistMeta { exe_path, user, uid, system }`, `Block { gap_before, mem_total_kib, catalog: Option<Vec<HistDevice>> }` and `HistDevice { idx, kind, parent, name }`, and fix the ADR-012 text in the same step. No file exists yet, so the format can still change. Blocks: E30-002 and later.
3. **E30-OI-3. On-disk details that ADR-012 leaves open.** Recommendation, as written in the beads:
   - The 64-byte header is little endian, fields packed in order at offsets 0, 4, 6, 14 and 22, zero bytes from 30 to 64.
   - The boot directory name is lowercase hex.
   - The footer is postcard `Vec<FooterEntry { start_t_ns, offset }>`, then `u32` length, `u32` crc32 and magic `TLHF`.
   - `FieldPatch` is a struct of `Option` fields. `ppid` and `gpu_pct` are stored on any change.
   - The index space for `changed` and `exited` is the keyframe followed by the started entries in order.
   - `MetaId` scope is one segment, or the rolling window's lifetime.
   - A NaN point marks a system-series gap.
   - Non-`Valid` fields are stored as 0.
   - The deleted-keys file is `deleted-keys.bin`, postcard `Vec<ProcessKey>`, mode 0600.
4. **E30-OI-4. Engine wiring needs edits that section 2 does not allow.** Section 2 has no chain edit for any of these:
   - installing the `RecordSink` in E09's session;
   - routing `Command::HistoryQuery` and `HistoryDelete` from `reducer/apply.rs` to the history worker;
   - a `SourceUpdate::History(Arc<HistoryView>)` variant in E08's `update.rs`;
   - an `EngineConfig.history_dir: Option<PathBuf>` field (E08), set by tl-app (E33) to `$XDG_STATE_HOME/tasklens/history`.

   Recommendation: E08 and E09 add these hook points up front, as `SourceUpdate` "carries every variant up front", and E30-A2 makes the named chain edits in `session.rs` and `reducer/apply.rs`. E30-A2 lists the exact edits. Also decide the small proposals in E30-013 to E30-015:
   - the health text "History dropped N events";
   - a store write error shows as `Problem(OpenFailed)`, with a retry after 60 s;
   - a read error shows as `Problem(EmptyOrTruncated)`.
5. **E30-OI-5. Two data shapes are open.** PROC-089 and UI-PRC-032 do not define the stacked "top-3 plus other" area. Recommendation: per bucket, the three largest per-process values plus the sum of the rest, drawn in the metric's colour. Header totals for a selected range are also open. Recommendation: averages over the range. Blocks: E30-006, E30-015, E30-021.
6. **E30-OI-6. Two UI details are open.** (a) Query loss recovery. Recommendation: resend after 1,000 ms without an answer (E30-017). (b) Copy details and Copy all metrics on history rows. PROC-070's block needs fields that history does not store: Private bytes, Working set, Threads, Handles, Priority and Command line. Recommendation: Copy details prints Name, PID, Parent PID, CPU, Memory, Disk, Network and GPU from the history row. Copy all metrics prints every visible history column. Neither prints a command line (E30-020).
7. **E30-OI-7. Who declares `ToggleHistoryMode`.** UI-SHL-023 has the tab icon dispatch it, and E20 owns `actions.rs`. Recommendation: E20 declares the Action and selects the Processes tab, and E30-A3 handles the mode flip in `pages/processes/mod.rs`. Also make sure E16's demand aggregator sets `Demand.recording` from `history.enabled`, so sources run at the ADR-008 "recording on" rates while hidden. That is E16's bead, not E30's.
8. **E30-OI-8. Minor proposals in the UI beads.**
   - Axis labels under an hour use "-M:SS", for example "-0:45" (E30-021).
   - The disk-limit input snaps to the nearest multiple of 64 (E30-027).
   - The `history-scrub` scene answers queries from synthetic views generated in tl-ui, not from a real store. PROC-092 says "synthetic data from `tl-ui/src/bench/`", and tl-ui may not depend on tl-history.
9. **Epic lint.** `br lint tasklens-8fx` warns that the epic bead has no "## Success Criteria" section. The leader created the epic, so the fix is the leader's.
