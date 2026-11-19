//! Pushing animated values into signals, so reactive code can follow an
//! animation through the one signal graph instead of polling the table.

use super::{AnimKey, AnimationTable, IntegrityError, PropId};
use crate::color::Color;
use crate::reactive::{Signal, SignalStore};

/// Where [`AnimationTable::publish_signals`] writes a key's animated value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalBinding {
    /// The value of one prop.
    Value(PropId, Signal<f32>),
    /// The color animated at a base prop; see
    /// [`AnimationTable::animate_color`].
    Color(PropId, Signal<Color>),
}

/// Bindings in columns: binding `i` is `keys[i]` and `bindings[i]`.
#[derive(Debug, Clone, Default)]
pub(super) struct Bindings {
    keys: Vec<AnimKey>,
    bindings: Vec<SignalBinding>,
}

impl Bindings {
    fn bind(&mut self, key: AnimKey, binding: SignalBinding) {
        let same_source = |existing: &SignalBinding| match (*existing, binding) {
            (SignalBinding::Value(a, _), SignalBinding::Value(b, _))
            | (SignalBinding::Color(a, _), SignalBinding::Color(b, _)) => a == b,
            _ => false,
        };
        let found =
            (0..self.keys.len()).find(|&i| self.keys[i] == key && same_source(&self.bindings[i]));
        match found {
            Some(i) => self.bindings[i] = binding,
            None => {
                self.keys.push(key);
                self.bindings.push(binding);
            }
        }
    }

    pub(super) fn verify(&self) -> Result<(), IntegrityError> {
        if self.keys.len() != self.bindings.len() {
            return Err(IntegrityError::BindingLength {
                keys: self.keys.len(),
                bindings: self.bindings.len(),
            });
        }
        Ok(())
    }
}

impl AnimationTable {
    /// Write `(key, prop)`'s value into `signal` on every
    /// [`Self::publish_signals`]. Replaces an earlier binding of the same
    /// prop.
    pub fn bind_signal(&mut self, key: AnimKey, prop: PropId, signal: Signal<f32>) {
        self.bindings.bind(key, SignalBinding::Value(prop, signal));
        debug_assert_eq!(self.bindings.verify(), Ok(()));
    }

    /// Write the color animated at `base` for `key` into `signal` on every
    /// [`Self::publish_signals`].
    pub fn bind_color_signal(&mut self, key: AnimKey, base: PropId, signal: Signal<Color>) {
        self.bindings.bind(key, SignalBinding::Color(base, signal));
        debug_assert_eq!(self.bindings.verify(), Ok(()));
    }

    /// Drop every binding of `key`. Returns how many went.
    pub fn unbind_signals(&mut self, key: AnimKey) -> usize {
        let before = self.bindings.keys.len();
        let mut i = 0;
        while i < self.bindings.keys.len() {
            if self.bindings.keys[i] == key {
                self.bindings.keys.swap_remove(i);
                self.bindings.bindings.swap_remove(i);
            } else {
                i += 1;
            }
        }
        debug_assert_eq!(self.bindings.verify(), Ok(()));
        before - self.bindings.keys.len()
    }

    /// Write each bound row's current value into its signal, where it
    /// differs from what the signal holds, so subscribers wake only while
    /// the value moves. Call after [`Self::tick`]. Rows that do not exist
    /// and signals that were disposed are skipped. Returns how many signals
    /// were written.
    pub fn publish_signals(&self, store: &SignalStore) -> usize {
        let mut written = 0;
        for (key, binding) in self.bindings.keys.iter().zip(&self.bindings.bindings) {
            let changed = match *binding {
                SignalBinding::Value(prop, signal) => match self.get(*key, prop) {
                    Some(value) if store.is_live(signal) => store.set_if_changed(signal, value),
                    _ => false,
                },
                SignalBinding::Color(base, signal) => match self.color(*key, base) {
                    Some(color) if store.is_live(signal) => store.set_if_changed(signal, color),
                    _ => false,
                },
            };
            written += usize::from(changed);
        }
        written
    }
}
