//! Tray contract between `tl-ui` (tray bridge) and `tl-tray` (StatusNotifierItem service).
//!
//! ADR-001 lets neither crate depend on the other, so the shared model, events and channels
//! live here (R-161 item 1). The UI sends a `TrayModel` about once per second and receives
//! `TrayEvent`s from the tray menus and icons.

use std::sync::Arc;

use crate::series::SeriesView;
use crate::time::TimeNs;

/// Stable id of one tray graph, assigned at load from the order of `[[tray_graphs]]`
/// (1-based) and then incremented for each new graph. Never reused in one run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TrayGraphId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayGraphState {
    Live,
    NoMatch,
    Ended,
    Failed,
}

/// Everything `tl-tray` needs to draw and describe one graph icon. Built by tl-ui.
#[derive(Clone, Debug)]
pub struct TrayGraphFrame {
    pub id: TrayGraphId,
    /// None: baseline only (Failed, or no series yet).
    pub series: Option<Arc<SeriesView>>,
    pub now_ns: TimeNs,
    /// `range_s * 1_000_000_000`.
    pub range_ns: u64,
    /// Scale ceiling, computed in tl-ui by the chart scale rules.
    pub max: f32,
    /// Resolved graph colour (accent or named).
    pub line_rgb: [u8; 3],
    pub state: TrayGraphState,
    pub tooltip_title: Arc<str>,
    pub tooltip_description: Arc<str>,
    /// Stored value: "accent" or a lower-case APP-039 name.
    pub color: Arc<str>,
    /// 30, 60, 300 or 600.
    pub range_s: u32,
}

/// Check state of the "Tray icon graphs >" submenu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayMenuState {
    /// In this order: cpu, memory, disk, network, gpu, vram, gpu_temp. true = a tray graph with that source exists.
    pub page_sources: [bool; 7],
    /// Process-source graphs (process:, name:, app:) by id and label, in `[[tray_graphs]]` order.
    pub process_graphs: Arc<[(TrayGraphId, Arc<str>)]>,
}

#[derive(Clone, Debug)]
pub struct TrayModel {
    pub show_main_icon: bool,
    /// "tasklens".
    pub main_tooltip_title: Arc<str>,
    /// Five lines, see E31-007.
    pub main_tooltip_description: Arc<str>,
    /// In `[[tray_graphs]]` order.
    pub graphs: Arc<[TrayGraphFrame]>,
    pub menu: TrayMenuState,
}

/// Page sources in menu order: (stored source string, menu label).
pub const TRAY_PAGE_SOURCES: [(&str, &str); 7] = [
    ("cpu", "CPU"),
    ("memory", "Memory"),
    ("disk", "Disk"),
    ("network", "Network"),
    ("gpu", "GPU"),
    ("vram", "VRAM"),
    ("gpu_temp", "GPU temp"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum TrayEvent {
    /// A StatusNotifierWatcher is on the bus.
    WatcherOnline,
    /// No watcher; "Add tray icon graph" items disable.
    WatcherOffline,
    /// ksni spawn failed for this graph.
    RegisterFailed(TrayGraphId),
    /// Left click (SNI Activate) on any tasklens item.
    ToggleMainWindow,
    ShowMainWindow,
    ShowMiniWindow,
    Exit,
    SetColor {
        id: TrayGraphId,
        color: Arc<str>,
    },
    SetRange {
        id: TrayGraphId,
        secs: u32,
    },
    Remove {
        id: TrayGraphId,
    },
    /// `source` is one of the `TRAY_PAGE_SOURCES` stored strings.
    TogglePageSource {
        source: &'static str,
    },
}

/// UI end: tl-ui sends models and receives events.
pub struct TrayUiEnd {
    pub models: async_channel::Sender<TrayModel>,
    pub events: async_channel::Receiver<TrayEvent>,
}

/// Service end: tl-tray receives models and sends events.
pub struct TrayServiceEnd {
    pub models: async_channel::Receiver<TrayModel>,
    pub events: async_channel::Sender<TrayEvent>,
}

/// Model channel capacity. The UI uses `try_send` and drops a model when full, because the next
/// 1 Hz model replaces it.
const MODEL_CAPACITY: usize = 2;

/// Event channel capacity (UI-PRF-051).
const EVENT_CAPACITY: usize = 16;

/// Model channel bounded(2), event channel bounded(16).
pub fn tray_channels() -> (TrayUiEnd, TrayServiceEnd) {
    let (model_tx, model_rx) = async_channel::bounded(MODEL_CAPACITY);
    let (event_tx, event_rx) = async_channel::bounded(EVENT_CAPACITY);
    (
        TrayUiEnd {
            models: model_tx,
            events: event_rx,
        },
        TrayServiceEnd {
            models: model_rx,
            events: event_tx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(title: &str) -> TrayModel {
        TrayModel {
            show_main_icon: true,
            main_tooltip_title: Arc::from(title),
            main_tooltip_description: Arc::from("line 1\nline 2"),
            graphs: Arc::from([TrayGraphFrame {
                id: TrayGraphId(1),
                series: None,
                now_ns: 5_000_000_000,
                range_ns: 60_000_000_000,
                max: 100.0,
                line_rgb: [0x35, 0x84, 0xe4],
                state: TrayGraphState::Failed,
                tooltip_title: Arc::from("CPU"),
                tooltip_description: Arc::from("12%"),
                color: Arc::from("accent"),
                range_s: 60,
            }]),
            menu: TrayMenuState {
                page_sources: [true, false, false, false, false, false, false],
                process_graphs: Arc::from([(TrayGraphId(2), Arc::from("firefox"))]),
            },
        }
    }

    #[test]
    fn channel_roundtrip() {
        let (ui, service) = tray_channels();
        ui.models.try_send(model("tasklens")).unwrap();
        let got = service.models.try_recv().unwrap();
        assert!(got.show_main_icon);
        assert_eq!(&*got.main_tooltip_title, "tasklens");
        assert_eq!(&*got.main_tooltip_description, "line 1\nline 2");
        assert_eq!(got.graphs.len(), 1);
        let frame = &got.graphs[0];
        assert_eq!(frame.id, TrayGraphId(1));
        assert!(frame.series.is_none());
        assert_eq!(frame.now_ns, 5_000_000_000);
        assert_eq!(frame.range_ns, 60_000_000_000);
        assert_eq!(frame.max, 100.0);
        assert_eq!(frame.line_rgb, [0x35, 0x84, 0xe4]);
        assert_eq!(frame.state, TrayGraphState::Failed);
        assert_eq!(&*frame.color, "accent");
        assert_eq!(frame.range_s, 60);
        assert_eq!(got.menu, model("tasklens").menu);
    }

    #[test]
    fn model_channel_capacity_is_2() {
        let (ui, _service) = tray_channels();
        ui.models.try_send(model("a")).unwrap();
        ui.models.try_send(model("b")).unwrap();
        let err = ui.models.try_send(model("c")).unwrap_err();
        assert!(err.is_full());
    }

    #[test]
    fn event_channel_capacity_is_16() {
        let (_ui, service) = tray_channels();
        for _ in 0..16 {
            service.events.try_send(TrayEvent::ShowMainWindow).unwrap();
        }
        let err = service.events.try_send(TrayEvent::Exit).unwrap_err();
        assert!(err.is_full());
    }

    #[test]
    fn page_sources_order() {
        assert_eq!(
            TRAY_PAGE_SOURCES,
            [
                ("cpu", "CPU"),
                ("memory", "Memory"),
                ("disk", "Disk"),
                ("network", "Network"),
                ("gpu", "GPU"),
                ("vram", "VRAM"),
                ("gpu_temp", "GPU temp"),
            ]
        );
    }
}
