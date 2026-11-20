//! [`SignalStore::verify_integrity`](super::SignalStore::verify_integrity).

use super::{Inner, NodeState};

/// An invariant of the store that does not hold. Slots are named by arena
/// index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReactiveIntegrityError {
    /// A slot on the free list holds a value or a memo, is listed twice,
    /// or is out of range.
    FreeList { index: u32 },
    /// A live slot is not accounted for: free list and live slots do not
    /// add up to the arena.
    SlotCount {
        slots: usize,
        free: usize,
        live: usize,
    },
    /// `memo` lists `source` as a source, but `source` does not list it as
    /// a subscriber, or the other way around.
    OneSidedEdge { source: u32, memo: u32 },
    /// An edge leaves or reaches a free slot, or its downstream end is not
    /// a memo.
    BadEdge { source: u32, subscriber: u32 },
    /// The same edge is recorded twice.
    DuplicateEdge { source: u32, subscriber: u32 },
    /// A raw signal is not `Clean`, has sources, or is marked computing.
    RawSignal { index: u32 },
    /// `source` may be stale but its subscriber `memo` is `Clean`: a read
    /// of the memo would return a value its sources no longer give.
    StaleSource { source: u32, memo: u32 },
    /// A slot's version is ahead of the store clock.
    VersionAhead {
        index: u32,
        version: u64,
        clock: u64,
    },
    /// A slot's held-back write and the pending list disagree, or writes
    /// are held back with no batch open.
    Pending { index: u32 },
    /// A memo is marked computing but is not on the compute stack.
    Computing { index: u32 },
}

type Result = std::result::Result<(), ReactiveIntegrityError>;

impl Inner {
    /// Every invariant, over the whole store.
    pub(super) fn verify(&self) -> Result {
        let mut free = 0;
        for (at, &index) in self.free_list.iter().enumerate() {
            let in_use = self.nodes.get(index as usize).is_none_or(|n| n.is_live());
            if in_use || self.free_list[..at].contains(&index) {
                return Err(ReactiveIntegrityError::FreeList { index });
            }
            free += 1;
        }
        let live = self.nodes.iter().filter(|n| n.is_live()).count();
        if live + free != self.nodes.len() {
            return Err(ReactiveIntegrityError::SlotCount {
                slots: self.nodes.len(),
                free,
                live,
            });
        }
        for idx in 0..self.nodes.len() {
            self.verify_node(idx)?;
        }
        if self.batch_depth == 0 && !self.pending.is_empty() {
            return Err(ReactiveIntegrityError::Pending {
                index: self.pending[0],
            });
        }
        Ok(())
    }

    /// The invariants that involve slot `idx` and its edges. O(edges of
    /// `idx` and of its neighbours).
    pub(super) fn verify_node(&self, idx: usize) -> Result {
        let index = idx as u32;
        let node = &self.nodes[idx];
        if !node.is_live() {
            let has_edges = !node.subscribers.is_empty() || !node.sources.is_empty();
            if has_edges || node.pending || node.computing {
                return Err(ReactiveIntegrityError::BadEdge {
                    source: index,
                    subscriber: index,
                });
            }
            return Ok(());
        }
        if node.version > self.clock {
            return Err(ReactiveIntegrityError::VersionAhead {
                index,
                version: node.version,
                clock: self.clock,
            });
        }
        let listed = self.pending.iter().filter(|&&p| p == index).count();
        if listed != usize::from(node.pending) || (node.pending && self.batch_depth == 0) {
            return Err(ReactiveIntegrityError::Pending { index });
        }
        // Only this way round: a memo disposed by its own compute leaves its
        // index on the stack until the compute returns, and a new signal
        // may take the slot meanwhile.
        if node.computing && !self.compute_stack.contains(&index) {
            return Err(ReactiveIntegrityError::Computing { index });
        }
        if node.memo.is_none()
            && (node.state != NodeState::Clean || !node.sources.is_empty() || node.computing)
        {
            return Err(ReactiveIntegrityError::RawSignal { index });
        }
        for (at, &sub) in node.subscribers.iter().enumerate() {
            let edge = (index, sub);
            if node.subscribers[..at].contains(&sub) {
                return Err(duplicate(edge));
            }
            let Some(memo) = self.nodes.get(sub as usize).filter(|n| n.memo.is_some()) else {
                return Err(bad(edge));
            };
            if !memo.sources.contains(&index) {
                return Err(one_sided(edge));
            }
            if node.state != NodeState::Clean && memo.state == NodeState::Clean {
                return Err(ReactiveIntegrityError::StaleSource {
                    source: index,
                    memo: sub,
                });
            }
        }
        for (at, &src) in node.sources.iter().enumerate() {
            let edge = (src, index);
            if node.sources[..at].contains(&src) {
                return Err(duplicate(edge));
            }
            let Some(source) = self.nodes.get(src as usize).filter(|n| n.is_live()) else {
                return Err(bad(edge));
            };
            if !source.subscribers.contains(&index) {
                return Err(one_sided(edge));
            }
            if source.state != NodeState::Clean && node.state == NodeState::Clean {
                return Err(ReactiveIntegrityError::StaleSource {
                    source: src,
                    memo: index,
                });
            }
        }
        Ok(())
    }
}

fn duplicate((source, subscriber): (u32, u32)) -> ReactiveIntegrityError {
    ReactiveIntegrityError::DuplicateEdge { source, subscriber }
}

fn bad((source, subscriber): (u32, u32)) -> ReactiveIntegrityError {
    ReactiveIntegrityError::BadEdge { source, subscriber }
}

fn one_sided((source, memo): (u32, u32)) -> ReactiveIntegrityError {
    ReactiveIntegrityError::OneSidedEdge { source, memo }
}
