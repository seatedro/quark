//! Localization for Quark apps and Quark's own strings.
//!
//! Messages are [Fluent](https://projectfluent.org): translators get plural
//! and gender selectors, and plural categories come from CLDR rules for
//! each language (`one`, `few`, `many`, ...). A [`Localizer`] negotiates
//! the user's preferred locales (from the OS by default) against the
//! locales that have messages, falls back through them to English, and
//! formats numbers and dates the locale's way ([`NumberFormat`],
//! [`DateFormat`]). Numbers inside messages are formatted the same way.
//!
//! Quark's own labels (find bar buttons, palette placeholder, assistive
//! tech names) are messages with ids starting `quark-`, shipped in
//! English, German, Japanese, and Arabic; an app translates them to other
//! locales by including those ids in its own catalogs. Framework code looks
//! messages up through the process-wide localizer ([`tr`], [`tr_args`]),
//! which is English until the app [`install`]s another.
//!
//! [`Localizer::direction`] says whether the locale is written right to
//! left, for the app to hand to its layout.
//!
//! Fluent was chosen over a lighter format for its selectors and its
//! translator tooling. Its crates (fluent-bundle, fluent-syntax,
//! intl_pluralrules, unic-langid) carry no locale data beyond the plural
//! rules; number and date conventions come from the table in [`format`]
//! rather than icu4x, whose compiled date data alone runs to megabytes.

pub mod format;

use std::sync::{Arc, LazyLock, RwLock};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::types::FluentNumber;
use fluent_bundle::{FluentResource, FluentValue};
use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use intl_memoizer::Memoizable;

pub use fluent_bundle::FluentArgs;
pub use format::{DateFormat, NumberFormat};
pub use unic_langid::LanguageIdentifier;

/// Quark's own messages, by locale.
const BUILTIN: &[(&str, &str)] = &[
    ("en-US", include_str!("../locales/en-US/quark.ftl")),
    ("de", include_str!("../locales/de/quark.ftl")),
    ("ja", include_str!("../locales/ja/quark.ftl")),
    ("ar", include_str!("../locales/ar/quark.ftl")),
];

const DEFAULT_LOCALE: &str = "en-US";

/// Which way a locale's text runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    LeftToRight,
    RightToLeft,
}

/// Messages that could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I18nError {
    pub locale: String,
    pub errors: Vec<String>,
}

impl std::fmt::Display for I18nError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "messages for {}: {}",
            self.locale,
            self.errors.join("; ")
        )
    }
}

impl std::error::Error for I18nError {}

/// Messages, plural rules, and number and date formats for a negotiated
/// chain of locales.
pub struct Localizer {
    /// Best match first, ending with English.
    bundles: Vec<FluentBundle<FluentResource>>,
    locale: LanguageIdentifier,
    /// Of the language the messages are in.
    direction: Direction,
    numbers: NumberFormat,
    dates: DateFormat,
}

impl std::fmt::Debug for Localizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Localizer")
            .field("locale", &self.locale.to_string())
            .finish()
    }
}

/// Builds a [`Localizer`] from the app's catalogs and preferred locales.
#[derive(Debug, Clone, Default)]
pub struct LocalizerBuilder {
    requested: Vec<LanguageIdentifier>,
    catalogs: Vec<(LanguageIdentifier, String)>,
}

impl LocalizerBuilder {
    /// The user's preferred locales, most preferred first (BCP 47 tags such
    /// as `de-AT`). Without any, the OS's are used.
    pub fn requested<S: AsRef<str>>(mut self, locales: impl IntoIterator<Item = S>) -> Self {
        self.requested = locales
            .into_iter()
            .filter_map(|l| parse_locale(l.as_ref()))
            .collect();
        self
    }

    /// Fluent messages for `locale`. Ids that Quark also defines replace
    /// Quark's text.
    pub fn messages(mut self, locale: &str, ftl: impl Into<String>) -> Self {
        if let Some(locale) = parse_locale(locale) {
            self.catalogs.push((locale, ftl.into()));
        }
        self
    }

    pub fn build(self) -> Result<Localizer, I18nError> {
        let requested = if self.requested.is_empty() {
            os_locales()
        } else {
            self.requested
        };
        let mut available: Vec<LanguageIdentifier> = BUILTIN
            .iter()
            .filter_map(|(tag, _)| parse_locale(tag))
            .chain(self.catalogs.iter().map(|(l, _)| l.clone()))
            .collect();
        available.dedup();
        let default = default_locale();
        let chosen: Vec<LanguageIdentifier> = negotiate_languages(
            &requested,
            &available,
            Some(&default),
            NegotiationStrategy::Filtering,
        )
        .into_iter()
        .cloned()
        .collect();
        // Formats follow the most preferred requested locale even when no
        // messages exist for it (Swiss German numbers with German text).
        let locale = requested
            .first()
            .cloned()
            .or_else(|| chosen.first().cloned())
            .unwrap_or_else(default_locale);
        let mut bundles = Vec::with_capacity(chosen.len());
        for (i, bundle_locale) in chosen.iter().enumerate() {
            // A bundle in the user's language selects plurals and formats
            // numbers for their region too (de-CH with de messages); any
            // other uses its own language's rules, which its messages are
            // written for.
            let same_language = i == 0 && bundle_locale.language == locale.language;
            let lang = if same_language {
                locale.clone()
            } else {
                bundle_locale.clone()
            };
            let mut bundle = FluentBundle::new_concurrent(vec![lang]);
            bundle.set_use_isolating(false);
            bundle.set_formatter(Some(format_number));
            let sources = BUILTIN
                .iter()
                .filter(|(tag, _)| parse_locale(tag).as_ref() == Some(bundle_locale))
                .map(|(_, ftl)| (*ftl).to_owned())
                .chain(
                    self.catalogs
                        .iter()
                        .filter(|(l, _)| l == bundle_locale)
                        .map(|(_, ftl)| ftl.clone()),
                );
            for ftl in sources {
                let resource = FluentResource::try_new(ftl).map_err(|(_, errors)| I18nError {
                    locale: bundle_locale.to_string(),
                    errors: errors.iter().map(|e| e.to_string()).collect(),
                })?;
                bundle.add_resource_overriding(resource);
            }
            bundles.push(bundle);
        }
        Ok(Localizer {
            direction: direction(chosen.first().unwrap_or(&locale)),
            bundles,
            numbers: NumberFormat::for_locale(&locale),
            dates: DateFormat::for_locale(&locale),
            locale,
        })
    }
}

impl Localizer {
    pub fn builder() -> LocalizerBuilder {
        LocalizerBuilder::default()
    }

    /// Quark's messages in English.
    pub fn english() -> Self {
        Self::for_locales(&[DEFAULT_LOCALE])
    }

    /// Quark's messages for the OS's preferred locales.
    pub fn from_os() -> Self {
        Self::builder().build().unwrap_or_else(|_| Self::english())
    }

    /// Quark's messages for `locales`, most preferred first.
    pub fn for_locales(locales: &[&str]) -> Self {
        Self::builder()
            .requested(locales)
            .build()
            .unwrap_or_else(|_| Self::english())
    }

    /// The locale formats follow: the user's most preferred one.
    pub fn locale(&self) -> &LanguageIdentifier {
        &self.locale
    }

    /// Which way the messages' language runs.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// The message `id` without arguments; the id itself if no catalog has
    /// it.
    pub fn message(&self, id: &str) -> String {
        self.format(id, None)
    }

    /// The message `id` with `args`. Numbers given as numbers select
    /// plurals and print the locale's way.
    pub fn format(&self, id: &str, args: Option<&FluentArgs>) -> String {
        for bundle in &self.bundles {
            let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) else {
                continue;
            };
            let mut errors = Vec::new();
            return bundle
                .format_pattern(pattern, args, &mut errors)
                .into_owned();
        }
        id.to_owned()
    }

    /// `value` with up to `max_fraction` fraction digits.
    pub fn number(&self, value: f64, max_fraction: usize) -> String {
        self.numbers.format(value, 0, max_fraction)
    }

    /// A short numeric date.
    pub fn date(&self, year: i32, month: u8, day: u8) -> String {
        self.dates.format(year, month, day)
    }
}

/// Numbers in messages, in the bundle locale's format.
fn format_number<M: fluent_bundle::memoizer::MemoizerKind>(
    value: &FluentValue,
    memoizer: &M,
) -> Option<String> {
    let FluentValue::Number(FluentNumber { value, options }) = value else {
        return None;
    };
    let min = options.minimum_fraction_digits.unwrap_or(0);
    let max = options.maximum_fraction_digits.unwrap_or(3).max(min);
    memoizer
        .with_try_get_threadsafe::<MemoizedNumbers, _, _>((), |numbers| {
            numbers.0.format(*value, min, max)
        })
        .ok()
}

/// [`NumberFormat`] built once per bundle by Fluent's memoizer.
struct MemoizedNumbers(NumberFormat);

impl Memoizable for MemoizedNumbers {
    type Args = ();
    type Error = ();

    fn construct(lang: LanguageIdentifier, _: ()) -> Result<Self, ()> {
        Ok(Self(NumberFormat::for_locale(&lang)))
    }
}

fn parse_locale(tag: &str) -> Option<LanguageIdentifier> {
    // POSIX locales arrive as `de_DE.UTF-8`.
    let tag = tag
        .split(['.', '@'])
        .next()
        .unwrap_or(tag)
        .replace('_', "-");
    tag.parse().ok()
}

fn default_locale() -> LanguageIdentifier {
    parse_locale(DEFAULT_LOCALE).unwrap_or_default()
}

/// The OS's preferred locales, most preferred first.
pub fn os_locales() -> Vec<LanguageIdentifier> {
    sys_locale::get_locales()
        .filter_map(|tag| parse_locale(&tag))
        .collect()
}

/// Right to left for Arabic, Hebrew, Persian, Urdu, and other languages
/// in those scripts.
pub fn direction(locale: &LanguageIdentifier) -> Direction {
    let rtl_script = locale.script.as_ref().is_some_and(|s| {
        matches!(
            s.as_str(),
            "Arab" | "Hebr" | "Thaa" | "Syrc" | "Nkoo" | "Adlm"
        )
    });
    let latin_script = locale.script.as_ref().is_some_and(|s| s.as_str() == "Latn");
    let rtl_language = matches!(
        locale.language.as_str(),
        "ar" | "he" | "fa" | "ur" | "ps" | "sd" | "ug" | "yi" | "dv" | "ckb" | "syr"
    );
    if rtl_script || (rtl_language && !latin_script) {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    }
}

static CURRENT: LazyLock<RwLock<Arc<Localizer>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Localizer::english())));

/// Make `localizer` the one Quark's components and [`tr`] use. Views built
/// after this read the new strings.
pub fn install(localizer: Localizer) {
    if let Ok(mut current) = CURRENT.write() {
        *current = Arc::new(localizer);
    }
}

/// The process-wide localizer.
pub fn current() -> Arc<Localizer> {
    CURRENT
        .read()
        .map(|current| current.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// The message `id` from the process-wide localizer.
pub fn tr(id: &str) -> String {
    current().message(id)
}

/// The message `id` with named arguments from the process-wide localizer.
/// Integers and floats select plurals; strings are inserted as is.
pub fn tr_args<'a>(id: &str, args: impl IntoIterator<Item = (&'a str, Arg<'a>)>) -> String {
    let mut fluent = FluentArgs::new();
    for (name, value) in args {
        match value {
            Arg::Number(n) => fluent.set(name, n),
            Arg::Text(s) => fluent.set(name, s),
        }
    }
    current().format(id, Some(&fluent))
}

/// A message argument for [`tr_args`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'a> {
    Number(f64),
    Text(&'a str),
}

impl From<f64> for Arg<'_> {
    fn from(n: f64) -> Self {
        Self::Number(n)
    }
}

impl From<usize> for Arg<'_> {
    fn from(n: usize) -> Self {
        Self::Number(n as f64)
    }
}

impl<'a> From<&'a str> for Arg<'a> {
    fn from(s: &'a str) -> Self {
        Self::Text(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A localizer for `locale` whose `items` message shows the plural
    /// category CLDR picks, after the number in the locale's format.
    fn items(locale: &str) -> Localizer {
        let ftl = "items = { $n ->
    [zero] { $n } zero
    [one] { $n } one
    [two] { $n } two
    [few] { $n } few
    [many] { $n } many
   *[other] { $n } other
}
";
        Localizer::builder()
            .requested([locale])
            .messages(locale, ftl)
            .build()
            .expect("valid messages")
    }

    #[test]
    fn plural_categories_and_numbers_in_messages() {
        let cases: &[(&str, f64, &str)] = &[
            ("en-US", 1.0, "1 one"),
            ("en-US", 1234.0, "1,234 other"),
            ("en-US", 1.5, "1.5 other"),
            ("pl", 1.0, "1 one"),
            ("pl", 3.0, "3 few"),
            ("pl", 5.0, "5 many"),
            ("pl", 22.0, "22 few"),
            ("pl", 12345.0, "12\u{a0}345 many"),
            ("ar", 0.0, "\u{660} zero"),
            ("ar", 2.0, "\u{662} two"),
            ("ar", 3.0, "\u{663} few"),
            ("ar", 11.0, "\u{661}\u{661} many"),
            ("ar", 100.0, "\u{661}\u{660}\u{660} other"),
            ("ja", 1.0, "1 other"),
            ("ru", 21.0, "21 one"),
        ];
        for (locale, n, expected) in cases {
            let mut args = FluentArgs::new();
            args.set("n", *n);
            let got = items(locale).format("items", Some(&args));
            assert_eq!(got, *expected, "{locale} {n}");
        }
    }

    #[test]
    fn number_and_date_formats() {
        let numbers: &[(&str, f64, usize, &str)] = &[
            ("en-US", 1234567.891, 2, "1,234,567.89"),
            ("de", 1234567.891, 2, "1.234.567,89"),
            ("de-CH", 1234.5, 1, "1\u{2019}234.5"),
            ("fr", 1234.5, 1, "1\u{202f}234,5"),
            ("es", 1234.0, 0, "1234"),
            ("es", 12345.0, 0, "12.345"),
            ("en-IN", 12345678.0, 0, "1,23,45,678"),
            (
                "ar-EG",
                1234.5,
                1,
                "\u{661}\u{66c}\u{662}\u{663}\u{664}\u{66b}\u{665}",
            ),
            ("en-US", 2.50, 2, "2.5"),
            ("en-US", -0.4, 0, "0"),
            ("en-US", -1234.0, 0, "-1,234"),
        ];
        for (locale, value, max_fraction, expected) in numbers {
            let got = Localizer::for_locales(&[locale]).number(*value, *max_fraction);
            assert_eq!(got, *expected, "{locale} {value}");
        }
        let dates = [
            ("en-US", "10/7/2026"),
            ("en-GB", "07/10/2026"),
            ("de", "7.10.2026"),
            ("ja", "2026/10/07"),
            ("ko", "2026. 10. 7."),
            ("sv", "2026-10-07"),
            ("ar", "\u{667}/\u{661}\u{660}/\u{662}\u{660}\u{662}\u{666}"),
        ];
        for (locale, expected) in dates {
            let got = Localizer::for_locales(&[locale]).date(2026, 10, 7);
            assert_eq!(got, expected, "{locale}");
        }
    }

    #[test]
    fn negotiation_picks_messages_formats_and_direction() {
        let cases = [
            // (requested, quark-find, a formatted number, direction)
            (&["de-AT"][..], "Suchen", "1.234,5", Direction::LeftToRight),
            (
                &["fr-FR", "ja"],
                "検索",
                "1\u{202f}234,5",
                Direction::LeftToRight,
            ),
            (&["fr-FR"], "Find", "1\u{202f}234,5", Direction::LeftToRight),
            (
                &["ar-EG"],
                "بحث",
                "\u{661}\u{66c}\u{662}\u{663}\u{664}\u{66b}\u{665}",
                Direction::RightToLeft,
            ),
            (
                &["de_DE.UTF-8"],
                "Suchen",
                "1.234,5",
                Direction::LeftToRight,
            ),
        ];
        for (requested, find, number, direction) in cases {
            let l = Localizer::for_locales(requested);
            assert_eq!(
                (
                    l.message("quark-find").as_str(),
                    l.number(1234.5, 1).as_str(),
                    l.direction()
                ),
                (find, number, direction),
                "{requested:?}"
            );
        }
    }

    #[test]
    fn app_messages_replace_quark_ones_and_missing_ids_show_themselves() {
        let l = Localizer::builder()
            .requested(["en-US"])
            .messages("en-US", "quark-find = Look up\nmine = Mine")
            .build()
            .expect("valid");
        assert_eq!(
            [
                l.message("quark-find"),
                l.message("mine"),
                l.message("absent")
            ],
            ["Look up", "Mine", "absent"]
        );
        let broken = Localizer::builder().messages("en-US", "= nope").build();
        assert!(matches!(broken, Err(I18nError { ref locale, .. }) if locale == "en-US"));
    }
}
