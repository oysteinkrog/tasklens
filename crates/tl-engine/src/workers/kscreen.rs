//! The KScreen display reader for the inventory scan (E12, PERF-072, PERF-073, R-057).
//!
//! On KDE every host inventory scan runs `kscreen-doctor -j` once. The scan runs when the GPU
//! page or the Information tab opens, on `RefreshNow`, and when KWin's output config file
//! `kwinoutputconfig.json` changes while one of them is open (the KScreen config change). The
//! JSON supplies the enabled state, primary output (`priority == 1`), rotation, scale and the
//! current mode with its refresh rate. Max bpc stays `None` until KMS (P4).
//!
//! Only `kwinoutputconfig.json` is watched, with one `stat` per turn. Plasma 5 has no such file,
//! so there the values refresh when the GPU page or the Information tab opens and on a manual
//! refresh, not on a config change.
//!
//! Without KDE or `kscreen-doctor`, or when it fails, times out or prints unexpected JSON, the
//! sysfs values stay as they are. The text output (`kscreen-doctor -o`) is never parsed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tl_collect::inventory::ConnectorInfo;
use tl_model::snapshot::DisplayMode;

use crate::workers::inventory::SystemEnv;

/// The program, looked up in `$PATH`.
pub const PROGRAM: &str = "kscreen-doctor";
/// KWin's output config, below `$XDG_CONFIG_HOME`. KWin rewrites it on every output change.
pub const CONFIG_FILE: &str = "kwinoutputconfig.json";
/// How long one run may take.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// Most stdout bytes kept. Past that the run counts as failed.
pub const MAX_OUTPUT: usize = 1 << 20;
/// How often the run is checked for an exit.
const POLL: Duration = Duration::from_millis(10);
/// Longest output name kept, in bytes. Names are only matched, never shown.
const NAME_MAX: usize = 64;
/// Highest refresh rate kept, in Hz.
const REFRESH_MAX_HZ: f64 = 1000.0;
/// Largest scale factor kept.
const SCALE_MAX: f64 = 10.0;

/// The current mode of one output.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct KScreenMode {
    pub width: u32,
    pub height: u32,
    /// 0 when KScreen gives no usable rate.
    pub refresh_mhz: u32,
}

/// One output of `kscreen-doctor -j`.
#[derive(Clone, PartialEq, Debug)]
pub struct KScreenOutput {
    /// The connector name, for example "DP-3".
    pub name: Arc<str>,
    pub enabled: bool,
    pub primary: bool,
    /// 1, 2, 4 or 8; `None` for any other value.
    pub rotation: Option<u8>,
    /// `None` when missing, not finite, not above 0 or above 10.
    pub scale: Option<f32>,
    /// `None` when `currentModeId` names no mode with a size.
    pub mode: Option<KScreenMode>,
}

/// True when `XDG_CURRENT_DESKTOP` lists KDE.
pub fn is_kde(env: &SystemEnv) -> bool {
    env.xdg_current_desktop
        .as_deref()
        .is_some_and(|d| d.split(':').any(|d| d == "KDE"))
}

/// Blocking. Runs `kscreen-doctor -j` from `env.path_dirs` on KDE and parses it. `None` off KDE,
/// when the program is missing, or when the run or the parse fails.
pub fn read(env: &SystemEnv) -> Option<Vec<KScreenOutput>> {
    if !is_kde(env) {
        return None;
    }
    let exe = env
        .path_dirs
        .iter()
        .map(|d| d.join(PROGRAM))
        .find(|p| p.is_file())?;
    let Some(out) = run(&exe, TIMEOUT) else {
        tracing::debug!("inventory: kscreen-doctor gave no output");
        return None;
    };
    let parsed = parse(&out);
    if parsed.is_none() {
        tracing::debug!("inventory: unexpected kscreen-doctor output");
    }
    parsed
}

/// Runs `exe -j` with stdin and stderr null. Stdout is drained on its own thread so a large
/// output cannot block the child. `None` on a spawn error, a timeout, a non-zero exit or more
/// than `MAX_OUTPUT` bytes.
#[doc(hidden)]
pub fn run(exe: &Path, timeout: Duration) -> Option<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(exe)
        .arg("-j")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (tx, rx) = mpsc::channel();
    let reader = child.stdout.take().map(|stdout| {
        std::thread::Builder::new()
            .name("tl-kscreen-out".into())
            .spawn(move || {
                let _ = tx.send(read_capped(stdout));
            })
    });
    if !matches!(reader, Some(Ok(_))) {
        reap(&mut child);
        return None;
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => {
                reap(&mut child);
                return None;
            }
        }
    };
    // The reader is not joined: a pipe held open elsewhere must not hold this thread.
    let wait = deadline.saturating_duration_since(Instant::now()).max(POLL);
    let out = rx.recv_timeout(wait).ok()??;
    status.success().then_some(out)
}

/// Kills and waits for the child.
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Stdout up to `MAX_OUTPUT` bytes. Past that the rest is read and dropped so the child can end.
fn read_capped(mut stdout: impl Read) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let limit = u64::try_from(MAX_OUTPUT)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    (&mut stdout).take(limit).read_to_end(&mut out).ok()?;
    if out.len() > MAX_OUTPUT {
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        return None;
    }
    Some(out)
}

/// The outputs of `kscreen-doctor -j`. `None` when the bytes are not JSON or have no `outputs`
/// array. An output without a usable `name` or `enabled` is skipped.
pub fn parse(bytes: &[u8]) -> Option<Vec<KScreenOutput>> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let outputs = root.get("outputs")?.as_array()?;
    Some(outputs.iter().filter_map(output).collect())
}

fn output(o: &Value) -> Option<KScreenOutput> {
    let name = o.get("name")?.as_str()?;
    if name.is_empty() || name.len() > NAME_MAX {
        return None;
    }
    // Plasma 6 has `priority`; Plasma 5 had a `primary` flag.
    let primary = o.get("priority").and_then(Value::as_u64) == Some(1)
        || o.get("primary").and_then(Value::as_bool) == Some(true);
    let rotation = o
        .get("rotation")
        .and_then(Value::as_u64)
        .filter(|r| matches!(r, 1 | 2 | 4 | 8))
        .and_then(|r| u8::try_from(r).ok());
    let scale = o
        .get("scale")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0 && *s <= SCALE_MAX)
        .map(|s| s as f32);
    Some(KScreenOutput {
        name: Arc::from(name),
        enabled: o.get("enabled")?.as_bool()?,
        primary,
        rotation,
        scale,
        mode: current_mode(o),
    })
}

/// The mode whose `id` equals `currentModeId`. Ids are strings; numbers are accepted too.
fn current_mode(o: &Value) -> Option<KScreenMode> {
    let id = mode_id(o.get("currentModeId")?)?;
    let mode = o
        .get("modes")?
        .as_array()?
        .iter()
        .find(|m| m.get("id").and_then(mode_id).as_deref() == Some(id.as_str()))?;
    let size = mode.get("size")?;
    let dim = |key: &str| {
        let v = size.get(key)?.as_u64()?;
        u32::try_from(v).ok().filter(|v| *v > 0)
    };
    let refresh_mhz = mode
        .get("refreshRate")
        .and_then(Value::as_f64)
        .filter(|hz| hz.is_finite() && *hz > 0.0 && *hz <= REFRESH_MAX_HZ)
        .map_or(0, |hz| (hz * 1000.0).round() as u32);
    Some(KScreenMode {
        width: dim("width")?,
        height: dim("height")?,
        refresh_mhz,
    })
}

fn mode_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The connector part of a sysfs connector name: "card1-DP-2" gives "DP-2".
pub fn connector_name(sysfs_name: &str) -> &str {
    sysfs_name
        .strip_prefix("card")
        .map(|rest| rest.trim_start_matches(|c: char| c.is_ascii_digit()))
        .and_then(|rest| rest.strip_prefix('-'))
        .unwrap_or(sysfs_name)
}

/// The output for connector `c`. KScreen names carry no card, so a name that two connected
/// connectors share (two GPUs) matches nothing.
pub fn output_for<'a>(
    outputs: &'a [KScreenOutput],
    connectors: &[ConnectorInfo],
    c: &ConnectorInfo,
) -> Option<&'a KScreenOutput> {
    let name = connector_name(&c.sysfs_name);
    let shared = connectors
        .iter()
        .filter(|o| o.connected && connector_name(&o.sysfs_name) == name)
        .count();
    if shared > 1 {
        return None;
    }
    outputs.iter().find(|o| &*o.name == name)
}

/// Fills primary, rotation and scale from `out`, and the size and refresh rate from its current
/// mode when it has one. The rest of `mode` stays as it is.
pub fn fill(mode: &mut DisplayMode, out: &KScreenOutput) {
    mode.primary = out.primary;
    mode.rotation = out.rotation;
    mode.scale = out.scale;
    if let Some(current) = out.mode {
        mode.width = current.width;
        mode.height = current.height;
        mode.refresh_mhz = current.refresh_mhz;
    }
}

/// `<config_home>/kwinoutputconfig.json` on KDE, with `config_home` from `$XDG_CONFIG_HOME` or
/// `$HOME/.config`. Relative values are ignored.
pub fn config_path(
    env: &SystemEnv,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if !is_kde(env) {
        return None;
    }
    let config_home = xdg_config_home
        .filter(|p| p.is_absolute())
        .or_else(|| home.filter(|p| p.is_absolute()).map(|h| h.join(".config")))?;
    Some(config_home.join(CONFIG_FILE))
}

/// [`config_path`] with this process's environment.
pub fn config_file(env: &SystemEnv) -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    config_path(env, var("XDG_CONFIG_HOME"), var("HOME"))
}

/// The modification time of `path`, `None` when it is missing or unreadable.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use smallvec::smallvec;
    use tl_collect::inventory::ConnectorType;
    use tl_model::snapshot::DisplayConnector;

    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/kscreen/doctor-j.json");

    fn kde() -> SystemEnv {
        SystemEnv {
            xdg_current_desktop: Some("KDE".into()),
            ..SystemEnv::default()
        }
    }

    fn connector(card: u32, name: &str, connected: bool) -> ConnectorInfo {
        ConnectorInfo {
            card,
            sysfs_name: Arc::from(format!("card{card}-{name}")),
            connector_type: ConnectorType::DisplayPort,
            connected,
            enabled: Some(connected),
            monitor_name: None,
            modes: smallvec![(1920, 1080)],
        }
    }

    fn sysfs_mode() -> DisplayMode {
        DisplayMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 0,
            primary: false,
            monitor_name: Some(Arc::from("Monitor")),
            connector: DisplayConnector::DisplayPort,
            max_bpc: None,
            rotation: None,
            scale: None,
        }
    }

    #[test]
    fn parse_fixture() {
        let outputs = parse(FIXTURE).expect("fixture parses");
        assert_eq!(outputs.len(), 3);
        assert_eq!(
            outputs[0],
            KScreenOutput {
                name: Arc::from("DP-2"),
                enabled: true,
                primary: true,
                rotation: Some(1),
                scale: Some(1.5),
                mode: Some(KScreenMode {
                    width: 3840,
                    height: 2160,
                    refresh_mhz: 143_963,
                }),
            }
        );
        assert_eq!(
            outputs[1],
            KScreenOutput {
                name: Arc::from("DP-3"),
                enabled: true,
                primary: false,
                rotation: Some(2),
                scale: Some(1.1),
                mode: Some(KScreenMode {
                    width: 1920,
                    height: 1200,
                    refresh_mhz: 59_950,
                }),
            }
        );
        // Disabled, with an empty `currentModeId`.
        assert_eq!(&*outputs[2].name, "DP-1");
        assert!(!outputs[2].enabled);
        assert!(!outputs[2].primary);
        assert_eq!(outputs[2].mode, None);
    }

    #[test]
    fn parse_rejects_and_skips() {
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"not json"), None);
        assert_eq!(parse(br#"{"screen": {}}"#), None);
        assert_eq!(parse(br#"{"outputs": {}}"#), None);
        assert_eq!(parse(br#"{"outputs": []}"#), Some(Vec::new()));
        // No name, an empty name, a long name, no `enabled`: skipped.
        let long = "x".repeat(NAME_MAX + 1);
        let json = format!(
            r#"{{"outputs": [{{"enabled": true}}, {{"name": "", "enabled": true}},
                {{"name": "{long}", "enabled": true}}, {{"name": "DP-1"}},
                {{"name": "DP-2", "enabled": false}}]}}"#
        );
        let outputs = parse(json.as_bytes()).expect("outputs array");
        assert_eq!(outputs.len(), 1);
        assert_eq!(&*outputs[0].name, "DP-2");
    }

    #[test]
    fn parse_value_rules() {
        let one = |fields: &str| {
            let json = format!(r#"{{"outputs": [{{"name": "DP-1", "enabled": true, {fields}}}]}}"#);
            parse(json.as_bytes()).expect("parses").remove(0)
        };
        assert_eq!(one(r#""rotation": 3"#).rotation, None);
        assert_eq!(one(r#""rotation": 8"#).rotation, Some(8));
        assert_eq!(one(r#""scale": 0"#).scale, None);
        assert_eq!(one(r#""scale": -1.25"#).scale, None);
        assert_eq!(one(r#""scale": 11"#).scale, None);
        assert_eq!(one(r#""scale": 2"#).scale, Some(2.0));
        // Plasma 5's flag.
        assert!(one(r#""primary": true"#).primary);
        assert!(!one(r#""priority": 2"#).primary);
        // A numeric mode id, and a mode without a usable rate.
        let o = one(
            r#""currentModeId": 4, "modes": [{"id": "4", "refreshRate": -1,
                "size": {"width": 800, "height": 600}}]"#,
        );
        assert_eq!(
            o.mode,
            Some(KScreenMode {
                width: 800,
                height: 600,
                refresh_mhz: 0,
            })
        );
        // The current mode is missing, or its size is not positive.
        assert_eq!(one(r#""currentModeId": "5", "modes": []"#).mode, None);
        let o = one(
            r#""currentModeId": "1", "modes": [{"id": "1", "refreshRate": 60,
                "size": {"width": -1, "height": -1}}]"#,
        );
        assert_eq!(o.mode, None);
    }

    #[test]
    fn connector_names() {
        assert_eq!(connector_name("card1-DP-2"), "DP-2");
        assert_eq!(connector_name("card12-HDMI-A-1"), "HDMI-A-1");
        assert_eq!(connector_name("DP-2"), "DP-2");
        assert_eq!(connector_name("cardX-DP-2"), "cardX-DP-2");
    }

    #[test]
    fn match_by_connector_name() {
        let outputs = parse(FIXTURE).expect("fixture parses");
        let dp2 = connector(1, "DP-2", true);
        let hdmi = connector(1, "HDMI-A-1", true);
        let list = [dp2.clone(), hdmi.clone()];
        assert_eq!(
            output_for(&outputs, &list, &dp2).map(|o| &*o.name),
            Some("DP-2")
        );
        assert_eq!(output_for(&outputs, &list, &hdmi), None);
        // A second GPU with a connected DP-2: the name is ambiguous.
        let other = connector(2, "DP-2", true);
        let list = [dp2.clone(), other.clone()];
        assert_eq!(output_for(&outputs, &list, &dp2), None);
        assert_eq!(output_for(&outputs, &list, &other), None);
        // A disconnected twin does not count.
        let list = [dp2.clone(), connector(2, "DP-2", false)];
        assert!(output_for(&outputs, &list, &dp2).is_some());
    }

    #[test]
    fn fill_keeps_sysfs_fields() {
        let outputs = parse(FIXTURE).expect("fixture parses");
        let mut mode = sysfs_mode();
        fill(&mut mode, &outputs[1]);
        assert_eq!(
            mode,
            DisplayMode {
                width: 1920,
                height: 1200,
                refresh_mhz: 59_950,
                primary: false,
                monitor_name: Some(Arc::from("Monitor")),
                connector: DisplayConnector::DisplayPort,
                max_bpc: None,
                rotation: Some(2),
                scale: Some(1.1),
            }
        );
        // No current mode: size and refresh stay.
        let mut mode = sysfs_mode();
        fill(&mut mode, &outputs[2]);
        assert_eq!((mode.width, mode.height, mode.refresh_mhz), (1920, 1080, 0));
        assert_eq!(mode.rotation, Some(1));
        assert_eq!(mode.scale, Some(1.0));
    }

    #[test]
    fn config_path_rules() {
        let abs = |p: &str| Some(PathBuf::from(p));
        let want = PathBuf::from("/x/kwinoutputconfig.json");
        assert_eq!(config_path(&kde(), abs("/x"), abs("/h")), Some(want));
        assert_eq!(
            config_path(&kde(), abs("rel"), abs("/h")),
            Some(PathBuf::from("/h/.config/kwinoutputconfig.json"))
        );
        assert_eq!(config_path(&kde(), None, abs("rel")), None);
        let gnome = SystemEnv {
            xdg_current_desktop: Some("GNOME".into()),
            ..SystemEnv::default()
        };
        assert_eq!(config_path(&gnome, abs("/x"), abs("/h")), None);
        assert_eq!(config_path(&SystemEnv::default(), abs("/x"), None), None);
    }

    #[test]
    fn missing_tool_reads_nothing() {
        let dir = scratch();
        let env = SystemEnv {
            path_dirs: vec![dir.path().to_path_buf()],
            ..kde()
        };
        assert_eq!(read(&env), None);
        // Off KDE the program is never looked up.
        let env = SystemEnv {
            xdg_current_desktop: Some("GNOME".into()),
            ..env
        };
        assert_eq!(read(&env), None);
    }

    /// A directory under `target/tmp`, removed on drop.
    fn scratch() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("tasklens-hjn40-")
            .tempdir_in(base)
            .expect("tempdir")
    }

    /// A fake `kscreen-doctor` in `dir`, as a shell script. A `cp` child writes the program, so
    /// no fork of another test thread holds it open for writing when it runs ("Text file busy").
    fn fake(dir: &Path, script: &str) -> PathBuf {
        let src = dir.join("script.sh");
        std::fs::write(&src, format!("#!/bin/sh\n{script}\n")).expect("write script");
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let exe = dir.join(PROGRAM);
        let _ = std::fs::remove_file(&exe);
        let copied = Command::new("cp").arg(&src).arg(&exe).status();
        assert!(copied.is_ok_and(|s| s.success()), "cp fake program");
        exe
    }

    #[test]
    fn fake_program_runs() {
        let dir = scratch();
        let json = dir.path().join("out.json");
        std::fs::write(&json, FIXTURE).expect("write json");
        fake(
            dir.path(),
            &format!("[ \"$1\" = -j ] || exit 2\ncat '{}'", json.display()),
        );
        let env = SystemEnv {
            path_dirs: vec![dir.path().to_path_buf()],
            ..kde()
        };
        let outputs = read(&env).expect("fake output parses");
        assert_eq!(outputs.len(), 3);
    }

    #[test]
    fn failed_runs_give_none() {
        let dir = scratch();
        let exe = fake(dir.path(), "echo '{\"outputs\": []}'\nexit 1");
        assert_eq!(run(&exe, TIMEOUT), None);
        let exe = fake(dir.path(), "exec sleep 5");
        let start = Instant::now();
        assert_eq!(run(&exe, Duration::from_millis(200)), None);
        assert!(start.elapsed() < Duration::from_secs(3));
        let exe = fake(dir.path(), "head -c 1048577 /dev/zero");
        assert_eq!(run(&exe, TIMEOUT), None);
        fake(dir.path(), "echo garbage");
        let env = SystemEnv {
            path_dirs: vec![dir.path().to_path_buf()],
            ..kde()
        };
        assert_eq!(read(&env), None);
    }
}
