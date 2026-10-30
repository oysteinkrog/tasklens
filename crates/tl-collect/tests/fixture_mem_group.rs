//! Tests for memory groups (D-036, UI-PRF-060). Each test builds a `/proc` tree in a tempdir,
//! reads it with the sweep's readers and groups the processes.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::Arc;

use rustix::fs::{Mode, OFlags};
use tl_collect::apps::cgroup_app::parse_app_unit;
use tl_collect::apps::mem_group::{MemGroup, MemGroupInput, MemGroupKind, MemGrouper};
use tl_collect::proc::cgroup::app_slice_unit;
use tl_collect::proc::meta::{ExeLink, MetaCache, RawMeta, ReadBufs};
use tl_collect::proc::pid_stat::{PidStat, is_kernel_thread, parse_pid_stat};
use tl_collect::proc::status::{PidStatus, parse_status};
use tl_model::ids::ProcessKey;
use tl_model::process::WineProcess;
use tl_model::time::BootId;

const UID: u32 = 1000;
const APP_SLICE: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice";

/// One process of a fixture tree.
struct Proc {
    pid: u32,
    ppid: u32,
    start_ticks: u64,
    uid: u32,
    comm: &'static str,
    /// `None` leaves out the `exe` link, as for a zombie.
    exe: Option<&'static str>,
    /// The `0::` cgroup path.
    cgroup: String,
    kernel_thread: bool,
}

fn user(
    pid: u32,
    ppid: u32,
    start: u64,
    comm: &'static str,
    exe: &'static str,
    unit: &str,
) -> Proc {
    Proc {
        pid,
        ppid,
        start_ticks: start,
        uid: UID,
        comm,
        exe: Some(exe),
        cgroup: format!("{APP_SLICE}/{unit}"),
        kernel_thread: false,
    }
}

fn write_tree(root: &Path, procs: &[Proc]) {
    for p in procs {
        let dir = root.join(p.pid.to_string());
        fs::create_dir_all(&dir).expect("create pid dir");
        // Field 9 is flags: 0x0020_0000 is PF_KTHREAD.
        let flags = if p.kernel_thread { 0x0020_0000 } else { 0 };
        fs::write(
            dir.join("stat"),
            format!(
                "{} ({}) S {} {} {} 0 -1 {flags} 0 0 0 0 0 0 0 0 20 0 1 0 {} 0 0 \
                 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
                p.pid, p.comm, p.ppid, p.pid, p.pid, p.start_ticks
            ),
        )
        .expect("write fixture file");
        fs::write(
            dir.join("status"),
            format!(
                "Name:\t{}\nUid:\t{u}\t{u}\t{u}\t{u}\nRssAnon:\t  1024 kB\n",
                p.comm,
                u = p.uid
            ),
        )
        .expect("write fixture file");
        fs::write(dir.join("comm"), format!("{}\n", p.comm)).expect("write fixture file");
        fs::write(dir.join("cmdline"), "").expect("write fixture file");
        fs::write(dir.join("cgroup"), format!("0::{}\n", p.cgroup)).expect("write fixture file");
        if let Some(exe) = p.exe {
            // The target need not exist: the reader only reads the link.
            symlink(exe, dir.join("exe")).expect("write exe link");
        }
    }
}

/// What the sweep knows about one process after reading its directory.
struct Read {
    pid: u32,
    stat: PidStat,
    ruid: u32,
    meta: RawMeta,
    app_id: Option<Arc<str>>,
    unit: Option<Arc<str>>,
}

fn read_tree(root: &Path, pids: &[u32]) -> Vec<Read> {
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    pids.iter()
        .map(|&pid| {
            let dir = root.join(pid.to_string());
            let mut stat = PidStat::default();
            parse_pid_stat(&fs::read(dir.join("stat")).expect("read stat"), &mut stat)
                .expect("parse stat");
            let mut status = PidStatus::default();
            parse_status(
                &fs::read(dir.join("status")).expect("read status"),
                &mut status,
            )
            .expect("parse status");
            let key = ProcessKey {
                start_ticks: stat.start_ticks,
                boot_id: BootId(1),
                pid,
            };
            let (meta, _) = cache
                .get_or_refresh_with(key, &stat.comm_raw, &mut bufs, || {
                    rustix::fs::open(&dir, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty()).ok()
                })
                .expect("pid dir opens");
            let meta = meta.clone();
            // `match_app` takes the app id of the app.slice unit first (E05-016), so a process
            // in an app scope is `GroupKind::App` with that id.
            let unit = meta
                .cgroup
                .as_ref()
                .and_then(|c| app_slice_unit(c, UID))
                .map(Arc::<str>::from);
            let app_id = unit.as_deref().and_then(parse_app_unit).map(|u| u.app_id);
            Read {
                pid,
                ruid: status.uid[0],
                stat,
                meta,
                app_id,
                unit,
            }
        })
        .collect()
}

fn app_name(app_id: &str) -> Option<&'static str> {
    match app_id {
        "google-chrome" => Some("Google Chrome"),
        "org.kde.konsole" => Some("Konsole"),
        _ => None,
    }
}

/// Groups the processes of `procs`, in their order.
fn groups(procs: &[Proc]) -> Vec<Arc<MemGroup>> {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_tree(tmp.path(), procs);
    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let read = read_tree(tmp.path(), &pids);
    let inputs: Vec<MemGroupInput<'_>> = read
        .iter()
        .map(|r| MemGroupInput {
            pid: r.pid,
            start_ticks: r.stat.start_ticks,
            ruid: r.ruid,
            kernel_thread: is_kernel_thread(&r.stat),
            comm: &r.meta.comm,
            exe: match &r.meta.exe {
                ExeLink::Path { path, .. } => Some(path),
                ExeLink::Restricted | ExeLink::None => None,
            },
            app_id: r.app_id.as_deref(),
            app_name: r.app_id.as_deref().and_then(app_name),
            unit: r.unit.as_deref(),
            wine: r.meta.wine.as_deref(),
        })
        .collect();
    let mut out = Vec::new();
    MemGrouper::new().classify(&inputs, UID, &mut out);
    out
}

fn keys(groups: &[Arc<MemGroup>]) -> Vec<&str> {
    groups.iter().map(|g| &*g.key).collect()
}

#[test]
fn mem_group_rules() {
    const CHROME: &str = "app-google\\x2dchrome-1234.scope";
    const KONSOLE: &str = "app-org.kde.konsole-5678.scope";
    let procs = [
        // Chrome: the browser, a crashpad handler and two renderers in one scope.
        user(100, 1, 1000, "chrome", "/opt/google/chrome/chrome", CHROME),
        user(
            101,
            100,
            1001,
            "chrome_crashpad",
            "/opt/google/chrome/chrome_crashpad_handler",
            CHROME,
        ),
        user(
            102,
            100,
            1002,
            "chrome",
            "/opt/google/chrome/chrome",
            CHROME,
        ),
        user(
            103,
            100,
            1003,
            "chrome",
            "/opt/google/chrome/chrome",
            CHROME,
        ),
        // Konsole with a shell and two Claude Code sessions inside its scope.
        user(200, 1, 2000, "konsole", "/usr/bin/konsole", KONSOLE),
        user(201, 200, 2001, "bash", "/usr/bin/bash", KONSOLE),
        user(
            202,
            201,
            2002,
            "2.1.309",
            "/home/u/.local/share/claude/versions/2.1.309",
            KONSOLE,
        ),
        // A second Claude Code session in its own scope, on an older version.
        user(
            300,
            1,
            3000,
            "2.1.300",
            "/home/u/.local/share/claude/versions/2.1.300",
            "claude-7.scope",
        ),
        // A zombie of the user: no exe link.
        Proc {
            exe: None,
            ..user(400, 1, 4000, "defunct-tool", "", "app-foo-1.scope")
        },
        // A root daemon and a kernel thread.
        Proc {
            uid: 0,
            cgroup: "/system.slice/sshd.service".into(),
            ..user(500, 1, 50, "sshd", "/usr/bin/sshd", "")
        },
        Proc {
            uid: 0,
            exe: None,
            ppid: 2,
            cgroup: "/".into(),
            kernel_thread: true,
            ..user(600, 2, 10, "kworker/0:1", "", "")
        },
    ];
    let g = groups(&procs);
    assert_eq!(
        keys(&g),
        [
            "app:google-chrome",
            "app:google-chrome",
            "app:google-chrome",
            "app:google-chrome",
            "app:org.kde.konsole",
            "exe:bash",
            "exe:claude",
            "exe:claude",
            "comm:defunct-tool",
            "system",
            "system",
        ]
    );

    // Chrome helpers are one group: the same `Arc`, named by the desktop entry.
    assert!(g[..4].iter().all(|x| Arc::ptr_eq(x, &g[0])));
    assert_eq!(&*g[0].name, "Google Chrome");
    assert_eq!(g[0].kind, MemGroupKind::App);
    assert_eq!(g[0].app_id.as_deref(), Some("google-chrome"));

    // The shell in the terminal's scope is not in the terminal's group.
    assert_eq!(&*g[4].name, "Konsole");
    assert_eq!(g[5].kind, MemGroupKind::Program);
    assert_eq!(&*g[5].name, "bash");
    assert_eq!(g[5].app_id, None);

    // The versions rule: both sessions are `claude`, shown as Claude Code.
    assert!(Arc::ptr_eq(&g[6], &g[7]));
    assert_eq!(&*g[6].name, "Claude Code");
    assert_eq!(g[6].kind, MemGroupKind::Program);

    // An unreadable executable falls back to `comm`.
    assert_eq!(g[8].kind, MemGroupKind::Unknown);
    assert_eq!(&*g[8].name, "defunct-tool");

    assert_eq!(&*g[9].name, "System");
    assert_eq!(g[10].kind, MemGroupKind::System);
}

#[test]
fn mem_group_oldest_process_sets_the_directory() {
    // A helper started first, from another directory, makes the scope's reference: the browser
    // that came later is then a program.
    const SCOPE: &str = "app-google\\x2dchrome-9.scope";
    let procs = [
        user(11, 1, 600, "chrome", "/opt/google/chrome/chrome", SCOPE),
        user(10, 1, 500, "launcher", "/usr/lib/launcher/launcher", SCOPE),
    ];
    let g = groups(&procs);
    assert_eq!(keys(&g), ["exe:chrome", "app:google-chrome"]);
}

#[test]
fn mem_group_bwrap_is_not_the_reference() {
    // A Flatpak scope starts with `bwrap`; the app's processes still form the app group.
    const SCOPE: &str = "app-flatpak-org.mozilla.firefox-42.scope";
    let procs = [
        user(20, 1, 100, "bwrap", "/usr/bin/bwrap", SCOPE),
        user(21, 20, 101, "firefox", "/app/lib/firefox/firefox", SCOPE),
        user(
            22,
            21,
            102,
            "Web Content",
            "/app/lib/firefox/firefox",
            SCOPE,
        ),
    ];
    let g = groups(&procs);
    assert_eq!(&*g[0].key, "exe:bwrap");
    assert!(g[1].key.starts_with("app:"));
    assert!(Arc::ptr_eq(&g[1], &g[2]));
}

#[test]
fn mem_group_keys_dropped_when_unused() {
    let mut grouper = MemGrouper::new();
    let mut out = Vec::new();
    let input = |exe| MemGroupInput {
        pid: 1,
        start_ticks: 1,
        ruid: UID,
        kernel_thread: false,
        comm: "x",
        exe: Some(exe),
        app_id: None,
        app_name: None,
        unit: None,
        wine: None,
    };
    grouper.classify(&[input("/usr/bin/a")], UID, &mut out);
    let first = out[0].clone();
    grouper.classify(&[input("/usr/bin/a")], UID, &mut out);
    assert!(Arc::ptr_eq(&first, &out[0]), "a steady group keeps its Arc");
    grouper.classify(&[input("/usr/bin/b")], UID, &mut out);
    grouper.classify(&[input("/usr/bin/a")], UID, &mut out);
    assert!(!Arc::ptr_eq(&first, &out[0]), "an unused group was dropped");
    assert_eq!(&*out[0].key, "exe:a");
}

/// The Wine prefix of the owner's Backblaze install (tasklens-o55.58).
const BACKBLAZE: &str = "/srv/backblaze/prefix";
const PRELOADER: &str = "/usr/lib/wine/x86_64-unix/wine-preloader";

/// A Wine process of the current user: (pid, prefix, Windows image).
fn wine_groups(procs: &[(u32, Option<&str>, &str)], exe: &str) -> Vec<Arc<MemGroup>> {
    let wines: Vec<WineProcess> = procs
        .iter()
        .map(|&(_, prefix, image)| WineProcess {
            prefix: prefix.map(Arc::from),
            image: Some(Arc::from(image)),
        })
        .collect();
    let inputs: Vec<MemGroupInput<'_>> = procs
        .iter()
        .zip(&wines)
        .map(|(&(pid, _, image), wine)| MemGroupInput {
            pid,
            start_ticks: u64::from(pid),
            ruid: UID,
            kernel_thread: false,
            comm: image.rsplit('\\').next().unwrap_or(image),
            exe: Some(exe),
            app_id: None,
            app_name: None,
            unit: None,
            wine: Some(wine),
        })
        .collect();
    let mut out = Vec::new();
    MemGrouper::new().classify(&inputs, UID, &mut out);
    out
}

const BACKBLAZE_IMAGES: [&str; 15] = [
    "C:\\Program Files\\Backblaze\\bzbui.exe",
    "C:\\Program Files\\Backblaze\\bzserv.exe",
    "C:\\Program Files\\Backblaze\\bzbuitray.exe",
    "C:\\Program Files\\Backblaze\\bztransmit.exe",
    "C:\\Program Files\\Backblaze\\bzfilelist.exe",
    "C:\\windows\\system32\\services.exe",
    "C:\\windows\\system32\\winedevice.exe",
    "C:\\windows\\system32\\winedevice.exe",
    "C:\\windows\\system32\\svchost.exe",
    "C:\\windows\\system32\\plugplay.exe",
    "C:\\windows\\explorer.exe",
    "C:\\windows\\system32\\rpcss.exe",
    "C:\\windows\\system32\\lsass.exe",
    "C:\\windows\\system32\\conhost.exe",
    "C:\\windows\\system32\\conhost.exe",
];

#[test]
fn wine_backblaze_is_one_group() {
    for exe in [PRELOADER, "/usr/lib/wine/x86_64-unix/wine64-preloader"] {
        let procs: Vec<_> = BACKBLAZE_IMAGES
            .iter()
            .enumerate()
            .map(|(i, &image)| (100 + i as u32, Some(BACKBLAZE), image))
            .collect();
        let g = wine_groups(&procs, exe);
        assert_eq!(g.len(), 15);
        assert!(
            g.iter().all(|x| Arc::ptr_eq(x, &g[0])),
            "one group: {:?}",
            keys(&g)
        );
        assert_eq!(&*g[0].key, "wine:/srv/backblaze/prefix:backblaze");
        assert_eq!(&*g[0].name, "Backblaze");
        assert_eq!(g[0].kind, MemGroupKind::Wine);
        assert_eq!(g[0].wine_prefix.as_deref(), Some(BACKBLAZE));
        assert_eq!(g[0].app_id, None);
    }
}

#[test]
fn wine_two_prefixes_stay_two_groups() {
    let a = "/home/u/.wine";
    let b = "/home/u/Games/other";
    let procs = [
        (1, Some(a), "C:\\Program Files\\Backblaze\\bzbui.exe"),
        (2, Some(a), "C:\\windows\\system32\\services.exe"),
        (3, Some(b), "C:\\Program Files\\Backblaze\\bzbui.exe"),
        (4, Some(b), "C:\\windows\\system32\\services.exe"),
    ];
    let g = wine_groups(&procs, PRELOADER);
    assert_eq!(
        keys(&g),
        [
            "wine:/home/u/.wine:backblaze",
            "wine:/home/u/.wine:backblaze",
            "wine:/home/u/Games/other:backblaze",
            "wine:/home/u/Games/other:backblaze",
        ]
    );
    assert!(!Arc::ptr_eq(&g[0], &g[2]));
    assert_eq!(&*g[2].name, "Backblaze");
}

#[test]
fn wine_prefix_without_app() {
    let procs = [
        (1, Some(BACKBLAZE), "C:\\windows\\system32\\services.exe"),
        (2, Some(BACKBLAZE), "C:\\windows\\system32\\winedevice.exe"),
        (3, None, "C:\\windows\\system32\\services.exe"),
    ];
    let g = wine_groups(&procs, PRELOADER);
    assert_eq!(&*g[0].key, "wine:/srv/backblaze/prefix");
    assert_eq!(&*g[0].name, "Wine (prefix)");
    assert!(Arc::ptr_eq(&g[0], &g[1]));
    assert_eq!(&*g[2].key, "wine:");
    assert_eq!(&*g[2].name, "Wine");
}

#[test]
fn wine_system_joins_the_app_with_most_processes() {
    let procs = [
        (1, Some(BACKBLAZE), "C:\\Program Files\\Tool\\tool.exe"),
        (
            2,
            Some(BACKBLAZE),
            "C:\\Program Files\\Backblaze\\bzbui.exe",
        ),
        (
            3,
            Some(BACKBLAZE),
            "C:\\Program Files\\Backblaze\\bzserv.exe",
        ),
        (4, Some(BACKBLAZE), "C:\\windows\\system32\\services.exe"),
    ];
    let g = wine_groups(&procs, PRELOADER);
    assert_eq!(&*g[0].name, "Tool");
    assert_eq!(&*g[3].key, "wine:/srv/backblaze/prefix:backblaze");
    // A tie goes to the folder that sorts first.
    let g = wine_groups(&[procs[0], procs[1], procs[3]], PRELOADER);
    assert_eq!(&*g[2].name, "Backblaze");
}

#[test]
fn wine_folder_names_ignore_case() {
    let procs = [
        (
            1,
            Some(BACKBLAZE),
            "C:\\Program Files\\Backblaze\\bzbui.exe",
        ),
        (
            2,
            Some(BACKBLAZE),
            "C:\\Program Files\\BACKBLAZE\\bzserv.exe",
        ),
        (
            3,
            Some(BACKBLAZE),
            "c:\\program files\\backblaze\\bztransmit.exe",
        ),
        (
            4,
            Some(BACKBLAZE),
            "C:\\Program Files\\Backblaze\\bzbuitray.exe",
        ),
        (5, Some(BACKBLAZE), "C:\\Program Files\\Tool\\a.exe"),
        (6, Some(BACKBLAZE), "C:\\Program Files\\Tool\\b.exe"),
        (7, Some(BACKBLAZE), "C:\\Program Files\\Tool\\c.exe"),
        (8, Some(BACKBLAZE), "C:\\windows\\system32\\services.exe"),
    ];
    let g = wine_groups(&procs, PRELOADER);
    // One group, named by the spelling most of its processes use.
    for x in &g[..4] {
        assert!(Arc::ptr_eq(x, &g[0]), "one group: {:?}", keys(&g));
    }
    assert_eq!(&*g[0].key, "wine:/srv/backblaze/prefix:backblaze");
    assert_eq!(&*g[0].name, "Backblaze");
    // Wine's own process joins Backblaze: four processes beat three of Tool, though no single
    // spelling of Backblaze has more than two.
    assert!(Arc::ptr_eq(&g[7], &g[0]));
    assert_eq!(&*g[4].name, "Tool");
    // Equal counts go to the spelling that sorts first.
    let g = wine_groups(&[procs[1], procs[2]], PRELOADER);
    assert!(Arc::ptr_eq(&g[0], &g[1]));
    assert_eq!(&*g[0].name, "BACKBLAZE");
}

#[test]
fn wine_process_of_another_user_is_system() {
    let wine = WineProcess {
        prefix: Some(Arc::from(BACKBLAZE)),
        image: Some(Arc::from("C:\\Program Files\\Backblaze\\bzbui.exe")),
    };
    let input = MemGroupInput {
        pid: 1,
        start_ticks: 1,
        ruid: UID + 1,
        kernel_thread: false,
        comm: "bzbui.exe",
        exe: None,
        app_id: None,
        app_name: None,
        unit: None,
        wine: Some(&wine),
    };
    let mut out = Vec::new();
    MemGrouper::new().classify(&[input], UID, &mut out);
    assert_eq!(&*out[0].key, "system");
}
