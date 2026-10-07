//! Process Properties window and tabs (E23, UI-PWN-003 to UI-PWN-009).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, Hsla, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::actions::SchedPolicy;
use tl_model::details::{
    ElfArch, ExeLocation, GeneralDetail, HashState, IdTriple, PackageCheck, PackageInfo, Relro,
    Seccomp, TabPayload,
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
