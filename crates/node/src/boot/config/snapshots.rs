//! The `OSS_SNAPSHOTS_*` and `OSS_SYSTEM_INDEX_PATTERNS` variables: which
//! snapshot routes a node serves, and which index names they refuse.

use super::NodeConfig;

/// What a node serves of snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotConfig {
    /// `serverless.snapshots.enabled`: serve the snapshot read routes. On by default.
    pub enabled: bool,
    /// `serverless.snapshots.take.enabled`: take shallow snapshots, beside the snapshot routes. On by default.
    pub take: bool,
    /// `serverless.snapshots.restore.enabled`: restore shallow snapshots, beside the take. On by default.
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
        enabled: flag("OSS_SNAPSHOTS_ENABLED", "true")?,
        take: flag("OSS_SNAPSHOTS_TAKE_ENABLED", "true")?,
        restore: flag("OSS_SNAPSHOTS_RESTORE_ENABLED", "true")?,
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

#[cfg(test)]
mod tests {
    use super::super::NodeConfig;
    use super::super::tests::{lookup, with};

    fn config(extra: &[(&str, &str)]) -> super::SnapshotConfig {
        NodeConfig::from_lookup(lookup(&with(extra)))
            .expect("valid")
            .snapshots
    }

    #[test]
    fn the_reads_the_take_and_the_restore_are_on_until_set_and_each_stands_alone() {
        let defaults = config(&[]);
        assert!(defaults.enabled && defaults.take && defaults.restore);
        let snaps = config(&[("OSS_SNAPSHOTS_ENABLED", "false")]);
        assert!(!snaps.enabled && snaps.take && snaps.restore);
        let take = config(&[("OSS_SNAPSHOTS_TAKE_ENABLED", "false")]);
        assert!(!take.take && take.enabled && take.restore);
        let restore = config(&[("OSS_SNAPSHOTS_RESTORE_ENABLED", "false")]);
        assert!(!restore.restore && restore.enabled && restore.take);
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[("OSS_SNAPSHOTS_RESTORE_ENABLED", "yes")])))
                .is_err()
        );
    }

    #[test]
    fn the_system_index_patterns_are_read_as_given() {
        assert_eq!(config(&[]).system_index_patterns, "");
        let set = config(&[("OSS_SYSTEM_INDEX_PATTERNS", ".kibana*,.tasks")]);
        assert_eq!(set.system_index_patterns, ".kibana*,.tasks");
    }

    #[test]
    fn what_is_set_reaches_the_assembled_node() {
        let from = NodeConfig::from_lookup(lookup(&with(&[
            ("OSS_SNAPSHOTS_ENABLED", "true"),
            ("OSS_SNAPSHOTS_TAKE_ENABLED", "true"),
            ("OSS_SNAPSHOTS_RESTORE_ENABLED", "true"),
            ("OSS_SYSTEM_INDEX_PATTERNS", ".kibana*"),
        ])))
        .expect("valid");
        let mut to = crate::assemble::Config::new("", "n1", [0; 6]);
        super::apply(&from, &mut to);
        assert!(to.snapshots && to.snapshots_take && to.snapshots_restore);
        assert_eq!(to.system_index_patterns, ".kibana*");
    }
}
