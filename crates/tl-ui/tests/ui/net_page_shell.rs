//! The Network page's Public IP tests that click Fetch. The click goes through the shell's
//! public IP service (`tl_ui::window::main::install_shell_api`): it opens the notice window or
//! sends the command. The other Network page tests are in tl-ui-perf:
//! `cargo test -p tl-ui-perf --features test-hooks --test perf -- net_page::`.

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, DeviceCommand};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{PublicIpState, SystemSnapshot};
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::kv_grid;
use tl_ui_kit::components::menu::context_menu_layer;
use tl_ui_perf::performance::net::{NetPage, NetSettingsDesktop};
use tl_ui_perf::performance::page::DevicePage as _;
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

/// The interface in the fake catalog.
const NET: DeviceIdx = DeviceIdx(1);

/// The page in a window with an animation driver: a live chart asks it for frames.
struct Host {
    page: Entity<NetPage>,
    _driver: Entity<AnimationDriver>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(1000.))
            .h(px(2000.))
            .child(self.page.clone())
            .children(context_menu_layer(window, cx))
    }
}

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("enp38s0"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

/// One interface up at `speed_mbps` (sysfs -1 gives Unsupported).
fn system(speed_mbps: i64) -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.version = 1;
    let link_speed_mbps = match u32::try_from(speed_mbps) {
        Ok(v) => Metric::valid(v),
        Err(_) => Metric::with(Validity::Unsupported),
    };
    sys.nets = Arc::from(
        [(
            NET,
            NetTotals {
                device: NET,
                rx_bps: Metric::valid(0.),
                tx_bps: Metric::valid(0.),
                rx_total: 0,
                tx_total: 0,
                link_speed_mbps,
                state: LinkState::Up,
            },
        )]
        .as_slice(),
    );
    sys
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<NetPage>,
    vcx: &'a mut VisualTestContext,
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// A Network page for `enp38s0` over `sys` in a 1000 px wide window, with `settings`.
fn open(cx: &mut TestAppContext, settings: Settings, sys: SystemSnapshot) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(
                    1,
                    DeviceIdentity::Net {
                        netns_ino: 1,
                        ifindex: 2,
                        name: Arc::from("enp38s0"),
                    },
                ),
            ]),
        });
        s.system = Arc::new(sys);
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        tl_ui_kit::actions::register_global_handlers(cx);
        tl_ui_kit::components::menu::bind_keys(cx);
        // No Network settings link, whatever XDG_CURRENT_DESKTOP the test runs under.
        cx.set_global(NetSettingsDesktop(None));
        // The public IP notice is a shell service the page reaches through ShellApi.
        tl_ui::window::main::install_shell_api(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    // Only EngineGlobal is installed here, so the host has the client and nothing else.
    let ctx = HostContext::from_parts(Some(Arc::clone(&client)), None, None, None, None);
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let key = DeviceKey(Arc::from("net:enp38s0"));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Net,
        idx: NET,
        generation: 0,
        net_class: None,
        fan_connected: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| NetPage::new(&ctx, key, row, h, d, cx));
    let host = page.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Host {
            page: host,
            _driver: driver,
        }
    });
    vcx.simulate_scale_factor_change(1.0);
    draw(vcx);
    Rig { engine, page, vcx }
}

mod public_ip {
    use super::*;

    const FETCH_BUTTON: &str = "TlButton-perf-net-public-ip-fetch";

    fn accepted(accepted: bool) -> Settings {
        let mut s = Settings::default();
        s.network.public_ip_notice_accepted = accepted;
        s
    }

    /// Local 14:02 on a fixed day, in seconds since the epoch, so the text is the same in every
    /// time zone.
    fn at_1402() -> i64 {
        jiff::civil::date(2026, 10, 16)
            .at(14, 2, 0, 0)
            .to_zoned(jiff::tz::TimeZone::system())
            .expect("local time")
            .timestamp()
            .as_second()
    }

    fn publish(r: &mut Rig<'_>, state: PublicIpState) {
        r.engine.publish_with(|s| {
            let inv = Arc::make_mut(&mut s.inventory);
            inv.header.version += 1;
            inv.public_ip = state;
        });
        draw(r.vcx);
    }

    fn fetched(v6: Option<&str>) -> PublicIpState {
        PublicIpState::Fetched {
            v4: Some(Arc::from("203.0.113.4")),
            v6: v6.map(Arc::from),
            fetched_at_s: at_1402(),
        }
    }

    /// The shown value and tooltip of the Public IP row.
    fn row(r: &mut Rig<'_>) -> (String, Option<String>) {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let found = page
                .read(cx)
                .details()
                .to_kv_rows()
                .find(|row| row.label.as_ref() == "Public IP")
                .expect("Public IP row");
            (
                found.value.text.to_string(),
                found.value.tooltip.map(|t| t.to_string()),
            )
        })
    }

    fn fetches(r: &Rig<'_>) -> usize {
        r.engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::Device(DeviceCommand::FetchPublicIp)))
            .count()
    }

    fn click_fetch(r: &mut Rig<'_>) {
        let button = r
            .vcx
            .debug_bounds(FETCH_BUTTON)
            .expect("Fetch button drawn");
        r.vcx.simulate_click(button.center(), Modifiers::none());
        draw(r.vcx);
    }

    #[gpui_kit::test]
    fn public_ip_states(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        assert_eq!(row(&mut r).0, "Not fetched");
        let value = r.vcx.debug_bounds("TlKv-value-15").expect("value drawn");
        let button = r.vcx.debug_bounds(FETCH_BUTTON).expect("Fetch button");
        assert!(button.left() > value.left(), "the button follows the text");
        assert_eq!(button.size.height, px(24.));

        publish(&mut r, PublicIpState::Fetching);
        assert_eq!(row(&mut r).0, "Fetching…");
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 0, "a loading button ignores clicks");

        publish(&mut r, fetched(None));
        assert_eq!(row(&mut r).0, "203.0.113.4 (fetched 14:02)");
        assert!(
            r.vcx.debug_bounds(FETCH_BUTTON).is_none(),
            "no button after a result"
        );
    }

    #[gpui_kit::test]
    fn public_ip_first_fetch_opens_notice(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(false), system(1000));
        let before = r.vcx.update(|_, cx| cx.windows().len());
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 0, "no command before Continue");
        let after = r.vcx.update(|_, cx| cx.windows().len());
        assert_eq!(after, before + 1, "the notice window opened");
    }

    #[gpui_kit::test]
    fn public_ip_accepted_sends_command(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 1, "the command went out at once");
        publish(&mut r, PublicIpState::Fetching);
        assert_eq!(row(&mut r).0, "Fetching…");
        assert!(
            r.vcx.debug_bounds(FETCH_BUTTON).is_some(),
            "the button shows its spinner"
        );
    }
}
