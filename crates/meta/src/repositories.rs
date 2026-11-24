//! Registering and removing a snapshot repository, as the shell's
//! `RepositoryStore` does.
//!
//! A repository is one blob, `repositories/<name>`. Registering writes it
//! (an upsert, no read first); removing it first checks that no readable,
//! held snapshot names it, which is the shell's `SnapshotStore.list` over the
//! two discovery-marker prefixes.

use std::collections::HashSet;
use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, MAX_LIST_KEYS, SliceSource};
use oss_format::{
    FormatError, RepositoryRecord, SnapshotState, SnapshotSummary, looks_like_an_id,
    repository_key, sha256_hex, snapshot_legacy_key, snapshot_marker_prefixes,
    snapshot_state_key_of_hash,
};

use crate::snapshots::{SnapshotReadError, SnapshotReader};

/// The most discovery markers one repository delete will look at.
pub const MAX_SNAPSHOT_MARKERS: usize = 1_000;

const SCRATCH: usize = 4 << 10;

/// Why a repository write or delete failed.
#[derive(Debug)]
pub enum RepositoryWriteError {
    /// The record did not fit the buffer, or a name made no key.
    Format(FormatError),
    /// The store failed.
    Blob(BlobError),
    /// A snapshot record could not be read into the buffer.
    Snapshot(SnapshotReadError),
    /// More than [`MAX_SNAPSHOT_MARKERS`] markers name the repository.
    TooManyMarkers,
}

impl std::fmt::Display for RepositoryWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(e) => write!(f, "{e}"),
            Self::Blob(e) => write!(f, "{e}"),
            Self::Snapshot(e) => write!(f, "{e}"),
            Self::TooManyMarkers => write!(
                f,
                "more than {MAX_SNAPSHOT_MARKERS} snapshot markers name the repository"
            ),
        }
    }
}

impl std::error::Error for RepositoryWriteError {}

impl From<FormatError> for RepositoryWriteError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

impl From<BlobError> for RepositoryWriteError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<SnapshotReadError> for RepositoryWriteError {
    fn from(e: SnapshotReadError) -> Self {
        match e {
            SnapshotReadError::Blob(b) => Self::Blob(b),
            other => Self::Snapshot(other),
        }
    }
}

/// What a repository delete did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// There was no such repository.
    Missing,
    /// A readable, held snapshot still names it.
    InUse,
    /// The record is gone.
    Removed,
}

/// Writes and removes repository records.
pub struct RepositoryWriter {
    store: Arc<dyn BlobStore>,
    reader: SnapshotReader,
    base: String,
}

impl RepositoryWriter {
    /// A writer over `store` under `base`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            reader: SnapshotReader::new(Arc::clone(&store), base),
            store,
            // ALLOC: one name, once a node.
            base: base.to_owned(),
        }
    }

    /// Registers `record`, replacing any record of that name, through `buf`
    /// (the record is written into it). Cost: one PUT.
    ///
    /// # Errors
    ///
    /// [`RepositoryWriteError`].
    pub fn put(
        &self,
        record: &RepositoryRecord,
        buf: &mut [u8],
    ) -> Result<(), RepositoryWriteError> {
        let key = repository_key(&self.base, &record.name)?;
        let len = record.write(buf)?;
        let body = buf.get(..len).unwrap_or_default();
        let mut scratch = [0u8; SCRATCH];
        self.store
            .put(&key, &mut SliceSource::new(body), &mut scratch)?;
        Ok(())
    }

    /// Whether `name` is registered. Cost: one HEAD.
    ///
    /// # Errors
    ///
    /// [`RepositoryWriteError`].
    pub fn exists(&self, name: &str) -> Result<bool, RepositoryWriteError> {
        let key = repository_key(&self.base, name)?;
        Ok(self.store.head(&key)?.is_some())
    }

    /// Removes the repository `name` unless a snapshot still names it.
    /// Cost: one HEAD, two LISTs (one more a 1,000 markers), a read a marker
    /// and, when none holds, one DELETE.
    ///
    /// # Errors
    ///
    /// [`RepositoryWriteError`].
    pub fn remove(&self, name: &str, buf: &mut [u8]) -> Result<Removal, RepositoryWriteError> {
        if !self.exists(name)? {
            return Ok(Removal::Missing);
        }
        if self.holds_snapshots(name, buf)? {
            return Ok(Removal::InUse);
        }
        self.store.delete(&repository_key(&self.base, name)?)?;
        Ok(Removal::Removed)
    }

    /// Whether any readable, held snapshot names `repo`: the markers under
    /// both prefixes, each read until one is held. An unreadable, released
    /// or mismatched one is not held, as in the shell.
    fn holds_snapshots(&self, repo: &str, buf: &mut [u8]) -> Result<bool, RepositoryWriteError> {
        let container = [self.base.as_str(), "snapshots/"].concat();
        let mut seen: HashSet<String> = HashSet::new();
        for prefix in snapshot_marker_prefixes(&self.base, repo)? {
            let mut after: Option<String> = None;
            loop {
                // ALLOC: one key a marker, at most MAX_LIST_KEYS a page.
                let mut page_keys: Vec<String> = Vec::new();
                // LIST: a repository's snapshot markers, the shell's snapshot
                // listing; an operator's delete, never a timer, bounded at
                // MAX_SNAPSHOT_MARKERS.
                let page = self.store.list_prefix(
                    &prefix,
                    after.as_deref(),
                    MAX_LIST_KEYS,
                    // ALLOC: one key a marker, at most MAX_LIST_KEYS a page.
                    &mut |key, _| page_keys.push(key.to_owned()),
                )?;
                for key in &page_keys {
                    let Some(marker) = key.strip_prefix(container.as_str()) else {
                        continue;
                    };
                    // ALLOC: one marker's parts or key, bounded by the key.
                    if !seen.insert(marker.to_owned()) {
                        continue;
                    }
                    if seen.len() > MAX_SNAPSHOT_MARKERS {
                        return Err(RepositoryWriteError::TooManyMarkers);
                    }
                    if self.marker_holds(repo, marker, buf)? {
                        return Ok(true);
                    }
                }
                if !page.truncated {
                    break;
                }
                after = page_keys.last().cloned();
            }
        }
        Ok(false)
    }

    /// Whether the snapshot a marker names is held and belongs to `repo`.
    /// Anything unreadable is not held; a failing store is an error.
    fn marker_holds(
        &self,
        repo: &str,
        marker: &str,
        buf: &mut [u8],
    ) -> Result<bool, RepositoryWriteError> {
        // ALLOC: one marker's parts or key, bounded by the key.
        let parts: Vec<&str> = marker.split('#').collect();
        let current = match parts.as_slice() {
            [v2, key_hash, id] => {
                let hash = |s: &str| {
                    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                };
                v2.len() == 67
                    && v2.starts_with("v2-")
                    && v2.get(3..).is_some_and(hash)
                    && hash(key_hash)
                    && looks_like_an_id(id)
            }
            _ => false,
        };
        let key_hash = match (current, parts.as_slice()) {
            // ALLOC: one marker's parts or key, bounded by the key.
            (true, [_, hash, _]) => (*hash).to_owned(),
            _ => sha256_hex(marker),
        };
        let state_key = snapshot_state_key_of_hash(&self.base, &key_hash)?;
        match self.store.read_register(&state_key, buf) {
            Ok(Some(read)) => {
                let value = buf.get(..read.len).unwrap_or_default();
                let Ok(SnapshotState::Held {
                    marker: held,
                    record,
                    ..
                }) = SnapshotState::parse(value)
                else {
                    return Ok(false);
                };
                let Ok(summary) = SnapshotSummary::parse(record) else {
                    return Ok(false);
                };
                let key = [summary.repo.as_str(), "#", summary.name.as_str()].concat();
                Ok(held == marker && summary.repo == repo && sha256_hex(&key) == key_hash)
            }
            Ok(None) if current => Ok(false),
            Ok(None) => self.legacy_holds(repo, marker, buf),
            Err(BlobError::InvalidArgument(_)) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// A legacy marker with no register: the record at the marker, which the
    /// shell would adopt into a register, names `repo` and is its own key.
    fn legacy_holds(
        &self,
        repo: &str,
        marker: &str,
        buf: &mut [u8],
    ) -> Result<bool, RepositoryWriteError> {
        let Some((marker_repo, name)) = marker.split_once('#') else {
            return Ok(false);
        };
        let key = snapshot_legacy_key(&self.base, marker_repo, name)?;
        let len = match self.reader.blob(&key, buf) {
            Ok(Some(len)) => len,
            Ok(None) | Err(SnapshotReadError::TooLarge) => return Ok(false),
            Err(SnapshotReadError::Blob(BlobError::InvalidArgument(_))) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let Ok(summary) = SnapshotSummary::parse(buf.get(..len).unwrap_or_default()) else {
            return Ok(false);
        };
        Ok(summary.repo == repo && summary.repo == marker_repo && summary.name == name)
    }
}
