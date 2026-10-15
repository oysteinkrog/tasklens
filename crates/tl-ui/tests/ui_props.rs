//! Process Properties window and tabs (E23, UI-PWN-003 to UI-PWN-009).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, Hsla, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::actions::SchedPolicy;
use tl_model::details::{
    ElfArch, ExeLocation, GeneralDetail, HashError, HashState, IdTriple, PackageCheck, PackageInfo,
    Relro, Seccomp, TabPayload,
};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{AppearanceSettings, Decorations, ThemeSettings};
use tl_model::snapshot::{ComponentHeader, FrameGeometry, WindowEntry, WindowsSnapshot};
use tl_model::system::CpuSet;
use tl_model::table::Total;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::components::kv_grid::{ValueLook, kv_words, value_look_in};
use tl_ui::components::window_frame::window_options;
use tl_ui::windows::frame::{PwnWindow, open_pwn, pwn_window_spec, spec};
use tl_ui::windows::props::filter::{PropsFilter, filter_bar};
use tl_ui::windows::props::general::{FieldId, GeneralTab, RESTRICTED_TOOLTIP};

const KEY: ProcessKey = ProcessKey {
    start_ticks: 77,
    boot_id: BootId(1),
    pid: 4321,
};

fn valid<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn text(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn cpus(ranges: &[(u16, u16)]) -> CpuSet {
    let mut set = CpuSet::empty();
    for &(a, b) in ranges {
        for cpu in a..=b {
            set.insert(cpu);
        }
    }
    set
}

fn detail(cmdline: &str) -> GeneralDetail {
    GeneralDetail {
        started_s: valid(1_760_000_000),
        uids: valid(IdTriple {
            real: 1000,
            effective: 0,
            saved: 0,
            fs: 0,
        }),
        uid_names: valid((text("oystein"), text("root"), text("root"))),
        policy: valid(SchedPolicy::Other),
        rt_prio: 0,
        kernel_prio: valid(120),
        autogroup: valid((text("autogroup-12"), 0)),
        affinity: valid(cpus(&[(0, 7), (16, 23)])),
        affinity_threads_differ: true,
        pie: valid(true),
        nx_stack: valid(true),
        relro: valid(Relro::Full),
        shadow_stack: Total {
            value: false,
            validity: Validity::Unsupported,
        },
        aslr_system: valid(2),
        aslr_disabled_for_process: valid(false),
        no_new_privs: valid(false),
        seccomp: valid(Seccomp::Filter { filters: 3 }),
        architecture: valid(ElfArch::X86_64),
        exe: valid(text("/usr/lib/firefox/firefox")),
        exe_deleted: false,
        location: valid(ExeLocation::System),
        package: valid(Some(PackageInfo {
            name: text("firefox"),
            version: text("131.0-1"),
            packager: Some(text("Arch Linux")),
        })),
        description: valid(text("Web browser")),
        cwd: valid(text("/home/oystein")),
        root: valid(text("/")),
        cgroup: Some(text("/user.slice/app.slice/app-firefox.scope")),
        unit: None,
        app_id: None,
        cmdline: valid(text(cmdline)),
        hashes: HashState::Pending,
        package_check: PackageCheck::NotVerified,
        handles: valid(412),
        rss_shared_bytes: valid(64 << 20),
        commit_bytes: valid(512 << 20),
        cpu_time_ms: valid(43_424_000),
        io_rchar: valid(18 << 20),
        io_wchar: valid(2 << 20),
        io_syscr: valid(12_345),
        io_syscw: valid(678),
        disk_read_bytes: valid(18 << 20),
        disk_write_bytes: valid(1 << 20),
        cancelled_write_bytes: valid(0),
        gpu_mem_shared_bytes: Total {
            value: 0,
            validity: Validity::Unsupported,
        },
        gpu_engine: valid(None),
    }
}

fn record(cpu_pct: f32) -> ProcessRecord {
    let name = text("firefox");
    ProcessRecord {
        key: KEY,
        ppid: 1200,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.clone(),
            name: name.clone(),
            exe: None,
            cmdline: None,
            user: text("oystein"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: name,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::default(),
        nice: 0,
        threads: 97,
        cpu_pct,
        rss_anon_bytes: 800 << 20,
        rss_bytes: 900 << 20,
        pss_bytes: 700 << 20,
        uss_bytes: 600 << 20,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 150_000.,
        net_tx_bps: 37_500.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn payload(d: GeneralDetail) -> Arc<TabPayload> {
    Arc::new(TabPayload::General(Arc::new(d)))
}

/// The filter bar over the tab, `width` px wide.
struct Host {
    tab: Entity<GeneralTab>,
    filter: Entity<PropsFilter>,
    width: f32,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(self.width))
            .h(px(1400.))
            .flex()
            .flex_col()
            .child(filter_bar(&self.filter, None, cx))
            .child(div().flex_1().child(self.tab.clone()))
    }
}

fn host(cx: &mut TestAppContext, width: f32) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
    });
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let filter = cx.new(|cx| PropsFilter::new("General", window, cx));
        let tab = cx.new(|cx| GeneralTab::new(KEY, filter.clone(), cx));
        Host { tab, filter, width }
    });
    draw(cx);
    (view, cx)
}

fn tab(view: &Entity<Host>, cx: &mut VisualTestContext) -> Entity<GeneralTab> {
    cx.update(|_, cx| view.read(cx).tab.clone())
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
}

fn feed(
    tab: &Entity<GeneralTab>,
    cx: &mut VisualTestContext,
    p: Option<&Arc<TabPayload>>,
    cpu: f32,
) {
    cx.update(|_, cx| {
        tab.update(cx, |t, cx| {
            t.set_online_cpus(cpus(&[(0, 31)]), cx);
            t.set_payload(p, cx);
            t.set_record(Some(&record(cpu)), 32 << 30, cx);
        })
    });
}

fn value(tab: &Entity<GeneralTab>, cx: &mut VisualTestContext, id: FieldId) -> String {
    cx.update(|_, cx| {
        tab.read(cx)
            .field(id)
            .map(|f| f.value.to_string())
            .unwrap_or_default()
    })
}

fn panels(cx: &mut VisualTestContext) -> [Bounds<Pixels>; 3] {
    [
        "GeneralTab-panel-0",
        "GeneralTab-panel-1",
        "GeneralTab-panel-2",
    ]
    .map(|s| cx.debug_bounds(s).expect("panel drawn"))
}

#[gpui_kit::test]
fn general_three_columns_at_900(cx: &mut TestAppContext) {
    // Body 900 - 2 × 12 = 876 px.
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);
    let [a, b, c] = panels(cx);
    assert_eq!(a.origin.y, b.origin.y);
    assert_eq!(b.origin.y, c.origin.y);
    assert!(a.right() <= b.origin.x && b.right() <= c.origin.x);
    // Equal widths, up to pixel rounding.
    assert!((a.size.width - b.size.width).abs() <= px(1.));
}

#[gpui_kit::test]
fn general_single_column_below_840(cx: &mut TestAppContext) {
    // Body 824 - 2 × 12 = 800 px.
    let (view, cx) = host(cx, 824.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);
    let [a, b, c] = panels(cx);
    assert_eq!(a.origin.x, b.origin.x);
    assert_eq!(b.origin.x, c.origin.x);
    assert!(a.bottom() <= b.origin.y && b.bottom() <= c.origin.y);
}

#[gpui_kit::test]
fn general_loading_shows_collecting_and_pid(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, None, 1.);
    assert_eq!(value(&t, cx, FieldId::Package), "Collecting…");
    assert_eq!(value(&t, cx, FieldId::Pid), "4321");
    assert_eq!(value(&t, cx, FieldId::Threads), "97");
}

#[gpui_kit::test]
fn general_field_values(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    assert_eq!(
        value(&t, cx, FieldId::Priority),
        "Nice 0, SCHED_OTHER, kernel prio 120"
    );
    assert_eq!(
        value(&t, cx, FieldId::Affinity),
        "0-7,16-23 (16 CPUs) (threads differ)"
    );
    assert_eq!(
        value(&t, cx, FieldId::User),
        "oystein (1000), euid root (0), suid root (0)"
    );
    assert_eq!(value(&t, cx, FieldId::Seccomp), "Filter (3 filters)");
    assert_eq!(value(&t, cx, FieldId::Package), "firefox 131.0-1");
    assert_eq!(
        value(&t, cx, FieldId::Hardening),
        "PIE, NX stack, Full RELRO, Seccomp"
    );
    assert_eq!(value(&t, cx, FieldId::ShadowStack), "Not supported");
    assert_eq!(
        value(&t, cx, FieldId::NetIo),
        "Receive 1.2 Mbps, send 0.3 Mbps"
    );
    assert_eq!(value(&t, cx, FieldId::CpuTime), "12:03:44");
    // Root "/" and the absent unit and app id are hidden.
    let hidden = cx.update(|_, cx| {
        let t = t.read(cx);
        [FieldId::Root, FieldId::Unit, FieldId::AppId].map(|id| t.field(id).is_none())
    });
    assert_eq!(hidden, [true; 3]);
}

/// The grid row of `label` in column 2.
fn col2_row(
    tab: &Entity<GeneralTab>,
    cx: &mut VisualTestContext,
    label: &str,
) -> tl_ui::components::kv_grid::TlKvRow {
    cx.update(|_, cx| {
        tab.read(cx)
            .rows(1)
            .into_iter()
            .find(|r| r.label.as_ref() == label)
            .expect("row shown")
    })
}

fn sha256_3f9a() -> [u8; 32] {
    let mut sha = [0u8; 32];
    sha[..8].copy_from_slice(&[0x3f, 0x9a, 0x0c, 0x41, 0xe2, 0xb7, 0x7d, 0x09]);
    for (i, b) in sha.iter_mut().enumerate().skip(8) {
        *b = i as u8;
    }
    sha
}

#[gpui_kit::test]
fn hash_pending_then_done_text(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    assert_eq!(value(&t, cx, FieldId::Sha256), "Computing…");
    assert_eq!(value(&t, cx, FieldId::Md5), "Computing…");
    let row = col2_row(&t, cx, "SHA-256:");
    assert_eq!(row.value.validity, Validity::Stale, "text_secondary");
    assert_eq!(row.value.tooltip, None);

    let mut d = detail("firefox");
    d.hashes = HashState::Done {
        sha256: sha256_3f9a(),
        md5: [0xa0; 16],
    };
    feed(&t, cx, Some(&payload(d)), 1.);
    assert_eq!(value(&t, cx, FieldId::Sha256), "3f9a0c41e2b77d09…");
    assert_eq!(value(&t, cx, FieldId::Md5), "a0a0a0a0a0a0a0a0…");
    let row = col2_row(&t, cx, "SHA-256:");
    let full = row.value.tooltip.expect("full hash tooltip").to_string();
    assert_eq!(full.len(), 64);
    assert!(full.starts_with("3f9a0c41e2b77d09"), "{full}");
    assert_eq!(full, full.to_lowercase());
    assert_eq!(row.value.validity, Validity::Valid);
    let md5 = col2_row(&t, cx, "MD5:").value.tooltip.expect("md5 tooltip");
    assert_eq!(md5.as_ref(), "a0".repeat(16));
    let field_full = cx.update(|_, cx| {
        t.read(cx)
            .field(FieldId::Sha256)
            .and_then(|f| f.full.clone())
    });
    assert_eq!(field_full.as_deref(), Some(full.as_str()), "copy form");
}

#[gpui_kit::test]
fn hash_failed_tooltip_text(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    let mut d = detail("firefox");
    d.hashes = HashState::Failed(HashError::ExePathUnavailable);
    feed(&t, cx, Some(&payload(d)), 1.);
    for (id, label) in [(FieldId::Sha256, "SHA-256:"), (FieldId::Md5, "MD5:")] {
        assert_eq!(value(&t, cx, id), "Unavailable");
        let row = col2_row(&t, cx, label);
        assert_eq!(row.value.validity, Validity::Failed);
        assert_eq!(
            row.value.tooltip.as_deref(),
            Some("Executable path unavailable")
        );
    }

    let mut d = detail("firefox");
    d.hashes = HashState::Failed(HashError::ReadFailed { errno: 13 });
    feed(&t, cx, Some(&payload(d)), 1.);
    assert_eq!(value(&t, cx, FieldId::Md5), "Unavailable");
    let tip = col2_row(&t, cx, "SHA-256:")
        .value
        .tooltip
        .expect("tooltip")
        .to_string();
    let (first, second) = tip.split_once('\n').expect("two lines");
    assert_eq!(first, "Failed to compute one or more hashes");
    assert!(second.contains("Permission denied"), "{second}");
}

#[gpui_kit::test]
fn verify_button_hidden_when_not_packaged(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);
    assert_eq!(value(&t, cx, FieldId::PackageStatus), "Not verified");
    let button = cx
        .update(|_, cx| t.read(cx).verify_button())
        .expect("shown");
    assert_eq!(button.label.as_ref(), "Verify against package");
    assert!(!button.disabled && !button.loading);
    let status = cx
        .debug_bounds("GeneralTab-verify-row")
        .expect("verify row");
    assert_eq!(status.size.height, px(26.));
    let button_h = cx
        .debug_bounds("TlButton-general-verify")
        .expect("button drawn")
        .size
        .height;
    assert_eq!(button_h, px(24.));

    let mut d = detail("firefox");
    d.package_check = PackageCheck::NotPackaged;
    feed(&t, cx, Some(&payload(d)), 1.);
    draw(cx);
    assert_eq!(value(&t, cx, FieldId::PackageStatus), "Not packaged");
    assert!(cx.update(|_, cx| t.read(cx).verify_button()).is_none());
    assert!(cx.debug_bounds("GeneralTab-verify-row").is_none(), "hidden");
    assert!(cx.debug_bounds("TlButton-general-verify").is_none());

    let mut d = detail("firefox");
    d.package_check = PackageCheck::NoChecksum;
    feed(&t, cx, Some(&payload(d)), 1.);
    assert_eq!(
        value(&t, cx, FieldId::PackageStatus),
        "No checksum in package database"
    );
    let button = cx
        .update(|_, cx| t.read(cx).verify_button())
        .expect("shown");
    assert!(button.disabled);
    assert_eq!(
        button.tooltip.as_deref(),
        Some("No checksum in package database")
    );

    cx.update(|_, cx| t.update(cx, |t, cx| t.set_exited(true, cx)));
    let button = cx
        .update(|_, cx| t.read(cx).verify_button())
        .expect("shown");
    assert!(button.disabled);
    assert_eq!(
        button.tooltip.as_deref(),
        Some("The process is not running.")
    );
}

fn window_entry(n: u32, caption: &str, g: (i32, i32, i32, i32)) -> WindowEntry {
    WindowEntry {
        internal_id: text(&format!("{{0000000{n}-aaaa-bbbb-cccc-dddddddddddd}}")),
        pid: KEY.pid,
        caption: text(caption),
        resource_class: text("firefox"),
        desktop_file_name: text("firefox"),
        minimized: false,
        maximized: n == 1,
        keep_above: false,
        frame_geometry: FrameGeometry {
            x: g.0,
            y: g.1,
            width: g.2,
            height: g.3,
        },
        window_type: 0,
    }
}

/// `Snapshot.windows` with `entries` for this pid and one window of another pid on each side.
fn windows_snap(validity: Validity, entries: Vec<WindowEntry>) -> WindowsSnapshot {
    let other = |pid| WindowEntry {
        pid,
        ..window_entry(9, "Other", (0, 0, 10, 10))
    };
    let mut all = vec![other(KEY.pid - 1)];
    all.extend(entries);
    all.push(other(KEY.pid + 1));
    WindowsSnapshot {
        header: ComponentHeader {
            validity,
            ..ComponentHeader::default()
        },
        windows: Arc::from(all),
        pick: None,
    }
}

fn feed_windows(tab: &Entity<GeneralTab>, cx: &mut VisualTestContext, w: &WindowsSnapshot) {
    cx.update(|_, cx| tab.update(cx, |t, cx| t.set_windows(w, cx)));
}

/// The value and tooltip of the column 3 row labelled `label`.
fn col3_row(
    tab: &Entity<GeneralTab>,
    cx: &mut VisualTestContext,
    label: &str,
) -> (String, Option<String>) {
    cx.update(|_, cx| {
        tab.read(cx)
            .rows(2)
            .into_iter()
            .find(|r| r.label.as_ref() == label)
            .map(|r| {
                (
                    r.value.text.to_string(),
                    r.value.tooltip.map(|t| t.to_string()),
                )
            })
            .expect("row shown")
    })
}

#[gpui_kit::test]
fn window_block_none_text(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    // Before the first windows component: collecting.
    assert_eq!(value(&t, cx, FieldId::Window), "Collecting…");

    feed_windows(&t, cx, &windows_snap(Validity::WarmingUp, Vec::new()));
    assert_eq!(value(&t, cx, FieldId::Window), "Collecting…");

    feed_windows(&t, cx, &windows_snap(Validity::Valid, Vec::new()));
    draw(cx);
    assert_eq!(value(&t, cx, FieldId::Window), "No top-level window");
    assert_eq!(value(&t, cx, FieldId::WindowTitle), "", "no six fields");
    assert!(cx.debug_bounds("GeneralTab-window-divider").is_some());

    // Frozen once exited.
    cx.update(|_, cx| t.update(cx, |t, cx| t.set_exited(true, cx)));
    let one = vec![window_entry(1, "Mozilla Firefox", (0, 32, 1280, 800))];
    feed_windows(&t, cx, &windows_snap(Validity::Valid, one));
    assert_eq!(value(&t, cx, FieldId::Window), "No top-level window");
}

#[gpui_kit::test]
fn window_block_and_n_more(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    let entries = vec![
        window_entry(1, "Mozilla Firefox\u{202E}", (0, 32, 1280, 800)),
        window_entry(2, "Library", (100, 100, 600, 400)),
        window_entry(3, "Downloads", (200, 200, 500, 300)),
    ];
    let snap = windows_snap(Validity::Valid, entries);
    feed_windows(&t, cx, &snap);
    let formats = cx.update(|_, cx| t.read(cx).format_count());
    assert_eq!(
        value(&t, cx, FieldId::WindowTitle),
        "Mozilla Firefox\\u{202E}",
        "escaped"
    );
    assert_eq!(
        value(&t, cx, FieldId::WindowId),
        "{00000001-aaaa-bbbb-cccc-dddddddddddd}"
    );
    assert_eq!(value(&t, cx, FieldId::WindowState), "Maximized");
    assert_eq!(value(&t, cx, FieldId::WindowClass), "firefox");
    assert_eq!(value(&t, cx, FieldId::WindowRect), "L:0 T:32 R:1280 B:832");
    assert_eq!(value(&t, cx, FieldId::WindowSize), "1280 x 800");
    assert_eq!(value(&t, cx, FieldId::Window), "", "no Window: row");
    let (more, tip) = col3_row(&t, cx, "");
    assert_eq!(more, "And 2 more");
    assert_eq!(tip.as_deref(), Some("Library\nDownloads"));

    // The same entries again and a new sweep re-format no window value.
    feed_windows(&t, cx, &snap);
    feed(&t, cx, Some(&payload(detail("firefox"))), 2.);
    let window_ids = [
        FieldId::WindowTitle,
        FieldId::WindowId,
        FieldId::WindowState,
        FieldId::WindowClass,
        FieldId::WindowRect,
        FieldId::WindowSize,
    ];
    let texts: Vec<String> = window_ids.iter().map(|id| value(&t, cx, *id)).collect();
    let delta = cx.update(|_, cx| t.read(cx).format_count()) - formats;
    assert!(delta <= 25, "{delta}");
    assert_eq!(texts[0], "Mozilla Firefox\\u{202E}");
}

#[gpui_kit::test]
fn window_block_unsupported_without_bridge(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    feed_windows(&t, cx, &windows_snap(Validity::Unsupported, Vec::new()));
    let (text, tip) = col3_row(&t, cx, "Window:");
    assert_eq!(text, "Not supported");
    assert_eq!(tip.as_deref(), Some("Needs the KDE window bridge."));
}

#[gpui_kit::test]
fn general_restricted_word_and_tooltip(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    let mut d = detail("firefox");
    d.exe = Total {
        value: text(""),
        validity: Validity::Restricted,
    };
    feed(&t, cx, Some(&payload(d)), 1.);
    draw(cx);
    assert_eq!(value(&t, cx, FieldId::Executable), "Restricted");
    let (look, tooltip, secondary) = cx.update(|_, cx| {
        let words = kv_words(cx);
        let row = t
            .read(cx)
            .rows(0)
            .into_iter()
            .find(|r| r.label.as_ref() == "Executable:")
            .expect("Executable row");
        let theme = tl_ui::theme::current(cx);
        (
            value_look_in(&row.value, &words, theme).expect("drawn"),
            row.value.tooltip.clone(),
            theme.text_secondary,
        )
    });
    assert_eq!(look.text.as_ref(), "Restricted");
    assert_eq!(look.color, secondary);
    assert_eq!(tooltip.as_deref(), Some(RESTRICTED_TOOLTIP));
    assert_eq!(RESTRICTED_TOOLTIP, "Needs administrator access.");
}

fn exe_and_pid_looks(
    t: &Entity<GeneralTab>,
    cx: &mut VisualTestContext,
) -> (ValueLook, ValueLook, Hsla, Hsla) {
    cx.update(|_, cx| {
        let rows: Vec<_> = (0..3).flat_map(|i| t.read(cx).rows(i)).collect();
        let words = kv_words(cx);
        let theme = tl_ui::theme::current(cx);
        let look = |label: &str| {
            let row = rows
                .iter()
                .find(|r| r.label.as_ref() == label)
                .expect("row");
            value_look_in(&row.value, &words, theme).expect("drawn")
        };
        (
            look("Executable:"),
            look("PID:"),
            theme.status.warning,
            theme.text_primary,
        )
    })
}

#[gpui_kit::test]
fn general_deleted_exe_in_warning_colour(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    let mut d = detail("firefox");
    d.exe = valid(text("/usr/lib/firefox/firefox (deleted)"));
    d.exe_deleted = true;
    feed(&t, cx, Some(&payload(d)), 1.);
    draw(cx);
    let (exe, pid, warning, primary) = exe_and_pid_looks(&t, cx);
    assert_eq!(exe.text.as_ref(), "/usr/lib/firefox/firefox (deleted)");
    assert_eq!(exe.color, warning);
    assert_eq!(pid.color, primary);

    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);
    let (exe, _, _, primary) = exe_and_pid_looks(&t, cx);
    assert_eq!(exe.color, primary);
}

#[gpui_kit::test]
fn general_cmdline_not_reshaped_on_sweep(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    let cmd = "/usr/lib/firefox/firefox -contentproc -childID 12 -isForBrowser";
    feed(&t, cx, Some(&payload(detail(cmd))), 1.);
    draw(cx);
    let shapes =
        |cx: &mut VisualTestContext| cx.update(|_, cx| t.read(cx).cmdline().read(cx).shape_count());
    let before = shapes(cx);
    assert_eq!(before, 1);
    for sweep in 0..10 {
        let formats = cx.update(|_, cx| t.read(cx).format_count());
        // A new payload Arc with the same command line, and new live numbers.
        let mut d = detail(cmd);
        d.cpu_time_ms = valid(43_424_000 + sweep * 1000);
        feed(&t, cx, Some(&payload(d)), 2. + sweep as f32);
        draw(cx);
        let delta = cx.update(|_, cx| t.read(cx).format_count()) - formats;
        assert!(delta <= 25, "sweep {sweep} re-formatted {delta} values");
    }
    assert_eq!(shapes(cx), before);
    let shown = cx.update(|_, cx| t.read(cx).cmdline().read(cx).text().to_string());
    assert_eq!(shown, cmd);
}

#[gpui_kit::test]
fn props_filter_hides_general_fields(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    let filter = cx.update(|_, cx| view.read(cx).filter.clone());
    cx.update(|window, cx| {
        let input = filter.read(cx).input.clone();
        input.update(cx, |s, cx| s.insert("pid".to_owned(), window, cx));
    });
    cx.executor().advance_clock(Duration::from_millis(100));
    draw(cx);
    let visible = cx.update(|_, cx| t.read(cx).visible_ids());
    assert_eq!(visible, vec![FieldId::Pid, FieldId::Ppid]);
    // The panels stay, even when empty.
    assert_eq!(panels(cx).len(), 3);
}

/// The Unsupported words come from the catalogue and are kept by variant, not by text (R-132).
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn general_unsupported_words_follow_test_locale(cx: &mut TestAppContext) {
    use tl_ui::components::kv_grid::UnsupportedWord;
    use tl_ui::i18n::{set_test_catalogue, tr_calls};

    const MARKERS: &str = "
fnd-kv-not-supported = <not-supported>
fnd-kv-not-reported = <not-reported>
";
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
        set_test_catalogue(cx, MARKERS);
    });
    // The tab alone: the frame check below is for the tab, not the filter bar.
    let (t, cx) = cx.add_window_view(|window, cx| {
        let filter = cx.new(|cx| PropsFilter::new("General", window, cx));
        GeneralTab::new(KEY, filter, cx)
    });
    let mut d = detail("firefox");
    d.cgroup = None;
    d.cmdline = Total {
        value: text(""),
        validity: Validity::Unsupported,
    };
    feed(&t, cx, Some(&payload(d)), 1.);
    draw(cx);

    assert_eq!(value(&t, cx, FieldId::ShadowStack), "<not-supported>");
    assert_eq!(value(&t, cx, FieldId::Cgroup), "<not-reported>");
    let (cgroup, cmdline) = cx.update(|_, cx| {
        let words = kv_words(cx);
        let theme = tl_ui::theme::current(cx);
        let row = (0..3)
            .flat_map(|i| t.read(cx).rows(i))
            .find(|r| r.label.as_ref() == "Cgroup:")
            .expect("Cgroup row");
        let look = value_look_in(&row.value, &words, theme).expect("drawn");
        (
            (row.value.word, look.text.to_string()),
            t.read(cx).cmdline().read(cx).text().to_string(),
        )
    });
    assert_eq!(
        cgroup,
        (
            Some(UnsupportedWord::NotReported),
            "<not-reported>".to_owned()
        )
    );
    assert_eq!(cmdline, "<not-reported>");

    let before = tr_calls();
    for _ in 0..60 {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }
    assert_eq!(tr_calls(), before, "60 frames make no tr call");
}

/// Field labels and value words come from the catalogue (R-132): a swapped key shows in the tab,
/// in the copy text, and the formatted values use the swapped words.
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn general_labels_and_words_follow_test_locale(cx: &mut TestAppContext) {
    use tl_ui::i18n::set_test_catalogue;

    const MARKERS: &str = "
pwn-general-label-cgroup = <cgroup>:
pwn-general-yes = <yes>
pwn-general-seccomp-filter-many = <filter { $n }>
";
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
        set_test_catalogue(cx, MARKERS);
    });
    let (t, cx) = cx.add_window_view(|window, cx| {
        let filter = cx.new(|cx| PropsFilter::new("General", window, cx));
        GeneralTab::new(KEY, filter, cx)
    });
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);

    assert_eq!(value(&t, cx, FieldId::Pie), "<yes>");
    assert_eq!(value(&t, cx, FieldId::Seccomp), "<filter 3>");
    let labels: Vec<String> = cx.update(|_, cx| {
        (0..3)
            .flat_map(|i| t.read(cx).rows(i))
            .map(|r| r.label.to_string())
            .collect()
    });
    assert!(labels.iter().any(|l| l == "<cgroup>:"), "{labels:?}");
    assert!(
        labels.iter().any(|l| l == "Process:"),
        "other keys keep their en-US text"
    );
}

/// The filter bar resolves its words once per strings version: the General tab with the bar makes
/// no tr call over 60 frames, and a new catalogue reaches the placeholder and tooltip (R-132).
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn props_filter_bar_words_follow_test_locale(cx: &mut TestAppContext) {
    use tl_ui::i18n::{set_test_catalogue, tr_calls};

    const MARKERS: &str = "
pwn-props-filter-placeholder = <filter-placeholder>
pwn-props-filter-icon-tooltip = <filter-tooltip>
pwn-props-filter-name = <filter { $tab }>
";
    let (view, cx) = host(cx, 900.);
    let t = tab(&view, cx);
    feed(&t, cx, Some(&payload(detail("firefox"))), 1.);
    draw(cx);
    let filter = cx.update(|_, cx| view.read(cx).filter.clone());
    let frames = |cx: &mut VisualTestContext| {
        let before = tr_calls();
        for _ in 0..60 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        tr_calls() - before
    };
    assert!(cx.debug_bounds("PropsFilter-bar").is_some(), "bar drawn");
    assert_eq!(frames(cx), 0, "60 frames make no tr call");

    cx.update(|_, cx| set_test_catalogue(cx, MARKERS));
    draw(cx);
    let words = cx.update(|_, cx| {
        let f = filter.read(cx);
        (
            f.input.read(cx).presentation().placeholder().to_string(),
            f.icon_tooltip().to_string(),
            f.accessible_name().to_string(),
        )
    });
    assert_eq!(
        words,
        (
            "<filter-placeholder>".to_owned(),
            "<filter-tooltip>".to_owned(),
            "<filter General>".to_owned()
        )
    );
    assert_eq!(frames(cx), 0, "60 frames after the switch make no tr call");
}

struct Dummy;

impl Render for Dummy {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn frame_setup(cx: &mut TestAppContext, ui_scale_pct: u16) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings {
                ui_scale_pct,
                ..AppearanceSettings::default()
            },
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
    });
}

/// The opened window's size, and the minimum `open_pwn` asks for.
fn open_sizes(
    cx: &mut TestAppContext,
    w: PwnWindow,
) -> (gpui_kit::Size<Pixels>, gpui_kit::Size<Pixels>) {
    let title: gpui_kit::SharedString = format!("{w:?}").into();
    let min = cx.update(|cx| {
        let spec = pwn_window_spec(cx, w, title.clone(), None);
        window_options(&spec, Decorations::Client, cx).window_min_size
    });
    let handle = cx
        .update(|cx| open_pwn(cx, w, title, None, |_, cx| cx.new(|_| Dummy)))
        .expect("window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let opened = vcx.update(|window, _| window.bounds().size);
    (opened, min.expect("minimum size set"))
}

#[gpui_kit::test]
fn pwn_window_sizes_match_table(cx: &mut TestAppContext) {
    frame_setup(cx, 100);
    let table = [
        (PwnWindow::ProcessProperties, (900., 640.), (640., 400.)),
        (PwnWindow::AppProperties, (720., 560.), (640., 400.)),
        (PwnWindow::SetAffinity, (640., 360.), (640., 360.)),
        (PwnWindow::SetPriority, (440., 120.), (440., 120.)),
        (PwnWindow::RealtimeSignal, (360., 120.), (360., 120.)),
        (PwnWindow::Run, (480., 248.), (480., 248.)),
        (PwnWindow::HandleSearch, (860., 560.), (560., 360.)),
    ];
    for (w, default, min) in table {
        let (opened, min_asked) = open_sizes(cx, w);
        assert_eq!(opened, size(px(default.0), px(default.1)), "{w:?} default");
        assert_eq!(min_asked, size(px(min.0), px(min.1)), "{w:?} minimum");
        assert_eq!(spec(w).default_size, opened, "{w:?} spec");
    }
}

#[gpui_kit::test]
fn pwn_props_size_scales_with_ui_scale(cx: &mut TestAppContext) {
    frame_setup(cx, 125);
    let (opened, min) = open_sizes(cx, PwnWindow::ProcessProperties);
    assert_eq!(opened, size(px(1125.), px(800.)));
    assert_eq!(min, size(px(800.), px(500.)));
}

#[gpui_kit::test]
fn run_size_scales_with_ui_scale(cx: &mut TestAppContext) {
    frame_setup(cx, 125);
    let (opened, min) = open_sizes(cx, PwnWindow::Run);
    assert_eq!(opened, size(px(600.), px(310.)));
    assert_eq!(min, size(px(600.), px(310.)));
}

// ---- App Properties General (E23-026, UI-PWN-019) ----

mod app_props {
    use tl_model::details::{AppGeneralDetail, AppKind};
    use tl_ui::windows::app_props::AppGeneralTab;

    use super::*;

    struct AppHost {
        tab: Entity<AppGeneralTab>,
        filter: Entity<PropsFilter>,
        width: f32,
    }

    impl Render for AppHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(560.))
                .flex()
                .flex_col()
                .child(filter_bar(&self.filter, None, cx))
                .child(div().flex_1().child(self.tab.clone()))
        }
    }

    fn app_detail(swap: u64) -> AppGeneralDetail {
        AppGeneralDetail {
            name: text("Firefox"),
            kind: AppKind::App,
            icon_path: None,
            app_id: Some(text("org.mozilla.firefox")),
            desktop_file: Some(text("/usr/share/applications/firefox.desktop")),
            unit: None,
            cgroup: Some(text("/user.slice/app-firefox.scope")),
            processes: 34,
            memory_anon_bytes: valid(1_310_720_000),
            swap_bytes: valid(swap),
            disk_read_bps: valid(0.4 * 1024. * 1024.),
            disk_write_bps: valid(0.1 * 1024. * 1024.),
            primary_cmdline: valid(text("/usr/lib/firefox/firefox")),
        }
    }

    fn open(cx: &mut TestAppContext, width: f32) -> (Entity<AppHost>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let (host, cx) = cx.add_window_view(move |window, cx| {
            let filter = cx.new(|cx| PropsFilter::new("General", window, cx));
            let tab = cx.new(|cx| AppGeneralTab::new(GroupKey(77), filter.clone(), cx));
            AppHost { tab, filter, width }
        });
        draw(cx);
        (host, cx)
    }

    #[gpui_kit::test]
    fn app_props_swap_updates_live(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, 700.);
        let tab = cx.update(|_, cx| host.read(cx).tab.clone());
        for (bytes, expected) in [(0, "0 B"), (12 * 1024 * 1024, "12.00 MiB")] {
            let payload = Arc::new(TabPayload::AppGeneral(Arc::new(app_detail(bytes))));
            cx.update(|_, cx| tab.update(cx, |tab, cx| tab.set_payload(Some(&payload), cx)));
            assert_eq!(cx.update(|_, cx| tab.read(cx).swap_text()), expected);
        }
    }

    #[gpui_kit::test]
    fn app_props_two_panels_breakpoint(cx: &mut TestAppContext) {
        let (wide, cx) = open(cx, 700.);
        draw(cx);
        assert!(cx.update(|_, cx| wide.read(cx).tab.read(cx).panels_are_side_by_side()));
        cx.update(|_, cx| {
            wide.update(cx, |host, cx| {
                host.width = 600.;
                cx.notify();
            })
        });
        draw(cx);
        assert!(!cx.update(|_, cx| wide.read(cx).tab.read(cx).panels_are_side_by_side()));
    }
}

/// The Process Properties window over the real app globals and main window (E23-A1, UI-PWN-002
/// to UI-PWN-004, UI-PWN-018). Needs `--features test-hooks` for `AppEnv::test`.
#[cfg(feature = "test-hooks")]
mod window {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{AnyWindowHandle, WindowHandle};
    use tl_model::actions::ActionKind;
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::demand::DetailDemand;
    use tl_model::details::{DetailTab, DetailTarget, TargetDetail};
    use tl_model::ids::{ColumnId, NodeId};
    use tl_model::process::{GroupKind, GroupRow, GroupRowKind};
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::ProcessSnapshot;
    use tl_model::table::{ProcessTableView, TableRow};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::TlMenu;
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::demand::GlobalDemand;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::actions::{CapturedTarget, CapturedTargets};
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::menu::{MenuCaps, row_menu};
    use tl_ui::table::tree_table::RowKind;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
    use tl_ui::windows::app_props::{
        APP_EXITED_BANNER, AppProcsGoTo, AppPropsWindow, open_app_properties,
    };
    use tl_ui::windows::props::{
        ACCESS_ENV, ACCESS_FDS, ACCESS_MAPS, ACCESS_SECURITY, ACCESS_STRINGS, PropsWindow,
        open_process_properties, props_words,
    };

    use super::*;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey { pid, ..KEY }
    }

    fn rec(pid: u32, flags: ProcFlags) -> ProcessRecord {
        let mut r = record(1.0);
        r.key = key(pid);
        r.flags = flags;
        r
    }

    /// App globals and the main window on Processes.
    fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let init = UiInit {
            settings: Settings::default(),
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        (engine, main)
    }

    /// Publishes `records` (any order) as the process list.
    fn publish_records(
        cx: &mut TestAppContext,
        engine: &FakeEngine,
        mut records: Vec<ProcessRecord>,
    ) {
        records.sort_by_key(|r| r.key);
        engine.publish_with(|s| {
            let p = Arc::make_mut(&mut s.processes);
            p.records = Arc::from(records);
            p.header.version += 1;
        });
        cx.run_until_parked();
    }

    fn app_group(name: &str, kind: GroupRowKind, members: u32) -> GroupRow {
        let class = GroupKind::App {
            app_id: Arc::from(name.to_lowercase()),
        };
        let key = class.key();
        GroupRow {
            node: NodeId::Group(key),
            key,
            kind,
            class,
            name: Arc::from(name),
            icon: None,
            members,
            pids: members,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 1.0,
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
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn publish_groups(cx: &mut TestAppContext, engine: &FakeEngine, groups: Vec<GroupRow>) {
        engine.publish_with(|s| {
            let table = Arc::make_mut(&mut s.table);
            table.groups = Arc::from(groups);
            table.header.version += 1;
        });
        cx.run_until_parked();
    }

    fn processes_page(main: &mut VisualTestContext) -> Entity<ProcessesPage> {
        main.update(|_, cx| {
            main_window_view(cx)
                .and_then(|view| view.read(cx).page_entity::<ProcessesPage>(Tab::Processes))
                .expect("the Processes page is built")
        })
    }

    fn table_row(node: NodeId, record: Option<u32>, group: Option<u32>) -> TableRow {
        TableRow {
            node,
            depth: 0,
            expanded: false,
            has_children: false,
            record,
            group,
            descendants: 0,
            subtree: None,
        }
    }

    fn publish_table(
        engine: &FakeEngine,
        main: &mut VisualTestContext,
        records: Vec<ProcessRecord>,
        groups: Vec<GroupRow>,
        rows: Vec<TableRow>,
    ) {
        let page = processes_page(main);
        let generation =
            main.update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
        engine.publish_with(|snapshot| {
            snapshot.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                records: records.into(),
                ..ProcessSnapshot::default()
            });
            snapshot.table = Arc::new(ProcessTableView {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                query_generation: generation,
                rows: rows.into(),
                groups: groups.into(),
                ..ProcessTableView::default()
            });
        });
        main.run_until_parked();
    }

    fn select_and_focus(
        main: &mut VisualTestContext,
        page: &Entity<ProcessesPage>,
        ids: &[ProcRowId],
    ) {
        let table = main.update(|_, cx| page.read(cx).table().clone());
        main.update(|_, cx| {
            table.update(cx, |table, cx| {
                for (index, id) in ids.iter().enumerate() {
                    if index == 0 {
                        table.selection.click(*id);
                    } else {
                        table.selection.ctrl_click(*id);
                    }
                }
                cx.notify();
            })
        });
        let page = page.clone();
        main.update(|window, cx| page.update(cx, |page, cx| page.focus_table(window, cx)));
        draw(main);
    }

    #[gpui_kit::test]
    fn enter_on_process_row_opens_properties(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        publish_table(
            &engine,
            &mut main,
            vec![rec(101, ProcFlags::empty())],
            Vec::new(),
            vec![table_row(NodeId::Process(key(101)), Some(0), None)],
        );
        let page = processes_page(&mut main);
        select_and_focus(&mut main, &page, &[ProcRowId::Process(key(101))]);

        main.simulate_keystrokes("enter");
        main.run_until_parked();

        assert!(cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Properties(key(101)))
                .is_some()
        }));
    }

    #[gpui_kit::test]
    fn ctrl_i_on_app_row_opens_app_properties(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let group = app_group("Firefox", GroupRowKind::App, 1);
        let mut process = rec(101, ProcFlags::empty());
        process.group = group.key;
        publish_table(
            &engine,
            &mut main,
            vec![process],
            vec![group.clone()],
            vec![table_row(group.node.clone(), None, Some(0))],
        );
        let page = processes_page(&mut main);
        select_and_focus(&mut main, &page, &[ProcRowId::Group(group.key)]);

        main.simulate_keystrokes("ctrl-i");
        main.run_until_parked();

        assert!(cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::AppProperties(group.key))
                .is_some()
        }));
    }

    #[gpui_kit::test]
    fn multi_select_opens_one_window_each(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let pids = [101, 102, 103];
        publish_table(
            &engine,
            &mut main,
            pids.iter()
                .map(|&pid| rec(pid, ProcFlags::empty()))
                .collect(),
            Vec::new(),
            pids.iter()
                .enumerate()
                .map(|(index, &pid)| table_row(NodeId::Process(key(pid)), Some(index as u32), None))
                .collect(),
        );
        let page = processes_page(&mut main);
        let ids: Vec<_> = pids
            .iter()
            .map(|&pid| ProcRowId::Process(key(pid)))
            .collect();
        select_and_focus(&mut main, &page, &ids);

        main.simulate_keystrokes("alt-enter");
        main.run_until_parked();

        assert_eq!(props_count(cx), 3);
        for pid in pids {
            assert!(cx.update(|cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .get(&WindowKey::Properties(key(pid)))
                    .is_some()
            }));
        }
    }

    #[gpui_kit::test]
    fn row_menu_properties_item_states(cx: &mut TestAppContext) {
        let (_engine, mut main) = setup(cx);
        let page = processes_page(&mut main);
        let target = |flags| CapturedTarget {
            node: NodeId::Process(key(101)),
            kind: RowKind::Process,
            name: "process".into(),
            pid: Some(101),
            uid: 1000,
            user: "user".into(),
            flags,
            member_count: 1,
            stopped: false,
            ppid: 1,
            nice: Some(0),
            exe_deleted: false,
            unit: None,
        };
        let build = |targets: CapturedTargets, main: &mut VisualTestContext| {
            let focus = main.update(|_, cx| page.read(cx).table().read(cx).focus_handle().clone());
            main.update(|window, cx| {
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    row_menu(menu, &targets, &MenuCaps::default(), window, cx)
                })
            })
        };

        let live = build(
            CapturedTargets::new([target(ProcFlags::empty())], Some(ColumnId::Name)),
            &mut main,
        );
        let live_entries = main.update(|window, cx| live.read(cx).entries_for_test(window));
        let item = live_entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Properties"))
            .expect("Properties item");
        assert_eq!(item.kind, MenuEntryKind::Item);

        let exited = build(
            CapturedTargets::new([target(ProcFlags::EXITED)], Some(ColumnId::Name)),
            &mut main,
        );
        let exited_entries = main.update(|window, cx| exited.read(cx).entries_for_test(window));
        let item = exited_entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Properties"))
            .expect("disabled Properties item");
        assert_eq!(item.kind, MenuEntryKind::Disabled);
        assert_eq!(item.tooltip.as_deref(), Some("Process exited"));
    }

    /// Edits the detail of `pid` and publishes it.
    fn publish_detail(
        cx: &mut TestAppContext,
        engine: &FakeEngine,
        pid: u32,
        f: impl FnOnce(&mut TargetDetail),
    ) {
        let target = DetailTarget::Process(key(pid));
        engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.details);
            let mut detail = d
                .targets
                .get(&target)
                .map(|t| TargetDetail::clone(t))
                .unwrap_or_default();
            f(&mut detail);
            d.targets.insert(target, Arc::new(detail));
            d.header.version += 1;
        });
        cx.run_until_parked();
    }

    fn open(cx: &mut TestAppContext, pid: u32) -> Option<AnyWindowHandle> {
        cx.update(|cx| open_process_properties(key(pid), cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::Properties(key(pid)))
        })
    }

    fn open_app(cx: &mut TestAppContext, group: GroupKey) -> Option<AnyWindowHandle> {
        cx.update(|cx| open_app_properties(group, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::AppProperties(group))
        })
    }

    fn props(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Entity<PropsWindow> {
        cx.update(|cx| PropsWindow::find(handle, cx))
            .expect("props window")
    }

    fn app_props(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Entity<AppPropsWindow> {
        cx.update(|cx| AppPropsWindow::find(handle, cx))
            .expect("app props window")
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn props_count(cx: &mut TestAppContext) -> usize {
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).count(CapClass::ProcessProps)
        })
    }

    fn app_props_count(cx: &mut TestAppContext) -> usize {
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).count(CapClass::AppProps)
        })
    }

    fn main_toasts(main: &mut VisualTestContext) -> Vec<String> {
        main.update(|_, cx| {
            let view = main_window_view(cx).expect("main window");
            let texts = view.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    fn demanded(cx: &mut TestAppContext) -> Vec<DetailDemand> {
        cx.update(|cx| {
            let demand = cx.global::<GlobalDemand>().0.clone();
            demand.read(cx).merged().details.to_vec()
        })
    }

    fn title(cx: &mut TestAppContext, handle: AnyWindowHandle) -> String {
        cx.update(|cx| {
            let frame =
                TlSecondaryWindow::<PropsWindow>::find(WindowHandle::new(handle.window_id()), cx)
                    .expect("frame");
            frame.read(cx).title().to_string()
        })
    }

    fn app_title(cx: &mut TestAppContext, handle: AnyWindowHandle) -> String {
        cx.update(|cx| {
            let frame = TlSecondaryWindow::<AppPropsWindow>::find(
                WindowHandle::new(handle.window_id()),
                cx,
            )
            .expect("app frame");
            frame.read(cx).title().to_string()
        })
    }

    fn general_demand(pid: u32, tab: DetailTab) -> DetailDemand {
        DetailDemand {
            target: DetailTarget::Process(key(pid)),
            tab,
        }
    }

    fn app_demand(group: GroupKey, tab: DetailTab) -> DetailDemand {
        DetailDemand {
            target: DetailTarget::App(group),
            tab,
        }
    }

    #[gpui_kit::test]
    fn app_props_title_for_app_and_unit(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let app = app_group("Firefox", GroupRowKind::App, 34);
        let unit = app_group("foo.service", GroupRowKind::Unit, 1);
        publish_groups(cx, &engine, vec![app.clone(), unit.clone()]);

        let app_handle = open_app(cx, app.key).expect("app window opened");
        let unit_handle = open_app(cx, unit.key).expect("unit window opened");
        assert_eq!(app_title(cx, app_handle), "App Properties - Firefox");
        assert_eq!(app_title(cx, unit_handle), "App Properties - foo.service");

        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        let again = open_app(cx, app.key).expect("existing app window");
        assert_eq!(again.window_id(), app_handle.window_id());
        assert_eq!(app_props_count(cx), 2, "one window per group");
        assert_eq!(
            cx.update(|cx| cx.active_window().map(|w| w.window_id())),
            Some(app_handle.window_id()),
            "opening twice activates the existing window"
        );
    }

    fn member(pid: u32, group: GroupKey) -> ProcessRecord {
        let mut r = rec(pid, ProcFlags::empty());
        r.group = group;
        r
    }

    /// Two app rows (G, H) with three processes: 101 and 103 in G, 102 in H.
    fn publish_two_apps(
        engine: &FakeEngine,
        main: &mut VisualTestContext,
        g_members: u32,
    ) -> (GroupRow, GroupRow) {
        let g = app_group("Firefox", GroupRowKind::App, g_members);
        let h = app_group("Chrome", GroupRowKind::App, 1);
        publish_table(
            engine,
            main,
            vec![member(101, g.key), member(102, h.key), member(103, g.key)],
            vec![g.clone(), h.clone()],
            Vec::new(),
        );
        (g, h)
    }

    /// A later sweep: new versions of the process records and the table groups, so the
    /// windows see both components change.
    fn sweep(
        engine: &FakeEngine,
        main: &mut VisualTestContext,
        records: Vec<ProcessRecord>,
        groups: Vec<GroupRow>,
    ) {
        engine.publish_with(|s| {
            let p = Arc::make_mut(&mut s.processes);
            p.records = Arc::from(records);
            p.header.version += 1;
            let t = Arc::make_mut(&mut s.table);
            t.groups = Arc::from(groups);
            t.header.version += 1;
        });
        main.run_until_parked();
    }

    /// `publish_two_apps` again as a later sweep, with G's row count set to `g_members`.
    fn sweep_two_apps(engine: &FakeEngine, main: &mut VisualTestContext, g_members: u32) {
        let g = app_group("Firefox", GroupRowKind::App, g_members);
        let h = app_group("Chrome", GroupRowKind::App, 1);
        let records = vec![member(101, g.key), member(102, h.key), member(103, g.key)];
        sweep(engine, main, records, vec![g, h]);
    }

    fn open_processes_tab(
        cx: &mut TestAppContext,
        group: GroupKey,
    ) -> (VisualTestContext, Entity<AppPropsWindow>) {
        let handle = open_app(cx, group).expect("app window opened");
        let view = app_props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.select_tab(DetailTab::AppProcesses, window, cx)
            })
        });
        draw(&mut vcx);
        (vcx, view)
    }

    fn view_pids(vcx: &mut VisualTestContext, view: &Entity<AppPropsWindow>) -> Vec<u32> {
        vcx.read(|cx| {
            let tab = view.read(cx).processes().read(cx);
            tab.table()
                .read(cx)
                .delegate
                .view_keys()
                .map(|k| k.pid)
                .collect()
        })
    }

    fn focus_row(vcx: &mut VisualTestContext, view: &Entity<AppPropsWindow>, pid: u32) {
        let table = vcx.read(|cx| view.read(cx).processes().read(cx).table().clone());
        let focus = vcx.read(|cx| table.read(cx).focus_handle().clone());
        vcx.update(|window, cx| {
            table.update(cx, |t, cx| {
                t.selection.click(key(pid));
                cx.notify();
            });
            focus.focus(window, cx);
        });
        draw(vcx);
    }

    #[gpui_kit::test]
    fn app_processes_members_only(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let (g, _) = publish_two_apps(&engine, &mut main, 2);
        let handle = open_app(cx, g.key).expect("app window opened");
        let view = app_props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        let passes = |vcx: &mut VisualTestContext| {
            vcx.read(|cx| view.read(cx).processes().read(cx).passes())
        };

        // Not active: sweeps arrive and the tab runs no member pass. The label still counts
        // G's two records, not the row's filtered count of 5.
        sweep_two_apps(&engine, &mut main, 5);
        assert_eq!(passes(&mut vcx), 0);
        assert_eq!(
            vcx.read(|cx| view.read(cx).tab_text(DetailTab::AppProcesses)),
            "Processes [2]",
            "the label counts the members without a member pass"
        );

        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.select_tab(DetailTab::AppProcesses, window, cx)
            })
        });
        draw(&mut vcx);
        assert_eq!(passes(&mut vcx), 1);
        assert_eq!(view_pids(&mut vcx, &view), [101, 103]);

        // Active: one pass per sweep, and the label counts that pass.
        sweep_two_apps(&engine, &mut main, 5);
        assert_eq!(passes(&mut vcx), 2);
        assert_eq!(
            vcx.read(|cx| view.read(cx).tab_text(DetailTab::AppProcesses)),
            "Processes [2]"
        );

        // An exited member leaves the list at the next sweep.
        let mut exited = member(103, g.key);
        exited.flags = ProcFlags::EXITED;
        publish_records(cx, &engine, vec![member(101, g.key), exited]);
        assert_eq!(view_pids(&mut vcx, &view), [101]);
    }

    /// The default sort is PID ascending, even when a higher PID started earlier.
    #[gpui_kit::test]
    fn app_processes_sort_pid_ascending(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let g = app_group("Firefox", GroupRowKind::App, 3);
        let records = [(300, 10), (100, 30), (200, 20)]
            .map(|(pid, start_ticks)| {
                let mut r = member(pid, g.key);
                r.key.start_ticks = start_ticks;
                r
            })
            .to_vec();
        publish_table(&engine, &mut main, records, vec![g.clone()], Vec::new());
        let (mut vcx, view) = open_processes_tab(cx, g.key);
        assert_eq!(view_pids(&mut vcx, &view), [100, 200, 300]);
    }

    #[gpui_kit::test]
    fn app_processes_enter_opens_props(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let (g, _) = publish_two_apps(&engine, &mut main, 2);
        let (mut vcx, view) = open_processes_tab(cx, g.key);
        focus_row(&mut vcx, &view, 103);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();

        assert!(
            cx.update(|cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .get(&WindowKey::Properties(key(103)))
                    .is_some()
            }),
            "Enter opens Process Properties for the focused row"
        );
        assert_eq!(props_count(cx), 1);
    }

    /// The label counts the members, whether the tab is active or not, even when the row's
    /// count (which follows the main table's filter) says less.
    #[gpui_kit::test]
    fn app_processes_count_label(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let g = app_group("Firefox", GroupRowKind::App, 34);
        let mut records: Vec<ProcessRecord> = (1000..1034).map(|pid| member(pid, g.key)).collect();
        records.push(member(2000, app_group("Chrome", GroupRowKind::App, 1).key));
        publish_table(
            &engine,
            &mut main,
            records.clone(),
            vec![g.clone()],
            Vec::new(),
        );
        let handle = open_app(cx, g.key).expect("app window opened");
        let view = app_props(cx, handle);
        let label =
            |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).tab_text(DetailTab::AppProcesses));
        assert_eq!(label(cx), "Processes [34]");

        let filtered = app_group("Firefox", GroupRowKind::App, 5);
        sweep(&engine, &mut main, records.clone(), vec![filtered]);
        assert_eq!(
            label(cx),
            "Processes [34]",
            "not active: the label ignores the row's filtered count"
        );
        sweep(&engine, &mut main, records.clone(), vec![g.clone()]);

        let (mut vcx, view) = open_processes_tab(cx, g.key);
        assert_eq!(
            view_pids(&mut vcx, &view).len(),
            34,
            "the list has as many rows"
        );
        assert_eq!(label(cx), "Processes [34]");

        let filtered = app_group("Firefox", GroupRowKind::App, 5);
        sweep(&engine, &mut main, records, vec![filtered]);
        assert_eq!(label(cx), "Processes [34]", "the label follows the list");
    }

    /// The command line column takes the width the other columns leave, so the columns fill
    /// the window.
    #[gpui_kit::test]
    fn app_processes_cmdline_column_is_flex(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let (g, _) = publish_two_apps(&engine, &mut main, 2);
        let (mut vcx, view) = open_processes_tab(cx, g.key);
        vcx.simulate_resize(size(px(1400.), px(700.)));
        draw(&mut vcx);
        let (total, window_w) = vcx.update(|window, cx| {
            let tab = view.read(cx).processes().read(cx);
            let total = tab.table().read(cx).columns().total_width().as_f32();
            (total, window.viewport_size().width.as_f32())
        });
        assert!(
            (total - window_w).abs() < 1.0,
            "columns span the window: {total} vs {window_w}"
        );
    }

    #[gpui_kit::test]
    fn app_processes_go_to_process(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let (g, h) = publish_two_apps(&engine, &mut main, 2);
        let page = processes_page(&mut main);
        let generation = |main: &mut VisualTestContext| {
            main.update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation())
        };
        let before = generation(&mut main);
        let (mut vcx, view) = open_processes_tab(cx, g.key);
        focus_row(&mut vcx, &view, 103);
        let table = vcx.read(|cx| view.read(cx).processes().read(cx).table().clone());
        vcx.update(|_, cx| table.update(cx, |t, _| t.delegate_mut().capture_for_test(&[key(103)])));

        vcx.dispatch_action(AppProcsGoTo);
        vcx.run_until_parked();

        assert!(
            generation(&mut main) > before,
            "the jump asks the Processes page for the row's key"
        );
        // The engine answers the jump's query with the process rows. A newer version, so the
        // page installs the view.
        let answered = generation(&mut main);
        let records = vec![member(101, g.key), member(102, h.key), member(103, g.key)];
        let rows = vec![
            table_row(NodeId::Group(g.key), None, Some(0)),
            table_row(NodeId::Process(key(101)), Some(0), None),
            table_row(NodeId::Process(key(103)), Some(2), None),
        ];
        let groups = vec![g.clone(), h.clone()];
        engine.publish_with(|snapshot| {
            let mut procs = ProcessSnapshot::clone(&snapshot.processes);
            procs.header.version += 1;
            procs.records = records.into();
            snapshot.processes = Arc::new(procs);
            snapshot.table = Arc::new(ProcessTableView {
                header: ComponentHeader {
                    version: snapshot.table.header.version + 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                query_generation: answered,
                rows: rows.into(),
                groups: groups.into(),
                ..ProcessTableView::default()
            });
        });
        main.run_until_parked();
        draw(&mut main);
        let selected: Vec<ProcRowId> =
            main.update(|_, cx| page.read(cx).table().read(cx).selection.ids().collect());
        assert_eq!(
            selected,
            vec![ProcRowId::Process(key(103))],
            "the jump selects the focused process, not another"
        );
        let main_id = main.update(|window, _| window.window_handle().window_id());
        assert_eq!(
            cx.update(|cx| cx.active_window().map(|w| w.window_id())),
            Some(main_id),
            "the main window is activated"
        );
    }

    /// Two units of the System class have their own row keys, so Ctrl+I on both opens two
    /// App Properties windows, one per unit (tasklens-8q0.40).
    #[gpui_kit::test]
    fn ctrl_i_on_two_system_units_opens_two_windows(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let unit_row = |name: &str| {
            let unit = tl_model::table::UnitRef {
                manager: tl_model::table::Manager::System,
                name: Arc::from(name),
            };
            GroupRow {
                node: NodeId::Unit(unit.clone()),
                key: unit.key(),
                class: GroupKind::System,
                ..app_group(name, GroupRowKind::Unit, 2)
            }
        };
        let foo = unit_row("foo.service");
        let bar = unit_row("bar.service");
        assert_ne!(foo.key, bar.key);
        publish_table(
            &engine,
            &mut main,
            Vec::new(),
            vec![foo.clone(), bar.clone()],
            vec![
                table_row(foo.node.clone(), None, Some(0)),
                table_row(bar.node.clone(), None, Some(1)),
            ],
        );
        let page = processes_page(&mut main);
        let ids: Vec<ProcRowId> = main.update(|_, cx| {
            let table = page.read(cx).table().read(cx);
            [&foo.node, &bar.node]
                .into_iter()
                .map(|node| table.delegate.id_of(node).expect("the unit row is shown"))
                .collect()
        });
        select_and_focus(&mut main, &page, &ids);

        main.simulate_keystrokes("ctrl-i");
        main.run_until_parked();

        assert_eq!(app_props_count(cx), 2, "one window per unit");
        for (row, title) in [
            (&foo, "App Properties - foo.service"),
            (&bar, "App Properties - bar.service"),
        ] {
            let handle = cx
                .update(|cx| {
                    let registry = cx.global::<GlobalRegistry>().0.clone();
                    registry.read(cx).get(&WindowKey::AppProperties(row.key))
                })
                .expect("the unit has its own window");
            assert_eq!(app_title(cx, handle), title);
        }
    }

    #[gpui_kit::test]
    fn app_props_ninth_window_toast(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let groups: Vec<_> = (0..9)
            .map(|i| app_group(&format!("App {i}"), GroupRowKind::App, 1))
            .collect();
        let keys: Vec<_> = groups.iter().map(|group| group.key).collect();
        publish_groups(cx, &engine, groups);
        for &group in &keys[..8] {
            assert!(open_app(cx, group).is_some(), "window opened");
        }
        main.update(|window, _| window.activate_window());
        main.run_until_parked();

        assert!(open_app(cx, keys[8]).is_none(), "ninth window refused");
        assert_eq!(app_props_count(cx), 8);
        assert!(
            main_toasts(&mut main)
                .iter()
                .any(|text| text == "Too many app properties windows are already open"),
            "{:?}",
            main_toasts(&mut main)
        );
    }

    #[gpui_kit::test]
    fn app_props_exited_banner(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        let app = app_group("Firefox", GroupRowKind::App, 34);
        publish_groups(cx, &engine, vec![app.clone()]);
        let handle = open_app(cx, app.key).expect("app window opened");
        let view = app_props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        draw(&mut vcx);
        assert_eq!(
            demanded(cx),
            vec![app_demand(app.key, DetailTab::AppGeneral)]
        );

        publish_groups(cx, &engine, Vec::new());
        draw(&mut vcx);

        assert!(cx.read(|cx| view.read(cx).exited()));
        assert_eq!(
            app_title(cx, handle),
            "App Properties - Firefox (not running)"
        );
        assert!(
            vcx.debug_bounds("AppPropsExitedBanner").is_some(),
            "banner drawn"
        );
        assert_eq!(
            cx.read(|cx| view.read(cx).exited_banner_text().to_owned()),
            APP_EXITED_BANNER
        );
        assert!(demanded(cx).is_empty(), "demand removed");
        assert_eq!(app_props_count(cx), 1, "window stays open");
    }

    #[gpui_kit::test]
    fn props_open_focuses_existing_window(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, cx| {
            view.update(cx, |p, cx| p.select_tab(DetailTab::Threads, window, cx))
        });
        main.update(|window, _| window.activate_window());
        main.run_until_parked();

        let again = open(cx, 4321).expect("still open");
        assert_eq!(again.window_id(), handle.window_id());
        assert_eq!(props_count(cx), 1);
        let active = cx.update(|cx| cx.active_window().map(|w| w.window_id()));
        assert_eq!(
            active,
            Some(handle.window_id()),
            "the existing window became active"
        );
        assert_eq!(cx.read(|cx| view.read(cx).active()), DetailTab::Threads);
    }

    #[gpui_kit::test]
    fn props_ninth_window_shows_toast(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let pids: Vec<u32> = (100..109).collect();
        publish_records(
            cx,
            &engine,
            pids.iter().map(|&p| rec(p, ProcFlags::default())).collect(),
        );
        for &pid in &pids[..8] {
            assert!(open(cx, pid).is_some(), "window {pid} opened");
        }
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        assert!(open(cx, pids[8]).is_none());
        assert_eq!(props_count(cx), 8);
        assert!(
            main_toasts(&mut main)
                .iter()
                .any(|t| t == "Too many process properties windows are already open"),
            "{:?}",
            main_toasts(&mut main)
        );
    }

    #[gpui_kit::test]
    fn props_open_failure_toast(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        publish_records(cx, &engine, vec![rec(200, ProcFlags::EXITED)]);
        assert!(open(cx, 4321).is_none(), "missing key");
        assert!(open(cx, 200).is_none(), "exited record");
        assert_eq!(props_count(cx), 0);
        assert!(
            main_toasts(&mut main)
                .iter()
                .any(|t| t.starts_with("Could not open properties.")),
            "{:?}",
            main_toasts(&mut main)
        );
    }

    #[gpui_kit::test]
    fn props_demand_registered_and_removed(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        assert!(demanded(cx).contains(&general_demand(4321, DetailTab::General)));
        assert!(
            engine
                .commands()
                .iter()
                .any(|c| matches!(c, Command::ComputeHashes(k) if *k == key(4321))),
            "hashes requested on open"
        );

        cx.update(|cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
        cx.run_until_parked();
        assert!(demanded(cx).is_empty(), "{:?}", demanded(cx));
    }

    #[gpui_kit::test]
    fn props_tab_switch_updates_demand(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Maps, window, cx)));
        vcx.run_until_parked();
        assert_eq!(demanded(cx), vec![general_demand(4321, DetailTab::Maps)]);
    }

    #[gpui_kit::test]
    fn props_alt_number_selects_tab(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);

        vcx.simulate_keystrokes("alt-2");
        vcx.run_until_parked();
        assert_eq!(cx.read(|cx| view.read(cx).active()), DetailTab::Threads);
        assert_eq!(demanded(cx), vec![general_demand(4321, DetailTab::Threads)]);

        vcx.simulate_keystrokes("alt-7");
        assert_eq!(cx.read(|cx| view.read(cx).active()), DetailTab::Strings);
        vcx.simulate_keystrokes("ctrl-tab");
        assert_eq!(cx.read(|cx| view.read(cx).active()), DetailTab::General);
        vcx.simulate_keystrokes("ctrl-pageup");
        assert_eq!(cx.read(|cx| view.read(cx).active()), DetailTab::Strings);
    }

    #[gpui_kit::test]
    fn props_refresh_sends_detail_refresh(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        vcx.simulate_keystrokes("f5");
        vcx.run_until_parked();
        assert!(engine.commands().iter().any(|c| matches!(
            c,
            Command::DetailRefresh { target, tab: DetailTab::General }
                if *target == DetailTarget::Process(key(4321))
        )));
    }

    #[gpui_kit::test]
    fn props_ignores_other_targets_detail_change(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(
            cx,
            &engine,
            vec![
                rec(4321, ProcFlags::default()),
                rec(555, ProcFlags::default()),
            ],
        );
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let general = cx
            .read(|cx| view.read(cx).tabs().general.clone())
            .expect("general tab");
        // A frame is drawn only after something in the window notifies, so the notifications
        // of the window view and its tab are the renders a snapshot causes.
        let notified = Rc::new(Cell::new(0u32));
        let _subs = cx.update(|cx| {
            let a = notified.clone();
            let b = notified.clone();
            [
                cx.observe(&view, move |_, _| a.set(a.get() + 1)),
                cx.observe(&general, move |_, _| b.set(b.get() + 1)),
            ]
        });
        let general_payload = |name: &str| {
            let p = payload(detail(name));
            move |d: &mut TargetDetail| {
                d.tabs[DetailTab::General.index()].payload = Some(p);
                d.tabs[DetailTab::General.index()].validity = Validity::Valid;
            }
        };

        publish_detail(cx, &engine, 555, general_payload("other"));
        assert_eq!(notified.get(), 0, "another key's payload");

        publish_detail(cx, &engine, 4321, general_payload("firefox"));
        assert!(notified.get() > 0, "this key's payload re-renders");
        let after = notified.get();
        publish_detail(cx, &engine, 555, general_payload("other 2"));
        assert_eq!(notified.get(), after, "another key's payload again");
        assert!(cx.read(|cx| view.read(cx).render_count()) > 0);
    }

    #[gpui_kit::test]
    fn props_tab_counts_format(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        cx.update(|cx| {
            view.update(cx, |p, cx| {
                p.set_count(DetailTab::Fds, 1204, cx);
                p.set_count(DetailTab::Environment, 51, cx);
                p.set_count(DetailTab::Security, 3, cx);
            })
        });
        cx.read(|cx| {
            let p = view.read(cx);
            assert_eq!(
                p.tab_text(DetailTab::Fds),
                "File descriptors [1\u{2009}204]"
            );
            assert_eq!(p.tab_text(DetailTab::Environment), "Environment [51]");
            assert_eq!(
                p.tab_text(DetailTab::Threads),
                "Threads [97]",
                "from the record"
            );
            assert_eq!(p.tab_text(DetailTab::Security), "Security", "never a count");
        });
    }

    #[gpui_kit::test]
    fn props_tab_no_count_before_load(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        cx.read(|cx| {
            let p = view.read(cx);
            assert_eq!(p.tab_text(DetailTab::Maps), "Memory maps");
            assert_eq!(p.tab_text(DetailTab::Fds), "File descriptors");
            assert_eq!(p.tab_text(DetailTab::General), "General");
        });
    }

    #[gpui_kit::test]
    fn props_exited_freezes_values(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        publish_detail(cx, &engine, 4321, |d| {
            d.tabs[DetailTab::General.index()].payload = Some(payload(detail("firefox --x")));
            d.tabs[DetailTab::General.index()].validity = Validity::Valid;
        });
        draw(&mut vcx);
        let general = cx
            .read(|cx| view.read(cx).tabs().general.clone())
            .expect("general tab");
        let values = |cx: &mut TestAppContext| {
            cx.read(|cx| {
                let g = general.read(cx);
                [FieldId::Affinity, FieldId::Executable, FieldId::Pid]
                    .map(|id| g.field(id).map(|f| f.value.to_string()).unwrap_or_default())
            })
        };
        let before = values(cx);
        assert_eq!(title(cx, handle), "Process Properties - firefox - PID 4321");

        publish_records(cx, &engine, vec![rec(4321, ProcFlags::EXITED)]);
        publish_records(cx, &engine, Vec::new());
        draw(&mut vcx);

        assert!(cx.read(|cx| view.read(cx).exited()));
        assert_eq!(
            title(cx, handle),
            "Process Properties - firefox - PID 4321 (not running)"
        );
        assert!(
            vcx.debug_bounds("PropsExitedBanner").is_some(),
            "banner drawn"
        );
        assert_eq!(
            cx.read(|cx| view.read(cx).words().exited_banner.to_string()),
            "This process has exited. Values are frozen at the last update."
        );
        assert_eq!(values(cx), before, "values frozen");
        assert!(demanded(cx).is_empty(), "demand removed");
        assert_eq!(props_count(cx), 1, "the window stays open");
    }

    #[gpui_kit::test]
    fn props_exited_when_key_missing_without_exited_flag(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        draw(&mut vcx);

        // A partial snapshot: the system changed, the process table did not.
        engine.publish_with(|s| {
            Arc::make_mut(&mut s.processes).records = Arc::from(Vec::new());
            Arc::make_mut(&mut s.system).header.version += 1;
        });
        cx.run_until_parked();
        assert!(!cx.read(|cx| view.read(cx).exited()), "partial snapshot");

        // Exited and reaped between two snapshots: never seen with `EXITED`.
        publish_records(cx, &engine, vec![rec(99, ProcFlags::default())]);
        draw(&mut vcx);
        assert!(cx.read(|cx| view.read(cx).exited()));
        assert_eq!(
            title(cx, handle),
            "Process Properties - firefox - PID 4321 (not running)"
        );
        assert!(
            vcx.debug_bounds("PropsExitedBanner").is_some(),
            "banner drawn"
        );
        assert!(demanded(cx).is_empty(), "demand removed");
    }

    #[gpui_kit::test]
    fn props_tab_tooltip_names_alt_key(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let tips = cx.read(|cx| view.read(cx).words().tab_tips.clone());
        assert_eq!(tips[0].as_ref(), "General (Alt+1)");
        assert_eq!(tips[2].as_ref(), "File descriptors (Alt+3)");
    }

    #[gpui_kit::test]
    fn access_block_texts_per_tab(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        let words = cx.update(props_words);
        for text in [
            ACCESS_FDS,
            ACCESS_MAPS,
            ACCESS_SECURITY,
            ACCESS_ENV,
            ACCESS_STRINGS,
        ] {
            assert_eq!(words.access(text).as_ref(), text, "en-US text");
        }

        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        publish_detail(cx, &engine, 4321, |d| {
            d.tabs[DetailTab::Fds.index()].validity = Validity::Restricted;
        });
        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Fds, window, cx)));
        draw(&mut vcx);
        assert!(
            vcx.debug_bounds("PropsAccessBlock").is_some(),
            "access block drawn"
        );
        assert!(vcx.debug_bounds("PropsAccessBlock-text").is_some());

        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Maps, window, cx)));
        draw(&mut vcx);
        assert!(
            vcx.debug_bounds("PropsAccessBlock").is_none(),
            "Maps is not restricted"
        );
    }

    fn props_toasts(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Vec<String> {
        cx.update(|cx| {
            let frame =
                TlSecondaryWindow::<PropsWindow>::find(WindowHandle::new(handle.window_id()), cx)
                    .expect("frame");
            let texts = frame.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    fn set_check(check: PackageCheck) -> impl FnOnce(&mut TargetDetail) {
        move |d| {
            let mut g = detail("firefox");
            g.package_check = check;
            d.tabs[DetailTab::General.index()].payload = Some(payload(g));
            d.tabs[DetailTab::General.index()].validity = Validity::Valid;
        }
    }

    #[gpui_kit::test]
    fn verify_result_toasts(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        publish_detail(cx, &engine, 4321, set_check(PackageCheck::NotVerified));
        draw(&mut vcx);
        let general = cx
            .read(|cx| view.read(cx).tabs().general.clone())
            .expect("general tab");
        let status = |cx: &mut TestAppContext| {
            cx.read(|cx| {
                general
                    .read(cx)
                    .field(FieldId::PackageStatus)
                    .map(|f| (f.value.to_string(), f.tone))
                    .expect("status")
            })
        };

        let cases = [
            (
                PackageCheck::Matches,
                "Executable matches the package.",
                "Matches package",
            ),
            (
                PackageCheck::DoesNotMatch,
                "Executable does not match the package.",
                "Does not match package",
            ),
            (
                PackageCheck::Failed,
                "Could not verify against the package.",
                "Not verified",
            ),
        ];
        for (n, (result, toast, word)) in cases.into_iter().enumerate() {
            publish_detail(cx, &engine, 4321, set_check(PackageCheck::NotVerified));
            draw(&mut vcx);
            let button = vcx
                .debug_bounds("TlButton-general-verify")
                .expect("verify button drawn");
            vcx.simulate_click(button.center(), Default::default());
            draw(&mut vcx);
            let sent = engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::VerifyPackage(k) if *k == key(4321)))
                .count();
            assert_eq!(sent, n + 1, "VerifyPackage sent");
            let button = cx
                .read(|cx| general.read(cx).verify_button())
                .expect("shown");
            assert!(button.loading);
            assert_eq!(button.label.as_ref(), "Verifying…");
            assert_eq!(status(cx).0, "Verifying…");

            publish_detail(cx, &engine, 4321, set_check(PackageCheck::Verifying));
            publish_detail(cx, &engine, 4321, set_check(result));
            draw(&mut vcx);
            assert!(
                props_toasts(cx, handle).iter().any(|t| t == toast),
                "{:?}",
                props_toasts(cx, handle)
            );
            assert_eq!(status(cx).0, word);
            let button = cx
                .read(|cx| general.read(cx).verify_button())
                .expect("shown");
            assert!(!button.loading);
        }
        // A result without a verify running here shows no toast.
        let before = props_toasts(cx, handle).len();
        publish_detail(cx, &engine, 4321, set_check(PackageCheck::Matches));
        assert_eq!(props_toasts(cx, handle).len(), before);
        assert_eq!(
            status(cx),
            (
                "Matches package".to_string(),
                Some(tl_ui::components::kv_grid::ValueTone::Success)
            )
        );
    }

    // ---- General copy, links, field menu and keyboard (E23-011) ----

    struct GeneralRig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
        handle: AnyWindowHandle,
        vcx: VisualTestContext,
        general: Entity<GeneralTab>,
    }

    /// Parent 1200 and the process 4321 with a General payload from `edit`.
    fn general_rig(cx: &mut TestAppContext, edit: impl FnOnce(&mut GeneralDetail)) -> GeneralRig {
        let (engine, main) = setup(cx);
        publish_records(
            cx,
            &engine,
            vec![
                rec(1200, ProcFlags::default()),
                rec(4321, ProcFlags::default()),
            ],
        );
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        let mut d = detail("firefox --x");
        edit(&mut d);
        publish_detail(cx, &engine, 4321, move |t| {
            t.tabs[DetailTab::General.index()].payload = Some(payload(d));
            t.tabs[DetailTab::General.index()].validity = Validity::Valid;
        });
        draw(&mut vcx);
        draw(&mut vcx);
        let general = cx
            .read(|cx| view.read(cx).tabs().general.clone())
            .expect("general tab");
        GeneralRig {
            engine,
            main,
            handle,
            vcx,
            general,
        }
    }

    /// The row index of `label` in column `col`.
    fn general_row(rig: &mut GeneralRig, col: usize, label: &str) -> usize {
        rig.vcx
            .read(|cx| rig.general.read(cx).rows(col))
            .iter()
            .position(|r| r.label.as_ref() == label)
            .unwrap_or_else(|| panic!("{label} shown"))
    }

    /// A point just inside the value of row `row` in column `col` (rows in the first grid).
    fn general_value_point(
        rig: &mut GeneralRig,
        col: usize,
        row: usize,
    ) -> gpui_kit::Point<Pixels> {
        draw(&mut rig.vcx);
        let panel = rig
            .vcx
            .debug_bounds(
                [
                    "GeneralTab-panel-0",
                    "GeneralTab-panel-1",
                    "GeneralTab-panel-2",
                ][col],
            )
            .expect("panel drawn");
        let (sp3, sp4, row_h) = rig.vcx.update(|_, cx| {
            let m = tl_ui::theme::metrics(cx);
            (m.sp[3], m.sp[4], px(26. * m.ui_scale))
        });
        let label = rig
            .vcx
            .read(|cx| rig.general.read(cx).label_width(col))
            .expect("measured");
        gpui_kit::point(
            panel.left() + sp4 + label + px(3.),
            panel.top() + sp3 + row_h * (row as f32 + 0.5),
        )
    }

    fn general_click(rig: &mut GeneralRig, col: usize, row: usize) {
        let at = general_value_point(rig, col, row);
        rig.vcx.simulate_click(at, Default::default());
        rig.vcx.run_until_parked();
    }

    fn clipboard(vcx: &mut VisualTestContext) -> String {
        vcx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    }

    fn focus_fields(rig: &mut GeneralRig) {
        let focus = rig.vcx.read(|cx| rig.general.read(cx).grid_focus().clone());
        rig.vcx.update(|window, cx| focus.focus(window, cx));
        draw(&mut rig.vcx);
    }

    #[gpui_kit::test]
    fn general_click_copies_full_hash(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |d| {
            d.hashes = HashState::Done {
                sha256: sha256_3f9a(),
                md5: [0xa0; 16],
            };
        });
        let row = general_row(&mut rig, 1, "SHA-256:");
        general_click(&mut rig, 1, row);
        let copied = clipboard(&mut rig.vcx);
        assert_eq!(copied.len(), 64, "{copied}");
        assert!(copied.starts_with("3f9a0c41e2b77d09"));
        assert!(copied.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            props_toasts(cx, rig.handle),
            ["Process property value copied."]
        );
    }

    #[gpui_kit::test]
    fn general_ppid_link_dispatches_goto(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |_| {});
        let page = rig.main.update(|_, cx| {
            main_window_view(cx)
                .and_then(|v| {
                    v.read(cx)
                        .page_entity::<tl_ui::pages::processes::ProcessesPage>(Tab::Processes)
                })
                .expect("processes page")
        });
        let generation = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation())
        };
        let before = generation(&mut rig.main);
        let row = general_row(&mut rig, 0, "PPID:");
        assert!(
            rig.vcx
                .read(|cx| rig.general.read(cx).rows(0)[row].value.link.is_some()),
            "PPID is a link"
        );
        general_click(&mut rig, 0, row);
        assert!(
            generation(&mut rig.main) > before,
            "the jump to the parent expands its path"
        );
        let active = cx.update(|cx| cx.active_window().map(|w| w.window_id()));
        let main_id = rig
            .main
            .update(|window, _| window.window_handle().window_id());
        assert_eq!(active, Some(main_id), "the main window is activated");
        assert!(
            clipboard(&mut rig.vcx).is_empty(),
            "a link click copies nothing"
        );
    }

    #[gpui_kit::test]
    fn general_exe_link_sends_show_in_file_manager(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |_| {});
        let row = general_row(&mut rig, 0, "Executable:");
        general_click(&mut rig, 0, row);
        let sent: Vec<_> = rig
            .engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(req) => Some(req),
                _ => None,
            })
            .collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].targets.is_empty());
        assert!(!sent[0].privileged);
        assert!(matches!(
            &sent[0].kind,
            ActionKind::ShowInFileManager { path, properties: false }
                if &**path == "/usr/lib/firefox/firefox"
        ));
        assert_eq!(props_toasts(cx, rig.handle), ["Opening file location"]);
    }

    #[gpui_kit::test]
    fn general_keyboard_cursor_moves_between_columns(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |_| {});
        focus_fields(&mut rig);
        let cursor = |rig: &mut GeneralRig| rig.vcx.read(|cx| rig.general.read(cx).cursor());

        rig.vcx.simulate_keystrokes("down");
        assert_eq!(
            cursor(&mut rig),
            Some((0, 0)),
            "the first key places the cursor"
        );
        rig.vcx.simulate_keystrokes("down down");
        assert_eq!(cursor(&mut rig), Some((0, 2)), "column 1 row 3");
        rig.vcx.simulate_keystrokes("right");
        assert_eq!(cursor(&mut rig), Some((1, 2)), "column 2 row 3");
        draw(&mut rig.vcx);
        assert!(
            rig.vcx.debug_bounds("TlKv-cursor-2").is_some(),
            "the ring is drawn"
        );

        // Column 2 is longer than column 3: Right clamps to column 3's last row.
        let (n2, n3) = rig.vcx.read(|cx| {
            let g = rig.general.read(cx);
            (g.rows(1).len(), g.rows(2).len())
        });
        assert!(n2 > n3, "{n2} > {n3}");
        rig.vcx.simulate_keystrokes("end");
        assert_eq!(cursor(&mut rig), Some((1, n2 - 1)));
        rig.vcx.simulate_keystrokes("right");
        assert_eq!(cursor(&mut rig), Some((2, n3 - 1)), "clamped");
        rig.vcx.simulate_keystrokes("home left");
        assert_eq!(cursor(&mut rig), Some((1, 0)));

        // Enter copies a plain value; Ctrl+C too.
        rig.vcx.simulate_keystrokes("left enter");
        assert_eq!(clipboard(&mut rig.vcx), "firefox");
        assert_eq!(
            props_toasts(cx, rig.handle),
            ["Process property value copied."]
        );
    }

    #[gpui_kit::test]
    fn general_copy_all_fields_order(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |d| {
            d.hashes = HashState::Done {
                sha256: sha256_3f9a(),
                md5: [0xa0; 16],
            };
        });
        focus_fields(&mut rig);
        // Shift+F10 opens the field menu; Copy all fields is its last item.
        rig.vcx.simulate_keystrokes("shift-f10");
        draw(&mut rig.vcx);
        rig.vcx.simulate_keystrokes("end enter");
        rig.vcx.run_until_parked();
        let text = clipboard(&mut rig.vcx);
        let lines: Vec<&str> = text.lines().collect();
        let at = |prefix: &str| {
            lines
                .iter()
                .position(|l| l.starts_with(prefix))
                .unwrap_or_else(|| panic!("{prefix} in {text}"))
        };
        assert_eq!(lines[0], "Process: firefox");
        assert!(at("PID: 4321") < at("Executable: /usr/lib/firefox/firefox"));
        assert!(
            at("Cgroup:").max(at("Executable:")) < at("SHA-256: "),
            "column 1 first"
        );
        assert!(
            at("I/O write ops:").max(at("SHA-256:")) < at("Disk read bytes:"),
            "then 2, 3"
        );
        let sha = lines[at("SHA-256: ")];
        assert_eq!(sha.len(), "SHA-256: ".len() + 64, "the full hash");
        assert_eq!(*lines.last().unwrap(), "Command line: firefox --x");
        assert_eq!(props_toasts(cx, rig.handle), ["Copied to clipboard"]);
    }

    #[gpui_kit::test]
    fn general_exited_links_plain_and_copy(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |_| {});
        publish_records(cx, &rig.engine, vec![rec(1200, ProcFlags::default())]);
        draw(&mut rig.vcx);
        let row = general_row(&mut rig, 0, "PPID:");
        let value = rig
            .vcx
            .read(|cx| rig.general.read(cx).rows(0)[row].value.clone());
        assert!(value.link.is_none(), "plain text");
        assert_eq!(value.validity, Validity::Stale, "text_secondary");
        general_click(&mut rig, 0, row);
        assert_eq!(clipboard(&mut rig.vcx), "1200");
        assert_eq!(
            props_toasts(cx, rig.handle),
            ["Process property value copied."]
        );
        assert!(
            !rig.engine
                .commands()
                .iter()
                .any(|c| matches!(c, Command::Action(_))),
            "nothing sent"
        );
    }

    #[gpui_kit::test]
    fn general_tab_order_and_click_moves_cursor(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |_| {});
        // A left click copies and moves the cursor to the clicked field.
        let row = general_row(&mut rig, 0, "PID:");
        general_click(&mut rig, 0, row);
        assert_eq!(
            rig.vcx.read(|cx| rig.general.read(cx).cursor()),
            Some((0, row))
        );
        assert_eq!(clipboard(&mut rig.vcx), "4321");

        // Tab: the fields, then the command line Copy button, then Verify.
        focus_fields(&mut rig);
        let (fields, copy, verify) = rig.vcx.read(|cx| {
            let g = rig.general.read(cx);
            (
                g.grid_focus().clone(),
                g.cmdline().read(cx).copy_focus().clone(),
                g.verify_focus().clone(),
            )
        });
        let focused = |rig: &mut GeneralRig, h: &gpui_kit::FocusHandle| {
            rig.vcx.update(|window, _| h.is_focused(window))
        };
        assert!(focused(&mut rig, &fields));
        rig.vcx.simulate_keystrokes("tab");
        draw(&mut rig.vcx);
        assert!(focused(&mut rig, &copy), "Copy after the fields");
        rig.vcx.simulate_keystrokes("tab");
        draw(&mut rig.vcx);
        assert!(focused(&mut rig, &verify), "Verify after Copy");
    }

    // ---- File descriptors tab (E23-021, UI-PWN-012) ----

    use gpui_kit::Modifiers;
    use tl_model::details::{FdDetail, FdKind, FdRow};
    use tl_ui::components::menu::{MenuEntryKind, open_context_menu};
    use tl_ui::table::actions::{CopyRow, CopyRowsAsTable, OpenContextMenu};
    use tl_ui::windows::props::fds::FdsTab;

    fn fd(fd: i32, kind: FdKind, name: &str, flags: Option<u32>, pos: Option<u64>) -> FdRow {
        FdRow {
            fd,
            kind,
            name: text(name),
            unnamed: name.is_empty(),
            pos,
            flags,
            mnt_id: Some(25),
            inode: Some(1_048_577),
            eventfd_count: None,
            drm_client_id: None,
            drm_driver: None,
            socket_inode: None,
        }
    }

    fn fd_rows() -> Vec<FdRow> {
        vec![
            fd(
                3,
                FdKind::File,
                "/home/u/notes.txt",
                Some(0o2104002),
                Some(4096),
            ),
            fd(4, FdKind::Pipe, "pipe:[90211]", Some(0o4000), None),
            fd(7, FdKind::AnonInode, "", Some(0o2), None),
        ]
    }

    struct FdsRig {
        engine: Arc<FakeEngine>,
        _main: VisualTestContext,
        handle: AnyWindowHandle,
        view: Entity<PropsWindow>,
        vcx: VisualTestContext,
        tab: Entity<FdsTab>,
    }

    /// Opens Properties for `pid` on the File descriptors tab with `rows` as its payload.
    fn fds_window(
        cx: &mut TestAppContext,
        engine: &Arc<FakeEngine>,
        pid: u32,
        rows: Vec<FdRow>,
        validity: Validity,
    ) -> (
        AnyWindowHandle,
        Entity<PropsWindow>,
        VisualTestContext,
        Entity<FdsTab>,
    ) {
        let handle = open(cx, pid).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        let payload = Arc::new(TabPayload::Fds(Arc::new(FdDetail {
            rows: Arc::from(rows),
            fdinfo_available: true,
        })));
        publish_detail(cx, engine, pid, move |d| {
            let slot = &mut d.tabs[DetailTab::Fds.index()];
            slot.payload = Some(payload);
            slot.validity = validity;
        });
        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Fds, window, cx)));
        draw(&mut vcx);
        let tab = cx
            .read(|cx| view.read(cx).tabs().fds.clone())
            .expect("fds tab");
        (handle, view, vcx, tab)
    }

    fn fds_rig(cx: &mut TestAppContext) -> FdsRig {
        let (engine, main) = setup(cx);
        publish_records(
            cx,
            &engine,
            vec![
                rec(1200, ProcFlags::default()),
                rec(4321, ProcFlags::default()),
            ],
        );
        let (handle, view, vcx, tab) = fds_window(cx, &engine, 4321, fd_rows(), Validity::Valid);
        FdsRig {
            engine,
            _main: main,
            handle,
            view,
            vcx,
            tab,
        }
    }

    fn shown_fds(rig: &mut FdsRig) -> Vec<i32> {
        rig.vcx.read(|cx| {
            let table = rig.tab.read(cx).table().clone();
            table.read(cx).delegate.view_fds().collect()
        })
    }

    /// Selects `fds` with the first as the focus row and focuses the table.
    fn select_fds(rig: &mut FdsRig, fds: &[i32]) {
        let table = rig.vcx.read(|cx| rig.tab.read(cx).table().clone());
        let fds = fds.to_vec();
        rig.vcx.update(|window, cx| {
            table.update(cx, |t, cx| {
                for (i, fd) in fds.iter().enumerate() {
                    if i == 0 {
                        t.selection.click(*fd);
                    } else {
                        t.selection.ctrl_click(*fd);
                    }
                }
                cx.notify();
            });
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        draw(&mut rig.vcx);
    }

    fn clip(vcx: &mut VisualTestContext) -> String {
        vcx.read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default()
    }

    /// The keyboard row menu's entries as (label, enabled).
    fn fds_menu(rig: &mut FdsRig) -> Vec<(String, bool)> {
        rig.vcx.dispatch_action(OpenContextMenu);
        draw(&mut rig.vcx);
        rig.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the row menu is open");
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| {
                    (
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.kind == MenuEntryKind::Item,
                    )
                })
                .collect()
        })
    }

    #[gpui_kit::test]
    fn fds_hide_unnamed_filters_anon_inode(cx: &mut TestAppContext) {
        let mut rig = fds_rig(cx);
        assert_eq!(
            shown_fds(&mut rig),
            [3, 4],
            "the unnamed anon_inode row is hidden"
        );
        let label = rig
            .vcx
            .read(|cx| rig.view.read(cx).tab_text(DetailTab::Fds));
        assert_eq!(label, "File descriptors [2]");

        // The filter bar checkbox turns the switch off and the row comes back.
        let check = rig
            .vcx
            .debug_bounds("TlCheckbox-label")
            .expect("Hide unnamed handles drawn");
        rig.vcx.simulate_click(check.center(), Modifiers::none());
        draw(&mut rig.vcx);
        let on = rig.vcx.read(|cx| {
            tl_ui::settings_global::TlSettings::settings(cx)
                .properties
                .hide_unnamed_handles
        });
        assert!(!on, "the click wrote the setting");
        assert_eq!(shown_fds(&mut rig), [3, 4, 7]);
        let label = rig
            .vcx
            .read(|cx| rig.view.read(cx).tab_text(DetailTab::Fds));
        assert_eq!(label, "File descriptors [3]");
    }

    /// The switch is one setting: a toggle in one window re-filters another.
    #[gpui_kit::test]
    fn fds_hide_toggle_refilters_other_window(cx: &mut TestAppContext) {
        let mut rig = fds_rig(cx);
        let engine = rig.engine.clone();
        let (_, _, mut other_vcx, other) =
            fds_window(cx, &engine, 1200, fd_rows(), Validity::Valid);
        let other_fds = |vcx: &mut VisualTestContext| {
            vcx.read(|cx| {
                let table = other.read(cx).table().clone();
                table.read(cx).delegate.view_fds().collect::<Vec<_>>()
            })
        };
        assert_eq!(other_fds(&mut other_vcx), [3, 4]);
        let check = rig
            .vcx
            .debug_bounds("TlCheckbox-label")
            .expect("Hide unnamed handles drawn");
        rig.vcx.simulate_click(check.center(), Modifiers::none());
        draw(&mut rig.vcx);
        draw(&mut other_vcx);
        assert_eq!(other_fds(&mut other_vcx), [3, 4, 7]);
        assert_eq!(shown_fds(&mut rig), [3, 4, 7]);
    }

    #[gpui_kit::test]
    fn fds_pane_reads_decoded_flags(cx: &mut TestAppContext) {
        let mut rig = fds_rig(cx);
        let pane = rig.vcx.read(|cx| rig.tab.read(cx).pane().clone());
        let message = rig
            .vcx
            .read(|cx| pane.read(cx).message().map(|m| m.to_string()));
        assert_eq!(
            message.as_deref(),
            Some("Select a descriptor to see its flags.")
        );
        assert!(rig.vcx.debug_bounds("FdDetailsPane-message").is_some());

        select_fds(&mut rig, &[3]);
        let lines: Vec<(String, String)> = rig.vcx.read(|cx| {
            pane.read(cx)
                .lines()
                .into_iter()
                .map(|(l, v)| (l.to_string(), v.to_string()))
                .collect()
        });
        let pairs: Vec<(&str, &str)> = lines.iter().map(|(l, v)| (&**l, &**v)).collect();
        assert_eq!(
            pairs,
            [
                ("Raw flags:", "0o2104002"),
                ("Access mode:", "Read/write"),
                ("Status flags:", "Non-blocking"),
                ("Descriptor flags:", "Close-on-exec"),
                ("Mount ID:", "25"),
                ("Inode:", "1048577"),
            ]
        );
        assert!(
            !lines
                .iter()
                .any(|(_, v)| v.to_lowercase().contains("large")),
            "no large-file word"
        );
        assert!(rig.vcx.debug_bounds("FdDetailsPane-message").is_none());
    }

    #[gpui_kit::test]
    fn fds_open_location_only_for_files(cx: &mut TestAppContext) {
        let mut rig = fds_rig(cx);
        select_fds(&mut rig, &[4]);
        let items = fds_menu(&mut rig);
        let shown: Vec<(&str, bool)> = items.iter().map(|(l, e)| (&**l, *e)).collect();
        assert_eq!(
            shown,
            [("Copy", true), ("Open file location", false)],
            "pipe row"
        );
        rig.vcx.update(tl_ui::components::menu::close_context_menu);
        draw(&mut rig.vcx);

        select_fds(&mut rig, &[3]);
        let items = fds_menu(&mut rig);
        let shown: Vec<(&str, bool)> = items.iter().map(|(l, e)| (&**l, *e)).collect();
        assert_eq!(
            shown,
            [("Copy", true), ("Open file location", true)],
            "file row"
        );
        // The first "down" highlights Copy.
        rig.vcx.simulate_keystrokes("down down enter");
        draw(&mut rig.vcx);
        let sent: Vec<_> = rig
            .engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(req) => Some(req),
                _ => None,
            })
            .collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].targets.is_empty());
        assert!(matches!(
            &sent[0].kind,
            ActionKind::ShowInFileManager { path, properties: false }
                if &**path == "/home/u/notes.txt"
        ));
        assert_eq!(props_toasts(cx, rig.handle), ["Opening file location"]);
    }

    #[gpui_kit::test]
    fn fds_privilege_state_text(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let (_, _, mut vcx, _) = fds_window(cx, &engine, 4321, Vec::new(), Validity::Restricted);
        assert!(vcx.debug_bounds("PropsAccessBlock").is_some());
        assert!(
            vcx.debug_bounds("PropsFds").is_none(),
            "no table behind the block"
        );
        let words = cx.update(props_words);
        assert_eq!(
            words.access(ACCESS_FDS).as_ref(),
            "Handles unavailable (insufficient privileges)"
        );
    }

    #[gpui_kit::test]
    fn maps_restricted_access_block(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        publish_detail(cx, &engine, 4321, |d| {
            d.tabs[DetailTab::Maps.index()].validity = Validity::Restricted;
        });
        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Maps, window, cx)));
        draw(&mut vcx);
        assert!(vcx.debug_bounds("PropsAccessBlock").is_some());
        assert!(
            vcx.debug_bounds("PropsMaps").is_none(),
            "no tree behind the block"
        );
        let words = cx.update(props_words);
        assert_eq!(
            words.access(ACCESS_MAPS).as_ref(),
            "Needs administrator access."
        );
    }

    #[gpui_kit::test]
    fn security_restricted_access_block(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        publish_detail(cx, &engine, 4321, |d| {
            d.tabs[DetailTab::Security.index()].validity = Validity::Restricted;
        });
        vcx.update(|window, cx| {
            view.update(cx, |p, cx| p.select_tab(DetailTab::Security, window, cx))
        });
        draw(&mut vcx);
        assert!(vcx.debug_bounds("PropsAccessBlock").is_some());
        assert!(
            vcx.debug_bounds("PropsSecurity").is_none(),
            "no summary behind the block"
        );
        assert!(
            vcx.debug_bounds("TlButton").is_none(),
            "Security never gets a button"
        );
        let words = cx.update(props_words);
        assert_eq!(
            words.access(ACCESS_SECURITY).as_ref(),
            "Security information unavailable (insufficient privileges)"
        );
    }

    #[gpui_kit::test]
    fn fds_copy_formats(cx: &mut TestAppContext) {
        let mut rig = fds_rig(cx);
        select_fds(&mut rig, &[4, 3]);
        rig.vcx.dispatch_action(CopyRow);
        assert_eq!(
            clip(&mut rig.vcx),
            "3\tfile\t/home/u/notes.txt\tRW +NB CLOEXEC\t4096\n4\tpipe\tpipe:[90211]\tR +NB\t-",
            "view order, no header"
        );
        rig.vcx.dispatch_action(CopyRowsAsTable);
        let tsv = clip(&mut rig.vcx);
        let mut lines = tsv.lines();
        assert_eq!(lines.next(), Some("FD\tType\tName\tAccess\tPosition"));
        assert_eq!(
            lines.next(),
            Some("3\tfile\t/home/u/notes.txt\tRW +NB CLOEXEC\t4096")
        );

        // The menu Copy takes the Name cell from the keyboard menu.
        select_fds(&mut rig, &[4]);
        let items = fds_menu(&mut rig);
        assert_eq!(items[0].0, "Copy");
        rig.vcx.simulate_keystrokes("down enter");
        draw(&mut rig.vcx);
        assert_eq!(clip(&mut rig.vcx), "pipe:[90211]");

        // The pane copies itself as lines.
        let pane = rig.vcx.read(|cx| rig.tab.read(cx).pane().clone());
        let focus = rig.vcx.read(|cx| pane.read(cx).focus_handle().clone());
        rig.vcx.update(|window, cx| focus.focus(window, cx));
        draw(&mut rig.vcx);
        rig.vcx.dispatch_action(CopyRow);
        let text = clip(&mut rig.vcx);
        assert!(
            text.starts_with("Raw flags: 0o4000\nAccess mode: Read only\n"),
            "{text}"
        );
        assert!(text.contains("Status flags: Non-blocking"), "{text}");
        // Four copies, one toast with its repeat count.
        assert_eq!(props_toasts(cx, rig.handle), ["Copied to clipboard (×4)"]);
    }

    // ---- Environment tab (E23-022, UI-PWN-016, R-099) ----

    use tl_model::details::EnvDetail;
    use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
    use tl_ui::table::actions::SelectAll;
    use tl_ui::windows::props::environ::{
        EnvCopyAll, EnvironTab, MASK, READ_FAILED_ROW, errno_text,
    };

    struct EnvRig {
        engine: Arc<FakeEngine>,
        _main: VisualTestContext,
        handle: AnyWindowHandle,
        vcx: VisualTestContext,
        tab: Entity<EnvironTab>,
    }

    fn env_payload(vars: &[(&str, &str)], read_errno: Option<i32>) -> Arc<TabPayload> {
        let vars: Vec<(Box<str>, Box<str>)> = vars
            .iter()
            .map(|(n, v)| (Box::from(*n), Box::from(*v)))
            .collect();
        Arc::new(TabPayload::Environ(Arc::new(EnvDetail {
            vars: Arc::from(vars),
            truncated: false,
            read_errno,
            unavailable: false,
        })))
    }

    fn env_vars() -> Arc<TabPayload> {
        env_payload(
            &[
                ("PATH", "/usr/bin"),
                ("GH_TOKEN", "ghp_secret1"),
                ("HOME", "/home/u"),
            ],
            None,
        )
    }

    fn publish_env(cx: &mut TestAppContext, engine: &FakeEngine, payload: Arc<TabPayload>) {
        publish_detail(cx, engine, 4321, move |d| {
            let slot = &mut d.tabs[DetailTab::Environment.index()];
            slot.payload = Some(payload);
            slot.validity = Validity::Valid;
        });
    }

    /// Opens Properties for 4321 on the Environment tab with `payload`.
    fn env_rig(cx: &mut TestAppContext, payload: Arc<TabPayload>) -> EnvRig {
        let (engine, main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        publish_env(cx, &engine, payload);
        vcx.update(|window, cx| {
            view.update(cx, |p, cx| p.select_tab(DetailTab::Environment, window, cx))
        });
        draw(&mut vcx);
        let tab = cx
            .read(|cx| view.read(cx).tabs().environ.clone())
            .expect("environment tab");
        EnvRig {
            engine,
            _main: main,
            handle,
            vcx,
            tab,
        }
    }

    /// Selects the row of `name` and focuses the table.
    fn select_env(rig: &mut EnvRig, name: &str) {
        let table = rig.vcx.read(|cx| rig.tab.read(cx).table().clone());
        let id = rig
            .vcx
            .read(|cx| table.read(cx).delegate.row_named(name))
            .expect("row shown");
        rig.vcx.update(|window, cx| {
            table.update(cx, |t, cx| {
                t.selection.click(id);
                cx.notify();
            });
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        draw(&mut rig.vcx);
    }

    fn env_names(rig: &mut EnvRig) -> Vec<String> {
        rig.vcx.read(|cx| {
            let table = rig.tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            d.view_ids().map(|i| d.name(i).to_owned()).collect()
        })
    }

    /// Runs row menu item `n` (1-based) from the keyboard.
    fn env_menu_pick(rig: &mut EnvRig, n: usize) -> Vec<String> {
        rig.vcx.dispatch_action(OpenContextMenu);
        draw(&mut rig.vcx);
        let labels = rig.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the row menu is open");
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| e.label.map(|l| l.to_string()).unwrap_or_default())
                .collect::<Vec<_>>()
        });
        let keys = vec!["down"; n].join(" ") + " enter";
        rig.vcx.simulate_keystrokes(&keys);
        draw(&mut rig.vcx);
        labels
    }

    fn env_filter(rig: &mut EnvRig, text: &str) {
        let filter = rig.vcx.read(|cx| rig.tab.read(cx).filter().clone());
        rig.vcx
            .update(|window, cx| filter.update(cx, |f, cx| f.clear(window, cx)));
        rig.vcx.executor().advance_clock(Duration::from_millis(150));
        draw(&mut rig.vcx);
        if !text.is_empty() {
            rig.vcx.update(|window, cx| {
                let input = filter.read(cx).input.clone();
                input.update(cx, |s, cx| s.insert(text.to_owned(), window, cx));
            });
            rig.vcx.executor().advance_clock(Duration::from_millis(150));
        }
        draw(&mut rig.vcx);
    }

    fn masked(rig: &mut EnvRig, name: &str) -> bool {
        rig.vcx.read(|cx| {
            let table = rig.tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            d.row_named(name).is_some_and(|i| d.is_masked(i))
        })
    }

    #[gpui_kit::test]
    fn env_copy_value_masked_unless_revealed(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_vars());
        assert_eq!(env_names(&mut rig), ["GH_TOKEN", "HOME", "PATH"]);
        select_env(&mut rig, "GH_TOKEN");
        let labels = env_menu_pick(&mut rig, 2);
        assert_eq!(
            labels,
            [
                "Copy name",
                "Copy value",
                "Copy all as NAME=value",
                "",
                "Reveal value"
            ]
        );
        assert_eq!(clip(&mut rig.vcx), MASK);
        rig.vcx.dispatch_action(CopyRow);
        assert_eq!(clip(&mut rig.vcx), format!("GH_TOKEN={MASK}"));

        // Space on the focus row reveals it.
        select_env(&mut rig, "GH_TOKEN");
        rig.vcx.simulate_keystrokes("space");
        draw(&mut rig.vcx);
        assert!(!masked(&mut rig, "GH_TOKEN"));
        let labels = env_menu_pick(&mut rig, 2);
        assert_eq!(labels[4], "Hide value");
        assert_eq!(clip(&mut rig.vcx), "ghp_secret1");
        rig.vcx.dispatch_action(CopyRow);
        assert_eq!(clip(&mut rig.vcx), "GH_TOKEN=ghp_secret1");

        // The menu hides it again.
        env_menu_pick(&mut rig, 4);
        assert!(masked(&mut rig, "GH_TOKEN"));
    }

    #[gpui_kit::test]
    fn env_copy_all_masks(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_vars());
        select_env(&mut rig, "HOME");
        rig.vcx.dispatch_action(EnvCopyAll);
        assert_eq!(
            clip(&mut rig.vcx),
            format!("GH_TOKEN={MASK}\nHOME=/home/u\nPATH=/usr/bin")
        );
        rig.vcx.dispatch_action(SelectAll);
        rig.vcx.dispatch_action(CopyRowsAsTable);
        assert_eq!(
            clip(&mut rig.vcx),
            format!("Name\tValue\nGH_TOKEN\t{MASK}\nHOME\t/home/u\nPATH\t/usr/bin")
        );
        assert!(
            props_toasts(cx, rig.handle)
                .iter()
                .all(|t| t.starts_with("Copied to clipboard")),
        );
    }

    #[gpui_kit::test]
    fn env_refresh_remasks(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_vars());
        select_env(&mut rig, "GH_TOKEN");
        rig.vcx.simulate_keystrokes("space");
        draw(&mut rig.vcx);
        assert!(!masked(&mut rig, "GH_TOKEN"));
        rig.vcx.simulate_keystrokes("f5");
        draw(&mut rig.vcx);
        assert!(masked(&mut rig, "GH_TOKEN"), "Refresh masks every row");
        assert!(rig.vcx.read(|cx| rig.tab.read(cx).is_refreshing()));
        assert!(rig.engine.commands().iter().any(|c| matches!(
            c,
            Command::DetailRefresh { target, tab: DetailTab::Environment }
                if *target == DetailTarget::Process(key(4321))
        )));
        let engine = rig.engine.clone();
        publish_env(cx, &engine, env_vars());
        draw(&mut rig.vcx);
        assert!(!rig.vcx.read(|cx| rig.tab.read(cx).is_refreshing()));
        assert!(masked(&mut rig, "GH_TOKEN"));
    }

    #[gpui_kit::test]
    fn env_mask_off_setting(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_vars());
        assert!(masked(&mut rig, "GH_TOKEN"));
        rig.vcx.update(|_, cx| {
            TlSettings::set(cx, SettingKey::PropertiesEnvMask, SettingValue::Bool(false))
        });
        draw(&mut rig.vcx);
        assert!(!masked(&mut rig, "GH_TOKEN"));
        select_env(&mut rig, "GH_TOKEN");
        rig.vcx.dispatch_action(CopyRow);
        assert_eq!(clip(&mut rig.vcx), "GH_TOKEN=ghp_secret1");
        rig.vcx.update(|_, cx| {
            TlSettings::set(cx, SettingKey::PropertiesEnvMask, SettingValue::Bool(true))
        });
        draw(&mut rig.vcx);
        assert!(masked(&mut rig, "GH_TOKEN"));
    }

    #[gpui_kit::test]
    fn env_read_failed_row(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_payload(&[], Some(13)));
        let ids: Vec<u32> = rig.vcx.read(|cx| {
            let table = rig.tab.read(cx).table().clone();
            table.read(cx).delegate.view_ids().collect()
        });
        assert_eq!(ids, [READ_FAILED_ROW]);
        assert_eq!(env_names(&mut rig), ["(read failed)"]);
        assert!(
            errno_text(13).contains("Permission denied"),
            "{}",
            errno_text(13)
        );
    }

    #[gpui_kit::test]
    fn props_filter_masked_env_matches_name_only(cx: &mut TestAppContext) {
        let mut rig = env_rig(cx, env_vars());
        env_filter(&mut rig, "ghp_secret");
        assert!(
            env_names(&mut rig).is_empty(),
            "a masked value never matches"
        );
        env_filter(&mut rig, "token");
        assert_eq!(env_names(&mut rig), ["GH_TOKEN"]);
        env_filter(&mut rig, "/home");
        assert_eq!(env_names(&mut rig), ["HOME"], "plain values match");
        env_filter(&mut rig, "");
        select_env(&mut rig, "GH_TOKEN");
        rig.vcx.simulate_keystrokes("space");
        draw(&mut rig.vcx);
        env_filter(&mut rig, "ghp_secret");
        assert_eq!(
            env_names(&mut rig),
            ["GH_TOKEN"],
            "a revealed value matches"
        );
    }

    #[gpui_kit::test]
    fn env_restricted_access_block(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        publish_detail(cx, &engine, 4321, |d| {
            d.tabs[DetailTab::Environment.index()].validity = Validity::Restricted;
        });
        vcx.update(|window, cx| {
            view.update(cx, |p, cx| p.select_tab(DetailTab::Environment, window, cx))
        });
        draw(&mut vcx);
        assert!(vcx.debug_bounds("PropsAccessBlock").is_some());
        assert!(vcx.debug_bounds("PropsEnvironment").is_none());
        let words = cx.update(props_words);
        assert_eq!(
            words.access(ACCESS_ENV).as_ref(),
            "Environment unavailable (insufficient privileges)"
        );
    }
}

// ---- Threads tab (E23-016, UI-PWN-010) ----

#[cfg(feature = "test-hooks")]
mod threads {
    use tl_model::actions::ActionTarget;
    use tl_model::details::ThreadRow;
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenu, TlMenuPopup};
    use tl_ui::table::TreeTableDelegate as _;
    use tl_ui::table::actions::{CopyRow, CopyRowsAsTable};
    use tl_ui::windows::actions::{SchedPolicyWire, SetThreadSched};
    use tl_ui::windows::props::threads::{ThreadColumn, ThreadsTab};

    use super::*;

    struct ThreadsHost {
        tab: Entity<ThreadsTab>,
        filter: Entity<PropsFilter>,
    }

    impl Render for ThreadsHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(1400.))
                .h(px(600.))
                .flex()
                .flex_col()
                .child(filter_bar(&self.filter, None, cx))
                .child(div().flex_1().child(self.tab.clone()))
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<ThreadsHost>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
            tl_ui::table::actions::bind_keys_table(cx);
        });
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let filter = cx.new(|cx| PropsFilter::new("Threads", window, cx));
            let tab = cx.new(|cx| ThreadsTab::new(KEY, filter.clone(), window, cx));
            ThreadsHost { tab, filter }
        });
        draw(cx);
        (view, cx)
    }

    fn row(tid: u32, comm: &str, policy: SchedPolicy, rt_prio: u8, cpu: f32) -> ThreadRow {
        ThreadRow {
            tid,
            comm: text(comm),
            state: ProcState::Sleeping,
            cpu_pct: cpu,
            utime_ms: 1500,
            stime_ms: 20,
            policy,
            rt_prio,
            nice: 0,
            cpus: text("0-31"),
            last_cpu: 3,
            vol_ctx: 1204,
            invol_ctx: 33,
            wchan: text("0"),
            validity: Validity::Valid,
        }
    }

    fn threads(rows: Vec<ThreadRow>) -> Arc<TabPayload> {
        Arc::new(TabPayload::Threads(Arc::from(rows)))
    }

    fn feed(view: &Entity<ThreadsHost>, cx: &mut VisualTestContext, p: Option<Arc<TabPayload>>) {
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        cx.update(|_, cx| {
            tab.update(cx, |t, cx| {
                t.set_online_cpus(cpus(&[(0, 31)]), cx);
                t.set_payload(p.as_ref(), cx);
            })
        });
        draw(cx);
    }

    fn cell(
        view: &Entity<ThreadsHost>,
        cx: &mut VisualTestContext,
        tid: u32,
        col: ThreadColumn,
    ) -> String {
        cx.update(|_, cx| {
            let table = view.read(cx).tab.read(cx).table().clone();
            table
                .read(cx)
                .delegate
                .cell_text(tid, col)
                .expect("row shown")
        })
    }

    fn empty_text(view: &Entity<ThreadsHost>, cx: &mut VisualTestContext) -> Option<String> {
        use tl_ui::table::TreeTableDelegate as _;
        cx.update(|_, cx| {
            let table = view.read(cx).tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            (d.row_count() == 0).then(|| d.empty_text().map(|t| t.to_string()))?
        })
    }

    fn menu(
        view: &Entity<ThreadsHost>,
        cx: &mut VisualTestContext,
        tids: &[u32],
        col: Option<ThreadColumn>,
    ) -> Entity<TlMenuPopup> {
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        cx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            TlMenu::build(window, cx, focus, |menu, window, cx| {
                table.update(cx, |table, cx| {
                    table.delegate.context_menu(tids, col, menu, window, cx)
                })
            })
        })
    }

    fn menu_entries(menu: &Entity<TlMenuPopup>, cx: &mut VisualTestContext) -> Vec<MenuEntryInfo> {
        cx.update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    fn sample() -> Vec<ThreadRow> {
        vec![
            row(4321, "firefox", SchedPolicy::Other, 0, 1.0),
            row(4325, "Renderer", SchedPolicy::Fifo, 50, 9.5),
            row(4330, "IPC I/O", SchedPolicy::Batch, 0, 4.0),
        ]
    }

    #[gpui_kit::test]
    fn threads_priority_words(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        assert_eq!(cell(&view, cx, 4321, ThreadColumn::Priority), "Variable");
        assert_eq!(cell(&view, cx, 4325, ThreadColumn::Priority), "Realtime 50");
        assert_eq!(cell(&view, cx, 4330, ThreadColumn::Priority), "Variable");
        assert_eq!(cell(&view, cx, 4325, ThreadColumn::BasePriority), "50");
        assert_eq!(
            cell(&view, cx, 4321, ThreadColumn::CtxSwitches),
            "1\u{2009}204 / 33"
        );
        assert_eq!(cell(&view, cx, 4321, ThreadColumn::WaitChannel), "-");
        assert_eq!(cell(&view, cx, 4321, ThreadColumn::Affinity), "All");
        assert_eq!(cell(&view, cx, 4325, ThreadColumn::Cpu), "9.5");
        assert_eq!(
            cell(&view, cx, 4321, ThreadColumn::UserTime),
            "1\u{2009}500"
        );
    }

    #[gpui_kit::test]
    fn threads_selection_survives_resort(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        cx.update(|_, cx| {
            table.update(cx, |t, cx| {
                t.selection.click(4325);
                t.sort_by_column(ThreadColumn::Cpu, cx);
            })
        });
        draw(cx);
        let order = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| table.read(cx).delegate.view_tids().collect::<Vec<_>>())
        };
        assert_eq!(order(cx), [4325, 4330, 4321], "CPU % descending first");

        // A new sweep: the CPU values move and the order with them.
        let mut next = sample();
        next[0].cpu_pct = 50.;
        feed(&view, cx, Some(threads(next)));
        assert_eq!(order(cx), [4321, 4325, 4330]);
        let (selected, focus) = cx.update(|_, cx| {
            let t = table.read(cx);
            (t.selection.ids().collect::<Vec<_>>(), t.selection.focus())
        });
        assert_eq!(selected, [4325]);
        assert_eq!(focus, Some(4325));
    }

    #[gpui_kit::test]
    fn threads_loading_and_empty_texts(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        assert_eq!(empty_text(&view, cx).as_deref(), Some("Loading threads..."));
        assert!(cx.debug_bounds("TlTreeTable-empty").is_some());
        feed(&view, cx, Some(threads(Vec::new())));
        assert_eq!(empty_text(&view, cx).as_deref(), Some("No threads found."));

        // A filter with no match keeps the header and shows the Threads text.
        feed(&view, cx, Some(threads(sample())));
        let filter = cx.update(|_, cx| view.read(cx).filter.clone());
        cx.update(|window, cx| {
            let input = filter.read(cx).input.clone();
            input.update(cx, |s, cx| s.insert("xyz".to_owned(), window, cx));
        });
        cx.executor().advance_clock(Duration::from_millis(100));
        draw(cx);
        assert_eq!(
            empty_text(&view, cx).as_deref(),
            Some("No threads match the current filter.")
        );
        assert!(
            cx.debug_bounds("TlTreeTableHeader-label-tid").is_some(),
            "the header stays"
        );

        // A matching filter keeps the rows it names.
        cx.update(|window, cx| {
            let input = filter.read(cx).input.clone();
            input.update(cx, |s, cx| {
                s.clean(window, cx);
                s.insert("render".to_owned(), window, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_millis(100));
        draw(cx);
        let shown = cx.update(|_, cx| {
            let table = view.read(cx).tab.read(cx).table().clone();
            table.read(cx).delegate.view_tids().collect::<Vec<_>>()
        });
        assert_eq!(shown, [4325]);
    }

    /// The Threads layout is tagged `TableId::Processes` but lives in memory only: a column
    /// change never reaches `state.toml`, so it cannot overwrite the Processes layout.
    #[gpui_kit::test]
    fn threads_layout_never_persisted(cx: &mut TestAppContext) {
        use tl_model::settings::{Settings, UiState};
        use tl_ui::app::BootSettings;
        use tl_ui::settings_global::TlSettings;
        use tl_ui::table::TreeTableDelegate as _;
        cx.update(|cx| {
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        let saved = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                let s = TlSettings::state(cx);
                (s.columns.clone(), s.sort.clone())
            })
        };
        let before = saved(cx);
        cx.update(|_, cx| {
            table.update(cx, |t, cx| {
                let mut layout = t.columns().clone();
                layout.widths.insert(ThreadColumn::Tid, px(200.));
                t.delegate_mut().on_columns_changed(&layout, cx);
                t.sort_by_column(ThreadColumn::Name, cx);
            })
        });
        draw(cx);
        let width = cx.update(|_, cx| table.read(cx).columns().widths[&ThreadColumn::Tid]);
        assert_eq!(width, px(200.), "kept in memory");
        assert_eq!(saved(cx), before, "nothing reached the saved state");
    }

    #[gpui_kit::test]
    fn threads_copy_row_and_tsv(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        cx.update(|window, cx| {
            table.update(cx, |t, _| t.selection.click(4325));
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        draw(cx);

        cx.dispatch_action(CopyRow);
        let text = cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 12, "{text}");
        assert_eq!(lines[0], "Thread ID: 4325");
        assert_eq!(lines[1], "Name: Renderer");
        assert_eq!(lines[6], "Priority: Realtime 50");
        assert_eq!(lines[10], "Ctx switches: 1\u{2009}204 / 33");

        cx.dispatch_action(CopyRowsAsTable);
        let tsv = cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default();
        let mut tsv_lines = tsv.lines();
        let header = tsv_lines.next().unwrap_or_default();
        assert!(
            header.starts_with("Thread ID\tName\tState\tCPU %"),
            "{header}"
        );
        let first = tsv_lines.next().unwrap_or_default();
        assert!(
            first.starts_with("4325\tRenderer\tSleeping\t9.5"),
            "{first}"
        );
        assert_eq!(tsv_lines.next(), None, "one selected row");
    }

    /// rsf.28: the main thread (TID = PID 4321) draws its Name at 500, the others at normal.
    #[gpui_kit::test]
    fn threads_main_thread_name_weight_500(cx: &mut TestAppContext) {
        use gpui_kit::FontWeight;
        use tl_ui::table::TreeTableDelegate as _;
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let weights = cx.update(|_, cx| {
            let table = view.read(cx).tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            [4321, 4325, 4330].map(|tid| d.name_weight(d.row_index(tid).expect("shown")))
        });
        assert_eq!(
            weights,
            [FontWeight::MEDIUM, FontWeight::NORMAL, FontWeight::NORMAL]
        );
    }

    /// rsf.28: dragging the Ctx switches boundary far left stops at its 100 px minimum
    /// (gma.19), not at the 60 px text minimum.
    #[gpui_kit::test]
    fn threads_resize_stops_at_column_minimum(cx: &mut TestAppContext) {
        use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
        use tl_ui::table::columns::TableColumn as _;
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        let header = cx.debug_bounds("TlTreeTableHeader").expect("the header");
        // The right boundary of Ctx switches: the widths of every column up to it.
        let right = cx.update(|_, cx| {
            let layout = table.read(cx).columns();
            ThreadColumn::ALL
                .iter()
                .take_while(|&&c| c != ThreadColumn::WaitChannel)
                .map(|c| layout.width(*c))
                .fold(px(0.), |a, w| a + w)
        });
        let at = |x: Pixels| gpui_kit::point(header.left() + x, header.top() + px(10.));
        cx.simulate_event(MouseDownEvent {
            position: at(right),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        draw(cx);
        for x in [right - px(40.), right - px(120.)] {
            cx.simulate_event(MouseMoveEvent {
                position: at(x),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::none(),
            });
            draw(cx);
        }
        cx.simulate_event(MouseUpEvent {
            position: at(right - px(120.)),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
        draw(cx);
        let width = cx.update(|_, cx| table.read(cx).columns().widths[&ThreadColumn::CtxSwitches]);
        assert_eq!(ThreadColumn::CtxSwitches.min_width(), Some(100.));
        assert_eq!(width, px(100.), "stopped at its own minimum");
    }

    #[gpui_kit::test]
    fn thread_menu_captures_thread_targets(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut rows = sample();
        rows.push(row(4326, "Renderer 2", SchedPolicy::Other, 0, 2.));
        feed(&view, cx, Some(threads(rows)));
        let built = menu(&view, cx, &[4325, 4326], Some(ThreadColumn::Name));
        let _ = built;
        let targets = cx.update(|_, cx| {
            let table = view.read(cx).tab.read(cx).table().clone();
            table.read(cx).delegate.captured_targets_for_test()
        });
        assert_eq!(
            targets.as_slice(),
            [
                ActionTarget::Thread {
                    process: KEY,
                    tid: 4325,
                },
                ActionTarget::Thread {
                    process: KEY,
                    tid: 4326,
                },
            ]
        );

        let normal_menu = menu(&view, cx, &[4321], Some(ThreadColumn::Name));
        let priority = menu_entries(&normal_menu, cx)
            .into_iter()
            .find(|entry| entry.label.as_deref() == Some("Set thread priority"))
            .and_then(|entry| entry.submenu)
            .expect("priority submenu");
        let normal = menu_entries(&priority, cx)
            .into_iter()
            .find(|entry| entry.label.as_deref() == Some("Normal"))
            .expect("Normal item");
        assert!(normal.checked);
    }

    #[gpui_kit::test]
    fn thread_signal_toast_text_single_and_multi(cx: &mut TestAppContext) {
        cx.update(|cx| tl_ui::i18n::init_strings(cx, "en-US"));
        use tl_ui::windows::props::threads::signal_success_for_test;
        assert_eq!(
            signal_success_for_test(10, &[4325]),
            "Sent SIGUSR1 to thread 4325"
        );
        assert_eq!(
            signal_success_for_test(10, &[4325, 4326]),
            "Sent SIGUSR1 to 2 threads"
        );
    }

    #[gpui_kit::test]
    fn thread_menu_disabled_when_exited(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        cx.update(|_, cx| tab.update(cx, |tab, cx| tab.set_exited(true, cx)));
        let built = menu(&view, cx, &[4325], Some(ThreadColumn::Name));
        let entries = menu_entries(&built, cx);
        assert_eq!(entries.len(), 4);
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == MenuEntryKind::Disabled)
        );
        assert!(
            entries
                .iter()
                .all(|entry| { entry.tooltip.as_deref() == Some("The process is not running.") })
        );
    }

    #[gpui_kit::test]
    fn thread_realtime_confirms_first(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, Some(threads(sample())));
        let built = menu(&view, cx, &[4325], Some(ThreadColumn::Name));
        let _ = built;
        let table = cx.update(|_, cx| view.read(cx).tab.read(cx).table().clone());
        cx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        let before = cx.update(|_, cx| cx.windows().len());
        cx.dispatch_action(SetThreadSched {
            policy: SchedPolicyWire::Rr,
            rt_prio: 1,
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| cx.windows().len()), before + 1);
    }
}

// ---- Memory maps tab (E23-018, UI-PWN-013) ----

mod maps {
    use tl_model::details::{MapNode, MapRegion, MapsDetail};
    use tl_ui::windows::props::maps::{MapsColumn, MapsTab};

    use super::*;

    struct MapsHost {
        tab: Entity<MapsTab>,
        filter: Entity<PropsFilter>,
    }

    impl Render for MapsHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(1600.))
                .h(px(600.))
                .flex()
                .flex_col()
                .child(filter_bar(&self.filter, None, cx))
                .child(div().flex_1().child(self.tab.clone()))
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<MapsHost>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
            tl_ui::table::actions::bind_keys_table(cx);
        });
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let filter = cx.new(|cx| PropsFilter::new("Memory maps", window, cx));
            let tab = cx.new(|cx| MapsTab::new(KEY, filter.clone(), window, cx));
            MapsHost { tab, filter }
        });
        draw(cx);
        (view, cx)
    }

    fn rss(validity: Validity) -> Total<u64> {
        Total {
            value: 8192,
            validity,
        }
    }

    fn node(name: &str, base: u64, regions: std::ops::Range<u32>, shared: bool) -> MapNode {
        node_with(name, base, regions, shared, Validity::Valid)
    }

    fn node_with(
        name: &str,
        base: u64,
        regions: std::ops::Range<u32>,
        shared: bool,
        validity: Validity,
    ) -> MapNode {
        MapNode {
            name: text(name),
            path: Some(text(&format!("/usr/lib/{name}"))),
            primary: false,
            outside: false,
            shared_object: shared,
            base,
            size_bytes: 0x21000,
            rss_bytes: rss(validity),
            description: None,
            package: Some(text("glibc 2.42-1")),
            regions,
        }
    }

    fn region(start: u64, label: Option<&str>, validity: Validity) -> MapRegion {
        MapRegion {
            start,
            end: start + 0x21000,
            perms: *b"r-xp",
            offset: 0x1a000,
            rss_bytes: rss(validity),
            label: label.map(text),
        }
    }

    /// firefox (primary), libc.so.6, kernel32.dll (a Wine mapping the engine counts as shared),
    /// data.bin (not shared) and the outside node with [heap] and a memfd.
    fn payload(validity: Validity) -> Arc<TabPayload> {
        let mut primary = node("firefox", 0x55d3_f000_0000, 0..1, false);
        primary.primary = true;
        primary.path = Some(text("/usr/lib/firefox/firefox"));
        let mut outside = node("", 0x1000, 4..6, false);
        outside.outside = true;
        outside.path = None;
        outside.package = None;
        let detail = MapsDetail {
            nodes: Arc::from(vec![
                outside,
                node_with("libc.so.6", 0x7f3a_2c00_0000, 1..3, true, validity),
                node("kernel32.dll", 0x7b00_0000_0000, 3..4, true),
                node("data.bin", 0x7f00_0000_0000, 4..4, false),
                primary,
            ]),
            regions: Arc::from(vec![
                region(0x55d3_f000_0000, None, validity),
                region(0x7f3a_2c00_0000, None, validity),
                region(0x7f3a_2c02_1000, None, validity),
                region(0x7b00_0000_0000, None, validity),
                region(0x55d3_f1a0_0000, Some("[heap]"), validity),
                region(0x7f10_0000_0000, Some("memfd:wayland-shm"), validity),
            ]),
        };
        Arc::new(TabPayload::Maps(Arc::new(detail)))
    }

    fn feed(view: &Entity<MapsHost>, cx: &mut VisualTestContext, p: Arc<TabPayload>) {
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        cx.update(|_, cx| tab.update(cx, |t, cx| t.set_payload(Some(&p), cx)));
        draw(cx);
    }

    fn names(view: &Entity<MapsHost>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.clone();
            let table = tab.read(cx).table().clone();
            table.read(cx).delegate.row_names()
        })
    }

    fn toggle(view: &Entity<MapsHost>, cx: &mut VisualTestContext, name: &str) {
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        let id = cx
            .update(|_, cx| {
                let table = tab.read(cx).table().clone();
                table.read(cx).delegate.node_named(name)
            })
            .expect("node shown");
        cx.update(|_, cx| tab.update(cx, |t, cx| t.toggle(id, cx)));
        draw(cx);
    }

    #[gpui_kit::test]
    fn maps_primary_first_outside_last(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::Valid));
        assert_eq!(
            names(&view, cx),
            [
                "firefox (Primary)",
                "kernel32.dll",
                "libc.so.6",
                "Outside loaded modules"
            ]
        );
        let base = cx.update(|_, cx| {
            let tab = view.read(cx).tab.clone();
            let table = tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            let id = d.node_named("libc.so.6").expect("libc");
            d.cell_text(id, MapsColumn::Base)
        });
        assert_eq!(base.as_deref(), Some("0x7f3a2c000000"));
        let count = cx.update(|_, cx| view.read(cx).tab.read(cx).count(cx));
        assert_eq!(count, 4);
    }

    #[gpui_kit::test]
    fn maps_shared_only_filter_includes_wine_dll(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::Valid));
        let shown = names(&view, cx);
        assert!(shown.iter().any(|n| n == "kernel32.dll"), "{shown:?}");
        assert!(!shown.iter().any(|n| n == "data.bin"), "{shown:?}");
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        cx.update(|_, cx| tab.update(cx, |t, cx| t.set_shared_only(false, cx)));
        draw(cx);
        assert!(names(&view, cx).iter().any(|n| n == "data.bin"));
    }

    /// Assumption until the leader answers the open item: with the switch on, "(Primary)" and
    /// "Outside loaded modules" stay visible.
    #[gpui_kit::test]
    fn maps_shared_only_keeps_primary_and_outside(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::Valid));
        let shown = names(&view, cx);
        assert_eq!(shown.first().map(String::as_str), Some("firefox (Primary)"));
        assert_eq!(
            shown.last().map(String::as_str),
            Some("Outside loaded modules")
        );
    }

    #[gpui_kit::test]
    fn maps_rss_warming_up_glyph(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::WarmingUp));
        toggle(&view, cx, "libc.so.6");
        let shown = names(&view, cx);
        assert!(
            shown.iter().any(|n| n == "7f3a2c000000-7f3a2c021000"),
            "{shown:?}"
        );
        let rss = cx.update(|_, cx| {
            let tab = view.read(cx).tab.clone();
            let table = tab.read(cx).table().clone();
            let d = &table.read(cx).delegate;
            let id = d.node_named("libc.so.6").expect("libc");
            let region = (id.0, 0x7f3a_2c00_0000);
            (
                d.cell_text(id, MapsColumn::Rss),
                d.cell_text(region, MapsColumn::Rss),
                d.cell_text(region, MapsColumn::Perms),
                d.cell_text(region, MapsColumn::Offset),
            )
        });
        assert_eq!(rss.0.as_deref(), Some("\u{2026}"));
        assert_eq!(rss.1.as_deref(), Some("\u{2026}"));
        assert_eq!(rss.2.as_deref(), Some("r-xp"));
        assert_eq!(rss.3.as_deref(), Some("0x0001a000"));
    }

    #[gpui_kit::test]
    fn maps_expand_state_survives_refresh(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::Valid));
        assert_eq!(names(&view, cx).len(), 4, "all nodes start collapsed");
        toggle(&view, cx, "libc.so.6");
        assert_eq!(names(&view, cx).len(), 6);
        // A new payload (a new Arc) keeps libc expanded.
        feed(&view, cx, payload(Validity::Valid));
        let shown = names(&view, cx);
        assert_eq!(shown.len(), 6, "{shown:?}");
        let ix = shown.iter().position(|n| n == "libc.so.6").expect("libc");
        assert_eq!(shown[ix + 1], "7f3a2c000000-7f3a2c021000");
    }

    #[gpui_kit::test]
    fn maps_filter_keeps_parent(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        feed(&view, cx, payload(Validity::Valid));
        toggle(&view, cx, "Outside loaded modules");
        let filter = cx.update(|_, cx| view.read(cx).filter.clone());
        cx.update(|window, cx| {
            let input = filter.read(cx).input.clone();
            input.update(cx, |s, cx| s.insert("wayland".to_owned(), window, cx));
        });
        cx.executor().advance_clock(Duration::from_millis(150));
        draw(cx);
        assert_eq!(
            names(&view, cx),
            [
                "Outside loaded modules",
                "memfd:wayland-shm 7f1000000000-7f1000021000"
            ],
            "the matching region keeps its node; the other region is hidden"
        );
    }

    /// rsf.28: the gma.23 minimum widths, logical px at 100%.
    #[test]
    fn maps_columns_have_minimum_widths() {
        use tl_ui::table::columns::TableColumn as _;
        let mins: Vec<_> = MapsColumn::ALL.iter().map(|c| c.min_width()).collect();
        let want = [120., 110., 70., 70., 60., 80., 60., 60., 60.];
        assert_eq!(mins, want.map(Some));
    }
}

// ---- Security tab summary (E23-020, UI-PWN-014) ----

mod security {
    use gpui_kit::{Modifiers, MouseMoveEvent, point};
    use tl_model::details::{
        CapRow, Elevated, GroupMembershipRow, NsKind, NsRow, NsState, SecurityDetail,
    };
    use tl_ui::components::tooltip::DELAY_CONTROL;
    #[cfg(feature = "test-hooks")]
    use tl_ui::windows::props::security::CAP_CHECK_SIZE;
    use tl_ui::windows::props::security::{
        CapColumn, NO_ID, NS_TIP_SELF, RISKY_TOOLTIP, SecField, SecurityTab,
    };
    use tl_ui::{table::cells::CellOut, table::tree_table::TreeTableDelegate as _};

    use super::*;

    struct SecurityHost {
        tab: Entity<SecurityTab>,
        filter: Entity<PropsFilter>,
    }

    impl Render for SecurityHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(1000.))
                .h(px(600.))
                .flex()
                .flex_col()
                .child(filter_bar(&self.filter, None, cx))
                .child(div().flex_1().child(self.tab.clone()))
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<SecurityHost>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::table::actions::bind_keys_table(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let filter = cx.new(|cx| PropsFilter::new("Security", window, cx));
            let tab = cx.new(|cx| SecurityTab::new(KEY, filter.clone(), cx));
            SecurityHost { tab, filter }
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        (view, cx)
    }

    fn ids(real: u32, effective: u32) -> IdTriple {
        IdTriple {
            real,
            effective,
            saved: effective,
            fs: effective,
        }
    }

    fn names(real: &str, other: &str) -> [Arc<str>; 4] {
        [text(real), text(other), text(other), text(other)]
    }

    fn group(name: &str, gid: u32, admin: bool, root_equivalent: bool) -> GroupMembershipRow {
        GroupMembershipRow {
            name: text(name),
            gid,
            primary: false,
            root_equivalent,
            admin,
        }
    }

    fn cap(
        cap: u8,
        effective: bool,
        permitted: bool,
        inheritable: bool,
        bounding: bool,
        ambient: bool,
        risky: bool,
    ) -> CapRow {
        CapRow {
            cap,
            effective,
            permitted,
            inheritable,
            bounding,
            ambient,
            risky,
        }
    }

    fn detail(uids: IdTriple, uid_names: [Arc<str>; 4]) -> SecurityDetail {
        let ns = |kind| NsRow {
            kind,
            state: NsState::SharedWithInit,
            validity: Validity::Valid,
        };
        SecurityDetail {
            uids: valid(uids),
            uid_names: valid(uid_names),
            gids: valid(ids(1000, 1000)),
            gid_names: valid(names("oystein", "oystein")),
            elevated: valid(Elevated::No),
            no_new_privs: valid(false),
            seccomp: valid(Seccomp::Disabled),
            dumpable: valid(true),
            security_label: valid(text("unconfined")),
            session: Some(text("2")),
            audit_session: Some(NO_ID),
            login_uid: Some(1000),
            namespaces: [
                NsKind::User,
                NsKind::Pid,
                NsKind::Net,
                NsKind::Mnt,
                NsKind::Ipc,
                NsKind::Uts,
                NsKind::Cgroup,
                NsKind::Time,
            ]
            .map(ns),
            store_bypass: valid(text("thread vulnerable")),
            indirect_branch: valid(text("conditional enabled")),
            caps: Arc::from(Vec::<CapRow>::new()),
            groups: Arc::from(vec![
                group("wheel", 998, true, false),
                group("docker", 965, false, true),
                group("audio", 995, false, false),
            ]),
        }
    }

    fn feed(view: &Entity<SecurityHost>, cx: &mut VisualTestContext, d: SecurityDetail) {
        let p = Arc::new(TabPayload::Security(Arc::new(d)));
        let tab = cx.update(|_, cx| view.read(cx).tab.clone());
        cx.update(|_, cx| tab.update(cx, |t, cx| t.set_payload(Some(&p), cx)));
        draw(cx);
    }

    fn value(view: &Entity<SecurityHost>, cx: &mut VisualTestContext, id: SecField) -> String {
        cx.update(|_, cx| {
            view.read(cx)
                .tab
                .read(cx)
                .value(id)
                .map(|v| v.to_string())
                .unwrap_or_default()
        })
    }

    #[gpui_kit::test]
    fn security_user_line_equal_and_mixed(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        assert_eq!(value(&view, cx, SecField::User), "Collecting\u{2026}");
        feed(
            &view,
            cx,
            detail(ids(1000, 1000), names("oystein", "oystein")),
        );
        assert_eq!(value(&view, cx, SecField::User), "oystein (1000)");
        assert_eq!(value(&view, cx, SecField::Group), "oystein (1000)");
        assert_eq!(value(&view, cx, SecField::Groups), "3");
        assert_eq!(value(&view, cx, SecField::LoginUid), "oystein (1000)");
        assert_eq!(value(&view, cx, SecField::AuditSession), "None");
        feed(&view, cx, detail(ids(1000, 0), names("oystein", "root")));
        assert_eq!(
            value(&view, cx, SecField::User),
            "real oystein (1000), effective root (0), saved root (0), fs root (0)"
        );
    }

    #[gpui_kit::test]
    fn security_admin_groups_root_equivalent(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.caps = Arc::from(vec![CapRow {
            cap: 21,
            effective: true,
            permitted: true,
            inheritable: false,
            bounding: true,
            ambient: false,
            risky: true,
        }]);
        feed(&view, cx, d);
        assert_eq!(
            value(&view, cx, SecField::AdminGroups),
            "wheel, docker (root-equivalent)"
        );
        assert_eq!(value(&view, cx, SecField::RiskyCaps), "SYS_ADMIN");
        let tip = cx.update(|_, cx| view.read(cx).tab.read(cx).tooltip(SecField::RiskyCaps));
        assert_eq!(tip.as_deref(), Some(RISKY_TOOLTIP));
    }

    #[gpui_kit::test]
    fn security_ns_same_as_tasklens_without_helper(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.namespaces[0].state = NsState::SameAsTasklens;
        d.namespaces[1].state = NsState::IsolatedFromTasklens { inode: 4026532211 };
        d.namespaces[2].validity = Validity::Restricted;
        feed(&view, cx, d);
        let ns = SecField::Ns(NsKind::User);
        assert_eq!(value(&view, cx, ns), "Same as tasklens");
        let tip = cx.update(|_, cx| view.read(cx).tab.read(cx).tooltip(ns));
        assert_eq!(tip.as_deref(), Some(NS_TIP_SELF));
        assert_eq!(
            tip.as_deref(),
            Some("Compared with tasklens (PID 1 not readable)")
        );
        assert_eq!(
            value(&view, cx, SecField::Ns(NsKind::Pid)),
            "Isolated (4026532211)"
        );
        assert_eq!(value(&view, cx, SecField::Ns(NsKind::Net)), "Restricted");
    }

    /// A clipboard failure cannot be detected in v1 (R-199); the text the failed write shows
    /// is checked here.
    #[gpui_kit::test]
    fn security_copy_failure_toast_text(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let (failed, ok) = cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            (tab.copy_toast(Err(()), true), tab.copy_toast(Ok(()), true))
        });
        assert_eq!(failed.1.as_ref(), "Could not copy token summary value.");
        assert_eq!(ok.1.as_ref(), "Process property value copied.");
    }

    #[gpui_kit::test]
    fn caps_rows_exclude_bounding_only(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.caps = Arc::from(vec![
            cap(21, false, false, false, true, false, true),
            cap(13, true, false, false, true, false, false),
            cap(0, false, true, false, true, false, true),
        ]);
        feed(&view, cx, d);
        let ids = cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            tab.caps_table()
                .expect("table")
                .read(cx)
                .delegate
                .view_ids()
                .collect::<Vec<_>>()
        });
        assert_eq!(ids, [13, 0], "effective first; bounding-only omitted");
    }

    #[gpui_kit::test]
    fn caps_risky_name_colour(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.caps = Arc::from(vec![cap(21, true, true, false, true, false, true)]);
        feed(&view, cx, d);
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            let table = tab.caps_table().expect("table").read(cx);
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            table.delegate.cell(0, CapColumn::Name, &mut out);
            assert_eq!(out.text, "CAP_SYS_ADMIN");
            assert_eq!(out.tooltip.as_deref(), Some("Risky capability"));
            assert_eq!(
                out.text_color,
                Some(tl_ui::theme::current(cx).status.warning)
            );
        });
        let caps = cx
            .debug_bounds("SecurityCapabilities")
            .expect("capabilities region");
        let position = point(caps.left() + px(60.), caps.top() + px(24. + 28. + 13.));
        for dx in [0., 1., 2.] {
            cx.simulate_event(MouseMoveEvent {
                position: point(position.x + px(dx), position.y),
                pressed_button: None,
                modifiers: Modifiers::none(),
            });
            draw(cx);
        }
        cx.executor().advance_clock(DELAY_CONTROL * 2);
        draw(cx);
        assert!(
            cx.debug_bounds("TlTooltip").is_some(),
            "the drawn risky name shows its tooltip"
        );
    }

    #[cfg(feature = "test-hooks")]
    #[gpui_kit::test]
    fn security_cap_checks_are_lucide_size_and_centred(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.caps = Arc::from(vec![cap(13, true, true, true, true, true, false)]);
        feed(&view, cx, d);
        assert!(
            cx.debug_bounds("SecurityCapabilityChecks").is_some(),
            "the check-icon canvas is drawn"
        );
        let canvas = cx
            .debug_bounds("SecurityCapabilityChecks")
            .expect("check-icon canvas");
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            let table = tab.caps_table().expect("table").read(cx);
            let columns = [
                CapColumn::Effective,
                CapColumn::Permitted,
                CapColumn::Inheritable,
                CapColumn::Bounding,
                CapColumn::Ambient,
            ];
            let marks = table.delegate.check_icon_bounds.borrow();
            assert_eq!(marks.len(), 5, "one Lucide check for every set column");
            for (mark, col) in marks.iter().zip(columns) {
                assert_eq!(mark.size.width, px(CAP_CHECK_SIZE));
                assert_eq!(mark.size.height, px(CAP_CHECK_SIZE));
                let layout = table.delegate.columns();
                let pos = layout
                    .visible_columns()
                    .iter()
                    .position(|shown| *shown == col)
                    .expect("visible set column");
                assert_eq!(
                    mark.center().x,
                    canvas.left() + layout.offsets()[pos] + layout.width(col) / 2.,
                    "the Lucide check is centred in {col:?}"
                );
            }
            assert!(
                marks
                    .windows(2)
                    .all(|pair| pair[1].center().y == pair[0].center().y)
            );
        });
    }

    #[gpui_kit::test]
    fn groups_kind_marks(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.groups = Arc::from(vec![
            GroupMembershipRow {
                name: text("oystein"),
                gid: 1000,
                primary: true,
                root_equivalent: false,
                admin: false,
            },
            group("docker", 965, false, true),
            group("wheel", 998, true, false),
        ]);
        feed(&view, cx, d);
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            let d = &tab.groups_table().expect("table").read(cx).delegate;
            assert_eq!(d.kind(1000).as_deref(), Some("Primary"));
            assert_eq!(
                d.kind(965).as_deref(),
                Some("Supplementary (root-equivalent)")
            );
            assert_eq!(d.kind(998).as_deref(), Some("Supplementary (admin)"));
        });
    }

    #[gpui_kit::test]
    fn security_tables_none_reported(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            assert_eq!(
                tab.caps_table()
                    .expect("table")
                    .read(cx)
                    .delegate
                    .empty_text()
                    .as_deref(),
                Some("Collecting\u{2026}")
            );
            assert_eq!(
                tab.groups_table()
                    .expect("table")
                    .read(cx)
                    .delegate
                    .empty_text()
                    .as_deref(),
                Some("Collecting\u{2026}")
            );
        });
        assert!(cx.debug_bounds("TlTreeTable-empty").is_some());
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.groups = Arc::from(Vec::<GroupMembershipRow>::new());
        feed(&view, cx, d);
        cx.update(|_, cx| {
            let tab = view.read(cx).tab.read(cx);
            assert_eq!(
                tab.caps_table()
                    .expect("table")
                    .read(cx)
                    .delegate
                    .empty_text()
                    .as_deref(),
                Some("None reported")
            );
            assert_eq!(
                tab.groups_table()
                    .expect("table")
                    .read(cx)
                    .delegate
                    .empty_text()
                    .as_deref(),
                Some("None reported")
            );
        });
        assert!(cx.debug_bounds("TlTreeTable-empty").is_some());
    }

    #[gpui_kit::test]
    fn security_caps_ctrl_c_uses_yes(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let mut d = detail(ids(1000, 1000), names("oystein", "oystein"));
        d.caps = Arc::from(vec![cap(13, true, false, false, true, false, false)]);
        feed(&view, cx, d);
        let (table, focus) = cx.update(|_, cx| {
            let table = view
                .read(cx)
                .tab
                .read(cx)
                .caps_table()
                .expect("table")
                .clone();
            let focus = table.read(cx).focus_handle().clone();
            table.update(cx, |t, _| t.selection.click(13));
            (table, focus)
        });
        let _ = table;
        cx.update(|window, cx| focus.focus(window, cx));
        draw(cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("CAP_NET_RAW\tYes\t\t\tYes\t\tUse raw and packet sockets")
        );
    }
}
