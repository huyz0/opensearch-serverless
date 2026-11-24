//! Reading the shell's snapshot repositories and snapshots.
//!
//! Read-only (ADR 0039): nothing here writes a blob. A repository is the
//! record `repositories/<name>`; a snapshot name's state is the register
//! `snapshot-states/<sha256 of "<repo>#<name>">`, with the legacy record
//! `snapshots/<repo>#<name>` read only when the register is absent.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange, MAX_LIST_KEYS};
use oss_format::{
    FormatError, MAX_REPOSITORY_RECORD_BYTES, RepositoryRecord, SnapshotState, SnapshotSummary,
    repository_key, snapshot_legacy_key, snapshot_state_key,
};

/// The most repositories a listing reads: its fan-out is one GET each.
pub const MAX_REPOSITORIES: usize = 256;

/// The scratch a blob read streams through.
const SCRATCH: usize = 16 << 10;

/// Why a repository or snapshot could not be read.
#[derive(Debug)]
pub enum SnapshotReadError {
    /// A record or register value is larger than the buffer given.
    TooLarge,
    /// A record that cannot be read, or whose key is not the one asked for.
    Unreadable(FormatError),
    /// A record named another snapshot than the one its key stands for.
    KeyMismatch,
    /// More repositories than [`MAX_REPOSITORIES`].
    TooManyRepositories,
    /// The store failed.
    Blob(BlobError),
}

impl std::fmt::Display for SnapshotReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("a record larger than the read limit"),
            Self::Unreadable(e) => write!(f, "an unreadable record: {e}"),
            Self::KeyMismatch => f.write_str("a record that does not match its key"),
            Self::TooManyRepositories => write!(f, "more than {MAX_REPOSITORIES} repositories"),
            Self::Blob(e) => write!(f, "the store: {e}"),
        }
    }
}

impl std::error::Error for SnapshotReadError {}

impl From<BlobError> for SnapshotReadError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for SnapshotReadError {
    fn from(e: FormatError) -> Self {
        Self::Unreadable(e)
    }
}

/// A snapshot as the store holds it.
#[derive(Debug, PartialEq, Eq)]
pub struct SnapshotRead {
    /// What the record says.
    pub summary: SnapshotSummary,
}

/// Reads repositories and snapshots from the deployment's store.
pub struct SnapshotReader {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl SnapshotReader {
    /// A reader over `store` under `base`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: one name, once a node.
            base: base.to_owned(),
        }
    }

    /// The plain blob at `key` read into the front of `buf`: its length, or
    /// `None` when there is none. Cost: one GET.
    pub(crate) fn blob(
        &self,
        key: &str,
        buf: &mut [u8],
    ) -> Result<Option<usize>, SnapshotReadError> {
        let mut scratch = [0u8; SCRATCH];
        let mut len = 0usize;
        let mut too_large = false;
        let got = self.store.get(
            key,
            ByteRange::Full,
            &mut scratch,
            &mut |chunk: &[u8]| -> Result<(), BlobError> {
                let end = len.saturating_add(chunk.len());
                match buf.get_mut(len..end) {
                    Some(dest) => dest.copy_from_slice(chunk),
                    None => {
                        too_large = true;
                        return Err(BlobError::InvalidArgument(
                            "a record larger than the buffer",
                        ));
                    }
                }
                len = end;
                Ok(())
            },
        );
        match got {
            Ok(_) => Ok(Some(len)),
            Err(BlobError::NotFound) => Ok(None),
            Err(_) if too_large => Err(SnapshotReadError::TooLarge),
            Err(e) => Err(e.into()),
        }
    }

    /// One repository's record; `None` when it is not registered. Cost: one
    /// GET.
    ///
    /// # Errors
    ///
    /// [`SnapshotReadError`].
    pub fn repository(
        &self,
        name: &str,
        buf: &mut [u8],
    ) -> Result<Option<RepositoryRecord>, SnapshotReadError> {
        let key = repository_key(&self.base, name)?;
        let limit = buf.len().min(MAX_REPOSITORY_RECORD_BYTES);
        let Some(front) = buf.get_mut(..limit) else {
            return Err(SnapshotReadError::TooLarge);
        };
        let Some(len) = self.blob(&key, front)? else {
            return Ok(None);
        };
        let bytes = front.get(..len).unwrap_or_default();
        Ok(Some(RepositoryRecord::parse(bytes)?))
    }

    /// Every readable repository, in key order; one that cannot be read is
    /// omitted, as the Java omits it. Cost: one LIST per 1,000 keys and one
    /// GET a repository.
    ///
    /// # Errors
    ///
    /// [`SnapshotReadError::TooManyRepositories`] once more than
    /// [`MAX_REPOSITORIES`] keys are seen (the listing stops there), and
    /// the store's failures.
    pub fn repositories(&self, buf: &mut [u8]) -> Result<Vec<RepositoryRecord>, SnapshotReadError> {
        let prefix = repository_key(&self.base, "")?;
        // ALLOC: one key a repository, at most MAX_REPOSITORIES (checked).
        let mut keys: Vec<String> = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut last = None;
            // LIST: the repositories container, as the Java's listing; one page per
            // 1,000 keys, a repository being operator-registered and human-scale,
            // asked for by a caller and never by a timer.
            let page = self.store.list_prefix(
                &prefix,
                after.as_deref(),
                MAX_LIST_KEYS,
                &mut |key, _| {
                    // ALLOC: one key, as above.
                    keys.push(key.to_owned());
                    last = Some(key.to_owned());
                },
            )?;
            if keys.len() > MAX_REPOSITORIES {
                return Err(SnapshotReadError::TooManyRepositories);
            }
            if !page.truncated {
                break;
            }
            after = last;
        }
        let limit = buf.len().min(MAX_REPOSITORY_RECORD_BYTES);
        let Some(front) = buf.get_mut(..limit) else {
            return Err(SnapshotReadError::TooLarge);
        };
        // ALLOC: one record a repository, at most MAX_REPOSITORIES.
        let mut found = Vec::new();
        for key in &keys {
            match self.blob(key, front) {
                Ok(Some(len)) => {
                    let bytes = front.get(..len).unwrap_or_default();
                    if let Ok(record) = RepositoryRecord::parse(bytes) {
                        found.push(record);
                    }
                }
                // Gone since the listing, or not readable: omitted.
                Ok(None) | Err(SnapshotReadError::TooLarge) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(found)
    }

    /// One snapshot; `None` when the name holds none. Cost: one GET when the
    /// name's register holds or released it, two when it is absent (the
    /// register, then the legacy record). A released register is no snapshot
    /// and the legacy record is not consulted: the Java releases the register
    /// before it removes the record. Nothing is cached.
    ///
    /// # Errors
    ///
    /// [`SnapshotReadError`].
    pub fn snapshot(
        &self,
        repo: &str,
        name: &str,
        buf: &mut [u8],
    ) -> Result<Option<SnapshotRead>, SnapshotReadError> {
        let state_key = snapshot_state_key(&self.base, repo, name)?;
        match self.store.read_register(&state_key, buf) {
            Ok(Some(read)) => {
                let value = buf.get(..read.len).unwrap_or_default();
                return match SnapshotState::parse(value)? {
                    SnapshotState::Released => Ok(None),
                    SnapshotState::Held { record, .. } => Self::of(repo, name, record).map(Some),
                };
            }
            Ok(None) => {}
            Err(BlobError::InvalidArgument(_)) => return Err(SnapshotReadError::TooLarge),
            Err(e) => return Err(e.into()),
        }
        let legacy = snapshot_legacy_key(&self.base, repo, name)?;
        match self.blob(&legacy, buf) {
            Ok(Some(len)) => {
                let record = buf.get(..len).unwrap_or_default();
                Self::of(repo, name, record).map(Some)
            }
            Ok(None) => Ok(None),
            // A key an fs store cannot name is no snapshot (a deliberate
            // deviation: the Java lists to find out).
            Err(SnapshotReadError::Blob(BlobError::InvalidArgument(_))) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn of(repo: &str, name: &str, record: &[u8]) -> Result<SnapshotRead, SnapshotReadError> {
        let summary = SnapshotSummary::parse(record)?;
        if summary.repo != repo || summary.name != name {
            return Err(SnapshotReadError::KeyMismatch);
        }
        Ok(SnapshotRead { summary })
    }
}
