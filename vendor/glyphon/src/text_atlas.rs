use crate::{
    text_render::GlyphonCacheKey, Cache, ContentType, FontSystem, GlyphDetails, GpuCacheStatus,
    RasterizeCustomGlyphRequest, RasterizedCustomGlyph, State, SwashCache,
};
use etagere::{size2, Allocation, BucketedAtlasAllocator};
use lru::LruCache;
use rustc_hash::FxHasher;
use std::{collections::HashSet, hash::BuildHasherDefault};
use wgpu::{
    BindGroup, CommandEncoderDescriptor, DepthStencilState, Device, Extent3d, MultisampleState,
    Origin3d, Queue, RenderPipeline, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

type Hasher = BuildHasherDefault<FxHasher>;

#[allow(dead_code)]
pub(crate) struct InnerAtlas {
    pub kind: Kind,
    pub texture: Texture,
    pub texture_view: TextureView,
    pub packer: BucketedAtlasAllocator,
    pub size: u32,
    pub glyph_cache: LruCache<GlyphonCacheKey, GlyphDetails, Hasher>,
    pub glyphs_in_use: HashSet<GlyphonCacheKey, Hasher>,
    pub max_texture_dimension_2d: u32,
    // quark patch: work counters, summed by `TextAtlas::stats`.
    pub stats: AtlasStats,
}

// quark patch: atlas work counters for tests and devtools.
/// Counts of the work a [`TextAtlas`] has done since it was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasStats {
    /// Glyphs rasterized because the cache did not hold them.
    pub misses: u64,
    /// Cached glyphs dropped to make room for others.
    pub evictions: u64,
    /// Times an atlas texture grew.
    pub growths: u64,
    /// Glyphs rasterized again because a growth could not copy the old
    /// texture into the new one.
    pub rerasterized: u64,
    /// Bytes of glyph images written into atlas textures.
    pub upload_bytes: u64,
}

impl std::ops::Add for AtlasStats {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            misses: self.misses + other.misses,
            evictions: self.evictions + other.evictions,
            growths: self.growths + other.growths,
            rerasterized: self.rerasterized + other.rerasterized,
            upload_bytes: self.upload_bytes + other.upload_bytes,
        }
    }
}

/// Usages of every atlas texture. quark patch: `COPY_SRC`, so growing can
/// copy the old texture into the new one.
const ATLAS_USAGE: TextureUsages = TextureUsages::TEXTURE_BINDING
    .union(TextureUsages::COPY_DST)
    .union(TextureUsages::COPY_SRC);

impl InnerAtlas {
    const INITIAL_SIZE: u32 = 256;

    fn new(state: &State, kind: Kind) -> Self {
        let max_texture_dimension_2d = state.device.limits().max_texture_dimension_2d;
        let size = Self::INITIAL_SIZE.min(max_texture_dimension_2d);

        let packer = BucketedAtlasAllocator::new(size2(size as i32, size as i32));

        // Create a texture to use for our atlas
        let texture = state.device.create_texture(&TextureDescriptor {
            label: Some("glyphon atlas"),
            size: Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: kind.texture_format(),
            usage: ATLAS_USAGE,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&TextureViewDescriptor::default());

        let glyph_cache = LruCache::unbounded_with_hasher(Hasher::default());
        let glyphs_in_use = HashSet::with_hasher(Hasher::default());

        Self {
            kind,
            texture,
            texture_view,
            packer,
            size,
            glyph_cache,
            glyphs_in_use,
            max_texture_dimension_2d,
            stats: AtlasStats::default(),
        }
    }

    pub(crate) fn try_allocate(&mut self, width: usize, height: usize) -> Option<Allocation> {
        let size = size2(width as i32, height as i32);

        loop {
            let allocation = self.packer.allocate(size);

            if allocation.is_some() {
                return allocation;
            }

            // Try to free least recently used allocation
            let (mut key, mut value) = self.glyph_cache.peek_lru()?;

            // Find a glyph with an actual size
            while value.atlas_id.is_none() {
                // All sized glyphs are in use, cache is full
                if self.glyphs_in_use.contains(key) {
                    return None;
                }

                let _ = self.glyph_cache.pop_lru();
                self.stats.evictions += 1;

                (key, value) = self.glyph_cache.peek_lru()?;
            }

            // All sized glyphs are in use, cache is full
            if self.glyphs_in_use.contains(key) {
                return None;
            }

            let (_, value) = self.glyph_cache.pop_lru().unwrap();
            self.packer.deallocate(value.atlas_id.unwrap());
            self.stats.evictions += 1;
        }
    }

    pub fn num_channels(&self) -> usize {
        self.kind.num_channels()
    }

    pub(crate) fn grow(
        &mut self,
        state: &State,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        scale_factor: f32,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
    ) -> bool {
        if self.size >= self.max_texture_dimension_2d {
            return false;
        }

        // Grow each dimension by a factor of 2. The growth factor was chosen to match the growth
        // factor of `Vec`.`
        const GROWTH_FACTOR: u32 = 2;
        let new_size = (self.size * GROWTH_FACTOR).min(self.max_texture_dimension_2d);

        self.packer.grow(size2(new_size as i32, new_size as i32));

        // quark patch: kept to copy from below.
        let old_texture = self.texture.clone();

        // Create a texture to use for our atlas
        self.texture = state.device.create_texture(&TextureDescriptor {
            label: Some("glyphon atlas"),
            size: Extent3d {
                width: new_size,
                height: new_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: self.kind.texture_format(),
            usage: ATLAS_USAGE,
            view_formats: &[],
        });
        self.stats.growths += 1;

        // quark patch: the allocations stayed put, so copying the old
        // texture into the new one's corner keeps every cached glyph without
        // rasterizing it again. Same format and sample count, so the copy is
        // valid whenever the old texture allows copying from it, as every
        // texture made here does. Queued glyph writes reach the old texture
        // before this submission runs, and later ones land on the new
        // texture before the caller's next submission.
        if old_texture.usage().contains(TextureUsages::COPY_SRC) {
            let mut encoder = state
                .device
                .create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("glyphon atlas growth"),
                });
            encoder.copy_texture_to_texture(
                old_texture.as_image_copy(),
                self.texture.as_image_copy(),
                Extent3d {
                    width: self.size,
                    height: self.size,
                    depth_or_array_layers: 1,
                },
            );
            state.queue.submit([encoder.finish()]);
            self.texture_view = self.texture.create_view(&TextureViewDescriptor::default());
            self.size = new_size;
            return true;
        }

        // Re-upload glyphs
        for (&cache_key, glyph) in &self.glyph_cache {
            let (x, y) = match glyph.gpu_cache {
                GpuCacheStatus::InAtlas { x, y, .. } => (x, y),
                GpuCacheStatus::SkipRasterization => continue,
            };

            let (image_data, width, height) = match cache_key {
                GlyphonCacheKey::Text(cache_key) => {
                    let image = cache.get_image_uncached(font_system, cache_key).unwrap();
                    let width = image.placement.width as usize;
                    let height = image.placement.height as usize;

                    (image.data, width, height)
                }
                GlyphonCacheKey::Custom(cache_key) => {
                    let input = RasterizeCustomGlyphRequest {
                        id: cache_key.glyph_id,
                        width: cache_key.width,
                        height: cache_key.height,
                        x_bin: cache_key.x_bin,
                        y_bin: cache_key.y_bin,
                        scale: scale_factor,
                    };

                    let Some(rasterized_glyph) = (rasterize_custom_glyph)(input) else {
                        panic!("Custom glyph rasterizer returned `None` when it previously returned `Some` for the same input {:?}", &input);
                    };

                    // Sanity checks on the rasterizer output
                    rasterized_glyph.validate(&input, Some(self.kind.as_content_type()));

                    (
                        rasterized_glyph.data,
                        cache_key.width as usize,
                        cache_key.height as usize,
                    )
                }
            };

            self.stats.rerasterized += 1;
            self.stats.upload_bytes += image_data.len() as u64;
            state.queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: x as u32,
                        y: y as u32,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image_data,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width as u32 * self.kind.num_channels() as u32),
                    rows_per_image: None,
                },
                Extent3d {
                    width: width as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.texture_view = self.texture.create_view(&TextureViewDescriptor::default());
        self.size = new_size;

        true
    }

    fn trim(&mut self) {
        self.glyphs_in_use.clear();
    }

    // quark patch: see `TextAtlas::clear`.
    fn clear(&mut self) {
        self.packer.clear();
        self.glyph_cache.clear();
        self.glyphs_in_use.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Mask,
    Color { srgb: bool },
}

impl Kind {
    fn num_channels(self) -> usize {
        match self {
            Kind::Mask => 1,
            Kind::Color { .. } => 4,
        }
    }

    fn texture_format(self) -> wgpu::TextureFormat {
        match self {
            Kind::Mask => TextureFormat::R8Unorm,
            Kind::Color { srgb } => {
                if srgb {
                    TextureFormat::Rgba8UnormSrgb
                } else {
                    TextureFormat::Rgba8Unorm
                }
            }
        }
    }

    fn as_content_type(&self) -> ContentType {
        match self {
            Self::Mask => ContentType::Mask,
            Self::Color { .. } => ContentType::Color,
        }
    }
}

/// The color mode of a [`TextAtlas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Accurate color management.
    ///
    /// This mode will use a proper sRGB texture for colored glyphs. This will
    /// produce physically accurate color blending when rendering.
    Accurate,

    /// Web color management.
    ///
    /// This mode reproduces the color management strategy used in the Web and
    /// implemented by browsers.
    ///
    /// This entails storing glyphs colored using the sRGB color space in a
    /// linear RGB texture. Blending will not be physically accurate, but will
    /// produce the same results as most UI toolkits.
    ///
    /// This mode should be used to render to a linear RGB texture containing
    /// sRGB colors.
    Web,
}

/// An atlas containing a cache of rasterized glyphs that can be rendered.
pub struct TextAtlas {
    cache: Cache,
    pub(crate) bind_group: BindGroup,
    pub(crate) color_atlas: InnerAtlas,
    pub(crate) mask_atlas: InnerAtlas,
    pub(crate) format: TextureFormat,
    pub(crate) color_mode: ColorMode,
    // quark patch: see `TextAtlas::epoch`.
    clears: u64,
}

impl TextAtlas {
    /// Creates a new [`TextAtlas`].
    pub fn new(device: &Device, queue: &Queue, cache: &Cache, format: TextureFormat) -> Self {
        Self::with_color_mode(device, queue, cache, format, ColorMode::Accurate)
    }

    /// Creates a new [`TextAtlas`] with the given [`ColorMode`].
    pub fn with_color_mode(
        device: &Device,
        queue: &Queue,
        cache: &Cache,
        format: TextureFormat,
        color_mode: ColorMode,
    ) -> Self {
        let state = State { device, queue };
        let color_atlas = InnerAtlas::new(
            &state,
            Kind::Color {
                srgb: match color_mode {
                    ColorMode::Accurate => true,
                    ColorMode::Web => false,
                },
            },
        );
        let mask_atlas = InnerAtlas::new(&state, Kind::Mask);

        let bind_group = cache.create_atlas_bind_group(
            device,
            &color_atlas.texture_view,
            &mask_atlas.texture_view,
        );

        Self {
            cache: cache.clone(),
            bind_group,
            color_atlas,
            mask_atlas,
            format,
            color_mode,
            clears: 0,
        }
    }

    pub fn trim(&mut self) {
        self.mask_atlas.trim();
        self.color_atlas.trim();
    }

    // quark patch: forgetting every glyph.
    /// Drops every cached glyph, in use or not, keeping the textures and
    /// their size. Glyphs are keyed by font id, so an atlas that keeps
    /// drawing after the font database is replaced must be cleared: the
    /// new database numbers its faces from the start again.
    pub fn clear(&mut self) {
        self.mask_atlas.clear();
        self.color_atlas.clear();
        self.clears += 1;
    }

    // quark patch: when prepared vertices go stale.
    /// Changes whenever a cached glyph may have left its place: an eviction
    /// or a [`Self::clear`]. Growth keeps every glyph where it was. Vertices
    /// a renderer prepared at one epoch draw the same glyphs while the
    /// epoch is unchanged, so they may be drawn again without preparing.
    pub fn epoch(&self) -> u64 {
        self.mask_atlas.stats.evictions + self.color_atlas.stats.evictions + self.clears
    }

    // quark patch: atlas work counters.
    /// Work done by both atlas textures since this atlas was created.
    pub fn stats(&self) -> AtlasStats {
        self.mask_atlas.stats + self.color_atlas.stats
    }

    pub(crate) fn grow(
        &mut self,
        state: &State,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        content_type: ContentType,
        scale_factor: f32,
        rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> bool {
        let did_grow = match content_type {
            ContentType::Mask => self.mask_atlas.grow(
                state,
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
            ),
            ContentType::Color => self.color_atlas.grow(
                state,
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
            ),
        };

        if did_grow {
            self.rebind(state.device);
        }

        did_grow
    }

    pub(crate) fn inner_for_content_mut(&mut self, content_type: ContentType) -> &mut InnerAtlas {
        match content_type {
            ContentType::Color => &mut self.color_atlas,
            ContentType::Mask => &mut self.mask_atlas,
        }
    }

    pub(crate) fn get_or_create_pipeline(
        &self,
        device: &Device,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> RenderPipeline {
        self.cache
            .get_or_create_pipeline(device, self.format, multisample, depth_stencil)
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.bind_group = self.cache.create_atlas_bind_group(
            device,
            &self.color_atlas.texture_view,
            &self.mask_atlas.texture_view,
        );
    }
}
