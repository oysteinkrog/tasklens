# wlf.36 design: cached sidebar cards (GPT-6 Astra, 2026-10-14, adopted by the leader)

The blocker is confirmed. One correction: **a CPU-total change affects both CPU and Overview sidebar cards**, because both display that series (`sidebar.rs:285–291`; [D-022](/home/oystein/work/tasklens/docs/DECISIONS.md:17)). Exactly one requires a fixture hiding Overview, or a CPU-kernel-only change.

Paths below: `U` = `crates/tl-ui`, `P` = `U/src/pages/performance`, `G` = `/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.7/src`. Proposed behavior is labelled **design inference**. No edits, commits or builds were performed.

1. GPUI caching works as follows:
   `gpui-kit 0.7.0` resolves to `gpui-pre 0.3.7` (`Cargo.lock:2933,2962`).
   Cached views take their layout from the supplied style, without rendering their contents ([G/view.rs:432](/home/oystein/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.7/src/view.rs:432)).
   Reuse requires existing state, equal bounds, content mask and text style, a clean entity, and `!window.refreshing` (`G/view.rs:484`).
   A hit reuses prepaint state and accessed entities; paint calls `reuse_paint`, skipping the subtree’s element paint methods (`G/view.rs:491,568`).
   A miss sets `window.refreshing = true` around rendering/layout/prepaint, forcing descendant cache misses; painting the rebuilt subtree also sets it (`G/view.rs:501,564`).
   `notify` invalidates displaying windows; frame invalidation marks the notifying view and its dispatch-tree ancestors dirty, not its siblings (`G/app.rs:2794`; `G/window.rs:3427,2148`).
   `Window::refresh()` bypasses caches globally; an uncached parent’s ordinary render does not set that flag (`G/window.rs:2272`; `G/view.rs:438,473`).
   “Not painted” means no subtree paint callbacks: cached primitives still replay into the next scene (`G/window.rs:3879,3917`).

2. The exact defeating chain is: CPU `ChartView::on_snapshot → cx.notify()` (`U/src/chart/view.rs:137–149`) → ancestor dirtiness → cached **PerformancePage** misses at [main.rs:2323](/home/oystein/work/tasklens/crates/tl-ui/src/window/main.rs:2323) → `refreshing=true` → cached **DeviceSidebar** at `P/mod.rs:938` and selected page at `P/mod.rs:960` miss → every descendant cache misses.

   There is also a direct path: changed card text → `refresh_lines` → sidebar `cx.notify()` (`P/sidebar.rs:819,858–860`). Sidebar render reconstructs every `DeviceCard` (`P/sidebar.rs:1099–1119`); those are `RenderOnce` components containing text and only a cached chart (`P/card.rs:816,827,883`). This matches the nine-chart repaint finding in `docs/plan/leader-notes.md:777,836`.

3. **Recommended design inference:** keep traversal containers uncached; cache complete independently changing visual units. This follows the distinction between `G/view.rs:438` and `501`.

   In `U/src/window/main.rs:2318`, extend the uncached branch to `opacity.is_some() || self.active == Tab::Performance`. Preserve other tabs’ caches and the existing fade path. In `P/mod.rs:938`, embed the sidebar normally; at `:958–960`, embed `selected_view()` normally. Both sides need this change.

   Add a persistent `Entity<CardView>` per `CardState`, retained through reconciliation (`P/sidebar.rs:118,654–680`). Its render delegates to the existing complete `DeviceCard`, including text, borders and chart. Embed it cached at the existing definite card width and scaled `CARD_H` (`P/card.rs:893–894`).

   Keep the sidebar’s hub/settings/metrics/strings subscriptions (`P/sidebar.rs:450–453`). Push changed card properties into the corresponding entity and notify **that card**; reserve sidebar notifications for list/layout changes. Replace the aggregate text-change notification at `:858–860` with per-card updates.

   Card views need no broad hub or sidebar observer. Existing chart subscriptions already filter their own series and notify themselves (`U/src/chart/view.rs:89,137–149`); ancestor marking dirties the owning cached card. Compare and notify on text, validity, scale/spec and presentation changes too, not just series versions (`P/sidebar.rs:800–837`; `P/card.rs:779–783`).

   Overview boxes already are separate cached entities (`P/overview/mod.rs:367–377`). Retain their selective subscriptions and notifications, for example `cpu_box.rs:184–188,231–260`; leave OverviewPage uncached through the change at `P/mod.rs:960`.

   Device chart areas already have cached `ChartView`/`ChartGridView` boundaries (`P/cpu.rs:1456`; `P/cpu_grid.rs:398`; `P/disk.rs:680`). Retain these and their own-series notifications (`U/src/chart/grid_element.rs:336–361`). No new encompassing cached chart-area wrapper is needed; it would couple its child charts again (`G/view.rs:501`).

   Preserve geometry, chart specs, demand, animation behavior and GPUI itself. D-022 explicitly rejects a GPUI fork (`docs/DECISIONS.md:12–15`). Uncached container renders remain expected; unchanged leaf renders and paint callbacks should disappear.

4. **Recommended split, design inference:** first a prerequisite bead naming `U/src/window/main.rs`, `P/mod.rs`, and full-window regression tests; then complete `wlf.36`. Its current file list excludes both necessary ancestor files, and `AGENTS.md:23` restricts edits to named files.

   Edit order: ancestor embeddings → `P/card.rs` persistent wrapper/hooks → `P/sidebar.rs` entity retention and selective updates → `U/tests/ui_sidebar.rs`, `ui_overview.rs`. Overview/chart production changes are unnecessary unless verification exposes another invalidation dependency; their boundaries already exist (`P/overview/mod.rs:371`; `P/cpu.rs:1456`).

5. Risks and preservation checks, **design inference**:

   Goldens/layout: cached wrappers need definite dimensions; preserve current width, height, padding and IDs (`G/view.rs:234–240`; `P/card.rs:823,888–900`). Text-only changes now repaint the whole changed card, so update the old “no chart rendered” expectation (`U/tests/ui_sidebar.rs:729–751`).

   Focus/hover: retain the sidebar’s single focus handle, action routing, scrolling and menu wrappers; propagate selection/focus changes to affected card entities (`P/sidebar.rs:1087–1118,1160–1183`). Preserve hover/active styles and tooltip identity (`P/card.rs:861–875,902–905`); cached paint replay retains listeners/tab stops (`G/window.rs:3900–3915`).

   Ctrl-pause: preserve shell events and table freezing; do not add chart freezing (`U/src/window/main.rs:1700–1709`; `docs/ui/shell.md:1413–1414`). Charts currently subscribe directly to snapshots (`U/src/chart/view.rs:89`).

   Scale/theme/strings: explicitly update affected card properties and notify leaves; container notification alone will no longer refresh them (`P/sidebar.rs:885–906`; `G/view.rs:484`). Display-scale changes already force window refresh (`G/window.rs:2683–2689,2931–2933`).

6. Test plan, **design inference**:

   Use the real MainWindow fixture with all cards (`U/tests/ui_overview.rs:1951–1959`), plus isolated sidebar/Overview tests. Remove cached-container assumptions from test hosts too (`U/tests/ui_sidebar.rs:401`). Set smooth motion off, settle layout/focus, and keep pointer, scale and bounds fixed.

   Warm caches; baseline counters; install `AnimationDriver::install_paint_log()`; publish one CPU-series change; drain events and draw using the existing helpers (`U/src/anim.rs:207`; `U/tests/ui_sidebar.rs:84`). Assert CPU card render delta **1**, all unrelated cards **0**, and paint-log IDs only for affected charts (`U/src/chart/element.rs:144–152`).

   Existing hooks count chart renders, builds and path paints (`U/src/chart/view.rs:60–66`), but no complete-card render counter exists because `DeviceCard` is `RenderOnce`. Add that counter to the new wrapper; for literal whole-card paint counts, add a test-only paint marker. Chart logs alone cannot measure text/background paint.

   Test CPU-total with Overview hidden for exactly one card, then both visible for exactly two. On Overview, assert only CpuBox renders/paints; on device pages, assert only charts whose inputs changed do so. Strengthen the existing Overview test, which currently checks unrelated **builds**, not renders/paints (`U/tests/ui_overview.rs:1754–1763`).

   Add text-only, unchanged-snapshot, focus/hover, Ctrl-pause, resize/scale and golden checks. Test smooth motion separately: live smooth charts independently request notifications, so unchanged series do not guarantee zero paints (`U/src/chart/element.rs:507–512`; `U/src/anim.rs:281–282`). Profile focused `static` and `perf-cpu` afterward (`docs/plan/leader-notes.md:828,836`).

The remaining spec question is whether to revise `wlf.36` to “only affected cards,” with the one-card fixture above. Requiring one CPU-total card while both are visible conflicts with D-022; leave that acceptance decision to the leader (`AGENTS.md:18–19`).