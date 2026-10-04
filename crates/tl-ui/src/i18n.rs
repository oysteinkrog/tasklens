//! Fluent string layer (APP-043, R-132, UI-SHL-131).
//!
//! Every user-visible string in tl-ui goes through a message key. The English catalogue is
//! embedded. A locale without an embedded catalogue uses English, and a key missing in the chosen
//! catalogue falls back to English. A language change replaces the bundle, bumps
//! `strings_version` and refreshes every window, so text changes without a restart.
//!
//! Numbers never pass through Fluent (R-081). Callers format them with the table (d) forms and
//! pass the text in as an argument.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use gpui_kit::{App, Global, SharedString};
use unic_langid::LanguageIdentifier;

/// The `general.language` values, canonical table (a).
pub const LOCALES: [&str; 12] = [
    "system", "en-US", "de-DE", "es-ES", "fr-FR", "ja-JP", "ko-KR", "pt-BR", "ru-RU", "uk-UA",
    "zh-CN", "nb-NO",
];

const EN_US_FTL: &str = include_str!("../../../assets/locales/en-US.ftl");

/// The active string bundles. Installed as a GPUI global.
pub struct Strings {
    bundle: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
    /// The resolved locale. It can name a locale whose text is still English.
    pub locale: LanguageIdentifier,
    /// Bumps on every language change. Callers that cache translated text key it by this.
    pub strings_version: u64,
}

impl Global for Strings {}

impl Strings {
    fn new(locale: LanguageIdentifier, strings_version: u64) -> Self {
        let bundle = match catalogue(&locale) {
            Some(ftl) => build_bundle(locale.clone(), ftl),
            None => build_bundle(en_us(), EN_US_FTL),
        };
        Strings {
            bundle,
            fallback: build_bundle(en_us(), EN_US_FTL),
            locale,
            strings_version,
        }
    }

    fn format(&self, key: &str, args: Option<&FluentArgs>) -> SharedString {
        format_in(&self.bundle, key, args)
            .or_else(|| format_in(&self.fallback, key, args))
            .unwrap_or_else(|| missing(key))
    }
}

/// Installs the strings for `setting` (`general.language`) with `strings_version` 0.
pub fn init_strings(cx: &mut App, setting: &str) {
    cx.set_global(Strings::new(resolve(setting, system_locale), 0));
}

/// Replaces the bundle for `setting`, bumps `strings_version` by 1 and refreshes every window.
pub fn set_language(cx: &mut App, setting: &str) {
    let version = cx
        .try_global::<Strings>()
        .map_or(0, |s| s.strings_version + 1);
    cx.set_global(Strings::new(resolve(setting, system_locale), version));
    cx.refresh_windows();
}

/// The text for `key`. Formats on every call, so it is not for per-cell, per-frame use.
pub fn tr(cx: &App, key: &str) -> SharedString {
    cx.global::<Strings>().format(key, None)
}

/// The text for `key` with Fluent arguments substituted.
pub fn tr_args(cx: &App, key: &str, args: &[(&str, FluentValue<'_>)]) -> SharedString {
    let mut fluent_args = FluentArgs::with_capacity(args.len());
    for (name, value) in args {
        fluent_args.set(*name, value.clone());
    }
    cx.global::<Strings>().format(key, Some(&fluent_args))
}

/// The locale from LC_ALL, then LC_MESSAGES, then LANG. Read once at startup. "C", "POSIX" and
/// values that do not parse give en-US.
pub fn system_locale() -> LanguageIdentifier {
    static SYSTEM: OnceLock<LanguageIdentifier> = OnceLock::new();
    SYSTEM
        .get_or_init(|| locale_from_env(|name| std::env::var(name).ok()))
        .clone()
}

/// Picks the first non-empty of LC_ALL, LC_MESSAGES and LANG from `var` and parses it.
fn locale_from_env(var: impl Fn(&str) -> Option<String>) -> LanguageIdentifier {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| var(name))
        .find(|value| !value.is_empty())
        .and_then(|value| parse_posix_locale(&value))
        .unwrap_or_else(en_us)
}

/// Parses a POSIX locale such as `nb_NO.UTF-8` or `de_DE@euro`. `None` for "C" and "POSIX".
fn parse_posix_locale(value: &str) -> Option<LanguageIdentifier> {
    let name = value.split(['.', '@']).next().unwrap_or_default();
    if name.is_empty() || name == "C" || name == "POSIX" {
        return None;
    }
    name.replace('_', "-").parse().ok()
}

/// `"system"` uses the system locale. A value that does not parse gives en-US.
fn resolve(setting: &str, system: fn() -> LanguageIdentifier) -> LanguageIdentifier {
    if setting == "system" {
        system()
    } else {
        setting.parse().unwrap_or_else(|_| en_us())
    }
}

fn en_us() -> LanguageIdentifier {
    "en-US".parse().unwrap_or_default()
}

/// The embedded catalogue for `locale`. Only en-US has one in P1.
fn catalogue(locale: &LanguageIdentifier) -> Option<&'static str> {
    (*locale == en_us()).then_some(EN_US_FTL)
}

/// Builds a bundle from `ftl`. Parse errors are logged and the parser's partial resource is used.
fn build_bundle(locale: LanguageIdentifier, ftl: &str) -> FluentBundle<FluentResource> {
    let resource = FluentResource::try_new(ftl.to_owned()).unwrap_or_else(|(partial, errors)| {
        tracing::error!(target: "tasklens::ui", %locale, ?errors, "string catalogue has parse errors");
        partial
    });
    let mut bundle = FluentBundle::new(vec![locale.clone()]);
    // The UI is LTR (UI-FND-053 rule 8), so no bidi isolation marks around arguments.
    bundle.set_use_isolating(false);
    if let Err(errors) = bundle.add_resource(resource) {
        tracing::error!(target: "tasklens::ui", %locale, ?errors, "string catalogue has duplicate keys");
    }
    bundle
}

fn format_in(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs>,
) -> Option<SharedString> {
    let pattern = bundle.get_message(key)?.value()?;
    let mut errors = Vec::new();
    let text = bundle.format_pattern(pattern, args, &mut errors);
    if !errors.is_empty() {
        tracing::warn!(target: "tasklens::ui", key, ?errors, "string format errors");
    }
    Some(SharedString::from(text.into_owned()))
}

/// A key in no catalogue: logs once per key and returns the key text.
fn missing(key: &str) -> SharedString {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let first = SEEN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert(key.to_owned());
    if first {
        tracing::warn!(target: "tasklens::ui", key, "missing string");
    }
    // tl-ui's own tests check the fallback on purpose, so the assert is off for them.
    #[cfg(not(test))]
    debug_assert!(false, "missing string: {key}");
    SharedString::from(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window, div};
    use std::cell::Cell;
    use std::rc::Rc;

    fn langid(s: &str) -> LanguageIdentifier {
        s.parse().expect("valid locale")
    }

    #[gpui_kit::test]
    fn tr_known_key(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_strings(cx, "en-US");
            assert_eq!(tr(cx, "fnd-menu-no-items"), "No items");
        });
    }

    #[gpui_kit::test]
    fn tr_args_substitutes_without_isolation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_strings(cx, "en-US");
            let text = tr_args(
                cx,
                "fnd-validity-stale",
                &[("age", FluentValue::from("5s"))],
            );
            assert_eq!(text, "Last updated 5s ago");
            assert!(!text.contains(['\u{2068}', '\u{2069}']));
        });
    }

    #[gpui_kit::test]
    fn unknown_locale_falls_back_to_english(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_strings(cx, "de-DE");
            assert_eq!(cx.global::<Strings>().locale, langid("de-DE"));
            assert_eq!(tr(cx, "fnd-menu-no-items"), "No items");
            assert_eq!(tr(cx, "fnd-pin-on"), "Always on top (on)");
        });
    }

    #[gpui_kit::test]
    fn system_setting_uses_system_locale_with_english_text(cx: &mut TestAppContext) {
        let nb = || locale_from_env(|name| (name == "LANG").then(|| "nb_NO.UTF-8".to_owned()));
        cx.update(|cx| {
            cx.set_global(Strings::new(resolve("system", nb), 0));
            assert_eq!(cx.global::<Strings>().locale, langid("nb-NO"));
            assert_eq!(tr(cx, "fnd-menu-no-items"), "No items");
        });
    }

    #[test]
    fn system_locale_reads_env_in_order() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let all = [
            ("LC_ALL", "de_DE.UTF-8"),
            ("LC_MESSAGES", "fr_FR.UTF-8"),
            ("LANG", "nb_NO.UTF-8"),
        ];
        assert_eq!(locale_from_env(env(&all)), langid("de-DE"));
        assert_eq!(locale_from_env(env(&all[1..])), langid("fr-FR"));
        assert_eq!(locale_from_env(env(&all[2..])), langid("nb-NO"));
        assert_eq!(
            locale_from_env(env(&[("LC_ALL", ""), ("LANG", "ja_JP.UTF-8")])),
            langid("ja-JP")
        );
        assert_eq!(
            locale_from_env(env(&[("LANG", "de_DE@euro")])),
            langid("de-DE")
        );
        assert_eq!(
            locale_from_env(env(&[("LC_ALL", "C"), ("LANG", "de_DE")])),
            en_us()
        );
        assert_eq!(locale_from_env(env(&[("LANG", "POSIX")])), en_us());
        assert_eq!(locale_from_env(env(&[("LANG", "C.UTF-8")])), en_us());
        assert_eq!(locale_from_env(env(&[])), en_us());
    }

    struct RenderCounter(Rc<Cell<u32>>);

    impl Render for RenderCounter {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div()
        }
    }

    #[gpui_kit::test]
    fn set_language_bumps_strings_version(cx: &mut TestAppContext) {
        cx.update(|cx| init_strings(cx, "en-US"));
        let renders = Rc::new(Cell::new(0));
        let counter = renders.clone();
        cx.add_window(|_, _| RenderCounter(counter));
        cx.run_until_parked();
        let before = renders.get();

        cx.update(|cx| {
            assert_eq!(cx.global::<Strings>().strings_version, 0);
            set_language(cx, "de-DE");
            assert_eq!(cx.global::<Strings>().strings_version, 1);
            assert_eq!(cx.global::<Strings>().locale, langid("de-DE"));
        });
        cx.run_until_parked();
        assert!(renders.get() > before, "window was not refreshed");
    }

    #[gpui_kit::test]
    fn missing_key_returns_key(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_strings(cx, "en-US");
            assert_eq!(tr(cx, "no-such-key-anywhere"), "no-such-key-anywhere");
        });
    }

    #[test]
    fn catalogue_parses_without_errors() {
        if let Err((_, errors)) = FluentResource::try_new(EN_US_FTL.to_owned()) {
            panic!("en-US.ftl has parse errors: {errors:?}");
        }
    }

    #[test]
    fn locales_match_table_a() {
        for locale in &LOCALES[1..] {
            assert!(locale.parse::<LanguageIdentifier>().is_ok(), "{locale}");
        }
    }
}
