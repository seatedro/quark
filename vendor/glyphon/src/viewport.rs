use crate::{Cache, Params, Resolution};
use std::{mem, slice};
use wgpu::{BindGroup, Buffer, BufferDescriptor, BufferUsages, Device, Queue};

/// How many draws [`Viewport::set_draw_offsets`] can move; see
/// [`crate::TextRenderer::render_at`].
pub const MAX_DRAW_OFFSETS: usize = 2048;

/// Bytes of the draw offsets uniform: two offsets per 16-byte vector.
pub(crate) const DRAW_OFFSETS_SIZE: u64 = (MAX_DRAW_OFFSETS * 8) as u64;

/// Controls the visible area of all text for a given renderer. Any text outside of the visible
/// area will be clipped.
///
/// Many projects will only ever need a single `Viewport`, but it is possible to create multiple
/// `Viewport`s if you want to render text to specific areas within a window (without having to)
/// bound each `TextArea`).
#[derive(Debug)]
pub struct Viewport {
    params: Params,
    params_buffer: Buffer,
    /// The draw offsets last written, and their buffer (zeroed at
    /// creation).
    offsets: Vec<[i32; 2]>,
    offsets_buffer: Buffer,
    pub(crate) bind_group: BindGroup,
}

impl Viewport {
    /// Creates a new `Viewport` with the given `device` and `cache`.
    pub fn new(device: &Device, cache: &Cache) -> Self {
        let params = Params {
            screen_resolution: Resolution {
                width: 0,
                height: 0,
            },
            _pad: [0, 0],
        };

        let params_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyphon params"),
            size: mem::size_of::<Params>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let offsets_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyphon draw offsets"),
            size: DRAW_OFFSETS_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = cache.create_uniforms_bind_group(device, &params_buffer, &offsets_buffer);

        Self {
            params,
            params_buffer,
            offsets: Vec::new(),
            offsets_buffer,
            bind_group,
        }
    }

    /// Sets the offset, in pixels, that each draw slot moves its glyphs by:
    /// `offsets[s]` for [`crate::TextRenderer::render_at`] with slot `s`.
    /// Slots past the end of `offsets` keep the offsets last set (zero at
    /// first); at most [`MAX_DRAW_OFFSETS`] are used. Writes the buffer
    /// only when an offset changed.
    pub fn set_draw_offsets(&mut self, queue: &Queue, offsets: &[[i32; 2]]) {
        let offsets = &offsets[..offsets.len().min(MAX_DRAW_OFFSETS)];
        let unchanged =
            offsets.len() <= self.offsets.len() && self.offsets[..offsets.len()] == *offsets;
        if unchanged {
            return;
        }
        if self.offsets.len() < offsets.len() {
            self.offsets.resize(offsets.len(), [0, 0]);
        }
        self.offsets[..offsets.len()].copy_from_slice(offsets);
        // Whole vectors, from the first pair.
        let pairs = self.offsets.len().div_ceil(2);
        self.offsets.resize(pairs * 2, [0, 0]);
        queue.write_buffer(&self.offsets_buffer, 0, unsafe {
            slice::from_raw_parts(
                self.offsets.as_ptr() as *const u8,
                mem::size_of_val(&self.offsets[..]),
            )
        });
    }

    /// Updates the `Viewport` with the given `resolution`.
    pub fn update(&mut self, queue: &Queue, resolution: Resolution) {
        if self.params.screen_resolution != resolution {
            self.params.screen_resolution = resolution;

            queue.write_buffer(&self.params_buffer, 0, unsafe {
                slice::from_raw_parts(
                    &self.params as *const Params as *const u8,
                    mem::size_of::<Params>(),
                )
            });
        }
    }

    /// Returns the current resolution of the `Viewport`.
    pub fn resolution(&self) -> Resolution {
        self.params.screen_resolution
    }
}
