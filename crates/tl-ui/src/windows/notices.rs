//! Process window notices (E23-A3, UI-PWN-036, UI-PWN-025, PROC-074, R-095, R-134): the
//! VirusTotal flow, and the dump flow (UI-PWN-034, UI-PWN-035, PROC-075, R-119, R-224).
//!
//! [`open_virustotal`] shows the one-time notice (UI-PRC-014 "Open VirusTotal?") until
//! `state.processes.virustotal_notice_accepted` is true. It then asks the engine for the hashes
//! (`Command::ComputeHashes`) and holds a General detail demand for the process while it waits.
//! An Info toast "Computing SHA-256…" shows only if the hash takes longer than 500 ms. When the
//! General payload's hash is `Done`, it removes the toast and sends
//! `ActionKind::OpenUri("https://www.virustotal.com/gui/file/<sha256>")` through the action
//! tracker. Errors replace the toast.
//!
//! Only the SHA-256 leaves the machine, as part of the URL the browser opens. tl-ui hashes
//! nothing and makes no network contact itself; the engine hashes and opens the browser.
//!
//! [`start_dump`] asks first when VmRSS is over 2 GiB, then sends `ActionKind::Dump` with a
//! path the UI picks under `<state_dir>/dumps/`. While the action runs, the Info toast
//! `DumpToast(action_id)` shows the file size, read with one `stat` every 500 ms off the UI
//! thread. The result replaces that toast. The UI never reads the core file: it holds process
//! memory.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, BorrowAppContext as _, Context, Entity, Global, Pixels,
    SharedString, Subscription, Task, WeakEntity, Window, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{
    ActionError, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, DumpMode,
    RejectReason,
};
use tl_model::command::Command;
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{DetailTab, DetailTarget, HashError, HashState, TabPayload};
use tl_model::ids::ProcessKey;
use tl_model::units;
use tracing::warn;

use crate::action_tracker::{
    ActionTracker, ActionTrackerEvent, GlobalActionTracker, ToastTexts, action_words,
};
use crate::app::{AppEnv, EngineGlobal};
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::toast::{self, ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::demand::GlobalDemand;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{tr, tr_args};
use crate::pages::processes::confirm::{self, ConfirmKind};
use crate::pages::startup::delegate::home_tilde;
use crate::settings_global::TlSettings;
use crate::text::ellipsis::middle_ellipsis;
use crate::text::metrics::TextMetricsCache;
use crate::theme::{UiMetrics, metrics};
use crate::windows::actions::OpenDumpFolder;

/// How long the hash may take before the "Computing SHA-256…" toast shows.
pub const TOAST_DELAY: Duration = Duration::from_millis(500);

/// The VirusTotal file page; the lowercase hex SHA-256 follows.
pub const VT_FILE_URL: &str = "https://www.virustotal.com/gui/file/";

/// The toast keys of the VirusTotal flow sit in their own range.
const VT_KEY_BASE: u64 = 0x8000_0000_0400_0000;

const TITLE: &str = "processes-confirm-virustotal-title";
const TEXT: &str = "processes-confirm-virustotal-text";
const OPEN: &str = "processes-confirm-virustotal-button";
const CANCEL: &str = "shell-confirm-cancel";
const COMPUTING: &str = "pwn-notices-vt-computing";
const HASH_FAILED: &str = "pwn-notices-vt-hash-failed";
const NO_IMAGE: &str = "pwn-notices-vt-no-image";
const BROWSER_FAILED: &str = "pwn-notices-vt-browser-failed";

/// The toast key of one VirusTotal request: the hashing toast and the error that replaces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VtToast(pub u64);

impl From<VtToast> for ToastKey {
    fn from(key: VtToast) -> Self {
        ToastKey(VT_KEY_BASE | (key.0 & 0xFF_FFFF))
    }
}

/// Opens `key`'s executable on VirusTotal by its SHA-256, after the one-time notice. `window` is
/// the caller's window: the notice is modal to it and the toasts show in it.
pub fn open_virustotal(key: ProcessKey, window: &mut Window, cx: &mut App) {
    if TlSettings::state(cx).processes.virustotal_notice_accepted {
        start(key, window, cx);
        return;
    }
    let dialog = TlDialog::confirm(tr(cx, TEXT), tr(cx, OPEN))
        .title(tr(cx, TITLE))
        .cancel_label(tr(cx, CANCEL));
    let opened = dialog.open(window, cx, move |result, window, cx| {
        if let DialogResult::Confirmed { .. } = result {
            TlSettings::update(cx, |d| d.state.processes.virustotal_notice_accepted = true);
            start(key, window, cx);
        }
    });
    if opened.is_err() {
        warn!(target: "tasklens::ui", "the VirusTotal notice did not open");
    }
}

/// The requests waiting for their hash. Each removes itself when it ends.
#[derive(Default)]
struct VtRequests {
    next: u64,
    live: Vec<Entity<VtRequest>>,
}

impl Global for VtRequests {}

fn start(key: ProcessKey, window: &mut Window, cx: &mut App) {
    let Some(queue) = ToastQueue::of_window(window, cx) else {
        warn!(target: "tasklens::ui", "no toast queue; VirusTotal was not opened");
        return;
    };
    let requests = cx.default_global::<VtRequests>();
    requests.next += 1;
    let toast = VtToast(requests.next);
    let handle = window.window_handle();
    let request = cx.new(|cx| VtRequest::new(key, toast, queue.downgrade(), handle, cx));
    cx.default_global::<VtRequests>().live.push(request.clone());
    // The hash may already be in the snapshot (an open Properties window computed it).
    request.update(cx, |r, cx| r.check(cx));
}

/// One VirusTotal request, from `ComputeHashes` to the browser open or an error.
pub struct VtRequest {
    key: ProcessKey,
    toast: VtToast,
    queue: WeakEntity<ToastQueue>,
    toast_shown: bool,
    done: bool,
    _timer: Option<Task<()>>,
    _hub: Option<Subscription>,
}

impl VtRequest {
    fn new(
        key: ProcessKey,
        toast: VtToast,
        queue: WeakEntity<ToastQueue>,
        window: gpui_kit::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(engine) = cx.try_global::<EngineGlobal>()
            && let Err(err) = engine.0.send(Command::ComputeHashes(key))
        {
            warn!(target: "tasklens::ui", ?err, "ComputeHashes not sent");
        }
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let part = Demand {
                details: Arc::from([DetailDemand {
                    target: DetailTarget::Process(key),
                    tab: DetailTab::General,
                }]),
                ..Demand::default()
            };
            let id = cx.entity_id();
            demand.update(cx, |d, cx| d.set(id, Some(window), part, cx));
        }
        let hub = cx
            .try_global::<GlobalHub>()
            .map(|h| h.0.clone())
            .map(|hub| {
                cx.subscribe(&hub, |this: &mut Self, _, ev: &SnapshotChanged, cx| {
                    if ev.mask.intersects(ComponentMask::DETAILS) {
                        this.check(cx);
                    }
                })
            });
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DELAY).await;
            let _ = this.update(cx, |this, cx| this.show_toast(cx));
        });
        Self {
            key,
            toast,
            queue,
            toast_shown: false,
            done: false,
            _timer: Some(timer),
            _hub: hub,
        }
    }

    /// "Computing SHA-256…", once the hash is late.
    fn show_toast(&mut self, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        self.toast_shown = true;
        let text = tr(cx, COMPUTING);
        self.push(ToastKind::Info, text, cx);
    }

    fn push(&self, kind: ToastKind, text: SharedString, cx: &mut Context<Self>) {
        let key = self.toast.into();
        if let Some(queue) = self.queue.upgrade() {
            queue.update(cx, |q, cx| q.push(kind, text, Some(key), None, cx));
        }
    }

    /// Acts on the General payload's hash state, if it has one.
    fn check(&mut self, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        let Some(state) = hash_state(self.key, cx) else {
            return;
        };
        match state {
            HashState::Pending => {}
            HashState::Done { sha256, .. } => {
                if self.toast_shown
                    && let Some(queue) = self.queue.upgrade()
                {
                    let key = self.toast.into();
                    queue.update(cx, |q, cx| q.remove(key, cx));
                }
                self.open_url(&sha256, cx);
                self.finish(cx);
            }
            HashState::Failed(err) => {
                let text = match err {
                    HashError::ReadFailed { .. } => tr(cx, HASH_FAILED),
                    HashError::ExePathUnavailable => tr(cx, NO_IMAGE),
                };
                self.push(ToastKind::Error, text, cx);
                self.finish(cx);
            }
        }
    }

    fn open_url(&self, sha256: &[u8; 32], cx: &mut Context<Self>) {
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
            warn!(target: "tasklens::ui", "no action tracker; VirusTotal was not opened");
            return;
        };
        let mut texts = ToastTexts::new("", tr(cx, BROWSER_FAILED));
        texts.toast_key = Some(self.toast.into());
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::OpenUri(vt_url(sha256).into()),
            targets: SmallVec::new(),
            privileged: false,
        };
        let queue = self.queue.clone();
        tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue, texts, cx);
        });
    }

    /// Drops the timer, the hub subscription and the demand, and forgets the request.
    fn finish(&mut self, cx: &mut Context<Self>) {
        self.done = true;
        self._timer = None;
        self._hub = None;
        let id = cx.entity_id();
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            demand.update(cx, |d, cx| d.remove(id, cx));
        }
        cx.defer(move |cx| {
            if cx.has_global::<VtRequests>() {
                cx.global_mut::<VtRequests>()
                    .live
                    .retain(|r| r.entity_id() != id);
            }
        });
    }
}

/// The hash state of `key`'s General payload in the current snapshot.
fn hash_state(key: ProcessKey, cx: &App) -> Option<HashState> {
    let hub = cx.try_global::<GlobalHub>()?.0.read(cx);
    let detail = hub
        .current()
        .details
        .targets
        .get(&DetailTarget::Process(key))?;
    match detail.tab(DetailTab::General).payload.as_deref()? {
        TabPayload::General(g) => Some(g.hashes.clone()),
        _ => None,
    }
}

/// `https://www.virustotal.com/gui/file/` and the lowercase hex of `sha256`.
pub fn vt_url(sha256: &[u8; 32]) -> String {
    let mut url = String::with_capacity(VT_FILE_URL.len() + 64);
    url.push_str(VT_FILE_URL);
    for b in sha256 {
        let _ = write!(url, "{b:02x}");
    }
    url
}

/// Over this VmRSS the dump asks first (UI-PRC-014 "Create dump file, VmRSS > 2 GiB").
pub const LARGE_DUMP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How often the progress toast reads the size of the core file.
pub const DUMP_POLL: Duration = Duration::from_millis(500);

/// The longest `{name}` part of a dump file name, in bytes (R-224).
pub const DUMP_NAME_MAX: usize = 64;

/// The toast keys of the dump flow sit in their own range.
const DUMP_KEY_BASE: u64 = 0x8000_0000_0500_0000;

/// The close button beside the toast text, before the UI scale.
const TOAST_CLOSE_W: f32 = 24.0;

const DUMP_WRITING: &str = "pwn-notices-dump-writing";
const DUMP_WRITING_SIZE: &str = "pwn-notices-dump-writing-size";
const DUMP_CREATED: &str = "pwn-notices-dump-created";
const DUMP_OPEN_FOLDER: &str = "pwn-notices-dump-open-folder";
const DUMP_OPENING: &str = "pwn-notices-dump-opening";
const DUMP_FAILED: &str = "pwn-notices-dump-failed";
const DUMP_NEEDS_ADMIN: &str = "pwn-notices-dump-needs-admin";

/// The toast key of one dump, by its action id: the progress toast and the result that
/// replaces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DumpToast(pub u64);

impl From<DumpToast> for ToastKey {
    fn from(key: DumpToast) -> Self {
        ToastKey(DUMP_KEY_BASE | (key.0 & 0xFF_FFFF))
    }
}

/// Creates a core dump of `key` (PROC-075). Asks first when VmRSS is over 2 GiB. `window` is
/// the caller's window: the confirmation is modal to it and the toasts show in it.
pub fn start_dump(key: ProcessKey, window: &mut Window, cx: &mut App) {
    let (name, rss) = name_and_rss(key, cx);
    if rss <= LARGE_DUMP_BYTES {
        send_dump(key, &name, window, cx);
        return;
    }
    let mut size = String::new();
    units::bytes(&mut size, rss);
    let kind = ConfirmKind::LargeDump {
        name: name.clone(),
        size: size.into(),
    };
    let dialog = confirm::dialog(confirm::texts(&kind, cx));
    let opened = dialog.open(window, cx, move |result, window, cx| {
        if let DialogResult::Confirmed { .. } = result {
            send_dump(key, &name, window, cx);
        }
    });
    if opened.is_err() {
        warn!(target: "tasklens::ui", "the large dump confirmation did not open");
    }
}

/// The display name and VmRSS of `key` in the current snapshot. A process that is gone has an
/// empty name and 0 bytes; the engine then reports that it has exited.
fn name_and_rss(key: ProcessKey, cx: &App) -> (SharedString, u64) {
    cx.try_global::<GlobalHub>()
        .and_then(|hub| {
            let record = hub.0.read(cx).current().processes.get(key)?;
            Some((
                SharedString::from(record.meta.name.to_string()),
                record.rss_bytes,
            ))
        })
        .unwrap_or_default()
}

/// The `{name}` part of a dump file name (R-224). Every char outside `[A-Za-z0-9._-]` becomes
/// `_`, and a run of them one `_`, because the engine writes no other file name
/// (`valid_dump_path`). Cut to 64 bytes; `process` when empty.
pub fn dump_file_stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len().min(DUMP_NAME_MAX));
    let mut replaced = false;
    for ch in name.chars() {
        if out.len() == DUMP_NAME_MAX {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
            replaced = false;
        } else if !replaced {
            out.push('_');
            replaced = true;
        }
    }
    if out.is_empty() {
        out.push_str("process");
    }
    out
}

/// `<dir>/{name}_{pid}_{yyyymmdd_hhmmss}.core` (UI-PWN-034). `at` is local time.
pub fn dump_path(dir: &Path, name: &str, pid: u32, at: jiff::civil::DateTime) -> String {
    let file = format!(
        "{}_{pid}_{}.core",
        dump_file_stem(name),
        at.strftime("%Y%m%d_%H%M%S")
    );
    dir.join(file).to_string_lossy().into_owned()
}

/// The path as the success toast shows it: `$HOME` as `~`.
pub fn shown_dump_path(path: &str, home: &str) -> String {
    home_tilde(path, home).unwrap_or_else(|| path.to_owned())
}

/// The running dumps and the last finished one. `OpenDumpFolder` has no payload, so "Open
/// folder" shows the last dump that succeeded; every dump is in the same folder.
#[derive(Default)]
struct DumpFlows {
    live: Vec<Entity<DumpRequest>>,
    last_done: Option<DoneDump>,
    folder_action: bool,
}

impl Global for DumpFlows {}

#[derive(Clone)]
struct DoneDump {
    path: Arc<str>,
    queue: WeakEntity<ToastQueue>,
}

fn send_dump(key: ProcessKey, name: &str, window: &mut Window, cx: &mut App) {
    let Some(queue) = ToastQueue::of_window(window, cx) else {
        warn!(target: "tasklens::ui", "no toast queue; the dump was not started");
        return;
    };
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "no action tracker; the dump was not started");
        return;
    };
    let dir = cx
        .try_global::<AppEnv>()
        .map(|env| env.state_dir.join("dumps"))
        .unwrap_or_default();
    let at = jiff::Zoned::now().datetime();
    let path: Arc<str> = dump_path(&dir, name, key.pid, at).into();
    let mode: DumpMode = TlSettings::settings(cx).processes.dump_type;
    let id = tracker.read(cx).next_id();
    let request = ActionRequest {
        id,
        kind: ActionKind::Dump {
            mode,
            path: path.clone(),
        },
        targets: smallvec![ActionTarget::Process(key)],
        privileged: false,
    };
    let mut texts = ToastTexts::new(SharedString::default(), SharedString::default());
    texts.owner_toasts = true;
    texts.toast_key = Some(DumpToast(id.0).into());
    let sent = tracker.update(cx, |t, cx| {
        t.track_or_toast(request, queue.downgrade(), texts, cx)
    });
    if sent.is_none() {
        return;
    }
    let handle = window.window_handle();
    let dump = cx.new(|cx| DumpRequest::new(id, path, queue.downgrade(), handle, &tracker, cx));
    let flows = cx.default_global::<DumpFlows>();
    flows.live.push(dump);
    if !flows.folder_action {
        flows.folder_action = true;
        App::on_action(cx, open_dump_folder);
    }
}

/// "Open folder": FileManager1 `ShowItems` with the core file selected, then the Info toast
/// "Opening file location". A failure shows the tracker's reason.
fn open_dump_folder(_: &OpenDumpFolder, cx: &mut App) {
    let Some(done) = cx
        .try_global::<DumpFlows>()
        .and_then(|flows| flows.last_done.clone())
    else {
        return;
    };
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "no action tracker; the dump folder was not opened");
        return;
    };
    let request = ActionRequest {
        id: ActionId(0),
        kind: ActionKind::ShowInFileManager {
            path: done.path,
            properties: false,
        },
        targets: SmallVec::new(),
        privileged: false,
    };
    let texts = ToastTexts::new(SharedString::default(), SharedString::default());
    let queue = done.queue;
    let sent = tracker.update(cx, |t, cx| {
        t.track_or_toast(request, queue.clone(), texts, cx)
    });
    if sent.is_some()
        && let Some(queue) = queue.upgrade()
    {
        let text = tr(cx, DUMP_OPENING);
        queue.update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
    }
}

/// One dump, from the request to its result toast. Its `DumpProgress` task reads the file
/// size every 500 ms and ends with the action.
pub struct DumpRequest {
    id: ActionId,
    path: Arc<str>,
    queue: WeakEntity<ToastQueue>,
    window: AnyWindowHandle,
    /// The last size read; `None` before the first read.
    size: Option<u64>,
    done: bool,
    _progress: Option<Task<()>>,
    _tracker: Option<Subscription>,
}

impl DumpRequest {
    fn new(
        id: ActionId,
        path: Arc<str>,
        queue: WeakEntity<ToastQueue>,
        window: AnyWindowHandle,
        tracker: &Entity<ActionTracker>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(
            tracker,
            |this: &mut Self, tracker, ev: &ActionTrackerEvent, cx| {
                if let ActionTrackerEvent::Finished { id, .. } = *ev
                    && id == this.id
                {
                    let state = tracker
                        .read(cx)
                        .results(id)
                        .and_then(|states| states.first())
                        .map(|(_, state)| state.clone());
                    this.finish(state, cx);
                }
            },
        );
        let file = path.clone();
        let progress = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DUMP_POLL).await;
                let file = file.clone();
                let size = cx
                    .background_executor()
                    .spawn(async move { std::fs::metadata(&*file).map(|m| m.len()).ok() })
                    .await;
                if this.update(cx, |this, cx| this.on_size(size, cx)).is_err() {
                    return;
                }
            }
        });
        let this = Self {
            id,
            path,
            queue,
            window,
            size: None,
            done: false,
            _progress: Some(progress),
            _tracker: Some(subscription),
        };
        this.show_progress(cx);
        this
    }

    fn on_size(&mut self, size: Option<u64>, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        if size.is_some() {
            self.size = size;
        }
        self.show_progress(cx);
    }

    /// "Writing core…", then "Writing core… 1.20 GiB" once a size was read. Pushed on every
    /// read: each push restarts the toast's timer, so the Info toast stays while the dump runs.
    fn show_progress(&self, cx: &mut Context<Self>) {
        let text = match self.size {
            None => tr(cx, DUMP_WRITING),
            Some(n) => {
                let mut size = String::new();
                units::bytes(&mut size, n);
                tr_args(cx, DUMP_WRITING_SIZE, &[("size", FluentValue::from(size))])
            }
        };
        self.push(ToastKind::Info, text, None, cx);
    }

    fn push(
        &self,
        kind: ToastKind,
        text: SharedString,
        action: Option<ToastAction>,
        cx: &mut Context<Self>,
    ) {
        let key = DumpToast(self.id.0).into();
        if let Some(queue) = self.queue.upgrade() {
            queue.update(cx, |q, cx| q.push(kind, text, Some(key), action, cx));
        }
    }

    /// Stops the size reads and replaces the progress toast with the result. `None` is a
    /// request that ended with no target result (the engine stopped).
    fn finish(&mut self, state: Option<ActionState>, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        self.done = true;
        self._progress = None;
        self._tracker = None;
        match state {
            Some(ActionState::Succeeded) => self.show_success(cx),
            Some(
                ActionState::Failed(ActionError::PermissionDenied)
                | ActionState::Rejected(RejectReason::NotOwner),
            ) => {
                let text = tr(cx, DUMP_NEEDS_ADMIN);
                self.push(ToastKind::Warning, text, None, cx);
            }
            other => {
                let words = action_words(cx);
                let reason = match &other {
                    Some(ActionState::Failed(err)) => words.message(err),
                    Some(ActionState::Rejected(reason)) => words.reject(*reason).1,
                    _ => words.outcome_unknown.clone(),
                };
                let text = format!("{}\n{reason}", tr(cx, DUMP_FAILED));
                self.push(ToastKind::Error, text.into(), None, cx);
            }
        }
        let id = cx.entity_id();
        cx.defer(move |cx| {
            if cx.has_global::<DumpFlows>() {
                cx.global_mut::<DumpFlows>()
                    .live
                    .retain(|r| r.entity_id() != id);
            }
        });
    }

    /// "Dump created: ~/…/name_1_20261003_101500.core" with "Open folder".
    fn show_success(&self, cx: &mut Context<Self>) {
        let shown = shown_dump_path(&self.path, &home_dir(cx));
        let shown = fit_path(self.window, &shown, cx);
        let text = tr_args(
            cx,
            DUMP_CREATED,
            &[("path", FluentValue::from(shown.to_string()))],
        );
        let button = ToastAction {
            label: tr(cx, DUMP_OPEN_FOLDER),
            action: Box::new(OpenDumpFolder),
        };
        let done = DoneDump {
            path: self.path.clone(),
            queue: self.queue.clone(),
        };
        cx.default_global::<DumpFlows>().last_done = Some(done);
        self.push(ToastKind::Success, text, Some(button), cx);
    }
}

/// `$HOME`, for the `~` form of the path.
fn home_dir(cx: &App) -> String {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(home) = cx.try_global::<TestHome>() {
        return home.0.clone();
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = cx;
    std::env::var("HOME").unwrap_or_default()
}

#[cfg(any(test, feature = "test-hooks"))]
struct TestHome(String);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for TestHome {}

/// Makes the dump toasts shorten paths under `home` instead of `$HOME`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_home(cx: &mut App, home: &str) {
    cx.set_global(TestHome(home.to_owned()));
}

/// The width the path may take in the success toast: the toast's text column (the toast less
/// its padding, icon, gaps, close button and border) on every line but the first, which holds
/// "Dump created: ". A longer path loses its middle and keeps the file name.
fn path_budget(m: &UiMetrics) -> Pixels {
    let column =
        px(toast::WIDTH) - m.sp[4] * 4.0 - m.icon.md - px(TOAST_CLOSE_W * m.ui_scale) - px(2.0);
    column * (toast::MAX_LINES - 1) as f32
}

/// `shown` cut in the middle to [`path_budget`], measured in `window`. Whole when the window is
/// gone.
fn fit_path(window: AnyWindowHandle, shown: &str, cx: &mut App) -> SharedString {
    let m = *metrics(cx);
    let budget = path_budget(&m);
    let fitted = window.update(cx, |_, window, cx| {
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            let mut width_of = |s: &str| {
                s.chars().fold(px(0.), |sum, ch| {
                    sum + cache.char_advance(ch, m.text.base, window, cx)
                })
            };
            middle_ellipsis(shown, budget, &mut width_of)
        })
    });
    fitted.unwrap_or_else(|_| SharedString::from(shown.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's file name check (`valid_dump_path` in tl-engine): `^[A-Za-z0-9._-]+\.core$`.
    fn engine_accepts(file: &str) -> bool {
        file.len() > ".core".len()
            && file.ends_with(".core")
            && file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    }

    fn at() -> jiff::civil::DateTime {
        jiff::civil::date(2026, 10, 3).at(10, 15, 0, 0)
    }

    #[test]
    fn dump_path_format() {
        let dir = Path::new("/home/u/.local/state/tasklens/dumps");
        assert_eq!(
            dump_path(dir, "x", 1, at()),
            "/home/u/.local/state/tasklens/dumps/x_1_20261003_101500.core"
        );
        assert_eq!(
            dump_path(dir, "", 42, jiff::civil::date(2026, 1, 2).at(3, 4, 5, 0)),
            "/home/u/.local/state/tasklens/dumps/process_42_20260102_030405.core"
        );
    }

    #[test]
    fn dump_file_stem_rules() {
        assert_eq!(dump_file_stem("Isolated Web Co"), "Isolated_Web_Co");
        assert_eq!(dump_file_stem("kworker/0:1H"), "kworker_0_1H");
        assert_eq!(dump_file_stem("a / :b"), "a_b");
        assert_eq!(dump_file_stem("Блокнот"), "_");
        assert_eq!(dump_file_stem("tab\there\0x"), "tab_here_x");
        assert_eq!(dump_file_stem("ok.name_v-2"), "ok.name_v-2");
        assert_eq!(dump_file_stem(""), "process");
        let long = "a".repeat(100);
        assert_eq!(dump_file_stem(&long).len(), DUMP_NAME_MAX);
        let wide = "é".repeat(100);
        assert_eq!(dump_file_stem(&wide), "_");
    }

    #[test]
    fn dump_names_pass_the_engine_check() {
        let dir = Path::new("/s/tasklens/dumps");
        for name in [
            "Isolated Web Co",
            "kworker/0:1H",
            "Блокнот",
            "日本語のアプリ",
            "",
            "..",
            "/",
            "\u{7}bell",
        ] {
            let path = dump_path(dir, name, 4321, at());
            let (parent, file) = path.rsplit_once('/').expect("a file name");
            assert_eq!(parent, "/s/tasklens/dumps", "{name:?}");
            assert!(engine_accepts(file), "{name:?} gave {file:?}");
        }
    }

    #[test]
    fn shown_path_uses_tilde() {
        assert_eq!(
            shown_dump_path(
                "/home/u/.local/state/tasklens/dumps/x_1_20261003_101500.core",
                "/home/u"
            ),
            "~/.local/state/tasklens/dumps/x_1_20261003_101500.core"
        );
        assert_eq!(shown_dump_path("/var/x.core", "/home/u"), "/var/x.core");
        assert_eq!(
            shown_dump_path("/home/user2/x.core", "/home/u"),
            "/home/user2/x.core"
        );
    }

    #[test]
    fn dump_toast_keys_in_their_own_range() {
        let key = ToastKey::from(DumpToast(7));
        assert_eq!(key, ToastKey(0x8000_0000_0500_0007));
        assert_ne!(key, ToastKey::from(VtToast(7)));
    }
}
