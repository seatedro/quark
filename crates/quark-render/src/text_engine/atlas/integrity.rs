//! The atlas's invariants, checked after mutations in debug builds, and
//! hooks for tests to reach states frames rarely do.

use super::GlyphAtlas;
use super::entries::State;
use super::pages::Role;

/// A broken invariant of a [`GlyphAtlas`].
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct IntegrityError(String);

impl GlyphAtlas {
    /// Checks the entry slab, the LRU lists, the key map, and the page
    /// counts against each other.
    pub(crate) fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let fail = |what: String| Err(IntegrityError(what));
        self.verify_lists().map_err(IntegrityError)?;
        let mut live = vec![0u32; self.pages.len()];
        let mut used = 0;
        for (index, entry) in self.entries.iter().enumerate() {
            if matches!(entry.state, State::Free) {
                if !self.unlisted(index) {
                    return fail(format!("free entry {index} is listed"));
                }
                continue;
            }
            used += 1;
            if self.map.get(&entry.key) != Some(&(index as u32)) {
                return fail(format!("entry {index} missing from the key map"));
            }
            if let State::Resident(p) = entry.state {
                match self.pages.get(p.page as usize).and_then(Option::as_ref) {
                    Some(page) if page.kind == p.kind => live[p.page as usize] += 1,
                    _ => return fail(format!("entry {index} on a missing page")),
                }
            }
        }
        if used != self.map.len() || used + self.free_entries.len() != self.entries.len() {
            return fail("entry counts disagree".into());
        }
        for (slot, page) in self.pages.iter().enumerate() {
            if let Some(page) = page
                && page.role != Role::Overflow
                && page.live != live[slot]
            {
                return fail(format!(
                    "page {slot} counts {} live, holds {}",
                    page.live, live[slot]
                ));
            }
        }
        Ok(())
    }

    /// Evicts every glyph the current frame has not used.
    #[cfg(test)]
    pub(crate) fn evict_unused(&mut self) {
        for index in 0..self.entries.len() as u32 {
            let entry = &self.entries[index as usize];
            if !matches!(entry.state, State::Free) && entry.used != self.frame {
                self.remove(index);
            }
        }
        debug_assert!(
            self.verify_integrity().is_ok(),
            "{:?}",
            self.verify_integrity()
        );
    }

    /// Evacuates a sparse page at the start of the next frame, as a
    /// fragmented miss would.
    #[cfg(test)]
    pub(crate) fn request_evacuation(&mut self) {
        self.fragmented = true;
    }
}
