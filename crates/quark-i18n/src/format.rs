//! Locale-aware number and date formatting from a small table of CLDR
//! conventions (separators, grouping, digits, numeric date order).
//!
//! Full CLDR data through icu4x costs megabytes of compiled data for
//! dates alone; this table covers the conventions of the most used
//! locales in a few kilobytes. A locale it does not know formats like
//! English.

use unic_langid::LanguageIdentifier;

/// Digits a locale writes numbers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Digits {
    Latin,
    /// Arabic-Indic (U+0660..).
    Arabic,
    /// Extended Arabic-Indic, as in Persian (U+06F0..).
    Persian,
}

impl Digits {
    fn map(self, d: char) -> char {
        let zero = match self {
            Self::Latin => return d,
            Self::Arabic => 0x0660,
            Self::Persian => 0x06F0,
        };
        d.to_digit(10)
            .and_then(|n| char::from_u32(zero + n))
            .unwrap_or(d)
    }
}

/// How a locale writes numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberFormat {
    decimal: char,
    group: char,
    /// Digits in the group nearest the decimal point, then in each group
    /// before it (3 and 2 in India).
    primary: usize,
    secondary: usize,
    /// Integers with fewer digits than `primary + min_grouping` are not
    /// grouped (Spanish and Polish write 1234 but 12 345).
    min_grouping: usize,
    digits: Digits,
}

impl NumberFormat {
    pub fn for_locale(locale: &LanguageIdentifier) -> Self {
        let language = locale.language.as_str();
        let region = locale.region.as_ref().map(|r| r.as_str());
        let mut f = Self {
            decimal: '.',
            group: ',',
            primary: 3,
            secondary: 3,
            min_grouping: 1,
            digits: Digits::Latin,
        };
        match (language, region) {
            ("de", Some("CH" | "LI")) => (f.decimal, f.group) = ('.', '\u{2019}'),
            ("en" | "hi" | "bn" | "mr" | "ta" | "te", Some("IN")) | ("hi" | "mr", _) => {
                f.secondary = 2;
            }
            ("fr", Some("CH")) => (f.decimal, f.group) = (',', '\u{202f}'),
            ("fr", _) => (f.decimal, f.group) = (',', '\u{202f}'),
            ("es" | "pl", _) => {
                f.decimal = ',';
                f.group = if language == "pl" { '\u{a0}' } else { '.' };
                f.min_grouping = 2;
            }
            ("pt", Some("PT")) => {
                (f.decimal, f.group) = (',', '\u{a0}');
                f.min_grouping = 2;
            }
            (
                "de" | "it" | "nl" | "pt" | "id" | "tr" | "da" | "el" | "ro" | "hr" | "sl" | "sr"
                | "vi",
                _,
            ) => (f.decimal, f.group) = (',', '.'),
            (
                "ru" | "uk" | "be" | "cs" | "sk" | "sv" | "fi" | "nb" | "no" | "nn" | "hu" | "bg"
                | "lt" | "lv" | "et",
                _,
            ) => (f.decimal, f.group) = (',', '\u{a0}'),
            ("ar", Some("MA" | "DZ" | "TN" | "LY" | "EH")) => (f.decimal, f.group) = (',', '.'),
            ("ar", _) => {
                (f.decimal, f.group) = ('\u{66b}', '\u{66c}');
                f.digits = Digits::Arabic;
            }
            ("fa", _) => {
                (f.decimal, f.group) = ('\u{66b}', '\u{66c}');
                f.digits = Digits::Persian;
            }
            _ => {}
        }
        f
    }

    /// `value` with between `min_fraction` and `max_fraction` fraction
    /// digits (trailing zeros past the minimum dropped), rounded half away
    /// from zero.
    pub fn format(&self, value: f64, min_fraction: usize, max_fraction: usize) -> String {
        if value.is_nan() {
            return "NaN".to_owned();
        }
        let negative = value.is_sign_negative();
        if value.is_infinite() {
            return if negative { "-\u{221e}" } else { "\u{221e}" }.to_owned();
        }
        let max_fraction = max_fraction.max(min_fraction).min(15);
        let fixed = format!("{:.*}", max_fraction, value.abs());
        // No sign on a value that rounds to zero.
        let sign = if negative && fixed.bytes().any(|b| matches!(b, b'1'..=b'9')) {
            "-"
        } else {
            ""
        };
        let (int, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
        let mut keep = fraction.len();
        while keep > min_fraction && fraction.as_bytes().get(keep - 1) == Some(&b'0') {
            keep -= 1;
        }
        let fraction = fraction.get(..keep).unwrap_or_default();
        let mut out = String::with_capacity(fixed.len() + 4);
        out.push_str(sign);
        self.push_grouped(&mut out, int);
        if !fraction.is_empty() {
            out.push(self.decimal);
            out.extend(fraction.chars().map(|d| self.digits.map(d)));
        }
        out
    }

    /// An integer, grouped.
    pub fn integer(&self, value: i64) -> String {
        self.format(value as f64, 0, 0)
    }

    fn push_grouped(&self, out: &mut String, int: &str) {
        let len = int.len();
        let grouped = len >= self.primary + self.min_grouping;
        for (i, d) in int.chars().enumerate() {
            let left = len - i;
            if grouped && i > 0 {
                let at_primary = left == self.primary;
                let at_secondary =
                    left > self.primary && (left - self.primary) % self.secondary == 0;
                if at_primary || at_secondary {
                    out.push(self.group);
                }
            }
            out.push(self.digits.map(d));
        }
    }

    pub(crate) fn digit(&self, d: char) -> char {
        self.digits.map(d)
    }
}

/// The order of a numeric date's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Dmy,
    Mdy,
    Ymd,
}

/// How a locale writes a short numeric date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateFormat {
    order: Order,
    separator: &'static str,
    /// Written after the last field too (Korean, Hungarian).
    trailing: bool,
    /// Days and months below 10 get a leading zero.
    pad: bool,
    number: NumberFormat,
}

impl DateFormat {
    pub fn for_locale(locale: &LanguageIdentifier) -> Self {
        let language = locale.language.as_str();
        let region = locale.region.as_ref().map(|r| r.as_str());
        let (order, separator, trailing, pad) = match (language, region) {
            ("en", Some("US" | "PH") | None) => (Order::Mdy, "/", false, false),
            ("en", Some("CA")) | ("sv" | "lt", _) => (Order::Ymd, "-", false, true),
            ("en", _) | ("fr" | "it" | "pt" | "el" | "vi", _) => (Order::Dmy, "/", false, true),
            ("es" | "ar" | "fa", _) => (Order::Dmy, "/", false, false),
            ("de" | "fi" | "cs" | "sk", _) => (Order::Dmy, ".", false, false),
            ("ru" | "uk" | "be" | "pl" | "tr" | "nb" | "no" | "nn" | "da" | "ro" | "bg", _) => {
                (Order::Dmy, ".", false, true)
            }
            ("nl", _) => (Order::Dmy, "-", false, false),
            ("ja" | "zh", _) => (Order::Ymd, "/", false, language == "ja"),
            ("ko", _) => (Order::Ymd, ". ", true, false),
            ("hu", _) => (Order::Ymd, ". ", true, true),
            _ => (Order::Ymd, "-", false, true),
        };
        Self {
            order,
            separator,
            trailing,
            pad,
            number: NumberFormat::for_locale(locale),
        }
    }

    /// `year`, `month` (1-12), and `day` as a short numeric date.
    pub fn format(&self, year: i32, month: u8, day: u8) -> String {
        let field = |n: u32, pad: bool| {
            let text = if pad {
                format!("{n:02}")
            } else {
                n.to_string()
            };
            text.chars()
                .map(|d| self.number.digit(d))
                .collect::<String>()
        };
        let y = field(year.unsigned_abs(), false);
        let m = field(u32::from(month), self.pad);
        let d = field(u32::from(day), self.pad);
        let fields = match self.order {
            Order::Dmy => [d, m, y],
            Order::Mdy => [m, d, y],
            Order::Ymd => [y, m, d],
        };
        let mut out = fields.join(self.separator);
        if self.trailing {
            out.push_str(self.separator.trim_end());
        }
        out
    }
}
