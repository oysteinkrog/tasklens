//! UI-PRF-062, D-042: the CPU by app chart, `AppsChart` of kind `AppsMetric::Cpu`, in an
//! Overview body. The stack sums to busy CPU, the top N groups get their own band, a system band
//! that is 0 over the range is hidden, a group takes the colour of its memory pick, and the legend
//! shows percent and opens Processes for a group row only.

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseUpEvent, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::MemGroupBand;
use tl_model::time::now_boot_ns;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_overview::overview::memory_apps::{
    APPS_ID, AppsChart, AppsMetric, AppsPlace, BandKind, CPU_BANDS, LEGEND_ID,
};
use tl_ui_settings::settings::{
    AppearanceSettings, GroupColor, OverviewRange, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

const S: u64 = 1_000_000_000;
const STEPS: u64 = 60;
const OTHER: DeviceIdx = DeviceIdx(80);

fn group_dev(i: u16) -> DeviceIdx {
    DeviceIdx(10 + i)
}

/// The device of system band `i` of [`CPU_BANDS`].
fn band_dev(i: usize) -> DeviceIdx {
    DeviceIdx(81 + i as u16)
}

fn sid(device: DeviceIdx) -> SeriesId {
    SeriesId {
        device,
        metric: MetricId::CPU_GROUP,
    }
}

fn mem_group(idx: DeviceIdx, key: &str, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx,
        identity: DeviceIdentity::MemGroup {
            key: Arc::from(key),
        },
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(format!("memgroup:{key}")),
        net_kind: None,
        hot_c: None,
    }
}

/// One band of the fixture: its key, device and samples, one a second, the newest now.
struct Line {
    key: String,
    name: String,
    device: DeviceIdx,
    samples: Vec<Sample>,
}

/// The CPU_GROUP series of one sweep script.
struct Fixture {
    base: u64,
    groups: Vec<Line>,
    other: Line,
    bands: Vec<Line>,
}

impl Fixture {
    /// Groups `exe:p<i>` of `group(i, k)` percent at step `k`, "Other apps" of `other(k)` and
    /// the five system bands of `band(b, k)`, step `k` at `base + k` seconds.
    fn new(
        base: u64,
        groups: u16,
        group: impl Fn(u16, u64) -> Option<Sample>,
        other: impl Fn(u64) -> f32,
        band: impl Fn(usize, u64) -> f32,
    ) -> Self {
        let t = |k: u64| base + k * S;
        let line = |key: String, device: DeviceIdx, value: &dyn Fn(u64) -> Option<Sample>| Line {
            name: key.rsplit(':').next().unwrap_or_default().to_owned(),
            key,
            device,
            samples: (0..STEPS).filter_map(value).collect(),
        };
        Self {
            base,
            groups: (0..groups)
                .map(|i| line(format!("exe:p{i}"), group_dev(i), &|k| group(i, k)))
                .collect(),
            other: line("other".into(), OTHER, &|k| {
                Some(Sample::valid(t(k), other(k)))
            }),
            bands: (0..CPU_BANDS.len())
                .map(|b| {
                    line(CPU_BANDS[b].0.into(), band_dev(b), &|k| {
                        Some(Sample::valid(t(k), band(b, k)))
                    })
                })
                .collect(),
        }
    }

    fn t(&self, k: u64) -> u64 {
        self.base + k * S
    }

    /// Every line's value at step `k`, a gap or a missing sample as 0: busy CPU.
    fn busy(&self, k: u64) -> f64 {
        let t = self.t(k);
        self.groups
            .iter()
            .chain([&self.other])
            .chain(&self.bands)
            .filter_map(|l| l.samples.iter().find(|s| s.t_ns == t))
            .filter(|s| s.is_valid())
            .map(|s| f64::from(s.value))
            .sum()
    }

    fn publish(&self, engine: &FakeEngine) {
        engine.publish_with(|s| {
            let mut entries = Vec::new();
            let mut views = Vec::new();
            let mut published = Vec::new();
            for l in &self.groups {
                let name = l.name.clone();
                entries.push(mem_group(l.device, &l.key, &name));
                published.push(MemGroupBand {
                    key: Arc::from(l.key.as_str()),
                    name: Arc::from(name),
                    process_count: 3,
                    device: l.device,
                    wine: None,
                });
            }
            entries.push(mem_group(OTHER, "other", "Other apps"));
            for (b, l) in self.bands.iter().enumerate() {
                // The catalog name is the engine's en-US name; the chart shows its own word.
                entries.push(mem_group(l.device, &l.key, &format!("engine {b}")));
            }
            for l in self.groups.iter().chain([&self.other]).chain(&self.bands) {
                views.push(Arc::new(SeriesView {
                    id: sid(l.device),
                    version: 1,
                    period_ns: S,
                    samples: l.samples.clone().into(),
                }));
            }
            views.sort_by_key(|v| v.id);
            s.devices = Arc::new(DeviceCatalog {
                version: s.devices.version + 1,
                entries: entries.into(),
            });
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version: s.series.header.version + 1,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.cpu.groups = Arc::new(published);
        });
    }
}

/// A flat fixture: group `exe:p<i>` holds `groups[i]` percent, "Other apps" 1.5% and the
/// system bands `bands`.
fn flat(groups: &[f32], bands: [f32; 5]) -> Fixture {
    let base = now_boot_ns() - (STEPS - 1) * S;
    let groups = groups.to_vec();
    Fixture::new(
        base,
        groups.len() as u16,
        |i, k| Some(Sample::valid(base + k * S, groups[usize::from(i)])),
        |_| 1.5,
        |b, _| bands[b],
    )
}

struct Host {
    apps: Entity<AppsChart>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(700.)).h(px(400.)).child(self.apps.clone()))
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

struct Rig<'a> {
    apps: Entity<AppsChart>,
    vcx: &'a mut VisualTestContext,
}

impl Rig<'_> {
    fn bands(&mut self) -> Vec<(String, BandKind, String, f64)> {
        self.vcx.read_entity(&self.apps, |a, _| {
            a.bands()
                .iter()
                .map(|b| (b.key.to_string(), b.kind, b.name.to_string(), b.value))
                .collect()
        })
    }

    fn kinds(&mut self) -> Vec<BandKind> {
        self.bands().into_iter().map(|b| b.1).collect()
    }

    fn settings(&mut self) -> Settings {
        self.vcx.update(|_, cx| TlSettings::settings(cx).clone())
    }

    fn click(&mut self, id: String) {
        let at = self
            .vcx
            .debug_bounds(Box::leak(id.clone().into_boxed_str()))
            .expect(&id)
            .center();
        self.vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        self.vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        draw(self.vcx);
    }

    fn last_open(&mut self) -> Option<String> {
        self.vcx.read_entity(&self.apps, |a, _| {
            a.last_open.as_ref().map(|o| o.0.to_string())
        })
    }
}

/// A CPU by app chart in a 700 × 400 Overview body at the 1 min range over `fixture`, with
/// `settings` changed by `edit`.
fn open<'a>(
    cx: &'a mut TestAppContext,
    fixture: &Fixture,
    edit: impl FnOnce(&mut Settings),
) -> Rig<'a> {
    let engine = FakeEngine::new();
    fixture.publish(&engine);
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
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        settings.performance.overview_range = OverviewRange::M1;
        edit(&mut settings);
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let apps = cx.new(|cx| AppsChart::new_in(hub, AppsMetric::Cpu, AppsPlace::Overview, cx));
        Host { apps }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1000.), px(700.)));
    draw(vcx);
    let apps = vcx.update(|_, cx| host.read(cx).apps.clone());
    Rig { apps, vcx }
}

/// At every axis time the top of the built stack is busy CPU within 1e-3, while a group is
/// below the 0.05% floor (it folds into "Other apps"), a group arrives mid-range after a gap
/// sample, and "Short-lived processes" and "Steal" go to 0 and back.
#[gpui_kit::test]
fn cpu_by_app_stack_sums_to_total(cx: &mut TestAppContext) {
    let base = now_boot_ns() - (STEPS - 1) * S;
    let fixture = Fixture::new(
        base,
        6,
        |i, k| {
            let t = base + k * S;
            match i {
                // Below the floor: never ranked, still stacked.
                4 => Some(Sample::valid(t, 0.01)),
                // Arrives at step 30; the engine wrote a gap the step before.
                5 if k < 29 => None,
                5 if k == 29 => Some(Sample::gap(t)),
                5 => Some(Sample::valid(t, 7.25)),
                _ => Some(Sample::valid(
                    t,
                    f32::from(8 - i) * 2. + 0.125 * ((k + u64::from(i)) % 4) as f32,
                )),
            }
        },
        |k| 1.5 + 0.25 * (k % 3) as f32,
        |b, k| match b {
            0 => 0.75,
            1 => 0.125,
            2 => 0.375,
            3 => 0.,
            _ => 0.5 * (k % 2) as f32,
        },
    );
    let mut r = open(cx, &fixture, |s| s.performance.cpu_by_app.top_n = 3);

    let kinds = r.kinds();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| matches!(k, BandKind::Group(_)))
            .count(),
        3,
        "{kinds:?}"
    );
    let keys: Vec<String> = r.bands().into_iter().map(|b| b.0).collect();
    assert!(!keys.contains(&"exe:p4".to_owned()), "{keys:?}");
    assert!(!keys.contains(&"cpu:steal".to_owned()), "{keys:?}");
    for k in 0..STEPS {
        let t = fixture.t(k);
        let top = r.vcx.read_entity(&r.apps, |a, _| a.stack_top(t));
        let busy = fixture.busy(k);
        assert!(
            (top - busy).abs() < 1e-3,
            "step {k}: stack {top}, busy {busy}"
        );
    }
    // The legend values are the newest step, and so is the chart's own value.
    let sum: f64 = r.bands().iter().map(|b| b.3).sum();
    let busy = fixture.busy(STEPS - 1);
    assert!((sum - busy).abs() < 1e-3, "newest: {sum} vs {busy}");
}

/// The top N groups get their own band at 3, 10 and 20 groups shown; the rest fold into "Other
/// apps"; the five system bands follow and never count toward top N.
#[gpui_kit::test]
fn cpu_by_app_top_n(cx: &mut TestAppContext) {
    let groups: Vec<f32> = (0..25).map(|i| 30. - i as f32).collect();
    let fixture = flat(&groups, [0.5, 0.25, 0.75, 0.125, 0.375]);
    let mut r = open(cx, &fixture, |s| s.performance.cpu_by_app.top_n = 3);
    for n in [3usize, 10, 20] {
        if n != 3 {
            r.vcx.update(|_, cx| AppsMetric::Cpu.set_top_n(n as u8, cx));
            draw(r.vcx);
        }
        assert_eq!(usize::from(r.settings().performance.cpu_by_app.top_n), n);
        let kinds = r.kinds();
        let mut want = vec![BandKind::Other];
        want.extend((0..5u8).map(BandKind::Kernel));
        assert_eq!(kinds.len(), n + 6, "{n}: {kinds:?}");
        assert!(
            kinds[..n].iter().all(|k| matches!(k, BandKind::Group(_))),
            "{n}: {kinds:?}"
        );
        assert_eq!(kinds[n..], want[..], "{n}");
        let (bottom, other_series) = r.vcx.read_entity(&r.apps, |a, _| {
            (a.bands()[0].key.to_string(), a.bands()[n].series.len())
        });
        assert_eq!(bottom, "exe:p0", "{n}: largest first");
        assert_eq!(
            other_series,
            25 - n + 1,
            "{n}: the rest fold into Other apps"
        );
    }
    // The memory setting is separate.
    assert_eq!(r.settings().performance.memory_by_app.top_n, 10);
}

/// "Steal" is 0 over the range: it has no band and no legend row. The other system bands use
/// the localised names, and "Short-lived processes" says it is an estimate.
#[gpui_kit::test]
fn cpu_by_app_system_bands_hidden_when_zero(cx: &mut TestAppContext) {
    let fixture = flat(&[10., 5.], [0.5, 0.25, 0.75, 0., 0.375]);
    let mut r = open(cx, &fixture, |_| {});
    let bands = r.bands();
    let keys: Vec<&str> = bands.iter().map(|b| b.0.as_str()).collect();
    assert_eq!(
        keys,
        [
            "exe:p0",
            "exe:p1",
            "other",
            "cpu:kthreads",
            "cpu:irq",
            "cpu:softirq",
            "cpu:short"
        ]
    );
    let names: Vec<&str> = bands[3..].iter().map(|b| b.2.as_str()).collect();
    assert_eq!(
        names,
        [
            "Kernel threads",
            "Hardware interrupts",
            "Software interrupts",
            "Short-lived processes"
        ]
    );
    // One legend row per band, none for Steal.
    for row in 0..bands.len() {
        let id: &'static str = Box::leak(format!("{LEGEND_ID}-{row}").into_boxed_str());
        assert!(r.vcx.debug_bounds(id).is_some(), "row {row}");
    }
    let id: &'static str = Box::leak(format!("{LEGEND_ID}-{}", bands.len()).into_boxed_str());
    assert!(r.vcx.debug_bounds(id).is_none(), "no row past the bands");

    let short = r
        .vcx
        .update(|_, cx| r.apps.read(cx).tooltip_lines(6, None, cx));
    assert_eq!(
        short.last().map(String::as_str),
        Some("Estimate: CPU time of processes that ended before tasklens could give it to an app"),
        "{short:?}"
    );
    let kthreads = r
        .vcx
        .update(|_, cx| r.apps.read(cx).tooltip_lines(3, None, cx));
    assert!(
        !kthreads.iter().any(|l| l.starts_with("Estimate")),
        "{kthreads:?}"
    );
}

/// A group with a memory pick and no CPU pick takes that slot when it is free; one whose memory
/// slot a CPU pick holds does not. The CPU picks are saved, the memory picks never change.
#[gpui_kit::test]
fn cpu_by_app_colour_from_memory_pick(cx: &mut TestAppContext) {
    let pick = |key: &str, slot: u8| GroupColor {
        key: key.into(),
        slot,
    };
    let memory = vec![pick("exe:p1", 7), pick("exe:p0", 3)];
    let fixture = flat(&[10., 8., 6.], [0.5; 5]);
    let seeded = memory.clone();
    let mut r = open(cx, &fixture, move |s| {
        s.performance.memory_by_app.colors = seeded;
        s.performance.cpu_by_app.colors = vec![pick("exe:p2", 3)];
    });
    let slot = |r: &mut Rig, key: &str| {
        r.bands()
            .into_iter()
            .find(|b| b.0 == key)
            .map(|b| b.1)
            .expect(key)
    };
    assert_eq!(
        slot(&mut r, "exe:p1"),
        BandKind::Group(7),
        "the memory pick"
    );
    assert_eq!(
        slot(&mut r, "exe:p2"),
        BandKind::Group(3),
        "its own CPU pick"
    );
    assert_ne!(
        slot(&mut r, "exe:p0"),
        BandKind::Group(3),
        "slot 3 is taken by a CPU pick"
    );
    let settings = r.settings();
    assert_eq!(settings.performance.memory_by_app.colors, memory);
    let cpu = &settings.performance.cpu_by_app.colors;
    assert!(cpu.contains(&pick("exe:p1", 7)), "{cpu:?}");
    assert!(cpu.contains(&pick("exe:p2", 3)), "{cpu:?}");
}

/// The legend and the tooltip show percent with one decimal, and the tooltip's share is of busy
/// CPU.
#[gpui_kit::test]
fn cpu_by_app_legend_percent(cx: &mut TestAppContext) {
    let fixture = flat(&[12.34, 2.], [0.5, 0.25, 0.75, 0.125, 0.375]);
    let r = open(cx, &fixture, |_| {});
    let lines = r
        .vcx
        .update(|_, cx| r.apps.read(cx).tooltip_lines(0, None, cx));
    assert_eq!(lines[0], "p0");
    assert_eq!(lines[1], "12.3%");
    // 12.34 of 12.34 + 2 + 1.5 + 2 = 17.84 busy.
    assert_eq!(lines[2], "69.2% of busy CPU");
    assert_eq!(lines[3], "3 processes");
    let other = r
        .vcx
        .update(|_, cx| r.apps.read(cx).tooltip_lines(2, None, cx));
    assert_eq!(other[1], "1.5%");
}

/// A group's legend row opens Processes filtered to that group; "Other apps" and the system rows
/// open nothing.
#[gpui_kit::test]
fn cpu_by_app_legend_click(cx: &mut TestAppContext) {
    let fixture = flat(&[8., 6., 4., 2.], [0.5, 0.25, 0.75, 0.125, 0.375]);
    let mut r = open(cx, &fixture, |_| {});
    // Rows: Other, p3, p2, p1, p0, then the system bands top first under the divider.
    r.click(format!("{LEGEND_ID}-0"));
    assert_eq!(r.last_open(), None);
    for row in 5..10 {
        r.click(format!("{LEGEND_ID}-{row}"));
        assert_eq!(r.last_open(), None, "system row {row}");
    }
    r.click(format!("{LEGEND_ID}-1"));
    assert_eq!(r.last_open().as_deref(), Some("exe:p3"));
    let name = r.vcx.read_entity(&r.apps, |a, _| {
        a.last_open.as_ref().map(|o| o.1.to_string())
    });
    assert_eq!(name.as_deref(), Some("p3"));
    r.click(format!("{LEGEND_ID}-4"));
    assert_eq!(r.last_open().as_deref(), Some("exe:p0"));
}

/// Two groups with one name get a suffix from their kind; an app keeps its plain name, and the
/// Processes chip gets the same name (tasklens-02dhy).
#[gpui_kit::test]
fn cpu_by_app_same_name_gets_kind_suffix(cx: &mut TestAppContext) {
    let mut fixture = flat(&[8., 6., 4.], [0.5, 0.25, 0.75, 0.125, 0.375]);
    fixture.groups[0].key = "app:io.x.Tasklens".into();
    fixture.groups[0].name = "tasklens".into();
    fixture.groups[1].key = "exe:tasklens".into();
    fixture.groups[1].name = "tasklens".into();
    let mut r = open(cx, &fixture, |_| {});
    let name_of = |r: &mut Rig<'_>, key: &str| {
        r.bands()
            .into_iter()
            .find(|b| b.0 == key)
            .map(|b| b.2)
            .expect(key)
    };
    assert_eq!(name_of(&mut r, "app:io.x.Tasklens"), "tasklens");
    assert_eq!(name_of(&mut r, "exe:tasklens"), "tasklens (program)");
    assert_eq!(name_of(&mut r, "exe:p2"), "p2");
    // Rows: Other, p2, the program, the app.
    r.click(format!("{LEGEND_ID}-2"));
    assert_eq!(r.last_open().as_deref(), Some("exe:tasklens"));
    let name = r.vcx.read_entity(&r.apps, |a, _| {
        a.last_open.as_ref().map(|o| o.1.to_string())
    });
    assert_eq!(name.as_deref(), Some("tasklens (program)"));
}

/// docs/ui/performance.md "hidden on mouse leave": the window sends a `MouseExitEvent` and no
/// move when the pointer leaves it, and the hover goes with it.
#[gpui_kit::test]
fn cpu_by_app_hover_clears_on_exit(cx: &mut TestAppContext) {
    let r = open(
        cx,
        &flat(&[8., 6.], [0.5, 0.25, 0.75, 0.125, 0.375]),
        |_| {},
    );
    let b = r.vcx.debug_bounds(APPS_ID).expect("the chart");
    let at = gpui_kit::point(b.center().x, b.bottom() - px(4.));
    r.vcx.simulate_mouse_move(at, None, Modifiers::none());
    draw(r.vcx);
    assert!(r.vcx.read_entity(&r.apps, |a, _| a.hovered()).is_some());
    r.vcx.simulate_event(MouseExitEvent {
        position: at,
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
    draw(r.vcx);
    assert_eq!(r.vcx.read_entity(&r.apps, |a, _| a.hovered()), None);
}
