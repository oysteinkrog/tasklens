//! Column header tests in a real window (UI-PRC-005, ADR-013): the two-line header with totals,
//! the sort arrow and the sort click (E18-009); column resize and fit (E18-010); column reorder
//! drag (E18-011); the header menu (E18-012). Later column beads append their own sections.
//!
//! The header texts are measured, so the window uses the real text system with the bundled fonts.

use std::rc::Rc;

use gpui_kit::Role;
use gpui_kit::base::Root;
use gpui_kit::base::test_support::snapshots;
use gpui_kit::{
    AnyView, AppContext as _, Context, CursorStyle, Entity, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    SharedString, Styled, TestAppContext, TestDispatcher, VisualTestContext, Window, div, point,
    px,
};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::SortSpec;
use tl_model::validity::Validity;
use tl_ui::components::menu::{TlMenu, context_menu_layer};
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::validity::{GlyphContent, glyph_parts};
use tl_ui::table::actions::{ResetColumns, ResizeColumnsToFit};
use tl_ui::table::cells::CellOut;
use tl_ui::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId, min_width, next_sort,
};
use tl_ui::table::header::{
    GHOST_OPACITY, HeaderMenuItem, HeaderMenuRow, UNSORTABLE_TOOLTIP, ghost_fill, header_menu_rows,
    header_tooltip, resize_behavior_rows,
};
use tl_ui::table::tree_table::{
    ColumnResizeMode, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable,
    TreeTableDelegate,
};
use tl_ui::text::fonts::load_bundled_fonts;
use tl_ui::text::metrics::TextMetricsCache;
use tl_ui::theme::{current, metrics};

fn cosmic_cx() -> TestAppContext {
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    cx.update(|cx| {
        let report = load_bundled_fonts(cx);
        assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        gpui_kit::init(cx);
        tl_ui::table::actions::bind_keys_table(cx);
        tl_ui::components::menu::bind_keys(cx);
        tl_ui::components::toast::init(cx);
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

fn spec(id: ColumnId, kind: ColumnKind, width: f32, label: &'static str) -> ColumnSpec {
    ColumnSpec {
        id,
        label: label.into(),
        full_name: label.into(),
        group: label.into(),
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

/// Columns and their x ranges: Name 0..200, User 200..300, PID 300..380, CPU 380..460,
/// PSS 460..540 (unsortable).
const ORDER: [ColumnId; 5] = [
    ColumnId::Name,
    ColumnId::User,
    ColumnId::Pid,
    ColumnId::Cpu,
    ColumnId::Pss,
];

/// A delegate with three rows, the columns above and settable header totals.
struct HDelegate {
    specs: Vec<ColumnSpec>,
    layout: ColumnLayout,
    sort: SortSpec,
    lines: HeaderLines,
    /// Header totals: (column, bits, text, validity). Other columns have none.
    totals: Vec<(ColumnId, u64, &'static str, Validity)>,
    /// Every column `on_sort` received, oldest first.
    sorted: Vec<ColumnId>,
    /// Cell texts by column. Other columns show "1".
    texts: Vec<(ColumnId, &'static str)>,
    /// Calls of `on_columns_changed`.
    columns_changed: usize,
    /// `columns_fixed`.
    fixed: bool,
}

impl HDelegate {
    fn new() -> Self {
        let mut pss = spec(ColumnId::Pss, ColumnKind::Numeric, 80., "PSS");
        pss.sortable = false;
        let specs = vec![
            spec(ColumnId::Name, ColumnKind::Text, 200., "Name"),
            spec(ColumnId::User, ColumnKind::Text, 100., "User"),
            spec(ColumnId::Pid, ColumnKind::Numeric, 80., "PID"),
            spec(ColumnId::Cpu, ColumnKind::Numeric, 80., "CPU"),
            pss,
        ];
        let layout = ColumnLayout::defaults(TableId::Processes, &specs, &ORDER, &ORDER);
        Self {
            specs,
            layout,
            sort: SortSpec {
                column: ColumnId::Pid,
                ascending: true,
            },
            lines: HeaderLines::Two,
            totals: vec![(ColumnId::Cpu, 119, "11.9%", Validity::Valid)],
            sorted: Vec::new(),
            texts: Vec::new(),
            columns_changed: 0,
            fixed: false,
        }
    }
}

impl TreeTableDelegate for HDelegate {
    type RowId = u32;
    type Column = ColumnId;
    type Sort = SortSpec;

    fn row_count(&self) -> usize {
        3
    }
    fn row_id(&self, ix: usize) -> u32 {
        ix as u32
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        ((id as usize) < 3).then_some(id as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Process
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
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
    fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut) {
        out.bits = ix as u64;
        if out.want_text {
            let text = self.texts.iter().find(|t| t.0 == col).map_or("1", |t| t.1);
            out.text.push_str(text);
        }
    }
    fn name_cell(&self, _: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::new_static("row"),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool {
        let Some(&(_, bits, text, validity)) = self.totals.iter().find(|t| t.0 == col) else {
            return false;
        };
        out.bits = bits;
        out.validity = validity;
        if out.want_text {
            out.text.push_str(text);
        }
        true
    }
    fn sort(&self) -> SortSpec {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
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
    fn on_sort(&mut self, col: ColumnId, _: &mut Context<TlTreeTable<Self>>) {
        self.sorted.push(col);
        let kind = self.column_spec(col).kind;
        self.sort = next_sort(&self.sort, col, kind);
    }
    fn on_columns_changed(&mut self, layout: &ColumnLayout, _: &mut Context<TlTreeTable<Self>>) {
        self.layout = layout.clone();
        self.columns_changed += 1;
    }
    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        None
    }
    fn header_lines(&self) -> HeaderLines {
        self.lines
    }
    fn columns_fixed(&self) -> bool {
        self.fixed
    }
}

struct Host {
    table: Entity<TlTreeTable<HDelegate>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(600.))
            .h(px(300.))
            .flex()
            .flex_col()
            .child(self.table.clone())
    }
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(
    cx: &mut TestAppContext,
    delegate: HDelegate,
) -> (Entity<TlTreeTable<HDelegate>>, &mut VisualTestContext) {
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        *out.borrow_mut() = Some(table.clone());
        Host { table }
    });
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    redraw(vcx);
    let table = slot.borrow_mut().take().expect("table created");
    (table, vcx)
}

fn hover(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_event(MouseMoveEvent {
        position: at,
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
    redraw(vcx);
}

fn click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_click(at, Modifiers::none());
    redraw(vcx);
}

fn sort_of(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<HDelegate>>) -> SortSpec {
    vcx.update(|_, cx| table.read(cx).delegate.sort)
}

/// Line 1 of a header cell, by its x centre.
fn header_at(x: f32) -> Point<Pixels> {
    point(px(x), px(12.))
}

#[test]
fn header_sort_cycle_numeric_desc_first() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    click(vcx, header_at(420.));
    assert_eq!(
        sort_of(vcx, &table),
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false
        },
        "a numeric column sorts descending first"
    );
    // Line 2 is part of the cell too.
    click(vcx, point(px(420.), px(32.)));
    assert_eq!(
        sort_of(vcx, &table),
        SortSpec {
            column: ColumnId::Cpu,
            ascending: true
        },
        "a second click flips"
    );
    let sorted = vcx.update(|_, cx| table.read(cx).delegate.sorted.clone());
    assert_eq!(sorted, vec![ColumnId::Cpu, ColumnId::Cpu]);
}

#[test]
fn header_sort_cycle_text_asc_first() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    click(vcx, header_at(250.));
    assert_eq!(
        sort_of(vcx, &table),
        SortSpec {
            column: ColumnId::User,
            ascending: true
        }
    );
    click(vcx, header_at(250.));
    assert!(!sort_of(vcx, &table).ascending);
}

#[test]
fn header_drag_does_not_sort() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    let down = header_at(420.);
    let moved = point(down.x + px(6.), down.y);
    vcx.simulate_event(MouseDownEvent {
        position: down,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    redraw(vcx);
    vcx.simulate_event(MouseMoveEvent {
        position: moved,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::none(),
    });
    redraw(vcx);
    vcx.simulate_event(MouseUpEvent {
        position: moved,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 1,
    });
    redraw(vcx);
    let sorted = vcx.update(|_, cx| table.read(cx).delegate.sorted.clone());
    assert!(sorted.is_empty(), "a 6 px drag is not a click");
    assert_eq!(sort_of(vcx, &table).column, ColumnId::Pid);
}

#[test]
fn header_line2_blank_for_text_columns() {
    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    d.totals.push((ColumnId::Pss, 0, "", Validity::Unsupported));
    let (table, vcx) = open(&mut cx, d);

    let cpu = vcx.update(|_, cx| table.update(cx, |t, _| t.header_total_text(ColumnId::Cpu)));
    assert_eq!(cpu, Some(("11.9%".into(), Validity::Valid)));
    let line = vcx
        .debug_bounds("TlTreeTableHeader-total-cpu")
        .expect("cpu line 2");
    // The full cell width, so the right-aligned text ends at the cell's right padding edge.
    assert_eq!(line.left(), px(380.));
    assert_eq!(line.right(), px(460.));
    assert_eq!(line.top(), px(22.));
    assert_eq!(line.size.height, px(18.));

    let name = vcx.update(|_, cx| table.update(cx, |t, _| t.header_total_text(ColumnId::Name)));
    assert_eq!(name, None);
    assert!(
        vcx.debug_bounds("TlTreeTableHeader-total-name").is_none(),
        "no line 2 on Name"
    );

    // Unsupported draws the `n/a` glyph.
    let pss = vcx.update(|_, cx| table.update(cx, |t, _| t.header_total_text(ColumnId::Pss)));
    let (text, validity) = pss.expect("pss total");
    assert_eq!(validity, Validity::Unsupported);
    assert!(vcx.debug_bounds("TlTreeTableHeader-total-pss").is_some());
    let theme = vcx.update(|_, cx| current(cx).clone());
    match glyph_parts(validity, &text, false, &theme).0 {
        GlyphContent::Text(t) => assert_eq!(t.as_ref(), "n/a"),
        GlyphContent::Lock => panic!("Unsupported shows n/a"),
    }

    // Totals format once and re-format only when the value changes.
    let misses = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| table.read(cx).header_state().totals.stats().1)
    };
    let before = misses(vcx);
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    assert_eq!(misses(vcx), before, "unchanged totals are not re-formatted");
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.totals[0] = (ColumnId::Cpu, 120, "12.0%", Validity::Valid);
            cx.notify();
        })
    });
    redraw(vcx);
    assert_eq!(misses(vcx), before + 1, "a changed total formats once");
    let cpu = vcx.update(|_, cx| table.update(cx, |t, _| t.header_total_text(ColumnId::Cpu)));
    assert_eq!(cpu, Some(("12.0%".into(), Validity::Valid)));
}

#[test]
fn unsortable_header_no_hover_no_sort() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    let theme = vcx.update(|_, cx| current(cx).clone());
    let fill = |vcx: &mut VisualTestContext, col: ColumnId| {
        vcx.update(|_, cx| {
            table
                .read(cx)
                .header_state()
                .cells
                .iter()
                .find(|c| c.id == col)
                .expect("visible cell")
                .fill
        })
    };

    // A sortable cell gets the hover fill.
    hover(vcx, header_at(420.));
    assert_eq!(fill(vcx, ColumnId::Cpu), Some(theme.row_hover));

    let pss = header_at(500.);
    hover(vcx, pss);
    hover(vcx, pss);
    let hovered = vcx.update(|_, cx| table.read(cx).header_state().hover);
    assert_eq!(hovered, Some(ColumnId::Pss));
    assert_eq!(fill(vcx, ColumnId::Pss), None, "no hover fill");
    assert_eq!(fill(vcx, ColumnId::Cpu), None);

    vcx.executor().advance_clock(DELAY_CONTROL / 2);
    redraw(vcx);
    assert!(vcx.debug_bounds("TlTooltip").is_none(), "not before 500 ms");
    vcx.executor().advance_clock(DELAY_CONTROL);
    redraw(vcx);
    assert!(
        vcx.debug_bounds("TlTooltip").is_some(),
        "the unsortable tooltip shows"
    );
    let tip =
        vcx.update(|_, cx| header_tooltip(table.read(cx).delegate.column_spec(ColumnId::Pss)));
    assert_eq!(tip.as_deref(), Some(UNSORTABLE_TOOLTIP));
    assert_eq!(UNSORTABLE_TOOLTIP, "Only measured for visible rows");

    click(vcx, pss);
    let sorted = vcx.update(|_, cx| table.read(cx).delegate.sorted.clone());
    assert!(sorted.is_empty(), "no sort on an unsortable column");
    assert_eq!(fill(vcx, ColumnId::Pss), None, "no pressed fill either");
}

#[test]
fn header_tooltip_falls_back_to_full_name() {
    let mut s = spec(ColumnId::Memory, ColumnKind::Numeric, 80., "Memory");
    assert_eq!(header_tooltip(&s), None, "full name equals label");
    s.full_name = "Working set private".into();
    assert_eq!(header_tooltip(&s).as_deref(), Some("Working set private"));
    s.tooltip = Some("Private memory (RssAnon).".into());
    assert_eq!(
        header_tooltip(&s).as_deref(),
        Some("Private memory (RssAnon).")
    );
}

#[test]
fn sort_arrow_position_right_aligned() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    let arrow = vcx.update(|_, cx| table.read(cx).header_state().arrow);
    let (col, ascending, bounds) = arrow.expect("an arrow on the sort column");
    assert_eq!((col, ascending), (ColumnId::Pid, true));
    // PID spans 300..380: the arrow ends at the right padding edge, centred in line 1 (y 4..22).
    assert_eq!(bounds.right(), px(372.));
    assert_eq!(bounds.size.width, px(12.));
    assert_eq!(bounds.top(), px(7.));

    // The label box ends 14 px left of where an unsorted label ends.
    let pid = vcx
        .debug_bounds("TlTreeTableHeader-label-pid")
        .expect("pid label");
    let cpu = vcx
        .debug_bounds("TlTreeTableHeader-label-cpu")
        .expect("cpu label");
    assert_eq!(pid.right(), px(380. - 14.));
    assert_eq!(cpu.right(), px(460.));
    assert_eq!(pid.top(), px(4.));
    assert_eq!(pid.size.height, px(18.));

    // A left-aligned sort column puts the arrow 2 px after its label.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.sort = SortSpec {
                column: ColumnId::User,
                ascending: false,
            };
            cx.notify();
        })
    });
    redraw(vcx);
    let arrow = vcx.update(|_, cx| table.read(cx).header_state().arrow);
    let (col, ascending, bounds) = arrow.expect("arrow on User");
    assert_eq!((col, ascending), (ColumnId::User, false));
    assert!(
        bounds.left() > px(210.) && bounds.right() <= px(292.),
        "after the label, inside the cell: {bounds:?}"
    );
}

#[test]
fn header_one_line_mode_height_28() {
    let mut cx = cosmic_cx();
    let (_, vcx) = open(&mut cx, HDelegate::new());
    let header = vcx.debug_bounds("TlTreeTableHeader").expect("header");
    assert_eq!(header.size.height, px(44.));

    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    d.lines = HeaderLines::One;
    let (table, vcx) = open(&mut cx, d);
    let header = vcx.debug_bounds("TlTreeTableHeader").expect("header");
    assert_eq!(header.size.height, px(28.));
    assert!(
        vcx.debug_bounds("TlTreeTableHeader-total-cpu").is_none(),
        "no line 2"
    );
    let label = vcx
        .debug_bounds("TlTreeTableHeader-label-cpu")
        .expect("cpu label");
    assert_eq!((label.top(), label.size.height), (px(0.), px(28.)));
    // Sorting still works in one-line mode.
    click(vcx, header_at(420.));
    let sort = vcx.update(|_, cx| table.read(cx).delegate.sort);
    assert_eq!(sort.column, ColumnId::Cpu);
}

fn mouse_down(vcx: &mut VisualTestContext, at: Point<Pixels>, click_count: usize) {
    vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count,
        first_mouse: false,
    });
    redraw(vcx);
}

fn mouse_up(vcx: &mut VisualTestContext, at: Point<Pixels>, click_count: usize) {
    vcx.simulate_event(MouseUpEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count,
    });
    redraw(vcx);
}

fn drag_move(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_event(MouseMoveEvent {
        position: at,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::none(),
    });
    redraw(vcx);
}

/// The width of `col` in the delegate's stored layout (100%).
fn stored_width(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<HDelegate>>,
    col: ColumnId,
) -> Pixels {
    vcx.update(|_, cx| table.read(cx).delegate.layout.widths[&col])
}

fn changed_count(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<HDelegate>>) -> usize {
    vcx.update(|_, cx| table.read(cx).delegate.columns_changed)
}

#[test]
fn boundary_band_sets_resize_cursor() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    let cursor =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| table.read(cx).header_state().cursor());
    assert_eq!(cursor(vcx), CursorStyle::Arrow);
    // 3 px left and right of the Name and User boundary at x 200, and after the last column.
    for x in [197., 203., 300., 543.] {
        hover(vcx, header_at(x));
        assert_eq!(cursor(vcx), CursorStyle::ResizeColumn, "x {x}");
        let hovered = vcx.update(|_, cx| table.read(cx).header_state().hover);
        assert_eq!(hovered, None, "a band is not a cell, x {x}");
    }
    for x in [195., 250., 420., 545.] {
        hover(vcx, header_at(x));
        assert_eq!(cursor(vcx), CursorStyle::Arrow, "x {x}");
    }
}

#[test]
fn drag_persists_once_on_release() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    #[cfg(feature = "test-hooks")]
    let before = vcx.update(|_, cx| table.read(cx).text_cache_stats().1);

    mouse_down(vcx, header_at(200.), 1);
    for i in 1..=10 {
        drag_move(vcx, header_at(200. + 4. * i as f32));
        let live = vcx.update(|_, cx| table.read(cx).columns().width(ColumnId::Name));
        assert_eq!(live, px(200. + 4. * i as f32), "live width after move {i}");
    }
    assert_eq!(
        changed_count(vcx, &table),
        0,
        "nothing persists during the drag"
    );
    assert_eq!(stored_width(vcx, &table, ColumnId::Name), px(200.));
    let cursor = vcx.update(|_, cx| table.read(cx).header_state().cursor());
    assert_eq!(cursor, CursorStyle::ResizeColumn);
    #[cfg(feature = "test-hooks")]
    {
        let misses = vcx.update(|_, cx| table.read(cx).text_cache_stats().1);
        assert_eq!(misses, before, "a drag re-formats no cell text");
    }

    mouse_up(vcx, header_at(240.), 1);
    assert_eq!(changed_count(vcx, &table), 1);
    assert_eq!(stored_width(vcx, &table, ColumnId::Name), px(240.));
    // Proportional: the columns to the right share the rest of the 600 px body.
    let total = vcx.update(|_, cx| table.read(cx).columns().total_width());
    assert!((total.as_f32() - 600.).abs() < 0.01, "total {total:?}");
    // The release did not sort.
    let sorted = vcx.update(|_, cx| table.read(cx).delegate.sorted.clone());
    assert!(sorted.is_empty());
}

#[test]
fn drag_last_column_mode_and_minimum() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    vcx.update(|_, cx| {
        table.update(cx, |t, _| {
            t.set_resize_mode(tl_ui::table::ColumnResizeMode::LastColumn)
        })
    });
    // User 200..300 dragged far left: clamped to the text minimum 60; PSS takes the difference.
    mouse_down(vcx, header_at(300.), 1);
    drag_move(vcx, header_at(220.));
    drag_move(vcx, header_at(210.));
    mouse_up(vcx, header_at(210.), 1);
    assert_eq!(changed_count(vcx, &table), 1);
    assert_eq!(stored_width(vcx, &table, ColumnId::User), px(60.));
    assert_eq!(stored_width(vcx, &table, ColumnId::Pss), px(120.));
    assert_eq!(stored_width(vcx, &table, ColumnId::Pid), px(80.));
    assert_eq!(stored_width(vcx, &table, ColumnId::Name), px(200.));

    // A press and release without a move changes nothing and persists nothing.
    mouse_down(vcx, header_at(260.), 1);
    mouse_up(vcx, header_at(260.), 1);
    assert_eq!(changed_count(vcx, &table), 1);
}

/// Width of `text` as the fit rule measures it: per-character advances of the table font.
fn advance_width(vcx: &mut VisualTestContext, text: &str) -> Pixels {
    vcx.update(|window, cx| {
        let size = metrics(cx).text.base;
        let mut cache = TextMetricsCache::default();
        text.chars()
            .fold(px(0.), |w, ch| w + cache.char_advance(ch, size, window, cx))
    })
}

#[test]
fn double_click_boundary_fits_one_column() {
    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    const LONG: &str = "systemd-journal-remote-upload";
    d.texts.push((ColumnId::User, LONG));
    let (table, vcx) = open(&mut cx, d);
    let text = advance_width(vcx, LONG);
    assert!(
        text > px(150.),
        "the text is wider than the column: {text:?}"
    );

    // User's right boundary is at x 300.
    mouse_down(vcx, header_at(300.), 1);
    mouse_up(vcx, header_at(300.), 1);
    mouse_down(vcx, header_at(300.), 2);
    mouse_up(vcx, header_at(300.), 2);
    assert_eq!(changed_count(vcx, &table), 1, "one persist for the fit");
    // The label "User" + 28 and the minimum 60 are both below the text + 16.
    assert_eq!(stored_width(vcx, &table, ColumnId::User), text + px(16.));
    for (col, w) in [
        (ColumnId::Name, 200.),
        (ColumnId::Pid, 80.),
        (ColumnId::Cpu, 80.),
    ] {
        assert_eq!(stored_width(vcx, &table, col), px(w), "{col:?} unchanged");
    }
}

#[test]
fn fit_columns_never_below_minimum() {
    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    d.specs[3].template = Some("100.0%");
    d.texts.push((ColumnId::User, "root"));
    let (table, vcx) = open(&mut cx, d);
    let handle = vcx.update(|_, cx| table.read(cx).focus_handle().clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    redraw(vcx);

    // An action for another table is not this table's.
    vcx.dispatch_action(ResizeColumnsToFit {
        table: TableId::Services,
    });
    redraw(vcx);
    assert_eq!(changed_count(vcx, &table), 0);

    vcx.dispatch_action(ResizeColumnsToFit {
        table: TableId::Processes,
    });
    redraw(vcx);
    assert_eq!(changed_count(vcx, &table), 1);

    let mins: Vec<(ColumnId, Pixels)> = vcx.update(|window, cx| {
        let m = *metrics(cx);
        let mut cache = TextMetricsCache::default();
        let d = &table.read(cx).delegate;
        ORDER
            .iter()
            .map(|&col| {
                let spec = d.column_spec(col);
                let sorted = col == d.sort.column;
                (col, min_width(spec, sorted, &mut cache, &m, window, cx))
            })
            .collect()
    });
    for (col, min) in mins {
        let w = stored_width(vcx, &table, col);
        assert!(w >= min, "{col:?}: {w:?} below its minimum {min:?}");
    }
    // Short texts: Name shrinks from 200 to fit "row" and its label.
    assert!(stored_width(vcx, &table, ColumnId::Name) < px(200.));
}

// Column reorder drag (E18-011).

fn visible_order(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<HDelegate>>,
) -> Vec<ColumnId> {
    vcx.update(|_, cx| table.read(cx).columns().visible_columns().to_vec())
}

fn reorder_slot(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<HDelegate>>,
) -> Option<usize> {
    vcx.update(|_, cx| table.read(cx).header_state().reorder.map(|d| d.slot))
}

fn sorted_cols(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<HDelegate>>,
) -> Vec<ColumnId> {
    vcx.update(|_, cx| table.read(cx).delegate.sorted.clone())
}

#[test]
fn reorder_moves_column_and_persists_once() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    #[cfg(feature = "test-hooks")]
    let before = vcx.update(|_, cx| table.read(cx).text_cache_stats().1);

    // Grab CPU (380..460) 40 px from its left edge and drag it onto the Name and User boundary.
    mouse_down(vcx, header_at(420.), 1);
    drag_move(vcx, header_at(300.));
    drag_move(vcx, header_at(210.));
    assert_eq!(reorder_slot(vcx, &table), Some(1));
    assert_eq!(
        changed_count(vcx, &table),
        0,
        "nothing persists during the drag"
    );

    // The ghost follows the pointer where it was grabbed and keeps the cell's width.
    let ghost = vcx
        .debug_bounds("TlTreeTableHeader-ghost")
        .expect("ghost while dragging");
    assert_eq!(ghost.origin.x, px(170.));
    assert_eq!(ghost.size.width, px(80.));
    assert_eq!(ghost.size.height, px(44.));
    let theme = vcx.update(|_, cx| current(cx).clone());
    assert_eq!(ghost_fill(&theme), theme.header.opacity(GHOST_OPACITY));
    assert_eq!(GHOST_OPACITY, 0.85);

    // A 2 px line on x 200 over the header and the body.
    let head_line = vcx
        .debug_bounds("TlTreeTableHeader-insert")
        .expect("insertion line in the header");
    let body_line = vcx
        .debug_bounds("TlTreeTable-insert")
        .expect("insertion line over the body");
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    for line in [head_line, body_line] {
        assert_eq!(line.origin.x, px(199.));
        assert_eq!(line.size.width, px(2.));
    }
    assert_eq!(head_line.origin.y, px(0.));
    assert_eq!(head_line.size.height, px(44.));
    assert_eq!(body_line.origin.y, body.origin.y);
    assert_eq!(body_line.size.height, body.size.height);
    #[cfg(feature = "test-hooks")]
    {
        let misses = vcx.update(|_, cx| table.read(cx).text_cache_stats().1);
        assert_eq!(misses, before, "a drag re-formats no cell text");
    }

    mouse_up(vcx, header_at(210.), 1);
    assert_eq!(
        visible_order(vcx, &table),
        [
            ColumnId::Name,
            ColumnId::Cpu,
            ColumnId::User,
            ColumnId::Pid,
            ColumnId::Pss
        ]
    );
    assert_eq!(changed_count(vcx, &table), 1);
    assert!(sorted_cols(vcx, &table).is_empty(), "a drop does not sort");
    assert!(vcx.debug_bounds("TlTreeTableHeader-ghost").is_none());
    assert!(vcx.debug_bounds("TlTreeTable-insert").is_none());
}

#[test]
fn reorder_cannot_move_name() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    mouse_down(vcx, header_at(100.), 1);
    drag_move(vcx, header_at(150.));
    assert_eq!(reorder_slot(vcx, &table), None, "Name never drags");
    assert!(vcx.debug_bounds("TlTreeTableHeader-ghost").is_none());
    assert!(vcx.debug_bounds("TlTreeTableHeader-insert").is_none());
    mouse_up(vcx, header_at(150.), 1);
    assert_eq!(visible_order(vcx, &table), ORDER);
    assert_eq!(changed_count(vcx, &table), 0);
    assert!(
        sorted_cols(vcx, &table).is_empty(),
        "a 50 px move is not a click"
    );
}

#[test]
fn reorder_never_before_name() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    // User to the left edge: the line stays on the Name and User boundary.
    mouse_down(vcx, header_at(250.), 1);
    for x in [150., 60., 2., 0.] {
        drag_move(vcx, header_at(x));
        assert_eq!(reorder_slot(vcx, &table), Some(1), "x {x}");
        let line = vcx
            .debug_bounds("TlTreeTableHeader-insert")
            .expect("insertion line");
        assert_eq!(line.origin.x, px(199.), "never before Name, x {x}");
    }
    mouse_up(vcx, header_at(0.), 1);
    assert_eq!(
        visible_order(vcx, &table),
        ORDER,
        "User stays right after Name"
    );
    assert_eq!(
        changed_count(vcx, &table),
        0,
        "no change, nothing to persist"
    );

    // PID to the left edge lands right after Name.
    mouse_down(vcx, header_at(340.), 1);
    drag_move(vcx, header_at(0.));
    mouse_up(vcx, header_at(0.), 1);
    assert_eq!(
        visible_order(vcx, &table),
        [
            ColumnId::Name,
            ColumnId::Pid,
            ColumnId::User,
            ColumnId::Cpu,
            ColumnId::Pss
        ]
    );
    assert_eq!(changed_count(vcx, &table), 1);
}

#[test]
fn reorder_threshold_4px() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    // 3 px: a click, so PID sorts.
    mouse_down(vcx, header_at(340.), 1);
    drag_move(vcx, header_at(343.));
    assert_eq!(reorder_slot(vcx, &table), None);
    assert!(vcx.debug_bounds("TlTreeTableHeader-ghost").is_none());
    mouse_up(vcx, header_at(343.), 1);
    assert_eq!(sorted_cols(vcx, &table), [ColumnId::Pid]);
    assert_eq!(changed_count(vcx, &table), 0);

    // 5 px: a drag, no sort.
    mouse_down(vcx, header_at(340.), 1);
    drag_move(vcx, header_at(345.));
    assert_eq!(
        reorder_slot(vcx, &table),
        Some(3),
        "nearest boundary is PID's right edge"
    );
    assert!(vcx.debug_bounds("TlTreeTableHeader-ghost").is_some());
    mouse_up(vcx, header_at(345.), 1);
    assert_eq!(
        sorted_cols(vcx, &table),
        [ColumnId::Pid],
        "a drag does not sort"
    );
    assert_eq!(visible_order(vcx, &table), ORDER);
    assert_eq!(changed_count(vcx, &table), 0);
}

#[test]
fn escape_cancels_reorder() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open(&mut cx, HDelegate::new());
    mouse_down(vcx, header_at(420.), 1);
    drag_move(vcx, header_at(210.));
    assert_eq!(reorder_slot(vcx, &table), Some(1));

    vcx.simulate_keystrokes("escape");
    redraw(vcx);
    assert_eq!(reorder_slot(vcx, &table), None);
    assert!(vcx.debug_bounds("TlTreeTableHeader-ghost").is_none());
    assert!(vcx.debug_bounds("TlTreeTable-insert").is_none());

    // The release after Esc neither moves nor sorts.
    drag_move(vcx, header_at(215.));
    mouse_up(vcx, header_at(215.), 1);
    assert_eq!(visible_order(vcx, &table), ORDER);
    assert_eq!(changed_count(vcx, &table), 0);
    assert!(sorted_cols(vcx, &table).is_empty());
}

// ---------------------------------------------------------------------------------------------
// Header menu (E18-012, UI-PRC-006)
// ---------------------------------------------------------------------------------------------

/// The table under a kit Root with a toast queue, the context menu layer and a `ResetColumns`
/// handler that records what reaches the page.
struct MenuHost {
    table: Entity<TlTreeTable<HDelegate>>,
    resets: Rc<std::cell::RefCell<Vec<ResetColumns>>>,
}

impl Render for MenuHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let resets = self.resets.clone();
        div()
            .w(px(600.))
            .h(px(300.))
            .flex()
            .flex_col()
            .on_action(move |a: &ResetColumns, _, _| resets.borrow_mut().push(a.clone()))
            .child(self.table.clone())
            .children(context_menu_layer(window, cx))
    }
}

struct MenuRig {
    table: Entity<TlTreeTable<HDelegate>>,
    host: Entity<MenuHost>,
    toasts: Entity<ToastQueue>,
    vcx: VisualTestContext,
}

fn open_menu_rig(cx: &mut TestAppContext, delegate: HDelegate) -> MenuRig {
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let window = cx.add_window(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let host = cx.new(|_| MenuHost {
            table: table.clone(),
            resets: Rc::default(),
        });
        *out.borrow_mut() = Some((table, host.clone()));
        Root::new(host, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, _| window.activate_window());
    let toasts = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
    redraw(&mut vcx);
    let (table, host) = slot.borrow_mut().take().expect("table created");
    vcx.update(|window, cx| table.read(cx).focus_handle().clone().focus(window, cx));
    redraw(&mut vcx);
    MenuRig {
        table,
        host,
        toasts,
        vcx,
    }
}

fn right_click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Right,
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Right,
        click_count: 1,
    });
    redraw(vcx);
}

/// The open top menu's bounds, if a menu is open.
fn open_menu(vcx: &mut VisualTestContext) -> Option<gpui_kit::Bounds<Pixels>> {
    vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::Menu))
            .map(|s| s.bounds())
    })
}

/// The labels of the open top menu's items, in order. Submenus are closed.
fn menu_labels(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .filter(|s| s.role() == Some(Role::MenuItem))
            .filter_map(|s| s.label().map(str::to_owned))
            .collect()
    })
}

fn click_menu_item(vcx: &mut VisualTestContext, label: &str) {
    let at = vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::MenuItem) && s.label() == Some(label))
            .unwrap_or_else(|| panic!("menu item {label:?}"))
            .bounds()
            .center()
    });
    click(vcx, at);
}

/// The header menu rows for a right-click on `col` of the rig's table.
fn rows_for(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<HDelegate>>,
    col: ColumnId,
) -> Vec<HeaderMenuRow> {
    vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        header_menu_rows(d.column_spec(col), &d.layout, d.sort.column, d.fixed, cx).to_vec()
    })
}

fn toast_texts(vcx: &mut VisualTestContext, toasts: &Entity<ToastQueue>) -> Vec<SharedString> {
    vcx.update(|_, cx| toasts.read(cx).visible_texts())
}

#[test]
fn header_menu_hide_disabled_on_name() {
    let mut cx = cosmic_cx();
    let mut rig = open_menu_rig(&mut cx, HDelegate::new());
    right_click(&mut rig.vcx, header_at(100.));
    let menu = open_menu(&mut rig.vcx).expect("the menu opens");
    assert_eq!(
        menu.origin,
        header_at(100.),
        "the menu opens at the pointer"
    );
    assert!(
        menu.size.width >= px(240.),
        "menu width {:?}",
        menu.size.width
    );

    let hide = rows_for(&mut rig.vcx, &rig.table, ColumnId::Name)
        .into_iter()
        .find(|r| r.item == HeaderMenuItem::Hide)
        .expect("hide row");
    assert!(!hide.enabled);
    assert_eq!(hide.tooltip.as_deref(), Some("Name cannot be hidden"));

    // Only Name visible: still disabled.
    rig.vcx.update(|_, cx| {
        rig.table.update(cx, |t, _| {
            for col in &ORDER[1..] {
                t.delegate.layout.set_visible(*col, false).expect("hide");
            }
        })
    });
    let hide = rows_for(&mut rig.vcx, &rig.table, ColumnId::Name)
        .into_iter()
        .find(|r| r.item == HeaderMenuItem::Hide)
        .expect("hide row");
    assert!(!hide.enabled);
}

#[test]
fn header_menu_items_and_order() {
    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    d.sort = SortSpec {
        column: ColumnId::Cpu,
        ascending: false,
    };
    let mut rig = open_menu_rig(&mut cx, d);
    right_click(&mut rig.vcx, header_at(420.));
    assert_eq!(
        menu_labels(&mut rig.vcx),
        [
            "Sort by CPU",
            "Select Columns…",
            "Hide This Column",
            "Resize columns to fit content",
            "Resize Behavior",
            "Reset to default",
        ]
    );
    let rows = rows_for(&mut rig.vcx, &rig.table, ColumnId::Cpu);
    let items: Vec<_> = rows.iter().map(|r| r.item).collect();
    assert_eq!(
        items,
        [
            HeaderMenuItem::SortBy,
            HeaderMenuItem::SelectColumns,
            HeaderMenuItem::Hide,
            HeaderMenuItem::Separator,
            HeaderMenuItem::FitColumns,
            HeaderMenuItem::ResizeBehavior,
            HeaderMenuItem::Reset,
        ]
    );
    assert!(rows[0].enabled && rows[0].checked, "Sort by CPU is checked");
    assert!(rows[2].enabled);
    assert_eq!(rows[2].tooltip.as_deref(), Some("Hide CPU"));

    let user = rows_for(&mut rig.vcx, &rig.table, ColumnId::User);
    assert!(
        user[0].enabled && !user[0].checked,
        "Sort by User is not checked"
    );

    // PSS is unsortable: item 1 is disabled.
    let pss = rows_for(&mut rig.vcx, &rig.table, ColumnId::Pss);
    assert_eq!(pss[0].label.as_ref(), "Sort by PSS");
    assert!(!pss[0].enabled);
}

#[test]
fn header_menu_fixed_columns_hides_items_2_3_6() {
    let mut cx = cosmic_cx();
    let mut d = HDelegate::new();
    d.fixed = true;
    let mut rig = open_menu_rig(&mut cx, d);
    right_click(&mut rig.vcx, header_at(250.));
    assert_eq!(
        menu_labels(&mut rig.vcx),
        [
            "Sort by User",
            "Resize columns to fit content",
            "Resize Behavior",
        ]
    );
}

#[test]
fn hide_column_persists() {
    let mut cx = cosmic_cx();
    let mut rig = open_menu_rig(&mut cx, HDelegate::new());
    right_click(&mut rig.vcx, header_at(250.));
    click_menu_item(&mut rig.vcx, "Hide This Column");
    assert!(
        open_menu(&mut rig.vcx).is_none(),
        "running the item closes the menu"
    );
    assert_eq!(
        visible_order(&mut rig.vcx, &rig.table),
        [ColumnId::Name, ColumnId::Pid, ColumnId::Cpu, ColumnId::Pss]
    );
    assert_eq!(changed_count(&mut rig.vcx, &rig.table), 1);
    assert!(
        toast_texts(&mut rig.vcx, &rig.toasts).is_empty(),
        "no toast"
    );
}

#[test]
fn resize_behavior_radio_reflects_mode() {
    let mut cx = cosmic_cx();
    let mut rig = open_menu_rig(&mut cx, HDelegate::new());
    let radios = rig
        .vcx
        .update(|_, cx| resize_behavior_rows(ColumnResizeMode::LastColumn, cx));
    assert_eq!(
        radios.map(|(label, mode, on)| (label.to_string(), mode, on)),
        [
            (
                "Proportional".to_owned(),
                ColumnResizeMode::Proportional,
                false
            ),
            ("Last Column".to_owned(), ColumnResizeMode::LastColumn, true),
        ]
    );

    // The submenu of the open menu shows the same radios.
    rig.vcx.update(|_, cx| {
        rig.table
            .update(cx, |t, _| t.set_resize_mode(ColumnResizeMode::LastColumn))
    });
    right_click(&mut rig.vcx, header_at(420.));
    click_menu_item(&mut rig.vcx, "Resize Behavior");
    let labels = menu_labels(&mut rig.vcx);
    assert!(
        labels.iter().any(|l| l == "Proportional") && labels.iter().any(|l| l == "Last Column"),
        "{labels:?}"
    );
}

#[test]
fn reset_columns_restores_defaults_and_toasts() {
    let mut cx = cosmic_cx();
    let mut rig = open_menu_rig(&mut cx, HDelegate::new());
    let defaults = rig
        .vcx
        .update(|_, cx| rig.table.read(cx).delegate.layout.clone());
    rig.vcx.update(|_, cx| {
        rig.table.update(cx, |t, _| {
            t.set_default_layout(defaults.clone());
            let layout = &mut t.delegate.layout;
            layout.set_visible(ColumnId::User, false).expect("hide");
            layout.set_width(ColumnId::Cpu, px(150.));
            layout
                .move_column(ColumnId::Pss, Some(ColumnId::Pid))
                .expect("move");
        })
    });
    redraw(&mut rig.vcx);
    right_click(&mut rig.vcx, header_at(250.));
    click_menu_item(&mut rig.vcx, "Reset to default");
    let layout = rig
        .vcx
        .update(|_, cx| rig.table.read(cx).delegate.layout.clone());
    assert_eq!(layout, defaults);
    assert_eq!(changed_count(&mut rig.vcx, &rig.table), 1);
    assert_eq!(
        toast_texts(&mut rig.vcx, &rig.toasts),
        [SharedString::new_static("Columns reset to default")]
    );
    assert!(
        rig.vcx
            .update(|_, cx| rig.host.read(cx).resets.borrow().is_empty())
    );
}

#[test]
fn reset_columns_without_defaults_propagates() {
    let mut cx = cosmic_cx();
    let mut rig = open_menu_rig(&mut cx, HDelegate::new());
    right_click(&mut rig.vcx, header_at(250.));
    click_menu_item(&mut rig.vcx, "Reset to default");
    let resets = rig
        .vcx
        .update(|_, cx| rig.host.read(cx).resets.borrow().clone());
    assert_eq!(
        resets,
        [ResetColumns {
            table: TableId::Processes
        }]
    );
    assert_eq!(changed_count(&mut rig.vcx, &rig.table), 0);
    assert!(toast_texts(&mut rig.vcx, &rig.toasts).is_empty());
}
