//! `tasklens remote-admin on|off|status` (ADR-018 Remote root actions and Privilege coverage,
//! D-038, leader decision on tasklens-q4x76.13).
//!
//! Remote admin needs two things on the host: the polkit rule
//! `/etc/polkit-1/rules.d/49-tasklens-remote-admin.rules`, which only root can write, and
//! `remote_admin = true` in `engine.toml`, which only the engine service writes. This command runs
//! as the user. It runs `tasklens-setup` through pkexec only to write or remove the rule, and it
//! saves the setting through the engine socket, as a control-only client (no snapshots).
//!
//! - `on` reads the host settings, prints what remote admin reaches and asks for confirmation.
//!   Then it writes the rule and saves `remote_admin = true`. When the save fails it removes the
//!   rule again, so the rule and the setting never disagree.
//! - `off` saves `remote_admin = false` first, which closes admission at once, then removes the
//!   rule. When the save fails nothing has changed. When removing the rule fails, the setting
//!   stays off and the command says so.
//! - `status` prints the setting and whether polkit allows the tasklens helper for the engine
//!   process without a password, the same check the engine makes. A user cannot read
//!   `/etc/polkit-1/rules.d`, so this is how the rule is seen. It says whether the two agree.
//!
//! Every step with the engine has one deadline (ADR-018 Implementation contracts). When the
//! socket does not answer, the command starts the engine unit once, as `tasklens bridge` does.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_engine::remote_admin::{HELPER_ACTION, PolkitAnswer};
use tl_model::remote_proto::{
    BootPrefix, ClientMsg, Hello, HelloTail, HostSettings, HostSettingsChanges, ServerBootstrap,
    ServerMsg, SetHostSettingsReply, Want, decode_server_bootstrap, encode_hello,
};
use tl_remote::envelope::{FrameReader, FrameWriter};
use tl_remote::server::{self, DeadlineStream};
use zbus::zvariant::Value;

use crate::args::RemoteAdminArg as What;
use crate::engine_link::{self, CONNECT_DEADLINE};
use crate::paths;

/// The root setup script, run through pkexec.
pub const SETUP_PATH: &str = "/usr/lib/tasklens/tasklens-setup";
/// How long each step with the engine may take: connect and handshake, or one request.
pub const STEP_DEADLINE: Duration = Duration::from_secs(5);
/// How many times a save is tried when another client changed the settings in between.
const SAVE_TRIES: usize = 3;
/// How long the polkit call may take.
const POLKIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `tasklens remote-admin <what>` for this user. `app_id` is the checked `--app-id`.
pub fn main(app_id: Option<&str>, what: What) -> ExitCode {
    let app_dir = app_id.unwrap_or(paths::APP_DIR);
    let socket =
        match engine_link::socket_path(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), app_dir) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("tasklens remote-admin: {e}");
                return ExitCode::from(1);
            }
        };
    let uid = server::own_uid();
    let unit: &dyn Fn(Duration) -> bool = &engine_link::start_unit;
    let setup = Setup::pkexec();
    let polkit = |pid: u32| polkit_answer(pid, uid);
    let env = Env {
        socket,
        uid,
        hello: HelloTail {
            pid: std::process::id(),
            app_id: Arc::from(app_id.unwrap_or(tl_ui::window::main::APP_ID)),
            bridge: false,
            want: Want { snapshots: false },
        },
        user: user_name(uid),
        // The unit serves the default paths only (D-016).
        start_unit: app_id.is_none().then_some(unit),
        setup: &setup,
        polkit: &polkit,
        deadline: STEP_DEADLINE,
    };
    let stdin = io::stdin();
    let code = run(
        what,
        &env,
        &mut stdin.lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}

/// The user's name for the messages: `$USER`, else the uid.
fn user_name(uid: u32) -> String {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("uid {uid}"))
}

/// What the command needs from the process, so tests can stand in for pkexec, polkit and the
/// engine service.
pub struct Env<'a> {
    /// `$XDG_RUNTIME_DIR/<app>/engine.sock`.
    pub socket: PathBuf,
    /// The user the engine must run as.
    pub uid: u32,
    /// The `Hello` tail: a control-only client that is not a bridge.
    pub hello: HelloTail,
    /// The user's name, for the messages.
    pub user: String,
    /// Starts the engine unit and waits at most the given time. `None` for an `--app-id`
    /// instance, which has no unit.
    pub start_unit: Option<&'a dyn Fn(Duration) -> bool>,
    pub setup: &'a Setup,
    /// What polkit says about `HELPER_ACTION` for the process with this pid.
    pub polkit: &'a dyn Fn(u32) -> PolkitAnswer,
    /// `STEP_DEADLINE` in the app.
    pub deadline: Duration,
}

/// Runs the subcommand and returns the exit code: 0 when it did what was asked (for `status`:
/// the setting and the rule agree), 1 otherwise. Questions and results go to `out`, failures to
/// `err`.
pub fn run(
    what: What,
    env: &Env<'_>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let result = match what {
        What::On => turn_on(env, input, out, err),
        What::Off => turn_off(env, out, err),
        What::Status => status(env, out),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(err, "tasklens remote-admin: {e}");
            1
        }
    }
}

/// The reach sentence and the question (ADR-018 Remote root actions).
fn reach(host: &str, user: &str) -> String {
    format!(
        "Remote admin lets tasklens windows on other PCs run administrator actions on {host} \
         without a password prompt: acting on other users' processes, closing network \
         connections, the network monitor, tasklens setup, and starting, stopping, enabling and \
         disabling system services.\n\
         With it on, any program that runs as {user} on {host} can do these actions without a \
         prompt, whether it came through tasklens or not.\n\
         Turn on remote admin on {host}? [y/N] "
    )
}

fn turn_on(
    env: &Env<'_>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8, String> {
    let (host, before) = {
        let mut session = Session::open(env)?;
        let current = session.get()?;
        (Arc::clone(&session.host), current)
    };
    if before.read_only {
        return Err(read_only_text());
    }
    write!(out, "{}", reach(&host, &env.user)).map_err(write_error)?;
    out.flush().map_err(write_error)?;
    let mut answer = String::new();
    input.read_line(&mut answer).map_err(|e| e.to_string())?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        writeln!(out, "Nothing changed.").map_err(write_error)?;
        return Ok(1);
    }
    if let Err(e) = env.setup.run(true) {
        writeln!(
            err,
            "tasklens remote-admin: the polkit rule was not written: {e}. Nothing changed."
        )
        .map_err(write_error)?;
        return Ok(1);
    }
    match save(env, true) {
        Ok(()) => {
            writeln!(out, "Remote admin is on for {} on {host}.", env.user).map_err(write_error)?;
            Ok(0)
        }
        Err(e) => {
            writeln!(
                err,
                "tasklens remote-admin: remote_admin was not saved: {e}."
            )
            .map_err(write_error)?;
            if before.values.remote_admin {
                // The setting was on already, so the rule now matches it.
                return Ok(1);
            }
            match env.setup.run(false) {
                Ok(()) => writeln!(err, "The polkit rule was removed again. Nothing changed."),
                Err(e) => writeln!(
                    err,
                    "The polkit rule could not be removed again: {e}. Run tasklens \
                     remote-admin off to remove it."
                ),
            }
            .map_err(write_error)?;
            Ok(1)
        }
    }
}

fn turn_off(env: &Env<'_>, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8, String> {
    let host = match save(env, false) {
        Ok(host) => host,
        Err(e) => {
            writeln!(
                err,
                "tasklens remote-admin: remote_admin was not saved: {e}. Nothing changed. To \
                 remove only the polkit rule, run: pkexec {SETUP_PATH} remote-admin off"
            )
            .map_err(write_error)?;
            return Ok(1);
        }
    };
    if let Err(e) = env.setup.run(false) {
        writeln!(
            err,
            "tasklens remote-admin: remote_admin is off, but the polkit rule was not removed: \
             {e}. Remote windows get no administrator actions, but other programs that run as \
             {} still do. Run tasklens remote-admin off again to remove the rule.",
            env.user
        )
        .map_err(write_error)?;
        return Ok(1);
    }
    writeln!(out, "Remote admin is off on {host}.").map_err(write_error)?;
    Ok(0)
}

fn status(env: &Env<'_>, out: &mut dyn Write) -> Result<u8, String> {
    let mut session = Session::open(env)?;
    let current = session.get()?;
    let setting = current.values.remote_admin;
    let answer = (env.polkit)(session.engine_pid);
    let host = &session.host;
    let mut text = format!(
        "Remote admin on {host}:\n  Setting in engine.toml: {}\n  polkit: {}\n",
        if setting { "on" } else { "off" },
        match &answer {
            PolkitAnswer::Allowed =>
                "allows the tasklens helper without a password (the rule is in place)".to_owned(),
            PolkitAnswer::NotAllowed => "asks for a password (the rule is missing)".to_owned(),
            PolkitAnswer::Failed(why) => format!("could not be asked: {why}"),
        }
    );
    let code = match (setting, &answer) {
        (true, PolkitAnswer::Allowed) => {
            text.push_str("They agree: remote admin is on.\n");
            0
        }
        (false, PolkitAnswer::NotAllowed) => {
            text.push_str("They agree: remote admin is off.\n");
            0
        }
        (true, PolkitAnswer::NotAllowed) => {
            text.push_str(
                "They disagree: the setting is on, but the rule is missing, so the engine keeps \
                 remote admin off. Run tasklens remote-admin on to write the rule, or tasklens \
                 remote-admin off to turn the setting off.\n",
            );
            1
        }
        (false, PolkitAnswer::Allowed) => {
            text.push_str(&format!(
                "They disagree: the setting is off, but the rule is in place. Remote windows get \
                 no administrator actions, but other programs that run as {} still do. Run \
                 tasklens remote-admin off to remove the rule.\n",
                env.user
            ));
            1
        }
        (_, PolkitAnswer::Failed(_)) => {
            text.push_str("Cannot tell whether they agree.\n");
            1
        }
    };
    out.write_all(text.as_bytes()).map_err(write_error)?;
    Ok(code)
}

/// Saves `remote_admin = on` on a new session: the pkexec prompt before it may have taken
/// minutes, and the engine may have restarted meanwhile. Another client's change in between is
/// a conflict; the save is tried again on the new revision, up to `SAVE_TRIES` times. Returns
/// the host name.
fn save(env: &Env<'_>, on: bool) -> Result<Arc<str>, String> {
    let mut session = Session::open(env)?;
    for _ in 0..SAVE_TRIES {
        let current = session.get()?;
        if current.read_only {
            return Err(read_only_text());
        }
        let changes = HostSettingsChanges {
            remote_admin: Some(on),
            ..HostSettingsChanges::default()
        };
        match session.set(current.rev, changes)? {
            SetHostSettingsReply::Ack { .. } => return Ok(Arc::clone(&session.host)),
            SetHostSettingsReply::Conflict { .. } => {}
        }
    }
    Err(format!(
        "another window changed the host settings {SAVE_TRIES} times in a row"
    ))
}

fn read_only_text() -> String {
    "engine.toml was written by a newer tasklens, so this engine never saves it".to_owned()
}

fn write_error(e: io::Error) -> String {
    format!("writing to the terminal: {e}")
}

/// One control-only session with the engine service: no snapshots, only host settings.
struct Session {
    stream: UnixStream,
    frames: FrameReader,
    writer: FrameWriter,
    deadline: Duration,
    /// From the engine's `Welcome`.
    host: Arc<str>,
    /// The engine process, from `SO_PEERCRED`.
    engine_pid: u32,
}

impl Session {
    /// Connects to the engine. When that fails, starts the unit once and connects again.
    fn open(env: &Env<'_>) -> Result<Self, String> {
        match Self::connect(env) {
            Ok(session) => Ok(session),
            Err(first) => {
                let Some(start_unit) = env.start_unit else {
                    return Err(first);
                };
                start_unit(CONNECT_DEADLINE);
                Self::connect(env).map_err(|e| {
                    format!(
                        "{e}. If a tasklens window runs its own engine, choose \"Switch to engine \
                         service\" in it first"
                    )
                })
            }
        }
    }

    /// Connects, checks the peer and sends `Hello`, all by one deadline.
    fn connect(env: &Env<'_>) -> Result<Self, String> {
        let deadline = Instant::now() + env.deadline;
        let fail = |e: &dyn std::fmt::Display| {
            format!(
                "cannot reach the engine service at {}: {e}",
                env.socket.display()
            )
        };
        let stream = engine_link::connect_by(&env.socket, deadline).map_err(|e| fail(&e))?;
        server::check_peer(&stream, env.uid).map_err(|e| fail(&e))?;
        let cred = rustix::net::sockopt::socket_peercred(&stream).map_err(|e| fail(&e))?;
        let engine_pid = rustix::process::Pid::as_raw(Some(cred.pid)).cast_unsigned();
        let frames = FrameReader::new().map_err(|e| fail(&e))?;
        let writer = FrameWriter::new().map_err(|e| fail(&e))?;
        let mut hello = Vec::new();
        encode_hello(
            &Hello {
                prefix: BootPrefix::ours(),
                tail: Some(env.hello.clone()),
            },
            &mut hello,
        )
        .map_err(|e| fail(&e))?;
        DeadlineStream::new(&stream, deadline)
            .write_all(&hello)
            .map_err(|e| fail(&e))?;
        let frame = frames
            .read_frame(&mut DeadlineStream::new(&stream, deadline))
            .map_err(|e| fail(&e))?;
        let host = match decode_server_bootstrap(&frame).map_err(|e| fail(&e))? {
            ServerBootstrap::Welcome(welcome) => match welcome.tail {
                Some(identity) => identity.host_name,
                None => return Err(fail(&"the engine runs another tasklens version")),
            },
            ServerBootstrap::Refused(refused) => return Err(fail(&refused.text)),
        };
        Ok(Self {
            stream,
            frames,
            writer,
            deadline: env.deadline,
            host,
            engine_pid,
        })
    }

    fn get(&mut self) -> Result<HostSettings, String> {
        match self.request(&ClientMsg::GetHostSettings)? {
            ServerMsg::HostSettings(current) => Ok(current),
            _ => Err("the engine sent the wrong reply".to_owned()),
        }
    }

    fn set(
        &mut self,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<SetHostSettingsReply, String> {
        match self.request(&ClientMsg::SetHostSettings { base_rev, changes })? {
            ServerMsg::SetHostSettings(reply) => Ok(reply),
            _ => Err("the engine sent the wrong reply".to_owned()),
        }
    }

    /// Sends `msg` and reads until its reply, all by one deadline. A control-only session gets
    /// no snapshots; any other message before the reply is skipped.
    fn request(&mut self, msg: &ClientMsg) -> Result<ServerMsg, String> {
        let deadline = Instant::now() + self.deadline;
        let fail = |e: &dyn std::fmt::Display| format!("the engine service did not answer: {e}");
        let mut bytes = Vec::new();
        self.writer
            .encode_client(msg, &mut bytes)
            .map_err(|e| fail(&e))?;
        DeadlineStream::new(&self.stream, deadline)
            .write_all(&bytes)
            .map_err(|e| fail(&e))?;
        loop {
            let frame = self
                .frames
                .read_frame(&mut DeadlineStream::new(&self.stream, deadline))
                .map_err(|e| fail(&e))?;
            match self.frames.decode_server(&frame).map_err(|e| fail(&e))? {
                reply @ (ServerMsg::HostSettings(_) | ServerMsg::SetHostSettings(_)) => {
                    return Ok(reply);
                }
                _ => {}
            }
        }
    }
}

/// Runs the root setup script for `remote-admin on|off`: `pkexec /usr/lib/tasklens/tasklens-setup`
/// in the app, a fake in tests.
pub struct Setup {
    program: OsString,
    /// Before `remote-admin on|off`.
    args: Vec<OsString>,
    /// Added to the inherited environment.
    envs: Vec<(OsString, OsString)>,
    /// The script's own "Enabled" or "Reverted" goes nowhere. pkexec asks for the password on
    /// the terminal, so the app keeps the terminal's output.
    quiet: bool,
}

impl Setup {
    pub fn pkexec() -> Self {
        Self {
            program: "pkexec".into(),
            args: vec![SETUP_PATH.into()],
            envs: Vec::new(),
            quiet: false,
        }
    }

    fn command(&self, on: bool) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .args(["remote-admin", if on { "on" } else { "off" }])
            .envs(self.envs.iter().map(|(k, v)| (k, v)));
        if self.quiet {
            command.stdout(Stdio::null());
        }
        command
    }

    /// Runs the script and waits for it. pkexec's own exit codes are named.
    fn run(&self, on: bool) -> Result<(), String> {
        let status = self
            .command(on)
            .status()
            .map_err(|e| format!("{}: {e}", self.program.to_string_lossy()))?;
        match status.code() {
            Some(0) => Ok(()),
            Some(126) => Err("the password prompt was dismissed (pkexec exit 126)".to_owned()),
            Some(127) => Err("polkit did not allow it (pkexec exit 127)".to_owned()),
            Some(code) => Err(format!(
                "{SETUP_PATH} remote-admin {} exited with {code}",
                if on { "on" } else { "off" }
            )),
            None => Err("tasklens-setup was ended by a signal".to_owned()),
        }
    }
}

/// What polkit says about `HELPER_ACTION` for the engine process `pid` of user `uid`, without
/// interaction: the check the engine makes for itself (`tl_engine::remote_admin`). The engine is
/// asked about, not this command, because this command may run in the active local session,
/// where the APP-049 rule allows tasklens actions without the remote admin rule.
fn polkit_answer(pid: u32, uid: u32) -> PolkitAnswer {
    match check_authorization(pid, uid) {
        Ok(true) => PolkitAnswer::Allowed,
        Ok(false) => PolkitAnswer::NotAllowed,
        Err(zbus::Error::MethodError(name, _, _)) => PolkitAnswer::Failed(name.as_str().into()),
        Err(e) => PolkitAnswer::Failed(e.to_string().into()),
    }
}

/// `CheckAuthorization` with flags 0 (no interaction). Start time 0 makes polkit read it from
/// `/proc`. A challenge counts as not allowed.
fn check_authorization(pid: u32, uid: u32) -> zbus::Result<bool> {
    let conn = zbus::blocking::connection::Builder::system()?
        .method_timeout(POLKIT_TIMEOUT)
        .build()?;
    let mut subject: HashMap<&str, Value<'_>> = HashMap::new();
    subject.insert("pid", Value::U32(pid));
    subject.insert("start-time", Value::U64(0));
    // polkit takes the uid as int32; a uid above i32::MAX is sent as its bit pattern.
    subject.insert("uid", Value::I32(uid.cast_signed()));
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;

    use tl_model::actions::ActionEvent;
    use tl_model::command::{Command as EngineCommand, CommandError};
    use tl_model::demand::Demand;
    use tl_model::journal::{JournalHandle, JournalQuery};
    use tl_model::remote_proto::{HostIdentity, HostSettingsValues};
    use tl_model::request::RequestEvent;
    use tl_model::snapshot::Snapshot;
    use tl_model::time::BootId;
    use tl_remote::server::{Port, Receiver, SessionConfig};

    use super::*;

    const RULE: &str = "etc/polkit-1/rules.d/49-tasklens-remote-admin.rules";
    const HOST: &str = "home";

    /// What the test engine does with a save.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Saves {
        Work,
        /// Every save is a conflict, as when another window keeps changing the settings.
        Conflict,
    }

    struct EngineState {
        host: Mutex<HostSettings>,
        saves: Saves,
        wake_tx: async_channel::Sender<()>,
        wake_rx: async_channel::Receiver<()>,
        events_rx: async_channel::Receiver<ActionEvent>,
        requests_rx: async_channel::Receiver<RequestEvent>,
        _events_tx: async_channel::Sender<ActionEvent>,
        _requests_tx: async_channel::Sender<RequestEvent>,
    }

    #[derive(Clone)]
    struct TestPort(Arc<EngineState>);

    impl Port for TestPort {
        fn latest(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::empty(BootId(1), 0))
        }
        fn wake(&self) -> Receiver<()> {
            self.0.wake_rx.clone()
        }
        fn action_events(&self) -> Receiver<ActionEvent> {
            self.0.events_rx.clone()
        }
        fn request_events(&self) -> Receiver<RequestEvent> {
            self.0.requests_rx.clone()
        }
        fn set_demand(&self, _demand: Demand) {}
        fn send(&self, _cmd: EngineCommand) -> Result<(), CommandError> {
            Ok(())
        }
        fn journal(&self, _query: JournalQuery) -> JournalHandle {
            let (_events_tx, events_rx) = async_channel::unbounded();
            let (control_tx, _control_rx) = async_channel::unbounded();
            JournalHandle::new(events_rx, control_tx)
        }
        fn is_ended(&self) -> bool {
            false
        }
        fn host_settings(&self) -> HostSettings {
            self.0.host.lock().expect("host").clone()
        }
        fn set_host_settings(
            &self,
            base_rev: u64,
            changes: HostSettingsChanges,
        ) -> SetHostSettingsReply {
            let mut host = self.0.host.lock().expect("host");
            if base_rev != host.rev || self.0.saves == Saves::Conflict {
                return SetHostSettingsReply::Conflict {
                    current: host.clone(),
                };
            }
            let next = host.values.with_changes(&changes);
            if next != host.values {
                host.values = next;
                host.rev += 1;
            }
            SetHostSettingsReply::Ack { rev: host.rev }
        }
    }

    /// An engine on a Unix socket, served with tl-remote's real handshake and session code.
    struct TestEngine {
        state: Arc<EngineState>,
        sessions: Arc<AtomicU32>,
    }

    impl TestEngine {
        /// Serves at most `sessions` connections on `socket`, then closes the listener, as an
        /// engine that stopped.
        fn start(socket: &Path, remote_admin: bool, saves: Saves, sessions: u32) -> Self {
            let (wake_tx, wake_rx) = async_channel::bounded(1);
            let (events_tx, events_rx) = async_channel::unbounded();
            let (requests_tx, requests_rx) = async_channel::unbounded();
            let state = Arc::new(EngineState {
                host: Mutex::new(HostSettings {
                    rev: 1,
                    values: HostSettingsValues {
                        remote_admin,
                        ..HostSettingsValues::default()
                    },
                    read_only: false,
                }),
                saves,
                wake_tx,
                wake_rx,
                events_rx,
                requests_rx,
                _events_tx: events_tx,
                _requests_tx: requests_tx,
            });
            let listener = UnixListener::bind(socket).expect("bind");
            let port = TestPort(Arc::clone(&state));
            let served = Arc::new(AtomicU32::new(0));
            let count = Arc::clone(&served);
            thread::spawn(move || {
                for _ in 0..sessions {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    count.fetch_add(1, Ordering::SeqCst);
                    let port = port.clone();
                    thread::spawn(move || serve(stream, port));
                }
            });
            Self {
                state,
                sessions: served,
            }
        }

        fn remote_admin(&self) -> bool {
            self.state.host.lock().expect("host").values.remote_admin
        }

        fn rev(&self) -> u64 {
            self.state.host.lock().expect("host").rev
        }
    }

    fn serve(stream: UnixStream, port: TestPort) {
        let frames = FrameReader::new().expect("frames");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reader = DeadlineStream::new(&stream, deadline);
        let mut writer = DeadlineStream::new(&stream, deadline);
        let identity = HostIdentity {
            host_name: Arc::from(HOST),
            machine_id: Arc::from("m1"),
            boot_id: BootId(1),
            engine_instance: 1,
        };
        let Ok((hello, port)) =
            server::handshake(&mut reader, &mut writer, &frames, |_| Ok((identity, port)))
        else {
            return;
        };
        assert!(!hello.bridge);
        assert!(
            !hello.want.snapshots,
            "remote-admin is a control-only client"
        );
        let _ = port.0.wake_tx.try_send(());
        let config = SessionConfig {
            snapshots: hello.want.snapshots,
            ..SessionConfig::default()
        };
        server::serve_session(stream, port, config);
    }

    struct Rig {
        dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            fs::create_dir_all(dir.path().join("root/etc/polkit-1/rules.d")).expect("mkdir");
            Self { dir }
        }

        fn socket(&self) -> PathBuf {
            self.dir.path().join("engine.sock")
        }

        fn root(&self) -> PathBuf {
            self.dir.path().join("root")
        }

        fn rule(&self) -> PathBuf {
            self.root().join(RULE)
        }

        /// A fake pkexec: runs the real script without privilege in its test mode, under this
        /// rig's root, with `PKEXEC_UID` set to this user, as pkexec would.
        fn setup(&self) -> Setup {
            Setup {
                program: "sh".into(),
                args: vec![
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../packaging/tasklens-setup")
                        .into(),
                ],
                envs: vec![
                    ("TASKLENS_SETUP_ROOT".into(), self.root().into()),
                    ("PKEXEC_UID".into(), server::own_uid().to_string().into()),
                ],
                quiet: true,
            }
        }

        /// Runs the command; returns its exit code, output and errors.
        fn run(
            &self,
            what: What,
            setup: &Setup,
            polkit: &dyn Fn(u32) -> PolkitAnswer,
            answer: &str,
        ) -> (u8, String, String) {
            let env = Env {
                socket: self.socket(),
                uid: server::own_uid(),
                hello: HelloTail {
                    pid: std::process::id(),
                    app_id: Arc::from("test"),
                    bridge: false,
                    want: Want { snapshots: false },
                },
                user: "alice".to_owned(),
                start_unit: None,
                setup,
                polkit,
                deadline: STEP_DEADLINE,
            };
            let mut out = Vec::new();
            let mut err = Vec::new();
            let code = run(what, &env, &mut answer.as_bytes(), &mut out, &mut err);
            (
                code,
                String::from_utf8(out).expect("utf-8"),
                String::from_utf8(err).expect("utf-8"),
            )
        }
    }

    /// A pkexec that fails the test if it is run.
    fn no_pkexec() -> Setup {
        Setup {
            program: "sh".into(),
            args: vec!["-c".into(), "exit 99".into(), "pkexec".into()],
            envs: Vec::new(),
            quiet: true,
        }
    }

    /// A pkexec whose password prompt was dismissed.
    fn dismissed_pkexec() -> Setup {
        Setup {
            program: "sh".into(),
            args: vec!["-c".into(), "exit 126".into(), "pkexec".into()],
            envs: Vec::new(),
            quiet: true,
        }
    }

    fn unused_polkit(_pid: u32) -> PolkitAnswer {
        panic!("only status asks polkit")
    }

    fn own_user() -> String {
        let out = Command::new("id").arg("-nu").output().expect("id -nu");
        String::from_utf8(out.stdout)
            .expect("utf-8")
            .trim()
            .to_owned()
    }

    /// The rule `tasklens-setup remote-admin on` writes for `user` (ADR-018 Privilege coverage).
    fn rule_text(user: &str) -> String {
        format!(
            "\
polkit.addRule(function(action, subject) {{
    if (subject.user != \"{user}\") {{
        return polkit.Result.NOT_HANDLED;
    }}
    if (action.id == \"io.github.oysteinkrog.tasklens.helper\" ||
        action.id == \"io.github.oysteinkrog.tasklens.setup\" ||
        action.id == \"io.github.oysteinkrog.tasklens.netmon\" ||
        action.id == \"org.freedesktop.systemd1.manage-unit-files\" ||
        (action.id == \"org.freedesktop.systemd1.manage-units\" && action.lookup(\"unit\"))) {{
        return polkit.Result.YES;
    }}
    return polkit.Result.NOT_HANDLED;
}});
"
        )
    }

    /// On and off round trip: `on` asks first, writes the rule for this user through the fake
    /// pkexec and saves the setting through the socket; `off` saves it off and removes the rule.
    #[test]
    fn on_then_off_round_trip() {
        use std::os::unix::fs::PermissionsExt;
        let rig = Rig::new();
        let engine = TestEngine::start(&rig.socket(), false, Saves::Work, 2);
        let setup = rig.setup();

        let (code, out, err) = rig.run(What::On, &setup, &unused_polkit, "y\n");
        assert_eq!(code, 0, "{out}{err}");
        assert_eq!(err, "");
        assert_eq!(
            out,
            format!(
                "{}Remote admin is on for alice on home.\n",
                reach(HOST, "alice")
            )
        );
        assert!(out.contains(
            "any program that runs as alice on home can do these actions without a prompt, \
             whether it came through tasklens or not."
        ));
        assert_eq!(
            fs::read_to_string(rig.rule()).expect("rule"),
            rule_text(&own_user())
        );
        let mode = fs::metadata(rig.rule()).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644);
        assert!(engine.remote_admin());
        assert_eq!(engine.rev(), 2);

        // A second engine for `off`: the first served its two sessions.
        fs::remove_file(rig.socket()).expect("rm socket");
        let engine = TestEngine::start(&rig.socket(), true, Saves::Work, 1);
        let (code, out, err) = rig.run(What::Off, &setup, &unused_polkit, "");
        assert_eq!(code, 0, "{out}{err}");
        assert_eq!(out, "Remote admin is off on home.\n");
        assert_eq!(err, "");
        assert!(!rig.rule().exists());
        assert!(!engine.remote_admin());
    }

    /// Anything but y or yes changes nothing; pkexec never runs.
    #[test]
    fn on_needs_confirmation() {
        for answer in ["\n", "n\n", "", "yes please\n"] {
            let rig = Rig::new();
            let engine = TestEngine::start(&rig.socket(), false, Saves::Work, 2);
            let (code, out, err) = rig.run(What::On, &no_pkexec(), &unused_polkit, answer);
            assert_eq!(code, 1, "{answer:?}");
            assert_eq!(err, "", "{answer:?}");
            assert!(out.ends_with("[y/N] Nothing changed.\n"), "{out}");
            assert!(!rig.rule().exists());
            assert!(!engine.remote_admin());
        }
    }

    /// A dismissed password prompt saves nothing.
    #[test]
    fn on_with_dismissed_prompt_saves_nothing() {
        let rig = Rig::new();
        let engine = TestEngine::start(&rig.socket(), false, Saves::Work, 2);
        let (code, _, err) = rig.run(What::On, &dismissed_pkexec(), &unused_polkit, "y\n");
        assert_eq!(code, 1);
        assert_eq!(
            err,
            "tasklens remote-admin: the polkit rule was not written: the password prompt was \
             dismissed (pkexec exit 126). Nothing changed.\n"
        );
        assert!(!engine.remote_admin());
        assert_eq!(engine.rev(), 1);
    }

    /// q4x76.31: the engine stops after the first session, so the save after pkexec fails. The
    /// rule that was written is removed again: the rules file is as it was.
    #[test]
    fn on_save_fails_so_the_rule_is_removed_again() {
        let rig = Rig::new();
        let engine = TestEngine::start(&rig.socket(), false, Saves::Work, 1);
        let (code, _, err) = rig.run(What::On, &rig.setup(), &unused_polkit, "y\n");
        assert_eq!(code, 1);
        assert!(
            err.starts_with(
                "tasklens remote-admin: remote_admin was not saved: cannot reach the engine \
                 service at "
            ),
            "{err}"
        );
        assert!(
            err.ends_with("The polkit rule was removed again. Nothing changed.\n"),
            "{err}"
        );
        assert!(!rig.rule().exists());
        assert!(!engine.remote_admin());
        assert_eq!(engine.sessions.load(Ordering::SeqCst), 1);
    }

    /// q4x76.31: every save is a conflict, so the save fails after the rule was written; the rule
    /// is removed again.
    #[test]
    fn on_save_conflicts_so_the_rule_is_removed_again() {
        let rig = Rig::new();
        let engine = TestEngine::start(&rig.socket(), false, Saves::Conflict, 2);
        let (code, _, err) = rig.run(What::On, &rig.setup(), &unused_polkit, "y\n");
        assert_eq!(code, 1);
        assert_eq!(
            err,
            "tasklens remote-admin: remote_admin was not saved: another window changed the host \
             settings 3 times in a row.\nThe polkit rule was removed again. Nothing changed.\n"
        );
        assert!(!rig.rule().exists());
        assert!(!engine.remote_admin());
    }

    /// q4x76.31: when the save of `off` fails, the rule stays as it was, and so does the setting.
    #[test]
    fn off_save_fails_so_the_rule_stays() {
        let rig = Rig::new();
        fs::write(rig.rule(), rule_text("alice")).expect("write rule");
        let engine = TestEngine::start(&rig.socket(), true, Saves::Conflict, 1);
        let (code, out, err) = rig.run(What::Off, &rig.setup(), &unused_polkit, "");
        assert_eq!(code, 1);
        assert_eq!(out, "");
        assert!(
            err.contains("remote_admin was not saved: another window changed the host settings"),
            "{err}"
        );
        assert!(err.contains("Nothing changed."), "{err}");
        assert_eq!(
            fs::read_to_string(rig.rule()).expect("rule"),
            rule_text("alice")
        );
        assert!(engine.remote_admin());
    }

    /// The setting is saved off before the rule is removed, so a failed removal leaves remote
    /// admin off and says that the rule is still there.
    #[test]
    fn off_with_dismissed_prompt_keeps_the_setting_off() {
        let rig = Rig::new();
        let engine = TestEngine::start(&rig.socket(), true, Saves::Work, 1);
        let (code, out, err) = rig.run(What::Off, &dismissed_pkexec(), &unused_polkit, "");
        assert_eq!(code, 1);
        assert_eq!(out, "");
        assert!(
            err.starts_with(
                "tasklens remote-admin: remote_admin is off, but the polkit rule was not removed"
            ),
            "{err}"
        );
        assert!(!engine.remote_admin());
    }

    /// Status prints both states, says whether they agree and asks polkit about the engine's
    /// process (here the test process, which runs the test engine).
    #[test]
    fn status_reports_agreement_and_mismatch() {
        let cases = [
            (
                true,
                PolkitAnswer::Allowed,
                0,
                "They agree: remote admin is on.\n",
            ),
            (
                false,
                PolkitAnswer::NotAllowed,
                0,
                "They agree: remote admin is off.\n",
            ),
            (
                true,
                PolkitAnswer::NotAllowed,
                1,
                "They disagree: the setting is on, but the rule is missing, so the engine keeps \
                 remote admin off. Run tasklens remote-admin on to write the rule, or tasklens \
                 remote-admin off to turn the setting off.\n",
            ),
            (
                false,
                PolkitAnswer::Allowed,
                1,
                "They disagree: the setting is off, but the rule is in place. Remote windows get \
                 no administrator actions, but other programs that run as alice still do. Run \
                 tasklens remote-admin off to remove the rule.\n",
            ),
            (
                true,
                PolkitAnswer::Failed("org.freedesktop.DBus.Error.ServiceUnknown".into()),
                1,
                "Cannot tell whether they agree.\n",
            ),
        ];
        for (setting, answer, want_code, verdict) in cases {
            let rig = Rig::new();
            let _engine = TestEngine::start(&rig.socket(), setting, Saves::Work, 1);
            let asked = Mutex::new(Vec::new());
            let polkit = |pid: u32| {
                asked.lock().expect("asked").push(pid);
                answer.clone()
            };
            let (code, out, err) = rig.run(What::Status, &no_pkexec(), &polkit, "");
            assert_eq!(code, want_code, "{out}");
            assert_eq!(err, "");
            let polkit_line = match &answer {
                PolkitAnswer::Allowed => {
                    "allows the tasklens helper without a password (the rule is in place)"
                }
                PolkitAnswer::NotAllowed => "asks for a password (the rule is missing)",
                PolkitAnswer::Failed(_) => {
                    "could not be asked: org.freedesktop.DBus.Error.ServiceUnknown"
                }
            };
            assert_eq!(
                out,
                format!(
                    "Remote admin on home:\n  Setting in engine.toml: {}\n  polkit: \
                     {polkit_line}\n{verdict}",
                    if setting { "on" } else { "off" }
                )
            );
            assert_eq!(*asked.lock().expect("asked"), [std::process::id()]);
        }
    }

    /// No engine on the socket and no unit to start: every subcommand fails before pkexec.
    #[test]
    fn no_engine_fails_before_pkexec() {
        for what in [What::On, What::Off, What::Status] {
            let rig = Rig::new();
            let (code, out, err) = rig.run(what, &no_pkexec(), &unused_polkit, "y\n");
            assert_eq!(code, 1, "{what:?}");
            assert_eq!(out, "", "{what:?}");
            assert!(err.contains("cannot reach the engine service at "), "{err}");
        }
    }

    /// The app runs `pkexec /usr/lib/tasklens/tasklens-setup remote-admin on|off`.
    #[test]
    fn pkexec_command_line() {
        for (on, word) in [(true, "on"), (false, "off")] {
            let command = Setup::pkexec().command(on);
            assert_eq!(command.get_program(), "pkexec");
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args, [SETUP_PATH, "remote-admin", word]);
        }
    }

    /// The script refuses a missing or odd `PKEXEC_UID`, so no rule is written for nobody.
    #[test]
    fn script_refuses_a_bad_pkexec_uid() {
        for uid in [None, Some(""), Some("1000 0"), Some("-1"), Some("abc")] {
            let rig = Rig::new();
            let mut setup = rig.setup();
            setup.envs.retain(|(k, _)| k != "PKEXEC_UID");
            let mut command = setup.command(true);
            command.env_remove("PKEXEC_UID").stderr(Stdio::null());
            if let Some(uid) = uid {
                command.env("PKEXEC_UID", uid);
            }
            let status = command.status().expect("run script");
            assert_eq!(status.code(), Some(1), "{uid:?}");
            assert!(!rig.rule().exists(), "{uid:?}");
        }
    }
}
