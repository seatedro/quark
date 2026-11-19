//! What a glyph atlas has done and what it holds.

use super::entries::State;
use super::pages::Role;
use super::{ContentType, GlyphAtlas};

/// Counts of the work a glyph atlas has done since it was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasStats {
    /// Glyphs admitted because the cache did not hold them, blank ones
    /// included (not every raster attempt: failures count apart).
    pub misses: u64,
    /// Cached glyphs dropped to make room for others or to trim.
    pub evictions: u64,
    /// Atlas pages created (each a new texture; no cached glyph moves).
    pub growths: u64,
    /// Glyphs rasterized again because the atlas moved them. Always zero:
    /// pages never grow, and evacuation copies on the GPU.
    pub rerasterized: u64,
    /// Bytes of glyph pixels uploaded (gutters excluded).
    pub upload_bytes: u64,
    /// Bytes staged for uploads, gutters and row padding included.
    pub padded_upload_bytes: u64,
    /// Buffer-to-texture copy commands recorded for glyph uploads.
    pub copy_commands: u64,
    /// Writes of staged pixels into transfer buffers.
    pub transfer_chunks: u64,
    /// Glyphs uploaded one by one through the queue (oversized, or past
    /// the staging limit).
    pub unbatched_uploads: u64,
    /// Raster attempts that failed (unsupported, invalid, too large, or a
    /// platform error).
    pub raster_failures: u64,
    /// Glyphs drawn by the font system path because their font instance
    /// could not be prepared for the rasterizer.
    pub fallbacks: u64,
    /// Glyphs the native rasterizer could not draw, drawn by swash.
    pub native_fallbacks: u64,
    /// Glyphs not drawn at all (too large even alone, or overflow staging
    /// full).
    pub dropped_glyphs: u64,
    /// Pages released (emptied by eviction, trimming, or evacuation).
    pub pages_released: u64,
    /// Frames drawn in overflow mode, and the ordinals they drew in.
    pub overflow_frames: u64,
    pub overflow_segments: u64,
    /// Misses that found no room although a page had enough free area.
    pub fragmented_misses: u64,
    /// Sparse pages emptied by moving their glyphs, and the bytes copied.
    pub evacuated_pages: u64,
    pub evacuated_bytes: u64,
}

impl std::ops::Add for AtlasStats {
    type Output = Self;

    fn add(self, o: Self) -> Self {
        Self {
            misses: self.misses + o.misses,
            evictions: self.evictions + o.evictions,
            growths: self.growths + o.growths,
            rerasterized: self.rerasterized + o.rerasterized,
            upload_bytes: self.upload_bytes + o.upload_bytes,
            padded_upload_bytes: self.padded_upload_bytes + o.padded_upload_bytes,
            copy_commands: self.copy_commands + o.copy_commands,
            transfer_chunks: self.transfer_chunks + o.transfer_chunks,
            unbatched_uploads: self.unbatched_uploads + o.unbatched_uploads,
            raster_failures: self.raster_failures + o.raster_failures,
            fallbacks: self.fallbacks + o.fallbacks,
            native_fallbacks: self.native_fallbacks + o.native_fallbacks,
            dropped_glyphs: self.dropped_glyphs + o.dropped_glyphs,
            pages_released: self.pages_released + o.pages_released,
            overflow_frames: self.overflow_frames + o.overflow_frames,
            overflow_segments: self.overflow_segments + o.overflow_segments,
            fragmented_misses: self.fragmented_misses + o.fragmented_misses,
            evacuated_pages: self.evacuated_pages + o.evacuated_pages,
            evacuated_bytes: self.evacuated_bytes + o.evacuated_bytes,
        }
    }
}

/// What a glyph atlas holds now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasMemory {
    /// Shared pages by kind, dedicated (oversized glyph) textures, and
    /// overflow pages.
    pub mask_pages: u32,
    pub color_pages: u32,
    pub dedicated_pages: u32,
    pub overflow_pages: u32,
    /// Texture bytes of each.
    pub mask_bytes: u64,
    pub color_bytes: u64,
    pub dedicated_bytes: u64,
    pub overflow_bytes: u64,
    /// Bytes of pages released this frame, still counted against the hard
    /// limit until the frame is submitted.
    pub retired_bytes: u64,
    /// Everything above.
    pub total_bytes: u64,
    /// The most `total_bytes` has been.
    pub peak_bytes: u64,
    pub target_bytes: u64,
    pub hard_limit_bytes: u64,
    /// Glyph entries (resident, blank, and failed) and resident ones.
    pub entries: u32,
    pub resident: u32,
    /// Bytes the upload staging holds for reuse, CPU shelves and transfer
    /// buffer.
    pub staging_bytes: u64,
}

impl GlyphAtlas {
    pub(crate) fn memory(&self) -> AtlasMemory {
        let mut memory = AtlasMemory {
            retired_bytes: self.retired_bytes,
            peak_bytes: self.peak_bytes,
            target_bytes: self.limits.target_bytes,
            hard_limit_bytes: self.limits.hard_limit_bytes,
            staging_bytes: self
                .shelves
                .iter()
                .map(|s| s.data.capacity() as u64)
                .sum::<u64>()
                + self.transfer.as_ref().map_or(0, wgpu::Buffer::size),
            ..AtlasMemory::default()
        };
        for page in self.pages.iter().flatten() {
            let (pages, bytes) = match (page.role, page.kind) {
                (Role::Shared, ContentType::Mask) => {
                    (&mut memory.mask_pages, &mut memory.mask_bytes)
                }
                (Role::Shared, ContentType::Color) => {
                    (&mut memory.color_pages, &mut memory.color_bytes)
                }
                (Role::Dedicated, _) => (&mut memory.dedicated_pages, &mut memory.dedicated_bytes),
                (Role::Overflow, _) => (&mut memory.overflow_pages, &mut memory.overflow_bytes),
            };
            *pages += 1;
            *bytes += page.bytes;
        }
        memory.total_bytes = self.total_bytes();
        for entry in &self.entries {
            match entry.state {
                State::Free => {}
                State::Resident(_) => {
                    memory.entries += 1;
                    memory.resident += 1;
                }
                State::Blank | State::Failed { .. } => memory.entries += 1,
            }
        }
        memory
    }
}
