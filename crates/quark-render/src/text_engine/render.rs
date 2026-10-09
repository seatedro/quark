use super::{
    ContentType, GlyphDetails, GlyphToRender, GpuCacheStatus, PositionedGlyph, PrepareError,
    RenderError, State, TextArea, TextAtlas, TextBounds, Viewport,
};
use quark_text::cosmic_text::{CacheKey, Color, FontSystem, LayoutRun, SwashCache, SwashContent};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, COPY_BUFFER_ALIGNMENT, DepthStencilState, Device,
    Extent3d, MultisampleState, Origin3d, Queue, RenderPass, RenderPipeline, TexelCopyBufferLayout,
    TexelCopyTextureInfo, TextureAspect,
};

/// Most [`GlyphFill`]s one renderer holds.
pub(crate) const MAX_GLYPH_FILLS: usize = 64;

/// A coordinate-dependent color for monochrome glyphs, evaluated per pixel
/// in target pixels: `kind` 1 is a linear gradient from `color_a` at the
/// axis start to `color_b` at its end, clamped; `kind` 2 is a highlight
/// band of half width `params[1]` pixels centered `params[2]` pixels along
/// the axis (from its start, in the axis direction), `color_b` at its center
/// fading to `color_a`. Colors are straight-alpha sRGB-encoded channels in
/// 0..1; a linear target decodes the result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GlyphFill {
    /// Axis start and end: `[x0, y0, x1, y1]`.
    pub axis: [f32; 4],
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    /// `[kind, band half width, band center, 0]`.
    pub params: [f32; 4],
}

/// Draws glyph instances, prepared from the atlas, into a render pass.
pub(crate) struct TextRenderer {
    vertex_buffer: Buffer,
    vertex_buffer_size: u64,
    pipeline: RenderPipeline,
    /// The target format `pipeline` draws to, and the fills.
    format: wgpu::TextureFormat,
    multisample: MultisampleState,
    depth_stencil: Option<DepthStencilState>,
    fills: Vec<GlyphFill>,
    fills_dirty: bool,
    fills_buffer: Buffer,
    fills_bind_group: wgpu::BindGroup,
    glyph_vertices: Vec<GlyphToRender>,
}

impl TextRenderer {
    pub(crate) fn new(
        atlas: &mut TextAtlas,
        device: &Device,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> Self {
        let vertex_buffer_size = next_copy_buffer_size(4096);
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("quark text vertices"),
            size: vertex_buffer_size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = atlas.get_or_create_pipeline(device, multisample, depth_stencil.clone());
        let fills_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("quark text fills"),
            size: (MAX_GLYPH_FILLS * std::mem::size_of::<GlyphFill>()) as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fills_bind_group = atlas.create_fills_bind_group(device, &fills_buffer);

        Self {
            vertex_buffer,
            vertex_buffer_size,
            pipeline,
            format: atlas.format,
            multisample,
            depth_stencil,
            fills: Vec::new(),
            fills_dirty: false,
            fills_buffer,
            fills_bind_group,
            glyph_vertices: Vec::new(),
        }
    }

    /// Draw later passes into targets of `format` (the atlas's format at
    /// first), e.g. a non-sRGB view holding encoded colors.
    pub(crate) fn set_target_format(
        &mut self,
        atlas: &TextAtlas,
        device: &Device,
        format: wgpu::TextureFormat,
    ) {
        if self.format != format {
            self.pipeline = atlas.cache.get_or_create_pipeline(
                device,
                format,
                self.multisample,
                self.depth_stencil.clone(),
            );
            self.format = format;
        }
    }

    /// Sets the fills glyphs prepared with a nonzero
    /// [`PositionedGlyph::fill`] read; at most [`MAX_GLYPH_FILLS`] are kept.
    /// [`Self::upload_fills`] copies them to the GPU when they changed, so
    /// a moving shimmer costs one small write a frame and no preparing.
    pub(crate) fn set_fills(&mut self, fills: &[GlyphFill]) {
        let fills = &fills[..fills.len().min(MAX_GLYPH_FILLS)];
        if self.fills != fills {
            self.fills.clear();
            self.fills.extend_from_slice(fills);
            self.fills_dirty = true;
        }
    }

    /// Writes the fills set since the last upload.
    pub(crate) fn upload_fills(&mut self, queue: &Queue) {
        if std::mem::take(&mut self.fills_dirty) && !self.fills.is_empty() {
            queue.write_buffer(&self.fills_buffer, 0, bytemuck::cast_slice(&self.fills));
        }
    }

    /// Prepares the glyphs of `text_areas`' visible layout runs, in order,
    /// and uploads them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
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

        for text_area in text_areas {
            let bounds = GlyphBounds::clipped(text_area.bounds, resolution);

            let is_run_visible = |run: &LayoutRun| {
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
                            cache_key: physical_glyph.cache_key,
                            scale_factor: text_area.scale,
                            paint: 0,
                        },
                        bounds,
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_glyphs(
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
                    cache_key: glyph.cache_key,
                    paint: u32::from(glyph.fill)
                        | glyph
                            .backdrop
                            .map_or(0, |luminance| 0x100 | u32::from(luminance) << 16),
                },
                GlyphBounds::clipped(glyph.bounds, resolution),
            )? {
                self.glyph_vertices.push(glyph_to_render);
            }
        }

        Ok(())
    }

    /// Copies the prepared vertices to the GPU, growing the buffer if needed.
    /// [`Self::prepare`] and its variants call this themselves.
    pub(crate) fn upload(&mut self, device: &Device, queue: &Queue) {
        let will_render = !self.glyph_vertices.is_empty();
        if !will_render {
            return;
        }

        let vertices_raw: &[u8] = bytemuck::cast_slice(&self.glyph_vertices);

        if self.vertex_buffer_size >= vertices_raw.len() as u64 {
            queue.write_buffer(&self.vertex_buffer, 0, vertices_raw);
        } else {
            self.vertex_buffer.destroy();

            let (buffer, buffer_size) = create_oversized_buffer(
                device,
                Some("quark text vertices"),
                vertices_raw,
                BufferUsages::VERTEX | BufferUsages::COPY_DST,
            );

            self.vertex_buffer = buffer;
            self.vertex_buffer_size = buffer_size;
        }
    }

    /// Draws the glyphs last prepared, moving every glyph by the viewport's
    /// draw offset `slot` (see [`Viewport::set_draw_offsets`]), clip included.
    /// Drawing glyphs prepared earlier where the same glyphs moved by
    /// whole pixels, with their bounds moved alike and inside the viewport
    /// both times, would be prepared, needs no preparing or uploading
    /// again. `slot` must be below [`super::MAX_DRAW_OFFSETS`].
    pub(crate) fn render_at(
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
        pass.set_bind_group(2, &self.fills_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        // The vertex shader reads the slot from the vertex index.
        pass.draw(slot * 4..slot * 4 + 4, 0..self.glyph_vertices.len() as u32);

        Ok(())
    }
}

/// Bit 0 of the vertex's upper content type: decode the glyph's color.
const CONVERT_TO_LINEAR: u16 = 1;

/// The upper half of the vertex's content type for a glyph whose cache key
/// asks for linear correction: bit 1 set, and the background's
/// sRGB-encoded luminance in bits 8 to 15. Bit 0 is the color conversion.
fn linear_correction(key: &CacheKey) -> u16 {
    key.flags
        .blend_background()
        .map_or(0, |luminance| 2 | u16::from(luminance) << 8)
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
    cache_key: CacheKey,
    paint: u32,
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
    fn clipped(bounds: TextBounds, resolution: super::Resolution) -> Self {
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
fn text_glyph_image(system: &mut GlyphSystem, cache_key: CacheKey) -> Option<GetGlyphImageResult> {
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

fn prepare_glyph(
    state: &State,
    system: &mut GlyphSystem,
    metadata: GlyphMetadata,
    bounds: GlyphBounds,
) -> Result<Option<GlyphToRender>, PrepareError> {
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
            let Some(image) = text_glyph_image(system, metadata.cache_key) else {
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

    Ok(Some(GlyphToRender {
        pos: [x, y],
        dim: [width as u16, height as u16],
        uv: [atlas_x, atlas_y],
        color: metadata.color.0,
        // Colors are sRGB-encoded; the shader decodes them unless the
        // target is encoded.
        content_type_with_srgb: [
            content_type as u16,
            CONVERT_TO_LINEAR | linear_correction(&metadata.cache_key),
        ],
        depth: 0.0,
        paint: metadata.paint,
    }))
}
