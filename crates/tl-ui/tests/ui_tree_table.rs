//! Tree-table tests in a real window (UI-PRC-001, ADR-013). This section covers the generic cell
//! painters (E18-004); E18-A1 and later table beads append their own sections below.
//!
//! The cells are measured, so the window uses the real text system with the bundled fonts.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    App, Context, Div, IntoElement, Modifiers, MouseMoveEvent, ParentElement, Pixels, Render,
    SharedString, Styled, TestAppContext, TestDispatcher, VisualTestContext, Window, div, point,
    px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::validity::Validity;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::validity::{GlyphContent, glyph_parts};
use tl_ui::table::cells::{
    CellCtx, CellOut, attach_cell_tooltip, cell_background, cell_text_color, glyph_cell,
    numeric_cell, text_cell, with_overrides,
};
use tl_ui::text::fonts::load_bundled_fonts;
use tl_ui::theme::{current, metrics};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
}

fn cosmic_cx() -> TestAppContext {
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    cx.update(|cx| {
        let report = load_bundled_fonts(cx);
        assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        setup(cx);
    });
    cx
}

type BuildCell = Box<dyn Fn(&CellCtx, &mut Window, &mut App) -> Div>;

/// One row with one cell at x 0, the way the table positions cells.
struct Host {
    width: Pixels,
    selected: bool,
    cell: BuildCell,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let ctx = CellCtx {
            m: &m,
            theme: &theme,
            selected: self.selected,
            width: self.width,
        };
        let cell = (self.cell)(&ctx, window, cx);
        div().size(px(400.)).child(
            div()
                .relative()
                .w(px(400.))
                .h(m.row_height)
                .child(cell.absolute().left(px(0.)).top(px(0.))),
        )
    }
}

fn open(cx: &mut TestAppContext, width: f32, cell: BuildCell) -> &mut VisualTestContext {
    let (_, vcx) = cx.add_window_view(|_, _| Host {
        width: px(width),
        selected: false,
        cell,
    });
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx
}

/// A text cell with its hover tooltip, as the table builds it.
fn tooltip_text_cell(text: &'static str, flag: Rc<Cell<bool>>) -> BuildCell {
    Box::new(move |ctx, window, cx| {
        let text = SharedString::new_static(text);
        let cell = text_cell(text.clone(), ctx, flag.clone());
        attach_cell_tooltip(cell, "cell-tooltip", text, &flag, window, cx)
    })
}

fn hover(cx: &mut VisualTestContext, x: f32) {
    cx.simulate_event(MouseMoveEvent {
        position: point(px(x), px(10.)),
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[test]
fn text_cell_tooltip_only_when_truncated() {
    // 40 characters in a 60 px column.
    const LONG: &str = "gnome-shell-calendar-server-helper-12345";
    assert_eq!(LONG.chars().count(), 40);

    let mut cx = cosmic_cx();
    let flag = Rc::new(Cell::new(false));
    let cx = open(&mut cx, 60., tooltip_text_cell(LONG, flag.clone()));
    assert!(flag.get(), "the 40-character name is cut");
    // The table re-renders on hover; the second frame attaches the tooltip.
    cx.update(|window, _| window.refresh());
    redraw(cx);
    hover(cx, 30.);
    cx.executor().advance_clock(DELAY_CONTROL / 2);
    redraw(cx);
    assert!(cx.debug_bounds("TlTooltip").is_none(), "not before 500 ms");
    cx.executor().advance_clock(DELAY_CONTROL);
    redraw(cx);
    assert!(
        cx.debug_bounds("TlTooltip").is_some(),
        "the cut name shows its tooltip"
    );

    let mut cx = cosmic_cx();
    let flag = Rc::new(Cell::new(true));
    let cx = open(&mut cx, 60., tooltip_text_cell("sh", flag.clone()));
    assert!(!flag.get(), "a text that fits is not cut");
    cx.update(|window, _| window.refresh());
    redraw(cx);
    hover(cx, 30.);
    cx.executor().advance_clock(DELAY_CONTROL * 2);
    redraw(cx);
    assert!(
        cx.debug_bounds("TlTooltip").is_none(),
        "no tooltip on a text that fits"
    );
}

#[test]
fn numeric_cell_clips_without_ellipsis() {
    let numeric = |flag: Rc<Cell<bool>>| -> BuildCell {
        Box::new(move |ctx, _, _| {
            numeric_cell("10.00 GiB/s".into(), Validity::Valid, flag.clone(), ctx)
        })
    };

    // An ellipsized text would fit the content width, so a set flag also proves no "…" was
    // drawn: the whole text is shaped and wider than the cell.
    let mut cx = cosmic_cx();
    let narrow = Rc::new(Cell::new(false));
    let vcx = open(&mut cx, 50., numeric(narrow.clone()));
    assert!(narrow.get(), "10.00 GiB/s does not fit in 50 px");
    let cell = vcx.debug_bounds("TlCell-numeric").expect("cell drawn");
    assert_eq!(cell.size.width, px(50.));

    let mut cx = cosmic_cx();
    let wide = Rc::new(Cell::new(true));
    open(&mut cx, 200., numeric(wide.clone()));
    assert!(!wide.get(), "10.00 GiB/s fits in 200 px");
}

#[test]
fn glyph_cells_keep_right_edge() {
    for validity in [
        Validity::WarmingUp,
        Validity::Restricted,
        Validity::Unsupported,
        Validity::Missing,
        Validity::Failed,
    ] {
        let mut cx = cosmic_cx();
        let clipped = Rc::new(Cell::new(true));
        let flag = clipped.clone();
        let cx = open(
            &mut cx,
            120.,
            Box::new(move |ctx, _, _| numeric_cell("100.0%".into(), validity, flag.clone(), ctx)),
        );
        assert!(!clipped.get(), "{validity:?}: a glyph is never clipped");
        let pad = cx.update(|_, cx| metrics(cx).sp[4]);
        let cell = cx.debug_bounds("TlCell-glyph").expect("glyph cell drawn");
        let selector = if validity == Validity::Restricted {
            "TlValidityGlyph-lock"
        } else {
            "TlValidityGlyph-text"
        };
        let inner = cx.debug_bounds(selector).expect("glyph drawn");
        assert_eq!(cell.size.width, px(120.), "{validity:?}");
        assert_eq!(
            inner.right(),
            cell.right() - pad,
            "{validity:?} ends at the right padding edge"
        );
        if validity == Validity::Restricted {
            assert_eq!(inner.size.width, px(12.), "the lock is 12 px");
        }
    }
}

#[gpui_kit::test]
fn selected_row_uses_selected_text_colours(cx: &mut TestAppContext) {
    cx.update(setup);
    let (theme, m) = cx.update(|cx| (current(cx).clone(), *metrics(cx)));
    let ctx = CellCtx {
        m: &m,
        theme: &theme,
        selected: true,
        width: px(80.),
    };
    assert_eq!(ctx.primary(), theme.text_selected_primary);
    assert_eq!(ctx.secondary(), theme.text_selected_secondary);
    let value: SharedString = "3.2 MiB".into();
    let (content, color) = glyph_parts(Validity::Stale, &value, ctx.selected, &theme);
    assert_eq!(content, GlyphContent::Text(value));
    assert_eq!(
        color, theme.text_selected_secondary,
        "Stale on a selected row"
    );

    let out = CellOut {
        text_color: Some(theme.text_secondary),
        ..CellOut::default()
    };
    assert_eq!(
        cell_text_color(&out, &ctx),
        Some(theme.text_selected_secondary),
        "a dimmed value on a selected row"
    );
    let unselected = CellCtx {
        selected: false,
        ..ctx
    };
    assert_eq!(unselected.primary(), theme.text_primary);
    assert_eq!(unselected.secondary(), theme.text_secondary);
    assert_eq!(
        cell_text_color(&out, &unselected),
        Some(theme.text_secondary)
    );
}

#[test]
fn no_heat_on_glyph_cells() {
    let mut cx = cosmic_cx();
    let (theme, m) = cx.update(|cx| (current(cx).clone(), *metrics(cx)));
    let heat = theme.value_heat;
    let ctx = CellCtx {
        m: &m,
        theme: &theme,
        selected: false,
        width: px(80.),
    };
    let failed = CellOut {
        validity: Validity::Failed,
        background: Some(heat),
        ..CellOut::default()
    };
    assert_eq!(cell_background(&failed, &ctx), None, "no heat on a glyph");
    let (content, color) = glyph_parts(Validity::Failed, &"1".into(), false, &theme);
    assert_eq!(content, GlyphContent::Text("!".into()));
    assert_eq!(color, theme.status.warning);

    let valid = CellOut {
        validity: Validity::Valid,
        background: Some(heat),
        ..CellOut::default()
    };
    assert_eq!(cell_background(&valid, &ctx), Some(heat));
    let selected = CellCtx {
        selected: true,
        ..ctx
    };
    assert_eq!(
        cell_background(&valid, &selected),
        None,
        "the selected fill wins"
    );

    // The glyph cell renders with the overrides applied and keeps its glyph.
    let cx = open(
        &mut cx,
        80.,
        Box::new(move |ctx, _, _| {
            let out = CellOut {
                validity: Validity::Failed,
                background: Some(heat),
                ..CellOut::default()
            };
            with_overrides(glyph_cell(out.validity, &"1".into(), ctx), &out, ctx)
        }),
    );
    assert!(cx.debug_bounds("TlValidityGlyph-text").is_some());
}
