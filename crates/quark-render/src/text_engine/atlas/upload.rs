//! Uploading glyph pixels: staged in CPU shelves, written into one
//! transfer buffer, and copied into their pages by the frame's encoder.

use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoder, Extent3d, Origin3d, Queue, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
};

use super::pages::bytes_per_pixel;
use super::rasterize::Drawn;
use super::{ContentType, GUTTER, GlyphAtlas, Placed, kind_slot};

/// The most bytes one frame stages for uploads; past it uploads go one by
/// one through the queue (and overflow glyphs are dropped).
const MAX_STAGING_BYTES: usize = 16 << 20;

/// A CPU image whose rows are a valid buffer copy pitch, packed with
/// glyph rectangles in shelves.
#[derive(Default)]
pub(super) struct Shelf {
    pitch: usize,
    bpp: usize,
    pub(super) data: Vec<u8>,
    x: usize,
    y: usize,
    height: usize,
}

impl Shelf {
    fn new(width: u32, bpp: u32) -> Self {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        Self {
            pitch: (width as usize * bpp as usize).div_ceil(align) * align,
            bpp: bpp as usize,
            ..Self::default()
        }
    }

    /// Shelves for pages of side `page_size`, by kind (mask, color).
    pub(super) fn pair(page_size: u32) -> [Self; 2] {
        [ContentType::Mask, ContentType::Color]
            .map(|kind| Self::new(page_size, bytes_per_pixel(kind)))
    }

    pub(super) fn reset(&mut self) {
        self.data.clear();
        self.x = 0;
        self.y = 0;
        self.height = 0;
    }

    /// Room for a `width` by `height` rectangle, zeroed, at the returned
    /// byte offset; `None` past the pitch or `budget` bytes in all.
    fn place(&mut self, width: usize, height: usize, budget: usize) -> Option<usize> {
        if width * self.bpp > self.pitch {
            return None;
        }
        if (self.x + width) * self.bpp > self.pitch {
            self.y += self.height;
            self.x = 0;
            self.height = 0;
        }
        let end = (self.y + height.max(self.height)) * self.pitch;
        if end > budget {
            return None;
        }
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        let offset = self.y * self.pitch + self.x * self.bpp;
        self.x += width;
        self.height = self.height.max(height);
        Some(offset)
    }
}

/// A staged glyph: its gutter-wrapped rectangle in a shelf and in a page.
pub(super) struct PendingCopy {
    kind: ContentType,
    offset: usize,
    pub(super) page: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    ordinal: u32,
}

/// A glyph moved by evacuation, copied page to page.
pub(super) struct Move {
    pub(super) from: Texture,
    pub(super) from_origin: (u32, u32),
    pub(super) to: u32,
    pub(super) to_origin: (u32, u32),
    pub(super) size: (u32, u32),
}

impl GlyphAtlas {
    /// Copies `drawn`'s pixels, gutter around them, into the staging
    /// shelves for `placed`'s page, for overflow ordinal `ordinal`. With
    /// `fallback`, a glyph that does not fit the shelves is written through
    /// the queue instead; without it, false.
    pub(super) fn stage(
        &mut self,
        drawn: &Drawn,
        placed: &Placed,
        ordinal: u32,
        fallback: bool,
    ) -> bool {
        let bpp = bytes_per_pixel(drawn.kind) as usize;
        let (w, h) = (
            drawn.width as usize + 2 * GUTTER as usize,
            drawn.rows() as usize,
        );
        let shelf_index = kind_slot(drawn.kind);
        let used = self.shelves.iter().map(|s| s.data.len()).sum::<usize>()
            - self.shelves[shelf_index].data.len();
        let budget = MAX_STAGING_BYTES.saturating_sub(used);
        let pixels = &self.scratch.pixels;
        let row_bytes = drawn.width as usize * bpp;
        let plane_rows = drawn.height as usize + 2 * GUTTER as usize;
        let write_rows = |out: &mut [u8], pitch: usize| {
            for plane in 0..drawn.planes as usize {
                let start = drawn.bytes.start + plane * drawn.plane_len;
                let source = &pixels[start..start + drawn.plane_len];
                for row in 0..drawn.height as usize {
                    let src = &source[row * drawn.stride..row * drawn.stride + row_bytes];
                    let at = (plane * plane_rows + row + GUTTER as usize) * pitch
                        + GUTTER as usize * bpp;
                    let dst = &mut out[at..at + row_bytes];
                    dst.copy_from_slice(src);
                    if drawn.premultiplied {
                        unpremultiply(dst);
                    }
                }
            }
        };
        self.stats.upload_bytes +=
            (row_bytes * drawn.height as usize * drawn.planes as usize) as u64;
        let shelf = &mut self.shelves[shelf_index];
        if let Some(offset) = shelf.place(w, h, budget) {
            let pitch = shelf.pitch;
            write_rows(&mut shelf.data[offset..], pitch);
            self.pending.push(PendingCopy {
                kind: drawn.kind,
                offset,
                page: placed.page,
                x: placed.x - GUTTER,
                y: placed.y - GUTTER,
                width: w as u32,
                height: h as u32,
                ordinal,
            });
            return true;
        }
        if !fallback {
            return false;
        }
        // Oversized for the shelves or past the staging limit: one write
        // through the queue, which lands before the frame's commands.
        let mut image = vec![0u8; w * h * bpp];
        write_rows(&mut image, w * bpp);
        self.stats.unbatched_uploads += 1;
        self.stats.padded_upload_bytes += image.len() as u64;
        if let Some(page) = self.pages[placed.page as usize].as_ref() {
            self.queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: placed.x - GUTTER,
                        y: placed.y - GUTTER,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((w * bpp) as u32),
                    rows_per_image: None,
                },
                Extent3d {
                    width: w as u32,
                    height: h as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        true
    }

    /// Records this frame's evacuation moves and staged uploads (all but
    /// later overflow ordinals') into `encoder`, ahead of its draws.
    pub(crate) fn record_uploads(&mut self, queue: &Queue, encoder: &mut CommandEncoder) {
        for m in self.moves.drain(..) {
            let Some(to) = self.pages[m.to as usize].as_ref() else {
                continue;
            };
            encoder.copy_texture_to_texture(
                TexelCopyTextureInfo {
                    texture: &m.from,
                    mip_level: 0,
                    origin: Origin3d {
                        x: m.from_origin.0,
                        y: m.from_origin.1,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                TexelCopyTextureInfo {
                    texture: &to.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: m.to_origin.0,
                        y: m.to_origin.1,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                Extent3d {
                    width: m.size.0,
                    height: m.size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        if self.pending.is_empty() {
            return;
        }
        // One write per shelf into the transfer buffer.
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;
        let mask_len = (self.shelves[0].data.len() as u64).div_ceil(align) * align;
        let color_len = (self.shelves[1].data.len() as u64).div_ceil(4) * 4;
        let needed = (mask_len + color_len).max(align);
        if self.transfer.as_ref().is_none_or(|b| b.size() < needed) {
            self.transfer = Some(self.device.create_buffer(&BufferDescriptor {
                label: Some("quark text uploads"),
                size: needed.next_power_of_two(),
                usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let transfer = self.transfer.as_ref().expect("transfer buffer");
        for (shelf, base) in self.shelves.iter_mut().zip([0, mask_len]) {
            if shelf.data.is_empty() {
                continue;
            }
            let padded = shelf.data.len().div_ceil(4) * 4;
            shelf.data.resize(padded, 0);
            queue.write_buffer(transfer, base, &shelf.data);
            self.stats.transfer_chunks += 1;
            self.stats.padded_upload_bytes += shelf.data.len() as u64;
        }
        self.shelf_base = [0, mask_len];
        self.stats.copy_commands += self.pending.iter().filter(|c| c.ordinal > 0).count() as u64;
        self.stats.copy_commands += self.record_ordinal(encoder, 0);
    }

    /// Records the uploads overflow ordinal `ordinal` draws from, after
    /// the previous ordinal's draws.
    pub(crate) fn record_overflow(&self, encoder: &mut CommandEncoder, ordinal: u32) {
        self.record_ordinal(encoder, ordinal);
    }

    /// Records the copies of `ordinal`'s staged glyphs; returns how many.
    fn record_ordinal(&self, encoder: &mut CommandEncoder, ordinal: u32) -> u64 {
        let Some(transfer) = self.transfer.as_ref() else {
            return 0;
        };
        let mut recorded = 0;
        for copy in self.pending.iter().filter(|c| c.ordinal == ordinal) {
            let Some(page) = self.pages[copy.page as usize].as_ref() else {
                continue;
            };
            let shelf = kind_slot(copy.kind);
            encoder.copy_buffer_to_texture(
                TexelCopyBufferInfo {
                    buffer: transfer,
                    layout: TexelCopyBufferLayout {
                        offset: self.shelf_base[shelf] + copy.offset as u64,
                        bytes_per_row: Some(self.shelves[shelf].pitch as u32),
                        rows_per_image: None,
                    },
                },
                TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: copy.x,
                        y: copy.y,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                Extent3d {
                    width: copy.width,
                    height: copy.height,
                    depth_or_array_layers: 1,
                },
            );
            recorded += 1;
        }
        recorded
    }
}

/// Straight color from premultiplied, in place: RGB divided by alpha,
/// zero where alpha is.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + a / 2)
                .checked_div(a)
                .map_or(0, |v| v.min(255) as u8);
        }
    }
}
