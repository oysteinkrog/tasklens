//! Host clocks (tasklens-q4x76.35, ADR-018 "Hosts"): every sample and snapshot time is on the
//! boot clock of the host that made it, so the UI reads "now" from that host's clock, never from
//! this PC's. Covers `HostClock`, a remote host's chart whose boot clock is hours ahead of or
//! behind this PC's, and a sweep that fails when UI code reads this PC's boot clock directly.
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::path::Path;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, div, px,
};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::Validity;
use tl_ui::chart::view::ChartView;
use tl_ui::chart::{ChartLabels, ChartSeries, ChartSpec, ChartState, ValueAlign, ValueFormat};
use tl_ui::hub::{HostClock, SnapshotHub};
use tl_ui::testing::fake_engine::FakeEngine;

const MS: u64 = 1_000_000;
const S: u64 = 1_000_000_000;
const HOUR: u64 = 3_600 * S;

#[test]
fn local_clock_is_this_pcs_boot_clock() {
    let clock = HostClock::local();
    assert!(!clock.is_remote());
    // A local clock learns nothing from snapshots.
    clock.observe_at(10 * HOUR, 2 * HOUR, true);
    assert_eq!(clock.offset_ns(), 0);
    assert_eq!(clock.now_at(5 * S), 5 * S);
}

#[test]
fn remote_clock_follows_a_host_hours_ahead_and_behind() {
    // This PC has been up 10 h. One host has been up 13 h, another 7 h.
    let local = 10 * HOUR;
    for host in [13 * HOUR, 7 * HOUR] {
        let clock = HostClock::remote();
        assert!(clock.is_remote());
        // Before the first published snapshot it reads this PC's clock.
        assert_eq!(clock.now_at(local), local);
        clock.observe_at(host, local, true);
        assert_eq!(clock.now_at(local), host);
        // The host clock moves with this PC's clock.
        assert_eq!(clock.now_at(local + 2 * S), host + 2 * S);
    }
}

#[test]
fn remote_clock_keeps_the_shortest_delay_and_moves_down_slowly() {
    let local = 10 * HOUR;
    let host = 13 * HOUR;
    let clock = HostClock::remote();
    // Installed 40 ms after it was published.
    clock.observe_at(host, local + 40 * MS, true);
    let first = clock.offset_ns();
    // A snapshot with a 5 ms delay is closer to the true offset: taken at once.
    clock.observe_at(host + S, local + S + 5 * MS, false);
    assert_eq!(clock.offset_ns(), first + 35 * MS as i64);
    // A snapshot 500 ms late moves the estimate down by 1 ms only.
    let before = clock.offset_ns();
    clock.observe_at(host + 2 * S, local + 2 * S + 500 * MS, false);
    assert_eq!(clock.offset_ns(), before - MS as i64);
}

#[test]
fn remote_clock_starts_over_on_reset_and_ignores_unpublished_snapshots() {
    let clock = HostClock::remote();
    clock.observe_at(13 * HOUR, 10 * HOUR, true);
    // A snapshot never published (`published_ns` 0, `Snapshot::empty`) teaches nothing.
    clock.observe_at(0, 10 * HOUR, true);
    assert_eq!(clock.now_at(10 * HOUR), 13 * HOUR);
    // The host rebooted: its clock is now 2 min, far below the old estimate.
    clock.observe_at(2 * 60 * S, 11 * HOUR, true);
    assert_eq!(clock.now_at(11 * HOUR), 2 * 60 * S);
    // Clones share the offset.
    let copy = clock.clone();
    clock.observe_at(3 * HOUR, 11 * HOUR, true);
    assert_eq!(copy.now_at(11 * HOUR), 3 * HOUR);
}

struct Root {
    chart: Entity<ChartView>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(602.)).h(px(202.)).child(self.chart.clone())
    }
}

fn cpu() -> SeriesId {
    SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    }
}

/// 600 samples 100 ms apart on the host's clock, the newest at `newest`.
fn host_series(newest: TimeNs) -> Arc<SeriesView> {
    let n = 600;
    let samples: Vec<Sample> = (0..n)
        .map(|i| Sample::valid(newest - (n - 1 - i) * 100 * MS, 20.0 + (i % 50) as f32))
        .collect();
    Arc::new(SeriesView {
        id: cpu(),
        version: 1,
        period_ns: 100 * MS,
        samples: samples.into(),
    })
}

/// A remote host whose boot clock reads `host_now` publishes one snapshot through its engine;
/// a CPU chart on that host's hub is drawn. Returns the chart's right edge, its state and value
/// validity at its own clock, and its built path count.
fn draw_remote_chart(
    cx: &mut TestAppContext,
    host_now: TimeNs,
) -> (TimeNs, ChartState, Option<Validity>, usize) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
    });
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.boot_id = BootId(0x5eed);
        s.published_ns = host_now;
        s.series = Arc::new(SeriesSet {
            header: ComponentHeader {
                version: 1,
                ..ComponentHeader::default()
            },
            series: Arc::from([host_series(host_now)]),
            slow: Arc::from([]),
        });
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    let spec = ChartSpec {
        series: smallvec![ChartSeries {
            id: cpu(),
            line: gpui_kit::hsla(0.5, 0.6, 0.5, 1.0),
            ..ChartSeries::default()
        }],
        ..ChartSpec::default()
    };
    let labels = ChartLabels {
        heading: Some("CPU".into()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: "Utilization".into(),
        legend: smallvec![],
        show_axis_max: true,
    };
    let (root, vcx) = cx.add_window_view(|_, cx| {
        let hub = cx.new(|cx| SnapshotHub::new_remote(client, cx));
        let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
        Root { chart }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let chart = vcx.read_entity(&root, |r, _| r.chart.clone());
    vcx.read_entity(&chart, |v, _| {
        let now = v.now_ns();
        (
            v.last_right_edge_ns.get(),
            v.derived_state(now),
            v.value_now().map(|(_, validity)| validity),
            v.cached_path_count(),
        )
    })
}

/// The chart's right edge sits at the host's now, the newest samples are in view, and the
/// chart is Live with a Valid value.
fn assert_points(host_now: TimeNs, got: (TimeNs, ChartState, Option<Validity>, usize)) {
    let (right_edge, state, validity, paths) = got;
    assert!(
        right_edge.abs_diff(host_now) < 5 * S,
        "right edge {right_edge} is not near the host's now {host_now}"
    );
    assert_eq!(state, ChartState::Live);
    assert_eq!(validity, Some(Validity::Valid));
    assert!(paths > 0, "the chart built no paths");
}

#[gpui_kit::test]
fn remote_host_hours_ahead_draws_points(cx: &mut TestAppContext) {
    let host_now = now_boot_ns() + 3 * HOUR;
    let got = draw_remote_chart(cx, host_now);
    assert_points(host_now, got);
}

#[gpui_kit::test]
fn remote_host_hours_behind_draws_points(cx: &mut TestAppContext) {
    // Three hours behind this PC; a host up only 2 min when this PC itself is younger than that.
    let host_now = now_boot_ns().saturating_sub(3 * HOUR).max(120 * S);
    let got = draw_remote_chart(cx, host_now);
    assert_points(host_now, got);
}

/// Files that may read this PC's boot clock: the host clock itself, and the bench engine, which
/// publishes snapshots like an engine does.
const CLOCK_OWNERS: [&str; 2] = ["src/hub.rs", "src/bench/"];

#[test]
fn ui_reads_now_only_through_the_host_clock() {
    // Built from parts so the needles are plain text here.
    let needles = [
        concat!("now_boot_ns", "()"),
        concat!("ClockId::", "Boottime"),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read tl-ui source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .expect("under the crate")
                .to_string_lossy()
                .replace('\\', "/");
            if CLOCK_OWNERS.iter().any(|owner| rel.starts_with(owner)) {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source file");
            for (i, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if needles.iter().any(|n| line.contains(n)) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "UI code compares with this PC's boot clock; use the host's clock \
         (SnapshotHub::now_ns, HostContext::clock or ChartView::now_ns) instead:\n{}",
        hits.join("\n")
    );
}
