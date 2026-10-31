//! KWin bridge for tasklens's own small windows (E26-019, PERF-083, PERF-093, R-084, R-086).
//! On each `WINDOWS` change it matches the detached graph windows and the mini window in
//! `WindowsSnapshot` by pid and caption, pins them above, restores a saved position, and saves
//! the position the user moves a detached window to, 500 ms after the last move.
//!
//! The windows are this PC's own, so the bridge always uses the local host's hub and engine,
//! whatever host the main window shows.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, Global, Task, WeakEntity};
use tl_model::command::{Command, KWinCommand};
use tl_model::snapshot::FrameGeometry;

use super::panel::MinigraphPanel;
use crate::host::HostContext;
use crate::hub::{ComponentMask, SnapshotChanged};
use crate::window::registry::{GlobalRegistry, WindowKey};
use crate::window::titlebar::keep_above_supported;
use crate::windows::mini::{MINI_WINDOW_TITLE, MiniWindow};

/// The last move of a detached window is saved this long after it.
const POSITION_SAVE_DELAY: Duration = Duration::from_millis(500);

/// Caption prefix of a detached graph window; the window id follows.
const GRAPH_CAPTION_PREFIX: &str = "tasklens Graph ";

/// Which of the app's own windows a caption names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OwnWindow {
    Detached(u32),
    Mini,
}

fn parse_caption(caption: &str) -> Option<OwnWindow> {
    if caption == MINI_WINDOW_TITLE {
        return Some(OwnWindow::Mini);
    }
    let id: u32 = caption.strip_prefix(GRAPH_CAPTION_PREFIX)?.parse().ok()?;
    Some(OwnWindow::Detached(id))
}

pub struct OwnWindowBridge {
    /// KWin internal id to the last frame seen, for the windows already matched.
    seen: HashMap<Arc<str>, FrameGeometry>,
    /// Moved detached windows waiting for the debounce: window id to `(x, y)`.
    pending: BTreeMap<u32, (i32, i32)>,
    save_task: Option<Task<()>>,
    /// Positions written to the panel state.
    #[cfg(any(test, feature = "test-hooks"))]
    pub writes: u32,
}

struct GlobalOwnWindowBridge(Entity<OwnWindowBridge>);

impl Global for GlobalOwnWindowBridge {}

impl OwnWindowBridge {
    /// Starts the bridge once. The window openers call it first. Nothing happens without a hub.
    pub fn ensure(cx: &mut App) {
        if cx.has_global::<GlobalOwnWindowBridge>() {
            return;
        }
        // This PC's own windows: the local host.
        let Some(hub) = HostContext::local_hub(cx).cloned() else {
            return;
        };
        let bridge = cx.new(|cx| {
            cx.subscribe(&hub, |this: &mut Self, _, ev: &SnapshotChanged, cx| {
                if ev.mask.intersects(ComponentMask::WINDOWS) {
                    this.on_windows(cx);
                }
            })
            .detach();
            Self {
                seen: HashMap::new(),
                pending: BTreeMap::new(),
                save_task: None,
                #[cfg(any(test, feature = "test-hooks"))]
                writes: 0,
            }
        });
        cx.set_global(GlobalOwnWindowBridge(bridge.clone()));
        // Windows may already be listed when the first one opens.
        bridge.update(cx, |this, cx| this.on_windows(cx));
    }

    /// The running bridge, if any.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalOwnWindowBridge>()
            .map(|g| g.0.clone())
    }

    fn on_windows(&mut self, cx: &mut Context<Self>) {
        if !keep_above_supported(cx) {
            return;
        }
        // This PC's own windows: the local host.
        let Some(hub) = HostContext::local_hub(cx).cloned() else {
            return;
        };
        let snapshot = Arc::clone(&hub.read(cx).current().windows);
        let mut live: Vec<Arc<str>> = Vec::new();
        for entry in snapshot.for_pid(std::process::id()) {
            let Some(own) = parse_caption(&entry.caption) else {
                continue;
            };
            if !is_open(own, cx) {
                continue;
            }
            live.push(Arc::clone(&entry.internal_id));
            match self.seen.get(&entry.internal_id).copied() {
                None => {
                    if first_match(own, entry.frame_geometry, &entry.caption, cx) {
                        self.seen
                            .insert(Arc::clone(&entry.internal_id), entry.frame_geometry);
                    }
                }
                Some(last) => {
                    let now = entry.frame_geometry;
                    if (now.x, now.y) == (last.x, last.y) {
                        continue;
                    }
                    self.seen.insert(Arc::clone(&entry.internal_id), now);
                    if let OwnWindow::Detached(id) = own {
                        self.pending.insert(id, (now.x, now.y));
                        self.restart_save(cx);
                    }
                }
            }
        }
        // A window that closed and opens again is matched again.
        self.seen.retain(|id, _| live.contains(id));
    }

    fn restart_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(POSITION_SAVE_DELAY).await;
                let _ = this.update(cx, |this, cx| this.flush(cx));
            }),
        );
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.pending);
        let Some(panel) = MinigraphPanel::global(cx) else {
            return;
        };
        for (id, (x, y)) in pending {
            let wrote = panel.update(cx, |p, cx| p.set_window_position(id, x, y, cx));
            #[cfg(any(test, feature = "test-hooks"))]
            if wrote {
                self.writes += 1;
            }
            #[cfg(not(any(test, feature = "test-hooks")))]
            let _ = wrote;
        }
    }
}

fn is_open(own: OwnWindow, cx: &App) -> bool {
    let key = match own {
        OwnWindow::Mini => WindowKey::Mini,
        OwnWindow::Detached(id) => match u8::try_from(id) {
            Ok(id) => WindowKey::DetachedGraph(id),
            Err(_) => return false,
        },
    };
    cx.try_global::<GlobalRegistry>()
        .is_some_and(|r| r.0.read(cx).get(&key).is_some())
}

/// Pins a window the first time it is listed and restores a saved detached position. True when
/// every command was accepted, so a busy engine gets another try on the next change.
fn first_match(own: OwnWindow, frame: FrameGeometry, caption: &Arc<str>, cx: &App) -> bool {
    // This PC's own windows: the local host's engine.
    let Some(engine) = HostContext::local_client(cx).cloned() else {
        return false;
    };
    let pid = std::process::id();
    let (on, saved) = match own {
        OwnWindow::Detached(id) => {
            let saved = MinigraphPanel::global(cx).and_then(|p| {
                p.read(cx)
                    .windows()
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| (w.x, w.y))
            });
            (true, saved.filter(|&xy| xy != (0, 0)))
        }
        OwnWindow::Mini => {
            let on = MiniWindow::open_view(cx).is_some_and(|m| m.read(cx).always_on_top());
            (on, None)
        }
    };
    let keep = Command::KWin(KWinCommand::KeepAbove {
        pid,
        caption: Some(Arc::clone(caption)),
        on,
    });
    if engine.send(keep).is_err() {
        return false;
    }
    let Some((x, y)) = saved else {
        return true;
    };
    let geometry = FrameGeometry {
        x,
        y,
        width: frame.width,
        height: frame.height,
    };
    engine
        .send(Command::KWin(KWinCommand::SetFrameGeometry {
            pid,
            caption: Arc::clone(caption),
            geometry,
        }))
        .is_ok()
}
