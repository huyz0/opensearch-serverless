//! The `OSS_SNAPSHOTS_*` and `OSS_SYSTEM_INDEX_PATTERNS` variables: which
//! snapshot routes a node serves, and which index names they refuse.

use super::NodeConfig;

/// What a node serves of snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotConfig {
    /// `serverless.snapshots.enabled`: serve the read-only snapshot routes. Off by default.
    pub enabled: bool,
    /// `serverless.snapshots.take.enabled`: take shallow snapshots, beside the snapshot routes. Off by default.
    pub take: bool,
    /// `serverless.snapshots.restore.enabled`: restore shallow snapshots, beside the take. Off by default.
    pub restore: bool,
    /// `serverless.system_indices.patterns`: comma-separated `*` patterns of the
    /// index names a take and a restore refuse with 403; none by default.
    pub system_index_patterns: String,
}

/// Reads the switches with `flag` and the patterns with `or`.
pub(in crate::boot) fn read(
    flag: &dyn Fn(&str, &str) -> Result<bool, String>,
    or: &dyn Fn(&str, &str) -> String,
) -> Result<SnapshotConfig, String> {
    Ok(SnapshotConfig {
        enabled: flag("OSS_SNAPSHOTS_ENABLED", "false")?,
        take: flag("OSS_SNAPSHOTS_TAKE_ENABLED", "false")?,
        restore: flag("OSS_SNAPSHOTS_RESTORE_ENABLED", "false")?,
        system_index_patterns: or("OSS_SYSTEM_INDEX_PATTERNS", ""),
    })
}

/// Gives the assembled node what this node's variables set for snapshots.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.snapshots = from.snapshots.enabled;
    to.snapshots_take = from.snapshots.take;
    to.snapshots_restore = from.snapshots.restore;
    to.system_index_patterns
        .clone_from(&from.snapshots.system_index_patterns);
}
