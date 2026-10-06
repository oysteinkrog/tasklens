//! `tasklens --gallery`: the component gallery window (UI-FND-030, R-138 item 15).
//!
//! Compiled only with the `bench` feature. No engine starts: the gallery uses no snapshot data,
//! so tl-ui gets a client that holds one empty snapshot and never wakes.

use std::sync::Arc;

use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::settings::{EngineSettings, Settings, UiState};
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;
use tl_ui::{AppEnv, BuildInfo, OpenMode, StartOptions, UiInit, UiLaunch};
use tracing::info;

/// Opens the gallery window and runs GPUI until it closes. Returns the exit code (always 0).
pub fn run() -> i32 {
    let (_app_requests_tx, app_requests) = async_channel::bounded(1);
    let env = AppEnv {
        build: BuildInfo {
            version: env!("CARGO_PKG_VERSION"),
            git_hash: env!("TASKLENS_GIT_SHA"),
            ..BuildInfo::default()
        },
        ..AppEnv::test()
    };
    let exit = tl_ui::run(UiLaunch {
        client: Arc::new(GalleryClient::new()),
        init: UiInit {
            settings: Settings::default(),
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests,
            startup_toasts: Vec::new(),
            env,
        },
        open: OpenMode::Gallery,
        start: StartOptions::default(),
    });
    info!(target: "tasklens::ui", ?exit, "gallery closed");
    0
}

/// A client with no engine behind it. It keeps both channel senders, so the hub's receive task
/// waits instead of ending, and it refuses every command.
struct GalleryClient {
    snapshot: Arc<Snapshot>,
    _wake_tx: async_channel::Sender<()>,
    wake: async_channel::Receiver<()>,
    _events_tx: async_channel::Sender<ActionEvent>,
    action_events: async_channel::Receiver<ActionEvent>,
}

impl GalleryClient {
    fn new() -> Self {
        let (_wake_tx, wake) = async_channel::bounded(1);
        let (_events_tx, action_events) = async_channel::bounded(1);
        Self {
            snapshot: Arc::new(Snapshot::empty(BootId(0), 0)),
            _wake_tx,
            wake,
            _events_tx,
            action_events,
        }
    }
}

impl EngineClient for GalleryClient {
    fn latest(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.wake.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.action_events.clone()
    }

    fn set_demand(&self, _demand: Demand) {}

    fn set_settings(&self, _settings: EngineSettings) {}

    fn send(&self, _cmd: Command) -> Result<(), CommandError> {
        Err(CommandError::EngineStopped)
    }

    /// A stream that ends at once: both far ends are dropped.
    fn journal(&self, _query: JournalQuery) -> JournalHandle {
        let (_events_tx, events) = async_channel::bounded(1);
        let (control, _control_rx) = async_channel::bounded(1);
        JournalHandle::new(events, control)
    }
}
