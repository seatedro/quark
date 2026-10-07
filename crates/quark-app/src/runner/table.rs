//! A small generational table for the runner's windows. Handles carry the
//! slot's generation, so a handle kept after its window closed stops matching
//! once the slot is reused.

/// Identifies one window opened through the runner. Stays valid until the
/// window closes; afterwards every lookup with it fails, even when a new
/// window reuses the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowHandle {
    index: u32,
    generation: u32,
}

#[derive(Debug)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

#[derive(Debug)]
pub(super) struct WindowTable<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for WindowTable<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> WindowTable<T> {
    pub(super) fn insert(&mut self, value: T) -> WindowHandle {
        let handle = match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                slot.value = Some(value);
                WindowHandle {
                    index,
                    generation: slot.generation,
                }
            }
            None => {
                let index = self.slots.len() as u32;
                self.slots.push(Slot {
                    generation: 0,
                    value: Some(value),
                });
                WindowHandle {
                    index,
                    generation: 0,
                }
            }
        };
        debug_assert!(self.verify_integrity().is_ok());
        handle
    }

    pub(super) fn remove(&mut self, handle: WindowHandle) -> Option<T> {
        let slot = self.slot_mut(handle)?;
        let value = slot.value.take()?;
        // Bump on removal so the old handle is stale even before reuse.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        debug_assert!(self.verify_integrity().is_ok());
        Some(value)
    }

    pub(super) fn get(&self, handle: WindowHandle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    pub(super) fn get_mut(&mut self, handle: WindowHandle) -> Option<&mut T> {
        self.slot_mut(handle)?.value.as_mut()
    }

    pub(super) fn handles(&self) -> Vec<WindowHandle> {
        self.iter().map(|(handle, _)| handle).collect()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (WindowHandle, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let value = slot.value.as_ref()?;
            Some((
                WindowHandle {
                    index: index as u32,
                    generation: slot.generation,
                },
                value,
            ))
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.len() == self.free.len()
    }

    fn slot_mut(&mut self, handle: WindowHandle) -> Option<&mut Slot<T>> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        (slot.generation == handle.generation).then_some(slot)
    }

    /// Every free index points at an empty slot exactly once, and every empty
    /// slot is on the free list.
    pub(super) fn verify_integrity(&self) -> Result<(), String> {
        let mut seen = vec![false; self.slots.len()];
        for &index in &self.free {
            let Some(slot) = self.slots.get(index as usize) else {
                return Err(format!("free index {index} is out of range"));
            };
            if slot.value.is_some() {
                return Err(format!("free index {index} holds a value"));
            }
            if std::mem::replace(&mut seen[index as usize], true) {
                return Err(format!("free index {index} is listed twice"));
            }
        }
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.value.is_none() && !seen[index] {
                return Err(format!("empty slot {index} is not on the free list"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_of_closed_window_stays_stale_after_slot_reuse() {
        let mut table = WindowTable::default();
        let first = table.insert("first");
        assert_eq!(table.remove(first), Some("first"));

        let second = table.insert("second");
        assert_eq!(table.get(first), None);
        assert_eq!(table.remove(first), None);
        assert_eq!(table.get(second), Some(&"second"));
    }
}
