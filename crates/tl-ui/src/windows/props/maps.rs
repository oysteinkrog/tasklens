//! The Memory maps tab of Process Properties: a tree of mapped files with their regions as
//! children, the "Shared libraries only" switch, copy, and the Copy path and Open file location
//! menu items (UI-PWN-013, UI-PWN-018, PROC-081, R-098).
//!
//! `PropsWindow` feeds the tab its payload and the exited state. The flattened visible rows are
//! rebuilt only when the payload, the expansion, the sort, the filter or the switch changes.
//! "(Primary)" is always first and "Outside loaded modules" always last; the other nodes follow
//! the sort (Base address ascending by default). Regions are always in address order.
//!
//! Expanded nodes are kept by node path for the window's life, so a new payload keeps them
//! open. The switch is `properties.shared_objects_only`; every open tab observes the settings
//! global. The engine sets `MapNode.shared_object` (with the Wine `.dll` rule), and the switch
//! keeps the "(Primary)" and "Outside loaded modules" nodes visible (an open item, see the bead).
//!
//! Paths are untrusted: the collector sanitizes them and the cells draw them as plain text (E15).

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, px,
};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::details::{MapNode, MapRegion, MapsDetail, TabPayload};
use tl_model::ids::ProcessKey;
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{ClearPropsFilter, FilterChanged, NO_MATCH, PropsFilter, lowercase};
use crate::action_tracker::ToastTexts;
use crate::components::checkbox::TlCheckbox;
use crate::components::menu::TlMenu;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::components::validity::copy_text;
use crate::host::HostContext;
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::table::actions::CopyRow;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};

gpui_kit::actions!(
    tasklens_maps,
    [MapsCopyCell, MapsCopyPath, MapsOpenFileLocation]
);

/// The columns of the Memory maps tree, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MapsColumn {
    Name,
    Base,
    Size,
    Rss,
    Perms,
    Offset,
    Path,
    Description,
    Package,
}

impl MapsColumn {
    pub const ALL: [MapsColumn; 9] = [
        Self::Name,
        Self::Base,
        Self::Size,
        Self::Rss,
        Self::Perms,
        Self::Offset,
        Self::Path,
        Self::Description,
        Self::Package,
    ];

    /// Header key, width at 100% and alignment. The minimum widths are in `min_width`.
    fn meta(self) -> (&'static str, f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Name => ("pwn-maps-col-name", 260., Left),
            Self::Base => ("pwn-maps-col-base", 140., Right),
            Self::Size => ("pwn-maps-col-size", 90., Right),
            Self::Rss => ("pwn-maps-col-rss", 90., Right),
            Self::Perms => ("pwn-maps-col-perms", 64., Left),
            Self::Offset => ("pwn-maps-col-offset", 100., Right),
            Self::Path => ("pwn-maps-col-path", 280., Left),
            Self::Description => ("pwn-maps-col-desc", 200., Left),
            Self::Package => ("pwn-maps-col-pkg", 140., Left),
        }
    }
}

impl TableColumn for MapsColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Base => "base",
            Self::Size => "size",
            Self::Rss => "rss",
            Self::Perms => "perms",
            Self::Offset => "offset",
            Self::Path => "path",
            Self::Description => "desc",
            Self::Package => "pkg",
        }
    }

    /// The layout is never saved, so no stored key maps to it.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }

    /// The bead's minimum widths, logical px at 100% (gma.23).
    fn min_width(self) -> Option<f32> {
        Some(match self {
            Self::Name => 120.,
            Self::Base => 110.,
            Self::Size | Self::Rss => 70.,
            Self::Offset => 80.,
            Self::Perms | Self::Path | Self::Description | Self::Package => 60.,
        })
    }
}

/// The sort of the module nodes, kept for the window's life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapsSort {
    pub column: MapsColumn,
    pub ascending: bool,
}

impl Default for MapsSort {
    /// Base address ascending.
    fn default() -> Self {
        Self {
            column: MapsColumn::Base,
            ascending: true,
        }
    }
}

impl TableSort<MapsColumn> for MapsSort {
    fn column(&self) -> MapsColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The tab's words, translated once per strings version.
pub struct MapsWords {
    strings_version: Option<u64>,
    /// Header labels in [`MapsColumn::ALL`] order.
    pub headers: [SharedString; 9],
    /// `{name} (Primary)`.
    primary: Template,
    pub outside: SharedString,
    pub primary_tip: SharedString,
    pub outside_tip: SharedString,
    pub loading: SharedString,
    pub empty: SharedString,
    pub no_match: SharedString,
    pub shared_only: SharedString,
    pub copy: SharedString,
    pub copy_path: SharedString,
    pub open_location: SharedString,
    pub copied: SharedString,
    pub opening: SharedString,
    pub open_failed: SharedString,
}

impl MapsWords {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            strings_version: strings_version(cx),
            headers: MapsColumn::ALL.map(|c| t(c.meta().0)),
            primary: Template::new(cx, "pwn-maps-primary", &["name"]),
            outside: t("pwn-maps-outside"),
            primary_tip: t("pwn-maps-primary-tooltip"),
            outside_tip: t("pwn-maps-outside-tooltip"),
            loading: t("pwn-maps-loading"),
            empty: t("pwn-maps-empty"),
            no_match: t(NO_MATCH),
            shared_only: t("pwn-maps-shared-only"),
            copy: t("pwn-maps-menu-copy"),
            copy_path: t("pwn-maps-menu-copy-path"),
            open_location: t("pwn-maps-menu-open-location"),
            copied: t("pwn-maps-toast-copied"),
            opening: t("pwn-maps-toast-opening"),
            open_failed: t("pwn-maps-toast-open-failed"),
        }
    }

    /// The Name cell of `node`: `firefox (Primary)`, `Outside loaded modules` or the file name.
    fn node_name(&self, node: &MapNode, out: &mut String) {
        if node.outside {
            out.push_str(&self.outside);
        } else if node.primary {
            let _ = self.primary.write(out, &[&node.name]);
        } else {
            out.push_str(&node.name);
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

/// `properties.shared_objects_only`, `true` without the settings global.
fn shared_setting(cx: &App) -> bool {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).properties.shared_objects_only
    } else {
        true
    }
}

// ---- Rows (pure) ----

/// One flattened row: a module node, or one region under its node. Indexes are into
/// `MapsDetail.nodes` and `MapsDetail.regions`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeRowRef {
    Node(u32),
    Region { node: u32, region: u32 },
}

/// Row identity across payloads: the node key's hash, and the region start or [`NODE_ROW`].
pub type MapsRowId = (u64, u64);

/// The second half of a node row's id.
pub const NODE_ROW: u64 = u64::MAX;

/// The key a node keeps across payloads: its path, else its name; the two special nodes have
/// their own.
fn node_key(node: &MapNode) -> Box<str> {
    if node.outside {
        "\0outside".into()
    } else if node.primary {
        "\0primary".into()
    } else {
        node.path.as_deref().unwrap_or(&node.name).into()
    }
}

fn hash_key(key: &str) -> u64 {
    let mut h = FxHasher::default();
    key.hash(&mut h);
    h.finish()
}

/// `0x7f3a2c000000`: lowercase hex, no padding.
fn fmt_addr(out: &mut String, addr: u64) {
    let _ = write!(out, "0x{addr:x}");
}

/// `[heap] 55d3f1a00000-55d3f1c21000`, or the range alone.
fn fmt_region_name(out: &mut String, r: &MapRegion) {
    if let Some(label) = &r.label {
        out.push_str(label);
        out.push(' ');
    }
    let _ = write!(out, "{:x}-{:x}", r.start, r.end);
}

/// The regions of `node`, clamped to `regions`.
fn node_regions(node: &MapNode, regions: &[MapRegion]) -> std::ops::Range<usize> {
    let end = (node.regions.end as usize).min(regions.len());
    (node.regions.start as usize).min(end)..end
}

/// Orders nodes `a` and `b` by `col`, then by base address.
fn compare(a: &MapNode, b: &MapNode, col: MapsColumn) -> Ordering {
    let by = match col {
        MapsColumn::Name => a.name.cmp(&b.name),
        MapsColumn::Base | MapsColumn::Perms | MapsColumn::Offset => Ordering::Equal,
        MapsColumn::Size => a.size_bytes.cmp(&b.size_bytes),
        MapsColumn::Rss => a.rss_bytes.value.cmp(&b.rss_bytes.value),
        MapsColumn::Path => a.path.cmp(&b.path),
        MapsColumn::Description => a.description.cmp(&b.description),
        MapsColumn::Package => a.package.cmp(&b.package),
    };
    by.then(a.base.cmp(&b.base))
}

/// The node order: "(Primary)" first, "Outside loaded modules" last, the others by `sort`.
pub fn node_order(nodes: &[MapNode], sort: MapsSort) -> Vec<u32> {
    let rank = |n: &MapNode| {
        if n.primary {
            0
        } else if n.outside {
            2
        } else {
            1
        }
    };
    let mut order: Vec<u32> = (0..nodes.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&nodes[a as usize], &nodes[b as usize]);
        rank(a).cmp(&rank(b)).then_with(|| {
            let o = compare(a, b, sort.column);
            if sort.ascending { o } else { o.reverse() }
        })
    });
    order
}

/// Whether the switch keeps `node`. "(Primary)" and "Outside loaded modules" always stay.
pub fn passes_shared(node: &MapNode, shared_only: bool) -> bool {
    !shared_only || node.shared_object || node.primary || node.outside
}

/// The text filter's result: one flag per node and per region.
#[derive(Clone, Debug, Default)]
pub struct FilterHits {
    pub nodes: Vec<bool>,
    pub regions: Vec<bool>,
}

impl FilterHits {
    /// From the matched haystack indexes: nodes first, then regions.
    fn from_indexes(keep: &[u32], nodes: usize, regions: usize) -> Self {
        let mut out = Self {
            nodes: vec![false; nodes],
            regions: vec![false; regions],
        };
        for &i in keep {
            let i = i as usize;
            if i < nodes {
                out.nodes[i] = true;
            } else if let Some(r) = out.regions.get_mut(i - nodes) {
                *r = true;
            }
        }
        out
    }
}

/// The visible rows: nodes in `order` that pass the switch and the filter, each followed by
/// its regions when expanded. A node shows when it or one of its regions matches the filter;
/// under a matching node every region shows, under a node shown for a region only the matching
/// regions.
pub fn flatten(
    detail: &MapsDetail,
    order: &[u32],
    shared_only: bool,
    hits: Option<&FilterHits>,
    expanded: impl Fn(u32) -> bool,
) -> Vec<TreeRowRef> {
    let mut out = Vec::new();
    for &n in order {
        let node = &detail.nodes[n as usize];
        if !passes_shared(node, shared_only) {
            continue;
        }
        let range = node_regions(node, &detail.regions);
        let node_hit = hits.is_none_or(|h| h.nodes.get(n as usize).copied().unwrap_or(false));
        let region_hit =
            |r: usize| hits.is_some_and(|h| h.regions.get(r).copied().unwrap_or(false));
        if !node_hit && !range.clone().any(region_hit) {
            continue;
        }
        out.push(TreeRowRef::Node(n));
        if expanded(n) {
            out.extend(
                range
                    .filter(|&r| node_hit || region_hit(r))
                    .map(|r| TreeRowRef::Region {
                        node: n,
                        region: r as u32,
                    }),
            );
        }
    }
    out
}

// ---- The table ----

/// The Memory maps tree's rows, columns and sort.
pub struct MapsDelegate {
    detail: Arc<MapsDetail>,
    /// Per node: its key and the key's hash.
    keys: Vec<Box<str>>,
    ids: Vec<u64>,
    /// Node indexes in display order.
    order: Vec<u32>,
    hits: Option<FilterHits>,
    shared_only: bool,
    /// Node keys of the expanded nodes, kept for the window's life.
    expanded: FxHashSet<Box<str>>,
    /// The flattened visible rows.
    rows: Arc<[TreeRowRef]>,
    index: FxHashMap<MapsRowId, u32>,
    loading: bool,
    exited: bool,
    /// The row and column the last menu opened on, for its Copy item.
    menu_cell: Option<(MapsRowId, MapsColumn)>,
    version: u64,
    sort: MapsSort,
    specs: Vec<ColumnSpec<MapsColumn>>,
    layout: ColumnLayout<MapsColumn>,
    words: Rc<MapsWords>,
    /// "Main executable" and "Anonymous, heap, stack and other mappings without a file".
    tips: Option<(AnyView, AnyView)>,
}

impl MapsDelegate {
    fn new(words: Rc<MapsWords>, shared_only: bool) -> Self {
        let specs = specs(&words);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &MapsColumn::ALL,
            &MapsColumn::ALL,
        );
        Self {
            detail: Arc::default(),
            keys: Vec::new(),
            ids: Vec::new(),
            order: Vec::new(),
            hits: None,
            shared_only,
            expanded: FxHashSet::default(),
            rows: Arc::from([]),
            index: FxHashMap::default(),
            loading: true,
            exited: false,
            menu_cell: None,
            version: 0,
            sort: MapsSort::default(),
            specs,
            layout,
            words,
            tips: None,
        }
    }

    /// The visible rows, in order.
    pub fn rows(&self) -> &Arc<[TreeRowRef]> {
        &self.rows
    }

    pub fn detail(&self) -> &Arc<MapsDetail> {
        &self.detail
    }

    /// Visible nodes after the switch, before the text filter: the tab count.
    pub fn count(&self) -> usize {
        self.detail
            .nodes
            .iter()
            .filter(|n| passes_shared(n, self.shared_only))
            .count()
    }

    /// The id of node `n`'s row.
    pub fn node_id(&self, n: u32) -> MapsRowId {
        (self.ids.get(n as usize).copied().unwrap_or(0), NODE_ROW)
    }

    /// The id of the visible node whose Name cell reads `name`.
    pub fn node_named(&self, name: &str) -> Option<MapsRowId> {
        let mut text = String::new();
        self.rows.iter().find_map(|r| match *r {
            TreeRowRef::Node(n) => {
                text.clear();
                self.words
                    .node_name(&self.detail.nodes[n as usize], &mut text);
                (text == name).then(|| self.node_id(n))
            }
            TreeRowRef::Region { .. } => None,
        })
    }

    /// The Name cells of the visible rows, in order.
    pub fn row_names(&self) -> Vec<String> {
        (0..self.rows.len())
            .map(|ix| {
                let mut s = String::new();
                self.write_text(ix, MapsColumn::Name, &mut s);
                s
            })
            .collect()
    }

    /// The text of the `col` cell of `id`, `None` when the row is not shown.
    /// It is the copy form: an RSS still warming up reads `…`.
    pub fn cell_text(&self, id: MapsRowId, col: MapsColumn) -> Option<String> {
        let ix = self.row_index(id)?;
        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        self.cell(ix, col, &mut out);
        let text = SharedString::from(std::mem::take(&mut out.text));
        Some(copy_text(out.validity, &text).to_string())
    }

    /// Whether node `id` is expanded.
    pub fn is_expanded(&self, id: MapsRowId) -> bool {
        self.node_of(id)
            .is_some_and(|n| self.expanded.contains(&self.keys[n as usize]))
    }

    fn node_of(&self, id: MapsRowId) -> Option<u32> {
        let ix = self.row_index(id)?;
        match self.rows[ix] {
            TreeRowRef::Node(n) | TreeRowRef::Region { node: n, .. } => Some(n),
        }
    }

    /// The path of node row `id`.
    fn node_path(&self, id: MapsRowId) -> Option<Arc<str>> {
        let ix = self.row_index(id)?;
        match self.rows[ix] {
            TreeRowRef::Node(n) => self.detail.nodes[n as usize].path.clone(),
            TreeRowRef::Region { .. } => None,
        }
    }

    fn set_detail(&mut self, detail: Arc<MapsDetail>) {
        self.keys = detail.nodes.iter().map(node_key).collect();
        self.ids = self.keys.iter().map(|k| hash_key(k)).collect();
        self.detail = detail;
        self.loading = false;
        self.hits = None;
        self.resort();
    }

    fn resort(&mut self) {
        self.order = node_order(&self.detail.nodes, self.sort);
        self.rebuild();
    }

    /// Flattens the rows again and bumps the version.
    fn rebuild(&mut self) {
        let rows = flatten(
            &self.detail,
            &self.order,
            self.shared_only,
            self.hits.as_ref(),
            |n| self.expanded.contains(&self.keys[n as usize]),
        );
        self.rows = rows.into();
        self.index.clear();
        for ix in 0..self.rows.len() {
            let id = self.row_id(ix);
            self.index.insert(id, ix as u32);
        }
        self.version += 1;
    }

    fn node(&self, n: u32) -> &MapNode {
        &self.detail.nodes[n as usize]
    }

    fn region(&self, r: u32) -> &MapRegion {
        &self.detail.regions[r as usize]
    }

    /// The display text of `col` for row `ix`.
    fn write_text(&self, ix: usize, col: MapsColumn, out: &mut String) {
        match self.rows[ix] {
            TreeRowRef::Node(n) => {
                let node = self.node(n);
                match col {
                    MapsColumn::Name => self.words.node_name(node, out),
                    MapsColumn::Base => fmt_addr(out, node.base),
                    MapsColumn::Size => units::bytes(out, node.size_bytes),
                    MapsColumn::Rss => units::bytes(out, node.rss_bytes.value),
                    MapsColumn::Perms | MapsColumn::Offset => {}
                    MapsColumn::Path => out.push_str(node.path.as_deref().unwrap_or("")),
                    MapsColumn::Description => {
                        out.push_str(node.description.as_deref().unwrap_or(""))
                    }
                    MapsColumn::Package => out.push_str(node.package.as_deref().unwrap_or("")),
                }
            }
            TreeRowRef::Region { region, .. } => {
                let r = self.region(region);
                match col {
                    MapsColumn::Name => fmt_region_name(out, r),
                    MapsColumn::Base => fmt_addr(out, r.start),
                    MapsColumn::Size => units::bytes(out, r.end.saturating_sub(r.start)),
                    MapsColumn::Rss => units::bytes(out, r.rss_bytes.value),
                    MapsColumn::Perms => out.push_str(&String::from_utf8_lossy(&r.perms)),
                    MapsColumn::Offset => {
                        let _ = write!(out, "0x{:08x}", r.offset);
                    }
                    MapsColumn::Path | MapsColumn::Description | MapsColumn::Package => {}
                }
            }
        }
    }
}

/// Pointer and length of an optional text, as cell bits.
fn text_bits(text: Option<&Arc<str>>) -> u64 {
    text.map_or(0, |t| (t.as_ptr() as u64) ^ ((t.len() as u64) << 48))
}

fn specs(words: &MapsWords) -> Vec<ColumnSpec<MapsColumn>> {
    MapsColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width, align) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align,
                kind: match (id, align) {
                    (MapsColumn::Path, _) => ColumnKind::Path,
                    (_, ColumnAlign::Left) => ColumnKind::Text,
                    (_, ColumnAlign::Right) => ColumnKind::Numeric,
                },
                sortable: true,
                template: None,
                default_width: px(width),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

impl TreeTableDelegate for MapsDelegate {
    type RowId = MapsRowId;
    type Column = MapsColumn;
    type Sort = MapsSort;

    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_id(&self, ix: usize) -> MapsRowId {
        match self.rows[ix] {
            TreeRowRef::Node(n) => self.node_id(n),
            TreeRowRef::Region { node, region } => {
                (self.node_id(node).0, self.region(region).start)
            }
        }
    }
    fn row_index(&self, id: MapsRowId) -> Option<usize> {
        self.index.get(&id).map(|&p| p as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, ix: usize) -> u8 {
        match self.rows[ix] {
            TreeRowRef::Node(_) => 0,
            TreeRowRef::Region { .. } => 1,
        }
    }
    fn row_expandable(&self, ix: usize) -> Option<bool> {
        match self.rows[ix] {
            TreeRowRef::Node(n) if !node_regions(self.node(n), &self.detail.regions).is_empty() => {
                Some(self.expanded.contains(&self.keys[n as usize]))
            }
            _ => None,
        }
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<MapsColumn> {
        &self.layout
    }
    fn column_spec(&self, id: MapsColumn) -> &ColumnSpec<MapsColumn> {
        let i = MapsColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: MapsColumn, out: &mut CellOut) {
        let (bits, validity) = match self.rows[ix] {
            TreeRowRef::Node(n) => {
                let node = self.node(n);
                match col {
                    MapsColumn::Name => (
                        text_bits(Some(&node.name)) ^ u64::from(node.primary),
                        Validity::Valid,
                    ),
                    MapsColumn::Base => (node.base, Validity::Valid),
                    MapsColumn::Size => (node.size_bytes, Validity::Valid),
                    MapsColumn::Rss => (node.rss_bytes.value, node.rss_bytes.validity),
                    MapsColumn::Perms | MapsColumn::Offset => (0, Validity::Valid),
                    MapsColumn::Path => (text_bits(node.path.as_ref()), Validity::Valid),
                    MapsColumn::Description => {
                        (text_bits(node.description.as_ref()), Validity::Valid)
                    }
                    MapsColumn::Package => (text_bits(node.package.as_ref()), Validity::Valid),
                }
            }
            TreeRowRef::Region { region, .. } => {
                let r = self.region(region);
                match col {
                    MapsColumn::Name | MapsColumn::Base => (r.start, Validity::Valid),
                    MapsColumn::Size => (r.end.saturating_sub(r.start), Validity::Valid),
                    MapsColumn::Rss => (r.rss_bytes.value, r.rss_bytes.validity),
                    MapsColumn::Perms => (u64::from(u32::from_le_bytes(r.perms)), Validity::Valid),
                    MapsColumn::Offset => (r.offset, Validity::Valid),
                    _ => (0, Validity::Valid),
                }
            }
        };
        out.bits = bits;
        out.validity = validity;
        if out.want_text {
            self.write_text(ix, col, &mut out.text);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let mut text = String::new();
        self.write_text(ix, MapsColumn::Name, &mut text);
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: text.into(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<MapsColumn> {
        Some(MapsColumn::Name)
    }

    fn header_total(&self, _: MapsColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> MapsSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    /// Copy (the cell), Copy path and Open file location (node rows with a path). Everything is
    /// disabled once the process exited.
    fn context_menu(
        &mut self,
        targets: &[MapsRowId],
        col: Option<MapsColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let w = self.words.clone();
        self.menu_cell = targets
            .first()
            .map(|&id| (id, col.unwrap_or(MapsColumn::Name)));
        if self.exited {
            return menu
                .item_disabled(w.copy.clone(), None)
                .item_disabled(w.copy_path.clone(), None)
                .item_disabled(w.open_location.clone(), None);
        }
        let menu = menu.item(w.copy.clone(), Box::new(MapsCopyCell));
        let path = match targets {
            [id] => self.node_path(*id),
            _ => None,
        };
        if path.is_some() {
            menu.item(w.copy_path.clone(), Box::new(MapsCopyPath))
                .item(w.open_location.clone(), Box::new(MapsOpenFileLocation))
        } else {
            menu.item_disabled(w.copy_path.clone(), None)
                .item_disabled(w.open_location.clone(), None)
        }
    }

    fn on_toggle_expand(&mut self, id: MapsRowId, cx: &mut Context<TlTreeTable<Self>>) {
        let Some(n) = self.node_of(id).filter(|_| id.1 == NODE_ROW) else {
            return;
        };
        let key = self.keys[n as usize].clone();
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        self.rebuild();
        cx.notify();
    }

    /// A new column sorts descending first; the same column flips.
    fn on_sort(&mut self, col: MapsColumn, cx: &mut Context<TlTreeTable<Self>>) {
        self.sort = MapsSort {
            column: col,
            ascending: self.sort.column == col && !self.sort.ascending,
        };
        self.resort();
        cx.notify();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<MapsColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[MapsRowId], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: MapsRowId, _: &mut Context<TlTreeTable<Self>>) {}

    /// "(Primary)" and "Outside loaded modules" explain themselves.
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        let (primary, outside) = self.tips.as_ref()?;
        match self.rows[ix] {
            TreeRowRef::Node(n) if self.node(n).primary => Some(primary.clone()),
            TreeRowRef::Node(n) if self.node(n).outside => Some(outside.clone()),
            _ => None,
        }
    }

    fn empty_text(&self) -> Option<SharedString> {
        let w = &self.words;
        Some(if self.loading {
            w.loading.clone()
        } else if self.count() == 0 {
            w.empty.clone()
        } else {
            w.no_match.clone()
        })
    }

    /// The no-match text comes with "Clear filter" (UI-PWN-005).
    fn empty_clear_filter(&self) -> bool {
        !self.loading && self.count() != 0
    }

    fn empty_clear_action(&self) -> Box<dyn gpui_kit::Action> {
        Box::new(ClearPropsFilter)
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- The tab ----

/// The tab count changed: the visible nodes after the switch.
pub struct MapsCountChanged(pub u32);

/// The Memory maps tab view. `PropsWindow` builds it with the process key and the tab's filter.
pub struct MapsTab {
    /// The host this tab shows. Set when it opens; never changes.
    host: HostContext,
    key: ProcessKey,
    table: Entity<TlTreeTable<MapsDelegate>>,
    filter: Entity<PropsFilter>,
    /// Lowercase haystacks: per node `name path`, then per region its Name cell.
    haystacks: Arc<[Box<str>]>,
    last_payload: Option<Arc<TabPayload>>,
    exited: bool,
    words: Rc<MapsWords>,
    _subs: SmallVec<[Subscription; 4]>,
}

impl EventEmitter<MapsCountChanged> for MapsTab {}

fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
    }
}

fn tip_views(words: &MapsWords, cx: &mut App) -> (AnyView, AnyView) {
    (
        TlTooltip::text(words.primary_tip.clone()).view(cx),
        TlTooltip::text(words.outside_tip.clone()).view(cx),
    )
}

impl MapsTab {
    pub fn new(
        host: &HostContext,
        key: ProcessKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(MapsWords::new(cx));
        let mut delegate = MapsDelegate::new(words.clone(), shared_setting(cx));
        delegate.tips = Some(tip_views(&words, cx));
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&filter, |this, _, ev: &FilterChanged, cx| {
            this.run_filter(ev.generation, cx);
        }));
        subs.push(cx.subscribe_in(
            &table,
            window,
            |this, _, ev: &TreeTableEvent, window, cx| {
                if matches!(ev, TreeTableEvent::CopiedRows { .. }) {
                    toast(window, cx, ToastKind::Success, this.words.copied.clone());
                }
            },
        ));
        if cx.has_global::<TlSettings>() {
            subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
                let on = shared_setting(cx);
                this.set_shared_only(on, cx);
            }));
        }
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            if this.words.strings_version != strings_version(cx) {
                this.words = Rc::new(MapsWords::new(cx));
                let words = this.words.clone();
                let tips = tip_views(&words, cx);
                this.table.update(cx, |t, cx| {
                    let d = t.delegate_mut();
                    d.specs = specs(&words);
                    d.words = words;
                    d.tips = Some(tips);
                    d.version += 1;
                    t.on_new_version(cx);
                });
            }
        }));
        Self {
            host: host.clone(),
            key,
            table,
            filter,
            haystacks: Arc::from([]),
            last_payload: None,
            exited: false,
            words,
            _subs: subs,
        }
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn table(&self) -> &Entity<TlTreeTable<MapsDelegate>> {
        &self.table
    }

    pub fn words(&self) -> &Rc<MapsWords> {
        &self.words
    }

    /// The tab count: nodes after the switch.
    pub fn count(&self, cx: &App) -> u32 {
        self.table.read(cx).delegate.count() as u32
    }

    /// Takes the tab's payload. The same `Arc` again does nothing; while exited the rows stay
    /// frozen. Expanded nodes stay expanded.
    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let same = match (&self.last_payload, payload) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.last_payload = payload.cloned();
        let Some(TabPayload::Maps(detail)) = payload.map(|p| &**p) else {
            return;
        };
        let detail = detail.clone();
        let mut hay = String::new();
        let nodes = detail.nodes.iter().map(|n| {
            hay.clear();
            self.words.node_name(n, &mut hay);
            hay.push(' ');
            hay.push_str(n.path.as_deref().unwrap_or(""));
            lowercase(&hay).into_boxed_str()
        });
        let mut haystacks: Vec<Box<str>> = nodes.collect();
        haystacks.extend(detail.regions.iter().map(|r| {
            let mut s = String::new();
            fmt_region_name(&mut s, r);
            lowercase(&s).into_boxed_str()
        }));
        self.haystacks = haystacks.into();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_detail(detail);
            t.on_new_version(cx);
        });
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        }
        let n = self.count(cx);
        cx.emit(MapsCountChanged(n));
    }

    /// "Shared libraries only" changed (here or in another window).
    pub fn set_shared_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate.shared_only == on {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.shared_only = on;
            d.rebuild();
            t.on_new_version(cx);
        });
        let n = self.count(cx);
        cx.emit(MapsCountChanged(n));
        cx.notify();
    }

    /// Freezes the rows and disables the menu once the process exited.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            self.table
                .update(cx, |t, _| t.delegate_mut().exited = exited);
            cx.notify();
        }
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Expands or collapses node `id`, as Space would.
    pub fn toggle(&mut self, id: MapsRowId, cx: &mut Context<Self>) {
        self.table
            .update(cx, |t, cx| t.delegate_mut().on_toggle_expand(id, cx));
    }

    /// The checkbox for the filter bar's right end.
    pub fn filter_tool(&self, cx: &App) -> AnyElement {
        TlCheckbox::new("MapsSharedOnly")
            .checked(Some(shared_setting(cx)))
            .label(self.words.shared_only.clone())
            .on_change(|on, _, cx| {
                if cx.has_global::<TlSettings>() {
                    TlSettings::set(
                        cx,
                        SettingKey::PropertiesSharedObjectsOnly,
                        SettingValue::Bool(on),
                    );
                }
            })
            .into_any_element()
    }

    fn run_filter(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.filter.read(cx).is_current(generation) {
            return;
        }
        if self.filter.read(cx).needle().is_empty() {
            self.apply_filter(generation, None, cx);
            return;
        }
        let rows = self.haystacks.clone();
        let task = self.filter.update(cx, |f, cx| f.run_filter(rows, cx));
        cx.spawn(async move |this, cx| {
            if let Some((generation, keep)) = task.await {
                let _ = this.update(cx, |this, cx| this.apply_filter(generation, Some(keep), cx));
            }
        })
        .detach();
    }

    fn apply_filter(&mut self, generation: u64, keep: Option<Arc<[u32]>>, cx: &mut Context<Self>) {
        if !self.filter.update(cx, |f, _| f.accept(generation)) {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.hits = keep.map(|keep| {
                FilterHits::from_indexes(&keep, d.detail.nodes.len(), d.detail.regions.len())
            });
            d.rebuild();
            t.on_new_version(cx);
        });
    }

    fn selected(&self, cx: &App) -> Vec<MapsRowId> {
        self.table.read(cx).selection.ids().collect()
    }

    /// Ctrl+C: the selected rows' cells as TSV lines.
    fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected(cx);
        let d = &self.table.read(cx).delegate;
        let mut ixs: Vec<usize> = ids.iter().filter_map(|id| d.row_index(*id)).collect();
        if ixs.is_empty() {
            return;
        }
        ixs.sort_unstable();
        let mut out = String::new();
        for ix in ixs {
            if !out.is_empty() {
                out.push('\n');
            }
            for (i, &col) in MapsColumn::ALL.iter().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                d.write_text(ix, col, &mut out);
            }
        }
        cx.write_to_clipboard(ClipboardItem::new_string(out));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Menu "Copy": the text of the cell the menu opened on.
    fn on_copy_cell(&mut self, _: &MapsCopyCell, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let d = &self.table.read(cx).delegate;
            d.menu_cell.and_then(|(id, col)| d.cell_text(id, col))
        };
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// The path of the node row the menu opened on.
    fn menu_path(&self, cx: &App) -> Option<Arc<str>> {
        let d = &self.table.read(cx).delegate;
        d.menu_cell.and_then(|(id, _)| d.node_path(id))
    }

    /// Menu "Copy path".
    fn on_copy_path(&mut self, _: &MapsCopyPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.menu_path(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Open file location (PROC-073) for the node row the menu opened on.
    fn on_open_location(
        &mut self,
        _: &MapsOpenFileLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let Some(path) = self.menu_path(cx) else {
            return;
        };
        let Some(tracker) = self.host.tracker().cloned() else {
            warn!(target: "tasklens::ui", "no action tracker, file location not opened");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            return;
        };
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager {
                path,
                properties: false,
            },
            targets: SmallVec::new(),
            privileged: false,
        };
        let texts = ToastTexts::new(SharedString::default(), self.words.open_failed.clone());
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if sent.is_some() {
            toast(window, cx, ToastKind::Info, self.words.opening.clone());
        }
    }
}

impl Render for MapsTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "PropsMaps".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_row))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_path))
            .on_action(cx.listener(Self::on_open_location))
            .child(self.table.clone())
    }
}

#[cfg(test)]
mod tests {
    use tl_model::table::Total;

    use super::*;

    fn node(name: &str, base: u64, regions: std::ops::Range<u32>) -> MapNode {
        MapNode {
            name: name.into(),
            path: Some(format!("/usr/lib/{name}").into()),
            primary: false,
            outside: false,
            shared_object: name.contains(".so"),
            base,
            size_bytes: 4096,
            rss_bytes: Total {
                value: 0,
                validity: Validity::Valid,
            },
            description: None,
            package: None,
            regions,
        }
    }

    fn region(start: u64, label: Option<&str>) -> MapRegion {
        MapRegion {
            start,
            end: start + 0x1000,
            perms: *b"r-xp",
            offset: 0x1a000,
            rss_bytes: Total {
                value: 0,
                validity: Validity::Valid,
            },
            label: label.map(Into::into),
        }
    }

    fn detail() -> MapsDetail {
        let mut primary = node("firefox", 0x5000, 0..1);
        primary.primary = true;
        let mut outside = node("", 0x1000, 2..4);
        outside.outside = true;
        outside.path = None;
        MapsDetail {
            nodes: Arc::from(vec![
                outside,
                node("libc.so.6", 0x7000, 1..2),
                node("data.bin", 0x6000, 4..4),
                primary,
            ]),
            regions: Arc::from(vec![
                region(0x5000, None),
                region(0x7000, None),
                region(0x1000, Some("[heap]")),
                region(0x2000, Some("memfd:wayland-shm")),
            ]),
        }
    }

    #[test]
    fn order_pins_primary_and_outside() {
        let d = detail();
        assert_eq!(node_order(&d.nodes, MapsSort::default()), [3, 2, 1, 0]);
        let desc = MapsSort {
            column: MapsColumn::Base,
            ascending: false,
        };
        assert_eq!(node_order(&d.nodes, desc), [3, 1, 2, 0]);
    }

    #[test]
    fn flatten_shared_filter_and_children() {
        let d = detail();
        let order = node_order(&d.nodes, MapsSort::default());
        let rows = flatten(&d, &order, true, None, |_| false);
        assert_eq!(
            rows,
            [
                TreeRowRef::Node(3),
                TreeRowRef::Node(1),
                TreeRowRef::Node(0)
            ],
            "data.bin is not a shared object"
        );
        // A region match keeps its node; only matching regions show under it.
        let hits = FilterHits::from_indexes(&[4 + 3], 4, 4);
        let rows = flatten(&d, &order, true, Some(&hits), |_| true);
        assert_eq!(
            rows,
            [
                TreeRowRef::Node(0),
                TreeRowRef::Region { node: 0, region: 3 }
            ]
        );
        let mut s = String::new();
        fmt_region_name(&mut s, &d.regions[2]);
        assert_eq!(s, "[heap] 1000-2000");
        s.clear();
        fmt_addr(&mut s, 0x7f3a2c000000);
        assert_eq!(s, "0x7f3a2c000000");
    }
}
