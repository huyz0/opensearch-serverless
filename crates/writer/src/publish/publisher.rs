//! `ShardPublisher`: a shard's commit, file by file, into its segment blobs
//! and its register.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use lucene_store::Directory;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{ManifestEntry, ShardRef, encode_manifest};

use super::PublishError;
use super::commit_register::{CommitRegister, OwnRegister};
use super::names::segment_name;
use super::packed::{PackPlacement, Packable};
use super::register::{Known, Last, fence};
use super::staging::{Staged, due};
use crate::boot_clock::BootInstant;
use crate::bulk::IdGenerator;
use crate::pack::{Part, SWAP_DEADLINE_MILLIS};

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
    /// Whether the manifest is staged, not in the register: the generation
    /// is then the register's, which this commit did not move.
    pub staged: bool,
}

/// One shard's publisher. Callers serialise its publishes.
pub struct ShardPublisher {
    pub(super) store: Arc<dyn BlobStore>,
    pub(super) base: String,
    pub(super) register: Arc<dyn CommitRegister>,
    pub(super) ids: IdGenerator,
    pub(super) last: Option<Last>,
    pub(super) packs: Option<PackPlacement>,
    pub(super) swap_deadline: Duration,
    /// The checkpoint age when staging is on.
    pub(super) staging: Option<Duration>,
    pub(super) staged: Option<Staged>,
    pub(super) sequence: i64,
    /// When the oldest pack of a commit adopted from a predecessor's log
    /// landed, minus the open margin: the commit is only that old to a swap.
    pub(super) adopted: Option<BootInstant>,
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
        let register = OwnRegister::new(Arc::clone(&store), shard.manifest_key(base)?);
        Ok(Self {
            store,
            base: shard.base_path(base)?,
            // ALLOC: one register a publisher.
            register: Arc::new(register),
            ids: IdGenerator::random(),
            last: None,
            packs: None,
            swap_deadline: Duration::from_millis(SWAP_DEADLINE_MILLIS),
            staging: None,
            staged: None,
            sequence: -1,
            adopted: None,
        })
    }

    /// Records this shard's commits somewhere other than its own manifest
    /// register: a bucket member's entry in its bucket's manifest. Set before
    /// the first publish.
    pub fn commit_register(&mut self, register: Arc<dyn CommitRegister>) {
        self.register = register;
        self.last = None;
    }

    /// Sends this shard's next publishes' small segment blobs to node packs
    /// (`Some`), or to blobs of their own (`None`, the default). Decided per
    /// publish, so a switch of the setting takes effect at the next one.
    pub fn use_packs(&mut self, placement: Option<PackPlacement>) {
        self.packs = placement;
    }

    /// How long a publish may take from its pack landing to its manifest
    /// swap; [`SWAP_DEADLINE_MILLIS`] unless a test shortens it.
    pub fn set_swap_deadline(&mut self, deadline: Duration) {
        self.swap_deadline = deadline;
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
        self.publish_with(dir, files, term, writer, wal_ordinal, now_millis, false)
    }

    /// [`Self::publish`] for a routine publish: with staging on, it keeps the
    /// manifest instead of swapping the register while the publisher holds
    /// the register it last wrote in this term as `writer` and the oldest
    /// staged commit is younger than the checkpoint age.
    ///
    /// # Errors
    ///
    /// As [`Self::publish`].
    pub fn publish_routine(
        &mut self,
        dir: &dyn Directory,
        files: &[String],
        term: u64,
        writer: &str,
        wal_ordinal: i64,
        now_millis: u64,
    ) -> Result<Published, PublishError> {
        self.publish_with(dir, files, term, writer, wal_ordinal, now_millis, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_with(
        &mut self,
        dir: &dyn Directory,
        files: &[String],
        term: u64,
        writer: &str,
        wal_ordinal: i64,
        now_millis: u64,
        routine: bool,
    ) -> Result<Published, PublishError> {
        let adopting = self.adopted.is_some();
        let done = self.publish_once(dir, files, term, writer, wal_ordinal, now_millis, routine);
        if adopting && done.is_err() {
            // The commit adopted from a log was this publish's only chance: a
            // later one builds on the register.
            self.adopted = None;
            self.last = None;
        }
        done
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_once(
        &mut self,
        dir: &dyn Directory,
        files: &[String],
        term: u64,
        writer: &str,
        wal_ordinal: i64,
        now_millis: u64,
        routine: bool,
    ) -> Result<Published, PublishError> {
        // A staged commit whose packs may be judged unreferenced is dropped:
        // its files are uploaded again.
        if self
            .staged
            .as_ref()
            .is_some_and(|s| expired(s.oldest.elapsed(), self.swap_deadline))
        {
            self.staged = None;
        }
        // An adopted commit's one chance: it is spent by this publish, and a
        // swap lost over it is not retried over what the register holds now.
        let adopted = self.adopted.take();
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut uploaded: BTreeMap<String, Known> = BTreeMap::new();
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut blobs: Vec<String> = Vec::new();
        // When this publish asked for its pack, the earliest, so the swap's
        // age check never undercounts how long ago a pack landed.
        let mut packed_at: Option<BootInstant> = None;
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
            // A publisher made again in a term continues what the register holds.
            self.sequence = self.sequence.max(base.stage_seq);
            let generation = generation.unwrap_or(base.generation);
            let mut known = base.files.clone();
            if let Some(staged) = &self.staged {
                known.extend(staged.files.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            known.extend(uploaded.iter().map(|(k, v)| (k.clone(), v.clone())));
            let entries = self.upload(
                dir,
                files,
                term,
                &known,
                &mut uploaded,
                &mut blobs,
                &mut packed_at,
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
            let stage_sequence = if self.staging.is_some() {
                self.sequence = self.sequence.saturating_add(1);
                self.sequence
            } else {
                -1
            };
            let manifest = encode_manifest(term, Some(writer), &refs, wal_ordinal, stage_sequence)?;
            // Collection deletes a pack no manifest names once it is old
            // enough; a swap this late could name one already gone.
            if packed_at
                .into_iter()
                .chain(adopted)
                .any(|at| expired(at.elapsed(), self.swap_deadline))
            {
                self.last = None;
                return Err(PublishError::PackExpired);
            }
            let stage_now = routine
                && remembered
                && base.term == term
                && base.writer.as_deref() == Some(writer)
                && self.staging.is_some_and(|age| {
                    !due(
                        self.staged
                            .as_ref()
                            .map_or(Duration::ZERO, |s| s.oldest.elapsed()),
                        age,
                    )
                });
            if stage_now {
                let oldest = self
                    .staged
                    .as_ref()
                    .map_or_else(BootInstant::now, |s| s.oldest)
                    .min(packed_at.unwrap_or_else(BootInstant::now));
                // ALLOC: one manifest's bytes and names a shard with a staged commit.
                self.staged = Some(Staged {
                    manifest: manifest.clone(),
                    files: entries,
                    term,
                    // ALLOC: one name a shard with a staged commit.
                    writer: writer.to_owned(),
                    oldest,
                });
                return Ok(Published {
                    manifest,
                    generation,
                    blobs,
                    superseded: false,
                    staged: true,
                });
            }
            match self.register.swap(generation, &manifest)? {
                SwapOutcome::Applied(now) => {
                    self.staged = None;
                    self.last = Some(Last {
                        generation: now,
                        term,
                        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                        writer: Some(writer.to_owned()),
                        files: entries,
                        stage_seq: stage_sequence,
                    });
                    return Ok(Published {
                        manifest,
                        generation: now,
                        blobs,
                        superseded: false,
                        staged: false,
                    });
                }
                SwapOutcome::Conflict(_) => {
                    self.last = None;
                    self.staged = None;
                    if adopted.is_some() {
                        return Err(PublishError::AdoptionLost);
                    }
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
    pub(super) fn after_lost_swap(
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
                staged: false,
            });
        }
        Err(PublishError::Stale {
            term,
            found: Some(current.term),
        })
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
        packed_at: &mut Option<BootInstant>,
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
        // No fresh segment to carry the commit points: they get a blob of
        // their own, or the manifest would name no commit a reader can open.
        if by_segment.is_empty() && !commit_points.is_empty() {
            by_segment.insert("segments", std::mem::take(&mut commit_points));
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
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut packable: Vec<(Packable, Part)> = Vec::new();
        for (segment, mut group) in by_segment {
            if Some(segment) == last {
                group.extend(commit_points.iter().copied());
            }
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let plain = format!("{segment}.seg");
            let taken = published_segments.contains(segment);
            if let Some(found) =
                self.pack_candidate(dir, segment, &plain, taken, &mut group, now_millis)?
            {
                packable.push(found);
                continue;
            }
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
        self.put_packed(packable, packed_at, uploaded, &mut entries)?;
        Ok(entries)
    }
}

/// Whether a pack put `elapsed` ago is past the swap deadline: at exactly
/// the deadline it is not.
pub(super) fn expired(elapsed: Duration, deadline: Duration) -> bool {
    elapsed > deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_is_expired_only_past_the_deadline() {
        let deadline = Duration::from_secs(5);
        assert!(!expired(Duration::from_secs(4), deadline));
        assert!(!expired(deadline, deadline));
        assert!(expired(deadline + Duration::from_nanos(1), deadline));
    }
}
