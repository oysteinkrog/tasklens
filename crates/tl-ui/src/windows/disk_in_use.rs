//! "tasklens Disk is in use" window (PERF-052, UI-PRF-030, R-036, D-009).
//!
//! `PerfEject` on a Disk page calls [`start_eject`]. It sends `DeviceCommand::Eject` with a new
//! sequence number and keeps one [`EjectWatch`] per disk. The watch follows the disk record's
//! `eject` state for that sequence: `Blocked` opens this window, `Done` closes it and shows
//! "Disk ejected", `Failed` shows the reason. The watch follows the hub itself, because a USB disk
//! leaves the catalog after `Done` and its page goes with it.
//!
//! The window lists the blocking processes grouped by app. End Task sends SIGTERM and End Process
//! (Shift held) SIGKILL through the shared process action entry point, `run_process_action`, so
//! the refusals, confirmations and toasts are the Processes tab's; the window never sends a signal
//! itself (ADR-006). End All with more than one process, and any action on a protected process or
//! another user's process, asks first (R-036, D-009). 500 ms after an end action finishes, the
//! eject is sent again, once per action. While an eject is in flight nothing new is sent and
//! Retry is disabled, because the worker drops an `Eject` that arrives while one runs.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::Icon;
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, AsyncApp, Context, Entity, FocusHandle, FontWeight,
    Global, ImageSource, InteractiveElement as _, IntoElement, ModifiersChangedEvent,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, WeakEntity, Window, WindowHandle, div, img, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::ActionId;
use tl_model::command::{Command, CommandError, DeviceCommand};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, ProcessKey};
use tl_model::snapshot::{Blocker, DiskDetails, EjectState, Snapshot};
use tl_model::validity::Validity;
use tracing::warn;

use crate::action_tracker::{ActionTrackerEvent, action_words, host_send_allowed};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::toast::ToastKind;
use crate::components::tooltip::TlTooltip;
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::host::HostContext;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::DeviceKey;
use crate::pages::processes::actions::{CapturedTargets, EndProcess, EndTask};
use crate::pages::processes::record_target;
use crate::pages::processes::run::{OnSent, RunOptions, run_process_action_with};
use crate::pages::processes::toasts::toast_words;
use crate::text::style::{label_style, t_base, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, WindowKey};

/// Window size, × ui_scale.
pub const SIZE: (f32, f32) = (520., 400.);
/// Process row height, × ui_scale.
const ROW: f32 = 28.;
/// Group header row height, × ui_scale.
const GROUP_ROW: f32 = 28.;
/// App icon size, × ui_scale.
const ICON: f32 = 16.;
/// Width of the PID column, × ui_scale.
const PID_W: f32 = 64.;
/// The wait between an end action's result and the automatic retry.
pub const RETRY_DELAY: Duration = Duration::from_millis(500);

/// Eject sequence numbers. The worker ignores a number at or below the last one it saw for a
/// disk, so every send takes a new one.
static EJECT_SEQ: AtomicU64 = AtomicU64::new(1);

/// One blocking process as the list shows it.
#[derive(Clone, Debug)]
pub struct BlockerRow {
    pub key: ProcessKey,
    pub name: SharedString,
    pub pid: SharedString,
    /// The open paths joined with ", ".
    pub paths: SharedString,
    /// Every open path, one per line.
    pub tooltip: SharedString,
}

/// The blocking processes of one app, in the order the engine listed them.
#[derive(Clone, Debug)]
pub struct BlockerGroup {
    /// The app id, `None` for the processes outside an app.
    pub app: Option<Arc<str>>,
    pub name: SharedString,
    /// The resolved icon file path.
    pub icon: Option<Arc<str>>,
    pub rows: Vec<BlockerRow>,
}

/// Groups `blockers` by app in first-seen order. The name and icon come from the first member's
/// live record, with the app id as the name fallback. Processes outside an app go last under
/// `background`.
pub fn group_blockers(
    blockers: &[Blocker],
    snap: &Snapshot,
    background: &SharedString,
) -> Vec<BlockerGroup> {
    let mut groups: Vec<BlockerGroup> = Vec::new();
    let mut loose: Vec<BlockerRow> = Vec::new();
    for b in blockers {
        let row = BlockerRow {
            key: b.key,
            name: SharedString::from(&b.name),
            pid: b.key.pid.to_string().into(),
            paths: b.paths.join(", ").into(),
            tooltip: b.paths.join("\n").into(),
        };
        let Some(app) = &b.app else {
            loose.push(row);
            continue;
        };
        if let Some(group) = groups.iter_mut().find(|g| g.app.as_ref() == Some(app)) {
            group.rows.push(row);
            continue;
        }
        let meta = snap.processes.get(b.key).map(|r| &r.meta);
        groups.push(BlockerGroup {
            app: Some(app.clone()),
            name: SharedString::from(meta.map_or(app, |m| &m.name)),
            icon: meta.and_then(|m| m.icon.clone()),
            rows: vec![row],
        });
    }
    if !loose.is_empty() {
        groups.push(BlockerGroup {
            app: None,
            name: background.clone(),
            icon: None,
            rows: loose,
        });
    }
    groups
}

/// The kernel name of disk `idx` from the device catalog.
fn kernel_name(snap: &Snapshot, idx: DeviceIdx) -> Option<Arc<str>> {
    snap.devices
        .entries
        .iter()
        .find(|e| e.idx == idx)
        .and_then(|e| match &e.identity {
            DeviceIdentity::Disk { name, .. } => Some(name.clone()),
            _ => None,
        })
}

fn disk_details(snap: &Snapshot, idx: DeviceIdx) -> Option<&DiskDetails> {
    snap.device_details
        .disks
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, d)| d)
}

fn shown(text: &str, validity: Validity) -> Option<&str> {
    let ok = matches!(validity, Validity::Valid | Validity::Stale) && !text.trim().is_empty();
    ok.then_some(text)
}

/// The disk model as the page header finds it, else the kernel name.
fn model_text(snap: &Snapshot, idx: DeviceIdx) -> SharedString {
    let details = disk_details(snap, idx).and_then(|d| shown(&d.model.value, d.model.validity));
    let model = details.or_else(|| {
        snap.inventory
            .disks
            .iter()
            .find(|d| d.device == Some(idx))
            .and_then(|d| shown(&d.model.value, d.model.validity))
    });
    match model {
        Some(model) => model.trim().to_owned().into(),
        None => kernel_name(snap, idx).map_or_else(SharedString::default, SharedString::from),
    }
}

/// The eject toasts, recorded for tests: they go to the main window, which tests do not have.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub struct EjectToasts(pub Vec<(ToastKind, SharedString)>);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for EjectToasts {}

fn toast(kind: ToastKind, text: SharedString, cx: &mut App) {
    #[cfg(any(test, feature = "test-hooks"))]
    cx.default_global::<EjectToasts>()
        .0
        .push((kind, text.clone()));
    push_main_toast(cx, kind, text, None, None);
}

/// The open "Disk is in use" window of `idx`, with its content.
pub fn find_disk_in_use(
    idx: DeviceIdx,
    cx: &App,
) -> Option<(WindowHandle<Root>, Entity<DiskInUseWindow>)> {
    cx.windows().into_iter().find_map(|w| {
        let handle = w.downcast::<Root>()?;
        let frame = TlSecondaryWindow::<DiskInUseWindow>::find(handle, cx)?;
        let content = frame.read(cx).content().clone();
        (content.read(cx).idx == idx).then_some((handle, content))
    })
}

fn close_disk_in_use(idx: DeviceIdx, cx: &mut App) {
    if let Some((handle, _)) = find_disk_in_use(idx, cx) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// The watch of each disk with an eject in flight.
#[derive(Default)]
struct EjectWatches(FxHashMap<DeviceIdx, Entity<EjectWatch>>);

impl Global for EjectWatches {}

/// Removes the watch of `idx` when it is still `watch`, so a newer one stays.
fn remove_watch(idx: DeviceIdx, watch: &Entity<EjectWatch>, cx: &mut App) {
    let watches = &mut cx.default_global::<EjectWatches>().0;
    if watches.get(&idx).is_some_and(|w| w == watch) {
        watches.remove(&idx);
    }
    notify_window(idx, cx);
}

/// True while the eject of `idx` is sent and not yet `Blocked`, `Done` or `Failed`. The worker
/// drops an `Eject` that arrives while one runs, so nothing new is sent then.
pub fn eject_in_flight(idx: DeviceIdx, cx: &App) -> bool {
    cx.try_global::<EjectWatches>()
        .and_then(|w| w.0.get(&idx))
        .is_some_and(|w| matches!(w.read(cx).last, None | Some(EjectState::Running)))
}

/// Draws the open window of `idx` again, so Retry follows the eject state.
fn notify_window(idx: DeviceIdx, cx: &mut App) {
    if let Some((_, content)) = find_disk_in_use(idx, cx) {
        content.update(cx, |_, cx| cx.notify());
    }
}

/// Sends the eject of disk `idx` with a new sequence number and follows its state. `disk_label`
/// is the page header word ("Disk 1"). A busy engine shows the tracker's busy warning. While an
/// eject of `idx` is in flight nothing is sent and its watch stays; its open window comes to the
/// front. The eject goes to `host`'s engine.
pub fn start_eject(
    host: &HostContext,
    key: DeviceKey,
    idx: DeviceIdx,
    hub: Entity<SnapshotHub>,
    disk_label: SharedString,
    cx: &mut App,
) {
    if eject_in_flight(idx, cx) {
        // The caller may be the window itself, which is being updated now.
        cx.defer(move |cx| {
            if let Some((handle, _)) = find_disk_in_use(idx, cx) {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        });
        return;
    }
    let Some(disk) = kernel_name(hub.read(cx).current(), idx) else {
        warn!(target: "tasklens::ui", ?idx, "eject: the disk is not in the catalog");
        return;
    };
    let Some(engine) = host.client().cloned() else {
        warn!(target: "tasklens::ui", "eject: no engine");
        return;
    };
    if !host_send_allowed(host, cx) {
        return;
    }
    let seq = EJECT_SEQ.fetch_add(1, Ordering::Relaxed);
    match engine.send(Command::Device(DeviceCommand::Eject { seq, disk })) {
        Ok(()) => {}
        Err(CommandError::Busy) => {
            let text = action_words(cx).send_busy.clone();
            toast(ToastKind::Warning, text, cx);
            return;
        }
        Err(CommandError::EngineStopped) => {
            warn!(target: "tasklens::ui", "eject not sent: the engine has stopped");
            return;
        }
    }
    let watch = cx.new(|cx| EjectWatch::new(host, key, idx, seq, hub, disk_label, cx));
    cx.default_global::<EjectWatches>().0.insert(idx, watch);
    cx.defer(move |cx| notify_window(idx, cx));
}

/// Follows the `eject` state of one disk for one sequence number.
pub struct EjectWatch {
    /// The host of the disk. Set when the eject starts; never changes.
    host: HostContext,
    key: DeviceKey,
    idx: DeviceIdx,
    seq: u64,
    hub: Entity<SnapshotHub>,
    disk_label: SharedString,
    /// The last state acted on, so a repeat of it does nothing.
    last: Option<EjectState>,
    _hub: Subscription,
}

impl EjectWatch {
    fn new(
        host: &HostContext,
        key: DeviceKey,
        idx: DeviceIdx,
        seq: u64,
        hub: Entity<SnapshotHub>,
        disk_label: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let _hub = cx.subscribe(&hub, |this, hub, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::DEVICE_DETAILS) {
                let snap = hub.read(cx).current().clone();
                this.on_details(&snap, cx);
            }
        });
        Self {
            host: host.clone(),
            key,
            idx,
            seq,
            hub,
            disk_label,
            last: None,
            _hub,
        }
    }

    fn on_details(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let me = cx.entity();
        let idx = self.idx;
        let Some(d) = disk_details(snap, idx) else {
            // The disk is gone: no toast.
            cx.defer(move |cx| {
                close_disk_in_use(idx, cx);
                remove_watch(idx, &me, cx);
            });
            return;
        };
        let Some((seq, state)) = &d.eject else {
            return;
        };
        if *seq != self.seq || self.last.as_ref() == Some(state) {
            return;
        }
        self.last = Some(state.clone());
        match state {
            EjectState::Running => {}
            EjectState::Blocked { .. } => {
                let (host, key, hub, label) = (
                    self.host.clone(),
                    self.key.clone(),
                    self.hub.clone(),
                    self.disk_label.clone(),
                );
                cx.defer(move |cx| {
                    open_disk_in_use(&host, key, idx, hub, label, cx);
                    // Retry is allowed again.
                    notify_window(idx, cx);
                });
            }
            EjectState::Done => {
                let text = tr(cx, "perf-toast-ejected");
                cx.defer(move |cx| {
                    close_disk_in_use(idx, cx);
                    toast(ToastKind::Success, text, cx);
                    remove_watch(idx, &me, cx);
                });
            }
            EjectState::Failed(reason) => {
                let text = tr_args(
                    cx,
                    "perf-toast-eject-failed",
                    &[("reason", FluentValue::from(reason.to_string()))],
                );
                // The window stays open so Retry still works.
                cx.defer(move |cx| {
                    toast(ToastKind::Error, text, cx);
                    remove_watch(idx, &me, cx);
                });
            }
        }
    }
}

/// Opens the window of disk `idx`, or brings the open one to the front; it follows the hub
/// itself. A tool window "tasklens Disk is in use", 520 × 400 × ui_scale, keyed
/// `WindowKey::DiskInUse(idx)`. The window shows `host`.
pub fn open_disk_in_use(
    host: &HostContext,
    key: DeviceKey,
    idx: DeviceIdx,
    hub: Entity<SnapshotHub>,
    disk_label: SharedString,
    cx: &mut App,
) {
    if let Some((handle, _)) = find_disk_in_use(idx, cx) {
        // A new eject that is blocked again shows the window the user may have left behind.
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let s = metrics(cx).ui_scale;
    let window_size = size(px(SIZE.0 * s), px(SIZE.1 * s));
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, "perf-diu-title"),
        window_size,
    )
    .key(WindowKey::DiskInUse(idx));
    let host = host.clone();
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| DiskInUseWindow::new(&host, key, idx, hub, disk_label, window, cx))
        })?;
        if let Some(frame) = TlSecondaryWindow::<DiskInUseWindow>::find(handle, cx) {
            // The title follows a language change.
            handle.update(cx, |_, window, cx| {
                frame.update(cx, |_, cx| {
                    cx.observe_global_in::<Strings>(window, |frame, window, cx| {
                        let title = tr(cx, "perf-diu-title");
                        if title != *frame.title() {
                            frame.set_title(title, window, cx);
                        }
                    })
                    .detach();
                });
            })?;
        }
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::DiskInUse(idx), cx, open)
            });
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// The window texts, translated when the strings change.
#[derive(Clone, Debug, Default)]
struct Words {
    end_task: SharedString,
    end_process: SharedString,
    close: SharedString,
    retry: SharedString,
    end_all: SharedString,
    other_users: SharedString,
    background: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        Self {
            end_task: tr(cx, "perf-diu-end-task"),
            end_process: tr(cx, "perf-diu-end-process"),
            close: tr(cx, "perf-diu-close"),
            retry: tr(cx, "perf-diu-retry"),
            end_all: tr(cx, "perf-diu-end-all"),
            other_users: tr(cx, "perf-diu-other-users"),
            background: tr(cx, "processes-header-background"),
        }
    }
}

/// The window content. The frame around it is `TlSecondaryWindow`.
pub struct DiskInUseWindow {
    /// The host this window shows. Set when it opens; never changes.
    host: HostContext,
    idx: DeviceIdx,
    key: DeviceKey,
    hub: Entity<SnapshotHub>,
    disk_label: SharedString,
    /// The blocker list the groups were built from, compared by pointer.
    blockers: Option<Arc<[Blocker]>>,
    groups: Arc<[BlockerGroup]>,
    intro: SharedString,
    /// Shift is held: the row buttons read End Process and send SIGKILL.
    shift: bool,
    /// The scan could not read some other user's process.
    eacces_note: bool,
    /// End actions in flight; each one retries the eject once when it finishes.
    ending: SmallVec<[ActionId; 2]>,
    /// An automatic retry is waiting for its 500 ms.
    retry_pending: bool,
    words: Words,
    close_focus: FocusHandle,
    _subs: SmallVec<[Subscription; 3]>,
}

impl DiskInUseWindow {
    fn new(
        host: &HostContext,
        key: DeviceKey,
        idx: DeviceIdx,
        hub: Entity<SnapshotHub>,
        disk_label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let close_focus = cx.focus_handle();
        close_focus.focus(window, cx);
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, |this, hub, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::DEVICE_DETAILS) {
                let snap = hub.read(cx).current().clone();
                if this.sync(&snap, false, cx) {
                    cx.notify();
                }
            }
        }));
        if let Some(tracker) = host.tracker().cloned() {
            subs.push(
                cx.subscribe(&tracker, |this, _, ev: &ActionTrackerEvent, cx| {
                    if let ActionTrackerEvent::Finished { id, .. } = *ev {
                        this.on_finished(id, cx);
                    }
                }),
            );
        }
        subs.push(cx.observe_global::<Strings>(|this, cx| {
            this.words = Words::new(cx);
            let snap = this.hub.read(cx).current().clone();
            this.sync(&snap, true, cx);
            cx.notify();
        }));
        let mut this = Self {
            host: host.clone(),
            idx,
            key,
            hub: hub.clone(),
            disk_label,
            blockers: None,
            groups: Arc::from([]),
            intro: SharedString::default(),
            shift: window.modifiers().shift,
            eacces_note: false,
            ending: SmallVec::new(),
            retry_pending: false,
            words: Words::new(cx),
            close_focus,
            _subs: subs,
        };
        let snap = hub.read(cx).current().clone();
        this.sync(&snap, true, cx);
        this
    }

    /// Takes a new blocker list from the disk record. With `force` the texts are built again
    /// even when the list did not change. True when something drawn changed.
    fn sync(&mut self, snap: &Snapshot, force: bool, cx: &App) -> bool {
        let Some(EjectState::Blocked {
            blockers,
            others_may_hold,
        }) = disk_details(snap, self.idx).and_then(|d| d.eject.as_ref().map(|(_, s)| s))
        else {
            return false;
        };
        let same = self
            .blockers
            .as_ref()
            .is_some_and(|b| Arc::ptr_eq(b, blockers));
        if same && !force {
            return false;
        }
        self.blockers = Some(blockers.clone());
        self.groups = group_blockers(blockers, snap, &self.words.background).into();
        self.eacces_note = *others_may_hold;
        self.intro = tr_args(
            cx,
            "perf-diu-intro",
            &[
                ("disk", FluentValue::from(self.disk_label.to_string())),
                (
                    "model",
                    FluentValue::from(model_text(snap, self.idx).to_string()),
                ),
            ],
        );
        true
    }

    pub fn idx(&self) -> DeviceIdx {
        self.idx
    }

    pub fn groups(&self) -> &[BlockerGroup] {
        &self.groups
    }

    pub fn intro(&self) -> &SharedString {
        &self.intro
    }

    pub fn eacces_note(&self) -> bool {
        self.eacces_note
    }

    pub fn retry_pending(&self) -> bool {
        self.retry_pending
    }

    /// The label every row button shows now.
    pub fn row_label(&self) -> &SharedString {
        if self.shift {
            &self.words.end_process
        } else {
            &self.words.end_task
        }
    }

    pub fn close_focus(&self) -> &FocusHandle {
        &self.close_focus
    }

    /// Every listed process key, in list order.
    fn all_keys(&self) -> Vec<ProcessKey> {
        self.groups
            .iter()
            .flat_map(|g| g.rows.iter().map(|r| r.key))
            .collect()
    }

    /// Ends `keys`: End Process (SIGKILL) with `kill`, else End Task (SIGTERM), through the
    /// shared entry point. Each request it sends retries the eject once when it finishes.
    fn end(
        &mut self,
        keys: Vec<ProcessKey>,
        kill: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snap = self.hub.read(cx).current().clone();
        let targets = CapturedTargets::new(
            keys.iter()
                .filter_map(|k| snap.processes.get(*k))
                .map(record_target),
            None,
        );
        if targets.items.is_empty() {
            let text = toast_words(cx).exited.clone();
            toast(ToastKind::Warning, text, cx);
            return;
        }
        let this = cx.entity().downgrade();
        let on_sent: OnSent = Rc::new(move |id: ActionId, cx: &mut App| {
            let _ = this.update(cx, |w, _| w.ending.push(id));
        });
        let options = RunOptions {
            snapshot: Some(snap),
            on_sent: Some(on_sent),
        };
        let action: &dyn Action = if kill { &EndProcess } else { &EndTask };
        let host = self.host.clone();
        run_process_action_with(&host, action, targets, options, window, cx);
    }

    /// An end action finished: retry the eject 500 ms later, once.
    fn on_finished(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let before = self.ending.len();
        self.ending.retain(|a| *a != id);
        if self.ending.len() == before {
            return;
        }
        self.retry_pending = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(RETRY_DELAY).await;
            let _ = this.update(cx, |w, cx| {
                w.retry_pending = false;
                w.retry(cx);
            });
        })
        .detach();
    }

    /// Sends the eject again, unless one is in flight.
    fn retry(&mut self, cx: &mut Context<Self>) {
        if eject_in_flight(self.idx, cx) {
            return;
        }
        start_eject(
            &self.host,
            self.key.clone(),
            self.idx,
            self.hub.clone(),
            self.disk_label.clone(),
            cx,
        );
    }

    fn on_modifiers(&mut self, ev: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev.modifiers.shift != self.shift {
            self.shift = ev.modifiers.shift;
            cx.notify();
        }
    }

    fn render_group_header(
        i: usize,
        g: &BlockerGroup,
        theme: &TlTheme,
        m: &UiMetrics,
    ) -> AnyElement {
        let s = m.ui_scale;
        let icon_size = px(ICON * s);
        let fallback_color = theme.text_secondary;
        let fallback = move || {
            Icon::new(IconName::AppWindow)
                .size(icon_size)
                .text_color(fallback_color)
                .into_any_element()
        };
        let icon = g.icon.clone().map_or_else(fallback, |path| {
            img(ImageSource::from(Arc::<Path>::from(Path::new(
                path.as_ref(),
            ))))
            .size(icon_size)
            .with_fallback(fallback)
            .into_any_element()
        });
        let mut name = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(theme.text_primary);
        *name.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
        div()
            .debug_selector(move || format!("DiskInUseGroup-{i}"))
            .flex_none()
            .flex()
            .items_center()
            .gap(m.sp[3])
            .h(px(GROUP_ROW * s))
            .px(m.sp[3])
            .child(div().flex_none().child(icon))
            .child(name.child(g.name.clone()))
            .into_any_element()
    }

    fn render_row(
        &self,
        i: usize,
        row: &BlockerRow,
        theme: &TlTheme,
        m: &UiMetrics,
        this: WeakEntity<Self>,
    ) -> AnyElement {
        let s = m.ui_scale;
        let mut name = div()
            .flex_none()
            .max_w(px(160. * s))
            .truncate()
            .text_color(theme.text_primary);
        *name.text_style() = t_base(m);
        let mut pid = div()
            .flex_none()
            .w(px(PID_W * s))
            .whitespace_nowrap()
            .text_color(theme.text_secondary);
        *pid.text_style() = t_base(m);
        let mut paths = div()
            .id(("DiskInUsePaths", i))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_secondary)
            .tooltip(TlTooltip::text(row.tooltip.clone()).build());
        *paths.text_style() = t_xs(m);
        let key = row.key;
        let kill = self.shift;
        let button = TlButton::new(("DiskInUseEnd", i), TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .label(self.row_label().clone())
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |w, cx| w.end(vec![key], kill, window, cx));
            });
        div()
            .debug_selector(move || format!("DiskInUseRow-{i}"))
            .flex_none()
            .flex()
            .items_center()
            .gap(m.sp[3])
            .h(px(ROW * s))
            .pl(px(ICON * s) + m.sp[3] * 2.)
            .pr(m.sp[3])
            .child(name.child(row.name.clone()))
            .child(pid.child(row.pid.clone()))
            .child(paths.child(row.paths.clone()))
            .child(
                div()
                    .debug_selector(move || format!("DiskInUseEnd-{i}"))
                    .flex_none()
                    .child(button),
            )
            .into_any_element()
    }
}

impl Render for DiskInUseWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let this = cx.entity().downgrade();

        let mut list = div()
            .id("DiskInUseList")
            .debug_selector(|| "DiskInUseList".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .border_1()
            .border_color(theme.divider);
        let mut row_ix = 0;
        for (gi, g) in self.groups.iter().enumerate() {
            list = list.child(Self::render_group_header(gi, g, &theme, &m));
            for row in &g.rows {
                list = list.child(self.render_row(row_ix, row, &theme, &m, this.clone()));
                row_ix += 1;
            }
        }

        let mut intro = div()
            .debug_selector(|| "DiskInUseIntro".into())
            .text_color(theme.text_primary);
        *intro.text_style() = t_base(&m);

        let note = self.eacces_note.then(|| {
            let mut note = div()
                .debug_selector(|| "DiskInUseNote".into())
                .text_color(theme.text_secondary);
            *note.text_style() = t_sm(&m);
            note.child(self.words.other_users.clone())
        });

        let close = TlButton::new("DiskInUseClose", TlButtonKind::Secondary)
            .label(self.words.close.clone())
            .focus_handle(self.close_focus.clone())
            .on_click(|_, window, _| window.remove_window());
        let retry = {
            let this = this.clone();
            TlButton::new("DiskInUseRetry", TlButtonKind::Secondary)
                .label(self.words.retry.clone())
                .disabled(eject_in_flight(self.idx, cx))
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |w, cx| w.retry(cx));
                })
        };
        let kill = self.shift;
        let end_all = TlButton::new("DiskInUseEndAll", TlButtonKind::Danger)
            .label(self.words.end_all.clone())
            .disabled(self.groups.is_empty())
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |w, cx| {
                    let keys = w.all_keys();
                    w.end(keys, kill, window, cx);
                });
            });

        div()
            .id("DiskInUse")
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .size_full()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .p(m.sp[6])
            .bg(theme.window_bg)
            .child(intro.child(self.intro.clone()))
            .child(list)
            .children(note)
            .child(
                div()
                    .debug_selector(|| "DiskInUseFooter".into())
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(m.sp[3])
                    .child(
                        div()
                            .debug_selector(|| "DiskInUseClose".into())
                            .child(close),
                    )
                    .child(
                        div()
                            .debug_selector(|| "DiskInUseRetry".into())
                            .child(retry),
                    )
                    .child(
                        div()
                            .debug_selector(|| "DiskInUseEndAll".into())
                            .child(end_all),
                    ),
            )
    }
}
