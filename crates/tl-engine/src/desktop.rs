//! The host's desktop session (ADR-018 "Commands that act on the host's desktop", D-038).
//!
//! Commands that act on the screen (open a file or URI, show a file in the file manager,
//! launch a program, Startup "Run now", the KWin bridge) need the desktop session's
//! environment. An engine in the window (`DesktopSource::Process`) runs inside that session, so
//! its own environment is the session's and children inherit it, as before the service. The
//! engine service (`DesktopSource::UserManager`) starts with the user manager, often before
//! anyone logs in, so its own environment lacks `WAYLAND_DISPLAY` and the rest. The KWin worker
//! watches KWin's name on the user bus; when KWin appears it reads the user manager's
//! environment, which Plasma fills at login, and stores the session's variables here. When KWin
//! leaves, the session is gone and the commands are refused with `NoDesktopSession`.
//!
//! The same handle keeps the pid and app id of each window connected to the service from this
//! host (`WindowGuard`). With the engine's own pid they are "self": the processes sweep marks
//! them `IS_SELF`, so ending them is refused as before the service (leader decision on
//! q4x76.12). The KWin worker finds a window's own windows by that pid and app id.
//!
//! It also carries Switch to window (`ActionKind::ActivateWindow`) from the action gate to the
//! KWin worker and the worker's answer back (`activate_window`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tl_model::actions::ActionError;

/// The variables taken from the desktop session. `XDG_ACTIVATION_TOKEN` is never taken
/// (ADR-018).
pub const SESSION_KEYS: [&str; 6] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
];

/// Every tasklens window carries this app ID as its `resourceClass` (R-086, APP-009).
pub const APP_ID: &str = "io.github.oysteinkrog.Tasklens";

/// A longer value is not taken.
const VALUE_MAX: usize = 4096;

/// `env` runs the program with the session's variables added.
pub const ENV_PATH: &str = "/usr/bin/env";

/// `env` reads every argument with `=` before the program as a variable, and has no way to stop
/// that. A program whose name holds `=` is started through this shell line, which runs its
/// arguments as they are: `exec "$@"` never reads a word as a variable.
pub const SH_PATH: &str = "/bin/sh";
const EXEC_ARGS: &str = "exec \"$@\"";

/// Switch to window calls waiting for the KWin worker. A full queue fails the call at once.
const ACTIVATE_QUEUE: usize = 8;

/// The session variables, in `SESSION_KEYS` order. A key with no usable value is left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopEnv {
    vars: Vec<(&'static str, String)>,
}

impl DesktopEnv {
    /// From `KEY=VALUE` lines, such as the user manager's `Environment` property. A later line
    /// for the same key wins, as in systemd. Empty values, values with control characters and
    /// values over 4 KiB are left out.
    pub fn from_lines<S: AsRef<str>>(lines: &[S]) -> Self {
        let mut found: [Option<&str>; SESSION_KEYS.len()] = [None; SESSION_KEYS.len()];
        for line in lines {
            let Some((key, value)) = line.as_ref().split_once('=') else {
                continue;
            };
            if let Some(i) = SESSION_KEYS.iter().position(|k| *k == key) {
                found[i] = Some(value);
            }
        }
        let vars = SESSION_KEYS
            .iter()
            .zip(found)
            .filter_map(|(key, value)| {
                let value = value?;
                usable(value).then(|| (*key, value.to_owned()))
            })
            .collect();
        Self { vars }
    }

    /// From this process's environment.
    pub fn from_process() -> Self {
        let lines: Vec<String> = SESSION_KEYS
            .iter()
            .filter_map(|key| std::env::var(key).ok().map(|v| format!("{key}={v}")))
            .collect();
        Self::from_lines(&lines)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    /// True when the session names a display: `WAYLAND_DISPLAY` or `DISPLAY`.
    pub fn has_display(&self) -> bool {
        self.get("WAYLAND_DISPLAY").is_some() || self.get("DISPLAY").is_some()
    }

    /// `argv` run through `/usr/bin/env` with the session's variables, so the program starts on
    /// the session's screen. `env` would read a program name that contains `=` as a variable,
    /// so such a program runs through `/bin/sh -c 'exec "$@"' sh`. An empty `argv`, or any
    /// `argv` when there are no variables, is returned as it is.
    pub fn wrap(&self, argv: Vec<String>) -> Vec<String> {
        let Some(program) = argv.first() else {
            return argv;
        };
        if self.vars.is_empty() {
            return argv;
        }
        let through_sh = program.contains('=');
        let mut out = Vec::with_capacity(4 + self.vars.len() + argv.len());
        out.push(ENV_PATH.to_owned());
        out.extend(self.vars.iter().map(|(k, v)| format!("{k}={v}")));
        if through_sh {
            out.extend([SH_PATH, "-c", EXEC_ARGS, "sh"].map(str::to_owned));
        }
        out.extend(argv);
        out
    }
}

fn usable(value: &str) -> bool {
    !value.is_empty() && value.len() <= VALUE_MAX && !value.chars().any(char::is_control)
}

/// Where the engine finds the desktop session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopSource {
    /// The engine runs in the window: its own environment is the session's, and the session is
    /// always there.
    Process,
    /// The engine service: the session comes and goes with KWin on the user bus, and its
    /// variables come from the user manager.
    UserManager,
}

/// The engine's view of the host's desktop session. Cheap to clone: clones share one state.
#[derive(Clone, Debug)]
pub struct DesktopSession {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    source: DesktopSource,
    /// `UserManager` only: the session's variables while there is a session.
    env: Mutex<Option<Arc<DesktopEnv>>>,
    /// The pid and app id of each window connected from this host, one entry per connection.
    windows: Mutex<Vec<(u32, Arc<str>)>>,
    /// True while the KWin worker serves its bridge and reads `activate_rx`.
    bridge_up: AtomicBool,
    activate_tx: async_channel::Sender<ActivateCall>,
    activate_rx: async_channel::Receiver<ActivateCall>,
}

/// One Switch to window, from the action gate to the KWin worker. The worker answers on
/// `reply`; dropping the call unanswered fails it.
#[derive(Debug)]
pub struct ActivateCall {
    /// The KWin window id, as in `WindowEntry::internal_id`.
    pub internal_id: Arc<str>,
    pub reply: async_channel::Sender<Result<(), ActionError>>,
}

impl DesktopSession {
    pub fn new(source: DesktopSource) -> Self {
        let (activate_tx, activate_rx) = async_channel::bounded(ACTIVATE_QUEUE);
        Self {
            shared: Arc::new(Shared {
                source,
                env: Mutex::new(None),
                windows: Mutex::new(Vec::new()),
                bridge_up: AtomicBool::new(false),
                activate_tx,
                activate_rx,
            }),
        }
    }

    pub fn source(&self) -> DesktopSource {
        self.shared.source
    }

    /// True while commands can act on the host's screen. Always true for `Process`.
    pub fn is_present(&self) -> bool {
        match self.shared.source {
            DesktopSource::Process => true,
            DesktopSource::UserManager => self.lock_env().is_some(),
        }
    }

    /// The variables a spawned program must be given. `None` for `Process`, whose children
    /// inherit them, and while there is no session.
    pub fn overrides(&self) -> Option<Arc<DesktopEnv>> {
        match self.shared.source {
            DesktopSource::Process => None,
            DesktopSource::UserManager => self.lock_env().clone(),
        }
    }

    /// One session variable: the session's value under `UserManager`, this process's otherwise.
    pub fn var(&self, key: &str) -> Option<String> {
        match self.shared.source {
            DesktopSource::Process => std::env::var(key).ok().filter(|v| usable(v)),
            DesktopSource::UserManager => self
                .lock_env()
                .as_ref()
                .and_then(|env| env.get(key).map(str::to_owned)),
        }
    }

    /// Records that the session started (`Some`) or ended (`None`). Ignored for `Process`.
    pub fn set(&self, env: Option<DesktopEnv>) {
        if self.shared.source == DesktopSource::Process {
            return;
        }
        let present = env.is_some();
        let mut slot = self.lock_env();
        if slot.is_some() != present {
            tracing::info!(target: "tasklens::engine", present, "desktop session changed");
        }
        *slot = env.map(Arc::new);
    }

    /// The pids of the windows connected from this host, sorted, each once.
    pub fn window_pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.lock_windows().iter().map(|(pid, _)| *pid).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// The app ids a window of `pid` may carry as its `resourceClass` for an own-window command
    /// (keep above, frame geometry): the app id each connection from `pid` announced in its
    /// `Hello`. The engine in the window (`Process`) also takes `APP_ID`, its own window's.
    pub fn own_app_ids(&self, pid: u32) -> Vec<Arc<str>> {
        let mut ids: Vec<Arc<str>> = self
            .lock_windows()
            .iter()
            .filter(|(p, _)| *p == pid)
            .map(|(_, app_id)| Arc::clone(app_id))
            .collect();
        if self.shared.source == DesktopSource::Process {
            ids.push(Arc::from(APP_ID));
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Adds the window `pid` with `app_id` until the guard is dropped.
    pub fn add_window(&self, pid: u32, app_id: Arc<str>) -> WindowGuard {
        self.lock_windows().push((pid, Arc::clone(&app_id)));
        WindowGuard {
            shared: Arc::clone(&self.shared),
            pid,
            app_id,
        }
    }

    /// Switch to window: asks the KWin worker to activate the window `internal_id` and waits
    /// for its answer. `Unsupported` when the worker does not serve its bridge (no KWin, or the
    /// bridge has not started yet). Never blocks on a full queue.
    pub async fn activate_window(&self, internal_id: Arc<str>) -> Result<(), ActionError> {
        if !self.shared.bridge_up.load(Ordering::Acquire) {
            return Err(ActionError::Unsupported);
        }
        let (reply, answer) = async_channel::bounded(1);
        let call = ActivateCall { internal_id, reply };
        if self.shared.activate_tx.try_send(call).is_err() {
            return Err(ActionError::Internal(Arc::from(
                "too many window switches waiting",
            )));
        }
        match answer.recv().await {
            Ok(result) => result,
            Err(_) => Err(ActionError::Internal(Arc::from(
                "the KWin bridge stopped before the switch",
            ))),
        }
    }

    /// The KWin worker's end of `activate_window`.
    pub(crate) fn activate_calls(&self) -> async_channel::Receiver<ActivateCall> {
        self.shared.activate_rx.clone()
    }

    /// The KWin worker serves its bridge until the guard is dropped. Dropping it fails every
    /// call still waiting in the queue.
    pub(crate) fn bridge_serving(&self) -> BridgeGuard {
        self.shared.bridge_up.store(true, Ordering::Release);
        BridgeGuard {
            shared: Arc::clone(&self.shared),
        }
    }

    fn lock_windows(&self) -> std::sync::MutexGuard<'_, Vec<(u32, Arc<str>)>> {
        self.shared
            .windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_env(&self) -> std::sync::MutexGuard<'_, Option<Arc<DesktopEnv>>> {
        self.shared
            .env
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// One connected window in `DesktopSession::window_pids`. Dropping it removes the window.
#[derive(Debug)]
pub struct WindowGuard {
    shared: Arc<Shared>,
    pid: u32,
    app_id: Arc<str>,
}

impl Drop for WindowGuard {
    fn drop(&mut self) {
        let mut windows = self
            .shared
            .windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = windows
            .iter()
            .position(|(p, a)| *p == self.pid && *a == self.app_id)
        {
            windows.swap_remove(i);
        }
    }
}

/// The KWin worker serves its bridge while this lives (`DesktopSession::bridge_serving`).
#[derive(Debug)]
pub(crate) struct BridgeGuard {
    shared: Arc<Shared>,
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        self.shared.bridge_up.store(false, Ordering::Release);
        // A dropped call fails its waiter.
        while self.shared.activate_rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_env() -> Vec<&'static str> {
        vec![
            "HOME=/home/u",
            "WAYLAND_DISPLAY=wayland-1",
            "DISPLAY=:0",
            "XDG_CURRENT_DESKTOP=KDE",
            "XDG_ACTIVATION_TOKEN=abc",
            "XAUTHORITY=",
            "XDG_SESSION_TYPE=wayland\u{1b}[31m",
            "DISPLAY=:1",
        ]
    }

    #[test]
    fn from_lines_takes_only_session_keys() {
        let env = DesktopEnv::from_lines(&manager_env());
        assert_eq!(env.get("WAYLAND_DISPLAY"), Some("wayland-1"));
        // The later line wins, as in systemd.
        assert_eq!(env.get("DISPLAY"), Some(":1"));
        assert_eq!(env.get("XDG_CURRENT_DESKTOP"), Some("KDE"));
        assert_eq!(env.get("HOME"), None);
        assert_eq!(env.get("XDG_ACTIVATION_TOKEN"), None);
        // Empty and control characters are left out.
        assert_eq!(env.get("XAUTHORITY"), None);
        assert_eq!(env.get("XDG_SESSION_TYPE"), None);
        assert!(env.has_display());
        assert!(!DesktopEnv::from_lines(&["HOME=/x"]).has_display());
    }

    #[test]
    fn wrap_adds_the_session_through_env() {
        let env = DesktopEnv::from_lines(&["WAYLAND_DISPLAY=wayland-1", "DISPLAY=:0"]);
        let argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), "a=b x".to_owned()];
        assert_eq!(
            env.wrap(argv.clone()),
            [
                ENV_PATH,
                "WAYLAND_DISPLAY=wayland-1",
                "DISPLAY=:0",
                "/bin/sh",
                "-c",
                "a=b x"
            ]
        );
        // `env` would take a program name with `=` as a variable, so it runs through `sh`,
        // and it still gets the session's variables.
        let odd = vec!["./a=b".to_owned(), "x=y".to_owned()];
        assert_eq!(
            env.wrap(odd),
            [
                ENV_PATH,
                "WAYLAND_DISPLAY=wayland-1",
                "DISPLAY=:0",
                SH_PATH,
                "-c",
                "exec \"$@\"",
                "sh",
                "./a=b",
                "x=y"
            ]
        );
        assert_eq!(DesktopEnv::default().wrap(argv.clone()), argv);
        assert!(env.wrap(Vec::new()).is_empty());
    }

    #[test]
    fn user_manager_session_comes_and_goes() {
        let session = DesktopSession::new(DesktopSource::UserManager);
        assert!(!session.is_present());
        assert!(session.overrides().is_none());
        assert_eq!(session.var("WAYLAND_DISPLAY"), None);
        session.set(Some(DesktopEnv::from_lines(&manager_env())));
        assert!(session.is_present());
        assert_eq!(session.var("WAYLAND_DISPLAY").as_deref(), Some("wayland-1"));
        assert!(session.overrides().is_some());
        session.set(None);
        assert!(!session.is_present());
    }

    #[test]
    fn process_session_is_always_there_and_never_overrides() {
        let session = DesktopSession::new(DesktopSource::Process);
        session.set(None);
        assert!(session.is_present());
        assert!(session.overrides().is_none());
    }

    #[test]
    fn window_pids_follow_their_guards() {
        let session = DesktopSession::new(DesktopSource::UserManager);
        let a = session.add_window(40, Arc::from(APP_ID));
        let b = session.add_window(30, Arc::from(APP_ID));
        let c = session.add_window(40, Arc::from(APP_ID));
        assert_eq!(session.window_pids(), [30, 40]);
        drop(a);
        assert_eq!(
            session.window_pids(),
            [30, 40],
            "one connection of 40 is left"
        );
        drop(c);
        drop(b);
        assert!(session.window_pids().is_empty());
    }

    /// q4x76.12: own-window commands match the app id each window announced in its `Hello`,
    /// not a fixed one. The engine in the window also takes its own `APP_ID`.
    #[test]
    fn own_app_ids_are_the_announced_ones() {
        let service = DesktopSession::new(DesktopSource::UserManager);
        assert!(service.own_app_ids(40).is_empty(), "no window connected");
        let dev = service.add_window(40, Arc::from("io.github.oysteinkrog.Tasklens.Devel"));
        let other = service.add_window(41, Arc::from(APP_ID));
        assert_eq!(
            service.own_app_ids(40),
            [Arc::from("io.github.oysteinkrog.Tasklens.Devel")]
        );
        drop(dev);
        assert!(service.own_app_ids(40).is_empty());
        drop(other);

        let window = DesktopSession::new(DesktopSource::Process);
        assert_eq!(window.own_app_ids(7), [Arc::from(APP_ID)]);
    }

    /// q4x76.12: Switch to window waits for the KWin worker's answer. With no bridge it fails
    /// at once; a call left in the queue when the bridge stops fails too.
    #[test]
    fn activate_window_waits_for_the_bridge() {
        let session = DesktopSession::new(DesktopSource::UserManager);
        let id: Arc<str> = Arc::from("{0a1b2c3d-0000-4000-8000-00000000000a}");
        let no_bridge = futures_lite::future::block_on(session.activate_window(Arc::clone(&id)));
        assert_eq!(no_bridge, Err(ActionError::Unsupported));

        let calls = session.activate_calls();
        let guard = session.bridge_serving();
        let worker = std::thread::spawn(move || {
            let call = calls.recv_blocking().expect("a call");
            call.reply
                .try_send(Err(ActionError::NoSuchProcess))
                .expect("reply");
            call.internal_id
        });
        let answer = futures_lite::future::block_on(session.activate_window(Arc::clone(&id)));
        assert_eq!(answer, Err(ActionError::NoSuchProcess));
        assert_eq!(worker.join().expect("worker"), id);

        // Nobody reads the queue: the bridge stops and the waiting call fails.
        let waiting = session.clone();
        let wait_id = Arc::clone(&id);
        let caller = std::thread::spawn(move || {
            futures_lite::future::block_on(waiting.activate_window(wait_id))
        });
        while session.shared.activate_rx.is_empty() {
            std::thread::yield_now();
        }
        drop(guard);
        assert!(matches!(
            caller.join().expect("caller"),
            Err(ActionError::Internal(_))
        ));
        assert_eq!(
            futures_lite::future::block_on(session.activate_window(id)),
            Err(ActionError::Unsupported)
        );
    }
}
