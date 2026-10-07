//! Minigraph actions (tasklens-xdh.4, UI-PRF-040 to UI-PRF-045, UI-PRC-020).

use std::sync::Arc;

use gpui_kit::{Action, TestAppContext};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_ui::pages::minigraph::GraphColor;
use tl_ui::pages::minigraph::actions::{
    AddAppGraph, AddMinigraph, AddNameGraph, CloseMiniWindow, MinigraphAddToWindow,
    MinigraphAttach, MinigraphAttachAll, MinigraphCloseDetachedWindow, MinigraphDetach,
    MinigraphHide, MinigraphMoveToSlot, MinigraphRemove, MinigraphSetColor, MinigraphSetRange,
    ResetMinigraphLayout, ToggleMiniAlwaysOnTop,
};
use tl_ui::pages::minigraph::sources::{GraphColumn, MinigraphId, MinigraphSource};

fn id() -> MinigraphId {
    MinigraphId(3)
}

fn all() -> Vec<Box<dyn Action>> {
    vec![
        Box::new(ResetMinigraphLayout),
        Box::new(MinigraphAttachAll),
        Box::new(ToggleMiniAlwaysOnTop),
        Box::new(CloseMiniWindow),
        Box::new(MinigraphHide { id: id() }),
        Box::new(MinigraphRemove { id: id() }),
        Box::new(MinigraphSetRange {
            id: id(),
            secs: 300,
        }),
        Box::new(MinigraphSetColor {
            id: id(),
            color: GraphColor::Teal,
        }),
        Box::new(MinigraphMoveToSlot { id: id(), slot: 7 }),
        Box::new(MinigraphDetach { id: id() }),
        Box::new(MinigraphAttach { id: id() }),
        Box::new(MinigraphAddToWindow {
            window: 2,
            source: MinigraphSource::Process {
                key: ProcessKey {
                    start_ticks: 1_234_567,
                    boot_id: BootId(0x0123_4567_89ab_cdef),
                    pid: 4242,
                },
            },
        }),
        Box::new(MinigraphCloseDetachedWindow { window: 2 }),
        Box::new(AddMinigraph {
            column: GraphColumn::RssShared,
        }),
        Box::new(AddNameGraph {
            column: GraphColumn::Cpu,
        }),
        Box::new(AddAppGraph {
            column: GraphColumn::GpuShared,
        }),
    ]
}

#[gpui_kit::test]
fn minigraph_actions_in_namespace(cx: &mut TestAppContext) {
    let all = all();
    assert_eq!(all.len(), 16);
    cx.read(|cx| {
        let names = cx.all_action_names();
        for action in &all {
            let name = action.name();
            assert!(name.starts_with("tasklens::"), "{name}");
            assert!(names.contains(&name), "{name} is registered");
        }
    });
}

/// Every action survives serialising its fields and building it again by name, the path the
/// row menu and tray bridge use.
#[gpui_kit::test]
fn minigraph_action_round_trip(cx: &mut TestAppContext) {
    let range = MinigraphSetRange {
        id: id(),
        secs: 300,
    };
    assert_eq!(
        serde_json::to_value(&range).unwrap(),
        serde_json::json!({ "id": 3, "secs": 300 })
    );
    let add = MinigraphAddToWindow {
        window: 2,
        source: MinigraphSource::Gpu(Some(tl_ui::pages::performance::DeviceKey(Arc::from(
            "gpu:GPU-1",
        )))),
    };
    assert_eq!(
        serde_json::to_value(&add).unwrap(),
        serde_json::json!({ "window": 2, "source": "gpu:gpu:GPU-1" })
    );
    let set_color = MinigraphSetColor {
        id: id(),
        color: GraphColor::Lavender,
    };
    assert_eq!(
        serde_json::to_value(&set_color).unwrap()["color"],
        serde_json::json!("lavender")
    );
    let mut actions = all();
    actions.push(Box::new(range));
    actions.push(Box::new(add));
    cx.read(|cx| {
        for action in &actions {
            let json = action_json(action.as_ref());
            let built = cx
                .build_action(action.name(), json)
                .unwrap_or_else(|e| panic!("build {}: {e:?}", action.name()));
            assert!(built.partial_eq(action.as_ref()), "got {built:?}");
        }
    });
}

/// The JSON fields of a data action, `None` for a unit action.
fn action_json(action: &dyn Action) -> Option<serde_json::Value> {
    let any = action.as_any();
    macro_rules! data {
        ($($ty:ty),*) => {
            $(if let Some(a) = any.downcast_ref::<$ty>() {
                return Some(serde_json::to_value(a).unwrap());
            })*
        };
    }
    data!(
        MinigraphHide,
        MinigraphRemove,
        MinigraphSetRange,
        MinigraphSetColor,
        MinigraphMoveToSlot,
        MinigraphDetach,
        MinigraphAttach,
        MinigraphAddToWindow,
        MinigraphCloseDetachedWindow,
        AddMinigraph,
        AddNameGraph,
        AddAppGraph
    );
    None
}

#[gpui_kit::test]
fn add_minigraph_from_json(cx: &mut TestAppContext) {
    cx.read(|cx| {
        let built = cx
            .build_action(
                AddMinigraph {
                    column: GraphColumn::Vram,
                }
                .name(),
                Some(serde_json::json!({ "column": "vram" })),
            )
            .expect("build AddMinigraph");
        assert!(built.partial_eq(&AddMinigraph {
            column: GraphColumn::Vram,
        }));
        let bad = cx.build_action(
            "tasklens::MinigraphAddToWindow",
            Some(serde_json::json!({ "window": 1, "source": "bogus:1" })),
        );
        assert!(bad.is_err(), "an unknown source does not build");
    });
}
