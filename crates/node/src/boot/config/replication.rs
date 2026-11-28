//! The `OSS_REPLICATION*` variables: cross-cluster replication's switch and
//! where a remote may be read from.

use super::NodeConfig;

/// What a node serves of cross-cluster replication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplicationConfig {
    /// `serverless.replication.enabled`: on by default; nothing is polled until a replication is started.
    pub enabled: bool,
    /// `OSS_REPLICATION_REMOTE_ROOTS`: comma-separated absolute directories and `bucket[/prefix]` roots a remote may sit under; none by default, so no remote is readable.
    pub remote_roots: String,
}

/// Reads the switch with `flag` and the roots with `or`.
pub(in crate::boot) fn read(
    flag: &dyn Fn(&str, &str) -> Result<bool, String>,
    or: &dyn Fn(&str, &str) -> String,
) -> Result<ReplicationConfig, String> {
    Ok(ReplicationConfig {
        enabled: flag("OSS_REPLICATION", "true")?,
        remote_roots: or("OSS_REPLICATION_REMOTE_ROOTS", ""),
    })
}

/// The roots as (directories, s3 roots): an absolute path is a directory.
fn split(roots: &str) -> (String, String) {
    let mut fs = Vec::new(); // ALLOC: one entry a listed root, once a node.
    let mut s3 = Vec::new(); // ALLOC: as above.
    for root in roots.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        if std::path::Path::new(root).is_absolute() {
            fs.push(root);
        } else {
            s3.push(root);
        }
    }
    (fs.join(","), s3.join(","))
}

/// Gives the assembled node what this node's variables set for replication.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.replication = from.replication.enabled;
    to.replication_remote_roots = split(&from.replication.remote_roots);
}

#[cfg(test)]
mod tests {
    use super::super::NodeConfig;
    use super::super::tests::{lookup, with};

    fn config(extra: &[(&str, &str)]) -> super::ReplicationConfig {
        NodeConfig::from_lookup(lookup(&with(extra)))
            .expect("valid")
            .replication
    }

    #[test]
    fn replication_is_on_with_no_remote_readable_until_roots_are_set() {
        let defaults = config(&[]);
        assert!(defaults.enabled);
        assert_eq!(defaults.remote_roots, "");
        let off = config(&[("OSS_REPLICATION", "false")]);
        assert!(!off.enabled);
        assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_REPLICATION", "maybe")]))).is_err());
    }

    #[test]
    fn roots_split_into_directories_and_buckets_and_reach_the_assembled_config() {
        let from = NodeConfig::from_lookup(lookup(&with(&[(
            "OSS_REPLICATION_REMOTE_ROOTS",
            "/srv/a, shared/leaders ,/srv/b,other",
        )])))
        .expect("valid");
        let mut to = crate::assemble::Config::new("", "n", [0; 6]);
        assert!(to.replication);
        assert_eq!(to.replication_remote_roots, (String::new(), String::new()));
        super::apply(&from, &mut to);
        assert_eq!(
            to.replication_remote_roots,
            (
                "/srv/a,/srv/b".to_owned(),
                "shared/leaders,other".to_owned()
            )
        );
    }
}
