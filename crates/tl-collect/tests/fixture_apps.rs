//! Tests for app naming (E05-018), the desktop entry index (E05-013), the desktop watch
//! (E05-014), icon theme resolution (E05-019), the icon path cache (E05-020) and process
//! classification (E05-015).

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tl_collect::apps::classify::{ClassifyInput, ProcClass, classify, group_key};
use tl_collect::apps::desktop::{
    DesktopIndex, DesktopWatch, data_dirs_from_env, exec_binary, exec_binary_in,
};
use tl_collect::apps::icons::{
    GenericIcon, IconCache, ThemeDir, ThemeDirs, generic_icon, kde_icon_theme, load_theme, resolve,
    theme_order,
};
use tl_collect::apps::naming::{
    NameInput, display_name, exe_basename, image_name, is_wine, search_lower,
};
use tl_collect::proc::cgroup::{CgroupPath, parse_cgroup_file};
use tl_model::ids::GroupKey;
use tl_model::process::GroupKind;

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
    assert_eq!(index.len(), 8);
}

#[test]
fn desktop_user_overrides_system() {
    let index = desktop_index();
    let firefox = index.by_id("firefox").expect("firefox indexed");
    assert_eq!(&*firefox.name, "Firefox (User)");
    assert!(
        firefox
            .path
            .ends_with("home/.local/share/applications/firefox.desktop")
    );
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

    assert_eq!(
        exec_binary_in("snap run spotify %U", &[]).as_deref(),
        Some("spotify")
    );
    assert_eq!(exec_binary_in("%f %U", &[]), None);
    assert_eq!(exec_binary_in("", &[]), None);
}

#[test]
fn desktop_wm_class_case_insensitive() {
    let index = desktop_index();
    let firefox = index.by_wm_class("firefox").expect("wm class indexed");
    assert_eq!(&*firefox.id, "firefox");
    assert_eq!(
        index.by_wm_class("FIREFOX").map(|e| &*e.id),
        Some("firefox")
    );
    assert_eq!(
        index.by_wm_class("Konsole").map(|e| &*e.id),
        Some("org.kde.konsole")
    );
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

const MS: u64 = 1_000_000;

fn write_app(dir: &Path, id: &str) {
    fs::write(
        dir.join(format!("{id}.desktop")),
        format!("[Desktop Entry]\nType=Application\nName={id}\nExec=/usr/bin/{id}\n"),
    )
    .expect("write desktop file");
}

#[test]
fn desktop_watch_debounce_500ms() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    let mut watch = DesktopWatch::new(std::slice::from_ref(&apps)).expect("inotify watch");

    write_app(&apps, "one");
    assert!(!watch.poll(0));
    assert!(!watch.poll(100 * MS));
    assert!(!watch.poll(400 * MS));
    assert!(watch.poll(600 * MS));
    assert!(!watch.poll(700 * MS));

    // A second write restarts the debounce.
    write_app(&apps, "two");
    assert!(!watch.poll(1000 * MS));
    write_app(&apps, "three");
    assert!(!watch.poll(1450 * MS));
    assert!(!watch.poll(1600 * MS));
    assert!(watch.poll(1951 * MS));
    assert!(!watch.poll(2100 * MS));
}

#[test]
fn desktop_watch_debounce_restarts_on_second_write() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    let mut watch = DesktopWatch::new(std::slice::from_ref(&apps)).expect("inotify watch");

    write_app(&apps, "one");
    assert!(!watch.poll(0));
    write_app(&apps, "two");
    assert!(!watch.poll(450 * MS));
    assert!(!watch.poll(600 * MS));
    assert!(watch.poll(951 * MS));
}

#[test]
fn desktop_rebuild_finds_new_entry() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    write_app(&apps, "old");
    let dirs = vec![apps.clone()];
    let mut index = DesktopIndex::build(&dirs);
    let mut watch = DesktopWatch::new(&dirs).expect("inotify watch");
    let before = index.generation;

    write_app(&apps, "new");
    assert!(!index.rebuild_if(&mut watch, 0));
    assert!(index.by_id("new").is_none());
    assert!(index.rebuild_if(&mut watch, 600 * MS));
    assert!(index.by_id("new").is_some());
    assert!(index.by_id("old").is_some());
    assert_eq!(index.generation, before + 1);

    // A subdirectory created now is watched after the rebuild.
    let sub = apps.join("kde");
    fs::create_dir(&sub).expect("create subdir");
    assert!(!index.rebuild_if(&mut watch, 700 * MS));
    assert!(index.rebuild_if(&mut watch, 1200 * MS));
    assert_eq!(index.generation, before + 2);
    write_app(&sub, "nested");
    assert!(!index.rebuild_if(&mut watch, 1300 * MS));
    assert!(index.rebuild_if(&mut watch, 1800 * MS));
    assert!(index.by_id("kde-nested").is_some());
    assert_eq!(index.generation, before + 3);
}

#[test]
fn desktop_watch_missing_dir_ok() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    let missing = dir.path().join("missing/applications");
    let mut watch = DesktopWatch::new(&[missing, apps.clone()]).expect("missing dir skipped");
    write_app(&apps, "one");
    assert!(!watch.poll(0));
    assert!(watch.poll(500 * MS));
}

fn icons_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apps-icons")
}

fn icon_themes() -> Vec<ThemeDirs> {
    let root = icons_root();
    let bases = vec![
        root.join("home/.local/share/icons"),
        root.join("usr/share/icons"),
    ];
    let kdeglobals = fs::read(root.join("home/.config/kdeglobals")).expect("read kdeglobals");
    let user = kde_icon_theme(&kdeglobals);
    theme_order(user.as_deref())
        .iter()
        .filter_map(|name| load_theme(name, &bases))
        .collect()
}

fn icon_rel(path: Option<Arc<str>>) -> Option<String> {
    let path = path?;
    let base = icons_root().join("usr/share/icons");
    let rel = Path::new(&*path)
        .strip_prefix(&base)
        .expect("icon inside the fixture");
    Some(rel.to_str().expect("utf-8 path").to_owned())
}

#[test]
fn icons_kdeglobals_theme() {
    let kdeglobals =
        fs::read(icons_root().join("home/.config/kdeglobals")).expect("read kdeglobals");
    assert_eq!(kde_icon_theme(&kdeglobals).as_deref(), Some("breeze-dark"));
    assert_eq!(kde_icon_theme(b"[General]\nTheme=other\n"), None);
    assert_eq!(kde_icon_theme(b"[Icons]\n#Theme=old\nTheme=\n"), None);
    assert_eq!(
        theme_order(Some("breeze-dark")).as_slice(),
        ["breeze-dark", "breeze", "hicolor"]
    );
    assert_eq!(
        theme_order(Some("breeze")).as_slice(),
        ["breeze", "hicolor"]
    );
    assert_eq!(theme_order(None).as_slice(), ["breeze", "hicolor"]);
}

#[test]
fn icons_resolve_user_theme_first() {
    let themes = icon_themes();
    let names: Vec<_> = themes.iter().map(|t| t.theme.as_str()).collect();
    assert_eq!(names, ["breeze-dark", "breeze", "hicolor"]);
    assert_eq!(
        themes[0].dirs[2],
        ThemeDir {
            rel: "apps/scalable".into(),
            size: 48,
            scale: 1,
            scalable: true,
        }
    );
    assert_eq!(
        icon_rel(resolve("firefox", 22, &themes)).as_deref(),
        Some("breeze-dark/apps/22/firefox.svg")
    );
}

#[test]
fn icons_exact_size_before_scalable() {
    let themes = icon_themes();
    assert_eq!(
        icon_rel(resolve("konsole", 24, &themes)).as_deref(),
        Some("hicolor/24x24/apps/konsole.png")
    );
}

#[test]
fn icons_size_fallback_order() {
    let themes = icon_themes();
    assert_eq!(
        icon_rel(resolve("konsole", 32, &themes)).as_deref(),
        Some("hicolor/24x24/apps/konsole.png")
    );
    assert_eq!(
        icon_rel(resolve("only16", 24, &themes)).as_deref(),
        Some("hicolor/16x16/apps/only16.png")
    );
}

#[test]
fn icons_absolute_path() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let icon = dir.path().join("icon.png");
    fs::write(&icon, b"").expect("write icon");
    let icon = icon.to_str().expect("utf-8 path");
    assert_eq!(resolve(icon, 24, &[]).as_deref(), Some(icon));

    let missing = dir.path().join("missing.png");
    assert_eq!(
        resolve(missing.to_str().expect("utf-8 path"), 24, &[]),
        None
    );
}

#[test]
fn icons_unknown_none() {
    let themes = icon_themes();
    assert_eq!(resolve("no-such-icon", 24, &themes), None);
    assert_eq!(resolve("../hicolor/24x24/apps/konsole", 24, &themes), None);
    assert_eq!(resolve("", 24, &themes), None);
    assert!(load_theme("no-such-theme", &[icons_root().join("usr/share/icons")]).is_none());
    assert!(load_theme("..", &[icons_root().join("usr/share/icons")]).is_none());
    assert_eq!(
        generic_icon(GenericIcon::Executable),
        "application-x-executable"
    );
    assert_eq!(generic_icon(GenericIcon::KernelThread), "cpu");
    assert_eq!(
        generic_icon(GenericIcon::Service),
        "preferences-system-services"
    );
}

fn firefox_path() -> String {
    icons_root()
        .join("usr/share/icons/breeze-dark/apps/22/firefox.svg")
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

/// Writes a cache with `firefox@24` through `IconCache` and returns its path.
fn written_cache(dir: &Path) -> PathBuf {
    let path = dir.join("cache/tasklens/icons.toml");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    let themes = icon_themes();
    assert_eq!(
        cache.lookup("firefox", 24, &themes).as_deref(),
        Some(firefox_path().as_str())
    );
    cache.save().expect("save cache");
    path
}

#[test]
fn icon_cache_write_and_mode() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = written_cache(dir.path());
    let text = fs::read_to_string(&path).expect("read cache");
    assert!(text.contains("version = 1\n"), "{text}");
    assert!(text.contains("theme = \"breeze-dark\"\n"), "{text}");
    assert!(
        text.contains(&format!("\"firefox@24\" = \"{}\"\n", firefox_path())),
        "{text}"
    );
    let mode = fs::metadata(&path)
        .expect("stat cache")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let parent = path.parent().expect("parent");
    let mode = fs::metadata(parent).expect("stat dir").permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    let names: Vec<_> = fs::read_dir(parent)
        .expect("list dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, [OsStr::new("icons.toml")]);
}

#[test]
fn icon_cache_hit_skips_resolve() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = written_cache(dir.path());
    let mut cache = IconCache::load(path, "breeze-dark");
    assert_eq!(
        cache.lookup("firefox", 24, &[]).as_deref(),
        Some(firefox_path().as_str())
    );
}

#[test]
fn icon_cache_theme_change_discards() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = written_cache(dir.path());
    let mut cache = IconCache::load(path.clone(), "breeze");
    assert_eq!(cache.lookup("firefox", 24, &[]), None);
    cache.save().expect("save cache");
    let text = fs::read_to_string(&path).expect("read cache");
    assert!(text.contains("theme = \"breeze\"\n"), "{text}");
    assert!(!text.contains(&firefox_path()), "{text}");
}

#[test]
fn icon_cache_stale_path_reresolved() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path().join("icons.toml");
    let gone = dir.path().join("gone.svg");
    fs::write(
        &path,
        format!(
            "version = 1\ntheme = \"breeze-dark\"\n[paths]\n\"firefox@24\" = \"{}\"\n",
            gone.display()
        ),
    )
    .expect("write cache");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    assert_eq!(
        cache.lookup("firefox", 24, &icon_themes()).as_deref(),
        Some(firefox_path().as_str())
    );
    cache.save().expect("save cache");
    let text = fs::read_to_string(&path).expect("read cache");
    assert!(text.contains(&firefox_path()), "{text}");
}

#[test]
fn icon_cache_corrupt_starts_empty() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path().join("icons.toml");
    fs::write(
        &path,
        format!(
            "version = 1\ntheme = \"breeze-dark\"\n[paths\n\"firefox@24\" = \"{}\"\n",
            firefox_path()
        ),
    )
    .expect("write cache");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    assert_eq!(cache.lookup("firefox", 24, &[]), None);
    cache.save().expect("save cache");
    let text = fs::read_to_string(&path).expect("read cache");
    assert!(text.contains("\"firefox@24\" = \"\"\n"), "{text}");
}

#[test]
fn icon_cache_quote_roundtrip() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let icon = dir.path().join("we\"ird\\name.png");
    fs::write(&icon, b"").expect("write icon");
    let icon = icon.to_str().expect("utf-8 path");
    let path = dir.path().join("icons.toml");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    assert_eq!(cache.lookup(icon, 16, &[]).as_deref(), Some(icon));
    // A control character drops the entry when writing.
    let bad = dir.path().join("tab\there.png");
    fs::write(&bad, b"").expect("write icon");
    let bad = bad.to_str().expect("utf-8 path");
    assert_eq!(cache.lookup(bad, 16, &[]).as_deref(), Some(bad));
    cache.save().expect("save cache");

    let text = fs::read_to_string(&path).expect("read cache");
    assert!(text.contains("we\\\"ird\\\\name.png@16"), "{text}");
    assert!(!text.contains("tab"), "{text}");
    let mut cache = IconCache::load(path, "breeze-dark");
    assert_eq!(cache.lookup(icon, 16, &[]).as_deref(), Some(icon));
}

#[test]
fn icon_cache_negative_entry() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path().join("icons.toml");
    fs::write(
        &path,
        "version = 1\ntheme = \"breeze-dark\"\n[paths]\n\"firefox@24\" = \"\"\n",
    )
    .expect("write cache");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    assert_eq!(cache.lookup("firefox", 24, &icon_themes()), None);
}

#[test]
fn icon_cache_no_write_when_clean() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = written_cache(dir.path());
    let before = fs::metadata(&path).expect("stat cache");
    let mut cache = IconCache::load(path.clone(), "breeze-dark");
    assert!(cache.lookup("firefox", 24, &[]).is_some());
    cache.save().expect("save cache");
    let after = fs::metadata(&path).expect("stat cache");
    assert_eq!(after.ino(), before.ino());
    assert_eq!(after.mtime_nsec(), before.mtime_nsec());
    assert_eq!(after.mtime(), before.mtime());
}

// ---- Classification and GroupKey (E05-015) ----

const SELF_UID: u32 = 1000;

fn cgroup(path: &str) -> CgroupPath {
    parse_cgroup_file(format!("0::/{path}\n").as_bytes()).expect("cgroup path")
}

fn class_input<'a>(cg: Option<&'a CgroupPath>, app_id: Option<&'a Arc<str>>) -> ClassifyInput<'a> {
    ClassifyInput {
        pid: 4242,
        ruid: SELF_UID,
        kernel_thread: false,
        cgroup: cg,
        cgroup_denied: false,
        app_id,
        owns_window: false,
    }
}

#[test]
fn classify_kernel_thread() {
    let cg = cgroup("");
    let inp = ClassifyInput {
        ruid: 0,
        kernel_thread: true,
        ..class_input(Some(&cg), None)
    };
    let (class, kind) = classify(&inp, SELF_UID);
    assert_eq!((class, kind), (ProcClass::KernelThread, GroupKind::System));
    assert_eq!(class.type_label(), "Kernel thread");
}

#[test]
fn classify_other_uid_is_system() {
    let cg = cgroup("user.slice/user-1000.slice/user@1000.service/app.slice/foo.service");
    let inp = ClassifyInput {
        ruid: 0,
        ..class_input(Some(&cg), None)
    };
    let (class, kind) = classify(&inp, SELF_UID);
    assert_eq!((class, kind), (ProcClass::System, GroupKind::System));
    assert_eq!(class.type_label(), "System process");
}

#[test]
fn classify_system_slice_is_system() {
    let cg = cgroup("system.slice/pipewire.service");
    let inp = class_input(Some(&cg), None);
    assert_eq!(
        classify(&inp, SELF_UID),
        (ProcClass::System, GroupKind::System)
    );
    // Another user's slice is System too, even with our ruid.
    let cg = cgroup("user.slice/user-1001.slice/user@1001.service/app.slice/foo.service");
    let inp = class_input(Some(&cg), None);
    assert_eq!(
        classify(&inp, SELF_UID),
        (ProcClass::System, GroupKind::System)
    );
}

#[test]
fn classify_app_scope_is_app() {
    let cg =
        cgroup("user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@3a1b.service");
    let app: Arc<str> = Arc::from("firefox");
    let inp = class_input(Some(&cg), Some(&app));
    let (class, kind) = classify(&inp, SELF_UID);
    assert_eq!(
        (class, kind),
        (
            ProcClass::App,
            GroupKind::App {
                app_id: Arc::from("firefox")
            }
        )
    );
    assert_eq!(class.type_label(), "App");
}

#[test]
fn classify_session_slice_is_background() {
    for path in [
        "user.slice/user-1000.slice/user@1000.service/session.slice/plasma-kwin_wayland.service",
        "user.slice/user-1000.slice/user@1000.service/init.scope",
        "user.slice/user-1000.slice/session-2.scope",
    ] {
        let cg = cgroup(path);
        let inp = class_input(Some(&cg), None);
        let (class, kind) = classify(&inp, SELF_UID);
        assert_eq!(
            (class, kind),
            (ProcClass::Background, GroupKind::Background),
            "{path}"
        );
        assert_eq!(class.type_label(), "Background process");
    }
}

#[test]
fn classify_autostart_without_window_is_background() {
    let cg =
        cgroup("user.slice/user-1000.slice/user@1000.service/app.slice/app-foo@autostart.service");
    let inp = class_input(Some(&cg), None);
    assert_eq!(
        classify(&inp, SELF_UID),
        (ProcClass::Background, GroupKind::Background)
    );
}

#[test]
fn classify_window_in_app_slice_unit_is_app() {
    let with_window = |path: &str| {
        let cg = cgroup(path);
        let inp = ClassifyInput {
            owns_window: true,
            ..class_input(Some(&cg), None)
        };
        classify(&inp, SELF_UID)
    };
    let base = "user.slice/user-1000.slice/user@1000.service/app.slice/";
    for (unit, app_id) in [
        ("app-foo@autostart.service", "foo"),
        ("app-chrome\\x2dwork@00bca6.service/sub", "chrome-work"),
        (
            "app-gnome-org.gnome.Nautilus-777.scope",
            "gnome-org.gnome.Nautilus",
        ),
    ] {
        assert_eq!(
            with_window(&format!("{base}{unit}")),
            (
                ProcClass::App,
                GroupKind::App {
                    app_id: Arc::from(app_id)
                }
            ),
            "{unit}"
        );
    }
    // A unit that does not parse, or a window outside app.slice, stays Background.
    for path in [
        format!("{base}dconf.service"),
        format!("{base}app-x.slice/app-y.scope"),
        "user.slice/user-1000.slice/session-2.scope".to_owned(),
    ] {
        assert_eq!(
            with_window(&path),
            (ProcClass::Background, GroupKind::Background),
            "{path}"
        );
    }
}

#[test]
fn classify_unreadable_cgroup_ungrouped() {
    let cg = cgroup("user.slice/user-1000.slice/session-2.scope");
    let denied = ClassifyInput {
        cgroup_denied: true,
        ..class_input(Some(&cg), None)
    };
    assert_eq!(
        classify(&denied, SELF_UID),
        (ProcClass::Background, GroupKind::Ungrouped)
    );
    let missing = class_input(None, None);
    assert_eq!(
        classify(&missing, SELF_UID),
        (ProcClass::Background, GroupKind::Ungrouped)
    );
}

#[test]
fn group_key_stable() {
    let app = GroupKind::App {
        app_id: Arc::from("firefox"),
    };
    assert_eq!(group_key(&app), group_key(&app.clone()));
    assert_eq!(group_key(&app), app.key());
    let keys = [
        group_key(&app),
        group_key(&GroupKind::Background),
        group_key(&GroupKind::System),
        group_key(&GroupKind::Ungrouped),
    ];
    // History (ADR-012) stores these values. A change here breaks stored history.
    assert_eq!(
        keys,
        [
            GroupKey(8_670_821_890_558_686_982),
            GroupKey(5_869_058_164_875_963_351),
            GroupKey(18_026_959_284_135_166_402),
            GroupKey(11_738_116_329_684_817_838),
        ]
    );
}

// ---- App id from app.slice unit names (E05-011) ----

use tl_collect::apps::cgroup_app::{
    AppSliceScan, AppUnitKind, is_not_app, parse_app_unit, scan_app_slice,
};
use tl_collect::proc::cgroup::CgroupRoot;
use tl_collect::proc::meta::ReadBufs;

fn pair(a: &str, b: &str) -> Option<(Arc<str>, Arc<str>)> {
    Some((Arc::from(a), Arc::from(b)))
}

#[test]
fn app_unit_kde_service_unescaped() {
    let id = parse_app_unit("app-chrome\\x2dwork@00bca6.service").expect("parses");
    assert_eq!(&*id.app_id, "chrome-work");
    assert_eq!(id.kind, AppUnitKind::KdeService);
    assert_eq!(id.alt, None);
    assert!(!id.autostart);
}

#[test]
fn app_unit_scope_with_launcher() {
    let id = parse_app_unit("app-gnome-org.gnome.Nautilus-777.scope").expect("parses");
    assert_eq!(&*id.app_id, "gnome-org.gnome.Nautilus");
    assert_eq!(id.alt, pair("gnome", "org.gnome.Nautilus"));
    assert_eq!(id.kind, AppUnitKind::Scope);
    // An `@<instance>` part is removed before the split.
    let id = parse_app_unit("app-kde-org.kde.dolphin@x-9.scope").expect("parses");
    assert_eq!(&*id.app_id, "kde-org.kde.dolphin");
    assert_eq!(id.alt, pair("kde", "org.kde.dolphin"));
}

#[test]
fn app_unit_scope_without_launcher() {
    let id = parse_app_unit("app-org.kde.konsole-12345.scope").expect("parses");
    assert_eq!(&*id.app_id, "org.kde.konsole");
    assert_eq!(id.alt, None);
    assert_eq!(id.kind, AppUnitKind::Scope);
    assert_eq!(parse_app_unit("app-konsole.scope"), None);
}

#[test]
fn app_unit_flatpak() {
    let id = parse_app_unit("flatpak-org.mozilla.Thunderbird-4321.scope").expect("parses");
    assert_eq!(&*id.app_id, "org.mozilla.Thunderbird");
    assert_eq!(id.kind, AppUnitKind::Flatpak);
}

#[test]
fn app_unit_snap() {
    let id = parse_app_unit("snap.code.code-0f1e.scope").expect("parses");
    assert_eq!(&*id.app_id, "code_code");
    assert_eq!(id.kind, AppUnitKind::Snap);
    let id =
        parse_app_unit("snap.snap-store.snap-store-0e3a6b1c-2f4d-4a5b-9c8d-7e6f5a4b3c2d.scope")
            .expect("parses");
    assert_eq!(&*id.app_id, "snap-store_snap-store");
}

#[test]
fn app_unit_dbus() {
    let id = parse_app_unit("dbus-:1.2-org.kde.kded6@0.service").expect("parses");
    assert_eq!(&*id.app_id, "org.kde.kded6");
    assert_eq!(id.kind, AppUnitKind::DbusActivated);
}

#[test]
fn app_unit_autostart() {
    let id = parse_app_unit("app-org.kde.kalendarac@autostart.service").expect("parses");
    assert_eq!(&*id.app_id, "org.kde.kalendarac");
    assert!(id.autostart);
}

#[test]
fn not_app_units() {
    for unit in [
        "dconf.service",
        "obex.service",
        "dbus.socket",
        "frankenterm-mux.service",
        "run-u12.scope",
    ] {
        assert!(is_not_app(unit), "{unit}");
    }
    for unit in [
        "app-firefox@3a1b.service",
        "flatpak-org.mozilla.Thunderbird-4321.scope",
        "dbus-:1.2-org.kde.kded6@0.service",
        "snap.code.code-0f1e.scope",
    ] {
        assert!(!is_not_app(unit), "{unit}");
    }
    assert_eq!(parse_app_unit("frankenterm-mux.service"), None);
}

#[test]
fn scan_app_slice_fixture() {
    let sys = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process-edge/sys");
    let cg = CgroupRoot::open(&sys).expect("open cgroup root");
    let mut bufs = ReadBufs::default();
    let mut scan = AppSliceScan::default();
    scan_app_slice(&cg, 1000, &mut bufs, &mut scan);
    let slice = "user.slice/user-1000.slice/user@1000.service/app.slice/";
    let got: Vec<(&str, &str, Vec<u32>)> = scan
        .units
        .iter()
        .map(|u| {
            let mut pids = u.pids.to_vec();
            pids.sort_unstable();
            (
                u.rel.strip_prefix(slice).expect("in app.slice"),
                &*u.id.app_id,
                pids,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "app-chrome\\x2dwork@00bca6.service",
                "chrome-work",
                vec![5101, 5102]
            ),
            ("app-firefox@3a1b.service", "firefox", vec![4201, 4210]),
            (
                "app-gnome-org.gnome.Nautilus-777.scope",
                "gnome-org.gnome.Nautilus",
                vec![5301]
            ),
            (
                "app-org.kde.kalendarac@autostart.service",
                "org.kde.kalendarac",
                vec![5601]
            ),
            (
                "app-org.kde.konsole-12345.scope",
                "org.kde.konsole",
                vec![5201]
            ),
            (
                "flatpak-org.mozilla.Thunderbird-4321.scope",
                "org.mozilla.Thunderbird",
                vec![5401, 5402]
            ),
            ("snap.code.code-0f1e.scope", "code_code", vec![5501]),
        ]
    );
    for u in &scan.units {
        let meta = fs::metadata(sys.join("fs/cgroup").join(&*u.rel)).expect("unit dir");
        assert_eq!(u.cgroup_id, meta.ino());
    }
    // Another uid has no app.slice in the fixture.
    scan_app_slice(&cg, 1001, &mut bufs, &mut scan);
    assert!(scan.units.is_empty());
}

fn bwrap_key(pid: u32, start_ticks: u64) -> tl_model::ids::ProcessKey {
    tl_model::ids::ProcessKey {
        start_ticks,
        boot_id: tl_model::time::BootId(1),
        pid,
    }
}

fn bwrap_record(
    pid: u32,
    ppid: u32,
    start_ticks: u64,
    comm: &str,
) -> tl_model::process::ProcessRecord {
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::validity::FieldValidity;
    ProcessRecord {
        key: bwrap_key(pid, start_ticks),
        ppid,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: comm.into(),
            name: comm.into(),
            exe: Some(format!("/usr/bin/{comm}").into()),
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            search_lower: comm.into(),
        }),
        group: GroupKind::Background.key(),
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
        validity: FieldValidity::default(),
    }
}

fn unwrap_targets(
    roots: &[tl_model::ids::ProcessKey],
    records: &[tl_model::process::ProcessRecord],
) -> Vec<tl_model::ids::ProcessKey> {
    use tl_collect::apps::cgroup_app::{is_bwrap, unwrap_bwrap_targets};
    unwrap_bwrap_targets(roots, records, |r| {
        let base = r.meta.exe.as_deref().map(exe_basename);
        is_bwrap(base, &r.meta.comm)
    })
    .to_vec()
}

#[test]
fn bwrap_chain_unwrapped() {
    let records = [
        bwrap_record(10, 1, 100, "bwrap"),
        bwrap_record(11, 10, 101, "bwrap"),
        bwrap_record(12, 11, 102, "thunderbird"),
    ];
    assert_eq!(
        unwrap_targets(&[bwrap_key(10, 100)], &records),
        [bwrap_key(12, 102)]
    );
}

#[test]
fn bwrap_branches_each_unwrapped() {
    let records = [
        bwrap_record(10, 1, 100, "bwrap"),
        bwrap_record(11, 10, 101, "bwrap"),
        bwrap_record(12, 11, 102, "app"),
        bwrap_record(13, 10, 103, "helper"),
    ];
    assert_eq!(
        unwrap_targets(&[bwrap_key(10, 100)], &records),
        [bwrap_key(12, 102), bwrap_key(13, 103)]
    );
    // A root named twice, or reached through another root, is returned once.
    assert_eq!(
        unwrap_targets(
            &[bwrap_key(10, 100), bwrap_key(12, 102), bwrap_key(10, 100)],
            &records
        ),
        [bwrap_key(12, 102), bwrap_key(13, 103)]
    );
    // A wrapper with only wrapper descendants is dropped.
    let only = [
        bwrap_record(10, 1, 100, "bwrap"),
        bwrap_record(11, 10, 101, "bwrap"),
    ];
    assert!(unwrap_targets(&[bwrap_key(10, 100)], &only).is_empty());
}

#[test]
fn non_bwrap_root_unchanged() {
    let records = [
        bwrap_record(20, 1, 200, "firefox"),
        bwrap_record(21, 20, 201, "firefox"),
    ];
    assert_eq!(
        unwrap_targets(&[bwrap_key(20, 200)], &records),
        [bwrap_key(20, 200)]
    );
    // The exe decides; comm counts only when the exe is unreadable.
    use tl_collect::apps::cgroup_app::is_bwrap;
    assert!(is_bwrap(Some("bwrap"), "x"));
    assert!(!is_bwrap(Some("firefox"), "bwrap"));
    assert!(is_bwrap(None, "bwrap"));
    assert!(!is_bwrap(None, "firefox"));
}

/// Security regression (ADR-006): a record whose pid was reused before the wrapper started
/// is not the wrapper's child and must never become a target.
#[test]
fn bwrap_reused_pid_not_followed() {
    let records = [
        bwrap_record(10, 1, 100, "bwrap"),
        bwrap_record(14, 10, 50, "stale"),
        bwrap_record(12, 10, 102, "app"),
    ];
    assert_eq!(
        unwrap_targets(&[bwrap_key(10, 100)], &records),
        [bwrap_key(12, 102)]
    );
    let stale_only = [
        bwrap_record(10, 1, 100, "bwrap"),
        bwrap_record(14, 10, 50, "stale"),
    ];
    assert!(unwrap_targets(&[bwrap_key(10, 100)], &stale_only).is_empty());
}

#[test]
fn children_file_parsed() {
    use rustix::fd::AsFd;
    use smallvec::SmallVec;
    use tl_collect::apps::cgroup_app::children_from_proc;
    let proc = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process-edge/proc");
    let dir = fs::File::open(proc.join("10")).expect("open /proc/10 fixture");
    let mut buf = Vec::new();
    let mut out: SmallVec<[u32; 8]> = SmallVec::new();
    children_from_proc(dir.as_fd(), 10, &mut buf, &mut out);
    assert_eq!(out.as_slice(), [11, 13]);
    // No children file: out is cleared and stays empty.
    let other = fs::File::open(proc.join("57")).expect("open /proc/57 fixture");
    children_from_proc(other.as_fd(), 57, &mut buf, &mut out);
    assert!(out.is_empty());
}

// ---- App matching (E05-016) ----

use tl_collect::apps::cgroup_app::UnitAppId;
use tl_collect::apps::classify::{MatchInput, match_app, qualifier};

fn match_input<'a>(exe: Option<&'a str>, cmdline: Option<&'a str>) -> MatchInput<'a> {
    MatchInput {
        unit_app: None,
        exe,
        argv0: cmdline.and_then(|c| c.split_ascii_whitespace().next()),
        cmdline,
        wm_class: None,
    }
}

fn matched_id(inp: &MatchInput<'_>, index: &DesktopIndex) -> Option<String> {
    match_app(inp, index).map(|e| e.id.to_string())
}

#[test]
fn match_by_unit_app_id() {
    let index = desktop_index();
    let unit: UnitAppId = parse_app_unit("app-firefox@1234.service").expect("unit parses");
    let inp = MatchInput {
        unit_app: Some(&unit),
        ..match_input(
            Some("/usr/lib/firefox/firefox"),
            Some("/usr/lib/firefox/firefox"),
        )
    };
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("firefox"));
    assert_eq!(qualifier(&inp, Some(&unit)), None);

    // A Flatpak unit also matches the entry whose X-Flatpak equals its id.
    let flatpak = parse_app_unit("flatpak-org.mozilla.Thunderbird-12345.scope").expect("parses");
    let inp = MatchInput {
        unit_app: Some(&flatpak),
        ..match_input(Some("/app/lib/thunderbird/thunderbird"), None)
    };
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("flatpak-tb"));
    assert_eq!(qualifier(&inp, Some(&flatpak)), Some("Flatpak"));
}

#[test]
fn match_exception_firefox_bin() {
    let index = desktop_index();
    let inp = match_input(
        Some("/usr/lib/firefox/firefox-bin"),
        Some("/usr/lib/firefox/firefox-bin"),
    );
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("firefox"));
}

#[test]
fn match_disambiguate_profile_directory() {
    let index = desktop_index();
    assert_eq!(index.by_exe("/opt/google/chrome/chrome").len(), 2);
    let inp = match_input(
        Some("/opt/google/chrome/chrome"),
        Some("/opt/google/chrome/chrome --profile-directory=Work --no-first-run"),
    );
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("chrome-work"));
}

#[test]
fn match_true_tie_unmatched() {
    let index = desktop_index();
    let inp = match_input(
        Some("/opt/google/chrome/chrome"),
        Some("/opt/google/chrome/chrome --type=renderer"),
    );
    assert_eq!(matched_id(&inp, &index), None);
}

#[test]
fn match_wine_exe() {
    let index = desktop_index();
    let inp = MatchInput {
        unit_app: None,
        exe: Some("/usr/bin/wine64-preloader"),
        argv0: Some(r"C:\Games\Foo\foo.exe"),
        cmdline: Some(r"C:\Games\Foo\foo.exe"),
        wm_class: None,
    };
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("foo-wine"));
    assert_eq!(qualifier(&inp, None), Some("Wine"));

    // The .exe name in a later argv token, matched case-insensitively.
    let inp = MatchInput {
        argv0: Some("/usr/bin/wine"),
        cmdline: Some(r"/usr/bin/wine Z:\home\u\FOO.EXE"),
        ..inp
    };
    assert_eq!(matched_id(&inp, &index).as_deref(), Some("foo-wine"));
}

#[test]
fn match_stay_background_and_ignore() {
    let index = desktop_index();
    let xwayland = match_input(Some("/usr/bin/Xwayland"), Some("/usr/bin/Xwayland :0"));
    assert_eq!(matched_id(&xwayland, &index), None);
    let foot_server = match_input(Some("/usr/bin/foot"), Some("foot --server"));
    assert_eq!(matched_id(&foot_server, &index), None);
    let guake = match_input(Some("/usr/bin/guake-prefs"), Some("guake-prefs"));
    assert_eq!(matched_id(&guake, &index), None);

    // A STAY_BACKGROUND exe is not matched even inside an app unit.
    let unit = parse_app_unit("app-firefox@1234.service").expect("unit parses");
    let portal = MatchInput {
        unit_app: Some(&unit),
        ..match_input(Some("/usr/lib/xdg-desktop-portal-kde"), None)
    };
    assert_eq!(matched_id(&portal, &index), None);
}

#[test]
fn match_scope_launcher_alt() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let apps = dir.path().join("applications");
    fs::create_dir(&apps).expect("create applications dir");
    fs::write(
        apps.join("org.gnome.Nautilus.desktop"),
        "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus --new-window %U\n",
    )
    .expect("write desktop file");
    let index = DesktopIndex::build_in(&[apps], &[]);

    let unit = parse_app_unit("app-gnome-org.gnome.Nautilus-777.scope").expect("unit parses");
    assert_eq!(&*unit.app_id, "gnome-org.gnome.Nautilus");
    let inp = MatchInput {
        unit_app: Some(&unit),
        ..match_input(
            Some("/usr/bin/nautilus"),
            Some("nautilus --gapplication-service"),
        )
    };
    assert_eq!(
        matched_id(&inp, &index).as_deref(),
        Some("org.gnome.Nautilus")
    );
}

// ---- Process flags and the protected set (E05-017) ----

use smallvec::smallvec;
use tl_collect::apps::classify::{ProtectInputs, flags_for, protected_set, wayland_compositor_pid};
use tl_model::process::ProcFlags;

fn session_inputs() -> ProtectInputs {
    ProtectInputs {
        self_pids: smallvec![3000],
        compositor_pid: Some(900),
        session_bus_pid: Some(810),
    }
}

#[test]
fn protected_set_full_session() {
    let user_init = cgroup("user.slice/user-1000.slice/user@1000.service/init.scope");
    let logind = cgroup("system.slice/systemd-logind.service");
    let session = cgroup("user.slice/user-1000.slice/session-2.scope");
    let records = [
        (1, 0, "systemd", None, 0),
        (450, 1, "systemd-logind", Some(&logind), 0),
        (500, 1, "sddm", None, 0),
        (600, 500, "startplasma-wayland", Some(&session), SELF_UID),
        (900, 600, "kwin_wayland", Some(&session), SELF_UID),
        (901, 800, "plasmashell", None, SELF_UID),
        (902, 800, "ksmserver", None, SELF_UID),
        (903, 900, "Xwayland", None, SELF_UID),
        (800, 1, "systemd", Some(&user_init), SELF_UID),
        (810, 800, "dbus-broker", None, SELF_UID),
        (2000, 800, "bash", None, SELF_UID),
        (3000, 800, "tasklens", None, SELF_UID),
    ];
    let set = protected_set(&records, SELF_UID, &session_inputs());
    for &(pid, ..) in &records {
        let want = !matches!(pid, 2000 | 600);
        assert_eq!(set.contains(pid), want, "pid {pid}");
    }
}

#[test]
fn protected_set_other_uid_excluded() {
    let records = [(901, 1, "plasmashell", None, 1001)];
    let set = protected_set(&records, SELF_UID, &ProtectInputs::default());
    assert!(!set.contains(901));
    assert!(set.contains(1));
}

#[test]
fn flags_self_and_protected() {
    // Security regression: tasklens must never lose IS_SELF.
    let inp = session_inputs();
    let records = [(3000, 1, "tasklens", None, SELF_UID)];
    let set = protected_set(&records, SELF_UID, &inp);
    let flags = flags_for(3000, false, SELF_UID, SELF_UID, &set, &inp);
    assert!(flags.contains(ProcFlags::IS_SELF | ProcFlags::PROTECTED));
    assert!(!flags.intersects(ProcFlags::NEW | ProcFlags::EXITED | ProcFlags::OTHER_USER));
}

#[test]
fn flags_kernel_thread_other_user() {
    let inp = ProtectInputs::default();
    let set = protected_set(&[], SELF_UID, &inp);
    let flags = flags_for(42, true, 0, SELF_UID, &set, &inp);
    assert!(flags.contains(ProcFlags::KERNEL_THREAD | ProcFlags::OTHER_USER));
    assert!(!flags.intersects(ProcFlags::IS_SELF | ProcFlags::PROTECTED));
}

#[test]
fn compositor_pid_via_peercred() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _server = std::os::unix::net::UnixListener::bind(dir.path().join("sock")).expect("bind");
    assert_eq!(
        wayland_compositor_pid(dir.path(), OsStr::new("sock")),
        Some(std::process::id())
    );
    let absolute = dir.path().join("sock");
    assert_eq!(
        wayland_compositor_pid(Path::new("/nonexistent"), absolute.as_os_str()),
        Some(std::process::id())
    );
}

#[test]
fn compositor_pid_missing_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(wayland_compositor_pid(dir.path(), OsStr::new("sock")), None);
    assert_eq!(wayland_compositor_pid(dir.path(), OsStr::new("")), None);
}
