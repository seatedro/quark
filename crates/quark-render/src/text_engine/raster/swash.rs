//! The swash backend: Linux's rasterizer, every platform's fallback, and
//! the deterministic one tests use.
//!
//! It draws what cosmic-text's `SwashCache::get_image_uncached` draws for
//! the same glyph (hinting, the same color sources in the same order, a
//! fiftieth of the em of thickening, a 14 degree synthetic slant), but from
//! a [`PreparedFont`] rather than a mutable `FontSystem`, applying every
//! variation axis the font was shaped with where that path applies only
//! `wght`; the two agree because quark-text shapes with the other axes at
//! their defaults.

use std::collections::HashMap;

use quark_text::cosmic_text::{
    SwashCache, SwashContent, SwashFace, SwashImage, SwashRender, SwashSource,
};
use quark_text::fonts::FaceId;

use super::{
    AlphaMode, BitmapContent, ColorGlyphs, GlyphBitmap, GlyphRasterizer, Hinting, MAX_BITMAP_BYTES,
    Placement, PreparedFont, RasterBackend, RasterCapabilities, RasterError, RasterOutcome,
    RasterProfile, RasterRequest, RasterScratch,
};

/// Bump when the bitmaps drawn for the same request change.
const PROFILE: RasterProfile = RasterProfile {
    backend: RasterBackend::Swash,
    revision: 1,
};

/// The slant of synthetic italic, as cosmic-text's `FAKE_ITALIC` draws it.
const SYNTHETIC_ITALIC_DEGREES: f32 = 14.0;

pub(crate) struct SwashRasterizer {
    /// Swash's scale context; its per-font state is keyed by the faces
    /// below, so each face is parsed once.
    cache: SwashCache,
    /// `None` for a face swash cannot parse.
    faces: HashMap<FaceId, Option<SwashFace>>,
    /// Drawn into, then swapped with the scratch's pixels, so neither
    /// buffer is copied or allocated once both have grown.
    image: SwashImage,
}

impl Default for SwashRasterizer {
    fn default() -> Self {
        Self {
            cache: SwashCache::new(),
            faces: HashMap::new(),
            image: SwashImage::new(),
        }
    }
}

impl SwashRasterizer {
    /// Lets go of every face, and the font bytes they hold. Call when the
    /// fonts change; faces still in use are parsed again on their next
    /// glyph.
    pub(crate) fn forget_faces(&mut self) {
        self.faces.clear();
    }
}

impl GlyphRasterizer for SwashRasterizer {
    fn profile(&self) -> RasterProfile {
        PROFILE
    }

    fn capabilities(&self) -> RasterCapabilities {
        RasterCapabilities {
            color_outlines: true,
            color_bitmaps: true,
            variations: true,
            synthesis: true,
        }
    }

    fn rasterize(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
        scratch: &mut RasterScratch,
    ) -> Result<RasterOutcome, RasterError> {
        if u32::from(request.glyph()) >= font.glyph_count() {
            return Err(RasterError::InvalidGlyph);
        }
        let face = self
            .faces
            .entry(font.face())
            .or_insert_with(|| SwashFace::new(font.source().shared_data().clone(), font.index()))
            .as_ref()
            .ok_or(RasterError::UnsupportedFont)?;
        let size = request.physical_em();
        let synthesis = font.synthesis();
        let options = request.options();
        let render = SwashRender {
            size,
            hint: options.hinting == Hinting::Font,
            offset: request.subpixel().pixels(),
            embolden: if synthesis.thicken { size / 50.0 } else { 0.0 },
            skew_degrees: synthesis.italic.then_some(SYNTHETIC_ITALIC_DEGREES),
            color: options.color == ColorGlyphs::Prefer,
        };
        let variations = font.variations().iter().map(|v| (v.tag, v.value));
        if !self
            .cache
            .render_face_into(face, request.glyph(), variations, &render, &mut self.image)
        {
            return Ok(RasterOutcome::Empty);
        }
        let content = match self.image.content {
            SwashContent::Mask => BitmapContent::Mask,
            SwashContent::Color => BitmapContent::Color {
                // Layers composite over transparent black; embedded
                // bitmaps decode as stored.
                alpha: match self.image.source {
                    SwashSource::ColorOutline(_) => AlphaMode::Premultiplied,
                    _ => AlphaMode::Straight,
                },
            },
            // Only asked for with a subpixel format, which this never uses.
            SwashContent::SubpixelMask => return Err(RasterError::UnsupportedFormat),
        };
        let placement = self.image.placement;
        if placement.width == 0 || placement.height == 0 {
            return Ok(RasterOutcome::Empty);
        }
        let stride = placement
            .width
            .checked_mul(content.bytes_per_pixel())
            .ok_or(RasterError::TooLarge)?;
        let len = (stride as usize)
            .checked_mul(placement.height as usize)
            .filter(|&len| len <= MAX_BITMAP_BYTES && len <= self.image.data.len())
            .ok_or(RasterError::TooLarge)?;
        std::mem::swap(&mut scratch.pixels, &mut self.image.data);
        Ok(RasterOutcome::Bitmap(GlyphBitmap {
            content,
            placement: Placement {
                left: placement.left,
                top: placement.top,
                width: placement.width,
                height: placement.height,
            },
            stride,
            bytes: 0..len,
        }))
    }
}

#[cfg(test)]
mod tests {
    use quark::FontWeight;
    use quark::scene::FontStyle;
    use quark_text::fonts::FontRegistry;
    use quark_text::{FontSettings, TextParams, TextSpan, TextStyle, TextSystem};

    use super::*;

    /// A glyph's drawing, compared across rasterizers: `None` for nothing
    /// to draw, else whether it is a mask, its placement, and its pixels.
    type Drawn = Option<(bool, [i32; 4], Vec<u8>)>;

    /// The texts of the comparison: static and variable faces at clamped
    /// and unclamped weights, a real italic face, synthetic italic,
    /// thickening, ligatures, CJK, and color emoji.
    fn cases() -> Vec<TextParams> {
        let style = TextStyle::new(13.0);
        let italic = |text: &str, style: TextStyle| {
            TextParams::new(text, style).spans(vec![TextSpan {
                range: 0..text.len(),
                weight: None,
                style: Some(FontStyle::Italic),
                kind: None,
                size: None,
                letter_spacing: None,
                keep_together: false,
            }])
        };
        let family = |family, weight| style.family(Some(family)).weight(weight);
        vec![
            TextParams::new("Hamburg fjord \u{c5}\u{c9}", style),
            TextParams::new("Wavy 0123", family("Inter", FontWeight::Light)),
            TextParams::new("Wavy 0123", family("Inter", FontWeight::Numeric(650))),
            TextParams::new("Wavy 0123", family("Inter", FontWeight::Numeric(1000))),
            TextParams::new("fn main() -> ffi", family("Fira Code", FontWeight::Normal)),
            italic("slanted", family("JetBrains Mono", FontWeight::Bold)),
            italic("slanted", style),
            TextParams::new("thick", style.thicken(true)),
            TextParams::new("\u{65e5}\u{672c}\u{8a9e}", style),
            TextParams::new("\u{1f600}\u{1f469}\u{200d}\u{1f4bb}", style),
        ]
    }

    // Catches the swash backend drawing other pixels than the path the
    // renderer rasterizes with today, cosmic-text's `get_image_uncached`
    // through the font system: a variation, weight clamp, synthesis,
    // subpixel offset, size, or color source the prepared font or request
    // loses or maps differently.
    #[test]
    fn swash_rasterizer_draws_the_bitmaps_of_the_font_system_path() {
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        let mut registry = FontRegistry::new(system.font_snapshot());
        let mut legacy = SwashCache::new();
        let mut rasterizer = SwashRasterizer::default();
        let mut scratch = RasterScratch::default();
        let (mut masks, mut colors) = (0, 0);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for params in cases() {
                let layout = system
                    .layout(&params.clone().scale_factor(scale))
                    .expect("layout");
                // Origins whose fractions reach each horizontal subpixel bin.
                for origin in [(0.0, 0.0), (10.3, 4.6), (-7.55, -2.5), (3.8, 0.0)] {
                    for i in 0..layout.glyph_count() {
                        let key = layout.physical_glyph(i, origin).expect("glyph").cache_key;
                        let expected: Drawn = legacy
                            .get_image_uncached(system.raster_font_system(), key)
                            .filter(|image| image.placement.width * image.placement.height > 0)
                            .map(|image| {
                                let p = image.placement;
                                let mask = image.content == SwashContent::Mask;
                                let placement = [p.left, p.top, p.width as i32, p.height as i32];
                                (mask, placement, image.data)
                            });
                        let font = registry
                            .prepare(key.font_id, key.font_weight, key.flags)
                            .expect("font");
                        let request = RasterRequest::from_cache_key(&key, scale).expect("request");
                        let outcome = rasterizer
                            .rasterize(font, &request, &mut scratch)
                            .expect("raster");
                        let drawn: Drawn = match &outcome {
                            RasterOutcome::Empty => None,
                            RasterOutcome::Bitmap(bitmap) => {
                                let p = bitmap.placement;
                                let mask = bitmap.content == BitmapContent::Mask;
                                let placement = [p.left, p.top, p.width as i32, p.height as i32];
                                Some((mask, placement, scratch.bitmap(bitmap).to_vec()))
                            }
                        };
                        match &drawn {
                            Some((true, ..)) => masks += 1,
                            Some((false, ..)) => colors += 1,
                            None => {}
                        }
                        assert!(
                            drawn == expected,
                            "glyph {i} of {:?} at {scale}x, origin {origin:?}",
                            params.text
                        );
                    }
                }
            }
        }
        // The comparison covered both kinds of bitmap.
        assert!(masks > 0 && colors > 0, "{masks} masks, {colors} colors");
    }

    // Regression: SF Pro drawn after bold JetBrains Mono through one shared
    // swash context took the coordinate left by the earlier variable face
    // on an axis its own settings leave out, about 15% more ink, so its
    // pixels depended on drawing order. Both swash paths must draw it alike
    // whatever came before. Only macOS has SF Pro installed.
    #[cfg(target_os = "macos")]
    #[test]
    fn sf_pro_draws_alike_whatever_was_drawn_before() {
        use quark_text::fonts::SYSTEM_UI;

        let mut system = TextSystem::with_settings(&FontSettings {
            ui_family: SYSTEM_UI.into(),
            mono_family: "JetBrains Mono".into(),
            ..FontSettings::default()
        });
        let keys = |system: &mut TextSystem, style: TextStyle| {
            let params = TextParams::new("Hamburgefonstiv quick 0123 llll", style);
            let layout = system.layout(&params).expect("layout");
            (0..layout.glyph_count())
                .map(|i| {
                    layout
                        .physical_glyph(i, (0.0, 0.0))
                        .expect("glyph")
                        .cache_key
                })
                .collect::<Vec<_>>()
        };
        let sf = keys(&mut system, TextStyle::new(13.0));
        let mono: Vec<_> = [11.0, 13.0, 17.0, 20.0, 28.0, 9.0, 10.0, 12.0, 14.0]
            .into_iter()
            .flat_map(|size| {
                let style = TextStyle::new(size)
                    .kind(quark::FontKind::Mono)
                    .weight(quark::FontWeight::Bold);
                keys(&mut system, style)
            })
            .collect();

        // The font system path, one context as the atlas's fallback keeps.
        let mut legacy = |cache: &mut SwashCache, keys: &[quark_text::cosmic_text::CacheKey]| {
            keys.iter()
                .map(|&key| {
                    cache
                        .get_image_uncached(system.raster_font_system(), key)
                        .map(|image| image.data)
                })
                .collect::<Vec<_>>()
        };
        let alone = legacy(&mut SwashCache::new(), &sf);
        assert!(
            alone.iter().flatten().any(|data| !data.is_empty()),
            "SF Pro drew nothing"
        );
        let mut shared = SwashCache::new();
        legacy(&mut shared, &mono);
        assert!(
            legacy(&mut shared, &sf) == alone,
            "font system path: order changed SF Pro"
        );

        // The prepared-font path.
        let mut registry = FontRegistry::new(system.font_snapshot());
        let mut draw = |rasterizer: &mut SwashRasterizer,
                        keys: &[quark_text::cosmic_text::CacheKey]| {
            let mut scratch = RasterScratch::default();
            keys.iter()
                .map(|key| {
                    let font = registry
                        .prepare(key.font_id, key.font_weight, key.flags)
                        .expect("font")
                        .clone();
                    let request = RasterRequest::from_cache_key(key, 1.0).expect("request");
                    match rasterizer.rasterize(&font, &request, &mut scratch) {
                        Ok(RasterOutcome::Bitmap(bitmap)) => scratch.bitmap(&bitmap).to_vec(),
                        Ok(RasterOutcome::Empty) => Vec::new(),
                        Err(error) => panic!("{error}"),
                    }
                })
                .collect::<Vec<_>>()
        };
        let alone = draw(&mut SwashRasterizer::default(), &sf);
        let mut shared = SwashRasterizer::default();
        draw(&mut shared, &mono);
        assert!(
            draw(&mut shared, &sf) == alone,
            "prepared path: order changed SF Pro"
        );
    }
}
