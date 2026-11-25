//! Reading the metadata of a stock OpenSearch repository through a source
//! store: `index.latest`, `index-N`, a snapshot's own blob, an index's
//! metadata blob and a shard's snapshot blob, each bounded and decoded
//! through `oss-format`, every key built only from names checked as blob
//! names. No LIST: the generation is read from `index.latest`.

use oss_blob::{BlobError, BlobStore, ByteRange};
use oss_format::{
    FormatError, StockEnvelopeLimits, StockIndexMetadata, StockRepositoryData, StockShardSnapshot,
    StockSnapshotInfo, is_blob_name, stock_blob_to_json,
};

/// The most bytes an `index-N` may hold.
pub const MAX_INDEX_N_BYTES: usize = 64 << 20;
/// The most bytes a metadata or shard blob may hold.
pub const MAX_BLOB_BYTES: usize = 16 << 20;
/// The bytes one read goes through.
const SCRATCH_BYTES: usize = 64 << 10;

/// Why a stock repository could not be read.
#[derive(Debug)]
pub enum StockReadError {
    /// A blob the layout needs is not there.
    Missing(String),
    /// A blob is larger than its limit.
    TooLarge {
        /// The blob's key.
        key: String,
        /// The most bytes it may hold.
        limit: usize,
    },
    /// A name that would become part of a key is not a blob name.
    BadName(&'static str),
    /// The store failed.
    Blob(BlobError),
    /// A blob is malformed, unsupported or past a bound; names the blob.
    Format {
        /// The blob's key.
        key: String,
        /// What was wrong.
        error: FormatError,
    },
}

impl std::fmt::Display for StockReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(key) => write!(f, "the repository has no blob [{key}]"),
            Self::TooLarge { key, limit } => write!(f, "blob [{key}] is larger than {limit} bytes"),
            Self::BadName(what) => write!(f, "{what} is not a valid blob name"),
            Self::Blob(e) => write!(f, "{e}"),
            Self::Format { key, error } => write!(f, "blob [{key}]: {error}"),
        }
    }
}

impl std::error::Error for StockReadError {}

/// A stock repository: a store the node only reads, and the repository's
/// key prefix in it (empty, or ending in `/`).
pub struct StockRepo<'a> {
    store: &'a dyn BlobStore,
    prefix: &'a str,
}

impl<'a> StockRepo<'a> {
    /// The repository at `prefix` of `store`.
    #[must_use]
    pub fn new(store: &'a dyn BlobStore, prefix: &'a str) -> Self {
        Self { store, prefix }
    }

    /// The whole blob at `key`, at most `limit` bytes.
    fn blob(&self, key: &str, limit: usize) -> Result<Vec<u8>, StockReadError> {
        let key = [self.prefix, key].concat();
        // ALLOC: one scratch buffer a read, SCRATCH_BYTES.
        let mut scratch = vec![0u8; SCRATCH_BYTES];
        // ALLOC: grows to at most `limit`, which the caller reserved.
        let mut out = Vec::new();
        let mut over = false;
        let mut sink = |chunk: &[u8]| -> oss_blob::BlobResult<()> {
            if out.len().saturating_add(chunk.len()) > limit {
                over = true;
                return Err(BlobError::Stream("a blob past its limit"));
            }
            out.extend_from_slice(chunk);
            Ok(())
        };
        match self
            .store
            .get(&key, ByteRange::Full, &mut scratch, &mut sink)
        {
            Ok(_) => Ok(out),
            Err(BlobError::NotFound) => Err(StockReadError::Missing(key)),
            Err(_) if over => Err(StockReadError::TooLarge { key, limit }),
            Err(e) => Err(StockReadError::Blob(e)),
        }
    }

    /// The generation `index.latest` names: one 8-byte big-endian number.
    ///
    /// # Errors
    ///
    /// [`StockReadError::Missing`] without `index.latest`.
    pub fn generation(&self) -> Result<u64, StockReadError> {
        let bytes = self.blob("index.latest", 8)?;
        let raw: [u8; 8] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| StockReadError::Format {
                key: "index.latest".to_owned(), // ALLOC: one key, at most 300 bytes.
                error: FormatError::Corrupt {
                    what: "stock repository",
                    reason: "index.latest is not an 8-byte generation",
                },
            })?;
        i64::from_be_bytes(raw)
            .try_into()
            .map_err(|_| StockReadError::Format {
                key: "index.latest".to_owned(), // ALLOC: one key, at most 300 bytes.
                error: FormatError::Corrupt {
                    what: "stock repository",
                    reason: "index.latest names a negative generation",
                },
            })
    }

    /// The repository's data at `generation`.
    ///
    /// # Errors
    ///
    /// A [`StockReadError`], naming `index-<generation>`.
    pub fn repository_data(&self, generation: u64) -> Result<StockRepositoryData, StockReadError> {
        let key = format!("index-{generation}"); // ALLOC: one key, at most 300 bytes.
        let bytes = self.blob(&key, MAX_INDEX_N_BYTES)?;
        StockRepositoryData::parse(&bytes).map_err(|error| StockReadError::Format { key, error })
    }

    /// A snapshot's own blob.
    ///
    /// # Errors
    ///
    /// A [`StockReadError`], naming the blob.
    pub fn snapshot_info(&self, uuid: &str) -> Result<StockSnapshotInfo, StockReadError> {
        check(uuid, "a snapshot uuid")?;
        let key = format!("snap-{uuid}.dat"); // ALLOC: one key, at most 300 bytes.
        let json = self.json(&key, "snapshot")?;
        StockSnapshotInfo::parse(&json).map_err(|error| StockReadError::Format { key, error })
    }

    /// An index's metadata in the snapshot that wrote `meta_id`.
    ///
    /// # Errors
    ///
    /// A [`StockReadError`], naming the blob.
    pub fn index_metadata(
        &self,
        index_id: &str,
        meta_id: &str,
    ) -> Result<StockIndexMetadata, StockReadError> {
        check(index_id, "an index id")?;
        check(meta_id, "a metadata id")?;
        let key = format!("indices/{index_id}/meta-{meta_id}.dat"); // ALLOC: one key, at most 300 bytes.
        let json = self.json(&key, "index-metadata")?;
        StockIndexMetadata::parse(&json).map_err(|error| StockReadError::Format { key, error })
    }

    /// A shard's snapshot blob.
    ///
    /// # Errors
    ///
    /// A [`StockReadError`], naming the blob.
    pub fn shard_snapshot(
        &self,
        index_id: &str,
        shard: u32,
        snapshot_uuid: &str,
    ) -> Result<StockShardSnapshot, StockReadError> {
        check(index_id, "an index id")?;
        check(snapshot_uuid, "a snapshot uuid")?;
        let key = format!("indices/{index_id}/{shard}/snap-{snapshot_uuid}.dat"); // ALLOC: one key, at most 300 bytes.
        let json = self.json(&key, "snapshot")?;
        StockShardSnapshot::parse(&json).map_err(|error| StockReadError::Format { key, error })
    }

    /// The JSON of the envelope blob at `key`.
    fn json(&self, key: &str, codec: &str) -> Result<Vec<u8>, StockReadError> {
        let blob = self.blob(key, MAX_BLOB_BYTES)?;
        // ALLOC: at most the envelope's 64 MiB output bound, one a blob.
        let mut out = Vec::new();
        stock_blob_to_json(&blob, codec, &StockEnvelopeLimits::default(), &mut out).map_err(
            |error| StockReadError::Format {
                key: key.to_owned(), // ALLOC: one key, at most 300 bytes.
                error,
            },
        )?;
        Ok(out)
    }
}

fn check(name: &str, what: &'static str) -> Result<(), StockReadError> {
    if is_blob_name(name) {
        Ok(())
    } else {
        Err(StockReadError::BadName(what))
    }
}
