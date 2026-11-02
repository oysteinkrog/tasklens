# tasklens crate split plan

Status: planned 2026-11-01. Oystein approved the full split, to run before the phase 3 widget work
(epic tasklens-hzire, on hold). Beads: 49, keys CS-A1 to CS-I3 (section letters below). Rules for
the new layout go in ADR-020 (bead CS-A1).

## 1. Goals and baselines

The goal is a fast edit loop for lanes and for Oystein: an edit rebuilds and tests only the crate
it touches, and tl-ui is compiled once per edit, not twice.

Measured on 2026-11-01 (FINDINGS.md section 1, loaded machine, so trust ratios more than seconds):

| Edit | Command | INCR=0 | INCR=1 |
|---|---|---|---|
| leaf file comment | `check -p tl-ui` | 23 s | 7 s |
| leaf file comment | `build -p tl-app` | 122 s | 16 s |
| leaf file comment | `test --no-run` (ui binary) | 259 s | 23 s |
| leaf body | `build -p tl-app` | 125 s | 24 s |
| leaf body | `test --no-run` | 223 s | 34 s |
| tl-model edit | `build` (9 crates rebuilt) | 141 s | 17 s |
| new test fn | `test --no-run` | 85 s | 37 s |
| components/badge.rs | `test --no-run` | 186 s | 71 s |

Other baselines:

- Cold `cargo build -p tl-app`: 381 s, 873 units. tl-ui alone 95 s.
- tl-ui: 222k source lines in one crate; 142.4k test lines in one `ui` test binary (80 files, 366 MB).
- tl-ui is compiled twice per edit: once without `test-hooks` (app build) and once with it (tests).
  gpui-kit `test-support` is a dev-dependency feature, so the GPUI stack also has two variants.
- Lane time in a 40.8 h window: 6.14 h compiling test builds, mostly tl-ui and the `ui` binary.
- Last full suite before the split: 7,163 tests passed (leader-notes, 2026-11-01).

## 2. Target crate graph

Twelve new crates. The workspace grows from 14 to 26 members.

| Crate | Holds (paths are today's, under crates/tl-ui/src unless noted) | Lines (about) |
|---|---|---|
| tl-ui-settings | UI half of tl-model `settings.rs`, plus `layout.rs`, `layout_boxes.rs`, `layout_pages.rs` (from crates/tl-model) | 7.5k |
| tl-ui-base | theme/, text/, i18n.rs, template.rs (Template), selector.rs, util.rs, focus.rs, anim.rs, motion, hub.rs, demand.rs, settings_global.rs, host.rs, host_settings.rs, hosts_api.rs, hosts_edit.rs, action_tracker.rs, registry.rs, app_env.rs, shell.rs (shell types and ShellApi), toast_types.rs, engine_link.rs, request_ends.rs | 14k |
| tl-ui-kit | components/, helper_flow.rs, actions.rs | 22k |
| tl-ui-chart | chart/, net_totals.rs | 9k |
| tl-ui-table | table/, text_cache.rs, sys_table.rs, sys_status.rs, sys_copy.rs | 10k |
| tl-ui-perf-core | pages/performance/core/ (DeviceKey, DeviceKind, perf actions, details, core_clocks, shared consts and helpers moved down), pages/minigraph/, tray_bridge.rs, windows/detached_graph.rs | 10k |
| tl-ui-overview | pages/performance/overview/, pages/performance/widget/, memory_apps.rs | 40k |
| tl-ui-perf | the rest of pages/performance/ (PerformancePage, device pages, page_flow, aggregate, card, sidebar, status_bar, menu), windows/mini.rs | 31k |
| tl-ui-procs | pages/processes/, windows/{props/, app_props, affinity, priority, signal, handle_search, notices, actions, disk_in_use, frame} | 43k |
| tl-ui-sys | pages/{services, connections, startup}/, windows/{service_properties/, connection_properties, startup_properties, confirm_startup_delete} | 17k |
| tl-ui (shell) | window/, app.rs, pages/information/, windows/{run, run_history, low_memory, smart}, bench/ | 32k |
| tl-ui-testkit (dev) | FakeEngine; render helpers behind feature `render` | 0.8k |
| tl-ui-pagehost (dev) | a host that mounts one page with its globals, without MainWindow | 0.5k |

CS-C6 (tasklens-zljr5.19) chose Option A: base stays below kit, as two crates. Option A changed
no behaviour and needed about 400 changed lines, under the bead's 600-line limit, so the merge
(Option B, one tl-ui-kit of about 36k lines) was not needed. With two crates a component edit
rebuilds about 22k lines instead of 36k. Motion moved to base as `motion.rs`, the toast data types
to base as `toast_types.rs`, and the action tracker reaches the toast queue and confirm dialog
through the base trait `TrackerUi`. actions.rs moved to kit, because `bind_keys_global` binds the
component keys and no base or kit file uses actions.rs. ADR-020 has the details.

### Allowed edges

Each crate may depend on the crates listed, plus tl-model and gpui-kit.

| Crate | May depend on |
|---|---|
| tl-ui-settings | (tl-model only) |
| tl-ui-base | settings |
| tl-ui-kit | base, settings |
| tl-ui-chart | kit, base, settings |
| tl-ui-table | kit, base, settings |
| tl-ui-perf-core | chart, table, kit, base, settings |
| tl-ui-overview | perf-core, chart, table, kit, base, settings |
| tl-ui-perf | overview, perf-core, chart, table, kit, base, settings |
| tl-ui-procs | perf-core, chart, table, kit, base, settings |
| tl-ui-sys | procs, perf-core, chart, table, kit, base, settings |
| tl-ui | all of the above |
| tl-ui-testkit | (tl-model and gpui-kit only) |
| tl-ui-pagehost | testkit, kit, base, settings |

Page crates never depend on each other, except tl-ui-perf on tl-ui-overview and tl-ui-sys on
tl-ui-procs. The ADR-001 closure rule for tl-ui (no tl-engine, tl-collect or tl-ops; arc-swap and
zbus only through gpui-kit) applies to every tl-ui-* crate. tl-app keeps depending on tl-ui only;
the shell re-exports what tl-app needs.

```text
tl-app -> tl-ui (shell) -> tl-ui-sys -> tl-ui-procs --------\
                        -> tl-ui-perf -> tl-ui-overview -----> tl-ui-perf-core -> tl-ui-chart -> tl-ui-kit -> tl-ui-base -> tl-ui-settings -> tl-model
                                                              \-> tl-ui-table --/
tl-engine, tl-server, tl-remote -> tl-model   (no longer rebuilt by a UI settings edit)
```

## 3. What FINDINGS.md got wrong or left out

Checked with grep on 2026-11-01 at commit e977d49c.

1. Test-hooks gates: 831 in 95 files now (FINDINGS: 822 in 94). Window: 206; chart 126; overview 104;
   device pages 102; windows/ 91; widget 42; processes 41.
2. Cycle 1 sites: theme/store.rs:36 is an edge to `components::motion`, not to app.rs. theme reaches
   app.rs only in test code (store.rs:618, preview.rs:407). "window_frame.rs:51" is
   components/window_frame.rs:51 (`window::registry`) and :322 (`app::window_app_id`).
   engine_link.rs:14 (`app::AppEnv`) was missing.
3. Cycle 2 is wider than `window::main`. Outside window/, code also uses window::registry (18 refs),
   status (9), setup (5), titlebar (4), settings, public_ip, menubar and toolbar. window::main is
   used by 42 files outside window/ (46 counting window/ itself).
4. A seventh loop was missing: settings_global uses host::HostContext; host holds an ActionTracker;
   ActionTracker uses components::dialog and components::toast; components use settings_global and
   host. FINDINGS put settings_global in base and host and action_tracker in kit, which is an
   upward edge from base to kit. CS-C6 broke it with Option A (section 2).
5. net_totals.rs uses chart::view::derived_view, so it belongs with the chart, not in base.
6. Cycle 5 is deeper than four helpers. Device pages build on widget (page_flow, aggregate, page,
   sidebar and menu use widget::{container, edit, grid, compose, source}), which is the allowed
   direction. Overview and widget name about 20 device-page items (gpu, cpu, core_clocks, memory,
   memory_apps, net, sensors, sidebar, status_bar::rules, page::PAGE_PAD, details, actions) and
   PerformancePage's DeviceKey family. memory_apps and overview use each other. aggregate.rs and
   card.rs cannot go to perf-core (they use widget and overview::parts), unlike the FINDINGS table.
7. Cycle 6 is between procs and sys, not between processes and its windows (those share a crate).
   The upward edges: processes/menu.rs:39 and windows/props/general.rs:78 use
   services::actions::{GoToService, JumpOrigin, ManagerArg}; props/fds.rs:46 uses
   connections::actions::GoToConnection; notices.rs:56 uses startup::delegate::home_tilde.
8. pages/processes uses pages/minigraph (MinigraphPanel, AddMinigraph actions, GraphColumn), so
   minigraph must sit below procs: it goes to perf-core, not to tl-ui-perf. minigraph/bridge.rs:22
   uses windows::mini::MiniWindow, an upward edge to cut (CS-C9).
9. The `testing` module (FakeEngine, render helpers) cannot simply move to a `debug_assertions`
   gate: render.rs needs gpui-kit `test-support`, which app builds lack. It must become its own
   crate first (CS-B1).
10. "A widget edit in components rebuilds kit and its tests instead of 2 x 222k" is true for
    `cargo test -p tl-ui-kit`, which is the lane loop. An app build after a kit edit still re-runs
    rustc for every crate above kit and relinks tl-app (incremental makes most of that cheap, not
    free).
11. check-deps: tl-ui depends on intl_pluralrules, which is not in the tl-ui ALLOWED row in
    xtask/src/check_deps.rs. By reading the code, `cargo xtask check-deps` should fail on main
    today (not run, to avoid a build). CS-A2 fixes the row.
12. Small: `pub(crate)` sites are 166 (FINDINGS 163), and there are 138 `pub(super)` sites, some of
    which will need widening too. Test lines are 142.4k (FINDINGS 140.7k).
13. A `[profile.test]` that differs from `[profile.dev]` may stop cargo sharing the library build
    between `cargo build` and `cargo test`. CS-H1 measures that before choosing.

## 4. Order and why

| Step | Beads | Why here |
|---|---|---|
| A. Decisions and checks | CS-A1 ADR-020 and doc rows; CS-A2 `cargo xtask check-layers` | Every later bead needs one written target and a machine check. check-layers starts with an allowlist of today's upward edges that may only shrink, so it passes on main from day one and each cycle bead proves its progress. |
| B. One tl-ui build | CS-B1 testkit crate; CS-B2 gate rule, audit and first files; CS-B3 to B7 one directory each; CS-B8 drop the feature from tests and docs; CS-B9 GPUI variant and feature-set decision | This halves the work per edit right away, before any crate exists. The testkit goes first because the `testing` module cannot use the new gate. B3 to B7 touch separate directories, so two lanes can run them at once. |
| D. tl-ui-settings | CS-D1 in-crate cut; CS-D2 crate move and path rewrite | D1 only touches tl-model and runs beside B. D2 rewrites about 438 import lines in 202 tl-ui files, so it runs alone, after B and before C. |
| C. Break the cycles | CS-C1 app types; C2 shell types and registry; C3 and C4 ShellApi; C5 Template; C6 base and kit loop; C7 table edges; C8 and C9 perf; C10 procs and sys; C11 remove shims, empty allowlist | All inside one crate, so every step is a normal refactor that compiles. Serial, because the beads share files (action_tracker.rs, window/status.rs, the page mod.rs files). |
| E. Extract crates | CS-E1 base (writes the extract script) to CS-E9 sys, bottom-up; CS-E10 shell surface, final rows, actions link test | Bottom-up so each new crate depends only on crates that already exist. Serial, because each edits the root Cargo.toml, tl-ui's lib.rs and check_deps.rs. |
| F. Tests | CS-F1 page host (inside the E chain); CS-F2 to F6 page tests to their crates (beside E); CS-F7 triage of the 39 shell-bound modules; CS-F8 and F9 convert process and system shell tests | Page tests move as soon as their crate exists. Shell-bound tests come last, when every page crate exists. |
| G. TlTreeTable | CS-G1 dyn core | Needs tl-ui-table and its users as crates to measure the drop; runs beside F. |
| H. Profiles | CS-H2 thin-LTO perf profile (early, beside B); CS-H1 opt-level choice (late) | H2 is independent and saves lane hours at once. H1 must be measured on the split tree with tests in their final binaries. |
| I. Close | CS-I1 measure; CS-I2 full suite; CS-I3 docs | Prove the gain, prove nothing was lost, and point lanes at the new layout. |

The lead's order put C before D. D2 moved ahead of C because D2 is a mass path rewrite that must run
alone; doing it before C means the C beads see final import paths and do not have to be redone.

Critical path: 37 beads (A1, A2, B1, B2, B3, B8, B9, D2, C1 to C11, E1, E2, F1, E3 to E10, F7, F8, F9,
H1, I1, I2, I3). The other 12 run beside it: B4 to B7, D1, H2, F2 to F6, G1. At about an hour per bead
including review, expect two to three nights, not one.

## 5. How main stays green

- **Re-exports keep paths stable during a move.** A moved module's new crate imports the lower
  crates' modules at its root (`use tl_ui_base::{theme, i18n};`), so the moved code keeps its
  `crate::theme::...` paths. tl-ui's lib.rs replaces `pub mod x;` with `pub use tl_ui_x::x;`, so
  upper code, tl-app and the tests keep `tl_ui::x::...`. An extraction is then `git mv`, two
  lib.rs edits, a Cargo.toml and the visibility fixes the compiler names. That fits in one commit.
- **Cycle beads leave re-exports at old paths** so each bead only rewrites the lower-layer users.
  CS-C11 removes them before extraction, because a lower crate cannot name an upper module.
- **One atomic commit per move.** A move that cannot be split into green steps goes in one commit,
  committed before any long build.
- **Shared hot files are serialized by dependencies.** The root Cargo.toml, crates/tl-ui/src/lib.rs,
  tests/ui/main.rs and xtask/src/check_deps.rs are each edited by one chain only. Beads that run
  beside each other touch separate files (B3 to B7: separate directories; F2 to F6: test files only,
  told not to touch the root Cargo.toml, tl-ui sources or check_deps.rs).
- **Exclusive beads.** CS-D2 and CS-C11 rewrite paths across many tl-ui files. They carry the label
  `exclusive`; the dispatcher must not run another lane beside them, because a pathspec commit of a
  file would sweep up another lane's uncommitted edits in that file.
- **Every bead runs** clippy and tests for the crates it touches, `cargo clippy -p tl-app` (until
  CS-B9 says otherwise), check-deps and check-layers.

## 6. Risks and how the beads guard against them

| Risk | Guard |
|---|---|
| A test-hooks hook changes app behaviour once it is compiled into debug app builds | CS-B2 sorts hooks into recorders, readers and switches; every switch must be inert until a test sets it. The list goes in ADR-020. Release builds are unchanged. |
| The test-hooks feature, if still enabled anywhere, recreates the second tl-ui build | CS-B8 removes it from tl-app's dev-dependency, tests, scripts and docs, and measures the compile count. |
| GPUI `test-support` keeps two variants | CS-B9 measures and sets the lane commands so lanes build one variant; the app variant is a leader-gate build. Test support stays out of release. |
| Feature unification per `-p` set rebuilds dependencies | CS-B9 measures which dependencies change; pins features in `[workspace.dependencies]` first; cargo-hakari only with Oystein's approval. |
| `actions!` in a crate that a binary does not link: keymap entries stop resolving | Each crate exposes init or bind_keys, called from app init; CS-E10 adds a test that every bound action name builds and every crate with `actions!` is initialised. |
| `#[cfg(test)]` helpers do not cross crates | The extraction method turns them into `#[cfg(any(test, debug_assertions))] pub` or moves them with their tests. |
| Unit tests in base using a testkit that depends on base would build base twice | tl-ui-testkit depends only on tl-model and gpui-kit; the page host is a separate crate used only from tests/. |
| A behaviour change hides inside a move | Beads move code only; a changed assertion means the move is wrong. Test counts are recorded per module before and after each move; CS-I2 compares the total with 7,163. |
| Upward edges creep back | check-layers inside tl-ui before the split; check-deps rows from the same table after it. |
| Too many crates slow clean builds (about 12 per app was an inferred limit) | 12 new crates, sizes 10k to 43k except the two small dev crates. CS-I1 measures cold build units and time against 873 units and 381 s. |
| More test binaries use more disk (one GPUI link each, about 300 MB) | One binary per crate (about 10), not one per file. CS-I1 records target/ size; the 80 GiB sweep cap stays. |
| Golden files are read by relative path | Test moves move owned goldens and use `CARGO_MANIFEST_DIR`; shared goldens stay and are named in a comment. |
| Phase 3 (hzire) beads name old paths | CS-I3 lists open beads with old paths for the leader to update. |

## 7. Lane commands

Test command: `heavy-build cargo test -p tl-ui --test ui -- <filter>` (no feature flag since CS-B8).
After CS-B8: no feature flag. After CS-B9: the aliases and the leader-gate rule it writes into
AGENTS.md. After extraction: `heavy-build cargo test -p tl-ui-<crate>` for the crate a bead touches,
plus `cargo clippy -p tl-ui-<crate> --all-targets -- -D warnings`. Never `--workspace`, never
`--release` in a lane (CS-H2 has one approved exception to measure the new profile).

## 8. Expected gains

These are estimates for the lane loop at INCR=1; CS-I1 measures the real numbers.

| Edit | Before | After (estimate) | Why |
|---|---|---|---|
| component body, then test that crate | 71 s (`test --no-run`, whole ui binary) | 10 to 20 s | `cargo test -p tl-ui-kit` builds a 22k-line crate and its own test binary; nothing above it. |
| Overview box body, then test | 34 s | 10 to 20 s | 40k-line crate and its test binary; procs, sys and the shell are not built. |
| new test fn | 37 s | 5 to 15 s | Links one crate's test binary instead of the 366 MB ui binary. |
| any tl-ui edit, app build plus tests | two tl-ui builds | one | Hooks on `debug_assertions`, no feature flip; one GPUI variant in lanes. |
| UI settings edit | 9 crates rebuilt | UI crates only | tl-engine, tl-collect, tl-ops, tl-history, tl-remote stay fresh. |
| table-heavy crates | about 17 copies of TlTreeTable | one core copy | dyn delegate (CS-G1). |
| bench builds in lanes | fat LTO, codegen-units 1 | thin LTO, 16 units | CS-H2; packaging keeps fat LTO. |

A full `cargo build -p tl-app` after a kit edit still re-runs rustc on every UI crate above kit; with
incremental most of that is reused, but it is not free. The big win is that lanes stop paying for it
on every loop.

## 9. Decided by Oystein (2026-11-01)

1. The workspace may grow from 14 to 26 members. CS-I1 measures the cost of a build from an empty cache against
   the baseline (873 units, 381 s).
2. `intl_pluralrules` joins the tl-ui row in ADR-001 and check-deps (in use since o55.37).
3. Test hooks are compiled into debug builds (`cargo run`, smoke harness) behind their runtime checks, never
   into release builds.
4. cargo-hakari is allowed if CS-B9 shows that pinning features in `[workspace.dependencies]` does not stop
   `-p` sets from rebuilding dependencies. The hakari crate must not give a tl-ui crate a direct asupersync or zbus
   dependency (ADR-001 rule 1).
5. Phase 3 (tasklens-hzire) starts once CS-E7 (tl-ui-perf) and CS-F5 have closed, not after the whole epic.
6. During the split, moved code keeps its old paths through root re-exports. CS-J1 rewrites them to direct
   crate paths after CS-E10 and CS-F9, before the final measurement.

## 10. Shell-bound tests

Triage for CS-F7 (tasklens-zljr5.41), at commit 6f2e93dd7. A module is shell-bound when it names
`MainWindow`, `TasklensUi`, `start_for_test`, `open_first_for_test` or `main_window`. 40 modules in
crates/tl-ui/tests/ui match (the bead counted 39). For each one: does it test the shell (tabs,
menus, title bar, windows the shell opens, shortcuts, hosts, startup), or does it reach a page
through the shell only?

Verdicts:

- **stays**: the tests check shell behaviour, so they stay in the tl-ui `ui` binary.
- **convert**: move to the crate named, on the tl-ui-pagehost rig.
- **split**: move the page tests to the crate named and keep the shell tests in tl-ui. Split by test,
  not by file.

The target crate is the lowest one whose dependency closure covers every path the moved tests use.
Effort: S under an hour (a rig swap); M a rig swap plus tests to split; L a large module or many
tests tied to the shell. Test counts are `#[test]` and `#[gpui_kit::test]` attributes. Rows other
than overview, overview_range and performance were read but not converted, so their counts and
splits are estimates for the bead that does the work.

| Module | Lines | Tests | What it uses the shell for | Verdict | Effort | Bead |
|---|---|---|---|---|---|---|
| action_links | 244 | 2 | `TasklensUi::init`, to check that every keymap action resolves; a source sweep of app.rs | stays | S | |
| actions | 1,330 | 36 | Only `with_shell` (1 test) opens MainWindow with fake pages. The other 35 test tracker toasts and dialogs in a plain window | split: 35 to tl-ui-kit, 1 stays (MainWindow and status events) | S | none yet |
| connections | 3,608 | 80 | MainWindow on the Connections tab as page host and toast sink. 4 tests need the tab switch or the jump to Processes | split: 76 to tl-ui-sys, 4 stay (connections_page_registered, tab_mounted_and_kept, demand_cleared_on_tab_switch, end_to_end_jump) | M | zljr5.43 |
| demand | 340 | 9 | Nothing. It matched on `DemandAggregator::set_main_window` | convert to tl-ui-base, with no page host (plain window) | S | none yet |
| detached | 1,827 | 34 | Own rigs without the shell, except 2 tests that open the main window from a mini window card and keep detached windows when the main window closes | split: 32 to tl-ui-perf, 2 stay | M | none yet |
| dialogs | 914 | 28 | `TasklensUi::init` only to install globals. Tests cover window frames, TlDialog and the window registry | convert to tl-ui-procs (one test uses `windows::frame`; the rest need only kit and base) | S | zljr5.42 |
| dialogs_process | 3,807 | 62 | `TasklensUi::init` in 5 rigs; one test opens MainWindow on Processes as a bystander | convert to tl-ui-procs | S | zljr5.42 |
| focus | 402 | 7 | `esc_chain` (5 tests) builds MainWindow for the Esc chain and Tab order | split: 2 to tl-ui-base, 5 stay | S | none yet |
| handle_search | 823 | 15 | `TasklensUi::init` and MainWindow on Processes; one test reads the ProcessesPage through the shell | convert to tl-ui-procs | S | zljr5.42 |
| helper_flow | 323 | 9 | MainWindow on Processes as the requester window and the fallback toast sink | convert to tl-ui-procs (toast checks read `ShellLog`) | M | zljr5.42 |
| history | 3,290 | 82 | `row_actions` uses MainWindow only as toast sink. `mode` toggles history mode through MainWindow and reads the tab strip label. Also settings window, bench and filter box tests | split: 69 to tl-ui-procs, 13 stay (mode 6, settings window 4, bench 2, filter box 1) | M | zljr5.42 |
| host_clock | 444 | 9 | One test opens Collector Health; one is a workspace source sweep | split: 5 to tl-ui-base, 2 to tl-ui-chart, 2 stay | S | none yet |
| host_manage | 478 | 6 | Settings > Hosts editor, host tab context menu, MainWindow | stays | S | |
| host_settings | 231 | 4 | Settings window pages, `menus_settings::refresh_marks`, persist hook | stays | S | |
| host_tabs | 918 | 9 | Host tab strip, Ctrl+digit, window title, pages per host, reconnect | stays | S | |
| hub | 687 | 23 | 16 tests check `TasklensUi::init`, `open_first_for_test`, `start_for_test` and the exit slots; 7 are plain SnapshotHub tests | split: 7 to tl-ui-base, 16 stay | S | none yet |
| information | 2,907 | 77 | The Information page is in the shell crate. `main_window` (8) and the keymap test use MainWindow | stays (the page itself is in tl-ui) | S | |
| low_memory | 1,261 | 26 | The Low memory window is in the shell crate; `start_for_test`, `OpenMode`, the Settings window | stays | S | |
| minigraphs | 3,202 | 65 | `mounted` (7) opens MainWindow with the real Processes page; one test switches tabs. The other 58 have no shell | split: 64 to tl-ui-procs (about 17 need the Processes menu), 1 stays (panel_absent_on_other_tabs) | M | none yet |
| overview | 12,202 | 171 | Only `wired` (19) opens MainWindow: tab leave, the cached Processes page, the title bar. `page` (84) needs the device sidebar. The other 68 need no shell | done: 68 to tl-ui-overview, 84 to tl-ui-perf, 19 stay | L | zljr5.41 |
| overview_range | 982 | 10 | MainWindow mostly as host for the Performance page. One test measures box chrome at a window size, so it depends on the page area the main window gives | done: 9 to tl-ui-perf on the page host, 1 stays (overview_box_chrome_at_most_40). page_range (1) moved with the rig | S | zljr5.41 |
| performance | 3,497 | 65 | `summary` (38) and one `tab` test open MainWindow; the rest need no shell | done: 26 to tl-ui-perf, 39 stay (see below) | M | zljr5.41 |
| process_menu | 4,603 | 77 | `TasklensUi::init` for globals; `end` and `toasts` open MainWindow on Processes for the page and its toasts | convert to tl-ui-procs | M | zljr5.42 |
| processes | 4,516 | 89 | MainWindow for the filter box, `FilterChanged`, the pause state, the status chip, tab visits, the toolbar and a fake Performance tab | split: 42 to tl-ui-procs (top 8, delegate 15, page 14, keys 5), 47 stay | L | zljr5.42 |
| props | 6,260 | 135 | `window` (77) uses one rig with `TasklensUi::init` and MainWindow on Processes, for the page and the main toast queue | convert to tl-ui-procs | M | zljr5.42 |
| run | 449 | 11 | The Run window is in the shell crate; MainWindow as parent and toast sink | stays | S | |
| selectors | 372 | 5 | Opens MainWindow with the real Processes and Performance pages and switches tabs | split: 1 to tl-ui-procs, 1 to tl-ui-perf, 3 to tl-ui-base | M | none yet |
| service_props | 2,886 | 64 | One rig: `TasklensUi::init`, MainWindow on Services, the main toasts | convert to tl-ui-sys | S | zljr5.43 |
| services | 4,962 | 98 | MainWindow on Services as page host and toast sink. 4 tests check lazy page creation, toolbar slots, the status chip and the key binding at init | split: 94 to tl-ui-sys, 4 stay | L | zljr5.43 |
| settings_window | 2,680 | 88 | The Settings window, persist counters, import, menus | stays | S | |
| setup_window | 797 | 25 | The Additional Setup window | stays | S | |
| shell | 5,992 | 198 | Toolbar, title bar, tab strip, status chip, filter, MainWindow | stays | S | |
| shell_windows | 2,580 | 65 | Share Theme, Collector Health, Shortcuts, About, Reset, Crash report, Public IP, Theme editor | stays | S | |
| shield | 1,015 | 24 | The shield in MainWindow, the Settings Privileges page, unlock at launch | stays | S | |
| smart_window | 378 | 7 | The SMART window is in the shell crate; MainWindow on Services as host and toast sink | stays | S | |
| startup | 3,079 | 68 | MainWindow with a fake page slot. 3 tests need the shell: the keymap, mount and keep on the tab, demand cleared on tab switch | split: 65 to tl-ui-sys, 3 stay | M | zljr5.43 |
| status_bar | 622 | 7 | `performance_tab` (5) and `overview` (1) already use the page host. `main_window` (1) checks that every tab sits under the tab strip | split: 6 to tl-ui-perf, 1 stays | S | none yet |
| sys_table | 2,049 | 44 | Only `frame` (6) calls `TasklensUi::init`, to install globals, in its own window. No MainWindow | convert to tl-ui-table | S | none yet |
| theme | 207 | 5 | One test boots `TasklensUi::init` to follow the portal color scheme | split: 4 to tl-ui-base, 1 stays | S | none yet |
| tray_bridge | 1,505 | 44 | `full()` runs `TasklensUi::init` and MainWindow so toasts have a queue. 8 tests check the shell: the open main window, close and minimize to tray, the Settings tray rows | split: 31 to tl-ui-perf-core, 5 to tl-ui-procs (the row menu), 8 stay | M | none yet |

Totals: 18 modules stay, 9 convert and 13 split (3 of them done here). The two follow-up beads
cover the procs and sys rows. The rows marked "none yet" (base, kit, chart, table, perf-core and
perf targets) have no bead; they are small and could go in one bead per target crate.

Open points for those beads:

- `TasklensUi::init` installs more than `PageHost::install`: the toast queues, `bind_keys_global`,
  the window frame keys (Esc, Ctrl+W), the helper flow and the action tracker. dialogs, helper_flow,
  process_menu, props, service_props and tray_bridge depend on them. Either `PageHost` gains them or
  each rig adds them.
- `ShellLog` records each toast push, while the main toast queue merges repeats. Tests that count
  toasts in process_menu, props, helper_flow and tray_bridge must keep their counts on the new rig.
- The processes split is the least certain: filter, pause, status and jump may move once the page
  host carries `GlobalShellEvents`; `legend` and `top_procs` cross from the Overview to the Processes
  tab on purpose.

### What zljr5.41 moved

Counts from `cargo test -p tl-ui --features test-hooks --test ui -- --list` before the move
(2,127 tests at 6f2e93dd7) and from each binary after it.

| Module | Before (ui) | After | Stayed in tl-ui, and why |
|---|---|---|---|
| overview | 171 | tl-ui-overview `overview` 68 (top level 10, drawn 3, boxes 18, bottom 30, title_row 1, sweep 3, mem_by_app_box 3); tl-ui-perf `perf` 84 (page) | wired 19: opens MainWindow, leaves and revisits the tab, reads the title bar and the cached Processes page |
| overview_range | 10 | tl-ui-perf 9, on the page host | overview_box_chrome_at_most_40: on the page host the page is taller than in the main window, and the GPU box chrome came out at 41 px against the 40 px limit |
| page_range | 1 | tl-ui-perf 1 (uses the overview_range rig) | none |
| performance | 65 | tl-ui-perf 26 (top level 4, page_frame 10, perf_keys 6, tab 6) | tab 1 (performance_tab_initial_focus_is_sidebar: first visit through MainWindow); summary 38: the rig opens the main window, and tests check the tab switch block, the title bar menus, the main toasts and keys through the window |

Shared helpers moved rather than copied: `drawn_selectors` and `assert_none_drawn_twice` to
`tl_ui_pagehost::selectors`, and `proc_key` and `proc_record` to `tl_ui_testkit::fixtures`.
tl-ui keeps the main-window version of the overview_range rig for the chrome check,
overview_timeline and device_pages, so that rig is now in both binaries.

overview_timeline (8 tests) is not in the list of 40, but it builds on the overview_range rig. It
stays in tl-ui: `timeline_click_modes` checks that a timeline click opens History mode, which goes
through `ShellApi::process_jump` to MainWindow. The other 7 could move to tl-ui-perf with a split.

The summary module is the largest page-test group left in tl-ui. About 30 of its 38 tests check the
summary view itself and could move to tl-ui-perf on a page host rig that binds the same keys. That
needs the rig to reproduce the key dispatch and the window size the main window gives the page.
