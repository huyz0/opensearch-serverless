//! Every setting this node's crates define, as one registry (the shell's
//! `ServerlessSettings`, which collects each module's provider).

use oss_types::settings::{Registry, RegistryError, Setting};

/// The settings providers of the crates this binary was built with. Every
/// role runs on an object store, so each brings the store settings.
fn providers() -> Vec<&'static [&'static Setting]> {
    // ALLOC: one entry a crate, once a node.
    vec![
        #[cfg(any(feature = "reader", feature = "writer", feature = "coordinator"))]
        oss_blob::settings::SETTINGS,
        #[cfg(feature = "reader")]
        crate::newest_ask::SETTINGS,
        #[cfg(feature = "reader")]
        crate::search::open_prefetch::SETTINGS,
        #[cfg(feature = "reader")]
        crate::vector_warm_setting::SETTINGS,
        #[cfg(feature = "reader")]
        crate::tail_overlay::SETTINGS,
        #[cfg(all(feature = "reader", feature = "writer"))]
        crate::commit_push::SETTINGS,
        #[cfg(all(feature = "reader", feature = "writer"))]
        crate::warm_setting::SETTINGS,
        #[cfg(all(feature = "writer", feature = "coordinator"))]
        crate::knn_setting::SETTINGS,
        #[cfg(all(feature = "writer", feature = "coordinator"))]
        crate::nested_setting::SETTINGS,
        crate::security_setting::SETTINGS,
        #[cfg(feature = "extensions")]
        crate::extension_setting::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::log::settings::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::admission::settings::SETTINGS,
        #[cfg(feature = "writer")]
        oss_cluster::early_revoke::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::publish::lazy::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::publish::staging::SETTINGS,
        #[cfg(feature = "writer")]
        oss_janitor::settings::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::bucket::settings::SETTINGS,
        #[cfg(feature = "writer")]
        oss_writer::bucket::policy::SETTINGS,
        #[cfg(feature = "writer")]
        crate::writer::PROACTIVE_SETTINGS,
        #[cfg(feature = "writer")]
        crate::writer::HIBERNATE_SETTINGS,
        #[cfg(feature = "writer")]
        crate::writer::CAPACITY_SETTINGS,
        #[cfg(feature = "writer")]
        crate::writer::MERGE_SETTINGS,
        #[cfg(feature = "writer")]
        crate::writer::PACK_SETTINGS,
    ]
}

/// The node's settings registry.
///
/// # Errors
///
/// [`RegistryError`] when two crates define one key, or a definition breaks
/// the registry's rules: a fault in the build, so the node refuses to start.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::new(&providers())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nodes_settings_form_a_valid_registry() {
        let registry = registry().expect("the node's settings pass the registry's rules");
        let keys: Vec<&str> = registry.all().map(|s| s.key).collect();
        if cfg!(any(
            feature = "reader",
            feature = "writer",
            feature = "coordinator"
        )) {
            assert!(keys.contains(&"serverless.store.type"), "{keys:?}");
            let writer = cfg!(feature = "writer");
            let reader = cfg!(feature = "reader");
            assert_eq!(
                keys.len(),
                7 + 7 * usize::from(reader)
                    + if writer { 51 + usize::from(reader) } else { 0 }
                    + usize::from(reader && writer)
                    + 2 * usize::from(writer && cfg!(feature = "coordinator"))
                    + usize::from(cfg!(feature = "extensions"))
            );
            assert_eq!(
                keys.contains(&"serverless.extensions.pins.allowed"),
                cfg!(feature = "extensions")
            );
            assert_eq!(
                keys.contains(&"serverless.reader.newest_commit_ask"),
                reader
            );
            assert_eq!(
                keys.contains(&"serverless.search.tail_overlay.enabled"),
                reader
            );
            assert_eq!(
                keys.contains(&"serverless.reader.open_prefetch.enabled"),
                reader
            );
            assert_eq!(
                keys.contains(&"index.serverless.reader.open_prefetch.enabled"),
                reader
            );
            assert_eq!(keys.contains(&"serverless.lease.early_revoke"), writer);
            assert_eq!(
                keys.contains(&"serverless.wal.node_log.publish_max_age_millis"),
                writer
            );
            assert_eq!(
                keys.contains(&"serverless.publish.max_unpublished_operations"),
                writer
            );
            assert_eq!(keys.contains(&"serverless.wal.node_log.idle_flush"), writer);
            assert_eq!(
                keys.contains(&"serverless.write.backpressure.max_in_flight"),
                writer
            );
            for key in [
                "serverless.takeover.proactive.enabled",
                "serverless.takeover.proactive.reapers",
                "serverless.takeover.proactive.max_concurrent",
                "serverless.takeover.proactive.pass_millis",
                "serverless.takeover.proactive.rust_reapers_only",
                "serverless.activation.hibernate_after_millis",
                "serverless.activation.give_back_after_millis",
                "serverless.activation.max_shards",
                "serverless.activation.evict_after_millis",
                "serverless.activation.evict_headroom_percent",
                "serverless.activation.pinned_indexes",
                "serverless.activation.hand_off.enabled",
                "serverless.writer.merge.enabled",
                "serverless.writer.merge.segments_per_tier",
                "serverless.writer.merge.max_merge_at_once",
                "serverless.writer.merge.max_merged_segment_mb",
                "serverless.wal.node_log.commits.staging",
                "serverless.wal.node_log.commits.checkpoint_millis",
                "serverless.wal.node_log.commits.logged",
                "serverless.wal.node_log.commits.open_from_logged",
            ] {
                assert_eq!(keys.contains(&key), writer, "{key}");
            }
        } else {
            assert!(keys.is_empty());
        }
    }
}
