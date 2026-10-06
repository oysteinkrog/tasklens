# Leader notes for the bead round

Working notes so a terminal restart does not lose them. Items here go into the final open-items
pass once all batch writers finish.

## Cross-batch links reported by writers

- E01 bench flag: `tasklens-crw.11` (`bench_show_both`, CPU page).
- E01 paint hook: `tasklens-yrf.7` and `tasklens-yrf.9` (`AnimationDriver.paint_log`, ChartElement
  paint step 11), exported by `tasklens-yrf.1`. The hooks sit behind the `test-hooks` feature, so
  `tasklens-wlf.3` must declare `bench = ["test-hooks"]` in tl-ui.

- E01 `first_frame_ms` log line: `tasklens-jqk.10` (E20-008).
- E31 tray chain-edit points: `tasklens-jqk.1` (`close_decision`, `reopen_main_window`),
  `tasklens-jqk.10` (`tray_available`), `tasklens-jqk.6` (`titlebar::minimize_main`).
- E33 needs `tl_ui::run(UiLaunch)`: `tasklens-fut.15` (E16-012, P0).

## Open items from B1 (infra) still to settle

1. E33-A1: tl-app needs a `tl_ui::run(UiLaunch)` entry point in E16 (asked of B5).
2. Missing allowed-dependency rows or pins: clap and async-channel (tl-app), rustc-hash
   (tl-model), bitflags `serde` feature, rustix `mm` feature, a PRNG and a time-zone crate (tl-ui).
3. `bench` feature lines: E02-005 writes both up front, so the E01 chain-edit bead for the tl-ui
   `bench` line should be dropped.
4. E31 tray: `TrayModel` and `TrayEvent` need a home in tl-model (`tray.rs`); tray start needs
   tl-app `main.rs` edits; close-to-tray needs edits to E20's window `main.rs` and `titlebar.rs`.
5. E32 low memory: no type or `SourceUpdate` variant carries the low-memory sweep data. R-062's
   PSI trigger uses a 1 s window, but unprivileged triggers need multiples of 2 s since Linux
   6.5. `--force-full-memory-mode` behaviour is unspecified.
6. tl-model types with derived fields (GroupRow, system totals, TrayDemand, FourTuple,
   RecoveryEdit, KWinCommand, helper reply) and the proposed ColumnId numbers 1 to 121 need
   review.
7. `write_atomic` comes in E07-A3 (P2) but the P1 settings store needs it.

## Leader jobs at the end

- One pass replacing the old `flock ... systemd-run ...` build line with `heavy-build` in every
  bead description.
- `br sync --flush-only` at the end (writers use `--no-auto-flush`). The database is
  FrankenSQLite (fsqlite), so never touch it with the sqlite3 tool. Its WAL is 640 MB and did
  not shrink with a lone write; a lone write still takes 1.7 s, so it is not urgent. If it
  matters later, rebuild from the flushed JSONL with `br doctor --repair` after a backup.
- Delete old `.beads/.br_history` copies (410 MB) once the round is done, after asking.
- Collect every batch's open items, settle them (new R entries), apply them to beads.
- Quality pass: `br dep cycles`, `br lint`, orphan and coverage checks.
- Astra review of the plan and beads after Codex returns (Sunday 4 October 11:12), then oracle
  review.

## Batch final reports

- B3 (E08 to E11, E13): done, 91 beads, no cycles, plan committed in 17c2c8e. 17 open items are in
  `docs/plan/b3-engine-core.md`, and the beads follow the recommendations. The build line is
  already replaced in 8hz, 8q0, dwy, i3o and ndy. E13-006 is blocked on open item 13 (`unsafe`
  in tl-gpu-probe).
- B4 (E12, E14, E29, E30): done, 123 beads, no cycles, plan in d456386 and e98c11f. Open items:
  E12 has 11 (see the plan file), E14 has 16, E29 19, E30 9. E12-OI-1 (contract additions, first
  `Source::take_extra()`) blocks 21 E12 beads, including the P1 anchor E12-A2; settle it first.
  E12-OI-4 (udev reader) needs a new B2 bead. Build line still old on E14 and on E29 after qfx.8.
- All batches: `br lint` flags only the epic beads (no Success Criteria section). Leader adds them.
- B7 (E20, E21, E23): done, 115 beads (E20 30, E21 47, E23 38), no cycles, plan in 56b421a.
  16 open items (E23 has 8 more in the plan). Cross-batch edges for the leader to add:
  qfx.32 on jqk.28; 8fx.3 on jqk.27; E22 on jqk.29 (`end_window_pick`); E26 card colour menu on
  E21-032; E25 and E28 Fetch buttons on E21-017; E29 privileges chain edit on E21-035; E21-043 on
  the E30, E31 and E32 settings-row beads; E21-029 to E21-031 on E17's share-code bead. 00-epics
  section 2 needs the E31 chain-edit rule for window/main.rs and window/titlebar.rs.
- B6 (E19, E24, E25, E26): done, 102 beads (E19 18, E24 27, E25 35, E26 22), no cycles, plan in
  2c96816. Open items E24-6 to E24-10, E25-OI-1 to 8, E26-OI-1 to 7 (plan file). Cross-batch
  edges for the leader: o55.22 on bj6.20; xdh.12 on bj6.35; E20's close bead gets the
  `any_detached_open` call (E26-OI-3). Build line still old on 40 E19 and E24 beads.
- Overlaps to settle together: device commands (SMART, eject, public IP) are E12-OI-1, E25-OI-1
  and the E12-A3 placeholder; X11 keep-above is B7 item 4 and E26-OI-5; `QuitMode::Explicit`
  (B7 item 5) is recorded in research/claude-gpui-apis.md:58 (gpui app.rs:202).
- B8 (E22, E27, E28): done, 115 beads (E22 47, E27 35, E28 33), all 227 planned edges in, no
  cycles, plan in 16a07bf. Open items: E22 19, E27 9, E28 15 (plan file). Leader edges:
  wlf.10 (E01-008) on pdu.1 (scroll handle); E18-A1 must expose `TlTreeTable::scroll_handle`
  (check B5). Order the `window/main.rs` chain edits of E27-023, E27-025 and the E28 anchors
  with edges. Public IP fetch API joins the device-command overlap (blocks 9o0.32).
- B2 (E04 to E07): done, 111 beads (E04 32, E05 36, E06 26, E07 17), all edges checked, no
  cycles, plan in 1ac80b1. Open items E04-OI-1 to 13, E05 1 to 13, E06 1 to 11, E07-O1 to O6
  (plan file). Leader work: build line still old on 36 beads; E04-017, E04-020, E04-025 need a
  `## Notes` heading; cr0.11 and cr0.17 depend on iuz.13 instead of E07-A3 (E07-O4); PERF-050
  Partitions table has no bead (E04-OI-13); PERF-037 moves to E06 in 00-epics section 6.
  Check in the quality pass that E10 to E13 beads depend on the B2 anchors they use.
- Overlaps: udev reader is E04-OI-2 and E12-OI-4; ProcessRecord extras are E05 1 and B3 item 5;
  rustix `param` and serde_json join B1 item 2 (dependency rows).
- B5 (E15 to E18): done, 70 beads (E15 25, E16 15, E17 14, E18 16), all 139 edges in, no
  cycles, plan in d69765a. Open items 1 to 38 in the plan file. Leader edge: E33-A1 on fut.15,
  and B1's E33-OI-1 proposal follows the `UiLaunch` shape in fut.15. Check that E18-A1 exposes
  `TlTreeTable::scroll_handle` (B8).

## Owner answers on 2026-10-04

- OI-U40 themes: the two default themes ship at P0 in E17-002 (lgw.4); E17-005 (lgw.7, P2) adds the ten palettes
  when the owner exports `assets/themes/builtin.json`.
- OI-U41 share codes: TL1 import now; TS1 import when the owner supplies a sample code (U-17).
- OI-U42 app icon: placeholder now (256 px rounded square, accent colour, white monospace "tl");
  new P3 bead E15-026 swaps in final artwork.
- Build-line rewrite: done on 405 beads, 0 failures.
- OI-E14 `--force-full-memory-mode`: with `--low-memory` it starts the full engine and keeps the
  rescue window; alone it does nothing. Bead it.
- OI-E21 `--app-id`: isolates everything (config, state, cache, runtime dirs, Wayland app_id,
  bus name). PKGBUILD licence: MIT. TaskSlinger INI import: plain booleans and in-range integers
  only until the owner supplies `UserSettings.ini`.
- OI-E30 polkit texts: the three E29-021 texts as written.
- OI-E31 history chart: stacked top 3 plus "other" per bucket, in the metric colour.

## R numbering for the settled open items

OI-E01 to OI-E32 become R-139 to R-170 (R = 138 + n). OI-U01 to OI-U65 become R-171 to R-235
(R = 170 + n). The owner answers above become D-012 to D-020 in DECISIONS.md.
- v9m.1 (E02-A1): the asupersync feature check must use `cargo tree -p tl-engine -e normal,features -i asupersync` (dev-deps include test-internals). Tell the v9m.1 implementer.
- R-267 (leader, 2026-10-04): fixture capture writes the fixed UUID
  `00000000-0000-4000-8000-000000000001` to `proc/sys/kernel/random/boot_id`, and
  `00000000000040008000000000000001` to a captured machine-id file. Found by the 2dl.10 privacy
  gate. Record in RECONCILIATION and ADR-013 in the next docs pass.
- Review later: 8q0.4 keeps the fast ring at its old capacity when a source's period changes (bead says keep the ring). Check the visible time span after a rate change in the e2e pass.
- 8fx.8: replay perf budget (5 ms per block, 2,000 processes) not checked in release mode; check in the e2e pass.
- 2dl.32: `impl Default for FanControl` lives in sample/sensors.rs as a placeholder; consider moving to a derive in parse/hwmon.rs.
- 2dl.21: `impl Default for ZramReading` written by hand because counters::Rate has no Default.
- cpufreq.rs:310 failed `cargo fmt --check` around 20:22; 2dl.16 now owns the file and must leave it formatted. Check in the full-workspace pass.
- 2dl.30: netif::read_oper_state builds a PathBuf per interface per tick (one alloc per iface per sample). Make it take a reusable buffer or dirfd in the perf pass.
- 8fx.11: a `deleted-keys.bin` that fails to decode loads as empty and the next delete overwrites it, so earlier user deletions come back. Review: keep the bad file aside, or refuse to show those boots, rather than silently forget deletions.
- 8fx.11: history of other boots is opened (counts in disk_bytes and Stale) but never returned by span()/blocks(). Check the history spec for whether past boots are ever shown.
- hx1.25: amdgpu gpu_metrics has no vcn_activity in any supported struct; video engine falls back to fdinfo. `average_mm_activity` not mapped (needs a decision). v2 structs give no edge/hotspot/mem temperature.
- lgw.5: no test for the hairline at scale 1.1.
- hx1.6: SockDiag recv blocks with no timeout. Consider SO_RCVTIMEO (e.g. 2 s) so a stuck dump cannot hold a blocking-pool worker (Astra risk 2).
- yms.16: desktop index snapshot canonicalizes /opt/google/chrome/chrome against the host filesystem, so the snapshot depends on the host. Make the canonicalize root injectable or skip canonicalize in the fixture test.
- ndy.3: tl-gpu-probe opens fd 3 with pidfd_open(getpid()) + pidfd_getfd, because /proc/self/fd/3 fails with ENXIO for a socket. Needs Linux 5.6+. Check the engine spawn side (tl-engine/src/gpu/spawn.rs) agrees. ndy.4 must remove the dead_code allow in sampler.rs.
- ndy.3: a failed Hello leaves the probe locked; a second Hello exits 0 with no answer.
- 2dl.36: /var/tmp/claude/minimal-vm/ (745 MB) is left for the owner to delete.
- iuz.5: sys.rs reads `environ` directly; its SAFETY comment assumes tasklens never calls setenv after startup. Make that a written rule (ARCHITECTURE or AGENTS.md) in the review pass, and grep for set_var. sys.rs has a file-wide allow(dead_code) to drop when E07-003/005/014 land.
- lgw.1 / fut.5: no tracing test subscriber, so "a warning is logged" criteria are untested. Consider a small test subscriber helper in tl-ui testing.
- 8hz.7: `budget_exhausted` is inferred (outcome Ok + restart_batches >= 5 + last generation errored), because asupersync gives no direct signal with EscalationPolicy::Stop. Review in the e2e pass.
- 8hz.7: cancelling the task that awaits `run_supervised` does NOT stop the supervisor (join ignores cancellation); shutdown must cancel the region or call handle.abort(). The engine shutdown bead must do this; add a lab test there.
- iuz.14: script_set_enabled does lstat then fchmodat (TOCTOU symlink swap, same-user folder). Kernel 6.6+ has fchmodat2 with AT_SYMLINK_NOFOLLOW; or open O_NOFOLLOW|O_NONBLOCK and fchmod the fd. Fix in the security review.
- 2dl.34 vs 2dl.24: partitions.rs stores udev and dm/name text unsanitized (bead: "sanitized by the UI"), while block.rs sanitizes dm names in tl-collect. Pick one rule (sanitize in tl-collect with sanitize_text, as E05-002 does for process text) and apply it to every collector text field in the review pass. 2dl.34's proptest covers type_name and gpt_flags only, not mutated udev/sysfs files.
- 8fx.13: rolling window worst case (2,000 processes changing each second) passes under 16 MiB but the peak was not recorded; measure the margin in the perf pass.
- ndy.5: real_sample_under_100ms hit 99.99 ms once under heavy machine load (quiet machine: 43 to 50 ms). It is #[ignore]; if it fails on the owner machine, read RX and TX concurrently.
- fut.6: REFRESH_WHILE_HIDDEN path calls cx.refresh_windows() (no main window handle); whoever settles U-3 should pass a window handle.
- 8hz.8: bead timeline was wrong (budget runs out at 15.5 s, not 7.5 s); tests assert the real 8hz.7 timeline.
- qfx.10: resolve_socket_inodes has cfg_attr(not(test), expect(dead_code)); E29-A1 must remove it when wiring dispatch.
- ndy.8: crates/tl-gpu-probe/src/api_versions.rs has #![allow(unsafe_code)] (ash and EGL FFI) with SAFETY comments. Second place with unsafe besides tl-ops/src/sys.rs; check against the ADR unsafe rule and review each block in the security pass.
- 8hz.10: on a rate change the source loop takes a reset sample at the same instant as the old-rate sample (log 2000, reset, 2000). Check the samplers handle dt = 0 (no divide by zero, WarmingUp), and consider skipping the immediate sample when one was taken this instant. Also: after switching to Off, one already-scheduled sample still runs.

- iuz.9 thread ops: if a thread exits between the task-dir check and the nice, sched or affinity call, a reused TID in another process can get the change. Signals are safe (tgkill). Review whether a pidfd-based thread check or a post-call recheck is worth it.
- wox.5 tray raster: heights round to whole target pixels and then double in the 2x buffer, so the box filter never changes a value. The supersampling does nothing visible. Decide if fractional heights are wanted.
- yms.9 cgroup: a `\xNN` escape that is not valid UTF-8 becomes U+FFFD in the unit name.
- 8fx.1 store_6h writes ~65 MB to target/tmp/store_6h and never removes it; two parallel runs clash. Consider a tempdir.
- Store::will_rotate is true before the first append. Decide whether to fix in tl-history instead of guarding in callers.
- iuz.1 real_other_user_process_permission_denied skips when root or CAP_SYS_NICE (so CI as root never runs it).
- iuz.2 real_cgroup_session_scope_refused ran outside app.slice, so it hit NotOwner; the Protected path (own scope inside app.slice) was never exercised. Needs a test that runs from a real app scope.
- iuz.3 real_write_atomic_in_xdg_config writes into the real ~/.config when XDG_CONFIG_HOME is unset. Check it only creates and removes its own uniquely named file. real_dump_as_root_writes_core has never run as root.
- qfx.5 helper ops: tl-ops InvalidInput, NotOwner, Protected and Unsupported all map to NotAllowed; OutcomeUnknown maps to Os(3). SetIoPriority carries a ProcessKey, so the thread-target rule is only reachable through run_op. read_boot_id() is not wired yet (E29-A1).
- ndy.9 gpu tests use a TempDir under std::env::temp_dir() (/tmp, tmpfs) because tl-engine has no tempfile dev-dependency. Tiny files, but move to tempfile or CARGO_TARGET_TMPDIR.
- 8q0.16 lab_table.rs: committed #[path] include of table.rs plus an update shim; switch to tl_engine::reducer::table once public. table.rs mod tests duplicate tree and grouped tests. hide_system and totals tests live in table.rs mod tests, not under the names the beads list. The 2000-row timing test runs under the optimized test profile, not true debug.
- qfx.12 helper drop-in writes through /proc/self/fd/<dirfd>/50-tasklens-recovery.conf because write_atomic takes only a path; relies on /proc and on write_atomic's temp+rename staying in that dir. Review in the security pass; a dirfd variant of write_atomic would be cleaner. Empty OnFailure= value is refused.
- 8q0.15 totals: network header counts Wired, Wireless and Other only (not VPN or Virtual), so it can differ from the Performance NET aggregate, which follows show_network_*. Check against UI-PRC header spec. totals() not yet called from build().
- yms.14 snap ids: parser strips a full uuid with dashes; snap.<a>.<b>-<uuid>.scope gives <a>_<b>. Classify rule 6 still inactive: parse_app_unit exists, but owns_window is false in P1; wire when the KWin window bead fills owns_window.
- qfx.16 helper kill(): SIGKILL to the root helper gets EPERM, so kill() shuts the socket down and waits; a helper stuck in an operation blocks wait until it next reads. Confirm in E29-A2. Engine side does not enforce the 4 MiB multi-frame total (helper replies TooLarge).
- ndy.10 uses per-item #[allow(unsafe_code)] on two fns, stricter than ADR-001 rule 4's file-level allow. Fine; update ADR text in the doc pass.
- 8q0.3: Snapshot::empty doc says windows starts WarmingUp; Reducer::new sets Unsupported per ADR-003. Fix the doc comment in tl-model.
- hjn.4: no cfg(test) counter (integration tests cannot see it); fast-tick freq reuse proven through file change instead.
- yms.14 rule 6 for app-...-<rand>.scope uses the whole remainder as app id (gnome-org.gnome.Nautilus); E05-A2's desktop index must pick between it and the split form. Close reason predates ea30b8f and 4d054a0.
- yms.28 ThreadReader keeps per-key baselines until that key reads Gone or Restricted; a process whose details are closed and never read again leaves its entry. Check for growth when Details windows close (drop key on close).
- qfx.7 helper ReadProcessFiles: empty set gives one Err(NotAllowed); other open_verified errors give Err(Os(errno)) per file; unknown ProcFiles bits ignored. Review in security pass.
- hjn.5: leader approved net_key(i, mac: Option<&str>) and net_list(t_ns, ifaces, sys_root) reading class/net/<name>/address.
- yms.27 read_smaps has no size cap (buffer grows 64 KiB steps). A process with very many mappings could cost tens of MB per read. Decide a cap (the helper uses 4 MiB) in review.
- yms.19 qualifier marks any process with a .exe argv token as Wine when no Flatpak or Snap unit; a native tool given a .exe argument gets Wine. Tighten by requiring a Wine loader exe (wine, wine64, wine64-preloader, wineserver children).
- 8hz.11 dbus: zbus p2p dev feature for tests; DbusConn Drop aborts the tick task; error_name keeps a full fdo variant name list (update when zbus changes).
- ROOT-RUN LIST for the validation phase (needs Oystein or sudo): tl-ops real_dump_as_root_writes_core (iuz.3); tl-helper dump_child_writes_elf and dump_restores_coredump_filter with a real gdb run (qfx.9, ptrace_scope is 1 here); iuz.2 Protected path from inside app.slice.
- FLAKY, INVESTIGATE: gpu::spawn::tests::no_leaked_fds failed once during the 8q0.9 full run. spawn returns after exec, so an extra fd in the probe means some fd in the test process was open without CLOEXEC while another test ran (candidates: socketpairs in helper/session.rs tests, dbus p2p tests). Make the assert print readlink of each extra fd, then fix the non-CLOEXEC open. Matters for the real engine: the helper socket must never leak into the probe.
- 8q0.9: apply returns Result<(), InvariantViolation>; series period rounds the sample gap to 100 ms or 1 s; first CPU sample assumes 100 ms (602-sample ring); stand-in device names until the DeviceList; Failed/Unsupported sources set header validity only for single-source components (system header untouched); Sleep{entering:false} does the same as ResumeDetected; GpuInfo caps not stored (no model field).
- yms.20 protected set: empty WAYLAND_DISPLAY falls back to wayland-0; ppid walk capped at 64; plasmashell walk start only for self_uid.
- yms.29 fd reader stats /proc/<pid>/fd/<n> following the link; stat on a file on a hung NFS or FUSE mount can block the Details read. Check whether the per-fd stat is needed (fdinfo has ino and flags) or should run with a deadline.
- Reducer Tick reads real CLOCK_BOOTTIME (now_boot_ns), so lab tests cannot drive stale or linger timing with virtual time; 8q0.23 backdates samples instead. Consider passing time in the Tick message or through a clock trait.
- 8q0.23 fix 0282f38: health last_ok_ns updates every sample without republishing HEALTH, so the published value lags. Decide if the UI shows last_ok_ns (then publish on change at most 1 Hz).
- yms.2 resolver: icon cache saved at first end_sweep then every 60 s, no shutdown save (needs an engine shutdown hook). Undecided: owns_window with neither scope id in the index keeps the whole remainder as group id; settle when the KWin window bead lands.
- hjn.6: worker factories build sources inside cx.spawn_blocking because constructors read sysfs; cpu.rs has no spec or factory yet.
- hjn.7 disk replug: sampler drops counters of absent disks, so a replugged disk starts with empty flags, not GAP_BEFORE. Accepted: the reducer writes gap samples while present == false (8q0.9 point 5), so the chart still shows the gap. DiskSource::disk_ids() added for tests.
- MANUAL DESKTOP CHECKS for the validation phase (owner present): y6k.24 real_open_uri_portal (opens a browser); portal OpenFile with a directory fd (does it open the folder?).
- y6k.24 opener: FileManager1 call failure gives Internal(name) with no xdg-open fallback; consider falling back.
- 8q0.21: Snapshot::empty sets windows WarmingUp, so both Reducer::new and bridge empty_snapshot() override it to Unsupported (ADR-003). Fix Snapshot::empty in tl-model and drop the overrides. read_boot_id now exists twice (tl-helper ops.rs and tl-engine bridge.rs); move one into tl-collect sysroot or tl-model.
- 8q0.19 chaos: a lab chaos cancel ends a Transient worker generation for good (supervisor treats cancel as a clean stop) and twice cancelled the root. Fine while only shutdown cancels in production; confirm no production path cancels a worker except shutdown or demand. explore_supervisor_budget runs without cancel chaos.
- yms.1 sampler: a process whose status cannot be read gets ruid 0 and shows as root with OTHER_USER; should be unknown owner instead. SCHED_DEADLINE maps to SchedPolicy::Other (no model variant). tracing-subscriber added as a tl-collect dev-dependency (check-deps ok).
- MANUAL DESKTOP CHECKS add: real_nvml real_process_rows_u12 from a desktop terminal (needs DISPLAY for glxgears) to verify U-12.
- lab_session.rs explore_supervisor_budget has a DBG eprintln and, per 8q0.1, no assertion; fix in the test cleanup pass (8q0.19 said it asserts start times; check).
- 8q0.1: after dropping the engine handle, 2 extra threads (blocking-1, async-io) remain, process-wide from zbus/async-io via the logind worker. E09-A2's hard thread assert must allow for them.
- ndy.14: one early run saw an extra fd in the fake probe before Hello; unconfirmed. Ties in with the fd-leak investigation.
- yms.26 sweep gate (U-10): 35 ms on the 2,000-pid fixture against 15 ms; live /proc 34 us per process (992 procs 33.9 ms, near the 3.5% Processes-tab budget). Syscalls are 80%. Measuring fewer-files options before deciding.
- Socket map optimisation (R-287): skip pids whose /proc/<pid>/fd st_size is unchanged since the last rebuild; full rebuild every 10 s.
- 8q0.26 subtree totals: tree mode only; grouped-mode process rows with children (depth 2+) get none, check against UI-PRC spec. Handles not summed (GroupRow has no handles field). Bead AC had a wrong number (B subtree CPU is 5, not 3).
- 8q0.2 added #[doc(hidden)] pub Engine::start_with_workers(config, hooks, workers); start() calls it with workers_for(mode). Solves the fake-worker gap from 8q0.21/8q0.22/8q0.1; later tests should use it.
- 8q0.8: reducer keeps last GPU and PSS values per key across sweeps (sweep sends 0 WarmingUp). ProcessState::gpu_process() instead of a Coverage field. Wiring into apply.rs is bead tasklens-eal2 (after 8q0.10).
- hx1.9: connections address cache key includes foreign_unbound. hx1.13: duplicate Type= takes the last value. hx1.21: populated-slot-free MEMORY_DEVICE file gives Modules([]), not NotReported.
- dwy.1: health stays `Running` with `last_ok_ns: None` while a source keeps failing to build. Check supervisor health reporting (relates to 8q0.23).
- dwy.1: want_io uses `d.recording || s.history.enabled`; R-286 says "history recording is on". Both count; harmless, confirm in review.
- owner-desktop fixture has no sys/fs/cgroup, so ProcessesSource::new fails on it; tests create an empty cgroup dir.
- 8q0.10: while sleeping, every series sample pushed becomes a gap (memory and temperature too), not only rates. ADR-008 says "pauses publication of rates". Review.
- i3o.3: a key that exited within the linger has no flags, so only the pid 1 check applies; thread targets get their process checks.
- y6k.4: journal fetch stdout has no cap (stderr capped 64 KiB). Perf/security pass: confirm the query is bounded (-n) or add a cap.
- y6k.4: bridge.rs rt_handle comment "held only to keep the runtime alive" is out of date; spawn/IO errors map to Exit{code:-1}.
- eal2: Demand reaches the reducer through `Reducer.demand_rx` (mirror of settings_rx), wired in session.rs. Hidden PSS column clears all PSS. Candidate R entry.
- 8fx.15: history.rs has #![allow(dead_code)]; E30-013 and E30-A2 must remove it.
- y6k.5: real system unit listing takes 3.4 s with one GetAll at a time. Perf pass: batch or pipeline. CommandProgress setters are private, so the systemd worker does not report progress.
- hx1.1: sockets-basic fixture uses public address 140.82.112.3 (bead names it, against its own privacy note). Polish: switch to 203.0.113.x and refresh the snapshot.
- ug9.16: reduced motion draws a plain div at opacity 0.7 instead of kit Skeleton; bar width test checks the helper, not rendered bounds. UI validation item.
- i3o.4: dispatch has no process arm until i3o.5; targetless kinds return Rejected(Unsupported) until E11-016; unit kind with no Unit target gets the system-manager deadline. HelperCallError is ready for qfx.2.
- ug9.23/ug9.22: no bind_keys_components exists. slider.rs (and radio) expose bind_keys(cx); nothing calls them. Need a bead: app keymap calls every component bind_keys. Slider uses kit base parts, not component::Slider (spec thumb). set_from_elsewhere rebuilds kit state, resets drag flag. Chose 8 px gaps and 16 px hit height.
- fix-fdleak (decision pending on resume): zbus 5.19.0 recvmsg has no MSG_CMSG_CLOEXEC (connection/socket/unix.rs:284), so fds received over zbus are briefly inheritable; probe spawn can inherit them (no_leaked_fds flake with /usr fd). Options: (a) patch zbus to pass RecvFlags::CMSG_CLOEXEC (upstream PR + [patch]); (b) close_range(4, MAX) in spawn.rs child_setup (needs libc or raw syscall); (c) serialize tests only. Agent recommends a+b. Fixed so far: 4882181, 5146695, c21eeeb, b0d4e0a.
- ug9.22: bind_keys_components now exists in checkbox.rs and calls radio::bind_keys_radio; slider::bind_keys is not in it yet, and the app keymap must call bind_keys_components. Reduced motion reads only cx.reduce_motion(); motion.animations setting not wired.
- ug9.23: slider::bind_keys now called from bind_keys_components (a1cb21f). Remaining gap: the app keymap must call bind_keys_components.
- y6k.13: journal follow has no byte cap on initial fetch / Load older stdout or on one unfinished line (docs set no number; decide in perf/security pass). set_follow(false) can block while the UI does not drain the 64-slot channel (send_blocking).
- ug9.13: switch built from kit base parts (styled Switch cannot meet spec). M_FAST 120 ms is a local const until a motion tokens module exists. motion.animations=false only works if the shell maps it to set_reduce_motion: needs a bead (affects switch, radio, skeleton).
- DONE: ndy.2 clippy rerun clean; was: rerun clippy -p tl-engine --all-targets passes (blocked by 8fx.16 history.rs). Commits 13af3d6 ff6fee3 a967de5 a5b4b89 b745e7f 7e9713f.
- LEADER TODO: check-deps matches "race!" as plain text, so tracing::trace!( is a false positive (y6k.13 worked around it in 4f6749d with tracing::event!). Fix the matcher with a word boundary, then revert that workaround.
- DONE: closed tasklens-hjn.10 (cf304f6) after clippy passes; its live-stats slow-tick test checks behaviour since no E12-001 call counter exists.
- eal2 closed (efcf924, de17346): demand_rx in Reducer, keep set = detail_keys while pss/uss shown. Watch: processes.rs overlay() adds Valid-Pss sweep records to coverage.
- ug9.12: checkbox hover on checked box lightens fill+border 8% (not text_primary); min radius 3*ui_scale even with rounded_corners off. bind_keys_components lives in checkbox.rs.
- ug9.9: progress built on kit base parts; TlSpinner has reduced_motion(bool) builder. Doc conflict: spinner sizes 14/16 (bead) vs 16/20 (foundation.md). Fix docs.
- ug9.4: fonts from NotoSans-v2.015 and NotoSansMono-v2.014; pre-commit hook stripped a trailing space on LICENSE-OFL.txt line 21 (restore with \x20 trick, as with MAPS_GROUPING). Empty commit 77c2b56.
- Motion: no shared motion tokens; M_FAST 120 ms is copied into checkbox, switch and radio. Needs a bead with the motion.animations -> set_reduce_motion wiring.
- MANUAL DESKTOP CHECK (y6k.2): on Plasma 6.7.5, a new window appears in Snapshot.windows within 1 s (loads a script into live KWin; owner runs it). KWin Scripting names unverified: org.kde.kwin.Scripting /Scripting loadScript(s,s)->i, unloadScript(s)->b, start().
- y6k.2: captions map \n \r NUL to U+FFFD before sanitize_text; windows stable-sorted by pid (tl-model doc "Sorted by (pid, internal_id)" now out of date); KWin worker Full mode only (LowMemory KeepAbove belongs with E14-018); KWin owner gone -> Unsupported snapshot but health stays Running.
- yms.38: extras_p2 read() takes PidDir, &RawMeta and Option<&PidStatus> (status up to 10 sweeps old). Sandbox falls back to PROC_USER_INIT_INO 4026531837 when /proc/1/ns/user is unreadable (needs root). process-edge 4201 stat tty_nr changed 0 -> 34817. Engine still must copy extras_p2 into ProcessSweep (8hz.5) and run pkg_mut().run_batch() in its own job.
- 8fx.16: test access now via `#[doc(hidden)] pub mod history;` (57f9b6e, e23da55). MemTotal read from host /proc/meminfo (HistoryWorkerConfig has no sys_root). Mode keeps Rolling(RollingWindow) ~550 B with allow(large_enum_variant). history.enabled maps to Off until E30-014.
- yms.26: leader decision: meta refresh staggered by pid % 10 (first after 10 - pid%10 sweeps, then every 10; R-064 kept as max age). Add as R entry with R-286. Criterion 2000_steady was 22.9 ms before stagger; median gate 18.8-19.0 ms. processes.rs passes want_status_fields false / want_io true until dwy.4 (dwy.1 has since wired sweep_wants).
- yms.26 OPEN: stagger landed (2754968, 1922ca2). Criterion 2000_steady 20.33 ms [20.24, 20.43] at load 8-10 (was 22.9); gate test 1 of 3 passed at that load. Rerun bench and gate on an idle machine before deciding more work; about 17 ms of the sweep is stat+statm syscalls.
- i3o.5: tl-ops gains pub send_signal_fd(BorrowedFd, Signal); RealOps lives in actions/mod.rs next to ActionOps.
- y6k.18: ConnectionsUpdate gains attribution: FxHashMap<ProcessKey, NetRate> (chain edit). New-socket owners get Valid 0 after the first pass (R-162 beats bead text). Reducer use of Connections still needs a bead: check one exists.
- ug9.5: line height exactly 1.3 x size (16.9 px at 13 px; bead said 17). template_width allocates per call: never call per cell. UI validation item.
- i3o.11: CgroupInvalid in the bead maps to Rejected(NotOwner) or Rejected(IdentityChanged), own cgroup to Rejected(Protected), freeze timeout to OutcomeUnknown (tl-ops has no CgroupInvalid). Accepted.
- ug9.20: badge scales dot (6 px) and max width (120 px) by ui_scale too; dot-text gap sp2 4 px; dot colour for Neutral/Count is text_primary (spec silent).
- ndy.16: probe child uses close_range(4, MAX, CLOSE_RANGE_CLOEXEC) so std exec-error pipe survives (R-290). Not proven that the new test fails without the fix: check during review by reverting in a worktree.
- y6k.14: U-49 verified (follow resume, no gap, no duplicate). real_journal writes ~1000 lines to the user journal via transient unit tasklens-journal-test (ignored test).
- y6k.22: KeepAbove/SetFrameGeometry match pid + resourceClass io.github.oysteinkrog.Tasklens + caption (None = "tasklens", else single window). Accepted. Script-file delete race: follow-up bead. Pick/CancelPick dropped until E14-019.
- y6k.6: ServicesUpdate.units is Vec<UnitRecord> so publish clones each record (perf pass: Arc in update.rs). Reconnecting publishes Stale. 60 s resync LoadUnit roughly doubles GetAll calls.
- y6k.18: restricted pids left out of attribution (no ProcessKey); first pass and after gaps every owner gets WarmingUp 0; hidden tab sends empty records.
- 8fx.17: Recording ignores HistoryMsg::Delete (8fx.18 or E30-A2 must cover store delete); HistoryWorkerConfig.load_delay test seam; state in Arc<Mutex<State>>.
- ug9.7: name_runs takes &Font. Sanitizer gaps (ZWJ removed, U+2028/2029 kept): follow-up bead.
- ug9.6: UI-FND-052a example is wrong (expected string is 35 chars at max_px 30). Fix foundation.md example to max_px 35 or the 30 px result /home/oystein/.lo…/game.x86_64.
- ug9.10: hover tooltips use GPUI .tooltip (pointer placement, no close on key/deactivate); follow-up HoverTooltip bead. FocusTooltip for keyboard.
- lgw.14: render tests need --features render-tests,test-hooks; goldens 960x1520 and 1056x1672 checked by eye; R-291 scale decision.
- lban: restart reaches reducer as Failed then Running via source.rs runner (option b, leader decision); Late counts as running.
- i3o.12: own_cgroup_protected skips under heavy-build (builds.slice); passed when run inside an app.slice scope.
- Possible flaky: probe_integration spawn_fd3_env_and_fds failed once in a full run (lban).
- i3o.21: socket.rs has a private copy of the HelperCallError mapping that dump.rs (i3o.20) also needs. Polish: share one mapping fn in actions/mod.rs. Helper NotAllowed -> Failed(PermissionDenied).
- y6k.26: KWin cmd file kept until next command, next Update, or exit. Risk: if a Full and a LowMemory engine run at once in one session, stale cleanup and plugin names clash. Check in review whether that can happen (low-memory rescue).
- i3o.6: ActionOps::close_pidfd added (default closes); real kill_tree test waits 50 ms between STOP and KILL to observe T.
- i3o.20: privileged Dump never routed (qfx.17 comment). TooLarge maps to Internal(refused by the administrator helper). ROOT-RUN: actions::dump::tests::real_ops_dump_writes_core.
- hx1.3: inventory snapshot pins install_date; path_dirs under root are root/usr/bin and root/bin; connectors for every DRM card; cpuinfo cap 1 MiB.
- y6k.7: missing stat file (not denied) gives Unsupported; pid tick walks every unit with a cgroup. Slices and scopes lack ControlGroup: follow-up bead.
- m1f.8: tempfile added as tl-engine dev-dependency (check ARCHITECTURE and check-deps). One LowMemGroup per unit cgroup.
- wox.1: U-31 verified (ksni 0.3.6, 105 crates, no tokio). zbus dev-dep in tl-tray for the private-daemon test.
- ug9.11: U-41 answered: no kit Button size gives 24 or 28 px at rem 13 (16.25, 19.5, 26 px). TlButton is built on the base Button; .tooltip() takes text; 72 px min width for every labelled button incl. Segment.
- ug9.15: glyph() draws no tooltip; glyph_with_tooltip adds it; Failed with no last_error reads "Could not read: " with nothing after (polish).
- ug9.28: tooltips open only after a pointer move; follow-up bead migrates raw .tooltip call sites.
- y6k.15: startup worker lists no user units until E14-013 fills ScanInput.units; impact NotMeasured until E14-013.
- hx1.10: netns: if proc self ns net is unreadable nothing is filtered as host.
- lban closed: restart reporting lives in controller.rs (SourceController wraps the factory): Err or panic sends Failed, generation above 1 sends Running first. Closed and cancel send nothing.
- ug9.19: kv_grid::init(cx) must run at app startup (noted on jqk.4). TlButton draws icons at i_md only, so Copy section icon is 16 not 14 (polish: icon size option). Kit PopupMenu has no item height, so menu items are not 26 px. Ellipsis and full-text tooltip paths untested.
- i3o.7: no PidOutcome type; tl-ops returns Partial and OutcomeUnknown errors, engine maps them. PidOp Sched and IoPriority already forwarded in RealOps.
- ug9.17: Clear filter button has a fixed ID TlEmptyState-clear-filter (collides if two empty states share a window; polish: derive from owner id). TlButton actions go to the focused view, so the owner table must hold focus for ClearFilter: UI validation item.
- ug9.14: TlTabs on kit base Tabs (styled TabBar cannot meet spec). Overflow » tooltip text passed by owner via overflow_tooltip(); polish: add fnd-tabs-more to en-US.ftl. » is not a tab stop (arrow keys reach every tab).
- qfx.22: pkexec timeout path unverified: SIGKILL on the root script fails with EPERM; code closes the pipe and waits, so a script that hangs without writing blocks past 120 s. Security/robustness review item. A RunSetup queued during a run still runs after it (spec says ignore while running). LinkEnv gained setup_probe and setup_runner pub fields.

## 2026-10-06 hjn.15, ug9.29, y6k.17

- hjn.15 (disk worker): `DiskSource::with_statvfs` is `#[doc(hidden)] pub` so tests/ can reach it (same as lowmem.rs). Under a fixture root, statvfs reads Missing and swap paths match /dev/<name>. A missing /proc/swaps means Valid(false). Review: accepted.
- hjn.15 saw `lab_session::end_to_end_engine_in_lab` fail ("Flat mode" 51 rows vs 50) while workers/processes.rs had another agent's uncommitted edits. Recheck after the in-flight processes.rs bead lands. Flaky once: gpu::spawn::tests::child_env_is_minimal, workers::journal::tests::follow_every_line_once.
- ug9.29: `glyph_with_tooltip` now takes window and cx. `button.rs:539` `TlButton::tooltip(..)` is the builder, not raw GPUI, and is fine.
- y6k.17: leader chose `StartupEntry.state_code: Option<Arc<str>>` in tl-model ("active (running)", None when no unit). Closes B8-E28-12 for this field. 9o0.12 reads it.

## 2026-10-06 m1f.9, 8fx.18, ug9.21

- m1f.9: LowMemory arm in processes::factory runs LowMemSource. The compositor pid helper is still copied in lowmem.rs and processes.rs; share it in a later cleanup.
- 8fx.18: the 10-minute Frame on 2,000 processes took 75 ms against 50 ms (load 55). Release test ignored; follow-up bead for read::stack. Rolling window loses the device catalog after 10 minutes; P1 follow-up bead. Also: a delete while the store is loading or failed is not saved.
- ug9.21: TlBanner tooltip shows on every hover, not only when cut. Button ids "TlBanner-<label>" clash if two banners share a parent and a label.

## 2026-10-06 y6k.3, y6k.8, 8q0.25, i3o.8, i3o.9, qfx.34, y6k.19

- y6k.3 (launcher): accepted all ten choices. System-manager units and non-Command as_admin are Unsupported. Tab in a command ends as Unsupported because tl-ops shell_argv rejects it. pkexec is /usr/bin/pkexec. launcher::LaunchOutcome is a second enum; E11-016 turns the actions/mod.rs one into a re-export.
- y6k.8: details go out as a Vec in ServicesUpdate; the reducer copy is on 8q0.29 (comment added). None in UnitDetails means Unsupported. A demanded unit with no record waits for a listing (no LoadUnit).
- 8q0.25: Interrupts row goes n/a after 10 valid samples at exactly 0 irq; before the first sample it shows the note. GroupRow has no user field, so the UI shows "root" for pseudo rows itself. Check in the processes UI beads.
- 8q0.25 saw 2 journal follow lib tests fail (journal.rs:1438, :1465). Seen flaky before. Recheck in the full-suite pass.
- i3o.8: one ActionOps::apply_thread_op instead of three methods. i3o.9 lab_actions has a stand-in gate until i3o.1.
- qfx.34: shellcheck is not installed, so script_shellcheck_clean skips. Owner decision: install shellcheck (pacman) to make it real.
- y6k.19 blocked; leader chose reverse_lookup signature, ConnRecord.remote_host, and a resolver task in connections.rs beside source::run (bead comment).
- i3o.10: real_actions.rs kill-tree test wraps RealOps in StopSettles (waits for state T after SIGSTOP, then 5 ms), because the STOP and KILL passes are microseconds apart. Order is unchanged. thread_nice_only_that_tid is covered by i3o.8 in process.rs.
- y6k.17: StartupEntry.state_code landed (e5c2405).
- y6k.19: negative rDNS cache is 1 min (SYS-035), not the 2 min in my bead comment.
- 8fx.33: rolling window keeps dropped catalogs in RAM and injects the right one into the first returned block's decoded copy. Accepted over the bead's "mark due again" example (that would put the catalog in every block). Test feeds 12 min because the first drop is at 660 s. The new test was not run against the old code.
- ug9.1: TlTextInput wraps the kit Input (own border, own clear button with HoverTooltip, Esc handled in capture phase). "Clear filter" and "Filter:" are hard-coded, not i18n keys; sweep with the other component strings in the i18n pass.
- i3o.18: closed by leader; the openers line moves to i3o.1. launcher::LaunchOutcome is now `pub`. Targetless kinds skip the privileged arm (privileged only picks the 120 s deadline).
- qfx.25: built as written, but sensors-detect 3.6 never prints MODULE_<n>=. Leader chose: read HWMON_MODULES or the cut-here block, and make revert undo conf.d and the service. Follow-up bug bead. OWNER REVIEW: this decision changes E29-023 behaviour.
- y6k.27: -.slice walks every pid once per slice level. Follow-up bead: one walk per tick with ancestor attribution, skip "/", plus a DbusConn test constructor for a fake-bus test.
- hjn.13: added `build_static_in(.., dev: &Path)` so the virtualization test can fake /dev, and one `#[allow(unsafe_code)]` test for set_var (edition 2024) behind a static mutex. Live host: running_in_vm Valid(""), ppd_profile "balanced".
- ug9.24: leader approved chain edit to input.rs: align_right, suffix_text, square_corners builders on TlTextInput.
- y6k.19 a1f13c8 broke the build (private tl_ops::sys); agent told to fix.
- y6k.25: real launcher tests pass (3, ignored by default); nothing left behind. They create a user unit tasklens-test.service and refuse if it exists.
- 8q0.27: Reducer.unit_names never removes names (an empty listing while systemd is down would blank rows). Accepted.
- y6k.19: `tl_ops::sys` is now `pub mod`. Negative rDNS TTL 1 min.
- hjn.16: no udev file gives Unsupported interface/WWN/serial even for nvme*/vd*; a udev file without ID_BUS uses the name fallback. A failed udev read gives Restricted. Accepted.
- qfx.35: state in /var/lib/tasklens (lm_sensors.before, enabled marker); reads BUS_MODULES and HWMON_MODULES; revert disables but does not stop the service; /etc/modprobe.d/lm_sensors.conf not reverted. docs/spec/features-perf.md (superseded) still says MODULE_n. OWNER REVIEW with the qfx.25 decision.
- ug9.24: TlStepper closed; TlTextInput gained align_right, suffix_text, square_corners.
- y6k.28: one walk per tick; DbusConn::p2p is cfg(test) only (no test-hooks feature in tl-engine). The user manager's -.slice keeps all of that manager's pids (its ControlGroup is not "/"); accepted. An unreadable child cgroup is skipped with its subtree.
- 8fx.34: Frame budget test 12 to 17 ms at load 31 to 50 (budget 50 ms), un-ignored; accepted despite the AC's quiet-machine clause because the margin is wide. The single-sweep path assumes a process key appears once per sweep; one timing run exited 101 with the cause unseen. Recheck both in the full-suite pass.
- i3o.1: Gate installed; `impl Drop for Gate` aborts in-flight actions and run_action checks cancel before each target and event. Spawn failure still `expect`s (panics the reducer during shutdown); revisit. Old Harness::submit stand-in kept.
- 8q0.28: DetailsState in the reducer. No DetailsUpdate clears the handle search; `clear_handle_search()` has no caller yet. Decide with E10/dwy.3. Component header stays WarmingUp; tabs carry their own validity.
- Leader fixed lab_session end_to_end_engine_in_lab: Flat mode now has SWEEP_SIZE + 1 rows (Interrupts pseudo row).
- qfx.26: polkit-rule on/off. The script does not create /etc/polkit-1/rules.d (the polkit package ships it). Accepted.
- hjn.17: NetDetails has no PartialEq; net.rs compares fields itself. Adapter name via root.run_path, not a udev_data arg. Assertion in net_first_sample_and_device_list changed to Missing (approved, 720c64e).
- y6k.12: OpenRC backend in dbus_systemd.rs. sub_state is the raw OpenRC word, load_state and path empty, User "root". Closed while clippy was blocked by sys_table.rs:1041 println (y6k.1); recheck.
- 8q0.29: chain edit approved in reducer/processes.rs for net attribution.
- ug9.2: leader decision, TlStepper is one Tab stop; minus/plus get tab_stop(false) (spec silent; GTK SpinButton pattern).
- hjn.18: rtnetlink RTM_GETADDR dump (read-only). Demand gating goes through a Cell<bool> set in period(), because sample() does not see Demand; consider a Source hook later. IPv6 Host and Other scope addresses are dropped (so ::1 is not shown). Fixture roots get no addresses.
- ug9.2: TlSelect built on the kit base Select/Combobox with a tasklens Entity<TlSelectState> (set_selected_value, selected_value, set_items), not the styled kit Select the bead named, because the kit state hides highlight and items. About 35 beads use it. Accepted. select::bind_keys goes into bind_keys_components.
- 8q0.29: Inventory merges peak_committed_bytes and GpuApiVersions (Reducer.api_versions). GraphicsAdapter.device must be set by the Inventory worker (hjn.19 and later) for version matching. Startup counts cover all entries, not filtered (E14-005 job). Health Failed then update: the update's validity wins.
- 8q0.29: closed with net attribution (ProcessState.net, apply_net_rates, overlay on sweeps). NetRate.shared is not copied because ProcessRecord has no field for it; check against the UI spec for the shared-socket marker. Full tl-engine suite passed at 8b21ae7.
- 8q0.31: leader chose a separate ProcessSeriesSet.slow list (sorted, with get_slow) over the bead layout of fast then slow in one list, which broke the binary search in get(). on_sweep gains period_ns.
- hjn.19: Inventory worker landed. Non-Inventory-tab RefreshNow and other Inventory commands are marked done at once; E12-019 (FetchPublicIp) must change that. GraphicsAdapter.device not built here (no adapters in this bead); the later graphics bead must set it.
- 8q0.31: closed before the slow-list decision arrived; same agent asked to do the ProcessSeriesSet.slow follow-up. View ids are placeholder SeriesId(0,0). Gap rule: any non-Valid field (Stale too) writes a gap.
- y6k.1: Services projection in sys_table.rs (10k units 3.4 ms release). Leader decision: Active since is not matched by the text filter (reducer has no time zone, no I/O). OWNER REVIEW: if wanted, pass a UTC offset into EngineConfig or the query. Reconnecting manager keeps its last rows.
- 8q0.32: per-GPU process util was folded to a max; leader approved chain edits in processes.rs and apply.rs to keep per-GPU util and power and to reapply power on republish.
- fut.1: TasklensUi::init landed. Asset source cannot be set from init in gpui-pre 0.3.7; fut.15 (tl_ui::run) must call Application::with_assets(TlAssets). AppEnv::test() is behind test-hooks (the bead said test-support). themes_json only validated until E17-008.
- ug9.3: gallery with 40 goldens (about 12 MB in git). ThemeScope swaps ThemeStore.preview per half while drawing. Not captured: hover, open select popup, stepper invalid state. render-tests now enables bench in tl-ui Cargo.toml. Leader to view goldens in the UI validation phase. Consider git LFS or smaller goldens if the repo grows.
- y6k.9: build_startup in sys_table.rs; null publisher sorts last both ways. Follow-up bead for apply.rs startup counts (wait for 8q0.32 to leave apply.rs).
- rsf.3: Selection by identity. click/ctrl_click/right_click take no view, so the E18-007 wiring must never pass a group header id. select_all drops hidden rows.
- y6k.11: real systemd tests pass (user unit start and stop, 460 system units listed). Engine shutdown report printed, not failed.
- i3o.13: unit actions. ActionCtx gained unit_bus_override (one line in each ActionCtx literal across actions and tests). JobRemoved match filters on sender and path. Lost connection gives OutcomeUnknown.
- 8q0.33 done by the 8q0.31 agent; leader updated ARCHITECTURE.md. 8q0.32 resent full-wiring approval.
- Flaky tests made into beads: lab_processes shared fixture dir; probe_integration fd 5 and empty env.
- fut.15: tl_ui::run landed; renderer failure on Linux is an Err from open_window, so the main hook (jqk.10) must call request_exit(RendererFailed); comments on jqk.10 and cr0.15. X11 display panic is caught. Quit-mode test is weaker than the AC (test platform quit does nothing).
- Leader fixed a tl-ui build break from jqk.3 (a06b1fb, missing BorrowAppContext import).
- hjn.26: UPower worker with sysfs fallback. No Degraded state in SourceState, so the fallback is Running with last_error set. Only ServiceUnknown and NameHasNoOwner fall back; other bus errors restart the worker. BatteryHistory wake waits for E12-029.
- 8fx.35: lab_processes fixtures unique per run; 20 overlapping runs passed.
- wlf.5: `mod bench` in tl-ui lib.rs behind the bench feature.
- 8q0.32: power estimate wired. GPU term only when supports_process_util; share uses max(SM, enc, dec) to match the GPU column (PROC-034 says SM). GPU power keyed by UUID; AMD GPUs share a zero UUID, so two AMD GPUs would collide once AMD sends per-process rows.
- yrf.7: AnimationDriver registry per window (register, global panics if none, try_global). thread_cpu_ns is 0 until E03 adds tl_model::time::thread_cpu_ns.
- hjn.26: fake bus returns Closed on cancel (1d01e51).
- rsf.5: ColumnLayout over ColumnId only; follow-up rsf.18 makes it generic for SysColumnId (blocks hk6.1). min_width takes window and cx; set_width takes scaled px.
- yrf.3: chart types and ChartView skeleton; submodules are stubs for later yrf beads; three placeholder fields carry dead_code allows. crw.3 PerformancePage fields unused (in progress).
- hjn.20: Inventory System section (hostnamed, logind, env). Fixture root skips the bus, env and shell. run-with-timeout is a local copy of tl-collect run_version (private); share it in a cleanup.
- fut.13: TlMenu over kit PopupMenu misses several spec points (keys, sizes, scale). Follow-up bead rebuilds it on base parts with the same API. context_menu_layer must be rendered by the window root (comment on jqk.10).
- gma.31: run history store with in-memory fake store in tests.
- 8q0.34: Startup counts follow the filter.
- fut.9: ToastQueue done; kit stack look differs from UI-FND-020; follow-up bead draws the stack in tasklens code, adds TlButton::icon_size, moves kind words to i18n. hjn.21: leader chose GraphicsAdapter.pci_slot plus reducer matching in apply_inventory (R-166). Leader fixed clippy single_element_loop in ui_hub.rs.
- Leader fixed ui_hub::init_installs_globals after jqk.3 (persist moves into TlSettings).
- jqk.3: TlSettings sets cx.set_reduce_motion(true) when motion.animations is off and only clears what it set. 85 SettingKeys, 55 Bool.
- i3o.14: FreezeUnit AccessDenied gives Failed(NotAuthorized) per map_bus_error and ADR-015, not the bead's PermissionDenied. Accepted (ADR wins over bead text).
- fut.14: TlShield on the kit base Button (TlButton cannot show the app SVG or a 16 px spinner). "Waiting for authentication…" tooltip still unconfirmed by the owner. OWNER REVIEW.
- wlf.6: StatsSynth::sample_1hz takes the CpuFrame; samples use Sample::valid (empty flags mean a gap in tl-model).
- yrf.7 closed.
- y6k.10: Connections projection. User names come from the process snapshot (uid to meta.user), else the number. PID "?" restricted, "-" kernel or exited. Interface sorts by ifindex. Follow-up bead wires build_connections counts in apply.rs after hjn.21.
- yrf.4: geometry part 1; split_runs and decimate clear their out buffers.
- Leader allowed dead_code on ChartView.cache (read only by test hooks until E19-009).
- crw.3: NetClass is tl_model NetKind (VpnTunnel). No DeviceKind for aggregate:net yet (comment on crw.26). JsonSchema through gpui_kit::private::schemars.
- rsf.4: CellTextCache hit 8.4 ns, miss 90 ns per cell (release, loaded machine). Bound R: Eq + Hash (NodeId is not Copy). Path cache clears at 8192 entries.
- gma.12: approved aria_label and prefix_tooltip builders on TlTextInput.
- rsf.18: ColumnLayout generic (TableColumn trait). System tables pin no column first; docs silent. OWNER REVIEW if Services or Startup Name should stay first.
- 8fx.36: probe flakes were test races (fd 5 is the probe's own copy of fd 3; /proc environ empty right after spawn). Fixed by waiting for Hello. Found 438 tl-gpu-probe core dumps from fake crashes today; follow-up bug bead. OWNER: existing core dumps can be cleared with coredumpctl when convenient (not done by agents).
- hjn.21: sections built; interface and kind mappings copied from disk.rs (share later). Resent pci_slot and DiskDrive.name decision with reducer matching. hk6.4: services and sys_* page modules.
- jqk.4: 29 window Actions and bind_keys_global (calls bind_keys_components). Kit Input binds escape/enter/down; jqk.11 must give ShellFilter precedence (comment added). rsf.7 cells.rs debug_selector breaks non-test build; agent told.
- hjn.21: closed with GraphicsAdapter.pci_slot and map_gpu_devices. Gaps: NVML 8-digit domain never matches sysfs form; mapping only on Inventory arrival; no DiskDrive.name. Follow-up P1 bug bead.
- 8fx.37: fake probe crash sets PR_SET_DUMPABLE 0 (rustix, no unsafe); 200 of 200 passes, no new core dumps (438 old ones remain). OWNER: clear old tl-gpu-probe dumps with coredumpctl if wanted.
- gma.12: approval resent; ClearPropsFilter note on gma.1.
- hjn.33: battery history 24 h at 288 points (R-166 over PERF-078's week); comment on o55.33.
- fut.16: TlMenu rebuilt on base parts; build returns Entity<TlMenuPopup> (comment on bj6.1). No visible scrollbar for tall menus. Highlighted theme rows get a 1 px accent outline (leader accepts).
- hjn.23: UDisks2 worker read-only (GetManagedObjects only). U-25 answered: udisks2 reports NVMe SMART for nvme0n1 and nvme1n1, sda is ATA.
- i3o.15: unit file actions; fixed call_raw for zero-argument calls (Reload never worked before). Unmask-then-enable failure leaves the unit unmasked without reload.
- hjn.21: closed again with DiskDrive.name and map_inventory_devices; NVML form and late catalog left to 8q0.36.
- jqk.5: approved TlButton::icon_muted.
- wlf.7: BenchEngine. Up time goes through SystemSnapshot.boot_time_s; NET_RX equals NET_TX in the synth. A tl-bench-bridge thread outlives stop() until the real client sends or closes.
- fut.17: toast stack drawn by tasklens (ToastLayer root plugin; toast::init in TasklensUi::init before any window). TlButton::icon_size. Close tooltip still hard-coded "Close" (i18n sweep).
- gma.12: closed; PropsFilter::new takes the tab label. GPUI test window has no a11y tree, so aria names are only checked at the builder.
- hjn.24: SMART failure reason goes in health with Failed; new tl_model::units::bytes_si_1dp.
- 8q0.36: normalise_pci (strict hex parse) and remap on GpuInfo or new catalog entry. 8q0.35: Connections counts follow the filter; established and listening count TCP only (as build_connections).
- tl-ui clippy --all-targets -D warnings is clean as of fut.17 136058c and yrf.5 e52853e. ui_shell.rs:498 fmt diff is jqk.5's in-progress work.
- lgw.10: user themes store; BootSettings.persist_themes moved to ThemePersist Global. Non-object theme entries skipped with warn.
- jqk.9: focus rules and esc_outcome table.
- yrf.5: merged fills use non-zero rule (even-odd cancelled equal polygons). Stale tail decision moved to yrf.9 (comment).
- yrf.8: grid paints through a QuadSink trait; baseline is 1 physical px at ui_scale 1.1.
- m1f.10: low_memory window module.
- fut.12: TlSecondaryWindow with own title bar and pin. Title bar follows the appearance.decorations setting only; if the compositor forces server decorations while the setting says client, two title bars show. OWNER REVIEW / desktop check. Non-resizable Tool windows (About, Crash Report) do not maximize though the spec table says Tool windows maximize. has_modal_child(window, cx) for E20. Key binding move is a follow-up bead.
- 8fx.2: history queries never reached the worker (run_history does not read WorkerControl). Leader chose Reducer.history_cmd: Option<HistorySink> on the same slot as record_sink.
- rsf.7: cell painters with no per-cell div (custom CellText element); numbers right-aligned, clipped not ellipsized. API notes passed to rsf.6 and rsf.8. Dim text on selected rows uses text_selected_secondary (leader accepts).
- lgw.9: follow-system theme on SnapshotChanged scheme changes (2 light, else dark). No start-up sync when the portal stays at 0. tests/ui_performance.rs untracked WIP from crw.7.
- yrf.6: Y scale rules; private axis formatters in scale.rs drop a trailing .0 ("5 Mbps"). StickyTemp keeps its range with no samples; floor only moves down. compute does not allocate.

- yrf.6 (1fe5854, 1f7c800): chart/scale.rs compute covers nine YScale kinds; private formatters drop a trailing ".0"; empty temperature window keeps 35 to 45 C; byte-mode link speed label is speed/8.
- hjn.22 (9083bf2, 95b899a, 80c7325, ec84c48): public IP via ureq =3.4.0 on FetchPublicIp only, never at start; fixture sys_root never fetches. 95b899a alone does not build. OWNER REVIEW: ureq 3.4.2 exists; pin kept at the checked 3.4.0.
- gma.13 (ea3aa6a, 4f61232, 399a316): General tab. Deleted-exe colour moved to a kv_grid follow-up bead; command line block is a child view, not .cached(), because its height varies; copy text waits for E23-011.
- i3o.16 (e3e7c7d): startup enable and disable; suffix rule in private valid_for(source, id); lab tests wait for i3o.19.
- hjn.29 (dce5917, 2d73c38, eecdbc6, d6c4fd2): AMD gpu_drm worker. No AMD card gives Health Off, never an empty sample. Mixed AMD and NVIDIA hosts break in the reducer: follow-up bead under 8q0. OWNER REVIEW: this machine has only an NVIDIA card, so gpu_drm_real_host (ignored) cannot pass here.
- i3o.22 (e020ed0): recovery drop-ins. User config dir from actx.startup_paths.roots.systemd_user; engine creates <unit>.d at 0755; fails before writing if the manager is not connected. OWNER REVIEW: empty on_failure list writes "OnFailure=" on the user path while the helper refuses it on the system path.
- hjn.24 (1a2e7cc, 03ab90b, 574d510, c402536): SMART reads. Failure reason in SmartData.health (Failed); told to use units::bytes_si_1dp. udisks_smart_real not run (ROOT-RUN list is not needed, but owner should run it once).
- crw.7 (c756ec1, f1ace3d, 4a8e6fe, cf26ef1, d953704): Performance details grid. Added OpenAdditionalSetup to actions.rs. Unknown drawn as Stale. Column count, sp6 gap and hover underline moved to a kv_grid follow-up bead.
- ug9.30 (12f3a8c, 658c5dd): TlValue.tone (ValueTone maps to theme.status.*), Valid values only.
- jqk.5 (7ac36f0, 69c0d9d, 49100d7, 50d239d): toolbar. Accepted TlButton::icon_muted and icon_color. Toolbar slots stop mouse-down so the title bar does not start a move.
- rsf.6 (d1b31eb): TreeTableDelegate. Decision: associated types Column and Sort, CellTextCache generic over the column, ColumnResizeMode re-exports tl_model ColumnResize. New rsf bead blocks rsf.8, hk6.1, hk6.10, 9o0.6, 9o0.14.
- fut.10 (6147ca3, 358077b, 7b82e88, 35870b4): TlDialog. Accepted: extra takes a closure, footer on base Button, sizes scale with ui_scale. Dialog centres on the display (parent bounds not readable during open); Wayland places it as an xdg_dialog anyway.
- 8q0.37 (e91d743, 3be9e86, 09ec39b): GPU ownership by uuid ([0;16] is GpuDrm). gpu_power keyed by DeviceIdx. Known: an empty GPU update counts as GpuDrm, so an empty NVIDIA sample keeps old NVIDIA rows (device list removal still clears them). Processes::apply_gpu_processes clears all per-process GPU data; fine while gpu_drm sends processes: None.
- hjn.24 closed (adds e3aa926: uses units::bytes_si_1dp).
- 8fx.2 (b92bb92, dc9ddec, 0f7ae3e, eba16b8, 88843e9): history recorder runs in full mode; SessionStart sent after the worker's first poll; Reducer.history_cmd (option A). lab_session rig installs a no-op sink.
- hjn.31 (803be98): Intel i915 and xe in gpu_drm; xe engine classes mapped to i915 names; Intel mem_total 0 in static info (R-164), samples carry the fdinfo total.
- i3o.17 (1db91cd): Run now and Delete entry; set_unit_file_state takes reload; reload_after_change shared. Plasma script cwd from HOME.
- i3o.24 (fc8577c): four #[ignore] real user-manager tests; they write tasklens-test-* units in ~/.config/systemd/user and clean up. Verified empty afterwards.
- Dispatch note: only tl-ui has the test-hooks feature.
- yrf.9 (ab12b5e, d5b2d9e): ChartElement paint plan. Stale tail: right edge follows the clock while all series are stale. First timing (release, headless): about 17 us per draw for one 600-sample chart with cached geometry.
- crw.10 decisions: update_static takes &CpuStaticInfo; cpu_error from HealthSnapshot last_error; CpuDetails caches sensor indices (k10temp, zenpower, coretemp, cpu_thermal); BogoMIPS flag is a follow-up crw bead.
- i3o.19 (6da3bb9): lab_actions FakeUnitBus per manager, real Gate in the harness, explore_unit_jobs (16 seeds). Busy second request gives Started, per-target Rejected(Busy), then a Rejected(Busy) summary; the AC's "exactly one" was written against the summary. Accepted.
- fut.2 (4e6dc4a): window registry (open-or-focus, per-kind caps). Deregisters on App::on_window_closed (deferred) instead of a window_frame.rs call; covers detached graphs too. Accepted.
- ug9.31 (9536c8d): kv_grid columns(n), column_gap, hover underline. Sent back: underline in text_secondary (performance.md:564 outranks the bead) and none on link values.
- ug9.31 follow-up c9c1f62: underline text_secondary, tooltip values only (not links).
- 8q0.30 (ee151da, 616be0e): record_system batches samples per update into one on_system_sample; on_device_catalog on publish; Reducer::set_record_sink; SeriesStore::append returns the stored Sample. set_record_sink is not tied to history.enabled (the 8fx.2 worker already switches modes).
- rsf.19 (63dbfbd, 6891896): delegate associated types Column and Sort; CellTextCache<R, C>; ColumnResizeMode re-export; VecDelegate::new only for ColumnId, other columns use with_columns.
- i3o.2 (793b4fd): dispatch is one exhaustive (kind, target) match; table tests and three engine end-to-end action tests added.
- hjn.25 (ccf951a): disk eject plan and /proc blocker scan; repeat Eject ignored while one runs; scan failure gives Blocked with others_may_hold.
- pdu.18 decision: TlDialog gets second_action; footer Cancel, Terminate (danger), Select (primary default).
- crw.10 final shape: update_static(model, &CpuStaticInfo), no CpuDetails struct; crw.11 keeps the sensor Vec.
- hjn.27 (c00c1e5, 74eb97c, 6c05324, 0ebcbc1): NetworkManager worker. NameHasOwner check first so D-Bus activation never starts NM. No Degraded SourceState exists: missing NM reports Running plus last_error, as UPower does. Accepted.
- crw.8 decision: PerfShowMiniWindow declared in pages/performance/actions.rs now; crw.4 extends it.
- yrf.10 (fdeeae4, c3521d7, 428b435): ChartView hub binding; own three-way search_series to meet the comparison bound; no allocation counter (unsafe_code is denied), tests use Arc counts.
- gma.5 (82a7dd2, 1ba712d): process window specs; pwn_window_spec helper for keyed windows; Set Priority and Signal open at 120 and grow to content; Affinity 640 x 360. Run window scale gap is a new fut bead.
- rsf.8 decisions: R-190 name cell geometry (pad 20, indent 20, chevron at indent + 6, guides at 20 + 20k + 14); delegate name_column() defaulting to pinned_first, else first visible column.
- crw.8 decision: header_text takes &Snapshot and reads inventory and device_details.
- pdu.18 (f35e063, d61eac0): confirm dialogs; TlDialog second_action; footer Cancel, Terminate, Select; focus never on Terminate.
- crw.28 (06acd11): CpuTotals.freq_is_bogomips set in workers/cpu.rs; tl-ui verification waits on the rsf.8 build fix.
- hjn.32 (e40b76f): smartctl through pkexec. SECURITY decision: smartctl looked up only in /usr/sbin, /usr/bin, /sbin, /bin, never PATH (a user-writable PATH entry would run as root); pkexec by absolute path. Sent back for that fix plus a regression test.
- Disk: target/ reached 247 GB. fd4e3dc sets debug = "line-tables-only" in profile.dev (owner approved). Hold new dispatch at the next lull, then cargo clean --profile dev and tell manager-machine.
- i3o.23 (2b171b0, 1d8b50d, ba1736a): FakePrivilegedOps, harness_p3, end-to-end dump, socket destroy and recovery tests. ConnectionsDestroySupport(false) is sent on every EOPNOTSUPP (accepted; the reducer stores a bool). Privileged dumps bypass run_action until E29-014.
- hjn.28 (24bc575, f2391ce, ccfb23b, fbafc7e): nl80211 Wi-Fi signal and rates merged into dbus_nm. Real read test found wlan0 (not connected). Parser test vectors assume little-endian. Values cleared when Demand.nets turns off.
- hjn.32 security fix dff2f16 verified: fixed SMARTCTL_DIRS, pkexec by absolute path, child PATH fixed, regression test locate_ignores_path.
- fut.19 (12d8762): Run window size and min size scale with ui_scale (600 x 310 at 125%).
- rsf.8 decision: zebra on the flattened index (processes.md:214, :285).
- yrf.1 (9f3161e, a764cba, bd6d4bc): chart roll-up gaps closed (failed and restricted states, warming-up grid, value_now). Owner contract: embed charts with .cached(size_full) so render_count stays 1. bench feature = test-hooks + gpui-kit/profiler. perf-cpu p50 waits for wlf.9.
- 17:45 to 17:52: API 503s stopped rsf.8 and pdu.18 mid-work; both resumed by message.
- rsf.8 (c8cd65d, 1672e6e, 780323c, e22e48e): TlTreeTable body and header. c8cd65d alone does not compile. Chevron click moved to rsf.9. Icon PathBuf per frame: new rsf bead.
- pdu.18 final: 551f2f4 enter_result; Enter on open goes to Select.
- Disk clean 18:05: cargo clean --profile dev removed 258.6 GiB; target 6.3 GB, /home 343 GB free; manager-machine told.
- yrf.11 decision: network series are bytes per second (NetTotals contract), formatted through units::net_rate.
- qfx.23 decision: approved health setters, EngineHooks.helper, SharedHandles.helper; noted on qfx.17 to qfx.21.
- hjn.3 (1787759): P2 worker roll-up. D-Bus worker idle proofs stay as in-crate tests (Connector seams are private); accepted. Real run here: Inventory, Udisks, UPower, NetworkManager Running; GpuDrm Off. MANUAL CHECK for owner: run p2_real_host_once on an AMD machine to see GpuDrm Running.
- wlf.8 (ed5ab44): bench/stats.rs FrameProbe and report stats; process_cpu_pct and accessors for E01-007.
- yrf.14 (34f4465): stacked bands by period slot, overlaid merge, buffers in Scratch. Geometry key allocates per paint beyond 2 series: new P1 yrf bead after yrf.11. Partial-point flag noted on yrf.2.
- qfx.23 (99dddb6, fde67ea, 6d63ef0): helper task runs in each session; helper and setup commands routed; HealthState set_helper and set_setup; EngineHooks.helper (None in lab_session rig); SharedHandles.helper. One extra publish per session start for the helper's first state.
- rsf.9 (019e4e3): table actions and keymap (context "Table"), chevron 20 px hit area in tree_table.rs, group header double-click toggles once, PgUp/PgDn by visible rows minus 1. Known: chevron and row tooltip can show together.
- lgw.8 (4e1036b): 200 ms theme crossfade; R-209 requester id; set_anim_window must be called by MainWindow::new (noted on jqk.10).
- yrf.11 (586ee3d, 1fcbe3b, 4765021, 11831de): chart labels cached and reshaped only on change; legend hidden under the range label size rule; a chart with a 2-item legend paints 6 lines (spec says 5); accepted.
- yrf.19 (96b0ce6): ChartGeometryKey::sync compares in place; 120 unchanged paints of a 16-series chart do 0 key rebuilds and 0 geometry builds. tl-ui clippy all-targets clean at that point.
- crw.5 (1228ab9, d652182, 4405d12, f9c0e75, 0b6b6ba, f2dfc93, 9d29fd5): DeviceCard. Approved: CardTextInput takes &Snapshot, fan_label and fan_temp, lock offsets, PerfSelectDevice declared early. Notes on crw.4 and crw.6.
- lgw.2 (bfb6cca, 1728f01, ebd1eb1): theme preview module; the only real gap in E17-A2. Duplicate-name check on rename belongs to the editor (noted on bj6.34).
- qfx.17 (6536b4e, 97083ef, 9c72984, fa09a3a): privileged route through the helper. Accepted: summary uses actions::summarize (Partial), same as unprivileged; cancel or not-authorized ends the request with no second prompt; 120 s prompt deadline whenever the helper is Locked at call time. explore_helper_calls now uses run_action.
- rsf.10 (b8d95f4): scroll anchoring sets the base offset directly; keep-visible bottom case scrolls to ix + 1; demand hash is an order-free sum of FxHash. Accepted.
- yrf.12 (ad829bd, 566449a, 3a0338e, 58e9bef, 34c262b, 37e2f07): ChartGridElement, one element for n cells, request_measured_layout for first-frame height. Offline text and per-cell dotted gaps moved to a new yrf bead. Cell mark and tooltip are P2 work still to come.
- jqk.6 decision: TlIcon::Square added for the maximize caption.
- lgw.11 (420f444, 6ba8ccd, de739e8): TL1 share codes and import. Input capped at MAX_IMPORT_BYTES (1 MiB) and MAX_USER_THEMES; proptests over random strings and payloads; clipboard text never logged. Checked the bounds in share_code.rs.
- jqk.6 (2b36602, 0dfa547, c64a45b): title bar as a plain flex row (kit TitleBar always adds its own controls, U-44 note in RECONCILIATION). TlIcon::Square in 2b36602. TitleBarProbe test global records move, zoom and window-menu calls and overrides decorations.
- rsf.20 (ff2b0e2): IconSourceCache in TlTreeTable; 20 distinct icons over 60 frames build 20 sources; unused icons evicted one version later.
- yrf.18 (27649d4): monotone cubic curves in Scratch buffers. Performance settings do not bump style_version, so GeometryScalars gained curved; toggling rebuilds once. Grid cells stay straight.
- lgw.12 BLOCKED on owner: one TS1 code from TaskSlinger 0.9.2 under Wine plus the matching themes.json export (U-17). Same owner export as lgw.7.
- yrf.2 (a114d42): grid heading hitboxes only while the pointer is inside; hover never rebuilds geometry; owner reads hovered_tooltip (noted on crw.9). Partial flag left for yrf.16.
- lgw.13 (30915f8): TaskSlinger themes.json import from the documented format (22 slots, no accent); 1 MiB cap; unchanged built-ins skipped; proptests. Built from docs, not a real export: re-check against the owner's export when lgw.7 unblocks. import_taskslinger maps any import_themes error to Ok(0); fine while import_themes cannot return Err.
- qfx.20 (2f49b50, 1004246, 8d0c1ef): helper reads for restricted process cells, max 64 per second, visible rows only, outside the sweep. SECURITY gap: a helper exit between the Unlocked check and call() would prompt; new P1 qfx bead adds a no-start call for background reads. OWNER REVIEW: while restricted rows are visible the helper never reaches its 300 s idle exit (PROC-076 as written).
- jqk.7 (7cc4537, 83f0b56, e54821e): tab strip with accent outline rebuilt in prepaint only when bounds change; 4 subscriptions; tabs stop mouse-down.
- yrf.20 (a4e5257, 6cac597): chart-grid-offline Fluent key; per-cell gap spans drawn as quads (UI-PRF-001 says the dotted baseline is quads; UI-PRF-002 keeps 3 paths), accepted over the bead text.
- rsf.1 (6838a6e, f0911f9): tree table roll-up done: TreeTableEvent, right-press targets, context menu through TlMenu (deferred build), CopyRowsAsTable TSV, CellTextCache::stats. CopyFailed never emitted (gpui clipboard write has no error).
- qfx.36 (5be2bb2): HelperHandle::read never starts a session; state check and refusal inside the helper task (verified in helper/mod.rs read()). HelperReadError { NotRunning, Call } kept separate so existing exhaustive matches stay valid; accepted. Five regression tests in lab_helper.
- yrf.15 (571aeb2, 8690cc2): ranges over 60 s read the 1 Hz tier with fallback; ChangedRuns splits changed_series by tier and binary-searches each run (log n kept). Fallback keeps the full range width.
- yrf.13 (67b889d, 1028a83): chart and CPU grid goldens at 100% and 110% with the render harness and lavapipe. Leader viewed chart_gap_spike_100 and cpu_grid_16_100: gap, spike, labels, legend in cell 0 and the Offline cell are correct. The dotted gap baseline is too faint to confirm by eye at that scale.
- pdu.6 (514ba96, a0d5ef9): process column registry, 121 columns per R-191, labels as Fluent keys, OnceLock index lookup.
- crw.9 (170add9, cbf6b7e, 9e2bff2): CPU grid wrapper; specs only on topology change; clock tooltips edited in place without notify; heading HoverTooltip trigger per cell. English constants in performance pages collected into a new crw bead (tr tables per strings version). crw.11 told to rebuild the grid on style_version change.
- qfx.21 (6028214): socket owners for other users' sockets through HelperHandle::read, 4096 inodes per 2 s rebuild, wraps; owners appear one rebuild later. Accepted. workers::connections is now pub.
- pdu.5 (ff8e850): Processes page actions and captured targets; 256-row cap in CapturedTargets::new (noted on pdu.20); RestartUnit is no_json.
- pdu.7 decisions: clk_tck in CellCtx; app rows borrow user, command line and start time from the first member; desktop path and unit row fields are a new 8q0 bead; RowRef::Record carries &TableRow.
- crw.11 decisions: TlButton gains focus_handle(FocusHandle) and trailing_icon(TlIcon) (button.rs chain edit); PerfSetCpuGraphMode and PerfToggleCpuKernel declared early.
- yrf.16 (b5efbc0, 402ab9d, 88eebf1): chart hover readout; hitbox only when spec.hover; text reshaped only on index or version change; offset from the newest sample. Known: window deactivation clears the pin only on the next paint (no repaint is forced).
- pdu.11 (fe4fe77, 9f9f8c0): process query state, generations, expansion, resend when the command queue was full. GroupHeader::node mirrors tl-engine GroupClass::key (noted on pdu.12).
- crw.11 (e96173a, df61a3a): CPU page; charts cached; demand with window handle. Follow-up commits in flight: TlButton focus_handle and trailing_icon, grid rebuild on style_version, its strings through tr.
- rsf.11 (b32090f, 17e2aa2): two-line header; totals from CellTextCache keyed by header row; TableSort<C> trait bound on TreeTableDelegate::Sort; header click calls sort_by_column directly.
- pdu.7 (ea5e0b2, 5079cca, ae79d64): cell values. Sent back to index group members once per view generation instead of scanning per cell per frame. Sum columns on group rows and the desktop path folded into 8q0.38. split_pid_text noted on pdu.8.
- pdu.7 closed (53f0ad6, 85cfd81) before the index fix arrived; first_member still scans records per cell. New P1 pdu bead to index once per view generation.
- pdu.7 final 5713b4c: GroupMembers index (apps and units), rebuilt only on view version or slice change. Unit rows read text columns from the earliest member until 8q0.38 adds the unit's own fields. pdu.49 closes as covered.
- rsf.12 (963dd4e, 7d8b72a): column resize bands, live drag copy, fit from visible rows plus cached texts; drag does not touch the text cache. CellTextCache::cell_texts added.
- crw.11 final (fdf45b7, 6c949b1): TlButton focus_handle and trailing_icon; CPU page words via tr cached per strings version; grid rebuilt on style_version. range_word typed &'static str (folded into crw.29); per-CPU MHz missing (new crw bead).
- yrf.17 (ef22ee7): grid cell hover reusing hover.rs; widened plot hack and CPU grid hover off -> new yrf bead.
- pdu.49 closed as covered by 5713b4c.
- rsf.15 (b3b7dde, b4418a6): 3 table goldens pass; RTL blocked. Decisions in a new P1 rsf bead: FSI/PDI isolate before the count, TlIcon::Binary, Name column minimum 120 px. OWNER REVIEW: the Name minimum of 120 px is a leader call; PROC-023 gives 60 px for all columns.
- jqk.8 (daceda8, 95deff0, 11700bc): status chip; truncation keeps actions and paused tag whole; tab order and cached sizing noted on jqk.10.
- rsf.13 (a8631ab): column reorder; Esc via intercept_keystrokes while dragging (accepted); columns_fixed gating added to rsf.14.
- crw.30 (321f84c, c8546a7, d68b188): CpuTotals.per_cpu_mhz Arc (CpuTotals no longer Copy); grid tooltips show clocks; unchanged values keep the Arc.
- rsf.21 (5dc681d, 5265e51, 456205b, 03180c0, 3650424, 0e27d6a, 33ab37e): FSI/PDI isolate cached in IconSourceCache, TlIcon::Binary, Name min 120 px.
- pdu.8 (2248f10): process cell painting; tooltips, heat background and stale double-dim rule noted on pdu.12.
- crw.29 (f7c0916, d8b3a69, b40b1d4, dfae5b2): Performance words through tr tables; range_word is SharedString; tr_en accepted; tr_calls and set_test_catalogue test hooks. Leftover English moved to a new ug9 bead.
- rsf.15 (34e57c5): RTL golden committed; the 60 px golden is unchanged. The isolate swallows the space before the count, so a follow-up bead was filed.
- yrf.21 (dc9e308): HoverPaint.flip_right; CPU grid hover on; element.rs got one line outside the file list (accepted).
- pdu.9: EXITED branch in paint_name_cell approved (icon at STALE_OPACITY, text secondary).
- rsf.14: header_menu generic over C, using a ColumnId round trip for now; a follow-up bead makes the action key a string.
- 8q0.38: GroupRow.desktop_path added and left None; ProcessMeta.desktop_path is a follow-up bead. TableInput.unit_users added.
- 8q0.38 (c90aedc): closed by the leader; desktop_path source is 8q0.39. Extra files reducer/apply.rs, mod.rs and lab_table.rs accepted.
- rsf.14 (d4a699b, 003bd42, f3fa67d): header menu, ResetColumns and HideColumn, ToastQueue::of_window (accepted). TlMenu cannot show a tooltip on an enabled item, so "Hide <label>" never shows (low priority; left as is). System columns: rsf.22.
- pdu.9 (7271c9b, 4ed2132, 098f3ea): name_cell.rs with R-190 geometry; the EXITED branch approval was resent because the messages crossed.
- pdu.9 (f4299be, e644236): closed; EXITED rows via name_look in table/cells.rs. KernelThread RowKind noted on pdu.12.
- rsf.22 (eb67e1c): column actions carry a SharedString key; system tables can sort and hide from the header menu.
- jqk.10 (14f2aeb, 7a3ebc0, 771dfc7, 8a7e2d5): MainWindow. The handle is WindowHandle<Root> plus main_view WeakEntity (accepted, same as TlSecondaryWindow). match_single_binding allow stays until page arms land. Notes on jqk.1 and wlf.19.
- 8q0.39 (117501c): ProcessMeta.desktop_path set in proc/sweep.rs; app rows share the Arc.
- ug9.32 (71ec3f9, e973fbb, 22a3417, 82efe8f): KvWords global; cpu_details cache and boot words. The leftover English moved to a new ug9 bead. Test committed before build (process slip, fixed in a follow-up commit).
- rsf.23 (63f119c, 64421ad): isolate ends before the trailing space; RTL golden regenerated.
- jqk.10: the clippy errors were fixed in 771dfc7 before my message; HEAD is clean.
- rsf.2 (87a2657, ebebe40, 3e6c685): column picker. ColumnHost uses string keys (follows rsf.22, accepted). Group order comes from the layout. Five table-name keys added. Tab order and window centring moved to a follow-up bead. Unscaled minimum size matches the other windows (accepted).
- yms.31 (e887376, a4305ca): security reader with O_NOFOLLOW and a read cap on each path part. Snapshot rows for pids 1 and 4212 accepted. Leader fix requested: no readable ns reference gives Restricted, not Isolated.
- yms.31 fix (a6f50c2): no ns reference gives Restricted.
- ug9.33 (82678f8, a814452, 4f3af54): KvWords covers the empty text and copy labels; props fields store the UnsupportedWord variant. Follow-up bead filed for tr calls per frame in props filter.rs.
- jqk.1 (2c59d78): close decision, window actions, size saving with a 500 ms debounce. Show and toggle run through cx.defer; Quit saves the last observed bounds. TitleBarProbe.minimizes added (accepted). The test window cannot report maximized, so the test sets the probe flag.
- hk6.5 (529322c, efdc4e7, a568f66): sys status cell with a StatusWords global (R-132), cell_colors, STALE_DOT_ALPHA 0.5.
- rsf.17 (6068143): first_visible_row uses visible_range.start (logical_scroll_top_index only exists with test-support); strict scroll on restore; test for criterion 3 uses a one-row viewport. All accepted. Note added on jqk.15.
- ug9.34 (1990509): FilterWords on PropsFilter; asked to cache no_match too. m1f.11: set_pinned on TlSecondaryWindow approved.
- yms.34 (93d05f4, 372b6d0): handle walk and pipe peers. FD_WALK_MAX 65536 accepted; asked for a truncated_pids count. Fixture 4201 paths changed and 4214 rows added to the snapshots (accepted). HandleHitInfo.uid is the /proc/<pid> owner.
- yms.34 follow-up (d534b38): WalkProgress.truncated_pids. find_pipe_peers keeps the cap but has no counter (accepted).
- pdu.10 (844aa96, 1551b0e): header_total uses the cell contract and the header_state.totals cache. Network percent mode sets bit 63. No words.
- crw.4 (e8863c4, 521029c, a3560ea): Performance actions and keys. Leader: no schemars in tl-model; a crw bead adds JSON mirror enums in tl-ui.
- m1f.11 (76ae642, 59ce714, 8190f31): rescue window; set_pinned committed before my reply (approved anyway); follow-up in progress. The frame Tab order and the title on a language change are in a new m1f bead.
- jqk.14 (3f8f99c): Ctrl pause. observe_window_activation added. The case of a window removed while paused is noted on jqk.15.
- yms.35: UTF-16LE rule approved (even addresses; reject surrogates, controls and FFFD to FFFF, and units made of two ASCII bytes).
- bj6.32 (68e85fc..104185e): Share Theme window, Confirmation kind, kit Textarea for the code. Enter may fire twice (also in TlDialog), so a new ug9 bug bead was filed and also covers the title on a language change.
- pdu.12: the Copy row id uses Unit(u32) from an interner on the delegate (leader: no hash, because a clash could hit the wrong unit). Static validity tooltips are cached; want_tooltip is a new rsf bead.
- m1f.11 follow-up (c35351b, 862e636): set_pinned now shares its code path with toggle_pin.
- ug9.34 follow-up (081a45b): no_match text cached in FilterWords.
- yms.35 (e5e9ec9, e3275b6): strings scanner with a 1 MiB step, a 4096-byte value cap, MAX_STRINGS and MAX_TEXT_BYTES. pread from rustix::io. Detectors overlap on mixed text (accepted). Asked to confirm the random-bytes test.
- crw.31 (04b15eb): JSON mirror enums. The action fields keep the model types, and JSON goes through deserialize_with and schemars(with) (accepted).
- rsf.24 (d20a80c): TlCheckbox::tab_stop; the picker takes Option<AnyWindowHandle> through TlWindowSpec::parent (accepted).
- yms.35 follow-up (660714f): random bytes give about 60k hits per MiB, mostly UTF-16LE. Leader rule: a UTF-16LE run keeps one high byte, and the threshold is under 20k per MiB.
- bj6.9: SourceHealth.restarts added (an 8hz bead fills it); 7 state words; names for the open item 6 sources; all 21 rows shown.
- yms.35 (3e572d3, 283062e): a UTF-16LE run keeps one high byte; space, digits and punctuation stay inside a run of another alphabet. Random data gives 0 UTF-16LE hits.
- cr0.1 (cb51c7b, d25c86a): the binary starts and the window opens on Wayland. first_frame_ms=900 (about 450 ms in the wgpu adapter probe), over the ADR-014 gates; a cr0 startup bead was filed. MANUAL DESKTOP CHECK: closing the window quits with exit 0; the crash report on a panic (no injection hook in production).
- crw.6 (6e6b29a, 0280e89): device sidebar. Two follow-up crw beads: the memory card shows a percent with words through tr, and the sidebar drops its demand on other tabs. Fan scale has no maximum (accepted).
- rsf.16 (519bdbf): column separators, one canvas per row, painted after the cells (accepted).
- jqk.16 (75f1071): server-decorations menu row and MenuRowHeight global; the bar height follows the reported decorations.
