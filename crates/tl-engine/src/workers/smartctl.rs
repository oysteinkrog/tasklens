//! SMART read with `pkexec smartctl` for a disk UDisks2 cannot read (E12-028, PERF-051, U-25).
//!
//! This is the one privileged action of the disk pages. polkit asks for the password; tasklens
//! never sees or keeps it, and the privilege lasts one smartctl run. The disk name is checked to
//! `[a-z0-9]` before it reaches a command line, both programs are absolute paths, the environment
//! is cleared and no shell runs. The output holds the serial: it is shown, never logged.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;
use tl_collect::sanitize::sanitize_text;
use tl_collect::startup::find_on_path;
use tl_model::snapshot::{InvField, SmartAttr, SmartData};
use tl_model::units;
use tl_model::validity::Validity;

/// How long one run may take. Includes the time the user takes at the polkit prompt.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// Most stdout bytes kept.
pub const MAX_OUTPUT: usize = 1 << 20;
/// How often the run is checked for an exit.
const POLL: Duration = Duration::from_millis(50);
/// The only folders searched for `smartctl`, in order. Never `PATH`: the program runs as root,
/// and a user-writable `PATH` folder could hold a fake one.
const SMARTCTL_DIRS: [&str; 4] = ["/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// The only folders searched for `pkexec`, in order.
const PKEXEC_DIRS: [&str; 2] = ["/usr/bin", "/bin"];
/// The whole `PATH` of the child.
const CHILD_PATH: &str = "/usr/sbin:/usr/bin";
/// Longest attribute name or self-test text kept, in bytes.
const TEXT_MAX: usize = 256;
/// One NVMe data unit: 1000 blocks of 512 bytes.
const NVME_DATA_UNIT: u64 = 512_000;

pub const INVALID_DISK: &str = "Invalid disk name";
pub const NOT_INSTALLED: &str = "smartctl is not installed (package smartmontools)";
pub const NO_PKEXEC: &str = "pkexec is not installed (package polkit)";
pub const NOT_RUN: &str = "Could not run smartctl";
pub const DENIED: &str = "Permission denied";
pub const CANCELLED: &str = "Authentication cancelled";
pub const NO_ANSWER: &str = "No answer from smartctl";
pub const CANNOT_OPEN: &str = "smartctl could not open the disk";
pub const TOO_LARGE: &str = "smartctl output too large";
pub const UNEXPECTED: &str = "Unexpected smartctl output";

/// A kernel disk name: 1 to 32 bytes of `[a-z0-9]`, starting with a letter.
pub fn valid_disk_name(disk: &str) -> bool {
    let b = disk.as_bytes();
    (1..=32).contains(&b.len())
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `pkexec` and `smartctl` as absolute paths, from fixed system folders only. `PATH` is never
/// read.
pub fn locate() -> Result<(PathBuf, PathBuf), Arc<str>> {
    let dirs = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();
    let smartctl =
        find_on_path("smartctl", &dirs(&SMARTCTL_DIRS)).ok_or_else(|| Arc::from(NOT_INSTALLED))?;
    let pkexec = find_on_path("pkexec", &dirs(&PKEXEC_DIRS)).ok_or_else(|| Arc::from(NO_PKEXEC))?;
    Ok((pkexec, smartctl))
}

/// Blocking. Checks the name, finds both programs and runs smartctl once.
pub fn read_raw(disk: &str) -> Result<Vec<u8>, Arc<str>> {
    if !valid_disk_name(disk) {
        return Err(Arc::from(INVALID_DISK));
    }
    let (pkexec, smartctl) = locate()?;
    run_smartctl(&pkexec, &smartctl, disk)
}

/// Blocking. Runs pkexec with an absolute smartctl path; returns stdout bytes.
pub fn run_smartctl(pkexec: &Path, smartctl: &Path, disk: &str) -> Result<Vec<u8>, Arc<str>> {
    run_for(pkexec, smartctl, disk, TIMEOUT)
}

fn run_for(
    pkexec: &Path,
    smartctl: &Path,
    disk: &str,
    timeout: Duration,
) -> Result<Vec<u8>, Arc<str>> {
    if !valid_disk_name(disk) {
        return Err(Arc::from(INVALID_DISK));
    }
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(pkexec)
        .arg(smartctl)
        .args(["--json=c", "-a"])
        .arg(format!("/dev/{disk}"))
        .env_clear()
        .env("PATH", CHILD_PATH)
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Arc::from(NOT_RUN))?;
    // The reader drains stdout so a large output cannot block the child on a full pipe.
    let (tx, rx) = mpsc::channel();
    let reader = child.stdout.take().map(|stdout| {
        std::thread::Builder::new()
            .name("tl-smartctl-out".into())
            .spawn(move || {
                let _ = tx.send(read_capped(stdout));
            })
    });
    if !matches!(reader, Some(Ok(_))) {
        reap(&mut child);
        return Err(Arc::from(NOT_RUN));
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => {
                reap(&mut child);
                return Err(Arc::from(NO_ANSWER));
            }
        }
    };
    // The reader is not joined: a pipe held open elsewhere must not hold this thread.
    let wait = deadline.saturating_duration_since(Instant::now()).max(POLL);
    let out = rx.recv_timeout(wait).map_err(|_| Arc::from(NO_ANSWER))?;
    match status.code() {
        // pkexec (R-117): 126 is a dismissed prompt; 127 is not authorized, or pkexec could not
        // authenticate.
        Some(126) => return Err(Arc::from(CANCELLED)),
        Some(127) => return Err(Arc::from(DENIED)),
        // smartctl's bit mask: bit 0 is a command line error, bit 1 a device open failure.
        // The other bits report disk state and still come with JSON.
        Some(code) if code & 0b11 != 0 => return Err(Arc::from(CANNOT_OPEN)),
        _ => {}
    }
    out.map_err(Arc::from)
}

/// Kills and waits for the child.
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Stdout up to `MAX_OUTPUT` bytes. Past that the rest is read and dropped so the child can end.
fn read_capped(mut stdout: impl Read) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    let limit = u64::try_from(MAX_OUTPUT)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    (&mut stdout)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| UNEXPECTED)?;
    if out.len() > MAX_OUTPUT {
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        return Err(TOO_LARGE);
    }
    Ok(out)
}

/// `SmartData` from `smartctl --json=c -a`. Missing sections leave those parts empty.
pub fn parse_smartctl_json(bytes: &[u8], now_s: i64) -> Result<SmartData, Arc<str>> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| Arc::from(UNEXPECTED))?;
    if !root.is_object() {
        return Err(Arc::from(UNEXPECTED));
    }
    let health = match root
        .pointer("/smart_status/passed")
        .and_then(Value::as_bool)
    {
        Some(true) => valid(Arc::from("OK")),
        Some(false) => valid(Arc::from("Failing")),
        None => unsupported(),
    };
    let power_on_s = root
        .pointer("/power_on_time/hours")
        .and_then(Value::as_u64)
        .map(|h| h.saturating_mul(3600));
    let selftest = root
        .pointer("/ata_smart_self_test_log/standard/table/0/status/string")
        .and_then(Value::as_str)
        .map(clean)
        .filter(|s| !s.is_empty());
    let ata = root
        .pointer("/ata_smart_attributes/table")
        .and_then(Value::as_array)
        .map(|table| table.iter().filter_map(ata_attr).collect())
        .unwrap_or_default();
    let nvme = root
        .get("nvme_smart_health_information_log")
        .map(nvme_rows)
        .unwrap_or_default();
    Ok(SmartData {
        updated_s: now_s,
        health,
        power_on_s: opt(power_on_s),
        selftest: opt(selftest.map(Arc::from)),
        ata,
        nvme,
    })
}

/// One `ata_smart_attributes.table` entry. `None` without a valid `id`.
fn ata_attr(a: &Value) -> Option<SmartAttr> {
    let id = a.get("id").and_then(Value::as_u64)?;
    let id = u8::try_from(id).ok()?;
    let int = |ptr: &str| a.pointer(ptr).and_then(Value::as_i64).unwrap_or(0);
    let small = |ptr: &str| i32::try_from(int(ptr)).unwrap_or(0);
    let flag = |ptr: &str| a.pointer(ptr).and_then(Value::as_bool).unwrap_or(false);
    let assessment = match a.get("when_failed").and_then(Value::as_str) {
        Some("now") => "Failing",
        Some("past") => "Failed in the past",
        _ => "OK",
    };
    // A raw value past `i64::MAX` is kept at the top of the range.
    let raw = a.pointer("/raw/value").map_or(0, |v| {
        v.as_i64()
            .or_else(|| v.as_u64().map(|_| i64::MAX))
            .unwrap_or(0)
    });
    Some(SmartAttr {
        id,
        name: Arc::from(clean(a.get("name").and_then(Value::as_str).unwrap_or(""))),
        value: raw,
        normalized: small("/value"),
        threshold: small("/thresh"),
        worst: small("/worst"),
        prefail: flag("/flags/prefailure"),
        online: flag("/flags/updated_online"),
        assessment: Arc::from(assessment),
    })
}

/// The `nvme_smart_health_information_log` rows, with the PERF-051 labels and order.
fn nvme_rows(log: &Value) -> Arc<[(Arc<str>, Arc<str>)]> {
    let get = |key: &str| log.get(key).and_then(Value::as_u64);
    let mut rows: Vec<(Arc<str>, Arc<str>)> = Vec::new();
    let mut row = |label: &str, text: String| rows.push((Arc::from(label), Arc::from(text)));

    let before: [NvmeField; 7] = [
        ("percentage_used", "Percent used", percent),
        ("available_spare", "Available spare", percent),
        ("available_spare_threshold", "Spare threshold", percent),
        ("data_units_read", "Total data read", data_units),
        ("data_units_written", "Total data written", data_units),
        ("warning_temp_time", "Warning temp time", minutes),
        ("critical_comp_time", "Critical temp time", minutes),
    ];
    for (key, label, format) in before {
        if let Some(n) = get(key) {
            row(label, format(n));
        }
    }
    if let Some(n) = get("controller_busy_time") {
        row("Controller busy time", minutes(n));
    }
    let celsius = |c: i64| {
        let mut text = String::new();
        units::celsius(&mut text, c as f32);
        text
    };
    // smartctl reports the thresholds only when the drive's identify data carries them.
    for (key, label) in [
        ("warning_temp_threshold", "Warning temperature"),
        ("critical_temp_threshold", "Critical temperature"),
    ] {
        if let Some(c) = log.get(key).and_then(Value::as_i64) {
            row(label, celsius(c));
        }
    }
    if let Some(sensors) = log.get("temperature_sensors").and_then(Value::as_array) {
        let sensors: Vec<String> = sensors
            .iter()
            .filter_map(Value::as_i64)
            .map(celsius)
            .collect();
        if !sensors.is_empty() {
            row("Temperature sensors", sensors.join(", "));
        }
    }
    let after: [NvmeField; 4] = [
        ("unsafe_shutdowns", "Unsafe shutdowns", count),
        ("media_errors", "Media errors", count),
        ("num_err_log_entries", "Error log entries", count),
        ("power_cycles", "Power cycles", count),
    ];
    for (key, label, format) in after {
        if let Some(n) = get(key) {
            row(label, format(n));
        }
    }
    rows.into()
}

/// smartctl JSON key, row label, value format.
type NvmeField = (&'static str, &'static str, fn(u64) -> String);

fn percent(n: u64) -> String {
    format!("{n} %")
}

fn minutes(n: u64) -> String {
    format!("{n} min")
}

fn count(n: u64) -> String {
    n.to_string()
}

fn data_units(n: u64) -> String {
    units::bytes_si_1dp(n.saturating_mul(NVME_DATA_UNIT))
}

/// A smartctl string, sanitized and cut at `TEXT_MAX`.
fn clean(s: &str) -> String {
    let mut out = String::new();
    sanitize_text(s.as_bytes(), TEXT_MAX, &mut out);
    out
}

fn valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn unsupported<T: Default>() -> InvField<T> {
    InvField {
        value: T::default(),
        validity: Validity::Unsupported,
    }
}

fn opt<T: Default>(value: Option<T>) -> InvField<T> {
    value.map_or_else(unsupported, valid)
}

/// `smartctl --json=c -a` for an NVMe disk, shortened. Shared with the worker tests.
#[cfg(test)]
pub(crate) const NVME_JSON: &str = r#"{
  "json_format_version": [1, 0],
  "smartctl": {"version": [7, 5], "exit_status": 0},
  "device": {"name": "/dev/nvme0n1", "type": "nvme", "protocol": "NVMe"},
  "model_name": "Samsung SSD 980 1TB",
  "serial_number": "S64ANS0T000000X",
  "smart_status": {"passed": true, "nvme": {"value": 0}},
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "temperature": 38,
    "temperature_sensors": [38, 41],
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 3,
    "data_units_read": 31250000,
    "data_units_written": 23437500,
    "host_reads": 400000000,
    "host_writes": 300000000,
    "controller_busy_time": 820,
    "power_cycles": 412,
    "power_on_hours": 1234,
    "unsafe_shutdowns": 17,
    "media_errors": 0,
    "num_err_log_entries": 0,
    "warning_temp_time": 0,
    "critical_comp_time": 0
  },
  "temperature": {"current": 38},
  "power_cycle_count": 412,
  "power_on_time": {"hours": 1234}
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    /// `smartctl --json=c -a` for an ATA disk, shortened. Attribute 5 is failing now.
    const ATA_JSON: &str = r#"{
      "json_format_version": [1, 0],
      "smartctl": {"version": [7, 5], "exit_status": 8},
      "device": {"name": "/dev/sda", "type": "sat", "protocol": "ATA"},
      "smart_status": {"passed": false},
      "ata_smart_attributes": {
        "revision": 16,
        "table": [
          {"id": 5, "name": "Reallocated_Sector_Ct", "value": 5, "worst": 5, "thresh": 10,
           "when_failed": "now",
           "flags": {"value": 51, "string": "PO--CK ", "prefailure": true,
                     "updated_online": true, "performance": false, "error_rate": false,
                     "event_count": true, "auto_keep": true},
           "raw": {"value": 3912, "string": "3912"}},
          {"id": 9, "name": "Power_On_Hours", "value": 90, "worst": 90, "thresh": 0,
           "when_failed": "",
           "flags": {"value": 50, "string": "-O--CK ", "prefailure": false,
                     "updated_online": true, "performance": false, "error_rate": false,
                     "event_count": true, "auto_keep": true},
           "raw": {"value": 9000, "string": "9000"}},
          {"id": 199, "name": "UDMA_CRC_Error_Count", "value": 200, "worst": 199, "thresh": 0,
           "when_failed": "past",
           "flags": {"value": 50, "string": "-O--CK ", "prefailure": false,
                     "updated_online": true, "performance": false, "error_rate": false,
                     "event_count": true, "auto_keep": true},
           "raw": {"value": 2, "string": "2"}}
        ]
      },
      "power_on_time": {"hours": 9000},
      "ata_smart_self_test_log": {
        "standard": {
          "revision": 1,
          "table": [
            {"type": {"value": 1, "string": "Short offline"},
             "status": {"value": 121, "string": "Completed: read failure", "passed": false},
             "lifetime_hours": 8990}
          ],
          "count": 1
        }
      }
    }"#;

    fn row<'a>(data: &'a SmartData, label: &str) -> Option<&'a str> {
        data.nvme
            .iter()
            .find(|(l, _)| &**l == label)
            .map(|(_, v)| &**v)
    }

    #[test]
    fn disk_name_rules() {
        for good in ["nvme0n1", "sda", "sdaa", "mmcblk0", &"a".repeat(32)] {
            assert!(valid_disk_name(good), "{good}");
        }
        for bad in [
            "sda; rm -rf /",
            "../sda",
            "",
            "Sda",
            "0sda",
            "sda1 ",
            "sd-a",
            "/dev/sda",
            &"a".repeat(33),
        ] {
            assert!(!valid_disk_name(bad), "{bad:?}");
        }
        // The check runs before any program.
        assert_eq!(
            run_smartctl(
                Path::new("/nonexistent"),
                Path::new("/nonexistent"),
                "../sda"
            ),
            Err(Arc::from(INVALID_DISK))
        );
    }

    #[test]
    fn parse_nvme_vector() {
        let data = parse_smartctl_json(NVME_JSON.as_bytes(), 1_700_000_000).expect("parse");
        assert_eq!(data.updated_s, 1_700_000_000);
        assert_eq!(data.health, valid(Arc::from("OK")));
        assert_eq!(data.power_on_s, valid(4_442_400));
        assert_eq!(data.selftest.validity, Validity::Unsupported);
        assert!(data.ata.is_empty());
        assert_eq!(row(&data, "Total data written"), Some("12.0 TB"));
        assert_eq!(row(&data, "Total data read"), Some("16.0 TB"));
        assert_eq!(row(&data, "Percent used"), Some("3 %"));
        assert_eq!(row(&data, "Temperature sensors"), Some("38 °C, 41 °C"));
        assert_eq!(row(&data, "Critical warning"), None);
        assert_eq!(row(&data, "Power-on hours"), None);
        assert_eq!(row(&data, "Controller busy time"), Some("820 min"));
        let labels: Vec<&str> = data.nvme.iter().map(|(l, _)| &**l).collect();
        assert_eq!(
            labels,
            [
                "Percent used",
                "Available spare",
                "Spare threshold",
                "Total data read",
                "Total data written",
                "Warning temp time",
                "Critical temp time",
                "Controller busy time",
                "Temperature sensors",
                "Unsafe shutdowns",
                "Media errors",
                "Error log entries",
                "Power cycles",
            ]
        );
    }

    #[test]
    fn parse_ata_failing_now() {
        let data = parse_smartctl_json(ATA_JSON.as_bytes(), 0).expect("parse");
        assert_eq!(data.health, valid(Arc::from("Failing")));
        assert_eq!(data.power_on_s, valid(9000 * 3600));
        assert_eq!(data.selftest, valid(Arc::from("Completed: read failure")));
        assert!(data.nvme.is_empty());
        assert_eq!(data.ata.len(), 3);
        let a = &data.ata[0];
        assert_eq!(a.id, 5);
        assert_eq!(&*a.name, "Reallocated_Sector_Ct");
        assert_eq!(a.value, 3912);
        assert_eq!((a.normalized, a.worst, a.threshold), (5, 5, 10));
        assert!(a.prefail && a.online);
        assert_eq!(&*a.assessment, "Failing");
        assert_eq!(&*data.ata[1].assessment, "OK");
        assert!(!data.ata[1].prefail);
        assert_eq!(&*data.ata[2].assessment, "Failed in the past");
    }

    #[test]
    fn parse_edge_cases() {
        assert_eq!(
            parse_smartctl_json(b"smartctl 7.5: not json", 0).err(),
            Some(Arc::from(UNEXPECTED))
        );
        assert_eq!(
            parse_smartctl_json(b"[1, 2]", 0).err(),
            Some(Arc::from(UNEXPECTED))
        );
        // No sections: every part is empty.
        let data = parse_smartctl_json(b"{}", 5).expect("parse");
        assert_eq!(data.health.validity, Validity::Unsupported);
        assert_eq!(data.power_on_s.validity, Validity::Unsupported);
        assert!(data.ata.is_empty() && data.nvme.is_empty());
    }

    /// A directory under `target/tmp`, removed on drop.
    fn scratch() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("tasklens-hjn32-")
            .tempdir_in(base)
            .expect("tempdir")
    }

    /// Runs `script` as the fake pkexec. `/bin/sh` stands in for pkexec and reads the script as
    /// the "smartctl" argument, so the script gets smartctl's arguments and no freshly written
    /// file is executed (which can fail with "Text file busy" while other tests fork).
    fn run_fake(script: &str, timeout: Duration) -> Result<Vec<u8>, Arc<str>> {
        let dir = scratch();
        let path = dir.path().join("fake-pkexec.sh");
        fs::write(&path, script).expect("write script");
        run_for(Path::new("/bin/sh"), &path, "sda", timeout)
    }

    /// Env var that tells `locate_child` the fake folder put first on its `PATH`.
    const FAKE_DIR_ENV: &str = "TASKLENS_HJN32_FAKE_DIR";

    /// Security regression: a fake `smartctl` and `pkexec` first on `PATH` are never chosen.
    /// The test binary runs `locate_child` again in a child process with that `PATH`, so this
    /// process's environment is never changed.
    #[test]
    fn locate_ignores_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        for name in ["smartctl", "pkexec"] {
            let path = dir.path().join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let exe = std::env::current_exe().expect("test binary");
        let out = Command::new(exe)
            .args([
                "--exact",
                "workers::smartctl::tests::locate_child",
                "--ignored",
                "--test-threads=1",
            ])
            .env("PATH", dir.path())
            .env(FAKE_DIR_ENV, dir.path())
            .output()
            .expect("run child");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}");
        assert!(stdout.contains("1 passed"), "{stdout}");
    }

    #[test]
    #[ignore = "run by locate_ignores_path with a fake PATH"]
    fn locate_child() {
        let fake =
            PathBuf::from(std::env::var_os(FAKE_DIR_ENV).expect("run by locate_ignores_path"));
        match locate() {
            Ok((pkexec, smartctl)) => {
                assert!(!pkexec.starts_with(&fake), "{}", pkexec.display());
                assert!(!smartctl.starts_with(&fake), "{}", smartctl.display());
                let parent = |p: &Path| p.parent().and_then(Path::to_str).map(str::to_owned);
                assert!(
                    PKEXEC_DIRS
                        .iter()
                        .any(|d| parent(&pkexec).as_deref() == Some(*d))
                );
                assert!(
                    SMARTCTL_DIRS
                        .iter()
                        .any(|d| parent(&smartctl).as_deref() == Some(*d))
                );
            }
            Err(e) => assert!(&*e == NOT_INSTALLED || &*e == NO_PKEXEC, "{e}"),
        }
    }

    #[test]
    fn pkexec_cancel_and_denial() {
        assert_eq!(run_fake("exit 126\n", TIMEOUT), Err(Arc::from(CANCELLED)));
        assert_eq!(run_fake("exit 127\n", TIMEOUT), Err(Arc::from(DENIED)));
    }

    #[test]
    fn timeout_kills() {
        let begin = Instant::now();
        let got = run_fake("exec sleep 30\n", Duration::from_secs(1));
        assert_eq!(got, Err(Arc::from(NO_ANSWER)));
        assert!(
            begin.elapsed() < Duration::from_secs(10),
            "{:?}",
            begin.elapsed()
        );
    }

    #[test]
    fn exit_bits_and_output() {
        // Arguments, environment and stdout reach the caller; bit 3 (disk failing) still parses.
        let script = r#"[ "$1 $2 $3" = "--json=c -a /dev/sda" ] || exit 1
[ "$PATH" = "/usr/sbin:/usr/bin" ] && [ "$LANG" = "C" ] && [ -z "$HOME" ] || exit 1
printf '{"smart_status":{"passed":false}}'
exit 8
"#;
        let out = run_fake(script, TIMEOUT).expect("output");
        let data = parse_smartctl_json(&out, 0).expect("parse");
        assert_eq!(data.health, valid(Arc::from("Failing")));
        assert_eq!(run_fake("exit 2\n", TIMEOUT), Err(Arc::from(CANNOT_OPEN)));
        assert_eq!(run_fake("exit 1\n", TIMEOUT), Err(Arc::from(CANNOT_OPEN)));
    }

    #[test]
    fn output_too_large() {
        let script = format!("head -c {} /dev/zero\n", MAX_OUTPUT + 10);
        assert_eq!(run_fake(&script, TIMEOUT), Err(Arc::from(TOO_LARGE)));
    }
}
