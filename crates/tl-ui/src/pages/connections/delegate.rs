//! The Connections table delegate (UI-SYS-061 and UI-SYS-062).
//!
//! Addresses are shared directly from the engine snapshot. Other values use the tree table's
//! `CellTextCache`, keyed by socket cookie and column, so an unchanged row formats nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Window, div, px,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{Family, Proto};
use tl_model::process::ProcessRecord;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnRecord, ConnState, ConnectionsSnapshot, ProcessSnapshot,
    Snapshot,
};
use tl_model::table::{ConnColumn, SysColumnId, SysNodeId, SysRow, SysSortSpec};
use tl_model::units;
use tl_model::validity::Validity;

use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_ROW_NAME, TlTooltip};
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::pages::sys_status::{ConnStateClass, LabelTone, conn_state_tone};
use crate::pages::sys_table::SysDelegate;
use crate::table::cells::{CellOut, CellSuffix};
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};
use crate::theme::{TlTheme, current};

/// The P2 columns in display order. Remote host and Namespace are added by their P4 beads.
pub const CONNECTION_COLUMNS: [ConnColumn; 15] = [
    ConnColumn::Proto,
    ConnColumn::Local,
    ConnColumn::Foreign,
    ConnColumn::State,
    ConnColumn::Pid,
    ConnColumn::Process,
    ConnColumn::User,
    ConnColumn::Family,
    ConnColumn::LocalPort,
    ConnColumn::ForeignPort,
    ConnColumn::RecvQ,
    ConnColumn::SendQ,
    ConnColumn::Interface,
    ConnColumn::Inode,
    ConnColumn::Service,
];

/// The six columns visible in the default layout.
pub const DEFAULT_COLUMNS: [ConnColumn; 6] = [
    ConnColumn::Proto,
    ConnColumn::Local,
    ConnColumn::Foreign,
    ConnColumn::State,
    ConnColumn::Pid,
    ConnColumn::Process,
];

pub const fn id(column: ConnColumn) -> SysColumnId {
    SysColumnId::Connections(column)
}

pub const DEFAULT_SORT: SysSortSpec = SysSortSpec {
    column: id(ConnColumn::Proto),
    ascending: true,
};

pub const fn default_width(column: ConnColumn) -> f32 {
    match column {
        ConnColumn::Proto => 90.,
        ConnColumn::Local | ConnColumn::Foreign => 260.,
        ConnColumn::State | ConnColumn::User => 120.,
        ConnColumn::Pid | ConnColumn::RecvQ | ConnColumn::SendQ => 80.,
        ConnColumn::Process => 180.,
        ConnColumn::Family => 70.,
        ConnColumn::LocalPort | ConnColumn::ForeignPort => 90.,
        ConnColumn::Interface | ConnColumn::Inode => 100.,
        ConnColumn::Service => 200.,
        ConnColumn::RemoteHost => 240.,
        ConnColumn::Netns => 160.,
    }
}

pub const fn min_width(column: ConnColumn) -> f32 {
    match column {
        ConnColumn::Local | ConnColumn::Foreign => 120.,
        ConnColumn::State => 90.,
        _ => 60.,
    }
}

const COLUMN_KEYS: [&str; 15] = [
    "connections-column-proto",
    "connections-column-local",
    "connections-column-foreign",
    "connections-column-state",
    "connections-column-pid",
    "connections-column-process",
    "connections-column-user",
    "connections-column-family",
    "connections-column-local-port",
    "connections-column-foreign-port",
    "connections-column-recv-q",
    "connections-column-send-q",
    "connections-column-interface",
    "connections-column-inode",
    "connections-column-service",
];

const STATE_KEYS: [&str; 14] = [
    "connections-state-listening",
    "connections-state-syn-sent",
    "connections-state-syn-received",
    "connections-state-established",
    "connections-state-fin-wait-1",
    "connections-state-fin-wait-2",
    "connections-state-close-wait",
    "connections-state-closing",
    "connections-state-last-ack",
    "connections-state-time-wait",
    "connections-state-closed",
    "connections-state-unknown",
    "connections-state-unconnected",
    "connections-state-connected",
];

fn state_index(state: ConnState) -> usize {
    state as usize
}

/// Translated words used by connection rows.
#[derive(Clone, Debug)]
pub struct ConnectionWords {
    version: Option<u64>,
    pub columns: [SharedString; 15],
    pub states: [Arc<str>; 14],
    pub no_owner: Arc<str>,
    pub restricted_glyph_tip: SharedString,
    restricted_owner_tip: SharedString,
    owner_line: SharedString,
    owner_more: SharedString,
}

const OWNER_SENTINEL: &str = "\u{fdd0}";
const PID_SENTINEL: &str = "\u{fdd1}";
const NAME_SENTINEL: &str = "\u{fdd2}";
const COUNT_SENTINEL: &str = "\u{fdd3}";

impl ConnectionWords {
    pub fn new(cx: &App) -> Self {
        match cx.try_global::<Strings>() {
            Some(strings) => Self::build(
                Some(strings.strings_version),
                |key| tr(cx, key),
                || {
                    tr_args(
                        cx,
                        "connections-owner-restricted-tip",
                        &[("user", OWNER_SENTINEL.into())],
                    )
                },
                || {
                    tr_args(
                        cx,
                        "connections-owner-line",
                        &[("pid", PID_SENTINEL.into()), ("name", NAME_SENTINEL.into())],
                    )
                },
                || {
                    tr_args(
                        cx,
                        "connections-owner-more",
                        &[("count", COUNT_SENTINEL.into())],
                    )
                },
            ),
            None => Self::en(),
        }
    }

    pub fn en() -> Self {
        Self::build(
            None,
            |key| tr_en(key, &[]),
            || {
                tr_en(
                    "connections-owner-restricted-tip",
                    &[("user", OWNER_SENTINEL.into())],
                )
            },
            || {
                tr_en(
                    "connections-owner-line",
                    &[("pid", PID_SENTINEL.into()), ("name", NAME_SENTINEL.into())],
                )
            },
            || {
                tr_en(
                    "connections-owner-more",
                    &[("count", COUNT_SENTINEL.into())],
                )
            },
        )
    }

    fn build(
        version: Option<u64>,
        text: impl Fn(&str) -> SharedString,
        restricted_owner_tip: impl Fn() -> SharedString,
        owner_line: impl Fn() -> SharedString,
        owner_more: impl Fn() -> SharedString,
    ) -> Self {
        Self {
            version,
            columns: COLUMN_KEYS.map(&text),
            states: STATE_KEYS.map(|key| Arc::from(&*text(key))),
            no_owner: Arc::from(&*text("connections-owner-kernel")),
            restricted_glyph_tip: text("connections-owner-restricted-glyph-tip"),
            restricted_owner_tip: restricted_owner_tip(),
            owner_line: owner_line(),
            owner_more: owner_more(),
        }
    }

    pub fn state(&self, state: ConnState) -> &Arc<str> {
        &self.states[state_index(state)]
    }

    fn restricted_owner_tip(&self, user: &str) -> SharedString {
        self.restricted_owner_tip
            .replace(OWNER_SENTINEL, user)
            .into()
    }

    fn owner_line(&self, pid: u32, name: &str) -> SharedString {
        self.owner_line
            .replace(PID_SENTINEL, &pid.to_string())
            .replace(NAME_SENTINEL, name)
            .into()
    }

    fn owner_more(&self, count: usize) -> SharedString {
        self.owner_more
            .replace(COUNT_SENTINEL, &count.to_string())
            .into()
    }
}

fn template(column: ConnColumn) -> Option<&'static str> {
    match column {
        ConnColumn::Pid => Some("999999 +99"),
        ConnColumn::LocalPort | ConnColumn::ForeignPort => Some("99999"),
        ConnColumn::RecvQ | ConnColumn::SendQ => Some("999999"),
        ConnColumn::Inode => Some("9999999999"),
        _ => None,
    }
}

fn numeric(column: ConnColumn) -> bool {
    matches!(
        column,
        ConnColumn::Pid
            | ConnColumn::LocalPort
            | ConnColumn::ForeignPort
            | ConnColumn::RecvQ
            | ConnColumn::SendQ
            | ConnColumn::Inode
    )
}

pub fn connection_specs(words: &ConnectionWords) -> Vec<ColumnSpec<SysColumnId>> {
    CONNECTION_COLUMNS
        .iter()
        .zip(&words.columns)
        .map(|(&column, label)| ColumnSpec {
            id: id(column),
            label: label.clone(),
            full_name: label.clone(),
            group: label.clone(),
            align: if numeric(column) {
                ColumnAlign::Right
            } else {
                ColumnAlign::Left
            },
            kind: if numeric(column) {
                ColumnKind::Numeric
            } else {
                ColumnKind::Text
            },
            sortable: true,
            template: template(column),
            default_width: px(default_width(column)),
            tooltip: None,
            availability: Availability::Available,
            hideable: true,
        })
        .collect()
}

pub const fn state_class(state: ConnState) -> ConnStateClass {
    match state {
        ConnState::Listening => ConnStateClass::Listening,
        ConnState::Established => ConnStateClass::Established,
        ConnState::SynSent
        | ConnState::SynReceived
        | ConnState::FinWait1
        | ConnState::FinWait2
        | ConnState::CloseWait
        | ConnState::Closing
        | ConnState::LastAck => ConnStateClass::Transitional,
        ConnState::TimeWait | ConnState::Closed => ConnStateClass::TimeWaitOrClosed,
        ConnState::Unknown => ConnStateClass::Unknown,
        ConnState::Unconnected | ConnState::Connected => ConnStateClass::Udp,
    }
}

#[derive(Default)]
struct TipState {
    key: Option<(u64, u64)>,
    lines: Vec<SharedString>,
}

struct OwnerTip {
    state: Rc<RefCell<TipState>>,
    built: Option<((u64, u64), Entity<TlTooltip>)>,
}

impl Render for OwnerTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.borrow();
        if let Some(key) = state.key
            && self.built.as_ref().is_none_or(|built| built.0 != key)
        {
            self.built = Some((key, cx.new(|_| TlTooltip::lines(state.lines.clone()))));
        }
        div().children(self.built.as_ref().map(|(_, view)| view.clone()))
    }
}

/// The Connections `TreeTableDelegate` and `SysDelegate`.
pub struct ConnectionsDelegate {
    conns: Option<Arc<ConnectionsSnapshot>>,
    procs: Option<Arc<ProcessSnapshot>>,
    rows: Arc<[SysRow]>,
    index: FxHashMap<u64, usize>,
    query_generation: u64,
    header: ComponentHeader,
    specs: Vec<ColumnSpec<SysColumnId>>,
    layout: ColumnLayout<SysColumnId>,
    sort: SysSortSpec,
    sort_request: Option<SysSortSpec>,
    words: ConnectionWords,
    theme: Arc<TlTheme>,
    version: u64,
    tip: Rc<RefCell<TipState>>,
    tip_view: Option<AnyView>,
}

impl ConnectionsDelegate {
    pub fn new(cx: &App) -> Self {
        let words = ConnectionWords::new(cx);
        let specs = connection_specs(&words);
        let order = CONNECTION_COLUMNS.map(id);
        let visible = DEFAULT_COLUMNS.map(id);
        Self {
            conns: None,
            procs: None,
            rows: Arc::from([]),
            index: FxHashMap::default(),
            query_generation: 0,
            header: ComponentHeader::default(),
            layout: ColumnLayout::defaults(TableId::Connections, &specs, &order, &visible),
            specs,
            sort: DEFAULT_SORT,
            sort_request: None,
            words,
            theme: current(cx).clone(),
            version: 0,
            tip: Rc::default(),
            tip_view: None,
        }
    }

    pub fn record(&self, ix: usize) -> Option<&ConnRecord> {
        let record = self.rows.get(ix)?.record? as usize;
        self.conns.as_ref()?.records.get(record)
    }

    fn owner(&self, record: &ConnRecord) -> Option<&ProcessRecord> {
        record
            .owners
            .iter()
            .filter_map(|key| self.procs.as_ref()?.get(*key))
            .min_by_key(|process| process.key.pid)
    }

    fn process(&self, key: tl_model::ids::ProcessKey) -> Option<&ProcessRecord> {
        self.procs.as_ref()?.get(key)
    }

    fn user(&self, record: &ConnRecord) -> Option<&Arc<str>> {
        self.owner(record).map(|p| &p.meta.user).or_else(|| {
            self.procs
                .as_ref()?
                .records
                .iter()
                .find(|p| p.uid == record.uid)
                .map(|p| &p.meta.user)
        })
    }

    fn kernel_owned(record: &ConnRecord) -> bool {
        record.inode == 0 && record.owners.is_empty()
    }

    pub fn owner_tooltip_lines(&self, ix: usize) -> Option<Vec<SharedString>> {
        let record = self.record(ix)?;
        if record.owner_validity == Validity::Restricted {
            let user = self
                .user(record)
                .map_or_else(|| record.uid.to_string(), ToString::to_string);
            return Some(vec![self.words.restricted_owner_tip(&user)]);
        }
        if record.owners.len() <= 1 {
            return None;
        }
        let mut owners: Vec<_> = record.owners.iter().copied().collect();
        owners.sort_unstable_by_key(|key| key.pid);
        let shown = owners.len().min(16);
        let mut lines = Vec::with_capacity(shown + usize::from(owners.len() > shown));
        for key in &owners[..shown] {
            let name = self
                .process(*key)
                .map_or_else(|| "?".to_owned(), |p| p.meta.name.to_string());
            lines.push(self.words.owner_line(key.pid, &name));
        }
        if owners.len() > shown {
            lines.push(self.words.owner_more(owners.len() - shown));
        }
        Some(lines)
    }

    pub fn accessible_name(&self, ix: usize) -> Option<String> {
        let record = self.record(ix)?;
        let process = if record.owner_validity == Validity::Restricted {
            "Restricted"
        } else if Self::kernel_owned(record) {
            &self.words.no_owner
        } else {
            self.owner(record).map_or("?", |p| &p.meta.name)
        };
        Some(format!(
            "{} {} to {}, {}, {}",
            proto(record.proto),
            record.local,
            record.foreign,
            self.words.state(record.state),
            process
        ))
    }

    pub fn set_sort(&mut self, sort: SysSortSpec) {
        self.sort = sort;
    }

    pub fn take_sort_request(&mut self) -> Option<SysSortSpec> {
        self.sort_request.take()
    }

    fn stale(&self) -> bool {
        self.header.validity == Validity::Stale
    }

    fn refresh_look(&mut self, cx: &App) -> bool {
        let mut changed = false;
        let strings_version = cx.try_global::<Strings>().map(|s| s.strings_version);
        if strings_version != self.words.version {
            self.words = ConnectionWords::new(cx);
            self.specs = connection_specs(&self.words);
            changed = true;
        }
        let theme = current(cx);
        if !Arc::ptr_eq(theme, &self.theme) {
            self.theme = theme.clone();
            changed = true;
        }
        if changed {
            self.version += 1;
        }
        changed
    }
}

fn proto(value: Proto) -> &'static str {
    match value {
        Proto::Tcp => "TCP",
        Proto::Udp => "UDP",
    }
}

fn family(value: Family) -> &'static str {
    match value {
        Family::Inet => "IPv4",
        Family::Inet6 => "IPv6",
    }
}

fn write_u64(out: &mut CellOut, value: u64, grouped: bool) {
    out.bits = value;
    if out.want_text {
        if grouped {
            units::count(&mut out.text, value);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out.text, "{value}");
        }
    }
}

impl TreeTableDelegate for ConnectionsDelegate {
    type RowId = u64;
    type Column = SysColumnId;
    type Sort = SysSortSpec;

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn row_id(&self, ix: usize) -> u64 {
        match self.rows[ix].node {
            SysNodeId::Conn(cookie) => cookie,
            _ => 0,
        }
    }

    fn row_index(&self, id: u64) -> Option<usize> {
        self.index.get(&id).copied()
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

    fn row_flags(&self, ix: usize) -> RowFlags {
        let mut bits = if self.stale() { RowFlags::STALE } else { 0 };
        if self
            .record(ix)
            .is_some_and(|record| Self::kernel_owned(record) || self.owner(record).is_none())
        {
            bits |= RowFlags::SECONDARY_NAME;
        }
        RowFlags {
            bits,
            ..RowFlags::default()
        }
    }

    fn table_version(&self) -> u64 {
        self.version
    }

    fn columns(&self) -> &ColumnLayout<SysColumnId> {
        &self.layout
    }

    fn column_spec(&self, col: SysColumnId) -> &ColumnSpec<SysColumnId> {
        self.specs
            .iter()
            .find(|spec| spec.id == col)
            .unwrap_or(&self.specs[0])
    }

    fn cell(&self, ix: usize, col: SysColumnId, out: &mut CellOut) {
        let (Some(record), SysColumnId::Connections(column)) = (self.record(ix), col) else {
            return;
        };
        match column {
            ConnColumn::Proto => out.arc = Some(Arc::from(proto(record.proto))),
            ConnColumn::Local => out.arc = Some(record.local.clone()),
            ConnColumn::Foreign => out.arc = Some(record.foreign.clone()),
            ConnColumn::State => {
                out.arc = Some(self.words.state(record.state).clone());
                out.text_color = match conn_state_tone(state_class(record.state)) {
                    LabelTone::Info => Some(self.theme.status.info),
                    LabelTone::Warning => Some(self.theme.status.warning),
                    LabelTone::Secondary => Some(self.theme.text_secondary),
                    _ => None,
                };
            }
            ConnColumn::Pid => {
                if record.owner_validity == Validity::Restricted {
                    out.arc = Some(Arc::from("?"));
                    out.text_color = Some(self.theme.text_secondary);
                } else if Self::kernel_owned(record) {
                    out.arc = Some(Arc::from("-"));
                    out.text_color = Some(self.theme.text_secondary);
                } else if let Some(owner) = self.owner(record) {
                    let extra = record.owners.len().saturating_sub(1);
                    out.bits = (u64::from(owner.key.pid) << 16) | extra as u64;
                    if out.want_text {
                        use std::fmt::Write as _;
                        let _ = write!(out.text, "{}", owner.key.pid);
                        if extra > 0 {
                            let _ = write!(out.text, " +{extra}");
                        }
                    }
                    if extra > 0 {
                        let suffix_len = 2 + extra.to_string().len();
                        out.suffix = Some(CellSuffix {
                            len: suffix_len,
                            color: self.theme.text_secondary,
                            selected_color: self.theme.text_selected_secondary,
                        });
                    }
                } else {
                    out.arc = Some(Arc::from("?"));
                    out.text_color = Some(self.theme.text_secondary);
                }
                if out.want_tooltip {
                    out.tooltip = self
                        .owner_tooltip_lines(ix)
                        .map(|lines| lines.join("\n").into());
                }
            }
            ConnColumn::Process => {
                if out.want_tooltip {
                    out.tooltip = self
                        .owner_tooltip_lines(ix)
                        .map(|lines| lines.join("\n").into());
                }
            }
            ConnColumn::User => out.arc = self.user(record).cloned(),
            ConnColumn::Family => out.arc = Some(Arc::from(family(record.family))),
            ConnColumn::LocalPort => {
                write_u64(out, u64::from(record.four_tuple.local.port()), false)
            }
            ConnColumn::ForeignPort => {
                if !record.foreign.ends_with(":*") {
                    write_u64(out, u64::from(record.four_tuple.remote.port()), false);
                }
            }
            ConnColumn::RecvQ => write_u64(out, u64::from(record.recv_q), true),
            ConnColumn::SendQ => write_u64(out, u64::from(record.send_q), true),
            ConnColumn::Interface => out.arc = record.interface.clone(),
            ConnColumn::Inode => write_u64(out, record.inode, false),
            ConnColumn::Service => out.arc = record.unit.as_ref().map(|unit| unit.name.clone()),
            ConnColumn::RemoteHost | ConnColumn::Netns => {}
        }
        if self.stale() {
            out.text_color = Some(self.theme.text_secondary);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let Some(record) = self.record(ix) else {
            return empty_name_cell();
        };
        if record.owner_validity == Validity::Restricted {
            return NameCell {
                icon: NameIcon::None,
                icon_size: IconSlot::Small,
                primary: SharedString::default(),
                count_run: None,
                secondary: None,
                glyphs: smallvec![(IconName::Lock, self.words.restricted_glyph_tip.clone())],
                highlight_fffd: false,
                trailing: None,
            };
        }
        if Self::kernel_owned(record) {
            return NameCell {
                primary: SharedString::from(self.words.no_owner.clone()),
                ..empty_name_cell()
            };
        }
        let Some(owner) = self.owner(record) else {
            return NameCell {
                primary: "?".into(),
                ..empty_name_cell()
            };
        };
        NameCell {
            icon: owner
                .meta
                .icon
                .as_ref()
                .map_or(NameIcon::None, |path| NameIcon::File(path.clone())),
            icon_size: IconSlot::Small,
            primary: SharedString::from(owner.meta.name.clone()),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<SysColumnId> {
        Some(id(ConnColumn::Process))
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
        _: &[u64],
        _: Option<SysColumnId>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }

    fn on_toggle_expand(&mut self, _: u64, _: &mut Context<TlTreeTable<Self>>) {}

    fn on_sort(&mut self, column: SysColumnId, cx: &mut Context<TlTreeTable<Self>>) {
        let ascending = if self.sort.column == column {
            !self.sort.ascending
        } else {
            true
        };
        self.sort = SysSortSpec { column, ascending };
        self.sort_request = Some(self.sort);
        cx.notify();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SysColumnId>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u64], _: &mut Context<TlTreeTable<Self>>) {}

    /// Enter, Alt+Enter and Ctrl+I arrive here as `OpenProperties`, which continues to the
    /// owning Connections page. The page also converts a propagated row double-click into the
    /// same action, as the Startup page does. The Connections page is tasklens-9o0.15.
    fn on_activate(&mut self, _: u64, cx: &mut Context<TlTreeTable<Self>>) {
        cx.propagate();
    }

    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        let lines = self.owner_tooltip_lines(ix)?;
        let cookie = self.record(ix)?.cookie;
        let key = (self.header.version, cookie);
        let mut tip = self.tip.borrow_mut();
        if tip.key != Some(key) {
            tip.key = Some(key);
            tip.lines = lines;
        }
        self.tip_view.clone()
    }

    fn empty_text(&self) -> Option<SharedString> {
        None
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn cell_tip_delay(&self) -> Duration {
        DELAY_ROW_NAME
    }

    fn cell_tip_content(&self, _: SysColumnId, tip: SharedString) -> TlTooltip {
        TlTooltip::lines(tip.split('\n').map(|line| line.to_owned().into()).collect())
    }

    fn before_rows(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) {
        if self.tip_view.is_none() {
            let state = Rc::clone(&self.tip);
            self.tip_view = Some(cx.new(|_| OwnerTip { state, built: None }).into());
        }
        if self.refresh_look(cx) {
            cx.notify();
        }
    }
}

fn empty_name_cell() -> NameCell {
    NameCell {
        icon: NameIcon::None,
        icon_size: IconSlot::Small,
        primary: SharedString::default(),
        count_run: None,
        secondary: None,
        glyphs: SmallVec::new(),
        highlight_fffd: false,
        trailing: None,
    }
}

impl SysDelegate for ConnectionsDelegate {
    fn component_header<'a>(&self, snap: &'a Snapshot) -> &'a ComponentHeader {
        &snap.connections.header
    }

    fn rows_len(&self, snap: &Snapshot) -> usize {
        snap.connections.rows.len()
    }

    fn query_generation(&self, snap: &Snapshot) -> u64 {
        snap.connections.query_generation
    }

    fn install(&mut self, snap: &Arc<Snapshot>) {
        let conns = Arc::clone(&snap.connections);
        self.rows = Arc::clone(&conns.rows);
        self.index = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(ix, row)| match row.node {
                SysNodeId::Conn(cookie) => Some((cookie, ix)),
                _ => None,
            })
            .collect();
        self.query_generation = conns.query_generation;
        self.header = conns.header;
        self.conns = Some(conns);
        self.procs = Some(Arc::clone(&snap.processes));
        self.tip.borrow_mut().key = None;
        self.version += 1;
    }

    fn mask(&self) -> ComponentMask {
        ComponentMask::CONNECTIONS | ComponentMask::PROCESSES
    }
}
