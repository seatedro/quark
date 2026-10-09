use std::ops::Range;

use super::atlas::{GlyphAtlas, GlyphHandle};
use super::{
    GlyphToRender, PositionedGlyph, PrepareError, RenderError, TextArea, TextBounds, Viewport,
};
use quark_text::cosmic_text::{CacheKey, Color, FontSystem, LayoutRun};
use wgpu::{
    BindGroup, Buffer, BufferDescriptor, BufferUsages, COPY_BUFFER_ALIGNMENT, DepthStencilState,
    Device, MultisampleState, Queue, RenderPass, RenderPipeline,
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

/// Consecutive glyph instances drawn with one bind group: at most one
/// color page and one mask page, and one overflow ordinal.
struct DrawSegment {
    instances: Range<u32>,
    color: Option<u32>,
    mask: Option<u32>,
    ordinal: u32,
    bind: Option<BindGroup>,
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
    segments: Vec<DrawSegment>,
    /// The atlas entries the instances draw from, each once, sorted.
    handles: Vec<GlyphHandle>,
    /// Some glyph sits in an overflow page, filled for one frame only.
    overflowed: bool,
}

impl TextRenderer {
    pub(crate) fn new(
        atlas: &GlyphAtlas,
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

        let pipeline = atlas.cache.get_or_create_pipeline(
            device,
            atlas.format,
            multisample,
            depth_stencil.clone(),
        );
        let fills_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("quark text fills"),
            size: (MAX_GLYPH_FILLS * std::mem::size_of::<GlyphFill>()) as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fills_bind_group = atlas.cache.create_fills_bind_group(device, &fills_buffer);

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
            segments: Vec::new(),
            handles: Vec::new(),
            overflowed: false,
        }
    }

    /// Draw later passes into targets of `format` (the atlas's format at
    /// first), e.g. a non-sRGB view holding encoded colors.
    pub(crate) fn set_target_format(
        &mut self,
        atlas: &GlyphAtlas,
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

    /// The atlas entries the prepared glyphs draw from; pinning them all
    /// ([`GlyphAtlas::pin`]) proves the vertices still draw those glyphs.
    pub(crate) fn handles(&self) -> &[GlyphHandle] {
        &self.handles
    }

    /// Whether any prepared glyph sits in an overflow page, which only
    /// this frame's uploads fill.
    pub(crate) fn uses_overflow(&self) -> bool {
        self.overflowed
    }

    /// The overflow ordinal of each draw segment, in draw order.
    pub(crate) fn segment_ordinals(&self) -> impl Iterator<Item = u32> + '_ {
        self.segments.iter().map(|s| s.ordinal)
    }

    /// Prepares the glyphs of `text_areas`' visible layout runs, in order.
    pub(crate) fn prepare<'a>(
        &mut self,
        atlas: &mut GlyphAtlas,
        font_system: &mut FontSystem,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = (TextArea<'a>, f32)>,
    ) -> Result<(), PrepareError> {
        self.clear();
        let resolution = viewport.resolution();

        for (text_area, raster_scale) in text_areas {
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

                    self.push_glyph(
                        atlas,
                        font_system,
                        GlyphMetadata {
                            x: physical_glyph.x,
                            y: physical_glyph.y,
                            line_y: run.line_y,
                            scale_factor: text_area.scale,
                            raster_scale,
                            color,
                            cache_key: physical_glyph.cache_key,
                            paint: 0,
                        },
                        bounds,
                    )?;
                }
            }
        }

        self.finish(atlas);
        Ok(())
    }

    /// Prepares glyphs that are already positioned, in the order given.
    /// Unlike [`Self::prepare`], nothing is read from a cosmic-text buffer:
    /// each glyph brings its own position, color, and clip. Call
    /// [`Self::upload`] before rendering; keeping the GPU copy separate lets
    /// a caller budget preparation apart from the driver's staging.
    pub(crate) fn prepare_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        font_system: &mut FontSystem,
        viewport: &Viewport,
        glyphs: impl IntoIterator<Item = PositionedGlyph>,
    ) -> Result<(), PrepareError> {
        self.clear();
        let resolution = viewport.resolution();

        for glyph in glyphs {
            self.push_glyph(
                atlas,
                font_system,
                GlyphMetadata {
                    x: glyph.x,
                    y: glyph.y,
                    line_y: 0.0,
                    scale_factor: 1.0,
                    raster_scale: f32::from_bits(glyph.scale),
                    color: glyph.color,
                    cache_key: glyph.cache_key,
                    paint: u32::from(glyph.fill)
                        | glyph
                            .backdrop
                            .map_or(0, |luminance| 0x100 | u32::from(luminance) << 16),
                },
                GlyphBounds::clipped(glyph.bounds, resolution),
            )?;
        }

        self.finish(atlas);
        Ok(())
    }

    fn clear(&mut self) {
        self.glyph_vertices.clear();
        self.segments.clear();
        self.handles.clear();
        self.overflowed = false;
    }

    /// Sorts the handles and gives every segment its bind group.
    fn finish(&mut self, atlas: &mut GlyphAtlas) {
        self.handles.sort_unstable();
        self.handles.dedup();
        for segment in &mut self.segments {
            if segment.bind.is_none() {
                segment.bind = Some(atlas.bind_group(segment.color, segment.mask));
            }
        }
    }

    fn push_glyph(
        &mut self,
        atlas: &mut GlyphAtlas,
        font_system: &mut FontSystem,
        metadata: GlyphMetadata,
        bounds: GlyphBounds,
    ) -> Result<(), PrepareError> {
        let Some(glyph) = atlas.glyph(font_system, metadata.cache_key, metadata.raster_scale)?
        else {
            return Ok(());
        };
        let placed = glyph.placed;

        let mut x = metadata.x + placed.left;
        let mut y =
            (metadata.line_y * metadata.scale_factor).round() as i32 + metadata.y - placed.top;

        let (mut atlas_x, mut atlas_y) = (placed.x, placed.y);
        let mut width = placed.width as i32;
        let mut height = placed.height as i32;

        // Starts beyond right edge or ends beyond left edge
        let max_x = x + width;
        if x > bounds.x.max || max_x < bounds.x.min {
            return Ok(());
        }

        // Starts beyond bottom edge or ends beyond top edge
        let max_y = y + height;
        if y > bounds.y.max || max_y < bounds.y.min {
            return Ok(());
        }

        // Clip left edge
        if x < bounds.x.min {
            let right_shift = bounds.x.min - x;

            x = bounds.x.min;
            width = max_x - bounds.x.min;
            atlas_x += right_shift as u32;
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
            atlas_y += bottom_shift as u32;
        }

        // Clip bottom edge
        if y + height > bounds.y.max {
            height = bounds.y.max - y;
        }

        let index = self.glyph_vertices.len() as u32;
        self.glyph_vertices.push(GlyphToRender {
            pos: [x, y],
            dim: [width as u16, height as u16],
            uv: [atlas_x as u16, atlas_y as u16],
            color: metadata.color.0,
            // Colors are sRGB-encoded; the shader decodes them unless the
            // target is encoded.
            content_type_with_srgb: [
                placed.kind as u16,
                CONVERT_TO_LINEAR | linear_correction(&metadata.cache_key),
            ],
            depth: 0.0,
            paint: metadata.paint,
        });
        match glyph.handle {
            Some(handle) => self.handles.push(handle),
            None => self.overflowed = true,
        }

        // Extend the last segment when its bind group can hold this page.
        let page = Some(placed.page);
        let is_color = placed.kind == super::ContentType::Color;
        if let Some(last) = self.segments.last_mut()
            && last.ordinal == glyph.ordinal
        {
            let slot = if is_color {
                &mut last.color
            } else {
                &mut last.mask
            };
            if slot.is_none() || *slot == page {
                *slot = page;
                last.instances.end = index + 1;
                return Ok(());
            }
        }
        let (color, mask) = if is_color { (page, None) } else { (None, page) };
        self.segments.push(DrawSegment {
            instances: index..index + 1,
            color,
            mask,
            ordinal: glyph.ordinal,
            bind: None,
        });
        Ok(())
    }

    /// Copies the prepared vertices to the GPU, growing the buffer if needed.
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

    /// Draws the segments `segments` of the glyphs last prepared, moving
    /// every glyph by the viewport's draw offset `slot` (see
    /// [`Viewport::set_draw_offsets`]), clip included. Drawing glyphs
    /// prepared earlier where the same glyphs moved by whole pixels, with
    /// their bounds moved alike and inside the viewport both times, would
    /// be prepared, needs no preparing or uploading again. `slot` must be
    /// below [`super::MAX_DRAW_OFFSETS`].
    pub(crate) fn render_at(
        &self,
        viewport: &Viewport,
        pass: &mut RenderPass<'_>,
        slot: u32,
        segments: Range<usize>,
    ) -> Result<(), RenderError> {
        let segments = &self.segments
            [segments.start.min(self.segments.len())..segments.end.min(self.segments.len())];
        if self.glyph_vertices.is_empty() || segments.is_empty() {
            return Ok(());
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        pass.set_bind_group(2, &self.fills_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for segment in segments {
            let Some(bind) = &segment.bind else {
                continue;
            };
            pass.set_bind_group(0, bind, &[]);
            // The vertex shader reads the slot from the vertex index.
            pass.draw(slot * 4..slot * 4 + 4, segment.instances.clone());
        }

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

struct GlyphMetadata {
    x: i32,
    y: i32,
    line_y: f32,
    scale_factor: f32,
    /// The device scale the glyph was shaped at, for its raster key.
    raster_scale: f32,
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
