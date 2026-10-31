//! Two hosts in one window, end to end (ADR-018, D-038, UI-SHL-008).
//!
//! Two `tasklens-engine` processes, each with its own state, config and runtime directories,
//! act as two hosts on this machine. Host A is reached the way the window reaches its own
//! engine: a `RemoteClient` on the engine socket. Host B is reached the way the window reaches
//! another PC: a `RemoteClient` with an `SshConnector` whose program is a fake `ssh` script
//! that runs `tasklens bridge` with B's directories. `RemoteClient` is the client the window's
//! host manager builds for every host; the pixels on top of it (banner, tooltip, disabled
//! actions) are covered by the tl-ui host tab tests.
//!
//! Covered: a snapshot on each host, an action on each host that lands on its own host, a
//! killed engine that shows the link down and then recovers, a protocol mismatch that shows
//! the version text, and a connection dropped while an action waits, which ends that action
//! with `OutcomeUnknown`.
//!
//! The engine is a binary of tl-server, which `cargo test -p tl-app` does not build. Build it
//! first with `cargo build -p tl-server --bin tasklens-engine`.
//!
//! The session bus address points nowhere and the fake `systemctl` only records its arguments,
//! so nothing here reaches the desktop session or the user's real engine unit. The test kills
//! only the engines, `sleep` processes and bridges it started itself.

#![allow(clippy::unwrap_used)]

use std::fs::{File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_model::actions::{
    ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::Command as EngineCommand;
use tl_model::demand::Demand;
use tl_model::ids::ProcessKey;
use tl_model::remote_proto::{
    HEADER_LEN, HelloTail, HostSettingsChanges, PROTO, SetHostSettingsReply, Want,
};
use tl_remote::client::{ClientConfig, Connection, Connector, LinkStatus, RemoteClient};
use tl_remote::ssh::SshConnector;

/// The `tasklens` binary, which runs `tasklens bridge` on host B.
const TASKLENS: &str = env!("CARGO_BIN_EXE_tasklens");

/// The longest wait for one step. The ssh deadline is 20 s, and the session's `Ack` timeout
/// is 30 s, so no step here can pass because one of those ran out.
const STEP: Duration = Duration::from_secs(20);

/// `tasklens-engine`, next to `tasklens` in the target directory.
fn engine_bin() -> PathBuf {
    let path = Path::new(TASKLENS).with_file_name("tasklens-engine");
    assert!(
        path.is_file(),
        "{} is missing: run heavy-build cargo build -p tl-server --bin tasklens-engine first",
        path.display()
    );
    path
}

/// Polls `f` every 10 ms until it returns `Some`, for at most `STEP`.
fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + STEP;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends `signal` (a name such as `STOP`) to `pid` with `kill`.
fn send_signal(signal: &str, pid: u32) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .unwrap();
    assert!(status.success(), "kill -{signal} {pid}: {status:?}");
}

fn write_script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
}

/// One host: its own XDG roots and, while it runs, its engine. Dropping it kills the engine.
struct Host {
    _tmp: tempfile::TempDir,
    name: &'static str,
    state: PathBuf,
    runtime: PathBuf,
    home: PathBuf,
    /// The fake `ssh` program and the fake `systemctl` the bridge finds on its `PATH`.
    bin: PathBuf,
    engine: Option<Child>,
    /// How many engines this host has started; each writes its own log file.
    starts: u32,
}

impl Host {
    fn new(name: &'static str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let runtime = tmp.path().join("run");
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        for dir in [&state, &runtime, &home, &bin] {
            std::fs::create_dir(dir).unwrap();
        }
        std::fs::set_permissions(&runtime, Permissions::from_mode(0o700)).unwrap();
        Host {
            _tmp: tmp,
            name,
            state,
            runtime,
            home,
            bin,
            engine: None,
            starts: 0,
        }
    }

    /// Starts this host's engine and waits until its socket accepts.
    fn start(&mut self) {
        assert!(self.engine.is_none(), "{}: engine already runs", self.name);
        self.starts += 1;
        let log = self.log(self.starts);
        let mut cmd = Command::new(engine_bin());
        cmd.env_clear()
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/tasklens-test-bus",
            )
            .env("TASKLENS_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // A file, not a pipe: nobody reads the log while the engine runs.
            .stderr(File::create(&log).unwrap());
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        self.engine = Some(cmd.spawn().unwrap());
        let socket = self.socket();
        wait_for(&format!("{}: engine socket", self.name), || {
            if let Some(status) = self.engine.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "{}: tasklens-engine exited with {status:?}:\n{}",
                    self.name,
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            UnixStream::connect(&socket).ok()
        });
    }

    fn log(&self, start: u32) -> PathBuf {
        self.home.join(format!("engine-{start}.log"))
    }

    fn socket(&self) -> PathBuf {
        self.runtime.join("tasklens/engine.sock")
    }

    fn engine_pid(&self) -> u32 {
        self.engine.as_ref().unwrap().id()
    }

    /// SIGKILL to this host's engine, then reaps it so its engine lock is free.
    fn kill_engine(&mut self) {
        let mut child = self.engine.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn engine_toml(&self) -> String {
        std::fs::read_to_string(self.home.join(".config/tasklens/engine.toml")).unwrap()
    }

    /// A client on this host's engine socket, as the window connects to its own engine.
    fn local_client(&self) -> RemoteClient {
        let socket = self.socket();
        let connector = move || UnixStream::connect(&socket).and_then(Connection::unix);
        start_client(connector)
    }

    /// The connector the window uses for a remote host, with a fake `ssh` that ignores its
    /// arguments and runs `tasklens bridge` with this host's directories. The bridge finds only
    /// the fake `systemctl` on its `PATH`, so it never starts a real user unit.
    fn ssh_connector(&self) -> SshConnector {
        let systemctl = self.bin.join("systemctl");
        write_script(
            &systemctl,
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n",
                self.systemctl_calls_path().display()
            ),
        );
        let ssh = self.bin.join("ssh");
        write_script(
            &ssh,
            &format!(
                "#!/bin/sh\nexec /usr/bin/env -i HOME='{home}' XDG_RUNTIME_DIR='{runtime}' \
                 XDG_STATE_HOME='{state}' \
                 DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/tasklens-test-bus \
                 PATH='{bin}' '{tasklens}' bridge\n",
                home = self.home.display(),
                runtime = self.runtime.display(),
                state = self.state.display(),
                bin = self.bin.display(),
                tasklens = TASKLENS,
            ),
        );
        SshConnector::new(self.name, self.name, ssh.into_os_string())
    }

    fn ssh_client(&self) -> RemoteClient {
        start_client(self.ssh_connector())
    }

    fn systemctl_calls_path(&self) -> PathBuf {
        self.home.join("systemctl-calls")
    }

    fn systemctl_calls(&self) -> String {
        std::fs::read_to_string(self.systemctl_calls_path()).unwrap_or_default()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(child) = self.engine.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A `sleep 600` this test started; killed when dropped.
struct Sleeper(Child);

impl Sleeper {
    fn start() -> Self {
        Sleeper(
            Command::new("sleep")
                .arg("600")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A client as the window's host manager starts one, with a short backoff, asking for the
/// process table.
fn start_client(connector: impl Connector) -> RemoteClient {
    let config = ClientConfig {
        hello: HelloTail {
            pid: std::process::id(),
            app_id: Arc::from(tl_ui::window::main::APP_ID),
            bridge: false,
            want: Want { snapshots: true },
        },
        backoff: vec![Duration::from_millis(100), Duration::from_millis(200)],
    };
    let client = RemoteClient::start(connector, config).unwrap();
    client.set_demand(Demand {
        any_surface_visible: true,
        process_table: true,
        ..Demand::default()
    });
    client
}

fn wait_connected(client: &RemoteClient, what: &str) {
    wait_for(&format!("{what}: connected"), || {
        (client.status() == LinkStatus::Connected).then_some(())
    });
}

/// The key of `pid` in the client's newest snapshot, once a snapshot lists it.
fn process_key(client: &RemoteClient, pid: u32, what: &str) -> ProcessKey {
    wait_for(&format!("{what}: pid {pid} in the snapshot"), || {
        client
            .latest()
            .processes
            .records
            .iter()
            .find(|r| r.key.pid == pid)
            .map(|r| r.key)
    })
}

fn term(id: u64, key: ProcessKey) -> EngineCommand {
    EngineCommand::Action(ActionRequest {
        id: ActionId(id),
        kind: ActionKind::Signal(Signal::Term),
        targets: [ActionTarget::Process(key)].into_iter().collect(),
        privileged: false,
    })
}

/// Reads action events into `seen` until the summary (`target: None`, final) of `id` arrives,
/// and returns its state.
fn summary(client: &RemoteClient, id: u64, seen: &mut Vec<ActionEvent>, what: &str) -> ActionState {
    let events = client.action_events();
    wait_for(&format!("{what}: summary of action {id}"), || {
        while let Ok(ev) = events.try_recv() {
            seen.push(ev.clone());
            if ev.id == ActionId(id) && ev.target.is_none() && ev.state.is_final() {
                return Some(ev.state);
            }
        }
        None
    })
}

#[test]
fn snapshots_on_both_hosts() {
    let mut a = Host::new("hosta");
    let mut b = Host::new("hostb");
    a.start();
    b.start();
    let sleep_a = Sleeper::start();
    let sleep_b = Sleeper::start();
    let client_a = a.local_client();
    let client_b = b.ssh_client();
    wait_connected(&client_a, "host A");
    wait_connected(&client_b, "host B");

    for (client, what) in [(&client_a, "host A"), (&client_b, "host B")] {
        for pid in [sleep_a.pid(), sleep_b.pid()] {
            process_key(client, pid, what);
        }
    }

    // Both engines run on this machine, so only the engine instance tells them apart.
    let host_a = client_a.host().unwrap();
    let host_b = client_b.host().unwrap();
    assert_eq!(host_a.host_name, host_b.host_name);
    assert_eq!(host_a.machine_id, host_b.machine_id);
    assert_eq!(host_a.boot_id, host_b.boot_id);
    assert_ne!(host_a.engine_instance, host_b.engine_instance);
    // B's bridge found its engine socket at once, so it never tried to start the unit.
    assert_eq!(b.systemctl_calls(), "");
}

#[test]
fn action_lands_on_its_own_host() {
    let mut a = Host::new("hosta");
    let mut b = Host::new("hostb");
    a.start();
    b.start();
    let mut sleep_a = Sleeper::start();
    let mut sleep_b = Sleeper::start();
    let client_a = a.local_client();
    let client_b = b.ssh_client();
    wait_connected(&client_a, "host A");
    wait_connected(&client_b, "host B");
    let key_a = process_key(&client_a, sleep_a.pid(), "host A");
    let key_b = process_key(&client_b, sleep_b.pid(), "host B");

    client_a.send(term(1, key_a)).unwrap();
    client_b.send(term(2, key_b)).unwrap();
    let mut seen_a = Vec::new();
    let mut seen_b = Vec::new();
    assert_eq!(
        summary(&client_a, 1, &mut seen_a, "host A"),
        ActionState::Succeeded
    );
    assert_eq!(
        summary(&client_b, 2, &mut seen_b, "host B"),
        ActionState::Succeeded
    );
    wait_for("both sleeps to exit", || {
        (!sleep_a.running() && !sleep_b.running()).then_some(())
    });
    // Each client hears only about its own action.
    assert!(seen_a.iter().all(|ev| ev.id == ActionId(1)), "{seen_a:?}");
    assert!(seen_b.iter().all(|ev| ev.id == ActionId(2)), "{seen_b:?}");

    // A host settings change through A changes A's engine.toml and leaves B's as it was.
    let before_b = b.engine_toml();
    let (generation, snap) = client_a.latest_with_generation();
    let base_rev = snap.host_settings.rev;
    assert!(base_rev >= 1, "the engine sends its host settings");
    let interval_ms = if snap.host_settings.values.interval_ms == 2000 {
        3000
    } else {
        2000
    };
    let reply = client_a
        .set_host_settings(
            generation,
            base_rev,
            HostSettingsChanges {
                interval_ms: Some(interval_ms),
                ..HostSettingsChanges::default()
            },
        )
        .unwrap();
    let reply = wait_for("host A: settings reply", || reply.try_recv().ok());
    assert_eq!(reply, SetHostSettingsReply::Ack { rev: base_rev + 1 });
    let saved: toml::Table = a.engine_toml().parse().unwrap();
    assert_eq!(
        saved["processes"]["interval_ms"].as_integer(),
        Some(i64::from(interval_ms))
    );
    assert_eq!(saved["rev"].as_integer(), Some(base_rev as i64 + 1));
    assert_eq!(b.engine_toml(), before_b);
}

#[test]
fn killed_engine_reconnects() {
    let mut a = Host::new("hosta");
    let mut b = Host::new("hostb");
    a.start();
    b.start();
    let client_a = a.local_client();
    let client_b = b.ssh_client();
    wait_connected(&client_a, "host A");
    wait_connected(&client_b, "host B");

    for (host, client, what) in [(&mut a, &client_a, "host A"), (&mut b, &client_b, "host B")] {
        let old = client.host().unwrap().engine_instance;
        let old_generation = client.latest_with_generation().0;
        host.kill_engine();
        // The window shows the host as reconnecting: the link is down and the client's
        // generation is newer than its stored snapshot's.
        wait_for(&format!("{what}: link down"), || {
            (client.status() != LinkStatus::Connected).then_some(())
        });
        assert!(client.generation() > old_generation, "{what}");
        host.start();
        wait_for(&format!("{what}: a new engine instance"), || {
            (client.status() == LinkStatus::Connected
                && client.host().is_some_and(|h| h.engine_instance != old))
            .then_some(())
        });
        // The snapshot is the new session's.
        let (generation, _) = client.latest_with_generation();
        assert!(generation > old_generation, "{what}");
        assert_eq!(generation, client.generation(), "{what}");
    }
    // While B's engine was down, its bridge tried to start the engine unit, and only the fake
    // systemctl saw it.
    assert!(
        b.systemctl_calls()
            .contains("--user start tasklens-engine.service"),
        "{}",
        b.systemctl_calls()
    );
}

/// Sets the bootstrap `proto` of the first frame written (the window's `Hello`) to another
/// value, as a window of another tasklens version would send it.
struct OtherProto {
    inner: Box<dyn Write + Send>,
    done: bool,
}

impl Write for OtherProto {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.done {
            return self.inner.write(buf);
        }
        self.done = true;
        // The client writes the whole `Hello` with one `write_all`; bootstrap frames are never
        // compressed, so the payload starts with the proto.
        assert!(buf.len() >= HEADER_LEN + 2, "short Hello write");
        let mut patched = buf.to_vec();
        patched[HEADER_LEN..HEADER_LEN + 2].copy_from_slice(&(PROTO + 1).to_le_bytes());
        self.inner.write_all(&patched)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[test]
fn proto_mismatch_shows_version() {
    let mut b = Host::new("hostb");
    b.start();
    let ssh = b.ssh_connector();
    let connector = move || {
        let conn = ssh.connect()?;
        Ok(Connection {
            reader: conn.reader,
            writer: Box::new(OtherProto {
                inner: conn.writer,
                done: false,
            }),
            shutdown: conn.shutdown,
        })
    };
    let client = start_client(connector);
    let text = wait_for("host B: the version refusal", || match client.status() {
        LinkStatus::Failed(text) if text.contains("protocol") => Some(text),
        _ => None,
    });
    assert!(text.contains(&format!("(protocol {PROTO})")), "{text}");
    assert!(
        text.contains(&format!("(protocol {})", PROTO + 1)),
        "{text}"
    );
    assert!(client.host().is_none(), "no session opened: {text}");
}

#[test]
fn drop_mid_action_is_outcome_unknown() {
    let mut b = Host::new("hostb");
    b.start();
    let mut sleep_b = Sleeper::start();
    let client = b.ssh_client();
    wait_connected(&client, "host B");
    let key = process_key(&client, sleep_b.pid(), "host B");
    let host_name = client.host().unwrap().host_name;

    // A stopped engine reads nothing, so the action stays open on the client.
    send_signal("STOP", b.engine_pid());
    client.send(term(7, key)).unwrap();
    b.kill_engine();

    let mut seen = Vec::new();
    let state = summary(&client, 7, &mut seen, "host B");
    assert_eq!(
        state,
        ActionState::OutcomeUnknown(Arc::from(format!("connection to {host_name} lost")))
    );
    // The engine never ran the action, and the client does not send it again.
    assert!(sleep_b.running());
}
