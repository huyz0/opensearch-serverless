//! Reading a shard's commit register as the publisher builds on it, and the
//! fence a register's manifest puts on a publish.

use std::collections::BTreeMap;

use oss_blob::register::ABSENT_GENERATION;
use oss_format::CommitManifest;

use super::PublishError;
use super::publisher::ShardPublisher;

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
    /// The manifest's staged sequence number; -1 for none.
    pub(super) stage_seq: i64,
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
                    stage_seq: -1,
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
        let Some(value) = self.register.read()? else {
            return Err(PublishError::Unusable("no commit published"));
        };
        let last = last_of(&value.bytes, value.generation)?;
        Ok((last, value.generation, value.bytes))
    }
}

/// The commit a manifest names, as the publisher remembers one, read at
/// `generation`.
pub(super) fn last_of(bytes: &[u8], generation: u64) -> Result<Last, PublishError> {
    {
        let manifest = CommitManifest::parse(bytes)?;
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
        Ok(Last {
            generation,
            term: manifest.term,
            writer: manifest.writer.as_deref().map(str::to_owned),
            files,
            stage_seq: manifest.stage_sequence,
        })
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
