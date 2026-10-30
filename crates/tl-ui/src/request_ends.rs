//! `RequestEnds`: reads `EngineClient::request_events` and hands each end to the views
//! (ADR-018, tasklens-q4x76.17).
//!
//! The engine sends exactly one `RequestEvent` per action, Strings scan, handle search and
//! eject, on an unbounded channel. One task per app reads it as soon as an event arrives, so the
//! channel is empty before every frame, and emits each event. Views that sent a request
//! subscribe and act on the ends the snapshot cannot show: a request the engine lost (`Lost`)
//! or refused (`Busy`) never gets a result there, so without this event it would wait forever.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Task, WeakEntity,
};
use tl_model::client::EngineClient;
use tl_model::request::RequestEvent;

/// The app's reader of request ends.
pub struct RequestEnds {
    /// Ends read so far.
    received: u64,
    _events: Task<()>,
}

/// The app's [`RequestEnds`], set by [`install`].
pub struct GlobalRequestEnds(pub Entity<RequestEnds>);

impl Global for GlobalRequestEnds {}

impl EventEmitter<RequestEvent> for RequestEnds {}

/// Creates the reader and sets [`GlobalRequestEnds`].
pub fn install(client: Arc<dyn EngineClient>, cx: &mut App) {
    let ends = cx.new(|cx| RequestEnds::new(client.as_ref(), cx));
    cx.set_global(GlobalRequestEnds(ends));
}

impl RequestEnds {
    /// Starts the task that reads every end and emits it.
    pub fn new(client: &dyn EngineClient, cx: &mut Context<Self>) -> Self {
        let events = client.request_events();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(ev) = events.recv().await {
                let read = this.update(cx, |t, cx| {
                    t.received += 1;
                    cx.emit(ev);
                });
                if read.is_err() {
                    return;
                }
            }
        });
        Self {
            received: 0,
            _events: task,
        }
    }

    /// Ends read so far.
    pub fn received(&self) -> u64 {
        self.received
    }
}

/// The app's [`RequestEnds`], if [`install`] ran.
pub fn global(cx: &App) -> Option<Entity<RequestEnds>> {
    cx.try_global::<GlobalRequestEnds>().map(|g| g.0.clone())
}
