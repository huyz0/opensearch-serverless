//! Which logged commit a successor may open from (docs/specs/open-from-
//! logged-commit). A logged commit is a hint: this only ever says yes to one
//! that is newer than the register by the manifests' own order, young enough
//! that its packs are still referenced, and from a term before the head's.

use std::time::Duration;

use oss_format::{CommitManifest, ShardKey};

use super::Predecessors;
use crate::log_commit::{CommitRecord, commit_key};
use crate::pack::SWAP_DEADLINE_MILLIS;

/// How much younger than the pack swap deadline a commit's oldest pack must be
/// for a successor to build on it: the time to restore and replay, and the
/// skew between two nodes' clocks. Collection waits three deadlines before it
/// deletes a pack it has not seen named, so this is slack on top of slack.
pub const OPEN_MARGIN_MILLIS: u64 = 60_000;

const _: () = assert!(OPEN_MARGIN_MILLIS < SWAP_DEADLINE_MILLIS);

/// A logged commit a successor may open from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The commit's manifest.
    pub manifest: Vec<u8>,
    /// The generation of the register read it was judged against, which the
    /// swap that adopts it is made over.
    pub generation: u64,
    /// The manifest's term.
    pub term: u64,
    /// The manifest's staged sequence number.
    pub stage_seq: i64,
    /// How long ago its oldest pack landed, with the open margin added: the
    /// pack's age to the swap deadline's check.
    pub packed_ago: Duration,
}

/// The newest of `records` a successor at `won_term` may open from, judged
/// against `register`, its manifest and generation as one read gave them;
/// `None` when the register is absent or has no staged sequence number, or no
/// record qualifies. `now_millis` is this node's wall clock.
#[must_use]
pub fn select(
    register: Option<(&[u8], u64)>,
    won_term: i64,
    records: &[CommitRecord],
    now_millis: u64,
) -> Option<Candidate> {
    let (bytes, generation) = register?;
    let held = CommitManifest::parse(bytes).ok()?;
    if held.stage_sequence < 0 {
        return None;
    }
    let won_term = u64::try_from(won_term).ok()?;
    let usable = SWAP_DEADLINE_MILLIS.saturating_sub(OPEN_MARGIN_MILLIS);
    let mut best: Option<Candidate> = None;
    for record in records {
        let Ok(packed) = u64::try_from(record.oldest_pack_millis) else {
            continue;
        };
        // A record that names no age is not known to be young.
        if packed == 0 {
            continue;
        }
        let age = now_millis.saturating_sub(packed);
        if age >= usable {
            continue;
        }
        let Ok(manifest) = CommitManifest::parse(record.manifest.as_bytes()) else {
            continue;
        };
        let order = (manifest.term, manifest.stage_sequence);
        if manifest.term >= won_term
            || order <= (held.term, held.stage_sequence)
            || best
                .as_ref()
                .is_some_and(|b| order <= (b.term, b.stage_seq))
        {
            continue;
        }
        best = Some(Candidate {
            // ALLOC: one manifest, bounded by the manifest's own limit.
            manifest: record.manifest.clone().into_bytes(),
            generation,
            term: manifest.term,
            stage_seq: manifest.stage_sequence,
            packed_ago: Duration::from_millis(age.saturating_add(OPEN_MARGIN_MILLIS)),
        });
    }
    best
}

/// The commit records `writers` logged for `shard`, as many as their logs
/// still hold: each log's own headers say where they are, one ranged read a
/// slice. A log that cannot be read adds none (a logged commit is a hint).
#[must_use]
pub fn read_logged(
    predecessors: &Predecessors,
    writers: &[String],
    shard: &ShardKey,
) -> Vec<CommitRecord> {
    let key = commit_key(shard);
    // ALLOC: the records a head's writers logged, one or a few each.
    let mut found = Vec::new();
    for writer in writers {
        let Ok(Some(log)) = predecessors.of(writer) else {
            continue;
        };
        let _ = log.for_each_record_above(&key, -1, |record| {
            if let Some(commit) = CommitRecord::parse(record.source()) {
                found.push(commit);
            }
            Ok::<_, crate::log::TakeoverError>(())
        });
    }
    found
}
