use std::hash::Hash;

use rustc_hash::FxHashMap;
use taffy::{TaffyTree, TraversePartialTree};

use super::super::cache::{CacheKey, EVICT_AFTER_PASSES};
use super::{LayoutId, NodeMeasure, slot};

/// Spare nodes an engine keeps for new keys; nodes evicted past this leave
/// the tree.
const SPARE_NODES: usize = 256;

/// Identity of a layout node across passes: a hash of its parent's key and
/// its own key among its siblings or, unkeyed, its place among its unkeyed
/// siblings. Inserting, removing, or reordering keyed siblings leaves every
/// other node's key as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutKey(u64);

impl LayoutKey {
    /// The key of an engine's root, which top-level nodes' keys derive
    /// from.
    pub(super) const ROOT: Self = Self(0);

    /// The child keyed `key` among its siblings.
    pub(super) fn keyed(self, key: u64) -> Self {
        Self(mix(self.0 ^ KEYED, key))
    }

    /// The `n`th unkeyed child.
    pub(super) fn nth(self, n: u32) -> Self {
        Self(mix(self.0 ^ UNKEYED, u64::from(n)))
    }
}

const KEYED: u64 = 0x2545_F491_4F6C_DD1D;
const UNKEYED: u64 = 0x9E6C_63D0_676A_9A99;
const PROBE: u64 = 0xD6E8_FEB8_6659_FD93;

/// `a` and `b` mixed into one key, through splitmix64's finalizer.
fn mix(a: u64, b: u64) -> u64 {
    let mut z = a.rotate_left(32) ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The key of a [`KeyedRows`] row. A key claimed twice in one pass (two
/// siblings with one key) is probed to the next key in its sequence, so
/// each claim gets a row of its own and the same claims repeat next pass.
pub(super) trait RowKey: Copy + Eq + Hash {
    fn probe(self) -> Self;
}

impl RowKey for LayoutKey {
    fn probe(self) -> Self {
        Self(mix(self.0, PROBE))
    }
}

impl RowKey for CacheKey {
    fn probe(self) -> Self {
        Self(mix(self.0, PROBE))
    }
}

/// Rows retained across layout passes by key, stored column-wise. A pass
/// claims the rows it uses by key, and a row unclaimed for
/// [`EVICT_AFTER_PASSES`] passes is evicted. Owners keep their own columns
/// by row and remove from them as [`Self::next_pass`] says.
pub(super) struct KeyedRows<K> {
    rows: FxHashMap<K, u32>,
    key: Vec<K>,
    last_pass: Vec<u64>,
    pass: u64,
    /// Rows claimed this pass.
    claimed: usize,
}

/// What [`KeyedRows::claim`] found for a key.
pub(super) enum Claim<K> {
    /// The key's row; `continuous` when the pass before claimed it too.
    Row { row: usize, continuous: bool },
    /// No row has the key, as probed: add one with [`KeyedRows::push`].
    Vacant(K),
}

impl<K: RowKey> KeyedRows<K> {
    pub(super) fn new() -> Self {
        Self {
            rows: FxHashMap::default(),
            key: Vec::new(),
            last_pass: Vec::new(),
            pass: 0,
            claimed: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.key.len()
    }

    pub(super) fn claimed(&self) -> usize {
        self.claimed
    }

    pub(super) fn is_claimed(&self, row: usize) -> bool {
        self.last_pass[row] == self.pass
    }

    /// Claim `key`'s row for this pass.
    pub(super) fn claim(&mut self, mut key: K) -> Claim<K> {
        loop {
            let Some(&row) = self.rows.get(&key) else {
                return Claim::Vacant(key);
            };
            let row = row as usize;
            let last = self.last_pass[row];
            if last != self.pass {
                self.last_pass[row] = self.pass;
                self.claimed += 1;
                return Claim::Row {
                    row,
                    continuous: last + 1 == self.pass,
                };
            }
            key = key.probe();
        }
    }

    /// Add a row, claimed this pass, for a key [`Self::claim`] found
    /// vacant.
    pub(super) fn push(&mut self, key: K) -> usize {
        let row = self.key.len();
        self.rows.insert(key, row as u32);
        self.key.push(key);
        self.last_pass.push(self.pass);
        self.claimed += 1;
        row
    }

    /// Start the next pass, evicting the rows unclaimed for
    /// [`EVICT_AFTER_PASSES`] passes. Each goes as `Vec::swap_remove`
    /// removes it, the last row moving into its place; `evict(row)` removes
    /// it from the owner's columns the same way.
    pub(super) fn next_pass(&mut self, mut evict: impl FnMut(usize)) {
        if self.claimed < self.key.len() {
            for row in (0..self.key.len()).rev() {
                if self.last_pass[row] + EVICT_AFTER_PASSES < self.pass {
                    self.swap_remove(row);
                    evict(row);
                }
            }
        }
        self.pass += 1;
        self.claimed = 0;
    }

    fn swap_remove(&mut self, row: usize) {
        self.rows.remove(&self.key[row]);
        self.key.swap_remove(row);
        self.last_pass.swap_remove(row);
        if let Some(moved) = self.key.get(row) {
            self.rows.insert(*moved, row as u32);
        }
    }

    /// Every column has one entry per row, the key map points at them, and
    /// the claim count is the rows stamped with this pass.
    pub(super) fn verify_integrity(&self, table: &'static str) -> Result<(), LayoutIntegrityError> {
        let rows = self.key.len();
        if self.last_pass.len() != rows {
            return Err(LayoutIntegrityError::ColumnLength {
                table,
                column: "last_pass",
                len: self.last_pass.len(),
                rows,
            });
        }
        let mapped = |(row, key): (usize, &K)| self.rows.get(key) == Some(&(row as u32));
        if self.rows.len() != rows || !self.key.iter().enumerate().all(mapped) {
            return Err(LayoutIntegrityError::KeyMap { table });
        }
        let stamped = self.last_pass.iter().filter(|&&p| p == self.pass).count();
        if stamped != self.claimed || self.last_pass.iter().any(|&p| p > self.pass) {
            return Err(LayoutIntegrityError::PassAccounting { table });
        }
        Ok(())
    }
}

/// An engine's taffy nodes, each a keyed row's or a spare.
pub(super) struct NodeStore {
    rows: KeyedRows<LayoutKey>,
    /// Each row's node.
    node: Vec<LayoutId>,
    /// Nodes of evicted rows, detached, childless, and without measure
    /// contexts, which new keys take before creating nodes.
    spare: Vec<LayoutId>,
    /// The row or spare holding each node, by node slot.
    owner: Vec<Owner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Vacant,
    Row(u32),
    Spare(u32),
}

/// What [`NodeStore::claim`] found for a key.
pub(super) enum NodeClaim {
    /// The key's node, or a spare taken for it; `continuous` when the pass
    /// before laid the node out too.
    Held { node: LayoutId, continuous: bool },
    /// No node: create one and [`NodeStore::insert`] it under this key.
    New(LayoutKey),
}

impl NodeStore {
    pub(super) fn new() -> Self {
        Self {
            rows: KeyedRows::new(),
            node: Vec::new(),
            spare: Vec::new(),
            owner: Vec::new(),
        }
    }

    /// Nodes claimed this pass.
    #[cfg(test)]
    pub(super) fn claimed(&self) -> usize {
        self.rows.claimed()
    }

    /// Claim `key`'s node for this pass.
    pub(super) fn claim(&mut self, key: LayoutKey) -> NodeClaim {
        match self.rows.claim(key) {
            Claim::Row { row, continuous } => NodeClaim::Held {
                node: self.node[row],
                continuous,
            },
            Claim::Vacant(key) => match self.spare.pop() {
                Some(node) => {
                    self.insert(key, node);
                    NodeClaim::Held {
                        node,
                        continuous: false,
                    }
                }
                None => NodeClaim::New(key),
            },
        }
    }

    /// Hold `node`, created for a key [`Self::claim`] found no node for.
    pub(super) fn insert(&mut self, key: LayoutKey, node: LayoutId) {
        let row = self.rows.push(key);
        self.node.push(node);
        let slot = slot(node);
        if slot >= self.owner.len() {
            self.owner.resize(slot + 1, Owner::Vacant);
        }
        self.owner[slot] = Owner::Row(row as u32);
    }

    /// Start the next pass. Nodes unclaimed past the horizon drop their
    /// measure contexts (and the text and closures those hold) and become
    /// spares, or leave the tree once the spares are full.
    pub(super) fn next_pass(&mut self, tree: &mut TaffyTree<NodeMeasure>) {
        let Self {
            rows,
            node,
            spare,
            owner,
        } = self;
        rows.next_pass(|row| {
            let evicted = node.swap_remove(row);
            if let Some(&moved) = node.get(row) {
                owner[slot(moved)] = Owner::Row(row as u32);
            }
            if tree.get_node_context(evicted).is_some() {
                tree.set_node_context(evicted, None).expect("valid node");
            }
            if spare.len() < SPARE_NODES {
                detach(tree, evicted);
                owner[slot(evicted)] = Owner::Spare(spare.len() as u32);
                spare.push(evicted);
            } else {
                tree.remove(evicted).expect("valid node");
                owner[slot(evicted)] = Owner::Vacant;
            }
        });
    }

    /// Nodes claimed this pass.
    #[cfg(test)]
    pub(super) fn claimed_nodes(&self) -> impl Iterator<Item = LayoutId> + '_ {
        (0..self.rows.len())
            .filter(|&row| self.rows.is_claimed(row))
            .map(|row| self.node[row])
    }

    /// The rows and spares hold every node of `tree` once, and spares are
    /// detached and empty.
    pub(super) fn verify_integrity(
        &self,
        tree: &TaffyTree<NodeMeasure>,
    ) -> Result<(), LayoutIntegrityError> {
        self.rows.verify_integrity("nodes")?;
        if self.node.len() != self.rows.len() {
            return Err(LayoutIntegrityError::ColumnLength {
                table: "nodes",
                column: "node",
                len: self.node.len(),
                rows: self.rows.len(),
            });
        }
        let held_by = |node: LayoutId, owner: Owner| self.owner.get(slot(node)) == Some(&owner);
        for (row, &node) in self.node.iter().enumerate() {
            if !held_by(node, Owner::Row(row as u32)) {
                return Err(LayoutIntegrityError::NodeOwner);
            }
        }
        for (i, &node) in self.spare.iter().enumerate() {
            if !held_by(node, Owner::Spare(i as u32)) {
                return Err(LayoutIntegrityError::NodeOwner);
            }
            if tree.parent(node).is_some()
                || tree.child_count(node) > 0
                || tree.get_node_context(node).is_some()
            {
                return Err(LayoutIntegrityError::SpareInUse);
            }
        }
        // Each row and spare owns its own slot, so with as many owned slots
        // as nodes in the tree, they hold every node once.
        let nodes = self.node.len() + self.spare.len();
        let owned = self.owner.iter().filter(|&&o| o != Owner::Vacant).count();
        if owned != nodes || tree.total_node_count() != nodes {
            return Err(LayoutIntegrityError::NodeOwner);
        }
        Ok(())
    }
}

/// Detach `node` from its parent and children, and drop its cached layout.
fn detach(tree: &mut TaffyTree<NodeMeasure>, node: LayoutId) {
    if let Some(parent) = tree.parent(node) {
        tree.remove_child(parent, node).expect("valid node");
    }
    if tree.child_count(node) > 0 {
        tree.set_children(node, &[]).expect("valid node");
    }
    tree.mark_dirty(node).expect("valid node");
}

/// What [`LayoutEngine::verify_integrity`](super::LayoutEngine::verify_integrity)
/// found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutIntegrityError {
    /// A column of `table` has `len` entries for `rows` rows.
    ColumnLength {
        table: &'static str,
        column: &'static str,
        len: usize,
        rows: usize,
    },
    /// The key map of `table` and its key column disagree.
    KeyMap { table: &'static str },
    /// The rows of `table` stamped with the current pass are not the ones
    /// claimed in it.
    PassAccounting { table: &'static str },
    /// The node rows and spares do not hold every node of the tree once.
    NodeOwner,
    /// A spare node is attached, has children, or keeps a measure context.
    SpareInUse,
    /// A container's children are still being requested between passes.
    OpenScope,
}
