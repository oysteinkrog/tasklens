//! Tests for app naming (E05-018), the desktop entry index (E05-013), the desktop watch
//! (E05-014), icon theme resolution (E05-019) and the icon path cache (E05-020).

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
