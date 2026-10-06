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
    Action, AnyView, App, AppContext as _, Context, Div, Entity, InteractiveElement as _,
    IntoElement, KeyContext, Modifiers, MouseMoveEvent, ParentElement, Pixels, Render,
    ScrollStrategy, SharedString, Styled, TestAppContext, TestDispatcher, VisualTestContext,
    Window, div, point, px,
};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::SortSpec;
use tl_model::validity::Validity;
use tl_ui::components::menu::TlMenu;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::validity::{GlyphContent, glyph_parts};
use tl_ui::table::actions::{
    CollapseAll, CollapseOrParent, CopyRow, CopyRowsAsTable, ExpandOrChild, MoveFocus,
    MoveFocusEdge, MoveFocusPage, OpenContextMenu, OpenProperties, SelectAll, TABLE_CONTEXT,
    ToggleExpand, ToggleSelectFocus,
};
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
    tl_ui::table::actions::bind_keys_table(cx);
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
    /// `None`: no chevron. Flipped by `on_toggle_expand`.
    expandable: Option<bool>,
}

fn plain(id: u32) -> TRow {
    TRow {
        id,
        kind: RowKind::Process,
        depth: 0,
        flags: RowFlags::default(),
        icon: None,
        name: "row",
        expandable: None,
    }
}

fn group(id: u32) -> TRow {
    TRow {
        kind: RowKind::GroupHeader,
        name: "Apps",
        expandable: Some(true),
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
    double_expands: bool,
    /// Every gesture the table reported, oldest first.
    toggled: Vec<u32>,
    activated: Vec<u32>,
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
            double_expands: false,
            toggled: Vec::new(),
            activated: Vec::new(),
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
        self.rows[ix].expandable
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
        self.specs
            .iter()
            .find(|s| s.id == id)
            .expect("spec for every column")
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
    fn on_toggle_expand(&mut self, id: u32, _: &mut Context<TlTreeTable<Self>>) {
        self.toggled.push(id);
        if let Some(row) = self.rows.iter_mut().find(|r| r.id == id) {
            row.expandable = row.expandable.map(|e| !e);
        }
    }
    fn on_sort(&mut self, _: ColumnId, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_columns_changed(&mut self, _: &ColumnLayout, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, id: u32, _: &mut Context<TlTreeTable<Self>>) {
        self.activated.push(id);
    }
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
    fn double_click_expands(&self) -> bool {
        self.double_expands
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
    let table = slot.borrow_mut().take().expect("table created");
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
    // Zebra on the flattened index (processes.md:285): the group header takes index 0.
    assert_eq!(fills, vec![theme.header, theme.row_alt, theme.list_bg]);
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
    // R-190: pad 20, indent 20 per level, gap 6, chevron 16, gap 6, icon 16, gap 6.
    assert_eq!(
        text.left() - cell.left(),
        px(20. + 40. + 6. + 16. + 6. + 16. + 6.)
    );

    let mut cx = cosmic_cx();
    let app = TRow {
        icon: Some(IconSlot::Large),
        ..plain(1)
    };
    let (_, vcx) = open_table(&mut cx, TDelegate::new(vec![app]), 600., 100.);
    let cell = vcx.debug_bounds("TlNameCell").expect("name cell");
    let text = vcx.debug_bounds("TlNameCell-primary").expect("primary");
    // R-190: an app row at depth 0 has its text at 78.
    assert_eq!(text.left() - cell.left(), px(78.));
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
    // Guide lines run through the chevron centre of each level: 20 + 20k + 14 (R-190).
    assert_eq!(quads[0].origin, point(px(34.), px(0.)));
    assert_eq!(quads[0].size, gpui_kit::size(px(1.), px(28.)));
    assert_eq!(quads[1].origin, point(px(54.), px(0.)));
    assert_eq!(quads[1].size.height, px(15.));
    // To the chevron's left edge at 20 + 40 + 6.
    assert_eq!(quads[2].origin, point(px(54.), px(14.)));
    assert_eq!(quads[2].size, gpui_kit::size(px(12.), px(1.)));
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
    let label2 = vcx
        .debug_bounds("TlTreeTableHeader-label-cpu")
        .expect("cpu label after scroll");
    let cell2 = vcx
        .debug_bounds("TlCell-numeric")
        .expect("cpu cell after scroll");
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

// ---------------------------------------------------------------------------------------------
// E18-007: mouse and keyboard selection, table Actions and the Table keymap.
// ---------------------------------------------------------------------------------------------

/// A row at `depth` with a chevron (`Some(expanded)`) or without one.
fn node(id: u32, depth: u8, expandable: Option<bool>) -> TRow {
    TRow {
        depth,
        expandable,
        ..plain(id)
    }
}

fn focus_table(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>) {
    let handle = vcx.update(|_, cx| table.read(cx).focus_handle().clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    redraw(vcx);
}

fn keys(vcx: &mut VisualTestContext, keys: &str) {
    vcx.simulate_keystrokes(keys);
    redraw(vcx);
}

/// The selected ids, sorted, and the focus.
fn sel(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> (Vec<u32>, Option<u32>) {
    vcx.update(|_, cx| {
        let s = &table.read(cx).selection;
        let mut ids: Vec<u32> = s.ids().collect();
        ids.sort_unstable();
        (ids, s.focus())
    })
}

fn gestures(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> (Vec<u32>, Vec<u32>) {
    vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        (d.toggled.clone(), d.activated.clone())
    })
}

fn select(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>, id: u32) {
    vcx.update(|_, cx| table.update(cx, |t, _| t.selection.click(id)));
    redraw(vcx);
}

#[test]
fn keys_move_focus_and_skip_group_headers() {
    let mut cx = cosmic_cx();
    let rows = vec![
        group(10),
        plain(11),
        plain(12),
        plain(13),
        group(14),
        plain(15),
    ];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    focus_table(vcx, &table);

    // No selection: the first Down selects the first selectable row, past the group header.
    keys(vcx, "down");
    assert_eq!(sel(vcx, &table), (vec![11], Some(11)));
    keys(vcx, "down");
    assert_eq!(
        sel(vcx, &table),
        (vec![12], Some(12)),
        "Down from B selects only C"
    );
    keys(vcx, "up");
    assert_eq!(sel(vcx, &table), (vec![11], Some(11)));

    keys(vcx, "shift-down shift-down");
    assert_eq!(sel(vcx, &table), (vec![11, 12, 13], Some(13)));
    // Down from 13 skips the group header at 14.
    keys(vcx, "down");
    assert_eq!(sel(vcx, &table), (vec![15], Some(15)));
    keys(vcx, "home");
    assert_eq!(sel(vcx, &table), (vec![11], Some(11)));
    keys(vcx, "shift-end");
    assert_eq!(sel(vcx, &table), (vec![11, 12, 13, 15], Some(15)));
    keys(vcx, "end");
    assert_eq!(sel(vcx, &table), (vec![15], Some(15)));
    keys(vcx, "pageup");
    assert_eq!(
        sel(vcx, &table),
        (vec![11], Some(11)),
        "300 px fits 10 rows, so PgUp clamps"
    );
    keys(vcx, "pagedown");
    assert_eq!(sel(vcx, &table), (vec![15], Some(15)));
    keys(vcx, "ctrl-a");
    assert_eq!(
        sel(vcx, &table).0,
        vec![11, 12, 13, 15],
        "Ctrl+A skips group headers"
    );

    // A 3-row body pages by 2 rows.
    let mut cx = cosmic_cx();
    let rows = (0..10).map(plain).collect();
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 84.);
    focus_table(vcx, &table);
    keys(vcx, "down pagedown");
    assert_eq!(sel(vcx, &table), (vec![2], Some(2)));
    keys(vcx, "shift-pagedown");
    assert_eq!(sel(vcx, &table), (vec![2, 3, 4], Some(4)));
}

#[test]
fn right_left_expand_collapse_parent() {
    let mut cx = cosmic_cx();
    let rows = vec![node(1, 0, Some(false)), node(2, 1, None), node(3, 1, None)];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    focus_table(vcx, &table);
    select(vcx, &table, 1);

    keys(vcx, "right");
    assert_eq!(
        gestures(vcx, &table).0,
        vec![1],
        "a collapsed parent expands once"
    );
    assert_eq!(sel(vcx, &table), (vec![1], Some(1)));
    keys(vcx, "right");
    assert_eq!(
        sel(vcx, &table),
        (vec![2], Some(2)),
        "Right again moves to the first child"
    );
    assert_eq!(gestures(vcx, &table).0, vec![1]);
    keys(vcx, "right");
    assert_eq!(
        sel(vcx, &table),
        (vec![2], Some(2)),
        "Right on a leaf does nothing"
    );

    keys(vcx, "down left");
    assert_eq!(
        sel(vcx, &table),
        (vec![1], Some(1)),
        "Left on a leaf focuses its parent"
    );
    keys(vcx, "left");
    assert_eq!(
        gestures(vcx, &table).0,
        vec![1, 1],
        "Left on an expanded row collapses it"
    );
    keys(vcx, "left");
    assert_eq!(
        gestures(vcx, &table).0,
        vec![1, 1],
        "Left on a collapsed root does nothing"
    );
    assert_eq!(sel(vcx, &table), (vec![1], Some(1)));
}

#[test]
fn left_on_depth1_collapses_group() {
    let mut cx = cosmic_cx();
    let rows = vec![group(10), node(11, 1, None), node(12, 1, None)];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    focus_table(vcx, &table);
    select(vcx, &table, 12);
    keys(vcx, "left");
    assert_eq!(gestures(vcx, &table).0, vec![10], "the group collapses");
    assert_eq!(
        sel(vcx, &table),
        (vec![12], Some(12)),
        "the selection stays"
    );
}

#[test]
fn enter_activates_space_toggles() {
    let mut cx = cosmic_cx();
    let rows = vec![group(10), node(11, 1, Some(false)), node(12, 2, None)];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    focus_table(vcx, &table);

    // No focus row: Enter does nothing.
    keys(vcx, "enter");
    assert_eq!(gestures(vcx, &table), (vec![], vec![]));

    // A focused group header toggles and is never activated (D-008).
    vcx.update(|_, cx| table.update(cx, |t, _| t.selection.set_focus_only(10)));
    keys(vcx, "enter");
    assert_eq!(gestures(vcx, &table), (vec![10], vec![]));

    select(vcx, &table, 11);
    keys(vcx, "enter alt-enter ctrl-i");
    assert_eq!(gestures(vcx, &table), (vec![10], vec![11, 11, 11]));
    keys(vcx, "space");
    assert_eq!(
        gestures(vcx, &table).0,
        vec![10, 11],
        "Space toggles a row with children"
    );
    select(vcx, &table, 12);
    keys(vcx, "space");
    assert_eq!(
        gestures(vcx, &table).0,
        vec![10, 11],
        "Space on a leaf does nothing"
    );

    // Ctrl+Space takes the focus row out of the set and the focus ring shows on it.
    assert!(vcx.debug_bounds("TlTreeTable-focus-row").is_none());
    keys(vcx, "ctrl-space");
    assert_eq!(sel(vcx, &table), (vec![], Some(12)));
    let ring = vcx
        .debug_bounds("TlTreeTable-focus-row")
        .expect("focus ring");
    assert_eq!(ring.top(), px(HEADER_H + 2. * 28.));
    assert_eq!(ring.size.height, px(28.));
    keys(vcx, "ctrl-space");
    assert_eq!(sel(vcx, &table), (vec![12], Some(12)));
    assert!(
        vcx.debug_bounds("TlTreeTable-focus-row").is_none(),
        "no ring on a selected focus row"
    );
}

/// The centre of row `ix` at `x`.
fn row_at(ix: usize, x: f32) -> gpui_kit::Point<Pixels> {
    point(px(x), px(HEADER_H + ix as f32 * 28. + 14.))
}

fn click(vcx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>, modifiers: Modifiers) {
    vcx.simulate_click(at, modifiers);
    redraw(vcx);
}

fn double_click(vcx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>) {
    vcx.simulate_click(at, Modifiers::none());
    for (click_count, down) in [(2, true), (2, false)] {
        if down {
            vcx.simulate_event(gpui_kit::MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: gpui_kit::MouseButton::Left,
                click_count,
                first_mouse: false,
            });
        } else {
            vcx.simulate_event(gpui_kit::MouseUpEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: gpui_kit::MouseButton::Left,
                click_count,
            });
        }
    }
    redraw(vcx);
}

#[test]
fn mouse_gesture_matrix() {
    let mut cx = cosmic_cx();
    let rows = vec![
        plain(1),
        plain(2),
        plain(3),
        plain(4),
        group(5),
        node(6, 0, Some(false)),
        plain(7),
    ];
    let (table, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    let x = 150.;

    click(vcx, row_at(0, x), Modifiers::none());
    assert_eq!(sel(vcx, &table), (vec![1], Some(1)));
    let focused = vcx.update(|window, cx| table.read(cx).focus_handle().is_focused(window));
    assert!(focused, "a click focuses the table");
    click(vcx, row_at(2, x), Modifiers::control());
    assert_eq!(sel(vcx, &table), (vec![1, 3], Some(3)));
    click(vcx, row_at(1, x), Modifiers::shift());
    assert_eq!(
        sel(vcx, &table),
        (vec![2, 3], Some(2)),
        "Shift: anchor 3 to 2"
    );
    click(vcx, row_at(3, x), Modifiers::control_shift());
    assert_eq!(
        sel(vcx, &table),
        (vec![2, 3, 4], Some(4)),
        "Ctrl+Shift adds anchor..row"
    );

    // A group header toggles on any click and never enters the selection, with any modifier.
    for modifiers in [
        Modifiers::none(),
        Modifiers::control(),
        Modifiers::shift(),
        Modifiers::control_shift(),
    ] {
        click(vcx, row_at(4, x), modifiers);
        assert_eq!(sel(vcx, &table), (vec![2, 3, 4], Some(4)));
    }
    assert_eq!(gestures(vcx, &table).0, vec![5, 5, 5, 5]);

    // The chevron of the unselected row 6: chevron box at 20 + 6, hit area 24..44.
    click(vcx, row_at(5, 34.), Modifiers::none());
    assert_eq!(
        gestures(vcx, &table).0,
        vec![5, 5, 5, 5, 6],
        "the row expands"
    );
    assert_eq!(
        sel(vcx, &table),
        (vec![2, 3, 4], Some(4)),
        "selection unchanged"
    );
    // Left of the hit area is the row itself.
    click(vcx, row_at(5, 20.), Modifiers::none());
    assert_eq!(sel(vcx, &table), (vec![6], Some(6)));

    // Double-click: system tables activate.
    double_click(vcx, row_at(6, x));
    assert_eq!(gestures(vcx, &table).1, vec![7]);
    assert_eq!(sel(vcx, &table), (vec![7], Some(7)));
    // A double-click on a group header toggles it once.
    double_click(vcx, row_at(4, x));
    assert_eq!(gestures(vcx, &table).0, vec![5, 5, 5, 5, 6, 5]);

    // Processes (R-216): a row with children toggles, a leaf does nothing, nothing activates.
    vcx.update(|_, cx| table.update(cx, |t, _| t.delegate.double_expands = true));
    double_click(vcx, row_at(5, x));
    double_click(vcx, row_at(6, x));
    let (toggled, activated) = gestures(vcx, &table);
    assert_eq!(toggled, vec![5, 5, 5, 5, 6, 5, 6]);
    assert_eq!(activated, vec![7]);

    // Below the last row (7 rows end at y 28 + 196).
    click(vcx, point(px(x), px(HEADER_H + 250.)), Modifiers::none());
    assert_eq!(sel(vcx, &table), (vec![], None));
}

#[test]
fn chevron_tooltip_on_hovered_row() {
    let mut cx = cosmic_cx();
    cx.update(|cx| tl_ui::i18n::init_strings(cx, "en-US"));
    let rows = vec![node(1, 0, Some(false)), node(2, 0, Some(true))];
    let (_, vcx) = open_table(&mut cx, TDelegate::new(rows), 600., 300.);
    // One hit area per row with a chevron.
    assert!(vcx.debug_bounds("TlTreeTable-chevron").is_some());
    // Row 0's chevron hit area spans x 24..44.
    hover_at(vcx, 34., HEADER_H + 14.);
    hover_at(vcx, 34., HEADER_H + 14.);
    vcx.executor().advance_clock(DELAY_CONTROL * 2);
    redraw(vcx);
    assert!(
        vcx.debug_bounds("TlTooltip").is_some(),
        "Expand shows on the chevron"
    );
}

/// A page that holds the table and listens for the Actions the table leaves to its owner.
struct OwnerHost {
    table: Entity<TlTreeTable<TDelegate>>,
    hits: Rc<Cell<u32>>,
}

impl Render for OwnerHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let copy = self.hits.clone();
        let collapse = self.hits.clone();
        div()
            .w(px(600.))
            .h(px(300.))
            .flex()
            .flex_col()
            .on_action(move |_: &CopyRow, _, _| copy.set(copy.get() + 1))
            .on_action(move |_: &CollapseAll, _, _| collapse.set(collapse.get() + 10))
            .child(self.table.clone())
    }
}

#[test]
fn unhandled_actions_propagate_to_owner() {
    let mut cx = cosmic_cx();
    let hits = Rc::new(Cell::new(0));
    let slot = Rc::new(std::cell::RefCell::new(None));
    let (out, page_hits) = (slot.clone(), hits.clone());
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let rows = (0..3).map(plain).collect();
        let table = cx.new(|cx| TlTreeTable::new(TDelegate::new(rows), window, cx));
        *out.borrow_mut() = Some(table.clone());
        OwnerHost {
            table,
            hits: page_hits,
        }
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    let table = slot.borrow_mut().take().expect("table created");
    focus_table(vcx, &table);
    keys(vcx, "down ctrl-c");
    assert_eq!(hits.get(), 1, "the page receives CopyRow");
    assert_eq!(
        sel(vcx, &table),
        (vec![0], Some(0)),
        "the table handled Down"
    );
    keys(vcx, "ctrl-l");
    assert_eq!(hits.get(), 11, "the page receives CollapseAll");
}

#[test]
fn table_keymap_matches_table_b() {
    let mv = |delta, extend| -> Box<dyn Action> { Box::new(MoveFocus { delta, extend }) };
    let page = |down, extend| -> Box<dyn Action> { Box::new(MoveFocusPage { down, extend }) };
    let edge = |end, extend| -> Box<dyn Action> { Box::new(MoveFocusEdge { end, extend }) };
    let table: Vec<(&str, Box<dyn Action>)> = vec![
        ("up", mv(-1, false)),
        ("down", mv(1, false)),
        ("shift-up", mv(-1, true)),
        ("shift-down", mv(1, true)),
        ("pageup", page(false, false)),
        ("pagedown", page(true, false)),
        ("shift-pageup", page(false, true)),
        ("shift-pagedown", page(true, true)),
        ("home", edge(false, false)),
        ("end", edge(true, false)),
        ("shift-home", edge(false, true)),
        ("shift-end", edge(true, true)),
        ("ctrl-a", Box::new(SelectAll)),
        ("right", Box::new(ExpandOrChild)),
        ("left", Box::new(CollapseOrParent)),
        ("space", Box::new(ToggleExpand)),
        ("ctrl-space", Box::new(ToggleSelectFocus)),
        ("enter", Box::new(OpenProperties)),
        ("alt-enter", Box::new(OpenProperties)),
        ("ctrl-i", Box::new(OpenProperties)),
        ("ctrl-c", Box::new(CopyRow)),
        ("ctrl-shift-c", Box::new(CopyRowsAsTable)),
        ("ctrl-l", Box::new(CollapseAll)),
        ("shift-f10", Box::new(OpenContextMenu)),
        ("menu", Box::new(OpenContextMenu)),
    ];
    let cx = cosmic_cx();
    cx.update(|cx| {
        let in_table = [KeyContext::parse(TABLE_CONTEXT).expect("context")];
        let elsewhere = [KeyContext::parse("Global").expect("context")];
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        for (keys, action) in &table {
            let found: Vec<_> = keymap
                .bindings_for_action(&**action)
                .filter(|b| {
                    let typed: Vec<String> = b.keystrokes().iter().map(|k| k.unparse()).collect();
                    typed.join(" ") == *keys
                })
                .collect();
            assert_eq!(found.len(), 1, "{keys} is bound once to {action:?}");
            let predicate = found[0].predicate().expect("bound in a context");
            assert!(predicate.eval(&in_table), "{keys} works in Table");
            assert!(!predicate.eval(&elsewhere), "{keys} is Table only");
        }
    });
}
