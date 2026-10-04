//! Tests for app naming (E05-018) and the desktop entry index (E05-013).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tl_collect::apps::desktop::{DesktopIndex, data_dirs_from_env, exec_binary, exec_binary_in};
use tl_collect::apps::naming::{
    NameInput, display_name, exe_basename, image_name, is_wine, search_lower,
};

fn argv(tokens: &[&str]) -> Vec<Arc<str>> {
    tokens.iter().map(|t| Arc::from(*t)).collect()
}

fn name(inp: &NameInput<'_>) -> String {
    let mut out = String::from("stale");
    display_name(inp, &mut out);
    out
}

fn plain<'a>(exe: Option<&'a str>, argv: &'a [Arc<str>], comm: &'a str) -> NameInput<'a> {
    NameInput {
        app_name: None,
        is_app_primary: false,
        exe,
        argv,
        comm,
        kernel_thread: false,
    }
}

#[test]
fn name_kernel_thread_brackets() {
    let inp = NameInput {
        kernel_thread: true,
        ..plain(None, &[], "kworker/0:1")
    };
    assert_eq!(name(&inp), "[kworker/0:1]");
}

#[test]
fn name_app_primary_uses_app_name() {
    let args = argv(&["/usr/lib/firefox/firefox"]);
    let inp = NameInput {
        app_name: Some("Firefox"),
        is_app_primary: true,
        ..plain(Some("/usr/lib/firefox/firefox"), &args, "firefox")
    };
    assert_eq!(name(&inp), "Firefox");
}

#[test]
fn name_comm_qualifier() {
    let args = argv(&["/usr/lib/firefox/firefox", "-contentproc"]);
    let inp = NameInput {
        app_name: Some("Firefox"),
        ..plain(Some("/usr/lib/firefox/firefox"), &args, "Web Content")
    };
    assert_eq!(name(&inp), "firefox (Web Content)");
}

#[test]
fn name_truncated_comm_no_qualifier() {
    let args = argv(&["/usr/bin/chromium-browser"]);
    let inp = plain(Some("/usr/bin/chromium-browser"), &args, "chromium-browse");
    assert_eq!(name(&inp), "chromium-browser");
}

#[test]
fn name_wine_exe() {
    let args = argv(&["C:\\Games\\Foo\\game.exe"]);
    let exe = Some("/usr/bin/wine64-preloader");
    let inp = plain(exe, &args, "game.exe");
    assert_eq!(name(&inp), "game.exe");

    let wine = is_wine(exe.map(exe_basename), "wine64-preloader");
    assert!(wine);
    assert_eq!(
        image_name(exe, &args, "wine64-preloader", wine),
        Some("game.exe")
    );
}

#[test]
fn name_restricted_exe_uses_argv() {
    let args = argv(&["sshd:", "user@pts/0"]);
    let inp = plain(None, &args, "sshd");
    assert_eq!(name(&inp), "sshd");
    assert_eq!(image_name(None, &args, "sshd", false), None);
}

#[test]
fn name_falls_back_to_comm() {
    let inp = plain(None, &[], "foo");
    assert_eq!(name(&inp), "foo");
}

#[test]
fn exe_basename_strips_deleted() {
    assert_eq!(exe_basename("/usr/bin/foo (deleted)"), "foo");
    assert_eq!(exe_basename("foo"), "foo");
}

#[test]
fn search_lower_order() {
    let parts = [
        "Firefox",
        "firefox",
        "/usr/lib/firefox/firefox -P",
        "alice",
        "",
        "Firefox",
        "app-firefox@1.service",
    ];
    let mut out = String::from("stale");
    search_lower(&parts, &mut out);
    assert_eq!(
        out,
        "firefox firefox /usr/lib/firefox/firefox -p alice firefox app-firefox@1.service"
    );
}

fn desktop_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apps-desktop");
    vec![
        root.join("home/.local/share/applications"),
        root.join("usr/share/applications"),
    ]
}

/// Built with an empty `PATH` list so relative binaries stay as is on every host.
fn desktop_index() -> DesktopIndex {
    DesktopIndex::build_in(&desktop_dirs(), &[])
}

#[test]
fn desktop_skip_rules() {
    let index = desktop_index();
    assert!(index.by_id("hidden").is_none());
    assert!(index.by_id("link").is_none());
    assert!(index.by_id("org.kde.konsole").is_some());
    assert_eq!(index.len(), 6);
}

#[test]
fn desktop_user_overrides_system() {
    let index = desktop_index();
    let firefox = index.by_id("firefox").expect("firefox indexed");
    assert_eq!(&*firefox.name, "Firefox (User)");
    assert!(firefox.path.ends_with("home/.local/share/applications/firefox.desktop"));
}

#[test]
fn desktop_subdir_id() {
    let index = desktop_index();
    let dolphin = index.by_id("kde-org.kde.dolphin").expect("dolphin indexed");
    assert_eq!(&*dolphin.name, "Dolphin");
    assert!(index.by_id("org.kde.dolphin").is_none());
}

#[test]
fn exec_binary_env_flatpak_quotes() {
    assert_eq!(
        exec_binary("env GDK_BACKEND=x11 /usr/bin/foo %F").as_deref(),
        Some("/usr/bin/foo")
    );

    let flatpak = "flatpak run --branch=stable --command=thunderbird org.mozilla.Thunderbird %U";
    assert_eq!(exec_binary_in(flatpak, &[]).as_deref(), Some("thunderbird"));
    let resolved = exec_binary(flatpak).expect("flatpak exec resolves");
    assert!(
        &*resolved == "thunderbird" || resolved.starts_with('/'),
        "got {resolved}"
    );
    let no_command = "flatpak run org.mozilla.Thunderbird %U";
    assert_eq!(
        exec_binary_in(no_command, &[]).as_deref(),
        Some("org.mozilla.Thunderbird")
    );

    assert_eq!(
        exec_binary(r#""/opt/My App/app" --x"#).as_deref(),
        Some("/opt/My App/app")
    );

    assert_eq!(exec_binary_in("snap run spotify %U", &[]).as_deref(), Some("spotify"));
    assert_eq!(exec_binary_in("%f %U", &[]), None);
    assert_eq!(exec_binary_in("", &[]), None);
}

#[test]
fn desktop_wm_class_case_insensitive() {
    let index = desktop_index();
    let firefox = index.by_wm_class("firefox").expect("wm class indexed");
    assert_eq!(&*firefox.id, "firefox");
    assert_eq!(index.by_wm_class("FIREFOX").map(|e| &*e.id), Some("firefox"));
    assert_eq!(index.by_wm_class("Konsole").map(|e| &*e.id), Some("org.kde.konsole"));
}

#[test]
fn desktop_values_sanitized() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    fs::write(
        apps.join("evil.desktop"),
        "[Desktop Entry]\nType=Application\nName=Safe\u{202E}exe.txt\nComment=a\u{1b}[31mb\nExec=/usr/bin/evil\n",
    )
    .expect("write desktop file");

    let index = DesktopIndex::build_in(&[apps], &[]);
    let evil = index.by_id("evil").expect("evil indexed");
    assert!(!evil.name.contains('\u{202E}'));
    assert_eq!(&*evil.name, "Safeexe.txt");
    let comment = evil.comment.as_deref().expect("comment kept");
    assert!(!comment.contains('\u{1b}'));
}

#[test]
fn data_dirs_fallback() {
    let home = Path::new("/home/user1");
    let defaults = vec![
        PathBuf::from("/home/user1/.local/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/usr/share/applications"),
    ];
    assert_eq!(data_dirs_from_env(home, None, None), defaults);
    assert_eq!(
        data_dirs_from_env(home, Some(OsStr::new("")), Some(OsStr::new(""))),
        defaults
    );
    assert_eq!(
        data_dirs_from_env(
            home,
            Some(OsStr::new("relative")),
            Some(OsStr::new("relative/path"))
        ),
        defaults
    );
    assert_eq!(
        data_dirs_from_env(
            home,
            Some(OsStr::new("/data")),
            Some(OsStr::new("/opt/share:relative:/usr/share"))
        ),
        vec![
            PathBuf::from("/data/applications"),
            PathBuf::from("/opt/share/applications"),
            PathBuf::from("/usr/share/applications"),
        ]
    );
}

#[test]
fn desktop_index_snapshot() {
    let index = desktop_index();
    let mut rows: Vec<_> = index
        .entries()
        .map(|e| {
            (
                e.id.to_string(),
                e.name.to_string(),
                e.exec_binary.as_deref().map(str::to_owned),
                e.icon.as_deref().map(str::to_owned),
            )
        })
        .collect();
    rows.sort();
    insta::assert_debug_snapshot!(rows);
}
