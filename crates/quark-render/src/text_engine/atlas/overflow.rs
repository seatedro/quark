//! Overflow mode: drawing a frame whose glyphs do not all fit the atlas.

use std::collections::HashMap;

use super::pages::{Role, bytes_per_pixel};
use super::{Allocation, ContentType, Fx, GlyphAtlas, GlyphKey, Placed, kind_slot};

/// Frames without overflow after which the overflow pages are released.
const OVERFLOW_KEEP_FRAMES: u64 = 120;

/// The overflow pages, and whether the frame being prepared overflows.
#[derive(Default)]
pub(super) struct Overflow {
    /// Page slots by kind (mask, color), kept between overflow frames.
    pages: [Option<u32>; 2],
    /// The last frame drawn in overflow mode.
    last_frame: u64,
    mode: Mode,
}

#[derive(Default)]
enum Mode {
    /// Every glyph of the frame is resident.
    #[default]
    Off,
    /// The frame draws in overflow mode.
    On(Ordinal),
}

/// The overflow ordinal the frame's glyphs are being placed for.
struct Ordinal {
    number: u32,
    /// Its glyphs, in its overflow pages.
    placed: HashMap<GlyphKey, Placed, Fx>,
    /// Kinds (mask, color) whose resident pages are full this frame.
    full: [bool; 2],
}

impl Overflow {
    pub(super) fn is_active(&self) -> bool {
        matches!(self.mode, Mode::On(_))
    }

    fn ordinal_mut(&mut self) -> Option<&mut Ordinal> {
        match &mut self.mode {
            Mode::On(ordinal) => Some(ordinal),
            Mode::Off => None,
        }
    }

    /// Where glyph `key` is in this ordinal's overflow pages, and the
    /// ordinal; `None` outside overflow mode.
    pub(super) fn placed(&self, key: &GlyphKey) -> Option<(Placed, u32)> {
        let Mode::On(ordinal) = &self.mode else {
            return None;
        };
        let placed = *ordinal.placed.get(key)?;
        Some((placed, ordinal.number))
    }

    /// Note glyph `key` placed in this ordinal's overflow pages.
    pub(super) fn remember(&mut self, key: GlyphKey, placed: Placed) {
        if let Some(ordinal) = self.ordinal_mut() {
            ordinal.placed.insert(key, placed);
        }
    }

    /// Whether glyphs of `kind` go straight to the overflow pages: their
    /// resident pages filled earlier this frame.
    pub(super) fn kind_full(&self, kind: ContentType) -> bool {
        match &self.mode {
            Mode::Off => false,
            Mode::On(ordinal) => ordinal.full[kind_slot(kind)],
        }
    }

    pub(super) fn mark_full(&mut self, kind: ContentType) {
        if let Some(ordinal) = self.ordinal_mut() {
            ordinal.full[kind_slot(kind)] = true;
        }
    }

    /// Forget the overflow pages, which the atlas released.
    pub(super) fn forget_pages(&mut self) {
        self.pages = [None, None];
    }
}

impl GlyphAtlas {
    /// Bytes kept free below the hard limit for one overflow page per
    /// kind, unless those pages already exist.
    pub(super) fn overflow_reserve(&self) -> u64 {
        let side = u64::from(self.page_size) * u64::from(self.page_size);
        [ContentType::Mask, ContentType::Color]
            .into_iter()
            .filter(|&kind| self.overflow.pages[kind_slot(kind)].is_none())
            .map(|kind| side * u64::from(bytes_per_pixel(kind)))
            .sum()
    }

    /// Switches the frame being prepared to overflow mode: glyphs not
    /// resident go through the overflow pages, ordinal by ordinal.
    pub(crate) fn begin_overflow(&mut self) {
        self.overflow.mode = Mode::On(Ordinal {
            number: 0,
            placed: HashMap::default(),
            full: [false; 2],
        });
        self.overflow.last_frame = self.frame;
        self.clear_overflow_pages();
        // Copies staged for overflow pages by an earlier attempt are void.
        let overflow = self.overflow.pages;
        self.pending
            .retain(|copy| !overflow.contains(&Some(copy.page)));
        self.stats.overflow_frames += 1;
        self.stats.overflow_segments += 1;
    }

    /// Leaves overflow mode at the end of a frame, releasing the overflow
    /// pages once no frame has needed them for a while.
    pub(super) fn end_overflow(&mut self) {
        self.overflow.mode = Mode::Off;
        if self.frame.saturating_sub(self.overflow.last_frame) > OVERFLOW_KEEP_FRAMES {
            for slot in std::mem::take(&mut self.overflow.pages)
                .into_iter()
                .flatten()
            {
                self.release_page(slot);
            }
        }
    }

    /// Room in the current overflow ordinal's page of `kind`, starting the
    /// next ordinal when it is full, and that ordinal; `None` for a glyph
    /// larger than an empty overflow page, or outside overflow mode.
    pub(super) fn overflow_allocate(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> Option<(Allocation, u32)> {
        let mut number = self.overflow.ordinal_mut()?.number;
        let i = kind_slot(kind);
        let slot = match self.overflow.pages[i] {
            Some(slot) => slot,
            None => {
                let side = self.page_size;
                let slot = self.new_page(kind, Role::Overflow, side, side);
                self.overflow.pages[i] = Some(slot);
                slot
            }
        };
        let page = self.pages[slot as usize].as_mut()?;
        if let Some(allocation) = page.allocate(slot, w, h) {
            return Some((allocation, number));
        }
        if page.allocator.is_empty() {
            return None;
        }
        // The next ordinal reuses both pages from empty.
        let ordinal = self.overflow.ordinal_mut()?;
        ordinal.number += 1;
        ordinal.placed.clear();
        number = ordinal.number;
        self.stats.overflow_segments += 1;
        self.clear_overflow_pages();
        let allocation = self.pages[slot as usize].as_mut()?.allocate(slot, w, h)?;
        Some((allocation, number))
    }

    /// Empty both overflow pages for the next ordinal's glyphs.
    fn clear_overflow_pages(&mut self) {
        for slot in self.overflow.pages.into_iter().flatten() {
            if let Some(page) = self.pages[slot as usize].as_mut() {
                page.allocator.clear();
            }
        }
    }
}
