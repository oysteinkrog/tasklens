//! `parse_exec` against the Desktop Entry Exec rules: field codes, quoting, string escapes and
//! the errors for malformed values.

use std::path::Path;

use proptest::prelude::*;
use tl_ops::OpsError;
use tl_ops::launch::{
    ExecContext, admin_argv, parse_exec, script_argv, shell_argv, validate_command_text,
};

const NO_CTX: ExecContext<'static> = ExecContext {
    name: "App",
    icon: None,
    desktop_file: None,
};

fn parse(raw: &str) -> Result<Vec<String>, OpsError> {
    parse_exec(raw, &NO_CTX)
}

fn invalid(raw: &str) -> &'static str {
    match parse(raw) {
        Err(OpsError::InvalidInput(msg)) => msg,
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn strips_url_and_file_codes() {
    assert_eq!(parse("firefox %u").unwrap(), ["firefox"]);
}

#[test]
fn quoted_path_with_spaces() {
    assert_eq!(
        parse(r#""/opt/My App/run" --flag %F"#).unwrap(),
        ["/opt/My App/run", "--flag"]
    );
}

#[test]
fn quoted_escapes() {
    let args = parse(r#"app "a \\"b\\" \\$c""#).unwrap();
    assert_eq!(args, ["app", r#"a "b" $c"#]);
}

#[test]
fn string_value_escapes() {
    assert_eq!(parse(r"foo\sbar baz").unwrap(), ["foo bar", "baz"]);
}

#[test]
fn icon_and_name_codes() {
    let ctx = ExecContext {
        name: "My App",
        icon: Some("app-icon"),
        desktop_file: None,
    };
    assert_eq!(
        parse_exec("app %i --title=%c", &ctx).unwrap(),
        ["app", "--icon", "app-icon", "--title=My App"]
    );
}

#[test]
fn icon_code_without_icon_removed() {
    assert_eq!(parse("app %i").unwrap(), ["app"]);
    let empty_icon = ExecContext {
        name: "App",
        icon: Some(""),
        desktop_file: None,
    };
    assert_eq!(parse_exec("app %i", &empty_icon).unwrap(), ["app"]);
}

#[test]
fn desktop_file_code() {
    let ctx = ExecContext {
        name: "App",
        icon: None,
        desktop_file: Some("/usr/share/applications/app.desktop"),
    };
    assert_eq!(
        parse_exec("app %k", &ctx).unwrap(),
        ["app", "/usr/share/applications/app.desktop"]
    );
}

#[test]
fn percent_escape() {
    assert_eq!(parse("app 100%%").unwrap(), ["app", "100%"]);
}

#[test]
fn percent_literal_inside_quotes() {
    assert_eq!(parse(r#"app "50%""#).unwrap(), ["app", "50%"]);
}

#[test]
fn invalid_field_code() {
    assert_eq!(invalid("app %z"), "invalid field code in Exec");
}

#[test]
fn unterminated_quote() {
    assert_eq!(
        invalid(r#"app "unterminated"#),
        "unterminated quote in Exec"
    );
}

#[test]
fn quote_inside_unquoted_word() {
    assert_eq!(
        invalid(r#"app ab"cd""#),
        "quote inside an unquoted Exec argument"
    );
}

#[test]
fn empty_after_codes() {
    assert_eq!(invalid("%f %U"), "Exec is empty");
}

#[test]
fn nul_byte_rejected() {
    assert_eq!(invalid("app\0 x"), "Exec contains a NUL byte");
}

#[test]
fn no_shell_interpretation() {
    assert_eq!(
        parse(r#"sh -c "echo $HOME; ls | wc -l""#).unwrap(),
        ["sh", "-c", "echo $HOME; ls | wc -l"]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn prop_never_panics(raw in "(?s).{0,4096}") {
        let _ = parse(&raw);
    }
}

#[test]
fn command_text_empty() {
    assert_eq!(
        validate_command_text("   "),
        Err(OpsError::InvalidInput("command is empty"))
    );
}

/// Regression test for the PROC-095 rule: C0, DEL and C1 control characters are rejected.
#[test]
fn command_text_control_chars_rejected() {
    for text in ["ls\u{0}x", "ls\nrm", "ls\u{7f}", "ls\u{85}"] {
        assert_eq!(
            validate_command_text(text),
            Err(OpsError::InvalidInput("command contains invalid text")),
            "{text:?}"
        );
    }
    assert_eq!(validate_command_text("ls -la ~"), Ok(()));
}

#[test]
fn shell_argv_keeps_text() {
    assert_eq!(
        shell_argv("echo hi > /tmp/x").unwrap(),
        ["/bin/sh", "-c", "echo hi > /tmp/x"]
    );
    assert_eq!(shell_argv("  kate ").unwrap(), ["/bin/sh", "-c", "  kate "]);
}

#[test]
fn admin_argv_full_env() {
    let env = [
        ("DISPLAY", Some(":0")),
        ("WAYLAND_DISPLAY", Some("wayland-0")),
        ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
    ];
    assert_eq!(
        admin_argv("kate", &env).unwrap(),
        [
            "/usr/bin/pkexec",
            "env",
            "DISPLAY=:0",
            "WAYLAND_DISPLAY=wayland-0",
            "XDG_RUNTIME_DIR=/run/user/1000",
            "/bin/sh",
            "-c",
            "kate",
        ]
    );
}

#[test]
fn admin_argv_skips_unset_env() {
    let env = [
        ("DISPLAY", None),
        ("WAYLAND_DISPLAY", Some("wayland-0")),
        ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
    ];
    let argv = admin_argv("kate", &env).unwrap();
    assert!(!argv.iter().any(|a| a.starts_with("DISPLAY=")));
    assert_eq!(
        argv,
        [
            "/usr/bin/pkexec",
            "env",
            "WAYLAND_DISPLAY=wayland-0",
            "XDG_RUNTIME_DIR=/run/user/1000",
            "/bin/sh",
            "-c",
            "kate",
        ]
    );

    let empty = [("DISPLAY", Some(""))];
    assert_eq!(
        admin_argv("kate", &empty).unwrap(),
        ["/usr/bin/pkexec", "env", "/bin/sh", "-c", "kate"]
    );
    let bad = [("DISPLAY", Some(":0\nX=1"))];
    assert_eq!(
        admin_argv("kate", &bad),
        Err(OpsError::InvalidInput(
            "environment value contains invalid text"
        ))
    );
    assert_eq!(
        admin_argv("ls\nrm", &env),
        Err(OpsError::InvalidInput("command contains invalid text"))
    );
}

#[test]
fn script_argv_requires_absolute() {
    assert_eq!(
        script_argv(Path::new("bin/run.sh")),
        Err(OpsError::InvalidInput("script path is not absolute UTF-8"))
    );
    assert_eq!(
        script_argv(Path::new("/home/u/run.sh")).unwrap(),
        ["/bin/sh", "/home/u/run.sh"]
    );
}
