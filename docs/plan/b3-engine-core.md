# B3 engine-core: bead plan for E08, E09, E10, E11, E13

Batch B3 of the bead-writing round (`docs/plan/00-epics.md` section 5). This file maps every plan
key to its bead ID, lists the shared design points the beads build on, and records the open items
for the leader. The epic and anchor beads were created by the leader (`docs/plan/anchors.md`); the
anchor descriptions were replaced with the full template by this batch.

Rules followed: `docs/research/brief-beads.md`. Every bead carries the build rule in its Notes
(`heavy-build cargo ...`). Priority equals phase; the beads that E08-A1 and E09-A1 need are P0
because the phase-0 spike (E01) depends on those anchors.

Bead counts: E08 11, E09 30, E10 11, E11 24, E13 15 (anchors included), 91 in total.

## Shared design points

These are fixed in E08 beads and used by every other engine epic. Items marked (open) are listed
again under "Open items" because the ADRs do not spell them out.

- `SourceUpdate` (E08-003, `tl-engine/src/update.rs`) carries every variant for every phase.
  Device samples are keyed by `DeviceIdentity`; the reducer owns `DeviceIdx`. (open item 3)
- `WorkerIo` has a fourth field `control: watch::Receiver<WorkerControl>` beside `updates`,
  `demand` and `settings`, and a fifth field `shared: SharedHandles` (open item 8).
  `WorkerControl { reset_generation, commands: Arc<[QueuedCommand]> }` carries baseline resets
  after resume and the reducer-to-worker commands (`RefreshNow`, `KWin`, `DetailRefresh`,
  `HandleSearch`, `HistoryQuery` and the rest) as a latest-wins queue of the newest 64 entries, so
  no restartable worker owns an `mpsc::Receiver`. (open item 1)
- `EngineMsg` has a fourth variant `Tick`, sent every 1 s by the session for stale marking and
  linger expiry. (open item 4)
- `workers/mod.rs::all_workers()` is the worker registry. Each worker bead appends one
  `WorkerSpec { id, name, modes, factory }` line (chain edit on E08-A1).
- `Source` has two defaulted methods beyond ADR-004's three: `reset_baselines(&mut self)` and
  `command(&mut self, &WorkerCommand) -> bool`. Workers that own a child process or a bus
  connection (logind, GpuNvidia, the D-Bus workers) do not use `source::run`; they write their own
  loop with the same interval pattern and return `Outcome::Err` when they need a restart.
- `dbus/mod.rs` (E08-009) owns `ManagerConnSlot` and `SharedHandles { systemd_system, systemd_user }`.
  The systemd workers (E14-001) fill the slots; E11 unit actions, the launcher and the opener read
  them. (open item 8)
- `EngineHooks.action_gate` takes a `HookCtx { events_tx, msgs_tx, boot_id, config, shared }`
  (E09-018); E11-A1's `gate_factory` is that hook. (open item 8)
- The privileged helper reaches E11 through `actions::PrivilegedOps` (E11-002), the shape E29-OI-6
  asked for; E29-014 implements it for `HelperHandle`. (open item 9)

## E08 engine-runtime

Epic bead `tasklens-8hz`. 9 task beads plus 2 anchors.

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E08-001 | tasklens-8hz.3 | `EngineConfig` and `EngineMode` (`config.rs`) | P0 |
| E08-002 | tasklens-8hz.4 | `build_runtime` with the ADR-004 `RuntimeBuilder` settings | P0 |
| E08-003 | tasklens-8hz.5 | `EngineMsg`, `SourceUpdate` (every variant) and `WorkerError` (`update.rs`) | P0 |
| E08-004 | tasklens-8hz.6 | `Source` trait, `WorkerIo`, `WorkerControl` and the sampling loop (`source.rs`) | P0 |
| E08-005 | tasklens-8hz.7 | Per-worker `ManagedSupervisor` with backoff and budget (`supervise.rs`) | P0 |
| E08-006 | tasklens-8hz.8 | `SourceController` with the 60 s disable (`controller.rs`) | P0 |
| E08-007 | tasklens-8hz.9 | Fake sources and `Source`-loop LabRuntime tests | P0 |
| E08-008 | tasklens-8hz.10 | Demand-driven rate LabRuntime tests (`lab_demand.rs`) | P0 |
| E08-A1 | tasklens-8hz.1 | Runtime, supervision, `Source` loop (`lib.rs`, `workers/mod.rs`, `Cargo.toml`, end-to-end test) | P0 |
| E08-009 | tasklens-8hz.11 | zbus connection helper with executor tick and timeouts (`dbus/mod.rs`) | P1 |
| E08-A2 | tasklens-8hz.2 | D-Bus helper and logind worker (`workers/logind.rs`) | P1 |

Dependencies inside the epic: 001 and 002 on E02-A1 (001 also on E03-A2); 003 on E03-A2 and 002;
004 and 005 on 003; 006 on 004 and 005; 007 on 004 and 005; 008 on 007; A1 on 001 to 008 and
E02-A2; 009 on A1; A2 on A1 and 009.

## E09 engine-reducer-bridge

Epic bead `tasklens-8q0`. 28 task beads plus 2 anchors. Files: `session.rs`, `reducer/*.rs`,
`bridge.rs`, `tests/lab_{reducer,session,bridge,table,suspend}.rs`, `tests/real_shutdown.rs`.

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E09-001 | tasklens-8q0.3 | Reducer state, loop, publish and the RecordSink hook (reducer/mod.rs) | P0 |
| E09-002 | tasklens-8q0.4 | SeriesStore fast tier with gaps and Arc reuse (reducer/series.rs) | P0 |
| E09-003 | tasklens-8q0.5 | device catalog with generations (reducer/devices.rs) | P0 |
| E09-004 | tasklens-8q0.6 | health state and stale marking (reducer/health.rs) | P0 |
| E09-005 | tasklens-8q0.7 | process cache with linger, NEW and EXITED (reducer/processes.rs) | P0 |
| E09-007 | tasklens-8q0.9 | apply.rs dispatch for every EngineMsg and Command | P0 |
| E09-009 | tasklens-8q0.11 | table builder flat mode, sort, tie-break, filter (reducer/table.rs) | P0 |
| E09-010 | tasklens-8q0.12 | table builder tree mode, forest, PID +N, include children | P0 |
| E09-011 | tasklens-8q0.13 | table builder grouped mode, app and unit rows, Kernel threads row | P0 |
| E09-014 | tasklens-8q0.16 | lab_table.rs harness and the table builder tests | P0 |
| E09-015 | tasklens-8q0.17 | sys_table.rs stub with full signatures and default sorts | P0 |
| E09-016 | tasklens-8q0.18 | engine_root and engine_session with epochs and catch_unwind (session.rs) | P0 |
| E09-017 | tasklens-8q0.19 | chaos explore_seeds runs and the fake worker set | P0 |
| E09-018 | tasklens-8q0.20 | EngineHandle implementing EngineClient and EngineHooks (bridge.rs part 1) | P0 |
| E09-019 | tasklens-8q0.21 | Engine::start (bridge.rs part 2) | P0 |
| E09-020 | tasklens-8q0.22 | bridge tests: 10,000 publishes, never blocks, Busy (lab_bridge.rs) | P0 |
| E09-021 | tasklens-8q0.23 | reducer tests: sequence, versions, Tick effects (lab_reducer.rs) | P0 |
| E09-A1 | tasklens-8q0.1 | Engine publishes snapshots | P0 |
| E09-006 | tasklens-8q0.8 | GPU pid mapping and PSS coverage in process records | P1 |
| E09-008 | tasklens-8q0.10 | suspend and resume handling (reducer/suspend.rs, lab_suspend.rs) | P1 |
| E09-012 | tasklens-8q0.14 | hide system processes in the table builder | P1 |
| E09-013 | tasklens-8q0.15 | two-line header totals (TableTotals) | P1 |
| E09-A2 | tasklens-8q0.2 | Shutdown | P1 |
| E09-022 | tasklens-8q0.24 | 1 Hz series tier with per-second max | P2 |
| E09-023 | tasklens-8q0.25 | pseudo rows Interrupts and Memory compression (zram) | P2 |
| E09-024 | tasklens-8q0.26 | subtree totals on parent processes | P2 |
| E09-025 | tasklens-8q0.27 | unit row names from systemd Description | P2 |
| E09-026 | tasklens-8q0.28 | DetailsSnapshot assembly with Arc reuse and demand retain | P2 |
| E09-027 | tasklens-8q0.29 | P2 components (connections, services, startup, inventory, windows) and sys_table calls | P2 |
| E09-028 | tasklens-8q0.30 | HistoryView component and the remaining RecordSink calls | P3 |
| E09-029 | tasklens-8q0.31 | Per-process series from Demand.graph_targets (R-154) | P2 |
| E09-030 | tasklens-8q0.32 | Process power estimate, PROC-034 (R-163 item 9) | P2 |

Dependencies: 001 on E08-A1 and E03-A2; 002, 003, 004 on E03-A1; 005 and 009 on E03-A2 and
E08-003; 006 on 005 and A1; 007 on 001 to 005; 008 on A1; 010 on 009; 011 on 010; 012 on 011;
013 on 009; 014 on 009 to 011; 015 on E03-A2; 016 on 001, 007, E08-A1; 017 on 016; 018 on 001 and
E08-A1; 019 on 018, 016, E08-002; 020 on 019 and 017; 021 on 007; A1 on 001 to 005, 007, 009 to
011, 014 to 021, E08-A1, E08-A2, E03-A2; A2 on A1; 022, 024, 025, 026, 028 on A1; 023 on A1 and
012; 027 on A1 and 026; 029 on A1 and E03-A2 (wox.9 and E26-011 depend on 029); 030 on A1,
E12-008 (hjn.11) and E13-A1 (ndy.2).

## E10 engine-processes

Epic bead `tasklens-dwy`. 8 task beads plus 3 anchors. Files: `workers/processes.rs`,
`workers/process_detail.rs`, `workers/handle_search.rs`, `tests/lab_processes.rs`,
`tests/lab_details.rs`.

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E10-001 | tasklens-dwy.4 | ProcessesSource wrapping the tl-collect sweep (workers/processes.rs) | P1 |
| E10-002 | tasklens-dwy.5 | Processes worker LabRuntime tests on fixtures (lab_processes.rs) | P1 |
| E10-003 | tasklens-dwy.6 | ProcessDetail PSS and USS for detail_keys through the FD cache | P1 |
| E10-A1 | tasklens-dwy.1 | Processes worker | P1 |
| E10-A2 | tasklens-dwy.2 | Process detail worker | P1 |
| E10-004 | tasklens-dwy.7 | ProcessDetail Properties tab payloads at the ADR-008 rates | P2 |
| E10-005 | tasklens-dwy.8 | ProcessDetail hashes, VerifyPackage and affinity masks | P2 |
| E10-007 | tasklens-dwy.10 | lab_details.rs harness and the ProcessDetail tests | P2 |
| E10-006 | tasklens-dwy.9 | ProcessDetail StringScan with cancel and limits | P3 |
| E10-008 | tasklens-dwy.11 | handle search walk with globs, progress and cancel (workers/handle_search.rs) | P3 |
| E10-A3 | tasklens-dwy.3 | Handle search | P3 |

Dependencies: 001 on E05-A1, E05-A2, E08-A1; 002 on 001 and E04-A1; A1 on 001, 002, E09-A1,
E08-A1, E04-A1; 003 on E05-A3, E04-A1, E08-A1; A2 on 003, E09-A1, E08-A1; 004 on A2 and E05-A3;
005 on 004; 006 on A2, 004, E05-A3; 007 on 005; 008 on E05-A3, E08-A1, A2; A3 on 008, A2 and
E09-026.

## E11 engine-actions

Epic bead `tasklens-i3o`. 22 task beads plus 2 anchors. Files: `actions/mod.rs`,
`actions/{process,cgroup,unit,startup,targetless,dump,socket,recovery}.rs`,
`tests/lab_actions.rs`, `tests/real_actions.rs`, `tests/real_cgroup.rs`. `targetless.rs` is a new
file beside the ones `00-epics.md` lists (E11 owns the directory); E29 adds `actions/helper.rs`.

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E11-001 | tasklens-i3o.3 | action admission rules (actions/mod.rs part 1) | P1 |
| E11-002 | tasklens-i3o.4 | run_action task, deadlines, event order, hooks (actions/mod.rs part 2) | P1 |
| E11-003 | tasklens-i3o.5 | process signals, suspend and resume through the verified pidfd (actions/process.rs part 1) | P1 |
| E11-004 | tasklens-i3o.6 | kill_tree: SIGSTOP all, then SIGKILL all | P1 |
| E11-005 | tasklens-i3o.7 | nice and affinity with the pidfd poll rule | P1 |
| E11-007 | tasklens-i3o.9 | lab_actions.rs part 1: FakeOps harness and the P1 process cases | P1 |
| E11-008 | tasklens-i3o.10 | real_actions.rs: process actions on real children | P1 |
| E11-009 | tasklens-i3o.11 | cgroup freeze, thaw and kill on app.slice cgroups (actions/cgroup.rs) | P1 |
| E11-010 | tasklens-i3o.12 | real_cgroup.rs: transient scope freeze, thaw, kill | P1 |
| E11-A1 | tasklens-i3o.1 | Action pipeline for processes | P1 |
| E11-006 | tasklens-i3o.8 | sched policy, I/O priority, Send Signal numbers and thread targets | P2 |
| E11-011 | tasklens-i3o.13 | unit start, stop, restart, reload, reset-failed with the JobRemoved wait (actions/unit.rs part 1) | P2 |
| E11-012 | tasklens-i3o.14 | unit freeze, thaw and kill through the manager | P2 |
| E11-013 | tasklens-i3o.15 | unit enable, disable, mask, unmask and Reload | P2 |
| E11-014 | tasklens-i3o.16 | startup enable and disable (actions/startup.rs part 1) | P2 |
| E11-015 | tasklens-i3o.17 | startup Run now and Delete entry | P2 |
| E11-016 | tasklens-i3o.18 | targetless kinds OpenUri, ShowInFileManager, Launch and RealOpeners | P2 |
| E11-017 | tasklens-i3o.19 | lab_actions.rs part 2: unit, startup, opener and cgroup cases | P2 |
| E11-022 | tasklens-i3o.24 | real_actions.rs: user-manager unit actions and recovery drop-in | P2 |
| E11-A2 | tasklens-i3o.2 | cgroup, unit and startup actions | P2 |
| E11-018 | tasklens-i3o.20 | Dump direct or through the helper (actions/dump.rs) | P3 |
| E11-019 | tasklens-i3o.21 | SockDestroy through the helper and destroy_supported (actions/socket.rs) | P3 |
| E11-020 | tasklens-i3o.22 | recovery drop-in write and remove (actions/recovery.rs) | P3 |
| E11-021 | tasklens-i3o.23 | lab_actions.rs part 3: FakePrivilegedOps, dump, socket, recovery cases | P3 |

Dependencies: 001 on E09-A1 and E03-A2; 002 on 001, E08-A1, E08-009; 003 on 002 and E07-A1; 004
on 003; 005 and 006 on 003 or 005 plus E07-A1; 007 on 001 to 005; 008 on 003 to 005; 009 on 002
and E07-A2; 010 on 009; 011 on 002, E08-009, E14-A1; 012 and 013 on 011; 014 on 002, 013, E07-A3;
015 on 014, 013, E14-A3; 016 on 002 and E14-A3; 017 on 006, 007, 009, 011 to 016; 018 on 002,
E07-A3, E03-A2; 019 on 002 and E03-A2; 020 on 013, E07-A3, E03-A2; 021 on 017 to 020; 022 on
008, 011, 013, 020; A1 on 001 to 005, 007, 008, E09-A1, E08-A1, E07-A1; A2 on A1, 009 to 017,
E07-A2, E14-A1, E14-A3.

The deadline table every E11 bead uses is written out in E11-002; the unit job kinds have no
client-side timeout (ADR-005 wins over ADR-006's 30 s row, open item 15).

## E13 gpu-probe

Epic bead `tasklens-ndy`. 13 task beads plus 2 anchors. Files: `crates/tl-gpu-probe/*`,
`tl-engine/src/gpu/{mod,spawn,client}.rs`, `tl-engine/src/workers/gpu_nvidia.rs`,
`tests/probe_integration.rs`, `tests/real_nvml.rs`.

| Plan key | Bead ID | Title | Phase |
|---|---|---|---|
| E13-001 | tasklens-ndy.3 | tl-gpu-probe crate, argument modes and the fd 3 sampler loop (main.rs, sampler.rs) | P1 |
| E13-002 | tasklens-ndy.4 | NVML init, device enumeration and Hello (nvml.rs part 1) | P1 |
| E13-003 | tasklens-ndy.5 | NVML per-device sample with Reading per metric | P1 |
| E13-004 | tasklens-ndy.6 | NVML per-process utilisation and memory rows | P1 |
| E13-005 | tasklens-ndy.7 | --fake <script> backend and fake_scripts.rs | P1 |
| E13-007 | tasklens-ndy.9 | gpu/mod.rs: probe lookup order, ProbeError, deadlines | P1 |
| E13-008 | tasklens-ndy.10 | gpu/spawn.rs: socketpair, pre_exec dup2, death signal (the one unsafe file) | P1 |
| E13-009 | tasklens-ndy.11 | gpu/client.rs: one outstanding request, poll, deadlines, kill and reap | P1 |
| E13-010 | tasklens-ndy.12 | GpuNvidia worker with the failure table and 300 s retry (workers/gpu_nvidia.rs) | P1 |
| E13-012 | tasklens-ndy.14 | probe_integration.rs: client, worker and supervisor cases | P1 |
| E13-013 | tasklens-ndy.15 | real_nvml.rs: Hello, samples, process rows, kill and restart on the owner machine | P1 |
| E13-A1 | tasklens-ndy.1 | Probe binary | P1 |
| E13-A2 | tasklens-ndy.2 | GpuNvidia worker | P1 |
| E13-006 | tasklens-ndy.8 | --api-versions one-shot mode with ash and khronos-egl | P2 |
| E13-011 | tasklens-ndy.13 | api-versions run once at start with a 5 s deadline | P2 |

Dependencies: 001 on E02-A1 and E03-A2; 002 on 001; 003 on 002; 004 on 003; 005 and 006 on 001;
A1 on 001 to 005 and E03-A2; 007 on E08-A1; 008 on 007; 009 on 008 and E03-A2; 010 on 009,
E08-A1, E03-A2; 011 on 009 and 006; 012 on 010, 011, A1, E08-A1; 013 on 010, 011, A1; A2 on 010
to 013, A1, E09-A1, E08-A1, E03-A2.

## Open items

Numbered across the batch. Each has a recommendation; the beads are written to the
recommendation so nothing waits, and the leader can veto by editing the named beads.

1. **Reducer-to-worker control channel.** ADR-004's `WorkerIo` has three fields, but ADR-008
   needs `ResetBaselines` to reach every worker after resume, and ADR-005 routes `RefreshNow`,
   `ReprobeSource`, `KWin`, `DetailRefresh`, `ComputeHashes`, `VerifyPackage`, `StringScan`,
   `HandleSearch`, `HistoryQuery`, `HistoryDelete`, `SetupStatus` and `RunSetup` to specific
   workers. Recommendation (built into E08-004): a fourth `watch::Receiver<WorkerControl>` with a
   `reset_generation` counter and a latest-wins queue of the newest 64 `QueuedCommand`s. One
   sentence in ADR-004 to update. Affected beads: E08-004, E09-007.
2. **`EngineConfig` fields.** The ADRs name the type but not its fields. Recommendation
   (E08-001): `mode`, `sys_root: tl_collect::SysRoot`, `state_dir: PathBuf`.
3. **`SourceUpdate` payload field sets.** `00-epics.md` assigns the full variant list to E08 and
   the ADRs give only the component types. E08-003 derives each payload from ADR-003 and
   ADR-016 (for example `CpuSample.irq_pct` for the Interrupts row, `ProcessSweep.app_stats`
   from R-137, `ProcessSweep.file_nr` for the Handles total). B4 (E12, E14) must build to these
   payloads; the leader should hand B4 the E08-003 text.
4. **Housekeeping tick.** Stale marking (`now - t_last > max(3 × period, 1.5 s)`) and exited-row
   linger expiry need the reducer to run when no message arrives. Recommendation (E08-003,
   E09-016): `EngineMsg::Tick` every 1 s from a session task.
5. **`tl-model` table and series fields E09 needs.** ADR-003 does not list them. Recommendation
   (used by E09-009 to E09-013, E09-022 to E09-024): `TableRow.descendants: u32` and
   `TableRow.subtree: Option<u32>` (index into `ProcessTableView.subtree: Vec<GroupRow>`, PROC-018,
   PROC-019); `TableQuery.include_children: bool`; `TableTotals { cpu_pct, memory_bytes,
   disk_bps, net_bps, gpu_pct, processes, threads, handles }` with one `Validity` each;
   `GroupKind::{App, Unit, KernelThreads, Pseudo}` and `GroupRow { key, name, icon, cpu_pct,
   memory_bytes, disk_bps, net_bps, gpu_pct, pids, note: Option<Arc<str>> }`;
   `SeriesSet.slow: Arc<[Arc<SeriesView>]>` for the 1 Hz tier. All E03 edits (B1).
6. **Columns with no `ProcessRecord` field.** E09-009's `sort_key` returns `Absent` for a
   `ColumnId` whose data is not in the record (listed in that bead's doc comment); such columns
   sort last and the header shows the "Only measured for visible rows" tooltip where ADR-008 says
   so. The leader confirms the list against R-007.
7. **Who reads `/proc/sys/fs/file-nr`.** Recommendation (E09-013): the E05 sweep fills
   `ProcessSweep.file_nr` once per sweep; the reducer copies it into `TableTotals.handles`.
8. **Shared handles and the action-gate hook.** B4 open item 2 asked how the systemd workers'
   `ManagerConnSlot` reaches E11, the launcher and the opener, and how the gate gets its channels.
   Recommendation (written into E08-004, E08-009, E09-016, E09-018, E11-002, E11-A1):
   `dbus/mod.rs` owns `ManagerConnSlot` and `SharedHandles`; `WorkerIo` gains `shared`;
   `engine_session` creates one `SharedHandles` per session; `EngineHooks.action_gate` becomes
   `Fn(HookCtx) -> Box<dyn ActionGate>`. E14-001 must use the E08 type instead of its own
   (one-line change for B4). `HookCtx` is `#[doc(hidden)]` because it names an asupersync sender.
9. **Privileged helper hook.** E29-OI-6 asked E11 to define the trait. Recommendation (E11-002):
   `PrivilegedOps { call(cx, HelperRequest, Option<OwnedFd>, deadline) -> Result<Vec<HelperReply>,
   HelperCallError>; state() -> HelperState }` and `HelperCallError { Missing, Cancelled,
   NotAuthorized, Timeout, Exited, Protocol }`, both in `actions/mod.rs`. E29-014 implements it and
   adds the `req.privileged` routing arm; until then a privileged request is `Rejected(Unsupported)`.
10. **"Already running" for Startup Run now.** `ActionState` has no value for it. Recommendation
    (E11-015, E11-016): the launcher's `AlreadyRunning` becomes `Rejected(Busy)` on the request;
    the UI maps `StartupRunNow` + `Rejected(Busy)` to the toast "Already running" (E28).
11. **GPU devices in the catalog.** `DeviceList` is a complete list per kind, so `GpuNvidia` and
    `GpuDrm` cannot both send `GPUS` lists. Recommendation (E13-010, E09-007): neither sends a
    `GPUS` list in P1; the reducer's `Gpu` and `GpuInfo` arms call `devices.ensure` per identity, as
    the disk, net and sensor arms do; `GpuTotals.device` is a placeholder the reducer overwrites.
    A GPU that disappears stays `present` until the catalog grows a per-source list (P3 at the
    earliest). One sentence for E09-007 and E12's enumerate bead.
12. **PERF-071 static GPU fields are not in the probe protocol.** CUDA version, VBIOS, architecture
    and PCIe throughput have no field in `GpuDeviceInfo`, `GpuTotals` or `GpuStaticInfo`.
    Recommendation: add `cuda_version: Option<String>`, `vbios: Option<String>`,
    `architecture: Option<String>` to `GpuDeviceInfo` (E03-069) and `pcie_rx_bps`, `pcie_tx_bps`
    to `GpuTotals` (E03-009); E13-002 and E13-010 fill them when present and skip them otherwise.
13. **`unsafe` in `api_versions.rs`.** `ash` and `khronos-egl` calls are `unsafe` by design, and
    ADR-001 allows `unsafe` in three files only. Recommendation: add
    `crates/tl-gpu-probe/src/api_versions.rs` to the ADR-001 list (file-level allow, `// SAFETY:`
    per block). E13-006 is blocked until the leader decides; the alternative is to drop the mode
    and show `Unknown` for Vulkan and OpenGL.
14. **Health state for "GPU probe not installed" and NVML init failures.** `SourceState` has no
    `Unsupported` or `Missing` value (ADR-007 says "health `Unsupported`"; PERF-062 says `Not
    supported`). Recommendation (E13-010): `Failed` with `last_error` = `"GPU probe not
    installed"`, `"LibraryNotFound"` or `"DriverNotLoaded"`, and the UI (E24, E20 chip) maps those
    texts to the "Not supported" presentation. Alternative: a `SourceState::Unsupported` variant
    (E03-011 edit).
15. **Unit job deadline.** ADR-006's table says 30 s until `JobRemoved`; ADR-005 and SYS-005 say
    service jobs have no client-side timeout. Recommendation (E11-002, E11-011): ADR-005 wins; the
    method call itself has 10 s (user) or 120 s (system, prompt); a bus loss while waiting is
    `OutcomeUnknown`. One row of ADR-006 to update.
16. **Dump path confinement and the CI build of the fake probe.** (a) E11-018 accepts a dump path
    only directly under `<state_dir>/dumps/` with a `*.core` name; the UI already builds such
    paths (UI-PWN-034). (b) E13-012 needs `cargo build -p tl-gpu-probe --features fake` before the
    engine tests; `scripts/ci.sh` is E02's. Recommendation: add that line to E02-A2.
17. **`AccessDenied` mapping until E29-OI-3 lands.** systemd D-Bus denials map to
    `Failed(PermissionDenied)` (UI text "not authorized") in E11-011; when E03 adds
    `ActionError::NotAuthorized` and `AuthCancelled` (E29-OI-3), E11-011, E11-018 and E11-020
    switch to them. No bead waits on it.
