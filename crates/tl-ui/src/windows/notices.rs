//! Process window notices (E23-A3, UI-PWN-036, UI-PWN-025, PROC-074, R-095, R-134): the
//! VirusTotal flow. E23-033 adds the dump toasts here in P3.
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

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, SharedString, Subscription, Task, WeakEntity,
    Window,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::command::Command;
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{DetailTab, DetailTarget, HashError, HashState, TabPayload};
use tl_model::ids::ProcessKey;
use tracing::warn;

use crate::action_tracker::{GlobalActionTracker, ToastTexts};
use crate::app::EngineGlobal;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::demand::GlobalDemand;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::settings_global::TlSettings;

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
