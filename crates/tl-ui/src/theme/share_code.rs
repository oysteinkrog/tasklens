//! Theme share codes and clipboard import (APP-038, UI-FND-042, R-079).
//!
//! A `TL1.` code is `"TL1."` plus base64url without padding of the JSON object
//! `{"v":1,"name":…,"accent":"#RRGGBB","WINDOW_BG":"#RRGGBB",…}` with the 22 slots in
//! `ThemeSlot` order. Import also takes raw `themes.json` text and a single theme object.
//!
//! Import text comes from the clipboard, so it is untrusted: the text is capped at
//! `MAX_IMPORT_BYTES`, the theme count at `MAX_USER_THEMES`, nothing panics, and nothing logs the
//! text itself.

use std::collections::BTreeMap;

use gpui_kit::{Hsla, Rgba};
use serde::Serialize;
use serde::ser::SerializeMap;
use serde_json::Value;

use super::user_store::{MAX_USER_THEMES, repair_theme, theme_name};
use super::{ThemeDef, ThemeSlot, builtin};

/// The prefix of a tasklens theme code.
pub const TL1_PREFIX: &str = "TL1.";

/// The prefix of a TaskSlinger theme code (E17-010).
const TS1_PREFIX: &str = "TS1.";

/// The longest import text accepted, in bytes. Longer text is not a theme code.
pub const MAX_IMPORT_BYTES: usize = 1 << 20;

/// Why import text is not a theme. The shell shows "Clipboard does not contain a theme code" for
/// every variant (APP-038).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareCodeError {
    /// No `TL1.` prefix, not theme JSON, or longer than `MAX_IMPORT_BYTES`.
    NotAThemeCode,
    /// The text after `TL1.` is not base64url.
    BadBase64,
    /// The decoded bytes are not a UTF-8 JSON theme object.
    BadJson,
    /// The code's `v` is not 1.
    UnsupportedVersion(u64),
}

/// The code payload in key order: `v`, `name`, `accent`, then the 22 slots.
struct Tl1Out<'a>(&'a ThemeDef);

impl Serialize for Tl1Out<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0;
        let mut m = s.serialize_map(Some(25))?;
        m.serialize_entry("v", &1u64)?;
        m.serialize_entry("name", &*d.name)?;
        m.serialize_entry(ThemeSlot::Accent.json_key(), &rgb_hex(d.accent))?;
        for (slot, &c) in ThemeSlot::ALL.iter().zip(d.slots.iter()) {
            m.serialize_entry(slot.json_key(), &rgb_hex(c))?;
        }
        m.end()
    }
}

/// Upper-case `#RRGGBB`. The alpha is dropped.
fn rgb_hex(c: Hsla) -> String {
    let c: Rgba = c.into();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c.r), b(c.g), b(c.b))
}

/// The `TL1.` code of `def`. `mode`, `status`, `graph` and `extra` are not included. Share passes
/// the preview def while the theme editor is open (UI-FND-043 rule 8).
pub fn encode_tl1(def: &ThemeDef) -> String {
    let json = serde_json::to_vec(&Tl1Out(def)).expect("theme values always serialize");
    let mut code = String::with_capacity(TL1_PREFIX.len() + json.len().div_ceil(3) * 4);
    code.push_str(TL1_PREFIX);
    base64url_encode(&json, &mut code);
    code
}

/// Decodes a `TL1.` code. Surrounding whitespace is ignored and base64 padding is optional.
/// Missing or bad colours take the value of the built-in theme of the same mode, as in the user
/// themes file (E17-008).
pub fn decode_tl1(code: &str) -> Result<ThemeDef, ShareCodeError> {
    let code = code.trim();
    if code.len() > MAX_IMPORT_BYTES {
        return Err(ShareCodeError::NotAThemeCode);
    }
    let payload = code
        .strip_prefix(TL1_PREFIX)
        .ok_or(ShareCodeError::NotAThemeCode)?;
    let bytes = base64url_decode(payload).ok_or(ShareCodeError::BadBase64)?;
    let mut obj: BTreeMap<String, Value> =
        serde_json::from_slice(&bytes).map_err(|_| ShareCodeError::BadJson)?;
    match obj.remove("v").as_ref().and_then(Value::as_u64) {
        Some(1) => {}
        Some(v) => return Err(ShareCodeError::UnsupportedVersion(v)),
        None => return Err(ShareCodeError::BadJson),
    }
    let mut themes = lenient_defs(vec![obj]);
    themes.pop().ok_or(ShareCodeError::BadJson)
}

/// The themes in clipboard text: a `TL1.` code, a `TS1.` code (E17-010), `themes.json` text
/// (`{"themes":[…]}`, at most `MAX_USER_THEMES` entries) or a single theme object (one with
/// `WINDOW_BG`). Entries of the `themes` array that are not objects are skipped.
pub fn import_text(text: &str) -> Result<Vec<ThemeDef>, ShareCodeError> {
    let text = text.trim();
    if text.len() > MAX_IMPORT_BYTES {
        return Err(ShareCodeError::NotAThemeCode);
    }
    if text.starts_with(TL1_PREFIX) {
        return decode_tl1(text).map(|def| vec![def]);
    }
    if text.starts_with(TS1_PREFIX) {
        // TaskSlinger codes wait for U-17 (E17-010).
        return Err(ShareCodeError::NotAThemeCode);
    }
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(text) else {
        return Err(ShareCodeError::NotAThemeCode);
    };
    let objects: Vec<BTreeMap<String, Value>> = match root.remove("themes") {
        Some(Value::Array(entries)) => entries
            .into_iter()
            .filter_map(|e| match e {
                Value::Object(m) => Some(m.into_iter().collect()),
                _ => None,
            })
            .take(MAX_USER_THEMES)
            .collect(),
        Some(_) => return Err(ShareCodeError::NotAThemeCode),
        None if root.contains_key(ThemeSlot::WindowBg.json_key()) => {
            vec![root.into_iter().collect()]
        }
        None => return Err(ShareCodeError::NotAThemeCode),
    };
    Ok(lenient_defs(objects))
}

/// Parses raw theme objects with the user file's lenient repair. Unnamed themes get
/// "Custom theme N"; `ThemeStore::import_themes` renames duplicates.
fn lenient_defs(objects: Vec<BTreeMap<String, Value>>) -> Vec<ThemeDef> {
    let builtins = builtin::load();
    let mut repairs = Vec::new();
    let mut unnamed = 0usize;
    objects
        .into_iter()
        .enumerate()
        .filter_map(|(index, obj)| {
            let name = theme_name(&obj, index, &mut unnamed, &mut repairs);
            repair_theme(obj, name, &builtins, &mut repairs)
        })
        .collect()
}

/// The RFC 4648 section 5 alphabet.
const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Appends base64url of `data` to `out`, without padding.
fn base64url_encode(data: &[u8], out: &mut String) {
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(B64URL[(n >> (18 - 6 * i) & 0x3F) as usize]));
        }
    }
}

/// The 6-bit value of a base64url character.
fn b64url_value(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

/// Decodes base64url with or without padding. `None` for a character outside the alphabet,
/// misplaced padding or a length that cannot be base64.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let data = match bytes {
        [rest @ .., b'=', b'='] | [rest @ .., b'='] => {
            // Padding fills the last group to 4 characters, no more.
            if !bytes.len().is_multiple_of(4) {
                return None;
            }
            rest
        }
        _ => bytes,
    };
    if data.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= b64url_value(c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{App, TestAppContext, rgb};
    use proptest::prelude::*;
    use serde_json::json;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::theme::store::{self, DEFAULT_DARK, DEFAULT_LIGHT, ThemeStore};
    use crate::theme::user_store::{PersistTheme, ThemePersist, serialize_user_themes};

    fn hex(v: u32) -> Hsla {
        rgb(v).into()
    }

    fn builtin_def(name: &str) -> ThemeDef {
        builtin::load()
            .iter()
            .find(|d| &*d.name == name)
            .cloned()
            .unwrap()
    }

    fn encode_b64(data: &[u8]) -> String {
        let mut s = String::new();
        base64url_encode(data, &mut s);
        s
    }

    /// The JSON object inside a code, as text.
    fn payload(code: &str) -> String {
        let bytes = base64url_decode(code.strip_prefix(TL1_PREFIX).unwrap()).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn base64url_rfc4648_vectors() {
        // RFC 4648 section 10, without padding. These use no '+' or '/' characters.
        let vectors = [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, coded) in vectors {
            assert_eq!(encode_b64(plain.as_bytes()), coded, "{plain}");
            assert_eq!(
                base64url_decode(coded).as_deref(),
                Some(plain.as_bytes()),
                "{coded}"
            );
        }
        // Padding is optional on decode.
        for padded in ["Zg==", "Zm8=", "Zm9vYg==", "Zm9vYmE="] {
            let bare = padded.trim_end_matches('=');
            assert_eq!(base64url_decode(padded), base64url_decode(bare), "{padded}");
        }
        // The URL alphabet: 62 is '-', 63 is '_'.
        assert_eq!(encode_b64(&[0xFB, 0xFF]), "-_8");
        assert_eq!(base64url_decode("-_8"), Some(vec![0xFB, 0xFF]));
        for bad in [
            "Z", "Zg=", "Zg===", "Z===", "=", "Zm+v", "Zm/v", "Zm9v\n", "Zm=v",
        ] {
            assert_eq!(base64url_decode(bad), None, "{bad:?}");
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64url_decode(&encode_b64(&all)), Some(all));
    }

    #[test]
    fn tl1_round_trip() {
        for name in [DEFAULT_DARK, DEFAULT_LIGHT] {
            let def = builtin_def(name);
            let back = decode_tl1(&encode_tl1(&def)).unwrap();
            assert_eq!(back.name, def.name);
            assert_eq!(back.accent, def.accent);
            assert_eq!(back.slots, def.slots);
            assert!(back.extra.is_empty());
            assert_eq!(back.effective_mode(), def.effective_mode());
        }

        // Names with quotes and non-ASCII text survive.
        let mut def = builtin_def(DEFAULT_DARK);
        def.name = "Mørk \"nat\" ✓".into();
        let code = format!("  \n{}\t ", encode_tl1(&def));
        assert_eq!(decode_tl1(&code).unwrap().name, def.name);
    }

    #[test]
    fn tl1_alphabet_and_key_order() {
        let mut def = builtin_def(DEFAULT_DARK);
        def.slots[ThemeSlot::RowHover as usize] = gpui_kit::rgba(0x23262980).into();
        let code = encode_tl1(&def);
        assert!(code.starts_with("TL1."));
        assert!(
            code[4..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );

        let text = payload(&code);
        let mut expected = String::from("{\"v\":1,\"name\":\"Default Dark\",\"accent\":\"");
        expected.push_str(&rgb_hex(def.accent));
        expected.push('"');
        for (slot, &c) in ThemeSlot::ALL[..22].iter().zip(def.slots.iter()) {
            expected.push_str(&format!(",\"{}\":\"{}\"", slot.json_key(), rgb_hex(c)));
        }
        expected.push('}');
        assert_eq!(text, expected);
        // Colours are upper-case #RRGGBB; the alpha is dropped.
        assert!(text.contains("\"ROW_HOVER\":\"#232629\""));
        assert!(!text.contains("\"mode\"") && !text.contains("\"status\""));
    }

    #[test]
    fn bad_codes_rejected() {
        assert_eq!(decode_tl1("TL1.!!!").err(), Some(ShareCodeError::BadBase64));
        assert_eq!(decode_tl1("TL1.Z").err(), Some(ShareCodeError::BadBase64));
        assert_eq!(
            decode_tl1("hello").err(),
            Some(ShareCodeError::NotAThemeCode)
        );
        assert_eq!(
            decode_tl1("tl1.Zm9v").err(),
            Some(ShareCodeError::NotAThemeCode)
        );
        let code = |v: Value| format!("TL1.{}", encode_b64(v.to_string().as_bytes()));
        assert_eq!(
            decode_tl1(&code(json!({"v": 2, "name": "X"}))).err(),
            Some(ShareCodeError::UnsupportedVersion(2))
        );
        assert_eq!(
            decode_tl1(&code(json!({"name": "X"}))).err(),
            Some(ShareCodeError::BadJson)
        );
        assert_eq!(
            decode_tl1(&code(json!({"v": "1"}))).err(),
            Some(ShareCodeError::BadJson)
        );
        assert_eq!(
            decode_tl1(&code(json!([1]))).err(),
            Some(ShareCodeError::BadJson)
        );
        assert_eq!(
            decode_tl1(&format!("TL1.{}", encode_b64(&[0xFF, 0xFE, b'{']))).err(),
            Some(ShareCodeError::BadJson)
        );
        assert_eq!(decode_tl1("TL1.").err(), Some(ShareCodeError::BadJson));

        // A code with only v and a light window background repairs from Default Light.
        let def = decode_tl1(&code(json!({"v": 1, "WINDOW_BG": "#FAFAFA"}))).unwrap();
        let light = builtin_def(DEFAULT_LIGHT);
        assert_eq!(&*def.name, "Custom theme 1");
        assert_eq!(def.slots[ThemeSlot::WindowBg as usize], hex(0xFAFAFA));
        assert_eq!(
            def.slots[ThemeSlot::Header as usize],
            light.slots[ThemeSlot::Header as usize]
        );
        assert_eq!(def.accent, light.accent);

        // Over 1 MiB is refused before decoding.
        let long = format!("TL1.{}", "A".repeat(MAX_IMPORT_BYTES));
        assert_eq!(decode_tl1(&long).err(), Some(ShareCodeError::NotAThemeCode));
        assert_eq!(
            import_text(&long).err(),
            Some(ShareCodeError::NotAThemeCode)
        );
    }

    #[test]
    fn import_raw_json_list_and_single() {
        let dark = builtin_def(DEFAULT_DARK);
        let light = builtin_def(DEFAULT_LIGHT);
        let file = serialize_user_themes(&[dark.clone(), light.clone()]);
        let defs = import_text(&file).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, dark.name);
        assert_eq!(defs[1].slots, light.slots);

        let root: Value = serde_json::from_str(&file).unwrap();
        let single = root["themes"][1].to_string();
        let defs = import_text(&single).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, light.name);

        let defs = import_text(&encode_tl1(&dark)).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].slots, dark.slots);

        // Non-object entries are skipped; the count is capped at 64.
        let many = json!({"themes": (0..70).map(|i| json!({"name": format!("T{i}")})).chain([json!(5)]).collect::<Vec<_>>()});
        assert_eq!(import_text(&many.to_string()).unwrap().len(), 64);

        for bad in [
            "hello",
            "",
            "TS1.abc",
            "[]",
            "{}",
            "{\"themes\":{}}",
            "{\"name\":\"x\"}",
            "42",
        ] {
            assert_eq!(
                import_text(bad).err(),
                Some(ShareCodeError::NotAThemeCode),
                "{bad}"
            );
        }
        assert!(import_text("{\"themes\":[]}").unwrap().is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn import_never_panics(text in any::<String>()) {
            let _ = import_text(&text);
            let _ = decode_tl1(&text);
            let _ = import_text(&format!("TL1.{text}"));
            let _ = import_text(&format!("{{\"themes\":[{text}]}}"));
        }

        #[test]
        fn import_never_panics_on_base64_payloads(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = decode_tl1(&format!("TL1.{}", encode_b64(&bytes)));
        }

        #[test]
        fn base64url_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            prop_assert_eq!(base64url_decode(&encode_b64(&bytes)), Some(bytes));
        }
    }

    #[gpui_kit::test]
    fn share_encodes_preview_def(cx: &mut TestAppContext) {
        cx.update(|cx: &mut App| {
            gpui_kit::init(cx);
            let saves: Rc<RefCell<usize>> = Rc::default();
            let s = saves.clone();
            cx.set_global(ThemePersist(Box::new(move |p| {
                if let PersistTheme::Save(_) = p {
                    *s.borrow_mut() += 1;
                }
            })));
            store::init_with_file(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                None,
                1,
            );
            let saved = builtin_def(DEFAULT_DARK);
            let tok = ThemeStore::begin_preview(&saved, cx);
            ThemeStore::set_preview_slot(&tok, ThemeSlot::WindowBg, hex(0x123456), cx);

            let code = encode_tl1(&ThemeStore::preview_def(&tok, cx));
            let back = decode_tl1(&code).unwrap();
            assert_eq!(back.slots[ThemeSlot::WindowBg as usize], hex(0x123456));
            assert_ne!(
                back.slots[ThemeSlot::WindowBg as usize],
                saved.slots[ThemeSlot::WindowBg as usize]
            );
            assert_eq!(*saves.borrow(), 0);
            ThemeStore::cancel_preview(tok, cx);
        });
    }
}
