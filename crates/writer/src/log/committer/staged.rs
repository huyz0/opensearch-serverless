//! Records joined to the node log's windows and not yet waited for: the
//! halves of a commit, so a caller can stage under one lock and wait outside it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use oss_format::ShardKey;

use super::{CommitError, NodeLogCommitter, Position};
use crate::log::window::Latch;

/// A shard's records joined to the windows, from [`NodeLogCommitter::stage`].
///
/// Dropping one without waiting leaves its windows to be driven by whoever
/// waits on them; [`Staged::finish`] drives them to the end.
#[derive(Debug)]
pub struct Staged {
    shard: ShardKey,
    /// The windows the records joined, oldest first.
    joined: Vec<Arc<Latch>>,
    pub(super) mine: Arc<Latch>,
    /// Whether a writer already drove the youngest window when it was joined.
    pub(super) driven: bool,
    joined_at: Instant,
}

impl Staged {
    pub(super) fn new(
        shard: ShardKey,
        joined: Vec<Arc<Latch>>,
        mine: Arc<Latch>,
        driven: bool,
        joined_at: Instant,
    ) -> Self {
        Self {
            shard,
            joined,
            mine,
            driven,
            joined_at,
        }
    }

    /// Waits until the windows are written and says where the records
    /// landed: the second half of [`NodeLogCommitter::commit`].
    ///
    /// # Errors
    ///
    /// [`CommitError`] if a window carrying them did not land, or the shard
    /// was refused admission to its epoch.
    pub fn wait(&self, committer: &NodeLogCommitter) -> Result<Option<Position>, CommitError> {
        committer.land(self);
        if let Some(result) = self.mine.get() {
            committer.timings.caller(self.joined_at, &result);
        }
        // Windows are written in order, one at a time, so the last landing
        // means the others were written.
        for latch in &self.joined {
            let result = latch.get().ok_or(CommitError::Lost)?;
            if let Some(failure) = result.failures.get(&self.shard) {
                return Err(failure.clone());
            }
        }
        self.mine
            .get()
            .and_then(|r| r.landed)
            .map(Some)
            .ok_or(CommitError::Lost)
    }

    /// Drives or waits until every window is final, whatever its outcome,
    /// and never unwinds: a panic from the write is caught here, and reported
    /// as `true`, so that a drop guard can call it during an unwind.
    pub fn finish(&self, committer: &NodeLogCommitter) -> bool {
        catch_unwind(AssertUnwindSafe(|| committer.land(self))).is_err()
    }
}
