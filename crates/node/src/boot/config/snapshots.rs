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
    /// `serverless.snapshots.stock.enabled`: restore from a stock OpenSearch repository. Off by default: reading another system's files is opt-in.
    pub stock: bool,
    /// `OSS_STOCK_REPO_ROOTS`: comma-separated directories a stock `fs` repository may sit under; none by default, so none is readable.
    pub stock_fs_roots: String,
    /// `OSS_STOCK_REPO_S3_ROOTS`: comma-separated `bucket` or `bucket/prefix` roots a stock `s3` repository may sit under; none by default.
    pub stock_s3_roots: String,
    /// `OSS_STOCK_RESTORE_FANOUT`: how many metadata blobs a stock restore reads at once, 1 to 64.
    pub stock_fanout: usize,
    /// `serverless.snapshots.stock.export.enabled`: export a snapshot into a stock repository. Off by default: it copies the tenant's bytes out of the deployment.
    pub stock_export: bool,
    /// `OSS_STOCK_EXPORT_FS_ROOTS`: comma-separated directories an export may write a stock `fs` repository under; none by default, so none is writable.
    pub stock_export_fs_roots: String,
    /// `OSS_STOCK_EXPORT_S3_ROOTS`: comma-separated `bucket` or `bucket/prefix` roots an export may write a stock `s3` repository under; none by default.
    pub stock_export_s3_roots: String,
}

/// The most metadata blobs a stock restore reads at once.
const MAX_FANOUT: usize = 64;

/// Reads the switches with `flag` and the patterns with `or`.
pub(in crate::boot) fn read(
    flag: &dyn Fn(&str, &str) -> Result<bool, String>,
    or: &dyn Fn(&str, &str) -> String,
) -> Result<SnapshotConfig, String> {
    let fanout: usize = or("OSS_STOCK_RESTORE_FANOUT", "8")
        .parse()
        .map_err(|e| format!("OSS_STOCK_RESTORE_FANOUT: {e}"))?;
    if !(1..=MAX_FANOUT).contains(&fanout) {
        return Err(format!(
            "OSS_STOCK_RESTORE_FANOUT [{fanout}] is not between 1 and {MAX_FANOUT}"
        ));
    }
    Ok(SnapshotConfig {
        enabled: flag("OSS_SNAPSHOTS_ENABLED", "true")?,
        take: flag("OSS_SNAPSHOTS_TAKE_ENABLED", "true")?,
        restore: flag("OSS_SNAPSHOTS_RESTORE_ENABLED", "true")?,
        system_index_patterns: or("OSS_SYSTEM_INDEX_PATTERNS", ""),
        stock: flag("OSS_SNAPSHOTS_STOCK_ENABLED", "false")?,
        stock_fs_roots: or("OSS_STOCK_REPO_ROOTS", ""),
        stock_s3_roots: or("OSS_STOCK_REPO_S3_ROOTS", ""),
        stock_fanout: fanout,
        stock_export: flag("OSS_SNAPSHOTS_STOCK_EXPORT_ENABLED", "false")?,
        stock_export_fs_roots: or("OSS_STOCK_EXPORT_FS_ROOTS", ""),
        stock_export_s3_roots: or("OSS_STOCK_EXPORT_S3_ROOTS", ""),
    })
}

/// Gives the assembled node what this node's variables set for snapshots.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.snapshots = from.snapshots.enabled;
    to.snapshots_take = from.snapshots.take;
    to.snapshots_restore = from.snapshots.restore;
    to.system_index_patterns
        .clone_from(&from.snapshots.system_index_patterns);
    to.snapshots_stock = from.snapshots.stock;
    to.stock_fs_roots.clone_from(&from.snapshots.stock_fs_roots);
    to.stock_s3_roots.clone_from(&from.snapshots.stock_s3_roots);
    to.stock_fanout = from.snapshots.stock_fanout;
    to.snapshots_stock_export = from.snapshots.stock_export;
    to.stock_export_fs_roots
        .clone_from(&from.snapshots.stock_export_fs_roots);
    to.stock_export_s3_roots
        .clone_from(&from.snapshots.stock_export_s3_roots);
    to.stock_s3 = match &from.store {
        super::StoreChoice::S3 {
            endpoint,
            bucket,
            region,
            access_key,
            secret_key,
        } => Some(crate::stock_s3::SourceS3 {
            endpoint: endpoint.clone(),
            region: region.clone(),
            credentials: oss_blob::Credentials::new(access_key, secret_key),
            own_bucket: bucket.clone(),
        }),
        super::StoreChoice::Fs { .. } => None,
    };
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
    fn the_stock_export_is_off_with_no_roots_until_set() {
        let defaults = config(&[]);
        assert!(!defaults.stock_export);
        assert_eq!(defaults.stock_export_fs_roots, "");
        assert_eq!(defaults.stock_export_s3_roots, "");
        let set = config(&[
            ("OSS_SNAPSHOTS_STOCK_EXPORT_ENABLED", "true"),
            ("OSS_STOCK_EXPORT_FS_ROOTS", "/srv/out"),
            ("OSS_STOCK_EXPORT_S3_ROOTS", "bkt/out"),
        ]);
        assert!(set.stock_export);
        assert_eq!(set.stock_export_fs_roots, "/srv/out");
        assert_eq!(set.stock_export_s3_roots, "bkt/out");
        // The restore's switches do not turn the export on, nor its roots open it.
        let restore_only = config(&[
            ("OSS_SNAPSHOTS_STOCK_ENABLED", "true"),
            ("OSS_STOCK_REPO_ROOTS", "/srv"),
        ]);
        assert!(!restore_only.stock_export);
        assert_eq!(restore_only.stock_export_fs_roots, "");
        let from = NodeConfig::from_lookup(lookup(&with(&[
            ("OSS_SNAPSHOTS_STOCK_EXPORT_ENABLED", "true"),
            ("OSS_STOCK_EXPORT_FS_ROOTS", "/srv/out"),
            ("OSS_STOCK_EXPORT_S3_ROOTS", "bkt/out"),
        ])))
        .expect("valid");
        let mut to = crate::assemble::Config::new("", "n1", [0; 6]);
        super::apply(&from, &mut to);
        assert!(to.snapshots_stock_export);
        assert_eq!(
            (
                to.stock_export_fs_roots.as_str(),
                to.stock_export_s3_roots.as_str()
            ),
            ("/srv/out", "bkt/out")
        );
    }

    #[test]
    fn the_stock_restore_is_off_with_no_roots_until_set() {
        let defaults = config(&[]);
        assert!(!defaults.stock);
        assert_eq!(defaults.stock_fs_roots, "");
        assert_eq!(defaults.stock_s3_roots, "");
        assert_eq!(defaults.stock_fanout, 8);
        let set = config(&[
            ("OSS_SNAPSHOTS_STOCK_ENABLED", "true"),
            ("OSS_STOCK_REPO_ROOTS", "/srv/a,/srv/b"),
            ("OSS_STOCK_REPO_S3_ROOTS", "src/stock"),
            ("OSS_STOCK_RESTORE_FANOUT", "64"),
        ]);
        assert!(set.stock);
        assert_eq!(set.stock_fs_roots, "/srv/a,/srv/b");
        assert_eq!(set.stock_s3_roots, "src/stock");
        assert_eq!(set.stock_fanout, 64);
        for bad in ["0", "65", "x", "-1"] {
            assert!(
                NodeConfig::from_lookup(lookup(&with(&[("OSS_STOCK_RESTORE_FANOUT", bad)])))
                    .is_err(),
                "{bad}"
            );
        }
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[("OSS_SNAPSHOTS_STOCK_ENABLED", "yes")])))
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
        let from = NodeConfig::from_lookup(lookup(&with(&[
            ("OSS_SNAPSHOTS_STOCK_ENABLED", "true"),
            ("OSS_STOCK_REPO_ROOTS", "/srv"),
            ("OSS_STOCK_REPO_S3_ROOTS", "b/p"),
            ("OSS_STOCK_RESTORE_FANOUT", "3"),
        ])))
        .expect("valid");
        let mut to = crate::assemble::Config::new("", "n1", [0; 6]);
        super::apply(&from, &mut to);
        assert!(to.snapshots_stock);
        assert_eq!(
            (
                to.stock_fs_roots.as_str(),
                to.stock_s3_roots.as_str(),
                to.stock_fanout
            ),
            ("/srv", "b/p", 3)
        );
    }
}
