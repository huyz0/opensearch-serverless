//! `ShardPublisher`: a shard's commit, file by file, into its segment blobs
//! and its register.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lucene_store::Directory;
use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, CreateOutcome, SwapOutcome};
use oss_format::{
    CommitManifest, FileLocation, ManifestEntry, ShardRef, encode_manifest, encode_segment_header,
    segment_blob_order,
};

use super::PublishError;
use super::blob::SegmentBlob;
use super::names::segment_name;
use crate::bulk::IdGenerator;

/// The buffer a blob's PUT streams through.
const IO_BUF_BYTES: usize = 64 * 1024;

/// What a register read first has room for: well over a thousand files.
const MANIFEST_READ_FIRST: usize = 256 << 10;

/// The most a manifest read keeps: 16,384 files at well under 200 bytes.
const MANIFEST_READ_BYTES: usize = 4 << 20;

/// What a publish did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    /// The manifest as the register holds it, for readers that are pushed
    /// the commit.
    pub manifest: Vec<u8>,
    /// The register's generation after the publish.
    pub generation: u64,
    /// Blobs this publish created, by name in the term container.
    pub blobs: Vec<String>,
    /// Whether another publish of this writer's own got there first, so the
    /// manifest is its, not this one's.
    pub superseded: bool,
}

/// A file the register already names: where it is and how long it is.
#[derive(Clone, Debug)]
struct Known {
    location: String,
    length: u64,
}

/// The manifest this publisher last wrote, and the generation it wrote it
/// at, so the next publish reads no register.
#[derive(Clone, Debug)]
struct Last {
    generation: u64,
    term: u64,
    writer: Option<String>,
    files: BTreeMap<String, Known>,
}

/// One shard's publisher. Callers serialise its publishes.
pub struct ShardPublisher {
    store: Arc<dyn BlobStore>,
    base: String,
    manifest_key: String,
    ids: IdGenerator,
    last: Option<Last>,
}

impl std::fmt::Debug for ShardPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShardPublisher")
            .field("base", &self.base)
            .field("remembered", &self.last.is_some())
            .finish_non_exhaustive()
    }
}

impl ShardPublisher {
    /// A publisher of `shard` under the cluster's `base` (empty, or ending in
    /// `/`).
    ///
    /// # Errors
    ///
    /// [`PublishError::Format`] when the index name, uuid or base cannot be
    /// part of a key.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        shard: &ShardRef<'_>,
    ) -> Result<Self, PublishError> {
        Ok(Self {
            store,
            base: shard.base_path(base)?,
            manifest_key: shard.manifest_key(base)?,
            ids: IdGenerator::random(),
            last: None,
        })
    }

    /// Publishes the commit whose files are `files` in `dir`, as `writer` of
    /// `term`, `wal_ordinal` being how far into the node log it reaches
    /// (negative for unknown); `now_millis` names a blob whose plain name is
    /// taken.
    ///
    /// Only the files the last published commit does not already name are
    /// uploaded, a segment's new files as one blob, and the manifest is
    /// swapped in over the generation this publisher last wrote.
    ///
    /// # Errors
    ///
    /// [`PublishError::Stale`] when a newer term holds the register,
    /// [`PublishError::Foreign`] when another writer holds this term, and
    /// the store's, the format's or the directory's failures.
    pub fn publish(
        &mut self,
        dir: &dyn Directory,
        files: &[String],
        term: u64,
        writer: &str,
        wal_ordinal: i64,
        now_millis: u64,
    ) -> Result<Published, PublishError> {
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut uploaded: BTreeMap<String, Known> = BTreeMap::new();
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut blobs: Vec<String> = Vec::new();
        // At most two rounds: a swap lost over a remembered generation is
        // tried once more over what the register holds.
        for round in 0..2 {
            let remembered = self.last.is_some();
            let (base, generation) = match self.last.clone() {
                Some(last) => (last, None),
                None => {
                    let read = self.read_register()?;
                    (read.0, Some(read.1))
                }
            };
            fence(&base, term, writer)?;
            let generation = generation.unwrap_or(base.generation);
            let mut known = base.files.clone();
            known.extend(uploaded.iter().map(|(k, v)| (k.clone(), v.clone())));
            let entries = self.upload(
                dir,
                files,
                term,
                &known,
                &mut uploaded,
                &mut blobs,
                now_millis,
            )?;
            let refs: Vec<ManifestEntry<'_>> = entries
                .iter()
                .map(|(name, known)| ManifestEntry {
                    name,
                    location: known.location.clone(),
                    length: known.length,
                })
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                .collect();
            let manifest = encode_manifest(term, Some(writer), &refs, wal_ordinal, -1)?;
            match self
                .store
                .swap_register(&self.manifest_key, generation, &manifest)?
            {
                SwapOutcome::Applied(now) => {
                    self.last = Some(Last {
                        generation: now,
                        term,
                        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                        writer: Some(writer.to_owned()),
                        files: entries,
                    });
                    return Ok(Published {
                        manifest,
                        generation: now,
                        blobs,
                        superseded: false,
                    });
                }
                SwapOutcome::Conflict(_) => {
                    self.last = None;
                    if remembered && round == 0 {
                        // The register moved under what this publisher
                        // remembered: read it and publish over it, once.
                        continue;
                    }
                    return self.after_lost_swap(term, writer, blobs);
                }
            }
        }
        Err(PublishError::Stale { term, found: None })
    }

    /// The register holds another manifest than the one this publish swapped
    /// in: this writer's own concurrent publish, whose manifest is then the
    /// answer, or the fence a newer term is.
    fn after_lost_swap(
        &mut self,
        term: u64,
        writer: &str,
        blobs: Vec<String>,
    ) -> Result<Published, PublishError> {
        let (current, generation, bytes) = self.read_register_bytes()?;
        if current.term == term && current.writer.as_deref() == Some(writer) {
            self.last = Some(current);
            return Ok(Published {
                manifest: bytes,
                generation,
                blobs,
                superseded: true,
            });
        }
        Err(PublishError::Stale {
            term,
            found: Some(current.term),
        })
    }

    fn read_register(&self) -> Result<(Last, u64), PublishError> {
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
    fn read_register_bytes(&self) -> Result<(Last, u64, Vec<u8>), PublishError> {
        let mut size = MANIFEST_READ_FIRST;
        let (buf, read) = loop {
            // ALLOC: one buffer a register read, at most manifest-sized: a publisher's first, or after a lost swap.
            let mut buf = vec![0u8; size];
            match self.store.read_register(&self.manifest_key, &mut buf) {
                Ok(Some(read)) => break (buf, read),
                Ok(None) => return Err(PublishError::Unusable("no commit published")),
                // A larger manifest than the first buffer: once more with room for the most.
                Err(BlobError::InvalidArgument(_)) if size < MANIFEST_READ_BYTES => {
                    size = MANIFEST_READ_BYTES;
                }
                Err(e) => return Err(e.into()),
            }
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

    /// Uploads what `known` does not hold, returning every file of the
    /// commit with its location.
    #[allow(clippy::too_many_arguments)]
    fn upload(
        &mut self,
        dir: &dyn Directory,
        files: &[String],
        term: u64,
        known: &BTreeMap<String, Known>,
        uploaded: &mut BTreeMap<String, Known>,
        blobs: &mut Vec<String>,
        now_millis: u64,
    ) -> Result<BTreeMap<String, Known>, PublishError> {
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let term_dir = format!("t={term}");
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut entries: BTreeMap<String, Known> = BTreeMap::new();
        // Fresh files by segment; the commit points ride in the last
        // segment's blob.
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut by_segment: BTreeMap<&str, Vec<(&str, u64)>> = BTreeMap::new();
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut commit_points: Vec<(&str, u64)> = Vec::new();
        for name in files {
            if let Some(have) = known.get(name) {
                entries.insert(name.clone(), have.clone());
                continue;
            }
            let length = dir
                .file_length(name)
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                .map_err(|e| PublishError::Directory(format!("{name}: {e}")))?;
            if length == 0 {
                self.put_whole(&term_dir, name, uploaded, &mut entries)?;
                continue;
            }
            if name.starts_with("segments") {
                commit_points.push((name, length));
            } else {
                by_segment
                    .entry(segment_name(name))
                    .or_default()
                    .push((name, length));
            }
        }
        // Segments that already have files published: their next files,
        // such as deletions, cannot take the segment's plain name.
        let published_segments: BTreeSet<&str> = known
            .keys()
            .filter(|n| !n.starts_with("segments"))
            .map(|n| segment_name(n))
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            .collect();
        let last = by_segment.keys().next_back().copied();
        for (segment, mut group) in by_segment {
            if Some(segment) == last {
                group.extend(commit_points.iter().copied());
            }
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let plain = format!("{segment}.seg");
            let taken = published_segments.contains(segment);
            self.put_segment(
                dir,
                &term_dir,
                segment,
                &plain,
                taken,
                &mut group,
                now_millis,
                uploaded,
                &mut entries,
                blobs,
            )?;
        }
        Ok(entries)
    }

    fn put_whole(
        &self,
        term_dir: &str,
        name: &str,
        uploaded: &mut BTreeMap<String, Known>,
        entries: &mut BTreeMap<String, Known>,
    ) -> Result<(), PublishError> {
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let key = format!("{}{term_dir}/{name}", self.base);
        self.store
            .put(&key, &mut oss_blob::SliceSource::new(&[]), &mut [0u8; 1])?;
        let known = Known {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            location: term_dir.to_owned(),
            length: 0,
        };
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        uploaded.insert(name.to_owned(), known.clone());
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        entries.insert(name.to_owned(), known);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn put_segment(
        &mut self,
        dir: &dyn Directory,
        term_dir: &str,
        segment: &str,
        plain: &str,
        taken: bool,
        group: &mut [(&str, u64)],
        now_millis: u64,
        uploaded: &mut BTreeMap<String, Known>,
        entries: &mut BTreeMap<String, Known>,
        blobs: &mut Vec<String>,
    ) -> Result<(), PublishError> {
        segment_blob_order(group);
        let header = encode_segment_header(group)?;
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut parts = Vec::with_capacity(group.len());
        for (name, length) in group.iter() {
            let input = dir
                .open(name)
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                .map_err(|e| PublishError::Directory(format!("{name}: {e}")))?;
            if u64::try_from(input.len()).ok() != Some(*length) {
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                return Err(PublishError::Directory(format!(
                    "{name} changed length while it was being published"
                )));
            }
            parts.push(input);
        }
        let mut source = SegmentBlob::new(header.clone(), parts);
        let mut buf = [0u8; IO_BUF_BYTES];
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut blob = plain.to_owned();
        let mut created = false;
        if !taken {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let key = format!("{}{term_dir}/{blob}", self.base);
            created =
                self.store.create_if_absent(&key, &mut source, &mut buf)? == CreateOutcome::Created;
        }
        if !created {
            // An earlier attempt of this term, or a writer that reused the
            // segment's name, left a blob by it.
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            blob = format!("{segment}-{}.seg", self.ids.next(now_millis));
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let key = format!("{}{term_dir}/{blob}", self.base);
            oss_blob::ChunkSource::rewind(&mut source)?;
            if self.store.create_if_absent(&key, &mut source, &mut buf)? != CreateOutcome::Created {
                return Err(PublishError::Unusable("a fresh blob name was taken"));
            }
        }
        let mut offset = u64::try_from(header.len()).unwrap_or(u64::MAX);
        for (name, length) in group.iter() {
            let known = Known {
                location: FileLocation::Packed {
                    term_dir,
                    blob: &blob,
                    offset,
                    length: *length,
                }
                .encode(),
                length: *length,
            };
            offset = offset.saturating_add(*length);
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            uploaded.insert((*name).to_owned(), known.clone());
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            entries.insert((*name).to_owned(), known);
        }
        blobs.push(blob);
        Ok(())
    }
}

/// The register's manifest against this publish: a newer term is a stale
/// writer, and this term from another node is a second writer.
fn fence(base: &Last, term: u64, writer: &str) -> Result<(), PublishError> {
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
