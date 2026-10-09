// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::{mem, ops::Range};
use fontdb::Family;
use unicode_script::Script;

use crate::{BuildHasher, Font, FontMatchKey, FontSystem, HashMap, ShapeBuffer};

#[cfg(not(any(all(unix, not(target_os = "android")), target_os = "windows")))]
#[path = "other.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;

#[cfg(all(unix, not(any(target_os = "android", target_os = "macos"))))]
#[path = "unix.rs"]
mod platform;

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

/// The `Fallback` trait allows for configurable font fallback lists to be set during construction of the [`FontSystem`].
///
/// A custom fallback list can be added via the [`FontSystem::new_with_locale_and_db_and_fallback`] constructor.
///
/// A default implementation is provided by the [`PlatformFallback`] struct, which encapsulates the target platform's pre-configured fallback lists.
///
/// ```rust
/// # use unicode_script::Script;
/// # use cosmic_text::{Fallback, FontSystem};
/// struct MyFallback;
/// impl Fallback for MyFallback {
///     fn common_fallback(&self) -> &[&'static str] {
///         &[
///             "Segoe UI",
///             "Segoe UI Emoji",
///             "Segoe UI Symbol",
///             "Segoe UI Historic",
///         ]
///     }
///
///     fn forbidden_fallback(&self) -> &[&'static str] {
///         &[]
///     }
///
///     fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
///         match script {
///             Script::Adlam => &["Ebrima"],
///             Script::Bengali => &["Nirmala UI"],
///             Script::Canadian_Aboriginal => &["Gadugi"],
///             // ...
///             _ => &[],
///        }
///     }
/// }
///
/// let locale = "en-US".to_string();
/// let db = fontdb::Database::new();
/// let font_system = FontSystem::new_with_locale_and_db_and_fallback(locale, db, MyFallback);
/// ```
pub trait Fallback: Send + Sync {
    /// Fallbacks to use after any script specific fallbacks
    fn common_fallback(&self) -> &[&'static str];

    /// Fallbacks to never use
    fn forbidden_fallback(&self) -> &[&'static str];

    /// Fallbacks to use per script
    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str];
}

#[derive(Debug, Default)]
pub struct Fallbacks {
    lists: Vec<&'static str>,
    common_fallback_range: Range<usize>,
    forbidden_fallback_range: Range<usize>,
    // PERF: Consider using NoHashHasher since Script is just an integer
    script_fallback_ranges: HashMap<Script, Range<usize>>,
    locale: String,
}

impl Fallbacks {
    pub(crate) fn new(fallbacks: &dyn Fallback, scripts: &[Script], locale: &str) -> Self {
        let common_fallback = fallbacks.common_fallback();

        let forbidden_fallback = fallbacks.forbidden_fallback();

        let mut lists =
            Vec::with_capacity(common_fallback.len() + forbidden_fallback.len() + scripts.len());

        let mut index = lists.len();
        let mut new_range = |lists: &Vec<&str>| {
            let old_index = index;
            index = lists.len();
            old_index..index
        };

        lists.extend_from_slice(common_fallback);
        let common_fallback_range = new_range(&lists);

        lists.extend_from_slice(forbidden_fallback);
        let forbidden_fallback_range = new_range(&lists);

        let mut script_fallback_ranges =
            HashMap::with_capacity_and_hasher(scripts.len(), BuildHasher::default());
        for &script in scripts {
            let script_fallback = fallbacks.script_fallback(script, locale);
            lists.extend_from_slice(script_fallback);
            let script_fallback_range = new_range(&lists);
            script_fallback_ranges.insert(script, script_fallback_range);
        }

        let locale = locale.to_owned();
        Self {
            lists,
            common_fallback_range,
            forbidden_fallback_range,
            script_fallback_ranges,
            locale,
        }
    }

    pub(crate) fn extend(&mut self, fallbacks: &dyn Fallback, scripts: &[Script]) {
        self.lists.reserve(scripts.len());

        let mut index = self.lists.len();
        let mut new_range = |lists: &Vec<&str>| {
            let old_index = index;
            index = lists.len();
            old_index..index
        };

        for &script in scripts {
            self.script_fallback_ranges
                .entry(script)
                .or_insert_with_key(|&script| {
                    let script_fallback = fallbacks.script_fallback(script, &self.locale);
                    self.lists.extend_from_slice(script_fallback);
                    new_range(&self.lists)
                });
        }
    }

    pub(crate) fn common_fallback(&self) -> &[&'static str] {
        &self.lists[self.common_fallback_range.clone()]
    }

    pub(crate) fn forbidden_fallback(&self) -> &[&'static str] {
        &self.lists[self.forbidden_fallback_range.clone()]
    }

    pub(crate) fn script_fallback(&self, script: Script) -> &[&'static str] {
        self.script_fallback_ranges
            .get(&script)
            .map_or(&[], |range| &self.lists[range.clone()])
    }
}

pub use platform::PlatformFallback;

#[cfg(not(feature = "warn_on_missing_glyphs"))]
use log::debug as missing_warn;
#[cfg(feature = "warn_on_missing_glyphs")]
use log::warn as missing_warn;

// Match on lowest font_weight_diff, then script_non_matches, then font_weight
// Default font gets None for both `weight_offset` and `script_non_matches`, and thus, it is
// always the first to be popped from the set.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonospaceFallbackInfo {
    font_weight_diff: Option<u16>,
    codepoint_non_matches: Option<usize>,
    font_weight: u16,
    id: fontdb::ID,
}

/// The monospace fallback candidates of a word, popped smallest first. A
/// vector kept sorted largest first, so it keeps its storage from word to
/// word where a `BTreeSet` would free and allocate its node.
#[derive(Debug, Default)]
pub(crate) struct MonospaceFallbacks(Vec<MonospaceFallbackInfo>);

impl MonospaceFallbacks {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// Adds `info`, returning whether it was not already present.
    pub(crate) fn insert(&mut self, info: MonospaceFallbackInfo) -> bool {
        match self.0.binary_search_by(|probe| info.cmp(probe)) {
            Ok(_) => false,
            Err(i) => {
                self.0.insert(i, info);
                true
            }
        }
    }

    pub(crate) fn pop_first(&mut self) -> Option<MonospaceFallbackInfo> {
        self.0.pop()
    }
}

#[derive(Debug)]
pub struct FontFallbackIter<'a> {
    font_system: &'a mut FontSystem,
    font_match_keys: &'a [FontMatchKey],
    default_families: &'a [&'a Family<'a>],
    default_i: usize,
    scripts: &'a [Script],
    word: &'a str,
    script_i: (usize, usize),
    common_i: usize,
    other_i: usize,
    end: bool,
    ideal_weight: fontdb::Weight,
    /// The default monospace family's font, already returned before the
    /// other monospace candidates were collected. The candidates are
    /// collected when a next font is asked for, as most words never ask.
    deferred_mono: Option<FontMatchKey>,
    /// Collects the monospace candidates before returning the default
    /// monospace font, as upstream does, for tests that compare the two.
    #[cfg(test)]
    eager_mono: bool,
}

impl<'a> FontFallbackIter<'a> {
    pub fn new(
        font_system: &'a mut FontSystem,
        font_match_keys: &'a [FontMatchKey],
        default_families: &'a [&'a Family<'a>],
        scripts: &'a [Script],
        word: &'a str,
        ideal_weight: fontdb::Weight,
    ) -> Self {
        font_system
            .fallbacks
            .extend(font_system.dyn_fallback.as_ref(), scripts);
        font_system.monospace_fallbacks_buffer.clear();
        Self {
            font_system,
            font_match_keys,
            default_families,
            default_i: 0,
            scripts,
            word,
            script_i: (0, 0),
            common_i: 0,
            other_i: 0,
            end: false,
            ideal_weight,
            deferred_mono: None,
            #[cfg(test)]
            eager_mono: false,
        }
    }

    pub fn check_missing(&self, word: &str) {
        if self.end {
            missing_warn!(
                "Failed to find any fallback for {:?} locale '{}': '{}'",
                self.scripts,
                self.font_system.locale(),
                word
            );
        } else if self.other_i > 0 {
            missing_warn!(
                "Failed to find preset fallback for {:?} locale '{}', used '{}': '{}'",
                self.scripts,
                self.font_system.locale(),
                self.face_name(self.font_match_keys[self.other_i - 1].id),
                word
            );
        } else if !self.scripts.is_empty() && self.common_i > 0 {
            let family = self.font_system.fallbacks.common_fallback()[self.common_i - 1];
            missing_warn!(
                "Failed to find script fallback for {:?} locale '{}', used '{}': '{}'",
                self.scripts,
                self.font_system.locale(),
                family,
                word
            );
        }
    }

    pub fn face_name(&self, id: fontdb::ID) -> &str {
        self.font_system
            .db()
            .face(id)
            .map_or("invalid font id", |face| {
                if let Some((name, _)) = face.families.first() {
                    name
                } else {
                    &face.post_script_name
                }
            })
    }

    pub fn shape_caches(&mut self) -> &mut ShapeBuffer {
        &mut self.font_system.shape_buffer
    }

    fn face_contains_family(&self, id: fontdb::ID, family_name: &str) -> bool {
        self.font_system
            .db()
            .face(id)
            .is_some_and(|face| face.families.iter().any(|(name, _)| name == family_name))
    }

    fn default_font_match_key(&self) -> Option<&FontMatchKey> {
        let default_family = self.default_families[self.default_i - 1];
        let default_family_name = self.font_system.db().family_name(default_family);
        self.family_match_key(default_family_name)
    }

    /// The face of `family_name` nearest the ideal weight: the family
    /// comes before the weight, as in CSS font matching, so a family with
    /// fewer weights than asked for (one variable face, a single-weight
    /// fallback) is taken at its nearest one rather than skipped for
    /// another family of the exact weight. The keys sort by weight
    /// difference, except that the first is fontdb's CSS match for the
    /// default family, which also settles ties toward the CSS side.
    fn family_match_key(&self, family_name: &str) -> Option<&FontMatchKey> {
        let in_family = |m_key: &&FontMatchKey| self.face_contains_family(m_key.id, family_name);
        self.font_match_keys
            .first()
            .filter(in_family)
            .or_else(|| {
                self.font_match_keys
                    .iter()
                    .filter(in_family)
                    .min_by_key(|m_key| m_key.font_weight_diff)
            })
    }

    /// How many of the word's chars font `id` lacks, or `None` when the
    /// font does not load.
    fn codepoint_non_matches(&mut self, id: fontdb::ID) -> Option<usize> {
        let supported = self.font_system.get_font_supported_codepoints_in_word(
            id,
            self.ideal_weight,
            self.word,
        )?;
        Some(self.word.chars().count() - supported)
    }

    /// Queues every monospace font but the default (`default_id`) as a
    /// fallback candidate, fewest missing chars first.
    fn queue_mono_candidates(&mut self, default_id: Option<fontdb::ID>) {
        let mono_ids_for_scripts = if self.scripts.is_empty() {
            Vec::new()
        } else {
            let scripts = self.scripts.iter().filter_map(|script| {
                let script_as_lower = script.short_name().to_lowercase();
                <[u8; 4]>::try_from(script_as_lower.as_bytes()).ok()
            });
            self.font_system.get_monospace_ids_for_scripts(scripts)
        };

        // Every weight: a monospace face of another weight beats a
        // proportional one.
        for m_key in self.font_match_keys {
            if Some(m_key.id) == default_id {
                continue;
            }
            let is_mono_id = if mono_ids_for_scripts.is_empty() {
                self.font_system.is_monospace(m_key.id)
            } else {
                mono_ids_for_scripts.binary_search(&m_key.id).is_ok()
            };
            if is_mono_id {
                if let Some(codepoint_non_matches) = self.codepoint_non_matches(m_key.id) {
                    let fallback_info = MonospaceFallbackInfo {
                        font_weight_diff: Some(m_key.font_weight_diff),
                        codepoint_non_matches: Some(codepoint_non_matches),
                        font_weight: m_key.font_weight,
                        id: m_key.id,
                    };
                    assert!(self
                        .font_system
                        .monospace_fallbacks_buffer
                        .insert(fallback_info));
                }
            }
        }
    }

    fn next_item(&mut self, fallbacks: &Fallbacks) -> Option<<Self as Iterator>::Item> {
        // The default monospace font went out first. Unless it has every
        // char of the word, the other monospace fonts come next, as they
        // would have had they been queued with it.
        if let Some(default) = self.deferred_mono.take() {
            if self.codepoint_non_matches(default.id) != Some(0) {
                self.queue_mono_candidates(Some(default.id));
            }
        }

        if let Some(fallback_info) = self.font_system.monospace_fallbacks_buffer.pop_first() {
            if let Some(font) = self
                .font_system
                .get_font(fallback_info.id, self.ideal_weight)
            {
                return Some(font);
            }
        }

        'DEF_FAM: while self.default_i < self.default_families.len() {
            self.default_i += 1;
            let is_mono = self.default_families[self.default_i - 1] == &Family::Monospace;
            let default_font_match_key = self.default_font_match_key().copied();

            match (is_mono, default_font_match_key.as_ref()) {
                (false, None) => break 'DEF_FAM,
                (false, Some(m_key)) => {
                    if let Some(font) = self.font_system.get_font(m_key.id, self.ideal_weight) {
                        return Some(font);
                    }
                    break 'DEF_FAM;
                }
                (true, None) => (),
                (true, Some(m_key)) => {
                    // The default monospace font, when it loads, sorts
                    // before every candidate (it has no weight difference),
                    // so it comes first whatever they are; they are only
                    // collected when a next font is asked for.
                    #[cfg(test)]
                    let deferred = !self.eager_mono;
                    #[cfg(not(test))]
                    let deferred = true;
                    if deferred {
                        if let Some(font) = self.font_system.get_font(m_key.id, self.ideal_weight) {
                            self.deferred_mono = Some(*m_key);
                            return Some(font);
                        }
                    } else if let Some(codepoint_non_matches) = self.codepoint_non_matches(m_key.id)
                    {
                        // Return early if default Monospace font supports all word codepoints.
                        // Otherewise, add to fallbacks set
                        if codepoint_non_matches == 0 {
                            if let Some(font) =
                                self.font_system.get_font(m_key.id, self.ideal_weight)
                            {
                                return Some(font);
                            }
                        } else {
                            assert!(self.font_system.monospace_fallbacks_buffer.insert(
                                MonospaceFallbackInfo {
                                    font_weight_diff: None,
                                    codepoint_non_matches: Some(codepoint_non_matches),
                                    font_weight: m_key.font_weight,
                                    id: m_key.id,
                                }
                            ));
                        }
                    }
                }
            }

            // Only a monospace default family gets here.
            self.queue_mono_candidates(default_font_match_key.map(|m_key| m_key.id));
            // If default family is Monospace fallback to first monospaced font
            if let Some(fallback_info) = self.font_system.monospace_fallbacks_buffer.pop_first() {
                if let Some(font) = self
                    .font_system
                    .get_font(fallback_info.id, self.ideal_weight)
                {
                    return Some(font);
                }
            }
        }

        while self.script_i.0 < self.scripts.len() {
            let script = self.scripts[self.script_i.0];

            let script_families = fallbacks.script_fallback(script);

            while self.script_i.1 < script_families.len() {
                let script_family = script_families[self.script_i.1];
                self.script_i.1 += 1;
                if let Some(m_key) = self.family_match_key(script_family).copied() {
                    if let Some(font) = self.font_system.get_font(m_key.id, self.ideal_weight) {
                        return Some(font);
                    }
                }
                log::debug!(
                    "failed to find family '{}' for script {:?} and locale '{}'",
                    script_family,
                    script,
                    self.font_system.locale(),
                );
            }

            self.script_i.0 += 1;
            self.script_i.1 = 0;
        }

        let common_families = fallbacks.common_fallback();
        while self.common_i < common_families.len() {
            let common_family = common_families[self.common_i];
            self.common_i += 1;
            if let Some(m_key) = self.family_match_key(common_family).copied() {
                if let Some(font) = self.font_system.get_font(m_key.id, self.ideal_weight) {
                    return Some(font);
                }
            }
            log::debug!("failed to find family '{common_family}'");
        }

        //TODO: do we need to do this?
        //TODO: do not evaluate fonts more than once!
        let forbidden_families = fallbacks.forbidden_fallback();
        while self.other_i < self.font_match_keys.len() {
            let id = self.font_match_keys[self.other_i].id;
            self.other_i += 1;
            if forbidden_families
                .iter()
                .all(|family_name| !self.face_contains_family(id, family_name))
            {
                if let Some(font) = self.font_system.get_font(id, self.ideal_weight) {
                    return Some(font);
                }
            }
        }

        self.end = true;
        None
    }
}

impl Iterator for FontFallbackIter<'_> {
    type Item = Arc<Font>;
    fn next(&mut self) -> Option<Self::Item> {
        let mut fallbacks = mem::take(&mut self.font_system.fallbacks);
        let item = self.next_item(&fallbacks);
        mem::swap(&mut fallbacks, &mut self.font_system.fallbacks);
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, Weight};

    /// Two weights of a monospace family, other monospace families (one
    /// variable), and a proportional family, from quark-text's assets.
    fn font_system() -> FontSystem {
        const FONTS: [&[u8]; 6] = [
            include_bytes!("../../../../../crates/quark-text/assets/fonts/GeistMono-Regular.otf"),
            include_bytes!("../../../../../crates/quark-text/assets/fonts/GeistMono-Bold.otf"),
            include_bytes!(
                "../../../../../crates/quark-text/assets/fonts/JetBrainsMono-Variable.ttf"
            ),
            include_bytes!("../../../../../crates/quark-text/assets/fonts/FiraCode-Variable.ttf"),
            include_bytes!("../../../../../crates/quark-text/assets/fonts/IBMPlexMono-Regular.ttf"),
            include_bytes!("../../../../../crates/quark-text/assets/fonts/Geist-Regular.otf"),
        ];
        let mut db = fontdb::Database::new();
        for font in FONTS {
            db.load_font_data(font.to_vec());
        }
        db.set_monospace_family("Geist Mono");
        db.set_sans_serif_family("Geist");
        FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, PlatformFallback)
    }

    /// Every font `word` falls back through, in order.
    fn fallback_order(
        fs: &mut FontSystem,
        family: Family,
        weight: u16,
        word: &str,
        eager: bool,
    ) -> Vec<fontdb::ID> {
        let attrs = Attrs::new().family(family).weight(Weight(weight));
        let mut scripts = Vec::new();
        crate::shape::collect_scripts(word, &mut scripts);
        let fonts = fs.get_font_matches(&attrs);
        let families = [&attrs.family];
        let mut iter = FontFallbackIter::new(fs, &fonts, &families, &scripts, word, attrs.weight);
        iter.eager_mono = eager;
        iter.map(|font| font.id()).collect()
    }

    // The default monospace font goes out before the other monospace
    // candidates are collected. Every font after it must come in the order
    // collecting them first gave, for words the default font covers and
    // words it lacks, at its own weights and others.
    #[test]
    fn deferred_monospace_candidates_keep_the_fallback_order() {
        let mut fs = font_system();
        let words = [
            "abc",
            " ",
            "a\u{3b1}",
            "\u{65e5}\u{672c}",
            "a\u{2603}",
            "\u{1f600}",
            "e\u{301}",
            "\u{5e9}\u{5dc}",
        ];
        for word in words {
            for weight in [300, 400, 700] {
                for family in [Family::Monospace, Family::SansSerif] {
                    let deferred = fallback_order(&mut fs, family, weight, word, false);
                    let eager = fallback_order(&mut fs, family, weight, word, true);
                    assert!(deferred.len() > 1, "{word:?} {weight} {family:?}");
                    assert_eq!(deferred, eager, "{word:?} {weight} {family:?}");
                }
            }
        }
    }
}
