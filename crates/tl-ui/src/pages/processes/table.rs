//! Process table query state (ADR-009, UI-PRC-002, UI-PRC-016).
//!
//! `ProcessQuery` owns the `TableQuery` the UI sends to the engine: mode, sort, filter, expanded
//! rows and the generation. The engine sorts, filters, groups and flattens (PROC-013). A view is
//! shown only when its `query_generation` equals the last generation sent, so a stale view costs
//! one integer compare.
//!
//! Every change of mode, sort, filter or expansion is followed by exactly one `bump_and_send`.
//! Group header expansion and the sort persist to `state.toml` through `persist`; process, app
//! and unit expansion lives for the session only (PROC-014).

use std::sync::Arc;

use gpui_kit::App;
use rustc_hash::FxHashSet;
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::ids::{ColumnId, NodeId};
use tl_model::process::GroupKind;
use tl_model::settings::{Settings, SortState, UiState};
use tl_model::table::{ProcessTableView, SortSpec, TableMode, TableQuery};
use tracing::{debug, warn};

use super::cells::ProcColumnKind;
use crate::settings_global::TlSettings;

/// The three group headers of grouped mode, in display order (PROC-004).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupHeader {
    Apps,
    Background,
    System,
}

impl GroupHeader {
    pub const ALL: [GroupHeader; 3] = [
        GroupHeader::Apps,
        GroupHeader::Background,
        GroupHeader::System,
    ];

    /// The header row's `NodeId`, as the engine builds it: Background and System use the key of
    /// their `GroupKind`, Apps uses the key of an app with an empty id.
    pub fn node(self) -> NodeId {
        let kind = match self {
            GroupHeader::Apps => GroupKind::App {
                app_id: Arc::from(""),
            },
            GroupHeader::Background => GroupKind::Background,
            GroupHeader::System => GroupKind::System,
        };
        NodeId::Group(kind.key())
    }
}

/// The process table's query and the view on screen.
pub struct ProcessQuery {
    /// Starts at 0; +1 for every query sent.
    generation: u64,
    mode: TableMode,
    sort: SortSpec,
    /// Lowercase. User text: never logged.
    filter: Arc<str>,
    /// `processes.filter_include_children`.
    include_children: bool,
    /// Session state per identity.
    expanded: FxHashSet<NodeId>,
    /// Rows the user collapsed, so defaults do not re-expand them.
    collapsed_by_user: FxHashSet<NodeId>,
    /// The view on screen.
    shown: Option<Arc<ProcessTableView>>,
    /// The last `send` failed (queue full), so the engine has not seen `generation`.
    unsent: bool,
    /// The header node ids, computed once.
    headers: [NodeId; 3],
}

impl ProcessQuery {
    /// Mode from `processes.mode`, sort from `[sort.processes]` (an unknown column falls back to
    /// pid ascending), group headers from `[processes] group_expanded`. Every other row starts
    /// collapsed, the Kernel threads row included (PROC-009).
    pub fn new(settings: &Settings, state: &UiState) -> Self {
        let saved = &state.sort.processes;
        let sort = match ColumnId::from_str(&saved.column) {
            Some(column) => SortSpec {
                column,
                ascending: saved.ascending,
            },
            None => {
                warn!(target: "tasklens::processes", column = %saved.column, "unknown sort column");
                TableQuery::default().sort
            }
        };
        let headers = GroupHeader::ALL.map(GroupHeader::node);
        let open = &state.processes.group_expanded;
        let mut expanded = FxHashSet::default();
        let mut collapsed_by_user = FxHashSet::default();
        for (header, is_open) in headers
            .iter()
            .zip([open.apps, open.background, open.system])
        {
            if is_open {
                expanded.insert(header.clone());
            } else {
                collapsed_by_user.insert(header.clone());
            }
        }
        Self {
            generation: 0,
            mode: settings.processes.mode,
            sort,
            filter: Arc::from(""),
            include_children: settings.processes.filter_include_children,
            expanded,
            collapsed_by_user,
            shown: None,
            unsent: false,
            headers,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn mode(&self) -> TableMode {
        self.mode
    }

    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    pub fn filter(&self) -> &Arc<str> {
        &self.filter
    }

    pub fn is_expanded(&self, id: &NodeId) -> bool {
        self.expanded.contains(id)
    }

    /// The query for the current state. Allocates one `Arc<[NodeId]>`; call it per sent query,
    /// never per frame.
    pub fn to_query(&self) -> TableQuery {
        TableQuery {
            generation: self.generation,
            mode: self.mode,
            sort: self.sort,
            filter: Arc::clone(&self.filter),
            expanded: self.expanded.iter().cloned().collect(),
            include_children: self.include_children,
        }
    }

    /// `generation += 1`, then sends `Command::SetTableQuery`. A full command queue leaves the
    /// query unsent; `resend_if_unsent` tries again.
    pub fn bump_and_send(&mut self, client: &dyn EngineClient) {
        self.generation += 1;
        self.send(client);
    }

    /// Sends the current query again when the last send failed. Call it once per snapshot.
    pub fn resend_if_unsent(&mut self, client: &dyn EngineClient) {
        if self.unsent {
            self.send(client);
        }
    }

    fn send(&mut self, client: &dyn EngineClient) {
        match client.send(Command::SetTableQuery(self.to_query())) {
            Ok(()) => self.unsent = false,
            Err(e) => {
                debug!(target: "tasklens::processes", generation = self.generation, error = ?e, "table query not sent");
                self.unsent = true;
            }
        }
    }

    /// True and stores `view` when it was built for the current generation. A stale view leaves
    /// `shown` as it was. The same `Arc` again is not stored twice.
    pub fn accept(&mut self, view: &Arc<ProcessTableView>) -> bool {
        if view.query_generation != self.generation {
            return false;
        }
        if !self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, view)) {
            self.shown = Some(Arc::clone(view));
        }
        true
    }

    /// Forgets the stored view, so `shown` is `None` until a view for this generation arrives.
    pub fn forget_shown(&mut self) {
        self.shown = None;
    }

    pub fn set_mode(&mut self, m: TableMode) {
        self.mode = m;
    }

    /// Follows `processes.mode` and `processes.filter_include_children`. Sends one query and
    /// returns true when either changed.
    pub fn sync_settings(&mut self, settings: &Settings, client: &dyn EngineClient) -> bool {
        let p = &settings.processes;
        if p.mode == self.mode && p.filter_include_children == self.include_children {
            return false;
        }
        self.set_mode(p.mode);
        self.include_children = p.filter_include_children;
        self.bump_and_send(client);
        true
    }

    /// A header click. The same column again flips the direction. A new column starts
    /// ascending for Text and PowerClass and descending for every numeric kind (PROC-047).
    pub fn set_sort(&mut self, col: ColumnId, kind: ProcColumnKind) -> SortSpec {
        self.sort = if col == self.sort.column {
            SortSpec {
                column: col,
                ascending: !self.sort.ascending,
            }
        } else {
            SortSpec {
                column: col,
                ascending: matches!(kind, ProcColumnKind::Text | ProcColumnKind::PowerClass),
            }
        };
        self.sort
    }

    /// `lower` is already lowercase.
    pub fn set_filter(&mut self, lower: Arc<str>) {
        self.filter = lower;
    }

    pub fn toggle(&mut self, id: NodeId) {
        if self.expanded.remove(&id) {
            self.collapsed_by_user.insert(id);
        } else {
            self.collapsed_by_user.remove(&id);
            self.expanded.insert(id);
        }
    }

    /// Expands every row in `ids`, for the jump target (E22-A3).
    pub fn expand_path(&mut self, ids: &[NodeId]) {
        for id in ids {
            self.collapsed_by_user.remove(id);
            self.expanded.insert(id.clone());
        }
    }

    /// Clears every expansion except the three group headers, which keep their state (PROC-020).
    pub fn collapse_all(&mut self) {
        let headers = &self.headers;
        let collapsed = &mut self.collapsed_by_user;
        self.expanded.retain(|id| {
            let keep = headers.contains(id);
            if !keep {
                collapsed.insert(id.clone());
            }
            keep
        });
    }

    pub fn shown(&self) -> Option<&Arc<ProcessTableView>> {
        self.shown.as_ref()
    }

    /// Writes the sort and the group header expansion into `state`.
    pub fn save(&self, state: &mut UiState) {
        state.sort.processes = SortState {
            column: self.sort.column.as_str().to_owned(),
            ascending: self.sort.ascending,
        };
        let g = &mut state.processes.group_expanded;
        let [apps, background, system] = &self.headers;
        g.apps = self.expanded.contains(apps);
        g.background = self.expanded.contains(background);
        g.system = self.expanded.contains(system);
    }

    /// `save` through `TlSettings::update`, which persists only when the state changed (tl-app
    /// debounces the write by 500 ms, ADR-011). Call it after every sort or expansion change.
    pub fn persist(&self, cx: &mut App) {
        TlSettings::update(cx, |d| self.save(&mut d.state));
    }
}
