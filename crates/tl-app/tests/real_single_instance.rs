//! Single instance on a real bus (E33-009, ADR-010 Verification).
//!
//! Each test starts its own `dbus-daemon`, so it never touches a running tasklens or anything
//! else on the user's session bus. The ignored test needs `dbus-daemon`; the forwarding test
//! needs `dbus-daemon` and a display and is skipped without them.

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/single_instance.rs"]
mod single_instance;

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use single_instance::{Forward, Outcome, acquire_on, object_path};
use tl_model::command::{AppRequest, Tab};
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::Value;

const TEST_NAME: &str = "io.github.oysteinkrog.TasklensTest";
const INTERFACE: &str = "org.freedesktop.Application";

/// A `dbus-daemon` this test started. Dropping it ends the daemon.
struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> PrivateBus {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start dbus-daemon");
        let stdout = child.stdout.take().expect("daemon stdout");
        let mut address = String::new();
        BufReader::new(stdout)
            .read_line(&mut address)
            .expect("read daemon address");
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon printed no address");
        PrivateBus { child, address }
    }

    fn connect(&self) -> Connection {
        Builder::address(self.address.as_str())
            .expect("address")
            .method_timeout(Duration::from_secs(2))
            .build()
            .expect("connect to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn no_forward() -> Forward {
    Forward {
        tab: None,
        select_pid: None,
    }
}

/// The D-Bus error name of a failed call.
fn error_name(result: zbus::Result<zbus::message::Message>) -> String {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        Err(e) => panic!("unexpected error: {e}"),
        Ok(_) => panic!("call succeeded"),
    }
}

fn empty() -> HashMap<&'static str, Value<'static>> {
    HashMap::new()
}

#[test]
fn no_bus_returns_nobus() {
    let conn = Builder::address("unix:path=/nonexistent").and_then(|b| b.build());
    assert!(matches!(
        acquire_on(conn, TEST_NAME, &no_forward(), None),
        Outcome::NoBus
    ));
}

/// `--app-id`: a copy with another id takes its own name instead of forwarding (APP-052).
#[test]
#[ignore = "needs dbus-daemon"]
fn app_id_runs_its_own_instance() {
    let bus = PrivateBus::start();
    let Outcome::Primary {
        conn: installed, ..
    } = acquire_on(Ok(bus.connect()), TEST_NAME, &no_forward(), None)
    else {
        panic!("the installed copy did not get the name");
    };
    let dev = "io.github.oysteinkrog.TasklensTestDev";
    let Outcome::Primary { conn: dev_conn, .. } =
        acquire_on(Ok(bus.connect()), dev, &no_forward(), None)
    else {
        panic!("the --app-id copy forwarded instead of starting its own instance");
    };
    // A second copy with the same id forwards to the first.
    assert!(matches!(
        acquire_on(Ok(bus.connect()), dev, &no_forward(), None),
        Outcome::Forwarded
    ));
    drop(dev_conn);
    drop(installed);
}

#[test]
#[ignore = "needs dbus-daemon"]
fn interface_methods() {
    let bus = PrivateBus::start();
    let Outcome::Primary { conn, requests } =
        acquire_on(Ok(bus.connect()), TEST_NAME, &no_forward(), None)
    else {
        panic!("the first copy did not get the name");
    };
    let owner = conn.unique_name().expect("unique name").to_string();
    let client = bus.connect();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&client).expect("bus proxy");
    let name_owner = dbus
        .get_name_owner(TEST_NAME.try_into().expect("bus name"))
        .expect("name has an owner");
    assert_eq!(name_owner.to_string(), owner);

    let call = |method: &str, body: &dyn erased_body::Body| body.call(&client, method);

    // Bad parameters are refused and send nothing.
    let zero = ("select-pid", vec![Value::from(0u32)], empty());
    assert_eq!(
        error_name(call("ActivateAction", &zero)),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    let nowhere = ("tab", vec![Value::from("nowhere")], empty());
    assert_eq!(
        error_name(call("ActivateAction", &nowhere)),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    let unknown = ("rm-rf", Vec::<Value<'_>>::new(), empty());
    assert_eq!(
        error_name(call("ActivateAction", &unknown)),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    let open = (vec!["file:///x"], empty());
    assert_eq!(
        error_name(call("Open", &open)),
        "org.freedesktop.DBus.Error.NotSupported"
    );
    assert!(
        requests.try_recv().is_err(),
        "a refused call sent a request"
    );

    // Quit.
    let quit = ("quit", Vec::<Value<'_>>::new(), empty());
    call("ActivateAction", &quit).expect("quit");
    assert_eq!(requests.try_recv(), Ok(AppRequest::Quit));

    // A 300-byte token is ignored; a short one is kept.
    let long = "t".repeat(300);
    let data = HashMap::from([("activation-token", Value::from(long.as_str()))]);
    call("Activate", &(data,)).expect("activate");
    assert_eq!(
        requests.try_recv(),
        Ok(AppRequest::Activate { token: None })
    );
    let data = HashMap::from([("activation-token", Value::from("tok"))]);
    call("Activate", &(data,)).expect("activate");
    assert_eq!(
        requests.try_recv(),
        Ok(AppRequest::Activate {
            token: Some("tok".into())
        })
    );

    // Five calls with no receiver progress: four queued, the fifth dropped without an error.
    for _ in 0..5 {
        call("Activate", &(empty(),)).expect("activate");
    }
    for _ in 0..4 {
        assert_eq!(
            requests.try_recv(),
            Ok(AppRequest::Activate { token: None })
        );
    }
    assert!(requests.try_recv().is_err(), "the fifth request was queued");

    // A second copy forwards Activate then the tab, and gets Forwarded.
    let forward = Forward {
        tab: Some(Tab::Services),
        select_pid: None,
    };
    let second = acquire_on(Ok(bus.connect()), TEST_NAME, &forward, None);
    assert!(matches!(second, Outcome::Forwarded));
    // One Activate from the Activate call and one from the tab action.
    for _ in 0..2 {
        assert_eq!(
            requests.try_recv(),
            Ok(AppRequest::Activate { token: None })
        );
    }
    assert_eq!(
        requests.try_recv(),
        Ok(AppRequest::SelectTab(Tab::Services))
    );
    assert!(requests.try_recv().is_err());
    drop(conn);
}

/// Lets `interface_methods` call one closure with bodies of different types.
mod erased_body {
    use super::{INTERFACE, TEST_NAME, object_path};
    use zbus::blocking::Connection;

    pub trait Body {
        fn call(&self, conn: &Connection, method: &str) -> zbus::Result<zbus::message::Message>;
    }

    impl<B> Body for B
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        fn call(&self, conn: &Connection, method: &str) -> zbus::Result<zbus::message::Message> {
            conn.call_method(
                Some(TEST_NAME),
                object_path(TEST_NAME).as_str(),
                Some(INTERFACE),
                method,
                self,
            )
        }
    }
}

/// Waits up to `limit` for `done`, checking every 50 ms.
fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

fn log_contains(logs_dir: &Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(needle)))
}

/// True when a display and `dbus-daemon` are present, so the end-to-end test can run.
fn can_run_end_to_end() -> bool {
    let display = ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()));
    let daemon = Command::new("dbus-daemon")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    display && daemon
}

#[test]
fn second_instance_forwards_and_exits() {
    if !can_run_end_to_end() {
        eprintln!("skipped: needs dbus-daemon and a display");
        return;
    }
    let bus = PrivateBus::start();
    let home = tempfile::tempdir().expect("tempdir");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tasklens"));
        command
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("TASKLENS_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    };
    let mut first = command().spawn().expect("start the first copy");

    let client = bus.connect();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&client).expect("bus proxy");
    let owned = wait_for(Duration::from_secs(30), || {
        dbus.name_has_owner(single_instance::BUS_NAME.try_into().expect("bus name"))
            .unwrap_or(false)
    });
    if !owned {
        let _ = first.kill();
        let _ = first.wait();
        panic!("the first copy did not take the bus name");
    }

    let mut second = command()
        .args(["--tab", "services"])
        .spawn()
        .expect("start the second copy");
    let mut status = None;
    wait_for(Duration::from_secs(1), || {
        status = second.try_wait().expect("second copy status");
        status.is_some()
    });
    if status.is_none() {
        let _ = second.kill();
        let _ = second.wait();
    }

    let logs_dir = home.path().join("state/tasklens/logs");
    let logged = wait_for(Duration::from_secs(5), || {
        log_contains(&logs_dir, "D-Bus Activate received")
    });
    let _ = first.kill();
    let _ = first.wait();

    let status = status.expect("the second copy did not exit within 1 s");
    assert!(status.success(), "the second copy exited with {status}");
    assert!(logged, "the first copy did not log Activate");
}
