//! The fixture engine never talks to the real desktop: with `DesktopSource::None` the KWin
//! worker reports `Unsupported` and the user systemd manager `Unavailable`, with no session
//! bus connection (tasklens-aujsq).

use std::time::{Duration, Instant};

use tl_collect::sysroot::SysRoot;
use tl_engine::bridge::{Engine, EngineHooks};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::desktop::DesktopSource;
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::health::{ManagerState, SourceId, SourceState};

#[test]
fn fixture_engine_keeps_desktop_sources_off() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tl-collect/tests/fixtures/minimal-vm");
    let state = tempfile::tempdir().expect("state dir");
    let config = EngineConfig::new(EngineMode::Full, state.path().join("state"))
        .with_sys_root(SysRoot::at(&base))
        .with_desktop(DesktopSource::None);
    let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
    handle.set_demand(Demand {
        any_surface_visible: true,
        services: true,
        ..Demand::default()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut off = false;
    while Instant::now() < deadline {
        let snap = handle.latest();
        if snap.health.get(SourceId::KWin).state == SourceState::Unsupported
            && snap.services.managers[1] == ManagerState::Unavailable
        {
            off = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = handle.shutdown();
    assert!(off, "kwin and the user manager must report off");
    assert!(report.drained && report.torn_down, "{report:?}");
}
