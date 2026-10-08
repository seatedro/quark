// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::too_many_arguments)]

use crate::fallback::FontFallbackIter;
use crate::{
    math, Align, AttrsList, CacheKeyFlags, Color, Font, FontSystem, LayoutGlyph, LayoutLine,
    Metrics, Wrap,
};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use alloc::collections::VecDeque;
use core::cmp::{max, min};
use core::fmt;
use core::mem;
use core::ops::Range;

#[cfg(not(feature = "std"))]
use core_maths::CoreFloat;
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

/// The shaping strategy of some text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shaping {
    /// Basic shaping with no font fallback.
    ///
    /// This shaping strategy is very cheap, but it will not display complex
    /// scripts properly nor try to find missing glyphs in your system fonts.
    ///
    /// You should use this strategy when you have complete control of the text
    /// and the font you are displaying in your application.
    #[cfg(feature = "swash")]
    Basic,
    /// Advanced text shaping and font fallback.
    ///
    /// You will need to enable this strategy if the text contains a complex
    /// script, the font used needs it, and/or multiple fonts in your system
    /// may be needed to display all of the glyphs.
    Advanced,
}

impl Shaping {
    fn run(
        self,
        glyphs: &mut Vec<ShapeGlyph>,
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        start_run: usize,
        end_run: usize,
        span_rtl: bool,
    ) {
        match self {
            #[cfg(feature = "swash")]
            Self::Basic => shape_skip(font_system, glyphs, line, attrs_list, start_run, end_run),
            #[cfg(not(feature = "shape-run-cache"))]
            Self::Advanced => shape_run(
                glyphs,
                font_system,
                line,
                attrs_list,
                start_run,
                end_run,
                span_rtl,
            ),
            #[cfg(feature = "shape-run-cache")]
            Self::Advanced => shape_run_cached(
                glyphs,
                font_system,
                line,
                attrs_list,
                start_run,
                end_run,
                span_rtl,
            ),
        }
    }
}

/// How many harfrust shape plans [`ShapeBuffer`] keeps by default.
///
/// Font fallback shapes a run with each candidate font until its glyphs are
/// found, so text in a script no font covers needs a plan for every font in
/// the fallback chain. A cache smaller than that working set rebuilds every
/// plan on every run. A plan is about 2 KB.
pub const DEFAULT_SHAPE_PLAN_CAPACITY: usize = 128;

/// A set of buffers containing allocations for shaped text.
pub struct ShapeBuffer {
    /// Cache for harfrust shape plans, least recently used first. Stores up to
    /// `shape_plan_capacity` plans; inserting one past that removes the least
    /// recently used.
    shape_plan_cache: VecDeque<(fontdb::ID, harfrust::ShapePlan)>,

    /// How many plans `shape_plan_cache` keeps.
    shape_plan_capacity: usize,

    /// Buffer for holding unicode text.
    harfrust_buffer: Option<harfrust::UnicodeBuffer>,

    /// Temporary buffers for scripts.
    scripts: Vec<Script>,

    /// Buffer for shape spans.
    spans: Vec<ShapeSpan>,

    /// Buffer for shape words.
    words: Vec<ShapeWord>,

    /// Buffers for visual lines.
    visual_lines: Vec<VisualLine>,
    cached_visual_lines: Vec<VisualLine>,

    /// Buffer for sets of layout glyphs.
    glyph_sets: Vec<Vec<LayoutGlyph>>,

    /// Buffer for a line's bidi levels, adjusted for whitespace.
    bidi_levels: Vec<unicode_bidi::Level>,

    /// Buffers for a visual line's bidi levels and reordered level runs.
    reorder_levels: Vec<unicode_bidi::Level>,
    reorder_runs: Vec<Range<usize>>,

    /// Buffer for a run's font features converted for harfrust.
    rb_font_features: Vec<harfrust::Feature>,

    /// Buffers for font fallback: the clusters still missing a glyph, and a
    /// fallback font's glyphs and missing clusters.
    missing: Vec<usize>,
    fb_missing: Vec<usize>,
    fb_glyphs: Vec<ShapeGlyph>,
}

impl Default for ShapeBuffer {
    fn default() -> Self {
        Self {
            shape_plan_cache: VecDeque::new(),
            shape_plan_capacity: DEFAULT_SHAPE_PLAN_CAPACITY,
            harfrust_buffer: None,
            scripts: Vec::new(),
            spans: Vec::new(),
            words: Vec::new(),
            visual_lines: Vec::new(),
            cached_visual_lines: Vec::new(),
            glyph_sets: Vec::new(),
            bidi_levels: Vec::new(),
            reorder_levels: Vec::new(),
            reorder_runs: Vec::new(),
            rb_font_features: Vec::new(),
            missing: Vec::new(),
            fb_missing: Vec::new(),
            fb_glyphs: Vec::new(),
        }
    }
}

impl ShapeBuffer {
    /// Sets how many shape plans to keep, dropping the least recently used
    /// past it.
    pub fn set_shape_plan_capacity(&mut self, capacity: usize) {
        self.shape_plan_capacity = capacity.max(1);
        let excess = self
            .shape_plan_cache
            .len()
            .saturating_sub(self.shape_plan_capacity);
        self.shape_plan_cache.drain(..excess);
    }
}

impl fmt::Debug for ShapeBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("ShapeBuffer { .. }")
    }
}

fn shape_fallback(
    scratch: &mut ShapeBuffer,
    glyphs: &mut Vec<ShapeGlyph>,
    font: &Font,
    line: &str,
    attrs_list: &AttrsList,
    start_run: usize,
    end_run: usize,
    span_rtl: bool,
    missing: &mut Vec<usize>,
) {
    let run = &line[start_run..end_run];

    let font_scale = font.metrics().units_per_em as f32;
    let ascent = font.metrics().ascent / font_scale;
    let descent = -font.metrics().descent / font_scale;

    let mut buffer = scratch.harfrust_buffer.take().unwrap_or_default();
    buffer.set_direction(if span_rtl {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    });
    if run.contains('\t') {
        // Push string to buffer, replacing tabs with spaces
        //TODO: Find a way to do this with minimal allocating, calling
        // UnicodeBuffer::push_str multiple times causes issues and
        // UnicodeBuffer::add resizes the buffer with every character
        buffer.push_str(&run.replace('\t', " "));
    } else {
        buffer.push_str(run);
    }
    buffer.guess_segment_properties();

    let rtl = matches!(buffer.direction(), harfrust::Direction::RightToLeft);
    assert_eq!(rtl, span_rtl);

    let attrs = attrs_list.get_span_ref(start_run);
    let mut rb_font_features = mem::take(&mut scratch.rb_font_features);
    rb_font_features.clear();

    // Convert attrs::Feature to harfrust::Feature
    for feature in &attrs.font_features.features {
        rb_font_features.push(harfrust::Feature::new(
            harfrust::Tag::new(feature.tag.as_bytes()),
            feature.value,
            0..usize::MAX,
        ));
    }

    let language = buffer.language();
    let key = harfrust::ShapePlanKey::new(Some(buffer.script()), buffer.direction())
        .features(&rb_font_features)
        .instance(Some(font.shaper_instance()))
        .language(language.as_ref());

    let cache = &mut scratch.shape_plan_cache;
    match cache
        .iter()
        .rposition(|(id, plan)| *id == font.id() && key.matches(plan))
    {
        // Move a hit to the back, so eviction from the front drops the
        // least recently used plan.
        Some(i) => {
            if i + 1 != cache.len() {
                let entry = cache.remove(i).expect("position is in bounds");
                cache.push_back(entry);
            }
        }
        None => {
            let plan = harfrust::ShapePlan::new(
                font.shaper(),
                buffer.direction(),
                Some(buffer.script()),
                buffer.language().as_ref(),
                &rb_font_features,
            );
            if cache.len() >= scratch.shape_plan_capacity {
                cache.pop_front();
            }
            cache.push_back((font.id(), plan));
        }
    }
    let shape_plan = &cache.back().expect("the plan is cached").1;

    let glyph_buffer = font
        .shaper()
        .shape_with_plan(shape_plan, buffer, &rb_font_features);
    let glyph_infos = glyph_buffer.glyph_infos();
    let glyph_positions = glyph_buffer.glyph_positions();

    missing.clear();
    glyphs.reserve(glyph_infos.len());
    let glyph_start = glyphs.len();
    for (info, pos) in glyph_infos.iter().zip(glyph_positions.iter()) {
        let start_glyph = start_run + info.cluster as usize;

        if info.glyph_id == 0 {
            missing.push(start_glyph);
        }

        let attrs = attrs_list.get_span_ref(start_glyph);
        let x_advance = pos.x_advance as f32 / font_scale
            + attrs.letter_spacing_opt.map_or(0.0, |spacing| spacing.0);
        let y_advance = pos.y_advance as f32 / font_scale;
        let x_offset = pos.x_offset as f32 / font_scale;
        let y_offset = pos.y_offset as f32 / font_scale;

        glyphs.push(ShapeGlyph {
            start: start_glyph,
            end: end_run, // Set later
            x_advance,
            y_advance,
            x_offset,
            y_offset,
            ascent,
            descent,
            font_monospace_em_width: font.monospace_em_width(),
            font_id: font.id(),
            font_weight: attrs.weight,
            glyph_id: info.glyph_id.try_into().expect("failed to cast glyph ID"),
            //TODO: color should not be related to shaping
            color_opt: attrs.color_opt,
            metadata: attrs.metadata,
            cache_key_flags: attrs.cache_key_flags,
            metrics_opt: attrs.metrics_opt.map(Into::into),
        });
    }

    // Adjust end of glyphs
    if rtl {
        for i in glyph_start + 1..glyphs.len() {
            let next_start = glyphs[i - 1].start;
            let next_end = glyphs[i - 1].end;
            let prev = &mut glyphs[i];
            if prev.start == next_start {
                prev.end = next_end;
            } else {
                prev.end = next_start;
            }
        }
    } else {
        for i in (glyph_start + 1..glyphs.len()).rev() {
            let next_start = glyphs[i].start;
            let next_end = glyphs[i].end;
            let prev = &mut glyphs[i - 1];
            if prev.start == next_start {
                prev.end = next_end;
            } else {
                prev.end = next_start;
            }
        }
    }

    // Restore the buffers to save allocations.
    scratch.harfrust_buffer = Some(glyph_buffer.clear());
    scratch.rb_font_features = rb_font_features;
}

fn shape_run(
    glyphs: &mut Vec<ShapeGlyph>,
    font_system: &mut FontSystem,
    line: &str,
    attrs_list: &AttrsList,
    start_run: usize,
    end_run: usize,
    span_rtl: bool,
) {
    // Re-use the previous script buffer if possible.
    let mut scripts = {
        let mut scripts = mem::take(&mut font_system.shape_buffer.scripts);
        scripts.clear();
        scripts
    };
    // And the fallback buffers, which `shape_fallback` clears.
    let mut missing = mem::take(&mut font_system.shape_buffer.missing);
    let mut fb_missing = mem::take(&mut font_system.shape_buffer.fb_missing);
    let mut fb_glyphs = mem::take(&mut font_system.shape_buffer.fb_glyphs);
    for c in line[start_run..end_run].chars() {
        match c.script() {
            Script::Common | Script::Inherited | Script::Latin | Script::Unknown => (),
            script => {
                if !scripts.contains(&script) {
                    scripts.push(script);
                }
            }
        }
    }

    log::trace!("      Run {:?}: '{}'", &scripts, &line[start_run..end_run],);

    let attrs = attrs_list.get_span_ref(start_run).as_match_attrs();

    let fonts = font_system.get_font_matches(&attrs);

    let default_families = [&attrs.family];
    let mut font_iter = FontFallbackIter::new(
        font_system,
        &fonts,
        &default_families,
        &scripts,
        &line[start_run..end_run],
        attrs.weight,
    );

    let font = font_iter.next().expect("no default font found");

    let glyph_start = glyphs.len();
    shape_fallback(
        font_iter.shape_caches(),
        glyphs,
        &font,
        line,
        attrs_list,
        start_run,
        end_run,
        span_rtl,
        &mut missing,
    );

    //TODO: improve performance!
    while !missing.is_empty() {
        let Some(font) = font_iter.next() else {
            break;
        };

        log::trace!(
            "Evaluating fallback with font '{}'",
            font_iter.face_name(font.id())
        );
        fb_glyphs.clear();
        shape_fallback(
            font_iter.shape_caches(),
            &mut fb_glyphs,
            &font,
            line,
            attrs_list,
            start_run,
            end_run,
            span_rtl,
            &mut fb_missing,
        );

        // Insert all matching glyphs
        let mut fb_i = 0;
        while fb_i < fb_glyphs.len() {
            let start = fb_glyphs[fb_i].start;
            let end = fb_glyphs[fb_i].end;

            // Skip clusters that are not missing, or where the fallback font is missing
            if !missing.contains(&start) || fb_missing.contains(&start) {
                fb_i += 1;
                continue;
            }

            let mut missing_i = 0;
            while missing_i < missing.len() {
                if missing[missing_i] >= start && missing[missing_i] < end {
                    // println!("No longer missing {}", missing[missing_i]);
                    missing.remove(missing_i);
                } else {
                    missing_i += 1;
                }
            }

            // Find prior glyphs
            let mut i = glyph_start;
            while i < glyphs.len() {
                if glyphs[i].start >= start && glyphs[i].end <= end {
                    break;
                }
                i += 1;
            }

            // Remove prior glyphs
            while i < glyphs.len() {
                if glyphs[i].start >= start && glyphs[i].end <= end {
                    let _glyph = glyphs.remove(i);
                    // log::trace!("Removed {},{} from {}", _glyph.start, _glyph.end, i);
                } else {
                    break;
                }
            }

            while fb_i < fb_glyphs.len() {
                if fb_glyphs[fb_i].start >= start && fb_glyphs[fb_i].end <= end {
                    let fb_glyph = fb_glyphs.remove(fb_i);
                    // log::trace!("Insert {},{} from font {} at {}", fb_glyph.start, fb_glyph.end, font_i, i);
                    glyphs.insert(i, fb_glyph);
                    i += 1;
                } else {
                    break;
                }
            }
        }
    }

    // Debug missing font fallbacks
    font_iter.check_missing(&line[start_run..end_run]);

    /*
    for glyph in glyphs.iter() {
        log::trace!("'{}': {}, {}, {}, {}", &line[glyph.start..glyph.end], glyph.x_advance, glyph.y_advance, glyph.x_offset, glyph.y_offset);
    }
    */

    // Restore the buffers.
    font_system.shape_buffer.scripts = scripts;
    font_system.shape_buffer.missing = missing;
    font_system.shape_buffer.fb_missing = fb_missing;
    font_system.shape_buffer.fb_glyphs = fb_glyphs;
}

#[cfg(feature = "shape-run-cache")]
fn shape_run_cached(
    glyphs: &mut Vec<ShapeGlyph>,
    font_system: &mut FontSystem,
    line: &str,
    attrs_list: &AttrsList,
    start_run: usize,
    end_run: usize,
    span_rtl: bool,
) {
    use crate::{AttrsOwned, ShapeRunKey};

    let run_range = start_run..end_run;
    let mut key = ShapeRunKey {
        text: line[run_range.clone()].to_string(),
        default_attrs: AttrsOwned::new(&attrs_list.defaults()),
        attrs_spans: Vec::new(),
    };
    for (attrs_range, attrs) in attrs_list.spans.overlapping(&run_range) {
        if attrs == &key.default_attrs {
            // Skip if attrs matches default attrs
            continue;
        }
        let start = max(attrs_range.start, start_run).saturating_sub(start_run);
        let end = min(attrs_range.end, end_run).saturating_sub(start_run);
        if end > start {
            let range = start..end;
            key.attrs_spans.push((range, attrs.clone()));
        }
    }
    if let Some(cache_glyphs) = font_system.shape_run_cache.get(&key) {
        for mut glyph in cache_glyphs.iter().cloned() {
            // Adjust glyph start and end to match run position
            glyph.start += start_run;
            glyph.end += start_run;
            glyphs.push(glyph);
        }
        return;
    }

    // Fill in cache if not already set
    let mut cache_glyphs = Vec::new();
    shape_run(
        &mut cache_glyphs,
        font_system,
        line,
        attrs_list,
        start_run,
        end_run,
        span_rtl,
    );
    glyphs.extend_from_slice(&cache_glyphs);
    for glyph in cache_glyphs.iter_mut() {
        // Adjust glyph start and end to remove run position
        glyph.start -= start_run;
        glyph.end -= start_run;
    }
    font_system.shape_run_cache.insert(key, cache_glyphs);
}

#[cfg(feature = "swash")]
fn shape_skip(
    font_system: &mut FontSystem,
    glyphs: &mut Vec<ShapeGlyph>,
    line: &str,
    attrs_list: &AttrsList,
    start_run: usize,
    end_run: usize,
) {
    let attrs = attrs_list.get_span(start_run);
    let fonts = font_system.get_font_matches(&attrs);

    let default_families = [&attrs.family];
    let mut font_iter = FontFallbackIter::new(
        font_system,
        &fonts,
        &default_families,
        &[],
        "",
        attrs.weight,
    );

    let font = font_iter.next().expect("no default font found");
    let font_id = font.id();
    let font_monospace_em_width = font.monospace_em_width();
    let font = font.as_swash();

    let charmap = font.charmap();
    let metrics = font.metrics(&[]);
    let glyph_metrics = font.glyph_metrics(&[]).scale(1.0);

    let ascent = metrics.ascent / f32::from(metrics.units_per_em);
    let descent = metrics.descent / f32::from(metrics.units_per_em);

    glyphs.extend(
        line[start_run..end_run]
            .char_indices()
            .map(|(chr_idx, codepoint)| {
                let glyph_id = charmap.map(codepoint);
                let x_advance = glyph_metrics.advance_width(glyph_id)
                    + attrs.letter_spacing_opt.map_or(0.0, |spacing| spacing.0);
                let attrs = attrs_list.get_span(start_run + chr_idx);

                ShapeGlyph {
                    start: chr_idx + start_run,
                    end: chr_idx + start_run + codepoint.len_utf8(),
                    x_advance,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                    ascent,
                    descent,
                    font_monospace_em_width,
                    font_id,
                    font_weight: attrs.weight,
                    glyph_id,
                    color_opt: attrs.color_opt,
                    metadata: attrs.metadata,
                    cache_key_flags: attrs.cache_key_flags,
                    metrics_opt: attrs.metrics_opt.map(Into::into),
                }
            }),
    );
}

/// A shaped glyph
#[derive(Clone, Debug)]
pub struct ShapeGlyph {
    pub start: usize,
    pub end: usize,
    pub x_advance: f32,
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub ascent: f32,
    pub descent: f32,
    pub font_monospace_em_width: Option<f32>,
    pub font_id: fontdb::ID,
    pub font_weight: fontdb::Weight,
    pub glyph_id: u16,
    pub color_opt: Option<Color>,
    pub metadata: usize,
    pub cache_key_flags: CacheKeyFlags,
    pub metrics_opt: Option<Metrics>,
}

impl ShapeGlyph {
    const fn layout(
        &self,
        font_size: f32,
        line_height_opt: Option<f32>,
        x: f32,
        y: f32,
        w: f32,
        level: unicode_bidi::Level,
    ) -> LayoutGlyph {
        LayoutGlyph {
            start: self.start,
            end: self.end,
            font_size,
            line_height_opt,
            font_id: self.font_id,
            font_weight: self.font_weight,
            glyph_id: self.glyph_id,
            x,
            y,
            w,
            level,
            x_offset: self.x_offset,
            y_offset: self.y_offset,
            color_opt: self.color_opt,
            metadata: self.metadata,
            cache_key_flags: self.cache_key_flags,
        }
    }

    /// Get the width of the [`ShapeGlyph`] in pixels, either using the provided font size
    /// or the [`ShapeGlyph::metrics_opt`] override.
    pub fn width(&self, font_size: f32) -> f32 {
        self.metrics_opt.map_or(font_size, |x| x.font_size) * self.x_advance
    }
}

/// A shaped word (for word wrapping)
#[derive(Clone, Debug)]
pub struct ShapeWord {
    pub blank: bool,
    pub glyphs: Vec<ShapeGlyph>,
}

impl ShapeWord {
    /// Creates an empty word.
    ///
    /// The returned word is in an invalid state until [`Self::build_in_buffer`] is called.
    pub(crate) fn empty() -> Self {
        Self {
            blank: true,
            glyphs: Vec::default(),
        }
    }

    /// Shape a word into a set of glyphs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        word_range: Range<usize>,
        level: unicode_bidi::Level,
        blank: bool,
        shaping: Shaping,
    ) -> Self {
        let mut empty = Self::empty();
        empty.build(
            font_system,
            line,
            attrs_list,
            word_range,
            level,
            blank,
            shaping,
        );
        empty
    }

    /// See [`Self::new`].
    ///
    /// Reuses as much of the pre-existing internal allocations as possible.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        word_range: Range<usize>,
        level: unicode_bidi::Level,
        blank: bool,
        shaping: Shaping,
    ) {
        let word = &line[word_range.clone()];

        log::trace!(
            "      Word{}: '{}'",
            if blank { " BLANK" } else { "" },
            word
        );

        let mut glyphs = mem::take(&mut self.glyphs);
        glyphs.clear();

        let span_rtl = level.is_rtl();

        // Fast path optimization: For simple ASCII words, skip expensive grapheme iteration
        let is_simple_ascii =
            word.is_ascii() && !word.chars().any(|c| c.is_ascii_control() && c != '\t');

        if is_simple_ascii && !word.is_empty() {
            shaping.run(
                &mut glyphs,
                font_system,
                line,
                attrs_list,
                word_range.start,
                word_range.end,
                span_rtl,
            );
        } else {
            // Complex text path: Full grapheme iteration and attribute processing
            let mut start_run = word_range.start;
            let mut attrs = attrs_list.defaults_ref();
            for (egc_i, _egc) in word.grapheme_indices(true) {
                let start_egc = word_range.start + egc_i;
                let attrs_egc = attrs_list.get_span_ref(start_egc);
                if !attrs.compatible(attrs_egc) {
                    shaping.run(
                        &mut glyphs,
                        font_system,
                        line,
                        attrs_list,
                        start_run,
                        start_egc,
                        span_rtl,
                    );

                    start_run = start_egc;
                    attrs = attrs_egc;
                }
            }
            if start_run < word_range.end {
                shaping.run(
                    &mut glyphs,
                    font_system,
                    line,
                    attrs_list,
                    start_run,
                    word_range.end,
                    span_rtl,
                );
            }
        }

        self.blank = blank;
        self.glyphs = glyphs;
    }

    /// Get the width of the [`ShapeWord`] in pixels, using the [`ShapeGlyph::width`] function.
    pub fn width(&self, font_size: f32) -> f32 {
        let mut width = 0.0;
        for glyph in &self.glyphs {
            width += glyph.width(font_size);
        }
        width
    }
}

/// Whether the Unicode bidi algorithm gives `line` one left-to-right
/// paragraph with every level 0, so it need not run: the line is not empty
/// and has no paragraph separator, no strong right-to-left or Arabic number
/// character, and no explicit embedding, override, or isolate initiator.
/// These are the characters that clear unicode-bidi's own pure-LTR flag,
/// plus paragraph separators, which would split the line.
fn bidi_levels_all_ltr(line: &str) -> bool {
    use unicode_bidi::BidiClass::{AL, AN, B, FSI, LRE, LRI, LRO, R, RLE, RLI, RLO};
    !line.is_empty()
        && line.chars().all(|c| {
            !matches!(
                unicode_bidi::bidi_class(c),
                AL | AN | B | FSI | LRE | LRI | LRO | R | RLE | RLI | RLO
            )
        })
}

/// A shaped span (for bidirectional processing)
#[derive(Clone, Debug)]
pub struct ShapeSpan {
    pub level: unicode_bidi::Level,
    pub words: Vec<ShapeWord>,
}

impl ShapeSpan {
    /// Creates an empty span.
    ///
    /// The returned span is in an invalid state until [`Self::build_in_buffer`] is called.
    pub(crate) fn empty() -> Self {
        Self {
            level: unicode_bidi::Level::ltr(),
            words: Vec::default(),
        }
    }

    /// Shape a span into a set of words.
    pub fn new(
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        span_range: Range<usize>,
        line_rtl: bool,
        level: unicode_bidi::Level,
        shaping: Shaping,
    ) -> Self {
        let mut empty = Self::empty();
        empty.build(
            font_system,
            line,
            attrs_list,
            span_range,
            line_rtl,
            level,
            shaping,
        );
        empty
    }

    /// See [`Self::new`].
    ///
    /// Reuses as much of the pre-existing internal allocations as possible.
    pub fn build(
        &mut self,
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        span_range: Range<usize>,
        line_rtl: bool,
        level: unicode_bidi::Level,
        shaping: Shaping,
    ) {
        let span = &line[span_range.start..span_range.end];

        log::trace!(
            "  Span {}: '{}'",
            if level.is_rtl() { "RTL" } else { "LTR" },
            span
        );

        let mut words = mem::take(&mut self.words);

        // Cache the shape words in reverse order so they can be popped for reuse in the same order.
        let mut cached_words = mem::take(&mut font_system.shape_buffer.words);
        cached_words.clear();
        if line_rtl != level.is_rtl() {
            // Un-reverse previous words so the internal glyph counts match accurately when rewriting memory.
            cached_words.append(&mut words);
        } else {
            cached_words.extend(words.drain(..).rev());
        }

        let mut start_word = 0;
        for (end_lb, _) in unicode_linebreak::linebreaks(span) {
            let mut start_lb = end_lb;
            for (i, c) in span[start_word..end_lb].char_indices().rev() {
                // TODO: Not all whitespace characters are linebreakable, e.g. 00A0 (No-break
                // space)
                // https://www.unicode.org/reports/tr14/#GL
                // https://www.unicode.org/Public/UCD/latest/ucd/PropList.txt
                if c.is_whitespace() {
                    start_lb = start_word + i;
                } else {
                    break;
                }
            }
            if start_word < start_lb {
                let mut word = cached_words.pop().unwrap_or_else(ShapeWord::empty);
                word.build(
                    font_system,
                    line,
                    attrs_list,
                    (span_range.start + start_word)..(span_range.start + start_lb),
                    level,
                    false,
                    shaping,
                );
                words.push(word);
            }
            if start_lb < end_lb {
                for (i, c) in span[start_lb..end_lb].char_indices() {
                    // assert!(c.is_whitespace());
                    let mut word = cached_words.pop().unwrap_or_else(ShapeWord::empty);
                    word.build(
                        font_system,
                        line,
                        attrs_list,
                        (span_range.start + start_lb + i)
                            ..(span_range.start + start_lb + i + c.len_utf8()),
                        level,
                        true,
                        shaping,
                    );
                    words.push(word);
                }
            }
            start_word = end_lb;
        }

        // Reverse glyphs in RTL lines
        if line_rtl {
            for word in &mut words {
                word.glyphs.reverse();
            }
        }

        // Reverse words in spans that do not match line direction
        if line_rtl != level.is_rtl() {
            words.reverse();
        }

        self.level = level;
        self.words = words;

        // Cache buffer for future reuse.
        font_system.shape_buffer.words = cached_words;
    }
}

/// A shaped line (or paragraph)
#[derive(Clone, Debug)]
pub struct ShapeLine {
    pub rtl: bool,
    pub spans: Vec<ShapeSpan>,
    pub metrics_opt: Option<Metrics>,
}

// Visual Line Ranges: (span_index, (first_word_index, first_glyph_index), (last_word_index, last_glyph_index))
type VlRange = (usize, (usize, usize), (usize, usize));

#[derive(Default)]
struct VisualLine {
    ranges: Vec<VlRange>,
    spaces: u32,
    w: f32,
}

impl VisualLine {
    fn clear(&mut self) {
        self.ranges.clear();
        self.spaces = 0;
        self.w = 0.;
    }
}

impl ShapeLine {
    /// Creates an empty line.
    ///
    /// The returned line is in an invalid state until [`Self::build_in_buffer`] is called.
    pub(crate) fn empty() -> Self {
        Self {
            rtl: false,
            spans: Vec::default(),
            metrics_opt: None,
        }
    }

    /// Shape a line into a set of spans, using a scratch buffer. If [`unicode_bidi::BidiInfo`]
    /// detects multiple paragraphs, they will be joined.
    ///
    /// # Panics
    ///
    /// Will panic if `line` contains multiple paragraphs that do not have matching direction
    pub fn new(
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        shaping: Shaping,
        tab_width: u16,
    ) -> Self {
        let mut empty = Self::empty();
        empty.build(font_system, line, attrs_list, shaping, tab_width);
        empty
    }

    /// See [`Self::new`].
    ///
    /// Reuses as much of the pre-existing internal allocations as possible.
    ///
    /// # Panics
    ///
    /// Will panic if `line` contains multiple paragraphs that do not have matching direction
    pub fn build(
        &mut self,
        font_system: &mut FontSystem,
        line: &str,
        attrs_list: &AttrsList,
        shaping: Shaping,
        tab_width: u16,
    ) {
        let mut spans = mem::take(&mut self.spans);

        // Cache the shape spans in reverse order so they can be popped for reuse in the same order.
        let mut cached_spans = mem::take(&mut font_system.shape_buffer.spans);
        cached_spans.clear();
        cached_spans.extend(spans.drain(..).rev());

        let rtl;
        if bidi_levels_all_ltr(line) {
            // The full pass would find one left-to-right paragraph at level
            // 0 throughout, so build its one span without running it.
            rtl = false;
            let mut span = cached_spans.pop().unwrap_or_else(ShapeSpan::empty);
            span.build(
                font_system,
                line,
                attrs_list,
                0..line.len(),
                false,
                unicode_bidi::Level::ltr(),
                shaping,
            );
            spans.push(span);
        } else {
            let bidi = unicode_bidi::BidiInfo::new(line, None);
            rtl = if bidi.paragraphs.is_empty() {
                false
            } else {
                bidi.paragraphs[0].level.is_rtl()
            };

            log::trace!("Line {}: '{}'", if rtl { "RTL" } else { "LTR" }, line);

            let mut levels = mem::take(&mut font_system.shape_buffer.bidi_levels);
            for para_info in &bidi.paragraphs {
                let line_rtl = para_info.level.is_rtl();
                assert_eq!(line_rtl, rtl);

                let line_range = para_info.range.clone();
                Self::adjust_levels(&unicode_bidi::Paragraph::new(&bidi, para_info), &mut levels);

                // Find consecutive level runs. We use this to create Spans.
                // Each span is a set of characters with equal levels.
                let mut start = line_range.start;
                let mut run_level = levels[start];
                spans.reserve(line_range.end - start + 1);

                for (i, &new_level) in levels
                    .iter()
                    .enumerate()
                    .take(line_range.end)
                    .skip(start + 1)
                {
                    if new_level != run_level {
                        // End of the previous run, start of a new one.
                        let mut span = cached_spans.pop().unwrap_or_else(ShapeSpan::empty);
                        span.build(
                            font_system,
                            line,
                            attrs_list,
                            start..i,
                            line_rtl,
                            run_level,
                            shaping,
                        );
                        spans.push(span);
                        start = i;
                        run_level = new_level;
                    }
                }
                let mut span = cached_spans.pop().unwrap_or_else(ShapeSpan::empty);
                span.build(
                    font_system,
                    line,
                    attrs_list,
                    start..line_range.end,
                    line_rtl,
                    run_level,
                    shaping,
                );
                spans.push(span);
            }
            font_system.shape_buffer.bidi_levels = levels;
        }

        // Adjust for tabs
        let mut x = 0.0;
        for span in &mut spans {
            for word in &mut span.words {
                for glyph in &mut word.glyphs {
                    if line.get(glyph.start..glyph.end) == Some("\t") {
                        // Tabs are shaped as spaces, so they will always have the x_advance of a space.
                        let tab_x_advance = f32::from(tab_width) * glyph.x_advance;
                        let tab_stop = (math::floorf(x / tab_x_advance) + 1.0) * tab_x_advance;
                        glyph.x_advance = tab_stop - x;
                    }
                    x += glyph.x_advance;
                }
            }
        }

        self.rtl = rtl;
        self.spans = spans;
        self.metrics_opt = attrs_list.defaults_ref().metrics_opt.map(Into::into);

        // Return the buffers for later reuse.
        font_system.shape_buffer.spans = cached_spans;
    }

    // A modified version of first part of unicode_bidi::bidi_info::visual_run
    // Fills `levels`, which it clears first.
    fn adjust_levels(para: &unicode_bidi::Paragraph, levels: &mut Vec<unicode_bidi::Level>) {
        use unicode_bidi::BidiClass::{B, BN, FSI, LRE, LRI, LRO, PDF, PDI, RLE, RLI, RLO, S, WS};
        let text = para.info.text;
        let original_classes = &para.info.original_classes;

        levels.clear();
        levels.extend_from_slice(&para.info.levels);
        let line_classes = &original_classes[..];
        let line_levels = &mut levels[..];

        // Reset some whitespace chars to paragraph level.
        // <http://www.unicode.org/reports/tr9/#L1>
        let mut reset_from: Option<usize> = Some(0);
        let mut reset_to: Option<usize> = None;
        for (i, c) in text.char_indices() {
            match line_classes[i] {
                // Ignored by X9
                RLE | LRE | RLO | LRO | PDF | BN => {}
                // Segment separator, Paragraph separator
                B | S => {
                    assert_eq!(reset_to, None);
                    reset_to = Some(i + c.len_utf8());
                    if reset_from.is_none() {
                        reset_from = Some(i);
                    }
                }
                // Whitespace, isolate formatting
                WS | FSI | LRI | RLI | PDI => {
                    if reset_from.is_none() {
                        reset_from = Some(i);
                    }
                }
                _ => {
                    reset_from = None;
                }
            }
            if let (Some(from), Some(to)) = (reset_from, reset_to) {
                for level in &mut line_levels[from..to] {
                    *level = para.para.level;
                }
                reset_from = None;
                reset_to = None;
            }
        }
        if let Some(from) = reset_from {
            for level in &mut line_levels[from..] {
                *level = para.para.level;
            }
        }
    }

    // A modified version of second part of unicode_bidi::bidi_info::visual run
    /// Fills `runs` with the visual order of `line_range`'s level runs, using
    /// `line` for the levels.
    fn reorder(
        &self,
        line_range: &[VlRange],
        line: &mut Vec<unicode_bidi::Level>,
        runs: &mut Vec<Range<usize>>,
    ) {
        line.clear();
        line.extend(
            line_range
                .iter()
                .map(|(span_index, _, _)| self.spans[*span_index].level),
        );
        // Find consecutive level runs.
        runs.clear();
        let mut start = 0;
        let mut run_level = line[start];
        let mut min_level = run_level;
        let mut max_level = run_level;

        for (i, &new_level) in line.iter().enumerate().skip(start + 1) {
            if new_level != run_level {
                // End of the previous run, start of a new one.
                runs.push(start..i);
                start = i;
                run_level = new_level;
                min_level = min(run_level, min_level);
                max_level = max(run_level, max_level);
            }
        }
        runs.push(start..line.len());

        let run_count = runs.len();

        // Re-order the odd runs.
        // <http://www.unicode.org/reports/tr9/#L2>

        // Stop at the lowest *odd* level.
        min_level = min_level.new_lowest_ge_rtl().expect("Level error");

        while max_level >= min_level {
            // Look for the start of a sequence of consecutive runs of max_level or higher.
            let mut seq_start = 0;
            while seq_start < run_count {
                if line[runs[seq_start].start] < max_level {
                    seq_start += 1;
                    continue;
                }

                // Found the start of a sequence. Now find the end.
                let mut seq_end = seq_start + 1;
                while seq_end < run_count {
                    if line[runs[seq_end].start] < max_level {
                        break;
                    }
                    seq_end += 1;
                }

                // Reverse the runs within this sequence.
                runs[seq_start..seq_end].reverse();

                seq_start = seq_end;
            }
            max_level
                .lower(1)
                .expect("Lowering embedding level below zero");
        }
    }

    pub fn layout(
        &self,
        font_size: f32,
        width_opt: Option<f32>,
        wrap: Wrap,
        align: Option<Align>,
        match_mono_width: Option<f32>,
    ) -> Vec<LayoutLine> {
        let mut lines = Vec::with_capacity(1);
        self.layout_to_buffer(
            &mut ShapeBuffer::default(),
            font_size,
            width_opt,
            wrap,
            align,
            &mut lines,
            match_mono_width,
        );
        lines
    }

    pub fn layout_to_buffer(
        &self,
        scratch: &mut ShapeBuffer,
        font_size: f32,
        width_opt: Option<f32>,
        wrap: Wrap,
        align: Option<Align>,
        layout_lines: &mut Vec<LayoutLine>,
        match_mono_width: Option<f32>,
    ) {
        fn add_to_visual_line(
            vl: &mut VisualLine,
            span_index: usize,
            start: (usize, usize),
            end: (usize, usize),
            width: f32,
            number_of_blanks: u32,
        ) {
            if end == start {
                return;
            }

            vl.ranges.push((span_index, start, end));
            vl.w += width;
            vl.spaces += number_of_blanks;
        }

        // For each visual line a list of  (span index,  and range of words in that span)
        // Note that a BiDi visual line could have multiple spans or parts of them
        // let mut vl_range_of_spans = Vec::with_capacity(1);
        let mut visual_lines = mem::take(&mut scratch.visual_lines);
        let mut cached_visual_lines = mem::take(&mut scratch.cached_visual_lines);
        cached_visual_lines.clear();
        cached_visual_lines.extend(visual_lines.drain(..).map(|mut l| {
            l.clear();
            l
        }));

        // Cache glyph sets in reverse order so they will ideally be reused in exactly the same lines.
        let mut cached_glyph_sets = mem::take(&mut scratch.glyph_sets);
        cached_glyph_sets.clear();
        cached_glyph_sets.extend(layout_lines.drain(..).rev().map(|mut v| {
            v.glyphs.clear();
            v.glyphs
        }));

        // This would keep the maximum number of spans that would fit on a visual line
        // If one span is too large, this variable will hold the range of words inside that span
        // that fits on a line.
        // let mut current_visual_line: Vec<VlRange> = Vec::with_capacity(1);
        let mut current_visual_line = cached_visual_lines.pop().unwrap_or_default();
        let mut reorder_levels = mem::take(&mut scratch.reorder_levels);
        let mut new_order = mem::take(&mut scratch.reorder_runs);

        if wrap == Wrap::None {
            for (span_index, span) in self.spans.iter().enumerate() {
                let mut word_range_width = 0.;
                let mut number_of_blanks: u32 = 0;
                for word in &span.words {
                    let word_width = word.width(font_size);
                    word_range_width += word_width;
                    if word.blank {
                        number_of_blanks += 1;
                    }
                }
                add_to_visual_line(
                    &mut current_visual_line,
                    span_index,
                    (0, 0),
                    (span.words.len(), 0),
                    word_range_width,
                    number_of_blanks,
                );
            }
        } else {
            for (span_index, span) in self.spans.iter().enumerate() {
                let mut word_range_width = 0.;
                let mut width_before_last_blank = 0.;
                let mut number_of_blanks: u32 = 0;

                // Create the word ranges that fits in a visual line
                if self.rtl != span.level.is_rtl() {
                    // incongruent directions
                    let mut fitting_start = (span.words.len(), 0);
                    for (i, word) in span.words.iter().enumerate().rev() {
                        let word_width = word.width(font_size);

                        // Addition in the same order used to compute the final width, so that
                        // relayouts with that width as the `line_width` will produce the same
                        // wrapping results.
                        if current_visual_line.w + (word_range_width + word_width)
                            <= width_opt.unwrap_or(f32::INFINITY)
                            // Include one blank word over the width limit since it won't be
                            // counted in the final width
                            || (word.blank
                                && (current_visual_line.w + word_range_width) <= width_opt.unwrap_or(f32::INFINITY))
                        {
                            // fits
                            if word.blank {
                                number_of_blanks += 1;
                                width_before_last_blank = word_range_width;
                            }
                            word_range_width += word_width;
                        } else if wrap == Wrap::Glyph
                            // Make sure that the word is able to fit on it's own line, if not, fall back to Glyph wrapping.
                            || (wrap == Wrap::WordOrGlyph && word_width > width_opt.unwrap_or(f32::INFINITY))
                        {
                            // Commit the current line so that the word starts on the next line.
                            if word_range_width > 0.
                                && wrap == Wrap::WordOrGlyph
                                && word_width > width_opt.unwrap_or(f32::INFINITY)
                            {
                                add_to_visual_line(
                                    &mut current_visual_line,
                                    span_index,
                                    (i + 1, 0),
                                    fitting_start,
                                    word_range_width,
                                    number_of_blanks,
                                );

                                visual_lines.push(current_visual_line);
                                current_visual_line = cached_visual_lines.pop().unwrap_or_default();

                                number_of_blanks = 0;
                                word_range_width = 0.;

                                fitting_start = (i, 0);
                            }

                            for (glyph_i, glyph) in word.glyphs.iter().enumerate().rev() {
                                let glyph_width = glyph.width(font_size);
                                if current_visual_line.w + (word_range_width + glyph_width)
                                    <= width_opt.unwrap_or(f32::INFINITY)
                                {
                                    word_range_width += glyph_width;
                                } else {
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        (i, glyph_i + 1),
                                        fitting_start,
                                        word_range_width,
                                        number_of_blanks,
                                    );
                                    visual_lines.push(current_visual_line);
                                    current_visual_line =
                                        cached_visual_lines.pop().unwrap_or_default();

                                    number_of_blanks = 0;
                                    word_range_width = glyph_width;
                                    fitting_start = (i, glyph_i + 1);
                                }
                            }
                        } else {
                            // Wrap::Word, Wrap::WordOrGlyph

                            // If we had a previous range, commit that line before the next word.
                            if word_range_width > 0. {
                                // Current word causing a wrap is not whitespace, so we ignore the
                                // previous word if it's a whitespace
                                let trailing_blank = span
                                    .words
                                    .get(i + 1)
                                    .is_some_and(|previous_word| previous_word.blank);

                                if trailing_blank {
                                    number_of_blanks = number_of_blanks.saturating_sub(1);
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        (i + 2, 0),
                                        fitting_start,
                                        width_before_last_blank,
                                        number_of_blanks,
                                    );
                                } else {
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        (i + 1, 0),
                                        fitting_start,
                                        word_range_width,
                                        number_of_blanks,
                                    );
                                }

                                visual_lines.push(current_visual_line);
                                current_visual_line = cached_visual_lines.pop().unwrap_or_default();
                                number_of_blanks = 0;
                            }

                            if word.blank {
                                word_range_width = 0.;
                                fitting_start = (i, 0);
                            } else {
                                word_range_width = word_width;
                                fitting_start = (i + 1, 0);
                            }
                        }
                    }
                    add_to_visual_line(
                        &mut current_visual_line,
                        span_index,
                        (0, 0),
                        fitting_start,
                        word_range_width,
                        number_of_blanks,
                    );
                } else {
                    // congruent direction
                    let mut fitting_start = (0, 0);
                    for (i, word) in span.words.iter().enumerate() {
                        let word_width = word.width(font_size);
                        if current_visual_line.w + (word_range_width + word_width)
                            <= width_opt.unwrap_or(f32::INFINITY)
                            // Include one blank word over the width limit since it won't be
                            // counted in the final width.
                            || (word.blank
                                && (current_visual_line.w + word_range_width) <= width_opt.unwrap_or(f32::INFINITY))
                        {
                            // fits
                            if word.blank {
                                number_of_blanks += 1;
                                width_before_last_blank = word_range_width;
                            }
                            word_range_width += word_width;
                        } else if wrap == Wrap::Glyph
                            // Make sure that the word is able to fit on it's own line, if not, fall back to Glyph wrapping.
                            || (wrap == Wrap::WordOrGlyph && word_width > width_opt.unwrap_or(f32::INFINITY))
                        {
                            // Commit the current line so that the word starts on the next line.
                            if word_range_width > 0.
                                && wrap == Wrap::WordOrGlyph
                                && word_width > width_opt.unwrap_or(f32::INFINITY)
                            {
                                add_to_visual_line(
                                    &mut current_visual_line,
                                    span_index,
                                    fitting_start,
                                    (i, 0),
                                    word_range_width,
                                    number_of_blanks,
                                );

                                visual_lines.push(current_visual_line);
                                current_visual_line = cached_visual_lines.pop().unwrap_or_default();

                                number_of_blanks = 0;
                                word_range_width = 0.;

                                fitting_start = (i, 0);
                            }

                            for (glyph_i, glyph) in word.glyphs.iter().enumerate() {
                                let glyph_width = glyph.width(font_size);
                                if current_visual_line.w + (word_range_width + glyph_width)
                                    <= width_opt.unwrap_or(f32::INFINITY)
                                {
                                    word_range_width += glyph_width;
                                } else {
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        fitting_start,
                                        (i, glyph_i),
                                        word_range_width,
                                        number_of_blanks,
                                    );
                                    visual_lines.push(current_visual_line);
                                    current_visual_line =
                                        cached_visual_lines.pop().unwrap_or_default();

                                    number_of_blanks = 0;
                                    word_range_width = glyph_width;
                                    fitting_start = (i, glyph_i);
                                }
                            }
                        } else {
                            // Wrap::Word, Wrap::WordOrGlyph

                            // If we had a previous range, commit that line before the next word.
                            if word_range_width > 0. {
                                // Current word causing a wrap is not whitespace, so we ignore the
                                // previous word if it's a whitespace.
                                let trailing_blank = i > 0 && span.words[i - 1].blank;

                                if trailing_blank {
                                    number_of_blanks = number_of_blanks.saturating_sub(1);
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        fitting_start,
                                        (i - 1, 0),
                                        width_before_last_blank,
                                        number_of_blanks,
                                    );
                                } else {
                                    add_to_visual_line(
                                        &mut current_visual_line,
                                        span_index,
                                        fitting_start,
                                        (i, 0),
                                        word_range_width,
                                        number_of_blanks,
                                    );
                                }

                                visual_lines.push(current_visual_line);
                                current_visual_line = cached_visual_lines.pop().unwrap_or_default();
                                number_of_blanks = 0;
                            }

                            if word.blank {
                                word_range_width = 0.;
                                fitting_start = (i + 1, 0);
                            } else {
                                word_range_width = word_width;
                                fitting_start = (i, 0);
                            }
                        }
                    }
                    add_to_visual_line(
                        &mut current_visual_line,
                        span_index,
                        fitting_start,
                        (span.words.len(), 0),
                        word_range_width,
                        number_of_blanks,
                    );
                }
            }
        }

        if current_visual_line.ranges.is_empty() {
            current_visual_line.clear();
            cached_visual_lines.push(current_visual_line);
        } else {
            visual_lines.push(current_visual_line);
        }

        // Create the LayoutLines using the ranges inside visual lines
        let align = align.unwrap_or(if self.rtl { Align::Right } else { Align::Left });

        let line_width = width_opt.map_or_else(
            || {
                let mut width: f32 = 0.0;
                for visual_line in &visual_lines {
                    width = width.max(visual_line.w);
                }
                width
            },
            |width| width,
        );

        let start_x = if self.rtl { line_width } else { 0.0 };

        let number_of_visual_lines = visual_lines.len();
        for (index, visual_line) in visual_lines.iter().enumerate() {
            if visual_line.ranges.is_empty() {
                continue;
            }
            self.reorder(&visual_line.ranges, &mut reorder_levels, &mut new_order);
            let mut glyphs = cached_glyph_sets
                .pop()
                .unwrap_or_else(|| Vec::with_capacity(1));
            let mut x = start_x;
            let mut y = 0.;
            let mut max_ascent: f32 = 0.;
            let mut max_descent: f32 = 0.;
            let alignment_correction = match (align, self.rtl) {
                (Align::Left, true) => line_width - visual_line.w,
                (Align::Left, false) => 0.,
                (Align::Right, true) => 0.,
                (Align::Right, false) => line_width - visual_line.w,
                (Align::Center, _) => (line_width - visual_line.w) / 2.0,
                (Align::End, _) => line_width - visual_line.w,
                (Align::Justified, _) => 0.,
            };

            if self.rtl {
                x -= alignment_correction;
            } else {
                x += alignment_correction;
            }

            // TODO: Only certain `is_whitespace` chars are typically expanded but this is what is
            // currently used to compute `visual_line.spaces`.
            //
            // https://www.unicode.org/reports/tr14/#Introduction
            // > When expanding or compressing interword space according to common
            // > typographical practice, only the spaces marked by U+0020 SPACE and U+00A0
            // > NO-BREAK SPACE are subject to compression, and only spaces marked by U+0020
            // > SPACE, U+00A0 NO-BREAK SPACE, and occasionally spaces marked by U+2009 THIN
            // > SPACE are subject to expansion. All other space characters normally have
            // > fixed width.
            //
            // (also some spaces aren't followed by potential linebreaks but they could
            //  still be expanded)

            // Amount of extra width added to each blank space within a line.
            let justification_expansion = if matches!(align, Align::Justified)
                && visual_line.spaces > 0
                // Don't justify the last line in a paragraph.
                && index != number_of_visual_lines - 1
            {
                (line_width - visual_line.w) / visual_line.spaces as f32
            } else {
                0.
            };

            let mut process_range = |range: Range<usize>| {
                for &(span_index, (starting_word, starting_glyph), (ending_word, ending_glyph)) in
                    &visual_line.ranges[range]
                {
                    let span = &self.spans[span_index];
                    // If ending_glyph is not 0 we need to include glyphs from the ending_word
                    for i in starting_word..ending_word + usize::from(ending_glyph != 0) {
                        let word = &span.words[i];
                        let included_glyphs = match (i == starting_word, i == ending_word) {
                            (false, false) => &word.glyphs[..],
                            (true, false) => &word.glyphs[starting_glyph..],
                            (false, true) => &word.glyphs[..ending_glyph],
                            (true, true) => &word.glyphs[starting_glyph..ending_glyph],
                        };

                        for glyph in included_glyphs {
                            // Use overridden font size
                            let font_size = glyph.metrics_opt.map_or(font_size, |x| x.font_size);

                            let match_mono_em_width = match_mono_width.map(|w| w / font_size);

                            let glyph_font_size = match (
                                match_mono_em_width,
                                glyph.font_monospace_em_width,
                            ) {
                                (Some(match_em_width), Some(glyph_em_width))
                                    if glyph_em_width != match_em_width =>
                                {
                                    let glyph_to_match_factor = glyph_em_width / match_em_width;
                                    let glyph_font_size = math::roundf(glyph_to_match_factor)
                                        .max(1.0)
                                        / glyph_to_match_factor
                                        * font_size;
                                    log::trace!(
                                        "Adjusted glyph font size ({font_size} => {glyph_font_size})"
                                    );
                                    glyph_font_size
                                }
                                _ => font_size,
                            };

                            let x_advance = glyph_font_size.mul_add(
                                glyph.x_advance,
                                if word.blank {
                                    justification_expansion
                                } else {
                                    0.0
                                },
                            );
                            if self.rtl {
                                x -= x_advance;
                            }
                            let y_advance = glyph_font_size * glyph.y_advance;
                            glyphs.push(glyph.layout(
                                glyph_font_size,
                                glyph.metrics_opt.map(|x| x.line_height),
                                x,
                                y,
                                x_advance,
                                span.level,
                            ));
                            if !self.rtl {
                                x += x_advance;
                            }
                            y += y_advance;
                            max_ascent = max_ascent.max(glyph_font_size * glyph.ascent);
                            max_descent = max_descent.max(glyph_font_size * glyph.descent);
                        }
                    }
                }
            };

            if self.rtl {
                for range in new_order.iter().rev() {
                    process_range(range.clone());
                }
            } else {
                /* LTR */
                for range in &new_order {
                    process_range(range.clone());
                }
            }

            let mut line_height_opt: Option<f32> = None;
            for glyph in &glyphs {
                if let Some(glyph_line_height) = glyph.line_height_opt {
                    line_height_opt = line_height_opt
                        .map_or(Some(glyph_line_height), |line_height| {
                            Some(line_height.max(glyph_line_height))
                        });
                }
            }

            layout_lines.push(LayoutLine {
                w: if align != Align::Justified {
                    visual_line.w
                } else if self.rtl {
                    start_x - x
                } else {
                    x
                },
                max_ascent,
                max_descent,
                line_height_opt,
                glyphs,
            });
        }

        // This is used to create a visual line for empty lines (e.g. lines with only a <CR>)
        if layout_lines.is_empty() {
            layout_lines.push(LayoutLine {
                w: 0.0,
                max_ascent: 0.0,
                max_descent: 0.0,
                line_height_opt: self.metrics_opt.map(|x| x.line_height),
                glyphs: Vec::default(),
            });
        }

        // Restore the buffer to the scratch set to prevent reallocations.
        scratch.visual_lines = visual_lines;
        scratch.visual_lines.append(&mut cached_visual_lines);
        scratch.cached_visual_lines = cached_visual_lines;
        scratch.glyph_sets = cached_glyph_sets;
        scratch.reorder_levels = reorder_levels;
        scratch.reorder_runs = new_order;
    }
}

#[cfg(test)]
mod tests {
    use super::{bidi_levels_all_ltr, ShapeLine};
    use alloc::vec::Vec;
    use core::ops::Range;
    use unicode_bidi::{BidiInfo, Level, Paragraph};

    /// What the full bidi pass gives `line`: each paragraph's range and
    /// level, and every byte's level after whitespace adjustment.
    fn full_pass(line: &str) -> (Vec<(Range<usize>, Level)>, Vec<Level>) {
        let bidi = BidiInfo::new(line, None);
        let mut all = Vec::new();
        let mut levels = Vec::new();
        for para in &bidi.paragraphs {
            ShapeLine::adjust_levels(&Paragraph::new(&bidi, para), &mut levels);
            all.extend_from_slice(&levels[para.range.clone()]);
        }
        let paragraphs = bidi
            .paragraphs
            .iter()
            .map(|p| (p.range.clone(), p.level))
            .collect();
        (paragraphs, all)
    }

    // The fast path must take exactly the lines the full pass resolves to
    // one level-0 paragraph, and every class that can raise a level (or
    // split the line) must send the line through the full pass.
    #[test]
    fn bidi_fast_path_matches_full_pass() {
        let cases: &[(&str, bool)] = &[
            ("hello world", true),
            ("\u{416}\u{438}\u{437}\u{43d}\u{44c} \u{65e5}\u{672c}", true),
            ("123 + 4.5% = $6, (7) / 8:9", true),
            ("-- ... !? [] {} <>", true),
            (
                "tab\there e\u{301} soft\u{ad}hyphen zero\u{200b}width",
                true,
            ),
            ("emoji \u{1f600} \u{1f469}\u{200d}\u{1f4bb}", true),
            ("stray pdf\u{202c} and pdi\u{2069}", true),
            ("   ", true),
            ("", false),
            ("ab \u{5e9}\u{5dc}\u{5d5}\u{5dd}", false),
            ("ab \u{633}\u{644}\u{627}\u{645}", false),
            ("ab \u{661}\u{662}\u{663}", false),
            ("ab \u{202a}cd\u{202c}", false),
            ("ab \u{202b}cd\u{202c}", false),
            ("ab \u{202d}cd\u{202c}", false),
            ("ab \u{202e}cd\u{202c}", false),
            ("ab \u{2066}cd\u{2069}", false),
            ("ab \u{2067}cd\u{2069}", false),
            ("ab \u{2068}cd\u{2069}", false),
            ("file 12 \u{5e9}\u{5dc} (34) end", false),
            ("ab\u{2029}cd", false),
        ];
        for &(line, fast) in cases {
            assert_eq!(bidi_levels_all_ltr(line), fast, "{line:?}");
            let (paragraphs, levels) = full_pass(line);
            let one_ltr_paragraph = paragraphs == [(0..line.len(), Level::ltr())]
                && levels.iter().all(|&level| level == Level::ltr());
            // A separator splits an otherwise left-to-right line, and an
            // empty line has no paragraph; every other slow case raises a
            // level somewhere.
            assert_eq!(
                one_ltr_paragraph, fast,
                "{line:?}: {paragraphs:?} {levels:?}"
            );
        }
    }
}
