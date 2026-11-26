//! Choosing the commit a taken shard opens from (docs/specs/open-from-
//! logged-commit): the register's, or a newer one a predecessor logged.

use std::time::Instant;

use oss_format::ShardKey;
use oss_writer::takeover::logged::{Candidate, read_logged, select};

use super::{Takeover, now_millis};

/// A logged commit to open from, with the moment its oldest pack is judged
/// to have landed.
pub(super) struct Adopting {
    pub(super) candidate: Candidate,
    pub(super) packed_at: Instant,
}

/// The newest commit `head`'s writers logged for `shard` that the register
/// (`value`, read at `generation`) does not hold and a successor at the head's
/// term may build on; `None` when there is none, or on anything unreadable:
/// a logged commit is a hint.
pub(super) fn choose(
    takeover: &Takeover,
    shard: &ShardKey,
    writers: &[String],
    head_term: i64,
    register: Option<(&[u8], u64)>,
) -> Option<Adopting> {
    let records = read_logged(&takeover.predecessors, writers, shard);
    let candidate = select(register, head_term, &records, now_millis())?;
    let packed_at = Instant::now().checked_sub(candidate.packed_ago)?;
    Some(Adopting {
        candidate,
        packed_at,
    })
}
