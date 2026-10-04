//! Records the git short hash as `TASKLENS_GIT_SHA` for the crash report and the About window.
//! Never fails the build: without git or a checkout the value is `unknown`.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let manifest_dir = Path::new(&manifest_dir);

    for rel in ["../../.git/HEAD", "../../.git/refs"] {
        if manifest_dir.join(rel).exists() {
            println!("cargo:rerun-if-changed={rel}");
        }
    }

    let sha = git_short_sha(manifest_dir).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=TASKLENS_GIT_SHA={sha}");
}

fn git_short_sha(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let valid = sha.len() == 12 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then_some(sha)
}
