//! CPU grid hover (tasklens-yrf.21), split from `chart.rs` when the chart tests moved to
//! tl-ui-chart (tasklens-zljr5.36): `CpuGrid` lives in tl-ui-perf, which tl-ui-chart cannot use.

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, IntoElement, Modifiers, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, point, px,
};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
use tl_model::time::now_boot_ns;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu_grid::CpuGrid;
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};
use tl_ui_testkit::fake_engine::FakeEngine;

const MS: u64 = 1_000_000;
const S: u64 = 1_000_000_000;

/// `n` logical CPUs on `n / 2` cores, all online.
fn topology(n: u16) -> CpuTopology {
    let half = n / 2;
    let mut online = CpuSet::empty();
    for id in 0..n {
        online.insert(id);
    }
    CpuTopology {
        online,
        cores: (0..half)
            .map(|c| CoreInfo {
                id: c,
                logical: smallvec![c, c + half],
                l3_group: 0,
                kind: CoreKind::Standard,
            })
            .collect(),
    }
}

/// 600 valid samples 100 ms apart on `device`, the newest at `newest`: 60 s of data.
fn view_of(device: DeviceIdx, newest: u64, value: f32) -> Arc<SeriesView> {
    let samples: Vec<Sample> = (0..600u64)
        .map(|k| Sample::valid(newest - (599 - k) * 100 * MS, value))
        .collect();
    Arc::new(SeriesView {
        id: SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        },
        version: 1,
        period_ns: 100 * MS,
        samples: samples.into(),
    })
}

struct Root {
    grid: CpuGrid,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1300.)).child(self.grid.element(window, cx))
    }
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

fn hover(vcx: &mut VisualTestContext, x: f32, y: f32) {
    vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
    vcx.run_until_parked();
    draw(vcx);
}

/// A 16-CPU grid in a 1300 px window at scale 1: 6 columns of 210 px with 8 px gaps, rows
/// of 112 px. Every cell has 60 s of data.
fn rig(cx: &mut TestAppContext) -> (gpui_kit::Entity<Root>, &mut VisualTestContext) {
    let grid = cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        let client: Arc<dyn EngineClient> = FakeEngine::new();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let theme = tl_ui::theme::current(cx).clone();
        let devices: Vec<DeviceIdx> = (0..16).map(|id| DeviceIdx(100 + id)).collect();
        CpuGrid::new(
            &hub,
            Arc::new(topology(16)),
            &devices,
            false,
            60 * S,
            false,
            &theme,
            cx,
        )
    });
    let (root, vcx) = cx.add_window_view(move |_, _| Root { grid });
    vcx.simulate_scale_factor_change(1.0);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(vcx);
    let view = vcx.read_entity(&root, |r, _| r.grid.view.clone());
    let newest = now_boot_ns();
    view.update(vcx, |v, cx| {
        let series = (0..16u16)
            .map(|i| (Some(view_of(DeviceIdx(100 + i), newest, 20.0)), None))
            .collect();
        v.set_series_for_test(series, cx);
    });
    draw(vcx);
    draw(vcx);
    (root, vcx)
}

#[gpui_kit::test]
fn cpu_grid_hover_on(cx: &mut TestAppContext) {
    let (root, vcx) = rig(cx);
    let view = vcx.read_entity(&root, |r, _| r.grid.view.clone());
    assert!(
        vcx.read_entity(&view, |v, _| v.spec.hover),
        "CPU grid hover on"
    );
}

#[gpui_kit::test]
fn cpu_grid_hover_box_flips_at_element_edge(cx: &mut TestAppContext) {
    let (root, vcx) = rig(cx);
    let view = vcx.read_entity(&root, |r, _| r.grid.view.clone());
    let builds = vcx.read_entity(&view, |v, _| v.build_count.get());

    // Cell 5 is the right-most column, 1090..1300: near the element's right edge the box
    // flips left of the marker and stays inside the element.
    hover(vcx, 1290., 60.);
    vcx.read_entity(&view, |v, _| {
        assert_eq!(v.hover_cell(), Some(5));
        let h = v.hover_state();
        let b = h.last_box.expect("box painted");
        let x = h.last_marker_x.expect("marker painted");
        assert!(b.right() < x, "flipped left: {b:?}, marker {x:?}");
        assert!(
            b.left() >= px(0.) && b.right() <= px(1300.),
            "inside: {b:?}"
        );
    });

    // Cell 4 (872..1082), near its own right edge: the box stays right of the marker and
    // passes the cell's edge, since the flip edge is the element's, not the cell plot's.
    hover(vcx, 1070., 60.);
    vcx.read_entity(&view, |v, _| {
        assert_eq!(v.hover_cell(), Some(4));
        let h = v.hover_state();
        let b = h.last_box.expect("box painted");
        assert!(b.left() > h.last_marker_x.unwrap(), "right of the marker");
        assert!(b.right() > px(1082.) && b.right() <= px(1300.), "{b:?}");
    });

    hover(vcx, 100., 500.);
    assert_eq!(
        vcx.read_entity(&view, |v, _| v.build_count.get()),
        builds,
        "hover never rebuilds geometry"
    );
}
