//! GPU memory reused from frame to frame: offscreen render targets and
//! instance buffers.

use std::sync::Arc;

use bytemuck::Pod;

use super::gpu_context::GpuShared;
use super::pipelines::texture_bind_group;

/// Frames a released offscreen texture stays pooled before it is dropped.
const KEEP_UNUSED_OFFSCREEN_FRAMES: u64 = 2;

/// Handle to an offscreen render target allocated from the pool. The
/// texture returns to the pool when the handle is released or dropped.
pub struct OffscreenTarget {
    lease: Arc<()>,
    pub width: u32,
    pub height: u32,
}

/// One pooled texture.
///
/// The [`OffscreenTarget`] handed out for it shares `lease`, so the
/// texture is leased exactly while that handle is alive: dropping a target
/// without releasing it frees the texture instead of pinning the pool
/// forever. The lease's count is the only record of ownership.
struct PooledTexture {
    view: wgpu::TextureView,
    /// Bind group sampling this texture, created on first use. The view is
    /// immutable for the entry's lifetime and the layout and sampler never
    /// change, so it is reused until the entry is trimmed.
    bind_group: Option<wgpu::BindGroup>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    lease: Arc<()>,
    /// The frame it was last acquired or released in.
    last_used_frame: u64,
}

impl PooledTexture {
    fn leased(&self) -> bool {
        Arc::strong_count(&self.lease) > 1
    }

    fn is(&self, target: &OffscreenTarget) -> bool {
        Arc::ptr_eq(&self.lease, &target.lease)
    }
}

/// Reusable offscreen render targets: layers, blur scratch, and the
/// encoded staging texture.
pub(super) struct TexturePool {
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Vec<PooledTexture>,
    /// The format of linear-light targets; encoded targets use its
    /// non-sRGB twin.
    format: wgpu::TextureFormat,
    frame: u64,
}

impl TexturePool {
    pub(super) fn new(gpu: &GpuShared) -> Self {
        Self {
            device: gpu.device.clone(),
            layout: gpu.layouts.texture.clone(),
            sampler: gpu.sampler.clone(),
            textures: Vec::new(),
            format: gpu.format,
            frame: 0,
        }
    }

    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
    }

    /// Acquire a linear-light texture of at least the given dimensions;
    /// see [`Self::acquire_in`].
    pub(super) fn acquire(&mut self, width: u32, height: u32) -> OffscreenTarget {
        self.acquire_in(width, height, false)
    }

    /// Acquire a texture of at least the given dimensions, holding encoded
    /// sRGB values when `encoded`. It stays out of the pool until the
    /// target is released or dropped.
    pub(super) fn acquire_in(&mut self, width: u32, height: u32, encoded: bool) -> OffscreenTarget {
        let w = width.max(1);
        let h = height.max(1);
        let format = if encoded {
            self.format.remove_srgb_suffix()
        } else {
            self.format
        };

        let reusable = self.textures.iter_mut().find(|entry| {
            !entry.leased() && entry.format == format && entry.width >= w && entry.height >= h
        });
        if let Some(entry) = reusable {
            entry.last_used_frame = self.frame;
            return OffscreenTarget {
                lease: Arc::clone(&entry.lease),
                width: entry.width,
                height: entry.height,
            };
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("quark_offscreen"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let lease = Arc::new(());
        self.textures.push(PooledTexture {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            bind_group: None,
            width: w,
            height: h,
            format,
            lease: Arc::clone(&lease),
            last_used_frame: self.frame,
        });
        OffscreenTarget {
            lease,
            width: w,
            height: h,
        }
    }

    /// The entry `target` leases. A target from another renderer's pool is a
    /// caller bug.
    fn entry(&self, target: &OffscreenTarget) -> &PooledTexture {
        self.textures
            .iter()
            .find(|entry| entry.is(target))
            .expect("offscreen target belongs to this renderer's pool")
    }

    fn entry_mut(&mut self, target: &OffscreenTarget) -> &mut PooledTexture {
        self.textures
            .iter_mut()
            .find(|entry| entry.is(target))
            .expect("offscreen target belongs to this renderer's pool")
    }

    pub(super) fn view(&self, target: &OffscreenTarget) -> &wgpu::TextureView {
        &self.entry(target).view
    }

    /// The cached bind group sampling `target`, created on first use. An
    /// owned handle (`wgpu::BindGroup` is reference counted), so several
    /// targets can be bound in one pass.
    pub(super) fn bind_group(&mut self, target: &OffscreenTarget) -> wgpu::BindGroup {
        let (device, layout, sampler) = (&self.device, &self.layout, &self.sampler);
        let entry = self
            .textures
            .iter_mut()
            .find(|entry| entry.is(target))
            .expect("offscreen target belongs to this renderer's pool");
        let view = &entry.view;
        entry
            .bind_group
            .get_or_insert_with(|| texture_bind_group(device, layout, view, sampler))
            .clone()
    }

    /// Return `target` to the pool, counting this frame as its last use.
    pub(super) fn release(&mut self, target: OffscreenTarget) {
        let frame = self.frame;
        self.entry_mut(&target).last_used_frame = frame;
    }

    /// Drop textures no target has leased for a few frames.
    pub(super) fn trim_unused(&mut self) {
        let frame = self.frame;
        self.textures.retain(|entry| {
            entry.leased()
                || frame.saturating_sub(entry.last_used_frame) <= KEEP_UNUSED_OFFSCREEN_FRAMES
        });
    }

    /// Bytes of every pooled texture, as four bytes a pixel.
    #[cfg(test)]
    pub(super) fn pooled_bytes(&self) -> u64 {
        self.textures
            .iter()
            .map(|t| u64::from(t.width) * u64::from(t.height) * 4)
            .sum()
    }
}

/// Vertex buffers of instances, reused frame after frame: the frame's
/// uploads take them in order, growing one only when its data no longer
/// fits.
pub(super) struct TransientBufferPool {
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: Vec<ReusableBuffer>,
    next_buffer: usize,
}

struct ReusableBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: usize,
}

impl TransientBufferPool {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            buffers: Vec::new(),
            next_buffer: 0,
        }
    }

    pub(super) fn begin_frame(&mut self) {
        self.next_buffer = 0;
    }

    /// The next buffer of the frame holding `data`; `None` for no data.
    pub(super) fn upload<T: Pod>(
        &mut self,
        label: &'static str,
        data: &[T],
    ) -> Option<wgpu::Buffer> {
        if data.is_empty() {
            return None;
        }

        let bytes = bytemuck::cast_slice(data);
        let required_bytes = bytes.len().max(size_of::<T>());
        let capacity_bytes = required_bytes.next_power_of_two().max(256);
        let create = |label| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity_bytes as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let buffer = match self.buffers.get_mut(self.next_buffer) {
            Some(entry) => {
                if entry.capacity_bytes < required_bytes {
                    entry.buffer = create(label);
                    entry.capacity_bytes = capacity_bytes;
                }
                entry.buffer.clone()
            }
            None => {
                let buffer = create(label);
                self.buffers.push(ReusableBuffer {
                    buffer: buffer.clone(),
                    capacity_bytes,
                });
                buffer
            }
        };
        self.next_buffer += 1;
        self.queue.write_buffer(&buffer, 0, bytes);
        Some(buffer)
    }
}
