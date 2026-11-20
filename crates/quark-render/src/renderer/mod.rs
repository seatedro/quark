//! Drawing scenes: a [`GpuContext`] shared by every window, and one
//! [`Renderer`] per window or headless target.
//!
//! A frame passes through these modules in order:
//!
//! 1. `layers` plans layers and isolated groups: skipped, drawn in place,
//!    or rendered into an offscreen target of their own.
//! 2. `flatten` walks each target's primitives into draw order: clipped,
//!    faded, and batched into z-layer segments.
//! 3. `batches` turns each flattened target into instance arrays and
//!    scissor-batched draw commands.
//! 4. `frame` uploads them and prepares text (`text_runs`); `encode`
//!    records each target's passes, with `blur` regions blurring what was
//!    drawn before them.
//! 5. `surface` acquires and presents the window's texture; `headless`
//!    reads frames back instead.
//!
//! `gpu_context`, `pipelines`, `instances`, `pools`, and `images` hold the
//! GPU objects every step draws with.

mod batches;
mod blur;
mod encode;
mod flatten;
mod frame;
mod gpu_context;
#[cfg(any(test, feature = "headless-render"))]
mod headless;
mod images;
mod instances;
mod layers;
mod pipelines;
mod pools;
mod surface;
mod text_runs;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use quark_text::{FontEpoch, TextSystem};
use thiserror::Error;

use self::flatten::Flattener;
use self::frame::{SegmentTexture, TargetFrame, Viewports};
use self::images::SharedImages;
use self::layers::{LayerPlan, LayerScratch};
use self::pipelines::texture_bind_group;
use self::pools::{TexturePool, TransientBufferPool};
use self::surface::Surface;
use crate::scene::{Primitive, SurfaceBackground, TextRendering, UiCompositing};
use crate::text::{GlyphOwner, RecoloredBuffers, TextPath, measure_mono_char_width};
use crate::text_engine::GlyphAtlas;

pub(crate) use self::flatten::{ClippedRichText, ClippedText, fade_color};
pub use self::gpu_context::GpuContext;
pub use self::pools::OffscreenTarget;
/// Memory limits of a renderer's glyph atlas, for
/// [`Renderer::set_text_atlas_limits`].
pub use crate::text_engine::AtlasLimits as TextAtlasLimits;
/// What a renderer's glyph atlas holds, from [`Renderer::text_atlas_memory`].
pub use crate::text_engine::AtlasMemory as TextAtlasMemory;
/// Glyph atlas work counters, from [`Renderer::text_atlas_stats`].
pub use crate::text_engine::AtlasStats as TextAtlasStats;
/// Why text could not be prepared or drawn, in [`RenderError`].
pub use crate::text_engine::{PrepareError as TextPrepareError, RenderError as TextRenderError};
pub use crate::text_engine::{TextRasterizer, TextSmoothing};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub ui_font_size_px: f32,
    pub ui_line_height_px: f32,
    pub mono_font_size_px: f32,
    pub mono_line_height_px: f32,
    pub mono_char_width_px: f32,
}

impl Default for TextMetrics {
    fn default() -> Self {
        Self {
            ui_font_size_px: 14.0,
            ui_line_height_px: 18.0,
            mono_font_size_px: 13.0,
            mono_line_height_px: 24.0,
            mono_char_width_px: 8.0,
        }
    }
}

/// How one renderer's surface draws: chosen per window or headless
/// renderer, never process-wide, so changing one leaves other windows and
/// offscreen renders as they were.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RendererOptions {
    pub compositing: UiCompositing,
    /// Coverage policy for the whole surface: `Perceptual` (the default)
    /// honors each `StyledText` primitive's own rendering and backdrop;
    /// `Linear` keeps every glyph's plain coverage, the appearance before
    /// perceptual text, for apps that rely on it. `TextRun` and
    /// `RichTextRun` primitives name no backdrop, so they draw with plain
    /// coverage (or their layout's terminal correction) either way.
    pub text_rendering: TextRendering,
}

impl RendererOptions {
    /// Whether the window's target holds encoded sRGB values.
    fn encoded(self) -> bool {
        self.compositing == UiCompositing::WebCompatible
    }
}

/// What a renderer's surface does with its options and background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceCapabilities {
    /// Transparent pixels reach the window system's compositor.
    pub transparent: bool,
    /// The surface composites in [`UiCompositing::WebCompatible`].
    pub web_compatible: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub primitive_count: usize,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub cpu_us: u64,
    pub acquire_us: u64,
    pub present_us: u64,
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("no compatible GPU adapter found")]
    NoAdapter,
    #[error("failed to create surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("device request failed: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("failed to prepare text: {0}")]
    PrepareText(#[from] TextPrepareError),
    #[error("failed to render text: {0}")]
    RenderText(#[from] TextRenderError),
    /// [`Renderer::set_text_rasterizer`] asked for a rasterizer this build
    /// or platform does not have.
    #[error("the text rasterizer {0:?} is not available here")]
    TextRasterizerUnavailable(TextRasterizer),
    #[error("surface acquisition failed")]
    SurfaceAcquire,
    /// The surface was lost or outdated and has been reconfigured; nothing
    /// was drawn, so draw the frame again.
    #[error("the surface was lost or outdated and has been reconfigured")]
    SurfaceReconfigured,
    /// The next surface texture did not arrive in time; skip this frame.
    #[error("timed out acquiring the next surface texture")]
    SurfaceTimeout,
    /// The GPU ran out of memory for the next frame. Not recoverable.
    #[error("the GPU is out of memory")]
    OutOfMemory,
    #[error("the renderer has no window surface; use the headless render path")]
    NoSurface,
    #[error("no surface format matches the GPU's pipelines")]
    IncompatibleSurface,
    #[error("buffer map failed")]
    BufferMap,
    #[error("png encode/write failed: {0}")]
    PngWrite(String),
}

/// Draws scenes into one window's surface (or offscreen targets) using a
/// [`GpuContext`] that may be shared with other windows.
pub struct Renderer {
    gpu: GpuContext,
    // Handles cloned out of `gpu` (wgpu handles are reference counted) so
    // the drawing code reads them as plain fields.
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Where frames go: a window's surface, or nowhere for a headless
    /// renderer.
    surface: Surface,
    scale_factor: f64,
    /// Encoded frames draw through a non-sRGB view of the window's own
    /// texture; otherwise they draw into a pooled encoded texture that
    /// one more pass converts into it.
    direct_encoded: bool,
    /// Viewport uniform of that conversion pass.
    present_uniform: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    texture_pool: TexturePool,
    instance_buffer_pool: TransientBufferPool,
    images: Arc<Mutex<SharedImages>>,
    viewports: Viewports,
    atlas: GlyphAtlas,
    /// The rasterizer [`Self::set_text_rasterizer`] last asked for.
    text_rasterizer: TextRasterizer,
    /// One text renderer per text run of the frame, kept with what it
    /// prepared.
    text_runs: text_runs::TextRuns,
    /// Positioned glyphs unless a test compares against the buffer path.
    text_path: TextPath,
    /// Buffer path only.
    recolored: RecoloredBuffers,
    glyph_owner: GlyphOwner,
    /// `(font size, TextSystem generation, width)` of the last measurement.
    cached_mono_char_width: Option<(f32, FontEpoch, f32)>,
    flattener: Flattener,
    /// The frame's primitives with chunks expanded, when layers need them.
    expanded: Vec<Primitive>,
    /// The window's target, then each offscreen layer's by target number.
    frames: Vec<TargetFrame>,
    /// Targets in use this frame.
    active_frames: usize,
    plans: Vec<LayerPlan>,
    layer_scratch: LayerScratch,
    segment_texture: Option<SegmentTexture>,
    /// Effect animation time of the frame being drawn, in seconds.
    time: f32,
    options: RendererOptions,
    background: SurfaceBackground,
}

impl Renderer {
    /// Per-window state on top of the shared context.
    fn assemble(
        gpu: GpuContext,
        surface: Surface,
        scale_factor: f64,
        direct_encoded: bool,
    ) -> Self {
        let shared = &*gpu.inner;
        let device = shared.device.clone();
        let queue = shared.queue.clone();
        let mut atlas = GlyphAtlas::new(&device, &queue, &shared.glyph_cache, shared.format);
        // Windows draw with the platform's rasterizer; headless renderers
        // (tests, fixtures) with deterministic swash unless asked.
        let text_rasterizer = match surface {
            Surface::Window(_) => TextRasterizer::Auto,
            Surface::Headless { .. } => TextRasterizer::Swash,
        };
        if let Err(error) = atlas.set_rasterizer(text_rasterizer) {
            tracing::warn!("text rasterizer unavailable, drawing with swash: {error}");
        }

        Self {
            texture_pool: TexturePool::new(shared),
            instance_buffer_pool: TransientBufferPool::new(&device, &queue),
            images: Arc::clone(&shared.images),
            viewports: Viewports::new(shared),
            device,
            queue,
            surface,
            scale_factor,
            direct_encoded,
            present_uniform: None,
            atlas,
            text_rasterizer,
            text_runs: text_runs::TextRuns::default(),
            text_path: TextPath::default(),
            recolored: RecoloredBuffers::default(),
            glyph_owner: GlyphOwner::default(),
            cached_mono_char_width: None,
            flattener: Flattener::default(),
            expanded: Vec::new(),
            frames: Vec::new(),
            active_frames: 0,
            plans: Vec::new(),
            layer_scratch: LayerScratch::default(),
            segment_texture: None,
            time: 0.0,
            options: RendererOptions::default(),
            background: SurfaceBackground::default(),
            gpu,
        }
    }

    /// The context this renderer draws with; pass it to [`Self::with_gpu`]
    /// to share it with another window.
    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    /// Glyph atlas work since this renderer was created: cache misses,
    /// evictions, growths, and bytes uploaded.
    pub fn text_atlas_stats(&self) -> TextAtlasStats {
        self.atlas.stats()
    }

    /// What the glyph atlas holds now: pages and bytes by kind, the
    /// limits, and the peak.
    pub fn text_atlas_memory(&self) -> TextAtlasMemory {
        self.atlas.memory()
    }

    /// Rasterizes glyphs with `rasterizer` from the next frame, forgetting
    /// every glyph rasterized before; layouts stay valid. Swash still
    /// draws what a native rasterizer cannot. Fails, changing nothing, for
    /// a rasterizer this build or platform lacks.
    pub fn set_text_rasterizer(&mut self, rasterizer: TextRasterizer) -> Result<(), RenderError> {
        self.atlas
            .set_rasterizer(rasterizer)
            .map_err(|_| RenderError::TextRasterizerUnavailable(rasterizer))?;
        self.text_rasterizer = rasterizer;
        self.text_runs.forget();
        Ok(())
    }

    /// The rasterizer asked for, and the one drawing: [`TextRasterizer::Auto`]
    /// resolved, or [`TextRasterizer::Swash`] when Auto's could not start.
    pub fn text_rasterizer(&self) -> (TextRasterizer, TextRasterizer) {
        use crate::text_engine::raster::RasterBackend;
        let effective = match self.atlas.native_profile().map(|p| p.backend) {
            None | Some(RasterBackend::Swash) => TextRasterizer::Swash,
            Some(RasterBackend::DirectWrite) => TextRasterizer::DirectWrite,
            Some(RasterBackend::CoreText) => match self.text_rasterizer {
                TextRasterizer::Auto => crate::text_engine::auto_rasterizer(),
                other => other,
            },
        };
        (self.text_rasterizer, effective)
    }

    /// Limits this renderer's glyph atlas to `limits` from the next frame.
    /// A new page size forgets every cached glyph.
    pub fn set_text_atlas_limits(&mut self, limits: TextAtlasLimits) {
        self.atlas.set_limits(limits);
        self.text_runs.forget();
    }

    pub fn options(&self) -> RendererOptions {
        self.options
    }

    /// Draw later frames as `options` say. Affects this renderer only.
    pub fn set_options(&mut self, options: RendererOptions) {
        self.options = options;
        self.configure_view_formats();
    }

    /// What the surface does with the options it was given.
    pub fn capabilities(&self) -> SurfaceCapabilities {
        SurfaceCapabilities {
            transparent: self.background == SurfaceBackground::Transparent,
            web_compatible: self.options.encoded(),
        }
    }

    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    pub fn text_metrics(&mut self, text: &mut TextSystem) -> TextMetrics {
        let scale = self.scale_factor as f32;
        let mono_font_size = 13.0 * scale;
        let fonts = text.font_epoch();
        let char_w = match self.cached_mono_char_width {
            Some((cached_size, cached_fonts, cached_w))
                if (cached_size - mono_font_size).abs() < 0.001 && cached_fonts == fonts =>
            {
                cached_w
            }
            _ => {
                let w = measure_mono_char_width(text, mono_font_size);
                self.cached_mono_char_width = Some((mono_font_size, fonts, w));
                w
            }
        };
        TextMetrics {
            ui_font_size_px: 14.0 * scale,
            ui_line_height_px: 18.0 * scale,
            mono_font_size_px: mono_font_size,
            mono_line_height_px: 24.0 * scale,
            mono_char_width_px: char_w,
        }
    }

    /// Acquire an offscreen texture from the pool. The returned target can be
    /// used as a render attachment and later sampled via `create_texture_bind_group`.
    pub fn acquire_offscreen(&mut self, width: u32, height: u32) -> OffscreenTarget {
        self.texture_pool.acquire(width, height)
    }

    /// Get the texture view for an offscreen target (for use as a render attachment).
    pub fn offscreen_view(&self, target: &OffscreenTarget) -> &wgpu::TextureView {
        self.texture_pool.view(target)
    }

    /// Create a bind group for sampling an offscreen target in a shader.
    pub fn create_texture_bind_group(&self, target: &OffscreenTarget) -> wgpu::BindGroup {
        let gpu = &*self.gpu.inner;
        texture_bind_group(
            &self.device,
            &gpu.layouts.texture,
            self.texture_pool.view(target),
            &gpu.sampler,
        )
    }

    /// Return an offscreen target to the pool for reuse. Dropping the target
    /// also returns it.
    pub fn release_offscreen(&mut self, target: OffscreenTarget) {
        self.texture_pool.release(target);
    }
}
