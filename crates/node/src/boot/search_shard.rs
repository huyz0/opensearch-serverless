//! The `internal:serverless/search/shard` action of a node that writes: the
//! slot it is registered in at start, and the handler set once the roles
//! exist (`serverless.search.shard_action.enabled`).

use std::sync::{Arc, OnceLock};

use oss_blob::Shared;
use oss_mem::MemoryBudget;
use oss_transport::actions::search_shard::SEARCH_ACTION;
use oss_transport::server::Handler;

use super::config::NodeConfig;
use super::error::BootError;
use crate::handlers::Registry;
use crate::lease_age::LeaseAge;
use crate::search::SearchService;
use crate::search_shard_action::{Limits, handler};
use crate::writer::WriterRole;

type Slot = Arc<OnceLock<Arc<dyn Handler>>>;

/// The action's slot, when the setting is on, and the marks of this node's
/// lease renewals that the handler reads.
pub(super) struct Wiring {
    slot: Option<Slot>,
    /// Marked by the lease thread at each renewal that succeeds.
    pub(super) age: Arc<LeaseAge>,
}

/// Registers the action, answering "still starting" until [`Wiring::install`].
pub(super) fn register(registry: &mut Registry, config: &NodeConfig) -> Result<Wiring, BootError> {
    let late = super::transport::Late::default();
    let slot = config.search_shard_action.then(|| late.slot());
    if slot.is_some() {
        registry
            .add(SEARCH_ACTION, late)
            .map_err(|e| BootError::Cluster(e.to_string()))?;
    }
    Ok(Wiring {
        slot,
        age: Arc::default(),
    })
}

impl Wiring {
    /// Sets the handler over this node's writer role and search service.
    pub(super) fn install(
        &self,
        role: &Arc<WriterRole>,
        search: &Arc<SearchService<Shared>>,
        config: &NodeConfig,
        budget: &MemoryBudget,
    ) {
        if let Some(slot) = &self.slot {
            let limits = Limits {
                max_lease_age_millis: config.search_shard_max_lease_age_millis,
                max_concurrent: config.search_shard_max_concurrent,
            };
            let handler = handler(
                Arc::clone(role),
                Arc::clone(search),
                Arc::clone(&self.age),
                budget.clone(),
                limits,
            );
            slot.set(Arc::new(handler)).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use oss_transport::auth::{AuthConfig, Authenticator, SecretSource, Secrets};

    use super::*;

    struct Secret;

    impl SecretSource for Secret {
        fn secrets(&self) -> Option<Arc<Secrets>> {
            Some(Arc::new(Secrets {
                generation: 1,
                current: "00".repeat(32),
                previous: None,
                rotated_at_millis: 0,
            }))
        }

        fn refresh(&self) {}
    }

    fn registry() -> Registry {
        Registry::new(Arc::new(Authenticator::new(
            AuthConfig {
                identity: "n1".to_owned(),
                window_millis: 60_000,
                replay_capacity: 8,
            },
            Arc::new(Secret),
            Arc::new(|| 0),
        )))
    }

    #[test]
    fn the_action_is_registered_when_the_setting_is_on_and_not_when_it_is_off() {
        let node = |extra: Option<(&'static str, &'static str)>| {
            NodeConfig::from_lookup(|key| {
                extra
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_owned())
                    .or_else(|| match key {
                        "OSS_NODE_ID" => Some("n1".to_owned()),
                        "OSS_STORE" => Some("fs".to_owned()),
                        "OSS_FS_ROOT" => Some("/data".to_owned()),
                        _ => None,
                    })
            })
            .expect("valid")
        };
        let on = node(None);
        let mut registry = registry();
        let wiring = register(&mut registry, &on).expect("registers");
        assert!(wiring.slot.is_some());
        assert_eq!(registry.actions(), [SEARCH_ACTION]);
        let off = node(Some(("OSS_SEARCH_SHARD_ACTION", "false")));
        let mut registry = self::registry();
        let wiring = register(&mut registry, &off).expect("registers");
        assert!(wiring.slot.is_none());
        assert!(registry.actions().is_empty());
    }
}
