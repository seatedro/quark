//! Glyph entries and the LRU lists that order them for eviction.

use super::{ContentType, GlyphAtlas, GlyphKey, Placed};

/// End of an LRU list.
const NIL: u32 = u32::MAX;
/// Blank and failed glyphs kept (about 4 MiB of metadata with resident
/// entries); the least recently used go first.
const MAX_METADATA_ENTRIES: usize = 65_536;

#[derive(Debug, Clone, Copy)]
pub(super) enum State {
    Free,
    /// Nothing to draw.
    Blank,
    /// The rasterizer failed; tried again from frame `retry` (never for a
    /// permanent failure).
    Failed {
        retry: u64,
    },
    Resident(Placed),
}

impl State {
    /// The list an entry in this state belongs on; `None` when free.
    fn list(&self) -> Option<Lru> {
        match self {
            State::Free => None,
            State::Resident(placed) => Some(Lru::of(placed.kind)),
            State::Blank | State::Failed { .. } => Some(Lru::Meta),
        }
    }
}

#[derive(Debug)]
pub(super) struct Entry {
    pub(super) key: GlyphKey,
    pub(super) generation: u32,
    pub(super) state: State,
    /// The frame that last used it; the current frame pins it.
    pub(super) used: u64,
    prev: u32,
    next: u32,
    /// The list it is on, `None` while free.
    list: Option<Lru>,
}

/// The LRU lists: resident masks, resident color glyphs, and metadata
/// (blank and failed glyphs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lru {
    Mask,
    Color,
    Meta,
}

impl Lru {
    pub(super) const COUNT: usize = 3;
    const ALL: [Lru; Self::COUNT] = [Lru::Mask, Lru::Color, Lru::Meta];

    /// The list of resident glyphs of `kind`.
    pub(super) fn of(kind: ContentType) -> Self {
        match kind {
            ContentType::Mask => Lru::Mask,
            ContentType::Color => Lru::Color,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One LRU list: most recently used at the head.
#[derive(Debug, Clone, Copy)]
pub(super) struct LruList {
    head: u32,
    tail: u32,
    len: usize,
}

impl LruList {
    pub(super) const EMPTY: Self = Self {
        head: NIL,
        tail: NIL,
        len: 0,
    };
}

impl GlyphAtlas {
    fn list(&self, list: Lru) -> &LruList {
        &self.lists[list.index()]
    }

    fn list_mut(&mut self, list: Lru) -> &mut LruList {
        &mut self.lists[list.index()]
    }

    /// The least recently used entry of `list`, if any.
    pub(super) fn least_recent(&self, list: Lru) -> Option<u32> {
        let tail = self.list(list).tail;
        (tail != NIL).then_some(tail)
    }

    /// Add an entry for `key` in `state`, used this frame, and return its
    /// index. Past the metadata limit, the least recently used blank or
    /// failed glyph goes first.
    pub(super) fn insert(&mut self, key: GlyphKey, state: State) -> u32 {
        let list = state.list().expect("a new entry is not free");
        if list == Lru::Meta
            && self.list(Lru::Meta).len >= MAX_METADATA_ENTRIES
            && let Some(tail) = self.least_recent(Lru::Meta)
            && self.entries[tail as usize].used != self.frame
        {
            self.remove(tail);
        }
        let index = match self.free_entries.pop() {
            Some(index) => {
                let entry = &mut self.entries[index as usize];
                entry.key = key;
                entry.state = state;
                index
            }
            None => {
                self.entries.push(Entry {
                    key,
                    generation: 0,
                    state,
                    used: 0,
                    prev: NIL,
                    next: NIL,
                    list: None,
                });
                self.entries.len() as u32 - 1
            }
        };
        self.map.insert(key, index);
        self.link(index, list);
        self.entries[index as usize].used = self.frame;
        index
    }

    /// Drops entry `index`: frees its rectangle, stales its handles.
    pub(super) fn remove(&mut self, index: u32) {
        self.unlink(index);
        let entry = &mut self.entries[index as usize];
        let state = std::mem::replace(&mut entry.state, State::Free);
        entry.generation = entry.generation.wrapping_add(1);
        let key = entry.key;
        self.map.remove(&key);
        self.free_entries.push(index);
        self.stats.evictions += 1;
        if let State::Resident(placed) = state {
            self.free_rectangle(&placed);
        }
    }

    /// Mark entry `index` used this frame, moving it to the front of its
    /// list.
    pub(super) fn touch(&mut self, index: u32) {
        let entry = &mut self.entries[index as usize];
        entry.used = self.frame;
        let Some(list) = entry.list else {
            return;
        };
        if self.list(list).head != index {
            self.unlink(index);
            self.link(index, list);
        }
    }

    fn link(&mut self, index: u32, list: Lru) {
        let head = self.list(list).head;
        let entry = &mut self.entries[index as usize];
        entry.prev = NIL;
        entry.next = head;
        entry.list = Some(list);
        if head != NIL {
            self.entries[head as usize].prev = index;
        } else {
            self.list_mut(list).tail = index;
        }
        let lru = self.list_mut(list);
        lru.head = index;
        lru.len += 1;
    }

    fn unlink(&mut self, index: u32) {
        let entry = &mut self.entries[index as usize];
        let Some(list) = entry.list.take() else {
            return;
        };
        let (prev, next) = (entry.prev, entry.next);
        entry.prev = NIL;
        entry.next = NIL;
        if prev != NIL {
            self.entries[prev as usize].next = next;
        } else {
            self.list_mut(list).head = next;
        }
        if next != NIL {
            self.entries[next as usize].prev = prev;
        } else {
            self.list_mut(list).tail = prev;
        }
        self.list_mut(list).len -= 1;
    }

    /// Checks every LRU list against the entries on it.
    pub(super) fn verify_lists(&self) -> Result<(), String> {
        for list in Lru::ALL {
            let (mut at, mut prev, mut len) = (self.list(list).head, NIL, 0);
            while at != NIL {
                let entry = &self.entries[at as usize];
                if entry.list != Some(list) || entry.prev != prev {
                    return Err(format!("list {list:?} broken at entry {at}"));
                }
                if entry.state.list() != Some(list) {
                    return Err(format!(
                        "entry {at} in list {list:?}, belongs in {:?}",
                        entry.state.list()
                    ));
                }
                (prev, at, len) = (at, entry.next, len + 1);
            }
            if self.list(list).tail != prev || self.list(list).len != len {
                return Err(format!("list {list:?} tail or length wrong"));
            }
        }
        Ok(())
    }

    /// Whether entry `index` is on no list, as a free entry must be.
    pub(super) fn unlisted(&self, index: usize) -> bool {
        self.entries[index].list.is_none()
    }
}
