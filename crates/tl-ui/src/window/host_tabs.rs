//! The host tabs (UI-SHL-008, D-038, ADR-018 "Hosts", tasklens-q4x76.27).
//!
//! One tab per host, the local host first, right-aligned in the title bar or the menu row just
//! left of the pin. The main window shows the view only while it has more than one host. Each
//! tab: a 6 px status dot, the host name in `t_sm` and the host's CPU percent in
//! `text_secondary` with tabular figures ("home · 23%"). Height 24, padding 8, gap 2. The
//! selected host has the accent outline.
//!
//! The dot is `success` when connected, `warning` while connecting (it pulses at 1 Hz: the main
//! window flips [`HostTabs::flip_pulse`] every 500 ms) and `danger` when the last attempt
//! failed, with the reason as the tab's tooltip. A click dispatches [`SelectHost`].

use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::{
    App, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};

use crate::actions::SelectHost;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hosts_api::HostLinkStatus;
use crate::i18n::tr_args;
use crate::text::style::numeric_style;
use crate::theme::{current, metrics};

/// Tab height, × ui_scale.
const TAB_H: f32 = 24.0;
/// Tab padding left and right, × ui_scale.
const TAB_PAD: f32 = 8.0;
/// Gap between two tabs, × ui_scale.
const TAB_GAP: f32 = 2.0;
/// Status dot side, × ui_scale.
const DOT: f32 = 6.0;
/// Gap between the dot and the name, × ui_scale.
const DOT_GAP: f32 = 6.0;
/// Opacity of a connecting dot in the off half of its pulse.
const PULSE_LOW: f32 = 0.35;

/// What one tab shows.
#[derive(Clone, Debug, PartialEq)]
pub struct HostTabInfo {
    pub name: Arc<str>,
    pub status: HostLinkStatus,
    /// The host's CPU percent, rounded. `None` before the first value.
    pub cpu: Option<u8>,
}

/// One tab as last drawn, for tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnHostTab {
    pub label: SharedString,
    pub selected: bool,
    pub status: HostLinkStatus,
    pub tooltip: Option<SharedString>,
}

/// The host tab row. The main window sets its content.
#[derive(Default)]
pub struct HostTabs {
    tabs: Vec<HostTabInfo>,
    selected: usize,
    /// The on half of the connecting dot's pulse.
    pulse_on: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn: Vec<DrawnHostTab>,
}

/// The CPU percent of the newest valid `CPU_TOTAL` sample of the CPU package.
pub fn cpu_percent(snap: &tl_model::snapshot::Snapshot) -> Option<u8> {
    use tl_model::devices::DeviceIdentity;
    use tl_model::ids::{MetricId, SeriesId};
    let device = snap.devices.find(&DeviceIdentity::CpuPackage)?.idx;
    let value = snap
        .series
        .get(SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        })?
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())?
        .value;
    Some(value.clamp(0., 100.).round() as u8)
}

impl HostTabs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the tabs and the selected index. Notifies only on a change.
    pub fn set(&mut self, tabs: Vec<HostTabInfo>, selected: usize, cx: &mut Context<Self>) {
        if tabs != self.tabs || selected != self.selected {
            self.tabs = tabs;
            self.selected = selected;
            cx.notify();
        }
    }

    /// Flips the pulse of connecting dots. Notifies only when a tab is connecting.
    pub fn flip_pulse(&mut self, cx: &mut Context<Self>) {
        self.pulse_on = !self.pulse_on;
        if self
            .tabs
            .iter()
            .any(|t| t.status == HostLinkStatus::Connecting)
        {
            cx.notify();
        }
    }

    pub fn tabs(&self) -> &[HostTabInfo] {
        &self.tabs
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The tabs as last drawn.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn(&self) -> &[DrawnHostTab] {
        &self.drawn
    }

    /// "home · 23%", or the name alone before the first CPU value.
    fn label(info: &HostTabInfo) -> (SharedString, Option<SharedString>) {
        let cpu = info.cpu.map(|pct| SharedString::from(format!("· {pct}%")));
        (SharedString::from(info.name.to_string()), cpu)
    }

    /// The tooltip of a failed tab: "Can't connect: <reason>".
    fn tooltip(info: &HostTabInfo, cx: &App) -> Option<SharedString> {
        match &info.status {
            HostLinkStatus::Failed(why) => Some(tr_args(
                cx,
                "shell-host-tab-failed",
                &[("reason", why.to_string().into())],
            )),
            _ => None,
        }
    }
}

impl Render for HostTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        #[cfg(any(test, feature = "test-hooks"))]
        self.drawn.clear();
        let mut row = div()
            .debug_selector(|| "HostTabs".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(TAB_GAP * s))
            .h_full();
        for (i, info) in self.tabs.iter().enumerate() {
            let selected = i == self.selected;
            let (name, cpu) = Self::label(info);
            let tooltip = Self::tooltip(info, cx);
            #[cfg(any(test, feature = "test-hooks"))]
            self.drawn.push(DrawnHostTab {
                label: match &cpu {
                    Some(cpu) => format!("{name} {cpu}").into(),
                    None => name.clone(),
                },
                selected,
                status: info.status.clone(),
                tooltip: tooltip.clone(),
            });
            let (dot, dot_opacity) = match info.status {
                HostLinkStatus::Connected => (theme.status.success, 1.),
                HostLinkStatus::Connecting => (
                    theme.status.warning,
                    if self.pulse_on { 1. } else { PULSE_LOW },
                ),
                HostLinkStatus::Failed(_) => (theme.status.danger, 1.),
            };
            let hover = theme.tab_hover;
            let mut cpu_el = div().text_color(theme.text_secondary);
            *cpu_el.text_style() = numeric_style(&m, m.text.sm);
            let tab = div()
                .id(("HostTab", i))
                .debug_selector(move || format!("HostTab{i}"))
                .role(Role::Tab)
                .aria_label(name.clone())
                .flex()
                .flex_none()
                .items_center()
                .gap(px(DOT_GAP * s))
                .h(px(TAB_H * s))
                .px(px(TAB_PAD * s))
                .rounded(m.radius_small)
                .text_size(m.text.sm)
                .text_color(theme.text_primary)
                .whitespace_nowrap()
                .when(selected, |el| el.border_1().border_color(theme.accent))
                .hover(move |st| st.bg(hover))
                // A click on a tab never starts a window drag.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(SelectHost(i)), cx);
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(DOT * s))
                        .rounded_full()
                        .bg(dot)
                        .opacity(dot_opacity),
                )
                .child(name)
                .children(cpu.map(|cpu| cpu_el.child(cpu)));
            let tab = match tooltip {
                Some(text) => {
                    let tip = HoverTooltip::state(("HostTabTip", i), window, cx);
                    HoverTooltip::attach(
                        &tip,
                        tab,
                        DELAY_CONTROL,
                        TlTooltip::text(text).build(),
                        window,
                        cx,
                    )
                    .into_any_element()
                }
                None => tab.into_any_element(),
            };
            row = row.child(tab);
        }
        row
    }
}
