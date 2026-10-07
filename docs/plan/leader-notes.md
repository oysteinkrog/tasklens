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
- m1f.18 (16d8f29): the frame pin and close sit in tab group index 1 after the content; close is a focusable kit Button; the rescue title follows strings_version. The ui_columns expectation was updated (accepted).
- pdu.12 (9a16718, 75b5172, 0558014): ProcessDelegate. Leader fix requested: Unit row id from an interner, not FxHash. Accepted: clk_tck is fixed at 100 (USER_HZ); gpu_N is offered per adapter; persist through TlSettings::update. API noted on pdu.1.
- cr0.12 (51ceea4, e6d3fed, d14a60a): single instance on the session bus, with input validation and bounded(4) try_send; the real-bus test runs on a private dbus-daemon. The forwarding test is ignored until acquire is wired (noted on cr0.2).
- cr0.13 (11369b6, d2d2b88, 963e4d5, 8872276): notify.rs keeps fixed English text, because the panic path cannot use GPUI; keys are added and checked by tr_en equality (accepted). MANUAL DESKTOP CHECK: one 'tasklens crashed' test notification was sent at about 21:20 on 2026-10-06; did it appear?
- m1f.3 (9632901): meminfo reader with no allocation, a proptest, and MemInfoReader (fixed 4 KiB pread). Noted on tasklens-m1f.1.
- cr0.10 (7ccc7e2, f869051): migrations.rs; a bad schema_version is logged without its value.
- crw.32 (d371e74, ba948ac, c4b3f0b): ValueFormat::PercentOf; CardWords table; chart/mod.rs and scale.rs touched (accepted).
- cr0.10: logged above. cr0.23: logo SVG text becomes paths (an SVG <text> makes gpui scan all system fonts); warm first frame 290 to 500 ms at load 37; GPU adapter probe 230 to 270 ms. Note added on ug9.26.
- wlf.15 (b5eb1b4): gallery_main.rs; needs a tl-ui gallery hook behind bench (approved in app.rs).
- pdu.12 fix (9eae47a, 59a7a1e): UnitIds counter keyed by the full UnitRef (manager plus name; accepted); ids are built once per view.
- bj6.9 (73d5774, 84ed085, fecabda, d9558a0, bd0cdea, 9a17d2a): Collector Health. Off rows hidden; SourceHealth.restarts added. Source names and the 'ago' word are still English and concatenated, so a follow-up bj6 bead was filed (after ug9.35).
- bj6.48 closed: bj6.9 did it in f6c6f82. Cargo.lock proptest line committed (m1f.3 left it).
- 8hz.12 (a664d0f): RestartCounts. Counts reset when a session restarts; the doc now says per session (accepted). bj6.9 closed by the leader.
- crw.33 (fe1466c): DeviceSidebar::set_active and PerformancePage::set_active; crw.1 must call them on tab activation.
- cr0.9 (ef0e8f0, 1d57223, f7c0eb1): settings store. A corrupt file is renamed to .corrupt-<unix>; a newer file opens read-only. Clamp logs name the key only, with no values (leader privacy rule overrides the bead text).
- wlf.15 (b5eb1b4, 8d691f8, de6315c): gallery entry and the tl-ui hook behind bench. The gallery window opened at 1850 ms (bench build, machine under load). MANUAL DESKTOP CHECK after wlf.16: run tasklens --gallery, close the window, and check it exits 0 within 2 s.
- ug9.35 (6ad0e77, 27f0ff6): the Enter double fire was not real; a key binding runs before button key-down. Fixed: Enter in a text field now runs the default button; the Share Theme title follows strings_version. Key-up tests added.
- gma.4 (b9d5809, 31029cd): process window actions; OpenVirusTotal and CreateDump are re-exported from pdu.5 (accepted). jqk.11's build break was fixed in f41d785.
- cr0.23 (41b0299, 9515dab, f3007b8): perf step logs; the logo SVG is now paths. Warm median 385 to 365 ms, cold 2660 to 1060 ms (load 25 to 30). The gpui window_open takes 276 ms, of which about 265 ms is the adapter probe (Vulkan and GL both enabled, a full device and surface test). OWNER REVIEW: the 200 ms warm gate cannot pass on this hardware without a gpui change. The options are in docs/spike/phase0-results.md: raise the gate, patch gpui through localbuilds, or cache the adapter. Rerun on an idle machine.
- cr0.11 (28af994, 7da3d53): settings save half; wiring noted on cr0.2. themes.json uses tl-ui's format (accepted).
- pdu.1 (a1fcb0a, c23c046, f683bc4): ProcessesPage. The main.rs arm and the ui_shell test fix are approved (re-sent).
- pdu.1 arm (10a1107): Processes is in page_factory; GlobalHub is set at app.rs:253 before the open hook, so the panic is test-only. pdu.13 tests remain. jqk.11 (c9136bf, 9d8bc47, f41d785, d677fb5) is wiring main.rs now.
- rsf.25 (2c5e7a1, 52029b4): want_tooltip is set only when hovered and hover_tips_ready. Stale age comes from the process header observed_end; a stale extras cell shows the process sample age (accepted for now, minor).
- rsf.25 extra: Services child process rows do not get Stale and Failed tooltips (only ProcessDelegate builds them); the Services delegate bead should call live_tip.
- cr0.3 (4842348): PKGBUILD with options !strip !lto (makepkg LTO broke ring). The package has the five files; tasklens --version works. check() under release fat LTO thrashed, so a cr0 bead moves it to the dev profile. namcap is not installed (OWNER: optional install of namcap for the package lint).
- m1f.15 (ad1b317): --low-memory init dispatch in start(); the rescue run loads only Regular and SemiBold.
- pdu.13 (10a1107, 074bf0d): the Processes page is built on first visit and kept; an inactive page is not drawn.
- pdu.14 (8cc741a): guide_lines() turned on; sibling_pass off-by-one fixed per PROC-017; RowFlags::IN_GROUP puts the group line at x 16. The extra wrapper div plus canvas per row is merged with the separators canvas in a new rsf bead. pdu.1: the tl warning came from a draft and is not in HEAD.
- cr0.2 (9d30119): acquire, load, Store, flush, and crash notify wired into main. tasklens's own startup steps fell from 6.9 to 4.3 ms warm; single_instance takes 1.4 ms warm and 9.2 ms cold. Notes on jqk.20 and jqk.21. MANUAL DESKTOP CHECK: settings are saved when you close the window.
- pdu.25 (bbb8697, aeb7a86, 757f069): Processes toolbar. TlIcon gains LayoutGrid, Rows2, ListTree and List; gallery goldens regenerated and checked by the agent. pdu.26 registers the toolbar in main.rs.
- cr0.24 (3fb13e0): check() runs in the dev profile and compiles in 9 min. It found a tl-model ceil_125 subnormal bug; a new yrf bug bead blocks the cr0.24 rerun.
- cr0.17 (263609a, 9193258): the autostart entry follows [startup] through the save thread; tests use a scratch XDG folder.
- jqk.11 (57a53a4, ed3cf9d): the filter box is wired into MainWindow; tests run on the real window.
- m1f.4 (815729a, 96fc890, 76f80a2): the lowmem unit template and install, enable and remove; unit_path returns a Result (accepted). OWNER RUN (U-24): `heavy-build cargo test -p tl-app --test watch_memory -- --ignored real_install_enable_remove --nocapture` installs the unit in your real user session, removes it, and prints the accepted OOMScoreAdjust.
- cr0.19 (acf9d32, 9193258): rendering_env. GPU selection does not shorten the adapter probe. "software" may still pick a GL adapter for the real GPU; a cr0 bug bead was filed to verify it.
- cr0.21 (8361a3c, da42146, 66f0f74): import_ts with a 1 MiB cap, clamp-or-skip, and only key names logged. The wiring, a callback that takes a path from the portal chooser (APP-034), and UTF-16 BOM detection are in a new cr0 P4 bead.
- cr0.14 (eb5cbb1): finish() flushes the store, then shuts the engine down within 1500 ms; U-2 was answered from source (Application::run returns after quit). MANUAL DESKTOP CHECK: close the window, time the exit (<= 2 s), and check that `pgrep tl-gpu-probe` finds nothing.
- pdu.15 (8369ce1, 09caa37, eca9adf): new and exited tint with a 300 ms fade, no row rebuilds, frames requested only while fading; TreeTableDelegate gains row_tint and before_rows (accepted); heat cells no longer double-tint.
- yrf.22 (bfb80ef): ceil_125 returns 5e-308 for x <= 5e-308. ceil_125(1e-300) gives 2e-300, still >= x (fine).
- Process note: tl-app main.rs pathspec commits took other agents' unbuilt edits twice (9193258 had cr0.19's; 92eadf3 had cr0.15's). From now on only one agent at a time edits tl-app main.rs; the leader holds main.rs beads until it is free.
- pdu.26 (8625b2d, 12c09d2): toolbar_for now has a Processes arm and the allow is removed; the tests use the real MainWindow. Nothing in src handles ToggleHideSystem or CycleViewMode yet; when a handler lands, the app-level counters in those tests must change.
- cr0.22 (cfb9e79, 92eadf3): --app-id paths and bus name are validated; '-' maps to '_' in the object path. The window app_id and the autostart file name are on jqk.32.
- jqk.12 (cc5eb39, 4abb95b): Include children checkbox. No Tab binding exists in the main window, so a new jqk P1 bug binds Tab and Shift+Tab after jqk.13. The accent line gap is in a jqk P3 bead. filter_esc_rule is jqk.13's to update.
- cr0.22 follow-up (3ea3756): autostart file per app id; Exec passes --app-id.
- cr0.15 (92eadf3, 42d7317): renderer failure notice and a single relaunch after device loss (with a marker; the session bus is closed before relaunch). The RendererFailed path relies on jqk.10's open hook calling request_exit(RendererFailed).
- jqk.13 (18de48b, a6ae01e, 14090d2): Esc chain on MainWindow; filter_esc_rule updated. CancelWindowPick has no handler until E20-027.
- cr0.26: the import callback returns ImportedSettings (settings, state, summary); tl-ui applies them through TlSettings::update. Leader approved: one write path, so the UI and the store stay in sync.
- pdu.16 (c40b5b5, e2346c7, f617d58, fd86c72): RowTooltip view, one per table, filled only on hover. Kernel threads get no path line. The build break was fixed in e2346c7. All accepted.
- cr0.25 closed: rendering.gpu = software now also sets both lavapipe ICD paths, __EGL_VENDOR_LIBRARY_FILENAMES (Mesa) and LIBGL_ALWAYS_SOFTWARE; gpui picks llvmpipe Vulkan (5bde23c, 3503156). Follow-up cr0.27: exit when the main window cannot open.
- m1f.1 closed: watcher loop and real I/O (afdfa01, a59e519). The PSI trigger needs a trailing NUL. Comment on m1f.5: the --watch-memory arm must run before logging::init.
- Leader 9856fbf: Fatal::key is test-only (clippy dead_code).
- rsf.26 closed: one line canvas per row for guides and separators (a738bee). Follow-up: regenerate two render_table goldens for pdu.15 dimming.
- jqk.34 closed: accent line ends at the filter outline (4f7e01d).
- cr0.26 closed: import callback takes path, Settings and UiState and returns ImportedSettings; tl-app saves nothing (5dce869, b3c1e46, ff1b7f0, 7bd7e68). The UI half is on bj6.45 (comment added).
- jqk.33 closed without new bindings: gpui_kit Root already binds Tab and Shift+Tab in every window; real-keystroke tests added (082485e, a2513c8). Leader accepts: a Global binding would bypass the kit focus trap.
- rsf.27 closed: two render_table goldens regenerated; every changed pixel is in the exited gnome-shell Name cell (842fe30).
- cr0.24 closed: makepkg -f with check() passes at fe4b28a; 3287 tests passed, 0 failed, 105 ignored (3fb13e0, c8b6540). Package not installed.
- jqk.15 closed: Ctrl+Plus/Minus/0 UI scale with toast; metrics re-apply keeps the Processes first visible row; compositor scale bumps style_version; removing the window ends Ctrl-pause (136035b, 18e7f6d). Accepted extras: apply_metrics once at window open; persisted counter in ui_shell Opts. Every later page with a table must add its arm to keep_first_row in window/main.rs.
- 8fx.19 closed: HistoryPage, header_text(words, view, state, out), layout slots, HISTORY-bit notify (d37f5ed, d4fcc6e, 558a415). Accepted: words argument and three history-duration keys. Leader decision: scrub age counts from view.span.1 (newest retained frame); revisit only if the recording-off banner bead (E30-026) shows stale ages while recording is off.
- yms.40 decision: keep ADR-003 at 160 bytes. ProcessRecord gets euid, net_shared and cancelled_write_bps (f32); per-GPU VRAM goes in ProcessExtrasP2.gpu_vram_bytes. AMD and Intel per-process VRAM is a separate bead under ndy.
- 8fx.20 closed: QueryController, one query in flight, 1 s live loop, 1,000 ms resend, Busy retry (82cc143). Follow-up for queries on view range, metric and selection changes while scrubbed.
- bj6.19 closed: Keyboard Shortcuts window built from a keymap walk (9e926a3, f053f53, 8c6099c). Accepted: R-220 headings ("Startup Apps"), struct entries, "Open Debug menu" in debug only, Esc clears search first. Follow-up for keys missing from table (b).
- jqk.18 closed: health items in the status chip (894dfdd, 6de5f10). Accepted: two extra chip names (GPU, Services), Restricted item clickable and drawn last. 6de5f10 swept in two cr0.27 hunks in window/main.rs; cr0.27 told to verify and continue. Root cause: a pathspec commit takes files whole from disk. brief-implement.md step 5 now says to stage hunks with git apply --cached and commit without a pathspec when a file holds another agent's hunks.
- bj6.49 closed: Shortcuts rows for Alt+Up/Down and a Dialogs and windows heading; table (b) in docs/RECONCILIATION.md updated (73913fe, ee83e14).
- cr0.27 closed: the quit requested in on_finish_launching was lost because calloop clears the stop flag at loop start; a second request from a loop task fixes it; exit 1 after 1144 ms (6de5f10, 6399af1). Follow-ups: request_exit for all early paths, no-display exit, engine teardown during start.
- yms.40 closed: ProcessRecord euid, net_shared, cancelled_write_bps (still 160 bytes); ProcessExtrasP2.gpu_vram_bytes from NVIDIA samples; table::gpu_columns helper (fc17e9c). Sweep bench 20.8 ms and 27.4 ms, but no clean baseline: recheck with yms.26 on an idle machine. pdu.50 goes to the pdu.17 agent.
- cr0.27: the swept hunks in 6de5f10 are its final version; no follow-up needed.
- pdu.17 closed: cell_tooltip with 8 of 12 texts, 600 ms process cell delay via TreeTableDelegate::cell_tip_delay, TlTooltip::preformatted for the command line (986656d, fe4b28a, ad7f7b4, 87e805f). The other four go in pdu.50.
- 8hz.13 closed: the UI's EngineHandle clone (EngineGlobal, never dropped by GPUI) kept rt_handle alive, so shutdown_timeout always ran out; shutdown now takes rt_handle out of Inner at ADR-004 step 3 (83693d8). Leader accepts reversing lab_bridge's old live-clone test: clones see a stopped engine. Likely also fixes the normal Quit path; covered by the OWNER manual check 'app quits within 2 s'.
- m1f.5 closed: --watch-memory arm runs before logging::init, 1 thread (da6f444, 4cddd56). VmRSS 14.5 MiB and mlockall ENOMEM inside the tasklens binary. Leader decision: the watcher becomes its own small binary tl-watch (std plus libc), the unit runs it, tasklens --watch-memory execs it.
- ndy.17 closed: DRM per-client VRAM from the gpu_drm fdinfo scan, matched to GPU N by PCI slot; reducer keeps NVIDIA and DRM rows apart (53eed0e). Accepted the tl-collect fdinfo parser change. Follow-up: GPU memory validity on DRM-only machines.
- pdu.50 closed: the four remaining tooltips (a50d2b1, a2e502e). Leader decision: the cancelled-writes tooltip shows a rate, "cancelled: 1.20 MiB/s".
- fut.3 closed: ActionTracker with outcome toasts, retry as administrator, Details (0ac9390, c17434d, b54c6c9, 039aec5, 5ec776e). Accepted: TlDialog::without_cancel, no toast on EngineStopped, one button per partial toast (Retry wins on EPERM), retry only PermissionDenied targets, the Details target labels. Follow-up: the low-memory window's ActionTracker marker.
- jqk.19 closed: ToggleHideSystem, CycleViewMode, SetViewMode with toasts (61bd129, a5df591). Follow-up to remove duplicate view mode strings: keep processes-toolbar-* keys.
- 8fx.22 closed: History table delegate (9b6615f, 8afec71, 814eda6). Accepted: icons only for running apps, line 2 totals for CPU and Memory only, "(previous boot)" wins, selection diffing, query only when the first 8 keys change. Follow-up: apply a filter set before History mode.
- 8fx.38 closed: set_view_range, set_metric, set_selected send one query while scrubbed (ffa7653, 6bfb30f).
- pdu.50's rate fix c94b9b8 swept in m1f.19's two staged renames (watch_memory.rs into crates/tl-watch). No revert; m1f.19 told to land a building state first.
- ndy.18 closed: separate validity for GPU utilization and memory; DRM rows make memory Valid (450964a, ef833c6). On an Intel iGPU-only machine every process shows a valid 0 for dedicated GPU memory; accepted, it is true.
- cr0.28 closed: request_exit quits again from a loop task for every early path; local workaround removed (405c41c).
- bj6.1 closed: menu bar and File menu (64f7651, adc6561, 1c29dcc). Accepted highlight_first on TlMenuPopup, plain-div buttons for the 26 px spec, has_menu plus build_menu. For later menu beads: test Left/Right between two menus; add check and radio state to the row accessible name with the first check items.
- gma.6 closed: Set Affinity dialog (9808c43, 6f4ff65, 01a269d). Accepted: one L3 group shows "CPUs: {list}", empty cpuset_limit means no limit, Enter on Cancel cancels, open() returns the handle, live apply of each valid edit. Follow-up for P/E icons, CPU label style and first focus.
- jqk.17 closed: tab overflow menu, FilterBox::target_width with the prepaint-measured input width, TlIcon::ChevronsRight (fbd9544, 7ea03b9).
- 9o0.21 closed: InfoSection, rows on TlKeyValueGrid with multi-line values, indent flag, label inset, on_copy_section (8e7cf7e, 10c65ee, bc724e6, f0695ce, 7e08f15, 2192d45). The bead wins over the UI-SYS-081 "one line" wording. Follow-up for accessible names, label tooltips and unused keys.
- pdu.19 closed: process action toasts through track_or_toast (496554d tracker change, 8e351fb, dca8468, a065b4a). Accepted: texts_for returns Option, refusal_reason and refuse helpers, End counts member_count per row, per-reason counts for End and reason lists for priority, affinity and I/O. Reason wording beyond permission denied stays open item 6 in b8-ui-tables.md.
- m1f.19 closed: tasklens-watch is its own binary (std plus libc), flock single instance, unit runs it directly, tasklens --watch-memory execs it (4b1b8ee, 96c753c). Release VmRSS 3776 kB after 2 s, VmLck 3768 kB, mlockall OK under 8 MiB, 1 thread. Margin about 320 kB, mostly locked libc; recheck under systemd during the OWNER RUN of U-24. No panic = "abort" (Cargo forbids it in a package override); accepted.
- cr0.29 closed: no WAYLAND_DISPLAY and no DISPLAY prints "tasklens needs a Wayland or X11 display" and exits 1 in 37 ms, also with --start-minimized and --low-memory (8c28614). Bug confirmed on the old binary first.
- gma.1 closed: PropsWindow and General tab (1055d43, 864a15e, c8f3807). Accepted: CircleOff and ShieldAlert only in TlExtraIcons, notification counts as the render-counter proxy, ninth-window toast to the active window, scan_running false until Strings exists. Follow-ups: TlTabs tooltips on enabled cells; freeze when the key disappears without EXITED. WINDOWS bit is for gma.16.

## Reboot stop point, 2026-10-07 about 09:10

The PC reboots to move the root disk. Agents were told to commit only green work, leave untested work on disk, and write a bead comment with their uncommitted files.
In progress at the stop: fut.11, jqk.2, 9o0.22 (then 9o0.34, 9o0.23), bj6.10, 8fx.24 (then 8fx.39), pdu.20, m1f.2 (then m1f.20), m1f.6, gma.15 (then gma.41, gma.40), bj6.17.
Queued next: m1f.21 (after m1f.6), jqk.35, jqk.32, gma.39.
After the reboot: read each in-progress bead's latest comment, check `git status` against it, then resume each bead with a fresh agent (the old agents do not survive). Run the full tl-ui clippy with and without test-hooks once the tree is quiet.
- Reboot note: gma.15 committed abd5beb and 14be5fe without compiling them, so HEAD may not build until its tests run; check first after the reboot. gma.15 also killed pdu.20's and bj6.17's queued heavy-build jobs with a pattern pkill; both rerun after the reboot. The brief now forbids killing by pattern.
- jqk.2 closed: type-to-filter routing from Table context, U-45 verified (6226311). Accepted: also blocks the tl-ui contexts TlMenu, TlDialog and TlInput.
- pdu.20 closed: EndTask, EndProcess, EndProcessTree handlers through pdu.19's submit (1845e4a, 62087c9, 223eaa9, 6e03bcd). Accepted: one refusal toast per distinct reason with the rest still sent, dialog from 2 sent keys, app and unit rows take the union of their primaries' flags, tree acts on the first process row and warns if any subtree process is protected. Follow-up: refuse a tree that contains tasklens or PID 1.
- 9o0.22 closed: Information row builders for OS, Processor, System, BIOS, Motherboard (3aa2431, beb4f9f, 3cdf6a3, 5dc1f6b). Session type shows the type alone until the model has a logind Class (follow-up bead). 9o0.34 committed (9540194, 6042214, 49463c1), tests wait for the build hold.
- 2026-10-07 build queue: the leader stopped 7 duplicate or stale heavy-build jobs from this session (manager-machine asked for at most 2 queued). Only gma.15 and m1f.6 may build; the other agents are paused until told "your turn". Brief: one combined heavy-build job per agent.
- 2026-10-07 about 11:45, build queue overload: agents re-queued 7 jobs after the pause message, so the leader stopped those jobs again and stopped six agents (fut.11, bj6.10, 8fx.24 with 8fx.39, m1f.2 with m1f.20, bj6.17, pdu.51). Their uncommitted work stays on disk:
  - fut.11: components/gallery.rs, tests/ui_components.rs (shared), six untracked gallery_{dialog,menu,shield}_1_{0,1}.png goldens.
  - bj6.10: window/menus_file.rs, tests/ui_menus.rs.
  - pdu.51: pages/processes/mod.rs, tests/ui_process_menu.rs.
  - 8fx.24 (last commit 77cc6f16), bj6.17 (5ff1c9ff), m1f.2 (3327378f): check their bead status and comments before resuming.
  Still running: gma.15 agent (then gma.41, gma.40), m1f.6, 9o0.22 agent (9o0.34, then 9o0.23). Resume the stopped beads with fresh agents, one or two at a time, when the machine has build capacity again.
- 2026-10-07 14:24: the PC rebooted at 13:38 and reboots again at 14:28. No agents ran between the two boots. The on-disk state is unchanged from the 11:45 note (nine modified files and six untracked gallery goldens). Resume plan stays the same: first a build check of HEAD (gma.15's abd5beb and 14be5fe were never compiled), then the open beads with one or two fresh agents at a time.

## Owner direction, 2026-10-07 afternoon

Oystein wants the system monitor graphs pushed forward, and one view that shows CPU, memory and GPU load together (TaskSlinger lacks it). That view is the Graph Summary View (PERF-010, crw.17), now P1 and depending only on crw.1 and o55.11.
Order: pdu.52 (P0, live data missing in the real app) and crw.1 (Performance tab) first; then o55.2 Memory, o55.9 GPU, o55.5 Disk, o55.7 Net pages; then o55.11 build_page arms; then crw.17. The six interrupted beads from the reboot (pdu.51, bj6.10, fut.11, 8fx.24, m1f.2, bj6.17) wait behind this path.
Do not take desktop screenshots to show Oystein the app; he sees his own screen.
- gma.15 closed (tests de662d7e). gma.41 closed (b63b1cd7): freeze as exited when the key leaves a snapshot whose PROCESSES component changed; seen_exited removed. gma.40 closed (9e84f3ae, 984f20d2): TlTabSpec::tooltip, new ftl key pwn-props-tab-tooltip.
- 49105ed7 fixed the 3c23e489 build break (BorrowAppContext import); tl-ui lib check passed.
- pdu.51 resumed with a fresh agent on the work left on disk.
- m1f.6 closed (fe82e965). m1f.21 closed (3c23e489): AppRequest::SetLowMemoryEnabled(prev) on a failed install or remove; Outcome::Primary carries the sender and QUEUE is pub (outside the file list, accepted). Same agent now takes m1f.2, then m1f.20.
- 9o0.34 closed (bf97ed59). 9o0.23: builders in 060859bc and 06a370da. Leader approved label_lines in kv_grid.rs and section.rs for Peak committed (default 1). Accepted choices: NVMe/SSD/eMMC/SD show "SSD", HDD "HDD", other kinds as given; disks by device index; no "@ N Hz" when refresh is 0; no IP shows "None"; GPU memory falls back to live totals; info_no_dimm_serial checks the UI side only.
- m1f.2: failure toast uses the ActionTracker text ("End Task Failed. " plus the tracker message), no strerror table; no success toast in the rescue window. Bead text updated to match.
- fut.11: may edit components/toast.rs (pub(crate) static card shared with ToastQueue) and window_frame.rs (title bar and pin pulled out into a stateless element, no behavior change), so the gallery has all five pages.
- m1f.2 closed (2856a37d): no success toast, ActionTracker::push skips an empty text (outside the file list, accepted). m1f.20 closed (017f1f5d). The same agent takes bj6.10.
- 9o0.23 closed (060859bc, 06a370da, 4c10ea99): label_lines by row index on the grid, like label_tooltips, so TlKvRow literals stay unchanged. The same agent takes 8fx.24, then 8fx.39.
- pdu.51 closed (9aa00583, 1b837169): the subtree check refuses IS_SELF or PID 1. The test data had to be in PID order (Forest::index_of binary-searches). The same agent takes bj6.17.
- fut.11 closed (19c6e83a, a066eb4e, d7d0a878, f8cf47c6, 47c05f0f, a1f3cf32): five gallery pages; TlTitleBar and TlWindowPin hold no state; static_card only with the bench feature. The gallery_icons goldens are updated for the ChevronsRight icon that jqk.17 added. The inactive window frame is not drawn separately, because the compositor dims the border and shadow, not the bar. The same agent takes gma.39.
- bj6.10 closed (2dc7fadf): Run... with Ctrl+N, separator, Exit tasklens. The same agent takes bj6.4.
- bj6.17 closed (3ac55981, 0cc680d5, 5ff1c9ff, 707e241e): About links show only failure toasts, per the OpenUri decision. pdu.51 clippy confirmed. The same agent takes pdu.21.
- gma.39: P-core and E-core icons sit in the group headers only. A 10 px icon in the 72 px tile (64 px inside) would cut "CPU 23" off. Tiles keep the core-type tooltip.
- pdu.52 closed (45e220b5, 047364fe, 346d0ee0, 8f89a28d, 0a00b007, a6db65fd). Cause 1: the Processes page sent a demand part with any_surface_visible false, which merges to Demand::default(). Cause 2: the hub marked the window hidden because a data change redraws only the cached table, not MainWindow::render; MainWindow now counts its next frame. New live test in tl-app/tests/live_processes.rs. The demand lesson is a comment on hk6.6, hk6.12, 9o0.7, 9o0.15, 9o0.24. Open: a release-binary check with the screen unlocked. crw.1 must fix ui_shell startup_tab_unbuilt_falls_back.
- pdu.21: CapturedTarget gains stopped: bool (from ProcState::Stopped) so Suspend and Resume follow State T; edit outside the file list, approved.
- 8fx.24 closed (ed9a82d8, e2820c1a, 77cc6f16). 8fx.39 (b0980008): HistoryPage reads the main FilterBox text once through defer_in; clippy was blocked by in-progress pdu.21, end-of-goal clippy covers it. The same agent takes 9o0.24.
- hk6.6 closed (eba859c1): sys table demand sets any_surface_visible and the page flag, carries the main window, uses reserved aggregator ids; live test file renamed to tl-app/tests/live_engine.rs with a Startup view test. No resend on EngineStopped. The same agent takes hk6.7.
- pdu.21 closed (81a8b8d5): Suspend, Resume, Set Priority (nice -10, -5, 0, 5, 19); end() split into shared prune() and send(); EndWords renamed ConfirmWords. gma.39 closed (8c6363a9, d08613ea): P/E icons in group headers, label_icon dropped from TlCheckbox; follow-up puts P-core or E-core in hybrid tile tooltips. Next: pdu.22 and gma.16.
- pdu.22: the table keeps its own CopyRowsAsTable TSV and the page toasts on CopiedRows; no second TSV builder. All copy paths use the ADR-015 words through validity.rs copy_text ("Restricted" capitalized); the bead text is corrected. CopyDetails is declared in processes/actions.rs; pdu.30 adds the binding.
- bj6.4 (082e49d7): build returns Entity<TlMenuPopup> like menus_file; View always has a button; resize marks tested through menus_view::resize_marks. Final check waits on the hk6.7 sys_table.rs debug_selector fix (84bad80c broke the tl-ui build).
- 9o0.24 closed (3caa85b8, 018d2b2d, 258d15ab): the Information page demand covers inventory, memory, gpus, disks and, as a follow-up, cpu, so the System counts fill. InfoSection measures its height for the cached view (section.rs edit outside the list, accepted). filter field deferred to 9o0.25. The same agent takes 9o0.27.
- gma.16 closed (1a58e8b6): window block in General column 3; PropsWindow calls GeneralTab::set_windows (props/mod.rs, outside the list, accepted). gma.39 follow-up 19666d91: P-core or E-core in hybrid tile tooltips. The same agent takes jqk.22.
- hk6.7 closed (84bad80c, 14059472, b16f141a): sys_body rules, skeleton capped at 12 rows, UI-SYS-005 empty states as data. Follow-up: TlEmptyState gains an optional heading, button label and icon tone so sys_table.rs uses the shared component. Missing TlIcon names use gpui_kit IconName. The same agent takes hk6.12.
- crw.1 closed (54bc91a8, c48313d1, 45ea86c6, d533f028, 56ebfc17, 13e9505b): fresh split state on window width change keeps nav_width; focus moves to the page before the sidebar hides; mod.rs downcast list for set_active and focus_order (o55.11 adds a line per page type); Removed drops the device page. All accepted.
- o55 pages: each page gets its own test file (ui_memory_page.rs, ui_gpu_page.rs, ui_disk_page.rs, ui_net_page.rs) instead of the shared ui_device_pages.rs the beads name, so parallel agents do not collide. crw.1 agent takes o55.2 then o55.5; a new agent takes o55.9 then o55.10; o55.7 goes to the next free agent.
- o55.9: vendor from GraphicsAdapter.kernel_module (nvidia, amdgpu or radeon, i915 or xe), not the UUID; ChartGridView::set_state approved in chart/grid_element.rs; engines listed unless Unsupported. o55.10 is done partially from GpuTotals and GraphicsAdapter; the four missing model fields moved to new bead o55.36 (P2).
- jqk.22 closed (21af077d): FPS overlay, nothing runs while off, never requests a frame. Frame CPU time was always 0, so new bead jqk.36 adds tl_model::time::thread_cpu_ns. The same agent takes jqk.36, then o55.7.
- bj6.4 closed (082e49d7). The same agent takes bj6.3.
- pdu.22 closed. The same agent takes wlf.9 (P0 bench scenes).
- pdu.22 closed (6e74730c, 70593a9e, dfbe1a40, 144e385d, d4e22e17, b24f66f2): the table TSV with a CopiedRows toast, copy_text words everywhere. 144e385d swept a 9o0 ftl line; d4e22e17 put it back. 9o0.24 follow-up b84b3a56 adds CPU demand. 9o0.25 closed (1c016211). The thread count needs the process sweep, so new bead 9o0.36 reads it from /proc/loadavg instead. The 9o0 agent takes 9o0.26, 9o0.27, then 9o0.36.
- 9o0.26: follows R-199 (write_to_clipboard returns () in gpui-pre 0.3.7), success toast only; the failure AC is dropped. CopyInformationAll is declared in pages/information/actions.rs.
- o55.7: ChartView gains value_sum (set_value_sum) so Throughput shows rx + tx; an invalid newest sample in any series shows that state, never a partial sum.
- hk6.7 follow-up (7c6c543a, 51b406d4): TlEmptyState gains heading, button_label, tone, kit_icon and one_line; the text cap and centring apply only under a heading, so existing callers and the gallery golden are unchanged.
- hk6.9: ActionTracker gains ToastTexts.owner_toasts (default false: no tracker toasts, still sends, sweeps and emits), ActionTrackerEvent::TargetFinished and results(id), with finished results kept for a bounded time. SysActions owns the system-table toasts (pending toasts at submit, fxhash keys, failure_text, View logs). Process action toasts are unchanged.
- 69a93ee5 (o55.7) swept hk6.9 ftl lines; nothing lost. The brief now forbids naming en-US.ftl in a pathspec commit.
- bj6.3 closed (9346c4d1, 7d234d10, 7ddcb69c). Follow-up: TlDialog::cancel_label so shell-confirm-cancel is used. The same agent takes bj6.5.
- o55.9: ChartView::set_value_text override for "used / total" text, set only when the numbers change; made after gma lands value_sum in chart/view.rs. Agents sometimes miss leader messages sent while they are busy, so resend the answer when a report says it is still waiting.
- wlf.9 closed (a2c8b7e8, 98215f1e, 18bfb112): bench scenes with honest measurement; latency counts samples that were newest at a paint; probe_cpu_pct null in phase 0; table-2000 errors until wlf.10. The same agent takes wlf.11, then wlf.10, with a first real run under the build lock.
- o55.7 (69a93ee5, 8cf6c9a1, b70efc9a, 0d311c21, fb09758d, e0acc2a4): value_sum uses the invalid series state (Failed shows !, Missing or Unsupported shows -, WarmingUp shows ...). A trailing invalid run draws no dotted span; the disconnected test checks that the lines stop early.
- wlf.11 (5c75f303): tl_ui::bench::run_app builds the GPUI app for bench_main, so tl-app needs no gpui-kit dependency. The same agent takes wlf.12 (main.rs bench arm) to make it runnable.
- HEAD broke again (hk6.9 sys_table.rs:827). The brief now says the first build after a src commit is a cargo check and lib clippy, fixed at once. 9o0.27 is committed and waits for a green build; the 9o0 agent starts 9o0.36.
- hk6.9 (abec715a tracker, f7b1d4c1, b93a0f7a, 7cf9fc7e): owner_toasts, TargetFinished, results() bounded to the last 16. Action texts must use i18n::tr_args, not text substitution. The uncommitted gpu.rs (o55.9) blocks the tl-ui build. The copy_rows.tsv golden lacks the trailing tab the table writes for an empty last cell; pdu.22 owner to fix it.
- d1e31084 (hk6.9) left a dead field at sys_table.rs:516, so HEAD clippy is red; the owner is fixing it. o55.7 and jqk.36 may close with clippy -A dead_code noted. The gma agent takes crw.12 next.
- o55.2 closed (43d65dff, 833943a8, 6e97e8fe, 148351f9) and o55.5 closed (00e82b34, 25142bf6). Extra range keys are accepted. The Commit tooltip covers the whole chart box. Disk N follows the sidebar order. The dead hub field is removed. The crw agent takes o55.11 now (leader override of the o55.9 block: GPU arm last), then crw.17.
- bj6.3 follow-up e23ce56f: TlDialog::cancel_label. bj6.5 closed (3d332eed): Process View submenu. The same agent takes bj6.7.
- o55.9 and o55.10 closed (3cf76d72, 33756fe1, aa5d818e, 141e573a, 100d77f0): vendor from GraphicsAdapter.kernel_module, ChartGridView::set_state, labels.rs glyphs pub(crate); "0%" at zero accepted; set_value_text for used / total comes next, then crw.20.
- Bug found by o55.7: chart/scale.rs LinkSpeed and AutoRate bits treat byte-per-second samples as bits, so a full link reaches only 1/8 of the height. Fixed under o55.7 in scale.rs (samples stay bytes, axis text in bits); the yrf.6 tests are updated. value_sum states (d940a09f): no valid sample yet shows ..., a gap shows -, and ! comes from the chart state.
- hk6.9 closed (abec715a, f7b1d4c1, b93a0f7a, 7cf9fc7e, d1e31084, fb83e37a): counted texts through tr_args. hk6.12 waits on hk6.11. The same agent takes pdu.2.
- 9o0.36 closed (f6a0347f): threads from /proc/loadavg; also fills the Performance CPU Threads row. 9o0.27 closed (306ae4c4, 5bfe4322, ea4710d3). journal follow_every_line_once was flaky once, so a new bug bead covers it. ui_chart.rs:1284 breaks with o55.9 set_value_text. The 9o0 agent takes xdh.3.
- pdu.2: TlMenu::tooltip for enabled items and submenus (components/menu.rs); row_menu and priority_submenu take window and cx; CapturedTarget gains ppid, nice and exe_deleted; clipboard_write always returns Ok(()) (R-199) and is used by the processes and information copy paths.
- pdu.2: the group header menu dispatches ToggleGroupRow { node } with the header node, because a header right-click keeps focus elsewhere. Space and Enter still toggle the focus row.
- xdh.3 closed (bfe570ac): strict process strings so parse and format round-trip; an unknown column or colour falls back with a warn and only an unknown source drops the card; a taken slot moves to the previous slot + 1 and compact_slots closes the holes. The same agent takes xdh.4.
- wlf.11 and wlf.12 closed (5c75f303, b3ed6963, 1556da1d, cfad4ee0). First real bench runs under load average 15 to 32: static 0 frames at 0.29% CPU; perf-cpu 24 fps, frame CPU p50 9.6 ms, p99 27 ms, 34% process CPU, RSS 241 MB; bridge latency p50 15 ms, p99 50 ms. These need a quiet-machine rerun. The first perf-cpu run drew 0 frames after a 250 ms stall, which looks like zed#64499; the evidence is on wlf.20. measure() will fail a non-static scene with 0 frames and report warm-up stalls. The same agent takes wlf.10, wlf.17, then wlf.20, and stops before vendoring (wlf.21).
- crw.17: right-click waits for crw.13; each tile ChartView paints its own heading and value, so the summary renders only on enter, resize and exit; legends for CPU Total/Kernel, Memory Used/Committed, Network Receive/Send only; the summary sends its own demand part with CPU at 10 Hz (PERF-010), and the hidden device page withdraws its part.
- bj6.7 closed (95d28051): Settings menu. The same agent takes bj6.21.
- bj6.21 closed (173724be, 8d4597e2): PageBuilder pub; no SettingsInputs; all_pages() empty until the page beads; test-hooks set_bool. The m1f agent takes bj6.22. The o55.9 agent takes o55.3 before crw.20.
- o55.11 closed (ac770c1d, 524b5356, de37c912, 184b5e0b): all device pages open through the real PerformancePage with demand checked. o55.9 follow-up cb1ec844, 7f5450b8, eec88ef8: ChartView::set_value_text, GPU memory shows used / total.
- xdh.4 closed (a2f2c0c9): MinigraphSource serialises as its state.toml string with a hand-written JsonSchema, because tl-model has no schema derives (ADR-001). Full tl-ui clippy green at that point. The 9o0 agent takes pdu.3.
- pdu.2 closed (1ca16929, 5afb432c, e3ffe268, 07adcd63, 4fd77b16): process row and group header menus with all P1 items, TlMenu::tooltip, ToggleGroupRow, test-only TlMenuPopup::entries_for_test. The keyboard menu still shows Copy, so new bead pdu.53 changes context_menu to take Option<Column>; the same agent takes it.
- wlf.10 (99a2e6e7, 4578b675, 365cdff0, d490aee9, 3d6bd5a0): table-2000 scene with 9 columns; Power appears at SHIPPED_PHASE 2. Correction: the 0-frame runs were the locked screen, not zed#64499. The bench will refuse on a locked session and fail on 0 frames. Gate risk: table-2000 RSS 192 MB with no frames drawn (gate 150 MB), measured under load. Real bench numbers and the soak need an unlocked session.
- pdu.3 (618a2270 and a follow-up): the jump sends one expand-path query and the engine view decides between found, hidden (toast with Show) and not found; no copy of the hide or filter rules in the UI. ee4c1a32 (crw.12) broke HEAD at sidebar.rs:891.
- bj6.22 closed (dc306abe): General and Units pages; SHIPPED_LANGUAGES shared as pub(crate); PageBuilder::description plus test-hooks readers. Leader fixed the crw.12 sidebar.rs E0502 in 9bd5107e. pdu.53 (72ef55f7) tests do not compile yet. crw.17 committed (c86ce530, 825410fd), tests pending. The m1f agent takes bj6.24, then bj6.25.
- o55.7 scale fix 54e1303e: LinkSpeed y_max = bps / 8, AutoRate bits ceiling on vmax * 8, full-link tests for the page and the sidebar card. crw.12 closed (ee4c1a32, 9bd5107e, 4e47897c). crw.17 tests fail: the summary re-renders 4 times per sample tick, and Ctrl+1 is not blocked; sent to the owner. ui_chart thirty_two_charts_one_frame_request was flaky once. The gma agent takes gma.19.
- crw.17 closed (c86ce530, 825410fd, c68c0756): Graph Summary View with CPU at 10 Hz and its own demand part; NoOp handler so the tab keys are really swallowed. Finding: a ChartView notify marks every ancestor dirty, so each sample re-renders the summary, the sidebar and the device pages; new P1 bead crw.34 for a paint-only chart update path. The missing fade and hidden title-bar menus moved to crw.35. The crw agent takes crw.34, then crw.35.
- Cause of the stray staged copy_rows.tsv: the pre-commit hook strips trailing whitespace and re-stages, which removed the TSV trailing tab; a4c7a02a and d161ba33 came out empty. 6136592b exempts crates/tl-ui/tests/golden/*.tsv and *.txt in .gitattributes and commits the golden with its tabs. pdu.3 closed (618a2270, 4b1e77be, f614b8dc). o55.3 closed (867dfcb3, ecc3fdce, ce1e85ac, 36c2d4d0): CompositionBarView is its own cached view. The 9o0 agent takes j185i.
- pdu.53 closed (72ef55f7, b3b5d9ab). crw.17 option (a): GPUI marks ancestors dirty on a chart notify; crw.34 is now a measurement and an investigation, and a GPUI patch is an owner decision. gma.19: no Deadline (new bead), no bold main thread or column minimums (new table bead). wlf.10 accepted with 9 columns. The pdu agent takes pdu.32.
- crw.34 closed as measured: a CPU tick in summary mode costs about 170 µs (p50, 9 tiles, the same as a full refresh); gpui-pre 0.3.7 has no paint-only path (reuse_paint copies cached scenes, request_animation_frame notifies). No GPUI fork for about 1.7 ms per second; revisit only if the wlf.18 gate fails.
- pdu.32 closed (584ce381): dim zero values as a text-colour multiplier only. Queued: pdu agent pdu.23 then gma.7; crw agent crw.35 then crw.13; o55.9 agent crw.20 then o55.4; 9o0 agent j185i then xdh.5; bench agent wlf.1 (P0) then wlf.17.
- bj6.24 and bj6.25 closed (1b1d8ff8, 731937d9): Motion and Processes settings pages; system_reduce_motion added so the banner does not blame the system when Animations is off. 731937d9 swept the gma.19 pwn-threads strings (lines only added, owner told). The m1f agent takes bj6.8, then bj6.26.
- j185i closed (d57fde9a): a timing flake, no loop bug; tests wait for lines plus quiet, spacing moved to a synthetic-clock test; 200 of 200 under CPU load.
- xdh.5: AC 5 restated after crw.34: the card never notifies on equal value bits; a snapshot with no change to its series causes 0 renders; a same-value sample re-renders only through the chart.
- bj6.8 closed (3c7f5a5e, 4c9849ad): Crash Report waits for the main window to become visible; about.rs send_open_uri is pub(crate). ad375964 (pdu.23) swept the bj6.8 ftl strings. shortcuts_cover_all_bindings fails for the Information bindings from 9o0.24; the 9o0 agent is adding SHORTCUT_DOCS entries.
- bj6.26: GPU engine list for Settings comes from a new pub gpu::device_engines(snap, device), so Settings and the GPU page share one key list; the page has 19 keys (the bead said 21).
- bbcd999d (gma.7) swept the bj6.26 ftl block, because blocks appended at the end merge into one hunk. The brief now says to add keys next to their area and to check every staged key. pdu.23 (ad375964, 13ef099b): result_toast shows the reason alone when the failure text is empty. gma.7 (bbcd999d) tests are in ui_process_menu.rs. Both wait on the build (minigraph card.rs WIP).
- gma.19 (905631a8): Threads tab; cell text through CellTextCache; follow-ups gma.42 and rsf.28. settings/mod.rs:351 type_complexity sent to the m1f agent. The gma agent takes gma.32.
- pdu.23 (ad375964, 13ef099b) and gma.7 (bbcd999d, be128f80) closed: tests pass, clippy clean. The pdu-52 agent takes pdu.24.
- crw.35 (9d8aaede, 955f45cb) closed, with the titlebar.rs and actions.rs edits accepted. crw.34 closed without a fork: 170 µs per sample, 176 µs full refresh. Revisit only if the wlf.18 gate fails. The crw agent takes crw.13.
- crw.20 (c958c890, a1fd8e39): in the overlaid and stacked modes the value label shows overall CPU, through set_value_text, in a follow-up commit. The o55 agent then takes o55.4.
- bj6.26 (1eaeef5d, 95d57919) closed. Network adapters is its own group with no indent. New key shell-settings-perf-no-gpu. The ftl block stays where it is. The m1f agent takes bj6.20.
- wlf.10 closed. The lock check is in aa291812. wlf.17 code is in 536e7d1f and 66855ba2, and the Soak variant is approved in args.rs. wlf.17 AC1 and wlf.1 wait for an unlocked session. The pdu-51 agent takes wlf.16.
- xdh.5 (7f124e49, 71dbe3e2, 9ad6d01c): AC 5 accepted as restated (no render from other sources; the card re-renders only through gpui ancestor dirtying). The card draws its own value. The net card uses AutoRate. The 9o0 agent does the SHORTCUT_DOCS fix, then jqk.20.
- pdu.24: TlTreeDelegate gets a defaulted empty_clear_filter() so the no-match state draws TlEmptyState with Clear filter at top 96. Chosen over a page overlay, which would re-render the page.
- crw.13 (64428080, 786505d2, 8443afd1, b6033ff5) closed. The trailing separator is dropped before P3. The crw agent takes crw.14, and summary tile right-click moves to the page menu, as UI-PRF-016 says.
- gma.32 (19f48947, d861ad05) closed. Gaps: Browse has no file-type filter (gpui), the history popup is a deferred panel, and the result comes from ActionTracker Finished. The gma agent takes gma.14.
- crw.14: the summary tile menu shows only the common items (Copy, Graph Summary View, Time range). Page items need the page to be rendered, and summary mode does not render it.
- bj6.20 (242ac452, 8e8bb580) closed. TlDialog is 420 px wide, not 460. open_or_focus is deferred. The fetch is a closure passed by the caller. The m1f agent takes bj6.11.
- gma.14: kv_grid gains opt-in TlValue.copy, menu_items() and cursor(), so General keeps using the grid and does not copy its rows. The jump uses ProcessesPage::select_by_key with the Go to parent texts.
- gma.14 kv_grid hooks (80184733): copy_texts(row, text), not a TlValue field, because details.rs builds it as a struct literal. on_context_menu(row, pos) takes over right-click from the kit menu. cursor(row).
- Owner, 2026-10-07: new Performance page "Overview" in btop layout (CPU, MEM, GPU, DISK, NET boxes on one page), drawn in the tasklens theme. It is the first sidebar entry. Summary View stays. The boxes come from a small layout model so a later editable layout can use it. A planning agent is drafting the beads.
- pdu.24 (a3db9cb4, ed170588, 083b44c5) closed. A FilterChanged with the same lowercased text sends no query, which keeps the jump Show path working. The pdu-52 agent takes pdu.27.
- crw.14 (b63b6ffe, eeb687aa, 1066362d) closed. Time range offers 30 s and 60 s only. Summary tiles get the common page menu. A page with no items of its own shows no separator, because TlMenu drops a trailing one. The crw agent adds the SHORTCUT_DOCS entry for PerfOpenCardMenu, then takes crw.21.
- pdu.27: one Ctrl-pause rule, in MainWindow::on_modifiers. It starts on Ctrl alone (no Shift, Alt or Super) while the window is active and ends on Ctrl up. ProcessesPage follows PauseChanged. UI-PRC-017 wins over E20-012.
- bj6.11 (7ef4f73b) closed. caption = WINDOW_TITLE. keep_above_supported checks the KWin bridge only. The m1f agent takes gma.3. bj6.27 is held until the Overview sidebar entry is planned.
- o55.4 (6e0a779d, b7e20325, 0388d59e, 14ce54e7, 098cf881) closed. The o55.12 row keys already exist. The crw.20 value label is in 730a7458 and e75d8cac. The o55 agent takes o55.6.
- Overview page plan: 8 beads under tasklens-crw (layout model, drawing parts, CPU and MEM boxes, GPU, DISK and NET boxes, page shell, sidebar entry, wiring, perf-overview bench).
  - The render contract follows crw.34: the page never notifies on a tick and untouched boxes build nothing. No GPUI fork.
  - cpu_fast is on. Overview is the first card and can be moved. F1 is Overview and F2 is CPU.
  - New profiles still default to "cpu". The page shows the visible GPU and disk cards only.
  - Summary has no Overview tile, and the page has no header row.
  - The fixed values go in D-022. The layout editor and its persistence come later. bj6.27 depends on the sidebar entry bead.
- gma.3 (f17100b3, 65b514cf) closed. The engine computes the hashes and opens the browser (ComputeHashes, OpenUri), so tl-ui has no network code. The m1f agent takes Overview bead 1.
- crw.21 (024f61a6, cebd603a, 6c967db3) closed. The core marks switch to our own CpuPCore and CpuECore icons, to match the affinity dialog, in a follow-up. The preferred-core star appears only when the ranks differ.
- Overview beads filed as crw.36 to crw.43 (fd5ef0a0). Owners: m1f takes crw.36 then crw.39, crw takes crw.37 then crw.38.
- o55.6 (f8eef0b3) closed. Rows come from Snapshot.system DiskTotals, and capacity from DiskTotals.capacity. Tests are in ui_disk_page.rs. The o55 agent takes o55.8.
- wlf.1 closed. Unlocked run, load 9 to 11:
  - perf-cpu: frame CPU p50 8.99 ms, p99 24.4 ms, 33% CPU.
  - bridge: p50 8.5 ms, latency p99 51 ms.
  - table-2000: p50 4.0 ms.
  - static: 0 frames but 21.9% CPU.
  - The moving scenes run at 24 to 31 fps.
  - All of these are far above the UI-PRF-053 gates. Filed as P0 wlf.27 (static idle CPU) and wlf.28 (frame cost and pacing), both for the pdu-51 agent after the soak.
- crw.36 (314825d8, 2559ac5a) closed. Stacked content_h is 1284, with an 8 px gap. The crw agent starts crw.37. The m1f agent takes bj6.12 while it waits for crw.39.
- o55.8: the Display unit menu opens through a new opt-in TlKeyValueGrid::on_link_click(row, bounds) and a TlLink variant that passes the click bounds, so the menu sits under the value for mouse and keyboard. Chosen over a new action that would open at the pointer.
- bj6.12: TlMenu gains a slider_row entry and a keep_open flag on check_item, in components/menu.rs. Existing menus behave the same.
- wlf.17 soak passed: 600 s, 3 windows, 0 stalls, 35,846 frames, 23.7% CPU, 325 MB RSS, load 10. The zed#64499 patch (wlf.21) is not needed. The pdu-51 agent writes up wlf.20, then takes wlf.27 and wlf.28. wlf.16 waits.
- jqk.20 (42c07173, d231cd60, 6c2976d6) closed. --start-minimized only counts under TitleBarProbe in tests. The 9o0 agent takes xdh.6.
- TMOG research is in docs/research/tmog-inspiration.md (0f3a76f7). Filed wlf.29 (hover snap), pdu.54 (sort on the shown value), wlf.30 (a blocked source must not stall the others) and crw.44 (title click). Comments added to crw.38, crw.39 and crw.43. Owner questions: pixel pacing (an ADR-009 change), the Top processes box and the battery saver.
- Owner, 2026-10-07: pixel-paced scrolling to be prototyped behind a flag that defaults off, then compared side by side at 144 Hz (wlf.31, after wlf.28). An opt-in Top processes box (crw.45, after crw.42). "Save power on battery" on by default (wlf.32).
