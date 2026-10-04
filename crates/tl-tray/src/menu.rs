//! Tray dbusmenu contents.
//!
//! The menus are plain data so their order and labels are testable without D-Bus. `item.rs`
//! turns them into `ksni::MenuItem`s when ksni asks for the menu (on open), not per tick.

use std::sync::Arc;

use tl_model::tray::{TRAY_PAGE_SOURCES, TrayEvent, TrayGraphFrame, TrayMenuState};

/// One dbusmenu entry. A submenu label carries no ">" character.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Item {
        label: &'static str,
        event: TrayEvent,
    },
    Sub {
        label: &'static str,
        children: Vec<MenuNode>,
    },
    Radio {
        options: Vec<(Arc<str>, TrayEvent)>,
        selected: usize,
    },
    Check {
        label: Arc<str>,
        checked: bool,
        event: TrayEvent,
    },
    Separator,
}

/// (stored value, menu label) in APP-039 order.
pub const GRAPH_COLORS: [(&str, &str); 18] = [
    ("accent", "Accent (theme default)"),
    ("coral", "Coral"),
    ("orange", "Orange"),
    ("amber", "Amber"),
    ("yellow", "Yellow"),
    ("lime", "Lime"),
    ("green", "Green"),
    ("mint", "Mint"),
    ("teal", "Teal"),
    ("cyan", "Cyan"),
    ("blue", "Blue"),
    ("indigo", "Indigo"),
    ("violet", "Violet"),
    ("lavender", "Lavender"),
    ("fuchsia", "Fuchsia"),
    ("pink", "Pink"),
    ("rose", "Rose"),
    ("slate", "Slate"),
];

/// (seconds, label).
pub const RANGES: [(u32, &str); 4] = [
    (30, "30 seconds"),
    (60, "1 minute"),
    (300, "5 minutes"),
    (600, "10 minutes"),
];

/// Range radio index used when `range_s` is not in `RANGES` ("1 minute").
const DEFAULT_RANGE_INDEX: usize = 1;

/// Menu of one tray graph icon, in PERF-090 order.
pub fn graph_icon_menu(frame: &TrayGraphFrame, state: &TrayMenuState) -> Vec<MenuNode> {
    let id = frame.id;
    let color_selected = GRAPH_COLORS
        .iter()
        .position(|(value, _)| *value == &*frame.color)
        .unwrap_or(0);
    let color_options = GRAPH_COLORS
        .iter()
        .map(|(value, label)| {
            (
                Arc::from(*label),
                TrayEvent::SetColor {
                    id,
                    color: Arc::from(*value),
                },
            )
        })
        .collect();
    let range_selected = RANGES
        .iter()
        .position(|(secs, _)| *secs == frame.range_s)
        .unwrap_or(DEFAULT_RANGE_INDEX);
    let range_options = RANGES
        .iter()
        .map(|(secs, label)| (Arc::from(*label), TrayEvent::SetRange { id, secs: *secs }))
        .collect();
    vec![
        MenuNode::Sub {
            label: "Graph color",
            children: vec![MenuNode::Radio {
                options: color_options,
                selected: color_selected,
            }],
        },
        MenuNode::Sub {
            label: "Time range",
            children: vec![MenuNode::Radio {
                options: range_options,
                selected: range_selected,
            }],
        },
        MenuNode::Item {
            label: "Close tray icon graph",
            event: TrayEvent::Remove { id },
        },
        graphs_submenu(state),
        MenuNode::Separator,
        MenuNode::Item {
            label: "Show Main Window",
            event: TrayEvent::ShowMainWindow,
        },
        MenuNode::Item {
            label: "Show mini window",
            event: TrayEvent::ShowMiniWindow,
        },
        MenuNode::Item {
            label: "Exit tasklens",
            event: TrayEvent::Exit,
        },
    ]
}

/// Menu of the main tasklens icon (UI-SHL-113, R-027).
pub fn main_icon_menu(state: &TrayMenuState) -> Vec<MenuNode> {
    vec![
        MenuNode::Item {
            label: "Show Main Window",
            event: TrayEvent::ShowMainWindow,
        },
        graphs_submenu(state),
        MenuNode::Separator,
        MenuNode::Item {
            label: "Exit tasklens",
            event: TrayEvent::Exit,
        },
    ]
}

/// "Tray icon graphs" submenu: one check per page source, then one checked entry per process
/// graph. Unchecking a process graph removes it (UI-PRF-051).
fn graphs_submenu(state: &TrayMenuState) -> MenuNode {
    let mut children: Vec<MenuNode> = TRAY_PAGE_SOURCES
        .iter()
        .zip(state.page_sources)
        .map(|((source, label), checked)| MenuNode::Check {
            label: Arc::from(*label),
            checked,
            event: TrayEvent::TogglePageSource { source },
        })
        .collect();
    if !state.process_graphs.is_empty() {
        children.push(MenuNode::Separator);
        children.extend(
            state
                .process_graphs
                .iter()
                .map(|(id, label)| MenuNode::Check {
                    label: label.clone(),
                    checked: true,
                    event: TrayEvent::Remove { id: *id },
                }),
        );
    }
    MenuNode::Sub {
        label: "Tray icon graphs",
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::tray::{TrayGraphId, TrayGraphState};

    fn frame(color: &str, range_s: u32) -> TrayGraphFrame {
        TrayGraphFrame {
            id: TrayGraphId(3),
            series: None,
            now_ns: 0,
            range_ns: u64::from(range_s) * 1_000_000_000,
            max: 100.0,
            line_rgb: [0, 0, 0],
            state: TrayGraphState::Live,
            tooltip_title: Arc::from("CPU"),
            tooltip_description: Arc::from(""),
            color: Arc::from(color),
            range_s,
        }
    }

    /// Top-level labels, with "-" for a separator.
    fn labels(nodes: &[MenuNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match node {
                MenuNode::Item { label, .. } | MenuNode::Sub { label, .. } => label.to_string(),
                MenuNode::Check { label, .. } => label.to_string(),
                MenuNode::Radio { .. } => "radio".to_string(),
                MenuNode::Separator => "-".to_string(),
            })
            .collect()
    }

    fn sub_children<'a>(nodes: &'a [MenuNode], wanted: &str) -> &'a [MenuNode] {
        nodes
            .iter()
            .find_map(|node| match node {
                MenuNode::Sub { label, children } if *label == wanted => Some(children.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {wanted}"))
    }

    fn radio(children: &[MenuNode]) -> (&[(Arc<str>, TrayEvent)], usize) {
        match children {
            [MenuNode::Radio { options, selected }] => (options.as_slice(), *selected),
            other => panic!("expected one radio, got {other:?}"),
        }
    }

    fn graphs_children(state: &TrayMenuState) -> Vec<MenuNode> {
        match graphs_submenu(state) {
            MenuNode::Sub { label, children } => {
                assert_eq!(label, "Tray icon graphs");
                children
            }
            other => panic!("expected submenu, got {other:?}"),
        }
    }

    #[test]
    fn tray_menu_order_graph_icon() {
        let menu = graph_icon_menu(&frame("teal", 300), &TrayMenuState::default());
        assert_eq!(
            labels(&menu),
            [
                "Graph color",
                "Time range",
                "Close tray icon graph",
                "Tray icon graphs",
                "-",
                "Show Main Window",
                "Show mini window",
                "Exit tasklens",
            ]
        );
        let (colors, selected) = radio(sub_children(&menu, "Graph color"));
        assert_eq!(selected, 8);
        assert_eq!(&*colors[selected].0, "Teal");
        assert_eq!(
            colors[selected].1,
            TrayEvent::SetColor {
                id: TrayGraphId(3),
                color: Arc::from("teal"),
            }
        );
        let (ranges, selected) = radio(sub_children(&menu, "Time range"));
        assert_eq!(selected, 2);
        assert_eq!(&*ranges[selected].0, "5 minutes");
        assert_eq!(
            ranges[selected].1,
            TrayEvent::SetRange {
                id: TrayGraphId(3),
                secs: 300,
            }
        );
        assert!(menu.contains(&MenuNode::Item {
            label: "Close tray icon graph",
            event: TrayEvent::Remove { id: TrayGraphId(3) },
        }));
        assert!(menu.contains(&MenuNode::Item {
            label: "Show mini window",
            event: TrayEvent::ShowMiniWindow,
        }));
        assert!(labels(&menu).iter().all(|label| !label.contains('>')));

        let unknown = graph_icon_menu(&frame("unknown", 60), &TrayMenuState::default());
        assert_eq!(radio(sub_children(&unknown, "Graph color")).1, 0);
    }

    #[test]
    fn tray_menu_order_main_icon() {
        let menu = main_icon_menu(&TrayMenuState::default());
        assert_eq!(
            labels(&menu),
            ["Show Main Window", "Tray icon graphs", "-", "Exit tasklens"]
        );
        assert_eq!(
            menu[0],
            MenuNode::Item {
                label: "Show Main Window",
                event: TrayEvent::ShowMainWindow,
            }
        );
        assert_eq!(
            menu[3],
            MenuNode::Item {
                label: "Exit tasklens",
                event: TrayEvent::Exit,
            }
        );
    }

    #[test]
    fn graph_color_list_order() {
        let values: Vec<&str> = GRAPH_COLORS.iter().map(|(value, _)| *value).collect();
        assert_eq!(
            values,
            [
                "accent", "coral", "orange", "amber", "yellow", "lime", "green", "mint", "teal",
                "cyan", "blue", "indigo", "violet", "lavender", "fuchsia", "pink", "rose", "slate",
            ]
        );
        assert_eq!(GRAPH_COLORS[0].1, "Accent (theme default)");
        assert_eq!(GRAPH_COLORS[17], ("slate", "Slate"));
    }

    #[test]
    fn range_radio_selection() {
        for (secs, expected) in [(30, 0), (60, 1), (300, 2), (600, 3), (45, 1)] {
            let menu = graph_icon_menu(&frame("accent", secs), &TrayMenuState::default());
            assert_eq!(
                radio(sub_children(&menu, "Time range")).1,
                expected,
                "{secs} s"
            );
        }
    }

    #[test]
    fn graphs_submenu_page_and_process_entries() {
        let only_cpu_network = TrayMenuState {
            page_sources: [true, false, false, true, false, false, false],
            process_graphs: Arc::from([]),
        };
        let children = graphs_children(&only_cpu_network);
        assert_eq!(children.len(), 7);
        assert!(!children.contains(&MenuNode::Separator));
        let checks: Vec<(String, bool, TrayEvent)> = children
            .iter()
            .map(|node| match node {
                MenuNode::Check {
                    label,
                    checked,
                    event,
                } => (label.to_string(), *checked, event.clone()),
                other => panic!("expected check, got {other:?}"),
            })
            .collect();
        let expected_labels = [
            "CPU", "Memory", "Disk", "Network", "GPU", "VRAM", "GPU temp",
        ];
        for (i, (label, checked, event)) in checks.iter().enumerate() {
            assert_eq!(label, expected_labels[i]);
            assert_eq!(*checked, label == "CPU" || label == "Network", "{label}");
            assert_eq!(
                *event,
                TrayEvent::TogglePageSource {
                    source: TRAY_PAGE_SOURCES[i].0,
                }
            );
        }

        let with_processes = TrayMenuState {
            page_sources: [false; 7],
            process_graphs: Arc::from([
                (TrayGraphId(4), Arc::from("firefox")),
                (TrayGraphId(9), Arc::from("cargo")),
            ]),
        };
        let children = graphs_children(&with_processes);
        assert_eq!(children.len(), 10);
        assert_eq!(children[7], MenuNode::Separator);
        assert_eq!(
            children[8],
            MenuNode::Check {
                label: Arc::from("firefox"),
                checked: true,
                event: TrayEvent::Remove { id: TrayGraphId(4) },
            }
        );
        assert_eq!(
            children[9],
            MenuNode::Check {
                label: Arc::from("cargo"),
                checked: true,
                event: TrayEvent::Remove { id: TrayGraphId(9) },
            }
        );

        let main = main_icon_menu(&with_processes);
        assert_eq!(sub_children(&main, "Tray icon graphs"), children.as_slice());
    }
}
