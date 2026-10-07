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

/// The Process Properties window over the real app globals and main window (E23-A1, UI-PWN-002
/// to UI-PWN-004, UI-PWN-018). Needs `--features test-hooks` for `AppEnv::test`.
#[cfg(feature = "test-hooks")]
mod window {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{AnyWindowHandle, WindowHandle};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::demand::DetailDemand;
    use tl_model::details::{DetailTab, DetailTarget, TargetDetail};
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::demand::GlobalDemand;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
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

    fn props(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Entity<PropsWindow> {
        cx.update(|cx| PropsWindow::find(handle, cx))
            .expect("props window")
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

    fn general_demand(pid: u32, tab: DetailTab) -> DetailDemand {
        DetailDemand {
            target: DetailTarget::Process(key(pid)),
            tab,
        }
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
}
