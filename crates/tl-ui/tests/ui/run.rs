//! Run dialog history through an in-memory `RunHistoryStore` (tasklens-gma.31).

use std::cell::RefCell;

use tl_ui::app::{RunHistoryEntry, RunHistoryStore};
use tl_ui::windows::run_history;

#[derive(Default)]
struct MemStore(RefCell<Vec<RunHistoryEntry>>);

impl RunHistoryStore for MemStore {
    fn load(&self) -> Vec<RunHistoryEntry> {
        self.0.borrow().clone()
    }

    fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String> {
        *self.0.borrow_mut() = entries.to_vec();
        Ok(())
    }
}

#[test]
fn history_roundtrip_through_store() {
    let store = MemStore::default();
    *store.0.borrow_mut() = vec![
        RunHistoryEntry {
            command: "top".into(),
            executed_at: "2026-10-06T10:00:00+02:00".into(),
            elevated: false,
        },
        RunHistoryEntry {
            command: "ls".into(),
            executed_at: "2026-10-05T09:00:00+02:00".into(),
            elevated: false,
        },
    ];

    let loaded = run_history::load(Some(&store));
    let commands: Vec<_> = loaded.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["top", "ls"]);

    let updated = run_history::record(&loaded, "firefox", true, "2026-10-06T11:30:00-03:30");
    assert_eq!(run_history::save(Some(&store), &updated), Ok(()));

    let again = run_history::load(Some(&store));
    assert_eq!(&*again, updated.as_slice());
    assert_eq!(again[0].executed_at, "2026-10-06T11:30:00-03:30");
    assert!(again[0].elevated);
    assert!(!again[1].elevated);

    assert_eq!(run_history::clear(Some(&store)), Ok(()));
    assert!(run_history::load(Some(&store)).is_empty());
}

/// The Run window (tasklens-gma.32, UI-PWN-028, UI-PWN-029). Run with `--features test-hooks`.
#[cfg(feature = "test-hooks")]
mod window {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, WindowHandle, px, size};
    use tl_model::actions::{
        ActionError, ActionEvent, ActionKind, ActionRequest, ActionState, LaunchWhat,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::settings::{Decorations, Settings, UiState};
    use tl_ui::app::{AppEnv, RunHistoryEntry, RunHistoryStore, TasklensUi, UiInit};
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::components::window_frame::{TestClientFrame, client_frame_extra, frame_bottom};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::windows::run::{POPUP_ID, ROW_H, RunWindow, VISIBLE_ROWS, open_run_window};

    /// A history file shared with the test.
    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<Vec<RunHistoryEntry>>>);

    impl RunHistoryStore for Shared {
        fn load(&self) -> Vec<RunHistoryEntry> {
            self.0.borrow().clone()
        }

        fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String> {
            *self.0.borrow_mut() = entries.to_vec();
            Ok(())
        }
    }

    fn entry(command: &str) -> RunHistoryEntry {
        RunHistoryEntry {
            command: command.into(),
            executed_at: "2026-10-06T10:00:00+02:00".into(),
            elevated: false,
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        store: Shared,
        main: VisualTestContext,
        run: VisualTestContext,
        view: Entity<RunWindow>,
    }

    /// App globals with `history` in the store, the main window, and the Run window. The window
    /// is checked with the kit title bar and the client frame. The default is server decorations
    /// (D-035).
    fn setup(cx: &mut TestAppContext, history: &[&str]) -> Rig {
        setup_with(cx, history, Decorations::Client)
    }

    /// `setup` with `decorations` as `appearance.decorations`.
    fn setup_with(cx: &mut TestAppContext, history: &[&str], decorations: Decorations) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let store = Shared::default();
        *store.0.borrow_mut() = history.iter().map(|c| entry(c)).collect();
        let mut env = AppEnv::test();
        env.run_history = Some(Box::new(store.clone()));
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.appearance.decorations = decorations;
        let init = UiInit {
            settings,
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env,
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let main = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();

        cx.update(open_run_window);
        cx.run_until_parked();
        let (handle, view) = find_run(cx).expect("the Run window opens");
        let mut run = VisualTestContext::from_window(handle.into(), cx);
        run.update(|window, _| window.activate_window());
        run.run_until_parked();
        Rig {
            engine,
            store,
            main,
            run,
            view,
        }
    }

    fn find_run(cx: &mut TestAppContext) -> Option<(WindowHandle<Root>, Entity<RunWindow>)> {
        cx.update(|cx| {
            cx.windows().into_iter().find_map(|w| {
                let h = w.downcast::<Root>()?;
                RunWindow::find(h, cx).map(|v| (h, v))
            })
        })
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();
    }

    fn launches(engine: &FakeEngine) -> Vec<ActionRequest> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(req) if matches!(req.kind, ActionKind::Launch(_)) => Some(req),
                _ => None,
            })
            .collect()
    }

    fn run_toasts(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|window, cx| {
            ToastQueue::of_window(window, cx)
                .expect("toast queue")
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        })
    }

    fn main_toasts(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .toasts()
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        })
    }

    fn set_text(rig: &mut Rig, text: &str) {
        let input = rig.run.read(|cx| rig.view.read(cx).input().clone());
        let text = text.to_owned();
        rig.run
            .update(|window, cx| input.update(cx, |s, cx| s.set_value(text, window, cx)));
        rig.run.run_until_parked();
    }

    fn text(rig: &mut Rig) -> String {
        rig.run
            .read(|cx| rig.view.read(cx).input().read(cx).text().to_string())
    }

    #[gpui_kit::test]
    fn run_empty_command_toast(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &[]);
        set_text(&mut rig, "   ");
        rig.run.simulate_keystrokes("enter");
        assert_eq!(run_toasts(&mut rig.run), ["Please enter a command"]);
        assert!(launches(&rig.engine).is_empty());
        assert!(find_run(cx).is_some(), "the window stays open");
    }

    /// The visible window is the 480 x 248 spec: the footer and its buttons end inside it, the
    /// root note has its own row above the footer, and the empty-command toast stays off the
    /// input (UI-PWN-028, tasklens-fut.23). The client-frame hook lays the window out inside the
    /// 21 px frame on each edge, as under KWin with client decorations.
    #[gpui_kit::test]
    fn run_window_shows_all_content_and_toast_clears_the_input(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(TestClientFrame));
        let mut rig = setup(cx, &["firefox"]);
        let extra = client_frame_extra(Decorations::Client);
        draw(&mut rig.run);
        let window = rig.run.update(|window, _| window.bounds());
        assert_eq!(window.size, size(px(480.) + extra, px(248.) + extra));
        let inset = rig.run.update(|window, cx| frame_bottom(window, cx));
        #[cfg(target_os = "linux")]
        assert_eq!(inset, px(21.), "the client frame hook is on");
        let inner_bottom = window.size.height - inset;
        let footer = rig.run.debug_bounds("RunFooter").expect("footer");
        let note = rig.run.debug_bounds("RunWaylandNote").expect("note");
        assert_eq!(note.size.height, px(18.));
        assert_eq!(footer.size.height, px(52.));
        assert!(note.bottom() <= footer.top(), "{note:?} over {footer:?}");
        assert!(footer.bottom() <= inner_bottom, "{footer:?} in {window:?}");

        set_text(&mut rig, "");
        rig.run.simulate_keystrokes("enter");
        draw(&mut rig.run);
        let input = rig.run.debug_bounds("RunCombo").expect("combo");
        let toast = rig.run.debug_bounds("TlToast-a1-frame").expect("toast");
        assert!(
            toast.bottom() <= input.top() || toast.top() >= input.bottom(),
            "{toast:?} covers {input:?}"
        );
    }

    /// Under server decorations (D-035) the Run window draws no kit title bar and no caption
    /// buttons: the window is the 480 x 248 spec with no frame, the 24 px pin sits 8 px in from
    /// the content's top-right corner, the footer ends inside the window and Esc cancels.
    #[gpui_kit::test]
    fn run_window_server_decorations(cx: &mut TestAppContext) {
        let mut rig = setup_with(cx, &[], Decorations::Server);
        draw(&mut rig.run);
        assert!(rig.run.debug_bounds("TlWindowTitleBar").is_none());
        for caption in [
            "TlWindowCaption-minimize",
            "TlWindowCaption-maximize",
            "TlWindowCaption-close",
        ] {
            assert!(rig.run.debug_bounds(caption).is_none(), "{caption}");
        }
        let window = rig.run.update(|window, _| window.bounds());
        assert_eq!(window.size, size(px(480.), px(248.)));
        let content = rig.run.debug_bounds("TlWindowContent").expect("content");
        assert_eq!(content.top(), px(0.), "the content starts at the top");
        let pin = rig.run.debug_bounds("TlWindowPin").expect("pin");
        assert_eq!(pin.size, size(px(24.), px(24.)));
        assert_eq!(pin.top(), content.top() + px(8.));
        assert_eq!(pin.right(), content.right() - px(8.));
        let footer = rig.run.debug_bounds("RunFooter").expect("footer");
        assert!(footer.bottom() <= window.size.height, "{footer:?}");

        rig.run.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(find_run(cx).is_none(), "Esc cancels the window");
    }

    #[gpui_kit::test]
    fn run_invalid_text_toast(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &[]);
        set_text(&mut rig, "kate\u{7}");
        rig.run.simulate_keystrokes("enter");
        assert_eq!(run_toasts(&mut rig.run), ["Command contains invalid text."]);
        assert!(launches(&rig.engine).is_empty());
    }

    #[gpui_kit::test]
    fn run_shift_enter_elevated(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &[]);
        set_text(&mut rig, "kate");
        rig.run.simulate_keystrokes("shift-enter");
        let sent = launches(&rig.engine);
        assert_eq!(sent.len(), 1);
        let ActionKind::Launch(spec) = &sent[0].kind else {
            unreachable!()
        };
        assert!(spec.as_admin);
        assert!(matches!(&spec.what, LaunchWhat::Command(c) if &**c == "kate"));
        assert!(rig.run.read(|cx| rig.view.read(cx).is_pending()));
        // The checkbox was not changed by Shift+Enter.
        assert!(!rig.run.read(|cx| rig.view.read(cx).elevated()));

        // Authentication cancelled: an Info toast, the window stays, nothing is recorded.
        rig.engine.emit(ActionEvent {
            id: sent[0].id,
            target: None,
            state: ActionState::Failed(ActionError::AuthCancelled),
        });
        cx.run_until_parked();
        assert_eq!(run_toasts(&mut rig.run), ["Authentication cancelled"]);
        assert!(!rig.run.read(|cx| rig.view.read(cx).is_pending()));
        assert!(rig.store.0.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn run_success_closes_and_toasts_in_main(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &["top"]);
        set_text(&mut rig, "kate ~/notes.txt");
        rig.run.simulate_keystrokes("enter");
        let sent = launches(&rig.engine);
        assert_eq!(sent.len(), 1);
        let ActionKind::Launch(spec) = &sent[0].kind else {
            unreachable!()
        };
        assert!(!spec.as_admin);
        rig.engine.emit(ActionEvent {
            id: sent[0].id,
            target: None,
            state: ActionState::Succeeded,
        });
        cx.run_until_parked();
        assert!(find_run(cx).is_none(), "the Run window closed");
        assert_eq!(main_toasts(&mut rig.main), ["Command launched"]);
        let saved: Vec<_> = rig
            .store
            .0
            .borrow()
            .iter()
            .map(|e| e.command.clone())
            .collect();
        assert_eq!(saved, ["kate ~/notes.txt", "top"]);
    }

    #[gpui_kit::test]
    fn run_clear_history_toast_and_disabled(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &["top", "htop"]);
        assert!(rig.run.read(|cx| rig.view.read(cx).can_clear()));
        let view = rig.view.clone();
        rig.run
            .update(|window, cx| view.update(cx, |v, cx| v.clear_history(window, cx)));
        rig.run.run_until_parked();
        assert_eq!(run_toasts(&mut rig.run), ["Command history cleared"]);
        assert!(rig.store.0.borrow().is_empty());
        assert!(!rig.run.read(|cx| rig.view.read(cx).can_clear()));
        // A second clear does nothing.
        rig.run
            .update(|window, cx| view.update(cx, |v, cx| v.clear_history(window, cx)));
        rig.run.run_until_parked();
        assert_eq!(run_toasts(&mut rig.run), ["Command history cleared"]);
        // Down finds no history to show.
        rig.run.simulate_keystrokes("down");
        assert!(!rig.run.read(|cx| rig.view.read(cx).popup_open()));
    }

    #[gpui_kit::test]
    fn run_combo_up_down_fills_input(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &["top", "htop", "ls"]);
        // The newest command is in the input when the window opens.
        assert_eq!(text(&mut rig), "top");
        set_text(&mut rig, "");
        rig.run.simulate_keystrokes("down");
        assert!(rig.run.read(|cx| rig.view.read(cx).popup_open()));
        assert_eq!(text(&mut rig), "top");
        rig.run.simulate_keystrokes("down");
        assert_eq!(text(&mut rig), "htop");
        rig.run.simulate_keystrokes("down");
        rig.run.simulate_keystrokes("down");
        assert_eq!(text(&mut rig), "ls", "Down stops at the last row");
        rig.run.simulate_keystrokes("up");
        assert_eq!(text(&mut rig), "htop");
        assert_eq!(rig.run.read(|cx| rig.view.read(cx).highlight()), Some(1));
        // Esc closes the popup first, then the window.
        rig.run.simulate_keystrokes("escape");
        assert!(!rig.run.read(|cx| rig.view.read(cx).popup_open()));
        assert!(find_run(cx).is_some());
    }

    #[gpui_kit::test]
    fn run_combo_five_rows_visible(cx: &mut TestAppContext) {
        let commands: Vec<String> = (0..10).map(|i| format!("cmd{i}")).collect();
        let refs: Vec<&str> = commands.iter().map(String::as_str).collect();
        let mut rig = setup(cx, &refs);
        set_text(&mut rig, "");
        rig.run.simulate_keystrokes("down");
        assert_eq!(rig.run.read(|cx| rig.view.read(cx).popup_rows().len()), 10);
        assert_eq!(text(&mut rig), "cmd0", "the first command fills the input");
        draw(&mut rig.run);
        let popup = rig.run.debug_bounds(POPUP_ID).expect("popup drawn");
        let s = rig.run.update(|_, cx| tl_ui::theme::metrics(cx).ui_scale);
        let want = ROW_H * s * VISIBLE_ROWS as f32;
        assert!(
            (f32::from(popup.size.height) - want).abs() < 0.5,
            "popup height {:?}, want {want}",
            popup.size.height
        );
    }

    #[gpui_kit::test]
    fn run_ctrl_n_activates_existing(cx: &mut TestAppContext) {
        let mut rig = setup(cx, &[]);
        let (first, _) = find_run(cx).expect("open");
        rig.main.update(|window, _| window.activate_window());
        rig.main.simulate_keystrokes("ctrl-n");
        cx.run_until_parked();
        let runs = cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter_map(|w| w.downcast::<Root>())
                .filter(|h| RunWindow::find(*h, cx).is_some())
                .count()
        });
        assert_eq!(runs, 1, "Ctrl+N does not open a second Run window");
        let active = cx.update(|cx| cx.active_window().map(|w| w.window_id()));
        assert_eq!(active, Some(first.window_id()));
    }
}
