// SPDX-License-Identifier: MIT OR Apache-2.0

//! Glyphs of short runs already shaped, so text that repeats a word (a
//! terminal's blanks, a log's recurring words) shapes it once.
//!
//! Advanced shaping of a run reads only the run's text, its direction, and
//! the attributes at its start that pick fonts and features (family,
//! stretch, style, weight, font features); harfrust and the fallback walk
//! are deterministic for a given font database. Each glyph's own
//! attributes add its letter spacing and copy its color, weight, metadata,
//! flags, and metrics, so a hit recomputes those from the attributes at its
//! glyph. The memo is keyed by exactly the inputs above, so a hit yields
//! what shaping the run again would.
// Upstream's `shape-run-cache` replaces the memo's lookups.
#![cfg_attr(feature = "shape-run-cache", allow(dead_code))]

#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec::Vec};
use core::hash::{Hash, Hasher};

use crate::{AttrsList, AttrsOwned, FamilyOwned, Feature, ShapeGlyph};

/// Runs kept, one per slot of a direct-mapped table.
const SLOTS: usize = 256;
/// Longest run text kept, in bytes.
const MAX_TEXT: usize = 16;
/// Most glyphs a kept run has.
const MAX_GLYPHS: usize = 16;
/// Most fonts a kept run's glyphs come from.
const MAX_FONTS: usize = 2;
/// Most font features a kept run's attributes set (three turn ligatures
/// off).
const MAX_FEATURES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct MemoFont {
    id: fontdb::ID,
    ascent: f32,
    descent: f32,
    monospace_em_width: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
struct MemoGlyph {
    /// Bytes from the run's start.
    start: u8,
    end: u8,
    /// Index into the slot's fonts.
    font: u8,
    glyph_id: u16,
    /// Without letter spacing.
    x_advance: f32,
    y_advance: f32,
    x_offset: f32,
    y_offset: f32,
}

const NO_GLYPH: MemoGlyph = MemoGlyph {
    start: 0,
    end: 0,
    font: 0,
    glyph_id: 0,
    x_advance: 0.0,
    y_advance: 0.0,
    x_offset: 0.0,
    y_offset: 0.0,
};

const NO_FEATURE: Feature = Feature {
    tag: crate::FeatureTag::new(b"    "),
    value: 0,
};

#[derive(Debug)]
struct Slot {
    hash: u64,
    used: bool,
    rtl: bool,
    text_len: u8,
    text: [u8; MAX_TEXT],
    family: FamilyOwned,
    stretch: fontdb::Stretch,
    style: fontdb::Style,
    weight: fontdb::Weight,
    feature_count: u8,
    features: [Feature; MAX_FEATURES],
    font_count: u8,
    fonts: [MemoFont; MAX_FONTS],
    glyph_count: u8,
    glyphs: [MemoGlyph; MAX_GLYPHS],
}

impl Slot {
    fn empty() -> Self {
        let no_font = MemoFont {
            id: fontdb::ID::dummy(),
            ascent: 0.0,
            descent: 0.0,
            monospace_em_width: None,
        };
        Self {
            hash: 0,
            used: false,
            rtl: false,
            text_len: 0,
            text: [0; MAX_TEXT],
            family: FamilyOwned::Monospace,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
            weight: fontdb::Weight::NORMAL,
            feature_count: 0,
            features: [NO_FEATURE; MAX_FEATURES],
            font_count: 0,
            fonts: [no_font; MAX_FONTS],
            glyph_count: 0,
            glyphs: [NO_GLYPH; MAX_GLYPHS],
        }
    }

    fn holds(&self, hash: u64, text: &str, attrs: &AttrsOwned, rtl: bool) -> bool {
        self.used
            && self.hash == hash
            && self.rtl == rtl
            && self.text[..usize::from(self.text_len)] == *text.as_bytes()
            && self.family == attrs.family_owned
            && self.stretch == attrs.stretch
            && self.style == attrs.style
            && self.weight == attrs.weight
            && self.features[..usize::from(self.feature_count)] == *attrs.font_features.features
    }
}

/// The memo of shaped runs a [`ShapeBuffer`](crate::ShapeBuffer) keeps.
#[derive(Debug)]
pub(crate) struct RunMemo {
    /// Allocated on first use, all at once, so a run kept later allocates
    /// nothing.
    slots: Option<Box<[Slot]>>,
    enabled: bool,
}

impl Default for RunMemo {
    fn default() -> Self {
        Self {
            slots: None,
            enabled: true,
        }
    }
}

/// The memo's key hash, or `None` when the run is too long or its
/// attributes set too many features to keep.
fn key_hash(text: &str, attrs: &AttrsOwned, rtl: bool) -> Option<u64> {
    if text.len() > MAX_TEXT || attrs.font_features.features.len() > MAX_FEATURES {
        return None;
    }
    let mut hasher = rustc_hash::FxHasher::default();
    text.hash(&mut hasher);
    attrs.family_owned.hash(&mut hasher);
    attrs.stretch.hash(&mut hasher);
    attrs.style.hash(&mut hasher);
    attrs.weight.hash(&mut hasher);
    attrs.font_features.features.hash(&mut hasher);
    rtl.hash(&mut hasher);
    Some(hasher.finish())
}

fn slot_index(hash: u64) -> usize {
    // FxHash mixes into the high bits.
    (hash >> 56) as usize % SLOTS
}

impl RunMemo {
    /// Turns the memo on or off, forgetting every run.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.clear();
    }

    /// Forgets every run, as when the fonts change.
    pub(crate) fn clear(&mut self) {
        if let Some(slots) = &mut self.slots {
            for slot in slots.iter_mut() {
                slot.used = false;
            }
        }
    }

    /// Appends the glyphs of `line[start..end]` shaped with `attrs_list` to
    /// `glyphs` when the memo has them, returning whether it did.
    pub(crate) fn get(
        &self,
        glyphs: &mut Vec<ShapeGlyph>,
        line: &str,
        attrs_list: &AttrsList,
        start: usize,
        end: usize,
        rtl: bool,
    ) -> bool {
        let Some(slots) = self.slots.as_deref().filter(|_| self.enabled) else {
            return false;
        };
        let text = &line[start..end];
        let attrs = attrs_list.get_span_ref(start);
        let Some(hash) = key_hash(text, attrs, rtl) else {
            return false;
        };
        let slot = &slots[slot_index(hash)];
        if !slot.holds(hash, text, attrs, rtl) {
            return false;
        }
        glyphs.reserve(usize::from(slot.glyph_count));
        for g in &slot.glyphs[..usize::from(slot.glyph_count)] {
            let font = &slot.fonts[usize::from(g.font)];
            let glyph_start = start + usize::from(g.start);
            // What shaping copies from the attributes at each glyph.
            let attrs = attrs_list.get_span_ref(glyph_start);
            glyphs.push(ShapeGlyph {
                start: glyph_start,
                end: start + usize::from(g.end),
                x_advance: g.x_advance + attrs.letter_spacing_opt.map_or(0.0, |spacing| spacing.0),
                y_advance: g.y_advance,
                x_offset: g.x_offset,
                y_offset: g.y_offset,
                ascent: font.ascent,
                descent: font.descent,
                font_monospace_em_width: font.monospace_em_width,
                font_id: font.id,
                font_weight: attrs.weight,
                glyph_id: g.glyph_id,
                color_opt: attrs.color_opt,
                metadata: attrs.metadata,
                cache_key_flags: attrs.cache_key_flags,
                metrics_opt: attrs.metrics_opt.map(Into::into),
            });
        }
        true
    }

    /// Keeps `shaped`, the glyphs of `line[start..end]` just shaped with
    /// `attrs_list`, when they fit a slot.
    pub(crate) fn insert(
        &mut self,
        shaped: &[ShapeGlyph],
        line: &str,
        attrs_list: &AttrsList,
        start: usize,
        end: usize,
        rtl: bool,
    ) {
        if !self.enabled || shaped.len() > MAX_GLYPHS {
            return;
        }
        let text = &line[start..end];
        let attrs = attrs_list.get_span_ref(start);
        let Some(hash) = key_hash(text, attrs, rtl) else {
            return;
        };
        // A glyph's advance includes its letter spacing, which a later run
        // of the same text may not have; only runs without any keep the
        // bare advance. (Shaping adds `0.0` without spacing, which leaves
        // an advance from integer font units unchanged.)
        if shaped.iter().any(|g| {
            attrs_list
                .get_span_ref(g.start)
                .letter_spacing_opt
                .is_some()
        }) {
            return;
        }
        let slots = self
            .slots
            .get_or_insert_with(|| (0..SLOTS).map(|_| Slot::empty()).collect());
        let slot = &mut slots[slot_index(hash)];
        slot.used = false;
        slot.font_count = 0;
        for (i, g) in shaped.iter().enumerate() {
            let font = MemoFont {
                id: g.font_id,
                ascent: g.ascent,
                descent: g.descent,
                monospace_em_width: g.font_monospace_em_width,
            };
            let fonts = &mut slot.fonts[..usize::from(slot.font_count)];
            let font_index = match fonts.iter().position(|f| *f == font) {
                Some(i) => i,
                None if fonts.len() < MAX_FONTS => {
                    slot.fonts[fonts.len()] = font;
                    slot.font_count += 1;
                    usize::from(slot.font_count) - 1
                }
                None => return,
            };
            slot.glyphs[i] = MemoGlyph {
                // Within the run, which is at most MAX_TEXT bytes.
                start: (g.start - start) as u8,
                end: (g.end - start) as u8,
                font: font_index as u8,
                glyph_id: g.glyph_id,
                x_advance: g.x_advance,
                y_advance: g.y_advance,
                x_offset: g.x_offset,
                y_offset: g.y_offset,
            };
        }
        slot.glyph_count = shaped.len() as u8;
        slot.hash = hash;
        slot.rtl = rtl;
        slot.text_len = text.len() as u8;
        slot.text[..text.len()].copy_from_slice(text.as_bytes());
        slot.family.clone_from(&attrs.family_owned);
        slot.stretch = attrs.stretch;
        slot.style = attrs.style;
        slot.weight = attrs.weight;
        let features = &attrs.font_features.features;
        slot.feature_count = features.len() as u8;
        slot.features[..features.len()].copy_from_slice(features);
        slot.used = true;
    }
}
