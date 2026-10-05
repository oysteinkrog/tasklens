//! Opens URLs, files and file locations for the UI, on the session bus (R-134, PROC-073, PROC-095).
//!
//! `open_uri` uses the desktop portal (`OpenURI` for a URI, `OpenFile` for a path) and falls back
//! to `/usr/bin/xdg-open <target>` when there is no session bus or the portal call fails.
//! `show_in_file_manager` uses `org.freedesktop.FileManager1`. `activate_action` serves
//! `LaunchWhat::DbusActivate` (R-170 item 11). The colour scheme reader feeds
//! `SystemSnapshot.color_scheme` (R-170 item 4); the `SystemdUser` worker runs it (R-244).
//!
//! Every D-Bus call has a 2 s `time::timeout` followed by a checkpoint. `xdg-open` gets the
//! target as one argv element, never through a shell. Never log a path or URI above `debug`.

#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "E11 actions and the E14 systemd worker are the callers"
    )
)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::future::Future;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::time;
use futures_lite::StreamExt;
use tl_model::actions::{ActionError, ActionState};
use zbus::message::Type;
use zbus::zvariant::{Fd, OwnedValue, Value};

use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// The deadline for each D-Bus call (ADR-006).
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// The fallback opener.
pub(crate) const XDG_OPEN: &str = "/usr/bin/xdg-open";

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const OPEN_URI_IFACE: &str = "org.freedesktop.portal.OpenURI";
const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NS: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";
const FILE_MANAGER_DEST: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
const APPLICATION_IFACE: &str = "org.freedesktop.Application";

/// Starts a program detached. `Detached` in production; tests record the argv instead.
pub(crate) trait Spawn: Sync {
    fn spawn(
        &self,
        cx: &Cx,
        argv: Vec<String>,
    ) -> impl Future<Output = Result<(), ActionError>> + Send;
}

/// The tl-ops launch helper (`posix_spawn`, `setsid`, stdio on `/dev/null`). The child is reaped
/// by a background task, so the action does not wait for it to exit.
pub(crate) struct Detached;

impl Spawn for Detached {
    async fn spawn(&self, cx: &Cx, argv: Vec<String>) -> Result<(), ActionError> {
        let mut job = cx
            .spawn_blocking(move |_| tl_ops::launch::spawn_detached(&argv, None))
            .map_err(|_| internal("spawn job not admitted"))?;
        let child = match job.join(cx).await {
            Ok(Ok(child)) => child,
            Ok(Err(e)) => {
                return Err(match e.to_action_state() {
                    ActionState::Failed(err) => err,
                    _ => internal(&e.to_string()),
                });
            }
            Err(_) => return Err(internal("spawn job failed")),
        };
        let reaper = cx.spawn(move |cx| async move {
            if let Ok(mut wait) = cx.spawn_blocking(move |_| tl_ops::launch::wait_exit(&child)) {
                let _ = wait.join(&cx).await;
            }
        });
        if reaper.is_err() {
            tracing::warn!("opener: reaper task not started");
        }
        Ok(())
    }
}

/// Opens `target`: a URI with a scheme, or an absolute path.
pub(crate) async fn open_uri(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    target: &str,
) -> Result<(), ActionError> {
    open_uri_with(cx, session.as_deref(), target, &Detached).await
}

/// Shows `path` in the file manager, or its properties dialog when `properties` is true.
pub(crate) async fn show_in_file_manager(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    path: &str,
    properties: bool,
) -> Result<(), ActionError> {
    show_in_file_manager_with(cx, session.as_deref(), path, properties, &Detached).await
}

/// `org.freedesktop.Application.ActivateAction(action, [parameter], {})` on `bus_name` at
/// `object_path`. Nothing is spawned on failure.
pub(crate) async fn activate_action(
    cx: &Cx,
    session: Option<Arc<zbus::Connection>>,
    bus_name: &str,
    object_path: &str,
    action: &str,
    parameter: Option<&str>,
) -> Result<(), ActionError> {
    let Some(conn) = session else {
        return Err(internal("no session bus"));
    };
    let params: Vec<Value<'_>> = parameter.into_iter().map(Value::from).collect();
    let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    let body = (action, params, platform_data);
    let call = conn.call_method(
        Some(bus_name),
        object_path,
        Some(APPLICATION_IFACE),
        "ActivateAction",
        &body,
    );
    timed(cx, call)
        .await
        .map(drop)
        .map_err(ActionError::Internal)
}

/// The portal `color-scheme` setting: 0 no preference, 1 prefer dark, 2 prefer light. `None` for
/// any other value, a missing key or an error.
pub(crate) async fn read_color_scheme(cx: &Cx, session: &zbus::Connection) -> Option<u8> {
    let call = session.call_method(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        Some(SETTINGS_IFACE),
        "Read",
        &(APPEARANCE_NS, COLOR_SCHEME_KEY),
    );
    let reply = match timed(cx, call).await {
        Ok(reply) => reply,
        Err(reason) => {
            tracing::debug!(%reason, "opener: color-scheme read failed");
            return None;
        }
    };
    let value: OwnedValue = reply.body().deserialize().ok()?;
    color_scheme_of(&value)
}

/// Sends `SourceUpdate::ColorScheme` with the current value, then once per `SettingChanged`
/// signal for `color-scheme` with a value from 0 to 2. Runs until `cx` is cancelled.
pub(crate) async fn watch_color_scheme(
    cx: &Cx,
    session: &zbus::Connection,
    tx: &mpsc::Sender<EngineMsg>,
) -> Result<(), WorkerError> {
    let rule = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(PORTAL_DEST)
        .and_then(|b| b.path(PORTAL_PATH))
        .and_then(|b| b.interface(SETTINGS_IFACE))
        .and_then(|b| b.member("SettingChanged"))
        .and_then(|b| b.arg(0, APPEARANCE_NS))
        .map_err(|e| WorkerError::Dbus(e.to_string().into()))?
        .build();
    // Subscribe before the first read so a change between the two is not lost.
    let subscribe = zbus::MessageStream::for_match_rule(rule, session, None);
    let mut stream = timed(cx, subscribe).await.map_err(WorkerError::Dbus)?;

    if let Some(v) = read_color_scheme(cx, session).await {
        send_scheme(cx, tx, v).await?;
    }
    loop {
        let next = time::timeout(cx.now(), CALL_TIMEOUT, stream.next()).await;
        if cx.checkpoint().is_err() {
            return Ok(());
        }
        let msg = match next {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                tracing::debug!(error = %e, "opener: bad SettingChanged message");
                continue;
            }
            Ok(None) => return Err(WorkerError::Dbus("SettingChanged stream ended".into())),
            Err(_) => continue,
        };
        let Ok((namespace, key, value)) = msg.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if namespace != APPEARANCE_NS || key != COLOR_SCHEME_KEY {
            continue;
        }
        if let Some(v) = color_scheme_of(&value) {
            send_scheme(cx, tx, v).await?;
        }
    }
}

/// `file://` plus `path` with every byte outside `A-Z a-z 0-9 - . _ ~ /` written as `%XX`.
pub(crate) fn file_uri(path: &str) -> String {
    let mut uri = String::with_capacity(7 + path.len());
    uri.push_str("file://");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

async fn open_uri_with(
    cx: &Cx,
    session: Option<&zbus::Connection>,
    target: &str,
    spawner: &impl Spawn,
) -> Result<(), ActionError> {
    let is_uri = has_scheme(target);
    if !is_uri && !target.starts_with('/') {
        return Err(internal("not a URI or an absolute path"));
    }
    if let Some(conn) = session {
        let sent = if is_uri && !target.starts_with("file:") {
            portal_open_uri(cx, conn, target).await
        } else {
            portal_open_file(cx, conn, target).await
        };
        match sent {
            Ok(()) => return Ok(()),
            Err(reason) => tracing::debug!(%reason, "opener: portal failed, using xdg-open"),
        }
    }
    spawner
        .spawn(cx, vec![XDG_OPEN.to_owned(), target.to_owned()])
        .await
}

async fn show_in_file_manager_with(
    cx: &Cx,
    session: Option<&zbus::Connection>,
    path: &str,
    properties: bool,
    spawner: &impl Spawn,
) -> Result<(), ActionError> {
    if !path.starts_with('/') {
        return Err(internal("not an absolute path"));
    }
    let owned = match session {
        Some(conn) => has_file_manager(cx, conn).await,
        None => false,
    };
    if let (true, Some(conn)) = (owned, session) {
        let method = if properties {
            "ShowItemProperties"
        } else {
            "ShowItems"
        };
        let body = (vec![file_uri(path)], "");
        let call = conn.call_method(
            Some(FILE_MANAGER_DEST),
            FILE_MANAGER_PATH,
            Some(FILE_MANAGER_DEST),
            method,
            &body,
        );
        return timed(cx, call)
            .await
            .map(drop)
            .map_err(ActionError::Internal);
    }
    if properties {
        // R-148 item 3: the UI hides File properties without a FileManager1 owner.
        return Err(ActionError::Unsupported);
    }
    let parent = Path::new(path).parent().unwrap_or(Path::new("/"));
    let parent = parent.to_str().unwrap_or("/");
    open_uri_with(cx, session, parent, spawner).await
}

async fn has_file_manager(cx: &Cx, conn: &zbus::Connection) -> bool {
    let call = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(FILE_MANAGER_DEST,),
    );
    match timed(cx, call).await {
        Ok(reply) => reply.body().deserialize::<bool>().unwrap_or(false),
        Err(reason) => {
            tracing::debug!(%reason, "opener: NameHasOwner failed");
            false
        }
    }
}

async fn portal_open_uri(cx: &Cx, conn: &zbus::Connection, uri: &str) -> Result<(), Arc<str>> {
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let body = ("", uri, options);
    let call = conn.call_method(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        Some(OPEN_URI_IFACE),
        "OpenURI",
        &body,
    );
    timed(cx, call).await.map(drop)
}

/// Opens the file `O_RDONLY | O_CLOEXEC` on the blocking pool and passes the fd to `OpenFile`.
async fn portal_open_file(cx: &Cx, conn: &zbus::Connection, target: &str) -> Result<(), Arc<str>> {
    let path = if target.starts_with('/') {
        PathBuf::from(target)
    } else {
        file_uri_path(target).ok_or("not a local file URI")?
    };
    // std opens with O_CLOEXEC.
    let mut job = cx
        .spawn_blocking(move |_| std::fs::File::open(path))
        .map_err(|_| "open job not admitted")?;
    let file = match job.join(cx).await {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => return Err(format!("open: {:?}", e.kind()).into()),
        Err(_) => return Err("open job failed".into()),
    };
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let body = ("", Fd::from(&file), options);
    let call = conn.call_method(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        Some(OPEN_URI_IFACE),
        "OpenFile",
        &body,
    );
    timed(cx, call).await.map(drop)
}

/// `time::timeout` with `CALL_TIMEOUT`, then a checkpoint. The error is a reason without the
/// D-Bus error message, which can hold a path.
async fn timed<T>(cx: &Cx, fut: impl Future<Output = zbus::Result<T>>) -> Result<T, Arc<str>> {
    let result = time::timeout(cx.now(), CALL_TIMEOUT, fut).await;
    if cx.checkpoint().is_err() {
        return Err("cancelled".into());
    }
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(crate::dbus::error_name(&e).unwrap_or("zbus error").into()),
        Err(_) => Err("timeout".into()),
    }
}

async fn send_scheme(cx: &Cx, tx: &mpsc::Sender<EngineMsg>, v: u8) -> Result<(), WorkerError> {
    tx.send(cx, EngineMsg::Source(SourceUpdate::ColorScheme(v)))
        .await
        .map_err(|_| WorkerError::Closed)
}

/// Unwraps nested variants down to a `u32` from 0 to 2.
fn color_scheme_of(value: &Value<'_>) -> Option<u8> {
    match value {
        Value::Value(inner) => color_scheme_of(inner),
        Value::U32(v @ 0..=2) => u8::try_from(*v).ok(),
        _ => None,
    }
}

/// `^[A-Za-z][A-Za-z0-9+.-]*:`
fn has_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
}

/// The local path of `file:///p` or `file://localhost/p`, percent-decoded.
fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut out = Vec::with_capacity(rest.len());
    let mut bytes = rest.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            out.push(u8::try_from(hi * 16 + lo).ok()?);
        } else {
            out.push(b);
        }
    }
    if out.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(&out)))
}

fn internal(reason: &str) -> ActionError {
    ActionError::Internal(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use asupersync::runtime::Runtime;
    use zbus::message::Header;
    use zbus::zvariant::OwnedObjectPath;

    type Log = Arc<Mutex<Vec<String>>>;

    /// Records each argv instead of starting a program.
    #[derive(Default)]
    struct FakeSpawn(Mutex<Vec<Vec<String>>>);

    impl Spawn for FakeSpawn {
        async fn spawn(&self, _cx: &Cx, argv: Vec<String>) -> Result<(), ActionError> {
            self.0.lock().expect("spawn log").push(argv);
            Ok(())
        }
    }

    impl FakeSpawn {
        fn calls(&self) -> Vec<Vec<String>> {
            self.0.lock().expect("spawn log").clone()
        }
    }

    fn record(log: &Log, entry: String) {
        log.lock().expect("call log").push(entry);
    }

    /// The fd's target, read from `/proc/self/fd`.
    fn fd_target(fd: &zbus::zvariant::OwnedFd) -> String {
        use std::os::fd::AsRawFd;
        let link = format!("/proc/self/fd/{}", fd.as_raw_fd());
        std::fs::read_link(link)
            .expect("fd link")
            .to_string_lossy()
            .into_owned()
    }

    struct FakePortal {
        log: Log,
        open_fails: bool,
    }

    #[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
    impl FakePortal {
        #[zbus(name = "OpenURI")]
        fn open_uri(
            &self,
            parent: &str,
            uri: &str,
            options: HashMap<String, OwnedValue>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            record(
                &self.log,
                format!("OpenURI {parent:?} {uri} {}", options.len()),
            );
            if self.open_fails {
                return Err(zbus::fdo::Error::Failed("no handler".into()));
            }
            Ok(OwnedObjectPath::try_from("/request/1").expect("path"))
        }

        fn open_file(
            &self,
            parent: &str,
            fd: zbus::zvariant::OwnedFd,
            options: HashMap<String, OwnedValue>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            record(
                &self.log,
                format!("OpenFile {parent:?} {} {}", fd_target(&fd), options.len()),
            );
            if self.open_fails {
                return Err(zbus::fdo::Error::Failed("no handler".into()));
            }
            Ok(OwnedObjectPath::try_from("/request/1").expect("path"))
        }
    }

    struct FakeSettings {
        scheme: OwnedValue,
    }

    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl FakeSettings {
        fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
            if namespace == APPEARANCE_NS && key == COLOR_SCHEME_KEY {
                self.scheme
                    .try_clone()
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
            } else {
                Err(zbus::fdo::Error::Failed("not found".into()))
            }
        }
    }

    struct FakeBusDaemon {
        log: Log,
        file_manager: bool,
    }

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl FakeBusDaemon {
        fn name_has_owner(&self, name: &str) -> bool {
            record(&self.log, format!("NameHasOwner {name}"));
            self.file_manager && name == FILE_MANAGER_DEST
        }
    }

    struct FakeFileManager {
        log: Log,
    }

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FakeFileManager {
        fn show_items(&self, uris: Vec<String>, startup_id: &str) {
            record(&self.log, format!("ShowItems {uris:?} {startup_id:?}"));
        }

        fn show_item_properties(&self, uris: Vec<String>, startup_id: &str) {
            record(
                &self.log,
                format!("ShowItemProperties {uris:?} {startup_id:?}"),
            );
        }
    }

    struct FakeApplication {
        log: Log,
    }

    #[zbus::interface(name = "org.freedesktop.Application")]
    impl FakeApplication {
        fn activate_action(
            &self,
            #[zbus(header)] hdr: Header<'_>,
            action: &str,
            parameter: Vec<OwnedValue>,
            platform_data: HashMap<String, OwnedValue>,
        ) {
            let params: Vec<String> = parameter
                .iter()
                .map(|v| match &**v {
                    Value::Str(s) => format!("s:{s}"),
                    other => format!("{other:?}"),
                })
                .collect();
            record(
                &self.log,
                format!(
                    "ActivateAction {} {} {action} {params:?} {}",
                    hdr.destination().map(|d| d.to_string()).unwrap_or_default(),
                    hdr.path().map(|p| p.to_string()).unwrap_or_default(),
                    platform_data.len(),
                ),
            );
        }
    }

    struct Fake {
        log: Log,
        server: zbus::Connection,
        client: Arc<zbus::Connection>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.log.lock().expect("call log").clone()
        }
    }

    /// A peer-to-peer pair over a socketpair. The server side plays the portal, the bus daemon,
    /// FileManager1 (when `file_manager`) and a GApplication at `/org/gnome/Settings`.
    fn fake_bus(open_fails: bool, file_manager: bool, scheme: Value<'static>) -> Fake {
        let log = Log::default();
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let mut server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(
                PORTAL_PATH,
                FakePortal {
                    log: Arc::clone(&log),
                    open_fails,
                },
            )
            .expect("portal")
            .serve_at(
                PORTAL_PATH,
                FakeSettings {
                    scheme: OwnedValue::try_from(scheme).expect("owned value"),
                },
            )
            .expect("settings")
            .serve_at(
                "/org/freedesktop/DBus",
                FakeBusDaemon {
                    log: Arc::clone(&log),
                    file_manager,
                },
            )
            .expect("bus daemon")
            .serve_at(
                "/org/gnome/Settings",
                FakeApplication {
                    log: Arc::clone(&log),
                },
            )
            .expect("application");
        if file_manager {
            server = server
                .serve_at(
                    FILE_MANAGER_PATH,
                    FakeFileManager {
                        log: Arc::clone(&log),
                    },
                )
                .expect("file manager");
        }
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server.build(), client));
        Fake {
            log,
            server: server.expect("server connects"),
            client: Arc::new(client.expect("client connects")),
        }
    }

    fn run<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
        let rt: Runtime = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(f);
        rt.block_on(join).expect("task completes")
    }

    fn cx() -> Cx {
        Cx::current().expect("task has a Cx")
    }

    #[test]
    fn file_uri_encoding() {
        assert_eq!(
            file_uri("/home/u/My File#1.txt"),
            "file:///home/u/My%20File%231.txt"
        );
        assert_eq!(file_uri("/a-b.c_d~e/F9"), "file:///a-b.c_d~e/F9");
    }

    #[test]
    fn file_uri_utf8() {
        assert_eq!(file_uri("/caf\u{e9}"), "file:///caf%C3%A9");
        assert_eq!(
            file_uri_path("file:///caf%C3%A9/a%20b").expect("decodes"),
            PathBuf::from("/caf\u{e9}/a b")
        );
    }

    #[test]
    fn target_kinds() {
        assert!(has_scheme("https://example.org"));
        assert!(has_scheme("file:///x"));
        assert!(has_scheme("a+b.c-d:x"));
        assert!(!has_scheme("/usr/bin"));
        assert!(!has_scheme("1x:y"));
        assert!(!has_scheme("-x:y"));
        assert!(!has_scheme("relative/path"));
        assert_eq!(file_uri_path("file://host/x"), None);
        assert_eq!(file_uri_path("file:///x%00"), None);
        assert_eq!(
            file_uri_path("file://localhost/x"),
            Some(PathBuf::from("/x"))
        );
    }

    #[test]
    fn open_uri_portal_error_falls_back() {
        let fake = fake_bus(true, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            open_uri_with(&cx(), Some(&client), "https://example.org", &*spawned).await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(fake.calls(), ["OpenURI \"\" https://example.org 0"]);
        assert_eq!(
            spawner.calls(),
            [vec![XDG_OPEN.to_owned(), "https://example.org".to_owned()]]
        );
    }

    #[test]
    fn open_uri_portal_success_spawns_nothing() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            open_uri_with(&cx(), Some(&client), "https://example.org", &*spawned).await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(fake.calls(), ["OpenURI \"\" https://example.org 0"]);
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn open_uri_without_session_spawns_xdg_open() {
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move { open_uri_with(&cx(), None, "https://x", &*spawned).await });
        assert_eq!(result, Ok(()));
        assert_eq!(
            spawner.calls(),
            [vec![XDG_OPEN.to_owned(), "https://x".to_owned()]]
        );
    }

    #[test]
    fn open_uri_rejects_relative_target() {
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move { open_uri_with(&cx(), None, "-x relative", &*spawned).await });
        assert_eq!(result, Err(internal("not a URI or an absolute path")));
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn open_file_uri_passes_fd() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result =
            run(async move { open_uri_with(&cx(), Some(&client), "file:///usr", &*spawned).await });
        assert_eq!(result, Ok(()));
        assert_eq!(fake.calls(), ["OpenFile \"\" /usr 0"]);
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn show_rejects_relative_path() {
        let fake = fake_bus(false, true, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            show_in_file_manager_with(&cx(), Some(&client), "relative/path", false, &*spawned).await
        });
        assert_eq!(result, Err(internal("not an absolute path")));
        assert!(fake.calls().is_empty(), "D-Bus calls {:?}", fake.calls());
        assert!(spawner.calls().is_empty());

        // The production entry point checks before it can spawn anything.
        let result = run(async move { show_in_file_manager(&cx(), None, "a/b", false).await });
        assert_eq!(result, Err(internal("not an absolute path")));
    }

    #[test]
    fn show_with_filemanager_calls_show_items() {
        let fake = fake_bus(false, true, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            let cx = cx();
            let items =
                show_in_file_manager_with(&cx, Some(&client), "/usr/bin/a b", false, &*spawned)
                    .await;
            let props =
                show_in_file_manager_with(&cx, Some(&client), "/usr/bin/foo", true, &*spawned)
                    .await;
            (items, props)
        });
        assert_eq!(result, (Ok(()), Ok(())));
        assert_eq!(
            fake.calls(),
            [
                "NameHasOwner org.freedesktop.FileManager1",
                "ShowItems [\"file:///usr/bin/a%20b\"] \"\"",
                "NameHasOwner org.freedesktop.FileManager1",
                "ShowItemProperties [\"file:///usr/bin/foo\"] \"\"",
            ]
        );
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn show_without_filemanager_opens_parent() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            show_in_file_manager_with(&cx(), Some(&client), "/usr/bin/foo", false, &*spawned).await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(
            fake.calls(),
            [
                "NameHasOwner org.freedesktop.FileManager1",
                "OpenFile \"\" /usr/bin 0",
            ]
        );
        assert!(spawner.calls().is_empty());

        // Without a session bus the parent goes to xdg-open.
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            show_in_file_manager_with(&cx(), None, "/usr/bin/foo", false, &*spawned).await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(
            spawner.calls(),
            [vec![XDG_OPEN.to_owned(), "/usr/bin".to_owned()]]
        );
    }

    #[test]
    fn show_properties_without_filemanager_unsupported() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let spawner = Arc::new(FakeSpawn::default());
        let spawned = Arc::clone(&spawner);
        let result = run(async move {
            show_in_file_manager_with(&cx(), Some(&client), "/usr/bin/foo", true, &*spawned).await
        });
        assert_eq!(result, Err(ActionError::Unsupported));
        // Only the owner check; no portal or FileManager1 call.
        assert_eq!(fake.calls(), ["NameHasOwner org.freedesktop.FileManager1"]);
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn activate_action_args() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let result = run(async move {
            activate_action(
                &cx(),
                Some(client),
                "org.gnome.Settings",
                "/org/gnome/Settings",
                "launch-panel",
                Some("network"),
            )
            .await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(
            fake.calls(),
            [
                "ActivateAction org.gnome.Settings /org/gnome/Settings launch-panel [\"s:network\"] 0"
            ]
        );
    }

    #[test]
    fn activate_action_no_parameter_empty_av() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let result = run(async move {
            activate_action(
                &cx(),
                Some(client),
                "org.gnome.Settings",
                "/org/gnome/Settings",
                "launch-panel",
                None,
            )
            .await
        });
        assert_eq!(result, Ok(()));
        assert_eq!(
            fake.calls(),
            ["ActivateAction org.gnome.Settings /org/gnome/Settings launch-panel [] 0"]
        );
    }

    #[test]
    fn activate_action_failures_are_internal() {
        let fake = fake_bus(false, false, Value::U32(0));
        let client = Arc::clone(&fake.client);
        let (no_session, unknown_path) = run(async move {
            let cx = cx();
            let a = activate_action(&cx, None, "org.x", "/org/x", "a", None).await;
            let b = activate_action(&cx, Some(client), "org.x", "/no/such", "a", None).await;
            (a, b)
        });
        assert_eq!(no_session, Err(internal("no session bus")));
        assert!(
            matches!(unknown_path, Err(ActionError::Internal(_))),
            "{unknown_path:?}"
        );
    }

    #[test]
    fn color_scheme_read_nested_variant() {
        let nested = Value::Value(Box::new(Value::U32(1)));
        let fake = fake_bus(false, false, nested);
        let client = Arc::clone(&fake.client);
        let v = run(async move { read_color_scheme(&cx(), &client).await });
        assert_eq!(v, Some(1));
    }

    #[test]
    fn color_scheme_read_out_of_range_none() {
        let fake = fake_bus(false, false, Value::U32(7));
        let client = Arc::clone(&fake.client);
        let v = run(async move { read_color_scheme(&cx(), &client).await });
        assert_eq!(v, None);
        let fake = fake_bus(false, false, Value::from("dark"));
        let client = Arc::clone(&fake.client);
        let v = run(async move { read_color_scheme(&cx(), &client).await });
        assert_eq!(v, None);
    }

    /// Runs `watch_color_scheme` against a portal whose `Read` gives 1. After the first update it
    /// emits `signals`, then a `color-scheme` 0 signal as the end marker, and returns every scheme
    /// received up to that marker.
    fn watch_with(signals: Vec<(&'static str, &'static str, Value<'static>)>) -> Vec<u8> {
        let fake = fake_bus(false, false, Value::U32(1));
        let client = Arc::clone(&fake.client);
        let server = fake.server.clone();
        let signals: Vec<(&str, &str, OwnedValue)> = signals
            .into_iter()
            .map(|(ns, key, v)| (ns, key, OwnedValue::try_from(v).expect("owned")))
            .collect();
        run(async move {
            let cx = cx();
            let (tx, mut rx) = mpsc::channel::<EngineMsg>(16);
            let watcher = cx
                .spawn(move |cx| async move { watch_color_scheme(&cx, &client, &tx).await })
                .expect("watcher spawns");
            let mut got = Vec::new();
            let mut emitted = false;
            loop {
                let msg = time::timeout(cx.now(), Duration::from_secs(5), rx.recv(&cx))
                    .await
                    .expect("update within 5 s")
                    .expect("channel open");
                let EngineMsg::Source(SourceUpdate::ColorScheme(v)) = msg else {
                    panic!("unexpected message");
                };
                got.push(v);
                if !emitted {
                    emitted = true;
                    for (ns, key, value) in &signals {
                        server
                            .emit_signal(
                                None::<&str>,
                                PORTAL_PATH,
                                SETTINGS_IFACE,
                                "SettingChanged",
                                &(*ns, *key, value),
                            )
                            .await
                            .expect("signal sent");
                    }
                    // The sentinel: a valid change after every test signal.
                    server
                        .emit_signal(
                            None::<&str>,
                            PORTAL_PATH,
                            SETTINGS_IFACE,
                            "SettingChanged",
                            &(APPEARANCE_NS, COLOR_SCHEME_KEY, Value::U32(0)),
                        )
                        .await
                        .expect("sentinel sent");
                } else if v == 0 {
                    break;
                }
            }
            watcher.abort();
            got
        })
    }

    #[test]
    fn color_scheme_signal_sends_update() {
        let got = watch_with(vec![(APPEARANCE_NS, COLOR_SCHEME_KEY, Value::U32(2))]);
        assert_eq!(got, [1, 2, 0]);
    }

    #[test]
    fn color_scheme_other_key_ignored() {
        let got = watch_with(vec![
            (APPEARANCE_NS, "accent-color", Value::U32(2)),
            ("org.other", COLOR_SCHEME_KEY, Value::U32(2)),
            (APPEARANCE_NS, COLOR_SCHEME_KEY, Value::U32(7)),
        ]);
        assert_eq!(got, [1, 0]);
    }

    #[test]
    fn detached_spawn_reaps_child() {
        let result = run(async move {
            let cx = cx();
            let spawned = Detached.spawn(&cx, vec!["/bin/true".to_owned()]).await;
            let missing = Detached
                .spawn(&cx, vec!["/nonexistent/tasklens-y6k24".to_owned()])
                .await;
            (spawned, missing)
        });
        assert_eq!(result.0, Ok(()));
        assert_eq!(
            result.1,
            Err(ActionError::Os(rustix::io::Errno::NOENT.raw_os_error()))
        );
    }

    /// Opens `https://example.org` in the owner's browser through the real portal.
    #[test]
    #[ignore = "opens a browser window (owner machine)"]
    fn real_open_uri_portal() {
        let result = run(async move {
            let cx = cx();
            let conn = zbus::Connection::session().await.expect("session bus");
            open_uri(&cx, Some(Arc::new(conn)), "https://example.org").await
        });
        assert_eq!(result, Ok(()));
    }
}
