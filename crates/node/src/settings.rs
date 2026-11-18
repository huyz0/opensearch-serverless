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
        #[cfg(feature = "writer")]
        oss_writer::log::settings::SETTINGS,
        #[cfg(feature = "writer")]
        oss_cluster::early_revoke::SETTINGS,
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
            assert_eq!(keys.len(), if writer { 12 } else { 6 });
            assert_eq!(keys.contains(&"serverless.lease.early_revoke"), writer);
            assert_eq!(keys.contains(&"serverless.wal.node_log.idle_flush"), writer);
        } else {
            assert!(keys.is_empty());
        }
    }
}
