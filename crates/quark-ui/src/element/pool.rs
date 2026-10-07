//! Frame memory for the element tree. A view builds a fresh tree every
//! frame and drops the last one, so the tree's storage is recycled instead
//! of freed: each element type keeps a free list of its boxes, and child
//! lists keep their buffers. A steady frame builds its tree from last
//! frame's memory without calling the allocator, and the builder API stays
//! plain values (no arena lifetimes or handles to thread through views).
//!
//! The lists live in thread locals because views build elements before any
//! context exists. Each list is capped, so one huge frame does not pin its
//! memory for the rest of the session.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use super::AnyElement;

/// Most recycled boxes kept per element type, and child lists kept.
const KEEP: usize = 1 << 14;

/// A recyclable element box: its contents can be dropped while the box is
/// kept, and refilled when an element of the same type is built.
pub(super) trait Recycle: Any {
    /// Drop the element and its states, keeping the allocation.
    fn empty(&mut self);
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

type Boxes = Vec<Box<dyn super::traits::HolderImpl>>;

thread_local! {
    static BOXES: RefCell<HashMap<TypeId, Boxes>> = RefCell::new(HashMap::new());
    static CHILD_LISTS: RefCell<Vec<Vec<AnyElement>>> = const { RefCell::new(Vec::new()) };
}

/// A recycled box of type `T`, if one is free.
pub(super) fn take_box<T: 'static>() -> Option<Box<dyn super::traits::HolderImpl>> {
    BOXES
        .try_with(|boxes| {
            boxes
                .borrow_mut()
                .get_mut(&TypeId::of::<T>())
                .and_then(Vec::pop)
        })
        .ok()
        .flatten()
}

/// Return an emptied box of type `T` to its free list.
pub(super) fn give_box<T: 'static>(holder: Box<dyn super::traits::HolderImpl>) {
    let _ = BOXES.try_with(|boxes| {
        let mut boxes = boxes.borrow_mut();
        let free = boxes.entry(TypeId::of::<T>()).or_default();
        if free.len() < KEEP {
            free.push(holder);
        }
    });
}

/// A child list whose buffer goes back to a free list when dropped.
#[derive(Default)]
pub(super) struct ChildList(Vec<AnyElement>);

impl ChildList {
    pub(super) fn new() -> Self {
        let buffer = CHILD_LISTS
            .try_with(|lists| lists.borrow_mut().pop())
            .ok()
            .flatten();
        Self(buffer.unwrap_or_default())
    }
}

impl Deref for ChildList {
    type Target = Vec<AnyElement>;

    fn deref(&self) -> &Vec<AnyElement> {
        &self.0
    }
}

impl DerefMut for ChildList {
    fn deref_mut(&mut self) -> &mut Vec<AnyElement> {
        &mut self.0
    }
}

impl Drop for ChildList {
    fn drop(&mut self) {
        // Children recycle themselves first; the free list is not borrowed
        // while they do.
        self.0.clear();
        if self.0.capacity() == 0 {
            return;
        }
        let buffer = std::mem::take(&mut self.0);
        let _ = CHILD_LISTS.try_with(|lists| {
            let mut lists = lists.borrow_mut();
            if lists.len() < KEEP {
                lists.push(buffer);
            }
        });
    }
}
