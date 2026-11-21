//! The cluster register's settings, read into the node's [`Live`] so a
//! setting that may change while a node runs is followed without a restart.
//!
//! One GET of the register a read. The Java shell publishes cluster settings
//! to every node's state; here the register is the only place they are, and
//! each node that has a dynamic setting to follow reads it.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_format::{MAX_CLUSTER_SETTINGS_BYTES, Setting as Held};
use oss_meta::{ClusterConfig, MetaError};
use oss_types::settings::{Live, Snapshot, Source};

use crate::tail_overlay::REFRESH_VISIBILITY;
use crate::writer::RefreshVisibility;

/// The buffer the register is read through: written, a control character
/// takes six bytes.
const BUFFER: usize = 6 * MAX_CLUSTER_SETTINGS_BYTES;

/// Reads the cluster register into a [`Live`].
pub struct ClusterFeed {
    config: ClusterConfig,
    live: Arc<Live>,
    buffer: Mutex<Vec<u8>>,
}

impl ClusterFeed {
    /// A feed of the register under `base` of `store` into `live`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end
    /// in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, live: Arc<Live>) -> Result<Self, MetaError> {
        Ok(Self {
            config: ClusterConfig::new(store, base)?,
            live,
            // ALLOC: one buffer a node, BUFFER bytes.
            buffer: Mutex::new(vec![0u8; BUFFER]),
        })
    }

    /// The settings in force.
    pub fn live(&self) -> &Arc<Live> {
        &self.live
    }

    /// Reads the register once and puts what it holds in force when it is
    /// newer than what is. Gives whether it was.
    ///
    /// # Errors
    ///
    /// As [`ClusterConfig::read`].
    pub fn read(&self) -> Result<bool, MetaError> {
        let mut buffer = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        let value = self.config.read(&mut buffer)?;
        // ALLOC: one entry a setting, bounded by the register's size limit.
        let values = value
            .settings
            .settings()
            .iter()
            .filter_map(|(key, held)| match held {
                Held::Value(v) => Some((key.clone(), v.clone())),
                // A list is no setting this node reads.
                Held::List(_) => None,
            })
            .collect();
        Ok(self.live.replace(Snapshot {
            generation: value.generation,
            values,
        }))
    }
}

/// The refresh visibility in force: the cluster's value when it names a
/// mode, else `node`'s.
pub fn visibility(cluster: &dyn Source, node: RefreshVisibility) -> RefreshVisibility {
    cluster
        .value(REFRESH_VISIBILITY.setting().key)
        .and_then(RefreshVisibility::parse)
        .unwrap_or(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::Emulator;

    fn feed() -> (ClusterFeed, ClusterConfig) {
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
        (
            ClusterFeed::new(Arc::clone(&store), "", Arc::new(Live::new())).expect("feed"),
            ClusterConfig::new(store, "").expect("config"),
        )
    }

    fn put(config: &ClusterConfig, changes: &[(&str, Option<&str>)]) {
        let mut buf = vec![0u8; BUFFER];
        config.update(changes, &mut buf).expect("updated");
    }

    #[test]
    fn a_register_never_written_leaves_nothing_in_force() {
        let (feed, _) = feed();
        assert!(!feed.read().expect("read"));
        assert_eq!(feed.live().current().values.len(), 0);
    }

    #[test]
    fn what_the_register_holds_is_put_in_force_once_per_generation() {
        let (feed, config) = feed();
        put(
            &config,
            &[("serverless.search.refresh_visibility", Some("owner"))],
        );
        assert!(feed.read().expect("read"));
        assert_eq!(
            feed.live()
                .current()
                .value("serverless.search.refresh_visibility"),
            Some("owner")
        );
        // The same generation again changes nothing.
        assert!(!feed.read().expect("read"));
        put(
            &config,
            &[("serverless.search.refresh_visibility", Some("serving"))],
        );
        assert!(feed.read().expect("read"));
        assert_eq!(
            feed.live()
                .current()
                .value("serverless.search.refresh_visibility"),
            Some("serving")
        );
    }

    #[test]
    fn a_removed_key_goes_and_the_rest_stay() {
        let (feed, config) = feed();
        put(
            &config,
            &[
                ("a.b", Some("1")),
                ("serverless.search.refresh_visibility", Some("owner")),
            ],
        );
        assert!(feed.read().expect("read"));
        put(&config, &[("serverless.search.refresh_visibility", None)]);
        assert!(feed.read().expect("read"));
        let now = feed.live().current();
        assert_eq!(now.value("a.b"), Some("1"));
        assert_eq!(now.value("serverless.search.refresh_visibility"), None);
    }

    #[test]
    fn the_cluster_value_wins_when_it_names_a_mode() {
        let mut values = std::collections::BTreeMap::new();
        let node = RefreshVisibility::Everywhere;
        assert_eq!(visibility(&values, node), node);
        values.insert(
            "serverless.search.refresh_visibility".to_owned(),
            "owner".to_owned(),
        );
        assert_eq!(visibility(&values, node), RefreshVisibility::Owner);
        values.insert(
            "serverless.search.refresh_visibility".to_owned(),
            "everything".to_owned(),
        );
        assert_eq!(
            visibility(&values, node),
            node,
            "an unknown word is ignored"
        );
    }
}
