//! Commit staging: a routine publish keeps its manifest instead of swapping
//! the register, and the register is written as a checkpoint.
//!
//! Ported from `CommitStaging` (huyz0/OpenSearch `feature/serverlessshell`).
//! What is staged is never the only home of a record: the node trims its
//! write log only behind a register write.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};

use super::register::Known;
use crate::pack::SWAP_DEADLINE_MILLIS;

/// `serverless.wal.node_log.commits.staging`: routine publishes keep their
/// manifest and write the register at a checkpoint. Default `false`.
pub const STAGING: BoolSetting = BoolSetting::new(
    "serverless.wal.node_log.commits.staging",
    false,
    Scope::Node,
    "A routine publish uploads its files but keeps its manifest, and the shard's register is \
     written once a checkpoint age: one request fewer on every publish between. A successor \
     replays up to a checkpoint age more of the write log.",
);

/// The default checkpoint age, a minute.
pub const DEFAULT_CHECKPOINT_MILLIS: i64 = 60_000;

/// `serverless.wal.node_log.commits.checkpoint_millis`: how old the oldest
/// staged commit gets before the register is written. Default 60,000.
pub const CHECKPOINT_MILLIS: LongSetting = LongSetting::new(
    "serverless.wal.node_log.commits.checkpoint_millis",
    DEFAULT_CHECKPOINT_MILLIS,
    MIN_STAGED_MILLIS as i64,
    MAX_STAGED_MILLIS as i64,
    Scope::Node,
    "With commit staging on, how old the oldest staged commit gets before the shard's register \
     is written, in milliseconds: at least two commit periods and 1,000, at most half the pack \
     deadline.",
);

/// `serverless.wal.node_log.commits.logged`: each staged commit joins the node
/// log under the shard's commit key. Default `false`; needs staging.
pub const LOGGED: BoolSetting = BoolSetting::new(
    "serverless.wal.node_log.commits.logged",
    false,
    Scope::Node,
    "With commit staging on, each staged commit joins the node's write log, riding a window \
     that is open anyway, so a successor can open from it instead of the older register.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    STAGING.setting(),
    CHECKPOINT_MILLIS.setting(),
    LOGGED.setting(),
];

/// The longest a commit may stay staged: half the pack deadline, so a slow
/// store or renewal still writes it in time.
pub const MAX_STAGED_MILLIS: u64 = SWAP_DEADLINE_MILLIS / 2;

/// The shortest a commit may stay staged, whatever the commit period.
pub const MIN_STAGED_MILLIS: u64 = 1_000;

/// The shortest a commit may stay staged, in commit periods.
pub const MIN_COMMIT_PERIODS: u64 = 2;

/// Checks a checkpoint age against the commit period: at least
/// [`MIN_COMMIT_PERIODS`] periods and [`MIN_STAGED_MILLIS`], at most
/// [`MAX_STAGED_MILLIS`].
///
/// # Errors
///
/// Why not, as the shell words it.
pub fn check(checkpoint_millis: u64, commit_period_millis: u64) -> Result<(), String> {
    let least = MIN_STAGED_MILLIS.max(MIN_COMMIT_PERIODS.saturating_mul(commit_period_millis));
    if least > MAX_STAGED_MILLIS {
        // ALLOC: error path; one message.
        return Err(format!(
            "the commit period of {commit_period_millis} ms leaves no checkpoint age: it must be \
             at least {MIN_COMMIT_PERIODS} commit periods and at most {MAX_STAGED_MILLIS} ms"
        ));
    }
    if checkpoint_millis < least || checkpoint_millis > MAX_STAGED_MILLIS {
        // ALLOC: error path; one message.
        return Err(format!(
            "the checkpoint age must be between {least} ms and {MAX_STAGED_MILLIS} ms, got \
             {checkpoint_millis}"
        ));
    }
    Ok(())
}

/// Whether a commit staged `elapsed` ago is due to be written at `checkpoint`.
pub(super) fn due(elapsed: Duration, checkpoint: Duration) -> bool {
    elapsed >= checkpoint
}

/// What a publisher holds besides the register: the newest staged commit.
#[derive(Debug)]
pub(super) struct Staged {
    /// The manifest as it would be written.
    pub(super) manifest: Vec<u8>,
    /// Every file the commit names.
    pub(super) files: BTreeMap<String, Known>,
    pub(super) term: u64,
    pub(super) writer: String,
    /// When the oldest pack a staged commit names landed, or the first was staged.
    pub(super) oldest: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_is_due_at_exactly_the_checkpoint_age() {
        let age = Duration::from_secs(60);
        assert!(!due(age - Duration::from_nanos(1), age));
        assert!(due(age, age));
        assert!(due(age + Duration::from_nanos(1), age));
    }

    #[test]
    fn the_checkpoint_age_has_the_shells_bounds() {
        assert!(check(1_000, 0).is_ok());
        assert!(check(999, 0).is_err());
        assert!(check(MAX_STAGED_MILLIS, 0).is_ok());
        assert!(check(MAX_STAGED_MILLIS + 1, 0).is_err());
        assert!(check(4_000, 2_000).is_ok(), "two commit periods");
        assert!(check(3_999, 2_000).is_err());
        let too_long = MAX_STAGED_MILLIS / MIN_COMMIT_PERIODS + 1;
        assert!(check(MAX_STAGED_MILLIS, too_long).is_err(), "no age fits");
    }

    #[test]
    fn the_settings_default_to_off_and_a_minute() {
        assert!(!STAGING.default());
        assert_eq!(CHECKPOINT_MILLIS.default(), 60_000);
        assert!(check(60_000, 5_000).is_ok());
    }
}
