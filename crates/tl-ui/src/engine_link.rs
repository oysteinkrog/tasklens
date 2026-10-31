//! Where the window's engine runs, for the status chip (ADR-018 Window fallback and Lifetime,
//! tasklens-q4x76.8).
//!
//! tl-app decides at start whether the window uses the engine service or its own in-process
//! engine, and sends each change on `EngineLinkApi::states`. `install` keeps the newest state in
//! the `EngineLinkGlobal`, which the status chip observes. "Switch to engine service" in the chip
//! dispatches `SwitchToEngineService`, which calls `EngineLinkApi::switch`.

use std::sync::Arc;

use gpui_kit::{App, AsyncApp, Global};
use tracing::info;

use crate::app::AppEnv;

gpui_kit::actions!(tasklens, [SwitchToEngineService]);

/// Where the window's engine runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EngineLinkState {
    /// The window uses the engine service or is connecting to it. The chip shows nothing.
    #[default]
    Service,
    /// The service did not answer, so the window runs its own engine: "Engine service not
    /// running". `can_switch` is false when tl-app cannot start the service's unit (an
    /// `--app-id` instance); the chip then does not offer "Switch to engine service".
    Fallback { can_switch: bool },
    /// Another process holds the engine lock and the socket does not answer: "Can't start the
    /// engine". tl-app keeps trying the socket.
    NoEngine,
}

/// What tl-app gives the UI. `None` in `AppEnv` for the rescue window and in tests.
pub struct EngineLinkApi {
    /// The state at start, then every change.
    pub states: async_channel::Receiver<EngineLinkState>,
    /// Starts the switch to the engine service and returns at once. tl-app sends the new state
    /// when the switch ends.
    pub switch: Arc<dyn Fn() + Send + Sync>,
}

/// The newest state from tl-app.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EngineLinkGlobal(pub EngineLinkState);

impl Global for EngineLinkGlobal {}

/// The state for the chip. `Service` when tl-app gave no link.
pub fn state(cx: &App) -> EngineLinkState {
    cx.try_global::<EngineLinkGlobal>()
        .map_or(EngineLinkState::Service, |g| g.0)
}

/// Sets the state when it changed, which the chip observes.
pub fn set_state(state: EngineLinkState, cx: &mut App) {
    if cx.try_global::<EngineLinkGlobal>().map(|g| g.0) != Some(state) {
        cx.set_global(EngineLinkGlobal(state));
    }
}

/// Reads the states tl-app sends and registers the `SwitchToEngineService` handler. Call it
/// after `AppEnv` is set. The states already queued are applied at once, so the first frame
/// shows the start state.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &SwitchToEngineService, cx| {
        let switch = cx
            .try_global::<AppEnv>()
            .and_then(|env| env.engine_link.as_ref())
            .map(|api| Arc::clone(&api.switch));
        if let Some(switch) = switch {
            info!(target: "tasklens::engine", "switch to the engine service requested");
            switch();
        }
    });
    let Some(states) = cx
        .try_global::<AppEnv>()
        .and_then(|env| env.engine_link.as_ref())
        .map(|api| api.states.clone())
    else {
        return;
    };
    let mut start = EngineLinkState::Service;
    while let Ok(next) = states.try_recv() {
        start = next;
    }
    set_state(start, cx);
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(next) = states.recv().await {
            cx.update(|cx| set_state(next, cx));
        }
    })
    .detach();
}
