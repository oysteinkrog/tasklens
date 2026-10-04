//! The built-in themes and the strict theme object parser (UI-FND-001, UI-FND-041, APP-035).
//! The theme file is embedded in the binary and never read from disk (ADR-011).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use gpui_kit::{Hsla, rgb, rgba};
use serde_json::Value;

use super::{GraphPalette, StatusColors, ThemeDef, ThemeMode, ThemeSlot};

const BUILTIN_JSON: &str = include_str!("../../../../assets/themes/builtin.json");

/// TaskSlinger's theme order (APP-035). Built-in themes keep this relative order.
pub const TS_THEME_ORDER: [&str; 12] = [
    "Default Dark",
    "Vivid Nightfall",
    "Catppuccin Frappé",
    "Catppuccin Macchiato",
    "Catppuccin Mocha",
    "Dracula",
    "OLED Black",
    "Default Light",
    "Windows 7 Aero",
    "Golden Summer Fields",
    "Soft Pink Delight",
    "Catppuccin Latte",
];

const STATUS_KEYS: [&str; 4] = ["success", "warning", "danger", "info"];

const GRAPH_KEYS: [&str; 13] = [
    "cpu_fill",
    "cpu_line",
    "kernel_line",
    "mem_fill",
    "mem_line",
    "gpu_fill",
    "gpu_line",
    "disk_fill",
    "disk_line",
    "net_fill",
    "net_line",
    "grid",
    "chart_bg",
];

/// A colour string that is not `#RRGGBB` or `#RRGGBBAA`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorParseError {
    pub value: String,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "colour {:?} is not #RRGGBB or #RRGGBBAA", self.value)
    }
}

impl std::error::Error for ColorParseError {}

/// Parses `#RRGGBB` or `#RRGGBBAA`. Hex digits may be upper or lower case. Anything else is an
/// error.
pub fn parse_hex(s: &str) -> Result<Hsla, ColorParseError> {
    let err = || ColorParseError {
        value: s.to_string(),
    };
    let digits = s.strip_prefix('#').ok_or_else(err)?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err());
    }
    let value = u32::from_str_radix(digits, 16).map_err(|_| err())?;
    match digits.len() {
        6 => Ok(rgb(value).into()),
        8 => Ok(rgba(value).into()),
        _ => Err(err()),
    }
}

/// Why a theme file or theme object could not be parsed.
#[derive(Debug)]
pub enum ThemeParseError {
    Json(serde_json::Error),
    NotAnObject,
    MissingThemesArray,
    MissingName {
        index: usize,
    },
    MissingSlot {
        theme: Arc<str>,
        key: &'static str,
    },
    BadColor {
        theme: Arc<str>,
        key: String,
        value: String,
    },
}

impl fmt::Display for ThemeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeParseError::Json(e) => write!(f, "theme file is not valid JSON: {e}"),
            ThemeParseError::NotAnObject => write!(f, "theme file is not a JSON object"),
            ThemeParseError::MissingThemesArray => {
                write!(f, "theme file has no \"themes\" array")
            }
            ThemeParseError::MissingName { index } => {
                write!(f, "theme {index} has no \"name\" string")
            }
            ThemeParseError::MissingSlot { theme, key } => {
                write!(f, "theme {theme:?} is missing key {key:?}")
            }
            ThemeParseError::BadColor { theme, key, value } => {
                write!(f, "theme {theme:?} key {key:?} has bad value {value:?}")
            }
        }
    }
}

impl std::error::Error for ThemeParseError {}

impl From<serde_json::Error> for ThemeParseError {
    fn from(e: serde_json::Error) -> Self {
        ThemeParseError::Json(e)
    }
}

/// Reads one required colour: a slot or `accent`.
fn required_color(
    obj: &BTreeMap<String, Value>,
    theme: &Arc<str>,
    key: &'static str,
) -> Result<Hsla, ThemeParseError> {
    match obj.get(key) {
        None => Err(ThemeParseError::MissingSlot {
            theme: theme.clone(),
            key,
        }),
        Some(v) => color_value(v, theme, key),
    }
}

fn color_value(v: &Value, theme: &Arc<str>, key: &str) -> Result<Hsla, ThemeParseError> {
    let bad = || ThemeParseError::BadColor {
        theme: theme.clone(),
        key: key.to_string(),
        value: v.to_string(),
    };
    v.as_str()
        .ok_or_else(bad)
        .and_then(|s| parse_hex(s).map_err(|_| bad()))
}

/// Reads an optional nested colour object (`status` or `graph`). Returns `Ok(None)` when the
/// object is absent, or when it is broken and `strict` is false.
fn nested_colors<const N: usize>(
    obj: &BTreeMap<String, Value>,
    theme: &Arc<str>,
    key: &'static str,
    sub_keys: &[&'static str; N],
    full_keys: &[&'static str; N],
    strict: bool,
) -> Result<Option<[Hsla; N]>, ThemeParseError> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let parsed = (|| {
        let Some(map) = v.as_object() else {
            return Err(ThemeParseError::BadColor {
                theme: theme.clone(),
                key: key.to_string(),
                value: v.to_string(),
            });
        };
        let mut out = [Hsla::default(); N];
        for (i, sub) in sub_keys.iter().enumerate() {
            out[i] = match map.get(*sub) {
                None => {
                    return Err(ThemeParseError::MissingSlot {
                        theme: theme.clone(),
                        key: full_keys[i],
                    });
                }
                Some(c) => color_value(c, theme, full_keys[i])?,
            };
        }
        Ok(out)
    })();
    match parsed {
        Ok(out) => Ok(Some(out)),
        Err(e) if strict => Err(e),
        Err(_) => Ok(None),
    }
}

/// Parses one theme object. Reads `name`, optional `mode`, the 22 upper-case slots, `accent`,
/// optional `status` (4 keys) and optional `graph` (13 keys). Every other key goes into `extra`.
///
/// A missing or bad slot or `accent` is always an error; the user store repairs those before it
/// calls this (E17-008). With `strict = true` a broken `mode`, `status` or `graph` is also an
/// error; with `strict = false` it is dropped, so `TlTheme::from_def` uses the fallback theme's
/// values. A missing name returns `MissingName { index: 0 }`; the caller sets the index.
pub fn parse_theme_object(
    obj: &BTreeMap<String, Value>,
    strict: bool,
) -> Result<ThemeDef, ThemeParseError> {
    let name: Arc<str> = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or(ThemeParseError::MissingName { index: 0 })?
        .into();

    let mode = match obj.get("mode") {
        None => None,
        Some(v) => match v.as_str() {
            Some("dark") => Some(ThemeMode::Dark),
            Some("light") => Some(ThemeMode::Light),
            _ if strict => {
                return Err(ThemeParseError::BadColor {
                    theme: name,
                    key: "mode".to_string(),
                    value: v.to_string(),
                });
            }
            _ => None,
        },
    };

    let mut slots = [Hsla::default(); 22];
    for (i, slot) in ThemeSlot::ALL[..22].iter().enumerate() {
        slots[i] = required_color(obj, &name, slot.json_key())?;
    }
    let accent = required_color(obj, &name, ThemeSlot::Accent.json_key())?;

    let status = nested_colors(
        obj,
        &name,
        "status",
        &STATUS_KEYS,
        &[
            "status.success",
            "status.warning",
            "status.danger",
            "status.info",
        ],
        strict,
    )?
    .map(|[success, warning, danger, info]| StatusColors {
        success,
        warning,
        danger,
        info,
    });

    let graph = nested_colors(
        obj,
        &name,
        "graph",
        &GRAPH_KEYS,
        &[
            "graph.cpu_fill",
            "graph.cpu_line",
            "graph.kernel_line",
            "graph.mem_fill",
            "graph.mem_line",
            "graph.gpu_fill",
            "graph.gpu_line",
            "graph.disk_fill",
            "graph.disk_line",
            "graph.net_fill",
            "graph.net_line",
            "graph.grid",
            "graph.chart_bg",
        ],
        strict,
    )?
    .map(
        |[
            cpu_fill,
            cpu_line,
            kernel_line,
            mem_fill,
            mem_line,
            gpu_fill,
            gpu_line,
            disk_fill,
            disk_line,
            net_fill,
            net_line,
            grid,
            chart_bg,
        ]| GraphPalette {
            cpu_fill,
            cpu_line,
            kernel_line,
            mem_fill,
            mem_line,
            gpu_fill,
            gpu_line,
            disk_fill,
            disk_line,
            net_fill,
            net_line,
            grid,
            chart_bg,
        },
    );

    let known = |k: &str| {
        matches!(k, "name" | "mode" | "status" | "graph")
            || ThemeSlot::ALL.iter().any(|s| s.json_key() == k)
    };
    let extra = obj
        .iter()
        .filter(|(k, _)| !known(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Ok(ThemeDef {
        name,
        mode,
        slots,
        accent,
        status,
        graph,
        extra,
    })
}

fn parse_builtin(text: &str) -> Result<Vec<ThemeDef>, ThemeParseError> {
    let root: Value = serde_json::from_str(text)?;
    let root = root.as_object().ok_or(ThemeParseError::NotAnObject)?;
    let themes = root
        .get("themes")
        .and_then(Value::as_array)
        .ok_or(ThemeParseError::MissingThemesArray)?;
    themes
        .iter()
        .enumerate()
        .map(|(index, v)| {
            let obj: BTreeMap<String, Value> = v
                .as_object()
                .ok_or(ThemeParseError::MissingName { index })?
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            parse_theme_object(&obj, true).map_err(|e| match e {
                ThemeParseError::MissingName { .. } => ThemeParseError::MissingName { index },
                e => e,
            })
        })
        .collect()
}

/// Parses the embedded built-in themes. Runs once at `TasklensUi::init`. Panics with the error
/// text when the embedded file is broken; the tests catch that at build time.
pub fn load() -> Arc<[ThemeDef]> {
    match parse_builtin(BUILTIN_JSON) {
        Ok(themes) => themes.into(),
        Err(e) => panic!("built-in themes are broken: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Rgba;

    /// The 22 slots plus accent, in `ThemeSlot::ALL` order.
    const DARK: [u32; 23] = [
        0x191B1D, 0x27292B, 0x202224, 0x202224, 0x25282B, 0x2E3235, 0x191B1D, 0x1D1F20, 0x232629,
        0x075B84, 0x2B2F33, 0x2A2E31, 0x104B68, 0x202224, 0x27292B, 0x1A1C1E, 0x494F54, 0xD6D8D9,
        0x959A9C, 0xFFFFFF, 0xBFD9E6, 0x3E2A28, 0x0277A6,
    ];
    const LIGHT: [u32; 23] = [
        0xF3F4F6, 0xE4E6E9, 0xEBEDEF, 0xFFFFFF, 0xFFFFFF, 0xDDE1E5, 0xFFFFFF, 0xF6F7F8, 0xE8EBEE,
        0xCFE6F2, 0xD4D8DC, 0xFFFFFF, 0xE1EEF5, 0xEBEDEF, 0xE0E3E6, 0xEBEDEF, 0xB5BBC0, 0x1B1F22,
        0x5F666B, 0x0F1416, 0x3E5460, 0xF6D5CF, 0x0277A6,
    ];

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    /// The first built-in theme object as a map, for edits in tests.
    fn dark_object() -> BTreeMap<String, Value> {
        let root: Value = serde_json::from_str(BUILTIN_JSON).unwrap();
        root["themes"][0]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn slot_values(def: &ThemeDef) -> Vec<Hsla> {
        def.slots.iter().copied().chain([def.accent]).collect()
    }

    fn collect_colour_strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) if s.starts_with('#') => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| collect_colour_strings(i, out)),
            Value::Object(map) => {
                for (k, v) in map {
                    if k != "name" && k != "mode" {
                        assert!(
                            v.is_object()
                                || v.is_array()
                                || v.as_str().is_some_and(|s| s.starts_with('#')),
                            "key {k} has a non-colour value {v}"
                        );
                    }
                    collect_colour_strings(v, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn builtin_json_parses() {
        let themes = load();
        assert_eq!(themes.len(), 2);
        for t in themes.iter() {
            assert!(t.status.is_some(), "{} has no status", t.name);
            assert!(t.graph.is_some(), "{} has no graph", t.name);
            assert!(t.extra.is_empty(), "{} has extra keys", t.name);
        }
        let root: Value = serde_json::from_str(BUILTIN_JSON).unwrap();
        let mut colours = Vec::new();
        collect_colour_strings(&root, &mut colours);
        // 2 themes x (22 slots + accent + 4 status + 13 graph).
        assert_eq!(colours.len(), 2 * (23 + 4 + 13));
        for c in &colours {
            assert_eq!(c.len(), 7, "{c} is not #RRGGBB");
            assert!(c[1..].bytes().all(|b| b.is_ascii_hexdigit()), "{c}");
        }
    }

    #[test]
    fn all_slots_present_in_every_theme() {
        let themes = load();
        assert_eq!(&*themes[0].name, "Default Dark");
        assert_eq!(&*themes[1].name, "Default Light");
        assert_eq!(themes[0].mode, Some(ThemeMode::Dark));
        assert_eq!(themes[1].mode, Some(ThemeMode::Light));
        for (def, table) in themes.iter().zip([DARK, LIGHT]) {
            let want: Vec<Hsla> = table.iter().map(|&v| hex(v)).collect();
            assert_eq!(slot_values(def), want, "{}", def.name);
        }
    }

    #[test]
    fn names_in_ts_order() {
        let themes = load();
        let mut order = TS_THEME_ORDER.iter();
        for t in themes.iter() {
            assert!(
                order.any(|n| *n == &*t.name),
                "{} is out of TaskSlinger order",
                t.name
            );
        }
    }

    #[test]
    fn light_theme_mode_inferred_if_missing() {
        let root: Value = serde_json::from_str(BUILTIN_JSON).unwrap();
        let mut obj: BTreeMap<String, Value> = root["themes"][1]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        obj.remove("mode");
        assert_eq!(obj["WINDOW_BG"], "#F3F4F6");
        let def = parse_theme_object(&obj, true).unwrap();
        assert_eq!(def.mode, None);
        assert_eq!(def.effective_mode(), ThemeMode::Light);
    }

    #[test]
    fn strict_rejects_missing_slot_and_bad_colour() {
        let mut obj = dark_object();
        obj.remove("HEADER");
        match parse_theme_object(&obj, true) {
            Err(ThemeParseError::MissingSlot { theme, key }) => {
                assert_eq!(&*theme, "Default Dark");
                assert_eq!(key, "HEADER");
            }
            other => panic!("expected MissingSlot, got {other:?}"),
        }
        let msg = parse_theme_object(&obj, true).unwrap_err().to_string();
        assert!(
            msg.contains("Default Dark") && msg.contains("HEADER"),
            "{msg}"
        );

        let mut obj = dark_object();
        obj.insert("accent".into(), "blue".into());
        match parse_theme_object(&obj, true) {
            Err(ThemeParseError::BadColor { theme, key, value }) => {
                assert_eq!(&*theme, "Default Dark");
                assert_eq!(key, "accent");
                assert_eq!(value, "\"blue\"");
            }
            other => panic!("expected BadColor, got {other:?}"),
        }

        let mut obj = dark_object();
        obj.get_mut("graph")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("grid".into(), "#12345".into());
        match parse_theme_object(&obj, true) {
            Err(ThemeParseError::BadColor { key, .. }) => assert_eq!(key, "graph.grid"),
            other => panic!("expected BadColor, got {other:?}"),
        }
        let lenient = parse_theme_object(&obj, false).unwrap();
        assert!(lenient.graph.is_none());

        let mut obj = dark_object();
        obj.remove("name");
        assert!(matches!(
            parse_theme_object(&obj, true),
            Err(ThemeParseError::MissingName { .. })
        ));

        let mut obj = dark_object();
        obj.insert("FOO".into(), "bar".into());
        let def = parse_theme_object(&obj, true).unwrap();
        assert_eq!(def.extra.len(), 1);
        assert_eq!(def.extra["FOO"], "bar");

        assert!(matches!(
            parse_builtin("{\"themes\":"),
            Err(ThemeParseError::Json(_))
        ));
        assert!(matches!(
            parse_builtin("[]"),
            Err(ThemeParseError::NotAnObject)
        ));
        assert!(matches!(
            parse_builtin("{}"),
            Err(ThemeParseError::MissingThemesArray)
        ));
        assert!(matches!(
            parse_builtin("{\"themes\":[{\"name\":\"A\"},{}]}"),
            Err(ThemeParseError::MissingSlot {
                key: "WINDOW_BG",
                ..
            })
        ));
        assert!(matches!(
            parse_builtin("{\"themes\":[{}]}"),
            Err(ThemeParseError::MissingName { index: 0 })
        ));
    }

    #[test]
    fn parse_hex_forms() {
        for bad in [
            "#12345", "red", "#GG0000", "", "#", "0277A6", "#+277A6", "#0277A6C",
        ] {
            assert!(parse_hex(bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(parse_hex("#0277A6").unwrap(), hex(0x0277A6));
        assert_eq!(parse_hex("#0277a6").unwrap(), hex(0x0277A6));
        assert_eq!(
            parse_hex("#0277A6CC").unwrap(),
            Hsla::from(rgba(0x0277A6CC))
        );
    }

    fn luminance(c: Hsla) -> f32 {
        let c: Rgba = c.into();
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn contrast_floor() {
        use ThemeSlot::*;
        // Secondary text is checked on every background slot (R-269).
        let secondary_bgs = [
            ListBg, WindowBg, RowAlt, RowHover, Header, MenuBg, Toolbar, Input, InputFocus, TabBg,
            Titlebar,
        ];
        let pairs: Vec<(ThemeSlot, ThemeSlot, f32)> = [
            (TextPrimary, ListBg, 4.5),
            (TextPrimary, WindowBg, 4.5),
            (TextPrimary, RowAlt, 4.5),
            (TextPrimary, MenuBg, 4.5),
            (TextPrimary, Header, 4.5),
            (TextSelectedPrimary, RowSelected, 4.4),
            (TextSelectedSecondary, RowSelected, 4.4),
        ]
        .into_iter()
        .chain(secondary_bgs.map(|bg| (TextSecondary, bg, 4.5)))
        .collect();
        let mut failures = Vec::new();
        for def in load().iter() {
            for &(fg, bg, floor) in &pairs {
                let ratio = contrast(def.slots[fg as usize], def.slots[bg as usize]);
                if ratio < floor {
                    failures.push(format!(
                        "{}: {} on {} is {ratio:.2}, below {floor}",
                        def.name,
                        fg.json_key(),
                        bg.json_key()
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
