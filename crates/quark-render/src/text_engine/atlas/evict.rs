//! Making room: evicting unpinned glyphs, releasing whole pages, and
//! evacuating sparse ones.

use super::entries::{Lru, State};
use super::pages::{Role, bytes_per_pixel};
use super::upload::Move;
use super::{Allocation, ContentType, GUTTER, GlyphAtlas, Placed};

/// The most pixel bytes one frame's evacuation copies.
const EVACUATION_BYTES: u64 = 4 << 20;
/// A page whose allocated area is at most this fraction is sparse enough
/// to evacuate.
const SPARSE_PAGE: f32 = 0.25;

impl GlyphAtlas {
    /// Evicts the least recently used unpinned glyph of `kind`; false when
    /// every one is pinned.
    pub(super) fn evict_one(&mut self, kind: ContentType) -> bool {
        match self.least_recent(Lru::of(kind)) {
            Some(tail) if self.entries[tail as usize].used != self.frame => {
                self.remove(tail);
                true
            }
            _ => false,
        }
    }

    /// Releases the shared page with the fewest glyphs none of which is
    /// pinned, evicting them; false when every page holds a pinned glyph.
    pub(super) fn release_unpinned_page(&mut self) -> bool {
        let mut pinned = vec![false; self.pages.len()];
        for entry in &self.entries {
            if let State::Resident(placed) = entry.state
                && entry.used == self.frame
            {
                pinned[placed.page as usize] = true;
            }
        }
        let candidate = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(slot, page)| Some((slot, page.as_ref()?)))
            .filter(|(slot, page)| page.role != Role::Overflow && !pinned[*slot])
            .min_by_key(|(_, page)| page.live)
            .map(|(slot, _)| slot as u32);
        let Some(slot) = candidate else {
            return false;
        };
        self.empty_page(slot);
        if self.pages[slot as usize].is_some() {
            self.release_page(slot);
        }
        true
    }

    /// Evicts every glyph on page `slot`.
    fn empty_page(&mut self, slot: u32) {
        for index in 0..self.entries.len() as u32 {
            if let State::Resident(placed) = self.entries[index as usize].state
                && placed.page == slot
            {
                self.remove(index);
            }
        }
    }

    /// Moves the glyphs of the sparsest shared page into holes of the
    /// others of its kind, then releases it, copying at most
    /// [`EVACUATION_BYTES`]. Moved glyphs get new generations, so only the
    /// runs that draw them prepare again; nothing is rasterized.
    pub(super) fn evacuate(&mut self) {
        let candidate = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(slot, page)| Some((slot as u32, page.as_ref()?)))
            .filter(|(_, page)| {
                page.role == Role::Shared
                    && page.live > 0
                    && (page.allocator.allocated_space() as f32)
                        <= SPARSE_PAGE * (page.width * page.height) as f32
            })
            .filter(|(_, page)| self.shared_pages(page.kind) > 1)
            .min_by_key(|(_, page)| page.allocator.allocated_space())
            .map(|(slot, page)| (slot, page.kind));
        let Some((source, kind)) = candidate else {
            return;
        };
        let bpp = u64::from(bytes_per_pixel(kind));
        let moving: Vec<(u32, Placed)> = (0..self.entries.len() as u32)
            .filter_map(|i| match self.entries[i as usize].state {
                State::Resident(p) if p.page == source => Some((i, p)),
                _ => None,
            })
            .collect();
        let bytes: u64 = moving
            .iter()
            .map(|(_, p)| u64::from(p.width + 2 * GUTTER) * u64::from(p.rows()) * bpp)
            .sum();
        if bytes > EVACUATION_BYTES {
            return;
        }
        // Destinations first, so a page without room for all leaves
        // everything where it was.
        let mut targets = Vec::with_capacity(moving.len());
        for (_, p) in &moving {
            match self.allocate_elsewhere(kind, p.width + 2 * GUTTER, p.rows(), source) {
                Some(found) => targets.push(found),
                None => {
                    for allocation in targets {
                        if let Some(page) = self.pages[allocation.page as usize].as_mut() {
                            page.allocator.deallocate(allocation.id);
                        }
                    }
                    return;
                }
            }
        }
        let from = self.pages[source as usize]
            .as_ref()
            .expect("source page")
            .texture
            .clone();
        for ((index, p), to) in moving.into_iter().zip(targets) {
            self.moves.push(Move {
                from: from.clone(),
                from_origin: (p.x - GUTTER, p.y - GUTTER),
                to: to.page,
                to_origin: (to.x, to.y),
                size: (p.width + 2 * GUTTER, p.rows()),
            });
            let entry = &mut self.entries[index as usize];
            entry.state = State::Resident(Placed {
                page: to.page,
                alloc: to.id,
                x: to.x + GUTTER,
                y: to.y + GUTTER,
                ..p
            });
            entry.generation = entry.generation.wrapping_add(1);
            if let Some(page) = self.pages[to.page as usize].as_mut() {
                page.live += 1;
            }
        }
        if let Some(page) = self.pages[source as usize].as_mut() {
            page.live = 0;
        }
        self.release_page(source);
        self.stats.evacuated_pages += 1;
        self.stats.evacuated_bytes += bytes;
    }

    /// Room in a shared page of `kind` other than `source`, without new
    /// pages or evictions.
    fn allocate_elsewhere(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
        source: u32,
    ) -> Option<Allocation> {
        for slot in (0..self.pages.len()).rev() {
            if slot as u32 == source {
                continue;
            }
            let Some(page) = self.pages[slot].as_mut() else {
                continue;
            };
            if page.role != Role::Shared || page.kind != kind {
                continue;
            }
            if let Some(allocation) = page.allocate(slot as u32, w, h) {
                return Some(allocation);
            }
        }
        None
    }
}
