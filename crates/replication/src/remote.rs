//! The read-only handle on a leader's store.
use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ReadOnlyStore};
use oss_format::{CommitManifest, FormatError, NameRecord, ShardRef, descriptor_key};

/// Why a read of a leader failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// The leader has no such index or shard.
    Missing,
    /// What is there is not a format this reader knows.
    Format(String),
    /// The store failed.
    Store(String),
}

/// A leader shard's published commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardManifest {
    /// The register's generation.
    pub generation: u64,
    /// The publishing writer's term.
    pub term: u64,
    /// The files it names, in name order.
    pub files: Vec<String>,
}

/// A leader index's descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderIndex {
    /// The uuid.
    pub uuid: String,
    /// The shard count.
    pub number_of_shards: u32,
    /// The mapping source.
    pub mapping: Option<String>,
}

/// A leader's store, readable and nothing else.
///
/// The handle holds a [`ReadOnlyStore`] and exposes no method that writes,
/// deletes or lists, so a write to a leader does not compile:
///
/// ```compile_fail
/// # use std::sync::Arc;
/// # use oss_blob::{BlobStore, Emulator};
/// let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
/// let remote = oss_replication::RemoteStore::new(store, "d/").unwrap();
/// remote.put("k", &mut oss_blob::SliceSource::new(b"x"), &mut [0u8; 8]);
/// ```
pub struct RemoteStore {
    store: ReadOnlyStore,
    base: String,
}

/// The most a manifest register may hold, as the writer reads it.
const MANIFEST_READ_BYTES: usize = 4 << 20;
/// The most a descriptor register may hold.
const DESCRIPTOR_READ_BYTES: usize = 1 << 20;

fn store_error(e: &BlobError) -> RemoteError {
    RemoteError::Store(e.to_string())
}

fn format_error(e: &FormatError) -> RemoteError {
    RemoteError::Format(e.to_string())
}

impl RemoteStore {
    /// Opens a handle on `store`, under `base`, the deployment's key prefix.
    ///
    /// # Errors
    ///
    /// [`RemoteError::Format`] for a base that is not empty and does not end in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, RemoteError> {
        descriptor_key(base, "probe").map_err(|e| format_error(&e))?;
        Ok(Self {
            store: ReadOnlyStore::new(store),
            base: base.to_owned(), // ALLOC: one prefix a handle.
        })
    }

    fn read(&self, key: &str, bytes: usize) -> Result<(u64, Vec<u8>), RemoteError> {
        // ALLOC: one buffer a read, MANIFEST_READ_BYTES or DESCRIPTOR_READ_BYTES at most.
        let mut buf = vec![0u8; bytes];
        let read = self
            .store
            .read_register(key, &mut buf)
            .map_err(|e| store_error(&e))?
            .ok_or(RemoteError::Missing)?;
        buf.truncate(read.len);
        Ok((read.generation, buf))
    }

    /// Reads a shard's commit register: one GET.
    ///
    /// # Errors
    ///
    /// [`RemoteError::Missing`] for no register; [`RemoteError::Format`] for a
    /// manifest this reader does not know; [`RemoteError::Store`].
    pub fn shard_manifest(
        &self,
        index: &str,
        uuid: &str,
        shard: u32,
    ) -> Result<ShardManifest, RemoteError> {
        let key = ShardRef { index, uuid, shard }
            .manifest_key(&self.base)
            .map_err(|e| format_error(&e))?;
        let (generation, value) = self.read(&key, MANIFEST_READ_BYTES)?;
        let manifest = CommitManifest::parse(&value).map_err(|e| format_error(&e))?;
        let mut files: Vec<String> = manifest
            .files()
            .iter()
            .map(|f| f.name.to_string()) // ALLOC: one name a file, at most MAX_FILES.
            .collect(); // ALLOC: as above.
        files.sort();
        Ok(ShardManifest {
            generation,
            term: manifest.term,
            files,
        })
    }

    /// Reads an index's descriptor register: one GET.
    ///
    /// # Errors
    ///
    /// [`RemoteError::Missing`] for no index, an alias or a tombstone;
    /// [`RemoteError::Format`]; [`RemoteError::Store`].
    pub fn index_descriptor(&self, index: &str) -> Result<LeaderIndex, RemoteError> {
        let key = descriptor_key(&self.base, index).map_err(|e| format_error(&e))?;
        let (_, value) = self.read(&key, DESCRIPTOR_READ_BYTES)?;
        match NameRecord::parse(&value).map_err(|e| format_error(&e))? {
            NameRecord::Index(record) => Ok(LeaderIndex {
                uuid: record.uuid.to_string(), // ALLOC: one uuid a read.
                number_of_shards: record.number_of_shards,
                mapping: record.mapping.map(|m| m.to_string()), // ALLOC: one mapping a read.
            }),
            _ => Err(RemoteError::Missing),
        }
    }
}
