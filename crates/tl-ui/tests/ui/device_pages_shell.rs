//! The device page tests that need the shell. The Disk is in use window opens through the
//! shell's `ShellApi::disk_eject`, and the SMART window lives in tl-ui. The other device page
//! tests are in tl-ui-perf: `cargo test -p tl-ui-perf --features test-hooks --test perf --
//! device_pages::`.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyView, AppContext as _, Entity, Focusable as _, SharedString, TestAppContext,
    VisualTestContext, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, DeviceCommand};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::DiskDetails;
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui_base::action_tracker::{ActionTracker, GlobalActionTracker};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::kv_grid;
use tl_ui_perf::performance::disk::DiskPage;
use tl_ui_perf::performance::{DeviceKey, PerformancePage};
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("device"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind,
        hot_c: None,
    }
}

fn disk(idx: u16, name: &str) -> DeviceEntry {
    entry(
        idx,
        DeviceIdentity::Disk {
            major: 8,
            minor: idx.into(),
            name: Arc::from(name),
            wwn_or_serial: None,
        },
        None,
    )
}

fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
    entry(
        idx,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: idx.into(),
            name: Arc::from(name),
        },
        Some(kind),
    )
}

/// CPU, Memory, a GPU, four disks, a wired interface and a VPN tunnel.
fn catalog() -> Vec<DeviceEntry> {
    vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(
            2,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:2d:00.0"),
            },
            None,
        ),
        disk(3, "sda"),
        disk(4, "sdb"),
        disk(5, "sdc"),
        disk(6, "sdd"),
        net(7, "enp38s0", NetKind::Wired),
        net(8, "tailscale0", NetKind::VpnTunnel),
    ]
}

struct Rig<'a> {
    page: Entity<PerformancePage>,
    engine: Arc<FakeEngine>,
    vcx: &'a mut VisualTestContext,
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// Default settings with the Overview card hidden (D-022), so CPU is the first page.
fn hidden_overview() -> Settings {
    let mut s = Settings::default();
    s.performance.device_overrides.insert(
        tl_ui_perf::performance::overview::OVERVIEW_KEY.to_owned(),
        tl_ui_settings::settings::DeviceVisibility::Hide,
    );
    s
}

fn install(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(catalog()),
        });
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings: hidden_overview(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    engine
}

/// The Performance tab as the window root, 1280 × 800, CPU selected. With `with_tracker` also
/// an action tracker in the page's host, set as `GlobalActionTracker` too. The tracker belongs
/// to the host when the page is built.
fn open_with(cx: &mut TestAppContext, with_tracker: bool) -> Rig<'_> {
    let engine = install(cx);
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
    let tracker = with_tracker.then(|| {
        let tracker = cx.new(|cx| ActionTracker::new(Arc::clone(&client), None, cx));
        cx.update(|cx| cx.set_global(GlobalActionTracker(tracker.clone())));
        tracker
    });
    // Only EngineGlobal (and the tracker) are installed here; the hub and demand are the test's.
    let host = HostContext::from_parts(Some(client), Some(hub), Some(demand), tracker, None);
    let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(&host, cx));
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1280.), px(800.)));
    draw(vcx);
    Rig { page, engine, vcx }
}

fn key(s: &str) -> DeviceKey {
    DeviceKey(Arc::from(s))
}

/// Selects the card of `k` and lets the demand flush reach the engine.
fn select(r: &mut Rig, k: &str) {
    let page = r.page.clone();
    let k = key(k);
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(k, cx));
    });
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(200));
    draw(r.vcx);
}

fn view_of(r: &mut Rig, k: &str) -> AnyView {
    let page = r.page.clone();
    let k = key(k);
    r.vcx
        .update(|_, cx| page.read(cx).page(&k).expect("page built").clone())
}

/// The "Disk is in use" window (E25-020, PERF-052, UI-PRF-030).
mod disk_in_use {
    use super::*;

    use gpui_kit::Modifiers;
    use gpui_kit::base::Root;
    use tl_model::actions::{
        ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, Signal,
    };
    use tl_model::command::{Command, DeviceCommand};
    use tl_model::ids::{GroupKey, ProcessKey};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::{Blocker, DiskDetails, EjectState};
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;
    use tl_ui_kit::components::dialog::{DialogButton, TlDialogView};
    use tl_ui_kit::components::toast::ToastKind;
    use tl_ui_kit::components::window_frame::TlSecondaryWindow;
    use tl_ui_perf::performance::actions::PerfEject;
    use tl_ui_procs::windows::disk_in_use::{
        DiskInUseWindow, EjectToasts, RETRY_DELAY, find_disk_in_use,
    };

    const SDA: &str = "disk:sda";
    const IDX: DeviceIdx = DeviceIdx(3);
    const PIDS: [u32; 3] = [101, 102, 103];

    fn pkey(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 500,
            boot_id: BootId(0),
            pid,
        }
    }

    fn record(pid: u32, name: &str, app: &str) -> ProcessRecord {
        let text = |value: &str| Arc::<str>::from(value);
        ProcessRecord {
            key: pkey(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text(name),
                name: text(name),
                exe: Some(text(&format!("/usr/bin/{name}"))),
                cmdline: None,
                user: text("me"),
                cgroup: None,
                app_id: Some(text(app)),
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: text(name),
            }),
            group: GroupKey(u64::from(pid)),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// Two apps and three processes: 101 and 103 in Files, 102 in Editor.
    fn blockers() -> Arc<[Blocker]> {
        let b = |pid: u32, name: &str, app: &str, path: &str| Blocker {
            key: pkey(pid),
            name: Arc::from(name),
            app: Some(Arc::from(app)),
            paths: Arc::from([Arc::<str>::from(path), Arc::<str>::from("/media/usb/b")]),
        };
        Arc::from([
            b(101, "files", "org.files", "/media/usb/a"),
            b(102, "editor", "org.editor", "/media/usb/c"),
            b(103, "files-helper", "org.files", "/media/usb/d"),
        ])
    }

    fn records() -> Arc<[ProcessRecord]> {
        Arc::from([
            record(101, "Files", "org.files"),
            record(102, "Editor", "org.editor"),
            record(103, "files-helper", "org.files"),
        ])
    }

    /// The Disk page of sda with an action tracker installed. The toast layer is registered as
    /// the app does, so the window's own toast stack takes the action toasts. The Disk page asks
    /// the shell for the eject, so the shell services are installed (without a main window).
    fn setup(cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(tl_ui_kit::components::toast::init);
        cx.update(tl_ui::window::main::install_shell_api);
        cx.update(|cx| cx.set_global(EjectToasts::default()));
        let mut r = open_with(cx, true);
        select(&mut r, SDA);
        publish(&mut r, None);
        r
    }

    fn publish(r: &mut Rig, eject: Option<(u64, EjectState)>) {
        r.engine.publish_with(|s| {
            let details = Arc::make_mut(&mut s.device_details);
            details.header.version += 1;
            details.disks = Arc::from([(
                IDX,
                DiskDetails {
                    removable: true,
                    eject,
                    ..DiskDetails::default()
                },
            )]);
            let p = Arc::make_mut(&mut s.processes);
            p.header.version += 1;
            p.records = records();
        });
        draw(r.vcx);
    }

    fn eject_seqs(r: &Rig) -> Vec<u64> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Device(DeviceCommand::Eject { seq, disk }) => {
                    assert_eq!(&*disk, "sda");
                    Some(seq)
                }
                _ => None,
            })
            .collect()
    }

    fn sent(r: &Rig) -> Vec<ActionRequest> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// Dispatches `PerfEject` on the Disk page and returns the sequence it sent.
    fn eject(r: &mut Rig) -> u64 {
        let page = view_of(r, SDA).downcast::<DiskPage>().expect("a DiskPage");
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(&PerfEject { key: key(SDA) }, window, cx);
        });
        r.vcx.run_until_parked();
        *eject_seqs(r).last().expect("an eject was sent")
    }

    fn blocked(others_may_hold: bool) -> EjectState {
        EjectState::Blocked {
            blockers: blockers(),
            others_may_hold,
        }
    }

    /// The open window, drawn.
    fn window(r: &mut Rig) -> (VisualTestContext, Entity<DiskInUseWindow>) {
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_disk_in_use(IDX, cx))
            .expect("the Disk is in use window is open");
        let mut vcx = VisualTestContext::from_window(handle.into(), &r.vcx.cx);
        redraw(&mut vcx);
        (vcx, view)
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Ejects and publishes `Blocked` for that sequence; returns the sequence and the window.
    fn open_blocked(
        r: &mut Rig,
        others_may_hold: bool,
    ) -> (u64, VisualTestContext, Entity<DiskInUseWindow>) {
        let seq = eject(r);
        publish(r, Some((seq, blocked(others_may_hold))));
        let (vcx, view) = window(r);
        (seq, vcx, view)
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = vcx.debug_bounds(selector).expect(selector).center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }

    /// The open confirmation dialog, drawn.
    fn dialog(r: &mut Rig) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
        let (any, frame) = r.vcx.update(|_, cx| {
            cx.windows().into_iter().find_map(|w| {
                let handle = w.downcast::<Root>()?;
                let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                Some((w, frame))
            })
        })?;
        let view = r.vcx.update(|_, cx| frame.read(cx).content().clone());
        let mut vcx = VisualTestContext::from_window(any, &r.vcx.cx);
        vcx.update(|window, _| window.activate_window());
        redraw(&mut vcx);
        Some((vcx, view))
    }

    #[gpui_kit::test]
    fn disk_in_use_layout(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, false);
        vcx.update(|_, cx| {
            let w = view.read(cx);
            assert_eq!(w.groups().len(), 2);
            assert_eq!(w.groups()[0].name.as_ref(), "Files");
            assert_eq!(w.groups()[0].rows.len(), 2, "101 and 103 share an app");
            assert_eq!(w.groups()[1].name.as_ref(), "Editor");
            assert_eq!(w.row_label().as_ref(), "End Task");
            assert_eq!(w.intro().as_ref(), "These programs are using Disk 0 (sda):");
            assert!(!w.eacces_note());
        });
        assert!(vcx.debug_bounds("DiskInUseGroup-0").is_some());
        assert!(vcx.debug_bounds("DiskInUseGroup-1").is_some());
        assert!(vcx.debug_bounds("DiskInUseGroup-2").is_none());
        for sel in ["DiskInUseEnd-0", "DiskInUseEnd-1", "DiskInUseEnd-2"] {
            assert!(vcx.debug_bounds(sel).is_some(), "{sel}");
        }
        assert!(vcx.debug_bounds("DiskInUseRow-3").is_none());
        assert!(vcx.debug_bounds("DiskInUseNote").is_none());
        for sel in ["DiskInUseClose", "DiskInUseRetry", "DiskInUseEndAll"] {
            assert!(vcx.debug_bounds(sel).is_some(), "{sel}");
        }
    }

    #[gpui_kit::test]
    fn disk_in_use_shift_relabels(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, false);
        vcx.simulate_modifiers_change(Modifiers::shift());
        redraw(&mut vcx);
        let label = vcx.update(|_, cx| view.read(cx).row_label().clone());
        assert_eq!(label.as_ref(), "End Process");
        vcx.simulate_modifiers_change(Modifiers::none());
        redraw(&mut vcx);
        let label = vcx.update(|_, cx| view.read(cx).row_label().clone());
        assert_eq!(label.as_ref(), "End Task");
    }

    #[gpui_kit::test]
    fn disk_in_use_single_end_task_no_dialog(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEnd-0");
        r.vcx.run_until_parked();
        assert!(dialog(&mut r).is_none(), "no confirmation");
        let sent = sent(&r);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        assert_eq!(
            sent[0].targets.as_slice(),
            &[ActionTarget::Process(pkey(PIDS[0]))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_end_all_confirms(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEndAll");
        r.vcx.run_until_parked();
        assert!(sent(&r).is_empty(), "nothing before the confirmation");
        let (mut dvcx, view) = dialog(&mut r).expect("a confirmation");
        let destructive = dvcx.update(|_, cx| view.read(cx).dialog().is_destructive());
        if destructive {
            assert_eq!(
                dvcx.update(|window, cx| view.read(cx).focused_button(window)),
                Some(DialogButton::Cancel)
            );
            dvcx.simulate_keystrokes("tab");
        }
        dvcx.simulate_keystrokes("enter");
        dvcx.run_until_parked();
        r.vcx.run_until_parked();
        let sent = sent(&r);
        assert_eq!(sent.len(), 1, "one request");
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        // List order: the Files group (101, 103), then Editor (102).
        let targets: Vec<_> = [101, 103, 102]
            .iter()
            .map(|p| ActionTarget::Process(pkey(*p)))
            .collect();
        assert_eq!(sent[0].targets.to_vec(), targets);
    }

    /// End All runs through the shared process action entry point (tasklens-pdu.58): tasklens
    /// itself is dropped with the Processes tab's refusal toast, and the rest are ended after the
    /// same "End 2 selected processes?" confirmation.
    #[gpui_kit::test]
    fn disk_in_use_end_all_refuses_self_like_processes_tab(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        r.engine.publish_with(|s| {
            let p = Arc::make_mut(&mut s.processes);
            p.header.version += 1;
            let mut records = records().to_vec();
            records[1].flags = ProcFlags::IS_SELF | ProcFlags::PROTECTED;
            p.records = records.into();
        });
        draw(r.vcx);
        click(&mut vcx, "DiskInUseEndAll");
        r.vcx.run_until_parked();
        redraw(&mut vcx);
        let toasts: Vec<String> = vcx.update(|window, cx| {
            tl_ui_kit::components::toast::ToastQueue::of_window(window, cx)
                .map(|q| {
                    let texts = q.read(cx).visible_texts();
                    texts.iter().map(ToString::to_string).collect()
                })
                .unwrap_or_default()
        });
        assert!(
            toasts
                .iter()
                .any(|t| t == "Refusing to end tasklens itself."),
            "{toasts:?}"
        );
        assert!(sent(&r).is_empty(), "nothing before the confirmation");
        let (mut dvcx, view) = dialog(&mut r).expect("a confirmation");
        let text = dvcx.update(|_, cx| view.read(cx).dialog().text().to_string());
        assert_eq!(text, "End 2 selected processes?");
        if dvcx.update(|_, cx| view.read(cx).dialog().is_destructive()) {
            dvcx.simulate_keystrokes("tab");
        }
        dvcx.simulate_keystrokes("enter");
        dvcx.run_until_parked();
        r.vcx.run_until_parked();
        let sent = sent(&r);
        assert_eq!(sent.len(), 1, "one request");
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        let targets: Vec<_> = [101, 103]
            .iter()
            .map(|p| ActionTarget::Process(pkey(*p)))
            .collect();
        assert_eq!(sent[0].targets.to_vec(), targets);
    }

    #[gpui_kit::test]
    fn disk_in_use_auto_retry(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (seq, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEnd-1");
        let id = sent(&r).first().expect("End Task sent").id;
        r.engine.emit(ActionEvent {
            id,
            target: None,
            state: ActionState::Succeeded,
        });
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r), vec![seq], "no retry before 500 ms");
        r.vcx
            .executor()
            .advance_clock(RETRY_DELAY - Duration::from_millis(1));
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r).len(), 1);
        r.vcx.executor().advance_clock(Duration::from_millis(1));
        r.vcx.run_until_parked();
        let seqs = eject_seqs(&r);
        assert_eq!(seqs.len(), 2, "one retry");
        assert!(seqs[1] > seq);
        r.vcx.executor().advance_clock(Duration::from_secs(2));
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r).len(), 2, "once per action");
    }

    #[gpui_kit::test]
    fn disk_in_use_done_closes_and_toasts(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (seq, _, _) = open_blocked(&mut r, false);
        publish(&mut r, Some((seq, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "the window closed"
        );
        let toasts = r.vcx.update(|_, cx| {
            cx.try_global::<EjectToasts>()
                .map(|t| t.0.clone())
                .unwrap_or_default()
        });
        assert_eq!(
            toasts,
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_eject_again_activates(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, _, first) = open_blocked(&mut r, false);
        r.vcx.update(|window, _| window.activate_window());
        r.vcx.run_until_parked();
        let seq = eject(&mut r);
        publish(&mut r, Some((seq, blocked(false))));
        r.vcx.run_until_parked();
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_disk_in_use(IDX, cx))
            .expect("the window is still open");
        assert_eq!(view, first, "the same window, not a second one");
        let active = r
            .vcx
            .update(|_, cx| cx.active_window().map(|w| w.window_id()));
        assert_eq!(active, Some(handle.window_id()), "brought to the front");
    }

    fn eject_toasts(r: &mut Rig) -> Vec<(ToastKind, SharedString)> {
        r.vcx.update(|_, cx| {
            cx.try_global::<EjectToasts>()
                .map(|t| t.0.clone())
                .unwrap_or_default()
        })
    }

    #[gpui_kit::test]
    fn disk_in_use_eject_while_running_keeps_watch(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let seq = eject(&mut r);
        publish(&mut r, Some((seq, EjectState::Running)));
        eject(&mut r);
        assert_eq!(eject_seqs(&r), vec![seq], "nothing sent while it runs");
        publish(&mut r, Some((seq, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "no window"
        );
        assert_eq!(
            eject_toasts(&mut r),
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_retry_while_running_keeps_watch(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (first, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseRetry");
        let seqs = eject_seqs(&r);
        assert_eq!(seqs.len(), 2, "Retry after Blocked sends a new eject");
        let second = seqs[1];
        assert!(second > first);
        publish(&mut r, Some((second, EjectState::Running)));
        redraw(&mut vcx);
        click(&mut vcx, "DiskInUseRetry");
        eject(&mut r);
        assert_eq!(
            eject_seqs(&r),
            vec![first, second],
            "nothing sent while it runs"
        );
        publish(&mut r, Some((second, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "the window closed"
        );
        assert_eq!(
            eject_toasts(&mut r),
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_eacces_note(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, true);
        assert!(vcx.update(|_, cx| view.read(cx).eacces_note()));
        let note = vcx.debug_bounds("DiskInUseNote").expect("the note");
        let footer = vcx.debug_bounds("DiskInUseFooter").expect("the footer");
        let list = vcx.debug_bounds("DiskInUseList").expect("the list");
        assert!(note.top() >= list.bottom(), "below the list");
        assert!(note.bottom() <= footer.top(), "above the footer");
    }
}

/// The SMART window's smartctl fallback button (E25, PERF-051, UI-PRF-030, R-088).
mod smartctl_fallback {
    use super::*;

    use gpui_kit::Modifiers;
    use tl_model::snapshot::{SmartData, SmartKind};
    use tl_model::table::Total;
    use tl_ui::windows::smart::{SmartWindow, find_smart};
    use tl_ui_base::hub::GlobalHub;
    use tl_ui_base::i18n::tr;
    use tl_ui_kit::components::window_frame::{TestClientFrame, frame_bottom};
    use tl_ui_perf::performance::actions::PerfShowSmart;

    const SDA: &str = "disk:sda";
    const IDX: DeviceIdx = DeviceIdx(3);

    /// The page rig plus what the SMART window needs: the global hub and its action handlers.
    fn open_smart_rig(cx: &mut TestAppContext) -> Rig<'_> {
        let engine = install(cx);
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        cx.update(|cx| {
            cx.set_global(GlobalHub(hub.clone()));
            tl_ui::windows::smart::init(cx);
        });
        let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
        let host = HostContext::from_parts(Some(client), Some(hub), Some(demand), None, None);
        let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(&host, cx));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1280.), px(800.)));
        draw(vcx);
        let mut r = Rig { page, engine, vcx };
        select(&mut r, SDA);
        r
    }

    fn empty() -> SmartData {
        SmartData {
            updated_s: 0,
            health: Total {
                value: Arc::from(""),
                validity: Validity::Unsupported,
            },
            power_on_s: Total {
                value: 0,
                validity: Validity::Unsupported,
            },
            selftest: Total {
                value: Arc::from(""),
                validity: Validity::Unsupported,
            },
            ata: Arc::from([]),
            nvme: Arc::from([]),
        }
    }

    fn publish(r: &mut Rig, smart: SmartKind, data: Option<(Validity, SmartData)>) {
        r.engine.publish_with(move |s| {
            let details = Arc::make_mut(&mut s.device_details);
            details.header.version += 1;
            details.disks = Arc::from([(
                IDX,
                DiskDetails {
                    smart,
                    smart_data: data.map(|(validity, d)| Total {
                        value: Arc::new(d),
                        validity,
                    }),
                    ..DiskDetails::default()
                },
            )]);
        });
        draw(r.vcx);
    }

    fn smart_available(r: &mut Rig) -> bool {
        let page = view_of(r, SDA).downcast::<DiskPage>().expect("a DiskPage");
        r.vcx.update(|_, cx| page.read(cx).smart_available())
    }

    fn open_window(r: &mut Rig) -> (VisualTestContext, Entity<SmartWindow>) {
        r.vcx.update(|window, cx| {
            window.dispatch_action(Box::new(PerfShowSmart { key: key(SDA) }), cx)
        });
        r.vcx.run_until_parked();
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_smart(IDX, cx))
            .expect("the SMART window is open");
        let mut vcx = VisualTestContext::from_window(handle.into(), &r.vcx.cx);
        redraw(&mut vcx);
        (vcx, view)
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = vcx.debug_bounds(selector).expect(selector).center();
        vcx.simulate_click(at, Modifiers::none());
        redraw(vcx);
    }

    fn smart_commands(r: &Rig) -> Vec<DeviceCommand> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Device(
                    d @ (DeviceCommand::SmartRead { .. } | DeviceCommand::SmartReadAdmin { .. }),
                ) => Some(d),
                _ => None,
            })
            .collect()
    }

    #[gpui_kit::test]
    fn smartctl_button_shown_without_udisks_smart(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        assert!(smart_available(&mut r), "the page button shows");
        let (mut vcx, view) = open_window(&mut r);
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_some());
        assert!(
            vcx.debug_bounds("SmartRefresh").is_none(),
            "replaces Refresh"
        );
        assert!(vcx.debug_bounds("SmartTable").is_none(), "no table yet");
        assert!(vcx.debug_bounds("SmartFailure").is_none());
        let (fallback, data) = vcx.update(|_, cx| {
            let w = view.read(cx);
            (w.fallback(), w.data().is_some())
        });
        assert!(fallback && !data);
        // UDisks2 cannot read this disk, so opening sends nothing.
        assert!(smart_commands(&r).is_empty());
        let label = vcx.update(|_, cx| tr(cx, "perf-smart-smartctl"));
        assert_eq!(label.as_ref(), "Read with smartctl (admin)");
    }

    #[gpui_kit::test]
    fn smartctl_sends_command_once(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, view) = open_window(&mut r);
        click(&mut vcx, "SmartReadSmartctl");
        assert!(vcx.update(|_, cx| view.read(cx).reading_admin()), "spins");
        // Disabled while the read runs.
        click(&mut vcx, "SmartReadSmartctl");
        // Security: only the kernel name goes out; the engine builds the command line.
        let sent = smart_commands(&r);
        assert_eq!(sent.len(), 1, "{sent:?}");
        match &sent[0] {
            DeviceCommand::SmartReadAdmin { disk } => {
                assert_eq!(&**disk, "sda");
                assert!(!disk.contains('/'), "no path: {disk}");
            }
            other => panic!("expected SmartReadAdmin, got {other:?}"),
        }
        // The engine's WarmingUp step is not the answer.
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::WarmingUp, empty())),
        );
        redraw(&mut vcx);
        assert!(vcx.update(|_, cx| view.read(cx).reading_admin()));
        let mut read = empty();
        read.updated_s = 1_000;
        read.nvme = Arc::from([(Arc::<str>::from("Percent used"), Arc::<str>::from("3%"))]);
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::Valid, read)),
        );
        redraw(&mut vcx);
        assert!(!vcx.update(|_, cx| view.read(cx).reading_admin()), "stops");
        assert!(vcx.debug_bounds("SmartNvmeRow-0").is_some());
        assert_eq!(smart_commands(&r).len(), 1);
    }

    #[gpui_kit::test]
    fn smartctl_auth_cancelled_text(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, view) = open_window(&mut r);
        click(&mut vcx, "SmartReadSmartctl");
        let mut failed = empty();
        failed.health = Total {
            value: Arc::from("Authentication cancelled"),
            validity: Validity::Failed,
        };
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::Failed, failed)),
        );
        redraw(&mut vcx);
        assert!(vcx.debug_bounds("SmartFailure").is_some());
        let (reason, spinning) = vcx.update(|_, cx| {
            let w = view.read(cx);
            (w.error().cloned(), w.reading_admin())
        });
        assert_eq!(reason.as_deref(), Some("Authentication cancelled"));
        assert!(!spinning);
        let title = vcx.update(|_, cx| tr(cx, "perf-smart-failed"));
        assert_eq!(title.as_ref(), "Failed to get SMART data");
        // The button stays for another try.
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_some());
    }

    #[gpui_kit::test]
    fn smartctl_missing_hides_buttons(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        // Without smartctl the engine reports no SMART at all, never NeedsFallback.
        publish(&mut r, SmartKind::None, None);
        assert!(!smart_available(&mut r));
        assert!(
            r.vcx.debug_bounds("perf-page-toggle-0").is_none(),
            "no Show SMART data button"
        );
        let (mut vcx, _view) = open_window(&mut r);
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_none());
    }

    /// The header pairs as drawn: box, button and the four pairs, after the paint that measures
    /// the box and the one that draws its columns.
    fn header_layout(
        vcx: &mut VisualTestContext,
        button: &'static str,
    ) -> (
        gpui_kit::Bounds<gpui_kit::Pixels>,
        gpui_kit::Bounds<gpui_kit::Pixels>,
        Vec<gpui_kit::Bounds<gpui_kit::Pixels>>,
    ) {
        redraw(vcx);
        let header = vcx.debug_bounds("SmartHeaderBox").expect("header box");
        let button = vcx.debug_bounds(button).expect(button);
        let rows = ["TlKv-row-0", "TlKv-row-1", "TlKv-row-2", "TlKv-row-3"]
            .into_iter()
            .map(|s| vcx.debug_bounds(s).expect(s))
            .collect();
        (header, button, rows)
    }

    fn assert_clear(
        header: gpui_kit::Bounds<gpui_kit::Pixels>,
        button: gpui_kit::Bounds<gpui_kit::Pixels>,
        rows: &[gpui_kit::Bounds<gpui_kit::Pixels>],
    ) {
        for (i, row) in rows.iter().enumerate() {
            assert!(
                row.right() <= header.right() && row.right() < button.left(),
                "pair {i} {row:?} runs under the button {button:?} (box {header:?})"
            );
            for (j, other) in rows.iter().enumerate().skip(i + 1) {
                assert!(!row.intersects(other), "pairs {i} and {j} overlap");
            }
        }
    }

    /// Shot s1: at the default 760 px the four header pairs ran under "Read with smartctl
    /// (admin)". The grid drops to 2 columns when 4 do not fit beside the button, also inside
    /// the client frame (UI-PRF-030).
    fn smartctl_header(cx: &mut TestAppContext, frame: bool) {
        if frame {
            cx.update(|cx| cx.set_global(TestClientFrame));
        }
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, _view) = open_window(&mut r);
        // The window opens with room for a client frame; without the frame the content would get
        // that room too. Size it so the content is the default 760 x 520 either way.
        let inset = vcx.update(|window, cx| frame_bottom(window, cx));
        vcx.simulate_resize(size(px(760.) + inset * 2., px(520.) + inset * 2.));
        redraw(&mut vcx);
        let width = vcx.update(|window, _| window.viewport_size().width);
        assert_eq!(width - inset * 2., px(760.), "default content width");
        let (header, button, rows) = header_layout(&mut vcx, "SmartReadSmartctl");
        assert_clear(header, button, &rows);
        assert!(rows[2].top() > rows[0].top(), "2 columns: {rows:?}");
    }

    #[gpui_kit::test]
    fn smartctl_header_pairs_clear_the_button(cx: &mut TestAppContext) {
        smartctl_header(cx, false);
    }

    #[gpui_kit::test]
    fn smartctl_header_pairs_clear_the_button_in_the_frame(cx: &mut TestAppContext) {
        smartctl_header(cx, true);
    }

    /// With the short Refresh button the default size keeps the spec's 4 columns on one line.
    #[gpui_kit::test]
    fn refresh_header_has_four_columns(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::Nvme, None);
        let (mut vcx, _view) = open_window(&mut r);
        let (header, button, rows) = header_layout(&mut vcx, "SmartRefresh");
        assert_clear(header, button, &rows);
        assert!(
            rows.iter().all(|row| row.top() == rows[0].top()),
            "{rows:?}"
        );
    }
}
