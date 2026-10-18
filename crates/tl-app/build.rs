//! Records the git short hash as `TASKLENS_GIT_SHA` and the build date as `TASKLENS_BUILD_DATE`
//! for the crash report and the About window. Never fails the build: without git or a checkout
//! the hash is `unknown`, and the date always has a value (UI-SHL-092).

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Length of the short hash shown in About (UI-SHL-092).
const SHA_LEN: usize = 7;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let manifest_dir = Path::new(&manifest_dir);

    for rel in ["../../.git/HEAD", "../../.git/refs"] {
        if manifest_dir.join(rel).exists() {
            println!("cargo:rerun-if-changed={rel}");
        }
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let sha = git_short_sha(manifest_dir).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=TASKLENS_GIT_SHA={sha}");
    println!(
        "cargo:rustc-env=TASKLENS_BUILD_DATE={}",
        build_date(manifest_dir)
    );
}

fn git_short_sha(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", &format!("--short={SHA_LEN}"), "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let valid = sha.len() == SHA_LEN && sha.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then_some(sha)
}

/// `SOURCE_DATE_EPOCH`, else the HEAD commit date, else today (UTC). Always `YYYY-MM-DD`.
fn build_date(dir: &Path) -> String {
    if let Some(secs) = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        return civil_date(secs);
    }
    if let Some(date) = git_commit_date(dir) {
        return date;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    civil_date(now)
}

fn git_commit_date(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cs"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let date = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let b = date.as_bytes();
    let valid = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    valid.then_some(date)
}

/// UTC calendar date of a Unix timestamp (Howard Hinnant's civil-from-days).
fn civil_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
