//! The Set Priority submenu is wide enough for the autogroup note (UI-PRC-020, bead
//! tasklens-pdu.65 e). The text is measured, so the window uses the real text system.

use gpui_kit::base::test_support::snapshots;
use gpui_kit::{
    Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
    Styled as _, TestAppContext, TestDispatcher, TextRun, VisualTestContext, Window, div, font, px,
};
use smallvec::SmallVec;
use tl_model::ids::{NodeId, ProcessKey};
use tl_model::process::ProcFlags;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::time::BootId;
use tl_ui::components::menu::{MENU_MAX_W, TlMenu, context_menu_layer, open_context_menu_at};
use tl_ui::host::HostContext;
use tl_ui::pages::processes::actions::{CapturedTarget, CapturedTargets};
use tl_ui::pages::processes::menu::priority_submenu;
use tl_ui::table::tree_table::RowKind;
use tl_ui::text::fonts::{UI_FAMILY, load_bundled_fonts};
use tl_ui::theme::metrics;

/// The en-US autogroup note, the first line of the submenu.
const NOTE: &str = "With autogroup on, nice only orders processes within the same session.";

fn cosmic_cx() -> TestAppContext {
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    cx.update(|cx| {
        let report = load_bundled_fonts(cx);
        assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        gpui_kit::init(cx);
        tl_ui::components::menu::bind_keys(cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    });
    cx
}

struct Host {
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(13.));
        div()
            .size(px(1000.))
            .track_focus(&self.focus)
            .children(context_menu_layer(window, cx))
    }
}

fn targets() -> CapturedTargets {
    let key = ProcessKey {
        start_ticks: 1_000,
        boot_id: BootId(1),
        pid: 100,
    };
    let target = CapturedTarget {
        node: NodeId::Process(key),
        kind: RowKind::Process,
        name: "proc".into(),
        pid: Some(100),
        uid: 1000,
        user: "me".into(),
        flags: ProcFlags::empty(),
        member_count: 1,
        stopped: false,
        ppid: 1,
        nice: Some(0),
        io_priority: None,
        exe_deleted: false,
        unit: None,
    };
    CapturedTargets::new(SmallVec::<[CapturedTarget; 1]>::from_buf([target]), None)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[test]
fn priority_menu_is_wider_than_the_default_for_the_autogroup_note() {
    let mut cx = cosmic_cx();
    let (host, vcx) = cx.add_window_view(|_, cx| Host {
        focus: cx.focus_handle(),
    });
    redraw(vcx);
    let focus = vcx.update(|_, cx| host.read(cx).focus.clone());
    vcx.update(|window, cx| {
        focus.focus(window, cx);
        let targets = targets();
        let host = HostContext::local(cx);
        let menu = TlMenu::build(window, cx, focus, move |menu, window, cx| {
            priority_submenu(&host, menu, &targets, window, cx)
        });
        open_context_menu_at(gpui_kit::point(px(10.), px(10.)), window, cx, menu);
    });
    redraw(vcx);
    let width = vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::Menu))
            .map(|s| s.bounds().size.width)
            .expect("menu open")
    });
    assert!(
        width > px(MENU_MAX_W) && width <= px(520.),
        "priority menu is {width:?} wide"
    );

    // The note's label starts after the mark column and ends at the row's right padding. It
    // must hold the whole line, so the note shows no ellipsis.
    let (room, need) = vcx.update(|window, cx| {
        let snaps = snapshots(window);
        let header = snaps
            .iter()
            .find(|s| s.label() == Some(NOTE))
            .expect("autogroup note row");
        let mark = snaps
            .iter()
            .find(|s| {
                s.path().len() == header.path().len() + 1 && s.path().starts_with(header.path())
            })
            .expect("mark column of the note row");
        let m = *metrics(cx);
        let room = header.bounds().right() - m.sp[4] - mark.bounds().right();
        let run = TextRun {
            len: NOTE.len(),
            font: font(UI_FAMILY),
            color: gpui_kit::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let need = window
            .text_system()
            .shape_line(NOTE.into(), m.text.xs, &[run], None)
            .width;
        (room, need)
    });
    assert!(
        room >= need,
        "the note needs {need:?} but its label has {room:?}"
    );
}
