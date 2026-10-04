//! StatusNotifierItem service for the main and graph icons.
//!
//! One `ksni::Tray` type serves both kinds. ksni names the bus
//! `org.kde.StatusNotifierItem-<pid>-<n>` and serves `/StatusNotifierItem` and `/MenuBar` itself
//! (PERF-088). Every callback only sends a `TrayEvent` with `try_send`, so a slow UI never
//! blocks the ksni thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Category, Icon, OfflineReason, ToolTip};
use tl_model::tray::{TrayEvent, TrayGraphFrame, TrayGraphId, TrayGraphState, TrayMenuState};

use crate::menu::{self, MenuNode};

/// Events dropped because the event channel was full.
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Number of tray events dropped on a full channel since start.
pub fn dropped_events() -> u64 {
    DROPPED_EVENTS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Graph(TrayGraphId),
    Main,
}

/// What `menu.rs` needs to rebuild the menu when ksni asks for it. `color` and `range_s` are
/// read only for a graph item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuSource {
    /// Stored colour value: "accent" or a lower-case APP-039 name.
    pub color: Arc<str>,
    pub range_s: u32,
    pub state: TrayMenuState,
}

pub struct TrayItem {
    pub kind: ItemKind,
    /// ARGB32, 22*22*4 bytes.
    pub icon_22: Vec<u8>,
    /// ARGB32, 44*44*4 bytes.
    pub icon_44: Vec<u8>,
    pub tooltip_title: Arc<str>,
    pub tooltip_description: Arc<str>,
    pub menu: MenuSource,
    pub events: async_channel::Sender<TrayEvent>,
}

impl TrayItem {
    /// Sends without blocking; a full or closed channel drops the event and counts it.
    fn send(&self, event: TrayEvent) {
        if self.events.try_send(event).is_err() {
            DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn menu_nodes(&self) -> Vec<MenuNode> {
        match self.kind {
            ItemKind::Main => menu::main_icon_menu(&self.menu.state),
            ItemKind::Graph(id) => {
                // `graph_icon_menu` reads only id, color and range_s from the frame.
                let frame = TrayGraphFrame {
                    id,
                    series: None,
                    now_ns: 0,
                    range_ns: u64::from(self.menu.range_s) * 1_000_000_000,
                    max: 0.0,
                    line_rgb: [0, 0, 0],
                    state: TrayGraphState::Live,
                    tooltip_title: self.tooltip_title.clone(),
                    tooltip_description: self.tooltip_description.clone(),
                    color: self.menu.color.clone(),
                    range_s: self.menu.range_s,
                };
                menu::graph_icon_menu(&frame, &self.menu.state)
            }
        }
    }
}

fn to_ksni(node: MenuNode) -> MenuItem<TrayItem> {
    match node {
        MenuNode::Item { label, event } => StandardItem {
            label: label.to_string(),
            activate: Box::new(move |tray: &mut TrayItem| tray.send(event.clone())),
            ..Default::default()
        }
        .into(),
        MenuNode::Sub { label, children } => SubMenu {
            label: label.to_string(),
            submenu: children.into_iter().map(to_ksni).collect(),
            ..Default::default()
        }
        .into(),
        MenuNode::Radio { options, selected } => {
            let labels = options
                .iter()
                .map(|(label, _)| RadioItem {
                    label: label.to_string(),
                    ..Default::default()
                })
                .collect();
            let events: Vec<TrayEvent> = options.into_iter().map(|(_, event)| event).collect();
            RadioGroup {
                selected,
                select: Box::new(move |tray: &mut TrayItem, index: usize| {
                    if let Some(event) = events.get(index) {
                        tray.send(event.clone());
                    }
                }),
                options: labels,
            }
            .into()
        }
        MenuNode::Check {
            label,
            checked,
            event,
        } => CheckmarkItem {
            label: label.to_string(),
            checked,
            activate: Box::new(move |tray: &mut TrayItem| tray.send(event.clone())),
            ..Default::default()
        }
        .into(),
        MenuNode::Separator => MenuItem::Separator,
    }
}

impl ksni::Tray for TrayItem {
    fn id(&self) -> String {
        match self.kind {
            ItemKind::Main => "tasklens".to_string(),
            ItemKind::Graph(TrayGraphId(n)) => format!("tasklens-graph-{n}"),
        }
    }

    fn title(&self) -> String {
        "tasklens".to_string()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![
            Icon {
                width: 22,
                height: 22,
                data: self.icon_22.clone(),
            },
            Icon {
                width: 44,
                height: 44,
                data: self.icon_44.clone(),
            },
        ]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: vec![],
            title: self.tooltip_title.to_string(),
            description: self.tooltip_description.to_string(),
        }
    }

    /// Left click on any tasklens item toggles the main window (PERF-090).
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayEvent::ToggleMainWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.menu_nodes().into_iter().map(to_ksni).collect()
    }

    fn watcher_online(&self) {
        self.send(TrayEvent::WatcherOnline);
    }

    /// Returns `true` so the service keeps running and ksni registers the item again when a
    /// `StatusNotifierWatcher` gains an owner (PERF-092).
    fn watcher_offline(&self, _reason: OfflineReason) -> bool {
        self.send(TrayEvent::WatcherOffline);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    fn item(kind: ItemKind, events: async_channel::Sender<TrayEvent>) -> TrayItem {
        TrayItem {
            kind,
            icon_22: vec![1; 22 * 22 * 4],
            icon_44: vec![2; 44 * 44 * 4],
            tooltip_title: Arc::from("CPU Usage"),
            tooltip_description: Arc::from("18.6%"),
            menu: MenuSource {
                color: Arc::from("accent"),
                range_s: 60,
                state: TrayMenuState::default(),
            },
            events,
        }
    }

    fn graph_item() -> (TrayItem, async_channel::Receiver<TrayEvent>) {
        let (tx, rx) = async_channel::unbounded();
        (item(ItemKind::Graph(TrayGraphId(3)), tx), rx)
    }

    fn standard<'a>(menu: &'a [MenuItem<TrayItem>], wanted: &str) -> &'a StandardItem<TrayItem> {
        menu.iter()
            .find_map(|entry| match entry {
                MenuItem::Standard(item) if item.label == wanted => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {wanted}"))
    }

    fn sub_radio<'a>(menu: &'a [MenuItem<TrayItem>], wanted: &str) -> &'a RadioGroup<TrayItem> {
        menu.iter()
            .find_map(|entry| match entry {
                MenuItem::SubMenu(sub) if sub.label == wanted => match sub.submenu.as_slice() {
                    [MenuItem::RadioGroup(group)] => Some(group),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_else(|| panic!("no radio submenu {wanted}"))
    }

    #[test]
    fn item_ids() {
        let (tx, _rx) = async_channel::unbounded();
        assert_eq!(item(ItemKind::Main, tx.clone()).id(), "tasklens");
        assert_eq!(
            item(ItemKind::Graph(TrayGraphId(3)), tx).id(),
            "tasklens-graph-3"
        );
    }

    #[test]
    fn activate_sends_toggle() {
        for kind in [ItemKind::Main, ItemKind::Graph(TrayGraphId(3))] {
            let (tx, rx) = async_channel::unbounded();
            let mut tray = item(kind, tx);
            tray.activate(0, 0);
            assert_eq!(rx.try_recv(), Ok(TrayEvent::ToggleMainWindow));
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn pixmaps_two_sizes() {
        let (tray, _rx) = graph_item();
        let icons = tray.icon_pixmap();
        assert_eq!(icons.len(), 2);
        assert_eq!((icons[0].width, icons[0].height), (22, 22));
        assert_eq!((icons[1].width, icons[1].height), (44, 44));
        assert_eq!(icons[0].data, tray.icon_22);
        assert_eq!(icons[1].data, tray.icon_44);
    }

    #[test]
    fn tooltip_fields() {
        let (tray, _rx) = graph_item();
        let tip = tray.tool_tip();
        assert_eq!(tip.title, "CPU Usage");
        assert_eq!(tip.description, "18.6%");
        assert!(tip.icon_name.is_empty());
        assert!(tip.icon_pixmap.is_empty());
        assert_eq!(tray.title(), "tasklens");
        assert!(matches!(tray.category(), Category::SystemServices));
    }

    #[test]
    fn menu_events_reach_channel() {
        let (mut tray, rx) = graph_item();
        let id = TrayGraphId(3);
        let menu = tray.menu();

        (standard(&menu, "Close tray icon graph").activate)(&mut tray);
        assert_eq!(rx.try_recv(), Ok(TrayEvent::Remove { id }));

        let colors = sub_radio(&menu, "Graph color");
        assert_eq!(colors.selected, 0);
        assert_eq!(colors.options[8].label, "Teal");
        (colors.select)(&mut tray, 8);
        assert_eq!(
            rx.try_recv(),
            Ok(TrayEvent::SetColor {
                id,
                color: Arc::from("teal"),
            })
        );

        let ranges = sub_radio(&menu, "Time range");
        assert_eq!(ranges.selected, 1);
        (ranges.select)(&mut tray, 3);
        assert_eq!(rx.try_recv(), Ok(TrayEvent::SetRange { id, secs: 600 }));

        (standard(&menu, "Exit tasklens").activate)(&mut tray);
        assert_eq!(rx.try_recv(), Ok(TrayEvent::Exit));

        let checks = menu
            .iter()
            .find_map(|entry| match entry {
                MenuItem::SubMenu(sub) if sub.label == "Tray icon graphs" => Some(&sub.submenu),
                _ => None,
            })
            .expect("graphs submenu");
        match &checks[0] {
            MenuItem::Checkmark(check) => {
                assert_eq!(check.label, "CPU");
                assert!(!check.checked);
                (check.activate)(&mut tray);
            }
            _ => panic!("expected checkmark"),
        }
        assert_eq!(
            rx.try_recv(),
            Ok(TrayEvent::TogglePageSource { source: "cpu" })
        );
        assert!(
            menu.iter()
                .any(|entry| matches!(entry, MenuItem::Separator))
        );

        let (tx, main_rx) = async_channel::unbounded();
        let mut main = item(ItemKind::Main, tx);
        let main_menu = main.menu();
        assert_eq!(main_menu.len(), 4);
        (standard(&main_menu, "Show Main Window").activate)(&mut main);
        assert_eq!(main_rx.try_recv(), Ok(TrayEvent::ShowMainWindow));
    }

    #[test]
    fn full_channel_drops_without_blocking() {
        let (tx, rx) = async_channel::bounded(1);
        let mut tray = item(ItemKind::Main, tx);
        tray.activate(0, 0);
        let before = dropped_events();
        tray.activate(0, 0);
        assert_eq!(dropped_events(), before + 1);
        assert_eq!(rx.len(), 1);
    }

    #[test]
    fn watcher_callbacks_send_and_keep_running() {
        let (tray, rx) = graph_item();
        tray.watcher_online();
        assert_eq!(rx.try_recv(), Ok(TrayEvent::WatcherOnline));
        assert!(tray.watcher_offline(OfflineReason::No));
        assert_eq!(rx.try_recv(), Ok(TrayEvent::WatcherOffline));
    }
}
