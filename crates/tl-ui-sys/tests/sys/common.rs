//! The setup the system page tests share. `TasklensUi::init` installs more than
//! `PageHost::install` does; [`shell_free_init`] adds the part a system page or window needs,
//! and [`open_sink`] opens a window that stands in for the main window's toast queue.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::SharedString;
use gpui_kit::base::Root;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px,
    size,
};
use tl_model::client::EngineClient;
use tl_ui_base::app_env::AppEnv;
use tl_ui_base::host::HostContext;
use tl_ui_base::shell::{GlobalShellEvents, ShellApi, ShellEvents};
use tl_ui_base::{action_tracker, request_ends};
use tl_ui_kit::actions;
use tl_ui_kit::actions::GLOBAL_CONTEXT;
use tl_ui_kit::components::menu::context_menu_layer;
use tl_ui_kit::components::toast;
use tl_ui_kit::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use tl_ui_kit::helper_flow;
use tl_ui_pagehost::PageHost;
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_sys::pages::connections::actions::bind_keys_connections;
use tl_ui_sys::pages::services::actions::bind_keys_services;
use tl_ui_sys::pages::startup::actions::bind_keys_startup;
use tl_ui_table::table::actions::bind_keys_table;
use tl_ui_testkit::fake_engine::FakeEngine;

/// What `TasklensUi::init` installs for the system pages and windows, on top of the page host:
/// the toast queues, the key bindings, the action tracker, the request ends and the helper flow.
pub fn install_shell_services(cx: &mut App) {
    let local = HostContext::local(cx);
    let client = local
        .client()
        .cloned()
        .expect("an engine client is installed");
    let hub = local.hub().cloned().expect("a hub is installed");
    actions::bind_keys_global(cx);
    bind_keys_table(cx);
    bind_keys_startup(cx);
    bind_keys_connections(cx);
    bind_keys_services(cx);
    actions::register_global_handlers(cx);
    toast::init(cx);
    action_tracker::install(Arc::clone(&client), Some(hub), cx);
    request_ends::install(client, cx);
    helper_flow::install(cx);
    let events = cx.new(|_| ShellEvents);
    cx.set_global(GlobalShellEvents(events));
}

/// A new `FakeEngine` with the page host and the shell services installed.
pub fn shell_free_init(cx: &mut TestAppContext, settings: Settings) -> (Arc<FakeEngine>, PageHost) {
    shell_free_init_with(cx, settings, UiState::default(), AppEnv::test())
}

/// [`shell_free_init`] with a persisted `state` and an `env`.
pub fn shell_free_init_with(
    cx: &mut TestAppContext,
    settings: Settings,
    state: UiState,
    env: AppEnv,
) -> (Arc<FakeEngine>, PageHost) {
    let engine = FakeEngine::new();
    let host = shell_free_init_on(cx, Arc::clone(&engine), settings, state, env);
    (engine, host)
}

/// [`shell_free_init_with`] on `engine`.
pub fn shell_free_init_on(
    cx: &mut TestAppContext,
    engine: Arc<FakeEngine>,
    settings: Settings,
    state: UiState,
    env: AppEnv,
) -> PageHost {
    shell_free_init_client(cx, engine, None, settings, state, env)
}

/// [`shell_free_init_on`] with a `client` that wraps `engine`, for a test whose client answers a
/// demand with data. The hub, demand and action tracker use `client`.
pub fn shell_free_init_client(
    cx: &mut TestAppContext,
    engine: Arc<FakeEngine>,
    client: Option<Arc<dyn EngineClient>>,
    settings: Settings,
    state: UiState,
    env: AppEnv,
) -> PageHost {
    let host = match client {
        Some(client) => PageHost::install_with_client(cx, engine, client, settings, state, env),
        None => PageHost::install_with(cx, engine, settings, state, env),
    };
    cx.update(|cx| {
        install_shell_services(cx);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    host
}

/// The stand-in for the main window's toast queue: [`open_sink`] fills it and the `ShellApi`
/// calls that name the main window read it.
#[derive(Default)]
struct SinkSlot {
    toasts: Option<Entity<ToastQueue>>,
    /// Toasts pushed while the window was still being built, before its queue existed.
    early: Vec<EarlyToast>,
}

type EarlyToast = (
    ToastKind,
    SharedString,
    Option<ToastKey>,
    Option<ToastAction>,
);

impl Global for SinkSlot {}

/// Points `ShellApi` at the sink: main toasts push to its queue. The other calls keep recording
/// in `ShellLog`.
fn install_sink_api(cx: &mut App) {
    let mut api = cx.remove_global::<ShellApi>();
    api.toast = |cx, kind, text, key, action| {
        if let Some(queue) = cx.global::<SinkSlot>().toasts.clone() {
            queue.update(cx, |q, cx| q.push(kind, text, key, action, cx));
        } else {
            cx.global_mut::<SinkSlot>()
                .early
                .push((kind, text, key, action));
        }
    };
    api.main_toasts = |cx| cx.global::<SinkSlot>().toasts.clone().map(Into::into);
    cx.set_global(api);
}

/// A window with one page, in place of `MainWindow` on that page's tab.
struct PageView<P: Render + 'static> {
    page: Entity<P>,
}

impl<P: Render + 'static> Render for PageView<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Last: an open context menu covers the window, as in `MainWindow`.
        let menu = context_menu_layer(window, cx);
        div()
            .size_full()
            .key_context(GLOBAL_CONTEXT)
            .child(self.page.clone())
            .children(menu)
    }
}

/// Opens a window at 1200 x 800 px that holds the page `build` makes and owns the main toast
/// queue. Call it after [`shell_free_init`]. Keep the returned context: it drives the window.
pub fn open_page<P: Render + 'static>(
    cx: &mut TestAppContext,
    host: &PageHost,
    build: impl FnOnce(&HostContext, &mut Window, &mut Context<P>) -> P,
) -> (Entity<P>, VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(SinkSlot::default());
        install_sink_api(cx);
    });
    let local = cx.update(|cx| HostContext::local(cx));
    let slot: Rc<RefCell<Option<Entity<P>>>> = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    let (_, vcx) = host.open(cx, size(px(1200.), px(800.)), move |window, cx| {
        let page = cx.new(|cx| build(&local, window, cx));
        *keep.borrow_mut() = Some(page.clone());
        let view = cx.new(|_| PageView { page });
        Root::new(view, window, cx)
    });
    let mut vcx = vcx.clone();
    let page = slot.borrow().clone().expect("the page is built");
    vcx.update(|window, cx| {
        window.activate_window();
        let toasts = cx.new(|cx| ToastQueue::new(window, cx, px(64.)));
        let slot = cx.global_mut::<SinkSlot>();
        slot.toasts = Some(toasts.clone());
        let early = std::mem::take(&mut slot.early);
        toasts.update(cx, |q, cx| {
            for (kind, text, key, action) in early {
                q.push(kind, text, key, action, cx);
            }
        });
    });
    vcx.run_until_parked();
    (page, vcx)
}

/// [`open_page`] with an empty page: only the main toast queue.
pub fn open_sink(cx: &mut TestAppContext, host: &PageHost) -> VisualTestContext {
    open_page(cx, host, |_, _, _| Empty).1
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The texts of the sink's toast queue, the visible ones first and then the waiting ones.
pub fn sink_toasts(cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let queue = cx
            .global::<SinkSlot>()
            .toasts
            .clone()
            .expect("the sink window is open");
        let queue = queue.read(cx);
        queue
            .visible_texts()
            .into_iter()
            .chain(queue.waiting_texts())
            .map(|t| t.to_string())
            .collect()
    })
}
