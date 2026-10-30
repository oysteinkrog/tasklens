//! StatusNotifierItem tray and tray graph rasterizer (P3).

pub mod item;
pub mod menu;
mod model;
pub mod raster;

pub use item::dropped_events;
pub use tl_model::tray::{TrayEvent, TrayGraphId, TrayModel, TrayServiceEnd};

use model::{ItemBackend, KsniBackend, Service};

/// ARGB32 pixels of the app icon for the main tray item, supplied by the caller (b1-infra open item E31-4).
pub struct MainIcon {
    pub argb_22: Vec<u8>,
    pub argb_44: Vec<u8>,
}

/// Starts the thread "tasklens-tray". It runs until the model channel closes, then shuts every item down and returns.
pub fn spawn(
    end: TrayServiceEnd,
    main_icon: MainIcon,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("tasklens-tray".to_string())
        .spawn(move || run(end, main_icon, KsniBackend))
}

fn run<B: ItemBackend>(end: TrayServiceEnd, main_icon: MainIcon, backend: B) {
    let mut service = Service::new(backend, end.events, main_icon);
    while let Ok(model) = end.models.recv_blocking() {
        service.apply(&model);
    }
    service.shutdown_all();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::tray::{TrayGraphFrame, TrayGraphState, TrayMenuState, tray_channels};

    use super::*;
    use crate::model::fake::FakeBackend;

    fn main_icon() -> MainIcon {
        MainIcon {
            argb_22: vec![7; 22 * 22 * 4],
            argb_44: vec![9; 44 * 44 * 4],
        }
    }

    fn graph(id: u32) -> TrayGraphFrame {
        TrayGraphFrame {
            id: TrayGraphId(id),
            series: None,
            now_ns: 10_000_000_000,
            range_ns: 60_000_000_000,
            max: 100.0,
            line_rgb: [0x35, 0x84, 0xe4],
            state: TrayGraphState::Live,
            tooltip_title: Arc::from("CPU Usage"),
            tooltip_description: Arc::from("18.6%"),
            color: Arc::from("accent"),
            range_s: 60,
        }
    }

    fn model(graphs: Vec<TrayGraphFrame>) -> TrayModel {
        TrayModel {
            show_main_icon: true,
            main_tooltip_title: Arc::from("tasklens"),
            main_tooltip_description: Arc::from("CPU 12%"),
            graphs: Arc::from(graphs),
            menu: TrayMenuState::default(),
        }
    }

    #[test]
    fn thread_exits_on_closed_channel() {
        let (ui, service_end) = tray_channels();
        let backend = FakeBackend::default();
        let thread_backend = backend.clone();
        let thread = std::thread::Builder::new()
            .name("tasklens-tray".to_string())
            .spawn(move || run(service_end, main_icon(), thread_backend))
            .expect("spawn thread");

        ui.models
            .send_blocking(model(vec![graph(1), graph(2)]))
            .expect("send model");
        drop(ui);
        thread.join().expect("tray thread panicked");

        let log = backend.log();
        assert_eq!(log.items.len(), 2);
        assert_eq!(log.shutdowns, [0, 1]);
        assert_eq!(log.waits, [0, 1]);
        assert!(log.live().is_empty());
    }

    #[test]
    fn spawn_names_thread_and_exits_without_items() {
        let (ui, service_end) = tray_channels();
        let thread = spawn(service_end, main_icon()).expect("spawn");
        assert_eq!(thread.thread().name(), Some("tasklens-tray"));
        // No model was sent, so no ksni item is registered on the live desktop.
        drop(ui);
        thread.join().expect("tray thread panicked");
    }

    /// Environment variable that marks the child process of `tray_reregisters_on_watcher_restart`.
    /// The child runs only with it set, so it never touches the real session bus.
    const PRIVATE_BUS: &str = "TL_TRAY_PRIVATE_BUS";

    /// Real system: starts a private `dbus-daemon --session` and runs
    /// `reregister_child` in a child process whose session bus is that daemon (PERF-092).
    #[test]
    #[ignore = "real system: starts a private dbus-daemon"]
    fn tray_reregisters_on_watcher_restart() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start dbus-daemon");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().expect("daemon stdout"))
            .read_line(&mut address)
            .expect("read bus address");

        let status = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "tests::reregister_child", "--ignored"])
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .env(PRIVATE_BUS, "1")
            .status()
            .expect("run child test");

        daemon.kill().expect("stop dbus-daemon");
        let _ = daemon.wait();
        assert!(status.success(), "child test failed: {status}");
    }

    #[test]
    #[ignore = "child of tray_reregisters_on_watcher_restart"]
    fn reregister_child() {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};

        if std::env::var_os(PRIVATE_BUS).is_none() {
            return;
        }

        struct Watcher(Arc<Mutex<Vec<String>>>);

        #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
        impl Watcher {
            fn register_status_notifier_item(&self, service: &str) {
                self.0.lock().expect("calls lock").push(service.to_string());
            }

            fn register_status_notifier_host(&self, _service: &str) {}

            #[zbus(property)]
            fn registered_status_notifier_items(&self) -> Vec<String> {
                self.0.lock().expect("calls lock").clone()
            }

            #[zbus(property)]
            fn is_status_notifier_host_registered(&self) -> bool {
                true
            }

            #[zbus(property)]
            fn protocol_version(&self) -> i32 {
                0
            }
        }

        const WATCHER: &str = "org.kde.StatusNotifierWatcher";

        fn wait_for_calls(calls: &Mutex<Vec<String>>, n: usize, limit: Duration) -> Duration {
            let start = Instant::now();
            while calls.lock().expect("calls lock").len() < n {
                assert!(
                    start.elapsed() < limit,
                    "no registration {n} within {limit:?}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            start.elapsed()
        }

        fn icon(conn: &zbus::blocking::Connection, name: &str) -> Vec<(i32, i32, Vec<u8>)> {
            zbus::blocking::Proxy::new(
                conn,
                name.to_string(),
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
            )
            .expect("item proxy")
            .get_property("IconPixmap")
            .expect("IconPixmap")
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let conn = zbus::blocking::connection::Builder::session()
            .expect("session builder")
            .serve_at("/StatusNotifierWatcher", Watcher(calls.clone()))
            .expect("serve watcher")
            .name(WATCHER)
            .expect("watcher name")
            .build()
            .expect("connect to private bus");

        let (ui, service_end) = tray_channels();
        let thread = spawn(service_end, main_icon()).expect("spawn");
        let mut frame = graph(1);
        frame.series = None;
        ui.models
            .send_blocking(model(vec![frame]))
            .expect("send model");

        wait_for_calls(&calls, 1, Duration::from_secs(5));
        let name = calls.lock().expect("calls lock")[0].clone();
        let first = icon(&conn, &name);
        assert_eq!(first.len(), 2);

        conn.release_name(WATCHER).expect("release watcher name");
        std::thread::sleep(Duration::from_millis(200));
        conn.request_name(WATCHER).expect("retake watcher name");

        let took = wait_for_calls(&calls, 2, Duration::from_secs(2));
        assert!(took < Duration::from_secs(2));
        assert_eq!(calls.lock().expect("calls lock")[1], name);
        assert_eq!(icon(&conn, &name), first);

        drop(ui);
        thread.join().expect("tray thread panicked");
    }
}
