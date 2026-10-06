//! Process table query state (tasklens-pdu.11). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::GroupKind;
use tl_model::settings::{Settings, UiState};
use tl_model::table::{ProcessTableView, SortSpec, TableMode, TableQuery};
use tl_model::time::BootId;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::pages::processes::cells::ProcColumnKind;
use tl_ui::pages::processes::table::{GroupHeader, ProcessQuery};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

fn query() -> ProcessQuery {
    ProcessQuery::new(&Settings::default(), &UiState::default())
}

fn view(generation: u64) -> Arc<ProcessTableView> {
    Arc::new(ProcessTableView {
        query_generation: generation,
        ..ProcessTableView::default()
    })
}

fn process(pid: u32) -> NodeId {
    NodeId::Process(ProcessKey {
        start_ticks: 100,
        boot_id: BootId(0),
        pid,
    })
}

fn app(id: &str) -> NodeId {
    NodeId::Group(GroupKind::App { app_id: id.into() }.key())
}

fn sent(engine: &FakeEngine) -> Vec<TableQuery> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::SetTableQuery(q) => Some(q),
            _ => None,
        })
        .collect()
}

#[test]
fn query_defaults_from_settings_and_state() {
    let q = query();
    assert_eq!(q.mode(), TableMode::Grouped);
    assert_eq!(
        q.sort(),
        SortSpec {
            column: ColumnId::Pid,
            ascending: true
        }
    );
    assert_eq!(q.generation(), 0);
    for h in GroupHeader::ALL {
        assert!(q.is_expanded(&h.node()), "{h:?}");
    }
    assert!(!q.is_expanded(&NodeId::KernelThreads));
    let sent = q.to_query();
    assert_eq!(sent.expanded.len(), 3);
    assert!(!sent.expanded.contains(&NodeId::KernelThreads));

    let mut settings = Settings::default();
    settings.processes.mode = TableMode::Tree;
    let mut state = UiState::default();
    state.sort.processes.column = "cpu".to_owned();
    state.sort.processes.ascending = false;
    state.processes.group_expanded.background = false;
    let q = ProcessQuery::new(&settings, &state);
    assert_eq!(q.mode(), TableMode::Tree);
    assert_eq!(
        q.sort(),
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false
        }
    );
    assert!(q.is_expanded(&GroupHeader::Apps.node()));
    assert!(!q.is_expanded(&GroupHeader::Background.node()));
    assert!(q.is_expanded(&GroupHeader::System.node()));
}

#[test]
fn stale_generation_results_ignored() {
    let engine = FakeEngine::new();
    let mut q = query();
    for _ in 0..3 {
        q.bump_and_send(&*engine);
    }
    let three = view(3);
    assert!(q.accept(&three));
    q.bump_and_send(&*engine);
    assert_eq!(q.generation(), 4);

    assert!(!q.accept(&view(3)));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &three));

    let four = view(4);
    assert!(q.accept(&four));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &four));
    assert!(q.accept(&four));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &four));
}

#[test]
fn sort_first_click_desc_numeric_asc_text() {
    let mut q = query();
    let s = q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false
        }
    );
    let s = q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Cpu,
            ascending: true
        }
    );
    assert_eq!(q.sort(), s);

    let s = q.set_sort(ColumnId::Name, ProcColumnKind::Text);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Name,
            ascending: true
        }
    );
}

#[test]
fn collapse_all_keeps_group_headers() {
    let mut q = query();
    q.toggle(app("firefox"));
    q.toggle(process(42));
    q.expand_path(&[app("kate"), process(7), NodeId::KernelThreads]);
    assert_eq!(q.to_query().expanded.len(), 8);

    q.collapse_all();
    let expanded = q.to_query().expanded;
    assert_eq!(expanded.len(), 3);
    for h in GroupHeader::ALL {
        assert!(expanded.contains(&h.node()), "{h:?}");
    }
}

#[gpui_kit::test]
fn group_expand_persists_to_state(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let saved: Arc<std::sync::Mutex<Vec<UiState>>> = Arc::default();
    let log = Arc::clone(&saved);
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: Some(Box::new(move |_, state| {
                log.lock().unwrap().push(state.clone());
            })),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });

    let mut q = cx.read(|cx| ProcessQuery::new(TlSettings::settings(cx), TlSettings::state(cx)));
    q.toggle(GroupHeader::Apps.node());
    q.toggle(process(42));
    q.set_sort(ColumnId::Memory, ProcColumnKind::MemorySize);
    cx.update(|cx| q.persist(cx));

    let state = cx.read(|cx| TlSettings::state(cx).clone());
    let g = &state.processes.group_expanded;
    assert!(!g.apps);
    assert!(g.background);
    assert!(g.system);
    assert_eq!(state.sort.processes.column, "memory");
    assert!(!state.sort.processes.ascending);
    assert_eq!(saved.lock().unwrap().last(), Some(&state));

    // Expanding a process row alone changes nothing in state.toml.
    q.toggle(process(43));
    cx.update(|cx| q.persist(cx));
    assert_eq!(saved.lock().unwrap().len(), 1);
}

#[test]
fn each_change_sends_one_query() {
    let engine = FakeEngine::new();
    let mut q = query();

    q.set_mode(TableMode::Tree);
    q.bump_and_send(&*engine);
    q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    q.bump_and_send(&*engine);
    q.set_filter(Arc::from("fire"));
    q.bump_and_send(&*engine);
    q.toggle(process(42));
    q.bump_and_send(&*engine);

    let sent = sent(&engine);
    assert_eq!(sent.len(), 4);
    let generations: Vec<u64> = sent.iter().map(|s| s.generation).collect();
    assert_eq!(generations, [1, 2, 3, 4]);
    assert_eq!(sent[0].mode, TableMode::Tree);
    assert_eq!(sent[1].sort.column, ColumnId::Cpu);
    assert_eq!(&*sent[2].filter, "fire");
    assert!(sent[3].expanded.contains(&process(42)));
    assert!(!sent[2].expanded.contains(&process(42)));

    let mut settings = Settings::default();
    settings.processes.mode = TableMode::Tree;
    assert!(!q.sync_settings(&settings, &*engine));
    settings.processes.mode = TableMode::Flat;
    assert!(q.sync_settings(&settings, &*engine));
    let sent = self::sent(&engine);
    assert_eq!(sent.len(), 5);
    assert_eq!(sent[4].mode, TableMode::Flat);
    assert_eq!(sent[4].generation, 5);
}

#[test]
fn busy_query_is_resent() {
    let engine = FakeEngine::new();
    let mut q = query();
    engine.set_busy(true);
    q.bump_and_send(&*engine);
    assert!(sent(&engine).is_empty());
    engine.set_busy(false);
    q.resend_if_unsent(&*engine);
    q.resend_if_unsent(&*engine);
    let sent = sent(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].generation, 1);
}
