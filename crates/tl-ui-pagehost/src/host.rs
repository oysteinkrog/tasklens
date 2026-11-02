//! The page host: globals in the order the shell's `TasklensUi::init` and the test rigs install
//! them, one window with the test's root view, and a `ShellApi` that records instead of acting.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, Pixels, Render, SharedString, Size,
    TestAppContext, VisualTestContext, Window,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_ui_base::app_env::{AppEnv, BootSettings, EngineGlobal};
use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::registry::GlobalRegistry;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::shell::{DiskEject, ProcessJump, ShellApi};
use tl_ui_base::theme;
use tl_ui_base::toast_types::{ToastKey, ToastKind};
use tl_ui_kit::components::kv_grid;
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

/// The strings every page host installs, so a test does not depend on the system locale.
pub const LANGUAGE: &str = "en-US";

/// Installs the kit and the default theme with its metrics. Component tests that need no
/// settings, strings or engine use this alone.
pub fn install_theme(cx: &mut App) {
    gpui_kit::init(cx);
    theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
}

/// One toast a page pushed through `ShellApi::push_toast`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushedToast {
    pub kind: ToastKind,
    pub text: SharedString,
    pub key: Option<ToastKey>,
    /// The action button's label, when the toast has one.
    pub action: Option<SharedString>,
}

/// What pages asked the shell for, in call order. The page host's `ShellApi` writes it; a test
/// reads it with [`ShellLog::get`].
#[derive(Default)]
pub struct ShellLog {
    pub toasts: Vec<PushedToast>,
    pub tabs: Vec<Tab>,
    pub group_chips: Vec<Option<SharedString>>,
    /// Each `note_restricted` count. The host has no status chip, so the call returns false.
    pub restricted: Vec<u32>,
    /// Public IP fetches asked for. The host drops the fetch, as the app does without a shell.
    pub public_ip_fetches: u32,
    pub process_jumps: Vec<ProcessJump>,
    pub disk_ejects: Vec<DiskEject>,
}

impl Global for ShellLog {}

impl ShellLog {
    /// The log of the installed page host.
    pub fn get(cx: &App) -> &ShellLog {
        cx.global::<ShellLog>()
    }

    fn write(cx: &mut App) -> &mut ShellLog {
        cx.default_global::<ShellLog>()
    }
}

/// A `ShellApi` with no main window: each call is written to [`ShellLog`], and the queries
/// return what they return without a main window.
fn recording_shell() -> ShellApi {
    ShellApi {
        toast: |cx, kind, text, key, action| {
            ShellLog::write(cx).toasts.push(PushedToast {
                kind,
                text,
                key,
                action: action.map(|a| a.label),
            });
        },
        main_toasts: |_| None,
        select_tab: |tab, _, cx| ShellLog::write(cx).tabs.push(tab),
        page: |_, _| None,
        filter_text: |_, _| None,
        set_group_chip: |text, cx| ShellLog::write(cx).group_chips.push(text),
        note_restricted: |count, cx| {
            ShellLog::write(cx).restricted.push(count);
            false
        },
        public_ip_fetch: |_, cx, _| ShellLog::write(cx).public_ip_fetches += 1,
        process_jump: |_, cx, jump| ShellLog::write(cx).process_jumps.push(jump),
        disk_eject: |cx, eject| ShellLog::write(cx).disk_ejects.push(eject),
    }
}

/// The globals of one page on a [`FakeEngine`]. Build it with [`PageHost::install`], then open
/// the page with [`PageHost::open`].
pub struct PageHost {
    pub engine: Arc<FakeEngine>,
    pub client: Arc<dyn EngineClient>,
    pub hub: Entity<SnapshotHub>,
    pub demand: Entity<DemandAggregator>,
}

impl PageHost {
    /// Installs the kit, the theme from `settings` with its metrics, the [`LANGUAGE`] strings,
    /// the key/value grid keys, the engine on `engine`, `settings` with the default state as
    /// `TlSettings`, the hub and demand (created after the settings, as the test rigs did), the
    /// window registry, `AppEnv::test()` and the recording `ShellApi`.
    ///
    /// Publish the snapshot the page should start from before this call: the hub reads it here.
    pub fn install(cx: &mut TestAppContext, engine: Arc<FakeEngine>, settings: Settings) -> Self {
        cx.update(|cx| Self::install_in(cx, engine, settings))
    }

    /// [`PageHost::install`] on an `App`.
    pub fn install_in(cx: &mut App, engine: Arc<FakeEngine>, settings: Settings) -> Self {
        let client: Arc<dyn EngineClient> = engine.clone();
        gpui_kit::init(cx);
        theme::store::init(cx, &settings.theme, &settings.appearance, Vec::new());
        theme::metrics::init_metrics(&settings.appearance, cx);
        init_strings(cx, LANGUAGE);
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
        cx.set_global(GlobalHub(hub.clone()));
        cx.set_global(GlobalDemand(demand.clone()));
        GlobalRegistry::init(cx);
        cx.set_global(AppEnv::test());
        cx.set_global(ShellLog::default());
        cx.set_global(recording_shell());
        Self {
            engine,
            client,
            hub,
            demand,
        }
    }

    /// The local host as a page gets it: the client, hub and demand, with no action tracker and
    /// no shell events.
    pub fn host(&self) -> HostContext {
        HostContext::from_parts(
            Some(Arc::clone(&self.client)),
            Some(self.hub.clone()),
            Some(self.demand.clone()),
            None,
            None,
        )
    }

    /// Opens one window whose root view `build` makes, at scale factor 1 and `size`. Nothing is
    /// drawn yet.
    pub fn open<'a, V: Render + 'static>(
        &self,
        cx: &'a mut TestAppContext,
        size: Size<Pixels>,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> (Entity<V>, &'a mut VisualTestContext) {
        let (view, vcx) = cx.add_window_view(build);
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size);
        (view, vcx)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{IntoElement, div, px, size};
    use tl_ui_base::shell::ShellApi;

    use super::*;

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn shell_calls_are_recorded(cx: &mut TestAppContext) {
        let host = PageHost::install(cx, FakeEngine::new(), Settings::default());
        let (_, vcx) = host.open(cx, size(px(400.), px(300.)), |_, _| Empty);
        vcx.update(|window, cx| {
            ShellApi::push_toast(cx, ToastKind::Warning, "Disk full", Some(ToastKey(7)), None);
            ShellApi::select_tab(Tab::Services, window, cx);
            assert!(!ShellApi::note_restricted(3, cx), "no status chip");
            assert!(ShellApi::main_toasts(cx).is_none());
        });
        let host_hub = host.host();
        vcx.update(|_, cx| {
            let log = ShellLog::get(cx);
            assert_eq!(
                log.toasts,
                vec![PushedToast {
                    kind: ToastKind::Warning,
                    text: "Disk full".into(),
                    key: Some(ToastKey(7)),
                    action: None,
                }]
            );
            assert_eq!(log.tabs, vec![Tab::Services]);
            assert_eq!(log.restricted, vec![3]);
            let local = HostContext::local(cx);
            assert_eq!(local.hub(), host_hub.hub(), "the hub is the global one");
            assert_eq!(TlSettings::settings(cx), &Settings::default());
        });
    }
}
