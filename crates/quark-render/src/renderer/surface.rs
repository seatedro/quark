//! The window surface: configuring it, following the window's size and
//! the renderer's options, and drawing each frame into its texture.

use std::sync::Arc;
use std::time::Instant;

use winit::dpi::PhysicalSize;
use winit::window::Window;

use super::encode::begin_pass;
use super::gpu_context::{GpuContext, GpuShared};
use super::instances::{BlitInstance, SourceSpace, ViewportUniform};
use super::pipelines::viewport_uniform;
use super::pools::OffscreenTarget;
use super::{FrameStats, RenderError, Renderer, RendererOptions};
use crate::scene::{Scene, SurfaceBackground};
use quark_text::TextSystem;

/// Where a renderer's frames go.
pub(super) enum Surface {
    /// No window: frames render offscreen and are read back. A zero size
    /// skips them, as a minimized window's are.
    #[cfg_attr(
        not(any(test, feature = "headless-render")),
        expect(dead_code, reason = "only the headless constructors make one")
    )]
    Headless { size: PhysicalSize<u32> },
    /// A window: frames draw into its surface's textures.
    Window(WindowSurface),
}

impl Surface {
    /// The size frames draw at; `None` while the window is minimized.
    pub(super) fn drawable_size(&self) -> Option<(u32, u32)> {
        match self {
            Surface::Headless { size } => {
                (size.width > 0 && size.height > 0).then_some((size.width, size.height))
            }
            Surface::Window(window) => match window.state {
                SurfaceState::Configured => Some((window.config.width, window.config.height)),
                SurfaceState::Minimized => None,
            },
        }
    }

    /// The window's next texture to draw a frame into.
    fn acquire(&self, device: &wgpu::Device) -> Result<wgpu::SurfaceTexture, RenderError> {
        let Surface::Window(window) = self else {
            return Err(RenderError::NoSurface);
        };
        match window.surface.get_current_texture() {
            Ok(frame) => Ok(frame),
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                window.surface.configure(device, &window.config);
                Err(RenderError::SurfaceReconfigured)
            }
            Err(wgpu::SurfaceError::Timeout) => Err(RenderError::SurfaceTimeout),
            Err(wgpu::SurfaceError::OutOfMemory) => Err(RenderError::OutOfMemory),
            Err(wgpu::SurfaceError::Other) => Err(RenderError::SurfaceAcquire),
        }
    }
}

/// A window's surface and the configuration it follows.
pub(super) struct WindowSurface {
    surface: wgpu::Surface<'static>,
    /// The configuration the surface has, or gets once the window is
    /// restored.
    config: wgpu::SurfaceConfiguration,
    state: SurfaceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceState {
    /// Configured with `config`: frames draw at its size.
    Configured,
    /// The window is minimized (a zero size): frames are skipped and the
    /// surface keeps its last configuration. Acquiring would fail as
    /// outdated on some platforms, and reconfiguring and redrawing would
    /// spin the event loop for as long as the window stays minimized, so
    /// changes to `config` wait for a real size.
    Minimized,
}

impl WindowSurface {
    /// `surface` configured for a window of `size` in the shared format.
    fn new(surface: wgpu::Surface<'static>, gpu: &GpuShared, size: PhysicalSize<u32>) -> Self {
        let (width, height) = (size.width.max(1), size.height.max(1));
        let config = surface
            .get_default_config(&gpu.adapter, width, height)
            .map(|config| wgpu::SurfaceConfiguration {
                format: gpu.format,
                ..config
            })
            .unwrap_or(wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width,
                height,
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
            });
        surface.configure(&gpu.device, &config);
        let state = if size.width == 0 || size.height == 0 {
            SurfaceState::Minimized
        } else {
            SurfaceState::Configured
        };
        Self {
            surface,
            config,
            state,
        }
    }

    /// Follow a window size: a zero dimension minimizes, any other
    /// configures the surface at it.
    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.state = SurfaceState::Minimized;
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.state = SurfaceState::Configured;
        self.surface.configure(device, &self.config);
    }

    fn set_alpha_mode(&mut self, device: &wgpu::Device, mode: wgpu::CompositeAlphaMode) {
        if self.config.alpha_mode != mode {
            self.config.alpha_mode = mode;
            self.apply(device);
        }
    }

    fn set_view_formats(&mut self, device: &wgpu::Device, formats: Vec<wgpu::TextureFormat>) {
        if self.config.view_formats != formats {
            self.config.view_formats = formats;
            self.apply(device);
        }
    }

    /// Configure the surface with a changed `config`, unless it waits for
    /// the window to be restored.
    fn apply(&self, device: &wgpu::Device) {
        if self.state == SurfaceState::Configured {
            self.surface.configure(device, &self.config);
        }
    }
}

impl Renderer {
    /// A renderer for `window` with a new [`GpuContext`] chosen for it.
    pub fn new(window: Arc<Window>) -> Result<Self, RenderError> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
            let surface = instance.create_surface(window.clone())?;
            let gpu = GpuContext::for_surface(instance, &surface).await?;
            Ok(Self::for_surface(gpu, surface, &window))
        })
    }

    /// A renderer for another window on an existing context.
    ///
    /// The context's adapter was picked for the first window's surface. If it
    /// cannot present to this window's surface (another GPU's display, or no
    /// surface format matching the shared pipelines), the window gets a new
    /// context chosen for its own surface instead; that window then shares
    /// nothing with the others. Compare with [`GpuContext::same_device`].
    pub fn with_gpu(gpu: &GpuContext, window: Arc<Window>) -> Result<Self, RenderError> {
        let surface = gpu.inner.instance.create_surface(window.clone())?;
        if gpu.supports(&surface) {
            return Ok(Self::for_surface(gpu.clone(), surface, &window));
        }
        tracing::warn!("the shared GPU adapter cannot present to this window; using a new device");
        pollster::block_on(async {
            let fresh = GpuContext::for_surface(gpu.inner.instance.clone(), &surface).await?;
            Ok(Self::for_surface(fresh, surface, &window))
        })
    }

    /// A renderer for `window` drawing as `options` say, on a new context.
    pub fn new_with_options(
        window: Arc<Window>,
        options: RendererOptions,
    ) -> Result<Self, RenderError> {
        let mut renderer = Self::new(window)?;
        renderer.set_options(options);
        Ok(renderer)
    }

    /// [`Self::with_gpu`] drawing as `options` say.
    pub fn with_gpu_and_options(
        gpu: &GpuContext,
        window: Arc<Window>,
        options: RendererOptions,
    ) -> Result<Self, RenderError> {
        let mut renderer = Self::with_gpu(gpu, window)?;
        renderer.set_options(options);
        Ok(renderer)
    }

    fn for_surface(gpu: GpuContext, surface: wgpu::Surface<'static>, window: &Window) -> Self {
        let window_surface = WindowSurface::new(surface, &gpu.inner, window.inner_size());
        let direct_encoded = gpu
            .inner
            .adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS);
        Self::assemble(
            gpu,
            Surface::Window(window_surface),
            window.scale_factor(),
            direct_encoded,
        )
    }

    /// Adopt a new window size. A zero dimension (a minimized window) leaves
    /// the surface configured at its last size and makes [`Self::render`] a
    /// no-op until a real size arrives.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) {
        self.scale_factor = scale_factor;
        match &mut self.surface {
            Surface::Headless { size } => *size = PhysicalSize::new(width, height),
            Surface::Window(window) => window.resize(&self.device, width, height),
        }
    }

    /// Show `background` where the scene paints nothing, returning the
    /// background in effect. A transparent background picks a surface alpha
    /// mode that composites premultiplied output with the window system's
    /// (see `transparent_alpha_mode`) and keeps it across resizes; with no
    /// such mode it falls back to an opaque black background and says so.
    /// Headless renderers keep alpha in their readback either way.
    pub fn set_surface_background(&mut self, background: SurfaceBackground) -> SurfaceBackground {
        let effective = match (&mut self.surface, background) {
            (Surface::Headless { .. }, _) => background,
            (Surface::Window(window), SurfaceBackground::Transparent) => {
                let modes = window
                    .surface
                    .get_capabilities(&self.gpu.inner.adapter)
                    .alpha_modes;
                match transparent_alpha_mode(&modes) {
                    Some(mode) => {
                        window.set_alpha_mode(&self.device, mode);
                        SurfaceBackground::Transparent
                    }
                    None => {
                        window.set_alpha_mode(&self.device, wgpu::CompositeAlphaMode::Opaque);
                        SurfaceBackground::default()
                    }
                }
            }
            (Surface::Window(window), SurfaceBackground::Opaque(_)) => {
                window.set_alpha_mode(&self.device, wgpu::CompositeAlphaMode::Opaque);
                background
            }
        };
        self.background = effective;
        effective
    }

    /// Let the surface's textures take a view of the encoded format when
    /// frames draw encoded through one.
    pub(super) fn configure_view_formats(&mut self) {
        let Surface::Window(window) = &mut self.surface else {
            return;
        };
        let encoded = self.gpu.target_format(true);
        let formats =
            if self.options.encoded() && self.direct_encoded && encoded != window.config.format {
                vec![encoded]
            } else {
                Vec::new()
            };
        window.set_view_formats(&self.device, formats);
    }

    /// `text` must be the system that shaped the scene's text layouts.
    pub fn render(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        time_seconds: f32,
    ) -> Result<FrameStats, RenderError> {
        let Some((width, height)) = self.surface.drawable_size() else {
            return Ok(FrameStats::default());
        };
        let render_started_at = Instant::now();
        self.begin_frame(time_seconds);
        self.flattener.adopt_svg_fonts(text);
        self.flatten(scene, width, height);

        let acquire_started_at = Instant::now();
        let frame = self.surface.acquire(&self.device)?;
        let acquire_us = acquire_started_at.elapsed().as_micros() as u64;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("quark_frame_encoder"),
            });
        self.record_root(&mut encoder, &frame.texture, (width, height), text)?;

        let present_started_at = Instant::now();
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        let present_us = present_started_at.elapsed().as_micros() as u64;
        self.end_frame();

        let cpu_us = (render_started_at.elapsed().as_micros() as u64)
            .saturating_sub(acquire_us)
            .saturating_sub(present_us);
        Ok(FrameStats {
            primitive_count: scene.len(),
            viewport_width: width,
            viewport_height: height,
            cpu_us,
            acquire_us,
            present_us,
        })
    }

    /// Record the frame flattened last into `texture`, the window's
    /// target of `width` x `height`: directly when it composites in linear
    /// light, through an encoded view when it is web-compatible and the
    /// backend allows one, and otherwise into an encoded texture converted
    /// into `texture` by one more pass.
    pub(super) fn record_root(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        (width, height): (u32, u32),
        text: &mut TextSystem,
    ) -> Result<(), RenderError> {
        let encoded_format = self.gpu.target_format(true);
        if !self.options.encoded() {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            return self.record_frame(encoder, &view, text);
        }
        if self.direct_encoded || texture.format() == encoded_format {
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(encoded_format),
                ..Default::default()
            });
            return self.record_frame(encoder, &view, text);
        }
        let staging = self.texture_pool.acquire_in(width, height, true);
        let staging_view = self.texture_pool.view(&staging).clone();
        let result = self.record_frame(encoder, &staging_view, text);
        if result.is_ok() {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.present_encoded(encoder, &staging, &view, (width, height));
        }
        self.texture_pool.release(staging);
        result
    }

    /// Convert the encoded frame in `source` into the linear-light `dest`.
    fn present_encoded(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &OffscreenTarget,
        dest: &wgpu::TextureView,
        (width, height): (u32, u32),
    ) {
        let gpu = &*self.gpu.inner;
        let (buffer, bind_group) = self.present_uniform.get_or_insert_with(|| {
            viewport_uniform(&gpu.device, &gpu.layouts.viewport, "quark_present_uniform")
        });
        let uniform = ViewportUniform::new(width, height, 0.0, false);
        self.queue
            .write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        let bind_group = bind_group.clone();
        let instance = [BlitInstance {
            bounds: [0.0, 0.0, width as f32, height as f32],
            uv_rect: [
                0.0,
                0.0,
                width as f32 / source.width as f32,
                height as f32 / source.height as f32,
            ],
            tint: [1.0; 4],
            radii: [0.0; 4],
            space: SourceSpace::Encoded.attribute(),
        }];
        let Some(instances) = self
            .instance_buffer_pool
            .upload("quark_present_blit", &instance)
        else {
            return;
        };
        let texture = self.texture_pool.bind_group(source);
        let mut pass = begin_pass(
            encoder,
            "quark_present_pass",
            dest,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            (width, height),
        );
        pass.set_pipeline(&gpu.linear.blit);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_bind_group(1, &texture, &[]);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.draw(0..4, 0..1);
    }
}

/// The alpha mode a transparent window surface uses, from those the
/// surface offers: premultiplied, which matches the renderer's output;
/// else post-multiplied (Metal's non-opaque layer), exact for fully
/// transparent and opaque pixels and slightly dark on antialiased edges
/// over the desktop; else inherit (the window system's choice). `None`
/// when the surface can only be opaque (an X11 visual without alpha).
pub(super) fn transparent_alpha_mode(
    offered: &[wgpu::CompositeAlphaMode],
) -> Option<wgpu::CompositeAlphaMode> {
    use wgpu::CompositeAlphaMode::{Inherit, PostMultiplied, PreMultiplied};
    [PreMultiplied, PostMultiplied, Inherit]
        .into_iter()
        .find(|mode| offered.contains(mode))
}
