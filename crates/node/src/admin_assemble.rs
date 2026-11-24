//! The cluster settings and index table routes a node always serves, and the
//! template routes when templates are on: they
//! read the registers under the deployment's base and need nothing else.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_mem::MemoryBudget;
use oss_meta::DescriptorStore;
use oss_rest::Router;

use crate::assemble::{AssembleError, Config};
use crate::cat_route::{self, CatService};
use crate::cluster_route::{self, ClusterService};
use crate::ingest_run::IngestPipelines;
use crate::pipeline_route::{self, PipelineService};
use crate::security_auth::{AuthGuard, Authenticator};
use crate::security_authz::RouteAuthorizer;
use crate::security_route::{self, SecurityService};
use crate::security_state::{Clock, SecurityState};
use crate::snapshot_route::{self, SnapshotService};
use crate::snapshot_take_route::SnapshotTakeService;
use crate::template_route::{self, TemplateService};

/// Registers `GET`/`PUT /_cluster/settings` and the `_cat` and `_list` index
/// tables on `router`, over `store`.
pub(crate) fn register(
    router: Router,
    store: &Arc<dyn BlobStore>,
    config: &Config,
    budget: &MemoryBudget,
) -> Result<Router, AssembleError> {
    let store_error = |e: oss_meta::MetaError| AssembleError::Store(e.to_string());
    let settings = ClusterService::new(Arc::clone(store), &config.base, config.publish_tick_millis)
        .map_err(store_error)?;
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(store_error)?;
    let router = cluster_route::routes(router, Arc::new(settings)).map_err(AssembleError::Route)?;
    let router = cat_route::routes(router, Arc::new(CatService::new(descriptors)))
        .map_err(AssembleError::Route)?;
    let router = if config.snapshots {
        let service = Arc::new(SnapshotService::new(
            Arc::clone(store),
            &config.base,
            &config.node_id,
            Arc::new(crate::clock::now_millis),
        ));
        let take = if config.snapshots_take {
            let descriptors =
                DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
                    .map_err(store_error)?;
            Some(Arc::new(SnapshotTakeService::new(
                Arc::clone(store),
                &config.base,
                descriptors,
                Arc::new(crate::clock::now_millis),
            )))
        } else {
            None
        };
        snapshot_route::routes(router, &service, take.as_ref()).map_err(AssembleError::Route)?
    } else {
        router
    };
    let router = if config.index_templates {
        let templates = TemplateService::new(store, &config.base, config.pit_buffer_bytes)
            .map_err(store_error)?;
        template_route::routes(router, &Arc::new(templates)).map_err(AssembleError::Route)?
    } else {
        router
    };
    let router = if config.security {
        security(router, store, config, budget)?
    } else {
        router
    };
    let router = plugin_routes(config, router)?;
    if !config.ingest_pipelines {
        return Ok(router);
    }
    let pipelines =
        PipelineService::new(store, &config.base, config.pit_buffer_bytes).map_err(store_error)?;
    pipeline_route::routes(router, &Arc::new(pipelines)).map_err(AssembleError::Route)
}

/// The prefix routes of the plugins not ported, proxied to the Java upstreams
/// when there are any.
fn plugin_routes(config: &Config, router: Router) -> Result<Router, AssembleError> {
    if config.plugin_upstreams.is_empty() {
        return Ok(router);
    }
    let proxy = crate::plugin_proxy::PluginProxy::new(&crate::plugin_proxy::ProxySettings {
        upstreams: config.plugin_upstreams.clone(),
        max_request_bytes: config.plugin_max_request_bytes,
        timeout: config.plugin_timeout,
    })
    .map_err(|e| AssembleError::Store(format!("serverless.plugins.upstreams: {}", e.0)))?;
    Arc::new(proxy)
        .register(router)
        .map_err(AssembleError::Route)
}

/// The pipelines writes run, when the setting is on.
pub(crate) fn ingest_pipelines(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
) -> Result<Option<Arc<IngestPipelines>>, crate::assemble::AssembleError> {
    if !config.ingest_pipelines {
        return Ok(None);
    }
    IngestPipelines::open(store, &config.base, config.pit_buffer_bytes)
        .map(Some)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))
}

/// The security routes, the authinfo route, and the guard that authenticates
/// every request, over one snapshot of the configuration.
fn security(
    router: Router,
    store: &Arc<dyn BlobStore>,
    config: &Config,
    budget: &MemoryBudget,
) -> Result<Router, AssembleError> {
    let admin = config.security_admin.clone().ok_or_else(|| {
        AssembleError::Security(
            "serverless.security.enabled needs an administrator (OSS_SECURITY_ADMIN_USER and OSS_SECURITY_ADMIN_HASH)"
                .to_owned(),
        )
    })?;
    if bcrypt::verify("", &admin.1).is_err() {
        return Err(AssembleError::Security(
            "OSS_SECURITY_ADMIN_HASH is not a bcrypt hash".to_owned(),
        ));
    }
    if config.security_refresh_millis < 1000 {
        return Err(AssembleError::Security(
            "the security refresh interval is at least 1 s".to_owned(),
        ));
    }
    let clock: Clock = Arc::new(crate::clock::monotonic_millis);
    let state = Arc::new(
        SecurityState::new(
            store,
            &config.base,
            config.pit_buffer_bytes,
            config.security_refresh_millis,
            admin,
            budget,
            Arc::clone(&clock),
        )
        .map_err(AssembleError::Security)?,
    );
    let service = SecurityService::new(
        store,
        &config.base,
        config.pit_buffer_bytes,
        config.security_bcrypt_cost,
    )
    .map_err(|e| AssembleError::Store(e.to_string()))?
    .with_state(Arc::clone(&state));
    let router =
        security_route::routes(router, &Arc::new(service)).map_err(AssembleError::Route)?;
    let router = security_route::authinfo_route(router, &state).map_err(AssembleError::Route)?;
    let authorizer = RouteAuthorizer::new(Arc::clone(&state));
    let guard = AuthGuard(Arc::new(Authenticator::new(state, clock)));
    Ok(router
        .with_guard(Arc::new(guard))
        .with_authorizer(Arc::new(authorizer)))
}

#[cfg(test)]
mod tests {
    use oss_blob::Emulator;
    use oss_mem::{BudgetConfig, Pool};

    use super::*;

    fn parts() -> (Arc<dyn BlobStore>, MemoryBudget) {
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let budget = MemoryBudget::new(
            &BudgetConfig::new(64 << 20)
                .with_pool(Pool::RequestBodies, 32 << 20)
                .expect("fits"),
        );
        (store, budget)
    }

    // docs/specs/security-authn AC-9
    #[test]
    fn security_on_without_an_administrator_does_not_start() {
        let (store, budget) = parts();
        let config = Config {
            security: true,
            ..Config::new("", "n", [0; 6])
        };
        let err = register(Router::new(), &store, &config, &budget)
            .err()
            .expect("refused");
        assert!(matches!(err, AssembleError::Security(m) if m.contains("administrator")));
        let hash = bcrypt::hash("p", 4).expect("hashes");
        let not_a_hash = Config {
            security_admin: Some(("a".into(), "h".into())),
            ..config.clone()
        };
        assert!(matches!(
            register(Router::new(), &store, &not_a_hash, &budget).err(),
            Some(AssembleError::Security(m)) if m.contains("bcrypt")
        ));
        let too_often = Config {
            security_admin: Some(("a".into(), hash)),
            security_refresh_millis: 999,
            ..config
        };
        assert!(matches!(
            register(Router::new(), &store, &too_often, &budget).err(),
            Some(AssembleError::Security(m)) if m.contains("1 s")
        ));
    }

    // docs/specs/security-authn AC-9
    #[test]
    fn with_security_off_no_guard_is_installed() {
        let (store, budget) = parts();
        let config = Config::new("", "n", [0; 6]);
        let router = register(Router::new(), &store, &config, &budget).expect("registers");
        assert!(!router.has_guard());
        assert!(!router.has_authorizer());
    }

    // docs/specs/security-authz AC-9
    #[test]
    fn with_security_on_a_guard_and_an_authorizer_are_installed() {
        let (store, _) = parts();
        let budget = MemoryBudget::new(
            &BudgetConfig::new(256 << 20)
                .with_pool(Pool::RequestBodies, 96 << 20)
                .expect("fits"),
        );
        let config = Config {
            security: true,
            security_admin: Some(("a".into(), bcrypt::hash("p", 4).expect("hashes"))),
            ..Config::new("", "n", [0; 6])
        };
        let router = register(Router::new(), &store, &config, &budget).expect("registers");
        assert!(router.has_guard());
        assert!(router.has_authorizer());
    }
}
