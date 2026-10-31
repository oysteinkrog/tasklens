//! The shared launcher: Run dialog commands (PROC-095), Startup "Run now" (SYS-023) and unit
//! starts (R-134, ADR-005, ADR-006).
//!
//! Programs start detached through the tl-ops launch helper (`posix_spawn`, `setsid`, stdio on
//! `/dev/null`), then go into a new transient user scope so they group as apps. A scope that
//! cannot be made is logged and the program still runs. "Run as administrator" goes only through
//! `pkexec env … /bin/sh -c <text>` and gets no scope; the launcher watches the child's comm to
//! see when the polkit prompt closed (R-253).
//!
//! Every D-Bus call has a 2 s `time::timeout` followed by a checkpoint, except the 30 s wait for
//! `JobRemoved`. Never log a command text, an Exec line or a path above `debug`.

#![cfg_attr(
    not(test),
    allow(dead_code, reason = "E11-016 RealOpeners is the caller")
)]

use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::time;
use futures_lite::StreamExt;
use smallvec::SmallVec;
use tl_collect::startup::{
    DesktopEntry, MAX_DESKTOP_FILE, StartupEnv, autostart_unit_name, parse_desktop_entry,
    systemd_escape,
};
use tl_model::actions::{ActionError, ActionState, LaunchSpec, LaunchWhat};
use tl_model::table::Manager;
use tl_ops::OpsError;
use tl_ops::launch::{
    ADMIN_ENV_KEYS, EnvChange, ExecContext, ExitKind, PKEXEC_DISMISSED, PKEXEC_NOT_AUTHORIZED,
    Spawned,
};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::desktop::{DesktopSession, SESSION_KEYS};
use crate::opener;

/// The deadline for each D-Bus call (ADR-006).
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest wait for the `JobRemoved` of a started unit (R-134).
const JOB_WAIT: Duration = Duration::from_secs(30);
/// How often the `pkexec` child is checked (R-253).
const ADMIN_POLL: Duration = Duration::from_millis(250);
/// How long the program must keep running after the prompt closed (R-148 item 5).
const ADMIN_GRACE: Duration = Duration::from_secs(2);

const SYSTEMD_DEST: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";

/// What a launch did. E11 reports `AlreadyRunning` as `Rejected(RejectReason::AlreadyRunning)`
/// (SYS-023, R-148 item 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaunchOutcome {
    Launched,
    AlreadyRunning,
}

/// Runs `spec`. `session` is the user manager connection on the session bus
/// (`SharedHandles.systemd_user`). Programs start with `desktop`'s variables (ADR-018).
pub(crate) async fn launch(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    desktop: &DesktopSession,
    spec: &LaunchSpec,
) -> Result<LaunchOutcome, ActionError> {
    let host = RealHost {
        desktop: desktop.clone(),
    };
    launch_with(cx, session, spec, &host).await
}

/// One check of a watched child.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Probe {
    /// The comm field of `/proc/<pid>/stat`, `None` when it could not be read.
    comm: Option<String>,
    /// The exit, when the child has ended. The child is reaped.
    exit: Option<ExitKind>,
}

/// The process, environment and opener side of a launch. `RealHost` in production; tests record
/// the calls instead.
trait Host: Sync {
    type Child: Send + Sync + 'static;

    fn var(&self, key: &str) -> Option<String>;
    /// The environment changes that start a program on the desktop session's screen. Not used
    /// for the administrator path, which passes the session's variables through `pkexec env`
    /// itself.
    fn session_env(&self) -> Vec<EnvChange> {
        Vec::new()
    }
    fn startup_env(&self) -> StartupEnv;
    /// Starts `argv` itself detached in `cwd`, with `env` applied to its environment. The
    /// caller passes the child to `detach` or reaps it through `poll`.
    fn spawn(
        &self,
        cx: &Cx,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<EnvChange>,
    ) -> impl Future<Output = Result<Self::Child, ActionError>> + Send;
    fn pid(&self, child: &Self::Child) -> u32;
    /// Reaps the child in a background task.
    fn detach(&self, cx: &Cx, child: Self::Child);
    fn poll(
        &self,
        cx: &Cx,
        child: &Self::Child,
    ) -> impl Future<Output = Result<Probe, ActionError>> + Send;
    fn open_uri(
        &self,
        cx: &Cx,
        session: Option<Arc<zbus::Connection>>,
        target: &str,
    ) -> impl Future<Output = Result<(), ActionError>> + Send;
    fn activate_action(
        &self,
        cx: &Cx,
        session: Option<Arc<zbus::Connection>>,
        bus_name: &str,
        object_path: &str,
        action: &str,
        parameter: Option<&str>,
    ) -> impl Future<Output = Result<(), ActionError>> + Send;
}

struct RealHost {
    desktop: DesktopSession,
}

impl Host for RealHost {
    type Child = Arc<Spawned>;

    /// The desktop session's value for a session variable, this process's for any other.
    fn var(&self, key: &str) -> Option<String> {
        if SESSION_KEYS.contains(&key) {
            self.desktop.var(key)
        } else {
            std::env::var(key).ok()
        }
    }

    fn session_env(&self) -> Vec<EnvChange> {
        self.desktop
            .overrides()
            .map(|env| env.child_env())
            .unwrap_or_default()
    }

    fn startup_env(&self) -> StartupEnv {
        StartupEnv::from_env()
    }

    async fn spawn(
        &self,
        cx: &Cx,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<EnvChange>,
    ) -> Result<Arc<Spawned>, ActionError> {
        let program = argv.first().cloned().unwrap_or_default();
        let mut job = cx
            .spawn_blocking(move |_| {
                tl_ops::launch::spawn_detached_env(&argv, cwd.as_deref(), &env)
            })
            .map_err(|_| internal("spawn job not admitted"))?;
        match job.join(cx).await {
            Ok(Ok(child)) => Ok(Arc::new(child)),
            Ok(Err(e)) => Err(spawn_error(&program, &e)),
            Err(_) => Err(internal("spawn job failed")),
        }
    }

    fn pid(&self, child: &Arc<Spawned>) -> u32 {
        child.pid
    }

    fn detach(&self, cx: &Cx, child: Arc<Spawned>) {
        let reaper = cx.spawn(move |cx| async move {
            if let Ok(mut wait) = cx.spawn_blocking(move |_| tl_ops::launch::wait_exit(&child)) {
                let _ = wait.join(&cx).await;
            }
        });
        if reaper.is_err() {
            tracing::warn!("launcher: reaper task not started");
        }
    }

    async fn poll(&self, cx: &Cx, child: &Arc<Spawned>) -> Result<Probe, ActionError> {
        let child = Arc::clone(child);
        let mut job = cx
            .spawn_blocking(move |_| probe_child(&child))
            .map_err(|_| internal("probe job not admitted"))?;
        match job.join(cx).await {
            Ok(probe) => probe,
            Err(_) => Err(internal("probe job failed")),
        }
    }

    async fn open_uri(
        &self,
        cx: &Cx,
        session: Option<Arc<zbus::Connection>>,
        target: &str,
    ) -> Result<(), ActionError> {
        opener::open_uri(cx, session, self.desktop.overrides(), target).await
    }

    async fn activate_action(
        &self,
        cx: &Cx,
        session: Option<Arc<zbus::Connection>>,
        bus_name: &str,
        object_path: &str,
        action: &str,
        parameter: Option<&str>,
    ) -> Result<(), ActionError> {
        opener::activate_action(cx, session, bus_name, object_path, action, parameter).await
    }
}

/// Reads the comm, then checks for an exit without blocking (`waitid` with `WNOHANG`).
fn probe_child(child: &Spawned) -> Result<Probe, ActionError> {
    use rustix::process::{WaitId, WaitIdOptions};
    use std::os::fd::AsFd;

    let comm = std::fs::read(format!("/proc/{}/stat", child.pid))
        .ok()
        .and_then(|stat| stat_comm(&stat));
    let status = loop {
        match rustix::process::waitid(
            WaitId::PidFd(child.pidfd.as_fd()),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
        ) {
            Err(rustix::io::Errno::INTR) => continue,
            other => break other,
        }
    };
    let exit = match status {
        Ok(None) => None,
        Ok(Some(status)) => match (status.exit_status(), status.terminating_signal()) {
            (Some(code), _) => Some(ExitKind::Exited(code)),
            (None, Some(signo)) => Some(ExitKind::Signaled(signo)),
            (None, None) => None,
        },
        Err(errno) => return Err(ActionError::Os(errno.raw_os_error())),
    };
    Ok(Probe { comm, exit })
}

/// The text between the first `(` and the last `)` of a `/proc/<pid>/stat` line.
fn stat_comm(stat: &[u8]) -> Option<String> {
    let open = stat.iter().position(|&b| b == b'(')?;
    let close = stat.iter().rposition(|&b| b == b')')?;
    let comm = stat.get(open + 1..close)?;
    Some(String::from_utf8_lossy(comm).into_owned())
}

async fn launch_with<H: Host>(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    spec: &LaunchSpec,
    host: &H,
) -> Result<LaunchOutcome, ActionError> {
    if spec.as_admin && !matches!(spec.what, LaunchWhat::Command(_)) {
        // ADR-006: only the Run dialog form has an administrator path.
        return Err(ActionError::Unsupported);
    }
    match &spec.what {
        LaunchWhat::Command(text) => launch_command(cx, session, spec, text, host).await,
        LaunchWhat::DesktopEntry(id) => launch_desktop_entry(cx, session, spec, id, host).await,
        LaunchWhat::DbusActivate {
            bus_name,
            object_path,
            action,
            parameter,
        } => {
            host.activate_action(
                cx,
                session,
                bus_name,
                object_path,
                action,
                parameter.as_deref(),
            )
            .await?;
            Ok(LaunchOutcome::Launched)
        }
        LaunchWhat::Unit(unit) => {
            if unit.manager != Manager::User {
                // The launcher only holds the session bus (R-134).
                return Err(ActionError::Unsupported);
            }
            let Some(conn) = session else {
                return Err(internal("no session bus"));
            };
            start_unit_and_wait(cx, &conn, &unit.name).await?;
            Ok(LaunchOutcome::Launched)
        }
    }
}

async fn launch_command<H: Host>(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    spec: &LaunchSpec,
    text: &str,
    host: &H,
) -> Result<LaunchOutcome, ActionError> {
    if text.chars().any(|c| c < ' ' && c != '\t') {
        return Err(ActionError::Unsupported);
    }
    let home = host.var("HOME").filter(|h| h.starts_with('/'));
    let cwd = spec
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| home.as_deref().map(PathBuf::from));

    if spec.as_admin {
        let values: Vec<Option<String>> = ADMIN_ENV_KEYS.iter().map(|k| host.var(k)).collect();
        let env: Vec<(&str, Option<&str>)> = ADMIN_ENV_KEYS
            .iter()
            .zip(&values)
            .map(|(k, v)| (*k, v.as_deref()))
            .collect();
        let argv = tl_ops::launch::admin_argv(text, &env).map_err(invalid_text)?;
        let child = host.spawn(cx, argv, cwd, Vec::new()).await?;
        return wait_admin(cx, host, child).await;
    }

    let trimmed = text.trim();
    if has_scheme(trimmed) {
        host.open_uri(cx, session, trimmed).await?;
        return Ok(LaunchOutcome::Launched);
    }
    if let Some(path) = openable_path(cx, trimmed, home.as_deref()).await {
        host.open_uri(cx, session, &path).await?;
        return Ok(LaunchOutcome::Launched);
    }
    let argv = if is_kcm(trimmed) {
        vec!["systemsettings".to_owned(), trimmed.to_owned()]
    } else {
        tl_ops::launch::shell_argv(text).map_err(invalid_text)?
    };
    let scope = format!("app-tasklens-run-{}.scope", rand_suffix());
    spawn_in_scope(cx, session, host, argv, cwd, &scope, text).await
}

async fn launch_desktop_entry<H: Host>(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    spec: &LaunchSpec,
    id: &str,
    host: &H,
) -> Result<LaunchOutcome, ActionError> {
    if id.is_empty() || id.starts_with('.') || id.contains(['/', '\0']) {
        return Err(ActionError::Unsupported);
    }
    let file_name = format!("{id}.desktop");
    if let Some(conn) = &session {
        let unit = autostart_unit_name(&file_name);
        match unit_active_state(cx, conn, &unit).await? {
            Some(state) if state == "active" => return Ok(LaunchOutcome::AlreadyRunning),
            Some(_) => {
                start_unit_and_wait(cx, conn, &unit).await?;
                return Ok(LaunchOutcome::Launched);
            }
            None => {}
        }
    }

    let env = host.startup_env();
    let mut find = cx
        .spawn_blocking(move |_| find_desktop_entry(&env, &file_name))
        .map_err(|_| internal("read job not admitted"))?;
    let (entry, path) = match find.join(cx).await {
        Ok(Some(found)) => found,
        Ok(None) => return Err(internal("desktop entry not found")),
        Err(_) => return Err(internal("read job failed")),
    };
    let Some(exec) = entry.exec.as_deref() else {
        return Err(internal("desktop entry has no Exec"));
    };
    let name = entry.name.as_deref().unwrap_or(id);
    let ctx = ExecContext {
        name,
        icon: entry.icon.as_deref(),
        desktop_file: path.to_str(),
    };
    let argv = tl_ops::launch::parse_exec(exec, &ctx).map_err(|e| ops_error(&e))?;
    let cwd = entry
        .path
        .as_deref()
        .filter(|p| !p.is_empty())
        .or(spec.cwd.as_deref())
        .map(PathBuf::from)
        .or_else(|| {
            host.var("HOME")
                .filter(|h| h.starts_with('/'))
                .map(PathBuf::from)
        });
    let mut scope = String::from("app-tasklens-");
    systemd_escape(id, &mut scope);
    scope.push('-');
    scope.push_str(&rand_suffix());
    scope.push_str(".scope");
    spawn_in_scope(cx, session, host, argv, cwd, &scope, name).await
}

/// Spawns `argv`, reaps it in the background and asks the user manager for a scope around it.
async fn spawn_in_scope<H: Host>(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    host: &H,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    scope: &str,
    description: &str,
) -> Result<LaunchOutcome, ActionError> {
    let child = host.spawn(cx, argv, cwd, host.session_env()).await?;
    let pid = host.pid(&child);
    host.detach(cx, child);
    match session {
        Some(conn) => {
            if let Err(e) = start_scope(cx, &conn, scope, pid, description).await {
                tracing::warn!(reason = e.reason(), "launcher: transient scope not created");
            }
        }
        None => tracing::warn!("launcher: no session bus, transient scope not created"),
    }
    Ok(LaunchOutcome::Launched)
}

/// Waits for the polkit prompt to close, then for the program to keep running for 2 s (R-253).
async fn wait_admin<H: Host>(
    cx: &Cx,
    host: &H,
    child: H::Child,
) -> Result<LaunchOutcome, ActionError> {
    loop {
        let probe = host.poll(cx, &child).await?;
        let in_pkexec = probe.comm.as_deref() == Some("pkexec");
        match (probe.exit, in_pkexec) {
            (Some(exit), true) => return Err(pkexec_exit(exit)),
            (Some(exit), false) => return program_exit(exit),
            (None, false) => break,
            (None, true) => {}
        }
        if pause(cx, ADMIN_POLL).await.is_err() {
            host.detach(cx, child);
            return Err(internal("cancelled"));
        }
    }

    let closed = cx.now();
    loop {
        if pause(cx, ADMIN_POLL).await.is_err() {
            host.detach(cx, child);
            return Err(internal("cancelled"));
        }
        let probe = host.poll(cx, &child).await?;
        if let Some(exit) = probe.exit {
            return program_exit(exit);
        }
        if Duration::from_nanos(cx.now().duration_since(closed)) >= ADMIN_GRACE {
            host.detach(cx, child);
            return Ok(LaunchOutcome::Launched);
        }
    }
}

async fn pause(cx: &Cx, d: Duration) -> Result<(), ()> {
    time::sleep(cx.now(), d).await;
    cx.checkpoint().map_err(drop)
}

/// The `pkexec` process itself ended: the prompt was dismissed or refused (R-117).
fn pkexec_exit(exit: ExitKind) -> ActionError {
    match exit {
        ExitKind::Exited(PKEXEC_DISMISSED) => ActionError::AuthCancelled,
        ExitKind::Exited(PKEXEC_NOT_AUTHORIZED) => ActionError::NotAuthorized,
        ExitKind::Exited(n) => {
            ActionError::Internal(format!("pkexec exited with status {n}").into())
        }
        ExitKind::Signaled(n) => {
            ActionError::Internal(format!("pkexec ended by signal {n}").into())
        }
    }
}

/// The program ended within 2 s of the prompt closing.
fn program_exit(exit: ExitKind) -> Result<LaunchOutcome, ActionError> {
    match exit {
        ExitKind::Exited(0) => Ok(LaunchOutcome::Launched),
        ExitKind::Exited(n) => Err(ActionError::Internal(
            format!("exited with status {n}").into(),
        )),
        ExitKind::Signaled(n) => Err(ActionError::Internal(format!("ended by signal {n}").into())),
    }
}

/// `^[A-Za-z][A-Za-z0-9+.-]*:`
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
}

/// `^kcm_[A-Za-z0-9_]+$`
fn is_kcm(text: &str) -> bool {
    text.strip_prefix("kcm_").is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// The path to open instead of run: `text` (with `~/` expanded) names an existing directory, or a
/// file the user cannot execute.
async fn openable_path(cx: &Cx, text: &str, home: Option<&str>) -> Option<String> {
    let path = if let Some(rest) = text.strip_prefix("~/") {
        format!("{}/{rest}", home?.trim_end_matches('/'))
    } else if text.starts_with('/') {
        text.to_owned()
    } else {
        return None;
    };
    let checked = path.clone();
    let mut job = cx
        .spawn_blocking(move |_| {
            let path = Path::new(&checked);
            match std::fs::metadata(path) {
                Ok(meta) if meta.is_dir() => true,
                Ok(_) => rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_err(),
                Err(_) => false,
            }
        })
        .ok()?;
    match job.join(cx).await {
        Ok(true) => Some(path),
        _ => None,
    }
}

/// `<file_name>` in `$XDG_CONFIG_HOME/autostart`, then each `$XDG_CONFIG_DIRS` entry plus
/// `/autostart` (SYS-019). The first file that can be read wins.
fn find_desktop_entry(env: &StartupEnv, file_name: &str) -> Option<(DesktopEntry, PathBuf)> {
    let locales: SmallVec<[&str; 4]> = env.locales.iter().map(String::as_str).collect();
    let user = Some(&env.config_home).filter(|d| d.is_absolute());
    let system = env.config_dirs.iter().filter(|d| d.is_absolute());
    for base in user.into_iter().chain(system) {
        let path = base.join("autostart").join(file_name);
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_DESKTOP_FILE as u64)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        return Some((parse_desktop_entry(&bytes, &locales), path));
    }
    None
}

/// 8 lowercase hex digits from 4 bytes of `/dev/urandom` (R-167 item 12).
fn rand_suffix() -> String {
    let mut bytes = [0u8; 4];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        // A fixed suffix only risks a name clash, which fails the scope and not the launch.
        tracing::warn!("launcher: /dev/urandom not readable");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why a D-Bus call failed. Only the error name is kept; the message can hold a unit name.
#[derive(Debug)]
enum CallError {
    Timeout,
    Cancelled,
    Dbus(Option<Arc<str>>),
}

impl CallError {
    fn reason(&self) -> &str {
        match self {
            CallError::Timeout => "timeout",
            CallError::Cancelled => "cancelled",
            CallError::Dbus(Some(name)) => name,
            CallError::Dbus(None) => "zbus error",
        }
    }

    fn into_action_error(self) -> ActionError {
        internal(self.reason())
    }
}

async fn timed<T>(cx: &Cx, fut: impl Future<Output = zbus::Result<T>>) -> Result<T, CallError> {
    let result = time::timeout(cx.now(), CALL_TIMEOUT, fut).await;
    if cx.checkpoint().is_err() {
        return Err(CallError::Cancelled);
    }
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(CallError::Dbus(crate::dbus::error_name(&e).map(Arc::from))),
        Err(_) => Err(CallError::Timeout),
    }
}

/// The `ActiveState` of `name` on the user manager, or `None` when its `LoadState` is
/// `not-found`.
async fn unit_active_state(
    cx: &Cx,
    conn: &zbus::Connection,
    name: &str,
) -> Result<Option<String>, ActionError> {
    let body = (name,);
    let call = conn.call_method(
        Some(SYSTEMD_DEST),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "LoadUnit",
        &body,
    );
    let reply = match timed(cx, call).await {
        Ok(reply) => reply,
        Err(CallError::Dbus(Some(e))) if &*e == NO_SUCH_UNIT => return Ok(None),
        Err(e) => return Err(e.into_action_error()),
    };
    let path: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|_| internal("bad LoadUnit reply"))?;
    let call = conn.call_method(
        Some(SYSTEMD_DEST),
        &path,
        Some(PROPERTIES_IFACE),
        "GetAll",
        &(UNIT_IFACE,),
    );
    let reply = timed(cx, call)
        .await
        .map_err(CallError::into_action_error)?;
    let props: HashMap<String, OwnedValue> = reply
        .body()
        .deserialize()
        .map_err(|_| internal("bad GetAll reply"))?;
    let text = |key: &str| match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.as_str().to_owned(),
        _ => String::new(),
    };
    if text("LoadState") == "not-found" {
        return Ok(None);
    }
    Ok(Some(text("ActiveState")))
}

/// `StartUnit(name, "replace")` on the user manager, then waits up to 30 s for the job's
/// `JobRemoved`. The match is added before the call so the signal cannot be missed.
async fn start_unit_and_wait(
    cx: &Cx,
    conn: &zbus::Connection,
    name: &str,
) -> Result<(), ActionError> {
    let rule = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(SYSTEMD_DEST)
        .and_then(|b| b.path(SYSTEMD_PATH))
        .and_then(|b| b.interface(MANAGER_IFACE))
        .and_then(|b| b.member("JobRemoved"))
        .map_err(|_| internal("bad match rule"))?
        .build();
    let subscribe = zbus::MessageStream::for_match_rule(rule, conn, None);
    let mut stream = timed(cx, subscribe)
        .await
        .map_err(CallError::into_action_error)?;

    let body = (name, "replace");
    let call = conn.call_method(
        Some(SYSTEMD_DEST),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "StartUnit",
        &body,
    );
    let reply = match timed(cx, call).await {
        Ok(reply) => reply,
        Err(CallError::Dbus(Some(e))) if &*e == ACCESS_DENIED => {
            return Err(ActionError::NotAuthorized);
        }
        Err(CallError::Dbus(error_name)) => {
            return Err(ActionError::UnitJob {
                result: "failed".into(),
                error_name,
            });
        }
        Err(e) => return Err(e.into_action_error()),
    };
    let job: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|_| internal("bad StartUnit reply"))?;

    let start = cx.now();
    loop {
        let waited = Duration::from_nanos(cx.now().duration_since(start));
        if waited >= JOB_WAIT {
            return Err(internal("job wait timed out"));
        }
        let next = time::timeout(
            cx.now(),
            (JOB_WAIT - waited).min(CALL_TIMEOUT),
            stream.next(),
        )
        .await;
        if cx.checkpoint().is_err() {
            return Err(internal("cancelled"));
        }
        let msg = match next {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(_))) | Err(_) => continue,
            Ok(None) => return Err(internal("JobRemoved stream ended")),
        };
        let Ok((_, path, _, result)) = msg
            .body()
            .deserialize::<(u32, OwnedObjectPath, String, String)>()
        else {
            continue;
        };
        if path != job {
            continue;
        }
        if result == "done" {
            return Ok(());
        }
        return Err(ActionError::UnitJob {
            result: result.into(),
            error_name: None,
        });
    }
}

/// `StartTransientUnit(scope, "fail", [PIDs, Description, CollectMode], [])` on the user manager.
async fn start_scope(
    cx: &Cx,
    conn: &zbus::Connection,
    scope: &str,
    pid: u32,
    description: &str,
) -> Result<(), CallError> {
    let props: Vec<(&str, Value<'_>)> = vec![
        ("PIDs", Value::from(vec![pid])),
        ("Description", Value::from(description)),
        ("CollectMode", Value::from("inactive-or-failed")),
    ];
    let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = Vec::new();
    let body = (scope, "fail", props, aux);
    let call = conn.call_method(
        Some(SYSTEMD_DEST),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "StartTransientUnit",
        &body,
    );
    timed(cx, call).await.map(drop)
}

/// Invalid command text from the tl-ops argv builders (R-148 item 3).
fn invalid_text(e: OpsError) -> ActionError {
    match e {
        OpsError::InvalidInput(_) => ActionError::Unsupported,
        other => ops_error(&other),
    }
}

fn ops_error(e: &OpsError) -> ActionError {
    match e.to_action_state() {
        ActionState::Failed(err) => err,
        _ => internal(&e.to_string()),
    }
}

/// A failed spawn names the program, so "not found" and "not executable" say which one. The
/// error travels in the action's result only; launcher logs never carry it (ADR-015).
fn spawn_error(program: &str, e: &OpsError) -> ActionError {
    match e {
        OpsError::Os(code) => ActionError::Internal(
            format!("{program}: {}", std::io::Error::from_raw_os_error(*code)).into(),
        ),
        _ => ops_error(e),
    }
}

fn internal(reason: &str) -> ActionError {
    ActionError::Internal(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use asupersync::runtime::Runtime;
    use tl_model::table::UnitRef;

    use crate::desktop::DesktopSource;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::ObjectPath;

    type Log = Arc<Mutex<Vec<String>>>;

    const FAKE_PID: u32 = 4242;
    const UNIT_PATH: &str = "/org/freedesktop/systemd1/unit/fake";
    const JOB_PATH: &str = "/org/freedesktop/systemd1/job/7";

    fn record(log: &Log, entry: String) {
        log.lock().expect("log").push(entry);
    }

    fn entries(log: &Log) -> Vec<String> {
        log.lock().expect("log").clone()
    }

    /// The scripted answers of `FakeHost::poll`: poll `i` sees `comms[min(i, len - 1)]`, and the
    /// exit from poll `exit_at.0` on.
    #[derive(Default)]
    struct Script {
        comms: Vec<&'static str>,
        exit_at: Option<(usize, ExitKind)>,
    }

    #[derive(Default)]
    struct FakeHost {
        vars: Vec<(&'static str, String)>,
        env: StartupEnv,
        script: Script,
        spawns: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
        log: Log,
        polls: AtomicUsize,
        detached: AtomicUsize,
    }

    impl FakeHost {
        fn spawned(&self) -> Vec<Vec<String>> {
            let spawns = self.spawns.lock().expect("spawns");
            spawns.iter().map(|(argv, _)| argv.clone()).collect()
        }
    }

    impl Host for FakeHost {
        type Child = u32;

        fn var(&self, key: &str) -> Option<String> {
            self.vars
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        }

        fn startup_env(&self) -> StartupEnv {
            self.env.clone()
        }

        async fn spawn(
            &self,
            _cx: &Cx,
            argv: Vec<String>,
            cwd: Option<PathBuf>,
            _env: Vec<EnvChange>,
        ) -> Result<u32, ActionError> {
            self.spawns.lock().expect("spawns").push((argv, cwd));
            Ok(FAKE_PID)
        }

        fn pid(&self, child: &u32) -> u32 {
            *child
        }

        fn detach(&self, _cx: &Cx, _child: u32) {
            self.detached.fetch_add(1, Ordering::SeqCst);
        }

        async fn poll(&self, _cx: &Cx, _child: &u32) -> Result<Probe, ActionError> {
            let i = self.polls.fetch_add(1, Ordering::SeqCst);
            let comms = &self.script.comms;
            let comm = comms[i.min(comms.len() - 1)].to_owned();
            let exit = self
                .script
                .exit_at
                .filter(|(at, _)| i >= *at)
                .map(|(_, e)| e);
            Ok(Probe {
                comm: Some(comm),
                exit,
            })
        }

        async fn open_uri(
            &self,
            _cx: &Cx,
            _session: Option<Arc<zbus::Connection>>,
            target: &str,
        ) -> Result<(), ActionError> {
            record(&self.log, format!("open_uri {target}"));
            Ok(())
        }

        async fn activate_action(
            &self,
            _cx: &Cx,
            _session: Option<Arc<zbus::Connection>>,
            bus_name: &str,
            object_path: &str,
            action: &str,
            parameter: Option<&str>,
        ) -> Result<(), ActionError> {
            record(
                &self.log,
                format!("activate_action {bus_name} {object_path} {action} {parameter:?}"),
            );
            Ok(())
        }
    }

    /// The user manager: `LoadUnit` always answers `UNIT_PATH`, `StartUnit` emits `JobRemoved`
    /// with `job_result` before it replies.
    struct FakeManager {
        log: Log,
        job_result: &'static str,
        scope_fails: bool,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeManager {
        fn load_unit(&self, name: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            record(&self.log, format!("LoadUnit {name}"));
            Ok(OwnedObjectPath::try_from(UNIT_PATH).expect("path"))
        }

        async fn start_unit(
            &self,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
            name: &str,
            mode: &str,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            record(&self.log, format!("StartUnit {name} {mode}"));
            let other = ObjectPath::try_from("/org/freedesktop/systemd1/job/6").expect("path");
            let job = ObjectPath::try_from(JOB_PATH).expect("path");
            // Another job's signal first: only the returned job path counts.
            Self::job_removed(&emitter, 6, other, name, "done").await?;
            Self::job_removed(&emitter, 7, job, name, self.job_result).await?;
            Ok(OwnedObjectPath::try_from(JOB_PATH).expect("path"))
        }

        fn start_transient_unit(
            &self,
            name: &str,
            mode: &str,
            props: Vec<(String, OwnedValue)>,
            aux: Vec<(String, Vec<(String, OwnedValue)>)>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let shown: Vec<String> = props
                .iter()
                .map(|(k, v)| format!("{k}={}", show(v)))
                .collect();
            record(
                &self.log,
                format!("StartTransientUnit {name} {mode} {shown:?} {}", aux.len()),
            );
            if self.scope_fails {
                return Err(zbus::fdo::Error::Failed("no".into()));
            }
            Ok(OwnedObjectPath::try_from(JOB_PATH).expect("path"))
        }

        #[zbus(signal)]
        async fn job_removed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            job: ObjectPath<'_>,
            unit: &str,
            result: &str,
        ) -> zbus::Result<()>;
    }

    fn show(v: &Value<'_>) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            Value::Array(a) => {
                let items: Vec<String> = a.inner().iter().map(show).collect();
                format!("[{}]", items.join(","))
            }
            Value::U32(n) => n.to_string(),
            other => format!("{other:?}"),
        }
    }

    struct FakeUnit {
        load_state: &'static str,
        active_state: &'static str,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl FakeUnit {
        #[zbus(property)]
        fn load_state(&self) -> String {
            self.load_state.to_owned()
        }

        #[zbus(property)]
        fn active_state(&self) -> String {
            self.active_state.to_owned()
        }
    }

    struct FakeSystemd {
        log: Log,
        _server: zbus::Connection,
        client: Arc<zbus::Connection>,
    }

    impl FakeSystemd {
        fn calls(&self) -> Vec<String> {
            entries(&self.log)
        }
    }

    /// A peer-to-peer pair over a socketpair; the server side plays the user manager.
    fn fake_systemd(
        unit: (&'static str, &'static str),
        job_result: &'static str,
        scope_fails: bool,
    ) -> FakeSystemd {
        let log = Log::default();
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(
                SYSTEMD_PATH,
                FakeManager {
                    log: Arc::clone(&log),
                    job_result,
                    scope_fails,
                },
            )
            .expect("manager")
            .serve_at(
                UNIT_PATH,
                FakeUnit {
                    load_state: unit.0,
                    active_state: unit.1,
                },
            )
            .expect("unit");
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server.build(), client));
        FakeSystemd {
            log,
            _server: server.expect("server connects"),
            client: Arc::new(client.expect("client connects")),
        }
    }

    fn run<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
        let rt: Runtime = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(f);
        rt.block_on(join).expect("task completes")
    }

    fn command(text: &str, as_admin: bool) -> LaunchSpec {
        LaunchSpec {
            what: LaunchWhat::Command(text.into()),
            as_admin,
            cwd: None,
        }
    }

    /// Runs `spec` against `host` and the session connection `session`.
    fn go(
        host: &Arc<FakeHost>,
        session: Option<Arc<zbus::Connection>>,
        spec: LaunchSpec,
    ) -> Result<LaunchOutcome, ActionError> {
        let host = Arc::clone(host);
        run(async move {
            let cx = Cx::current().expect("task has a Cx");
            launch_with(&cx, session, &spec, &*host).await
        })
    }

    /// A directory under `target/tmp`, removed on drop.
    fn scratch() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("tasklens-y6k3-")
            .tempdir_in(base)
            .expect("tempdir")
    }

    fn write(path: &Path, text: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, text).expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    fn is_scope(name: &str, prefix: &str) -> bool {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".scope"))
            .is_some_and(|hex| {
                hex.len() == 8 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            })
    }

    /// Splits a recorded `StartTransientUnit` entry into its name and the rest.
    fn scope_call(calls: &[String]) -> (String, String) {
        let call = calls
            .iter()
            .find(|c| c.starts_with("StartTransientUnit "))
            .expect("a scope was requested");
        let rest = &call["StartTransientUnit ".len()..];
        let (name, tail) = rest.split_once(' ').expect("name");
        (name.to_owned(), tail.to_owned())
    }

    #[test]
    fn rejects_nul_and_controls() {
        for text in ["echo\u{0}x", "echo\u{1b}[31m", "a\nb", "a\rb"] {
            for as_admin in [false, true] {
                let host = Arc::new(FakeHost::default());
                let result = go(&host, None, command(text, as_admin));
                assert_eq!(result, Err(ActionError::Unsupported), "{text:?}");
                assert!(host.spawned().is_empty(), "nothing spawned for {text:?}");
                assert!(entries(&host.log).is_empty(), "nothing opened for {text:?}");
            }
        }
    }

    #[test]
    fn real_entry_point_rejects_controls() {
        let result = run(async move {
            let cx = Cx::current().expect("task has a Cx");
            launch(&cx, None, &local_desktop(), &command("echo\u{0}x", false)).await
        });
        assert_eq!(result, Err(ActionError::Unsupported));
    }

    #[test]
    fn scheme_goes_to_opener() {
        let host = Arc::new(FakeHost::default());
        let result = go(&host, None, command("  https://example.org ", false));
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(entries(&host.log), ["open_uri https://example.org"]);
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn non_executable_path_goes_to_opener() {
        let dir = scratch();
        let root = dir.path().to_str().expect("utf-8").to_owned();
        write(&dir.path().join("x.txt"), "text", 0o644);
        write(&dir.path().join("run.sh"), "#!/bin/sh\n", 0o755);
        let host = Arc::new(FakeHost {
            vars: vec![("HOME", root.clone())],
            ..FakeHost::default()
        });

        let file = format!("{root}/x.txt");
        assert_eq!(
            go(&host, None, command(&file, false)),
            Ok(LaunchOutcome::Launched)
        );
        assert_eq!(
            go(&host, None, command(&root, false)),
            Ok(LaunchOutcome::Launched)
        );
        assert_eq!(
            go(&host, None, command("~/x.txt", false)),
            Ok(LaunchOutcome::Launched)
        );
        assert_eq!(
            entries(&host.log),
            [
                format!("open_uri {file}"),
                format!("open_uri {root}"),
                format!("open_uri {file}"),
            ]
        );
        assert!(host.spawned().is_empty());

        // An executable file runs through the shell; a missing path is a command too.
        let script = format!("{root}/run.sh");
        go(&host, None, command(&script, false)).expect("launched");
        go(&host, None, command("/no/such/tasklens-y6k3", false)).expect("launched");
        assert_eq!(
            host.spawned(),
            [
                vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "/no/such/tasklens-y6k3".to_owned()
                ],
            ]
        );
        assert_eq!(entries(&host.log).len(), 3, "no further opens");
    }

    #[test]
    fn kcm_runs_systemsettings() {
        let host = Arc::new(FakeHost::default());
        let result = go(&host, None, command("kcm_kwin_effects", false));
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(host.spawned(), [vec!["systemsettings", "kcm_kwin_effects"]]);
        assert!(is_kcm("kcm_a_1"));
        assert!(!is_kcm("kcm_"));
        assert!(!is_kcm("kcm_x; rm"));
        assert!(!is_kcm("xkcm_x"));
    }

    #[test]
    fn shell_command_argv_and_scope_name() {
        let fake = fake_systemd(("loaded", "inactive"), "done", false);
        let host = Arc::new(FakeHost {
            vars: vec![("HOME", "/home/u".to_owned())],
            ..FakeHost::default()
        });
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            command("firefox --new-window", false),
        );
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        let spawns = host.spawns.lock().expect("spawns").clone();
        assert_eq!(
            spawns,
            [(
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "firefox --new-window".to_owned()
                ],
                Some(PathBuf::from("/home/u"))
            )]
        );
        assert_eq!(host.detached.load(Ordering::SeqCst), 1, "child is reaped");
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let (name, rest) = scope_call(&calls);
        assert!(is_scope(&name, "app-tasklens-run-"), "{name}");
        assert_eq!(
            rest,
            "fail [\"PIDs=[4242]\", \"Description=firefox --new-window\", \
             \"CollectMode=inactive-or-failed\"] 0"
        );
    }

    #[test]
    fn spec_cwd_wins_over_home() {
        let host = Arc::new(FakeHost {
            vars: vec![("HOME", "/home/u".to_owned())],
            ..FakeHost::default()
        });
        let mut spec = command("true", false);
        spec.cwd = Some("/srv".into());
        go(&host, None, spec).expect("launched");
        let spawns = host.spawns.lock().expect("spawns").clone();
        assert_eq!(spawns[0].1, Some(PathBuf::from("/srv")));
    }

    fn admin_host(vars: Vec<(&'static str, String)>, script: Script) -> Arc<FakeHost> {
        Arc::new(FakeHost {
            vars,
            script,
            ..FakeHost::default()
        })
    }

    #[test]
    fn admin_argv_only_set_vars_no_scope() {
        let fake = fake_systemd(("loaded", "inactive"), "done", false);
        let host = admin_host(
            vec![
                ("WAYLAND_DISPLAY", "wayland-0".to_owned()),
                ("XDG_RUNTIME_DIR", "/run/user/1000".to_owned()),
            ],
            Script {
                comms: vec!["env"],
                exit_at: None,
            },
        );
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            command("gparted", true),
        );
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(
            host.spawned(),
            [vec![
                tl_ops::launch::PKEXEC_PATH,
                "env",
                "WAYLAND_DISPLAY=wayland-0",
                "XDG_RUNTIME_DIR=/run/user/1000",
                "/bin/sh",
                "-c",
                "gparted",
            ]]
        );
        assert!(fake.calls().is_empty(), "no scope: {:?}", fake.calls());
        assert!(entries(&host.log).is_empty());
    }

    #[test]
    fn admin_uri_still_goes_through_pkexec() {
        let host = admin_host(
            vec![],
            Script {
                comms: vec!["pkexec"],
                exit_at: Some((0, ExitKind::Exited(PKEXEC_DISMISSED))),
            },
        );
        let result = go(&host, None, command("https://example.org", true));
        assert_eq!(result, Err(ActionError::AuthCancelled));
        assert!(entries(&host.log).is_empty(), "no opener");
        assert_eq!(host.spawned()[0][0], tl_ops::launch::PKEXEC_PATH);
    }

    #[test]
    fn admin_exit_126_auth_cancelled() {
        let host = admin_host(
            vec![],
            Script {
                comms: vec!["pkexec"],
                exit_at: Some((1, ExitKind::Exited(126))),
            },
        );
        assert_eq!(
            go(&host, None, command("true", true)),
            Err(ActionError::AuthCancelled)
        );
        assert_eq!(host.detached.load(Ordering::SeqCst), 0, "reaped by poll");
    }

    #[test]
    fn admin_exit_127_not_authorized() {
        let host = admin_host(
            vec![],
            Script {
                comms: vec!["pkexec"],
                exit_at: Some((1, ExitKind::Exited(127))),
            },
        );
        assert_eq!(
            go(&host, None, command("true", true)),
            Err(ActionError::NotAuthorized)
        );
    }

    #[test]
    fn admin_running_after_2s_launched() {
        let host = admin_host(
            vec![],
            Script {
                comms: vec!["pkexec", "pkexec", "sh"],
                exit_at: None,
            },
        );
        let started = std::time::Instant::now();
        assert_eq!(
            go(&host, None, command("true", true)),
            Ok(LaunchOutcome::Launched)
        );
        assert!(started.elapsed() >= ADMIN_GRACE, "waited the 2 s grace");
        assert_eq!(host.detached.load(Ordering::SeqCst), 1, "child is reaped");
        // Two prompt polls, the poll that saw "sh", then one grace poll every 250 ms.
        assert!(host.polls.load(Ordering::SeqCst) >= 10);
    }

    #[test]
    fn admin_prompt_closed_by_comm_change() {
        let comms = vec!["pkexec", "pkexec", "env"];
        let alive = admin_host(
            vec![],
            Script {
                comms: comms.clone(),
                exit_at: None,
            },
        );
        assert_eq!(
            go(&alive, None, command("true", true)),
            Ok(LaunchOutcome::Launched)
        );

        let ok = admin_host(
            vec![],
            Script {
                comms: comms.clone(),
                exit_at: Some((4, ExitKind::Exited(0))),
            },
        );
        assert_eq!(
            go(&ok, None, command("true", true)),
            Ok(LaunchOutcome::Launched)
        );
        assert_eq!(ok.polls.load(Ordering::SeqCst), 5, "returns at the exit");
        assert_eq!(ok.detached.load(Ordering::SeqCst), 0);

        let failed = admin_host(
            vec![],
            Script {
                comms,
                exit_at: Some((4, ExitKind::Exited(2))),
            },
        );
        assert_eq!(
            go(&failed, None, command("true", true)),
            Err(internal("exited with status 2"))
        );
    }

    #[test]
    fn admin_exit_before_prompt_closed() {
        let host = admin_host(
            vec![],
            Script {
                comms: vec!["pkexec"],
                exit_at: Some((2, ExitKind::Exited(1))),
            },
        );
        assert_eq!(
            go(&host, None, command("true", true)),
            Err(internal("pkexec exited with status 1"))
        );
    }

    #[test]
    fn dbus_activate_goes_to_opener() {
        let host = Arc::new(FakeHost::default());
        let spec = LaunchSpec {
            what: LaunchWhat::DbusActivate {
                bus_name: "org.gnome.Settings".into(),
                object_path: "/org/gnome/Settings".into(),
                action: "launch-panel".into(),
                parameter: Some("network".into()),
            },
            as_admin: false,
            cwd: None,
        };
        assert_eq!(go(&host, None, spec), Ok(LaunchOutcome::Launched));
        assert_eq!(
            entries(&host.log),
            [
                "activate_action org.gnome.Settings /org/gnome/Settings launch-panel Some(\"network\")"
            ]
        );
        assert!(host.spawned().is_empty());
    }

    fn desktop(id: &str) -> LaunchSpec {
        LaunchSpec {
            what: LaunchWhat::DesktopEntry(id.into()),
            as_admin: false,
            cwd: None,
        }
    }

    #[test]
    fn desktop_entry_already_running() {
        let fake = fake_systemd(("loaded", "active"), "done", false);
        let host = Arc::new(FakeHost::default());
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            desktop("org.kde.foo"),
        );
        assert_eq!(result, Ok(LaunchOutcome::AlreadyRunning));
        assert_eq!(fake.calls(), ["LoadUnit app-org.kde.foo@autostart.service"]);
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn desktop_entry_starts_generated_unit() {
        let fake = fake_systemd(("loaded", "inactive"), "done", false);
        let host = Arc::new(FakeHost::default());
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            desktop("org.kde.foo"),
        );
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(
            fake.calls(),
            [
                "LoadUnit app-org.kde.foo@autostart.service",
                "StartUnit app-org.kde.foo@autostart.service replace",
            ]
        );
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn desktop_entry_exec_and_scope() {
        let dir = scratch();
        let user = dir.path().join("config");
        let system = dir.path().join("xdg");
        std::fs::create_dir_all(user.join("autostart")).expect("mkdir");
        std::fs::create_dir_all(system.join("autostart")).expect("mkdir");
        let entry = "[Desktop Entry]\nType=Application\nName=My App\nExec=foo %U --bar\n";
        write(&user.join("autostart/my-app.desktop"), entry, 0o644);
        let shadowed = "[Desktop Entry]\nName=Other\nExec=other\n";
        write(&system.join("autostart/my-app.desktop"), shadowed, 0o644);

        let fake = fake_systemd(("not-found", "inactive"), "done", false);
        let host = Arc::new(FakeHost {
            vars: vec![("HOME", "/home/u".to_owned())],
            env: StartupEnv {
                config_home: user,
                config_dirs: vec![system],
                ..StartupEnv::default()
            },
            ..FakeHost::default()
        });
        let result = go(&host, Some(Arc::clone(&fake.client)), desktop("my-app"));
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(host.spawned(), [vec!["foo", "--bar"]]);
        let calls = fake.calls();
        assert_eq!(calls[0], "LoadUnit app-my\\x2dapp@autostart.service");
        assert!(
            !calls.iter().any(|c| c.starts_with("StartUnit")),
            "{calls:?}"
        );
        let (name, rest) = scope_call(&calls);
        assert!(is_scope(&name, "app-tasklens-my\\x2dapp-"), "{name}");
        assert!(rest.contains("\"Description=My App\""), "{rest}");
        assert!(rest.contains("\"PIDs=[4242]\""), "{rest}");
    }

    #[test]
    fn desktop_entry_system_dir_and_path_key() {
        let dir = scratch();
        let system = dir.path().join("xdg");
        std::fs::create_dir_all(system.join("autostart")).expect("mkdir");
        let entry = "[Desktop Entry]\nName=Sys\nExec=sys-app --x\nPath=/opt/sys\n";
        write(&system.join("autostart/sys.desktop"), entry, 0o644);
        let host = Arc::new(FakeHost {
            env: StartupEnv {
                config_home: dir.path().join("missing"),
                config_dirs: vec![dir.path().join("none"), system],
                ..StartupEnv::default()
            },
            ..FakeHost::default()
        });
        // No session: no unit check, no scope.
        assert_eq!(go(&host, None, desktop("sys")), Ok(LaunchOutcome::Launched));
        let spawns = host.spawns.lock().expect("spawns").clone();
        assert_eq!(
            spawns,
            [(
                vec!["sys-app".to_owned(), "--x".to_owned()],
                Some(PathBuf::from("/opt/sys"))
            )]
        );
        assert_eq!(
            go(&host, None, desktop("absent")),
            Err(internal("desktop entry not found"))
        );
        for bad in ["../x", "", ".hidden", "a/b"] {
            assert_eq!(go(&host, None, desktop(bad)), Err(ActionError::Unsupported));
        }
    }

    #[test]
    fn scope_failure_not_fatal() {
        let fake = fake_systemd(("loaded", "inactive"), "done", true);
        let host = Arc::new(FakeHost::default());
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            command("true", false),
        );
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(host.spawned().len(), 1);
        assert_eq!(fake.calls().len(), 1, "the scope was asked for");
    }

    fn unit(manager: Manager, name: &str) -> LaunchSpec {
        LaunchSpec {
            what: LaunchWhat::Unit(UnitRef {
                manager,
                name: name.into(),
            }),
            as_admin: false,
            cwd: None,
        }
    }

    #[test]
    fn unit_job_failed_maps_unitjob() {
        let fake = fake_systemd(("loaded", "inactive"), "failed", false);
        let host = Arc::new(FakeHost::default());
        let result = go(
            &host,
            Some(Arc::clone(&fake.client)),
            unit(Manager::User, "x.service"),
        );
        assert_eq!(
            result,
            Err(ActionError::UnitJob {
                result: "failed".into(),
                error_name: None
            })
        );
        assert_eq!(fake.calls(), ["StartUnit x.service replace"]);

        let done = fake_systemd(("loaded", "inactive"), "done", false);
        let result = go(
            &host,
            Some(Arc::clone(&done.client)),
            unit(Manager::User, "x.service"),
        );
        assert_eq!(result, Ok(LaunchOutcome::Launched));
        assert_eq!(
            go(&host, None, unit(Manager::System, "x.service")),
            Err(ActionError::Unsupported)
        );
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn admin_only_for_commands() {
        let host = Arc::new(FakeHost::default());
        let mut spec = desktop("foo");
        spec.as_admin = true;
        assert_eq!(go(&host, None, spec), Err(ActionError::Unsupported));
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn stat_comm_parses() {
        assert_eq!(stat_comm(b"42 (pkexec) S 1 42").as_deref(), Some("pkexec"));
        assert_eq!(stat_comm(b"42 (a) b)) S 1").as_deref(), Some("a) b)"));
        assert_eq!(stat_comm(b"garbage"), None);
    }

    #[test]
    fn rand_suffix_is_8_lower_hex() {
        let a = rand_suffix();
        assert_eq!(a.len(), 8, "{a}");
        assert!(
            a.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{a}"
        );
        assert_ne!(a, rand_suffix(), "two suffixes differ");
    }

    fn local_desktop() -> DesktopSession {
        DesktopSession::new(DesktopSource::Process)
    }

    fn real_host() -> RealHost {
        RealHost {
            desktop: local_desktop(),
        }
    }

    /// ADR-018: under the engine service a program starts with the desktop session's
    /// variables, set on the spawn itself, and the session's values win for those keys only.
    #[test]
    fn real_host_takes_the_session_under_the_service() {
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        let host = RealHost {
            desktop: desktop.clone(),
        };
        assert!(host.session_env().is_empty(), "no session yet");
        assert_eq!(host.var("WAYLAND_DISPLAY"), None);
        let session = crate::desktop::DesktopEnv::from_lines(&[
            "WAYLAND_DISPLAY=wayland-1",
            "HOME=/elsewhere",
        ]);
        desktop.set(Some(session.clone()));
        assert_eq!(host.session_env(), session.child_env());
        assert_eq!(host.var("WAYLAND_DISPLAY").as_deref(), Some("wayland-1"));
        assert_eq!(host.var("HOME"), std::env::var("HOME").ok());
        // The window's engine: children inherit the process environment as before.
        assert!(real_host().session_env().is_empty());
    }

    /// Leader decision on q4x76.12: a launch spawns the program itself with the session's
    /// variables on the spawn, never through `env` or a shell. A missing program and a file
    /// that is not executable fail the launch with an error that names the program, and a
    /// program that starts gets the session's `DISPLAY` and `WAYLAND_DISPLAY`.
    #[test]
    fn launch_spawns_the_program_itself_with_the_session() {
        let dir = scratch();
        let missing = dir.path().join("no-such-program");
        let plain = dir.path().join("not-executable");
        write(&plain, "#!/bin/sh\n", 0o644);
        let out = dir.path().join("env");
        let script = format!(
            "printf '%s|%s' \"$DISPLAY\" \"$WAYLAND_DISPLAY\" > '{}'",
            out.display()
        );
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        desktop.set(Some(crate::desktop::DesktopEnv::from_lines(&[
            "WAYLAND_DISPLAY=wayland-5",
            "DISPLAY=:5",
        ])));
        let host = Arc::new(RealHost { desktop });
        let start = |argv: Vec<String>| {
            let host = Arc::clone(&host);
            run(async move {
                let cx = Cx::current().expect("task has a Cx");
                spawn_in_scope(&cx, None, &*host, argv, None, "app-test.scope", "test").await
            })
        };

        for program in [&missing, &plain] {
            let program = program.to_str().expect("utf-8").to_owned();
            match start(vec![program.clone()]) {
                Err(ActionError::Internal(text)) => {
                    assert!(text.starts_with(&format!("{program}: ")), "{text}");
                }
                other => panic!("{program}: {other:?}"),
            }
        }

        let argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), script];
        assert_eq!(start(argv), Ok(LaunchOutcome::Launched));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let got = loop {
            match std::fs::read_to_string(&out) {
                Ok(text) if !text.is_empty() => break text,
                _ if std::time::Instant::now() > deadline => panic!("the program never ran"),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert_eq!(got, ":5|wayland-5");
    }

    /// Spawns `/bin/true` for real and waits for it through `RealHost::poll`.
    #[test]
    fn real_host_poll_reaps_true() {
        let probe = run(async move {
            let cx = Cx::current().expect("task has a Cx");
            let child = real_host()
                .spawn(&cx, vec!["/bin/true".to_owned()], None, Vec::new())
                .await
                .expect("spawned");
            loop {
                let probe = real_host().poll(&cx, &child).await.expect("probe");
                if probe.exit.is_some() {
                    break probe;
                }
                time::sleep(cx.now(), Duration::from_millis(20)).await;
            }
        });
        assert_eq!(probe.exit, Some(ExitKind::Exited(0)));
    }
}

/// Real-system tests on the owner machine (ADR-013), with a real session bus and runtime. Run with
/// `heavy-build cargo test -p tl-engine launcher::real_tests -- --ignored`. Each test starts only
/// a `sleep` or `tasklens-test.service`, and on drop stops its own scope or unit and removes its
/// own file, also when it fails. Without a session bus each test returns early.
#[cfg(test)]
mod real_tests {
    use super::*;

    use std::fs;
    use std::io::Write;
    use std::process::{Command as Proc, Stdio};
    use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
    use std::time::Instant;

    use asupersync::runtime::Runtime;
    use tl_model::table::UnitRef;

    /// The bead's bound for the program to show up in its scope.
    const SCOPE_WAIT: Duration = Duration::from_secs(2);
    /// The bound for `launch` to answer: the 30 s job wait plus the calls around it.
    const LAUNCH_WAIT: Duration = Duration::from_secs(35);
    const TEST_UNIT: &str = "tasklens-test.service";
    const ENTRY_ID: &str = "tasklens-test-entry";
    /// `systemd-escape tasklens-test-entry` prints `tasklens\x2dtest\x2dentry`.
    const ENTRY_SCOPE_PREFIX: &str = "app-tasklens-tasklens\\x2dtest\\x2dentry-";

    /// The tests share this process's children and the user manager, so they run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
        let rt: Runtime = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(f);
        rt.block_on(join).expect("task completes")
    }

    fn have_session() -> bool {
        let ok = run(async { zbus::Connection::session().await.is_ok() });
        if !ok {
            eprintln!("no session bus: skipped");
        }
        ok
    }

    /// Runs `spec` through `launch` on its own thread and runtime with a new session connection.
    /// The outcome is sent as soon as `launch` returns, so the test does not wait for the
    /// background reaper of the launched program.
    fn launch_in_background(
        spec: LaunchSpec,
    ) -> mpsc::Receiver<Result<LaunchOutcome, ActionError>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            run(async move {
                let cx = Cx::current().expect("task has a Cx");
                let result = match zbus::Connection::session().await {
                    Ok(conn) => {
                        let desktop = DesktopSession::new(crate::desktop::DesktopSource::Process);
                        launch(&cx, Some(Arc::new(conn)), &desktop, &spec).await
                    }
                    Err(_) => Err(internal("no session bus")),
                };
                let _ = tx.send(result);
            });
        });
        rx
    }

    fn systemctl(args: &[&str]) -> String {
        let out = Proc::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("systemctl runs");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// `ActiveState` of a user unit; `inactive` for a unit that does not exist.
    fn active_state(unit: &str) -> String {
        systemctl(&["show", "-P", "ActiveState", unit])
    }

    fn parent_of(pid: u32) -> Option<u32> {
        let stat = fs::read(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.iter().rposition(|&b| b == b')')?;
        let rest = std::str::from_utf8(stat.get(close + 2..)?).ok()?;
        rest.split(' ').nth(1)?.parse().ok()
    }

    /// The direct children of this test process.
    fn children() -> Vec<u32> {
        let me = std::process::id();
        let Ok(dir) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter(|&pid| parent_of(pid) == Some(me))
            .collect()
    }

    /// The last component of the pid's cgroup v2 path.
    fn cgroup_leaf(pid: u32) -> Option<String> {
        let text = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        let path = text.lines().find_map(|l| l.strip_prefix("0::"))?;
        Some(path.rsplit('/').next()?.to_owned())
    }

    fn cmdline(pid: u32) -> Vec<String> {
        let bytes = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        bytes
            .split(|&b| b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect()
    }

    /// A program a test launched. On drop its scope is stopped and the child killed.
    struct Launched {
        before: Vec<u32>,
        pids: Vec<u32>,
        scope: Option<String>,
    }

    impl Launched {
        /// Call right before the launch, so only children started after it count.
        fn expect() -> Self {
            Launched {
                before: children(),
                pids: Vec::new(),
                scope: None,
            }
        }

        /// Waits up to 2 s for a new child of this process inside a scope named
        /// `<prefix><8 hex>.scope`, and returns its pid.
        fn wait_in_scope(&mut self, prefix: &str) -> u32 {
            let begin = Instant::now();
            loop {
                for pid in children() {
                    if self.before.contains(&pid) {
                        continue;
                    }
                    if !self.pids.contains(&pid) {
                        self.pids.push(pid);
                    }
                    if let Some(leaf) = cgroup_leaf(pid)
                        && leaf.starts_with(prefix)
                        && leaf.ends_with(".scope")
                    {
                        self.scope = Some(leaf);
                        return pid;
                    }
                }
                let seen: Vec<_> = self.pids.iter().map(|&p| cgroup_leaf(p)).collect();
                assert!(
                    begin.elapsed() < SCOPE_WAIT,
                    "no child in a {prefix}*.scope after {SCOPE_WAIT:?}: {seen:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn cleanup(&mut self) {
            if let Some(scope) = self.scope.take() {
                systemctl(&["stop", &scope]);
            }
            let me = std::process::id();
            for pid in self.pids.drain(..) {
                // Only while it is still this process's child, so a reused pid is never hit.
                if parent_of(pid) == Some(me)
                    && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
                {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
        }
    }

    impl Drop for Launched {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    /// A file this test created with `create_new`, so it never replaces one of the owner's. On drop
    /// it is removed, and so is its directory when the test made that too.
    struct OwnFile {
        path: PathBuf,
        made_dir: Option<PathBuf>,
    }

    impl OwnFile {
        fn create(path: PathBuf, text: &str) -> Self {
            let dir = path.parent().expect("has a parent").to_owned();
            let made_dir = (!dir.exists()).then(|| dir.clone());
            fs::create_dir_all(&dir).expect("create the directory");
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("{} not created: {e}", path.display()));
            let own = OwnFile { path, made_dir };
            file.write_all(text.as_bytes()).expect("write");
            own
        }
    }

    impl Drop for OwnFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            if let Some(dir) = &self.made_dir {
                let _ = fs::remove_dir(dir);
            }
        }
    }

    /// `tasklens-test.service` from E14-007 in `$XDG_CONFIG_HOME/systemd/user`. On drop the unit
    /// is stopped, the file removed and the user manager reloaded.
    struct TestUnit {
        file: Option<OwnFile>,
    }

    impl TestUnit {
        fn install() -> Self {
            let load = systemctl(&["show", "-P", "LoadState", TEST_UNIT]);
            assert_eq!(load, "not-found", "{TEST_UNIT} already exists; not touched");
            let path = StartupEnv::from_env()
                .config_home
                .join("systemd/user")
                .join(TEST_UNIT);
            let file = OwnFile::create(
                path,
                "[Unit]\nDescription=tasklens integration test unit\n\
                 [Service]\nExecStart=/usr/bin/sleep 600\n",
            );
            let unit = TestUnit { file: Some(file) };
            systemctl(&["daemon-reload"]);
            unit
        }
    }

    impl Drop for TestUnit {
        fn drop(&mut self) {
            systemctl(&["stop", TEST_UNIT]);
            drop(self.file.take());
            systemctl(&["daemon-reload"]);
        }
    }

    #[test]
    #[ignore = "starts a transient user scope (owner machine)"]
    fn real_command_lands_in_transient_scope() {
        let _serial = serial();
        if !have_session() {
            return;
        }
        let mut launched = Launched::expect();
        let outcome = launch_in_background(LaunchSpec {
            what: LaunchWhat::Command("sleep 30".into()),
            as_admin: false,
            cwd: None,
        });
        let pid = launched.wait_in_scope("app-tasklens-run-");
        let scope = launched.scope.clone().expect("scope found");
        eprintln!("pid {pid} in {scope}");
        assert_eq!(
            outcome.recv_timeout(LAUNCH_WAIT),
            Ok(Ok(LaunchOutcome::Launched))
        );
        assert_eq!(active_state(&scope), "active");

        launched.cleanup();
        assert_eq!(active_state(&scope), "inactive", "{scope} is gone");
    }

    #[test]
    #[ignore = "writes an autostart entry and starts a transient user scope (owner machine)"]
    fn real_desktop_entry_run_now() {
        let _serial = serial();
        if !have_session() {
            return;
        }
        let path = StartupEnv::from_env()
            .config_home
            .join("autostart")
            .join(format!("{ENTRY_ID}.desktop"));
        let entry = OwnFile::create(
            path.clone(),
            "[Desktop Entry]\nType=Application\nName=tasklens test entry\nExec=sleep 30\n",
        );
        let mut launched = Launched::expect();
        let outcome = launch_in_background(LaunchSpec {
            what: LaunchWhat::DesktopEntry(ENTRY_ID.into()),
            as_admin: false,
            cwd: None,
        });
        let pid = launched.wait_in_scope(ENTRY_SCOPE_PREFIX);
        let scope = launched.scope.clone().expect("scope found");
        eprintln!("pid {pid} in {scope}");
        assert_eq!(cmdline(pid), ["sleep", "30"]);
        assert_eq!(
            outcome.recv_timeout(LAUNCH_WAIT),
            Ok(Ok(LaunchOutcome::Launched))
        );

        launched.cleanup();
        drop(entry);
        assert_eq!(active_state(&scope), "inactive", "{scope} is gone");
        assert!(!path.exists(), "{} is removed", path.display());
    }

    #[test]
    #[ignore = "installs and starts tasklens-test.service (owner machine)"]
    fn real_unit_start() {
        let _serial = serial();
        if !have_session() {
            return;
        }
        let unit = TestUnit::install();
        let outcome = launch_in_background(LaunchSpec {
            what: LaunchWhat::Unit(UnitRef {
                manager: Manager::User,
                name: TEST_UNIT.into(),
            }),
            as_admin: false,
            cwd: None,
        });
        assert_eq!(
            outcome.recv_timeout(LAUNCH_WAIT),
            Ok(Ok(LaunchOutcome::Launched))
        );
        assert_eq!(active_state(TEST_UNIT), "active");

        drop(unit);
        let load = systemctl(&["show", "-P", "LoadState", TEST_UNIT]);
        assert_eq!(load, "not-found", "{TEST_UNIT} is gone");
    }
}
