//! Tree-table render goldens (E18, tasklens-rsf.15, ADR-013, UI-PRC-002, UI-FND-052,
//! UI-FND-053): 30 rows with every `Validity` state at `ui_scale` 1.0 and 1.1, a 60 px Name
//! column that ends in "…", and a Hebrew name with a count run in an LTR cell. Rendered on
//! lavapipe and compared with `tests/golden/table_*.png`. `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! The 60 px and RTL cases also have a `TestAppContext` half with the real text system: the
//! Name tooltip holds the full name, and the count run stays right of the RTL run.
//!
//! Run with `cargo test -p tl-ui --features render-tests --test render_table`.

#![cfg(feature = "render-tests")]

use std::fmt::Write as _;
use std::ops::Range;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, Modifiers, MouseMoveEvent,
    ParentElement, Pixels, Render, SharedString, Styled, TestAppContext, TestDispatcher, TextRun,
    VisualTestContext, Window, div, font, point, px, size,
};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::SortSpec;
use tl_model::validity::Validity;
use tl_ui::components::menu::TlMenu;
use tl_ui::components::tooltip::{DELAY_ROW_NAME, TlTooltip};
use tl_ui::table::cells::{CellOut, IconSourceCache, name_primary_text};
use tl_ui::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use tl_ui::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};
use tl_ui::testing::render::{RenderHarness, assert_golden, run_on_lavapipe};
use tl_ui::text::fonts::{UI_FAMILY, load_bundled_fonts};
use tl_ui::text::style::t_sm;
use tl_ui::theme::{current, metrics};

/// The seven states, in the order the rows cycle them.
const STATES: [Validity; 7] = [
    Validity::Valid,
    Validity::WarmingUp,
    Validity::Stale,
    Validity::Restricted,
    Validity::Unsupported,
    Validity::Missing,
    Validity::Failed,
];

/// Every cell shows this value; glyph states draw their glyph instead.
const VALUE: &str = "11.9%";

/// The validity table: 30 rows in a 1000 × 900 window.
const ROWS: u32 = 30;
const VALIDITY_W: f32 = 1000.;
const VALIDITY_H: f32 = 900.;
const SELECTED_ROW: usize = 3;
const NEW_ROW: usize = 5;
const EXITED_ROW: usize = 7;
const STALE_ROW: usize = 9;

/// The 60 px Name column case.
const LONG_NAME: &str = "a-very-long-process-name-for-ellipsis";
const NARROW_NAME_W: f32 = 60.;

/// The RTL case: a Hebrew name and its count run in a 200 px Name column.
const RTL_NAME: &str = "תהליך-בדיקה (3)";
const RTL_NAME_W: f32 = 200.;
const CPU_W: f32 = 80.;

/// One-line header height at 100%.
const HEADER_H_ONE: f32 = 28.;
const ROW_H: f32 = 28.;

struct Row {
    id: u32,
    kind: RowKind,
    depth: u8,
    expandable: Option<bool>,
    flags: RowFlags,
    name: SharedString,
    count_run: Option<Range<usize>>,
    /// The validity of each numeric column, by position after Name.
    states: Vec<Validity>,
}

impl Row {
    fn plain(id: u32, name: &'static str, states: Vec<Validity>) -> Self {
        Self {
            id,
            kind: RowKind::Process,
            depth: 0,
            expandable: None,
            flags: RowFlags::default(),
            name: SharedString::new_static(name),
            count_run: None,
            states,
        }
    }
}

/// A delegate over fixed rows with a Name column and numeric columns.
struct Delegate {
    rows: Vec<Row>,
    specs: Vec<ColumnSpec>,
    layout: ColumnLayout,
    lines: HeaderLines,
    /// Row tooltips by row index, built when the delegate is made (`row_tooltip` has no `cx`).
    tips: Vec<Option<AnyView>>,
}

fn spec(id: ColumnId, label: &'static str, kind: ColumnKind, width: f32) -> ColumnSpec {
    let label = SharedString::new_static(label);
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

impl Delegate {
    /// `specs` starts with Name. The layout is scaled to `ui_scale`, as a page does.
    fn new(rows: Vec<Row>, specs: Vec<ColumnSpec>, lines: HeaderLines, ui_scale: f32) -> Self {
        let order: Vec<ColumnId> = specs.iter().map(|s| s.id).collect();
        let mut layout = ColumnLayout::defaults(TableId::Processes, &specs, &order, &order);
        layout.rebuild(ui_scale);
        let tips = rows.iter().map(|_| None).collect();
        Self {
            rows,
            specs,
            layout,
            lines,
            tips,
        }
    }

    /// Position of `col` among the numeric columns.
    fn numeric_pos(&self, col: ColumnId) -> usize {
        self.specs
            .iter()
            .position(|s| s.id == col)
            .expect("known column")
            - 1
    }
}

impl TreeTableDelegate for Delegate {
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
    fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut) {
        let validity = self.rows[ix]
            .states
            .get(self.numeric_pos(col))
            .copied()
            .unwrap_or(Validity::Valid);
        out.bits = validity as u64;
        out.validity = validity;
        if out.want_text {
            out.text.push_str(VALUE);
        }
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let row = &self.rows[ix];
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: row.name.clone(),
            count_run: row.count_run.clone(),
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool {
        if col == ColumnId::Name {
            return false;
        }
        out.validity = Validity::Valid;
        if out.want_text {
            let _ = write!(out.text, "{}%", 40 + self.numeric_pos(col));
        }
        true
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
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        self.tips[ix].clone()
    }
    fn empty_text(&self) -> Option<SharedString> {
        None
    }
    fn header_lines(&self) -> HeaderLines {
        self.lines
    }
}

/// The theme's window background behind the table.
struct Root {
    table: Entity<TlTreeTable<Delegate>>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(current(cx).window_bg)
            .child(self.table.clone())
    }
}

/// Builds the table in `window` with row `select` selected, if any.
fn table_root(
    delegate: Delegate,
    select: Option<u32>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Root> {
    tl_ui::i18n::init_strings(cx, "en-US");
    let table = cx.new(|cx| {
        let mut t = TlTreeTable::new(delegate, window, cx);
        if let Some(id) = select {
            t.selection.click(id);
        }
        t
    });
    cx.new(|_| Root { table })
}

/// The seven numeric columns of the validity table.
fn validity_specs() -> Vec<ColumnSpec> {
    vec![
        spec(ColumnId::Name, "Name", ColumnKind::Text, 248.),
        spec(ColumnId::Cpu, "CPU", ColumnKind::Numeric, 80.),
        spec(ColumnId::CpuKernel, "Kernel", ColumnKind::Numeric, 80.),
        spec(ColumnId::Memory, "Memory", ColumnKind::Numeric, 96.),
        spec(ColumnId::Swap, "Swap", ColumnKind::Numeric, 80.),
        spec(ColumnId::Pss, "PSS", ColumnKind::Numeric, 80.),
        spec(ColumnId::Uss, "USS", ColumnKind::Numeric, 80.),
        spec(ColumnId::Disk, "Disk", ColumnKind::Numeric, 96.),
    ]
}

/// Row names of the validity table, cycled.
const NAMES: [&str; 6] = [
    "firefox",
    "gnome-shell",
    "pipewire",
    "systemd",
    "kworker/3:1",
    "Xwayland",
];

/// A group header, then 29 rows. Row r's numeric column c shows `STATES[(r + c) % 7]`, so every
/// row and every column holds all seven states. Every fifth row from 1 is an expanded parent at
/// depth 0; the rows between are its children at depth 1.
fn validity_rows() -> Vec<Row> {
    let mut rows = vec![Row {
        kind: RowKind::GroupHeader,
        expandable: Some(true),
        name: SharedString::new_static("Apps (29)"),
        ..Row::plain(0, "", Vec::new())
    }];
    for ix in 1..ROWS as usize {
        let states = (0..STATES.len())
            .map(|c| STATES[(ix + c) % STATES.len()])
            .collect();
        let parent = (ix - 1).is_multiple_of(5);
        let mut row = Row::plain(ix as u32, NAMES[ix % NAMES.len()], states);
        row.depth = if parent { 0 } else { 1 };
        row.expandable = parent.then_some(true);
        row.flags.bits = match ix {
            NEW_ROW => RowFlags::NEW,
            EXITED_ROW => RowFlags::EXITED,
            STALE_ROW => RowFlags::STALE,
            _ => 0,
        };
        rows.push(row);
    }
    rows
}

fn check_validity_rows(scale: f32, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    harness.set_scale(scale);
    let img = harness.render(size(px(VALIDITY_W), px(VALIDITY_H)), |window, cx| {
        let s = metrics(cx).ui_scale;
        let delegate = Delegate::new(validity_rows(), validity_specs(), HeaderLines::Two, s);
        table_root(delegate, Some(SELECTED_ROW as u32), window, cx)
    });
    assert_golden(name, &img);
}

#[test]
fn validity_rows_scale_100() {
    run_on_lavapipe("validity_rows_scale_100", || {
        check_validity_rows(1.0, "table_validity_rows_100")
    });
}

#[test]
fn validity_rows_scale_110() {
    run_on_lavapipe("validity_rows_scale_110", || {
        check_validity_rows(1.1, "table_validity_rows_110")
    });
}

/// Name 60 px and CPU, one row with `LONG_NAME` and one with "sh".
fn narrow_delegate(ui_scale: f32) -> Delegate {
    let rows = vec![
        Row::plain(1, LONG_NAME, vec![Validity::Valid]),
        Row::plain(2, "sh", vec![Validity::Valid]),
    ];
    let specs = vec![
        spec(ColumnId::Name, "Name", ColumnKind::Text, NARROW_NAME_W),
        spec(ColumnId::Cpu, "CPU", ColumnKind::Numeric, CPU_W),
    ];
    Delegate::new(rows, specs, HeaderLines::One, ui_scale)
}

/// Name 200 px and CPU, one row with `RTL_NAME` and its count run, and one Latin row.
fn rtl_delegate(ui_scale: f32) -> Delegate {
    let count = RTL_NAME.find('(').expect("count run")..RTL_NAME.len();
    let mut hebrew = Row::plain(1, RTL_NAME, vec![Validity::Valid]);
    hebrew.count_run = Some(count);
    let mut latin = Row::plain(2, "bash (2)", vec![Validity::Valid]);
    latin.count_run = Some(5..8);
    let specs = vec![
        spec(ColumnId::Name, "Name", ColumnKind::Text, RTL_NAME_W),
        spec(ColumnId::Cpu, "CPU", ColumnKind::Numeric, CPU_W),
    ];
    Delegate::new(vec![hebrew, latin], specs, HeaderLines::One, ui_scale)
}

/// Window for the small cases: the columns plus 40 px, the header and two rows plus 16 px.
fn small_window(name_w: f32) -> gpui_kit::Size<Pixels> {
    size(
        px(name_w + CPU_W + 40.),
        px(HEADER_H_ONE + 2. * ROW_H + 16.),
    )
}

fn check_small(name: &str, name_w: f32, delegate: fn(f32) -> Delegate) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    let img = harness.render(small_window(name_w), |window, cx| {
        let s = metrics(cx).ui_scale;
        table_root(delegate(s), None, window, cx)
    });
    assert_golden(name, &img);
}

// The TestAppContext halves use the real text system with the bundled fonts.

fn cosmic_cx() -> TestAppContext {
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    cx.update(|cx| {
        let report = load_bundled_fonts(cx);
        assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    });
    cx
}

/// Opens the table in a window of `size` and draws it.
fn open_table(
    cx: &mut TestAppContext,
    window_size: gpui_kit::Size<Pixels>,
    build: impl FnOnce(&mut App) -> Delegate + 'static,
) -> &mut VisualTestContext {
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let delegate = build(cx);
        let root = table_root(delegate, None, window, cx);
        Frame {
            size: window_size,
            root,
        }
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    vcx
}

/// A box of a fixed size around `root`.
struct Frame {
    size: gpui_kit::Size<Pixels>,
    root: Entity<Root>,
}

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.size.width)
            .h(self.size.height)
            .flex()
            .flex_col()
            .child(self.root.clone())
    }
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn hover_at(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_event(MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
    redraw(cx);
}

/// Shaped width of `text` in the tooltip style.
fn tooltip_text_width(cx: &mut VisualTestContext, text: &'static str) -> Pixels {
    cx.update(|window, cx| {
        let size = t_sm(metrics(cx)).font_size.expect("t_sm sets a size");
        let size = size.to_pixels(window.rem_size());
        let run = TextRun {
            len: text.len(),
            font: font(UI_FAMILY),
            color: current(cx).text_primary,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text.into(), size, &[run], None)
            .width
    })
}

#[test]
fn name_column_60px_ellipsis_and_tooltip() {
    run_on_lavapipe("name_column_60px_ellipsis_and_tooltip", || {
        check_small("table_name_60px", NARROW_NAME_W, narrow_delegate)
    });

    // TestAppContext half: the cut name is shorter than the full name, and the row tooltip
    // holds the full name.
    let mut cx = cosmic_cx();
    let vcx = open_table(&mut cx, small_window(NARROW_NAME_W), |cx| {
        let mut d = narrow_delegate(1.0);
        d.tips[0] = Some(TlTooltip::text(LONG_NAME).view(cx));
        d
    });
    let cell = vcx.debug_bounds("TlNameCell").expect("Name cell drawn");
    assert_eq!(cell.size.width, px(NARROW_NAME_W));
    let primary = vcx
        .debug_bounds("TlNameCell-primary")
        .expect("primary text drawn");
    assert!(
        primary.right() <= cell.right(),
        "the name text stays inside the cell: {primary:?} in {cell:?}"
    );
    // The row tooltip is attached once the hovered row has drawn twice; the next move starts
    // its 600 ms delay.
    let (x, y) = (30., HEADER_H_ONE + ROW_H / 2.);
    for _ in 0..3 {
        hover_at(vcx, x, y);
    }
    assert!(vcx.debug_bounds("TlTooltip").is_none(), "not at once");
    vcx.executor().advance_clock(DELAY_ROW_NAME * 2);
    redraw(vcx);
    let text = vcx
        .debug_bounds("TlTooltip-text")
        .expect("the Name tooltip shows");
    let full = tooltip_text_width(vcx, LONG_NAME);
    assert!(
        (text.size.width - full).abs() < px(1.),
        "the tooltip holds the full name: text {:?}, full name {full:?}",
        text.size.width
    );
}

/// True when fontconfig knows a font with Hebrew glyphs.
fn hebrew_font() -> bool {
    std::process::Command::new("fc-list")
        .args([":lang=he", "family"])
        .output()
        .is_ok_and(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
}

/// Left and right x of the glyphs whose byte index is in `range`. A glyph's right edge is the
/// next glyph's x in visual order, or the line end.
fn glyph_span(xs: &[(Pixels, usize)], width: Pixels, range: &Range<usize>) -> (Pixels, Pixels) {
    let mut left = width;
    let mut right = Pixels::ZERO;
    for (k, &(x, index)) in xs.iter().enumerate() {
        if range.contains(&index) {
            let end = xs.get(k + 1).map_or(width, |next| next.0);
            left = left.min(x);
            right = right.max(end);
        }
    }
    (left, right)
}

#[test]
fn rtl_name_in_ltr_cell() {
    if !hebrew_font() {
        eprintln!("skipped rtl_name_in_ltr_cell: no font with Hebrew glyphs (fc-list :lang=he)");
        return;
    }
    let count = RTL_NAME.find('(').expect("count run")..RTL_NAME.len();
    let hebrew = 0..RTL_NAME.find(' ').expect("space before the count");
    run_on_lavapipe("rtl_name_in_ltr_cell", || {
        check_small("table_rtl_name", RTL_NAME_W, rtl_delegate)
    });

    // TestAppContext half: the line as the Name cell shapes it, the name wrapped in FSI and PDI
    // and the count run in tnum. Byte ranges move by the marks: 3 bytes each.
    let mark = '\u{2068}'.len_utf8();
    let count = count.start + 2 * mark..count.end + 2 * mark;
    let hebrew = hebrew.start + mark..hebrew.end + mark;
    let mut cx = cosmic_cx();
    let w = RTL_NAME_W + CPU_W + 40.;
    let vcx = open_table(&mut cx, size(px(w), px(120.)), |_| rtl_delegate(1.0));
    let (xs, width) = vcx.update(|window, cx| {
        let size = metrics(cx).text.base;
        let theme = current(cx).clone();
        let primary = SharedString::new_static(RTL_NAME);
        let count_run = RTL_NAME.find('(').expect("count run")..RTL_NAME.len();
        let (text, runs) = name_primary_text(
            &primary,
            true,
            Some(&count_run),
            theme.text_primary,
            theme.status.warning,
            &mut IconSourceCache::new(),
        );
        assert_eq!(
            text.len(),
            RTL_NAME.len() + 2 * mark,
            "the name is isolated"
        );
        let runs = runs.expect("count run in tnum");
        let line = window.text_system().shape_line(text, size, &runs, None);
        let mut xs: Vec<(Pixels, usize)> = line
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter().map(|g| (g.position.x, g.index)))
            .collect();
        xs.sort_by(|a, b| f32::from(a.0).total_cmp(&f32::from(b.0)));
        (xs, line.width)
    });
    let (count_left, count_right) = glyph_span(&xs, width, &count);
    let (hebrew_left, hebrew_right) = glyph_span(&xs, width, &hebrew);
    assert!(
        count_left >= hebrew_right,
        "the count run starts right of the RTL run: count {count_left:?}, RTL ends at \
         {hebrew_right:?}, glyphs {xs:?}"
    );
    // The space stays outside the isolate: it draws between the name and the count, not at
    // the name's left edge, so the text starts at x 0 like the LTR rows.
    assert!(
        count_left - hebrew_right > px(1.),
        "a gap between the name and the count: count {count_left:?}, RTL ends at \
         {hebrew_right:?}, glyphs {xs:?}"
    );
    assert!(
        hebrew_left < px(0.5),
        "the RTL run starts the text: {hebrew_left:?}, glyphs {xs:?}"
    );
    assert!(
        (count_right - width).abs() < px(0.5),
        "the count run ends at the text end: {count_right:?} of {width:?}"
    );
    // The text fits, and the CPU cell keeps its x.
    let primary = vcx
        .debug_bounds("TlNameCell-primary")
        .expect("primary text drawn");
    let cell = vcx.debug_bounds("TlNameCell").expect("Name cell drawn");
    assert!(
        primary.right() <= cell.right() - px(8.),
        "{primary:?} in {cell:?}"
    );
    let cpu = vcx.debug_bounds("TlCell-numeric").expect("CPU cell drawn");
    assert_eq!(cpu.origin.x, cell.origin.x + px(RTL_NAME_W));
}
