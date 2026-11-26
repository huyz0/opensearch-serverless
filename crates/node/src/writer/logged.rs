//! Staged commits that ride the node log (docs/specs/logged-commits): the
//! role's side of [`oss_writer::log_commit::LoggedCommits`].

use std::time::Duration;

use oss_format::ShardKey;
use oss_writer::log::Position;
use oss_writer::log_commit::{CommitRecord, LoggedCommits};

use super::WriterRole;
use super::publishing::Key;

impl WriterRole {
    /// Logs every staged commit in the node log's windows, `window` long.
    /// False when logging is on already.
    #[must_use]
    pub fn log_commits(&self, window: Duration) -> bool {
        self.logged
            .set(LoggedCommits::new(
                std::sync::Arc::clone(&self.committer),
                window,
            ))
            .is_ok()
    }

    /// `shard`'s commit was staged: it joins the log, so a successor could
    /// open from it. `through` is the highest sequence number it holds, the
    /// generation the register's write later covers.
    pub(super) fn log_staged(
        &self,
        shard: &ShardKey,
        term: u64,
        age: Option<Duration>,
        covered: Option<Position>,
        manifest: &[u8],
        through: i64,
    ) {
        let (Some(logged), Ok(term)) = (self.logged.get(), i64::try_from(term)) else {
            return;
        };
        // A commit whose oldest pack's age is unknown is not logged: a
        // successor could not tell whether its packs are still named.
        let Some(age) = age else {
            return;
        };
        let age = i64::try_from(age.as_millis()).unwrap_or(0);
        let record = CommitRecord {
            oldest_pack_millis: i64::try_from(super::now_millis())
                .unwrap_or(0)
                .saturating_sub(age),
            covered,
            manifest: String::from_utf8_lossy(manifest).into_owned(),
        };
        // A commit that cannot be logged is only a missed hint.
        let _ = logged.staged(shard, term, &record, u64::try_from(through).unwrap_or(0));
    }

    /// The register now holds `key`'s commits through `through`.
    pub(super) fn log_checkpointed(&self, key: &Key, through: i64) {
        if let Some(logged) = self.logged.get() {
            let shard = ShardKey::new(key.0.as_str(), key.1, None::<&str>);
            logged.checkpointed(&shard, u64::try_from(through).unwrap_or(0));
        }
    }

    /// `key` was let go.
    pub(super) fn log_forget(&self, key: &Key) {
        if let Some(logged) = self.logged.get() {
            logged.forget(&ShardKey::new(key.0.as_str(), key.1, None::<&str>));
        }
    }

    /// Writes the commits nothing carried.
    pub(super) fn log_tick(&self) {
        if let Some(logged) = self.logged.get() {
            logged.tick();
        }
    }
}
