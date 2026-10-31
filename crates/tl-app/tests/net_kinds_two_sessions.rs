//! Two windows on one engine, each with its own network kinds (ADR-018, tasklens-q4x76.22).
//!
//! The real engine runs on a copy of the `owner-desktop` fixture. Window A is the local session
//! and shows every adapter kind. Window B is a second client session, opened with
//! `EngineHandle::open_session` as the engine service does, and shows wired adapters only. Both
//! read their network total with `tl_ui::net_totals::NetTotal`, the code behind the Overview
//! box, the tray and the minigraphs. The engine sends both sessions the same totals; each window
//! takes away the kinds it hides, so A sees every adapter and B sees the wired one alone.
//!
//! The test moves the receive counters of `enp38s0` (wired), `wlan0` and `tailscale0` in the
//! copied `proc/net/dev` on every step, so the engine reads real download rates.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::client_session::ClientSessionId;
use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient as _;
use tl_model::demand::Demand;
use tl_model::devices::{AggregateKind, DeviceIdentity, NetKind};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::settings::PerformanceSettings;
use tl_model::snapshot::Snapshot;
use tl_ui::net_totals::NetTotal;

/// How long the engine gets to publish matching totals to both sessions. The net worker samples
/// once a second; a debug build on a loaded machine gets more time.
const DEADLINE: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_millis(200);

fn fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tl-collect/tests/fixtures/"
    ))
}

/// Copies every entry of `src` over `dst`, keeping symlinks as symlinks.
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create fixture dir");
    for entry in fs::read_dir(src).expect("list fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path()).expect("read fixture symlink");
            std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
        } else if file_type.is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}

/// Moves the receive bytes of three interfaces in the copied `proc/net/dev` on by a fixed
/// amount per call: 4 MB for `enp38s0`, 1 MB for `wlan0` and 2 MB for `tailscale0`.
struct NetBytes {
    path: PathBuf,
    text: String,
    calls: u64,
}

impl NetBytes {
    const STEPS: [(&'static str, u64); 3] = [
        ("enp38s0", 4_000_000),
        ("wlan0", 1_000_000),
        ("tailscale0", 2_000_000),
    ];

    fn new(root: &Path) -> Self {
        let path = root.join("proc/net/dev");
        let text = fs::read_to_string(&path).expect("read proc/net/dev");
        Self {
            path,
            text,
            calls: 0,
        }
    }

    fn advance(&mut self) {
        self.calls += 1;
        let mut out = String::with_capacity(self.text.len() + 64);
        for line in self.text.lines() {
            let step = line.split_once(':').and_then(|(name, _)| {
                Self::STEPS
                    .iter()
                    .find(|(n, _)| *n == name.trim())
                    .map(|(_, s)| *s)
            });
            match (step, line.split_once(':')) {
                (Some(step), Some((name, rest))) => {
                    let mut fields: Vec<String> =
                        rest.split_whitespace().map(str::to_owned).collect();
                    let rx: u64 = fields[0].parse().expect("rx bytes");
                    fields[0] = (rx + step * self.calls).to_string();
                    out.push_str(&format!("{name}: {}\n", fields.join(" ")));
                }
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        fs::write(&self.path, out).expect("write proc/net/dev");
    }
}

/// The engine on `root`, with app lookups on the tl-collect app fixtures and the icon cache in
/// `dir`, so the test never writes the real `~/.cache`. No helper task.
fn start_engine(dir: &Path, root: &Path) -> EngineHandle {
    let share = |name: &str| fixtures().join(name).join("usr/share");
    let dirs = [share("apps-desktop"), share("apps-icons")];
    let paths = AppPaths {
        home: fixtures().join("apps-icons/home"),
        xdg_data_home: None,
        xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
        xdg_config_home: None,
        icon_cache: dir.join("cache/icons.toml"),
    };
    let config = EngineConfig::new(EngineMode::Full, dir.join("state"))
        .with_history_dir(dir.join("history"))
        .with_sys_root(SysRoot::at(root))
        .with_app_paths(paths);
    let hooks = EngineHooks {
        helper: None,
        ..EngineHooks::default()
    };
    Engine::start(config, hooks).expect("engine starts")
}

/// What a window with settings `p` reads from `snap`, at the newest sample of `aggregate:net`.
#[derive(Debug, PartialEq)]
struct Read {
    /// The time of the newest valid `aggregate:net` download sample.
    t_ns: u64,
    /// The engine's total of every kind at `t_ns`.
    engine_total: f32,
    /// The window's shown total: `NetTotal::newest`.
    shown: f32,
    /// The sum of the engine's per-kind totals of the kinds `p` shows, at `t_ns`.
    shown_kinds: f32,
}

/// The newest valid download sample of aggregate `kind`, as (time, value).
fn newest(snap: &Snapshot, kind: AggregateKind) -> Option<(u64, f32)> {
    let device = snap.devices.find(&DeviceIdentity::Aggregate(kind))?.idx;
    let view = snap.series.get(SeriesId {
        device,
        metric: MetricId::NET_RX,
    })?;
    let s = view.samples.iter().rev().find(|s| s.is_valid())?;
    Some((s.t_ns, s.value))
}

fn read(snap: &Snapshot, p: &PerformanceSettings) -> Option<Read> {
    let (t_ns, engine_total) = newest(snap, AggregateKind::Nets)?;
    let shown = NetTotal::of(&snap.devices, p).newest(snap, MetricId::NET_RX)?;
    let mut shown_kinds = 0.;
    for kind in NetKind::ALL
        .into_iter()
        .filter(|&k| p.shows_net_kind(Some(k)))
    {
        let (t, v) = newest(snap, AggregateKind::NetsOf(kind))?;
        if t != t_ns {
            return None;
        }
        shown_kinds += v;
    }
    Some(Read {
        t_ns,
        engine_total,
        shown,
        shown_kinds,
    })
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= a.abs().max(b.abs()) * 1e-4 + 1.
}

#[test]
fn two_sessions_see_their_own_network_totals_from_one_engine() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
    let root = dir.path().join("root");
    copy_tree(&fixtures().join("owner-desktop"), &root);
    let mut bytes = NetBytes::new(&root);
    let engine = start_engine(dir.path(), &root);

    let demand = Demand {
        any_surface_visible: true,
        nets: true,
        ..Demand::default()
    };
    engine.set_demand(demand.clone());
    let port = engine
        .open_session(ClientSessionId(1))
        .expect("the engine opens a second session");
    port.set_demand(demand);

    let all = PerformanceSettings {
        show_network_wired: true,
        show_network_wireless: true,
        show_network_vpn: true,
        show_network_virtual: true,
        show_network_other: true,
        ..PerformanceSettings::default()
    };
    let wired_only = PerformanceSettings {
        show_network_wired: true,
        show_network_wireless: false,
        show_network_vpn: false,
        show_network_virtual: false,
        show_network_other: false,
        ..PerformanceSettings::default()
    };

    let begin = Instant::now();
    let mut last = (None, None);
    while begin.elapsed() < DEADLINE {
        bytes.advance();
        std::thread::sleep(STEP);
        let a = read(&engine.latest(), &all);
        let b = read(&port.latest(), &wired_only);
        let done = matches!((&a, &b), (Some(a), Some(b))
            if a.t_ns == b.t_ns && b.shown > 0. && a.shown > b.shown);
        last = (a, b);
        if done {
            break;
        }
    }
    drop(port);
    // This test is about the totals, not shutdown, so the report is only printed. The engine is
    // a few seconds old here and its startup scans may still be in a system call, which leaves
    // `torn_down` false. Shutdown is tested in tl-engine (`lab_bridge`, `shutdown_during_start`).
    let report = engine.shutdown();
    eprintln!(
        "window A {:?}\nwindow B {:?}\nshutdown {report:?}",
        last.0, last.1
    );

    let (Some(a), Some(b)) = last else {
        panic!("both sessions read a network total before the deadline: {last:?}");
    };
    assert_eq!(a.t_ns, b.t_ns, "both read the same engine sample");
    // The engine sends both sessions the same total of every kind.
    assert_eq!(a.engine_total, b.engine_total);
    // Window A shows every kind: the engine's total as it is.
    assert!(close(a.shown, a.engine_total), "{a:?}");
    assert!(close(a.shown, a.shown_kinds), "{a:?}");
    // Window B shows the wired total alone, which is less than the total of every kind.
    assert!(close(b.shown, b.shown_kinds), "{b:?}");
    assert!(b.shown > 0., "{b:?}");
    assert!(b.shown < a.shown, "A {a:?}, B {b:?}");
}
