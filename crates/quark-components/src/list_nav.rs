//! Keyboard movement shared by the list and group controls: stepping over
//! disabled items, type-ahead, and focus ids for items of a control.

use quark_ui::FocusId;

/// Focus id of item `index` of the control whose own focus id is `base`.
/// Distinct for every index and from `base`, with no string to build.
pub(crate) fn item_focus(base: FocusId, index: usize) -> FocusId {
    // Odd multiplier: distinct indices give distinct offsets.
    FocusId::new(
        base.0
            .wrapping_add((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
    )
}

/// The enabled item `delta` enabled steps from `current` among `len` items.
/// With no current item, a forward step lands on the first enabled item and
/// a backward one on the last. Without `wrap`, movement stops at the ends.
pub(crate) fn step(
    len: usize,
    current: Option<usize>,
    delta: i32,
    wrap: bool,
    disabled: impl Fn(usize) -> bool,
) -> Option<usize> {
    if len == 0 || (0..len).all(&disabled) {
        return None;
    }
    let Some(mut at) = current.filter(|&i| i < len) else {
        return if delta >= 0 {
            (0..len).find(|&i| !disabled(i))
        } else {
            (0..len).rfind(|&i| !disabled(i))
        };
    };
    let forward = delta >= 0;
    for _ in 0..delta.unsigned_abs() {
        let mut probe = at;
        let next = loop {
            probe = match (forward, wrap) {
                (true, _) if probe + 1 < len => probe + 1,
                (false, _) if probe > 0 => probe - 1,
                (true, true) => 0,
                (false, true) => len - 1,
                (_, false) => break None,
            };
            if probe == at {
                break None;
            }
            if !disabled(probe) {
                break Some(probe);
            }
        };
        match next {
            Some(next) => at = next,
            None => break,
        }
    }
    Some(at)
}

/// Type-ahead: letters typed in quick succession jump to the first item
/// whose label starts with them. Typing one letter repeatedly cycles
/// through the items starting with it.
#[derive(Debug, Default)]
pub struct TypeAhead {
    buffer: String,
    last_ms: u64,
}

impl TypeAhead {
    /// A pause longer than this starts a new search.
    pub const TIMEOUT_MS: u64 = 1000;

    /// Add `ch`, typed at `now_ms`, and find the match among `labels`
    /// (index and label of each enabled item, in order), searching from
    /// `current`.
    pub fn push<'a, I>(
        &mut self,
        ch: char,
        now_ms: u64,
        current: Option<usize>,
        labels: I,
    ) -> Option<usize>
    where
        I: Iterator<Item = (usize, &'a str)> + Clone,
    {
        if now_ms.saturating_sub(self.last_ms) > Self::TIMEOUT_MS {
            self.buffer.clear();
        }
        self.last_ms = now_ms;
        self.buffer.extend(ch.to_lowercase());
        let mut chars = self.buffer.chars();
        let first = chars.next()?;
        let repeated = chars.all(|c| c == first);
        // A repeated letter searches for that letter after the current
        // item; a longer prefix may still match the current item.
        let (prefix, skip_current) = if repeated {
            (&self.buffer[..first.len_utf8()], true)
        } else {
            (self.buffer.as_str(), false)
        };
        let starts_with = |label: &str| {
            let mut label = label.chars().flat_map(char::to_lowercase);
            prefix.chars().all(|p| label.next() == Some(p))
        };
        let after = |i: usize| match current {
            Some(c) if skip_current => i > c,
            Some(c) => i >= c,
            None => true,
        };
        labels
            .clone()
            .filter(|&(i, _)| after(i))
            .chain(labels.filter(|&(i, _)| !after(i)))
            .find(|(_, label)| starts_with(label))
            .map(|(i, _)| i)
    }
}

/// Bindings for type-ahead: letters and digits without modifiers.
pub(crate) const TYPE_AHEAD_KEYS: [&str; 36] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_skips_disabled_items_and_wraps_only_when_asked() {
        // Items 0..5 with 1 and 4 disabled.
        let disabled = |i: usize| i == 1 || i == 4;
        let cases = [
            (Some(0), 1, false, Some(2)),
            (Some(3), 1, false, Some(3)),
            (Some(3), 1, true, Some(0)),
            (Some(0), -1, true, Some(3)),
            (Some(0), -1, false, Some(0)),
            (Some(0), 2, false, Some(3)),
            (None, 1, false, Some(0)),
            (None, -1, false, Some(3)),
        ];
        for (current, delta, wrap, expected) in cases {
            assert_eq!(
                step(5, current, delta, wrap, disabled),
                expected,
                "from {current:?} by {delta} wrap={wrap}"
            );
        }
        assert_eq!(step(3, Some(0), 1, true, |_| true), None);
    }

    #[test]
    fn type_ahead_matches_prefixes_cycles_repeats_and_times_out() {
        let labels = ["Apple", "Banana", "Blueberry", "Cherry", "blackberry"];
        let all = || labels.iter().copied().enumerate();
        let mut t = TypeAhead::default();
        // "bl" narrows to Blueberry; a pause then "b" cycles from there.
        assert_eq!(t.push('b', 0, None, all()), Some(1));
        assert_eq!(t.push('l', 100, Some(1), all()), Some(2));
        assert_eq!(t.push('b', 2000, Some(2), all()), Some(4));
        // Repeating the letter wraps around to the first "b" item.
        assert_eq!(t.push('b', 2100, Some(4), all()), Some(1));
        assert_eq!(t.push('z', 5000, Some(1), all()), None);
    }
}
