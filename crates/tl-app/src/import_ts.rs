//! Import TaskSlinger `UserSettings.ini` into `Settings` and `UiState` (APP-034).
//!
//! The file is picked by the user, so it is untrusted input. Text above `MAX_INI_BYTES` is
//! refused before parsing. Keys map through the APP-032 origin table. D-018: only values that are
//! a plain `true`/`false` or a decimal integer inside the target range are imported, because the
//! INI section names and value encodings are not documented yet. Every other key is skipped by
//! name. Values are never logged.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::path::Path;

use tl_model::settings::{Settings, UiState};
use tl_ui::app::{ImportSummary, ImportedSettings};
use tl_ui::theme::share_code::MAX_IMPORT_BYTES;
use tracing::{debug, info};

/// The largest `UserSettings.ini` accepted, 1 MiB.
pub const MAX_INI_BYTES: usize = 1 << 20;

/// The error text for a file above `MAX_INI_BYTES`.
pub const TOO_LARGE: &str = "File is too large to be a TaskSlinger settings file";

/// At most this many skipped key names go to the debug log.
const MAX_LOGGED_SKIPS: usize = 50;

/// What an import brought over.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportResult {
    pub settings: Settings,
    pub state: UiState,
    /// The number of mapped keys that were applied.
    pub imported: usize,
    /// `section.key` names that were not applied: unknown, unparseable, out of range or with a
    /// target that D-018 does not import yet.
    pub skipped: Vec<String>,
}

/// Why a `UserSettings.ini` could not be read.
#[derive(Debug)]
pub enum ImportError {
    /// Larger than `MAX_INI_BYTES`.
    TooLarge,
    Io(std::io::Error),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str(TOO_LARGE),
            Self::Io(e) => write!(f, "Could not read the TaskSlinger settings file: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Reads `path` as INI text. Reads at most one byte past the cap, so a huge file is never loaded.
pub fn read_ini(path: &Path) -> Result<String, ImportError> {
    let file = std::fs::File::open(path).map_err(ImportError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_INI_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ImportError::Io)?;
    decode_ini(&bytes)
}

/// Refuses bytes above `MAX_INI_BYTES`, then decodes them. A UTF-16 byte order mark (`FF FE` or
/// `FE FF`) selects UTF-16, because TaskSlinger is a Windows program; anything else is read as
/// UTF-8. Invalid sequences and a trailing odd byte become U+FFFD.
pub fn decode_ini(bytes: &[u8]) -> Result<String, ImportError> {
    if bytes.len() > MAX_INI_BYTES {
        return Err(ImportError::TooLarge);
    }
    let unit: fn([u8; 2]) -> u16 = match bytes {
        [0xFF, 0xFE, ..] => u16::from_le_bytes,
        [0xFE, 0xFF, ..] => u16::from_be_bytes,
        _ => return Ok(String::from_utf8_lossy(bytes).into_owned()),
    };
    let (pairs, rest) = bytes[2..].as_chunks::<2>();
    let odd = !rest.is_empty();
    let units = pairs.iter().map(|&pair| unit(pair));
    let mut text: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if odd {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    Ok(text)
}

/// Splits INI text into `(section, key, value)` triples in file order. Handles `[Section]`
/// headers, `key=value` lines, `;` and `#` comment lines, CRLF line ends and a leading UTF-8 BOM.
/// Keys and values are trimmed. Lines without `=` and lines with an empty key are ignored. Keys
/// before the first header have the section `""`. Text above `MAX_INI_BYTES` gives no entries.
pub fn parse_ini(text: &str) -> Vec<(String, String, String)> {
    if text.len() > MAX_INI_BYTES {
        return Vec::new();
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut section = String::new();
    let mut out = Vec::new();
    // `lines` drops the `\r` of CRLF; `trim` drops a stray one.
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.push((section.clone(), key.to_owned(), value.trim().to_owned()));
    }
    out
}

/// The import target: both files at once, so one accessor type covers settings and state keys.
struct Target {
    settings: Settings,
    state: UiState,
}

/// How a mapped value is stored.
enum Field {
    Bool(fn(&mut Target) -> &mut bool),
    U8(fn(&mut Target) -> &mut u8),
    U16(fn(&mut Target) -> &mut u16),
    U32(fn(&mut Target) -> &mut u32),
}

/// One APP-032 origin row whose TaskSlinger source D-018 can import.
struct Row {
    /// The TaskSlinger key, dotted when the docs write it with its section (`Window.Width`).
    ts: &'static str,
    /// The dotted key that `Settings::clamp` or `UiState::clamp` reports when it changes the value.
    /// Unused for booleans.
    clamp_key: &'static str,
    field: Field,
}

const fn bool_row(ts: &'static str, field: fn(&mut Target) -> &mut bool) -> Row {
    Row {
        ts,
        clamp_key: "",
        field: Field::Bool(field),
    }
}

/// The APP-032 rows with a TaskSlinger source and a boolean or integer target. Rows with a
/// string or enum target (`DefaultTab`, `UI.Language`, theme names, `ProcessNetworkRateUnit`,
/// `TreeView`, `TaskManagerGrouping`, `ColumnResizeBehavior`, `CpuPerCore`, `HiddenGpuEngines`,
/// `Graphics.*`, column, sort and graph layouts) are skipped until the owner supplies a sample
/// file (D-018).
const ROWS: &[Row] = &[
    bool_row("ThemeSyncWithWindows", |t| {
        &mut t.settings.theme.follow_system
    }),
    Row {
        ts: "UIScale",
        clamp_key: "appearance.ui_scale_pct",
        field: Field::U16(|t| &mut t.settings.appearance.ui_scale_pct),
    },
    Row {
        ts: "RowHeightPct",
        clamp_key: "appearance.row_height_pct",
        field: Field::U16(|t| &mut t.settings.appearance.row_height_pct),
    },
    bool_row("RoundedCorners", |t| {
        &mut t.settings.appearance.rounded_corners
    }),
    bool_row("ProcessValueColorCoding", |t| {
        &mut t.settings.appearance.heat_coloring
    }),
    Row {
        ts: "ProcessValueColorCodingIntensityPct",
        clamp_key: "appearance.heat_intensity_pct",
        field: Field::U8(|t| &mut t.settings.appearance.heat_intensity_pct),
    },
    Row {
        ts: "DropShadowIntensityPct",
        clamp_key: "appearance.shadow_intensity_pct",
        field: Field::U8(|t| &mut t.settings.appearance.shadow_intensity_pct),
    },
    bool_row("AnimationsEnabled", |t| &mut t.settings.motion.animations),
    bool_row("SmoothGraphs", |t| &mut t.settings.motion.smooth_graphs),
    bool_row("SmoothScrolling", |t| &mut t.settings.motion.smooth_scroll),
    Row {
        ts: "SmoothScrollAmount",
        clamp_key: "motion.smooth_scroll_amount",
        field: Field::U8(|t| &mut t.settings.motion.smooth_scroll_amount),
    },
    Row {
        ts: "MainTabAnimationMs",
        clamp_key: "motion.tab_switch_ms",
        field: Field::U16(|t| &mut t.settings.motion.tab_switch_ms),
    },
    bool_row("NetworkRateAutoScale", |t| {
        &mut t.settings.units.network_auto_scale
    }),
    Row {
        ts: "RefreshIntervalMs",
        clamp_key: "processes.interval_ms",
        field: Field::U32(|t| &mut t.settings.processes.interval_ms),
    },
    bool_row("TreeGuideLines", |t| {
        &mut t.settings.processes.tree_guide_lines
    }),
    bool_row("HighlightNewExitedProcesses", |t| {
        &mut t.settings.processes.highlight_new_exited
    }),
    bool_row("FilterShowChildren", |t| {
        &mut t.settings.processes.filter_include_children
    }),
    bool_row("HideMicrosoftProcesses", |t| {
        &mut t.settings.processes.hide_system
    }),
    bool_row("HideUnnamedHandles", |t| {
        &mut t.settings.properties.hide_unnamed_handles
    }),
    bool_row("ModulesDllOnly", |t| {
        &mut t.settings.properties.shared_objects_only
    }),
    bool_row("CpuKernel", |t| &mut t.settings.performance.cpu_kernel),
    bool_row("ShowCpuGraph", |t| &mut t.settings.graphs.minigraph_cpu),
    bool_row("ShowRamGraph", |t| &mut t.settings.graphs.minigraph_memory),
    bool_row("ShowDiskGraph", |t| &mut t.settings.graphs.minigraph_disk),
    bool_row("ShowNetworkGraph", |t| {
        &mut t.settings.graphs.minigraph_network
    }),
    bool_row("ProcessGraphs.ShowGpuTemperatureGraph", |t| {
        &mut t.settings.graphs.minigraph_gpu_temperature
    }),
    bool_row("KeepDetachedOpenOnMainClose", |t| {
        &mut t.settings.graphs.keep_detached_on_close
    }),
    Row {
        ts: "HistoryStorageDurationHours",
        clamp_key: "history.retention_h",
        field: Field::U8(|t| &mut t.settings.history.retention_h),
    },
    bool_row("HistoryPersistToDisk", |t| &mut t.settings.history.persist),
    bool_row("CloseWithEsc", |t| &mut t.settings.window.close_with_esc),
    bool_row("Window.AlwaysOnTop", |t| {
        &mut t.settings.window.always_on_top
    }),
    bool_row("CloseToTray", |t| &mut t.settings.tray.close_to_tray),
    bool_row("MinimizeToTray", |t| &mut t.settings.tray.minimize_to_tray),
    bool_row("AlwaysShowTrayIcon", |t| &mut t.settings.tray.always_show),
    bool_row("StartWithWindows", |t| {
        &mut t.settings.startup.start_at_login
    }),
    bool_row("StartMinimized", |t| {
        &mut t.settings.startup.start_minimized
    }),
    bool_row("Connections.AutoFetchPublicIP", |t| {
        &mut t.settings.network.auto_fetch_public_ip
    }),
    bool_row("PublicIPFetchNoticeAccepted", |t| {
        &mut t.settings.network.public_ip_notice_accepted
    }),
    bool_row("AlwaysLaunchElevated", |t| {
        &mut t.settings.privilege.unlock_at_launch
    }),
    bool_row("UacLessElevation", |t| {
        &mut t.settings.privilege.passwordless
    }),
    bool_row("ShowFps", |t| &mut t.settings.debug.show_fps),
    Row {
        ts: "Window.Width",
        clamp_key: "window.width",
        field: Field::U32(|t| &mut t.state.window.width),
    },
    Row {
        ts: "Window.Height",
        clamp_key: "window.height",
        field: Field::U32(|t| &mut t.state.window.height),
    },
    bool_row("Window.Maximized", |t| &mut t.state.window.maximized),
    bool_row("ProcessesHistoryMode", |t| {
        &mut t.state.processes.history_mode
    }),
    bool_row("TaskManagerAppsExpanded", |t| {
        &mut t.state.processes.group_expanded.apps
    }),
    bool_row("TaskManagerBackgroundExpanded", |t| {
        &mut t.state.processes.group_expanded.background
    }),
    bool_row("TaskManagerWindowsExpanded", |t| {
        &mut t.state.processes.group_expanded.system
    }),
    Row {
        ts: "Performance.NavWidth",
        clamp_key: "performance.nav_width",
        field: Field::U16(|t| &mut t.state.performance.nav_width),
    },
];

/// The row for an INI entry. The section is not documented, so a row matches when the dotted
/// `section.key` name equals its TaskSlinger name or ends with `.` plus that name, ignoring
/// ASCII case.
fn find_row(name: &str) -> Option<(usize, &'static Row)> {
    ROWS.iter().enumerate().find(|(_, row)| {
        let (n, ts) = (name.as_bytes(), row.ts.as_bytes());
        n.eq_ignore_ascii_case(ts)
            || (n.len() > ts.len()
                && n[n.len() - ts.len() - 1] == b'.'
                && n[n.len() - ts.len()..].eq_ignore_ascii_case(ts))
    })
}

/// `true` or `false`, ignoring ASCII case. Anything else is not a plain boolean.
fn parse_bool(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// A decimal integer: ASCII digits with an optional leading `-`.
fn parse_int(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Sets an integer field when `value` fits its type and both clamps leave it alone. An
/// out-of-range value is skipped, not clamped (D-018), so the old value is put back.
fn apply_int<T>(
    target: &mut Target,
    field: fn(&mut Target) -> &mut T,
    row: &Row,
    value: &str,
) -> bool
where
    T: Copy + TryFrom<i64>,
{
    let Some(v) = parse_int(value).and_then(|n| T::try_from(n).ok()) else {
        return false;
    };
    let old = std::mem::replace(field(target), v);
    let mut changed = target.settings.clamp();
    changed.extend(target.state.clamp());
    if changed.contains(&row.clamp_key) {
        *field(target) = old;
        return false;
    }
    true
}

/// Applies one entry. Returns false when it must be skipped.
fn apply(target: &mut Target, row: &Row, value: &str) -> bool {
    match row.field {
        Field::Bool(field) => match parse_bool(value) {
            Some(v) => {
                *field(target) = v;
                true
            }
            None => false,
        },
        Field::U8(field) => apply_int(target, field, row, value),
        Field::U16(field) => apply_int(target, field, row, value),
        Field::U32(field) => apply_int(target, field, row, value),
    }
}

/// Maps the INI `text` onto copies of `base` with the APP-032 origin table (D-018 rules). A key
/// that appears twice counts once; the later value wins when it is valid.
pub fn import(text: &str, base: (&Settings, &UiState)) -> ImportResult {
    let mut target = Target {
        settings: base.0.clone(),
        state: base.1.clone(),
    };
    let mut applied = BTreeSet::new();
    let mut skipped = Vec::new();
    for (section, key, value) in parse_ini(text) {
        let name = if section.is_empty() {
            key
        } else {
            format!("{section}.{key}")
        };
        match find_row(&name) {
            Some((index, row)) if apply(&mut target, row, &value) => {
                applied.insert(index);
            }
            _ => skipped.push(name),
        }
    }
    if !skipped.is_empty() {
        // Key names only; values may hold anything (privacy rule).
        let shown = &skipped[..skipped.len().min(MAX_LOGGED_SKIPS)];
        debug!(
            target: "tasklens::settings",
            "TaskSlinger import skipped {} keys: {shown:?}",
            skipped.len()
        );
    }
    ImportResult {
        settings: target.settings,
        state: target.state,
        imported: applied.len(),
        skipped,
    }
}

/// The `AppEnv.read_ts_themes` callback: reads a picked `themes.json` as text, capped at
/// `MAX_IMPORT_BYTES`. tl-ui parses it. The error text is shown to the user and holds no path.
pub fn read_themes_file(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("The file is too large".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "The file is not valid UTF-8".to_owned())
}

/// The `AppEnv.import_taskslinger` callback: reads `path` and imports it onto copies of the
/// current `settings` and `state`. Writes nothing; tl-ui applies the result through
/// `TlSettings::update`, which persists it. The error text is shown to the user.
pub fn import_file(
    path: &Path,
    settings: &Settings,
    state: &UiState,
) -> Result<ImportedSettings, String> {
    let text = read_ini(path).map_err(|e| e.to_string())?;
    let result = import(&text, (settings, state));
    // Counts only, never values or the path (privacy rule).
    info!(
        target: "tasklens::settings",
        imported = result.imported,
        skipped = result.skipped.len(),
        "TaskSlinger settings imported"
    );
    Ok(ImportedSettings {
        settings: result.settings,
        state: result.state,
        summary: ImportSummary {
            settings: u32::try_from(result.imported).unwrap_or(u32::MAX),
            // Themes import with the themes.json bead.
            themes: 0,
        },
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn run(text: &str) -> ImportResult {
        import(text, (&Settings::default(), &UiState::default()))
    }

    #[test]
    fn parse_sections_and_comments() {
        let text = "; top comment\n# hash comment\nLoose = 1\n[ General ]\nA=b c\n\
                    no equals sign\n  ; indented comment\n[Window]\nWidth = 1400\n=empty key\n";
        let entries = parse_ini(text);
        let triple = |s: &str, k: &str, v: &str| (s.to_owned(), k.to_owned(), v.to_owned());
        assert_eq!(
            entries,
            vec![
                triple("", "Loose", "1"),
                triple("General", "A", "b c"),
                triple("Window", "Width", "1400"),
            ]
        );
    }

    #[test]
    fn crlf_accepted() {
        let text = "\u{feff}[Window]\r\nMaximized=true\r\nAnimationsEnabled=false\r\n";
        assert_eq!(
            parse_ini(text),
            vec![
                ("Window".into(), "Maximized".into(), "true".into()),
                ("Window".into(), "AnimationsEnabled".into(), "false".into()),
            ]
        );
        let r = run(text);
        assert!(r.state.window.maximized);
        assert!(!r.settings.motion.animations);
        assert_eq!(r.imported, 2);
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn bool_key_mapped() {
        let r = run("AnimationsEnabled=false\n");
        assert!(!r.settings.motion.animations);
        assert_eq!(r.imported, 1);
        assert!(r.skipped.is_empty());

        // Case-insensitive names and values, any section.
        let r = run("[Settings]\nclosetotray=True\nMinimizeToTray=FALSE\n");
        assert!(r.settings.tray.close_to_tray);
        assert!(!r.settings.tray.minimize_to_tray);
        assert_eq!(r.imported, 2);

        // Not a plain boolean: skipped, value kept.
        let r = run("[Settings]\nAnimationsEnabled=1\nSmoothGraphs=yes\n");
        assert!(r.settings.motion.animations);
        assert!(r.settings.motion.smooth_graphs);
        assert_eq!(r.imported, 0);
        assert_eq!(
            r.skipped,
            vec!["Settings.AnimationsEnabled", "Settings.SmoothGraphs"]
        );
    }

    #[test]
    fn int_key_in_range() {
        let r = run("RefreshIntervalMs=1000\nUIScale=125\n[Window]\nWidth=1600\nHeight=900\n");
        assert_eq!(r.settings.processes.interval_ms, 1000);
        assert_eq!(r.settings.appearance.ui_scale_pct, 125);
        assert_eq!(r.state.window.width, 1600);
        assert_eq!(r.state.window.height, 900);
        assert_eq!(r.imported, 4);

        let mut base = Settings::default();
        base.processes.interval_ms = 2000;
        let r = import("RefreshIntervalMs=1000\n", (&base, &UiState::default()));
        assert_eq!(r.settings.processes.interval_ms, 1000);
    }

    #[test]
    fn int_key_out_of_range_skipped() {
        let mut base = Settings::default();
        base.processes.interval_ms = 2000;
        let r = import("RefreshIntervalMs=50\n", (&base, &UiState::default()));
        assert_eq!(r.settings.processes.interval_ms, 2000);
        assert_eq!(r.imported, 0);
        assert_eq!(r.skipped, vec!["RefreshIntervalMs"]);

        // Off the 5 % step, past the type, negative, not decimal.
        let r = run(
            "UIScale=103\nDropShadowIntensityPct=300\nRowHeightPct=-90\nSmoothScrollAmount=0x10\n",
        );
        assert_eq!(r.settings, Settings::default());
        assert_eq!(r.imported, 0);
        assert_eq!(r.skipped.len(), 4);
    }

    #[test]
    fn position_skipped() {
        let r = run("Window.X=100\n");
        assert_eq!(r.imported, 0);
        assert_eq!(r.skipped, vec!["Window.X"]);

        let r = run("[Window]\nX=100\nY=40\nWidth=1400\n");
        assert_eq!(r.imported, 1);
        assert_eq!(r.skipped, vec!["Window.X", "Window.Y"]);
    }

    #[test]
    fn duplicate_key_counts_once() {
        let r = run("CloseToTray=false\nCloseToTray=true\n");
        assert!(r.settings.tray.close_to_tray);
        assert_eq!(r.imported, 1);
    }

    #[test]
    fn too_large_rejected() {
        let big = vec![b'a'; 2 * MAX_INI_BYTES];
        let err = decode_ini(&big).expect_err("2 MiB is refused");
        assert!(matches!(err, ImportError::TooLarge));
        assert_eq!(err.to_string(), TOO_LARGE);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("UserSettings.ini");
        std::fs::write(&path, &big).expect("write");
        assert!(matches!(read_ini(&path), Err(ImportError::TooLarge)));

        // Exactly 1 MiB is still accepted.
        assert!(decode_ini(&big[..MAX_INI_BYTES]).is_ok());

        // Text above the cap that skipped `decode_ini` gives nothing.
        let text = format!("AnimationsEnabled=false\n{}", "a".repeat(2 * MAX_INI_BYTES));
        assert!(parse_ini(&text).is_empty());
        assert_eq!(run(&text).imported, 0);
    }

    #[test]
    fn invalid_utf8_decoded_lossily() {
        let text = decode_ini(b"[W\xffindow]\nAnimationsEnabled=false\n\xfe=1\n").expect("small");
        let r = run(&text);
        assert!(!r.settings.motion.animations);
        assert_eq!(r.imported, 1);
        assert_eq!(r.skipped.len(), 1);
    }

    fn utf16(text: &str, le: bool) -> Vec<u8> {
        let mut bytes = if le {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for u in text.encode_utf16() {
            bytes.extend(if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        bytes
    }

    #[test]
    fn utf16_bom_decoded() {
        let text = "[Window]\r\nMaximized=true\r\nAnimationsEnabled=false\r\nUIScale=125\r\n";
        let utf8 = run(&decode_ini(text.as_bytes()).expect("small"));
        assert_eq!(utf8.imported, 3);
        for le in [true, false] {
            let decoded = decode_ini(&utf16(text, le)).expect("small");
            assert_eq!(decoded, text);
            assert_eq!(run(&decoded), utf8);
        }
    }

    #[test]
    fn utf16_broken_units_replaced() {
        // A lone high surrogate, then a key, then an odd trailing byte.
        let mut bytes = vec![0xFF, 0xFE, 0x00, 0xD8];
        bytes.extend(utf16("\nCloseToTray=true\n", true).split_off(2));
        bytes.push(0x41);
        let text = decode_ini(&bytes).expect("small");
        assert!(text.starts_with('\u{fffd}'));
        assert!(text.ends_with('\u{fffd}'));
        let r = run(&text);
        assert!(r.settings.tray.close_to_tray);
        assert_eq!(r.imported, 1);

        // A UTF-16 file above the cap is refused like any other.
        let big = utf16(&"a".repeat(MAX_INI_BYTES), true);
        assert!(matches!(decode_ini(&big), Err(ImportError::TooLarge)));
        // Only a BOM gives an empty text.
        assert_eq!(decode_ini(&[0xFE, 0xFF]).expect("small"), "");
    }

    #[test]
    fn binary_garbage_imports_nothing() {
        let garbage: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let r = run(&decode_ini(&garbage).expect("small"));
        assert_eq!(r.imported, 0);
    }

    proptest! {
        #[test]
        fn never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let text = decode_ini(&bytes).expect("below the cap");
            let r = run(&text);
            prop_assert!(r.imported <= ROWS.len());
        }

        #[test]
        fn never_panics_on_ini_shaped_text(
            lines in proptest::collection::vec(
                ("[A-Za-z.]{0,24}", "[ -~]{0,16}", 0u8..4),
                0..64,
            )
        ) {
            let text: String = lines
                .iter()
                .map(|(k, v, kind)| match kind {
                    0 => format!("[{k}]\r\n"),
                    1 => format!("{k}={v}\n"),
                    2 => format!("; {v}\n"),
                    _ => format!("{v}\n"),
                })
                .collect();
            let r = run(&text);
            let mut settings = r.settings.clone();
            let mut state = r.state.clone();
            // Nothing out of range is ever imported.
            prop_assert!(settings.clamp().is_empty());
            prop_assert!(state.clamp().is_empty());
        }
    }
}
