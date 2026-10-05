//! Desktop Entry parser for XDG autostart files (SYS-019, SYS-020, SYS-026).
//!
//! Autostart files are untrusted input: any program can drop one in `~/.config/autostart`. The
//! parser reads at most [`MAX_DESKTOP_FILE`] bytes, never panics, and passes every string it keeps
//! through [`crate::sanitize`] (ADR-015). It only reads; writing overrides lives in `tl-ops`.

use std::sync::Arc;

use smallvec::SmallVec;

use crate::sanitize::sanitize_text;

/// Keys of the `[Desktop Entry]` group of one autostart file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopEntry {
    /// `Name`, localised with the caller's match list.
    pub name: Option<Arc<str>>,
    /// `Exec`, unescaped.
    pub exec: Option<Arc<str>>,
    /// `TryExec`.
    pub try_exec: Option<Arc<str>>,
    /// `Icon`: a name or a path, resolved later.
    pub icon: Option<Arc<str>>,
    /// `Path`: the working directory.
    pub path: Option<Arc<str>>,
    /// `Type`.
    pub entry_type: Option<Arc<str>>,
    /// `Hidden=true`.
    pub hidden: bool,
    /// `X-GNOME-Autostart-enabled`.
    pub gnome_autostart_enabled: Option<bool>,
    /// `OnlyShowIn`, split on `;`, empty items dropped.
    pub only_show_in: SmallVec<[Arc<str>; 2]>,
    /// `NotShowIn`, split on `;`, empty items dropped.
    pub not_show_in: SmallVec<[Arc<str>; 2]>,
    /// `X-KDE-autostart-phase`.
    pub kde_autostart_phase: Option<Arc<str>>,
    /// `X-Tasklens-Override=true`.
    pub tasklens_override: bool,
}

/// Read limit for one `.desktop` file, in bytes (R-164).
pub const MAX_DESKTOP_FILE: usize = 64 * 1024;

const GROUP: &[u8] = b"[Desktop Entry]";

// Bits in the "seen" mask, so the first of duplicate keys wins.
const K_NAME: u16 = 1 << 0;
const K_EXEC: u16 = 1 << 1;
const K_TRY_EXEC: u16 = 1 << 2;
const K_ICON: u16 = 1 << 3;
const K_PATH: u16 = 1 << 4;
const K_TYPE: u16 = 1 << 5;
const K_HIDDEN: u16 = 1 << 6;
const K_GNOME: u16 = 1 << 7;
const K_ONLY: u16 = 1 << 8;
const K_NOT: u16 = 1 << 9;
const K_KDE_PHASE: u16 = 1 << 10;
const K_OVERRIDE: u16 = 1 << 11;

/// Parses the `[Desktop Entry]` group of `bytes`. `locales` is the caller's match list, most
/// specific first, for example ["de_DE@euro", "de_DE", "de@euro", "de"].
pub fn parse_desktop_entry(bytes: &[u8], locales: &[&str]) -> DesktopEntry {
    let bytes = &bytes[..bytes.len().min(MAX_DESKTOP_FILE)];
    let mut entry = DesktopEntry::default();
    let mut seen = 0u16;
    // Index into `locales` of the best `Name[..]` found so far, and its value.
    let mut local_name: Option<(usize, Arc<str>)> = None;
    let mut in_group = false;
    let mut scratch = Vec::new();
    let mut text = String::new();

    let mut start = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |i| start + i);
        let line = bytes[start..end].trim_ascii();
        start = end + 1;

        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            if in_group {
                break;
            }
            in_group = line == GROUP;
            continue;
        }
        if !in_group {
            continue;
        }
        let Some(eq) = memchr::memchr(b'=', line) else {
            continue;
        };
        let key = line[..eq].trim_ascii_end();
        let value = line[eq + 1..].trim_ascii_start();

        if let Some(locale) = key
            .strip_prefix(b"Name[")
            .and_then(|k| k.strip_suffix(b"]"))
        {
            let Some(rank) = locales.iter().position(|l| l.as_bytes() == locale) else {
                continue;
            };
            if local_name.as_ref().is_none_or(|(best, _)| rank < *best) {
                local_name = Some((rank, string_value(value, &mut scratch, &mut text)));
            }
            continue;
        }

        let bit = match key {
            b"Name" => K_NAME,
            b"Exec" => K_EXEC,
            b"TryExec" => K_TRY_EXEC,
            b"Icon" => K_ICON,
            b"Path" => K_PATH,
            b"Type" => K_TYPE,
            b"Hidden" => K_HIDDEN,
            b"X-GNOME-Autostart-enabled" => K_GNOME,
            b"OnlyShowIn" => K_ONLY,
            b"NotShowIn" => K_NOT,
            b"X-KDE-autostart-phase" => K_KDE_PHASE,
            b"X-Tasklens-Override" => K_OVERRIDE,
            _ => continue,
        };
        if seen & bit != 0 {
            continue;
        }
        seen |= bit;

        match bit {
            K_NAME => entry.name = Some(string_value(value, &mut scratch, &mut text)),
            K_EXEC => entry.exec = Some(string_value(value, &mut scratch, &mut text)),
            K_TRY_EXEC => entry.try_exec = Some(string_value(value, &mut scratch, &mut text)),
            K_ICON => entry.icon = Some(string_value(value, &mut scratch, &mut text)),
            K_PATH => entry.path = Some(string_value(value, &mut scratch, &mut text)),
            K_TYPE => entry.entry_type = Some(string_value(value, &mut scratch, &mut text)),
            K_KDE_PHASE => {
                entry.kde_autostart_phase = Some(string_value(value, &mut scratch, &mut text));
            }
            K_HIDDEN => entry.hidden = bool_value(value) == Some(true),
            K_GNOME => entry.gnome_autostart_enabled = bool_value(value),
            K_OVERRIDE => entry.tasklens_override = bool_value(value) == Some(true),
            K_ONLY => list_value(value, &mut entry.only_show_in, &mut scratch, &mut text),
            K_NOT => list_value(value, &mut entry.not_show_in, &mut scratch, &mut text),
            _ => {}
        }
    }

    if let Some((_, name)) = local_name {
        entry.name = Some(name);
    }
    entry
}

/// Builds the match list from a POSIX locale string such as "de_DE.UTF-8@euro" (encoding dropped).
pub fn locale_match_list(locale: &str) -> SmallVec<[String; 4]> {
    let (rest, modifier) = match locale.split_once('@') {
        Some((rest, m)) if !m.is_empty() => (rest, Some(m)),
        Some((rest, _)) => (rest, None),
        None => (locale, None),
    };
    let rest = rest.split_once('.').map_or(rest, |(r, _)| r);
    let (lang, country) = match rest.split_once('_') {
        Some((l, c)) if !c.is_empty() => (l, Some(c)),
        Some((l, _)) => (l, None),
        None => (rest, None),
    };

    let mut out = SmallVec::new();
    if lang.is_empty() {
        return out;
    }
    if let (Some(c), Some(m)) = (country, modifier) {
        out.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        out.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        out.push(format!("{lang}@{m}"));
    }
    out.push(lang.to_owned());
    out
}

/// `true` or `false` exactly; anything else is absent.
fn bool_value(value: &[u8]) -> Option<bool> {
    match value {
        b"true" => Some(true),
        b"false" => Some(false),
        _ => None,
    }
}

/// Unescapes and sanitizes one string value.
fn string_value(value: &[u8], scratch: &mut Vec<u8>, text: &mut String) -> Arc<str> {
    scratch.clear();
    let mut rest = value;
    while let Some((&b, tail)) = rest.split_first() {
        rest = tail;
        if b == b'\\' {
            match rest
                .split_first()
                .and_then(|(&e, t)| unescape(e, false).map(|u| (u, t)))
            {
                Some((u, t)) => {
                    scratch.push(u);
                    rest = t;
                }
                None => scratch.push(b),
            }
        } else {
            scratch.push(b);
        }
    }
    sanitize_text(scratch, MAX_DESKTOP_FILE, text);
    Arc::from(text.as_str())
}

/// Splits a list value on unescaped `;`, unescapes and sanitizes each item, and drops empty items.
fn list_value(
    value: &[u8],
    out: &mut SmallVec<[Arc<str>; 2]>,
    scratch: &mut Vec<u8>,
    text: &mut String,
) {
    scratch.clear();
    let mut rest = value;
    loop {
        let Some((&b, tail)) = rest.split_first() else {
            push_item(scratch, out, text);
            return;
        };
        rest = tail;
        match b {
            b';' => {
                push_item(scratch, out, text);
                scratch.clear();
            }
            b'\\' => match rest
                .split_first()
                .and_then(|(&e, t)| unescape(e, true).map(|u| (u, t)))
            {
                Some((u, t)) => {
                    scratch.push(u);
                    rest = t;
                }
                None => scratch.push(b),
            },
            _ => scratch.push(b),
        }
    }
}

fn push_item(item: &[u8], out: &mut SmallVec<[Arc<str>; 2]>, text: &mut String) {
    sanitize_text(item, MAX_DESKTOP_FILE, text);
    if !text.is_empty() {
        out.push(Arc::from(text.as_str()));
    }
}

/// The byte an escape `\<e>` stands for. `None` keeps both characters.
fn unescape(e: u8, list: bool) -> Option<u8> {
    match e {
        b's' => Some(b' '),
        b'n' => Some(b'\n'),
        b't' => Some(b'\t'),
        b'r' => Some(b'\r'),
        b'\\' => Some(b'\\'),
        b';' if list => Some(b';'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[Arc<str>]) -> Vec<&str> {
        list.iter().map(|s| &**s).collect()
    }

    #[test]
    fn basic_keys_and_locale() {
        let file = b"[Desktop Entry]\nName=Foo\nName[de]=Fuh\nExec=foo --bar\nHidden=true\nOnlyShowIn=KDE;GNOME;\n";
        let e = parse_desktop_entry(file, &["de"]);
        assert_eq!(e.name.as_deref(), Some("Fuh"));
        assert_eq!(e.exec.as_deref(), Some("foo --bar"));
        assert!(e.hidden);
        assert_eq!(names(&e.only_show_in), ["KDE", "GNOME"]);

        // Without a matching locale, plain Name is used.
        let e = parse_desktop_entry(file, &["fr"]);
        assert_eq!(e.name.as_deref(), Some("Foo"));

        // The most specific locale wins, whatever the order in the file.
        let file = b"[Desktop Entry]\nName[de]=A\nName[de_DE]=B\nName=C\n";
        let e = parse_desktop_entry(file, &["de_DE", "de"]);
        assert_eq!(e.name.as_deref(), Some("B"));

        // Spaces around `=` are trimmed, and the first duplicate key wins.
        let file = b"[Desktop Entry]\nExec = first\nExec=second\nType=Application\n";
        let e = parse_desktop_entry(file, &[]);
        assert_eq!(e.exec.as_deref(), Some("first"));
        assert_eq!(e.entry_type.as_deref(), Some("Application"));
    }

    #[test]
    fn escapes() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nExec=foo\\sbar\\\\x\n", &[]);
        assert_eq!(e.exec.as_deref(), Some("foo bar\\x"));

        // An unknown escape keeps both characters; `\;` is only special in lists.
        let e = parse_desktop_entry(b"[Desktop Entry]\nExec=a\\qb\\;c\\\n", &[]);
        assert_eq!(e.exec.as_deref(), Some("a\\qb\\;c\\"));

        let e = parse_desktop_entry(b"[Desktop Entry]\nNotShowIn=A\\;B;;C\n", &[]);
        assert_eq!(names(&e.not_show_in), ["A;B", "C"]);
    }

    #[test]
    fn ignores_other_groups() {
        let file = b"# comment\nExec=outside\n[Desktop Action new]\nExec=action\nName=Action\n\
            [Desktop Entry]\nName=Real\nnot a key line\n[Other Group]\nExec=other\n";
        let e = parse_desktop_entry(file, &[]);
        assert_eq!(e.name.as_deref(), Some("Real"));
        assert_eq!(e.exec, None);
    }

    #[test]
    fn bool_strict() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nHidden=yes\n", &[]);
        assert!(!e.hidden);
        let e = parse_desktop_entry(
            b"[Desktop Entry]\nX-GNOME-Autostart-enabled=false\nHidden=True\n",
            &[],
        );
        assert_eq!(e.gnome_autostart_enabled, Some(false));
        assert!(!e.hidden);
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-GNOME-Autostart-enabled=1\n", &[]);
        assert_eq!(e.gnome_autostart_enabled, None);
    }

    #[test]
    fn sanitized_strings() {
        let e = parse_desktop_entry(
            "[Desktop Entry]\nName=ab\u{202E}cd\x07ef\nX-KDE-autostart-phase=\u{2066}1\n"
                .as_bytes(),
            &[],
        );
        assert_eq!(e.name.as_deref(), Some("abcd\u{FFFD}ef"));
        assert_eq!(e.kde_autostart_phase.as_deref(), Some("1"));

        // Invalid UTF-8 becomes U+FFFD.
        let e = parse_desktop_entry(b"[Desktop Entry]\nIcon=a\xFFb\n", &[]);
        assert_eq!(e.icon.as_deref(), Some("a\u{FFFD}b"));
    }

    #[test]
    fn tasklens_override_key() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-Tasklens-Override=true\n", &[]);
        assert!(e.tasklens_override);
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-Tasklens-Override=yes\n", &[]);
        assert!(!e.tasklens_override);
    }

    #[test]
    fn oversized_input_is_cut() {
        let mut file = b"[Desktop Entry]\nName=Big\n".to_vec();
        file.resize(MAX_DESKTOP_FILE, b'#');
        file.extend_from_slice(b"\nExec=past-the-limit\n");
        let e = parse_desktop_entry(&file, &[]);
        assert_eq!(e.name.as_deref(), Some("Big"));
        assert_eq!(e.exec, None);
    }

    #[test]
    fn locale_list() {
        assert_eq!(
            locale_match_list("de_DE.UTF-8@euro").as_slice(),
            ["de_DE@euro", "de_DE", "de@euro", "de"]
        );
        assert_eq!(locale_match_list("nb_NO.UTF-8").as_slice(), ["nb_NO", "nb"]);
        assert_eq!(locale_match_list("sr@latin").as_slice(), ["sr@latin", "sr"]);
        assert_eq!(locale_match_list("C").as_slice(), ["C"]);
        assert!(locale_match_list("").is_empty());
    }
}
