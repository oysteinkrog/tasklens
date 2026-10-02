# asupersync 0.5.0 API map for the tasklens engine

asupersync 0.5.0 can run the tasklens engine, with three changes to the plan:

1. **Supervision API.** Use `ManagedSupervisor` for the worker tree, not `AppSpec`, `GenServer` or `actor`. In a production runtime those three can only be spawned with `&mut RuntimeState`, which only `LabRuntime` exposes.
2. **Engine-to-UI path.** Keep the ArcSwap slot plus `async-channel` wake. asupersync's channel *senders* need no `Cx`, so the UI-to-engine path can use asupersync channels directly.
3. **Actions.** Run each action as its own task with an explicit `time::timeout`. Do not use `GenServer::call`, and do not rely on a `Budget` deadline alone.

**How this was checked.**
- Sources: published 0.5.0 is at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/asupersync-0.5.0` (sha `78b64636`). Main is at `/home/oystein/work/tasklens-ref/asupersync` (version 0.6.0, unpublished, 250 commits past 0.5.0). The clone is shallow and has no `v0.5.0` tag, so I compared against the registry source.
- Line numbers: `L` is the 0.5.0 line, `m:` is the main line. Paths are under `src/`.
- Compile test: I built and ran a scratch crate on **stable rustc 1.98.1** against `asupersync =0.5.0`, `default-features = false, features = ["proc-macros"]`. It uses the runtime, supervisor, watch, mpsc, oneshot, interval, sleep, spawn_blocking, timeout, drain and shutdown code below. It compiled and ran: the drain returned `Quiescent`, then `shutdown_timeout` returned `true`. The crate is at `/var/tmp/claude/claude-1000/-home-oystein/3bebe002-7be5-4c37-8e3b-0d5332a57446/scratchpad/asx/src/main.rs`.
- Anything I did not read in code or run is marked [inferred].

## 1. Crate setup

**Cargo.toml for tasklens:**
```toml
asupersync = { version = "=0.5.0", default-features = false, features = ["proc-macros"] }
[dev-dependencies]
asupersync = { version = "=0.5.0", default-features = false, features = ["proc-macros", "test-internals"] }
```

**Features and toolchain.**
- The 0.5.0 defaults are `["proc-macros", "nightly-outcome-try"]` (registry `Cargo.toml:215`).
- `nightly-outcome-try` turns on `feature(try_trait_v2)` (`lib.rs:52-53`). That needs nightly; the repo pins `nightly-2026-08-31`.
- The README (0.5.0, around line 660) names `--no-default-features --features proc-macros` as the audited stable subset. I verified that it compiles on stable 1.98.1.
- There is no `rust-version` field. Edition is 2024.
- The `desktop-runtime-profile` feature exists on main only.
- With `nightly-outcome-try` off, `Outcome` cannot be used with `?`. Match on it explicitly.

**`test-internals` must stay in dev-dependencies.** It gates these:
- `Cx::for_testing` (L5198) and `Cx::for_request` (L5323)
- `test_utils` (`lib.rs:401`)

The skill's "Quick Orient" example uses `Cx::for_request()`, which does not compile in a production build. The CHANGELOG migration note on main (lines 102-117) says to use `Cx::current()` or `Runtime::request_cx_with_budget` instead.

**Notable dependencies** (non-optional): parking_lot, polling, nix, libc, signal-hook, socket2, sysinfo, serde, rmp-serde, prost, the franken-* crates and nkeys. There is no tokio in the normal graph.

## 2. Runtime bootstrap and calls from outside the runtime

**Builder** (`runtime/builder.rs`):
- `RuntimeBuilder::new()` L803
- `.worker_threads(n)` L2659
- `.blocking_threads(min, max)` L2789
- `.thread_name_prefix(..)` L2742
- `.global_queue_limit(n)` L2749
- `.root_region_limits(RegionLimits)` L3019
- `.on_thread_start(f)` L3033
- `.build() -> Result<Runtime, Error>` L3276

**Thread model.**
- `build()` starts its own worker threads, so GPUI keeps the main thread with no extra wrapper thread.
- The timer driver and the platform reactor are installed by default (main L3292-3330, L3565).
- Do not use `current_thread()` (L3636): it runs work on whichever thread calls `block_on`.

**The blocking pool is off by default.**
- `BlockingPoolConfig` derives `Default` as 0/0 (m:`config.rs:86`), and no pool is created when max is 0 (m:`builder.rs:6138`).
- Without a pool, `Cx::spawn_blocking` runs the closure **inline on the async worker**. The entry macros' docs say the same (`asupersync-macros/src/entry.rs:13-18`).
- So always call `.blocking_threads(..)`.

**`RuntimeHandle`** (Clone, usable from any thread):

| Method | Signature | 0.5.0 line |
|---|---|---|
| `Runtime::handle` | `(&self) -> RuntimeHandle` | L4023 |
| `spawn` | `<F: Future + Send + 'static>(&self, F) -> JoinHandle<F::Output>` | L4867 |
| `try_spawn` | `-> Result<JoinHandle<_>, SpawnError>` | L4878 |
| `spawn_with_cx` | `<F: FnOnce(Cx) -> Fut + Send + 'static, Fut: Future<Output = ()> + Send + 'static>(&self, f: F)` | L4942 |
| `try_spawn_with_cx` | same, `-> Result<(), SpawnError>` | L4968 |
| `spawn_checked` | `(&self, F) -> CheckedJoinHandle<F::Output>` | L6199 |
| `try_spawn_checked` | `-> Result<CheckedJoinHandle<_>, SpawnError>` | L6210 |
| `request_cx_with_budget` (on `RuntimeHandle`) | `(&self, Budget) -> Cx` | L6235 |
| `request_cx_with_budget` (on `Runtime`) | `(&self, Budget) -> Cx` | L4158 |

**Join handles a foreign executor can await.**
- `JoinHandle<T>` and `CheckedJoinHandle<T>` are ordinary `Future`s that store the caller's `std::task::Waker` (m:`builder.rs:5262`, m:6683).
- GPUI's executor can await them [inferred from the poll code; not run under GPUI].
- `CheckedJoinHandle` yields `Result<T, JoinError>` (`runtime/task_handle.rs`, m:136).
- `JoinHandle` **panics** if the task was cancelled (m:5280). Use `spawn_checked`.

**Root region.**
- Tasks from `spawn`, `spawn_with_cx` and `block_on` live in the root region.
- `Cx::current()` works inside `spawn` futures.
- `spawn_with_cx` hands over a `Cx` that sees shutdown cancellation from `drain_root_region`.

## 3. Supervision tree

**Use `ManagedSupervisor`.** It is in 0.5.0 (`supervision.rs`) and is the only supervisor that actually executes restarts.

**Do not use these** on a production runtime:
- `AppSpec::start(self, state: &mut RuntimeState, cx: &Cx, parent_region: RegionId)` (m:`app.rs:2565`)
- `CompiledSupervisor::spawn` (m:1639)
- `Scope::spawn_gen_server(&self, state: &mut RuntimeState, cx, server, mailbox_capacity)` (L1766)
- `Scope::spawn_actor` (`actor.rs:1084`)

All four need `&mut RuntimeState`. Only `LabRuntime.state` exposes it (`pub state`, m:`lab/runtime.rs:2375`); a `Runtime` has no public accessor. The bridge plan lists AppSpec as "STUB: metadata only; legacy supervisor" (`ASUPERSYNC_BRIDGE_PLAN_2026-09-01.md:604, 615`). `ManagedApp` is main-only and built around HTTP routes.

**Topology types:**
- `SupervisorBuilder::new(name)` (m:894)
  - `.with_restart_policy(RestartPolicy)` (m:924)
  - `.with_budget` (m:906)
  - `.child(ChildSpec)` (m:931)
  - `.compile() -> Result<CompiledSupervisor, SupervisorCompileError>` (L970)
- `ChildSpec::new<F: ChildStart + 'static>(name: impl Into<ChildName>, start: F)` (L654)
  - `.depends_on(name)` (L686)
  - `.with_shutdown_budget(Budget)` (L679)
  - `.with_required(bool)` (m:710)
  - `.with_start_immediately(bool)` (m:701)

**API wart:** `ChildSpec::new` requires a legacy `ChildStart` even in managed mode. Pass a stub that panics. The repo's own tests do this (`tests/supervision_regression.rs:197-204`):
```rust
fn legacy(_: &Scope<'static, FailFast>, _: &mut RuntimeState, _: &Cx) -> Result<TaskId, SpawnError> { unreachable!() }
```

**Restart modes and policies:**
- `RestartPolicy { OneForOne (default), OneForAll, RestForOne }` (L382)
- `ManagedRestartMode` (L1811):
  - `Permanent`: restarts on any exit, success included.
  - `Transient`: restarts on `Err` and on panic.
  - `Temporary`: never restarts.
  - Cancelling the controller never restarts a child.

**Intensity** is `SupervisionConfig` (L429):
- `new(max_restarts: u32, restart_window: Duration)` (m:470). The default is 3 restarts per 60 s.
- `.with_restart_policy` (m:491), which must equal the builder's policy or binding fails with `RestartPolicyMismatch`.
- `.with_backoff(BackoffStrategy::{None, Fixed(d), Exponential{initial, max, multiplier}})` (L324). The default is exponential, 100 ms to 10 s, ×2.
- `.with_escalation(EscalationPolicy::{Stop, Escalate, ResetCounter})` (L407).
- `.with_storm_threshold(f64)`.

**The intensity budget is shared by the whole supervisor, not counted per child.** The controller holds one `tracker` (m:3006-3040), and the test comment reads "two failing children share one allowance". To keep a crash-looping NVML worker from using up the cpu worker's restart budget, give each worker its **own one-child `ManagedSupervisor`**, all spawned from one engine task. When the budget runs out:
- `Stop`: the failed child stays down and its siblings continue (m:3022).
- `Escalate`: cancels the parent region.

**Defining a worker.**
- Trait: `pub trait ManagedChildFactory<E>: Send + Sync + 'static { fn start(&self, cx: Cx, generation: ManagedGeneration) -> ManagedChildFuture<E>; }` (L1854).
- Any `Fn(Cx, ManagedGeneration) -> Fut` with `Fut: Future<Output = Outcome<(), E>> + Send` implements it.
- `ManagedGeneration { number, region, task }`.
- Bind with `ManagedChildBinding::new(name, mode, factory)` (m:1899).

**Binding and running:**
- `CompiledSupervisor::bind_managed<E>(self, Vec<ManagedChildBinding<E>>, SupervisionConfig) -> Result<ManagedSupervisor<E>, ManagedSupervisorBindError>` (L2054)
- `ManagedSupervisor::run(self, cx: &Cx) -> ManagedSupervisorReport<E>` (async, L3062)
- `ManagedSupervisor::spawn(self, cx: &Cx) -> Result<ManagedSupervisorHandle<E>, SpawnError>` (L3084)
- `ManagedSupervisorHandle::abort(&self)` (L3026)
- `ManagedSupervisorHandle::join(&mut self) -> Result<ManagedSupervisorReport<E>, JoinError>` (async, L3032)
- Dropping a `ManagedSupervisorHandle` aborts it.
- The report (L1997) carries each child's latest outcome, `started`, `joined`, `restart_batches` and `escalations`.

**Names.**
- Use `ChildName`; it shows up in the report and in traces.
- Registry-backed names (`bind_managed_with_registry`) and `DynamicSupervisor` exist on main only.

**How a worker receives messages.**
- Through channels captured by the factory. Each generation must take fresh endpoints: call `watch::Sender::subscribe()` or clone a `watch::Receiver` or `mpsc::Sender`.
- Do not give a restartable worker a single-owner `mpsc::Receiver`. A panicking generation drops it.

## 4. Timers, checkpoint, budgets, blocking pool

**`interval` is synchronous.**
- Signature: `time::interval(now: Time, period: Duration) -> Interval` (`time/interval.rs:430`).
- `tick(&mut self, now)` and `poll_tick` do not await anything.
- Other methods: `deadline()`, `set_missed_tick_behavior(MissedTickBehavior::Skip)`.

**Sleep:**
- `time::sleep(now: Time, d) -> Sleep` (`time/sleep.rs:1124`)
- `time::sleep_until(deadline: Time) -> Sleep` (L1145)
- `Sleep` uses the ambient `Cx` timer, which is virtual time in the lab.
- `Sleep` **returns early when a user or shutdown cancellation arrives** (`sleep.rs:749`). Deadline and timeout cancellations do not trigger the early return.

**Worker loop pattern** (this is what the compiled test does):
```rust
let mut iv = interval(cx.now(), period);
loop {
    sleep_until(iv.deadline()).await;
    if cx.checkpoint().is_err() { return Outcome::Cancelled(cx.cancel_reason().unwrap()); }
    iv.tick(cx.now());
    /* blocking work below */
}
```
Create a new `Sleep` each time round the loop. Main's bridge plan records a `Sleep polled after completion` panic in another driver.

**Cx methods** (`cx/cx.rs`):
- `checkpoint(&self) -> Result<(), Error>` (L2508)
- `now()` (L2838)
- `is_cancel_requested()` (L2400)
- `cancel_reason()`
- `masked(f)` (L3070)
- `budget_for_timeout(d)` (L2302)
- `scope_with_budget(Budget)` (L4130)
- `spawn<F: FnOnce(Cx) -> Fut>(&self, f) -> Result<TaskHandle<Fut::Output>, SpawnError>` (L4420)

**Budget** (`types/budget.rs`):
- `Budget::INFINITE`
- `Budget::new()`
- `.with_timeout(now, d)` (L360)
- `.with_deadline(Time)` (L336)
- `.with_poll_quota(n)`
- Main (0.6.0) now enforces poll quotas in production. Never set one on a long-lived worker.

**Blocking pool calls:**
- **Preferred:** `Cx::spawn_blocking<F: FnOnce(Cx) -> R + Send + 'static, R>(&self, f) -> Result<TaskHandle<R>, SpawnError>` (L4661). The work belongs to the region and is joined with `TaskHandle::join(&mut self, &Cx)` (m:`task_handle.rs:793`).
- Free function: `runtime::spawn_blocking::spawn_blocking(f) -> T` (async, L224). It uses the ambient `Cx`'s pool, runs inline when there is no pool, and uses its own thread when there is no `Cx`.
- **Avoid** `RuntimeHandle::spawn_blocking(..) -> Option<BlockingTaskHandle>` (L4980): the work belongs to no region (open bead `bi2462.156`).
- `spawn_blocking_drained` is main-only.
- A running closure is not interrupted by cancellation; only its result is discarded.

**Sizing.**
- `blocking_threads(min, max)`. Idle threads retire after 10 s (m:`builder.rs:6142`).
- Suggested starting point [inferred]: `.worker_threads(2).blocking_threads(1, 4)`. Main's desktop profile uses 2 workers, 1-2 blocking threads and a global queue limit of 256 (m:`runtime/desktop_profile.rs:46-58`).
- Do a whole /proc sweep inside **one** `spawn_blocking` closure. `asupersync::fs::read_to_string` sends each call to the pool separately (`fs/path_ops.rs:443`).

## 5. Channels and whether they need a Cx

| Channel | No-Cx operations | Operations that need `&Cx` |
|---|---|---|
| `mpsc::channel<T>(cap)` (L540) | `Sender::try_send(&self, T) -> Result<(), SendError<T>>` (L720), `send_evict_oldest` (L823), `Receiver::try_recv` (L1807), `UnboundedSender::send` | `send(&self, cx, T)` (async, L665), `recv(&mut self, cx)` (L1576), `poll_recv(&mut self, cx, &mut Context)` (L1629) |
| `watch::channel(init)` (L408) | `Sender::send(&self, T)` (L488; works with zero receivers), `send_modify`, `subscribe`, `Receiver::borrow_and_update` (L803), `has_changed` (L856), `borrow_and_clone` | `Receiver::changed(&mut self, cx)` (L685) |
| `broadcast::channel(cap)` (L250) | `Receiver::try_recv` (L831) | `Sender::send(&self, cx, T) -> Result<usize, _>` (L487), `recv(cx)` |
| `oneshot::channel()` (L395) | `Sender::send_blocking(self, T)` (L572; never blocks or parks), `Receiver::try_recv` (L1298) | `send(self, cx, T)` (L550), `recv(&mut self, cx)` (L1240) |

**From the GPUI main thread:**
- Every no-Cx operation above is a plain lock operation, so it is safe from a foreign thread.
- The async receives use the caller's std `Waker`. The `Cx` is used only for `checkpoint()` (mpsc `poll_recv`, m:1639-1660).
- A foreign executor *could* await them with a Cx minted by `Runtime::request_cx_with_budget(Budget::INFINITE)` [inferred; not run].

**Recommended wiring:**
- **UI to engine settings** (which tab and rows are visible, refresh rate): `watch::Sender::send`. Workers call `subscribe()` per generation and `changed(&cx)`.
- **UI to engine one-off commands:** `mpsc::Sender::try_send`.
- **Engine to UI:** ArcSwap plus `async_channel::bounded(1)::try_send` as the wake, as planned. A `watch::Receiver` could replace the ArcSwap: `has_changed` plus `borrow_and_clone`. Bead `c11v6v` warns never to hold a `watch` `Ref` across other channel calls (read-lock deadlock).

## 6. Request/response with a deadline

**Do not use `GenServer::call`.**
- It cannot be spawned in production (section 3).
- Main rejects calls from root-region tasks with `[ASUP-E103]` (m:`gen_server.rs:874`). 0.5.0 lacks that guard and would panic in the token layer. Every `RuntimeHandle::spawn` task is a root-region caller.
- `call` (L909) has no deadline parameter.

**Do not rely on `Budget` deadlines alone.** Bead `pev2xi` (open): region deadlines only take effect at checkpoints and never wake a parked task.

**Verified pattern** (one task per action, spawned from the UI thread):
```rust
let (tx, rx) = oneshot::channel::<Result<(), ActionError>>();
let join = handle.spawn_checked(async move {
    let cx = Cx::current().unwrap();
    let work = Box::pin(async { /* cx.spawn_blocking(|_| libc::kill(..))?.join(&cx).await */ });
    let r = time::timeout(cx.now(), Duration::from_secs(2), work).await; // Result<_, Elapsed>
    let _ = tx.send_blocking(r.unwrap_or(Err(ActionError::Deadline)));
});
// UI: await `join` (CheckedJoinHandle) on GPUI, or poll rx.try_recv(), or push the result over async-channel.
```

Supporting signatures:
- `time::timeout<F>(now: Time, d: Duration, f: F) -> TimeoutFuture<F>` (`time/timeout_future.rs:344`)
- `timeout_at(deadline, f)` (L366)
- The output is `Result<F::Output, Elapsed>`.
- `Cx::race_timeout(duration, Vec<Pin<Box<dyn Future>>>)` (`cx.rs:3983`) also exists, but it drops losers without draining them.

**The deadline does not stop work already running on the blocking pool** (for example a stuck NVML or D-Bus call). It only bounds when the reply arrives.

## 7. LabRuntime tests

**Constructors and config:**
- `LabRuntime::new(LabConfig) -> Self` and `LabRuntime::with_seed(u64)`
- `LabConfig::new(seed)` (`lab/config.rs:184`)
- `.max_steps(n)` (L299)
- `.worker_count(n)` (L271)
- `.with_chaos(ChaosConfig)` (L322), `.with_light_chaos()`, `.with_heavy_chaos()`
- `ChaosConfig` builders (`lab/chaos.rs`): `with_cancel_probability`, `with_delay_probability`, `with_budget_exhaust_probability`, `with_wakeup_storm_probability`, `with_io_error_probability`

**Driving the lab:**
- `lab.state.create_root_region(Budget) -> RegionId` (`runtime/state.rs:3449`)
- `lab.state.create_task(region, budget, fut) -> Result<(TaskId, TaskHandle<T>), SpawnError>` (L3965)
- `lab.scheduler.lock().schedule(task, 0)`
- `run_until_idle()` (`lab/runtime.rs:3269`)
- `advance_time(nanos)` (L2956) and `advance_time_to(Time)`
- `run_with_auto_advance() -> VirtualTimeReport` (L3104), which jumps straight to each timer deadline
- `run_until_quiescent_with_report().lab_test_passed()` (L3296)

**Behaviour inside the lab.**
- `Cx::spawn_blocking` runs inline, so the run stays deterministic.
- Supervisor backoff uses virtual time.

**Macros:** `#[lab_test]` and `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]`, from `proc-macros` (macros 0.5.0 `lib.rs:446, 475`).

**Design rule:** write the engine as `async fn engine(cx: Cx, chans…)`.
- In production, spawn it with `try_spawn_with_cx`.
- In tests, wrap it in `create_task` and get the `Cx` with `Cx::current()`.

**Restart test**, modelled on `supervision.rs` tests m:3376-3395 and m:3606-3651:
- Build a `ManagedChildBinding` whose generation 1 returns `Outcome::Err` or panics. Check `report.started` and `report.restart_batches`.
- Abort the parent handle and confirm no generation is started after cancellation.
- Finish with `run_until_quiescent_with_report().lab_test_passed()`.

## 8. Shutdown

Order when the window closes, verified in the scratch run:
1. Drop the UI-side senders.
2. Call `rt.drain_root_region(bound) -> RootDrainOutcome { Quiescent | TimedOut }` (L4445). It sends `CancelKind::Shutdown` to every root task, so the supervisors cancel and drain their children.
3. **Drop every `RuntimeHandle` clone.**
4. Call `rt.shutdown_timeout(Duration) -> bool` (L4274). It returns `true` only when teardown finished.

In my run, step 4 returned `false` while a handle was still alive and `true` after dropping it.

Notes:
- `shutdown_background(self)` (L4389) exists.
- `shutdown_drained -> RootDrainReport` is main-only.
- Do not await `TaskHandle`s from other threads after teardown. Bead `bi2462.91` (in progress) says a join can hang forever.

## 9. zbus, D-Bus, fs and process

**zbus 5 with its default `async-io` backend:**
- Starts its own `"zbus::Connection executor"` thread (`zbus-5.19.0/src/connection/builder.rs:789`) plus async-io's reactor thread.
- Its futures wake through the std `Waker`, so awaiting them from asupersync tasks should work beside asupersync's own epoll reactor. [inferred; not run]
- zbus futures do not see `Cx` cancellation. Wrap each call in `time::timeout`, and checkpoint after it.
- Keep D-Bus proxies inside one worker generation, or rebuild them on restart.

**What asupersync has instead:**
- No D-Bus support and no netlink/sock_diag. Run sock_diag as blocking netlink calls inside `spawn_blocking`.
- `asupersync::fs` only wraps std through the pool.
- `asupersync::process` exists, but bead `bi2462.114` (open) reports that dropped child processes become zombies.
- **Avoid `asupersync::signal`/`ctrl_c` in 0.5.0.** In 0.5.0 the first call installs handlers for 10 signals process-wide (`bi2462.113`, fixed only on main on 09-28).

## 10. Maturity and things to avoid

**Version state.**
- 0.5.0 was published 2026-09-12. Main is 250 commits ahead and unpublished.
- Open GitHub issues: only #72 (CI time) and #74 (TCP_USER_TIMEOUT); neither affects tasklens.

**Fixed on main but still present in 0.5.0:**
- `race!`/`select!` losers never observe cancellation (`bi2462.89`). Avoid these macros and `race_drained`.
- Signal handler takeover (`bi2462.113`).
- `GenServer` root-caller panic (section 6).
- File poll traits block the worker when there is no pool (`bi2462.118`).

**Open or in progress on main:**
- `bi2462.99`: `ManagedSupervisor` silently never restarts a required child whose dependency is gone, and the report ends `Ok`. Do not use `depends_on`.
- `bi2462.91`: join hang after teardown.
- `pev2xi`: deadlines only fire at checkpoints.
- `bi2462.156`: runtime-level `spawn_blocking` belongs to no region.
- `c11v6v`: `watch` borrow deadlock.
- `gqnbdw`: `Cx::cancelled()` never wakes after the task has finished.
- `52hxjz`: the lab feeds only 8 of 24 oracles.

**Experimental or main-only:**
- `desktop_profile` and `desktop_samples`
- `DynamicSupervisor`, `supervisor_service`, `resource_bracket`
- `worker_readiness` (gated behind `test-internals`, labelled "source-only")
- `ManagedApp`

**Project health.** Main's bridge plan (2026-09-23) reports that validation capacity is the binding constraint, and that several commits landed without being compiled.

**Rules for tasklens:**
- Use `ManagedSupervisor`, one per worker.
- Use `Cx::spawn_blocking` with an explicit `blocking_threads`.
- Use explicit `time::timeout` for deadlines.
- Use `spawn_checked` rather than `spawn`.
- Avoid `race!`, `select!`, asupersync signals, `GenServer`/`actor`/`AppSpec` in production code, and `Cx::for_request`.
