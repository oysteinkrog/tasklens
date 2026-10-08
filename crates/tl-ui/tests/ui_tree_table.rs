//! Tree-table tests in a real window (UI-PRC-001, ADR-013). The first section covers the generic
//! cell painters (E18-004), the second the table entity (E18-006); later table beads append their
//! own sections below. `tree_table_renders_visible_range_only` needs `--features test-hooks`.
//!
//! The cells are measured, so the window uses the real text system with the bundled fonts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::ScrollbarHandle;
use gpui_kit::base::test_support::snapshots;
use gpui_kit::{
    Action, AnyView, App, AppContext as _, Context, Div, Entity, InteractiveElement as _,
    IntoElement, KeyContext, Modifiers, MouseButton, MouseMoveEvent, ParentElement, Pixels, Render,
    Role, ScrollDelta, ScrollStrategy, ScrollWheelEvent, SharedString, Styled, Subscription,
    TestAppContext, TestDispatcher, TouchPhase, VisualTestContext, Window, div, point, px,
};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::{ServiceColumn, SortSpec, SysColumnId, SysSortSpec};
use tl_model::validity::Validity;
use tl_ui::components::menu::{TlMenu, context_menu_layer};
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::validity::{GlyphContent, GlyphCtx, glyph_parts, tooltip_text};
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
use tl_ui::table::header::{HeaderMenuItem, header_menu_rows};
use tl_ui::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
    TreeTableEvent, metric_cell_opacity,
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
    /// The last ids `on_visible_range` received, and how often it was called.
    visible: Vec<u32>,
    visible_calls: usize,
    /// When not empty, a row with an icon gets the file icon `icon_paths[ix % len]`.
    icon_paths: Vec<std::sync::Arc<str>>,
    version: u64,
    /// Cell overrides: (row id, column, text, validity). Other cells show "{ix}.0%".
    cells: Vec<(u32, ColumnId, &'static str, Validity)>,
    /// Every `context_menu` call: the targets and the clicked column.
    menu_calls: Vec<(Vec<u32>, Option<ColumnId>)>,
    /// Every `cell` call, and the row index of each call that had `want_tooltip` set.
    cell_calls: Cell<usize>,
    tip_rows: RefCell<Vec<usize>>,
    /// Every Stale tooltip `cell` built.
    tips_built: RefCell<Vec<SharedString>>,
    /// The row whose Name text `name_weight` gives 500.
    medium_row: Option<usize>,
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
            visible: Vec::new(),
            visible_calls: 0,
            icon_paths: Vec::new(),
            version: 1,
            cells: Vec::new(),
            menu_calls: Vec::new(),
            cell_calls: Cell::new(0),
            tip_rows: RefCell::new(Vec::new()),
            tips_built: RefCell::new(Vec::new()),
            medium_row: None,
        }
    }

    /// Columns Name, PID and CPU, labelled as in the process table.
    fn name_pid_cpu(rows: Vec<TRow>) -> Self {
        let mut d = Self::new(rows);
        let labelled = |id, kind, w, label: &'static str| ColumnSpec {
            label: label.into(),
            ..spec(id, kind, w)
        };
        d.specs = vec![
            labelled(ColumnId::Name, ColumnKind::Text, 248., "Name"),
            labelled(ColumnId::Pid, ColumnKind::Numeric, 80., "PID"),
            labelled(ColumnId::Cpu, ColumnKind::Numeric, 80., "CPU"),
        ];
        let order = [ColumnId::Name, ColumnId::Pid, ColumnId::Cpu];
        d.layout = ColumnLayout::defaults(TableId::Processes, &d.specs, &order, &order);
        d
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
        self.version
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
        self.cell_calls.set(self.cell_calls.get() + 1);
        if out.want_tooltip {
            self.tip_rows.borrow_mut().push(ix);
        }
        let id = self.rows[ix].id;
        if let Some(k) = self.cells.iter().position(|c| c.0 == id && c.1 == col) {
            let (_, _, text, validity) = self.cells[k];
            out.bits = k as u64;
            out.validity = validity;
            if out.want_text {
                out.text.push_str(text);
            }
            if out.want_tooltip && validity == Validity::Stale {
                let ctx = GlyphCtx {
                    age_ns: 12_000_000_000,
                    ..GlyphCtx::default()
                };
                out.tooltip = tooltip_text(validity, &ctx);
                self.tips_built.borrow_mut().extend(out.tooltip.clone());
            }
            return;
        }
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
                Some(_) if !self.icon_paths.is_empty() => {
                    NameIcon::File(self.icon_paths[ix % self.icon_paths.len()].clone())
                }
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
        targets: &[u32],
        col: Option<ColumnId>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        self.menu_calls.push((targets.to_vec(), col));
        menu.item(
            "End task",
            Box::new(EndRows {
                ids: targets.to_vec(),
            }),
        )
    }
    fn on_toggle_expand(&mut self, id: u32, _: &mut Context<TlTreeTable<Self>>) {
        self.toggled.push(id);
        if let Some(row) = self.rows.iter_mut().find(|r| r.id == id) {
            row.expandable = row.expandable.map(|e| !e);
        }
    }
    fn on_sort(&mut self, _: ColumnId, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_columns_changed(&mut self, layout: &ColumnLayout, _: &mut Context<TlTreeTable<Self>>) {
        self.layout = layout.clone();
    }
    fn on_visible_range(&mut self, ids: &[u32], _: &mut Context<TlTreeTable<Self>>) {
        self.visible = ids.to_vec();
        self.visible_calls += 1;
    }
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
    fn name_weight(&self, ix: usize) -> gpui_kit::FontWeight {
        if self.medium_row == Some(ix) {
            gpui_kit::FontWeight::MEDIUM
        } else {
            gpui_kit::FontWeight::NORMAL
        }
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
        let handle = table.read(cx).scroll_handle();
        ScrollbarHandle::set_offset(&handle, point(px(0.), px(-14.)));
    });
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
    assert!(built <= 27, "{built} rows built after scrolling");
}

/// rsf.28: `name_weight` sets the Name text's weight. The same name at 500 is wider than at the
/// default, normal weight, which every other table keeps.
#[test]
fn name_weight_draws_the_name_at_500() {
    let primary_width = |medium_row: Option<usize>| {
        let mut cx = cosmic_cx();
        let rows = vec![TRow {
            name: "systemd-journald",
            ..plain(1)
        }];
        let d = TDelegate {
            medium_row,
            ..TDelegate::new(rows)
        };
        let (_, vcx) = open_table(&mut cx, d, 600., 200.);
        vcx.debug_bounds("TlNameCell-primary")
            .expect("the Name text")
            .size
            .width
    };
    let normal = primary_width(None);
    let medium = primary_width(Some(0));
    assert!(normal > px(0.));
    assert!(medium > normal, "500 is wider: {medium:?} vs {normal:?}");
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

/// A depth-0 row and its depth-1 child, guide lines on or off.
#[cfg(feature = "test-hooks")]
fn parent_child(guides: bool) -> TDelegate {
    let mut d = TDelegate::new(vec![
        plain(0),
        TRow {
            depth: 1,
            flags: RowFlags {
                bits: RowFlags::LAST_SIBLING,
                ancestor_continues: 0,
            },
            ..plain(1)
        },
    ]);
    d.guides = guides;
    d
}

#[cfg(feature = "test-hooks")]
fn guides(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> Vec<gpui_kit::Bounds<Pixels>> {
    vcx.update(|_, cx| table.read(cx).guide_quads.borrow().clone())
}

#[cfg(feature = "test-hooks")]
fn line_canvases(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>) -> u32 {
    vcx.update(|_, cx| table.read(cx).line_canvases.get())
}

#[cfg(feature = "test-hooks")]
#[test]
fn connector_lines_only_when_enabled() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, parent_child(false), 600., 100.);
    assert!(vcx.debug_bounds("TlNameCell").is_some(), "rows drawn");
    assert!(guides(vcx, &table).is_empty());
    assert_eq!(
        line_canvases(vcx, &table),
        0,
        "no line canvas when both are off"
    );

    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, parent_child(true), 600., 100.);
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    let scale = vcx.update(|window, _| window.scale_factor());
    // Only the child draws lines: the depth-0 row has no continuing line, so no canvas.
    let child = point(body.left(), body.top() + px(ROW_H));
    let flags = parent_child(true).rows[1].flags;
    assert_eq!(
        guides(vcx, &table),
        guide_quads(1, flags, child, px(ROW_H), 1.0, scale).into_vec()
    );
    assert_eq!(line_canvases(vcx, &table), 1);
}

#[cfg(feature = "test-hooks")]
#[test]
fn connector_lines_skip_group_headers() {
    let mut d = TDelegate::new(vec![group(0), plain(1)]);
    d.guides = true;
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, d, 600., 100.);
    assert!(
        vcx.debug_bounds("TlTreeTable-group-row").is_some(),
        "rows drawn"
    );
    assert!(guides(vcx, &table).is_empty());
    assert_eq!(line_canvases(vcx, &table), 0);
}

#[cfg(feature = "test-hooks")]
#[test]
fn guides_and_separators_share_one_canvas_per_row() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, parent_child(true), 600., 100.);
    separators_on(vcx, &table);
    // Two body rows, each with one canvas: guides on the child, separators on both.
    assert_eq!(line_canvases(vcx, &table), 2);
    assert_eq!(separators(vcx, &table).len(), 2 * 2);
    assert_eq!(guides(vcx, &table).len(), 2, "the child's elbow");
}

#[test]
fn connector_quads_geometry() {
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
        let handle = table.read(cx).scroll_handle();
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

// ---------------------------------------------------------------------------------------------
// E18-008: scroll anchoring, keep-visible, jump and visible-range demand (UI-PRC-010).
// ---------------------------------------------------------------------------------------------

const ROW_H: f32 = 28.;

/// A table of `n` plain rows with ids 0..n, `rows` rows tall.
fn open_rows(
    cx: &mut TestAppContext,
    n: u32,
    rows: f32,
) -> (Entity<TlTreeTable<TDelegate>>, &mut VisualTestContext) {
    open_table(
        cx,
        TDelegate::new((0..n).map(plain).collect()),
        600.,
        rows * ROW_H,
    )
}

/// How far the body is scrolled down, in px.
fn scroll_top(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>) -> f32 {
    vcx.update(|_, cx| {
        let offset = ScrollbarHandle::offset(&table.read(cx).scroll_handle());
        -offset.y / px(1.)
    })
}

/// Scrolls the body to `top` px and draws a frame.
fn scroll_to(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>, top: f32) {
    vcx.update(|window, cx| {
        ScrollbarHandle::set_offset(&table.read(cx).scroll_handle(), point(px(0.), px(-top)));
        window.refresh();
    });
    redraw(vcx);
}

/// Moves row `id` to display index `to` and installs the new version.
fn move_row(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
    id: u32,
    to: usize,
) {
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            let rows = &mut t.delegate.rows;
            let ix = rows.iter().position(|r| r.id == id).expect("row exists");
            let row = rows.remove(ix);
            rows.insert(to, row);
            t.on_new_version(cx);
        })
    });
    redraw(vcx);
}

/// Delivers the next frame: runs the pending next-frame callbacks, then draws.
fn next_frame(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    redraw(vcx);
}

fn demand(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> (Vec<u32>, usize) {
    vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        (d.visible.clone(), d.visible_calls)
    })
}

#[test]
fn scroll_anchor_keeps_top_row_on_resort() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    // Row 100 is the top row, 10 px of it scrolled out.
    scroll_to(vcx, &table, 100. * ROW_H + 10.);
    assert_eq!(
        vcx.update(|_, cx| table.read(cx).visible_range().start),
        100
    );

    move_row(vcx, &table, 100, 140);
    assert_eq!(
        scroll_top(vcx, &table),
        140. * ROW_H + 10.,
        "row 100 keeps its y"
    );
    let top = vcx.update(|_, cx| {
        let t = table.read(cx);
        t.delegate.row_id(t.visible_range().start)
    });
    assert_eq!(top, 100, "row 100 is still the top row");

    // When the anchor row is gone the scroll position stays.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.rows.retain(|r| r.id != 100);
            t.on_new_version(cx);
        })
    });
    redraw(vcx);
    assert_eq!(scroll_top(vcx, &table), 140. * ROW_H + 10.);
}

#[test]
fn scroll_anchor_skipped_during_wheel() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    vcx.simulate_event(ScrollWheelEvent {
        position: point(px(100.), px(HEADER_H + 100.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-100. * ROW_H))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    redraw(vcx);
    assert_eq!(scroll_top(vcx, &table), 100. * ROW_H, "the wheel scrolled");

    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    move_row(vcx, &table, 100, 140);
    assert_eq!(
        scroll_top(vcx, &table),
        100. * ROW_H,
        "no anchoring 100 ms after a wheel"
    );
    assert_eq!(
        vcx.update(|_, cx| table.read(cx).visible_range().start),
        100
    );

    // 250 ms after the wheel, anchoring works again.
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    let top_id = vcx.update(|_, cx| table.read(cx).delegate.row_id(100));
    move_row(vcx, &table, top_id, 60);
    assert_eq!(scroll_top(vcx, &table), 60. * ROW_H);
}

#[test]
fn no_scroll_while_paused() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    scroll_to(vcx, &table, 100. * ROW_H);
    vcx.update(|_, cx| table.update(cx, |t, _| t.paused = true));
    move_row(vcx, &table, 100, 140);
    assert_eq!(
        scroll_top(vcx, &table),
        100. * ROW_H,
        "nothing moves during Ctrl-pause"
    );
    move_row(vcx, &table, 5, 120);
    assert_eq!(scroll_top(vcx, &table), 100. * ROW_H);
}

#[test]
fn focus_move_scrolls_nearest_with_margin() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    focus_table(vcx, &table);
    // Rows 0 to 29 fill the body; 29 is the last fully visible line.
    select(vcx, &table, 29);
    keys(vcx, "down");
    assert_eq!(sel(vcx, &table), (vec![30], Some(30)));
    // Row 31 ends at the bottom edge: one row of margin under the focus row.
    assert_eq!(scroll_top(vcx, &table), 32. * ROW_H - 30. * ROW_H);

    // At the top edge the margin row is above the focus row.
    scroll_to(vcx, &table, 10. * ROW_H);
    select(vcx, &table, 11);
    keys(vcx, "up");
    assert_eq!(sel(vcx, &table), (vec![10], Some(10)));
    assert_eq!(scroll_top(vcx, &table), 9. * ROW_H);

    // Inside the margins a move does not scroll.
    select(vcx, &table, 15);
    keys(vcx, "down");
    assert_eq!(scroll_top(vcx, &table), 9. * ROW_H);

    // A mouse click never scrolls, even on the last fully visible line.
    click(vcx, row_at(29, 100.), Modifiers::none());
    assert_eq!(sel(vcx, &table), (vec![38], Some(38)));
    assert_eq!(scroll_top(vcx, &table), 9. * ROW_H);
}

#[test]
fn jump_target_centres_row() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    let found = vcx.update(|_, cx| table.update(cx, |t, cx| t.jump_to(900, cx)));
    assert!(found);
    redraw(vcx);
    assert_eq!(sel(vcx, &table), (vec![900], Some(900)));
    let centre = 900. * ROW_H + ROW_H / 2. - scroll_top(vcx, &table);
    let viewport_centre = 30. * ROW_H / 2.;
    assert!(
        (centre - viewport_centre).abs() <= ROW_H / 2.,
        "row centre {centre} vs viewport centre {viewport_centre}"
    );
    let missing = vcx.update(|_, cx| table.update(cx, |t, cx| t.jump_to(5000, cx)));
    assert!(!missing, "an id outside the view is not found");
}

#[test]
fn tree_table_reports_visible_demand() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    scroll_to(vcx, &table, 100. * ROW_H);
    next_frame(vcx);
    let (ids, calls) = demand(vcx, &table);
    assert_eq!(ids.len(), 90, "30 visible, 30 above, 30 below");
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (70..160).collect::<Vec<u32>>());
    assert_eq!(calls, 1);
}

#[test]
fn visible_demand_capped_512_and_sent_on_change_only() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 400.);
    scroll_to(vcx, &table, 800. * ROW_H);
    next_frame(vcx);
    let (ids, calls) = demand(vcx, &table);
    assert_eq!(ids.len(), 512, "a 400-row viewport sends at most 512 ids");
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (744..1256).collect::<Vec<u32>>(),
        "centred on rows 800..1200"
    );
    assert_eq!(calls, 1);

    // A new frame with the same ids sends nothing.
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    next_frame(vcx);
    assert_eq!(demand(vcx, &table).1, 1);

    // Nor does a new version with the same visible ids.
    vcx.update(|_, cx| table.update(cx, |t, cx| t.on_new_version(cx)));
    redraw(vcx);
    next_frame(vcx);
    assert_eq!(demand(vcx, &table).1, 1);
}

#[test]
fn demand_resent_after_resort_with_same_range() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    next_frame(vcx);
    let (ids, calls) = demand(vcx, &table);
    assert_eq!(ids, (0..60).collect::<Vec<u32>>());
    assert_eq!(calls, 1);

    // Row 0 stays on top, the rest reverses: same range 0..30, other ids.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.rows[1..].reverse();
            t.on_new_version(cx);
        })
    });
    redraw(vcx);
    assert_eq!(vcx.update(|_, cx| table.read(cx).visible_range()), 0..30);
    next_frame(vcx);
    let (ids, calls) = demand(vcx, &table);
    assert_eq!(calls, 2, "a changed id set is sent again");
    let expected: Vec<u32> = std::iter::once(0)
        .chain((1940..2000).rev().take(59))
        .collect();
    assert_eq!(ids, expected);
}

// ---------------------------------------------------------------------------------------------
// rsf.20: file icon image sources are cached per table.
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "test-hooks")]
mod icon_cache {
    use std::sync::Arc;

    use super::*;

    /// `n` app rows with 24 px icons, `distinct` icon paths between them.
    fn app_rows(n: u32, distinct: u32, tag: &str) -> TDelegate {
        let rows = (0..n)
            .map(|id| TRow {
                icon: Some(IconSlot::Large),
                ..plain(id)
            })
            .collect();
        let mut d = TDelegate::new(rows);
        d.icon_paths = (0..distinct)
            .map(|i| Arc::from(format!("/nonexistent/tasklens-rsf20/{tag}-{i}.png")))
            .collect();
        d
    }

    fn icon_sources(
        vcx: &mut VisualTestContext,
        table: &Entity<TlTreeTable<TDelegate>>,
    ) -> (u32, usize) {
        vcx.update(|_, cx| table.read(cx).icon_sources())
    }

    #[test]
    fn icon_sources_built_once_over_60_frames() {
        let mut cx = cosmic_cx();
        // 26 visible app rows; 20 distinct icons, so some rows share one.
        let (table, vcx) = open_table(&mut cx, app_rows(26, 20, "a"), 600., 26. * ROW_H);
        assert_eq!(vcx.update(|_, cx| table.read(cx).visible_range()), 0..26);
        for _ in 0..60 {
            vcx.update(|window, _| window.refresh());
            redraw(vcx);
        }
        assert_eq!(
            vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get()),
            26
        );
        assert_eq!(icon_sources(vcx, &table), (20, 20));
    }

    #[test]
    fn unused_icon_sources_evicted_on_new_version() {
        let mut cx = cosmic_cx();
        let (table, vcx) = open_table(&mut cx, app_rows(26, 20, "a"), 600., 26. * ROW_H);
        assert_eq!(icon_sources(vcx, &table), (20, 20));

        // A new row set with other icons: the old ones stay one version, then go.
        let fresh = app_rows(26, 5, "b").icon_paths;
        let bump = |vcx: &mut VisualTestContext, paths: Option<Vec<Arc<str>>>| {
            vcx.update(|_, cx| {
                table.update(cx, |t, cx| {
                    if let Some(p) = paths {
                        t.delegate.icon_paths = p;
                    }
                    t.delegate.version += 1;
                    t.on_new_version(cx);
                })
            });
            redraw(vcx);
        };
        bump(vcx, Some(fresh));
        assert_eq!(icon_sources(vcx, &table), (25, 25));
        bump(vcx, None);
        assert_eq!(icon_sources(vcx, &table), (25, 5), "unused icons evicted");
        bump(vcx, None);
        assert_eq!(icon_sources(vcx, &table), (25, 5), "used icons kept");
    }
}

// ---------------------------------------------------------------------------------------------
// E18-A1: context menu, Shift+F10, Copy rows as table, text cache, render counter and the
// scroll handle (ADR-009, UI-PRC-008, UI-PRC-009, ADR-015, R-186).
// ---------------------------------------------------------------------------------------------

/// The data action of the test menu item: the targets captured when the menu opened.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tl_tree_table_test, no_json)]
struct EndRows {
    ids: Vec<u32>,
}

/// A page with the table, the window's context menu layer and an `EndRows` handler.
struct MenuHost {
    table: Entity<TlTreeTable<TDelegate>>,
    ended: Rc<RefCell<Vec<Vec<u32>>>>,
}

impl Render for MenuHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ended = self.ended.clone();
        div()
            .w(px(600.))
            .h(px(HEADER_H + 10. * ROW_H))
            .flex()
            .flex_col()
            .on_action(move |a: &EndRows, _, _| ended.borrow_mut().push(a.ids.clone()))
            .child(self.table.clone())
            .children(context_menu_layer(window, cx))
    }
}

type Ended = Rc<RefCell<Vec<Vec<u32>>>>;

fn open_menu_host(
    cx: &mut TestAppContext,
    delegate: TDelegate,
) -> (
    Entity<TlTreeTable<TDelegate>>,
    Ended,
    &mut VisualTestContext,
) {
    cx.update(|cx| tl_ui::i18n::init_strings(cx, "en-US"));
    let ended: Ended = Rc::default();
    let slot = Rc::new(RefCell::new(None));
    let (out, page_ended) = (slot.clone(), ended.clone());
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        *out.borrow_mut() = Some(table.clone());
        MenuHost {
            table,
            ended: page_ended,
        }
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    let table = slot.borrow_mut().take().expect("table created");
    (table, ended, vcx)
}

fn named(id: u32, name: &'static str) -> TRow {
    TRow { name, ..plain(id) }
}

fn right_click(vcx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>) {
    vcx.simulate_event(gpui_kit::MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Right,
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(gpui_kit::MouseUpEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Right,
        click_count: 1,
    });
    redraw(vcx);
}

fn menu_calls(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> Vec<(Vec<u32>, Option<ColumnId>)> {
    vcx.update(|_, cx| table.read(cx).delegate.menu_calls.clone())
}

/// The bounds of the open menu, if one is open.
fn menu_bounds(vcx: &mut VisualTestContext) -> Option<gpui_kit::Bounds<Pixels>> {
    vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::Menu))
            .map(|s| s.bounds())
    })
}

fn ctrl_select(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>, id: u32) {
    vcx.update(|_, cx| table.update(cx, |t, _| t.selection.ctrl_click(id)));
    redraw(vcx);
}

fn abc() -> Vec<TRow> {
    vec![named(1, "A"), named(2, "B"), named(3, "C")]
}

#[test]
fn right_click_selects_unselected_and_captures_targets() {
    let mut cx = cosmic_cx();
    let (table, _, vcx) = open_menu_host(&mut cx, TDelegate::new(abc()));
    select(vcx, &table, 1);
    // x 300 is in the CPU column (Name is 248 wide).
    let at = row_at(2, 300.);
    right_click(vcx, at);
    assert_eq!(sel(vcx, &table), (vec![3], Some(3)));
    assert_eq!(
        menu_calls(vcx, &table),
        vec![(vec![3], Some(ColumnId::Cpu))]
    );
    let menu = menu_bounds(vcx).expect("menu open");
    assert_eq!(menu.origin, at, "the menu opens at the pointer");
}

#[test]
fn right_click_keeps_multi_selection() {
    let mut cx = cosmic_cx();
    let (table, _, vcx) = open_menu_host(&mut cx, TDelegate::new(abc()));
    select(vcx, &table, 3);
    ctrl_select(vcx, &table, 1);
    right_click(vcx, row_at(2, 50.));
    assert_eq!(sel(vcx, &table).0, vec![1, 3]);
    assert_eq!(
        menu_calls(vcx, &table),
        vec![(vec![1, 3], Some(ColumnId::Name))],
        "targets in view order"
    );
}

#[test]
fn right_click_targets_survive_resort() {
    let mut cx = cosmic_cx();
    let (table, ended, vcx) = open_menu_host(&mut cx, TDelegate::new(abc()));
    right_click(vcx, row_at(2, 50.));
    assert_eq!(
        menu_calls(vcx, &table),
        vec![(vec![3], Some(ColumnId::Name))]
    );

    // A new version reverses the rows while the menu is open: C is now first, A last.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.rows.reverse();
            t.delegate.version += 1;
            t.on_new_version(cx);
        })
    });
    redraw(vcx);
    let item = vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::MenuItem))
            .expect("menu item")
            .bounds()
    });
    click(vcx, item.center(), Modifiers::none());
    assert_eq!(*ended.borrow(), vec![vec![3]], "the action still carries C");
    assert!(
        menu_bounds(vcx).is_none(),
        "running an item closes the menu"
    );
}

#[test]
fn group_header_right_click_no_selection_change() {
    let mut cx = cosmic_cx();
    let rows = vec![group(10), named(1, "A"), named(2, "B")];
    let (table, _, vcx) = open_menu_host(&mut cx, TDelegate::new(rows));
    select(vcx, &table, 2);
    right_click(vcx, row_at(0, 50.));
    assert_eq!(sel(vcx, &table), (vec![2], Some(2)), "selection unchanged");
    assert_eq!(
        menu_calls(vcx, &table),
        vec![(vec![10], Some(ColumnId::Name))]
    );
    assert_eq!(gestures(vcx, &table), (vec![], vec![]), "no toggle");
}

#[test]
fn shift_f10_opens_at_focus_row() {
    let mut cx = cosmic_cx();
    let mut rows = abc();
    rows[1].icon = Some(IconSlot::Small);
    let (table, _, vcx) = open_menu_host(&mut cx, TDelegate::new(rows));
    focus_table(vcx, &table);
    // No focus row: nothing.
    keys(vcx, "shift-f10");
    assert!(menu_calls(vcx, &table).is_empty());
    assert!(menu_bounds(vcx).is_none());

    select(vcx, &table, 2);
    keys(vcx, "shift-f10");
    assert_eq!(
        menu_calls(vcx, &table),
        vec![(vec![2], None)],
        "the keyboard names no column"
    );
    let menu = menu_bounds(vcx).expect("menu open");
    // Name text start: pad 20, chevron slot 16 after the 6 px gap, then a 16 px icon and the
    // text, each after a 6 px gap.
    let text_x = 20. + 6. + 16. + 6. + 16. + 6.;
    let b_bottom = HEADER_H + 2. * ROW_H;
    assert_eq!(menu.origin, point(px(text_x), px(b_bottom)));
}

/// Subscribes to the table's events.
fn events(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> (Rc<RefCell<Vec<TreeTableEvent>>>, Subscription) {
    let seen: Rc<RefCell<Vec<TreeTableEvent>>> = Rc::default();
    let log = seen.clone();
    let sub = vcx.update(|_, cx| {
        cx.subscribe(table, move |_, ev: &TreeTableEvent, _| {
            log.borrow_mut().push(*ev)
        })
    });
    (seen, sub)
}

fn clipboard(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.read_from_clipboard().and_then(|c| c.text())
}

#[test]
fn copy_rows_as_table_tsv_with_header() {
    let mut cx = cosmic_cx();
    let mut d = TDelegate::name_pid_cpu(abc());
    d.cells = vec![
        (2, ColumnId::Pid, "12", Validity::Valid),
        (2, ColumnId::Cpu, "3.4%", Validity::Valid),
        (3, ColumnId::Pid, "13", Validity::Valid),
        (3, ColumnId::Cpu, "", Validity::WarmingUp),
    ];
    let (table, _, vcx) = open_menu_host(&mut cx, d);
    let (seen, _sub) = events(vcx, &table);
    focus_table(vcx, &table);

    // No selection: nothing copied, nothing emitted.
    keys(vcx, "ctrl-shift-c");
    assert_eq!(clipboard(vcx), None);
    assert!(seen.borrow().is_empty());

    select(vcx, &table, 3);
    ctrl_select(vcx, &table, 2);
    keys(vcx, "ctrl-shift-c");
    assert_eq!(
        clipboard(vcx).as_deref(),
        Some("Name\tPID\tCPU\nB\t12\t3.4%\nC\t13\t\u{2026}")
    );
    assert_eq!(*seen.borrow(), vec![TreeTableEvent::CopiedRows { rows: 2 }]);
}

#[test]
fn copy_rows_glyph_texts() {
    let mut cx = cosmic_cx();
    let rows = vec![
        group(10),
        named(1, "A"),
        named(2, "B"),
        named(3, "C"),
        named(4, "D"),
        named(5, "E"),
        named(6, "F"),
    ];
    let mut d = TDelegate::name_pid_cpu(rows);
    // PID hidden: only visible columns are copied.
    let order = [ColumnId::Name, ColumnId::Pid, ColumnId::Cpu];
    d.layout = ColumnLayout::defaults(
        TableId::Processes,
        &d.specs,
        &order,
        &[ColumnId::Name, ColumnId::Cpu],
    );
    d.cells = vec![
        (1, ColumnId::Cpu, "", Validity::Restricted),
        (2, ColumnId::Cpu, "", Validity::Unsupported),
        (3, ColumnId::Cpu, "", Validity::Missing),
        (4, ColumnId::Cpu, "", Validity::Failed),
        (5, ColumnId::Cpu, "", Validity::WarmingUp),
        (6, ColumnId::Cpu, "7.0%", Validity::Stale),
    ];
    let (table, _, vcx) = open_menu_host(&mut cx, d);
    focus_table(vcx, &table);
    keys(vcx, "ctrl-a ctrl-shift-c");
    assert_eq!(
        clipboard(vcx).as_deref(),
        Some("Name\tCPU\nA\tRestricted\nB\tn/a\nC\t-\nD\t!\nE\t\u{2026}\nF\t7.0%"),
        "the group header is never copied"
    );
}

#[test]
fn selection_survives_resort_in_entity() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 5, 5.);
    click(vcx, row_at(2, 50.), Modifiers::none());
    assert_eq!(sel(vcx, &table), (vec![2], Some(2)));
    move_row(vcx, &table, 2, 4);
    assert_eq!(
        sel(vcx, &table),
        (vec![2], Some(2)),
        "selection by identity"
    );
    let (bg, selected) = vcx.update(|_, cx| {
        let theme = current(cx).clone();
        (table.read(cx).row_background(4, &theme), theme.row_selected)
    });
    assert_eq!(
        bg, selected,
        "the moved row draws selected at its new index"
    );
}

#[test]
fn scroll_handle_clone_scrolls_table() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    let handle = vcx.update(|_, cx| table.read(cx).scroll_handle());
    handle.scroll_to_item_with_offset(1000, ScrollStrategy::Top, 0);
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    let first = vcx.update(|_, cx| table.read(cx).visible_range().start);
    assert_eq!(first, 1000);
}

#[cfg(feature = "test-hooks")]
#[test]
fn tree_table_text_cache_hit_rate() {
    let mut cx = cosmic_cx();
    let rows = (0..200).map(plain).collect();
    let (table, vcx) = open_table(&mut cx, TDelegate::name_pid_cpu(rows), 600., 30. * ROW_H);
    // The first version fills the cache; measure the next 10.
    let stats = |vcx: &mut VisualTestContext| vcx.update(|_, cx| table.read(cx).text_cache_stats());
    let (h0, m0) = stats(vcx);
    assert!(m0 > 0, "the first frame formats");
    for _ in 0..10 {
        vcx.update(|_, cx| {
            table.update(cx, |t, cx| {
                t.delegate.version += 1;
                t.on_new_version(cx);
            })
        });
        redraw(vcx);
    }
    let (h1, m1) = stats(vcx);
    let (hits, misses) = (h1 - h0, m1 - m0);
    assert!(hits > 0);
    let rate = hits as f64 / (hits + misses) as f64;
    assert!(
        rate > 0.95,
        "hit rate {rate} ({hits} hits, {misses} misses)"
    );
}

/// A page that gets snapshots but never installs them into its table.
#[cfg(feature = "test-hooks")]
struct SnapshotHost {
    table: Entity<TlTreeTable<TDelegate>>,
    _sub: Subscription,
}

#[cfg(feature = "test-hooks")]
impl Render for SnapshotHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(600.))
            .h(px(300.))
            .flex()
            .flex_col()
            .child(self.table.clone())
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn render_counter_unchanged_without_new_version() {
    use std::sync::Arc;

    use tl_model::client::EngineClient;
    use tl_ui::hub::{SnapshotChanged, SnapshotHub};
    use tl_ui::testing::fake_engine::FakeEngine;

    let mut cx = cosmic_cx();
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(client, cx));
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let page_hub = hub.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let rows = (0..10).map(plain).collect();
        let table = cx.new(|cx| TlTreeTable::new(TDelegate::new(rows), window, cx));
        *out.borrow_mut() = Some(table.clone());
        let sub = cx.subscribe(&page_hub, |_, _, _: &SnapshotChanged, _| {});
        SnapshotHost { table, _sub: sub }
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    let table = slot.borrow_mut().take().expect("table created");
    let renders =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| table.read(cx).render_count.get());
    let before = renders(vcx);

    engine.publish_with(|_| {});
    vcx.run_until_parked();
    let seq = vcx.update(|_, cx| hub.read(cx).current().sequence);
    assert_eq!(seq, 1, "the hub installed the snapshot");
    assert_eq!(renders(vcx), before, "no render without on_new_version");

    vcx.update(|_, cx| table.update(cx, |t, cx| t.on_new_version(cx)));
    vcx.run_until_parked();
    assert!(renders(vcx) > before, "on_new_version renders");
}

// E18 rsf.21: the Name cell keeps its count right of an RTL name, the Binary icon has an asset,
// and the Name column's minimum is 120.
mod name_cell_fixes {
    use std::sync::Arc;

    use gpui_kit::{AssetSource as _, MouseDownEvent, MouseUpEvent, SvgRenderer, TextRun};
    use tl_ui::components::icons::TlAssets;
    use tl_ui::table::cells::{IconSourceCache, name_primary_text};

    use super::*;

    const HEBREW: &str =
        "\u{05EA}\u{05D4}\u{05DC}\u{05D9}\u{05DA}-\u{05D1}\u{05D3}\u{05D9}\u{05E7}\u{05D4} (3)";
    /// UTF-8 length of FSI and of PDI.
    const MARK: usize = 3;

    /// Largest x of a glyph whose byte index is in `name` and smallest x of one in `count`.
    fn name_max_count_min(
        line: &gpui_kit::ShapedLine,
        name: std::ops::Range<usize>,
        count: std::ops::RangeFrom<usize>,
    ) -> (f32, f32) {
        let (mut name_max, mut count_min) = (f32::MIN, f32::MAX);
        for g in line.runs.iter().flat_map(|r| &r.glyphs) {
            let x = f32::from(g.position.x);
            if name.contains(&g.index) {
                name_max = name_max.max(x);
            } else if count.contains(&g.index) {
                count_min = count_min.min(x);
            }
        }
        assert!(
            name_max > f32::MIN && count_min < f32::MAX,
            "both parts have glyphs"
        );
        (name_max, count_min)
    }

    #[test]
    fn hebrew_name_paints_count_on_the_right() {
        let mut cx = cosmic_cx();
        let vcx = cx.add_empty_window();
        vcx.update(|window, cx| {
            let primary: SharedString = HEBREW.into();
            let start = HEBREW.find('(').expect("count");
            let count = start..HEBREW.len();
            let mut cache = IconSourceCache::new();
            let (text, runs) = name_primary_text(
                &primary,
                true,
                Some(&count),
                gpui_kit::white(),
                gpui_kit::red(),
                &mut cache,
            );
            let runs = runs.expect("a count gives runs");
            assert_eq!(text.len(), primary.len() + 2 * MARK);
            assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
            assert!(text.starts_with('\u{2068}'));
            // PDI comes before the space, which stays between the name and the count.
            let name_end = start - 1;
            assert_eq!(&text[MARK + name_end..MARK + name_end + MARK], "\u{2069}");
            assert_eq!(&text[name_end + 2 * MARK..start + 2 * MARK], " ");

            let size = metrics(cx).text.base;
            let isolated = window
                .text_system()
                .shape_line(text.clone(), size, &runs, None);
            let (name_max, count_min) =
                name_max_count_min(&isolated, MARK..MARK + name_end, start + 2 * MARK..);
            assert!(
                count_min > name_max,
                "count at {count_min} must be right of the name ending at {name_max}"
            );

            // Without the isolate the paragraph is RTL and the count lands on the left.
            let plain_run = TextRun {
                len: primary.len(),
                ..runs[0].clone()
            };
            let plain = window
                .text_system()
                .shape_line(primary.clone(), size, &[plain_run], None);
            let (name_max, count_min) = name_max_count_min(&plain, 0..start, start..);
            assert!(count_min < name_max, "the plain text shows the defect");

            // FSI and PDI add no width.
            assert!(
                (f32::from(isolated.width) - f32::from(plain.width)).abs() < 0.5,
                "{:?} vs {:?}",
                isolated.width,
                plain.width
            );
        });
    }

    #[test]
    fn isolated_name_built_once() {
        // Longer than `SharedString`'s 23 inline bytes, so clones share one heap text.
        const NAME: &str = "systemd-journal-upload (12)";
        let primary: SharedString = NAME.into();
        let mut cache = IconSourceCache::new();
        cache.begin_version(1);
        let a = cache.isolated_name(&primary, 23);
        let b = cache.isolated_name(&SharedString::from(NAME.to_string()), 23);
        assert_eq!(a.as_ref(), "\u{2068}systemd-journal-upload\u{2069} (12)");
        assert!(
            std::ptr::eq(a.as_ref(), b.as_ref()),
            "the second call reuses the text"
        );
        #[cfg(feature = "test-hooks")]
        assert_eq!(cache.names_built, 1);

        // A name without a count is shaped as it is.
        let (text, runs) = name_primary_text(
            &primary,
            false,
            None,
            gpui_kit::white(),
            gpui_kit::red(),
            &mut cache,
        );
        assert!(runs.is_none());
        assert_eq!(text, primary);
    }

    #[test]
    fn binary_icon_renders_a_sprite() {
        // The Name cell draws `IconName::Binary` for Lucide binary icons and as the file icon
        // fallback; the app's asset source must have it.
        let path = IconName::Binary.path();
        let bytes = TlAssets
            .load(&path)
            .unwrap()
            .unwrap_or_else(|| panic!("{path} not in TlAssets"));
        let image = SvgRenderer::new(Arc::new(TlAssets))
            .render_single_frame(&bytes, 1.0)
            .expect("svg renders");
        let pixels = image.as_bytes(0).expect("one frame");
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "the sprite has coverage"
        );
    }

    #[test]
    fn name_resize_clamps_to_120() {
        let mut cx = cosmic_cx();
        let (table, vcx) = open_table(
            &mut cx,
            TDelegate::new((0..3).map(plain).collect()),
            600.,
            200.,
        );
        let header = |x: f32| point(px(x), px(12.));
        vcx.simulate_event(MouseDownEvent {
            position: header(248.),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        redraw(vcx);
        for x in [200., 100., 30.] {
            vcx.simulate_event(MouseMoveEvent {
                position: header(x),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::none(),
            });
            redraw(vcx);
        }
        vcx.simulate_event(MouseUpEvent {
            position: header(30.),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
        redraw(vcx);
        let name_w = vcx.update(|_, cx| table.read(cx).delegate.layout.widths[&ColumnId::Name]);
        assert_eq!(name_w, px(120.));
    }
}

// ---------------------------------------------------------------------------------------------
// Header menu on a system table (rsf.22)
// ---------------------------------------------------------------------------------------------

/// A Services table with Name and Display Name. Display Name has no process column key, so the
/// header menu actions must carry the key itself.
struct SysDelegate {
    specs: Vec<ColumnSpec<SysColumnId>>,
    layout: ColumnLayout<SysColumnId>,
    sort: SysSortSpec,
}

const SYS_NAME: SysColumnId = SysColumnId::Services(ServiceColumn::Name);
const SYS_DISPLAY: SysColumnId = SysColumnId::Services(ServiceColumn::DisplayName);

impl SysDelegate {
    fn new() -> Self {
        let spec = |id, label: &'static str| ColumnSpec {
            id,
            label: label.into(),
            full_name: label.into(),
            group: label.into(),
            align: ColumnAlign::Left,
            kind: ColumnKind::Text,
            sortable: true,
            template: None,
            default_width: px(248.),
            tooltip: None,
            availability: Availability::Available,
            hideable: true,
        };
        let specs = vec![spec(SYS_NAME, "Name"), spec(SYS_DISPLAY, "Display Name")];
        let order = [SYS_NAME, SYS_DISPLAY];
        Self {
            layout: ColumnLayout::defaults(TableId::Services, &specs, &order, &order),
            specs,
            sort: SysSortSpec {
                column: SYS_NAME,
                ascending: true,
            },
        }
    }
}

impl TreeTableDelegate for SysDelegate {
    type RowId = u32;
    type Column = SysColumnId;
    type Sort = SysSortSpec;

    fn row_count(&self) -> usize {
        2
    }
    fn row_id(&self, ix: usize) -> u32 {
        ix as u32
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        (id < 2).then_some(id as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
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
    fn columns(&self) -> &ColumnLayout<SysColumnId> {
        &self.layout
    }
    fn column_spec(&self, id: SysColumnId) -> &ColumnSpec<SysColumnId> {
        self.specs
            .iter()
            .find(|s| s.id == id)
            .expect("spec for every column")
    }
    fn cell(&self, ix: usize, _: SysColumnId, out: &mut CellOut) {
        out.bits = ix as u64;
    }
    fn name_cell(&self, _: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::new_static("unit"),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn header_total(&self, _: SysColumnId, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SysSortSpec {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }
    fn context_menu(
        &mut self,
        _: &[u32],
        _: Option<SysColumnId>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, col: SysColumnId, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = SysSortSpec {
            column: col,
            ascending: !(self.sort.column == col && self.sort.ascending),
        };
    }
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SysColumnId>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
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
        HeaderLines::One
    }
}

/// The system table with the window's context menu layer.
struct SysMenuHost {
    table: Entity<TlTreeTable<SysDelegate>>,
}

impl Render for SysMenuHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(600.))
            .h(px(HEADER_H + 10. * ROW_H))
            .flex()
            .flex_col()
            .child(self.table.clone())
            .children(context_menu_layer(window, cx))
    }
}

/// Right-clicks the Display Name header and clicks the menu item `label`.
fn sys_header_menu_click(vcx: &mut VisualTestContext, label: &str) {
    right_click(vcx, point(px(300.), px(12.)));
    let item = vcx.update(|window, _| {
        snapshots(window)
            .into_iter()
            .find(|s| s.role() == Some(Role::MenuItem) && s.label() == Some(label))
            .unwrap_or_else(|| panic!("menu item {label:?}"))
            .bounds()
    });
    click(vcx, item.center(), Modifiers::none());
}

#[test]
fn sys_table_header_menu_sorts_and_hides() {
    assert!(
        ColumnId::from_str(SYS_DISPLAY.as_str()).is_none(),
        "Display Name has no process column key"
    );
    let mut cx = cosmic_cx();
    cx.update(|cx| tl_ui::i18n::init_strings(cx, "en-US"));
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(SysDelegate::new(), window, cx));
        *out.borrow_mut() = Some(table.clone());
        SysMenuHost { table }
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    let table = slot.borrow_mut().take().expect("table created");
    let handle = vcx.update(|_, cx| table.read(cx).focus_handle().clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    redraw(vcx);

    let rows = vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        header_menu_rows(
            d.column_spec(SYS_DISPLAY),
            &d.layout,
            d.sort.column,
            false,
            cx,
        )
        .to_vec()
    });
    for item in [HeaderMenuItem::SortBy, HeaderMenuItem::Hide] {
        let row = rows.iter().find(|r| r.item == item).expect("row");
        assert!(row.enabled, "{item:?} is enabled on a system column");
    }

    sys_header_menu_click(vcx, "Sort by Display Name");
    let sort = vcx.update(|_, cx| table.read(cx).delegate.sort);
    assert_eq!(sort.column, SYS_DISPLAY, "sorted by the clicked column");

    sys_header_menu_click(vcx, "Hide This Column");
    let visible = vcx.update(|_, cx| table.read(cx).delegate.layout.visible_columns().to_vec());
    assert_eq!(visible, vec![SYS_NAME], "Display Name is hidden");
}

// ---------------------------------------------------------------------------------------------
// E18-017: first visible row across a UI scale change (UI-SHL-130 step 5)
// ---------------------------------------------------------------------------------------------

fn first_row(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> Option<(u32, usize)> {
    vcx.update(|_, cx| table.read(cx).first_visible_row())
}

#[test]
fn scale_keeps_first_row() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_rows(&mut cx, 2000, 30.);
    scroll_to(vcx, &table, 300. * ROW_H);
    let anchor = first_row(vcx, &table).expect("rows");
    assert_eq!(anchor, (300, 300));

    vcx.update(|_, cx| {
        let appearance = AppearanceSettings {
            ui_scale_pct: 150,
            ..AppearanceSettings::default()
        };
        tl_ui::theme::store::apply_metrics(&appearance, cx);
        assert_eq!(metrics(cx).row_height, px(42.));
        table.update(cx, |t, cx| t.restore_first_visible_row(anchor, cx));
    });
    redraw(vcx);
    assert_eq!(first_row(vcx, &table), Some((300, 300)));
    assert_eq!(scroll_top(vcx, &table), 300. * 42.);
}

#[test]
fn restore_first_row_falls_back_to_index() {
    let mut cx = cosmic_cx();
    // One row tall, so the last row can be the first visible row.
    let (table, vcx) = open_rows(&mut cx, 2000, 1.);
    scroll_to(vcx, &table, 300. * ROW_H);
    let anchor = first_row(vcx, &table).expect("rows");
    assert_eq!(anchor, (300, 300));

    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.delegate.rows.truncate(120);
            t.on_new_version(cx);
        })
    });
    // From the top, so only the restore can bring the view down to the end.
    scroll_to(vcx, &table, 0.);
    vcx.update(|_, cx| table.update(cx, |t, cx| t.restore_first_visible_row(anchor, cx)));
    redraw(vcx);
    assert_eq!(first_row(vcx, &table), Some((119, 119)));
    assert_eq!(scroll_top(vcx, &table), 119. * ROW_H);
}

#[test]
fn first_visible_row_empty_table() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, TDelegate::new(Vec::new()), 600., 300.);
    assert_eq!(first_row(vcx, &table), None);
    vcx.update(|_, cx| table.update(cx, |t, cx| t.restore_first_visible_row((5, 300), cx)));
    redraw(vcx);
    assert_eq!(scroll_top(vcx, &table), 0.);
}

// Column separators (E18-014).

/// Columns Name, PID, CPU, Memory and User: 540 px in all.
#[cfg(feature = "test-hooks")]
fn five_columns(rows: Vec<TRow>) -> TDelegate {
    let mut d = TDelegate::new(rows);
    d.specs = vec![
        spec(ColumnId::Name, ColumnKind::Text, 200.),
        spec(ColumnId::Pid, ColumnKind::Numeric, 80.),
        spec(ColumnId::Cpu, ColumnKind::Numeric, 80.),
        spec(ColumnId::Memory, ColumnKind::Numeric, 80.),
        spec(ColumnId::User, ColumnKind::Text, 100.),
    ];
    let order = [
        ColumnId::Name,
        ColumnId::Pid,
        ColumnId::Cpu,
        ColumnId::Memory,
        ColumnId::User,
    ];
    d.layout = ColumnLayout::defaults(TableId::Processes, &d.specs, &order, &order);
    d
}

#[cfg(feature = "test-hooks")]
fn separators(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<TDelegate>>,
) -> Vec<gpui_kit::Bounds<Pixels>> {
    vcx.update(|_, cx| table.read(cx).separator_quads.borrow().clone())
}

#[cfg(feature = "test-hooks")]
fn separators_on(vcx: &mut VisualTestContext, table: &Entity<TlTreeTable<TDelegate>>) {
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.set_column_separators(true);
            cx.notify();
        })
    });
    redraw(vcx);
}

#[cfg(feature = "test-hooks")]
#[test]
fn separators_off_by_default() {
    let mut cx = cosmic_cx();
    let rows = (0..3).map(plain).collect();
    let (table, vcx) = open_table(&mut cx, five_columns(rows), 600., 200.);
    assert!(vcx.debug_bounds("TlCell-numeric").is_some(), "rows drawn");
    assert!(separators(vcx, &table).is_empty());
}

#[cfg(feature = "test-hooks")]
#[test]
fn separators_on_one_per_boundary() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, five_columns(vec![plain(0)]), 600., 200.);
    separators_on(vcx, &table);
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    let line = vcx.update(|window, _| px(1.0 / window.scale_factor()));
    let quads = separators(vcx, &table);
    let xs: Vec<Pixels> = quads.iter().map(|q| q.left() - body.left()).collect();
    let expected: Vec<Pixels> = [200., 280., 360., 440., 540.]
        .iter()
        .map(|&b| px(b) - line)
        .collect();
    assert_eq!(xs, expected);
    for q in &quads {
        assert_eq!(q.size.width, line, "one physical px");
        assert_eq!(q.size.height, px(ROW_H), "full row height");
        assert_eq!(q.top(), body.top());
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn separators_skip_group_headers() {
    let mut cx = cosmic_cx();
    let rows = vec![group(0), plain(1)];
    let (table, vcx) = open_table(&mut cx, five_columns(rows), 600., 200.);
    separators_on(vcx, &table);
    let body = vcx.debug_bounds("TlTreeTable-body").expect("body");
    let quads = separators(vcx, &table);
    assert_eq!(quads.len(), 5, "only the plain row");
    assert!(quads.iter().all(|q| q.top() == body.top() + px(ROW_H)));
}

#[cfg(feature = "test-hooks")]
#[test]
fn separators_move_with_horizontal_scroll() {
    let mut cx = cosmic_cx();
    let (table, vcx) = open_table(&mut cx, five_columns(vec![plain(0)]), 300., 200.);
    separators_on(vcx, &table);
    let before = separators(vcx, &table);
    let cell = vcx.debug_bounds("TlCell-numeric").expect("pid cell");
    vcx.update(|_, cx| {
        let handle = table.read(cx).scroll_handle();
        ScrollbarHandle::set_offset(&handle, point(px(-100.), px(0.)));
    });
    vcx.update(|window, _| window.refresh());
    redraw(vcx);
    let after = separators(vcx, &table);
    let cell2 = vcx
        .debug_bounds("TlCell-numeric")
        .expect("pid cell after scroll");
    assert_eq!(cell2.left(), cell.left() - px(100.));
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(&before) {
        assert_eq!(a.left(), b.left() - px(100.));
    }
}

// ---------------------------------------------------------------------------------------------
// tasklens-rsf.25: `CellOut::want_tooltip` on the hovered row only.
// ---------------------------------------------------------------------------------------------

#[test]
fn want_tooltip_only_on_hovered_row() {
    let mut cx = cosmic_cx();
    let mut d = TDelegate::new((0..3).map(plain).collect());
    d.cells = vec![(1, ColumnId::Cpu, "4.0%", Validity::Stale)];
    let (table, vcx) = open_table(&mut cx, d, 600., 200.);

    // 60 frames with no hover: cells are built, but none is asked for a tooltip.
    let calls_before = vcx.update(|_, cx| table.read(cx).delegate.cell_calls.get());
    for _ in 0..60 {
        vcx.update(|window, _| window.refresh());
        redraw(vcx);
    }
    let (calls, tip_rows, built) = vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        (
            d.cell_calls.get(),
            d.tip_rows.borrow().clone(),
            d.tips_built.borrow().len(),
        )
    });
    assert!(calls > calls_before, "the frames built cells");
    assert!(tip_rows.is_empty(), "no row is hovered: {tip_rows:?}");
    assert_eq!(built, 0, "no tooltip text allocated");

    // Hover the Stale CPU cell of row 1 (Name is 248 px wide, CPU 80 px). The first move sets
    // the hovered row, the second makes its tooltips ready, the third starts the tooltip delay.
    for _ in 0..3 {
        hover_at(vcx, 290., HEADER_H + 28. + 14.);
    }
    vcx.executor().advance_clock(DELAY_CONTROL * 2);
    redraw(vcx);
    let (tip_rows, built) = vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        (d.tip_rows.borrow().clone(), d.tips_built.borrow().clone())
    });
    assert!(!tip_rows.is_empty(), "the hovered row asks for tooltips");
    assert!(
        tip_rows.iter().all(|&ix| ix == 1),
        "only row 1 is hovered: {tip_rows:?}"
    );
    assert!(
        built.iter().all(|t| t.as_ref() == "Last updated 12s ago"),
        "{built:?}"
    );
    assert!(!built.is_empty());
    assert!(
        vcx.debug_bounds("TlTooltip").is_some(),
        "the Stale cell shows its age tooltip"
    );
}
