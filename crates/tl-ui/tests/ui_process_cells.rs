//! Process column registry (E22-002): ids, labels, groups, kinds, templates, widths and heat.

use gpui_kit::TestAppContext;
use rustc_hash::FxHashSet;
use tl_model::ids::ColumnId;
use tl_model::settings::ColumnTables;
use tl_ui::i18n::{init_strings, tr};
use tl_ui::pages::processes::cells::{
    DEFAULT_VISIBLE, HeatRef, PROCESS_COLUMNS, PickerGroup, ProcColumnKind, process_column,
};
use tl_ui::table::columns::ColumnAlign;

const UNSORTABLE: [ColumnId; 6] = [
    ColumnId::Pss,
    ColumnId::Uss,
    ColumnId::Cycles,
    ColumnId::CyclesDelta,
    ColumnId::CtxSwitches,
    ColumnId::CtxSwitchesDelta,
];

#[test]
fn registry_ids_unique_and_complete() {
    assert_eq!(PROCESS_COLUMNS.len(), ColumnId::COUNT);
    assert_eq!(PROCESS_COLUMNS[0].id, ColumnId::Name);
    let ids: FxHashSet<ColumnId> = PROCESS_COLUMNS.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), ColumnId::COUNT, "an id appears twice");
    for id in ColumnId::ALL {
        assert!(ids.contains(&id), "{} missing", id.as_str());
        assert_eq!(process_column(id).id, id);
    }
}

#[test]
fn registry_groups_in_picker_order() {
    let mut last = 0;
    for col in PROCESS_COLUMNS {
        let at = PickerGroup::ALL
            .iter()
            .position(|g| *g == col.group)
            .expect("group listed");
        assert!(at >= last, "{} out of group order", col.id.as_str());
        last = at;
    }
    assert!(PROCESS_COLUMNS.iter().all(|c| (1..=4).contains(&c.phase)));
}

#[gpui_kit::test]
fn registry_labels_resolve(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_strings(cx, "en-US");
        for col in PROCESS_COLUMNS {
            for key in [Some(col.label), col.alias, col.header_tooltip]
                .into_iter()
                .flatten()
            {
                assert_ne!(tr(cx, key).as_ref(), key, "{key} has no string");
            }
        }
        for group in PickerGroup::ALL {
            assert_ne!(tr(cx, group.label_key()).as_ref(), group.label_key());
        }
        let label = |id| tr(cx, process_column(id).label);
        assert_eq!(label(ColumnId::Name), "Name");
        assert_eq!(label(ColumnId::CpuRelative), "CPU (one core = 100%)");
        assert_eq!(label(ColumnId::GpuMemDedicated), "VRAM");
        assert_eq!(label(ColumnId::Gpu15), "GPU 15");
        assert_eq!(label(ColumnId::OomScoreAdj), "OOM adj");
        let alias = |id| process_column(id).alias.map(|k| tr(cx, k));
        assert_eq!(
            alias(ColumnId::Memory).as_deref(),
            Some("Working set private")
        );
        assert_eq!(
            alias(ColumnId::GpuMemDedicated).as_deref(),
            Some("GPU memory dedicated")
        );
        assert_eq!(alias(ColumnId::Cpu), None);
        let tip = |id| process_column(id).header_tooltip.map(|k| tr(cx, k));
        assert_eq!(
            tip(ColumnId::Memory).as_deref(),
            Some("Private memory (RssAnon). Shared and file-backed pages are not counted.")
        );
        assert_eq!(
            tip(ColumnId::WsTotal).as_deref(),
            Some("Resident set size (VmRSS)")
        );
        assert_eq!(
            tip(ColumnId::Network).as_deref(),
            Some("Attributed TCP traffic. UDP and other users' sockets are not counted.")
        );
        assert_eq!(
            tip(ColumnId::Handles).as_deref(),
            Some("Open file descriptors")
        );
        assert_eq!(tr(cx, PickerGroup::Io.label_key()), "I/O");
    });
}

#[gpui_kit::test]
fn registry_unsortable_columns(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_strings(cx, "en-US");
        for id in UNSORTABLE {
            let col = process_column(id);
            assert!(!col.sortable, "{} sortable", id.as_str());
            let tip = col.header_tooltip.map(|k| tr(cx, k));
            assert_eq!(tip.as_deref(), Some("Only measured for visible rows"));
        }
    });
    let sortable = PROCESS_COLUMNS.iter().filter(|c| c.sortable).count();
    assert_eq!(sortable, ColumnId::COUNT - UNSORTABLE.len());
}

#[test]
fn registry_default_set_matches_table_a() {
    let table_a = ColumnTables::default().processes;
    assert_eq!(table_a.order, DEFAULT_VISIBLE);
    assert_eq!(table_a.visible, DEFAULT_VISIBLE);
    for id in DEFAULT_VISIBLE {
        assert!(ColumnId::from_str(id).is_some(), "{id} unknown");
    }
    for col in PROCESS_COLUMNS {
        let width = table_a.widths.get(col.id.as_str()).map(|&w| w as f32);
        assert_eq!(col.default_width, width, "{}", col.id.as_str());
    }
}

#[test]
fn registry_templates_per_kind() {
    let t = |id, net_bytes| process_column(id).template(net_bytes);
    for id in [ColumnId::Pid, ColumnId::Ppid, ColumnId::Sid] {
        assert_eq!(t(id, false), Some("999999 +99"));
    }
    for col in PROCESS_COLUMNS {
        if col.kind == ProcColumnKind::Percent {
            assert_eq!(col.template(false), Some("100.0%"), "{}", col.id.as_str());
        }
    }
    assert_eq!(t(ColumnId::Gpu, false), Some("100.0%"));
    assert_eq!(t(ColumnId::Gpu7, false), Some("100.0%"));
    for id in [
        ColumnId::Network,
        ColumnId::NetSendRate,
        ColumnId::NetRecvRate,
    ] {
        assert_eq!(t(id, false), Some("xxx Mbps"));
        assert_eq!(t(id, true), Some("xxx MiB/s"));
    }
    for id in [
        ColumnId::Disk,
        ColumnId::DiskReadRate,
        ColumnId::IoWriteRate,
    ] {
        assert_eq!(t(id, false), Some("xxx GiB/s"));
        assert_eq!(t(id, true), Some("xxx GiB/s"));
    }
    for id in [
        ColumnId::Memory,
        ColumnId::WsTotal,
        ColumnId::GpuMemDedicated,
    ] {
        assert_eq!(t(id, false), Some("999 MiB"));
    }
    for id in [
        ColumnId::Threads,
        ColumnId::Handles,
        ColumnId::PageFaults,
        ColumnId::CtxSwitches,
        ColumnId::IoReadOps,
        ColumnId::IoWriteOps,
    ] {
        assert_eq!(t(id, false), Some("999999"));
    }
    for id in [
        ColumnId::CpuTime,
        ColumnId::CpuTimeKernel,
        ColumnId::CpuTimeUser,
    ] {
        assert_eq!(t(id, false), Some("999:59:59"));
    }
    for id in [
        ColumnId::Name,
        ColumnId::User,
        ColumnId::ImagePath,
        ColumnId::Cmdline,
    ] {
        assert_eq!(t(id, false), None);
    }
    for col in PROCESS_COLUMNS {
        let left = matches!(col.kind, ProcColumnKind::Text | ProcColumnKind::PowerClass);
        let expected = if left {
            ColumnAlign::Left
        } else {
            ColumnAlign::Right
        };
        assert_eq!(col.align(), expected, "{}", col.id.as_str());
    }
}

#[test]
fn registry_heat_refs() {
    let heat = |id| process_column(id).heat;
    assert_eq!(heat(ColumnId::Cpu), HeatRef::CpuMachine);
    assert_eq!(heat(ColumnId::CpuKernel), HeatRef::CpuMachine);
    assert_eq!(heat(ColumnId::CpuRelative), HeatRef::CpuOneCore);
    assert_eq!(heat(ColumnId::Memory), HeatRef::Memory);
    assert_eq!(heat(ColumnId::Disk), HeatRef::DiskIo);
    assert_eq!(heat(ColumnId::IoReadRate), HeatRef::DiskIo);
    assert_eq!(heat(ColumnId::Network), HeatRef::Network);
    assert_eq!(heat(ColumnId::Gpu), HeatRef::GpuPercent);
    assert_eq!(heat(ColumnId::Gpu3), HeatRef::GpuPercent);
    assert_eq!(heat(ColumnId::GpuMemDedicated), HeatRef::Vram);
    assert_eq!(heat(ColumnId::Power), HeatRef::Power);
    assert_eq!(heat(ColumnId::Threads), HeatRef::None);
    for col in PROCESS_COLUMNS {
        if matches!(
            col.kind,
            ProcColumnKind::Count
                | ProcColumnKind::IntegerId
                | ProcColumnKind::Text
                | ProcColumnKind::Duration
        ) {
            assert_eq!(col.heat, HeatRef::None, "{}", col.id.as_str());
        }
        if col.kind == ProcColumnKind::MemorySize && col.id != ColumnId::GpuMemDedicated {
            assert_eq!(col.heat, HeatRef::Memory, "{}", col.id.as_str());
        }
    }
}
