use crate::{
    custom_glyph::CustomGlyphCacheKey, ColorMode, ContentType, FontSystem, GlyphDetails,
    GlyphToRender, GpuCacheStatus, PositionedGlyph, PrepareError, RasterizeCustomGlyphRequest,
    RasterizedCustomGlyph, RenderError, State, SwashCache, SwashContent, TextArea, TextAtlas,
    TextBounds, Viewport,
};
use cosmic_text::{Color, SubpixelBin};
use std::slice;
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, DepthStencilState, Device, Extent3d, MultisampleState,
    Origin3d, Queue, RenderPass, RenderPipeline, TexelCopyBufferLayout, TexelCopyTextureInfo,
    TextureAspect, COPY_BUFFER_ALIGNMENT,
};

/// A text renderer that uses cached glyphs to render text into an existing render pass.
pub struct TextRenderer {
    vertex_buffer: Buffer,
    vertex_buffer_size: u64,
    pipeline: RenderPipeline,
    glyph_vertices: Vec<GlyphToRender>,
}

impl TextRenderer {
    /// Creates a new `TextRenderer`.
    pub fn new(
        atlas: &mut TextAtlas,
        device: &Device,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> Self {
        let vertex_buffer_size = next_copy_buffer_size(4096);
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyphon vertices"),
            size: vertex_buffer_size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = atlas.get_or_create_pipeline(device, multisample, depth_stencil);

        Self {
            vertex_buffer,
            vertex_buffer_size,
            pipeline,
            glyph_vertices: Vec::new(),
        }
    }

    /// Prepares all of the provided text areas for rendering.
    pub fn prepare<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
    ) -> Result<(), PrepareError> {
        self.prepare_with_depth_and_custom(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            text_areas,
            cache,
            zero_depth,
            |_| None,
        )
    }

    /// Prepares all of the provided text areas for rendering.
    pub fn prepare_with_depth<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        metadata_to_depth: impl FnMut(usize) -> f32,
    ) -> Result<(), PrepareError> {
        self.prepare_with_depth_and_custom(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            text_areas,
            cache,
            metadata_to_depth,
            |_| None,
        )
    }

    /// Prepares all of the provided text areas for rendering.
    pub fn prepare_with_custom<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> Result<(), PrepareError> {
        self.prepare_with_depth_and_custom(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            text_areas,
            cache,
            zero_depth,
            rasterize_custom_glyph,
        )
    }

    /// Prepares all of the provided text areas for rendering.
    pub fn prepare_with_depth_and_custom<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        mut metadata_to_depth: impl FnMut(usize) -> f32,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
    ) -> Result<(), PrepareError> {
        self.glyph_vertices.clear();

        let state = State { device, queue };
        let mut system = GlyphSystem {
            atlas,
            cache,
            font_system,
        };
        let resolution = viewport.resolution();

        for text_area in text_areas {
            let bounds = GlyphBounds::clipped(text_area.bounds, resolution);

            for glyph in text_area.custom_glyphs.iter() {
                let x = text_area.left + (glyph.left * text_area.scale);
                let y = text_area.top + (glyph.top * text_area.scale);
                let width = (glyph.width * text_area.scale).round() as u16;
                let height = (glyph.height * text_area.scale).round() as u16;

                let (x, y, x_bin, y_bin) = if glyph.snap_to_physical_pixel {
                    (
                        x.round() as i32,
                        y.round() as i32,
                        SubpixelBin::Zero,
                        SubpixelBin::Zero,
                    )
                } else {
                    let (x, x_bin) = SubpixelBin::new(x);
                    let (y, y_bin) = SubpixelBin::new(y);
                    (x, y, x_bin, y_bin)
                };

                let cache_key = GlyphonCacheKey::Custom(CustomGlyphCacheKey {
                    glyph_id: glyph.id,
                    width,
                    height,
                    x_bin,
                    y_bin,
                });

                let color = glyph.color.unwrap_or(text_area.default_color);

                if let Some(glyph_to_render) = prepare_glyph(
                    &state,
                    &mut system,
                    GlyphMetadata {
                        x,
                        y,
                        line_y: 0.0,
                        scale_factor: text_area.scale,
                        color,
                        metadata: glyph.metadata,
                        cache_key,
                    },
                    bounds,
                    |_system, rasterize_custom_glyph| -> Option<GetGlyphImageResult> {
                        if width == 0 || height == 0 {
                            return None;
                        }

                        let input = RasterizeCustomGlyphRequest {
                            id: glyph.id,
                            width,
                            height,
                            x_bin,
                            y_bin,
                            scale: text_area.scale,
                        };

                        let output = (rasterize_custom_glyph)(input)?;

                        output.validate(&input, None);

                        Some(GetGlyphImageResult {
                            content_type: output.content_type,
                            top: 0,
                            left: 0,
                            width,
                            height,
                            data: output.data,
                        })
                    },
                    &mut metadata_to_depth,
                    &mut rasterize_custom_glyph,
                )? {
                    self.glyph_vertices.push(glyph_to_render);
                }
            }

            let is_run_visible = |run: &cosmic_text::LayoutRun| {
                let start_y_physical = (text_area.top + (run.line_top * text_area.scale)) as i32;
                let end_y_physical = start_y_physical + (run.line_height * text_area.scale) as i32;

                start_y_physical <= text_area.bounds.bottom
                    && text_area.bounds.top <= end_y_physical
            };

            let layout_runs = text_area
                .buffer
                .layout_runs()
                .skip_while(|run| !is_run_visible(run))
                .take_while(is_run_visible);

            for run in layout_runs {
                for glyph in run.glyphs.iter() {
                    let physical_glyph =
                        glyph.physical((text_area.left, text_area.top), text_area.scale);

                    let color = match glyph.color_opt {
                        Some(some) => some,
                        None => text_area.default_color,
                    };

                    if let Some(glyph_to_render) = prepare_glyph(
                        &state,
                        &mut system,
                        GlyphMetadata {
                            x: physical_glyph.x,
                            y: physical_glyph.y,
                            line_y: run.line_y,
                            color,
                            metadata: glyph.metadata,
                            cache_key: GlyphonCacheKey::Text(physical_glyph.cache_key),
                            scale_factor: text_area.scale,
                        },
                        bounds,
                        |system, _rasterize_custom_glyph| {
                            text_glyph_image(system, physical_glyph.cache_key)
                        },
                        &mut metadata_to_depth,
                        &mut rasterize_custom_glyph,
                    )? {
                        self.glyph_vertices.push(glyph_to_render);
                    }
                }
            }
        }

        self.upload(device, queue);
        Ok(())
    }

    /// Prepares glyphs that are already positioned, in the order given.
    /// Unlike [`Self::prepare`], nothing is read from a [`cosmic_text::Buffer`]:
    /// each glyph brings its own position, color, and clip. Call
    /// [`Self::upload`] before rendering; keeping the GPU copy separate lets
    /// a caller budget preparation apart from the driver's staging.
    pub fn prepare_glyphs(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        glyphs: impl IntoIterator<Item = PositionedGlyph>,
        cache: &mut SwashCache,
    ) -> Result<(), PrepareError> {
        self.glyph_vertices.clear();

        let state = State { device, queue };
        let mut system = GlyphSystem {
            atlas,
            cache,
            font_system,
        };
        let resolution = viewport.resolution();

        for glyph in glyphs {
            if let Some(glyph_to_render) = prepare_glyph(
                &state,
                &mut system,
                GlyphMetadata {
                    x: glyph.x,
                    y: glyph.y,
                    line_y: 0.0,
                    scale_factor: 1.0,
                    color: glyph.color,
                    metadata: 0,
                    cache_key: GlyphonCacheKey::Text(glyph.cache_key),
                },
                GlyphBounds::clipped(glyph.bounds, resolution),
                |system, _rasterize_custom_glyph| text_glyph_image(system, glyph.cache_key),
                zero_depth,
                |_| None,
            )? {
                self.glyph_vertices.push(glyph_to_render);
            }
        }

        Ok(())
    }

    /// Copies the prepared vertices to the GPU, growing the buffer if needed.
    /// [`Self::prepare`] and its variants call this themselves.
    pub fn upload(&mut self, device: &Device, queue: &Queue) {
        let will_render = !self.glyph_vertices.is_empty();
        if !will_render {
            return;
        }

        let vertices = self.glyph_vertices.as_slice();
        let vertices_raw = unsafe {
            slice::from_raw_parts(
                vertices as *const _ as *const u8,
                std::mem::size_of_val(vertices),
            )
        };

        if self.vertex_buffer_size >= vertices_raw.len() as u64 {
            queue.write_buffer(&self.vertex_buffer, 0, vertices_raw);
        } else {
            self.vertex_buffer.destroy();

            let (buffer, buffer_size) = create_oversized_buffer(
                device,
                Some("glyphon vertices"),
                vertices_raw,
                BufferUsages::VERTEX | BufferUsages::COPY_DST,
            );

            self.vertex_buffer = buffer;
            self.vertex_buffer_size = buffer_size;
        }
    }

    /// Renders all layouts that were previously provided to `prepare`.
    pub fn render(
        &self,
        atlas: &TextAtlas,
        viewport: &Viewport,
        pass: &mut RenderPass<'_>,
    ) -> Result<(), RenderError> {
        self.render_at(atlas, viewport, pass, 0)
    }

    /// Like [`Self::render`], moving every glyph by the viewport's draw
    /// offset `slot` (see [`Viewport::set_draw_offsets`]), clip included.
    /// Drawing glyphs prepared earlier where the same glyphs moved by
    /// whole pixels, with their bounds moved alike and inside the viewport
    /// both times, would be prepared, needs no preparing or uploading
    /// again. `slot` must be below [`crate::MAX_DRAW_OFFSETS`].
    pub fn render_at(
        &self,
        atlas: &TextAtlas,
        viewport: &Viewport,
        pass: &mut RenderPass<'_>,
        slot: u32,
    ) -> Result<(), RenderError> {
        if self.glyph_vertices.is_empty() {
            return Ok(());
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        // The vertex shader reads the slot from the vertex index.
        pass.draw(slot * 4..slot * 4 + 4, 0..self.glyph_vertices.len() as u32);

        Ok(())
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TextColorConversion {
    None = 0,
    ConvertToLinear = 1,
}

/// The upper half of the vertex's content type for a glyph whose cache key
/// asks for linear correction: bit 1 set, and the background's
/// sRGB-encoded luminance in bits 8 to 15. Bit 0 is the color conversion.
fn linear_correction(key: &GlyphonCacheKey) -> u16 {
    match key {
        GlyphonCacheKey::Text(key) => key
            .flags
            .blend_background()
            .map_or(0, |luminance| 2 | u16::from(luminance) << 8),
        GlyphonCacheKey::Custom(_) => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GlyphonCacheKey {
    Text(cosmic_text::CacheKey),
    Custom(CustomGlyphCacheKey),
}

fn next_copy_buffer_size(size: u64) -> u64 {
    let align_mask = COPY_BUFFER_ALIGNMENT - 1;
    ((size.next_power_of_two() + align_mask) & !align_mask).max(COPY_BUFFER_ALIGNMENT)
}

fn create_oversized_buffer(
    device: &Device,
    label: Option<&str>,
    contents: &[u8],
    usage: BufferUsages,
) -> (Buffer, u64) {
    let size = next_copy_buffer_size(contents.len() as u64);
    let buffer = device.create_buffer(&BufferDescriptor {
        label,
        size,
        usage,
        mapped_at_creation: true,
    });
    buffer.slice(..).get_mapped_range_mut()[..contents.len()].copy_from_slice(contents);
    buffer.unmap();
    (buffer, size)
}

fn zero_depth(_: usize) -> f32 {
    0f32
}

struct GetGlyphImageResult {
    content_type: ContentType,
    top: i16,
    left: i16,
    width: u16,
    height: u16,
    data: Vec<u8>,
}

struct GlyphMetadata {
    x: i32,
    y: i32,
    line_y: f32,
    scale_factor: f32,
    color: Color,
    metadata: usize,
    cache_key: GlyphonCacheKey,
}

#[derive(Clone, Copy)]
struct Bounds {
    min: i32,
    max: i32,
}

#[derive(Clone, Copy)]
struct GlyphBounds {
    x: Bounds,
    y: Bounds,
}

impl GlyphBounds {
    /// `bounds` clipped to the viewport.
    fn clipped(bounds: TextBounds, resolution: crate::Resolution) -> Self {
        Self {
            x: Bounds {
                min: bounds.left.max(0),
                max: bounds.right.min(resolution.width as i32),
            },
            y: Bounds {
                min: bounds.top.max(0),
                max: bounds.bottom.min(resolution.height as i32),
            },
        }
    }
}

/// Rasterizes a font glyph for the atlas.
fn text_glyph_image(
    system: &mut GlyphSystem,
    cache_key: cosmic_text::CacheKey,
) -> Option<GetGlyphImageResult> {
    let image = system
        .cache
        .get_image_uncached(system.font_system, cache_key)?;

    let content_type = match image.content {
        SwashContent::Color => ContentType::Color,
        SwashContent::Mask => ContentType::Mask,
        SwashContent::SubpixelMask => {
            // Not implemented yet, but don't panic if this happens.
            ContentType::Mask
        }
    };

    Some(GetGlyphImageResult {
        content_type,
        top: image.placement.top as i16,
        left: image.placement.left as i16,
        width: image.placement.width as u16,
        height: image.placement.height as u16,
        data: image.data,
    })
}

struct GlyphSystem<'a> {
    atlas: &'a mut TextAtlas,
    cache: &'a mut SwashCache,
    font_system: &'a mut FontSystem,
}

fn prepare_glyph<R>(
    state: &State,
    system: &mut GlyphSystem,
    metadata: GlyphMetadata,
    bounds: GlyphBounds,
    get_glyph_image: impl FnOnce(&mut GlyphSystem, &mut R) -> Option<GetGlyphImageResult>,
    mut metadata_to_depth: impl FnMut(usize) -> f32,
    mut rasterize_custom_glyph: R,
) -> Result<Option<GlyphToRender>, PrepareError>
where
    R: FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
{
    let details =
        if let Some(details) = system.atlas.mask_atlas.glyph_cache.get(&metadata.cache_key) {
            system
                .atlas
                .mask_atlas
                .glyphs_in_use
                .insert(metadata.cache_key);
            details
        } else if let Some(details) = system
            .atlas
            .color_atlas
            .glyph_cache
            .get(&metadata.cache_key)
        {
            system
                .atlas
                .color_atlas
                .glyphs_in_use
                .insert(metadata.cache_key);
            details
        } else {
            let Some(image) = (get_glyph_image)(system, &mut rasterize_custom_glyph) else {
                return Ok(None);
            };

            let should_rasterize = image.width > 0 && image.height > 0;

            let (gpu_cache, atlas_id, inner) = if should_rasterize {
                let mut inner = system.atlas.inner_for_content_mut(image.content_type);

                // Find a position in the packer
                let allocation = loop {
                    match inner.try_allocate(image.width as usize, image.height as usize) {
                        Some(a) => break a,
                        None => {
                            if !system.atlas.grow(
                                state,
                                system.font_system,
                                system.cache,
                                image.content_type,
                                metadata.scale_factor,
                                &mut rasterize_custom_glyph,
                            ) {
                                return Err(PrepareError::AtlasFull);
                            }

                            inner = system.atlas.inner_for_content_mut(image.content_type);
                        }
                    }
                };
                let atlas_min = allocation.rectangle.min;

                inner.stats.upload_bytes += image.data.len() as u64;
                state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &inner.texture,
                        mip_level: 0,
                        origin: Origin3d {
                            x: atlas_min.x as u32,
                            y: atlas_min.y as u32,
                            z: 0,
                        },
                        aspect: TextureAspect::All,
                    },
                    &image.data,
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(image.width as u32 * inner.num_channels() as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: image.width as u32,
                        height: image.height as u32,
                        depth_or_array_layers: 1,
                    },
                );

                (
                    GpuCacheStatus::InAtlas {
                        x: atlas_min.x as u16,
                        y: atlas_min.y as u16,
                        content_type: image.content_type,
                    },
                    Some(allocation.id),
                    inner,
                )
            } else {
                let inner = &mut system.atlas.color_atlas;
                (GpuCacheStatus::SkipRasterization, None, inner)
            };

            inner.stats.misses += 1;
            inner.glyphs_in_use.insert(metadata.cache_key);
            // Insert the glyph into the cache and return the details reference
            inner
                .glyph_cache
                .get_or_insert(metadata.cache_key, || GlyphDetails {
                    width: image.width,
                    height: image.height,
                    gpu_cache,
                    atlas_id,
                    top: image.top,
                    left: image.left,
                })
        };

    let mut x = metadata.x + details.left as i32;
    let mut y =
        (metadata.line_y * metadata.scale_factor).round() as i32 + metadata.y - details.top as i32;

    let (mut atlas_x, mut atlas_y, content_type) = match details.gpu_cache {
        GpuCacheStatus::InAtlas { x, y, content_type } => (x, y, content_type),
        GpuCacheStatus::SkipRasterization => return Ok(None),
    };

    let mut width = details.width as i32;
    let mut height = details.height as i32;

    // Starts beyond right edge or ends beyond left edge
    let max_x = x + width;
    if x > bounds.x.max || max_x < bounds.x.min {
        return Ok(None);
    }

    // Starts beyond bottom edge or ends beyond top edge
    let max_y = y + height;
    if y > bounds.y.max || max_y < bounds.y.min {
        return Ok(None);
    }

    // Clip left ege
    if x < bounds.x.min {
        let right_shift = bounds.x.min - x;

        x = bounds.x.min;
        width = max_x - bounds.x.min;
        atlas_x += right_shift as u16;
    }

    // Clip right edge
    if x + width > bounds.x.max {
        width = bounds.x.max - x;
    }

    // Clip top edge
    if y < bounds.y.min {
        let bottom_shift = bounds.y.min - y;

        y = bounds.y.min;
        height = max_y - bounds.y.min;
        atlas_y += bottom_shift as u16;
    }

    // Clip bottom edge
    if y + height > bounds.y.max {
        height = bounds.y.max - y;
    }

    let depth = metadata_to_depth(metadata.metadata);

    Ok(Some(GlyphToRender {
        pos: [x, y],
        dim: [width as u16, height as u16],
        uv: [atlas_x, atlas_y],
        color: metadata.color.0,
        content_type_with_srgb: [
            content_type as u16,
            match system.atlas.color_mode {
                ColorMode::Accurate => TextColorConversion::ConvertToLinear,
                ColorMode::Web => TextColorConversion::None,
            } as u16
                | linear_correction(&metadata.cache_key),
        ],
        depth,
    }))
}
