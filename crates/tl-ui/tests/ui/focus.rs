//! Main window focus rules (E20-007, UI-SHL-121, UI-SHL-122): the initial-focus helper, the
//! Esc chain decision table, and the Tab order.

use gpui_kit::TestAppContext;
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

/// The main window's Esc chain (E20-011, UI-SHL-122) and Tab order (UI-SHL-121). Tab and
/// Shift+Tab are the kit `Root` bindings, so the tests press the real keys. Run with
/// `--features test-hooks`.
#[cfg(feature = "test-hooks")]
mod esc_chain {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
        IntoElement, ParentElement as _, Render, Styled, TestAppContext, VisualTestContext, Window,
        div, point, px,
    };
    use tl_model::command::Tab;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::table::TableMode;
    use tl_ui::actions::{EscapeChain, RefreshCurrentTab};
    use tl_ui::app::{BootSettings, UiExit, UiExitSlot};
    use tl_ui::components::menu::{TlMenu, context_menu_open, open_context_menu_at};
    use tl_ui::components::toast;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::window::main::{
        self as shell, FilterChanged, GlobalShellEvents, MainWindow, PageSlot, ShellPage,
        main_window_view, open_main_window, set_test_page,
    };
    use tl_ui::window::status::StatusChanged;

    struct FakePage {
        focus: FocusHandle,
    }

    impl EventEmitter<StatusChanged> for FakePage {}

    impl Render for FakePage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("fake-page")
                .track_focus(&self.focus)
                .size_full()
                .child("")
        }
    }

    impl ShellPage for FakePage {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    fn fake(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| FakePage {
            focus: cx.focus_handle().tab_stop(true),
        });
        PageSlot::new(page, cx)
    }

    type Events = Rc<RefCell<Vec<String>>>;

    struct Rig {
        vcx: VisualTestContext,
        events: Events,
    }

    /// The main window on a fake Processes page, with a fake Services page, recording
    /// `FilterChanged` texts.
    fn rig(cx: &mut TestAppContext) -> Rig {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            set_test_page(cx, Tab::Processes, fake);
            set_test_page(cx, Tab::Services, fake);
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let events: Events = Rc::default();
        let seen = events.clone();
        vcx.update(|_, cx| {
            let bus = cx.global::<GlobalShellEvents>().0.clone();
            cx.subscribe(&bus, move |_, e: &FilterChanged, _| {
                seen.borrow_mut().push(e.text.to_string());
            })
            .detach();
        });
        let mut r = Rig { vcx, events };
        r.draw();
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn main(&mut self) -> Entity<MainWindow> {
            self.vcx
                .update(|_, cx| main_window_view(cx).expect("main window open"))
        }

        fn page_focus(&mut self) -> FocusHandle {
            self.page_focus_on(Tab::Processes)
        }

        fn page_focus_on(&mut self, tab: Tab) -> FocusHandle {
            let main = self.main();
            let page = self
                .vcx
                .read_entity(&main, |m, _| m.page_entity::<FakePage>(tab))
                .expect("page built");
            self.vcx.read_entity(&page, |p, _| p.focus.clone())
        }

        fn page_focused(&mut self) -> bool {
            let focus = self.page_focus();
            self.vcx.update(|window, _| focus.is_focused(window))
        }

        fn filter_focused(&mut self) -> bool {
            let main = self.main();
            self.vcx.update(|window, cx| {
                let filter = main.read(cx).filter_box().clone();
                filter.read(cx).is_focused(window, cx)
            })
        }

        /// The "Include children" checkbox holds focus.
        fn checkbox_focused(&mut self) -> bool {
            self.vcx.update(|window, _| {
                window
                    .context_stack()
                    .iter()
                    .any(|c| c.contains("TlCheckbox"))
            })
        }

        fn focus_filter(&mut self) {
            let main = self.main();
            self.vcx.update(|window, cx| {
                let input = main.read(cx).filter_box().read(cx).input().clone();
                input.update(cx, |s, cx| s.focus(window, cx));
            });
            self.draw();
        }

        /// Presses `keys` as real keystrokes and draws the next frame.
        fn press(&mut self, keys: &str) {
            self.vcx.simulate_keystrokes(keys);
            self.vcx.run_until_parked();
            self.draw();
        }

        fn filter_text(&mut self) -> String {
            let main = self.main();
            self.vcx.update(|_, cx| {
                let filter = main.read(cx).filter_box().clone();
                filter.read(cx).text(Tab::Processes).to_string()
            })
        }

        /// Types `text` into the filter, lets the debounce fire, focuses the page and forgets the
        /// events.
        fn type_then_focus_page(&mut self, text: &str) {
            let main = self.main();
            self.vcx.update(|window, cx| {
                let input = main.read(cx).filter_box().read(cx).input().clone();
                input.update(cx, |s, cx| s.focus(window, cx));
            });
            self.draw();
            self.vcx.simulate_input(text);
            self.vcx
                .executor()
                .advance_clock(Duration::from_millis(200));
            self.vcx.run_until_parked();
            let focus = self.page_focus();
            self.vcx.update(|window, cx| window.focus(&focus, cx));
            self.draw();
            self.events.borrow_mut().clear();
        }

        fn esc(&mut self) {
            self.vcx.simulate_keystrokes("escape");
            self.vcx.run_until_parked();
            self.draw();
        }

        fn exit(&mut self) -> Option<UiExit> {
            self.vcx
                .update(|_, cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone()))
        }
    }

    #[gpui_kit::test]
    fn esc_chain_order(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.type_then_focus_page("fire");
        assert_eq!(r.filter_text(), "fire");
        assert!(r.page_focused());

        // Filter text, focus on the table: Esc clears at once and the table keeps focus.
        r.esc();
        assert_eq!(r.filter_text(), "");
        assert_eq!(
            *r.events.borrow(),
            [String::new()],
            "FilterChanged \"\" at once"
        );
        assert!(r.page_focused());

        // Empty filter, "Close with Esc" off: nothing happens.
        r.events.borrow_mut().clear();
        r.esc();
        assert!(r.events.borrow().is_empty());
        assert!(r.page_focused());
        assert_eq!(r.exit(), None);

        // "Close with Esc" on, default close settings: the window closes and the app quits.
        r.vcx
            .update(|_, cx| TlSettings::update(cx, |d| d.settings.window.close_with_esc = true));
        r.esc();
        assert_eq!(r.exit(), Some(UiExit::Quit));
    }

    #[gpui_kit::test]
    fn esc_menu_first(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.type_then_focus_page("fire");
        let focus = r.page_focus();
        r.vcx.update(|window, cx| {
            let menu = TlMenu::build(window, cx, focus, |m, _, _| {
                m.item("Refresh", Box::new(RefreshCurrentTab))
            });
            open_context_menu_at(point(px(20.), px(80.)), window, cx, menu);
        });
        r.draw();
        assert!(r.vcx.update(|window, cx| context_menu_open(window, cx)));

        r.esc();
        assert!(
            !r.vcx.update(|window, cx| context_menu_open(window, cx)),
            "menu closed"
        );
        assert_eq!(r.filter_text(), "fire", "the filter text stays");
        assert!(r.events.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn esc_cancels_pick(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let main = r.main();
        r.vcx
            .update(|_, cx| main.update(cx, |m, _| m.set_picking_for_test(true)));

        r.vcx.dispatch_action(EscapeChain);
        r.vcx.run_until_parked();
        // The main window handles `CancelWindowPick` itself (jqk.29) and ends picking.
        assert!(
            !r.vcx.update(|_, cx| main.read(cx).is_picking()),
            "CancelWindowPick ended picking"
        );
        assert_eq!(r.exit(), None);
    }

    /// UI-SHL-121 on Processes: the filter, "Include children" while shown, the table, then
    /// back to the filter. The fake page has no status chip actions, so nothing follows the
    /// table.
    #[gpui_kit::test]
    fn tab_order_processes(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.focus_filter();
        assert!(r.filter_focused());

        r.press("tab");
        assert!(r.page_focused(), "Tab from the filter reaches the table");
        r.press("tab");
        assert!(r.filter_focused(), "Tab from the table wraps to the filter");
        r.press("shift-tab");
        assert!(
            r.page_focused(),
            "Shift+Tab from the filter wraps to the table"
        );
        r.press("shift-tab");
        assert!(
            r.filter_focused(),
            "Shift+Tab from the table reaches the filter"
        );

        // Tree mode shows "Include children" between the filter and the table.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.processes.mode = TableMode::Tree)
        });
        r.vcx.run_until_parked();
        r.draw();
        assert!(r.vcx.debug_bounds("FilterBox-include-children").is_some());

        r.press("tab");
        assert!(
            r.checkbox_focused(),
            "Tab from the filter reaches the checkbox"
        );
        assert!(!r.filter_focused());
        r.press("tab");
        assert!(r.page_focused(), "the table follows the checkbox");
        r.press("tab");
        assert!(r.filter_focused(), "Tab wraps to the filter");
        r.press("shift-tab");
        assert!(r.page_focused());
        r.press("shift-tab");
        assert!(
            r.checkbox_focused(),
            "Shift+Tab from the table reaches the checkbox"
        );
        r.press("shift-tab");
        assert!(r.filter_focused());
    }

    /// UI-SHL-121 initial focus: after Ctrl+3 (`SelectTab(2)`) the Services table holds focus,
    /// and Tab goes on from there.
    #[gpui_kit::test]
    fn focus_after_tab_switch(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.focus_filter();
        r.press("ctrl-3");
        let services = r.page_focus_on(Tab::Services);
        assert!(
            r.vcx.update(|window, _| services.is_focused(window)),
            "the Services table is focused"
        );
        r.press("tab");
        assert!(r.filter_focused(), "Tab from the table wraps to the filter");
        r.press("shift-tab");
        assert!(r.vcx.update(|window, _| services.is_focused(window)));
    }
}
