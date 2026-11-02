//! `Template`: a translated message whose arguments are left as holes, so a new value is
//! spliced in without going through Fluent. It sits in the base layer because pages, windows
//! and the action tracker all use it.

use std::fmt;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString};
use intl_pluralrules::{PluralCategory, PluralRuleType, PluralRules};
use smallvec::SmallVec;
use unic_langid::LanguageIdentifier;

use crate::i18n::{Strings, tr_args, tr_en};

/// Stand-ins for the arguments of a [`Template`]: private-use characters no translation has.
pub const HOLES: [&str; 4] = ["\u{E000}", "\u{E001}", "\u{E002}", "\u{E003}"];

/// A translated message whose arguments are left as holes, so a new value is spliced in
/// without going through Fluent. Numbers never pass through Fluent anyway (R-081).
///
/// A counted message ([`Template::counted`]) is translated once per CLDR plural category of the
/// text's language, with a sample number of that category as the count, so a Fluent selector
/// on the count picks that category's variant. [`Template::write_count`] picks the form for a
/// count with the plural rules and makes no Fluent call. Only category keys (`[one]`) work: a
/// key that is an exact number (`[0]`) is never picked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Template {
    /// The text with every argument a hole. For a counted message, the default variant.
    text: SharedString,
    plural: Option<Arc<Plural>>,
}

/// The forms of a counted message.
struct Plural {
    rules: PluralRules,
    /// The text per category in [`CATEGORIES`] order, the count a hole; `None` for a category
    /// the language does not have.
    forms: [Option<SharedString>; 6],
}

impl fmt::Debug for Plural {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plural")
            .field("locale", self.rules.get_locale())
            .field("forms", &self.forms)
            .finish()
    }
}

impl PartialEq for Plural {
    fn eq(&self, other: &Self) -> bool {
        self.rules.get_locale() == other.rules.get_locale() && self.forms == other.forms
    }
}

impl Eq for Plural {}

/// The CLDR plural categories, in the order of [`Plural::forms`].
const CATEGORIES: [PluralCategory; 6] = [
    PluralCategory::ZERO,
    PluralCategory::ONE,
    PluralCategory::TWO,
    PluralCategory::FEW,
    PluralCategory::MANY,
    PluralCategory::OTHER,
];
/// Counts below this are searched for samples of each category.
const SAMPLE_LIMIT: u32 = 1000;
/// Samples tried per category, for one whose number shows once in the text.
const SAMPLES_TRIED: usize = 3;

impl Template {
    /// Translates `key` with the arguments `args` (at most four) as holes.
    pub fn new(cx: &App, key: &str, args: &[&str]) -> Self {
        Self::plain(tr_args(cx, key, &holes(args)))
    }

    /// [`Template::new`] in en-US, for a table built before the strings can be read.
    pub fn en(key: &str, args: &[&str]) -> Self {
        Self::plain(tr_en(key, &holes(args)))
    }

    /// [`Template::new`] for a message whose argument `args[count]` is a count. Write it with
    /// [`Template::write_count`].
    pub fn counted(cx: &App, key: &str, args: &[&str], count: usize) -> Self {
        let locale = cx.global::<Strings>().text_locale();
        Self::counted_in(locale, args, count, |a| tr_args(cx, key, a))
    }

    /// [`Template::counted`] in en-US.
    pub fn counted_en(key: &str, args: &[&str], count: usize) -> Self {
        let locale: LanguageIdentifier = "en-US".parse().unwrap_or_default();
        Self::counted_in(&locale, args, count, |a| tr_en(key, a))
    }

    fn plain(text: SharedString) -> Self {
        Self { text, plural: None }
    }

    /// Translates with `tr` once per plural category of `locale`, and once with every argument
    /// a hole for [`Template::text`].
    fn counted_in(
        locale: &LanguageIdentifier,
        args: &[&str],
        count: usize,
        tr: impl Fn(&[(&str, FluentValue<'static>)]) -> SharedString,
    ) -> Self {
        let mut fluent = holes(args);
        let plain = Self::plain(tr(&fluent));
        let Some(rules) = plural_rules(locale).filter(|_| count < fluent.len()) else {
            return plain;
        };
        let mut samples: [SmallVec<[u32; SAMPLES_TRIED]>; 6] = Default::default();
        for n in 0..SAMPLE_LIMIT {
            if let Ok(category) = rules.select(n) {
                let list = &mut samples[category_index(&category)];
                if list.len() < SAMPLES_TRIED {
                    list.push(n);
                }
            }
        }
        let mut forms: [Option<SharedString>; 6] = Default::default();
        for (form, samples) in forms.iter_mut().zip(&samples) {
            for &n in samples {
                fluent[count].1 = FluentValue::from(n);
                let text = tr(&fluent);
                let digits = n.to_string();
                let spots = number_spots(&text, &digits);
                // The first sample whose number shows at most once; else the first try.
                let done = spots.len() <= 1;
                if done || form.is_none() {
                    *form = Some(match spots.first() {
                        Some(&at) => {
                            let mut out = String::with_capacity(text.len());
                            out.push_str(&text[..at]);
                            out.push_str(HOLES[count]);
                            out.push_str(&text[at + digits.len()..]);
                            out.into()
                        }
                        None => text,
                    });
                }
                if done {
                    break;
                }
            }
        }
        Self {
            plural: Some(Arc::new(Plural { rules, forms })),
            ..plain
        }
    }

    /// Writes the text with `values[i]` in the place of argument `i`.
    pub fn write(&self, out: &mut dyn fmt::Write, values: &[&dyn fmt::Display]) -> fmt::Result {
        write_holes(&self.text, out, values)
    }

    /// [`Template::write`] in the plural form for `count`, for a [`Template::counted`] message.
    /// Any other message writes as [`Template::write`].
    pub fn write_count(
        &self,
        out: &mut dyn fmt::Write,
        count: u64,
        values: &[&dyn fmt::Display],
    ) -> fmt::Result {
        let form = self.plural.as_ref().and_then(|p| {
            let category = p.rules.select(count).ok()?;
            p.forms[category_index(&category)].as_ref()
        });
        write_holes(form.unwrap_or(&self.text), out, values)
    }

    /// Writes the text, with `arg(i, out)` writing argument `i` in its place.
    pub fn write_each(&self, out: &mut String, mut arg: impl FnMut(usize, &mut String)) {
        let mut rest = self.text.as_ref();
        while let Some((at, i)) = next_hole(rest) {
            out.push_str(&rest[..at]);
            arg(i, out);
            rest = &rest[at + HOLES[i].len()..];
        }
        out.push_str(rest);
    }

    /// The text with no arguments.
    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

/// Writes `text` with `values[i]` in the place of hole `i`.
fn write_holes(text: &str, out: &mut dyn fmt::Write, values: &[&dyn fmt::Display]) -> fmt::Result {
    let mut rest = text;
    while let Some((at, i)) = next_hole(rest) {
        out.write_str(&rest[..at])?;
        if let Some(value) = values.get(i) {
            write!(out, "{value}")?;
        }
        rest = &rest[at + HOLES[i].len()..];
    }
    out.write_str(rest)
}

/// The cardinal plural rules of `locale`, else of its language.
fn plural_rules(locale: &LanguageIdentifier) -> Option<PluralRules> {
    PluralRules::create(locale.clone(), PluralRuleType::CARDINAL)
        .or_else(|_| {
            let language = LanguageIdentifier::from_parts(locale.language, None, None, &[]);
            PluralRules::create(language, PluralRuleType::CARDINAL)
        })
        .ok()
}

fn category_index(category: &PluralCategory) -> usize {
    CATEGORIES
        .iter()
        .position(|c| c == category)
        .unwrap_or(CATEGORIES.len() - 1)
}

/// The byte offsets where `digits` stands in `text` as a whole number.
fn number_spots(text: &str, digits: &str) -> SmallVec<[usize; 2]> {
    let is_digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    text.match_indices(digits)
        .map(|(at, _)| at)
        .filter(|&at| {
            !is_digit(text[..at].chars().next_back())
                && !is_digit(text[at + digits.len()..].chars().next())
        })
        .collect()
}

/// The Fluent arguments that put a hole in the place of each of `names`.
fn holes<'a>(names: &[&'a str]) -> SmallVec<[(&'a str, FluentValue<'static>); 4]> {
    names
        .iter()
        .zip(HOLES)
        .map(|(name, hole)| (*name, hole.into()))
        .collect()
}

/// The byte offset and index of the first hole in `s`.
pub fn next_hole(s: &str) -> Option<(usize, usize)> {
    s.char_indices().find_map(|(at, c)| {
        let i = (c as u32).checked_sub(0xE000)? as usize;
        (i < HOLES.len()).then_some((at, i))
    })
}
