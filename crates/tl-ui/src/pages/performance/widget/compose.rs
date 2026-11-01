//! Widget compositions (UI-PRF-061, ADR-019): the fixed arrangement a widget uses for its forms
//! at its size, the minimum size of each, and the order a narrow Row drops its parts in.
//!
//! A widget uses the first composition that fits its forms and size:
//!
//! - **Cell**: the widget shows a chart and is at least the Cell minimum. A header line with the
//!   name and the value, the stats line, the bar, then the chart filling the rest.
//! - **Tile**: no chart, and at least the Tile minimum. The name, the value under it in large
//!   text, the stats line, and the bar along the bottom.
//! - **Row**: everything else, and the fallback when the widget is below its other minimum. One
//!   line: name, bar, value, stats, and a sparkline of the chart.
//!
//! A Row narrower than its parts drops them in this order: stats, then the bar (the sparkline
//! takes the bar's place), then the sparkline, then the name. The value stays. The last form a
//! widget has is never dropped.
//!
//! A widget in a device page container draws a Cell in the page style (ADR-019 phase 3 "Page
//! style"): the chart has the 1.3.0 page chart's heading row (the name and the value) above
//! the plot and its range text under it, so its minimum is taller ([`page_cell_min`]). Tile
//! and Row are the same in both styles.
//!
//! Every size here is logical px at `ui_scale_pct = 100`; the functions take the UI scale and
//! multiply. This module does no drawing and keeps no state.

use gpui_kit::{Pixels, Size, px, size};
use tl_model::layout::{FixedWidget, FormSet, GroupItems, WidgetContent, WidgetSetting};

/// The header line of a Cell: name at the left, value at the right.
pub const CELL_HEADER_H: f32 = 18.;
/// The stats line of a Cell.
pub const CELL_STATS_H: f32 = 16.;
/// The bar of a Cell: `METER_H` 6 px with 4 px above and below.
pub const CELL_BAR_H: f32 = 14.;
/// The space above and below the bar of a Cell.
pub const CELL_BAR_PAD: f32 = 4.;
/// The smallest chart a Cell draws.
pub const CELL_CHART_MIN_H: f32 = 30.;
/// The narrowest Cell.
pub const CELL_MIN_W: f32 = 120.;

/// The heading row of a page style Cell: the chart's title row (`labels::TITLE_ROW`).
pub const PAGE_CELL_HEADING_H: f32 = 20.;
/// The range text under the plot of a page style Cell (`labels::LABEL_ROW`).
pub const PAGE_CELL_RANGE_H: f32 = 18.;

/// The name and value of a Tile.
pub const TILE_BASE_H: f32 = 40.;
/// The stats line of a Tile.
pub const TILE_STATS_H: f32 = 16.;
/// The bar of a Tile.
pub const TILE_BAR_H: f32 = 10.;
/// The narrowest Tile.
pub const TILE_MIN_W: f32 = 96.;

/// The line of a Row. A taller Row grows its bar and sparkline (D-040).
pub const ROW_H: f32 = 18.;
/// The name column of a Row, its trailing space included.
pub const ROW_NAME_W: f32 = 72.;
/// The narrowest bar a Row keeps.
pub const ROW_BAR_MIN_W: f32 = 40.;
/// The value column of a Row.
pub const ROW_VALUE_W: f32 = 48.;
/// The stats column of a Row ("min 6%  avg 42%  peak 100%" at `t_xs`).
pub const ROW_STATS_W: f32 = 160.;
/// The sparkline column of a Row next to a bar (`parts::SPARK_W`). Without a bar the sparkline
/// takes the bar's place and is as narrow as a bar may be.
pub const ROW_SPARK_W: f32 = 96.;
/// The gap between the parts of a Row after the name.
pub const ROW_GAP: f32 = 8.;
/// The narrowest Row with a name.
pub const ROW_MIN_W_NAME: f32 = 120.;
/// The narrowest Row without a name.
pub const ROW_MIN_W: f32 = 48.;

/// The order a narrow Row drops its parts in. CHART stands for the sparkline.
pub const ROW_DROP_ORDER: [FormSet; 4] =
    [FormSet::STATS, FormSet::BAR, FormSet::CHART, FormSet::LABEL];

/// The three arrangements of UI-PRF-061. The default is the fallback, a Row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Composition {
    Cell,
    Tile,
    #[default]
    Row,
}

/// What a widget draws at its size: the composition and the forms it keeps there. In a Row,
/// CHART is the sparkline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Composed {
    pub composition: Composition,
    pub forms: FormSet,
}

impl Composed {
    /// A number that changes when the composition or the kept forms change: the class the
    /// widget's size probe compares (`parts::class_probe`).
    pub fn class(self) -> u32 {
        let c = match self.composition {
            Composition::Cell => 0,
            Composition::Tile => 1,
            Composition::Row => 2,
        };
        c | (u32::from(self.forms.bits()) << 2)
    }
}

/// The smallest Cell for `forms`, logical px: 120 wide, 30 for the chart plus 18 for the header
/// (with a name or a value), 16 for stats and 14 for the bar.
pub fn cell_min(forms: FormSet) -> Size<f32> {
    let mut h = CELL_CHART_MIN_H;
    if forms.intersects(FormSet::LABEL | FormSet::VALUE) {
        h += CELL_HEADER_H;
    }
    if forms.contains(FormSet::STATS) {
        h += CELL_STATS_H;
    }
    if forms.contains(FormSet::BAR) {
        h += CELL_BAR_H;
    }
    size(CELL_MIN_W, h)
}

/// The smallest page style Cell for `forms`, logical px (ADR-019 phase 3): 120 wide, 30 for the
/// plot, 20 for the heading row (with a name or a value), 18 for the range text, 16 for stats
/// and 14 for the bar.
pub fn page_cell_min(forms: FormSet) -> Size<f32> {
    let mut h = CELL_CHART_MIN_H + PAGE_CELL_RANGE_H;
    if forms.intersects(FormSet::LABEL | FormSet::VALUE) {
        h += PAGE_CELL_HEADING_H;
    }
    if forms.contains(FormSet::STATS) {
        h += CELL_STATS_H;
    }
    if forms.contains(FormSet::BAR) {
        h += CELL_BAR_H;
    }
    size(CELL_MIN_W, h)
}

/// The smallest Tile for `forms`, logical px: 96 × 40, plus 16 for stats and 10 for the bar.
pub fn tile_min(forms: FormSet) -> Size<f32> {
    let mut h = TILE_BASE_H;
    if forms.contains(FormSet::STATS) {
        h += TILE_STATS_H;
    }
    if forms.contains(FormSet::BAR) {
        h += TILE_BAR_H;
    }
    size(TILE_MIN_W, h)
}

/// The smallest Row for `forms`, logical px: 120 wide with a name, else 48, and 18 high.
pub fn row_min(forms: FormSet) -> Size<f32> {
    let w = if forms.contains(FormSet::LABEL) {
        ROW_MIN_W_NAME
    } else {
        ROW_MIN_W
    };
    size(w, ROW_H)
}

/// The smallest size of `composition` for `forms`, logical px.
pub fn min_size(composition: Composition, forms: FormSet) -> Size<f32> {
    match composition {
        Composition::Cell => cell_min(forms),
        Composition::Tile => tile_min(forms),
        Composition::Row => row_min(forms),
    }
}

/// The smallest composition of `forms`, logical px: a Row. The inner grid takes its minimum
/// span from this (ADR-019 "Minimum span").
pub fn smallest(forms: FormSet) -> Size<f32> {
    row_min(forms)
}

/// The composition each item of a group drawn as `items` uses (ADR-019 "Group widgets"): a
/// Row for Rows; a Cell for Cells, or a Tile when the forms have no chart.
pub fn group_item_composition(items: GroupItems, forms: FormSet) -> Composition {
    match items {
        GroupItems::Rows => Composition::Row,
        GroupItems::Cells if forms.contains(FormSet::CHART) => Composition::Cell,
        GroupItems::Cells => Composition::Tile,
    }
}

/// The smallest item of a group drawn as `items` with `forms`, logical px: the Row minimum
/// width × 18 px for Rows, the Cell or Tile minimum for Cells. It is the group's minimum span
/// too: one item; more items scroll.
pub fn group_item_min(items: GroupItems, forms: FormSet) -> Size<f32> {
    min_size(group_item_composition(items, forms), forms)
}

/// The smallest fixed widget of `kind`, logical px (ADR-019 "Fixed widgets"): Processes and
/// Timeline at their box body's kind minimum, Memory by app where its legend shows, and the
/// page parts at the minimums of the ADR-019 phase 3 fixed widget table.
pub fn fixed_min(kind: &FixedWidget) -> Size<f32> {
    match kind {
        FixedWidget::Processes => size(256., 192.),
        FixedWidget::Timeline => size(376., 92.),
        FixedWidget::MemoryByApp => size(160., 100.),
        FixedWidget::Details
        | FixedWidget::MemoryCompressed
        | FixedWidget::SwapDevices
        | FixedWidget::MemoryModules
        | FixedWidget::DiskPartitions => size(360., 74.),
        FixedWidget::CpuCoreGrid | FixedWidget::GpuEngineGrid => size(120., 112.),
        FixedWidget::CpuCoreBoard => size(200., 60.),
        FixedWidget::MemoryBar => size(160., 44.),
        FixedWidget::GpuDisplays => size(360., 48.),
        FixedWidget::NetSettings => size(160., 24.),
        FixedWidget::BatteryHistory => size(200., 80.),
        FixedWidget::Other(_) => row_min(FormSet::LABEL),
    }
}

/// The smallest composition of widget `w`, logical px, which the inner grid takes its minimum
/// span from: a metric widget's smallest composition, one item of a group, a fixed widget's
/// minimum.
pub fn widget_smallest(w: &WidgetSetting) -> Size<f32> {
    match &w.content {
        WidgetContent::Metric(_) => smallest(w.forms),
        WidgetContent::Group(g) => group_item_min(g.options.items, w.forms),
        WidgetContent::Fixed(kind) => fixed_min(kind),
    }
}

/// `forms` drawn as `composition` at `area` (window px) and UI scale `scale`, whatever the size:
/// a group's items keep the composition of their kind. A Row still drops parts by its width.
pub fn compose_as(
    composition: Composition,
    forms: FormSet,
    area: Size<Pixels>,
    scale: f32,
) -> Composed {
    let scale = if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    };
    match composition {
        Composition::Row => Composed {
            composition,
            forms: row_forms(forms, f32::from(area.width) / scale),
        },
        _ => Composed { composition, forms },
    }
}

/// The width the parts of a Row with `forms` take, logical px. The name column carries its
/// own trailing space; every other part after the first adds a gap. With a bar the sparkline
/// is a fixed column; without one it takes the bar's place at a bar's minimum width.
pub fn row_parts_w(forms: FormSet) -> f32 {
    let mut parts: [f32; 4] = [0.; 4];
    let mut n = 0;
    let mut push = |w: f32| {
        parts[n] = w;
        n += 1;
    };
    if forms.contains(FormSet::BAR) {
        push(ROW_BAR_MIN_W);
    }
    if forms.contains(FormSet::VALUE) {
        push(ROW_VALUE_W);
    }
    if forms.contains(FormSet::STATS) {
        push(ROW_STATS_W);
    }
    if forms.contains(FormSet::CHART) {
        push(if forms.contains(FormSet::BAR) {
            ROW_SPARK_W
        } else {
            ROW_BAR_MIN_W
        });
    }
    let rest: f32 = parts[..n].iter().sum::<f32>() + ROW_GAP * n.saturating_sub(1) as f32;
    let name = if forms.contains(FormSet::LABEL) {
        ROW_NAME_W
    } else {
        0.
    };
    name + rest
}

/// The forms a Row `width` logical px wide keeps: parts are dropped in [`ROW_DROP_ORDER`] until
/// the rest fit. The last form is never dropped.
pub fn row_forms(forms: FormSet, width: f32) -> FormSet {
    let mut kept = forms;
    for part in ROW_DROP_ORDER {
        if row_parts_w(kept) <= width {
            break;
        }
        let less = kept - part;
        if kept.contains(part) && !less.is_empty() {
            kept = less;
        }
    }
    kept
}

/// The composition of a widget with `forms` at `area` (window px) and UI scale `scale`.
pub fn compose(forms: FormSet, area: Size<Pixels>, scale: f32) -> Composed {
    compose_styled(forms, area, scale, false)
}

/// [`compose`] in the page style (`page`, ADR-019 phase 3) or the box style: a page style Cell
/// needs [`page_cell_min`].
pub fn compose_styled(forms: FormSet, area: Size<Pixels>, scale: f32, page: bool) -> Composed {
    let scale = if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    };
    let w = f32::from(area.width) / scale;
    let h = f32::from(area.height) / scale;
    let fits = |min: Size<f32>| w >= min.width && h >= min.height;
    let cell = if page {
        page_cell_min(forms)
    } else {
        cell_min(forms)
    };
    if forms.contains(FormSet::CHART) && fits(cell) {
        return Composed {
            composition: Composition::Cell,
            forms,
        };
    }
    if !forms.contains(FormSet::CHART) && !forms.is_empty() && fits(tile_min(forms)) {
        return Composed {
            composition: Composition::Tile,
            forms,
        };
    }
    Composed {
        composition: Composition::Row,
        forms: row_forms(forms, w),
    }
}

/// [`min_size`] in window px at UI scale `scale`.
pub fn min_px(composition: Composition, forms: FormSet, scale: f32) -> Size<Pixels> {
    let s = min_size(composition, forms);
    size(px(s.width * scale), px(s.height * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(forms: FormSet, w: f32, h: f32, scale: f32) -> Composed {
        compose(forms, size(px(w), px(h)), scale)
    }

    #[test]
    fn cell_at_its_minimum_and_larger() {
        for forms in [
            FormSet::CHART,
            FormSet::CHART | FormSet::LABEL | FormSet::VALUE,
            FormSet::all(),
        ] {
            let min = cell_min(forms);
            for scale in [1.0, 1.5, 2.0] {
                let (w, h) = (min.width * scale, min.height * scale);
                assert_eq!(at(forms, w, h, scale).composition, Composition::Cell);
                assert_eq!(at(forms, w, h, scale).forms, forms);
                assert_eq!(
                    at(forms, w * 3., h * 4., scale).composition,
                    Composition::Cell
                );
                assert_eq!(
                    at(forms, w - 1., h, scale).composition,
                    Composition::Row,
                    "{forms:?} narrower than a Cell"
                );
                assert_eq!(at(forms, w, h - 1., scale).composition, Composition::Row);
            }
        }
        assert_eq!(cell_min(FormSet::CHART), size(120., 30.));
        assert_eq!(cell_min(FormSet::all()), size(120., 30. + 18. + 16. + 14.));
    }

    #[test]
    fn tile_at_its_minimum_and_larger() {
        let forms = FormSet::LABEL | FormSet::VALUE | FormSet::STATS | FormSet::BAR;
        assert_eq!(tile_min(forms), size(96., 66.));
        assert_eq!(tile_min(FormSet::VALUE), size(96., 40.));
        assert_eq!(at(forms, 96., 66., 1.).composition, Composition::Tile);
        assert_eq!(at(forms, 400., 200., 1.).composition, Composition::Tile);
        assert_eq!(at(forms, 144., 99., 1.5).composition, Composition::Tile);
        assert_eq!(at(forms, 95., 66., 1.).composition, Composition::Row);
        assert_eq!(at(forms, 96., 65., 1.).composition, Composition::Row);
    }

    #[test]
    fn row_is_the_fallback() {
        let forms = FormSet::LABEL | FormSet::VALUE;
        assert_eq!(row_min(forms), size(120., 18.));
        assert_eq!(row_min(FormSet::VALUE), size(48., 18.));
        assert_eq!(smallest(forms), row_min(forms));
        let r = at(forms, 120., 18., 1.);
        assert_eq!(r.composition, Composition::Row);
        assert_eq!(r.forms, forms, "a name and a value fit at the Row minimum");
        let r = at(FormSet::VALUE, 48., 18., 1.);
        assert_eq!(r.forms, FormSet::VALUE);
    }

    #[test]
    fn narrow_row_drop_order() {
        let all = FormSet::all();
        let full = row_parts_w(all);
        assert_eq!(row_forms(all, full), all);
        let no_stats = all - FormSet::STATS;
        assert_eq!(row_forms(all, full - 1.), no_stats);
        let w = row_parts_w(no_stats);
        // Then the bar: the sparkline takes its place.
        let no_bar = no_stats - FormSet::BAR;
        assert_eq!(row_forms(all, w - 1.), no_bar);
        assert!(row_parts_w(no_bar) < w);
        // Then the sparkline, then the name. The value stays.
        let no_spark = no_bar - FormSet::CHART;
        assert_eq!(row_forms(all, row_parts_w(no_bar) - 1.), no_spark);
        assert_eq!(row_forms(all, row_parts_w(no_spark) - 1.), FormSet::VALUE);
        assert_eq!(row_forms(all, 1.), FormSet::VALUE);
        // The last form stays even when it does not fit.
        assert_eq!(
            row_forms(FormSet::LABEL | FormSet::BAR, 10.),
            FormSet::LABEL
        );
        assert_eq!(row_forms(FormSet::BAR, 1.), FormSet::BAR);
    }

    #[test]
    fn group_items_and_widget_minimums() {
        let chart = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        assert_eq!(
            group_item_composition(GroupItems::Rows, chart),
            Composition::Row
        );
        assert_eq!(
            group_item_composition(GroupItems::Cells, chart),
            Composition::Cell
        );
        let tile = FormSet::VALUE | FormSet::LABEL;
        assert_eq!(
            group_item_composition(GroupItems::Cells, tile),
            Composition::Tile
        );
        assert_eq!(group_item_min(GroupItems::Rows, chart), size(120., 18.));
        assert_eq!(group_item_min(GroupItems::Cells, chart), cell_min(chart));
        assert_eq!(group_item_min(GroupItems::Cells, tile), tile_min(tile));
        assert_eq!(fixed_min(&FixedWidget::Processes), size(256., 192.));
        // A forced Row drops parts by its width; a forced Cell keeps its forms at any size.
        let all = FormSet::all();
        let row = compose_as(Composition::Row, all, size(px(130.), px(60.)), 1.);
        assert_eq!(row.composition, Composition::Row);
        assert_eq!(row.forms, row_forms(all, 130.));
        let cell = compose_as(Composition::Cell, all, size(px(10.), px(10.)), 1.);
        assert_eq!(
            cell,
            Composed {
                composition: Composition::Cell,
                forms: all
            }
        );
    }

    #[test]
    fn class_follows_composition_and_forms() {
        let a = at(FormSet::all(), 400., 300., 1.);
        let b = at(FormSet::all(), 401., 300., 1.);
        assert_eq!(a.class(), b.class());
        let row = at(FormSet::all(), 400., 18., 1.);
        assert_ne!(a.class(), row.class());
        let narrow = at(FormSet::all(), 130., 18., 1.);
        assert_ne!(row.class(), narrow.class());
    }

    #[test]
    fn page_style_cell_at_its_minimum() {
        for forms in [
            FormSet::CHART,
            FormSet::CHART | FormSet::LABEL | FormSet::VALUE,
            FormSet::all(),
        ] {
            let min = page_cell_min(forms);
            assert!(min.height > cell_min(forms).height);
            for scale in [1.0, 1.5, 2.0] {
                let (w, h) = (min.width * scale, min.height * scale);
                let at = |w: f32, h: f32| compose_styled(forms, size(px(w), px(h)), scale, true);
                assert_eq!(at(w, h).composition, Composition::Cell);
                assert_ne!(at(w, h - scale).composition, Composition::Cell);
            }
        }
        // 30 + 20 + 18 with a name and a value.
        assert_eq!(
            page_cell_min(FormSet::CHART | FormSet::LABEL | FormSet::VALUE).height,
            68.
        );
    }
}
