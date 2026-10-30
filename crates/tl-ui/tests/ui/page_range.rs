//! One time range for the Overview and every device page (crw.105). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::settings::OverviewRange;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::cpu::CpuPage;
use tl_ui::pages::performance::overview::OVERVIEW_KEY;
use tl_ui::pages::performance::overview::range::{PAGE_SWITCH, segment_id};
use tl_ui::settings_global::TlSettings;

use crate::overview_range::{Rig, box_charts, click, draw, open};

const S: u64 = 1_000_000_000;

fn select(r: &mut Rig, key: &str) {
    let page = r.page.clone();
    let key = DeviceKey(Arc::from(key));
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(key, cx));
    });
    draw(r.vcx);
}

/// A click on "1 h" in the CPU page header saves the shared range: the CPU charts and every
/// Overview box draw one hour.
#[gpui_kit::test]
fn page_switch_moves_the_overview(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "cpu");
    let id: &'static str = Box::leak(segment_id(PAGE_SWITCH, OverviewRange::H1).into_boxed_str());
    click(r.vcx, id);
    let saved = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.overview_range);
    assert_eq!(saved, OverviewRange::H1);
    let page = r.page.clone();
    let cpu_range = r.vcx.update(|_, cx| {
        let cpu = page
            .read(cx)
            .page(&DeviceKey(Arc::from("cpu")))
            .and_then(|v| v.clone().downcast::<CpuPage>().ok())
            .expect("a CpuPage");
        cpu.read(cx).overall().read(cx).spec.range_ns
    });
    assert_eq!(cpu_range, 3_600 * S);

    select(&mut r, OVERVIEW_KEY);
    for chart in box_charts(&mut r) {
        let range = r.vcx.read_entity(&chart, |c, _| c.spec.range_ns);
        assert_eq!(range, 3_600 * S, "the Overview follows");
    }
}
