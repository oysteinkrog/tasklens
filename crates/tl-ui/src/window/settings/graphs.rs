//! Settings window, Graphs page, P2 rows (E21-025, UI-SHL-067, APP-021, R-089): the seven
//! minigraph switches, "Keep detached graphs open when closing main window" (E21-036) and
//! "Reset Minigraph Layout".

use gpui_kit::App;

use super::{PageBuilder, PageId};
use crate::components::button::TlButtonKind;
use crate::host::HostContext;
use crate::i18n::tr;
use crate::pages::minigraph::actions::ResetMinigraphLayout;
use crate::settings_global::SettingKey;

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::Graphs);
    // One untitled group: the page title already says "Graphs".
    let group = "";
    for (key, label) in [
        (SettingKey::GraphsMinigraphCpu, "shell-menu-graphs-cpu"),
        (
            SettingKey::GraphsMinigraphMemory,
            "shell-menu-graphs-memory",
        ),
        (SettingKey::GraphsMinigraphDisk, "shell-menu-graphs-disk"),
        (
            SettingKey::GraphsMinigraphNetwork,
            "shell-menu-graphs-network",
        ),
        (SettingKey::GraphsMinigraphGpu, "shell-menu-graphs-gpu"),
        (SettingKey::GraphsMinigraphVram, "shell-menu-graphs-vram"),
        (
            SettingKey::GraphsMinigraphGpuTemperature,
            "shell-menu-graphs-gpu-temp",
        ),
    ] {
        page.switch(group, key, tr(cx, label), None);
    }
    page.switch(
        group,
        SettingKey::GraphsKeepDetachedOnClose,
        tr(cx, "shell-menu-graphs-keep-detached"),
        None,
    );
    page.button(
        group,
        tr(cx, "shell-menu-graphs-reset"),
        Box::new(ResetMinigraphLayout),
        TlButtonKind::Secondary,
    );
    page
}
