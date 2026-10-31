//! The Processes tab's side of the shell filter box (PROC-044, PROC-045, R-043, R-188,
//! UI-PRC-016, UI-PRC-024, UI-SHL-024, UI-SHL-025).
//!
//! The shell owns the input, Esc, Enter, Ctrl+F and type-to-filter, and debounces 100 ms
//! before it emits `FilterChanged`. Each event for this tab becomes one table query at once,
//! with the text lowercased; the table keeps its rows until the view of that generation
//! arrives. The engine filters. The filter text is never logged.

use std::sync::Arc;

use gpui_kit::{App, Context, Subscription};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::table::TableMode;

use super::{ProcessesPage, toast};
use crate::components::toast::ToastKind;
use crate::host::HostContext;
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::window::main::{FilterChanged, MemGroupCleared};

/// "Include children" shows while the filter has text or the mode is Tree (UI-SHL-025). The
/// shell's checkbox applies the same rule.
pub fn include_children_visible(filter_nonempty: bool, mode: TableMode) -> bool {
    filter_nonempty || mode == TableMode::Tree
}

/// The page's filter subscriptions and the last `processes.filter_include_children` seen.
pub struct FilterBinding {
    include_children: bool,
    _subs: SmallVec<[Subscription; 3]>,
}

impl FilterBinding {
    /// Subscribes to the shell's `FilterChanged` on `host`'s event bus and follows the
    /// include-children setting.
    pub fn new(host: &HostContext, cx: &mut Context<ProcessesPage>) -> Self {
        let mut subs = SmallVec::new();
        if let Some(events) = host.events().cloned() {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Processes {
                    this.apply_filter(&ev.text, cx);
                }
            }));
            subs.push(cx.subscribe(&events, |this, _, _: &MemGroupCleared, cx| {
                this.clear_mem_group(cx);
            }));
        }
        subs.push(cx.observe_global::<TlSettings>(|this, cx| this.on_include_children(cx)));
        Self {
            include_children: include_children_setting(cx),
            _subs: subs,
        }
    }
}

fn include_children_setting(cx: &App) -> bool {
    cx.has_global::<TlSettings>() && TlSettings::settings(cx).processes.filter_include_children
}

impl ProcessesPage {
    /// One `FilterChanged` for this tab: the lowercased text and a new generation, sent now.
    /// The same text sends nothing; the jump's Show already cleared the filter before the
    /// shell's own `FilterChanged` for it arrives.
    fn apply_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        let lower = text.to_lowercase();
        let client = Arc::clone(&self.client);
        self.table.update(cx, |t, cx| {
            let q = t.delegate.query_mut();
            if q.filter().as_ref() == lower {
                return;
            }
            q.set_filter(Arc::from(lower));
            q.bump_and_send(&*client);
            // The filtered list opens at its first match, not at the old offset.
            t.scroll_to_top(cx);
        });
    }

    /// The shell's group chip was cleared: drop the memory group filter and send the query.
    fn clear_mem_group(&mut self, cx: &mut Context<Self>) {
        let client = Arc::clone(&self.client);
        self.table.update(cx, |t, cx| {
            let q = t.delegate.query_mut();
            if q.mem_group().is_none() {
                return;
            }
            q.set_mem_group(None);
            q.bump_and_send(&*client);
            t.scroll_to_top(cx);
        });
    }

    /// The Info toast when `processes.filter_include_children` changes (UI-PRC-016).
    fn on_include_children(&mut self, cx: &mut Context<Self>) {
        let on = include_children_setting(cx);
        if on == self.filter.include_children {
            return;
        }
        self.filter.include_children = on;
        let key = if on {
            "processes-toast-including-children"
        } else {
            "processes-toast-hiding-children"
        };
        toast(ToastKind::Info, tr(cx, key), cx);
    }
}
