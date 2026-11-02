//! The setup the process tests share. `TasklensUi::init` installs more than `PageHost::install`
//! does; [`shell_free_init`] adds the part a process page or window needs, without a main window.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyView, AnyWeakEntity, App, AppContext as _, Context, Entity, Global, InteractiveElement as _,
    IntoElement, ModifiersChangedEvent, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use tl_model::command::Tab;
use tl_ui_base::app_env::AppEnv;
use tl_ui_base::host::HostContext;
use tl_ui_base::shell::{
    GlobalShellEvents, PauseChanged, ShellApi, ShellEvents, ShellPage as _, ShellState,
};
use tl_ui_base::{action_tracker, request_ends};
use tl_ui_kit::actions::GLOBAL_CONTEXT;
use tl_ui_kit::components::menu::context_menu_layer;
use tl_ui_kit::components::toast;
use tl_ui_kit::components::toast::ToastQueue;
use tl_ui_kit::{actions, helper_flow};
use tl_ui_pagehost::PageHost;
use tl_ui_procs::pages::processes::ProcessesPage;
use tl_ui_procs::pages::processes::bind_keys_processes;
use tl_ui_procs::pages::processes::history::actions::bind_keys_history;
use tl_ui_procs::windows::actions::bind_keys_process_windows;
use tl_ui_procs::windows::handle_search;
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_table::table::actions::bind_keys_table;
use tl_ui_testkit::fake_engine::FakeEngine;

/// What `TasklensUi::init` installs for the process windows and the Processes page, on top of
/// the page host: the toast queues, the key bindings, the action tracker, the request ends, the
/// helper flow and the handle search open hook.
pub fn install_shell_services(host: &PageHost, cx: &mut App) {
    actions::bind_keys_global(cx);
    bind_keys_table(cx);
    bind_keys_process_windows(cx);
    bind_keys_processes(cx);
    bind_keys_history(cx);
    actions::register_global_handlers(cx);
    toast::init(cx);
    action_tracker::install(Arc::clone(&host.client), Some(host.hub.clone()), cx);
    request_ends::install(Arc::clone(&host.client), cx);
    helper_flow::install(cx);
    handle_search::init(cx);
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
    let host = PageHost::install_with(cx, Arc::clone(&engine), settings, state, env);
    cx.update(|cx| {
        install_shell_services(&host, cx);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    (engine, host)
}

/// The stand-in for the main window's toast queue and its Processes page: [`open_stand_in`] fills
/// it, and the `ShellApi` calls that name the main window read it.
#[derive(Default)]
struct StandInSlot {
    toasts: Option<Entity<ToastQueue>>,
    page: Option<AnyView>,
}

impl Global for StandInSlot {}

/// A window with the real Processes page, in place of `MainWindow` on the Processes tab.
struct StandInView {
    page: Entity<ProcessesPage>,
}

impl StandInView {
    /// Ctrl-pause as `MainWindow` runs it (UI-SHL-123): starts when Ctrl goes down with no other
    /// modifier while the window is active, and ends when Ctrl goes up. A modifier added during
    /// the pause keeps it.
    fn on_modifiers(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = ev.modifiers;
        let paused = cx.try_global::<ShellState>().is_some_and(|s| s.paused);
        let want = if paused {
            m.control
        } else {
            m.control
                && !m.shift
                && !m.alt
                && !m.platform
                && !m.function
                && window.is_window_active()
        };
        if paused == want {
            return;
        }
        cx.default_global::<ShellState>().paused = want;
        if let Some(events) = HostContext::local(cx).events().cloned() {
            events.update(cx, |_, cx| cx.emit(PauseChanged(want)));
        }
    }
}

impl Render for StandInView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Last: an open context menu covers the window, as in `MainWindow`.
        let menu = context_menu_layer(window, cx);
        div()
            .size_full()
            .key_context(GLOBAL_CONTEXT)
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .child(self.page.clone())
            .children(menu)
    }
}

/// What a test reads from the stand-in window. The toast queue of that window is the main toast
/// queue, so repeats merge as in the app.
#[derive(Clone)]
pub struct StandIn {
    pub page: Entity<ProcessesPage>,
    pub toasts: Entity<ToastQueue>,
}

/// Points `ShellApi` at the stand-in: main toasts push to its queue, and `page` returns its
/// Processes page. The other calls keep recording in `ShellLog`.
fn install_stand_in_api(cx: &mut App) {
    let mut api = cx.remove_global::<ShellApi>();
    api.toast = |cx, kind, text, key, action| {
        if let Some(queue) = cx.global::<StandInSlot>().toasts.clone() {
            queue.update(cx, |q, cx| q.push(kind, text, key, action, cx));
        }
    };
    api.main_toasts = |cx| cx.global::<StandInSlot>().toasts.clone().map(Into::into);
    api.page = |cx, tab| {
        (tab == Tab::Processes)
            .then(|| cx.global::<StandInSlot>().page.clone())
            .flatten()
    };
    cx.set_global(api);
}

/// Opens the stand-in window at 1200 x 800 px on `host`, in a kit `Root` like the main window.
/// Call it after [`shell_free_init`]. Keep the returned context: it drives the window.
pub fn open_stand_in(cx: &mut TestAppContext, host: &PageHost) -> (StandIn, VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(StandInSlot::default());
        install_stand_in_api(cx);
    });
    let local = cx.update(|cx| HostContext::local(cx));
    let slot = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    let view_slot: Rc<RefCell<Option<AnyWeakEntity>>> = Rc::new(RefCell::new(None));
    let keep_view = Rc::clone(&view_slot);
    let (_, vcx) = host.open(cx, size(px(1200.), px(800.)), move |window, cx| {
        let page = cx.new(|cx| ProcessesPage::new(&local, window, cx));
        *keep.borrow_mut() = Some(page.clone());
        let view = cx.new(|_| StandInView { page });
        *keep_view.borrow_mut() = Some(view.downgrade().into());
        Root::new(view, window, cx)
    });
    let mut vcx = vcx.clone();
    let page: Entity<ProcessesPage> = slot.borrow().clone().expect("the Processes page is built");
    let toasts = vcx.update(|window, cx| {
        window.activate_window();
        let toasts = cx.new(|cx| ToastQueue::new(window, cx, px(64.)));
        // The main window shows the Processes tab, so the page is the active one.
        page.update(cx, |p, cx| p.set_active(true, cx));
        // The main window gives the shown page its primary focus.
        let focus = page.read(cx).primary_focus();
        window.focus(&focus, cx);
        let slot = cx.global_mut::<StandInSlot>();
        slot.toasts = Some(toasts.clone());
        slot.page = Some(page.clone().into());
        let state = cx.default_global::<ShellState>();
        state.main_window = window.window_handle().downcast::<Root>();
        // `ShellState::main_view_open` is true while the stand-in lives, as for the main window.
        state.main_view = view_slot.borrow().clone();
        toasts
    });
    vcx.run_until_parked();
    (StandIn { page, toasts }, vcx)
}

/// The texts of the stand-in window's toast queue, the visible ones first and then the waiting
/// ones.
pub fn stand_in_toasts(cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let queue = cx
            .global::<StandInSlot>()
            .toasts
            .clone()
            .expect("the stand-in window is open");
        let queue = queue.read(cx);
        queue
            .visible_texts()
            .into_iter()
            .chain(queue.waiting_texts())
            .map(|t| t.to_string())
            .collect()
    })
}

/// The open stand-in window's page and toast queue.
pub fn stand_in_of(cx: &App) -> StandIn {
    let slot = cx.global::<StandInSlot>();
    StandIn {
        page: slot
            .page
            .clone()
            .and_then(|view| view.downcast().ok())
            .expect("the stand-in window is open"),
        toasts: slot.toasts.clone().expect("the stand-in window is open"),
    }
}
