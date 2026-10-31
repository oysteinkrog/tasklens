//! `WindowKey` and `WindowRegistry`: the identity of every secondary window and the one place
//! that knows which are open (UI-FND-021, UI-SHL-090, APP-045, INDEX.md "Window keys", R-176,
//! R-218).
//!
//! The registry focuses an open window instead of opening a second one, enforces the per-kind
//! caps, drops a key when its window closes, and closes the children when the main window
//! closes. Every insert and every removal notifies the registry entity.

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, Global, Subscription};
use rustc_hash::FxHashMap;
use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
use tl_model::table::{StartupRef, UnitRef};

/// One secondary window. Confirmations, Set Affinity, Set Priority and Send Signal have no key
/// (modal children of their parent); the low-memory window has none (own process).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum WindowKey {
    /// Process Properties, cap 8 (PROC-077).
    Properties(ProcessKey),
    /// App Properties, cap 8.
    AppProperties(GroupKey),
    /// Service Properties, cap 10.
    Service(UnitRef),
    /// Startup Properties, cap 10.
    StartupProperties(StartupRef),
    /// Connection Properties by socket cookie, cap 10.
    ConnectionProperties(u64),
    Run,
    HandleSearch,
    Columns,
    Settings,
    ThemeEditor,
    ShareTheme,
    About,
    CrashReport,
    AdditionalSetup,
    /// The packaged `tasklens-setup` script of one subcommand (`SetupStep::args()[0]`).
    SetupScript(&'static str),
    Shortcuts,
    PublicIpNotice,
    CollectorHealth,
    Smart(DeviceIdx),
    DiskInUse(DeviceIdx),
    DetachedGraph(u8),
    Mini,
    /// The main window (R-176). Never stored in the registry and has no cap class.
    Main,
    /// A window opened from the remote host named by the first field: the key it would have on
    /// the local host, wrapped with the host name, so the same process, app, unit, startup
    /// entry or socket on two hosts gets two windows (ADR-018, leader decision Q2 on
    /// tasklens-q4x76.11). Build it with `WindowKey::on_host`; local keys are never wrapped.
    OnHost(Arc<str>, Box<WindowKey>),
}

/// The window kinds that have a cap on open windows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CapClass {
    ProcessProps,
    AppProps,
    ServiceProps,
    StartupProps,
    ConnectionProps,
}

pub const CAP_PROCESS_PROPS: usize = 8;
pub const CAP_APP_PROPS: usize = 8;
pub const CAP_SERVICE_PROPS: usize = 10;
pub const CAP_STARTUP_PROPS: usize = 10;
pub const CAP_CONNECTION_PROPS: usize = 10;

impl CapClass {
    /// The most windows of this class open at once.
    pub fn cap(self) -> usize {
        match self {
            Self::ProcessProps => CAP_PROCESS_PROPS,
            Self::AppProps => CAP_APP_PROPS,
            Self::ServiceProps => CAP_SERVICE_PROPS,
            Self::StartupProps => CAP_STARTUP_PROPS,
            Self::ConnectionProps => CAP_CONNECTION_PROPS,
        }
    }
}

impl WindowKey {
    /// This key for a window of the host whose remote name is `remote`: unchanged for the
    /// local host (`None`), wrapped in `OnHost` for a remote one. `Main` and a key that is
    /// already wrapped stay as they are.
    pub fn on_host(self, remote: Option<&Arc<str>>) -> Self {
        match (remote, self) {
            (_, key @ (Self::OnHost(..) | Self::Main)) | (None, key) => key,
            (Some(name), key) => Self::OnHost(Arc::clone(name), Box::new(key)),
        }
    }

    /// The key without its host: the inner key of `OnHost`, else this key.
    pub fn base(&self) -> &WindowKey {
        match self {
            Self::OnHost(_, key) => key.base(),
            key => key,
        }
    }

    /// The cap class of a Properties key; `None` for single windows and `Main`. The caps count
    /// the windows of every host together.
    pub fn cap_class(&self) -> Option<CapClass> {
        match self.base() {
            Self::Properties(_) => Some(CapClass::ProcessProps),
            Self::AppProperties(_) => Some(CapClass::AppProps),
            Self::Service(_) => Some(CapClass::ServiceProps),
            Self::StartupProperties(_) => Some(CapClass::StartupProps),
            Self::ConnectionProperties(_) => Some(CapClass::ConnectionProps),
            _ => None,
        }
    }
}

/// What `WindowRegistry::open_or_focus` did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenOutcome {
    /// The key was open; its window was activated.
    Focused,
    Opened,
    /// The class is full; nothing opened. The owner shows its toast (UI-PRC-015).
    CapReached(CapClass),
    /// The open closure failed. The owner shows its error toast.
    Failed,
}

/// Every open secondary window by key. `Main` is never stored.
pub struct WindowRegistry {
    windows: FxHashMap<WindowKey, AnyWindowHandle>,
    _closed: Subscription,
}

/// The app's registry. `TasklensUi::init` installs it.
pub struct GlobalRegistry(pub Entity<WindowRegistry>);

impl Global for GlobalRegistry {}

impl GlobalRegistry {
    /// Creates the registry and installs it as a Global.
    pub fn init(cx: &mut App) {
        let registry = cx.new(WindowRegistry::new);
        cx.set_global(Self(registry));
    }
}

impl WindowRegistry {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        // A window can close inside an update of this entity (`close` removes one), so the
        // removal is deferred to keep the update unnested.
        let _closed = cx.on_window_closed(move |cx, id| {
            let this = this.clone();
            cx.defer(move |cx| {
                let _ = this.update(cx, |this, cx| {
                    let before = this.windows.len();
                    this.windows.retain(|_, w| w.window_id() != id);
                    if this.windows.len() != before {
                        cx.notify();
                    }
                });
            });
        });
        Self {
            windows: FxHashMap::default(),
            _closed,
        }
    }

    /// Activates the window open for `key`, or opens one with `open` when the cap allows
    /// (UI-SHL-090, UI-FND-021). `open` runs inside this entity's update, so it must not read
    /// the registry.
    pub fn open_or_focus(
        &mut self,
        key: WindowKey,
        cx: &mut Context<Self>,
        open: impl FnOnce(&mut App) -> gpui_kit::Result<AnyWindowHandle>,
    ) -> OpenOutcome {
        if let Some(handle) = self.windows.get(&key).copied() {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return OpenOutcome::Focused;
            }
            // Closed but the close notice has not run yet.
            self.windows.remove(&key);
            cx.notify();
        }
        if let Some(class) = key.cap_class()
            && self.count(class) >= class.cap()
        {
            return OpenOutcome::CapReached(class);
        }
        match open(cx) {
            Ok(handle) => {
                if key != WindowKey::Main {
                    self.windows.insert(key, handle);
                    cx.notify();
                }
                OpenOutcome::Opened
            }
            Err(err) => {
                tracing::warn!(target: "tasklens::ui", ?key, %err, "could not open a window");
                OpenOutcome::Failed
            }
        }
    }

    pub fn get(&self, key: &WindowKey) -> Option<AnyWindowHandle> {
        self.windows.get(key).copied()
    }

    /// Removes `key` and closes its window.
    pub fn close(&mut self, key: &WindowKey, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.remove(key) {
            cx.notify();
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// Closes every registered window when the main window closes. With `keep_detached`
    /// (`graphs.keep_detached_on_close`) detached graphs stay open (APP-045).
    pub fn close_children_of_main(&mut self, keep_detached: bool, cx: &mut Context<Self>) {
        let keys: Vec<WindowKey> = self
            .windows
            .keys()
            .filter(|k| !(keep_detached && matches!(k.base(), WindowKey::DetachedGraph(_))))
            .cloned()
            .collect();
        for key in keys {
            self.close(&key, cx);
        }
    }

    /// Open windows of `class`.
    pub fn count(&self, class: CapClass) -> usize {
        self.count_where(|k| k.cap_class() == Some(class))
    }

    /// Whether any open window's key matches `pred` (R-218).
    pub fn has_key(&self, pred: impl Fn(&WindowKey) -> bool) -> bool {
        self.windows.keys().any(pred)
    }

    /// Open windows whose key matches `pred` (R-218).
    pub fn count_where(&self, pred: impl Fn(&WindowKey) -> bool) -> usize {
        self.windows.keys().filter(|k| pred(k)).count()
    }
}
