// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use core::fmt;
use swash::scale::{image::Content, ScaleContext};
use swash::scale::{Render, Source, StrikeWith};
use swash::zeno::{Format, Vector};

use crate::{CacheKey, CacheKeyFlags, Color, FontSystem, HashMap};

pub use swash::scale::image::{Content as SwashContent, Image as SwashImage};
pub use swash::scale::Source as SwashSource;
pub use swash::zeno::{Angle, Command, Placement, Transform};

fn swash_image(
    font_system: &mut FontSystem,
    context: &mut ScaleContext,
    cache_key: CacheKey,
) -> Option<SwashImage> {
    let Some(font) = font_system.get_font(cache_key.font_id, cache_key.font_weight) else {
        log::warn!("did not find font {:?}", cache_key.font_id);
        return None;
    };

    let variable_width = font
        .as_swash()
        .variations()
        .find_by_tag(swash::Tag::from_be_bytes(*b"wght"));

    // Build the scaler. The context keeps the last font's variation
    // coordinates and `variations` only overwrites the axes it names, so
    // clear them first: otherwise an axis this font has but the settings
    // leave out (Inter's `opsz`) takes another font's coordinate.
    let mut scaler = context
        .builder(font.as_swash())
        .normalized_coords(core::iter::empty::<i16>())
        .size(f32::from_bits(cache_key.font_size_bits))
        .hint(!cache_key.flags.contains(CacheKeyFlags::DISABLE_HINTING));
    if let Some(variation) = variable_width {
        scaler = scaler.variations(std::iter::once(swash::Setting {
            tag: swash::Tag::from_be_bytes(*b"wght"),
            value: f32::from(cache_key.font_weight.0)
                .clamp(variation.min_value(), variation.max_value()),
        }));
    }
    let mut scaler = scaler.build();

    // Compute the fractional offset-- you'll likely want to quantize this
    // in a real renderer
    let offset = if cache_key.flags.contains(CacheKeyFlags::PIXEL_FONT) {
        Vector::new(
            cache_key.x_bin.as_float().round() + 1.0,
            cache_key.y_bin.as_float().round(),
        )
    } else {
        Vector::new(cache_key.x_bin.as_float(), cache_key.y_bin.as_float())
    };

    // Thickening widens outlines by a fiftieth of the em: half a pixel at
    // 26 pixels per em (13 points on a 2x display).
    let embolden = if cache_key.flags.contains(CacheKeyFlags::THICKEN) {
        f32::from_bits(cache_key.font_size_bits) / 50.0
    } else {
        0.0
    };

    // Select our source order
    Render::new(&[
        // Color outline with the first palette
        Source::ColorOutline(0),
        // Color bitmap with best fit selection mode
        Source::ColorBitmap(StrikeWith::BestFit),
        // Standard scalable outline
        Source::Outline,
    ])
    // Select a subpixel format
    .format(Format::Alpha)
    .embolden(embolden)
    // Apply the fractional offset
    .offset(offset)
    .transform(if cache_key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
        Some(Transform::skew(
            Angle::from_degrees(14.0),
            Angle::from_degrees(0.0),
        ))
    } else {
        None
    })
    // Render the image
    .render(&mut scaler, cache_key.glyph_id)
}

fn swash_outline_commands(
    font_system: &mut FontSystem,
    context: &mut ScaleContext,
    cache_key: CacheKey,
) -> Option<Box<[swash::zeno::Command]>> {
    use swash::zeno::PathData as _;

    let Some(font) = font_system.get_font(cache_key.font_id, cache_key.font_weight) else {
        log::warn!("did not find font {:?}", cache_key.font_id);
        return None;
    };

    // Build the scaler
    let mut scaler = context
        .builder(font.as_swash())
        .size(f32::from_bits(cache_key.font_size_bits))
        .hint(!cache_key.flags.contains(CacheKeyFlags::DISABLE_HINTING))
        .build();

    // Scale the outline
    let mut outline = scaler
        .scale_outline(cache_key.glyph_id)
        .or_else(|| scaler.scale_color_outline(cache_key.glyph_id))?;

    if cache_key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
        outline.transform(&Transform::skew(
            Angle::from_degrees(14.0),
            Angle::from_degrees(0.0),
        ));
    }

    // Get the path information of the outline
    let path = outline.path();

    // Return the commands
    Some(path.commands().collect())
}

/// A face for [`SwashCache::render_face_into`]: the font's bytes, the face's
/// offset in them, and swash's key for the scale context's per-font state.
/// Make one per face and keep it: a new one per glyph has a new key, which
/// throws that state away.
pub struct SwashFace {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    offset: u32,
    key: swash::CacheKey,
}

impl SwashFace {
    /// Face `index` of a font file or collection, or `None` when the bytes
    /// hold no such face.
    pub fn new(data: Arc<dyn AsRef<[u8]> + Send + Sync>, index: u32) -> Option<Self> {
        let font = swash::FontRef::from_index((*data).as_ref(), index as usize)?;
        let (offset, key) = (font.offset, font.key);
        Some(Self { data, offset, key })
    }

    fn font(&self) -> swash::FontRef<'_> {
        swash::FontRef {
            data: (*self.data).as_ref(),
            offset: self.offset,
            key: self.key,
        }
    }
}

impl fmt::Debug for SwashFace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwashFace")
            .field("offset", &self.offset)
            .finish_non_exhaustive()
    }
}

/// How [`SwashCache::render_face_into`] draws a glyph: what a [`CacheKey`]
/// and a font system's face decide for [`SwashCache::get_image`], given
/// directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwashRender {
    /// Pixels per em.
    pub size: f32,
    pub hint: bool,
    /// The glyph origin's offset within its pixel, in pixels.
    pub offset: (f32, f32),
    /// How far to widen outlines on each side, in pixels (`THICKEN` uses a
    /// fiftieth of the size).
    pub embolden: f32,
    /// Slant outlines by this many degrees (`FAKE_ITALIC` uses 14).
    pub skew_degrees: Option<f32>,
    /// Draw a color outline in the first palette, else a color bitmap of the
    /// best fitting strike, before the monochrome outline; otherwise only
    /// the monochrome outline.
    pub color: bool,
}

/// Cache for rasterizing with the swash scaler
pub struct SwashCache {
    context: ScaleContext,
    pub image_cache: HashMap<CacheKey, Option<SwashImage>>,
    pub outline_command_cache: HashMap<CacheKey, Option<Box<[swash::zeno::Command]>>>,
}

impl fmt::Debug for SwashCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("SwashCache { .. }")
    }
}

impl SwashCache {
    /// Create a new swash cache
    pub fn new() -> Self {
        Self {
            context: ScaleContext::new(),
            image_cache: HashMap::default(),
            outline_command_cache: HashMap::default(),
        }
    }

    /// Create a swash Image from a cache key, without caching results
    pub fn get_image_uncached(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Option<SwashImage> {
        swash_image(font_system, &mut self.context, cache_key)
    }

    /// Create a swash Image from a cache key, caching results
    pub fn get_image(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> &Option<SwashImage> {
        self.image_cache
            .entry(cache_key)
            .or_insert_with(|| swash_image(font_system, &mut self.context, cache_key))
    }

    /// Draws glyph `glyph_id` of `face` into `image` (cleared first, its
    /// storage reused) with each variation axis set to its value, as
    /// `render` says, without a font system. Returns `false`, leaving
    /// `image` empty, when the face has no drawing for the glyph.
    pub fn render_face_into(
        &mut self,
        face: &SwashFace,
        glyph_id: u16,
        variations: impl IntoIterator<Item = ([u8; 4], f32)>,
        render: &SwashRender,
        image: &mut SwashImage,
    ) -> bool {
        image.clear();
        let mut scaler = self
            .context
            .builder(face.font())
            // Clear the last font's coordinates; see `swash_image`.
            .normalized_coords(core::iter::empty::<i16>())
            .size(render.size)
            .hint(render.hint)
            .variations(variations.into_iter().map(|(tag, value)| swash::Setting {
                tag: swash::Tag::from_be_bytes(tag),
                value,
            }))
            .build();
        let sources: &[Source] = if render.color {
            &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ]
        } else {
            &[Source::Outline]
        };
        Render::new(sources)
            .format(Format::Alpha)
            .embolden(render.embolden)
            .offset(Vector::new(render.offset.0, render.offset.1))
            .transform(render.skew_degrees.map(|degrees| {
                Transform::skew(Angle::from_degrees(degrees), Angle::from_degrees(0.0))
            }))
            .render_into(&mut scaler, glyph_id, image)
    }

    /// Creates outline commands
    pub fn get_outline_commands(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Option<&[swash::zeno::Command]> {
        self.outline_command_cache
            .entry(cache_key)
            .or_insert_with(|| swash_outline_commands(font_system, &mut self.context, cache_key))
            .as_deref()
    }

    /// Creates outline commands, without caching results
    pub fn get_outline_commands_uncached(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Option<Box<[swash::zeno::Command]>> {
        swash_outline_commands(font_system, &mut self.context, cache_key)
    }

    /// Enumerate pixels in an Image, use `with_image` for better performance
    pub fn with_pixels<F: FnMut(i32, i32, Color)>(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
        base: Color,
        mut f: F,
    ) {
        if let Some(image) = self.get_image(font_system, cache_key) {
            let x = image.placement.left;
            let y = -image.placement.top;

            match image.content {
                Content::Mask => {
                    let mut i = 0;
                    for off_y in 0..image.placement.height as i32 {
                        for off_x in 0..image.placement.width as i32 {
                            //TODO: blend base alpha?
                            f(
                                x + off_x,
                                y + off_y,
                                Color((u32::from(image.data[i]) << 24) | base.0 & 0xFF_FF_FF),
                            );
                            i += 1;
                        }
                    }
                }
                Content::Color => {
                    let mut i = 0;
                    for off_y in 0..image.placement.height as i32 {
                        for off_x in 0..image.placement.width as i32 {
                            //TODO: blend base alpha?
                            f(
                                x + off_x,
                                y + off_y,
                                Color::rgba(
                                    image.data[i],
                                    image.data[i + 1],
                                    image.data[i + 2],
                                    image.data[i + 3],
                                ),
                            );
                            i += 4;
                        }
                    }
                }
                Content::SubpixelMask => {
                    log::warn!("TODO: SubpixelMask");
                }
            }
        }
    }
}
