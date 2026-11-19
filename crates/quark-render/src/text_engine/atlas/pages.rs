//! Atlas pages: creating and releasing them, and allocating rectangles in
//! them.

use etagere::{AtlasAllocator, size2};
use wgpu::{
    BindGroup, Extent3d, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureView, TextureViewDescriptor,
};

use super::{Allocation, ContentType, GlyphAtlas, Placed, PrepareError};

/// What a page holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Glyphs of one kind, allocated from the page's free space.
    Shared,
    /// One glyph too large for a shared page, in a texture of its size.
    Dedicated,
    /// This ordinal's glyphs of a frame in overflow mode.
    Overflow,
}

pub(super) struct Page {
    /// Unique across the atlas's life, unlike the slot a page sits in.
    pub(super) uid: u32,
    pub(super) kind: ContentType,
    pub(super) role: Role,
    pub(super) texture: Texture,
    pub(super) view: TextureView,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) allocator: AtlasAllocator,
    /// Resident glyphs on it.
    pub(super) live: u32,
    pub(super) bytes: u64,
}

impl Page {
    /// Room for a `w` by `h` rectangle in this page, which sits in `slot`.
    pub(super) fn allocate(&mut self, slot: u32, w: u32, h: u32) -> Option<Allocation> {
        let allocation = self.allocator.allocate(size2(w as i32, h as i32))?;
        let min = allocation.rectangle.min;
        Some(Allocation {
            page: slot,
            id: allocation.id,
            x: min.x as u32,
            y: min.y as u32,
        })
    }
}

pub(super) fn bytes_per_pixel(kind: ContentType) -> u32 {
    match kind {
        ContentType::Mask => 1,
        ContentType::Color => 4,
    }
}

pub(super) fn texture_format(kind: ContentType) -> TextureFormat {
    match kind {
        ContentType::Mask => TextureFormat::R8Unorm,
        ContentType::Color => TextureFormat::Rgba8UnormSrgb,
    }
}

impl GlyphAtlas {
    /// Bytes of every page, those released this frame included.
    pub(super) fn total_bytes(&self) -> u64 {
        self.total_bytes_without_retired() + self.retired_bytes
    }

    pub(super) fn total_bytes_without_retired(&self) -> u64 {
        self.pages.iter().flatten().map(|p| p.bytes).sum()
    }

    pub(super) fn shared_pages(&self, kind: ContentType) -> usize {
        self.pages
            .iter()
            .flatten()
            .filter(|p| p.role == Role::Shared && p.kind == kind)
            .count()
    }

    /// Space for a `w` by `h` rectangle (gutter included) of `kind`.
    pub(super) fn allocate(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> Result<Allocation, PrepareError> {
        let side = self.page_size;
        if w > side || h > side {
            return self.dedicated(kind, w, h);
        }
        let page_bytes = u64::from(side) * u64::from(side) * u64::from(bytes_per_pixel(kind));
        let mut fragmented = false;
        loop {
            if let Some(found) = self.try_pages(kind, w, h, &mut fragmented) {
                return Ok(found);
            }
            if self.total_bytes() + page_bytes <= self.limits.target_bytes {
                return Ok(self.allocate_in_new_page(kind, w, h));
            }
            if self.evict_one(kind) {
                continue;
            }
            if self.total_bytes() + page_bytes + self.overflow_reserve()
                <= self.limits.hard_limit_bytes
            {
                return Ok(self.allocate_in_new_page(kind, w, h));
            }
            if self.release_unpinned_page() {
                continue;
            }
            if fragmented {
                self.stats.fragmented_misses += 1;
                self.fragmented = true;
            }
            return Err(PrepareError::AtlasFull);
        }
    }

    /// Room in an existing shared page of `kind`, newest first. Sets
    /// `fragmented` when a page had the area but not the shape.
    fn try_pages(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
        fragmented: &mut bool,
    ) -> Option<Allocation> {
        let need = (w * h) as i32;
        for slot in (0..self.pages.len()).rev() {
            let Some(page) = self.pages[slot].as_mut() else {
                continue;
            };
            if page.role != Role::Shared || page.kind != kind {
                continue;
            }
            if let Some(allocation) = page.allocate(slot as u32, w, h) {
                return Some(allocation);
            }
            if page.allocator.free_space() >= need {
                *fragmented = true;
            }
        }
        if *fragmented {
            self.stats.fragmented_misses += 1;
            self.fragmented = true;
        }
        None
    }

    fn allocate_in_new_page(&mut self, kind: ContentType, w: u32, h: u32) -> Allocation {
        let side = self.page_size;
        let slot = self.new_page(kind, Role::Shared, side, side);
        self.pages[slot as usize]
            .as_mut()
            .and_then(|page| page.allocate(slot, w, h))
            .expect("a glyph no larger than an empty page fits it")
    }

    fn dedicated(&mut self, kind: ContentType, w: u32, h: u32) -> Result<Allocation, PrepareError> {
        let bytes = u64::from(w) * u64::from(h) * u64::from(bytes_per_pixel(kind));
        while self.total_bytes() + bytes + self.overflow_reserve() > self.limits.hard_limit_bytes {
            if !self.release_unpinned_page() {
                return Err(PrepareError::AtlasFull);
            }
        }
        let slot = self.new_page(kind, Role::Dedicated, w, h);
        Ok(self.pages[slot as usize]
            .as_mut()
            .and_then(|page| page.allocate(slot, w, h))
            .expect("a dedicated page fits its glyph"))
    }

    /// A new, empty page in a free slot.
    pub(super) fn new_page(
        &mut self,
        kind: ContentType,
        role: Role,
        width: u32,
        height: u32,
    ) -> u32 {
        let texture = self.device.create_texture(&TextureDescriptor {
            label: Some(match kind {
                ContentType::Mask => "quark text mask page",
                ContentType::Color => "quark text color page",
            }),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: texture_format(kind),
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let page = Page {
            uid: self.next_uid,
            kind,
            role,
            texture,
            view,
            width,
            height,
            allocator: AtlasAllocator::new(size2(width as i32, height as i32)),
            live: 0,
            bytes: u64::from(width) * u64::from(height) * u64::from(bytes_per_pixel(kind)),
        };
        self.next_uid += 1;
        self.stats.growths += 1;
        let slot = match self.free_pages.pop() {
            Some(slot) => {
                self.pages[slot as usize] = Some(page);
                slot
            }
            None => {
                self.pages.push(Some(page));
                self.pages.len() as u32 - 1
            }
        };
        self.peak_bytes = self.peak_bytes.max(self.total_bytes());
        slot
    }

    /// Release the page in `slot`. Its texture lives until the frame's
    /// commands are recorded.
    pub(super) fn release_page(&mut self, slot: u32) {
        let Some(page) = self.pages[slot as usize].take() else {
            return;
        };
        self.free_pages.push(slot);
        self.retired_bytes += page.bytes;
        self.bind_groups
            .retain(|&(color, mask, _)| color != page.uid && mask != page.uid);
        self.pending.retain(|copy| copy.page != slot);
        self.retired.push(page.texture);
        self.stats.pages_released += 1;
    }

    /// Return the rectangle of a resident glyph to its page. An emptied
    /// page goes when it is oversized, the atlas is over its target, or
    /// another page of its kind remains.
    pub(super) fn free_rectangle(&mut self, placed: &Placed) {
        let Some(page) = self.pages[placed.page as usize].as_mut() else {
            return;
        };
        page.allocator.deallocate(placed.alloc);
        page.live -= 1;
        let (empty, dedicated) = (page.live == 0, page.role == Role::Dedicated);
        if empty
            && (dedicated
                || self.total_bytes_without_retired() > self.limits.target_bytes
                || self.shared_pages(placed.kind) > 1)
        {
            self.release_page(placed.page);
        }
    }

    /// The bind group drawing glyphs from color page `color` and mask page
    /// `mask` (either may be absent).
    pub(crate) fn bind_group(&mut self, color: Option<u32>, mask: Option<u32>) -> BindGroup {
        let page = |slot: Option<u32>| slot.and_then(|s| self.pages[s as usize].as_ref());
        let (color_page, mask_page) = (page(color), page(mask));
        let key = (
            color_page.map_or(0, |p| p.uid),
            mask_page.map_or(0, |p| p.uid),
        );
        if let Some((_, _, group)) = self.bind_groups.iter().find(|(c, m, _)| (*c, *m) == key) {
            return group.clone();
        }
        let group = self.cache.create_atlas_bind_group(
            &self.device,
            color_page.map_or(&self.dummy[1], |p| &p.view),
            mask_page.map_or(&self.dummy[0], |p| &p.view),
        );
        self.bind_groups.push((key.0, key.1, group.clone()));
        group
    }
}
