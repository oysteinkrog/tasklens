//! Settings window, Motion page (E21-021, UI-SHL-063, UI-SHL-132, R-017, UI-PRF-003): the
//! motion switches and sliders, with the notes shown while the system's reduced motion is on.

use gpui_kit::App;

use super::{PageBuilder, PageId};
use crate::components::banner::TlBannerKind;
use crate::i18n::tr;
use crate::settings_global::{SettingKey, TlSettings, system_reduce_motion};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Motion);
    page.page_description(tr(cx, "shell-settings-motion-desc"));
    // One untitled group: the page title already says "Motion".
    let group = "";
    let reduced = system_reduce_motion(cx);
    let smooth_scroll = TlSettings::settings(cx).motion.smooth_scroll;
    if reduced {
        page.banner(
            group,
            TlBannerKind::Info,
            tr(cx, "shell-settings-reduced-motion-on"),
        );
    }
    page.switch(
        group,
        SettingKey::MotionAnimations,
        tr(cx, "shell-menu-settings-animations"),
        None,
    )
    // Smooth Graphs keeps its stored value; the note says why graphs are not smooth now.
    .switch(
        group,
        SettingKey::MotionSmoothGraphs,
        tr(cx, "shell-menu-settings-smooth-graphs"),
        reduced.then(|| tr(cx, "shell-settings-smooth-graphs-note")),
    )
    .switch(
        group,
        SettingKey::MotionSavePowerOnBattery,
        tr(cx, "shell-settings-save-power-on-battery"),
        Some(tr(cx, "shell-settings-save-power-on-battery-note")),
    )
    .switch(
        group,
        SettingKey::MotionSmoothScroll,
        tr(cx, "shell-menu-settings-smooth-scroll"),
        None,
    )
    .disable_next(!smooth_scroll)
    .slider(
        group,
        SettingKey::MotionSmoothScrollAmount,
        tr(cx, "shell-settings-smooth-scroll-amount"),
        1..=50,
        1,
        |v| format!("{v}").into(),
    )
    .slider(
        group,
        SettingKey::MotionTabSwitchMs,
        tr(cx, "shell-settings-tab-switch"),
        0..=500,
        10,
        |v| format!("{v} ms").into(),
    );
    page
}
