//! Main window focus rules (E20-007, UI-SHL-121, UI-SHL-122): the initial-focus helper and the
//! Esc chain decision table.

use gpui_kit::{AppContext as _, TestAppContext};
use tl_ui::focus::{EscOutcome, EscState, esc_outcome, focus_initial};

#[gpui_kit::test]
fn focus_initial_prefers_primary(cx: &mut TestAppContext) {
    let (primary, root) = cx.update(|cx| (cx.focus_handle(), cx.focus_handle()));
    let cx = cx.add_empty_window();

    cx.update(|window, cx| {
        focus_initial(Some(&primary), &root, window, cx);
        assert!(primary.is_focused(window), "primary handle focused");
        assert!(!root.is_focused(window), "root handle not focused");
    });

    cx.update(|window, cx| {
        focus_initial(None, &root, window, cx);
        assert!(root.is_focused(window), "root handle focused");
        assert!(!primary.is_focused(window), "primary handle not focused");
    });
}

#[test]
fn esc_outcome_order() {
    for bits in 0u8..16 {
        let s = EscState {
            picking: bits & 1 != 0,
            filter_focused: bits & 2 != 0,
            filter_has_text: bits & 4 != 0,
            close_with_esc: bits & 8 != 0,
        };
        let expected = if s.picking {
            EscOutcome::CancelPick
        } else if s.filter_focused {
            EscOutcome::FilterRule
        } else if s.filter_has_text {
            EscOutcome::ClearFilterAndFocusTable
        } else if s.close_with_esc {
            EscOutcome::CloseWindow
        } else {
            EscOutcome::Nothing
        };
        assert_eq!(esc_outcome(s), expected, "{s:?}");
    }

    let all = EscState {
        picking: true,
        filter_focused: true,
        filter_has_text: true,
        close_with_esc: true,
    };
    assert_eq!(esc_outcome(all), EscOutcome::CancelPick);
    assert_eq!(esc_outcome(EscState::default()), EscOutcome::Nothing);
}
