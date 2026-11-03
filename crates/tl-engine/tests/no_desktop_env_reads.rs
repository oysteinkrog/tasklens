//! No worker reads the desktop session from the process environment: the variables come from
//! `DesktopSession`, so a test engine without a desktop never sees the real one (tasklens-aujsq).

const KEYS: [&str; 3] = [
    "XDG_CURRENT_DESKTOP",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
];

#[test]
fn workers_never_read_desktop_env() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/workers");
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("workers dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read worker file");
        for (n, line) in text.lines().enumerate() {
            let reads_env = line.contains("env::var") || line.contains("var_os");
            if reads_env && KEYS.iter().any(|k| line.contains(k)) {
                hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "workers read the desktop env:\n{}",
        hits.join("\n")
    );
}
