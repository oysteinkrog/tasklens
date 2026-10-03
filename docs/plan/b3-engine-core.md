# B3 engine-core: bead plan for E08, E09, E10, E11, E13

Batch B3 of the bead-writing round (`docs/plan/00-epics.md` section 5). This file maps every plan
key to its bead ID, lists the shared design points the beads build on, and records the open items
for the leader. The epic and anchor beads were created by the leader (`docs/plan/anchors.md`); the
anchor descriptions were replaced with the full template by this batch.

Rules followed: `docs/research/brief-beads.md`. Every bead carries the build rule in its Notes.
Priority equals phase; the beads that E08-A1 and E09-A1 need are P0 because the phase-0 spike
(E01) depends on those anchors.

## Shared design points

These are fixed in E08 beads and used by every other engine epic. Items marked (open) are listed
again under "Open items" because the ADRs do not spell them out.

- `SourceUpdate` (E08-003, `tl-engine/src/update.rs`) carries every variant for every phase.
  Device samples are keyed by `DeviceIdentity`; the reducer owns `DeviceIdx`. (open item 3)
- `WorkerIo` has a fourth field `control: watch::Receiver<WorkerControl>` beside `updates`,
  `demand` and `settings`. `WorkerControl { reset_generation, commands: Arc<[QueuedCommand]> }`
  carries baseline resets after resume and the reducer-to-worker commands (`RefreshNow`, `KWin`,
  `DetailRefresh`, `HandleSearch`, `HistoryQuery` and the rest) as a latest-wins queue of the
  newest 64 entries, so no restartable worker owns an `mpsc::Receiver`. (open item 1)
- `EngineMsg` has a fourth variant `Tick`, sent every 1 s by the session for stale marking and
  linger expiry. (open item 4)
- `workers/mod.rs::all_workers()` is the worker registry. Each worker bead appends one
  `WorkerSpec { id, name, modes, factory }` line (chain edit on E08-A1).
- `Source` has two defaulted methods beyond ADR-004's three: `reset_baselines(&mut self)` and
  `command(&mut self, &WorkerCommand) -> bool`.

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
