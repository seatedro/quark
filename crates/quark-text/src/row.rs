use std::collections::HashMap;

/// Height of a virtual-list row identified by `key` at a given width.
pub trait RowMeasure {
    fn measure(&mut self, key: u64, width: f32) -> f32;
}

impl<F: FnMut(u64, f32) -> f32> RowMeasure for F {
    fn measure(&mut self, key: u64, width: f32) -> f32 {
        self(key, width)
    }
}

/// Memoizes an inner [`RowMeasure`] for the current width. A width change
/// drops every cached height; call [`Self::invalidate`] when a row's content
/// changes.
#[derive(Debug)]
pub struct RowHeights<M> {
    inner: M,
    width_bits: u32,
    heights: HashMap<u64, f32>,
}

impl<M: RowMeasure> RowHeights<M> {
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            width_bits: f32::NAN.to_bits(),
            heights: HashMap::new(),
        }
    }

    pub fn invalidate(&mut self, key: u64) {
        self.heights.remove(&key);
    }

    pub fn clear(&mut self) {
        self.heights.clear();
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    pub fn inner_mut(&mut self) -> &mut M {
        &mut self.inner
    }
}

impl<M: RowMeasure> RowMeasure for RowHeights<M> {
    fn measure(&mut self, key: u64, width: f32) -> f32 {
        if width.to_bits() != self.width_bits {
            self.heights.clear();
            self.width_bits = width.to_bits();
        }
        let inner = &mut self.inner;
        *self
            .heights
            .entry(key)
            .or_insert_with(|| inner.measure(key, width))
    }
}
