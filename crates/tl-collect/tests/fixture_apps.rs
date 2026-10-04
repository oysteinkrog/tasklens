//! Tests for app naming (E05-018).

use std::sync::Arc;

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
    assert_eq!(image_name(exe, &args, "wine64-preloader", wine), Some("game.exe"));
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
