//! Reading a shard's commit register as the publisher builds on it, and the
//! fence a register's manifest puts on a publish.

use std::collections::BTreeMap;

use oss_blob::BlobError;
use oss_blob::register::ABSENT_GENERATION;
use oss_format::CommitManifest;

use super::PublishError;
use super::publisher::ShardPublisher;

/// What a register read first has room for: well over a thousand files.
const MANIFEST_READ_FIRST: usize = 256 << 10;

/// The most a manifest read keeps: 16,384 files at well under 200 bytes.
const MANIFEST_READ_BYTES: usize = 4 << 20;

/// A file the register already names: where it is and how long it is.
#[derive(Clone, Debug)]
pub(super) struct Known {
    pub(super) location: String,
    pub(super) length: u64,
}

/// The manifest this publisher last wrote, and the generation it wrote it
/// at, so the next publish reads no register.
#[derive(Clone, Debug)]
pub(super) struct Last {
    pub(super) generation: u64,
    pub(super) term: u64,
    pub(super) writer: Option<String>,
    pub(super) files: BTreeMap<String, Known>,
}

impl ShardPublisher {
    pub(super) fn read_register(&self) -> Result<(Last, u64), PublishError> {
        match self.read_register_bytes() {
            Ok((last, generation, _)) => Ok((last, generation)),
            Err(PublishError::Unusable("no commit published")) => Ok((
                Last {
                    generation: ABSENT_GENERATION,
                    term: 0,
                    writer: None,
                    // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                    files: BTreeMap::new(),
                },
                ABSENT_GENERATION,
            )),
            Err(e) => Err(e),
        }
    }

    /// The register as it is: its manifest, generation and manifest bytes.
    /// An absent register is [`PublishError::Unusable`] "no commit
    /// published".
    pub(super) fn read_register_bytes(&self) -> Result<(Last, u64, Vec<u8>), PublishError> {
        let mut found = None;
        for size in [MANIFEST_READ_FIRST, MANIFEST_READ_BYTES] {
            // ALLOC: one buffer a register read, at most manifest-sized: a publisher's first, or after a lost swap.
            let mut buf = vec![0u8; size];
            match self.store.read_register(&self.manifest_key, &mut buf) {
                Ok(Some(read)) => {
                    found = Some((buf, read));
                    break;
                }
                Ok(None) => return Err(PublishError::Unusable("no commit published")),
                // A larger manifest than this buffer: once more with room for the most.
                Err(BlobError::InvalidArgument(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let Some((buf, read)) = found else {
            return Err(PublishError::Unusable("a manifest too large to read"));
        };
        let value = buf.get(..read.len).unwrap_or_default();
        let manifest = CommitManifest::parse(value)?;
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut files = BTreeMap::new();
        for file in manifest.files() {
            let Some(length) = file.length else {
                return Err(PublishError::Unusable("a file has no recorded length"));
            };
            // ALLOC: one entry a published file, at most manifest::MAX_FILES.
            files.insert(
                file.name.to_string(),
                Known {
                    location: file.location.encode(),
                    length,
                },
            );
        }
        let last = Last {
            generation: read.generation,
            term: manifest.term,
            writer: manifest.writer.as_deref().map(str::to_owned),
            files,
        };
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        Ok((last, read.generation, value.to_vec()))
    }
}

/// The register's manifest against this publish: a newer term is a stale
/// writer, and this term from another node is a second writer.
pub(super) fn fence(base: &Last, term: u64, writer: &str) -> Result<(), PublishError> {
    if base.generation == ABSENT_GENERATION {
        return Ok(());
    }
    if base.term > term {
        return Err(PublishError::Stale {
            term,
            found: Some(base.term),
        });
    }
    if base.term == term
        && let Some(existing) = &base.writer
        && existing != writer
    {
        return Err(PublishError::Foreign {
            term,
            existing: existing.clone(),
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            ours: writer.to_owned(),
        });
    }
    Ok(())
}
