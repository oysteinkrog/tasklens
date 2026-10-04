# B2 collect-ops beads: plan keys and bead IDs

Batch B2 covers epics E04, E05, E06 and E07 (`docs/plan/00-epics.md` section 5): every
synchronous system reader in `tl-collect` and every verified operation in `tl-ops`. The leader
created the epic and anchor beads (`docs/plan/anchors.md`). B2 replaced each anchor's placeholder
with the full template and created every other bead under its epic.

Totals: E04 32 beads, E05 36, E06 26, E07 17 (111 task beads, anchors included). Bead titles start
with the plan key. Every bead carries the build rule in its Notes.

Conventions used by all four epics:

- **Anchors close their sets.** Each anchor in section 3 of the epic map lists more files than one
  bead may touch. The small beads build the files, and the anchor is the last bead of its set: it
  depends on them, adds the `mod.rs` re-exports and the set's integration tests, and freezes the
  public API. Other epics depend on the anchor only.
- **Chain edits come first.** The first bead of each epic (E04-001, E05-001, E06-001, E07-001)
  adds every `mod` line and `CollectError` or `OpsError` variant the epic needs, so later beads
  edit only their own module file and parallel beads never touch `lib.rs` or `error.rs`.
- **`tl-collect` returns its own reading types.** E03's `tl-model` totals and payload fields are
  not fixed by ADR-003, so E04, E05 and E06 return `tl-collect` structs and the engine epics (E10,
  E12, E14) map them into `tl-model` types. See E04-OI-6 and E05 open item 1.
- **Dependency rule.** Edges inside B2 point at any bead. Edges to other batches point at anchors
  (E02-A1, E03-A1, E03-A2, E04-A1, E05-A1). The "Depends on" column lists plan keys; anchor IDs
  are in `docs/plan/anchors.md`.

## E04 collect-system (tasklens-2dl)

E04 has 32 beads: the 3 anchors plus E04-001 to E04-029. Each anchor in section 3 of the epic map lists more than three files, so each anchor is kept as the bead that closes its set. It depends on the small beads that build the parts, adds one test file of its own, and checks the whole set. E04-001 writes every E04 `mod` line up front with one-line stub files. Later beads then edit only their own module file, and parallel beads never touch `lib.rs`, `parse/mod.rs` or `sample/mod.rs`. Parser property tests live in each parser's own `#[cfg(test)]` module. `tests/prop_system.rs` is written once by E04-A1 and extended once by E04-A3, so no two beads edit it at the same time. Samplers are synchronous structs with `sample(&mut self, .., out: &mut Reading)` and `sample_at(clock, ..)` for tests. They return `tl-collect` reading types (`CpuReading`, `MemoryReading`, `DiskReading`, `NetReading`, `SensorsReading`); E12 maps them to `tl-model` series and totals (see E04-OI-6).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E04-A1 | tasklens-2dl.1 | SysRoot, counters, fixture infrastructure | P0 | E04-001 to E04-007 |
| E04-A2 | tasklens-2dl.2 | CPU sampler | P0 | E04-A1, E04-008, E04-009, E04-010, E04-011 |
| E04-A3 | tasklens-2dl.3 | Memory, disk, net, sensors samplers | P1 | E04-A1, E04-A2, E04-012, E04-016, E04-017, E04-018, E04-020, E04-021, E04-022, E04-025, E04-026, E04-027, E04-029 |
| E04-001 | tasklens-2dl.4 | tl-collect crate skeleton, module tree and CollectError | P0 | E02-A1, E03-A1 |
| E04-002 | tasklens-2dl.5 | SysRoot, KeptFile and bounded read helpers | P0 | E04-001 |
| E04-003 | tasklens-2dl.6 | Counter rules: rates, resets, suspend gap, stale threshold | P0 | E04-001 |
| E04-004 | tasklens-2dl.7 | smaps_rollup fd cache (128 entries) | P0 | E04-001 |
| E04-005 | tasklens-2dl.8 | cargo xtask capture-fixture with scrubbing | P0 | E02-A1 |
| E04-006 | tasklens-2dl.9 | Add capture-fixture arm to xtask/src/main.rs (chain edit, E02 file) | P0 | E04-005, E02-A1 |
| E04-007 | tasklens-2dl.10 | Capture owner-desktop fixture and wire insta | P0 | E04-006, E04-001 |
| E04-008 | tasklens-2dl.11 | Parser for /proc/stat | P0 | E04-A1 |
| E04-009 | tasklens-2dl.12 | CPU topology, caches and core kinds | P0 | E04-A1 |
| E04-010 | tasklens-2dl.13 | cpufreq clocks and base speed | P0 | E04-A1, E04-009 |
| E04-011 | tasklens-2dl.14 | hwmon enumeration and value parsers | P0 | E04-A1 |
| E04-012 | tasklens-2dl.15 | CPU live stats: load average, open files, process count, uptime | P1 | E04-A2 |
| E04-013 | tasklens-2dl.16 | cpufreq static info and preferred-core ranking | P2 | E04-010 |
| E04-014 | tasklens-2dl.17 | RAPL power parser and readability probe | P2 | E04-A1 |
| E04-015 | tasklens-2dl.18 | RAPL power in the CPU sampler | P2 | E04-012, E04-014 |
| E04-016 | tasklens-2dl.19 | Parser for /proc/meminfo | P1 | E04-A1 |
| E04-017 | tasklens-2dl.20 | zram enumeration and parsers | P1 | E04-A1 |
| E04-018 | tasklens-2dl.21 | Memory sampler with composition and zram rates | P1 | E04-016, E04-017 |
| E04-019 | tasklens-2dl.22 | zswap parameters, swap devices and memory details | P2 | E04-018 |
| E04-020 | tasklens-2dl.23 | Parser for /proc/diskstats | P1 | E04-A1 |
| E04-021 | tasklens-2dl.24 | Block device enumeration, kinds and children | P1 | E04-A1 |
| E04-022 | tasklens-2dl.25 | Disk sampler: active time, rates, latency, totals | P1 | E04-020, E04-021 |
| E04-023 | tasklens-2dl.26 | Disk temperature from the drive hwmon | P2 | E04-022, E04-011 |
| E04-024 | tasklens-2dl.27 | Disk usage: mountinfo, statvfs, system disk, swap | P2 | E04-023, E04-019 |
| E04-025 | tasklens-2dl.28 | Parser for /proc/net/dev | P1 | E04-A1 |
| E04-026 | tasklens-2dl.29 | Network interface enumeration, kinds and state | P1 | E04-A1 |
| E04-027 | tasklens-2dl.30 | Network sampler: rates, totals, state | P1 | E04-025, E04-026 |
| E04-028 | tasklens-2dl.31 | Network interface details: speed, MAC, driver | P2 | E04-026 |
| E04-029 | tasklens-2dl.32 | Sensors sampler: temperatures and fans | P1 | E04-011 |
| E04-030 | tasklens-2dl.33 | udev database reader (`read_udev_props`) (R-140) | P1 | E04-A1 |
| E04-031 | tasklens-2dl.34 | Partitions table rows (PERF-050, R-140) | P2 | E04-021, E04-024, E04-030, E03-A2 |
| E04-032 | tasklens-2dl.36 | `minimal-vm` fixture capture (R-140) | P1 | E04-A1 |
| E04-033 | tasklens-2dl.37 | capture-fixture: substring user scrubbing and scrubbed udev data (R-260, R-261) | P0 | E04-005; E04-A1 depends on it |
| E04-A4 | tasklens-2dl.35 | Anchor: P2 system details (R-140) | P2 | E04-013, E04-014, E04-015, E04-019, E04-023, E04-024, E04-028, E04-031 |

Cross-epic edges: E04-001 depends on E02-A1 (tasklens-v9m.1) and E03-A1 (tasklens-as2.1). E04-005 and E04-006 depend on E02-A1. No other cross-epic edge.

### Writer choices (E04, the leader should confirm)

These are small internal choices the docs leave open. The beads state them as fixed values.

- New parser files beside the R-135 list, under R-135's "one file per system file" rule: `parse/loadavg.rs`, `parse/filenr.rs` (PERF-033), `parse/swaps.rs` (PERF-045, PERF-049), `parse/zswap.rs` (PERF-044), `parse/mountinfo.rs` (PERF-049). Recommend adding them to R-135 and the section 2 table.
- `FdCache` does not evict. When it holds 128 entries, `insert` hands the fd back and the caller reads once and closes it.
- `Counter`: an elapsed time of zero or less gives `WarmingUp`, not a division.
- Disk latency with no I/O in the interval is `Valid(0.0)`.
- `tl-collect` `Cargo.toml` lists every ADR-001 row crate that has a workspace pin up front (the netlink crates included). E05 and E06 then add only `mod` lines.
- `cargo xtask capture-fixture` takes the 50 lowest readable pids. It writes a `CAPTURE.txt` manifest with no date, host or user. It also replaces the hostname only when the name is at least 3 characters long.

### Open items (E04)

- **E04-OI-1 `minimal-vm` fixture.** ADR-013 requires it, and the E04-A1 row in section 3 lists it. No doc says which VM, distribution or devices to use, so no bead captures it, and E04-A1 delivers `owner-desktop` only. Recommendation: run `cargo xtask capture-fixture minimal-vm` inside an Arch Linux cloud image under QEMU with 2 vCPUs, 2 GiB RAM, one virtio-blk disk, one virtio-net NIC, no zram, no cpufreq and no hwmon, then add it as bead E04-030 under E04-A1.
- **E04-OI-2 udev properties.** PERF-047 (disk kind `ID_BUS=usb`, model `ID_MODEL`), PERF-005 (`disk:<ID_WWN or ID_SERIAL>` keys), PERF-049 (WWN, Serial, Interface) and PERF-056 (adapter name from `ID_MODEL_FROM_DATABASE` and `ID_VENDOR_FROM_DATABASE`) need udev data. ADR-001 allows no libudev binding, and `SysRoot` has no `/run`. Meanwhile E04-021 uses "the canonical sysfs path has a `usb*` component" for USB and `device/model` for the model. `wwn_or_serial` stays `None`. Recommendation: read `/run/udev/data/b<major>:<minor>` and `n<ifindex>` (`E:KEY=value` lines) in a new `parse/udev.rs`, and add `run: PathBuf` to `SysRoot`. This needs an ADR-013 and ADR-016 edit.
- **E04-OI-3 MAC addresses in fixtures.** ADR-013 scrubs users, hostnames and serials but does not mention MAC addresses. E04-005 writes `00:00:00:00:00:00` to be safe. Recommendation: keep that and add MAC addresses to the ADR-013 scrub list.
- **E04-OI-4 `/proc/vmstat` fields.** ADR-004 and ADR-016 list `/proc/vmstat` for the `Memory` worker, but no spec entry uses a vmstat field. `parse/vmstat.rs` is only a stub and no bead fills it. Recommendation: drop vmstat from v1 until a spec needs it, or name the fields (for example `pswpin`, `pswpout`, `pgmajfault` for a swap-activity row).
- **E04-OI-5 PERF-037 Virtualization and security rows (P2).** The epic map gives PERF-037 to E04, but no E04 file in section 2 fits, and the readers need `/proc/cpuinfo` flags, which E06 owns. E06-015 (tasklens-hx1.18) builds these readers in `inventory/cpuinfo.rs` (see E06 open item 8), so E04 has no bead for them. "Running in VM" needs systemd D-Bus and stays with E12. Recommendation: confirm E06 as the owner and move PERF-037 to E06 in section 6.
- **E04-OI-6 `tl-model` totals types.** Section 3 says E04-A2 produces `CpuTotals`, but ADR-003 does not fix the fields of `CpuTotals`, `MemoryTotals`, `DiskTotals`, `NetTotals` or `SensorReading`. E03's beads are not written yet. E04 samplers therefore return their own reading types, and E12 maps them. `CpuTopology` (fixed in ADR-003) is filled directly. Recommendation: B1 (E03) and B4 (E12) agree that E12 does the mapping.
- **E04-OI-7 Disk latency formula.** ADR-016 says Δ(read_ticks + write_ticks) / Δ(ios). PERF-049 adds discard and flush ticks and counts. E04-022 follows ADR-016, which has higher precedence. Recommendation: confirm, or edit ADR-016 to PERF-049's form (the parser already reads discard and flush fields).
- **E04-OI-8 Network classification order.** PERF-054 checks `bridge/` (rule 2) before the Virtual name patterns (rule 6). But its own text and PERF-004 say `docker0` (a bridge) is Virtual and hidden by default, and `virbr0` and `br-*` are bridges too. E04-026 checks the Virtual name patterns first. Recommendation: confirm and edit PERF-054's order.
- **E04-OI-9 Visibility group of Bridge, USB network and Cellular.** PERF-004 has the groups Wired, Wireless, VPN and tunnel, Virtual and Other, and PERF-054 has nine types. The mapping for Bridge, USB network and Cellular is not written. Recommendation: Ethernet and USB network go to Wired; Wi-Fi and Cellular to Wireless; Tunnel and VPN to VPN; Virtual and Bridge to Virtual; Other to Other. The mapping belongs to E12 or E24, not E04.
- **E04-OI-10 Multipass bridge names.** PERF-004 says Virtual covers "Multipass bridges" but gives no name pattern. Recommendation: add `mpqemubr` to `VIRTUAL_NAME_PREFIXES` once confirmed on a Multipass install.
- **E04-OI-11 P2 beads behind no anchor.** E04-013, E04-014, E04-015, E04-019, E04-023, E04-024 and E04-028 are P2 parts that E12's P2 workers (RAPL power, memory details, disk temperature and usage, network details) compile against. Cross-epic edges may only point at anchors, and E04 has no P2 anchor. Recommendation: the leader lets E12 depend on these beads directly, or adds an anchor E04-A4 "P2 system details" that closes them.
- **E04-OI-12 Build rule wording.** The bead brief now says `heavy-build cargo ...`. 75 B2 beads use that form; the leader's final pass rewrites the other 36.
- **E04-OI-13 PERF-050 Partitions table (P2) has no bead.** PERF-050 is E04 data (section 6 row "PERF-047 to PERF-050: E25 (E04, E12)"). E04-021 (tasklens-2dl.24) gives each disk its partitions with number and start sector, and E04-024 (tasklens-2dl.27) gives mounts and usage. Still missing: partition size and offset rows ordered by offset, unallocated gaps over 1 MiB, `ro`, holders, the GPT type GUID name table, GPT attribute flags, and MBR types. File system, label, partition table type, part entry type and flags come from udev (`ID_FS_TYPE`, `ID_FS_LABEL`, `ID_PART_TABLE_TYPE`, `ID_PART_ENTRY_TYPE`, `ID_PART_ENTRY_FLAGS`), which is blocked on E04-OI-2. Recommendation: once E04-OI-2 is settled, add bead E04-030 `parse/partitions.rs` (sysfs fields, udev fields, GUID table, flags, gaps), depending on E04-021, E04-024 and the udev bead, plus a P2 anchor as in E04-OI-11. If E04-OI-1 is accepted first, renumber the `minimal-vm` bead to E04-031.

## E05 collect-process (tasklens-yms)

E05 has 36 beads: the three anchors and 33 others. All of them write `tl-collect` files that section 2 gives to E05.

How the epic is split:

- **Sweep set (P1).** E05-001 is the chain edit: it adds the module lines to `lib.rs` and the E05 variants to `CollectError`. Section 2 says to do this "in their anchor beads", but the anchors close their sets, so the edit has to come first. Next come the sanitizer, one bead per `/proc/<pid>` reader, the cgroup path and stat readers, the fd count, the metadata cache and the sweep core.
- **Apps set (P1).** Unit-name app ids and the app.slice scan, the bwrap unwrap, the desktop index and its watch, classification, app matching, flags, naming, icons and the icon cache.
- **Anchors.** Each anchor is the bead that closes its set:
  - E05-A2 (`AppResolver` in `apps/mod.rs`) depends on every apps bead.
  - E05-A1 (`ProcessSampler` in `proc/sweep.rs`) depends on the sweep set, on E05-A2 and on the PSS reader. So other epics get grouped, named and flagged records from one anchor.
  - E05-A3 is the `hash.rs` bead. It depends on every detail-reader bead, so E10 can depend on one anchor.
- **Detail readers (P2).** `proc/detail.rs` is large, so it is split by tab into six beads that edit it in sequence (threads, fds, maps, security, environ with cwd and root, General static fields).
- **Moved earlier.** The handle walk and the strings scanner core are P3 features. They are written in P2 because section 3 puts `handles.rs` and `strings.rs` in E05-A3, which E10 depends on. Their beads have priority 2.
- **P4.** The launcher split (E05-033) is the only P4 bead.

Choices made inside beads. The docs fix only the outcome of each, so the beads choose the detail. They are recorded here for review:

- The icon cache file layout in `icons.toml` (E05-020).
- The hash cache size (256 entries) and the read buffer (64 KiB) (E05-A3).
- How the optional launcher prefix in `app-<launcher>-<id>-<n>.scope` is resolved: by looking the id up in the desktop index, with no hard-coded list of launcher names (E05-011, E05-016).
- The comm-qualifier prefix rule for the kernel's 15-byte `comm` cut (E05-018).
- Relative `Exec` binaries are resolved through the fixed directory list `/usr/local/bin`, `/usr/bin`, `/bin` (E05-013).
- Localised `Name[xx]=` keys are ignored in v1 (E05-013).
- The capability description wording (E05-028).
- Readers return `tl-collect` structs (`ThreadInfo`, `FdInfo`, `MapNodeInfo`, `SecurityInfo`, `EnvInfo`). E10 maps them into the `tl-model` payload types, so E05 does not depend on how E03 names those fields.

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E05-A1 | tasklens-yms.1 | Process sweep | P1 | E05-010, E05-021, E05-A2 |
| E05-A2 | tasklens-yms.2 | App grouping, naming, icons | P1 | E05-009, E05-012, E05-014, E05-017, E05-018, E05-020 |
| E05-A3 | tasklens-yms.3 | Detail readers | P2 | E05-024, E05-030, E05-031, E05-032 |
| E05-001 | tasklens-yms.4 | Add process modules and CollectError variants to tl-collect | P1 | E02-A1, E04-A1, E03-A1 |
| E05-002 | tasklens-yms.5 | Sanitizer for untrusted process text | P1 | E05-001 |
| E05-003 | tasklens-yms.6 | Parser for /proc/<pid>/stat | P1 | E05-001, E03-A1 |
| E05-004 | tasklens-yms.7 | Parser for /proc/<pid>/status | P1 | E05-001 |
| E05-005 | tasklens-yms.8 | Parsers for /proc/<pid>/statm and io with Restricted | P1 | E05-001 |
| E05-006 | tasklens-yms.9 | cgroup v2 path parsing and placement | P1 | E05-001 |
| E05-007 | tasklens-yms.10 | cgroup stat file readers for apps and units | P1 | E05-006, E04-A1 |
| E05-008 | tasklens-yms.11 | fd count reader and U-34 check | P1 | E05-001 |
| E05-009 | tasklens-yms.12 | Process metadata cache with 10-sweep refresh | P1 | E05-002, E05-003, E05-006 |
| E05-010 | tasklens-yms.13 | Process sweep core with rates and validity | P1 | E05-003, E05-004, E05-005, E05-008, E05-009, E04-A1, E03-A1 |
| E05-011 | tasklens-yms.14 | App id from app.slice unit names and app scan | P1 | E05-006, E05-007 |
| E05-012 | tasklens-yms.15 | Flatpak bwrap unwrap for app-level targets | P1 | E05-011, E03-A1 |
| E05-013 | tasklens-yms.16 | Desktop entry parser and index | P1 | E05-002 |
| E05-014 | tasklens-yms.17 | Desktop index inotify rescan with 500 ms debounce | P1 | E05-013 |
| E05-015 | tasklens-yms.18 | Process classification and GroupKey | P1 | E05-006, E03-A1 |
| E05-016 | tasklens-yms.19 | App matching by unit, exe, argv and Wine | P1 | E05-015, E05-013, E05-011 |
| E05-017 | tasklens-yms.20 | Process flags and the protected set | P1 | E05-016 |
| E05-018 | tasklens-yms.21 | Process display naming and search text | P1 | E05-001 |
| E05-019 | tasklens-yms.22 | Icon theme resolution | P1 | E05-001 |
| E05-020 | tasklens-yms.23 | Icon path cache in icons.toml | P1 | E05-019 |
| E05-021 | tasklens-yms.24 | PSS and USS from smaps_rollup with the FD cache | P1 | E05-003, E04-A1, E03-A1 |
| E05-022 | tasklens-yms.25 | Real-system test: other users' fields are Restricted | P1 | E05-A1 |
| E05-023 | tasklens-yms.26 | Sweep bench with the 15 ms gate | P1 | E05-A1 |
| E05-024 | tasklens-yms.27 | Full smaps parser for per-mapping RSS | P2 | E05-021 |
| E05-025 | tasklens-yms.28 | Threads reader in proc/detail.rs | P2 | E05-004, E05-003, E05-002 |
| E05-026 | tasklens-yms.29 | File descriptor and fdinfo reader | P2 | E05-025 |
| E05-027 | tasklens-yms.30 | Memory maps reader and node grouping | P2 | E05-026, E05-024 |
| E05-028 | tasklens-yms.31 | Security tab reader: credentials, capabilities, namespaces | P2 | E05-027 |
| E05-029 | tasklens-yms.32 | Environment, cwd and root readers | P2 | E05-028 |
| E05-030 | tasklens-yms.33 | General tab static readers: ELF hardening, ASLR, location | P2 | E05-029 |
| E05-031 | tasklens-yms.34 | Handle walk and pipe peers in proc/handles.rs | P2 | E05-027 |
| E05-032 | tasklens-yms.35 | Strings scanner core | P2 | E05-027 |
| E05-033 | tasklens-yms.36 | Launcher split: Steam, Lutris and Heroic games | P4 | E05-A2, E05-029, E05-019 |
| E05-034 | tasklens-yms.37 | Package owner index (`pkg.rs`) (R-163 item 1; "E05-030" in open-items-engine.md, renamed because E05-030 exists) | P2 | E05-A1 |
| E05-035 | tasklens-yms.38 | P2 per-process column readers (`proc/extras_p2.rs`) (R-163 item 2; "E05-031" in open-items-engine.md, renamed because E05-031 exists) | P2 | E05-A1, E05-034 |

### Open items (E05)

1. **Where the extra-column values go in `tl-model`.** `ProcessRecord` (ADR-003) has no fields for the P1 extra columns: `cpu_kernel`, `cpu_user`, `cpu_relative`, CPU times, Start time, `ws_peak`, `ws_shared`, `commit`, `vsize`, `vsize_peak`, page faults, Priority, Affinity, Handles, Image name and path, Unit, State "Frozen". It has no field for the app-row cgroup values either (`anon`, swap, `io.stat` rates). E05-A1 produces them as `tl-collect` types: `ProcessExtras`, aligned by index with the records, and `AppCgroupSample`. Recommendation: E03 adds `ProcessSnapshot.extras: Arc<[ProcessExtras]>` (the type moved into `tl-model`) and `ProcessSnapshot.apps: Arc<[AppCgroupSample]>`, and E09 reads both. The leader settles this with the E03 and E09 writers.
2. **`ProcState` has no `Frozen`, `TracingStop` or `Dead`.** E05-003 maps `t` to `Stopped` and `X` to `Other`. For the State column labels, `ProcessExtras` keeps the raw letter and `frozen`. Recommendation: the UI formats the State column from those two fields, and `ProcState` stays as it is.
3. **Source of `CLK_TCK`.** The workspace `rustix` pin lacks the `param` feature that has `clock_ticks_per_second()`, and `tl-collect` may not use `libc`. `SweepCtx.ticks_per_sec` is an input for now. Recommendation: E02 adds `param` to the `rustix` features, and E10 fills the value from it.
4. **User name lookup.** PROC-028 says `getpwuid_r`, which needs `libc`, and `libc` is not in the `tl-collect` row. E05-009 adds the `UserNames` trait with a numeric fallback. Recommendation: parse `/etc/passwd` in `tl-collect`. Users from NSS sources (LDAP, systemd-homed) then show their numeric uid. The alternative is to add `libc` to the row.
5. **The package index has no file and no bead.** It covers the PROC-027 Version, Build date, Package, Packager and Package status columns, the package `%DESC%` part of Description, and the PROC-078 "Verify against package" check. It needs `pacman -Qoq` batching, the `desc` parser, and the mtree checksum. The mtree file is gzip-compressed, and no gzip crate is allowed in `tl-collect`. The same gap holds for the other P2 per-process columns: File modified time, File size, Architecture column, Session, Security label, Elevated, Sandbox, Scheduling policy, I/O priority, OOM score, TTY, Working directory, Context switches. They need item 1 settled first. Recommendation: after item 1, add `tl-collect/src/pkg.rs` (owner E05) and an E05 P2 bead set for these columns, and decide whether to allow `flate2` or to run `pacman -Qkk` for the check.
6. **How strings scanning reads memory.** PROC-084 says `process_vm_readv`, which needs `libc` and `unsafe`, and `tl-collect` may use neither (ADR-001). Also, `tl-helper` may not depend on `tl-collect`, so the helper's `ScanStrings` cannot reuse E05-032. Recommendation: use `pread` on `/proc/<pid>/mem` through `rustix`, which needs the same ptrace access. Then either move the scanner core to `tl-ops`, or let `tl-helper` depend on `tl-collect`.
7. **inotify on `app.slice` (PROC-005).** The spec also rescans on every sweep, so in a sampler driven by sweeps inotify adds nothing. E05-011 rescans every sweep and has no watch. Recommendation: drop the inotify sentence from PROC-005.
8. **Dumpable flag of another process (PROC-082).** No source is given; `prctl` works only on the caller itself. Recommendation: report "not dumpable" when `/proc/<pid>` is owned by uid 0 while the process's real uid is not 0, and "Not reported" otherwise.
9. **U-21 check (interrupt time accounting).** The data is in `/proc/stat`, which E04 parses, and the Interrupts row is built by E09. PROC-012's `/proc/config.gz` check needs a gzip crate. Recommendation: E04 or E09 owns U-21 and uses only the rule "the `irq` field stays 0 across 10 samples". No E05 bead.
10. **Targets of the app-matching exceptions table.** PROC-006 names the helper binaries but not the desktop ids they map to. Mission Center's `matching.rs:10-24` is not quoted in the docs. E05-016 uses `firefox`, `libreoffice`, `org.gnome.Terminal`, `google-chrome`, `chromium` and `electron`, and falls through to exe matching when an id is missing. Recommendation: check against Mission Center's table before E05-016 starts.
11. **Display name of Lutris and Heroic games (PROC-011, P4).** Only the environment variables are given. E05-033 shows the variable's value. Recommendation: decide when P4 starts.
12. **`Cargo.toml` edits beyond `mod` lines.** E05-023 adds the `[[bench]] sweep` block and `criterion`, and E05-A1 adds `tracing-subscriber` as a dev-dependency. Section 2 allows only module lines as chain edits to the E04-owned `Cargo.toml`. Recommendation: either allow these three lines as E05 chain edits (as the beads are written), or have E04-A1 add them.
13. **Power usage estimate (PROC-034, "E05 estimate" in section 6).** It combines RAPL (E04), process CPU (E05) and GPU data (E13), and it needs item 1 for a place to put the value. Recommendation: the reducer (E09) computes it; no E05 bead.

## E06 collect-netsys (tasklens-hx1)

26 beads: 3 anchors and 23 others. Each anchor in section 3 lists four to six files, so the files are built by small beads (one file each, or a second bead on the same file in sequence) and the anchor is the bead that closes the set: it adds the `mod.rs` re-exports, the fixture snapshot tests and the proptests, and freezes the public API. E06-001 makes the chain edits in E04's `lib.rs` and `error.rs` up front (module lines and the `CollectError` variants) instead of in the anchors, so every later bead compiles on its own; it also creates `tests/prop_netsys.rs` with one section per anchor. E06-A3 also closes the P2 AMD DRM readers (`gpu_drm/{mod,fdinfo,amdgpu}.rs`), because E12's `GpuDrm` worker may depend on E06-A3 only. tl-collect returns its own raw types (`SockRecord`, `ConnRecord`, `StartupRaw`, `InventoryRaw`, `AmdSample`); E12 and E14 map them to the `tl-model` snapshot types, which E03 has not written yet. Readers that need the text sanitizer or the `/proc/<pid>/stat` start-ticks reader depend on E05-A1. D-Bus data (unit timing, hostnamed, systemd virtualization, KScreen) stays in the engine; tl-collect gets the pure parts (impact classification, unit-file parsing).

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E06-001 | tasklens-hx1.4 | Wire E06 modules, error variants and prop_netsys.rs | P2 | E04-A1, E03-A1 |
| E06-002 | tasklens-hx1.5 | sock_diag record types and dump parser | P2 | E06-001, E03-A1 |
| E06-003 | tasklens-hx1.6 | sock_diag netlink dump requests | P2 | E06-002 |
| E06-004 | tasklens-hx1.7 | Socket inode to process map | P2 | E06-001, E05-A1, E03-A1, E04-A1 |
| E06-005 | tasklens-hx1.8 | Per-process TCP attribution | P2 | E06-002, E06-004, E04-A1 |
| E06-006 | tasklens-hx1.9 | Connection records, owners and address formats | P2 | E06-002, E06-004 |
| E06-A1 | tasklens-hx1.1 | Sockets (anchor) | P2 | E06-003, E06-005, E06-006 |
| E06-007 | tasklens-hx1.10 | Other network namespaces list | P4 | E06-A1 |
| E06-008 | tasklens-hx1.11 | Desktop Entry parser for autostart files | P2 | E06-001, E05-A1 |
| E06-009 | tasklens-hx1.12 | Autostart scan, precedence and status | P2 | E06-008 |
| E06-010 | tasklens-hx1.13 | systemd user unit startup rows and unit-name escaping | P2 | E06-009, E05-A1 |
| E06-011 | tasklens-hx1.14 | Plasma pre-startup and logout scripts | P2 | E06-009 |
| E06-012 | tasklens-hx1.15 | Startup impact classification | P2 | E06-001 |
| E06-013 | tasklens-hx1.16 | inotify watch for startup directories | P2 | E06-009 |
| E06-A2 | tasklens-hx1.2 | Startup entries (anchor) | P2 | E06-009 to E06-013 |
| E06-014 | tasklens-hx1.17 | /proc/cpuinfo parser | P2 | E06-001, E05-A1 |
| E06-015 | tasklens-hx1.18 | Virtualization and security feature readers | P2 | E06-014 |
| E06-016 | tasklens-hx1.19 | DMI fields and firmware type | P2 | E06-015, E05-A1 |
| E06-017 | tasklens-hx1.20 | OS release, kernel, install date and package manager | P2 | E06-001, E05-A1 |
| E06-018 | tasklens-hx1.21 | Memory modules from udev without serials | P2 | E06-001, E05-A1 |
| E06-019 | tasklens-hx1.22 | Display connectors from sysfs and EDID | P2 | E06-015 |
| E06-020 | tasklens-hx1.23 | DRM card discovery and PCIe link | P2 | E06-001 |
| E06-021 | tasklens-hx1.24 | DRM fdinfo client parser and engine busy | P2 | E06-020, E04-A1, E03-A1 |
| E06-022 | tasklens-hx1.25 | AMD GPU sysfs, hwmon and gpu_metrics readers | P2 | E06-020, E06-021 |
| E06-023 | tasklens-hx1.26 | Intel GPU readers for i915 and xe | P3 | E06-020, E06-021 |
| E06-A3 | tasklens-hx1.3 | Inventory readers (anchor, also closes the AMD DRM readers) | P2 | E06-014 to E06-022 |

Cross-batch anchor IDs used: E03-A1 tasklens-as2.1, E04-A1 tasklens-2dl.1, E05-A1 tasklens-yms.1.

### Open items (E06)

1. **KMS read-only ioctls for displays (PERF-072, R-057, U-29).** `rustix::ioctl::ioctl` is `unsafe`, ADR-001 allows `unsafe` in three files and none is in tl-collect, and no allowed crate wraps DRM KMS. No bead was written for the KMS path (current mode, refresh, `max bpc`). Recommend adding the `drm` crate (Smithay drm-rs, safe `ControlDevice` API) to tl-collect's ADR-001 row and the ADR-002 pins, then one E06 bead for a KMS part of `inventory/displays.rs` that depends on E06-019. Alternative: add a tl-collect KMS file to the ADR-001 `unsafe` list.
2. **KScreen JSON (PERF-072).** `kscreen-doctor -j` prints JSON and neither tl-collect nor tl-engine may depend on `serde_json` (ADR-001). Recommend allowing `serde_json` in tl-engine's row and parsing it in an E12 worker.
3. **Reverse DNS (SYS-035, P4, `net/rdns.rs`).** `getnameinfo` needs `libc`, which is not in tl-collect's row, and `unsafe`; std has no reverse lookup. No bead written. Recommend putting the call in tl-ops `sys.rs` (already the `unsafe` file with libc) or adding a resolver crate when P4 starts.
4. **Intel i915 PMU (PERF-075).** `perf_event_open` has no safe wrapper in the allowed crates. E06-023 uses the fdinfo fallback only. Recommend keeping fdinfo only; with `perf_event_paranoid = 2` the PMU would fail unprivileged anyway.
5. **xe VRAM total (PERF-064).** "`device/tile0/vram0` sizes" does not name a file. E06-023 leaves VRAM total out for xe. Recommend the leader names the file before P3.
6. **Package owner index (SYS-020 Publisher, R-007 Package column).** SYS-020 puts it in tl-collect, shared with the process Package column, but no file in 00-epics.md section 2 owns it. No bead written. Recommend a new file `crates/tl-collect/src/packages.rs` owned by E05 (Package column, P1), with an E06 bead that fills Publisher after it.
7. **X11 resources column (U-32, PROC-035, P4).** Section 6 assigns U-32 to E06, but it needs `x11rb`, which no crate row allows, and no file is listed. No bead written. Recommend deciding the crate and file when P4 starts.
8. **PERF-037 readers overlap.** PERF-037 (virtualization and security rows) is in E04's listed scope (PERF-033 to PERF-040), while U-27 is E06's. E06-015 builds these readers in `inventory/cpuinfo.rs`; E04 should not build them again.
9. **Intel DRM anchor.** E12's Intel `GpuDrm` bead (P3) needs E06-023, which is behind no anchor. Recommend the leader allows that one edge, or treats E06-023 as part of the E06-A3 surface for P3.
10. **Small choices made in beads (leader to confirm):** IPv6 link-local scope written as the interface name, numeric when unknown (E06-006); autostart status precedence NotInThisDesktop, then MissingProgram, then Disabled (E06-009); autostart and unit files read at most 64 KiB (E06-008, E06-010); per-engine fdinfo busy summed over clients and clamped to 100% (E06-021); first TCP attribution pass is WarmingUp and a new socket adds nothing in its first interval (E06-005); install date from pacman.log only in the `[YYYY-MM-DDTHH:MM:SS+HHMM]` form (E06-017).
11. **Page size row (PERF-098).** `rustix::param::page_size` needs rustix feature `param`, which the ADR-002 pin does not enable. Not read in E06. Recommend E02 adds `param` to the rustix features and E12 reads the value.

## E07 ops (tasklens-iuz)

17 beads: the three anchors plus 14 work beads. Each anchor covers more files than one bead may touch, so each one is the closing bead of its set. The work beads build the files one responsibility at a time. The anchor then fixes the public surface, adds rustdoc and adds the real-system tests to `tests/ops_real.rs`. Section 3 still holds: E07-A1 delivers `error.rs`, `sys.rs`, all of `process.rs` and `tests/ops_real.rs`. E07-A2 delivers `cgroup.rs` and the validation tests. E07-A3 delivers `fs.rs`, `launch.rs` and `dump.rs`. `process.rs` is built by five beads in a fixed order (003, 004, 005, 006, 007). `tests/ops_real.rs` is appended in the order A1, A2, A3. Section 3 puts `SetSched`, `SetIoPriority` and thread targets in E07-A1, so E07-005 and E07-006 have priority 1 even though the UI that uses them is P2. E07-010 (`write_atomic`) has priority 1 because the P1 settings store in E33 needs it. Every bead is labelled `security`.

| Plan key | Bead ID | Title | Phase | Depends on |
|---|---|---|---|---|
| E07-001 | tasklens-iuz.4 | tl-ops crate root and OpsError | P1 | E02-A1, E03-A1, E03-A2 |
| E07-002 | tasklens-iuz.5 | sys.rs unsafe wrappers: ioprio_set, sched_setattr, tgkill, posix_spawnp with setsid | P1 | E07-001 |
| E07-003 | tasklens-iuz.6 | open_verified and pidfd signals | P1 | E07-002, E03-A1, E03-A2 |
| E07-004 | tasklens-iuz.7 | Per-TID loop with the pidfd poll rule, set_nice, set_affinity | P1 | E07-003 |
| E07-005 | tasklens-iuz.8 | set_sched and set_io_priority on every TID | P1 (priority 1, feature P2) | E07-004, E07-002 |
| E07-006 | tasklens-iuz.9 | Thread targets: signal, nice, sched, affinity on one TID | P1 (priority 1, feature P2) | E07-005 |
| E07-007 | tasklens-iuz.10 | kill_tree: SIGSTOP all, then SIGKILL all | P1 | E07-003 |
| E07-A1 | tasklens-iuz.1 | Verified process operations | P1 | E07-003, E07-004, E07-005, E07-006, E07-007 |
| E07-008 | tasklens-iuz.11 | cgroup open_validated and validation tests | P1 | E07-001, E03-A2 |
| E07-009 | tasklens-iuz.12 | cgroup freeze, thaw, kill and the frozen wait | P1 | E07-008 |
| E07-A2 | tasklens-iuz.2 | cgroup operations | P1 | E07-009, E07-A1 (same test file) |
| E07-010 | tasklens-iuz.13 | fs::write_atomic | P1 | E07-001 |
| E07-011 | tasklens-iuz.14 | Autostart override write and remove, script toggle, confined delete | P2 | E07-010 |
| E07-012 | tasklens-iuz.15 | dump.rs: core dump into a passed fd | P2 | E07-003 |
| E07-013 | tasklens-iuz.16 | Desktop Entry Exec parser | P2 | E07-001 |
| E07-014 | tasklens-iuz.17 | Detached spawn, shell and pkexec argv, child reaping | P2 | E07-002, E07-013 |
| E07-A3 | tasklens-iuz.3 | Atomic files and launch | P2 | E07-010, E07-011, E07-012, E07-013, E07-014, E07-A2 (same test file) |

### Open items (E07)

- **E07-O1: how the dump tool writes into a passed fd (blocks E07-012 and so E07-A3).** R-004 and ADR-006 say the dump runs `/usr/bin/gcore` by fixed path and writes to a passed fd. `gcore -o <prefix> <pid>` always writes `<prefix>.<pid>`, so it cannot write to an fd or to the exact path the UI polls for progress (UI-PWN-034). Recommendation: run `/usr/bin/gdb --nx --nh --batch -p <pid> -ex "gcore /proc/<tasklens pid>/fd/<n>"`, which is the command `gcore` runs internally, and change "gcore by fixed path" to "gdb by fixed path" in R-004 and ADR-006. Alternative: let gcore write into a private temp directory and rename or copy the result. Progress then stays at "Writing core…" until the end, and the helper needs a root-private temp directory.
- **E07-O2: rule for re-enabling a system autostart entry.** ADR-016 says to remove an override "that differs only in `Hidden`", otherwise set `Hidden=false`. SYS-022 says to delete the user file if it carries `X-Tasklens-Override=true`, otherwise set `Hidden=false` and remove `X-GNOME-Autostart-enabled=false`. Under SYS-022, a tasklens override the user later edited by hand is deleted; under ADR-016 it is kept. E07-011 is written to SYS-022 (later and more specific). Recommendation: keep SYS-022 and edit the ADR-016 row to match.
- **E07-O3: `SCHED_RESET_ON_FORK` for Custom FIFO and RR.** ADR-006 and R-038 set the flag for the Realtime menu item (`SCHED_RR` 1). `ActionKind::SetSched { policy, rt_prio }` cannot tell the Realtime item from a Custom RR 1 request. E07-005 sets the flag for every FIFO and RR request. Recommendation: accept this, because children of a real-time process should not inherit real-time priority by accident.
- **E07-O4: E07-A3 is P2, but `write_atomic` is needed at P1.** E33's settings store (tasklens-cr0.11, P1) and autostart writer (tasklens-cr0.17) already depend on E07-A3. That anchor also waits for launch, Exec parsing and the dump (blocked by O1). Recommendation: let tasklens-cr0.11 and tasklens-cr0.17 depend on E07-010 (tasklens-iuz.13) instead of E07-A3, as a named exception to the anchors-only rule. Otherwise the P1 settings store waits for P2 work.
- **E07-O5: environment for "Run as administrator".** PROC-095 writes `pkexec env DISPLAY=… WAYLAND_DISPLAY=… XDG_RUNTIME_DIR=… /bin/sh -c <text>` and does not say what happens when a variable is unset. E07-014 omits an unset or empty variable and passes exactly these three keys. Recommendation: accept.
- **E07-O6: mode of new autostart overrides and the desktop-file read cap.** E07-011 creates new overrides with mode `0600` (the ADR-011 rule for files tasklens creates) and keeps the current mode when it rewrites an existing file. KDE's own Autostart module writes `0644`. It also refuses desktop files over 1 MiB. Recommendation: accept both.
- **Note for E05 and E10 (not an E07 item):** the Set I/O Priority check mark needs the current value from `ioprio_get`, which needs `unsafe`. Only `tl-ops/src/sys.rs` may contain `unsafe`. E07-005 adds `sys::ioprio_get_tid` as `pub(crate)`. If a reader in tl-collect or the engine needs it, `tl-ops` must make it public, or ADR-001 must change.

## Cross-batch edges

Outgoing edges (B2 beads that depend on other batches) all point at anchors:

| B2 bead | Depends on |
|---|---|
| E04-001 | E02-A1 (tasklens-v9m.1), E03-A1 (tasklens-as2.1) |
| E04-005, E04-006 | E02-A1 |
| E05-001 | E02-A1, E03-A1 (and E04-A1 inside B2) |
| E05-003, E05-010, E05-012, E05-015, E05-021 | E03-A1 |
| E06-001, E06-002, E06-004, E06-021 | E03-A1 |
| E07-001 | E02-A1, E03-A1, E03-A2 (tasklens-as2.2) |
| E07-003 | E03-A1, E03-A2 |
| E07-008 | E03-A2 |

Incoming edges from other batches, as found in the database on 2026-10-03. All point at B2
anchors, which is the rule. The leader should check this list against the other plan files when
resolving plan keys.

| Bead | Depends on |
|---|---|
| tasklens-8fx.16 (E30) | E04-A3 |
| tasklens-cr0.11, tasklens-cr0.17 (E33) | E07-A3 (see E07-O4: should be E07-010) |
| tasklens-m1f.2 (E32) | E07-A1, E07-A2 |
| tasklens-m1f.8 (E32) | E05-A1, E05-A2 |
| tasklens-qfx.5, tasklens-qfx.7 (E29) | E07-A1 |
| tasklens-qfx.6 (E29) | E07-A2 |
| tasklens-qfx.8 (E29) | E05-A3 |
| tasklens-qfx.9, tasklens-qfx.12 (E29) | E07-A3 |
| tasklens-qfx.22 (E29) | E04-A1 |
| tasklens-y6k.2 (E14) | E05-A1, E07-A3 |
| tasklens-y6k.3 (E14) | E06-A2, E07-A3 |
| tasklens-y6k.5 (E14) | E05-A1 |
| tasklens-y6k.7 (E14) | E05-A1, E05-A2 |
| tasklens-y6k.15, tasklens-y6k.16 (E14) | E06-A2 |
| tasklens-y6k.18, tasklens-y6k.19 (E14) | E06-A1 |
| tasklens-y6k.24 (E14) | E07-A3 |

No E10, E11, E12 or E13 bead depended on a B2 anchor at the time of this check, although the epic
map's section 4 has E10 on E05, E11 on E07 and E12 on E04 and E06. Those batches (B3, B4) add the
edges, or the leader adds them from the plan keys.

Beads that other epics need but that sit behind no anchor: E04-013, E04-014, E04-015, E04-019,
E04-023, E04-024, E04-028 (E04-OI-11), E06-023 (E06 open item 9) and E07-010 (E07-O4).

## Resume note

The first B2 writer was stopped by a terminal restart after creating all 111 beads. The resumed
writer found every bead present and no placeholder text left. It then:

- added 37 missing dependency edges (the tail of the E04-A3 list and E05-021 onward), which the
  first writer's dependency script had not reached;
- replaced the long `flock ... systemd-run ...` build line with `heavy-build` in 75 of the 111
  bead descriptions, then stopped at the leader's request. The leader rewrites the rest in one
  pass over all beads. E04-017, E04-020 and E04-025 also lack a `## Notes` heading (their notes
  lines sit under Cross-cutting), so that pass should add it;
- checked all 111 beads with `br dep list`: every planned edge is present, no unplanned edge,
  and `br dep cycles` finds no cycle (2026-10-04). `br lint` flags only the four epic beads;
- checked spec coverage against section 6 and added E04-OI-13 (PERF-050 Partitions table);
- wrote this file.

Drafts, ID maps and the edge scripts (`b2/add_missing_deps.py`, `b2/verify.py`, both read the
database only through `br`) are in the B2 scratch folders
(`scratchpad/e04` to `e07` and `scratchpad/b2`).
