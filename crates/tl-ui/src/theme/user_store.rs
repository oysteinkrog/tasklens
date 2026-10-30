//! The user themes file `$XDG_CONFIG_HOME/tasklens/themes.json` (UI-FND-042, ADR-011, R-018,
//! R-079): lenient loading with per-theme repair, Create copy, Delete and saving.
//!
//! tl-ui does no file I/O. tl-app reads the file text into `UiInit.themes_json` and performs
//! every `PersistTheme` request sent through the `ThemePersist` Global. Saving happens after
//! Create copy, Delete, Import and editor Done, never on activate.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use gpui_kit::{App, Global, Hsla, Rgba, SharedString};
use serde::Serialize;
use serde::ser::SerializeMap;
use serde_json::Value;
use tl_model::settings::ThemeSettings;
use tracing::warn;

use super::builtin::{parse_hex, parse_theme_object};
use super::store::{self, ActivateSource, ThemeError, ThemeEvent, ThemeStore};
use super::{ThemeDef, ThemeMode, ThemeSlot};
use crate::settings_global::TlSettings;

/// The most user themes the file keeps. Later ones are dropped at load.
pub const MAX_USER_THEMES: usize = 64;

/// What the theme store asks tl-app to write.
#[derive(Clone, Debug)]
pub enum PersistTheme {
    /// Write these user themes to themes.json with `write_atomic`.
    Save(Vec<ThemeDef>),
    /// Rename a themes.json that does not parse to `themes.json.corrupt-<unix_seconds>`.
    RenameCorrupt { unix_seconds: u64 },
}

/// The persist callback from `UiInit.persist_themes`. Installed by `TasklensUi::init`; tests
/// install a recording closure.
pub struct ThemePersist(pub Box<dyn Fn(PersistTheme)>);

impl Global for ThemePersist {}

/// Sends `req` to tl-app. Without a `ThemePersist` Global the request is dropped with a warning.
pub fn persist(req: PersistTheme, cx: &App) {
    match cx.try_global::<ThemePersist>() {
        Some(p) => (p.0)(req),
        None => warn!(target: "tasklens::ui", "no theme persist callback, request dropped"),
    }
}

/// One problem `load_user_themes` fixed in a theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Repair {
    /// A slot or `accent` was missing; the same-mode built-in value was used.
    MissingSlot { theme: Arc<str>, key: &'static str },
    /// A slot or `accent` was not `#RRGGBB` or `#RRGGBBAA`; the same-mode built-in value was used.
    BadColor { theme: Arc<str>, key: String },
    /// The theme at file position `index` had no name and was given `given`.
    Unnamed { index: usize, given: Arc<str> },
    /// A duplicate name got a " (N)" suffix.
    Renamed { from: Arc<str>, to: Arc<str> },
    /// The file had `total` themes; only the first `MAX_USER_THEMES` were kept.
    Truncated { total: usize },
}

/// The result of reading the themes file text.
#[derive(Clone, Debug, Default)]
pub struct LoadOutcome {
    pub themes: Vec<ThemeDef>,
    /// Unix seconds of the load when the file did not parse. The file is then renamed and the
    /// list reset.
    pub corrupt: Option<u64>,
    pub repairs: Vec<Repair>,
}

/// The Warning toast shown after the first frame when the themes file did not parse.
pub fn corrupt_toast_text(t: u64) -> SharedString {
    format!(
        "Themes file was unreadable and has been reset. The old file was kept as \
         themes.json.corrupt-{t}"
    )
    .into()
}

/// The built-in theme a user theme of `mode` takes missing values from.
fn fallback_def(builtins: &[ThemeDef], mode: ThemeMode) -> Option<&ThemeDef> {
    let name = match mode {
        ThemeMode::Dark => store::DEFAULT_DARK,
        ThemeMode::Light => store::DEFAULT_LIGHT,
    };
    builtins
        .iter()
        .find(|d| &*d.name == name)
        .or_else(|| builtins.iter().find(|d| d.effective_mode() == mode))
        .or_else(|| builtins.first())
}

/// The mode of a raw theme object: the `mode` key, else the `WINDOW_BG` lightness, else dark.
fn raw_mode(obj: &BTreeMap<String, Value>) -> ThemeMode {
    match obj.get("mode").and_then(Value::as_str) {
        Some("dark") => ThemeMode::Dark,
        Some("light") => ThemeMode::Light,
        _ => match obj
            .get(ThemeSlot::WindowBg.json_key())
            .and_then(Value::as_str)
            .and_then(|s| parse_hex(s).ok())
        {
            Some(c) if c.l >= 0.5 => ThemeMode::Light,
            _ => ThemeMode::Dark,
        },
    }
}

/// The `name` of a raw theme object, or "Custom theme N" when it has none. `unnamed` counts the
/// names given so far. Logs nothing, since the object may be clipboard text.
pub(crate) fn theme_name(
    obj: &BTreeMap<String, Value>,
    index: usize,
    unnamed: &mut usize,
    repairs: &mut Vec<Repair>,
) -> Arc<str> {
    match obj.get("name").and_then(Value::as_str) {
        Some(n) if !n.trim().is_empty() => n.into(),
        _ => {
            *unnamed += 1;
            let given: Arc<str> = format!("Custom theme {unnamed}").into();
            repairs.push(Repair::Unnamed {
                index,
                given: given.clone(),
            });
            given
        }
    }
}

/// `name` with the first free " (N)" suffix, N from 2.
fn free_name(name: &str, taken: impl Fn(&Arc<str>) -> bool) -> Arc<str> {
    (2..)
        .map(|n| Arc::<str>::from(format!("{name} ({n})")))
        .find(|n| !taken(n))
        .expect("a free suffix exists")
}

/// Parses one raw theme object leniently (UI-FND-042 step 3): a missing or bad slot or `accent`
/// takes the value of the built-in theme of the same mode. The def is named `name`. `None` only
/// when there is no built-in theme to repair from. Logs nothing, since the object may be
/// clipboard text; the file loader logs the repairs.
pub(crate) fn repair_theme(
    mut obj: BTreeMap<String, Value>,
    name: Arc<str>,
    builtins: &[ThemeDef],
    repairs: &mut Vec<Repair>,
) -> Option<ThemeDef> {
    obj.insert("name".into(), Value::String(name.to_string()));

    let fallback = fallback_def(builtins, raw_mode(&obj))?;

    // Replace each missing or bad colour with a placeholder so the strict parse passes,
    // then copy the built-in colour in exactly.
    let mut repaired: Vec<ThemeSlot> = Vec::new();
    for slot in ThemeSlot::ALL {
        let key = slot.json_key();
        let repair = match obj.get(key) {
            None => Repair::MissingSlot {
                theme: name.clone(),
                key,
            },
            Some(v) if v.as_str().is_some_and(|s| parse_hex(s).is_ok()) => continue,
            Some(_) => Repair::BadColor {
                theme: name.clone(),
                key: key.to_string(),
            },
        };
        repairs.push(repair);
        obj.insert(key.to_string(), Value::String("#000000".into()));
        repaired.push(slot);
    }

    let mut def = parse_theme_object(&obj, false).ok()?;
    for slot in repaired {
        match slot {
            ThemeSlot::Accent => def.accent = fallback.accent,
            s => def.slots[s as usize] = fallback.slots[s as usize],
        }
    }
    Some(def)
}

/// Parses the themes file text. Never fails: a file that does not parse sets `corrupt`, and a
/// broken theme is repaired from the built-in of the same mode (UI-FND-042).
pub fn load_user_themes(text: Option<&str>, builtins: &[ThemeDef], now_unix: u64) -> LoadOutcome {
    let mut out = LoadOutcome::default();
    let Some(text) = text else {
        return out;
    };
    let root: Option<Value> = serde_json::from_str(text).ok();
    let Some(entries) = root
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|o| o.get("themes"))
        .and_then(Value::as_array)
    else {
        warn!(target: "tasklens::ui", "themes.json does not parse, reset");
        out.corrupt = Some(now_unix);
        return out;
    };

    if entries.len() > MAX_USER_THEMES {
        warn!(target: "tasklens::ui", total = entries.len(), kept = MAX_USER_THEMES, "too many user themes");
        out.repairs.push(Repair::Truncated {
            total: entries.len(),
        });
    }

    let mut unnamed = 0usize;
    let mut seen: BTreeSet<Arc<str>> = BTreeSet::new();
    for (index, entry) in entries.iter().take(MAX_USER_THEMES).enumerate() {
        let Some(map) = entry.as_object() else {
            warn!(target: "tasklens::ui", index, "user theme is not an object, skipped");
            continue;
        };
        let obj: BTreeMap<String, Value> =
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        let first_repair = out.repairs.len();
        let mut name = theme_name(&obj, index, &mut unnamed, &mut out.repairs);
        if seen.contains(&name) {
            let to = free_name(&name, |n| seen.contains(n));
            warn!(target: "tasklens::ui", from = %name, to = %to, "duplicate user theme name");
            out.repairs.push(Repair::Renamed {
                from: name,
                to: to.clone(),
            });
            name = to;
        }
        seen.insert(name.clone());
        let def = repair_theme(obj, name.clone(), builtins, &mut out.repairs);
        for r in &out.repairs[first_repair..] {
            match r {
                Repair::Unnamed { given, .. } => {
                    warn!(target: "tasklens::ui", index, theme = %given, "user theme has no name");
                }
                Repair::MissingSlot { key, .. } => {
                    warn!(target: "tasklens::ui", theme = %name, slot = key, "user theme colour repaired from built-in");
                }
                Repair::BadColor { key, .. } => {
                    warn!(target: "tasklens::ui", theme = %name, slot = %key, "user theme colour repaired from built-in");
                }
                Repair::Renamed { .. } | Repair::Truncated { .. } => {}
            }
        }
        match def {
            Some(def) => out.themes.push(def),
            None => warn!(target: "tasklens::ui", theme = %name, "user theme skipped"),
        }
    }
    out
}

/// `#RRGGBB`, or `#RRGGBBAA` when the colour is not opaque.
fn hex_string(c: Hsla) -> String {
    let c: Rgba = c.into();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, bl, a) = (b(c.r), b(c.g), b(c.b), b(c.a));
    if a == 255 {
        format!("#{r:02X}{g:02X}{bl:02X}")
    } else {
        format!("#{r:02X}{g:02X}{bl:02X}{a:02X}")
    }
}

/// One theme in file key order: `name`, `mode`, the 22 slots, `accent`, `status`, `graph`, then
/// the unknown keys.
struct ThemeOut<'a>(&'a ThemeDef);

impl Serialize for ThemeOut<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("name", &*d.name)?;
        if let Some(mode) = d.mode {
            m.serialize_entry("mode", &mode)?;
        }
        for (slot, &c) in ThemeSlot::ALL.iter().zip(d.slots.iter()) {
            m.serialize_entry(slot.json_key(), &hex_string(c))?;
        }
        m.serialize_entry(ThemeSlot::Accent.json_key(), &hex_string(d.accent))?;
        if let Some(st) = d.status {
            m.serialize_entry(
                "status",
                &Colors(&[
                    ("success", st.success),
                    ("warning", st.warning),
                    ("danger", st.danger),
                    ("info", st.info),
                ]),
            )?;
        }
        if let Some(g) = d.graph {
            m.serialize_entry(
                "graph",
                &Colors(&[
                    ("cpu_fill", g.cpu_fill),
                    ("cpu_line", g.cpu_line),
                    ("kernel_line", g.kernel_line),
                    ("mem_fill", g.mem_fill),
                    ("mem_line", g.mem_line),
                    ("gpu_fill", g.gpu_fill),
                    ("gpu_line", g.gpu_line),
                    ("disk_fill", g.disk_fill),
                    ("disk_line", g.disk_line),
                    ("net_fill", g.net_fill),
                    ("net_line", g.net_line),
                    ("grid", g.grid),
                    ("chart_bg", g.chart_bg),
                ]),
            )?;
        }
        for (k, v) in &d.extra {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// A nested colour object in the given key order.
struct Colors<'a>(&'a [(&'static str, Hsla)]);

impl Serialize for Colors<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, c) in self.0 {
            m.serialize_entry(k, &hex_string(*c))?;
        }
        m.end()
    }
}

#[derive(Serialize)]
struct FileOut<'a> {
    themes: Vec<ThemeOut<'a>>,
}

/// The themes file text: `{"themes":[...]}` pretty-printed with a 2-space indent.
pub fn serialize_user_themes(themes: &[ThemeDef]) -> String {
    let file = FileOut {
        themes: themes.iter().map(ThemeOut).collect(),
    };
    serde_json::to_string_pretty(&file).expect("theme values always serialize")
}

/// `theme.dark` and `theme.light` from `TlSettings`, or the defaults when it is not installed.
fn theme_settings(cx: &App) -> ThemeSettings {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).theme.clone()
    } else {
        ThemeSettings::default()
    }
}

impl ThemeStore {
    /// Replaces the user themes with the loaded ones. Called before the first activate.
    pub fn install_user(&mut self, outcome: LoadOutcome) {
        self.user = outcome.themes;
    }

    /// Copies `name` (user or built-in) to a new user theme "<name> copy", or "<name> copy 2",
    /// "<name> copy 3" and so on, whichever is free first. Saves the user themes and activates
    /// the copy.
    pub fn copy_theme(name: &str, cx: &mut App) -> Result<ThemeEvent, ThemeError> {
        let store = cx.global::<ThemeStore>();
        if store.preview.is_some() {
            return Err(ThemeError::PreviewActive);
        }
        let Some((def, _)) = store.resolve(name) else {
            return Err(ThemeError::UnknownTheme(name.into()));
        };
        let mut copy = def.clone();
        let new_name: Arc<str> = std::iter::once(format!("{name} copy"))
            .chain((2..).map(|n| format!("{name} copy {n}")))
            .find(|n| store.resolve(n).is_none())
            .expect("a free copy name exists")
            .into();
        copy.name = new_name.clone();

        let store = cx.global_mut::<ThemeStore>();
        store.user.push(copy);
        let saved = store.user.clone();
        persist(PersistTheme::Save(saved), cx);
        store::activate(&new_name, ActivateSource::User, cx)?;
        Ok(ThemeEvent::Copied { name: new_name })
    }

    /// Deletes the user theme `name` and saves. When it was active, activates `theme.dark` or
    /// `theme.light` by the deleted theme's mode, or the same-mode default when that name does
    /// not resolve. The confirmation dialog is the shell's (UI-SHL-084).
    pub fn delete_theme(name: &str, cx: &mut App) -> Result<ThemeEvent, ThemeError> {
        let store = cx.global::<ThemeStore>();
        if store.preview.is_some() {
            return Err(ThemeError::PreviewActive);
        }
        let Some(pos) = store.user.iter().position(|d| &*d.name == name) else {
            return Err(if store.builtin.iter().any(|d| &*d.name == name) {
                ThemeError::BuiltinCannotBeDeleted
            } else {
                ThemeError::UnknownTheme(name.into())
            });
        };
        let was_active = !store.active.builtin && &*store.active.name == name;

        let store = cx.global_mut::<ThemeStore>();
        let removed = store.user.remove(pos);
        let saved = store.user.clone();
        persist(PersistTheme::Save(saved), cx);

        if was_active {
            let settings = theme_settings(cx);
            let (wanted, default) = match removed.effective_mode() {
                ThemeMode::Dark => (settings.dark, store::DEFAULT_DARK),
                ThemeMode::Light => (settings.light, store::DEFAULT_LIGHT),
            };
            let next = if cx.global::<ThemeStore>().resolve(&wanted).is_some() {
                wanted.as_str()
            } else {
                default
            };
            store::activate(next, ActivateSource::User, cx)?;
        }
        Ok(ThemeEvent::Deleted {
            name: removed.name.clone(),
        })
    }

    /// Adds imported themes to the end of `user` (APP-038, UI-FND-042). A name already used by a
    /// user theme gets " (2)", " (3)" and so on. Themes past `MAX_USER_THEMES` are dropped and
    /// counted in the log. Saves once and does not activate. No themes added gives
    /// `ImportFailed`.
    pub fn import_themes(defs: Vec<ThemeDef>, cx: &mut App) -> Result<ThemeEvent, ThemeError> {
        let total = defs.len();
        let store = cx.global_mut::<ThemeStore>();
        let room = MAX_USER_THEMES.saturating_sub(store.user.len());
        let mut seen: BTreeSet<Arc<str>> = store.user.iter().map(|d| d.name.clone()).collect();
        let mut count = 0usize;
        for mut def in defs.into_iter().take(room) {
            if seen.contains(&def.name) {
                def.name = free_name(&def.name, |n| seen.contains(n));
            }
            seen.insert(def.name.clone());
            store.user.push(def);
            count += 1;
        }
        if count < total {
            warn!(target: "tasklens::ui", dropped = total - count, kept = count, "user theme cap reached, imported themes dropped");
        }
        if count == 0 {
            return Ok(ThemeEvent::ImportFailed);
        }
        let saved = store.user.clone();
        persist(PersistTheme::Save(saved), cx);
        Ok(ThemeEvent::Imported { count })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{TestAppContext, rgb};
    use serde_json::json;
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::theme::{builtin, current};

    fn builtins() -> Arc<[ThemeDef]> {
        builtin::load()
    }

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    /// A full theme object from a built-in, as JSON, with `name` and edits applied.
    fn theme_json(builtin_name: &str, name: &str) -> serde_json::Map<String, Value> {
        let def = builtins()
            .iter()
            .find(|d| &*d.name == builtin_name)
            .cloned()
            .unwrap();
        let text = serialize_user_themes(&[def]);
        let root: Value = serde_json::from_str(&text).unwrap();
        let mut obj = root["themes"][0].as_object().unwrap().clone();
        obj.insert("name".into(), name.into());
        obj
    }

    fn file(themes: Vec<serde_json::Map<String, Value>>) -> String {
        json!({ "themes": themes }).to_string()
    }

    fn load(text: &str) -> LoadOutcome {
        load_user_themes(Some(text), &builtins(), 1)
    }

    type Recorded = Rc<RefCell<Vec<PersistTheme>>>;

    fn setup(cx: &mut App, text: Option<&str>, now: u64) -> (Recorded, Option<SharedString>) {
        gpui_kit::init(cx);
        let rec: Recorded = Rc::default();
        let r = rec.clone();
        cx.set_global(ThemePersist(Box::new(move |p| r.borrow_mut().push(p))));
        let toast = store::init_with_file(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            text,
            now,
        );
        (rec, toast)
    }

    #[test]
    fn missing_file_empty() {
        let out = load_user_themes(None, &builtins(), 1759500000);
        assert!(out.themes.is_empty());
        assert_eq!(out.corrupt, None);
        assert!(out.repairs.is_empty());
    }

    #[gpui_kit::test]
    fn missing_file_sends_nothing(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (rec, toast) = setup(cx, None, 1759500000);
            assert!(rec.borrow().is_empty());
            assert_eq!(toast, None);
            assert!(cx.global::<ThemeStore>().user.is_empty());
        });
    }

    #[gpui_kit::test]
    fn corrupt_renamed_and_toast(cx: &mut TestAppContext) {
        let out = load_user_themes(Some("{\"themes\":"), &builtins(), 1759500000);
        assert!(out.themes.is_empty());
        assert_eq!(out.corrupt, Some(1759500000));
        assert_eq!(
            &*corrupt_toast_text(1759500000),
            "Themes file was unreadable and has been reset. The old file was kept as \
             themes.json.corrupt-1759500000"
        );
        for bad in ["[]", "{}", "{\"themes\":{}}", "not json"] {
            assert_eq!(load(bad).corrupt, Some(1), "{bad}");
        }

        cx.update(|cx| {
            let (rec, toast) = setup(cx, Some("{\"themes\":"), 1759500000);
            assert_eq!(toast, Some(corrupt_toast_text(1759500000)));
            let rec = rec.borrow();
            assert_eq!(rec.len(), 1);
            assert!(matches!(
                rec[0],
                PersistTheme::RenameCorrupt {
                    unix_seconds: 1759500000
                }
            ));
        });
    }

    #[test]
    fn missing_slot_falls_back() {
        let mut light = theme_json("Default Light", "Paper");
        light.insert("WINDOW_BG".into(), "#FAFAFA".into());
        light.remove("HEADER");
        light.remove("mode");
        let out = load(&file(vec![light]));
        assert_eq!(out.themes.len(), 1);
        let t = &out.themes[0];
        assert_eq!(t.slots[ThemeSlot::Header as usize], hex(0xDDE1E5));
        assert_eq!(t.slots[ThemeSlot::WindowBg as usize], hex(0xFAFAFA));
        assert_eq!(
            out.repairs,
            vec![Repair::MissingSlot {
                theme: "Paper".into(),
                key: "HEADER"
            }]
        );

        // Same slot missing from a dark theme takes Default Dark's value.
        let mut dark = theme_json("Default Dark", "Night");
        dark.remove("HEADER");
        let out = load(&file(vec![dark]));
        assert_eq!(
            out.themes[0].slots[ThemeSlot::Header as usize],
            hex(0x2E3235)
        );
    }

    #[test]
    fn unknown_keys_round_trip() {
        let mut dark = theme_json("Default Dark", "Ocean");
        dark.insert("FOO".into(), "x".into());
        dark.insert("nested".into(), json!({"a": [1, 2, {"b": null}]}));
        dark.insert("accent".into(), "blue".into());
        let out = load(&file(vec![dark]));
        let t = &out.themes[0];
        assert_eq!(t.accent, hex(0x0277A6));
        assert_eq!(
            out.repairs,
            vec![Repair::BadColor {
                theme: "Ocean".into(),
                key: "accent".into()
            }]
        );
        assert_eq!(t.extra["FOO"], "x");

        let text = serialize_user_themes(&out.themes);
        assert!(text.starts_with("{\n  \"themes\": [\n    {\n      \"name\": \"Ocean\""));
        let root: Value = serde_json::from_str(&text).unwrap();
        let obj = &root["themes"][0];
        assert_eq!(obj["FOO"], "x");
        assert_eq!(obj["nested"], json!({"a": [1, 2, {"b": null}]}));
        assert_eq!(obj["accent"], "#0277A6");
        assert_eq!(obj["HEADER"], "#2E3235");
        assert_eq!(obj["mode"], "dark");

        // A second load of the saved text gives the same theme and no repairs.
        let again = load(&text);
        assert!(again.repairs.is_empty());
        assert_eq!(serialize_user_themes(&again.themes), text);
    }

    #[test]
    fn translucent_colour_round_trips() {
        let mut dark = theme_json("Default Dark", "Glass");
        dark.insert("ROW_HOVER".into(), "#23262980".into());
        let out = load(&file(vec![dark]));
        let text = serialize_user_themes(&out.themes);
        let root: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["themes"][0]["ROW_HOVER"], "#23262980");
    }

    #[test]
    fn duplicate_and_unnamed_names() {
        let mut a = theme_json("Default Dark", "Mine");
        let b = theme_json("Default Dark", "Mine");
        let c = theme_json("Default Dark", "Mine");
        let mut d = theme_json("Default Light", "x");
        d.remove("name");
        let mut e = theme_json("Default Light", "");
        e.insert("name".into(), json!(7));
        a.insert("extra_key".into(), json!(true));
        let out = load(&file(vec![a, b, c, d, e]));
        let names: Vec<&str> = out.themes.iter().map(|t| &*t.name).collect();
        assert_eq!(
            names,
            [
                "Mine",
                "Mine (2)",
                "Mine (3)",
                "Custom theme 1",
                "Custom theme 2"
            ]
        );
        assert_eq!(
            out.repairs,
            vec![
                Repair::Renamed {
                    from: "Mine".into(),
                    to: "Mine (2)".into()
                },
                Repair::Renamed {
                    from: "Mine".into(),
                    to: "Mine (3)".into()
                },
                Repair::Unnamed {
                    index: 3,
                    given: "Custom theme 1".into()
                },
                Repair::Unnamed {
                    index: 4,
                    given: "Custom theme 2".into()
                },
            ]
        );
    }

    #[test]
    fn cap_at_64() {
        let themes: Vec<_> = (0..70)
            .map(|i| theme_json("Default Dark", &format!("T{i}")))
            .collect();
        let out = load(&file(themes));
        assert_eq!(out.themes.len(), 64);
        assert_eq!(&*out.themes[63].name, "T63");
        assert_eq!(out.repairs, vec![Repair::Truncated { total: 70 }]);
    }

    #[test]
    fn load_64_themes_is_fast() {
        let themes: Vec<_> = (0..64)
            .map(|i| theme_json("Default Dark", &format!("T{i}")))
            .collect();
        let text = file(themes);
        let b = builtins();
        let start = std::time::Instant::now();
        let out = load_user_themes(Some(&text), &b, 1);
        let took = start.elapsed();
        assert_eq!(out.themes.len(), 64);
        // UI-FND-042 budget is 1 ms in release; the old test limit was 20 ms in debug. The limit
        // is 2 s so a busy machine does not fail it (tasklens-cr0.32); no bench scene covers it.
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[gpui_kit::test]
    fn copy_names_increment(cx: &mut TestAppContext) {
        let text = file(vec![
            theme_json("Default Dark", "Ocean"),
            theme_json("Default Dark", "Ocean copy"),
        ]);
        cx.update(|cx| {
            let (rec, _) = setup(cx, Some(&text), 1);
            let r = ThemeStore::copy_theme("Ocean", cx);
            assert_eq!(
                r,
                Ok(ThemeEvent::Copied {
                    name: "Ocean copy 2".into()
                })
            );
            assert_eq!(&*current(cx).name, "Ocean copy 2");
            assert!(!current(cx).builtin);
            {
                let rec = rec.borrow();
                assert_eq!(rec.len(), 1);
                let PersistTheme::Save(saved) = &rec[0] else {
                    panic!("expected Save, got {:?}", rec[0]);
                };
                let names: Vec<&str> = saved.iter().map(|t| &*t.name).collect();
                assert_eq!(names, ["Ocean", "Ocean copy", "Ocean copy 2"]);
            }

            // A built-in copies too.
            let r = ThemeStore::copy_theme("Default Light", cx);
            assert_eq!(
                r,
                Ok(ThemeEvent::Copied {
                    name: "Default Light copy".into()
                })
            );
            assert_eq!(current(cx).mode, ThemeMode::Light);
            assert_eq!(rec.borrow().len(), 2);

            assert_eq!(
                ThemeStore::copy_theme("Missing", cx),
                Err(ThemeError::UnknownTheme("Missing".into()))
            );
            assert_eq!(rec.borrow().len(), 2);
        });
    }

    #[gpui_kit::test]
    fn import_renames_duplicates_and_saves_once(cx: &mut TestAppContext) {
        let text = file(vec![theme_json("Default Dark", "Mine")]);
        cx.update(|cx| {
            let (rec, _) = setup(cx, Some(&text), 1);
            let active = current(cx).name.clone();
            let mine = cx.global::<ThemeStore>().user[0].clone();
            let mut other = mine.clone();
            other.slots[ThemeSlot::WindowBg as usize] = hex(0x102030);

            let r = ThemeStore::import_themes(vec![other, mine.clone()], cx);
            assert_eq!(r, Ok(ThemeEvent::Imported { count: 2 }));
            let names: Vec<&str> = cx
                .global::<ThemeStore>()
                .user
                .iter()
                .map(|t| &*t.name)
                .collect();
            assert_eq!(names, ["Mine", "Mine (2)", "Mine (3)"]);
            assert_eq!(
                cx.global::<ThemeStore>().user[1].slots[ThemeSlot::WindowBg as usize],
                hex(0x102030)
            );
            assert_eq!(current(cx).name, active);
            {
                let rec = rec.borrow();
                assert_eq!(rec.len(), 1);
                let PersistTheme::Save(saved) = &rec[0] else {
                    panic!("expected Save, got {:?}", rec[0]);
                };
                assert_eq!(saved.len(), 3);
            }

            assert_eq!(
                ThemeStore::import_themes(Vec::new(), cx),
                Ok(ThemeEvent::ImportFailed)
            );
            assert_eq!(rec.borrow().len(), 1);

            // The cap keeps 64 user themes and drops the rest.
            let many: Vec<ThemeDef> = (0..70)
                .map(|i| {
                    let mut d = mine.clone();
                    d.name = format!("T{i}").into();
                    d
                })
                .collect();
            assert_eq!(
                ThemeStore::import_themes(many.clone(), cx),
                Ok(ThemeEvent::Imported { count: 61 })
            );
            assert_eq!(cx.global::<ThemeStore>().user.len(), MAX_USER_THEMES);
            assert_eq!(
                ThemeStore::import_themes(many, cx),
                Ok(ThemeEvent::ImportFailed)
            );
            assert_eq!(rec.borrow().len(), 2);
        });
    }

    #[gpui_kit::test]
    fn delete_builtin_refused(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (rec, _) = setup(cx, None, 1);
            assert_eq!(
                ThemeStore::delete_theme("Default Dark", cx),
                Err(ThemeError::BuiltinCannotBeDeleted)
            );
            assert!(rec.borrow().is_empty());
        });
    }
}
