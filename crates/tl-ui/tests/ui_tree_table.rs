//! Tree-table tests in a real window (UI-PRC-001, ADR-013). The first section covers the generic
//! cell painters (E18-004), the second the table entity (E18-006); later table beads append their
//! own sections below. `tree_table_renders_visible_range_only` needs `--features test-hooks`.
//!
//! The cells are measured, so the window uses the real text system with the bundled fonts.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::ScrollbarHandle;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Div, Entity, IntoElement, Modifiers, MouseMoveEvent,
    ParentElement, Pixels, Render, ScrollStrategy, SharedString, Styled, TestAppContext,
    TestDispatcher, VisualTestContext, Window, div, point, px,
};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::SortSpec;
use tl_model::validity::Validity;
use tl_ui::components::menu::TlMenu;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::validity::{GlyphContent, glyph_parts};
use tl_ui::table::cells::{
    CellCtx, CellOut, attach_cell_tooltip, cell_background, cell_text_color, glyph_cell,
    guide_quads, numeric_cell, text_cell, with_overrides,
};
use tl_ui::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use tl_ui::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
    metric_cell_opacity,
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

// ---------------------------------------------------------------------------------------------
// E18-006: the TlTreeTable entity, its body and the one-line header.
// ---------------------------------------------------------------------------------------------

/// One test row. Every row has a Name cell and a CPU value.
#[derive(Clone)]
struct TRow {
    id: u32,
    kind: RowKind,
    depth: u8,
    flags: RowFlags,
    icon: Option<IconSlot>,
    name: &'static str,
}

fn plain(id: u32) -> TRow {
    TRow {
        id,
        kind: RowKind::Process,
        depth: 0,
        flags: RowFlags::default(),
        icon: None,
        name: "row",
    }
}

fn group(id: u32) -> TRow {
    TRow {
        kind: RowKind::GroupHeader,
        name: "Apps",
        ..plain(id)
    }
}

fn spec(id: ColumnId, kind: ColumnKind, width: f32) -> ColumnSpec {
    let label: SharedString = id.as_str().into();
    ColumnSpec {
        id,
        label: label.clone(),
        full_name: label.clone(),
        group: label,
        align: if kind == ColumnKind::Numeric {
            ColumnAlign::Right
        } else {
            ColumnAlign::Left
        },
        kind,
        sortable: true,
        template: None,
        default_width: px(width),
        tooltip: None,
        availability: Availability::Available,
        hideable: id != ColumnId::Name,
    }
}

/// A delegate with Name (Text) and CPU (Numeric) columns.
struct TDelegate {
    rows: Vec<TRow>,
    specs: Vec<ColumnSpec>,
    layout: ColumnLayout,
    guides: bool,
    empty: Option<SharedString>,
}

impl TDelegate {
    fn new(rows: Vec<TRow>) -> Self {
        Self::with_widths(rows, 248., 80.)
    }

    fn with_widths(rows: Vec<TRow>, name_w: f32, cpu_w: f32) -> Self {
        let specs = vec![
            spec(ColumnId::Name, ColumnKind::Text, name_w),
            spec(ColumnId::Cpu, ColumnKind::Numeric, cpu_w),
        ];
        let order = [ColumnId::Name, ColumnId::Cpu];
        let layout = ColumnLayout::defaults(TableId::Processes, &specs, &order, &order);
        Self {
            rows,
            specs,
            layout,
            guides: false,
            empty: None,
        }
    }
}

impl TreeTableDelegate for TDelegate {
    type RowId = u32;
    type Column = ColumnId;
    type Sort = SortSpec;

    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        self.rows[ix].id
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }
    fn row_kind(&self, ix: usize) -> RowKind {
        self.rows[ix].kind
    }
    fn row_depth(&self, ix: usize) -> u8 {
        self.rows[ix].depth
    }
    fn row_expandable(&self, ix: usize) -> Option<bool> {
        (self.rows[ix].kind == RowKind::GroupHeader).then_some(true)
    }
    fn row_flags(&self, ix: usize) -> RowFlags {
        self.rows[ix].flags
    }
    fn table_version(&self) -> u64 {
        1
    }
    fn columns(&self) -> &ColumnLayout {
        &self.layout
    }
    fn column_spec(&self, id: ColumnId) -> &ColumnSpec {
        self.specs.iter().find(|s| s.id == id).unwrap()
    }
    fn cell(&self, ix: usize, _: ColumnId, out: &mut CellOut) {
        out.bits = ix as u64;
        out.validity = Validity::Valid;
        if out.want_text {
            use std::fmt::Write as _;
            let _ = write!(out.text, "{ix}.0%");
        }
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let row = &self.rows[ix];
        NameCell {
            icon: match row.icon {
                Some(_) => NameIcon::Lucide(IconName::Binary),
                None => NameIcon::None,
            },
            icon_size: row.icon.unwrap_or(IconSlot::Small),
            primary: SharedString::new_static(row.name),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn header_total(&self, _: ColumnId, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SortSpec {
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false,
        }
    }
    fn selectable(&self, ix: usize) -> bool {
        self.rows[ix].kind != RowKind::GroupHeader
    }
    fn context_menu(
        &mut self,
        _: &[u32],
        _: ColumnId,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, _: ColumnId, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_columns_changed(&mut self, _: &ColumnLayout, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        self.empty.clone()
    }
    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }
    fn guide_lines(&self) -> bool {
        self.guides
    }
}

/// A fixed-size box holding the table, so the body has a known viewport.
struct TableHost {
    w: f32,
    h: f32,
    table: Entity<TlTreeTable<TDelegate>>,
}

impl Render for TableHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(self.w))
            .h(px(self.h))
            .flex()
            .flex_col()
            .child(self.table.clone())
    }
}

const HEADER_H: f32 = 28.;

/// Opens the table with a body `body_w` × `body_h` px (the one-line header sits above it).
fn open_table(
    cx: &mut TestAppContext,
    delegate: TDelegate,
    body_w: f32,
    body_h: f32,
) -> (Entity<TlTreeTable<TDelegate>>, &mut VisualTestContext) {
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        *out.borrow_mut() = Some(table.clone());
        TableHost {
            w: body_w,
            h: body_h + HEADER_H,
            table,
        }
    });
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    redraw(vcx);
    let table = slot.borrow_mut().take().unwrap();
    (table, vcx)
}

#[cfg(feature = "test-hooks")]
#[test]
fn tree_table_renders_visible_range_only() {
    let mut cx = cosmic_cx();
    let rows = (0..2000).map(plain).collect();
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 720.);
    let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
    assert!(built > 0 && built <= 27, "{built} rows built");

    // Half a row down: one more row is partly visible.
    vcx.update(|_, cx| {
        let handle = table.read(cx).scroll_handle().clone();
        ScrollbarHandle::set_offset(&handle, point(px(0.), px(-14.)));
    });
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
    assert!(built <= 27, "{built} rows built after scrolling");
}

#[test]
fn zebra_on_flattened_index_and_group_band() {
    let mut cx = cosmic_cx();
    let rows = vec![group(10), plain(11), plain(12)];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 200.);
    let theme = vcx.update(|_, cx| current(cx).clone());
    let fills: Vec<_> = vcx.update(|_, cx| {
        let t = table.read(cx);
        (0..3).map(|ix| t.row_background(ix, &theme)).collect()
    });
    assert_eq!(fills, vec![theme.header, theme.list_bg, theme.row_alt]);
    assert!(vcx.debug_bounds("TlGroupHeaderCell").is_some());
    assert!(
        vcx.debug_bounds("TlTreeTable-group-line").is_none(),
        "no top line on row 0"
    );
}

#[test]
fn selected_wins_over_hover() {
    let mut cx = cosmic_cx();
    let rows = (0..4).map(plain).collect();
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 200.);
    let theme = vcx.update(|_, cx| current(cx).clone());
    vcx.update(|_, cx| table.update(cx, |t, _| t.selection.click(2)));
    // Row 2 spans y 28 + 56 ..= 28 + 84.
    hover_at(vcx, 100., HEADER_H + 2. * 28. + 14.);
    let (hover, fill) = vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.hover_row(), t.row_background(2, &theme))
    });
    assert_eq!(hover, Some(2));
    assert_eq!(fill, theme.row_selected);

    hover_at(vcx, 100., HEADER_H + 28. + 14.);
    let (hover, fill) = vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.hover_row(), t.row_background(1, &theme))
    });
    assert_eq!(hover, Some(1));
    assert_eq!(
        fill, theme.row_hover,
        "hover shows on a row that is not selected"
    );
}

fn hover_at(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_event(MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
    redraw(cx);
}

#[test]
fn group_header_band_has_top_line_and_name_only() {
    let mut cx = cosmic_cx();
    let rows = vec![plain(0), plain(1), plain(2), plain(3), plain(4), group(5)];
    // One row tall, scrolled to row 5, so row 5 is the only row drawn.
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 28.);
    vcx.update(|_, cx| {
        table
            .read(cx)
            .scroll_handle()
            .scroll_to_item_strict(5, ScrollStrategy::Top)
    });
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    let line = vcx
        .debug_bounds("TlTreeTable-group-line")
        .expect("top line on the group row");
    assert_eq!(line.top(), body.top());
    assert!(line.size.height <= px(1.));
    assert!(vcx.debug_bounds("TlGroupHeaderCell").is_some());
    assert!(
        vcx.debug_bounds("TlCell-numeric").is_none(),
        "no metric cell"
    );
    assert!(vcx.debug_bounds("TlNameCell").is_none());
}

#[test]
fn name_cell_indent_by_depth() {
    let mut cx = cosmic_cx();
    let deep = TRow {
        depth: 2,
        icon: Some(IconSlot::Small),
        ..plain(1)
    };
    let (_, vcx) = open_table(&mut cx, TDelegate::new(vec![deep]), 600., 100.);
    let cell = vcx.debug_bounds("TlNameCell").expect("name cell");
    let text = vcx.debug_bounds("TlNameCell-primary").expect("primary");
    // 8 + 18 × 2 + 16 + 6 + 16.
    assert_eq!(text.left() - cell.left(), px(8. + 36. + 16. + 6. + 16.));

    let mut cx = cosmic_cx();
    let app = TRow {
        icon: Some(IconSlot::Large),
        ..plain(1)
    };
    let (_, vcx) = open_table(&mut cx, TDelegate::new(vec![app]), 600., 100.);
    let cell = vcx.debug_bounds("TlNameCell").expect("name cell");
    let text = vcx.debug_bounds("TlNameCell-primary").expect("primary");
    assert_eq!(text.left() - cell.left(), px(8. + 16. + 6. + 24.));
}

#[test]
fn connector_lines_only_when_enabled() {
    let child = |guides: bool| {
        let mut d = TDelegate::new(vec![
            plain(0),
            TRow {
                depth: 1,
                ..plain(1)
            },
        ]);
        d.guides = guides;
        d
    };
    let mut cx = cosmic_cx();
    let (_, vcx) = open_table(&mut cx, child(false), 600., 100.);
    assert!(vcx.debug_bounds("TlNameCell-guides").is_none());
    let mut cx = cosmic_cx();
    let (_, vcx) = open_table(&mut cx, child(true), 600., 100.);
    assert!(vcx.debug_bounds("TlNameCell-guides").is_some());

    // Depth 2, the level-0 line continues, last sibling: one full line, the elbow's vertical
    // half line and its horizontal line to the chevron.
    let flags = RowFlags {
        bits: RowFlags::LAST_SIBLING,
        ancestor_continues: 0b01,
    };
    let quads = guide_quads(2, flags, point(px(0.), px(0.)), px(28.), 1.0, 1.0);
    assert_eq!(quads.len(), 3);
    assert_eq!(quads[0].origin, point(px(16.), px(0.)));
    assert_eq!(quads[0].size, gpui_kit::size(px(1.), px(28.)));
    assert_eq!(quads[1].origin, point(px(34.), px(0.)));
    assert_eq!(quads[1].size.height, px(15.));
    assert_eq!(quads[2].origin, point(px(34.), px(14.)));
    assert_eq!(quads[2].size, gpui_kit::size(px(10.), px(1.)));
    // Not the last sibling: the elbow runs to the bottom. Depth 0 has no elbow.
    let open = guide_quads(
        2,
        RowFlags::default(),
        point(px(0.), px(0.)),
        px(28.),
        1.0,
        1.0,
    );
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].size.height, px(28.));
    assert!(
        guide_quads(
            0,
            RowFlags::default(),
            point(px(0.), px(0.)),
            px(28.),
            1.0,
            1.0
        )
        .is_empty()
    );
}

#[test]
fn horizontal_scrollbar_only_on_overflow() {
    let mut cx = cosmic_cx();
    let rows = (0..3).map(plain).collect();
    let (table, vcx) = open_table(
        &mut cx,
        TDelegate::with_widths(rows, 400., 1000.),
        1000.,
        200.,
    );
    assert!(vcx.update(|_, cx| table.read(cx).horizontal_overflow()));
    let label = vcx
        .debug_bounds("TlTreeTableHeader-label-cpu")
        .expect("cpu label");
    let cell = vcx.debug_bounds("TlCell-numeric").expect("cpu cell");
    assert_eq!(label.left(), cell.left());

    vcx.update(|_, cx| {
        let handle = table.read(cx).scroll_handle().clone();
        ScrollbarHandle::set_offset(&handle, point(px(-200.), px(0.)));
    });
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    assert_eq!(vcx.update(|_, cx| table.read(cx).x_offset()), px(200.));
    let label2 = vcx.debug_bounds("TlTreeTableHeader-label-cpu").unwrap();
    let cell2 = vcx.debug_bounds("TlCell-numeric").unwrap();
    assert_eq!(label2.left(), label.left() - px(200.));
    assert_eq!(cell2.left(), cell.left() - px(200.));

    let mut cx = cosmic_cx();
    let rows = (0..3).map(plain).collect();
    let (table, _) = open_table(
        &mut cx,
        TDelegate::with_widths(rows, 400., 500.),
        1000.,
        200.,
    );
    assert!(!cx.update(|cx| table.read(cx).horizontal_overflow()));
}

#[test]
fn stale_row_dims_metric_cells() {
    let stale = RowFlags {
        bits: RowFlags::STALE,
        ancestor_continues: 0,
    };
    assert_eq!(metric_cell_opacity(stale), 0.6);
    assert_eq!(metric_cell_opacity(RowFlags::default()), 1.0);

    // A stale row still draws its Name cell and its metric cells.
    let mut cx = cosmic_cx();
    let row = TRow {
        flags: stale,
        ..plain(1)
    };
    let (_, vcx) = open_table(&mut cx, TDelegate::new(vec![row]), 600., 100.);
    assert!(vcx.debug_bounds("TlNameCell").is_some());
    assert!(vcx.debug_bounds("TlCell-numeric").is_some());
}

#[test]
fn empty_text_keeps_header() {
    let mut cx = cosmic_cx();
    let mut d = TDelegate::new(Vec::new());
    d.empty = Some("No processes match the filter".into());
    let (_, vcx) = open_table(&mut cx, d, 600., 300.);
    let header = vcx.debug_bounds("TlTreeTableHeader").expect("header shows");
    assert_eq!(header.size.height, px(HEADER_H));
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    let empty = vcx.debug_bounds("TlTreeTable-empty").expect("empty text");
    assert_eq!(
        empty, body,
        "the text box fills the body and centres the text"
    );
}
