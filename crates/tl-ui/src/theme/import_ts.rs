//! The themes half of "Import from TaskSlinger…" (P4, APP-034, APP-035, UI-FND-042): parses a
//! TaskSlinger `themes.json` and adds its custom themes to the user themes.
//!
//! TaskSlinger's file is `{"themes":[{"name":…,"SLOT":"#RRGGBB",…}]}` with the 22 slots and no
//! `accent`, which is tasklens's base format, so each theme goes through the user file's lenient
//! repair (E17-008). The caller reads the file (tl-ui does no file I/O, ADR-011). The text is
//! untrusted: it is capped at `MAX_IMPORT_BYTES`, the kept themes at `MAX_USER_THEMES`, nothing
//! panics, and nothing logs the text itself.

use std::collections::BTreeMap;

use gpui_kit::App;
use serde_json::Value;

use super::ThemeDef;
use super::builtin::TS_THEME_ORDER;
use super::share_code::{MAX_IMPORT_BYTES, ShareCodeError};
use super::store::{ThemeEvent, ThemeStore};
use super::user_store::{MAX_USER_THEMES, repair_theme, theme_name};

/// The themes found in a TaskSlinger `themes.json`.
#[derive(Clone, Debug)]
pub struct TsImport {
    /// The themes to add, in file order.
    pub themes: Vec<ThemeDef>,
    /// Unchanged built-in themes in the file, which are not added.
    pub skipped_builtin_names: usize,
}

/// `true` when `def` carries one of TaskSlinger's 12 built-in names and its 22 slots equal the
/// shipped built-in of that name exactly. A user copy with a built-in name is not skipped.
fn is_unchanged_builtin(def: &ThemeDef, builtins: &[ThemeDef]) -> bool {
    TS_THEME_ORDER.contains(&&*def.name)
        && builtins
            .iter()
            .any(|b| b.name == def.name && b.slots == def.slots)
}

/// Parses a TaskSlinger `themes.json`. A missing slot or `accent` takes the value of the built-in
/// theme of the same mode and unknown keys are kept in `extra`. Unchanged built-ins are skipped
/// and counted. Entries that are not objects are skipped. At most `MAX_USER_THEMES` themes are
/// kept. Text longer than `MAX_IMPORT_BYTES`, text that is not JSON and JSON without a `themes`
/// array give `BadJson`.
pub fn parse_taskslinger_themes(
    text: &str,
    builtins: &[ThemeDef],
) -> Result<TsImport, ShareCodeError> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(ShareCodeError::BadJson);
    }
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(text) else {
        return Err(ShareCodeError::BadJson);
    };
    let Some(Value::Array(entries)) = root.remove("themes") else {
        return Err(ShareCodeError::BadJson);
    };

    let mut out = TsImport {
        themes: Vec::new(),
        skipped_builtin_names: 0,
    };
    let mut repairs = Vec::new();
    let mut unnamed = 0usize;
    for (index, entry) in entries.into_iter().enumerate() {
        if out.themes.len() == MAX_USER_THEMES {
            break;
        }
        let Value::Object(map) = entry else {
            continue;
        };
        let obj: BTreeMap<String, Value> = map.into_iter().collect();
        let name = theme_name(&obj, index, &mut unnamed, &mut repairs);
        let Some(def) = repair_theme(obj, name, builtins, &mut repairs) else {
            continue;
        };
        if is_unchanged_builtin(&def, builtins) {
            out.skipped_builtin_names += 1;
        } else {
            out.themes.push(def);
        }
    }
    Ok(out)
}

impl ThemeStore {
    /// Parses a TaskSlinger `themes.json` against the store's built-ins and adds the themes with
    /// `import_themes`, which renames duplicates, applies the user theme cap and saves once.
    /// Returns the number of themes added, for the "Imported N settings and M themes" toast.
    /// Nothing is saved when the text is refused or no theme is added.
    pub fn import_taskslinger(text: &str, cx: &mut App) -> Result<usize, ShareCodeError> {
        let parsed = parse_taskslinger_themes(text, &cx.global::<ThemeStore>().builtin)?;
        match ThemeStore::import_themes(parsed.themes, cx) {
            Ok(ThemeEvent::Imported { count }) => Ok(count),
            // `ImportFailed` means no theme was added: none in the file, or the store is full.
            _ => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{Hsla, TestAppContext, rgb};
    use proptest::prelude::*;
    use serde_json::json;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::theme::user_store::{PersistTheme, ThemePersist};
    use crate::theme::{ThemeSlot, builtin, store};

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    /// `#RRGGBB` of an opaque colour.
    fn hex_of(c: Hsla) -> String {
        let c: gpui_kit::Rgba = c.into();
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", b(c.r), b(c.g), b(c.b))
    }

    /// The shipped built-ins plus "Dracula", a dark theme with its own window background, so the
    /// skip rule can be tested before the 12 built-ins ship (E17-005).
    fn builtins_with_dracula() -> Arc<[ThemeDef]> {
        let mut list: Vec<ThemeDef> = builtin::load().to_vec();
        let mut dracula = list
            .iter()
            .find(|d| &*d.name == "Default Dark")
            .cloned()
            .unwrap();
        dracula.name = "Dracula".into();
        dracula.slots[ThemeSlot::WindowBg as usize] = hex(0x282A36);
        list.push(dracula);
        list.into()
    }

    /// A theme object as TaskSlinger writes it: `name` and the 22 slots, no `accent` or `mode`.
    fn ts_theme(def: &ThemeDef, name: &str) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), name.into());
        for (slot, &c) in ThemeSlot::ALL.iter().zip(def.slots.iter()) {
            obj.insert(slot.json_key().into(), hex_of(c).into());
        }
        Value::Object(obj)
    }

    fn find<'a>(list: &'a [ThemeDef], name: &str) -> &'a ThemeDef {
        list.iter().find(|d| &*d.name == name).unwrap()
    }

    /// 3 custom themes and the unchanged Dracula built-in.
    fn ts_file(builtins: &[ThemeDef]) -> String {
        let dark = find(builtins, "Default Dark");
        let light = find(builtins, "Default Light");
        let dracula = find(builtins, "Dracula");
        let mut edited = dark.clone();
        edited.slots[ThemeSlot::Header as usize] = hex(0x123456);
        json!({ "themes": [
            ts_theme(&edited, "Ocean"),
            ts_theme(dracula, "Dracula"),
            ts_theme(light, "Paper"),
            // A user copy that kept a built-in name but changed a slot is kept.
            ts_theme(&edited, "Dracula"),
        ]})
        .to_string()
    }

    type Recorded = Rc<RefCell<Vec<PersistTheme>>>;

    /// Installs the theme store with a recording persist callback, then swaps in `builtins`. No
    /// file is read or written.
    fn setup(cx: &mut App, builtins: Arc<[ThemeDef]>) -> Recorded {
        gpui_kit::init(cx);
        let rec: Recorded = Rc::default();
        let r = rec.clone();
        cx.set_global(ThemePersist(Box::new(move |p| r.borrow_mut().push(p))));
        store::init_with_file(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            None,
            1,
        );
        cx.global_mut::<ThemeStore>().builtin = builtins;
        rec
    }

    #[gpui_kit::test]
    fn imports_custom_skips_unchanged_builtins(cx: &mut TestAppContext) {
        let builtins = builtins_with_dracula();
        let text = ts_file(&builtins);

        let parsed = parse_taskslinger_themes(&text, &builtins).unwrap();
        assert_eq!(parsed.skipped_builtin_names, 1);
        let names: Vec<&str> = parsed.themes.iter().map(|d| &*d.name).collect();
        assert_eq!(names, ["Ocean", "Paper", "Dracula"]);
        assert_eq!(
            parsed.themes[0].slots[ThemeSlot::Header as usize],
            hex(0x123456)
        );

        cx.update(|cx| {
            let rec = setup(cx, builtins.clone());
            assert_eq!(ThemeStore::import_taskslinger(&text, cx), Ok(3));
            let user: Vec<&str> = cx
                .global::<ThemeStore>()
                .user
                .iter()
                .map(|d| &*d.name)
                .collect();
            assert_eq!(user, ["Ocean", "Paper", "Dracula"]);
            let rec = rec.borrow();
            assert_eq!(rec.len(), 1);
            let PersistTheme::Save(saved) = &rec[0] else {
                panic!("expected Save, got {:?}", rec[0]);
            };
            assert_eq!(saved.len(), 3);
        });

        // A file of only unchanged built-ins adds nothing and saves nothing.
        cx.update(|cx| {
            let rec = setup(cx, builtins.clone());
            let only = json!({ "themes": [
                ts_theme(find(&builtins, "Default Dark"), "Default Dark"),
                ts_theme(find(&builtins, "Dracula"), "Dracula"),
            ]})
            .to_string();
            assert_eq!(ThemeStore::import_taskslinger(&only, cx), Ok(0));
            assert!(cx.global::<ThemeStore>().user.is_empty());
            assert!(rec.borrow().is_empty());
        });
    }

    #[test]
    fn missing_accent_falls_back() {
        let builtins = builtins_with_dracula();
        let dark = find(&builtins, "Default Dark");
        let light = find(&builtins, "Default Light");
        let mut night = dark.clone();
        night.slots[ThemeSlot::Header as usize] = hex(0x223344);
        let mut day = light.clone();
        day.slots[ThemeSlot::Header as usize] = hex(0xCCDDEE);
        let mut odd = ts_theme(&night, "Odd");
        odd["FOO"] = json!({"a": [1, null]});
        let text = json!({ "themes": [ts_theme(&night, "Night"), ts_theme(&day, "Day"), odd] })
            .to_string();

        let parsed = parse_taskslinger_themes(&text, &builtins).unwrap();
        assert_eq!(parsed.themes.len(), 3);
        assert_eq!(parsed.themes[0].accent, dark.accent);
        assert_eq!(parsed.themes[1].accent, light.accent);
        assert_eq!(parsed.themes[0].mode, None);
        assert_eq!(parsed.themes[2].extra["FOO"], json!({"a": [1, null]}));
    }

    #[gpui_kit::test]
    fn bad_json_and_size_cap(cx: &mut TestAppContext) {
        let builtins = builtins_with_dracula();
        for bad in [
            "",
            "not json",
            "{\"themes\":",
            "[]",
            "{}",
            "{\"themes\":{}}",
            "\"themes\"",
        ] {
            assert_eq!(
                parse_taskslinger_themes(bad, &builtins).err(),
                Some(ShareCodeError::BadJson),
                "{bad}"
            );
        }

        // 2 MiB of otherwise valid JSON is refused before parsing.
        let pad = "x".repeat(2 << 20);
        let big = json!({ "themes": [], "pad": pad }).to_string();
        assert!(big.len() > MAX_IMPORT_BYTES);
        assert_eq!(
            parse_taskslinger_themes(&big, &builtins).err(),
            Some(ShareCodeError::BadJson)
        );

        // Entries that are not objects are skipped; unnamed themes get a name.
        let mixed = json!({ "themes": [1, "x", null, {"WINDOW_BG": "#FAFAFA"}] }).to_string();
        let parsed = parse_taskslinger_themes(&mixed, &builtins).unwrap();
        assert_eq!(parsed.themes.len(), 1);
        assert_eq!(&*parsed.themes[0].name, "Custom theme 1");

        // More than 64 custom themes keeps the first 64.
        let dark = find(&builtins, "Default Dark");
        let many: Vec<Value> = (0..70).map(|i| ts_theme(dark, &format!("T{i}"))).collect();
        let parsed =
            parse_taskslinger_themes(&json!({ "themes": many }).to_string(), &builtins).unwrap();
        assert_eq!(parsed.themes.len(), MAX_USER_THEMES);
        assert_eq!(&*parsed.themes[63].name, "T63");

        cx.update(|cx| {
            let rec = setup(cx, builtins.clone());
            assert_eq!(
                ThemeStore::import_taskslinger("not json", cx),
                Err(ShareCodeError::BadJson)
            );
            assert_eq!(
                ThemeStore::import_taskslinger(&big, cx),
                Err(ShareCodeError::BadJson)
            );
            assert!(cx.global::<ThemeStore>().user.is_empty());
            assert!(rec.borrow().is_empty());
        });
    }

    /// A JSON value of bounded depth with theme-like keys and colour-like strings.
    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            "#[0-9A-Fa-f]{0,9}".prop_map(Value::String),
            ".{0,12}".prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 32, 6, |inner| {
            let key = prop_oneof![
                Just("name".to_string()),
                Just("mode".to_string()),
                Just("accent".to_string()),
                Just("WINDOW_BG".to_string()),
                Just("HEADER".to_string()),
                Just("status".to_string()),
                Just("graph".to_string()),
                Just("themes".to_string()),
                "[a-z]{1,6}",
            ];
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                prop::collection::btree_map(key, inner, 0..6)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #[test]
        fn arbitrary_text_never_panics(text in ".{0,256}") {
            let builtins = builtin::load();
            let _ = parse_taskslinger_themes(&text, &builtins);
        }

        #[test]
        fn arbitrary_themes_never_panic(entries in prop::collection::vec(json_value(), 0..80)) {
            let builtins = builtins_with_dracula();
            let text = json!({ "themes": entries }).to_string();
            if let Ok(parsed) = parse_taskslinger_themes(&text, &builtins) {
                prop_assert!(parsed.themes.len() <= MAX_USER_THEMES);
                prop_assert!(parsed.themes.len() + parsed.skipped_builtin_names <= entries.len());
                for def in &parsed.themes {
                    prop_assert!(!def.name.trim().is_empty());
                }
            }
        }
    }
}
