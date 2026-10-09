//! Versioned files of a diff that changes while it is shown: stable file
//! identities, monotonic revisions, and the rules that keep a late update
//! from undoing a newer one.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::model::{DiffDocument, IntegrityError};
use crate::remap::SourceRemap;
use crate::source::{FileSources, HydrationError, SourceCoverage};

/// A file's identity, assigned by the app and independent of its path and
/// arrival order. A rename keeps it; a file deleted and created again gets
/// a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u64);

/// A content revision. Revisions of one file, and of the session's order,
/// only increase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(pub u64);

/// One revision of one file's diff.
#[derive(Debug, Clone)]
pub struct FileDiffSnapshot {
    pub id: FileId,
    pub revision: Revision,
    /// A valid one-file document; text stores are shared, not copied.
    pub diff: Arc<DiffDocument>,
}

impl FileDiffSnapshot {
    /// Checks that `diff` holds exactly one file and is intact.
    pub fn new(
        id: FileId,
        revision: Revision,
        diff: Arc<DiffDocument>,
    ) -> Result<Self, UpdateError> {
        if diff.file_count() != 1 {
            return Err(UpdateError::NotOneFile {
                file: id,
                files: diff.file_count(),
            });
        }
        diff.verify_integrity()
            .map_err(|error| UpdateError::Integrity { file: id, error })?;
        Ok(Self { id, revision, diff })
    }

    pub fn coverage(&self) -> SourceCoverage {
        self.diff.coverage(0)
    }
}

/// Why sources are wanted, so the app can prioritize loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourcePurpose {
    ExpandContext,
    CompleteSyntax,
}

/// A request for the exact sources of one file revision. The app reads
/// them from that revision's old and new commits, not from whatever file
/// has the same path now, and answers with [`DiffUpdate::Hydrate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRequest {
    pub file: FileId,
    pub revision: Revision,
    pub purpose: SourcePurpose,
}

/// A change to a [`DiffSession`].
#[derive(Debug, Clone)]
pub enum DiffUpdate {
    /// Adds a file at the end of the order, or replaces an existing file
    /// with a newer revision. `remap` maps the previous revision's source
    /// lines to this one's, for keeping selection and scroll anchors.
    Upsert {
        file: FileDiffSnapshot,
        remap: Option<SourceRemap>,
    },
    /// Removes a file for good: later upserts of the id are refused.
    Remove { file: FileId, revision: Revision },
    /// Reorders files. Listed files come first in the given order; live
    /// files it omits keep their relative order after them; removed ids
    /// are skipped.
    Order {
        revision: Revision,
        files: Arc<[FileId]>,
    },
    /// Supplies the whole sources of a patch-only file revision. Keeps the
    /// revision: the diff is the same, only its coverage grows.
    Hydrate {
        file: FileId,
        revision: Revision,
        sources: FileSources,
    },
}

/// What an applied [`DiffUpdate`] changed. Indices are positions in
/// [`DiffSession::order`] after the update.
#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    Inserted {
        index: u32,
    },
    Replaced {
        index: u32,
        previous: Revision,
        remap: Option<SourceRemap>,
    },
    /// `index` is where the file was, or `None` when it was never seen.
    Removed {
        index: Option<u32>,
    },
    Reordered,
    Hydrated {
        index: u32,
    },
}

/// Why a [`DiffUpdate`] was refused. A refused update leaves the session
/// as it was; [`UpdateError::Stale`] and [`UpdateError::Removed`] are the
/// normal fate of updates that arrive late and can be dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// The revision is not newer than the one the session holds.
    Stale {
        file: Option<FileId>,
        current: Revision,
        received: Revision,
    },
    /// The file was removed at `revision`; its id cannot come back.
    Removed {
        file: FileId,
        revision: Revision,
    },
    UnknownFile {
        file: FileId,
    },
    DuplicateInOrder {
        file: FileId,
    },
    NotOneFile {
        file: FileId,
        files: u32,
    },
    Integrity {
        file: FileId,
        error: IntegrityError,
    },
    /// The remap is not from the held revision to the new one, or its
    /// line counts do not fit the snapshots.
    RemapMismatch {
        file: FileId,
    },
    Hydration {
        file: FileId,
        error: HydrationError,
    },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "diff update: {self:?}")
    }
}

impl std::error::Error for UpdateError {}

#[derive(Debug, Clone)]
enum Record {
    Live(FileDiffSnapshot),
    /// Removed at this revision.
    Tombstone(Revision),
}

/// The files of a changing diff, in display order. Lookups by id are
/// O(1); an update touches one file's record, plus O(files) of order
/// bookkeeping when files are inserted, removed, or reordered.
#[derive(Debug, Clone, Default)]
pub struct DiffSession {
    records: HashMap<FileId, Record>,
    order: Vec<FileId>,
    /// Position of each live file in `order`.
    index: HashMap<FileId, u32>,
    order_revision: Revision,
}

impl DiffSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Live files in display order.
    pub fn order(&self) -> &[FileId] {
        &self.order
    }

    pub fn order_revision(&self) -> Revision {
        self.order_revision
    }

    pub fn len(&self) -> u32 {
        self.order.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn file(&self, id: FileId) -> Option<&FileDiffSnapshot> {
        match self.records.get(&id)? {
            Record::Live(snapshot) => Some(snapshot),
            Record::Tombstone(_) => None,
        }
    }

    pub fn index_of(&self, id: FileId) -> Option<u32> {
        self.index.get(&id).copied()
    }

    /// The file at `index` of the order.
    pub fn at(&self, index: u32) -> Option<&FileDiffSnapshot> {
        self.file(*self.order.get(index as usize)?)
    }

    /// Whether `id` was removed.
    pub fn is_removed(&self, id: FileId) -> bool {
        matches!(self.records.get(&id), Some(Record::Tombstone(_)))
    }

    /// The request that would complete file `id`'s sources, or `None` when
    /// it is unknown, binary, or already whole.
    pub fn source_request(&self, id: FileId, purpose: SourcePurpose) -> Option<SourceRequest> {
        let snapshot = self.file(id)?;
        let binary = snapshot.diff.files().meta[0].binary;
        (snapshot.coverage() == SourceCoverage::PatchOnly && !binary).then_some(SourceRequest {
            file: id,
            revision: snapshot.revision,
            purpose,
        })
    }

    pub fn apply(&mut self, update: DiffUpdate) -> Result<UpdateOutcome, UpdateError> {
        let outcome = match update {
            DiffUpdate::Upsert { file, remap } => self.upsert(file, remap),
            DiffUpdate::Remove { file, revision } => self.remove(file, revision),
            DiffUpdate::Order { revision, files } => self.reorder(revision, &files),
            DiffUpdate::Hydrate {
                file,
                revision,
                sources,
            } => self.hydrate(file, revision, sources),
        };
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        outcome
    }

    fn upsert(
        &mut self,
        file: FileDiffSnapshot,
        remap: Option<SourceRemap>,
    ) -> Result<UpdateOutcome, UpdateError> {
        let id = file.id;
        match self.records.get(&id) {
            None => {
                let index = self.order.len() as u32;
                self.order.push(id);
                self.index.insert(id, index);
                self.records.insert(id, Record::Live(file));
                Ok(UpdateOutcome::Inserted { index })
            }
            Some(&Record::Tombstone(revision)) => Err(UpdateError::Removed { file: id, revision }),
            Some(Record::Live(current)) => {
                if file.revision <= current.revision {
                    return Err(UpdateError::Stale {
                        file: Some(id),
                        current: current.revision,
                        received: file.revision,
                    });
                }
                if let Some(remap) = &remap
                    && !remap.fits(current, &file)
                {
                    return Err(UpdateError::RemapMismatch { file: id });
                }
                let previous = current.revision;
                self.records.insert(id, Record::Live(file));
                Ok(UpdateOutcome::Replaced {
                    index: self.index[&id],
                    previous,
                    remap,
                })
            }
        }
    }

    fn remove(&mut self, id: FileId, revision: Revision) -> Result<UpdateOutcome, UpdateError> {
        let current = match self.records.get(&id) {
            Some(Record::Live(snapshot)) => Some(snapshot.revision),
            Some(&Record::Tombstone(revision)) => {
                return Err(UpdateError::Removed { file: id, revision });
            }
            None => None,
        };
        if let Some(current) = current
            && revision <= current
        {
            return Err(UpdateError::Stale {
                file: Some(id),
                current,
                received: revision,
            });
        }
        self.records.insert(id, Record::Tombstone(revision));
        let index = self.index.remove(&id);
        if let Some(at) = index {
            self.order.remove(at as usize);
            self.reindex(at as usize);
        }
        Ok(UpdateOutcome::Removed { index })
    }

    fn reorder(
        &mut self,
        revision: Revision,
        files: &[FileId],
    ) -> Result<UpdateOutcome, UpdateError> {
        if revision <= self.order_revision {
            return Err(UpdateError::Stale {
                file: None,
                current: self.order_revision,
                received: revision,
            });
        }
        let mut listed = HashMap::with_capacity(files.len());
        for &id in files {
            match self.records.get(&id) {
                None => return Err(UpdateError::UnknownFile { file: id }),
                Some(Record::Tombstone(_)) => continue,
                Some(Record::Live(_)) => {}
            }
            if listed.insert(id, ()).is_some() {
                return Err(UpdateError::DuplicateInOrder { file: id });
            }
        }
        let mut order: Vec<FileId> = files
            .iter()
            .copied()
            .filter(|id| listed.contains_key(id))
            .collect();
        order.extend(self.order.iter().filter(|id| !listed.contains_key(id)));
        self.order = order;
        self.order_revision = revision;
        self.reindex(0);
        Ok(UpdateOutcome::Reordered)
    }

    fn hydrate(
        &mut self,
        id: FileId,
        revision: Revision,
        sources: FileSources,
    ) -> Result<UpdateOutcome, UpdateError> {
        let current = match self.records.get(&id) {
            Some(Record::Live(snapshot)) => snapshot,
            Some(&Record::Tombstone(revision)) => {
                return Err(UpdateError::Removed { file: id, revision });
            }
            None => return Err(UpdateError::UnknownFile { file: id }),
        };
        if revision != current.revision {
            return Err(UpdateError::Stale {
                file: Some(id),
                current: current.revision,
                received: revision,
            });
        }
        let diff = current
            .diff
            .hydrate_file(0, sources)
            .map_err(|error| UpdateError::Hydration { file: id, error })?;
        let snapshot = FileDiffSnapshot {
            id,
            revision,
            diff: Arc::new(diff),
        };
        self.records.insert(id, Record::Live(snapshot));
        Ok(UpdateOutcome::Hydrated {
            index: self.index[&id],
        })
    }

    fn reindex(&mut self, from: usize) {
        for (i, id) in self.order.iter().enumerate().skip(from) {
            self.index.insert(*id, i as u32);
        }
    }

    /// Checks that the order lists every live file once, the index agrees
    /// with it, and every snapshot holds one file under its own id.
    /// O(files).
    pub fn verify_integrity(&self) -> Result<(), SessionIntegrityError> {
        let live = self
            .records
            .values()
            .filter(|r| matches!(r, Record::Live(_)))
            .count();
        if live != self.order.len() || self.index.len() != self.order.len() {
            return Err(SessionIntegrityError::OrderCount);
        }
        for (i, id) in self.order.iter().enumerate() {
            let Some(Record::Live(snapshot)) = self.records.get(id) else {
                return Err(SessionIntegrityError::NotLive(*id));
            };
            if self.index.get(id) != Some(&(i as u32)) {
                return Err(SessionIntegrityError::Index(*id));
            }
            if snapshot.id != *id || snapshot.diff.file_count() != 1 {
                return Err(SessionIntegrityError::Snapshot(*id));
            }
        }
        Ok(())
    }
}

/// A broken [`DiffSession`] invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIntegrityError {
    /// The order and the live records disagree in number.
    OrderCount,
    NotLive(FileId),
    Index(FileId),
    Snapshot(FileId),
}
