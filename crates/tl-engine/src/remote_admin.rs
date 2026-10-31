//! Whether remote admin is on for this engine (ADR-018 Remote root actions, Privilege coverage).
//!
//! The host setting `remote_admin` alone does not turn it on. When the setting turns on (at
//! start, or on a later change), the engine asks polkit with `CheckAuthorization`, without user
//! interaction, whether this process may run the tasklens helper action. That is the grant a
//! remote session's root actions will use, so the answer shows whether the rule that
//! `tasklens remote-admin on` writes is in place. A user process cannot read
//! `/etc/polkit-1/rules.d`, so the engine never looks at the file itself.
//!
//! Remote admin is on only while the setting is on and polkit said yes. Until polkit answers, and
//! when it says no or cannot be asked, remote admin is off and the reason is logged. The result
//! is kept in memory only: `engine.toml` is never rewritten from this check. A new check runs on
//! the next change of the setting.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use zbus::zvariant::Value;

/// The polkit action of the tasklens helper. Remote sessions' root actions run through it.
pub const HELPER_ACTION: &str = "io.github.oysteinkrog.tasklens.helper";
/// How long the polkit call may take before the check counts as failed.
const POLKIT_TIMEOUT: Duration = Duration::from_secs(5);
const LOG: &str = "tasklens::engine";

/// What polkit said about `HELPER_ACTION` for this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolkitAnswer {
    /// Allowed without a password: the remote admin rule is in place.
    Allowed,
    /// Not allowed, or only after someone types a password.
    NotAllowed,
    /// polkit could not be asked. The text is the D-Bus error name, or zbus's error text.
    Failed(Arc<str>),
}

/// Asks polkit about `HELPER_ACTION`. Tests pass a fake.
pub trait PolkitCheck: Send + Sync + 'static {
    /// Blocks until polkit answers or the call fails. Never shows a prompt.
    fn helper_allowed(&self) -> PolkitAnswer;
}

/// polkit on the system bus, with this process as the subject.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPolkit;

impl PolkitCheck for SystemPolkit {
    fn helper_allowed(&self) -> PolkitAnswer {
        match check_authorization() {
            Ok(true) => PolkitAnswer::Allowed,
            Ok(false) => PolkitAnswer::NotAllowed,
            Err(e) => PolkitAnswer::Failed(match crate::dbus::error_name(&e) {
                Some(name) => name.into(),
                None => e.to_string().into(),
            }),
        }
    }
}

/// `org.freedesktop.PolicyKit1.Authority.CheckAuthorization` for `HELPER_ACTION`, with flags 0
/// (no interaction). The subject is this process; start time 0 makes polkit read it from
/// `/proc`. True only when `is_authorized` is true; a challenge counts as not allowed.
fn check_authorization() -> zbus::Result<bool> {
    let conn = zbus::blocking::connection::Builder::system()?
        .method_timeout(POLKIT_TIMEOUT)
        .build()?;
    let mut subject: HashMap<&str, Value<'_>> = HashMap::new();
    subject.insert("pid", Value::U32(std::process::id()));
    subject.insert("start-time", Value::U64(0));
    // polkit takes the uid as int32; a uid above i32::MAX is sent as its bit pattern.
    subject.insert(
        "uid",
        Value::I32(rustix::process::getuid().as_raw().cast_signed()),
    );
    let details: HashMap<&str, &str> = HashMap::new();
    let reply = conn.call_method(
        Some("org.freedesktop.PolicyKit1"),
        "/org/freedesktop/PolicyKit1/Authority",
        Some("org.freedesktop.PolicyKit1.Authority"),
        "CheckAuthorization",
        &(("unix-process", subject), HELPER_ACTION, details, 0u32, ""),
    )?;
    let (authorized, _challenge, _details): (bool, bool, HashMap<String, String>) =
        reply.body().deserialize()?;
    Ok(authorized)
}

/// Where the remote admin check stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteAdminState {
    /// The setting is off.
    Off,
    /// The setting is on and polkit has not answered yet. Remote admin is off meanwhile.
    Checking,
    /// The setting is on and polkit allows the helper. Remote admin is on.
    On,
    /// The setting is on, but polkit did not allow the helper. Remote admin is off.
    Refused(PolkitAnswer),
}

struct State {
    /// The setting as last applied.
    wanted: bool,
    /// Counts setting changes, so an answer to an older check is dropped.
    generation: u64,
    state: RemoteAdminState,
}

struct Shared {
    polkit: Arc<dyn PolkitCheck>,
    state: Mutex<State>,
    changed: Condvar,
}

/// Remote admin for one engine. Clones share the state: `EngineHandle` applies the setting, and
/// each session's reducer reads `is_on` when it admits a request.
#[derive(Clone)]
pub struct RemoteAdmin {
    shared: Arc<Shared>,
}

impl Default for RemoteAdmin {
    /// Off, with `SystemPolkit`.
    fn default() -> Self {
        Self::new(Arc::new(SystemPolkit))
    }
}

impl fmt::Debug for RemoteAdmin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RemoteAdmin").field(&self.state()).finish()
    }
}

impl RemoteAdmin {
    /// Off, asking `polkit` when the setting turns on.
    pub fn new(polkit: Arc<dyn PolkitCheck>) -> Self {
        Self {
            shared: Arc::new(Shared {
                polkit,
                state: Mutex::new(State {
                    wanted: false,
                    generation: 0,
                    state: RemoteAdminState::Off,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    /// True only while the setting is on and polkit allows the helper.
    pub fn is_on(&self) -> bool {
        self.lock().state == RemoteAdminState::On
    }

    pub fn state(&self) -> RemoteAdminState {
        self.lock().state.clone()
    }

    /// Takes the host setting `remote_admin`. Turning it off takes effect at once. Turning it on
    /// starts a polkit check on its own thread, so the caller never waits on D-Bus; remote admin
    /// stays off until polkit allows it. The same value again changes nothing.
    pub fn apply(&self, setting: bool) {
        let generation = {
            let mut s = self.lock();
            if s.wanted == setting {
                return;
            }
            s.wanted = setting;
            s.generation += 1;
            s.state = if setting {
                RemoteAdminState::Checking
            } else {
                RemoteAdminState::Off
            };
            self.shared.changed.notify_all();
            if !setting {
                return;
            }
            s.generation
        };
        // The check thread logs to the caller's subscriber, so its reason lands where the
        // caller's logs do.
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name("tl-polkit-check".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    let answer = this.shared.polkit.helper_allowed();
                    this.settle(generation, answer);
                });
            });
        if let Err(e) = spawned {
            self.settle(
                generation,
                PolkitAnswer::Failed(e.kind().to_string().into()),
            );
        }
    }

    /// Waits until no check is running, or `timeout` passes, and returns the state.
    pub fn wait_checked(&self, timeout: Duration) -> RemoteAdminState {
        let deadline = Instant::now() + timeout;
        let mut s = self.lock();
        while s.state == RemoteAdminState::Checking {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s = self
                .shared
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        s.state.clone()
    }

    /// Stores polkit's answer to the check of `generation`, unless the setting changed since.
    fn settle(&self, generation: u64, answer: PolkitAnswer) {
        let mut s = self.lock();
        if s.generation != generation {
            return;
        }
        match &answer {
            PolkitAnswer::Allowed => {
                info!(target: LOG, "remote admin is on: polkit allows the tasklens helper for this engine");
            }
            PolkitAnswer::NotAllowed => warn!(
                target: LOG,
                "remote admin is off: remote_admin is on in engine.toml, but polkit does not \
                 allow {HELPER_ACTION} for this engine without a password, so the remote admin \
                 rule is missing"
            ),
            PolkitAnswer::Failed(reason) => warn!(
                target: LOG,
                "remote admin is off: remote_admin is on in engine.toml, but polkit could not be \
                 asked: {reason}"
            ),
        }
        s.state = match answer {
            PolkitAnswer::Allowed => RemoteAdminState::On,
            refused => RemoteAdminState::Refused(refused),
        };
        self.shared.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    /// Gives `answer` and counts the calls.
    struct FakePolkit {
        answer: PolkitAnswer,
        calls: AtomicUsize,
    }

    impl FakePolkit {
        fn new(answer: PolkitAnswer) -> Arc<Self> {
            Arc::new(Self {
                answer,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl PolkitCheck for FakePolkit {
        fn helper_allowed(&self) -> PolkitAnswer {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    /// Keeps the warnings and info lines, as text.
    #[derive(Clone, Default)]
    struct Lines(Arc<Mutex<Vec<String>>>);

    struct Message(String);

    impl tracing::field::Visit for Message {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl tracing::Subscriber for Lines {
        fn register_callsite(
            &self,
            _: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::sometimes()
        }
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut message = Message(String::new());
            event.record(&mut message);
            let level = *event.metadata().level();
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{level} {}", message.0));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// Applies `setting` with `lines` as the subscriber and waits for the check.
    fn apply_logged(admin: &RemoteAdmin, lines: &Lines, setting: bool) -> RemoteAdminState {
        tracing::subscriber::with_default(lines.clone(), || {
            admin.apply(setting);
            admin.wait_checked(WAIT)
        })
    }

    fn logged(lines: &Lines) -> Vec<String> {
        lines
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// q4x76.13: polkit says no, so remote admin stays off although the setting is on, and the
    /// log says why. The setting itself is not touched here.
    #[test]
    fn polkit_denies_so_remote_admin_stays_off_and_says_why() {
        let polkit = FakePolkit::new(PolkitAnswer::NotAllowed);
        let admin = RemoteAdmin::new(polkit.clone());
        let lines = Lines::default();
        let state = apply_logged(&admin, &lines, true);
        assert_eq!(state, RemoteAdminState::Refused(PolkitAnswer::NotAllowed));
        assert!(!admin.is_on());
        assert_eq!(polkit.calls.load(Ordering::SeqCst), 1);
        let lines = logged(&lines);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("WARN remote admin is off:"),
            "{lines:?}"
        );
        assert!(lines[0].contains(HELPER_ACTION), "{lines:?}");
        assert!(lines[0].contains("rule is missing"), "{lines:?}");
    }

    /// q4x76.13: polkit cannot be asked, so remote admin stays off and the log names the error.
    #[test]
    fn polkit_fails_so_remote_admin_stays_off_and_says_why() {
        let polkit = FakePolkit::new(PolkitAnswer::Failed(
            "org.freedesktop.DBus.Error.ServiceUnknown".into(),
        ));
        let admin = RemoteAdmin::new(polkit);
        let lines = Lines::default();
        let state = apply_logged(&admin, &lines, true);
        assert!(matches!(
            state,
            RemoteAdminState::Refused(PolkitAnswer::Failed(_))
        ));
        assert!(!admin.is_on());
        let lines = logged(&lines);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("WARN remote admin is off:"),
            "{lines:?}"
        );
        assert!(
            lines[0].contains("org.freedesktop.DBus.Error.ServiceUnknown"),
            "{lines:?}"
        );
    }

    /// q4x76.13: polkit allows the helper, so remote admin is on. Turning the setting off turns
    /// it off at once, with no polkit call; turning it on again asks polkit again.
    #[test]
    fn polkit_allows_so_remote_admin_is_on_until_the_setting_turns_off() {
        let polkit = FakePolkit::new(PolkitAnswer::Allowed);
        let admin = RemoteAdmin::new(polkit.clone());
        assert_eq!(admin.state(), RemoteAdminState::Off);
        let lines = Lines::default();
        assert_eq!(apply_logged(&admin, &lines, true), RemoteAdminState::On);
        assert!(admin.is_on());
        assert!(logged(&lines)[0].starts_with("INFO remote admin is on"));
        // The same value again, as when another host key changes: no new check.
        assert_eq!(apply_logged(&admin, &lines, true), RemoteAdminState::On);
        assert_eq!(polkit.calls.load(Ordering::SeqCst), 1);

        admin.apply(false);
        assert_eq!(admin.state(), RemoteAdminState::Off);
        assert!(!admin.is_on());
        assert_eq!(polkit.calls.load(Ordering::SeqCst), 1);

        assert_eq!(apply_logged(&admin, &lines, true), RemoteAdminState::On);
        assert_eq!(polkit.calls.load(Ordering::SeqCst), 2);
    }

    /// An answer to a check the setting has since overtaken is dropped: off stays off.
    #[test]
    fn a_late_answer_does_not_turn_remote_admin_back_on() {
        let admin = RemoteAdmin::new(FakePolkit::new(PolkitAnswer::Allowed));
        let generation = {
            let mut s = admin.lock();
            s.wanted = true;
            s.generation = 1;
            s.state = RemoteAdminState::Checking;
            s.generation
        };
        admin.apply(false);
        admin.settle(generation, PolkitAnswer::Allowed);
        assert_eq!(admin.state(), RemoteAdminState::Off);
        assert!(!admin.is_on());
    }

    /// The real polkit on this machine answers the call, whatever it decides. Needs the system
    /// bus, so it runs only when asked: `cargo test -p tl-engine remote_admin -- --ignored`.
    #[test]
    #[ignore = "needs the system bus and polkitd"]
    fn system_polkit_answers() {
        let answer = SystemPolkit.helper_allowed();
        assert!(!matches!(answer, PolkitAnswer::Failed(_)), "{answer:?}");
    }
}
