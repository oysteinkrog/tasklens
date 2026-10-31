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
//! The same handle keeps the pids of the windows connected to the service from this host
//! (`WindowGuard`). With the engine's own pid they are "self": the processes sweep marks them
//! `IS_SELF`, so ending them is refused as before the service (leader decision on q4x76.12).

use std::sync::{Arc, Mutex, PoisonError};

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
    /// the session's screen. `env` would read a first argument that contains `=` as a variable,
    /// so such an `argv` is returned as it is, as is any `argv` when there are no variables.
    pub fn wrap(&self, argv: Vec<String>) -> Vec<String> {
        let first_ok = argv.first().is_some_and(|a| !a.contains('='));
        if self.vars.is_empty() || !first_ok {
            return argv;
        }
        let mut out = Vec::with_capacity(1 + self.vars.len() + argv.len());
        out.push(ENV_PATH.to_owned());
        out.extend(self.vars.iter().map(|(k, v)| format!("{k}={v}")));
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
    /// The pids of the windows connected from this host, one entry per connection.
    windows: Mutex<Vec<u32>>,
}

impl DesktopSession {
    pub fn new(source: DesktopSource) -> Self {
        Self {
            shared: Arc::new(Shared {
                source,
                env: Mutex::new(None),
                windows: Mutex::new(Vec::new()),
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
        let mut pids = self
            .shared
            .windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Adds `pid` to the window pids until the guard is dropped.
    pub fn add_window(&self, pid: u32) -> WindowGuard {
        self.shared
            .windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(pid);
        WindowGuard {
            shared: Arc::clone(&self.shared),
            pid,
        }
    }

    fn lock_env(&self) -> std::sync::MutexGuard<'_, Option<Arc<DesktopEnv>>> {
        self.shared
            .env
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// One connected window's pid in `DesktopSession::window_pids`. Dropping it removes the pid.
#[derive(Debug)]
pub struct WindowGuard {
    shared: Arc<Shared>,
    pid: u32,
}

impl Drop for WindowGuard {
    fn drop(&mut self) {
        let mut pids = self
            .shared
            .windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = pids.iter().position(|p| *p == self.pid) {
            pids.swap_remove(i);
        }
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
        // `env` would take a first argument with `=` as a variable.
        let odd = vec!["a=b".to_owned()];
        assert_eq!(env.wrap(odd.clone()), odd);
        assert_eq!(DesktopEnv::default().wrap(argv.clone()), argv);
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
        let a = session.add_window(40);
        let b = session.add_window(30);
        let c = session.add_window(40);
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
}
