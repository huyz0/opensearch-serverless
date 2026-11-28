//! Where a replication remote may be read from.

use oss_coordinator::cluster::settings::Setting;

/// The operator's roots and this deployment's own bucket and prefix, which a
/// remote may not overlap. With no roots every remote is refused.
#[derive(Clone, Default)]
pub struct RemoteRoots {
    roots: oss_blob::SourceRoots,
    own: Option<(String, String)>,
}

impl RemoteRoots {
    /// The roots from `(directories, s3 roots)`, and this deployment's own
    /// S3 source (none when its store is not S3) under `base`.
    #[must_use]
    pub fn new(
        roots: &(String, String),
        own: Option<&crate::stock_s3::SourceS3>,
        base: &str,
    ) -> Self {
        Self {
            roots: oss_blob::SourceRoots::parse(&roots.0, &roots.1),
            own: own.map(|s3| (s3.own_bucket.clone(), base.to_owned())), // ALLOC: once a node.
        }
    }

    /// The roots and own store a node's assembled configuration names.
    #[cfg(feature = "reader")]
    #[must_use]
    pub fn of(config: &crate::assemble::Config) -> Self {
        Self::new(
            &config.replication_remote_roots,
            config.stock_s3.as_ref(),
            &config.base,
        )
    }

    /// The refusal of a settings update's remotes, if any.
    pub(crate) fn check(&self, settings: &[Setting]) -> Result<(), String> {
        let own = self.own.as_ref().map(|(b, p)| (b.as_str(), p.as_str()));
        for remote in oss_replication::remotes(settings).map_err(|e| e.to_string())? {
            remote
                .check_roots(&self.roots, own)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
