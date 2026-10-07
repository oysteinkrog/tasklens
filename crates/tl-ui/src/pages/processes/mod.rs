//! Processes tab: ProcessesPage, the process table delegate, its cells, menus, confirmations and toasts (docs/ui/processes.md).
//!
//! `ProcessesPage` owns the process table (ADR-009). It subscribes to the hub in `new`, hands
//! each snapshot whose drawn parts changed to the delegate and notifies the table only when the
//! view is the one last asked for. The table is a cached view, so a page render does not redraw
//! it, and nothing here notifies inside `render`.

pub mod actions;
pub mod cells;
pub mod confirm;
pub mod delegate;
pub mod header_totals;
pub mod history;
pub mod name_cell;
pub mod table;
pub mod toolbar;
pub mod tooltip;

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StyleRefinement, Styled as _, Subscription,
    UniformListScrollHandle, Window, div,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::settings::{Settings, UiState};

use self::delegate::ProcessDelegate;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::Strings;
use crate::settings_global::{Section, TlSettings};
use crate::table::actions::CollapseAll;
use crate::table::tree_table::TlTreeTable;
use crate::theme::{UiMetrics, bump_style_version, current, metrics};
use crate::window::main::ShellPage;
use crate::window::status::StatusChanged;

/// Key context of the page div. The table inside it has `Table`.
pub const PAGE_CONTEXT: &str = "ProcessTable";

/// The snapshot parts the table draws. Settings changes come through `TlSettings`.
const DRAWN: ComponentMask = ComponentMask::PROCESSES
    .union(ComponentMask::TABLE)
    .union(ComponentMask::WINDOWS)
    .union(ComponentMask::DEVICES);

/// The settings sections the page follows, in this order: `processes`, `appearance`, `units`,
/// `tables`.
const SECTIONS: [Section; 4] = [
    Section::Processes,
    Section::Appearance,
    Section::Units,
    Section::Tables,
];

/// The entity behind the Processes tab. Built on the first visit and kept (ADR-009).
pub struct ProcessesPage {
    table: Entity<TlTreeTable<ProcessDelegate>>,
    /// Created in `new`, never in render.
    _hub_sub: Subscription,
    /// The table body's focus handle.
    focus: FocusHandle,
    client: Arc<dyn EngineClient>,
    /// Settings, metrics and strings observers.
    _subs: SmallVec<[Subscription; 3]>,
    /// `TlSettings` versions of [`SECTIONS`] last applied.
    section_versions: [u64; 4],
    style_version: u64,
    strings_version: u64,
}

impl EventEmitter<StatusChanged> for ProcessesPage {}

impl ProcessesPage {
    /// Builds the table over the hub's current snapshot, sends the first table query
    /// (generation 1) and subscribes to the hub. Needs `GlobalHub`.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<GlobalHub>().0.clone();
        let (snapshot, client) = {
            let hub = hub.read(cx);
            (Arc::clone(hub.current()), Arc::clone(hub.client()))
        };
        let (settings, state) = if cx.has_global::<TlSettings>() {
            (
                TlSettings::settings(cx).clone(),
                TlSettings::state(cx).clone(),
            )
        } else {
            (Settings::default(), UiState::default())
        };
        let mut delegate = ProcessDelegate::new(snapshot, &settings, &state, window, cx);
        delegate.set_window(Some(window.window_handle()));
        delegate.query_mut().bump_and_send(&*client);
        let defaults = delegate.default_layout();
        let table = cx.new(|cx| {
            let mut t = TlTreeTable::new(delegate, window, cx);
            t.set_default_layout(defaults);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            t
        });
        let focus = table.read(cx).focus_handle().clone();
        let hub_sub = cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            this.on_snapshot(ev.mask, cx);
        });
        let mut subs = SmallVec::new();
        subs.push(cx.observe_global::<TlSettings>(|this, cx| this.on_settings(cx)));
        subs.push(cx.observe_global::<UiMetrics>(|this, cx| this.on_metrics(cx)));
        subs.push(cx.observe_global::<Strings>(|this, cx| this.on_strings(cx)));
        let section_versions = cx
            .try_global::<TlSettings>()
            .map_or([0; 4], |tl| SECTIONS.map(|s| tl.version(s)));
        Self {
            table,
            _hub_sub: hub_sub,
            focus,
            client,
            _subs: subs,
            section_versions,
            style_version: cx.try_global::<UiMetrics>().map_or(0, |m| m.style_version),
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
        }
    }

    pub fn table(&self) -> &Entity<TlTreeTable<ProcessDelegate>> {
        &self.table
    }

    /// The handle the table's `uniform_list` tracks. Scrolling through it moves the table (the
    /// E01 `table-2000` bench scene drives it).
    pub fn scroll_handle(&self, cx: &App) -> UniformListScrollHandle {
        self.table.read(cx).scroll_handle()
    }

    pub fn focus_table(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// One hub change. Only a change to a drawn part reaches the delegate, and the table is
    /// notified only when the view was built for the current query.
    fn on_snapshot(&mut self, mask: ComponentMask, cx: &mut Context<Self>) {
        let hub = cx.global::<GlobalHub>().0.clone();
        let snapshot = Arc::clone(hub.read(cx).current());
        let client = &*self.client;
        self.table.update(cx, |t, cx| {
            let d = &mut t.delegate;
            d.query_mut().resend_if_unsent(client);
            if !mask.intersects(DRAWN) || snapshot.table.query_generation != d.query().generation()
            {
                return;
            }
            if d.install(&snapshot) {
                t.on_new_version(cx);
            } else {
                cx.notify();
            }
        });
    }

    /// `processes.mode` sends a query; `appearance`, `units` and `tables` start a new style
    /// version, which clears the cell text caches.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let Some(tl) = cx.try_global::<TlSettings>() else {
            return;
        };
        let versions = SECTIONS.map(|s| tl.version(s));
        if versions == self.section_versions {
            return;
        }
        let old = std::mem::replace(&mut self.section_versions, versions);
        let processes = versions[0] != old[0];
        let restyle = versions[1..] != old[1..];
        let settings = TlSettings::settings(cx).clone();
        let client = &*self.client;
        self.table.update(cx, |t, cx| {
            t.delegate.set_settings(&settings);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            if processes {
                t.delegate.query_mut().sync_settings(&settings, client);
            }
            if restyle {
                bump_style_version(cx);
            }
            cx.notify();
        });
    }

    /// A theme switch bumps the style version; the heat colours follow the new theme.
    fn on_metrics(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let theme = current(cx).clone();
        self.table.update(cx, |t, cx| {
            t.delegate.set_theme(theme);
            cx.notify();
        });
    }

    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.table.update(cx, |t, cx| {
            t.delegate.refresh_strings(cx);
            t.on_new_version(cx);
        });
    }

    /// Ctrl+L: collapses every row but the three group headers (PROC-020).
    fn on_collapse_all(&mut self, _: &CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        let client = &*self.client;
        self.table.update(cx, |t, _| {
            let q = t.delegate.query_mut();
            q.collapse_all();
            q.bump_and_send(client);
        });
    }
}

impl Render for ProcessesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "ProcessesPage".into())
            .key_context(PAGE_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_collapse_all))
            .child(
                self.table
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
    }
}

impl ShellPage for ProcessesPage {
    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }
}
